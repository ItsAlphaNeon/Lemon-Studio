// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Stereo Shaper: Lemon Studio's stereo shaper (a recreation of FL's Fruity Stereo Shaper) as a plugin. The sound
//! is `os_engine::effects::stereo_shaper`, started with its gains at the settings; the editor has the four levels in
//! two cards (what goes to the left output, what goes to the right) and the delay, the phase and their position.
//!
//! The difference output (the input minus the output, to another track) is the host's mixer's: a plugin has no tracks
//! to send it to. It has no parameter; a slot's stored value is kept in the saved state as it is.
//!
// spec: plugins/lemon-stereo-shaper.md, plugins/fruity-stereo-shaper.md

use nih_plug::prelude::*;

use os_engine::effects::stereo_shaper::{self as dsp, Settings, StereoShaper};
use os_theme::design::lemon as brand;
use os_ui::{Color, Rect};

use crate::params::{leading_number, nearest_rising};
use crate::{Card, Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

/// The engine's settings the plugin has, in order: all but the difference output.
pub const FIELDS: [usize; 7] = [
    dsp::RIGHT_INTO_LEFT,
    dsp::LEFT,
    dsp::RIGHT,
    dsp::LEFT_INTO_RIGHT,
    dsp::DELAY,
    dsp::PHASE,
    dsp::POSITION,
];

const fn setting(id: &'static str, name: &'static str, k: usize, magnet: Option<i32>) -> Setting {
    Setting { id, name, range: dsp::RANGES[k], start: dsp::DEFAULTS[k], magnet }
}

/// The settings in the original's order, the difference output left out (spec §5).
pub const SETTINGS: [Setting; 7] = [
    setting("rl", "Right into left", dsp::RIGHT_INTO_LEFT, Some(0)),
    setting("left", "Left", dsp::LEFT, Some(dsp::UNITY)),
    setting("right", "Right", dsp::RIGHT, Some(dsp::UNITY)),
    setting("lr", "Left into right", dsp::LEFT_INTO_RIGHT, Some(0)),
    setting("delay", "Delay", dsp::DELAY, Some(0)),
    setting("phase", "Phase", dsp::PHASE, Some(0)),
    setting("position", "Effects position", dsp::POSITION, None),
];

/// Setting indices.
const DELAY: usize = 4;
const PHASE: usize = 5;
const POSITION: usize = 6;

/// Editor layout (spec §2), as the original's: the four level faders of the matrix (what reaches the left output, then
/// the right), and the effects: the position, the delay and the phase.
const CARDS: [Card; 2] = [
    Card { x0: 8.0, x1: 216.0, y0: 28.0, y1: 306.0, heading: "MIXER MATRIX" },
    Card { x0: 222.0, x1: 352.0, y0: 28.0, y1: 306.0, heading: "EFFECTS" },
];

/// A fader carrying the right input (the left's are lemon).
const FROM_RIGHT: Color = Color::rgb(0xcfcfcf);
/// -100 %, 0 and 100 %.
const MARKS: &[f32] = &[0.25, 0.5, 0.75];

const fn level(id: &'static str, x: f32, right: bool) -> Control {
    let (label, col) = if right { ("R", FROM_RIGHT) } else { ("L", brand::MAIN) };
    Control::Fader { id, x, top: 66.0, bot: 270.0, label, col, bipolar: true, marks: MARKS }
}

const CONTROLS: [Control; 7] = [
    level("rl", 50.0, true),
    level("left", 88.0, false),
    level("right", 136.0, true),
    level("lr", 174.0, false),
    Control::Choice { id: "position", x: 260.0, top: 42.0, names: &["OFF", "PRE", "POST"] },
    Control::Knob { id: "delay", cx: 287.0, cy: 140.0, r: 24.0, caption: "DELAY", bipolar: true },
    Control::Knob { id: "phase", cx: 287.0, cy: 226.0, r: 24.0, caption: "PHASE", bipolar: true },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Stereo Shaper",
    word: "STEREO SHAPER",
    about: "Lemon Studio's stereo shaper",
    settings: &SETTINGS,
    version: Some(dsp::VERSION),
    any_version: true,
    fields: &FIELDS,
    tail: &[],
    size: (360, 372),
    cards: &CARDS,
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: 352.0, y: 353.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 314.0, 344.0, 22.0)),
    labels: &[(69.0, 286.0, "LEFT OUT"), (155.0, 286.0, "RIGHT OUT")],
    text,
    parse,
    sound,
    rings: true,
};

/// A level as a percentage (12800 is 100 %), to one decimal.
fn percent(v: i32) -> f64 {
    let p = (f64::from(v) / 128.0 * 10.0).round() / 10.0;
    if p == 0.0 { 0.0 } else { p }
}

/// The delay in ms: `(1001^(|k| / 4096) - 1) / 2` (the sound rounds it to samples).
pub fn delay_ms(k: i32) -> f64 {
    (1001f64.powf(f64::from(k.unsigned_abs()) / 4096.0) - 1.0) / 2.0
}

/// The side a delay or phase setting acts on.
fn side(v: i32) -> &'static str {
    if v < 0 { "left" } else { "right" }
}

/// Value texts (spec §3, Lemon Studio's choice).
fn text(k: usize, v: i32) -> String {
    match k {
        DELAY => match v {
            0 => "Off".into(),
            // A little better than the knob's step: 0.1 us below 1 ms, 1 us below 10 ms, 10 us above.
            v => match delay_ms(v) {
                ms if ms < 1.0 => format!("{ms:.4}ms {}", side(v)),
                ms if ms < 10.0 => format!("{ms:.3}ms {}", side(v)),
                ms => format!("{ms:.2}ms {}", side(v)),
            },
        },
        PHASE => match v {
            0 => "Off".into(),
            v => format!("{} {}", crate::chorus::hz_text(f64::from(dsp::phase_hz(v))), side(v)),
        },
        POSITION => match v {
            v if v < 0 => "Off",
            0 => "Pre",
            _ => "Post",
        }
        .into(),
        _ => format!("{:.1}%", percent(v)),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    // A delay or phase on the left is negative: named, or typed with a minus sign.
    let left = |x: f64| low.contains("left") || x < 0.0;
    match k {
        DELAY | PHASE if low.starts_with("off") => Some(0),
        DELAY => {
            let x = leading_number(&low)?;
            let v = nearest_rising((1, range.1), x.abs(), delay_ms);
            Some(if left(x) { -v } else { v })
        }
        PHASE => {
            let x = leading_number(&low)?;
            let hz = if low.contains('k') { x.abs() * 1000.0 } else { x.abs() };
            // The frequency falls as the setting moves away from 0.
            let v = nearest_rising((1, range.1), -hz, |v| -f64::from(dsp::phase_hz(v)));
            Some(if left(x) { -v } else { v })
        }
        POSITION => ["off", "pre", "post"].iter().position(|n| low.starts_with(n)).map(|i| i as i32 - 1),
        _ => Some(((leading_number(&low)? * 128.0).round() as i32).clamp(range.0, range.1)),
    }
}

/// The engine's settings for the plugin's (the difference output off).
fn settings(v: &[i32]) -> Settings {
    let mut s = [0; dsp::PARAMS];
    for (k, &i) in FIELDS.iter().enumerate() {
        s[i] = v[k];
    }
    Settings { v: s }
}

struct Shaper(StereoShaper);

impl Sound for Shaper {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    /// The delay and the all-pass for the new rate (the line empty).
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    /// The line and the all-pass memory empty; the gains stand at the settings.
    fn reset(&mut self) {
        self.0.clear();
        self.0.flush();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

/// The plugin starts with its gains at the settings (as a slot that is on when the song starts).
fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    let mut e = StereoShaper::with_settings(rate, &settings(v));
    e.flush();
    Box::new(Shaper(e))
}

pub struct StereoShaperKind;

impl Kind for StereoShaperKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-stereo-shaper";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's stereo shaper";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Utility, ClapFeature::Mixing, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioStShp";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Spatial];
}

pub type LemonStereoShaper = SmallFx<StereoShaperKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(1, 12800), "100.0%");
        assert_eq!(text(0, -8076), "-63.1%");
        assert_eq!(text(2, -25600), "-200.0%");
        assert_eq!(text(3, -1), "0.0%");
        assert_eq!(text(DELAY, 0), "Off");
        assert_eq!(text(DELAY, 1), "0.0008ms right");
        assert_eq!(text(DELAY, -653), "1.004ms left");
        assert_eq!(text(DELAY, 2416), "28.93ms right");
        assert_eq!(text(DELAY, -4096), "500.00ms left");
        assert_eq!(text(PHASE, 0), "Off");
        assert_eq!(text(PHASE, 1), "21.95kHz right");
        assert_eq!(text(PHASE, -2496), "128.1Hz left");
        assert_eq!(text(PHASE, 4096), "10.0Hz right");
        assert_eq!(text(POSITION, -1), "Off");
        assert_eq!(text(POSITION, 1), "Post");
        assert_eq!(parse(1, "100%"), Some(12800));
        assert_eq!(parse(0, "-63.1"), Some(-8077));
        assert_eq!(parse(0, "-300"), Some(-25600));
        assert_eq!(parse(DELAY, "500ms left"), Some(-4096));
        assert_eq!(parse(DELAY, "-28.93"), Some(-2416));
        assert_eq!(parse(DELAY, "off"), Some(0));
        assert_eq!(parse(PHASE, "128.1Hz left"), Some(-2496));
        assert_eq!(parse(PHASE, "21.95 kHz"), Some(1));
        assert_eq!(parse(POSITION, "pre"), Some(0));
    }

    #[test]
    fn the_plugin_starts_at_its_settings_and_leaves_the_difference_output_alone() {
        // FL's "LR to MS": the matrix at its settings from the first sample.
        let v = [8076, 8076, -8076, 8076, 0, 0, 1];
        let mut s = sound(44100, &v);
        let (mut l, mut r) = (vec![0.5f32; 4], vec![0.25f32; 4]);
        s.process(&mut l, &mut r);
        let h = dsp::level_gain(8076);
        assert_eq!((l[0], r[0]), (h * 0.5 + h * 0.25, -h * 0.25 + h * 0.5));
        assert_eq!(settings(&v).v, [8076, 8076, -8076, 8076, 0, 0, 0, 1]);
        // A delay of 100 on the right, before the matrix: 4 samples at 44100 Hz, 9 at 96000 Hz after a new rate.
        let mut s = sound(44100, &[0, 12800, 12800, 0, 100, 0, 0]);
        let (mut l, mut r) = (vec![0.0f32; 16], vec![0.0f32; 16]);
        r[0] = 1.0;
        s.process(&mut l, &mut r);
        assert_eq!(r.iter().position(|&x| x != 0.0), Some(4));
        s.set_sample_rate(96000);
        let (mut l, mut r) = (vec![0.0f32; 16], vec![0.0f32; 16]);
        r[0] = 1.0;
        s.process(&mut l, &mut r);
        assert_eq!(r.iter().position(|&x| x != 0.0), Some(dsp::delay_samples(100, os_engine::effects::stereo_enhancer::rate_factor(96000))));
        // A reset empties the line.
        let (mut l, mut r) = (vec![0.0f32; 4], vec![1.0f32; 4]);
        s.process(&mut l, &mut r);
        s.reset();
        let (mut l, mut r) = (vec![0.0f32; 16], vec![0.0f32; 16]);
        s.process(&mut l, &mut r);
        assert!(r.iter().all(|&x| x == 0.0));
    }
}
