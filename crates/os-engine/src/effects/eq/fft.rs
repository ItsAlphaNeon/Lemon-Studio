// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A plain radix-2 complex FFT in doubles, unscaled both ways, for LIN's kernels and convolution. The page leaves the
//! transform open ("any other FFT gives the same result up to single-precision rounding"); doubles rounded to single
//! where LIN holds singles is the variant it measured closest to FL's.
//!
// spec: plugins/fruity-parametric-eq-2.md ("LIN": "The transforms")

/// The transform of one power-of-two length, its twiddles and bit-reversed order worked out once.
pub struct Fft {
    n: usize,
    rev: Vec<u32>,
    cos: Vec<f64>,
    sin: Vec<f64>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two() && n >= 2, "FFT length {n}");
        let bits = n.trailing_zeros();
        let rev = (0..n as u32).map(|i| i.reverse_bits() >> (32 - bits)).collect();
        let (sin, cos) = (0..n / 2).map(|k| (std::f64::consts::TAU * k as f64 / n as f64).sin_cos()).unzip();
        Fft { n, rev, cos, sin }
    }

    /// In place: the forward transform (`e^-i`), or with `inverse` the inverse one (`e^+i`), neither scaled.
    pub fn run(&self, re: &mut [f64], im: &mut [f64], inverse: bool) {
        let n = self.n;
        for i in 0..n {
            let j = self.rev[i] as usize;
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let sign = if inverse { -1.0 } else { 1.0 };
        let mut len = 2;
        while len <= n {
            let (half, step) = (len / 2, n / len);
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (c, s) = (self.cos[k * step], self.sin[k * step] * sign);
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * c + im[b] * s;
                    let ti = im[b] * c - re[b] * s;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_definition_and_inverts() {
        let n = 16;
        let fft = Fft::new(n);
        let x: Vec<(f64, f64)> = (0..n).map(|i| ((i as f64 * 0.37).sin() + 0.1 * i as f64, (i as f64 * 1.3).cos())).collect();
        let (mut re, mut im): (Vec<f64>, Vec<f64>) = x.iter().copied().unzip();
        fft.run(&mut re, &mut im, false);
        for k in 0..n {
            let (mut sr, mut si) = (0.0, 0.0);
            for (t, &(xr, xi)) in x.iter().enumerate() {
                let (s, c) = (-std::f64::consts::TAU * (k * t) as f64 / n as f64).sin_cos();
                sr += xr * c - xi * s;
                si += xr * s + xi * c;
            }
            assert!((re[k] - sr).abs() < 1e-12 && (im[k] - si).abs() < 1e-12, "bin {k}");
        }
        fft.run(&mut re, &mut im, true);
        for (t, &(xr, xi)) in x.iter().enumerate() {
            assert!((re[t] / n as f64 - xr).abs() < 1e-14 && (im[t] / n as f64 - xi).abs() < 1e-14);
        }
    }
}
