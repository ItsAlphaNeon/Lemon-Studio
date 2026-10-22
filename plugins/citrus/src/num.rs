// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The arithmetic the Sytrus pages name: `single( )`, Round, Trunc, the flush of tiny values, the level curve and the
//! matrix gain. Where the pages say "extended precision" Citrus works in doubles (the nearest width Rust has).
//
// spec: audio/conventions.md ("Numbers", "Level curve"), plugins/sytrus.md ("The matrix", "Rates and control blocks")

/// 2^-24: values below it in magnitude are flushed to 0.
pub const TINY: f32 = 5.960_464_5e-8;

/// Round a double to single.
#[inline]
pub fn s(x: f64) -> f32 {
    x as f32
}

/// Round to nearest, halves to even.
#[inline]
pub fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Towards zero.
#[inline]
pub fn trunc(x: f64) -> i64 {
    x.trunc() as i64
}

/// A value below 2^-24 in magnitude becomes 0.
#[inline]
pub fn flush(x: f32) -> f32 {
    if x.abs() < TINY { 0.0 } else { x }
}

/// The same for doubles, at or below the limit (the stored filter and DC states).
#[inline]
pub fn flush_le(x: f64) -> f64 {
    if x.abs() <= f64::from(TINY) { 0.0 } else { x }
}

/// The level curve in the host's form (`audio/conventions.md`), 1 at exactly 1.
#[inline]
pub fn curve(x: f32) -> f32 {
    os_dsp::level::curve(x)
}

/// A matrix cell's gain: `sign(v) * curve(|single(v) * single(1 / 256)|)`.
pub fn cell_gain(v: i32) -> f32 {
    let g = curve((v as f32 * (1.0f32 / 256.0)).abs());
    if v < 0 { -g } else { g }
}

/// `sign(v) * curve(|single(v) * single(1 / 128)|)`: the master LFO level and other signed levels over 128.
pub fn signed_curve128(v: i32) -> f32 {
    let g = curve((v as f32 * (1.0f32 / 128.0)).abs());
    if v < 0 { -g } else { g }
}

/// The plugin library's random generator: `seed * 0x08088405 + 1` modulo 2^32.
#[inline]
pub fn lcg(seed: u32) -> u32 {
    seed.wrapping_mul(0x0808_8405).wrapping_add(1)
}

/// The exponential curve of the curves page with strength `v` (`plugins/image-line-curves.md`, "The exponential
/// curve"): `c0 = v`, `c1 = single(1 / |v|)`, `c2 = single(ln(|v| + 1))`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Expo {
    pub c0: f32,
    pub c1: f32,
    pub c2: f32,
}

impl Expo {
    pub fn new(v: f32) -> Expo {
        if v == 0.0 {
            return Expo::default();
        }
        let a = f64::from(v.abs());
        Expo { c0: v, c1: s(1.0 / a), c2: s((a + 1.0).ln()) }
    }

    /// `E(u)` in doubles: `u` (no strength), a fast start (`c0 > 0`) or a slow one (`c0 < 0`).
    #[inline]
    pub fn eval(&self, u: f64) -> f64 {
        if self.c0 == 0.0 {
            u
        } else if self.c0 > 0.0 {
            1.0 - (((1.0 - u) * f64::from(self.c2)).exp() - 1.0) * f64::from(self.c1)
        } else {
            ((u * f64::from(self.c2)).exp() - 1.0) * f64::from(self.c1)
        }
    }
}

/// A little-endian reader over a state's bytes. Reads past the end fail.
pub struct Reader<'a> {
    pub b: &'a [u8],
    pub at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Reader<'a> {
        Reader { b, at: 0 }
    }

    pub fn left(&self) -> usize {
        self.b.len().saturating_sub(self.at)
    }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.bytes(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f32(&mut self) -> Option<f32> {
        self.bytes(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn f64(&mut self) -> Option<f64> {
        self.bytes(8).map(|b| f64::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn ints(&mut self, n: usize) -> Option<Vec<i32>> {
        (0..n).map(|_| self.i32()).collect()
    }
}
