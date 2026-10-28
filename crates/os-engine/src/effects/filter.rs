// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's filter (a recreation of FL's Fruity Filter mixer effect), configured from its saved state.
//!
//! A resonant state-variable filter whose low-pass, band-pass and high-pass outputs are mixed by three levels,
//! optionally doubled into two sections in a row (X 2, the second without resonance), and optionally followed by a DC
//! remover (Center). The five coefficients glide from block to block. The DC remover's extended-precision loop runs in
//! doubles here (as Love Philter's does).
//!
// spec: plugins/fruity-filter.md, plugins/lemon-filter.md

use super::Effect;

/// The settings, in the saved state's order (after the version).
pub const PARAMS: usize = 6;
pub const CUTOFF: usize = 0;
pub const RESONANCE: usize = 1;
pub const LOW: usize = 2;
pub const BAND: usize = 3;
pub const HIGH: usize = 4;
pub const X2: usize = 5;

/// The settings' ranges (the editor's). Stored values outside them are clamped by the positions (X 2: only 1 is on).
pub const RANGES: [(i32, i32); PARAMS] = [(0, 1024), (0, 1024), (0, 1024), (0, 1024), (0, 1024), (0, 1)];

/// A fresh instance (FL's Default preset): cutoff fully open, the low pass at full level and the high pass at half.
pub const DEFAULTS: [i32; PARAMS] = [1024, 205, 1024, 0, 512, 0];

/// The version the state is written with; a state of a later one is not read (the defaults play).
pub const VERSION: i32 = 2;

/// The saved state: the version, the six ints and the Center byte (29 bytes; version 1 has no Center byte).
pub const STATE_LEN: usize = 29;

/// Everything the saved state says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub v: [i32; PARAMS],
    /// Remove the DC offset (the Center switch).
    pub center: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { v: DEFAULTS, center: true }
    }
}

impl Settings {
    /// A saved state: version 1 (no Center byte: Center off) and 2 as stored, a later version as the defaults; `None`
    /// when the ints are missing.
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        let version = super::field(b, 0)?;
        let v = super::ints(b, 1)?;
        if version > VERSION {
            return Some(Settings::default());
        }
        Some(Settings { v, center: b.get(28).is_some_and(|c| c & 1 != 0) })
    }

    /// The saved state (29 bytes, version 2).
    pub fn to_state(&self) -> Vec<u8> {
        let mut b = super::state_of(&[VERSION]);
        b.extend(super::state_of(&self.v));
        b.push(u8::from(self.center));
        b
    }

    /// X 2 is on only for a stored 1.
    pub fn x2(&self) -> bool {
        self.v[X2] == 1
    }
}

/// A knob's position in 0..1: `single(clamp(value / 1024, 0, 1))`.
pub fn position(v: i32) -> f32 {
    (f64::from(v) * (1.0 / 1024.0)).clamp(0.0, 1.0) as f32
}

/// The frequency coefficient `(26^x - 1) / 25` of a cutoff value, 0..1, in the page's precision.
pub fn freq(v: i32) -> f32 {
    let e = ((f64::from(position(v)) * 26f64.ln()).exp() - 1.0) as f32;
    (f64::from(e) * 0.04) as f32
}

/// The damping `1 - 0.8 x` of a resonance value, 1..0.2.
pub fn damping(v: i32) -> f32 {
    (1.0 - f64::from(position(v)) * 0.8) as f32
}

/// An output level `(11^x - 1) / 10` (the host's level curve), 0..1.
pub fn level(v: i32) -> f32 {
    ((f64::from(position(v)) * 11f64.ln()).exp() - 1.0) as f32 / 10.0
}

/// The five coefficients `[f, q, gl, gb, gh]` of the settings.
pub fn coefficients(v: &[i32; PARAMS]) -> [f32; 5] {
    [freq(v[CUTOFF]), damping(v[RESONANCE]), level(v[LOW]), level(v[BAND]), level(v[HIGH])]
}

/// The frequency in Hz a cutoff value sets at `rate` (the coefficient is `2 sin(pi fc / rate)`): for the editor.
pub fn cutoff_hz(v: i32, rate: f64) -> f64 {
    rate / std::f64::consts::PI * (f64::from(freq(v)) / 2.0).asin()
}

/// The gain of the settings at `hz` for a sample rate (the filter without the DC remover, from its difference
/// equations): for the editor's picture. With `z^-1 = u`, `d = 1 - u` and `D = d^2 + f^2 u + q f u d` a section passes
/// `f^2 u / D` to its low output, `f d / D` to its band output and `d^2 / D` to its high output; X 2 multiplies in a
/// second section with a damping of 1.
pub fn response(v: &[i32; PARAMS], hz: f64, rate: f64) -> f64 {
    type C = (f64, f64);
    let mul = |a: C, b: C| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
    let add = |a: C, b: C| (a.0 + b.0, a.1 + b.1);
    let scale = |a: C, k: f64| (a.0 * k, a.1 * k);
    let div = |a: C, b: C| {
        let n = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / n, (a.1 * b.0 - a.0 * b.1) / n)
    };
    let [f, q, gl, gb, gh] = coefficients(v).map(f64::from);
    let w = 2.0 * std::f64::consts::PI * hz / rate;
    let u = (w.cos(), -w.sin());
    let d = (1.0 - u.0, -u.1);
    let section = |q: f64| {
        let den = add(add(mul(d, d), scale(u, f * f)), scale(mul(u, d), q * f));
        let num = add(add(scale(u, f * f * gl), scale(d, f * gb)), scale(mul(d, d), gh));
        div(num, den)
    };
    let mut h = section(f64::from(q));
    if v[X2] == 1 {
        h = mul(h, section(1.0));
    }
    h.0.hypot(h.1)
}

/// 2^-24: smaller stored values are flushed to 0 at a block's end.
const TINY: f32 = 5.9604645e-8;

/// Flushes a value below 2^-24 in magnitude (compared on the single's bit pattern) to 0.
fn flush(x: &mut f32) {
    if x.to_bits() & 0x7fff_ffff < TINY.to_bits() {
        *x = 0.0;
    }
}

/// A coefficient gliding from block to block (spec "Glide of the coefficients").
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    /// The value reached.
    cur: f32,
}

impl Glide {
    /// Plans a block of `n` samples towards `t`: the start value and the step added before every frame. Blocks of 128
    /// or more reach `t` at their end; shorter ones move 1/128 of the distance per sample.
    fn block(&mut self, t: f32, n: usize) -> (f32, f32) {
        let start = self.cur;
        if self.cur == t {
            return (start, 0.0);
        }
        if n >= 128 {
            let step = (t - self.cur) / n as f32;
            self.cur = t;
            return (start, step);
        }
        let mut step = (f64::from(t - self.cur) * 0.0078125) as f32;
        self.cur += step * n as f32;
        if self.cur < TINY {
            self.cur = 0.0;
            step = (f64::from(-start) * 0.0078125) as f32;
        }
        (start, step)
    }
}

/// One channel's memory: the first section's low, band and high values, the second section's (X 2), and the DC
/// remover's `y` and `v`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Channel {
    low: f32,
    band: f32,
    high: f32,
    low2: f32,
    band2: f32,
    high2: f32,
    y: f64,
    v: f64,
}

impl Channel {
    /// One sample through the filter (every operation a single one, in the page's order), then the DC remover.
    #[inline]
    fn sample(&mut self, s: f32, k: &[f32; 5], x2: bool, dc: Option<(f64, f64)>) -> f32 {
        let [f, q, gl, gb, gh] = *k;
        self.low += f * self.band;
        self.high = (s - self.low) - q * self.band;
        self.band += f * self.high;
        let mut out = (self.low * gl + self.band * gb) + self.high * gh;
        if x2 {
            self.low2 += f * self.band2;
            self.high2 = (out - self.low2) - self.band2;
            self.band2 += f * self.high2;
            out = (self.low2 * gl + self.band2 * gb) + self.high2 * gh;
        }
        match dc {
            Some((c, r)) => {
                let x = f64::from(out);
                self.v += (x - self.y) * c;
                self.y += self.v;
                self.v *= r;
                (x - self.y) as f32
            }
            None => out,
        }
    }

    /// Empties the filter (not the DC remover).
    fn clear_filter(&mut self) {
        *self = Channel { y: self.y, v: self.v, ..Channel::default() };
    }

    /// The block's end: small values to 0.
    fn end_block(&mut self) {
        for x in [&mut self.low, &mut self.band, &mut self.high, &mut self.low2, &mut self.band2, &mut self.high2] {
            flush(x);
        }
        for x in [&mut self.y, &mut self.v] {
            if x.abs() <= f64::from(TINY) {
                *x = 0.0;
            }
        }
    }
}

/// The DC remover's coefficient `single(1.0070235 / rate)` (about 34 Hz).
pub fn dc_coefficient(rate: u32) -> f32 {
    (1.0070235 / f64::from(rate.max(1))) as f32
}

/// The DC remover's damping per sample (single 0x3F75C28F).
const DC_DAMPING: f32 = 0.96;

pub struct Filter {
    applied: Settings,
    target: [f32; 5],
    coef: [Glide; 5],
    x2: bool,
    ch: [Channel; 2],
    dc: f32,
}

impl Filter {
    /// A filter whose coefficients stand at the settings' values (the start of a song: nothing glides in; FL's idle
    /// blocks before an export complete that glide).
    pub fn with_settings(sample_rate: u32, s: &Settings) -> Filter {
        let target = coefficients(&s.v);
        Filter { applied: *s, target, coef: target.map(|cur| Glide { cur }), x2: s.x2(), ch: [Channel::default(); 2], dc: dc_coefficient(sample_rate) }
    }

    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<Filter> {
        Some(Filter::with_settings(sample_rate, &Settings::from_state(state)?))
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        self.applied
    }

    /// New settings: the coefficients glide to them and X 2 switches from the next block on; Center acts at once.
    pub fn set(&mut self, s: &Settings) {
        if s.v != self.applied.v {
            self.target = coefficients(&s.v);
        }
        self.applied = *s;
    }

    /// A new sample rate: the DC remover's coefficient for it, its memory cleared.
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.dc = dc_coefficient(sample_rate);
        for c in &mut self.ch {
            (c.y, c.v) = (0.0, 0.0);
        }
    }

    /// Plans a block of `n` frames: X 2 switched (the filter emptied when it changes), the coefficients' start values
    /// and steps, and the DC remover's constants when Center is on.
    fn begin(&mut self, n: usize) -> ([f32; 5], [f32; 5], bool, Option<(f64, f64)>) {
        if self.applied.x2() != self.x2 {
            self.x2 = self.applied.x2();
            for c in &mut self.ch {
                c.clear_filter();
            }
        }
        let mut start = [0.0; 5];
        let mut step = [0.0; 5];
        for k in 0..5 {
            (start[k], step[k]) = self.coef[k].block(self.target[k], n);
        }
        let dc = self.applied.center.then_some((f64::from(self.dc), f64::from(DC_DAMPING)));
        (start, step, step.iter().any(|s| s.to_bits() != 0), dc)
    }

    /// The coefficients of the next frame: the step is added before every frame, all five together.
    #[inline]
    fn advance(k: &mut [f32; 5], step: &[f32; 5], moving: bool) {
        if moving {
            for (c, s) in k.iter_mut().zip(step) {
                *c += *s;
            }
        }
    }

    /// One block on separate channels.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        let (mut k, step, moving, dc) = self.begin(n);
        let x2 = self.x2;
        let [a, b] = &mut self.ch;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            Filter::advance(&mut k, &step, moving);
            *l = a.sample(*l, &k, x2, dc);
            *r = b.sample(*r, &k, x2, dc);
        }
        self.ch.iter_mut().for_each(Channel::end_block);
    }
}

impl Effect for Filter {
    fn process(&mut self, buf: &mut [f32]) {
        let (mut k, step, moving, dc) = self.begin(buf.len() / 2);
        let x2 = self.x2;
        let [a, b] = &mut self.ch;
        for f in buf.chunks_exact_mut(2) {
            Filter::advance(&mut k, &step, moving);
            f[0] = a.sample(f[0], &k, x2, dc);
            f[1] = b.sample(f[1], &k, x2, dc);
        }
        self.ch.iter_mut().for_each(Channel::end_block);
    }

    fn reset(&mut self) {
        self.ch = [Channel::default(); 2];
        self.coef = self.target.map(|cur| Glide { cur });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FL's four presets (spec "Settings").
    const PRESETS: [(&str, [i32; PARAMS], bool); 4] = [
        ("Default", [1024, 205, 1024, 0, 512, 0], true),
        ("Party next door", [160, 1024, 864, 0, 0, 0], true),
        ("Retro sound", [480, 1024, 0, 0, 1024, 1], true),
        ("Tiny speakers", [544, 0, 0, 1024, 464, 1], true),
    ];

    #[test]
    fn states_read_by_version() {
        for (_, v, center) in PRESETS {
            let s = Settings { v, center };
            assert_eq!(s.to_state().len(), STATE_LEN);
            assert_eq!(Settings::from_state(&s.to_state()), Some(s));
        }
        // Version 1: no Center byte, Center off.
        let mut v1 = Settings { v: [100, 200, 300, 400, 500, 1], center: true }.to_state();
        v1.truncate(28);
        v1[0] = 1;
        assert_eq!(Settings::from_state(&v1), Some(Settings { v: [100, 200, 300, 400, 500, 1], center: false }));
        // Version 3: not read, the editor defaults play.
        let mut v3 = Settings { v: [0; PARAMS], center: false }.to_state();
        v3[0] = 3;
        assert_eq!(Settings::from_state(&v3), Some(Settings::default()));
        assert_eq!(Settings::from_state(&v3[..20]), None);
        // X 2 stored as 2 plays as off; Center is bit 0 of its byte.
        let mut b = Settings::default().to_state();
        b[24] = 2;
        b[28] = 0xfe;
        let s = Settings::from_state(&b).unwrap();
        assert!(!s.x2() && !s.center);
    }

    #[test]
    fn coefficient_laws() {
        assert_eq!(freq(1024), 1.0);
        assert_eq!(freq(0), 0.0);
        assert_eq!(damping(0), 1.0);
        assert_eq!(damping(1024), 0.2);
        assert_eq!(level(1024), 1.0);
        assert_eq!(level(0), 0.0);
        // Values outside the range are clamped by the positions.
        assert_eq!(freq(5000), 1.0);
        assert_eq!(level(-7), 0.0);
        // Half way: (26^0.5 - 1) / 25 and (11^0.5 - 1) / 10.
        assert!((freq(512) - ((26f32.sqrt() - 1.0) / 25.0)).abs() < 1e-7);
        assert!((level(512) - ((11f32.sqrt() - 1.0) / 10.0)).abs() < 1e-7);
        // The DC remover's constant at 44100 Hz.
        assert!((dc_coefficient(44100) - 2.2835e-5).abs() < 1e-9);
        assert_eq!(DC_DAMPING.to_bits(), 0x3F75C28F);
    }

    /// The filter, written out per sample from the page, against the effect.
    #[test]
    fn the_filter_follows_the_page() {
        let s = Settings { v: [480, 900, 700, 300, 200, 0], center: false };
        let [f, q, gl, gb, gh] = coefficients(&s.v);
        let mut fx = Filter::with_settings(44100, &s);
        let input: Vec<f32> = (0..300).map(|i| ((i * 37 % 101) as f32 / 50.0 - 1.0) * if i % 7 == 0 { 1.0 } else { 0.3 }).collect();
        let mut l = input.clone();
        let mut r: Vec<f32> = input.iter().map(|x| -x * 0.5).collect();
        let r0 = r.clone();
        fx.process(&mut l, &mut r);
        let run = |xs: &[f32]| -> Vec<f32> {
            let (mut low, mut band) = (0f32, 0f32);
            xs.iter()
                .map(|&x| {
                    low += f * band;
                    let high = (x - low) - q * band;
                    band += f * high;
                    (low * gl + band * gb) + high * gh
                })
                .collect()
        };
        assert_eq!(l, run(&input));
        assert_eq!(r, run(&r0));
    }

    #[test]
    fn x2_adds_a_second_section_and_switching_it_empties_the_filter() {
        let on = Settings { v: [300, 1024, 1024, 0, 0, 1], center: false };
        let mut fx = Filter::with_settings(44100, &on);
        let mut l = vec![1.0f32; 64];
        let mut r = vec![1.0f32; 64];
        fx.process(&mut l, &mut r);
        // Low pass only: the first frame's low is 0 in both sections (their bands were 0); then the second section
        // follows the first one.
        assert_eq!(l[0], 0.0);
        assert!(fx.ch[0].low2 != 0.0 && l[63] > 0.0);
        // Off: the next block starts from an empty filter.
        fx.set(&Settings { v: [300, 1024, 1024, 0, 0, 0], center: false });
        let mut a = vec![0.5f32; 32];
        let mut b = vec![0.5f32; 32];
        fx.process(&mut a, &mut b);
        let mut fresh = Filter::with_settings(44100, &Settings { v: [300, 1024, 1024, 0, 0, 0], center: false });
        let mut c = vec![0.5f32; 32];
        let mut d = vec![0.5f32; 32];
        fresh.process(&mut c, &mut d);
        assert_eq!(a, c);
        // Setting the state it already has does nothing.
        fx.set(&Settings { v: [300, 1024, 1024, 0, 0, 2], center: false });
        assert!(!fx.applied.x2());
        let before = fx.ch;
        fx.begin(0);
        assert_eq!(fx.ch, before);
    }

    #[test]
    fn coefficients_glide_over_a_block_or_by_128ths() {
        let mut g = Glide { cur: 1.0 };
        // A block of 128 or more: a linear ramp ending at the target.
        assert_eq!(g.block(0.0, 200), (1.0, -1.0 / 200.0));
        assert_eq!(g.cur, 0.0);
        assert_eq!(g.block(0.0, 64), (0.0, 0.0));
        // Shorter: 1/128 of the distance per sample for the block's length.
        let mut g = Glide { cur: 0.0 };
        let (start, step) = g.block(1.0, 64);
        assert_eq!((start, step), (0.0, 0.0078125));
        assert_eq!(g.cur, 0.5);
        // Ending below 2^-24: 0, the step from the start.
        let mut g = Glide { cur: 1.0e-7 };
        let (start, step) = g.block(0.0, 100);
        assert_eq!(g.cur, 0.0);
        assert_eq!(step, (f64::from(-start) * 0.0078125) as f32);
        // The step is added before the first frame.
        let mut fx = Filter::with_settings(44100, &Settings { v: [1024, 0, 0, 0, 1024, 0], center: false });
        fx.set(&Settings { v: [1024, 0, 0, 0, 0, 0], center: false });
        let mut l = vec![1.0f32; 200];
        let mut r = vec![0.0f32; 200];
        fx.process(&mut l, &mut r);
        assert!(l[0] < 1.0 && l[199].abs() < 1e-6, "{} {}", l[0], l[199]);
    }

    #[test]
    fn center_removes_a_dc_offset() {
        let mut fx = Filter::with_settings(44100, &Settings { v: [1024, 205, 1024, 0, 0, 0], center: true });
        let mut out = 0.0;
        for _ in 0..200 {
            let mut l = vec![0.5f32; 441];
            let mut r = vec![0.5f32; 441];
            fx.process(&mut l, &mut r);
            out = l[440];
        }
        assert!(out.abs() < 1e-3, "{out}");
        // Off: the offset passes (the open low pass is a unit gain on DC).
        let mut fx = Filter::with_settings(44100, &Settings { v: [1024, 205, 1024, 0, 0, 0], center: false });
        let mut l = vec![0.5f32; 4410];
        let mut r = vec![0.5f32; 4410];
        fx.process(&mut l, &mut r);
        assert!((l[4409] - 0.5).abs() < 1e-3, "{}", l[4409]);
    }

    /// The editor's response curve against sines through the filter.
    #[test]
    fn the_response_matches_sines_through_the_filter() {
        for v in [[480, 1024, 1024, 0, 0, 0], [544, 0, 0, 1024, 464, 1], [700, 600, 300, 900, 1024, 0], [160, 1024, 864, 0, 0, 0]] {
            for hz in [100.0, 1000.0, 3000.0] {
                let mut fx = Filter::with_settings(44100, &Settings { v, center: false });
                let n = 44100;
                let mut l: Vec<f32> = (0..n).map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / 44100.0).sin() as f32).collect();
                let mut r = vec![0.0f32; n];
                fx.process(&mut l, &mut r);
                let peak = l[n / 2..].iter().fold(0f32, |m, x| m.max(x.abs()));
                let want = response(&v, hz, 44100.0);
                assert!((f64::from(peak) - want).abs() < 0.01 * want.max(0.05), "{v:?} {hz}: {peak} {want}");
            }
        }
        // Full cutoff is a sixth of the rate.
        assert!((cutoff_hz(1024, 44100.0) - 7350.0).abs() < 1e-6);
    }

    #[test]
    fn small_values_are_flushed_at_the_block_end() {
        let mut c = Channel { low: 5.0e-8, band: -5.0e-8, low2: 6.0e-8, y: 5.0e-8, v: 1.0, ..Channel::default() };
        c.end_block();
        assert_eq!((c.low, c.band, c.low2, c.y, c.v), (0.0, 0.0, 6.0e-8, 0.0, 1.0));
    }

    #[test]
    fn interleaved_and_separate_blocks_agree() {
        let s = Settings { v: [544, 0, 0, 1024, 464, 1], center: true };
        let mut a = Filter::with_settings(48000, &s);
        let mut b = Filter::with_settings(48000, &s);
        let l: Vec<f32> = (0..500).map(|i| (i as f32 * 0.37).sin() + 0.2).collect();
        let r: Vec<f32> = (0..500).map(|i| (i as f32 * 0.11).cos()).collect();
        let mut buf: Vec<f32> = l.iter().zip(&r).flat_map(|(x, y)| [*x, *y]).collect();
        let (mut l2, mut r2) = (l.clone(), r.clone());
        Effect::process(&mut a, &mut buf);
        b.process(&mut l2, &mut r2);
        assert_eq!(buf.iter().step_by(2).copied().collect::<Vec<_>>(), l2);
        assert_eq!(buf.iter().skip(1).step_by(2).copied().collect::<Vec<_>>(), r2);
    }
}
