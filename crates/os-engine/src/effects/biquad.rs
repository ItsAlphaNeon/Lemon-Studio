// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Stereo biquad (transposed direct form II, f64 state) and RBJ-cookbook designs.

use std::f64::consts::PI;

#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z: [[f64; 2]; 2],
}

impl Biquad {
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Biquad { b0: b[0] / a[0], b1: b[1] / a[0], b2: b[2] / a[0], a1: a[1] / a[0], a2: a[2] / a[0], z: [[0.0; 2]; 2] }
    }

    #[inline]
    pub fn tick(&mut self, ch: usize, x: f64) -> f64 {
        let z = &mut self.z[ch];
        let y = self.b0 * x + z[0];
        z[0] = self.b1 * x - self.a1 * y + z[1];
        z[1] = self.b2 * x - self.a2 * y;
        y
    }

    /// Takes `o`'s coefficients and keeps this filter's state (a design change without a restart).
    pub fn set_coefs(&mut self, o: &Biquad) {
        let z = self.z;
        *self = *o;
        self.z = z;
    }

    pub fn reset(&mut self) {
        self.z = [[0.0; 2]; 2];
    }

    /// Magnitude of the response at `hz`.
    pub fn magnitude(&self, hz: f64, sr: f64) -> f64 {
        let w = 2.0 * PI * hz / sr;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let (nr, ni) = (self.b0 + self.b1 * c1 + self.b2 * c2, -(self.b1 * s1 + self.b2 * s2));
        let (dr, di) = (1.0 + self.a1 * c1 + self.a2 * c2, -(self.a1 * s1 + self.a2 * s2));
        ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-300)).sqrt()
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for f in buf.chunks_exact_mut(2) {
            f[0] = self.tick(0, f64::from(f[0])) as f32;
            f[1] = self.tick(1, f64::from(f[1])) as f32;
        }
    }

    fn w0(f0: f64, sr: f64) -> (f64, f64) {
        let w0 = 2.0 * PI * f0.clamp(1.0, sr * 0.49) / sr;
        (w0.cos(), w0.sin())
    }

    pub fn peaking(f0: f64, q: f64, db: f64, sr: f64) -> Self {
        let a = 10f64.powf(db / 40.0);
        let (c, s) = Self::w0(f0, sr);
        let al = s / (2.0 * q);
        Self::new([1.0 + al * a, -2.0 * c, 1.0 - al * a], [1.0 + al / a, -2.0 * c, 1.0 - al / a])
    }

    pub fn low_shelf(f0: f64, q: f64, db: f64, sr: f64) -> Self {
        let a = 10f64.powf(db / 40.0);
        let (c, s) = Self::w0(f0, sr);
        let sa = 2.0 * a.sqrt() * s / (2.0 * q);
        Self::new(
            [a * ((a + 1.0) - (a - 1.0) * c + sa), 2.0 * a * ((a - 1.0) - (a + 1.0) * c), a * ((a + 1.0) - (a - 1.0) * c - sa)],
            [(a + 1.0) + (a - 1.0) * c + sa, -2.0 * ((a - 1.0) + (a + 1.0) * c), (a + 1.0) + (a - 1.0) * c - sa],
        )
    }

    pub fn high_shelf(f0: f64, q: f64, db: f64, sr: f64) -> Self {
        let a = 10f64.powf(db / 40.0);
        let (c, s) = Self::w0(f0, sr);
        let sa = 2.0 * a.sqrt() * s / (2.0 * q);
        Self::new(
            [a * ((a + 1.0) + (a - 1.0) * c + sa), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - sa)],
            [(a + 1.0) - (a - 1.0) * c + sa, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - sa],
        )
    }

    pub fn low_pass(f0: f64, q: f64, sr: f64) -> Self {
        let (c, s) = Self::w0(f0, sr);
        let al = s / (2.0 * q);
        Self::new([(1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al])
    }

    pub fn high_pass(f0: f64, q: f64, sr: f64) -> Self {
        let (c, s) = Self::w0(f0, sr);
        let al = s / (2.0 * q);
        Self::new([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al])
    }

    pub fn band_pass(f0: f64, q: f64, sr: f64) -> Self {
        let (c, s) = Self::w0(f0, sr);
        let al = s / (2.0 * q);
        Self::new([al, 0.0, -al], [1.0 + al, -2.0 * c, 1.0 - al])
    }

    pub fn notch(f0: f64, q: f64, sr: f64) -> Self {
        let (c, s) = Self::w0(f0, sr);
        let al = s / (2.0 * q);
        Self::new([1.0, -2.0 * c, 1.0], [1.0 + al, -2.0 * c, 1.0 - al])
    }

    /// Q values of the biquad sections of an `order`-th order Butterworth filter (order even).
    pub fn butterworth_qs(order: usize) -> Vec<f64> {
        (1..=order / 2).map(|k| 1.0 / (2.0 * ((2 * k - 1) as f64 * PI / (2 * order) as f64).cos())).collect()
    }
}
