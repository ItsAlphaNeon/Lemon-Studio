// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The player's saved state: the original's whole saved state (`state`, hex), the font file to open (`font-path`,
//! "" = the stored path) and the playback mode (`mode`: "classic", "modern", or "" for the one the stored state's
//! version implies). No automatable parameters.
//!
// spec: plugins/lemon-soundfont.md §1, §4

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use nih_plug::prelude::*;

use crate::profile::Mode;
use crate::state::State;

pub const STATE_FIELD: &str = "state";
pub const PATH_FIELD: &str = "font-path";
pub const MODE_FIELD: &str = "mode";

pub struct FontParams {
    pub raw: RwLock<Vec<u8>>,
    pub raw_version: AtomicU64,
    /// The font file to open ("" = the stored path).
    pub path: RwLock<String>,
    /// The chosen mode; `None` = the stored state's.
    pub mode: RwLock<Option<Mode>>,
}

impl Default for FontParams {
    fn default() -> Self {
        FontParams { raw: RwLock::new(default_state()), raw_version: AtomicU64::new(0), path: RwLock::default(), mode: RwLock::new(None) }
    }
}

fn mode_name(m: Option<Mode>) -> &'static str {
    match m {
        Some(Mode::Classic) => "classic",
        Some(Mode::Modern) => "modern",
        None => "",
    }
}

fn mode_of(s: &str) -> Option<Mode> {
    match s {
        "classic" => Some(Mode::Classic),
        "modern" => Some(Mode::Modern),
        _ => None,
    }
}

unsafe impl Params for FontParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        Vec::new()
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let hex = self.raw.read().map(|b| hex(&b)).unwrap_or_default();
        let path = self.path.read().map(|p| serde_json::to_string(&*p).unwrap_or_default()).unwrap_or_default();
        let mode = self.mode.read().map(|m| mode_name(*m)).unwrap_or_default();
        BTreeMap::from([
            (STATE_FIELD.to_string(), format!("\"{hex}\"")),
            (PATH_FIELD.to_string(), path),
            (MODE_FIELD.to_string(), format!("\"{mode}\"")),
        ])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(bytes) = serialized.get(STATE_FIELD).and_then(|s| from_hex(s.trim_matches('"'))) {
            self.set_raw(bytes);
        }
        if let (Some(p), Ok(mut sp)) = (serialized.get(PATH_FIELD).and_then(|s| serde_json::from_str::<String>(s).ok()), self.path.write()) {
            *sp = p;
        }
        if let (Some(m), Ok(mut sm)) = (serialized.get(MODE_FIELD), self.mode.write()) {
            *sm = mode_of(m.trim_matches('"'));
        }
    }
}

impl FontParams {
    pub fn set_raw(&self, b: Vec<u8>) {
        if let Ok(mut r) = self.raw.write() {
            *r = b;
        }
        self.raw_version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn raw_copy(&self) -> Vec<u8> {
        self.raw.read().map(|b| b.clone()).unwrap_or_else(|_| default_state())
    }

    pub fn state(&self) -> State {
        State::parse(&self.raw_copy()).unwrap_or_default()
    }

    /// The mode the voices play in: the chosen one, else the one the stored state's version implies (the 32-bit
    /// player's version 2 → classic, a later version → modern).
    pub fn mode(&self) -> Mode {
        self.mode.read().ok().and_then(|m| *m).unwrap_or_else(|| if self.state().version() <= 2 { Mode::Classic } else { Mode::Modern })
    }

    pub fn set_mode(&self, m: Mode) {
        if let Ok(mut sm) = self.mode.write() {
            *sm = Some(m);
        }
        self.raw_version.fetch_add(1, Ordering::AcqRel);
    }

    /// The font file to open: the given one, else the stored path.
    pub fn font_file(&self) -> String {
        let given = self.path.read().map(|p| p.clone()).unwrap_or_default();
        if given.is_empty() { self.state().path() } else { given }
    }
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn default_state() -> Vec<u8> {
    State::default().to_bytes()
}

/// The plugin's own saved state (nih-plug JSON with the u64 length prefix of its CLAP state) for a channel's saved
/// state `fl` (a new instance's when empty) and the font file to open for it.
pub fn plugin_state_from_fl(fl: &[u8], path: &str) -> Vec<u8> {
    let b = if fl.is_empty() { default_state() } else { fl.to_vec() };
    let path = serde_json::to_string(&serde_json::to_string(path).unwrap_or_else(|_| "\"\"".into())).unwrap_or_default();
    let state = format!(
        "{{\"version\":\"{}\",\"params\":{{}},\"fields\":{{\"{STATE_FIELD}\":\"\\\"{}\\\"\",\"{PATH_FIELD}\":{path},\"{MODE_FIELD}\":\"\\\"\\\"\"}}}}",
        env!("CARGO_PKG_VERSION"),
        hex(&b)
    );
    let mut blob = (state.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(state.as_bytes());
    blob
}

/// The original's saved state in a plugin state blob (see [`plugin_state_from_fl`]); `None` when it is not one.
pub fn fl_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    let (_, json) = os_hostlink::state::json(blob);
    let v: serde_json::Value = serde_json::from_str(std::str::from_utf8(json).ok()?).ok()?;
    from_hex(v.get("fields")?.get(STATE_FIELD)?.as_str()?.trim_matches('"'))
}

/// The font path a saved state stores.
pub fn stored_path(fl: &[u8]) -> String {
    State::parse(fl).map(|s| s.path()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_path_and_mode_survive_the_plugin_state() {
        let mut st = State::default();
        st.set_path("%USERPROFILE%\\a.sf2");
        let st = st.to_bytes();
        let blob = plugin_state_from_fl(&st, "C:\\Users\\x\\a.sf2");
        assert_eq!(fl_from_plugin_state(&blob), Some(st.clone()));
        let json = std::str::from_utf8(&blob[8..]).unwrap();
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let fields: BTreeMap<String, String> = v["fields"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        let p = FontParams::default();
        p.deserialize_fields(&fields);
        assert_eq!(p.raw_copy(), st);
        assert_eq!(p.font_file(), "C:\\Users\\x\\a.sf2");
        assert_eq!(p.mode(), Mode::Classic);
        p.set_mode(Mode::Modern);
        let again = p.serialize_fields();
        let q = FontParams::default();
        q.deserialize_fields(&again);
        assert_eq!(q.mode(), Mode::Modern);
        assert_eq!(stored_path(&st), "%USERPROFILE%\\a.sf2");
    }
}
