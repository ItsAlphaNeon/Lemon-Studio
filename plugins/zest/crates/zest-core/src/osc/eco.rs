// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Dual VCO Eco (`pages/osc-eco.md`): two oscillators of one wave each, steps corrected at once, at 1x or 2x by accuracy.

use super::triple::{pink_draw, res_coefs, seed_phases, Rings};
use super::{clamp, clamp01, drift_of, insert_step, law_exp, level_of, lookup, pitch_points, tune_slop, wave_level, Down2, OscCtx, PitchIn, Up2};
use crate::dsp::{rcpn, Lcg, F4};
use crate::params::id;
use crate::tables::{Blep, Wave};

pub struct Eco {
    ph: [f32; 3],
    inc: [f32; 4],
    width: [f32; 3],
    w1: f32,
    w2: f32,
    p: [f32; 2],
    p3: f32,
    st_base: [f32; 4],
    st_shape: [f32; 4],
    st_v1: f32,
    st_v2: f32,
    level: usize,
    cache: [f32; 4],
    rings: Rings,
    idx: u8,
    x1: f32,
    y1: f32,
    rng: Lcg,
    pink: [f32; 5],
    up: Up2,
    down: Down2,
    last_end: i64,
    prev_mode: Option<(bool, bool)>,
    need_init: bool,
    woke: bool,
}

/// The settings a pass reads.
#[derive(Clone, Copy)]
struct Set {
    x2: bool,
    third: bool,
    wave1: f32,
    wave2: f32,
}

impl Set {
    fn read(x: &OscCtx) -> Set {
        let v = &x.s.raw;
        let third = v[id::OSC_RINGMODPULSE] >= 0.5;
        let x2 = if third { x.acc != 1 } else { x.acc >= 2 };
        Set { x2, third, wave1: v[id::OSC_ECOWAVE1], wave2: v[id::OSC_ECOWAVE2] }
    }

    #[inline(always)]
    fn w1_target(&self) -> f32 {
        ((self.wave1 + 0.5) as f64).floor() as f32 - 1.0
    }

    #[inline(always)]
    fn w2_target(&self) -> f32 {
        if self.wave2 <= 1.5 {
            1.0
        } else {
            2.0
        }
    }

    #[inline(always)]
    fn width2(&self) -> f32 {
        if ((self.wave2 + 0.5) as f64).floor() == 3.0 {
            0.905
        } else {
            0.498
        }
    }
}

/// `b_l` of the wave block.
#[inline(always)]
fn lane_width(l: f32) -> f32 {
    clamp(0.505 * l + 0.495, 0.495, 1.0)
}

/// Walk of one oscillator; the value it adds at the write position.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn walk(h: &mut f32, i: f32, w: f32, pp: &mut f32, c: f32, ring: &mut [f32; 256], idx: u8, step: &Blep, table: &[f32; 2048], noise: f32) {
    let mut z = *h + i;
    if z >= w && *pp < 0.0 {
        *pp = 1.0;
        if c == 2.0 {
            insert_step(ring, idx, step, clamp01((z - w) / i), -1.0);
        }
    }
    if z >= 1.0 {
        z = z - 1.0;
        let k = (if c == 1.0 { 1.0f32 } else { 0.0 }) + (if c == 2.0 { 1.0 } else { 0.0 });
        if k != 0.0 && *pp > 0.0 {
            insert_step(ring, idx, step, clamp01(z / i), k);
        }
        *pp = -1.0;
    }
    let v = if c == 0.0 {
        -lookup(table, z * 2048.0)
    } else if c == 1.0 {
        2.0 * z - 1.0
    } else if c == 2.0 {
        *pp
    } else {
        noise
    };
    ring[idx.wrapping_add(128) as usize] += v;
    *h = z;
}

impl Eco {
    /// `seed`: the voice's (a, b) of `data/osc-voice-seeds.tsv`.
    pub fn new(seed: (u32, u32)) -> Box<Eco> {
        let (ph, rng) = seed_phases(seed);
        Box::new(Eco {
            ph,
            inc: [0.0; 4],
            width: [0.0; 3],
            w1: 0.0,
            w2: 0.0,
            p: [-1.0; 2],
            p3: 0.0,
            st_base: [0.0; 4],
            st_shape: [0.0; 4],
            st_v1: 0.0,
            st_v2: 0.0,
            level: 0,
            cache: [0.0; 4],
            rings: Rings([[0.0; 256]; 3]),
            idx: 0,
            x1: 0.0,
            y1: 0.0,
            rng,
            pink: [0.0; 5],
            up: Up2::new(),
            down: Down2::new(),
            last_end: 0,
            prev_mode: None,
            need_init: true,
            woke: false,
        })
    }

    /// The voice wakes from idle (`voice.md`, note start, step 8).
    pub fn wake(&mut self) {
        self.woke = true;
    }

    /// A forced re-init (a replaced mono-type note, `voice.md`).
    pub fn force_init(&mut self) {
        self.need_init = true;
    }

    /// One call: `a`, `b` take 16 n samples of the mix and of the FM stream.
    pub fn run(&mut self, x: &OscCtx, a: &mut [f32], b: &mut [f32]) {
        let set = Set::read(x);
        let mode = (set.x2, set.third);
        let mut init = self.need_init || self.prev_mode != Some(mode);
        self.need_init = false;
        self.prev_mode = Some(mode);
        if self.woke {
            if x.time - self.last_end > 10 {
                init = true;
            }
            self.woke = false;
        }
        self.last_end = x.time + x.c as i64 + x.n as i64;
        self.pass(x, &set, x.n, init, a, b);
    }

    fn pass(&mut self, x: &OscCtx, set: &Set, n: usize, init: bool, a: &mut [f32], b: &mut [f32]) {
        let s = &x.s.osc;
        let (per_step, q_step, cap) = if set.x2 { (8usize, 0.125f32, 0.25f32) } else { (4usize, 0.25f32, 0.5f32) };
        let e = per_step as f32;
        // 1, targets
        let u = ((s.tune[0] as f64 * 0.083333333333333) as f32 + 0.5) as f64;
        let t_base = [(u.floor() as f32 * 12.0 + 76.0) + tune_slop(x, 0), (s.tune[1] + 76.0) + tune_slop(x, 1), (s.tune[2] + 76.0) + tune_slop(x, 2), 76.0];
        let ps = x.pw_slop * 0.06;
        let t_shape = [s.pulse_width * 0.01 + ps, (s.shape[0] - 1.0) * 0.125 + ps, (s.shape[1] - 1.0) * 0.125 + ps, (s.shape[2] - 1.0) * 0.125 + ps];
        let t_v1 = clamp01((x.row(s.noise_src, 0) * s.noise_depth + s.volume[0]) * 0.01);
        let t_v2 = clamp01(s.volume[1] * 0.01);
        if init {
            self.st_base = t_base;
            self.st_shape = t_shape;
            self.st_v1 = t_v1;
            self.st_v2 = t_v2;
        }
        // 2, slow ramps
        let f = (1.0 / (16.0 * n as f32)) * 4.0 * q_step;
        let (mut d_base, mut d_shape) = ([0.0f32; 4], [0.0f32; 4]);
        for l in 0..4 {
            d_base[l] = (t_base[l] - self.st_base[l]) * f;
            d_shape[l] = (t_shape[l] - self.st_shape[l]) * f;
        }
        let d_v1 = (t_v1 - self.st_v1) * f;
        let d_v2 = (t_v2 - self.st_v2) * f;
        // 3, drift
        let drift = drift_of(x, pink_draw(&mut self.rng, &mut self.pink));
        // 4, pitch points
        let ri = rcpn(if set.x2 { 2.0 } else { 1.0 });
        let dt = x.dt;
        let pin = PitchIn { e, d_base, st_base: self.st_base, drift, a: [s.tm1_depth; 4], b: [s.tm2_depth; 4] };
        let mut target = [[0.0f32; 4]; 16];
        let mask = F4::new(f32::from_bits(!0), f32::from_bits(!0), f32::from_bits(!0), 0.0);
        let last_p = pitch_points(x, &pin, &self.cache, &self.inc, init, cap, 4 * n, |p| law_exp(p, dt, ri) & mask, &mut target);
        // 5, init
        if init {
            self.up = Up2::new();
            self.down = Down2::new();
            self.x1 = 0.0;
            self.y1 = 0.0;
            if x.s.trans_m > 2 {
                self.ph = [s.rst_ph[0] * 0.01, s.rst_ph[1] * 0.01, s.rst_ph[2] * 0.01];
            }
            self.inc = target[0];
            self.width = [lane_width(self.st_shape[0]), lane_width(self.st_shape[1]), set.width2()];
            self.w1 = set.w1_target();
            self.w2 = set.w2_target();
            if self.w1 == 0.0 {
                self.level = wave_level(self.inc[0]);
            }
            self.y1 = if self.w1 == 2.0 { (-4.0f64 * (self.width[0] as f64 - 0.5) * self.st_v1 as f64) as f32 } else { 0.0 };
            self.p[0] = if self.ph[0] - self.width[0] > 0.0 { 1.0 } else { -1.0 };
            self.p[1] = if self.ph[2] - self.width[2] > 0.0 { 1.0 } else { -1.0 };
            for r in self.rings.0.iter_mut() {
                r.fill(0.0);
            }
            let mut left = if set.x2 { 4 } else { 8 };
            let (mut sa, mut sb) = ([0.0f32; 64], [0.0f32; 64]);
            while left > 0 {
                let m = left.min(n);
                self.last_end = x.time + x.c as i64 + m as i64;
                self.pass(x, set, m, false, &mut sa, &mut sb);
                left -= m;
            }
        }
        let rm = set.wave2 >= 3.5;
        match (set.x2, rm, set.third) {
            (false, false, _) => self.body::<1, false, false>(x, set, n, &target, &d_base, &d_shape, d_v1, d_v2, a, b),
            (false, true, false) => self.body::<1, true, false>(x, set, n, &target, &d_base, &d_shape, d_v1, d_v2, a, b),
            (false, true, true) => self.body::<1, true, true>(x, set, n, &target, &d_base, &d_shape, d_v1, d_v2, a, b),
            (true, false, _) => self.body::<2, false, false>(x, set, n, &target, &d_base, &d_shape, d_v1, d_v2, a, b),
            (true, true, false) => self.body::<2, true, false>(x, set, n, &target, &d_base, &d_shape, d_v1, d_v2, a, b),
            (true, true, true) => self.body::<2, true, true>(x, set, n, &target, &d_base, &d_shape, d_v1, d_v2, a, b),
        }
        self.cache = last_p;
    }

    #[allow(clippy::too_many_arguments)]
    fn body<const K: usize, const RM: bool, const THIRD: bool>(
        &mut self,
        x: &OscCtx,
        set: &Set,
        n: usize,
        target: &[[f32; 4]; 16],
        d_base: &[f32; 4],
        d_shape: &[f32; 4],
        d_v1: f32,
        d_v2: f32,
        a_out: &mut [f32],
        b_out: &mut [f32],
    ) {
        let s = &x.s.osc;
        let co = res_coefs();
        let (per_step, q, step): (usize, f32, &'static Blep) =
            if K == 2 { (8, 0.125, &x.tables.step2) } else { (4, 0.25, &x.tables.step1) };
        let e = per_step as f32;
        // 6, noise
        let ns = 16 * n;
        let mut noise = [0.0f32; 128];
        let bp = s.bp_noise + s.bp_noise;
        for i in 0..ns {
            let white = (self.rng.unit() * 4.0) - bp;
            if K == 2 {
                let (p, r) = self.up.step(&co, white);
                noise[2 * i] = p;
                noise[2 * i + 1] = r;
            } else {
                noise[i] = white;
            }
        }
        let r_dc = if K == 2 { 1.0 - 5.75 * x.dt } else { 1.0 - 11.5 * x.dt };
        let (mut out_y, mut out_s) = ([0.0f32; 128], [0.0f32; 128]);
        let slow = d_v1 != 0.0 || d_v2 != 0.0 || d_base.iter().any(|d| *d != 0.0) || d_shape.iter().any(|d| *d != 0.0);
        let (t_w1, t_w2, t_w2d) = (set.w1_target(), set.w2_target(), set.width2());
        let table_all = x.tables.wave(Wave::SoftTriangle);
        let mut pos = 0;
        let mut ph = self.ph;
        let mut inc = self.inc;
        let mut width = self.width;
        let (mut w1, mut w2) = (self.w1, self.w2);
        let (mut p0, mut p1, mut p3) = (self.p[0], self.p[1], self.p3);
        let mut st_shape = self.st_shape;
        let (mut st_v1, mut st_v2) = (self.st_v1, self.st_v2);
        let (mut x1, mut y1) = (self.x1, self.y1);
        let mut idx = self.idx;
        let [r1, r2, r3] = &mut self.rings.0;
        for j in 0..4 * n {
            let l0 = e * d_shape[0] + st_shape[0];
            let l1 = e * d_shape[1] + st_shape[1];
            let t_width = [lane_width(l0), lane_width(l1), t_w2d];
            let t = &target[j];
            if t_w1 == 0.0 {
                self.level = wave_level(t[0]);
            }
            let table = level_of(table_all, self.level);
            let di = [(t[0] - inc[0]) * q, (t[1] - inc[1]) * q];
            let dw = [(t_width[0] - width[0]) * q, (t_width[1] - width[1]) * q, (t_width[2] - width[2]) * q];
            let dw1 = (t_w1 - w1) * q;
            let dw2 = (t_w2 - w2) * q;
            for _ in 0..per_step {
                if slow {
                    for l in 0..4 {
                        self.st_base[l] += d_base[l];
                        st_shape[l] += d_shape[l];
                    }
                    st_v1 += d_v1;
                    st_v2 += d_v2;
                }
                inc[0] += di[0];
                inc[1] += di[1];
                width[0] += dw[0];
                width[1] += dw[1];
                width[2] += dw[2];
                w1 += dw1;
                w2 += dw2;
                let ii = idx as usize;
                let wi = idx.wrapping_add(128) as usize;
                let n_val = noise[pos];
                let mut r = ph[0] + inc[0];
                walk(&mut ph[0], inc[0], width[0], &mut p0, w1, r1, idx, step, table, n_val);
                let o1 = r1[ii];
                walk(&mut ph[1], inc[1], width[2], &mut p1, w2, r2, idx, step, table, n_val);
                let a1 = 2.0 * o1;
                let s1 = a1 + 1.0;
                let q2 = 2.0 * r2[ii];
                let y = if !RM {
                    -(st_v2 * q2 + st_v1 * a1)
                } else if !THIRD {
                    -((s1 * st_v2) * (q2 * 0.5 - 1.0) + st_v1 * a1)
                } else {
                    if r >= width[1] && p3 < 0.0 {
                        insert_step(r3, idx, step, clamp01((r - width[1]) / inc[0]), -1.0);
                        p3 = 1.0;
                    }
                    if r >= 1.0 {
                        r = r - 1.0;
                        if p3 > 0.0 {
                            insert_step(r3, idx, step, clamp01(r / inc[0]), 1.0);
                        }
                        p3 = -1.0;
                    }
                    r3[wi] += p3;
                    let v3 = 2.0 * r3[ii];
                    r3[ii] = 0.0;
                    -(((v3 + 1.0) * st_v2) * (q2 * 0.5 - 1.0) + st_v1 * a1)
                };
                r1[ii] = 0.0;
                r2[ii] = 0.0;
                idx = idx.wrapping_add(1);
                let yd = (y - x1) + r_dc * y1;
                y1 = yd;
                x1 = y;
                out_y[pos] = yd;
                out_s[pos] = s1;
                pos += 1;
            }
        }
        self.ph = ph;
        self.inc = inc;
        self.width = width;
        self.w1 = w1;
        self.w2 = w2;
        self.p = [p0, p1];
        self.p3 = p3;
        self.st_shape = st_shape;
        self.st_v1 = st_v1;
        self.st_v2 = st_v2;
        self.x1 = x1;
        self.y1 = y1;
        self.idx = idx;
        if K == 2 {
            for i in 0..ns {
                let (p, r) = self.down.step(&co, out_y[2 * i], out_s[2 * i], out_y[2 * i + 1], out_s[2 * i + 1]);
                a_out[i] = p;
                b_out[i] = r;
            }
        } else {
            a_out[..ns].copy_from_slice(&out_y[..ns]);
            b_out[..ns].copy_from_slice(&out_s[..ns]);
        }
    }
}
