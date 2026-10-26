// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Image-Line's curves as Citrus and Pulp Filter use them: envelopes, LFOs and maps, a list of points joined by shaped segments,
//! with a decay point, a loop and a sustain point. The stream, the preparation, the thirteen segment modes, the
//! walks (with Sytrus's note-off, step points and global envelopes, and Love Philter's starts at a time) and the LFO
//! oscillator.
//
// spec: plugins/image-line-curves.md

use crate::num::{round, s, Expo, Reader};

pub const ENVELOPE: i32 = 1;
pub const LFO: i32 = 2;
pub const MAP: i32 = 3;

/// Flag 0x01: tempo-synced. 0x02: free-running (an LFO), global (an envelope). 0x04: one-sided LFO tension.
/// 0x08: frozen LFO.
pub const SYNCED: u8 = 0x01;
pub const GLOBAL: u8 = 0x02;
pub const ONE_SIDED: u8 = 0x04;
pub const FROZEN: u8 = 0x08;

const W16: f64 = 1.525_878_906_25e-5;
const TINY24: f64 = 5.960_464_477_539_063e-8;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub dist: f32,
    pub level: f32,
    pub tension: f32,
    pub mode: u8,
    pub step: u8,
}

/// A curve as loaded, with what its preparation works out.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub kind: i32,
    pub points: Vec<Point>,
    pub flags: u8,
    pub enabled: bool,
    pub decay: i32,
    pub loop_start: i32,
    pub sustain: i32,
    /// Envelope time scales as stored (1/128): attack, decay, sustain offset, release.
    pub scales: [i32; 4],
    /// LFO: speed, tension, skew, pulse width, phase offset.
    pub lfo: [i32; 5],
    // Preparation.
    pub hold: i32,
    pub loop_empty: bool,
    /// Per point: the segment coefficients and whether the segment is a straight line.
    pub coef: Vec<([f32; 3], bool)>,
    pub weights: Vec<f32>,
    /// The curve counts its step points.
    pub has_steps: bool,
}

impl Curve {
    /// A curve's defaults when the stream does not hold one of the expected kind: one point at level 0 (an LFO:
    /// 0.5), no markers, disabled, an envelope's scales 128, 128, 0, 128.
    pub fn default_of(kind: i32) -> Curve {
        let level = if kind == LFO { 0.5 } else { 0.0 };
        let mut c = Curve {
            kind,
            points: vec![Point { level, ..Point::default() }],
            flags: 0,
            enabled: false,
            decay: -1,
            loop_start: -1,
            sustain: -1,
            scales: [128, 128, 0, 128],
            lfo: [0; 5],
            hold: -1,
            loop_empty: false,
            coef: Vec::new(),
            weights: Vec::new(),
            has_steps: false,
        };
        c.prepare();
        c
    }

    /// Reads one curve of `kind`. A leading kind that is not the expected one is not read further (the next curve
    /// starts after those four bytes) and the defaults stay. `None` when the stream ends.
    pub fn read(r: &mut Reader, kind: i32) -> Option<Curve> {
        let k = r.i32()?;
        if k != kind {
            return Some(Curve::default_of(kind));
        }
        let version = r.i32()?;
        if !(0..=3).contains(&version) {
            return None;
        }
        let n = r.i32()?;
        if !(0..=100_000).contains(&n) {
            return None;
        }
        let mut points = Vec::with_capacity(n as usize);
        let mut prev = 0.0f32;
        for _ in 0..n {
            let mut p = Point::default();
            if version >= 2 {
                p.dist = r.f64()? as f32;
                p.level = r.f64()? as f32;
            } else {
                p.dist = r.f32()?;
                p.level = r.f32()?;
            }
            p.tension = r.f32()?;
            if version >= 1 {
                let b = r.bytes(4)?;
                p.mode = b[0];
                p.step = b[1];
            }
            if p.level.is_nan() {
                p.level = prev;
            }
            prev = p.level;
            points.push(p);
        }
        let flags = r.i32()? as u8;
        let enabled = r.i32()? != 0;
        let decay = r.i32()?;
        let loop_start = r.i32()?;
        let sustain = r.i32()?;
        let mut c = Curve::default_of(kind);
        c.points = points;
        (c.flags, c.enabled, c.decay, c.loop_start, c.sustain) = (flags, enabled, decay, loop_start, sustain);
        if kind == ENVELOPE {
            for v in c.scales.iter_mut() {
                *v = r.i32()?;
            }
        } else if kind == LFO {
            for v in c.lfo[..4].iter_mut() {
                *v = r.i32()?;
            }
            if version >= 3 {
                c.lfo[4] = r.i32()?;
            }
        }
        if c.points.is_empty() {
            c.points.push(Point::default());
        }
        Some(c)
    }

    /// Writes the curve as curve version 3, the layout [`Curve::read`] reads (spec plugins/citrus.md §4).
    pub fn write(&self, w: &mut Vec<u8>) {
        let int = |w: &mut Vec<u8>, v: i32| w.extend_from_slice(&v.to_le_bytes());
        int(w, self.kind);
        int(w, 3);
        int(w, self.points.len() as i32);
        for p in &self.points {
            w.extend_from_slice(&f64::from(p.dist).to_le_bytes());
            w.extend_from_slice(&f64::from(p.level).to_le_bytes());
            w.extend_from_slice(&p.tension.to_le_bytes());
            w.extend_from_slice(&[p.mode, p.step, 0, 0]);
        }
        for v in [i32::from(self.flags), i32::from(self.enabled), self.decay, self.loop_start, self.sustain] {
            int(w, v);
        }
        let tail: &[i32] = match self.kind {
            ENVELOPE => &self.scales,
            LFO => &self.lfo,
            _ => &[],
        };
        for &v in tail {
            int(w, v);
        }
    }

    /// What Sytrus keeps of a curve when it is loaded (spec "What the owner keeps", "Fitting a map"), then the
    /// preparation.
    pub fn keep_sytrus(&mut self) {
        let n = self.points.len() as i32;
        let valid = |m: i32| if (0..n).contains(&m) { m } else { -1 };
        self.decay = valid(self.decay);
        self.loop_start = valid(self.loop_start);
        self.sustain = valid(self.sustain);
        match self.kind {
            LFO => self.decay = -1,
            MAP => {
                (self.decay, self.loop_start, self.sustain) = (-1, -1, -1);
                for p in self.points.iter_mut() {
                    p.step = 0;
                }
                self.fit_map();
            }
            _ => {}
        }
        self.prepare();
    }

    /// What Love Philter keeps of a curve when it is loaded (spec "What the owner keeps"): patterns and LFOs keep the
    /// loop start and the sustain point, maps no marker; no point keeps its step byte. Then the preparation.
    pub fn keep_love_philter(&mut self) {
        let n = self.points.len() as i32;
        let valid = |m: i32| if (0..n).contains(&m) { m } else { -1 };
        self.decay = -1;
        self.loop_start = valid(self.loop_start);
        self.sustain = valid(self.sustain);
        if self.kind == MAP {
            (self.loop_start, self.sustain) = (-1, -1);
        }
        for p in self.points.iter_mut() {
            p.step = 0;
        }
        self.prepare();
    }

    /// Fitting a map to the length 1.
    fn fit_map(&mut self) {
        if self.points[0].dist > 0.0 {
            let first = Point { dist: 0.0, level: self.points[0].level, ..Point::default() };
            self.points.insert(0, first);
        }
        let total: f64 = self.points.iter().map(|p| f64::from(p.dist)).sum();
        if total > 0.0 && total != 1.0 {
            let k = 1.0 / total;
            for p in self.points.iter_mut() {
                p.dist = s(f64::from(p.dist) * k);
            }
        } else if total == 0.0 {
            let last = self.points[self.points.len() - 1].level;
            self.points.push(Point { dist: 1.0, level: last, ..Point::default() });
        }
    }

    /// The preparation (spec "Preparing a curve").
    pub fn prepare(&mut self) {
        for p in self.points.iter_mut() {
            if p.dist < 0.0 {
                p.dist = 0.0;
            }
        }
        let n = self.points.len();
        self.hold = if self.sustain >= 0 {
            self.sustain
        } else if self.loop_start >= 0 {
            n as i32 - 1
        } else {
            -1
        };
        self.loop_empty = false;
        if self.hold >= 0 && self.loop_start >= 0 {
            let to = |m: i32| -> f64 { self.points[..=(m as usize)].iter().map(|p| f64::from(p.dist)).sum() };
            if to(self.hold) - to(self.loop_start) <= 0.0 {
                self.loop_empty = true;
            }
        }
        self.coef = (0..n).map(|i| if i == 0 { ([0.0; 3], true) } else { self.coefficients(i) }).collect();
        self.weights = self.offset_weights();
        self.has_steps = !self.loop_empty
            && self.points.iter().enumerate().any(|(i, p)| p.step != 0 && (self.sustain < 0 || i as i32 <= self.sustain));
    }

    fn coefficients(&self, i: usize) -> ([f32; 3], bool) {
        let p = self.points[i];
        let t = p.tension;
        let zero = t == 0.0;
        let count = || {
            let reach = s(1.0 - f64::from(t.abs()) * 0.6);
            let n = round(f64::from(s((f64::from(reach) * 1_000_001f64.ln()).exp() - 1.0)) * 0.001);
            let side: f32 = if t < 0.0 { -1.0 } else { 1.0 };
            (n, side)
        };
        match p.mode {
            0 | 1 => {
                let v = s((f64::from(t.abs()) * 1_000_001f64.ln()).exp() - 1.0);
                let e = Expo::new(if t < 0.0 { -v } else { v });
                ([e.c0, e.c1, e.c2], zero)
            }
            3 => {
                let (n, side) = count();
                let k = (n + 1 + i64::from(t >= 0.0)) as f32;
                ([k, 1.0 / (k - side), side * 0.5], false)
            }
            4 => {
                let (n, _) = count();
                let c0 = (n + 1) as f32;
                ([c0, 1.0 / c0, 0.0], false)
            }
            5 => {
                let (n, side) = count();
                ([((n + 1) * 2) as f32, 0.0, side * 0.5], false)
            }
            6 => {
                let (n, _) = count();
                ([s(n as f64 + 0.5), 6.283_185_5, 0.0], n == 0 && t > 0.0)
            }
            7 | 8 => ([t, 0.0, 0.0], zero),
            9 => ([t.abs(), 0.0, 0.0], zero),
            10 => self.smooth(i),
            11 | 12 => {
                let a = (f64::from(t.abs()) * 0.9).max(W16);
                let b = 1.0 - a;
                let c0 = s(1.0 - (b * b) * b);
                let mut c1 = (1.0 - c0) / c0;
                let mut c2 = c1;
                if t > 0.0 {
                    c1 = -c1;
                    c2 = c1 - 1.0;
                }
                if p.mode == 12 {
                    c1 *= 0.5;
                    c2 *= 0.5;
                }
                ([c0, c1, c2], zero)
            }
            _ => ([0.0; 3], false),
        }
    }

    /// Mode 10's coefficients, looking at the neighbouring points (and round a loop).
    fn smooth(&self, i: usize) -> ([f32; 3], bool) {
        let n = self.points.len() as i64;
        let (l, h) = (i64::from(self.loop_start), i64::from(self.hold));
        let inside = l >= 0 && h >= 0 && l + 1 < h && (l as usize) < i && i as i64 <= h;
        let fold = |mut k: i64| -> i64 {
            if inside {
                let span = h - l;
                while k < l + 1 {
                    k += span;
                }
                while k > h {
                    k -= span;
                }
            }
            k
        };
        let at = |k: i64| -> Option<Point> { if (0..n).contains(&k) { Some(self.points[k as usize]) } else { None } };
        let me = self.points[i];
        let Some(prev) = at(fold(i as i64 - 1)) else { return ([0.0; 3], true) };
        let (y0, y1) = (f64::from(prev.level), f64::from(me.level));
        let len = f64::from(me.dist).max(TINY24);
        let rise = y1 - y0;
        if rise.abs() < TINY24 {
            return ([0.0; 3], true);
        }
        let (mut before, mut back) = (y0, 1.0);
        if prev.mode == 10 {
            back = f64::from(prev.dist) / len;
            if let Some(pp) = at(fold(i as i64 - 2)) {
                before = f64::from(pp.level);
            }
        }
        let (mut after, mut ahead) = (y1, 1.0);
        if let Some(nx) = at(fold(i as i64 + 1)).filter(|p| p.mode == 10) {
            ahead = f64::from(nx.dist) / len;
            after = f64::from(nx.level);
        }
        let (back, ahead) = (s(back.max(TINY24)), s(ahead.max(TINY24)));
        let s_out = s(((after - y0) / rise - 1.0) / f64::from(ahead));
        let s_in = s(-((before - y0) / rise) / f64::from(back));
        let c0 = if s_out > 0.0 {
            let w = ahead * 2.0 + 1.0;
            s_out * (w + ahead + 2.0) / (w * s_out + (ahead + 2.0))
        } else {
            s_out * (ahead + 2.0) / (ahead + 2.0)
        };
        let c1 = if s_in > 0.0 {
            let w = back * 2.0 + 1.0;
            s_in * (back + 2.0 + w) / ((back + 2.0) + w * s_in)
        } else {
            s_in * (back + 2.0) / (back + 2.0)
        };
        let c2 = s(1.0 - f64::from(me.tension.abs()));
        ([c0, c1, c2], false)
    }

    fn offset_weights(&self) -> Vec<f32> {
        let n = self.points.len();
        let last = n as i32 - 1;
        let a = self.decay.max(0);
        let sp = if self.sustain >= 0 { self.sustain } else { last };
        let d = if self.loop_start >= 0 { self.loop_start } else { sp };
        let sum = |from: i32, to: i32| -> f64 {
            (from..=to).filter(|&k| k >= 0 && k <= last).map(|k| f64::from(self.points[k as usize].dist)).sum()
        };
        let div = |x: f64| if x > W16 { x } else { W16 };
        let big_a = div(sum(0, a));
        let big_d = div(sum(a + 1, d));
        let big_r = div(sum(sp + 1, last));
        let mut left = sum(0, a);
        let mut w = vec![0f32; n];
        for (i, p) in self.points.iter().enumerate() {
            let i = i as i32;
            let dist = f64::from(p.dist);
            if i < a {
                w[i as usize] = s((left - dist) / big_a);
                left -= dist;
            } else if i == a {
                w[i as usize] = 0.0;
                left = 0.0;
                if i == sp {
                    left = sum(sp + 1, last);
                }
            } else if i <= sp {
                left += dist;
                w[i as usize] = if i <= d { s(left / big_d) } else { 1.0 };
                if i == sp {
                    left = sum(sp + 1, last);
                }
            } else {
                w[i as usize] = s((left - dist) / big_r);
                left -= dist;
            }
        }
        w
    }

    /// A point's level for a walk carrying the offset `o`, minus `centre`; `limit` keeps it within 0..1.
    #[inline]
    pub fn level(&self, i: usize, o: f32, centre: f64, limit: bool) -> f64 {
        let y = f64::from(self.points[i].level);
        let v = if o == 0.0 {
            y
        } else {
            let v = y + f64::from(self.weights[i] * o);
            if limit { v.clamp(0.0, 1.0) } else { v }
        };
        v - centre
    }

    /// `progress(x)` of the segment that ends on point `i`, rounded to single.
    pub fn progress(&self, i: usize, x: f64) -> f32 {
        let p = self.points[i];
        let ([c0, c1, c2], straight) = self.coef[i];
        let t = p.tension;
        if straight && matches!(p.mode, 0 | 1 | 6 | 7 | 8 | 9 | 10 | 11 | 12) {
            return s(x);
        }
        let (c0d, c1d, c2d) = (f64::from(c0), f64::from(c1), f64::from(c2));
        let e = Expo { c0, c1, c2 };
        let r = match p.mode {
            0 => e.eval(x),
            1 => {
                if x < 0.5 {
                    e.eval(x * 2.0) * 0.5
                } else {
                    1.0 - e.eval((1.0 - x) * 2.0) * 0.5
                }
            }
            2 => 0.0,
            3 => return (round(x * c0d - c2d) as f32) * c1,
            4 => {
                let a = x * c0d - 0.5;
                let k = round(a - 0.5) as f64;
                let mut d = (a - k) * 2.0 - 1.0;
                if t < 0.0 {
                    let e = 1.0 - d.abs();
                    let m = 1.0 - (e * e) * e;
                    d = if d.is_sign_negative() { -m } else { m };
                } else {
                    d = (d * d) * d;
                }
                (k + d * 0.5 + 1.0) * c1d
            }
            5 => (round(x * c0d - c2d) & 1) as f64,
            6 => {
                if t < 0.0 {
                    (((x * c0d + 0.75) * c1d).sin() + 1.0) * 0.5
                } else {
                    let a = x * c0d - 0.5;
                    ((a - round(a - 0.5) as f64) * 2.0 - 1.0).abs()
                }
            }
            7 => curve2(x, t, c0d),
            8 => {
                if x < 0.5 {
                    curve2(x * 2.0, t, c0d) * 0.5
                } else {
                    1.0 - curve2((1.0 - x) * 2.0, t, c0d) * 0.5
                }
            }
            9 => {
                let q = x * 1.570_796_370_506_286_6;
                let h = if t > 0.0 { q.sin() } else { 1.0 - q.cos() };
                x - (x - h) * c0d
            }
            10 => {
                let x2 = x * x;
                x + ((((3.0 * x2 - (2.0 * x2) * x) + (x2 * (x - 1.0)) * c0d) + ((x * (x - 1.0)) * (x - 1.0)) * c1d) - x) * c2d
            }
            11 => {
                let u = if t < 0.0 { x } else { 1.0 - x };
                c1d / (1.0 - c0d * u) - c2d
            }
            12 => {
                if x < 0.5 {
                    let u = if t < 0.0 { x * 2.0 } else { 1.0 - x * 2.0 };
                    c1d / (1.0 - c0d * u) - c2d
                } else {
                    let a = (1.0 - x) * 2.0;
                    let u = if t < 0.0 { a } else { 1.0 - a };
                    (1.0 - c1d / (1.0 - c0d * u)) + c2d
                }
            }
            _ => 0.0,
        };
        s(r)
    }

    /// The summed distances of points `0..=m`.
    pub fn time_to(&self, m: usize) -> f64 {
        self.points[..=m.min(self.points.len() - 1)].iter().map(|p| f64::from(p.dist)).sum()
    }

    /// True when every point has one level.
    pub fn is_flat(&self) -> bool {
        self.points.iter().all(|p| p.level == self.points[0].level)
    }
}

/// Mode 7's `x - (u^6 - u) * c0`.
#[inline]
fn curve2(x: f64, t: f32, c0: f64) -> f64 {
    let u = if t > 0.0 { 1.0 - x } else { x };
    let u3 = u * u * u;
    x - (u3 * u3 - u) * c0
}

/// Stages of a walk.
pub const ATTACK: u8 = 1;
pub const DECAY: u8 = 3;
pub const WAITING: u8 = 5;
pub const RELEASED: u8 = 6;
pub const FINISHED: u8 = 9;

/// A walk along an envelope (or an LFO's own curve), with Sytrus's per-voice scales, offset, step points and
/// note-off.
#[derive(Debug, Clone, Default)]
pub struct Walk {
    pub i: usize,
    pub p: f64,
    pub r: f64,
    pub from: f32,
    pub by: f32,
    pub o: f32,
    pub stage: u8,
    pub out: f32,
    /// The loop stops at the hold point.
    pub stop_at_hold: bool,
    /// The decay point has been passed once.
    pub passed_decay: bool,
    /// Time scales: attack, decay, sustain offset, release.
    pub scales: [f32; 4],
    /// The walk's step in the curve's unit.
    pub step: f64,
    pub centre: f64,
    /// Levels are kept within 0..1.
    pub limit: bool,
    // Step points.
    pub step_pos: i32,
    pub holding: bool,
    pub held: f32,
    pub passed_step: bool,
    pub rank: i32,
    pub count: i32,
}

impl Walk {
    pub fn new(c: &Curve, step: f64, centre: f64, limit: bool) -> Walk {
        let scales = if c.kind == ENVELOPE { c.scales.map(|v| v as f32 / 128.0) } else { [1.0, 1.0, 0.0, 1.0] };
        let mut w = Walk { step, centre, limit, scales, step_pos: -1, ..Walk::default() };
        w.reset(c, 0.0);
        w
    }

    /// A walk with the scales attack 1, decay 1, sustain offset 0, release 1 whatever the curve stores (Love Philter's
    /// patterns).
    pub fn unscaled(c: &Curve, step: f64, centre: f64) -> Walk {
        let mut w = Walk {
            step,
            centre,
            limit: true,
            scales: [1.0, 1.0, 0.0, 1.0],
            step_pos: -1,
            ..Walk::default()
        };
        w.reset(c, 0.0);
        w
    }

    pub fn finished(&self) -> bool {
        self.stage >= FINISHED
    }

    /// Starts the walk at `s` steps from the curve's beginning (spec "Starting at a time"): reset, the time folded into
    /// the loop and walked in one step; a negative `s` makes the first segment end `-s` steps from now.
    pub fn start_at(&mut self, c: &Curve, s: f64) {
        self.reset(c, 0.0);
        if s < 0.0 {
            if !self.finished() {
                (self.r, self.p) = (-1.0 / s, 0.0);
            }
            return;
        }
        let t = s * self.step;
        let t = match (c.loop_start, c.hold) {
            (l, h) if l >= 0 && h >= 0 => {
                let a = c.time_to(l as usize);
                let len = c.time_to(h as usize) - a;
                if len > 0.0 {
                    let u = t - a;
                    let k = f64::from(self::s(u / len)).trunc();
                    (a + u) - k * len
                } else {
                    t
                }
            }
            _ => t,
        };
        if self.step != 0.0 {
            self.advance(c, t / self.step);
        }
    }

    fn lvl(&self, c: &Curve, i: usize) -> f64 {
        c.level(i, self.o, self.centre, self.limit)
    }

    /// The reset, with an offset the owner gives (portamento, a global envelope).
    pub fn reset(&mut self, c: &Curve, offset: f32) {
        self.passed_decay = false;
        self.holding = false;
        self.passed_step = false;
        self.step_pos = -1;
        if !c.enabled {
            (self.stage, self.from, self.by, self.out, self.p, self.r) = (FINISHED, 0.0, 0.0, 0.0, 0.0, 0.0);
            return;
        }
        self.i = 0;
        self.o = offset;
        self.stage = if c.decay >= 0 { ATTACK } else { DECAY };
        let d = f64::from(c.points[0].dist);
        if d == 0.0 {
            (self.p, self.r) = (1.0, 1.0);
        } else {
            (self.p, self.r) = (0.0, self.step / d);
        }
        self.from = s(self.lvl(c, 0));
        self.by = 0.0;
        self.out = self.from;
        self.stop_at_hold = c.loop_empty;
        if c.kind == ENVELOPE {
            self.rescale(c);
        }
    }

    /// An envelope's offset and output from its scales, as at its reset.
    pub fn rescale(&mut self, c: &Curve) {
        if !c.enabled || self.i != 0 {
            return;
        }
        if self.stage > ATTACK {
            self.o = if c.sustain >= 0 { self.scales[2] } else { 0.0 };
        }
        self.out = s(self.lvl(c, 0));
        self.stop_at_hold = c.loop_empty || self.scales[1] <= 0.0;
    }

    /// One step of `dt` control blocks.
    pub fn advance(&mut self, c: &Curve, dt: f64) {
        if self.stage >= FINISHED {
            return;
        }
        self.p += dt * self.r;
        let last = c.points.len() - 1;
        while self.p >= 1.0 {
            self.p = if self.r == 0.0 { dt } else { (self.p - 1.0) / self.r };
            self.from = s(self.lvl(c, self.i));
            if c.has_steps && c.points[self.i].step != 0 && self.stage < RELEASED {
                self.passed_step = true;
                if self.stage == WAITING {
                    self.release_now(c);
                    self.from = s(self.lvl(c, self.i));
                } else if self.count > 0 {
                    let pos = self.step_pos.max(0) - 2 + i32::from(c.points[self.i].step) + self.count;
                    self.step_pos = pos.rem_euclid(self.count);
                    self.holding = self.step_pos != self.rank;
                    if self.holding {
                        self.held = self.from;
                    }
                }
            }
            let i = self.i as i32;
            let mut stopped = false;
            if c.hold < 0 || i < c.hold || self.stage >= RELEASED {
                if c.decay >= 0 && i == c.decay && !self.passed_decay {
                    self.stage = self.stage.max(DECAY);
                    self.o = self.scales[2];
                    self.passed_decay = true;
                }
                if self.i == last {
                    self.stage = FINISHED;
                    (self.by, self.p, self.r) = (0.0, 0.0, 0.0);
                    self.out = self.from;
                    return;
                }
                self.i += 1;
            } else if c.loop_start < 0 || self.stop_at_hold {
                stopped = true;
            } else {
                self.i = c.loop_start as usize + 1;
                self.passed_decay = true;
            }
            if self.holding {
                self.from = self.held;
                self.by = 0.0;
            } else {
                self.by = s(self.lvl(c, self.i) - f64::from(self.from));
            }
            if stopped {
                (self.p, self.r) = (0.0, 0.0);
                break;
            }
            let scale = match self.stage {
                0..=2 => self.scales[0],
                3..=5 => self.scales[1],
                _ => {
                    if c.sustain >= 0 {
                        self.scales[3]
                    } else {
                        self.scales[1]
                    }
                }
            };
            let length = f64::from(c.points[self.i].dist) * f64::from(scale);
            if length <= 0.0 {
                self.r = 1.0;
                self.p += 1.0;
            } else {
                self.r = self.step / length;
                self.p *= self.r;
            }
        }
        self.out = if self.by == 0.0 { self.from } else { s(f64::from(self.from) + f64::from(c.progress(self.i, self.p) * self.by)) };
    }

    /// Note-off (Sytrus).
    pub fn release(&mut self, c: &Curve) {
        if self.stage >= RELEASED || !c.enabled {
            return;
        }
        if c.has_steps && self.passed_step && !self.holding && self.count >= 2 && self.stage != WAITING {
            if self.find_step(c) {
                self.stage = WAITING;
                return;
            }
        }
        self.release_now(c);
    }

    /// Note-off without the wait for a step.
    fn release_now(&mut self, c: &Curve) {
        if c.sustain >= 0 {
            let sp = c.sustain as usize;
            self.i = sp;
            let lv = c.level(sp, self.o, self.centre, false);
            self.o = s(f64::from(self.o) + (f64::from(self.out) - lv));
            self.p = 1.0;
            self.holding = false;
        }
        self.stage = RELEASED;
        if self.r == 0.0 {
            self.p = 1.0;
        }
    }

    /// The count of keys dropped below 1: a walk waiting for a step is released.
    pub fn keys_gone(&mut self, c: &Curve) {
        if self.stage == WAITING && self.count < 1 {
            self.release_now(c);
        }
    }

    /// Whether a step point lies ahead of the walk (from the sustain point on, round the loop).
    fn find_step(&self, c: &Curve) -> bool {
        let n = c.points.len();
        let mut k = self.i;
        for _ in 0..=n {
            if c.points[k].step != 0 {
                return true;
            }
            k += 1;
            if c.loop_start >= 0 && c.sustain >= 0 && k as i32 > c.sustain {
                k = c.loop_start as usize + 1;
            }
            if k >= n {
                return false;
            }
        }
        false
    }

    /// Velocity skip (switch 9 bit 1): one step of `dt` so that the walk skips into its attack.
    pub fn time_to_decay(&self, c: &Curve) -> f64 {
        if c.decay < 0 { 0.0 } else { c.time_to(c.decay as usize) }
    }
}

/// An LFO: its walk, its corners and its phase on a 32-bit circle.
#[derive(Debug, Clone, Default)]
pub struct Lfo {
    pub walk: Walk,
    pub phase: u32,
    pub value: f32,
    pub shape: LfoShape,
    /// The phase the LFO last started with (a free-running LFO resets to it).
    pub started: u32,
}

/// The LFO's set-up from its four parameters.
#[derive(Debug, Clone, Copy, Default)]
pub struct LfoShape {
    pub e: Expo,
    pub end: u32,
    pub rise: u32,
    pub fall: u32,
    pub fall_slope: f32,
    pub rise_slope: f32,
    /// Phase per walk step, unfrozen.
    pub dphi: i64,
    pub frozen: bool,
    pub one_sided: bool,
    pub start: u32,
}

const TWO32: f64 = 4_294_967_296.0;

impl LfoShape {
    pub fn new(c: &Curve, step: f64) -> LfoShape {
        let [speed, tension, skew, pw, offset] = c.lfo;
        let st = s(f64::from(s(((f64::from(tension.unsigned_abs() as f32) * (1.0 / 128.0)) * 21f64.ln()).exp() - 1.0)) * 0.05);
        let a = s((f64::from(st) * 999_999_984_306_749_440f64.ln()).exp() - 1.0);
        let e = Expo::new(if tension < 0 { -a } else { a });
        let pwf = pw as f32 * (1.0 / 128.0);
        let sk = skew as f32 * (1.0 / 256.0);
        let end: u32 = if pwf <= 0.0 { u32::MAX } else { round(f64::from((1.0 - pwf) * TWO32 as f32 - 1.0)) as u32 };
        let mut r = (f64::from(s((1.0 - f64::from(pwf)) / 2.0)) - f64::from(sk) * (1.0 - f64::from(pwf.abs()))) * TWO32;
        if r > f64::from(end) - 1.0 {
            r = f64::from(end) - 1.0;
        }
        if r < 1.0 {
            r = 1.0;
        }
        let rise = round(r) as u32;
        let fall = if pwf >= 0.0 { rise } else { (round(r + f64::from(s(f64::from(pwf) * TWO32)))).max(1) as u32 };
        let fall_slope = s(2.0 / f64::from(fall));
        let rise_slope = s(2.0 / (f64::from(end) - f64::from(rise)));
        let x = f64::from(s((f64::from(speed as f32 * (1.0 / 65536.0)) * 10001f64.ln()).exp() - 1.0));
        let dphi = round((x * 6_871_947.673_6 * step).min(805_306_368.0));
        let start = (u64::from(rise) + (u64::from(end - rise) / 2) + 1).wrapping_add(offset as u32 as u64) as u32;
        LfoShape {
            e,
            end,
            rise,
            fall,
            fall_slope,
            rise_slope,
            dphi,
            frozen: c.flags & FROZEN != 0,
            one_sided: c.flags & ONE_SIDED != 0,
            start,
        }
    }

    pub fn wave(&self, phase: u32) -> f32 {
        let x = if phase > self.end {
            1.0
        } else if phase < self.fall {
            1.0 - phase as f32 * self.fall_slope
        } else if phase < self.rise {
            -1.0
        } else {
            (phase - self.rise) as f32 * self.rise_slope - 1.0
        };
        if self.e.c0 == 0.0 {
            x
        } else if self.one_sided {
            let u = (x + 1.0) * 0.5;
            s(self.e.eval(f64::from(u)) * 2.0 - 1.0)
        } else {
            let v = s(self.e.eval(f64::from(x.abs())));
            if x < 0.0 { -v } else { v }
        }
    }
}

impl Lfo {
    /// A voice's LFO at note-on: the walk reset, the phase at its start (or the global counter's value).
    pub fn new(c: &Curve, step: f64, phase: Option<u32>) -> Lfo {
        let shape = LfoShape::new(c, step);
        let walk = Walk::new(c, step, 0.5, true);
        let phase = phase.unwrap_or(shape.start);
        Lfo { walk, phase, value: 0.0, shape, started: phase }
    }

    /// The reset: the walk reset, the phase at its start (a free-running LFO's: the phase it last started with), the
    /// value 0.
    pub fn reset(&mut self, c: &Curve) {
        self.walk.reset(c, 0.0);
        self.phase = if c.flags & GLOBAL != 0 { self.started } else { self.shape.start };
        self.value = 0.0;
    }

    /// Starts the LFO at `s` steps (spec "Start, reset and steps"): its walk as an envelope's, the phase at the start
    /// plus `s` steps of the unfrozen phase step, kept as the phase a free-running LFO resets to.
    pub fn start_at(&mut self, c: &Curve, s: f64) {
        self.reset(c);
        self.walk.start_at(c, s);
        self.phase = self.shape.start.wrapping_add(round(s * self.shape.dphi as f64) as u32);
        self.started = self.phase;
        let w = &self.walk;
        if s >= 0.0 && !(w.i == 0 && !w.finished() && w.p < 1.0) {
            self.value = self::s(f64::from(w.out) * f64::from(self.shape.wave(self.phase)));
        }
    }

    pub fn advance(&mut self, c: &Curve, dt: f64) {
        let w = &self.walk;
        let first = |w: &Walk| w.i == 0 && !w.finished() && w.p < 1.0;
        let dt2 = if first(w) { (dt + (w.p - 1.0) / w.r).max(0.0) } else { dt };
        self.walk.advance(c, dt);
        if !first(&self.walk) {
            let d = if self.shape.frozen { 0 } else { round(dt2 * self.shape.dphi as f64) };
            self.phase = self.phase.wrapping_add(d as u32);
            self.value = s(f64::from(self.walk.out) * f64::from(self.shape.wave(self.phase)));
        }
    }
}

/// A map walked from its start in steps of `dx` (spec "Walking a map"); the values after each step.
pub struct MapWalk<'a> {
    c: &'a Curve,
    i: usize,
    p: f64,
    r: f64,
    from: f32,
    by: f32,
    centre: f64,
    pub out: f32,
}

impl<'a> MapWalk<'a> {
    pub fn new(c: &'a Curve, centre: f64) -> MapWalk<'a> {
        let d = f64::from(c.points[0].dist);
        let (p, r) = if d == 0.0 { (1.0, 1.0) } else { (0.0, 1.0 / d) };
        let from = s(c.level(0, 0.0, centre, true));
        MapWalk { c, i: 0, p, r, from, by: 0.0, centre, out: from }
    }

    pub fn advance(&mut self, dt: f64) -> f32 {
        let c = self.c;
        self.p += dt * self.r;
        while self.p >= 1.0 {
            self.p = if self.r == 0.0 { dt } else { (self.p - 1.0) / self.r };
            self.from = s(c.level(self.i, 0.0, self.centre, true));
            let moved = self.i + 1 < c.points.len();
            if moved {
                self.i += 1;
            }
            self.by = s(c.level(self.i, 0.0, self.centre, true) - f64::from(self.from));
            if !moved {
                (self.p, self.r) = (0.0, 0.0);
                break;
            }
            let d = f64::from(c.points[self.i].dist);
            if d > 0.0 {
                self.r = 1.0 / d;
                self.p *= self.r;
            } else {
                self.r = 1.0;
                self.p += 1.0;
            }
        }
        self.out = s(f64::from(self.from) + f64::from(c.progress(self.i, self.p) * self.by));
        self.out
    }
}

/// A map as a table (spec "A map as a table of 1025 entries"): entry 0 the value at the start, then one step of `dx`
/// per entry, `n + 1` entries.
pub fn map_table(c: &Curve, centre: f64, dx: f64, n: usize) -> Vec<f32> {
    let mut w = MapWalk::new(c, centre);
    let mut t = Vec::with_capacity(n + 1);
    t.push(w.out);
    for _ in 0..n {
        t.push(w.advance(dx));
    }
    t
}

/// A map's value at `x` (key, velocity, unison and random maps, read at note-on; spec plugins/sytrus.md "Maps").
pub fn map_at(c: &Curve, x: f64, centre: f64) -> f32 {
    if c.is_flat() {
        return s(c.level(0, 0.0, centre, true));
    }
    if c.points.len() == 2 && matches!(c.points[1].mode, 0 | 1 | 7 | 8 | 9 | 11 | 12) && c.points[1].tension.to_bits() == 0 {
        let t0 = s(c.level(0, 0.0, centre, true));
        let t1 = s(c.level(1, 0.0, centre, true));
        let x = x.clamp(0.0, 1.0);
        return s(f64::from(t0) * (1.0 - x) + f64::from(t1) * x);
    }
    let mut w = MapWalk::new(c, centre);
    w.advance(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(points: &[(f32, f32, u8, f32)], decay: i32, sustain: i32) -> Curve {
        let mut c = Curve::default_of(ENVELOPE);
        c.points = points.iter().map(|&(dist, level, mode, tension)| Point { dist, level, tension, mode, step: 0 }).collect();
        (c.enabled, c.decay, c.sustain) = (true, decay, sustain);
        c.keep_sytrus();
        c
    }

    #[test]
    fn a_straight_attack_rises_by_its_step() {
        let c = env(&[(0.0, 0.0, 0, 0.0), (1.0, 1.0, 0, 0.0)], -1, -1);
        let mut w = Walk::new(&c, 0.25, 0.0, true);
        w.advance(&c, 0.0);
        assert_eq!(w.out, 0.0);
        w.advance(&c, 1.0);
        assert_eq!(w.out, 0.25);
        w.advance(&c, 1.0);
        assert_eq!(w.out, 0.5);
        for _ in 0..3 {
            w.advance(&c, 1.0);
        }
        assert!(w.finished());
        assert_eq!(w.out, 1.0);
    }

    #[test]
    fn a_sustain_point_holds_until_note_off() {
        let c = env(&[(0.0, 0.0, 0, 0.0), (0.5, 1.0, 0, 0.0), (0.5, 0.0, 0, 0.0)], -1, 1);
        let mut w = Walk::new(&c, 0.25, 0.0, true);
        for _ in 0..10 {
            w.advance(&c, 1.0);
        }
        assert_eq!(w.out, 1.0);
        assert!(!w.finished());
        w.release(&c);
        w.advance(&c, 1.0);
        assert_eq!(w.out, 0.5);
        w.advance(&c, 1.0);
        w.advance(&c, 1.0);
        assert!(w.finished());
    }

    #[test]
    fn the_lfo_starts_at_the_rising_zero() {
        let mut c = Curve::default_of(LFO);
        c.points[0].level = 1.0;
        c.lfo = [32768, 0, 0, 0, 0];
        c.enabled = true;
        c.keep_sytrus();
        let sh = LfoShape::new(&c, 1.0);
        assert!(sh.wave(sh.start).abs() < 1e-6);
        assert!(sh.wave(0) > 0.99);
    }
}
