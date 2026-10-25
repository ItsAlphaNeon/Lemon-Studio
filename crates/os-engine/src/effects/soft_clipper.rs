// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's soft clipper (a recreation of FL's Fruity Soft Clipper mixer effect), configured from its saved state.
//!
//! No memory: each sample above the threshold is bent onto an exponential curve that leaves the threshold with slope
//! 1 and approaches full scale; samples below it pass unchanged. A post gain follows.
//!
// spec: plugins/fruity-soft-clipper.md, plugins/lemon-soft-clipper.md

use super::Effect;

/// The settings, in the saved state's order.
pub const PARAMS: usize = 2;
pub const THRESHOLD: usize = 0;
pub const POST: usize = 1;

/// The editor's ranges (stored values are used as they are, also outside them).
pub const RANGES: [(i32, i32); PARAMS] = [(1, 127), (0, 160)];

/// FL's `Default` preset, also a fresh instance's settings.
pub const DEFAULTS: [i32; PARAMS] = [100, 128];

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
    /// A saved state (8 bytes; longer ones are read from their start).
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        Some(Settings { v: super::ints(b, 0)? })
    }

    /// The saved state (8 bytes).
    pub fn to_state(&self) -> Vec<u8> {
        super::state_of(&self.v)
    }
}

/// The threshold as a level, `single(threshold) * 0.0078125` (spec "From the settings to the constants").
pub fn threshold_level(threshold: i32) -> f32 {
    threshold as f32 * 0.0078125
}

/// The post gain: the level curve in the host's form of `single(post gain) * 0.0078125`; 128 is unity.
pub fn post_gain(post: i32) -> f32 {
    os_dsp::level::curve(post as f32 * 0.0078125)
}

/// The curve's constants for a threshold: `T`, `K = 1 - T` (2^-16 when not above 0) and `R = 1 / K`, singles.
pub fn constants(threshold: i32) -> (f32, f32, f32) {
    let t = threshold_level(threshold);
    let mut k = 1.0 - t;
    if k <= 0.0 {
        k = 1.0 / 65536.0;
    }
    (t, k, 1.0 / k)
}

/// 2^64: the spacing of the x87 unit's extended values just below 1 is its reciprocal.
const TWO64: f64 = 18_446_744_073_709_551_616.0;

/// The clipper's arithmetic on one sample (spec "Behaviour"): above the threshold, `(1 - exp((T - a) * R)) * K + T` in
/// doubles, rounded once, with the input's sign bit; below it the sample itself. `a > T` compares the singles as
/// non-negative numbers, so a NaN counts as above.
///
/// The exponential is FL's extended one: just below 1 its values lie 2^-64 apart, so `1 - exp` is a multiple of 2^-64
/// there (taken here from `exp_m1`, rounded to that grid). Doubles alone would put it on a grid of 2^-53 and lose small
/// results at threshold 0 (spec "Checked against FL", threshold 0).
#[inline]
pub fn clip(s: f32, t: f32, k: f32, r: f32) -> f32 {
    let a = s.abs();
    if a > t || a.is_nan() {
        let m = -((f64::from(t) - f64::from(a)) * f64::from(r)).exp_m1();
        let m = if m <= 0.5 { (m * TWO64).round_ties_even() / TWO64 } else { m };
        let y = (m * f64::from(k) + f64::from(t)) as f32;
        y.copysign(s)
    } else {
        s
    }
}

pub struct SoftClipper {
    applied: [i32; PARAMS],
    t: f32,
    k: f32,
    r: f32,
    gain: f32,
}

impl SoftClipper {
    pub fn new() -> SoftClipper {
        SoftClipper::with_settings(&Settings::default())
    }

    pub fn with_settings(s: &Settings) -> SoftClipper {
        let mut c = SoftClipper { applied: DEFAULTS, t: 0.0, k: 0.0, r: 0.0, gain: 1.0 };
        c.derive(&s.v);
        c
    }

    pub fn from_state(state: &[u8]) -> Option<SoftClipper> {
        Some(SoftClipper::with_settings(&Settings::from_state(state)?))
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// Applies the settings; nothing glides, a change acts from the next block on.
    pub fn set(&mut self, s: &Settings) {
        if s.v != self.applied {
            self.derive(&s.v);
        }
    }

    fn derive(&mut self, v: &[i32; PARAMS]) {
        self.applied = *v;
        (self.t, self.k, self.r) = constants(v[THRESHOLD]);
        self.gain = post_gain(v[POST]);
    }

    /// One sample of either channel: the curve, then the post gain unless it is exactly 1.
    #[inline]
    pub fn sample(&self, s: f32) -> f32 {
        let y = clip(s, self.t, self.k, self.r);
        if self.gain != 1.0 { y * self.gain } else { y }
    }

    /// Processes a stereo block in place (the channels alike and independently).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for v in left.iter_mut().chain(right.iter_mut()) {
            *v = self.sample(*v);
        }
    }
}

impl Default for SoftClipper {
    fn default() -> Self {
        SoftClipper::new()
    }
}

impl Effect for SoftClipper {
    fn process(&mut self, buf: &mut [f32]) {
        for v in buf.iter_mut() {
            *v = self.sample(*v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip() {
        let s = Settings { v: [64, 160] };
        let b = s.to_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(Settings::from_state(&b), Some(s));
        assert_eq!(Settings::from_state(&[100, 0, 0, 0]), None);
        // FL's Default preset: 100 and 128.
        assert_eq!(Settings::from_state(&[100, 0, 0, 0, 128, 0, 0, 0]), Some(Settings::default()));
    }

    #[test]
    fn constants_and_post_gain() {
        assert_eq!(constants(100), (0.78125, 0.21875, 1.0 / 0.21875f32));
        assert_eq!(constants(64), (0.5, 0.5, 2.0));
        // 128 and more: the distance to the ceiling is 2^-16.
        assert_eq!(constants(128).1, 1.0 / 65536.0);
        assert_eq!(constants(300).1, 1.0 / 65536.0);
        assert_eq!(post_gain(128), 1.0);
        assert_eq!(post_gain(0), 0.0);
        assert!((post_gain(160) - 1.9033).abs() < 1e-4);
    }

    #[test]
    fn below_the_threshold_passes_and_above_bends_towards_full_scale() {
        let c = SoftClipper::new();
        for s in [0.0f32, 0.5, -0.78125, 0.78125, 1e-30] {
            assert_eq!(c.sample(s).to_bits(), s.to_bits());
        }
        // Slope 1 at the threshold, never past 1.
        let just = c.sample(0.79);
        assert!(just > 0.789 && just < 0.79, "{just}");
        assert!((c.sample(1.0) - 0.9195).abs() < 1e-4);
        for s in [4.0f32, 1e6, f32::MAX] {
            let y = c.sample(s);
            assert!(y <= 1.0 && y > 0.99, "{s} -> {y}");
            assert_eq!(c.sample(-s), -y);
        }
        assert_eq!(c.sample(f32::INFINITY), 1.0);
        // Threshold 0: 1 - exp(-a) everywhere.
        let z = SoftClipper::with_settings(&Settings { v: [0, 128] });
        assert_eq!(z.sample(1.0), (1.0 - (-1.0f64).exp()) as f32);
        assert_eq!(z.sample(0.0), 0.0);
        // Tiny inputs at threshold 0 come out on the extended exponential's grid of 2^-64 (FL's render of the null
        // test's probe: 9.808161e-17 gives 9.8066087e-17, 2.180811e-16 gives 2.1808727e-16, 4.5e-22 gives 0).
        assert_eq!(z.sample(9.808161e-17), 9.8066087e-17);
        assert_eq!(z.sample(-2.180811e-16), -2.1808727e-16);
        assert_eq!(z.sample(4.5e-22), 0.0);
    }

    /// The exponent is kept in doubles: `(T - a) * R` rounded to single first gives other samples (spec "Checked
    /// against FL").
    #[test]
    fn the_exponent_is_not_rounded() {
        let c = SoftClipper::with_settings(&Settings { v: [90, 128] });
        let (t, k, r) = constants(90);
        let want = ((1.0 - ((f64::from(t) - 2.0) * f64::from(r)).exp()) * f64::from(k) + f64::from(t)) as f32;
        assert_eq!(c.sample(-2.0), -want);
        let c = SoftClipper::with_settings(&Settings { v: [127, 0] });
        assert_eq!(c.sample(0.999), 0.0);
        assert_eq!(c.sample(0.5), 0.0);
    }
}
