// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The parameters and saved state of a small effect.
//!
//! The settings come in the original's order (setting k = int k of its saved state, unless the description maps them:
//! [`Spec::fields`]) with the original's editor ranges. A copy of the whole saved state is persisted next to them (`state`): stored values outside a setting's
//! range keep their bytes and play as stored (the originals use stored values as they are).
//!
// spec: plugins/lemon-small-effects.md (§4 magnets and typed values, §5 parameters and state), the plugin pages
// (plugins/lemon-soft-clipper.md, lemon-fast-distortion.md, lemon-balance.md, lemon-stereo-enhancer.md,
// lemon-transient-processor.md, lemon-chorus.md, lemon-stereo-shaper.md; each §3 value texts, §5)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_plugkit::live::LiveEdits;

use crate::{Spec, MAX};

/// How near (a share of the range) a drag has to come to a magnet to land on it.
pub const MAGNET_ZONE: f32 = 0.015;

pub struct FxParams {
    pub spec: &'static Spec,
    /// The settings in the original's order.
    pub p: Vec<IntParam>,
    /// The original's saved state the parameters came from.
    pub raw: RwLock<Vec<u8>>,
    /// Bumped on every `raw` change.
    pub raw_version: AtomicU64,
    /// Editor edits the host has not confirmed yet (the sound hears them at once).
    pub live: Arc<LiveEdits>,
}

impl FxParams {
    pub fn new(spec: &'static Spec) -> FxParams {
        let p = spec
            .settings
            .iter()
            .enumerate()
            .map(|(k, s)| {
                let (text, parse) = (spec.text, spec.parse);
                IntParam::new(s.name, s.start, IntRange::Linear { min: s.range.0, max: s.range.1 })
                    .with_value_to_string(Arc::new(move |v| text(k, v)))
                    .with_string_to_value(Arc::new(move |t| parse(k, t)))
            })
            .collect();
        FxParams { spec, p, raw: RwLock::new(default_state(spec)), raw_version: AtomicU64::new(0), live: Arc::default() }
    }

    /// Setting `k`'s value for the sound (an editor edit the host has not confirmed yet wins).
    pub fn v(&self, k: usize) -> i32 {
        self.live.value(&self.p[k])
    }

    /// The settings the sound plays (the first `n` of `MAX`): settings whose stored value still means the same keep
    /// it (values outside a setting's range are used as they are).
    pub fn settings(&self, raw: &[u8]) -> [i32; MAX] {
        std::array::from_fn(|k| {
            if k >= self.p.len() {
                return 0;
            }
            let v = self.v(k);
            let s = setting(self.spec, raw, k);
            if decode(self.spec, k, s) == v { s } else { v }
        })
    }

    pub fn set_raw(&self, b: Vec<u8>) {
        if let Ok(mut r) = self.raw.write() {
            *r = b;
        }
        self.raw_version.fetch_add(1, Ordering::AcqRel);
    }

    pub fn raw_copy(&self) -> Vec<u8> {
        self.raw.read().map(|b| b.clone()).unwrap_or_else(|_| default_state(self.spec))
    }

    /// The original's saved state of the current settings.
    pub fn fl_state(&self) -> Vec<u8> {
        let vals: Vec<i32> = self.p.iter().map(|p| p.value()).collect();
        to_fl(self.spec, &vals, &self.raw_copy())
    }
}

/// The persisted field holding `raw`.
pub const STATE_FIELD: &str = "state";

unsafe impl Params for FxParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        self.p.iter().zip(self.spec.settings).map(|(p, s)| (s.id.to_string(), p.as_ptr(), String::new())).collect()
    }

    fn serialize_fields(&self) -> BTreeMap<String, String> {
        let hex = self.raw.read().map(|b| hex(&b)).unwrap_or_default();
        BTreeMap::from([(STATE_FIELD.to_string(), format!("\"{hex}\""))])
    }

    fn deserialize_fields(&self, serialized: &BTreeMap<String, String>) {
        if let Some(bytes) = serialized.get(STATE_FIELD).and_then(|s| from_hex(s.trim_matches('"'))) {
            self.set_raw(normalized(self.spec, bytes));
        }
    }
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    (s.len() % 2 == 0).then_some(())?;
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// The int the first setting is (1 after a version field, else 0).
pub fn first(spec: &Spec) -> usize {
    usize::from(spec.version.is_some())
}

/// In [`Spec::fields`]: the setting is not an int but bit 0 of the byte right after the ints (Fruity Filter's Center),
/// read as 0 from a state without that byte (an earlier version).
pub const BYTE: usize = byte(0);

/// In [`Spec::fields`]: bit 0 of the `n`-th byte after the ints (Fruity Peak Controller's Mute and Ramp: `byte(0)`,
/// `byte(1)`), read as 0 from a state without that byte.
pub const fn byte(n: usize) -> usize {
    usize::MAX - n
}

/// The byte after the ints a field of [`Spec::fields`] is, if it is one.
fn byte_of(i: usize) -> Option<usize> {
    (i > usize::MAX - 16).then(|| usize::MAX - i)
}

/// The int (after the version field) setting `k` is ([`BYTE`] for the byte after the ints).
pub fn int_of(spec: &Spec, k: usize) -> usize {
    spec.fields.get(k).copied().unwrap_or(k)
}

/// The ints the saved state has for its settings (after the version field): one per setting but a [`BYTE`] one, and
/// those the description leaves out.
pub fn ints(spec: &Spec) -> usize {
    let bytes = spec.fields.iter().filter(|&&i| byte_of(i).is_some()).count();
    spec.fields.iter().filter(|&&i| byte_of(i).is_none()).map(|&i| i + 1).max().unwrap_or(0).max(spec.settings.len() - bytes)
}

/// Where the `n`-th byte setting is: that byte after the ints.
fn byte_at(spec: &Spec, n: usize) -> usize {
    4 * (first(spec) + ints(spec)) + n
}

/// The shortest readable saved state: the version field, if any, and the settings' ints.
pub fn state_len(spec: &Spec) -> usize {
    4 * (first(spec) + ints(spec))
}

/// A readable saved state as it is (longer ones keep their tail); a short one, or one of a later version (unless every
/// version is read), gives the start.
pub fn normalized(spec: &Spec, b: Vec<u8>) -> Vec<u8> {
    let version_read = spec.any_version || spec.version.is_none_or(|v| field(&b, 0) <= v);
    if b.len() >= state_len(spec) && version_read { b } else { default_state(spec) }
}

/// Setting `k`'s stored value.
pub fn setting(spec: &Spec, b: &[u8], k: usize) -> i32 {
    let i = int_of(spec, k);
    match byte_of(i) {
        Some(n) => b.get(byte_at(spec, n)).map_or(0, |x| i32::from(x & 1)),
        None => field(b, first(spec) + i),
    }
}

/// Writes setting `k` (a [`BYTE`] setting only where the state has its byte; the other bits of it are kept).
pub fn set_setting(spec: &Spec, b: &mut [u8], k: usize, v: i32) {
    let i = int_of(spec, k);
    match byte_of(i) {
        Some(n) => {
            if let Some(x) = b.get_mut(byte_at(spec, n)) {
                *x = (*x & !1) | (v & 1) as u8;
            }
        }
        None => set_field(b, first(spec) + i, v),
    }
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

/// A new instance's saved state (the version, every setting at its start, an int left out at 0, the tail).
pub fn default_state(spec: &Spec) -> Vec<u8> {
    let start = |i: usize| (0..spec.settings.len()).find(|&k| int_of(spec, k) == i).map_or(0, |k| spec.settings[k].start);
    let starts = (0..ints(spec)).flat_map(|i| start(i).to_le_bytes());
    spec.version.iter().flat_map(|v| v.to_le_bytes()).chain(starts).chain(spec.tail.iter().copied()).collect()
}

/// Setting `k`'s parameter value for a stored field value.
pub fn decode(spec: &Spec, k: usize, s: i32) -> i32 {
    let (min, max) = spec.settings[k].range;
    s.clamp(min, max)
}

/// The original's saved state with values `vals`: fields whose value still means the same keep their stored bytes
/// (out-of-range values), the rest are written from the parameters.
pub fn to_fl(spec: &Spec, vals: &[i32], raw: &[u8]) -> Vec<u8> {
    let mut b = normalized(spec, raw.to_vec());
    for (k, &v) in vals.iter().enumerate().take(spec.settings.len()) {
        if decode(spec, k, setting(spec, &b, k)) != v {
            // A state of the version before the byte gains it, and the version that has it.
            if let Some(n) = byte_of(int_of(spec, k)).filter(|&n| b.len() <= byte_at(spec, n)) {
                b.resize(byte_at(spec, n) + 1, 0);
                if let Some(version) = spec.version {
                    set_field(&mut b, 0, version);
                }
            }
            set_setting(spec, &mut b, k, v);
        }
    }
    b
}

/// Parameter values of a saved state.
pub fn values_of(spec: &Spec, fl: &[u8]) -> Vec<i32> {
    (0..spec.settings.len()).map(|k| decode(spec, k, setting(spec, fl, k))).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Snapping (Lemon Studio choice, spec §4 of each plugin page)

/// Where a drag at the stored value `plain` lands: on the setting's magnet when within `MAGNET_ZONE` of its range;
/// `None` = where the drag is.
pub fn snap(spec: &Spec, k: usize, plain: f32) -> Option<i32> {
    let s = &spec.settings[k];
    let m = s.magnet?;
    ((plain - m as f32).abs() <= MAGNET_ZONE * (s.range.1 - s.range.0) as f32).then_some(m)
}

/// The wheel's next stored value from `v` given the plain step `next`: a setting with a magnet stops on it when the
/// step would pass it.
pub fn snap_step(spec: &Spec, k: usize, v: i32, next: i32) -> i32 {
    match spec.settings[k].magnet {
        Some(m) if (v < m && m <= next) || (next <= m && m < v) => m,
        _ => next,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Value texts shared by the effects (Lemon Studio choices, spec §3 of each plugin page)

/// A gain as `<dB>dB` with one decimal (`-INFdB` for silence).
pub fn db_text(gain: f32) -> String {
    if gain <= 0.0 { "-INFdB".into() } else { format!("{:.1}dB", 20.0 * f64::from(gain).log10()) }
}

/// As [`db_text`], with levels just under unity (that round to -0.0) shown as 0.0dB, so the text typed back gives a
/// value with the same text.
pub fn db_text_fine(gain: f32) -> String {
    match db_text(gain) {
        t if t == "-0.0dB" => "0.0dB".into(),
        t => t,
    }
}

/// The number a typed text starts with.
pub fn leading_number(s: &str) -> Option<f64> {
    let lead: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    lead.parse().ok()
}

/// The value in `range` whose number (`None` = it has none) is nearest to `x`.
pub fn nearest(range: (i32, i32), x: f64, number: impl Fn(i32) -> Option<f64>) -> Option<i32> {
    (range.0..=range.1)
        .filter_map(|v| number(v).map(|n| (v, (n - x).abs())))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(v, _)| v)
}

/// The value in `range` whose number is nearest to `x`, for a `number` that never falls as the value rises (a binary
/// search: for the long ranges).
pub fn nearest_rising(range: (i32, i32), x: f64, number: impl Fn(i32) -> f64) -> i32 {
    let (mut lo, mut hi) = range;
    // The first value whose number reaches x.
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if number(mid) < x { lo = mid + 1 } else { hi = mid }
    }
    if lo > range.0 && (x - number(lo - 1)).abs() <= (number(lo) - x).abs() { lo - 1 } else { lo }
}

/// A typed gain: `-inf` (or `off`) gives the value of silence, a number of dB the nearest value.
pub fn parse_db(range: (i32, i32), s: &str, gain: impl Fn(i32) -> f32) -> Option<i32> {
    let s = s.trim().to_ascii_lowercase();
    if s.contains("inf") || s == "off" {
        return (range.0..=range.1).find(|&v| gain(v) <= 0.0);
    }
    let x = leading_number(&s)?;
    nearest(range, x, |v| {
        let g = gain(v);
        (g > 0.0).then(|| 20.0 * f64::from(g).log10())
    })
}

/// As [`parse_db`], for a `gain` that never falls as the value rises (a binary search: for the long ranges).
pub fn parse_db_rising(range: (i32, i32), s: &str, gain: impl Fn(i32) -> f32) -> Option<i32> {
    let s = s.trim().to_ascii_lowercase();
    if s.contains("inf") || s == "off" {
        return (gain(range.0) <= 0.0).then_some(range.0);
    }
    let x = leading_number(&s)?;
    Some(nearest_rising(range, x, |v| {
        let g = gain(v);
        if g > 0.0 { 20.0 * f64::from(g).log10() } else { f64::NEG_INFINITY }
    }))
}

// ---------------------------------------------------------------------------------------------------------------
// Plugin state <-> the original's saved state

/// The plugin's own saved state (nih-plug JSON with the u64 length prefix of its CLAP state) for a slot's saved state.
pub fn plugin_state_from_fl(spec: &Spec, fl: &[u8]) -> Vec<u8> {
    let b = normalized(spec, fl.to_vec());
    let params: Vec<String> =
        values_of(spec, &b).iter().zip(spec.settings).map(|(v, s)| format!("\"{}\":{{\"i32\":{v}}}", s.id)).collect();
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
pub fn fl_from_plugin_state(spec: &Spec, blob: &[u8]) -> Option<Vec<u8>> {
    let json = if blob.len() > 8 && blob[0] != b'{' { &blob[8..] } else { blob };
    let v: serde_json::Value = serde_json::from_str(std::str::from_utf8(json).ok()?).ok()?;
    let raw = normalized(spec, from_hex(v.get("fields")?.get(STATE_FIELD)?.as_str()?.trim_matches('"'))?);
    let params = v.get("params")?;
    let mut vals = values_of(spec, &raw);
    for (k, s) in spec.settings.iter().enumerate() {
        if let Some(x) = params.get(s.id).and_then(|p| p.get("i32")).and_then(|x| x.as_i64()) {
            vals[k] = x as i32;
        }
    }
    Some(to_fl(spec, &vals, &raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{balance, chorus, fast_distortion, flanger, phaser, soft_clipper, stereo_enhancer, stereo_shaper, transient_processor};

    #[test]
    fn a_new_instance_starts_on_the_originals_default_and_its_state_reads_back() {
        for spec in crate::tests::SPECS {
            let b = default_state(spec);
            assert_eq!(b.len(), state_len(spec) + spec.tail.len());
            assert_eq!(values_of(spec, &b), spec.start());
            let p = FxParams::new(spec);
            assert_eq!(p.param_map().len(), spec.settings.len());
            assert_eq!(&p.settings(&b)[..spec.settings.len()], &spec.start()[..]);
            assert_eq!(p.fl_state(), b);
        }
        // The engine's defaults are the originals' Default presets.
        assert_eq!(default_state(soft_clipper::SPEC), os_engine::effects::soft_clipper::Settings::default().to_state());
        assert_eq!(default_state(fast_distortion::SPEC), os_engine::effects::fast_dist::Settings::default().to_state());
        assert_eq!(default_state(balance::SPEC), os_engine::effects::balance::Settings::default().to_state());
        assert_eq!(default_state(stereo_enhancer::SPEC), os_engine::effects::stereo_enhancer::Settings::default().to_state());
        assert_eq!(
            default_state(transient_processor::SPEC),
            os_engine::effects::transient_processor::Settings::default().to_state()
        );
        assert_eq!(default_state(chorus::SPEC), os_engine::effects::chorus::Settings::default().to_state());
        assert_eq!(default_state(stereo_shaper::SPEC), os_engine::effects::stereo_shaper::Settings::default().to_state());
        assert_eq!(default_state(crate::filter::SPEC), os_engine::effects::filter::Settings::default().to_state());
        assert_eq!(default_state(flanger::SPEC), os_engine::effects::flanger::Settings::default().to_state());
        assert_eq!(default_state(phaser::SPEC), os_engine::effects::phaser::Settings::default().to_state());
    }

    /// Lemon Stereo Shaper leaves the difference output (int 6 after the version) out of its settings: it is kept as
    /// stored; any version is read (spec plugins/lemon-stereo-shaper.md §5).
    #[test]
    fn a_mapped_state_keeps_the_ints_it_leaves_out() {
        let spec = stereo_shaper::SPEC;
        assert_eq!((ints(spec), state_len(spec)), (8, 36));
        // FL's "Mid - aSide splitter": difference output 2.
        let fl = os_engine::effects::stereo_shaper::Settings { v: [8076, 8076, 8076, 8076, 0, 0, 2, 1] }.to_state();
        let mut vals = values_of(spec, &fl);
        assert_eq!(vals, [8076, 8076, 8076, 8076, 0, 0, 1]);
        vals[6] = -1;
        vals[0] = 12800;
        let b = to_fl(spec, &vals, &fl);
        assert_eq!((field(&b, 1), field(&b, 7), field(&b, 8)), (12800, 2, -1));
        let mut v9 = b.clone();
        set_field(&mut v9, 0, 9);
        assert_eq!(normalized(spec, v9.clone()), v9);
        assert_eq!(normalized(spec, b[..32].to_vec()), default_state(spec));
        let blob = plugin_state_from_fl(spec, &b);
        assert_eq!(fl_from_plugin_state(spec, &blob), Some(b));
    }

    #[test]
    fn the_binary_search_finds_the_nearest() {
        let sq = |v: i32| f64::from(v) * f64::from(v);
        assert_eq!(nearest_rising((0, 100), 50.0, sq), 7);
        assert_eq!(nearest_rising((0, 100), 56.0, sq), 7);
        assert_eq!(nearest_rising((0, 100), 57.0, sq), 8);
        assert_eq!(nearest_rising((0, 100), -3.0, sq), 0);
        assert_eq!(nearest_rising((0, 100), 1e9, sq), 100);
        assert_eq!(nearest_rising((-5, 5), 0.2, f64::from), 0);
    }

    /// Transient Processor's state: a version first, display bytes after; versions 0 and 1 read, 2 not (spec
    /// plugins/lemon-transient-processor.md §5).
    #[test]
    fn a_versioned_state_reads_after_its_version_and_keeps_its_tail() {
        let spec = transient_processor::SPEC;
        let mut fl = default_state(spec);
        assert_eq!(fl.len(), 49);
        set_setting(spec, &mut fl, 0, 200); // attack above the range: kept
        fl[45] = 77; // the display's scroll speed: kept
        let mut vals = values_of(spec, &fl);
        assert_eq!(vals[0], 160);
        vals[1] = 40;
        let b = to_fl(spec, &vals, &fl);
        assert_eq!((field(&b, 0), field(&b, 1), field(&b, 2)), (1, 200, 40));
        assert_eq!(b[45], 77);
        let mut v0 = fl.clone();
        set_field(&mut v0, 0, 0);
        assert_eq!(normalized(spec, v0.clone()), v0);
        let mut v2 = fl.clone();
        set_field(&mut v2, 0, 2);
        assert_eq!(normalized(spec, v2), default_state(spec));
        // The settings without the display bytes read; fewer do not.
        assert_eq!(normalized(spec, fl[..40].to_vec()), fl[..40].to_vec());
        assert_eq!(normalized(spec, fl[..39].to_vec()), default_state(spec));
    }

    #[test]
    fn state_round_trips_through_the_plugin_state_and_keeps_stored_values() {
        let spec = stereo_enhancer::SPEC;
        let mut fl = default_state(spec);
        set_field(&mut fl, 0, -40);
        set_field(&mut fl, 3, 900); // a phase offset above the range: kept
        let blob = plugin_state_from_fl(spec, &fl);
        assert_eq!(fl_from_plugin_state(spec, &blob).expect("parses"), fl);
        let p = FxParams::new(spec);
        let json: serde_json::Value = serde_json::from_slice(&blob[8..]).unwrap();
        let fields: BTreeMap<String, String> =
            json["fields"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        p.deserialize_fields(&fields);
        assert_eq!(*p.raw.read().unwrap(), fl);
        let mut vals = values_of(spec, &fl);
        assert_eq!(vals[3], 512);
        vals[0] = 10;
        assert_eq!(field(&to_fl(spec, &vals, &fl), 0), 10);
        assert_eq!(field(&to_fl(spec, &vals, &fl), 3), 900);
        // A short state is not read.
        assert_eq!(normalized(spec, vec![1, 2, 3]), default_state(spec));
        assert_eq!(fl_from_plugin_state(spec, b"{\"params\":{}}"), None);
    }

    #[test]
    fn magnets_catch_drags_and_wheel_steps() {
        let spec = balance::SPEC;
        // Balance: the centre, within 1.5 % of the range (3.84 of 256).
        assert_eq!(snap(spec, 0, 3.0), Some(0));
        assert_eq!(snap(spec, 0, 4.5), None);
        assert_eq!(snap_step(spec, 0, -2, 3), 0);
        assert_eq!(snap_step(spec, 0, 0, 5), 5);
        // Volume: unity (256).
        assert_eq!(snap(spec, 1, 252.0), Some(256));
        assert_eq!(snap_step(spec, 1, 250, 258), 256);
    }

    #[test]
    fn gains_read_and_parse_in_db() {
        assert_eq!(db_text(1.0), "0.0dB");
        assert_eq!(db_text(0.0), "-INFdB");
        assert_eq!(db_text(0.5), "-6.0dB");
        let gain = |v: i32| v as f32 / 100.0;
        assert_eq!(parse_db((0, 200), "-inf", gain), Some(0));
        assert_eq!(parse_db((0, 200), "-6", gain), Some(50));
        assert_eq!(parse_db((0, 200), "0 dB", gain), Some(100));
        assert_eq!(parse_db((0, 200), "loud", gain), None);
        assert_eq!(leading_number(" 12.5%"), Some(12.5));
    }
}
