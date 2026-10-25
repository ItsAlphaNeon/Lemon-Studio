// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A filter module of a voice: its volume, one of thirteen filters in up to three (spread) stages, and its wave
//! shaper. Works a stretch at a time.
//
// spec: plugins/sytrus.md ("Filter modules", "Coefficients", "Per sample", "Wave shaper")

use std::f64::consts::PI;

use crate::curve::{Curve, MapWalk};
use crate::num::{curve, flush, flush_le, round, s, TINY};

/// `single(exp(x * ln base) - 1) * scale`.
#[inline]
fn e(x: f64, base: f64, scale: f64) -> f64 {
    f64::from(s((x * base.ln()).exp() - 1.0)) * scale
}

/// A gain gliding over a stretch by the conventions' rule.
#[derive(Debug, Clone, Copy, Default)]
pub struct Glide {
    pub value: f32,
}

/// The ramp a glide makes for one stretch: the start, the step and where it ends.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    pub start: f32,
    pub step: f32,
    pub end: f32,
}

impl Glide {
    pub fn ramp(&mut self, target: f32, n: usize, limit: f32) -> Ramp {
        let cur = self.value;
        if n == 0 {
            return Ramp { start: cur, step: 0.0, end: cur };
        }
        let nf = n as f32;
        let mut step = (target - cur) / nf;
        let end;
        if step.abs() <= limit {
            end = target;
        } else {
            step = if target > cur { limit } else { -limit };
            let mut v = cur + step * nf;
            if v.abs() < TINY {
                v = 0.0;
                step = -cur / nf;
            }
            end = v;
        }
        self.value = end;
        Ramp { start: cur, step, end }
    }
}

/// Applies a running gain to `x` sample by sample: four lanes (`lanes`) or one running sum.
pub fn running_gain(x: &mut [f32], r: Ramp, lanes: bool) {
    if r.step == 0.0 {
        if r.start != 1.0 {
            for v in x.iter_mut() {
                *v *= r.start;
            }
        }
        return;
    }
    let n = x.len();
    let mut g = r.start;
    let mut j0 = 0;
    if lanes {
        let s4 = 4.0 * r.step;
        let mut lane = [r.start, r.start + r.step, r.start + 2.0 * r.step, r.start + 0.75 * s4];
        let vec = 16 * (n / 16);
        for j in 0..vec {
            x[j] *= lane[j % 4];
            lane[j % 4] += s4;
        }
        g = lane[0];
        j0 = vec;
    }
    for v in x[j0..].iter_mut() {
        *v *= g;
        g += r.step;
    }
}

/// The two state-variable tables (for `kf` and `kf / 2`).
#[derive(Debug, Clone)]
pub struct SvfTables {
    pub kf: f32,
    pub full: Vec<f32>,
    pub half: Vec<f32>,
}

impl SvfTables {
    pub fn new(kf: f32) -> SvfTables {
        let make = |kf: f32| {
            let mut t = Vec::with_capacity(4096);
            for i in 0..4096 {
                let x = s(((f64::from(i as f32 * (1.0 / 2048.0))) + 0.01) * 0.5).min(1.0);
                let x = f64::from(x);
                let a = s(x.atan2(f64::from(s((x + 1.0) * (1.0 - x))).sqrt()));
                let v = s(2.0 * (f64::from(a) * f64::from(kf)).sin());
                if v >= 1.175 {
                    t.push(1.175);
                    break;
                }
                t.push(v);
            }
            t
        };
        SvfTables { kf, full: make(kf), half: make(kf * 0.5) }
    }
}

/// A shaper table: 16-bit entries with a guard entry at each end.
#[derive(Debug, Clone)]
pub struct Shaper {
    pub t: Vec<i32>,
    pub bipolar: bool,
}

impl Shaper {
    pub fn new(c: &Curve, bipolar: bool) -> Shaper {
        let n: usize = if bipolar { 32768 } else { 16384 };
        let centre = if bipolar { 0.5 } else { 0.0 };
        let scale = if bipolar { 65535.0 } else { 32767.0 };
        let mut w = MapWalk::new(c, centre);
        let step = f64::from(s(1.0 / (n as f64 - 1.0)));
        let mut t = Vec::with_capacity(n + 2);
        t.push(0);
        for k in 0..n {
            let v = if k == 0 { w.out } else { w.advance(step) };
            t.push((round(f64::from(v) * scale)).clamp(-32768, 32767) as i32);
        }
        t[0] = t[1];
        t.push(t[n]);
        Shaper { t, bipolar }
    }

    /// The shaped value of `x` at pre-amp `pre`.
    #[inline]
    fn apply(&self, x: f32, pre: f64) -> f32 {
        let n = (self.t.len() - 2) as i64;
        let top = (n - 1) as f64;
        let g = |k: i64| f64::from(self.t[(k + 1) as usize]);
        let v = if self.bipolar {
            let f = f64::from(x) * (pre * top * 0.5) + top * 0.5;
            let i = (f.trunc() as i64).clamp(-1, n - 1);
            let frac = f - i as f64;
            g(i) + (g(i + 1) - g(i)) * frac
        } else {
            let f = f64::from(x) * (pre * top);
            let i = f.trunc() as i64;
            let frac = f - i as f64;
            if i >= 0 {
                let j = i.min(n - 1);
                g(j) + (g(j + 1) - g(j)) * frac
            } else {
                let j = (-i).min(n - 1);
                -(g(j) - (g(j + 1) - g(j)) * frac)
            }
        };
        s(v * (1.0 / 32768.0))
    }
}

/// What a filter module's articulators and header give one stretch.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inputs {
    /// `header 2 / 256 + cutoff value`, `header 3 / 256 + resonance value`.
    pub c: f32,
    pub r: f32,
    pub volume: f32,
    pub low: f32,
    pub band: f32,
    pub high: f32,
    pub shaper_mix: f32,
}

/// One stage's memory.
#[derive(Debug, Clone, Copy, Default)]
struct Stage {
    m: [f64; 4],
    /// Glided `f` and `q` (types 0, 1, 2).
    f: Glide,
    q: Glide,
    /// Ladder feedback value / phaser `y`.
    y: f64,
    /// Ladder poles.
    p: [f32; 4],
}

/// A filter module of one voice.
#[derive(Debug, Clone)]
pub struct FilterRun {
    pub kind: u8,
    stages: usize,
    spread: Option<f64>,
    oversample: bool,
    track: bool,
    on: bool,
    header: [i32; 10],
    st: Vec<Stage>,
    mix: [Glide; 3],
    volume: Glide,
    shaper_mix: Glide,
    /// Phaser pairs' states.
    pairs: Vec<[f32; 2]>,
    first: bool,
    buf: Vec<f32>,
}

impl FilterRun {
    pub fn new(header: [i32; 10], kind: u8, oversample_flag: bool, track: bool, rate: u32, volume: f32) -> FilterRun {
        let h0 = header[0];
        let mut n = (h0 - 1).max(0) as usize;
        let mut spread = None;
        if kind != 12 && n > 2 {
            n -= 2;
            spread = Some(if n == 1 { 0.5 } else { 0.75 });
        }
        let oversample = (oversample_flag && rate <= 176_399) || (kind == 11 && rate <= 88_199);
        let pairs = if kind == 12 { n + 1 + if track { 5 } else { 0 } } else { 0 };
        FilterRun {
            kind,
            stages: n + 1,
            spread,
            oversample,
            track,
            on: h0 != 0,
            header,
            st: vec![Stage::default(); n + 1],
            mix: [Glide::default(); 3],
            volume: Glide { value: volume },
            shaper_mix: Glide::default(),
            pairs: vec![[0.0; 2]; pairs],
            first: true,
            buf: Vec::new(),
        }
    }

    /// The mapped cutoff per stage.
    fn cutoffs(&self, cm: f64) -> Vec<f64> {
        let mut v = vec![cm; self.stages];
        if let Some(p) = self.spread {
            for k in (0..self.stages.saturating_sub(1)).rev() {
                v[k] = v[k + 1].powf(p);
            }
        }
        v
    }

    /// Runs one stretch in place. `kf` = `single(k44)`, `ramp_a` the articulators' glide limit, `svf` the tables.
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, x: &mut [f32], inp: Inputs, kf: f32, ramp_a: f32, svf: &SvfTables, shaper: Option<&Shaper>, shaper_on: bool) {
        let n = x.len();
        // 1. Volume.
        let vr = self.volume.ramp(inp.volume, n, ramp_a);
        running_gain(x, vr, true);
        // 2. Filter.
        if self.on {
            if self.oversample {
                let mut up = std::mem::take(&mut self.buf);
                up.clear();
                for &v in x.iter() {
                    up.push(v);
                    up.push(v);
                }
                self.filter(&mut up, inp, kf * 0.5, ramp_a, svf, true);
                for (k, o) in x.iter_mut().enumerate() {
                    *o = s((f64::from(up[2 * k]) + f64::from(up[2 * k + 1])) * 0.5);
                }
                self.buf = up;
            } else {
                self.filter(x, inp, kf, ramp_a, svf, false);
            }
        }
        // 3. Wave shaper.
        if shaper_on {
            if let Some(sh) = shaper {
                let m = (inp.shaper_mix * (self.header[8] as f32 * (1.0 / 128.0))).clamp(0.0, 1.0);
                if self.first {
                    self.shaper_mix.value = m;
                }
                let r = self.shaper_mix.ramp(m, n, ramp_a);
                let pre = f64::from(self.header[7] as f32 * (1.0 / 128.0));
                if r.step == 0.0 && r.start == 1.0 {
                    for v in x.iter_mut() {
                        *v = sh.apply(*v, pre);
                    }
                } else if !(r.step == 0.0 && r.start == 0.0) {
                    let shaped: Vec<f32> = x.iter().map(|&v| sh.apply(v, pre)).collect();
                    running_gain(x, Ramp { start: 1.0 - r.start, step: -r.step, end: 1.0 - r.end }, true);
                    let mut g = r.start;
                    for (o, v) in x.iter_mut().zip(shaped) {
                        *o += v * g;
                        g += r.step;
                    }
                }
            }
        }
        self.first = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn filter(&mut self, x: &mut [f32], inp: Inputs, kf: f32, ramp_a: f32, svf: &SvfTables, over: bool) {
        let kind = self.kind;
        let c = f64::from(inp.c);
        let mut r = f64::from(inp.r);
        if self.track && (1..=10).contains(&kind) {
            r *= (c + 1.0) * 0.5;
        }
        let nst = self.stages;
        let sn = nst as f64;
        let roots = |v: f64| if nst > 1 { v.powf(1.0 / sn) } else { v };
        let kfd = f64::from(kf);
        let n = x.len();
        let limit = ramp_a;
        let first = self.first;
        match kind {
            0 => {
                let cm = if c <= 0.0 { 0.0 } else { e(c, 26.0, f64::from(0.0396f32)) };
                let mut q = if r <= 0.0 {
                    1.0
                } else if r >= 1.0 {
                    0.2
                } else {
                    1.0 - r * f64::from(0.8f32)
                };
                if self.track {
                    q *= q;
                }
                let table = if over { &svf.half } else { &svf.full };
                let lv = |hv: i32, art: f32| -> f32 {
                    let v = curve(hv as f32 * (1.0 / 128.0)) * art;
                    if nst > 1 {
                        if v > 0.0 { s(f64::from(v).powf(1.0 / sn)) } else { 0.0 }
                    } else {
                        v
                    }
                };
                let mixes = [lv(self.header[4], inp.low), lv(self.header[5], inp.band), lv(self.header[6], inp.high)];
                let mut mr = [Ramp { start: 0.0, step: 0.0, end: 0.0 }; 3];
                for k in 0..3 {
                    if first {
                        self.mix[k].value = mixes[k];
                    }
                    mr[k] = self.mix[k].ramp(mixes[k], n, limit);
                }
                let cuts = self.cutoffs(cm);
                for (k, stage) in self.st.iter_mut().enumerate() {
                    let idx = (round(cuts[k] * 2048.0).max(0) as usize).min(table.len() - 1);
                    let ft = table[idx];
                    let qt = if k == 0 { s(q) } else { 1.0 };
                    if first {
                        stage.f.value = ft;
                        stage.q.value = qt;
                    }
                    let fr = stage.f.ramp(ft, n, limit);
                    let mut qr = if k == 0 { stage.q.ramp(qt, n, limit) } else { Ramp { start: 1.0, step: 0.0, end: 1.0 } };
                    if self.track {
                        let top = fr.start.max(fr.end);
                        qr.start = if k == 0 { (qr.start * kf) / top } else { kf / top };
                    }
                    let moving = fr.step != 0.0 || qr.step != 0.0;
                    let (mut f, mut qq) = (fr.start, qr.start);
                    let (mut low, mut band) = (stage.m[0], stage.m[1]);
                    let mut lane = [mr[0].start, mr[0].start + mr[0].step, mr[0].start + 2.0 * mr[0].step, mr[0].start + 0.75 * (4.0 * mr[0].step)];
                    let s4 = 4.0 * mr[0].step;
                    let vec = 16 * (n / 16);
                    let (mut gl, mut gb, mut gh) = (mr[0].start, mr[1].start, mr[2].start);
                    for j in 0..n {
                        let xi = f64::from(x[j]);
                        low += band * f64::from(f);
                        let ol = s(low);
                        let h = xi - (band * f64::from(qq) + low);
                        let oh = s(h);
                        band += h * f64::from(f);
                        let ob = s(band);
                        let g0 = if mr[0].step == 0.0 {
                            mr[0].start
                        } else if j < vec {
                            let g = lane[j % 4];
                            lane[j % 4] += s4;
                            if j + 1 == vec {
                                gl = lane[0];
                            }
                            g
                        } else {
                            let g = gl;
                            gl += mr[0].step;
                            g
                        };
                        x[j] = ol * g0 + ob * gb + oh * gh;
                        gb += mr[1].step;
                        gh += mr[2].step;
                        if moving {
                            f += fr.step;
                            qq += qr.step;
                        }
                    }
                    stage.m[0] = flush_le(low);
                    stage.m[1] = flush_le(band);
                }
            }
            1 | 2 => {
                let cm = if c <= 0.0 { 0.0 } else { e(c, 76.0, 0.01324).min(1.6) };
                let rm = if r <= 0.0 {
                    1.0
                } else if r >= 1.0 {
                    50.0
                } else {
                    e(r, 1001.0, 0.049) + 1.0
                };
                let rm = roots(rm);
                let cuts = self.cutoffs(cm);
                let sr2 = f64::from(s(2f64.sqrt()));
                for (k, stage) in self.st.iter_mut().enumerate() {
                    let w = (cuts[k] + 0.007) * kfd * PI / 2.0;
                    let cw = w.min(0.4 * PI).cos();
                    let ft = s(2.0 - 2.0 * cw);
                    let d = cw - 1.0;
                    let qt = s((-(d * d * d)).sqrt() * sr2 / (rm * d) + 1.0).max(0.1);
                    if first {
                        stage.f.value = ft;
                        stage.q.value = qt;
                    }
                    let nn = n;
                    let fr = stage.f.ramp(ft, nn, limit);
                    let qr = stage.q.ramp(qt, nn, limit);
                    let (mut f, mut q) = (fr.start, qr.start);
                    let (mut y, mut v) = (stage.m[0], stage.m[1]);
                    for o in x.iter_mut() {
                        let xi = f64::from(*o);
                        v += (xi - y) * f64::from(f);
                        y += v;
                        *o = if kind == 1 { s(y) } else { s(xi - y) };
                        f += fr.step;
                        v *= f64::from(q);
                        q += qr.step;
                    }
                    stage.m[0] = flush_le(y);
                    stage.m[1] = flush_le(v);
                }
            }
            3..=10 => {
                let cm = if c <= 0.0 {
                    0.0
                } else {
                    match kind {
                        3 => e(c, 1501.0, f64::from(0.000_664_67f32)),
                        6 => e(c, 1501.0, f64::from(0.000_399_93f32)).min(1.0001).min(0.6001),
                        _ => e(c, 1501.0, f64::from(0.000_399_93f32)).min(1.0001),
                    }
                };
                let res3 = || roots(if r <= 0.0 { 0.1 } else if r >= 1.0 { 50.0 } else { e(r, 2001.0, f64::from(0.02495f32)) + 0.1 });
                let rm = match kind {
                    3 | 6 | 7 => res3(),
                    4 | 5 => {
                        if r <= 0.0 {
                            0.01
                        } else if r >= 1.0 {
                            12.0
                        } else {
                            e(r, 101.0, f64::from(0.1199f32)) + 0.01
                        }
                    }
                    8 | 10 => {
                        let u = 1.0 - r;
                        if u <= 0.0 {
                            0.1
                        } else if u >= 1.0 {
                            1.0
                        } else {
                            e(u, 3.0, f64::from(0.45f32)) + 0.1
                        }
                    }
                    _ => {
                        let u = 1.0 - r;
                        if u <= 0.0 {
                            0.1
                        } else if u >= 1.0 {
                            100.0
                        } else {
                            e(u, 2001.0, f64::from(0.04995f32)) + 0.1
                        }
                    }
                };
                let a = if matches!(kind, 8..=10) {
                    let xg = s(f64::from(s(f64::from(self.header[4]) * f64::from(inp.low))) * 0.015625);
                    let a = s(f64::from(s((f64::from(xg) * 8f64.ln()).exp() - 1.0)) * 0.125 + 0.125);
                    roots(f64::from(a))
                } else {
                    1.0
                };
                let cuts = self.cutoffs(cm);
                let o = if kind == 3 { 0.003 } else { 0.0001 };
                let h = f64::from(self.header[0] as f32);
                let bwh = if kind == 5 { f64::from(s(1.0 / h)) } else { h };
                let bw = f64::from(s(f64::from(bwh as f32) * 2f64.ln() / 2.0));
                for (k, stage) in self.st.iter_mut().enumerate() {
                    let w = ((cuts[k] + o) * kfd * 2.0 * PI).clamp(0.0, 2.849_517_146_113_191);
                    let (sw, cs) = (w.sin(), w.cos());
                    let alpha = match kind {
                        3 | 6 | 7 => sw * (0.5 / rm),
                        4 | 5 => sw * ((rm * bw * w / sw).sinh()),
                        9 => sw / (rm * 2.0),
                        _ => a.sqrt() * ((a + 1.0 / a) * (1.0 / rm - 1.0) + 2.0).sqrt(),
                    };
                    let ee = (a - 1.0) * cs;
                    let coef = match kind {
                        3 => {
                            let d = 1.0 / (alpha + 1.0);
                            let b1 = d * (1.0 - cs);
                            let b0 = b1 * 0.5;
                            [b0, b1, b0, d * cs * (-2.0), d * (1.0 - alpha)]
                        }
                        4 => {
                            let d = 1.0 / (alpha + 1.0);
                            [d * alpha, 0.0, d * (-alpha), d * cs * (-2.0), d * (1.0 - alpha)]
                        }
                        5 => {
                            let d = 1.0 / (alpha + 1.0);
                            let a1 = d * cs * (-2.0);
                            [d, a1, d, a1, d * (1.0 - alpha)]
                        }
                        6 => {
                            let d = 1.0 / (alpha + 1.0);
                            let b1 = d * (-(cs + 1.0));
                            let b0 = -b1 * 0.5;
                            [b0, b1, b0, d * cs * (-2.0), d * (1.0 - alpha)]
                        }
                        7 => {
                            let d = 1.0 / (alpha + 1.0);
                            let (a1, a2) = (d * cs * (-2.0), d * (1.0 - alpha));
                            [a2, a1, 1.0, a1, a2]
                        }
                        8 => {
                            let al = alpha * sw;
                            let d = 1.0 / (a + 1.0 + ee + al);
                            [
                                d * a * (((a + 1.0) - ee) + al),
                                d * a * 2.0 * ((a - 1.0) - (a + 1.0) * cs),
                                d * a * (((a + 1.0) - ee) - al),
                                d * ((a - 1.0) + (a + 1.0) * cs) * (-2.0),
                                d * ((a + 1.0 + ee) - al),
                            ]
                        }
                        9 => {
                            let d = 1.0 / (alpha / a + 1.0);
                            let a1 = d * cs * (-2.0);
                            [d * (alpha * a + 1.0), a1, d * (1.0 - alpha * a), a1, d * (1.0 - alpha / a)]
                        }
                        _ => {
                            let al = alpha * sw;
                            let d = 1.0 / (((a + 1.0) - ee) + al);
                            [
                                d * a * (a + 1.0 + ee + al),
                                d * a * (-2.0) * ((a - 1.0) + (a + 1.0) * cs),
                                d * a * ((a + 1.0 + ee) - al),
                                d * ((a - 1.0) - (a + 1.0) * cs) * 2.0,
                                d * (((a + 1.0) - ee) - al),
                            ]
                        }
                    };
                    let [b0, b1, b2, a1, a2] = coef;
                    let m = &mut stage.m;
                    for v in x.iter_mut() {
                        let xi = f64::from(*v);
                        let y = xi * b0 + (((m[0] * b1 - m[3] * a2) + m[1] * b2) - m[2] * a1);
                        *m = [xi, m[0], y, m[2]];
                        *v = s(y);
                    }
                    for v in m.iter_mut() {
                        if v.abs() < f64::from(TINY) {
                            *v = 0.0;
                        }
                    }
                }
            }
            11 => {
                let cm = if c <= 0.0 { 0.0 } else { e(c, 201.0, f64::from(0.01495f32)).min(3.0) };
                let g4 = (self.header[4] as f32 / 128.0) * inp.low;
                let drive = e(f64::from(g4), 251.0, 0.12);
                let id = if drive >= 0.001 { 1.0 / drive } else { 0.0 };
                let bracket = f64::from(s(drive)).min(1.0) * 0.6 + 1.9;
                let mut q = if r <= 0.0 {
                    0.0
                } else if r >= 1.0 {
                    bracket
                } else {
                    r * bracket
                };
                if nst > 1 {
                    q = (q + 1.0).powf(1.0 / ((nst - 1) as f64 / 4.0 + 1.0)) - 1.0;
                }
                let cuts = self.cutoffs(cm);
                for (k, stage) in self.st.iter_mut().enumerate() {
                    let w = ((cuts[k] + 0.01) * kfd * 0.5).min(0.65);
                    let w2 = w * w;
                    let mut kk = 1.0 - (-PI * (((w * 1.873 * w2 + w2 * 0.4955) - w * 0.649) + 0.9988) * w).exp();
                    let mut fb = f64::from(s(-q * (w2 * -3.9364 + w * 1.8409 + 0.9968)));
                    let (dd, gain);
                    if id != 0.0 {
                        kk *= id;
                        dd = drive * 2.0;
                        let t = f64::from(s((id * 4.0).tanh()));
                        if self.track {
                            fb *= t;
                        }
                        gain = f64::from(s(f64::from(s(f64::from(s(1.0 / t)) * 0.5)) * (1.0 - fb)));
                    } else {
                        dd = 0.0;
                        gain = f64::from(s((1.0 - fb) * 0.5));
                    }
                    let sat = |v: f64| {
                        let a = v.abs();
                        let p = (3.0 + a) * a + 6.0;
                        v * p / (a * p + 12.0)
                    };
                    let mut y = stage.y;
                    let mut p = stage.p;
                    for o in x.iter_mut() {
                        let xi = f64::from(*o);
                        let s4 = f64::from(p[3]);
                        if dd == 0.0 {
                            let mut u = y * fb + xi;
                            for pole in p.iter_mut() {
                                let sv = f64::from(*pole);
                                let v = sv + (u - sv) * kk;
                                *pole = s(v);
                                u = v;
                            }
                            y = s4 + u;
                        } else {
                            let mut u = (y * fb + xi) * dd;
                            let mut last = 0.0;
                            for pole in p.iter_mut() {
                                let sv = f64::from(*pole);
                                let v = (sat(u) - sat(sv * dd)) * kk + sv;
                                *pole = s(v);
                                u = v * dd;
                                last = v;
                            }
                            y = s4 + last;
                        }
                        *o = s(gain * y);
                    }
                    for pole in p.iter_mut() {
                        *pole = flush(*pole);
                    }
                    stage.p = p;
                    stage.y = f64::from(s(y));
                }
            }
            _ => {
                let cm = if c <= 0.0 { 0.0 } else { e(c, 151.0, f64::from(0.006666f32)) };
                let fb = if r <= 0.0 {
                    0.0
                } else if r >= 1.0 {
                    0.9
                } else {
                    f64::from(s(f64::from(s((f64::from(s(r * 10.0)) + 1.0).ln() / 11f64.ln())) * 0.9))
                };
                let w = ((cm + 0.0001) * kfd).min(1.0);
                let coef = f64::from(flush(s((1.0 - w) / (w + 1.0))));
                let stage = &mut self.st[0];
                let mut y = stage.y;
                for o in x.iter_mut() {
                    y = y * fb + f64::from(*o);
                    for pr in self.pairs.iter_mut() {
                        let a = y * (-coef) + f64::from(pr[0]);
                        pr[0] = s(y + a * coef);
                        y = a * (-coef) + f64::from(pr[1]);
                        pr[1] = s(a + y * coef);
                    }
                    *o = s(y);
                }
                stage.y = f64::from(flush(s(y)));
                for pr in self.pairs.iter_mut() {
                    pr[0] = flush(pr[0]);
                    pr[1] = flush(pr[1]);
                }
            }
        }
    }
}
