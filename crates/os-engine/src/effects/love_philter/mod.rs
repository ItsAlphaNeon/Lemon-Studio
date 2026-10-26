// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Fruity Love Philter: eight filters in a chain, each with its type, cascade, waveshaper, pan and volume, and eight
//! modulation targets driven by patterns, an LFO and three maps. Lemon Studio's Pulp Filter plays this code.
//!
//! The x87 extended-precision loops of the page run in doubles here (as Citrus does), so the samples are close to
//! FL's but not bit for bit.
//!
// spec: plugins/fruity-love-philter.md, plugins/image-line-curves.md, plugins/lemon-pulp-filter.md

pub mod coefs;
pub mod state;

use std::f64::consts::FRAC_PI_4;

use os_dsp::curve::{map_table, Curve, Lfo, LfoShape, Walk, SYNCED};
use os_dsp::glide::Glide;
use os_dsp::num::{round, s, trunc};

use super::eq::hq::HalfBand;
use super::Effect;
use coefs::{stages, work_out, Coef, Input};
use state::*;

/// 2^-24.
const TINY: f64 = 5.960464477539063e-8;

fn level(x: f32) -> f32 {
    os_dsp::level::curve(x)
}

fn slevel(x: f32) -> f32 {
    if x == 0.0 { 0.0 } else { level(x.abs()).copysign(x) }
}

/// A map is neutral when all its points have the target's neutral level (1, or 0.5 for an additive target).
pub fn neutral(c: &Curve, t: usize) -> bool {
    let n = if additive(t) { 0.5 } else { 1.0 };
    c.points.iter().all(|p| p.level == n)
}

/// One target's modulation as it runs.
#[derive(Debug, Clone)]
struct Art {
    running: bool,
    out: f32,
    walk: Walk,
    lfo: Lfo,
    maps: [Option<Vec<f32>>; 3],
    amount: f32,
    lfo_amount: f32,
}

/// A stage's state, per channel (the fields of its type are used).
#[derive(Debug, Clone, Copy, Default)]
struct Stage {
    /// Chocolate: low and band; Lime: y and v.
    a: [f64; 2],
    b: [f64; 2],
    x1: [f64; 2],
    x2: [f64; 2],
    y1: [f64; 2],
    y2: [f64; 2],
    p: [[f32; 4]; 2],
    phi: [f32; 2],
    /// The glided frequency and damping.
    f: f32,
    q: f32,
}

/// One filter unit as it runs.
#[derive(Debug, Clone)]
struct Unit {
    st: [Stage; 3],
    lv: [f32; 3],
    mix: f32,
    gains: [f32; 4],
    ws: Vec<i32>,
    arts: Vec<Art>,
    // The values worked out from the controls.
    c: f32,
    r: f32,
    svf: [f32; 3],
    gain: f32,
    drive: f32,
    pre: f32,
    mix_v: f32,
    pan: f32,
    vol: f32,
    input: f32,
    next: f32,
}

/// The settings a filter's chunk reads.
#[derive(Debug, Clone, Copy)]
struct Kind {
    kind: i32,
    order: i32,
    alt: bool,
    ws: bool,
    bipolar: bool,
}

pub struct LovePhilter {
    st: State,
    rate: f64,
    tempo: f64,
    playing: bool,
    /// The song position in beats (when playing).
    beats: f64,
    stages: u32,
    /// The internal rate.
    ir: f64,
    limit: f32,
    rf_filters: f32,
    hq_env: bool,
    tick: usize,
    sec: f64,
    mod_coef: f32,
    countdown: usize,
    mod_cur: [f32; 2],
    mod_pos: [usize; 2],
    master: f32,
    lfo_master: f32,
    follow_level: f32,
    att: f32,
    rel: f32,
    fol_run: bool,
    fol_e: f32,
    fol_peak: f32,
    fol_pos: usize,
    units: Vec<Unit>,
    up: Vec<[HalfBand; 2]>,
    down: Vec<[HalfBand; 2]>,
    dc: [[f64; 2]; 2],
    dc_c1: f32,
    dc_c2: f32,
    quiet: i64,
    jump: bool,
    // Scratch.
    host: [Vec<f32>; 2],
    xi: [Vec<f32>; 2],
    sum: [Vec<f32>; 2],
    cur: [Vec<f32>; 2],
    prev: [Vec<f32>; 2],
    z: [Vec<f32>; 2],
    pat: [Vec<f32>; 4],
}

/// The stepping pattern of the levels and the mix (spec "Glides"): pairs from `g` and `g + s` in steps of `2 s`,
/// then the rest from the next value in steps of `s`.
fn pattern(g: f32, st: f32, out: &mut [f32]) {
    let n = out.len();
    let s2 = st + st;
    let pairs = 8 * (n / 8);
    let (mut a, mut b) = (g, g + st);
    let mut i = 0;
    while i < pairs {
        out[i] = a;
        out[i + 1] = b;
        a += s2;
        b += s2;
        i += 2;
    }
    for v in &mut out[pairs..] {
        *v = a;
        a += st;
    }
}

/// The four pan / volume gains: left to left, right to right, right into left, left into right.
pub fn pan_gains(pan: f32, vol: f32) -> [f32; 4] {
    if vol == 0.0 {
        return [0.0; 4];
    }
    if pan.to_bits() == 0 {
        return [vol, vol, 0.0, 0.0];
    }
    let sq = s(1.0 / FRAC_PI_4.sin());
    let vs = f64::from(vol * sq);
    let ang = (1.0 + f64::from(pan)) * FRAC_PI_4;
    let l = s(ang.cos() * vs);
    let rg = s(ang.sin() * vs);
    let (a, b) = if pan > 0.0 {
        (0.0, (1.0 - l / vol) * 0.5)
    } else if pan < 0.0 {
        ((1.0 - rg / vol) * 0.5, 0.0)
    } else {
        (0.0, 0.0)
    };
    [l * (1.0 - a), rg * (1.0 - b), l * a, rg * b]
}

/// The waveshaper's table: a guard, entries 0..=N, a guard.
fn ws_table(c: &Curve, bipolar: u8) -> Vec<i32> {
    let bip = bipolar != 0;
    let n = if bip { 32767 } else { 16383 };
    let scale = if bip { 65535.0 } else { 32767.0 };
    let vals = map_table(c, f64::from(bipolar) * 0.5, f64::from(s(1.0 / n as f64)), n);
    let mut t = Vec::with_capacity(n + 3);
    t.push(0);
    t.extend(vals.iter().map(|&v| round(f64::from(s(f64::from(v) * scale))).clamp(-32768, 32767) as i32));
    t[0] = t[1];
    t.push(t[n + 1]);
    t
}

/// One sample through the waveshaper's table (spec "One sample").
fn shape(t: &[i32], bip: bool, pre: f32, x: f32, interp: bool) -> f32 {
    let n = (t.len() - 3) as i64;
    let nf = n as f64;
    let e = |j: i64| f64::from(t[(j + 1) as usize]);
    let x = f64::from(x);
    let v = if !bip {
        let tt = x * (f64::from(pre) * nf);
        if !interp {
            let i = round(tt);
            if i >= 0 { e(i.min(n)) } else { -e((-i).min(n)) }
        } else if !(tt.abs() < 2147483648.0) {
            0.0
        } else {
            let i = trunc(tt);
            let phi = tt - i as f64;
            if i >= 0 {
                let j = i.min(n);
                e(j) + (e(j + 1) - e(j)) * phi
            } else {
                let j = (-i).min(n);
                -(e(j) - (e(j + 1) - e(j)) * phi)
            }
        }
    } else {
        let tt = x * ((f64::from(pre) * nf) * 0.5) + nf * 0.5;
        if !interp {
            e(round(tt).clamp(0, n))
        } else {
            let i = trunc(tt).clamp(-1, n);
            let phi = tt - trunc(tt) as f64;
            e(i) + (e(i + 1) - e(i)) * phi
        }
    };
    s(v / 32768.0)
}

impl LovePhilter {
    /// An instance with `state` applied at `rate`, through the start a render's slot has been through: FL's silent
    /// device buffers (4 x 2048 frames), then a reposition to the song's start (spec "What the plugin has been through
    /// when a render starts").
    pub fn from_state(state: &[u8], rate: u32) -> Option<LovePhilter> {
        let st = State::from_bytes(state)?;
        Some(LovePhilter::with_state(st, rate))
    }

    pub fn with_state(st: State, rate: u32) -> LovePhilter {
        let mut lp = LovePhilter {
            st: State::new_default(),
            rate: 44100.0,
            tempo: 120.0,
            playing: false,
            beats: 0.0,
            stages: 0,
            ir: 44100.0,
            limit: 0.0,
            rf_filters: 1.0,
            hq_env: false,
            tick: 64,
            sec: 0.0,
            mod_coef: 0.0,
            countdown: 0,
            mod_cur: [0.0; 2],
            mod_pos: [0; 2],
            master: 1.0,
            lfo_master: 1.0,
            follow_level: 1.0,
            att: 0.0,
            rel: 0.0,
            fol_run: false,
            fol_e: 0.0,
            fol_peak: 0.0,
            fol_pos: 0,
            units: Vec::new(),
            up: Vec::new(),
            down: Vec::new(),
            dc: [[0.0; 2]; 2],
            dc_c1: 0.0,
            dc_c2: s(0.96),
            quiet: 0,
            jump: false,
            host: Default::default(),
            xi: Default::default(),
            sum: Default::default(),
            cur: Default::default(),
            prev: Default::default(),
            z: Default::default(),
            pat: Default::default(),
        };
        lp.set_rate(f64::from(rate.max(1)));
        lp.apply(st);
        lp.preroll();
        lp
    }

    /// FL's silent buffers before a render, then the song-position change to the start.
    pub fn preroll(&mut self) {
        let mut buf = vec![0.0f32; 2 * 2048];
        for _ in 0..4 {
            self.process(&mut buf);
            buf.fill(0.0);
        }
        self.reposition();
    }

    pub fn state(&self) -> &State {
        &self.st
    }

    /// The mod X / Y positions and the follower's (0..1024), for the editor.
    pub fn positions(&self) -> [usize; 3] {
        [self.mod_pos[0], self.mod_pos[1], self.fol_pos]
    }

    /// The output of filter `f`'s target `t` (the editor's playhead).
    pub fn target(&self, f: usize, t: usize) -> f32 {
        self.units.get(f).and_then(|u| u.arts.get(t)).map_or(0.0, |a| a.out)
    }

    /// Where filter `f`'s target `t`'s pattern walk is (segment, progress), for the editor.
    pub fn pattern_at(&self, f: usize, t: usize) -> Option<(usize, f64)> {
        let a = self.units.get(f)?.arts.get(t)?;
        (a.running && !a.walk.finished()).then_some((a.walk.i, a.walk.p))
    }

    /// The host's sample rate (spec "Messages from FL", SetSampleRate).
    pub fn set_rate(&mut self, rate: f64) {
        self.rate = rate;
        self.rates();
        self.dc_c1 = s(1.0070234999999998 / rate);
        self.dc = [[0.0; 2]; 2];
        self.follower_coefs();
        self.steps();
        self.reposition();
    }

    fn rates(&mut self) {
        self.stages = self.st.oversampling();
        self.ir = self.rate * f64::from(1u32 << self.stages);
        let rf = 44100.0 / self.ir;
        self.limit = s(f64::from(s(rf * 0.001)) * 10.0);
        let m = if self.hq_env { 8.0 } else { 64.0 };
        let a = (self.ir * m) / 44100.0;
        self.tick = (round(a / 4.0) * 4).max(4) as usize;
        let t = (self.tick as f64 * m) / (a * 96.0);
        self.sec = (t * 96.0) / 44100.0;
        self.mod_coef = s(((0.01f64.ln() * t) / 96.0).exp());
        if self.up.len() != self.stages as usize {
            self.up = vec![[HalfBand::default(); 2]; self.stages as usize];
            self.down = vec![[HalfBand::default(); 2]; self.stages as usize];
        }
    }

    fn follower_coefs(&mut self) {
        let coef = |v: i32| {
            let x = s(f64::from(v) * 6.103515625e-05);
            let u = f64::from(s((f64::from(x) * 81f64.ln()).exp() - 1.0)) * (self.ir * 0.0125);
            if u <= 0.0 { 0.0 } else { s((0.01f64.ln() / u).exp()) }
        };
        self.att = coef(self.st.globals[FOLLOW_ATTACK]);
        self.rel = coef(self.st.globals[FOLLOW_RELEASE]);
    }

    /// The song's tempo: the steps and LFOs set up again, no reposition.
    pub fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 && bpm != self.tempo {
            self.tempo = bpm;
            self.steps();
        }
    }

    /// The transport (FL's SetPlaying / SongPosChanged): playing or not, and the song position in beats; a
    /// reposition.
    pub fn set_transport(&mut self, playing: bool, beats: f64) {
        self.playing = playing;
        self.beats = beats;
        self.reposition();
    }

    fn step_of(&self, c: &Curve) -> f64 {
        if c.flags & SYNCED != 0 { (self.sec * self.tempo) / 60.0 } else { self.sec * 2.0 }
    }

    /// The patterns' and LFOs' steps from the control tick and tempo; the LFOs set up again.
    fn steps(&mut self) {
        let pat = self.pattern_index();
        for (f, u) in self.units.iter_mut().enumerate() {
            for (t, a) in u.arts.iter_mut().enumerate() {
                let sa = &self.st.filters[f].arts[t];
                let sp = if sa.patterns[pat].flags & SYNCED != 0 { (self.sec * self.tempo) / 60.0 } else { self.sec * 2.0 };
                rescale(&mut a.walk, sp);
                let sl = if sa.lfo.flags & SYNCED != 0 { (self.sec * self.tempo) / 60.0 } else { self.sec * 2.0 };
                rescale(&mut a.lfo.walk, sl);
                a.lfo.shape = LfoShape::new(&sa.lfo, sl);
            }
        }
    }

    fn pattern_index(&self) -> usize {
        (self.st.globals[PATTERN].clamp(1, PATTERNS as i32) - 1) as usize
    }

    /// Applies a state (loading a project or preset, spec "Applying a state").
    pub fn apply(&mut self, st: State) {
        self.st = st;
        self.hq_env = self.st.hq_envelopes();
        self.rates();
        self.rf_filters = s(44100.0 / (self.rate * f64::from(1u32 << self.st.oversampling())));
        let pat = self.pattern_index();
        let mut units = Vec::with_capacity(FILTERS);
        for (f, sf) in self.st.filters.iter().enumerate() {
            let old_gains = self.units.get(f).map_or([0.0; 4], |u| u.gains);
            let mut st0 = Stage::default();
            st0.f = 0.5;
            st0.q = if sf.kind == CHOCOLATE { 1.0 } else { 0.1 };
            let arts = sf
                .arts
                .iter()
                .enumerate()
                .map(|(t, a)| {
                    let maps = std::array::from_fn(|m| {
                        (!neutral(&a.maps[m], t)).then(|| map_table(&a.maps[m], centre(t), f64::from(s(1.0 / 1024.0)), 1024))
                    });
                    let pc = &a.patterns[pat];
                    let running = pc.enabled || a.lfo.enabled || maps.iter().any(Option::is_some);
                    Art {
                        running,
                        out: a.base,
                        walk: Walk::unscaled(pc, self.step_of(pc), centre(t)),
                        lfo: Lfo::new(&a.lfo, self.step_of(&a.lfo), None),
                        maps,
                        amount: a.amount,
                        lfo_amount: 1.0,
                    }
                })
                .collect();
            units.push(Unit {
                st: [st0; 3],
                lv: [0.0; 3],
                mix: 0.0,
                gains: old_gains,
                ws: ws_table(&sf.ws_curve, sf.ws_bipolar),
                arts,
                c: 0.0,
                r: 0.0,
                svf: [0.0; 3],
                gain: 1.0,
                drive: 0.0,
                pre: 1.0,
                mix_v: 1.0,
                pan: 0.0,
                vol: 0.0,
                input: 0.0,
                next: 0.0,
            });
        }
        self.units = units;
        let fol = self.units.iter().any(|u| u.arts.iter().any(|a| a.maps[2].is_some()));
        if !fol {
            (self.fol_e, self.fol_peak, self.fol_pos) = (0.0, 0.0, 0);
        }
        self.fol_run = fol;
        self.levels();
        self.follower_coefs();
        self.steps();
        self.reposition();
    }

    /// An edit of the state in use (Pulp Filter's editor, spec plugins/lemon-pulp-filter.md §6): unlike [`Self::apply`],
    /// only what changed is set up again, so the sound goes on. A filter whose type or order changed starts with empty
    /// stages; a changed waveshaper curve gets its table again; a target whose curves changed starts its pattern and LFO
    /// again (at the song position while playing, as a reposition does).
    pub fn edit(&mut self, new: State) {
        let old = std::mem::replace(&mut self.st, new);
        if old.oversampling() != self.st.oversampling() || old.hq_envelopes() != self.st.hq_envelopes() {
            self.hq_env = self.st.hq_envelopes();
            self.rates();
            self.rf_filters = s(44100.0 / self.ir);
            self.up = vec![[HalfBand::default(); 2]; self.stages as usize];
            self.down = vec![[HalfBand::default(); 2]; self.stages as usize];
            self.steps();
            self.countdown = 0;
        }
        let pat = self.pattern_index();
        let sp = self.start_time(true);
        for f in 0..FILTERS {
            let (o, n) = (&old.filters[f], &self.st.filters[f]);
            let u = &mut self.units[f];
            if o.kind != n.kind || o.order != n.order || o.alt != n.alt {
                let mut st0 = Stage::default();
                st0.f = 0.5;
                st0.q = if n.kind == CHOCOLATE { 1.0 } else { 0.1 };
                u.st = [st0; 3];
            }
            if o.ws_curve != n.ws_curve || o.ws_bipolar != n.ws_bipolar {
                u.ws = ws_table(&n.ws_curve, n.ws_bipolar);
            }
            for t in 0..TARGETS {
                let (oa, na) = (&o.arts[t], &n.arts[t]);
                if oa == na && old.globals[PATTERN] == self.st.globals[PATTERN] {
                    continue;
                }
                let a = &mut u.arts[t];
                a.maps = std::array::from_fn(|m| {
                    (!neutral(&na.maps[m], t)).then(|| map_table(&na.maps[m], centre(t), f64::from(s(1.0 / 1024.0)), 1024))
                });
                let pc = &na.patterns[pat];
                a.running = pc.enabled || na.lfo.enabled || a.maps.iter().any(Option::is_some);
                if !a.running {
                    a.out = na.base;
                    continue;
                }
                let step_p = if pc.flags & SYNCED != 0 { (self.sec * self.tempo) / 60.0 } else { self.sec * 2.0 };
                let step_l = if na.lfo.flags & SYNCED != 0 { (self.sec * self.tempo) / 60.0 } else { self.sec * 2.0 };
                if oa.lfo != na.lfo || !a.running {
                    a.lfo = Lfo::new(&na.lfo, step_l, None);
                    a.lfo.start_at(&na.lfo, sp);
                }
                if oa.patterns[pat] != *pc || old.globals[PATTERN] != self.st.globals[PATTERN] {
                    a.walk = Walk::unscaled(pc, step_p, centre(t));
                    a.walk.start_at(pc, sp);
                }
            }
        }
        let fol = self.units.iter().any(|u| u.arts.iter().any(|a| a.maps[2].is_some()));
        if !fol {
            (self.fol_e, self.fol_peak, self.fol_pos) = (0.0, 0.0, 0);
        }
        self.fol_run = fol;
        self.levels();
        self.follower_coefs();
    }

    /// Recomputes the values the controls give (spec "Values the processing uses").
    fn levels(&mut self) {
        let g = self.st.globals;
        self.master = level(g[MASTER] as f32 * (1.0 / 128.0));
        self.lfo_master = slevel(g[LFO_MASTER] as f32 * (1.0 / 128.0));
        self.follow_level = level(g[FOLLOW_LEVEL] as f32 * (1.0 / 128.0));
        let solo = self.st.solo();
        for (f, u) in self.units.iter_mut().enumerate() {
            let sf = &self.st.filters[f];
            let c = sf.controls;
            let v = |n: usize, d: f32| c[n] as f32 * d;
            let env = v(C_ENV, 1.0 / 128.0);
            for (t, a) in u.arts.iter_mut().enumerate() {
                if t == T_CUT {
                    a.amount = env;
                    a.lfo_amount = env * self.lfo_master;
                } else {
                    a.amount = sf.arts[t].amount;
                    a.lfo_amount = self.lfo_master;
                }
            }
            u.c = v(C_CUT, 1.0 / 256.0);
            u.r = v(C_RES, 1.0 / 256.0);
            u.svf = [level(v(C_LOW, 1.0 / 128.0)), level(v(C_BAND, 1.0 / 128.0)), level(v(C_HIGH, 1.0 / 128.0))];
            u.gain = s(f64::from(s((f64::from(s(f64::from(c[C_LOW] as f32) * 0.015625)) * 8f64.ln()).exp() - 1.0)) * 0.125 + 0.125);
            u.drive = v(C_LOW, 1.0 / 128.0);
            u.pre = v(C_PRE, 1.0 / 128.0);
            u.mix_v = v(C_WSMIX, 1.0 / 128.0);
            u.pan = v(C_PAN, 1.0 / 128.0);
            u.vol = if solo.is_some_and(|s| s != f) { 0.0 } else { level(v(C_VOL, 1.0 / 128.0)) * self.master };
            u.input = slevel(v(C_IN, 1.0 / 128.0));
            u.next = if f + 1 < FILTERS { slevel(v(C_NEXT, 1.0 / 128.0)) } else { 0.0 };
        }
    }

    /// Sets a setting by FL's automation index (spec "Automation").
    pub fn set_param(&mut self, k: usize, v: i32) {
        if k >= PARAMS {
            return;
        }
        let old_pattern = self.st.globals[PATTERN];
        self.st.set_param(k, v);
        match k {
            0..=2 => self.levels(),
            3 | 4 => self.follower_coefs(),
            5 => {
                if self.st.globals[PATTERN] != old_pattern {
                    self.restart_patterns();
                }
            }
            6 | 7 => {}
            _ => match filter_param(k) {
                Some((f, FilterParam::Lfo(t, _))) => {
                    let c = &self.st.filters[f].arts[t].lfo;
                    let step = self.step_of(c);
                    self.units[f].arts[t].lfo.shape = LfoShape::new(c, step);
                }
                _ => self.levels(),
            },
        }
    }

    /// The song position, in internal samples, a start follows.
    fn start_time(&self, follows: bool) -> f64 {
        let p = if follows && self.playing {
            let ticks = self.beats * 96.0;
            ((ticks * self.ir) * 240.0) / f64::from(s(f64::from(s(f64::from(self.tempo as f32) * 96.0)) * 4.0))
        } else {
            0.0
        };
        (p - self.tick as f64 + self.countdown as f64) / self.tick as f64
    }

    /// A new pattern number restarts every running articulator's pattern.
    fn restart_patterns(&mut self) {
        let sp = self.start_time(self.st.switches[SW_SYNC] != 0);
        let pat = self.pattern_index();
        for f in 0..FILTERS {
            for t in 0..TARGETS {
                let sa = &self.st.filters[f].arts[t];
                let pc = &sa.patterns[pat];
                let maps = self.units[f].arts[t].maps.iter().any(Option::is_some);
                let step = self.step_of(pc);
                let a = &mut self.units[f].arts[t];
                a.running = pc.enabled || sa.lfo.enabled || maps;
                if !a.running {
                    a.out = sa.base;
                    continue;
                }
                a.walk = Walk::unscaled(pc, step, centre(t));
                a.walk.start_at(pc, sp);
            }
        }
    }

    /// A reposition: the next sample starts a control tick; every running articulator starts its LFO and pattern at
    /// the start time.
    fn reposition(&mut self) {
        self.countdown = 0;
        let sp = self.start_time(true);
        let pat = self.pattern_index();
        for (f, u) in self.units.iter_mut().enumerate() {
            for (t, a) in u.arts.iter_mut().enumerate() {
                let sa = &self.st.filters[f].arts[t];
                if !a.running {
                    a.out = sa.base;
                    continue;
                }
                a.lfo.start_at(&sa.lfo, sp);
                a.walk.start_at(&sa.patterns[pat], sp);
            }
        }
    }

    /// FL's Flush: stage states, glided values, half-band, DC and follower cleared; the next chunk jumps.
    pub fn flush(&mut self) {
        for u in &mut self.units {
            for st in &mut u.st {
                *st = Stage { f: st.f, q: st.q, ..Stage::default() };
            }
        }
        self.up = vec![[HalfBand::default(); 2]; self.stages as usize];
        self.down = vec![[HalfBand::default(); 2]; self.stages as usize];
        self.dc = [[0.0; 2]; 2];
        (self.fol_e, self.fol_peak) = (0.0, 0.0);
        self.jump = true;
        self.reposition();
    }

    /// The modulation of one control tick.
    fn modulate(&mut self) {
        let smooth = self.st.smooth();
        for k in 0..2 {
            let target = (if k == 0 { self.st.mod_x() } else { self.st.mod_y() }) as f32;
            self.mod_cur[k] = if smooth { target + (self.mod_cur[k] - target) * self.mod_coef } else { target };
            self.mod_pos[k] = round(f64::from(self.mod_cur[k])).clamp(0, 1024) as usize;
        }
        if self.fol_run {
            self.fol_pos = round(f64::from(s(f64::from(self.fol_peak) * 1024.0))).clamp(0, 1024) as usize;
            self.fol_peak = 0.0;
        }
        let pos = [self.mod_pos[0], self.mod_pos[1], self.fol_pos];
        let pat = self.pattern_index();
        for (f, u) in self.units.iter_mut().enumerate() {
            for (t, a) in u.arts.iter_mut().enumerate() {
                if !a.running {
                    continue;
                }
                let sa = &self.st.filters[f].arts[t];
                let add = additive(t);
                let mut out = sa.base;
                for m in 0..3 {
                    if let Some(tab) = &a.maps[m] {
                        let mut v = tab[pos[m]];
                        if m == 2 {
                            v *= a.amount;
                        }
                        out = if add { out + v } else { out * v };
                    }
                }
                let pc = &sa.patterns[pat];
                if pc.enabled {
                    a.walk.advance(pc, 1.0);
                    let e = a.walk.out;
                    out = if add { out + e * a.amount } else { (out * e) * a.amount };
                }
                if sa.lfo.enabled {
                    a.lfo.advance(&sa.lfo, 1.0);
                    let l = a.lfo.value;
                    out = if add { out + l * a.lfo_amount } else { out * ((l * a.lfo_amount) * 2.0 + 1.0) };
                }
                a.out = out;
            }
        }
    }

    /// The follower over one chunk of samples.
    fn follow(&mut self, l: &[f32], r: &[f32]) {
        follow(&mut self.fol_e, &mut self.fol_peak, [self.follow_level, self.att, self.rel], l, r);
    }

    /// Internal samples `i0..i0 + n` (one chunk).
    fn chunk(&mut self, i0: usize, n: usize) {
        let mut sum = std::mem::take(&mut self.sum);
        let mut cur = std::mem::take(&mut self.cur);
        let mut prev = std::mem::take(&mut self.prev);
        let mut z = std::mem::take(&mut self.z);
        let mut pat = std::mem::take(&mut self.pat);
        let xi = std::mem::take(&mut self.xi);
        for ch in 0..2 {
            sum[ch][..n].fill(0.0);
        }
        let (xl, xr) = (&xi[0][i0..i0 + n], &xi[1][i0..i0 + n]);
        let follow1 = self.st.follow_filter1;
        if self.fol_run && !follow1 {
            self.follow(xl, xr);
        }
        let jump = std::mem::replace(&mut self.jump, false);
        let interp = self.st.hq_ws;
        let mut prev_on = false;
        let mut prev_next = 0.0f32;
        for f in 0..FILTERS {
            let sf = &self.st.filters[f];
            if !sf.enabled {
                prev_on = false;
                continue;
            }
            let k = Kind { kind: sf.kind, order: sf.order, alt: sf.alt, ws: sf.ws, bipolar: sf.ws_bipolar != 0 };
            // The input.
            let u = &self.units[f];
            let (inv, tn) = (u.input, if prev_on { prev_next } else { 0.0 });
            for ch in 0..2 {
                let x = if ch == 0 { xl } else { xr };
                for i in 0..n {
                    let a = if inv == 0.0 {
                        None
                    } else if inv == 1.0 {
                        Some(x[i])
                    } else {
                        Some(x[i] * inv)
                    };
                    let b = if tn == 0.0 {
                        None
                    } else if tn == 1.0 {
                        Some(prev[ch][i])
                    } else {
                        Some(prev[ch][i] * tn)
                    };
                    cur[ch][i] = match (b, a) {
                        (Some(b), Some(a)) => b + a,
                        (Some(v), None) | (None, Some(v)) => v,
                        (None, None) => 0.0,
                    };
                }
            }
            let rf = self.rf_filters;
            let limit = self.limit;
            let u = &mut self.units[f];
            cascade(u, k, rf, limit, jump, &mut cur, n, &mut pat);
            if k.ws {
                waveshape(u, k, limit, jump, interp, &mut cur, &mut z, n, &mut pat);
            }
            let next = u.next;
            if f == 0 && follow1 && self.fol_run {
                self.follow(&cur[0][..n], &cur[1][..n]);
            }
            let u = &mut self.units[f];
            // Pan and volume.
            let t = |i: usize| u.arts[i].out;
            let pan = u.pan + t(T_PAN) * 2.0;
            let vol = u.vol * t(T_VOL);
            let target = pan_gains(pan, vol);
            let mut ramps = [Glide::new(0.0).block(0.0, 1, 0.0); 4];
            for g in 0..4 {
                if jump {
                    u.gains[g] = target[g];
                }
                let mut gl = Glide::new(u.gains[g]);
                ramps[g] = gl.block(target[g], n, limit);
                u.gains[g] = gl.current;
            }
            for i in 0..n {
                let (yl, yr) = (cur[0][i], cur[1][i]);
                let [ll, rr, rl, lr] = [ramps[0].next(), ramps[1].next(), ramps[2].next(), ramps[3].next()];
                sum[0][i] += yl * ll + yr * rl;
                sum[1][i] += yr * rr + yl * lr;
            }
            std::mem::swap(&mut cur, &mut prev);
            prev_on = true;
            prev_next = next;
        }
        for ch in 0..2 {
            self.sum_out(ch, i0, &sum[ch][..n]);
        }
        self.sum = sum;
        self.cur = cur;
        self.prev = prev;
        self.z = z;
        self.pat = pat;
        self.xi = xi;
    }

    fn sum_out(&mut self, ch: usize, i0: usize, v: &[f32]) {
        self.host[ch][i0..i0 + v.len()].copy_from_slice(v);
    }

    fn grow(&mut self, frames: usize) {
        let ni = frames << self.stages;
        let tick = self.tick;
        for ch in 0..2 {
            if self.xi[ch].len() < ni {
                self.xi[ch].resize(ni, 0.0);
            }
            if self.host[ch].len() < ni {
                self.host[ch].resize(ni, 0.0);
            }
            for b in [&mut self.sum[ch], &mut self.cur[ch], &mut self.prev[ch], &mut self.z[ch]] {
                if b.len() < tick {
                    b.resize(tick, 0.0);
                }
            }
        }
        for p in &mut self.pat {
            if p.len() < tick {
                p.resize(tick, 0.0);
            }
        }
    }

    /// One host block of stereo, planar.
    pub fn process_planar(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if n == 0 {
            return;
        }
        self.grow(n);
        let ni = n << self.stages;
        // The silence test.
        let floor = s(1e-6);
        let quiet = left[..n].iter().chain(&right[..n]).all(|x| x.abs() <= floor);
        let skip_up = quiet && self.quiet <= 0;
        if quiet {
            self.quiet -= n as i64;
        } else {
            self.quiet = round(self.ir / 5.0);
        }
        // Up.
        let mut xi = std::mem::take(&mut self.xi);
        let mut tmp = std::mem::take(&mut self.host);
        for (ch, inp) in [&left[..n], &right[..n]].into_iter().enumerate() {
            if self.stages == 0 {
                xi[ch][..n].copy_from_slice(inp);
            } else if skip_up {
                xi[ch][..ni].fill(0.0);
            } else {
                xi[ch][..n].copy_from_slice(inp);
                let mut len = n;
                for st in 0..self.stages as usize {
                    tmp[ch][..len].copy_from_slice(&xi[ch][..len]);
                    for i in 0..len {
                        let [a, b] = self.up[st][ch].up(tmp[ch][i]);
                        xi[ch][2 * i] = a;
                        xi[ch][2 * i + 1] = b;
                    }
                    self.up[st][ch].tidy();
                    len *= 2;
                }
            }
        }
        self.xi = xi;
        self.host = tmp;
        // The chunks.
        let mut i = 0;
        while i < ni {
            if self.countdown == 0 {
                self.modulate();
                self.countdown = self.tick;
            }
            let len = self.countdown.min(ni - i);
            self.chunk(i, len);
            self.countdown -= len;
            i += len;
        }
        // Down.
        let mut out = std::mem::take(&mut self.host);
        for ch in 0..2 {
            let mut len = ni;
            for st in (0..self.stages as usize).rev() {
                for j in 0..len / 2 {
                    out[ch][j] = self.down[st][ch].down(out[ch][2 * j], out[ch][2 * j + 1]);
                }
                self.down[st][ch].tidy();
                len /= 2;
            }
        }
        // DC removal.
        let dc = self.st.dc();
        for (ch, o) in [&mut *left, &mut *right].into_iter().enumerate() {
            if dc {
                let (c1, c2) = (f64::from(self.dc_c1), f64::from(self.dc_c2));
                let [mut a, mut b] = self.dc[ch];
                for j in 0..n {
                    let x = f64::from(out[ch][j]);
                    b += (x - a) * c1;
                    a += b;
                    b *= c2;
                    o[j] = s(x - a);
                }
                self.dc[ch] = [flush_le(a), flush_le(b)];
            } else {
                o[..n].copy_from_slice(&out[ch][..n]);
            }
        }
        self.host = out;
    }
}

/// The envelope follower over samples: `e` carried unrounded through them, kept as a single after.
fn follow(fe: &mut f32, fpeak: &mut f32, k: [f32; 3], l: &[f32], r: &[f32]) {
    let mut e = f64::from(*fe);
    let [lv, at, re] = k.map(f64::from);
    let mut peak = *fpeak;
    for (&x, &y) in l.iter().zip(r) {
        let a = f64::from(x.abs().max(y.abs())) * lv;
        e = a + (e - a) * if a > e { at } else { re };
        let mut o = s(e);
        if o.to_bits() < 12 {
            o = 0.0;
            e = 0.0;
        }
        peak = peak.max(o);
    }
    *fe = s(e);
    *fpeak = peak;
}

fn flush_le(x: f64) -> f64 {
    if x.abs() <= TINY { 0.0 } else { x }
}

fn flush_lt(x: f64) -> f64 {
    if x.abs() < TINY { 0.0 } else { x }
}

/// A walk's step changed: the segment under way keeps its length.
fn rescale(w: &mut Walk, step: f64) {
    if w.step != 0.0 && w.r != 0.0 && w.step != step {
        w.r *= step / w.step;
    }
    w.step = step;
}

/// Plans a glide of `n` samples and returns its start and step.
fn glide(cur: &mut f32, target: f32, n: usize, limit: f32, jump: bool) -> (f32, f32) {
    if jump {
        *cur = target;
    }
    let mut g = Glide::new(*cur);
    let r = g.block(target, n, limit);
    *cur = g.current;
    (r.first(), r.step())
}

/// A filter's cascade over a chunk, in place.
#[allow(clippy::too_many_arguments)]
fn cascade(u: &mut Unit, k: Kind, rf: f32, limit: f32, jump: bool, buf: &mut [Vec<f32>; 2], n: usize, pat: &mut [Vec<f32>; 4]) {
    let ns = stages(k.order);
    if ns == 0 {
        return;
    }
    let t = |i: usize| u.arts[i].out;
    let input = Input {
        kind: k.kind,
        order: k.order,
        alt: k.alt,
        cutoff: u.c + t(T_CUT),
        res: u.r + t(T_RES),
        gain: u.gain,
        drive: u.drive,
        rf,
    };
    let co = work_out(&input);
    match k.kind {
        CHOCOLATE => chocolate(u, &co, ns, k.alt, rf, limit, jump, buf, n, pat),
        LIME_LP | LIME_HP => {
            for (st, c) in u.st.iter_mut().zip(&co).take(ns) {
                let (f0, df) = glide(&mut st.f, c.f, n, limit, jump);
                let (q0, dq) = glide(&mut st.q, c.q, n, limit, jump);
                for ch in 0..2 {
                    let (mut y, mut v) = (st.a[ch], st.b[ch]);
                    let (mut f, mut q) = (f64::from(f0), f64::from(q0));
                    for x in &mut buf[ch][..n] {
                        let xv = f64::from(*x);
                        v += (xv - y) * f;
                        y += v;
                        *x = s(if k.kind == LIME_LP { y } else { xv - y });
                        v *= q;
                        f += f64::from(df);
                        q += f64::from(dq);
                    }
                    st.a[ch] = flush_le(y);
                    st.b[ch] = flush_le(v);
                }
            }
        }
        MANGO => {
            for (st, c) in u.st.iter_mut().zip(&co).take(ns) {
                let (g, q, kk) = (c.g, f64::from(c.mq), f64::from(c.k));
                let fd = c.drive;
                let sig = |z: f64| {
                    let a = z.abs();
                    let cc = (3.0 + a) * a + 6.0;
                    z * cc / (a * cc + 12.0)
                };
                for ch in 0..2 {
                    let mut p = st.p[ch];
                    let mut phi = st.phi[ch];
                    for x in &mut buf[ch][..n] {
                        let xv = f64::from(*x);
                        let p4 = f64::from(p[3]);
                        let np = if fd == 0.0 {
                            let u0 = xv + f64::from(phi) * q;
                            let p1 = f64::from(p[0]) + (u0 - f64::from(p[0])) * g;
                            let p2 = f64::from(p[1]) + (p1 - f64::from(p[1])) * g;
                            let p3 = f64::from(p[2]) + (p2 - f64::from(p[2])) * g;
                            let p4n = p4 + (p3 - p4) * g;
                            [p1, p2, p3, p4n]
                        } else {
                            let u0 = sig((xv + f64::from(phi) * q) * fd);
                            let p1 = f64::from(p[0]) + (u0 - sig(f64::from(p[0]) * fd)) * g;
                            let p2 = f64::from(p[1]) + (sig(p1 * fd) - sig(f64::from(p[1]) * fd)) * g;
                            let p3 = f64::from(p[2]) + (sig(p2 * fd) - sig(f64::from(p[2]) * fd)) * g;
                            let p4n = p4 + (sig(p3 * fd) - sig(p4 * fd)) * g;
                            [p1, p2, p3, p4n]
                        };
                        let sm = p4 + np[3];
                        *x = s(kk * sm);
                        phi = s(sm);
                        p = np.map(s);
                    }
                    st.p[ch] = p.map(|v| flush_lt(f64::from(v)) as f32);
                    st.phi[ch] = flush_lt(f64::from(phi)) as f32;
                }
            }
        }
        _ => {
            for (st, c) in u.st.iter_mut().zip(&co).take(ns) {
                let [a1, a2, b0, b1, b2] = c.b;
                for ch in 0..2 {
                    let (mut x1, mut x2, mut y1, mut y2) = (st.x1[ch], st.x2[ch], st.y1[ch], st.y2[ch]);
                    for x in &mut buf[ch][..n] {
                        let xv = f64::from(*x);
                        let y = xv * b0 + (((x1 * b1 - y2 * a2) + x2 * b2) - y1 * a1);
                        x2 = x1;
                        x1 = xv;
                        y2 = y1;
                        y1 = y;
                        *x = s(y);
                    }
                    st.x1[ch] = flush_lt(x1);
                    st.x2[ch] = flush_lt(x2);
                    st.y1[ch] = flush_lt(y1);
                    st.y2[ch] = flush_lt(y2);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn chocolate(
    u: &mut Unit,
    co: &[Coef; 3],
    ns: usize,
    alt: bool,
    rf: f32,
    limit: f32,
    jump: bool,
    buf: &mut [Vec<f32>; 2],
    n: usize,
    pat: &mut [Vec<f32>; 4],
) {
    // The three levels.
    let tg = [T_LOW, T_BAND, T_HIGH];
    let mut ramps = [(0.0f32, 0.0f32); 3];
    for j in 0..3 {
        let mut g = u.svf[j] * u.arts[tg[j]].out;
        if ns > 1 {
            g = if g > 0.0 { coefs::pow_s(f64::from(g), f64::from(s(1.0 / ns as f64))) } else { 0.0 };
        }
        ramps[j] = glide(&mut u.lv[j], g, n, limit, jump);
    }
    let gliding = ramps.iter().any(|r| r.1 != 0.0);
    if gliding {
        for j in 0..3 {
            pattern(ramps[j].0, ramps[j].1, &mut pat[j][..n]);
        }
    }
    for (k, (st, c)) in u.st.iter_mut().zip(co).take(ns).enumerate() {
        let (f0, df) = glide(&mut st.f, c.f, n, limit, jump);
        let (mut q0, dq) = if k == 0 { glide(&mut st.q, c.q, n, limit, jump) } else { (1.0, 0.0) };
        if alt {
            let x = f0.max(f0 + df * n as f32);
            q0 = if k == 0 { (q0 * rf) / x } else { rf / x };
        }
        for ch in 0..2 {
            let (mut low, mut band) = (st.a[ch], st.b[ch]);
            let (mut f, mut q) = (f64::from(f0), f64::from(q0));
            for i in 0..n {
                let x = f64::from(buf[ch][i]);
                low += f * band;
                let high = x - (q * band + low);
                band += f * high;
                f += f64::from(df);
                q += f64::from(dq);
                let (lo, bo, ho) = (s(low), s(band), s(high));
                buf[ch][i] = if gliding {
                    let mut o = lo * pat[0][i];
                    o += bo * pat[1][i];
                    o + ho * pat[2][i]
                } else {
                    let [gl, gb, gh] = [ramps[0].0, ramps[1].0, ramps[2].0];
                    let mut o = if gl == 0.0 {
                        0.0
                    } else if gl == 1.0 {
                        lo
                    } else {
                        lo * gl
                    };
                    for (v, g) in [(bo, gb), (ho, gh)] {
                        if g == 1.0 {
                            o += v;
                        } else if g != 0.0 {
                            o = v.mul_add(g, o);
                        }
                    }
                    o
                };
            }
            st.a[ch] = flush_lt(low);
            st.b[ch] = flush_lt(band);
        }
    }
}

/// The waveshaper and its mix over a chunk, in place (`z` is scratch).
#[allow(clippy::too_many_arguments)]
fn waveshape(
    u: &mut Unit,
    k: Kind,
    limit: f32,
    jump: bool,
    interp: bool,
    buf: &mut [Vec<f32>; 2],
    z: &mut [Vec<f32>; 2],
    n: usize,
    pat: &mut [Vec<f32>; 4],
) {
    let m = (u.arts[T_WSMIX].out * u.mix_v).clamp(0.0, 1.0);
    let (m0, dm) = glide(&mut u.mix, m, n, limit, jump);
    let pre = u.pre;
    for ch in 0..2 {
        for i in 0..n {
            z[ch][i] = shape(&u.ws, k.bipolar, pre, buf[ch][i], interp);
        }
    }
    if dm != 0.0 {
        let (a, b) = pat.split_at_mut(1);
        pattern(1.0 - m0, -dm, &mut a[0][..n]);
        pattern(m0, dm, &mut b[0][..n]);
        for ch in 0..2 {
            for i in 0..n {
                buf[ch][i] = buf[ch][i] * a[0][i] + z[ch][i] * b[0][i];
            }
        }
    } else if m0 == 1.0 {
        for ch in 0..2 {
            buf[ch][..n].copy_from_slice(&z[ch][..n]);
        }
    } else if m0 != 0.0 {
        for ch in 0..2 {
            for i in 0..n {
                buf[ch][i] = buf[ch][i] * (1.0 - m0) + z[ch][i] * m0;
            }
        }
    }
}

impl Effect for LovePhilter {
    fn process(&mut self, buf: &mut [f32]) {
        let n = buf.len() / 2;
        let mut l: Vec<f32> = buf.iter().step_by(2).copied().collect();
        let mut r: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
        self.process_planar(&mut l[..n], &mut r[..n]);
        for i in 0..n {
            buf[2 * i] = l[i];
            buf[2 * i + 1] = r[i];
        }
    }

    fn reset(&mut self) {
        self.flush();
    }

    fn set_tempo(&mut self, bpm: f64) {
        LovePhilter::set_tempo(self, bpm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, n: usize) -> Vec<f32> {
        (0..n).flat_map(|i| {
            let v = (0.5 * (i as f64 * hz * std::f64::consts::TAU / 44100.0).sin()) as f32;
            [v, v]
        })
        .collect()
    }

    fn rms(b: &[f32]) -> f64 {
        (b.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / b.len() as f64).sqrt()
    }

    fn run(st: State, hz: f64) -> f64 {
        let mut lp = LovePhilter::with_state(st, 44100);
        let mut b = sine(hz, 16384);
        for c in b.chunks_mut(2 * 512) {
            lp.process(c);
        }
        assert!(b.iter().all(|x| x.is_finite()));
        rms(&b[8192..])
    }

    #[test]
    fn pattern_steps_in_pairs() {
        let mut o = [0.0; 11];
        pattern(0.0, 1.0, &mut o);
        assert_eq!(o, [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
    }

    #[test]
    fn pan_gains_centred_and_hard() {
        assert_eq!(pan_gains(0.0, 0.5), [0.5, 0.5, 0.0, 0.0]);
        assert_eq!(pan_gains(0.3, 0.0), [0.0; 4]);
        let g = pan_gains(1.0, 1.0);
        // Hard right: the left input goes over to the right side.
        assert!(g[0].abs() < 1e-6 && g[2].abs() < 1e-6, "{g:?}");
        assert!(
            (g[1] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6 && (g[3] - g[1]).abs() < 1e-6,
            "{g:?}"
        );
    }

    #[test]
    fn default_state_low_passes() {
        let st = State::new_default();
        let low = run(st.clone(), 100.0);
        let high = run(st, 15000.0);
        assert!(low > 0.2, "{low}");
        assert!(high < low * 0.5, "{low} {high}");
    }

    #[test]
    fn every_type_and_order_plays() {
        for kind in 0..TYPES {
            for order in 0..6 {
                let mut st = State::new_default();
                st.filters[0].kind = kind;
                st.filters[0].order = order;
                st.filters[0].alt = order % 2 == 1;
                st.filters[0].ws = order == 3;
                let v = run(st, 1000.0);
                assert!(v.is_finite() && v < 10.0, "type {kind} order {order}: {v}");
            }
        }
    }

    #[test]
    fn nothing_enabled_is_silence() {
        let mut st = State::new_default();
        st.filters[0].enabled = false;
        assert_eq!(run(st, 440.0), 0.0);
    }

    #[test]
    fn an_lfo_moves_the_cutoff() {
        let mut st = State::new_default();
        st.filters[0].arts[T_CUT].lfo.enabled = true;
        st.filters[0].arts[T_CUT].lfo.keep_love_philter();
        let mut lp = LovePhilter::with_state(st, 44100);
        let mut seen = Vec::new();
        let mut b = vec![0.0f32; 1024];
        for _ in 0..200 {
            lp.process(&mut b);
            seen.push(lp.target(0, T_CUT));
        }
        let (lo, hi) = seen.iter().fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        assert!(hi - lo > 0.2, "{lo} {hi}");
    }

    #[test]
    fn automation_reaches_the_sound() {
        let mut lp = LovePhilter::with_state(State::new_default(), 44100);
        lp.set_param(control_index(0, C_VOL).unwrap(), 0);
        let mut b = sine(440.0, 8192);
        for c in b.chunks_mut(1024) {
            lp.process(c);
        }
        assert!(rms(&b[8192..]) < 1e-3);
    }
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    /// An edit of a control keeps the filters' states (no restart); a type change empties that filter's stages.
    #[test]
    fn edits_keep_the_sound_going() {
        let mut lp = LovePhilter::with_state(State::new_default(), 44100);
        let mut b: Vec<f32> = (0..2048)
            .map(|i| ((i / 2) as f32 * 0.05).sin() * 0.5)
            .collect();
        lp.process(&mut b);
        let low = lp.units[0].st[0].a;
        assert!(low[0] != 0.0);
        let mut st = lp.state().clone();
        st.filters[0].controls[C_CUT] = 200;
        lp.edit(st.clone());
        assert_eq!(lp.units[0].st[0].a, low, "a control edit keeps the stage");
        assert_eq!(lp.units[0].c, 200.0 / 256.0);
        st.filters[0].kind = LIME_LP;
        lp.edit(st.clone());
        assert_eq!(lp.units[0].st[0].a, [0.0; 2]);
        st.filters[0].arts[T_CUT].lfo.enabled = true;
        lp.edit(st);
        assert!(lp.units[0].arts[T_CUT].running);
    }
}
