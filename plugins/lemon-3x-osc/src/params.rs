// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon Trio parameters and saved state.
//!
//! The 114 parameters come in the original's order (parameter k = field k + 1 of its saved state = automation index
//! k), each with the original's stored range, so a project's automation reaches the same control. A copy of the whole
//! 460-byte saved state is persisted next to them (`state`): fields whose meaning Lemon Studio does not change come back
//! out byte for byte (spec: plugins/3x-osc.md §1).
//!
// spec: plugins/3x-osc.md §1, plugins/lemon-3x-osc.md §4 (hint texts)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_plugkit::live::LiveEdits;

/// Parameters (fields 1..=114 of the saved state).
pub const COUNT: usize = 114;
/// Saved state size: 115 int32 fields.
pub const STATE_LEN: usize = 460;
/// Field 0.
pub const STATE_VERSION: i32 = 14;

pub const SHAPES: [&str; 7] = ["Sine", "Triangle", "Square", "Saw", "Rounded saw", "Noise", "Custom"];
pub const FILTERS: [&str; 8] = ["Fast LP", "LP", "BP", "HP", "BS", "LPx2", "SVF LP", "SVF LPx2"];
pub const LFO_SHAPES: [&str; 3] = ["sine", "triangle", "pulse"];
/// Envelope / LFO records in state order.
pub const TARGETS: [&str; 5] = ["Panning", "Volume", "Filter cutoff", "Filter resonance", "Pitch"];
const TARGET_IDS: [&str; 5] = ["pan", "vol", "cut", "res", "pitch"];
pub const T_PAN: usize = 0;
pub const T_VOL: usize = 1;
pub const T_CUT: usize = 2;
pub const T_RES: usize = 3;
pub const T_PITCH: usize = 4;

// Oscillator parameters: offsets from `osc(n)`.
pub const PAN: usize = 0;
pub const SHAPE: usize = 1;
pub const COARSE: usize = 2;
pub const FINE: usize = 3;
pub const PHASE: usize = 4;
pub const DETUNE: usize = 5;
/// Osc 2 / osc 3 mix level.
pub const MIX2: usize = 6;
pub const MIX3: usize = 13;
pub const RAND: usize = 20;
/// Flag bytes: invert osc 1 / 2 / 3, osc 3 = amplitude modulation.
pub const FLAGS: usize = 21;
pub const HQ: usize = 22;
pub const CUTOFF: usize = 110;
pub const RESONANCE: usize = 111;
pub const FILTER: usize = 113;

// Record parameters: offsets from `rec(t)`.
pub const R_FLAGS: usize = 0;
pub const R_ON: usize = 1;
pub const R_DELAY: usize = 2;
pub const R_ATT: usize = 3;
pub const R_HOLD: usize = 4;
pub const R_DEC: usize = 5;
pub const R_SUS: usize = 6;
pub const R_REL: usize = 7;
pub const R_AMT: usize = 8;
pub const R_LFO_DELAY: usize = 9;
pub const R_LFO_ATT: usize = 10;
pub const R_LFO_AMT: usize = 11;
pub const R_LFO_SPEED: usize = 12;
pub const R_LFO_SHAPE: usize = 13;
pub const R_ATT_T: usize = 14;
pub const R_DEC_T: usize = 15;
pub const R_REL_T: usize = 16;

/// First parameter of oscillator `n` (0..3): its panning.
pub const fn osc(n: usize) -> usize {
    7 * n
}

/// First parameter of record `t` (`T_*`).
pub const fn rec(t: usize) -> usize {
    23 + 17 * t
}

/// Flag byte of invert osc `n`; byte 3 = amplitude modulation.
pub const AM_BYTE: usize = 3;

/// How a parameter's value reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Text {
    Pan,
    Shape,
    Semis,
    Cents,
    /// −64..64 as −100..100 %.
    Phase,
    Mix,
    Rand,
    Raw,
    OnOff,
    /// Value / 65536 as %.
    Time,
    Sustain,
    /// ±128 as ±100 %.
    Amount,
    PitchCents,
    LfoShape,
    Filter,
    /// 0..256 as %.
    Quarter,
}

/// One parameter: id, name, stored range, default, text, hidden (not shown to hosts' automation lists).
#[derive(Clone, Debug)]
pub struct Def {
    pub id: String,
    pub name: String,
    pub min: i32,
    pub max: i32,
    pub default: i32,
    pub text: Text,
    pub hidden: bool,
}

/// Default record (spec §1.2), in parameter units (use envelope 0 = off).
const RECORD_DEFAULT: [i32; 17] = [0, 0, 100, 20000, 20000, 30000, 50, 20000, 0, 100, 20000, 0, 32950, 0, 0, 0, 0];

/// The 114 parameters in the original's order.
pub fn defs() -> Vec<Def> {
    let mut v = Vec::with_capacity(COUNT);
    let d = |id: String, name: String, min: i32, max: i32, default: i32, text: Text| Def { id, name, min, max, default, text, hidden: false };
    for n in 0..3 {
        let o = n + 1;
        if n > 0 {
            v.push(d(format!("o{o}_mix"), format!("Osc {o} mix level"), 0, 128, if n == 1 { 64 } else { 32 }, Text::Mix));
        }
        v.push(d(format!("o{o}_pan"), format!("Osc {o} panning"), -64, 64, 0, Text::Pan));
        v.push(d(format!("o{o}_shape"), format!("Osc {o} shape"), 0, 6, 0, Text::Shape));
        v.push(d(format!("o{o}_coarse"), format!("Osc {o} coarse pitch"), -24, 24, [0, -12, -24][n], Text::Semis));
        v.push(d(format!("o{o}_fine"), format!("Osc {o} fine pitch"), -100, 100, 0, Text::Cents));
        v.push(d(format!("o{o}_phase"), format!("Osc {o} stereo phase offset"), -64, 64, 0, Text::Phase));
        v.push(d(format!("o{o}_detune"), format!("Osc {o} stereo detune"), -50, 50, 0, Text::Cents));
    }
    v.push(d("rand".into(), "Stereo phase randomness".into(), 0, 64, 0, Text::Rand));
    // The stored field holds one byte per switch; the parameter is a 4-bit mask (bit b = byte b set), exact through
    // normalised values.
    v.push(Def { hidden: true, ..d("flags".into(), "Oscillator switches".into(), 0, 15, 0, Text::Raw) });
    v.push(d("hq".into(), "High quality".into(), 0, 1, 0, Text::OnOff));
    for (t, (target, tid)) in TARGETS.iter().zip(TARGET_IDS).enumerate() {
        let env = |s: &str| format!("{target} envelope - {s}");
        let lfo = |s: &str| format!("{target} LFO - {s}");
        let r = RECORD_DEFAULT;
        let amt = if t == T_PITCH { (-2400, 2400, Text::PitchCents) } else { (-128, 128, Text::Amount) };
        v.push(Def { hidden: true, ..d(format!("{tid}_flags"), format!("{target} switches"), 0, 1 << 30, if t == T_VOL { 4 } else { 0 }, Text::Raw) });
        v.push(d(format!("{tid}_env"), env("Use envelope"), 0, 1, 0, Text::OnOff));
        v.push(d(format!("{tid}_delay"), env("Predelay time"), 0, 65536, r[R_DELAY], Text::Time));
        v.push(d(format!("{tid}_att"), env("Attack time"), 0, 65536, r[R_ATT], Text::Time));
        v.push(d(format!("{tid}_hold"), env("Hold time"), 0, 65536, r[R_HOLD], Text::Time));
        v.push(d(format!("{tid}_dec"), env("Decay time"), 0, 65536, r[R_DEC], Text::Time));
        v.push(d(format!("{tid}_sus"), env("Sustain level"), 0, 128, r[R_SUS], Text::Sustain));
        v.push(d(format!("{tid}_rel"), env("Release time"), 0, 65536, r[R_REL], Text::Time));
        v.push(d(format!("{tid}_amt"), env("Modulation amount"), amt.0, amt.1, 0, amt.2));
        v.push(d(format!("{tid}_lfo_delay"), lfo("Predelay time"), 0, 65536, r[R_LFO_DELAY], Text::Time));
        v.push(d(format!("{tid}_lfo_att"), lfo("Attack time"), 0, 65536, r[R_LFO_ATT], Text::Time));
        v.push(d(format!("{tid}_lfo_amt"), lfo("Amount"), amt.0, amt.1, 0, amt.2));
        v.push(d(format!("{tid}_lfo_speed"), lfo("Speed"), 0, 65536, r[R_LFO_SPEED], Text::Time));
        v.push(d(format!("{tid}_lfo_shape"), lfo("Shape"), 0, 2, 0, Text::LfoShape));
        v.push(d(format!("{tid}_att_t"), env("Attack tension"), -128, 128, 0, Text::Amount));
        v.push(d(format!("{tid}_dec_t"), env("Decay tension"), -128, 128, 0, Text::Amount));
        v.push(d(format!("{tid}_rel_t"), env("Release tension"), -128, 128, if t == T_VOL { -101 } else { 0 }, Text::Amount));
    }
    v.push(Def { hidden: true, ..d("x108".into(), "Spare 1".into(), 0, 65536, 0, Text::Raw) });
    v.push(Def { hidden: true, ..d("x109".into(), "Spare 2".into(), 0, 65536, 0, Text::Raw) });
    v.push(d("cutoff".into(), "Filter frequency (modulation X)".into(), 0, 256, 256, Text::Quarter));
    v.push(d("resonance".into(), "Filter bandwidth (modulation Y)".into(), 0, 256, 0, Text::Quarter));
    v.push(Def { hidden: true, ..d("x112".into(), "Spare 3".into(), 0, 65536, 0, Text::Raw) });
    v.push(d("filter".into(), "Filter type".into(), 0, 7, 0, Text::Filter));
    debug_assert_eq!(v.len(), COUNT);
    v
}

fn plural(v: i32, one: &str, many: &str) -> String {
    let sign = if v >= 0 { "+" } else { "" };
    format!("{sign}{v} {}", if v.abs() <= 1 { one } else { many })
}

/// A parameter's value text (spec §1 "Value text").
pub fn value_text(t: Text, v: i32) -> String {
    let pct = |num: f64| format!("{:.0}%", num * 100.0);
    match t {
        Text::Pan => match v {
            0 => "Centered".into(),
            v if v < 0 => format!("{:.0}% left", f64::from(-v) / 64.0 * 100.0),
            v => format!("{:.0}% right", f64::from(v) / 64.0 * 100.0),
        },
        Text::Shape => SHAPES.get(v.clamp(0, 6) as usize).copied().unwrap_or("").into(),
        Text::Semis => plural(v, "semitone", "semitones"),
        Text::Cents | Text::PitchCents => plural(v, "cent", "cents"),
        Text::Phase => pct(f64::from(v) / 64.0),
        Text::Mix => pct(f64::from(v) / 128.0),
        Text::Rand => pct(f64::from(v) / 64.0),
        Text::Raw => v.to_string(),
        Text::OnOff => if v != 0 { "On" } else { "Off" }.into(),
        Text::Time => pct(f64::from(v) / 65536.0),
        Text::Sustain => pct(f64::from(v) / 128.0),
        Text::Amount => pct(f64::from(v) / 128.0),
        Text::LfoShape => LFO_SHAPES.get(v.clamp(0, 2) as usize).copied().unwrap_or("").into(),
        Text::Filter => FILTERS.get(v.clamp(0, 7) as usize).copied().unwrap_or("").into(),
        Text::Quarter => pct(f64::from(v) / 256.0),
    }
}

/// A typed value in a parameter's units: a number (percent texts take the percentage), or a list entry's name.
pub fn parse_text(t: Text, s: &str, min: i32, max: i32) -> Option<i32> {
    let s = s.trim();
    let named = |list: &[&str]| list.iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| i as i32);
    match t {
        Text::Shape => return named(&SHAPES),
        Text::LfoShape => return named(&LFO_SHAPES),
        Text::Filter => return named(&FILTERS),
        Text::OnOff => {
            return match s.to_ascii_lowercase().as_str() {
                "on" | "1" | "yes" => Some(1),
                "off" | "0" | "no" => Some(0),
                _ => None,
            }
        }
        Text::Pan if s.eq_ignore_ascii_case("centered") => return Some(0),
        _ => {}
    }
    let num: String = s.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let mut x: f64 = num.parse().ok()?;
    let lower = s.to_ascii_lowercase();
    if t == Text::Pan && lower.contains("left") {
        x = -x;
    }
    let scale = match t {
        Text::Pan | Text::Phase | Text::Rand => 64.0 / 100.0,
        Text::Mix | Text::Sustain | Text::Amount => 128.0 / 100.0,
        Text::Time => 65536.0 / 100.0,
        Text::Quarter => 256.0 / 100.0,
        _ => 1.0,
    };
    Some(((x * scale).round() as i32).clamp(min, max))
}

pub struct Osc3Params {
    /// The parameters in the original's order.
    pub p: Vec<IntParam>,
    pub defs: Vec<Def>,
    /// The original's saved state the parameters came from (fields the parameters do not change come back verbatim).
    pub raw: RwLock<Vec<u8>>,
    /// Bumped on every `raw` change.
    pub raw_version: AtomicU64,
    /// Editor edits the host has not confirmed yet (the sound hears them at once).
    pub live: Arc<LiveEdits>,
}

impl Default for Osc3Params {
    fn default() -> Self {
        let defs = defs();
        let p = defs
            .iter()
            .map(|d| {
                let (text, min, max) = (d.text, d.min, d.max);
                let mut ip = IntParam::new(d.name.clone(), d.default, IntRange::Linear { min: d.min, max: d.max })
                    .with_value_to_string(Arc::new(move |v| value_text(text, v)))
                    .with_string_to_value(Arc::new(move |s| parse_text(text, s, min, max)));
                if d.hidden {
                    ip = ip.hide().non_automatable();
                }
                ip
            })
            .collect();
        Osc3Params { p, defs, raw: RwLock::new(default_state()), raw_version: AtomicU64::new(0), live: Arc::default() }
    }
}

/// The persisted field holding `raw`.
pub const STATE_FIELD: &str = "state";

unsafe impl Params for Osc3Params {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        self.p.iter().zip(&self.defs).map(|(p, d)| (d.id.clone(), p.as_ptr(), String::new())).collect()
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let hex: String = self.raw.read().map(|b| b.iter().map(|x| format!("{x:02x}")).collect()).unwrap_or_default();
        BTreeMap::from([(STATE_FIELD.to_string(), format!("\"{hex}\""))])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(bytes) = serialized.get(STATE_FIELD).and_then(|s| from_hex(s.trim_matches('"'))) {
            self.set_raw(normalized(bytes));
        }
    }
}

impl Osc3Params {
    /// Parameter `k`'s value for the sound (an editor edit the host has not confirmed yet wins).
    pub fn v(&self, k: usize) -> i32 {
        self.live.value(&self.p[k])
    }

    pub fn set_raw(&self, b: Vec<u8>) {
        if let Ok(mut r) = self.raw.write() {
            *r = b;
        }
        self.raw_version.fetch_add(1, Ordering::AcqRel);
    }

    /// The original's saved state of the current settings.
    pub fn fl_state(&self) -> Vec<u8> {
        let raw = self.raw.read().map(|b| b.clone()).unwrap_or_else(|_| default_state());
        let vals: Vec<i32> = self.p.iter().map(|p| p.value()).collect();
        to_fl(&vals, &raw)
    }

    /// Whether the invert / AM flag byte `b` is set.
    pub fn flag(&self, b: usize) -> bool {
        self.v(FLAGS) & (1 << b) != 0
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

/// Little-endian int32 field `k`.
pub fn field(b: &[u8], k: usize) -> i32 {
    b.get(4 * k..4 * k + 4).map_or(0, |s| i32::from_le_bytes(s.try_into().unwrap()))
}

pub fn set_field(b: &mut [u8], k: usize, v: i32) {
    if let Some(s) = b.get_mut(4 * k..4 * k + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

/// True for the "use envelope" parameters (stored −1 on, 0 off).
fn is_use(k: usize) -> bool {
    (rec(0)..rec(5)).contains(&k) && (k - rec(0)) % 17 == R_ON
}

/// Parameter `k`'s value for stored field value `s` (clamped to the parameter's range).
pub fn decode(defs: &[Def], k: usize, s: i32) -> i32 {
    if is_use(k) {
        return i32::from(s != 0);
    }
    if k == FLAGS {
        return (0..4).filter(|b| (s >> (8 * b)) & 0xff != 0).map(|b| 1 << b).sum();
    }
    s.clamp(defs[k].min, defs[k].max)
}

/// The stored field value for parameter `k`'s value `v`.
pub fn encode(k: usize, v: i32) -> i32 {
    if is_use(k) {
        return if v != 0 { -1 } else { 0 };
    }
    if k == FLAGS {
        return (0..4).filter(|b| v & (1 << b) != 0).map(|b| 1 << (8 * b)).sum();
    }
    v
}

/// A new instance's saved state (spec §1.1).
pub fn default_state() -> Vec<u8> {
    let mut b = vec![0u8; STATE_LEN];
    set_field(&mut b, 0, STATE_VERSION);
    for (k, d) in defs().iter().enumerate() {
        set_field(&mut b, k + 1, encode(k, d.default));
    }
    b
}

/// The original's saved state with parameter values `vals`: fields whose value still means the same keep their stored
/// bytes (out-of-range values, other "on" values), the rest are written from the parameters.
pub fn to_fl(vals: &[i32], raw: &[u8]) -> Vec<u8> {
    let defs = defs();
    let mut b = normalized(raw.to_vec());
    if field(&b, 0) == 0 {
        set_field(&mut b, 0, STATE_VERSION);
    }
    for (k, &v) in vals.iter().enumerate().take(COUNT) {
        let stored = field(&b, k + 1);
        if decode(&defs, k, stored) != v {
            set_field(&mut b, k + 1, encode(k, v));
        }
    }
    b
}

/// Parameter values of a saved state.
pub fn values_of(fl: &[u8]) -> Vec<i32> {
    let defs = defs();
    (0..COUNT).map(|k| decode(&defs, k, field(fl, k + 1))).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Plugin state <-> the original's saved state (Lemon Studio keeps a project channel's original state, event 213)

/// The plugin's own saved state (nih-plug JSON with the u64 length prefix of its CLAP state) for a channel's saved
/// state `fl` (460 bytes).
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let b = normalized(fl.to_vec());
    let defs = defs();
    let params: Vec<String> = values_of(&b).iter().zip(&defs).map(|(v, d)| format!("\"{}\":{{\"i32\":{v}}}", d.id)).collect();
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
    let defs = defs();
    let mut vals = values_of(&raw);
    for (k, d) in defs.iter().enumerate() {
        if let Some(x) = params.get(&d.id).and_then(|p| p.get("i32")).and_then(|x| x.as_i64()) {
            vals[k] = x as i32;
        }
    }
    Some(to_fl(&vals, &raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_matches_the_measured_defaults() {
        let b = default_state();
        assert_eq!(b.len(), STATE_LEN);
        let f: Vec<i32> = (0..24).map(|k| field(&b, k)).collect();
        assert_eq!(f, vec![14, 0, 0, 0, 0, 0, 0, 64, 0, 0, -12, 0, 0, 0, 32, 0, 0, -24, 0, 0, 0, 0, 0, 0]);
        let rec_fields = |t: usize| (0..17).map(|i| field(&b, rec(t) + i + 1)).collect::<Vec<_>>();
        assert_eq!(rec_fields(T_PAN), vec![0, 0, 100, 20000, 20000, 30000, 50, 20000, 0, 100, 20000, 0, 32950, 0, 0, 0, 0]);
        let vol = rec_fields(T_VOL);
        assert_eq!((vol[0], vol[16]), (4, -101));
        assert_eq!((109..115).map(|k| field(&b, k)).collect::<Vec<_>>(), vec![0, 0, 256, 0, 0, 0]);
    }

    #[test]
    fn parameter_order_follows_the_original() {
        let d = defs();
        assert_eq!(d.len(), COUNT);
        assert_eq!(d[0].name, "Osc 1 panning");
        assert_eq!(d[6].name, "Osc 2 mix level");
        assert_eq!(d[13].name, "Osc 3 mix level");
        assert_eq!(d[16].name, "Osc 3 coarse pitch");
        assert_eq!(d[20].name, "Stereo phase randomness");
        assert_eq!(d[24].name, "Panning envelope - Use envelope");
        assert_eq!(d[52].name, "Volume LFO - Speed");
        assert_eq!(d[56].name, "Volume envelope - Release tension");
        assert_eq!(d[58].name, "Filter cutoff envelope - Use envelope");
        assert_eq!(d[84].name, "Filter resonance LFO - Attack time");
        assert_eq!(d[104].name, "Pitch LFO - Shape");
        assert_eq!(d[110].name, "Filter frequency (modulation X)");
        assert_eq!(d[111].name, "Filter bandwidth (modulation Y)");
        assert_eq!(d[113].name, "Filter type");
        // Ids are unique (they key the plugin state).
        let mut ids: Vec<&str> = d.iter().map(|x| x.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), COUNT);
    }

    #[test]
    fn value_texts_match_the_original() {
        assert_eq!(value_text(Text::Pan, 0), "Centered");
        assert_eq!(value_text(Text::Pan, -64), "100% left");
        assert_eq!(value_text(Text::Pan, 16), "25% right");
        assert_eq!(value_text(Text::Semis, 0), "+0 semitone");
        assert_eq!(value_text(Text::Semis, -12), "-12 semitones");
        assert_eq!(value_text(Text::Cents, 25), "+25 cents");
        assert_eq!(value_text(Text::Cents, 1), "+1 cent");
        assert_eq!(value_text(Text::Mix, 64), "50%");
        assert_eq!(value_text(Text::Mix, 32), "25%");
        assert_eq!(value_text(Text::Time, 20000), "31%");
        assert_eq!(value_text(Text::Sustain, 50), "39%");
        assert_eq!(value_text(Text::Amount, -101), "-79%");
        assert_eq!(value_text(Text::Time, 32950), "50%");
        assert_eq!(value_text(Text::Filter, 6), "SVF LP");
        assert_eq!(parse_text(Text::Pan, "50% left", -64, 64), Some(-32));
        assert_eq!(parse_text(Text::Semis, "-7", -24, 24), Some(-7));
        assert_eq!(parse_text(Text::Filter, "bp", 0, 7), Some(2));
    }

    #[test]
    fn state_round_trips_through_the_plugin_state() {
        let mut fl = default_state();
        set_field(&mut fl, osc(1) + COARSE + 1, 7);
        set_field(&mut fl, FLAGS + 1, 0x0100_0001); // invert osc 1, AM
        set_field(&mut fl, rec(T_VOL) + R_ON + 1, -1);
        set_field(&mut fl, rec(T_PITCH) + R_AMT + 1, 1200);
        set_field(&mut fl, rec(T_CUT) + R_FLAGS + 1, 397284480);
        set_field(&mut fl, 109, 77); // a field Lemon Studio does not use
        set_field(&mut fl, rec(T_PITCH) + R_LFO_AMT + 1, 9000); // out of the knob's range: kept
        let blob = plugin_state_from_fl(&fl);
        assert_eq!(fl_from_plugin_state(&blob).expect("parses"), fl);
        // The persisted field restores the stored bytes into an instance.
        let p = Osc3Params::default();
        let json: serde_json::Value = serde_json::from_slice(&blob[8..]).unwrap();
        let fields: BTreeMap<String, String> =
            json["fields"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        p.deserialize_fields(&fields);
        assert_eq!(*p.raw.read().unwrap(), fl);
        assert_eq!(to_fl(&values_of(&fl), &fl), fl);
    }

    #[test]
    fn edits_reach_the_saved_state() {
        let fl = default_state();
        let mut vals = values_of(&fl);
        vals[rec(T_VOL) + R_ON] = 1;
        vals[MIX3] = 128;
        vals[FILTER] = 5;
        let b = to_fl(&vals, &fl);
        assert_eq!(field(&b, rec(T_VOL) + R_ON + 1), -1);
        assert_eq!(field(&b, MIX3 + 1), 128);
        assert_eq!(field(&b, FILTER + 1), 5);
    }
}
