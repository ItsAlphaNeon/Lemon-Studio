// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's fast distortion (a recreation of FL's Fruity Fast Dist mixer effect), configured from its saved
//! state.
//!
//! A waveshaper without memory: each sample is replaced by a value looked up, with linear interpolation, from a table
//! of 8192 whole 16-bit levels over its own magnitude, and mixed with the unprocessed sample. Two curve types: A
//! compresses what is above -25 dB by a ratio the threshold sets and adds a make-up gain; B raises the attenuation in
//! dB to a power below one. A sample is worked out in more than single precision and rounded toward zero at the end.
//!
// spec: plugins/fruity-fast-dist.md, plugins/lemon-fast-distortion.md

use super::Effect;

/// The settings, in the saved state's order.
pub const PARAMS: usize = 5;
pub const PRE: usize = 0;
pub const THRESHOLD: usize = 1;
pub const TYPE: usize = 2;
pub const MIX: usize = 3;
pub const POST: usize = 4;

/// The settings' ranges. Stored values outside them: the threshold and the type pick a curve together ([`curve`]);
/// pre amp, mix and post gain are used as they are.
pub const RANGES: [(i32, i32); PARAMS] = [(64, 192), (1, 10), (0, 1), (0, 128), (0, 128)];

/// FL's `Default` preset: unity pre amp, threshold 10, type A, full mix and post gain.
pub const DEFAULTS: [i32; PARAMS] = [128, 10, 0, 128, 128];

/// Curve types.
pub const TYPE_A: i32 = 0;
pub const TYPE_B: i32 = 1;

/// The saved state: five ints, no version field.
pub const STATE_LEN: usize = 20;

/// The table's size (one more entry follows for the interpolation).
pub const TABLE: usize = 8192;

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
    /// A saved state (20 bytes).
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        Some(Settings { v: super::ints(b, 0)? })
    }

    /// The saved state (20 bytes).
    pub fn to_state(&self) -> Vec<u8> {
        super::state_of(&self.v)
    }
}

/// A level knob: the level curve in the plugins' form of `value / 128` (`x` and its product with ln 11 singles, the
/// exponential in doubles, times the single 0.1, one rounding; exactly 1 at 128).
pub fn knob(value: i32) -> f32 {
    os_dsp::level::curve(value as f32 * 0.0078125)
}

/// The gains of the mix: `wet = mix * post`, `dry = (post - wet) * pre * post` (singles).
pub fn gains(pre: f32, mix: f32, post: f32) -> (f32, f32) {
    let wet = mix * post;
    let dry = (post - wet) * pre * post;
    (wet, dry)
}

/// The curve's whole 16-bit level (`Round(10^(shaped / 20) * 32767)`, doubles) at `i` of 8191 for a step
/// `10 - threshold`; 0 at `i = 0`.
pub fn level(i: usize, step: i32, kind: i32) -> f64 {
    if i == 0 {
        return 0.0;
    }
    let u = i as f64 / 8191.0;
    let db = 20.0 * u.log10();
    let step = f64::from(step);
    let shaped = if kind == TYPE_A {
        let k = if db >= -25.0 { (db + 25.0) * (step + 1.0) * 0.1 - 25.0 } else { db };
        k + (9.0 - step) * 1.6
    } else {
        -(-db).powf(step * 0.02 + 0.8)
    };
    (10f64.powf(shaped / 20.0) * 32767.0).round_ties_even()
}

/// The curve a stored threshold and type pick, as (threshold 1..10, type): one of 20, A at thresholds 1..10 and then
/// B at 1..10, number `type * 10 + threshold - 1`. So a stored threshold of 0 with type B is A at 10, and 11 with type
/// A is B at 1 (spec "Values outside the ranges"). Beyond the 20, FL reads memory that holds no curve; Lemon Studio
/// takes the nearest end (A at 1 below, B at 10 above).
pub fn curve(threshold: i32, kind: i32) -> (i32, i32) {
    let n = (i64::from(kind) * 10 + i64::from(threshold) - 1).clamp(0, 19) as i32;
    (n % 10 + 1, n / 10)
}

/// The curve table, times `wet`: `single(value / 32768) * wet` per entry, the product not rounded to single, and an
/// 8193rd entry equal to the last.
pub fn table(threshold: i32, kind: i32, wet: f32) -> Vec<f64> {
    let (threshold, kind) = curve(threshold, kind);
    let step = 10 - threshold;
    // `value / 32768` is exact as a single (a whole number below 2^15 over 2^15).
    let mut t: Vec<f64> = (0..TABLE).map(|i| level(i, step, kind) / 32768.0 * f64::from(wet)).collect();
    t.push(t[TABLE - 1]);
    t
}

/// From `p = 2^31` on (and for NaN) the table's part of a sample is +0: FL's whole-number index overflows there (spec
/// "Checked against FL", very loud input).
const OVERFLOW: f64 = 2_147_483_648.0;

/// Rounds toward zero to single, as FL stores a sample (spec "Per sample").
#[inline]
pub fn chop(y: f64) -> f32 {
    let s = y as f32;
    if f64::from(s).abs() > y.abs() {
        // One step towards zero: the magnitude's bits less one (infinity becomes the largest finite single).
        f32::from_bits(s.to_bits() - 1)
    } else {
        s
    }
}

pub struct FastDist {
    applied: [i32; PARAMS],
    table: Vec<f64>,
    /// `pre * 8191`, not rounded to single.
    scale: f64,
    dry: f32,
}

impl FastDist {
    pub fn new() -> FastDist {
        FastDist::with_settings(&Settings::default())
    }

    pub fn with_settings(s: &Settings) -> FastDist {
        let mut d = FastDist { applied: s.v, table: Vec::new(), scale: 0.0, dry: 0.0 };
        d.derive();
        d
    }

    pub fn from_state(state: &[u8]) -> Option<FastDist> {
        Some(FastDist::with_settings(&Settings::from_state(state)?))
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// Applies the settings: the table and the two gains are worked out anew and hold from the next block on.
    pub fn set(&mut self, s: &Settings) {
        if s.v != self.applied {
            self.applied = s.v;
            self.derive();
        }
    }

    fn derive(&mut self) {
        let v = self.applied;
        let pre = knob(v[PRE]);
        let (wet, dry) = gains(pre, knob(v[MIX]), knob(v[POST]));
        self.table = table(v[THRESHOLD], v[TYPE], wet);
        self.scale = f64::from(pre) * 8191.0;
        self.dry = dry;
    }

    /// One sample of either channel: the interpolated table at `p = |x * pre * 8191|`, with the sign of `x`, over `x *
    /// dry`, all in doubles (FL's extended precision; doubles give the same samples), rounded toward zero to single.
    #[inline]
    pub fn sample(&self, x: f32) -> f32 {
        let x64 = f64::from(x);
        let p = (x64 * self.scale).abs();
        let shaped = if p < OVERFLOW {
            let i = p.min(8191.0) as usize;
            let f = p - p.trunc();
            let (a, b) = (self.table[i], self.table[i + 1]);
            (a + (b - a) * f).copysign(x64)
        } else {
            0.0
        };
        chop(x64 * f64::from(self.dry) + shaped)
    }

    /// Processes a stereo block in place (the channels alike and independently).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for v in left.iter_mut().chain(right.iter_mut()) {
            *v = self.sample(*v);
        }
    }
}

impl Default for FastDist {
    fn default() -> Self {
        FastDist::new()
    }
}

impl Effect for FastDist {
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
        // FL's "fuzz" preset.
        let s = Settings { v: [192, 10, 1, 128, 80] };
        let b = s.to_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(Settings::from_state(&b), Some(s));
        assert_eq!(Settings::from_state(&b[..16]), None);
    }

    #[test]
    fn knobs_and_gains() {
        assert_eq!(knob(128), 1.0);
        assert_eq!(knob(0), 0.0);
        // Full mix and post: all wet, nothing dry.
        assert_eq!(gains(knob(192), 1.0, 1.0), (1.0, 0.0));
        let (wet, dry) = gains(1.0, knob(64), 1.0);
        assert!(wet > 0.2 && wet < 0.3 && (dry - (1.0 - wet)).abs() < 1e-7, "{wet} {dry}");
    }

    #[test]
    fn the_table_holds_whole_16_bit_levels() {
        // Threshold 10 (step 0), type A: -25 dB and below unchanged but for 14.4 dB of make-up gain, above it a tenth.
        let t = table(10, TYPE_A, 1.0);
        assert_eq!(t.len(), TABLE + 1);
        assert_eq!(t[0], 0.0);
        assert_eq!(t[TABLE], t[TABLE - 1]);
        for &v in &t {
            assert_eq!((v * 32768.0).fract(), 0.0);
        }
        // Full scale comes out at 0.1 * 25 - 25 + 14.4 = -8.1 dB.
        let top = (10f64.powf(-8.1 / 20.0) * 32767.0).round_ties_even() / 32768.0;
        assert_eq!(t[8191], top);
        assert!(t.windows(2).all(|w| w[1] >= w[0]));
        // Type B at threshold 1 (step 9): the attenuation in dB raised to 0.98.
        let b = table(1, TYPE_B, 1.0);
        assert_eq!(b[8191], 32767.0 / 32768.0);
        assert!(b[4096] > 0.5 && b[4096] < 0.53, "{}", b[4096]);
        // Out of range: threshold and type pick one of the 20 curves together (FL 2026's renders of threshold 0 with
        // type B and 11 with type A); beyond them, the nearest end.
        assert_eq!(curve(0, TYPE_B), (10, TYPE_A));
        assert_eq!(curve(11, TYPE_A), (1, TYPE_B));
        assert_eq!(table(0, TYPE_B, 1.0), t);
        assert_eq!(table(11, TYPE_A, 1.0), b);
        assert_eq!(curve(5, 2), (10, TYPE_B));
        assert_eq!(curve(5, -1), (1, TYPE_A));
        assert_eq!(curve(i32::MAX, i32::MAX), (10, TYPE_B));
        assert_eq!(curve(i32::MIN, 0), (1, TYPE_A));
    }

    #[test]
    fn samples_interpolate_the_table_and_keep_their_sign() {
        let d = FastDist::with_settings(&Settings { v: [128, 1, 1, 128, 128] });
        let t = table(1, TYPE_B, 1.0);
        let top = t[8191] as f32;
        assert_eq!(d.sample(0.0), 0.0);
        assert_eq!(d.sample(1.0), top);
        assert_eq!(d.sample(-1.0), -top);
        assert_eq!(d.sample(5.0), top, "past the end: the last entry");
        let x = 100.5f32 / 8191.0;
        let p = f64::from(x) * 8191.0;
        let f = p - p.trunc();
        assert_eq!(d.sample(x), chop(t[100] + (t[101] - t[100]) * f));
        // Half mix: the dry part comes back.
        let h = FastDist::with_settings(&Settings { v: [128, 10, 0, 64, 128] });
        let (wet, dry) = gains(1.0, knob(64), 1.0);
        let tw = table(10, TYPE_A, wet);
        let shaped = tw[2047] + (tw[2048] - tw[2047]) * 0.75;
        assert_eq!(h.sample(-0.25), chop(-0.25 * f64::from(dry) - shaped));
    }

    #[test]
    fn results_round_toward_zero() {
        assert_eq!(chop(0.1), 0.099999994);
        assert_eq!(chop(-0.1), -0.099999994);
        assert_eq!(chop(0.5), 0.5);
        assert_eq!(chop(1e300), f32::MAX);
        assert_eq!(chop(-1e-50), 0.0);
        assert!(chop(f64::NAN).is_nan());
    }

    /// Samples of FL's renders of the null test's probe (spec "Checked against FL").
    #[test]
    fn known_samples_from_fl() {
        let cases: [([i32; PARAMS], f32, f32); 11] = [
            // Type A, threshold 1.
            ([128, 1, 0, 128, 128], 0.021931838, 0.02192916),
            ([128, 1, 0, 128, 128], 0.0591505, 0.059143275),
            // FL's compressor preset (pre amp 192, reduced mix and post gain).
            ([192, 1, 0, 104, 93], 0.087571196, 0.11551188),
            // FL's fuzz preset.
            ([192, 10, 1, 128, 80], 0.06572277, 0.14467822),
            // Type B, threshold 10, pre amp 192.
            ([192, 10, 1, 128, 128], -0.25366217, -0.8983255),
            // Pre amp, mix and post gain above their ranges.
            ([300, 6, 0, 140, 140], 0.021931838, 0.4818168),
            ([300, 6, 0, 140, 140], 0.043841213, 0.42654645),
            // Threshold 0 with type B plays A at 10; 11 with type A plays B at 1.
            ([128, 0, 1, 128, 128], 0.021931838, 0.115097575),
            ([128, 0, 1, 128, 128], -0.3715333, -0.3564453),
            ([128, 11, 0, 128, 128], 0.021931838, 0.028388012),
            ([128, 11, 0, 128, 128], -0.23921305, -0.2566399),
        ];
        for (v, x, want) in cases {
            assert_eq!(FastDist::with_settings(&Settings { v }).sample(x), want, "{v:?} {x}");
        }
        // Very loud input: from |x * pre * 8191| = 2^31 on, only the dry part is left.
        let d = FastDist::new();
        assert_eq!(d.sample(1e4), d.table[8191] as f32);
        assert_eq!(d.sample(1e6), 0.0);
        assert_eq!(d.sample(-1e6), 0.0);
        let h = FastDist::with_settings(&Settings { v: [128, 10, 0, 64, 128] });
        assert_eq!(h.sample(-1e6), chop(-1e6 * f64::from(h.dry)));
    }
}
