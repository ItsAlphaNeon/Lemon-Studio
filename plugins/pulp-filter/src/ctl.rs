// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The settings the editor's knobs and sliders show, by FL's automation index: names, ranges, defaults and value
//! texts.
//!
// spec: plugins/lemon-pulp-filter.md §3, plugins/fruity-love-philter.md ("The settings")

use os_engine::effects::love_philter::state::*;

/// The setting's default (the starting state's).
pub fn default(k: usize) -> i32 {
    match k {
        MASTER | LFO_MASTER | FOLLOW_LEVEL => 128,
        FOLLOW_ATTACK => 4096,
        FOLLOW_RELEASE => 8192,
        PATTERN => 1,
        6 | 7 => 512,
        _ => match filter_param(k) {
            Some((_, FilterParam::Control(n))) => CONTROL_DEFAULTS[n],
            Some((_, FilterParam::Lfo(_, i))) => LFO_DEFAULTS[i],
            None => 0,
        },
    }
}

/// The setting's value within its range (a state may hold values outside it; they play as stored).
pub fn shown(st: &State, k: usize) -> i32 {
    let (lo, hi) = range(k);
    st.param(k).unwrap_or(0).clamp(lo, hi)
}

/// The knob position of a value, 0..1.
pub fn norm(k: usize, v: i32) -> f32 {
    let (lo, hi) = range(k);
    ((v - lo) as f32 / (hi - lo).max(1) as f32).clamp(0.0, 1.0)
}

/// The value at a knob position.
pub fn from_norm(k: usize, x: f32) -> i32 {
    let (lo, hi) = range(k);
    lo + (x.clamp(0.0, 1.0) * (hi - lo) as f32).round() as i32
}

/// Whether the knob's arc starts at its centre.
pub fn bipolar(k: usize) -> bool {
    match k {
        LFO_MASTER => true,
        _ => match filter_param(k) {
            Some((_, FilterParam::Control(n))) => matches!(n, C_PAN | C_IN | C_NEXT),
            Some((_, FilterParam::Lfo(_, i))) => i > 0,
            None => false,
        },
    }
}

/// The hint's name of setting `k` (FL's hint names, measured; the low / gain / drive control is named for the filter's
/// type, a Lemon Studio choice).
pub fn name(st: &State, k: usize) -> String {
    match k {
        MASTER => "Master level".into(),
        LFO_MASTER => "Master LFO level".into(),
        FOLLOW_LEVEL => "Envelope follower level".into(),
        FOLLOW_ATTACK => "Envelope follower attack".into(),
        FOLLOW_RELEASE => "Envelope follower release".into(),
        PATTERN => "Pattern number".into(),
        6 => "Modulation X".into(),
        7 => "Modulation Y".into(),
        _ => match filter_param(k) {
            Some((f, FilterParam::Control(n))) => {
                let s: &str = match n {
                    C_ENV => "Cutoff envelope / LFO / IEF amount",
                    C_CUT => "Cutoff frequency",
                    C_RES => "Resonance / bandwidth",
                    C_LOW => match st.filters[f].kind {
                        LOW_SHELF | PEAKING | HIGH_SHELF => "Gain",
                        MANGO => "Drive",
                        _ => "Low level",
                    },
                    C_BAND => "Band level",
                    C_HIGH => "High level",
                    C_PRE => "Waveshaper pre amp",
                    C_WSMIX => "Waveshaper mix",
                    C_PAN => "Output panning",
                    C_VOL => "Output volume",
                    C_IN => "Input volume",
                    _ => "To next filter",
                };
                s.into()
            }
            Some((_, FilterParam::Lfo(_, i))) => {
                ["LFO speed", "LFO tension", "LFO skew", "LFO pulse width"][i].into()
            }
            None => String::new(),
        },
    }
}

fn pct(v: f64) -> String {
    format!("{}%", v.round() as i64)
}

/// The follower's attack or release in ms (its time to fall to 1 %, spec "follower coefficients").
pub fn follower_ms(v: i32) -> f64 {
    ((f64::from(v) / 16384.0 * 81f64.ln()).exp() - 1.0) * 12.5
}

/// An LFO speed in cycles per beat (synced) or per second.
pub fn lfo_rate(v: i32) -> f64 {
    let x = f64::from(((f64::from(v as f32 * (1.0 / 65536.0)) * 10001f64.ln()).exp() - 1.0) as f32);
    x * 6_871_947.673_6 * 2.0 / 4_294_967_296.0
}

/// The value text of setting `k` at `v`.
pub fn text(st: &State, k: usize, v: i32) -> String {
    let f = f64::from(v);
    match k {
        MASTER | FOLLOW_LEVEL => pct(f / 128.0 * 100.0),
        LFO_MASTER => pct(f / 128.0 * 100.0),
        FOLLOW_ATTACK | FOLLOW_RELEASE => format!("{}ms", follower_ms(v).round() as i64),
        PATTERN => v.to_string(),
        6 | 7 => pct(f / 1024.0 * 100.0),
        _ => match filter_param(k) {
            Some((_, FilterParam::Control(n))) => match n {
                C_ENV | C_CUT | C_RES => pct(f / 256.0 * 100.0),
                C_PAN => match v {
                    0 => "Centered".into(),
                    v if v < 0 => format!("{} left", pct(-f / 128.0 * 100.0)),
                    _ => format!("{} right", pct(f / 128.0 * 100.0)),
                },
                _ => pct(f / 128.0 * 100.0),
            },
            Some((f2, FilterParam::Lfo(t, i))) => match i {
                0 => {
                    let synced = st.filters[f2].arts[t].lfo.flags & os_dsp::curve::SYNCED != 0;
                    let r = lfo_rate(v);
                    if synced { format!("{r:.2} per beat") } else { format!("{r:.2} Hz") }
                }
                _ => pct(f / 128.0 * 100.0),
            },
            None => String::new(),
        },
    }
}

/// A typed value: a number in the text's unit (percent, ms, Hz or per beat), or "center" for a pan.
pub fn parse(st: &State, k: usize, s: &str) -> Option<i32> {
    let t = s.trim().to_ascii_lowercase();
    if t.starts_with("cent") {
        return Some(0);
    }
    let num: String = t.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let x: f64 = num.parse().ok()?;
    let (lo, hi) = range(k);
    // The nearest value whose text reads as the typed number.
    let reads = |v: i32| -> f64 {
        let s = text(st, k, v);
        let n: String = s.chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-')).collect();
        let mut r: f64 = n.parse().unwrap_or(0.0);
        if s.ends_with("left") {
            r = -r;
        }
        r
    };
    let x = if t.ends_with("left") || t.ends_with('l') { -x.abs() } else { x };
    (lo..=hi).min_by(|&a, &b| (reads(a) - x).abs().total_cmp(&(reads(b) - x).abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_match_fls_hints() {
        let st = State::new_default();
        assert_eq!(text(&st, FOLLOW_ATTACK, 4096), "25ms");
        assert_eq!(text(&st, FOLLOW_RELEASE, 8192), "100ms");
        assert_eq!(text(&st, MASTER, 104), "81%");
        assert_eq!(text(&st, control_index(0, C_CUT).unwrap(), 128), "50%");
        assert_eq!(text(&st, control_index(0, C_PAN).unwrap(), 0), "Centered");
        assert_eq!(text(&st, control_index(0, C_PAN).unwrap(), -64), "50% left");
        assert_eq!(name(&st, control_index(1, C_NEXT).unwrap()), "To next filter");
        assert_eq!(name(&st, lfo_index(1, T_CUT, 0)), "LFO speed");
        assert_eq!(parse(&st, control_index(0, C_VOL).unwrap(), "50"), Some(64));
        assert_eq!(parse(&st, control_index(0, C_PAN).unwrap(), "center"), Some(0));
        assert_eq!(parse(&st, FOLLOW_ATTACK, "25 ms").map(|v| text(&st, FOLLOW_ATTACK, v)), Some("25ms".into()));
        for k in 0..PARAMS {
            let (lo, hi) = range(k);
            assert!((lo..=hi).contains(&default(k)), "{k}");
        }
    }
}
