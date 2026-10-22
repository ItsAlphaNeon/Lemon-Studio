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
}

pub fn get(st: &State, f: Field) -> i32 {
    match f {
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
    let (lo, hi) = range(f);
    let v = v.clamp(lo, hi);
    match f {
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
    match f {
        Field::Main(0 | 1) | Field::S(0..=7 | 9 | 13) | Field::Soften => pct(v, 128.0),
        Field::Main(_) => format!("{}{:.2} semitones", if v > 0 { "+" } else { "" }, v as f32),
        Field::S(10) => pct(v - 16, 112.0),
        Field::S(11) => pct(v, 256.0),
        Field::S(12) => pct(v, 96.0),
        Field::S(14) => pct(v, 192.0),
        Field::S(25 | 26) => pct(v, 512.0),
        Field::S(30 | 31) => format!("{}x", 1 << v.clamp(0, 5)),
        Field::S32(_) | Field::Switch(_) | Field::S(28 | 29) => (if v != 0 { "on" } else { "off" }).into(),
        Field::S(_) => v.to_string(),
        Field::EqMode => EQ_TARGETS[v.clamp(0, 3) as usize].into(),
        Field::Eq(_, 0) => format!("{:.1}dB", v as f32 / 100.0),
        Field::Eq(_, 1) => format!("{}Hz", os_dsp::mixer::eq_frequency(v).round() as i64),
        Field::Eq(_, 2) => format!("{:.2}", v as f32 / 65536.0),
        Field::Eq(_, _) => EQ_TYPES[v.clamp(0, 2) as usize].into(),
        Field::Cell(_, COL_PAN) | Field::Ring(_, COL_PAN) => match v {
            0 => "Centered".into(),
            v if v < 0 => format!("{} left", pct(-v, 256.0)),
            v => format!("{} right", pct(v, 256.0)),
        },
        Field::Cell(..) | Field::Ring(..) => pct(v, 256.0),
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
        _ => format!("{}: {}", name(f), text(f, v)),
    }
}

/// Parses a typed value (the value text's number, with or without its unit) back to a setting's value.
pub fn parse(f: Field, s: &str) -> Option<i32> {
    let t = s.trim().to_ascii_lowercase();
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let x: f32 = num.parse().ok()?;
    let left = t.contains("left");
    let v = match f {
        Field::Main(0 | 1) | Field::S(0..=7 | 9 | 13) | Field::Soften => x * 1.28,
        Field::Main(_) => x,
        Field::S(10) => x * 1.12 + 16.0,
        Field::S(11) => x * 2.56,
        Field::S(12) => x * 0.96,
        Field::S(14) => x * 1.92,
        Field::S(25 | 26) => x * 5.12,
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
        Field::Cell(..) | Field::Ring(..) => x * 2.56 * if left { -1.0 } else { 1.0 },
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
