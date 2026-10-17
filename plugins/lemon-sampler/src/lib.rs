// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Sampler: Lemon Studio's sampler instrument as a VST3 / CLAP plugin (nih-plug).
//!
// spec: plugins/lemon-sampler.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod editor;
pub mod fx;
pub mod params;
pub mod view;

use std::num::NonZeroU32;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use os_engine::{envelope, Sample};

use params::{FxKey, Live, SamplerParams};

/// The processed sample for one (loaded sample version, precomputed settings) pair.
pub struct Processed {
    pub version: u64,
    pub key: FxKey,
    pub sample: Option<Arc<Sample>>,
    /// The played sample's loop (start and end frame, both inclusive), whether or not voices use it.
    pub looped: Option<(usize, usize)>,
    /// Frames of the played sample the whole loaded file stands for (per-note starts are fractions of the file).
    pub file_frames: f64,
}

/// State shared by the audio thread, the background task and the editor.
pub struct Shared {
    pub raw: Mutex<Option<Arc<Sample>>>,
    /// Bumped on every load / clear.
    pub raw_version: AtomicU64,
    pub processed: Mutex<Option<Processed>>,
    processed_version: AtomicU64,
    loaded_path: Mutex<String>,
    /// Source bit depth (low byte) and 0x100 when the source is floating point.
    pub bits: AtomicU32,
    /// What the played sample's steps need to know of the loaded file (its loop, 16-bit mono).
    source: Mutex<fx::Source>,
    /// The host's output rate (Hz), 44100 until the host tells.
    rate: AtomicU32,
    /// Host tempo (f64 bits), 130 BPM until the host tells.
    tempo: AtomicU64,
    /// Notes the editor plays itself (key, velocity, on) when the host does not play them.
    pub previews: Mutex<Vec<(u8, f32, bool)>>,
    /// Keys sounding (bit per key, 132 keys).
    pub sounding: [AtomicU64; 3],
    busy: AtomicBool,
    again: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            raw: Mutex::new(None),
            raw_version: AtomicU64::new(0),
            processed: Mutex::new(None),
            processed_version: AtomicU64::new(0),
            loaded_path: Mutex::new(String::new()),
            bits: AtomicU32::new(16),
            source: Mutex::new(fx::Source::default()),
            rate: AtomicU32::new(44100),
            tempo: AtomicU64::new(130f64.to_bits()),
            previews: Mutex::new(Vec::new()),
            sounding: Default::default(),
            busy: AtomicBool::new(false),
            again: AtomicBool::new(false),
        }
    }
}

/// Bit depth of a sample file's data (WAV `fmt `, FLAC STREAMINFO), | 0x100 for floating point; 16 when unknown.
pub fn source_bits(path: &Path) -> u32 {
    source_info(path).0
}

/// A sample file's bit depth as [`source_bits`] gives it, and what the played sample needs from the file beyond its
/// frames: the first loop of a WAV file's `smpl` chunk (spec: audio/sampler.md "The sample file") and whether the
/// file is 16-bit mono PCM (audio/precomputed-effects.md "Stereo delay"). A WAV file's chunks are walked without
/// reading its audio. **(Lemon Studio)** Loops are read from WAV files only (not yet from a WavPack file's wrapper).
pub fn source_info(path: &Path) -> (u32, fx::Source) {
    let mut src = fx::Source::default();
    let Ok(mut f) = std::fs::File::open(path) else {
        return (16, src);
    };
    let mut head = Vec::new();
    if (&mut f).take(64).read_to_end(&mut head).is_err() {
        return (16, src);
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        let mut bits = 16;
        if f.seek(SeekFrom::Start(12)).is_err() {
            return (bits, src);
        }
        let word = |d: &[u8], i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
        let long = |d: &[u8], i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
        let mut chunk = [0u8; 8];
        while f.read_exact(&mut chunk).is_ok() {
            let len = u64::from(long(&chunk, 4));
            let id = [chunk[0], chunk[1], chunk[2], chunk[3]];
            if (&id == b"fmt " || &id == b"smpl") && len <= 1 << 16 {
                let mut d = vec![0u8; len as usize];
                if f.read_exact(&mut d).is_err() {
                    break;
                }
                if len & 1 == 1 && f.seek(SeekFrom::Current(1)).is_err() {
                    break;
                }
                if &id == b"fmt " && d.len() >= 16 {
                    // WAVE_FORMAT_EXTENSIBLE names the format in its sub-format's first two bytes.
                    let tag = if word(&d, 0) == 0xFFFE && d.len() >= 26 { word(&d, 24) } else { word(&d, 0) };
                    let (channels, depth) = (word(&d, 2), u32::from(word(&d, 14)));
                    bits = depth | if tag == 3 { 0x100 } else { 0 };
                    src.int16_mono = tag == 1 && channels == 1 && depth == 16;
                } else if &id == b"smpl" && d.len() >= 36 + 24 && long(&d, 28) >= 1 {
                    // 36 bytes of header, then 24 bytes per loop: id, type, start, end, fraction, play count.
                    src.file_loop = Some((long(&d, 36 + 8), long(&d, 36 + 12)));
                }
            } else if f.seek(SeekFrom::Current((len + (len & 1)) as i64)).is_err() {
                break;
            }
        }
        return (bits, src);
    }
    if head.len() > 22 && &head[0..4] == b"fLaC" {
        // STREAMINFO: bits per sample − 1 sits in 5 bits across bytes 20 and 21 of the file.
        return (u32::from(((head[20] & 1) << 4) | (head[21] >> 4)) + 1, src);
    }
    if head.len() > 4 && &head[0..4] == b"OggS" {
        return (32 | 0x100, src);
    }
    (16, src)
}

impl Shared {
    /// Loads a sample file and makes it the current one (UI / initialize thread). A decode the host already holds
    /// (it decodes a project's samples ahead, spec: ui/project-loading.md) is taken as it is.
    pub fn load(&self, path: &str, params: &SamplerParams) -> Result<(), String> {
        let s = os_engine::sample::load_shared(Path::new(path)).map_err(|e| e.to_string())?;
        let (bits, source) = source_info(Path::new(path));
        self.bits.store(bits, Ordering::Release);
        *self.source.lock().unwrap() = source;
        self.set_raw(Some(s), path, params);
        Ok(())
    }

    /// Replaces the loaded sample (`path` is what the state remembers).
    pub fn set_raw(&self, s: Option<Arc<Sample>>, path: &str, params: &SamplerParams) {
        *self.raw.lock().unwrap() = s;
        *self.loaded_path.lock().unwrap() = path.to_string();
        if let Ok(mut p) = params.sample_path.write() {
            *p = path.to_string();
        }
        self.raw_version.fetch_add(1, Ordering::AcqRel);
        self.precompute(params);
    }

    pub fn clear(&self, params: &SamplerParams) {
        *self.source.lock().unwrap() = fx::Source::default();
        self.set_raw(None, "", params);
    }

    /// The host's output rate (files below 8000 Hz are converted to it).
    pub fn rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate.max(1), Ordering::Relaxed);
    }

    pub fn tempo(&self) -> f64 {
        f64::from_bits(self.tempo.load(Ordering::Relaxed))
    }

    pub fn set_tempo(&self, t: f64) {
        if t > 0.0 && (t - self.tempo()).abs() > 1e-9 {
            self.tempo.store(t.to_bits(), Ordering::Relaxed);
        }
    }

    /// Rebuilds the processed sample from the loaded one and the current settings. Calls that arrive while one runs
    /// only mark it to run again, so a knob drag never queues a backlog.
    pub fn precompute(&self, params: &SamplerParams) {
        if self.busy.swap(true, Ordering::AcqRel) {
            self.again.store(true, Ordering::Release);
            return;
        }
        loop {
            self.again.store(false, Ordering::Release);
            let version = self.raw_version.load(Ordering::Acquire);
            let key = params.fx_key(self.tempo(), self.rate());
            let fresh = self.processed.lock().unwrap().as_ref().is_some_and(|p| p.version == version && p.key == key);
            if !fresh {
                let raw = self.raw.lock().unwrap().clone();
                let source = *self.source.lock().unwrap();
                let built = raw.map(|r| (fx::build(&r, &source, &key), r.frames()));
                params.frames.store(built.as_ref().map_or(0, |(b, _)| b.sample.frames() as u32), Ordering::Relaxed);
                let (sample, looped, file_frames) = match built {
                    Some((b, raw_frames)) => (Some(Arc::new(b.sample)), b.looped, raw_frames as f64 * b.per_file_frame),
                    None => (None, None, 0.0),
                };
                *self.processed.lock().unwrap() = Some(Processed { version, key, sample, looped, file_frames });
                self.processed_version.fetch_add(1, Ordering::AcqRel);
            }
            if !self.again.load(Ordering::Acquire) {
                break;
            }
        }
        self.busy.store(false, Ordering::Release);
        if self.again.swap(false, Ordering::AcqRel) {
            self.precompute(params);
        }
    }

    /// Runs `precompute` on a thread of its own (editor edits while the host isn't processing).
    pub fn precompute_async(self: &Arc<Self>, params: &Arc<SamplerParams>) {
        if self.busy.load(Ordering::Acquire) {
            self.again.store(true, Ordering::Release);
            return;
        }
        let (s, p) = (self.clone(), params.clone());
        std::thread::spawn(move || s.precompute(&p));
    }

    /// True when the processed sample is out of date for the current settings.
    pub fn stale(&self, params: &SamplerParams) -> bool {
        let version = self.raw_version.load(Ordering::Acquire);
        let key = params.fx_key(self.tempo(), self.rate());
        !self.processed.lock().unwrap().as_ref().is_some_and(|p| p.version == version && p.key == key)
    }

    /// Changes whenever the shown sample changes (editor repaint check).
    pub fn version(&self) -> u64 {
        self.processed_version.load(Ordering::Acquire) ^ (self.raw_version.load(Ordering::Acquire) << 32)
    }

    pub fn loaded_path(&self) -> String {
        self.loaded_path.lock().unwrap().clone()
    }

    /// Sounding keys.
    pub fn sounding_keys(&self) -> Vec<i32> {
        let mut out = Vec::new();
        for (w, a) in self.sounding.iter().enumerate() {
            let bits = a.load(Ordering::Relaxed);
            for b in 0..64 {
                if bits & (1 << b) != 0 {
                    out.push((w * 64 + b) as i32);
                }
            }
        }
        out
    }
}

pub enum Task {
    Precompute,
}

pub struct LemonSampler {
    params: Arc<SamplerParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    /// (sample version, settings) last asked for / currently playing.
    requested: Option<(u64, FxKey)>,
    current: Option<(u64, FxKey)>,
    /// Pitch bend in cents (main pitch).
    bend: f64,
}

impl Default for LemonSampler {
    fn default() -> Self {
        LemonSampler {
            params: Arc::new(SamplerParams::default()),
            shared: Arc::new(Shared::default()),
            engine: dsp::Engine::new(44100.0),
            requested: None,
            current: None,
            bend: 0.0,
        }
    }
}

/// An envelope target's LFO (spec §5.2): times from the envelope time law, speed `0.1·2^(8v)` Hz or tempo-synced.
fn lfo(e: &params::EnvParams, tempo: f64) -> dsp::Lfo {
    let n = |v: f32| envelope::knob_seconds(params::time_field(v));
    let sp = f64::from(e.lfo_speed.live());
    let hz = if e.lfo_tempo.live() {
        16.0 * tempo / (60.0 * 2f64.powf(8.0 * (1.0 - sp)))
    } else {
        0.1 * 2f64.powf(8.0 * sp)
    };
    dsp::Lfo {
        shape: e.lfo_shape.live().clamp(0, 2) as u8,
        delay: n(e.lfo_delay.live()),
        attack: n(e.lfo_att.live()),
        amount: f64::from(e.lfo_amt.live()),
        hz,
        global: e.lfo_global.live(),
    }
}

/// The per-block settings of the voices from the parameters.
pub fn block(p: &SamplerParams, tempo: f64, bend: f64) -> dsp::Block {
    let mut targets = [dsp::Target::default(); 5];
    for (i, t) in targets.iter_mut().enumerate() {
        let e = p.env(i);
        // The Panning target has no envelope (spec §3).
        let mut env = if i == dsp::PAN { None } else { e.envelope(128.0) };
        if e.tempo.live() {
            // Tempo-based times: the time law at 120 BPM, scaled by the host tempo (Lemon Studio choice).
            env = env.map(|x| x.scaled(120.0 / tempo.max(1.0)));
        }
        *t = dsp::Target { env, amount: f64::from(e.amt.live()), lfo: lfo(e, tempo) };
    }
    // Resample and the realtime methods stretch in the voices, the other methods in the processed sample; PITCH plays
    // no part in Resample (spec §5.6).
    let mode = p.ts_mode.live();
    let (resample, realtime) = (mode == 0, params::stretch_realtime(mode));
    dsp::Block {
        root: p.root.live(),
        cents: f64::from(p.fine.live()) + if p.main_pitch.live() { bend } else { 0.0 },
        ts_cents: if realtime { params::ts_cents(p.ts_pitch.live()) } else { 0.0 },
        ts_ratio: if resample || realtime { params::mul_of(p.ts_mul.live()) } else { 1.0 },
        ts_time: if resample || realtime { p.ts_seconds(tempo) } else { None },
        realtime,
        targets,
        f_type: p.f_type.live().clamp(0, 7) as u8,
        f_cut: p.f_cut.live(),
        f_res: p.f_res.live(),
        start_offset: f64::from(p.start_offset.live()),
        // The played sample's loop (the file's, or the loop knobs' when they are off their defaults; ping-pong is
        // unfolded into the sample) plays while "Use loop points" is on (spec: audio/sampler.md "The sample file").
        loop_on: p.loop_use.live(),
        declick: p.declick.live(),
        zone: (p.zone_lo.live(), p.zone_hi.live()),
        add_key: p.add_key.live(),
    }
}

/// The voices play a processed sample from now on.
fn take(engine: &mut dsp::Engine, p: &Processed) {
    engine.sample = p.sample.clone();
    engine.looped = p.looped;
    engine.file_frames = p.file_frames;
}

impl LemonSampler {
    /// Takes a newer processed sample when the background task has one ready (never blocks).
    fn pick_up_sample(&mut self, ctx: &mut impl ProcessContext<Self>) {
        let want = (self.shared.raw_version.load(Ordering::Acquire), self.params.fx_key(self.shared.tempo(), self.shared.rate()));
        if self.requested != Some(want) {
            self.requested = Some(want);
            ctx.execute_background(Task::Precompute);
        }
        if let Ok(p) = self.shared.processed.try_lock() {
            if let Some(p) = p.as_ref() {
                if self.current != Some((p.version, p.key)) {
                    self.current = Some((p.version, p.key));
                    take(&mut self.engine, p);
                }
            }
        }
    }

    /// One note event (between the block's renders): `blk` follows a pitch bend.
    fn event(&mut self, ev: NoteEvent<()>, blk: &mut dsp::Block, tempo: f64) {
        match ev {
            NoteEvent::NoteOn { note, velocity, voice_id, .. } => self.engine.note_on_id(note, velocity, voice_id, blk),
            NoteEvent::NoteOff { note, .. } => self.engine.note_off(note, blk),
            NoteEvent::Choke { note, voice_id, .. } => self.engine.choke(voice_id, note, blk),
            // An audio clip's fades and gain, on its own voice.
            NoteEvent::PolyVolume { voice_id, note, gain, .. } => self.engine.volume(voice_id, note, gain),
            NoteEvent::PolyTuning { note, tuning, .. } => self.engine.tune(note, f64::from(tuning)),
            // The note's own pan, also while it sounds (spec: audio/voice-levels.md "Final levels").
            NoteEvent::PolyPan { voice_id, note, pan, .. } => self.engine.pan(voice_id, note, pan),
            NoteEvent::PolyModulation { voice_id, poly_modulation_id, normalized_offset, .. } => match poly_modulation_id {
                params::START_OFFSET_POLY => self.engine.start_later(voice_id, normalized_offset, blk),
                // The note's mod X / mod Y in knob units (the filter knobs' 0..1), added to the knobs for its voice.
                params::MOD_X_POLY => self.engine.modulate(voice_id, Some(normalized_offset), None),
                params::MOD_Y_POLY => self.engine.modulate(voice_id, None, Some(normalized_offset)),
                _ => {}
            },
            NoteEvent::MidiPitchBend { value, .. } => {
                // ±2 semitones (spec §5.1).
                self.bend = f64::from(value * 2.0 - 1.0) * 200.0;
                *blk = block(&self.params, tempo, self.bend);
            }
            NoteEvent::MidiCC { cc: 120 | 123, .. } => self.engine.all_off(),
            _ => {}
        }
    }

    fn publish_sounding(&self) {
        let mut bits = [0u64; 3];
        for k in self.engine.sounding() {
            let k = usize::from(k).min(191);
            bits[k / 64] |= 1 << (k % 64);
        }
        for (a, b) in self.shared.sounding.iter().zip(bits) {
            a.store(b, Ordering::Relaxed);
        }
    }
}

impl Plugin for LemonSampler {
    const NAME: &'static str = "Lemon Sampler";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = Task;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let (params, shared) = (self.params.clone(), self.shared.clone());
        Box::new(move |task| match task {
            Task::Precompute => shared.precompute(&params),
        })
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::SamplerEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.engine.set_rate(config.sample_rate);
        self.shared.set_rate(config.sample_rate.round() as u32);
        // An offline render (an export) gets the resampler's exact weights; live playback the tables.
        self.engine.quality = if config.process_mode == ProcessMode::Offline { os_dsp::resample::Quality::Exact } else { os_dsp::resample::Quality::Table };
        // A restored state names a sample: load it now (initialize may block).
        let path = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
        if !path.is_empty() && path != self.shared.loaded_path() {
            if let Err(e) = self.shared.load(&path, &self.params) {
                nih_log!("Lemon Sampler: {e}");
            }
        } else if path.is_empty() && !self.shared.loaded_path().is_empty() {
            // A restored state without a sample (the host undid a load into an empty channel): nothing plays.
            self.shared.clear(&self.params);
        } else {
            self.shared.precompute(&self.params);
        }
        if let Some(p) = self.shared.processed.lock().unwrap().as_ref() {
            self.current = Some((p.version, p.key));
            self.requested = self.current;
            take(&mut self.engine, p);
        }
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.publish_sounding();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        if let Some(t) = ctx.transport().tempo {
            self.shared.set_tempo(t);
        }
        self.pick_up_sample(ctx);
        let tempo = self.shared.tempo();
        let mut blk = block(&self.params, tempo, self.bend);
        // Notes the editor plays itself.
        if let Ok(mut q) = self.shared.previews.try_lock() {
            for (key, vel, on) in q.drain(..) {
                if on {
                    self.engine.note_on(key, vel, &blk);
                } else {
                    self.engine.note_off(key, &blk);
                }
            }
        }
        let len = buffer.samples();
        let out = buffer.as_slice();
        for ch in out.iter_mut() {
            ch.fill(0.0);
        }
        let mut at = 0;
        while let Some(ev) = ctx.next_event() {
            let t = (ev.timing() as usize).min(len);
            if t > at {
                let (left, right) = out.split_at_mut(1);
                self.engine.render(&mut left[0][at..t], &mut right[0][at..t], &blk);
                at = t;
            }
            self.event(ev, &mut blk, tempo);
        }
        if at < len {
            let (left, right) = out.split_at_mut(1);
            self.engine.render(&mut left[0][at..len], &mut right[0][at..len], &blk);
        }
        // Voice ends go back to the host, at the end of the block (spec: audio/voices.md §4).
        let last = (len as u32).saturating_sub(1);
        for (note, voice_id) in self.engine.take_ended() {
            ctx.send_event(NoteEvent::VoiceTerminated { timing: last, voice_id, channel: 0, note });
        }
        self.publish_sounding();
        // A long sample may be quiet for a while: tell the host to keep processing until the voices end.
        if self.engine.has_voices() { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

// CLAP id and VST3 class id keep the spelling of the plugin's earlier "Open" name: hosts find saved sessions'
// plugins by them (spec: plugins/plugin-formats.md, identity).
impl ClapPlugin for LemonSampler {
    const CLAP_ID: &'static str = "org.openstudio.open-sampler";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's sampler instrument");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Sampler, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonSampler {
    const VST3_CLASS_ID: [u8; 16] = *b"OpenStudioSamplr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Sampler];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonSampler);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonSampler);
os_plugkit::linked_clap_entry!(LemonSampler);

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin at 48 kHz playing a sine of `hz` (half scale), with its parameters' defaults.
    fn plugin(hz: f64) -> LemonSampler {
        let mut p = LemonSampler::default();
        p.engine.set_rate(48000.0);
        let x: Vec<f32> = (0..48000).map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / 48000.0).sin() as f32 * 0.5).collect();
        p.engine.sample = Some(Arc::new(Sample { rate: 48000, left: x.clone(), right: x }));
        p
    }

    fn note_on(id: i32) -> NoteEvent<()> {
        NoteEvent::NoteOn { timing: 0, voice_id: Some(id), channel: 0, note: 60, velocity: 100.0 / 128.0 }
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Plays note `id` with `events` sent after its note on; the level of each side over the last half of 9600 frames.
    fn level(hz: f64, events: &[NoteEvent<()>]) -> (f32, f32) {
        let mut p = plugin(hz);
        let mut blk = block(&p.params, 130.0, 0.0);
        p.event(note_on(3), &mut blk, 130.0);
        for e in events {
            p.event(*e, &mut blk, 130.0);
        }
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        p.engine.render(&mut l, &mut r, &blk);
        (rms(&l[4800..]), rms(&r[4800..]))
    }

    // spec: audio/voice-levels.md "Final levels": the note's pan reaches its voice; with the channel's pan centred the
    // host's split times the voice's makes the note's own split.
    #[test]
    fn a_note_pan_reaches_its_voice() {
        let (l0, r0) = level(1000.0, &[]);
        assert!((l0 - 0.5 / 2f32.sqrt()).abs() < 1e-3 && l0 == r0, "{l0} {r0}");
        let pan = |voice_id, pan| NoteEvent::PolyPan { timing: 0, voice_id, channel: 0, note: 60, pan };
        let (l, r) = level(1000.0, &[pan(Some(3), -1.0)]);
        assert!((l - l0 * 2f32.sqrt()).abs() < 1e-4 && r < 1e-6, "{l} {r}");
        let (l, r) = level(1000.0, &[pan(Some(3), 0.5)]);
        let a = 1.5 * std::f32::consts::PI / 4.0;
        assert!((l * std::f32::consts::FRAC_1_SQRT_2 - l0 * a.cos()).abs() < 1e-4, "{l}");
        assert!((r * std::f32::consts::FRAC_1_SQRT_2 - r0 * a.sin()).abs() < 1e-4, "{r}");
        // Another note's pan leaves it alone; a pan by key reaches it.
        assert_eq!(level(1000.0, &[pan(Some(4), -1.0)]), (l0, r0));
        assert!(level(1000.0, &[pan(None, 1.0)]).0 < 1e-6);
    }

    // spec: audio/voice-levels.md "Final levels", audio/channel-filter.md: the note's mod X / mod Y add to the filter
    // knobs for its voice.
    #[test]
    fn a_note_filter_offset_reaches_its_voice() {
        let modulate = |voice_id, poly_modulation_id, normalized_offset| NoteEvent::PolyModulation {
            timing: 0,
            voice_id,
            poly_modulation_id,
            normalized_offset,
        };
        let open = level(1000.0, &[]).0;
        let dull = level(1000.0, &[modulate(3, params::MOD_X_POLY, -0.8)]).0;
        assert!(dull < open * 0.1, "{open} {dull}");
        assert_eq!(level(1000.0, &[modulate(4, params::MOD_X_POLY, -0.8)]).0, open);
        // Resonance on a closed-down fast low-pass changes the sound.
        let half = level(1000.0, &[modulate(3, params::MOD_X_POLY, -0.5)]).0;
        let res = level(1000.0, &[modulate(3, params::MOD_X_POLY, -0.5), modulate(3, params::MOD_Y_POLY, 0.9)]).0;
        assert!((res - half).abs() > 1e-3, "{half} {res}");
    }

    // A sounding voice follows a new pan and a new offset.
    #[test]
    fn a_sounding_voice_follows_new_note_levels() {
        let mut p = plugin(1000.0);
        let mut blk = block(&p.params, 130.0, 0.0);
        p.event(note_on(3), &mut blk, 130.0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        p.engine.render(&mut l, &mut r, &blk);
        assert!(rms(&r) > 0.3);
        p.event(NoteEvent::PolyPan { timing: 0, voice_id: Some(3), channel: 0, note: 60, pan: 1.0 }, &mut blk, 130.0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        p.engine.render(&mut l, &mut r, &blk);
        // The pan ramps over one control span.
        assert!(rms(&l[64..]) < 1e-6 && rms(&r[64..]) > 0.45, "{} {}", rms(&l[64..]), rms(&r[64..]));
        p.event(NoteEvent::PolyModulation { timing: 0, voice_id: 3, poly_modulation_id: params::MOD_X_POLY, normalized_offset: -0.9 }, &mut blk, 130.0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        p.engine.render(&mut l, &mut r, &blk);
        assert!(rms(&r[2400..]) < 0.05, "{}", rms(&r[2400..]));
    }

    /// A WAV file: `channels`, 16-bit, `frames` of silence, then an `smpl` chunk with one loop when given.
    fn wav(channels: u16, frames: u32, lp: Option<(u32, u32)>) -> Vec<u8> {
        let data = frames * u32::from(channels) * 2;
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, channels] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        for v in [44100u32, 44100 * 2 * u32::from(channels)] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        for v in [2 * channels, 16] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        w.resize(w.len() + data as usize, 0);
        if let Some((a, b)) = lp {
            w.extend_from_slice(b"smpl");
            w.extend_from_slice(&60u32.to_le_bytes());
            let mut d = [0u32; 15];
            d[7] = 1;
            (d[11], d[12]) = (a, b);
            for v in d {
                w.extend_from_slice(&v.to_le_bytes());
            }
        }
        let len = (w.len() - 8) as u32;
        w[4..8].copy_from_slice(&len.to_le_bytes());
        w
    }

    // spec: audio/sampler.md "The sample file": the loop is the first loop of the `smpl` chunk, wherever it sits.
    #[test]
    fn the_file_loop_and_format_are_read() {
        let dir = std::env::temp_dir().join(format!("lemon-sampler-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.wav");
        std::fs::write(&p, wav(1, 100_001, Some((5, 90_000)))).unwrap();
        assert_eq!(source_info(&p), (16, fx::Source { int16_mono: true, file_loop: Some((5, 90_000)) }));
        std::fs::write(&p, wav(2, 1000, None)).unwrap();
        assert_eq!(source_info(&p), (16, fx::Source { int16_mono: false, file_loop: None }));
        assert_eq!(source_bits(&p), 16);
        std::fs::remove_dir_all(&dir).ok();
    }
}
