// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Sampler parameters. Every control of the editor is a host-visible parameter, addressed by its id. Most are
//! stored as the knob position 0..1 (FL's knobs are positions too) and converted where they are used; the value
//! texts follow FL's hint bar.
//!
// spec: plugins/lemon-sampler.md (§Controls: ids, ranges, defaults, value texts)

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use nih_plug::prelude::*;
use os_engine::envelope;
use os_model::SampleFx;

pub type Fmt = Arc<dyn Fn(f32) -> String + Send + Sync>;

/// Editor edits the host has not handed back yet (spec: plugins/lemon-sampler.md §4 "Edits apply at once").
///
/// The editor tells the host about every edit; nih-plug then changes the parameter only when the host passes the edit
/// back (VST3 while processing) or flushes (CLAP), and some hosts do neither. So the sound reads parameters through
/// [`Live::live`]: the editor's newest value wins until the host's own value moves to it (confirmed) or away from the
/// value it had at the edit (automation / undo in the host). Entries are keyed by the parameter's address, unique per
/// plugin instance.
static LIVE: std::sync::Mutex<Vec<LiveEdit>> = std::sync::Mutex::new(Vec::new());

#[derive(Clone, Copy)]
struct LiveEdit {
    addr: usize,
    /// The editor's value (normalised).
    norm: f32,
    /// The host's value when the edit was made.
    base: f32,
}

/// Records an editor edit of the parameter at `addr` whose host value is `host` now.
pub fn live_set(addr: usize, norm: f32, host: f32) {
    if let Ok(mut l) = LIVE.lock() {
        l.retain(|e| e.addr != addr);
        if (norm - host).abs() > 1e-7 {
            l.push(LiveEdit { addr, norm, base: host });
        }
    }
}

/// The normalised value the sound should use for the parameter at `addr` (`host` = its host value), or `None` to use
/// the host's. Never blocks: when the editor holds the list, the host value is used for this block.
pub fn live_norm(addr: usize, host: f32) -> Option<f32> {
    let mut l = LIVE.try_lock().ok()?;
    let i = l.iter().position(|e| e.addr == addr)?;
    let e = l[i];
    if (host - e.base).abs() <= 1e-7 && (host - e.norm).abs() > 1e-7 {
        Some(e.norm)
    } else {
        // Confirmed, or the host changed it since: the host's value from now on.
        l.swap_remove(i);
        None
    }
}

/// Forgets the edits of parameters inside `from..to` (a dropped instance).
fn live_forget(from: usize, to: usize) {
    if let Ok(mut l) = LIVE.lock() {
        l.retain(|e| e.addr < from || e.addr >= to);
    }
}

/// The normalised value the sound uses for `p` (its pending editor edit, else the host's value).
pub fn live_of(p: ParamPtr) -> f32 {
    // SAFETY: callers hold pointers from a live params object.
    let host = unsafe { p.unmodulated_normalized_value() };
    live_norm(param_addr(p), host).unwrap_or(host)
}

/// The address `live_set` keys a parameter by.
pub fn param_addr(p: ParamPtr) -> usize {
    match p {
        ParamPtr::FloatParam(p) => p as usize,
        ParamPtr::IntParam(p) => p as usize,
        ParamPtr::BoolParam(p) => p as usize,
        ParamPtr::EnumParam(p) => p as usize,
    }
}

/// Parameter values for the sound (see [`LIVE`]).
pub trait Live {
    type Plain;
    fn live(&self) -> Self::Plain;
}

impl Live for FloatParam {
    type Plain = f32;
    fn live(&self) -> f32 {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}

impl Live for IntParam {
    type Plain = i32;
    fn live(&self) -> i32 {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}

impl Live for BoolParam {
    type Plain = bool;
    fn live(&self) -> bool {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}


fn percent() -> Fmt {
    Arc::new(|v| format!("{:.0}%", v * 100.0))
}

/// "N %" (with a space: SMP START / LENGTH, as FL writes them).
fn percent_sp() -> Fmt {
    Arc::new(|v| format!("{:.0} %", v * 100.0))
}

/// A -1..1 value in percent ("-79%").
fn signed_percent() -> Fmt {
    Arc::new(|v| format!("{:.0}%", v * 100.0))
}

fn side_text() -> Fmt {
    Arc::new(|v| {
        let p = (v * 100.0).round();
        if p == 0.0 { "Centered".into() } else if p < 0.0 { format!("{:.0}% left", -p) } else { format!("{p:.0}% right") }
    })
}

/// Poly modulation id of "Sample start offset": per-note sample start.
pub const START_OFFSET_POLY: u32 = 0;
/// Poly modulation ids of the filter knobs: a host adds a note's mod X / mod Y to them per note, in knob units (spec:
/// audio/voice-levels.md "Final levels", audio/channel-filter.md "Knob positions").
pub const MOD_X_POLY: u32 = 1;
pub const MOD_Y_POLY: u32 = 2;

/// A 0..1 knob shown in percent.
pub fn knob(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 }).with_value_to_string(percent())
}

/// A -1..1 knob (centre = 0) shown in percent.
pub fn bi_knob(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: -1.0, max: 1.0 }).with_value_to_string(signed_percent())
}

/// A choice list stored as an integer (the label is the value text).
fn choice(name: &str, default: i32, labels: &'static [&'static str]) -> IntParam {
    IntParam::new(name, default, IntRange::Linear { min: 0, max: labels.len() as i32 - 1 })
        .with_value_to_string(Arc::new(move |v| labels.get(v as usize).copied().unwrap_or("?").to_string()))
}

/// Envelope / LFO time knobs: the knob position v is the channel-envelope field v·65536 (minimum 100 = none, spec
/// audio/channel-envelopes.md); FL shows the position in percent.
pub fn time_field(norm: f32) -> i32 {
    ((norm.clamp(0.0, 1.0) * 65536.0).round() as i32).max(100)
}

fn field_norm(field: i32) -> f32 {
    field as f32 / 65536.0
}

pub const NOTE_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// FL note naming: key 60 = C5.
pub fn note_name(key: i32) -> String {
    format!("{}{}", NOTE_NAMES[key.rem_euclid(12) as usize], key.div_euclid(12))
}

pub const DECLICK: &[&str] = &[
    "Out only (no bleeding)",
    "Transient (no bleeding)",
    "Transient (bleeding)",
    "Generic (bleeding)",
    "Smooth (bleeding)",
    "Crossfade (bleeding)",
];
/// Stretch modes in the order of the mode menu (spec §Menus: two columns with headers).
pub const STRETCH: &[&str] =
    &["Resample", "Stretch", "Stretch Pro", "e3 generic", "e3 mono", "Slice stretch", "Slice map", "Auto", "e2 generic", "e2 transient", "e2 mono", "e2 speech"];
pub const STRETCH_AUTO: i32 = 7;
/// The stretch method each mode menu entry stores (event 215 int 27; spec: audio/time-stretch.md "Fields").
pub const STRETCH_FILE: [i32; 12] = [0, -1, -2, 1, 2, 3, 4, 5, 6, 7, 8, 9];

/// Mode menu index of a stored stretch method (values not known: Auto).
pub fn stretch_index(file: i32) -> i32 {
    STRETCH_FILE.iter().position(|&m| m == file).map_or(STRETCH_AUTO, |i| i as i32)
}

/// Stretch and Stretch Pro: the voice stretches while it plays, the key and pitch moving only the pitch (spec §5.6).
pub fn stretch_realtime(index: i32) -> bool {
    matches!(index, 1 | 2)
}

/// The methods that stretch the sample once before it plays (spec §5.6).
pub fn stretch_whole(index: i32) -> bool {
    index >= 3
}
pub const FILTERS: &[&str] = &["Fast LP", "LP", "BP", "HP", "BS", "LPx2", "SVF LP", "SVF LPx2"];
pub const LFO_SHAPES: &[&str] = &["Sine", "Triangle", "Pulse"];
pub const REVERB_TYPES: &[&str] = &["A", "B"];

/// Highest key (C0 = 0 .. B10 = 131).
pub const KEY_MAX: i32 = 131;

/// Time stretch multiplier of a knob position: 25 %..400 %, doubling every quarter (FL: 100 px per octave). Positions
/// are clamped: a saved state is loaded unchecked, and an out-of-range value must not reach the stretcher.
pub fn mul_of(v: f32) -> f64 {
    2f64.powf(4.0 * (f64::from(v.clamp(0.0, 1.0)) - 0.5))
}

/// Time stretch pitch in cents of a knob position (±1200, clamped like [`mul_of`]).
pub fn ts_cents(v: f32) -> f64 {
    (f64::from(v.clamp(0.0, 1.0)) * 2400.0 - 1200.0).round()
}

/// TIME knob: the longest time in bars (a Lemon Studio limit).
pub const TIME_BARS: f64 = 1024.0;

/// TIME knob: pixels of drag over its whole range. The first 16 px are one step (a quarter beat) each, then one bar per
/// 36 px (spec §4).
pub const TIME_PX: f32 = (16.0 + (TIME_BARS - 1.0) * 36.0) as f32;

/// TIME knob: steps (quarter beats) of a knob position (0 = off). Drags land on whole steps (`time_snap`); times set
/// from a project or the playlist keep their fractions.
pub fn time_steps(v: f32) -> f64 {
    let p = f64::from(v.clamp(0.0, 1.0)) * f64::from(TIME_PX);
    if p < 16.0 {
        p
    } else {
        16.0 + (p - 16.0) / 36.0 * 16.0
    }
}

/// Knob position of a TIME value in steps (inverse of `time_steps`).
pub fn time_norm(steps: f64) -> f32 {
    let p = if steps < 16.0 { steps.max(0.0) } else { 16.0 + (steps - 16.0) / 16.0 * 36.0 };
    (p / f64::from(TIME_PX)).clamp(0.0, 1.0) as f32
}

/// The knob position at the nearest whole step.
pub fn time_snap(v: f32) -> f32 {
    time_norm(time_steps(v).round())
}

/// "bars:steps:ticks" as FL shows the TIME value (24 ticks a step; spec §2.1).
pub fn time_text(steps: f64) -> String {
    let ticks = (steps.max(0.0) * 24.0).round() as u64;
    format!("{}:{:02}:{:02}", ticks / (16 * 24), ticks / 24 % 16, ticks % 24)
}

/// TIME as event 215 int 24 stores it: a single in 1/192 beat (0 = off).
pub fn time_file(steps: f64) -> f32 {
    (steps * 48.0) as f32
}

/// Steps of a stored TIME (1/192 beat).
pub fn time_from_file(units: f32) -> f64 {
    if units.is_finite() && units >= 1.0 { f64::from(units) / 48.0 } else { 0.0 }
}

/// One envelope / LFO target of the envelope tab (Panning, Volume, Mod X, Mod Y, Pitch).
#[derive(Params)]
pub struct EnvParams {
    #[id = "on"]
    pub on: BoolParam,
    #[id = "delay"]
    pub delay: FloatParam,
    #[id = "att"]
    pub att: FloatParam,
    #[id = "hold"]
    pub hold: FloatParam,
    #[id = "dec"]
    pub dec: FloatParam,
    #[id = "sus"]
    pub sus: FloatParam,
    #[id = "rel"]
    pub rel: FloatParam,
    #[id = "att_t"]
    pub att_t: FloatParam,
    #[id = "dec_t"]
    pub dec_t: FloatParam,
    #[id = "rel_t"]
    pub rel_t: FloatParam,
    /// Envelope amount (Mod X / Mod Y / Pitch; pitch: ±2400 cents at full scale).
    #[id = "amt"]
    pub amt: FloatParam,
    #[id = "tempo"]
    pub tempo: BoolParam,
    #[id = "lfo_shape"]
    pub lfo_shape: IntParam,
    #[id = "lfo_delay"]
    pub lfo_delay: FloatParam,
    #[id = "lfo_att"]
    pub lfo_att: FloatParam,
    #[id = "lfo_amt"]
    pub lfo_amt: FloatParam,
    #[id = "lfo_speed"]
    pub lfo_speed: FloatParam,
    #[id = "lfo_tempo"]
    pub lfo_tempo: BoolParam,
    #[id = "lfo_global"]
    pub lfo_global: BoolParam,
}

impl EnvParams {
    fn new(target: &str, rel_tension: i32) -> Self {
        let n = |s: &str| format!("{target} {s}");
        let d = os_model::Envelope::default();
        let time = |s: &str, field: i32| knob(&n(s), field_norm(field));
        EnvParams {
            on: BoolParam::new(n("envelope"), false),
            delay: time("delay", d.delay),
            att: time("attack", d.attack),
            hold: time("hold", d.hold),
            dec: time("decay", d.decay),
            sus: knob(&n("sustain"), d.sustain as f32 / 128.0),
            rel: time("release", d.release),
            att_t: bi_knob(&n("attack tension"), 0.0),
            dec_t: bi_knob(&n("decay tension"), 0.0),
            rel_t: bi_knob(&n("release tension"), rel_tension as f32 / 128.0),
            amt: bi_knob(&n("amount"), 0.0),
            tempo: BoolParam::new(n("tempo"), false),
            lfo_shape: choice(&n("LFO shape"), 0, LFO_SHAPES),
            lfo_delay: knob(&n("LFO delay"), 0.0),
            lfo_att: knob(&n("LFO attack"), field_norm(20000)),
            lfo_amt: bi_knob(&n("LFO amount"), 0.0),
            lfo_speed: knob(&n("LFO speed"), field_norm(32950)),
            lfo_tempo: BoolParam::new(n("LFO tempo"), false),
            lfo_global: BoolParam::new(n("LFO global"), false),
        }
    }

    /// The record in channel-envelope units, `None` when off. `amount_scale` = units of the amount at full scale.
    pub fn envelope(&self, amount_scale: f32) -> Option<envelope::Params> {
        let rec = os_model::Envelope {
            enabled: self.on.live(),
            delay: time_field(self.delay.live()),
            attack: time_field(self.att.live()),
            hold: time_field(self.hold.live()),
            decay: time_field(self.dec.live()),
            sustain: (self.sus.live() * 128.0).round() as i32,
            release: time_field(self.rel.live()),
            amount: (self.amt.live() * amount_scale).round() as i32,
            attack_tension: (self.att_t.live() * 128.0).round() as i32,
            decay_tension: (self.dec_t.live() * 128.0).round() as i32,
            release_tension: (self.rel_t.live() * 128.0).round() as i32,
            ..Default::default()
        };
        envelope::Params::new(&rec)
    }
}

#[derive(Params)]
pub struct SamplerParams {
    /// The loaded sample's path (empty = none).
    #[persist = "sample-path"]
    pub sample_path: Arc<RwLock<String>>,
    /// The precomputed settings a project file stores for the channel, as `fx_text` writes them (empty = none). They
    /// play while the knobs stand where those values put them: start and length keep their doubles, and values beyond
    /// a knob's range are kept (spec: audio/precomputed-effects.md "Settings").
    #[persist = "file-settings"]
    pub file_settings: Arc<RwLock<String>>,
    /// Frames of the loaded sample (loop knob texts are in samples).
    pub frames: Arc<AtomicU32>,

    // Tab 1: sample.
    #[id = "keep_disk"]
    pub keep_disk: BoolParam,
    #[id = "resample"]
    pub resample: BoolParam,
    #[id = "load_regions"]
    pub load_regions: BoolParam,
    #[id = "load_slices"]
    pub load_slices: BoolParam,
    #[id = "declick"]
    pub declick: IntParam,
    #[id = "start_offset"]
    pub start_offset: FloatParam,
    #[id = "loop_start"]
    pub loop_start: FloatParam,
    #[id = "loop_end"]
    pub loop_end: FloatParam,
    #[id = "loop_use"]
    pub loop_use: BoolParam,
    #[id = "loop_pingpong"]
    pub loop_pingpong: BoolParam,
    #[id = "ts_pitch"]
    pub ts_pitch: FloatParam,
    #[id = "ts_mul"]
    pub ts_mul: FloatParam,
    #[id = "ts_time"]
    pub ts_time: FloatParam,
    #[id = "ts_mode"]
    pub ts_mode: IntParam,
    #[id = "fx_dc"]
    pub fx_dc: BoolParam,
    #[id = "fx_normalize"]
    pub fx_normalize: BoolParam,
    #[id = "fx_reverse"]
    pub fx_reverse: BoolParam,
    #[id = "fx_polarity"]
    pub fx_polarity: BoolParam,
    #[id = "fx_fade_stereo"]
    pub fx_fade_stereo: BoolParam,
    #[id = "fx_swap"]
    pub fx_swap: BoolParam,
    #[id = "fx_start"]
    pub fx_start: FloatParam,
    #[id = "fx_length"]
    pub fx_length: FloatParam,
    #[id = "fx_in"]
    pub fx_in: FloatParam,
    #[id = "fx_out"]
    pub fx_out: FloatParam,
    #[id = "fx_crossfade"]
    pub fx_crossfade: FloatParam,
    #[id = "fx_trim"]
    pub fx_trim: FloatParam,
    // Precomputed effects, effects page.
    #[id = "fx_boost"]
    pub fx_boost: FloatParam,
    #[id = "fx_eq"]
    pub fx_eq: FloatParam,
    #[id = "fx_rm_mix"]
    pub fx_rm_mix: FloatParam,
    #[id = "fx_rm_freq"]
    pub fx_rm_freq: FloatParam,
    #[id = "fx_cut"]
    pub fx_cut: FloatParam,
    #[id = "fx_res"]
    pub fx_res: FloatParam,
    #[id = "fx_clip"]
    pub fx_clip: BoolParam,
    #[id = "fx_reverb_type"]
    pub fx_reverb_type: IntParam,
    #[id = "fx_reverb"]
    pub fx_reverb: FloatParam,
    #[id = "fx_sdelay"]
    pub fx_sdelay: FloatParam,
    #[id = "fx_pogo"]
    pub fx_pogo: FloatParam,
    /// The loop points move with a reversal (event 215 byte 156; no control on the page).
    #[id = "fx_loop_follows"]
    pub fx_loop_follows: BoolParam,

    // Tab 2: envelope / instrument.
    #[nested(id_prefix = "envpan", group = "Panning")]
    pub env_pan: EnvParams,
    #[nested(id_prefix = "envvol", group = "Volume")]
    pub env_vol: EnvParams,
    #[nested(id_prefix = "envmodx", group = "Mod X")]
    pub env_modx: EnvParams,
    #[nested(id_prefix = "envmody", group = "Mod Y")]
    pub env_mody: EnvParams,
    #[nested(id_prefix = "envpitch", group = "Pitch")]
    pub env_pitch: EnvParams,
    #[id = "f_cut"]
    pub f_cut: FloatParam,
    #[id = "f_res"]
    pub f_res: FloatParam,
    #[id = "f_type"]
    pub f_type: IntParam,
    #[id = "root"]
    pub root: IntParam,
    #[id = "fine"]
    pub fine: IntParam,
    #[id = "main_pitch"]
    pub main_pitch: BoolParam,
    #[id = "add_key"]
    pub add_key: BoolParam,
    /// Key zone (ruler selection): lowest and highest key that play; the full range = no zone.
    #[id = "zone_lo"]
    pub zone_lo: IntParam,
    #[id = "zone_hi"]
    pub zone_hi: IntParam,
}

impl Drop for SamplerParams {
    fn drop(&mut self) {
        let from = self as *const Self as usize;
        live_forget(from, from + std::mem::size_of::<Self>());
    }
}

impl Default for SamplerParams {
    fn default() -> Self {
        let frames = Arc::new(AtomicU32::new(0));
        let samples = |f: Arc<AtomicU32>| -> Fmt {
            Arc::new(move |v| {
                let n = f.load(Ordering::Relaxed);
                format!("{}", (v * n as f32).round() as i64)
            })
        };
        SamplerParams {
            sample_path: Arc::new(RwLock::new(String::new())),
            file_settings: Arc::new(RwLock::new(String::new())),
            keep_disk: BoolParam::new("Keep on disk", false),
            resample: BoolParam::new("Resample", true),
            load_regions: BoolParam::new("Load regions", true),
            load_slices: BoolParam::new("Load slice markers", false),
            declick: choice("Declicking mode", 0, DECLICK),
            // A host can start one note further into the sample (a trimmed audio clip, spec §7).
            start_offset: knob("Sample start offset", 0.0).with_poly_modulation_id(START_OFFSET_POLY),
            loop_start: FloatParam::new("Loop start position", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(samples(frames.clone())),
            loop_end: FloatParam::new("Loop end position", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(samples(frames.clone())),
            loop_use: BoolParam::new("Use loop points", true),
            loop_pingpong: BoolParam::new("Ping pong loop", false),
            ts_pitch: FloatParam::new("Pitch shift", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(Arc::new(|v| {
                    let c = ts_cents(v);
                    if c == 0.0 { "0 cents".into() } else { format!("{c:+.0} cents") }
                })),
            ts_mul: FloatParam::new("Time stretch (multiplicator)", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(Arc::new(|v| format!("{:.2}%", mul_of(v) * 100.0))),
            ts_time: FloatParam::new("Time stretch (time)", 0.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_value_to_string(Arc::new(|v| time_text(time_steps(v)))),
            ts_mode: choice("Stretching method", 0, STRETCH),
            fx_dc: BoolParam::new("Remove DC offset", false),
            fx_normalize: BoolParam::new("Normalize", false),
            fx_reverse: BoolParam::new("Reverse", false),
            fx_polarity: BoolParam::new("Reverse polarity", false),
            fx_fade_stereo: BoolParam::new("Fade stereo", false),
            fx_swap: BoolParam::new("Swap stereo", false),
            fx_start: knob("Sample start", 0.0).with_value_to_string(percent_sp()),
            fx_length: knob("Sample end", 1.0).with_value_to_string(percent_sp()),
            fx_in: knob("Quick fade in", 0.0),
            fx_out: knob("Quick fade out", 0.0),
            fx_crossfade: knob("Crossfade loop", 0.0),
            fx_trim: knob("Silence trimming threshold", 0.0),
            fx_boost: knob("Boost", 0.0),
            fx_eq: bi_knob("EQ", 0.0),
            fx_rm_mix: knob("Ring modulation mix", 0.0),
            fx_rm_freq: knob("Ring modulation frequency", 0.5),
            fx_cut: knob("Filter cutoff", 1.0),
            fx_res: knob("Filter resonance", 0.0),
            fx_clip: BoolParam::new("Clipping", false),
            fx_reverb_type: choice("Reverb type", 1, REVERB_TYPES),
            fx_reverb: knob("Reverb amount", 0.0),
            fx_sdelay: FloatParam::new("Stereo delay", 0.0, FloatRange::Linear { min: -1.0, max: 1.0 }).with_value_to_string(side_text()),
            fx_pogo: bi_knob("Pogo effect", 0.0),
            // (Lemon Studio) On for a new channel: what most project files store.
            fx_loop_follows: BoolParam::new("Loop points follow a reversal", true).hide(),
            env_pan: EnvParams::new("Panning", 0),
            env_vol: EnvParams::new("Volume", -101),
            env_modx: EnvParams::new("Mod X", 0),
            env_mody: EnvParams::new("Mod Y", 0),
            env_pitch: EnvParams::new("Pitch", 0),
            f_cut: knob("Filter frequency (modulation X)", 1.0).with_poly_modulation_id(MOD_X_POLY),
            f_res: knob("Filter bandwidth (modulation Y)", 0.0).with_poly_modulation_id(MOD_Y_POLY),
            f_type: choice("Filter type", 0, FILTERS),
            root: IntParam::new("Root note", 60, IntRange::Linear { min: 0, max: KEY_MAX }).with_value_to_string(Arc::new(note_name)),
            fine: IntParam::new("Fine tuning", 0, IntRange::Linear { min: -100, max: 100 }).with_value_to_string(Arc::new(|v| format!("{v} cents"))),
            main_pitch: BoolParam::new("Affected by the main pitch", true),
            add_key: BoolParam::new("Add root note to key", false),
            zone_lo: IntParam::new("Zone low key", 0, IntRange::Linear { min: 0, max: KEY_MAX }).with_value_to_string(Arc::new(note_name)).hide(),
            zone_hi: IntParam::new("Zone high key", KEY_MAX, IntRange::Linear { min: 0, max: KEY_MAX })
                .with_value_to_string(Arc::new(note_name))
                .hide(),
            frames,
        }
    }
}

impl SamplerParams {
    /// The envelope tab's five targets in sub-tab order.
    pub fn env(&self, target: usize) -> &EnvParams {
        [&self.env_pan, &self.env_vol, &self.env_modx, &self.env_mody, &self.env_pitch][target.min(4)]
    }

    /// True when the ruler selected a key zone (not the full range).
    pub fn has_zone(&self) -> bool {
        self.zone_lo.live() > 0 || self.zone_hi.live() < KEY_MAX
    }
}

/// A precomputed setting's knob position and the value a project file stores for it (spec:
/// audio/precomputed-effects.md "Settings"): stored = base + Round(knob * span), limited to 0..=max; a stored value
/// puts the knob at (stored - base) / span, limited to the knob's range lo..1. **(Lemon Studio)** The spans are the
/// knobs' ranges from the sample page's measured laws (plugins/lemon-sampler.md §5.4, §5.5) brought to the stored units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub base: f64,
    pub span: f64,
    pub lo: f32,
    pub max: f64,
}

impl Unit {
    const fn new(base: f64, span: f64, lo: f32, max: f64) -> Unit {
        Unit { base, span, lo, max }
    }

    /// The stored value of a knob position.
    pub fn stored(&self, knob: f32) -> u32 {
        let k = f64::from(if knob.is_finite() { knob.clamp(self.lo, 1.0) } else { 0.0 });
        (self.base + (k * self.span).round_ties_even()).clamp(0.0, self.max) as u32
    }

    /// The knob position of a stored value.
    pub fn knob(&self, stored: u32) -> f32 {
        (((f64::from(stored) - self.base) / self.span) as f32).clamp(self.lo, 1.0)
    }
}

/// The precomputed settings' units (stored values in the file's own scales).
pub mod unit {
    use super::Unit;
    const W: f64 = 65535.0;
    /// Quick fade in / out: 256 = silent at the far end; the knob's 100 % reaches silence at a quarter.
    pub const FADE: Unit = Unit::new(0.0, 1024.0, 0.0, W);
    /// Boost: gain `v / 16 + 1`, 17 at 100 %.
    pub const BOOST: Unit = Unit::new(0.0, 256.0, 0.0, W);
    /// Stereo delay: centre 2048, 100 % right = the left side 2048 frames late (at 44100 Hz).
    pub const STEREO_DELAY: Unit = Unit::new(2048.0, 2048.0, -1.0, W);
    /// Pogo: centre 256.
    pub const POGO: Unit = Unit::new(256.0, 256.0, -1.0, W);
    /// EQ (the one-pole feedback): centre 128.
    pub const FEEDBACK: Unit = Unit::new(128.0, 128.0, -1.0, W);
    /// CUT: 0 % = 128 (the lowest value the spec tried; the knob's own minimum is not known), 100 % = 1024 = open.
    pub const CUTOFF: Unit = Unit::new(128.0, 896.0, 0.0, W);
    /// RES: 100 % = 640, the largest value that plays.
    pub const RESONANCE: Unit = Unit::new(0.0, 640.0, 0.0, W);
    /// RM MIX, RM FREQ, REVERB: 0..256 (RM FREQ's default 50 % = 128).
    pub const RING_MIX: Unit = Unit::new(0.0, 256.0, 0.0, W);
    pub const RING_FREQ: Unit = Unit::new(0.0, 256.0, 0.0, W);
    pub const REVERB: Unit = Unit::new(0.0, 256.0, 0.0, W);
    /// Crossfade: in 1/2048 of the loop; 100 % = half the loop.
    pub const CROSSFADE: Unit = Unit::new(0.0, 1024.0, 0.0, u32::MAX as f64);
    /// Trim: a threshold in 1/32768 of full scale; 100 % = 256 (1/128).
    pub const TRIM: Unit = Unit::new(0.0, 256.0, 0.0, u32::MAX as f64);
}

/// `SampleFx` as text (the `file-settings` field): its values in declaration order, comma-separated, the start and
/// length as the bits of their doubles.
pub fn fx_text(f: &SampleFx) -> String {
    let b = |v: bool| u64::from(v);
    let vals: [u64; 22] = [
        f.flags.into(),
        f.boost.into(),
        f.fade_out.into(),
        f.fade_in.into(),
        f.stereo_delay.into(),
        f.pogo.into(),
        f.feedback.into(),
        f.cutoff.into(),
        f.resonance.into(),
        f.ring_mix.into(),
        f.ring_freq.into(),
        f.reverb.into(),
        f.reverb_type.into(),
        b(f.ping_pong),
        b(f.remove_dc),
        b(f.normalize),
        b(f.polarity),
        f.crossfade.into(),
        f.trim.into(),
        f.start_bits,
        f.length_bits,
        b(f.loop_follows_reverse),
    ];
    vals.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")
}

/// The inverse of [`fx_text`] (`None` for empty or malformed text).
pub fn fx_from_text(t: &str) -> Option<SampleFx> {
    let mut v = [0u64; 22];
    let mut n = 0;
    for (i, part) in t.split(',').enumerate() {
        *v.get_mut(i)? = part.trim().parse().ok()?;
        n = i + 1;
    }
    if n != v.len() {
        return None;
    }
    let w = |x: u64| u16::try_from(x).ok();
    let d = |x: u64| u32::try_from(x).ok();
    Some(SampleFx {
        flags: w(v[0])?,
        boost: w(v[1])?,
        fade_out: w(v[2])?,
        fade_in: w(v[3])?,
        stereo_delay: w(v[4])?,
        pogo: w(v[5])?,
        feedback: w(v[6])?,
        cutoff: w(v[7])?,
        resonance: w(v[8])?,
        ring_mix: w(v[9])?,
        ring_freq: w(v[10])?,
        reverb: w(v[11])?,
        reverb_type: w(v[12])?,
        ping_pong: v[13] != 0,
        remove_dc: v[14] != 0,
        normalize: v[15] != 0,
        polarity: v[16] != 0,
        crossfade: d(v[17])?,
        trim: d(v[18])?,
        start_bits: v[19],
        length_bits: v[20],
        loop_follows_reverse: v[21] != 0,
    })
}

/// The precomputed settings' knob positions (plain values) and switches: what [`fx_from_knobs`] turns into the file's
/// units. The host reads them from a saved state to store a channel's settings (spec: plugins/lemon-sampler.md §7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxKnobs {
    pub fade_stereo: bool,
    pub reverse: bool,
    pub clip: bool,
    pub swap: bool,
    pub boost: f32,
    pub fade_out: f32,
    pub fade_in: f32,
    pub stereo_delay: f32,
    pub pogo: f32,
    pub eq: f32,
    pub cutoff: f32,
    pub resonance: f32,
    pub ring_mix: f32,
    pub ring_freq: f32,
    pub reverb: f32,
    pub reverb_type: i32,
    pub ping_pong: bool,
    pub remove_dc: bool,
    pub normalize: bool,
    pub polarity: bool,
    pub crossfade: f32,
    pub trim: f32,
    pub start: f32,
    pub length: f32,
    pub loop_follows: bool,
}

/// The precomputed settings in the file's units: each from its knob, or `file`'s own value while the knob stands where
/// that value puts it.
pub fn fx_from_knobs(k: &FxKnobs, file: Option<&SampleFx>) -> SampleFx {
    let keep = |unit: &Unit, knob: f32, of: fn(&SampleFx) -> u32| -> u32 {
        match file {
            Some(f) if unit.knob(of(f)) == knob => of(f),
            _ => unit.stored(knob),
        }
    };
    let word = |unit: &Unit, knob: f32, of: fn(&SampleFx) -> u32| keep(unit, knob, of).min(65535) as u16;
    let flag = |on: bool, bit: u16| if on { bit } else { 0 };
    let range = |knob: f32, bits: Option<u64>| match bits {
        Some(b) if f64::from_bits(b) as f32 == knob => b,
        _ => f64::from(knob.clamp(0.0, 1.0)).to_bits(),
    };
    SampleFx {
        flags: flag(k.fade_stereo, SampleFx::FADE_STEREO)
            | flag(k.reverse, SampleFx::REVERSE)
            | flag(k.clip, SampleFx::CLIP)
            | flag(k.swap, SampleFx::SWAP),
        boost: word(&unit::BOOST, k.boost, |f| f.boost.into()),
        fade_out: word(&unit::FADE, k.fade_out, |f| f.fade_out.into()),
        fade_in: word(&unit::FADE, k.fade_in, |f| f.fade_in.into()),
        stereo_delay: word(&unit::STEREO_DELAY, k.stereo_delay, |f| f.stereo_delay.into()),
        pogo: word(&unit::POGO, k.pogo, |f| f.pogo.into()),
        feedback: word(&unit::FEEDBACK, k.eq, |f| f.feedback.into()),
        cutoff: word(&unit::CUTOFF, k.cutoff, |f| f.cutoff.into()),
        resonance: word(&unit::RESONANCE, k.resonance, |f| f.resonance.into()),
        ring_mix: word(&unit::RING_MIX, k.ring_mix, |f| f.ring_mix.into()),
        ring_freq: word(&unit::RING_FREQ, k.ring_freq, |f| f.ring_freq.into()),
        reverb: word(&unit::REVERB, k.reverb, |f| f.reverb.into()),
        reverb_type: k.reverb_type.clamp(0, 1) as u16,
        ping_pong: k.ping_pong,
        remove_dc: k.remove_dc,
        normalize: k.normalize,
        polarity: k.polarity,
        crossfade: keep(&unit::CROSSFADE, k.crossfade, |f| f.crossfade),
        trim: keep(&unit::TRIM, k.trim, |f| f.trim),
        start_bits: range(k.start, file.map(|f| f.start_bits)),
        length_bits: range(k.length, file.map(|f| f.length_bits)),
        loop_follows_reverse: k.loop_follows,
    }
}

/// What builds the played sample: it is rebuilt when this changes (spec: audio/precomputed-effects.md).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxKey {
    /// The settings in the file's units.
    pub fx: SampleFx,
    /// **(Lemon Studio)** The loop knobs' own loop (fractions of the sample after start, length and stretching), when
    /// they are off their defaults: it takes the place of the file's loop.
    pub knob_loop: Option<(f32, f32)>,
    /// The output rate (Hz): files below 8000 Hz are converted to it, and it enters `rs`.
    pub rate: u32,
    /// Time stretching in a method that stretches the sample first: MUL ratio, TIME target length in seconds (0 = off)
    /// and the pitch shift in cents.
    pub stretch: bool,
    pub ratio: f32,
    pub time: f32,
    pub shift: f32,
}

impl FxKey {
    /// No setting at all (the file as it is) at 44100 Hz.
    pub fn neutral() -> FxKey {
        FxKey { fx: SampleFx::default(), knob_loop: None, rate: 44100, stretch: false, ratio: 1.0, time: 0.0, shift: 0.0 }
    }

    /// Stretch ratio for a sample of `secs` seconds: TIME's target length when set, else MUL.
    pub fn ratio_for(&self, secs: f64) -> f64 {
        if self.time > 0.0 && secs > 0.0 {
            f64::from(self.time) / secs
        } else {
            f64::from(self.ratio)
        }
    }
}

impl SamplerParams {
    /// TIME's target length in seconds at `tempo` BPM (`None` when TIME is off).
    pub fn ts_seconds(&self, tempo: f64) -> Option<f64> {
        let steps = time_steps(self.ts_time.live());
        (time_file(steps) >= 1.0).then(|| steps / 4.0 * 60.0 / tempo.max(1.0))
    }

    /// The project file's settings for the channel, if the host gave them.
    pub fn file_fx(&self) -> Option<SampleFx> {
        self.file_settings.read().ok().and_then(|t| fx_from_text(&t))
    }

    /// The precomputed settings in the file's units: each from its knob, or the file's own value while the knob stands
    /// where that value puts it.
    pub fn sample_fx(&self) -> SampleFx {
        fx_from_knobs(&self.fx_knobs(), self.file_fx().as_ref())
    }

    /// The precomputed settings' knobs and switches as the sound uses them.
    pub fn fx_knobs(&self) -> FxKnobs {
        FxKnobs {
            fade_stereo: self.fx_fade_stereo.live(),
            reverse: self.fx_reverse.live(),
            clip: self.fx_clip.live(),
            swap: self.fx_swap.live(),
            boost: self.fx_boost.live(),
            fade_out: self.fx_out.live(),
            fade_in: self.fx_in.live(),
            stereo_delay: self.fx_sdelay.live(),
            pogo: self.fx_pogo.live(),
            eq: self.fx_eq.live(),
            cutoff: self.fx_cut.live(),
            resonance: self.fx_res.live(),
            ring_mix: self.fx_rm_mix.live(),
            ring_freq: self.fx_rm_freq.live(),
            reverb: self.fx_reverb.live(),
            reverb_type: self.fx_reverb_type.live(),
            ping_pong: self.loop_pingpong.live(),
            remove_dc: self.fx_dc.live(),
            normalize: self.fx_normalize.live(),
            polarity: self.fx_polarity.live(),
            crossfade: self.fx_crossfade.live(),
            trim: self.fx_trim.live(),
            start: self.fx_start.live(),
            length: self.fx_length.live(),
            loop_follows: self.fx_loop_follows.live(),
        }
    }

    /// Precomputed settings at the host's `tempo` (TIME depends on it) and output `rate`.
    pub fn fx_key(&self, tempo: f64, rate: u32) -> FxKey {
        let stretch = stretch_whole(self.ts_mode.live());
        let time = if stretch { self.ts_seconds(tempo).unwrap_or(0.0) as f32 } else { 0.0 };
        let (ls, le) = (self.loop_start.live(), self.loop_end.live());
        FxKey {
            fx: self.sample_fx(),
            knob_loop: (ls > 0.0 || le < 1.0).then_some((ls, le)),
            rate,
            stretch,
            ratio: if stretch { mul_of(self.ts_mul.live()) as f32 } else { 1.0 },
            time,
            shift: if stretch { ts_cents(self.ts_pitch.live()) as f32 } else { 0.0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_edits_reach_the_sound_without_the_host() {
        let p = Arc::new(SamplerParams::default());
        let ptr = p.f_cut.as_ptr();
        let host = p.f_cut.unmodulated_normalized_value();
        assert_eq!(p.f_cut.live(), p.f_cut.value());
        // An edit the host has not handed back: the sound uses it.
        live_set(param_addr(ptr), 0.25, host);
        assert!((p.f_cut.live() - 0.25).abs() < 1e-6);
        assert!((live_of(ptr) - 0.25).abs() < 1e-6);
        // Setting it back to the host's value leaves nothing pending.
        live_set(param_addr(ptr), host, host);
        assert_eq!(p.f_cut.live(), p.f_cut.value());
        // Booleans and integers too; a dropped instance forgets its edits.
        live_set(param_addr(p.fx_reverse.as_ptr()), 1.0, 0.0);
        assert!(p.fx_reverse.live());
        let addr = param_addr(p.fx_reverse.as_ptr());
        drop(p);
        assert!(LIVE.lock().unwrap().iter().all(|e| e.addr != addr));
    }
}
