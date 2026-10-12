//! Open Sampler: Lemon Studio's sampler instrument as a VST3 / CLAP plugin (nih-plug).
//!
// spec: plugins/open-sampler.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod editor;
pub mod params;
pub mod view;

use std::num::NonZeroU32;
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
    // The header is enough: a long sample is not read twice.
    let mut data = Vec::new();
    let Ok(_) = std::fs::File::open(path).and_then(|f| std::io::Read::read_to_end(&mut std::io::Read::take(f, 1 << 16), &mut data)) else {
        return 16;
    };
    if data.len() > 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        let mut i = 12;
        while i + 8 <= data.len() {
            let id = &data[i..i + 4];
            let len = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
            if id == b"fmt " && i + 24 <= data.len() {
                let fmt = u16::from_le_bytes([data[i + 8], data[i + 9]]);
                let bits = u32::from(u16::from_le_bytes([data[i + 22], data[i + 23]]));
                return bits | if fmt == 3 { 0x100 } else { 0 };
            }
            i += 8 + len + (len & 1);
        }
    } else if data.len() > 22 && &data[0..4] == b"fLaC" {
        // STREAMINFO: bits per sample − 1 sits in 5 bits across bytes 20 and 21 of the file.
        return u32::from(((data[20] & 1) << 4) | (data[21] >> 4)) + 1;
    } else if data.len() > 4 && &data[0..4] == b"OggS" {
        return 32 | 0x100;
    }
    16
}

impl Shared {
    /// Loads a sample file and makes it the current one (UI / initialize thread). A decode the host already holds
    /// (it decodes a project's samples ahead, spec: ui/project-loading.md) is taken as it is.
    pub fn load(&self, path: &str, params: &SamplerParams) -> Result<(), String> {
        let s = os_engine::sample::load_shared(Path::new(path)).map_err(|e| e.to_string())?;
        self.bits.store(source_bits(Path::new(path)), Ordering::Release);
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
        self.set_raw(None, "", params);
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
            let key = params.fx_key(self.tempo());
            let fresh = self.processed.lock().unwrap().as_ref().is_some_and(|p| p.version == version && p.key == key);
            if !fresh {
                let raw = self.raw.lock().unwrap().clone();
                let sample = raw.map(|r| Arc::new(dsp::precompute(&r, &key)));
                params.frames.store(sample.as_ref().map_or(0, |s| s.frames() as u32), Ordering::Relaxed);
                *self.processed.lock().unwrap() = Some(Processed { version, key, sample });
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
        let key = params.fx_key(self.tempo());
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

pub struct OpenSampler {
    params: Arc<SamplerParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    /// (sample version, settings) last asked for / currently playing.
    requested: Option<(u64, FxKey)>,
    current: Option<(u64, FxKey)>,
    /// Pitch bend in cents (main pitch).
    bend: f64,
}

impl Default for OpenSampler {
    fn default() -> Self {
        OpenSampler {
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
    let resample = p.ts_mode.live() == 0;
    dsp::Block {
        root: p.root.live(),
        cents: f64::from(p.fine.live()) + if p.main_pitch.live() { bend } else { 0.0 },
        ts_cents: if resample { params::ts_cents(p.ts_pitch.live()) } else { 0.0 },
        ts_ratio: if resample { params::mul_of(p.ts_mul.live()) } else { 1.0 },
        ts_time: if resample { p.ts_seconds(tempo) } else { None },
        targets,
        f_type: p.f_type.live().clamp(0, 7) as u8,
        f_cut: f64::from(p.f_cut.live()),
        f_res: f64::from(p.f_res.live()),
        start_offset: f64::from(p.start_offset.live()),
        loop_on: p.loop_active(),
        loop_start: f64::from(p.loop_start.live()),
        loop_end: f64::from(p.loop_end.live()),
        pingpong: p.loop_pingpong.live(),
        declick: p.declick.live(),
        zone: (p.zone_lo.live(), p.zone_hi.live()),
        add_key: p.add_key.live(),
    }
}

impl OpenSampler {
    /// Takes a newer processed sample when the background task has one ready (never blocks).
    fn pick_up_sample(&mut self, ctx: &mut impl ProcessContext<Self>) {
        let want = (self.shared.raw_version.load(Ordering::Acquire), self.params.fx_key(self.shared.tempo()));
        if self.requested != Some(want) {
            self.requested = Some(want);
            ctx.execute_background(Task::Precompute);
        }
        if let Ok(p) = self.shared.processed.try_lock() {
            if let Some(p) = p.as_ref() {
                if self.current != Some((p.version, p.key)) {
                    self.current = Some((p.version, p.key));
                    self.engine.sample = p.sample.clone();
                }
            }
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

impl Plugin for OpenSampler {
    const NAME: &'static str = "Open Sampler";
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
        // A restored state names a sample: load it now (initialize may block).
        let path = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
        if !path.is_empty() && path != self.shared.loaded_path() {
            if let Err(e) = self.shared.load(&path, &self.params) {
                nih_log!("Open Sampler: {e}");
            }
        } else {
            self.shared.precompute(&self.params);
        }
        if let Some(p) = self.shared.processed.lock().unwrap().as_ref() {
            self.current = Some((p.version, p.key));
            self.requested = self.current;
            self.engine.sample = p.sample.clone();
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
            match ev {
                NoteEvent::NoteOn { note, velocity, voice_id, .. } => self.engine.note_on_id(note, velocity, voice_id, &blk),
                NoteEvent::NoteOff { note, .. } => self.engine.note_off(note, &blk),
                NoteEvent::Choke { note, .. } => self.engine.choke(note, &blk),
                NoteEvent::PolyTuning { note, tuning, .. } => self.engine.tune(note, f64::from(tuning)),
                NoteEvent::PolyModulation { voice_id, poly_modulation_id: params::START_OFFSET_POLY, normalized_offset, .. } => {
                    self.engine.start_later(voice_id, normalized_offset, &blk)
                }
                NoteEvent::MidiPitchBend { value, .. } => {
                    // ±2 semitones (spec §5.1).
                    self.bend = f64::from(value * 2.0 - 1.0) * 200.0;
                    blk = block(&self.params, tempo, self.bend);
                }
                NoteEvent::MidiCC { cc: 120 | 123, .. } => self.engine.all_off(),
                _ => {}
            }
        }
        if at < len {
            let (left, right) = out.split_at_mut(1);
            self.engine.render(&mut left[0][at..len], &mut right[0][at..len], &blk);
        }
        self.publish_sounding();
        // A long sample may be quiet for a while: tell the host to keep processing until the voices end.
        if self.engine.has_voices() { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

impl ClapPlugin for OpenSampler {
    const CLAP_ID: &'static str = "org.openstudio.open-sampler";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's sampler instrument");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Sampler, ClapFeature::Stereo];
}

impl Vst3Plugin for OpenSampler {
    const VST3_CLASS_ID: [u8; 16] = *b"OpenStudioSamplr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Sampler];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Open plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(OpenSampler);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(OpenSampler);
os_plugkit::linked_clap_entry!(OpenSampler);
