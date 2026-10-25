// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's stereo enhancer (a recreation of FL's Fruity Stereo Enhancer mixer effect), configured from its saved
//! state.
//!
//! A stereo separation mixes each channel into the other, with a positive weight (narrower) or a negative one (wider);
//! a pan scales one output down, a volume follows, and either output can be inverted. The four gains glide. A phase
//! offset delays one channel by up to half a second, before the gains or after them.
//!
// spec: plugins/fruity-stereo-enhancer.md, plugins/lemon-stereo-enhancer.md

use os_dsp::glide::{plugin_limit, Glide};

use super::Effect;

/// The settings, in the saved state's order.
pub const PARAMS: usize = 6;
pub const PAN: usize = 0;
pub const VOLUME: usize = 1;
pub const SEPARATION: usize = 2;
pub const PHASE: usize = 3;
pub const PLACEMENT: usize = 4;
pub const INVERT: usize = 5;

/// The settings' ranges (stored values are used as they are).
pub const RANGES: [(i32, i32); PARAMS] = [(-128, 128), (0, 320), (-96, 96), (-512, 512), (0, 1), (0, 2)];

/// FL's "Default" preset, also the plugin database's state (what a new Stereo Enhancer holds).
pub const DEFAULTS: [i32; PARAMS] = [0, 256, 0, 0, 1, 0];

/// Delay placements.
pub const PRE: i32 = 0;
pub const POST: i32 = 1;

/// Phase inversions.
pub const INVERT_NONE: i32 = 0;
pub const INVERT_LEFT: i32 = 1;
pub const INVERT_RIGHT: i32 = 2;

/// The saved state: six ints, no version field.
pub const STATE_LEN: usize = 24;

/// Silence FL runs through a slot before the song (2048 or 4096 frames; 4096 lets every glide from the creation gains
/// arrive).
pub const PREROLL: usize = 4096;

/// The delay's factor before a sample rate is known: `single(0.441)` (`0x3EE1CAC1`).
pub const CREATION_RATE_FACTOR: f32 = 0.441;

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
    /// A saved state (24 bytes).
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        Some(Settings { v: super::ints(b, 0)? })
    }

    /// The saved state (24 bytes).
    pub fn to_state(&self) -> Vec<u8> {
        super::state_of(&self.v)
    }
}

/// The volume's gain: the host's level curve of `single(volume) * 0.00390625`, exactly 1 at 256.
pub fn volume_gain(volume: i32) -> f32 {
    os_dsp::level::curve(volume as f32 * 0.00390625)
}

/// The gains' targets `[LL, RR, RL, LR]` (spec "The gains"; singles, in the order written).
pub fn gains(v: &[i32; PARAMS]) -> [f32; 4] {
    let p = v[PAN] as f32 * 0.0078125;
    let s = v[SEPARATION] as f32 * 0.0078125;
    let g = volume_gain(v[VOLUME]) * (1.0 - s * 0.5);
    let (mut ll, mut rr) = (g, g);
    let (mut rl, mut lr) = (g * s, g * s);
    if p < 0.0 {
        rr *= p + 1.0;
        lr *= p + 1.0;
    } else {
        ll *= 1.0 - p;
        rl *= 1.0 - p;
    }
    // An inversion other than 1 or 2 counts as none.
    match v[INVERT] {
        INVERT_LEFT => (ll, rl) = (-ll, -rl),
        INVERT_RIGHT => (rr, lr) = (-rr, -lr),
        _ => {}
    }
    [ll, rr, rl, lr]
}

/// The phase offset's delay in samples for the knob `k` and the rate factor `c = single(rate / 100000)`:
/// `(1001^(|k| / 512) - 1) / 2` ms, rounded (spec "From a setting to its value").
pub fn delay_samples(k: i32, c: f32) -> usize {
    let x = (f64::from(k.unsigned_abs()) / 512.0) as f32;
    let e = ((f64::from(x) * 1001f64.ln()).exp() - 1.0) as f32;
    let t = e * 50.0;
    (t * c).round_ties_even().max(0.0) as usize
}

/// `single(rate / 100000)`.
pub fn rate_factor(sample_rate: u32) -> f32 {
    (f64::from(sample_rate) / 100000.0) as f32
}

/// A delay line of exactly `d` samples (empty: zeros).
#[derive(Clone, Debug, Default)]
struct Line {
    buf: Vec<f32>,
    pos: usize,
}

impl Line {
    fn new(d: usize) -> Line {
        Line { buf: vec![0.0; d], pos: 0 }
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        if self.buf.is_empty() {
            return x;
        }
        let y = std::mem::replace(&mut self.buf[self.pos], x);
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

pub struct StereoEnhancer {
    applied: [i32; PARAMS],
    target: [f32; 4],
    gains: [Glide; 4],
    limit: f32,
    /// The rate factor in force when the phase offset is set.
    c: f32,
    line: Line,
    /// The delayed channel: 0 left, 1 right.
    side: usize,
    started: bool,
}

impl StereoEnhancer {
    /// A new instance: the gains of the editor defaults (`LL = RR = 1`), no delay, the rate already known.
    pub fn new(sample_rate: u32) -> StereoEnhancer {
        let start = gains(&DEFAULTS);
        StereoEnhancer {
            applied: DEFAULTS,
            target: start,
            gains: start.map(Glide::new),
            limit: plugin_limit(sample_rate),
            c: rate_factor(sample_rate),
            line: Line::default(),
            side: 1,
            started: false,
        }
    }

    /// FL's start of a render: created, the rate set, the state applied (its gains glide there in the pre-roll).
    pub fn with_settings(sample_rate: u32, s: &Settings) -> StereoEnhancer {
        let mut e = StereoEnhancer::new(sample_rate);
        for k in 0..PARAMS {
            e.apply(k, s.v[k]);
        }
        e
    }

    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<StereoEnhancer> {
        Some(StereoEnhancer::with_settings(sample_rate, &Settings::from_state(state)?))
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// Applies the settings that differ from the ones applied last, in index order.
    pub fn set(&mut self, s: &Settings) {
        for k in 0..PARAMS {
            if s.v[k] != self.applied[k] {
                self.apply(k, s.v[k]);
            }
        }
    }

    /// Applies setting `k`. A phase offset empties the line and gives it its new length (also an unchanged one); the
    /// placement keeps the line and its contents; the gains get new targets.
    pub fn apply(&mut self, k: usize, v: i32) {
        self.applied[k] = v;
        if k == PHASE {
            self.line = Line::new(delay_samples(v, self.c));
            self.side = if v < 0 { 0 } else { 1 };
        }
        self.target = gains(&self.applied);
    }

    /// A new sample rate: the glide's limit and the delay's factor (the line keeps its length until the phase offset
    /// is set again).
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.limit = plugin_limit(sample_rate);
        self.c = rate_factor(sample_rate);
    }

    /// The host's flush: every gain at its target at once; the line is left alone.
    pub fn flush(&mut self) {
        self.gains = self.target.map(Glide::new);
    }

    /// The delay in samples (tests and the editor).
    pub fn delay(&self) -> usize {
        self.line.buf.len()
    }

    /// The gains as they stand (tests).
    pub fn current(&self) -> [f32; 4] {
        self.gains.map(|g| g.current)
    }

    /// Where the delay acts: before the gains (Pre) or after them; with no delay, nowhere.
    fn pre(&self) -> Option<bool> {
        match self.applied[PLACEMENT] {
            _ if self.line.buf.is_empty() => None,
            p if p < 0 => None,
            0 => Some(true),
            _ => Some(false),
        }
    }

    /// Plans gain `k` over a block of `n` (a difference counts as zero only when it is +0).
    fn plan(&mut self, k: usize, n: usize) -> os_dsp::glide::Ramp {
        let (g, t) = (&mut self.gains[k], self.target[k]);
        let flat_but_signed = g.current == t && (t - g.current).to_bits() != 0;
        let ramp = g.block(t, n, self.limit);
        if flat_but_signed {
            g.current = t;
        }
        ramp
    }

    /// One block in place: the four gains planned once, running sums per sample, the delay before or after them.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        self.block(left.iter_mut().zip(right.iter_mut()), n);
    }

    fn block<'a>(&mut self, frames: impl Iterator<Item = (&'a mut f32, &'a mut f32)>, n: usize) {
        let [mut ll, mut rr, mut rl, mut lr] = [0, 1, 2, 3].map(|k| self.plan(k, n));
        let pre = self.pre();
        let side = self.side;
        for (l, r) in frames {
            let mut x = [*l, *r];
            if pre == Some(true) {
                x[side] = self.line.run(x[side]);
            }
            let (a, b, c, d) = (ll.next(), rr.next(), rl.next(), lr.next());
            let mut y = [x[0] * a + x[1] * c, x[1] * b + x[0] * d];
            if pre == Some(false) {
                y[side] = self.line.run(y[side]);
            }
            (*l, *r) = (y[0], y[1]);
        }
    }

    /// FL's silence before the song, in blocks of 512: the gains glide from the creation values to the state's.
    pub fn start(&mut self) {
        let mut l = [0.0f32; 512];
        let mut r = [0.0f32; 512];
        for _ in 0..PREROLL / 512 {
            l.fill(0.0);
            r.fill(0.0);
            self.process(&mut l, &mut r);
        }
        self.started = true;
    }
}

impl Effect for StereoEnhancer {
    fn process(&mut self, buf: &mut [f32]) {
        if !self.started {
            self.start();
        }
        let n = buf.len() / 2;
        self.block(buf.chunks_exact_mut(2).map(|f| {
            let (l, r) = f.split_at_mut(1);
            (&mut l[0], &mut r[0])
        }), n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip() {
        // FL's "For strings" preset.
        let s = Settings { v: [-18, 256, -54, -248, 1, 0] };
        assert_eq!(Settings::from_state(&s.to_state()), Some(s));
        assert_eq!(s.to_state().len(), STATE_LEN);
    }

    #[test]
    fn the_gain_matrix() {
        assert_eq!(gains(&DEFAULTS), [1.0, 1.0, 0.0, 0.0]);
        // Separation -96: wider, the opposite channel inverted.
        let [ll, rr, rl, lr] = gains(&[0, 256, -96, 0, 1, 0]);
        assert_eq!((ll, rr), (1.375, 1.375));
        assert_eq!((rl, lr), (1.375 * -0.75, 1.375 * -0.75));
        // Pan right halves the left output (both of its gains).
        assert_eq!(gains(&[64, 256, 64, 0, 1, 0]), [0.75 * 0.5, 0.75, 0.75 * 0.5 * 0.5, 0.75 * 0.5]);
        // Inversion of the right side.
        assert_eq!(gains(&[0, 256, 0, 0, 1, 2]), [1.0, -1.0, 0.0, -0.0]);
        assert!((volume_gain(320) - 1.9033).abs() < 1e-4);
        assert_eq!(volume_gain(0), 0.0);
    }

    #[test]
    fn phase_offsets_as_delays() {
        let c = rate_factor(44100);
        assert_eq!(c.to_bits(), 0x3EE1_CAC1);
        assert_eq!(c, CREATION_RATE_FACTOR);
        for k in [-1, 0, 1] {
            assert_eq!(delay_samples(k, c), 0);
        }
        assert_eq!(delay_samples(2, c), 1);
        // 248 gives 604 samples at 44100 Hz and 658 at 48000 Hz; 512 is half a second.
        assert_eq!(delay_samples(248, c), 604);
        assert_eq!(delay_samples(-248, rate_factor(48000)), 658);
        assert_eq!(delay_samples(512, c), 22050);
    }

    #[test]
    fn the_delay_moves_one_channel_before_or_after_the_gains() {
        // Phase +30 after the gains: the right output is late; separation crosses the undelayed left into it first.
        let s = Settings { v: [0, 256, 64, 30, POST, 0] };
        let mut e = StereoEnhancer::with_settings(44100, &s);
        e.start();
        let d = e.delay();
        assert!(d > 10, "{d}");
        let mut l = vec![0.0f32; 200];
        let mut r = vec![0.0f32; 200];
        l[0] = 1.0;
        e.process(&mut l, &mut r);
        let g = gains(&s.v);
        assert_eq!(l[0], g[0]);
        assert_eq!(r[0], 0.0);
        assert_eq!(r[d], g[3]);
        // Before the gains (Pre): the delayed right input also reaches the left output late.
        let mut e = StereoEnhancer::with_settings(44100, &Settings { v: [0, 256, 64, 30, PRE, 0] });
        e.start();
        let mut l = vec![0.0f32; 200];
        let mut r = vec![0.0f32; 200];
        r[0] = 1.0;
        e.process(&mut l, &mut r);
        assert!(l[..d].iter().chain(&r[..d]).all(|&v| v == 0.0));
        assert_eq!((l[d], r[d]), (g[2], g[1]));
    }

    #[test]
    fn gains_glide_from_the_creation_values_in_the_preroll() {
        // Left inverted at volume 320 and separation -96: LL goes from 1 to -2.617, 3617 samples at 0.001.
        let s = Settings { v: [0, 320, -96, 0, 1, 1] };
        let mut e = StereoEnhancer::with_settings(44100, &s);
        assert_eq!(e.current(), [1.0, 1.0, 0.0, 0.0]);
        e.start();
        assert_eq!(e.current(), gains(&s.v));
        // A new phase offset empties the line, an unchanged one too.
        let mut e = StereoEnhancer::with_settings(44100, &Settings { v: [0, 256, 0, 100, 1, 0] });
        e.start();
        let mut l = vec![0.0f32; 100];
        let mut r = vec![1.0f32; 100];
        e.process(&mut l, &mut r);
        assert_eq!(r[99], 1.0, "the line holds the ones");
        e.apply(PHASE, 100);
        let mut l = vec![0.0f32; 4];
        let mut r = vec![0.0f32; 4];
        e.process(&mut l, &mut r);
        assert!(r.iter().all(|&v| v == 0.0));
    }
}
