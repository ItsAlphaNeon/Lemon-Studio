// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The settings the editor shows: reading and setting them in the state, their ranges, defaults, hint names and value
//! texts. The ranges and defaults are Ogun's; its hint names were read where the spec lists them, the value texts are
//! Lemon Studio's own form (the effects' as Citrus shows the same settings).
//!
// spec: plugins/smith.md, plugins/ogun.md ("Controls and ranges", "The state")

use citrus::ctl::Field as CitrusField;
use os_dsp::curve::{Curve, GLOBAL, SYNCED};

use crate::state::*;
use crate::synth::time_of;

/// A switch of the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flag {
    Smooth,
    Declick,
    AltDecay,
    ReverbOn,
    ReverbTempo,
    DelayOn,
    DelayTempo,
    LinkRelease,
    NaturalScale,
    NaturalWeight,
    AutoLoudness,
    OneVoice,
    Porta,
    PortaVariable,
}

/// A setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// Int field `k`.
    Int(usize),
    Flag(Flag),
    /// Articulator `a`'s envelope (0) or LFO (1), setting `k`: 0 on, 1..=4 the time scales or the LFO's settings, 5
    /// tempo-based, 6 shared.
    Part(usize, usize, usize),
    /// The comb on final map `m`.
    Comb(usize),
    /// EQ band `b`'s type.
    EqType(usize),
    Quality,
    PortaLimit,
}

fn flag_ref(st: &mut State, f: Flag) -> &mut bool {
    match f {
        Flag::Smooth => &mut st.smooth,
        Flag::Declick => &mut st.declick,
        Flag::AltDecay => &mut st.alt_decay,
        Flag::ReverbOn => &mut st.reverb_on,
        Flag::ReverbTempo => &mut st.reverb_tempo,
        Flag::DelayOn => &mut st.delay_on,
        Flag::DelayTempo => &mut st.delay_tempo,
        Flag::LinkRelease => &mut st.link_release,
        Flag::NaturalScale => &mut st.natural_scale,
        Flag::NaturalWeight => &mut st.natural_weight,
        Flag::AutoLoudness => &mut st.auto_loudness,
        Flag::OneVoice => &mut st.one_voice,
        Flag::Porta => &mut st.porta,
        Flag::PortaVariable => &mut st.porta_variable,
    }
}

fn flag_of(st: &State, f: Flag) -> bool {
    match f {
        Flag::Smooth => st.smooth,
        Flag::Declick => st.declick,
        Flag::AltDecay => st.alt_decay,
        Flag::ReverbOn => st.reverb_on,
        Flag::ReverbTempo => st.reverb_tempo,
        Flag::DelayOn => st.delay_on,
        Flag::DelayTempo => st.delay_tempo,
        Flag::LinkRelease => st.link_release,
        Flag::NaturalScale => st.natural_scale,
        Flag::NaturalWeight => st.natural_weight,
        Flag::AutoLoudness => st.auto_loudness,
        Flag::OneVoice => st.one_voice,
        Flag::Porta => st.porta,
        Flag::PortaVariable => st.porta_variable,
    }
}

/// Articulator `a`'s envelope (0) or LFO (1).
pub fn part(st: &State, a: usize, p: usize) -> &Curve {
    if p == 0 { &st.arts[a].env } else { &st.arts[a].lfo }
}

fn part_mut(st: &mut State, a: usize, p: usize) -> &mut Curve {
    if p == 0 { &mut st.arts[a].env } else { &mut st.arts[a].lfo }
}

pub fn get(st: &State, f: Field) -> i32 {
    match f {
        Field::Int(k) => st.ints[k],
        Field::Flag(g) => i32::from(flag_of(st, g)),
        Field::Part(a, p, k) => {
            let c = part(st, a, p);
            match k {
                0 => i32::from(c.enabled),
                1..=4 if p == 0 => c.scales[k - 1],
                1..=4 => c.lfo[k - 1],
                5 => i32::from(c.flags & SYNCED != 0),
                _ => i32::from(c.flags & GLOBAL != 0),
            }
        }
        Field::Comb(m) => st.combs[m],
        Field::EqType(b) => st.eq_types[b],
        Field::Quality => st.quality,
        Field::PortaLimit => i32::from(st.porta_limit),
    }
}

/// Sets a setting (clamped to its range).
pub fn set(st: &mut State, f: Field, v: i32) {
    let (lo, hi) = range(f);
    let v = v.clamp(lo, hi);
    match f {
        Field::Int(k) => st.ints[k] = v,
        Field::Flag(g) => *flag_ref(st, g) = v != 0,
        Field::Part(a, p, k) => {
            let c = part_mut(st, a, p);
            let flag = |flags: &mut u8, bit: u8| {
                if v != 0 { *flags |= bit } else { *flags &= !bit }
            };
            match k {
                0 => c.enabled = v != 0,
                1..=4 if p == 0 => c.scales[k - 1] = v,
                1..=4 => c.lfo[k - 1] = v,
                5 => flag(&mut c.flags, SYNCED),
                _ => flag(&mut c.flags, GLOBAL),
            }
            // What the walks work out from the curve.
            c.prepare();
        }
        Field::Comb(m) => st.combs[m] = v,
        Field::EqType(b) => st.eq_types[b] = v,
        Field::Quality => st.quality = v,
        Field::PortaLimit => st.porta_limit = v as u8,
    }
}

/// The range of int field `k` (`plugins/ogun.md`, the table of int fields).
fn int_range(k: usize) -> (i32, i32) {
    match k {
        LEVEL => (0, 256),
        COARSE => (-24, 24),
        FINE => (-50, 50),
        MOD_X | MOD_Y => (0, 512),
        PRE => (0, 1000),
        DEC | REL => (0, 10000),
        FULL => (-1000, 1000),
        MORPH => (0, 1000),
        SEED1 | SEED2 => (0, 9999),
        U_ORD => (1, 9),
        U_PAN | U_PHS => (0, 128),
        U_VOL => (16, 128),
        U_PITCH => (0, 256),
        CUT | RES => (0, 256),
        19 => (0, 9),
        20 | 21 => (0, 1024),
        22 | 23 => (0, 128),
        24 | 25 => (-128, 128),
        26 => (0, 127),
        27 => (0, 384),
        28 => (-512, 512),
        29 => (-128, 128),
        30 => (0, 2),
        31 => (19, 3000),
        32 | 37 => (5, 221),
        33 => (0, 384),
        34 => (1, 100),
        35 => (0, 100),
        36 => (1, 200),
        38 => (0, 4),
        39 => (-128, 128),
        40..=42 => (-1800, 1800),
        43..=48 => (0, 65536),
        F_TYPE | F_MODE => (0, 3),
        FLAT => (-1, 0),
        RICH => (6, 16),
        _ => (0, 1),
    }
}

pub fn range(f: Field) -> (i32, i32) {
    match f {
        Field::Int(k) => int_range(k),
        Field::Flag(_) => (0, 1),
        Field::Part(_, 0, 1 | 2 | 4) => (0, 256),
        Field::Part(_, 0, 3) => (-128, 128),
        Field::Part(_, 1, 1) => (512, 65536),
        Field::Part(_, 1, 2 | 3) => (-128, 128),
        Field::Part(_, 1, 4) => (-112, 112),
        Field::Part(..) => (0, 1),
        Field::Comb(_) => (0, 7),
        Field::EqType(_) => (0, 2),
        Field::Quality => (0, 4),
        Field::PortaLimit => (0, 5),
    }
}

/// The setting's default.
pub fn default(f: Field) -> i32 {
    get(&State::default(), f)
}

/// The knob position of a value, 0..1.
pub fn norm(f: Field, v: i32) -> f32 {
    let (lo, hi) = range(f);
    ((v - lo) as f32 / (hi - lo).max(1) as f32).clamp(0.0, 1.0)
}

/// The setting's value within its range (a state may hold values outside it; they play as stored).
pub fn shown(st: &State, f: Field) -> i32 {
    let (lo, hi) = range(f);
    get(st, f).clamp(lo, hi)
}

/// Whether the knob's arc starts at its centre.
pub fn is_bipolar(f: Field) -> bool {
    match f {
        Field::Int(k) => matches!(k, COARSE | FINE | MOD_X | MOD_Y | FULL | 24 | 25 | 28 | 29 | 39 | 40..=42),
        // An envelope's attack, decay and release are scales: 1 stands in the middle of the knob.
        Field::Part(_, 0, 1..=4) | Field::Part(_, 1, 2 | 3 | 4) => true,
        _ => false,
    }
}

pub const DELAY_MODES: [&str; 3] = ["Normal", "Inverted", "Ping-pong"];
pub const QUALITY_NAMES: [&str; 5] = ["Draft", "Average", "High", "Pristine", "Huge waste of CPU"];
pub const COMB_NAMES: [&str; 8] = ["Off", "-2", "-3", "-4", "-5", "+3", "+4", "+5"];
pub const FILTER_TYPES: [&str; 4] = ["Low pass", "Band pass", "Band stop", "High pass"];
pub const FILTER_MODES: [&str; 4] = ["Off", "x1", "x2", "x3"];
pub const EQ_TYPES: [&str; 3] = ["Low shelf", "Peak", "High shelf"];
pub const PORTA_LIMITS: [&str; 6] = ["None", "Octave wrap", "4 semitones", "3 semitones", "2 semitones", "1 semitone"];

/// The name the hint shows.
pub fn name(f: Field) -> String {
    match f {
        Field::Int(k) => match k {
            LEVEL => "Level",
            COARSE => "Coarse pitch",
            FINE => "Fine pitch",
            MOD_X => "Mod X",
            MOD_Y => "Mod Y",
            PRE => "Timbre pre-decay",
            DEC => "Timbre decay time",
            REL => "Timbre release time",
            FULL => "Timbre fullness",
            MORPH => "Timbre morphing",
            SEED1 => "Timbre seed 1",
            SEED2 => "Timbre seed 2",
            U_ORD => "Unison order",
            U_PAN => "Unison panning",
            U_VOL => "Unison volume",
            U_PITCH => "Unison pitch",
            U_PHS => "Unison phase",
            CUT => "Filter frequency",
            RES => "Filter resonance",
            19 => "Chorus order",
            20 => "Chorus depth",
            21 => "Chorus speed",
            22 => "Chorus delay",
            23 => "Chorus spread",
            24 => "Chorus cross",
            25 => "Chorus volume",
            26 => "Delay feedback",
            27 => "Delay time",
            28 => "Delay stereo offset",
            29 => "Delay volume",
            30 => "Delay feedback mode",
            31 => "Reverb low cut",
            32 => "Reverb high cut",
            33 => "Reverb predelay",
            34 => "Reverb room size",
            35 => "Reverb diffusion",
            36 => "Reverb decay",
            37 => "Reverb high damping",
            38 => "Reverb color",
            39 => "Reverb wet",
            40 => "EQ band 1 level",
            41 => "EQ band 2 level",
            42 => "EQ band 3 level",
            43 => "EQ band 1 freq",
            44 => "EQ band 2 freq",
            45 => "EQ band 3 freq",
            46 => "EQ band 1 width",
            47 => "EQ band 2 width",
            48 => "EQ band 3 width",
            F_TYPE => "Filter type",
            F_MODE => "Filter mode",
            FLAT => "Flat filter response",
            RICH => "Timbre richness",
            _ => "",
        }
        .into(),
        Field::Flag(g) => match g {
            Flag::Smooth => "Smooth X/Y",
            Flag::Declick => "Declick",
            Flag::AltDecay => "Alternative decay",
            Flag::ReverbOn => "Reverb",
            Flag::ReverbTempo => "Tempo-based reverb predelay",
            Flag::DelayOn => "Delay",
            Flag::DelayTempo => "Tempo-based delay time",
            Flag::LinkRelease => "Link timbre release to volume decay",
            Flag::NaturalScale => "Natural EQ mapping scale",
            Flag::NaturalWeight => "Natural EQ mapping weighting",
            Flag::AutoLoudness => "Auto loudness",
            Flag::OneVoice => "Limit to 1 voice per key",
            Flag::Porta => "Portamento",
            Flag::PortaVariable => "Variable portamento time",
        }
        .into(),
        Field::Part(_, p, k) => match (p, k) {
            (0, 0) => "Enable envelope".into(),
            (_, 0) => "Enable LFO".into(),
            (0, 1..=4) => ["Attack time scale", "Decay time scale", "Sustain level offset", "Release time scale"][k - 1].into(),
            (_, 1..=4) => ["LFO speed", "LFO tension", "LFO skew", "LFO pulse width"][k - 1].into(),
            (_, 5) => "Tempo-based time".into(),
            (0, _) => "Shared / retriggered envelope".into(),
            _ => "Shared / retriggered LFO".into(),
        },
        Field::Comb(_) => "Comb".into(),
        Field::EqType(b) => format!("EQ band {} type", b + 1),
        Field::Quality => "Processing quality".into(),
        Field::PortaLimit => "Portamento limit".into(),
    }
}

fn pct(v: i32, full: f32) -> String {
    format!("{}%", (v as f32 / full * 100.0).round() as i32)
}

fn sign(v: i32) -> &'static str {
    if v > 0 { "+" } else { "" }
}

/// A time in seconds as the decay and release hints show it.
fn seconds(t: f64) -> String {
    if t < 1.0 { format!("{} ms", (t * 1000.0).round() as i32) } else { format!("{t:.2} s") }
}

/// The value as Citrus shows the same effect setting.
fn citrus_text(f: CitrusField, v: i32) -> String {
    citrus::ctl::text(f, v)
}

/// A setting's value as its hint shows it.
pub fn text(f: Field, v: i32) -> String {
    let on_off = |v: i32| (if v != 0 { "on" } else { "off" }).to_string();
    match f {
        Field::Int(k) => match k {
            LEVEL => pct(v, 128.0),
            COARSE => format!("{}{v} semitones", sign(v)),
            FINE => format!("{}{v} cents", sign(v)),
            MOD_X | MOD_Y => pct(v - 256, 256.0),
            PRE | MORPH => pct(v, 1000.0),
            FULL => pct(v, 1000.0),
            DEC if v >= 10000 => "Never".into(),
            DEC | REL => seconds(time_of(f64::from(v) / 10000.0)),
            SEED1 | SEED2 if v == 0 => "----".into(),
            SEED1 | SEED2 | U_ORD | RICH => v.to_string(),
            U_PAN | U_VOL | U_PHS | U_PITCH | CUT | RES => pct(v, if k == CUT || k == RES { 256.0 } else { 128.0 }),
            19 if v == 0 => "Off".into(),
            19 => v.to_string(),
            20 | 21 => pct(v, 1024.0),
            22..=25 => pct(v, 128.0),
            26 => pct(v, 127.0),
            27 => citrus_text(CitrusField::Delay(0, 1), v),
            28 => citrus_text(CitrusField::Delay(0, 2), v),
            29 | 39 => pct(v, 128.0),
            30 => DELAY_MODES[v.clamp(0, 2) as usize].into(),
            31..=38 => citrus_text(CitrusField::Reverb(k - 31), v),
            40..=42 => format!("{:.1}dB", v as f32 / 100.0),
            43..=45 => citrus_text(CitrusField::Eq(0, 1), v),
            46..=48 => citrus_text(CitrusField::Eq(0, 2), v),
            F_TYPE => FILTER_TYPES[v.clamp(0, 3) as usize].into(),
            F_MODE => FILTER_MODES[v.clamp(0, 3) as usize].into(),
            FLAT => on_off(v),
            _ => v.to_string(),
        },
        Field::Flag(_) | Field::Part(_, _, 0 | 5 | 6) => on_off(v),
        Field::Part(_, 1, 1) => pct(v - 512, (65536 - 512) as f32),
        Field::Part(_, 1, 4) => pct(v, 112.0),
        Field::Part(..) => pct(v, 128.0),
        Field::Comb(_) => COMB_NAMES[v.clamp(0, 7) as usize].into(),
        Field::EqType(_) => EQ_TYPES[v.clamp(0, 2) as usize].into(),
        Field::Quality => QUALITY_NAMES[v.clamp(0, 4) as usize].into(),
        Field::PortaLimit => PORTA_LIMITS[v.clamp(0, 5) as usize].into(),
    }
}

/// The hint of a setting: its name and value, a switch its name alone.
pub fn hint(st: &State, f: Field) -> String {
    let v = get(st, f);
    match f {
        Field::Flag(_) | Field::Part(_, _, 0 | 5 | 6) | Field::Int(FLAT) => name(f),
        Field::Int(27) if st.delay_tempo => format!("{}: {}", name(f), citrus::ctl::time_text(v, true)),
        Field::Int(33) if st.reverb_tempo => format!("{}: {}", name(f), citrus::ctl::time_text(v, true)),
        _ => format!("{}: {}", name(f), text(f, v)),
    }
}

/// A typed value: the value whose text shows it (or a name), else the nearest by the text's number. The current value
/// stays when it shows the typed text.
pub fn parse(f: Field, s: &str, cur: i32) -> Option<i32> {
    let t = s.trim().to_ascii_lowercase();
    let (lo, hi) = range(f);
    let number = |s: &str| -> Option<f64> {
        let n: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
        n.parse().ok()
    };
    let same = |v: i32| text(f, v).to_ascii_lowercase() == t;
    if (lo..=hi).contains(&cur) && same(cur) {
        return Some(cur);
    }
    if hi - lo <= 16384 {
        if let Some(v) = (lo..=hi).find(|&v| same(v)) {
            return Some(v);
        }
    }
    let x = number(&t)?;
    let step = ((hi - lo) / 4096).max(1);
    let mut best: Option<(f64, i32)> = None;
    let mut v = lo;
    while v <= hi {
        if let Some(r) = number(&text(f, v)) {
            let d = (r - x).abs();
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, v));
            }
        }
        v += step;
    }
    best.map(|(_, v)| v)
}

/// The value a switch moves to.
pub fn toggled(f: Field, v: i32) -> i32 {
    match f {
        Field::Int(FLAT) => if v != 0 { 0 } else { -1 },
        _ => i32::from(v == 0),
    }
}

/// A random preset: Lemon Studio's own small law (Ogun's was not studied), the same for the same number.
pub fn random_preset(n: u32) -> State {
    let mut x = u64::from(n).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut next = |lo: i32, hi: i32| -> i32 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        lo + ((x >> 33) % (hi - lo + 1) as u64) as i32
    };
    let mut st = State::default();
    st.ints[RICH] = next(9, 13);
    st.ints[SEED1] = next(1, 9999);
    st.ints[SEED2] = next(1, 9999);
    st.ints[PRE] = next(0, 600);
    st.ints[DEC] = next(3000, 9000);
    st.ints[REL] = next(3000, 8000);
    st.ints[FULL] = next(-500, 500);
    st.ints[MORPH] = next(0, 800);
    st.ints[U_ORD] = next(1, 5);
    st.ints[U_PITCH] = next(0, 48);
    st.comment = format!("Random preset {n}");
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default state's settings lie within their ranges; a setting set is the setting read, clamped.
    #[test]
    fn settings_set_within_their_ranges() {
        let st = State::default();
        for k in 0..INTS {
            let f = Field::Int(k);
            let (lo, hi) = range(f);
            assert!((lo..=hi).contains(&get(&st, f)), "{}", name(f));
            assert!(!name(f).is_empty(), "{k}");
        }
        let mut s = st.clone();
        set(&mut s, Field::Int(RICH), 99);
        assert_eq!(s.ints[RICH], 16);
        set(&mut s, Field::Part(A_VOLUME, 0, 0), 1);
        assert!(s.arts[A_VOLUME].env.enabled);
        set(&mut s, Field::Part(A_PITCH, 1, 5), 1);
        assert_eq!(get(&s, Field::Part(A_PITCH, 1, 5)), 1);
    }

    #[test]
    fn value_texts() {
        assert_eq!(text(Field::Int(LEVEL), 128), "100%");
        assert_eq!(text(Field::Int(COARSE), 3), "+3 semitones");
        assert_eq!(text(Field::Int(SEED1), 0), "----");
        assert_eq!(text(Field::Int(DEC), 10000), "Never");
        assert_eq!(text(Field::Int(DEC), 2000), "886 ms");
        assert_eq!(text(Field::Int(REL), 10000), "20.00 s");
        assert_eq!(text(Field::Int(40), -300), "-3.0dB");
        assert_eq!(text(Field::Int(32), 221), "Off");
        assert_eq!(text(Field::Comb(M_DECAY), 5), "+3");
    }

    #[test]
    fn typed_values_read_back() {
        let f = Field::Int(LEVEL);
        assert_eq!(parse(f, "100%", 0), Some(128));
        assert_eq!(parse(Field::Int(SEED1), "----", 5), Some(0));
        assert_eq!(parse(Field::Int(SEED1), "77", 5), Some(77));
        assert_eq!(parse(Field::Int(F_TYPE), "band stop", 0), Some(2));
        assert_eq!(parse(f, "words", 3), None);
        // The current value stays when it shows the typed text.
        assert_eq!(parse(Field::Int(FULL), "50%", 501), Some(501));
    }

    #[test]
    fn flat_is_minus_one_when_on() {
        assert_eq!(toggled(Field::Int(FLAT), 0), -1);
        assert_eq!(toggled(Field::Int(FLAT), -1), 0);
        assert_eq!(toggled(Field::Flag(Flag::Smooth), 1), 0);
    }
}
