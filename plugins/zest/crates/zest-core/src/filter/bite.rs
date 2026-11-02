// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! VCF1 Bite (`pages/filter-bite.md`): first-order sections around a feedback loop, solved per oversampled sample, in two
//! revisions. The rest of a call is the common kernel of `kernel.rs`.

use super::kernel::{solve, solve_two, table_pos, tuning, CallK, Common, Eval, Kernel, Root, KNOBS, TOL_11, TOL_19};
use super::solver::Solver;
use super::{FilterCtx, FilterIo};
use crate::dsp::{hz, rcpn, tan_d, tclip, F4};

const PI: f32 = 3.1415927;

/// The eight per-step coefficients.
type C8 = [f32; 8];

/// [revision 1, revision 2][draft, fast, great].
pub struct Tables {
    t: [[[C8; KNOBS]; 3]; 2],
    t017: f32,
}

fn rev1(h4: f32, fs: f32, inv: f32) -> C8 {
    let f6 = h4 * 0.68032527;
    let t1 = tan_d(((inv * f6) * PI) * 0.5) * 4.0;
    let t2 = tan_d((((f6 * 3.0) * inv) * PI) * 0.5) * 4.0;
    let a = ((fs as f64 + fs as f64) * 4.0) as f32;
    let w = ((((f6 * 5.0) as f64 * 0.9) * 2.0) * PI as f64) as f32;
    let q = 1.0 / (a + w);
    [
        (t1 - 1.0) / (t1 + 1.0),
        t1 / (t1 + 1.0),
        (t2 - 1.0) / (t2 + 1.0),
        t2 / (t2 + 1.0),
        (fs - 3218.98) * rcpn(fs + 3218.98),
        q * a,
        (a - w) * q,
        (1.0f64 - (inv * 64.0) as f64) as f32,
    ]
}

fn rev2(h4: f32, fs: f32, inv: f32) -> C8 {
    let f3 = h4 * 0.20905685;
    let t1 = tan_d(((inv * f3) * PI) * 0.5) * 4.0;
    let t2 = tan_d((((f3 * 3.0) * inv) * PI) * 0.5) * 4.0;
    let w = (f3 * 5.0) * 10.0;
    let w = (w + w) * PI;
    let a = ((fs as f64 + fs as f64) * 4.0) as f32;
    let q = (1.0f64 / (w + a) as f64) as f32;
    [
        (t1 - 1.0) / (t1 + 1.0),
        t1 / (t1 + 1.0),
        (t2 - 1.0) / (t2 + 1.0),
        t2 / (t2 + 1.0),
        q * a,
        (a - w) * q,
        (1.0f64 - (inv * 64.0) as f64) as f32,
        (fs - 1023.2) * rcpn(fs + 1023.2),
    ]
}

impl Tables {
    pub fn new(fs: f32) -> Tables {
        const TEXT: &str = include_str!("../../data/tuning-bite.tsv");
        let inv = 1.0f32 / fs;
        let mut t = [[[[0.0f32; 8]; KNOBS]; 3]; 2];
        for (r, rname) in ["r1", "r2"].into_iter().enumerate() {
            for (cl, (name, os)) in [("draft", 0.5f32), ("fast", 0.5), ("great", 0.25)].into_iter().enumerate() {
                let v = tuning(TEXT, &format!("{rname}_{name}"), fs);
                for i in 0..KNOBS {
                    let h4 = hz(v[i]) * os;
                    t[r][cl][i] = if r == 0 { rev1(h4, fs, inv) } else { rev2(h4, fs, inv) };
                }
            }
        }
        Tables { t, t017: tclip(0.17) }
    }
}

/// `CAt(x)`.
#[inline(always)]
fn c_at(t: &[C8; KNOBS], x: f32) -> C8 {
    let (m, w) = table_pos(x);
    let o = 1.0 - w;
    let (a, b) = (&t[m + 1], &t[m]);
    let mut c = [0.0f32; 8];
    for k in 0..8 {
        c[k] = w * a[k] + o * b[k];
    }
    c
}

#[derive(Clone, Copy, Default)]
struct Model {
    s: [f32; 9],
    f: f32,
    sol: Solver,
    c: C8,
    i: C8,
    c8: f32,
    i8: f32,
}

pub struct Bite {
    cm: Common,
    m: Model,
}

struct Run<'a> {
    m: &'a mut Model,
    t: &'a [C8; KNOBS],
    t017: f32,
    r1: bool,
    draft: bool,
    tol: f32,
}

/// Revision 1's values for one sample.
struct Eval1 {
    y1: f32,
    x: f32,
    c5: F4,
    c5s5: F4,
    c6s4: F4,
    y1v: F4,
    s3: F4,
    c3: F4,
    c2s2: F4,
    kt: F4,
    c8: F4,
    c7s6: F4,
    s7: F4,
    s8: F4,
    c4: F4,
}

impl Eval1 {
    #[inline(always)]
    fn new(m: &Model, x: f32, t017: f32) -> Eval1 {
        let [c0, c1, c2, c3, c4, c5, c6, c7] = m.c;
        let s = &m.s;
        let y1 = (x + s[1]) * c1 - c0 * s[0];
        Eval1 {
            y1,
            x,
            c5: F4::splat(c5),
            c5s5: F4::splat(c5 * s[5]),
            c6s4: F4::splat(c6 * s[4]),
            y1v: F4::splat(y1),
            s3: F4::splat(s[3]),
            c3: F4::splat(c3),
            c2s2: F4::splat(c2 * s[2]),
            kt: F4::splat(t017),
            c8: F4::splat(m.c8),
            c7s6: F4::splat(c7 * s[6]),
            s7: F4::splat(s[7]),
            s8: F4::splat(s[8]),
            c4: F4::splat(c4),
        }
    }

    #[inline(always)]
    fn commit(&self, m: &mut Model, v: [F4; 8], run: &Root) -> f32 {
        let l = run.sel;
        let [e0, e, y2, tap, fa, h6, n8, f] = v;
        m.s = [self.y1, self.x, y2.arr()[l], e.arr()[l], e0.arr()[l], run.cand[l], h6.arr()[l], fa.arr()[l], n8.arr()[l]];
        m.f = f.arr()[l];
        m.sol.finish(run.g, m.f);
        tap.arr()[l]
    }
}

impl Eval for Eval1 {
    type L = [F4; 8];

    #[inline(always)]
    fn eval(&self, c: F4) -> ([F4; 8], F4) {
        let e0 = (self.c5 * c - self.c5s5) + self.c6s4;
        let e = e0 + self.y1v;
        let y2 = (e + self.s3) * self.c3 - self.c2s2;
        let tap = ((F4::splat(1.331) * y2 + F4::splat(0.17)).tclip() - self.kt) * F4::splat(0.7513148);
        let fa = tap * self.c8;
        let h6 = self.c7s6 + (fa - self.s7);
        let g = F4::splat(0.82) * fa + F4::splat(0.18) * h6;
        let n8 = (self.s8 - g) * self.c4 + g;
        let f = F4::splat(0.13) * n8 + F4::splat(0.87) * g;
        ([e0, e, y2, tap, fa, h6, n8, f], c - f)
    }
}

/// Revision 2's values for one sample.
struct Eval2 {
    y1: f32,
    a: f32,
    c4: F4,
    c4s5: F4,
    c5s4: F4,
    y1v: F4,
    s3: F4,
    c3: F4,
    c2s2: F4,
    c8: F4,
    c6s6: F4,
    s7: F4,
    s8: F4,
    c7: F4,
}

impl Eval2 {
    #[inline(always)]
    fn new(m: &Model, x: f32) -> Eval2 {
        let [c0, c1, c2, c3, c4, c5, c6, c7] = m.c;
        let s = &m.s;
        let a = tclip(0.6 * x) * 1.6666666;
        let y1 = (a + s[1]) * c1 - c0 * s[0];
        Eval2 {
            y1,
            a,
            c4: F4::splat(c4),
            c4s5: F4::splat(c4 * s[5]),
            c5s4: F4::splat(c5 * s[4]),
            y1v: F4::splat(y1),
            s3: F4::splat(s[3]),
            c3: F4::splat(c3),
            c2s2: F4::splat(c2 * s[2]),
            c8: F4::splat(m.c8),
            c6s6: F4::splat(c6 * s[6]),
            s7: F4::splat(s[7]),
            s8: F4::splat(s[8]),
            c7: F4::splat(c7),
        }
    }

    #[inline(always)]
    fn commit(&self, m: &mut Model, v: [F4; 8], run: &Root) -> f32 {
        let l = run.sel;
        let [hh, e0, e, y2, fa, h6, n8, f] = v;
        let y2l = y2.arr()[l];
        m.s = [self.y1, self.a, y2l, e.arr()[l], e0.arr()[l], hh.arr()[l], h6.arr()[l], fa.arr()[l], n8.arr()[l]];
        m.f = f.arr()[l];
        m.sol.finish(run.g, m.f);
        y2l * 1.6666666
    }
}

impl Eval for Eval2 {
    type L = [F4; 8];

    #[inline(always)]
    fn eval(&self, c: F4) -> ([F4; 8], F4) {
        let hh = c.tclip();
        let e0 = (hh * self.c4 - self.c4s5) + self.c5s4;
        let e = e0 + self.y1v;
        let y2 = (e + self.s3) * self.c3 - self.c2s2;
        let fa = y2 * self.c8;
        let h6 = self.c6s6 + (fa - self.s7);
        let g = F4::splat(0.82) * fa + F4::splat(0.18) * h6;
        let n8 = (self.s8 - g) * self.c7 + g;
        let f = F4::splat(0.61) * g + F4::splat(0.39) * n8;
        ([hh, e0, e, y2, fa, h6, n8, f], c - f)
    }
}

impl Run<'_> {
    #[inline(always)]
    fn c8_at(&self, res: f32) -> f32 {
        if self.r1 {
            res * 1.4025 + 0.0
        } else {
            res * 5.5 + 0.0
        }
    }

    /// Draft: one evaluation with the previous F.
    #[inline(always)]
    fn draft_step(&mut self, x: f32) -> f32 {
        let m = &mut *self.m;
        let [c0, c1, c2, c3, c4, c5, c6, c7] = m.c;
        let s = m.s;
        let c = m.f;
        let tap;
        let f;
        if self.r1 {
            let y1 = (x + s[1]) * c1 - c0 * s[0];
            let e0 = (c5 * c - c5 * s[5]) + c6 * s[4];
            let e = e0 + y1;
            let y2 = (e + s[3]) * c3 - c2 * s[2];
            tap = (tclip(1.331 * y2 + 0.17) - self.t017) * 0.7513148;
            let fa = tap * m.c8;
            let h6 = c7 * s[6] + (fa - s[7]);
            let g = (fa as f64 * 0.8200000000000001 + h6 as f64 * 0.18) as f32;
            let n8 = (s[8] - g) * c4 + g;
            f = (g as f64 * 0.87 + n8 as f64 * 0.13) as f32;
            m.s = [y1, x, y2, e, e0, c, h6, fa, n8];
        } else {
            let a = tclip(0.6 * x) * 1.6666666;
            let y1 = (a + s[1]) * c1 - c0 * s[0];
            let hh = tclip(c);
            let e0 = (hh * c4 - c4 * s[5]) + c5 * s[4];
            let e = e0 + y1;
            let y2 = (e + s[3]) * c3 - c2 * s[2];
            let fa = y2 * m.c8;
            tap = y2 * 1.6666666;
            let h6 = c6 * s[6] + (fa - s[7]);
            let g = (fa as f64 * 0.8200000000000001 + h6 as f64 * 0.18) as f32;
            let n8 = (s[8] - g) * c7 + g;
            f = (g as f64 * 0.61 + n8 as f64 * 0.39) as f32;
            m.s = [y1, a, y2, e, e0, hh, h6, fa, n8];
        }
        m.f = f;
        m.sol.f2 = m.sol.f1;
        m.sol.f1 = m.sol.f0;
        m.sol.f0 = f;
        tap
    }
}

impl Kernel for Run<'_> {
    #[inline(always)]
    fn nhz(&self) -> f32 {
        5000.0
    }

    fn clear(&mut self, cut: f32, res: f32) {
        self.m.s = [0.0; 9];
        self.m.f = 0.0;
        self.m.c = c_at(self.t, cut);
        self.m.c8 = self.c8_at(res);
    }

    #[inline(always)]
    fn begin(&mut self) {
        self.m.i = [0.0; 8];
        self.m.i8 = 0.0;
        self.m.sol.reset_call();
    }

    #[inline(always)]
    fn ramps(&mut self, x: f32, res: f32, st: f32) {
        let t = c_at(self.t, x);
        for k in 0..8 {
            self.m.i[k] = (t[k] - self.m.c[k]) * st;
        }
        self.m.i8 = (self.c8_at(res) - self.m.c8) * st;
    }

    #[inline(always)]
    fn ramp_step(&mut self) {
        for k in 0..8 {
            self.m.c[k] = self.m.i[k] + self.m.c[k];
        }
        self.m.c8 = self.m.i8 + self.m.c8;
    }

    #[inline(always)]
    fn lookup(&mut self, x: f32, res: f32) {
        self.m.c = c_at(self.t, x);
        self.m.c8 = self.c8_at(res);
    }

    #[inline(always)]
    fn input(&self, k: &CallK, x0: f32, fb: f32, gprev: f32) -> f32 {
        let x1 = if k.fb_on { x0 + fb * gprev } else { x0 };
        x1 * (k.g12 * 1.488)
    }

    #[inline(always)]
    fn sample(&mut self, k: &CallK, x: f32) -> f32 {
        let tap = if self.draft {
            self.draft_step(x)
        } else if self.r1 {
            let e = Eval1::new(self.m, x, self.t017);
            let mut r = Root::start(&self.m.sol, self.tol);
            let l = solve(&e, &mut r);
            e.commit(self.m, l, &r)
        } else {
            let e = Eval2::new(self.m, x);
            let mut r = Root::start(&self.m.sol, self.tol);
            let l = solve(&e, &mut r);
            e.commit(self.m, l, &r)
        };
        ((k.g12 * 16.0) * tap) * 0.5
    }

    #[inline(always)]
    fn sample_two(p: &mut Self, q: &mut Self, k: &CallK, xp: f32, xq: f32) -> (f32, f32) {
        let scale = k.g12 * 16.0;
        let (mut rp, mut rq) = (Root::start(&p.m.sol, p.tol), Root::start(&q.m.sol, p.tol));
        let (tp, tq) = if p.r1 {
            let (ep, eq) = (Eval1::new(p.m, xp, p.t017), Eval1::new(q.m, xq, p.t017));
            let (lp, lq) = solve_two(&ep, &mut rp, &eq, &mut rq);
            (ep.commit(p.m, lp, &rp), eq.commit(q.m, lq, &rq))
        } else {
            let (ep, eq) = (Eval2::new(p.m, xp), Eval2::new(q.m, xq));
            let (lp, lq) = solve_two(&ep, &mut rp, &eq, &mut rq);
            (ep.commit(p.m, lp, &rp), eq.commit(q.m, lq, &rq))
        };
        ((scale * tp) * 0.5, (scale * tq) * 0.5)
    }
}

impl Bite {
    /// See [`Common::reset_dc`].
    #[inline]
    pub fn reset_dc(&mut self) {
        self.cm.reset_dc();
    }

    /// `seed`: the voice's (seed_a, seed_b) of `data/voice-seeds.tsv`, the filter noise's start.
    pub fn new(seed: (u32, u32)) -> Box<Bite> {
        Box::new(Bite { cm: Common::new(seed), m: Model::default() })
    }

    #[inline(always)]
    fn call<'a>(m: &'a mut Model, x: &'a FilterCtx) -> Run<'a> {
        let r1 = x.s.vcf.sk_revision == 1;
        let t = &x.ft.bite;
        Run {
            m,
            t: &t.t[if r1 { 0 } else { 1 }][x.acc.min(2) as usize],
            t017: t.t017,
            r1,
            draft: x.acc == 0,
            tol: if x.acc == 3 { TOL_19 } else { TOL_11 },
        }
    }

    /// One call.
    pub fn run(&mut self, x: &FilterCtx, io: &mut FilterIo) {
        let mut run = Self::call(&mut self.m, x);
        self.cm.run(&mut run, x, io);
    }

    /// Two voices' calls in step; not for draft.
    pub fn run_two(p: &mut Bite, q: &mut Bite, x: &FilterCtx, iop: &mut FilterIo, ioq: &mut FilterIo) {
        let mut a = Self::call(&mut p.m, x);
        let mut b = Self::call(&mut q.m, x);
        Common::run_two(&mut p.cm, &mut q.cm, &mut a, &mut b, x, iop, ioq);
    }
}
