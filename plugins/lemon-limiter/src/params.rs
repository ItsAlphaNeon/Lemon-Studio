// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Limiter parameters and saved state.
//!
//! The 18 parameters come in the original's order (parameter k = field k + 1 of its saved state = automation index k)
//! with the original's stored ranges. A copy of the whole 169-byte saved state is persisted next to them (`state`): the
//! spare bank, the editor bytes, the sidechain number and the flags live there, and bytes Lemon Studio does not
//! understand come back out unchanged.
//!
// spec: plugins/lemon-limiter.md §4 (value texts), §6 (parameters and state); plugins/fruity-limiter-dsp.md (layout)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_engine::effects::dynamics::{self as dsp, DEFAULTS, PARAMS, RANGES};
use os_plugkit::live::LiveEdits;

/// Saved state size and version (field 0).
pub const STATE_LEN: usize = 169;
pub const STATE_VERSION: i32 = 7;
/// The spare bank: 18 int32 from this byte (Store / Flip).
pub const SPARE_AT: usize = 76;
/// Editor bytes 148..163 (Lemon Studio's use: page, hidden display layers, scrolling speed).
pub const PAGE_BYTE: usize = 148;
pub const LAYERS_BYTE: usize = 149;
pub const SPEED_BYTE: usize = 150;
/// The sidechain input number (0 = none).
pub const SIDECHAIN_BYTE: usize = 164;
pub const SIDECHAIN_MAX: u8 = 125;
/// Flags: bit 0 = the limiter aims at the ceiling itself (the "0.2 dB limiter safety" menu tick cleared).
pub const FLAGS_BYTE: usize = 168;

/// How a parameter's value reads (spec §4.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Text {
    /// A level knob with base B: `<dB>dB  <amplitude>`.
    Level(f64),
    Ms,
    /// A release: shown as twice its time.
    MsDoubled,
    Curve,
    Ratio,
    Knee,
}

/// One parameter: id, name, text.
#[derive(Clone, Debug)]
pub struct Def {
    pub id: &'static str,
    pub name: &'static str,
    pub text: Text,
}

pub const DEFS: [Def; PARAMS] = [
    Def { id: "gain", name: "Gain", text: Text::Level(7.0) },
    Def { id: "sat", name: "Soft saturation threshold", text: Text::Level(3.0) },
    Def { id: "ceil", name: "Limiter ceiling", text: Text::Level(3.0) },
    Def { id: "att", name: "Limiter attack time", text: Text::Ms },
    Def { id: "att_curve", name: "Limiter attack curve", text: Text::Curve },
    Def { id: "rel", name: "Limiter release time", text: Text::MsDoubled },
    Def { id: "rel_curve", name: "Limiter release curve", text: Text::Curve },
    Def { id: "sustain", name: "Limiter peak window", text: Text::Ms },
    Def { id: "c_thres", name: "Comp threshold", text: Text::Level(7.0) },
    Def { id: "c_ratio", name: "Comp ratio", text: Text::Ratio },
    Def { id: "c_knee", name: "Comp knee", text: Text::Knee },
    Def { id: "c_att", name: "Comp attack time", text: Text::Ms },
    Def { id: "c_rel", name: "Comp release time", text: Text::MsDoubled },
    Def { id: "c_curve", name: "Comp curve", text: Text::Curve },
    Def { id: "c_rms", name: "Comp RMS window", text: Text::Ms },
    Def { id: "g_gain", name: "Noise gain", text: Text::Level(7.0) },
    Def { id: "g_thres", name: "Noise threshold", text: Text::Level(7.0) },
    Def { id: "g_rel", name: "Noise release time", text: Text::MsDoubled },
];

/// Parameter index of an id.
pub fn index(id: &str) -> Option<usize> {
    DEFS.iter().position(|d| d.id == id)
}

/// The number a value text shows (dB for levels, −∞ as `NEG_INFINITY`; ms; the ratio's X; %).
fn number(t: Text, v: i32) -> f64 {
    match t {
        Text::Level(b) => 20.0 * f64::from(dsp::level(v, b)).log10(),
        Text::Ms => f64::from(dsp::time_ms(v)),
        Text::MsDoubled => 2.0 * f64::from(dsp::time_ms(v)),
        Text::Curve => f64::from(v),
        Text::Ratio => {
            let c = 51f64.powf(f64::from(v.unsigned_abs()) / 1000.0) - 1.0;
            if v > 0 { 0.38 * c + 1.0 } else { -(0.06 * c + 1.0) }
        }
        Text::Knee => (f64::from(v) / 10.0).round(),
    }
}

/// A parameter's value text (spec §4).
pub fn value_text(t: Text, v: i32) -> String {
    match t {
        Text::Level(b) => {
            let a = dsp::level(v, b);
            if a <= 0.0 { "-INFdB  0.00".into() } else { format!("{:.1}dB  {:.2}", 20.0 * f64::from(a).log10(), a) }
        }
        Text::Ms | Text::MsDoubled => format!("{:.2}ms", number(t, v)),
        Text::Curve => v.to_string(),
        Text::Ratio => {
            let x = number(t, v);
            if v > 0 { format!("{x:.1}:1") } else { format!("1:{:.1}", -x) }
        }
        Text::Knee => format!("{}%", number(t, v)),
    }
}

/// A typed value: the stored value whose text shows the nearest number (dB, ms, X of `X:1` / `1:X`, %).
pub fn parse_text(t: Text, s: &str, min: i32, max: i32) -> Option<i32> {
    let s = s.trim().to_ascii_lowercase();
    if matches!(t, Text::Level(_)) && (s.contains("inf") || s == "off") {
        return Some(min);
    }
    let lead: String = s.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let mut x: f64 = lead.parse().ok()?;
    if t == Text::Ratio {
        // "1:4" = expansion, "4:1" (or "4") = compression.
        if let Some(rest) = s.strip_prefix("1:") {
            x = -rest.trim().parse::<f64>().ok()?;
        }
    }
    if t == Text::Curve {
        return Some((x.round() as i32).clamp(min, max));
    }
    (min..=max).min_by(|&a, &b| (number(t, a) - x).abs().total_cmp(&(number(t, b) - x).abs()))
}

pub struct LimiterParams {
    /// The parameters in the original's order.
    pub p: Vec<IntParam>,
    /// The original's saved state the parameters came from.
    pub raw: RwLock<Vec<u8>>,
    /// Bumped on every `raw` change.
    pub raw_version: AtomicU64,
    /// Editor edits the host has not confirmed yet (the sound hears them at once).
    pub live: Arc<LiveEdits>,
}

impl Default for LimiterParams {
    fn default() -> Self {
        let p = DEFS
            .iter()
            .enumerate()
            .map(|(k, d)| {
                let (text, (min, max)) = (d.text, RANGES[k]);
                IntParam::new(d.name, DEFAULTS[k], IntRange::Linear { min, max })
                    .with_value_to_string(Arc::new(move |v| value_text(text, v)))
                    .with_string_to_value(Arc::new(move |s| parse_text(text, s, min, max)))
            })
            .collect();
        LimiterParams { p, raw: RwLock::new(default_state()), raw_version: AtomicU64::new(0), live: Arc::default() }
    }
}

/// The persisted field holding `raw`.
pub const STATE_FIELD: &str = "state";

unsafe impl Params for LimiterParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        self.p.iter().zip(&DEFS).map(|(p, d)| (d.id.to_string(), p.as_ptr(), String::new())).collect()
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

impl LimiterParams {
    /// Parameter `k`'s value for the sound (an editor edit the host has not confirmed yet wins).
    pub fn v(&self, k: usize) -> i32 {
        self.live.value(&self.p[k])
    }

    pub fn values(&self) -> [i32; PARAMS] {
        std::array::from_fn(|k| self.v(k))
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

    /// Changes the state image (editor settings, spare bank, sidechain, flags).
    pub fn edit_raw(&self, f: impl FnOnce(&mut Vec<u8>)) {
        let mut b = self.raw_copy();
        f(&mut b);
        self.set_raw(b);
    }

    /// The original's saved state of the current settings.
    pub fn fl_state(&self) -> Vec<u8> {
        let vals: Vec<i32> = self.p.iter().map(|p| p.value()).collect();
        to_fl(&vals, &self.raw_copy())
    }
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A state of the full size (shorter ones are completed with the defaults).
fn normalized(mut b: Vec<u8>) -> Vec<u8> {
    let d = default_state();
    if b.len() < STATE_LEN {
        b.extend_from_slice(&d[b.len()..]);
    }
    b
}

/// Little-endian int32 field `k` (field 1..18 = the parameters).
pub fn field(b: &[u8], k: usize) -> i32 {
    int_at(b, 4 * k)
}

pub fn set_field(b: &mut [u8], k: usize, v: i32) {
    set_int_at(b, 4 * k, v);
}

pub fn int_at(b: &[u8], at: usize) -> i32 {
    b.get(at..at + 4).map_or(0, |s| i32::from_le_bytes(s.try_into().unwrap()))
}

pub fn set_int_at(b: &mut [u8], at: usize, v: i32) {
    if let Some(s) = b.get_mut(at..at + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

pub fn byte(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

/// The spare bank's 18 values.
pub fn spare(b: &[u8]) -> [i32; PARAMS] {
    std::array::from_fn(|k| int_at(b, SPARE_AT + 4 * k))
}

pub fn set_spare(b: &mut [u8], v: &[i32; PARAMS]) {
    for (k, x) in v.iter().enumerate() {
        set_int_at(b, SPARE_AT + 4 * k, *x);
    }
}

/// A fresh instance's saved state: FL's Default preset, the spare bank the same, no sidechain, flags 0 (spec §6).
pub fn default_state() -> Vec<u8> {
    let mut b = vec![0u8; STATE_LEN];
    set_field(&mut b, 0, STATE_VERSION);
    for (k, v) in DEFAULTS.iter().enumerate() {
        set_field(&mut b, k + 1, *v);
    }
    set_spare(&mut b, &DEFAULTS);
    b
}

/// Parameter `k`'s value for a stored field value.
pub fn decode(k: usize, s: i32) -> i32 {
    s.clamp(RANGES[k].0, RANGES[k].1)
}

/// The original's saved state with parameter values `vals`: fields whose value still means the same keep their stored
/// bytes (out-of-range values), the rest are written from the parameters.
pub fn to_fl(vals: &[i32], raw: &[u8]) -> Vec<u8> {
    let mut b = normalized(raw.to_vec());
    if field(&b, 0) == 0 {
        set_field(&mut b, 0, STATE_VERSION);
    }
    for (k, &v) in vals.iter().enumerate().take(PARAMS) {
        if decode(k, field(&b, k + 1)) != v {
            set_field(&mut b, k + 1, v);
        }
    }
    b
}

/// Parameter values of a saved state.
pub fn values_of(fl: &[u8]) -> Vec<i32> {
    (0..PARAMS).map(|k| decode(k, field(fl, k + 1))).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Plugin state <-> the original's saved state

/// The plugin's own saved state (nih-plug JSON with the u64 length prefix of its CLAP state) for a slot's saved state.
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let b = normalized(fl.to_vec());
    let params: Vec<String> = values_of(&b).iter().zip(&DEFS).map(|(v, d)| format!("\"{}\":{{\"i32\":{v}}}", d.id)).collect();
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
    Some(to_fl(&vals, &raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_match_the_original_hints() {
        let txt = |k: usize, v: i32| value_text(DEFS[k].text, v);
        assert_eq!(txt(0, 1000), "0.0dB  1.00");
        assert_eq!(txt(16, 0), "-INFdB  0.00");
        assert_eq!(txt(8, 600), "-8.7dB  0.37");
        assert_eq!(txt(3, 1115), "2.00ms");
        assert_eq!(txt(5, 5000), "85.53ms");
        assert_eq!(txt(7, 2882), "10.00ms");
        assert_eq!(txt(12, 6666), "248.22ms");
        assert_eq!(txt(14, 650), "1.00ms");
        assert_eq!(txt(17, 4187), "50.01ms");
        assert_eq!(txt(9, 0), "1:1.0");
        assert_eq!(txt(9, 1000), "20.0:1");
        assert_eq!(txt(9, -1000), "1:4.0");
        assert_eq!(txt(10, 400), "40%");
        assert_eq!(txt(10, -720), "-72%");
        assert_eq!(txt(13, 1), "1");
    }

    #[test]
    fn typed_values_find_the_nearest_setting() {
        let parse = |k: usize, s: &str| parse_text(DEFS[k].text, s, RANGES[k].0, RANGES[k].1);
        assert_eq!(parse(0, "0"), Some(1000));
        assert_eq!(parse(0, "0dB"), Some(1000));
        assert_eq!(parse(16, "-inf"), Some(0));
        assert_eq!(parse(9, "20:1"), Some(1000));
        assert_eq!(parse(9, "1:4"), Some(-1000));
        assert_eq!(parse(4, "12"), Some(8));
        let v = parse(3, "5ms").unwrap();
        assert!((dsp::time_ms(v) - 5.0).abs() < 0.01);
    }

    #[test]
    fn fresh_state_is_the_default_preset() {
        let b = default_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(field(&b, 0), 7);
        assert_eq!(values_of(&b), DEFAULTS.to_vec());
        assert_eq!(spare(&b), DEFAULTS);
        assert_eq!((byte(&b, SIDECHAIN_BYTE), byte(&b, FLAGS_BYTE)), (0, 0));
        // Host-facing parameters: FL's order, ranges and defaults.
        let p = LimiterParams::default();
        assert_eq!(p.p.len(), PARAMS);
        assert_eq!(p.p[17].name(), "Noise release time");
        assert_eq!(p.values(), DEFAULTS);
    }

    #[test]
    fn state_round_trips_through_the_plugin_state() {
        let mut fl = default_state();
        set_field(&mut fl, 1, 1500);
        set_field(&mut fl, 10, -300);
        set_field(&mut fl, 3, 2600); // out of the knob's range: kept
        set_int_at(&mut fl, SPARE_AT + 8, 777);
        fl[150] = 9;
        fl[SIDECHAIN_BYTE] = 3;
        fl[FLAGS_BYTE] = 1;
        fl[166] = 0x5a; // a byte Lemon Studio does not use
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(fl_from_plugin_state(&blob).expect("parses"), fl);
        let p = LimiterParams::default();
        let json: serde_json::Value = serde_json::from_slice(&blob[8..]).unwrap();
        let fields: BTreeMap<String, String> =
            json["fields"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        p.deserialize_fields(&fields);
        assert_eq!(*p.raw.read().unwrap(), fl);
        // Edited parameters win over the image.
        let mut vals = values_of(&fl);
        vals[0] = 500;
        assert_eq!(field(&to_fl(&vals, &fl), 1), 500);
        assert_eq!(field(&to_fl(&vals, &fl), 3), 2600);
    }
}
