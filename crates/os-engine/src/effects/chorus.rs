// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's chorus (a recreation of FL's Fruity Chorus mixer effect), configured from its saved state.
//!
//! The input, at 0.7, is split by a one-pole crossover; one band goes into a short delay line per side and is read
//! back at three places that three slow LFOs move; the three reads, at 0.6, are added to the input. There is no
//! feedback. The LFOs start where a generator puts them each time the effect starts (FL seeds it from the clock;
//! Lemon Studio from a fixed seed, so its renders repeat).
//!
// spec: plugins/fruity-chorus.md, plugins/lemon-chorus.md

use std::sync::OnceLock;

use super::Effect;

/// The settings, in the saved state's order (after the version field).
pub const PARAMS: usize = 12;
pub const DELAY: usize = 0;
pub const DEPTH: usize = 1;
pub const STEREO: usize = 2;
/// LFO 1, 2 and 3: frequency and wave.
pub const SPEED: [usize; 3] = [3, 4, 5];
pub const WAVE: [usize; 3] = [6, 7, 8];
pub const CROSS_TYPE: usize = 9;
pub const CROSS_CUTOFF: usize = 10;
pub const WET_ONLY: usize = 11;

/// The knobs' ranges (spec "Settings"). A stored value outside its range acts as the end it passes (the knob's
/// position is kept within 0..1).
pub const RANGES: [(i32, i32); PARAMS] =
    [(0, 1024), (0, 5000), (0, 1024), (0, 5000), (0, 5000), (0, 5000), (0, 1024), (0, 1024), (0, 1024), (0, 100), (0, 5000), (0, 1024)];

/// What a new Fruity Chorus holds (spec "Settings", the Default column). FL's "Default" preset differs in LFO 1's wave
/// only (0 instead of 23: the same shape).
pub const DEFAULTS: [i32; PARAMS] = [512, 2250, 341, 1500, 2500, 3500, 23, 0, 0, 0, 2500, 0];

/// The saved state: the version (1) and the twelve knob values.
pub const STATE_LEN: usize = 52;
pub const VERSION: i32 = 1;

/// The delay line per side, and the LFO tables' length.
pub const LINE: usize = 4096;
const MASK: u32 = LINE as u32 - 1;

/// The generator's seed when the effect is created (Lemon Studio choice: FL seeds it from the clock, so any seed is one
/// FL could have used; a fixed one makes renders repeat, spec "State and timing" and audio/quirks.md).
pub const SEED: u32 = 0;

/// LFO shapes (spec "LFO tables").
pub const ARC: usize = 0;
pub const ARC_CUBED: usize = 1;
pub const SINES: usize = 2;
pub const SINES_CUBED: usize = 3;

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
    /// A saved state: the twelve values after the version field. A version above 1 is not read: the instance keeps a
    /// new one's settings. `None` when the state is too short.
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
// From a knob to its working value (spec "Settings")

/// Knob `k`'s position: `single(value / max)`, the quotient in doubles rounded once, kept within 0..1.
pub fn position(k: usize, v: i32) -> f32 {
    ((f64::from(v) / f64::from(RANGES[k].1)) as f32).clamp(0.0, 1.0)
}

/// Trunc to a 32-bit unsigned integer.
fn trunc(x: f64) -> u32 {
    x as u32
}

/// The delay in 1/4096 of a sample: `Trunc((x * 29.95 + 0.05) * rate * 4.096)` in doubles (0.05 to 30 ms).
pub fn delay(x: f32, rate: u32) -> u32 {
    trunc((f64::from(x) * 29.95 + 0.05) * f64::from(rate) * 4.096)
}

/// The LFOs' swing: `Trunc(s * 4.096 / 64)` with `s = (x * 5) * rate`, all in singles (0 to 5 ms).
pub fn depth(x: f32, rate: u32) -> u32 {
    let s = (x * 5.0) * rate as f32;
    trunc(f64::from((s * 4.096) / 64.0))
}

/// The table entries by which the right side's LFOs run ahead: `Trunc(single(x * 4096) * 0.5) - 1`, wrapping (0 gives
/// one entry behind; 2048 is half a cycle).
pub fn stereo(x: f32) -> u32 {
    trunc(f64::from((x * 4096.0) * 0.5)).wrapping_sub(1)
}

/// An LFO's step per sample, where 2^24 is one cycle: `Trunc(((x * x) * 83886080) / rate)` in singles (`5 x^2` Hz).
pub fn step(x: f32, rate: u32) -> u32 {
    trunc(f64::from(((x * x) * 83_886_080.0) / rate as f32))
}

/// An LFO's shape: `Trunc(single(x * 3))`, 0..3 (3 only at the end of the knob).
pub fn shape(x: f32) -> usize {
    (((f64::from(x) * 3.0) as f32) as usize).min(SINES_CUBED)
}

/// The cross cutoff's frequency: a MIDI note number `127 x` as a frequency, `single(exp((127 x - 69) / 12 * ln 2) *
/// 440)` (8.2 Hz to 12.5 kHz).
pub fn cutoff_hz(x: f32) -> f32 {
    ((((127.0 * f64::from(x) - 69.0) / 12.0) * std::f64::consts::LN_2).exp() * 440.0) as f32
}

/// The crossover's pole and gain for the cutoff knob at `x` (spec "Cross cutoff"; the frequency at most `rate / 4.1`).
pub fn crossover(x: f32, rate: u32) -> (f32, f32) {
    let r = f64::from(rate);
    let w = f64::from(cutoff_hz(x)).min(r / 4.1) / r * 6.283184;
    let pole = ((1.0 - w.sin()) / w.cos()) as f32;
    let gain = (f64::from((1.0 - f64::from(pole)) as f32) * 0.5) as f32;
    (pole, gain)
}

/// "Cross type" and "wet only" are switches: on above a quarter of the knob.
pub fn switch(x: f32) -> bool {
    x > 0.25
}

/// The working values of a set of settings at a sample rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Working {
    pub delay: u32,
    pub depth: u32,
    pub stereo: u32,
    pub step: [u32; 3],
    pub shape: [usize; 3],
    /// The low band is chorused instead of the high band.
    pub low_band: bool,
    pub pole: f32,
    pub gain: f32,
    pub wet_only: bool,
}

impl Working {
    pub fn new(v: &[i32; PARAMS], rate: u32) -> Working {
        let x = |k: usize| position(k, v[k]);
        let (pole, gain) = crossover(x(CROSS_CUTOFF), rate);
        Working {
            delay: delay(x(DELAY), rate),
            depth: depth(x(DEPTH), rate),
            stereo: stereo(x(STEREO)),
            step: SPEED.map(|k| step(x(k), rate)),
            shape: WAVE.map(|k| shape(x(k))),
            low_band: switch(x(CROSS_TYPE)),
            pole,
            gain,
            wet_only: switch(x(WET_ONLY)),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// LFO tables and the generator (spec "LFO tables", "State and timing")

/// The four LFO tables (arc, arc cubed, sines, sines cubed), entries 0..32766 with 16383 the middle.
pub fn tables() -> &'static [[u32; LINE]; 4] {
    static TABLES: OnceLock<Box<[[u32; LINE]; 4]>> = OnceLock::new();
    TABLES.get_or_init(|| {
        let mut t = Box::new([[0u32; LINE]; 4]);
        let third = 1.0 / 3.0;
        let norm = 2.0 * 2f64.sqrt() + 1.0;
        for i in 0..LINE {
            let x = (i as f64 / 2048.0) as f32;
            let arc = if x < 1.0 { x * 4.0 * (1.0 - x) } else { (x - 1.0) * -4.0 * (2.0 - x) };
            let d = f64::from(x);
            let sines = (((3.1415 * d).sin() + ((9.4245 * d).sin() + (6.283 * d).sin()) * third) * 3.0 / norm) as f32;
            for (k, v) in [arc, arc * arc * arc, sines, sines * sines * sines].into_iter().enumerate() {
                t[k][i] = trunc(f64::from((v + 1.0) * 16383.0));
            }
        }
        t
    })
}

/// The generator the LFOs' start phases come from: `seed = seed * 0x08088405 + 1`, `value = (seed * 32767) >> 32`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generator(pub u32);

impl Generator {
    pub fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(0x0808_8405).wrapping_add(1);
        ((u64::from(self.0) * 32767) >> 32) as u32
    }

    /// Three successive values: the three LFOs' phases.
    pub fn phases(&mut self) -> [u32; 3] {
        let a = self.next();
        let b = self.next();
        [a, b, self.next()]
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The effect

pub struct Chorus {
    applied: [i32; PARAMS],
    rate: u32,
    w: Working,
    phase: [u32; 3],
    /// Per side: the crossover's state and the previous input.
    low: [f32; 2],
    before: [f32; 2],
    line: [Box<[f32; LINE]>; 2],
    at: usize,
    generator: Generator,
}

impl Chorus {
    /// A new instance started as at a render: the settings in force, the lines empty, the phases drawn from the fixed
    /// seed.
    pub fn with_settings(rate: u32, s: &Settings) -> Chorus {
        let mut c = Chorus {
            applied: s.v,
            rate,
            w: Working::new(&s.v, rate),
            phase: [0; 3],
            low: [0.0; 2],
            before: [0.0; 2],
            line: [Box::new([0.0; LINE]), Box::new([0.0; LINE])],
            at: 0,
            generator: Generator(SEED),
        };
        c.start();
        c
    }

    pub fn from_state(state: &[u8], rate: u32) -> Option<Chorus> {
        Some(Chorus::with_settings(rate, &Settings::from_state(state)?))
    }

    /// The host enables the effect (as at the start of a render): the delay lines and the crossover are cleared and
    /// the three LFO phases drawn anew from the generator.
    pub fn start(&mut self) {
        for l in &mut self.line {
            l.fill(0.0);
        }
        self.low = [0.0; 2];
        self.before = [0.0; 2];
        self.phase = self.generator.phases();
    }

    /// Puts the LFOs at given phases (FL's export to compare with started there).
    pub fn set_phases(&mut self, phase: [u32; 3]) {
        self.phase = phase;
    }

    /// The LFOs' phases (tests).
    pub fn phases(&self) -> [u32; 3] {
        self.phase
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// New settings: they act from the next sample (nothing is smoothed).
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

    /// One read of side `s`'s line at `p` (1/4096 of a sample): a straight line between two samples, the difference a
    /// single, the products and the sum in doubles.
    #[inline]
    fn read(line: &[f32; LINE], p: u32) -> f32 {
        let j = ((p >> 12) & MASK) as usize;
        let f = p & MASK;
        let a = line[j];
        let d = line[(j + 1) & MASK as usize] - a;
        (f64::from(a) + f64::from(d) * f64::from(f) * (1.0 / 4096.0)) as f32
    }

    /// One sample of both sides (spec "Per sample").
    #[inline]
    fn sample(&mut self, l: f32, r: f32) -> (f32, f32) {
        let w = &self.w;
        let t = tables();
        let x = [(f64::from(l) * 0.7) as f32, (f64::from(r) * 0.7) as f32];
        let mut off = [[0u32; 3]; 2];
        for k in 0..3 {
            let table = &t[w.shape[k]];
            let i = self.phase[k] >> 12;
            off[0][k] = table[(i & MASK) as usize].wrapping_mul(w.depth) >> 9;
            off[1][k] = table[(i.wrapping_add(w.stereo) & MASK) as usize].wrapping_mul(w.depth) >> 9;
            self.phase[k] = self.phase[k].wrapping_add(w.step[k]);
        }
        let base = ((self.at as u32) << 12).wrapping_sub(w.delay);
        let mut out = [0.0f32; 2];
        for s in 0..2 {
            let low = (x[s] + self.before[s]) * w.gain + w.pole * self.low[s];
            self.low[s] = low;
            self.before[s] = x[s];
            let line = &mut self.line[s];
            line[self.at] = if w.low_band { low } else { x[s] - low };
            let [a, b, c] = off[s].map(|o| Chorus::read(line, base.wrapping_sub(o)));
            let sum = a + b + c;
            let wet = (f64::from(sum) * 0.6) as f32;
            out[s] = if w.wet_only { wet } else { wet + x[s] };
        }
        self.at = (self.at + 1) & MASK as usize;
        (out[0], out[1])
    }

    /// The end of a block: a crossover value under 1e-11 in magnitude is set to 0.
    fn end_block(&mut self) {
        for low in &mut self.low {
            if f64::from(low.abs()) < 1e-11 {
                *low = 0.0;
            }
        }
    }

    /// One block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            (*l, *r) = self.sample(*l, *r);
        }
        self.end_block();
    }
}

impl Effect for Chorus {
    fn process(&mut self, buf: &mut [f32]) {
        for f in buf.chunks_exact_mut(2) {
            (f[0], f[1]) = self.sample(f[0], f[1]);
        }
        self.end_block();
    }

    /// A slot switched on during the song: FL enables the plugin again, which clears it and draws new phases.
    fn reset(&mut self) {
        self.start();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_round_trip_and_a_later_version_is_not_read() {
        // FL's "50 Detune" preset.
        let s = Settings { v: [40, 4375, 149, 3229, 0, 781, 587, 0, 1024, 65, 4531, 491] };
        let b = s.to_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(Settings::from_state(&b), Some(s));
        let mut v2 = b.clone();
        v2[0] = 2;
        assert_eq!(Settings::from_state(&v2), Some(Settings::default()));
        let mut v0 = b.clone();
        v0[0] = 0;
        assert_eq!(Settings::from_state(&v0), Some(s));
        assert_eq!(Settings::from_state(&b[..48]), None);
    }

    /// The working values of the spec's formulas (checked against an independent reading of the page).
    #[test]
    fn knobs_become_working_values() {
        let w = Working::new(&DEFAULTS, 44100);
        assert_eq!((w.delay, w.depth, w.stereo), (2_714_019, 6350, 681));
        assert_eq!((w.step, w.shape, w.low_band, w.wet_only), ([171, 475, 932], [0, 0, 0], false, false));
        assert_eq!(cutoff_hz(position(CROSS_CUTOFF, 2500)), 320.243_7);
        assert_eq!((w.pole.to_bits(), w.gain.to_bits()), (0x3f74_93fd, 0x3cb6_c030));
        // "50 Detune": shapes 1 and 3, the low band chorused, wet only.
        let w = Working::new(&[40, 4375, 149, 3229, 0, 781, 587, 0, 1024, 65, 4531, 491], 44100);
        assert_eq!((w.delay, w.depth, w.stereo, w.step, w.shape), (220_358, 12348, 297, [793, 0, 46], [1, 0, 3]));
        assert!(w.low_band && w.wet_only);
        assert_eq!((w.pole.to_bits(), w.gain.to_bits()), (0x3eb3_01a1, 0x3ea6_7f30));
        // Every knob at its end at 48 kHz: 30 ms, 5 ms, half a cycle, 5 Hz, the cutoff at 12.5 kHz.
        let w = Working::new(&[1024, 5000, 1024, 5000, 5000, 5000, 1024, 683, 682, 26, 5000, 257], 48000);
        assert_eq!((w.delay, w.depth, w.stereo, w.step, w.shape), (5_898_240, 15360, 2047, [1747; 3], [3, 2, 1]));
        assert!(w.low_band && w.wet_only, "26 of 100 and 257 of 1024 are past a quarter");
        assert_eq!(cutoff_hz(1.0), 12_543.854);
        assert_eq!((w.pole.to_bits(), w.gain.to_bits()), (0x3c9c_f267, 0x3efb_186d));
        // At their starts at 192 kHz: stereo wraps to one entry behind; a quarter is not past a quarter.
        let w = Working::new(&[0, 0, 0, 0, 1, 2, 341, 342, 1023, 25, 0, 256], 192_000);
        assert_eq!((w.delay, w.depth, w.stereo, w.step, w.shape), (39321, 0, u32::MAX, [0; 3], [0, 1, 2]));
        assert!(!w.low_band && !w.wet_only);
        assert_eq!(cutoff_hz(0.0), 8.175_799);
        assert_eq!((w.pole.to_bits(), w.gain.to_bits()), (0x3f7f_ee78, 0x390c_4000));
        // The cutoff is held to rate / 4.1 at low rates.
        let w = Working::new(&DEFAULTS, 8000);
        assert_eq!((w.delay, w.depth, w.step), (492_339, 1152, [943, 2621, 5138]));
        assert_eq!((w.pole.to_bits(), w.gain.to_bits()), (0x3f46_88e4, 0x3de5_dc70));
        // Stored values past the ranges act as the ends.
        assert_eq!(position(DEPTH, 9000), 1.0);
        assert_eq!(position(DELAY, -5), 0.0);
    }

    #[test]
    fn lfo_tables() {
        let t = tables();
        let at = |k: usize| [0, 1, 512, 1024, 1536, 2048, 2049, 2560, 3072, 3584, 4095].map(|i| t[k][i]);
        assert_eq!(at(ARC), [16383, 16414, 28670, 32766, 28670, 16383, 16351, 4095, 0, 4095, 16351]);
        assert_eq!(at(ARC_CUBED), [16383, 16383, 23294, 32766, 23294, 16383, 16383, 9471, 0, 9471, 16383]);
        assert_eq!(at(SINES), [16383, 16435, 32766, 24942, 24207, 16384, 16358, 8558, 7825, 0, 16324]);
        assert_eq!(at(SINES_CUBED), [16383, 16383, 32766, 18719, 18167, 16383, 16383, 14598, 14048, 0, 16382]);
        for table in t.iter() {
            assert_eq!((*table.iter().min().unwrap(), *table.iter().max().unwrap()), (0, 32766));
        }
    }

    #[test]
    fn the_generator_draws_phases_below_32767() {
        let mut g = Generator(0);
        assert_eq!([g.next(), g.next(), g.next(), g.next(), g.next(), g.next()], [0, 1028, 28213, 6637, 8942, 22008]);
        let mut g = Generator(0x1234_5678);
        for _ in 0..1000 {
            assert!(g.next() < 32767);
        }
        // A new instance starts on the first three; each start draws the next three.
        let mut c = Chorus::with_settings(44100, &Settings::default());
        assert_eq!(c.phases(), [0, 1028, 28213]);
        Effect::reset(&mut c);
        assert_eq!(c.phases(), [6637, 8942, 22008]);
    }

    /// The input of the reference run: a sine on the left, a saw-like pattern on the right.
    fn probe(n: usize) -> (Vec<f32>, Vec<f32>) {
        let l = (0..n).map(|i| ((i as f64 * 0.05).sin() * 0.5) as f32).collect();
        let r = (0..n).map(|i| (((i * 37) % 101) as f64 / 101.0 - 0.5) as f32).collect();
        (l, r)
    }

    fn run(v: [i32; PARAMS]) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = probe(3000);
        let mut c = Chorus::with_settings(44100, &Settings { v });
        let mut at = 0;
        for k in 0..13 {
            let n = if k == 12 { 3000 - at } else { 229 + k % 2 };
            c.process(&mut l[at..at + n], &mut r[at..at + n]);
            at += n;
        }
        (l, r)
    }

    /// Samples of three settings against an independent reading of the page (bit for bit).
    #[test]
    fn samples_match_the_reading_of_the_page() {
        let bits = |x: &[f32], i: usize| x[i].to_bits();
        let at = [0, 1, 2, 3, 100, 500, 1000, 1999, 2500, 2999];
        let (l, r) = run(DEFAULTS);
        let want_l = [0x0, 0x3c8f4cdf, 0x3d0f1f06, 0x3d563c08, 0xbeabd6d8, 0xbd3dbd4e, 0x3e9eed91, 0x3e38b7de, 0x3db8df10, 0xbd736430];
        let want_r = [0xbeb33333, 0xbdbf9eac, 0x3e26c7bb, 0xbe8fb700, 0x3dbf9eac, 0xbe6dc020, 0xbe4b70db, 0xbe4c49ac, 0x3eddf864, 0x3e2152f2];
        for (k, &i) in at.iter().enumerate() {
            assert_eq!((bits(&l, i), bits(&r, i)), (want_l[k], want_r[k]), "Default, sample {i}");
        }
        // "50 Detune": wet only, the low band, shapes 1 and 3.
        let (l, r) = run([40, 4375, 149, 3229, 0, 781, 587, 0, 1024, 65, 4531, 491]);
        let want_l = [0, 0, 0, 0, 0, 0xbf1f4597, 0xbf203e3e, 0xbedf6dbc, 0xbe895880, 0xbe1486a4];
        let want_r = [0, 0, 0, 0, 0, 0xbd104ae6, 0xbda07c82, 0xbe06f623, 0x3da7bf2c, 0x3da09ccd];
        for (k, &i) in at.iter().enumerate() {
            assert_eq!((bits(&l, i), bits(&r, i)), (want_l[k], want_r[k]), "50 Detune, sample {i}");
        }
        // "Low Enhance": two LFOs stopped.
        let (l, r) = run([243, 4336, 416, 0, 0, 781, 0, 0, 1024, 65, 4635, 96]);
        assert_eq!((bits(&l, 500), bits(&r, 500)), (0xbf24e906, 0xbe8134cd));
        assert_eq!((bits(&l, 2999), bits(&r, 2999)), (0xbf15707c, 0xbd853cb7));
    }

    /// The dry part is the input at 0.7; without delay or depth the three reads are the band just written.
    #[test]
    fn the_dry_part_and_the_reads() {
        let mut c = Chorus::with_settings(44100, &Settings { v: [0, 0, 341, 0, 0, 0, 0, 0, 0, 0, 2500, 0] });
        let mut l = [0.5f32, 0.0, 0.0, 0.0];
        let mut r = [0.0f32; 4];
        c.process(&mut l, &mut r);
        let x = (0.5f64 * 0.7) as f32;
        // The delay is never shorter than 0.05 ms (2.2 samples at 44100 Hz): the first sample is dry.
        assert_eq!(l[0], x);
        assert_eq!(r, [0.0; 4]);
        // Wet only on silence is silence; set takes effect at once.
        c.set(&Settings { v: [0, 0, 341, 0, 0, 0, 0, 0, 0, 0, 2500, 1024] });
        assert!(c.working().wet_only);
    }

    #[test]
    fn a_slot_reads_its_state_and_restarts_on_reset() {
        let s = Settings { v: [128, 1534, 480, 2695, 0, 1914, 0, 0, 949, 0, 4479, 0] };
        let mut slot = super::super::from_slot("Fruity Chorus", &s.to_state(), 44100).expect("a chorus");
        let mut own = Chorus::with_settings(44100, &s);
        let (l, r) = probe(1000);
        let mut buf: Vec<f32> = l.iter().zip(&r).flat_map(|(a, b)| [*a, *b]).collect();
        slot.process(&mut buf);
        let (mut l2, mut r2) = (l.clone(), r.clone());
        own.process(&mut l2, &mut r2);
        let mine: Vec<f32> = l2.iter().zip(&r2).flat_map(|(a, b)| [*a, *b]).collect();
        assert_eq!(buf, mine);
        // Reset: lines and crossover empty, new phases.
        own.reset();
        assert_eq!(own.phases(), [6637, 8942, 22008]);
        assert!(own.line.iter().all(|l| l.iter().all(|&v| v == 0.0)) && own.low == [0.0; 2] && own.before == [0.0; 2]);
    }

    #[test]
    fn small_crossover_values_are_cleared_at_block_ends() {
        let mut c = Chorus::with_settings(44100, &Settings::default());
        c.low = [5e-12, -2e-11];
        c.end_block();
        assert_eq!(c.low, [0.0, -2e-11]);
    }
}
