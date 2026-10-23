// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Delay parameters and saved state.
//!
//! The 26 settings come in the original's order (setting k = field k + 1 of its saved state) with the original's
//! stored ranges. A copy of the whole 108-byte saved state is persisted next to them (`state`): values outside a
//! setting's range keep their stored bytes.
//!
// spec: plugins/lemon-delay.md §3 (value texts), §5 (parameters and state); plugins/fruity-delay-3.md (layout)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_engine::effects::delay::{self as dsp, Settings, PARAMS, RANGES, STATE_LEN, VERSION};
use os_plugkit::live::LiveEdits;

/// How a setting's value reads (spec §3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Text {
    /// `<x * 100>%` with one decimal.
    Percent,
    /// `-100.0%` .. `100.0%`.
    Signed,
    /// `<x * 125>%`.
    Feedback,
    /// `<ms>ms`, or `<steps>:<ticks>` when synced.
    Time,
    /// Fake Stereo / True Stereo / Ping Pong / OFF.
    Type,
    /// Off / On.
    OnOff,
    /// LP / HP / BP / OFF.
    Filter,
    /// Limit / Saturate.
    DistType,
    /// `<Hz>Hz` from the filter mode's table.
    Cutoff,
    /// `<Hz> Hz`, `Max Hz` at the top.
    RateRed,
    /// `<bits> bits`, `Max bits` at the top.
    Bits,
    /// `<Hz>Hz`.
    ModRate,
}

/// One setting: id, name, text.
#[derive(Clone, Debug)]
pub struct Def {
    pub id: &'static str,
    pub name: &'static str,
    pub text: Text,
}

pub const DEFS: [Def; PARAMS] = [
    Def { id: "in", name: "Input wet", text: Text::Percent },
    Def { id: "type", name: "Delay type", text: Text::Type },
    Def { id: "sync", name: "Tempo sync", text: Text::OnOff },
    Def { id: "keep", name: "Keep pitch", text: Text::OnOff },
    Def { id: "time", name: "Time", text: Text::Time },
    Def { id: "offset", name: "Delay time R/L offset", text: Text::Signed },
    Def { id: "smooth", name: "Time smooth", text: Text::Percent },
    Def { id: "spread", name: "Stereo spread", text: Text::Percent },
    Def { id: "mrate", name: "Modulation rate", text: Text::ModRate },
    Def { id: "mtime", name: "Modulation time", text: Text::Percent },
    Def { id: "mcut", name: "Modulation cutoff", text: Text::Percent },
    Def { id: "dlevel", name: "Diffusion level", text: Text::Percent },
    Def { id: "dspread", name: "Diffusion spread", text: Text::Percent },
    Def { id: "dtype", name: "Distortion type", text: Text::DistType },
    Def { id: "fb", name: "Feedback level", text: Text::Feedback },
    Def { id: "filter", name: "Filter mode", text: Text::Filter },
    Def { id: "cutoff", name: "Feedback cutoff", text: Text::Cutoff },
    Def { id: "res", name: "Feedback resonance", text: Text::Percent },
    Def { id: "drive", name: "Distortion", text: Text::Percent },
    Def { id: "knee", name: "Distortion knee", text: Text::Percent },
    Def { id: "sym", name: "Distortion symmetry", text: Text::Percent },
    Def { id: "srate", name: "Rate reduction", text: Text::RateRed },
    Def { id: "bits", name: "Bits reduction", text: Text::Bits },
    Def { id: "wet", name: "Output wet", text: Text::Percent },
    Def { id: "tone", name: "Output tone", text: Text::Signed },
    Def { id: "dry", name: "Output dry", text: Text::Percent },
];

/// The R/L offset's name in ping pong, where it pans the input.
pub const PAN_NAME: &str = "Delay time R/L pan";

pub const TYPE_NAMES: [&str; 4] = ["Fake Stereo", "True Stereo", "Ping Pong", "OFF"];
pub const FILTER_NAMES: [&str; 4] = ["LP", "HP", "BP", "OFF"];

/// What a new Lemon Delay starts on: FL's "Default" preset, the one FL loads when the effect is added (True stereo,
/// tempo sync on at 4:0, Saturate, the low pass open, no resonance or diffusion spread). It is not the fresh instance's
/// settings (`dsp::DEFAULTS`), which are where the sound stands before a state arrives (spec §5).
pub const START: [i32; PARAMS] =
    [6000, 1, 1, 0, 192, 3000, 500, 6000, 3000, 0, 0, 0, 0, 1, 3000, 0, 6000, 0, 0, 0, 0, 60000, 6000, 6000, 0, 6000];

/// Setting index of an id.
pub fn index(id: &str) -> Option<usize> {
    DEFS.iter().position(|d| d.id == id)
}

// ---------------------------------------------------------------------------------------------------------------
// Snapping (Lemon Studio choice, spec §4)

/// How near (a share of the range) a drag has to come to a magnet to land on it.
pub const MAGNET_ZONE: f32 = 0.015;

/// A knob's magnet: the R/L offset's and the tone's centre, the feedback's 100 %.
pub fn magnet(k: usize) -> Option<i32> {
    match k {
        dsp::OFFSET => Some(3000),
        dsp::TONE => Some(0),
        dsp::FEEDBACK => Some(4800),
        _ => None,
    }
}

/// A musical synced time (48 per step): a multiple of a quarter step or of a third of one.
pub fn musical(n: i32) -> bool {
    n % 12 == 0 || n % 16 == 0
}

/// Where a drag at the stored value `plain` lands: a synced time on the nearest musical time, a knob with a magnet
/// on it when within `MAGNET_ZONE` of its range; `None` = where the drag is.
pub fn snap(k: usize, plain: f32, sync: bool) -> Option<i32> {
    let (min, max) = dsp::RANGES[k];
    if k == dsp::TIME && sync {
        let r = plain.round() as i32;
        return (0..=16).flat_map(|d| [r - d, r + d]).find(|&n| (12..=max).contains(&n) && musical(n));
    }
    let m = magnet(k)?;
    ((plain - m as f32).abs() <= MAGNET_ZONE * (max - min) as f32).then_some(m)
}

/// The wheel's next stored value from `v` (`up` or down) for a setting that snaps, given the plain step `next`: a
/// synced time goes to the next musical time, a knob with a magnet stops on it when the step would pass it.
pub fn snap_step(k: usize, v: i32, next: i32, up: bool, sync: bool) -> i32 {
    let (min, max) = dsp::RANGES[k];
    if k == dsp::TIME && sync {
        let mut n = v;
        loop {
            n += if up { 1 } else { -1 };
            if !(min..=max).contains(&n) {
                return v;
            }
            if n >= 12 && musical(n) {
                return n;
            }
        }
    }
    match magnet(k) {
        Some(m) if (v < m && m <= next) || (next <= m && m < v) => m,
        _ => next,
    }
}

/// What other settings a value text depends on: the time's sync and the cutoff's filter mode.
#[derive(Debug, Default)]
pub struct Shown {
    pub sync: AtomicBool,
    pub filter: AtomicI32,
}

impl Shown {
    fn get(&self) -> (bool, i32) {
        (self.sync.load(Ordering::Relaxed), self.filter.load(Ordering::Relaxed))
    }
}

/// The cutoff's corner in Hz for the filter mode (off reads as low pass).
pub fn cutoff_hz(x: f32, filter: i32) -> f32 {
    match filter {
        dsp::HIGH_PASS => dsp::lookup(&dsp::HP, x * 3.999),
        dsp::BAND_PASS => dsp::lookup(&dsp::LP, x * 3.6),
        _ => dsp::lookup(&dsp::LP, x * 3.999),
    }
}

/// The rate reduction's hold rate in Hz as the original shows it: the hold period's law on `1 - x` (the sound uses
/// its cube), at 44.1 kHz.
pub fn hold_hz(x: f32) -> f32 {
    44100.0 / dsp::hold_period((1.0 - f64::from(x)) as f32)
}

/// The modulation rate in Hz as the original shows it (the sound's speed is 0.99 of it).
pub fn lfo_hz(x: f32) -> f32 {
    1.0 / dsp::lfo_period(x)
}

/// The number a value text shows (ms or steps for the time, Hz, bits, %); `None` for named values and the tops.
fn number(k: usize, v: i32, sync: bool, filter: i32) -> Option<f64> {
    let x = dsp::norm(k, v);
    Some(match DEFS[k].text {
        Text::Percent => f64::from(x * 100.0),
        Text::Signed => f64::from((x - 0.5) * 200.0),
        Text::Feedback => f64::from(x * 125.0),
        Text::Time if sync => f64::from(dsp::sync_steps(x)) / 48.0,
        Text::Time => dsp::delay_seconds(x, false, 120.0) * 1000.0,
        Text::Cutoff => f64::from(cutoff_hz(x, filter)),
        Text::RateRed if x < 1.0 => f64::from(hold_hz(x)),
        Text::Bits if x < 1.0 => f64::from(dsp::bits(x)),
        Text::ModRate => f64::from(lfo_hz(x)),
        _ => return None,
    })
}

/// A setting's value text (spec §3). `sync` and `filter` are the shown tempo sync and filter mode.
pub fn value_text(k: usize, v: i32, sync: bool, filter: i32) -> String {
    let x = dsp::norm(k, v);
    let pick = |names: &[&str; 4]| names[dsp::four_way(x).clamp(0, 3) as usize].to_string();
    match DEFS[k].text {
        Text::Time if sync => {
            let n = dsp::sync_steps(x);
            format!("{}:{}", n / 48, n % 48)
        }
        Text::Time => format!("{:.1}ms", dsp::delay_seconds(x, false, 120.0) * 1000.0),
        Text::Type => pick(&TYPE_NAMES),
        Text::Filter => pick(&FILTER_NAMES),
        Text::OnOff => if x >= 0.5 { "On" } else { "Off" }.into(),
        Text::DistType => if (x + 0.5).trunc() != 0.0 { "Saturate" } else { "Limit" }.into(),
        Text::RateRed if x >= 1.0 => "Max Hz".into(),
        Text::RateRed => format!("{:.1} Hz", f64::from(hold_hz(x))),
        Text::Bits if x >= 1.0 => "Max bits".into(),
        Text::Bits => format!("{:.1} bits", f64::from(dsp::bits(x))),
        Text::Cutoff | Text::ModRate => format!("{:.1}Hz", number(k, v, sync, filter).unwrap_or(0.0)),
        Text::Percent | Text::Signed | Text::Feedback => format!("{:.1}%", number(k, v, sync, filter).unwrap_or(0.0)),
    }
}

/// A typed value: names pick their setting, `max` the top of the rate and bit reductions, `steps:ticks` a synced
/// time; a number gives the stored value whose text shows the nearest number.
pub fn parse_text(k: usize, s: &str, sync: bool, filter: i32) -> Option<i32> {
    let s = s.trim().to_ascii_lowercase();
    let (min, max) = RANGES[k];
    let named = |names: &[&str]| names.iter().position(|n| s.starts_with(n)).map(|i| i as i32);
    match DEFS[k].text {
        Text::Type => return named(&["fake", "true", "ping", "off"]).or_else(|| named(&["mono", "stereo"])),
        Text::Filter => return named(&["lp", "hp", "bp", "off"]),
        Text::DistType => return named(&["limit", "sat"]),
        Text::OnOff => return named(&["off", "on"]),
        Text::RateRed | Text::Bits if s.starts_with("max") => return Some(max),
        Text::Time if sync => {
            if let Some((a, b)) = s.split_once(':') {
                let (a, b): (i32, i32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
                return Some((a * 48 + b).clamp(min, max));
            }
        }
        _ => {}
    }
    let lead: String = s.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let x: f64 = lead.parse().ok()?;
    (min..=max)
        .filter_map(|v| number(k, v, sync, filter).map(|n| (v, (n - x).abs())))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(v, _)| v)
}

pub struct DelayParams {
    /// The settings in the original's order.
    pub p: Vec<IntParam>,
    /// The time's text follows the sync, the cutoff's the filter mode.
    pub shown: Arc<Shown>,
    /// The original's saved state the parameters came from.
    pub raw: RwLock<Vec<u8>>,
    /// Bumped on every `raw` change.
    pub raw_version: AtomicU64,
    /// Editor edits the host has not confirmed yet (the sound hears them at once).
    pub live: Arc<LiveEdits>,
}

impl Default for DelayParams {
    fn default() -> Self {
        let shown = Arc::new(Shown { sync: AtomicBool::new(START[dsp::SYNC] != 0), filter: AtomicI32::new(START[dsp::FILTER]) });
        let p = DEFS
            .iter()
            .enumerate()
            .map(|(k, d)| {
                let (min, max) = RANGES[k];
                let (a, b) = (shown.clone(), shown.clone());
                let mut param = IntParam::new(d.name, START[k], IntRange::Linear { min, max })
                    .with_value_to_string(Arc::new(move |v| {
                        let (sync, filter) = a.get();
                        value_text(k, v, sync, filter)
                    }))
                    .with_string_to_value(Arc::new(move |s| {
                        let (sync, filter) = b.get();
                        parse_text(k, s, sync, filter)
                    }));
                let sh = shown.clone();
                if k == dsp::SYNC {
                    param = param.with_callback(Arc::new(move |v| sh.sync.store(v != 0, Ordering::Relaxed)));
                } else if k == dsp::FILTER {
                    param = param.with_callback(Arc::new(move |v| sh.filter.store(v, Ordering::Relaxed)));
                }
                param
            })
            .collect();
        DelayParams { p, shown, raw: RwLock::new(default_state()), raw_version: AtomicU64::new(0), live: Arc::default() }
    }
}

/// The persisted field holding `raw`.
pub const STATE_FIELD: &str = "state";

unsafe impl Params for DelayParams {
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

impl DelayParams {
    /// Setting `k`'s value for the sound (an editor edit the host has not confirmed yet wins).
    pub fn v(&self, k: usize) -> i32 {
        self.live.value(&self.p[k])
    }

    /// The settings the sound plays: settings whose stored value still means the same keep it (values outside a
    /// setting's range are used as they are).
    pub fn settings(&self, raw: &[u8]) -> Settings {
        let v = std::array::from_fn(|k| {
            let v = self.v(k);
            let s = field(raw, k + 1);
            if decode(k, s) == v { s } else { v }
        });
        Settings { v }
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

/// Any readable saved state (version 0 or 1) in the version-1 layout; unreadable ones give the start (`START`).
pub fn normalized(b: Vec<u8>) -> Vec<u8> {
    match Settings::from_state(&b) {
        Some(_) if b.len() >= STATE_LEN && field(&b, 0) == VERSION => b,
        Some(s) => s.to_state(),
        None => default_state(),
    }
}

/// Little-endian int32 field `k` (field 1..26 = the settings).
pub fn field(b: &[u8], k: usize) -> i32 {
    b.get(4 * k..4 * k + 4).map_or(0, |s| i32::from_le_bytes(s.try_into().unwrap()))
}

pub fn set_field(b: &mut [u8], k: usize, v: i32) {
    if let Some(s) = b.get_mut(4 * k..4 * k + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

/// A new Lemon Delay's saved state (`START`).
pub fn default_state() -> Vec<u8> {
    Settings { v: START }.to_state()
}

/// Setting `k`'s parameter value for a stored field value.
pub fn decode(k: usize, s: i32) -> i32 {
    s.clamp(RANGES[k].0, RANGES[k].1)
}

/// The original's saved state with values `vals`: fields whose value still means the same keep their stored bytes
/// (out-of-range values), the rest are written from the parameters.
pub fn to_fl(vals: &[i32], raw: &[u8]) -> Vec<u8> {
    let mut b = normalized(raw.to_vec());
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
    use os_engine::effects::delay::{FILTER, LOW_PASS};

    fn txt(id: &str, v: i32) -> String {
        value_text(index(id).unwrap(), v, false, LOW_PASS)
    }

    /// The original's hint texts for generated states (spec §3), at 44.1 kHz.
    #[test]
    fn value_texts_match_the_original_hints() {
        let k = |id: &str| index(id).unwrap();
        let t = |id: &str, v: i32, sync: bool, filter: i32| value_text(k(id), v, sync, filter);
        // Time: ms with one decimal; synced, steps and ticks without padding.
        for (v, s) in [(80, "4.6ms"), (768, "1000.0ms"), (767, "996.3ms"), (400, "166.1ms"), (3, "1.0ms"), (100, "6.7ms"), (300, "78.3ms")] {
            assert_eq!(txt("time", v), s);
        }
        for (v, s) in [(192, "4:0"), (1, "0:1"), (37, "0:37"), (49, "1:1"), (95, "1:47"), (768, "16:0")] {
            assert_eq!(t("time", v, true, LOW_PASS), s);
        }
        // Percentages from the single normalised value; exact halves as FL rounds them.
        for (id, v, s) in [
            ("in", 1, "0.0%"),
            ("in", 3, "0.1%"),
            ("in", 6000, "100.0%"),
            ("spread", 9, "0.2%"),
            ("spread", 15, "0.2%"),
            ("mtime", 15, "0.2%"),
            ("smooth", 1, "0.1%"),
            ("smooth", 995, "99.5%"),
            ("smooth", 500, "50.0%"),
            ("fb", 3000, "62.5%"),
            ("fb", 6000, "125.0%"),
            ("fb", 3, "0.1%"),
            ("fb", 6, "0.1%"),
            ("fb", 9, "0.2%"),
            ("fb", 12, "0.2%"),
            ("fb", 18, "0.4%"),
            ("fb", 4800, "100.0%"),
            ("offset", 0, "-100.0%"),
            ("offset", 1500, "-50.0%"),
            ("offset", 2999, "-0.0%"),
            ("offset", 3001, "0.0%"),
            ("offset", 3015, "0.5%"),
            ("offset", 2985, "-0.5%"),
            ("tone", -3000, "-100.0%"),
            ("tone", 1, "0.0%"),
            ("tone", -1, "-0.0%"),
            ("tone", 29, "1.0%"),
            ("tone", -29, "-1.0%"),
            ("drive", 6000, "100.0%"),
        ] {
            assert_eq!(txt(id, v), s, "{id} {v}");
        }
        // Names.
        assert_eq!(txt("type", 0), "Fake Stereo");
        assert_eq!(txt("type", 2), "Ping Pong");
        assert_eq!(txt("type", 3), "OFF");
        assert_eq!(txt("filter", 3), "OFF");
        assert_eq!(txt("dtype", 1), "Saturate");
        assert_eq!(txt("sync", 0), "Off");
        // The cutoff follows the filter mode.
        for (v, f, s) in [
            (3000, 0, "1299.6Hz"),
            (0, 1, "20.0Hz"),
            (6000, 2, "16000.0Hz"),
            (1500, 3, "479.9Hz"),
            (4500, 0, "6995.7Hz"),
            (1000, 0, "410.0Hz"),
            (2000, 0, "753.1Hz"),
            (5000, 0, "11987.5Hz"),
            (5999, 0, "21975.0Hz"),
            (500, 0, "340.0Hz"),
            (3500, 0, "3196.7Hz"),
            (5800, 0, "19985.5Hz"),
            (200, 0, "298.0Hz"),
        ] {
            assert_eq!(t("cutoff", v, false, f), s, "cutoff {v} in mode {f}");
        }
        // Rate reduction, bits, modulation rate.
        for (v, s) in [(0, "192.3 Hz"), (30000, "1447.3 Hz"), (15000, "492.7 Hz"), (45000, "3202.7 Hz"), (6000, "273.5 Hz"), (54000, "5448.4 Hz"), (59999, "44030.6 Hz"), (1, "192.3 Hz"), (3000, "228.5 Hz"), (21000, "755.7 Hz"), (36000, "2114.1 Hz"), (1500, "209.4 Hz"), (60000, "Max Hz")] {
            assert_eq!(txt("srate", v), s);
        }
        for (v, s) in [(0, "2.0 bits"), (3000, "6.1 bits"), (4500, "9.0 bits"), (1500, "3.9 bits"), (4000, "7.9 bits"), (5999, "15.8 bits"), (1, "2.0 bits"), (5000, "10.3 bits"), (2000, "4.6 bits"), (500, "2.6 bits"), (5500, "12.0 bits"), (100, "2.1 bits"), (6000, "Max bits")] {
            assert_eq!(txt("bits", v), s);
        }
        for (v, s) in [(0, "0.1Hz"), (3000, "1.1Hz"), (6000, "20.0Hz"), (1500, "0.4Hz"), (4500, "3.6Hz"), (1000, "0.3Hz"), (2000, "0.5Hz"), (5000, "6.7Hz"), (5999, "19.9Hz"), (500, "0.2Hz"), (3500, "1.6Hz"), (5500, "10.0Hz"), (200, "0.1Hz")] {
            assert_eq!(txt("mrate", v), s);
        }
    }

    #[test]
    fn knobs_snap_to_their_magnets_and_synced_times_to_musical_ones() {
        // Within 1.5 % of the range (90 of 6000), the offset and tone land on the centre, the feedback on 100 %.
        assert_eq!(snap(dsp::OFFSET, 3080.0, false), Some(3000));
        assert_eq!(snap(dsp::OFFSET, 3100.0, false), None);
        assert_eq!(snap(dsp::TONE, -60.0, false), Some(0));
        assert_eq!(snap(dsp::FEEDBACK, 4750.0, false), Some(4800));
        assert_eq!(value_text(dsp::FEEDBACK, 4800, false, 0), "100.0%");
        assert_eq!(snap(dsp::SYMMETRY, 3000.0, false), None);
        // A synced time lands on quarter and third steps; unsynced it is free.
        assert_eq!(snap(dsp::TIME, 83.0, true), Some(84));
        assert_eq!(snap(dsp::TIME, 81.0, true), Some(80));
        assert_eq!(snap(dsp::TIME, 3.0, true), Some(12));
        assert_eq!(snap(dsp::TIME, 83.0, false), None);
        // The wheel: the next musical time; a magnet stops a step that would pass it.
        assert_eq!(snap_step(dsp::TIME, 80, 86, true, true), 84);
        assert_eq!(snap_step(dsp::TIME, 80, 74, false, true), 72);
        assert_eq!(snap_step(dsp::TIME, 768, 768, true, true), 768);
        assert_eq!(snap_step(dsp::OFFSET, 2980, 3027, true, false), 3000);
        assert_eq!(snap_step(dsp::OFFSET, 3000, 3047, true, false), 3047);
        assert_eq!(snap_step(dsp::DRIVE, 2980, 3027, true, false), 3027);
    }

    #[test]
    fn typed_values_find_the_nearest_setting() {
        let p = |id: &str, s: &str| parse_text(index(id).unwrap(), s, false, LOW_PASS);
        assert_eq!(p("type", "ping pong"), Some(2));
        assert_eq!(p("type", "Mono"), Some(0));
        assert_eq!(p("filter", "bp"), Some(2));
        assert_eq!(p("dtype", "Saturate"), Some(1));
        assert_eq!(p("sync", "on"), Some(1));
        assert_eq!(p("srate", "max"), Some(60000));
        assert_eq!(p("bits", "Max bits"), Some(6000));
        assert_eq!(p("fb", "62.5%"), Some(3000));
        assert_eq!(p("offset", "-50"), Some(1500));
        assert_eq!(p("time", "1000ms"), Some(768));
        assert_eq!(parse_text(index("time").unwrap(), "1:24", true, LOW_PASS), Some(72));
        assert_eq!(parse_text(index("time").unwrap(), "4", true, LOW_PASS), Some(192));
        assert_eq!(p("cutoff", "1299.6"), Some(3000));
    }

    #[test]
    fn a_new_instance_starts_on_fls_default_preset() {
        let b = default_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(field(&b, 0), VERSION);
        assert_eq!(values_of(&b), START.to_vec());
        // FL's "Default" preset (spec plugins/fruity-delay-3.md, "Presets"), as FL shows it.
        let shown = |k: usize| value_text(k, START[k], true, START[dsp::FILTER]);
        assert_eq!(shown(dsp::TYPE), "True Stereo");
        assert_eq!(shown(dsp::SYNC), "On");
        assert_eq!(shown(dsp::TIME), "4:0");
        assert_eq!(shown(dsp::DIST_TYPE), "Saturate");
        assert_eq!(shown(dsp::FEEDBACK), "62.5%");
        assert_eq!(shown(dsp::CUTOFF), "21985.0Hz");
        assert_eq!(shown(dsp::MOD_RATE), "1.1Hz");
        let p = DelayParams::default();
        assert_eq!(p.p.len(), PARAMS);
        assert_eq!(p.p[FILTER].name(), "Filter mode");
        assert_eq!(p.settings(&b), Settings { v: START });
        assert_eq!(p.param_map().len(), PARAMS);
    }

    #[test]
    fn state_round_trips_through_the_plugin_state() {
        let mut fl = default_state();
        set_field(&mut fl, 2, 2);
        set_field(&mut fl, 25, -1200);
        set_field(&mut fl, 5, 0); // Time 0: out of the range, kept
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(fl_from_plugin_state(&blob).expect("parses"), fl);
        let p = DelayParams::default();
        let json: serde_json::Value = serde_json::from_slice(&blob[8..]).unwrap();
        let fields: BTreeMap<String, String> =
            json["fields"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        p.deserialize_fields(&fields);
        assert_eq!(*p.raw.read().unwrap(), fl);
        let mut vals = values_of(&fl);
        vals[0] = 3000;
        assert_eq!(field(&to_fl(&vals, &fl), 1), 3000);
        assert_eq!(field(&to_fl(&vals, &fl), 5), 0);
        // Version 0 reads as 1; version 2 is not read.
        let mut v0 = fl.clone();
        set_field(&mut v0, 0, 0);
        assert_eq!(field(&normalized(v0), 0), VERSION);
        let mut v2 = fl;
        set_field(&mut v2, 0, 2);
        assert_eq!(normalized(v2), default_state());
    }
}
