// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Digital (`pages/osc-digital.md`): two oscillators of seven types, each one sample from its phase, its two shape values and,
//! by type, a table, a filter or a delay line.

use super::triple::{pink_draw, res_coefs, seed_phases, Rings2};
use super::{clamp01, drift_of, fmfactor, insert_step, law_exp, lookup, pitch_points, tune_slop, Down2, OscCtx, PitchIn, Up2};
use crate::dsp::{exp2, rcpn, Lcg, F4};
use crate::params::id;
use crate::tables::{Blep, BL_LEN};

const DELAY: usize = 384000;

const MULTISAW: i32 = 0;
const TRIWRAP: i32 = 1;
const NOISE: i32 = 2;
const FEEDBACK: i32 = 3;
const PULSE: i32 = 4;
const SAW: i32 = 5;

/// The filter's coefficients, kept while their inputs hold still.
#[derive(Clone, Copy)]
struct Coef {
    f: u32,
    q: u32,
    k: f32,
    c1: f32,
    c2: f32,
    c3: f32,
    c4: f32,
}

impl Coef {
    #[inline(always)]
    fn update(&mut self, f: f32, q: f32) {
        if f.to_bits() == self.f && q.to_bits() == self.q {
            return;
        }
        self.f = f.to_bits();
        self.q = q.to_bits();
        let k = (2.0f64 - 2.0f64 * q as f64) as f32;
        let b = ((f as f64 * 1.5707964f32 as f64).tan()) as f32;
        let a = k + b;
        let r = rcpn(a * b + 1.0);
        let c1 = b * r;
        self.k = k;
        self.c1 = c1;
        self.c2 = b * c1;
        self.c3 = (a * -2.0) * c1;
        self.c4 = c1 + c1;
    }
}

/// A filter's state (s0, s1, previous input).
#[derive(Clone, Copy, Default)]
struct Fst {
    s0: f32,
    s1: f32,
    prev: f32,
}

impl Fst {
    /// One step; returns (new0, new1).
    #[inline(always)]
    fn step(&mut self, c: &Coef, u: f32) -> (f32, f32) {
        let h = (u + self.prev) - (self.s1 + self.s1);
        let new0 = h * c.c1 + self.s0 * c.c3 + self.s0;
        let new1 = h * c.c2 + self.s0 * c.c4 + self.s1;
        self.prev = u;
        self.s0 = new0;
        self.s1 = new1;
        (new0, new1)
    }
}

/// What one oscillator keeps of its own.
struct Osc {
    ty: i32,
    saws: [f32; 6],
    q: f32,
    coef: Coef,
    f1: Fst,
    f2: Fst,
    lp: [f32; 2],
    ca: f32,
    delay: Vec<f32>,
    wi: usize,
}

impl Osc {
    fn new() -> Osc {
        Osc {
            ty: 0,
            saws: [0.0; 6],
            q: 0.7,
            coef: Coef { f: !0, q: !0, k: 0.0, c1: 0.0, c2: 0.0, c3: 0.0, c4: 0.0 },
            f1: Fst::default(),
            f2: Fst::default(),
            lp: [f32::from_bits(0x2EDB_E6FF); 2],
            ca: 0.0,
            delay: vec![0.0; DELAY],
            wi: 0,
        }
    }
}

/// `shape` of the page.
#[inline(always)]
fn shape(x: f32, s1: f32) -> f32 {
    ((s1 * x) * x) * x + (1.0 - s1) * x
}

/// `tent` of the page.
#[inline(always)]
fn tent(z: f32) -> f32 {
    let a = 4.0 * z;
    if a >= 2.0 {
        4.0 - a
    } else {
        a
    }
}

#[inline(always)]
fn lim(a: f32) -> f32 {
    let b = if a > -0.5 { a } else { -0.5 };
    if b < 0.5 {
        b
    } else {
        0.5
    }
}

/// c from x and s1, for Pulse.
#[inline(always)]
fn pulse_c(x: f32, s1: f32) -> f32 {
    let d = ((x - 0.5).abs() * 16.0) * s1;
    if d < 4.0 {
        1.0 - (d - d.floor()).abs() * 4.0
    } else {
        1.0
    }
}

/// c from e, for Triangle: the sum and the product in double.
#[inline(always)]
fn tri_c(e: f32) -> f32 {
    let p = (e - 1.5) * e;
    let m = e * 40.0;
    ((p as f64 + 0.5) * m as f64) as f32
}

/// The tables of a control step, per oscillator.
struct Bl<'a> {
    saw: &'a [f32; 2048],
    tri: &'a [f32; 2048],
}

/// What one oscillator sample reads.
struct In<'a> {
    x: f32,
    i: f32,
    s0: f32,
    s1: f32,
    n: f32,
    wrapped: bool,
    aa: bool,
    bl: &'a Bl<'a>,
    step: &'static Blep,
    /// S for Feedback.
    sr: f32,
    idx: u8,
}

#[inline(always)]
fn osc_sample(o: &mut Osc, ring: &mut [f32; 256], a: &In) -> (f32, f32) {
    let (x, i, s0, s1) = (a.x, a.i, a.s0, a.s1);
    match o.ty {
        MULTISAW => {
            let mut f = (i as f64 * 2.5) as f32;
            if !(f <= 0.99) {
                f = 0.99;
            }
            o.coef.update(f, o.q);
            let c = (((s0 * 3.79733 - 4.31147) * s0 + 1.43787) * s0 + 0.076266699) * s0;
            let g1 = 0.72 - s1 * 0.4;
            let nn = 1.0 - s1;
            let g2 = 0.43 - ((nn * 0.4) * nn) * nn;
            let fac = [
                1.0 - c * 0.019523559,
                c * 0.019912209 + 1.0,
                1.0 - c * 0.06288439,
                c * 0.06216538 + 1.0,
                1.0 - c * 0.110023133,
                c * 0.107452422 + 1.0,
            ];
            let mut acc;
            if a.aa {
                acc = lookup(a.bl.saw, x * 2048.0) * g1;
                for l in 0..6 {
                    let mut q = fac[l] * i + o.saws[l];
                    if q > 1.0 {
                        q = q - 1.0;
                    }
                    o.saws[l] = q;
                    acc = acc + lookup(a.bl.saw, q * 2048.0) * g2;
                }
            } else {
                acc = g1 * (2.0 * x - 1.0);
                for l in 0..6 {
                    let mut q = fac[l] * i + o.saws[l];
                    if q > 1.0 {
                        q = q - 1.0;
                    }
                    o.saws[l] = q;
                    acc = acc + (2.0 * q - 1.0) * g2;
                }
            }
            let (new0, new1) = o.f1.step(&o.coef, acc);
            let v = (acc - new1) - o.coef.k * new0;
            (v, 2.0 * v + 2.0)
        }
        TRIWRAP => {
            let h = (1.0f64 - (s0 * 0.5) as f64) as f32;
            let v = if a.aa {
                let z = shape(x, s1);
                let g = h + 1.0;
                let d = (h * 0.25) / g;
                let wr = |t: f32| if t >= 1.0 { t - 1.0 } else { t };
                let aa_a = wr((z + 0.5) + d);
                let aa_b = wr((z + 0.5) - d);
                let aa_c = wr((z + 1.0) - d);
                let aa_e = wr(d + z);
                let s = a.bl.saw;
                let r = |t: &[f32; 2048], p: f32| lookup(t, p * 2048.0);
                (r(s, aa_c) - r(s, aa_e)) + (r(s, aa_a) - r(s, aa_b)) + r(a.bl.tri, z) * g
            } else {
                let t = tent(shape(x, s1));
                let w = (h + 1.0) * (t - 1.0);
                let mut e = 0.0f32;
                if w > 1.0 {
                    e = -2.0;
                }
                if w < -1.0 {
                    e = 2.0;
                }
                e + w
            };
            (v, 2.0 * v + 2.0)
        }
        NOISE => {
            let u = s0 + 0.5;
            let mut f = exp2(u + u) * (i + i);
            if !(f <= 0.99) {
                f = 0.99;
            }
            let r = 1.0 - s1 * 0.85499996;
            let qp = 1.0 - (r * r) * r;
            o.q = if qp <= 1.0 { qp } else { 1.0 };
            o.coef.update(f, o.q);
            let (_, n1) = o.f1.step(&o.coef, a.n);
            let y = n1 * (1.0 - qp);
            let (_, n2) = o.f2.step(&o.coef, y);
            let v = n2 + n2;
            (v, v)
        }
        FEEDBACK => {
            let mut g = (s1 * 3.0) * s1;
            g = g + g;
            let e = if a.aa { lookup(a.bl.saw, x * 2048.0) * 0.5 } else { x - 0.5 };
            let mut l = (s0 * 0.5 + 0.5) / i;
            if !(l < a.sr) {
                l = a.sr;
            }
            let d = (l + 0.5).floor() as i64;
            let mut ri = o.wi as i64 - d;
            if ri < 0 {
                ri += DELAY as i64;
            }
            let b = o.delay[ri as usize % DELAY];
            let mut u = b * g - g * 0.5;
            let l0 = (u - o.lp[0]) * o.ca + o.lp[0];
            o.lp[0] = l0;
            u = lim(u - l0);
            u = lim(u + e);
            let l1 = (u - o.lp[1]) * o.ca + o.lp[1];
            o.lp[1] = l1;
            u = lim(u - l1);
            o.wi += 1;
            if o.wi > DELAY - 1 {
                o.wi = 0;
            }
            o.delay[o.wi] = u;
            let v = u * 3.0;
            (v, v * 1.3333334 + 2.0)
        }
        PULSE => {
            let c = pulse_c(x, s1);
            let v = if a.aa {
                let mut u = s0 + x;
                if u >= 1.0 {
                    u = u - 1.0;
                }
                ((lookup(a.bl.saw, x * 2048.0) - lookup(a.bl.saw, u * 2048.0)) + (2.0 * s0 - 1.0)) * c
            } else if x < s0 {
                c
            } else {
                -c
            };
            (v, 2.0 * v + 2.0)
        }
        SAW => {
            let u = (s1 + 1.0) * x;
            let e = u - u.floor();
            let val = (((e - 1.5) * e + 0.5) * (e * 40.0)) * (2.0 * s0 - 1.0) + (2.0 * x - 1.0);
            let v = if a.aa {
                if a.wrapped {
                    let g = (s1 + 1.0) - (s1 + 1.0).floor();
                    let j = ((g - 1.5) * g + 0.5) * (((2.0 * s0 - 1.0) * 40.0) * g);
                    let amp = j * 0.5 + 1.0;
                    insert_step(ring, a.idx, a.step, clamp01(x / i), amp);
                }
                ring[a.idx.wrapping_add(128) as usize] += val;
                ring[a.idx as usize]
            } else {
                val
            };
            (v, 2.0 * v + 2.0)
        }
        _ => {
            let (r, e) = if a.aa {
                let z = shape(x, s1);
                let r = lookup(a.bl.tri, z * 2048.0);
                (r, r * 0.5 + 0.5)
            } else {
                let t = tent(shape(x, s1)) - 1.0;
                (t, t * 0.5 + 0.5)
            };
            let c = tri_c(e);
            let v = (c * s0 + r) * -2.0;
            (v, v + 2.0)
        }
    }
}

const LEVEL_R: u32 = 0x3F49_932F;

/// The level rule of the band-limited tables.
#[inline(always)]
fn level_rule(x2: bool, inc: f32) -> usize {
    let r = f32::from_bits(LEVEL_R);
    let mut i = 0;
    if x2 {
        let mut g = 0.25f32;
        while i < 29 {
            if g <= inc {
                break;
            }
            g = g * r;
            i += 1;
        }
    } else {
        let mut g = 0.5f32;
        while i < 29 {
            g = r * g;
            if g <= inc {
                break;
            }
            i += 1;
        }
    }
    i
}

#[inline(always)]
fn bl_level(t: &[f32], l: usize) -> &[f32; 2048] {
    (&t[l * BL_LEN..l * BL_LEN + 2048]).try_into().unwrap()
}

/// The shape rule from the four lanes.
#[inline(always)]
fn shape_rule(l: &[f32; 4]) -> [f32; 4] {
    let cl = |x: f32| {
        let a = if x < 1.0 { x } else { 1.0 };
        if a > 0.0 {
            a
        } else {
            0.0
        }
    };
    [cl(l[0]), cl(l[1]) * 0.9999 + 0.00005, cl(l[2]), cl(l[3]) * 0.9999 + 0.00005]
}

pub struct Digital {
    ph: [f32; 2],
    inc: [f32; 4],
    s: [f32; 4],
    st_base: [f32; 4],
    st_shape: [f32; 4],
    st_mix: f32,
    st_fm: f32,
    cache: [f32; 4],
    level: [usize; 2],
    osc: [Osc; 2],
    rings: Rings2,
    idx: u8,
    x1: f32,
    y1: f32,
    rng: Lcg,
    pink: [f32; 5],
    up: Up2,
    down: Down2,
    last_end: i64,
    prev_mode: Option<(i32, i32, bool, bool)>,
    need_init: bool,
    woke: bool,
    saws_set: bool,
    /// g of `osc-digital.md`: 1 in a new instance, + 1 at each transport start.
    saw_runs: usize,
}

/// The settings a pass reads.
#[derive(Clone, Copy)]
struct Set {
    t1: i32,
    t2: i32,
    aa: bool,
    x2: bool,
    ring: bool,
    pwm: [f32; 2],
    shape_mod: [f32; 2],
    s2: [f32; 3],
    mix: f32,
}

impl Set {
    fn read(x: &OscCtx) -> Set {
        let v = &x.s.raw;
        let sw = |i: usize| if v[i] >= 0.5 { 1.0f32 } else { 0.0 };
        let t1 = (v[id::OSC_DIGITALTYPE1].round() as i32).clamp(0, 6);
        let t2 = (v[id::OSC_DIGITALTYPE2].round() as i32).clamp(0, 6);
        let aa = v[id::OSC_DIGITALANTIALIAS] >= 0.5;
        let x2 = if !aa {
            x.acc != 0
        } else {
            match x.acc {
                0 => false,
                3 => true,
                _ => t1 == SAW || t2 == SAW,
            }
        };
        Set {
            t1,
            t2,
            aa,
            x2,
            ring: v[id::OSC_RINGMODPULSE] >= 0.5,
            pwm: [sw(id::OSC_PWM1ON), sw(id::OSC_PWM2ON)],
            shape_mod: [x.s.osc.shape_mod[0], x.s.osc.shape_mod[1]],
            s2: [v[id::OSC_DIGITALSHAPE2], v[id::OSC_DIGITALSHAPE3], v[id::OSC_DIGITALSHAPE4]],
            mix: v[id::OSC_OSCMIX],
        }
    }
}

impl Digital {
    /// `seed`: the voice's (a, b) of `data/osc-voice-seeds.tsv`.
    pub fn new(seed: (u32, u32)) -> Box<Digital> {
        let (ph, rng) = seed_phases(seed);
        Box::new(Digital {
            ph: [ph[0], ph[1]],
            inc: [0.0; 4],
            s: [0.0; 4],
            st_base: [0.0; 4],
            st_shape: [0.0; 4],
            st_mix: 0.0,
            st_fm: 0.0,
            cache: [0.0; 4],
            level: [0; 2],
            osc: [Osc::new(), Osc::new()],
            rings: Rings2([[0.0; 256]; 2]),
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
            saws_set: false,
            saw_runs: 1,
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

    /// A transport start: one more run of the per-voice reset sets the Multisaw phases anew (`osc-digital.md`).
    pub fn transport_start(&mut self) {
        self.saw_runs += 1;
        self.saws_set = false;
    }

    /// The Multisaw phases come from their own generator, by voice number.
    fn set_saws(&mut self, voice: usize) {
        self.saws_set = true;
        let mut g = Lcg::new(0, 0);
        for _ in 0..12 * (16 * self.saw_runs + voice.min(15)) {
            g.step();
        }
        for o in 0..2 {
            for l in 0..6 {
                self.osc[o].saws[l] = g.unit();
            }
        }
    }

    /// One call: `a`, `b` take 16 n samples of the mix and of the FM stream.
    pub fn run(&mut self, x: &OscCtx, a: &mut [f32], b: &mut [f32]) {
        if !self.saws_set {
            self.set_saws(x.voice);
        }
        let set = Set::read(x);
        let mode = (set.t1, set.t2, set.aa, set.x2);
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
        let t_shape = [s.pulse_width * 0.01, set.s2[0] * 0.01, set.s2[1] * 0.01, set.s2[2] * 0.01];
        let t_mix = clamp01((x.row(s.noise_src, 0) * s.noise_depth + set.mix) * 0.01);
        let t_fm = clamp01((x.row(s.fm_src, 0) * s.fm_depth + s.fm) * 0.01);
        if init {
            self.st_base = t_base;
            self.st_shape = t_shape;
            self.st_mix = t_mix;
            self.st_fm = t_fm;
        }
        // 2, slow ramps
        let f = (1.0 / (16.0 * n as f32)) * 4.0 * q_step;
        let (mut d_base, mut d_shape) = ([0.0f32; 4], [0.0f32; 4]);
        for l in 0..4 {
            d_base[l] = (t_base[l] - self.st_base[l]) * f;
            d_shape[l] = (t_shape[l] - self.st_shape[l]) * f;
        }
        let d_mix = (t_mix - self.st_mix) * f;
        let d_fm = (t_fm - self.st_fm) * f;
        // 3, drift
        let drift = drift_of(x, pink_draw(&mut self.rng, &mut self.pink));
        // 4, pitch points
        let ri = rcpn(if set.x2 { 2.0 } else { 1.0 });
        let dt = x.dt;
        let pin = PitchIn {
            e,
            d_base,
            st_base: self.st_base,
            drift,
            a: [s.tm1_depth; 4],
            b: [s.tm2_src_gain(0), s.tm2_src_gain(1), 0.0, 0.0],
        };
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
                self.ph = [s.rst_ph[0] * 0.01, s.rst_ph[1] * 0.01];
                for o in 0..2 {
                    self.osc[o].saws = [0.0; 6];
                }
            }
            self.inc = target[0];
            self.s = shape_rule(&self.st_shape);
            self.level = [level_rule(set.x2, self.inc[0]), level_rule(set.x2, self.inc[1])];
            let fs = x.fs * if set.x2 { 2.0 } else { 1.0 };
            let b_ = (((fs - 1.0).abs() + 384001.0) - (fs - 384000.0).abs()) * 0.5;
            let h = ((b_ * 0.5 + 0.0001 + 51.19998) - (51.2 - b_ * 0.5).abs()) * 0.5;
            let ca = 1.0 - (((h * -6.2831855) / b_) as f64).exp() as f32;
            for (o, t) in [set.t1, set.t2].into_iter().enumerate() {
                let c = &mut self.osc[o];
                c.ty = t;
                c.ca = ca;
                c.q = 0.7;
            }
            self.rings.0[0].fill(0.0);
            self.rings.0[1].fill(0.0);
            let mut left = if set.x2 { 4 } else { 8 };
            let (mut sa, mut sb) = ([0.0f32; 64], [0.0f32; 64]);
            while left > 0 {
                let m = left.min(n);
                self.last_end = x.time + x.c as i64 + m as i64;
                self.pass(x, set, m, false, &mut sa, &mut sb);
                left -= m;
            }
        }
        self.osc[0].ty = set.t1;
        self.osc[1].ty = set.t2;
        self.body(x, set, n, &target, &d_base, &d_shape, d_mix, d_fm, a, b);
        self.cache = last_p;
    }

    #[allow(clippy::too_many_arguments)]
    fn body(
        &mut self,
        x: &OscCtx,
        set: &Set,
        n: usize,
        target: &[[f32; 4]; 16],
        d_base: &[f32; 4],
        d_shape: &[f32; 4],
        d_mix: f32,
        d_fm: f32,
        a_out: &mut [f32],
        b_out: &mut [f32],
    ) {
        let s = &x.s.osc;
        let co = res_coefs();
        let (per_step, q, step): (usize, f32, &'static Blep) =
            if set.x2 { (8, 0.125, &x.tables.step2) } else { (4, 0.25, &x.tables.step1) };
        let e = per_step as f32;
        // 6, noise
        let ns = 16 * n;
        let mut noise = [0.0f32; 128];
        let bp = s.bp_noise + s.bp_noise;
        for i in 0..ns {
            let white = (self.rng.unit() * 4.0) - bp;
            if set.x2 {
                let (p, r) = self.up.step(&co, white);
                noise[2 * i] = p;
                noise[2 * i + 1] = r;
            } else {
                noise[i] = white;
            }
        }
        let r_dc = if set.x2 { 1.0 - 5.75 * x.dt } else { 1.0 - 11.5 * x.dt };
        let (mut out_y, mut out_t) = ([0.0f32; 128], [0.0f32; 128]);
        let (pa, sb) = (s.pw_depth * 0.01, s.shape_depth * 0.01);
        let slow = d_mix != 0.0 || d_fm != 0.0 || d_base.iter().any(|d| *d != 0.0) || d_shape.iter().any(|d| *d != 0.0);
        let sr = if set.aa && set.x2 { ((2.0 * x.fs) as i64) as f32 } else { (x.fs as i64) as f32 };
        let sync = s.sync[0];
        let mut pos = 0;
        let mut ph = self.ph;
        let mut inc = self.inc;
        let mut sv = self.s;
        let mut st_shape = self.st_shape;
        let (mut st_mix, mut st_fm) = (self.st_mix, self.st_fm);
        let (mut x1, mut y1) = (self.x1, self.y1);
        let mut idx = self.idx;
        let [r1, r2] = &mut self.rings.0;
        let [o1, o2] = &mut self.osc;
        for j in 0..4 * n {
            let pr = x.row(s.pw_src, j);
            let sr_ = x.row(s.shape_src, j);
            let mut l = [
                (pr * (pa * set.pwm[0]) + e * d_shape[0]) + st_shape[0],
                (sr_ * (sb * set.shape_mod[0]) + e * d_shape[1]) + st_shape[1],
                (pr * (pa * set.pwm[1]) + e * d_shape[2]) + st_shape[2],
                (sr_ * (sb * set.shape_mod[1]) + e * d_shape[3]) + st_shape[3],
            ];
            if let Some(m) = x.mx_shape {
                for v in l.iter_mut() {
                    *v = *v + m[j];
                }
            }
            let t_s = shape_rule(&l);
            let t = &target[j];
            if set.aa {
                self.level = [level_rule(set.x2, t[0]), level_rule(set.x2, t[1])];
            }
            let bl1 = Bl { saw: bl_level(&x.tables.bl_saw, self.level[0]), tri: bl_level(&x.tables.bl_tri, self.level[0]) };
            let bl2 = Bl { saw: bl_level(&x.tables.bl_saw, self.level[1]), tri: bl_level(&x.tables.bl_tri, self.level[1]) };
            let di = [(t[0] - inc[0]) * q, (t[1] - inc[1]) * q];
            let mut ds = [0.0f32; 4];
            for k in 0..4 {
                ds[k] = (t_s[k] - sv[k]) * q;
            }
            for _ in 0..per_step {
                if slow {
                    for l in 0..4 {
                        self.st_base[l] += d_base[l];
                        st_shape[l] += d_shape[l];
                    }
                    st_mix += d_mix;
                    st_fm += d_fm;
                }
                inc[0] += di[0];
                inc[1] += di[1];
                for k in 0..4 {
                    sv[k] += ds[k];
                }
                let nv = noise[pos];
                // the pair core
                let old = ph[0];
                let mut p1 = inc[0] + old;
                let wrapped1 = p1 >= 1.0;
                if wrapped1 {
                    p1 = p1 - 1.0;
                }
                let (v1, t1) = osc_sample(
                    o1,
                    r1,
                    &In { x: p1, i: inc[0], s0: sv[0], s1: sv[1], n: nv, wrapped: wrapped1, aa: set.aa, bl: &bl1, step, sr, idx },
                );
                let mut start2 = ph[1];
                if sync && p1 < old {
                    start2 = (p1 / inc[0]) * inc[1];
                }
                let mut i2 = if st_fm == 0.0 { inc[1] } else { fmfactor(v1 * st_fm) * inc[1] };
                if !(i2 <= 0.125) {
                    i2 = 0.125;
                }
                let mut p2 = start2 + i2;
                let wrapped2 = p2 >= 1.0;
                if wrapped2 {
                    p2 = p2 - 1.0;
                }
                let (mut v2, _) = osc_sample(
                    o2,
                    r2,
                    &In { x: p2, i: i2, s0: sv[2], s1: sv[3], n: nv, wrapped: wrapped2, aa: set.aa, bl: &bl2, step, sr, idx },
                );
                if set.ring {
                    v2 = v2 * v1;
                }
                ph[0] = p1;
                ph[1] = p2;
                let m = st_mix;
                let g = 0.83 * m;
                let y = (1.0 - (g + 0.17) * m) * v1 + ((1.83 - g) * m) * v2;
                if set.aa {
                    r1[idx as usize] = 0.0;
                    r2[idx as usize] = 0.0;
                    idx = idx.wrapping_add(1);
                }
                let yd = (y - x1) + r_dc * y1;
                y1 = yd;
                x1 = y;
                out_y[pos] = yd;
                out_t[pos] = t1;
                pos += 1;
            }
        }
        self.ph = ph;
        self.inc = inc;
        self.s = sv;
        self.st_shape = st_shape;
        self.st_mix = st_mix;
        self.st_fm = st_fm;
        self.x1 = x1;
        self.y1 = y1;
        self.idx = idx;
        if set.x2 {
            for i in 0..ns {
                let (p, r) = self.down.step(&co, out_y[2 * i], out_t[2 * i], out_y[2 * i + 1], out_t[2 * i + 1]);
                a_out[i] = p;
                b_out[i] = r;
            }
        } else {
            a_out[..ns].copy_from_slice(&out_y[..ns]);
            b_out[..ns].copy_from_slice(&out_t[..ns]);
        }
    }
}

trait Tm2 {
    fn tm2_src_gain(&self, l: usize) -> f32;
}

impl Tm2 for crate::settings::OscSet {
    /// Tune2ModDepth reaches only the oscillators with their TuneModOsc on.
    #[inline(always)]
    fn tm2_src_gain(&self, l: usize) -> f32 {
        self.tune_mod[l] * self.tm2_depth
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ca_page_values() {
        for (fs, want) in [(44100.0f32, 0.007268131f32), (48000.0, 0.006679535), (88200.0, 0.0036407113), (96000.0, 0.0033453703), (192000.0, 0.0016742349)] {
            let b = (((fs - 1.0f32).abs() + 384001.0) - (fs - 384000.0).abs()) * 0.5;
            let h = ((b * 0.5 + 0.0001 + 51.19998) - (51.2 - b * 0.5).abs()) * 0.5;
            let ca = 1.0 - (((h * -6.2831855) / b) as f64).exp() as f32;
            assert!((ca - want).abs() < 2e-8, "{fs} {ca} {want}");
        }
    }
}
