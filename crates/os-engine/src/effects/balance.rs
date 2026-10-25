// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's balance (a recreation of FL's Fruity Balance mixer effect), configured from its saved state.
//!
//! Two knobs, balance and volume, make a two-by-two gain matrix: turning the balance to one side attenuates the other
//! side and folds what it loses into the favoured output, so hard left or right carries the sum of both inputs. The
//! four gains glide to new settings at ten times the plugins' usual rate.
//!
// spec: plugins/fruity-balance.md, plugins/lemon-balance.md

use os_dsp::glide::{plugin_limit, Glide};

use super::Effect;

/// The settings, in the saved state's order.
pub const PARAMS: usize = 2;
pub const BALANCE: usize = 0;
pub const VOLUME: usize = 1;

/// The settings' ranges (the editor's). Stored values outside them are used as they are.
pub const RANGES: [(i32, i32); PARAMS] = [(-128, 128), (0, 320)];

/// A fresh instance: centred, volume 256 (unity).
pub const DEFAULTS: [i32; PARAMS] = [0, 256];

/// The saved state: two ints, no version field.
pub const STATE_LEN: usize = 8;

/// Everything the saved state says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub v: [i32; PARAMS],
}

impl Default for Settings {
    fn default() -> Self {
        Settings { v: DEFAULTS }
    }
}

impl Settings {
    /// A saved state (8 bytes), its values as they are (FL does not clamp them: spec "Checked against FL").
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        Some(Settings { v: super::ints(b, 0)? })
    }

    /// The saved state (8 bytes).
    pub fn to_state(&self) -> Vec<u8> {
        super::state_of(&self.v)
    }
}

/// The four gains `[LL, RR, RL, LR]` (left to left, right to right, right into left, left into right) for a balance
/// and a volume (spec "Behaviour"): worked out in doubles from the single `level`, rounded to singles at the end. The
/// sines and cosines are the doubles nearest to the true values; the difference from a merely close sine shows at the
/// centre, where `LR = RR - LL` is not 0 (spec "Checked against FL").
pub fn gains(balance: i32, volume: i32) -> [f32; 4] {
    use std::f64::consts::PI;
    let x = (f64::from(volume) / 320.0 * 1.25) as f32;
    let level = ((f64::from(x) * 11f64.ln()).exp() - 1.0) as f32;
    let b = f64::from(balance);
    let side = b.abs() / 256.0;
    let unit = 1.0 / (10.0 * cos(PI / 4.0));
    let gain = f64::from(level) * (1.0 - 0.5 * sin(side * PI)) * unit;
    let angle = (b / 128.0 + 1.0) * PI / 4.0;
    let ll = gain * cos(angle);
    let rr = gain * sin(angle);
    let (rl, lr) = if balance < 0 { (ll - rr, 0.0) } else { (0.0, rr - ll) };
    [ll as f32, rr as f32, rl as f32, lr as f32]
}

/// The double nearest to `sin(x)`, for `|x|` up to a few units (the Taylor series in double-double arithmetic).
pub fn sin(x: f64) -> f64 {
    series(x, Dd(x, 0.0), 1)
}

/// The double nearest to `cos(x)`, for `|x|` up to a few units.
pub fn cos(x: f64) -> f64 {
    series(x, Dd(1.0, 0.0), 0)
}

/// `first - first * x^2 / ((n + 1) (n + 2)) + ...`, summed in double-double until the terms no longer count.
fn series(x: f64, first: Dd, n: u32) -> f64 {
    let x2 = Dd::prod(x, x);
    let (mut term, mut sum, mut n) = (first, first, n);
    while term.0.abs() > 1e-40 * sum.0.abs().max(1e-300) {
        term = term.mul(x2).div(f64::from((n + 1) * (n + 2))).neg();
        sum = sum.add(term);
        n += 2;
    }
    sum.0 + sum.1
}

/// A double-double: the unevaluated sum `hi + lo`, `|lo|` at most half a unit in the last place of `hi`.
#[derive(Clone, Copy)]
struct Dd(f64, f64);

impl Dd {
    /// `a + b` exactly, `|a| >= |b|`.
    fn quick(a: f64, b: f64) -> Dd {
        let s = a + b;
        Dd(s, b - (s - a))
    }

    /// `a + b` exactly.
    fn sum(a: f64, b: f64) -> Dd {
        let s = a + b;
        let bb = s - a;
        Dd(s, (a - (s - bb)) + (b - bb))
    }

    /// `a * b` exactly.
    fn prod(a: f64, b: f64) -> Dd {
        let p = a * b;
        Dd(p, a.mul_add(b, -p))
    }

    fn neg(self) -> Dd {
        Dd(-self.0, -self.1)
    }

    fn add(self, o: Dd) -> Dd {
        let Dd(s, e) = Dd::sum(self.0, o.0);
        Dd::quick(s, e + self.1 + o.1)
    }

    fn mul(self, o: Dd) -> Dd {
        let Dd(p, e) = Dd::prod(self.0, o.0);
        Dd::quick(p, e + (self.0 * o.1 + self.1 * o.0))
    }

    fn div(self, d: f64) -> Dd {
        let q = self.0 / d;
        let r = self.add(Dd::prod(q, d).neg());
        Dd::quick(q, r.0 / d)
    }
}

/// The glide's limit per sample: ten times the plugins' usual one, `single(44100 / rate * 0.001) * 10` in singles.
pub fn limit(rate: u32) -> f32 {
    plugin_limit(rate) * 10.0
}

pub struct Balance {
    applied: [i32; PARAMS],
    target: [f32; 4],
    gains: [Glide; 4],
    limit: f32,
}

impl Balance {
    /// A balance whose gains stand at the settings' values (the start of a song: nothing glides in).
    pub fn with_settings(sample_rate: u32, s: &Settings) -> Balance {
        let target = gains(s.v[BALANCE], s.v[VOLUME]);
        Balance { applied: s.v, target, gains: target.map(Glide::new), limit: limit(sample_rate) }
    }

    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<Balance> {
        Some(Balance::with_settings(sample_rate, &Settings::from_state(state)?))
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// New settings: the gains glide to them from the next block on.
    pub fn set(&mut self, s: &Settings) {
        if s.v != self.applied {
            self.applied = s.v;
            self.target = gains(s.v[BALANCE], s.v[VOLUME]);
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.limit = limit(sample_rate);
    }

    /// Puts every gain at its target (the host's flush).
    pub fn flush(&mut self) {
        self.gains = self.target.map(Glide::new);
    }

    /// One block: each gain planned once, a running sum per sample; `left' = LL * left + RL * right`, `right' = RR *
    /// right + LR * left` (singles).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let lim = self.limit;
        let [mut ll, mut rr, mut rl, mut lr] = std::array::from_fn(|k| self.gains[k].block(self.target[k], n, lim));
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (a, b, c, d) = (ll.next(), rr.next(), rl.next(), lr.next());
            let (x, y) = (*l, *r);
            *l = a * x + c * y;
            *r = b * y + d * x;
        }
    }
}

impl Effect for Balance {
    fn process(&mut self, buf: &mut [f32]) {
        let n = buf.len() / 2;
        let lim = self.limit;
        let [mut ll, mut rr, mut rl, mut lr] = std::array::from_fn(|k| self.gains[k].block(self.target[k], n, lim));
        for f in buf.chunks_exact_mut(2) {
            let (a, b, c, d) = (ll.next(), rr.next(), rl.next(), lr.next());
            let (x, y) = (f[0], f[1]);
            f[0] = a * x + c * y;
            f[1] = b * y + d * x;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_are_read_as_they_are() {
        let s = Settings { v: [-64, 300] };
        assert_eq!(Settings::from_state(&s.to_state()), Some(s));
        let wild = Settings { v: [-300, 400] };
        assert_eq!(Settings::from_state(&wild.to_state()), Some(wild));
        assert_eq!(Settings::from_state(&[0, 0, 0, 0]), None);
        // FL's render of balance -300, volume 400: the matrix fitted to it is LL 3.62603, RL 10.02686, RR -6.40084.
        let [ll, rr, rl, lr] = gains(-300, 400);
        assert!((ll - 3.62603).abs() < 1e-5 && (rl - 10.02686).abs() < 1e-5 && (rr + 6.40084).abs() < 1e-5);
        assert_eq!(lr, 0.0);
    }

    #[test]
    // The expected values are rounded sines and cosines (`sin(pi / 4)` is one below `FRAC_1_SQRT_2`), not stand-ins
    // for the constants.
    #[allow(clippy::approx_constant)]
    fn sines_and_cosines_are_the_nearest_doubles() {
        use std::f64::consts::PI;
        // pi / 4 (a double a little below the true one): the sine rounds down, the cosine up.
        assert_eq!(sin(PI / 4.0), 0.7071067811865475);
        assert_eq!(cos(PI / 4.0), 0.7071067811865476);
        // Some balance angles `(b / 128 + 1) * pi / 4` and sides `k / 256 * pi` where a close sine is one off.
        assert_eq!(cos((-63.0 / 128.0 + 1.0) * PI / 4.0), 0.921514039342042);
        assert_eq!(sin((-43.0 / 128.0 + 1.0) * PI / 4.0), 0.49822766697278187);
        assert_eq!(sin((16.0 / 128.0 + 1.0) * PI / 4.0), 0.773010453362737);
        assert_eq!(sin(97.0 / 256.0 * PI), 0.9285060804732155);
        assert_eq!(sin(0.0), 0.0);
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(sin(PI / 2.0), 1.0);
    }

    #[test]
    fn centred_at_unity_and_hard_sides_carry_the_sum() {
        // Centre: the nearest doubles to sin and cos(pi / 4) differ by one unit, so LR is -2^-52 (as in FL's render).
        assert_eq!(gains(0, 256), [1.0, 1.0, 0.0, -(2f32.powi(-52))]);
        assert_eq!(gains(0, 0), [0.0, 0.0, 0.0, 0.0]);
        // Hard left: RR = 0, left' = LL * (left + right), LL at 0.7071 of its centre value.
        let [ll, rr, rl, lr] = gains(-128, 256);
        assert!(rr.abs() < 1e-7 && lr == 0.0, "{rr} {lr}");
        assert!((ll - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6 && (rl - ll).abs() < 1e-6);
        let [ll, rr, rl, lr] = gains(128, 256);
        assert!(ll.abs() < 1e-7 && rl == 0.0 && (lr - rr).abs() < 1e-6, "{ll} {rl} {lr} {rr}");
        // Volume 320: 1.25 on the curve, 1.9033 at centre.
        assert!((gains(0, 320)[0] - 1.9033).abs() < 1e-4);
    }

    #[test]
    fn the_glide_limit_is_ten_times_the_usual_one() {
        assert_eq!(limit(44100).to_bits(), 0.010000000708f32.to_bits());
        assert_ne!(limit(44100), 0.01);
    }

    #[test]
    fn gains_start_at_the_settings_and_glide_to_new_ones() {
        let mut b = Balance::with_settings(44100, &Settings { v: [0, 256] });
        let mut l = vec![1.0f32; 100];
        let mut r = vec![0.5f32; 100];
        b.process(&mut l, &mut r);
        assert!(l.iter().all(|&v| v == 1.0) && r.iter().all(|&v| v == 0.5));
        // Volume 0: 1 to 0 at 0.01 per sample takes 100 samples; the block's first sample has the old gains.
        b.set(&Settings { v: [0, 0] });
        let mut l = vec![1.0f32; 50];
        let mut r = vec![1.0f32; 50];
        b.process(&mut l, &mut r);
        assert_eq!(l[0], 1.0);
        assert!(l[49] < 0.52 && l[49] > 0.5, "{}", l[49]);
        let mut l = vec![1.0f32; 60];
        let mut r = vec![1.0f32; 60];
        b.process(&mut l, &mut r);
        b.process(&mut l, &mut r);
        assert!(l.iter().all(|&v| v == 0.0), "{l:?}");
    }
}
