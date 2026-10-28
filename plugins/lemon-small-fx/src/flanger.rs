// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Flanger: Lemon Studio's flanger (a recreation of FL's Fruity Flanger) as a plugin. The sound is
//! `os_engine::effects::flanger`, started as a loaded project's slot (lines empty, the LFO at phase 0); the editor is
//! the original's grid of twelve tiles, three across and four down.
//!
// spec: plugins/lemon-flanger.md, plugins/fruity-flanger.md

use nih_plug::prelude::*;

use os_engine::effects::flanger::{self as dsp, Flanger, Settings};
use os_ui::Rect;

use crate::params::{db_text_fine, leading_number, nearest_rising, parse_db_rising};
use crate::{Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

const fn setting(id: &'static str, name: &'static str, k: usize) -> Setting {
    Setting { id, name, range: dsp::RANGES[k], start: dsp::DEFAULTS[k], magnet: None }
}

/// The settings in the original's order (spec §5).
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    setting("delay", "Delay", dsp::DELAY),
    setting("depth", "Depth", dsp::DEPTH),
    setting("rate", "Rate", dsp::RATE),
    setting("phase", "Phase", dsp::PHASE),
    setting("damp", "Damp", dsp::DAMP),
    setting("shape", "Shape", dsp::SHAPE),
    setting("feed", "Feedback", dsp::FEED),
    setting("invert_feed", "Invert feedback", dsp::INVERT_FEED),
    setting("invert_wet", "Invert wet", dsp::INVERT_WET),
    setting("dry", "Dry", dsp::DRY),
    setting("wet", "Wet", dsp::WET),
    setting("cross", "Cross", dsp::CROSS),
];

// Editor layout (spec §2): a 6 px grid. Every tile and the hint well are GAP clear of the edges and of each other;
// the band under the tiles holds the hint well and the wordmark on one centre line.
const GAP: f32 = 6.0;
const TILE_W: f32 = 160.0;
const TILE_H: f32 = 46.0;
const COLS: usize = 3;
const ROWS: usize = 4;
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
    tile("delay", 0, 0, "DELAY"),
    tile("depth", 1, 0, "DEPTH"),
    tile("rate", 2, 0, "RATE"),
    tile("phase", 0, 1, "PHASE"),
    tile("damp", 1, 1, "DAMP"),
    tile("shape", 2, 1, "SHAPE"),
    tile("feed", 0, 2, "FEED"),
    tile("invert_feed", 1, 2, "INV FEEDBACK"),
    tile("invert_wet", 2, 2, "INV WET"),
    tile("dry", 0, 3, "DRY"),
    tile("wet", 1, 3, "WET"),
    tile("cross", 2, 3, "CROSS"),
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Flanger",
    word: "FLANGER",
    about: "Lemon Studio's flanger",
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

/// The delay or depth in ms: `C(x, -0.8) * 20`.
fn ms(k: usize, v: i32) -> f64 {
    f64::from(dsp::ms(x(k, v)))
}

/// The rate in Hz: `C(x, -0.9) * 5`.
fn hz(v: i32) -> f64 {
    f64::from(dsp::speed(x(dsp::RATE, v)))
}

/// How far the right LFO runs ahead, in degrees.
fn degrees(v: i32) -> f64 {
    f64::from(x(dsp::PHASE, v)) * 360.0
}

/// A share of a knob in percent.
fn percent(k: usize, v: i32) -> f64 {
    f64::from(x(k, v)) * 100.0
}

/// Value texts (spec §3, Lemon Studio's choice).
fn text(k: usize, v: i32) -> String {
    match k {
        dsp::DELAY | dsp::DEPTH => format!("{:.2}ms", ms(k, v)),
        dsp::RATE => format!("{:.3}Hz", hz(v)),
        dsp::PHASE => format!("{:.0} deg", degrees(v)),
        dsp::DAMP => format!("{:.1}%", percent(k, v)),
        dsp::SHAPE => match v {
            v if v <= SETTINGS[k].range.0 => "Sine".into(),
            v if v >= SETTINGS[k].range.1 => "Triangle".into(),
            v => format!("{:.1}% triangle", percent(k, v)),
        },
        dsp::FEED => format!("{:.0}%", percent(k, v)),
        dsp::INVERT_FEED | dsp::INVERT_WET => if dsp::switch(x(k, v)) { "On" } else { "Off" }.into(),
        k => db_text_fine(x(k, v)),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    match k {
        dsp::DELAY | dsp::DEPTH => Some(nearest_rising(range, leading_number(&low)?, |v| ms(k, v))),
        dsp::RATE => Some(nearest_rising(range, leading_number(&low)?, hz)),
        dsp::PHASE => Some(nearest_rising(range, leading_number(&low)?, degrees)),
        dsp::SHAPE if low.starts_with("sin") => Some(range.0),
        dsp::SHAPE if low.starts_with("tri") => Some(range.1),
        dsp::DAMP | dsp::SHAPE | dsp::FEED => Some(nearest_rising(range, leading_number(&low)?, |v| percent(k, v))),
        dsp::INVERT_FEED | dsp::INVERT_WET => match low.as_str() {
            "on" | "yes" => Some(range.1),
            "off" | "no" => Some(range.0),
            _ => None,
        },
        k => parse_db_rising(range, &low, |v| x(k, v)),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]) }
}

struct FlangerSound(Flanger);

impl Sound for FlangerSound {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    /// The lines and the low pass empty, the LFO back at phase 0 (as FL's plugin when it is enabled again).
    fn reset(&mut self) {
        self.0.start();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(FlangerSound(Flanger::with_settings(rate, &settings(v))))
}

pub struct FlangerKind;

impl Kind for FlangerKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-flanger";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's flanger";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Flanger, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioFlngr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

pub type LemonFlanger = SmallFx<FlangerKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_is_six_pixels_apart() {
        assert_eq!(SPEC.size, (504, 242));
        let Control::Tile { x0, y0, x1, y1, .. } = CONTROLS[0] else { unreachable!() };
        assert_eq!((x0, y0, x1 + 1.0 - x0, y1 + 1.0 - y0), (6.0, 6.0, 160.0, 46.0));
        let Control::Tile { x0, x1, y0, y1, .. } = CONTROLS[11] else { unreachable!() };
        // The last tile ends 6 px from the right edge and 6 px over the band; the band 6 px from the bottom.
        assert_eq!((W - (x1 + 1.0), BAND_Y - (y1 + 1.0), x0), (6.0, 6.0, 338.0));
        assert_eq!(SPEC.size.1 as f32 - (BAND_Y + BAND_H), 6.0);
        let _ = y0;
    }

    #[test]
    fn value_texts_and_typed_values() {
        // The defaults (FL's editor shows 0 ms, 2.35294 ms, 0.31250 Hz, 118 degrees, 23 percent, -3.1, -4.4, -6.0 dB).
        assert_eq!(text(dsp::DELAY, 0), "0.00ms");
        assert_eq!(text(dsp::DEPTH, 2000), "2.35ms");
        assert_eq!(text(dsp::DELAY, 1024), "20.00ms");
        assert_eq!(text(dsp::RATE, 2000), "0.313Hz");
        assert_eq!(text(dsp::RATE, 5000), "5.000Hz");
        assert_eq!(text(dsp::PHASE, 118), "118 deg");
        assert_eq!(text(dsp::DAMP, 0), "0.0%");
        assert_eq!(text(dsp::DAMP, 2500), "50.0%");
        assert_eq!(text(dsp::SHAPE, 0), "Sine");
        assert_eq!(text(dsp::SHAPE, 512), "50.0% triangle");
        assert_eq!(text(dsp::SHAPE, 1024), "Triangle");
        assert_eq!(text(dsp::FEED, 23), "23%");
        assert_eq!(text(dsp::INVERT_FEED, 512), "Off");
        assert_eq!(text(dsp::INVERT_FEED, 513), "On");
        assert_eq!(text(dsp::DRY, 3500), "-3.1dB");
        assert_eq!(text(dsp::WET, 3000), "-4.4dB");
        assert_eq!(text(dsp::CROSS, 2500), "-6.0dB");
        assert_eq!(text(dsp::CROSS, 0), "-INFdB");
        assert_eq!(text(dsp::WET, 5000), "0.0dB");
        assert_eq!(parse(dsp::DELAY, "20"), Some(1024));
        assert_eq!(parse(dsp::DEPTH, "2.3529 ms"), Some(2000));
        assert_eq!(text(dsp::DEPTH, parse(dsp::DEPTH, "2.35 ms").unwrap()), "2.35ms");
        assert_eq!(text(dsp::WET, 4990), "0.0dB", "not -0.0dB");
        assert_eq!(parse(dsp::RATE, "5"), Some(5000));
        assert_eq!(parse(dsp::PHASE, "90"), Some(90));
        assert_eq!(parse(dsp::SHAPE, "triangle"), Some(1024));
        assert_eq!(parse(dsp::SHAPE, "sine"), Some(0));
        assert_eq!(parse(dsp::SHAPE, "25"), Some(256));
        assert_eq!(parse(dsp::INVERT_WET, "on"), Some(1024));
        assert_eq!(parse(dsp::DRY, "-inf"), Some(0));
        assert_eq!(parse(dsp::DRY, "-6"), Some(2506));
        assert_eq!(parse(dsp::WET, "0"), Some(5000));
    }

    /// The plugin plays the engine's sound: the same samples as a slot.
    #[test]
    fn the_plugin_plays_the_slots_sound() {
        let v = [200, 3000, 2600, 90, 800, 300, 70, 1024, 0, 3000, 4000, 1500];
        let mut s = sound(48000, &v);
        let mut f = Flanger::with_settings(48000, &settings(&v));
        let input = |i: usize| ((i as f32) * 0.021).sin() * 0.5;
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = (0..3000).map(|i| (input(i), -input(i + 11))).unzip();
        let (mut l2, mut r2) = (l.clone(), r.clone());
        s.process(&mut l, &mut r);
        f.process(&mut l2, &mut r2);
        assert_eq!((l, r), (l2, r2));
        // A reset empties the lines: silence in, silence out.
        s.reset();
        let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
        s.process(&mut l, &mut r);
        assert!(l.iter().chain(&r).all(|&x| x == 0.0));
    }
}
