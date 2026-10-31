// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Drum Sampler's saved state: the original's whole saved state (`state`, hex), so what the editor does not
//! change comes back out byte for byte, and the sample files to open (`sample-paths`, a JSON list, one per layer in
//! the state's order; "" = the stored path). The host fills the list with the stored paths resolved (FL's
//! placeholders made real); the stored paths stay as written. No automatable parameters: the pads' settings live
//! in the state.
//!
// spec: plugins/lemon-drum-sampler.md §1

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use nih_plug::prelude::*;

use crate::state::State;

pub const STATE_FIELD: &str = "state";
pub const PATHS_FIELD: &str = "sample-paths";

pub struct DrumParams {
    /// The original's saved state.
    pub raw: RwLock<Vec<u8>>,
    /// Bumped on every `raw` change.
    pub raw_version: AtomicU64,
    /// The files to open, one per layer in state order ("" = the stored path).
    pub paths: RwLock<Vec<String>>,
}

impl Default for DrumParams {
    fn default() -> Self {
        DrumParams { raw: RwLock::new(default_state()), raw_version: AtomicU64::new(0), paths: RwLock::default() }
    }
}

unsafe impl Params for DrumParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        Vec::new()
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let hex = self.raw.read().map(|b| hex(&b)).unwrap_or_default();
        let paths = self.paths.read().map(|p| serde_json::to_string(&*p).unwrap_or_default()).unwrap_or_default();
        BTreeMap::from([(STATE_FIELD.to_string(), format!("\"{hex}\"")), (PATHS_FIELD.to_string(), paths)])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(bytes) = serialized.get(STATE_FIELD).and_then(|s| from_hex(s.trim_matches('"'))) {
            self.set_raw(bytes);
        }
        if let (Some(p), Ok(mut sp)) = (serialized.get(PATHS_FIELD).and_then(|s| serde_json::from_str::<Vec<String>>(s).ok()), self.paths.write()) {
            *sp = p;
        }
    }
}

impl DrumParams {
    pub fn set_raw(&self, b: Vec<u8>) {
        if let Ok(mut r) = self.raw.write() {
            *r = b;
        }
        self.raw_version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn raw_copy(&self) -> Vec<u8> {
        self.raw.read().map(|b| b.clone()).unwrap_or_else(|_| default_state())
    }

    /// The saved state parsed (a new instance's when it cannot be read).
    pub fn state(&self) -> State {
        State::parse(&self.raw_copy()).unwrap_or_default()
    }

    /// The file given for layer `n` (state order), "" when none.
    pub fn given_path(&self, n: usize) -> String {
        self.paths.read().ok().and_then(|p| p.get(n).cloned()).unwrap_or_default()
    }
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A new instance's saved state (32 empty pads).
pub fn default_state() -> Vec<u8> {
    State::default().to_bytes()
}

/// The plugin's own saved state (nih-plug JSON with the u64 length prefix of its CLAP state) for a channel's saved
/// state `fl` (a new instance's when empty) and the files to open for its layers.
pub fn plugin_state_from_fl(fl: &[u8], paths: &[String]) -> Vec<u8> {
    let b = if fl.is_empty() { default_state() } else { fl.to_vec() };
    let paths = serde_json::to_string(&serde_json::to_string(paths).unwrap_or_else(|_| "[]".into())).unwrap_or_default();
    let state = format!(
        "{{\"version\":\"{}\",\"params\":{{}},\"fields\":{{\"{STATE_FIELD}\":\"\\\"{}\\\"\",\"{PATHS_FIELD}\":{paths}}}}}",
        env!("CARGO_PKG_VERSION"),
        hex(&b)
    );
    let mut blob = (state.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(state.as_bytes());
    blob
}

/// The original's saved state in a plugin state blob (see [`plugin_state_from_fl`]); `None` when it is not one.
pub fn fl_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    let json = if blob.len() > 8 && blob[0] != b'{' { &blob[8..] } else { blob };
    let v: serde_json::Value = serde_json::from_str(std::str::from_utf8(json).ok()?).ok()?;
    from_hex(v.get("fields")?.get(STATE_FIELD)?.as_str()?.trim_matches('"'))
}

/// The stored sample paths of a saved state, one per layer in state order.
pub fn stored_paths(fl: &[u8]) -> Vec<String> {
    State::parse(fl).map(|s| s.pads().into_iter().flat_map(|p| p.layers.into_iter().map(|l| l.path)).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_and_paths_survive_the_plugin_state() {
        let st = crate::state::tests::kit("%FLStudioFactoryData%\\Data\\kick.wav", "s.wav", "h.wav").to_bytes();
        let paths = vec!["".to_string(), "C:\\FL\\Data\\kick.wav".into()];
        let blob = plugin_state_from_fl(&st, &paths);
        assert_eq!(fl_from_plugin_state(&blob), Some(st.clone()));
        let json = std::str::from_utf8(&blob[8..]).unwrap();
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let fields: BTreeMap<String, String> = v["fields"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        let p = DrumParams::default();
        p.deserialize_fields(&fields);
        assert_eq!(p.raw_copy(), st);
        assert_eq!(p.given_path(1), "C:\\FL\\Data\\kick.wav");
        assert_eq!(p.serialize_fields(), fields);
        assert_eq!(fl_from_plugin_state(&plugin_state_from_fl(&[], &[])), Some(default_state()));
        assert_eq!(stored_paths(&st)[0], "s.wav");
    }
}
