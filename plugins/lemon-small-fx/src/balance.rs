// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Balance: Lemon Studio's balance (a recreation of FL's Fruity Balance) as a plugin. The sound is
//! `os_engine::effects::balance`; the editor has the two knobs on one card and the two outputs' levels in a well.
//!
// spec: plugins/lemon-balance.md, plugins/fruity-balance.md

use nih_plug::prelude::*;
use os_engine::effects::balance::{self as dsp, Balance, Settings};
use os_ui::{Canvas, Rect};

use crate::params::{db_text, leading_number, nearest, parse_db};
use crate::{view, Card, Control, Display, Kind, Mark, Setting, SmallFx, Sound, Spec};

/// The settings in the original's order (spec §5).
pub const SETTINGS: [Setting; dsp::PARAMS] = [
    Setting { id: "balance", name: "Balance", range: dsp::RANGES[dsp::BALANCE], start: dsp::DEFAULTS[dsp::BALANCE], magnet: Some(0) },
    Setting { id: "volume", name: "Volume", range: dsp::RANGES[dsp::VOLUME], start: dsp::DEFAULTS[dsp::VOLUME], magnet: Some(256) },
];

/// Editor layout (spec §2), as the original's: the balance knob, the levels between, the volume knob; the value texts
/// under the knobs.
const CARDS: [Card; 3] = [
    Card { x0: 8.0, x1: 100.0, y0: 28.0, y1: 124.0, heading: "BALANCE" },
    Card { x0: 106.0, x1: 182.0, y0: 28.0, y1: 124.0, heading: "OUTPUT" },
    Card { x0: 188.0, x1: 280.0, y0: 28.0, y1: 124.0, heading: "VOLUME" },
];

const CONTROLS: [Control; 2] = [
    Control::Knob { id: "balance", cx: 54.0, cy: 70.0, r: 24.0, caption: "", bipolar: true },
    Control::Knob { id: "volume", cx: 234.0, cy: 70.0, r: 24.0, caption: "", bipolar: false },
];

pub const SPEC: &Spec = &Spec {
    name: "Lemon Balance",
    word: "BALANCE",
    about: "Lemon Studio's balance",
    settings: &SETTINGS,
    version: None,
    any_version: false,
    fields: &[],
    tail: &[],
    size: (288, 162),
    cards: &CARDS,
    controls: &CONTROLS,
    display: Some(Display { r: Rect::new(114.0, 36.0, 60.0, 80.0), draw: levels }),
    mark: Mark { x: 280.0, y: 143.0, anchor: 1.0, stacked: false },
    hint: Some(Rect::new(8.0, 132.0, 272.0, 22.0)),
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

/// The volume's gain at the centre (`LL` there): the level curve of `volume / 256`, 1 at 256.
fn volume_gain(volume: i32) -> f32 {
    dsp::gains(0, volume)[0]
}

/// Value texts (spec §3, Lemon Studio's choice): the balance as how far it turns to a side, the volume as dB.
fn text(k: usize, v: i32) -> String {
    match k {
        dsp::BALANCE => side_text(v),
        _ => db_text(volume_gain(v)),
    }
}

/// `Centred`, or `<share>% left` / `right` of the way to that side's end.
pub fn side_text(v: i32) -> String {
    match v {
        0 => "Centred".into(),
        v => format!("{:.1}% {}", f64::from(v.unsigned_abs()) / 128.0 * 100.0, if v < 0 { "left" } else { "right" }),
    }
}

/// A typed side: `centre`, `<n>% left` / `right` (or a signed share, negative = left).
pub fn parse_side(range: (i32, i32), s: &str) -> Option<i32> {
    let s = s.trim().to_ascii_lowercase();
    if s.starts_with('c') {
        return Some(0);
    }
    let x = leading_number(&s)?;
    let x = if s.contains('l') { -x.abs() } else { x };
    nearest(range, x, |v| Some(f64::from(v) / 128.0 * 100.0))
}

fn parse(k: usize, s: &str) -> Option<i32> {
    match k {
        dsp::BALANCE => parse_side(SETTINGS[k].range, s),
        _ => parse_db(SETTINGS[k].range, s, volume_gain),
    }
}

/// Each output's level for the same signal on both inputs (`LL + RL` left, `RR + LR` right), against a top of 2.
fn levels(c: &mut Canvas, r: Rect, v: &[i32]) {
    let [ll, rr, rl, lr] = dsp::gains(v[dsp::BALANCE], v[dsp::VOLUME]);
    view::level_bars(c, r, ll + rl, rr + lr, 2.0);
}

struct Bal(Balance);

impl Sound for Bal {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&Settings { v: [v[0], v[1]] });
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    /// The gains stand at the settings (nothing glides in).
    fn reset(&mut self) {
        self.0.flush();
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(Bal(Balance::with_settings(rate, &Settings { v: [v[0], v[1]] })))
}

pub struct BalanceKind;

impl Kind for BalanceKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-balance";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's balance";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Utility, ClapFeature::Mixing, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioBalnc";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Spatial];
}

pub type LemonBalance = SmallFx<BalanceKind>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(dsp::BALANCE, 0), "Centred");
        assert_eq!(text(dsp::BALANCE, -128), "100.0% left");
        assert_eq!(text(dsp::BALANCE, 32), "25.0% right");
        assert_eq!(text(dsp::VOLUME, 256), "0.0dB");
        assert_eq!(text(dsp::VOLUME, 0), "-INFdB");
        assert_eq!(text(dsp::VOLUME, 320), "5.6dB");
        assert_eq!(parse(dsp::BALANCE, "centre"), Some(0));
        assert_eq!(parse(dsp::BALANCE, "50% left"), Some(-64));
        assert_eq!(parse(dsp::BALANCE, "-25"), Some(-32));
        assert_eq!(parse(dsp::BALANCE, "100 right"), Some(128));
        assert_eq!(parse(dsp::VOLUME, "0 dB"), Some(256));
    }

    #[test]
    fn the_plugin_plays_the_engines_balance_from_its_settings() {
        let v = [-64, 200];
        let mut s = sound(44100, &v);
        let mut l = vec![0.5f32, -0.25, 0.75];
        let mut r = vec![0.1f32, 0.3, -0.6];
        let (l0, r0) = (l.clone(), r.clone());
        s.process(&mut l, &mut r);
        // The gains stand at the settings from the start: no glide.
        let [ll, rr, rl, lr] = dsp::gains(v[0], v[1]);
        for i in 0..3 {
            assert_eq!(l[i], ll * l0[i] + rl * r0[i]);
            assert_eq!(r[i], rr * r0[i] + lr * l0[i]);
        }
    }
}
