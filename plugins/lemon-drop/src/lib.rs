// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Drop: Lemon Studio's audio visualizer (a recreation of FL's Wave Candy) as a VST3 / CLAP plugin (nih-plug).
//! The audio passes through untouched; while the editor is open the audio thread hands it the signal, and the editor
//! draws it as an oscilloscope, a spectrum, a level meter or a vectorscope. A project's or preset's state is kept as
//! the original stores it, and the editor edits that state.
//!
// spec: plugins/lemon-drop.md, plugins/wave-candy.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod analysis;
pub mod display;
pub mod editor;
#[cfg(windows)]
pub mod float;
#[cfg(target_os = "macos")]
#[path = "float_mac.rs"]
pub mod float;
pub mod state;
pub mod view;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicIsize, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use nih_plug::prelude::*;
use state::State;

/// The plugin's name: the one place it is spelled.
pub const NAME: &str = "Lemon Drop";

/// The original's state (event 213's bytes) as a persisted field: a hex string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateBytes(pub Vec<u8>);

impl serde::Serialize for StateBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.iter().map(|x| format!("{x:02x}")).collect::<String>())
    }
}

impl<'de> serde::Deserialize<'de> for StateBytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(d)?;
        from_hex(&text).map(StateBytes).ok_or_else(|| serde::de::Error::custom("not a hex string"))
    }
}

fn from_hex(t: &str) -> Option<Vec<u8>> {
    let t = t.as_bytes();
    if t.len() % 2 != 0 {
        return None;
    }
    t.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok()).collect()
}

/// The persisted field holding the state.
pub const STATE_FIELD: &str = "wave-candy-state";

/// A plugin state (the host's state blob: its length, then nih-plug's JSON) holding the original's state `fl`.
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let field = serde_json::to_string(&StateBytes(fl.to_vec())).unwrap_or_default();
    let json = serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "params": {}, "fields": { STATE_FIELD: field } }).to_string();
    let mut blob = (json.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(json.as_bytes());
    blob
}

/// The original's state inside a plugin state blob; `Some(empty)` for an instance on its starting state.
pub fn fl_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    let (_, json) = os_hostlink::state::json(blob);
    let v: serde_json::Value = serde_json::from_str(std::str::from_utf8(json).ok()?).ok()?;
    let field = v.get("fields")?.get(STATE_FIELD)?.as_str()?;
    serde_json::from_str::<StateBytes>(field).ok().map(|s| s.0)
}

/// The state a stored blob stands for: a new Lemon Drop's for none, or for one that does not read.
pub fn state_of(bytes: &[u8]) -> State {
    (!bytes.is_empty()).then(|| State::from_bytes(bytes)).flatten().unwrap_or_else(State::lemon_default)
}

/// Samples kept for the editor (per channel): enough for the largest analysis (8192 bands) and the longest block.
pub const SCOPE_LEN: usize = 1 << 16;

/// The signal the audio thread hands the editor.
pub struct Scope {
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub pos: usize,
    /// Samples written in total.
    pub written: u64,
}

impl Scope {
    /// The newest `n` frames, oldest first, into `l` and `r`.
    pub fn latest(&self, n: usize, l: &mut Vec<f32>, r: &mut Vec<f32>) {
        let len = self.l.len();
        let n = n.min(len);
        l.clear();
        r.clear();
        for i in 0..n {
            let k = (self.pos + len - n + i) % len;
            l.push(self.l[k]);
            r.push(self.r[k]);
        }
    }
}

/// What the audio thread shares with the editor.
pub struct Shared {
    pub scope: Mutex<Scope>,
    pub rate: AtomicU32,
    /// Displays open (the editor, the floating window): the audio thread feeds the scope only while there are.
    pub viewers: AtomicUsize,
    /// The floating display window (Win32 HWND; 0 = none).
    pub float: AtomicIsize,
    /// The floating display's GUI thread; dropping it closes the window (macOS).
    #[cfg(target_os = "macos")]
    pub float_thread: Mutex<Option<os_plugkit::gui::EditorThread>>,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            scope: Mutex::new(Scope { l: vec![0.0; SCOPE_LEN], r: vec![0.0; SCOPE_LEN], pos: 0, written: 0 }),
            rate: AtomicU32::new(44100),
            viewers: AtomicUsize::new(0),
            float: AtomicIsize::new(0),
            #[cfg(target_os = "macos")]
            float_thread: Mutex::new(None),
        }
    }
}

#[derive(Params)]
pub struct DropParams {
    /// The state in use (event 213's bytes), as loaded or as the editor last wrote it; empty for the starting state.
    #[persist = "wave-candy-state"]
    pub state: Arc<RwLock<StateBytes>>,
    /// Bumped whenever `state` is replaced from outside (not when the editor edits it).
    pub state_version: AtomicU64,
    /// Bumped by every edit, so each display (the editor, the floating window) reads the other's edits.
    pub edits: AtomicU64,
    /// The original's one automatable parameter: freezes the display.
    #[id = "hold"]
    pub hold: BoolParam,
}

impl Default for DropParams {
    fn default() -> Self {
        DropParams {
            state: Arc::new(RwLock::new(StateBytes::default())),
            state_version: AtomicU64::new(1),
            edits: AtomicU64::new(0),
            hold: BoolParam::new("Hold visualization", false),
        }
    }
}

impl DropParams {
    /// Replaces the state (a preset or a project's slot); the editor reads it again on its next tick.
    pub fn set_state(&self, bytes: Vec<u8>) {
        if let Ok(mut s) = self.state.write() {
            *s = StateBytes(bytes);
        }
        self.state_version.fetch_add(1, Ordering::AcqRel);
    }

    /// An edit from the editor: `st` becomes the state the host saves. False, and nothing changes, when the state was
    /// replaced since version `version` was read.
    pub fn edited(&self, version: u64, st: &State) -> bool {
        let Ok(mut s) = self.state.write() else {
            return false;
        };
        if self.state_version.load(Ordering::Acquire) != version {
            return false;
        }
        *s = StateBytes(st.to_bytes());
        self.edits.fetch_add(1, Ordering::AcqRel);
        true
    }

    /// The state as it stands, with its version.
    pub fn current(&self) -> (u64, State) {
        let v = self.state_version.load(Ordering::Acquire);
        let st = self.state.read().map(|b| state_of(&b.0)).unwrap_or_default();
        (v, st)
    }
}

#[derive(Default)]
pub struct LemonDrop {
    params: Arc<DropParams>,
    shared: Arc<Shared>,
}

impl LemonDrop {
    fn feed(&self, l: &[f32], r: &[f32]) {
        if let Ok(mut s) = self.shared.scope.try_lock() {
            let n = s.l.len();
            for (&a, &b) in l.iter().zip(r) {
                let p = s.pos;
                s.l[p] = a;
                s.r[p] = b;
                s.pos = (p + 1) % n;
            }
            s.written += l.len() as u64;
        }
    }
}

impl Drop for LemonDrop {
    fn drop(&mut self) {
        // The floating display outlives the editor, not the plugin.
        #[cfg(any(windows, target_os = "macos"))]
        float::close(&self.shared);
    }
}

impl Plugin for LemonDrop {
    const NAME: &'static str = NAME;
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] =
        &[AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::DropEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.shared.rate.store(config.sample_rate.round().max(1.0) as u32, Ordering::Relaxed);
        // nih-plug initializes the plugin again after the host loads a state: the editor reads it again too.
        self.params.state_version.fetch_add(1, Ordering::AcqRel);
        true
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let len = buffer.samples();
        let out = buffer.as_slice();
        if out.len() >= 2 && self.shared.viewers.load(Ordering::Relaxed) > 0 && !self.params.hold.value() {
            self.feed(&out[0][..len], &out[1][..len]);
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for LemonDrop {
    const CLAP_ID: &'static str = "org.openstudio.lemon-drop";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's audio visualizer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Analyzer, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonDrop {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioDrops";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonDrop);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonDrop);
os_plugkit::linked_clap_entry!(LemonDrop);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_states_carry_the_original_state() {
        let fl = State::new_default().to_bytes();
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(fl_from_plugin_state(&blob), Some(fl.clone()));
        assert_eq!(fl_from_plugin_state(&blob[8..]), Some(fl.clone()));
        assert_eq!(state_of(&fl), State::new_default());
        assert_eq!(state_of(&[]), State::lemon_default());
        assert_eq!(state_of(&[1, 2, 3]), State::lemon_default());
    }

    #[test]
    fn edits_of_a_replaced_state_are_refused() {
        let p = DropParams::default();
        let (v, mut st) = p.current();
        st.gain = 30;
        assert!(p.edited(v, &st));
        assert_eq!(p.current().1.gain, 30);
        p.set_state(State::new_default().to_bytes());
        assert!(!p.edited(v, &st));
        assert_eq!(p.current().1.gain, 0);
    }

    /// Every factory preset (`crates/os-app/factory-presets/Lemon Drop/`, spec: plugins/lemon-drop.md "Presets") holds
    /// the plugin's saved state with a version 9 state that reads and writes back as it is; Default is a new Lemon Drop.
    #[test]
    fn factory_presets_hold_readable_states() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/os-app/factory-presets/Lemon Drop");
        let (mut n, mut walk) = (0, vec![root.clone()]);
        while let Some(d) = walk.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk.push(p);
                    continue;
                }
                let b = std::fs::read(&p).unwrap();
                let mut part = b.splitn(3, |&c| c == b'\n');
                assert_eq!(part.next(), Some(&b"OSPRESET 1"[..]), "{p:?}");
                let head = std::str::from_utf8(part.next().unwrap()).unwrap();
                assert!(head.starts_with("plugin=Lemon Drop\tid=clap:org.openstudio.lemon-drop\t") && !head.contains("body="), "{p:?}");
                let fl = fl_from_plugin_state(part.next().unwrap()).unwrap_or_else(|| panic!("{p:?}"));
                let st = State::from_bytes(&fl).unwrap_or_else(|| panic!("{p:?}"));
                assert_eq!(st.to_bytes(), fl, "{p:?}");
                if p == root.join("Default.ospreset") {
                    assert_eq!(st, State::lemon_default());
                }
                n += 1;
            }
        }
        assert_eq!(n, 49);
    }

    #[test]
    fn the_scope_keeps_the_newest_frames() {
        let d = LemonDrop::default();
        let a: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let b: Vec<f32> = a.iter().map(|x| -x).collect();
        d.feed(&a, &b);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        d.shared.scope.lock().unwrap().latest(3, &mut l, &mut r);
        assert_eq!(l, [7.0, 8.0, 9.0]);
        assert_eq!(r, [-7.0, -8.0, -9.0]);
    }
}
