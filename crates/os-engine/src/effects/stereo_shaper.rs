// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's stereo shaper (a recreation of FL's Fruity Stereo Shaper mixer effect), configured from its saved
//! state.
//!
//! A 2 x 2 matrix mixes the two channels with four gliding gains, each with a sign. A delay of whole samples and a
//! second-order all-pass ("phase") act on one channel, before the matrix or after it. The input minus the output can be
//! added to another track (the difference output, which the engine carries: [`Effect::extra`]).
//!
// spec: plugins/fruity-stereo-shaper.md, plugins/lemon-stereo-shaper.md

use os_dsp::glide::{plugin_limit, Glide};

use super::stereo_enhancer::rate_factor;
use super::Effect;

/// The settings, in the saved state's order (after the version field).
pub const PARAMS: usize = 8;
pub const RIGHT_INTO_LEFT: usize = 0;
pub const LEFT: usize = 1;
pub const RIGHT: usize = 2;
pub const LEFT_INTO_RIGHT: usize = 3;
pub const DELAY: usize = 4;
pub const PHASE: usize = 5;
pub const DIFFERENCE: usize = 6;
pub const POSITION: usize = 7;

/// The settings' ranges (spec "Settings"; stored values are used as they are). The difference output's range is the
/// editor's (FL lists the routes of the track; 126 is the most a track can have).
pub const RANGES: [(i32, i32); PARAMS] =
    [(-25600, 25600), (-25600, 25600), (-25600, 25600), (-25600, 25600), (-4096, 4096), (-4096, 4096), (0, 126), (-1, 1)];

/// FL's "Default" preset (what a new Stereo Shaper holds): the levels at 100 %, nothing else, the effects "post".
pub const DEFAULTS: [i32; PARAMS] = [0, 12800, 12800, 0, 0, 0, 0, 1];

/// Effects positions.
pub const OFF: i32 = -1;
pub const PRE: i32 = 0;
pub const POST: i32 = 1;

/// The saved state: the version (0, but any is read) and the eight values.
pub const STATE_LEN: usize = 36;
pub const VERSION: i32 = 0;

/// The level that is unity.
pub const UNITY: i32 = 12800;

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
    /// A saved state: the eight values after the version field, whatever the version. `None` when it is too short.
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        super::field(b, 0)?;
        Some(Settings { v: super::ints(b, 1)? })
    }

    /// The saved state (36 bytes).
    pub fn to_state(&self) -> Vec<u8> {
        let mut b = super::state_of(&[VERSION]);
        b.extend(super::state_of(&self.v));
        b
    }
}

/// A level slider's gain: `(3^|x| - 1) / 2` with the sign of `x = single(value / 12800)` (spec "Matrix"): exactly 1 at
/// 100 %, 4 at 200 %, 0 at 0.
pub fn level_gain(v: i32) -> f32 {
    let x = (f64::from(v) / 12800.0) as f32;
    if x == 0.0 {
        return 0.0;
    }
    let g = (((f64::from(x.abs() * (3f64.ln() as f32))).exp() - 1.0) * 0.5) as f32;
    if x < 0.0 {
        -g
    } else {
        g
    }
}

/// The gains' targets `[L, R, RL, LR]`.
pub fn gains(v: &[i32; PARAMS]) -> [f32; 4] {
    [LEFT, RIGHT, RIGHT_INTO_LEFT, LEFT_INTO_RIGHT].map(|k| level_gain(v[k]))
}

/// The delay in samples for setting `k` and the rate factor `c = single(rate / 100000)`: `(1001^(|k| / 4096) - 1) / 2`
/// ms, rounded (spec "Delay").
pub fn delay_samples(k: i32, c: f32) -> usize {
    let x = (f64::from(k.unsigned_abs()) / 4096.0) as f32;
    let e = ((f64::from(x) * 1001f64.ln()).exp() - 1.0) as f32;
    let t = e * 50.0;
    (t * c).round_ties_even().max(0.0) as usize
}

/// The channel a delay or phase setting acts on: the right for a positive one, the left for a negative one.
fn side(k: i32) -> usize {
    usize::from(k >= 0)
}

/// The all-pass's frequency for setting `k` (not 0) before the rate's limit: `single(single(exp(u * ln 5001) - 1) *
/// 4.398 + 10)` with `u = single(1 - |k| / 4096)`, 22 kHz just off the centre to 10 Hz at either end.
pub fn phase_hz(k: i32) -> f32 {
    let u = (1.0 - f64::from(k.unsigned_abs()) / 4096.0) as f32;
    let e = ((f64::from(u) * 5001f64.ln()).exp() - 1.0) as f32;
    (f64::from(e) * 4.398 + 10.0) as f32
}

/// The all-pass's frequency and coefficients (spec "Phase").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AllPass {
    pub hz: f32,
    pub a1: f64,
    pub a2: f64,
}

impl AllPass {
    /// `None` for a setting of 0 (off).
    pub fn new(k: i32, rate: u32) -> Option<AllPass> {
        if k == 0 {
            return None;
        }
        let hz = phase_hz(k).min((f64::from(rate) * 0.4999) as f32);
        let w = f64::from(hz) * ((2.0 * std::f64::consts::PI) / f64::from(rate));
        let k = 1.0 / (w.sin() + 1.0);
        Some(AllPass { hz, a1: -2.0 * k * w.cos(), a2: k * (1.0 - w.sin()) })
    }
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

pub struct StereoShaper {
    applied: [i32; PARAMS],
    target: [f32; 4],
    gains: [Glide; 4],
    limit: f32,
    rate: u32,
    /// The rate factor in force when the delay is set.
    c: f32,
    line: Line,
    /// The delayed channel, and the channel the all-pass acts on: 0 left, 1 right.
    delayed: usize,
    phased: usize,
    allpass: Option<AllPass>,
    /// The all-pass's one memory: the last two inputs and outputs `[x1, x2, y1, y2]` (doubles).
    mem: [f64; 4],
    /// The slot has started: its gains were put at their targets (on at the song's start) or it was switched on.
    primed: bool,
    /// The difference output of the block processed last (interleaved).
    diff: Vec<f32>,
}

impl StereoShaper {
    /// A new instance, the rate known: the editor defaults, the matrix at zero (it has not rendered anything yet).
    pub fn new(rate: u32) -> StereoShaper {
        StereoShaper {
            applied: DEFAULTS,
            target: gains(&DEFAULTS),
            gains: [Glide::new(0.0); 4],
            limit: plugin_limit(rate),
            rate,
            c: rate_factor(rate),
            line: Line::default(),
            delayed: 1,
            phased: 1,
            allpass: None,
            mem: [0.0; 4],
            primed: false,
            diff: Vec::new(),
        }
    }

    /// A new instance with a state applied (its gains still at zero: see [`StereoShaper::flush`]).
    pub fn with_settings(rate: u32, s: &Settings) -> StereoShaper {
        let mut e = StereoShaper::new(rate);
        for k in 0..PARAMS {
            e.apply(k, s.v[k]);
        }
        e
    }

    pub fn from_state(state: &[u8], rate: u32) -> Option<StereoShaper> {
        Some(StereoShaper::with_settings(rate, &Settings::from_state(state)?))
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

    /// Applies setting `k`. A delay replaces the line by an empty one (also one of the same length); a phase changes
    /// the coefficients at once and keeps the memory, which 0 clears; a level gives its gain a new target.
    pub fn apply(&mut self, k: usize, v: i32) {
        self.applied[k] = v;
        match k {
            DELAY => {
                self.line = Line::new(delay_samples(v, self.c));
                self.delayed = side(v);
            }
            PHASE => {
                self.allpass = AllPass::new(v, self.rate);
                self.phased = side(v);
                if v == 0 {
                    self.mem = [0.0; 4];
                }
            }
            _ => self.target = gains(&self.applied),
        }
    }

    /// A new sample rate: the glide's limit, and the delay and the all-pass worked out again for it (the line empty).
    pub fn set_sample_rate(&mut self, rate: u32) {
        self.rate = rate;
        self.limit = plugin_limit(rate);
        self.c = rate_factor(rate);
        self.apply(DELAY, self.applied[DELAY]);
        self.allpass = AllPass::new(self.applied[PHASE], rate);
    }

    /// Every gain at its target at once.
    pub fn flush(&mut self) {
        self.gains = self.target.map(Glide::new);
    }

    /// The delay line and the all-pass memory emptied.
    pub fn clear(&mut self) {
        self.line = Line::new(self.line.buf.len());
        self.mem = [0.0; 4];
    }

    /// The delay in samples (tests and the editor).
    pub fn delay(&self) -> usize {
        self.line.buf.len()
    }

    /// The all-pass in force (tests and the editor).
    pub fn allpass(&self) -> Option<AllPass> {
        self.allpass
    }

    /// The gains `[L, R, RL, LR]` as they stand (tests).
    pub fn current(&self) -> [f32; 4] {
        self.gains.map(|g| g.current)
    }

    /// Where the delay and the all-pass act: before the matrix (`Some(true)`), after it, or not at all.
    fn pre(&self) -> Option<bool> {
        match self.applied[POSITION] {
            p if p < 0 => None,
            0 => Some(true),
            _ => Some(false),
        }
    }

    /// The delay, then the all-pass, on one frame.
    #[inline]
    fn effects(&mut self, x: &mut [f32; 2]) {
        let d = self.delayed;
        x[d] = self.line.run(x[d]);
        if let Some(ap) = self.allpass {
            let [x1, x2, y1, y2] = self.mem;
            let s = f64::from(x[self.phased]);
            let y = s * ap.a2 + (((x1 * ap.a1 - y2 * ap.a2) + x2) - y1 * ap.a1);
            self.mem = [s, x1, y, y1];
            x[self.phased] = y as f32;
        }
    }

    /// The end of a block: an all-pass memory value under 2^-24 in magnitude is set to 0.
    fn end_block(&mut self) {
        for m in &mut self.mem {
            if m.abs() < 1.0 / 16_777_216.0 {
                *m = 0.0;
            }
        }
    }

    /// One block in place (the plugin; no difference output).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        self.block(left.iter_mut().zip(right.iter_mut()), n, None);
    }

    fn block<'a>(&mut self, frames: impl Iterator<Item = (&'a mut f32, &'a mut f32)>, n: usize, mut diff: Option<&mut [f32]>) {
        let [mut l_, mut r_, mut rl_, mut lr_] = [0, 1, 2, 3].map(|k| self.gains[k].block(self.target[k], n, self.limit));
        // With no gain moving and no cross level, the cross terms are left out (which keeps a -0).
        let straight = [l_, r_, rl_, lr_].iter().all(|r| r.is_flat()) && rl_.first() == 0.0 && lr_.first() == 0.0;
        let pre = self.pre();
        for (i, (l, r)) in frames.enumerate() {
            let input = [*l, *r];
            let mut x = input;
            if pre == Some(true) {
                self.effects(&mut x);
            }
            let (gl, gr, grl, glr) = (l_.next(), r_.next(), rl_.next(), lr_.next());
            let mut y = if straight { [gl * x[0], gr * x[1]] } else { [gl * x[0] + grl * x[1], gr * x[1] + glr * x[0]] };
            if pre == Some(false) {
                self.effects(&mut y);
            }
            (*l, *r) = (y[0], y[1]);
            if let Some(d) = diff.as_deref_mut() {
                d[2 * i] = input[0] - y[0];
                d[2 * i + 1] = input[1] - y[1];
            }
        }
        self.end_block();
    }
}

impl Effect for StereoShaper {
    /// A slot that is on when the song starts has been running: its matrix is at the setting.
    fn process(&mut self, buf: &mut [f32]) {
        if !self.primed {
            self.flush();
            self.primed = true;
        }
        let n = buf.len() / 2;
        let mut diff = std::mem::take(&mut self.diff);
        diff.resize(buf.len(), 0.0);
        self.block(
            buf.chunks_exact_mut(2).map(|f| {
                let (l, r) = f.split_at_mut(1);
                (&mut l[0], &mut r[0])
            }),
            n,
            Some(&mut diff[..]),
        );
        self.diff = diff;
    }

    /// Switched on during the song: the matrix stays on the gains it last worked out (zero if it never ran, so it
    /// glides from there), the all-pass memory is cleared, the delay line keeps what is in it.
    fn reset(&mut self) {
        self.mem = [0.0; 4];
        self.primed = true;
    }

    fn extra(&self) -> Option<(usize, &[f32])> {
        usize::try_from(self.applied[DIFFERENCE]).ok().filter(|&n| n > 0).map(|n| (n, &self.diff[..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip_whatever_the_version() {
        // FL's "Stereoize 3" preset.
        let s = Settings { v: [0, 12800, 12800, 0, 1408, -2496, 0, 1] };
        let b = s.to_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(Settings::from_state(&b), Some(s));
        let mut v7 = b.clone();
        v7[0] = 7;
        assert_eq!(Settings::from_state(&v7), Some(s));
        assert_eq!(Settings::from_state(&b[..32]), None);
    }

    #[test]
    fn level_gains() {
        let bits = |v: i32| level_gain(v).to_bits();
        assert_eq!(level_gain(0).to_bits(), 0);
        assert_eq!((level_gain(12800), level_gain(-12800)), (1.0, -1.0));
        assert_eq!((level_gain(25600), level_gain(-25600)), (4.0, -4.0));
        assert_eq!((bits(1), bits(-1)), (0x3834_011e, 0xb834_011e));
        // 63.1 % is a half (a hair above).
        assert_eq!((bits(8076), bits(-8076)), (0x3f00_008f, 0xbf00_008f));
        assert_eq!((bits(6400), bits(19200), bits(100)), (0x3ebb_67af, 0x4006_46e2, 0x3b8d_3a43));
        assert_eq!(gains(&DEFAULTS), [1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn delays() {
        let c = rate_factor(44100);
        for k in [0, 1, -1, 2] {
            assert_eq!(delay_samples(k, c), 0);
        }
        assert_eq!(delay_samples(100, c), 4);
        assert_eq!(delay_samples(2416, c), 1276);
        assert_eq!(delay_samples(-1952, c), 571);
        assert_eq!(delay_samples(-22, c), 1);
        // 4096 is half a second.
        assert_eq!(delay_samples(4096, c), 22050);
        assert_eq!(delay_samples(4096, rate_factor(48000)), 24000);
        assert_eq!(delay_samples(240, rate_factor(96000)), 24);
    }

    #[test]
    fn allpass_coefficients() {
        assert_eq!(AllPass::new(0, 44100), None);
        let near = |a: f64, b: f64| (a - b).abs() <= 1e-15 * b.abs().max(1.0);
        for (k, rate, hz, a1, a2) in [
            (1, 44100, 21954.310546875, 1.9729173217630334, 0.9731006896281552),
            (-1, 44100, 21954.310546875, 1.9729173217630334, 0.9731006896281552),
            (864, 44100, 3653.49658203125, -1.1587891148062024, 0.3356980531483356),
            (1744, 44100, 590.8272094726562, -1.8383510023135674, 0.8448836019268244),
            (2080, 48000, 296.5964660644531, -1.9238204693033076, 0.9252712995275999),
            (4096, 44100, 10.0, -1.9971525108644803, 0.9971545379130746),
            (-2496, 44100, 128.1198272705078, -1.9638212342242898, 0.9641484599975532),
            // Held to rate * 0.4999.
            (1, 8000, 3999.199951171875, 1.9987436809065613, 0.9987440754909773),
        ] {
            let ap = AllPass::new(k, rate).unwrap();
            assert_eq!(ap.hz, hz as f32, "{k} at {rate}");
            assert!(near(ap.a1, a1) && near(ap.a2, a2), "{k} at {rate}: {ap:?}");
        }
    }

    /// Interleaves two channels.
    fn frames(l: &[f32], r: &[f32]) -> Vec<f32> {
        l.iter().zip(r).flat_map(|(a, b)| [*a, *b]).collect()
    }

    #[test]
    fn the_matrix_and_the_difference() {
        // FL's "LR to MS": mid on the left, side on the right, a half each.
        let mut e = StereoShaper::with_settings(44100, &Settings { v: [8076, 8076, -8076, 8076, 0, 0, 1, 1] });
        let mut buf = frames(&[0.5, 0.25], &[0.25, -0.5]);
        Effect::process(&mut e, &mut buf);
        let h = level_gain(8076);
        assert_eq!(buf, vec![h * 0.5 + h * 0.25, -h * 0.25 + h * 0.5, h * 0.25 + h * -0.5, -h * -0.5 + h * 0.25]);
        let (n, d) = e.extra().expect("difference output 1");
        assert_eq!(n, 1);
        assert_eq!(d, &[0.5 - buf[0], 0.25 - buf[1], 0.25 - buf[2], -0.5 - buf[3]]);
        // No difference output: nothing extra.
        let mut e = StereoShaper::with_settings(44100, &Settings::default());
        Effect::process(&mut e, &mut buf);
        assert!(e.extra().is_none());
        // Without cross levels a -0 stays -0 (the cross terms are left out); with one it can become +0.
        let mut buf = vec![1.0f32, -0.0];
        Effect::process(&mut e, &mut buf);
        assert_eq!(buf[1].to_bits(), 0x8000_0000);
        let mut e = StereoShaper::with_settings(44100, &Settings { v: [1, 12800, 12800, 0, 0, 0, 0, 1] });
        let mut buf = vec![1.0f32, -0.0];
        Effect::process(&mut e, &mut buf);
        assert_eq!(buf[1].to_bits(), 0);
    }

    /// The probe of the reference run: a sine on the left, a saw-like pattern on the right.
    fn probe(n: usize) -> (Vec<f32>, Vec<f32>) {
        let l = (0..n).map(|i| ((i as f64 * 0.05).sin() * 0.5) as f32).collect();
        let r = (0..n).map(|i| (((i * 37) % 101) as f64 / 101.0 - 0.5) as f32).collect();
        (l, r)
    }

    fn run(v: [i32; PARAMS]) -> (Vec<f32>, Vec<f32>) {
        let (l, r) = probe(3000);
        let mut buf = frames(&l, &r);
        let mut e = StereoShaper::with_settings(44100, &Settings { v });
        let mut at = 0;
        for k in 0..13 {
            let n = if k == 12 { 3000 - at } else { 229 + k % 2 };
            Effect::process(&mut e, &mut buf[2 * at..2 * (at + n)]);
            at += n;
        }
        (buf.iter().step_by(2).copied().collect(), buf.iter().skip(1).step_by(2).copied().collect())
    }

    /// Samples against an independent reading of the page (bit for bit): delay and phase before the matrix, after it,
    /// and off.
    #[test]
    fn samples_match_the_reading_of_the_page() {
        let at = [0, 1, 2, 3, 100, 500, 1000, 1999, 2500, 2999];
        for (v, want) in [
            // "Stereoize 3": delay 1408 on the right, phase -2496 on the left, post.
            ([0, 12800, 12800, 0, 1408, -2496, 0, 1], &STEREOIZE_3),
            // Mixed levels with the effects first: delay -300 on the left, phase 1744 on the right.
            ([-3000, 9000, 15000, 4000, -300, 1744, 0, 0], &MIXED_PRE),
            // The same with the effects off.
            ([-3000, 9000, 15000, 4000, -300, 1744, 0, -1], &MIXED_OFF),
        ] {
            let (l, r) = run(v);
            for (k, &i) in at.iter().enumerate() {
                assert_eq!((l[i].to_bits(), r[i].to_bits()), want[k], "{v:?}, sample {i}");
            }
        }
    }

    #[rustfmt::skip]
    const STEREOIZE_3: [(u32, u32); 10] = [(0x0, 0x0), (0x3cc56016, 0x0), (0x3d3dec0e, 0x0), (0x3d88de5c, 0x0), (0x3e2c6613, 0x0), (0x3ef497b8, 0xbdc0a238), (0x3ee803d8, 0x3d981447), (0x3ebafee0, 0x3d367ebc), (0x3eabe7ef, 0xbed7720f), (0x3e869232, 0x3ec32b17)];
    #[rustfmt::skip]
    const MIXED_PRE: [(u32, u32); 10] = [(0x3d7e138f, 0xbf0ddbc8), (0xbb8ea8cf, 0x3d1f4d57), (0xbd54beb6, 0x3eed905a), (0x3d2561d5, 0xbeb8acfb), (0xbe8ceac2, 0x3d1cbc1a), (0xbe334a56, 0xbf0666d1), (0xbe537256, 0xbee82434), (0xbe7c4945, 0xbeda2914), (0xbeb24b5b, 0x3ee123b5), (0xbe9cefea, 0x3d0e0abe)];
    #[rustfmt::skip]
    const MIXED_OFF: [(u32, u32); 10] = [(0x3d965c9d, 0xbf27e72d), (0x3d0c04e9, 0xbe2e4c8c), (0xbba6ac80, 0x3ea18022), (0x3dd1bb1a, 0xbf02bcc2), (0xbe990e02, 0x3d9df8ba), (0x3c265978, 0xbee5b3a4), (0xbd56c544, 0xbe76f35b), (0xbe06aa68, 0xbe9e717a), (0xbe6b1a6e, 0x3ec51cd6), (0xbe74f707, 0x3de49c45)];

    #[test]
    fn delay_and_phase_settings() {
        let mut e = StereoShaper::with_settings(44100, &Settings { v: [0, 12800, 12800, 0, 100, 0, 0, 0] });
        assert_eq!(e.delay(), 4);
        let mut buf = frames(&[0.0; 8], &[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        Effect::process(&mut e, &mut buf);
        assert_eq!(buf.iter().skip(1).step_by(2).copied().collect::<Vec<_>>(), [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        // Setting the delay empties the line, also to the same length.
        let mut buf = frames(&[0.0; 4], &[1.0; 4]);
        Effect::process(&mut e, &mut buf);
        e.apply(DELAY, 100);
        let mut buf = frames(&[0.0; 4], &[0.0; 4]);
        Effect::process(&mut e, &mut buf);
        assert!(buf.iter().all(|&v| v == 0.0));
        // A negative setting delays the left.
        e.apply(DELAY, -100);
        let mut buf = frames(&[1.0, 0.0, 0.0, 0.0, 0.0], &[1.0, 0.0, 0.0, 0.0, 0.0]);
        Effect::process(&mut e, &mut buf);
        assert_eq!((buf[0], buf[1], buf[8]), (0.0, 1.0, 1.0));
        // The phase's memory carries across a change of sign and is cleared at 0.
        e.apply(DELAY, 0);
        e.apply(PHASE, 864);
        let mut buf = frames(&[0.0; 4], &[1.0, 0.0, 0.0, 0.0]);
        Effect::process(&mut e, &mut buf);
        let ap = e.allpass().unwrap();
        assert_eq!(buf[1], ap.a2 as f32);
        assert_eq!(buf[0], 0.0);
        e.apply(PHASE, -864);
        assert_ne!(e.mem, [0.0; 4]);
        e.apply(PHASE, 0);
        assert_eq!(e.mem, [0.0; 4]);
        // Small memory values are cleared at the end of a block.
        e.mem = [1e-8, -1e-7, 5e-8, -6e-8];
        e.end_block();
        assert_eq!(e.mem, [0.0, -1e-7, 0.0, -6e-8]);
    }

    #[test]
    fn the_matrix_before_the_song_and_when_switched_on() {
        let s = Settings { v: [0, 25600, 6400, 0, 0, 0, 0, 1] };
        // On at the song's start: at its setting from the first sample.
        let mut e = StereoShaper::with_settings(44100, &s);
        assert_eq!(e.current(), [0.0; 4]);
        let mut buf = vec![1.0f32; 64];
        Effect::process(&mut e, &mut buf);
        assert_eq!((buf[0], buf[1]), (4.0, level_gain(6400)));
        // Off in the project, switched on during the song: the matrix glides from zero at the usual limit.
        let mut e = StereoShaper::with_settings(44100, &s);
        Effect::reset(&mut e);
        let mut buf = vec![1.0f32; 64];
        Effect::process(&mut e, &mut buf);
        assert_eq!((buf[0], buf[1]), (0.0, 0.0));
        assert_eq!(buf[2], 0.001);
        assert_eq!(e.current()[0], 0.001 * 32.0);
        // Switched off and on again: the gains stay where they were, the line keeps its contents, the memory goes.
        let mut e = StereoShaper::with_settings(44100, &Settings { v: [0, 12800, 12800, 0, 100, 864, 0, 0] });
        let mut buf = frames(&[0.0; 4], &[1.0; 4]);
        Effect::process(&mut e, &mut buf);
        Effect::reset(&mut e);
        assert_eq!(e.mem, [0.0; 4]);
        assert_eq!(e.current(), [1.0, 1.0, 0.0, 0.0]);
        let mut buf = frames(&[0.0; 1], &[0.0; 1]);
        Effect::process(&mut e, &mut buf);
        assert_ne!(buf[1], 0.0, "the line still holds the ones");
    }

    #[test]
    fn a_slot_reads_its_state() {
        let s = Settings { v: [-8076, 12800, 12800, -8076, 0, 864, 0, 0] };
        let mut slot = super::super::from_slot("Fruity Stereo Shaper", &s.to_state(), 44100).expect("a stereo shaper");
        let mut own = StereoShaper::with_settings(44100, &s);
        own.flush();
        let (l, r) = probe(600);
        let mut buf = frames(&l, &r);
        slot.process(&mut buf);
        let (mut l2, mut r2) = (l.clone(), r.clone());
        own.process(&mut l2, &mut r2);
        assert_eq!(buf, frames(&l2, &r2));
        assert!(slot.extra().is_none());
    }
}
