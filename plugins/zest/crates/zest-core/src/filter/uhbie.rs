// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! VCF1 Uhbie (`pages/filter-uhbie.md`): one nonlinear state-variable stage, solved per oversampled sample, its output crossfaded
//! by the shape mix. The rest of a call is the common kernel of `kernel.rs`.

use super::kernel::{solve, solve_two, table_pos, tuning, CallK, Common, Eval, Kernel, Root, KNOBS, TOL_11, TOL_19, TOL_9};
use super::solver::Solver;
use super::{FilterCtx, FilterIo};
use crate::dsp::{hz, tclip, F4};

const KI: f32 = 2.2015064;
const KF: f32 = 0.47016674;
const KO: f32 = -0.14;

/// Draft, fast, great.
pub struct Tables {
    g: [[f32; KNOBS]; 3],
    /// `P(clampT(KO))/KF`.
    tail: f32,
}

impl Tables {
    pub fn new(fs: f32) -> Tables {
        const TEXT: &str = include_str!("../../data/tuning-uhbie.tsv");
        let inv = 1.0f32 / fs;
        let mut g = [[0.0f32; KNOBS]; 3];
        for (cl, (name, os)) in [("draft", 0.5f32), ("fast", 0.5), ("great", 0.25)].into_iter().enumerate() {
            let v = tuning(TEXT, name, fs);
            for i in 0..KNOBS {
                g[cl][i] = ((((hz(v[i]) * os) * 1.614) as f64 * 9.0) * inv as f64) as f32;
            }
        }
        Tables { g, tail: tclip(KO) / KF }
    }
}

#[derive(Clone, Copy, Default)]
struct Model {
    s1: f32,
    s2: f32,
    f: f32,
    sol: Solver,
    c_g: f32,
    c_r: f32,
    ig: f32,
    ir: f32,
    mix: f32,
}

pub struct Uhbie {
    cm: Common,
    m: Model,
}

struct Run<'a> {
    m: &'a mut Model,
    g: &'a [f32; KNOBS],
    tail: f32,
    tol: f32,
    bp: bool,
}

#[inline(always)]
fn g_at(g: &[f32; KNOBS], x: f32) -> f32 {
    let (m, w) = table_pos(x);
    w * g[m + 1] + (1.0 - w) * g[m]
}

#[inline(always)]
fn r_at(res: f32) -> f32 {
    0.0 - res * 2.012076
}

/// The values of one sample.
struct Evals {
    a0: F4,
    g: F4,
    s1: F4,
    rr: F4,
    v2: F4,
    tail: F4,
    v2s: f32,
}

impl Evals {
    #[inline(always)]
    fn new(m: &Model, x: f32, tail: f32) -> Evals {
        let (g, r) = (m.c_g, m.c_r);
        let a0 = (0.1 * x) * 2.0;
        let v2s = g * m.s1 + m.s2;
        let rr = (2.0 * r) + 1.0;
        Evals { a0: F4::splat(a0), g: F4::splat(g), s1: F4::splat(m.s1), rr: F4::splat(rr), v2: F4::splat(v2s), tail: F4::splat(tail), v2s }
    }

    #[inline(always)]
    fn commit(&self, m: &mut Model, v: [F4; 3], run: &Root, bp: bool) -> f32 {
        let [h, v1, f] = v;
        let l = run.sel;
        let (hl, v1l) = (h.arr()[l], v1.arr()[l]);
        let v2 = self.v2s;
        m.s1 = v1l;
        m.s2 = v2;
        m.f = f.arr()[l];
        m.sol.finish(run.g, m.f);
        let mix = m.mix;
        if !bp {
            (hl - v2) * mix + v2
        } else if mix < 0.5 {
            let t = mix * 2.0;
            (v1l - v2) * t + v2
        } else {
            let t = mix * 2.0;
            (2.0 - t) * (v1l - hl) + hl
        }
    }
}

impl Eval for Evals {
    type L = [F4; 3];

    #[inline(always)]
    fn eval(&self, c: F4) -> ([F4; 3], F4) {
        let ki = F4::splat(KI);
        let kf = F4::splat(KF);
        let h = ((self.a0 - F4::splat(0.78) * c) * ki).tclip().div(ki);
        let v1 = h * self.g + self.s1;
        let u = ((self.rr * v1) * kf + F4::splat(KO)).tclip();
        let f = (u.div(kf) + (F4::splat(3.0) * v1 + self.v2)) - self.tail;
        ([h, v1, f], c - f)
    }
}

impl Kernel for Run<'_> {
    #[inline(always)]
    fn nhz(&self) -> f32 {
        5000.0
    }

    fn clear(&mut self, cut: f32, res: f32) {
        self.m.s1 = 0.0;
        self.m.s2 = 0.0;
        self.m.f = 0.0;
        self.m.c_g = g_at(self.g, cut);
        self.m.c_r = r_at(res);
    }

    #[inline(always)]
    fn begin(&mut self) {
        self.m.ig = 0.0;
        self.m.ir = 0.0;
        self.m.sol.reset_call();
    }

    #[inline(always)]
    fn ramps(&mut self, x: f32, res: f32, st: f32) {
        self.m.ig = (g_at(self.g, x) - self.m.c_g) * st;
        self.m.ir = (r_at(res) - self.m.c_r) * st;
    }

    #[inline(always)]
    fn ramp_step(&mut self) {
        self.m.c_r = self.m.ir + self.m.c_r;
        self.m.c_g = self.m.ig + self.m.c_g;
    }

    #[inline(always)]
    fn lookup(&mut self, x: f32, res: f32) {
        self.m.c_r = r_at(res);
        self.m.c_g = g_at(self.g, x);
    }

    #[inline(always)]
    fn mix(&mut self, shp: f32) {
        self.m.mix = shp;
    }

    #[inline(always)]
    fn input(&self, k: &CallK, x0: f32, fb: f32, gprev: f32) -> f32 {
        let x1 = if k.fb_on { x0 + tclip(gprev * fb) } else { x0 };
        x1 * k.g12
    }

    #[inline(always)]
    fn sample(&mut self, k: &CallK, x: f32) -> f32 {
        let e = Evals::new(self.m, x, self.tail);
        let mut r = Root::start(&self.m.sol, self.tol);
        let l = solve(&e, &mut r);
        let tap = e.commit(self.m, l, &r, self.bp);
        ((k.g12 * 16.0) * tap) * 4.0
    }

    #[inline(always)]
    fn sample_two(p: &mut Self, q: &mut Self, k: &CallK, xp: f32, xq: f32) -> (f32, f32) {
        let (ep, eq) = (Evals::new(p.m, xp, p.tail), Evals::new(q.m, xq, p.tail));
        let (mut rp, mut rq) = (Root::start(&p.m.sol, p.tol), Root::start(&q.m.sol, p.tol));
        let (lp, lq) = solve_two(&ep, &mut rp, &eq, &mut rq);
        let (tp, tq) = (ep.commit(p.m, lp, &rp, p.bp), eq.commit(q.m, lq, &rq, p.bp));
        let scale = k.g12 * 16.0;
        ((scale * tp) * 4.0, (scale * tq) * 4.0)
    }
}

impl Uhbie {
    /// See [`Common::reset_dc`].
    #[inline]
    pub fn reset_dc(&mut self) {
        self.cm.reset_dc();
    }

    /// `seed`: the voice's (seed_a, seed_b) of `data/voice-seeds.tsv`, the filter noise's start.
    pub fn new(seed: (u32, u32)) -> Box<Uhbie> {
        Box::new(Uhbie { cm: Common::new(seed), m: Model::default() })
    }

    #[inline(always)]
    fn call<'a>(m: &'a mut Model, x: &'a FilterCtx) -> Run<'a> {
        let t = &x.ft.uhbie;
        Run {
            m,
            g: &t.g[x.acc.min(2) as usize],
            tail: t.tail,
            tol: match x.acc {
                3 => TOL_19,
                0 => TOL_9,
                _ => TOL_11,
            },
            bp: x.s.vcf.uhbie_bp,
        }
    }

    /// One call.
    pub fn run(&mut self, x: &FilterCtx, io: &mut FilterIo) {
        let mut run = Self::call(&mut self.m, x);
        self.cm.run(&mut run, x, io);
    }

    /// Two voices' calls in step.
    pub fn run_two(p: &mut Uhbie, q: &mut Uhbie, x: &FilterCtx, iop: &mut FilterIo, ioq: &mut FilterIo) {
        let mut a = Self::call(&mut p.m, x);
        let mut b = Self::call(&mut q.m, x);
        Common::run_two(&mut p.cm, &mut q.cm, &mut a, &mut b, x, iop, ioq);
    }
}
