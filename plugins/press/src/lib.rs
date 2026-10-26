// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Press: Lemon Studio's multiband maximiser (a recreation of FL's Maximus) as a VST3 / CLAP plugin (nih-plug). The
//! sound is `os_engine::effects::maximus`; a project's or preset's state is loaded as it is stored, and the editor
//! edits that state.
//!
// spec: plugins/lemon-press.md, plugins/maximus.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod ctl;
pub mod edit;
pub mod editor;
pub mod view;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use nih_plug::prelude::*;
use os_engine::effects::maximus::state::{State, BANDS};
use os_engine::effects::maximus::Maximus;

/// A Maximus state (event 213's bytes) as a persisted field: a hex string.
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
pub const STATE_FIELD: &str = "maximus-state";

/// A plugin state (the host's state blob: its length, then nih-plug's JSON) holding the Maximus state `fl`.
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let field = serde_json::to_string(&StateBytes(fl.to_vec())).unwrap_or_default();
    let json = serde_json::json!({ "version": env!("CARGO_PKG_VERSION"), "params": {}, "fields": { STATE_FIELD: field } }).to_string();
    let mut blob = (json.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(json.as_bytes());
    blob
}

/// The Maximus state inside a plugin state blob; `Some(empty)` for an instance on its starting state.
pub fn fl_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    let json = if blob.len() > 8 && blob[0] != b'{' { &blob[8..] } else { blob };
    let v: serde_json::Value = serde_json::from_str(std::str::from_utf8(json).ok()?).ok()?;
    let field = v.get("fields")?.get(STATE_FIELD)?.as_str()?;
    serde_json::from_str::<StateBytes>(field).ok().map(|s| s.0)
}

/// The state a stored blob stands for: the factory default for none, or for one that does not read.
pub fn state_of(bytes: &[u8]) -> State {
    (!bytes.is_empty()).then(|| State::from_bytes(bytes)).flatten().unwrap_or_else(State::new_default)
}

#[derive(Params)]
pub struct PressParams {
    /// The state in use (event 213's bytes), as loaded or as the editor last wrote it; empty for the starting state.
    #[persist = "maximus-state"]
    pub state: Arc<RwLock<StateBytes>>,
    /// Bumped whenever `state` is replaced from outside (not when the editor edits it).
    pub state_version: AtomicU64,
    /// The editor's latest edit for the audio thread, and the state version it edits.
    pub edit: Mutex<Option<(u64, State)>>,
    /// Each band's detector level and gain (f32 bits) as of the audio thread's last block.
    pub meters: [[AtomicU32; 2]; BANDS],
}

impl Default for PressParams {
    fn default() -> Self {
        PressParams {
            state: Arc::new(RwLock::new(StateBytes::default())),
            state_version: AtomicU64::new(1),
            edit: Mutex::new(None),
            meters: std::array::from_fn(|_| [AtomicU32::new(0), AtomicU32::new(1f32.to_bits())]),
        }
    }
}

impl PressParams {
    /// Replaces the state (a preset or a project's slot); the audio thread picks it up at its next block.
    pub fn set_state(&self, bytes: Vec<u8>) {
        let s = self.state.write();
        if let Ok(mut s) = s {
            *s = StateBytes(bytes);
        }
        self.state_version.fetch_add(1, Ordering::AcqRel);
    }

    /// An edit from the editor: `st` becomes the state the host saves and the audio thread plays it from its next
    /// block. False, and nothing changes, when the state was replaced since version `version` was read.
    pub fn edited(&self, version: u64, st: State) -> bool {
        let Ok(mut s) = self.state.write() else { return false };
        if self.state_version.load(Ordering::Acquire) != version {
            return false;
        }
        *s = StateBytes(st.to_bytes());
        if let Ok(mut e) = self.edit.lock() {
            *e = Some((version, st));
        }
        true
    }

    /// The meters as the audio thread left them.
    pub fn meters(&self) -> [[f32; 2]; BANDS] {
        self.meters.each_ref().map(|m| m.each_ref().map(|a| f32::from_bits(a.load(Ordering::Relaxed))))
    }
}

pub struct Press {
    params: Arc<PressParams>,
    dsp: Maximus,
    seen: u64,
    /// The delay last reported to the host.
    latency: Option<usize>,
}

impl Default for Press {
    fn default() -> Self {
        Press { params: Arc::new(PressParams::default()), dsp: Maximus::with_state(State::new_default(), 44100), seen: 0, latency: None }
    }
}

impl Press {
    /// A state replaced from outside: applied as FL applies a loaded state.
    fn pick_up_state(&mut self) {
        let v = self.params.state_version.load(Ordering::Acquire);
        if v == self.seen {
            return;
        }
        if let Ok(b) = self.params.state.try_read() {
            let st = state_of(&b.0);
            drop(b);
            self.dsp.apply(st);
            self.seen = v;
        }
    }

    fn pick_up_edits(&mut self) {
        if let Ok(mut e) = self.params.edit.try_lock() {
            if let Some((v, st)) = e.take() {
                if v == self.seen {
                    self.dsp.edit(st);
                }
            }
        }
    }

    /// The look-ahead (and the linear-phase crossover's delay) told to the host when it changes.
    fn report_latency(&mut self, set: impl FnOnce(u32)) {
        let d = self.dsp.delay();
        if self.latency != Some(d) {
            self.latency = Some(d);
            set(d as u32);
        }
    }
}

impl Plugin for Press {
    const NAME: &'static str = "Press";
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
        Some(Box::new(editor::PressEditor::new(self.params.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, ctx: &mut impl InitContext<Self>) -> bool {
        self.dsp.set_rate(f64::from(config.sample_rate.round().max(1.0)));
        // nih-plug initializes the plugin again after the host loads a state: the editor reads it again too.
        self.params.state_version.fetch_add(1, Ordering::AcqRel);
        self.seen = 0;
        self.pick_up_state();
        self.latency = None;
        self.report_latency(|d| ctx.set_latency_samples(d));
        true
    }

    fn reset(&mut self) {
        self.dsp.flush();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.pick_up_state();
        self.pick_up_edits();
        self.report_latency(|d| ctx.set_latency_samples(d));
        let out = buffer.as_slice();
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (left, right) = out.split_at_mut(1);
        self.dsp.process_planar(left[0], right[0]);
        for (a, m) in self.params.meters.iter().zip(self.dsp.meters()) {
            a[0].store(m[0].to_bits(), Ordering::Relaxed);
            a[1].store(m[1].to_bits(), Ordering::Relaxed);
        }
        // The look-ahead and the release ring on after the input stops.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Press {
    const CLAP_ID: &'static str = "org.openstudio.press";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's multiband maximiser");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Compressor, ClapFeature::Limiter, ClapFeature::Stereo];
}

impl Vst3Plugin for Press {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioPress";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(Press);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(Press);
os_plugkit::linked_clap_entry!(Press);

#[cfg(test)]
mod tests {
    use super::*;
    use os_engine::effects::maximus::state::{band, B_POST, MASTER};

    #[test]
    fn plugin_states_carry_the_maximus_state() {
        let fl = State::new_default().to_bytes();
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(fl_from_plugin_state(&blob), Some(fl.clone()));
        assert_eq!(fl_from_plugin_state(&blob[8..]), Some(fl.clone()));
        assert_eq!(state_of(&fl), State::new_default());
        assert_eq!(state_of(&[]), State::new_default());
        assert_eq!(state_of(&[1, 2, 3]), State::new_default());
    }

    /// An editor edit reaches the sound without a restart; a state replaced from outside is applied.
    #[test]
    fn edits_and_new_states_reach_the_sound() {
        let mut p = Press::default();
        p.pick_up_state();
        let v = p.params.state_version.load(Ordering::Acquire);
        let mut st = State::new_default();
        st.params[band(MASTER, B_POST)] = 500;
        assert!(p.params.edited(v, st.clone()));
        p.pick_up_edits();
        assert_eq!(p.dsp.state(), &st);
        assert_eq!(state_of(&p.params.state.read().unwrap().0), st);
        p.params.set_state(State::new_default().to_bytes());
        assert!(!p.params.edited(v, st), "an edit of a replaced state is refused");
        p.pick_up_state();
        assert_eq!(p.dsp.state(), &State::new_default());
    }

    /// Every factory preset (`crates/os-app/factory-presets/Press/`, spec: plugins/factory-presets.md) holds the
    /// plugin's saved state with a Maximus state that reads; Default is the starting state.
    #[test]
    fn factory_presets_hold_readable_states() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/os-app/factory-presets/Press");
        let (mut n, mut walk) = (0, vec![root.clone()]);
        while let Some(d) = walk.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk.push(p);
                    continue;
                }
                // Two header lines, then the saved state.
                let b = std::fs::read(&p).unwrap();
                let mut part = b.splitn(3, |&c| c == b'\n');
                assert_eq!(part.next(), Some(&b"OSPRESET 1"[..]), "{p:?}");
                let head = std::str::from_utf8(part.next().unwrap()).unwrap();
                assert!(head.starts_with("plugin=Press\tid=clap:org.openstudio.press\t") && !head.contains("body="), "{p:?}");
                let fl = fl_from_plugin_state(part.next().unwrap()).unwrap_or_else(|| panic!("{p:?}"));
                let st = State::from_bytes(&fl).unwrap_or_else(|| panic!("{p:?}"));
                if p == root.join("Default.ospreset") {
                    assert_eq!(st, State::new_default());
                }
                n += 1;
            }
        }
        assert_eq!(n, 41);
    }
}
