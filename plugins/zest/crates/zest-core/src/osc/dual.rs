// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Dual VCO (`pages/osc-dual.md`): two oscillators at 4x, each a weighted mix of naive waves with band-limited steps.

use super::triple::{pink_draw, res_coefs, seed_phases, Down4, Rings2, Up4};
use super::{apply_events, clamp01, drift_of, fmfactor, law_exp, level_of, lookup, pitch_points, tune_slop, wave_level, Events, OscCtx, PitchIn};
use crate::dsp::{rcpn, tclip, Lcg};
use crate::params::id;
use crate::tables::{Blep, Wave};

/// Weight indexes.
const TRI1: usize = 0;
const SAW1: usize = 1;
const PUL1: usize = 2;
const NOI1: usize = 3;
const SIN2: usize = 4;
const TRI2: usize = 5;
const SAW2: usize = 6;
const PUL2: usize = 7;

pub struct Dual {
    ph: [f32; 2],
    inc: [f32; 4],
    pw: [f32; 2],
    ppw: [f32; 2],
    sgn: [f32; 2],
    w: [f32; 8],
    level: [usize; 2],
    st_base: [f32; 4],
    st_width: f32,
    st_mix: f32,
    st_fm: f32,
    cache: [f32; 4],
    rings: Rings2,
    idx: u8,
    x1: f32,
    y1: f32,
    rng: Lcg,
    pink: [f32; 5],
    up: Up4,
    down: Down4,
    last_end: i64,
    prev_sync: Option<bool>,
    need_init: bool,
    woke: bool,
}

/// The settings a pass reads.
struct Set {
    sync: bool,
    model: i32,
    pwm2: bool,
    weights: [f32; 8],
    table: &'static [f32],
    a: [f32; 4],
    b: [f32; 4],
}

impl Set {
    fn read(x: &OscCtx) -> Set {
        let v = &x.s.raw;
        let s = &x.s.osc;
        let sw = |i: usize| if v[i] >= 0.5 { 1.0f32 } else { 0.0 };
        let (t1, sa1, p1, no1) = (sw(id::OSC_TRIANGLE1ON), sw(id::OSC_SAW1ON), sw(id::OSC_PWM1ON), sw(id::OSC_NOISE1ON));
        let (si2, t2, sa2, p2) = (sw(id::OSC_SINE2ON), sw(id::OSC_TRIANGLE2ON), sw(id::OSC_SAW2ON), sw(id::OSC_PULSE2ON));
        let n1 = no1 + p1 + sa1 + t1;
        let n2 = si2 + p2 + sa2 + t2;
        let r1 = if n1 > 0.0 { 1.0 / n1 } else { 0.0 };
        let r2 = if n2 > 0.0 { 1.0 / n2 } else { 0.0 };
        let model = v[id::OSC_SHAPEMODEL].round() as i32;
        let wave = if model == 2 { Wave::SawTriangle } else { Wave::Triangle };
        let (d1, d2) = (s.tm1_depth, s.tm2_depth);
        let (a, b) = match v[id::OSC_TUNEMODMODE].round() as i32 {
            0 => ([d1, 0.0, 0.0, 0.0], [d2, 0.0, 0.0, 0.0]),
            1 => ([d1, d1, 0.0, 0.0], [d2, d2, 0.0, 0.0]),
            2 => ([0.0, d1, 0.0, 0.0], [0.0, d2, 0.0, 0.0]),
            _ => ([d1, 0.0, 0.0, 0.0], [0.0, d2, 0.0, 0.0]),
        };
        Set {
            sync: s.sync[0],
            model,
            pwm2: v[id::OSC_PWM2ON] >= 0.5,
            weights: [t1 * r1, sa1 * r1, p1 * r1, no1 * r1, si2 * r2, t2 * r2, sa2 * r2, p2 * r2],
            table: x.tables.wave(wave),
            a,
            b,
        }
    }
}

/// `naive` of the page.
#[inline(always)]
fn naive(ph: f32, pv: f32, w_sin: f32, w_tri: f32, w_saw: f32, w_pul: f32, table: &[f32; 2048]) -> f32 {
    let mut acc = 0.0f32;
    if w_sin > 0.0 {
        let x = ph - 0.5;
        let y = (8.0 - 16.0 * x.abs()) * x;
        acc = ((y.abs() * y - y) * 0.225 + y) * w_sin;
    }
    if w_tri > 0.0 {
        acc = acc + lookup(table, ph * 2048.0) * w_tri;
    }
    ((2.0 * ph - 1.0) * w_saw + acc) + pv * w_pul
}

/// Steps 1 to 4 of an oscillator's walk: the new phase, the pulse value, the sync fraction.
#[inline(always)]
fn walk(ph: f32, i: f32, pw: f32, ppw: f32, sgn: &mut f32, w_pul: f32, w_sum: f32, ev: &mut Events) -> (f32, f32, f32) {
    let mut z = ph + i;
    let mut sf = -2.0f32;
    if pw <= z && *sgn < 0.0 {
        *sgn = 1.0;
        if w_pul > 0.0 {
            ev.push((z - pw) / ((ppw - pw) + i), -w_pul);
        }
    }
    let mut pv;
    if z < 1.0 {
        pv = *sgn;
    } else {
        z = z - 1.0;
        sf = z / i;
        if w_sum > 0.0 && *sgn > 0.0 {
            ev.push(z / i, w_sum);
        }
        pv = -1.0;
        *sgn = -1.0;
    }
    if pw <= z && pv < 0.0 {
        *sgn = 1.0;
        pv = 1.0;
        if w_pul > 0.0 {
            ev.push((z - pw) / ((ppw - pw) + i), -w_pul);
        }
    }
    (z, pv, sf)
}

impl Dual {
    /// `seed`: the voice's (a, b) of `data/osc-voice-seeds.tsv`.
    pub fn new(seed: (u32, u32)) -> Box<Dual> {
        let (ph, rng) = seed_phases(seed);
        Box::new(Dual {
            ph: [ph[0], ph[1]],
            inc: [0.0; 4],
            pw: [0.0; 2],
            ppw: [0.0; 2],
            sgn: [-1.0; 2],
            w: [0.0; 8],
            level: [0; 2],
            st_base: [0.0; 4],
            st_width: 0.0,
            st_mix: 0.0,
            st_fm: 0.0,
            cache: [0.0; 4],
            rings: Rings2([[0.0; 256]; 2]),
            idx: 0,
            x1: 0.0,
            y1: 0.0,
            rng,
            pink: [0.0; 5],
            up: Up4::new(),
            down: Down4::new(),
            last_end: 0,
            prev_sync: None,
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
        let mut init = self.need_init || self.prev_sync != Some(set.sync);
        self.need_init = false;
        self.prev_sync = Some(set.sync);
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
        // 1, targets
        let octave = ((s.tune[0] * 0.083333336 + 0.5) as f64).floor() as f32;
        let t_base = [(octave * 12.0 + 76.0) + tune_slop(x, 0), (s.tune[1] + 76.0) + tune_slop(x, 1), (s.tune[2] + 76.0) + tune_slop(x, 2), 76.0];
        let mut t_width = s.pulse_width * 0.01 + x.pw_slop * 0.06;
        if t_width >= 0.75 {
            t_width = 0.9975 * t_width;
        }
        let mix = x.s.raw[id::OSC_OSCMIX];
        let t_mix = clamp01((x.row(s.noise_src, 0) * s.noise_depth + mix) * 0.01);
        let t_fm = clamp01((x.row(s.fm_src, 0) * s.fm_depth + s.fm) * 0.01);
        if init {
            self.st_base = t_base;
            self.st_width = t_width;
            self.st_mix = t_mix;
            self.st_fm = t_fm;
        }
        // 2, slow ramps
        let f = (1.0 / (16.0 * n as f32)) * 4.0 * 0.0625;
        let mut d_base = [0.0f32; 4];
        for l in 0..4 {
            d_base[l] = (t_base[l] - self.st_base[l]) * f;
        }
        let d_width = (t_width - self.st_width) * f;
        let d_mix = (t_mix - self.st_mix) * f;
        let d_fm = (t_fm - self.st_fm) * f;
        // 3, drift
        let drift = drift_of(x, pink_draw(&mut self.rng, &mut self.pink));
        // 4, pitch points
        let ri = rcpn(4.0);
        let dt = x.dt;
        let pin = PitchIn { e: 16.0, d_base, st_base: self.st_base, drift, a: set.a, b: set.b };
        let mut target = [[0.0f32; 4]; 16];
        let last_p = pitch_points(x, &pin, &self.cache, &self.inc, init, 0.125, 4 * n, |p| law_exp(p, dt, ri), &mut target);
        // 5, init
        if init {
            self.up = Up4::new();
            self.down = Down4::new();
            self.x1 = 0.0;
            self.y1 = 0.0;
            if x.s.trans_m > 2 {
                self.ph = [s.rst_ph[0] * 0.01, s.rst_ph[1] * 0.01];
            }
            self.inc = target[0];
            let (pt, lv) = self.wave_block(set, self.st_width, &target[0]);
            self.pw = pt;
            self.level = lv;
            self.w = set.weights;
            self.ppw = self.pw;
            let m = self.st_mix;
            let g1 = 1.0 - (0.83 * m + 0.17) * m;
            let g2 = (1.83 - 0.83 * m) * m;
            self.y1 = 4.0 * g1 * self.w[PUL1] * (self.pw[0] - 0.5) + 4.0 * g2 * self.w[PUL2] * (self.pw[1] - 0.5);
            for o in 0..2 {
                self.sgn[o] = if self.ph[o] - self.pw[o] > 0.0 { 1.0 } else { -1.0 };
            }
            self.rings.0[0].fill(0.0);
            self.rings.0[1].fill(0.0);
            let mut left = 2;
            let (mut sa, mut sb) = ([0.0f32; 64], [0.0f32; 64]);
            while left > 0 {
                let m = left.min(n);
                self.last_end = x.time + x.c as i64 + m as i64;
                self.pass(x, set, m, false, &mut sa, &mut sb);
                left -= m;
            }
        }
        match (set.sync, set.model == 1) {
            (false, false) => self.body::<false, false>(x, set, n, &target, d_width, d_mix, d_fm, &d_base, a, b),
            (false, true) => self.body::<false, true>(x, set, n, &target, d_width, d_mix, d_fm, &d_base, a, b),
            (true, false) => self.body::<true, false>(x, set, n, &target, d_width, d_mix, d_fm, &d_base, a, b),
            (true, true) => self.body::<true, true>(x, set, n, &target, d_width, d_mix, d_fm, &d_base, a, b),
        }
        self.cache = last_p;
    }

    /// The wave block's width targets and table levels.
    #[inline(always)]
    fn wave_block(&self, set: &Set, l: f32, inc: &[f32; 4]) -> ([f32; 2], [usize; 2]) {
        let s = clamp01(l + 0.005);
        let pw2 = if set.pwm2 { s } else { 0.504 };
        let l1 = if set.weights[TRI1] > 0.0 { wave_level(2.0 * inc[0]) } else { 0 };
        let l2 = if set.weights[TRI2] > 0.0 { wave_level(2.0 * inc[1]) } else { 0 };
        ([s, pw2], [l1, l2])
    }

    #[allow(clippy::too_many_arguments)]
    fn body<const SYNC: bool, const SAT: bool>(
        &mut self,
        x: &OscCtx,
        set: &Set,
        n: usize,
        target: &[[f32; 4]; 16],
        d_width: f32,
        d_mix: f32,
        d_fm: f32,
        d_base: &[f32; 4],
        a: &mut [f32],
        b: &mut [f32],
    ) {
        let s = &x.s.osc;
        let co = res_coefs();
        let step: &'static Blep = &x.tables.step4;
        // 6, noise
        let ns = 16 * n;
        let mut noise = [0.0f32; 256];
        let bp = s.bp_noise + s.bp_noise;
        for i in 0..ns {
            let u = self.rng.unit();
            let white = (u * 4.0) - bp;
            let (mut p, mut q) = ([0.0f32; 4], [0.0f32; 4]);
            self.up.step(&co, white, 0.0, &mut p, &mut q);
            noise[i * 4..i * 4 + 4].copy_from_slice(&p);
        }
        let r_dc = 1.0 - x.dt * 2.875;
        let (mut out_y, mut out_s) = ([0.0f32; 256], [0.0f32; 256]);
        let pw_d = s.pw_depth * 0.01;
        let slow = d_width != 0.0 || d_mix != 0.0 || d_fm != 0.0 || d_base.iter().any(|d| *d != 0.0);
        let (mut g1, mut g2) = mix_gains(self.st_mix);
        let mut pos = 0;
        // local copies: the ring writes below cannot alias these
        let mut ph = self.ph;
        let mut inc = self.inc;
        let mut pw = self.pw;
        let mut ppw = self.ppw;
        let mut sgn = self.sgn;
        let mut w = self.w;
        let (mut st_width, mut st_mix, mut st_fm) = (self.st_width, self.st_mix, self.st_fm);
        let (mut x1, mut y1) = (self.x1, self.y1);
        let mut idx = self.idx;
        let [r1, r2] = &mut self.rings.0;
        let mut ev = Events::new();
        for j in 0..4 * n {
            // 7, control step
            let mut l = (x.row(s.pw_src, j) * pw_d + 16.0 * d_width) + st_width;
            if let Some(m) = x.mx_shape {
                l = l + m[j];
            }
            let sv = clamp01(l + 0.005);
            let t_pw = [sv, if set.pwm2 { sv } else { 0.504 }];
            let t = &target[j];
            if set.weights[TRI1] > 0.0 {
                self.level[0] = wave_level(2.0 * t[0]);
            } else {
                self.level[0] = 0;
            }
            if set.weights[TRI2] > 0.0 {
                self.level[1] = wave_level(2.0 * t[1]);
            } else {
                self.level[1] = 0;
            }
            let tab1 = level_of(set.table, self.level[0]);
            let tab2 = level_of(set.table, self.level[1]);
            let di = [(t[0] - inc[0]) * 0.0625, (t[1] - inc[1]) * 0.0625];
            let dpw = [(t_pw[0] - pw[0]) * 0.0625, (t_pw[1] - pw[1]) * 0.0625];
            let mut dw = [0.0f32; 8];
            let mut wmove = false;
            for k in 0..8 {
                dw[k] = (set.weights[k] - w[k]) * 0.0625;
                wmove |= dw[k] != 0.0;
            }
            for _ in 0..16 {
                if slow {
                    for l in 0..4 {
                        self.st_base[l] += d_base[l];
                    }
                    st_width += d_width;
                    st_mix += d_mix;
                    st_fm += d_fm;
                    let gg = mix_gains(st_mix);
                    g1 = gg.0;
                    g2 = gg.1;
                }
                inc[0] += di[0];
                inc[1] += di[1];
                pw[0] += dpw[0];
                pw[1] += dpw[1];
                if wmove {
                    for k in 0..8 {
                        w[k] += dw[k];
                    }
                }
                let ii = idx as usize;
                let wi = idx.wrapping_add(128) as usize;
                // oscillator 1
                ev.n = 0;
                let (z, pv, sf) = walk(ph[0], inc[0], pw[0], ppw[0], &mut sgn[0], w[PUL1], w[PUL1] + w[SAW1], &mut ev);
                let mut v = naive(z, pv, 0.0, w[TRI1], w[SAW1], w[PUL1], tab1);
                if w[NOI1] > 0.0 {
                    v = v + w[NOI1] * noise[pos];
                }
                r1[wi] += v;
                if ev.n > 0 {
                    apply_events(r1, idx, step, &ev);
                }
                ph[0] = z;
                let o1 = r1[ii];
                // FM
                let i2 = {
                    let t = if st_fm == 0.0 { inc[1] } else { fmfactor((o1 * 4.0) * st_fm) * inc[1] };
                    if t > 0.125 {
                        0.125
                    } else {
                        t
                    }
                };
                // oscillator 2
                ev.n = 0;
                let v = if SYNC {
                    let mut sfx = sf;
                    let mut synced = sf > 0.0;
                    let mut z = if synced { i2 - i2 * sf } else { i2 };
                    z = z + ph[1];
                    let mut did = false;
                    let mut cur = sgn[1];
                    let (mut ph_s, mut pv_s) = (ph[1], cur);
                    let (e, pv) = loop {
                        if pw[1] <= z && cur < 0.0 {
                            sgn[1] = 1.0;
                            cur = 1.0;
                            if w[PUL2] > 0.0 {
                                ev.push((z - pw[1]) / ((ppw[1] - pw[1]) + i2), -w[PUL2]);
                            }
                        }
                        let mut e = z;
                        if z >= 1.0 {
                            e = z - 1.0;
                            if sfx < 0.0 {
                                sfx = e / i2;
                            }
                            if w[PUL2] + w[SAW2] > 0.0 && cur > 0.0 {
                                ev.push(e / i2, w[PUL2] + w[SAW2]);
                            }
                            sgn[1] = -1.0;
                            cur = -1.0;
                        }
                        let mut pv = cur;
                        if pw[1] <= e && cur < 0.0 {
                            sgn[1] = 1.0;
                            pv = 1.0;
                            if w[PUL2] > 0.0 {
                                ev.push((e - pw[1]) / ((ppw[1] - pw[1]) + i2), -w[PUL2]);
                            }
                        }
                        if synced {
                            sgn[1] = -1.0;
                            z = i2 * sfx;
                            synced = false;
                            did = true;
                            ph_s = e;
                            pv_s = pv;
                            cur = -1.0;
                            continue;
                        }
                        break (e, pv);
                    };
                    let v = naive(e, pv, w[SIN2], w[TRI2], w[SAW2], w[PUL2], tab2);
                    if did {
                        ev.push(sfx, naive(ph_s, pv_s, w[SIN2], w[TRI2], w[SAW2], w[PUL2], tab2) - v);
                    }
                    ph[1] = e;
                    v
                } else {
                    let (z, pv, _) = walk(ph[1], i2, pw[1], ppw[1], &mut sgn[1], w[PUL2], w[PUL2] + w[SAW2], &mut ev);
                    ph[1] = z;
                    naive(z, pv, w[SIN2], w[TRI2], w[SAW2], w[PUL2], tab2)
                };
                // mix
                r2[wi] += v;
                if ev.n > 0 {
                    apply_events(r2, idx, step, &ev);
                }
                let o2 = r2[ii];
                let a1 = 2.0 * o1;
                let a2 = 2.0 * o2;
                let y = if SAT {
                    (tclip(0.9 * a1) * g1 + tclip(0.9 * a2) * g2) * 1.39607
                } else {
                    g2 * a2 + g1 * a1
                };
                let s1 = a1 + 1.0;
                r1[ii] = 0.0;
                r2[ii] = 0.0;
                idx = idx.wrapping_add(1);
                ppw = pw;
                // DC blocker
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
        self.pw = pw;
        self.ppw = ppw;
        self.sgn = sgn;
        self.w = w;
        self.st_width = st_width;
        self.st_mix = st_mix;
        self.st_fm = st_fm;
        self.x1 = x1;
        self.y1 = y1;
        self.idx = idx;
        // 8, down by 4
        for i in 0..ns {
            let (p, q) = self.down.step(&co, &out_y[i * 4..i * 4 + 4], &out_s[i * 4..i * 4 + 4]);
            a[i] = p;
            b[i] = q;
        }
    }
}

#[inline(always)]
fn mix_gains(m: f32) -> (f32, f32) {
    (1.0 - (0.83 * m + 0.17) * m, (1.83 - 0.83 * m) * m)
}
