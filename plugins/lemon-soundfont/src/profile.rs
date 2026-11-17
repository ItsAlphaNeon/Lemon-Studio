// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The two playback modes: Classic (the 32-bit player's sound) and Modern (the 64-bit player's). Everything in which
//! they differ is here: interpolation, levels (output gain, velocity, attenuation, sustain, pan) and the note's start.
//!
// spec: plugins/lemon-soundfont.md §3

use crate::tables::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Linear interpolation (a band-limited one when pitching up), exact pitch, the classic level laws.
    #[default]
    Classic,
    /// A short windowed-sinc interpolator with no anti-aliasing when pitching up, the modern level laws.
    Modern,
}

/// What the player's cutoff control asks of the filter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CutoffSetting {
    Open,
    /// The −3 dB point.
    Hz(f64),
    /// A SoundFont cutoff.
    Cents(f64),
}

pub fn db_to_gain(db: f32) -> f32 {
    if db <= -150.0 { 0.0 } else { 10f32.powf(db / 20.0) }
}

/// Piecewise-linear lookup in a sorted (x, y) table, held at the ends.
fn lerp_table(t: &[(f32, f32)], x: f32) -> f32 {
    if x <= t[0].0 {
        return t[0].1;
    }
    for w in t.windows(2) {
        if x <= w[1].0 {
            let f = (x - w[0].0) / (w[1].0 - w[0].0);
            return w[0].1 + (w[1].1 - w[0].1) * f;
        }
    }
    t[t.len() - 1].1
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Classic, Mode::Modern];

    pub fn index(self) -> u8 {
        self as u8
    }

    pub fn from_index(i: u8) -> Mode {
        if i == 1 { Mode::Modern } else { Mode::Classic }
    }

    /// The output gain for a full-scale sample at velocity 127, centre pan and no attenuation.
    pub fn gain(self) -> f32 {
        match self {
            Mode::Classic => CLASSIC_GAIN,
            Mode::Modern => MODERN_GAIN,
        }
    }

    /// Velocity (0..=128, fractional allowed) → gain relative to velocity 127.
    pub fn velocity_gain(self, v: f32) -> f32 {
        let t: &[f32; 129] = match self {
            Mode::Classic => &CLASSIC_VEL_DB,
            Mode::Modern => &MODERN_VEL_DB,
        };
        let v = v.clamp(0.0, 128.0);
        let i = (v.floor() as usize).min(127);
        let f = v - i as f32;
        let (a, b) = (db_to_gain(t[i]), db_to_gain(t[i + 1]));
        a + (b - a) * f
    }

    /// Initial attenuation (cB, after modulators) → gain.
    pub fn attenuation_gain(self, cb: f32) -> f32 {
        match self {
            // Held at 960 cB.
            Mode::Classic => db_to_gain(-lerp_table(&CLASSIC_ATT_DB, cb.clamp(0.0, 960.0))),
            Mode::Modern => db_to_gain(-0.04 * cb.max(0.0)),
        }
    }

    /// The volume envelope's sustain (cB of attenuation) → level relative to the envelope's peak, 0 = silent.
    pub fn sustain_gain(self, cb: f32) -> f32 {
        let cb = cb.max(0.0);
        match self {
            // Measured to 600 cB (−48 dB); silent at 720.
            Mode::Classic => {
                if cb > 660.0 { 0.0 } else { db_to_gain(-lerp_table(&CLASSIC_SUS_DB, cb)) }
            }
            // Measured to 720 cB (−70 dB), about 0.096 dB per cB; silent at 900.
            Mode::Modern => {
                if cb <= 720.0 {
                    db_to_gain(-lerp_table(&MODERN_SUS_DB, cb))
                } else {
                    let db = 0.096 * cb;
                    if db >= 80.0 { 0.0 } else { db_to_gain(-db) }
                }
            }
        }
    }

    /// Pan (−500..=500, 0.1 % units) → (left, right) gains relative to the centre.
    pub fn pan_gains(self, pan: f32) -> (f32, f32) {
        let t: &[(f32, f32, f32)] = match self {
            Mode::Classic => &CLASSIC_PAN_DB,
            Mode::Modern => &MODERN_PAN_DB,
        };
        let p = pan.clamp(-500.0, 500.0);
        let k = t.windows(2).position(|w| p <= w[1].0).unwrap_or(t.len() - 2);
        let (a, b) = (t[k], t[k + 1]);
        let f = (p - a.0) / (b.0 - a.0);
        let side = |x: f32, y: f32| {
            let (gx, gy) = (db_to_gain(x), db_to_gain(y));
            gx + (gy - gx) * f
        };
        (side(a.1, b.1), side(a.2, b.2))
    }

    /// The player's sustain control (0..=127) → level relative to the envelope's peak.
    pub fn sustain_control(self, s: i32) -> f32 {
        let s = s.clamp(0, 127) as f32;
        match self {
            Mode::Classic => (s / 128.0).powi(2),
            Mode::Modern => {
                if s <= 0.0 {
                    return 0.0;
                }
                const T: [(f32, f32); 5] = [(1.0, -60.0), (32.0, -23.04), (64.0, -12.29), (96.0, -5.18), (127.0, 0.0)];
                db_to_gain(lerp_table(&T, s))
            }
        }
    }

    /// The player's LFO depth control (−127..=127) → vibrato depth in cents (the sign does not matter).
    pub fn lfo_depth(self, a: i32) -> f64 {
        let a = f64::from(a.clamp(-127, 127).abs()) / 127.0;
        match self {
            Mode::Classic => a * 1200.0,
            Mode::Modern => a * 1000.0,
        }
    }

    /// The player's LFO speed control (0..=127) → Hz.
    pub fn lfo_rate(self, s: i32) -> f64 {
        let s = s.clamp(0, 127) as f32;
        match self {
            Mode::Classic => {
                const T: [(f32, f32); 6] = [(0.0, 0.0), (16.0, 1.337), (32.0, 2.660), (64.0, 5.319), (96.0, 7.812), (127.0, 10.417)];
                f64::from(lerp_table(&T, s))
            }
            Mode::Modern => f64::from(s) * 0.08403,
        }
    }

    /// The player's cutoff control (0..=127) → the filter: classic gives its −3 dB point in Hz (None = open),
    /// modern a cutoff in cents.
    pub fn cutoff_control(self, c: i32) -> CutoffSetting {
        let c = c.clamp(0, 127) as f32;
        match self {
            Mode::Classic => {
                const T: [(f32, f32); 15] = [
                    (0.0, 189.0), (8.0, 781.0), (16.0, 1427.0), (24.0, 2110.0), (32.0, 2830.0), (40.0, 3555.0), (48.0, 4323.0), (56.0, 5171.0),
                    (64.0, 6087.0), (72.0, 7048.0), (80.0, 8161.0), (88.0, 9606.0), (96.0, 11873.0), (104.0, 16448.0), (112.0, 22050.0),
                ];
                if c >= 112.0 { CutoffSetting::Open } else { CutoffSetting::Hz(f64::from(lerp_table(&T, c))) }
            }
            Mode::Modern => CutoffSetting::Cents(1500.0 + 12000.0 * f64::from(c) / 127.0),
        }
    }

    /// The filter's resonance peak in dB for a resonance of `q` cB.
    pub fn resonance_peak(self, q: f64) -> f64 {
        match self {
            // Always a little resonant; held at 14.8 dB.
            Mode::Classic => (q / 10.0).max(1.1).min(14.8),
            Mode::Modern => {
                const T: [(f32, f32); 6] = [(0.0, 0.0), (60.0, 3.6), (120.0, 9.1), (240.0, 20.3), (480.0, 34.6), (960.0, 46.4)];
                f64::from(lerp_table(&T, q as f32))
            }
        }
    }

    /// Whether note velocity lowers the filter's cutoff (the default velocity → cutoff modulator).
    pub fn velocity_filters(self) -> bool {
        self == Mode::Modern
    }

    /// Frames of silence before a note's envelope starts (at 44.1 kHz).
    pub fn start_delay(self) -> f64 {
        match self {
            Mode::Classic => 0.0,
            Mode::Modern => 32.0,
        }
    }

    /// The shortest attack ramp, in frames at 44.1 kHz.
    pub fn min_attack(self) -> f64 {
        match self {
            Mode::Classic => 88.0,
            Mode::Modern => 64.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_laws_hold_their_measured_points() {
        for m in Mode::ALL {
            assert!((m.velocity_gain(127.0) - 1.0).abs() < 1e-6);
            assert_eq!(m.velocity_gain(0.0), 0.0);
            let (l, r) = m.pan_gains(0.0);
            assert!((l - 1.0).abs() < 0.02 && (r - 1.0).abs() < 0.02, "{m:?} centre {l} {r}");
        }
        let db = |g: f32| 20.0 * g.log10();
        assert!((db(Mode::Classic.velocity_gain(64.0)) + 14.81).abs() < 0.05);
        assert!((db(Mode::Modern.velocity_gain(64.0)) + 10.40).abs() < 0.05);
        assert!((db(Mode::Classic.attenuation_gain(120.0)) + 4.38).abs() < 0.05);
        assert!((db(Mode::Modern.attenuation_gain(120.0)) + 4.80).abs() < 0.01);
        assert!((db(Mode::Classic.attenuation_gain(1440.0)) - db(Mode::Classic.attenuation_gain(960.0))).abs() < 1e-4);
        assert!((db(Mode::Classic.sustain_gain(480.0)) + 36.09).abs() < 0.1);
        assert!((db(Mode::Modern.sustain_gain(480.0)) + 46.08).abs() < 0.1);
        assert_eq!(Mode::Classic.sustain_gain(720.0), 0.0);
        let (l, r) = Mode::Classic.pan_gains(-500.0);
        assert!(r == 0.0 && (db(l) - 6.09).abs() < 0.01);
    }
}
