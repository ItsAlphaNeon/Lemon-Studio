// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Transient Processor: Lemon Studio's transient processor (a recreation of FL's Transient Processor) as a
//! plugin. The sound is `os_engine::effects::transient_processor`, started with the settings in force (as a loaded
//! project's slot); the editor has the attack and the release with their shapes, the split, and the drive, the gain
//! and the on switch.
//!
// spec: plugins/lemon-transient-processor.md, plugins/transient-processor.md

use nih_plug::prelude::*;

use os_engine::effects::transient_processor::{self as dsp, Settings, TransientProcessor};
use os_ui::Rect;

use crate::params::{db_text, leading_number, nearest, parse_db};
use crate::{Card, Control, Kind, Mark, Setting, SmallFx, Sound, Spec};

/// The settings in the original's order (spec §5).
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    Setting { id: "attack", name: "Attack", range: dsp::RANGES[dsp::ATTACK], start: dsp::DEFAULTS[dsp::ATTACK], magnet: Some(80) },
    Setting { id: "drive", name: "Drive", range: dsp::RANGES[dsp::DRIVE], start: dsp::DEFAULTS[dsp::DRIVE], magnet: Some(16) },
    Setting { id: "release", name: "Release", range: dsp::RANGES[dsp::RELEASE], start: dsp::DEFAULTS[dsp::RELEASE], magnet: Some(80) },
    Setting { id: "gain", name: "Gain", range: dsp::RANGES[dsp::GAIN], start: dsp::DEFAULTS[dsp::GAIN], magnet: Some(80) },
    Setting {
        id: "attack_shape",
        name: "Attack shape",
        range: dsp::RANGES[dsp::ATTACK_SHAPE],
        start: dsp::DEFAULTS[dsp::ATTACK_SHAPE],
        magnet: None,
    },
    Setting {
        id: "release_shape",
        name: "Release shape",
        range: dsp::RANGES[dsp::RELEASE_SHAPE],
        start: dsp::DEFAULTS[dsp::RELEASE_SHAPE],
        magnet: None,
    },
    Setting { id: "split", name: "Split frequency", range: dsp::RANGES[dsp::SPLIT], start: dsp::DEFAULTS[dsp::SPLIT], magnet: None },
    Setting { id: "balance", name: "Split balance", range: dsp::RANGES[dsp::BALANCE], start: dsp::DEFAULTS[dsp::BALANCE], magnet: Some(80) },
    Setting { id: "on", name: "Processing", range: dsp::RANGES[dsp::ON], start: dsp::DEFAULTS[dsp::ON], magnet: None },
];

/// Editor layout (spec §2), as the original's: the wordmark on top; the split frequency over the processing switch,
/// the large attack knob over its shape button, the drive over the split balance, the large release knob over its
/// shape button and the gain; the hint well along the foot.
const CARDS: [Card; 5] = [
    Card { x0: 8.0, x1: 100.0, y0: 34.0, y1: 162.0, heading: "" },
    Card { x0: 106.0, x1: 226.0, y0: 34.0, y1: 162.0, heading: "" },
    Card { x0: 232.0, x1: 328.0, y0: 34.0, y1: 162.0, heading: "" },
    Card { x0: 334.0, x1: 454.0, y0: 34.0, y1: 162.0, heading: "" },
    Card { x0: 460.0, x1: 552.0, y0: 34.0, y1: 162.0, heading: "" },
];

const CONTROLS: [Control; 9] = [
    Control::Knob { id: "split", cx: 54.0, cy: 68.0, r: 18.0, caption: "SPLIT FREQ", bipolar: false },
    Control::Choice { id: "on", x: 32.0, top: 118.0, names: &["OFF", "ON"] },
    Control::Knob { id: "attack", cx: 166.0, cy: 80.0, r: 30.0, caption: "ATTACK", bipolar: true },
    Control::Shape { id: "attack_shape", cx: 166.0, cy: 144.0 },
    Control::Knob { id: "drive", cx: 280.0, cy: 66.0, r: 18.0, caption: "DRIVE", bipolar: false },
    Control::Knob { id: "balance", cx: 280.0, cy: 122.0, r: 16.0, caption: "SPLIT BALANCE", bipolar: true },
    Control::Knob { id: "release", cx: 394.0, cy: 80.0, r: 30.0, caption: "RELEASE", bipolar: true },
    Control::Shape { id: "release_shape", cx: 394.0, cy: 144.0 },
    Control::Knob { id: "gain", cx: 506.0, cy: 68.0, r: 18.0, caption: "GAIN", bipolar: true },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Transient Processor",
    word: "TRANSIENT PROCESSOR",
    about: "Lemon Studio's transient processor",
    settings: &SETTINGS,
    version: Some(dsp::VERSION),
    any_version: false,
    fields: &[],
    tail: &dsp::DISPLAY,
    size: (560, 200),
    cards: &CARDS,
    controls: &CONTROLS,
    display: None,
    mark: Mark { x: 280.0, y: 17.0, anchor: 0.5, stacked: false },
    hint: Some(Rect::new(8.0, 170.0, 544.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

/// Setting `k` at stored value `v` through the sound's mapping.
fn mapped(k: usize, v: i32) -> f32 {
    let x = dsp::position(k, v);
    match k {
        dsp::ATTACK => dsp::attack(x),
        dsp::RELEASE => dsp::release(x),
        dsp::DRIVE => dsp::drive(x),
        _ => dsp::gain(x),
    }
}

/// The split frequency in Hz.
fn split_hz(v: i32) -> f64 {
    dsp::split_hz(dsp::position(dsp::SPLIT, v))
}

/// The split balance as a signed share: -100 % only the low band, +100 % only the processed high band.
fn balance_share(v: i32) -> f64 {
    (f64::from(v.clamp(0, 160)) - 80.0) / 80.0 * 100.0
}

/// Value texts (spec §3, Lemon Studio's choice).
fn text(k: usize, v: i32) -> String {
    match k {
        dsp::ATTACK | dsp::RELEASE => match mapped(k, v) {
            0.0 => "0%".into(),
            w => format!("{:+.2}%", f64::from(w) * 100.0),
        },
        dsp::DRIVE | dsp::GAIN => db_text(mapped(k, v)),
        dsp::ATTACK_SHAPE | dsp::RELEASE_SHAPE => match dsp::shape(dsp::position(k, v)) {
            0.5 => "Sharp",
            1.0 => "Mid",
            _ => "Slow",
        }
        .into(),
        dsp::SPLIT => format!("{:.1}Hz", split_hz(v)),
        dsp::BALANCE => match balance_share(v) {
            0.0 => "Centred".into(),
            p => format!("{:.1}% {}", p.abs(), if p < 0.0 { "low" } else { "high" }),
        },
        _ => if v == 0 { "Off" } else { "On" }.into(),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    let named = |names: &[&str]| names.iter().position(|n| low.starts_with(n)).map(|i| i as i32);
    match k {
        dsp::ATTACK | dsp::RELEASE => nearest(range, leading_number(&low)?, |v| Some(f64::from(mapped(k, v)) * 100.0)),
        dsp::DRIVE | dsp::GAIN => parse_db(range, s, |v| mapped(k, v)),
        dsp::ATTACK_SHAPE | dsp::RELEASE_SHAPE => named(&["sharp", "mid", "slow"]),
        dsp::SPLIT => {
            let x = leading_number(&low)?;
            let hz = if low.contains("khz") { x * 1000.0 } else { x };
            nearest(range, hz, |v| Some(split_hz(v)))
        }
        dsp::BALANCE if low.starts_with('c') => Some(80),
        dsp::BALANCE => {
            let x = leading_number(&low)?;
            let x = if low.contains("low") { -x.abs() } else { x };
            nearest(range, x, |v| Some(balance_share(v)))
        }
        _ => named(&["off", "on"]),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]) }
}

struct Processor(TransientProcessor);

impl Sound for Processor {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    /// Nothing: the original's state is never reset (its followers settle in the silence it runs on in).
    fn reset(&mut self) {}
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

/// The plugin starts with the settings in force, as a loaded project's slot does; later changes glide.
fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(Processor(TransientProcessor::with_settings(rate, &settings(v))))
}

pub struct TransientProcessorKind;

impl Kind for TransientProcessorKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-transient-processor";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's transient processor";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::TransientShaper, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioTrPro";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

pub type LemonTransientProcessor = SmallFx<TransientProcessorKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(dsp::ATTACK, 80), "0%");
        assert_eq!(text(dsp::ATTACK, 160), "+40.00%");
        assert_eq!(text(dsp::ATTACK, 0), "-60.00%");
        assert_eq!(text(dsp::RELEASE, 0), "-100.00%");
        assert_eq!(text(dsp::RELEASE, 160), "+60.00%");
        assert_eq!(text(dsp::DRIVE, 0), "-20.0dB");
        assert_eq!(text(dsp::DRIVE, 16), "0.0dB");
        assert_eq!(text(dsp::DRIVE, 160), "23.5dB");
        assert_eq!(text(dsp::GAIN, 0), "-9.0dB");
        assert_eq!(text(dsp::GAIN, 160), "9.0dB");
        assert_eq!(text(dsp::ATTACK_SHAPE, 0), "Sharp");
        assert_eq!(text(dsp::RELEASE_SHAPE, 2), "Slow");
        assert_eq!(text(dsp::SPLIT, 0), "5.0Hz");
        assert_eq!(text(dsp::SPLIT, 80), "253.8Hz");
        assert_eq!(text(dsp::SPLIT, 160), "1000.0Hz");
        assert_eq!(text(dsp::BALANCE, 80), "Centred");
        assert_eq!(text(dsp::BALANCE, 40), "50.0% low");
        assert_eq!(text(dsp::BALANCE, 160), "100.0% high");
        assert_eq!(text(dsp::ON, 1), "On");
        assert_eq!(parse(dsp::ATTACK, "+40"), Some(160));
        assert_eq!(parse(dsp::RELEASE, "-100%"), Some(0));
        assert_eq!(parse(dsp::DRIVE, "0 dB"), Some(16));
        assert_eq!(parse(dsp::GAIN, "+9"), Some(160));
        assert_eq!(parse(dsp::ATTACK_SHAPE, "slow"), Some(2));
        assert_eq!(parse(dsp::SPLIT, "1 kHz"), Some(160));
        assert_eq!(parse(dsp::SPLIT, "253.8"), Some(80));
        assert_eq!(parse(dsp::BALANCE, "50% low"), Some(40));
        assert_eq!(parse(dsp::BALANCE, "centre"), Some(80));
        assert_eq!(parse(dsp::ON, "off"), Some(0));
    }

    /// The plugin plays the engine's samples: the settings in force from the first sample, later changes gliding.
    #[test]
    fn the_plugin_plays_the_engines_samples() {
        let v = [160, 38, 160, 80, 2, 2, 0, 80, 1]; // FL's "Max transients"
        let mut s = sound(44100, &v);
        let mut e = TransientProcessor::with_settings(44100, &settings(&v));
        let input: Vec<f32> = (0..4096).map(|i| if i % 700 < 30 { ((i as f32) * 0.7).sin() * 0.8 } else { 0.0 }).collect();
        let (mut l, mut r) = (input.clone(), input.clone());
        s.process(&mut l, &mut r);
        let (mut el, mut er) = (input.clone(), input.clone());
        e.process(&mut el, &mut er);
        assert_eq!(l, el);
        assert_ne!(l, input);
        // A change: the same glide in both.
        let w = [80, 0, 80, 160, 1, 1, 80, 80, 1];
        s.set(&w);
        e.set(&settings(&w));
        let (mut l, mut r) = (input.clone(), input.clone());
        s.process(&mut l, &mut r);
        let (mut el, mut er) = (input.clone(), input);
        e.process(&mut el, &mut er);
        assert_eq!(l, el);
        assert_eq!(r, er);
    }
}
