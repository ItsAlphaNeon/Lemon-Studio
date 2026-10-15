// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon Studio's limiter, configured from Fruity Limiter saved state (minimum version:
//! input gain + peak limiter at the ceiling; compressor and gate sections not yet modeled).
//!
// spec: plugins/fruity-limiter.md

use super::{field, Effect};

pub struct Limiter {
    gain: f32,
    ceiling: f32,
    release_coef: f32,
    env: f32,
}

/// `(base^x − 1)/(base − 1)`: the amplitude curve family used by the limiter's level knobs.
fn curve(base: f64, x: f64) -> f64 {
    (base.powf(x.max(0.0)) - 1.0) / (base - 1.0)
}

impl Limiter {
    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<Self> {
        let param = |k: usize| field(state, k + 1);
        let gain = curve(7.0, f64::from(param(0)?) / 1000.0);
        let ceiling = curve(3.0, f64::from(param(2)?) / 1000.0);
        let release_ms = 2000.0 * (500f64.powf(f64::from(param(5)?) / 10000.0) - 1.0) / 499.0;
        let release_samples = (release_ms / 1000.0 * f64::from(sample_rate)).max(1.0);
        Some(Limiter {
            gain: gain as f32,
            ceiling: ceiling.max(1e-6) as f32,
            release_coef: (1.0 - (-1.0 / release_samples).exp()) as f32,
            env: 1.0,
        })
    }
}

impl Effect for Limiter {
    fn reset(&mut self) {
        self.env = 1.0;
    }

    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.chunks_exact_mut(2) {
            let l = f[0] * self.gain;
            let r = f[1] * self.gain;
            let peak = l.abs().max(r.abs());
            let target = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
            // Instant attack (never exceeds the ceiling), exponential release.
            self.env = if target < self.env { target } else { self.env + (target - self.env) * self.release_coef };
            f[0] = l * self.env;
            f[1] = r * self.env;
        }
    }
}
