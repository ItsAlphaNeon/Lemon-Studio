// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's flanger (a recreation of FL's Fruity Flanger mixer effect), configured from its saved state.
//!
//! Per side a short delay line whose read position an LFO sweeps (sine, triangle or a blend of the two, from integer
//! tables), read with a straight line between two samples, smoothed by a one-pole low pass ("damp"), fed back into the
//! line and mixed with the dry signal and the other side's delayed signal ("cross"). The right LFO runs a settable
//! share of a cycle ahead of the left one. The LFO starts at phase 0 whenever the effect starts.
//!
// spec: plugins/fruity-flanger.md, plugins/lemon-flanger.md

use std::sync::OnceLock;

use super::Effect;

/// The settings, in the saved state's order (after the version field).
pub const PARAMS: usize = 12;
pub const DELAY: usize = 0;
pub const DEPTH: usize = 1;
pub const RATE: usize = 2;
pub const PHASE: usize = 3;
pub const DAMP: usize = 4;
pub const SHAPE: usize = 5;
pub const FEED: usize = 6;
pub const INVERT_FEED: usize = 7;
pub const INVERT_WET: usize = 8;
pub const DRY: usize = 9;
pub const WET: usize = 10;
pub const CROSS: usize = 11;

/// The knobs' ranges (spec "Settings"). A stored value outside its range acts as the end it passes (the position is
/// kept within 0..1).
pub const RANGES: [(i32, i32); PARAMS] =
    [(0, 1024), (0, 5000), (0, 5000), (0, 360), (0, 5000), (0, 1024), (0, 100), (0, 1024), (0, 1024), (0, 5000), (0, 5000), (0, 5000)];

/// What a new Fruity Flanger holds (spec "Settings", the Default column).
pub const DEFAULTS: [i32; PARAMS] = [0, 2000, 2000, 118, 0, 0, 23, 1024, 1024, 3500, 3000, 2500];

/// The saved state: the version (1) and the twelve knob values.
pub const STATE_LEN: usize = 52;
pub const VERSION: i32 = 1;

/// The LFO tables' length, and one cycle in phase units.
pub const TABLE: usize = 4096;
pub const CYCLE: u32 = 1 << 24;

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
    /// A saved state: the twelve values after the version field. A version of 2 or more is not read: the instance
    /// keeps a new one's settings. `None` when the state is too short.
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        let version = super::field(b, 0)?;
        let v: [i32; PARAMS] = super::ints(b, 1)?;
        Some(if version > VERSION { Settings::default() } else { Settings { v } })
    }

    /// The saved state (52 bytes).
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

/// The skew curve `C(x, c) = single(((c + 1) x) / (single(c x) + 1))`, the quotient in doubles.
pub fn skew(x: f32, c: f32) -> f32 {
    (((f64::from(c) + 1.0) * f64::from(x)) / (f64::from(c * x) + 1.0)) as f32
}

/// Round half to even, to an integer.
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Delay and depth in ms: `C(x, -0.8) * 20` (0 to 20 ms).
pub fn ms(x: f32) -> f32 {
    skew(x, -0.8) * 20.0
}

/// The rate in Hz: `C(x, -0.9) * 5` (0 to 5 Hz).
pub fn speed(x: f32) -> f32 {
    skew(x, -0.9) * 5.0
}

/// The sweep's centre in 1/4096 of a sample and its half width in whole samples, for delay `d0` and depth `dd` (ms).
pub fn centre_depth(d0: f32, dd: f32, rate: u32) -> (i32, i32) {
    let r = f64::from(rate);
    let centre = ((((f64::from(d0) + f64::from(dd) * 0.5) * r) * 4096.0) / 1000.0) as i32;
    let depth = (((f64::from(dd) * 0.5) * r) / 1000.0) as i32;
    (centre, depth)
}

/// The LFO's step per sample (2^24 a cycle): `Round(((speed * 4096) / rate) * 4096)`, the products in singles.
pub fn increment(speed: f32, rate: u32) -> u32 {
    round(f64::from(((speed * 4096.0) / rate as f32) * 4096.0)) as u32
}

/// How far the right LFO runs ahead, in phase units: `Round(single(x * 16777216))`.
pub fn offset(x: f32) -> u32 {
    round(f64::from(x * 16_777_216.0)) as u32
}

/// The shape's weights (triangle, sine): `Round(single(x * 4096))` and `Round(single(1 - x) * 4096)`.
pub fn weights(x: f32) -> (i32, i32) {
    (round(f64::from(x * 4096.0)) as i32, round(f64::from(1.0 - x) * 4096.0) as i32)
}

/// The invert knobs are switches: on above half the knob.
pub fn switch(x: f32) -> bool {
    x > 0.5
}

/// The delay line's length: the smallest power of two not below `Round(rate / 10)`.
pub fn line_len(rate: u32) -> usize {
    (round(f64::from(rate) / 10.0).max(1) as usize).next_power_of_two()
}

/// The working values of a set of settings at a sample rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Working {
    /// The sweep's centre (1/4096 of a sample) and half width (samples).
    pub centre: i32,
    pub depth: i32,
    pub inc: u32,
    pub offset: u32,
    /// The low pass's coefficient (1: no damping).
    pub damp: f32,
    /// The weights of the triangle and the sine.
    pub w_tri: i32,
    pub w_sine: i32,
    pub fb: f32,
    pub dry: f32,
    pub wet: f32,
    pub cross: f32,
}

impl Working {
    pub fn new(v: &[i32; PARAMS], rate: u32) -> Working {
        let x = |k: usize| position(k, v[k]);
        let (centre, depth) = centre_depth(ms(x(DELAY)), ms(x(DEPTH)), rate);
        let (w_tri, w_sine) = weights(x(SHAPE));
        let fb = (f64::from(x(FEED)) * 0.98) as f32;
        let inv = if switch(x(INVERT_WET)) { -1.0 } else { 1.0 };
        Working {
            centre,
            depth,
            inc: increment(speed(x(RATE)), rate),
            offset: offset(x(PHASE)),
            damp: (1.0 - f64::from(x(DAMP))) as f32,
            w_tri,
            w_sine,
            fb: if switch(x(INVERT_FEED)) { -fb } else { fb },
            dry: x(DRY),
            wet: inv * x(WET),
            cross: inv * x(CROSS),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The LFO (spec "The LFO")

/// The two waves (triangle, sine), 4096 integer points from -32767 to 32767.
pub fn waves() -> &'static [[i32; TABLE]; 2] {
    static WAVES: OnceLock<Box<[[i32; TABLE]; 2]>> = OnceLock::new();
    WAVES.get_or_init(|| {
        let mut w = Box::new([[0i32; TABLE]; 2]);
        // The triangle: d rises from 0, falls from 1 and rises from -1 in exact steps of 1/1024.
        let mut d = 0.0f64;
        for i in 0..TABLE {
            if i == 1024 {
                d = 1.0;
            } else if i == 3072 {
                d = -1.0;
            }
            w[0][i] = (d * 32767.0) as i32;
            d += if (1024..3072).contains(&i) { -1.0 / 1024.0 } else { 1.0 / 1024.0 };
        }
        for i in 0..TABLE {
            w[1][i] = (((i as f64 * 6.283185307) / 4096.0).sin() * 32767.0) as i32;
        }
        w
    })
}

/// The table the LFO reads, blended by the shape's weights: values and slopes.
#[derive(Clone)]
pub struct Table {
    pub value: Box<[i32; TABLE]>,
    pub slope: Box<[i32; TABLE]>,
}

impl Table {
    pub fn new(w_tri: i32, w_sine: i32) -> Table {
        let [tri, sine] = waves();
        let next = |w: &[i32; TABLE], i: usize| w[(i + 1) % TABLE].wrapping_sub(w[i]);
        let mut t = Table { value: Box::new([0; TABLE]), slope: Box::new([0; TABLE]) };
        for i in 0..TABLE {
            t.value[i] = w_tri.wrapping_mul(tri[i]).wrapping_add(w_sine.wrapping_mul(sine[i]));
            t.slope[i] = w_tri.wrapping_mul(next(tri, i)).wrapping_add(w_sine.wrapping_mul(next(sine, i))) >> 12;
        }
        t
    }

    /// The LFO's value at `phase` (2^24 a cycle; the top 8 bits are ignored): about -32767..32767.
    #[inline]
    pub fn at(&self, phase: u32) -> i32 {
        let hi = ((phase >> 12) & (TABLE as u32 - 1)) as usize;
        let lo = (phase & 4095) as i32;
        self.value[hi].wrapping_add(lo.wrapping_mul(self.slope[hi])) >> 12
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The effect

pub struct Flanger {
    applied: [i32; PARAMS],
    rate: u32,
    w: Working,
    table: Table,
    /// The left and right LFOs' phases.
    phase: [u32; 2],
    line: [Vec<f32>; 2],
    mask: u32,
    at: u32,
    /// The low pass's memory per side.
    m: [f32; 2],
}

impl Flanger {
    /// A new instance started as at a render: the lines empty, the left LFO at phase 0.
    pub fn with_settings(rate: u32, s: &Settings) -> Flanger {
        let w = Working::new(&s.v, rate);
        let n = line_len(rate);
        let mut f = Flanger {
            applied: s.v,
            rate,
            table: Table::new(w.w_tri, w.w_sine),
            w,
            phase: [0; 2],
            line: [vec![0.0; n], vec![0.0; n]],
            mask: n as u32 - 1,
            at: 0,
            m: [0.0; 2],
        };
        f.start();
        f
    }

    pub fn from_state(state: &[u8], rate: u32) -> Option<Flanger> {
        Some(Flanger::with_settings(rate, &Settings::from_state(state)?))
    }

    /// The host enables the effect (as at the start of a render): the lines, their write position and the low pass are
    /// cleared, the left LFO goes to phase 0 and the right one to the phase knob's offset.
    pub fn start(&mut self) {
        for l in &mut self.line {
            l.fill(0.0);
        }
        self.at = 0;
        self.m = [0.0; 2];
        self.phase = [0, self.w.offset];
    }

    /// The LFOs' phases (tests).
    pub fn phases(&self) -> [u32; 2] {
        self.phase
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// New settings: they act from the next sample (nothing glides). A new rate or phase puts the right LFO at the
    /// left one plus the offset.
    pub fn set(&mut self, s: &Settings) {
        if s.v != self.applied {
            let old = self.w;
            let moved = s.v[RATE] != self.applied[RATE] || s.v[PHASE] != self.applied[PHASE];
            self.applied = s.v;
            self.w = Working::new(&s.v, self.rate);
            self.new_table(&old);
            if moved {
                self.phase[1] = self.phase[0].wrapping_add(self.w.offset);
            }
        }
    }

    /// A new sample rate: new working values and line length (the lines start empty when their length changes); the
    /// right LFO goes to the left one plus the offset.
    pub fn set_sample_rate(&mut self, rate: u32) {
        let old = self.w;
        self.rate = rate;
        self.w = Working::new(&self.applied, rate);
        let n = line_len(rate);
        if n != self.line[0].len() {
            self.line = [vec![0.0; n], vec![0.0; n]];
            self.mask = n as u32 - 1;
            self.at = 0;
        }
        self.new_table(&old);
        self.phase[1] = self.phase[0].wrapping_add(self.w.offset);
    }

    /// The LFO's table again when the shape's weights changed.
    fn new_table(&mut self, old: &Working) {
        if (old.w_tri, old.w_sine) != (self.w.w_tri, self.w.w_sine) {
            self.table = Table::new(self.w.w_tri, self.w.w_sine);
        }
    }

    /// The working values (tests and the editor).
    pub fn working(&self) -> &Working {
        &self.w
    }

    /// One sample of both sides (spec "Per sample").
    #[inline]
    fn sample(&mut self, l: f32, r: f32) -> (f32, f32) {
        let w = &self.w;
        let input = [l, r];
        let mut tap = [0.0f32; 2];
        for s in 0..2 {
            let v = self.table.at(self.phase[s]);
            self.phase[s] = self.phase[s].wrapping_add(w.inc);
            let pos = (w.depth.wrapping_mul(v) >> 3).wrapping_add(w.centre) as u32;
            let line = &mut self.line[s];
            line[self.at as usize] = input[s] + self.m[s] * w.fb;
            let i = self.at.wrapping_sub(pos >> 12);
            let a = line[(i & self.mask) as usize];
            let b = line[(i.wrapping_sub(1) & self.mask) as usize];
            tap[s] = a + (b - a) * ((pos & 4095) as f32 / 4096.0);
        }
        self.at = (self.at + 1) & self.mask;
        for s in 0..2 {
            self.m[s] = self.m[s] + w.damp * (tap[s] - self.m[s]);
        }
        let [ml, mr] = self.m;
        ((ml * w.wet + l * w.dry) + w.cross * mr, (mr * w.wet + r * w.dry) + w.cross * ml)
    }

    /// One block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            (*l, *r) = self.sample(*l, *r);
        }
    }
}

impl Effect for Flanger {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.chunks_exact_mut(2) {
            (f[0], f[1]) = self.sample(f[0], f[1]);
        }
    }

    /// A slot switched on during the song: FL enables the plugin again, which clears it and restarts the LFO.
    fn reset(&mut self) {
        self.start();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip_and_a_later_version_is_not_read() {
        let s = Settings { v: [300, 4000, 1200, 360, 900, 512, 80, 0, 1024, 5000, 2500, 0] };
        let b = s.to_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(Settings::from_state(&b), Some(s));
        let mut v2 = b.clone();
        v2[0] = 2;
        assert_eq!(Settings::from_state(&v2), Some(Settings::default()));
        assert_eq!(Settings::from_state(&b[..48]), None);
    }

    /// The working values of the spec's formulas, worked by hand from the page.
    #[test]
    fn knobs_become_working_values() {
        // Depth 2000 is x = 0.4: C = 0.2 * 0.4 / (1 - 0.32) = 0.1176..., 2.3529 ms; the rate 0.3125 Hz.
        assert!((ms(position(DEPTH, 2000)) - 2.352_941).abs() < 1e-5);
        assert!((speed(position(RATE, 2000)) - 0.3125).abs() < 1e-6);
        assert_eq!((ms(0.0), ms(1.0), speed(1.0)), (0.0, 20.0, 5.0));
        let w = Working::new(&DEFAULTS, 44100);
        // centre = Trunc((0 + 2.3529 / 2) * 44100 * 4.096) = 212 510, depth = Trunc(1.17647 * 44.1) = 51.
        assert_eq!((w.centre, w.depth), (212_510, 51));
        // inc = Round(0.3125 * 4096 / 44100 * 4096) = Round(118.89) = 119.
        assert_eq!(w.inc, 119);
        // 118 degrees: Round(118 / 360 * 2^24) = 5 499 199 (the single rounds the position first).
        assert_eq!(w.offset, offset(position(PHASE, 118)));
        assert!((i64::from(w.offset) - 5_499_199).abs() <= 1);
        assert_eq!((w.damp, w.w_tri, w.w_sine), (1.0, 0, 4096));
        // Feed 23 %, inverted; wet and cross inverted.
        assert_eq!(w.fb, -((f64::from(position(FEED, 23)) * 0.98) as f32));
        assert_eq!((w.dry, w.wet, w.cross), (0.7, -0.6, -0.5));
        // The invert knobs switch above half: 512 is not, 513 is.
        let mut v = DEFAULTS;
        v[INVERT_FEED] = 512;
        v[INVERT_WET] = 513;
        let w = Working::new(&v, 48000);
        assert!(w.fb > 0.0 && w.wet < 0.0);
        // Every knob at its end at 48 kHz: 20 ms + 10 ms of centre, 10 ms of half width.
        let w = Working::new(&[1024, 5000, 5000, 360, 5000, 1024, 100, 0, 0, 5000, 5000, 5000], 48000);
        assert_eq!((w.centre, w.depth, w.offset, w.damp, w.w_tri, w.w_sine), (5_898_240, 480, CYCLE, 0.0, 4096, 0));
        assert_eq!(w.inc, 1748);
        assert_eq!(w.fb, 0.98);
        // Stored values past the ranges act as the ends.
        assert_eq!(position(DEPTH, 9000), 1.0);
        assert_eq!(position(DELAY, -5), 0.0);
        assert_eq!((line_len(44100), line_len(48000), line_len(96000)), (8192, 8192, 16384));
    }

    #[test]
    fn lfo_waves_and_table() {
        let [tri, sine] = waves();
        let at = |w: &[i32; TABLE]| [0, 1, 1023, 1024, 1025, 2048, 3071, 3072, 3073, 4095].map(|i| w[i]);
        assert_eq!(at(tri), [0, 31, 32735, 32767, 32735, 0, -32735, -32767, -32735, -31]);
        assert_eq!(at(sine), [0, 50, 32766, 32767, 32766, 0, -32766, -32767, -32766, -50]);
        // The sine alone: T = 4096 sine, and the value at a point is the sine's.
        let t = Table::new(0, 4096);
        assert_eq!(t.at(1024 << 12), 32767);
        assert_eq!(t.at(3072 << 12), -32767);
        // Halfway between points 0 and 1: 0 + 2048 * 50 = 102 400 >> 12 = 25; the top 8 bits are ignored.
        assert_eq!(t.at(2048), 25);
        assert_eq!(t.at(0xff00_0000 | 2048), 25);
        // A half blend at point 512: (2048 * 16383 + 2048 * 23169) >> 12.
        let t = Table::new(2048, 2048);
        assert_eq!(t.at(512 << 12), (2048 * tri[512] + 2048 * sine[512]) >> 12);
    }

    /// With the LFO stopped, no delay or depth and no damping, the line's tap is the sample just written: the output is
    /// the input times dry plus wet, plus cross times the other side.
    #[test]
    fn a_stopped_lfo_without_delay_passes_the_input() {
        let mut f = Flanger::with_settings(44100, &Settings { v: [0, 0, 0, 0, 0, 0, 0, 0, 0, 2500, 5000, 0] });
        let (mut l, mut r) = (vec![0.25f32, -0.5, 1.0], vec![0.5f32, 0.125, 0.0]);
        let (l0, r0) = (l.clone(), r.clone());
        f.process(&mut l, &mut r);
        for i in 0..3 {
            assert_eq!(l[i], l0[i] + l0[i] * 0.5);
            assert_eq!(r[i], r0[i] + r0[i] * 0.5);
        }
        // Cross alone: the other side's tap.
        let mut f = Flanger::with_settings(44100, &Settings { v: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5000] });
        let (mut l, mut r) = (vec![0.25f32], vec![0.5f32]);
        f.process(&mut l, &mut r);
        assert_eq!((l[0], r[0]), (0.5, 0.25));
    }

    /// A delay of 2 ms with no depth: an impulse comes back after Trunc(2 ms * 44.1 * 4096) / 4096 samples, split over
    /// two samples by the fraction; the feedback brings it again a delay later, times fb.
    #[test]
    fn an_impulse_comes_back_after_the_delay() {
        // Delay 1024 is 20 ms; find a stored delay of about 2 ms instead and read it back from the working values.
        let mut v = [0, 0, 0, 0, 0, 0, 50, 0, 0, 0, 5000, 0];
        v[DELAY] = 400;
        let mut f = Flanger::with_settings(44100, &Settings { v });
        let centre = f.working().centre;
        let (whole, frac) = ((centre >> 12) as usize, (centre & 4095) as f32 / 4096.0);
        let n = 4 * whole + 8;
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        l[0] = 1.0;
        f.process(&mut l, &mut r);
        assert!(l[..whole].iter().all(|&x| x == 0.0));
        assert_eq!(l[whole], 1.0 - frac);
        assert_eq!(l[whole + 1], frac);
        let fb = f.working().fb;
        assert!(fb > 0.0);
        // The second pass: a + (b - a) f over the first pass's two samples (times fb), spread over the three samples after 2 whole delays.
        let echo: f32 = l[2 * whole..2 * whole + 4].iter().sum();
        assert!((echo - fb).abs() < 1e-6, "{echo} vs {fb}");
        assert!(r.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn the_lfo_starts_at_zero_and_restarts_on_reset() {
        let mut f = Flanger::with_settings(44100, &Settings::default());
        let off = f.working().offset;
        assert_eq!(f.phases(), [0, off]);
        let mut buf = vec![0.1f32; 200];
        Effect::process(&mut f, &mut buf);
        assert_eq!(f.phases(), [100 * 119, off.wrapping_add(100 * 119)]);
        // A new phase puts the right LFO at the left one plus the new offset; another setting leaves it.
        let mut s = Settings::default();
        s.v[PHASE] = 180;
        f.set(&s);
        assert_eq!(f.phases(), [11900, 11900 + (1 << 23)]);
        s.v[WET] = 100;
        f.set(&s);
        assert_eq!(f.phases(), [11900, 11900 + (1 << 23)]);
        Effect::reset(&mut f);
        assert_eq!(f.phases(), [0, 1 << 23]);
        assert!(f.line.iter().all(|l| l.iter().all(|&v| v == 0.0)) && f.m == [0.0; 2] && f.at == 0);
    }

    #[test]
    fn a_slot_reads_its_state() {
        let s = Settings { v: [128, 3000, 2500, 90, 1000, 700, 60, 1024, 0, 2500, 4000, 1000] };
        let mut slot = super::super::from_slot("Fruity Flanger", &s.to_state(), 44100).expect("a flanger");
        let mut own = Flanger::with_settings(44100, &s);
        let input: Vec<f32> = (0..4000).map(|i| ((i as f32) * 0.013).sin() * 0.5).collect();
        let mut buf: Vec<f32> = input.iter().flat_map(|&x| [x, -x * 0.5]).collect();
        slot.process(&mut buf);
        let (mut l, mut r): (Vec<f32>, Vec<f32>) = input.iter().map(|&x| (x, -x * 0.5)).unzip();
        own.process(&mut l, &mut r);
        let mine: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
        assert_eq!(buf, mine);
        assert!(buf.iter().all(|x| x.is_finite()) && buf.iter().any(|&x| x != 0.0));
    }
}
