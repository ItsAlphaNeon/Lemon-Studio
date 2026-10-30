// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Peak Controller's settings and editor (a recreation of FL's Fruity Peak Controller): the peak follower's four
//! knobs, the LFO's five and its shape, the Ramp and Mute switches. The plugin itself (`plugins/lemon-peak-controller`)
//! is not a [`crate::SmallFx`]: it works on the song's ticks and sends its outputs out as MIDI, so it only borrows the
//! description, the parameters and the editor from here.
//!
// spec: plugins/lemon-peak-controller.md, plugins/fruity-peak-controller.md

use os_engine::effects::peak_ctrl::{self as dsp, Settings};

use crate::params::{byte, leading_number, nearest, nearest_rising};
use crate::{Card, Control, Mark, Setting, Sound, Spec};

pub const BASE: usize = 0;
pub const AMOUNT: usize = 1;
pub const TENSION: usize = 2;
pub const DECAY: usize = 3;
pub const LFO_BASE: usize = 4;
pub const LFO_AMOUNT: usize = 5;
pub const LFO_TENSION: usize = 6;
pub const SPEED: usize = 7;
pub const SHAPE: usize = 8;
pub const PHASE: usize = 9;
pub const MUTE: usize = 10;
pub const RAMP: usize = 11;

/// The settings in the saved state's order: ten ints, then Mute and Ramp in the two bytes after them (spec §5).
pub const SETTINGS: [Setting; 12] = [
    Setting { id: "base", name: "Peak base", range: (0, 65536), start: 0, magnet: None },
    Setting { id: "amount", name: "Peak volume", range: (-256, 256), start: 179, magnet: Some(0) },
    Setting { id: "tension", name: "Peak tension", range: (-128, 128), start: 0, magnet: Some(0) },
    Setting { id: "decay", name: "Peak decay", range: (5, 128), start: 66, magnet: None },
    Setting { id: "lfo_base", name: "LFO base", range: (0, 65536), start: 0, magnet: None },
    Setting { id: "lfo_amount", name: "LFO volume", range: (-256, 256), start: 0, magnet: Some(0) },
    Setting { id: "lfo_tension", name: "LFO tension", range: (-128, 128), start: 0, magnet: Some(0) },
    Setting { id: "speed", name: "LFO speed", range: (200, 65536), start: 32950, magnet: None },
    Setting { id: "shape", name: "LFO shape", range: (0, 4), start: 0, magnet: None },
    Setting { id: "phase", name: "LFO phase", range: (0, 65536), start: 0, magnet: None },
    Setting { id: "mute", name: "Mute", range: (0, 1), start: 0, magnet: None },
    Setting { id: "ramp", name: "Ramp controllers", range: (0, 1), start: 0, magnet: None },
];

// Editor layout (spec §2): cards 6 px from the edges and 6 px apart (`Rect::span` takes the last pixel, so a card ending
// at x1 leaves x1 + 1 .. x1 + 6 clear); a knob every 52 px, 32 px from its card's sides.
const GAP: f32 = 6.0;
const TOP: f32 = 28.0;
const FOOT: f32 = 124.0;
const KNOB_Y: f32 = 70.0;
const KNOB_R: f32 = 18.0;
const PITCH: f32 = 52.0;
const SIDE: f32 = 32.0;
const PEAK_X0: f32 = GAP;
const PEAK_X1: f32 = PEAK_X0 + 2.0 * SIDE + 3.0 * PITCH - 1.0;
const LFO_X0: f32 = PEAK_X1 + 1.0 + GAP;
/// The shape list: a column right of the LFO's knobs.
const SHAPE_X: f32 = LFO_X0 + SIDE + 4.0 * PITCH + 40.0;
const LFO_X1: f32 = SHAPE_X + 76.0;
const SWITCH_W: f32 = 62.0;
const RAMP_X0: f32 = LFO_X1 + 1.0 + GAP;
const RAMP_X1: f32 = RAMP_X0 + SWITCH_W - 1.0;
const MUTE_X0: f32 = RAMP_X1 + 1.0 + GAP;
const MUTE_X1: f32 = MUTE_X0 + SWITCH_W - 1.0;
const WIDTH: f32 = MUTE_X1 + 1.0 + GAP;
/// The hint well and wordmark row under the cards.
const HINT_Y: f32 = FOOT + 1.0 + GAP;
const HINT_H: f32 = 22.0;
const HEIGHT: f32 = HINT_Y + HINT_H + GAP;
/// Radio lists: rows 16 px apart, centred in their card.
const MID: f32 = (TOP + FOOT + 1.0) / 2.0;

const CARDS: [Card; 4] = [
    Card { x0: PEAK_X0, x1: PEAK_X1, y0: TOP, y1: FOOT, heading: "PEAK" },
    Card { x0: LFO_X0, x1: LFO_X1, y0: TOP, y1: FOOT, heading: "LFO" },
    Card { x0: RAMP_X0, x1: RAMP_X1, y0: TOP, y1: FOOT, heading: "RAMP" },
    Card { x0: MUTE_X0, x1: MUTE_X1, y0: TOP, y1: FOOT, heading: "MUTE" },
];

const fn knob(id: &'static str, x0: f32, k: f32, caption: &'static str, bipolar: bool) -> Control {
    Control::Knob { id, cx: x0 + SIDE + k * PITCH, cy: KNOB_Y, r: KNOB_R, caption, bipolar }
}

const CONTROLS: [Control; 12] = [
    knob("base", PEAK_X0, 0.0, "BASE", false),
    knob("amount", PEAK_X0, 1.0, "VOLUME", true),
    knob("tension", PEAK_X0, 2.0, "TENSION", true),
    knob("decay", PEAK_X0, 3.0, "DECAY", false),
    knob("lfo_base", LFO_X0, 0.0, "BASE", false),
    knob("lfo_amount", LFO_X0, 1.0, "VOLUME", true),
    knob("lfo_tension", LFO_X0, 2.0, "TENSION", true),
    knob("speed", LFO_X0, 3.0, "SPEED", false),
    knob("phase", LFO_X0, 4.0, "PHASE", false),
    Control::Choice { id: "shape", x: SHAPE_X, top: MID - 40.0, names: &["SINE", "TRIANGLE", "SQUARE", "SAW", "RANDOM"] },
    Control::Choice { id: "ramp", x: RAMP_X0 + 13.0, top: MID - 16.0, names: &["OFF", "ON"] },
    Control::Choice { id: "mute", x: MUTE_X0 + 13.0, top: MID - 16.0, names: &["OFF", "ON"] },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Peak Controller",
    word: "PEAK CONTROLLER",
    about: "Lemon Studio's peak controller",
    settings: &SETTINGS,
    version: None,
    any_version: false,
    fields: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, byte(0), byte(1)],
    tail: &[0, 0],
    size: (WIDTH as u32, HEIGHT as u32),
    cards: &CARDS,
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: WIDTH - GAP, y: HINT_Y + HINT_H / 2.0, anchor: 1.0, stacked: false },
    hint: Some(os_ui::Rect::new(GAP, HINT_Y, WIDTH - 2.0 * GAP, HINT_H)),
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

/// The settings of the stored values `v` (in [`SETTINGS`] order).
pub fn settings(v: &[i32]) -> Settings {
    let at = |k: usize| v.get(k).copied().unwrap_or(SETTINGS[k].start);
    Settings {
        base: at(BASE),
        amount: at(AMOUNT),
        tension: at(TENSION),
        decay: at(DECAY),
        lfo_base: at(LFO_BASE),
        lfo_amount: at(LFO_AMOUNT),
        lfo_tension: at(LFO_TENSION),
        speed: at(SPEED),
        shape: at(SHAPE),
        phase: at(PHASE),
        mute: at(MUTE) != 0,
        ramp: at(RAMP) != 0,
    }
    .clamped()
}

/// A share of 65536 as a percentage with one decimal.
fn share(v: i32) -> f64 {
    f64::from(v) / 65536.0 * 100.0
}

/// The LFO's period in beats (whole ticks of 1/96 beat).
fn beats(speed: i32) -> f64 {
    dsp::lfo_ticks(speed) / 96.0
}

/// A whole percentage (never "-0%").
fn percent(x: f64) -> String {
    format!("{:.0}%", if x.abs() < 0.5 { 0.0 } else { x })
}

/// Value texts (spec §3, Lemon Studio's choice): levels and the phase as shares, the volumes as the factor they give
/// the follower or the LFO, tensions and the decay as shares of their ends, the speed as the LFO's period.
fn text(k: usize, v: i32) -> String {
    match k {
        BASE | LFO_BASE | PHASE => format!("{:.1}%", share(v)),
        AMOUNT => percent(dsp::amount_factor(v) * 100.0),
        LFO_AMOUNT => percent(dsp::lfo_factor(v) * 100.0),
        TENSION | LFO_TENSION | DECAY => format!("{:.0}%", f64::from(v) / 128.0 * 100.0),
        SPEED => match beats(v) {
            b if b < 1.0 => format!("{b:.3} beats"),
            b => format!("{b:.2} beats"),
        },
        SHAPE => dsp::SHAPES.get(v as usize).copied().unwrap_or("Sine").into(),
        _ => if v == 1 { "On" } else { "Off" }.into(),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    match k {
        BASE | LFO_BASE | PHASE => Some(nearest_rising(range, leading_number(&low)?, share)),
        AMOUNT => Some(nearest_rising(range, leading_number(&low)?, |v| dsp::amount_factor(v) * 100.0)),
        LFO_AMOUNT => Some(nearest_rising(range, leading_number(&low)?, |v| dsp::lfo_factor(v) * 100.0)),
        TENSION | LFO_TENSION | DECAY => nearest(range, leading_number(&low)?, |v| Some(f64::from(v) / 128.0 * 100.0)),
        SPEED => Some(nearest_rising(range, leading_number(&low)?, beats)),
        SHAPE => dsp::SHAPES.iter().position(|n| n.to_ascii_lowercase().starts_with(&low) && !low.is_empty()).map(|i| i as i32),
        _ => match low.as_str() {
            "on" | "1" => Some(1),
            "off" | "0" => Some(0),
            _ => None,
        },
    }
}

/// The sound alone (the plugin's own processing runs the follower on the song's ticks): through, or silence when
/// muted.
struct Through(bool);

impl Sound for Through {
    fn set(&mut self, v: &[i32]) {
        self.0 = v.get(MUTE).is_some_and(|&m| m != 0);
    }
    fn set_sample_rate(&mut self, _rate: u32) {}
    fn reset(&mut self) {}
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.0 {
            left.fill(0.0);
            right.fill(0.0);
        }
    }
}

fn sound(_rate: u32, v: &[i32]) -> Box<dyn Sound> {
    let mut s = Through(false);
    s.set(v);
    Box::new(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{default_state, fl_from_plugin_state, plugin_state_from_fl, values_of};

    /// The saved state is the original's 42 bytes: ten ints, Mute and Ramp in the last two; a new instance holds the
    /// original's Default preset.
    #[test]
    fn the_state_is_the_originals() {
        let b = default_state(SPEC);
        assert_eq!(b, Settings::default().to_state());
        let s = Settings { base: 12800, amount: -256, tension: 128, decay: 30, lfo_base: 12800, lfo_amount: 256, lfo_tension: -128, speed: 13000, shape: 3, phase: 12800, mute: true, ramp: true };
        let fl = s.to_state();
        let v = values_of(SPEC, &fl);
        assert_eq!(settings(&v), s);
        assert_eq!(fl_from_plugin_state(SPEC, &plugin_state_from_fl(SPEC, &fl)), Some(fl.clone()));
        // Ramp alone on.
        let mut only_ramp = Settings::default().to_state();
        only_ramp[41] = 1;
        let v = values_of(SPEC, &only_ramp);
        assert_eq!((v[MUTE], v[RAMP]), (0, 1));
    }

    #[test]
    fn value_texts() {
        assert_eq!(text(AMOUNT, 179), "100%");
        assert_eq!(text(AMOUNT, -256), "-200%");
        assert_eq!(text(LFO_AMOUNT, 256), "100%");
        assert_eq!(text(BASE, 32768), "50.0%");
        assert_eq!(text(SPEED, 20643), "1.00 beats");
        assert_eq!(text(SPEED, 2760), "0.042 beats");
        assert_eq!(text(SHAPE, 4), "Random");
        assert_eq!(parse(SHAPE, "tri"), Some(1));
        assert_eq!(parse(AMOUNT, "100%"), Some(179));
        assert_eq!(parse(SPEED, "1 beat"), Some(nearest_rising((200, 65536), 1.0, beats)));
        assert_eq!(text(SPEED, parse(SPEED, "4 beats").unwrap()), "4.00 beats");
    }
}
