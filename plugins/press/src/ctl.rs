// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The settings the editor's knobs and switches show, by parameter index: names, ranges, defaults and value texts.
//!
// spec: plugins/lemon-press.md §3, plugins/maximus.md ("Parameters", "Knob laws")

use os_engine::effects::maximus::state::*;
use os_engine::effects::maximus::{crossover_hz, gain, low_cut_hz, samples};

/// The setting's default (the factory default's).
pub fn default(k: usize) -> i32 {
    State::new_default().params[k]
}

/// The setting's value within its range (a state may hold values outside it; they play as stored).
pub fn shown(st: &State, k: usize) -> i32 {
    let (lo, hi) = range(k);
    st.params[k].clamp(lo, hi)
}

/// The knob position of a value, 0..1.
pub fn norm(k: usize, v: i32) -> f32 {
    let (lo, hi) = range(k);
    ((v - lo) as f32 / (hi - lo).max(1) as f32).clamp(0.0, 1.0)
}

/// Whether the knob's arc starts at its centre.
pub fn bipolar(k: usize) -> bool {
    k >= 7 && matches!((k - 7) % 13, B_SEP | B_THRES)
}

/// The band and the band parameter of index `k` (none for the global ones).
pub fn band_param(k: usize) -> Option<(usize, usize)> {
    (7..PARAMS).contains(&k).then(|| ((k - 7) / 13, (k - 7) % 13))
}

/// The hint's name of setting `k` (FL's hint names, measured).
pub fn name(k: usize) -> &'static str {
    match k {
        P_LOW_FREQ => "Low band frequency",
        P_LOW_SLOPE => "Low band slope",
        P_HIGH_FREQ => "High band frequency",
        P_HIGH_SLOPE => "High band slope",
        P_LMH_DELAY => "LMH lookahead delay",
        P_LMH_MIX => "LMH mix",
        P_LOW_CUT => "Low cut",
        _ => match (k - 7) % 13 {
            B_MODE => "Mode",
            B_PRE => "Pre gain",
            B_POST => "Post gain",
            B_SEP => "Stereo separation",
            B_THRES => "Soft saturation threshold",
            B_CEIL => "Soft saturation ceiling",
            B_ATT => "Attack time",
            B_REL2 => "Release 2 time",
            B_ATT_STAGES => "Attack / release 2 curve",
            B_REL => "Release time",
            B_REL_STAGES => "Release curve",
            B_WINDOW => "Sustain window",
            _ => "Sustain mode",
        },
    }
}

/// A gain as dB and as a factor: `0.0dB 1.00`, `-INFdB 0.00`.
pub fn gain_text(g: f64) -> String {
    let db = if g > 0.0 { format!("{:.1}", 20.0 * g.log10()) } else { "-INF".into() };
    format!("{db}dB {g:.2}")
}

/// A time knob in ms: the knob law at 1 kHz, release times twice as long (as FL's hints show them).
pub fn ms(k: usize, v: i32) -> f64 {
    let x = samples(v, 1000.0);
    match band_param(k) {
        Some((_, B_REL | B_REL2)) => 2.0 * x,
        _ => x,
    }
}

fn pct(v: i32) -> String {
    format!("{}%", (f64::from(v.abs()) / 10.0).round() as i64)
}

pub const MODE_NAMES: [&str; 4] = ["on", "compression off", "muted", "off"];

/// The value text of setting `k` at `v` (FL's hint values, measured).
pub fn text(k: usize, v: i32) -> String {
    match k {
        P_LOW_FREQ | P_HIGH_FREQ => format!("{}Hz", crossover_hz(v).round() as i64),
        P_LOW_SLOPE | P_HIGH_SLOPE => if v == 0 { "12dB" } else { "24dB" }.into(),
        P_LMH_DELAY => format!("{:.2}ms", ms(k, v)),
        P_LMH_MIX => pct(v),
        P_LOW_CUT => if v < 0 { "Off".into() } else { format!("{}Hz", low_cut_hz(v).round() as i64) },
        _ => match (k - 7) % 13 {
            B_MODE => MODE_NAMES[v.clamp(0, 3) as usize].into(),
            B_PRE | B_POST | B_CEIL => gain_text(f64::from(gain(v))),
            B_SEP => match v {
                0 => "Original".into(),
                v if v > 0 => format!("{} merged", pct(v)),
                v => format!("{} separated", pct(v)),
            },
            B_THRES => match v {
                0 => "No saturation".into(),
                v if v < 0 => format!("{} saturation mode A", pct(v)),
                v => format!("{} saturation mode B", pct(v)),
            },
            B_ATT | B_REL2 if v == 0 => "Default".into(),
            B_ATT_STAGES | B_REL_STAGES => v.to_string(),
            B_RMS => if v == 0 { "PEAK" } else { "RMS" }.into(),
            _ => format!("{:.2}ms", ms(k, v)),
        },
    }
}

/// The number value `v` of setting `k` shows before its text rounds it: the gains in dB, the separation and the
/// saturation signed (separated and mode A below 0), the others in their text's unit; none for a named value
/// ("Off", "Default", a mode, PEAK / RMS).
fn amount(k: usize, v: i32) -> Option<f64> {
    let f = f64::from(v);
    Some(match k {
        P_LOW_FREQ | P_HIGH_FREQ => f64::from(crossover_hz(v)),
        P_LOW_SLOPE | P_HIGH_SLOPE => if v == 0 { 12.0 } else { 24.0 },
        P_LMH_DELAY => ms(k, v),
        P_LMH_MIX => f.abs() / 10.0,
        P_LOW_CUT if v < 0 => return None,
        P_LOW_CUT => f64::from(low_cut_hz(v)),
        _ => match (k - 7) % 13 {
            B_MODE | B_RMS => return None,
            B_PRE | B_POST | B_CEIL => {
                let g = f64::from(gain(v));
                if g > 0.0 { 20.0 * g.log10() } else { -1000.0 }
            }
            B_SEP | B_THRES => f / 10.0,
            B_ATT | B_REL2 if v == 0 => return None,
            B_ATT_STAGES | B_REL_STAGES => f,
            _ => ms(k, v),
        },
    })
}

/// A typed value: a number in the text's unit (Hz, ms, percent, dB for the gains), or a named value's text ("off" for
/// the low cut, "Default" for the attack and release 2, a mode, PEAK / RMS). When several values show the typed text,
/// the current value `cur` stays, else the default is taken, else the one nearest the typed number before rounding.
pub fn parse(k: usize, s: &str, cur: i32) -> Option<i32> {
    let t = s.trim().to_ascii_lowercase();
    let (lo, hi) = range(k);
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let x = num.parse::<f64>().ok().map(|x| match band_param(k) {
        Some((_, B_SEP)) if t.contains("sep") => -x.abs(),
        Some((_, B_THRES)) if t.ends_with('a') => -x.abs(),
        _ => x,
    });
    // The number a value's text reads as (signed for the separation and the saturation, 0 for "Original" and "No
    // saturation"); a named value reads as none.
    let reads = |v: i32| -> Option<f64> {
        let s = text(k, v);
        let n: String = s.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-')).collect();
        match band_param(k) {
            Some((_, B_SEP | B_THRES)) if v == 0 => Some(0.0),
            Some((_, B_SEP | B_THRES)) => n.parse::<f64>().ok().map(|r| if s.ends_with("separated") || s.contains("mode A") { -r } else { r }),
            _ if amount(k, v).is_none() => None,
            _ => n.parse().ok(),
        }
    };
    // The values that show the typed text: by the text itself (a named value's too), else by its number.
    let mut same: Vec<i32> = (lo..=hi).filter(|&v| text(k, v).to_ascii_lowercase() == t).collect();
    if same.is_empty() {
        if k == P_LOW_CUT && t.starts_with("off") {
            return Some(-1);
        }
        let x = x?;
        same = (lo..=hi).filter(|&v| reads(v) == Some(x)).collect();
    }
    if let Some(v) = [cur, default(k)].into_iter().find(|v| same.contains(v)) {
        return Some(v);
    }
    let near = |v: &i32| match (amount(k, *v), x) {
        (Some(a), Some(x)) => (a - x).abs(),
        _ => f64::INFINITY,
    };
    let pool: Vec<i32> = if same.is_empty() { (lo..=hi).filter(|&v| amount(k, v).is_some()).collect() } else { same };
    pool.into_iter().min_by(|a, b| near(a).total_cmp(&near(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The texts FL's hints show for the factory default and for unusual values (plugins/lemon-press.md §3).
    #[test]
    fn texts_match_fls_hints() {
        let b = |k| band(MASTER, k);
        for (k, v, t) in [
            (P_LOW_FREQ, 19800, "200Hz"),
            (P_HIGH_FREQ, 48065, "3000Hz"),
            (P_LOW_FREQ, 0, "30Hz"),
            (P_HIGH_FREQ, 65536, "16000Hz"),
            (P_LOW_CUT, 32768, "20Hz"),
            (P_LOW_CUT, -1, "Off"),
            (P_LMH_DELAY, 1115, "2.00ms"),
            (P_LMH_MIX, 1000, "100%"),
            (b(B_PRE), 1000, "0.0dB 1.00"),
            (b(B_PRE), 0, "-INFdB 0.00"),
            (b(B_POST), 2000, "18.1dB 8.00"),
            (b(B_CEIL), 1500, "9.3dB 2.92"),
            (b(B_SEP), 0, "Original"),
            (b(B_SEP), 500, "50% merged"),
            (b(B_SEP), -1000, "100% separated"),
            (b(B_THRES), 0, "No saturation"),
            (b(B_THRES), -500, "50% saturation mode A"),
            (b(B_THRES), 1000, "100% saturation mode B"),
            (b(B_ATT), 1115, "2.00ms"),
            (b(B_ATT), 0, "Default"),
            (b(B_ATT), 10000, "1000.00ms"),
            (b(B_REL), 5000, "85.53ms"),
            (b(B_REL), 0, "0.00ms"),
            (b(B_REL), 10000, "2000.00ms"),
            (b(B_REL2), 0, "Default"),
            (b(B_REL2), 1, "0.00ms"),
            (b(B_REL2), 10000, "2000.00ms"),
            (b(B_WINDOW), 2882, "10.00ms"),
            (b(B_WINDOW), 10000, "1000.00ms"),
            (b(B_MODE), 1, "compression off"),
            (b(B_RMS), 1, "RMS"),
        ] {
            assert_eq!(text(k, v), t, "{k} at {v}");
        }
        let p = |k, s| parse(k, s, default(k));
        assert_eq!(p(P_LOW_FREQ, "200 Hz").map(|v| text(P_LOW_FREQ, v)), Some("200Hz".into()));
        assert_eq!(p(P_LOW_CUT, "off"), Some(-1));
        assert_eq!(p(b(B_PRE), "0"), Some(1000));
        assert_eq!(p(b(B_SEP), "50% separated"), Some(-500));
        assert_eq!(p(b(B_SEP), "50"), Some(500));
        for k in 0..PARAMS {
            let (lo, hi) = range(k);
            assert!((lo..=hi).contains(&default(k)), "{k}");
        }
    }

    /// When several values show the typed text, the current one stays, else the default, else the nearest before
    /// rounding: a default's own text restores it and "100%" is the top.
    #[test]
    fn typed_ties_keep_the_current_value_then_the_default() {
        assert_eq!(parse(P_LMH_MIX, "100%", 0), Some(1000));
        assert_eq!(parse(P_LOW_FREQ, "200Hz", 0), Some(19800));
        assert_eq!(parse(P_LOW_CUT, "20Hz", -1), Some(32768));
        let v = 19790;
        assert_eq!(text(P_LOW_FREQ, v), "200Hz");
        assert_eq!(parse(P_LOW_FREQ, "200Hz", v), Some(v));
        assert_eq!(parse(band(MASTER, B_PRE), "0.0dB", 0), Some(1000));
    }

    /// 0.00 ms on the attack and release 2 is value 1; "Default" is 0 (plugins/lemon-press.md §3).
    #[test]
    fn zero_ms_types_apart_from_default() {
        for k in [band(MASTER, B_ATT), band(MASTER, B_REL2)] {
            assert_eq!(parse(k, "0", 500), Some(1), "{k}");
            assert_eq!(parse(k, "0.00ms", 500), Some(1), "{k}");
            assert_eq!(parse(k, "default", 500), Some(0), "{k}");
        }
    }
}
