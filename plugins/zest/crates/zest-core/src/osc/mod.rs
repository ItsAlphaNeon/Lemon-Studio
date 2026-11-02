// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The oscillator models. Each voice keeps one state per model; the model in use runs at the voice's place in the order.

pub mod dco;
pub mod digital;
pub mod dual;
pub mod eco;
pub mod triple;

use crate::dsp::{chain_step, F4};
use crate::settings::Settings;
use crate::tables::{Blep, Tables};

/// What a model's call reads.
pub struct OscCtx<'a> {
    pub s: &'a Settings,
    pub tables: &'static Tables,
    /// The shared row buffer: row * 16 + position.
    pub rows: &'a [f32; 448],
    /// Zest's matrix, 16 points; `None` without active slots of that target.
    pub mx_pitch: Option<&'a [f32; 16]>,
    pub mx_shape: Option<&'a [f32; 16]>,
    pub fs: f32,
    pub dt: f32,
    pub voice: usize,
    /// Chunk offset and count of the call, the block's chunk counter.
    pub c: usize,
    pub n: usize,
    pub time: i64,
    pub pw_slop: f32,
    /// Effective accuracy (`index.md`, Rendering a span, step 3).
    pub acc: i32,
}

impl OscCtx<'_> {
    #[inline(always)]
    pub fn row(&self, s: usize, j: usize) -> f32 {
        self.rows[16 * s + 4 * self.c + j]
    }
}

/// Band-limited step insertion (`osc-triple.md` section 8): ring[(idx + k) & 255] -= T * amplitude.
#[inline(never)]
pub fn insert_step(ring: &mut [f32; 256], idx: u8, tab: &Blep, fraction: f32, amp: f32) {
    let a = 128.0 * fraction;
    let j = (a as usize).min(127);
    let fr = a - j as f32;
    let i0 = idx as usize;
    let first = (256 - i0).min(TAPS);
    let (head, tail) = ring.split_at_mut(i0);
    if j == 127 {
        let col = &tab.col[127];
        for (r, t) in tail[..first].iter_mut().zip(&col[..first]) {
            *r -= *t * amp;
        }
        for (r, t) in head.iter_mut().zip(&col[first..]) {
            *r -= *t * amp;
        }
    } else {
        let (c0, c1) = (&tab.col[j], &tab.col[j + 1]);
        for ((r, t0), t1) in tail[..first].iter_mut().zip(&c0[..first]).zip(&c1[..first]) {
            *r -= (*t0 + (*t1 - *t0) * fr) * amp;
        }
        for ((r, t0), t1) in head.iter_mut().zip(&c0[first..]).zip(&c1[first..]) {
            *r -= (*t0 + (*t1 - *t0) * fr) * amp;
        }
    }
}

/// Band-limited ramp insertion: ring[(idx + k) & 255] = T * amplitude + ring[..].
#[inline(never)]
pub fn insert_ramp(ring: &mut [f32; 256], idx: u8, tab: &Blep, fraction: f32, amp: f32) {
    let a = 128.0 * fraction;
    let j = a as usize;
    let fr = a - j as f32;
    let i0 = idx as usize;
    let first = (256 - i0).min(TAPS);
    let (head, tail) = ring.split_at_mut(i0);
    let (c0, c1) = (&tab.col[j], &tab.col[j + 1]);
    for ((r, t0), t1) in tail[..first].iter_mut().zip(&c0[..first]).zip(&c1[..first]) {
        *r = ((*t1 - *t0) * fr + *t0) * amp + *r;
    }
    for ((r, t0), t1) in head.iter_mut().zip(&c0[first..]).zip(&c1[first..]) {
        *r = ((*t1 - *t0) * fr + *t0) * amp + *r;
    }
}

use crate::tables::TAPS;

/// clamp(x, 0, 1): max with 0, then min with 1. NaN goes to 0.
#[inline(always)]
pub fn clamp01(x: f32) -> f32 {
    let a = if x > 0.0 { x } else { 0.0 };
    if a < 1.0 {
        a
    } else {
        1.0
    }
}

/// clamp(x, lo, hi): max with lo, then min with hi.
#[inline(always)]
pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    let a = if x > lo { x } else { lo };
    if a < hi {
        a
    } else {
        hi
    }
}

/// x - trunc(x).
#[inline(always)]
pub fn ftrunc(x: f32) -> f32 {
    x - (x as i32) as f32
}

/// 2^x by a series on |x|; the refined reciprocal below zero (`osc-dual.md`).
#[inline(always)]
pub fn fmfactor(x: f32) -> f32 {
    let a = x.abs();
    let s = a * 0.0001540353 + 0.0013333558;
    let s = s * a + 0.009618129;
    let s = s * a + 0.05550411;
    let s = s * a + 0.2402265;
    let s = s * a + 0.6931472;
    let s = s * a + 1.0;
    if x >= 0.0 {
        s
    } else {
        crate::dsp::rcpn(s)
    }
}

/// Per-voice tune slop of oscillator `o`, in semitones.
#[inline]
pub fn tune_slop(x: &OscCtx, o: usize) -> f32 {
    let k = x.s.tune_slop * 0.01;
    ((k * k) * x.s.trims[o][x.voice & 7]) * 0.01
}

/// The drift of a pass, from one pink draw.
#[inline]
pub fn drift_of(x: &OscCtx, pink: f32) -> f32 {
    let d = x.s.osc.drift * 0.01;
    ((d * d) * 0.1) * 0.25 * pink
}

/// Linear read of one 2048-value table level.
#[inline(always)]
pub fn lookup(t: &[f32; 2048], v: f32) -> f32 {
    let k = v as i32;
    let r = v - k as f32;
    (1.0 - r) * t[(k & 2047) as usize] + r * t[(k.wrapping_add(1) & 2047) as usize]
}

/// One level of a wavetable.
#[inline(always)]
pub fn level_of(t: &[f32], l: usize) -> &[f32; 2048] {
    (&t[l * 2048..l * 2048 + 2048]).try_into().unwrap()
}

/// The wavetable level of a test value (`tables.md`, Level choice).
#[inline(always)]
pub fn wave_level(test: f32) -> usize {
    let mut g = 0.00048828125f32;
    let mut i = 0;
    while i < 9 && test > g {
        g = g + g;
        i += 1;
    }
    i
}

/// The events of one sample: fraction, amplitude and ring number.
#[derive(Clone, Copy)]
pub struct Events {
    pub f: [f32; 12],
    pub a: [f32; 12],
    pub r: [u8; 12],
    pub n: usize,
}

impl Events {
    #[inline(always)]
    pub fn new() -> Events {
        Events { f: [0.0; 12], a: [0.0; 12], r: [0; 12], n: 0 }
    }
    #[inline(always)]
    pub fn push(&mut self, f: f32, a: f32) {
        self.f[self.n] = f;
        self.a[self.n] = a;
        self.n += 1;
    }
    #[inline(always)]
    pub fn push_on(&mut self, ring: u8, f: f32, a: f32) {
        self.f[self.n] = f;
        self.a[self.n] = a;
        self.r[self.n] = ring;
        self.n += 1;
    }
}

const MERGE: f32 = 0.7 / 128.0;

/// One column of a step table at a fraction, interpolated.
#[inline(never)]
fn step_column(tab: &Blep, fraction: f32, v: &mut [f32; TAPS]) {
    let a = 128.0 * fraction;
    let j = (a as usize).min(127);
    if j == 127 {
        v.copy_from_slice(&tab.col[127]);
    } else {
        let w = a - j as f32;
        let (c0, c1) = (&tab.col[j], &tab.col[j + 1]);
        for ((o, t0), t1) in v.iter_mut().zip(c0.iter()).zip(c1.iter()) {
            *o = *t0 + (*t1 - *t0) * w;
        }
    }
}

/// ring[(idx + k) & 255] -= v[k] * a.
#[inline(never)]
fn ring_sub(ring: &mut [f32; 256], idx: u8, v: &[f32; TAPS], a: f32) {
    let i0 = idx as usize;
    let first = (256 - i0).min(TAPS);
    let (head, tail) = ring.split_at_mut(i0);
    for (r, t) in tail[..first].iter_mut().zip(&v[..first]) {
        *r -= a * *t;
    }
    for (r, t) in head.iter_mut().zip(&v[first..]) {
        *r -= a * *t;
    }
}

/// A sample's events, all into one ring: last first, neighbours within 0.7/128 of the last one share its table values
/// (`osc-dual.md`, Step corrections).
#[inline(never)]
pub fn apply_events(ring: &mut [f32; 256], idx: u8, tab: &Blep, ev: &Events) {
    let mut e = ev.n;
    while e > 0 {
        let fe = ev.f[e - 1];
        let mut m = e - 1;
        while m > 0 && (ev.f[m - 1] - fe).abs() < MERGE {
            m -= 1;
        }
        if e - m == 1 {
            insert_step(ring, idx, tab, clamp01(fe), ev.a[e - 1]);
        } else {
            let mut v = [0.0f32; TAPS];
            step_column(tab, clamp01(fe), &mut v);
            for q in (m..e).rev() {
                ring_sub(ring, idx, &v, ev.a[q]);
            }
        }
        e = m;
    }
}

/// The same with each event in its own ring: merging works across rings.
#[inline(never)]
pub fn apply_events_rings<const R: usize>(rings: &mut [[f32; 256]; R], idx: u8, tab: &Blep, ev: &Events) {
    let mut e = ev.n;
    while e > 0 {
        let fe = ev.f[e - 1];
        let mut m = e - 1;
        while m > 0 && (ev.f[m - 1] - fe).abs() < MERGE {
            m -= 1;
        }
        if e - m == 1 {
            insert_step(&mut rings[ev.r[e - 1] as usize], idx, tab, clamp01(fe), ev.a[e - 1]);
        } else {
            let mut v = [0.0f32; TAPS];
            step_column(tab, clamp01(fe), &mut v);
            for q in (m..e).rev() {
                ring_sub(&mut rings[ev.r[q] as usize], idx, &v, ev.a[q]);
            }
        }
        e = m;
    }
}

/// What the pitch points of a pass read (`osc-dual.md`, Pitch points; the other models' are the same form).
pub struct PitchIn {
    /// Samples per control step.
    pub e: f32,
    pub d_base: [f32; 4],
    pub st_base: [f32; 4],
    pub drift: f32,
    /// Depth of Tune1ModSrc and Tune2ModSrc per lane.
    pub a: [f32; 4],
    pub b: [f32; 4],
}

/// The points' four-lane pitches at j.
#[inline(always)]
fn point_p(x: &OscCtx, p: &PitchIn, j: usize) -> F4 {
    let s = &x.s.osc;
    let mut y = (F4::splat(j as f32) * F4::load(&p.d_base)) * F4::splat(p.e) + (F4::splat(p.drift) + F4::load(&p.st_base));
    y = y + F4::splat(x.row(s.tm1_src, j)) * F4::load(&p.a);
    if let Some(m) = x.mx_pitch {
        y = y + F4::splat(m[j]);
    }
    y = y + F4::splat(s.vibrato * 0.01) * F4::splat(x.row(16, j));
    y = y + F4::splat(x.row(s.tm2_src, j)) * F4::load(&p.b);
    let (k8, k9) = (x.row(8, j), x.row(9, j));
    F4::new(k8, k9, k9, k9) * F4::splat(64.0) + y
}

/// All four lanes equal.
#[inline(always)]
fn all_eq(a: F4, b: F4) -> bool {
    unsafe {
        use core::arch::x86_64::*;
        _mm_movemask_ps(_mm_cmpeq_ps(a.0, b.0)) == 15
    }
}

/// The pitch points of a pass: `target[j]` for the played points, the return value is P(15), kept for the cache. `law` turns
/// pitches into increments; a point equal to the cache takes the current increments. Either way `cap` limits them.
#[inline(always)]
pub fn pitch_points(
    x: &OscCtx,
    p: &PitchIn,
    cache: &[f32; 4],
    cur: &[f32; 4],
    init: bool,
    cap: f32,
    pts: usize,
    law: impl Fn(F4) -> F4,
    target: &mut [[f32; 4]; 16],
) -> [f32; 4] {
    let (cache, cur, cap) = (F4::load(cache), F4::load(cur), F4::splat(cap));
    let mut prev = F4::splat(f32::NAN);
    let mut prev_t = [0.0f32; 4];
    let mut last = F4::zero();
    for j in 0..pts {
        let q = point_p(x, p, j);
        if j == 15 {
            last = q;
        }
        // an equal point has an equal outcome: this saves the 2^x of a steady note
        if !all_eq(q, prev) {
            let incs = if !init && all_eq(q, cache) { cur } else { law(q) };
            prev_t = incs.min(cap).arr();
            prev = q;
        }
        target[j] = prev_t;
    }
    if pts <= 15 {
        last = point_p(x, p, 15);
    }
    last.arr()
}

/// The plain pitch law's lanes: ((2^((p - 69) / 12) * 110) * dt) * ri.
#[inline(always)]
pub fn law_exp(p: F4, dt: f32, ri: f32) -> F4 {
    (((p - F4::splat(69.0)).div(F4::splat(12.0)).exp2() * F4::splat(110.0)) * F4::splat(dt)) * F4::splat(ri)
}

/// Up by 2 on one signal: A(x), then B(x).
#[derive(Clone, Copy)]
pub struct Up2 {
    s: [F4; 3],
}

impl Up2 {
    pub fn new() -> Up2 {
        Up2 { s: [F4::zero(); 3] }
    }
    #[inline(always)]
    pub fn step(&mut self, co: &triple::ResCoefs, u: f32) -> (f32, f32) {
        let y = chain_step(&mut self.s, &co.up, F4::new(u, 0.0, u, 0.0)).arr();
        (y[0], y[2])
    }
}

/// Down by 2 on two parallel signals.
#[derive(Clone, Copy)]
pub struct Down2 {
    s: [F4; 7],
}

impl Down2 {
    pub fn new() -> Down2 {
        Down2 { s: [F4::zero(); 7] }
    }
    /// Pairs (p0, q0), (p1, q1) to one pair.
    #[inline(always)]
    pub fn step(&mut self, co: &triple::ResCoefs, p0: f32, q0: f32, p1: f32, q1: f32) -> (f32, f32) {
        let l = self.s[6].arr();
        let y = chain_step(&mut self.s, &co.down, F4::new(p0, q0, p1, q1)).arr();
        ((l[2] + y[0]) * 0.5, (l[3] + y[1]) * 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One event goes through the same arithmetic alone or as a merged group of one amplitude and one zero.
    #[test]
    fn merged_events_match_a_single_insertion() {
        let t = Tables::get();
        for idx in [0u8, 1, 100, 200, 255] {
            let (mut a, mut b) = ([0.0f32; 256], [0.0f32; 256]);
            insert_step(&mut a, idx, &t.step4, 0.3, 0.7);
            let mut ev = Events::new();
            ev.push(0.3 + MERGE * 0.5, 0.0);
            ev.push(0.3, 0.7);
            apply_events(&mut b, idx, &t.step4, &ev);
            assert_eq!(a, b, "idx {idx}");
        }
    }

    /// Events further apart than the merge width take their own fractions.
    #[test]
    fn far_events_do_not_merge() {
        let t = Tables::get();
        let (mut a, mut b) = ([0.0f32; 256], [0.0f32; 256]);
        insert_step(&mut a, 7, &t.step2, 0.9, 1.0);
        insert_step(&mut a, 7, &t.step2, 0.2, -1.0);
        let mut ev = Events::new();
        ev.push(0.2, -1.0);
        ev.push(0.9, 1.0);
        apply_events(&mut b, 7, &t.step2, &ev);
        assert_eq!(a, b);
    }

    /// Both resamplers pass a constant with gain 1.
    #[test]
    fn resampler_dc_gain() {
        let co = triple::res_coefs();
        let (mut up, mut down) = (Up2::new(), Down2::new());
        let (mut x, mut y) = (0.0, 0.0);
        for _ in 0..4000 {
            let (p, q) = up.step(&co, 0.5);
            (x, y) = down.step(&co, p, p, q, q);
        }
        assert!((x - 0.5f32).abs() < 1e-5 && (y - 0.5f32).abs() < 1e-5, "{x} {y}");
    }
}
