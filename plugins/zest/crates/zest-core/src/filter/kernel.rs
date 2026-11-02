// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The common kernel of Multimode, Bite and Uhbie (`pages/filter-multimode.md` sections 6, 9, 10, 11, 12, 13): the root finder,
//! the call's smoothing, noise, DC blocker, VCA and resampling. A model fills in its coefficients, its input gain and its solved
//! stage through [`Kernel`]; the loop is generic, so a model's evaluation inlines into it.

use super::{os_coefs, tuning_column, FilterCtx, FilterIo, OsConst, Resamplers, Vca};
use super::solver::Solver;
use crate::dsp::{rcpn, F4};
use crate::settings::{VcaSet, VcfSet};
use core::arch::x86_64::*;

pub const KNOBS: usize = 121;

/// `P(clampT(x))` is `T(x)`: `crate::dsp::tclip`. The tolerance values of the pages.
pub const TOL_9: f32 = 0.001953125;
pub const TOL_11: f32 = 0.00048828125;
pub const TOL_15: f32 = 0.000030517578;
pub const TOL_17: f32 = 0.0000076293945;
pub const TOL_19: f32 = 0.0000019073486;

/// The band of a tuning file of these pages, by rate: `441`, `48`, `hi`.
pub fn band(fs: f32) -> &'static str {
    if fs <= 44100.0 {
        "441"
    } else if fs <= 48000.0 {
        "48"
    } else {
        "hi"
    }
}

/// The steps of column `prefix_<band>` of a tuning file.
pub fn tuning(text: &str, prefix: &str, fs: f32) -> [f32; KNOBS] {
    tuning_column(text, &format!("{}_{}", prefix, band(fs)))
}

/// `y = x - 30`, `m` truncated toward zero, `w = y - float(m)`, then `m` limited to 0..119 (`w` is taken before the limit).
#[inline(always)]
pub fn table_pos(x: f32) -> (usize, f32) {
    let y = x - 30.0;
    let m = y as i32;
    let w = y - m as f32;
    (m.clamp(0, 119) as usize, w)
}

/// Lanes of `a` where `mask` is set, else lanes of `b`.
#[inline(always)]
pub fn pick(mask: __m128, a: F4, b: F4) -> F4 {
    F4(unsafe { _mm_or_ps(_mm_and_ps(mask, a.0), _mm_andnot_ps(mask, b.0)) })
}

/// The dead-zone soft clip of the Multimode, by mask, with the clip's work skipped when no lane leaves the zone.
#[inline(always)]
pub fn dead_zone(u: F4, q: f32) -> F4 {
    unsafe {
        let qv = F4::splat(q);
        let nq = F4::splat(-q);
        let pos = _mm_cmpge_ps(u.0, qv.0);
        let neg = _mm_cmple_ps(u.0, nq.0);
        let any = _mm_or_ps(pos, neg);
        if _mm_movemask_ps(any) == 0 {
            return u;
        }
        let shift = F4(_mm_or_ps(_mm_and_ps(pos, qv.0), _mm_and_ps(neg, nq.0)));
        let c = (u - shift).tclip() + shift;
        pick(any, c, u)
    }
}

const GRID: [f32; 4] = [0.9997, 0.9897, 0.9602, 0.619];

/// One sample's run of the root finder of `filter-multimode.md` section 6: hands out candidates, takes residuals, says which lane
/// of the last evaluation to commit. It differs from the Ladder's `SolveRun` in how the secant point joins the grid.
#[derive(Clone, Copy)]
pub struct Root {
    pub g: f32,
    z: f32,
    m: f32,
    tol: f32,
    bracket: bool,
    p: f32,
    fp: f32,
    q: f32,
    fq: f32,
    pub cand: [f32; 4],
    pub sel: usize,
}

impl Root {
    #[inline(always)]
    pub fn start(s: &Solver, tol: f32) -> Root {
        let c = ((s.f0 - (s.f1 + s.f1)) + s.f2) * 0.5;
        let g = (((s.f1 - s.f2) - c) * 3.0 + s.f2) + c * 9.0;
        let z = g - s.p;
        let y = s.w + 1e-7;
        Root { g, z, m: 1.0, tol, bracket: true, p: 0.0, fp: 0.0, q: 0.0, fq: 0.0, cand: Self::spread(z, y), sel: 0 }
    }

    #[inline(always)]
    fn spread(z: f32, y: f32) -> [f32; 4] {
        [z - (-1.0 * y), z - (-0.5 * y), z - (0.5 * y), z - (1.0 * y)]
    }

    /// Takes the residuals of `cand`; true when solved, with `sel` the lane to commit.
    #[inline(always)]
    pub fn feed(&mut self, r: &[f32; 4]) -> bool {
        let c = self.cand;
        if self.bracket {
            let i = if !(r[1] * r[0] > 0.0) {
                1
            } else if !(r[2] * r[1] > 0.0) {
                2
            } else if !(r[3] * r[2] > 0.0) {
                3
            } else {
                self.m = (self.m as f64 * 1.5) as f32;
                if self.m <= 1e30 {
                    self.cand = Self::spread(self.z, self.m * r[3]);
                    return false;
                }
                3
            };
            self.p = c[i - 1];
            self.fp = r[i - 1];
            self.q = c[i];
            self.fq = r[i];
            self.sel = if self.fp.abs() > self.fq.abs() { i } else { i - 1 };
            self.bracket = false;
        } else {
            let x = [self.p, c[0], c[1], c[2], c[3], self.q];
            let f = [self.fp, r[0], r[1], r[2], r[3], self.fq];
            let mut k = 1;
            while k < 5 && f[k] * f[k - 1] > 0.0 {
                k += 1;
            }
            self.p = x[k - 1];
            self.fp = f[k - 1];
            self.q = x[k];
            self.fq = f[k];
            self.sel = if k <= 4 { k - 1 } else { 3 };
        }
        if !((self.p - self.q).abs() > self.tol) {
            return true;
        }
        self.round();
        false
    }

    #[inline(always)]
    fn grid(p: f32, q: f32) -> [f32; 4] {
        [
            p * GRID[0] + q * (1.0 - GRID[0]),
            p * GRID[1] + q * (1.0 - GRID[1]),
            p * GRID[2] + q * (1.0 - GRID[2]),
            p * GRID[3] + q * (1.0 - GRID[3]),
        ]
    }

    #[inline(always)]
    fn round(&mut self) {
        if self.fp.abs() > self.fq.abs() {
            core::mem::swap(&mut self.p, &mut self.q);
            core::mem::swap(&mut self.fp, &mut self.fq);
        }
        let e = self.fp - self.fq;
        if e.abs() > 1e-10 {
            let s = self.p - ((self.p - self.q) * self.fp) * rcpn(e);
            let inside = (self.p < s && s < self.q) || (self.q < s && s < self.p);
            if inside {
                if self.p > self.q {
                    core::mem::swap(&mut self.p, &mut self.q);
                    core::mem::swap(&mut self.fp, &mut self.fq);
                }
                let w = Self::grid(self.p, self.q);
                self.cand = [
                    if w[0] < s { w[0] } else { s },
                    if w[1] < s {
                        w[1]
                    } else if s < w[0] {
                        w[0]
                    } else {
                        s
                    },
                    if w[2] < s {
                        w[2]
                    } else if s < w[1] {
                        w[1]
                    } else {
                        s
                    },
                    if s < w[2] { w[2] } else { s },
                ];
                return;
            }
        }
        self.cand = Self::grid(self.p, self.q);
    }
}

/// What the common loop knows of a call; a model reads it too.
pub struct CallK<'a> {
    pub os: &'a OsConst,
    pub v: &'a VcfSet,
    pub g12: f32,
    pub acc: i32,
    pub fb_on: bool,
}

/// One sample's evaluation: the values that do not depend on the candidate, ready for lanes of candidates.
pub trait Eval {
    /// What a commit takes from the last evaluation.
    type L: Copy;
    /// Lanes for candidates `c`, and the residuals.
    fn eval(&self, c: F4) -> (Self::L, F4);
}

/// Solve one sample.
#[inline(always)]
pub fn solve<E: Eval>(e: &E, run: &mut Root) -> E::L {
    loop {
        let (l, r) = e.eval(F4::load(&run.cand));
        if run.feed(&r.arr()) {
            return l;
        }
    }
}

/// Solve one sample of two voices together. Each evaluation's latency is what costs, so two running side by side take little more
/// than one (`performance.md`, Filters in lockstep). The same values as two calls of [`solve`].
#[inline(always)]
pub fn solve_two<E: Eval, G: Eval>(a: &E, ra: &mut Root, b: &G, rb: &mut Root) -> (E::L, G::L) {
    let (mut la, r1) = a.eval(F4::load(&ra.cand));
    let (mut lb, r2) = b.eval(F4::load(&rb.cand));
    let mut da = ra.feed(&r1.arr());
    let mut db = rb.feed(&r2.arr());
    while !da && !db {
        let (l1, r1) = a.eval(F4::load(&ra.cand));
        let (l2, r2) = b.eval(F4::load(&rb.cand));
        la = l1;
        lb = l2;
        da = ra.feed(&r1.arr());
        db = rb.feed(&r2.arr());
    }
    while !da {
        let (l1, r1) = a.eval(F4::load(&ra.cand));
        la = l1;
        da = ra.feed(&r1.arr());
    }
    while !db {
        let (l2, r2) = b.eval(F4::load(&rb.cand));
        lb = l2;
        db = rb.feed(&r2.arr());
    }
    (la, lb)
}

/// One model's part in a call: the model's state plus the call's constants.
pub trait Kernel {
    /// The model's noise corner.
    fn nhz(&self) -> f32;
    /// VCA Mode 1 without Offset (Multimode's two-stage modes).
    #[inline(always)]
    fn mode1_plain(&self) -> bool {
        false
    }
    /// A clear: states to 0 and the coefficients from the targets.
    fn clear(&mut self, cut: f32, res: f32);
    /// The start of every call: increments 0, solver contexts reset.
    fn begin(&mut self);
    /// The control step's coefficient ramps toward cutoff `x` and `res`.
    fn ramps(&mut self, x: f32, res: f32, st: f32);
    /// One os sample of the ramps.
    fn ramp_step(&mut self);
    /// The FilterFM path: coefficients directly from `x` and `res`.
    fn lookup(&mut self, x: f32, res: f32);
    /// The shape mix of this sample, before the smoothed values advance.
    #[inline(always)]
    fn mix(&mut self, _shp: f32) {}
    /// x0 plus feedback, then the input gain.
    fn input(&self, k: &CallK, x0: f32, fb: f32, gprev: f32) -> f32;
    /// The filter: from the model's input to the value ahead of the DC blocker.
    fn sample(&mut self, k: &CallK, x: f32) -> f32;
    /// Two voices' [`Kernel::sample`] together.
    fn sample_two(p: &mut Self, q: &mut Self, k: &CallK, xp: f32, xq: f32) -> (f32, f32);
}

/// The per-voice state common to the three models.
pub struct Common {
    res: Resamplers,
    cut_b: f32,
    cut: f32,
    rs: f32,
    ffm: f32,
    fb: f32,
    kf: f32,
    shp: f32,
    d_cut_b: f32,
    d_cut: f32,
    d_res: f32,
    d_ffm: f32,
    d_fb: f32,
    d_kf: f32,
    d_shp: f32,
    na: u32,
    nb: u32,
    nlp: f32,
    dc_in: f32,
    dc_out: f32,
    vca: Vca,
    started: bool,
    last_o: usize,
}

/// A call's constants, the same for every voice of the span.
pub struct Call<'a> {
    x: &'a FilterCtx<'a>,
    os: &'a OsConst,
    v: &'a VcfSet,
    vs: &'a VcaSet,
    fv: f32,
    st: f32,
    kk: f32,
    ncoef: f32,
    ncoef1: f32,
    p0: usize,
    spc: usize,
    t_off: f32,
    plain: bool,
    rn1: f32,
    t12: f32,
    ns: usize,
}

impl<'a> Call<'a> {
    #[inline(always)]
    fn new(x: &'a FilterCtx<'a>, nhz: f32, plain: bool) -> Call<'a> {
        let ft = x.ft;
        let os = if x.acc >= 2 { &ft.os4 } else { &ft.os2 };
        let n1 = (16 * x.n) as f32;
        let fv = (1.0 / n1) * os.finv;
        let st = os.kc;
        let fs = ft.fs;
        let fso = if os.o == 4 { fs * 4.0 } else { fs + fs };
        let ncoef = (1.0 / (fso + nhz)) * (fso - nhz);
        Call {
            x,
            os,
            v: &x.s.vcf,
            vs: &x.s.vca,
            fv,
            st,
            kk: st / fv,
            ncoef,
            ncoef1: 1.0 - ncoef,
            p0: 4 * x.c,
            spc: 4 * os.o,
            t_off: crate::dsp::tclip(x.s.vca.offset),
            plain,
            rn1: ft.rn1,
            t12: ft.t12,
            ns: 16 * x.n,
        }
    }

    #[inline(always)]
    fn k(&self) -> CallK<'a> {
        CallK { os: self.os, v: self.v, g12: self.x.ft.g, acc: self.x.acc, fb_on: self.x.fb_gate }
    }
}

#[inline(always)]
fn unit_limit(x: f32) -> f32 {
    let a = if x < 1.0 { x } else { 1.0 };
    if a > 0.0 {
        a
    } else {
        0.0
    }
}

impl Common {
    /// `OPT/TransM` above 0 at a note starting a stopped voice: the output DC blocker's two states go to 0.
    #[inline]
    pub fn reset_dc(&mut self) {
        self.dc_in = 0.0;
        self.dc_out = 0.0;
    }

    pub fn new(seed: (u32, u32)) -> Common {
        Common {
            res: Resamplers::new(),
            cut_b: 0.0,
            cut: 0.0,
            rs: 0.0,
            ffm: 0.0,
            fb: 0.0,
            kf: 0.0,
            shp: 0.0,
            d_cut_b: 0.0,
            d_cut: 0.0,
            d_res: 0.0,
            d_ffm: 0.0,
            d_fb: 0.0,
            d_kf: 0.0,
            d_shp: 0.0,
            na: seed.0,
            nb: seed.1,
            nlp: 0.0,
            dc_in: 0.0,
            dc_out: 0.0,
            vca: Vca::default(),
            started: false,
            last_o: 0,
        }
    }

    /// Targets, clear, ramps, VCA set-up.
    #[inline(always)]
    fn begin<K: Kernel>(&mut self, m: &mut K, c: &Call, io: &FilterIo) {
        let v = c.v;
        let rows = io.rows;
        let p0 = c.p0;
        let mut cut_t = (io.spread + io.spread) + v.frequency;
        if 147.0 <= cut_t {
            cut_t = 147.0;
        }
        if !(30.0 <= cut_t) {
            cut_t = 30.0;
        }
        let res_t = unit_limit(((rows[v.res_src * 16 + p0] * v.res_depth + io.mx.res) + v.resonance) * 0.01);
        let ffm_t = (rows[v.ffm_src * 16 + p0] * v.ffm_depth + v.filter_fm) * 0.041666668;
        let fb_t = (rows[v.fb_src * 16 + p0] * v.fb_depth + v.feedback) * 0.01;
        let kf_t = v.key_follow;
        let shp_t = unit_limit((rows[v.shape_src * 16 + p0] * v.shape_depth + v.shape_mix) * 0.01);
        let o = c.os.o;
        if io.clear || !self.started || self.last_o != o {
            self.started = true;
            self.cut_b = cut_t;
            self.cut = cut_t;
            self.rs = res_t;
            self.ffm = ffm_t;
            self.fb = fb_t;
            self.kf = kf_t;
            self.shp = shp_t;
            self.dc_in = 0.0;
            self.dc_out = 0.0;
            self.res = Resamplers::new();
            m.clear(cut_t, res_t);
        }
        self.last_o = o;
        m.begin();
        let fv = c.fv;
        self.d_cut_b = (cut_t - self.cut_b) * fv;
        self.d_res = (res_t - self.rs) * fv;
        self.d_ffm = (ffm_t - self.ffm) * fv;
        self.d_fb = (fb_t - self.fb) * fv;
        self.d_kf = (kf_t - self.kf) * fv;
        self.d_shp = (shp_t - self.shp) * fv;
        self.vca.begin(c.vs, c.x.n);
    }

    /// Control step `j`.
    #[inline(always)]
    fn control<K: Kernel>(&mut self, m: &mut K, c: &Call, io: &FilterIo, j: usize) {
        let v = c.v;
        let rows = io.rows;
        let p = c.p0 + j;
        let st = c.st;
        self.vca.control(c.vs, rows, io.mx, p, j, st);
        let mut t = ((rows[v.fm1_src * 16 + p] * v.fm1_depth + self.cut_b) + rows[v.fm2_src * 16 + p] * v.fm2_depth)
            + ((rows[8 * 16 + p] + 0.0) * self.kf) * 0.64;
        if io.mx.any_cut {
            t = t + io.mx.cut[j];
        }
        let t = if t < 147.0 { t } else { 147.0 };
        let t = if t > 30.0 { t } else { 30.0 };
        self.d_cut = (t - (self.d_cut_b * c.kk + self.cut)) * st + self.d_cut_b;
        if self.ffm.abs() < 0.01 {
            let xc = if self.cut <= 147.0 { self.cut } else { 147.0 };
            m.ramps(xc, self.rs, st);
        }
    }

    /// An os sample up to the model's input.
    #[inline(always)]
    fn ahead<K: Kernel>(&mut self, m: &mut K, c: &Call, k: &CallK, a: f32, b: f32) -> f32 {
        if self.ffm.abs() < 0.01 {
            m.ramp_step();
        } else {
            let xc = (b * 12.0) * self.ffm + self.cut;
            let xc = if xc <= 147.0 { xc } else { 147.0 };
            let xc = if 30.0 <= xc { xc } else { 30.0 };
            m.lookup(xc, self.rs);
        }
        m.mix(self.shp);
        self.cut_b = self.d_cut_b + self.cut_b;
        self.cut = self.d_cut + self.cut;
        self.rs = self.d_res + self.rs;
        self.ffm = self.d_ffm + self.ffm;
        self.fb = self.d_fb + self.fb;
        self.kf = self.d_kf + self.kf;
        self.shp = self.d_shp + self.shp;
        self.nb = self.nb.wrapping_add(0x3336_A4C3);
        self.na = (self.na ^ 0x0008_C52F).wrapping_add(self.nb);
        let nf = f32::from_bits((self.na & 0x7F_FFFF) | 0x3F80_0000);
        self.nlp = ((nf - 1.0) + 0.0) * c.ncoef1 + c.ncoef * self.nlp;
        let x0 = self.nlp * 0.0005 + a;
        m.input(k, x0, self.fb, self.vca.gg)
    }

    /// DC blocker, VCA, Mode shaper and output stage of an os sample.
    #[inline(always)]
    fn behind(&mut self, c: &Call, value: f32) -> (f32, f32) {
        let od = (value - self.dc_in) + c.os.co * self.dc_out;
        self.dc_in = value;
        self.dc_out = od;
        let (l, r) = self.vca.tick(od);
        match c.vs.mode {
            1 => {
                if c.plain {
                    let w = F4::new(l, r, 0.0, 0.0).tclip().arr();
                    (w[0], w[1])
                } else {
                    let off = c.vs.offset;
                    let w = F4::new(off + l, off + r, 0.0, 0.0).tclip().arr();
                    (w[0] - c.t_off, w[1] - c.t_off)
                }
            }
            2 => self.vca.stage(c.os.cs, c.rn1, c.t12, l, r),
            _ => (l, r),
        }
    }

    /// One call (`filter-multimode.md` section 9).
    #[inline(always)]
    pub fn run<K: Kernel>(&mut self, m: &mut K, x: &FilterCtx, io: &mut FilterIo) {
        let c = Call::new(x, m.nhz(), m.mode1_plain());
        self.begin(m, &c, io);
        let co = os_coefs();
        let (ns, o) = (c.ns, c.os.o);
        let (mut fa, mut fb) = ([0.0f32; 256], [0.0f32; 256]);
        let (mut fl, mut fr) = ([0.0f32; 256], [0.0f32; 256]);
        self.res.up(&co, o, &io.a[..ns], &io.b[..ns], &mut fa, &mut fb);
        let k = c.k();
        let mut i = 0;
        for j in 0..4 * x.n {
            self.control(m, &c, io, j);
            for _ in 0..c.spc {
                let inp = self.ahead(m, &c, &k, fa[i], fb[i]);
                let value = m.sample(&k, inp);
                let (l, r) = self.behind(&c, value);
                fl[i] = l;
                fr[i] = r;
                i += 1;
            }
        }
        self.res.down(&co, o, &fl, &fr, &mut io.l[..ns], &mut io.r[..ns]);
    }

    /// Two voices' calls in step: the same samples as two [`Common::run`], which touch no shared state.
    #[inline(always)]
    pub fn run_two<K: Kernel>(p: &mut Common, q: &mut Common, mp: &mut K, mq: &mut K, x: &FilterCtx, iop: &mut FilterIo, ioq: &mut FilterIo) {
        let c = Call::new(x, mp.nhz(), mp.mode1_plain());
        p.begin(mp, &c, iop);
        q.begin(mq, &c, ioq);
        let co = os_coefs();
        let (ns, o) = (c.ns, c.os.o);
        let (mut pa, mut pb, mut qa, mut qb) = ([0.0f32; 256], [0.0f32; 256], [0.0f32; 256], [0.0f32; 256]);
        let (mut pl, mut pr, mut ql, mut qr) = ([0.0f32; 256], [0.0f32; 256], [0.0f32; 256], [0.0f32; 256]);
        p.res.up(&co, o, &iop.a[..ns], &iop.b[..ns], &mut pa, &mut pb);
        q.res.up(&co, o, &ioq.a[..ns], &ioq.b[..ns], &mut qa, &mut qb);
        let k = c.k();
        let mut i = 0;
        for j in 0..4 * x.n {
            p.control(mp, &c, iop, j);
            q.control(mq, &c, ioq, j);
            for _ in 0..c.spc {
                let ip = p.ahead(mp, &c, &k, pa[i], pb[i]);
                let iq = q.ahead(mq, &c, &k, qa[i], qb[i]);
                let (vp, vq) = K::sample_two(mp, mq, &k, ip, iq);
                let (l, r) = p.behind(&c, vp);
                pl[i] = l;
                pr[i] = r;
                let (l, r) = q.behind(&c, vq);
                ql[i] = l;
                qr[i] = r;
                i += 1;
            }
        }
        p.res.down(&co, o, &pl, &pr, &mut iop.l[..ns], &mut iop.r[..ns]);
        q.res.down(&co, o, &ql, &qr, &mut ioq.l[..ns], &mut ioq.r[..ns]);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FilterTables, MatrixSums};
    use super::*;
    use crate::params::{id, COUNT, PARAMS};
    use crate::settings::Settings;

    fn settings(model: f32, f: impl Fn(&mut [f32; COUNT])) -> Settings {
        let mut v = [0.0f32; COUNT];
        for (i, p) in PARAMS.iter().enumerate() {
            v[i] = p.default;
        }
        v[id::VCF1_MODEL] = model;
        v[id::VCF1_RESONANCE] = 80.0;
        v[id::VCF1_FILTERFM] = 3.0;
        f(&mut v);
        Settings::read(&v)
    }

    fn input(seed: f32, i: usize) -> f32 {
        ((i as f32 * 0.137 + seed).sin() * 0.4) + ((i as f32 * 0.0191).cos() * 0.2)
    }

    /// `run_two` gives what two `run` give, for the three models.
    #[test]
    fn two_in_step_equal_two_alone() {
        use crate::filter::{bite::Bite, multimode::Multimode, uhbie::Uhbie};
        let ft = FilterTables::new(48000.0);
        let mut rows = [0.0f32; 448];
        rows[14 * 16..15 * 16].fill(1.0);
        rows[5 * 16..6 * 16].fill(1.0);
        let mx = MatrixSums::zero();
        let cases: [(f32, i32, f32); 9] =
            [(2.0, 2, 0.0), (2.0, 2, 1.0), (2.0, 1, 2.0), (2.0, 3, 3.0), (3.0, 2, 1.0), (3.0, 3, 2.0), (4.0, 2, 0.0), (4.0, 0, 0.0), (4.0, 1, 0.0)];
        for (model, acc, sub) in cases {
            let s = settings(model, |v| {
                v[id::VCF1_SVFMODE] = sub;
                v[id::VCF1_SLNKYREVISION] = sub;
                v[id::VCA1_MODE] = sub;
            });
            let x = FilterCtx { s: &s, ft: &ft, acc, c: 0, n: 4, fb_gate: true };
            let (mut a1, mut b1) = (Multimode::new((1, 2)), Multimode::new((3, 4)));
            let (mut a2, mut b2) = (Multimode::new((1, 2)), Multimode::new((3, 4)));
            let (mut c1, mut d1) = (Bite::new((1, 2)), Bite::new((3, 4)));
            let (mut c2, mut d2) = (Bite::new((1, 2)), Bite::new((3, 4)));
            let (mut e1, mut f1) = (Uhbie::new((1, 2)), Uhbie::new((3, 4)));
            let (mut e2, mut f2) = (Uhbie::new((1, 2)), Uhbie::new((3, 4)));
            let mut t = 0;
            for _ in 0..12 {
                let ia: Vec<f32> = (0..64).map(|i| input(0.0, t + i)).collect();
                let ib: Vec<f32> = (0..64).map(|i| input(1.0, t + i)).collect();
                t += 64;
                let (mut o, mut p, mut q, mut r) = ([0.0f32; 64], [0.0f32; 64], [0.0f32; 64], [0.0f32; 64]);
                let (mut o2, mut p2, mut q2, mut r2) = ([0.0f32; 64], [0.0f32; 64], [0.0f32; 64], [0.0f32; 64]);
                {
                    let io = |l, r| FilterIo { rows: &rows, mx: &mx, a: &ia, b: &ib, l, r, spread: 0.0, clear: false };
                    let (mut i1, mut i2) = (io(&mut o, &mut p), io(&mut q, &mut r));
                    match model as i32 {
                        2 => {
                            a1.run(&x, &mut i1);
                            b1.run(&x, &mut i2);
                        }
                        3 => {
                            c1.run(&x, &mut i1);
                            d1.run(&x, &mut i2);
                        }
                        _ => {
                            e1.run(&x, &mut i1);
                            f1.run(&x, &mut i2);
                        }
                    }
                }
                {
                    let io = |l, r| FilterIo { rows: &rows, mx: &mx, a: &ia, b: &ib, l, r, spread: 0.0, clear: false };
                    let (mut i1, mut i2) = (io(&mut o2, &mut p2), io(&mut q2, &mut r2));
                    match model as i32 {
                        2 => Multimode::run_two(&mut a2, &mut b2, &x, &mut i1, &mut i2),
                        3 => Bite::run_two(&mut c2, &mut d2, &x, &mut i1, &mut i2),
                        _ => Uhbie::run_two(&mut e2, &mut f2, &x, &mut i1, &mut i2),
                    }
                }
                let bits = |a: &[f32; 64]| a.map(f32::to_bits);
                assert_eq!(bits(&o), bits(&o2), "{model} {acc} {sub}");
                assert_eq!(bits(&r), bits(&r2), "{model} {acc} {sub}");
            }
        }
    }

    #[test]
    fn secant_point_joins_the_grid_by_the_pages_comparisons() {
        let s = Solver { f0: 0.0, f1: 0.0, f2: 0.0, w: 1e-2, p: 0.0, e: 0.0 };
        let mut run = Root::start(&s, 1e-6);
        // a line through 0 at c = 1e-3: the first bracket then refines
        let f = |c: f32| c - 1e-3;
        for _ in 0..64 {
            let c = run.cand;
            if run.feed(&[f(c[0]), f(c[1]), f(c[2]), f(c[3])]) {
                break;
            }
        }
        assert!((run.cand[run.sel] - 1e-3).abs() < 1e-5, "{:?} {}", run.cand, run.sel);
    }
}
