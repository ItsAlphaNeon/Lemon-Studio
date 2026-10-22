// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The effects section between the voices and the output: decimators, equaliser, chorus, three delays, the plate
//! reverb and the DC offset removal. Buses are two channels of singles.
//
// spec: plugins/sytrus-effects.md

use std::f64::consts::PI;

use os_dsp::mixer::TrackEq;
use os_engine::effects::reverb::{lowpass, Geometry};

use crate::num::{curve, round, s, trunc, TINY};
use crate::state::State;

/// The largest host block the lines are sized for (FL's own is not measured; the spec assumes 2048).
pub const HOST_BLOCK: usize = 2048;

pub type Bus = [Vec<f32>; 2];

fn ln(x: f64) -> f64 {
    x.ln()
}

// ---------------------------------------------------------------------------------------------------------------
// Decimators

const CA: [f64; 3] = [0.1473771136010466, 0.48246854276970014, 0.8830050257693731];
const CB: [f64; 3] = [0.039151597734460045, 0.3026468483284934, 0.6746159185469639];

/// One chain of three first-order all-passes: the previous input of the first, and every all-pass's previous output.
#[derive(Debug, Clone, Copy, Default)]
struct Branch {
    x: f64,
    y: [f64; 3],
}

impl Branch {
    #[inline]
    fn run(&mut self, x: f64, c: &[f64; 3]) -> f64 {
        let mut x = x;
        let mut xp = self.x;
        self.x = x;
        for k in 0..3 {
            let yp = self.y[k];
            let y = xp + (x - yp) * c[k];
            xp = yp;
            self.y[k] = y;
            x = y;
        }
        x
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Stage {
    a: [Branch; 2],
    b: [Branch; 2],
}

impl Stage {
    fn quiet(&self) -> bool {
        self.a.iter().chain(&self.b).all(|br| br.y.iter().all(|&y| y == 0.0))
    }

    /// Halves `n` frames of both channels in place.
    fn run(&mut self, bus: &mut Bus, n: usize) {
        for ch in 0..2 {
            let x = &mut bus[ch];
            for i in 0..n / 2 {
                let ya = self.a[ch].run(f64::from(x[2 * i]), &CA);
                let yb = self.b[ch].run(f64::from(x[2 * i + 1]), &CB);
                x[i] = s((yb + ya) * 0.5);
            }
            for br in [&mut self.a[ch], &mut self.b[ch]] {
                for y in br.y.iter_mut() {
                    if y.abs() < f64::from(TINY) {
                        *y = 0.0;
                    }
                }
            }
        }
    }
}

/// A chain of half-band stages; stage `k` takes `2^(k+1)` times the host rate down to `2^k`.
#[derive(Debug, Clone, Default)]
pub struct Decimator {
    stages: Vec<Stage>,
}

impl Decimator {
    pub fn new(stages: usize) -> Decimator {
        Decimator { stages: vec![Stage::default(); stages] }
    }

    /// Runs the stages `from - 1` down to `to` on `n` frames; returns the frames left.
    pub fn run(&mut self, bus: &mut Bus, n: usize, from: usize, to: usize) -> usize {
        if from <= to {
            return n;
        }
        let out = n >> (from - to);
        let quiet = bus.iter().all(|c| c[..n].iter().all(|x| x.abs() <= 1e-6)) && self.stages[to..from].iter().all(Stage::quiet);
        if quiet {
            return out;
        }
        let mut m = n;
        for k in (to..from).rev() {
            self.stages[k].run(bus, m);
            m /= 2;
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Chorus

const T: [f32; 20] = [
    0.967, 1.039, 1.021, 0.1013, 0.977, 1.033, 0.997, 0.971, 1.061, 0.947, 1.031, 0.1009, 1.063, 0.953, 0.991, 1.049, 0.983, 1.051, 1.019,
    1.069,
];

/// A single rounded toward zero.
fn tz(x: f64) -> f32 {
    let y = x as f32;
    if f64::from(y).abs() > x.abs() {
        if y > 0.0 { y.next_down() } else { y.next_up() }
    } else {
        y
    }
}

#[derive(Debug, Clone, Copy)]
struct ChorusVoice {
    second: bool,
    gain: [f32; 2],
    inc: u32,
    centre: f32,
    sq: f64,
    phase: u32,
}

#[derive(Debug, Clone)]
pub struct Chorus {
    voices: Vec<ChorusVoice>,
    ring: [Vec<f32>; 2],
    w: usize,
    pub longest: f64,
    out: Bus,
}

impl Chorus {
    pub fn new(st: &State, rate: u32) -> Option<Chorus> {
        let order = st.s[16].max(0) as usize;
        if order == 0 {
            return None;
        }
        let depth = st.s[17] as f32 / 1024.0;
        let speed = st.s[18] as f32 / 1024.0;
        let [delay, spread, cross, volume] = [19, 20, 21, 22].map(|i| st.s[i] as f32 / 128.0);
        let c = |x: f32, top: f64| s((f64::from(x) * ln(11.0)).exp() - 1.0) * s(top / 10.0);
        let dp = c(depth, 1092.5);
        let pace = 2147483648.0 / (f64::from(s((f64::from(s(1.0 - f64::from(speed))) * ln(101.0)).exp() - 1.0)) * 75000.0 + 50000.0) * 500.0;
        let dl = c(delay, 4410.0);
        let sp = s(f64::from(s((f64::from(spread) * ln(1001.0)).exp() - 1.0)) / 1000.0);
        let mix = |own: f64, shared: f64| own * f64::from(sp) + shared * f64::from(s(1.0 - f64::from(sp)));
        let k = s(44100.0 / f64::from(rate));
        let mut total = 0f32;
        let mut voices = Vec::with_capacity(2 * order);
        let mut longest = 0f64;
        for i in 0..2 * order {
            let second = i >= order;
            let rank = if second { 2 * order - i - 1 } else { i };
            let weight = 1.0 - rank as f32 / order as f32;
            let t = T[i % 20];
            let vary = s(mix(f64::from(t * t), 1.0));
            let other = cross * weight;
            let gain = if second { [other, 1.0] } else { [1.0, other] };
            total = s(f64::from(total) + f64::from(gain[0].abs()) + f64::from(gain[1].abs()));
            let mut sweep = dp * weight + 10.0;
            let inc = round(mix(pace * f64::from(vary) / f64::from(sweep), pace / f64::from(dp + 10.0)) * f64::from(k)) as i64 as u32;
            let mut centre = s(mix(f64::from(dl * weight * vary), f64::from(dl)));
            let start = round(f64::from(weight * sp) * 2.1474836e9 / 256.0) as i64 as u32;
            sweep /= k;
            centre = sweep + (centre / k).max(8.0);
            longest = longest.max(f64::from(centre + sweep));
            let sq = (f64::from(sweep) * 4.0).sqrt();
            let phase = start.wrapping_mul(256).wrapping_add(if second { 1 << 31 } else { 0 });
            voices.push(ChorusVoice { second, gain, inc, centre, sq, phase });
        }
        let g = (1.0 - s((f64::from(s(1.0 - 1.0 / f64::from(total))) * ln(1001.0)).exp() - 1.0) / 1000.0) * volume;
        for v in voices.iter_mut() {
            v.gain = v.gain.map(|x| x * g);
        }
        let len = (HOST_BLOCK * 4 + round(longest) as usize + 16).next_power_of_two();
        Some(Chorus { voices, ring: [vec![0.0; len], vec![0.0; len]], w: 0, longest, out: [Vec::new(), Vec::new()] })
    }

    /// Advances the phases by `n` frames without output.
    pub fn idle(&mut self, n: usize) {
        for v in self.voices.iter_mut() {
            v.phase = v.phase.wrapping_add(v.inc.wrapping_mul(n as u32));
        }
    }

    /// The chorus of `n` frames of the effects bus.
    pub fn run(&mut self, fx: &Bus, n: usize) -> &Bus {
        let mask = self.ring[0].len() - 1;
        for ch in 0..2 {
            for j in 0..n {
                self.ring[ch][(self.w + j) & mask] = fx[ch][j];
            }
            self.out[ch].clear();
            self.out[ch].resize(n, 0.0);
        }
        for (vi, v) in self.voices.iter_mut().enumerate() {
            let side = usize::from(v.second);
            let ring = &self.ring[side];
            let a = f64::from(s(v.sq / 2_147_483_648.0));
            let half = f64::from(s(v.sq / 2.0));
            let sqs = f64::from(s(v.sq));
            let centre = f64::from(v.centre);
            for j in 0..n {
                let t = ((v.phase as i32) as f64 * a).abs() - half;
                v.phase = v.phase.wrapping_add(v.inc);
                let d = t * (sqs - t.abs()) + centre;
                let w = d.trunc();
                let f = d - w;
                let at = self.w + j;
                let x0 = f64::from(ring[(at.wrapping_sub(w as usize)) & mask]);
                let x1 = f64::from(ring[(at.wrapping_sub(w as usize + 1)) & mask]);
                let x = x0 + (x1 - x0) * f;
                for c in 0..2 {
                    let o = &mut self.out[c][j];
                    let y = x * f64::from(v.gain[c]);
                    *o = if vi == 0 { tz(y) } else { tz(y + f64::from(*o)) };
                }
            }
        }
        self.w = (self.w + n) & mask;
        &self.out
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Delays

#[derive(Debug, Clone)]
pub struct DelayLine {
    d: usize,
    offset: [usize; 2],
    fb: f64,
    gain: f32,
    mode: i32,
    pub serial: bool,
    ring: Vec<[f32; 2]>,
    r: usize,
}

impl DelayLine {
    fn new(st: &State, k: usize, rate: u32, tempo: f32, serial: bool) -> DelayLine {
        let dl = &st.delays[k];
        let tr = f64::from(dl.time) * f64::from(rate);
        let d = if dl.synced { round(tr * 0.3125 / f64::from(tempo)) } else { round(tr / 384.0) }.max(1) as usize;
        let units = s(f64::from(s((f64::from(s(f64::from(dl.stereo.abs()) * (1.0 / 512.0))) * ln(1001.0)).exp() - 1.0)) * 50.0);
        let off = round(f64::from(s(f64::from(s(f64::from(units) * f64::from(rate as f32))) / 100000.0))).max(0) as usize;
        let offset = if dl.stereo > 0 { [0, off] } else { [off, 0] };
        let fb = f64::from(s(f64::from(dl.feedback) * (1.0 / 128.0)));
        let gain = if dl.level == 0 {
            0.0
        } else if dl.level.abs() >= 128 && dl.level.abs() == 128 {
            dl.level.signum() as f32
        } else {
            dl.level.signum() as f32 * curve(s(f64::from(dl.level.abs()) * (1.0 / 128.0)))
        };
        let len = d + off + HOST_BLOCK * 4 + 2;
        DelayLine { d, offset, fb, gain, mode: dl.mode, serial, ring: vec![[0.0; 2]; len], r: 0 }
    }

    /// One block: takes `input` when given, adds its output into `out`.
    fn run(&mut self, input: Option<&Bus>, out: &mut Bus, n: usize) {
        let len = self.ring.len();
        if let Some(x) = input {
            for ch in 0..2 {
                let base = self.r + self.d + self.offset[ch];
                for j in 0..n {
                    let c = &mut self.ring[(base + j) % len][ch];
                    *c = s(f64::from(*c) + f64::from(x[ch][j]));
                }
            }
        }
        let swap = self.mode != 0;
        let cross = self.mode >= 2 || self.mode < 0;
        for j in 0..n {
            let [l, r] = self.ring[self.r];
            self.ring[self.r] = [0.0; 2];
            let (ol, or) = if swap { (r, l) } else { (l, r) };
            let at = (self.r + self.d) % len;
            let (fl, fr) = if cross { (r, l) } else { (l, r) };
            let t = &mut self.ring[at];
            for (c, v) in [(0, fl), (1, fr)] {
                let y = s(f64::from(v) * self.fb + f64::from(t[c]));
                t[c] = if y.abs() < TINY { 0.0 } else { y };
            }
            self.r = (self.r + 1) % len;
            for (c, o) in [(0, ol), (1, or)] {
                let b = &mut out[c][j];
                *b = if self.gain == 1.0 { *b + o } else { s(f64::from(o) * f64::from(self.gain) + f64::from(*b)) };
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Reverb

#[derive(Debug, Clone, Default)]
struct Line {
    buf: Vec<f32>,
    pos: usize,
}

impl Line {
    fn new(len: usize) -> Line {
        Line { buf: vec![0.0; len.max(1)], pos: 0 }
    }
    fn out(&self) -> f32 {
        self.buf[self.pos]
    }
    fn push(&mut self, v: f32) {
        self.buf[self.pos] = v;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
    }
    fn delay(&mut self, x: f32) -> f32 {
        let y = self.out();
        self.push(x);
        y
    }
    fn allpass(&mut self, x: f32, f: f32, b: f32) -> f32 {
        let y = self.out() - f * x;
        self.push(b * y + x);
        y
    }
    fn tap(&self, k: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.pos + n - k % n) % n]
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Half {
    x: f32,
    s: f32,
    d: f32,
}

#[derive(Debug, Clone)]
pub struct Plate {
    geom: Geometry,
    lines: Vec<Line>,
    pre: Vec<f32>,
    pre_w: usize,
    predelay: usize,
    high_cut: f32,
    low_cut: f32,
    damping: f32,
    diffuser: f32,
    t0: f32,
    t1: f32,
    decay: f32,
    shelf: f32,
    shelf_gain: f32,
    pub wet: f32,
    half_a: Half,
    half_b: Half,
    lp: f32,
    lc_x: f32,
    lc_a: f32,
}

const LN001: f64 = -6.907755278982137;

fn quarter(lp: f32, t: f32) -> f32 {
    (0.25 * (LN001 * f64::from(lp) / f64::from(t)).exp().ln()).exp() as f32
}

impl Plate {
    pub fn new(st: &State, rate: u32, tempo: f32) -> Plate {
        let k = st.reverb.knobs;
        let ir = rate as i32;
        let q = s(1.0 / f64::from(rate));
        let half = s(f64::from(rate) * 0.5);
        let low_cut = if k[0] == 19 {
            0.0
        } else {
            let t = f64::from(s((f64::from((k[0] as f32).min(half)) * PI * f64::from(q)).tan()));
            s((t - 1.0) / (t + 1.0))
        };
        let pr = f64::from(k[2]) * f64::from(rate);
        let predelay = if st.reverb.tempo { round(pr * 0.3125 / f64::from(tempo)) } else { round(pr / 384.0) }.max(1) as usize;
        let geom = Geometry::new(k[3], ir);
        let len = |i: usize| geom.lengths[i] as i32;
        let d = s(f64::from(k[4]) * 0.01);
        let diff = |n: i32, spread: f64| if d == 0.0 { 0.0 } else { s((LN001 * f64::from(s(f64::from(n) * f64::from(q * 0.5))) / (f64::from(d) * spread)).exp()) };
        let lp = (len(5) + len(7) + len(9) + len(11)) as f32;
        let tl = s(f64::from(k[5]) * f64::from(rate) / 10.0);
        let decay = quarter(lp, tl);
        let mult = match k[7] {
            0 => 0.2,
            1 => 0.6,
            2 => 1.0,
            3 => 1.5,
            4 => 3.0,
            _ => 1.0,
        };
        let tb = tl * mult;
        let mut bass = quarter(lp, tb) / decay;
        let shelf_gain = s((f64::from(bass) - 1.0) * 0.5);
        bass = bass.min(1.0);
        let t = s((500.0 * PI * f64::from(q)).tan());
        let shelf = (t - bass) / (t + bass);
        Plate {
            lines: geom.lengths.iter().map(|&n| Line::new(n)).collect(),
            diffuser: diff(len(0) + len(1), 0.07),
            t0: diff(len(4) + len(8), 0.35),
            t1: diff(len(6) + len(10), 0.51),
            geom,
            pre: vec![0.0; predelay + 2],
            pre_w: 0,
            predelay,
            high_cut: lowpass(k[1], ir),
            low_cut,
            damping: lowpass(k[6], ir),
            decay,
            shelf,
            shelf_gain,
            wet: s(f64::from(k[8]) * (1.0 / 256.0)),
            half_a: Half::default(),
            half_b: Half::default(),
            lp: 0.0,
            lc_x: 0.0,
            lc_a: 0.0,
        }
    }

    fn half(h: &mut Half, x: f32, pre: f32, shelf: f32, gain: f32, damping: f32) -> f32 {
        let s = (x - h.s) * shelf + h.x;
        h.x = x;
        h.s = s;
        let y = (((s + x) * gain + x + pre) - h.d) * damping + h.d;
        h.d = y;
        y
    }

    #[inline]
    fn tick(&mut self, m: f32) -> (f32, f32) {
        let n = self.pre.len();
        let pre = self.pre[(self.pre_w + n - self.predelay % n) % n];
        let (shelf, gain, damping, decay) = (self.shelf, self.shelf_gain, self.damping, self.decay);
        let l = &mut self.lines;
        let x = l[11].out() * decay;
        let y = Self::half(&mut self.half_a, x, pre, shelf, gain, damping);
        let y = l[4].allpass(y, self.t0, self.t0);
        let y = l[5].delay(y) * decay;
        let y = l[6].allpass(y, self.t1, self.t1);
        l[7].push(y);
        let x = l[7].tap(0) * decay;
        let y = Self::half(&mut self.half_b, x, pre, shelf, gain, damping);
        let y = l[8].allpass(y, self.t0, self.t0);
        let y = l[9].delay(y) * decay;
        let y = l[10].allpass(y, self.t1, self.t1);
        l[11].push(y);

        let k = 1.0 - decay;
        let c06 = k * 0.6 + decay;
        let c025 = k * 0.25 + decay;
        let c07 = k * 0.7 + decay;
        let c02 = k * 0.2 + decay;
        let c05 = k * 0.5 + decay;
        let g = &self.geom;
        let tl = |i: usize| l[g.taps_l[i].0].tap(g.taps_l[i].1);
        let tr = |i: usize| l[g.taps_r[i].0].tap(g.taps_r[i].1);
        let left = (((tl(0) - c025 * tl(1)) - c07 * tl(2)) - c02 * tl(3)) + c06 * tl(4) + tl(5) + c05 * tl(6);
        let right = ((((c025 * tr(0) + tr(1)) - c07 * tr(2)) - c02 * tr(3)) - c06 * tr(4)) + tr(5) + c05 * tr(6);

        let x = ((m + 1e-30) - self.lp) * self.high_cut + self.lp;
        self.lp = x;
        let a = (x - self.lc_a) * self.low_cut + self.lc_x;
        self.lc_x = x;
        self.lc_a = a;
        let mut x = if self.low_cut != 0.0 { (x - a) * 0.5 + 1e-30 } else { x };
        let df = self.diffuser;
        for i in 0..4 {
            x = l[i].allpass(x, df, df);
        }
        self.pre[self.pre_w] = x;
        self.pre_w = (self.pre_w + 1) % n;
        (left, right)
    }

    /// The reverb of the effects bus, added to the dry bus at the wet level.
    pub fn run(&mut self, fx: &mut Bus, dry: &mut Bus, n: usize) {
        for j in 0..n {
            let (l, r) = self.tick(fx[0][j] + fx[1][j]);
            fx[0][j] = l;
            fx[1][j] = r;
            dry[0][j] = s(f64::from(l) * f64::from(self.wet) + f64::from(dry[0][j]));
            dry[1][j] = s(f64::from(r) * f64::from(self.wet) + f64::from(dry[1][j]));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The section

#[derive(Debug, Clone, Default)]
struct Dc {
    s: [f64; 2],
    v: [f64; 2],
}

pub struct Section {
    /// Oversampling exponents: voices `v`, effects `e`.
    pub v: usize,
    pub e: usize,
    host_rate: u32,
    dry_dec: Decimator,
    fx_dec: Decimator,
    eq: Option<(i32, TrackEq)>,
    chorus: Option<Chorus>,
    delays: Vec<(usize, DelayLine)>,
    plate: Option<Plate>,
    dc: Option<Dc>,
    feeds: bool,
    tail: i64,
    counter: i64,
    pub started_send: bool,
    eqbuf: Vec<f32>,
}

impl Section {
    pub fn new(st: &State, host_rate: u32, v: usize, tempo: f32) -> Section {
        let e = v.min(2);
        let rate = host_rate << e;
        let eq = (st.eq.mode != 0).then(|| {
            let b = st.eq.bands;
            (st.eq.mode, TrackEq::typed([0, 1, 2].map(|k| [b[k][0], b[k][1], b[k][2]]), [0, 1, 2].map(|k| b[k][3].clamp(0, 2) as usize), rate))
        });
        let chorus = Chorus::new(st, rate);
        let reverb_on = st.reverb.on;
        let delays: Vec<(usize, DelayLine)> = (0..3)
            .filter(|&k| st.delays[k].on)
            .map(|k| {
                let later = (k + 1..3).any(|j| st.delays[j].on) || reverb_on;
                (k, DelayLine::new(st, k, rate, tempo, st.delays[k].serial && later))
            })
            .collect();
        let plate = reverb_on.then(|| Plate::new(st, rate, tempo));
        let feeds = !delays.is_empty() || reverb_on || st.s[24] >= 0;
        let mut tail = trunc(f64::from(host_rate << v) * 100.0 / 44100.0);
        if let Some(c) = &chorus {
            tail += ((HOST_BLOCK << e) as i64 + round(c.longest) + 8) * (1 << v) / (1 << e);
        }
        Section {
            v,
            e,
            host_rate,
            dry_dec: Decimator::new(v),
            fx_dec: Decimator::new(v),
            eq,
            chorus,
            delays,
            plate,
            dc: (st.switch(1) != 0).then(Dc::default),
            feeds,
            tail,
            counter: 0,
            started_send: false,
            eqbuf: Vec::new(),
        }
    }

    fn eq(&mut self, bus: &mut Bus, n: usize) {
        let Some((_, eq)) = &mut self.eq else { return };
        self.eqbuf.clear();
        for j in 0..n {
            self.eqbuf.push(bus[0][j]);
            self.eqbuf.push(bus[1][j]);
        }
        eq.process(&mut self.eqbuf);
        for j in 0..n {
            bus[0][j] = self.eqbuf[2 * j];
            bus[1][j] = self.eqbuf[2 * j + 1];
        }
    }

    fn eq_mode(&self) -> i32 {
        self.eq.as_ref().map_or(0, |e| e.0)
    }

    /// Processes one host block: the buses hold `n << v` voice-rate frames; the dry bus leaves with `n` host frames.
    pub fn process(&mut self, dry: &mut Bus, fx: &mut Bus, n: usize, voice_alive: bool) {
        let vn = n << self.v;
        if !self.started_send {
            self.counter = 0;
        } else if voice_alive {
            self.counter = self.tail;
        } else if self.counter > 0 {
            self.counter -= vn as i64;
        }
        let awake = self.counter > 0;
        let m = self.dry_dec.run(dry, vn, self.v, self.e);
        self.fx_dec.run(fx, vn, self.v, self.e);
        match self.eq_mode() {
            2 => self.eq(dry, m),
            3 => self.eq(fx, m),
            _ => {}
        }
        if let Some(c) = &mut self.chorus {
            if awake {
                let out = c.run(fx, m);
                for ch in 0..2 {
                    for j in 0..m {
                        dry[ch][j] += out[ch][j];
                    }
                    if self.feeds {
                        for j in 0..m {
                            fx[ch][j] += out[ch][j];
                        }
                    }
                }
            } else {
                c.idle(m);
            }
        }
        let mut after_serial = false;
        for (_, d) in self.delays.iter_mut() {
            let input = (awake || after_serial).then_some(&*fx);
            let input: Option<Bus> = input.map(|b| [b[0][..m].to_vec(), b[1][..m].to_vec()]);
            if d.serial {
                d.run(input.as_ref(), fx, m);
                after_serial = true;
            } else {
                d.run(input.as_ref(), dry, m);
            }
        }
        if let Some(p) = &mut self.plate {
            p.run(fx, dry, m);
        }
        if self.eq_mode() == 1 {
            self.eq(dry, m);
        }
        self.dry_dec.run(dry, m, self.e, 0);
        if let Some(dc) = &mut self.dc {
            let k = f64::from(s(1.0070235 / f64::from(self.host_rate)));
            let c = f64::from(0.96f32);
            for ch in 0..2 {
                let (mut sv, mut v) = (dc.s[ch], dc.v[ch]);
                for x in dry[ch][..n].iter_mut() {
                    let xi = f64::from(*x);
                    v += (xi - sv) * k;
                    sv += v;
                    v *= c;
                    *x = s(xi - sv);
                }
                let f = |y: f64| if y.abs() <= f64::from(TINY) { 0.0 } else { y };
                dc.s[ch] = f(sv);
                dc.v[ch] = f(v);
            }
        }
    }
}
