// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! An operator's wave table: 16384 singles (plus a wrap entry) summed from 256 harmonics, either as pure sines or
//! as shapes (sine to triangle to saw to square to pulse) with skew, sine shaper, half wave and gate; then
//! normalised, bent by the tension, made absolute, pre-filtered. Its spectrum is kept for the band-limited copies a
//! voice takes.
//
// spec: plugins/sytrus.md ("The wave table", "Band limiting")

use std::f64::consts::PI;

use crate::num::{round, s, trunc, Expo, TINY};
use crate::state::{OpBlock, ABSOLUTE, GATE, HALF_WAVE};

pub const N: usize = 16384;

/// A table and the spectrum it keeps for band limiting (bins 0..=N/2, divided by N).
#[derive(Debug, Clone)]
pub struct Table {
    pub t: Vec<f32>,
    pub spectrum: Vec<(f64, f64)>,
}

/// Builds an operator's table. `smooth` is switch 2 (band-limit smoothing).
pub fn build(op: &OpBlock, smooth: bool) -> Table {
    let [shape, tension, skew, sine, prefilter] = op.shape;
    let half = op.flags[HALF_WAVE] != 0;
    let gate = op.flags[GATE] != 0;
    let mut t = vec![0f32; N + 1];
    if shape == 0 && skew == 0 && sine == 0 && !half && !gate {
        pure_sines(op, &mut t);
    } else {
        shapes(op, &mut t);
    }
    // 1. Normalised.
    let largest = t[..N].iter().fold(0f32, |m, &x| m.max(x.abs()));
    if largest > TINY {
        let k = s(1.0 / f64::from(largest));
        for x in t[..N].iter_mut() {
            *x *= k;
        }
    }
    // 2. Tension.
    if tension != 0 {
        let k = s(f64::from(s(((f64::from(tension.unsigned_abs() as f32 * (1.0 / 128.0))) * 21f64.ln()).exp() - 1.0)) * 0.05);
        let v = s((f64::from(k) * 1e18f64.ln()).exp() - 1.0);
        let e = Expo::new(if tension < 0 { -v } else { v });
        for x in t[..N].iter_mut() {
            let m = s(e.eval(f64::from(x.abs())));
            *x = if *x < 0.0 { -m } else { m };
        }
    }
    // 3. Absolute value.
    if op.flags[ABSOLUTE] != 0 {
        for x in t[..N].iter_mut() {
            *x = x.abs();
        }
    }
    // 4. Pre-filter: the spectrum is kept with its bins from `cut` up set to 0.
    let mut spec = forward(&t[..N]);
    let pf = prefilter as f32 * (1.0 / 128.0);
    let cut = round(f64::from(s(f64::from(s((f64::from(s(f64::from(pf) * 0.35 + 0.65)) * 1e10f64.ln()).exp() - 1.0)) * 1.6384e-06)));
    let cut = cut.max(0) as usize;
    for b in spec.iter_mut().skip(cut) {
        *b = (0.0, 0.0);
    }
    if cut <= N / 2 {
        if smooth && cut >= 2 {
            let j = cut - 1;
            let pi = std::f32::consts::PI;
            let a = j as f32 / pi;
            let step = pi / j as f32;
            for (k, b) in spec.iter_mut().enumerate().take(j + 1).skip(1) {
                let w = s(f64::from(a) * f64::from(k as f32 * step).sin() / k as f64);
                b.0 *= f64::from(w);
                b.1 *= f64::from(w);
            }
        }
        inverse(&spec, &mut t[..N]);
    }
    // 5. Flush, and the wrap entry.
    for x in t[..N].iter_mut() {
        if x.abs() < TINY {
            *x = 0.0;
        }
    }
    t[N] = t[0];
    Table { t, spectrum: spec }
}

/// The tone generator: each harmonic with a level adds a sine at `(h + 1) / N` cycles per sample.
fn pure_sines(op: &OpBlock, t: &mut [f32]) {
    for h in 0..256 {
        let l = op.levels[h];
        if l == 0.0 {
            continue;
        }
        let l = f64::from(l);
        let ph = f64::from(op.phases[h]);
        let phase = f64::from(s(PI * ((2.0 * ph + 1.5).rem_euclid(2.0))));
        let w = 2.0 * PI * (h + 1) as f64 / N as f64;
        let c = w.cos();
        let (c2, c4) = (2.0 * c, 4.0 * c * c - 2.0);
        let mut y = [0f64; 4];
        y[0] = l * phase.cos();
        y[1] = l * (phase + w).cos();
        y[2] = c2 * y[1] - y[0];
        y[3] = c2 * y[2] - y[1];
        for (n, x) in t[..N].iter_mut().enumerate() {
            let v = if n < 4 {
                y[n]
            } else {
                let v = c4 * y[(n - 2) % 4] - y[n % 4];
                y[n % 4] = v;
                v
            };
            *x += v as f32;
        }
    }
}

/// The shape's four values `(a4, a0, w, p)`.
fn shape_values(shape: i32) -> (f64, f64, f64, f64) {
    let sv = f64::from(shape as f32 * (1.0 / 128.0));
    if sv <= 1.0 {
        (0.0, 1.0, sv, 0.0)
    } else if sv <= 2.0 {
        (0.0, 2.0 - sv, 1.0, 0.0)
    } else if sv <= 3.0 {
        (sv - 2.0, 0.0, 1.0, 0.0)
    } else {
        (1.0, 0.0, 1.0, sv - 3.0)
    }
}

fn shapes(op: &OpBlock, t: &mut [f32]) {
    let [shape, _, skew, sine, _] = op.shape;
    let (a4, a0, w, p) = shape_values(shape);
    let k0 = if a0 < 2.0 { f64::from(s(2.0 / (2.0 - a0))) } else { 0.0 };
    let value = |x: f64| -> f64 {
        if p > 0.0 {
            return if x < 1.0 - p * 0.75 { 1.0 } else { -1.0 };
        }
        let mut v = (x * PI).sin() * (1.0 - w);
        if w > 0.0 {
            let mut y = x + a0 / 2.0;
            while y > 2.0 {
                y -= 2.0;
            }
            let d = if y >= a0 {
                let d = 1.0 - (y - a0) * k0;
                if a4 != 0.0 {
                    if y >= 1.0 { -a4 + d * (1.0 - a4) } else { a4 + d * (1.0 - a4) }
                } else {
                    d
                }
            } else {
                (y / a0) * 2.0 - 1.0
            };
            v += d * w;
        }
        v
    };
    let skew_e = (skew > 0).then(|| {
        let k = s(f64::from(s(((f64::from(skew as f32 * (1.0 / 128.0))) * 21f64.ln()).exp() - 1.0)) * 0.05);
        Expo::new(s((f64::from(k) * 1e9f64.ln()).exp() - 1.0))
    });
    let shaper = (sine > 0).then(|| f64::from(s(f64::from(sine) * 0.036_815_54)));
    let half = op.flags[HALF_WAVE] != 0;
    let gate = op.flags[GATE] != 0;
    for h in 0..256 {
        let l = op.levels[h];
        if l == 0.0 {
            continue;
        }
        let l = f64::from(l);
        let mut step = 2.0 * (h + 1) as f64 / N as f64;
        let mut wrap = 2.0;
        if half {
            step *= 0.5;
            wrap = 1.0;
        }
        let mut used = wrap;
        if gate {
            step *= 2.0;
            wrap *= 2.0;
            used = wrap / 2.0;
        }
        let mut x = f64::from(op.phases[h]) * 2.0;
        for e in t[..N].iter_mut() {
            while x > wrap {
                x -= wrap;
            }
            let mut v = if x > used {
                0.0
            } else {
                let pos = match &skew_e {
                    Some(se) => {
                        let m = se.eval((x - 1.0).abs());
                        if x <= 1.0 { 1.0 - m } else { m + 1.0 }
                    }
                    None => x,
                };
                value(pos)
            };
            if let Some(k) = shaper {
                v = (v * k).sin();
            }
            *e = s(f64::from(*e) + v * l);
            x += step;
        }
    }
}

/// A band-limited copy of `table` for an operator at `hz` (spec "Band limiting"); `None` when the full table serves.
pub fn band_limited(table: &Table, hz: f32, rate: u32, smooth: bool) -> Option<Vec<f32>> {
    let hz = f64::from(hz.max(0.1));
    let top = if smooth { 22050.0 } else { f64::from(s(f64::from(rate) * 0.5)) };
    let d = (top - 18000.0) * 0.25;
    let ihz = f64::from(s(1.0 / hz));
    let lo = trunc(f64::from(s((d + 18000.0) * ihz))) + 1;
    let hi = trunc(f64::from(s((d * 3.0 + 18000.0) * ihz))) + 1;
    if lo > 8192 {
        return None;
    }
    let (lo, hi) = (lo.max(0) as usize, (hi.max(0) as usize).min(8192));
    let q = s(-1.0 / (hi as f64 - lo as f64 + 1.0));
    let mut spec = table.spectrum.clone();
    let mut weights = vec![0f32; spec.len()];
    for (k, w) in weights.iter_mut().enumerate() {
        *w = if k < lo {
            1.0
        } else if k <= hi {
            s(f64::from(1.0f32 + q) + f64::from(q) * (k - lo) as f64)
        } else {
            0.0
        };
    }
    if smooth && hi >= 1 {
        let pi = std::f32::consts::PI;
        let step = pi / hi as f32;
        let ipi = s(1.0 / PI);
        for (k, w) in weights.iter_mut().enumerate().take(hi + 1).skip(1) {
            let a = *w * hi as f32;
            let a = a * ipi;
            *w = s(f64::from(a) * f64::from(k as f32 * step).sin() / k as f64);
        }
    }
    for (b, w) in spec.iter_mut().zip(&weights) {
        b.0 *= f64::from(*w);
        b.1 *= f64::from(*w);
    }
    let mut t = vec![0f32; N + 1];
    inverse(&spec, &mut t[..N]);
    for x in t[..N].iter_mut() {
        if x.abs() < TINY {
            *x = 0.0;
        }
    }
    t[N] = t[0];
    Some(t)
}

/// The forward transform of a real table, divided by `N`: bins 0..=N/2.
fn forward(x: &[f32]) -> Vec<(f64, f64)> {
    let mut re: Vec<f64> = x.iter().map(|&v| f64::from(v)).collect();
    let mut im = vec![0f64; N];
    fft(&mut re, &mut im, false);
    let k = 1.0 / N as f64;
    (0..=N / 2).map(|b| (re[b] * k, im[b] * k)).collect()
}

/// The inverse transform of bins 0..=N/2 (the rest mirrored) into a real table.
fn inverse(spec: &[(f64, f64)], out: &mut [f32]) {
    let mut re = vec![0f64; N];
    let mut im = vec![0f64; N];
    for (b, &(r, i)) in spec.iter().enumerate() {
        re[b] = r;
        im[b] = i;
        if b != 0 && b != N / 2 {
            re[N - b] = r;
            im[N - b] = -i;
        }
    }
    fft(&mut re, &mut im, true);
    for (o, r) in out.iter_mut().zip(re) {
        *o = r as f32;
    }
}

/// An in-place radix-2 transform of `N` points (`inverse`: the conjugate kernel, not scaled).
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let ang = sign * 2.0 * PI / len as f64;
        let half = len / 2;
        let tw: Vec<(f64, f64)> = (0..half).map(|k| ((ang * k as f64).cos(), (ang * k as f64).sin())).collect();
        for start in (0..n).step_by(len) {
            for k in 0..half {
                let (wr, wi) = tw[k];
                let (a, b) = (start + k, start + k + half);
                let xr = re[b] * wr - im[b] * wi;
                let xi = re[b] * wi + im[b] * wr;
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_harmonic_is_a_normalised_sine() {
        let mut op = OpBlock::default();
        op.levels[0] = 1.0;
        op.shape[4] = 128;
        let tb = build(&op, false);
        let peak = tb.t[..N].iter().fold(0f32, |m, &x| m.max(x.abs()));
        assert!((peak - 1.0).abs() < 1e-6, "{peak}");
        // Phase 0 starts the sine at 0 rising.
        assert!(tb.t[0].abs() < 1e-3 && tb.t[N / 4] > 0.99, "{} {}", tb.t[0], tb.t[N / 4]);
    }

    #[test]
    fn the_transform_round_trips() {
        let x: Vec<f32> = (0..N).map(|k| ((k * 7 % 13) as f32 - 6.0) * 0.1).collect();
        let spec = forward(&x);
        let mut y = vec![0f32; N];
        inverse(&spec, &mut y);
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-5));
    }
}
