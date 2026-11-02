// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The high-pass module (`pages/hpf.md`).

use crate::dsp::{exp2, hz, tan_d, tclip, unit_mantissa, Lcg, F4};
use crate::filter::solver::{SolveRun, Solver};
use crate::filter::{os_coefs, FilterTables, Resamplers};
use crate::settings::Settings;

/// What HPF pre and BITE read in a call. `rows` are the voice's own.
pub struct HpfCtx<'a> {
    pub s: &'a Settings,
    pub ft: &'a FilterTables,
    pub acc: i32,
    pub rows: &'a [f32; 448],
    pub c: usize,
    pub n: usize,
}

const PI: f32 = 3.1415927;

/// `hpf.md` section 4: one generator per kernel per voice, from 0.
#[derive(Clone, Copy, Default)]
struct Noise(Lcg);

impl Noise {
    #[inline(always)]
    fn step(&mut self) -> f32 {
        let u = unit_mantissa(self.0.step());
        ((u - 1.0) + 0.0) * 0.0005
    }
}

/// Coef(h) of section 5.
#[inline]
fn coef_pre(ts: f32, h: f32) -> (f32, f32) {
    let t = tan_d(((ts * h) * PI) * 0.5) * 4.0;
    ((t - 1.0) / (t + 1.0), t / (t + 1.0))
}

/// HPF pre (`hpf.md` section 5), per voice.
pub struct HpfPre {
    noise: Noise,
    a: f32,
    b: f32,
    sa: f32,
    sb: f32,
    fq: f32,
    wp: f32,
    y1: f32,
    flagged: bool,
    last_fl: f32,
    last_c: (f32, f32),
}

impl HpfPre {
    /// `OPT/TransM` above 1 at a note starting a stopped voice.
    #[inline]
    pub fn flag_clear(&mut self) {
        self.flagged = true;
    }

    pub fn new() -> Box<HpfPre> {
        Box::new(HpfPre {
            noise: Noise::default(),
            a: 0.0,
            b: 0.0,
            sa: 0.0,
            sb: 0.0,
            fq: 0.0,
            wp: 0.0,
            y1: 0.0,
            flagged: true,
            last_fl: f32::NAN,
            last_c: (0.0, 0.0),
        })
    }

    /// The module ran under another HPF model this call: a change of HPF/Model clears this kernel for its next run.
    #[inline]
    pub fn note_other_model(&mut self) {
        self.flagged = true;
    }

    /// In place on the signal row `a`; `b` passes.
    pub fn run(&mut self, x: &HpfCtx, a: &mut [f32], _b: &mut [f32]) {
        let ns = 16 * x.n;
        let (ts, g) = (x.ft.ts, x.ft.g);
        let f_n = (1.0f64 / ns as f64) as f32 * 0.25;
        let ft = x.s.hpf.frequency;
        if core::mem::take(&mut self.flagged) {
            self.wp = 0.0;
            self.y1 = 0.0;
            self.fq = ft;
            let (ca, cb) = coef_pre(ts, hz(ft));
            self.a = ca;
            self.b = cb;
            self.sa = 0.0;
            self.sb = 0.0;
        }
        let f0 = self.fq;
        let step = (ft - f0) * f_n;
        for (i, s) in a[..ns].iter_mut().enumerate() {
            if i & 3 == 0 {
                let fl = if self.fq < 150.0 { self.fq } else { 150.0 };
                let fl = if fl > 30.0 { fl } else { 30.0 };
                if fl != self.last_fl {
                    self.last_fl = fl;
                    self.last_c = coef_pre(ts, hz(fl));
                }
                self.sa = (self.last_c.0 - self.a) * 0.0625;
                self.sb = (self.last_c.1 - self.b) * 0.0625;
            }
            self.b = self.b + self.sb;
            self.a = self.a + self.sa;
            self.fq = step + self.fq;
            let xx = g * (self.noise.step() + *s);
            let w = 0.0 - xx;
            let y = ((w + self.wp) * self.b) - (self.y1 * self.a);
            self.wp = w;
            self.y1 = y;
            *s = ((y + xx) * g) * 16.0;
        }
    }
}

/// The five coefficients of HPF BITE: c2, c3, the feedback high-pass pair, c8.
#[derive(Clone, Copy, Default, PartialEq)]
struct BiteC {
    c2: f32,
    c3: f32,
    p: f32,
    q: f32,
    c8: f32,
}

/// Coef_1(h, r).
#[inline]
fn coef_1(fs: f32, ts: f32, h: f32, r: f32) -> BiteC {
    let f6 = h * 0.68032527;
    let t = tan_d((((f6 * 3.0) * ts) * PI) * 0.5) * 4.0;
    let c8 = (r as f64 * 1.4025) as f32;
    let f3 = ((fs as f64 + fs as f64) * 4.0) as f32;
    let w = (((((f6 * 5.0) as f64) * 0.9) * 2.0) * PI as f64) as f32;
    let g = 1.0 / (w + f3);
    BiteC { c2: (t - 1.0) / (t + 1.0), c3: t / (t + 1.0), p: g * f3, q: (f3 - w) * g, c8 }
}

/// Coef_2(h, r).
#[inline]
fn coef_2(fs: f32, ts: f32, h: f32, r: f32) -> BiteC {
    let f3 = h * 0.20905685;
    let t = tan_d((((f3 * 3.0) * ts) * PI) * 0.5) * 4.0;
    let c8 = r * 5.5;
    let w = (f3 * 5.0) * 10.0;
    let w = (w + w) * PI;
    let f1 = ((fs as f64 + fs as f64) * 4.0) as f32;
    let g = (1.0f64 / (w + f1) as f64) as f32;
    BiteC { c2: (t - 1.0) / (t + 1.0), c3: t / (t + 1.0), p: g * f1, q: (f1 - w) * g, c8 }
}

/// HPF BITE (`hpf.md` sections 7, 8), per voice.
pub struct HpfBite {
    noise: Noise,
    res: Resamplers,
    sol: Solver,
    c: BiteC,
    d: BiteC,
    fq: f32,
    rq: f32,
    cp: f32,
    v1p: f32,
    ep: f32,
    yp: f32,
    f2: f32,
    flagged: bool,
    last_rev: bool,
    last_acc: i32,
    last_in: (f32, f32),
    last_t: BiteC,
}

impl HpfBite {
    /// `OPT/TransM` above 1 at a note starting a stopped voice.
    #[inline]
    pub fn flag_clear(&mut self) {
        self.flagged = true;
    }

    pub fn new() -> Box<HpfBite> {
        Box::new(HpfBite {
            noise: Noise::default(),
            res: Resamplers::new(),
            sol: Solver::default(),
            c: BiteC::default(),
            d: BiteC::default(),
            fq: 0.0,
            rq: 0.0,
            cp: 0.0,
            v1p: 0.0,
            ep: 0.0,
            yp: 0.0,
            f2: 0.0,
            flagged: true,
            last_rev: false,
            last_acc: -1,
            last_in: (f32::NAN, f32::NAN),
            last_t: BiteC::default(),
        })
    }

    /// The module ran under another HPF model this call: a change of HPF/Model clears this kernel for its next run.
    #[inline]
    pub fn note_other_model(&mut self) {
        self.flagged = true;
    }

    /// In place on the signal row `a`; `b` goes through the same resamplers.
    pub fn run(&mut self, x: &HpfCtx, a: &mut [f32], b: &mut [f32]) {
        let ft = x.ft;
        let h = &x.s.hpf;
        let rev1 = h.revision == 1;
        let (fs, ts, g) = (ft.fs, ft.ts, ft.g);
        let ns = 16 * x.n;
        let draft = x.acc == 0;
        let o = if x.acc >= 2 { 4 } else { 2 };
        let scale_h = if o == 4 { 0.25f32 } else { 0.5 };
        let f = (1.0f64 / ns as f64) as f32 * 0.25;
        let f_t = h.frequency;
        let r_t = h.resonance * 0.01;
        self.sol.reset_call();
        if core::mem::take(&mut self.flagged) || rev1 != self.last_rev || x.acc != self.last_acc {
            self.res = Resamplers::new();
            self.fq = f_t;
            self.rq = r_t;
            let h0 = hz(f_t);
            self.c = if rev1 { coef_1(fs, ts, h0, self.rq) } else { coef_2(fs, ts, h0, self.rq) };
            self.d = BiteC::default();
            self.cp = 0.0;
            self.v1p = 0.0;
            self.ep = 0.0;
            self.yp = 0.0;
            self.f2 = 0.0;
            self.last_in = (f32::NAN, f32::NAN);
        }
        self.last_rev = rev1;
        self.last_acc = x.acc;
        let df = (f_t - self.fq) * f;
        let dr = (r_t - self.rq) * f;
        let co = os_coefs();
        let (mut fa, mut fb) = ([0.0f32; 256], [0.0f32; 256]);
        let mut fo = [0.0f32; 256];
        self.res.up(&co, o, &a[..ns], &b[..ns], &mut fa, &mut fb);
        let (gi, gout) = if rev1 { (g * 1.488, 16.0f32) } else { (g, 18.4f32) };
        let tol = if x.acc == 3 { 1.9073486e-06 } else { 0.00048828125 };
        let per = 4 * o;
        let p0 = 4 * x.c;
        let (t017, rn1, t0) = (tclip(0.17), ft.rn1, tclip(0.0) * ft.rn1);
        let mut i = 0;
        for q in 0..4 * x.n {
            let key = x.rows[8 * 16 + p0 + q];
            let fm = x.rows[h.freq_src * 16 + p0 + q];
            let ff = (((key + 0.0) * h.key_follow) * 0.64 + (fm * h.freq_depth)) + self.fq;
            let ff = if ff > 150.0 { 150.0 } else { ff };
            let ff = if 30.0 > ff { 30.0 } else { ff };
            let hh = (exp2((ff - 69.0) / 12.0) * 110.0) * scale_h;
            if (hh, self.rq) != self.last_in {
                self.last_in = (hh, self.rq);
                self.last_t = if rev1 { coef_1(fs, ts, hh, self.rq) } else { coef_2(fs, ts, hh, self.rq) };
            }
            let t = self.last_t;
            self.d = BiteC {
                c2: (t.c2 - self.c.c2) * 0.0625,
                c3: (t.c3 - self.c.c3) * 0.0625,
                p: (t.p - self.c.p) * 0.0625,
                q: (t.q - self.c.q) * 0.0625,
                c8: (t.c8 - self.c.c8) * 0.0625,
            };
            for _ in 0..per {
                self.c.c2 = self.c.c2 + self.d.c2;
                self.c.c3 = self.c.c3 + self.d.c3;
                self.c.p = self.c.p + self.d.p;
                self.c.q = self.c.q + self.d.q;
                self.c.c8 = self.c.c8 + self.d.c8;
                self.fq = self.fq + df;
                self.rq = self.rq + dr;
                let xx = (self.noise.step() + fa[i]) * gi;
                let tap = if draft {
                    self.draft(xx, rev1, t017)
                } else if rev1 {
                    self.solve1(xx, tol, t017)
                } else {
                    self.solve2(xx, tol, rn1, t0)
                };
                fo[i] = (tap * g) * gout;
                i += 1;
            }
        }
        let (mut l, mut r) = ([0.0f32; 64], [0.0f32; 64]);
        self.res.down(&co, o, &fo, &fb, &mut l[..ns], &mut r[..ns]);
        a[..ns].copy_from_slice(&l[..ns]);
        b[..ns].copy_from_slice(&r[..ns]);
    }

    /// Revision 1: one solved sample; returns the tap.
    #[inline(always)]
    fn solve1(&mut self, x: f32, tol: f32, t017: f32) -> f32 {
        let c = self.c;
        let (pv, ppc, qv1, xv, epv, c3v, c2y, c8v) =
            (F4::splat(c.p), F4::splat(c.p * self.cp), F4::splat(c.q * self.v1p), F4::splat(x), F4::splat(self.ep), F4::splat(c.c3), F4::splat(c.c2 * self.yp), F4::splat(c.c8));
        let (k1, k2, kt, kg) = (F4::splat(1.331), F4::splat(0.17), F4::splat(t017), F4::splat(0.7513148));
        let mut run = SolveRun::start(&self.sol, tol);
        let (mut v1, mut e, mut y, mut tap, mut f2);
        loop {
            let cv = F4::load(&run.cand);
            v1 = (pv * cv - ppc) + qv1;
            e = v1 - xv;
            y = (e + epv) * c3v - c2y;
            tap = (((y * k1 + xv) + k2).tclip() - kt) * kg;
            f2 = tap * c8v;
            let r = (cv - f2).arr();
            if run.feed(&r) {
                break;
            }
        }
        let l = run.lane;
        self.cp = run.cand[l];
        self.v1p = v1.arr()[l];
        self.ep = e.arr()[l];
        self.yp = y.arr()[l];
        self.f2 = f2.arr()[l];
        self.sol.finish(run.g, self.f2);
        tap.arr()[l]
    }

    /// Revision 2: one solved sample; returns the tap.
    #[inline(always)]
    fn solve2(&mut self, x: f32, tol: f32, rn1: f32, t0: f32) -> f32 {
        let c = self.c;
        let (pv, ppc, qv1, xv, epv, c3v, c2y, c8v) =
            (F4::splat(c.p), F4::splat(c.p * self.cp), F4::splat(c.q * self.v1p), F4::splat(x), F4::splat(self.ep), F4::splat(c.c3), F4::splat(c.c2 * self.yp), F4::splat(c.c8));
        let (rn, t0v) = (F4::splat(rn1), F4::splat(t0));
        let mut run = SolveRun::start(&self.sol, tol);
        let (mut d, mut v1, mut e, mut y, mut z, mut f2);
        loop {
            let cv = F4::load(&run.cand);
            d = cv.tclip() * rn - t0v;
            v1 = (d * pv - ppc) + qv1;
            e = v1 - xv;
            y = (e + epv) * c3v - c2y;
            z = y + xv;
            f2 = z * c8v;
            let r = (cv - f2).arr();
            if run.feed(&r) {
                break;
            }
        }
        let l = run.lane;
        self.cp = d.arr()[l];
        self.v1p = v1.arr()[l];
        self.ep = e.arr()[l];
        self.yp = y.arr()[l];
        self.f2 = f2.arr()[l];
        self.sol.finish(run.g, self.f2);
        tclip(z.arr()[l])
    }

    /// Section 8, draft: one evaluation with the previous F2.
    #[inline(always)]
    fn draft(&mut self, x: f32, rev1: bool, t017: f32) -> f32 {
        let c = self.c;
        let f = self.f2;
        let d = if rev1 { f } else { tclip(f) };
        let v1 = ((c.p * d) - (c.p * self.cp)) + (c.q * self.v1p);
        self.cp = d;
        let e = v1 - x;
        let y = ((e + self.ep) * c.c3) - (c.c2 * self.yp);
        self.v1p = v1;
        self.ep = e;
        self.yp = y;
        let tap;
        if rev1 {
            tap = (tclip(((y * 1.331) + x) + 0.17) - t017) * 0.7513148;
            self.f2 = tap * c.c8;
        } else {
            let z = y + x;
            self.f2 = z * c.c8;
            tap = tclip(z);
        }
        self.sol.f2 = self.sol.f1;
        self.sol.f1 = self.sol.f0;
        self.sol.f0 = self.f2;
        tap
    }
}

/// HPF post: a one-pole high-pass on the voice sum.
#[derive(Clone, Copy, Debug)]
pub struct HpfPost {
    inv: f32,
    last_setting: i32,
    c: f32,
    k1: f32,
    k2: f32,
    a: f32,
    b: f32,
    wp: [f32; 2],
    vp: [f32; 2],
}

impl HpfPost {
    pub fn new(fs: f32) -> HpfPost {
        HpfPost { inv: (1.0f64 / fs as f64) as f32, last_setting: i32::MIN, c: 1.0, k1: 0.0, k2: 1.0, a: 0.0, b: 0.0, wp: [0.0; 2], vp: [0.0; 2] }
    }

    fn set(&mut self, setting: i32) {
        self.last_setting = setting;
        let (f1, x): (f32, f32) = match setting {
            -1 => (30.52, 1.38),
            0 => (5.0, 0.0),
            1 => (30.8719, 0.0),
            2 => (63.6228, 0.0),
            _ => (148.5289, 0.0),
        };
        self.c = 1.0 - x;
        self.k1 = 1.0 - self.c.abs();
        self.k2 = 3.1 - (self.c * 2.1);
        let t = tan_d(((f1 * self.inv) * 3.1415927) * 0.5) * 4.0;
        self.a = (t - 1.0) / (t + 1.0);
        self.b = t / (t + 1.0);
    }

    pub fn process(&mut self, setting: i32, l: &mut [f32], r: &mut [f32]) {
        if setting != self.last_setting {
            self.set(setting);
        }
        let (a, b, c, k1, k2) = (self.a, self.b, self.c, self.k1, self.k2);
        for (ch, buf) in [l, r].into_iter().enumerate() {
            let (mut wp, mut vp) = (self.wp[ch], self.vp[ch]);
            for x in buf.iter_mut() {
                let s = *x;
                let w = 0.0 - s;
                let v = ((w + wp) * b) - (vp * a);
                wp = w;
                vp = v;
                *x = ((k1 * s) + ((v + s) * c)) * k2;
            }
            self.wp[ch] = wp;
            self.vp[ch] = vp;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_of_a_voice_starts_from_zero() {
        let mut n = Noise::default();
        let v = n.step();
        // the first word of the generator from 0: 0x333f69f2
        let u = f32::from_bits((0x333f_69f2u32 & 0x7f_ffff) | 0x3f80_0000);
        assert_eq!(v, ((u - 1.0) + 0.0) * 0.0005);
    }

    #[test]
    fn pre_passband_gain() {
        let ft = FilterTables::new(48000.0);
        // G * G * 16, the page's 1.017
        assert!((ft.g * ft.g * 16.0 - 1.0176).abs() < 0.001, "{}", ft.g * ft.g * 16.0);
    }

    #[test]
    fn pre_corner_is_a_high_pass() {
        let (a, b) = coef_pre(1.0 / 48000.0, 1000.0);
        // y = ((w + wp) * b) - y1 * a with w = -x, output y + x: DC is blocked
        assert!(((-2.0 * b) / (1.0 + a) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn bite_coefficients_at_the_page_constants() {
        let c = coef_1(48000.0, 1.0 / 48000.0, 1000.0 * 0.25, 1.0);
        assert!((c.c8 - 1.4025).abs() < 1e-6);
        let d = coef_2(48000.0, 1.0 / 48000.0, 1000.0 * 0.25, 1.0);
        assert_eq!(d.c8, 5.5);
        assert!(c.p > 0.9 && c.q > 0.9 && d.p > 0.9 && d.q > 0.9);
    }
}
