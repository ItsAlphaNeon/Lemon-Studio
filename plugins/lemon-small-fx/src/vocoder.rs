// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Vocoder's settings and editor (a recreation of FL's Vocodex): the mixer's levels and filters, the envelope
//! follower, the bands and the inputs. The plugin itself (`plugins/lemon-vocoder`) is not a [`crate::SmallFx`]: it has
//! a sidechain input bus for the carrier, so it only borrows the description, the parameters and the editor from here.
//!
//! The saved state is the original's (header, 13 mapping curves, 27 ints, switch bytes, the carrier synth's name and
//! latched notes); the settings are 16 of its ints and the L/R switch byte, everything else is kept as stored.
//!
// spec: plugins/lemon-vocoder.md, plugins/vocodex.md

use os_engine::effects::vocoder::{self as dsp, Params, Routing, Vocoder};

use crate::params::{byte, leading_number, nearest_rising, parse_db_rising};
use crate::{Card, Control, Mark, Setting, Sound, Spec};

pub const WET: usize = 0;
pub const MOD: usize = 1;
pub const MOD_HP: usize = 2;
pub const CAR: usize = 3;
pub const CAR_LP: usize = 4;
pub const NOISE: usize = 5;
pub const NOISE_HP: usize = 6;
pub const HOLD: usize = 7;
pub const ATTACK: usize = 8;
pub const RELEASE: usize = 9;
pub const WIDTH: usize = 10;
pub const RMS: usize = 11;
pub const ORDER: usize = 12;
pub const BANDS: usize = 13;
pub const MOD_INPUT: usize = 14;
pub const CAR_INPUT: usize = 15;
pub const LR: usize = 16;

/// The highest input number the selectors offer (the original's offer the inputs the track has).
pub const INPUTS: i32 = 16;

/// The settings in the saved state's order (spec §5): their ints, then the L/R switch, the byte after the ints.
pub const SETTINGS: [Setting; 17] = [
    Setting { id: "wet", name: "Wet level", range: (0, 2000), start: 1000, magnet: Some(1000) },
    Setting { id: "mod", name: "Modulator pass-through level", range: (0, 16000), start: 12800, magnet: Some(12800) },
    Setting { id: "mod_hp", name: "Modulator pass-through high-pass", range: (0, 65536), start: 55000, magnet: None },
    Setting { id: "car", name: "Carrier pass-through level", range: (0, 16000), start: 0, magnet: Some(12800) },
    Setting { id: "car_lp", name: "Carrier pass-through low-pass", range: (0, 65536), start: 28000, magnet: None },
    Setting { id: "noise", name: "Carrier added noise level", range: (0, 12800), start: 0, magnet: None },
    Setting { id: "noise_hp", name: "Carrier added noise high-pass", range: (0, 65536), start: 55000, magnet: None },
    Setting { id: "hold", name: "Hold time", range: (0, 10000), start: 1666, magnet: None },
    Setting { id: "attack", name: "Attack time", range: (0, 10000), start: 1115, magnet: None },
    Setting { id: "release", name: "Release time", range: (0, 10000), start: 3500, magnet: None },
    Setting { id: "width", name: "Band width", range: (5, 1000), start: 349, magnet: None },
    Setting { id: "rms", name: "Hold mode", range: (0, 1), start: 0, magnet: None },
    Setting { id: "order", name: "Order", range: (1, 4), start: 2, magnet: None },
    Setting { id: "bands", name: "Band count", range: (5, 100), start: 47, magnet: None },
    Setting { id: "mod_input", name: "Modulator input", range: (0, INPUTS), start: 0, magnet: None },
    Setting { id: "car_input", name: "Carrier input", range: (-1, INPUTS), start: 1, magnet: None },
    Setting { id: "lr", name: "L/R interlaced input", range: (0, 1), start: 0, magnet: None },
];

/// The int of the original's state each setting is.
const FIELDS: [usize; 17] = [
    dsp::WET,
    dsp::MOD,
    dsp::MOD_HP,
    dsp::CAR,
    dsp::CAR_LP,
    dsp::NOISE,
    dsp::NOISE_HP,
    dsp::HOLD,
    dsp::ATTACK,
    dsp::RELEASE,
    dsp::WIDTH,
    dsp::RMS,
    dsp::ORDER,
    dsp::BANDS,
    dsp::MOD_INPUT,
    dsp::CAR_INPUT,
    byte(dsp::LR_BYTE),
];

// Editor layout (spec §2): two rows of cards on the 6 px grid (`Rect::span` takes the last pixel, so a card ending at
// x1 leaves x1 + 1 .. x1 + 6 clear); knobs of one size every 52 px, 32 px from their card's sides; a radio list column
// 40 px right of a card's last knob.
const GAP: f32 = 6.0;
const KNOB_R: f32 = 18.0;
const PITCH: f32 = 52.0;
const SIDE: f32 = 32.0;
const LIST: f32 = 76.0;
/// Row 1: cards from y 28 to 124; row 2 under it with room for its headings.
const TOP1: f32 = 28.0;
const FOOT1: f32 = 124.0;
const TOP2: f32 = FOOT1 + 1.0 + GAP + 22.0;
const FOOT2: f32 = TOP2 + (FOOT1 - TOP1);
const KNOB_DY: f32 = 42.0;

/// The right edge of a card from `x0` holding `knobs` knobs.
const fn knobs_x1(x0: f32, knobs: f32) -> f32 {
    x0 + 2.0 * SIDE + (knobs - 1.0) * PITCH - 1.0
}
/// The next card's left edge.
const fn next(x1: f32) -> f32 {
    x1 + 1.0 + GAP
}

const MIX_X0: f32 = GAP;
const MIX_X1: f32 = knobs_x1(MIX_X0, 5.0);
const NOISE_X0: f32 = next(MIX_X1);
const NOISE_X1: f32 = knobs_x1(NOISE_X0, 2.0);
const IN_X0: f32 = next(NOISE_X1);
const IN_X1: f32 = knobs_x1(IN_X0, 2.0);
const LR_X0: f32 = next(IN_X1);
const LR_X1: f32 = LR_X0 + 62.0 - 1.0;
const WIDTH_PX: f32 = next(LR_X1);

const ENV_X0: f32 = GAP;
const ENV_LIST_X: f32 = ENV_X0 + SIDE + 2.0 * PITCH + 40.0;
const ENV_X1: f32 = ENV_LIST_X + LIST - 1.0;
const BAND_X0: f32 = next(ENV_X1);
const BAND_LIST_X: f32 = BAND_X0 + SIDE + PITCH + 40.0;
const BAND_X1: f32 = BAND_LIST_X + LIST - 1.0;

/// The hint well and wordmark row under the cards.
const HINT_Y: f32 = FOOT2 + 1.0 + GAP;
const HINT_H: f32 = 22.0;
const HEIGHT: f32 = HINT_Y + HINT_H + GAP;
/// Radio lists: rows 16 px apart, centred in their card.
const MID2: f32 = (TOP2 + FOOT2 + 1.0) / 2.0;
const MID1: f32 = (TOP1 + FOOT1 + 1.0) / 2.0;

const CARDS: [Card; 6] = [
    Card { x0: MIX_X0, x1: MIX_X1, y0: TOP1, y1: FOOT1, heading: "MIXER" },
    Card { x0: NOISE_X0, x1: NOISE_X1, y0: TOP1, y1: FOOT1, heading: "NOISE" },
    Card { x0: IN_X0, x1: IN_X1, y0: TOP1, y1: FOOT1, heading: "INPUTS" },
    Card { x0: LR_X0, x1: LR_X1, y0: TOP1, y1: FOOT1, heading: "L/R" },
    Card { x0: ENV_X0, x1: ENV_X1, y0: TOP2, y1: FOOT2, heading: "ENVELOPE" },
    Card { x0: BAND_X0, x1: BAND_X1, y0: TOP2, y1: FOOT2, heading: "BANDS" },
];

const fn knob(id: &'static str, x0: f32, k: f32, top: f32, caption: &'static str) -> Control {
    Control::Knob { id, cx: x0 + SIDE + k * PITCH, cy: top + KNOB_DY, r: KNOB_R, caption, bipolar: false }
}

const CONTROLS: [Control; 17] = [
    knob("wet", MIX_X0, 0.0, TOP1, "WET"),
    knob("mod", MIX_X0, 1.0, TOP1, "MOD"),
    knob("mod_hp", MIX_X0, 2.0, TOP1, "MOD HP"),
    knob("car", MIX_X0, 3.0, TOP1, "CAR"),
    knob("car_lp", MIX_X0, 4.0, TOP1, "CAR LP"),
    knob("noise", NOISE_X0, 0.0, TOP1, "LEVEL"),
    knob("noise_hp", NOISE_X0, 1.0, TOP1, "HP"),
    knob("mod_input", IN_X0, 0.0, TOP1, "MOD"),
    knob("car_input", IN_X0, 1.0, TOP1, "CAR"),
    Control::Choice { id: "lr", x: LR_X0 + 13.0, top: MID1 - 16.0, names: &["OFF", "ON"] },
    knob("hold", ENV_X0, 0.0, TOP2, "HOLD"),
    knob("attack", ENV_X0, 1.0, TOP2, "ATTACK"),
    knob("release", ENV_X0, 2.0, TOP2, "RELEASE"),
    Control::Choice { id: "rms", x: ENV_LIST_X, top: MID2 - 16.0, names: &["PEAK", "RMS"] },
    knob("bands", BAND_X0, 0.0, TOP2, "COUNT"),
    knob("width", BAND_X0, 1.0, TOP2, "WIDTH"),
    Control::Choice { id: "order", x: BAND_LIST_X, top: MID2 - 32.0, names: &["ORDER 1", "ORDER 2", "ORDER 3", "ORDER 4"] },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Vocoder",
    word: "VOCODER",
    about: "Lemon Studio's vocoder",
    settings: &SETTINGS,
    version: None,
    any_version: false,
    fields: &FIELDS,
    tail: &[],
    blank: Some(dsp::default_state),
    locate: Some(dsp::settings_at),
    size: (WIDTH_PX as u32, HEIGHT as u32),
    cards: &CARDS,
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: WIDTH_PX - GAP, y: HINT_Y + HINT_H / 2.0, anchor: 1.0, stacked: false },
    hint: Some(os_ui::Rect::new(GAP, HINT_Y, WIDTH_PX - 2.0 * GAP, HINT_H)),
    labels: &[],
    text,
    parse,
    sound,
    rings: true,
};

/// What the vocoder plays: the saved state (`raw`: its curves and the ints no setting covers) with the settings `v` (in
/// [`SETTINGS`] order) over it.
pub fn settings(raw: &[u8], v: &[i32]) -> dsp::Settings {
    let mut s = dsp::Settings::from_state(raw).unwrap_or_default();
    for (k, &f) in FIELDS.iter().enumerate().take(LR) {
        if let Some(&x) = v.get(k) {
            s.v[f] = x;
        }
    }
    if let Some(&lr) = v.get(LR) {
        s.lr = lr == 1;
    }
    s
}

fn hz_text(v: i32, off: bool) -> String {
    if off { "Off".into() } else { format!("{:.0}Hz", dsp::cutoff_hz(v)) }
}

fn ms(k: usize, v: i32) -> f64 {
    match k {
        HOLD => dsp::time_ms(v, 250.0),
        ATTACK => dsp::time_ms(v, 1000.0),
        _ => dsp::time_ms(v, 2000.0),
    }
}

fn level_db(v: i32) -> String {
    crate::params::db_text_fine(os_dsp::mixer::send_gain(v))
}

/// Value texts (spec §3): the original's where it was read (levels in dB, frequencies in whole hertz with Off at the
/// knob's end, times in ms, the noise and the band width in percent), Lemon Studio's for the inputs.
fn text(k: usize, v: i32) -> String {
    match k {
        WET => crate::params::db_text_fine(dsp::wet(v) as f32),
        MOD | CAR => level_db(v),
        MOD_HP | NOISE_HP => hz_text(v, v <= 0),
        CAR_LP => hz_text(v, v >= 65536),
        NOISE => format!("{}%", (dsp::noise(v) * 100.0).round()),
        HOLD | ATTACK | RELEASE => format!("{:.2}ms", ms(k, v)),
        WIDTH => format!("{}%", (dsp::width(v) * 100.0).round()),
        RMS => if v == 1 { "RMS" } else { "Peak" }.into(),
        ORDER | BANDS => v.to_string(),
        MOD_INPUT | CAR_INPUT => match v {
            v if v < 0 => "None".into(),
            0 => "Own track".into(),
            v => format!("Input {v}"),
        },
        _ => if v == 1 { "On" } else { "Off" }.into(),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    let hz = |range: (i32, i32)| {
        let x = leading_number(&low)?;
        let x = if low.contains('k') { x * 1000.0 } else { x };
        Some(nearest_rising(range, x, dsp::cutoff_hz))
    };
    match k {
        WET => parse_db_rising(range, s, |v| dsp::wet(v) as f32),
        MOD | CAR => parse_db_rising(range, s, os_dsp::mixer::send_gain),
        MOD_HP | NOISE_HP if low == "off" => Some(0),
        MOD_HP | NOISE_HP => hz((1, 65536)),
        CAR_LP if low == "off" => Some(65536),
        CAR_LP => hz((0, 65535)),
        NOISE => Some(nearest_rising(range, leading_number(&low)?, |v| dsp::noise(v) * 100.0)),
        HOLD | ATTACK | RELEASE => Some(nearest_rising(range, leading_number(&low)?, |v| ms(k, v))),
        WIDTH => Some(nearest_rising(range, leading_number(&low)?, |v| dsp::width(v) * 100.0)),
        RMS => match low.as_str() {
            "rms" | "1" => Some(1),
            "peak" | "0" => Some(0),
            _ => None,
        },
        ORDER | BANDS => Some((leading_number(&low)?.round() as i32).clamp(range.0, range.1)),
        MOD_INPUT | CAR_INPUT => {
            if low.starts_with("none") || low == "---" {
                (range.0 < 0).then_some(-1)
            } else if low.starts_with("own") || low == "0" {
                Some(0)
            } else {
                let n = low.trim_start_matches("input").trim();
                Some((n.parse::<f64>().ok()?.round() as i32).clamp(range.0, range.1))
            }
        }
        _ => match low.as_str() {
            "on" | "1" => Some(1),
            "off" | "0" => Some(0),
            _ => None,
        },
    }
}

/// The sound alone (no sidechain: the plugin's own processing picks its inputs): the slot's audio as both the
/// modulator and the carrier, or as L/R interlaced.
struct Alone {
    v: Vocoder,
    lr: bool,
    m: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
}

impl Sound for Alone {
    fn set(&mut self, v: &[i32]) {
        let s = settings(&dsp::default_state(), v);
        self.lr = s.lr;
        self.v.set(s.params());
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.v.set_sample_rate(rate);
    }
    fn reset(&mut self) {
        self.v.reset();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let r = Routing { modulator: 0, carrier: 0, lr: self.lr };
        let [ml, mr] = &mut self.m;
        ml.clear();
        ml.extend_from_slice(left);
        mr.clear();
        mr.extend_from_slice(right);
        let [ol, or] = &mut self.out;
        ol.resize(left.len(), 0.0);
        or.resize(left.len(), 0.0);
        let silent = vec![0.0; left.len()];
        let (m, c) = r.pick((ml, mr), (&silent, &silent), &silent);
        self.v.process(m, c, (ol, or));
        left.copy_from_slice(ol);
        right.copy_from_slice(or);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    let mut s = Alone { v: Vocoder::new(rate, Params::default()), lr: false, m: Default::default(), out: Default::default() };
    s.set(v);
    Box::new(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{default_state, fl_from_plugin_state, plugin_state_from_fl, to_fl, values_of};

    /// The saved state is the original's whole state: a new instance's is FL 2026's default (with Lemon Studio's
    /// inputs), the settings read from and write to their ints, and everything else is kept.
    #[test]
    fn the_state_is_the_originals() {
        let b = default_state(SPEC);
        assert_eq!(b, dsp::default_state());
        assert_eq!(values_of(SPEC, &b), SPEC.start());
        assert_eq!(settings(&b, &SPEC.start()), dsp::Settings::default());
        // A state with a longer curve: the settings are found after it, and written back there.
        let mut fl = b[..4].to_vec();
        fl.extend([3i32, 3, 3].iter().flat_map(|v| v.to_le_bytes()));
        for k in 0..3 {
            fl.extend(f64::to_le_bytes(if k == 0 { 0.0 } else { 0.5 }));
            fl.extend(0.25f64.to_le_bytes());
            fl.extend([0; 8]);
        }
        fl.extend([0; 4]);
        fl.extend([0xff; 16]);
        fl.extend(&b[84..]);
        let mut vals = values_of(SPEC, &fl);
        assert_eq!(vals, SPEC.start());
        (vals[BANDS], vals[CAR_INPUT], vals[LR]) = (20, -1, 1);
        let out = to_fl(SPEC, &vals, &fl);
        assert_eq!(out.len(), fl.len());
        let s = dsp::Settings::from_state(&out).unwrap();
        assert_eq!((s.v[dsp::BANDS], s.v[dsp::CAR_INPUT], s.lr), (20, -1, true));
        assert_eq!(&out[..1068], &fl[..1068], "the curves are kept");
        assert_eq!(s.v[dsp::SG], 150, "an int no setting covers is kept");
        assert_eq!(fl_from_plugin_state(SPEC, &plugin_state_from_fl(SPEC, &out)), Some(out.clone()));
        // A state that does not parse starts from the default.
        assert_eq!(crate::params::normalized(SPEC, vec![1, 2, 3]), b);
    }

    #[test]
    fn value_texts() {
        assert_eq!(text(WET, 1000), "0.0dB");
        assert_eq!(text(WET, 0), "-INFdB");
        assert_eq!(text(MOD, 12800), "0.0dB");
        assert_eq!(text(MOD, 16000), "5.6dB");
        assert_eq!(text(MOD_HP, 55000), "5785Hz");
        assert_eq!(text(MOD_HP, 0), "Off");
        assert_eq!(text(CAR_LP, 28000), "503Hz");
        assert_eq!(text(CAR_LP, 65536), "Off");
        assert_eq!(text(NOISE, 8000), "63%");
        assert_eq!(text(HOLD, 1666), "0.91ms");
        assert_eq!(text(ATTACK, 1115), "2.00ms");
        assert_eq!(text(WIDTH, 349), "75%");
        assert_eq!(text(CAR_INPUT, -1), "None");
        assert_eq!(text(MOD_INPUT, 0), "Own track");
        assert_eq!(text(CAR_INPUT, 2), "Input 2");
        assert_eq!(parse(CAR_INPUT, "---"), Some(-1));
        assert_eq!(parse(CAR_INPUT, "input 3"), Some(3));
        assert_eq!(parse(MOD_INPUT, "none"), None);
        assert_eq!(parse(CAR_LP, "off"), Some(65536));
        assert_eq!(text(CAR_LP, parse(CAR_LP, "1k").unwrap()), "1000Hz");
        assert_eq!(parse(RMS, "rms"), Some(1));
    }
}
