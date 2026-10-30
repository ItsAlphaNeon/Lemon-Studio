// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Filter: Lemon Studio's filter (a recreation of FL's Fruity Filter) as a plugin. The sound is
//! `os_engine::effects::filter`, started with its coefficients at the settings; the editor has the cutoff and the
//! resonance, the three output levels, the X 2 and Center switches, and the response in a well.
//!
// spec: plugins/lemon-filter.md, plugins/fruity-filter.md

use nih_plug::prelude::*;
use os_engine::effects::filter::{self as dsp, Filter, Settings};
use os_theme::design::lemon as brand;
use os_ui::kit::lemon::plug;
use os_ui::{Canvas, Rect};

use crate::params::{db_text, leading_number, nearest, nearest_rising, parse_db, BYTE};
use crate::{Card, Control, Display, Kind, Mark, Setting, SmallFx, Sound, Spec};

/// Center, after the original's six settings: a switch stored in the byte after them (spec §5).
pub const CENTER: usize = dsp::PARAMS;

/// The settings in the original's order, then Center (spec §5).
pub const SETTINGS: [Setting; dsp::PARAMS + 1] = [
    Setting { id: "cutoff", name: "Cutoff frequency", range: dsp::RANGES[dsp::CUTOFF], start: dsp::DEFAULTS[dsp::CUTOFF], magnet: None },
    Setting { id: "resonance", name: "Resonance", range: dsp::RANGES[dsp::RESONANCE], start: dsp::DEFAULTS[dsp::RESONANCE], magnet: None },
    Setting { id: "low", name: "Low pass", range: dsp::RANGES[dsp::LOW], start: dsp::DEFAULTS[dsp::LOW], magnet: Some(1024) },
    Setting { id: "band", name: "Band pass", range: dsp::RANGES[dsp::BAND], start: dsp::DEFAULTS[dsp::BAND], magnet: Some(1024) },
    Setting { id: "high", name: "High pass", range: dsp::RANGES[dsp::HIGH], start: dsp::DEFAULTS[dsp::HIGH], magnet: Some(1024) },
    Setting { id: "x2", name: "X 2", range: dsp::RANGES[dsp::X2], start: dsp::DEFAULTS[dsp::X2], magnet: None },
    Setting { id: "center", name: "Center", range: (0, 1), start: 1, magnet: None },
];

/// Editor layout (spec §2): cards 6 px from the edges and 6 px apart (`Rect::span` takes the last pixel, so a card
/// ending at x1 leaves x1 + 1 .. x1 + 6 clear). The filter's two knobs, the three levels, the two switches; the
/// response and the wordmark under them.
const CARDS: [Card; 5] = [
    Card { x0: 6.0, x1: 138.0, y0: 28.0, y1: 116.0, heading: "FILTER" },
    Card { x0: 145.0, x1: 344.0, y0: 28.0, y1: 116.0, heading: "MIX" },
    Card { x0: 351.0, x1: 412.0, y0: 28.0, y1: 116.0, heading: "X 2" },
    Card { x0: 419.0, x1: 480.0, y0: 28.0, y1: 116.0, heading: "CENTER" },
    Card { x0: 6.0, x1: 318.0, y0: 123.0, y1: 200.0, heading: "" },
];

const CONTROLS: [Control; 7] = [
    Control::Knob { id: "cutoff", cx: 40.0, cy: 60.0, r: 24.0, caption: "CUTOFF", bipolar: false },
    Control::Knob { id: "resonance", cx: 105.0, cy: 64.0, r: 20.0, caption: "RES", bipolar: false },
    Control::Knob { id: "low", cx: 181.0, cy: 64.0, r: 20.0, caption: "LOW", bipolar: false },
    Control::Knob { id: "band", cx: 245.0, cy: 64.0, r: 20.0, caption: "BAND", bipolar: false },
    Control::Knob { id: "high", cx: 309.0, cy: 64.0, r: 20.0, caption: "HIGH", bipolar: false },
    Control::Choice { id: "x2", x: 364.0, top: 57.0, names: &["OFF", "ON"] },
    Control::Choice { id: "center", x: 432.0, top: 57.0, names: &["OFF", "ON"] },
];

/// The response well, 6 px inside its card.
const RESPONSE: Rect = Rect::new(12.0, 129.0, 301.0, 66.0);

pub const SPEC: &Spec = &Spec {
    name: "Lemon Filter",
    word: "FILTER",
    about: "Lemon Studio's filter",
    settings: &SETTINGS,
    version: Some(dsp::VERSION),
    any_version: false,
    fields: &[0, 1, 2, 3, 4, 5, BYTE],
    tail: &[1],
    blank: None,
    locate: None,
    size: (487, 207),
    cards: &CARDS,
    controls: &CONTROLS,
    display: Some(Display { r: RESPONSE, draw: response }),
    mark: Mark { x: 400.0, y: 162.0, anchor: 0.5, stacked: false },
    hint: None,
    labels: &[],
    text,
    parse,
    sound,
    rings: false,
};

/// The rate the cutoff's text and the response are given at (spec §3).
const RATE: f64 = 44100.0;

/// A frequency: one decimal below 100 Hz, whole hertz below 1 kHz, then kHz with two decimals.
fn hz_text(hz: f64) -> String {
    match hz {
        h if h < 100.0 => format!("{h:.1} Hz"),
        h if h < 1000.0 => format!("{h:.0} Hz"),
        h => format!("{:.2} kHz", h / 1000.0),
    }
}

/// Value texts (spec §3, Lemon Studio's choice).
fn text(k: usize, v: i32) -> String {
    match k {
        dsp::CUTOFF => hz_text(dsp::cutoff_hz(v, RATE)),
        dsp::RESONANCE => format!("{:.1}%", f64::from(v.clamp(0, 1024)) / 1024.0 * 100.0),
        dsp::X2 | CENTER => if v == 1 { "On" } else { "Off" }.into(),
        // The levels just under unity round to -0.0 dB: they read as unity.
        _ => match db_text(dsp::level(v)) {
            t if t == "-0.0dB" => "0.0dB".into(),
            t => t,
        },
    }
}

fn parse(k: usize, s: &str) -> Option<i32> {
    let range = SETTINGS[k].range;
    let low = s.trim().to_ascii_lowercase();
    match k {
        dsp::CUTOFF => {
            let x = leading_number(&low)?;
            let hz = if low.contains('k') { x * 1000.0 } else { x };
            Some(nearest_rising(range, hz, |v| dsp::cutoff_hz(v, RATE)))
        }
        dsp::RESONANCE => nearest(range, leading_number(&low)?, |v| Some(f64::from(v) / 1024.0 * 100.0)),
        dsp::X2 | CENTER => match low.as_str() {
            "on" | "1" => Some(1),
            "off" | "0" => Some(0),
            _ => None,
        },
        _ => parse_db(range, s, dsp::level),
    }
}

fn settings(v: &[i32]) -> Settings {
    Settings { v: std::array::from_fn(|k| v[k]), center: v[CENTER] == 1 }
}

/// The response (spec §2): its gain in dB over 20 Hz .. 20 kHz on a log scale, -36 .. +18 dB up; dim lines at 0 dB and
/// at 100 Hz, 1 kHz and 10 kHz.
fn response(c: &mut Canvas, r: Rect, v: &[i32]) {
    const LO: f64 = 20.0;
    const HI: f64 = 20000.0;
    const TOP: f64 = 18.0;
    const BOT: f64 = -36.0;
    let inner = r.inset(6.0);
    let x_of = |hz: f64| inner.x + inner.w * ((hz / LO).ln() / (HI / LO).ln()) as f32;
    let y_of = |db: f64| inner.y + inner.h * ((TOP - db.clamp(BOT, TOP)) / (TOP - BOT)) as f32;
    let unity = y_of(0.0).round();
    c.fill_rect(Rect::new(inner.x, unity, inner.w, 1.0), plug::GRID);
    for hz in [100.0, 1000.0, 10000.0] {
        c.fill_rect(Rect::new(x_of(hz).round(), inner.y, 1.0, inner.h), plug::GRID);
    }
    let s = settings(v);
    let n = inner.w.max(2.0) as usize;
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let hz = LO * (HI / LO).powf(i as f64 / n as f64);
            let g = dsp::response(&s.v, hz, RATE);
            let db = if g > 0.0 { 20.0 * g.log10() } else { BOT };
            (x_of(hz), y_of(db))
        })
        .collect();
    c.polyline(&pts, 1.5, brand::MAIN);
}

struct Sounding(Filter);

impl Sound for Sounding {
    fn set(&mut self, v: &[i32]) {
        self.0.set(&settings(v));
    }
    fn set_sample_rate(&mut self, rate: u32) {
        self.0.set_sample_rate(rate);
    }
    fn reset(&mut self) {
        os_engine::effects::Effect::reset(&mut self.0);
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right);
    }
}

fn sound(rate: u32, v: &[i32]) -> Box<dyn Sound> {
    Box::new(Sounding(Filter::with_settings(rate, &settings(v))))
}

pub struct FilterKind;

impl Kind for FilterKind {
    const SPEC: &'static Spec = SPEC;
    const CLAP_ID: &'static str = "org.openstudio.lemon-filter";
    const CLAP_DESCRIPTION: &'static str = "Lemon Studio's filter";
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Filter, ClapFeature::Stereo];
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioFiltr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Filter];
}

pub type LemonFilter = SmallFx<FilterKind>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::{default_state, fl_from_plugin_state, plugin_state_from_fl, setting, to_fl, values_of};

    #[test]
    fn value_texts_and_typed_values() {
        assert_eq!(text(dsp::CUTOFF, 1024), "7.35 kHz");
        assert_eq!(text(dsp::CUTOFF, 0), "0.0 Hz");
        assert_eq!(text(dsp::RESONANCE, 205), "20.0%");
        assert_eq!(text(dsp::LOW, 1024), "0.0dB");
        assert_eq!(text(dsp::BAND, 0), "-INFdB");
        assert_eq!(text(dsp::X2, 1), "On");
        assert_eq!(text(CENTER, 0), "Off");
        assert_eq!(parse(dsp::CUTOFF, "7.35 kHz"), Some(1024));
        assert_eq!(parse(dsp::CUTOFF, "0 Hz"), Some(0));
        assert_eq!(parse(dsp::LOW, "0 dB"), Some(1024));
        assert_eq!(parse(dsp::HIGH, "-inf"), Some(0));
        assert_eq!(parse(dsp::X2, "on"), Some(1));
        assert_eq!(parse(CENTER, "maybe"), None);
    }

    /// The saved state: the original's 29 bytes, Center in the last byte's bit 0; a version 1 state (28 bytes, no
    /// Center byte) reads Center off and gains the byte and version 2 when Center is switched on.
    #[test]
    fn the_state_is_the_originals_with_center_in_its_last_byte() {
        let b = default_state(SPEC);
        assert_eq!(b, Settings::default().to_state());
        assert_eq!(values_of(SPEC, &b), [1024, 205, 1024, 0, 512, 0, 1]);
        // FL's "Tiny speakers", Center byte with other bits set: kept.
        let mut fl = Settings { v: [544, 0, 0, 1024, 464, 1], center: true }.to_state();
        fl[28] = 0x81;
        let mut vals = values_of(SPEC, &fl);
        assert_eq!(vals, [544, 0, 0, 1024, 464, 1, 1]);
        vals[CENTER] = 0;
        let out = to_fl(SPEC, &vals, &fl);
        assert_eq!(out[28], 0x80);
        assert_eq!(fl_from_plugin_state(SPEC, &plugin_state_from_fl(SPEC, &out)), Some(out));
        // Version 1.
        let mut v1 = Settings { v: [300, 400, 500, 600, 700, 0], center: false }.to_state();
        v1.truncate(28);
        v1[0] = 1;
        assert_eq!(setting(SPEC, &v1, CENTER), 0);
        let mut vals = values_of(SPEC, &v1);
        assert_eq!(to_fl(SPEC, &vals, &v1), v1);
        vals[CENTER] = 1;
        let out = to_fl(SPEC, &vals, &v1);
        assert_eq!(Settings::from_state(&out), Some(Settings { v: [300, 400, 500, 600, 700, 0], center: true }));
        // Version 3: not read.
        let mut v3 = Settings::default().to_state();
        v3[0] = 3;
        v3[4] = 7;
        assert_eq!(crate::params::normalized(SPEC, v3), b);
    }

    #[test]
    fn the_plugin_plays_the_engines_filter() {
        let v = [480, 1024, 0, 0, 1024, 1, 1];
        let mut s = sound(44100, &v);
        let mut fx = Filter::with_settings(44100, &settings(&v));
        let input: Vec<f32> = (0..400).map(|i| ((i * 13 % 29) as f32 / 14.0 - 1.0) * 0.5).collect();
        let (mut l, mut r) = (input.clone(), input.clone());
        let (mut l2, mut r2) = (input.clone(), input);
        s.process(&mut l, &mut r);
        fx.process(&mut l2, &mut r2);
        assert_eq!((l, r), (l2, r2));
        // X 2 stored as 2 plays as off, as stored.
        assert!(!settings(&[1024, 205, 1024, 0, 512, 2, 1]).x2());
    }
}
