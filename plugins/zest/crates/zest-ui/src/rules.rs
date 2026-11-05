// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Which controls a preset's choices make pointless ("veiled": drawn disabled, never hidden, still editable), the
//! modulation routes (which source feeds which depth), and the titles that follow the preset (the amp envelope reads
//! "Gate" when the VCA follows the gate).
//!
//! This is knowledge about Zest's parameter surface (which parameter matters for which oscillator or filter model),
//! not engine code: it names parameters by key only.

use crate::params::Model;

/// OSC/Model values.
pub const TRIPLE: i32 = 0;
pub const DUAL: i32 = 1;
pub const DCO: i32 = 2;
pub const ECO: i32 = 3;
pub const DIGITAL: i32 = 4;

/// VCF1/Model values.
pub const LADDER: i32 = 0;
pub const CASCADE: i32 = 1;
pub const MULTIMODE: i32 = 2;
pub const BITE: i32 = 3;
pub const UHBIE: i32 = 4;

/// HPF/Model values.
pub const HPF_FEEDBACK: i32 = 0;
pub const HPF_POST: i32 = 1;
pub const HPF_PRE: i32 = 2;
pub const HPF_BITE: i32 = 3;

/// Modulation source indices (the value of every source selector).
pub mod src {
    pub const NONE: i32 = 0;
    pub const ENV1: i32 = 14;
    pub const ENV2: i32 = 15;
    pub const LFO1: i32 = 16;
    pub const LFO2: i32 = 17;
    pub const RECTIFY: i32 = 18;
    pub const INVERT: i32 = 19;
    pub const QUANT: i32 = 20;
    pub const LAG: i32 = 21;
    pub const ADDER: i32 = 22;
    pub const MULTIPLY: i32 = 23;
}

/// Short source names for captions.
pub const SOURCE_SHORT: [&str; 24] = [
    "—", "MOD WHL", "BEND", "CTRL A", "CTRL B", "GATE", "VELO", "PRESS", "KEY", "KEY 2", "ALT", "RANDOM", "STACK", "VMAP", "ENV 1", "ENV 2", "LFO 1", "LFO 2",
    "RECT", "INVERT", "QUANT", "LAG", "ADD", "MULT",
];

/// A modulation route: the source selector, its depth, and what it moves (caption text).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Route {
    pub src: &'static str,
    pub depth: Option<&'static str>,
    pub target: &'static str,
}

const fn r(src: &'static str, depth: &'static str, target: &'static str) -> Route {
    Route { src, depth: Some(depth), target }
}

/// Every source selector with a depth of its own, in the order the LFO card lists them.
pub const ROUTES: &[Route] = &[
    r("VCF1/FreqModSrc", "VCF1/FreqModDepth", "CUTOFF"),
    r("VCF1/FreqMod2Src", "VCF1/FreqMod2Depth", "CUTOFF"),
    r("OSC/Tune1ModSrc", "OSC/Tune1ModDepth", "PITCH"),
    r("OSC/Tune2ModSrc", "OSC/Tune2ModDepth", "PITCH"),
    r("OSC/PWModSrc", "OSC/PWModDepth", "PW"),
    r("OSC/ShapeSrc", "OSC/ShapeDepth", "SHAPE"),
    r("OSC/FmModSrc", "OSC/FmModDepth", "FM"),
    r("OSC/NoiseVolModSrc", "OSC/NoiseVolModDepth", "NOISE"),
    r("VCF1/ResModSrc", "VCF1/ResModDepth", "RESO"),
    r("VCF1/FmAmountModSrc", "VCF1/FmAmountModDepth", "FILTER FM"),
    r("VCF1/FeedbackModSrc", "VCF1/FeedbackModDepth", "FEEDBACK"),
    r("VCF1/ShapeModSrc", "VCF1/ShapeModDepth", "SHAPE"),
    r("HPF/FreqModSrc", "HPF/FreqModDepth", "HP CUT"),
    r("VCA1/Modulation", "VCA1/ModDepth", "VOLUME"),
    r("VCA1/PanModulation", "VCA1/PanModDepth", "PAN"),
    r("LFO1/DepthMod Src1", "LFO1/DepthMod Dpt1", "LFO 1 AMT"),
    r("LFO1/FreqMod Src1", "LFO1/FreqMod Dpt", "LFO 1 RATE"),
    r("LFO2/DepthMod Src1", "LFO2/DepthMod Dpt1", "LFO 2 AMT"),
    r("LFO2/FreqMod Src1", "LFO2/FreqMod Dpt", "LFO 2 RATE"),
    r("MTX/Src1", "MTX/Amt1", "MATRIX 1"),
    r("MTX/Src2", "MTX/Amt2", "MATRIX 2"),
    r("MTX/Src3", "MTX/Amt3", "MATRIX 3"),
    r("MTX/Src4", "MTX/Amt4", "MATRIX 4"),
];

/// The modifiers' input selectors (source index of the modifier they feed).
pub const MODIFIER_INPUTS: &[(&str, i32)] = &[
    ("MOD/RectifySource", src::RECTIFY),
    ("MOD/InvertSource", src::INVERT),
    ("MOD/QuantiseSource", src::QUANT),
    ("MOD/LagSource", src::LAG),
    ("MOD/AddSource1", src::ADDER),
    ("MOD/AddSource2", src::ADDER),
    ("MOD/MulSource1", src::MULTIPLY),
    ("MOD/MulSource2", src::MULTIPLY),
];

/// Whether a route does something (a source and a depth).
fn active(m: &dyn Model, r: &Route) -> bool {
    m.int(r.src) != src::NONE && r.depth.is_none_or(|d| m.get(d).abs() > 1e-6)
}

/// Whether modulation source `s` reaches anything: directly through an active route, through a modifier that reaches
/// something, as the VCA's envelope (Env1) or as vibrato (LFO1).
pub fn source_used(m: &dyn Model, s: i32) -> bool {
    source_used_depth(m, s, 0)
}

fn source_used_depth(m: &dyn Model, s: i32, depth: usize) -> bool {
    if depth > 6 {
        return false;
    }
    if s == src::ENV1 && m.int("VCA1/VCA") == 1 {
        return true;
    }
    if s == src::LFO1 && m.get("OSC/Vibrato") > 1e-6 {
        return true;
    }
    if ROUTES.iter().any(|r| m.int(r.src) == s && active(m, r)) {
        return true;
    }
    MODIFIER_INPUTS.iter().any(|(k, modi)| m.int(k) == s && source_used_depth(m, *modi, depth + 1))
}

/// A source as its caption: single-input modifiers name what they pass on ("LAG ENV 2").
pub fn source_caption(m: &dyn Model, s: i32) -> String {
    let name = SOURCE_SHORT.get(s.clamp(0, 23) as usize).copied().unwrap_or("—");
    let input = match s {
        src::RECTIFY => Some("MOD/RectifySource"),
        src::INVERT => Some("MOD/InvertSource"),
        src::QUANT => Some("MOD/QuantiseSource"),
        src::LAG => Some("MOD/LagSource"),
        _ => None,
    };
    match input.map(|k| m.int(k)) {
        Some(i) if (src::ENV1..=src::LFO2).contains(&i) || (1..=13).contains(&i) => format!("{name} {}", SOURCE_SHORT[i as usize]),
        _ => name.to_string(),
    }
}

/// The routes LFO 1 drives now (active ones), vibrato first.
pub fn lfo1_routes(m: &dyn Model) -> Vec<Route> {
    ROUTES.iter().filter(|r| m.int(r.src) == src::LFO1 && active(m, r)).copied().collect()
}

/// The amp envelope card's title and whether it is veiled: "Amp envelope" when the VCA follows Env1; "Gate" (veiled)
/// when it follows the gate and Env1 drives nothing else; "Envelope 1" when it follows the gate but Env1 still
/// modulates something.
pub fn amp_env_title(m: &dyn Model) -> (&'static str, bool) {
    if m.int("VCA1/VCA") == 1 {
        ("AMP ENVELOPE", false)
    } else if source_used(m, src::ENV1) {
        ("ENVELOPE 1", false)
    } else {
        ("GATE", true)
    }
}

fn osc(m: &dyn Model) -> i32 {
    m.int("OSC/Model")
}

fn zero(m: &dyn Model, k: &str) -> bool {
    m.get(k).abs() < 1e-6
}

/// The noise-volume modulation (it moves OscMix in the Dual VCO and Digital models).
fn noise_mod(m: &dyn Model) -> bool {
    m.int("OSC/NoiseVolModSrc") != src::NONE && !zero(m, "OSC/NoiseVolModDepth")
}

/// Whether the oscillator model uses the parameter at all.
pub fn osc_uses(model: i32, key: &str) -> bool {
    let Some(name) = key.strip_prefix("OSC/") else { return true };
    // Every model: tune 1, vibrato, drift, the first tune modulation.
    if matches!(name, "Model" | "Tune1" | "Vibrato" | "Drift" | "Tune1ModSrc" | "Tune1ModDepth") {
        return true;
    }
    let digital = name.starts_with("Digital");
    match model {
        TRIPLE => matches!(
            name,
            "Tune2" | "Tune3" | "Shape1" | "Shape2" | "Shape3" | "Volume1" | "Volume2" | "Volume3" | "NoiseVol" | "NoiseColor" | "NoiseVolModSrc" | "NoiseVolModDepth" | "FM" | "FmModSrc" | "FmModDepth"
                | "Sync2" | "Sync3" | "TuneModOsc1" | "TuneModOsc2" | "TuneModOsc3" | "ShapeSrc" | "ShapeDepth" | "ShapeModOsc1" | "ShapeModOsc2" | "ShapeModOsc3"
        ),
        DUAL => matches!(
            name,
            "Tune2" | "Triangle1On" | "Saw1On" | "Pwm1On" | "Noise1On" | "Sine2On" | "Triangle2On" | "Saw2On" | "Pulse2On" | "PWM2On" | "PulseWidth" | "PWModSrc" | "PWModDepth" | "OscMix"
                | "NoiseVolModSrc" | "NoiseVolModDepth" | "FM" | "FmModSrc" | "FmModDepth" | "Sync2" | "ShapeModel" | "Tune2ModSrc" | "Tune2ModDepth" | "TuneModMode"
        ),
        DCO => matches!(
            name,
            "Tune2ModSrc" | "Tune2ModDepth" | "PulseShape" | "SawShape" | "SuboscShape" | "Volume3" | "NoiseVol" | "NoiseVolModSrc" | "NoiseVolModDepth" | "PulseWidth" | "PWModSrc" | "PWModDepth"
        ),
        ECO => matches!(name, "Tune2" | "Tune2ModSrc" | "Tune2ModDepth" | "EcoWave1" | "EcoWave2" | "Volume1" | "Volume2" | "NoiseVolModSrc" | "NoiseVolModDepth" | "PulseWidth" | "RingmodPulse" | "Shape1"),
        DIGITAL => {
            digital
                || matches!(
                    name,
                    "Tune2" | "Tune2ModSrc" | "Tune2ModDepth" | "TuneModOsc1" | "TuneModOsc2" | "PulseWidth" | "PWModSrc" | "PWModDepth" | "Pwm1On" | "PWM2On" | "ShapeSrc" | "ShapeDepth" | "ShapeModOsc1"
                        | "ShapeModOsc2" | "OscMix" | "NoiseVolModSrc" | "NoiseVolModDepth" | "FM" | "FmModSrc" | "FmModDepth" | "Sync2" | "RingmodPulse"
                )
        }
        _ => true,
    }
}

/// Whether the filter model uses the parameter.
pub fn vcf_uses(model: i32, key: &str) -> bool {
    let Some(name) = key.strip_prefix("VCF1/") else { return true };
    match name {
        "LadderMode" => matches!(model, LADDER | CASCADE),
        "LadderColor" => model == CASCADE,
        "SvfMode" => model == MULTIMODE,
        "SlnKyRevision" => model == BITE,
        "ShapeMix" | "ShapeModSrc" | "ShapeModDepth" | "UhbieBandpass" => model == UHBIE,
        _ => true,
    }
}

/// Whether the high-pass model (with this oscillator model) uses the parameter. With the Triple VCO the high-pass
/// does nothing.
pub fn hpf_uses(osc_model: i32, model: i32, key: &str) -> bool {
    let Some(name) = key.strip_prefix("HPF/") else { return true };
    if name == "Model" {
        return osc_model != TRIPLE;
    }
    if osc_model == TRIPLE {
        return false;
    }
    match model {
        HPF_POST => name == "Post-HPF Freq",
        HPF_PRE => name == "Frequency",
        HPF_BITE => matches!(name, "Frequency" | "Resonance" | "Revision" | "KeyFollow" | "FreqModSrc" | "FreqModDepth"),
        _ => false,
    }
}

/// Whether a control is veiled for the preset now: its model ignores it, or other settings make it pointless.
pub fn veiled(m: &dyn Model, key: &str) -> bool {
    let om = osc(m);
    if !osc_uses(om, key) || !vcf_uses(m.int("VCF1/Model"), key) || !hpf_uses(om, m.int("HPF/Model"), key) {
        return true;
    }
    // A selector or depth whose partner is off.
    if let Some(r) = ROUTES.iter().find(|r| r.depth == Some(key)) {
        if m.int(r.src) == src::NONE {
            return true;
        }
    }
    let osc2_off = |m: &dyn Model| zero(m, "OSC/OscMix") && !noise_mod(m);
    match key {
        // Triple VCO: an oscillator at volume 0 (osc 1 still feeds FM, sync and filter FM)
        "OSC/Tune2" | "OSC/Shape2" | "OSC/Sync2" | "OSC/TuneModOsc2" | "OSC/ShapeModOsc2" if om == TRIPLE => zero(m, "OSC/Volume2"),
        "OSC/Tune3" | "OSC/Shape3" | "OSC/Sync3" | "OSC/TuneModOsc3" | "OSC/ShapeModOsc3" if om == TRIPLE => zero(m, "OSC/Volume3"),
        "OSC/Shape1" if om == TRIPLE => {
            zero(m, "OSC/Volume1") && zero(m, "OSC/FM") && !m.on("OSC/Sync2") && !m.on("OSC/Sync3") && zero(m, "VCF1/FilterFM")
        }
        "OSC/FM" | "OSC/FmModSrc" | "OSC/FmModDepth" if om == TRIPLE => zero(m, "OSC/Volume2") && zero(m, "OSC/Volume3"),
        "OSC/NoiseColor" => zero(m, "OSC/NoiseVol") && !noise_mod(m),
        "OSC/Tune1ModSrc" | "OSC/Tune1ModDepth" if om == TRIPLE => !(1..=3).any(|i| m.on(&format!("OSC/TuneModOsc{i}"))),
        "OSC/ShapeSrc" | "OSC/ShapeDepth" if om == TRIPLE => !(1..=3).any(|i| m.on(&format!("OSC/ShapeModOsc{i}"))),
        // Dual VCO
        "OSC/PulseWidth" | "OSC/PWModSrc" | "OSC/PWModDepth" if om == DUAL => !(m.on("OSC/Pwm1On") || (m.on("OSC/Pulse2On") && m.on("OSC/PWM2On"))),
        "OSC/PWM2On" if om == DUAL => !m.on("OSC/Pulse2On"),
        "OSC/Tune2" | "OSC/Sync2" | "OSC/FM" | "OSC/Sine2On" | "OSC/Triangle2On" | "OSC/Saw2On" | "OSC/Pulse2On" if om == DUAL => osc2_off(m),
        // DCO
        "OSC/SuboscShape" if om == DCO => zero(m, "OSC/Volume3"),
        "OSC/PulseWidth" | "OSC/PWModSrc" | "OSC/PWModDepth" if om == DCO => m.int("OSC/PulseShape") != 3 && m.int("OSC/SawShape") != 3,
        // Dual VCO Eco
        "OSC/Tune2" if om == ECO => zero(m, "OSC/Volume2"),
        "OSC/PulseWidth" if om == ECO => m.get("OSC/EcoWave1").round() != 3.0,
        "OSC/RingmodPulse" | "OSC/Shape1" if om == ECO => m.get("OSC/EcoWave2") < 3.5,
        // Digital
        "OSC/Tune2" | "OSC/Sync2" | "OSC/FM" | "OSC/RingmodPulse" | "OSC/DigitalType2" | "OSC/DigitalShape3" | "OSC/DigitalShape4" if om == DIGITAL => osc2_off(m),
        "OSC/PWModSrc" | "OSC/PWModDepth" if om == DIGITAL => !m.on("OSC/Pwm1On") && !m.on("OSC/PWM2On"),
        "OSC/ShapeSrc" | "OSC/ShapeDepth" if om == DIGITAL => !m.on("OSC/ShapeModOsc1") && !m.on("OSC/ShapeModOsc2"),
        "OSC/Tune2ModSrc" | "OSC/Tune2ModDepth" if om == DIGITAL => !m.on("OSC/TuneModOsc1") && !m.on("OSC/TuneModOsc2"),
        // The filter's feedback path runs with the Triple VCO or the Feedback high-pass only.
        "VCF1/Feedback" | "VCF1/FeedbackModSrc" | "VCF1/FeedbackModDepth" => om != TRIPLE && m.int("HPF/Model") != HPF_FEEDBACK,
        // Envelopes: the ADS model has no release knob; quantise and curve are the digital model's.
        "ENV1/Release" | "ENV2/Release" => m.int(&format!("{}/Model", &key[..4])) == 0,
        "ENV1/Release On" | "ENV2/Release On" => m.int(&format!("{}/Model", &key[..4])) != 0,
        "ENV1/Quantise" | "ENV1/Curve" | "ENV2/Quantise" | "ENV2/Curve" => m.int(&format!("{}/Model", &key[..4])) != 2,
        // Glide range and the second glide only matter with some glide.
        "VCC/Glide2" | "VCC/GlideRange" => zero(m, "VCC/Glide"),
        // One voice at a time in the mono modes.
        "VCC/Voices" => matches!(m.int("VCC/Mode"), 1 | 2),
        // The arpeggiator's settings while it is off.
        k if k.starts_with("ARP/") && k != "ARP/OnOff" => !m.on("ARP/OnOff"),
        // A matrix slot without a target or source.
        k if k.starts_with("MTX/Amt") => {
            let n = &k[7..];
            m.int(&format!("MTX/Src{n}")) == src::NONE || m.int(&format!("MTX/Dst{n}")) == 0
        }
        _ => false,
    }
}

/// Whether an envelope or LFO card (its source) drives nothing: veiled.
pub fn source_card_veiled(m: &dyn Model, s: i32) -> bool {
    !source_used(m, s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::MemModel;
    use std::collections::HashMap;

    fn model(pairs: &[(&str, f32)]) -> MemModel {
        let mut v: HashMap<String, f32> = HashMap::new();
        for (k, x) in [
            ("OSC/Model", 0.0),
            ("VCF1/Model", 0.0),
            ("HPF/Model", 0.0),
            ("VCA1/VCA", 1.0),
            ("OSC/Volume1", 100.0),
            ("OSC/Volume2", 100.0),
            ("OSC/Volume3", 0.0),
            ("OSC/Vibrato", 0.0),
            ("VCF1/FreqModSrc", 15.0),
            ("VCF1/FreqModDepth", 50.0),
        ] {
            v.insert(k.into(), x);
        }
        for (k, x) in pairs {
            v.insert(k.to_string(), *x);
        }
        MemModel { values: v, gestures: vec![] }
    }

    #[test]
    fn oscillator_models_veil_what_they_ignore() {
        let m = model(&[]);
        assert!(!veiled(&m, "OSC/Tune2"));
        assert!(veiled(&m, "OSC/Tune3"), "Triple VCO, osc 3 at volume 0");
        assert!(veiled(&m, "OSC/PulseWidth"), "the Triple VCO has no pulse width");
        assert!(veiled(&m, "OSC/Saw1On"));
        let dual = model(&[("OSC/Model", 1.0), ("OSC/Saw1On", 1.0), ("OSC/OscMix", 50.0)]);
        assert!(!veiled(&dual, "OSC/Saw1On") && !veiled(&dual, "OSC/Tune2"));
        assert!(veiled(&dual, "OSC/Shape1") && veiled(&dual, "OSC/Volume1") && veiled(&dual, "OSC/Tune3"));
        assert!(veiled(&dual, "OSC/PulseWidth"), "no pulse wave on");
        let dual_pwm = model(&[("OSC/Model", 1.0), ("OSC/Pwm1On", 1.0)]);
        assert!(!veiled(&dual_pwm, "OSC/PulseWidth"));
        assert!(veiled(&dual_pwm, "OSC/Tune2"), "OscMix 0: osc 2 silent");
        let dco = model(&[("OSC/Model", 2.0), ("OSC/PulseShape", 3.0)]);
        assert!(veiled(&dco, "OSC/Tune2") && !veiled(&dco, "OSC/PulseWidth") && veiled(&dco, "OSC/SuboscShape"));
        let eco = model(&[("OSC/Model", 3.0), ("OSC/EcoWave2", 4.0)]);
        assert!(!veiled(&eco, "OSC/RingmodPulse") && !veiled(&eco, "OSC/Shape1") && veiled(&eco, "OSC/Shape2"));
        let dig = model(&[("OSC/Model", 4.0), ("OSC/OscMix", 30.0)]);
        assert!(!veiled(&dig, "OSC/DigitalType2") && veiled(&dig, "OSC/Volume1"));
    }

    #[test]
    fn filter_and_high_pass_models() {
        let m = model(&[("VCF1/Model", 2.0)]);
        assert!(!veiled(&m, "VCF1/SvfMode") && veiled(&m, "VCF1/LadderMode") && veiled(&m, "VCF1/ShapeMix"));
        let u = model(&[("VCF1/Model", 4.0)]);
        assert!(!veiled(&u, "VCF1/ShapeMix") && veiled(&u, "VCF1/SvfMode"));
        assert!(veiled(&m, "HPF/Frequency"), "no high-pass with the Triple VCO");
        assert!(!veiled(&m, "VCF1/Feedback"), "feedback with the Triple VCO");
        let pre = model(&[("OSC/Model", 1.0), ("HPF/Model", 2.0)]);
        assert!(!veiled(&pre, "HPF/Frequency") && veiled(&pre, "HPF/Resonance") && veiled(&pre, "VCF1/Feedback"));
        let fb = model(&[("OSC/Model", 1.0), ("HPF/Model", 0.0)]);
        assert!(!veiled(&fb, "VCF1/Feedback") && veiled(&fb, "HPF/Frequency"));
    }

    #[test]
    fn gate_veils_the_amp_envelope_unless_env1_modulates() {
        let m = model(&[]);
        assert_eq!(amp_env_title(&m), ("AMP ENVELOPE", false));
        let g = model(&[("VCA1/VCA", 0.0)]);
        assert_eq!(amp_env_title(&g), ("GATE", true));
        let g1 = model(&[("VCA1/VCA", 0.0), ("VCF1/FreqModSrc", 14.0)]);
        assert_eq!(amp_env_title(&g1), ("ENVELOPE 1", false));
        // Env1 through the lag modifier into the cutoff.
        let lag = model(&[("VCA1/VCA", 0.0), ("VCF1/FreqModSrc", 21.0), ("MOD/LagSource", 14.0)]);
        assert_eq!(amp_env_title(&lag), ("ENVELOPE 1", false));
        assert_eq!(source_caption(&lag, 21), "LAG ENV 1");
        // A route with depth 0 drives nothing.
        let d0 = model(&[("VCA1/VCA", 0.0), ("VCF1/FreqModSrc", 14.0), ("VCF1/FreqModDepth", 0.0)]);
        assert_eq!(amp_env_title(&d0), ("GATE", true));
    }

    #[test]
    fn lfo_routes_and_use() {
        let m = model(&[]);
        assert!(source_card_veiled(&m, src::LFO1));
        let v = model(&[("OSC/Vibrato", 20.0)]);
        assert!(!source_card_veiled(&v, src::LFO1), "vibrato is LFO 1's");
        let c = model(&[("VCF1/FreqMod2Src", 16.0), ("VCF1/FreqMod2Depth", 10.0)]);
        assert_eq!(lfo1_routes(&c).iter().map(|r| r.target).collect::<Vec<_>>(), ["CUTOFF"]);
        assert!(veiled(&model(&[("VCF1/FreqModSrc", 0.0)]), "VCF1/FreqModDepth"));
    }
}
