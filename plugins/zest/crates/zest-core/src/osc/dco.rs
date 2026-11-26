// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! DCO (`pages/osc-dco.md`): one master phase at 2x with a quantised increment; sub at the master rate, saw and pulse at twice
//! it. Each wave has its own ring; some shapes multiply a corrected wave by a gate table.

use super::triple::{pink_draw, res_coefs, seed_phases, Rings4};
use super::{apply_events_rings, clamp, clamp01, drift_of, ftrunc, level_of, lookup, pitch_points, tune_slop, wave_level, Down2, Events, OscCtx, PitchIn, Up2};
use crate::dsp::{exp2, rcpn, Lcg, F4};
use crate::params::id;
use crate::tables::{Blep, Wave};

const SUB: u8 = 0;
const SAW: u8 = 1;
const QR: u8 = 2;
const PUL: u8 = 3;

#[repr(align(64))]
struct Tv([[f32; 256]; 2]);

pub struct Dco {
    ph: [f32; 3],
    inc: f32,
    w: f32,
    v: f32,
    w_prev: f32,
    v_prev: f32,
    p: f32,
    q: f32,
    sub: f32,
    half: bool,
    level: usize,
    st_base: [f32; 4],
    st_width: f32,
    st_sub: f32,
    st_noise: f32,
    cache: [f32; 4],
    rings: Rings4,
    /// Table values, never cleared.
    tv: Tv,
    idx: u8,
    x1: f32,
    y1: f32,
    rng: Lcg,
    pink: [f32; 5],
    up: Up2,
    down: Down2,
    last_end: i64,
    need_init: bool,
    woke: bool,
}

/// `dcoinc` of the page.
#[inline]
fn dcoinc(p: f32, dt: f32, ri: f32) -> f32 {
    let x = (p - 76.0) - 36.0;
    let d: u32 = if x < -52.0 {
        3
    } else if x <= 8.0 {
        2
    } else {
        1
    };
    let t = ((12.0 * d as f32 + x) * 1.0009981 - 69.0) / 12.0;
    let a = exp2(t);
    let e = f32::from_bits(0x3AF1_A64B);
    let q = (((e * 110.0) / (a * 110.0)) * 115810.0) as i32;
    (((7611240u32 >> d) as f32 / q as f32) * dt) * ri
}

/// The settings a pass reads.
struct Set {
    ps: i32,
    ss: i32,
    subs: i32,
    saw_t: Option<&'static [f32]>,
    sub_t: Option<&'static [f32]>,
}

impl Set {
    fn read(x: &OscCtx) -> Set {
        let v = &x.s.raw;
        let ps = v[id::OSC_PULSESHAPE].round() as i32;
        let ss = v[id::OSC_SAWSHAPE].round() as i32;
        let subs = v[id::OSC_SUBOSCSHAPE].round() as i32;
        let saw_t = match ss {
            2 => Some(x.tables.wave(Wave::Pulse2)),
            4 => Some(x.tables.wave(Wave::Pulse8)),
            5 => Some(x.tables.wave(Wave::PairedPulse)),
            _ => None,
        };
        let sub_t = match subs {
            1 => Some(x.tables.wave(Wave::Pulse2)),
            2 => Some(x.tables.wave(Wave::Pulse4)),
            3 => Some(x.tables.wave(Wave::Pulse8)),
            5 => Some(x.tables.wave(Wave::Square)),
            _ => None,
        };
        Set { ps, ss, subs, saw_t, sub_t }
    }
}

impl Dco {
    /// `seed`: the voice's (a, b) of `data/osc-voice-seeds.tsv`.
    pub fn new(seed: (u32, u32)) -> Box<Dco> {
        let (ph, rng) = seed_phases(seed);
        Box::new(Dco {
            ph,
            inc: 0.0,
            w: 0.0,
            v: 0.0,
            w_prev: 0.0,
            v_prev: 0.0,
            p: -1.0,
            q: 0.0,
            sub: 1.0,
            half: false,
            level: 0,
            st_base: [0.0; 4],
            st_width: 0.0,
            st_sub: 0.0,
            st_noise: 0.0,
            cache: [0.0; 4],
            rings: Rings4([[0.0; 256]; 4]),
            tv: Tv([[0.0; 256]; 2]),
            idx: 0,
            x1: 0.0,
            y1: 0.0,
            rng,
            pink: [0.0; 5],
            up: Up2::new(),
            down: Down2::new(),
            last_end: 0,
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
        let mut init = self.need_init;
        self.need_init = false;
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
        let u = ((s.tune[0] as f64 * 0.083333333333333) as f32 + 0.5) as f64;
        let t_base = [(u.floor() as f32 * 12.0 + 76.0) + tune_slop(x, 0), (s.tune[1] + 76.0) + tune_slop(x, 1), (s.tune[2] + 76.0) + tune_slop(x, 2), 76.0];
        let t_width = s.pulse_width * 0.01 + x.pw_slop * 0.06;
        let t_sub = clamp01(s.volume[2] * 0.01);
        let nv = x.row(s.noise_src, 0) * s.noise_depth + s.noise_vol;
        let t_noise = clamp01((clamp(nv * nv * 0.0001, 0.0, 2.0) * 100.0) * 0.01);
        if init {
            self.st_base = t_base;
            self.st_width = t_width;
            self.st_sub = t_sub;
            self.st_noise = t_noise;
        }
        // 2, slow ramps
        let f = (1.0 / (16.0 * n as f32)) * 4.0 * 0.125;
        let mut d_base = [0.0f32; 4];
        for l in 0..4 {
            d_base[l] = (t_base[l] - self.st_base[l]) * f;
        }
        let d_width = (t_width - self.st_width) * f;
        let d_sub = (t_sub - self.st_sub) * f;
        let d_noise = (t_noise - self.st_noise) * f;
        // 3, drift
        let drift = drift_of(x, pink_draw(&mut self.rng, &mut self.pink));
        // 4, pitch points
        let ri = rcpn(2.0);
        let dt = x.dt;
        let pin = PitchIn { e: 8.0, d_base, st_base: self.st_base, drift, a: [s.tm1_depth; 4], b: [s.tm2_depth; 4] };
        let mut target = [[0.0f32; 4]; 16];
        let cur = [self.inc; 4];
        let law = |p: F4| F4::splat(dcoinc(p.arr()[0], dt, ri));
        let last_p = pitch_points(x, &pin, &self.cache, &cur, init, 0.25, 4 * n, law, &mut target);
        // 5, init
        if init {
            self.up = Up2::new();
            self.down = Down2::new();
            self.x1 = 0.0;
            self.y1 = 0.0;
            if x.s.trans_m > 2 {
                self.ph = [s.rst_ph[0] * 0.01, s.rst_ph[1] * 0.01, s.rst_ph[2] * 0.01];
            }
            self.inc = target[0][0];
            let (tw, tv, lv) = Self::wave_block(set, self.st_width, self.inc);
            self.w = tw;
            self.v = tv;
            self.level = lv;
            self.w_prev = self.w;
            self.v_prev = self.v;
            let pulse_term = match set.ps {
                2 => 0.5,
                3 => 2.0 * (self.w - 0.5),
                _ => 0.0,
            };
            let saw_term = match set.ss {
                2 => {
                    if lv < 9 {
                        0.375
                    } else {
                        0.0
                    }
                }
                3 => 0.25 * self.v * self.v + 0.75 * self.v,
                4 => {
                    if lv < 8 {
                        0.484375
                    } else {
                        0.0
                    }
                }
                5 => {
                    if lv < 8 {
                        0.671875
                    } else {
                        0.5
                    }
                }
                _ => 0.0,
            };
            let sub_term = match set.subs {
                1 | 2 | 3 | 5 => 0.5,
                _ => 0.0,
            };
            self.y1 = pulse_term + saw_term + self.st_sub * sub_term;
            self.sub = if self.ph[2] >= 0.5 { -1.0 } else { 1.0 };
            self.half = self.ph[1] >= 0.5;
            self.p = if ftrunc(2.0 * self.ph[0] + 0.5) >= self.w { 1.0 } else { -1.0 };
            self.q = if ftrunc(4.0 * self.ph[0]) >= self.v { 1.0 } else { 0.0 };
            for r in self.rings.0.iter_mut() {
                r.fill(0.0);
            }
            let mut left = 4;
            let (mut sa, mut sb) = ([0.0f32; 64], [0.0f32; 64]);
            while left > 0 {
                let m = left.min(n);
                self.last_end = x.time + x.c as i64 + m as i64;
                self.pass(x, set, m, false, &mut sa, &mut sb);
                left -= m;
            }
        }
        self.body(x, set, n, &target, &d_base, d_width, d_sub, d_noise, a, b);
        self.cache = last_p;
    }

    /// Width targets and table level.
    #[inline(always)]
    fn wave_block(set: &Set, l: f32, inc: f32) -> (f32, f32, usize) {
        let s = 0.5 * clamp((0.005 + l) + 1.0, 1.0, 1.98);
        let v = 2.0 * s - 1.0;
        let w = match set.ps {
            1 => 0.5,
            2 => 0.75,
            _ => s,
        };
        (w, v, wave_level(2.0 * inc))
    }

    #[allow(clippy::too_many_arguments)]
    fn body(
        &mut self,
        x: &OscCtx,
        set: &Set,
        n: usize,
        target: &[[f32; 4]; 16],
        d_base: &[f32; 4],
        d_width: f32,
        d_sub: f32,
        d_noise: f32,
        a_out: &mut [f32],
        b_out: &mut [f32],
    ) {
        let s = &x.s.osc;
        let co = res_coefs();
        let step: &'static Blep = &x.tables.step2;
        // 6, noise
        let ns = 16 * n;
        let mut noise = [0.0f32; 128];
        let bp = s.bp_noise + s.bp_noise;
        for i in 0..ns {
            let white = (self.rng.unit() * 4.0) - bp;
            let (p, q) = self.up.step(&co, white);
            noise[2 * i] = p;
            noise[2 * i + 1] = q;
        }
        let r_dc = 1.0 - 12.5 * x.dt;
        let (mut out_y, mut out_o) = ([0.0f32; 128], [0.0f32; 128]);
        let pw_d = s.pw_depth * 0.01;
        let slow = d_width != 0.0 || d_sub != 0.0 || d_noise != 0.0 || d_base.iter().any(|d| *d != 0.0);
        let (ps, ss, subs) = (set.ps, set.ss, set.subs);
        let mut pos = 0;
        let mut ev = Events::new();
        let mut xp = self.ph[0];
        let mut inc = self.inc;
        let (mut w, mut v, mut w_prev, mut v_prev) = (self.w, self.v, self.w_prev, self.v_prev);
        let (mut p, mut q, mut sub, mut half) = (self.p, self.q, self.sub, self.half);
        let (mut st_width, mut st_sub, mut st_noise) = (self.st_width, self.st_sub, self.st_noise);
        let (mut x1, mut y1) = (self.x1, self.y1);
        let mut idx = self.idx;
        for j in 0..4 * n {
            let mut l = (x.row(s.pw_src, j) * pw_d + 8.0 * d_width) + st_width;
            if let Some(m) = x.mx_shape {
                l = l + m[j];
            }
            let t_inc = target[j][0];
            let (t_w, t_v, lv) = Self::wave_block(set, l, t_inc);
            self.level = lv;
            let saw_t = set.saw_t.map(|t| level_of(t, lv));
            let sub_t = set.sub_t.map(|t| level_of(t, lv));
            let di = (t_inc - inc) * 0.125;
            let dw = (t_w - w) * 0.125;
            let dv = (t_v - v) * 0.125;
            for _ in 0..8 {
                if slow {
                    for l in 0..4 {
                        self.st_base[l] += d_base[l];
                    }
                    st_width += d_width;
                    st_sub += d_sub;
                    st_noise += d_noise;
                }
                inc += di;
                w += dw;
                v += dv;
                let (a, b, i) = (st_sub, st_noise, inc);
                ev.n = 0;
                // master phase and sub
                let old = xp;
                let mut xx = old + i;
                let mut fr = -1.0f32;
                let mut allowed = true;
                if xx >= 1.0 {
                    xx = ftrunc(xx);
                    if half {
                        half = false;
                        fr = xx / i;
                    }
                }
                if xx >= 0.5 {
                    if !half {
                        half = true;
                        fr = (xx - 0.5) / i;
                    }
                    if subs >= 4 {
                        allowed = false;
                    }
                } else if half && subs >= 4 {
                    allowed = false;
                }
                if allowed && fr >= 0.0 && a > 0.0 {
                    ev.push_on(SUB, fr, sub * a);
                    sub = if sub >= 0.0 { -1.0 } else { 1.0 };
                }
                xp = xx;
                // saw and pulse
                let u = ftrunc(2.0 * old + 0.5);
                let i2 = 2.0 * i;
                let mut y = u + i2;
                if y >= w && p < 0.0 {
                    p = 1.0;
                    if ps != 0 {
                        ev.push_on(PUL, (y - w) / ((w_prev - w) + i2), -1.0);
                    }
                }
                if y >= 1.0 {
                    y = y - 1.0;
                    if (ps != 0 || ss != 0) && p > 0.0 {
                        let g = y / i2;
                        ev.push_on(SAW, g, 1.0);
                        ev.push_on(PUL, g, 1.0);
                    }
                    p = -1.0;
                }
                // saw-pwm square
                if ss == 3 {
                    let i4 = 2.0 * i2;
                    let mut z = ftrunc(2.0 * u) + i4;
                    if z >= v && q == 0.0 {
                        ev.push_on(QR, (z - v) / ((v_prev - v) + i4), -0.5);
                        q = 1.0;
                    }
                    if z >= 1.0 {
                        z = z - 1.0;
                        if q > 0.0 {
                            ev.push_on(QR, z / i4, 0.5);
                        }
                        q = 0.0;
                    }
                    if z >= v && q == 0.0 {
                        ev.push_on(QR, (z - v) / ((v_prev - v) + i4), -0.5);
                        q = 1.0;
                    }
                }
                let rings = &mut self.rings.0;
                if ev.n > 0 {
                    apply_events_rings(rings, idx, step, &ev);
                }
                // naive values
                let ii = idx as usize;
                let wi = idx.wrapping_add(128) as usize;
                let [r_sub, r_saw, r_q, r_pul] = rings;
                if a > 0.0 {
                    r_sub[wi] += sub * a;
                    if let Some(t) = sub_t {
                        self.tv.0[0][wi] = lookup(t, ftrunc(xx + 1.0) * 2048.0);
                    }
                }
                if ps != 0 {
                    r_pul[wi] += p;
                }
                if ss != 0 {
                    if let Some(t) = saw_t {
                        self.tv.0[1][wi] = lookup(t, y * 2048.0);
                    }
                    r_saw[wi] += 2.0 * y - 1.0;
                    r_q[wi] += q;
                }
                // output
                let mut o = 0.0f32;
                if a > 0.0 {
                    let mut g = r_sub[ii] + 1.0;
                    if sub_t.is_some() {
                        g = g * self.tv.0[0][ii];
                    }
                    o = (g - 1.0) * a;
                }
                if ss != 0 {
                    let mut g = r_saw[ii] + 1.0;
                    if saw_t.is_some() {
                        g = g * self.tv.0[1][ii];
                    }
                    if ss == 3 {
                        g = g * r_q[ii];
                    }
                    o = o + (g - 1.0);
                }
                if ps != 0 {
                    o = o + r_pul[ii];
                }
                if b > 0.0 {
                    o = o + b * noise[pos];
                }
                r_sub[ii] = 0.0;
                r_saw[ii] = 0.0;
                r_q[ii] = 0.0;
                r_pul[ii] = 0.0;
                w_prev = w;
                v_prev = v;
                idx = idx.wrapping_add(1);
                // DC blocker
                let yd = (o - x1) + r_dc * y1;
                y1 = yd;
                x1 = o;
                out_y[pos] = yd;
                out_o[pos] = o;
                pos += 1;
            }
        }
        self.ph = [xp; 3];
        self.inc = inc;
        self.w = w;
        self.v = v;
        self.w_prev = w_prev;
        self.v_prev = v_prev;
        self.p = p;
        self.q = q;
        self.sub = sub;
        self.half = half;
        self.st_width = st_width;
        self.st_sub = st_sub;
        self.st_noise = st_noise;
        self.x1 = x1;
        self.y1 = y1;
        self.idx = idx;
        // 8, down by 2
        for i in 0..ns {
            let (p0, q0) = self.down.step(&co, out_y[2 * i], out_o[2 * i], out_y[2 * i + 1], out_o[2 * i + 1]);
            a_out[i] = p0;
            b_out[i] = q0;
        }

    }
}

#[cfg(test)]
mod tests {
    use super::dcoinc;
    use crate::dsp::rcpn;

    /// The pitch is quantised: nearby pitches share one increment, and the increment rises with the pitch.
    #[test]
    fn increments_step_with_the_count() {
        let (dt, ri) = (1.0 / 48000.0f32, rcpn(2.0));
        let mut distinct = 1;
        for k in 0..1200 {
            let p = 120.0 + k as f32 * 0.001;
            if dcoinc(p, dt, ri) != dcoinc(p + 0.001, dt, ri) {
                distinct += 1;
            }
        }
        assert!(distinct < 300, "{distinct}");
        let mut last = 0.0;
        for k in 30..100 {
            let v = dcoinc(k as f32 + 76.0 - 40.0, dt, ri);
            assert!(v >= last, "{k}: {v} after {last}");
            last = v;
        }
    }
}
