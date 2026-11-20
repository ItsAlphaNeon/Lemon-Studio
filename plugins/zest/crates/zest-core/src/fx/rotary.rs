// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Rotary (`pages/fx-rotary.md`). Base rate; a rotor's four taps run in four `F4` lanes, the two sides in two lanes where the
//! steps are per side.

use super::shared::trunc_i64;
use crate::dsp::F4;
use crate::params::id;
use crate::simd::*;
use std::f64::consts::PI;

const CURVES: &str = include_str!("../../data/rotary-curves.tsv");
const TWO_32: f32 = 4294967296.0;

/// The three curve tables, shared by every instance.
struct Tables {
    sine: [f32; 257],
    a: [f32; 257],
    b: [f32; 257],
}

fn tables() -> &'static Tables {
    static T: std::sync::OnceLock<Box<Tables>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut sine = [0.0f32; 257];
        for (i, v) in sine.iter_mut().enumerate().take(256) {
            *v = (2.0 * PI * i as f64 / 255.0).sin() as f32;
        }
        sine[256] = 0.0;
        Box::new(Tables { sine, a: directivity("A", -10.0), b: directivity("B", -3.0) })
    })
}

/// A directivity table (section 4).
fn directivity(name: &str, flat: f64) -> [f32; 257] {
    let rows: Vec<[f64; 6]> = CURVES
        .lines()
        .filter(|l| l.starts_with(name))
        .map(|l| {
            let c: Vec<f64> = l.split('\t').skip(1).map(|x| x.trim().parse().unwrap_or(0.0)).collect();
            [c[0], c[1], c[2], c[3], c[4], c[5]]
        })
        .collect();
    let db = |theta: f64| {
        if (60.0..=120.0).contains(&theta) {
            return flat;
        }
        let r = rows.iter().find(|r| theta >= r[0] && theta <= r[1]).expect("a curve row for every angle");
        let [t0, t1, d0, s0, d1, s1] = *r;
        let h = t1 - t0;
        let u = (theta - t0) / h;
        let (u2, u3) = (u * u, u * u * u);
        (2.0 * u3 - 3.0 * u2 + 1.0) * d0 + (u3 - 2.0 * u2 + u) * h * s0 + (-2.0 * u3 + 3.0 * u2) * d1 + (u3 - u2) * h * s1
    };
    let mut t = [0.0f32; 257];
    t[0] = 1.0;
    for i in 1..=129 {
        let theta = (i - 1) as f64 * 360.0 / 256.0;
        t[i] = 10f64.powf(db(theta) / 20.0) as f32;
    }
    for i in 130..=255 {
        t[i] = t[259 - i];
    }
    t[256] = 1.0;
    t
}

/// Four table readings at four phases.
#[inline(always)]
fn curve4(t: &[f32; 257], p: __m128i) -> F4 {
    unsafe {
        let mut idx = [0u32; 4];
        _mm_storeu_si128(idx.as_mut_ptr() as *mut __m128i, _mm_srli_epi32(p, 24));
        let [i0, i1, i2, i3] = idx.map(|i| (i & 255) as usize);
        let f = F4(_mm_cvtepi32_ps(_mm_and_si128(p, _mm_set1_epi32(0xff_ffff)))) * F4::splat(5.9604645e-08);
        let a = F4::new(t[i0], t[i1], t[i2], t[i3]);
        let b = F4::new(t[i0 + 1], t[i1 + 1], t[i2 + 1], t[i3 + 1]);
        (b - a) * f + a
    }
}

/// A rotor's delay ring (section 3).
struct Ring {
    buf: Box<[f32]>,
    w: i32,
    n: i32,
}

impl Ring {
    fn new(n: i32) -> Ring {
        Ring { buf: vec![0.0; n as usize + 1].into_boxed_slice(), w: 0, n }
    }

    /// The four taps at the positions in `pos`.
    #[inline(always)]
    fn read4(&self, pos: F4) -> F4 {
        unsafe {
            let k = _mm_cvttps_epi32((pos + F4::splat(1.0)).0);
            let frac = F4(_mm_cvtepi32_ps(k)) - pos;
            let j = _mm_sub_epi32(_mm_set1_epi32(self.w), k);
            let j = _mm_add_epi32(j, _mm_and_si128(_mm_srai_epi32(j, 31), _mm_set1_epi32(self.n - 1)));
            let mut q = [0i32; 4];
            _mm_storeu_si128(q.as_mut_ptr() as *mut __m128i, j);
            let [a, b, c, d] = q.map(|x| x as usize);
            let buf = &self.buf;
            let y0 = F4::new(buf[a], buf[b], buf[c], buf[d]);
            let y1 = F4::new(buf[a + 1], buf[b + 1], buf[c + 1], buf[d + 1]);
            (y1 - y0) * frac + y0
        }
    }

    #[inline(always)]
    fn write(&mut self, x: f32) {
        self.buf[self.w as usize] = x;
        self.w += 1;
        if self.w >= self.n {
            self.buf[0] = self.buf[self.n as usize - 1];
            self.w = 1;
        }
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.w = 0;
    }
}

#[derive(Clone, Copy)]
struct Biquad4 {
    x1: F4,
    x2: F4,
    y1: F4,
    y2: F4,
}

impl Biquad4 {
    fn new() -> Biquad4 {
        Biquad4 { x1: F4::zero(), x2: F4::zero(), y1: F4::zero(), y2: F4::zero() }
    }
}

/// One side's horn filters.
#[derive(Clone, Copy)]
struct HornSide {
    s1: Biquad4,
    s2: Biquad4,
    lx1: f32,
    lx2: f32,
    ly1: f32,
    ly2: f32,
}

impl HornSide {
    fn new() -> HornSide {
        HornSide { s1: Biquad4::new(), s2: Biquad4::new(), lx1: 0.0, lx2: 0.0, ly1: 0.0, ly2: 0.0 }
    }
}

/// Constants of construction (section 3).
#[derive(Clone, Copy)]
struct Geometry {
    pos_b: F4,
    pos_a: F4,
    a1: F4,
    b1: F4,
    a2: F4,
    b2: F4,
    lp: [f32; 5],
    c0: F4,
    c1: F4,
    c2: F4,
    kph: f32,
}

const MIX: usize = 0;
const DRY: usize = 1;
const G1: usize = 2;
const RA: usize = 3;
const G2: usize = 4;
const RB: usize = 5;
const BAL1: usize = 6;
const BAL2: usize = 7;
const STER: usize = 8;
const OUT: usize = 9;
const SPEED: usize = 10;

pub struct Rotary {
    dt: f32,
    t: &'static Tables,
    g: Geometry,
    drum: [Ring; 2],
    horn: [Ring; 2],
    side: [HornSide; 2],
    // crossover on (L, R) lanes
    xx1: F4,
    xx2: F4,
    xa1: F4,
    xb1: F4,
    xb2: F4,
    state: [f32; 12],
    step: [f32; 12],
    sp_h: f32,
    sp_d: f32,
    c_h: f32,
    c_d: f32,
    ph_h: u32,
    ph_d: u32,
    mode: i32,
    reset: bool,
    /// The R side's phase offsets of the last STER value.
    last_ster: f32,
    ster_off: [u32; 4],
}

impl Rotary {
    pub fn new(fs: f32) -> Rotary {
        let f = fs as f64;
        let spm = f / 340.0;
        let r = 0.25f64 + 0.1;
        let a = r + 0.3;
        let off1 = ((((a + a) * a).sqrt() + 0.1) + 0.3) * spm;
        let off2 = (spm + spm) * r;
        let off3 = (((r + r) * r).sqrt() + 0.1) * spm;
        let l = spm * 0.25;
        let m = (l + l).max(l * 1.6 + off1).max(l * 1.75 + off2).max(l * 1.9 + off3);
        let n = ((m as f32 + 1.0) as i32) + 1;
        let (lf, off1, off2, off3) = (l as f32, off1 as f32, off2 as f32, off3 as f32);

        let stage = |f0: f64, bw: f64| {
            let t = ((bw * PI) / f).tan();
            ((-((f0 * 2.0 * PI) / f).cos()) as f32, ((1.0 - t) / (t + 1.0)) as f32)
        };
        let (a1, b1) = stage(5000.0, 2000.0);
        let (a2, b2) = stage(2000.0, 1000.0);

        let q = 1.0 / 2f64.sqrt();
        let w = (2.0 * PI * 4000.0) / f;
        let (s, c) = (w.sin(), w.cos());
        let alpha = s / (q + q);
        let a0 = alpha + 1.0;
        let lp = [
            (((1.0 - c) * 0.5) / a0) as f32,
            ((1.0 - c) / a0) as f32,
            (((1.0 - c) * 0.5) / a0) as f32,
            ((c * -2.0) / a0) as f32,
            ((1.0 - alpha) / a0) as f32,
        ];

        let w = (800.0 * PI) / f;
        let w2 = w + w;
        let alpha = w2.sin() / (q + q);
        let nn = 1.0 / (alpha + 1.0);
        let t = w.tan();
        let c0 = (-(1.0 - t) / (t + 1.0)) as f32;
        let c1 = ((1.0 - alpha) * nn) as f32;
        let c2 = ((w2.cos() * -2.0) * nn) as f32;

        Rotary {
            dt: 1.0 / fs,
            t: tables(),
            g: Geometry {
                pos_b: F4::new(lf, lf * 0.6, lf * 0.75, lf * 0.9),
                pos_a: F4::new(1.0, lf + off1, (lf + 1.0) + off2, (lf + 1.0) + off3),
                a1: F4::splat(a1),
                b1: F4::splat(b1),
                a2: F4::splat(a2),
                b2: F4::splat(b2),
                lp,
                c0: F4::splat(c0),
                c1: F4::splat(c1),
                c2: F4::splat(c2),
                kph: ((4294967296.0f64 - 1.0) / f) as f32,
            },
            drum: [Ring::new(n), Ring::new(n)],
            horn: [Ring::new(n), Ring::new(n)],
            side: [HornSide::new(); 2],
            xx1: F4::zero(),
            xx2: F4::zero(),
            xa1: F4::zero(),
            xb1: F4::zero(),
            xb2: F4::zero(),
            state: [0.0; 12],
            step: [0.0; 12],
            sp_h: 0.0,
            sp_d: 0.0,
            c_h: 0.0,
            c_d: 0.0,
            ph_h: 0,
            ph_d: 0,
            mode: 0,
            reset: true,
            last_ster: f32::NAN,
            ster_off: [0; 4],
        }
    }

    /// Everything as new but the stored Mode.
    pub fn reactivate(&mut self) {
        for r in self.drum.iter_mut().chain(self.horn.iter_mut()) {
            r.clear();
        }
        self.side = [HornSide::new(); 2];
        self.xx1 = F4::zero();
        self.xx2 = F4::zero();
        self.xa1 = F4::zero();
        self.xb1 = F4::zero();
        self.xb2 = F4::zero();
        self.state = [0.0; 12];
        self.step = [0.0; 12];
        self.sp_h = 0.0;
        self.sp_d = 0.0;
        self.c_h = 0.0;
        self.c_d = 0.0;
        self.ph_h = 0;
        self.ph_d = 0;
        self.reset = true;
        self.last_ster = f32::NAN;
    }

    pub fn setup(&mut self, raw: &[f32], o: usize, rows: &[f32; 448]) {
        let mode = raw[id::RTARY1_MODE + o].round() as i32;
        if mode != self.mode {
            self.ph_h = 0;
            self.ph_d = 0;
            self.mode = mode;
        }
        let d = raw[id::RTARY1_DRIVE + o] * 0.01;
        let b = raw[id::RTARY1_BALANCE + o] * 0.01;
        let mix = raw[id::RTARY1_MIX + o] * 0.01;
        let g1 = 10f64.powf((d * 1.5) as f64) as f32;
        let g2 = 10f64.powf(d as f64) as f32;
        let ra = 1.0 / g1.sqrt();
        let rb = 1.0 / g2.sqrt();
        let c = 1.0 - b;
        let s = (raw[id::RTARY1_STEREO + o] as f64 * 0.004898979485566356) as f32;
        let mut target = [0.0f32; 11];
        target[MIX] = mix;
        target[DRY] = 1.0 - mix;
        target[G1] = g1;
        target[RA] = ra;
        target[G2] = g2;
        target[RB] = rb;
        target[BAL1] = crate::dsp::pmax(1.0, b + b);
        target[BAL2] = crate::dsp::pmax(1.0, c + c);
        target[STER] = s * s;
        target[OUT] = 10f64.powf((raw[id::RTARY1_OUT + o] * 0.05) as f64) as f32;

        let cont = raw[id::RTARY1_CONTROLLER + o].round().clamp(0.0, 3.0) as usize;
        let ctrl = rows[[1usize, 3, 4, 7][cont] * 16];
        let (slow, fast) = (raw[id::RTARY1_SLOW + o], raw[id::RTARY1_FAST + o]);
        let v = (fast - slow) * ctrl + slow;
        target[SPEED] = ((v as f64 * 0.04382026634673881).exp() * 0.1f32 as f64) as f32;

        let k = 0.0016666666666666668f64.ln() / (15f64.ln() as f32 as f64);
        let e = (raw[id::RTARY1_RISETIME + o] as f64 * (k * 0.01f32 as f64)) as f32;
        let t = (((15f64.powf(e as f64) as f32) as f64) * 60.0) as f32;
        let glide = |t: f32| ((t as f64 * -2.0 * PI) * self.dt as f64).exp() as f32;
        self.c_h = glide(t);
        self.c_d = if mode == 0 { glide(t * 0.8) } else { self.c_h };

        for i in 0..11 {
            self.step[i] = (target[i] - self.state[i]) / 64.0;
        }
        if self.reset {
            self.reset = false;
            self.state[..11].copy_from_slice(&target);
            self.state[RA] = target[RB];
            self.state[RB] = target[RA];
        }
    }

    pub fn chunk(&mut self, l: &mut [f32; 16], r: &mut [f32; 16]) {
        let g = self.g;
        let t = self.t;
        let mode2 = self.mode == 2;
        let (mut sp_h, mut sp_d) = (self.sp_h, self.sp_d);
        let (c_h, c_d) = (self.c_h, self.c_d);
        let (mut ph_h, mut ph_d) = (self.ph_h, self.ph_d);
        let mut st = self.state;
        let step = self.step;
        let (mut xx1, mut xx2, mut xa1, mut xb1, mut xb2) = (self.xx1, self.xx2, self.xa1, self.xb1, self.xb2);
        let half = F4::splat(0.5);
        let qs: __m128i = unsafe { _mm_setr_epi32(0, 0x4000_0000, 0x8000_0000u32 as i32, 0xc000_0000u32 as i32) };
        let mut last_ster = self.last_ster;
        let mut off_r = self.ster_off;

        for n in 0..16 {
            for i in 0..12 {
                st[i] = step[i] + st[i];
            }
            let sp = st[SPEED];
            sp_h = (sp_h - sp) * c_h + sp;
            sp_d = (sp_d - sp) * c_d + sp;
            ph_h = ph_h.wrapping_add(trunc_i64(sp_h * g.kph) as u32);
            ph_d = ph_d.wrapping_add(trunc_i64(sp_d * g.kph) as u32);

            let dry_in = F4::new(l[n], r[n], 0.0, 0.0);
            let x = dry_in * half;
            let a = (x - xa1) * g.c0 + xx1;
            let b = ((x - xb2) * g.c1 + xx2) + (xx1 - xb1) * g.c2;
            xx2 = xx1;
            xa1 = a;
            xb2 = xb1;
            xx1 = x;
            xb1 = b;
            let bass = a + b;
            let treble = a - b;
            let [bass_l, bass_r, ..] = bass.arr();
            let [treble_l, treble_r, ..] = treble.arr();

            let ster = st[STER];
            if ster != last_ster {
                last_ster = ster;
                for (o, q) in off_r.iter_mut().zip([0.0f32, 0.25, 0.5, 0.75]) {
                    *o = trunc_i64((ster + q) * TWO_32) as u32;
                }
            }
            let off_r_v: __m128i = unsafe { _mm_loadu_si128(off_r.as_ptr() as *const __m128i) };

            let (d_l, d_r) = if mode2 {
                (bass_l, bass_r)
            } else {
                let (dl, dr) = rotor_sines(t, ph_d, qs, off_r_v);
                let (pl, gl) = positions(&g, dl);
                let (pr, gr) = positions(&g, dr);
                let [t0, t1, t2, t3] = (self.drum[0].read4(pl) * gl).arr();
                let dl = ((t1 * 0.6 + t0) + t2 * 0.75) + t3 * 0.9;
                let [t0, t1, t2, t3] = (self.drum[1].read4(pr) * gr).arr();
                let dr = ((t1 * 0.6 + t0) + t2 * 0.75) + t3 * 0.9;
                self.drum[0].write(bass_l);
                self.drum[1].write(bass_r);
                (dl, dr)
            };

            let (g1, g2) = (st[G1], st[G2]);
            let [tc_l, tc_r, yc_l, yc_r] = F4::new(treble_l * g2, treble_r * g2, d_l * g1, d_r * g1).tclip().arr();
            let (ra, rb) = (st[RA], st[RB]);
            let (t_l, t_r, y_l, y_r) = (tc_l * rb, tc_r * rb, yc_l * ra, yc_r * ra);

            let (hl, hr) = rotor_sines(t, ph_h, qs, off_r_v);
            let h_l = horn(&g, t, &mut self.horn[0], &mut self.side[0], t_l, hl, unsafe { _mm_add_epi32(qs, _mm_set1_epi32(ph_h as i32)) });
            let h_r = horn(&g, t, &mut self.horn[1], &mut self.side[1], t_r, hr, unsafe { _mm_add_epi32(off_r_v, _mm_set1_epi32(ph_h as i32)) });

            let (b1, b2) = (F4::splat(st[BAL1]), F4::splat(st[BAL2]));
            let hv = F4::new(h_l, h_r, 0.0, 0.0);
            let yv = F4::new(y_l, y_r, 0.0, 0.0);
            let rotor = (((b2 * half) * hv + (F4::splat(1.0) - b2) * treble) + (b1 * half) * yv) + (F4::splat(1.0) - b1) * bass;
            let out = (rotor * F4::splat(st[MIX]) + dry_in * F4::splat(st[DRY])) * F4::splat(st[OUT]);
            let [o_l, o_r, ..] = out.arr();
            l[n] = o_l;
            r[n] = o_r;
        }
        self.state = st;
        self.sp_h = sp_h;
        self.sp_d = sp_d;
        self.ph_h = ph_h;
        self.ph_d = ph_d;
        self.xx1 = xx1;
        self.xx2 = xx2;
        self.xa1 = xa1;
        self.xb1 = xb1;
        self.xb2 = xb2;
        self.last_ster = last_ster;
        self.ster_off = off_r;
    }
}

/// The sines s0, s1 of a rotor for both sides: the phases q = 0 and q = 0.25.
#[inline(always)]
fn rotor_sines(t: &Tables, ph: u32, qs: __m128i, off_r: __m128i) -> ((f32, f32), (f32, f32)) {
    unsafe {
        let pv = _mm_set1_epi32(ph as i32);
        let lv = _mm_add_epi32(qs, pv);
        let rv = _mm_add_epi32(off_r, pv);
        // lanes: left q0, left q25, right q0, right q25
        let p = _mm_unpacklo_epi64(lv, rv);
        let [s0l, s1l, s0r, s1r] = curve4(&t.sine, p).arr();
        ((s0l, s1l), (s0r, s1r))
    }
}

/// Tap positions and gains from (s0, s1).
#[inline(always)]
fn positions(g: &Geometry, (s0, s1): (f32, f32)) -> (F4, F4) {
    let (n0, n1) = (-s0, -s1);
    let pos = g.pos_b * F4::new(s1 + 1.0, s0, n1, n0) + g.pos_a;
    let gain = F4::new(s1, s0, n1, n0) * F4::new(0.15, 0.4375, 0.375, 0.3225) + F4::new(0.85, 0.5625, 0.625, 0.6775);
    (pos, gain)
}

/// One side's horn: `input` into the ring, the taps through the directivity network.
#[inline(always)]
fn horn(g: &Geometry, t: &Tables, ring: &mut Ring, sd: &mut HornSide, input: f32, sines: (f32, f32), p: __m128i) -> f32 {
    let (pos, gain) = positions(g, sines);
    let xv = ring.read4(pos) * gain;
    ring.write(input);
    let bq = curve4(&t.b, p);
    let aq = curve4(&t.a, p);

    let half = F4::splat(0.5);
    let s = &mut sd.s1;
    let y = ((g.a1 * s.x1 + g.b1 * xv) + s.x2) - ((s.y1 * g.a1) + (g.b1 * s.y2));
    let z1 = ((bq * (xv - y) + y) + xv) * half;
    s.x2 = s.x1;
    s.x1 = xv;
    s.y2 = s.y1;
    s.y1 = y;

    let s = &mut sd.s2;
    let y = ((g.b2 * z1 + g.a2 * s.x1) + s.x2) - ((g.b2 * s.y2) + (s.y1 * g.a2));
    let z2 = ((aq * (z1 - y) + y) + z1) * half;
    s.x2 = s.x1;
    s.x1 = z1;
    s.y2 = s.y1;
    s.y1 = y;

    let [z0, z21, z22, z23] = z2.arr();
    let m = (z21 * 0.6 + z22 * 0.75) + z23 * 0.9;
    let [b0, b1, b2, a1, a2] = g.lp;
    let y = ((sd.lx1 * b1 + m * b0) + sd.lx2 * b2) - sd.ly1 * a1 - sd.ly2 * a2;
    sd.lx2 = sd.lx1;
    sd.lx1 = m;
    sd.ly2 = sd.ly1;
    sd.ly1 = y;
    z0 + y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_lengths_of_the_page() {
        assert_eq!(Rotary::new(44100.0).drum[0].n, 224);
        assert_eq!(Rotary::new(48000.0).drum[0].n, 244);
        assert_eq!(Rotary::new(96000.0).drum[0].n, 487);
    }

    #[test]
    fn curves_cover_the_circle() {
        let t = tables();
        assert_eq!(t.a[0], 1.0);
        assert_eq!(t.a[1], 1.0);
        assert_eq!(t.a[256], 1.0);
        assert_eq!(t.a[130], t.a[129]);
        assert_eq!(t.b[255], t.b[4]);
        assert!((t.a[65] - 10f32.powf(-0.5)).abs() < 1e-6);
        assert_eq!(t.sine[256], 0.0);
    }
}
