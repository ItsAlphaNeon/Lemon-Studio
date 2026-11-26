// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Main page's two macro knobs: editor logic that writes real parameters (no parameters of their own, nothing
//! saved). Each shows a value derived from the parameters it moves.
//!
//! - **Drift** moves OPT's five slop amounts together, keeping their ratios; shown as the largest of them.
//! - **Detune** spreads oscillators 2 and 3 symmetrically around oscillator 1 (Triple VCO): Tune 2 = its whole
//!   semitones + d, Tune 3 = its whole semitones - d. The whole-semitone parts stay; shown as d.

use crate::params::{Model, ParamTable};

/// The slop amounts Drift moves.
pub const SLOPS: [&str; 5] = ["OPT/TuneSlop", "OPT/CutoffSlop", "OPT/GlideSlop", "OPT/PWSlop", "OPT/EnvrateSlop"];
/// Drift's range (the slops' own range).
pub const DRIFT_MAX: f32 = 100.0;

/// Detune's range: half a semitone either way (the whole-semitone parts never change).
pub const DETUNE_MAX: f32 = 0.49;
pub const TUNE2: &str = "OSC/Tune2";
pub const TUNE3: &str = "OSC/Tune3";

/// A macro knob on the Main page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Macro {
    Drift,
    Detune,
}

impl Macro {
    pub fn name(self) -> &'static str {
        match self {
            Macro::Drift => "Drift",
            Macro::Detune => "Detune",
        }
    }

    /// The parameters it writes.
    pub fn keys(self) -> &'static [&'static str] {
        match self {
            Macro::Drift => &SLOPS,
            Macro::Detune => &[TUNE2, TUNE3],
        }
    }

    pub fn bipolar(self) -> bool {
        self == Macro::Detune
    }

    /// The shown value as 0..1.
    pub fn norm(self, m: &dyn Model) -> f32 {
        match self {
            Macro::Drift => drift(m) / DRIFT_MAX,
            Macro::Detune => (detune(m) / DETUNE_MAX + 1.0) / 2.0,
        }
    }

    pub fn text(self, m: &dyn Model) -> String {
        match self {
            Macro::Drift => format!("{:.1}", drift(m)),
            Macro::Detune => format!("{:+.1} ct", detune(m) * 100.0),
        }
    }

    /// The value a 0..1 position stands for.
    pub fn value_of(self, norm: f32) -> f32 {
        match self {
            Macro::Drift => norm.clamp(0.0, 1.0) * DRIFT_MAX,
            Macro::Detune => (norm.clamp(0.0, 1.0) * 2.0 - 1.0) * DETUNE_MAX,
        }
    }

    /// The default value (double click): the slops' defaults, no detune.
    pub fn default_norm(self, t: &ParamTable) -> f32 {
        match self {
            Macro::Drift => SLOPS.iter().filter_map(|k| t.get(k)).map(|p| p.default).fold(0.0, f32::max) / DRIFT_MAX,
            Macro::Detune => 0.5,
        }
    }

    /// Typed text to a value.
    pub fn parse(self, text: &str) -> Option<f32> {
        let num: String = text.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | ',')).collect();
        let v = num.replace(',', ".").parse::<f32>().ok()?;
        Some(match self {
            Macro::Drift => v.clamp(0.0, DRIFT_MAX),
            Macro::Detune => (v / 100.0).clamp(-DETUNE_MAX, DETUNE_MAX),
        })
    }

    /// Starts a gesture: what the drag keeps fixed (the ratios, the whole semitones).
    pub fn begin(self, m: &mut dyn Model, t: &ParamTable) -> MacroDrag {
        for k in self.keys() {
            m.begin(k);
        }
        match self {
            Macro::Drift => MacroDrag::Drift(drift_ratios(m, t)),
            Macro::Detune => MacroDrag::Detune(m.get(TUNE2).round(), m.get(TUNE3).round()),
        }
    }

    pub fn end(self, m: &mut dyn Model) {
        for k in self.keys() {
            m.end(k);
        }
    }
}

/// What a macro gesture keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MacroDrag {
    /// Each slop's share of the largest.
    Drift([f32; 5]),
    /// Tune 2's and Tune 3's whole semitones.
    Detune(f32, f32),
}

impl MacroDrag {
    /// Sets the macro to `value` (Drift 0..100, Detune in semitones).
    pub fn set(&self, m: &mut dyn Model, value: f32) {
        match *self {
            MacroDrag::Drift(r) => {
                let v = value.clamp(0.0, DRIFT_MAX);
                for (k, r) in SLOPS.iter().zip(r) {
                    m.set(k, (r * v).clamp(0.0, DRIFT_MAX));
                }
            }
            MacroDrag::Detune(b2, b3) => {
                let d = value.clamp(-DETUNE_MAX, DETUNE_MAX);
                m.set(TUNE2, (b2 + d).clamp(-30.0, 30.0));
                m.set(TUNE3, (b3 - d).clamp(-30.0, 30.0));
            }
        }
    }
}

/// Drift as shown: the largest slop.
pub fn drift(m: &dyn Model) -> f32 {
    SLOPS.iter().map(|k| m.get(k)).fold(0.0, f32::max)
}

/// The slops' shares of the largest; the defaults' when all are 0.
fn drift_ratios(m: &dyn Model, t: &ParamTable) -> [f32; 5] {
    let cur: [f32; 5] = std::array::from_fn(|i| m.get(SLOPS[i]).max(0.0));
    let max = cur.iter().copied().fold(0.0, f32::max);
    if max > 1e-6 {
        return cur.map(|v| v / max);
    }
    let def: [f32; 5] = std::array::from_fn(|i| t.get(SLOPS[i]).map_or(0.0, |p| p.default.max(0.0)));
    let dmax = def.iter().copied().fold(0.0, f32::max);
    if dmax > 1e-6 {
        def.map(|v| v / dmax)
    } else {
        [1.0; 5]
    }
}

/// Detune as shown: half the spread of Tune 2's and Tune 3's fractions (Tune 2 up, Tune 3 down).
pub fn detune(m: &dyn Model) -> f32 {
    let f2 = m.get(TUNE2) - m.get(TUNE2).round();
    let f3 = m.get(TUNE3) - m.get(TUNE3).round();
    ((f2 - f3) / 2.0).clamp(-DETUNE_MAX, DETUNE_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::testing::table;
    use crate::params::MemModel;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn drift_scales_the_slops_keeping_their_ratios() {
        let t = table();
        let mut mm = MemModel::new(&t);
        for (k, v) in SLOPS.iter().zip([10.0, 20.0, 40.0, 5.0, 0.0]) {
            mm.values.insert(k.to_string(), v);
        }
        let m: &mut dyn Model = &mut mm;
        assert_eq!(drift(m), 40.0, "shown as the largest");
        assert!(approx(Macro::Drift.norm(m), 0.4));
        let g = Macro::Drift.begin(m, &t);
        g.set(m, 80.0);
        let got: Vec<f32> = SLOPS.iter().map(|k| m.get(k)).collect();
        assert_eq!(got, vec![20.0, 40.0, 80.0, 10.0, 0.0]);
        // Down to 0 and back up within one gesture: the ratios survive.
        g.set(m, 0.0);
        assert_eq!(drift(m), 0.0);
        g.set(m, 20.0);
        let got: Vec<f32> = SLOPS.iter().map(|k| m.get(k)).collect();
        assert_eq!(got, vec![5.0, 10.0, 20.0, 2.5, 0.0]);
        Macro::Drift.end(m);
        assert_eq!(mm.gestures.len(), 10, "begin and end on each of the five");
    }

    #[test]
    fn drift_from_zero_takes_the_default_ratios() {
        let t = table();
        let mut mm = MemModel::new(&t);
        for k in SLOPS {
            mm.values.insert(k.to_string(), 0.0);
        }
        let m: &mut dyn Model = &mut mm;
        let g = Macro::Drift.begin(m, &t);
        g.set(m, 66.0);
        // defaults 25, 16, 24, 33, 26: the largest (PWSlop) lands on 66
        let got: Vec<f32> = SLOPS.iter().map(|k| m.get(k)).collect();
        for (g, d) in got.iter().zip([25.0, 16.0, 24.0, 33.0, 26.0]) {
            assert!(approx(*g, d * 2.0), "{got:?}");
        }
        assert!(approx(Macro::Drift.default_norm(&t), 0.33));
    }

    #[test]
    fn detune_spreads_tune_2_and_3_around_their_semitones() {
        let t = table();
        let mut mm = MemModel::new(&t);
        mm.values.insert(TUNE2.into(), 7.05);
        mm.values.insert(TUNE3.into(), -12.08);
        let m: &mut dyn Model = &mut mm;
        assert!(approx(detune(m), 0.065), "half of 0.05 - (-0.08)");
        let g = Macro::Detune.begin(m, &t);
        assert_eq!(g, MacroDrag::Detune(7.0, -12.0));
        g.set(m, 0.2);
        assert!(approx(m.get(TUNE2), 7.2) && approx(m.get(TUNE3), -12.2));
        assert!(approx(detune(m), 0.2));
        g.set(m, -0.3);
        assert!(approx(m.get(TUNE2), 6.7) && approx(m.get(TUNE3), -11.7), "negative: Tune 2 down, Tune 3 up");
        assert!(approx(detune(m), -0.3));
        g.set(m, 5.0);
        assert!(approx(m.get(TUNE2), 7.0 + DETUNE_MAX), "never reaches the next semitone");
        Macro::Detune.end(m);
        assert!(approx(Macro::Detune.norm(m), 1.0));
        assert_eq!(Macro::Detune.text(m), "+49.0 ct");
        assert_eq!(Macro::Detune.parse("-25"), Some(-0.25));
        assert!(approx(Macro::Detune.value_of(0.75), DETUNE_MAX / 2.0));
    }
}
