// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's vocoder (Lemon Vocoder, a recreation of FL's Vocodex mixer effect).
//!
//! A band-split channel vocoder: the modulator (its left and right summed) and the carrier are split by the same bank
//! of band-pass filters; each modulator band is followed by an envelope (peak with hold, or RMS over the hold time,
//! then attack and release), and the carrier's band of the same frequency is played at that envelope. The bands' sum
//! is the wet signal; the modulator and the carrier can also pass through (the modulator high-passed, the carrier
//! low-passed), and noise can be added to the carrier before the split. The carrier keeps its stereo; the modulator's
//! pass-through too.
//!
//! The band spacing, the filter shapes, the envelope laws and the level of the wet signal are Lemon Studio's own design
//! (spec plugins/lemon-vocoder.md §4); what the saved state's ints mean: plugins/vocodex.md "The 27 ints".
//!
// spec: plugins/vocodex.md, plugins/lemon-vocoder.md

use std::f64::consts::{LN_2, TAU};

use super::Effect;

/// The most bands (the original's range is 5..100).
pub const MAX_BANDS: usize = 100;
/// The fewest bands.
pub const MIN_BANDS: usize = 5;
/// The steepest band filters: sections in a row.
pub const MAX_ORDER: usize = 4;

/// The level the bands' sum is played at (Lemon Studio choice: a modulator around -12 dBFS gives about the carrier's
/// level).
pub const MAKEUP: f64 = 4.0;

/// The vocoder's settings in plain units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Bands, 5..100.
    pub bands: usize,
    /// Band-pass sections in a row per band, 1..4 (the original's ORDER).
    pub order: usize,
    /// The lowest and the highest band's centre, in Hz.
    pub low_hz: f64,
    pub high_hz: f64,
    /// A band's width as a share of the spacing between band centres (1: neighbours cross at their -3 dB points).
    pub width: f64,
    /// Envelope times in milliseconds.
    pub attack_ms: f64,
    pub release_ms: f64,
    pub hold_ms: f64,
    /// RMS detection (the hold time is the window) instead of peaks held for the hold time.
    pub rms: bool,
    /// Linear gains: the vocoded signal, the modulator's and the carrier's pass-through, the noise added to the carrier.
    pub wet: f64,
    pub mod_level: f64,
    pub car_level: f64,
    pub noise: f64,
    /// The modulator pass-through's high-pass, the carrier pass-through's low-pass and the added noise's high-pass
    /// cutoffs, in Hz (0: no filter).
    pub mod_hp_hz: f64,
    pub car_lp_hz: f64,
    pub noise_hp_hz: f64,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            bands: 32,
            order: 2,
            low_hz: 80.0,
            high_hz: 12000.0,
            width: 1.0,
            attack_ms: 2.0,
            release_ms: 40.0,
            hold_ms: 5.0,
            rms: false,
            wet: 1.0,
            mod_level: 0.0,
            car_level: 0.0,
            noise: 0.0,
            mod_hp_hz: 5000.0,
            car_lp_hz: 500.0,
            noise_hp_hz: 0.0,
        }
    }
}

impl Params {
    /// The band centres: spaced evenly in octaves from `low_hz` to `high_hz`.
    pub fn centres(&self) -> Vec<f64> {
        let n = self.bands.clamp(MIN_BANDS, MAX_BANDS);
        let (lo, hi) = (self.low_hz.max(1.0), self.high_hz.max(self.low_hz.max(1.0) * 1.01));
        (0..n).map(|b| lo * (hi / lo).powf(b as f64 / (n - 1) as f64)).collect()
    }

    /// A band's width in octaves.
    pub fn octaves(&self) -> f64 {
        let n = self.bands.clamp(MIN_BANDS, MAX_BANDS);
        let (lo, hi) = (self.low_hz.max(1.0), self.high_hz.max(self.low_hz.max(1.0) * 1.01));
        ((hi / lo).log2() / (n - 1) as f64 * self.width).clamp(0.01, 4.0)
    }
}

/// A biquad's coefficients, normalised (`a0 = 1`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Coefs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefs {
    /// A band-pass with 0 dB at its centre `hz` and a width of `oct` octaves between its -3 dB points.
    fn band(hz: f64, oct: f64, rate: f64) -> Coefs {
        let w = TAU * hz / rate;
        let (s, c) = w.sin_cos();
        let alpha = s * (LN_2 / 2.0 * oct * w / s).sinh();
        let n = 1.0 / (1.0 + alpha);
        Coefs { b0: alpha * n, b1: 0.0, b2: -alpha * n, a1: -2.0 * c * n, a2: (1.0 - alpha) * n }
    }

    /// A two-pole low-pass (`high`: high-pass) at `hz`, Q 0.707; a frequency of 0 passes everything.
    fn pass(hz: f64, rate: f64, high: bool) -> Coefs {
        if hz <= 0.0 {
            return Coefs { b0: 1.0, ..Coefs::default() };
        }
        let w = TAU * hz.clamp(10.0, rate * 0.45) / rate;
        let (s, c) = w.sin_cos();
        let alpha = s / std::f64::consts::SQRT_2;
        let n = 1.0 / (1.0 + alpha);
        let (b0, b1) = if high { ((1.0 + c) / 2.0, -(1.0 + c)) } else { ((1.0 - c) / 2.0, 1.0 - c) };
        Coefs { b0: b0 * n, b1: b1 * n, b2: b0 * n, a1: -2.0 * c * n, a2: (1.0 - alpha) * n }
    }
}

/// A biquad's state (transposed direct form II).
#[derive(Clone, Copy, Debug, Default)]
struct State {
    s1: f64,
    s2: f64,
}

impl State {
    #[inline]
    fn run(&mut self, c: &Coefs, x: f64) -> f64 {
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Flushes values too small to matter (no denormals in long silences).
    fn flush(&mut self) {
        if self.s1.abs() < 1e-30 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-30 {
            self.s2 = 0.0;
        }
    }
}

/// One band: its filter, the filters' states for the modulator and the carrier's two channels, its envelope.
#[derive(Clone, Debug, Default)]
struct Band {
    c: Coefs,
    m: [State; MAX_ORDER],
    car: [[State; MAX_ORDER]; 2],
    /// The detector's level (the held peak, or the mean square), the samples the peak is still held for, the
    /// envelope.
    det: f64,
    held: u32,
    env: f64,
}

/// A gain that glides to its target over about 10 ms (no zipper noise on automation).
#[derive(Clone, Copy, Debug)]
struct Gain {
    v: f64,
    to: f64,
}

impl Gain {
    #[inline]
    fn next(&mut self, k: f64) -> f64 {
        self.v += (self.to - self.v) * k;
        self.v
    }
}

/// The vocoder.
pub struct Vocoder {
    rate: f64,
    p: Params,
    bands: Vec<Band>,
    order: usize,
    hp: Coefs,
    lp: Coefs,
    nhp: Coefs,
    hp_s: [State; 2],
    lp_s: [State; 2],
    nhp_s: State,
    /// One-pole steps per sample: attack, release, RMS window, gain glide; the peak hold in samples.
    ka: f64,
    kr: f64,
    kw: f64,
    kg: f64,
    hold: u32,
    gains: [Gain; 4],
    /// The noise generator (a 32-bit LCG).
    seed: u32,
}

impl Vocoder {
    pub fn new(rate: u32, p: Params) -> Vocoder {
        let mut v = Vocoder {
            rate: f64::from(rate.max(1)),
            p,
            bands: Vec::new(),
            order: 1,
            hp: Coefs::default(),
            lp: Coefs::default(),
            nhp: Coefs::default(),
            hp_s: [State::default(); 2],
            lp_s: [State::default(); 2],
            nhp_s: State::default(),
            ka: 1.0,
            kr: 1.0,
            kw: 1.0,
            kg: 1.0,
            hold: 0,
            gains: [Gain { v: 0.0, to: 0.0 }; 4],
            seed: 0x1234_5678,
        };
        v.configure();
        for g in &mut v.gains {
            g.v = g.to;
        }
        v
    }

    pub fn params(&self) -> &Params {
        &self.p
    }

    /// New settings: the filters change at once (their states are kept), the levels glide.
    pub fn set(&mut self, p: Params) {
        if p != self.p {
            self.p = p;
            self.configure();
        }
    }

    pub fn set_sample_rate(&mut self, rate: u32) {
        let rate = f64::from(rate.max(1));
        if rate != self.rate {
            self.rate = rate;
            self.configure();
            self.reset();
        }
    }

    /// Forgets what the filters and envelopes hold.
    pub fn reset(&mut self) {
        for b in &mut self.bands {
            let c = b.c;
            *b = Band { c, ..Band::default() };
        }
        self.hp_s = [State::default(); 2];
        self.lp_s = [State::default(); 2];
        self.nhp_s = State::default();
        for g in &mut self.gains {
            g.v = g.to;
        }
    }

    fn configure(&mut self) {
        let p = self.p;
        let rate = self.rate;
        let oct = p.octaves();
        let centres: Vec<f64> = p.centres().into_iter().filter(|&f| f < rate * 0.45).collect();
        self.bands.resize_with(centres.len(), Band::default);
        for (b, f) in self.bands.iter_mut().zip(centres) {
            b.c = Coefs::band(f, oct, rate);
        }
        self.order = p.order.clamp(1, MAX_ORDER);
        self.hp = Coefs::pass(p.mod_hp_hz, rate, true);
        self.lp = Coefs::pass(p.car_lp_hz, rate, false);
        self.nhp = Coefs::pass(p.noise_hp_hz, rate, true);
        let step = |ms: f64| if ms <= 0.0 { 1.0 } else { 1.0 - (-1000.0 / (ms * rate)).exp() };
        self.ka = step(p.attack_ms);
        self.kr = step(p.release_ms);
        self.kw = step(p.hold_ms.max(0.5));
        self.kg = step(10.0);
        self.hold = (p.hold_ms.max(0.0) * rate / 1000.0).round() as u32;
        // The bands' sum is louder with steeper filters' narrower skirts made up (each section narrows the band).
        let wet = p.wet * MAKEUP * (self.order as f64).sqrt();
        let to = [wet, p.mod_level, p.car_level, p.noise];
        for (g, t) in self.gains.iter_mut().zip(to) {
            g.to = t;
        }
    }

    #[inline]
    fn noise(&mut self) -> f64 {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f64::from(self.seed as i32) / 2_147_483_648.0
    }

    /// Vocodes one block: `modulator` and `carrier` are (left, right) pairs of equal length; the result goes to `out`.
    pub fn process(&mut self, modulator: (&[f32], &[f32]), carrier: (&[f32], &[f32]), out: (&mut [f32], &mut [f32])) {
        let n = out.0.len().min(out.1.len()).min(modulator.0.len()).min(modulator.1.len()).min(carrier.0.len()).min(carrier.1.len());
        let order = self.order;
        let (hold, ka, kr, kw, kg, rms) = (self.hold, self.ka, self.kr, self.kw, self.kg, self.p.rms);
        for i in 0..n {
            let (ml, mr) = (f64::from(modulator.0[i]), f64::from(modulator.1[i]));
            let (cl, cr) = (f64::from(carrier.0[i]), f64::from(carrier.1[i]));
            let wet = self.gains[0].next(kg);
            let mod_level = self.gains[1].next(kg);
            let car_level = self.gains[2].next(kg);
            let noise_level = self.gains[3].next(kg);
            let nz = if noise_level > 0.0 {
                let x = self.noise();
                self.nhp_s.run(&self.nhp, x) * noise_level
            } else {
                0.0
            };
            let m = 0.5 * (ml + mr);
            let (xl, xr) = (cl + nz, cr + nz);
            let (mut yl, mut yr) = (0.0, 0.0);
            for b in &mut self.bands {
                // The modulator's band and its envelope.
                let mut a = m;
                for s in &mut b.m[..order] {
                    a = s.run(&b.c, a);
                }
                let level = if rms {
                    b.det += (a * a - b.det) * kw;
                    b.det.max(0.0).sqrt()
                } else {
                    let x = a.abs();
                    if x >= b.det {
                        b.det = x;
                        b.held = hold;
                    } else if b.held > 0 {
                        b.held -= 1;
                    } else {
                        b.det = x;
                    }
                    b.det
                };
                b.env += (level - b.env) * if level > b.env { ka } else { kr };
                // The carrier's band at that level.
                let (mut l, mut r) = (xl, xr);
                for s in &mut b.car[0][..order] {
                    l = s.run(&b.c, l);
                }
                for s in &mut b.car[1][..order] {
                    r = s.run(&b.c, r);
                }
                yl += l * b.env;
                yr += r * b.env;
            }
            let hl = self.hp_s[0].run(&self.hp, ml);
            let hr = self.hp_s[1].run(&self.hp, mr);
            let ll = self.lp_s[0].run(&self.lp, cl);
            let lr = self.lp_s[1].run(&self.lp, cr);
            out.0[i] = (yl * wet + hl * mod_level + ll * car_level) as f32;
            out.1[i] = (yr * wet + hr * mod_level + lr * car_level) as f32;
        }
        for b in &mut self.bands {
            let [c0, c1] = &mut b.car;
            for s in b.m.iter_mut().chain(c0.iter_mut()).chain(c1.iter_mut()) {
                s.flush();
            }
            if b.env < 1e-30 {
                b.env = 0.0;
            }
            if b.det < 1e-30 {
                b.det = 0.0;
            }
        }
        for s in self.hp_s.iter_mut().chain(self.lp_s.iter_mut()).chain(std::iter::once(&mut self.nhp_s)) {
            s.flush();
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The original's saved state (spec plugins/vocodex.md "Saved state")

/// The header int a state starts with (FL 2026 writes this one).
pub const HEADER: i32 = 0x0008_0003;
/// The mapping curves before the settings.
pub const CURVES: usize = 13;
/// The band distribution's curve (its first and last level give the lowest and the highest band).
pub const DISTRIBUTION: usize = 8;
/// The settings' ints after the curves.
pub const INTS: usize = 27;
/// The switch bytes after the ints.
pub const SWITCHES: usize = 21;

// The ints (spec plugins/vocodex.md "Settings").
pub const WET: usize = 0;
pub const SG: usize = 1;
pub const MOD: usize = 2;
pub const MOD_HP: usize = 3;
pub const CAR: usize = 4;
pub const CAR_LP: usize = 5;
pub const NOISE: usize = 6;
pub const NOISE_HP: usize = 7;
pub const HOLD: usize = 8;
pub const ATTACK: usize = 9;
pub const ATTACK_CURVE: usize = 10;
pub const RELEASE: usize = 11;
pub const RELEASE_CURVE: usize = 12;
pub const WIDTH: usize = 14;
pub const FLATNESS: usize = 15;
pub const MOUTH: usize = 16;
pub const PITCH: usize = 17;
pub const UNISON_SHIFT: usize = 18;
pub const UNISON_PAN: usize = 19;
pub const RMS: usize = 20;
pub const ORDER: usize = 21;
pub const UNISON_ORDER: usize = 22;
pub const BANDS: usize = 23;
pub const DRAFT: usize = 24;
pub const MOD_INPUT: usize = 25;
pub const CAR_INPUT: usize = 26;
/// The switch byte of the L/R interlaced input (the first after the ints).
pub const LR_BYTE: usize = 0;

/// A new instance's ints (FL 2026's; the inputs Lemon Studio's: the modulator from the slot's own audio, the carrier
/// from its first sidechain input).
pub const DEFAULTS: [i32; INTS] =
    [1000, 150, 12800, 55000, 0, 28000, 0, 55000, 1666, 1115, 0, 3500, 0, 0, 349, 100, 2000, 0, 250, 128, 0, 2, 0, 47, 0, 0, 1];
/// A new instance's switch bytes: L/R off, MIN TIMES on, contour to MOD and NOISE and from consonants on, THREADED on.
pub const DEFAULT_SWITCHES: [u8; SWITCHES] = [0, 1, 1, 1, 1, 1, 0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0];
/// The default band distribution's levels (singles stored as doubles).
pub fn default_span() -> (f64, f64) {
    (f64::from_bits(0x3fc2_4bc3_6000_0000), f64::from_bits(0x3fef_bc77_e000_0000))
}

/// A new instance's saved state: the header, 13 two-point curves, the ints, the switches, an empty carrier name and
/// one latched note (60).
pub fn default_state() -> Vec<u8> {
    let (lo, hi) = default_span();
    let levels = [(0.5, 0.5), (0.5, 0.5), (0.5, 0.5), (0.0, 0.0), (0.0, 0.0), (0.5, 0.5), (0.5, 0.5), (0.5, 0.5), (lo, hi), (0.5, 0.5), (0.5, 0.5), (0.0, 0.0), (0.0, 1.0)];
    let mut b = HEADER.to_le_bytes().to_vec();
    for (y0, y1) in levels {
        for v in [3i32, 3, 2] {
            b.extend(v.to_le_bytes());
        }
        for (dx, y, last) in [(0.0f64, y0, 0u8), (1.0, y1, 2)] {
            b.extend(dx.to_le_bytes());
            b.extend(f64::to_le_bytes(y));
            b.extend(0f32.to_le_bytes());
            b.extend([0, 0, 0, last]);
        }
        b.extend([0; 4]);
        b.extend([0xff; 16]);
    }
    b.extend(DEFAULTS.iter().flat_map(|v| v.to_le_bytes()));
    b.extend(DEFAULT_SWITCHES);
    b.push(0);
    b.extend(1i32.to_le_bytes());
    b.extend(60i32.to_le_bytes());
    b
}

/// The curves' points: for each, (start, count) of its points' bytes; `None` when they do not parse.
fn curves(b: &[u8]) -> Option<(usize, [(usize, usize); CURVES])> {
    let int = |o: usize| b.get(o..o + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()));
    let mut o = 4;
    let mut at = [(0, 0); CURVES];
    for c in &mut at {
        let (kind, format, count) = (int(o)?, int(o + 4)?, int(o + 8)?);
        if kind != 3 || !(2..=3).contains(&format) || !(0..=100_000).contains(&count) {
            return None;
        }
        *c = (o + 12, count as usize);
        o += 12 + 24 * count as usize + 20;
    }
    (o <= b.len()).then_some((o, at))
}

/// Where the settings' ints start in a saved state: after the header and the 13 curves (each: kind 3, format 2 or 3,
/// a point count, 24 bytes per point, 20 bytes after). `None` when the curves do not parse or the ints are missing.
pub fn settings_at(b: &[u8]) -> Option<usize> {
    let (o, _) = curves(b)?;
    (b.len() >= o + 4 * INTS).then_some(o)
}

/// Everything Lemon Studio plays from a saved state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub v: [i32; INTS],
    /// The L/R interlaced input switch.
    pub lr: bool,
    /// The band distribution's first and last level.
    pub span: (f64, f64),
}

impl Default for Settings {
    fn default() -> Self {
        Settings { v: DEFAULTS, lr: false, span: default_span() }
    }
}

impl Settings {
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        let o = settings_at(b)?;
        let (_, at) = curves(b)?;
        let v = std::array::from_fn(|k| i32::from_le_bytes(b[o + 4 * k..o + 4 * k + 4].try_into().unwrap()));
        let lr = b.get(o + 4 * INTS + LR_BYTE).is_some_and(|x| x & 1 != 0);
        Some(Settings { v, lr, span: span_of(b, at[DISTRIBUTION]).unwrap_or(default_span()) })
    }

    /// The vocoder's settings in plain units.
    pub fn params(&self) -> Params {
        let v = &self.v;
        let (lo, hi) = self.span;
        Params {
            bands: v[BANDS].clamp(MIN_BANDS as i32, MAX_BANDS as i32) as usize,
            order: v[ORDER].clamp(1, MAX_ORDER as i32) as usize,
            low_hz: distribution_hz(lo.min(hi)),
            high_hz: distribution_hz(lo.max(hi)),
            width: 2.0 * width(v[WIDTH]),
            attack_ms: time_ms(v[ATTACK], 1000.0),
            release_ms: time_ms(v[RELEASE], 2000.0),
            hold_ms: time_ms(v[HOLD], 250.0),
            rms: v[RMS] == 1,
            wet: wet(v[WET]),
            mod_level: f64::from(os_dsp::mixer::send_gain(v[MOD].max(0))),
            car_level: f64::from(os_dsp::mixer::send_gain(v[CAR].max(0))),
            noise: noise(v[NOISE]),
            mod_hp_hz: if v[MOD_HP] <= 0 { 0.0 } else { cutoff_hz(v[MOD_HP]) },
            car_lp_hz: if v[CAR_LP] >= 65536 { 0.0 } else { cutoff_hz(v[CAR_LP]) },
            noise_hp_hz: if v[NOISE_HP] <= 0 { 0.0 } else { cutoff_hz(v[NOISE_HP]) },
        }
    }

    pub fn routing(&self) -> Routing {
        Routing { modulator: self.v[MOD_INPUT].max(0), carrier: self.v[CAR_INPUT].max(-1), lr: self.lr }
    }
}

/// The first and last level of the curve whose points start at `at.0`.
fn span_of(b: &[u8], (o, n): (usize, usize)) -> Option<(f64, f64)> {
    let y = |k: usize| b.get(o + 24 * k + 8..o + 24 * k + 16).map(|s| f64::from_le_bytes(s.try_into().unwrap()));
    let (lo, hi) = (y(0)?, y(n.checked_sub(1)?)?);
    (lo.is_finite() && hi.is_finite()).then_some((lo.clamp(0.0, 1.0), hi.clamp(0.0, 1.0)))
}

/// The wet level's gain: `(7^(v / 1000) - 1) / 6` (0 at 0, 1 at 1000, 8 at 2000).
pub fn wet(v: i32) -> f64 {
    ((f64::from(v.max(0)) / 1000.0 * 7f64.ln()).exp() - 1.0) / 6.0
}

/// The added noise's level: `v / 12800`.
pub fn noise(v: i32) -> f64 {
    f64::from(v.max(0)) / 12800.0
}

/// The pass-through and noise filters' frequency: `40 * 375^(v / 65536)` Hz (40 Hz .. 15 kHz).
pub fn cutoff_hz(v: i32) -> f64 {
    40.0 * (f64::from(v) / 65536.0 * 375f64.ln()).exp()
}

/// An envelope time in ms of a knob reaching `top` ms: `top * (500^(v / 10000) - 1) / 499`.
pub fn time_ms(v: i32, top: f64) -> f64 {
    top * ((f64::from(v.max(0)) / 10000.0 * 500f64.ln()).exp() - 1.0) / 499.0
}

/// The attack and release curves: `4^(v / 2000)` (1 .. 4).
pub fn curve_power(v: i32) -> f64 {
    (f64::from(v) / 2000.0 * 4f64.ln()).exp()
}

/// The band width as a share: `5^(v / 1000) - 1` (0.75 at 349, 4 at 1000).
pub fn width(v: i32) -> f64 {
    (f64::from(v.max(0)) / 1000.0 * 5f64.ln()).exp() - 1.0
}

/// The modulator band width multiplier: `4^((v - 2000) / 2000)` (x0.25 .. x4).
pub fn mouth(v: i32) -> f64 {
    (f64::from(v - 2000) / 2000.0 * 4f64.ln()).exp()
}

/// A level of the band distribution curve as a frequency, a 24th of an octave per editor pixel:
/// `20950 * 2^(9.275 * (u - 1))` Hz (85 Hz at the default's lowest level, 19.9 kHz at its highest). Below about 80 Hz
/// the original's scale bends down to 0 Hz at the bottom; Lemon Studio keeps the exponential, from 20 Hz.
pub fn distribution_hz(u: f64) -> f64 {
    (20950.0 * (9.275 * (u - 1.0)).exp2()).max(20.0)
}

/// Where the modulator and the carrier come from: each the number of the track's input counted from 1, 0 for the
/// slot's own audio, -1 for none (the carrier only); or the L/R interlaced input (the modulator on the left channel of
/// the slot's own audio, the carrier on the right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Routing {
    pub modulator: i32,
    pub carrier: i32,
    pub lr: bool,
}

impl Routing {
    /// The input a single sidechain feed should carry: the carrier's when it names one, else the modulator's (0: none).
    pub fn key_input(&self) -> usize {
        if self.lr {
            0
        } else if self.carrier > 0 {
            self.carrier as usize
        } else {
            self.modulator.max(0) as usize
        }
    }

    /// Picks the modulator and the carrier from the slot's own audio and its one sidechain input (`side`; silence when
    /// it has none): an input number above 0 takes the sidechain, 0 the own audio, -1 nothing. When both name inputs
    /// the host can only feed one: the modulator then takes the own audio (spec plugins/lemon-vocoder.md §1).
    pub fn pick<'a>(&self, own: (&'a [f32], &'a [f32]), side: (&'a [f32], &'a [f32]), silent: &'a [f32]) -> ((&'a [f32], &'a [f32]), (&'a [f32], &'a [f32])) {
        if self.lr {
            return ((own.0, own.0), (own.1, own.1));
        }
        let none = (silent, silent);
        let car = match self.carrier {
            0 => own,
            c if c > 0 => side,
            _ => none,
        };
        let m = match self.modulator {
            m if m > 0 && self.carrier <= 0 => side,
            _ => own,
        };
        (m, car)
    }
}

/// A slot effect playing a project's Vocodex: the modulator and the carrier come from the slot's own audio or its
/// sidechain input as the state's selectors say.
pub struct VocoderFx {
    v: Vocoder,
    routing: Routing,
    own: [Vec<f32>; 2],
    side: [Vec<f32>; 2],
    out: [Vec<f32>; 2],
    silent: Vec<f32>,
}

impl VocoderFx {
    pub fn new(rate: u32, s: &Settings) -> VocoderFx {
        VocoderFx { v: Vocoder::new(rate, s.params()), routing: s.routing(), own: Default::default(), side: Default::default(), out: Default::default(), silent: Vec::new() }
    }

    pub fn from_state(state: &[u8], rate: u32) -> Option<VocoderFx> {
        Some(VocoderFx::new(rate, &Settings::from_state(state)?))
    }

    fn run(&mut self, buf: &mut [f32], key: Option<&[f32]>) {
        let n = buf.len() / 2;
        for v in self.own.iter_mut().chain(self.side.iter_mut()).chain(self.out.iter_mut()).chain(std::iter::once(&mut self.silent)) {
            v.clear();
            v.resize(n, 0.0);
        }
        for i in 0..n {
            self.own[0][i] = buf[2 * i];
            self.own[1][i] = buf[2 * i + 1];
            if let Some(k) = key {
                self.side[0][i] = k.get(2 * i).copied().unwrap_or(0.0);
                self.side[1][i] = k.get(2 * i + 1).copied().unwrap_or(0.0);
            }
        }
        let (m, c) = self.routing.pick((&self.own[0], &self.own[1]), (&self.side[0], &self.side[1]), &self.silent);
        let [ol, or] = &mut self.out;
        self.v.process(m, c, (ol, or));
        for i in 0..n {
            buf[2 * i] = self.out[0][i];
            buf[2 * i + 1] = self.out[1][i];
        }
    }
}

impl Effect for VocoderFx {
    fn process(&mut self, buf: &mut [f32]) {
        self.run(buf, None);
    }

    fn reset(&mut self) {
        self.v.reset();
    }

    fn key_input(&self) -> usize {
        self.routing.key_input()
    }

    fn process_keyed(&mut self, buf: &mut [f32], key: &[f32]) {
        self.run(buf, Some(key));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn white(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (f64::from(s as i32) / 2_147_483_648.0 * 0.5) as f32
            })
            .collect()
    }

    fn sine(n: usize, hz: f64, amp: f64) -> Vec<f32> {
        (0..n).map(|i| (amp * (TAU * hz * i as f64 / 44100.0).sin()) as f32).collect()
    }

    fn run(v: &mut Vocoder, m: &[f32], c: &[f32]) -> Vec<f32> {
        let mut out = Vec::new();
        for (mb, cb) in m.chunks(512).zip(c.chunks(512)) {
            let (mut l, mut r) = (vec![0.0; mb.len()], vec![0.0; mb.len()]);
            v.process((mb, mb), (cb, cb), (&mut l, &mut r));
            out.extend(l);
        }
        out
    }

    /// The power of `x` at `hz` (a Goertzel-like projection over the whole signal).
    fn power_at(x: &[f32], hz: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &s) in x.iter().enumerate() {
            let w = TAU * hz * i as f64 / 44100.0;
            re += f64::from(s) * w.cos();
            im += f64::from(s) * w.sin();
        }
        (re * re + im * im) / (x.len() as f64).powi(2)
    }

    /// The energy of `x` in a band around `hz` (a band-pass of a third of an octave).
    fn band_energy(x: &[f32], hz: f64) -> f64 {
        let c = Coefs::band(hz, 1.0 / 3.0, 44100.0);
        let (mut a, mut b) = (State::default(), State::default());
        x.iter().map(|&s| b.run(&c, a.run(&c, f64::from(s)))).map(|y| y * y).sum::<f64>() / x.len() as f64
    }

    #[test]
    fn silence_in_gives_silence_out() {
        for rms in [false, true] {
            let p = Params { noise: 0.5, mod_level: 1.0, car_level: 1.0, rms, ..Params::default() };
            let mut v = Vocoder::new(44100, p);
            let z = vec![0.0f32; 8192];
            assert!(run(&mut v, &z, &z).iter().all(|&s| s == 0.0));
            // A carrier with a silent modulator: nothing either (the pass-throughs off).
            let mut v = Vocoder::new(44100, Params { rms, ..Params::default() });
            assert!(run(&mut v, &z, &white(8192, 3)).iter().all(|&s| s == 0.0));
        }
    }

    #[test]
    fn a_modulator_in_one_band_passes_mostly_that_band_of_the_carrier() {
        let p = Params { bands: 16, ..Params::default() };
        let centres = p.centres();
        let n = 44100;
        let carrier = white(n, 7);
        for &k in &[3usize, 8, 12] {
            let f = centres[k];
            let mut v = Vocoder::new(44100, p);
            let out = run(&mut v, &sine(n, f, 0.25), &carrier);
            let tail = &out[n / 4..];
            let at = band_energy(tail, f);
            // Two bands or more away, an octave and more off.
            for &j in &[k - 3, k + 3] {
                let off = band_energy(tail, centres[j]);
                assert!(at > 30.0 * off, "band {k} ({f:.0} Hz): {at:e} against band {j}: {off:e}");
            }
            // The output is loud enough to hear: within 20 dB of the carrier's level in that band.
            assert!(at > band_energy(&carrier, f) * 0.01, "band {k}: {at:e}");
        }
        // A sine between bands is the same story; a sine carrier under a noise modulator comes out as that sine.
        let mut v = Vocoder::new(44100, p);
        let out = run(&mut v, &white(n, 11), &sine(n, 1000.0, 0.5));
        assert!(power_at(&out[n / 2..], 1000.0) > 100.0 * power_at(&out[n / 2..], 1500.0));
    }

    #[test]
    fn the_pass_throughs_filter_and_the_wet_level_scales() {
        let n = 22050;
        let m = sine(n, 200.0, 0.5);
        let c = sine(n, 8000.0, 0.5);
        // Wet off, the modulator through its high-pass (5 kHz: 200 Hz all but gone), the carrier through its low-pass.
        let p = Params { wet: 0.0, mod_level: 1.0, car_level: 1.0, ..Params::default() };
        let mut v = Vocoder::new(44100, p);
        let out = run(&mut v, &m, &c);
        let peak = out[n / 2..].iter().fold(0f32, |a, &s| a.max(s.abs()));
        assert!(peak < 0.05, "both filtered away: {peak}");
        let p = Params { wet: 0.0, mod_level: 1.0, mod_hp_hz: 20.0, ..Params::default() };
        let mut v = Vocoder::new(44100, p);
        let out = run(&mut v, &m, &c);
        let peak = out[n / 2..].iter().fold(0f32, |a, &s| a.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.03, "the modulator passes: {peak}");
        // Half the wet level: half the output (once the glide is done).
        let full = run(&mut Vocoder::new(44100, Params::default()), &white(n, 1), &white(n, 2));
        let half = run(&mut Vocoder::new(44100, Params { wet: 0.5, ..Params::default() }), &white(n, 1), &white(n, 2));
        let e = |x: &[f32]| x[n / 2..].iter().map(|&s| f64::from(s).powi(2)).sum::<f64>();
        assert!((e(&half) / e(&full) - 0.25).abs() < 1e-3);
        let _ = PI;
    }

    /// A new instance's state is FL 2026's 1182 bytes; the laws give the original's texts at their marks
    /// (spec plugins/vocodex.md "Settings").
    #[test]
    fn the_state_reads_and_the_laws_match_the_measured_texts() {
        let b = default_state();
        assert_eq!(b.len(), 1182);
        assert_eq!(settings_at(&b), Some(1044));
        assert_eq!(Settings::from_state(&b), Some(Settings::default()));
        let close = |a: f64, b: f64, tol: f64| (a - b).abs() <= tol;
        assert!(close(wet(1000), 1.0, 1e-12) && close(wet(2000), 8.0, 1e-9) && close(wet(320), 0.14, 0.005));
        assert!(close(cutoff_hz(55000), 5785.0, 1.0) && close(cutoff_hz(28000), 503.0, 0.5) && close(cutoff_hz(10240), 101.0, 0.5));
        assert!(close(cutoff_hz(65536), 15000.0, 1.0) && close(cutoff_hz(0), 40.0, 1e-9));
        assert!(close(time_ms(1666, 250.0), 0.91, 0.005) && close(time_ms(1115, 1000.0), 2.0, 0.005));
        assert!(close(time_ms(3500, 2000.0), 31.24, 0.05) && close(time_ms(10000, 2000.0), 2000.0, 1e-9));
        assert!(close(width(349), 0.75, 0.005) && close(width(165), 0.30, 0.005) && close(width(1000), 4.0, 1e-9));
        assert!(close(curve_power(400), 1.32, 0.005) && close(mouth(640), 0.39, 0.005));
        assert!(close(distribution_hz(default_span().0), 85.0, 1.0) && close(distribution_hz(default_span().1), 19900.0, 150.0));
        let p = Settings::default().params();
        assert_eq!((p.bands, p.order, p.rms), (47, 2, false));
        assert!(p.car_level == 0.0 && (p.mod_level - 1.0).abs() < 1e-6);
        // A state whose first curve has eight points: the settings are found after it; the L/R byte reads.
        let mut s = b[..4].to_vec();
        s.extend([3i32, 3, 8].iter().flat_map(|v| v.to_le_bytes()));
        for k in 0..8 {
            s.extend(f64::to_le_bytes(if k == 0 { 0.0 } else { 0.125 }));
            s.extend(0.5f64.to_le_bytes());
            s.extend([0; 8]);
        }
        s.extend([0; 4]);
        s.extend([0xff; 16]);
        s.extend(&b[84..]);
        let mut v = DEFAULTS;
        (v[BANDS], v[MOD_INPUT], v[CAR_INPUT]) = (12, 1, 0);
        let o = settings_at(&s).unwrap();
        assert_eq!(o, 1044 + 6 * 24);
        s[o..o + 4 * INTS].copy_from_slice(&v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>());
        s[o + 4 * INTS] = 1;
        let r = Settings::from_state(&s).unwrap();
        assert_eq!((r.v[BANDS], r.lr), (12, true));
        assert_eq!(r.routing(), Routing { modulator: 1, carrier: 0, lr: true });
        assert!(Settings::from_state(&s[..o + 50]).is_none());
        assert!(Settings::from_state(&s[..500]).is_none());
    }

    /// The slot effect takes the modulator from the sidechain when its selector names an input and the carrier is the
    /// slot's own audio (the original's production set-up), and the other way round.
    #[test]
    fn the_slot_effect_routes_its_inputs() {
        let n = 8192;
        let voice = sine(n, 1000.0, 0.3);
        let pad = white(n, 9);
        let stereo = |x: &[f32]| x.iter().flat_map(|&s| [s, s]).collect::<Vec<f32>>();
        let play = |s: &Settings, own: &[f32], side: Option<&[f32]>| {
            let mut fx = VocoderFx::new(44100, s);
            let (own, side) = (stereo(own), side.map(stereo));
            let mut out = Vec::new();
            for (k, b) in own.chunks(1024).enumerate() {
                let mut b = b.to_vec();
                let len = b.len();
                match &side {
                    Some(sd) => fx.process_keyed(&mut b, &sd[k * 1024..k * 1024 + len]),
                    None => fx.process(&mut b),
                }
                out.extend(b.iter().step_by(2).copied());
            }
            out
        };
        let mut s = Settings::default();
        s.v[MOD] = 0;
        (s.v[MOD_INPUT], s.v[CAR_INPUT]) = (1, 0);
        assert_eq!(VocoderFx::new(44100, &s).key_input(), 1);
        let out = play(&s, &pad, Some(&voice));
        assert!(band_energy(&out[n / 2..], 1000.0) > 1e-5, "the voice opens the pad's 1 kHz bands");
        // Carrier from the sidechain, modulator the own audio (Lemon Studio's default).
        let d = Settings { v: { let mut v = s.v; (v[MOD_INPUT], v[CAR_INPUT]) = (0, 1); v }, ..s };
        assert_eq!(VocoderFx::new(44100, &d).key_input(), 1);
        let out2 = play(&d, &voice, Some(&pad));
        assert!((band_energy(&out2[n / 2..], 1000.0) / band_energy(&out[n / 2..], 1000.0) - 1.0).abs() < 1e-6);
        // No sidechain: the carrier is silent.
        assert!(play(&d, &voice, None).iter().all(|&x| x == 0.0));
        // L/R interlaced: modulator left, carrier right of the own audio.
        let lr = Settings { lr: true, ..d };
        assert_eq!(VocoderFx::new(44100, &lr).key_input(), 0);
    }

    #[test]
    fn noise_reaches_the_bands_only() {
        let n = 22050;
        // Noise with a silent carrier: the modulator's bands play it.
        let p = Params { noise: 0.5, ..Params::default() };
        let out = run(&mut Vocoder::new(44100, p), &white(n, 5), &vec![0.0; n]);
        assert!(out[n / 2..].iter().any(|&s| s.abs() > 0.01));
    }
}
