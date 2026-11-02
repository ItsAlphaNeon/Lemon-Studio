// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Ladder and Cascade: the implicit solver filters (`pages/filter-ladder.md`, `pages/filter-cascade.md`).
//!
//! Two voices with one whole call in a span run their filters in lockstep, one AVX evaluation for both (`pages/performance.md`).

use super::{os_coefs, FilterCtx, FilterIo, OsConst, Resamplers, Tuning, Vca};
use crate::dsp::{rcpn, tclip, F4};
use crate::settings::VcfSet;
use core::arch::x86_64::*;

// Indices of the model's state in `Model::st` and of an evaluation's lanes.
const U: usize = 0;
const Y1: usize = 1;
const V1: usize = 2;
const Y2: usize = 3;
const V2: usize = 4;
const Y3: usize = 5;
const V3: usize = 6;
const Y4: usize = 7;
const H1: usize = 8;
const H2: usize = 9;
const FF: usize = 10;
const NST: usize = 11;

/// The solver's state kept across samples and calls.
#[derive(Clone, Copy, Debug, Default)]
pub struct Solver {
    pub f0: f32,
    pub f1: f32,
    pub f2: f32,
    pub w: f32,
    pub p: f32,
    pub e: f32,
}

impl Solver {
    #[inline(always)]
    pub fn reset_call(&mut self) {
        self.w = 1e-08;
        self.p = 1e-06;
        self.e = 0.0;
    }

    /// Step 9 of the solver: width and predictor.
    #[inline(always)]
    pub fn finish(&mut self, g: f32, f: f32) {
        let e = g - f;
        let dd = (self.p - e).abs();
        if dd > self.w {
            self.w = dd * 1.1;
        } else {
            self.w = ((self.w - dd) * 0.99) + dd;
        }
        self.p = (e + e) - self.e;
        self.e = e;
        self.f2 = self.f1;
        self.f1 = self.f0;
        self.f0 = f;
    }
}

const SOLVE_T: [f32; 4] = [0.9997, 0.9897, 0.9602, 0.619];

/// One sample's run of the root finder: hands out candidates, takes residuals, says when to commit which lane.
#[derive(Clone, Copy, Debug)]
pub struct SolveRun {
    pub g: f32,
    ctr: f32,
    mult: f32,
    tol: f32,
    bracket: bool,
    pt: (f32, f32),
    qt: (f32, f32),
    pub cand: [f32; 4],
    pub lane: usize,
}

impl SolveRun {
    #[inline(always)]
    pub fn start(s: &Solver, tol: f32) -> SolveRun {
        let c = ((s.f0 - (s.f1 + s.f1)) + s.f2) * 0.5;
        let g = (((s.f1 - s.f2) - c) * 3.0 + s.f2) + (c * 9.0);
        let ctr = g - s.p;
        let base = s.w + 1e-07;
        let mult = 1.0f32;
        let wd = mult * base;
        SolveRun {
            g,
            ctr,
            mult,
            tol,
            bracket: true,
            pt: (0.0, 0.0),
            qt: (0.0, 0.0),
            cand: Self::cands(ctr, wd),
            lane: 0,
        }
    }

    #[inline(always)]
    fn cands(ctr: f32, wd: f32) -> [f32; 4] {
        [ctr - (-1.0 * wd), ctr - (-0.5 * wd), ctr - (0.5 * wd), ctr - (1.0 * wd)]
    }

    /// Takes the residuals of `cand`. Returns true when the sample is solved: commit lane `lane` of this last evaluation.
    #[inline(always)]
    pub fn feed(&mut self, r: &[f32; 4]) -> bool {
        if self.bracket {
            let c = self.cand;
            let i = if !(r[1] * r[0] > 0.0) {
                1
            } else if !(r[2] * r[1] > 0.0) {
                2
            } else if !(r[3] * r[2] > 0.0) {
                3
            } else {
                // no sign change
                let base = r[3];
                self.mult = (self.mult as f64 * 1.5) as f32;
                if self.mult <= 1e+30 {
                    let wd = self.mult * base;
                    self.cand = Self::cands(self.ctr, wd);
                    return false;
                }
                3
            };
            self.lane = if r[i - 1].abs() > r[i].abs() { i } else { i - 1 };
            self.pt = (c[i - 1], r[i - 1]);
            self.qt = (c[i], r[i]);
            self.bracket = false;
        } else {
            let c = self.cand;
            let xs = [self.pt.0, c[0], c[1], c[2], c[3], self.qt.0];
            let rs = [self.pt.1, r[0], r[1], r[2], r[3], self.qt.1];
            let mut s = 1;
            while s < 5 && rs[s] * rs[s - 1] > 0.0 {
                s += 1;
            }
            self.lane = if s <= 4 { s - 1 } else { 3 };
            self.pt = (xs[s - 1], rs[s - 1]);
            self.qt = (xs[s], rs[s]);
        }
        if !((self.pt.0 - self.qt.0).abs() > self.tol) {
            return true;
        }
        self.next_round();
        false
    }

    #[inline(always)]
    fn next_round(&mut self) {
        if self.pt.1.abs() > self.qt.1.abs() {
            core::mem::swap(&mut self.pt, &mut self.qt);
        }
        let vs = |p: f32, q: f32| -> [f32; 4] {
            let mut v = [0.0; 4];
            for l in 0..4 {
                v[l] = (p * SOLVE_T[l]) + (q * (1.0 - SOLVE_T[l]));
            }
            v
        };
        let d = self.pt.1 - self.qt.1;
        if d.abs() > 1e-10 {
            let xs = self.pt.0 - (((self.pt.0 - self.qt.0) * self.pt.1) * rcpn(d));
            let (lo, hi) = if self.pt.0 < self.qt.0 { (self.pt.0, self.qt.0) } else { (self.qt.0, self.pt.0) };
            if xs > lo && xs < hi {
                if self.pt.0 > self.qt.0 {
                    core::mem::swap(&mut self.pt, &mut self.qt);
                }
                let mut v = vs(self.pt.0, self.qt.0);
                // drop V_3, insert xs in ascending order among V_0, V_1, V_2
                let mut at = 3;
                while at > 0 && v[at - 1] > xs {
                    v[at] = v[at - 1];
                    at -= 1;
                }
                v[at] = xs;
                self.cand = v;
                return;
            }
        }
        self.cand = vs(self.pt.0, self.qt.0);
    }
}

/// Everything of the kernel's per-voice state. Ladder and Cascade share one (`filter-cascade.md` section 7).
pub struct SolverFilter {
    res: Resamplers,
    pc: f32,
    qc: f32,
    qres: f32,
    qffm: f32,
    qfb: f32,
    qkey: f32,
    dc: f32,
    dres: f32,
    dffm: f32,
    dfb: f32,
    dkey: f32,
    sc: f32,
    a: f32,
    b: f32,
    k: f32,
    sa: f32,
    sb: f32,
    sk: f32,
    /// The committed model state.
    st: [f32; NST],
    tap: f32,
    solver: Solver,
    na: u32,
    nb: u32,
    nl: f32,
    bf: f32,
    wp: f32,
    np: f32,
    op: f32,
    vca: Vca,
    /// What the last call ran as: draft/fast/great class and model and colour.
    last: Option<(i32, bool, bool)>,
    k_cached: f32,
    fc_cached: f32,
}

/// The call's constants, derived from the context.
struct CallC<'a> {
    os: &'a OsConst,
    tune: &'a Tuning,
    v: &'a VcfSet,
    set_vca: &'a crate::settings::VcaSet,
    g: f32,
    rn1: f32,
    rn114: f32,
    t12: f32,
    f: f32,
    kk: f32,
    tol: f32,
    p0: usize,
    fb_gate: bool,
    casc: bool,
    twelve: bool,
    gf: f32,
    sf: f32,
    t_off: f32,
    vca_mode: i32,
}

/// Which tuning table and tolerances an accuracy uses.
#[inline]
fn acc_class(acc: i32) -> i32 {
    match acc {
        0 => 0,
        1 => 1,
        _ => 2,
    }
}

fn os_for<'a>(x: &FilterCtx<'a>) -> &'a OsConst {
    if x.acc >= 2 {
        &x.ft.os4
    } else {
        &x.ft.os2
    }
}

/// A lane-wise evaluation's inputs, computed once per sample.
#[derive(Clone, Copy, Default)]
struct Pre {
    i: f32,
    b: f32,
    k: f32,
    up: f32,
    y1a: f32,
    v1p: f32,
    y2a: f32,
    v2p: f32,
    y3a: f32,
    v3p: f32,
    y4a: f32,
    y4p: f32,
    ch1: f32,
    h1p: f32,
    ch2: f32,
    fc: f32,
    // Cascade
    y1p: f32,
    y2p: f32,
    y3p: f32,
    gf: f32,
    sf: f32,
}

macro_rules! kernels {
    ($V:ty, $($attr:tt)*) => {
        /// T lane by lane.
        $($attr)*
        fn t(x: $V) -> $V {
            let c = max(min(x, set1(3.2825)), set1(-3.2825));
            let q = mul(c, c);
            let d = add(mul(q, set1(9.0)), set1(27.0));
            let r = rcp(d);
            let rn = sub(r, mul(sub(mul(r, d), set1(1.0)), r));
            mul(rn, mul(add(q, set1(27.0)), c))
        }

        /// The Ladder's M(c): `out` takes all lanes of the state.
        $($attr)*
        fn ladder(p: &PreV, c: $V, rn114: $V) -> [$V; NST] {
            let d = add(mul(add(p.i, c), set1(8.88)), set1(-0.42));
            let d = add(mul(d, set1(0.14)), d);
            // the dead zone: T(d -+ 0.14) +- 0.14, else d
            let pos = cmp_ge(d, set1(0.14));
            let neg = cmp_le(d, set1(-0.14));
            let shift = or(and(pos, set1(0.14)), and(neg, set1(-0.14)));
            let clipped = add(t(sub(d, shift)), shift);
            let any = or(pos, neg);
            let dd = or(and(any, clipped), andnot(any, d));
            let u = add(mul(rn114, dd), set1(0.4102648));
            let y1 = sub(mul(add(u, p.up), p.b), p.y1a);
            let v1 = t(y1);
            let y2 = sub(mul(add(v1, p.v1p), p.b), p.y2a);
            let v2 = t(y2);
            let y3 = sub(mul(add(v2, p.v2p), p.b), p.y3a);
            let v3 = t(y3);
            let y4 = sub(mul(add(v3, p.v3p), p.b), p.y4a);
            let h1 = add(sub(y4, p.y4p), p.ch1);
            let h2 = add(sub(h1, p.h1p), p.ch2);
            let fi = mul(mul(neg_of(add(set1(0.14700001), h2)), set1(0.6112)), p.k);
            let f = add(t(fi), p.fc);
            [u, y1, v1, y2, v2, y3, v3, y4, h1, h2, f]
        }

        /// The Cascade's M(c).
        $($attr)*
        fn cascade(p: &PreV, c: $V) -> [$V; NST] {
            let s = mul(add(p.i, c), set1(0.19));
            let u = t(s);
            let y1 = sub(mul(add(u, p.up), p.b), p.y1a);
            let y2 = sub(mul(add(y1, p.y1p), p.b), p.y2a);
            let y3 = sub(mul(add(y2, p.y2p), p.b), p.y3a);
            let y4 = sub(mul(add(y3, p.y3p), p.b), p.y4a);
            let f = mul(t(mul(mul(y4, p.gf), p.k)), p.sf);
            let z = set1(0.0);
            [u, y1, z, y2, z, y3, z, y4, z, z, f]
        }
    };
}

/// Four lanes: one voice.
mod w4 {
    use super::*;

    pub struct PreV {
        pub i: __m128,
        pub b: __m128,
        pub k: __m128,
        pub up: __m128,
        pub y1a: __m128,
        pub v1p: __m128,
        pub y2a: __m128,
        pub v2p: __m128,
        pub y3a: __m128,
        pub v3p: __m128,
        pub y4a: __m128,
        pub y4p: __m128,
        pub ch1: __m128,
        pub h1p: __m128,
        pub ch2: __m128,
        pub fc: __m128,
        pub y1p: __m128,
        pub y2p: __m128,
        pub y3p: __m128,
        pub gf: __m128,
        pub sf: __m128,
    }

    #[inline(always)]
    pub fn pack(p: &Pre) -> PreV {
        macro_rules! s {
            ($f:ident) => {
                set1(p.$f)
            };
        }
        PreV {
            i: s!(i),
            b: s!(b),
            k: s!(k),
            up: s!(up),
            y1a: s!(y1a),
            v1p: s!(v1p),
            y2a: s!(y2a),
            v2p: s!(v2p),
            y3a: s!(y3a),
            v3p: s!(v3p),
            y4a: s!(y4a),
            y4p: s!(y4p),
            ch1: s!(ch1),
            h1p: s!(h1p),
            ch2: s!(ch2),
            fc: s!(fc),
            y1p: s!(y1p),
            y2p: s!(y2p),
            y3p: s!(y3p),
            gf: s!(gf),
            sf: s!(sf),
        }
    }

    #[inline(always)]
    pub fn set1(x: f32) -> __m128 {
        unsafe { _mm_set1_ps(x) }
    }
    #[inline(always)]
    fn add(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_add_ps(a, b) }
    }
    #[inline(always)]
    fn sub(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_sub_ps(a, b) }
    }
    #[inline(always)]
    fn mul(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_mul_ps(a, b) }
    }
    #[inline(always)]
    fn min(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_min_ps(a, b) }
    }
    #[inline(always)]
    fn max(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_max_ps(a, b) }
    }
    #[inline(always)]
    fn rcp(a: __m128) -> __m128 {
        unsafe { _mm_rcp_ps(a) }
    }
    #[inline(always)]
    fn cmp_ge(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_cmpge_ps(a, b) }
    }
    #[inline(always)]
    fn cmp_le(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_cmple_ps(a, b) }
    }
    #[inline(always)]
    fn and(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_and_ps(a, b) }
    }
    #[inline(always)]
    fn or(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_or_ps(a, b) }
    }
    #[inline(always)]
    fn andnot(a: __m128, b: __m128) -> __m128 {
        unsafe { _mm_andnot_ps(a, b) }
    }
    #[inline(always)]
    fn neg_of(a: __m128) -> __m128 {
        unsafe { _mm_xor_ps(a, _mm_set1_ps(-0.0)) }
    }

    kernels!(__m128, #[inline(always)] pub);
}

/// Eight lanes under AVX: two voices.
mod w8 {
    use super::*;

    #[derive(Clone, Copy)]
    pub struct PreV {
        pub i: __m256,
        pub b: __m256,
        pub k: __m256,
        pub up: __m256,
        pub y1a: __m256,
        pub v1p: __m256,
        pub y2a: __m256,
        pub v2p: __m256,
        pub y3a: __m256,
        pub v3p: __m256,
        pub y4a: __m256,
        pub y4p: __m256,
        pub ch1: __m256,
        pub h1p: __m256,
        pub ch2: __m256,
        pub fc: __m256,
        pub y1p: __m256,
        pub y2p: __m256,
        pub y3p: __m256,
        pub gf: __m256,
        pub sf: __m256,
    }

    #[inline]
    #[target_feature(enable = "avx")]
    pub fn both(a: f32, b: f32) -> __m256 {
        _mm256_set_m128(_mm_set1_ps(b), _mm_set1_ps(a))
    }

    #[inline]
    #[target_feature(enable = "avx")]
    pub fn pack(a: &Pre, b: &Pre) -> PreV {
        macro_rules! s {
            ($f:ident) => {
                both(a.$f, b.$f)
            };
        }
        PreV {
            i: s!(i),
            b: s!(b),
            k: s!(k),
            up: s!(up),
            y1a: s!(y1a),
            v1p: s!(v1p),
            y2a: s!(y2a),
            v2p: s!(v2p),
            y3a: s!(y3a),
            v3p: s!(v3p),
            y4a: s!(y4a),
            y4p: s!(y4p),
            ch1: s!(ch1),
            h1p: s!(h1p),
            ch2: s!(ch2),
            fc: s!(fc),
            y1p: s!(y1p),
            y2p: s!(y2p),
            y3p: s!(y3p),
            gf: s!(gf),
            sf: s!(sf),
        }
    }

    #[inline]
    #[target_feature(enable = "avx")]
    pub fn set1(x: f32) -> __m256 {
        _mm256_set1_ps(x)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn add(a: __m256, b: __m256) -> __m256 {
        _mm256_add_ps(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn sub(a: __m256, b: __m256) -> __m256 {
        _mm256_sub_ps(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn mul(a: __m256, b: __m256) -> __m256 {
        _mm256_mul_ps(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn min(a: __m256, b: __m256) -> __m256 {
        _mm256_min_ps(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn max(a: __m256, b: __m256) -> __m256 {
        _mm256_max_ps(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn rcp(a: __m256) -> __m256 {
        _mm256_rcp_ps(a)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn cmp_ge(a: __m256, b: __m256) -> __m256 {
        _mm256_cmp_ps::<_CMP_GE_OS>(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn cmp_le(a: __m256, b: __m256) -> __m256 {
        _mm256_cmp_ps::<_CMP_LE_OS>(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn and(a: __m256, b: __m256) -> __m256 {
        _mm256_and_ps(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn or(a: __m256, b: __m256) -> __m256 {
        _mm256_or_ps(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn andnot(a: __m256, b: __m256) -> __m256 {
        _mm256_andnot_ps(a, b)
    }
    #[inline]
    #[target_feature(enable = "avx")]
    fn neg_of(a: __m256) -> __m256 {
        _mm256_xor_ps(a, _mm256_set1_ps(-0.0))
    }

    kernels!(__m256, #[inline] #[target_feature(enable = "avx")] pub);
}

impl SolverFilter {
    pub fn new(noise: (u32, u32)) -> Box<SolverFilter> {
        Box::new(SolverFilter {
            res: Resamplers::new(),
            pc: 0.0,
            qc: 0.0,
            qres: 0.0,
            qffm: 0.0,
            qfb: 0.0,
            qkey: 0.0,
            dc: 0.0,
            dres: 0.0,
            dffm: 0.0,
            dfb: 0.0,
            dkey: 0.0,
            sc: 0.0,
            a: 0.0,
            b: 0.0,
            k: 0.0,
            sa: 0.0,
            sb: 0.0,
            sk: 0.0,
            st: [0.0; NST],
            tap: 0.0,
            solver: Solver::default(),
            na: noise.0,
            nb: noise.1,
            nl: 0.0,
            bf: 0.0,
            wp: 0.0,
            np: 0.0,
            op: 0.0,
            vca: Vca::default(),
            last: None,
            k_cached: f32::NAN,
            fc_cached: 0.0,
        })
    }

    fn call_c<'a>(x: &FilterCtx<'a>, casc: bool) -> CallC<'a> {
        let cl = acc_class(x.acc) as usize;
        let v = &x.s.vcf;
        let os = os_for(x);
        let rough = v.rough;
        let tune = if casc { &x.ft.cascade[rough as usize][cl] } else { &x.ft.ladder[cl] };
        let f = (1.0 / (16.0 * x.n as f32)) * os.finv;
        let (gf, sf) = if rough { (f32::from_bits(0xBFAEE632), f32::from_bits(0x3FB31562)) } else { (f32::from_bits(0xBFC4C2F9), f32::from_bits(0x3F8DDF12)) };
        CallC {
            os,
            tune,
            v,
            set_vca: &x.s.vca,
            g: x.ft.g,
            rn1: x.ft.rn1,
            rn114: x.ft.rn114,
            t12: x.ft.t12,
            f,
            kk: os.kc / f,
            tol: if x.acc == 3 { 1.9073486e-06 } else { 0.00048828125 },
            p0: 4 * x.c,
            fb_gate: x.fb_gate,
            casc,
            twelve: v.ladder_12db,
            gf,
            sf,
            t_off: tclip(x.s.vca.offset),
            vca_mode: x.s.vca.mode,
        }
    }

    /// `OPT/TransM` above 0 at a note starting a stopped voice: the output DC blocker's two states go to 0.
    #[inline]
    pub fn reset_dc(&mut self) {
        self.np = 0.0;
        self.op = 0.0;
    }

    /// Steps 1 and 2 of a call: targets, clear, ramps, solver reset.
    #[inline]
    fn begin(&mut self, x: &FilterCtx, c: &CallC, io: &FilterIo, casc: bool) {
        let v = c.v;
        let p0 = c.p0;
        let mut cut = (2.0 * io.spread) + v.frequency;
        if 147.0 <= cut {
            cut = 147.0;
        }
        if !(30.0 <= cut) {
            cut = 30.0;
        }
        let mut res = (io.rows[v.res_src * 16 + p0] * v.res_depth + io.mx.res) + v.resonance;
        res = res * 0.01;
        let res = if res < 1.0 { res } else { 1.0 };
        let res = if res > 0.0 { res } else { 0.0 };
        let ffm = (io.rows[v.ffm_src * 16 + p0] * v.ffm_depth + v.filter_fm) * 0.041666668;
        let fb = (io.rows[v.fb_src * 16 + p0] * v.fb_depth + v.feedback) * 0.01;
        let key = v.key_follow;

        let class = (acc_class(x.acc), casc, casc && v.rough);
        let changed = self.last != Some(class);
        // the same oversampling or draft flag with the same model and colour keeps the state
        let clear = io.clear || changed;
        self.last = Some(class);
        self.solver.reset_call();
        if clear {
            self.pc = cut;
            self.qc = cut;
            self.qres = res;
            self.qffm = ffm;
            self.qfb = fb;
            self.qkey = key;
            let (a, b) = c.tune.lookup(self.qc);
            self.a = a;
            self.b = b;
            self.k = if casc { self.qres * 8.0 } else { self.qres };
            self.st = [0.0; NST];
            self.tap = 0.0;
            self.res = Resamplers::new();
            self.np = 0.0;
            self.op = 0.0;
        }
        // the slopes of this call's last step that computed them, 0 before any
        self.sa = 0.0;
        self.sb = 0.0;
        self.sk = 0.0;
        let f = c.f;
        self.dc = (cut - self.pc) * f;
        self.dres = (res - self.qres) * f;
        self.dffm = (ffm - self.qffm) * f;
        self.dfb = (fb - self.qfb) * f;
        self.dkey = (key - self.qkey) * f;
        self.vca.begin(c.set_vca, x.n);
    }

    /// Per control step.
    #[inline(always)]
    fn control(&mut self, c: &CallC, io: &FilterIo, j: usize) {
        let p = c.p0 + j;
        let kc = c.os.kc;
        self.vca.control(c.set_vca, io.rows, io.mx, p, j, kc);
        let v = c.v;
        let mut m = ((io.rows[v.fm1_src * 16 + p] * v.fm1_depth + self.pc) + (io.rows[v.fm2_src * 16 + p] * v.fm2_depth))
            + (((io.rows[8 * 16 + p] + 0.0) * self.qkey) * 0.64);
        if io.mx.any_cut {
            m = m + io.mx.cut[j];
        }
        if !(m < 147.0) {
            m = 147.0;
        }
        if !(m > 30.0) {
            m = 30.0;
        }
        let e = (m - ((self.dc * c.kk) + self.qc)) * kc;
        self.sc = e + self.dc;
        if self.qffm.abs() < 0.01 {
            let x = if 147.0 < self.qc { 147.0 } else { self.qc };
            let (at, bt) = c.tune.lookup(x);
            self.sa = (at - self.a) * kc;
            self.sb = (bt - self.b) * kc;
            let kt = if c.casc { self.qres * 8.0 } else { self.qres };
            self.sk = (kt - self.k) * kc;
        }
    }

    /// The coefficient step, ramps and input of an oversampled sample; returns the model's x.
    #[inline(always)]
    fn input(&mut self, c: &CallC, a_s: f32, b_s: f32) -> f32 {
        if self.qffm.abs() < 0.01 {
            self.a = self.a + self.sa;
            self.b = self.b + self.sb;
            self.k = self.k + self.sk;
        } else {
            let mut x = ((b_s * 12.0) * self.qffm) + self.qc;
            if !(x < 147.0) {
                x = 147.0;
            }
            if !(x > 30.0) {
                x = 30.0;
            }
            let (a, b) = c.tune.lookup(x);
            self.a = a;
            self.b = b;
            self.k = if c.casc { self.qres * 8.0 } else { self.qres };
        }
        self.qres = self.qres + self.dres;
        self.qffm = self.qffm + self.dffm;
        self.qfb = self.qfb + self.dfb;
        self.qkey = self.qkey + self.dkey;
        self.pc = self.pc + self.dc;
        self.qc = self.qc + self.sc;
        // noise
        self.nb = self.nb.wrapping_add(0x3336_A4C3);
        self.na = (self.na ^ 0x0008_C52F).wrapping_add(self.nb);
        let u = f32::from_bits((self.na & 0x7F_FFFF) | 0x3F80_0000);
        self.nl = (((u - 1.0) + 0.0) * (1.0 - c.os.cn)) + (c.os.cn * self.nl);
        let mut x = (self.nl * 0.0005) + a_s;
        if c.casc {
            if c.fb_gate {
                x = x + (self.vca.gg * self.qfb);
            }
            x * c.g
        } else {
            if c.fb_gate {
                let w = tclip((self.qfb * 1.11) * self.vca.gg) * 1.862;
                self.bf = (c.os.cb * self.bf) + (w - self.wp);
                self.wp = w;
                x = x + self.bf;
            }
            x * (c.g * 1.12)
        }
    }

    /// What the evaluations need of this sample.
    #[inline(always)]
    fn pre(&mut self, c: &CallC, x: f32) -> Pre {
        let st = &self.st;
        if c.casc {
            let kk = self.k * 1.4518;
            Pre {
                i: ((0.056 * kk) + 1.0) * x,
                b: self.b,
                k: kk,
                up: st[U],
                y1a: self.a * st[Y1],
                y2a: self.a * st[Y2],
                y3a: self.a * st[Y3],
                y4a: self.a * st[Y4],
                y1p: st[Y1],
                y2p: st[Y2],
                y3p: st[Y3],
                gf: c.gf,
                sf: c.sf,
                ..Pre::default()
            }
        } else {
            let i = (tclip(0.08 * x) * 12.5) * 0.12;
            if self.k != self.k_cached {
                self.k_cached = self.k;
                self.fc_cached = tclip(0.0898464 * self.k);
            }
            Pre {
                i,
                b: self.b,
                k: self.k,
                up: st[U],
                y1a: st[Y1] * self.a,
                v1p: st[V1],
                y2a: st[Y2] * self.a,
                v2p: st[V2],
                y3a: st[Y3] * self.a,
                v3p: st[V3],
                y4a: st[Y4] * self.a,
                y4p: st[Y4],
                ch1: c.os.ch * st[H1],
                h1p: st[H1],
                ch2: c.os.ch * st[H2],
                fc: self.fc_cached,
                ..Pre::default()
            }
        }
    }

    /// DC blocker, VCA and output stage of one oversampled sample.
    #[inline(always)]
    fn output(&mut self, c: &CallC) -> (f32, f32) {
        let n = if c.casc { ((c.g * 16.0) * self.tap) * 5.25 } else { (c.g * 16.0) * self.tap };
        let o = (n - self.np) + (c.os.co * self.op);
        self.np = n;
        self.op = o;
        let (mut l, mut r) = self.vca.tick(o);
        if c.vca_mode == 1 {
            let v = F4::new(c.set_vca.offset + l, c.set_vca.offset + r, 0.0, 0.0).tclip().arr();
            l = v[0] - c.t_off;
            r = v[1] - c.t_off;
        }
        if !c.casc || c.vca_mode == 2 {
            return self.vca.stage(c.os.cs, c.rn1, c.t12, l, r);
        }
        (l, r)
    }

    #[inline(always)]
    fn commit(&mut self, c: &CallC, f: f32, g: f32) {
        self.tap = if c.twelve { self.st[Y2] } else { self.st[Y4] };
        self.solver.finish(g, f);
    }

    /// The Ladder's draft (no solver).
    #[inline(always)]
    fn draft_ladder(&mut self, c: &CallC, x: f32) {
        let i = (tclip(x * 0.08) * 12.5) * 0.12;
        let fp = self.st[FF];
        let d = ((i + fp) * 8.88) - 0.42;
        let q = 0.14f32;
        let mut w = (q + 1.0) * d;
        if q < w.abs() {
            if w < -q {
                w = tclip(q + w) - q;
            }
            if q < w {
                w = tclip(w - q) + q;
            }
        }
        let dd = w / (q + 1.0);
        let u = dd + 0.4102648;
        let (a, b, k) = (self.a, self.b, self.k);
        let s = &mut self.st;
        let y1 = ((u + s[U]) * b) - (s[Y1] * a);
        let v1 = tclip(y1);
        let y2 = ((v1 + s[V1]) * b) - (s[Y2] * a);
        let v2 = tclip(y2);
        let y3 = ((v2 + s[V2]) * b) - (s[Y3] * a);
        let v3 = tclip(y3);
        let y4 = ((v3 + s[V3]) * b) - (s[Y4] * a);
        let h1 = (y4 - s[Y4]) + (c.os.ch * s[H1]);
        let h2 = (h1 - s[H1]) + (c.os.ch * s[H2]);
        let f = tclip(((-(0.14700001 + h2)) * 0.6112) * k) + tclip(0.0898464 * k);
        *s = [u, y1, v1, y2, v2, y3, v3, y4, h1, h2, f];
        self.tap = if c.twelve { y2 } else { y4 };
    }

    /// The Cascade's draft.
    #[inline(always)]
    fn draft_cascade(&mut self, c: &CallC, x: f32) {
        let k = self.k;
        let fp = self.st[FF];
        let s0 = (((((k * 1.4518) * 0.056) + 1.0) * x) + fp) * 0.19;
        let u = tclip(s0);
        let kk = k * 1.4518;
        let (a, b) = (self.a, self.b);
        let s = &mut self.st;
        let y1 = ((u + s[U]) * b) - (a * s[Y1]);
        let y2 = ((y1 + s[Y1]) * b) - (a * s[Y2]);
        let y3 = ((y2 + s[Y2]) * b) - (a * s[Y3]);
        let y4 = ((y3 + s[Y3]) * b) - (a * s[Y4]);
        let f = tclip((y4 * c.gf) * kk) * c.sf;
        *s = [u, y1, 0.0, y2, 0.0, y3, 0.0, y4, 0.0, 0.0, f];
        self.tap = if c.twelve { y2 } else { y4 };
    }

    /// One filter alone.
    pub fn run(&mut self, x: &FilterCtx, io: &mut FilterIo, casc: bool) {
        let c = Self::call_c(x, casc);
        self.begin(x, &c, io, casc);
        let co = os_coefs();
        let os = c.os.o;
        let ns = 16 * x.n;
        let (mut fa, mut fb) = ([0.0f32; 256], [0.0f32; 256]);
        let (mut fl, mut fr) = ([0.0f32; 256], [0.0f32; 256]);
        self.res.up(&co, os, &io.a[..ns], &io.b[..ns], &mut fa, &mut fb);
        let per_step = 4 * os;
        let mut i = 0;
        let draft = x.acc == 0;
        let rn114 = w4::set1(c.rn114);
        for j in 0..4 * x.n {
            self.control(&c, io, j);
            for _ in 0..per_step {
                let inp = self.input(&c, fa[i], fb[i]);
                if draft {
                    if casc {
                        self.draft_cascade(&c, inp);
                    } else {
                        self.draft_ladder(&c, inp);
                    }
                } else {
                    let pre = self.pre(&c, inp);
                    let pv = w4::pack(&pre);
                    let mut run = SolveRun::start(&self.solver, c.tol);
                    let mut ev;
                    loop {
                        let cv = unsafe { _mm_loadu_ps(run.cand.as_ptr()) };
                        ev = if casc { w4::cascade(&pv, cv) } else { w4::ladder(&pv, cv, rn114) };
                        let r = unsafe { F4(_mm_sub_ps(cv, ev[FF])).arr() };
                        if run.feed(&r) {
                            break;
                        }
                    }
                    let l = run.lane;
                    for s in 0..NST {
                        self.st[s] = F4(ev[s]).arr()[l];
                    }
                    let f = self.st[FF];
                    self.commit(&c, f, run.g);
                }
                let (ol, or) = self.output(&c);
                fl[i] = ol;
                fr[i] = or;
                i += 1;
            }
        }
        self.res.down(&co, os, &fl, &fr, &mut io.l[..ns], &mut io.r[..ns]);
    }
}

/// Two filters with one whole call each, in step: one AVX evaluation for both per solver round.
///
/// # Safety
/// The CPU has AVX.
#[target_feature(enable = "avx")]
unsafe fn run_pair_avx(p: &mut SolverFilter, q: &mut SolverFilter, x: &FilterCtx, iop: &mut FilterIo, ioq: &mut FilterIo, casc: bool) {
    let c = SolverFilter::call_c(x, casc);
    p.begin(x, &c, iop, casc);
    q.begin(x, &c, ioq, casc);
    let co = os_coefs();
    let os = c.os.o;
    let ns = 16 * x.n;
    let (mut pa, mut pb, mut qa, mut qb) = ([0.0f32; 256], [0.0f32; 256], [0.0f32; 256], [0.0f32; 256]);
    let (mut pl, mut pr, mut ql, mut qr) = ([0.0f32; 256], [0.0f32; 256], [0.0f32; 256], [0.0f32; 256]);
    p.res.up(&co, os, &iop.a[..ns], &iop.b[..ns], &mut pa, &mut pb);
    q.res.up(&co, os, &ioq.a[..ns], &ioq.b[..ns], &mut qa, &mut qb);
    let per_step = 4 * os;
    let rn114 = w8::set1(c.rn114);
    let mut i = 0;
    for j in 0..4 * x.n {
        p.control(&c, iop, j);
        q.control(&c, ioq, j);
        for _ in 0..per_step {
            let ip = p.input(&c, pa[i], pb[i]);
            let iq = q.input(&c, qa[i], qb[i]);
            let pre_p = p.pre(&c, ip);
            let pre_q = q.pre(&c, iq);
            let pv = w8::pack(&pre_p, &pre_q);
            let mut rp = SolveRun::start(&p.solver, c.tol);
            let mut rq = SolveRun::start(&q.solver, c.tol);
            let (mut done_p, mut done_q) = (false, false);
            loop {
                let mut cand = [0.0f32; 8];
                cand[..4].copy_from_slice(&rp.cand);
                cand[4..].copy_from_slice(&rq.cand);
                let cv = _mm256_loadu_ps(cand.as_ptr());
                let ev = if casc { w8::cascade(&pv, cv) } else { w8::ladder(&pv, cv, rn114) };
                let mut r = [0.0f32; 8];
                _mm256_storeu_ps(r.as_mut_ptr(), _mm256_sub_ps(cv, ev[FF]));
                if !done_p {
                    let rr = [r[0], r[1], r[2], r[3]];
                    if rp.feed(&rr) {
                        done_p = true;
                        let l = rp.lane;
                        for s in 0..NST {
                            let mut a = [0.0f32; 8];
                            _mm256_storeu_ps(a.as_mut_ptr(), ev[s]);
                            p.st[s] = a[l];
                        }
                    }
                }
                if !done_q {
                    let rr = [r[4], r[5], r[6], r[7]];
                    if rq.feed(&rr) {
                        done_q = true;
                        let l = rq.lane + 4;
                        for s in 0..NST {
                            let mut a = [0.0f32; 8];
                            _mm256_storeu_ps(a.as_mut_ptr(), ev[s]);
                            q.st[s] = a[l];
                        }
                    }
                }
                if done_p && done_q {
                    break;
                }
            }
            let fp = p.st[FF];
            p.commit(&c, fp, rp.g);
            let fq = q.st[FF];
            q.commit(&c, fq, rq.g);
            let (a, b) = p.output(&c);
            pl[i] = a;
            pr[i] = b;
            let (a, b) = q.output(&c);
            ql[i] = a;
            qr[i] = b;
            i += 1;
        }
    }
    p.res.down(&co, os, &pl, &pr, &mut iop.l[..ns], &mut iop.r[..ns]);
    q.res.down(&co, os, &ql, &qr, &mut ioq.l[..ns], &mut ioq.r[..ns]);
}

/// Two filters together when the CPU can, else one after the other: the same samples either way.
pub fn run_pair(p: &mut SolverFilter, q: &mut SolverFilter, x: &FilterCtx, iop: &mut FilterIo, ioq: &mut FilterIo, casc: bool, avx: bool) {
    if avx {
        unsafe { run_pair_avx(p, q, x, iop, ioq, casc) }
    } else {
        p.run(x, iop, casc);
        q.run(x, ioq, casc);
    }
}
