// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Smith: Lemon Studio's additive synthesizer (a recreation of FL's Ogun) as a VST3 / CLAP plugin (nih-plug). It plays
//! Ogun patches: a project's or preset's state is loaded as it is stored and kept byte for byte until the first edit.
//!
// spec: plugins/smith.md, plugins/ogun.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod ctl;
pub mod editor;
pub mod fft;
pub mod seed_words;
pub mod state;
pub mod synth;
pub mod view;

pub use citrus::preset::state_from_fst;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use citrus::voice::Levels;
use nih_plug::prelude::*;

use synth::{NoteOn, Patch, Smith};

/// An Ogun state (event 213's bytes) as a persisted field: a hex string.
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

/// The persisted field holding the Ogun state.
pub const STATE_FIELD: &str = "ogun-state";

/// A plugin state (the host's state blob: its length, then nih-plug's JSON) holding the Ogun state `fl` as it is.
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let field = serde_json::to_string(&StateBytes(fl.to_vec())).unwrap_or_default();
    let json = serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "params": {}, "fields": { STATE_FIELD: field } }).to_string();
    let mut blob = (json.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(json.as_bytes());
    blob
}

/// The Ogun state inside a plugin state blob (see [`plugin_state_from_fl`]); `Some(empty)` for an instance that still
/// plays its default patch, `None` when the blob is not a Smith state.
pub fn fl_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    let (_, json) = os_hostlink::state::json(blob);
    let v: serde_json::Value = serde_json::from_str(std::str::from_utf8(json).ok()?).ok()?;
    let field = v.get("fields")?.get(STATE_FIELD)?.as_str()?;
    serde_json::from_str::<StateBytes>(field).ok().map(|s| s.0)
}

/// The patch stored bytes stand for: the default for no state or one that does not decode.
pub fn state_of(bytes: &[u8]) -> state::State {
    if bytes.is_empty() { state::State::default() } else { state::State::from_bytes(bytes).unwrap_or_default() }
}

#[derive(Params)]
pub struct SmithParams {
    /// The Ogun state in use (event 213's bytes), as loaded; empty for the default patch.
    #[persist = "ogun-state"]
    pub state: Arc<RwLock<StateBytes>>,
    /// Bumped whenever `state` is replaced (not when the editor edits it).
    pub state_version: AtomicU64,
    /// The editor's latest edit for the audio thread: the patch, and the state version it edits.
    pub edit: Mutex<Option<(u64, Patch)>>,
    /// Notes the editor's testing keyboard plays itself (key, on).
    pub previews: Mutex<Vec<(u8, bool)>>,
    /// The keys held down, the host's notes and the testing keyboard's (bit `k % 64` of word `k / 64` for key `k`), as
    /// of the audio thread's last block: the editor's keyboard shows them.
    pub playing: [AtomicU64; 2],
    #[id = "modx"]
    pub mod_x: FloatParam,
    #[id = "mody"]
    pub mod_y: FloatParam,
}

impl Default for SmithParams {
    fn default() -> Self {
        SmithParams {
            state: Arc::new(RwLock::new(StateBytes::default())),
            state_version: AtomicU64::new(1),
            edit: Mutex::new(None),
            previews: Mutex::new(Vec::new()),
            playing: [AtomicU64::new(0), AtomicU64::new(0)],
            mod_x: FloatParam::new("Mod A", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 }).with_poly_modulation_id(MOD_X_POLY),
            mod_y: FloatParam::new("Mod B", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 }).with_poly_modulation_id(MOD_Y_POLY),
        }
    }
}

impl SmithParams {
    /// Replaces the state (a preset or a project's channel); the audio thread picks it up at its next block.
    pub fn set_state(&self, bytes: Vec<u8>) {
        // The version moves while the state is locked, so an edit of the state it replaces cannot land after it.
        let s = self.state.write();
        if let Ok(mut s) = s {
            *s = StateBytes(bytes);
        }
        self.state_version.fetch_add(1, Ordering::AcqRel);
    }

    /// An edit's state without its patch yet (during a drag the editor makes the patch once a repaint): `bytes` become
    /// the state the host saves. False, and nothing changes, when the state was replaced since version `version` was read.
    pub fn stored(&self, version: u64, bytes: Vec<u8>) -> bool {
        let Ok(mut s) = self.state.write() else { return false };
        if self.state_version.load(Ordering::Acquire) != version {
            return false;
        }
        *s = StateBytes(bytes);
        true
    }

    /// The patch of the edited state: the audio thread plays it from its next block. False, and nothing changes, when
    /// the state was replaced since version `version` was read.
    pub fn play(&self, version: u64, patch: Patch) -> bool {
        let Ok(_s) = self.state.read() else { return false };
        if self.state_version.load(Ordering::Acquire) != version {
            return false;
        }
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
    /// The host's tuning of the note in cents (portamento and slide notes: a note expression).
    tune: f32,
}

/// A note's pitch in cents from key 60: its key, the pitch bend and the host's tuning.
fn note_pitch(h: &Held, bend: f32) -> f32 {
    (f32::from(h.note) - 60.0) * 100.0 + bend + h.tune
}

/// Released notes kept for the host's tunings (a glide goes on while a note rings out); the oldest goes first.
const RINGING: usize = 64;

/// The poly modulation ids of Mod X and Mod Y: a note's mod X / Y as the host's per-note modulation, in mod units added
/// to the parameter.
const MOD_X_POLY: u32 = 1;
const MOD_Y_POLY: u32 = 2;

/// The mod X / Y offsets the host gives voice `id` right after its note-on at `timing` (the events after it, `rest`).
fn start_mods(rest: &[NoteEvent<()>], timing: u32, id: i32) -> [f32; 2] {
    let mut mods = [0.0; 2];
    for e in rest.iter().take_while(|e| e.timing() == timing) {
        if let NoteEvent::PolyModulation { voice_id, poly_modulation_id, normalized_offset, .. } = *e
            && voice_id == id
            && let Some(k) = [MOD_X_POLY, MOD_Y_POLY].iter().position(|&p| p == poly_modulation_id)
        {
            mods[k] = normalized_offset;
        }
    }
    mods
}

pub struct SmithPlugin {
    params: Arc<SmithParams>,
    engine: Smith,
    seen: u64,
    held: Vec<Held>,
    /// Notes ended by the host, newest last: a tuning still moves them while they ring.
    ringing: Vec<Held>,
    bend: f32,
    /// The block's events, read ahead for a note's mods.
    events: Vec<NoteEvent<()>>,
}

impl Default for SmithPlugin {
    fn default() -> Self {
        SmithPlugin {
            params: Arc::new(SmithParams::default()),
            engine: Smith::new(44100),
            seen: 0,
            held: Vec::new(),
            ringing: Vec::new(),
            bend: 0.0,
            events: Vec::with_capacity(1024),
        }
    }
}

impl SmithPlugin {
    fn pick_up_state(&mut self) {
        let v = self.params.state_version.load(Ordering::Acquire);
        if v == self.seen {
            return;
        }
        if let Ok(b) = self.params.state.try_read() {
            // An empty state (a fresh instance), or one that does not decode, plays the default patch; the stored bytes
            // stay as they are.
            if b.0.is_empty() || !self.engine.load_state(&b.0) {
                self.engine.set_patch(Patch::default_patch());
            }
            self.seen = v;
            self.held.clear();
            self.ringing.clear();
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
                        self.ringing.clear();
                    }
                }
            }
        }
        if let Ok(mut q) = self.params.previews.try_lock() {
            for (note, on) in q.drain(..) {
                if on {
                    let velocity = 100.0 / 128.0;
                    let id = self.engine.note_on(NoteOn { levels: self.levels(note), velocity });
                    self.held.push(Held { note, channel: PREVIEW, voice_id: None, id, tune: 0.0 });
                } else if let Some(i) = self.held.iter().position(|h| h.note == note && h.channel == PREVIEW) {
                    let h = self.held.remove(i);
                    self.engine.note_off(h.id);
                }
            }
        }
    }

    /// The held keys for the editor.
    fn show_playing(&self) {
        let bits = playing_bits(self.held.iter().map(|h| h.note));
        for (a, b) in self.params.playing.iter().zip(bits) {
            a.store(b, Ordering::Relaxed);
        }
    }

    /// A note's own levels. The host sends FL's velocity byte / 128; the engine applies the velocity's own law. The
    /// channel's volume and pan are the host's (Smith does not take them over).
    fn levels(&self, note: u8) -> Levels {
        Levels {
            vol: 1.0,
            pan: 0.0,
            pitch: note_pitch(&Held { note, channel: 0, voice_id: None, id: 0, tune: 0.0 }, self.bend),
            mod_x: self.params.mod_x.value(),
            mod_y: self.params.mod_y.value(),
        }
    }
}

impl Plugin for SmithPlugin {
    const NAME: &'static str = "Smith";
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
        Some(Box::new(editor::SmithEditor::new(self.params.clone())))
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.held.clear();
        self.ringing.clear();
        self.show_playing();
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
        self.events.clear();
        while let Some(ev) = ctx.next_event() {
            self.events.push(ev);
        }
        for i in 0..self.events.len() {
            let ev = self.events[i];
            let t = (ev.timing() as usize).min(len);
            if t > at {
                let (left, right) = out.split_at_mut(1);
                self.engine.render(&mut left[0][at..t], &mut right[0][at..t]);
                at = t;
            }
            match ev {
                NoteEvent::NoteOn { timing, note, velocity, voice_id, channel } => {
                    let mut levels = self.levels(note);
                    if let Some(v) = voice_id {
                        let [x, y] = start_mods(&self.events[i + 1..], timing, v);
                        (levels.mod_x, levels.mod_y) = (levels.mod_x + x, levels.mod_y + y);
                    }
                    let id = self.engine.note_on(NoteOn { levels, velocity });
                    self.held.push(Held { note, channel, voice_id, id, tune: 0.0 });
                }
                NoteEvent::NoteOff { note, voice_id, channel, .. } => {
                    if let Some(i) = self.held.iter().position(|h| match voice_id {
                        Some(v) => h.voice_id == Some(v),
                        None => h.note == note && h.channel == channel,
                    }) {
                        let h = self.held.remove(i);
                        self.engine.note_off(h.id);
                        if self.ringing.len() >= RINGING {
                            self.ringing.remove(0);
                        }
                        self.ringing.push(h);
                    }
                }
                NoteEvent::Choke { .. } => {
                    self.engine.panic();
                    self.held.clear();
                    self.ringing.clear();
                }
                // Portamento and slide notes: the host's tuning of a key's notes, the one its id names or else every
                // note of the key, ringing ones too.
                NoteEvent::PolyTuning { note, voice_id, channel, tuning, .. } => {
                    let named = voice_id.is_some_and(|v| self.held.iter().chain(&self.ringing).any(|h| h.voice_id == Some(v)));
                    let bend = self.bend;
                    for h in self.held.iter_mut().chain(self.ringing.iter_mut()) {
                        let hit = if named { h.voice_id == voice_id } else { h.note == note && h.channel == channel };
                        if hit {
                            h.tune = tuning * 100.0;
                            self.engine.bend(h.id, note_pitch(h, bend));
                        }
                    }
                }
                // A note's own pan (a note expression): the note its id names, or else every note of the key.
                NoteEvent::PolyPan { note, voice_id, channel, pan, .. } => {
                    let named = voice_id.is_some_and(|v| self.held.iter().chain(&self.ringing).any(|h| h.voice_id == Some(v)));
                    for h in self.held.iter().chain(&self.ringing) {
                        if if named { h.voice_id == voice_id } else { h.note == note && h.channel == channel } {
                            self.engine.note_pan(h.id, pan);
                        }
                    }
                }
                // A note's mod X / Y (the host's per-note modulation of Mod X / Mod Y), added to the parameter.
                NoteEvent::PolyModulation { voice_id, poly_modulation_id, normalized_offset, .. } => {
                    let k = match poly_modulation_id {
                        MOD_X_POLY => 0,
                        MOD_Y_POLY => 1,
                        _ => continue,
                    };
                    let value = [&self.params.mod_x, &self.params.mod_y][k].value() + normalized_offset;
                    for h in self.held.iter().chain(&self.ringing).filter(|h| h.voice_id == Some(voice_id)) {
                        self.engine.note_mod(h.id, k, value);
                    }
                }
                NoteEvent::MidiPitchBend { value, .. } => {
                    // ±2 semitones, as the other Lemon instruments.
                    self.bend = (value * 2.0 - 1.0) * 200.0;
                    for h in self.held.iter().chain(&self.ringing) {
                        self.engine.bend(h.id, note_pitch(h, self.bend));
                    }
                }
                NoteEvent::MidiCC { cc: 120 | 123, .. } => {
                    self.engine.all_off();
                    self.held.clear();
                    self.ringing.clear();
                }
                _ => {}
            }
        }
        if at < len {
            let (left, right) = out.split_at_mut(1);
            self.engine.render(&mut left[0][at..len], &mut right[0][at..len]);
        }
        self.show_playing();
        if self.engine.voices() > 0 { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

impl ClapPlugin for SmithPlugin {
    const CLAP_ID: &'static str = "org.openstudio.smith";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's additive synthesizer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Synthesizer, ClapFeature::Stereo];
}

impl Vst3Plugin for SmithPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioSmith";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(SmithPlugin);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(SmithPlugin);
os_plugkit::linked_clap_entry!(SmithPlugin);

/// Keys as [`SmithParams::playing`] holds them.
fn playing_bits(notes: impl Iterator<Item = u8>) -> [u64; 2] {
    let mut bits = [0; 2];
    for k in notes.filter(|&k| k < 128) {
        bits[usize::from(k / 64)] |= 1 << (k % 64);
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Ogun state goes into a plugin state and comes back byte for byte; an empty one stands for the default patch.
    #[test]
    fn plugin_states_carry_the_ogun_state() {
        let fl = state::State::default().to_bytes();
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(u64::from_le_bytes(blob[..8].try_into().unwrap()) as usize, blob.len() - 8);
        assert_eq!(fl_from_plugin_state(&blob), Some(fl.clone()));
        assert_eq!(fl_from_plugin_state(&blob[8..]), Some(fl));
        assert_eq!(fl_from_plugin_state(&plugin_state_from_fl(&[])), Some(Vec::new()));
        assert_eq!(fl_from_plugin_state(b"{\"params\":{},\"fields\":{}}"), None);
        assert_eq!(fl_from_plugin_state(&[1, 2, 3]), None);
    }

    #[test]
    fn held_keys_as_bits() {
        assert_eq!(playing_bits([0, 63, 64, 127, 60, 60].into_iter()), [1 | 1 << 63 | 1 << 60, 1 | 1 << 63]);
    }

    #[test]
    fn a_broken_state_shows_the_default_patch() {
        assert_eq!(state_of(&[]), state::State::default());
        assert_eq!(state_of(&[1, 2, 3]), state::State::default());
    }

    /// A state replaced while the editor edits the one before it refuses the edit.
    #[test]
    fn an_edit_of_a_replaced_state_is_refused() {
        let p = SmithParams::default();
        let v = p.state_version.load(Ordering::Acquire);
        assert!(p.stored(v, vec![1]));
        p.set_state(vec![2]);
        assert!(!p.stored(v, vec![3]));
        assert_eq!(p.state.read().unwrap().0, vec![2]);
    }
}
