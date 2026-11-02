// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Triple VCO (`pages/osc-triple.md`).

use super::{insert_ramp, insert_step, OscCtx};
use crate::dsp::{chain_step, rcpn, Lcg, F4};
use crate::tables::Blep;

/// Hard-coded by the page: pink generator's rows.
const PINK_P: [f32; 5] = [0.91578, 0.23378, 0.06378, 0.01478, 0.00198];
const PINK_G: [f32; 5] = [3.4006, 3.087, 2.597, 2.9694, 3.8024];

/// One draw of the pink generator (shared with the other models).
#[inline]
pub fn pink_draw(rng: &mut Lcg, rows: &mut [f32; 5]) -> f32 {
    let r = rng.unit();
    for i in 0..5 {
        if r < PINK_P[i] {
            let u = rng.unit();
            rows[i] = (((u + u) - 1.0) * PINK_G[i]) * 0.1;
        }
    }
    (((rows[0] + rows[1]) + rows[2]) + rows[3]) + rows[4]
}

/// The breakpoints of a shape value (section 6).
#[inline]
pub fn breakpoints(s: f32) -> (f32, f32, f32) {
    let s = if s > 1.0 {
        1.0
    } else if s > 0.0 {
        s
    } else {
        0.0
    };
    if s < 0.25 {
        let x1 = 1.0 - 2.0 * s;
        let m0 = 2.0 / x1;
        (x1, m0, -m0)
    } else if s < 0.5 {
        let x1 = 1.0 - 2.0 * s;
        let m0 = 2.0 / (2.0 * s);
        (x1, m0, -m0)
    } else if s < 0.75 {
        (2.0 * s - 1.0, 0.0, (3.0 - 4.0 * s) * -2.0)
    } else {
        (2.0 * s - 1.0, 0.0, 0.0)
    }
}

/// Up by 4 on two parallel signals (section 11).
#[derive(Clone, Copy)]
pub struct Up4 {
    a1: [F4; 3],
    a2: [F4; 3],
}

/// Down by 4 on two parallel signals.
#[derive(Clone, Copy)]
pub struct Down4 {
    a1: [F4; 7],
    a2: [F4; 7],
}

pub struct ResCoefs {
    pub up: [F4; 2],
    pub down: [F4; 6],
}

#[inline(always)]
pub fn res_coefs() -> ResCoefs {
    // lanes: signal 1 branch A, signal 2 branch A, signal 1 branch B, signal 2 branch B
    let l = |a: f32, b: f32| F4::new(a, a, b, b);
    ResCoefs {
        up: [l(0.12073212116956711, 0.3903621733188629), l(0.6632020473480225, 0.8907868266105652)],
        down: [
            l(0.03668150305747986, 0.13654762506484985),
            l(0.2746317684650421, 0.4231386184692383),
            l(0.5610989928245544, 0.6775400638580322),
            l(0.7697418332099915, 0.839889645576477),
            l(0.8922607898712158, 0.9315419793128967),
            l(0.9620945453643799, 0.9878163933753967),
        ],
    }
}

impl Up4 {
    pub fn new() -> Up4 {
        Up4 { a1: [F4::zero(); 3], a2: [F4::zero(); 3] }
    }

    /// One host-rate pair to four pairs (`p`, `q`).
    #[inline(always)]
    pub fn step(&mut self, co: &ResCoefs, u: f32, v: f32, p: &mut [f32], q: &mut [f32]) {
        let w = chain_step(&mut self.a1, &co.up, F4::new(u, v, u, v)).arr();
        let a = chain_step(&mut self.a2, &co.up, F4::new(w[0], w[1], w[0], w[1])).arr();
        let b = chain_step(&mut self.a2, &co.up, F4::new(w[2], w[3], w[2], w[3])).arr();
        p[0] = a[0];
        q[0] = a[1];
        p[1] = a[2];
        q[1] = a[3];
        p[2] = b[0];
        q[2] = b[1];
        p[3] = b[2];
        q[3] = b[3];
    }
}

impl Down4 {
    pub fn new() -> Down4 {
        Down4 { a1: [F4::zero(); 7], a2: [F4::zero(); 7] }
    }

    /// Four pairs to one.
    #[inline(always)]
    pub fn step(&mut self, co: &ResCoefs, p: &[f32], q: &[f32]) -> (f32, f32) {
        let l = self.a1[6].arr();
        let y = chain_step(&mut self.a1, &co.down, F4::new(p[0], q[0], p[1], q[1])).arr();
        let g0 = (l[2] + y[0], l[3] + y[1]);
        let l = y;
        let y = chain_step(&mut self.a1, &co.down, F4::new(p[2], q[2], p[3], q[3])).arr();
        let g1 = (l[2] + y[0], l[3] + y[1]);
        let l = self.a2[6].arr();
        let y = chain_step(&mut self.a2, &co.down, F4::new(g0.0, g0.1, g1.0, g1.1)).arr();
        ((l[2] + y[0]) * 0.25, (l[3] + y[1]) * 0.25)
    }
}

#[derive(Clone, Copy, Default)]
struct Lane {
    ph: f32,
    val: f32,
    m: f32,
    x1: f32,
    m0: f32,
    m1: f32,
}

#[repr(align(64))]
pub struct Rings(pub [[f32; 256]; 3]);

#[repr(align(64))]
pub struct Rings2(pub [[f32; 256]; 2]);

#[repr(align(64))]
pub struct Rings4(pub [[f32; 256]; 4]);

pub struct Triple {
    o: [Lane; 3],
    rings: Rings,
    idx: u8,
    inc: [f32; 4],
    base: [f32; 4],
    shape: [f32; 3],
    vol: [f32; 4],
    fm: f32,
    pm: [f32; 4],
    x1dc: f32,
    y1dc: f32,
    rng: Lcg,
    pink: [f32; 5],
    color: bool,
    prev_mode: Option<(bool, bool, bool)>,
    up: Up4,
    down: Down4,
    last_time: i64,
    need_init: bool,
    woke: bool,
    /// The last shape's breakpoints (a shape holds still most of the time).
    bp_key: [f32; 3],
    bp: [(f32, f32, f32); 3],
}

/// Seed row for a voice: a, b before the four draws.
pub fn seed_phases(seed: (u32, u32)) -> ([f32; 3], Lcg) {
    let mut rng = Lcg::new(seed.0, seed.1);
    let _u1 = rng.unit();
    let u2 = rng.unit();
    let u3 = rng.unit();
    let u4 = rng.unit();
    ([u4, u3, u2], rng)
}

struct Targets {
    base: [f32; 4],
    shape: [f32; 3],
    vol: [f32; 4],
    fm: f32,
}

struct ModeC {
    x4: bool,
    k: usize,
    q: f32,
    ri: f32,
    cap: f32,
    rdc: f32,
    start_chunks: usize,
    step: &'static Blep,
    ramp: &'static Blep,
}

impl Triple {
    pub fn new(seed: (u32, u32)) -> Box<Triple> {
        let (ph, rng) = seed_phases(seed);
        let mut o = [Lane::default(); 3];
        for i in 0..3 {
            o[i].ph = ph[i];
        }
        Box::new(Triple {
            o,
            rings: Rings([[0.0; 256]; 3]),
            idx: 0,
            inc: [0.0; 4],
            base: [0.0; 4],
            shape: [0.0; 3],
            vol: [0.0; 4],
            fm: 0.0,
            pm: [0.0; 4],
            x1dc: 0.0,
            y1dc: 0.0,
            rng,
            pink: [0.0; 5],
            color: false,
            prev_mode: None,
            up: Up4::new(),
            down: Down4::new(),
            last_time: 0,
            need_init: true,
            woke: false,
            bp_key: [f32::NAN; 3],
            bp: [(0.0, 0.0, 0.0); 3],
        })
    }

    #[inline]
    pub fn wake(&mut self) {
        self.woke = true;
    }

    #[inline]
    pub fn force_init(&mut self) {
        self.need_init = true;
    }

    fn targets(&self, x: &OscCtx) -> Targets {
        let s = &x.s.osc;
        let v7 = x.voice & 7;
        let k = x.s.tune_slop * 0.01;
        let kk = k * k;
        let slop = |i: usize| (kk * x.s.trims[i][v7]) * 0.01;
        let octave = ((s.tune[0] * 0.083333336 + 0.5) as f64).floor() as f32;
        let base = [(octave * 12.0 + 76.0) + slop(0), (s.tune[1] + 76.0) + slop(1), (s.tune[2] + 76.0) + slop(2), 76.0];
        let ps = x.pw_slop * 0.06;
        let mut shape = [0.0; 3];
        for o in 0..3 {
            let t = ((s.shape[o] - 1.0) * 0.125 + 0.0) + ps;
            shape[o] = if t >= 0.75 { 0.9975 * t } else { t };
        }
        let cl = |x: f32| {
            let a = if x < 1.0 { x } else { 1.0 };
            if a > 0.0 {
                a
            } else {
                0.0
            }
        };
        let z = x.row(s.noise_src, 0) * s.noise_depth + s.noise_vol;
        let l = {
            let y = z * z * 0.0001;
            let a = if y < 2.0 { y } else { 2.0 };
            if a > 0.0 {
                a
            } else {
                0.0
            }
        };
        let vol = [cl(s.volume[0] * 0.01), cl(s.volume[1] * 0.01), cl(s.volume[2] * 0.01), cl((l * 100.0) * 0.01)];
        let fm = cl((x.row(s.fm_src, 0) * s.fm_depth + s.fm) * 0.01);
        Targets { base, shape, vol, fm }
    }

    fn mode_c(x: &OscCtx, x4: bool) -> ModeC {
        let t = x.tables;
        if x4 {
            ModeC { x4, k: 16, q: 0.0625, ri: rcpn(4.0), cap: 0.125, rdc: 1.0 - x.dt * 22.5, start_chunks: 2, step: &t.step4, ramp: &t.ramp4 }
        } else {
            ModeC { x4, k: 4, q: 0.25, ri: rcpn(1.0), cap: 0.5, rdc: 1.0 - x.dt * 90.0, start_chunks: 8, step: &t.step1, ramp: &t.ramp1 }
        }
    }

    /// Runs the call; `a`, `b` receive 16 n samples of the mix and of oscillator 1.
    pub fn run(&mut self, x: &OscCtx, a: &mut [f32], b: &mut [f32]) {
        let s = &x.s.osc;
        let x4 = s.tune_mod[0] != 0.0 || s.tune_mod[1] != 0.0 || s.tune_mod[2] != 0.0;
        let mode = (x4, s.sync[0], s.sync[1]);
        let mut init = self.need_init || self.prev_mode != Some(mode);
        self.need_init = false;
        self.prev_mode = Some(mode);
        if self.woke {
            if x.time - self.last_time > 10 {
                init = true;
            }
            self.woke = false;
        }
        self.last_time = x.time + x.n as i64 + x.c as i64;
        let mc = Self::mode_c(x, x4);
        self.body(x, &mc, x.n, init, a, b);
    }

    #[allow(clippy::too_many_arguments)]
    fn body(&mut self, x: &OscCtx, mc: &ModeC, n: usize, init: bool, a: &mut [f32], b: &mut [f32]) {
        let s = &x.s.osc;
        let t = self.targets(x);
        if init {
            self.base = t.base;
            self.shape = t.shape;
            self.vol = t.vol;
            self.fm = t.fm;
        }
        // the per-sample ramp F
        let mut f = 1.0 / (16.0 * n as f32);
        if mc.x4 {
            f = f / 4.0;
        }
        let d_fm = (t.fm - self.fm) * f;
        let mut d_shape = [0.0; 3];
        let mut d_vol = [0.0; 4];
        let mut d_base = [0.0; 4];
        for l in 0..4 {
            d_vol[l] = (t.vol[l] - self.vol[l]) * f;
            d_base[l] = (t.base[l] - self.base[l]) * f;
        }
        for o in 0..3 {
            d_shape[o] = (t.shape[o] - self.shape[o]) * f;
        }
        // step 4, drift
        let d = pink_draw(&mut self.rng, &mut self.pink);
        let e = s.drift * 0.01;
        let drift = (((e * e) * 0.1) * 0.25) * d;
        // step 5, control points
        let mut target = [[0.0f32; 4]; 16];
        let pts = 4 * n;
        let last_p = self.points(x, mc, &d_base, drift, pts, init, &mut target);
        // step 6, init
        if init {
            self.x1dc = 0.0;
            self.y1dc = 0.0;
            self.color = s.noise_color;
            if x.s.trans_m > 2 {
                for o in 0..3 {
                    self.o[o].ph = s.rst_ph[o] * 0.01;
                }
            }
            self.inc = target[0];
            for o in 0..3 {
                let (x1, m0, m1) = breakpoints(self.shape[o]);
                let l = &mut self.o[o];
                l.x1 = x1;
                l.m0 = m0;
                l.m1 = m1;
                if l.x1 > l.ph {
                    l.m = l.m0;
                    l.val = (l.ph - 0.0) * l.m0 + (-1.0);
                } else if 1.0 > l.ph {
                    l.m = l.m1;
                    l.val = (l.ph - l.x1) * l.m1 + 1.0;
                } else {
                    l.val = -1.0;
                    l.m = -1.0;
                }
                let dd = l.x1;
                let area = (((l.m0 * 0.5) as f64 * dd as f64 + (-1.0f64)) * dd as f64) as f32;
                self.y1dc = self.y1dc - self.vol[o] * area;
            }
            self.rings.0[0].fill(0.0);
            self.rings.0[1].fill(0.0);
            self.rings.0[2].fill(0.0);
            if mc.x4 {
                self.up = Up4::new();
                self.down = Down4::new();
            }
            // the start run, its output discarded
            let mut left = mc.start_chunks;
            let (mut sa, mut sb) = ([0.0f32; 64], [0.0f32; 64]);
            while left > 0 {
                let m = left.min(n);
                self.last_time = x.time + m as i64 + x.c as i64;
                self.body(x, mc, m, false, &mut sa, &mut sb);
                left -= m;
            }
        }
        self.samples(x, mc, n, &target, &d_base, &d_vol, &d_shape, d_fm, a, b);
        self.pm = last_p;
    }

    /// Step 5. Returns P(15).
    #[allow(clippy::too_many_arguments)]
    fn points(&self, x: &OscCtx, mc: &ModeC, d_base: &[f32; 4], drift: f32, pts: usize, init: bool, target: &mut [[f32; 4]; 16]) -> [f32; 4] {
        let s = &x.s.osc;
        let dbase = F4::load(d_base);
        let kf = F4::splat(mc.k as f32);
        let base = F4::load(&self.base);
        let dr = F4::splat(drift) + base;
        let tmv = F4::new(s.tune_mod[0] * s.tm1_depth, s.tune_mod[1] * s.tm1_depth, s.tune_mod[2] * s.tm1_depth, 0.0 * s.tm1_depth);
        let vib = F4::splat(s.vibrato * 0.01);
        let pm = F4::load(&self.pm);
        let inc = self.inc;
        let cap = F4::splat(mc.cap);
        let p_of = |j: usize| -> F4 {
            let mut y = (F4::splat(j as f32) * dbase) * kf + dr;
            y = y + F4::splat(x.row(s.tm1_src, j)) * tmv;
            if let Some(m) = x.mx_pitch {
                y = y + F4::splat(m[j]);
            }
            y = y + vib * F4::splat(x.row(16, j));
            let (k8, k9) = (x.row(8, j), x.row(9, j));
            F4::new(k8, k9, k9, k9) * F4::splat(64.0) + y
        };
        let mut last = F4::zero();
        for j in 0..pts {
            let p = p_of(j);
            if j == 15 {
                last = p;
            }
            let same = !init && unsafe {
                use core::arch::x86_64::*;
                _mm_movemask_ps(_mm_cmpeq_ps(p.0, pm.0)) == 15
            };
            let incs = if same {
                F4::load(&inc)
            } else {
                (((p - F4::splat(69.0)).div(F4::splat(12.0)).exp2() * F4::splat(110.0)) * F4::splat(x.dt)) * F4::splat(mc.ri)
            };
            target[j] = capped(incs, cap).arr();
        }
        if pts <= 15 {
            last = p_of(15);
        }
        last.arr()
    }

    #[allow(clippy::too_many_arguments)]
    fn samples(
        &mut self,
        x: &OscCtx,
        mc: &ModeC,
        n: usize,
        target: &[[f32; 4]; 16],
        d_base: &[f32; 4],
        d_vol: &[f32; 4],
        d_shape: &[f32; 3],
        d_fm: f32,
        a: &mut [f32],
        b: &mut [f32],
    ) {
        let s = &x.s.osc;
        let co = res_coefs();
        // step 7, noise at the host rate
        let ns = 16 * n;
        let (mut white, mut pink) = ([0.0f32; 256], [0.0f32; 256]);
        if self.color {
            for i in 0..ns {
                pink[i] = pink_draw(&mut self.rng, &mut self.pink);
            }
        } else {
            let bp = s.bp_noise + s.bp_noise;
            for w in white[..ns].iter_mut() {
                *w = (self.rng.unit() * 4.0) - bp;
            }
        }
        if mc.x4 {
            for i in (0..ns).rev() {
                // moved up in place from the top; states run forward below
                white[i * 4] = white[i];
                pink[i * 4] = pink[i];
            }
            for i in 0..ns {
                let (u, v) = (white[i * 4], pink[i * 4]);
                let (mut p, mut q) = ([0.0; 4], [0.0; 4]);
                self.up.step(&co, u, v, &mut p, &mut q);
                white[i * 4..i * 4 + 4].copy_from_slice(&p);
                pink[i * 4..i * 4 + 4].copy_from_slice(&q);
            }
        }
        let (mut out_y, mut out_w) = ([0.0f32; 256], [0.0f32; 256]);
        let k = mc.k;
        let sync = s.sync;
        let (sh_dep, sh_src) = (s.shape_depth * 0.01, s.shape_src);
        let mut pos = 0;
        for j in 0..4 * n {
            // shape targets
            let mut tb = [(0.0f32, 0.0f32, 0.0f32); 3];
            let sr = x.row(sh_src, j);
            for o in 0..3 {
                let mut l = (sr * (sh_dep * s.shape_mod[o]) + (k as f32 * d_shape[o])) + self.shape[o];
                if let Some(m) = x.mx_shape {
                    l = l + m[j];
                }
                if l.to_bits() != self.bp_key[o].to_bits() {
                    self.bp_key[o] = l;
                    self.bp[o] = breakpoints(l);
                }
                tb[o] = self.bp[o];
            }
            let q = mc.q;
            let mut di = [0.0f32; 4];
            for l in 0..4 {
                di[l] = (target[j][l] - self.inc[l]) * q;
            }
            let mut dx = [(0.0f32, 0.0f32, 0.0f32); 3];
            for o in 0..3 {
                dx[o] = ((tb[o].0 - self.o[o].x1) * q, (tb[o].1 - self.o[o].m0) * q, (tb[o].2 - self.o[o].m1) * q);
            }
            for _ in 0..k {
                for l in 0..4 {
                    self.base[l] += d_base[l];
                    self.vol[l] += d_vol[l];
                    self.inc[l] += di[l];
                }
                for o in 0..3 {
                    self.shape[o] += d_shape[o];
                    self.o[o].x1 += dx[o].0;
                    self.o[o].m0 += dx[o].1;
                    self.o[o].m1 += dx[o].2;
                }
                self.fm += d_fm;
                let (y, w) = self.one_sample(mc, sync, white[pos], pink[pos]);
                let yd = (y - self.x1dc) + mc.rdc * self.y1dc;
                self.y1dc = yd;
                self.x1dc = y;
                out_y[pos] = yd;
                out_w[pos] = w;
                pos += 1;
            }
        }
        if mc.x4 {
            for i in 0..ns {
                let (p, q) = self.down.step(&co, &out_y[i * 4..i * 4 + 4], &out_w[i * 4..i * 4 + 4]);
                a[i] = p;
                b[i] = q;
            }
        } else {
            a[..ns].copy_from_slice(&out_y[..ns]);
            b[..ns].copy_from_slice(&out_w[..ns]);
        }
    }

    #[inline(always)]
    fn one_sample(&mut self, mc: &ModeC, sync: [bool; 2], white: f32, pink: f32) -> (f32, f32) {
        let idx = self.idx;
        let ii = idx as usize;
        let [r1, r2, r3] = &mut self.rings.0;
        let mut acc;
        let [o1, o2, o3] = &mut self.o;
        // oscillator 1
        let sf = run_osc::<false>(o1, self.inc[0], 1.0, r1, idx, mc, -2.0);
        let w = r1[ii];
        r1[ii] = 0.0;
        acc = self.vol[0] * w;
        // FM factor
        let z = (w + w) * self.fm;
        let fmf = if self.fm == 0.0 {
            1.0
        } else {
            let a = z.abs();
            let s = ((((a * 0.0001540352968731895 + 0.0013333557872101665) * a + 0.009618128649890423) * a + 0.05550410971045494) * a + 0.24022650718688965) * a;
            let s = (s + 0.6931471824645996) * a + 1.0;
            if z >= 0.0 {
                s
            } else {
                rcpn(s)
            }
        };
        let cap = |t: f32| if t > 0.49 { 0.49 } else { t };
        // oscillator 2
        let f2 = cap(self.inc[1] * fmf);
        if sync[0] {
            run_osc::<true>(o2, f2, self.vol[1], r2, idx, mc, sf);
        } else {
            run_osc::<false>(o2, f2, self.vol[1], r2, idx, mc, -2.0);
        }
        acc = acc + r2[ii];
        r2[ii] = 0.0;
        // oscillator 3
        let f3 = cap(self.inc[2] * fmf);
        if sync[1] {
            run_osc::<true>(o3, f3, self.vol[2], r3, idx, mc, sf);
        } else {
            run_osc::<false>(o3, f3, self.vol[2], r3, idx, mc, -2.0);
        }
        acc = acc + r3[ii];
        r3[ii] = 0.0;
        // noise
        if self.vol[3] > 0.0 {
            if self.color {
                acc = self.vol[3] * pink + acc;
            } else {
                acc = acc + self.vol[3] * white;
            }
        }
        self.idx = idx.wrapping_add(1);
        (acc, w)
    }
}

/// C if inc > C, else inc.
#[inline(always)]
fn capped(inc: F4, cap: F4) -> F4 {
    inc.min(cap)
}

/// One oscillator for one sample (sections 7 and 10). Returns the sync fraction.
#[inline(always)]
fn run_osc<const SYNC: bool>(o: &mut Lane, f: f32, g: f32, ring: &mut [f32; 256], idx: u8, mc: &ModeC, sf: f32) -> f32 {
    let mut out_sf = -2.0f32;
    let mut p = o.ph;
    let mut e = p + f;
    let mut v = o.val;
    let mut d = o.m;
    let (x1, m0, m1) = (o.x1, o.m0, o.m1);
    let xs = [0.0f32, x1, 1.0];
    let ys = [-1.0f32, 1.0, -1.0];
    let ms = [m0, m1, m0];
    let (mut sp, mut rem) = (2.0f32, 2.0f32);
    if SYNC && sf >= 0.0 {
        rem = f * sf;
        sp = e - rem;
        if sp >= 1.0 {
            sp = sp - 1.0;
        }
    }
    let mut i = 0usize;
    while i < 3 {
        if SYNC && sp >= p && xs[i] > sp {
            let after = e - sp;
            v = v + (sp - p) * d;
            let frac = clamp01(after / f);
            if v != -1.0 {
                insert_step(ring, idx, mc.step, frac, (v - (-1.0)) * (g * 0.5));
                v = -1.0;
            }
            if d != m0 {
                insert_ramp(ring, idx, mc.ramp, frac, (m0 - d) * (g * f));
                d = m0;
            }
            i = 0;
            e = rem;
            p = 0.0;
            sp = 2.0;
        }
        let b = xs[i];
        if b >= p && e > b {
            v = v + (b - p) * d;
            let frac = clamp01((e - b) / f);
            if v != ys[i] {
                insert_step(ring, idx, mc.step, frac, (v - ys[i]) * (g * 0.5));
                v = ys[i];
            }
            if d != ms[i] {
                insert_ramp(ring, idx, mc.ramp, frac, (ms[i] - d) * (g * f));
                d = ms[i];
            }
            if b >= 1.0 {
                e = e - 1.0;
                p = 0.0;
                out_sf = e / f;
                i = 0;
                continue;
            }
            p = b;
        }
        i += 1;
    }
    // the end value
    let c = (e - p) * d + v;
    let (ex, em) = if x1 > e {
        ((e - 0.0) * m0 + (-1.0), m0)
    } else if 1.0 > e {
        ((e - x1) * m1 + 1.0, m1)
    } else {
        (c, d)
    };
    if (c - ex).abs() > 0.1 {
        insert_step(ring, idx, mc.step, 0.0, (v - ex) * (g * 0.5));
    }
    if (d - em).abs() > 0.1 {
        insert_ramp(ring, idx, mc.ramp, 0.0, (em - d) * (g * f));
    }
    o.val = ex;
    o.m = em;
    o.ph = e;
    let w = (idx as usize + 128) & 255;
    ring[w] += ex * g;
    out_sf
}

#[inline(always)]
fn clamp01(x: f32) -> f32 {
    let a = if x < 1.0 { x } else { 1.0 };
    if a > 0.0 {
        a
    } else {
        0.0
    }
}
