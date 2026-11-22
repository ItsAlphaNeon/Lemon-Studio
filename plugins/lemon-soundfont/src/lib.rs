// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon SoundFont Player: Lemon Studio's SoundFont 2 player as a VST3 / CLAP plugin (nih-plug). It plays one preset
//! of a `.sf2` file, in one of two playback modes (Classic or Modern), and keeps the original player's saved state, so
//! a project's player channel opens with its font and preset and saves back unchanged.
//!
// spec: plugins/lemon-soundfont.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod effects;
#[cfg(feature = "editor")]
pub mod editor;
pub mod params;
pub mod profile;
pub mod sf2;
pub mod state;
pub mod tables;
pub mod view;

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;

use dsp::Patch;
use params::FontParams;
use profile::Mode;
use sf2::Font;

/// What the audio thread and the editor share.
#[derive(Default)]
pub struct Shared {
    /// Notes the editor plays itself (key, velocity, on).
    pub previews: Mutex<Vec<(u8, f32, bool)>>,
    /// What the voices play; `patch_version` moves on every change.
    pub patch: Mutex<Option<Arc<Patch>>>,
    pub patch_version: AtomicU64,
    /// The mode the voices play in (0 classic, 1 modern).
    pub mode: std::sync::atomic::AtomicU8,
    /// Fonts opened, by file (also the ones that could not be read: the error).
    cache: Mutex<HashMap<String, Result<Arc<Font>, String>>>,
    /// Why the font could not be opened ("" when it opened or none is set).
    pub error: Mutex<String>,
    pub editor_open: AtomicBool,
}

/// Every font open in this process, by file: the players that name the same file share one copy of it (a project
/// with many channels on one large font would otherwise hold it many times over). Kept while some player uses it.
static FONTS: Mutex<std::collections::BTreeMap<String, std::sync::Weak<Font>>> = Mutex::new(std::collections::BTreeMap::new());

/// Font file `file`, the copy already open when there is one. The lock is held while it opens, so two players opening
/// the same file at once read it once.
fn shared_font(file: &str) -> Result<Arc<Font>, String> {
    let mut fonts = FONTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(f) = fonts.get(file).and_then(|w| w.upgrade()) {
        return Ok(f);
    }
    fonts.retain(|_, w| w.strong_count() > 0);
    let f = Font::open(Path::new(file)).map(Arc::new).map_err(|e| format!("{file}: {e}"))?;
    fonts.insert(file.to_string(), Arc::downgrade(&f));
    Ok(f)
}

/// A stored path made real: `%NAME%` placeholders read from the environment, FL's factory-data ones looked up.
pub fn resolve(stored: &str) -> String {
    let mut out = String::new();
    let mut rest = stored;
    while let Some(a) = rest.find('%') {
        let Some(b) = rest[a + 1..].find('%') else { break };
        let name = &rest[a + 1..a + 1 + b];
        match std::env::var(name) {
            Ok(v) if !name.is_empty() => {
                out.push_str(&rest[..a]);
                out.push_str(&v);
                rest = &rest[a + 2 + b..];
            }
            _ => break,
        }
    }
    out.push_str(rest);
    if out.contains('%') {
        if let Some(p) = os_engine::SampleLibrary::new(os_engine::SampleLibrary::default_factory_dir()).resolve(&out) {
            return p.to_string_lossy().into_owned();
        }
    }
    out
}

impl Shared {
    fn font(&self, file: &str) -> Result<Arc<Font>, String> {
        if let Some(f) = self.cache.lock().ok().and_then(|c| c.get(file).cloned()) {
            return f;
        }
        let f = shared_font(file);
        if let Ok(mut c) = self.cache.lock() {
            c.insert(file.to_string(), f.clone());
        }
        f
    }

    /// The font the state names, when it opened.
    pub fn current_font(&self) -> Option<Arc<Font>> {
        self.patch.lock().ok()?.as_ref().map(|p| p.font.clone())
    }

    pub fn current_preset(&self) -> Option<usize> {
        self.patch.lock().ok()?.as_ref()?.preset
    }

    /// Rebuilds what the voices play from the saved state, opening the font it names.
    pub fn rebuild(&self, params: &FontParams) {
        let file = params.font_file();
        let patch = if file.is_empty() {
            self.set_error(String::new());
            None
        } else {
            match self.font(&resolve(&file)) {
                Ok(font) => {
                    self.set_error(String::new());
                    let st = params.state();
                    let preset = st.preset_in(&font);
                    Some(Arc::new(Patch { font, preset, overrides: st.overrides() }))
                }
                Err(e) => {
                    self.set_error(e);
                    None
                }
            }
        };
        if let Ok(mut p) = self.patch.lock() {
            *p = patch;
        }
        self.mode.store(params.mode().index(), Ordering::Release);
        self.patch_version.fetch_add(1, Ordering::AcqRel);
    }

    fn set_error(&self, e: String) {
        if let Ok(mut s) = self.error.lock() {
            *s = e;
        }
    }

    /// Opens font file `path` (the editor's open dialog, a dropped file) and plays its first preset.
    pub fn load(&self, params: &FontParams, path: &str) -> Result<(), String> {
        if let Ok(mut c) = self.cache.lock() {
            c.remove(path);
        }
        if let Ok(mut c) = FONTS.lock() {
            c.remove(path);
        }
        let font = self.font(path)?;
        let mut st = params.state();
        st.set_path(path);
        st.set_preset(&font, 0);
        if let Ok(mut p) = params.path.write() {
            p.clear();
        }
        params.set_raw(st.to_bytes());
        self.rebuild(params);
        Ok(())
    }

    /// Chooses preset `i` of the open font.
    pub fn choose(&self, params: &FontParams, i: usize) {
        let Some(font) = self.current_font() else { return };
        let mut st = params.state();
        st.set_preset(&font, i);
        params.set_raw(st.to_bytes());
        self.rebuild(params);
    }

    pub fn set_mode(&self, params: &FontParams, m: Mode) {
        params.set_mode(m);
        self.rebuild(params);
    }
}

pub struct LemonSoundfont {
    params: Arc<FontParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    patch_seen: u64,
}

impl Default for LemonSoundfont {
    fn default() -> Self {
        LemonSoundfont { params: Arc::new(FontParams::default()), shared: Arc::new(Shared::default()), engine: dsp::Engine::new(44100.0), patch_seen: u64::MAX }
    }
}

impl LemonSoundfont {
    /// The voices take the patch last built (never waits: a busy lock is tried again next block).
    fn take_patch(&mut self) {
        let v = self.shared.patch_version.load(Ordering::Acquire);
        if v != self.patch_seen
            && let Ok(p) = self.shared.patch.try_lock()
        {
            let patch = p.clone();
            drop(p);
            self.engine.set_patch(patch);
            self.engine.mode = Mode::from_index(self.shared.mode.load(Ordering::Acquire));
            self.patch_seen = v;
        }
    }
}

impl Plugin for LemonSoundfont {
    const NAME: &'static str = "Lemon SoundFont Player";
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

    #[cfg(feature = "editor")]
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::FontEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.engine.set_rate(config.sample_rate);
        // A restored state names its font: open it now (initialize may block).
        self.shared.rebuild(&self.params);
        self.take_patch();
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.take_patch();
        if let Ok(mut q) = self.shared.previews.try_lock() {
            for (key, vel, on) in q.drain(..) {
                if on {
                    self.engine.note_on(key, vel, None);
                } else {
                    self.engine.note_off(key, None);
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
                NoteEvent::NoteOff { note, voice_id, .. } => self.engine.note_off(note, voice_id),
                NoteEvent::PolyTuning { note, voice_id, tuning, .. } => self.engine.set_tuning(note, voice_id, f64::from(tuning)),
                NoteEvent::MidiPitchBend { value, .. } => self.engine.controls.bend = value * 2.0 - 1.0,
                NoteEvent::MidiChannelPressure { pressure, .. } => self.engine.controls.pressure = pressure,
                NoteEvent::MidiCC { cc: 120 | 123, .. } | NoteEvent::Choke { .. } => self.engine.all_off(),
                NoteEvent::MidiCC { cc, value, .. } if (cc as usize) < 128 && !matches!(cc, 7 | 10 | 11) => self.engine.controls.cc[cc as usize] = value,
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

impl ClapPlugin for LemonSoundfont {
    const CLAP_ID: &'static str = "org.openstudio.lemon-soundfont";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's SoundFont player");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Sampler, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonSoundfont {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioSFont";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Sampler];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonSoundfont);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonSoundfont);
os_plugkit::linked_clap_entry!(LemonSoundfont);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_font_plays_its_preset() {
        let path = std::env::temp_dir().join(format!("lemon-sf-{}.sf2", std::process::id()));
        std::fs::write(&path, sf2::tests::font_bytes(&[8000; 4000], 44100, (0, 0), &[], 3)).unwrap();
        let params = FontParams::default();
        let sh = Shared::default();
        sh.load(&params, &path.to_string_lossy()).unwrap();
        assert_eq!(params.state().path(), path.to_string_lossy());
        assert_eq!(sh.current_preset(), Some(0));
        let mut e = dsp::Engine::new(44100.0);
        e.set_patch(sh.patch.lock().unwrap().clone());
        e.note_on(60, 100.0 / 128.0, None);
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        e.render(&mut l, &mut r);
        assert!(l.iter().any(|v| v.abs() > 0.05));
        assert!(sh.load(&params, "Z:\\nowhere.sf2").is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn placeholders_resolve_from_the_environment() {
        // SAFETY: tests in this module don't read this variable concurrently.
        unsafe { std::env::set_var("LEMON_SF_TEST", "C:\\x") };
        assert_eq!(resolve("%LEMON_SF_TEST%\\a.sf2"), "C:\\x\\a.sf2");
        assert_eq!(resolve("C:\\plain.sf2"), "C:\\plain.sf2");
    }
}
