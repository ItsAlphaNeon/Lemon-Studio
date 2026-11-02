// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The shared arithmetic of `pages/index.md`: float32 with one rounding per operation, the CPU's approximate reciprocal, the
//! fast 2^x, the soft clip, and the generators of `pages/voice.md`. Rust does not contract or reorder float operations, so an
//! expression here means what the page writes.

use core::arch::x86_64::*;
use core::ops::{Add, BitAnd, Mul, Neg, Sub};

/// The CPU's approximate reciprocal.
#[inline(always)]
pub fn rcp(x: f32) -> f32 {
    unsafe { _mm_cvtss_f32(_mm_rcp_ss(_mm_set_ss(x))) }
}

/// The reciprocal refined by one Newton step, as written: r - (r*x - 1)*r.
#[inline(always)]
pub fn rcpn(x: f32) -> f32 {
    let r = rcp(x);
    r - (r * x - 1.0) * r
}

/// The fast 2^x. Its polynomial is part of the sound.
#[inline(always)]
pub fn exp2(x: f32) -> f32 {
    let t = (x - 0.5) + 12582912.0;
    let k = (t.to_bits() as i32).wrapping_sub(0x4b40_0000);
    let f = x - k as f32;
    let p = ((f * 0.079440236 + 0.22449434) * f + 0.69606566) * f + 1.0;
    f32::from_bits(p.to_bits().wrapping_add((k as u32).wrapping_shl(23)))
}

/// pmin(a, b): b if b < a, else a. The first argument wins ties and NaN.
#[inline(always)]
pub fn pmin(a: f32, b: f32) -> f32 {
    if b < a {
        b
    } else {
        a
    }
}

/// pmax(a, b): b if b > a, else a.
#[inline(always)]
pub fn pmax(a: f32, b: f32) -> f32 {
    if b > a {
        b
    } else {
        a
    }
}

/// The soft clip T(x).
#[inline(always)]
pub fn tclip(x: f32) -> f32 {
    let a = if x < 3.2825 { x } else { 3.2825 };
    let c = if a > -3.2825 { a } else { -3.2825 };
    let q = c * c;
    rcpn(q * 9.0 + 27.0) * ((q + 27.0) * c)
}

/// Pitch to frequency: 69 is 110 Hz.
#[inline(always)]
pub fn hz(v: f32) -> f32 {
    exp2((v - 69.0) / 12.0) * 110.0
}

/// Decibels to gain, for an argument already offset by the caller.
#[inline(always)]
pub fn db_gain(x: f32) -> f32 {
    if x < -120.0 {
        0.0
    } else {
        exp2(x * 0.16562499)
    }
}

/// A float32 argument evaluated in double precision and rounded to float32.
#[inline]
pub fn tan_d(x: f32) -> f32 {
    (x as f64).tan() as f32
}

/// float32(1 - double(ts * c)): a one-minus coefficient "via double".
#[inline]
pub fn one_minus_via_double(ts: f32, c: f32) -> f32 {
    (1.0f64 - (ts * c) as f64) as f32
}

/// The float32 with the exponent of 1.0 and the mantissa of `bits`'s low 23 bits.
#[inline(always)]
pub fn unit_mantissa(bits: u32) -> f32 {
    f32::from_bits(0x3f80_0000 | (bits & 0x7f_ffff))
}

/// The common LCG step of `voice.md` on two 32-bit words.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lcg {
    pub a: u32,
    pub b: u32,
}

impl Lcg {
    pub const fn new(a: u32, b: u32) -> Lcg {
        Lcg { a, b }
    }

    /// One step, returning the word a.
    #[inline(always)]
    pub fn step(&mut self) -> u32 {
        self.b = self.b.wrapping_add(0x3336_a4c3);
        self.a = (self.a ^ 0x0008_c52f).wrapping_add(self.b);
        self.a
    }

    /// One step, returning u in [0, 1).
    #[inline(always)]
    pub fn unit(&mut self) -> f32 {
        unit_mantissa(self.step()) - 1.0
    }
}

/// The global multiply-with-carry generator of `voice.md`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mwc {
    pub x: u32,
    pub y: u32,
}

/// The generator's seed; the LFO modules' resets draw from it as an instance is built (`lfo.md`, Reset).
pub const MWC_SEED: Mwc = Mwc { x: 0x0056_CA39, y: 0x03E1_74BE };

impl Mwc {
    #[inline]
    pub fn unit(&mut self) -> f32 {
        self.x = (self.x & 0xffff).wrapping_mul(36969).wrapping_add(((self.x as i32) >> 16) as u32);
        self.y = (self.y & 0xffff).wrapping_mul(18000).wrapping_add(((self.y as i32) >> 16) as u32);
        unit_mantissa(self.x.wrapping_mul(65536).wrapping_add(self.y)) - 1.0
    }
}

/// Four float32 lanes. SSE is the baseline on x86-64, so these always inline.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct F4(pub __m128);

impl F4 {
    #[inline(always)]
    pub fn splat(x: f32) -> F4 {
        F4(unsafe { _mm_set1_ps(x) })
    }
    #[inline(always)]
    pub fn new(a: f32, b: f32, c: f32, d: f32) -> F4 {
        F4(unsafe { _mm_setr_ps(a, b, c, d) })
    }
    #[inline(always)]
    pub fn zero() -> F4 {
        F4(unsafe { _mm_setzero_ps() })
    }
    #[inline(always)]
    pub fn load(a: &[f32; 4]) -> F4 {
        F4(unsafe { _mm_loadu_ps(a.as_ptr()) })
    }
    #[inline(always)]
    pub fn arr(self) -> [f32; 4] {
        let mut o = [0.0f32; 4];
        unsafe { _mm_storeu_ps(o.as_mut_ptr(), self.0) };
        o
    }
    /// a < b ? a : b lane-wise.
    #[inline(always)]
    pub fn min(self, b: F4) -> F4 {
        F4(unsafe { _mm_min_ps(self.0, b.0) })
    }
    /// a > b ? a : b lane-wise.
    #[inline(always)]
    pub fn max(self, b: F4) -> F4 {
        F4(unsafe { _mm_max_ps(self.0, b.0) })
    }
    #[inline(always)]
    pub fn rcp(self) -> F4 {
        F4(unsafe { _mm_rcp_ps(self.0) })
    }
    #[inline(always)]
    pub fn rcpn(self) -> F4 {
        let r = self.rcp();
        r - (r * self - F4::splat(1.0)) * r
    }
    /// The soft clip, lane by lane; equal to [`tclip`] on each lane.
    #[inline(always)]
    pub fn tclip(self) -> F4 {
        let c = self.min(F4::splat(3.2825)).max(F4::splat(-3.2825));
        let q = c * c;
        (q * F4::splat(9.0) + F4::splat(27.0)).rcpn() * ((q + F4::splat(27.0)) * c)
    }
    #[inline(always)]
    pub fn sqrt(self) -> F4 {
        F4(unsafe { _mm_sqrt_ps(self.0) })
    }
    #[inline(always)]
    pub fn abs(self) -> F4 {
        F4(unsafe { _mm_andnot_ps(_mm_set1_ps(-0.0), self.0) })
    }
    /// True lane-wise division.
    #[inline(always)]
    pub fn div(self, o: F4) -> F4 {
        F4(unsafe { _mm_div_ps(self.0, o.0) })
    }
    /// The fast 2^x, lane by lane.
    #[inline(always)]
    pub fn exp2(self) -> F4 {
        unsafe {
            let t = _mm_add_ps(_mm_sub_ps(self.0, _mm_set1_ps(0.5)), _mm_set1_ps(12582912.0));
            let k = _mm_sub_epi32(_mm_castps_si128(t), _mm_set1_epi32(0x4b40_0000));
            let f = _mm_sub_ps(self.0, _mm_cvtepi32_ps(k));
            let f4 = F4(f);
            let p = ((f4 * F4::splat(0.079440236) + F4::splat(0.22449434)) * f4 + F4::splat(0.69606566)) * f4 + F4::splat(1.0);
            F4(_mm_castsi128_ps(_mm_add_epi32(_mm_castps_si128(p.0), _mm_slli_epi32(k, 23))))
        }
    }
}

impl Add for F4 {
    type Output = F4;
    #[inline(always)]
    fn add(self, o: F4) -> F4 {
        F4(unsafe { _mm_add_ps(self.0, o.0) })
    }
}
impl Sub for F4 {
    type Output = F4;
    #[inline(always)]
    fn sub(self, o: F4) -> F4 {
        F4(unsafe { _mm_sub_ps(self.0, o.0) })
    }
}
impl Mul for F4 {
    type Output = F4;
    #[inline(always)]
    fn mul(self, o: F4) -> F4 {
        F4(unsafe { _mm_mul_ps(self.0, o.0) })
    }
}
impl Neg for F4 {
    type Output = F4;
    #[inline(always)]
    fn neg(self) -> F4 {
        F4(unsafe { _mm_xor_ps(self.0, _mm_set1_ps(-0.0)) })
    }
}
impl BitAnd for F4 {
    type Output = F4;
    #[inline(always)]
    fn bitand(self, o: F4) -> F4 {
        F4(unsafe { _mm_and_ps(self.0, o.0) })
    }
}

/// Allpass chain step on four parallel lanes: `M` = sections + 1 states, `c` the per-lane coefficient of each section.
/// y' = ((y - s[i+1]) * c[i]) + s[i]; s[i] = y; y = y'; finally s[N] = y.
#[inline(always)]
pub fn chain_step<const M: usize>(s: &mut [F4; M], c: &[F4], w: F4) -> F4 {
    let mut y = w;
    for i in 0..M - 1 {
        let n = (y - s[i + 1]) * c[i] + s[i];
        s[i] = y;
        y = n;
    }
    s[M - 1] = y;
    y
}

/// Flush-to-zero and denormals-are-zero for the life of the guard, restoring the old modes after.
pub struct DenormalGuard(u32);

impl DenormalGuard {
    #[inline]
    pub fn new() -> DenormalGuard {
        let mut old = 0u32;
        unsafe {
            core::arch::asm!("stmxcsr [{}]", in(reg) &mut old, options(nostack));
            let new = old | (1 << 15) | (1 << 6);
            core::arch::asm!("ldmxcsr [{}]", in(reg) &new, options(nostack));
        }
        DenormalGuard(old)
    }
}

impl Drop for DenormalGuard {
    #[inline]
    fn drop(&mut self) {
        unsafe { core::arch::asm!("ldmxcsr [{}]", in(reg) &self.0, options(nostack)) };
    }
}

/// The numbers of a `pages/data` table, one row of a TSV by its first column's order.
pub fn parse_tsv(text: &str) -> (Vec<String>, Vec<Vec<f64>>) {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let head: Vec<String> = lines.next().unwrap_or("").split('\t').map(|s| s.trim().to_string()).collect();
    let rows = lines.map(|l| l.split('\t').skip(1).map(|c| c.trim().parse::<f64>().unwrap_or(0.0)).collect()).collect();
    (head, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp2_page_values() {
        assert_eq!(exp2(21.0), 2097152.25);
        assert_eq!(exp2(19.5), 741388.5625);
        assert_eq!(exp2(22.5), 5931108.5);
        assert_eq!(exp2(24.0), 16777216.0);
        assert!((exp2(1.6) - 3.0312328).abs() < 1e-6);
        assert_eq!(F4::splat(21.0).exp2().arr()[2], 2097152.25);
    }

    #[test]
    fn soft_clip_page_values() {
        assert!((tclip(0.12) - 0.11949044).abs() < 1e-6);
        assert!((tclip(4.0) - 1.0001819).abs() < 2e-6);
        let v = F4::new(0.12, -0.7, 2.0, 5.0).tclip().arr();
        assert_eq!(v[0], tclip(0.12));
        assert_eq!(v[1], tclip(-0.7));
        assert_eq!(v[3], tclip(5.0));
    }

    #[test]
    fn generators() {
        let mut g = MWC_SEED;
        for _ in 0..128 {
            g.unit();
        }
        assert_eq!(g.x, 863127029);
        let mut h = g;
        let u = h.unit();
        assert_eq!((h.x, h.y), (0x2776b4ef, 0x29d3cc92));
        assert!((u - 0.52186799).abs() < 1e-7);
        for _ in 0..64 {
            g.unit();
        }
        assert_eq!((g.x, g.y), (1247288786, 295144285));
        assert!((g.unit() - 0.5081986).abs() < 1e-6);
        let mut r = Lcg::new(0, 0);
        let u = r.unit();
        assert_eq!(r.a, 0x333f69f2);
        assert!(((u + u) - 1.0 + 0.009158611).abs() < 1e-6);
    }
}
