// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The editor's view of the parameters: metadata the plugin hands over once ([`ParamTable`]) and the live values
//! behind [`Model`]. Keys are Zest's parameter keys (`module/name`, e.g. `VCF1/Frequency`); values are plain values in
//! the parameter's own units (a choice's value is its index from `min`).

use std::collections::HashMap;

/// One parameter as the editor knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamInfo {
    pub key: String,
    /// Display name ("Frequency"); the hint line puts the card's name before it where that helps.
    pub name: String,
    /// The section it belongs to ("Filter", "Envelope 1", "Chorus 1", ...): the Advanced page's cards.
    pub group: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Whole steps only (choices, switches, counts).
    pub stepped: bool,
    /// Names of the steps from `min` (choices); empty for numbers.
    pub labels: Vec<String>,
    /// Kept in presets but never shown.
    pub hidden: bool,
}

impl ParamInfo {
    /// A plain value to 0..1.
    pub fn norm(&self, plain: f32) -> f32 {
        if self.max > self.min {
            ((plain - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// 0..1 to a plain value (whole steps for stepped parameters).
    pub fn plain(&self, norm: f32) -> f32 {
        let v = self.min + (self.max - self.min) * norm.clamp(0.0, 1.0);
        if self.stepped {
            v.round()
        } else {
            v
        }
    }

    /// Number of positions of a stepped parameter (0 for continuous ones).
    pub fn steps(&self) -> usize {
        if self.stepped {
            (self.max - self.min).round() as usize + 1
        } else {
            0
        }
    }

    /// A switch: two steps, "Off/On" or "0/1".
    pub fn is_switch(&self) -> bool {
        self.steps() == 2 && (self.labels.is_empty() || self.labels.iter().map(String::as_str).eq(["Off", "On"]) || self.labels.iter().map(String::as_str).eq(["0", "1"]))
    }

    /// A choice with named steps (not a switch, not a plain count like "1".."4").
    pub fn is_choice(&self) -> bool {
        self.stepped && !self.labels.is_empty() && !self.is_switch()
    }

    /// The text for a plain value: its label, or the number.
    pub fn format(&self, plain: f32) -> String {
        if self.stepped {
            let i = (plain - self.min).round() as isize;
            if let Some(l) = usize::try_from(i).ok().and_then(|i| self.labels.get(i)) {
                return l.clone();
            }
            return format!("{}", plain.round() as i64);
        }
        let span = self.max - self.min;
        if span <= 2.5 {
            format!("{plain:.3}")
        } else if span <= 30.0 {
            format!("{plain:.2}")
        } else {
            format!("{plain:.1}")
        }
    }

    /// Parses typed text: a label (any case), else a number.
    pub fn parse(&self, text: &str) -> Option<f32> {
        let t = text.trim();
        if let Some(i) = self.labels.iter().position(|l| l.eq_ignore_ascii_case(t)) {
            return Some(self.min + i as f32);
        }
        let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | ',')).collect();
        num.replace(',', ".").parse::<f32>().ok().map(|v| if self.stepped { v.round() } else { v }.clamp(self.min, self.max))
    }
}

/// Every parameter, in the plugin's order, found by key.
#[derive(Clone, Debug, Default)]
pub struct ParamTable {
    infos: Vec<ParamInfo>,
    index: HashMap<String, usize>,
}

impl ParamTable {
    pub fn new(infos: Vec<ParamInfo>) -> Self {
        let index = infos.iter().enumerate().map(|(i, p)| (p.key.clone(), i)).collect();
        ParamTable { infos, index }
    }

    pub fn get(&self, key: &str) -> Option<&ParamInfo> {
        self.index.get(key).map(|&i| &self.infos[i])
    }

    pub fn has(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    pub fn all(&self) -> &[ParamInfo] {
        &self.infos
    }

    /// The parameter's position in the plugin's list.
    pub fn position(&self, key: &str) -> Option<usize> {
        self.index.get(key).copied()
    }
}

/// Live parameter values, as the plugin holds them. Edits come as gestures: `begin`, any number of `set`, `end`.
pub trait Model {
    /// The plain value shown now (the editor's own pending edit included).
    fn get(&self, key: &str) -> f32;
    /// Sets a plain value inside a gesture.
    fn set(&mut self, key: &str, plain: f32);
    fn begin(&mut self, _key: &str) {}
    fn end(&mut self, _key: &str) {}
    /// The plugin's own text for a value, when it has one (units, rounding).
    fn text(&self, _key: &str, _plain: f32) -> Option<String> {
        None
    }
}

impl dyn Model + '_ {
    /// One complete edit.
    pub fn edit(&mut self, key: &str, plain: f32) {
        self.begin(key);
        self.set(key, plain);
        self.end(key);
    }

    /// A stepped value as an integer.
    pub fn int(&self, key: &str) -> i32 {
        self.get(key).round() as i32
    }

    pub fn on(&self, key: &str) -> bool {
        self.get(key) >= 0.5
    }
}

/// Values in memory (tests, previews): starts at the defaults; records every gesture.
#[derive(Clone, Debug, Default)]
pub struct MemModel {
    pub values: HashMap<String, f32>,
    /// (key, began) for every `begin` / `end`, in order.
    pub gestures: Vec<(String, bool)>,
}

impl MemModel {
    pub fn new(table: &ParamTable) -> Self {
        MemModel { values: table.all().iter().map(|p| (p.key.clone(), p.default)).collect(), gestures: Vec::new() }
    }
}

impl Model for MemModel {
    fn get(&self, key: &str) -> f32 {
        self.values.get(key).copied().unwrap_or(0.0)
    }
    fn set(&mut self, key: &str, plain: f32) {
        if let Some(v) = self.values.get_mut(key) {
            *v = plain;
        }
    }
    fn begin(&mut self, key: &str) {
        self.gestures.push((key.into(), true));
    }
    fn end(&mut self, key: &str) {
        self.gestures.push((key.into(), false));
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    fn p(key: &str, group: &str, min: f32, max: f32, default: f32, labels: &[&str]) -> ParamInfo {
        let name = key.split('/').nth(1).unwrap_or(key).to_string();
        ParamInfo { key: key.into(), name, group: group.into(), min, max, default, stepped: !labels.is_empty(), labels: labels.iter().map(|s| s.to_string()).collect(), hidden: false }
    }

    fn num(key: &str, group: &str, min: f32, max: f32, default: f32) -> ParamInfo {
        p(key, group, min, max, default, &[])
    }

    const SRC: &[&str] = &[
        "none", "ModWhl", "PitchW", "CtrlA", "CtrlB", "Gate", "Velocity", "Pressure", "KeyFollow", "KeyFollow2", "Alternate", "Random", "StackVoice", "VoiceMap", "Env1", "Env2",
        "LFO1", "LFO2", "Rectify", "Invert", "Quant", "Lag", "Adder", "Multiply",
    ];

    /// A table shaped like Zest's (the keys the editor's pages name, plus a hidden one).
    pub fn table() -> ParamTable {
        let mut v = vec![
            num("main/Output", "Main", 0.0, 200.0, 74.0),
            p("main/Active #FX1", "Main", 0.0, 1.0, 0.0, &["Off", "On"]),
            p("main/Active #FX2", "Main", 0.0, 1.0, 0.0, &["Off", "On"]),
            p("VCC/Voices", "Voice", 0.0, 7.0, 4.0, &["2", "3", "4", "5", "6", "8", "12", "16"]),
            p("VCC/Mode", "Voice", 0.0, 4.0, 0.0, &["poly", "mono", "legato", "duo", "poly2"]),
            p("VCC/GlideMode", "Voice", 0.0, 1.0, 0.0, &["time", "rate"]),
            num("VCC/Glide", "Voice", 0.0, 100.0, 8.0),
            p("VCC/PitchBend Up", "Voice", 0.0, 3.0, 2.0, &["0", "1", "2", "3"]),
            p("VCC/PitchBend Down", "Voice", 0.0, 3.0, 2.0, &["0", "1", "2", "3"]),
            num("OPT/TuneSlop", "Quality and spread", 0.0, 100.0, 25.0),
            num("OPT/CutoffSlop", "Quality and spread", 0.0, 100.0, 16.0),
            num("OPT/GlideSlop", "Quality and spread", 0.0, 100.0, 24.0),
            num("OPT/PWSlop", "Quality and spread", 0.0, 100.0, 33.0),
            num("OPT/EnvrateSlop", "Quality and spread", 0.0, 100.0, 26.0),
            p("OSC/Model", "Oscillators", 0.0, 4.0, 0.0, &["Triple VCO", "Dual VCO", "DCO", "Dual VCO Eco", "Digital"]),
            num("OSC/Tune1", "Oscillators", -30.0, 30.0, 0.0),
            num("OSC/Tune2", "Oscillators", -30.0, 30.0, 0.0),
            num("OSC/Tune3", "Oscillators", -30.0, 30.0, 0.0),
            num("OSC/Vibrato", "Oscillators", 0.0, 100.0, 50.0),
            num("OSC/Volume3", "Oscillators", 0.0, 100.0, 0.0),
            num("OSC/OscMix", "Oscillators", 0.0, 100.0, 0.0),
            p("VCF1/Model", "Filter", 0.0, 4.0, 0.0, &["Ladder", "Cascade", "Multimode", "BITE", "Uhbie"]),
            num("VCF1/Frequency", "Filter", 30.0, 150.0, 41.0),
            p("VCF1/FreqModSrc", "Filter", 0.0, 23.0, 21.0, SRC),
            num("VCF1/FreqModDepth", "Filter", -120.0, 120.0, 120.0),
            p("VCA1/VCA", "Amp", 0.0, 1.0, 1.0, &["Gate", "Env1"]),
            num("VCA1/Volume", "Amp", 0.0, 100.0, 48.5),
            p("VCA1/Modulation", "Amp", 0.0, 23.0, 16.0, SRC),
            num("VCA1/ModDepth", "Amp", -100.0, 100.0, -14.0),
            p("MOD/LagSource", "Modifiers", 0.0, 23.0, 15.0, SRC),
            num("ENV1/Attack", "Envelope 1", 0.0, 100.0, 15.0),
            num("ENV2/Attack", "Envelope 2", 0.0, 100.0, 0.0),
            p("FX1/Module", "Effect 1", 0.0, 4.0, 0.0, &["Chorus1", "Phaser1", "Plate1", "Delay1", "Rotary1"]),
            num("Chrs1/Rate", "Chorus 1", 0.0, 100.0, 12.0),
            p("Chrs1/Type", "Chorus 1", 0.0, 2.0, 1.0, &["Classic", "Dramatic", "Ensemble"]),
            num("Phase1/Rate", "Phaser 1", 0.0, 100.0, 50.0),
            p("FX2/Module", "Effect 2", 0.0, 4.0, 2.0, &["Chorus2", "Phaser2", "Plate2", "Delay2", "Rotary2"]),
            num("Plate2/Decay", "Plate 2", 0.0, 100.0, 45.0),
        ];
        let mut hidden = num("VCC/Vc1", "Voice", -48.0, 48.0, 0.0);
        hidden.hidden = true;
        v.push(hidden);
        ParamTable::new(v)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::table;
    use super::*;

    #[test]
    fn norm_plain_format_parse() {
        let t = table();
        let f = t.get("VCF1/Frequency").unwrap();
        assert_eq!(f.norm(90.0), 0.5);
        assert_eq!(f.plain(0.5), 90.0);
        assert_eq!(f.format(90.0), "90.0");
        assert_eq!(f.parse("75.5"), Some(75.5));
        assert_eq!(f.parse("999"), Some(150.0), "clamped");
        let m = t.get("VCC/Mode").unwrap();
        assert!(m.is_choice() && !m.is_switch());
        assert_eq!(m.steps(), 5);
        assert_eq!(m.format(2.0), "legato");
        assert_eq!(m.parse("DUO"), Some(3.0));
        assert_eq!(m.plain(0.3), 1.0, "stepped values round");
        assert!(t.get("main/Active #FX1").unwrap().is_switch());
        assert_eq!(t.position("VCC/Voices"), Some(3));
    }

    #[test]
    fn mem_model_records_gestures() {
        let t = table();
        let mut m = MemModel::new(&t);
        assert_eq!(m.get("VCF1/Frequency"), 41.0);
        let dm: &mut dyn Model = &mut m;
        dm.edit("VCF1/Frequency", 80.0);
        assert_eq!(dm.get("VCF1/Frequency"), 80.0);
        assert_eq!(m.gestures, vec![("VCF1/Frequency".into(), true), ("VCF1/Frequency".into(), false)]);
    }
}
