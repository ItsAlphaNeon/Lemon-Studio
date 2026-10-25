// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Fast Distortion: Lemon Studio's fast distortion (a recreation of FL's Fruity Fast Dist) as a plugin. The sound
//! is `os_engine::effects::fast_dist`; the editor has the input, the curve's threshold and type, the curve in a well
//! and the output's mix and post gain.
//!
// spec: plugins/lemon-fast-distortion.md, plugins/fruity-fast-dist.md

use nih_plug::prelude::*;
use os_engine::effects::fast_dist::{self as dsp, FastDist, Settings};
use os_ui::{Canvas, Rect};

use crate::params::{db_text, leading_number, nearest, parse_db};
use crate::{view, Card, Control, Display, Kind, Mark, Setting, SmallFx, Sound, Spec};

/// The settings in the original's order (spec §5).
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    Setting { id: "pre", name: "Pre amp", range: dsp::RANGES[dsp::PRE], start: dsp::DEFAULTS[dsp::PRE], magnet: Some(128) },
    Setting { id: "threshold", name: "Threshold", range: dsp::RANGES[dsp::THRESHOLD], start: dsp::DEFAULTS[dsp::THRESHOLD], magnet: None },
    Setting { id: "type", name: "Type", range: dsp::RANGES[dsp::TYPE], start: dsp::DEFAULTS[dsp::TYPE], magnet: None },
    Setting { id: "mix", name: "Mix", range: dsp::RANGES[dsp::MIX], start: dsp::DEFAULTS[dsp::MIX], magnet: None },
    Setting { id: "post", name: "Post gain", range: dsp::RANGES[dsp::POST], start: dsp::DEFAULTS[dsp::POST], magnet: None },
];

/// Editor layout (spec §2), as the original's: the knobs and the type switch in a row over the wordmark, the curve at
/// the right.
const CARDS: [Card; 2] = [
    Card { x0: 8.0, x1: 312.0, y0: 28.0, y1: 116.0, heading: "DISTORTION" },
    Card { x0: 318.0, x1: 468.0, y0: 28.0, y1: 146.0, heading: "CURVE" },
];

const CONTROLS: [Control; 5] = [
    Control::Knob { id: "pre", cx: 44.0, cy: 64.0, r: 20.0, caption: "PRE AMP", bipolar: false },
    Control::Knob { id: "threshold", cx: 108.0, cy: 64.0, r: 20.0, caption: "THRESHOLD", bipolar: false },
    Control::Choice { id: "type", x: 146.0, top: 49.0, names: &["A", "B"] },
    Control::Knob { id: "mix", cx: 212.0, cy: 64.0, r: 20.0, caption: "MIX", bipolar: false },
    Control::Knob { id: "post", cx: 276.0, cy: 64.0, r: 20.0, caption: "POST GAIN", bipolar: false },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Fast Distortion",
    word: "FAST DISTORTION",
    about: "Lemon Studio's fast distortion",
    settings: &SETTINGS,
    version: None,
    any_version: false,
    fields: &[],
    tail: &[],
    size: (476, 154),
    cards: &CARDS,
    controls: &CONTROLS,
    display: Some(Display { r: Rect::new(326.0, 36.0, 134.0, 102.0), draw: curve }),
    mark: Mark { x: 312.0, y: 135.0, anchor: 1.0, stacked: false },
    hint: None,
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

/// Value texts (spec §3, Lemon Studio's choice): pre amp and post gain as dB, the threshold as its number, the type
/// as A or B, the mix as the share of the distorted signal.
fn text(k: usize, v: i32) -> String {
    match k {
        dsp::THRESHOLD => v.to_string(),
        dsp::TYPE => if v == dsp::TYPE_A { "A" } else { "B" }.into(),
        dsp::MIX => format!("{:.1}%", f64::from(dsp::knob(v)) * 100.0),
        _ => db_text(dsp::knob(v)),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    match k {
        dsp::TYPE => match s.trim().to_ascii_lowercase().as_str() {
            "a" | "0" => Some(dsp::TYPE_A),
            "b" | "1" => Some(dsp::TYPE_B),
            _ => None,
        },
        dsp::THRESHOLD => leading_number(s).map(|x| (x.round() as i32).clamp(range.0, range.1)),
        dsp::MIX => {
            let x = leading_number(s)?;
            nearest(range, x, |v| Some(f64::from(dsp::knob(v)) * 100.0))
        }
        _ => parse_db(range, s, dsp::knob),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]) }
}

/// The transfer curve (input and output -1..1): the curve's whole 16-bit level at the input's magnitude times the
/// wet gain, over the dry part, as a sample is worked out (without the interpolation, which a drawing does not show).
fn curve(c: &mut Canvas, r: Rect, v: &[i32]) {
    let (threshold, kind) = dsp::curve(v[dsp::THRESHOLD], v[dsp::TYPE]);
    let pre = dsp::knob(v[dsp::PRE]);
    let (wet, dry) = dsp::gains(pre, dsp::knob(v[dsp::MIX]), dsp::knob(v[dsp::POST]));
    view::transfer(c, r, 1.0, 1.0, |x| {
        let i = (x.abs() * pre * 8191.0).min(8191.0) as usize;
        let shaped = dsp::level(i, 10 - threshold, kind) / 32768.0 * f64::from(wet);
        (f64::from(x) * f64::from(dry) + shaped.copysign(f64::from(x))) as f32
    });
}

struct Dist(FastDist);

impl Sound for Dist {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, _rate: u32) {}
    fn reset(&mut self) {}
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(_rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(Dist(FastDist::with_settings(&settings(v))))
}

pub struct FastDistortionKind;

impl Kind for FastDistortionKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-fast-distortion";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's fast distortion";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Distortion, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioFDist";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

pub type LemonFastDistortion = SmallFx<FastDistortionKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(dsp::PRE, 128), "0.0dB");
        assert_eq!(text(dsp::PRE, 192), "11.0dB");
        assert_eq!(text(dsp::THRESHOLD, 7), "7");
        assert_eq!(text(dsp::TYPE, 0), "A");
        assert_eq!(text(dsp::TYPE, 1), "B");
        assert_eq!(text(dsp::MIX, 128), "100.0%");
        assert_eq!(text(dsp::MIX, 0), "0.0%");
        assert_eq!(text(dsp::POST, 0), "-INFdB");
        assert_eq!(parse(dsp::TYPE, " b "), Some(1));
        assert_eq!(parse(dsp::THRESHOLD, "12"), Some(10));
        assert_eq!(parse(dsp::MIX, "100%"), Some(128));
        assert_eq!(parse(dsp::POST, "-inf"), Some(0));
        assert_eq!(parse(dsp::PRE, "0"), Some(128));
    }

    #[test]
    fn the_plugin_plays_the_engines_fast_distortion() {
        // FL's compressor preset and a stored threshold of 0 with type B (the curve of A at 10).
        for v in [[192, 1, 0, 104, 93], [128, 0, 1, 128, 128]] {
            let mut s = sound(44100, &v);
            let mut l = vec![0.087571196f32, -0.3, 0.9];
            let mut r = vec![0.021931838f32, 0.0, -1.5];
            let (l0, r0) = (l.clone(), r.clone());
            s.process(&mut l, &mut r);
            let d = FastDist::with_settings(&Settings { v });
            for (a, b) in l0.iter().zip(&l).chain(r0.iter().zip(&r)) {
                assert_eq!(d.sample(*a).to_bits(), b.to_bits());
            }
        }
    }
}
