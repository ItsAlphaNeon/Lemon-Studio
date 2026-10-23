// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Citrus: Lemon Studio's six-operator synthesizer (a recreation of FL's Sytrus) as a VST3 / CLAP plugin (nih-plug).
//! It plays Sytrus patches: a project's or preset's state is loaded as it is stored and kept byte for byte.
//!
// spec: plugins/citrus.md, plugins/sytrus.md, plugins/sytrus-effects.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod ctl;
pub mod curve;
pub mod edit;
pub mod editor;
pub mod engine;
pub mod filter;
pub mod fx;
pub mod num;
pub mod patch;
pub mod preset;
pub mod state;
pub mod view;
pub mod voice;
pub mod wave;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use nih_plug::prelude::*;

use engine::{Citrus, NoteOn};
use voice::Levels;

/// A Sytrus state (event 213's bytes) as a persisted field: a hex string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateBytes(pub Vec<u8>);

impl serde::Serialize for StateBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex(&self.0))
    }
}

impl<'de> serde::Deserialize<'de> for StateBytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(d)?;
        from_hex(&text).map(StateBytes).ok_or_else(|| serde::de::Error::custom("not a hex string"))
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(t: &str) -> Option<Vec<u8>> {
    let t = t.as_bytes();
    if t.len() % 2 != 0 {
        return None;
    }
    t.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok()).collect()
}

/// The persisted field holding the Sytrus state.
pub const STATE_FIELD: &str = "sytrus-state";

/// A plugin state (the host's state blob: its length, then nih-plug's JSON) holding the Sytrus state `fl` as it is.
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let field = serde_json::to_string(&StateBytes(fl.to_vec())).unwrap_or_default();
    let json = serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "params": {}, "fields": { STATE_FIELD: field } }).to_string();
    let mut blob = (json.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(json.as_bytes());
    blob
}

/// The Sytrus state inside a plugin state blob (see [`plugin_state_from_fl`]); `Some(empty)` for an instance that
/// still plays its default patch, `None` when the blob is not a Citrus state.
pub fn fl_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    let json = if blob.len() > 8 && blob[0] != b'{' { &blob[8..] } else { blob };
    let v: serde_json::Value = serde_json::from_str(std::str::from_utf8(json).ok()?).ok()?;
    let field = v.get("fields")?.get(STATE_FIELD)?.as_str()?;
    serde_json::from_str::<StateBytes>(field).ok().map(|s| s.0)
}

#[derive(Params)]
pub struct CitrusParams {
    /// The Sytrus state in use (event 213's bytes), as loaded; empty for the default patch.
    #[persist = "sytrus-state"]
    pub state: Arc<RwLock<StateBytes>>,
    /// Bumped whenever `state` is replaced (not when the editor edits it).
    pub state_version: AtomicU64,
    /// The editor's latest edit for the audio thread: the patch, and the state version it edits.
    pub edit: Mutex<Option<(u64, patch::Patch)>>,
    /// Notes the editor's testing keyboard plays itself (key, on).
    pub previews: Mutex<Vec<(u8, bool)>>,
    #[id = "modx"]
    pub mod_x: FloatParam,
    #[id = "mody"]
    pub mod_y: FloatParam,
}

impl Default for CitrusParams {
    fn default() -> Self {
        CitrusParams {
            state: Arc::new(RwLock::new(StateBytes::default())),
            state_version: AtomicU64::new(1),
            edit: Mutex::new(None),
            previews: Mutex::new(Vec::new()),
            mod_x: FloatParam::new("Mod X", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 }),
            mod_y: FloatParam::new("Mod Y", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 }),
        }
    }
}

impl CitrusParams {
    /// Replaces the state (a preset or a project's channel); the audio thread picks it up at its next block.
    pub fn set_state(&self, bytes: Vec<u8>) {
        // The version moves while the state is locked, so an edit of the state it replaces cannot land after it.
        let s = self.state.write();
        if let Ok(mut s) = s {
            *s = StateBytes(bytes);
        }
        self.state_version.fetch_add(1, Ordering::AcqRel);
    }

    /// An edit from the editor (spec plugins/citrus.md §6): `bytes` become the state the host saves and the audio
    /// thread plays `patch` from its next block. The state version stays (the audio thread does not load the bytes
    /// again). False, and nothing changes, when the state was replaced since version `version` was read.
    pub fn edited(&self, version: u64, bytes: Vec<u8>, patch: patch::Patch) -> bool {
        let Ok(mut s) = self.state.write() else { return false };
        if self.state_version.load(Ordering::Acquire) != version {
            return false;
        }
        *s = StateBytes(bytes);
        if let Ok(mut e) = self.edit.lock() {
            *e = Some((version, patch));
        }
        true
    }
}

/// The channel the testing keyboard's notes are held on (no MIDI channel goes past 15).
const PREVIEW: u8 = 255;

#[derive(Clone, Copy)]
struct Held {
    note: u8,
    channel: u8,
    voice_id: Option<i32>,
    id: u64,
}

pub struct CitrusPlugin {
    params: Arc<CitrusParams>,
    engine: Citrus,
    seen: u64,
    held: Vec<Held>,
    bend: f32,
}

impl Default for CitrusPlugin {
    fn default() -> Self {
        CitrusPlugin { params: Arc::new(CitrusParams::default()), engine: Citrus::new(44100), seen: 0, held: Vec::new(), bend: 0.0 }
    }
}

impl CitrusPlugin {
    fn pick_up_state(&mut self) {
        let v = self.params.state_version.load(Ordering::Acquire);
        if v == self.seen {
            return;
        }
        if let Ok(b) = self.params.state.try_read() {
            // An empty state (a fresh instance), or one that does not decode, plays the default patch; the stored bytes
            // stay as they are.
            if b.0.is_empty() || !self.engine.load_state(&b.0) {
                self.engine.set_state(preset::default_patch());
            }
            self.seen = v;
            self.held.clear();
        }
    }

    /// The editor's latest edit of the state in use, and the notes its keyboard plays.
    fn pick_up_edits(&mut self) {
        if let Ok(mut e) = self.params.edit.try_lock() {
            if let Some((v, p)) = e.take() {
                if v == self.seen {
                    self.engine.edit(p);
                    if self.engine.voices() == 0 {
                        self.held.clear();
                    }
                }
            }
        }
        if let Ok(mut q) = self.params.previews.try_lock() {
            for (note, on) in q.drain(..) {
                if on {
                    let velocity = 100.0 / 128.0;
                    let id = self.engine.note_on(NoteOn { levels: self.levels(note, velocity), velocity, release: 0.5 });
                    self.held.push(Held { note, channel: PREVIEW, voice_id: None, id });
                } else if let Some(i) = self.held.iter().position(|h| h.note == note && h.channel == PREVIEW) {
                    let h = self.held.remove(i);
                    self.engine.note_off(h.id);
                }
            }
        }
    }

    /// A note's levels. The host sends FL's velocity byte / 128; the channel's volume is the host's to apply (after the
    /// plugin, as for Lemon Studio's other instruments), so the voice takes the velocity's level alone.
    fn levels(&self, note: u8, velocity: f32) -> Levels {
        let link = self.engine.state().is_none_or(|s| s.switch(8) != 0);
        let v = if link { (velocity * 128.0).round().min(128.0) * 0.01 } else { 1.0 };
        Levels {
            vol: os_dsp::level::curve(v),
            pan: 0.0,
            pitch: (f32::from(note) - 60.0) * 100.0 + self.bend,
            mod_x: self.params.mod_x.value(),
            mod_y: self.params.mod_y.value(),
        }
    }
}

impl Plugin for CitrusPlugin {
    const NAME: &'static str = "Citrus";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] =
        &[AudioIOLayout { main_input_channels: None, main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.engine.set_rate(config.sample_rate.round() as u32);
        self.engine.set_offline(config.process_mode == ProcessMode::Offline);
        // nih-plug initializes the plugin again after the host loads a state: the editor reads it again too.
        self.params.state_version.fetch_add(1, Ordering::AcqRel);
        self.pick_up_state();
        true
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::CitrusEditor::new(self.params.clone())))
    }

    fn reset(&mut self) {
        self.engine.panic();
        self.held.clear();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.pick_up_state();
        self.pick_up_edits();
        if let Some(bpm) = ctx.transport().tempo {
            self.engine.set_tempo(bpm);
        }
        let len = buffer.samples();
        let out = buffer.as_slice();
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let mut at = 0;
        while let Some(ev) = ctx.next_event() {
            let t = (ev.timing() as usize).min(len);
            if t > at {
                let (left, right) = out.split_at_mut(1);
                self.engine.render(&mut left[0][at..t], &mut right[0][at..t]);
                at = t;
            }
            match ev {
                NoteEvent::NoteOn { note, velocity, voice_id, channel, .. } => {
                    let levels = self.levels(note, velocity);
                    let id = self.engine.note_on(NoteOn { levels, velocity, release: 0.5 });
                    self.held.push(Held { note, channel, voice_id, id });
                }
                NoteEvent::NoteOff { note, voice_id, channel, velocity, .. } => {
                    let _ = velocity;
                    if let Some(i) = self.held.iter().position(|h| match voice_id {
                        Some(v) => h.voice_id == Some(v),
                        None => h.note == note && h.channel == channel,
                    }) {
                        let h = self.held.remove(i);
                        self.engine.note_off(h.id);
                    }
                }
                NoteEvent::Choke { .. } => {
                    self.engine.panic();
                    self.held.clear();
                }
                NoteEvent::MidiPitchBend { value, .. } => {
                    // ±2 semitones, as the other Lemon instruments.
                    self.bend = (value * 2.0 - 1.0) * 200.0;
                    for h in self.held.clone() {
                        let l = self.levels(h.note, 1.0);
                        self.engine.bend(h.id, l.pitch);
                    }
                }
                NoteEvent::MidiCC { cc: 120 | 123, .. } => self.engine.all_off(),
                _ => {}
            }
        }
        if at < len {
            let (left, right) = out.split_at_mut(1);
            self.engine.render(&mut left[0][at..len], &mut right[0][at..len]);
        }
        if self.engine.voices() > 0 { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

impl ClapPlugin for CitrusPlugin {
    const CLAP_ID: &'static str = "org.openstudio.citrus";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's six-operator synthesizer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Synthesizer, ClapFeature::Stereo];
}

impl Vst3Plugin for CitrusPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioCitrs";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(CitrusPlugin);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(CitrusPlugin);
os_plugkit::linked_clap_entry!(CitrusPlugin);

#[cfg(test)]
mod tests {
    use super::*;

    /// A Sytrus state goes into a plugin state and comes back byte for byte; an empty one stands for the default patch.
    #[test]
    fn plugin_states_carry_the_sytrus_state() {
        let fl = preset::test_state(&[1.0]);
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(u64::from_le_bytes(blob[..8].try_into().unwrap()) as usize, blob.len() - 8);
        assert_eq!(fl_from_plugin_state(&blob), Some(fl.clone()));
        assert_eq!(fl_from_plugin_state(&blob[8..]), Some(fl));
        assert_eq!(fl_from_plugin_state(&plugin_state_from_fl(&[])), Some(Vec::new()));
        assert_eq!(fl_from_plugin_state(b"{\"params\":{},\"fields\":{}}"), None);
        assert_eq!(fl_from_plugin_state(&[1, 2, 3]), None);
        // The persisted field is the state as hex.
        let json = serde_json::to_string(&StateBytes(vec![0, 0xAB, 7])).unwrap();
        assert_eq!(json, "\"00ab07\"");
        assert_eq!(serde_json::from_str::<StateBytes>(&json).unwrap(), StateBytes(vec![0, 0xAB, 7]));
        assert!(serde_json::from_str::<StateBytes>("\"0g\"").is_err());
    }
}
