// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::typein` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use crate::app::State;
use crate::knobs::KnobId;
use crate::playlist::EditTarget;
#[allow(unused_imports)]
use crate::typein::*;

/// A member's parameter as the hosted instance's (`Target::Param`), while its effect is loaded.
fn resolved(s: &State, t: Target) -> Option<Target> {
    match t {
        Target::MemberParam(m, k, id, i) => Some(Target::Param(crate::container::member_key(&s.session, crate::container::place_key(m, k), id)?, i)),
        t => Some(t),
    }
}

/// The caption under the field (spec §6, §7).
pub fn caption(s: &State, t: Target) -> String {
    match t {
        Target::Knob(KnobId::MasterPitch) => "Master pitch (Hz)".into(),
        _ => format!("Value for {}", target_name(s, t)),
    }
}

/// What a target is called: "Bus 1 - Volume", "Channel volume", a plugin parameter's name (`crate::remote::label`
/// names the owner of every kind).
pub fn target_name(s: &State, t: Target) -> String {
    match t {
        Target::Knob(k @ (KnobId::MixPan(m) | KnobId::MixSep(m) | KnobId::SlotMix(m, _))) => {
            let label = if let KnobId::MixSep(_) = k { "Stereo separation".to_string() } else { knob_label(s, k) };
            format!("{} - {label}", crate::mixer::name(&s.session, m))
        }
        Target::Knob(k) => knob_label(s, k),
        Target::Fader(m) => format!("{} - Volume", crate::mixer::name(&s.session, m)),
        Target::Param(key, index) => crate::params_ui::param_name(s, key, index).unwrap_or_else(|| format!("Parameter {}", index + 1)),
        Target::Latency(m, input, u) => format!("{} - {} latency ({})", crate::mixer::name(&s.session, m), if input { "Input" } else { "Track" }, u.name()),
        Target::RackTrack(_) => "Target mixer track".into(),
        Target::RackLed(_) => "Channel on / off".into(),
        Target::Member(m, k, id, wet) => crate::container::level_name(&s.session, m, k, id, wet),
        Target::MemberParam(.., i) => match resolved(s, t) {
            Some(p @ Target::Param(..)) => target_name(s, p),
            _ => format!("Parameter {}", i + 1),
        },
    }
}

/// The knob's hint text up to its colon ("Channel volume").
fn knob_label(s: &State, k: KnobId) -> String {
    let text = s.knob_text(k);
    text.split_once(':').map_or(text.clone(), |(a, _)| a.to_string())
}

pub(crate) fn get(s: &State, t: Target) -> Option<f64> {
    match t {
        Target::Knob(k) => Some(f64::from(s.knob(k))),
        Target::Fader(m) => Some(crate::mixer::track(&s.session, m)?.volume.unwrap_or(0.8)),
        Target::Param(key, index) => crate::params_ui::param_value(s, key, index),
        Target::Latency(m, input, _) => crate::mixer::track(&s.session, m).map(|t| f64::from(if input { t.latency_in } else { t.latency_out })),
        Target::RackTrack(r) => s.session.project.channels.get(r).map(|c| f64::from(c.mixer_track)),
        Target::RackLed(r) => s.session.project.channels.get(r).map(|c| if c.enabled { 1.0 } else { 0.0 }),
        Target::Member(m, k, id, wet) => crate::container::level(&s.session, m, k, id, wet).map(f64::from),
        Target::MemberParam(..) => match resolved(s, t)? {
            p @ Target::Param(..) => get(s, p),
            _ => None,
        },
    }
}

fn set(s: &mut State, t: Target, v: f64) {
    match t {
        Target::Knob(k) => {
            crate::history::knob(&mut s.session, k, false);
            s.set_knob(k, v as f32);
        }
        Target::Fader(m) => crate::control_ui::knob_edit(s, crate::history::MIXER_VOLUME, crate::control::command::Region::Track(m), |s| {
            if let Some(t) = crate::mixer::track_mut(&mut s.session, m) {
                t.volume = Some(v.clamp(0.0, 1.0));
                s.session.dirty = true;
            }
        }),
        Target::Param(key, index) => {
            let before = crate::params_ui::param_value(s, key, index);
            if crate::params_ui::set_param_value(s, key, index, v) {
                crate::control_ui::param_set(s, key, index, before, v);
            } else {
                s.message = Some("The plugin did not take the value".into());
            }
        }
        // `v` is in the box's unit.
        Target::Latency(m, input, u) => crate::mixer::set_latency(s, m, input, (v * u.ms(s.tempo())).clamp(-LATENCY_MAX_MS, LATENCY_MAX_MS) as f32),
        // One undo step, as the box's own menu commands.
        Target::RackTrack(r) => {
            let track = v.round().clamp(0.0, f64::from(crate::mixer::max_insert(&s.session.project))) as u16;
            if s.session.project.channels.get(r).is_some_and(|c| c.mixer_track != track) {
                crate::control_ui::edit(s, "Channel mixer track", crate::control::command::Region::Channel(r), |s| {
                    if let Some(c) = s.session.project.get_mut().channels.get_mut(r) {
                        c.mixer_track = track;
                    }
                });
                s.session.dirty = true;
            }
        }
        Target::RackLed(r) => {
            if s.session.project.channels.get(r).is_some_and(|c| c.enabled != (v >= 0.5)) {
                crate::rack::toggle_mute(s, r);
            }
        }
        Target::Member(m, k, id, wet) => crate::container_ui::set_level(s, m, k, id, wet, v.clamp(0.0, 1.0) as f32),
        Target::MemberParam(..) => match resolved(s, t) {
            Some(p @ Target::Param(..)) => set(s, p, v),
            _ => s.message = Some("This effect is not loaded (its plugin was not found)".into()),
        },
    }
}

/// The 0..1 position of a rack row's mixer track box or LED, for Copy value and Paste value (Lemon Studio's design):
/// the track over the highest insert, the LED 0 (off) or 1 (on). `None` for the other targets, which copy their knob.
pub(crate) fn position(s: &State, t: Target) -> Option<f64> {
    let v = get(s, t)?;
    match t {
        Target::RackTrack(_) => Some(v / f64::from(crate::mixer::max_insert(&s.session.project).max(1))),
        Target::RackLed(_) => Some(v),
        _ => None,
    }
}

/// The value (in the box's unit) of a rack row's mixer track box or LED at position `p` (0..1): the nearest track, or
/// the LED on from one half.
pub(crate) fn at_position(s: &State, t: Target, p: f64) -> f64 {
    let p = p.clamp(0.0, 1.0);
    match t {
        Target::RackTrack(_) => (p * f64::from(crate::mixer::max_insert(&s.session.project))).round(),
        _ => if p >= 0.5 { 1.0 } else { 0.0 },
    }
}

/// Sets a rack row's mixer track box or LED to `v`, in the box's unit (its menu's Paste value and Set...).
pub(crate) fn set_value(s: &mut State, t: Target, v: f64) {
    set(s, t, v);
}

/// The field's text for the box's current value in a mode (spec §3, §6).
pub fn text(s: &State, t: Target, norm: bool) -> String {
    let n = get(s, t).unwrap_or(0.0);
    if let Target::Latency(_, _, u) = t {
        return short(n / u.ms(s.tempo()), 4);
    }
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
        Unit::Track => crate::views::rack::insert_name(n as u16),
        Unit::Switch => if n >= 0.5 { "On" } else { "Off" }.into(),
        Unit::Plain | Unit::Latency => short(n, 4),
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
    // A latency in its unit, with or without the unit's name after it.
    if let Target::Latency(_, _, lu) = t {
        return number(strip_suffix_ci(text, lu.name())).filter(|(_, pct)| !pct).map(|(v, _)| v);
    }
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
        // A track number, with or without "Bus" (or the original's "Insert") before it; "Master" or "M" is 0. Above the
        // last bus: the last.
        Unit::Track => {
            if text.eq_ignore_ascii_case("master") || text.eq_ignore_ascii_case("m") {
                return Some(0.0);
            }
            let prefixed = |p: &str| text.len() >= p.len() && text.is_char_boundary(p.len()) && text[..p.len()].eq_ignore_ascii_case(p);
            let t = if prefixed("insert") { &text[6..] } else if prefixed("bus") { &text[3..] } else { text };
            match number(t)? {
                (_, true) => None,
                (v, false) => Some(v.round().max(0.0)),
            }
        }
        // "On" / "Off" (also "unmute" / "mute", "muted"), or a number: on from 0.5 (from 50 with "%").
        Unit::Switch => {
            let w = text.to_ascii_lowercase();
            match w.as_str() {
                "on" | "unmute" | "unmuted" => Some(1.0),
                "off" | "mute" | "muted" => Some(0.0),
                _ => number(&w).map(|(v, pct)| if (if pct { v / 100.0 } else { v }) >= 0.5 { 1.0 } else { 0.0 }),
            }
        }
        Unit::Plain | Unit::Latency => None,
    }
}

/// "Enter value...": the box at the mouse (spec §1), its field holding the value in the remembered mode.
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
