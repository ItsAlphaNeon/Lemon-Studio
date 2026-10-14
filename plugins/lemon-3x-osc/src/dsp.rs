// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon 3x Osc sound: three oscillators, their stereo spread, mixing or amplitude modulation, the five envelope / LFO
//! targets, the voice filter and the voices.
//!
// spec: plugins/3x-osc.md §2

use std::f64::consts::PI;

use os_engine::{envelope, laws};

/// Key 60 sounds C6 (spec §2.1).
pub const KEY60_HZ: f64 = 523.251_130_601_197_2;
/// Note-off fade without a volume envelope (raised cosine, spec §2.5).
pub const FADE_SECONDS: f64 = 0.010;
/// Modulation (envelopes, LFOs, pitch, filter) is updated every this many samples.
pub const CTL: usize = 32;
pub const MAX_VOICES: usize = 64;

pub const T_PAN: usize = 0;
pub const T_VOL: usize = 1;
pub const T_CUT: usize = 2;
pub const T_RES: usize = 3;
pub const T_PITCH: usize = 4;

/// Shapes (spec §2.2).
pub const SINE: u8 = 0;
pub const TRIANGLE: u8 = 1;
pub const SQUARE: u8 = 2;
pub const SAW: u8 = 3;
pub const ROUNDED_SAW: u8 = 4;
pub const NOISE: u8 = 5;
pub const CUSTOM: u8 = 6;

/// Rounded saw: time constant (periods) and where the wave drops to −1.
const RS_TAU: f64 = 1.0 / 48.0;
const RS_DROP: f64 = 5.0 / 6.0;

/// One oscillator's settings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Osc {
    pub shape: u8,
    pub invert: bool,
    /// −1 (left) .. 1 (right).
    pub pan: f64,
    /// Coarse + fine pitch in semitones.
    pub semis: f64,
    /// Right side's phase lead in cycles (stereo phase offset: ±½).
    pub phase: f64,
    /// Right side's detune in cents.
    pub detune: f64,
}

/// One LFO (spec §2.6).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lfo {
    /// 0 sine, 1 triangle, 2 pulse.
    pub shape: u8,
    pub delay: f64,
    pub attack: f64,
    /// Stored amount (pitch: cents; others ±128).
    pub amount: f64,
    pub hz: f64,
}

impl Lfo {
    fn wave(&self, ph: f64) -> f64 {
        let ph = ph.fract();
        match self.shape {
            0 => (2.0 * PI * ph).sin(),
            1 => 1.0 - 4.0 * ((ph + 0.25).fract() - 0.5).abs(),
            _ => {
                if ph < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }

    /// The LFO's wave (−1..1, faded in) at voice time `t`, times the amount.
    pub fn value(&self, t: f64) -> f64 {
        if self.amount == 0.0 || t < self.delay {
            return 0.0;
        }
        let tl = t - self.delay;
        let fade = if self.attack > 0.0 { (tl / self.attack).min(1.0) } else { 1.0 };
        self.amount * fade * self.wave(self.hz * tl)
    }
}

/// LFO speed law: 32950 = 0.505 Hz, 8 octaves over the range (spec §2.6).
pub fn lfo_hz(speed: i32) -> f64 {
    0.505 * 2f64.powf(8.0 * f64::from(speed - 32950) / 65536.0)
}

/// An envelope / LFO record's settings.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Target {
    /// The envelope when switched on (its `amount` = the stored amount).
    pub env: Option<envelope::Params>,
    pub lfo: Lfo,
}

impl Target {
    /// Envelope level × amount plus the LFO at time `t` (stored units), `None` once a volume envelope has ended.
    fn mod_at(&self, t: f64, off: Option<f64>) -> f64 {
        let e = self.env.map_or(0.0, |e| e.amount * e.level(t, off).unwrap_or(0.0));
        e + self.lfo.value(t)
    }

    fn modulates(&self) -> bool {
        self.env.is_some_and(|e| e.amount != 0.0) || self.lfo.amount != 0.0
    }
}

/// Settings read once per block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    pub osc: [Osc; 3],
    /// Osc 2 / osc 3 mix level, 0..1.
    pub m2: f64,
    pub m3: f64,
    /// Osc 3 = amplitude modulation.
    pub am: bool,
    pub hq: bool,
    /// Stereo phase randomness 0..1.
    pub rand: f64,
    pub targets: [Target; 5],
    pub filter: u8,
    /// Stored cutoff / resonance, 0..256.
    pub cutoff: f64,
    pub resonance: f64,
    /// Pitch bend in cents.
    pub bend: f64,
}

impl Default for Block {
    fn default() -> Self {
        let osc = |semis: f64| Osc { semis, ..Osc::default() };
        Block {
            osc: [osc(0.0), osc(-12.0), osc(-24.0)],
            m2: 0.5,
            m3: 0.25,
            am: false,
            hq: false,
            rand: 0.0,
            targets: [Target::default(); 5],
            filter: 0,
            cutoff: 256.0,
            resonance: 0.0,
            bend: 0.0,
        }
    }
}

impl Block {
    /// No filter: cutoff 256, resonance 0 and nothing modulating them (spec §2.7).
    pub fn filter_off(&self) -> bool {
        self.cutoff >= 256.0 && self.resonance <= 0.0 && !self.targets[T_CUT].modulates() && !self.targets[T_RES].modulates()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Waves (spec §2.2)

/// The wave at phase `u` (0..1).
pub fn wave(shape: u8, u: f64) -> f64 {
    match shape {
        SINE => (2.0 * PI * u).sin(),
        TRIANGLE => {
            if u < 0.25 {
                4.0 * u
            } else if u < 0.75 {
                2.0 - 4.0 * u
            } else {
                4.0 * u - 4.0
            }
        }
        SQUARE => {
            if u < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        SAW => 2.0 * u - 1.0,
        ROUNDED_SAW => {
            if u < RS_DROP {
                1.0 - 2.0 * (-u / RS_TAU).exp()
            } else {
                -1.0
            }
        }
        _ => 0.0,
    }
}

/// The integral of one period's wave from 0 to `u` (0..=1).
fn prim1(shape: u8, u: f64) -> f64 {
    match shape {
        SINE => (1.0 - (2.0 * PI * u).cos()) / (2.0 * PI),
        TRIANGLE => {
            if u < 0.25 {
                2.0 * u * u
            } else if u < 0.75 {
                2.0 * u - 2.0 * u * u - 0.25
            } else {
                2.0 * u * u - 4.0 * u + 2.0
            }
        }
        SQUARE => {
            if u < 0.5 {
                u
            } else {
                1.0 - u
            }
        }
        SAW => u * u - u,
        ROUNDED_SAW => {
            let p = |u: f64| u - 2.0 * RS_TAU * (1.0 - (-u / RS_TAU).exp());
            if u < RS_DROP {
                p(u)
            } else {
                p(RS_DROP) - (u - RS_DROP)
            }
        }
        _ => 0.0,
    }
}

/// The integral of the periodic wave from 0 to `x` (any x ≥ 0).
fn prim(shape: u8, x: f64) -> f64 {
    let n = x.floor();
    n * prim1(shape, 1.0) + prim1(shape, x - n)
}

/// The wave's average over the next sample interval (phase `u` .. `u + dt`): the plain oscillators (spec §2.2 "Edges").
pub fn boxed(shape: u8, u: f64, dt: f64) -> f64 {
    if dt < 1e-9 {
        return wave(shape, u);
    }
    (prim(shape, u + dt) - prim(shape, u)) / dt
}

/// Band-limited step correction (polyBLEP) for a unit jump at phase 0.
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// The high-quality oscillators: the wave with band-limited edges (Lemon Studio choice: polyBLEP).
pub fn hq(shape: u8, u: f64, dt: f64) -> f64 {
    let dt = dt.min(0.5);
    match shape {
        SAW => 2.0 * u - 1.0 - blep(u, dt),
        SQUARE => wave(SQUARE, u) + blep(u, dt) - blep((u + 0.5).fract(), dt),
        ROUNDED_SAW => wave(ROUNDED_SAW, u) - blep((u - RS_DROP + 1.0).fract(), dt),
        _ => wave(shape, u),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Filter (spec §2.7)

/// Log interpolation through `(x, y)` points, extrapolated with the end segments.
fn log_interp(pts: &[(f64, f64)], x: f64) -> f64 {
    let n = pts.len();
    let i = pts.windows(2).position(|w| x <= w[1].0).unwrap_or(n - 2);
    let (x0, y0) = pts[i];
    let (x1, y1) = pts[i + 1];
    (y0.ln() + (y1.ln() - y0.ln()) * (x - x0) / (x1 - x0)).exp()
}

/// Natural frequency of the LP-family types (1..5) for a stored cutoff.
pub fn lp_hz(c: f64) -> f64 {
    log_interp(&[(32.0, 405.0), (64.0, 837.0), (128.0, 2727.0), (192.0, 8203.0)], c)
}

/// Damping of the LP-family types for a stored resonance.
pub fn lp_damping(r: f64) -> f64 {
    log_interp(&[(0.0, 10.0), (128.0, 0.949), (256.0, 0.227)], r)
}

/// Fast LP coefficient at 44.1 kHz for a stored cutoff (one-pole `y += a (x − y)`).
pub fn fast_lp_a(c: f64) -> f64 {
    (0.00655 * 5.975f64.powf((c - 64.0) / 64.0)).min(1.0)
}

/// SVF types (6, 7): natural frequency and Q.
pub fn svf_hz(c: f64) -> f64 {
    280.0 * 3.9f64.powf((c - 64.0) / 64.0)
}

pub fn svf_q(r: f64) -> f64 {
    log_interp(&[(0.0, 0.75), (128.0, 1.6), (192.0, 2.4)], r)
}

/// The SVF types' response floor (−43 dB at Nyquist).
const SVF_FLOOR: f64 = 0.007;

/// Trapezoidal state-variable filter, one channel.
#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    ic1: f64,
    ic2: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SvfCoefs {
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
}

impl SvfCoefs {
    fn new(hz: f64, k: f64, sr: f64) -> Self {
        let g = (PI * hz.clamp(5.0, sr * 0.49) / sr).tan();
        let a1 = 1.0 / (1.0 + g * (g + k));
        SvfCoefs { a1, a2: g * a1, a3: g * g * a1, k }
    }
}

impl Svf {
    /// (low, band, high).
    #[inline]
    fn tick(&mut self, c: &SvfCoefs, x: f64) -> (f64, f64, f64) {
        let v3 = x - self.ic2;
        let v1 = c.a1 * self.ic1 + c.a2 * v3;
        let v2 = self.ic2 + c.a2 * self.ic1 + c.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, x - c.k * v1 - v2)
    }
}

/// A filter setting for one control span.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Coefs {
    Off,
    OnePole(f64),
    /// Coefficients, output (1 LP, 2 BP, 3 HP, 4 BS, 5 LPx2, 6 SVF LP, 7 SVF LPx2).
    Svf(SvfCoefs, u8),
}

fn coefs(kind: u8, c: f64, r: f64, sr: f64) -> Coefs {
    let (c, r) = (c.clamp(0.0, 256.0), r.clamp(0.0, 256.0));
    match kind {
        0 => {
            let a = fast_lp_a(c);
            if a >= 1.0 {
                return Coefs::OnePole(1.0);
            }
            // Same corner frequency at any rate.
            let fc = -(1.0 - a).ln() * 44100.0 / (2.0 * PI);
            Coefs::OnePole(1.0 - (-2.0 * PI * fc / sr).exp())
        }
        1..=5 => Coefs::Svf(SvfCoefs::new(lp_hz(c), lp_damping(r), sr), kind),
        _ => Coefs::Svf(SvfCoefs::new(svf_hz(c), 1.0 / svf_q(r), sr), kind.min(7)),
    }
}

/// One side's filter memory.
#[derive(Clone, Copy, Debug, Default)]
struct Filter {
    y: f64,
    s1: Svf,
    s2: Svf,
}

impl Filter {
    #[inline]
    fn tick(&mut self, c: &Coefs, x: f64) -> f64 {
        match c {
            Coefs::Off => x,
            Coefs::OnePole(a) => {
                self.y += a * (x - self.y);
                self.y
            }
            Coefs::Svf(k, kind) => {
                let (lp, bp, hp) = self.s1.tick(k, x);
                match kind {
                    1 => lp,
                    2 => k.k * bp,
                    3 => hp,
                    4 => lp + hp,
                    5 => self.s2.tick(k, lp).0,
                    6 => lp + SVF_FLOOR * x,
                    _ => {
                        let y = lp + SVF_FLOOR * x;
                        self.s2.tick(k, y).0 + SVF_FLOOR * y
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Voices (spec §2.3 - §2.5)

struct Voice {
    key: u8,
    id: Option<i32>,
    /// Started by a host note: its end is reported back (spec: audio/voices.md §4). Editor previews are not.
    host: bool,
    /// Velocity gain.
    vel: f64,
    /// Samples since the note-on.
    n: u64,
    /// Note-off sample.
    off: Option<u64>,
    /// Cut by the host (choke) at this sample: the 10 ms fade.
    cut: Option<u64>,
    /// Phase per oscillator and side (cycles, 0..1).
    ph: [[f64; 2]; 3],
    filt: [Filter; 2],
    done: bool,
}

/// Raised-cosine fade factor `s` seconds into a 10 ms fade.
fn fade(s: f64) -> f64 {
    if s <= 0.0 {
        1.0
    } else if s >= FADE_SECONDS {
        0.0
    } else {
        0.5 + 0.5 * (PI * s / FADE_SECONDS).cos()
    }
}

pub struct Engine {
    rate: f64,
    voices: Vec<Voice>,
    rng: u64,
    /// Host voices that ended since the last `take_ended`: (key, host note id).
    ended: Vec<(u8, Option<i32>)>,
}

impl Engine {
    pub fn new(rate: f32) -> Self {
        Engine { rate: f64::from(rate), voices: Vec::with_capacity(MAX_VOICES), rng: 0x9e37_79b9_7f4a_7c15, ended: Vec::with_capacity(MAX_VOICES * 2) }
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.rate = f64::from(rate);
        self.voices.clear();
        self.ended.clear();
    }

    /// Drops every voice. Not reported: a host resetting the instrument forgets its voices itself.
    pub fn reset(&mut self) {
        self.voices.clear();
        self.ended.clear();
    }

    /// Host voices that ended since the last call: (key, host note id).
    pub fn take_ended(&mut self) -> std::vec::Drain<'_, (u8, Option<i32>)> {
        self.ended.drain(..)
    }

    pub fn has_voices(&self) -> bool {
        !self.voices.is_empty()
    }

    /// Keys of the voices still held or ringing.
    pub fn sounding(&self) -> impl Iterator<Item = u8> + '_ {
        self.voices.iter().filter(|v| !v.done).map(|v| v.key)
    }

    /// Uniform −1..1 (xorshift).
    fn noise(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x >> 11) as f64 / (1u64 << 52) as f64 - 1.0
    }

    /// Starts a voice of the plugin's own (the editor's keyboard): velocity 0..1 (the host's byte / 128).
    pub fn note_on(&mut self, key: u8, velocity: f32, id: Option<i32>, b: &Block) {
        self.start(key, velocity, id, false, b);
    }

    /// Starts the voice of a host note: its end is reported (spec: audio/voices.md §4).
    pub fn host_note_on(&mut self, key: u8, velocity: f32, id: Option<i32>, b: &Block) {
        self.start(key, velocity, id, true, b);
    }

    fn start(&mut self, key: u8, velocity: f32, id: Option<i32>, host: bool, b: &Block) {
        if self.voices.len() >= MAX_VOICES {
            let v = self.voices.remove(0);
            if v.host {
                self.ended.push((v.key, v.id));
            }
        }
        let mut ph = [[0.0; 2]; 3];
        for (n, p) in ph.iter_mut().enumerate() {
            let mut r = [0.0; 2];
            if b.rand > 0.0 {
                // Random start within ±(r/64)·½ cycle per side (spec §2.3).
                for x in &mut r {
                    *x = self.noise() * b.rand * 0.5;
                }
            }
            p[0] = r[0].rem_euclid(1.0);
            p[1] = (b.osc[n].phase + r[1]).rem_euclid(1.0);
        }
        let byte = (velocity.clamp(0.0, 1.0) * 128.0).round() as u8;
        self.voices.push(Voice {
            key,
            id,
            host,
            vel: laws::velocity_gain(byte),
            n: 0,
            off: None,
            cut: None,
            ph,
            filt: [Filter::default(); 2],
            done: false,
        });
    }

    pub fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.off.is_none()) {
            v.off = Some(v.n);
        }
    }

    /// Note-off of one voice by its host id (falls back to the key).
    pub fn note_off_id(&mut self, key: u8, id: Option<i32>) {
        if let Some(i) = id {
            if let Some(v) = self.voices.iter_mut().find(|v| v.id == Some(i) && v.off.is_none()) {
                v.off = Some(v.n);
                return;
            }
        }
        self.note_off(key);
    }

    /// Cuts the voices of `key` with the short fade.
    pub fn choke(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.cut.is_none()) {
            v.cut = Some(v.n);
        }
    }

    pub fn all_off(&mut self) {
        for v in self.voices.iter_mut().filter(|v| v.cut.is_none()) {
            v.cut = Some(v.n);
        }
    }

    /// Voice gain at sample `n` (velocity, volume envelope and LFO, note-off fade); `None` once the voice is over.
    fn gain_at(&self, v: &Voice, b: &Block, n: u64) -> Option<f64> {
        let sr = self.rate;
        let t = n as f64 / sr;
        let off = v.off.map(|o| o as f64 / sr);
        let vol = &b.targets[T_VOL];
        let mut g = v.vel;
        match vol.env {
            Some(e) => g *= e.level(t, off)?,
            None => {
                if let Some(o) = off {
                    if t - o >= FADE_SECONDS {
                        return None;
                    }
                    g *= fade(t - o);
                }
            }
        }
        if let Some(c) = v.cut {
            let s = (n.saturating_sub(c)) as f64 / sr;
            if s >= FADE_SECONDS {
                return None;
            }
            g *= fade(s);
        }
        // Volume LFO: gain × (1 + a·w), a = amount / 128.
        g *= 1.0 + vol.lfo.value(t) / 128.0;
        Some(g)
    }

    /// Adds the voices into `l` / `r`.
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32], b: &Block) {
        let len = l.len().min(r.len());
        let mut at = 0;
        while at < len {
            let end = (at + CTL).min(len);
            self.span(&mut l[at..end], &mut r[at..end], b);
            at = end;
        }
        let ended = &mut self.ended;
        self.voices.retain(|v| {
            if v.done && v.host {
                ended.push((v.key, v.id));
            }
            !v.done
        });
    }

    fn span(&mut self, l: &mut [f32], r: &mut [f32], b: &Block) {
        let len = l.len();
        let sr = self.rate;
        // Constant-power pan gains per oscillator and side.
        let pan: [[f64; 2]; 3] = std::array::from_fn(|n| {
            let a = (1.0 + b.osc[n].pan.clamp(-1.0, 1.0)) * PI / 4.0;
            [a.cos(), a.sin()]
        });
        // Mix weights: osc 1, osc 2, osc 3 (AM: osc 3 modulates instead).
        let w = [1.0 - b.m2, b.m2];
        let (use1, use2) = (w[0] != 0.0, w[1] != 0.0);
        let use3 = b.m3 != 0.0;
        let filter_off = b.filter_off();
        let mut voices = std::mem::take(&mut self.voices);
        for v in voices.iter_mut().filter(|v| !v.done) {
            let t0 = v.n as f64 / sr;
            let off = v.off.map(|o| o as f64 / sr);
            let Some(g0) = self.gain_at(v, b, v.n) else {
                v.done = true;
                continue;
            };
            // A voice that ends inside this span fades to nothing in it.
            let g1 = self.gain_at(v, b, v.n + len as u64).unwrap_or_else(|| {
                v.done = true;
                0.0
            });
            // Pitch: envelope (cents) + LFO (cents) + bend.
            let cents = b.targets[T_PITCH].mod_at(t0, off) + b.bend;
            let base = KEY60_HZ * 2f64.powf((f64::from(v.key) - 60.0) / 12.0 + cents / 1200.0);
            let inc: [[f64; 2]; 3] = std::array::from_fn(|n| {
                let f = base * 2f64.powf(b.osc[n].semis / 12.0) / sr;
                [f, f * 2f64.powf(b.osc[n].detune / 1200.0)]
            });
            let co = if filter_off {
                Coefs::Off
            } else {
                let c = b.cutoff + b.targets[T_CUT].mod_at(t0, off) / 128.0 * 256.0;
                let q = b.resonance + b.targets[T_RES].mod_at(t0, off) / 128.0 * 256.0;
                coefs(b.filter, c, q, sr)
            };
            let dg = (g1 - g0) / len as f64;
            for i in 0..len {
                let g = g0 + dg * i as f64;
                let mut out = [0.0f64; 2];
                for (s, o) in out.iter_mut().enumerate() {
                    let mut osc = [0.0f64; 3];
                    for n in 0..3 {
                        let used = match n {
                            0 => use1,
                            1 => use2,
                            _ => use3,
                        };
                        let os = &b.osc[n];
                        let u = v.ph[n][s];
                        let dt = inc[n][s];
                        if used {
                            let x = match os.shape {
                                NOISE => self.noise(),
                                CUSTOM => 0.0,
                                sh if b.hq => hq(sh, u, dt),
                                sh => boxed(sh, u, dt),
                            };
                            osc[n] = if os.invert { -x } else { x } * pan[n][s];
                        }
                        let p = u + dt;
                        v.ph[n][s] = p - p.floor();
                    }
                    let mix12 = w[0] * osc[0] + w[1] * osc[1];
                    let y = if b.am { mix12 * (1.0 - b.m3 * osc[2]) } else { (1.0 - b.m3) * mix12 + b.m3 * osc[2] };
                    *o = v.filt[s].tick(&co, y) * g;
                }
                l[i] += out[0] as f32;
                r[i] += out[1] as f32;
            }
            v.n += len as u64;
        }
        self.voices = voices;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;

    fn render(b: &Block, key: u8, vel: f32, frames: usize, off_at: Option<usize>) -> (Vec<f32>, Vec<f32>) {
        let mut e = Engine::new(SR);
        e.note_on(key, vel, None, b);
        let (mut l, mut r) = (vec![0.0; frames], vec![0.0; frames]);
        match off_at {
            Some(o) => {
                e.render(&mut l[..o], &mut r[..o], b);
                e.note_off(key);
                e.render(&mut l[o..], &mut r[o..], b);
            }
            None => e.render(&mut l, &mut r, b),
        }
        (l, r)
    }

    /// Only osc 1 sounds: mix levels 0.
    fn solo(shape: u8) -> Block {
        let mut b = Block { m2: 0.0, m3: 0.0, ..Block::default() };
        b.osc[0].shape = shape;
        b
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Frequency by zero crossings (rising) over a stretch.
    fn freq(x: &[f32]) -> f64 {
        let ups: Vec<f64> = (1..x.len())
            .filter(|&i| x[i - 1] < 0.0 && x[i] >= 0.0)
            .map(|i| (i - 1) as f64 + f64::from(-x[i - 1] / (x[i] - x[i - 1])))
            .collect();
        f64::from(SR) * (ups.len() - 1) as f64 / (ups[ups.len() - 1] - ups[0])
    }

    #[test]
    fn one_oscillator_peaks_at_one_before_pan_and_velocity_100_is_unity() {
        let (l, r) = render(&solo(SINE), 60, 100.0 / 128.0, 4410, None);
        assert!((peak(&l) - 0.70711).abs() < 2e-3, "{}", peak(&l));
        assert!((peak(&r) - 0.70711).abs() < 2e-3);
        // Velocity 50: the sampler's law, 0.23165.
        let (l, _) = render(&solo(SINE), 60, 50.0 / 128.0, 4410, None);
        assert!((peak(&l) / 0.70711 - 0.23165).abs() < 2e-3);
    }

    #[test]
    fn pitch_follows_the_measured_formula() {
        let (l, _) = render(&solo(SINE), 60, 0.78, 44100, None);
        assert!((freq(&l) - 523.25).abs() < 0.05, "{}", freq(&l));
        let (l, _) = render(&solo(SINE), 48, 0.78, 44100, None);
        assert!((freq(&l) - 261.63).abs() < 0.05);
        let mut b = solo(SINE);
        b.osc[0].semis = 7.0 + 0.5;
        let (l, _) = render(&b, 60, 0.78, 44100, None);
        assert!((freq(&l) - 806.96).abs() < 0.1, "{}", freq(&l));
        // Stereo detune moves only the right side.
        b = solo(SINE);
        b.osc[0].detune = 50.0;
        let (l, r) = render(&b, 60, 0.78, 44100, None);
        assert!((freq(&l) - 523.25).abs() < 0.05);
        assert!((freq(&r) - 538.58).abs() < 0.1, "{}", freq(&r));
    }

    #[test]
    fn shapes_start_and_move_as_measured() {
        // Sine starts at 0 rising; square starts high; saw starts at the bottom.
        let (l, _) = render(&solo(SINE), 60, 100.0 / 128.0, 64, None);
        assert!(l[0].abs() < 0.05 && l[3] > l[1]);
        let (l, _) = render(&solo(SQUARE), 60, 100.0 / 128.0, 64, None);
        assert!((l[1] - 0.7071).abs() < 1e-3);
        let (l, _) = render(&solo(SAW), 60, 100.0 / 128.0, 64, None);
        assert!(l[1] < -0.6);
        // Rounded saw: high 72 % of the time, low 16 %, mean +0.625.
        let mut hi = 0;
        let mut lo = 0;
        let mut sum = 0.0;
        let n = 100_000;
        for i in 0..n {
            let x = wave(ROUNDED_SAW, i as f64 / n as f64);
            hi += usize::from(x >= 0.99);
            lo += usize::from(x <= -0.99);
            sum += x;
        }
        assert!((hi as f64 / n as f64 - 0.72).abs() < 0.01);
        assert!((lo as f64 / n as f64 - 0.1667).abs() < 0.01);
        assert!((sum / n as f64 - 0.625).abs() < 0.01, "{}", sum / n as f64);
        // Noise: rms 1/√3, sides independent.
        let (l, r) = render(&solo(NOISE), 60, 100.0 / 128.0, 44100, None);
        let rms = (l.iter().map(|x| f64::from(*x / 0.70711).powi(2)).sum::<f64>() / l.len() as f64).sqrt();
        assert!((rms - 0.5774).abs() < 0.01, "{rms}");
        let corr: f64 = l.iter().zip(&r).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum::<f64>() / (l.len() as f64 * 0.5 * 0.333);
        assert!(corr.abs() < 0.05);
    }

    #[test]
    fn plain_edges_average_over_one_sample() {
        // A saw edge sample takes an intermediate value (box filter); the integral of every shape is right.
        for sh in [SINE, TRIANGLE, SQUARE, SAW, ROUNDED_SAW] {
            for &(u, dt) in &[(0.1, 0.01), (0.49, 0.03), (0.98, 0.05), (0.8, 0.07)] {
                let steps = 2000;
                let num: f64 = (0..steps).map(|i| wave(sh, (u + dt * (i as f64 + 0.5) / steps as f64).fract())).sum::<f64>() / steps as f64;
                assert!((boxed(sh, u, dt) - num).abs() < 2e-3, "shape {sh} at {u}: {} vs {num}", boxed(sh, u, dt));
            }
        }
        assert!(boxed(SAW, 0.99, 0.02) > -0.5 && boxed(SAW, 0.99, 0.02) < 0.5);
    }

    #[test]
    fn invert_negates_and_pan_is_constant_power() {
        let mut b = solo(SAW);
        let (a, _) = render(&b, 60, 0.78, 512, None);
        b.osc[0].invert = true;
        let (c, _) = render(&b, 60, 0.78, 512, None);
        assert!(a.iter().zip(&c).all(|(x, y)| (x + y).abs() < 1e-6));
        b = solo(SQUARE);
        b.osc[0].pan = -0.5;
        let (l, r) = render(&b, 60, 100.0 / 128.0, 512, None);
        assert!((peak(&l) - 0.9239).abs() < 1e-3 && (peak(&r) - 0.3827).abs() < 1e-3);
    }

    #[test]
    fn stereo_phase_offset_leads_the_right_side() {
        let mut b = solo(SINE);
        b.osc[0].phase = 0.25; // +32: R starts a quarter cycle ahead (at +1)
        let (l, r) = render(&b, 60, 100.0 / 128.0, 8, None);
        assert!(l[0].abs() < 0.05);
        assert!(r[0] > 0.68, "{}", r[0]);
    }

    #[test]
    fn mixing_and_amplitude_modulation() {
        // Defaults: 0.375 / 0.375 / 0.25 of three sines.
        let mut b = Block::default();
        for o in &mut b.osc {
            o.shape = SQUARE;
        }
        // Squares at octaves all start at +1: peak = sum of the weights.
        let (l, _) = render(&b, 60, 100.0 / 128.0, 8, None);
        assert!((l[1] / 0.70711 - 1.0).abs() < 1e-3);
        b.m2 = 1.0;
        b.m3 = 0.0;
        b.osc[1].shape = SINE;
        let (l, _) = render(&b, 60, 100.0 / 128.0, 4410, None);
        assert!((freq(&l) - 261.63).abs() < 0.2, "osc 2 alone");
        // AM with osc 3 a sine at the same pitch: 0.7071 sin x (1 − 0.7071 sin x).
        let mut b = solo(SINE);
        b.m3 = 1.0;
        b.am = true;
        b.osc[2] = Osc { shape: SINE, ..Osc::default() };
        let (l, _) = render(&b, 60, 100.0 / 128.0, 200, None);
        for (i, x) in l.iter().enumerate().skip(1) {
            let s = boxed(SINE, (i as f64 * KEY60_HZ / 44100.0).fract(), KEY60_HZ / 44100.0) * 0.70711;
            assert!((f64::from(*x) - s * (1.0 - s)).abs() < 1e-3);
        }
    }

    #[test]
    fn note_off_fades_over_10_ms() {
        let b = solo(SQUARE);
        let (l, _) = render(&b, 60, 100.0 / 128.0, 4410, Some(2205));
        let ms = |m: f64| 2205 + (m * 44.1) as usize;
        assert!((l[ms(5.0)].abs() / 0.70711 - 0.5).abs() < 0.02, "{}", l[ms(5.0)]);
        assert!((l[ms(2.0)].abs() / 0.70711 - 0.905).abs() < 0.02);
        assert!(l[ms(10.5)..].iter().all(|x| *x == 0.0));
    }

    #[test]
    fn volume_envelope_shapes_the_voice_and_ends_it() {
        let mut b = solo(SQUARE);
        let rec = os_model::Envelope { enabled: true, delay: 100, attack: 2000, hold: 100, decay: 12000, sustain: 0, release: 20000, ..Default::default() };
        b.targets[T_VOL].env = envelope::Params::new(&rec);
        let (l, _) = render(&b, 60, 100.0 / 128.0, 44100, None);
        let at = |ms: f64| l[(ms * 44.1) as usize].abs() / 0.70711;
        assert!(at(19.0) > 0.9, "peak near 18 ms: {}", at(19.0));
        assert!(at(200.0) < 0.01, "{}", at(200.0));
        let mut e = Engine::new(SR);
        e.note_on(60, 0.8, None, &b);
        let (mut l, mut r) = (vec![0.0; 44100], vec![0.0; 44100]);
        e.render(&mut l, &mut r, &b);
        assert!(e.has_voices(), "held at sustain 0");
        e.note_off(60);
        e.render(&mut l[..64], &mut r[..64], &b);
        assert!(!e.has_voices(), "a decayed voice is over once released");
    }

    #[test]
    fn pitch_envelope_and_lfo_move_the_pitch() {
        let mut b = solo(SINE);
        let rec = os_model::Envelope { enabled: true, delay: 100, attack: 100, hold: 100, decay: 100, sustain: 128, release: 100, amount: 1200, ..Default::default() };
        b.targets[T_PITCH].env = envelope::Params::new(&rec);
        let (l, _) = render(&b, 60, 0.78, 22050, None);
        assert!((freq(&l[4410..]) - 1046.5).abs() < 0.5, "{}", freq(&l[4410..]));
        assert!((lfo_hz(32950) - 0.505).abs() < 1e-9);
        let lfo = Lfo { shape: 0, amount: 100.0, hz: 0.505, ..Lfo::default() };
        assert!((lfo.value(0.495) - 100.0).abs() < 0.1, "a quarter period after the start: the top");
    }

    #[test]
    fn filter_fits_the_measured_points() {
        assert!((lp_hz(64.0) - 837.0).abs() < 1e-6 && (lp_hz(160.0) - (2727.0f64 * 8203.0).sqrt()).abs() < 1.0);
        assert!((lp_damping(128.0) - 0.949).abs() < 1e-9);
        assert!((fast_lp_a(128.0) - 0.03914).abs() < 2e-4);
        assert_eq!(fast_lp_a(256.0), 1.0);
        assert!(Block::default().filter_off());
        // LP at cutoff 128, resonance 0: −3 dB near 273 Hz (f0 / k), unity at low frequencies.
        let gain = |kind: u8, c: f64, r: f64, hz: f64| {
            let co = coefs(kind, c, r, 44100.0);
            let mut f = Filter::default();
            let n = 44100;
            let mut acc = 0.0;
            for i in 0..n {
                let y = f.tick(&co, (2.0 * PI * hz * i as f64 / 44100.0).sin());
                if i > n / 2 {
                    acc += y * y;
                }
            }
            20.0 * (acc / (n / 2 - 1) as f64 * 2.0).sqrt().log10()
        };
        assert!(gain(1, 128.0, 0.0, 50.0).abs() < 0.5);
        assert!((gain(1, 128.0, 0.0, 273.0) + 3.0).abs() < 0.6, "{}", gain(1, 128.0, 0.0, 273.0));
        // Resonance 256: +13.5 dB at f0.
        assert!((gain(1, 128.0, 256.0, 2727.0) - 13.5).abs() < 1.0, "{}", gain(1, 128.0, 256.0, 2727.0));
        // Fast LP c 128: −3 dB near 280 Hz.
        assert!((gain(0, 128.0, 0.0, 280.0) + 3.0).abs() < 0.6, "{}", gain(0, 128.0, 0.0, 280.0));
        // HP passes the top; BS notches f0.
        assert!(gain(3, 128.0, 128.0, 12000.0).abs() < 0.5);
        assert!(gain(4, 128.0, 128.0, 2727.0) < -20.0);
    }

    #[test]
    fn hq_edges_ring_less_than_aliasing() {
        // A band-limited saw stays near the ideal ramp away from the edge.
        let dt = 523.25 / 44100.0;
        assert!((hq(SAW, 0.5, dt) - 0.0).abs() < 1e-9);
        assert!(hq(SAW, 0.001, dt) > -0.9, "the edge is smoothed");
    }
}
