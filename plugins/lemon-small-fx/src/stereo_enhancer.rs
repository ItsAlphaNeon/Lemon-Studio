// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Stereo Enhancer: Lemon Studio's stereo enhancer (a recreation of FL's Fruity Stereo Enhancer) as a plugin.
//! The sound is `os_engine::effects::stereo_enhancer`, started with its gains at the settings (no pre-roll); the editor
//! has the separation and pan, the phase offset and its placement, and the volume and the inversion.
//!
// spec: plugins/lemon-stereo-enhancer.md, plugins/fruity-stereo-enhancer.md

use nih_plug::prelude::*;

use os_engine::effects::stereo_enhancer::{self as dsp, Settings, StereoEnhancer};

use crate::balance::{parse_side, side_text};
use crate::params::{db_text, leading_number, nearest, parse_db};
use crate::{Card, Control, Kind, Setting, SmallFx, Sound, Spec};

/// The settings in the original's order (spec §5).
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    Setting { id: "pan", name: "Pan", range: dsp::RANGES[dsp::PAN], start: dsp::DEFAULTS[dsp::PAN], magnet: Some(0) },
    Setting { id: "volume", name: "Volume", range: dsp::RANGES[dsp::VOLUME], start: dsp::DEFAULTS[dsp::VOLUME], magnet: Some(256) },
    Setting { id: "separation", name: "Stereo separation", range: dsp::RANGES[dsp::SEPARATION], start: dsp::DEFAULTS[dsp::SEPARATION], magnet: Some(0) },
    Setting { id: "phase", name: "Phase offset", range: dsp::RANGES[dsp::PHASE], start: dsp::DEFAULTS[dsp::PHASE], magnet: Some(0) },
    Setting { id: "placement", name: "Phase offset placement", range: dsp::RANGES[dsp::PLACEMENT], start: dsp::DEFAULTS[dsp::PLACEMENT], magnet: None },
    Setting { id: "invert", name: "Inversion", range: dsp::RANGES[dsp::INVERT], start: dsp::DEFAULTS[dsp::INVERT], magnet: None },
];

/// Editor layout (spec §2).
const CARDS: [Card; 3] = [
    Card { x0: 8.0, x1: 156.0, y0: 28.0, y1: 116.0, heading: "STEREO" },
    Card { x0: 162.0, x1: 310.0, y0: 28.0, y1: 116.0, heading: "PHASE OFFSET" },
    Card { x0: 316.0, x1: 464.0, y0: 28.0, y1: 116.0, heading: "OUTPUT" },
];

const CONTROLS: [Control; 6] = [
    Control::Knob { id: "separation", cx: 45.0, cy: 61.0, r: 22.0, caption: "SEPARATION", bipolar: true },
    Control::Knob { id: "pan", cx: 119.0, cy: 61.0, r: 22.0, caption: "PAN", bipolar: true },
    Control::Knob { id: "phase", cx: 204.0, cy: 61.0, r: 22.0, caption: "OFFSET", bipolar: true },
    Control::Choice { id: "placement", x0: 250.0, x1: 300.0, top: 45.0, names: &["PRE", "POST"] },
    Control::Knob { id: "volume", cx: 358.0, cy: 61.0, r: 22.0, caption: "VOLUME", bipolar: false },
    Control::Choice { id: "invert", x0: 402.0, x1: 456.0, top: 37.0, names: &["NONE", "LEFT", "RIGHT"] },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Stereo Enhancer",
    word: "STEREO ENHANCER",
    about: "Lemon Studio's stereo enhancer",
    settings: &SETTINGS,
    size: (472, 154),
    cards: &CARDS,
    controls: &CONTROLS,
    display: None,
    text,
    parse,
    sound,
    rings: true,
};

/// The phase offset's delay in ms, `(1001^(|k| / 512) - 1) / 2`, 0 to 500 (the sound rounds it to samples).
pub fn phase_ms(k: i32) -> f64 {
    (1001f64.powf(f64::from(k.unsigned_abs()) / 512.0) - 1.0) / 2.0
}

/// Value texts (spec §3, Lemon Studio's choice).
fn text(k: usize, v: i32) -> String {
    match k {
        dsp::PAN => side_text(v),
        dsp::VOLUME => db_text(dsp::volume_gain(v)),
        dsp::SEPARATION => match v {
            0 => "None".into(),
            v => format!("{:.1}% {}", f64::from(v.unsigned_abs()) / 128.0 * 100.0, if v < 0 { "separated" } else { "merged" }),
        },
        dsp::PHASE => match v {
            0 => "Off".into(),
            v => format!("{:.2}ms {}", phase_ms(v), if v < 0 { "left" } else { "right" }),
        },
        dsp::PLACEMENT => if v == dsp::PRE { "Pre" } else { "Post" }.into(),
        _ => match v {
            dsp::INVERT_LEFT => "Left",
            dsp::INVERT_RIGHT => "Right",
            _ => "None",
        }
        .into(),
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    let named = |names: &[&str]| names.iter().position(|n| low.starts_with(n)).map(|i| i as i32);
    // A signed number; `neg` in the text makes it negative.
    let signed = |neg: &str| leading_number(&low).map(|x| if low.contains(neg) { -x.abs() } else { x });
    match k {
        dsp::PAN => parse_side(range, s),
        dsp::VOLUME => parse_db(range, s, dsp::volume_gain),
        dsp::SEPARATION if low.starts_with("none") => Some(0),
        dsp::SEPARATION => nearest(range, signed("sep")?, |v| Some(f64::from(v) / 128.0 * 100.0)),
        dsp::PHASE if low.starts_with("off") => Some(0),
        dsp::PHASE => nearest(range, signed("l")?, |v| Some(phase_ms(v).copysign(f64::from(v)))),
        dsp::PLACEMENT => named(&["pre", "post"]),
        _ => named(&["none", "left", "right"]),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]) }
}

struct Enhancer(StereoEnhancer);

impl Enhancer {
    /// Sets the phase offset again: the line gets the length for the current rate, empty.
    fn renew_line(&mut self) {
        let phase = self.0.settings().v[dsp::PHASE];
        self.0.apply(dsp::PHASE, phase);
    }
}

impl Sound for Enhancer {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
        self.renew_line();
    }
    /// The line empties; the gains stand at the settings.
    fn reset(&mut self) {
        self.renew_line();
        self.0.flush();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

/// The plugin starts with its gains at the settings (Lemon Studio's choice: FL's mixer slot glides there from the
/// creation values in its pre-roll, which a plugin has none of).
fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    let mut e = StereoEnhancer::with_settings(rate, &settings(v));
    e.flush();
    Box::new(Enhancer(e))
}

pub struct StereoEnhancerKind;

impl Kind for StereoEnhancerKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-stereo-enhancer";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's stereo enhancer";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Utility, ClapFeature::Mixing, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioStEnh";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Spatial];
}

pub type LemonStereoEnhancer = SmallFx<StereoEnhancerKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        // FL's "For strings" preset: -18, 256, -54, -248, Post, none.
        assert_eq!(text(dsp::PAN, -18), "14.1% left");
        assert_eq!(text(dsp::VOLUME, 256), "0.0dB");
        assert_eq!(text(dsp::SEPARATION, -54), "42.2% separated");
        assert_eq!(text(dsp::SEPARATION, 96), "75.0% merged");
        assert_eq!(text(dsp::SEPARATION, 0), "None");
        assert_eq!(text(dsp::PHASE, 512), "500.00ms right");
        assert_eq!(text(dsp::PHASE, -248), "13.70ms left");
        assert_eq!(text(dsp::PHASE, 0), "Off");
        assert_eq!(text(dsp::PLACEMENT, 1), "Post");
        assert_eq!(text(dsp::INVERT, 2), "Right");
        assert_eq!(parse(dsp::SEPARATION, "42.2% separated"), Some(-54));
        assert_eq!(parse(dsp::SEPARATION, "none"), Some(0));
        assert_eq!(parse(dsp::PHASE, "13.70ms left"), Some(-248));
        assert_eq!(parse(dsp::PHASE, "500"), Some(512));
        assert_eq!(parse(dsp::PHASE, "off"), Some(0));
        assert_eq!(parse(dsp::PLACEMENT, "Pre"), Some(0));
        assert_eq!(parse(dsp::INVERT, "left"), Some(1));
        assert_eq!(parse(dsp::PAN, "centred"), Some(0));
    }

    #[test]
    fn the_plugin_starts_at_its_settings_and_delays_for_its_rate() {
        // "For strings": the left channel delayed after the gains.
        let v = [-18, 256, -54, -248, 1, 0];
        let mut s = sound(44100, &v);
        let n = 2048;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        l[0] = 1.0;
        s.process(&mut l, &mut r);
        // The engine's line for 44100 Hz; the gains at the settings from the first sample.
        let d = dsp::delay_samples(-248, dsp::rate_factor(44100));
        let [ll, _, _, lr] = dsp::gains(&v);
        assert!(d > 0);
        assert_eq!(l[d], ll);
        assert!(l[..d].iter().all(|&x| x == 0.0));
        assert_eq!(r[0], lr);
        // A new rate gives the line its length there.
        s.set_sample_rate(96000);
        let mut l = vec![0.0f32; 4096];
        let mut r = vec![0.0f32; 4096];
        l[0] = 1.0;
        s.process(&mut l, &mut r);
        let d96 = dsp::delay_samples(-248, dsp::rate_factor(96000));
        assert!(d96 > d);
        assert_eq!(l[d96], ll);
    }
}
