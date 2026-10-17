// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Parametric EQ parameters and saved state.
//!
//! The 36 automatable parameters come in the original's order (7 levels, 7 frequencies, 7 widths, 7 types, 7 orders,
//! main level). Everything else (options, the spare A/B bank, mute / solo) lives in a copy of the original's 354-byte
//! saved state (`extra`), persisted with the parameters, so a project's state goes in and comes back out unchanged in
//! meaning (spec: plugins/parametric-eq-2.md "Saved state").
//!
// spec: plugins/parametric-eq-2.md, plugins/lemon-parametric-eq-2.md (§5 hints, §8 state)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_engine::effects::eq::{self, Band, Settings, BANDS, MASK_BYTE, STATE_LEN, STATE_VERSION};
use os_plugkit::live::LiveEdits;

pub const TYPE_NAMES: [&str; 8] = ["Disabled", "Low pass", "Band pass", "High pass", "Band stop", "Low shelf", "Peaking", "High shelf"];
/// Order names for stored orders -3..=3.
pub const ORDER_NAMES: [&str; 7] = ["Steep 8", "Steep 6", "Steep 4", "2", "Gentle 4", "Gentle 6", "Gentle 8"];

pub fn level_id(k: usize) -> String {
    format!("level{}", k + 1)
}
pub fn freq_id(k: usize) -> String {
    format!("freq{}", k + 1)
}
pub fn width_id(k: usize) -> String {
    format!("width{}", k + 1)
}
pub fn type_id(k: usize) -> String {
    format!("type{}", k + 1)
}
pub fn order_id(k: usize) -> String {
    format!("order{}", k + 1)
}
pub const MAIN_ID: &str = "main";

/// "63Hz", "6325Hz" (whole hertz, spec §5).
pub fn hz_text(hz: f64) -> String {
    format!("{:.0}Hz", hz)
}

/// "0.0dB".
pub fn db_text(db: f64) -> String {
    let v = (db * 10.0).round() / 10.0;
    format!("{:.1}dB", if v == 0.0 { 0.0 } else { v })
}

/// Hertz from a typed text: "440", "440Hz", "1.2k", "1.2 kHz".
pub fn parse_hz(t: &str) -> Option<f64> {
    let l = t.trim().to_ascii_lowercase();
    let num: String = l.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
    let v: f64 = num.parse().ok()?;
    Some(if l[num.len()..].trim_start().starts_with('k') { v * 1000.0 } else { v })
}

fn number(t: &str) -> Option<f32> {
    let num: String = t.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    num.parse().ok()
}

pub struct EqParams {
    pub level: [FloatParam; BANDS],
    pub freq: [FloatParam; BANDS],
    pub width: [FloatParam; BANDS],
    pub kind: [IntParam; BANDS],
    pub order: [IntParam; BANDS],
    pub main: FloatParam,
    /// The original's saved-state image holding the options, the spare bank and bytes Lemon Studio does not use (fields
    /// 1..36 in it are stale: the parameters above are the truth).
    pub extra: RwLock<Vec<u8>>,
    /// Bumped on every `extra` change (repaint, audio refresh).
    pub extra_version: AtomicU64,
    /// Editor edits the host has not confirmed yet (the sound hears them at once).
    pub live: Arc<LiveEdits>,
}

fn level_param(name: String) -> FloatParam {
    FloatParam::new(name, 0.0, FloatRange::Linear { min: -18.0, max: 18.0 })
        .with_step_size(0.01)
        .with_value_to_string(Arc::new(|v| db_text(f64::from(v))))
        .with_string_to_value(Arc::new(number))
}

impl Default for EqParams {
    fn default() -> Self {
        let band = |k: usize| format!("Band {}", k + 1);
        EqParams {
            level: std::array::from_fn(|k| level_param(format!("{} level", band(k)))),
            freq: std::array::from_fn(|k| {
                FloatParam::new(format!("{} freq", band(k)), eq::default_freq(k) as f32, FloatRange::Linear { min: 0.0, max: 1.0 })
                    .with_value_to_string(Arc::new(|v| hz_text(eq::freq_hz(f64::from(v)))))
                    .with_string_to_value(Arc::new(|t| parse_hz(t).map(|hz| eq::freq_pos(hz) as f32)))
            }),
            width: std::array::from_fn(|k| {
                FloatParam::new(format!("{} width", band(k)), eq::default_width(eq::DEFAULT_KINDS[k]) as f32, FloatRange::Linear { min: 0.0, max: 1.0 })
                    .with_value_to_string(Arc::new(|v| format!("{:.0}%", v * 100.0)))
                    .with_string_to_value(Arc::new(|t| number(t).map(|v| v / 100.0)))
            }),
            kind: std::array::from_fn(|k| {
                IntParam::new(format!("{} type", band(k)), eq::DEFAULT_KINDS[k], IntRange::Linear { min: 0, max: 7 })
                    .with_value_to_string(Arc::new(|v| TYPE_NAMES[v.clamp(0, 7) as usize].to_string()))
                    .with_string_to_value(Arc::new(|t| TYPE_NAMES.iter().position(|n| n.eq_ignore_ascii_case(t.trim())).map(|i| i as i32)))
            }),
            order: std::array::from_fn(|k| {
                IntParam::new(format!("{} order", band(k)), 0, IntRange::Linear { min: -3, max: 3 })
                    .with_value_to_string(Arc::new(|v| ORDER_NAMES[(v.clamp(-3, 3) + 3) as usize].to_string()))
                    .with_string_to_value(Arc::new(|t| ORDER_NAMES.iter().position(|n| n.eq_ignore_ascii_case(t.trim())).map(|i| i as i32 - 3)))
            }),
            main: level_param("Main level".into()),
            extra: RwLock::new(default_state()),
            extra_version: AtomicU64::new(0),
            live: Arc::default(),
        }
    }
}

unsafe impl Params for EqParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut v = Vec::with_capacity(36);
        for k in 0..BANDS {
            v.push((level_id(k), self.level[k].as_ptr(), String::new()));
        }
        for k in 0..BANDS {
            v.push((freq_id(k), self.freq[k].as_ptr(), String::new()));
        }
        for k in 0..BANDS {
            v.push((width_id(k), self.width[k].as_ptr(), String::new()));
        }
        for k in 0..BANDS {
            v.push((type_id(k), self.kind[k].as_ptr(), String::new()));
        }
        for k in 0..BANDS {
            v.push((order_id(k), self.order[k].as_ptr(), String::new()));
        }
        v.push((MAIN_ID.into(), self.main.as_ptr(), String::new()));
        v
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let hex: String = self.extra.read().map(|b| b.iter().map(|x| format!("{x:02x}")).collect()).unwrap_or_default();
        // A persisted field is JSON: a string here.
        BTreeMap::from([(STATE_FIELD.to_string(), format!("\"{hex}\""))])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(bytes) = serialized.get(STATE_FIELD).and_then(|s| from_hex(s.trim_matches('"'))) {
            self.set_extra(normalized(bytes));
        }
    }
}

/// The persisted field holding `extra`.
pub const STATE_FIELD: &str = "state";

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// A state image of the full size (shorter ones are completed with the defaults).
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

/// Stored parameter values (fields 1..36) of the default bands.
fn default_fields() -> [i32; 36] {
    let mut f = [0; 36];
    for k in 0..BANDS {
        let kind = eq::DEFAULT_KINDS[k];
        f[7 + k] = (eq::default_freq(k) * 65536.0).round() as i32;
        f[14 + k] = (eq::default_width(kind) * 65536.0).round() as i32;
        f[21 + k] = kind;
    }
    f
}

/// A new instance's saved state (spec: defaults and default option fields).
pub fn default_state() -> Vec<u8> {
    let mut b = vec![0u8; STATE_LEN];
    set_field(&mut b, 0, STATE_VERSION);
    for (i, v) in default_fields().into_iter().enumerate() {
        set_field(&mut b, 1 + i, v);
        set_field(&mut b, 37 + i, v);
    }
    let opts = [1, 0, 513, 0, 512, 0, 8347648, 65536, 0, 196608, 65536, 131072, 131072, 131072, 0];
    for (i, v) in opts.into_iter().enumerate() {
        set_field(&mut b, 73 + i, v);
    }
    b
}

// ---------------------------------------------------------------------------------------------------------------
// Options inside the state image (spec: "Options" table)

pub const HQ_BYTE: usize = 292;
pub const HEAT_PRECISION_BYTE: usize = 296;
pub const TOKENS_BYTE: usize = 300;
pub const MONITOR_BYTE: usize = 301;
pub const ENHANCED_OFF_BYTE: usize = 305;
pub const LIN_PRECISION_BYTE: usize = 309;
pub const LIN_SPEED_BYTE: usize = 313;
pub const FLAGS_BYTE: usize = 317;

pub const F_LIN: u8 = 0x01;
pub const F_HQ_ENHANCED: u8 = 0x02;
pub const F_PHASE_ROTATION: u8 = 0x04;
pub const F_SOLO_SHIFT: u8 = 0x08;
pub const F_INHERIT_BW: u8 = 0x10;
pub const F_HEATMAP: u8 = 0x20;
pub const F_HISTOGRAM: u8 = 0x40;
pub const F_AVERAGE: u8 = 0x80;

/// Index-valued options stored as `value × 65536` in fields 80..87.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opt {
    /// 1 Mid, 2 Side, 3 Left, 4 Right.
    MonitorInput = 80,
    /// 0 Adaptive, 1 Bandpass.
    SoloMode = 81,
    PivotSlope = 82,
    FreqPrecision = 83,
    TimeSmoothing = 84,
    /// 1 -60 dB, 2 -90 dB, 3 -120 dB.
    Range = 85,
    HeatmapPosition = 86,
    MinPhaseSpeed = 87,
}

pub fn opt(b: &[u8], o: Opt) -> i32 {
    field(b, o as usize) / 65536
}

pub fn set_opt(b: &mut [u8], o: Opt, v: i32) {
    set_field(b, o as usize, v * 65536);
}

pub fn flag(b: &[u8], f: u8) -> bool {
    b.get(FLAGS_BYTE).is_some_and(|x| x & f != 0)
}

pub fn set_flag(b: &mut [u8], f: u8, on: bool) {
    if let Some(x) = b.get_mut(FLAGS_BYTE) {
        if on {
            *x |= f;
        } else {
            *x &= !f;
        }
    }
}

pub fn byte(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

pub fn set_byte(b: &mut [u8], i: usize, v: u8) {
    if let Some(x) = b.get_mut(i) {
        *x = v;
    }
}

pub fn mask(b: &[u8]) -> u8 {
    b.get(MASK_BYTE).copied().unwrap_or(0x7f) & 0x7f
}

impl EqParams {
    /// Replaces the state image.
    pub fn set_extra(&self, b: Vec<u8>) {
        if let Ok(mut e) = self.extra.write() {
            *e = b;
        }
        self.extra_version.fetch_add(1, Ordering::AcqRel);
    }

    /// Changes the state image in place.
    pub fn edit_extra(&self, f: impl FnOnce(&mut Vec<u8>)) {
        if let Ok(mut e) = self.extra.write() {
            f(&mut e);
        }
        self.extra_version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn extra_copy(&self) -> Vec<u8> {
        self.extra.read().map(|b| b.clone()).unwrap_or_else(|_| default_state())
    }

    /// The band settings the sound uses (live parameter values; mask from the state image when it is free).
    pub fn settings(&self, mask: u8) -> Settings {
        Settings {
            bands: std::array::from_fn(|k| Band {
                db: f64::from(self.live.value(&self.level[k])),
                freq: f64::from(self.live.value(&self.freq[k])),
                width: f64::from(self.live.value(&self.width[k])),
                kind: self.live.value(&self.kind[k]),
                order: self.live.value(&self.order[k]),
            }),
            main_db: f64::from(self.live.value(&self.main)),
            mask,
        }
    }
}

/// Stored parameter values (fields 1..36) of `s`'s bands and main level.
pub fn fields_of(s: &Settings) -> [i32; 36] {
    let mut f = [0; 36];
    for (k, b) in s.bands.iter().enumerate() {
        f[k] = (b.db * 100.0).round() as i32;
        f[7 + k] = (b.freq * 65536.0).round() as i32;
        f[14 + k] = (b.width * 65536.0).round() as i32;
        f[21 + k] = b.kind;
        f[28 + k] = b.order;
    }
    f[35] = (s.main_db * 100.0).round() as i32;
    f
}

/// The band settings stored in fields `first..first + 36` of a state image (1 = main bank, 37 = spare bank).
pub fn settings_at(b: &[u8], first: usize) -> Settings {
    let p = |k: usize| field(b, first + k);
    Settings {
        bands: std::array::from_fn(|k| Band {
            db: f64::from(p(k)) / 100.0,
            freq: f64::from(p(7 + k)) / 65536.0,
            width: f64::from(p(14 + k)) / 65536.0,
            kind: p(21 + k),
            order: p(28 + k),
        }),
        main_db: f64::from(p(35)) / 100.0,
        mask: mask(b),
    }
}

/// The original's saved state of an instance: the state image with the current parameters in fields 1..36.
pub fn to_fl(s: &Settings, extra: &[u8]) -> Vec<u8> {
    let mut b = normalized(extra.to_vec());
    set_field(&mut b, 0, STATE_VERSION);
    for (i, v) in fields_of(s).into_iter().enumerate() {
        set_field(&mut b, 1 + i, v);
    }
    b
}

// ---------------------------------------------------------------------------------------------------------------
// Plugin state <-> the original's saved state (for Lemon Studio: project slots hold the original's state)

/// The plugin's own saved state (nih-plug JSON with the u64 length prefix of its CLAP state) for a project slot's
/// saved state `fl` (354 bytes, event 213).
pub fn plugin_state_from_fl(fl: &[u8]) -> Vec<u8> {
    let b = normalized(fl.to_vec());
    let s = settings_at(&b, 1);
    let f = |v: f64| format!("{{\"f32\":{}}}", v as f32);
    let i = |v: i32| format!("{{\"i32\":{v}}}");
    let mut params = Vec::new();
    for (k, band) in s.bands.iter().enumerate() {
        params.push(format!("\"{}\":{}", level_id(k), f(band.db.clamp(-18.0, 18.0))));
        params.push(format!("\"{}\":{}", freq_id(k), f(band.freq.clamp(0.0, 1.0))));
        params.push(format!("\"{}\":{}", width_id(k), f(band.width.clamp(0.0, 1.0))));
        params.push(format!("\"{}\":{}", type_id(k), i(band.kind.clamp(0, 7))));
        params.push(format!("\"{}\":{}", order_id(k), i(band.order.clamp(-3, 3))));
    }
    params.push(format!("\"{MAIN_ID}\":{}", f(s.main_db.clamp(-18.0, 18.0))));
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    // The persisted field is a JSON string holding JSON (a string).
    let state = format!("{{\"version\":\"{}\",\"params\":{{{}}},\"fields\":{{\"{STATE_FIELD}\":\"\\\"{hex}\\\"\"}}}}", env!("CARGO_PKG_VERSION"), params.join(","));
    let mut blob = (state.len() as u64).to_le_bytes().to_vec();
    blob.extend_from_slice(state.as_bytes());
    blob
}

/// The original's saved state for a plugin state blob (see [`plugin_state_from_fl`]); `None` when it is not one.
pub fn fl_from_plugin_state(blob: &[u8]) -> Option<Vec<u8>> {
    let json = if blob.len() > 8 && blob[0] != b'{' { &blob[8..] } else { blob };
    let text = std::str::from_utf8(json).ok()?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let field = v.get("fields")?.get(STATE_FIELD)?.as_str()?;
    let extra = normalized(from_hex(field.trim_matches('"'))?);
    let params = v.get("params")?;
    let num = |id: &str| -> Option<f64> {
        let p = params.get(id)?;
        p.get("f32").and_then(|x| x.as_f64()).or_else(|| p.get("i32").and_then(|x| x.as_f64()))
    };
    let mut s = settings_at(&extra, 1);
    for (k, band) in s.bands.iter_mut().enumerate() {
        band.db = num(&level_id(k)).unwrap_or(band.db);
        band.freq = num(&freq_id(k)).unwrap_or(band.freq);
        band.width = num(&width_id(k)).unwrap_or(band.width);
        band.kind = num(&type_id(k)).map_or(band.kind, |x| x as i32);
        band.order = num(&order_id(k)).map_or(band.order, |x| x as i32);
    }
    s.main_db = num(MAIN_ID).unwrap_or(s.main_db);
    Some(to_fl(&s, &extra))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_matches_the_measured_defaults() {
        let b = default_state();
        assert_eq!(b.len(), 354);
        assert_eq!(field(&b, 0), 8);
        assert_eq!((8..15).map(|k| field(&b, k)).collect::<Vec<_>>(), vec![10923, 18204, 25486, 32768, 40050, 47332, 54613]);
        assert_eq!(field(&b, 15), 40124);
        assert_eq!(field(&b, 16), 25412);
        assert_eq!(byte(&b, HQ_BYTE), 1);
        assert_eq!(byte(&b, TOKENS_BYTE), 1);
        assert_eq!(byte(&b, MONITOR_BYTE), 2);
        assert_eq!(byte(&b, LIN_PRECISION_BYTE), 2);
        assert_eq!(byte(&b, FLAGS_BYTE), 0x60);
        assert_eq!(mask(&b), 0x7f);
        assert_eq!(opt(&b, Opt::MonitorInput), 1);
        assert_eq!(opt(&b, Opt::PivotSlope), 3);
        assert_eq!(opt(&b, Opt::Range), 2);
        // Spare bank = main bank.
        assert_eq!(b[4..148], b[148..292]);
    }

    #[test]
    fn state_round_trips_through_the_plugin_state() {
        let mut fl = default_state();
        set_field(&mut fl, 1 + 2, -650); // band 3 at -6.5 dB
        set_field(&mut fl, 22 + 6, 3); // band 7 high pass
        set_field(&mut fl, 29 + 6, -3); // Steep 8
        set_field(&mut fl, 36, 120); // main +1.2 dB
        set_byte(&mut fl, MASK_BYTE, 0x7b);
        set_byte(&mut fl, 330, 0x5a); // a byte Lemon Studio does not use survives
        let blob = plugin_state_from_fl(&fl);
        let back = fl_from_plugin_state(&blob).expect("parses");
        assert_eq!(back, fl);
    }

    #[test]
    fn value_texts() {
        let p = EqParams::default();
        assert_eq!(p.freq[0].to_string(), "63Hz");
        assert_eq!(p.width[0].normalized_value_to_string(p.width[0].unmodulated_normalized_value(), true), "61%");
        assert_eq!(p.level[0].to_string(), "0.0dB");
        assert_eq!(p.kind[0].to_string(), "Low shelf");
        assert_eq!(p.order[0].to_string(), "2");
        assert!((parse_hz("1.2kHz").unwrap() - 1200.0).abs() < 1e-9);
    }
}
