// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the effect modules share: the half-band resamplers on stereo lanes, the wet low-pass, the LFO law and small helpers
//! (`pages/fx-slots.md`, sections 6, 8, 9; `pages/fx-chorus.md`, section 7).
//!
//! The resamplers step each chain's two branches and both channels in one four-lane step: lanes (branch E left, branch E right,
//! branch O left, branch O right). Every lane is the page's float32 chain step, so the order of operations per value is unchanged.

use crate::dsp::{chain_step, exp2, F4};
use core::arch::x86_64::*;

const UE: [f32; 2] = [0.12073212, 0.66320205];
const UO: [f32; 2] = [0.39036217, 0.8907868];
const DE: [f32; 6] = [0.036681503, 0.27463177, 0.561099, 0.76974183, 0.8922608, 0.96209455];
const DO: [f32; 6] = [0.13654763, 0.42313862, 0.67754006, 0.83988965, 0.931542, 0.9878164];

/// One oversampled stereo frame: [left, right].
pub type Frame = [f32; 2];

#[inline(always)]
fn up_c() -> [F4; 2] {
    [F4::new(UE[0], UE[0], UO[0], UO[0]), F4::new(UE[1], UE[1], UO[1], UO[1])]
}

#[inline(always)]
fn down_c() -> [F4; 6] {
    core::array::from_fn(|i| F4::new(DE[i], DE[i], DO[i], DO[i]))
}

/// (a0, a1, b0, b1)
#[inline(always)]
fn low_low(a: F4, b: F4) -> F4 {
    F4(unsafe { _mm_movelh_ps(a.0, b.0) })
}

/// (a2, a3, a2, a3)
#[inline(always)]
fn high(a: F4) -> F4 {
    F4(unsafe { _mm_movehl_ps(a.0, a.0) })
}

/// A frame into the first two lanes; the others are zero.
#[inline(always)]
pub fn load2(p: &Frame) -> F4 {
    F4(unsafe { _mm_castpd_ps(_mm_load_sd(p.as_ptr() as *const f64)) })
}

/// The first two lanes into a frame.
#[inline(always)]
pub fn store2(x: F4, p: &mut Frame) {
    unsafe { _mm_store_sd(p.as_mut_ptr() as *mut f64, _mm_castps_pd(x.0)) }
}

/// Two neighbouring frames as four lanes.
#[inline(always)]
fn load_pair(p: &[Frame]) -> F4 {
    debug_assert!(p.len() >= 2);
    F4(unsafe { _mm_loadu_ps(p.as_ptr() as *const f32) })
}

#[inline(always)]
fn store_pair(x: F4, p: &mut [Frame]) {
    debug_assert!(p.len() >= 2);
    unsafe { _mm_storeu_ps(p.as_mut_ptr() as *mut f32, x.0) }
}

#[inline(always)]
fn chunk_out(o: F4, l: &mut f32, r: &mut f32) {
    let [a, b, ..] = o.arr();
    *l = a;
    *r = b;
}

#[derive(Clone, Copy)]
pub struct Up2 {
    s: [F4; 3],
}

impl Up2 {
    pub fn new() -> Up2 {
        Up2 { s: [F4::zero(); 3] }
    }

    #[inline]
    pub fn run(&mut self, l: &[f32; 16], r: &[f32; 16], x: &mut [Frame; 32]) {
        let c = up_c();
        let mut s = self.s;
        for i in 0..16 {
            let o = chain_step(&mut s, &c, F4::new(l[i], r[i], l[i], r[i]));
            store_pair(o, &mut x[2 * i..2 * i + 2]);
        }
        self.s = s;
    }
}

#[derive(Clone, Copy)]
pub struct Down2 {
    s: [F4; 7],
    q: F4,
}

impl Down2 {
    pub fn new() -> Down2 {
        Down2 { s: [F4::zero(); 7], q: F4::zero() }
    }

    #[inline]
    pub fn run(&mut self, x: &[Frame; 32], l: &mut [f32; 16], r: &mut [f32; 16]) {
        let c = down_c();
        let mut q = self.q;
        let mut s = self.s;
        for i in 0..16 {
            let o = chain_step(&mut s, &c, load_pair(&x[2 * i..2 * i + 2]));
            chunk_out((q + o) * F4::splat(0.5), &mut l[i], &mut r[i]);
            q = high(o);
        }
        self.s = s;
        self.q = q;
    }
}

#[derive(Clone, Copy)]
pub struct Up4 {
    s1: [F4; 3],
    s2: [F4; 3],
}

impl Up4 {
    pub fn new() -> Up4 {
        Up4 { s1: [F4::zero(); 3], s2: [F4::zero(); 3] }
    }

    #[inline]
    pub fn run(&mut self, l: &[f32; 16], r: &[f32; 16], x: &mut [Frame; 64]) {
        let c = up_c();
        let (mut s1, mut s2) = (self.s1, self.s2);
        for i in 0..16 {
            let a = chain_step(&mut s1, &c, F4::new(l[i], r[i], l[i], r[i]));
            let o = chain_step(&mut s2, &c, low_low(a, a));
            store_pair(o, &mut x[4 * i..4 * i + 2]);
            let o = chain_step(&mut s2, &c, high(a));
            store_pair(o, &mut x[4 * i + 2..4 * i + 4]);
        }
        self.s1 = s1;
        self.s2 = s2;
    }
}

#[derive(Clone, Copy)]
pub struct Down4 {
    b: [F4; 7],
    a: [F4; 7],
    qb: F4,
    qa: F4,
}

impl Down4 {
    pub fn new() -> Down4 {
        Down4 { b: [F4::zero(); 7], a: [F4::zero(); 7], qb: F4::zero(), qa: F4::zero() }
    }

    #[inline]
    pub fn run(&mut self, x: &[Frame; 64], l: &mut [f32; 16], r: &mut [f32; 16]) {
        let c = down_c();
        let (mut qb, mut qa) = (self.qb, self.qa);
        let (mut sb, mut sa) = (self.b, self.a);
        for i in 0..16 {
            let j = 4 * i;
            let o1 = chain_step(&mut sb, &c, load_pair(&x[j..j + 2]));
            let h0 = qb + o1;
            let o2 = chain_step(&mut sb, &c, load_pair(&x[j + 2..j + 4]));
            let h1 = high(o1) + o2;
            qb = high(o2);
            let o = chain_step(&mut sa, &c, low_low(h0, h1));
            chunk_out((qa + o) * F4::splat(0.25), &mut l[i], &mut r[i]);
            qa = high(o);
        }
        self.b = sb;
        self.a = sa;
        self.qb = qb;
        self.qa = qa;
    }
}

/// `flush(x)`: a value with exponent field 0 or 255 becomes +0.
#[inline(always)]
pub fn flush(x: f32) -> f32 {
    let e = (x.to_bits() >> 23) & 0xff;
    if e == 0 || e == 255 {
        0.0
    } else {
        x
    }
}

/// x86's truncating conversion: round toward zero.
#[inline(always)]
pub fn trunc_i32(x: f32) -> i32 {
    unsafe { _mm_cvttss_si32(_mm_set_ss(x)) }
}

/// float32 to a 64-bit integer, rounding toward zero.
#[inline(always)]
pub fn trunc_i64(x: f32) -> i64 {
    unsafe { _mm_cvttss_si64(_mm_set_ss(x)) }
}

/// Linear interpolation on a power-of-two ring, `d` behind the position `w`: the tap of the module pages.
#[inline(always)]
pub fn tap<const N: usize>(line: &[f32; N], w: u32, d: f32) -> f32 {
    let i = trunc_i32(d);
    let f = d - i as f32;
    tap_at(line, w, i as u32, f)
}

/// The same, with the delay already split into whole samples and fraction.
#[inline(always)]
pub fn tap_at<const N: usize>(line: &[f32; N], w: u32, i: u32, f: f32) -> f32 {
    let m = N - 1;
    let k = w.wrapping_sub(1).wrapping_sub(i);
    let y0 = line[k as usize & m];
    let y1 = line[k.wrapping_sub(1) as usize & m];
    (y1 - y0) * f + y0
}

/// A zeroed boxed array, without passing through the stack.
pub fn zeros<const N: usize>() -> Box<[f32; N]> {
    match vec![0.0f32; N].into_boxed_slice().try_into() {
        Ok(b) => b,
        Err(_) => unreachable!(),
    }
}

/// `lfo_hz(r)`, r in 0..1.
#[inline]
pub fn lfo_hz(r: f32) -> f32 {
    exp2((8.407278 - r * 1.1185706) * r) * 0.033333335 - r * 0.21192744
}

/// `tri(v)`, v in [0, 1).
#[inline(always)]
pub fn tri(v: f32) -> f32 {
    let v = v + v;
    if v >= 1.0 {
        2.0 - v
    } else {
        v
    }
}

/// True when every sample of the chunk is zero. With denormals-are-zero on, a denormal counts as zero, as it does in the states.
#[inline]
pub fn all_zero(l: &[f32; 16], r: &[f32; 16]) -> bool {
    let mut any = false;
    for i in 0..16 {
        any |= l[i] != 0.0;
        any |= r[i] != 0.0;
    }
    !any
}

/// `lowpass(cut, q)`, in the order b0, b1, b2, a1, a2 (`fx-chorus.md`, section 7), q = 0.71.
pub fn lowpass_coefficients(cut: f32, dt: f32) -> [f32; 5] {
    let h = exp2((cut - 69.0) / 12.0) * 110.0;
    let w = h * dt;
    let w = w + w;
    let w = w * 3.1415927;
    let ww = w as f64;
    let z = ww * ww;
    let sn = (((z * 2.7557318844628753e-06 - 0.00019841270113829523) * z + 0.008333333767950535) * z - 0.1666666716337204) * z + 1.0;
    let sn = sn * ww;
    let cs = (((((z * -2.605e-07 + 2.47609e-05) * z - 0.0013888397) * z + 0.0416666418) * z - 0.4999999963) * z) + 1.0;
    let iq = (1.0 / (0.71f32 as f64 + 0.707)) as f32;
    let alpha = iq as f64 * sn;
    let nm = 1.0 / (alpha + 1.0);
    let oc = 1.0 - cs;
    let b0 = (oc * 0.5 * nm) as f32;
    let b1 = (oc * nm) as f32;
    let a1 = (cs * -2.0 * nm) as f32;
    let a2 = ((1.0 - alpha) * nm) as f32;
    [b0, b1, b0, a1, a2]
}

/// The wet low-pass of the Chorus and the Flanged phaser on (L, R) lanes; the other two lanes run idle.
#[derive(Clone, Copy)]
pub struct WetLp {
    c: [F4; 5],
    d: [F4; 5],
    s1: F4,
    s2: F4,
    ramp: bool,
}

impl WetLp {
    pub fn new() -> WetLp {
        WetLp { c: [F4::zero(); 5], d: [F4::zero(); 5], s1: F4::zero(), s2: F4::zero(), ramp: false }
    }

    /// The ramp logic of a set-up, with `new` the coefficients just computed.
    pub fn set(&mut self, new: [f32; 5], reset: bool) {
        let inv = 1.0f32 / 256.0;
        if reset {
            self.s1 = F4::zero();
            self.s2 = F4::zero();
        }
        let mut any = false;
        for k in 0..5 {
            let old = self.c[k].arr()[0];
            let (co, de) = if reset { (new[k], 0.0) } else { (old, (old - new[k]) * inv) };
            let (co, de) = (flush(co), flush(de));
            any |= de != 0.0;
            self.c[k] = F4::splat(co);
            self.d[k] = F4::splat(de);
        }
        self.ramp = any;
        self.s1 = flush2(self.s1);
        self.s2 = flush2(self.s2);
    }

    /// One 4x sample on (L, R) lanes.
    #[inline(always)]
    pub fn step(&mut self, x: F4) -> F4 {
        if self.ramp {
            self.advance();
        }
        let [b0, b1, b2, a1, a2] = self.c;
        let y = x * b0 + self.s1 * F4::splat(0.99999);
        self.s1 = (x * b1 - a1 * y) + self.s2;
        self.s2 = b2 * x - a2 * y;
        y
    }

    /// The coefficient step alone, for frames whose audio is known to be zero.
    #[inline(always)]
    pub fn advance(&mut self) {
        for k in 0..5 {
            self.c[k] = self.c[k] - self.d[k];
        }
    }

    #[inline]
    pub fn ramping(&self) -> bool {
        self.ramp
    }

    pub fn states_zero(&self) -> bool {
        let (a, b) = (self.s1.arr(), self.s2.arr());
        a[0] == 0.0 && a[1] == 0.0 && b[0] == 0.0 && b[1] == 0.0
    }
}

fn flush2(x: F4) -> F4 {
    let a = x.arr();
    F4::new(flush(a[0]), flush(a[1]), 0.0, 0.0)
}
