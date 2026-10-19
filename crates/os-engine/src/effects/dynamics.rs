// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The limiter's whole processing (Lemon Limiter): gain, an RMS compressor with knee keyed from its own input or a
//! sidechain, a look-ahead peak limiter, a noise gate and a ceiling with optional soft saturation, all acting on one
//! smoothed gain applied to the delayed audio. Configured from the 18 parameters of the project file's limiter state.
//!
//! Arithmetic follows the spec's singles and doubles (`f32` / `f64`) so the output matches the original's.
//!
// spec: plugins/fruity-limiter-dsp.md, audio/conventions.md (single, double, Round)

use std::collections::VecDeque;

/// Parameters, in the original's order (state field k + 1).
pub const PARAMS: usize = 18;
pub const GAIN: usize = 0;
pub const SAT: usize = 1;
pub const CEILING: usize = 2;
pub const LIM_ATT: usize = 3;
pub const LIM_ATT_CURVE: usize = 4;
pub const LIM_REL: usize = 5;
pub const LIM_REL_CURVE: usize = 6;
pub const PEAK_WINDOW: usize = 7;
pub const COMP_THRES: usize = 8;
pub const COMP_RATIO: usize = 9;
pub const COMP_KNEE: usize = 10;
pub const COMP_ATT: usize = 11;
pub const COMP_REL: usize = 12;
pub const COMP_CURVE: usize = 13;
pub const RMS_WINDOW: usize = 14;
pub const GATE_GAIN: usize = 15;
pub const GATE_THRES: usize = 16;
pub const GATE_REL: usize = 17;

/// Stored ranges (min, max) of the parameters (spec "Parameters").
pub const RANGES: [(i32, i32); PARAMS] = [
    (0, 2000),
    (0, 1000),
    (0, 2000),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (0, 1000),
    (-1000, 1000),
    (-1000, 1000),
    (0, 10000),
    (0, 10000),
    (1, 8),
    (0, 10000),
    (0, 1000),
    (0, 1000),
    (0, 10000),
];

/// FL's factory preset "Default" (= a fresh instance).
pub const DEFAULTS: [i32; PARAMS] = [1000, 1000, 1000, 1115, 3, 5000, 3, 2882, 1000, 0, 0, 0, 6666, 1, 650, 1000, 0, 4187];

/// Flags bit 0: the limiter aims at the ceiling itself instead of 0.975 of it.
pub const FLAG_EXACT_CEILING: u8 = 1;

/// Silence: every sample's magnitude at or below this (bit pattern 0x358637BD).
const SILENT: f32 = 1e-6;
/// Scratch length: longer host blocks are processed in pieces of this many samples.
const CHUNK: usize = 1024;

/// A level knob: amplitude for stored value `v` with base `b` (spec "Knob laws").
pub fn level(v: i32, b: f64) -> f32 {
    let t = v as f32 * 0.001f32;
    if t == 1.0 {
        return 1.0;
    }
    let x = t * (b.ln() as f32);
    ((f64::from(x).exp() - 1.0) * f64::from((1.0 / (b - 1.0)) as f32)) as f32
}

/// A time knob's milliseconds (0..1000).
pub fn time_ms(v: i32) -> f32 {
    let x = (f64::from(v) * 0.0001) as f32;
    ((f64::from(x) * 501f64.ln()).exp() - 1.0) as f32 * 2.0
}

/// A time knob's length in samples.
pub fn time_samples(v: i32, rate: f64) -> f64 {
    f64::from(time_ms(v)) * 0.001 * rate
}

fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The coefficient of a smoother direction taking `t` samples (0 = no smoothing).
fn coef(t: f64) -> f64 {
    if t > 0.0 { (0.01f64.ln() / t.max(1.0)).exp() } else { 0.0 }
}

/// Stages of one-pole smoothing in series (spec "Smoothers").
#[derive(Clone, Debug)]
struct Chain {
    rise: f64,
    fall: f64,
    start: f64,
    s: Vec<f64>,
}

impl Chain {
    fn new(start: f64) -> Self {
        Chain { rise: 0.0, fall: 0.0, start, s: Vec::with_capacity(8) }
    }

    /// Sets the stage count (a changed count starts again at the starting value) and the times.
    fn set(&mut self, stages: usize, rise: f64, fall: f64) {
        if stages != self.s.len() {
            self.s.clear();
            self.s.resize(stages, self.start);
        }
        self.rise = coef(rise);
        self.fall = coef(fall);
    }

    fn reset_to(&mut self, v: f64) {
        self.s.iter_mut().for_each(|s| *s = v);
    }

    /// Runs every stage over `buf`, one stage after the other.
    fn run(&mut self, buf: &mut [f32]) {
        for s in &mut self.s {
            for x in buf.iter_mut() {
                let v = f64::from(*x);
                let c = if v >= *s { self.rise } else { self.fall };
                *s = v + (*s - v) * c;
                *x = *s as f32;
            }
        }
    }

    /// End of block: tiny states and states that are not a number become 0.
    fn flush(&mut self) {
        for s in &mut self.s {
            if !(s.abs() > 1.0 / 16_777_216.0) {
                *s = 0.0;
            }
        }
    }
}

/// The values worked out from the parameters (spec "Working values").
#[derive(Clone, Debug, Default)]
struct Work {
    gain: f32,
    ceiling: f32,
    limit: f32,
    gate_gain: f32,
    gate_below: f32,
    gate_coef: f32,
    knee: f32,
    room: f32,
    saturate: bool,
    comp: bool,
    threshold: f32,
    thr_db: f32,
    top: f32,
    ratio: f32,
    slope: f32,
    knee_kind: i8,
    ka: f32,
    kb: f32,
    kc: f32,
    look: usize,
    peak_n: usize,
    rms_m: usize,
    tail: i64,
}

pub struct Dynamics {
    rate: f64,
    p: [i32; PARAMS],
    flags: u8,
    w: Work,
    release: Chain,
    compressor: Chain,
    attack: Chain,
    gate_state: f32,
    /// Peak window: (position, peak) in decreasing order of peak; the front is the largest of the last N.
    peaks: VecDeque<(u64, f32)>,
    pos: u64,
    /// RMS window: the last M squared sums and their running total.
    rms_ring: Vec<f32>,
    rms_at: usize,
    rms_sum: f64,
    /// Look-ahead delay (stereo).
    delay: Vec<[f32; 2]>,
    delay_at: usize,
    counter: i64,
    asleep: bool,
    peak: Vec<f32>,
    rms: Vec<f32>,
    g: Vec<f32>,
    inp: Vec<f32>,
    /// Display metering (editor): samples per column (0 = off), the column being summed, finished columns.
    meter_len: usize,
    meter_n: usize,
    meter_acc: Column,
    pub columns: Vec<Column>,
}

/// One display column: the largest input and output peak, the largest analysed level, the smallest gain.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Column {
    pub input: f32,
    pub output: f32,
    pub level: f32,
    pub gain: f32,
}

/// Finished columns kept until the plugin takes them (older ones are dropped).
const MAX_COLUMNS: usize = 256;

impl Dynamics {
    /// A limiter at `rate` with stored parameters `p` and flags byte `flags`.
    pub fn new(rate: f64, p: [i32; PARAMS], flags: u8) -> Self {
        let mut d = Dynamics {
            rate,
            p,
            flags,
            w: Work::default(),
            release: Chain::new(0.0),
            compressor: Chain::new(0.0),
            attack: Chain::new(1.0),
            gate_state: 1.0,
            peaks: VecDeque::new(),
            pos: 0,
            rms_ring: Vec::new(),
            rms_at: 0,
            rms_sum: 0.0,
            delay: Vec::new(),
            delay_at: 0,
            counter: 0,
            asleep: true,
            peak: vec![0.0; CHUNK],
            rms: vec![0.0; CHUNK],
            g: vec![0.0; CHUNK],
            inp: vec![0.0; CHUNK],
            meter_len: 0,
            meter_n: 0,
            meter_acc: Column { gain: f32::MAX, ..Column::default() },
            columns: Vec::with_capacity(MAX_COLUMNS),
        };
        d.work(true);
        d
    }

    /// Display metering: `len` samples per column, 0 = off.
    pub fn set_meter(&mut self, len: usize) {
        if len != self.meter_len {
            self.meter_len = len;
            self.meter_n = 0;
            self.meter_acc = Column { gain: f32::MAX, ..Column::default() };
        }
    }

    fn meter(&mut self, input: f32, output: f32, level: f32, gain: f32) {
        let a = &mut self.meter_acc;
        a.input = a.input.max(input);
        a.output = a.output.max(output);
        a.level = a.level.max(level);
        a.gain = a.gain.min(gain);
        self.meter_n += 1;
        if self.meter_n >= self.meter_len {
            if self.columns.len() >= MAX_COLUMNS {
                self.columns.remove(0);
            }
            self.columns.push(*a);
            self.meter_n = 0;
            *a = Column { gain: f32::MAX, ..Column::default() };
        }
    }

    /// The delay the plugin reports: the look-ahead, in samples.
    pub fn latency(&self) -> usize {
        self.w.look
    }

    pub fn params(&self) -> &[i32; PARAMS] {
        &self.p
    }

    /// True while asleep (silence passes unchanged, spec "Silence").
    pub fn asleep(&self) -> bool {
        self.asleep
    }

    pub fn set_rate(&mut self, rate: f64) {
        if rate != self.rate {
            self.rate = rate;
            self.work(true);
        }
    }

    /// Sets parameter `k` to stored value `v` (automation: worked in at once, spec "A parameter under automation").
    pub fn set_param(&mut self, k: usize, v: i32) {
        if k >= PARAMS {
            return;
        }
        // Setting a time or curve knob, also to the value it has, opens the gate's glide again.
        if matches!(k, 3..=7 | 11..=14 | 17) {
            self.gate_state = 1.0;
        }
        if self.p[k] != v {
            self.p[k] = v;
            self.work(false);
        }
    }

    /// Sets every parameter (only those that change take effect, as one call each would).
    pub fn set_params(&mut self, p: &[i32; PARAMS]) {
        if *p == self.p {
            return;
        }
        for k in 0..PARAMS {
            if p[k] != self.p[k] {
                if matches!(k, 3..=7 | 11..=14 | 17) {
                    self.gate_state = 1.0;
                }
                self.p[k] = p[k];
            }
        }
        self.work(false);
    }

    pub fn set_flags(&mut self, flags: u8) {
        if flags != self.flags {
            self.flags = flags;
            self.work(false);
        }
    }

    /// Works the values out from the parameters; windows and the delay line follow their lengths.
    fn work(&mut self, fresh: bool) {
        let p = &self.p;
        let rate = self.rate;
        let mut w = Work { gain: level(p[GAIN], 7.0), ceiling: level(p[CEILING], 3.0), ..Work::default() };
        w.limit = if self.flags & FLAG_EXACT_CEILING != 0 { w.ceiling } else { w.ceiling * 0.975 };
        w.gate_gain = level(p[GATE_GAIN], 7.0);
        w.gate_below = level(p[GATE_THRES], 7.0).min(w.limit);
        w.gate_coef = (0.01f64.ln() / (time_samples(p[GATE_REL], rate).max(1.0) * 2.0)).exp() as f32;
        w.knee = level(p[SAT], 3.0);
        w.saturate = w.knee < 1.0;
        if w.saturate {
            let room = w.ceiling.min(1.0) - w.knee;
            w.room = if room > 0.0 { room } else { 1.525_878_9e-5 };
        }

        w.comp = p[COMP_THRES] < 1000;
        let db_unit = (10f64.ln() * 0.05) as f32;
        w.threshold = level(p[COMP_THRES], 7.0).max((f64::from(db_unit) * -60.0).exp() as f32);
        w.thr_db = (f64::from(w.threshold).log10() * 20.0) as f32;
        w.top = w.threshold * 100.0;
        let c = ((f64::from((p[COMP_RATIO].unsigned_abs() as f64 * 0.001) as f32) * 51f64.ln()).exp() - 1.0) as f32;
        w.ratio = if p[COMP_RATIO] > 0 { (1.0 / (f64::from(c) * 0.38 + 1.0)) as f32 } else { (f64::from(c) * 0.06 + 1.0) as f32 };
        w.slope = w.ratio - 1.0;
        let kv = p[COMP_KNEE];
        let k = ((f64::from((kv as f32 * 0.001).abs()) * 201f64.ln()).exp() - 1.0) as f32;
        if kv > 0 {
            w.knee_kind = 1;
            w.ka = (f64::from(k) * 0.005) as f32;
            w.kb = (1.0 / f64::from(w.ka + 1.0).ln()) as f32;
        } else if kv < 0 {
            w.knee_kind = -1;
            w.ka = (f64::from(k) * 0.001) as f32;
            w.kb = f64::from(w.ka + 1.0).ln() as f32;
            w.kc = 1.0 / w.ka;
        }

        let look = round(time_samples(p[LIM_ATT], rate)).max(0) as usize;
        w.look = look;
        w.peak_n = (round(time_samples(p[PEAK_WINDOW], rate)).max(0) as usize).max(look).max(1);
        w.rms_m = (round(time_samples(p[RMS_WINDOW], rate)).max(0) as usize).max(1);
        w.tail = look as i64 + round(rate * 1.5);

        // Smoother chains.
        let rel = time_samples(p[LIM_REL], rate);
        let rel_stages = if rel == 0.0 { 0 } else { p[LIM_REL_CURVE].clamp(0, 64) as usize };
        self.release.set(rel_stages, 0.0, rel * 2.0 / (rel_stages.max(1) as f64).sqrt());
        let cs = p[COMP_CURVE].clamp(0, 64) as usize;
        let sq = (cs.max(1) as f64).sqrt();
        self.compressor.set(cs, time_samples(p[COMP_ATT], rate) / sq, time_samples(p[COMP_REL], rate) * 2.0 / sq);
        let att_stages = if look < 1 { 0 } else { p[LIM_ATT_CURVE].clamp(0, 64) as usize };
        let at = f64::from((look as f64 * 0.8) as f32) / (att_stages.max(1) as f64).sqrt();
        self.attack.set(att_stages, at, at);

        let old = std::mem::replace(&mut self.w, w);
        // A changed look-ahead clears the delay line; a changed window clears that window.
        if fresh || old.look != look {
            self.delay.clear();
            self.delay.resize(look, [0.0; 2]);
            self.delay_at = 0;
        }
        if fresh || old.peak_n != self.w.peak_n {
            self.peaks.clear();
        }
        if fresh || old.rms_m != self.w.rms_m {
            self.rms_ring.clear();
            self.rms_ring.resize(self.w.rms_m, 0.0);
            self.rms_at = 0;
            self.rms_sum = 0.0;
        }
    }

    /// Falling asleep, or the slot switched off or on (spec "State and timing").
    pub fn reset(&mut self) {
        self.attack.reset_to(1.0);
        self.release.reset_to(0.0);
        self.compressor.reset_to(0.0);
        self.gate_state = 1.0;
        self.delay.iter_mut().for_each(|s| *s = [0.0; 2]);
        self.delay_at = 0;
    }

    /// Processes one block in place. `key` = the sidechain input (left, right) the compressor reads instead of the
    /// block's own input. Blocks are the original's ticks: sleep and waking are decided per block.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let n = left.len().min(right.len());
        let sound = left[..n].iter().chain(&right[..n]).any(|x| !(x.abs() <= SILENT));
        if sound {
            self.counter = self.w.tail;
        } else {
            self.counter -= n as i64;
            if self.counter <= -(n as i64) {
                if !self.asleep {
                    self.asleep = true;
                    self.reset();
                }
                return;
            }
        }
        self.asleep = false;
        let mut at = 0;
        while at < n {
            let end = (at + CHUNK).min(n);
            let k = key.map(|(l, r)| (&l[at.min(l.len())..end.min(l.len())], &r[at.min(r.len())..end.min(r.len())]));
            self.run(&mut left[at..end], &mut right[at..end], k);
            at = end;
        }
        self.release.flush();
        self.compressor.flush();
        self.attack.flush();
    }

    fn run(&mut self, left: &mut [f32], right: &mut [f32], key: Option<(&[f32], &[f32])>) {
        let n = left.len();
        let w = &self.w;
        // 1. Peak of the undelayed input, held over the peak window.
        for i in 0..n {
            let pk = left[i].abs().max(right[i].abs());
            self.inp[i] = pk;
            let at = self.pos;
            while self.peaks.back().is_some_and(|&(_, v)| v <= pk) {
                self.peaks.pop_back();
            }
            self.peaks.push_back((at, pk));
            while self.peaks.front().is_some_and(|&(q, _)| q + w.peak_n as u64 <= at) {
                self.peaks.pop_front();
            }
            self.peak[i] = self.peaks.front().map_or(0.0, |&(_, v)| v);
            self.pos += 1;
        }

        // 2. Compressor.
        let g = &mut self.g[..n];
        g.fill(w.gain);
        if w.comp {
            let m = w.rms_m;
            let inv = (1.0 / (2.0 * m as f64)) as f32;
            for i in 0..n {
                let (l, r) = match key {
                    Some((kl, kr)) => (kl.get(i).copied().unwrap_or(0.0), kr.get(i).copied().unwrap_or(0.0)),
                    None => (left[i], right[i]),
                };
                let x = l * l + r * r;
                self.rms_sum = (self.rms_sum - f64::from(self.rms_ring[self.rms_at])) + f64::from(x);
                self.rms_ring[self.rms_at] = x;
                self.rms_at = (self.rms_at + 1) % m;
                self.rms[i] = ((f64::from(inv) * self.rms_sum) as f32).max(0.0).sqrt();
            }
            self.compressor.run(&mut self.rms[..n]);
            let db_unit = (10f64.ln() * 0.05) as f32;
            for i in 0..n {
                let lv = self.rms[i];
                if lv > w.threshold {
                    let over = (f64::from(lv.min(w.top)).log10() as f32) * 20.0 + (-w.thr_db);
                    let db = match w.knee_kind {
                        0 => over * w.slope,
                        kind => {
                            let mut u = over + 1.0;
                            u = if kind > 0 {
                                (f64::from(u * w.ka + 1.0).ln() as f32) * w.kb
                            } else {
                                ((f64::from(u * w.kb).exp() as f32) + -1.0) * w.kc
                            };
                            (u + -1.0) * w.ratio - over
                        }
                    };
                    g[i] = (f64::from(db * db_unit).exp() as f32) * w.gain;
                }
            }
        }

        // 3. Delay the audio by the look-ahead.
        if w.look > 0 {
            for i in 0..n {
                let d = &mut self.delay[self.delay_at];
                let out = *d;
                *d = [left[i], right[i]];
                left[i] = out[0];
                right[i] = out[1];
                self.delay_at = (self.delay_at + 1) % w.look;
            }
        }

        // 4. Limiter and gate on the release-smoothed held peak.
        self.release.run(&mut self.peak[..n]);
        for i in 0..n {
            let lv = self.peak[i];
            if lv * g[i] > w.limit {
                g[i] = w.limit / lv;
            }
            if lv < w.gate_below {
                g[i] = w.gate_gain + (self.gate_state - w.gate_gain) * w.gate_coef;
            }
            self.gate_state = g[i];
        }

        // 5. Attack smoothing of the gain; 6. output; 7. ceiling.
        self.attack.run(g);
        let ceiling = w.ceiling;
        let inv_room = 1.0 / w.room;
        for i in 0..n {
            for s in [&mut left[i], &mut right[i]] {
                let v = *s * g[i];
                let a = v.abs();
                *s = if w.saturate {
                    if a > w.knee {
                        let y = ((1.0 - (f64::from(w.knee - a) * f64::from(inv_room)).exp()) * f64::from(w.room) + f64::from(w.knee)) as f32;
                        y.copysign(v)
                    } else {
                        v
                    }
                } else if a > ceiling {
                    ceiling.copysign(v)
                } else {
                    v
                };
            }
        }
        if self.meter_len > 0 {
            for i in 0..n {
                let (inp, lv, g) = (self.inp[i], self.peak[i], self.g[i]);
                self.meter(inp, left[i].abs().max(right[i].abs()), lv, g);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(n: usize, amp: f32, rate: f64) -> Vec<f32> {
        (0..n).map(|i| amp * (i as f64 * 2.0 * std::f64::consts::PI * 220.0 / rate).sin() as f32).collect()
    }

    #[test]
    fn knob_laws_match_the_displayed_values() {
        // Gain −∞ / −11.2 / 0.0 / +9.3 / +18.1 dB at normalised 0 / .25 / .5 / .75 / 1 (spec: fruity-limiter.md).
        let db = |a: f32| 20.0 * f64::from(a).log10();
        assert_eq!(level(0, 7.0), 0.0);
        assert!((db(level(500, 7.0)) + 11.2).abs() < 0.05);
        assert_eq!(level(1000, 7.0), 1.0);
        assert!((db(level(2000, 7.0)) - 18.06).abs() < 0.01);
        assert!((db(level(2000, 3.0)) - 12.04).abs() < 0.01);
        // Times: FL's Default values as the editor shows them (releases doubled).
        assert!((time_ms(1115) - 2.0).abs() < 0.005);
        assert!((2.0 * time_ms(5000) - 85.53).abs() < 0.005);
        assert!((time_ms(2882) - 10.0).abs() < 0.005);
        assert!((2.0 * time_ms(6666) - 248.22).abs() < 0.01);
        assert!((time_ms(10000) - 1000.0).abs() < 0.01);
    }

    #[test]
    fn default_setting_limits_at_the_safety_ceiling_and_reports_its_look_ahead() {
        let rate = 44100.0;
        let mut d = Dynamics::new(rate, DEFAULTS, 0);
        // 2 ms of look-ahead = 88 samples (also the reported delay).
        assert_eq!(d.latency(), 88);
        let mut l = sine(44100, 1.8, rate);
        let mut r = l.clone();
        for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
            d.process(cl, cr, None);
        }
        // Once settled the output stays at or under the ceiling and reaches about 0.975 of it.
        let peak = l[22050..].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak <= 1.0 && peak > 0.9, "{peak}");
        // The audio is delayed by the look-ahead: the first 88 samples are silent.
        assert!(l[..88].iter().all(|x| *x == 0.0));
    }

    #[test]
    fn quiet_input_passes_with_the_gain_only() {
        let rate = 48000.0;
        let mut p = DEFAULTS;
        p[LIM_ATT] = 0; // no look-ahead
        p[GAIN] = 2000; // +18 dB: x8
        let mut d = Dynamics::new(rate, p, 0);
        assert_eq!(d.latency(), 0);
        let input = sine(4800, 0.05, rate);
        let (mut l, mut r) = (input.clone(), input.clone());
        d.process(&mut l, &mut r, None);
        for (o, i) in l.iter().zip(&input) {
            assert!((o - i * 8.0).abs() < 1e-5);
        }
    }

    #[test]
    fn silence_sleeps_and_sound_wakes() {
        let mut d = Dynamics::new(44100.0, DEFAULTS, 0);
        let mut z = vec![0f32; 256];
        let mut z2 = z.clone();
        d.process(&mut z, &mut z2, None);
        assert!(d.asleep());
        let mut s = sine(256, 0.5, 44100.0);
        let mut s2 = s.clone();
        d.process(&mut s, &mut s2, None);
        assert!(!d.asleep());
        // The tail (look + 1.5 s) keeps it awake through silence after sound.
        let mut z = vec![0f32; 44100];
        let mut z2 = z.clone();
        d.process(&mut z, &mut z2, None);
        assert!(!d.asleep());
        // 88 + 66150 samples after the sound it sleeps.
        for _ in 0..50 {
            let mut z = vec![0f32; 512];
            let mut z2 = z.clone();
            d.process(&mut z, &mut z2, None);
        }
        assert!(d.asleep());
    }

    #[test]
    fn compressor_lowers_a_loud_signal_and_reads_the_sidechain() {
        let rate = 44100.0;
        let mut p = DEFAULTS;
        p[COMP_THRES] = 600; // ≈ −8.6 dB
        p[COMP_RATIO] = 1000; // 20:1
        let mut d = Dynamics::new(rate, p, 0);
        let input = sine(8820, 0.9, rate);
        let (mut l, mut r) = (input.clone(), input.clone());
        d.process(&mut l, &mut r, None);
        let out = l[4410..].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(out < 0.6, "{out}");
        // Keyed from a silent sidechain the compressor does nothing.
        let mut d = Dynamics::new(rate, p, 0);
        let silent = vec![0f32; input.len()];
        let (mut l, mut r) = (input.clone(), input.clone());
        d.process(&mut l, &mut r, Some((&silent, &silent)));
        let out = l[4410..].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!((out - 0.9).abs() < 0.01, "{out}");
    }

    #[test]
    fn saturation_rounds_off_under_the_ceiling() {
        let mut p = DEFAULTS;
        p[LIM_ATT] = 0;
        p[SAT] = 500; // knee ≈ −8.7 dB
        p[CEILING] = 1500; // well above the signal: the limiter stays out
        let mut d = Dynamics::new(44100.0, p, 0);
        let mut l = vec![0.9f32; 64];
        let mut r = vec![-0.9f32; 64];
        d.process(&mut l, &mut r, None);
        let knee = level(500, 3.0);
        assert!(l[63] > knee && l[63] < 1.0, "{}", l[63]);
        assert_eq!(r[63], -l[63]);
    }

    #[test]
    fn noise_gate_closes_below_its_threshold() {
        let mut p = DEFAULTS;
        p[LIM_ATT] = 0;
        p[GATE_THRES] = 500; // ≈ −16 dB
        p[GATE_GAIN] = 0; // close to silence
        let mut d = Dynamics::new(44100.0, p, 0);
        let mut l = sine(44100, 0.05, 44100.0);
        let mut r = l.clone();
        d.process(&mut l, &mut r, None);
        let tail = l[30000..].iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(tail < 1e-3, "{tail}");
    }
}
