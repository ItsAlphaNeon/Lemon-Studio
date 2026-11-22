// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The player's built-in reverb and chorus. Only the modern mode plays them (the 32-bit player's built-in effects
//! add nothing to its output); they take the whole dry mix, whatever the font's own send amounts, and their level
//! controls (128 = the default) set how much they add.
//!
// spec: plugins/lemon-soundfont.md §3.6

use std::f64::consts::PI;

/// The chorus: three voices reading a delay line, each swept by a 0.3 Hz sine a third of a cycle apart.
pub struct Chorus {
    line: Vec<f32>,
    at: usize,
    phase: f64,
    rate: f64,
}

const CHORUS_VOICES: usize = 3;
const CHORUS_HZ: f64 = 0.3;
/// Delay swing (peak to peak) and the shortest delay, in seconds.
const CHORUS_DEPTH: f64 = 0.008;
const CHORUS_MIN: f64 = 0.0035;

impl Chorus {
    pub fn new(rate: f64) -> Chorus {
        let n = ((CHORUS_MIN + CHORUS_DEPTH) * rate) as usize + 4;
        Chorus { line: vec![0.0; n.next_power_of_two()], at: 0, phase: 0.0, rate }
    }

    /// The wet signal's gain for level control `level` (0..=2560): it grows with the square of the level.
    pub fn gain(level: i32) -> f64 {
        let l = f64::from(level.max(0)) / 128.0;
        CHORUS_GAIN * l * l
    }

    /// Adds the chorus of the mono input `x` to `l` and `r`.
    pub fn run(&mut self, x: f32, gain: f64, l: &mut f32, r: &mut f32) {
        let mask = self.line.len() - 1;
        self.line[self.at] = x;
        let mut acc = 0.0;
        for v in 0..CHORUS_VOICES {
            let ph = self.phase + v as f64 / CHORUS_VOICES as f64;
            let d = (CHORUS_MIN + CHORUS_DEPTH * 0.5 * (1.0 + (2.0 * PI * ph).sin())) * self.rate;
            let p = self.at as f64 - d;
            let i = p.floor();
            let f = p - i;
            let i = i as isize;
            let a = self.line[(i as usize) & mask];
            let b = self.line[((i + 1) as usize) & mask];
            acc += f64::from(a) * (1.0 - f) + f64::from(b) * f;
        }
        self.at = (self.at + 1) & mask;
        self.phase = (self.phase + CHORUS_HZ / self.rate).fract();
        let y = (acc * gain) as f32;
        *l += y;
        *r += y;
    }
}

/// The reverb: a pre-delay, then eight damped feedback combs and four allpasses per side (the right side's lines a
/// little longer, for width).
pub struct Reverb {
    pre: Vec<f32>,
    pre_at: usize,
    combs: [Vec<Comb>; 2],
    alls: [Vec<Allpass>; 2],
    rate: f64,
}

struct Comb {
    buf: Vec<f32>,
    at: usize,
    store: f32,
}

struct Allpass {
    buf: Vec<f32>,
    at: usize,
}

const PRE_DELAY: f64 = 0.0552;
const COMB_MS: [f64; 8] = [25.31, 26.94, 28.96, 30.75, 32.24, 33.81, 35.31, 36.67];
const ALL_MS: [f64; 4] = [12.61, 10.0, 7.73, 5.10];
const SPREAD_MS: f64 = 0.52;
/// High frequencies fade faster in the tail.
const DAMP: f32 = 0.35;
const RT_SCALE: f64 = 1.24;
const CROSS: f32 = 0.12;

impl Reverb {
    pub fn new(rate: f64) -> Reverb {
        let len = |ms: f64| ((ms / 1000.0 * rate) as usize).max(1);
        let side = |s: f64| -> (Vec<Comb>, Vec<Allpass>) {
            (
                COMB_MS.iter().map(|&m| Comb { buf: vec![0.0; len(m + s)], at: 0, store: 0.0 }).collect(),
                ALL_MS.iter().map(|&m| Allpass { buf: vec![0.0; len(m + s)], at: 0 }).collect(),
            )
        };
        let (cl, al) = side(0.0);
        let (cr, ar) = side(SPREAD_MS);
        Reverb { pre: vec![0.0; len(PRE_DELAY * 1000.0)], pre_at: 0, combs: [cl, cr], alls: [al, ar], rate }
    }

    /// The reverb time (s) and wet gain for level control `level` (0..=2560): past the default the tail also grows
    /// longer.
    pub fn settings(level: i32) -> (f64, f64) {
        const T: [(f64, f64, f64); 8] = [
            (0.0, 1.82, -200.0),
            (32.0, 1.82, -39.7),
            (64.0, 1.82, -30.0),
            (128.0, 1.82, -15.4),
            (256.0, 1.82, 10.6),
            (512.0, 2.40, 46.6),
            (1024.0, 5.90, 55.9),
            (2560.0, 17.4, 61.8),
        ];
        let l = f64::from(level.clamp(0, 2560));
        let k = T.windows(2).position(|w| l <= w[1].0).unwrap_or(T.len() - 2);
        let (a, b) = (T[k], T[k + 1]);
        let f = (l - a.0) / (b.0 - a.0);
        let rt = a.1 + (b.1 - a.1) * f;
        let energy_db = a.2 + (b.2 - a.2) * f;
        (rt, energy_db)
    }

    /// Adds the reverb of the mono input `x` to `l` and `r`; `rt` seconds to fall 60 dB, `gain` on the output.
    pub fn run(&mut self, x: f32, rt: f64, gain: f32, l: &mut f32, r: &mut f32) {
        let input = self.pre[self.pre_at];
        self.pre[self.pre_at] = x;
        self.pre_at = (self.pre_at + 1) % self.pre.len();
        let rate = self.rate;
        let mut out = [0f32; 2];
        for s in 0..2 {
            let mut acc = 0.0f32;
            for c in &mut self.combs[s] {
                let n = c.buf.len();
                // The combs' own loss makes the tail fall faster than their feedback alone: aim 1.24× longer.
                let fb = 10f64.powf(-3.0 * n as f64 / rate / (rt * RT_SCALE).max(0.05)) as f32;
                let y = c.buf[c.at];
                c.store = y * (1.0 - DAMP) + c.store * DAMP;
                c.buf[c.at] = input + c.store * fb;
                c.at = (c.at + 1) % n;
                acc += y;
            }
            for a in &mut self.alls[s] {
                let n = a.buf.len();
                let b = a.buf[a.at];
                let y = b - acc;
                a.buf[a.at] = acc + b * 0.5;
                a.at = (a.at + 1) % n;
                acc = y;
            }
            out[s] = acc;
        }
        // A little of each side in the other: the measured sides correlate by about 0.24.
        *l += (out[0] + out[1] * CROSS) * gain;
        *r += (out[1] + out[0] * CROSS) * gain;
    }
}

/// The wet output gain of the chorus at level 128 (calibrated against the 64-bit player).
pub const CHORUS_GAIN: f64 = 0.87;
/// The reverb's output scale that makes [`Reverb::settings`]' energy (dB re the input impulse's) come out
/// (calibrated against the 64-bit player).
pub const REVERB_SCALE_DB: f64 = 0.0;

/// The reverb's impulse energy (one side, unit impulse, gain 1) at reverb times (s), measured from [`Reverb`] at
/// 44.1 kHz.
const REVERB_E1: [(f64, f64); 6] = [(0.5, 310.45), (1.0, 386.91), (1.82, 474.77), (2.4, 523.62), (5.9, 731.48), (17.4, 1130.91)];

/// The output gain that gives the reverb a tail energy of `energy_db` (both sides, dB re the dry impulse's energy on
/// one side, as measured from the 64-bit player) at reverb time `rt`.
pub fn reverb_gain(rt: f64, energy_db: f64) -> f64 {
    if energy_db <= -150.0 {
        return 0.0;
    }
    let k = REVERB_E1.windows(2).position(|w| rt <= w[1].0).unwrap_or(REVERB_E1.len() - 2);
    let (a, b) = (REVERB_E1[k], REVERB_E1[k + 1]);
    let f = ((rt - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    let e1 = (a.1.ln() + (b.1.ln() - a.1.ln()) * f).exp();
    let per_side = 10f64.powf((energy_db + DRY_IMPULSE_DB - 3.0103) / 10.0);
    (per_side / e1).sqrt()
}

/// The dry impulse's energy the measured tail energies are relative to (its peak 0.1855 on one side).
const DRY_IMPULSE_DB: f64 = 14.63;

#[cfg(test)]
mod tests {
    use super::*;

    /// The modern mode's built-in reverb rings on after a note; the classic mode's adds nothing.
    #[test]
    fn only_modern_plays_the_built_in_effects() {
        use crate::dsp::{Engine, Overrides, Patch};
        use crate::profile::Mode;
        let mut d = vec![0i16; 4000];
        d[100] = 16384;
        let font = std::sync::Arc::new(crate::sf2::Font::parse(&crate::sf2::tests::font_bytes(&d, 44100, (0, 0), &[], 0)).unwrap());
        let ov = Overrides { reverb_on: true, reverb_level: 128, chorus_on: true, chorus_level: 128, ..Overrides::default() };
        for (mode, rings) in [(Mode::Modern, true), (Mode::Classic, false)] {
            let mut e = Engine::new(44100.0);
            e.mode = mode;
            e.set_patch(Some(std::sync::Arc::new(Patch { font: font.clone(), preset: Some(0), overrides: ov })));
            e.note_on(60, 1.0, None);
            let (mut l, mut r) = (vec![0.0; 22050], vec![0.0; 22050]);
            e.render(&mut l, &mut r);
            let late = l[8000..].iter().map(|v| v.abs()).fold(0.0f32, f32::max);
            assert_eq!(late > 1e-4, rings, "{mode:?} {late}");
            assert_eq!(e.has_voices(), rings, "{mode:?}");
        }
    }

    /// Prints the reverb's unit-impulse energy at the table's reverb times (for `REVERB_E1`).
    #[test]
    fn reverb_impulse_energy() {
        for rt in [0.5, 1.0, 1.82, 2.4, 5.9, 17.4] {
            let mut r = Reverb::new(44100.0);
            let mut e = 0.0f64;
            for i in 0..(44100.0 * (rt * 2.5 + 0.3)) as usize {
                let (mut a, mut b) = (0.0, 0.0);
                r.run(if i == 0 { 1.0 } else { 0.0 }, rt, 1.0, &mut a, &mut b);
                e += f64::from(a) * f64::from(a);
            }
            println!("E1 {rt} {e:.6}");
        }
    }
}
