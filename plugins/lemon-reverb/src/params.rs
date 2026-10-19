// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Reverb parameters and saved state.
//!
//! The 15 knobs come in the original's order (knob k = field k + 1 of its saved state) with the original's stored
//! ranges, then the two switches (tempo-based pre-delay, mid/side input). A copy of the whole 66-byte saved state is
//! persisted next to them (`state`): values outside a knob's range keep their stored bytes.
//!
// spec: plugins/lemon-reverb.md §3 (value texts), §5 (parameters and state); plugins/fruity-reeverb-2.md (layout)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_engine::effects::reverb::{Settings, DEFAULTS, LOWPASS_OFF, LOW_CUT_OFF, MID_SIDE_BYTE, PARAMS, RANGES, STATE_LEN, TEMPO_BYTE, VERSION};
use os_plugkit::live::LiveEdits;

/// How a knob's value reads (spec §3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Text {
    /// `<v>Hz`, Off at the bottom (low cut).
    LowCut,
    /// `<v/10>kHz` with one decimal, Off at the top (high cut, high damping).
    KHz,
    /// `<ms>ms`, or `<steps>:<ticks>` when tempo-based.
    Predelay,
    /// The number itself.
    Plain,
    /// `<v/10>sec` with one decimal.
    Seconds,
    /// `<v>%`.
    Percent,
    /// `<v>Hz`.
    Hz,
    /// Original / `<n>% separated` / `<n>% merged`.
    Separation,
    /// 128 = 100 %.
    Level,
    /// 2048 = 100 %.
    Mod,
}

/// One knob: id, name, text.
#[derive(Clone, Debug)]
pub struct Def {
    pub id: &'static str,
    pub name: &'static str,
    pub text: Text,
}

pub const DEFS: [Def; PARAMS] = [
    Def { id: "lcut", name: "Low cut", text: Text::LowCut },
    Def { id: "hcut", name: "High cut", text: Text::KHz },
    Def { id: "delay", name: "Predelay", text: Text::Predelay },
    Def { id: "size", name: "Room size", text: Text::Plain },
    Def { id: "diff", name: "Diffusion", text: Text::Plain },
    Def { id: "dec", name: "Decay time", text: Text::Seconds },
    Def { id: "damp", name: "High damping", text: Text::KHz },
    Def { id: "bass", name: "Bass multiplier", text: Text::Percent },
    Def { id: "cross", name: "Crossover", text: Text::Hz },
    Def { id: "sep", name: "Stereo separation", text: Text::Separation },
    Def { id: "dry", name: "Dry level", text: Text::Level },
    Def { id: "er", name: "Early reflection level", text: Text::Level },
    Def { id: "wet", name: "Wet level", text: Text::Level },
    Def { id: "speed", name: "Mod Speed", text: Text::Mod },
    Def { id: "mod", name: "Mod Depth", text: Text::Mod },
];

/// The switches' ids and names.
pub const TEMPO_ID: &str = "tempo";
pub const TEMPO_NAME: &str = "Tempo-based predelay";
pub const MID_SIDE_ID: &str = "midside";
pub const MID_SIDE_NAME: &str = "Mid / side input";

/// Knob index of an id.
pub fn index(id: &str) -> Option<usize> {
    DEFS.iter().position(|d| d.id == id)
}

/// A level or modulation percentage as the original shows it: halves round up (16 / 128 = 12.5 -> 13%).
fn pct(v: i32, full: i32) -> i64 {
    (f64::from(v) * 100.0 / f64::from(full)).round() as i64
}

/// The separation's percentage: here the original rounds halves to even (8 / 64 = 12.5 -> 12%, 24 -> 38%).
fn sep_pct(v: i32) -> i64 {
    (f64::from(v.abs()) * 100.0 / 64.0).round_ties_even() as i64
}

/// The pre-delay's text: milliseconds (1/384 s steps, cut to whole ms), or 16th-note steps and their 48 ticks when
/// tempo-based.
pub fn predelay_text(v: i32, tempo: bool) -> String {
    if tempo {
        format!("{}:{:02}", v.div_euclid(48), v.rem_euclid(48))
    } else {
        format!("{}ms", (i64::from(v) * 1000).div_euclid(384))
    }
}

/// A knob's value text (spec §3). `tempo` = the pre-delay is tempo-based.
pub fn value_text(t: Text, v: i32, tempo: bool) -> String {
    match t {
        Text::LowCut if v <= LOW_CUT_OFF => "Off".into(),
        Text::LowCut | Text::Hz => format!("{v}Hz"),
        Text::KHz if v >= LOWPASS_OFF => "Off".into(),
        Text::KHz => format!("{:.1}kHz", f64::from(v) / 10.0),
        Text::Predelay => predelay_text(v, tempo),
        Text::Plain => v.to_string(),
        Text::Seconds => format!("{:.1}sec", f64::from(v) / 10.0),
        Text::Percent => format!("{v}%"),
        Text::Separation if v == 0 => "Original".into(),
        Text::Separation => format!("{}% {}", sep_pct(v), if v < 0 { "separated" } else { "merged" }),
        Text::Level => format!("{}%", pct(v, 128)),
        Text::Mod => format!("{}%", pct(v, 2048)),
    }
}

/// The number a value text shows (Hz; kHz; ms or steps; the number; seconds; %; signed % with merged positive).
fn number(t: Text, v: i32, tempo: bool) -> f64 {
    match t {
        Text::KHz => f64::from(v) / 10.0,
        Text::Predelay if tempo => f64::from(v) / 48.0,
        Text::Predelay => (i64::from(v) * 1000).div_euclid(384) as f64,
        Text::Seconds => f64::from(v) / 10.0,
        Text::Separation => f64::from(v) * 100.0 / 64.0,
        Text::Level => f64::from(v) * 100.0 / 128.0,
        Text::Mod => f64::from(v) * 100.0 / 2048.0,
        Text::LowCut | Text::Hz | Text::Plain | Text::Percent => f64::from(v),
    }
}

/// A typed value: the stored value whose text shows the nearest number. "off" picks a filter's off end, "original"
/// the middle of the separation; "sep" values read as separated, others as merged; a tempo-based pre-delay reads
/// `steps:ticks` or steps.
pub fn parse_text(t: Text, s: &str, min: i32, max: i32, tempo: bool) -> Option<i32> {
    let s = s.trim().to_ascii_lowercase();
    if s == "off" {
        return match t {
            Text::LowCut => Some(min),
            Text::KHz => Some(max),
            _ => None,
        };
    }
    if t == Text::Separation && s.starts_with("orig") {
        return Some(0);
    }
    if t == Text::Predelay && tempo {
        if let Some((a, b)) = s.split_once(':') {
            let (a, b): (i32, i32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
            return Some((a * 48 + b).clamp(min, max));
        }
    }
    let lead: String = s.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let mut x: f64 = lead.parse().ok()?;
    if t == Text::Separation && s.contains("sep") {
        x = -x.abs();
    }
    (min..=max).min_by(|&a, &b| (number(t, a, tempo) - x).abs().total_cmp(&(number(t, b, tempo) - x).abs()))
}

pub struct ReverbParams {
    /// The knobs in the original's order.
    pub p: Vec<IntParam>,
    pub tempo: BoolParam,
    pub mid_side: BoolParam,
    /// The pre-delay text follows the tempo switch.
    pub tempo_shown: Arc<AtomicBool>,
    /// The original's saved state the parameters came from.
    pub raw: RwLock<Vec<u8>>,
    /// Bumped on every `raw` change.
    pub raw_version: AtomicU64,
    /// Editor edits the host has not confirmed yet (the sound hears them at once).
    pub live: Arc<LiveEdits>,
}

impl Default for ReverbParams {
    fn default() -> Self {
        let tempo_shown = Arc::new(AtomicBool::new(false));
        let p = DEFS
            .iter()
            .enumerate()
            .map(|(k, d)| {
                let (text, (min, max)) = (d.text, RANGES[k]);
                let (ta, tb) = (tempo_shown.clone(), tempo_shown.clone());
                IntParam::new(d.name, DEFAULTS[k], IntRange::Linear { min, max })
                    .with_value_to_string(Arc::new(move |v| value_text(text, v, ta.load(Ordering::Relaxed))))
                    .with_string_to_value(Arc::new(move |s| parse_text(text, s, min, max, tb.load(Ordering::Relaxed))))
            })
            .collect();
        let shown = tempo_shown.clone();
        let tempo = BoolParam::new(TEMPO_NAME, false).with_callback(Arc::new(move |b| shown.store(b, Ordering::Relaxed)));
        let mid_side = BoolParam::new(MID_SIDE_NAME, false);
        ReverbParams { p, tempo, mid_side, tempo_shown, raw: RwLock::new(default_state()), raw_version: AtomicU64::new(0), live: Arc::default() }
    }
}

/// The persisted field holding `raw`.
pub const STATE_FIELD: &str = "state";

unsafe impl Params for ReverbParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut m: Vec<_> = self.p.iter().zip(&DEFS).map(|(p, d)| (d.id.to_string(), p.as_ptr(), String::new())).collect();
        m.push((TEMPO_ID.to_string(), self.tempo.as_ptr(), String::new()));
        m.push((MID_SIDE_ID.to_string(), self.mid_side.as_ptr(), String::new()));
        m
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let hex = self.raw.read().map(|b| hex(&b)).unwrap_or_default();
        BTreeMap::from([(STATE_FIELD.to_string(), format!("\"{hex}\""))])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(bytes) = serialized.get(STATE_FIELD).and_then(|s| from_hex(s.trim_matches('"'))) {
            self.set_raw(normalized(bytes));
        }
    }
}

impl ReverbParams {
    /// Knob `k`'s value for the sound (an editor edit the host has not confirmed yet wins).
    pub fn v(&self, k: usize) -> i32 {
        self.live.value(&self.p[k])
    }

    pub fn tempo_sync(&self) -> bool {
        self.live.value(&self.tempo)
    }

    pub fn mid_side(&self) -> bool {
        self.live.value(&self.mid_side)
    }

    /// The settings the sound plays: knobs whose stored value still means the same keep it (values outside a knob's
    /// range are used as they are).
    pub fn settings(&self, raw: &[u8]) -> Settings {
        let knobs = std::array::from_fn(|k| {
            let v = self.v(k);
            let s = field(raw, k + 1);
            if decode(k, s) == v { s } else { v }
        });
        Settings { knobs, tempo_sync: self.tempo_sync(), mid_side: self.mid_side() }
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

    /// The original's saved state of the current settings.
    pub fn fl_state(&self) -> Vec<u8> {
        let vals: Vec<i32> = self.p.iter().map(|p| p.value()).collect();
        to_fl(&vals, self.tempo.value(), self.mid_side.value(), &self.raw_copy())
    }
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Any saved state (66 bytes, version 10000, a factory preset) in the current layout; unreadable ones give the
/// defaults.
pub fn normalized(b: Vec<u8>) -> Vec<u8> {
    match Settings::from_state(&b) {
        Some(_) if b.len() >= STATE_LEN && field(&b, 0) == VERSION => b,
        Some(s) => s.to_state(),
        None => default_state(),
    }
}

/// Little-endian int32 field `k` (field 1..15 = the knobs).
pub fn field(b: &[u8], k: usize) -> i32 {
    b.get(4 * k..4 * k + 4).map_or(0, |s| i32::from_le_bytes(s.try_into().unwrap()))
}

pub fn set_field(b: &mut [u8], k: usize, v: i32) {
    if let Some(s) = b.get_mut(4 * k..4 * k + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

/// A fresh instance's saved state (spec §5).
pub fn default_state() -> Vec<u8> {
    Settings::default().to_state()
}

/// Knob `k`'s value for a stored field value.
pub fn decode(k: usize, s: i32) -> i32 {
    s.clamp(RANGES[k].0, RANGES[k].1)
}

/// The original's saved state with knob values `vals` and the switches: fields whose value still means the same keep
/// their stored bytes (out-of-range values), the rest are written from the parameters.
pub fn to_fl(vals: &[i32], tempo: bool, mid_side: bool, raw: &[u8]) -> Vec<u8> {
    let mut b = normalized(raw.to_vec());
    for (k, &v) in vals.iter().enumerate().take(PARAMS) {
        if decode(k, field(&b, k + 1)) != v {
            set_field(&mut b, k + 1, v);
        }
    }
    b[TEMPO_BYTE] = u8::from(tempo);
    b[MID_SIDE_BYTE] = u8::from(mid_side);
    b
}

/// Knob values of a saved state (in the current layout).
pub fn values_of(fl: &[u8]) -> Vec<i32> {
    (0..PARAMS).map(|k| decode(k, field(fl, k + 1))).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Plugin state <-> the original's saved state

/// The plugin's own saved state (nih-plug JSON with the u64 length prefix of its CLAP state) for a slot's saved state.
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let b = normalized(fl.to_vec());
    let mut params: Vec<String> = values_of(&b).iter().zip(&DEFS).map(|(v, d)| format!("\"{}\":{{\"i32\":{v}}}", d.id)).collect();
    params.push(format!("\"{TEMPO_ID}\":{{\"bool\":{}}}", b[TEMPO_BYTE] != 0));
    params.push(format!("\"{MID_SIDE_ID}\":{{\"bool\":{}}}", b[MID_SIDE_BYTE] != 0));
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
    let raw = normalized(from_hex(v.get("fields")?.get(STATE_FIELD)?.as_str()?.trim_matches('"'))?);
    let params = v.get("params")?;
    let mut vals = values_of(&raw);
    for (k, d) in DEFS.iter().enumerate() {
        if let Some(x) = params.get(d.id).and_then(|p| p.get("i32")).and_then(|x| x.as_i64()) {
            vals[k] = x as i32;
        }
    }
    let flag = |id: &str, at: usize| params.get(id).and_then(|p| p.get("bool")).and_then(|x| x.as_bool()).unwrap_or(raw[at] != 0);
    Some(to_fl(&vals, flag(TEMPO_ID, TEMPO_BYTE), flag(MID_SIDE_ID, MID_SIDE_BYTE), &raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txt(id: &str, v: i32) -> String {
        value_text(DEFS[index(id).unwrap()].text, v, false)
    }

    #[test]
    fn value_texts_match_the_original_hints() {
        // FL's hints (screens of the original, 130 BPM).
        assert_eq!(txt("lcut", 19), "Off");
        assert_eq!(txt("lcut", 75), "75Hz");
        assert_eq!(txt("lcut", 3000), "3000Hz");
        assert_eq!(txt("hcut", 5), "0.5kHz");
        assert_eq!(txt("hcut", 220), "22.0kHz");
        assert_eq!(txt("hcut", 221), "Off");
        assert_eq!(txt("damp", 155), "15.5kHz");
        assert_eq!(txt("delay", 0), "0ms");
        assert_eq!(txt("delay", 1), "2ms");
        assert_eq!(txt("delay", 384), "1000ms");
        assert_eq!(value_text(Text::Predelay, 192, true), "4:00");
        assert_eq!(value_text(Text::Predelay, 3, true), "0:03");
        assert_eq!(value_text(Text::Predelay, 384, true), "8:00");
        assert_eq!(txt("size", 50), "50");
        assert_eq!(txt("dec", 1), "0.1sec");
        assert_eq!(txt("dec", 15), "1.5sec");
        assert_eq!(txt("dec", 200), "20.0sec");
        assert_eq!(txt("bass", 150), "150%");
        assert_eq!(txt("cross", 25), "25Hz");
        assert_eq!(txt("sep", 0), "Original");
        assert_eq!(txt("sep", -64), "100% separated");
        assert_eq!(txt("sep", -1), "2% separated");
        assert_eq!(txt("sep", 48), "75% merged");
        assert_eq!(txt("dry", 128), "100%");
        assert_eq!(txt("dry", 129), "101%");
        assert_eq!(txt("dry", 160), "125%");
        assert_eq!(txt("er", 50), "39%");
        assert_eq!(txt("wet", 25), "20%");
        assert_eq!(txt("wet", 1), "1%");
        assert_eq!(txt("speed", 684), "33%");
        assert_eq!(txt("speed", 100), "5%");
        assert_eq!(txt("mod", 2047), "100%");
        assert_eq!(txt("mod", 4), "0%");
        // Exact halves (screens of the original): levels and modulation round up, the separation to even, the
        // pre-delay's milliseconds are cut.
        assert_eq!(txt("dry", 16), "13%");
        assert_eq!(txt("wet", 80), "63%");
        assert_eq!(txt("wet", 144), "113%");
        assert_eq!(txt("speed", 256), "13%");
        assert_eq!(txt("mod", 1280), "63%");
        assert_eq!(txt("sep", 8), "12% merged");
        assert_eq!(txt("sep", 24), "38% merged");
        assert_eq!(txt("sep", -8), "12% separated");
        assert_eq!(txt("sep", -24), "38% separated");
        assert_eq!(txt("delay", 3), "7ms");
        assert_eq!(txt("delay", 11), "28ms");
    }

    #[test]
    fn typed_values_find_the_nearest_setting() {
        let parse = |id: &str, s: &str| {
            let k = index(id).unwrap();
            parse_text(DEFS[k].text, s, RANGES[k].0, RANGES[k].1, false)
        };
        assert_eq!(parse("lcut", "off"), Some(19));
        assert_eq!(parse("hcut", "Off"), Some(221));
        assert_eq!(parse("hcut", "4kHz"), Some(40));
        assert_eq!(parse("dec", "2.5"), Some(25));
        assert_eq!(parse("sep", "original"), Some(0));
        assert_eq!(parse("sep", "50% separated"), Some(-32));
        assert_eq!(parse("sep", "50"), Some(32));
        assert_eq!(parse("dry", "100%"), Some(128));
        assert_eq!(parse("delay", "1000ms"), Some(384));
        assert_eq!(parse_text(Text::Predelay, "1:24", 0, 384, true), Some(72));
        assert_eq!(parse_text(Text::Predelay, "4", 0, 384, true), Some(192));
    }

    #[test]
    fn fresh_state_is_the_default() {
        let b = default_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(field(&b, 0), VERSION);
        assert_eq!(values_of(&b), DEFAULTS.to_vec());
        let p = ReverbParams::default();
        assert_eq!(p.p.len(), PARAMS);
        assert_eq!(p.p[12].name(), "Wet level");
        assert_eq!(p.settings(&b), Settings::default());
        assert_eq!(p.param_map().len(), PARAMS + 2);
    }

    #[test]
    fn older_states_come_in_the_current_layout() {
        // A factory preset: thirteen knobs and one byte (FL's Cathedral).
        let mut preset = Vec::new();
        for v in [33, 21, 27, 90, 100, 45, 31, 150, 298, 0, 128, 128, 90] {
            preset.extend_from_slice(&i32::to_le_bytes(v));
        }
        preset.push(0);
        let b = normalized(preset);
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(&values_of(&b)[..13], &[33, 21, 27, 90, 100, 45, 31, 150, 298, 0, 128, 128, 90]);
        assert_eq!(values_of(&b)[13..], [684, 0]);
    }

    #[test]
    fn state_round_trips_through_the_plugin_state() {
        let mut fl = default_state();
        set_field(&mut fl, 1, 1500);
        set_field(&mut fl, 10, -30);
        set_field(&mut fl, 6, 0); // decay 0: out of the knob's range, kept
        fl[TEMPO_BYTE] = 1;
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(fl_from_plugin_state(&blob).expect("parses"), fl);
        let p = ReverbParams::default();
        let json: serde_json::Value = serde_json::from_slice(&blob[8..]).unwrap();
        let fields: BTreeMap<String, String> =
            json["fields"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        p.deserialize_fields(&fields);
        assert_eq!(*p.raw.read().unwrap(), fl);
        // The out-of-range decay plays as stored while the knob shows its bottom.
        let s = p.settings(&fl);
        assert_eq!(s.knobs[5], DEFAULTS[5]); // the param itself is still at its default here
        let mut vals = values_of(&fl);
        vals[0] = 500;
        assert_eq!(field(&to_fl(&vals, true, false, &fl), 1), 500);
        assert_eq!(field(&to_fl(&vals, true, false, &fl), 6), 0);
    }
}
