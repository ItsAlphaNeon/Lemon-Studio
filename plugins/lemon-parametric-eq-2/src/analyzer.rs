// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The spectrum behind the curves (Lemon Studio design; the original's look was not captured): an FFT of the monitored
//! signal mapped to the graph's columns, tilted by the pivot slope and smoothed over time.
//!
// spec: plugins/lemon-parametric-eq-2.md §2 "Spectrum"

use std::f32::consts::PI;

use os_engine::effects::eq::freq_hz;

/// FFT sizes by "Frequency precision" (Low .. Highest).
pub const SIZES: [usize; 4] = [2048, 4096, 8192, 16384];
/// Pivot slopes in dB/oct by index.
pub const SLOPES: [f32; 5] = [0.0, 1.5, 3.0, 4.5, 6.0];
/// How much of the old value stays per frame when the level falls, by "Time smoothing" (Fastest .. Slowest).
const FALL: [f32; 5] = [0.0, 0.45, 0.7, 0.85, 0.93];

/// Analyzer settings from the options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub precision: usize,
    pub slope: usize,
    pub smoothing: usize,
    pub average: bool,
}

#[derive(Default)]
pub struct Analyzer {
    size: usize,
    window: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    /// Smoothed level per column in dB.
    pub cols: Vec<f32>,
}

/// In-place radix-2 FFT (`re.len()` a power of two).
pub fn fft(re: &mut [f32], im: &mut [f32]) {
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
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let nr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nr;
            }
        }
        len <<= 1;
    }
}

impl Analyzer {
    /// FFT size for a precision index.
    pub fn size_for(look: &Look) -> usize {
        SIZES[look.precision.min(3)]
    }

    /// Analyses the newest samples (`samples.len()` = the FFT size) into `cols` columns of level in dB.
    pub fn update(&mut self, samples: &[f32], rate: f32, cols: usize, look: &Look) {
        let n = samples.len().next_power_of_two().min(samples.len().max(2));
        if n != self.size {
            self.size = n;
            self.window = (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos()).collect();
        }
        self.re.clear();
        self.re.extend(samples.iter().zip(&self.window).map(|(x, w)| x * w));
        self.im.clear();
        self.im.resize(n, 0.0);
        fft(&mut self.re, &mut self.im);
        // Amplitude of a full-scale sine = 1 (Hann window sums to n / 2).
        let norm = 2.0 / (n as f32 * 0.5);
        let bins = n / 2;
        let mag = |b: usize| -> f32 {
            let b = b.min(bins - 1);
            (self.re[b] * self.re[b] + self.im[b] * self.im[b]).sqrt() * norm
        };
        let slope = SLOPES[look.slope.min(4)];
        let fall = FALL[look.smoothing.min(4)];
        if self.cols.len() != cols {
            self.cols = vec![-150.0; cols];
        }
        let per_bin = rate / n as f32;
        for x in 0..cols {
            let f0 = freq_hz(x as f64 / cols as f64) as f32;
            let f1 = freq_hz((x + 1) as f64 / cols as f64) as f32;
            let (b0, b1) = ((f0 / per_bin) as usize, (f1 / per_bin).ceil() as usize);
            // Wide columns take their loudest bin; narrow ones interpolate between two bins.
            let m = if b1 > b0 + 1 {
                (b0..b1).map(mag).fold(0.0f32, f32::max)
            } else {
                let fb = (f0 + f1) * 0.5 / per_bin;
                let (i, t) = (fb.floor() as usize, fb.fract());
                mag(i) * (1.0 - t) + mag(i + 1) * t
            };
            let fc = (f0 * f1).sqrt();
            let db = 20.0 * m.max(1e-9).log10() + slope * (fc / 1000.0).log2();
            let c = &mut self.cols[x];
            *c = if look.average {
                *c + (db - *c) * (1.0 - fall).max(0.15)
            } else if db >= *c {
                db
            } else {
                db + (*c - db) * fall
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_shows_at_its_frequency() {
        let rate = 44100.0;
        let n = 4096;
        let s: Vec<f32> = (0..n).map(|i| (2.0 * PI * 1000.0 * i as f32 / rate).sin()).collect();
        let mut a = Analyzer::default();
        let look = Look { precision: 1, slope: 0, smoothing: 0, average: false };
        a.update(&s, rate, 500, &look);
        let peak = (0..500).max_by(|&x, &y| a.cols[x].total_cmp(&a.cols[y])).unwrap();
        let hz = freq_hz(peak as f64 / 500.0);
        assert!((hz - 1000.0).abs() < 40.0, "{hz}");
        assert!(a.cols[peak] > -3.0 && a.cols[peak] < 1.0, "{}", a.cols[peak]);
    }
}
