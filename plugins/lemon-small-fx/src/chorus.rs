// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Chorus: Lemon Studio's chorus (a recreation of FL's Fruity Chorus) as a plugin. The sound is
//! `os_engine::effects::chorus`, started as a loaded project's slot (lines empty, LFO phases from the fixed seed); the
//! editor has two rows: the chorus and the crossover, then the three LFOs.
//!
// spec: plugins/lemon-chorus.md, plugins/fruity-chorus.md

use nih_plug::prelude::*;

use os_engine::effects::chorus::{self as dsp, Chorus, Settings};
use os_ui::Rect;

use crate::params::{leading_number, nearest_rising};
use crate::{Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

const fn setting(id: &'static str, name: &'static str, k: usize) -> Setting {
    Setting { id, name, range: dsp::RANGES[k], start: dsp::DEFAULTS[k], magnet: None }
}

/// The settings in the original's order (spec §5).
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    setting("delay", "Delay", dsp::DELAY),
    setting("depth", "Depth", dsp::DEPTH),
    setting("stereo", "Stereo", dsp::STEREO),
    setting("speed1", "LFO 1 speed", dsp::SPEED[0]),
    setting("speed2", "LFO 2 speed", dsp::SPEED[1]),
    setting("speed3", "LFO 3 speed", dsp::SPEED[2]),
    setting("wave1", "LFO 1 wave", dsp::WAVE[0]),
    setting("wave2", "LFO 2 wave", dsp::WAVE[1]),
    setting("wave3", "LFO 3 wave", dsp::WAVE[2]),
    setting("cross", "Cross type", dsp::CROSS_TYPE),
    setting("cutoff", "Cross cutoff", dsp::CROSS_CUTOFF),
    setting("wet", "Wet only", dsp::WET_ONLY),
];

/// Editor layout (spec §2), as the original's: a grid of tiles, three across and four down.
const fn tile(id: &'static str, col: usize, row: usize, caption: &'static str) -> Control {
    let (x0, y0) = (8.0 + 164.0 * col as f32, 8.0 + 52.0 * row as f32);
    Control::Tile { id, x0, y0, x1: x0 + 158.0, y1: y0 + 46.0, caption }
}

/// The stored value of each wave's lowest setting, and the top for the last (the typed-in names).
pub const WAVES: &[(&str, i32)] = &[("ARC", 0), ("SHARP ARC", 342), ("SINES", 683), ("SHARP SINES", 1024)];

const CONTROLS: [Control; 12] = [
    tile("delay", 0, 0, "DELAY"),
    tile("depth", 1, 0, "DEPTH"),
    tile("stereo", 2, 0, "STEREO"),
    tile("speed1", 0, 1, "LFO 1 FREQ"),
    tile("speed2", 1, 1, "LFO 2 FREQ"),
    tile("speed3", 2, 1, "LFO 3 FREQ"),
    tile("wave1", 0, 2, "LFO 1 WAVE"),
    tile("wave2", 1, 2, "LFO 2 WAVE"),
    tile("wave3", 2, 2, "LFO 3 WAVE"),
    tile("cross", 0, 3, "CROSS TYPE"),
    tile("cutoff", 1, 3, "CROSS CUTOFF"),
    tile("wet", 2, 3, "WET ONLY"),
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Chorus",
    word: "CHORUS",
    about: "Lemon Studio's chorus",
    settings: &SETTINGS,
    version: Some(dsp::VERSION),
    any_version: false,
    fields: &[],
    tail: &[],
    size: (502, 248),
    cards: &[],
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: 494.0, y: 229.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 218.0, 486.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: true,
};

/// The delay in ms: `x * 29.95 + 0.05`.
fn delay_ms(v: i32) -> f64 {
    f64::from(dsp::position(dsp::DELAY, v)) * 29.95 + 0.05
}

/// The depth in ms: `5 x`.
fn depth_ms(v: i32) -> f64 {
    f64::from(dsp::position(dsp::DEPTH, v)) * 5.0
}

/// How far the right side's LFOs run ahead, in degrees (one table entry is 360 / 4096; 0 is one entry behind).
fn stereo_deg(v: i32) -> f64 {
    f64::from(dsp::stereo(dsp::position(dsp::STEREO, v)) as i32) * 360.0 / 4096.0
}

/// An LFO's speed in Hz: `5 x^2` (the sound's step per sample gives it at every rate, to its rounding).
fn speed_hz(k: usize, v: i32) -> f64 {
    let x = f64::from(dsp::position(k, v));
    5.0 * x * x
}

fn cutoff_hz(v: i32) -> f64 {
    f64::from(dsp::cutoff_hz(dsp::position(dsp::CROSS_CUTOFF, v)))
}

/// A frequency to a little better than the knob's step: 0.1 Hz below 1 kHz, 1 Hz below 10 kHz, 10 Hz above.
pub fn hz_text(hz: f64) -> String {
    match hz {
        hz if hz < 1000.0 => format!("{hz:.1}Hz"),
        hz if hz < 10000.0 => format!("{:.3}kHz", hz / 1000.0),
        hz => format!("{:.2}kHz", hz / 1000.0),
    }
}

const WAVE_NAMES: [&str; 4] = ["Arc", "Sharp arc", "Sines", "Sharp sines"];

/// Value texts (spec §3, Lemon Studio's choice).
fn text(k: usize, v: i32) -> String {
    let on = || dsp::switch(dsp::position(k, v));
    match k {
        dsp::DELAY => format!("{:.2}ms", delay_ms(v)),
        dsp::DEPTH => format!("{:.2}ms", depth_ms(v)),
        dsp::STEREO => format!("{:.1} deg", stereo_deg(v)),
        dsp::CROSS_TYPE => if on() { "Lows" } else { "Highs" }.into(),
        dsp::CROSS_CUTOFF => hz_text(cutoff_hz(v)),
        dsp::WET_ONLY => if on() { "On" } else { "Off" }.into(),
        k if dsp::WAVE.contains(&k) => WAVE_NAMES[dsp::shape(dsp::position(k, v))].into(),
        k => format!("{:.2}Hz", speed_hz(k, v)),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    match k {
        dsp::DELAY => Some(nearest_rising(range, leading_number(&low)?, delay_ms)),
        dsp::DEPTH => Some(nearest_rising(range, leading_number(&low)?, depth_ms)),
        dsp::STEREO => Some(nearest_rising(range, leading_number(&low)?, stereo_deg)),
        dsp::CROSS_TYPE => {
            if low.starts_with("low") {
                Some(range.1)
            } else {
                low.starts_with("high").then_some(range.0)
            }
        }
        dsp::CROSS_CUTOFF => {
            let x = leading_number(&low)?;
            Some(nearest_rising(range, if low.contains('k') { x * 1000.0 } else { x }, cutoff_hz))
        }
        dsp::WET_ONLY => match low.as_str() {
            "on" | "yes" => Some(range.1),
            "off" | "no" => Some(range.0),
            _ => None,
        },
        k if dsp::WAVE.contains(&k) => {
            let at = if low.starts_with("sharp s") {
                3
            } else if low.starts_with("sharp") {
                1
            } else if low.starts_with("sin") {
                2
            } else if low.starts_with("arc") {
                0
            } else {
                return None;
            };
            Some(WAVES[at].1)
        }
        k => Some(nearest_rising(range, leading_number(&low)?, |v| speed_hz(k, v))),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]) }
}

struct ChorusSound(Chorus);

impl Sound for ChorusSound {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    /// The lines and the crossover empty; the LFOs start on the generator's next three phases (as FL's plugin when it
    /// is enabled again).
    fn reset(&mut self) {
        self.0.start();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(ChorusSound(Chorus::with_settings(rate, &settings(v))))
}

pub struct ChorusKind;

impl Kind for ChorusKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-chorus";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's chorus";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Chorus, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioChrus";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

pub type LemonChorus = SmallFx<ChorusKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        // The defaults.
        assert_eq!(text(dsp::DELAY, 512), "15.03ms");
        assert_eq!(text(dsp::DEPTH, 2250), "2.25ms");
        assert_eq!(text(dsp::STEREO, 341), "59.9 deg");
        assert_eq!(text(dsp::STEREO, 0), "-0.1 deg");
        assert_eq!(text(dsp::STEREO, 1024), "179.9 deg");
        assert_eq!(text(dsp::SPEED[0], 1500), "0.45Hz");
        assert_eq!(text(dsp::SPEED[2], 5000), "5.00Hz");
        assert_eq!(text(dsp::WAVE[0], 23), "Arc");
        assert_eq!(text(dsp::WAVE[1], 341), "Arc");
        assert_eq!(text(dsp::WAVE[1], 342), "Sharp arc");
        assert_eq!(text(dsp::WAVE[2], 1023), "Sines");
        assert_eq!(text(dsp::WAVE[2], 1024), "Sharp sines");
        assert_eq!(text(dsp::CROSS_TYPE, 25), "Highs");
        assert_eq!(text(dsp::CROSS_TYPE, 26), "Lows");
        assert_eq!(text(dsp::CROSS_CUTOFF, 2500), "320.2Hz");
        assert_eq!(text(dsp::CROSS_CUTOFF, 5000), "12.54kHz");
        assert_eq!(text(dsp::CROSS_CUTOFF, 0), "8.2Hz");
        assert_eq!(text(dsp::CROSS_CUTOFF, 3277), "1.001kHz");
        assert_eq!(text(dsp::WET_ONLY, 256), "Off");
        assert_eq!(text(dsp::WET_ONLY, 257), "On");
        assert_eq!(parse(dsp::DELAY, "30"), Some(1024));
        assert_eq!(parse(dsp::DEPTH, "2.25 ms"), Some(2250));
        assert_eq!(parse(dsp::SPEED[1], "5"), Some(5000));
        assert_eq!(parse(dsp::WAVE[0], "Sharp sines"), Some(1024));
        assert_eq!(parse(dsp::WAVE[0], "sines"), Some(683));
        assert_eq!(parse(dsp::WAVE[0], "square"), None);
        assert_eq!(parse(dsp::CROSS_TYPE, "lows"), Some(100));
        assert_eq!(parse(dsp::CROSS_CUTOFF, "12.54kHz"), Some(5000));
        assert_eq!(parse(dsp::CROSS_CUTOFF, "320.2"), Some(2500));
        assert_eq!(parse(dsp::WET_ONLY, "on"), Some(1024));
    }

    /// The plugin plays the engine's sound: the same samples as a slot started on the same phases.
    #[test]
    fn the_plugin_plays_the_slots_sound() {
        // FL's "Light HF" preset.
        let v = [128, 1534, 480, 2695, 0, 1914, 0, 0, 949, 0, 4479, 0];
        let mut s = sound(48000, &v);
        let mut c = Chorus::with_settings(48000, &settings(&v));
        let input = |i: usize| ((i as f32) * 0.031).sin() * 0.5;
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = (0..1500).map(|i| (input(i), -input(i + 7))).unzip();
        let (mut l2, mut r2) = (l.clone(), r.clone());
        s.process(&mut l, &mut r);
        c.process(&mut l2, &mut r2);
        assert_eq!((l, r), (l2, r2));
        // A reset empties the lines: silence in, silence out.
        s.reset();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        s.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
    }
}
