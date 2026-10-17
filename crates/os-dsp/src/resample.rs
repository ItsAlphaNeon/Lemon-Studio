// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The sampler's resampler: a windowed sinc over `N` frames either side of the read position, widened by the ratio
//! above the sample's own rate.
//!
//! ```text
//! w(u) = sin(a u) / (pi u) * (0.34 + 0.5 cos(pi u / N) + 0.16 cos(2 pi u / N))
//! a    = pi (ratio <= 1), pi / ratio (ratio > 1)
//! ```
//!
//! Two ways to get the weights:
//! - [`Quality::Exact`] evaluates the formula for every output sample (sines and cosines stepped from tap to tap in
//!   doubles). Exports use it.
//! - [`Quality::Table`] reads the weights from tables made once, interpolated linearly: within about 4e-7 of the
//!   formula and several times cheaper. Live playback uses it.
//!
// spec: audio/sampler.md ("The resampler", "How many frames", "Frames outside the data")

use std::f64::consts::PI;
use std::sync::OnceLock;

/// Frames either side up to a ratio of 16.5.
pub const N_MIN: usize = 16;
/// Most frames either side (a ratio of 509 and above).
pub const N_MAX: usize = 512;

/// How the weights are obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// Tables with linear interpolation (live playback).
    #[default]
    Table,
    /// The formula for every sample (export).
    Exact,
}

/// Frames either side at playback ratio `ratio` (source frames per output sample): 16 up to 16.5, then `Round(ratio)
/// + 3` rounded down to a multiple of 4, at most 512.
pub fn half_width(ratio: f64) -> usize {
    let r = ratio.abs().round_ties_even().min(1e6) as usize;
    ((r + 3) / 4 * 4).clamp(N_MIN, N_MAX)
}

/// The window at `v = u / N` (-1..1).
fn window(v: f64) -> f64 {
    let x = PI * v;
    0.34 + 0.5 * x.cos() + 0.16 * (2.0 * x).cos()
}

/// The weight of a frame at distance `u` (formula, doubles).
fn weight(u: f64, a: f64, n: usize) -> f64 {
    let s = if u == 0.0 { a / PI } else { (a * u).sin() / (PI * u) };
    s * window(u / n as f64)
}

/// Polyphase rows of the plain kernel (ratio <= 1, N = 16): row `j` holds the 32 weights at fraction `j / PHASES`.
const PHASES: usize = 1024;
/// Table points per unit of `sinc` and per unit of `|u| / N` of the window.
const SINC_RES: usize = 1024;
const WIN_RES: usize = 4096;
/// The sinc table covers 0..=SINC_SPAN (`|u| / ratio` stays below 16 above a ratio of 1).
const SINC_SPAN: usize = 17;

struct Tables {
    poly: Vec<[f32; 2 * N_MIN]>,
    sinc: Vec<f32>,
    win: Vec<f32>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let poly = (0..=PHASES)
            .map(|j| {
                let f = j as f64 / PHASES as f64;
                let mut row = [0.0f32; 2 * N_MIN];
                for (t, w) in row.iter_mut().enumerate() {
                    *w = weight(f + (N_MIN - 1) as f64 - t as f64, PI, N_MIN) as f32;
                }
                row
            })
            .collect();
        let sinc = (0..=SINC_SPAN * SINC_RES + 1)
            .map(|i| {
                let x = i as f64 / SINC_RES as f64;
                if x == 0.0 { 1.0 } else { ((PI * x).sin() / (PI * x)) as f32 }
            })
            .collect();
        let win = (0..=WIN_RES + 1).map(|i| window((i as f64 / WIN_RES as f64).min(1.0)) as f32).collect();
        Tables { poly, sinc, win }
    })
}

/// Linear interpolation in `table` at `x` table points.
#[inline]
fn lerp(table: &[f32], x: f32) -> f32 {
    let i = x as usize;
    match (table.get(i), table.get(i + 1)) {
        (Some(&a), Some(&b)) => a + (b - a) * (x - i as f32),
        _ => 0.0,
    }
}

/// The `2 n` weights of a read at fraction `f` (0 <= f < 1) past frame `k0`, for ratio `ratio` and `n` =
/// [`half_width`]: weight `t` belongs to frame `k0 - n + 1 + t`.
pub fn weights(f: f64, ratio: f64, n: usize, quality: Quality, w: &mut [f32]) {
    let w = &mut w[..2 * n];
    let ratio = ratio.abs();
    match quality {
        Quality::Table if n == N_MIN && ratio <= 1.0 => {
            let t = tables();
            let x = f * PHASES as f64;
            let j = (x as usize).min(PHASES - 1);
            let fr = (x - j as f64) as f32;
            let (a, b) = (&t.poly[j], &t.poly[j + 1]);
            for i in 0..2 * N_MIN {
                w[i] = a[i] + (b[i] - a[i]) * fr;
            }
        }
        Quality::Table => {
            // sinc(u / ratio) / ratio * window(u / N), from the two tables.
            let t = tables();
            let r = ratio.max(1.0);
            let (su, wu) = ((SINC_RES as f64 / r) as f32, (WIN_RES as f64 / n as f64) as f32);
            let scale = (1.0 / r) as f32;
            let u0 = (f + (n - 1) as f64) as f32;
            for (i, w) in w.iter_mut().enumerate() {
                let u = (u0 - i as f32).abs();
                *w = lerp(&t.sinc, u * su) * scale * lerp(&t.win, u * wu);
            }
        }
        Quality::Exact => {
            // sin(a u) and cos(pi u / N) stepped by one frame per tap: two rotations instead of three functions.
            let a = if ratio > 1.0 { PI / ratio } else { PI };
            let b = PI / n as f64;
            let u0 = f + (n - 1) as f64;
            let (mut s, mut c) = (a * u0).sin_cos();
            let (mut hs, mut h) = (b * u0).sin_cos();
            let (sa, ca) = a.sin_cos();
            let (sb, cb) = b.sin_cos();
            for (i, w) in w.iter_mut().enumerate() {
                let u = u0 - i as f64;
                // The two taps next to the position are worked out directly: stepped sines lose their last digits
                // there, where the quotient is largest.
                let sinc = match u.abs() {
                    0.0 => a / PI,
                    d if d < 1.0 => (a * u).sin() / (PI * u),
                    _ => s / (PI * u),
                };
                *w = (sinc * (0.18 + 0.5 * h + 0.32 * h * h)) as f32;
                // u -> u - 1
                (s, c) = (s * ca - c * sa, c * ca + s * sa);
                (hs, h) = (hs * cb - h * sb, h * cb + hs * sb);
            }
        }
    }
}

/// One stereo frame read at source position `pos` (>= 0) and ratio `ratio`. Frames before the first read the first
/// frame and frames past the last read the last one. At a ratio of 1 on a whole position the frame comes out as it is.
pub fn read(left: &[f32], right: &[f32], pos: f64, ratio: f64, quality: Quality) -> (f32, f32) {
    let len = left.len().min(right.len());
    if len == 0 {
        return (0.0, 0.0);
    }
    let k0 = pos.max(0.0).floor();
    let f = pos.max(0.0) - k0;
    let k0 = k0 as i64;
    if f == 0.0 && ratio.abs() == 1.0 {
        let i = (k0 as usize).min(len - 1);
        return (left[i], right[i]);
    }
    let n = half_width(ratio);
    let mut small = [0.0f32; 2 * N_MIN];
    let mut big;
    let w: &mut [f32] = if n == N_MIN {
        &mut small
    } else {
        big = [0.0f32; 2 * N_MAX];
        &mut big[..2 * n]
    };
    weights(f, ratio, n, quality, w);
    let first = k0 - n as i64 + 1;
    let (mut l, mut r) = (0.0f32, 0.0f32);
    if first >= 0 && (first as usize) + 2 * n <= len {
        let s = first as usize;
        for ((&w, &a), &b) in w.iter().zip(&left[s..s + 2 * n]).zip(&right[s..s + 2 * n]) {
            l += a * w;
            r += b * w;
        }
    } else {
        for (t, &w) in w.iter().enumerate() {
            let i = (first + t as i64).clamp(0, len as i64 - 1) as usize;
            l += left[i] * w;
            r += right[i] * w;
        }
    }
    (l, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(f: f64, ratio: f64) -> Vec<f64> {
        let n = half_width(ratio);
        let a = if ratio > 1.0 { PI / ratio } else { PI };
        (0..2 * n).map(|t| weight(f + (n - 1) as f64 - t as f64, a, n)).collect()
    }

    #[test]
    fn frames_either_side_grow_with_the_ratio() {
        assert_eq!(half_width(0.5), 16);
        assert_eq!(half_width(16.5), 16);
        assert_eq!(half_width(17.0), 20);
        assert_eq!(half_width(20.5), 20);
        assert_eq!(half_width(24.5), 24);
        assert_eq!(half_width(32.0), 32);
        assert_eq!(half_width(47.9), 48);
        assert_eq!(half_width(10000.0), 512);
    }

    #[test]
    fn both_qualities_follow_the_formula() {
        for &ratio in &[0.25, 0.9187, 1.0, 1.0884, 1.5, 2.0, 3.7, 8.0, 16.4, 23.0] {
            let n = half_width(ratio);
            for &f in &[0.0, 1e-9, 0.123, 0.5, 0.77, 0.999_999] {
                let want = exact(f, ratio);
                for q in [Quality::Exact, Quality::Table] {
                    let mut w = vec![0.0f32; 2 * n];
                    weights(f, ratio, n, q, &mut w);
                    let err = w.iter().zip(&want).map(|(a, b)| (f64::from(*a) - b).abs()).fold(0.0, f64::max);
                    let tol = if q == Quality::Exact { 1e-7 } else { 1e-6 };
                    assert!(err < tol, "{q:?} ratio {ratio} f {f}: off by {err}");
                }
            }
        }
    }

    #[test]
    fn a_constant_reads_as_itself_and_a_tone_keeps_its_level() {
        let one = vec![0.5f32; 4000];
        for q in [Quality::Table, Quality::Exact] {
            for &(pos, ratio) in &[(100.25, 0.9187), (2000.5, 1.0), (1500.7, 1.7), (1500.1, 3.0)] {
                let (l, r) = read(&one, &one, pos, ratio, q);
                assert!((l - 0.5).abs() < 2e-3 && l == r, "{q:?} {pos} {ratio}: {l}");
            }
            // Edges hold the first and last frames.
            let (l, _) = read(&one, &one, 0.3, 0.9187, q);
            assert!((l - 0.5).abs() < 2e-3);
            let (l, _) = read(&one, &one, 3999.6, 0.9187, q);
            assert!((l - 0.5).abs() < 2e-3);
        }
        // A 1 kHz tone at 48 kHz read between its samples.
        let tone: Vec<f32> = (0..4000).map(|i| (i as f64 * 2.0 * PI * 1000.0 / 48000.0).sin() as f32).collect();
        for q in [Quality::Table, Quality::Exact] {
            for &p in &[1000.5, 1234.25, 2000.9] {
                let (l, _) = read(&tone, &tone, p, 0.9187, q);
                let want = (p * 2.0 * PI * 1000.0 / 48000.0).sin() as f32;
                assert!((l - want).abs() < 2e-3, "{q:?} at {p}: {l} vs {want}");
            }
        }
    }

    #[test]
    fn a_whole_position_at_ratio_one_is_the_frame() {
        let x: Vec<f32> = (0..100).map(|i| i as f32 * 0.01).collect();
        assert_eq!(read(&x, &x, 42.0, 1.0, Quality::Table), (0.42, 0.42));
        assert_eq!(read(&x, &x, 42.0, 1.0, Quality::Exact), (0.42, 0.42));
    }
}
