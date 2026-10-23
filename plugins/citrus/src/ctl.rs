// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the editor's controls edit: one setting of the state each, its range, its reset value and its hint text.
//
// spec: plugins/sytrus-editor.md (§2 main page, §6 matrix: settings, ranges, hints), plugins/citrus.md §6 (reset
// values)

use crate::state::*;

/// A setting of the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// The state's first ints: 0 main volume, 1 master LFO level, 2 main pitch.
    Main(usize),
    /// A further-settings int `s[k]`.
    S(usize),
    /// `s[32]`'s switch byte `k`.
    S32(usize),
    /// Switch byte `n`.
    Switch(usize),
    Soften,
    /// The equaliser's target.
    EqMode,
    /// Equaliser band `b`'s field: 0 gain, 1 frequency, 2 width, 3 type.
    Eq(usize, usize),
    /// A modulation matrix cell (row, column).
    Cell(usize, usize),
    /// A ring-modulation matrix cell.
    Ring(usize, usize),
    /// Module `m`'s header int `k`.
    Head(usize, usize),
    /// Operator `m`'s block: 0 ratio, 1 offset, 2 noise, 3..=7 the shape ints, 8..=15 the flag bytes.
    Op(usize, usize),
    /// Filter module `m`'s block: 0 oversampling, 1 track, 2 wave shaper on, 3 type.
    Filt(usize, usize),
    /// Delay unit `d`: 0 feedback, 1 time, 2 stereo, 3 level, 4 mode, 5 tempo-synced, 6 serial, 7 on.
    Delay(usize, usize),
    /// The reverb: 0..=8 its ints, 9 on, 10 tempo-based pre-delay.
    Reverb(usize),
    /// Articulator `a` of module `m`, part `p` (0 envelope, 1 LFO, 2..=7 the maps): 0 on, 1..=4 the envelope's
    /// scales or the LFO's speed, tension, skew and pulse width, 5 tempo-synced, 6 global.
    Part(usize, usize, usize, usize),
}

/// Articulator `a`'s curve `p` (0 envelope, 1 LFO, 2.. the maps).
pub fn part(st: &State, m: usize, a: usize, p: usize) -> &crate::curve::Curve {
    let art = &st.modules[m].arts[a];
    match p {
        0 => &art.env,
        1 => &art.lfo,
        _ => &art.maps[p - 2],
    }
}

pub fn part_mut(st: &mut State, m: usize, a: usize, p: usize) -> &mut crate::curve::Curve {
    let art = &mut st.modules[m].arts[a];
    match p {
        0 => &mut art.env,
        1 => &mut art.lfo,
        _ => &mut art.maps[p - 2],
    }
}

fn flag(v: u8, bit: u8) -> i32 {
    i32::from(v & bit != 0)
}

fn set_flag(v: &mut u8, bit: u8, on: bool) {
    *v = if on { *v | bit } else { *v & !bit };
}

pub fn get(st: &State, f: Field) -> i32 {
    use crate::curve::{GLOBAL, SYNCED};
    match f {
        Field::Head(m, k) => st.modules[m].header[k],
        Field::Op(m, k) => {
            let o = &st.modules[m].op;
            match k {
                0 => o.ratio,
                1 => o.offset,
                2 => o.noise,
                3..=7 => o.shape[k - 3],
                _ => i32::from(o.flags[k - 8]),
            }
        }
        Field::Filt(m, k) => {
            let b = &st.modules[m].filter;
            match k {
                0 => i32::from(b.oversample),
                1 => i32::from(b.track),
                2 => i32::from(b.shaper_on),
                _ => i32::from(b.kind),
            }
        }
        Field::Delay(d, k) => {
            let u = &st.delays[d];
            match k {
                0 => u.feedback,
                1 => u.time,
                2 => u.stereo,
                3 => u.level,
                4 => u.mode,
                5 => i32::from(u.synced),
                6 => i32::from(u.serial),
                _ => i32::from(u.on),
            }
        }
        Field::Reverb(k @ 0..=8) => st.reverb.knobs[k],
        Field::Reverb(9) => i32::from(st.reverb.on),
        Field::Reverb(_) => i32::from(st.reverb.tempo),
        Field::Part(m, a, p, k) => {
            let c = part(st, m, a, p);
            match k {
                0 => i32::from(c.enabled),
                1..=4 if p == 0 => c.scales[k - 1],
                1..=4 => c.lfo[k - 1],
                5 => flag(c.flags, SYNCED),
                _ => flag(c.flags, GLOBAL),
            }
        }
        Field::Main(0) => st.main_volume,
        Field::Main(1) => st.master_lfo,
        Field::Main(_) => st.main_pitch,
        Field::S(k) => st.s[k],
        Field::S32(k) => i32::from(st.s32(k)),
        Field::Switch(n) => i32::from(st.switches[n]),
        Field::Soften => st.soften,
        Field::EqMode => st.eq.mode,
        Field::Eq(b, k) => st.eq.bands[b][k],
        Field::Cell(r, c) => st.matrix[r * 9 + c],
        Field::Ring(r, c) => st.ring[r * 9 + c],
    }
}

/// Sets a setting, clamped to its range.
pub fn set(st: &mut State, f: Field, v: i32) {
    use crate::curve::{GLOBAL, SYNCED};
    let (lo, hi) = range(f);
    let v = v.clamp(lo, hi);
    match f {
        Field::Head(m, k) => st.modules[m].header[k] = v,
        Field::Op(m, k) => {
            let o = &mut st.modules[m].op;
            match k {
                0 => o.ratio = v,
                1 => o.offset = v,
                2 => o.noise = v,
                3..=7 => o.shape[k - 3] = v,
                _ => o.flags[k - 8] = v as u8,
            }
        }
        Field::Filt(m, k) => {
            let b = &mut st.modules[m].filter;
            match k {
                0 => b.oversample = v != 0,
                1 => b.track = v != 0,
                2 => b.shaper_on = v != 0,
                _ => b.kind = v as u8,
            }
        }
        Field::Delay(d, k) => {
            let u = &mut st.delays[d];
            match k {
                0 => u.feedback = v,
                1 => u.time = v,
                2 => u.stereo = v,
                3 => u.level = v,
                4 => u.mode = v,
                5 => u.synced = v != 0,
                6 => u.serial = v != 0,
                _ => u.on = v != 0,
            }
        }
        Field::Reverb(k @ 0..=8) => st.reverb.knobs[k] = v,
        Field::Reverb(9) => st.reverb.on = v != 0,
        Field::Reverb(_) => st.reverb.tempo = v != 0,
        Field::Part(m, a, p, k) => {
            let c = part_mut(st, m, a, p);
            match k {
                0 => c.enabled = v != 0,
                1..=4 if p == 0 => c.scales[k - 1] = v,
                1..=4 => c.lfo[k - 1] = v,
                5 => set_flag(&mut c.flags, SYNCED, v != 0),
                _ => set_flag(&mut c.flags, GLOBAL, v != 0),
            }
            // What the walks work out from the curve.
            c.prepare();
        }
        Field::Main(0) => st.main_volume = v,
        Field::Main(1) => st.master_lfo = v,
        Field::Main(_) => st.main_pitch = v,
        Field::S(k) => st.s[k] = v,
        Field::S32(k) => st.s[32] = (st.s[32] & !(0xFF << (8 * k))) | (v << (8 * k)),
        Field::Switch(n) => st.switches[n] = v as u8,
        Field::Soften => st.soften = v,
        Field::EqMode => st.eq.mode = v,
        Field::Eq(b, k) => st.eq.bands[b][k] = v,
        Field::Cell(r, c) => st.matrix[r * 9 + c] = v,
        Field::Ring(r, c) => st.ring[r * 9 + c] = v,
    }
}

/// The range a control moves a setting over.
pub fn range(f: Field) -> (i32, i32) {
    match f {
        Field::Main(0) => (0, 160),
        Field::Main(1) => (-128, 128),
        Field::Main(_) => (-24, 24),
        Field::S(2 | 6) => (-128, 128),
        Field::S(0..=7) => (0, 256),
        Field::S(8) => (1, 9),
        Field::S(9 | 13) => (0, 128),
        Field::S(10) => (16, 128),
        Field::S(11) => (0, 256),
        Field::S(12) => (0, 96),
        Field::S(14) => (0, 192),
        Field::S(25 | 26) => (0, 512),
        // Oversampling 1x .. 32x: the engine's limit (FL's range was not measured).
        Field::S(30 | 31) => (0, 5),
        // The FX page.
        Field::S(15 | 21 | 22) => (-128, 128),
        Field::S(16) => (0, 9),
        Field::S(17 | 18) => (0, 1024),
        Field::S(19 | 20 | 23) => (0, 128),
        // The send's target: none, or the mixer's inserts (FL's end was not measured).
        Field::S(24) => (-1, 125),
        Field::Head(m, k) if m < OPERATORS => match k {
            0 => (0, 256),
            1 => (-256, 256),
            _ => (0, 4800),
        },
        Field::Head(_, 0) => (0, 5),
        Field::Head(_, 7) => (64, 192),
        Field::Head(_, 8) => (0, 128),
        Field::Head(_, 9) => (-256, 256),
        Field::Head(..) => (0, 256),
        // The ratio up to 100 x and the offset within 1 kHz (Lemon choices: FL's ranges were not measured).
        Field::Op(_, 0) => (0, 1_000_000),
        Field::Op(_, 1) => (-100_000, 100_000),
        Field::Op(_, 3) => (0, 512),
        Field::Op(_, 4) => (-128, 128),
        Field::Op(_, 2 | 5..=7) => (0, 128),
        Field::Op(..) => (0, 1),
        Field::Filt(_, 3) => (0, 12),
        Field::Filt(..) => (0, 1),
        Field::Delay(_, 0) => (0, 127),
        Field::Delay(_, 1) => (0, 384),
        Field::Delay(_, 2) => (-512, 512),
        Field::Delay(_, 3) => (-128, 128),
        Field::Delay(_, 4) => (0, 2),
        Field::Delay(..) => (0, 1),
        Field::Reverb(0) => (19, 3000),
        Field::Reverb(1 | 6) => (5, 221),
        Field::Reverb(2) => (0, 384),
        Field::Reverb(3) => (1, 100),
        Field::Reverb(4) => (0, 100),
        Field::Reverb(5) => (1, 200),
        Field::Reverb(7) => (0, 4),
        Field::Reverb(8) => (-128, 128),
        Field::Reverb(_) => (0, 1),
        // The bar's knobs (not measured in FL): the envelope's like the main page's, the LFO's what the curves
        // take (plugins/image-line-curves.md, "The LFO").
        Field::Part(_, _, 0, 1 | 2 | 4) => (0, 256),
        Field::Part(_, _, 0, 3) => (-128, 128),
        Field::Part(_, _, 1, 1) => (512, 65536),
        Field::Part(_, _, 1, 2 | 3) => (-128, 128),
        Field::Part(_, _, 1, 4) => (-112, 112),
        Field::Part(..) => (0, 1),
        Field::S(_) | Field::S32(_) => (0, 1),
        // Portamento's limit takes six values, the other switches two.
        Field::Switch(7) => (0, 5),
        Field::Switch(9) => (0, 3),
        Field::Switch(11) => (0, 3),
        Field::Switch(_) => (0, 1),
        Field::Soften => (0, 128),
        Field::EqMode => (0, 3),
        Field::Eq(_, 0) => (-1800, 1800),
        Field::Eq(_, 3) => (0, 2),
        Field::Eq(..) => (0, 65536),
        Field::Cell(..) | Field::Ring(..) => (-256, 256),
    }
}

/// The value a reset gives (spec plugins/citrus.md §6): the default patch's, and for what it leaves at 0 the
/// envelope scales 128, 128, 0, 128, unison volume and pitch 128, mod X / Y 256 and the equaliser's default bands.
pub fn default(f: Field) -> i32 {
    let v = match f {
        Field::S(0 | 1 | 3 | 4 | 5 | 7) => 128,
        Field::S(2 | 6) => 0,
        Field::S(10 | 11) => 128,
        Field::S(25 | 26) => 256,
        Field::Eq(b, k) => EqSettings::default().bands[b][k],
        _ => get(&crate::preset::default_patch(), f),
    };
    let (lo, hi) = range(f);
    v.clamp(lo, hi)
}

/// `v / d` as a whole percentage.
fn pct(v: i32, d: f32) -> String {
    format!("{}%", (v as f32 / d * 100.0).round() as i32)
}

const ENV_NAMES: [&str; 8] =
    ["Volume attack", "Volume decay", "Volume sustain", "Volume release", "Filter attack", "Filter decay", "Filter sustain", "Filter release"];
const EQ_TARGETS: [&str; 4] = ["none", "out+effects", "out only", "effects only"];
pub const EQ_TYPES: [&str; 3] = ["Low shelf", "Peaking", "High shelf"];
/// An articulator's parts, as the source tabs' hints name them.
pub const PART_NAMES: [&str; 8] =
    ["envelope", "LFO", "keyboard mapping", "velocity mapping", "modulation X mapping", "modulation Y mapping", "random mapping", "unison mapping"];
/// The filter types: the box's abbreviations and the hints' names (plugins/sytrus-editor.md §5).
pub const FILTER_ABBR: [&str; 13] = ["SVF", "LP", "HP", "LP2", "BP", "BS", "HP2", "AP", "LS", "PK", "HS", "LP3", "PHS"];
const FILTER_NAMES: [&str; 13] = [
    "Chocolate state variable",
    "Lime low pass",
    "Lime high pass",
    "Vanilla low pass",
    "Vanilla band pass",
    "Vanilla band stop",
    "Vanilla high pass",
    "Vanilla all pass",
    "Vanilla low shelf",
    "Vanilla peaking",
    "Vanilla high shelf",
    "Mango low pass",
    "Cherry phaser",
];
/// The stages column's positions; FL's hint read "dual" for x2, the others are Lemon names.
pub const STAGE_LABELS: [&str; 6] = ["OFF", "x1", "x2", "x3", "ALT x2", "ALT x3"];
const STAGE_NAMES: [&str; 6] = ["off", "single", "dual", "triple", "dual spread", "triple spread"];
const DELAY_MODES: [&str; 3] = ["normal", "inverted", "ping-pong"];
const REVERB_COLORS: [&str; 5] = ["Brighter", "Bright", "Neutral", "Warm", "Warmer"];

/// Settings shown as `v / d` in percent.
fn pct_of(f: Field) -> Option<f32> {
    Some(match f {
        Field::Main(0 | 1) | Field::S(0..=7 | 9 | 13) | Field::Soften => 128.0,
        Field::S(11) => 256.0,
        Field::S(12) => 96.0,
        Field::S(14) => 192.0,
        Field::S(25 | 26) => 512.0,
        Field::S(17 | 18) => 1024.0,
        Field::S(19..=23) => 128.0,
        Field::Head(m, 0 | 1) if m < OPERATORS => 256.0,
        Field::Head(m, 1..=3 | 9) if m >= OPERATORS => 256.0,
        Field::Head(m, 7 | 8) if m >= OPERATORS => 128.0,
        Field::Op(_, 3) => 512.0,
        Field::Op(_, 2 | 4..=7) => 128.0,
        Field::Delay(_, 0 | 3) | Field::Reverb(8) => 128.0,
        Field::Part(_, _, 0, 1..=4) | Field::Part(_, _, 1, 2 | 3) => 128.0,
        Field::Part(_, _, 1, 4) => 112.0,
        Field::Cell(..) | Field::Ring(..) => 256.0,
        _ => return None,
    })
}

/// The delay's stereo offset in ms (plugins/sytrus-effects.md, "Delays": 0.01 ms units).
fn stereo_ms(v: i32) -> f32 {
    ((((v.unsigned_abs() as f64 / 512.0) * 1001f64.ln()).exp() - 1.0) * 50.0 / 100.0) as f32
}

/// A time of 1/384 s per step, or 1/192 beat (48 to a step) when tempo-based: `ms`, or `steps:ticks`.
pub fn time_text(v: i32, tempo: bool) -> String {
    if tempo { format!("{}:{:02}", v / 48, (v % 48) / 2) } else { format!("{}ms", (v as f32 * 1000.0 / 384.0).round() as i32) }
}

/// Left / right of a signed `v / d`.
fn sides(v: i32, d: f32) -> String {
    match v {
        0 => "Centered".into(),
        v if v < 0 => format!("{} left", pct(-v, d)),
        v => format!("{} right", pct(v, d)),
    }
}

/// A module's name in the matrix's hints: "operator 1", "filter 1".
fn module(r: usize) -> String {
    if r < OPERATORS { format!("operator {}", r + 1) } else { format!("filter {}", r - OPERATORS + 1) }
}

/// A setting's name as its hint starts (FL's texts, spec plugins/sytrus-editor.md).
pub fn name(f: Field) -> String {
    match f {
        Field::Main(0) => "Master level".into(),
        Field::Main(1) => "Master LFO level".into(),
        Field::Main(_) => "Master pitch".into(),
        Field::S(k @ 0..=7) => ENV_NAMES[k].into(),
        Field::S(8) => "Unison order".into(),
        Field::S(9) => "Unison panning".into(),
        Field::S(10) => "Unison volume".into(),
        Field::S(11) => "Unison pitch".into(),
        Field::S(12) => "Unison sub level".into(),
        Field::S(13) => "Unison phase".into(),
        Field::S(14) => "Unison envelope variation".into(),
        Field::S(25) => "Modulation X".into(),
        Field::S(26) => "Modulation Y".into(),
        Field::S(28) => "Extra high envelope accuracy (draft)".into(),
        Field::S(29) => "Extra high envelope accuracy (rendering)".into(),
        Field::S(30) => "Oversampling factor (draft)".into(),
        Field::S(31) => "Oversampling factor (rendering)".into(),
        Field::S(15) => "Panning".into(),
        Field::S(k @ 16..=22) => format!("Chorus {}", ["order", "depth", "speed", "delay", "spread", "cross", "volume"][k - 16]),
        Field::S(23) => "Send level".into(),
        Field::S(24) => "Send to effect track number".into(),
        Field::S(k) => format!("Setting {k}"),
        // FL's hint for SMOOTH was not read.
        Field::S32(0) => "Smooth X / Y modulation".into(),
        Field::S32(1) => "Enable randomness".into(),
        Field::S32(_) => "Operator 1 pitch articulator = global".into(),
        Field::Switch(0) => "Unison for oscillators only".into(),
        Field::Switch(1) => "Global DC offset removal".into(),
        Field::Switch(2) => "Attenuate ringing ripples".into(),
        Field::Switch(3) => "Invert".into(),
        Field::Switch(4) => "Portamento".into(),
        Field::Switch(5) => "Variable time".into(),
        Field::Switch(6) => "Track origin".into(),
        Field::Switch(7) => "Portamento limit".into(),
        Field::Switch(8) => "Link velocity to volume".into(),
        Field::Switch(9) => "Link velocity to attack".into(),
        Field::Switch(10) => "Link release to release length".into(),
        Field::Switch(_) => "Unison distribution".into(),
        Field::Soften => "Soften attack of lower-velocity voices".into(),
        Field::EqMode => "Equalizer target".into(),
        Field::Eq(b, 0) => format!("EQ band {} level", b + 1),
        Field::Eq(b, 1) => format!("EQ band {} freq", b + 1),
        Field::Eq(b, 2) => format!("EQ band {} width", b + 1),
        Field::Eq(b, _) => format!("EQ band {} type", b + 1),
        Field::Head(m, k) if m < OPERATORS => ["Starting phase", "Volume", "Pitch envelope/LFO amount"].get(k).unwrap_or(&"Setting").to_string(),
        Field::Head(_, k) => match k {
            0 => "Filter mode",
            1 => "Cutoff envelope / LFO amount",
            2 => "Cutoff frequency",
            3 => "Resonance / bw / feedback",
            7 => "Waveshaper pre amp",
            8 => "Waveshaper mix",
            9 => "To next filter",
            _ => "Filter level",
        }
        .into(),
        Field::Op(_, k) => [
            "Freq ratio",
            "Freq offset (Hz)",
            "Noise",
            "Shape",
            "Tension",
            "Skew",
            "Sine shaper",
            "Pre-filter",
            "Half",
            "Even",
            "Absolute",
            "Global / retriggered phase",
            "DC offset removal",
            "Attack click removal",
            "Plucked mode",
            "Band-limit waveform",
        ][k]
            .into(),
        Field::Filt(_, k) => ["Oversample when required", "Alternate resonance / bandwidth / mode", "Enable waveshaper", "Filter type"][k].into(),
        Field::Delay(_, k) => [
            "Delay feedback level",
            "Delay time",
            "Delay time stereo offset",
            "Delay volume",
            "Delay feedback mode",
            "Tempo-based time",
            "Parallel / serial process",
            "Enable delay",
        ][k]
            .into(),
        Field::Reverb(k) => [
            "Reverb low cut",
            "Reverb high cut",
            "Reverb predelay",
            "Reverb room size",
            "Reverb diffusion",
            "Reverb decay",
            "Reverb high damping",
            "Reverb color",
            "Reverb wet level",
            "Enable reverb",
            "Tempo-based predelay",
        ][k]
            .into(),
        Field::Part(_, _, p, k) => match (p, k) {
            (_, 0) => format!("Enable {}", PART_NAMES[p]),
            (0, 1..=4) => ["Attack time scale", "Decay time scale", "Sustain level offset", "Release time scale"][k - 1].into(),
            (_, 1..=4) => ["LFO speed", "LFO tension", "LFO skew", "LFO pulse width"][k - 1].into(),
            (_, 5) => "Tempo-based time".into(),
            (0, _) => "Shared / retriggered envelope".into(),
            _ => "Shared / retriggered LFO".into(),
        },
        Field::Cell(r, c) | Field::Ring(r, c) => {
            let ring = matches!(f, Field::Ring(..));
            match c {
                COL_PAN => format!("{} panning", module(r)),
                COL_SEND => format!("{} to effects", module(r)),
                COL_OUT => format!("{} to output", module(r)),
                _ if r == c => format!("operator {} {} modulation feedback", r + 1, if ring { "ring" } else { "frequency" }),
                _ if r < OPERATORS => format!("operator {} to {} {} modulation", c + 1, r + 1, if ring { "ring" } else { "frequency" }),
                _ => format!("operator {} to {}", c + 1, module(r)),
            }
        }
    }
}

/// A setting's value as its hint shows it.
pub fn text(f: Field, v: i32) -> String {
    let on_off = |v: i32| (if v != 0 { "on" } else { "off" }).to_string();
    match f {
        Field::Cell(_, COL_PAN) | Field::Ring(_, COL_PAN) => sides(v, 256.0),
        Field::S(15) => sides(v, 128.0),
        _ if pct_of(f).is_some() => pct(v, pct_of(f).unwrap()),
        Field::Main(_) => format!("{}{:.2} semitones", if v > 0 { "+" } else { "" }, v as f32),
        Field::S(10) => pct(v - 16, 112.0),
        Field::S(30 | 31) => format!("{}x", 1 << v.clamp(0, 5)),
        Field::S32(_) | Field::Switch(_) | Field::S(28 | 29) => on_off(v),
        Field::S(24) if v < 0 => "...".into(),
        Field::S(24) => (v + 1).to_string(),
        Field::S(_) => v.to_string(),
        Field::Head(_, 2) => format!("{}{:.2} cents", if v > 0 { "+" } else { "" }, v as f32),
        Field::Head(_, 0) => STAGE_NAMES[v.clamp(0, 5) as usize].into(),
        Field::Head(..) => v.to_string(),
        Field::Op(_, 0) => format!("{:.4} x", v as f64 / 10000.0),
        Field::Op(_, 1) => format!("{:.2} Hz", v as f64 / 100.0),
        Field::Filt(_, 3) => FILTER_NAMES[v.clamp(0, 12) as usize].into(),
        Field::Op(..) | Field::Filt(..) | Field::Delay(_, 5..) | Field::Reverb(9 | 10) | Field::Part(_, _, _, 0 | 5 | 6) => on_off(v),
        Field::Delay(_, 1) => time_text(v, false),
        Field::Delay(_, 2) => format!("{:.1}ms {}", stereo_ms(v), if v < 0 { "left" } else { "right" }),
        Field::Delay(_, _) => DELAY_MODES[v.clamp(0, 2) as usize].into(),
        Field::Reverb(0) if v <= 19 => "Off".into(),
        Field::Reverb(0) => format!("{v}Hz"),
        Field::Reverb(1 | 6) if v >= 221 => "Off".into(),
        Field::Reverb(1 | 6) => format!("{:.1}kHz", v as f32 / 10.0),
        Field::Reverb(2) => time_text(v, false),
        Field::Reverb(5) => format!("{:.1}sec", v as f32 / 10.0),
        Field::Reverb(7) => REVERB_COLORS[v.clamp(0, 4) as usize].into(),
        Field::Reverb(_) => v.to_string(),
        // The LFO's speed, over its range.
        Field::Part(..) => pct(v - 512, (65536 - 512) as f32),
        Field::EqMode => EQ_TARGETS[v.clamp(0, 3) as usize].into(),
        Field::Eq(_, 0) => format!("{:.1}dB", v as f32 / 100.0),
        Field::Eq(_, 1) => format!("{}Hz", os_dsp::mixer::eq_frequency(v).round() as i64),
        Field::Eq(_, 2) => format!("{:.2}", v as f32 / 65536.0),
        Field::Eq(_, _) => EQ_TYPES[v.clamp(0, 2) as usize].into(),
        Field::Soften | Field::Cell(..) | Field::Ring(..) => unreachable!("shown in percent"),
    }
}

/// The hint while hovering (FL's: the name and the value; a switch its name alone, portamento its state; a matrix
/// cell its name, with "FM" or "RM" right on the operator block) or dragging (a cell's bare value).
pub fn hint(st: &State, f: Field, dragging: bool) -> String {
    let v = get(st, f);
    match f {
        Field::Cell(r, c) | Field::Ring(r, c) if dragging => {
            let _ = (r, c);
            text(f, v)
        }
        Field::Cell(r, c) if r < OPERATORS && c < OPERATORS => format!("{}\tFM", name(f)),
        Field::Ring(..) => format!("{}\tRM", name(f)),
        Field::Cell(..) => name(f),
        Field::Switch(4) => format!("Portamento: {}", text(f, v)),
        Field::S32(_) | Field::Switch(_) | Field::S(28 | 29) => name(f),
        Field::Eq(b, 3) => format!("{}\tEQ band {} type", text(f, v), b + 1),
        Field::EqMode => format!("{}: {}", name(f), text(f, v)),
        // Switches and lamps: their names.
        Field::Op(_, 8..) | Field::Filt(_, 0..=2) | Field::Delay(_, 5..) | Field::Reverb(9 | 10) | Field::Part(_, _, _, 0 | 5 | 6) => name(f),
        // The reverb's sliders show their names while hovered.
        Field::Reverb(_) if !dragging => name(f),
        // A delay's time as its tempo switch has it.
        Field::Delay(d, 1) => format!("{}: {}", name(f), time_text(v, st.delays[d].synced)),
        Field::Reverb(2) => format!("{}: {}", name(f), time_text(v, st.reverb.tempo)),
        _ => format!("{}: {}", name(f), text(f, v)),
    }
}

/// Parses a typed value (the value text's number, with or without its unit) back to a setting's value.
pub fn parse(f: Field, s: &str) -> Option<i32> {
    let t = s.trim().to_ascii_lowercase();
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let x: f32 = num.parse().ok()?;
    let left = t.contains("left");
    let sign = if left { -1.0 } else { 1.0 };
    let v = match f {
        Field::Part(_, _, 1, 1) => x * 650.24 + 512.0,
        _ if pct_of(f).is_some() => x * pct_of(f).unwrap() / 100.0 * sign,
        Field::S(15) => x * 1.28 * sign,
        Field::Main(_) => x,
        Field::S(10) => x * 1.12 + 16.0,
        Field::S(24) => x - 1.0,
        Field::Op(_, 0) => x * 10000.0,
        Field::Op(_, 1) => x * 100.0,
        Field::Delay(_, 1) | Field::Reverb(2) => x * 0.384,
        Field::Reverb(1 | 5 | 6) => x * 10.0,
        Field::S(30 | 31) => x.max(1.0).log2(),
        Field::Eq(_, 0) => x * 100.0,
        Field::Eq(_, 1) => {
            // The frequency law's inverse, by search (it rises with the value).
            let hz = x;
            let (mut lo, mut hi) = (0, 65536);
            while lo < hi {
                let mid = (lo + hi) / 2;
                if os_dsp::mixer::eq_frequency(mid) < hz { lo = mid + 1 } else { hi = mid }
            }
            lo as f32
        }
        Field::Eq(_, 2) => x * 65536.0,
        _ => x,
    };
    let (lo, hi) = range(f);
    Some((v.round() as i32).clamp(lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hints read FL's texts (spec plugins/sytrus-editor.md §2, §6).
    #[test]
    fn hints_read_fls_texts() {
        let mut st = crate::preset::default_patch();
        for (f, v, want) in [
            (Field::Main(0), 128, "Master level: 100%"),
            (Field::Main(2), -24, "Master pitch: -24.00 semitones"),
            (Field::Main(2), 12, "Master pitch: +12.00 semitones"),
            (Field::S(2), 0, "Volume sustain: 0%"),
            (Field::S(10), 100, "Unison volume: 75%"),
            (Field::S(11), 128, "Unison pitch: 50%"),
            (Field::S(25), 466, "Modulation X: 91%"),
            (Field::Eq(0, 0), 0, "EQ band 1 level: 0.0dB"),
            (Field::Eq(0, 2), 17500, "EQ band 1 width: 0.27"),
            (Field::EqMode, 1, "Equalizer target: out+effects"),
            (Field::Switch(4), 0, "Portamento: off"),
            (Field::Soften, 0, "Soften attack of lower-velocity voices: 0%"),
            (Field::Cell(0, 1), 0, "operator 2 to 1 frequency modulation\tFM"),
            (Field::Cell(0, 0), 0, "operator 1 frequency modulation feedback\tFM"),
            (Field::Cell(6, 0), 0, "operator 1 to filter 1"),
            (Field::Cell(6, COL_OUT), 0, "filter 1 to output"),
            (Field::Cell(0, COL_PAN), 0, "operator 1 panning"),
            (Field::Ring(0, 1), 0, "operator 2 to 1 ring modulation\tRM"),
        ] {
            set(&mut st, f, v);
            assert_eq!(hint(&st, f, false), want);
        }
        set(&mut st, Field::Cell(0, COL_PAN), 41);
        assert_eq!(hint(&st, Field::Cell(0, COL_PAN), true), "16% right");
        assert_eq!(parse(Field::Cell(0, COL_PAN), "16% left"), Some(-41));
        assert_eq!(parse(Field::Main(0), "100%"), Some(128));
        let hz = os_dsp::mixer::eq_frequency(5777).round();
        assert_eq!(text(Field::Eq(0, 1), parse(Field::Eq(0, 1), &format!("{hz}Hz")).unwrap()), format!("{hz}Hz"));
    }

    /// `s[32]`'s bytes and the ranges' ends.
    #[test]
    fn settings_set_within_their_ranges() {
        let mut st = State::default();
        set(&mut st, Field::S32(1), 1);
        set(&mut st, Field::S32(2), 1);
        assert_eq!(st.s[32], 0x0001_0100);
        assert!(st.s32(1) && st.s32(2) && !st.s32(0));
        set(&mut st, Field::S32(1), 0);
        assert_eq!(st.s[32], 0x0001_0000);
        set(&mut st, Field::S(8), 20);
        assert_eq!(st.s[8], 9);
        assert_eq!((default(Field::S(8)), default(Field::S(25)), default(Field::Cell(0, COL_OUT))), (1, 256, 256));
    }
}
