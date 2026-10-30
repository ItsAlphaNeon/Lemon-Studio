// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Slicer parameters and saved state.
//!
//! Five parameters: pitch shift, attack, decay, declick, play to end; they are fields of the original's saved state.
//! A copy of the whole saved state (the loop, its slices, every setting) is persisted next to them (`state`): what
//! the parameters do not change comes back out byte for byte, and a stored value outside a parameter's range is kept
//! while the parameter still shows it. The sample file to open is persisted as `sample-path`, as Lemon Sampler's.
//!
// spec: plugins/lemon-slicer.md §1, plugins/fruity-slicer.md §1

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_plugkit::live::LiveEdits;

use crate::state::{State, H_ATTACK, H_DECAY, H_PITCH, T_DECLICK, T_TO_END};

pub const PITCH_ID: &str = "pitch";
pub const ATTACK_ID: &str = "att";
pub const DECAY_ID: &str = "dec";
pub const DECLICK_ID: &str = "declick";
pub const TO_END_ID: &str = "toend";

/// The parameters in the host's order: (id, name).
pub const NAMES: [(&str, &str); 5] =
    [(PITCH_ID, "Pitch shift"), (ATTACK_ID, "Attack"), (DECAY_ID, "Decay"), (DECLICK_ID, "Declick"), (TO_END_ID, "Play to end")];

pub const PITCH_RANGE: (i32, i32) = (-1200, 1200);
pub const FADE_RANGE: (i32, i32) = (0, 1000);

/// The text of a pitch shift (cents).
pub fn pitch_text(v: i32) -> String {
    if v == 0 { "0 cents".into() } else { format!("{v:+} cents") }
}

/// The text of a fade (ms); 0 = off.
pub fn fade_text(v: i32) -> String {
    if v == 0 { "Off".into() } else { format!("{v}ms") }
}

/// A typed number (its unit, if any, ignored), clamped to the range; "off" is 0.
pub fn parse_number(s: &str, (lo, hi): (i32, i32)) -> Option<i32> {
    let t = s.trim().to_lowercase();
    if t == "off" {
        return Some(0.clamp(lo, hi));
    }
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')).collect();
    let x: f64 = num.parse().ok()?;
    Some((x.round() as i32).clamp(lo, hi))
}

/// The parameters' values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Values {
    pub pitch: i32,
    pub attack: i32,
    pub decay: i32,
    pub declick: bool,
    pub to_end: bool,
}

pub struct SlicerParams {
    pub pitch: IntParam,
    pub attack: IntParam,
    pub decay: IntParam,
    pub declick: BoolParam,
    pub to_end: BoolParam,
    /// The original's saved state (the loop, the slices and the settings the parameters do not hold).
    pub raw: RwLock<Vec<u8>>,
    /// Bumped on every `raw` change.
    pub raw_version: AtomicU64,
    /// Editor edits the host has not confirmed yet (the sound hears them at once).
    pub live: Arc<LiveEdits>,
    /// The sample file to open ("" = the state's own path), persisted as `sample-path`.
    pub sample_path: RwLock<String>,
}

impl Default for SlicerParams {
    fn default() -> Self {
        let d = values_of(&default_state());
        let int = |name: &'static str, v: i32, range: (i32, i32), text: fn(i32) -> String| {
            IntParam::new(name, v, IntRange::Linear { min: range.0, max: range.1 })
                .with_value_to_string(Arc::new(move |v| text(v)))
                .with_string_to_value(Arc::new(move |s| parse_number(s, range)))
        };
        SlicerParams {
            pitch: int(NAMES[0].1, d.pitch, PITCH_RANGE, pitch_text),
            attack: int(NAMES[1].1, d.attack, FADE_RANGE, fade_text),
            decay: int(NAMES[2].1, d.decay, FADE_RANGE, fade_text),
            declick: BoolParam::new(NAMES[3].1, d.declick),
            to_end: BoolParam::new(NAMES[4].1, d.to_end),
            raw: RwLock::new(default_state()),
            raw_version: AtomicU64::new(0),
            live: Arc::default(),
            sample_path: RwLock::default(),
        }
    }
}

/// The persisted fields.
pub const STATE_FIELD: &str = "state";
pub const SAMPLE_FIELD: &str = "sample-path";

unsafe impl Params for SlicerParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let ptrs = [self.pitch.as_ptr(), self.attack.as_ptr(), self.decay.as_ptr(), self.declick.as_ptr(), self.to_end.as_ptr()];
        NAMES.iter().zip(ptrs).map(|((id, _), p)| (id.to_string(), p, String::new())).collect()
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let hex = self.raw.read().map(|b| hex(&b)).unwrap_or_default();
        let path = self.sample_path.read().map(|p| serde_json::to_string(&*p).unwrap_or_default()).unwrap_or_default();
        BTreeMap::from([(STATE_FIELD.to_string(), format!("\"{hex}\"")), (SAMPLE_FIELD.to_string(), path)])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(bytes) = serialized.get(STATE_FIELD).and_then(|s| from_hex(s.trim_matches('"'))) {
            self.set_raw(bytes);
        }
        if let (Some(p), Ok(mut sp)) = (serialized.get(SAMPLE_FIELD).and_then(|s| serde_json::from_str::<String>(s).ok()), self.sample_path.write()) {
            *sp = p;
        }
    }
}

impl SlicerParams {
    /// The values the sound plays (an editor edit the host has not confirmed yet wins).
    pub fn values(&self) -> Values {
        let l = &self.live;
        Values { pitch: l.value(&self.pitch), attack: l.value(&self.attack), decay: l.value(&self.decay), declick: l.value(&self.declick), to_end: l.value(&self.to_end) }
    }

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

    /// The original's saved state of the current settings.
    pub fn fl_state(&self) -> Vec<u8> {
        let v = Values {
            pitch: self.pitch.value(),
            attack: self.attack.value(),
            decay: self.decay.value(),
            declick: self.declick.value(),
            to_end: self.to_end.value(),
        };
        to_fl(&v, &self.raw_copy())
    }
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A new instance's saved state (the original's new channel, 418 bytes).
pub fn default_state() -> Vec<u8> {
    State::default().to_bytes()
}

/// The parameter values of a saved state (stored values clamped to the parameters' ranges).
pub fn values_of(fl: &[u8]) -> Values {
    let st = State::parse(fl).unwrap_or_default();
    Values {
        pitch: st.i32(H_PITCH).clamp(PITCH_RANGE.0, PITCH_RANGE.1),
        attack: st.i32(H_ATTACK).clamp(FADE_RANGE.0, FADE_RANGE.1),
        decay: st.i32(H_DECAY).clamp(FADE_RANGE.0, FADE_RANGE.1),
        declick: st.switch(T_DECLICK),
        to_end: st.switch(T_TO_END),
    }
}

/// The original's saved state `raw` with the parameter values `v`: fields whose stored value still reads as the
/// parameter's keep their bytes. A state that cannot be read comes back as it is.
pub fn to_fl(v: &Values, raw: &[u8]) -> Vec<u8> {
    let Some(mut st) = State::parse(raw) else { return raw.to_vec() };
    let now = values_of(raw);
    for (at, want, had) in [(H_PITCH, v.pitch, now.pitch), (H_ATTACK, v.attack, now.attack), (H_DECAY, v.decay, now.decay)] {
        if want != had {
            st.set_i32(at, want);
        }
    }
    for (at, want, had) in [(T_DECLICK, v.declick, now.declick), (T_TO_END, v.to_end, now.to_end)] {
        if want != had {
            st.set_switch(at, want);
        }
    }
    st.to_bytes()
}

// ---------------------------------------------------------------------------------------------------------------
// Plugin state <-> the original's saved state (Lemon Studio keeps a project channel's original state, event 213)

/// The plugin's own saved state (nih-plug JSON with the u64 length prefix of its CLAP state) for a channel's saved
/// state `fl` (a new instance's when `fl` is empty).
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let b = if fl.is_empty() { default_state() } else { fl.to_vec() };
    let v = values_of(&b);
    let params = [
        format!("\"{PITCH_ID}\":{{\"i32\":{}}}", v.pitch),
        format!("\"{ATTACK_ID}\":{{\"i32\":{}}}", v.attack),
        format!("\"{DECAY_ID}\":{{\"i32\":{}}}", v.decay),
        format!("\"{DECLICK_ID}\":{{\"bool\":{}}}", v.declick),
        format!("\"{TO_END_ID}\":{{\"bool\":{}}}", v.to_end),
    ];
    let state = format!(
        "{{\"version\":\"{}\",\"params\":{{{}}},\"fields\":{{\"{STATE_FIELD}\":\"\\\"{}\\\"\"}}}}",
        env!("CARGO_PKG_VERSION"),
        params.join(","),
        hex(&b)
    );
    let mut blob = (state.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(state.as_bytes());
    blob
}

/// The original's saved state for a plugin state blob (see [`plugin_state_from_fl`]); `None` when it is not one.
pub fn fl_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    let json = if blob.len() > 8 && blob[0] != b'{' { &blob[8..] } else { blob };
    let v: serde_json::Value = serde_json::from_str(std::str::from_utf8(json).ok()?).ok()?;
    let raw = from_hex(v.get("fields")?.get(STATE_FIELD)?.as_str()?.trim_matches('"'))?;
    let params = v.get("params")?;
    let mut vals = values_of(&raw);
    let int = |id: &str| params.get(id).and_then(|p| p.get("i32")).and_then(|x| x.as_i64()).map(|x| x as i32);
    let flag = |id: &str| params.get(id).and_then(|p| p.get("bool")).and_then(|x| x.as_bool());
    vals.pitch = int(PITCH_ID).unwrap_or(vals.pitch);
    vals.attack = int(ATTACK_ID).unwrap_or(vals.attack);
    vals.decay = int(DECAY_ID).unwrap_or(vals.decay);
    vals.declick = flag(DECLICK_ID).unwrap_or(vals.declick);
    vals.to_end = flag(TO_END_ID).unwrap_or(vals.to_end);
    Some(to_fl(&vals, &raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Slice, H_BEATS, H_BPM};

    fn loop_state() -> Vec<u8> {
        let mut s = State::default();
        s.set_f32(H_BEATS, 8.0);
        s.set_f32(H_BPM, 60.0);
        s.set_i32(H_PITCH, 5000);
        s.set_path("%FLStudioFactoryData%\\Data\\Patches\\Packs\\Loops\\DL Breaker.wav");
        s.slices = vec![Slice { name: b"Kick".to_vec(), ..Slice::at(0) }, Slice::at(22050)];
        s.to_bytes()
    }

    #[test]
    fn texts_and_typed_values() {
        assert_eq!((pitch_text(0), pitch_text(100), pitch_text(-250)), ("0 cents".into(), "+100 cents".into(), "-250 cents".into()));
        assert_eq!((fade_text(0), fade_text(15)), ("Off".into(), "15ms".into()));
        assert_eq!(parse_number("15ms", FADE_RANGE), Some(15));
        assert_eq!(parse_number("-3000", PITCH_RANGE), Some(-1200));
        assert_eq!(parse_number("off", FADE_RANGE), Some(0));
        assert_eq!(parse_number("x", FADE_RANGE), None);
    }

    #[test]
    fn a_new_instance_holds_the_originals_defaults() {
        let v = values_of(&default_state());
        assert_eq!(v, Values { pitch: 0, attack: 0, decay: 4, declick: true, to_end: false });
        let p = SlicerParams::default();
        assert_eq!(p.fl_state(), default_state());
        assert_eq!(p.param_map().len(), 5);
    }

    /// The state comes back byte for byte through the plugin state, a stored value out of range included; an edited
    /// parameter changes only its field.
    #[test]
    fn state_round_trips_through_the_plugin_state() {
        let fl = loop_state();
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(fl_from_plugin_state(&blob), Some(fl.clone()));
        assert_eq!(values_of(&fl).pitch, 1200);
        let mut v = values_of(&fl);
        v.decay = 30;
        v.to_end = true;
        let out = to_fl(&v, &fl);
        let st = State::parse(&out).unwrap();
        assert_eq!((st.i32(H_DECAY), st.i32(H_PITCH), st.switch(T_TO_END)), (30, 5000, true));
        assert_eq!(out.len(), fl.len());
        // An empty channel state is a new instance.
        assert_eq!(fl_from_plugin_state(&plugin_state_from_fl(&[])), Some(default_state()));
        // Bytes that are not a state are kept as they are.
        assert_eq!(fl_from_plugin_state(&plugin_state_from_fl(&[1, 2, 3])), Some(vec![1, 2, 3]));
    }

    #[test]
    fn fields_persist_the_state_and_the_sample() {
        let p = SlicerParams::default();
        p.set_raw(loop_state());
        *p.sample_path.write().unwrap() = "C:\\Loops\\DL Breaker.wav".into();
        let f = p.serialize_fields();
        let q = SlicerParams::default();
        q.deserialize_fields(&f);
        assert_eq!(q.raw_copy(), loop_state());
        assert_eq!(*q.sample_path.read().unwrap(), "C:\\Loops\\DL Breaker.wav");
        assert_eq!(q.state().slices.len(), 2);
    }
}
