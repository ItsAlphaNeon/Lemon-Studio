// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Formula Controller's settings and editor (a recreation of FL's Fruity Formula Controller): the knobs a, b and
//! c, and the formula, a line of text kept in the saved state with the comments after it. The plugin itself
//! (`plugins/lemon-formula-controller`) is not a [`crate::SmallFx`]: it works the formula out on the song's ticks and
//! sends its output out as MIDI, so it only borrows the description, the parameters and the editor from here.
//!
// spec: plugins/lemon-formula-controller.md, plugins/fruity-formula-controller.md

use os_engine::effects::formula_ctrl::{Settings, KNOB_ONE, NEW_AFTER_KNOBS, VERSION};
use os_engine::formula::Formula;

use crate::params::{leading_number, nearest_rising};
use crate::{Card, Control, Mark, Setting, Sound, Spec};

/// The knobs' ids, in the saved state's order (ints 1 to 3, after the version).
pub const KNOBS: [&str; 3] = ["a", "b", "c"];

pub const SETTINGS: [Setting; 3] = [
    Setting { id: "a", name: "a parameter", range: (0, KNOB_ONE), start: 0, magnet: None },
    Setting { id: "b", name: "b parameter", range: (0, KNOB_ONE), start: 0, magnet: None },
    Setting { id: "c", name: "c parameter", range: (0, KNOB_ONE), start: 0, magnet: None },
];

// Editor layout (spec §2), on Lemon Peak Controller's grid: cards 6 px from the edges and 6 px apart (`Rect::span` takes
// the last pixel, so a card ending at x1 leaves x1 + 1 .. x1 + 6 clear); a knob every 52 px, 32 px from its card's
// sides.
const GAP: f32 = 6.0;
const TOP: f32 = 28.0;
const FOOT: f32 = 124.0;
const KNOB_Y: f32 = 70.0;
const KNOB_R: f32 = 18.0;
const PITCH: f32 = 52.0;
const SIDE: f32 = 32.0;
const INPUT_X0: f32 = GAP;
const INPUT_X1: f32 = INPUT_X0 + 2.0 * SIDE + 2.0 * PITCH - 1.0;
const FORMULA_X0: f32 = INPUT_X1 + 1.0 + GAP;
const FORMULA_X1: f32 = FORMULA_X0 + 300.0 - 1.0;
/// The formula's field: 12 px in from its card's sides, 24 px high, centred on the knobs' row; its status under it.
const FIELD_X0: f32 = FORMULA_X0 + 12.0;
const FIELD_X1: f32 = FORMULA_X1 - 12.0;
const FIELD_Y0: f32 = KNOB_Y - 12.0;
const FIELD_Y1: f32 = FIELD_Y0 + 24.0 - 1.0;
const WIDTH: f32 = FORMULA_X1 + 1.0 + GAP;
/// The hint well and wordmark row under the cards.
const HINT_Y: f32 = FOOT + 1.0 + GAP;
const HINT_H: f32 = 22.0;
const HEIGHT: f32 = HINT_Y + HINT_H + GAP;

const CARDS: [Card; 2] = [
    Card { x0: INPUT_X0, x1: INPUT_X1, y0: TOP, y1: FOOT, heading: "INPUT" },
    Card { x0: FORMULA_X0, x1: FORMULA_X1, y0: TOP, y1: FOOT, heading: "FORMULA" },
];

const fn knob(id: &'static str, k: f32, caption: &'static str) -> Control {
    Control::Knob { id, cx: INPUT_X0 + SIDE + k * PITCH, cy: KNOB_Y, r: KNOB_R, caption, bipolar: false }
}

const CONTROLS: [Control; 4] = [
    knob("a", 0.0, "A"),
    knob("b", 1.0, "B"),
    knob("c", 2.0, "C"),
    Control::Text { id: "formula", x0: FIELD_X0, y0: FIELD_Y0, x1: FIELD_X1, y1: FIELD_Y1, show, edit },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Formula Controller",
    word: "FORMULA CONTROLLER",
    about: "Lemon Studio's formula controller",
    settings: &SETTINGS,
    version: Some(VERSION),
    any_version: false,
    fields: &[],
    // A new instance's: no formula, the original's comments.
    tail: &NEW_AFTER_KNOBS,
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

/// The settings of a saved state, with the knobs `knobs` (in [`SETTINGS`] order) in place of the stored ones.
pub fn settings(raw: &[u8], knobs: &[i32]) -> Settings {
    let mut s = Settings::from_state(raw).unwrap_or_default();
    for (k, v) in knobs.iter().take(3).enumerate() {
        s.knobs[k] = *v;
    }
    s
}

/// The formula of a saved state and whether it compiles ("Compiled ok", or what is wrong).
fn show(raw: &[u8]) -> (String, String) {
    let formula = Settings::from_state(raw).unwrap_or_default().formula;
    let status = match Formula::compile(&formula) {
        Ok(_) => "Compiled ok".to_string(),
        Err(e) => format!("Syntax error: {e}"),
    };
    (formula, status)
}

/// The saved state with the formula `text` (trimmed, whether it compiles or not); the version and the rest stay.
fn edit(raw: &[u8], text: &str) -> Vec<u8> {
    let mut s = Settings::from_state(raw).unwrap_or_default();
    s.formula = text.trim().to_string();
    let mut out = s.to_state();
    if let Some(v) = raw.get(..4) {
        out[..4].copy_from_slice(v);
    }
    out
}

/// A knob's value as the formula sees it.
fn share(v: i32) -> f64 {
    f64::from(v) / f64::from(KNOB_ONE)
}

/// Value texts (spec §3, Lemon Studio's choice): the value the formula sees, with three decimals.
fn text(_k: usize, v: i32) -> String {
    format!("{:.3}", share(v))
}

/// A typed value: a number as the formula sees it, or a percentage.
fn parse(k: usize, s: &str) -> Option<i32> {
    let x = leading_number(s)?;
    let x = if s.trim_end().ends_with('%') { x / 100.0 } else { x };
    Some(nearest_rising(SETTINGS[k].range, x, share))
}

/// The sound passes through.
struct Through;

impl Sound for Through {
    fn set(&mut self, _v: &[i32]) {}
    fn set_sample_rate(&mut self, _rate: u32) {}
    fn reset(&mut self) {}
    fn process(&mut self, _left: &mut [f32], _right: &mut [f32]) {}
}

fn sound(_rate: u32, _v: &[i32]) -> Box<dyn Sound> {
    Box::new(Through)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{default_state, fl_from_plugin_state, plugin_state_from_fl, to_fl, values_of};

    /// The saved state is the original's: the version, the knobs, the formula and the comments, kept through the
    /// plugin's own state; editing the formula keeps the comments.
    #[test]
    fn the_state_is_the_originals() {
        assert_eq!(default_state(SPEC), Settings::default().to_state());
        let mut st = Settings { knobs: [46592, 65536, 32768], formula: String::new(), tail: vec![0, 0] }.to_state();
        // A comment: its length in UTF-16 units, the text, a 0 after it.
        st.truncate(17);
        st.extend([2, b'h', 0, b'i', 0, 0]);
        assert_eq!(values_of(SPEC, &st), [46592, 65536, 32768]);
        assert_eq!(fl_from_plugin_state(SPEC, &plugin_state_from_fl(SPEC, &st)), Some(st.clone()));
        let typed = edit(&st, " a*b+c ");
        assert_eq!(show(&typed), ("a*b+c".to_string(), "Compiled ok".to_string()));
        let s = Settings::from_state(&typed).unwrap();
        assert_eq!((s.knobs, s.comments()), ([46592, 65536, 32768], "hi".to_string()));
        assert_eq!(show(&edit(&st, "a*")).1, "Syntax error: function or operand has too few arguments");
        // An older version's state keeps its version.
        let mut old = st.clone();
        old[0] = 2;
        assert_eq!(&edit(&old, "b")[..4], &[2, 0, 0, 0]);
        // A knob edit writes its int and keeps the rest.
        let moved = to_fl(SPEC, &[0, 65536, 32768], &typed);
        assert_eq!(Settings::from_state(&moved).unwrap(), Settings { knobs: [0, 65536, 32768], ..s.clone() });
        assert_eq!(settings(&moved, &[1, 2, 3]).knobs, [1, 2, 3]);
    }

    #[test]
    fn value_texts() {
        assert_eq!(text(0, 15360), "0.234");
        assert_eq!(text(0, 65536), "1.000");
        assert_eq!(parse(0, "0.5"), Some(32768));
        assert_eq!(parse(1, "25%"), Some(16384));
    }
}
