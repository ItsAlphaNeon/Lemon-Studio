// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Drum Sampler: Lemon Studio's recreation of FPC as a VST3 / CLAP plugin (nih-plug). Sixteen pads in two
//! banks, each on its own key, each playing one or more velocity layers of samples; it keeps the original's saved
//! state, so a project's FPC channel opens with its pads and samples and saves back unchanged.
//!
// spec: plugins/lemon-drum-sampler.md, plugins/fpc.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod editor;
pub mod params;
pub mod state;
pub mod view;

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use os_engine::sample::Sample;

use params::DrumParams;
use state::State;

/// What the audio thread and the editor share.
#[derive(Default)]
pub struct Shared {
    /// Notes the editor plays itself (key, velocity).
    pub previews: Mutex<Vec<(u8, f32)>>,
    /// What the voices play; `kit_version` moves on every change.
    pub kit: Mutex<Option<Arc<dsp::Kit>>>,
    pub kit_version: AtomicU64,
    /// Samples opened, by file (also the ones that could not be read: `None`).
    cache: Mutex<HashMap<String, Option<Arc<Sample>>>>,
    /// Set while an editor is open.
    pub editor_open: AtomicBool,
}

impl Shared {
    /// The file layer `n` (state order) of stored path `stored` opens: the host's, else the stored one with FL's
    /// placeholders made real where the factory data is found.
    pub fn file_for(params: &DrumParams, n: usize, stored: &str) -> String {
        let given = params.given_path(n);
        if !given.is_empty() {
            return given;
        }
        os_engine::SampleLibrary::new(os_engine::SampleLibrary::default_factory_dir())
            .resolve(stored)
            .map_or_else(|| stored.to_string(), |p| p.to_string_lossy().into_owned())
    }

    /// Opens `file` once (kept for the next rebuild).
    fn sample(&self, file: &str) -> Option<Arc<Sample>> {
        if let Some(s) = self.cache.lock().ok().and_then(|c| c.get(file).cloned()) {
            return s;
        }
        let s = os_engine::sample::load_shared(Path::new(file)).ok();
        if let Ok(mut c) = self.cache.lock() {
            c.insert(file.to_string(), s.clone());
        }
        s
    }

    /// Rebuilds what the voices play from the saved state, opening the samples it names.
    pub fn rebuild(&self, params: &DrumParams) {
        let st = params.state();
        let mut n = 0;
        let kit = dsp::Kit::new(&st, &mut |_, _, stored| {
            let file = Self::file_for(params, n, stored);
            n += 1;
            self.sample(&file)
        });
        if let Ok(mut k) = self.kit.lock() {
            *k = Some(Arc::new(kit));
        }
        self.kit_version.fetch_add(1, Ordering::AcqRel);
    }

    /// Whether pad `i`'s layer `j` has a sample that opened.
    pub fn loaded(&self, i: usize, j: usize) -> bool {
        self.kit.lock().ok().and_then(|k| k.as_ref().and_then(|k| k.pads.get(i)).and_then(|p| p.layers.get(j)).map(|l| l.sample.is_some())).unwrap_or(false)
    }

    /// The first sample pad `i` plays (for its picture).
    pub fn pad_sample(&self, i: usize) -> Option<Arc<Sample>> {
        self.kit.lock().ok()?.as_ref()?.pads.get(i)?.layers.iter().find_map(|l| l.sample.clone())
    }

    /// Replaces the saved state and plays it. The files given for the layers stay with the layers they were given
    /// for only while the layers keep their stored paths.
    pub fn set_state(&self, params: &DrumParams, st: &State) {
        let old = params::stored_paths(&params.raw_copy());
        let new = st.pads().into_iter().flat_map(|p| p.layers.into_iter().map(|l| l.path)).collect::<Vec<_>>();
        if old != new
            && let Ok(mut given) = params.paths.write()
        {
            // Keep a given file for a stored path that is still there; layers moved: look them up by path.
            let by_path: HashMap<String, String> = old.iter().cloned().zip(given.iter().cloned()).filter(|(_, g)| !g.is_empty()).collect();
            *given = new.iter().map(|p| by_path.get(p).cloned().unwrap_or_default()).collect();
        }
        params.set_raw(st.to_bytes());
        self.rebuild(params);
    }

    /// Puts sample file `path` on pad `i` (the editor's load, a file dropped on a pad).
    pub fn load(&self, params: &DrumParams, i: usize, path: &str) -> Result<(), String> {
        os_engine::sample::load_shared(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
        if let Ok(mut c) = self.cache.lock() {
            c.remove(path);
        }
        let mut st = params.state();
        st.set_sample(i, path);
        self.set_state(params, &st);
        Ok(())
    }
}

pub struct LemonDrumSampler {
    params: Arc<DrumParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    kit_seen: u64,
}

impl Default for LemonDrumSampler {
    fn default() -> Self {
        LemonDrumSampler { params: Arc::new(DrumParams::default()), shared: Arc::new(Shared::default()), engine: dsp::Engine::new(44100.0), kit_seen: u64::MAX }
    }
}

impl LemonDrumSampler {
    /// The voices take the kit last built (never waits: a busy lock is tried again next block).
    fn take_kit(&mut self) {
        let v = self.shared.kit_version.load(Ordering::Acquire);
        if v != self.kit_seen
            && let Ok(k) = self.shared.kit.try_lock()
        {
            let kit = k.clone();
            drop(k);
            self.engine.set_kit(kit);
            self.kit_seen = v;
        }
    }
}

impl Plugin for LemonDrumSampler {
    const NAME: &'static str = "Lemon Drum Sampler";
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
        Some(Box::new(editor::DrumEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.engine.set_rate(config.sample_rate);
        // A restored state names its samples: open them now (initialize may block).
        self.shared.rebuild(&self.params);
        self.take_kit();
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.take_kit();
        if let Ok(mut q) = self.shared.previews.try_lock() {
            for (key, vel) in q.drain(..) {
                self.engine.note_on(key, vel, None);
            }
        }
        // The editor's previews end unreported.
        self.engine.take_ended();
        let len = buffer.samples();
        let out = buffer.as_slice();
        for ch in out.iter_mut() {
            ch.fill(0.0);
        }
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let mut at = 0;
        let mut ended = Vec::new();
        while let Some(ev) = ctx.next_event() {
            let t = (ev.timing() as usize).min(len);
            if t > at {
                let (left, right) = out.split_at_mut(1);
                self.engine.render(&mut left[0][at..t], &mut right[0][at..t]);
                at = t;
            }
            match ev {
                NoteEvent::NoteOn { note, velocity, voice_id, .. } => self.engine.note_on(note, velocity, voice_id),
                NoteEvent::Choke { .. } | NoteEvent::MidiCC { cc: 120 | 123, .. } => self.engine.all_off(),
                _ => {}
            }
            ended.extend(self.engine.take_ended());
        }
        if at < len {
            let (left, right) = out.split_at_mut(1);
            self.engine.render(&mut left[0][at..len], &mut right[0][at..len]);
        }
        ended.extend(self.engine.take_ended());
        // Voice ends go back to the host, at the end of the block (spec: audio/voices.md §4).
        let last = (len as u32).saturating_sub(1);
        for (note, voice_id) in ended {
            ctx.send_event(NoteEvent::VoiceTerminated { timing: last, voice_id, channel: 0, note });
        }
        if self.engine.has_voices() { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

impl ClapPlugin for LemonDrumSampler {
    const CLAP_ID: &'static str = "org.openstudio.lemon-drum-sampler";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's pad drum sampler");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Drum, ClapFeature::Sampler, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonDrumSampler {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioDrums";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Drum, Vst3SubCategory::Sampler];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonDrumSampler);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonDrumSampler);
os_plugkit::linked_clap_entry!(LemonDrumSampler);

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A short 44.1 kHz stereo 16-bit WAV: a tone at `amp` (0..1).
    pub(crate) fn wav(name: &str, amp: f32) -> std::path::PathBuf {
        let frames = 4410usize;
        let mut data = Vec::with_capacity(frames * 4);
        for i in 0..frames {
            let v = ((i as f32 * 0.07).sin() * amp * 32767.0) as i16;
            data.extend(v.to_le_bytes());
            data.extend(v.to_le_bytes());
        }
        let mut fmt = Vec::new();
        for v in [1u16, 2] {
            fmt.extend(v.to_le_bytes());
        }
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
        let mut file = b"RIFF".to_vec();
        file.extend((body.len() as u32).to_le_bytes());
        file.extend(body);
        let p = std::env::temp_dir().join(format!("lemon-drums-{}-{name}.wav", std::process::id()));
        std::fs::write(&p, file).unwrap();
        p
    }

    #[test]
    fn a_loaded_pad_plays_on_its_key() {
        let p = wav("kick", 0.5);
        let params = DrumParams::default();
        let sh = Shared::default();
        sh.load(&params, 1, &p.to_string_lossy()).unwrap();
        let st = params.state();
        assert_eq!(st.pad(1).unwrap().layers[0].path, p.to_string_lossy());
        assert!(sh.loaded(1, 0));
        let kit = sh.kit.lock().unwrap().clone().unwrap();
        assert_eq!(kit.pads_on(36), [1]);
        assert!(kit.pads_on(37).is_empty());
        let mut e = dsp::Engine::new(44100.0);
        e.set_kit(Some(kit));
        e.note_on(36, 100.0 / 127.0, None);
        let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
        e.render(&mut l, &mut r);
        assert!(l.iter().any(|v| v.abs() > 0.2));
        assert!(sh.load(&params, 2, "Z:\\nowhere.wav").is_err());
        let _ = std::fs::remove_file(p);
    }

    /// The host's files for the layers follow their layers through edits.
    #[test]
    fn given_files_follow_their_layers() {
        let params = DrumParams::default();
        let mut st = State::default();
        st.set_sample(3, "%X%\\tamb.wav");
        params.set_raw(st.to_bytes());
        let n = params::stored_paths(&params.raw_copy()).iter().position(|p| p == "%X%\\tamb.wav").unwrap();
        let mut given = vec![String::new(); 32];
        given[n] = "C:\\real\\tamb.wav".into();
        *params.paths.write().unwrap() = given;
        let sh = Shared::default();
        let mut st = params.state();
        st.set_sample(0, "C:\\new.wav");
        sh.set_state(&params, &st);
        let m = params::stored_paths(&params.raw_copy()).iter().position(|p| p == "%X%\\tamb.wav").unwrap();
        assert_eq!(params.given_path(m), "C:\\real\\tamb.wav");
        assert_eq!(params.given_path(0), "");
    }
}
