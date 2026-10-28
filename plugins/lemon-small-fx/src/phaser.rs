// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Phaser: Lemon Studio's phaser (a recreation of FL's Fruity Phaser) as a plugin. The sound is
//! `os_engine::effects::phaser`, started as a loaded project's slot (the filters empty, the LFO as after the idle
//! frames); the editor is the original's grid of nine tiles, three across and three down.
//!
// spec: plugins/lemon-phaser.md, plugins/fruity-phaser.md

use nih_plug::prelude::*;

use os_engine::effects::phaser::{self as dsp, Phaser, Settings};
use os_ui::Rect;

use crate::params::{db_text_fine, leading_number, nearest, nearest_rising, parse_db_rising};
use crate::{Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

const fn setting(id: &'static str, name: &'static str, k: usize, magnet: Option<i32>) -> Setting {
    Setting { id, name, range: dsp::RANGES[k], start: dsp::DEFAULTS[k], magnet }
}

/// The output gain knob's unity (0 dB before the feedback's reduction).
pub const UNITY: i32 = 2500;

/// The settings in the original's order (spec §5).
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    setting("sweep", "Sweep frequency", dsp::SWEEP, None),
    setting("min_depth", "Minimum depth", dsp::MIN_DEPTH, None),
    setting("max_depth", "Maximum depth", dsp::MAX_DEPTH, None),
    setting("range", "Frequency range", dsp::RANGE, None),
    setting("stereo", "Stereo", dsp::STEREO, None),
    setting("stages", "Stages", dsp::STAGES, None),
    setting("feedback", "Feedback", dsp::FEEDBACK, None),
    setting("dry_wet", "Dry - wet", dsp::DRY_WET, None),
    setting("gain", "Output gain", dsp::GAIN, Some(UNITY)),
];

// Editor layout (spec §2): a 6 px grid. Every tile and the hint well are GAP clear of the edges and of each other;
// the band under the tiles holds the hint well and the wordmark on one centre line.
const GAP: f32 = 6.0;
const TILE_W: f32 = 160.0;
const TILE_H: f32 = 46.0;
const COLS: usize = 3;
const ROWS: usize = 3;
const W: f32 = GAP + COLS as f32 * (TILE_W + GAP);
const BAND_Y: f32 = GAP + ROWS as f32 * (TILE_H + GAP);
const BAND_H: f32 = 22.0;
const H: f32 = BAND_Y + BAND_H + GAP;

/// Tile `(col, row)`: `Control::Tile` takes end-inclusive edges, so the last pixel is `x0 + TILE_W - 1`.
const fn tile(id: &'static str, col: usize, row: usize, caption: &'static str) -> Control {
    let (x0, y0) = (GAP + (TILE_W + GAP) * col as f32, GAP + (TILE_H + GAP) * row as f32);
    Control::Tile { id, x0, y0, x1: x0 + TILE_W - 1.0, y1: y0 + TILE_H - 1.0, caption }
}

const CONTROLS: [Control; dsp::PARAMS] = [
    tile("sweep", 0, 0, "SWEEP FREQ"),
    tile("min_depth", 1, 0, "MIN DEPTH"),
    tile("max_depth", 2, 0, "MAX DEPTH"),
    tile("range", 0, 1, "FREQ RANGE"),
    tile("stereo", 1, 1, "STEREO"),
    tile("stages", 2, 1, "STAGES"),
    tile("feedback", 0, 2, "FEEDBACK"),
    tile("dry_wet", 1, 2, "DRY - WET"),
    tile("gain", 2, 2, "OUT GAIN"),
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Phaser",
    word: "PHASER",
    about: "Lemon Studio's phaser",
    settings: &SETTINGS,
    version: Some(dsp::VERSION),
    any_version: false,
    fields: &[],
    tail: &[],
    size: (W as u32, H as u32),
    cards: &[],
    controls: &CONTROLS,
    display: None,
    // The name's last letter ends a pixel inside `x` (its side bearing).
    mark: Mark { x: W - GAP + 1.0, y: BAND_Y + BAND_H / 2.0, anchor: 1.0, stacked: false },
    // The well's outline covers its far edges (as `Rect::span`): one pixel less each way.
    hint: Some(Rect::new(GAP, BAND_Y, W - 2.0 * GAP - 1.0, BAND_H - 1.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: true,
};

fn x(k: usize, v: i32) -> f32 {
    dsp::position(k, v)
}

/// The sweep frequency in Hz on the 2 Hz range (`false`) or the 10 Hz one.
fn hz(v: i32, wide: bool) -> f64 {
    f64::from(dsp::speed(x(dsp::SWEEP, v), wide))
}

/// A share of a knob in percent.
fn percent(k: usize, v: i32) -> f64 {
    f64::from(x(k, v)) * 100.0
}

/// How far the right LFO runs ahead, in degrees (up to half a cycle).
fn degrees(v: i32) -> f64 {
    f64::from(x(dsp::STEREO, v)) * 180.0
}

/// The output gain knob's level: `2 x` (the feedback lowers it further, by `sqrt(1 - f^2)`).
fn knob_gain(v: i32) -> f32 {
    x(dsp::GAIN, v) * 2.0
}

/// Value texts (spec §3, Lemon Studio's choice).
fn text(k: usize, v: i32) -> String {
    match k {
        // The frequency depends on the range knob: both, the 2 Hz range's first.
        dsp::SWEEP => format!("{:.3} / {:.2}Hz", hz(v, false), hz(v, true)),
        dsp::MIN_DEPTH | dsp::MAX_DEPTH | dsp::FEEDBACK => format!("{:.1}%", percent(k, v)),
        dsp::RANGE => if dsp::wide(x(k, v)) { "10Hz" } else { "2Hz" }.into(),
        dsp::STEREO => format!("{:.1} deg", degrees(v)),
        dsp::STAGES => format!("{} stages", dsp::stages(v)),
        dsp::DRY_WET => format!("{:.1}% wet", 100.0 - percent(k, v)),
        _ => db_text_fine(knob_gain(v)),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    match k {
        // The 2 Hz range's frequency; a whole text names the value that shows it.
        dsp::SWEEP => {
            let v = nearest_rising(range, leading_number(&low)?, |v| hz(v, false));
            let same = |w: i32| text(k, w).to_ascii_lowercase().replace(' ', "") == low.replace(' ', "");
            Some((v - 40..=v + 40).filter(|&w| (range.0..=range.1).contains(&w)).find(|&w| same(w)).unwrap_or(v))
        }
        dsp::MIN_DEPTH | dsp::MAX_DEPTH | dsp::FEEDBACK => Some(nearest_rising(range, leading_number(&low)?, |v| percent(k, v))),
        dsp::RANGE => {
            if low.starts_with("small") || leading_number(&low).is_some_and(|n| n < 6.0) {
                Some(range.0)
            } else if low.starts_with("large") || leading_number(&low).is_some() {
                Some(range.1)
            } else {
                None
            }
        }
        dsp::STEREO => Some(nearest_rising(range, leading_number(&low)?, degrees)),
        dsp::STAGES => Some(nearest_rising(range, leading_number(&low)?, |v| dsp::stages(v) as f64)),
        dsp::DRY_WET => {
            let n = leading_number(&low)?;
            // "<n>% dry" names the other share.
            let wet = if low.contains("dry") { 100.0 - n } else { n };
            nearest(range, wet, |v| Some(100.0 - percent(k, v)))
        }
        _ => parse_db_rising(range, &low, knob_gain),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]) }
}

struct PhaserSound(Phaser);

impl Sound for PhaserSound {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    /// The filters empty and the LFO where a song starts it (as a new slot).
    fn reset(&mut self) {
        self.0.start(dsp::IDLE);
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(PhaserSound(Phaser::with_settings(rate, &settings(v))))
}

pub struct PhaserKind;

impl Kind for PhaserKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-phaser";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's phaser";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Phaser, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioPhasr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

pub type LemonPhaser = SmallFx<PhaserKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_is_six_pixels_apart() {
        assert_eq!(SPEC.size, (504, 190));
        let Control::Tile { x0, y0, x1, y1, .. } = CONTROLS[8] else { unreachable!() };
        assert_eq!((x0, y0, W - (x1 + 1.0), BAND_Y - (y1 + 1.0)), (338.0, 110.0, 6.0, 6.0));
    }

    #[test]
    fn value_texts_and_typed_values() {
        // The defaults (FL's editor shows 0.50000 Hz, 0.10000, 0.80000, small, 0.50000 phase, 8, 0.40000 amount,
        // 50 % wet and 4.08240 dB).
        assert_eq!(text(dsp::SWEEP, 2500), "0.238 / 1.19Hz");
        assert_eq!(text(dsp::SWEEP, 5000), "2.000 / 10.00Hz");
        assert_eq!(text(dsp::SWEEP, 0), "0.000 / 0.00Hz");
        assert_eq!(text(dsp::MIN_DEPTH, 100), "10.0%");
        assert_eq!(text(dsp::MAX_DEPTH, 800), "80.0%");
        assert_eq!(text(dsp::RANGE, 512), "2Hz");
        assert_eq!(text(dsp::RANGE, 513), "10Hz");
        assert_eq!(text(dsp::STEREO, 512), "90.0 deg");
        assert_eq!(text(dsp::STAGES, 7), "8 stages");
        assert_eq!(text(dsp::STAGES, 0), "3 stages");
        assert_eq!(text(dsp::FEEDBACK, 400), "40.0%");
        assert_eq!(text(dsp::DRY_WET, 512), "50.0% wet");
        assert_eq!(text(dsp::DRY_WET, 0), "100.0% wet");
        assert_eq!(text(dsp::GAIN, 4000), "4.1dB");
        assert_eq!(text(dsp::GAIN, UNITY), "0.0dB");
        assert_eq!(text(dsp::GAIN, 0), "-INFdB");
        assert_eq!(parse(dsp::SWEEP, "2"), Some(5000));
        assert_eq!(text(dsp::SWEEP, parse(dsp::SWEEP, "0.238 / 1.19Hz").unwrap()), "0.238 / 1.19Hz");
        assert_eq!(parse(dsp::RANGE, "10hz"), Some(1024));
        assert_eq!(parse(dsp::RANGE, "small"), Some(0));
        assert_eq!(parse(dsp::STAGES, "23"), Some(22));
        assert_eq!(parse(dsp::STAGES, "1"), Some(0));
        assert_eq!(parse(dsp::DRY_WET, "25% dry"), Some(256));
        assert_eq!(parse(dsp::DRY_WET, "100"), Some(0));
        assert_eq!(parse(dsp::GAIN, "0"), Some(UNITY));
        assert_eq!(parse(dsp::GAIN, "-inf"), Some(0));
    }

    /// The plugin plays the engine's sound: the same samples as a slot.
    #[test]
    fn the_plugin_plays_the_slots_sound() {
        let v = [3500, 150, 900, 700, 300, 12, 600, 400, 3000];
        let mut s = sound(44100, &v);
        let mut p = Phaser::with_settings(44100, &settings(&v));
        let input = |i: usize| ((i as f32) * 0.017).sin() * 0.5;
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = (0..3000).map(|i| (input(i), input(i + 5))).unzip();
        let (mut l2, mut r2) = (l.clone(), r.clone());
        s.process(&mut l, &mut r);
        p.process(&mut l2, &mut r2);
        assert_eq!((l, r), (l2, r2));
        // A reset empties the filters: silence in, silence out.
        s.reset();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        s.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
    }
}
