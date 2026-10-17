// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! "Type in value...": the value box of the knob and parameter menus. It is the playlist's inline text box
//! (`EditTarget::TypeIn`) drawn as FL's value box, reading the text in the knob's own unit or as a normalized value.
//!
// spec: ui/type-in-value.md

use crate::app::State;
use crate::knobs::KnobId;
use crate::playlist::EditTarget;

/// What a value box sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Knob(KnobId),
    /// A mixer track's volume fader (0..1, the fader law; spec: ui/mixer.md §4).
    Fader(u16),
    /// A plugin parameter: hosted key, parameter index.
    Param(usize, usize),
}

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
const CORNER: f32 = 10.0;

/// How the value mode reads and writes a knob (spec §4, §6, §8).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Unit {
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
}

/// The gain laws of the dB controls (spec: audio/gain-laws.md).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Law {
    Channel,
    Main,
    Fader,
}

fn unit(t: Target) -> Unit {
    match t {
        Target::Knob(KnobId::ChanPan(_) | KnobId::MixPan(_)) => Unit::Pan,
        Target::Knob(KnobId::MixSep(_)) => Unit::Sep,
        Target::Knob(KnobId::ChanVol(_)) => Unit::Db(Law::Channel),
        Target::Knob(KnobId::MasterVol) => Unit::Db(Law::Main),
        Target::Knob(KnobId::Send(..)) | Target::Fader(_) => Unit::Db(Law::Fader),
        Target::Knob(KnobId::Swing | KnobId::SlotMix(..)) => Unit::Percent,
        Target::Knob(KnobId::MasterPitch) => Unit::Hz,
        // The equaliser's settings take the plain 0..1 position for now.
        Target::Param(..) | Target::Knob(KnobId::Eq(..)) => Unit::Plain,
    }
}

/// The value-mode button's label and its name in the hint ("Type in value as: <name>"), when the box has the two
/// mode buttons (spec §3).
pub fn mode_button(t: Target) -> Option<(&'static str, &'static str)> {
    match unit(t) {
        Unit::Pan | Unit::Sep => Some(("C", "Centered")),
        Unit::Db(_) => Some(("dB", "dB")),
        Unit::Percent => Some(("%", "%")),
        Unit::Hz | Unit::Plain => None,
    }
}

pub fn width(t: Target) -> f32 {
    if mode_button(t).is_some() { W } else { W_PLAIN }
}

/// The mode a box opens in: the remembered one, or the only one it has.
fn opening_norm(t: Target, remembered: bool) -> bool {
    match unit(t) {
        Unit::Hz => false,
        Unit::Plain => true,
        _ => remembered,
    }
}

/// The caption under the field (spec §6, §7).
pub fn caption(s: &State, t: Target) -> String {
    let name = match t {
        Target::Knob(KnobId::MasterPitch) => return "Master pitch (Hz)".into(),
        Target::Knob(k @ (KnobId::MixPan(m) | KnobId::MixSep(m) | KnobId::SlotMix(m, _))) => {
            let label = if let KnobId::MixSep(_) = k { "Stereo separation".to_string() } else { knob_label(s, k) };
            format!("{} - {label}", crate::mixer::name(s, m))
        }
        Target::Knob(k) => knob_label(s, k),
        Target::Fader(m) => format!("{} - Volume", crate::mixer::name(s, m)),
        Target::Param(key, index) => crate::params::param_name(s, key, index).unwrap_or_else(|| format!("Parameter {}", index + 1)),
    };
    format!("Value for {name}")
}

/// The knob's hint text up to its colon ("Channel volume").
fn knob_label(s: &State, k: KnobId) -> String {
    let text = s.knob_text(k);
    text.split_once(':').map_or(text.clone(), |(a, _)| a.to_string())
}

/// The gain at position `n` (spec: audio/gain-laws.md).
fn gain(l: Law, n: f64) -> f64 {
    match l {
        Law::Main => f64::from(os_dsp::level::curve((n / 0.8) as f32)),
        Law::Fader => os_engine::laws::fader_gain(n),
        Law::Channel => os_engine::laws::channel_gain(n),
    }
}

/// The position 0..1 whose gain is `g` (the laws rise monotonically), clamped to the knob's range.
fn position_for_gain(k: Law, g: f64) -> f64 {
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

fn get(s: &State, t: Target) -> Option<f64> {
    match t {
        Target::Knob(k) => Some(f64::from(s.knob(k))),
        Target::Fader(m) => Some(crate::mixer::track(s, m)?.volume.unwrap_or(0.8)),
        Target::Param(key, index) => crate::params::param_value(s, key, index),
    }
}

fn set(s: &mut State, t: Target, v: f64) {
    match t {
        Target::Knob(k) => {
            crate::history::knob(s, k, false);
            s.set_knob(k, v as f32);
        }
        Target::Fader(m) => {
            if let Some(t) = crate::mixer::track_mut(s, m) {
                t.volume = Some(v.clamp(0.0, 1.0));
                s.piano.dirty = true;
            }
        }
        Target::Param(key, index) => {
            if !crate::params::set_param_value(s, key, index, v) {
                s.message = Some("The plugin did not take the value".into());
            }
        }
    }
}

/// `v` with at most `places` decimals and no trailing zeros ("0.5", "1", "466.16376151809").
fn short(v: f64, places: usize) -> String {
    let t = format!("{v:.places$}");
    let t = if t.contains('.') { t.trim_end_matches('0').trim_end_matches('.') } else { &t };
    if t == "-0" { "0".into() } else { t.to_string() }
}

/// The field's text for the box's current value in a mode (spec §3, §6).
pub fn text(s: &State, t: Target, norm: bool) -> String {
    let n = get(s, t).unwrap_or(0.0);
    if norm {
        return short(n, 4);
    }
    let bipolar = (n - 0.5) * 2.0;
    match unit(t) {
        Unit::Pan => crate::knobs::pan_text(bipolar as f32),
        Unit::Sep => crate::mixer::sep_text(bipolar).trim_start_matches("Stereo separation: ").to_string(),
        Unit::Db(k) => {
            let g = gain(k, n);
            if g > 0.0 { format!("{:.1}dB", 20.0 * g.log10()) } else { "-INFdB".into() }
        }
        Unit::Percent => format!("{:.0}%", n * 100.0),
        Unit::Hz => short(440.0 * 2f64.powf(bipolar), 11),
        Unit::Plain => short(n, 4),
    }
}

/// A number, optionally ending in "%": (value, had the percent sign). Infinities and NaN are not numbers here.
fn number(word: &str) -> Option<(f64, bool)> {
    let w = word.trim();
    let (w, pct) = match w.strip_suffix('%') {
        Some(rest) => (rest.trim_end(), true),
        None => (w, false),
    };
    let v: f64 = w.parse().ok()?;
    v.is_finite().then_some((v, pct))
}

fn strip_suffix_ci<'a>(text: &'a str, suffix: &str) -> &'a str {
    let n = text.len();
    if n >= suffix.len() && text.is_char_boundary(n - suffix.len()) && text[n - suffix.len()..].eq_ignore_ascii_case(suffix) {
        text[..n - suffix.len()].trim_end()
    } else {
        text
    }
}

/// The knob position (0..1) typed `text` asks for; `None` leaves the value as it is: empty or unreadable text
/// (spec §4, §5, §8; unreadable = unchanged is the OS choice).
pub fn parse(t: Target, norm: bool, text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let u = unit(t);
    if norm || u == Unit::Plain {
        // spec §5: up to 1 = the position, above 1 or with "%" = a percent.
        let (v, pct) = number(text)?;
        return Some(if pct || v > 1.0 { v / 100.0 } else { v }.clamp(0.0, 1.0));
    }
    match u {
        Unit::Pan | Unit::Sep => {
            let (zero, negative) = if u == Unit::Pan { (&["centered", "centred", "center", "centre"][..], "left") } else { (&["original"][..], "separated") };
            let mut words = text.split_whitespace();
            let first = words.next()?;
            if zero.iter().any(|z| first.eq_ignore_ascii_case(z)) {
                return Some(0.5);
            }
            let (v, _) = number(first)?;
            let v = if words.any(|w| w.eq_ignore_ascii_case(negative)) { -v.abs() } else { v };
            Some(0.5 + (v / 100.0).clamp(-1.0, 1.0) / 2.0)
        }
        Unit::Db(k) => {
            let t = strip_suffix_ci(text, "db");
            if t.eq_ignore_ascii_case("-inf") {
                return Some(0.0);
            }
            match number(t)? {
                (_, true) => None,
                (db, false) => Some(position_for_gain(k, 10f64.powf(db / 20.0))),
            }
        }
        Unit::Percent => number(text).map(|(v, _)| (v / 100.0).clamp(0.0, 1.0)),
        Unit::Hz => {
            let (hz, _) = number(strip_suffix_ci(text, "hz"))?;
            (hz > 0.0).then(|| 0.5 + (1200.0 * (hz / 440.0).log2()).clamp(-1200.0, 1200.0) / 2400.0)
        }
        Unit::Plain => None,
    }
}

/// "Type in value...": the box at the mouse (spec §1), its field holding the value in the remembered mode.
pub fn open(s: &mut State, t: Target, mouse: (f32, f32)) {
    let norm = opening_norm(t, s.playlist.typein_norm);
    let w = width(t);
    let at = (
        (mouse.0 + CORNER - w).clamp(0.0, (s.screen.0 - w).max(0.0)),
        (mouse.1 + CORNER - H).clamp(0.0, (s.screen.1 - H).max(0.0)),
    );
    crate::playlist::start_edit(s, EditTarget::TypeIn(t, norm), at);
    if let Some(e) = s.playlist.edit.as_mut() {
        e.sel = true;
    }
}

/// Enter / ✓: set the value from the text and remember the mode (spec §3).
pub fn accept(s: &mut State, t: Target, norm: bool, typed: &str) {
    if mode_button(t).is_some() {
        s.playlist.typein_norm = norm;
    }
    // The field as it opened keeps the exact value (no rounding to the shown decimals, OS).
    if typed.trim() == text(s, t, norm).trim() {
        return;
    }
    if let Some(v) = parse(t, norm, typed) {
        set(s, t, v);
    }
}

/// A click on the open box.
pub fn click(s: &mut State, a: TiAct) {
    match a {
        TiAct::Field => {}
        TiAct::Mode(norm) => {
            let Some(EditTarget::TypeIn(t, cur)) = s.playlist.edit.as_ref().map(|e| e.target) else { return };
            if cur != norm && mode_button(t).is_some() {
                let text = text(s, t, norm);
                if let Some(e) = s.playlist.edit.as_mut() {
                    e.target = EditTarget::TypeIn(t, norm);
                    e.text = text;
                    e.sel = true;
                }
            }
        }
        TiAct::Accept => crate::playlist::commit_edit(s),
        TiAct::Close => s.playlist.edit = None,
    }
}

/// Whether the open inline text box is a value box (it is modal, spec §1).
pub fn is_open(s: &State) -> bool {
    matches!(s.playlist.edit.as_ref().map(|e| e.target), Some(EditTarget::TypeIn(..)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_words_and_numbers() {
        let t = Target::Knob(KnobId::ChanPan(0));
        let pan = |s: &str| parse(t, false, s).map(|v| ((v - 0.5) * 200.0).round() as i32);
        assert_eq!(pan("50"), Some(50));
        assert_eq!(pan("-25"), Some(-25));
        assert_eq!(pan("150"), Some(100));
        assert_eq!(pan("30%"), Some(30));
        assert_eq!(pan("25% left"), Some(-25));
        assert_eq!(pan("30 LEFT"), Some(-30));
        assert_eq!(pan("-30 right"), Some(-30));
        assert_eq!(pan("30 l"), Some(30));
        assert_eq!(pan("  -45  "), Some(-45));
        assert_eq!(pan("Centered"), Some(0));
        assert_eq!(pan("abc"), None);
        assert_eq!(pan("7abc"), None);
        assert_eq!(pan(""), None);
    }

    #[test]
    fn normalized_mode() {
        let t = Target::Knob(KnobId::ChanVol(0));
        assert_eq!(parse(t, true, "0.5"), Some(0.5));
        assert_eq!(parse(t, true, "75"), Some(0.75));
        assert_eq!(parse(t, true, "30%"), Some(0.3));
        assert_eq!(parse(t, true, "1"), Some(1.0));
        assert_eq!(parse(t, true, "1.5"), Some(0.015));
        assert_eq!(parse(t, true, "-0.5"), Some(0.0));
        assert_eq!(parse(t, true, "-6dB"), None);
    }

    #[test]
    fn decibels_and_hertz() {
        let t = Target::Knob(KnobId::ChanVol(0));
        let n = parse(t, false, "-6 dB").unwrap();
        assert!((20.0 * gain(Law::Channel, n).log10() + 6.0).abs() < 1e-6);
        assert_eq!(parse(t, false, "-6dB"), Some(n));
        assert_eq!(parse(t, false, "3 dB"), Some(1.0));
        assert_eq!(parse(t, false, "-inf"), Some(0.0));
        assert_eq!(parse(t, false, "50%"), None);
        let p = Target::Knob(KnobId::MasterPitch);
        let cents = |s: &str| parse(p, false, s).map(|v| ((v - 0.5) * 2400.0).round() as i32);
        assert_eq!(cents("466.16"), Some(100));
        assert_eq!(cents("880"), Some(1200));
        assert_eq!(cents("100"), Some(-1200));
        assert_eq!(cents("440 Hz"), Some(0));
        assert_eq!(short(466.16376151809, 11), "466.16376151809");
        assert_eq!(short(0.5, 4), "0.5");
        assert_eq!(short(1.0, 4), "1");
    }
}
