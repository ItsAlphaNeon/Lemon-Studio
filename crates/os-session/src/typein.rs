// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! "Enter value...": the value box of the knob and parameter menus. It is the playlist's inline text box
//! (`EditTarget::TypeIn`) drawn as FL's value box, reading the text in the knob's own unit or as a normalized value.
//!
// spec: ui/type-in-value.md

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use crate::knob_id::KnobId;

/// What a value box sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Knob(KnobId),
    /// A mixer track's volume fader (0..1, the fader law; spec: ui/mixer.md §4).
    Fader(u16),
    /// A plugin parameter: hosted key, parameter index.
    Param(usize, usize),
    /// A mixer track's input latency (`true`) or output latency setting, typed in a unit (spec: audio/track-io.md
    /// §2.6).
    Latency(u16, bool, LatUnit),
    /// The mixer track box of the channel on rack row `.0`, typed as a track number, 0 or "Master" for the master (Lemon
    /// Studio's design: spec ui/type-in-value.md "Lemon Studio").
    RackTrack(usize),
    /// The mute LED of the channel on rack row `.0`: "On" (playing) or "Off" (muted) (Lemon Studio's design, as above).
    RackLed(usize),
    /// The wet (`true`) or dry level of member `.2` (its id) of the container in slot `.1` of mixer track `.0`, as a
    /// percent (Lemon Studio's design: `crate::container`).
    Member(u16, u8, u32, bool),
    /// Parameter `.3` of member `.2` (its id) of the container in slot `.1` of mixer track `.0`: a plugin parameter
    /// named by what a project keeps, for remote control links (`crate::params::link_target`).
    MemberParam(u16, u8, u32, usize),
}

/// The units a latency setting is typed in: Set in ms / samples / beats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatUnit {
    Ms,
    Samples,
    Beats,
}

/// The range a latency setting takes, in ms (OS).
pub const LATENCY_MAX_MS: f64 = 1000.0;

/// Clicks on the box's parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TiAct {
    /// The field (does nothing: the box is modal).
    Field,
    /// A mode button: `true` = normalized ("A").
    Mode(bool),
    Accept,
    Close,
}

/// Box size and the corner offset from the mouse (spec §1, §2).
pub const W: f32 = 244.0;
pub const W_PLAIN: f32 = 202.0;
pub const H: f32 = 51.0;
pub const CORNER: f32 = 10.0;

/// How the value mode reads and writes a knob (spec §4, §6, §8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Unit {
    /// "Centered", "30% right"; "left" is the negative side.
    Pan,
    /// "Original", "40% merged"; "separated" is the negative side.
    Sep,
    /// dB through the control's gain law.
    Db(Law),
    /// A percent of the knob (OS: swing, mix level).
    Percent,
    /// Master pitch: A4 in Hz.
    Hz,
    /// No value mode (plugin parameters, OS).
    Plain,
    /// A latency setting in its unit (no value mode).
    Latency,
    /// A rack row's mixer track box: "Master", "Bus 7" (no value mode).
    Track,
    /// A rack row's mute LED: "On" / "Off" (no value mode).
    Switch,
}

/// The gain laws of the dB controls (spec: audio/gain-laws.md).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Law {
    Channel,
    Main,
    Fader,
}

pub fn unit(t: Target) -> Unit {
    match t {
        Target::Knob(KnobId::ChanPan(_) | KnobId::MixPan(_)) => Unit::Pan,
        Target::Knob(KnobId::MixSep(_)) => Unit::Sep,
        Target::Knob(KnobId::ChanVol(_)) => Unit::Db(Law::Channel),
        Target::Knob(KnobId::MasterVol) => Unit::Db(Law::Main),
        Target::Knob(KnobId::Send(..)) | Target::Fader(_) => Unit::Db(Law::Fader),
        Target::Knob(KnobId::Swing | KnobId::SlotMix(..) | KnobId::Member(..)) | Target::Member(..) => Unit::Percent,
        Target::Knob(KnobId::MasterPitch) => Unit::Hz,
        // The equaliser's settings take the plain 0..1 position for now.
        Target::Param(..) | Target::MemberParam(..) | Target::Knob(KnobId::Eq(..) | KnobId::Strum(_) | KnobId::Arp(_) | KnobId::PianoDlg(_) | KnobId::AutoLfo(..) | KnobId::MidiOut(..)) => Unit::Plain,
        Target::Latency(..) => Unit::Latency,
        Target::RackTrack(_) => Unit::Track,
        Target::RackLed(_) => Unit::Switch,
    }
}

/// The value-mode button's label and its name in the hint ("Enter value as: <name>"), when the box has the two
/// mode buttons (spec §3).
pub fn mode_button(t: Target) -> Option<(&'static str, &'static str)> {
    match unit(t) {
        Unit::Pan | Unit::Sep => Some(("C", "Centered")),
        Unit::Db(_) => Some(("dB", "dB")),
        Unit::Percent => Some(("%", "%")),
        Unit::Hz | Unit::Plain | Unit::Latency | Unit::Track | Unit::Switch => None,
    }
}

pub fn width(t: Target) -> f32 {
    if mode_button(t).is_some() { W } else { W_PLAIN }
}

/// The mode a box opens in: the remembered one, or the only one it has.
pub fn opening_norm(t: Target, remembered: bool) -> bool {
    match unit(t) {
        Unit::Hz | Unit::Latency | Unit::Track | Unit::Switch => false,
        Unit::Plain => true,
        _ => remembered,
    }
}

/// The gain at position `n` (spec: audio/gain-laws.md).
pub fn gain(l: Law, n: f64) -> f64 {
    match l {
        Law::Main => f64::from(os_dsp::level::curve((n / 0.8) as f32)),
        Law::Fader => os_engine::laws::fader_gain(n),
        Law::Channel => os_engine::laws::channel_gain(n),
    }
}

/// The position 0..1 whose gain is `g` (the laws rise monotonically), clamped to the knob's range.
pub fn position_for_gain(k: Law, g: f64) -> f64 {
    if g <= 0.0 {
        return 0.0;
    }
    if g >= gain(k, 1.0) {
        return 1.0;
    }
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if gain(k, mid) < g { lo = mid } else { hi = mid }
    }
    (lo + hi) / 2.0
}

/// `v` with at most `places` decimals and no trailing zeros ("0.5", "1", "466.16376151809").
pub fn short(v: f64, places: usize) -> String {
    let t = format!("{v:.places$}");
    let t = if t.contains('.') { t.trim_end_matches('0').trim_end_matches('.') } else { &t };
    if t == "-0" { "0".into() } else { t.to_string() }
}

/// A number, optionally ending in "%": (value, had the percent sign). Infinities and NaN are not numbers here.
pub fn number(word: &str) -> Option<(f64, bool)> {
    let w = word.trim();
    let (w, pct) = match w.strip_suffix('%') {
        Some(rest) => (rest.trim_end(), true),
        None => (w, false),
    };
    let v: f64 = w.parse().ok()?;
    v.is_finite().then_some((v, pct))
}

pub fn strip_suffix_ci<'a>(text: &'a str, suffix: &str) -> &'a str {
    let n = text.len();
    if n >= suffix.len() && text.is_char_boundary(n - suffix.len()) && text[n - suffix.len()..].eq_ignore_ascii_case(suffix) {
        text[..n - suffix.len()].trim_end()
    } else {
        text
    }
}

impl LatUnit {
    pub fn name(self) -> &'static str {
        match self {
            LatUnit::Ms => "ms",
            LatUnit::Samples => "samples",
            LatUnit::Beats => "beats",
        }
    }

    /// Milliseconds per unit: a sample at the device's rate (44.1 kHz with none open), a beat at `tempo`.
    pub fn ms(self, tempo: f64) -> f64 {
        match self {
            LatUnit::Ms => 1.0,
            LatUnit::Samples => 1000.0 / f64::from(crate::device::rate().unwrap_or(44100).max(1)),
            LatUnit::Beats => 60000.0 / tempo.max(1.0),
        }
    }
}
