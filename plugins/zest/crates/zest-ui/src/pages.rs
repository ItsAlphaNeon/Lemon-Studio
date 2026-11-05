// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What each page shows: the Main page's cards and their controls (by oscillator and filter model), the Effects
//! page's slots, the Advanced page's sections. Layout and painting are in `view`.

use crate::macros::Macro;
use crate::params::{Model, ParamTable};
use crate::rules;

/// The pages, in tab order.
pub const PAGES: [&str; 3] = ["MAIN", "EFFECTS", "ADVANCED"];
pub const MAIN: usize = 0;
pub const EFFECTS: usize = 1;
pub const ADVANCED: usize = 2;

/// One control in a card's grid cell.
#[derive(Clone, Debug, PartialEq)]
pub enum Ctl {
    /// A knob with its caption.
    Knob { key: &'static str, cap: &'static str, bipolar: bool },
    Macro(Macro),
    /// On / off switches stacked in the cell: (key, label).
    Switches(Vec<(&'static str, &'static str)>),
    /// A choice as pills (stacked in a cell, side by side across a span), with short labels.
    Pills { key: &'static str, labels: Vec<&'static str>, cap: Option<&'static str> },
    /// A choice with many options: a box that opens a menu.
    Menu { key: &'static str, cap: &'static str },
    /// Two choice boxes stacked (pitch bend up / down) under one caption.
    MenuPair { keys: [&'static str; 2], cap: &'static str },
    /// The filter's envelope amount: VCF1/FreqModDepth, captioned with its source.
    EnvAmount,
    /// LFO 1's depth at its n-th active route.
    LfoDepth(usize),
}

/// A cell of a card's grid (columns of equal width).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub col: u8,
    pub row: u8,
    pub span: u8,
    pub ctl: Ctl,
}

fn at(col: u8, row: u8, ctl: Ctl) -> Cell {
    Cell { col, row, span: 1, ctl }
}

fn wide(col: u8, row: u8, span: u8, ctl: Ctl) -> Cell {
    Cell { col, row, span, ctl }
}

fn knob(key: &'static str, cap: &'static str) -> Ctl {
    Ctl::Knob { key, cap, bipolar: false }
}

fn bip(key: &'static str, cap: &'static str) -> Ctl {
    Ctl::Knob { key, cap, bipolar: true }
}

fn pills(key: &'static str, labels: &[&'static str], cap: Option<&'static str>) -> Ctl {
    Ctl::Pills { key, labels: labels.to_vec(), cap }
}

fn menu(key: &'static str, cap: &'static str) -> Ctl {
    Ctl::Menu { key, cap }
}

fn sw(list: &[(&'static str, &'static str)]) -> Ctl {
    Ctl::Switches(list.to_vec())
}

/// A card of the Main page.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub title: String,
    /// The whole card veiled (its source drives nothing).
    pub veiled: bool,
    /// A row of pills across the top (the model choice).
    pub top: Option<Ctl>,
    /// A control at the right end of the top row (the pitch bend range).
    pub top_extra: Option<Ctl>,
    pub cols: u8,
    pub rows: u8,
    pub cells: Vec<Cell>,
}

/// Columns of the Main page's side cards.
pub const COLS: u8 = 5;

/// The oscillator card's cells for a model. Detune sits at row 0 column 3 and vibrato at row 1 column 4 in every
/// model, so they never move.
pub fn osc_cells(model: i32) -> Vec<Cell> {
    let detune = at(3, 0, Ctl::Macro(Macro::Detune));
    let vib = at(4, 1, knob("OSC/Vibrato", "VIBRATO"));
    match model {
        rules::DUAL => vec![
            at(0, 0, bip("OSC/Tune1", "TUNE 1")),
            at(1, 0, bip("OSC/Tune2", "TUNE 2")),
            at(2, 0, knob("OSC/FM", "FM")),
            detune,
            at(4, 0, knob("OSC/OscMix", "MIX 1·2")),
            at(0, 1, sw(&[("OSC/Triangle1On", "TRI"), ("OSC/Saw1On", "SAW"), ("OSC/Pwm1On", "PULSE"), ("OSC/Noise1On", "NOISE")])),
            at(1, 1, sw(&[("OSC/Sine2On", "SINE"), ("OSC/Triangle2On", "TRI"), ("OSC/Saw2On", "SAW"), ("OSC/Pulse2On", "PULSE")])),
            at(2, 1, knob("OSC/PulseWidth", "PW")),
            at(3, 1, sw(&[("OSC/PWM2On", "PWM 2"), ("OSC/Sync2", "SYNC 2")])),
            vib,
            wide(0, 2, 3, pills("OSC/ShapeModel", &["IDEAL", "ANALOG 1", "ANALOG 2"], Some("MIXER"))),
        ],
        rules::DCO => vec![
            at(0, 0, bip("OSC/Tune1", "OCTAVE")),
            at(1, 0, knob("OSC/PulseWidth", "PW")),
            at(2, 0, knob("OSC/Volume3", "SUB")),
            detune,
            at(4, 0, knob("OSC/NoiseVol", "NOISE")),
            at(0, 1, menu("OSC/PulseShape", "PULSE")),
            at(1, 1, menu("OSC/SawShape", "SAW")),
            at(2, 1, menu("OSC/SuboscShape", "SUB SHAPE")),
            vib,
        ],
        rules::ECO => vec![
            at(0, 0, bip("OSC/Tune1", "TUNE 1")),
            at(1, 0, bip("OSC/Tune2", "TUNE 2")),
            at(2, 0, knob("OSC/PulseWidth", "PW")),
            detune,
            at(0, 1, knob("OSC/EcoWave1", "WAVE 1")),
            at(1, 1, knob("OSC/EcoWave2", "WAVE 2")),
            at(2, 1, knob("OSC/Volume1", "VOL 1")),
            at(3, 1, knob("OSC/Volume2", "VOL 2")),
            vib,
            at(0, 2, sw(&[("OSC/RingmodPulse", "RING PULSE")])),
            at(2, 2, knob("OSC/Shape1", "RING PW")),
        ],
        rules::DIGITAL => vec![
            at(0, 0, bip("OSC/Tune1", "TUNE 1")),
            at(1, 0, bip("OSC/Tune2", "TUNE 2")),
            at(2, 0, knob("OSC/FM", "FM")),
            detune,
            at(4, 0, knob("OSC/OscMix", "MIX 1·2")),
            at(0, 1, menu("OSC/DigitalType1", "TYPE 1")),
            at(1, 1, knob("OSC/PulseWidth", "SHAPE 1A")),
            at(2, 1, knob("OSC/DigitalShape2", "SHAPE 1B")),
            at(3, 1, sw(&[("OSC/Sync2", "SYNC"), ("OSC/RingmodPulse", "RING"), ("OSC/DigitalAntiAlias", "CLEAN")])),
            vib,
            at(0, 2, menu("OSC/DigitalType2", "TYPE 2")),
            at(1, 2, knob("OSC/DigitalShape3", "SHAPE 2A")),
            at(2, 2, knob("OSC/DigitalShape4", "SHAPE 2B")),
        ],
        // Triple VCO
        _ => vec![
            at(0, 0, bip("OSC/Tune1", "TUNE 1")),
            at(1, 0, bip("OSC/Tune2", "TUNE 2")),
            at(2, 0, bip("OSC/Tune3", "TUNE 3")),
            detune,
            at(4, 0, knob("OSC/FM", "FM")),
            at(0, 1, knob("OSC/Shape1", "SHAPE 1")),
            at(1, 1, knob("OSC/Shape2", "SHAPE 2")),
            at(2, 1, knob("OSC/Shape3", "SHAPE 3")),
            at(3, 1, knob("OSC/NoiseVol", "NOISE")),
            vib,
            at(0, 2, knob("OSC/Volume1", "VOL 1")),
            at(1, 2, knob("OSC/Volume2", "VOL 2")),
            at(2, 2, knob("OSC/Volume3", "VOL 3")),
            at(3, 2, sw(&[("OSC/Sync2", "SYNC 2"), ("OSC/Sync3", "SYNC 3")])),
            at(4, 2, pills("OSC/NoiseColor", &["WHITE", "PINK"], None)),
        ],
    }
}

/// The filter's character control by model (column 4, row 0).
fn filter_character(model: i32) -> Ctl {
    match model {
        rules::CASCADE => sw(&[("VCF1/LadderMode", "12 DB"), ("VCF1/LadderColor", "ROUGH")]),
        rules::MULTIMODE => pills("VCF1/SvfMode", &["LP 24", "LP 12", "HP", "BP"], None),
        rules::BITE => pills("VCF1/SlnKyRevision", &["REV 1", "REV 2"], Some("BITE")),
        rules::UHBIE => knob("VCF1/ShapeMix", "LP > HP"),
        _ => pills("VCF1/LadderMode", &["24 DB", "12 DB"], Some("SLOPE")),
    }
}

/// The Main page's cards: left column, then right column.
pub fn main_cards(m: &dyn Model) -> (Vec<Card>, Vec<Card>) {
    let om = m.int("OSC/Model");
    let osc = Card {
        title: "OSCILLATORS".into(),
        veiled: false,
        top_extra: None,
        top: Some(pills("OSC/Model", &["TRIPLE", "DUAL", "DCO", "ECO", "DIGITAL"], None)),
        cols: COLS,
        rows: 3,
        cells: osc_cells(om),
    };
    let voice = Card {
        title: "VOICE".into(),
        veiled: false,
        top: Some(pills("VCC/Mode", &["POLY", "MONO", "LEGATO", "DUO", "POLY 2"], None)),
        top_extra: Some(Ctl::MenuPair { keys: ["VCC/PitchBend Up", "VCC/PitchBend Down"], cap: "BEND" }),
        cols: COLS,
        rows: 1,
        cells: vec![
            at(0, 0, menu("VCC/Voices", "VOICES")),
            at(1, 0, knob("VCC/Glide", "GLIDE")),
            at(2, 0, pills("VCC/GlideMode", &["TIME", "RATE"], Some("GLIDE BY"))),
            at(3, 0, Ctl::Macro(Macro::Drift)),
            at(4, 0, knob("VCA1/Volume", "VOLUME")),
        ],
    };
    let filter = Card {
        title: "FILTER".into(),
        veiled: false,
        top_extra: None,
        top: Some(pills("VCF1/Model", &["LADDER", "CASCADE", "MULTI", "BITE", "UHBIE"], None)),
        cols: COLS,
        rows: 2,
        cells: vec![
            at(0, 0, knob("VCF1/Frequency", "CUTOFF")),
            at(1, 0, knob("VCF1/Resonance", "RESO")),
            at(2, 0, Ctl::EnvAmount),
            at(3, 0, knob("VCF1/KeyFollow", "KEY")),
            at(4, 0, filter_character(m.int("VCF1/Model"))),
            at(0, 1, knob("VCF1/Feedback", "FEEDBACK")),
            at(1, 1, knob("HPF/Frequency", "HP CUT")),
            at(2, 1, menu("HPF/Model", "HIGH-PASS")),
        ],
    };
    let (amp_title, amp_veiled) = rules::amp_env_title(m);
    let env = |p: &'static str| -> Vec<Cell> {
        let k = |n: &'static str| -> &'static str {
            match (p, n) {
                ("ENV1", "A") => "ENV1/Attack",
                ("ENV1", "D") => "ENV1/Decay",
                ("ENV1", "S") => "ENV1/Sustain",
                ("ENV1", "R") => "ENV1/Release",
                ("ENV1", _) => "ENV1/Velocity",
                (_, "A") => "ENV2/Attack",
                (_, "D") => "ENV2/Decay",
                (_, "S") => "ENV2/Sustain",
                (_, "R") => "ENV2/Release",
                _ => "ENV2/Velocity",
            }
        };
        vec![
            at(0, 0, knob(k("A"), "ATTACK")),
            at(1, 0, knob(k("D"), "DECAY")),
            at(2, 0, knob(k("S"), "SUSTAIN")),
            at(3, 0, knob(k("R"), "RELEASE")),
            at(4, 0, knob(k("V"), "VELOCITY")),
        ]
    };
    let amp = Card { title: amp_title.into(), veiled: amp_veiled, top: None,
        top_extra: None, cols: COLS, rows: 1, cells: env("ENV1") };
    let fenv = Card {
        title: "FILTER ENVELOPE".into(),
        veiled: rules::source_card_veiled(m, rules::src::ENV2),
        top: None,
        top_extra: None,
        cols: COLS,
        rows: 1,
        cells: env("ENV2"),
    };
    let lfo = Card {
        title: "LFO 1".into(),
        veiled: rules::source_card_veiled(m, rules::src::LFO1),
        top: None,
        top_extra: None,
        cols: COLS,
        rows: 1,
        cells: vec![
            at(0, 0, menu("LFO1/Waveform", "WAVE")),
            at(1, 0, menu("LFO1/Sync", "SYNC")),
            at(2, 0, bip("LFO1/Rate", "RATE")),
            at(3, 0, Ctl::LfoDepth(0)),
            at(4, 0, Ctl::LfoDepth(1)),
        ],
    };
    (vec![osc, voice], vec![filter, amp, fenv, lfo])
}

/// The effect modules in slot order of FX1/Module (FX2/Module the same with "2").
pub const MODULES: [(&str, &str); 5] = [("CHORUS", "Chorus"), ("PHASER", "Phaser"), ("PLATE", "Plate"), ("DELAY", "Delay"), ("ROTARY", "Rotary")];

/// Effect slot `slot` (1 or 2): its on switch, module choice and the chosen module's parameters (visible ones, in the
/// plugin's order).
pub struct Slot {
    pub on: String,
    pub module: String,
    pub params: Vec<String>,
}

pub fn effect_slot(t: &ParamTable, m: &dyn Model, slot: usize) -> Slot {
    let module = format!("FX{slot}/Module");
    let i = m.int(&module).clamp(0, 4) as usize;
    let group = format!("{} {slot}", MODULES[i].1);
    Slot {
        on: format!("main/Active #FX{slot}"),
        module,
        params: t.all().iter().filter(|p| p.group == group && !p.hidden).map(|p| p.key.clone()).collect(),
    }
}

/// The page that shows parameter `key`: the Main page's cards, an effect slot's parameters, or else the Advanced page,
/// which has every other.
pub fn page_of(t: &ParamTable, m: &dyn Model, key: &str) -> usize {
    let (left, right) = main_cards(m);
    let on_card = |c: &Card| {
        let has = |ctl: &Ctl| main_keys(ctl).contains(&key);
        c.top.as_ref().is_some_and(has) || c.top_extra.as_ref().is_some_and(has) || c.cells.iter().any(|cell| has(&cell.ctl))
    };
    if left.iter().chain(&right).any(on_card) {
        return MAIN;
    }
    let group = t.get(key).map_or("", |p| p.group.as_str());
    if key.starts_with("FX") || key.contains("#FX") || MODULES.iter().any(|(_, g)| group.starts_with(g)) {
        return EFFECTS;
    }
    ADVANCED
}

/// The Advanced page's sections in order: (title, keys). Every visible parameter except the effects' (their own page),
/// the big oscillator section split in three.
pub fn advanced_sections(t: &ParamTable) -> Vec<(String, Vec<String>)> {
    const ORDER: &[&str] = &[
        "Oscillators", "Oscillator waves", "Oscillator modulation", "High-pass", "Filter", "Amp", "Envelope 1", "Envelope 2", "LFO 1", "LFO 2", "Modifiers", "Matrix", "Voice",
        "Clock", "Arpeggiator", "Quality and spread",
    ];
    let mut out: Vec<(String, Vec<String>)> = ORDER.iter().map(|s| (s.to_string(), Vec::new())).collect();
    for p in t.all() {
        if p.hidden || p.key.starts_with("main/Active") {
            continue;
        }
        let g = if p.group == "Main" { "Voice".to_string() } else { p.group.clone() };
        let g = if g == "Oscillators" { osc_section(&p.key).to_string() } else { g };
        match out.iter_mut().find(|(s, _)| *s == g) {
            Some((_, v)) => v.push(p.key.clone()),
            None if is_effect_group(&g) => {}
            None => out.push((g, vec![p.key.clone()])),
        }
    }
    out.retain(|(_, v)| !v.is_empty());
    out
}

fn is_effect_group(g: &str) -> bool {
    g.starts_with("Effect ") || MODULES.iter().any(|(_, n)| g.starts_with(n))
}

/// Which oscillator section a parameter goes in.
fn osc_section(key: &str) -> &'static str {
    let n = key.trim_start_matches("OSC/");
    if n.contains("Mod") || n.contains("Src") || n.contains("Depth") {
        "Oscillator modulation"
    } else if n.ends_with("On") || n.starts_with("Digital") || n.starts_with("Eco") || n.ends_with("Shape") || n == "ShapeModel" || n == "RingmodPulse" {
        "Oscillator waves"
    } else {
        "Oscillators"
    }
}

/// Every parameter key a Main card shows (tests, hints).
pub fn main_keys(c: &Ctl) -> Vec<&'static str> {
    match c {
        Ctl::Knob { key, .. } | Ctl::Pills { key, .. } | Ctl::Menu { key, .. } => vec![key],
        Ctl::Macro(mc) => mc.keys().to_vec(),
        Ctl::Switches(v) => v.iter().map(|(k, _)| *k).collect(),
        Ctl::MenuPair { keys, .. } => keys.to_vec(),
        Ctl::EnvAmount => vec!["VCF1/FreqModDepth"],
        Ctl::LfoDepth(_) => vec![],
    }
}
