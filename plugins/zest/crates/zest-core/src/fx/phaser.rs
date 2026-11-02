// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Phaser, Stoned and Flanged (`pages/fx-phaser.md`). Left and right run in the first two lanes of `F4` where they can.

use super::shared::{lfo_hz, load2, lowpass_coefficients, store2, trunc_i32, tri, zeros, Down4, Frame, Up4, WetLp};
use crate::dsp::{exp2, pmax, pmin, F4};
use crate::params::id;
use core::arch::x86_64::*;

const SIZE: usize = 8192;

/// One stage of a Stoned chain on (L, R) lanes.
#[inline(always)]
fn stage(x: F4, x1: &mut F4, s: &mut F4, k: F4) -> F4 {
    let v = -((x + *x1) + *s);
    *x1 = x;
    let f = v.abs().min(F4::splat(1.2471));
    // the page's sg as a sign flip; v = -0 only changes the sign of a zero
    let neg = unsafe { _mm_and_ps(v.0, _mm_set1_ps(-0.0)) };
    let p = ((F4::splat(0.18) - f * F4::splat(0.16)) * f - F4::splat(0.24)) * f + F4::splat(1.0);
    let m = F4(unsafe { _mm_xor_ps((p * f).0, neg) });
    *s = m * k + *s;
    *s + x
}

/// The tap of both rings at once, on (L, R) lanes.
#[inline(always)]
fn tap2(l: &[f32; SIZE], r: &[f32; SIZE], w: u32, il: u32, ir: u32, f: F4) -> F4 {
    let m = SIZE - 1;
    let (kl, kr) = (w.wrapping_sub(1).wrapping_sub(il), w.wrapping_sub(1).wrapping_sub(ir));
    let y0 = F4::new(l[kl as usize & m], r[kr as usize & m], 0.0, 0.0);
    let y1 = F4::new(l[kl.wrapping_sub(1) as usize & m], r[kr.wrapping_sub(1) as usize & m], 0.0, 0.0);
    (y1 - y0) * f + y0
}

pub struct Phaser {
    fs: f32,
    dt: f32,
    reset: bool,
    ph: f32,
    wcur: f32,
    wstep: f32,
    counter: u32,
    acc: f32,
    bypass: bool,
    ty: u8,
    inc: f32,
    stereo: f32,
    phase: f32,
    up: Up4,
    down: Down4,
    // Stoned
    dt4: f32,
    x1: [F4; 4],
    s: [F4; 4],
    hx: F4,
    hy: F4,
    fb_mem: F4,
    c0: f32,
    c1: f32,
    gf: f32,
    lo: f32,
    range: f32,
    // Flanged
    rings: [Box<[f32; SIZE]>; 2],
    rings_dirty: bool,
    w: u32,
    lp: WetLp,
    fb: f32,
    lo2: f32,
    hi2: f32,
    depth: f32,
    ms: f32,
    off: f32,
}

impl Phaser {
    pub fn new(fs: f32) -> Phaser {
        Phaser {
            fs,
            dt: 1.0 / fs,
            reset: true,
            ph: 0.0,
            wcur: 0.0,
            wstep: 0.0,
            counter: 0,
            acc: 0.0,
            bypass: false,
            ty: 0,
            inc: 0.0,
            stereo: 0.0,
            phase: 0.0,
            up: Up4::new(),
            down: Down4::new(),
            dt4: 0.0,
            x1: [F4::zero(); 4],
            s: [F4::zero(); 4],
            hx: F4::zero(),
            hy: F4::zero(),
            fb_mem: F4::zero(),
            c0: 0.0,
            c1: 0.0,
            gf: 0.0,
            lo: 0.0,
            range: 0.0,
            rings: [zeros(), zeros()],
            rings_dirty: false,
            w: 0,
            lp: WetLp::new(),
            fb: 0.0,
            lo2: 0.0,
            hi2: 0.0,
            depth: 0.0,
            ms: 0.0,
            off: 0.0,
        }
    }

    pub fn reactivate(&mut self) {
        self.reset = true;
    }

    /// The rate function: A in Hz, and with Sync on the phase from the song position when `resync`.
    fn rate(&mut self, r: f32, sync: bool, resync: bool, bpm: f64, beats: f64) -> f32 {
        if sync {
            let mut b = r * 100.0;
            if 0.125 > b {
                b = 0.125;
            }
            if resync {
                let q = (beats / b as f64) as f32;
                self.ph = q - q.floor();
            }
            (bpm / (b * 60.0) as f64) as f32
        } else {
            lfo_hz(r)
        }
    }

    /// `il`, `ir`: the samples of the block's input rendered so far.
    #[allow(clippy::too_many_arguments)]
    pub fn setup(&mut self, raw: &[f32], o: usize, il: &[f32], ir: &[f32], bpm: f64, beats: f64) {
        self.ty = raw[id::PHASE1_TYPE + o].round().clamp(0.0, 1.0) as u8;
        self.acc = 0.0;
        self.bypass = false;
        let mut resync = false;
        if self.counter > 20 {
            let mut s = 0.0f32;
            for (a, b) in il.iter().zip(ir) {
                s = s + *a;
                s = s + *b;
            }
            if s.abs() < 1e-07 {
                self.bypass = true;
                return;
            }
            resync = true;
        }
        let sync = raw[id::PHASE1_SYNC + o] >= 0.5;
        let r = raw[id::PHASE1_RATE + o] * 0.01;
        self.phase = raw[id::PHASE1_PHASE + o] * 0.01;
        self.depth = raw[id::PHASE1_DEPTH + o] * 0.01;
        let wet = (raw[id::PHASE1_WET + o] * 0.01).clamp(-1.0, 1.0);
        let feedback = raw[id::PHASE1_FEEDBACK + o] * 0.01;
        let stereo = raw[id::PHASE1_STEREO + o];
        let c = raw[id::PHASE1_CENTER + o] * 0.02 - 1.0;
        self.dt4 = self.dt * 0.25;
        if self.ty == 0 {
            self.setup_stoned(stereo, feedback, wet, r, sync, resync, c, bpm, beats);
        } else {
            self.setup_flanged(stereo, feedback, wet, r, sync, resync, c, bpm, beats);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn setup_stoned(&mut self, stereo: f32, feedback: f32, wet: f32, r: f32, sync: bool, mut resync: bool, c: f32, bpm: f64, beats: f64) {
        let inv = 1.0f32 / 256.0;
        self.stereo = stereo * 0.01;
        let fb = feedback.clamp(-1.0, 1.0);
        if self.reset {
            self.reset = false;
            self.fb_mem = F4::zero();
            self.x1 = [F4::zero(); 4];
            self.s = [F4::zero(); 4];
            self.hx = F4::zero();
            self.hy = F4::zero();
            self.up = Up4::new();
            self.down = Down4::new();
            self.wcur = wet;
            resync = true;
        }
        self.wstep = (wet - self.wcur) * inv;
        let a = self.rate(r, sync, resync, bpm, beats);
        let dt4 = self.dt4;
        self.inc = a * (dt4 * 16.0);
        let c30 = c * 30.0;
        let m_a = pmin(1.0, a * 0.2);
        let hi = (fb * 15.0 + 115.0) + c30;
        let lo = ((m_a * 20.0 + 50.0) - fb * 12.0) + c30;
        let mid = (hi + lo) * 0.5;
        let lo = (lo - mid) * self.depth + mid;
        let hi = (hi - mid) * self.depth + mid;
        self.lo = lo;
        self.range = hi - lo;
        let h = exp2((33.0 - 69.0) / 12.0);
        let fs2 = (2.0 / dt4 as f64) as f32;
        let wh = (h * 110.0) * 6.2831855;
        let ah = (1.0 / (fs2 + wh) as f64) as f32;
        self.c0 = ah * fs2;
        self.c1 = (fs2 - wh) * ah;
        self.gf = fb * 0.9;
    }

    #[allow(clippy::too_many_arguments)]
    fn setup_flanged(&mut self, stereo: f32, feedback: f32, wet: f32, r: f32, sync: bool, mut resync: bool, c: f32, bpm: f64, beats: f64) {
        let inv = 1.0f32 / 256.0;
        self.stereo = stereo * 0.0025;
        self.fb = feedback.clamp(-0.68, 1.0);
        self.lp.set(lowpass_coefficients(122.0, self.dt), self.reset);
        if self.reset {
            self.reset = false;
            if self.rings_dirty {
                self.rings[0].fill(0.0);
                self.rings[1].fill(0.0);
                self.rings_dirty = false;
            }
            self.up = Up4::new();
            self.down = Down4::new();
            self.wcur = wet;
            resync = true;
        }
        self.wstep = (wet - self.wcur) * inv;
        let a = self.rate(r, sync, resync, bpm, beats);
        self.inc = a * ((self.dt4 * 16.0) * 4.0);
        let c2 = c * 0.5;
        let m_a = pmin(1.0, a * 0.2);
        let ms = (self.fs * 4.0) / 1000.0;
        let lo = 0.0 - c2;
        let hi = (100.0 - m_a * 84.0) * 0.01;
        let hi = hi + hi;
        self.off = (hi * 0.5 + 0.0) * ms;
        let hi = hi - c2;
        let mid = (hi + lo) * 0.5;
        self.lo2 = (lo - mid) * self.depth + mid;
        self.hi2 = (hi - mid) * self.depth + mid;
        self.ms = ms;
    }

    /// The LFO step, giving the triangle values of L and R.
    #[inline(always)]
    fn lfo_step(&mut self) -> (f32, f32) {
        let p = self.inc + self.ph;
        self.ph = if p > 1.0 { p - 1.0 } else { p };
        let wrap = |mut v: f32| {
            while v >= 1.0 {
                v = v - 1.0;
            }
            while v < 0.0 {
                v = v + 1.0;
            }
            v
        };
        let a = (self.ph - self.stereo) + self.phase;
        let b = (self.ph + self.stereo) + self.phase;
        (tri(wrap(a)), tri(wrap(b)))
    }

    pub fn chunk(&mut self, l: &mut [f32; 16], r: &mut [f32; 16]) {
        if self.bypass {
            return;
        }
        let mut x = [[0.0f32; 2]; 64];
        self.up.run(l, r, &mut x);
        if self.ty == 0 {
            // with no feedback the chain's last output adds a zero: leaving it out frees the frames of each other
            if self.gf != 0.0 {
                self.frames_stoned::<true>(&mut x);
            } else {
                self.frames_stoned::<false>(&mut x);
            }
        } else {
            self.frames_flanged(&mut x);
        }
        self.down.run(&x, l, r);
    }

    #[inline]
    fn frames_stoned<const FB: bool>(&mut self, fr: &mut [Frame; 64]) {
        let (c0, c1, gf) = (F4::splat(self.c0), F4::splat(self.c1), F4::splat(self.gf));
        let mut wcur = self.wcur;
        let wstep = self.wstep;
        let mut acc = self.acc;
        let (mut hx, mut hy, mut f) = (self.hx, self.hy, self.fb_mem);
        let [mut x10, mut x11, mut x12, mut x13] = self.x1;
        let [mut s0, mut s1, mut s2, mut s3] = self.s;
        let k_arg = self.dt4 * -6.2831855;
        for block in 0..4 {
            let (a, b) = self.lfo_step();
            let (range, lo) = (self.range, self.lo);
            let coef = |v: f32| {
                let p = range * v + lo;
                let p = pmin(155.0, p);
                let h = exp2((p - 69.0) / 12.0) * 110.0;
                1.0 - (k_arg * h).exp()
            };
            let k = F4::new(coef(a), coef(b), 0.0, 0.0);
            for i in block * 16..block * 16 + 16 {
                wcur = wstep + wcur;
                let x = load2(&fr[i]) * F4::splat(0.125);
                let y = (x - hx) * c0 + c1 * hy;
                hx = x;
                hy = y;
                let u = if FB { gf * f + y } else { y };
                let u = stage(u, &mut x10, &mut s0, k);
                let u = stage(u, &mut x11, &mut s1, k);
                let u = stage(u, &mut x12, &mut s2, k);
                f = stage(u, &mut x13, &mut s3, k);
                let w5 = wcur * 0.5;
                let d = 1.0 - w5;
                let out = (F4::splat(w5) * f + F4::splat(d) * y) * F4::splat(8.0);
                store2(out, &mut fr[i]);
                let o = out.arr();
                acc = acc + (o[1] + o[0]);
            }
        }
        self.wcur = wcur;
        self.acc = acc;
        self.hx = hx;
        self.hy = hy;
        self.fb_mem = f;
        self.x1 = [x10, x11, x12, x13];
        self.s = [s0, s1, s2, s3];
    }

    #[inline]
    fn frames_flanged(&mut self, fr: &mut [Frame; 64]) {
        self.rings_dirty = true;
        let (a, b) = self.lfo_step();
        let (lo2, hi2, depth, ms, off) = (self.lo2, self.hi2, self.depth, self.ms, self.off);
        let split = |v: f32| {
            let d = (((v - 1.0) * 0.9 + 1.0) * v * hi2 * depth + lo2) * ms;
            let tp = |x: f32| {
                let x = pmax(0.0, x);
                let i = trunc_i32(x);
                (i as u32, x - i as f32)
            };
            (tp(d), tp(d + off))
        };
        let ((io_l, fo_l), (if_l, ff_l)) = split(a);
        let ((io_r, fo_r), (if_r, ff_r)) = split(b);
        let (fo, ff) = (F4::new(fo_l, fo_r, 0.0, 0.0), F4::new(ff_l, ff_r, 0.0, 0.0));
        let fb = self.fb;
        let mut wcur = self.wcur;
        let wstep = self.wstep;
        let mut acc = self.acc;
        let mut w = self.w;
        let [ring_l, ring_r] = &mut self.rings;
        let (ring_l, ring_r) = (&mut **ring_l, &mut **ring_r);
        let mut lp = self.lp;
        for i in 0..64 {
            wcur = wstep + wcur;
            let [x_l, x_r] = fr[i];
            let y = lp.step(tap2(ring_l, ring_r, w, if_l, if_r, ff)).arr();
            // with no feedback the saturation's result is multiplied by 0
            let (wl, wr) = if fb != 0.0 {
                let s_l = (y[0] * 0.38).tanh() * 2.5;
                let s_r = (y[1] * 0.38).tanh() * 2.5;
                (x_l - s_l * fb, x_r - s_r * fb)
            } else {
                (x_l, x_r)
            };
            ring_l[w as usize & (SIZE - 1)] = wl;
            ring_r[w as usize & (SIZE - 1)] = wr;
            w = w.wrapping_add(1);
            let t = tap2(ring_l, ring_r, w, io_l, io_r, fo);
            let wa = F4::splat(wcur * 0.707);
            let g = 1.0 - wcur as f64 * 0.293;
            // the mix in double, on two lanes
            let [o_l, o_r] = unsafe {
                let dry = _mm_mul_pd(_mm_cvtps_pd(load2(&fr[i]).0), _mm_set1_pd(g));
                let wetv = _mm_cvtps_pd((wa * t).0);
                let o = F4(_mm_cvtpd_ps(_mm_add_pd(dry, wetv))).arr();
                [o[0], o[1]]
            };
            fr[i] = [o_l, o_r];
            acc = acc + (o_r + o_l);
        }
        self.lp = lp;
        self.wcur = wcur;
        self.acc = acc;
        self.w = w;
    }

    pub fn wrap_up(&mut self) {
        if self.bypass {
            return;
        }
        if self.acc.abs() < 1e-07 {
            self.counter += 1;
        } else {
            self.counter = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::DenormalGuard;
    use crate::params::{COUNT, PARAMS};

    /// With Feedback 0, the frame loop that leaves the feedback term out gives the samples of the one that has it.
    #[test]
    fn stoned_without_feedback_is_exact() {
        let _g = DenormalGuard::new();
        let raw: [f32; COUNT] = core::array::from_fn(|i| PARAMS[i].default);
        let setup = || {
            let mut p = Phaser::new(48000.0);
            p.setup(&raw, 0, &[], &[], 120.0, 0.0);
            assert_eq!(p.gf, 0.0);
            p
        };
        let (mut a, mut b) = (setup(), setup());
        let mut n = 12345u32;
        for _ in 0..400 {
            let mut x = [[0.0f32; 2]; 64];
            for f in x.iter_mut() {
                for v in f.iter_mut() {
                    n = n.wrapping_mul(1664525).wrapping_add(1013904223);
                    *v = (n >> 8) as f32 / 8388608.0 - 1.0;
                }
            }
            let mut y = x;
            a.frames_stoned::<true>(&mut x);
            b.frames_stoned::<false>(&mut y);
            assert_eq!(x, y);
        }
    }
}
