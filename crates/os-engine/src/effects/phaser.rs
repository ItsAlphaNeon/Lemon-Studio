// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's phaser (a recreation of FL's Fruity Phaser mixer effect), configured from its saved state.
//!
//! Per side a chain of 3 to 23 first-order all-pass sections whose common coefficient a sine LFO sweeps, with feedback
//! from the chain's end to its input, mixed with the dry signal. The right LFO runs up to half a cycle ahead of the
//! left one. A song starts with the LFO as after [`IDLE`] frames of running.
//!
// spec: plugins/fruity-phaser.md, plugins/lemon-phaser.md

use std::f64::consts::TAU;
use std::sync::OnceLock;

use super::Effect;

/// The settings, in the saved state's order (after the version field).
pub const PARAMS: usize = 9;
pub const SWEEP: usize = 0;
pub const MIN_DEPTH: usize = 1;
pub const MAX_DEPTH: usize = 2;
pub const RANGE: usize = 3;
pub const STEREO: usize = 4;
pub const STAGES: usize = 5;
pub const FEEDBACK: usize = 6;
pub const DRY_WET: usize = 7;
pub const GAIN: usize = 8;

/// The knobs' ranges (spec "Settings").
pub const RANGES: [(i32, i32); PARAMS] = [(0, 5000), (0, 1000), (0, 1000), (0, 1024), (0, 1024), (0, 22), (0, 1000), (0, 1024), (0, 5000)];

/// What a new Fruity Phaser holds (spec "Settings", the Default column).
pub const DEFAULTS: [i32; PARAMS] = [2500, 100, 800, 0, 512, 7, 400, 512, 4000];

/// The saved state: the version (1) and the nine knob values.
pub const STATE_LEN: usize = 40;
pub const VERSION: i32 = 1;

/// The most all-pass sections a side has.
pub const MAX_STAGES: usize = 23;

/// The LFO table's length.
pub const TABLE: usize = 4096;

/// The frames the LFO has run when a song starts (Lemon Studio choice; spec "The LFO", `plugins/lemon-phaser.md` §5):
/// a fixed start makes renders repeat.
pub const IDLE: u32 = 512;

/// Everything the saved state says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub v: [i32; PARAMS],
}

impl Default for Settings {
    fn default() -> Self {
        Settings { v: DEFAULTS }
    }
}

impl Settings {
    /// A saved state: the nine values after the version field. A version of 2 or more is not read: the instance keeps
    /// a new one's settings. `None` when the state is too short.
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        let version = super::field(b, 0)?;
        let v: [i32; PARAMS] = super::ints(b, 1)?;
        Some(if version > VERSION { Settings::default() } else { Settings { v } })
    }

    /// The saved state (40 bytes).
    pub fn to_state(&self) -> Vec<u8> {
        let mut b = super::state_of(&[VERSION]);
        b.extend(super::state_of(&self.v));
        b
    }
}

// ---------------------------------------------------------------------------------------------------------------
// From a knob to its working value (spec "Settings", "Parameter laws")

/// Knob `k`'s position: `single(clamp(value * (1 / max), 0, 1))` in doubles.
pub fn position(k: usize, v: i32) -> f32 {
    (f64::from(v) * (1.0 / f64::from(RANGES[k].1))).clamp(0.0, 1.0) as f32
}

/// Round half to even, to an integer.
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// The frequency range is 10 Hz above half the knob, 2 Hz otherwise.
pub fn wide(x: f32) -> bool {
    x > 0.5
}

/// The sweep frequency in Hz: `single(((exp(single(x * 4)) - 1) / (exp(4) - 1)) * R)` in doubles.
pub fn speed(x: f32, wide_range: bool) -> f32 {
    let r = if wide_range { 10.0 } else { 2.0 };
    (((f64::from(x * 4.0).exp() - 1.0) / (4f64.exp() - 1.0)) * r) as f32
}

/// The number of sections: the stored value plus 1, limited to 3..23.
pub fn stages(stored: i32) -> usize {
    stored.saturating_add(1).clamp(3, MAX_STAGES as i32) as usize
}

/// The LFO's step per sample (2^24 a cycle): `Round(single(speed * k))`, `k = single((4096 / rate) * 4096)`.
pub fn increment(speed: f32, rate: u32) -> u32 {
    let k = ((4096.0 / f64::from(rate)) * 4096.0) as f32;
    round(f64::from(speed * k)) as u32
}

/// How far the right LFO runs ahead, in phase units: `Round(x * 8388608)` (up to half a cycle).
pub fn offset(x: f32) -> u32 {
    round(f64::from(x) * 8_388_608.0) as u32
}

/// The output gain: `single(x * 2 * sqrt(single(1 - single(f * f))))`, `f` the feedback knob's position.
pub fn gain(x: f32, f: f32) -> f32 {
    (f64::from(x) * 2.0 * f64::from(1.0 - f * f).sqrt()) as f32
}

/// The working values of a set of settings at a sample rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Working {
    pub inc: u32,
    pub offset: u32,
    /// The depths' positions: the coefficient runs from `-min` (LFO at 0) to `-max` (LFO at 1).
    pub min: f32,
    pub max: f32,
    pub stages: usize,
    pub fb: f32,
    /// The dry share (0: only the chain's output).
    pub dry: f32,
    pub gain: f32,
}

impl Working {
    pub fn new(v: &[i32; PARAMS], rate: u32) -> Working {
        let x = |k: usize| position(k, v[k]);
        Working {
            inc: increment(speed(x(SWEEP), wide(x(RANGE))), rate),
            offset: offset(x(STEREO)),
            min: x(MIN_DEPTH),
            max: x(MAX_DEPTH),
            stages: stages(v[STAGES]),
            fb: (f64::from(x(FEEDBACK)) * 0.99) as f32,
            dry: x(DRY_WET),
            gain: gain(x(GAIN), x(FEEDBACK)),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The LFO (spec "The LFO")

/// The wave (0..1) and its slopes.
pub fn table() -> &'static ([f32; TABLE], [f32; TABLE]) {
    static T: OnceLock<Box<([f32; TABLE], [f32; TABLE])>> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = Box::new(([0.0f32; TABLE], [0.0f32; TABLE]));
        // The true 2 pi (the original's is a hair off it, so its exports differ slightly).
        for i in 0..TABLE {
            t.0[i] = ((((i as f64 / 4096.0) * TAU).sin() + 1.0) * 0.5) as f32;
        }
        for i in 0..TABLE {
            t.1[i] = t.0[(i + 1) % TABLE] - t.0[i];
        }
        t
    })
}

/// The LFO's value at `phase` (2^24 a cycle; the top 8 bits are ignored), 0..1.
#[inline]
pub fn lfo(phase: u32) -> f32 {
    let (w, d) = table();
    let hi = ((phase >> 12) & (TABLE as u32 - 1)) as usize;
    let f = (phase & 4095) as f32 / 4096.0;
    w[hi] + f * d[hi]
}

// ---------------------------------------------------------------------------------------------------------------
// The effect

/// One side's chain: the sections' last outputs, the previous input and the feedback value.
#[derive(Clone, Copy, Debug, Default)]
struct Chain {
    y: [f32; MAX_STAGES],
    before: f32,
    m: f32,
}

pub struct Phaser {
    applied: [i32; PARAMS],
    rate: u32,
    w: Working,
    phase: [u32; 2],
    chain: [Chain; 2],
}

impl Phaser {
    /// A new instance started as at a render: the filters empty, the LFO as after [`IDLE`] frames from phase 0.
    pub fn with_settings(rate: u32, s: &Settings) -> Phaser {
        let mut p = Phaser { applied: s.v, rate, w: Working::new(&s.v, rate), phase: [0; 2], chain: [Chain::default(); 2] };
        p.start(IDLE);
        p
    }

    pub fn from_state(state: &[u8], rate: u32) -> Option<Phaser> {
        Some(Phaser::with_settings(rate, &Settings::from_state(state)?))
    }

    /// The filters emptied and the left LFO put where `idle` frames from phase 0 (at the current increment) take it.
    pub fn start(&mut self, idle: u32) {
        self.chain = [Chain::default(); 2];
        self.phase[0] = idle.wrapping_mul(self.w.inc);
        self.phase[1] = self.phase[0].wrapping_add(self.w.offset);
    }

    /// The LFOs' phases (tests).
    pub fn phases(&self) -> [u32; 2] {
        self.phase
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// New settings: they act from the next block (nothing glides).
    pub fn set(&mut self, s: &Settings) {
        if s.v != self.applied {
            self.applied = s.v;
            self.w = Working::new(&s.v, self.rate);
        }
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        self.rate = rate;
        self.w = Working::new(&self.applied, rate);
    }

    /// The working values (tests and the editor).
    pub fn working(&self) -> &Working {
        &self.w
    }

    /// One side's sample (spec "Per sample"): the coefficient from the LFO, the chain, the mix.
    #[inline]
    fn side(w: &Working, chain: &mut Chain, phase: &mut u32, s: f32) -> f32 {
        let v = lfo(*phase);
        *phase = phase.wrapping_add(w.inc);
        let a = (-w.min) - v * (w.max - w.min);
        let input = s + chain.m * w.fb;
        let (mut x, mut x_before) = (input, chain.before);
        for y in &mut chain.y[..w.stages] {
            let out = a * (x - *y) + x_before;
            x_before = *y;
            *y = out;
            x = out;
        }
        chain.before = input;
        chain.m = x;
        (w.dry * (s - x) + x) * w.gain
    }

    /// The start of a block: the right LFO at the left one plus the stereo offset.
    fn begin_block(&mut self) {
        self.phase[1] = self.phase[0].wrapping_add(self.w.offset);
    }

    /// One block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.begin_block();
        let w = self.w;
        let [pl, pr] = &mut self.phase;
        let [cl, cr] = &mut self.chain;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l = Phaser::side(&w, cl, pl, *l);
            *r = Phaser::side(&w, cr, pr, *r);
        }
    }
}

impl Effect for Phaser {
    fn process(&mut self, buf: &mut [f32]) {
        self.begin_block();
        let w = self.w;
        let [pl, pr] = &mut self.phase;
        let [cl, cr] = &mut self.chain;
        for f in buf.chunks_exact_mut(2) {
            f[0] = Phaser::side(&w, cl, pl, f[0]);
            f[1] = Phaser::side(&w, cr, pr, f[1]);
        }
    }

    // A slot switched on during the song: the plugin ignores the enable message, so nothing is cleared and the LFO
    // runs on (the default `reset` does nothing).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip_and_a_later_version_is_not_read() {
        let s = Settings { v: [5000, 0, 1000, 1024, 1024, 22, 1000, 0, 5000] };
        let b = s.to_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(Settings::from_state(&b), Some(s));
        let mut v2 = b.clone();
        v2[0] = 2;
        assert_eq!(Settings::from_state(&v2), Some(Settings::default()));
        assert_eq!(Settings::from_state(&b[..36]), None);
    }

    /// The working values of the spec's formulas.
    #[test]
    fn knobs_become_working_values() {
        // 2500 is 0.238 Hz on the 2 Hz range; 5000 gives 2 or 10 Hz; 0 stops the LFO.
        assert!((speed(0.5, false) - 0.238_405_8).abs() < 1e-6, "{}", speed(0.5, false));
        assert_eq!((speed(1.0, false), speed(1.0, true), speed(0.0, true)), (2.0, 10.0, 0.0));
        // k = 380.43573 at 44100 Hz.
        assert_eq!(((4096.0 / 44100.0f64) * 4096.0) as f32, 380.435_73);
        let w = Working::new(&DEFAULTS, 44100);
        assert_eq!(w.inc, increment(speed(0.5, false), 44100));
        assert_eq!(w.inc, 91, "Round(0.2384 * 380.44) = Round(90.7)");
        assert_eq!(w.offset, 1 << 22, "a quarter cycle: half of the half cycle");
        assert_eq!((w.min, w.max, w.stages, w.dry), (0.1, 0.8, 8, 0.5));
        assert_eq!(w.fb, (0.4f64 * 0.99) as f32);
        // Gain 4000 (x 0.8) with feedback 0.4: 1.6 * sqrt(0.84).
        assert!((w.gain - 1.6 * 0.84f32.sqrt()).abs() < 1e-6);
        // The range switches above half: 512 is not, 513 is.
        let mut v = DEFAULTS;
        v[RANGE] = 513;
        assert_eq!(Working::new(&v, 44100).inc, increment(speed(0.5, true), 44100));
        // Stages: the stored value plus 1, within 3..23; stored values outside the range too.
        assert_eq!([0, 1, 2, 7, 22, 40, -9].map(stages), [3, 3, 3, 8, 23, 23, 3]);
        // Feedback 1000 makes the gain 0.
        let mut v = DEFAULTS;
        v[FEEDBACK] = 1000;
        assert_eq!(Working::new(&v, 44100).gain, 0.0);
        // Stereo 1024: half a cycle.
        v[STEREO] = 1024;
        assert_eq!(Working::new(&v, 48000).offset, 1 << 23);
    }

    #[test]
    fn the_lfo_table() {
        let (w, d) = table();
        assert_eq!((w[0], w[1024], w[3072]), (0.5, 1.0, 0.0));
        assert!((w[2048] - 0.5).abs() < 1e-6);
        assert_eq!(d[4095], w[0] - w[4095]);
        // Between points, a straight line; the top 8 bits are ignored.
        assert_eq!(lfo(0), 0.5);
        assert_eq!(lfo(1024 << 12), 1.0);
        assert_eq!(lfo(0xff00_0000 | (5 << 12) | 2048), w[5] + 0.5 * d[5]);
    }

    /// With the LFO stopped and both depths at 0 the sections pass their previous input (a = 0): the chain is a delay
    /// of N samples; the output is (dry (s - m) + m) g.
    #[test]
    fn a_zero_coefficient_makes_the_chain_a_delay() {
        // Sweep 0, depths 0, 3 stages (stored 2), no feedback, dry - wet 0 (only the chain), gain 2500 (x 0.5: g = 1).
        let mut p = Phaser::with_settings(44100, &Settings { v: [0, 0, 0, 0, 0, 2, 0, 0, 2500] });
        assert_eq!((p.working().inc, p.working().gain, p.working().stages), (0, 1.0, 3));
        let (mut l, mut r) = (vec![1.0f32, 0.5, 0.0, 0.0, 0.0, 0.0], vec![0.0f32; 6]);
        p.process(&mut l, &mut r);
        assert_eq!(l, [0.0, 0.0, 0.0, 1.0, 0.5, 0.0]);
        assert_eq!(r, [0.0; 6]);
    }

    /// The all-pass chain keeps a sine's level: after it settles, a slow sine through the chain alone comes out at the
    /// same peak (the coefficient held at -0.5).
    #[test]
    fn the_chain_passes_every_frequency_at_unity() {
        let mut p = Phaser::with_settings(44100, &Settings { v: [0, 500, 500, 0, 0, 7, 0, 0, 2500] });
        let n = 20000;
        let mut l: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        let mut r = l.clone();
        p.process(&mut l, &mut r);
        let peak = l[n / 2..].iter().fold(0.0f32, |m, &x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 1e-3, "{peak}");
        assert_eq!(l, r);
    }

    #[test]
    fn the_lfo_starts_after_the_idle_frames_and_runs_on() {
        let p = Phaser::with_settings(44100, &Settings::default());
        assert_eq!(p.phases(), [512 * 91, 512 * 91 + (1 << 22)]);
        let mut p = Phaser::with_settings(44100, &Settings::default());
        p.start(0);
        assert_eq!(p.phases(), [0, 1 << 22]);
        let mut buf = vec![0.0f32; 1000];
        Effect::process(&mut p, &mut buf);
        assert_eq!(p.phases(), [500 * 91, 500 * 91 + (1 << 22)]);
        // A switched-on slot: the plugin ignores it.
        p.chain[0].m = 0.25;
        Effect::reset(&mut p);
        assert_eq!((p.phases()[0], p.chain[0].m), (500 * 91, 0.25));
        // The right LFO is set from the left one at every block's start.
        p.phase[1] = 7;
        Effect::process(&mut p, &mut [0.0f32; 2]);
        assert_eq!(p.phases(), [501 * 91, 501 * 91 + (1 << 22)]);
    }

    #[test]
    fn a_slot_reads_its_state() {
        let s = Settings { v: [3000, 200, 900, 700, 300, 11, 600, 300, 3500] };
        let mut slot = super::super::from_slot("Fruity Phaser", &s.to_state(), 48000).expect("a phaser");
        let mut own = Phaser::with_settings(48000, &s);
        let input: Vec<f32> = (0..4000).map(|i| ((i * 37 % 101) as f32 / 101.0) - 0.5).collect();
        let mut buf: Vec<f32> = input.iter().flat_map(|&x| [x, x * 0.25]).collect();
        slot.process(&mut buf);
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = input.iter().map(|&x| (x, x * 0.25)).unzip();
        own.process(&mut l, &mut r);
        let mine: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
        assert_eq!(buf, mine);
        assert!(buf.iter().all(|x| x.is_finite()) && buf.iter().any(|&x| x != 0.0));
    }
}
