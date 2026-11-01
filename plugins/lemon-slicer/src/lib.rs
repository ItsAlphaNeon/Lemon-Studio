// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Slicer: Lemon Studio's recreation of Fruity Slicer as a VST3 / CLAP plugin (nih-plug). A loop cut into
//! slices; slice n plays on key C5 + n; the editor dumps the slices to the piano roll (host link).
//!
// spec: plugins/lemon-slicer.md, plugins/fruity-slicer.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod dump;
pub mod editor;
pub mod params;
pub mod slicing;
pub mod state;
pub mod view;

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use os_engine::sample::Sample;

use params::SlicerParams;
use state::{State, H_BEATS, H_BPM};

/// What the audio thread and the editor share.
#[derive(Default)]
pub struct Shared {
    /// Notes the editor plays itself (key, velocity, on).
    pub previews: Mutex<Vec<(u8, f32, bool)>>,
    /// What the voices play; `kit_version` moves on every change.
    pub kit: Mutex<Option<Arc<dsp::Kit>>>,
    pub kit_version: AtomicU64,
    /// The sample loaded and the file it came from (also kept when it could not be read).
    sample: Mutex<Option<Arc<Sample>>>,
    loaded_path: Mutex<String>,
    /// The project's tempo as the host last gave it (f64 bits; 0 = not known).
    pub tempo: AtomicU64,
    /// Set while an editor is open.
    pub editor_open: AtomicBool,
}

impl Shared {
    pub fn sample(&self) -> Option<Arc<Sample>> {
        self.sample.lock().ok().and_then(|s| s.clone())
    }

    pub fn loaded_path(&self) -> String {
        self.loaded_path.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// The host's tempo, if it gave one.
    pub fn tempo(&self) -> Option<f64> {
        Some(f64::from_bits(self.tempo.load(Ordering::Relaxed))).filter(|t| *t > 0.0)
    }

    /// Rebuilds what the voices play from the sample and the saved state's slices.
    pub fn rebuild(&self, params: &SlicerParams) {
        let kit = self.sample().map(|s| Arc::new(dsp::Kit::new(s, &params.state())));
        if let Ok(mut k) = self.kit.lock() {
            *k = kit;
        }
        self.kit_version.fetch_add(1, Ordering::AcqRel);
    }

    /// Replaces the saved state (slices, tempo, ...) and plays it.
    pub fn set_state(&self, params: &SlicerParams, st: &State) {
        params.set_raw(st.to_bytes());
        self.rebuild(params);
    }

    /// Opens sample file `path` for the state as it is (a restored state's sample); `path` stays named when it cannot
    /// be read, and the slicer plays silence.
    pub fn open(&self, path: &str, params: &SlicerParams) -> Result<(), String> {
        let s = if path.is_empty() { Ok(None) } else { os_engine::sample::load_shared(Path::new(path)).map(Some).map_err(|e| format!("{path}: {e}")) };
        if let Ok(mut l) = self.loaded_path.lock() {
            *l = path.to_string();
        }
        let ok = s.is_ok();
        if let Ok(mut cur) = self.sample.lock() {
            *cur = s.clone().unwrap_or(None);
        }
        self.rebuild(params);
        if ok { Ok(()) } else { Err(s.err().unwrap_or_default()) }
    }

    /// Loads a new loop from file `path` (the editor's Load): its beats and tempo from its `acid` chunk (else guessed),
    /// its own slices when it has cue points, else Medium auto slicing (spec: plugins/lemon-slicer.md §3). The
    /// settings stay.
    pub fn load(&self, path: &str, params: &SlicerParams) -> Result<(), String> {
        let (st, s) = file_state(path, params.state())?;
        if let Ok(mut l) = self.loaded_path.lock() {
            *l = path.to_string();
        }
        if let Ok(mut cur) = self.sample.lock() {
            *cur = Some(s);
        }
        if let Ok(mut p) = params.sample_path.write() {
            *p = path.to_string();
        }
        self.set_state(params, &st);
        Ok(())
    }
}

/// State `st` with loop file `path` loaded: its beats and tempo from its `acid` chunk (else guessed), its own slices
/// when it has cue points, else Medium auto slicing (spec: plugins/lemon-slicer.md §3); the settings stay. Also the
/// sample read. The editor's Load, and a new channel for a file sent from Tesla (spec: plugins/tesla/README.md
/// "Sending slices to Lemon Slicer").
pub fn file_state(path: &str, mut st: State) -> Result<(State, Arc<Sample>), String> {
    let s = os_engine::sample::load_shared(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    let info = std::fs::read(path).map(|b| slicing::wav_info(&b)).unwrap_or_default();
    st.set_path(path);
    let (gb, gt) = slicing::guess_tempo(s.frames(), s.rate);
    let (beats, bpm) = match (info.beats, info.bpm) {
        (Some(b), Some(t)) => (b, t),
        (Some(b), None) => (b, (f64::from(b) * 60.0 / (s.frames() as f64 / f64::from(s.rate.max(1)))) as f32),
        _ => (gb, gt),
    };
    st.set_f32(H_BEATS, beats);
    st.set_f32(H_BPM, bpm);
    st.slices = slicing::builtin(&info, s.frames());
    if st.slices.is_empty() {
        st.slices = slicing::auto(&s.left, &s.right, s.rate, slicing::AUTO_DB[1]);
    }
    Ok((st, s))
}

/// The file a state's sample is opened from: the host's resolved path, else the stored one (FL's placeholders made
/// real where the factory data is found).
pub fn sample_file(params: &SlicerParams) -> String {
    let given = params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
    if !given.is_empty() {
        return given;
    }
    let stored = params.state().path();
    if stored.is_empty() {
        return stored;
    }
    os_engine::SampleLibrary::new(os_engine::SampleLibrary::default_factory_dir())
        .resolve(&stored)
        .map_or(stored, |p| p.to_string_lossy().into_owned())
}

pub struct LemonSlicer {
    params: Arc<SlicerParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    kit: Option<Arc<dsp::Kit>>,
    kit_seen: u64,
}

impl Default for LemonSlicer {
    fn default() -> Self {
        LemonSlicer { params: Arc::new(SlicerParams::default()), shared: Arc::new(Shared::default()), engine: dsp::Engine::new(44100.0), kit: None, kit_seen: u64::MAX }
    }
}

impl LemonSlicer {
    /// The voices take the kit last built (never waits: a busy lock is tried again next block).
    fn take_kit(&mut self) {
        let v = self.shared.kit_version.load(Ordering::Acquire);
        if v != self.kit_seen
            && let Ok(k) = self.shared.kit.try_lock()
        {
            self.kit = k.clone();
            self.kit_seen = v;
        }
    }

    fn block(&self) -> dsp::Block {
        let v = self.params.values();
        dsp::Block { cents: f64::from(v.pitch), attack_ms: f64::from(v.attack), decay_ms: f64::from(v.decay), declick: v.declick, to_end: v.to_end }
    }
}

impl Plugin for LemonSlicer {
    const NAME: &'static str = "Lemon Slicer";
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
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::SlicerEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.engine.set_rate(config.sample_rate);
        // A restored state names its sample: open it now (initialize may block); the slices may have changed too.
        let path = sample_file(&self.params);
        if path != self.shared.loaded_path() {
            if let Err(e) = self.shared.open(&path, &self.params) {
                nih_log!("Lemon Slicer: {e}");
            }
        } else {
            self.shared.rebuild(&self.params);
        }
        self.take_kit();
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        if let Some(t) = ctx.transport().tempo {
            self.shared.tempo.store(t.to_bits(), Ordering::Relaxed);
        }
        self.take_kit();
        let blk = self.block();
        let kit = self.kit.clone();
        if let Ok(mut q) = self.shared.previews.try_lock() {
            for (key, vel, on) in q.drain(..) {
                if on {
                    self.engine.note_on(kit.as_deref(), key, vel, None);
                } else {
                    self.engine.note_off(key, None, &blk);
                }
            }
        }
        let len = buffer.samples();
        let out = buffer.as_slice();
        for ch in out.iter_mut() {
            ch.fill(0.0);
        }
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let mut at = 0;
        while let Some(ev) = ctx.next_event() {
            let t = (ev.timing() as usize).min(len);
            if t > at {
                let (left, right) = out.split_at_mut(1);
                self.engine.render(kit.as_deref(), &mut left[0][at..t], &mut right[0][at..t], &blk);
                at = t;
            }
            match ev {
                NoteEvent::NoteOn { note, velocity, voice_id, .. } => self.engine.note_on(kit.as_deref(), note, velocity, voice_id),
                NoteEvent::NoteOff { note, voice_id, .. } => self.engine.note_off(note, voice_id, &blk),
                NoteEvent::Choke { .. } | NoteEvent::MidiCC { cc: 120 | 123, .. } => self.engine.all_off(),
                _ => {}
            }
        }
        if at < len {
            let (left, right) = out.split_at_mut(1);
            self.engine.render(kit.as_deref(), &mut left[0][at..len], &mut right[0][at..len], &blk);
        }
        // Voice ends go back to the host, at the end of the block (spec: audio/voices.md §4).
        let last = (len as u32).saturating_sub(1);
        for (note, voice_id) in self.engine.take_ended() {
            ctx.send_event(NoteEvent::VoiceTerminated { timing: last, voice_id, channel: 0, note });
        }
        if self.engine.has_voices() { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

impl ClapPlugin for LemonSlicer {
    const CLAP_ID: &'static str = "org.openstudio.lemon-slicer";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's loop slicer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Sampler, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonSlicer {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioSlicr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Sampler];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonSlicer);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonSlicer);
os_plugkit::linked_clap_entry!(LemonSlicer);

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A loop file: 2 s of 44.1 kHz stereo 16-bit clicks every half second, with `extra` chunks.
    pub(crate) fn loop_file(name: &str, extra: &[Vec<u8>]) -> std::path::PathBuf {
        let frames = 88200usize;
        let mut data = Vec::with_capacity(frames * 4);
        for i in 0..frames {
            let v: i16 = if i % 22050 < 200 { if i % 2 == 0 { 20000 } else { -20000 } } else { 0 };
            data.extend(v.to_le_bytes());
            data.extend(v.to_le_bytes());
        }
        let mut fmt = Vec::new();
        fmt.extend(1u16.to_le_bytes());
        fmt.extend(2u16.to_le_bytes());
        fmt.extend(44100u32.to_le_bytes());
        fmt.extend((44100u32 * 4).to_le_bytes());
        fmt.extend(4u16.to_le_bytes());
        fmt.extend(16u16.to_le_bytes());
        let mut body = b"WAVE".to_vec();
        for (id, b) in [(b"fmt ", &fmt), (b"data", &data)] {
            body.extend(id);
            body.extend((b.len() as u32).to_le_bytes());
            body.extend(b);
        }
        for e in extra {
            body.extend(e);
        }
        let mut file = b"RIFF".to_vec();
        file.extend((body.len() as u32).to_le_bytes());
        file.extend(body);
        let p = std::env::temp_dir().join(format!("lemon-slicer-{}-{name}.wav", std::process::id()));
        std::fs::write(&p, file).unwrap();
        p
    }

    #[test]
    fn loading_a_loop_slices_it_and_keys_play_the_slices() {
        let p = loop_file("hits", &[]);
        let params = SlicerParams::default();
        let sh = Shared::default();
        sh.load(&p.to_string_lossy(), &params).unwrap();
        let st = params.state();
        // No acid chunk: 2 s taken as 4 beats at 120 BPM; Medium auto slicing finds the four clicks.
        assert_eq!((st.beats(), st.bpm()), (4.0, 120.0));
        let starts: Vec<u32> = st.slices.iter().map(|s| s.start).collect();
        assert_eq!(starts, [0, 22050, 44100, 66150]);
        assert_eq!(st.path(), p.to_string_lossy());
        assert_eq!(*params.sample_path.read().unwrap(), p.to_string_lossy());
        let kit = sh.kit.lock().unwrap().clone().unwrap();
        assert_eq!(kit.keys[63], Some(3));
        let mut e = dsp::Engine::new(44100.0);
        e.note_on(Some(&kit), 61, 1.0, None);
        let (mut l, mut r) = (vec![0.0; 300], vec![0.0; 300]);
        e.render(Some(&kit), &mut l, &mut r, &dsp::Block::default());
        assert!(l.iter().any(|v| v.abs() > 0.3));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_files_markers_and_tempo_win() {
        let mut extra = slicing::tests_support::cue_chunks(&[(0, "Kick"), (44100, "Snare")]);
        extra.push(slicing::tests_support::acid(8, 240.0));
        let p = loop_file("markers", &extra);
        let params = SlicerParams::default();
        let sh = Shared::default();
        sh.load(&p.to_string_lossy(), &params).unwrap();
        let st = params.state();
        assert_eq!((st.beats(), st.bpm()), (8.0, 240.0));
        assert_eq!(st.slices.iter().map(|s| s.name()).collect::<Vec<_>>(), ["Kick", "Snare"]);
        // A restored state opens its sample and keeps its slices.
        let q = SlicerParams::default();
        q.set_raw(params.raw_copy());
        let sh2 = Shared::default();
        sh2.open(&sample_file(&q), &q).unwrap();
        assert_eq!(sh2.kit.lock().unwrap().as_ref().unwrap().spans.len(), 2);
        assert!(sh2.open("Z:\\nowhere.wav", &q).is_err());
        assert!(sh2.kit.lock().unwrap().is_none());
        let _ = std::fs::remove_file(p);
    }
}
