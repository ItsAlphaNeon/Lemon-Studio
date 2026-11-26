// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! VCF1 Multimode (`pages/filter-multimode.md`): a nonlinear state-variable filter, one stage or two in series, solved per
//! oversampled sample.

use super::kernel::{dead_zone, solve, solve_two, table_pos, tuning, CallK, Common, Eval, Kernel, Root, KNOBS, TOL_15, TOL_17, TOL_19};
use super::solver::Solver;
use super::{FilterCtx, FilterIo};
use crate::dsp::{hz, rcpn, tclip, F4};

/// `T(0.2)`, the term that keeps the integrator input at zero at rest.
const T02: f32 = 0.19766083;

/// The per-accuracy tables: draft, fast, great.
pub struct Tables {
    g: [[f32; KNOBS]; 3],
    rec122: f32,
    softk: f32,
}

impl Tables {
    pub fn new(fs: f32) -> Tables {
        const TEXT: &str = include_str!("../../data/tuning-multimode.tsv");
        let mut g = [[0.0f32; KNOBS]; 3];
        let inv = 1.0f32 / fs;
        for (cl, (name, os)) in [("draft", 0.5f32), ("fast", 0.5), ("great", 0.25)].into_iter().enumerate() {
            let mut v = tuning(TEXT, name, fs);
            if fs > 48000.0 && fs < 64000.0 {
                for i in 111..KNOBS {
                    v[i] = v[110];
                }
            }
            for i in 0..KNOBS {
                g[cl][i] = ((((hz(v[i]) * os) * 1.614) as f64 * 9.0) * inv as f64) as f32;
            }
        }
        Tables { g, rec122: rcpn(1.22), softk: 0.22f32 + 1.0 }
    }

    #[inline(always)]
    fn g_at(&self, cl: usize, x: f32) -> f32 {
        let (m, w) = table_pos(x);
        w * self.g[cl][m + 1] + (1.0 - w) * self.g[cl][m]
    }
}

#[inline(always)]
fn d_at(res: f32) -> f32 {
    2.0 - res * 2.01
}

/// One solved stage.
#[derive(Clone, Copy, Default)]
struct Stage {
    s1: f32,
    s2: f32,
    f: f32,
    sol: Solver,
}

/// Section 8: the half-band and one-pole between the stages.
#[derive(Clone, Copy, Default)]
struct Between {
    x1: f32,
    x2: f32,
    a1: f32,
    a1p: f32,
    b1: f32,
    b1p: f32,
    a2: f32,
    a2p: f32,
    b2: f32,
    b2p: f32,
    y: f32,
}

impl Between {
    #[inline(always)]
    fn step(&mut self, x: f32, four: bool) -> f32 {
        let e1 = (x - self.b1p) * 0.39036217 + self.x2;
        let o1 = (x - self.a1p) * 0.12073212 + self.x2;
        let o2 = (o1 - self.a2p) * 0.66320205 + self.a1p;
        let e2 = (e1 - self.b2p) * 0.8907868 + self.b1p;
        let out = (o2 + self.b2) * 0.5;
        self.x2 = self.x1;
        self.x1 = x;
        self.a1p = self.a1;
        self.a1 = o1;
        self.b1p = self.b1;
        self.b1 = e1;
        self.a2p = self.a2;
        self.a2 = o2;
        self.b2p = self.b2;
        self.b2 = e2;
        if four {
            self.y = out * 0.33333337 + self.y * 0.6666666;
            self.y
        } else {
            out
        }
    }

    #[inline]
    fn clear(&mut self) {
        let y = self.y;
        *self = Between { y, ..Between::default() };
    }
}

/// The model's state per voice.
struct Model {
    st: [Stage; 2],
    between: Between,
    c_g: f32,
    c_d: f32,
    ig: f32,
    id: f32,
    /// The drive term of the last `c_d`.
    last_d: f32,
    drive: f32,
}

impl Model {
    /// The drive term of `d`, kept while `d` holds: it takes a true division.
    #[inline(always)]
    fn drive_of(&mut self, d: f32) -> f32 {
        if d != self.last_d {
            self.last_d = d;
            self.drive = ((d - 2.0) / 2.01 + 1.0) * 0.06 + 0.04;
        }
        self.drive
    }
}

pub struct Multimode {
    cm: Common,
    m: Model,
}

/// A call of the model.
struct Run<'a> {
    m: &'a mut Model,
    t: &'a Tables,
    cl: usize,
    single: bool,
    high1: bool,
    high2: bool,
    four: bool,
    draft: bool,
    tol1: f32,
    tol2: f32,
    gout: f32,
    nhz: f32,
}

/// The soft clip of draft.
#[inline(always)]
fn softclip(x: f32, k: f32, k1: f32) -> f32 {
    let mut y = k1 * x;
    if y.abs() > k {
        if y < -k {
            y = tclip(k + y) - k;
        }
        if y > k {
            y = tclip(y - k) + k;
        }
    }
    y / k1
}

/// A stage's values for one sample.
struct StageEval {
    a0: F4,
    g: F4,
    s1: F4,
    dm3: F4,
    v2: F4,
    rec: F4,
    v2s: f32,
}

impl StageEval {
    #[inline(always)]
    fn new(st: &Stage, x: f32, g: f32, dm3: f32, drive: f32, rec122: f32) -> StageEval {
        let a0 = (drive * x) * 2.0;
        let v2s = g * st.s1 + st.s2;
        StageEval { a0: F4::splat(a0), g: F4::splat(g), s1: F4::splat(st.s1), dm3: F4::splat(dm3), v2: F4::splat(v2s), rec: F4::splat(rec122), v2s }
    }

    /// Commit the lane the solver chose; returns the tap.
    #[inline(always)]
    fn commit(&self, st: &mut Stage, l: (F4, F4, F4), run: &Root, high: bool) -> f32 {
        let (h, v1, f) = l;
        st.s1 = v1.arr()[run.sel];
        st.s2 = self.v2s;
        st.f = f.arr()[run.sel];
        st.sol.finish(run.g, st.f);
        if high {
            h.arr()[run.sel]
        } else {
            self.v2s
        }
    }
}

impl Eval for StageEval {
    type L = (F4, F4, F4);

    #[inline(always)]
    fn eval(&self, c: F4) -> ((F4, F4, F4), F4) {
        let h = (((self.a0 - F4::splat(0.78) * c) + F4::splat(0.2)).tclip()) - F4::splat(T02);
        let v1 = h * self.g + self.s1;
        let k22 = F4::splat(0.22);
        let u = ((self.dm3 * v1) * F4::splat(2.0)) + k22;
        let u = k22 * u + u;
        let d = dead_zone(u, 0.22);
        let f = ((d * self.rec) - k22) * F4::splat(0.5) + (F4::splat(3.0) * v1 + self.v2);
        ((h, v1, f), c - f)
    }
}

#[inline(always)]
fn draft(st: &mut Stage, x: f32, g: f32, d: f32, high: bool, softk: f32) -> f32 {
    let a = (((d - 2.0) / 2.01 + 1.0) * 0.06 + 0.04) * x;
    let h = tclip(((a + a) - st.f * 0.78) + 0.2) - T02;
    let v1 = h * g + st.s1;
    let v2 = g * v1 + st.s2;
    let u = (d - 3.0) * v1;
    let u = (u + u) + 0.22;
    let f = (softclip(u, 0.22, softk) - 0.22) * 0.5 + (v1 * 3.0 + v2);
    st.s1 = v1;
    st.s2 = v2;
    st.f = f;
    st.sol.f2 = st.sol.f1;
    st.sol.f1 = st.sol.f0;
    st.sol.f0 = f;
    if high {
        h
    } else {
        v2
    }
}

impl Kernel for Run<'_> {
    #[inline(always)]
    fn nhz(&self) -> f32 {
        self.nhz
    }
    #[inline(always)]
    fn mode1_plain(&self) -> bool {
        !self.single
    }

    fn clear(&mut self, cut: f32, res: f32) {
        for s in &mut self.m.st {
            s.s1 = 0.0;
            s.s2 = 0.0;
            s.f = 0.0;
        }
        self.m.between.clear();
        self.m.c_g = self.t.g_at(self.cl, cut);
        self.m.c_d = d_at(res);
    }

    #[inline(always)]
    fn begin(&mut self) {
        self.m.ig = 0.0;
        self.m.id = 0.0;
        self.m.st[0].sol.reset_call();
        self.m.st[1].sol.reset_call();
    }

    #[inline(always)]
    fn ramps(&mut self, x: f32, res: f32, st: f32) {
        self.m.ig = (self.t.g_at(self.cl, x) - self.m.c_g) * st;
        self.m.id = (d_at(res) - self.m.c_d) * st;
    }

    #[inline(always)]
    fn ramp_step(&mut self) {
        self.m.c_d = self.m.id + self.m.c_d;
        self.m.c_g = self.m.ig + self.m.c_g;
    }

    #[inline(always)]
    fn lookup(&mut self, x: f32, res: f32) {
        self.m.c_g = self.t.g_at(self.cl, x);
        self.m.c_d = d_at(res);
    }

    #[inline(always)]
    fn input(&self, k: &CallK, x0: f32, fb: f32, gprev: f32) -> f32 {
        let x1 = if k.fb_on { x0 + tclip(fb * gprev) } else { x0 };
        x1 * k.g12
    }

    #[inline(always)]
    fn sample(&mut self, k: &CallK, x: f32) -> f32 {
        let m = &mut *self.m;
        let (g, d) = (m.c_g, m.c_d);
        let scale = k.g12 * 16.0;
        if self.draft {
            let t1 = draft(&mut m.st[0], x, g, d, self.high1, self.t.softk);
            if self.single {
                return (scale * t1) * self.gout;
            }
            let b = m.between.step(t1, self.four);
            let t2 = draft(&mut m.st[1], b, g, d, self.high2, self.t.softk);
            return (scale * t2) * self.gout;
        }
        let drive = m.drive_of(d);
        let dm3 = d - 3.0;
        let rec = self.t.rec122;
        let e1 = StageEval::new(&m.st[0], x, g, dm3, drive, rec);
        let mut r1 = Root::start(&m.st[0].sol, self.tol1);
        let l1 = solve(&e1, &mut r1);
        let t1 = e1.commit(&mut m.st[0], l1, &r1, self.high1);
        if self.single {
            return (scale * t1) * self.gout;
        }
        let b = m.between.step(t1, self.four);
        let e2 = StageEval::new(&m.st[1], b, g, dm3, drive, rec);
        let mut r2 = Root::start(&m.st[1].sol, self.tol2);
        let l2 = solve(&e2, &mut r2);
        let t2 = e2.commit(&mut m.st[1], l2, &r2, self.high2);
        (scale * t2) * self.gout
    }

    #[inline(always)]
    fn sample_two(p: &mut Self, q: &mut Self, k: &CallK, xp: f32, xq: f32) -> (f32, f32) {
        let scale = k.g12 * 16.0;
        let (mp, mq) = (&mut *p.m, &mut *q.m);
        let (gp, dp, gq, dq) = (mp.c_g, mp.c_d, mq.c_g, mq.c_d);
        let (drp, drq) = (mp.drive_of(dp), mq.drive_of(dq));
        let (dm3p, dm3q) = (dp - 3.0, dq - 3.0);
        let rec = p.t.rec122;
        let (ep, eq) = (StageEval::new(&mp.st[0], xp, gp, dm3p, drp, rec), StageEval::new(&mq.st[0], xq, gq, dm3q, drq, rec));
        let (mut rp, mut rq) = (Root::start(&mp.st[0].sol, p.tol1), Root::start(&mq.st[0].sol, p.tol1));
        let (lp, lq) = solve_two(&ep, &mut rp, &eq, &mut rq);
        let tp = ep.commit(&mut mp.st[0], lp, &rp, p.high1);
        let tq = eq.commit(&mut mq.st[0], lq, &rq, p.high1);
        if p.single {
            return ((scale * tp) * p.gout, (scale * tq) * p.gout);
        }
        let (bp, bq) = (mp.between.step(tp, p.four), mq.between.step(tq, p.four));
        let (ep, eq) = (StageEval::new(&mp.st[1], bp, gp, dm3p, drp, rec), StageEval::new(&mq.st[1], bq, gq, dm3q, drq, rec));
        let (mut rp, mut rq) = (Root::start(&mp.st[1].sol, p.tol2), Root::start(&mq.st[1].sol, p.tol2));
        let (lp, lq) = solve_two(&ep, &mut rp, &eq, &mut rq);
        let tp = ep.commit(&mut mp.st[1], lp, &rp, p.high2);
        let tq = eq.commit(&mut mq.st[1], lq, &rq, p.high2);
        ((scale * tp) * p.gout, (scale * tq) * p.gout)
    }
}

impl Multimode {
    /// See [`Common::reset_dc`].
    #[inline]
    pub fn reset_dc(&mut self) {
        self.cm.reset_dc();
    }

    /// `seed`: the voice's (seed_a, seed_b) of `data/voice-seeds.tsv`, the filter noise's start.
    pub fn new(seed: (u32, u32)) -> Box<Multimode> {
        Box::new(Multimode {
            cm: Common::new(seed),
            m: Model {
                st: [Stage::default(); 2],
                between: Between::default(),
                c_g: 0.0,
                c_d: 0.0,
                ig: 0.0,
                id: 0.0,
                last_d: f32::NAN,
                drive: 0.0,
            },
        })
    }

    #[inline(always)]
    fn call<'a>(m: &'a mut Model, x: &'a FilterCtx) -> Run<'a> {
        let mode = x.s.vcf.svf_mode;
        let single = mode == 1;
        let (tol1, tol2) = match x.acc {
            3 => (TOL_19, TOL_19),
            2 if single => (TOL_17, TOL_17),
            _ => (TOL_15, TOL_17),
        };
        Run {
            m,
            t: &x.ft.multimode,
            cl: x.acc.min(2) as usize,
            single,
            high1: mode == 2 || mode == 3,
            high2: mode == 2,
            four: x.acc >= 2,
            draft: x.acc == 0,
            tol1,
            tol2,
            gout: if single { 4.0 } else { 15.47415 },
            nhz: if single { 5000.0 } else { 1000.0 },
        }
    }

    /// One call: `io.clear` says the voice's first note (or a ghost call) wants a clear.
    pub fn run(&mut self, x: &FilterCtx, io: &mut FilterIo) {
        let mut run = Self::call(&mut self.m, x);
        self.cm.run(&mut run, x, io);
    }

    /// Two voices' calls in step; not for draft.
    pub fn run_two(p: &mut Multimode, q: &mut Multimode, x: &FilterCtx, iop: &mut FilterIo, ioq: &mut FilterIo) {
        let mut a = Self::call(&mut p.m, x);
        let mut b = Self::call(&mut q.m, x);
        Common::run_two(&mut p.cm, &mut q.cm, &mut a, &mut b, x, iop, ioq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_of_the_rest_term() {
        assert_eq!(tclip(0.2).to_bits(), T02.to_bits());
    }

    #[test]
    fn g_is_linear_in_hz() {
        let t = Tables::new(48000.0);
        // 12 knob units are one octave once the tuning is regular
        let r = t.g[2][60] / t.g[2][48];
        assert!((r - 2.0).abs() < 0.2, "{r}");
    }
}
