// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A project's Fruity Limiter slots, played by Lemon Limiter's whole processing (`dynamics`): gain, the compressor
//! keyed from the slot's own input or its sidechain input, the look-ahead limiter (reported as the slot's delay), the
//! noise gate and the ceiling.
//!
// spec: plugins/fruity-limiter-dsp.md "Settings", audio/sidechain.md

use super::dynamics::{Dynamics, PARAMS};
use super::{field, ints, Effect};

/// The state's sidechain input number (bytes 164..167, from version 6) and flags byte (168, from version 7).
const SIDECHAIN_FIELD: usize = 41;
const FLAGS_BYTE: usize = 168;

pub struct Limiter {
    d: Dynamics,
    key: usize,
    /// The block and the sidechain input as two sides.
    l: Vec<f32>,
    r: Vec<f32>,
    kl: Vec<f32>,
    kr: Vec<f32>,
}

impl Limiter {
    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<Self> {
        let version = field(state, 0)?;
        let p = ints::<PARAMS>(state, 1)?;
        let key = if version >= 6 { field(state, SIDECHAIN_FIELD).unwrap_or(0).max(0) as usize } else { 0 };
        let flags = if version >= 7 { state.get(FLAGS_BYTE).copied().unwrap_or(0) } else { 0 };
        Some(Limiter { d: Dynamics::new(f64::from(sample_rate), p, flags), key, l: Vec::new(), r: Vec::new(), kl: Vec::new(), kr: Vec::new() })
    }

    fn run(&mut self, buf: &mut [f32], key: Option<&[f32]>) {
        split(buf, &mut self.l, &mut self.r);
        let key = key.map(|k| {
            split(k, &mut self.kl, &mut self.kr);
            (&self.kl[..], &self.kr[..])
        });
        self.d.process(&mut self.l, &mut self.r, key);
        for (f, (l, r)) in buf.chunks_exact_mut(2).zip(self.l.iter().zip(&self.r)) {
            f[0] = *l;
            f[1] = *r;
        }
    }
}

/// Interleaved stereo as two sides.
fn split(buf: &[f32], l: &mut Vec<f32>, r: &mut Vec<f32>) {
    l.clear();
    r.clear();
    l.extend(buf.chunks_exact(2).map(|f| f[0]));
    r.extend(buf.chunks_exact(2).map(|f| f[1]));
}

impl Effect for Limiter {
    fn reset(&mut self) {
        self.d.reset();
    }

    fn process(&mut self, buf: &mut [f32]) {
        self.run(buf, None);
    }

    fn latency(&self) -> usize {
        self.d.latency()
    }

    fn key_input(&self) -> usize {
        self.key
    }

    fn process_keyed(&mut self, buf: &mut [f32], key: &[f32]) {
        self.run(buf, Some(key));
    }
}

#[cfg(test)]
mod tests {
    use super::super::dynamics::{DEFAULTS, COMP_RATIO, COMP_THRES, LIM_ATT};
    use super::super::state_of;
    use super::*;

    /// A version 7 state: `p`, a spare bank, editor bytes, the sidechain number and the flags.
    fn state(p: &[i32; PARAMS], key: i32, flags: u8) -> Vec<u8> {
        let mut v = vec![7];
        v.extend_from_slice(p);
        v.extend_from_slice(p);
        v.extend_from_slice(&[0; 4]);
        v.push(key);
        let mut s = state_of(&v);
        s.push(flags);
        s
    }

    fn sine(n: usize, amp: f32) -> Vec<f32> {
        (0..n).flat_map(|i| {
            let x = amp * (i as f32 * 0.05).sin();
            [x, x]
        })
        .collect()
    }

    #[test]
    fn reads_the_state_and_reports_the_look_ahead() {
        let s = state(&DEFAULTS, 3, 1);
        assert_eq!(s.len(), 169);
        let l = Limiter::from_state(&s, 44100).unwrap();
        assert_eq!(l.key_input(), 3);
        assert_eq!(l.latency(), Dynamics::new(44100.0, DEFAULTS, 1).latency());
        assert!(l.latency() > 0, "the default attack is a look-ahead");
        // A version 5 state has neither the number nor the flags.
        let mut old = s.clone();
        old[..4].copy_from_slice(&5i32.to_le_bytes());
        assert_eq!(Limiter::from_state(&old, 44100).unwrap().key_input(), 0);
        assert!(Limiter::from_state(&s[..40], 44100).is_none());
    }

    #[test]
    fn the_compressor_follows_the_sidechain() {
        let mut p = DEFAULTS;
        (p[COMP_THRES], p[COMP_RATIO], p[LIM_ATT]) = (300, 1000, 0);
        let s = state(&p, 1, 0);
        let input = sine(4096, 0.1);
        let run = |key: Option<Vec<f32>>| {
            let mut l = Limiter::from_state(&s, 44100).unwrap();
            let mut out = Vec::new();
            for (k, b) in input.chunks(512).enumerate() {
                let mut b = b.to_vec();
                match &key {
                    Some(key) => l.process_keyed(&mut b, &key[k * 512..k * 512 + 512]),
                    None => l.process(&mut b),
                }
                out.extend(b);
            }
            out.iter().skip(4096).map(|x| x.abs()).fold(0f32, f32::max)
        };
        let own = run(None);
        let quiet = run(Some(vec![0.0; 8192]));
        let loud = run(Some(sine(4096, 1.0)));
        assert!(own > 0.09, "a quiet input is under the threshold: {own}");
        assert!((quiet - own).abs() < 1e-3, "a silent key lowers nothing: {quiet} {own}");
        assert!(loud < own * 0.5, "a loud key lowers the input: {loud} {own}");
    }
}
