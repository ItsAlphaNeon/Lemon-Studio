// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's stereo delay (a recreation of FL's Fruity Delay 3 mixer effect), configured from its saved state.
//!
//! A delay line with feedback in four types (mono, true stereo, ping pong, off). The feedback path runs through a 10 Hz
//! high pass, a state-variable filter, a shaper (sample-rate and bit reduction, then a saturator or a limiter) and a
//! diffusion all-pass. Delay times follow the tempo when synced and glide or crossfade when they change; an LFO sways
//! the delay time and the filter's cutoff; a one-pole colour filter tints the echoes.
//!
// spec: plugins/fruity-delay-3.md, plugins/lemon-delay.md

use super::Effect;

/// The settings, in the saved state's order.
pub const PARAMS: usize = 26;
pub const INPUT: usize = 0;
pub const TYPE: usize = 1;
pub const SYNC: usize = 2;
pub const KEEP_PITCH: usize = 3;
pub const TIME: usize = 4;
pub const OFFSET: usize = 5;
pub const SMOOTH: usize = 6;
pub const SPREAD: usize = 7;
pub const MOD_RATE: usize = 8;
pub const MOD_TIME: usize = 9;
pub const MOD_CUTOFF: usize = 10;
pub const DIFF_LEVEL: usize = 11;
pub const DIFF_SPREAD: usize = 12;
pub const DIST_TYPE: usize = 13;
pub const FEEDBACK: usize = 14;
pub const FILTER: usize = 15;
pub const CUTOFF: usize = 16;
pub const RESONANCE: usize = 17;
pub const DRIVE: usize = 18;
pub const HARDNESS: usize = 19;
pub const SYMMETRY: usize = 20;
pub const RATE_RED: usize = 21;
pub const BITS: usize = 22;
pub const WET: usize = 23;
pub const TONE: usize = 24;
pub const DRY: usize = 25;

/// The settings' ranges (stored values are kept as they are; their normalised values are clamped).
pub const RANGES: [(i32, i32); PARAMS] = [
    (0, 6000),
    (0, 3),
    (0, 1),
    (0, 1),
    (1, 768),
    (0, 6000),
    (0, 1000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 1),
    (0, 6000),
    (0, 3),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 6000),
    (0, 60000),
    (0, 6000),
    (0, 6000),
    (-3000, 3000),
    (0, 6000),
];

/// A new instance without a state.
pub const DEFAULTS: [i32; PARAMS] =
    [6000, 0, 0, 0, 80, 3000, 500, 6000, 3000, 0, 0, 0, 3000, 0, 3000, 0, 3000, 3000, 0, 0, 0, 60000, 6000, 6000, 0, 6000];

/// Delay types.
pub const MONO: i32 = 0;
pub const TRUE_STEREO: i32 = 1;
pub const PING_PONG: i32 = 2;
pub const OFF: i32 = 3;

/// Filter modes.
pub const LOW_PASS: i32 = 0;
pub const HIGH_PASS: i32 = 1;
pub const BAND_PASS: i32 = 2;
pub const FILTER_OFF: i32 = 3;

/// The saved state: the version, then the 26 settings.
pub const VERSION: i32 = 1;
pub const STATE_LEN: usize = 108;

/// The settings that move to a new value in steps (the others take effect at once).
const SMOOTHED: [bool; PARAMS] = {
    let mut s = [false; PARAMS];
    let list = [INPUT, SPREAD, MOD_CUTOFF, DIFF_LEVEL, FEEDBACK, CUTOFF, RESONANCE, DRIVE, HARDNESS, SYMMETRY, RATE_RED, BITS, WET, TONE, DRY];
    let mut i = 0;
    while i < list.len() {
        s[list[i]] = true;
        i += 1;
    }
    s
};

/// The filter's and the colour's corner tables (Hz).
pub const LP: [f32; 5] = [270.0, 480.0, 1300.0, 7000.0, 22000.0];
pub const HP: [f32; 5] = [20.0, 90.0, 400.0, 1300.0, 4200.0];
/// The LFO's periods (s) over its speed.
pub const LFO_PERIODS: [f32; 8] = [8.872, 3.808, 1.833, 0.878, 0.398, 0.15, 0.05, 0.0];

/// Silence FL runs through a slot before the song (device buffers of 2048 frames; FL's count varies from 4 to 7).
pub const PREROLL: usize = 4 * 2048;

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

fn int_at(b: &[u8], at: usize) -> Option<i32> {
    b.get(at..at + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}

impl Settings {
    /// A saved state of version 1 (0 reads as 1). Versions 2 and up are not read.
    pub fn from_state(b: &[u8]) -> Option<Settings> {
        let version = int_at(b, 0)?;
        if version >= 2 || b.len() < STATE_LEN {
            return None;
        }
        let mut v = [0; PARAMS];
        for (k, x) in v.iter_mut().enumerate() {
            *x = int_at(b, 4 + 4 * k)?;
        }
        Some(Settings { v })
    }

    /// The saved state (108 bytes, version 1).
    pub fn to_state(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(STATE_LEN);
        b.extend_from_slice(&VERSION.to_le_bytes());
        for v in self.v {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Laws (also used by the editor's value texts)

/// Setting `k`'s normalised value for stored value `v`.
pub fn norm(k: usize, v: i32) -> f32 {
    if matches!(k, SYNC | KEEP_PITCH | DIST_TYPE) {
        return v as f32;
    }
    let (min, max) = RANGES[k];
    let r = 1.0 / f64::from(max - min);
    ((f64::from(v) - f64::from(min)) * r).clamp(0.0, 1.0) as f32
}

/// A four-position switch (type, filter mode) from its normalised value.
pub fn four_way(x: f32) -> i32 {
    (3.0 * f64::from(x) + 0.5).trunc() as i32
}

/// `(1 - u) T[i] + u T[i + 1]`, `i = Trunc(a)`.
pub fn lookup(t: &[f32], a: f32) -> f32 {
    let i = (a.trunc().max(0.0) as usize).min(t.len() - 2);
    let u = a - i as f32;
    (1.0 - u) * t[i] + u * t[i + 1]
}

/// The delay time in seconds for the Time setting's normalised value.
pub fn delay_seconds(x: f32, sync: bool, bpm: f64) -> f64 {
    if sync {
        let beats = sync_steps(x) as f32 / 192.0;
        f64::from(beats) * (1.0 / (bpm / 60.0))
    } else {
        let u = f64::from(x) * 0.94 + 0.06;
        ((u * u * u - 0.000216) * 1.0002160466660799) * 0.999 + 0.001
    }
}

/// The synced time in 1/192 beat steps (1..768).
pub fn sync_steps(x: f32) -> i32 {
    (767.0 * f64::from(x) + 0.5).trunc() as i32 + 1
}

/// The LFO's period (s) for a speed `s` (the setting's x times 0.99 in the sound).
pub fn lfo_period(s: f32) -> f32 {
    let a = s * 5.999_400_1;
    let i = (a.trunc().max(0.0) as usize).min(LFO_PERIODS.len() - 2);
    let u = a - i as f32;
    (1.0 - u) * LFO_PERIODS[i] + u * LFO_PERIODS[i + 1]
}

/// The rate reduction's hold period in frames at 44.1 kHz for an amount `t` (0..1).
pub fn hold_period(t: f32) -> f32 {
    let t = t * 127.0;
    ((((t * 0.0002) * t) * t - (t * 0.0171) * t) + t * 0.7435) + 1.0
}

/// The bit reduction's `1 - x`.
fn one_minus(x: f32) -> f32 {
    (1.0 - f64::from(x)) as f32
}

/// The bit reduction's resolution in bits (`steps = 2^bits`); 0 = off.
pub fn bits(x: f32) -> f32 {
    if x >= 1.0 {
        return 0.0;
    }
    let t = one_minus(x);
    let b = t * t;
    let q = b.sqrt().sqrt();
    (1.0 - q) * 14.0 + 2.0
}

/// `10^(db / 20)`, the division in single.
fn pow10_20(db: f32) -> f32 {
    10f64.powf(f64::from(db / 20.0)) as f32
}

// ---------------------------------------------------------------------------------------------------------------
// Building blocks

/// Flush to zero: the original runs with denormals off.
#[inline]
fn ftz(x: f32) -> f32 {
    if x.abs() < f32::MIN_POSITIVE { 0.0 } else { x }
}

/// A power-of-two ring read at fractional delays.
#[derive(Clone)]
struct Ring {
    buf: Vec<f32>,
    pos: usize,
}

impl Ring {
    fn new(len: usize) -> Ring {
        Ring { buf: vec![0.0; len], pos: 0 }
    }
    #[inline]
    fn read(&self, d: f32) -> f32 {
        let m = self.buf.len() - 1;
        let j = d.trunc() as i64 as u32;
        let f = d - j as f32;
        let i = self.pos.wrapping_sub(j as usize);
        (1.0 - f) * self.buf[i & m] + f * self.buf[i.wrapping_sub(1) & m]
    }
    #[inline]
    fn write(&mut self, v: f32) {
        let m = self.buf.len() - 1;
        self.buf[self.pos] = ftz(v);
        self.pos = (self.pos + 1) & m;
    }
    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

/// `b0 x + b1 x1 + a1 y1`.
#[derive(Clone, Copy, Debug, Default)]
struct OnePole {
    b0: f32,
    b1: f32,
    a1: f32,
    x1: f32,
    y1: f32,
}

impl OnePole {
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = ftz((self.b0 * x + self.b1 * self.x1) + self.a1 * self.y1);
        self.x1 = x;
        self.y1 = y;
        y
    }
    fn clear(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

/// The feedback high pass: 10 Hz, always designed for 44100 Hz.
fn high_pass_10hz() -> OnePole {
    let w = (1.0f64 / 44100.0) as f32 * std::f32::consts::PI;
    let t = (f64::from(w) * 10.0).tan() as f32;
    let b0 = 1.0 / (t + 1.0);
    OnePole { b0, b1: -b0, a1: -(t - 1.0) / (t + 1.0), ..OnePole::default() }
}

/// The state-variable filter's coefficients.
#[derive(Clone, Copy, Debug, Default)]
struct SvfCoefs {
    a1: f32,
    a2: f32,
    a3: f32,
    m: [f32; 3],
    off: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Svf {
    c1: f32,
    c2: f32,
}

impl Svf {
    #[inline]
    fn run(&mut self, x: f32, k: &SvfCoefs) -> f32 {
        if k.off {
            return x;
        }
        let v3 = x - self.c2;
        let v1 = self.c1 * k.a1 + k.a2 * v3;
        let v2 = (self.c1 * k.a2 + self.c2) + v3 * k.a3;
        self.c1 = ftz((v1 + v1) - self.c1);
        self.c2 = ftz((v2 + v2) - self.c2);
        (v1 * k.m[1] + x * k.m[0]) + v2 * k.m[2]
    }
}

/// The saturator's numbers for one side of zero.
#[derive(Clone, Copy, Debug, Default)]
struct Side {
    knee: f32,
    span: f32,
    curve: f32,
}

impl Side {
    fn new(level: f32, hardness: f32) -> Side {
        let knee = level * hardness;
        let r = (level - knee).max(1e-11);
        Side { knee, span: r + r, curve: 0.25 / r }
    }
    #[inline]
    fn run(&self, x: f32) -> f32 {
        if x < self.knee {
            return x;
        }
        let d = (x - self.knee).min(self.span);
        (1.0 - d * self.curve) * d + self.knee
    }
}

/// The saturator and the limiter's ceiling, from the drive, hardness and symmetry.
#[derive(Clone, Copy, Debug)]
pub struct Distortion {
    thr: f32,
    hardness: f32,
    symmetry: f32,
    gain: f32,
    ceiling: f32,
    pos: Side,
    neg: Side,
}

impl Default for Distortion {
    fn default() -> Self {
        let mut d = Distortion { thr: 0.0, hardness: 0.0, symmetry: 0.0, gain: 1.0, ceiling: 1.0, pos: Side::default(), neg: Side::default() };
        d.design();
        d
    }
}

impl Distortion {
    /// The drive setting's normalised value: the threshold, the make-up gain and the limiter's ceiling.
    pub fn set_drive(&mut self, x: f32) {
        let x = x * 0.95;
        self.thr = x * -30.0;
        self.design();
        let poly = (((((x * 2.9843) * x) * x) * x - ((x * 7.5177) * x) * x) + (x * 6.8819) * x - (x * 3.1787) * x) + 1.0;
        self.gain = (1.0 / f64::from(poly)).powf(f64::from(0.707f32)) as f32;
        self.ceiling = pow10_20(self.thr) * self.gain;
    }
    pub fn set_hardness(&mut self, x: f32) {
        self.hardness = x;
        self.design();
    }
    pub fn set_symmetry(&mut self, x: f32) {
        self.symmetry = x;
        self.design();
    }
    fn design(&mut self) {
        let top = pow10_20(self.thr);
        self.pos = Side::new((self.symmetry + 1.0) * top, self.hardness);
        self.neg = Side::new((1.0 - self.symmetry) * top, self.hardness);
    }
    /// The saturator: linear up to the knee, a parabola up to the side's level, then flat; times the gain.
    #[inline]
    pub fn saturate(&self, y: f32) -> f32 {
        let v = if y >= 0.0 { self.pos.run(y) } else { -self.neg.run(-y) };
        v * self.gain
    }
    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }
    /// The settings' distortion.
    pub fn of(v: &[i32; PARAMS]) -> Distortion {
        let mut d = Distortion::default();
        d.set_drive(norm(DRIVE, v[DRIVE]));
        d.set_hardness(norm(HARDNESS, v[HARDNESS]));
        d.set_symmetry(norm(SYMMETRY, v[SYMMETRY]));
        d
    }
    /// The static curve the editor draws: the saturator, or the limiter as a clip at its ceiling.
    pub fn curve(&self, saturate: bool, x: f32) -> f32 {
        if saturate { self.saturate(x) } else { x.clamp(-self.ceiling, self.ceiling) }
    }
}

/// Per-side shaper state.
#[derive(Clone, Copy, Debug, Default)]
struct Shaper {
    count: f32,
    held: f32,
    env: f32,
}

/// One set of the four delays: A (diffusion), then the reads (Mono: left and right; True stereo and Ping pong: the
/// echo delay C and the left and right input offsets).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Set {
    target: [i32; 4],
    live: [f64; 4],
}

// ---------------------------------------------------------------------------------------------------------------
// The delay

#[derive(Clone)]
pub struct Delay {
    rate: f64,
    bpm: f64,
    /// The stored values applied last.
    applied: [i32; PARAMS],
    /// Normalised values: where each setting is now and where it is going.
    now: [f32; PARAMS],
    target: [f32; PARAMS],
    moving: [bool; PARAMS],
    /// The smoothing's step factor.
    k: f32,
    /// Frames since creation (the smoothing and the filter's coefficients count them).
    frame: u64,

    // Derived values.
    input: f32,
    kind: i32,
    sync: bool,
    keep_pitch: bool,
    time: f32,
    pan: f32,
    w_ll: f32,
    w_lr: f32,
    w_rr: f32,
    w_rl: f32,
    glide: f64,
    straight: f32,
    crossed: f32,
    lfo_speed: f32,
    depth_setting: f32,
    filter_sway: f32,
    diff_gain: f32,
    diff_size: f32,
    saturate: bool,
    feedback: f32,
    filter: i32,
    cutoff: f32,
    resonance: f32,
    dist: Distortion,
    rate_amount: f32,
    hold: f32,
    steps: f32,
    wet: f32,
    colour: f32,
    colour_hz: Option<(f32, bool)>,
    dry: f32,

    // Delay times.
    sets: [Set; 2],
    gains: [f32; 2],
    current: usize,
    fading: bool,
    fade_pos: u32,
    fade_len: u32,
    marked: bool,

    // LFO.
    phase: f32,
    lfo_step: f32,
    lfo_scale: f32,
    depth: f32,
    depth_c: f32,
    lfo: f32,

    // Lines and states.
    lines: [Ring; 2],
    inputs: [Ring; 2],
    /// All-pass rings per side and set.
    allpass: [[Ring; 2]; 2],
    hp: [OnePole; 2],
    svf: [Svf; 2],
    svf_k: SvfCoefs,
    tone: [OnePole; 2],
    shaper: [Shaper; 2],
    attack: f32,
    release: f32,
    /// The mixer slot has run FL's start (pre-roll and two resets).
    started: bool,
}

fn line_len(rate: f64) -> usize {
    if rate > 126976.0 {
        1 << 22
    } else if rate > 61440.0 {
        1 << 21
    } else {
        1 << 20
    }
}

impl Delay {
    /// A new instance with the defaults at `sample_rate` (FL then sets the rate, which resets it).
    pub fn new(sample_rate: u32) -> Delay {
        let rate = f64::from(sample_rate.max(1));
        let n = line_len(rate);
        let ring = || Ring::new(n);
        let ap = || Ring::new(32768);
        let mut d = Delay {
            rate,
            bpm: 120.0,
            applied: DEFAULTS,
            now: [0.0; PARAMS],
            target: [0.0; PARAMS],
            moving: [false; PARAMS],
            k: 0.0,
            frame: 0,
            input: 0.0,
            kind: 0,
            sync: false,
            keep_pitch: false,
            time: 0.0,
            pan: 0.5,
            w_ll: 1.0,
            w_lr: 0.0,
            w_rr: 1.0,
            w_rl: 0.0,
            glide: 1.0,
            straight: 1.0,
            crossed: 0.0,
            lfo_speed: 0.0,
            depth_setting: 0.0,
            filter_sway: 0.0,
            diff_gain: 0.0,
            diff_size: 0.0,
            saturate: false,
            feedback: 0.0,
            filter: 0,
            cutoff: 0.0,
            resonance: 0.0,
            dist: Distortion::default(),
            rate_amount: 0.0,
            hold: 1.0,
            steps: 0.0,
            wet: 1.0,
            colour: 0.5,
            colour_hz: None,
            dry: 1.0,
            sets: [Set::default(); 2],
            gains: [1.0, 0.0],
            current: 0,
            fading: false,
            fade_pos: 0,
            fade_len: 1,
            marked: false,
            phase: 0.0,
            lfo_step: 0.0,
            lfo_scale: 1.0,
            depth: 0.0,
            depth_c: 0.0,
            lfo: 0.5,
            lines: [ring(), ring()],
            inputs: [ring(), ring()],
            allpass: [[ap(), ap()], [ap(), ap()]],
            hp: [high_pass_10hz(); 2],
            svf: [Svf::default(); 2],
            svf_k: SvfCoefs::default(),
            tone: [OnePole::default(); 2],
            shaper: [Shaper::default(); 2],
            attack: 1.0 - 0.1f64.powf(1.0 / 441.0) as f32,
            release: 1.0 - 0.1f64.powf(1.0 / 4410.0) as f32,
            started: false,
        };
        d.rate_changed();
        for k in 0..PARAMS {
            let x = norm(k, DEFAULTS[k]);
            d.now[k] = x;
            d.target[k] = x;
            d.derive(k);
        }
        let t = d.targets(&d.sets[0].target);
        d.sets = [Set { target: t, live: t.map(f64::from) }; 2];
        d.reset();
        d
    }

    /// FL's start of a render: created with the defaults, the sample rate set (a reset), the state applied, the tempo.
    pub fn with_settings(sample_rate: u32, s: &Settings, bpm: f64) -> Delay {
        let mut d = Delay::new(sample_rate);
        d.set(s);
        d.set_tempo(bpm);
        d
    }

    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<Delay> {
        Some(Delay::with_settings(sample_rate, &Settings::from_state(state)?, 120.0))
    }

    /// The stored values applied last.
    pub fn settings(&self) -> Settings {
        Settings { v: self.applied }
    }

    /// Applies the settings that differ from the ones applied last, in index order (each as a change of that setting).
    pub fn set(&mut self, s: &Settings) {
        for k in 0..PARAMS {
            if s.v[k] != self.applied[k] {
                self.apply(k, s.v[k]);
            }
        }
    }

    /// Applies setting `k` (smoothed settings start moving towards it).
    pub fn apply(&mut self, k: usize, v: i32) {
        self.applied[k] = v;
        let x = norm(k, v);
        if SMOOTHED[k] {
            self.target[k] = x;
            self.moving[k] = self.now[k] != x;
        } else {
            self.now[k] = x;
            self.target[k] = x;
            self.derive(k);
        }
    }

    /// The host's tempo (read once per block): a change marks the delays to be recomputed.
    pub fn set_tempo(&mut self, bpm: f64) {
        if bpm > 0.0 && bpm != self.bpm {
            self.bpm = bpm;
            self.marked = true;
        }
    }

    /// A new sample rate: new lines, a reset, and every rate-dependent value made again.
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        let rate = f64::from(sample_rate.max(1));
        if rate == self.rate {
            return;
        }
        self.rate = rate;
        let n = line_len(rate);
        for r in self.lines.iter_mut().chain(self.inputs.iter_mut()) {
            *r = Ring::new(n);
        }
        self.rate_changed();
        if let Some((hz, high)) = self.colour_hz {
            self.design_colour(hz.min((0.49 * rate) as f32), high);
        }
        self.reset();
    }

    fn rate_changed(&mut self) {
        let rate = self.rate;
        self.k = (1.0 - 0.01f64.powf(1.0 / (rate * 0.005))) as f32;
        self.depth_c = (1.0 - 0.01f64.powf(1.0 / (0.1 * rate))) as f32;
        self.fade_len = ((0.1 * rate).trunc() as u32).max(1);
        for k in [MOD_RATE, SMOOTH, RATE_RED] {
            self.derive(k);
        }
        self.make_filter();
    }

    /// The derived value(s) of setting `k` from its current normalised value.
    fn derive(&mut self, k: usize) {
        let x = self.now[k];
        match k {
            INPUT => self.input = x,
            TYPE => {
                let t = four_way(x);
                if t != self.kind {
                    self.kind = t;
                    self.reset();
                }
                self.marked = true;
            }
            SYNC => {
                self.sync = x >= 0.5;
                self.marked = true;
            }
            KEEP_PITCH => {
                let on = x >= 0.5;
                if self.keep_pitch && !on {
                    self.marked = true;
                }
                self.keep_pitch = on;
            }
            TIME => {
                self.time = x;
                self.marked = true;
            }
            OFFSET => {
                self.pan = x;
                let t = (x + x) - 1.0;
                if t >= 0.0 {
                    self.w_ll = 1.0 - t;
                    self.w_lr = (1.0 - self.w_ll) * 0.5;
                    self.w_rr = 1.0 - self.w_lr;
                    self.w_rl = 0.0;
                } else {
                    self.w_rr = t + 1.0;
                    self.w_rl = (1.0 - self.w_rr) * 0.5;
                    self.w_ll = 1.0 - self.w_rl;
                    self.w_lr = 0.0;
                }
                self.marked = true;
            }
            SMOOTH => self.glide = if x == 0.0 { 1.0 } else { 1.0 - 0.01f64.powf(1.0 / (f64::from(x) * self.rate)) },
            SPREAD => {
                self.straight = x * 0.5 + 0.5;
                self.crossed = 1.0 - self.straight;
            }
            MOD_RATE => {
                self.lfo_speed = x * 0.99f32;
                let period = lfo_period(self.lfo_speed);
                self.lfo_step = 1.0 / (self.rate as f32 * period);
                // Over the slowest period as stored (single): 8.872 in double is a step off at some speeds.
                self.lfo_scale = (period / LFO_PERIODS[0]).sqrt();
            }
            MOD_TIME => self.depth_setting = (f64::from(x) * f64::from(x)) as f32 * 0.1,
            MOD_CUTOFF => self.filter_sway = x * 0.5,
            DIFF_LEVEL => self.diff_gain = x,
            DIFF_SPREAD => {
                self.diff_size = x;
                self.marked = true;
            }
            DIST_TYPE => self.saturate = (x + 0.5).trunc() != 0.0,
            FEEDBACK => self.feedback = x * 1.25,
            FILTER => self.filter = four_way(x),
            CUTOFF => self.cutoff = x,
            RESONANCE => self.resonance = x,
            DRIVE => self.dist.set_drive(x),
            HARDNESS => self.dist.set_hardness(x),
            SYMMETRY => self.dist.set_symmetry(x),
            RATE_RED => {
                let t = one_minus(x);
                self.rate_amount = t * (t * t);
                self.hold = (hold_period(self.rate_amount) * (self.rate / 44100.0) as f32).max(1.0);
            }
            BITS => {
                self.steps = if x >= 1.0 {
                    0.0
                } else {
                    let t = one_minus(x);
                    let q = (t * t).sqrt().sqrt();
                    2f64.powf(f64::from((1.0 - q) * 14.0 + 2.0)) as f32
                };
            }
            WET => self.wet = x,
            TONE => self.set_colour(x),
            DRY => self.dry = x,
            _ => {}
        }
    }

    fn set_colour(&mut self, c: f32) {
        self.colour = c;
        if c < 0.5 {
            self.design_colour(lookup(&LP, (c + c) * 3.999_900_1), false);
        } else if c > 0.5 {
            self.design_colour(lookup(&HP, ((c - 0.5) + (c - 0.5)) * 3.999_900_1), true);
        } else {
            self.colour_hz = None;
            for t in &mut self.tone {
                (t.b0, t.b1, t.a1) = (1.0, 0.0, 0.0);
            }
        }
    }

    fn design_colour(&mut self, hz: f32, high: bool) {
        self.colour_hz = Some((hz, high));
        let w = (1.0 / self.rate) as f32 * std::f32::consts::PI;
        let t = (f64::from(hz) * f64::from(w)).tan() as f32;
        let (b0, b1) = if high {
            let b0 = 1.0 / (t + 1.0);
            (b0, -b0)
        } else {
            let b = t / (t + 1.0);
            (b, b)
        };
        for p in &mut self.tone {
            (p.b0, p.b1, p.a1) = (b0, b1, -(t - 1.0) / (t + 1.0));
        }
    }

    /// The filter's coefficients from the cutoff, its LFO sway and the resonance.
    fn make_filter(&mut self) {
        let at = (self.filter_sway * self.lfo + self.cutoff).clamp(0.0, 1.0);
        let q = self.resonance.clamp(0.0, 1.0) * 0.5;
        let q = q * q;
        let (hz, k, m) = match self.filter {
            LOW_PASS => {
                let k = 1.0 / (q * 4.0 + 0.707);
                (lookup(&LP, at * 3.999), k, [0.0, 0.0, 1.0])
            }
            HIGH_PASS => {
                let k = 1.0 / (q * 4.0 + 0.707);
                (lookup(&HP, at * 3.999), k, [1.0, -k, -1.0])
            }
            BAND_PASS => {
                let k = 1.0 / (q * 4.6 + 0.107);
                (lookup(&LP, at * 3.6), k, [0.0, k, 0.0])
            }
            _ => {
                self.svf_k.off = true;
                return;
            }
        };
        let hz = hz.min((0.49 * self.rate) as f32);
        let w = (std::f64::consts::PI / self.rate) as f32;
        // The product in single, the tangent in double.
        let g = f64::from(hz * w).tan() as f32;
        let a1 = 1.0 / ((g + k) * g + 1.0);
        let a2 = a1 * g;
        self.svf_k = SvfCoefs { a1, a2, a3: a2 * g, m, off: false };
    }

    /// The four delays' integer targets for the current settings (Off makes only A and keeps the rest).
    fn targets(&self, old: &[i32; 4]) -> [i32; 4] {
        let sec = delay_seconds(self.time, self.sync, self.bpm);
        let d = (sec * self.rate).trunc() as i32;
        let df = d as f32;
        let rate = self.rate as f32;
        let scale = (self.rate / 44100.0) as f32;
        let a = ((df / (rate * 0.015)).min(1.0) * (scale * (self.diff_size * 590.0 + 65.0))).max(1.0).trunc() as i32;
        let mut t = *old;
        t[0] = a;
        let w = ((self.pan - 0.5).abs() + (self.pan - 0.5).abs()) * 0.5;
        match self.kind {
            MONO => {
                let ra = (df - a as f32).max(1.0).trunc() as i32;
                let rb = ((df - (1.0 - w) * df) + ra as f32).trunc() as i32;
                (t[1], t[2]) = if self.pan < 0.5 { (rb, ra) } else { (ra, rb) };
            }
            TRUE_STEREO | PING_PONG => {
                t[1] = (df - a as f32).max(1.0).trunc() as i32;
                let o = ((w * df) * w).max(1.0).trunc() as i32;
                (t[2], t[3]) = if self.pan >= 0.5 { (o, 1) } else { (1, o) };
            }
            _ => {}
        }
        t
    }

    /// Empties the lines and filters; the live delays take their targets, then the targets are made again.
    pub fn reset(&mut self) {
        for r in self.lines.iter_mut().chain(self.inputs.iter_mut()) {
            r.clear();
        }
        for side in &mut self.allpass {
            side.iter_mut().for_each(Ring::clear);
        }
        self.svf = [Svf::default(); 2];
        self.hp.iter_mut().for_each(OnePole::clear);
        for s in &mut self.shaper {
            s.held = 0.0;
        }
        for s in &mut self.sets {
            s.live = s.target.map(f64::from);
        }
        for i in 0..2 {
            self.sets[i].target = self.targets(&self.sets[i].target);
        }
    }

    /// One smoothing step of every moving setting.
    fn smooth(&mut self) {
        for k in 0..PARAMS {
            if !self.moving[k] {
                continue;
            }
            let mut n = self.now[k] + (self.target[k] - self.now[k]) * self.k;
            if (n - self.target[k]).abs() < 0.0001 {
                n = self.target[k];
                self.moving[k] = false;
            }
            self.now[k] = n;
            self.derive(k);
        }
    }

    /// The delay times before the frame: glide (Keep pitch off) or crossfade (on).
    fn update_times(&mut self) {
        if !self.keep_pitch {
            self.gains = [1.0, 0.0];
            if self.marked {
                self.marked = false;
                self.sets[0].target = self.targets(&self.sets[0].target);
            }
            let g = self.glide;
            let s = &mut self.sets[0];
            for i in 0..4 {
                let (t, l) = (f64::from(s.target[i]), s.live[i]);
                if l != t {
                    s.live[i] = if (t - l).abs() < 0.001 { t } else { (t - l) * g + l };
                }
            }
        } else if self.marked && !self.fading {
            self.marked = false;
            let o = 1 - self.current;
            let t = self.targets(&self.sets[o].target);
            self.sets[o] = Set { target: t, live: t.map(f64::from) };
            self.fading = true;
            self.fade_pos = 0;
        }
    }

    /// After the frame: the crossfade moves on.
    fn advance_fade(&mut self) {
        if !self.keep_pitch || !self.fading {
            return;
        }
        let t = self.fade_pos as f32 * (1.0 / self.fade_len as f32);
        let o = 1 - self.current;
        self.gains[self.current] = 1.0 - t;
        self.gains[o] = t;
        self.fade_pos += 1;
        if self.fade_pos >= self.fade_len {
            self.current = o;
            self.fading = false;
            self.fade_pos = 0;
        }
    }

    /// Both sets' reads of `ring` at their delay `i` (plus a sway in frames), added by their gains.
    #[inline]
    fn tap(&self, ring: &Ring, i: usize, sway: f32, mono: bool) -> f32 {
        let d = |s: &Set| if mono { s.live[i] as f32 + sway } else { (f64::from(sway) + s.live[i]) as f32 };
        ring.read(d(&self.sets[0])) * self.gains[0] + ring.read(d(&self.sets[1])) * self.gains[1]
    }

    /// The diffusion all-passes of `side`, one per set, added by the sets' gains.
    #[inline]
    fn diffuse(&mut self, side: usize, x: f32) -> f32 {
        let h = self.diff_gain * 0.499;
        let mut out = [0.0f32; 2];
        for (s, o) in out.iter_mut().enumerate() {
            let ring = &mut self.allpass[side][s];
            let z = ring.read(self.sets[s].live[0] as f32);
            ring.write(x - z * h);
            *o = (1.0 - h * h) * z + h * x;
        }
        out[1] * self.gains[1] + out[0] * self.gains[0]
    }

    /// Rate reduction, bit reduction, then the saturator or the limiter.
    #[inline]
    fn shape(&mut self, side: usize, x: f32) -> f32 {
        let s = &mut self.shaper[side];
        let mut y = x;
        if self.rate_amount > 0.0 {
            s.count += 1.0;
            if s.count >= self.hold {
                s.held = y;
                if s.count < 0.0 || self.hold <= 0.0 {
                    s.count = 0.0;
                } else {
                    let r = s.count / self.hold;
                    s.count = (r - r.trunc()) * s.count;
                }
            } else {
                y = s.held;
            }
        }
        if self.steps > 0.0 {
            y = (self.steps * y).trunc() / self.steps;
        }
        if self.saturate {
            self.dist.saturate(y)
        } else {
            let mut v = y / self.dist.ceiling;
            let a = v.abs();
            let d = a - s.env;
            s.env = ftz(s.env + d * if a >= s.env { self.attack } else { self.release });
            if s.env > 1.0 {
                v /= s.env;
            }
            self.dist.ceiling * v
        }
    }

    #[inline]
    fn out_mix(&self, yl: f32, yr: f32, il: f32, ir: f32) -> (f32, f32) {
        let l = (self.crossed * yr + self.straight * yl) * self.wet + (il * self.dry) * 1.0;
        let r = (self.crossed * yl + self.straight * yr) * self.wet + (ir * self.dry) * 1.0;
        (l, r)
    }

    /// One stereo frame.
    #[inline]
    pub fn frame(&mut self, il: f32, ir: f32) -> (f32, f32) {
        let il = if il.abs() <= 9.999_999_8e-14 { 0.0 } else { il };
        let ir = if ir.abs() <= 9.999_999_8e-14 { 0.0 } else { ir };
        if self.frame % 8 == 0 {
            self.smooth();
        }
        self.update_times();
        let old = self.phase;
        let p = old + self.lfo_step;
        self.phase = p - p.trunc();
        // FL's single-precision sine: on Windows `f32::sin` is the C runtime's `sinf`, which gives FL's values.
        self.lfo = ((old * 6.283_185_5).sin() + 1.0) * 0.5;
        if self.kind != OFF {
            let half = 0.5 * self.rate as f32;
            self.depth = ftz(((self.depth_setting * half) * self.lfo_scale - self.depth) * self.depth_c + self.depth);
        }
        if self.frame % 4 == 0 {
            self.make_filter();
        }
        let sway = (self.lfo + 0.5) * self.depth;
        let out = match self.kind {
            MONO => {
                let m = ((il + ir) * 0.5) * self.input;
                let e1 = self.tap(&self.lines[0], 1, sway, true);
                let e2 = self.tap(&self.lines[0], 2, sway, true);
                let feed = if self.pan < 0.5 { e2 } else { e1 };
                let x = self.hp[0].run(self.feedback * feed) + m;
                let x = self.shape(0, x);
                let x = self.svf[0].run(x, &self.svf_k);
                let x = self.diffuse(0, x);
                self.lines[0].write(x);
                let yl = self.tone[0].run(e1);
                let yr = self.tone[1].run(e2);
                self.out_mix(yl, yr, il, ir)
            }
            TRUE_STEREO => {
                let e = [self.tap(&self.lines[0], 1, sway, false), self.tap(&self.lines[1], 1, sway, false)];
                let inp = [il, ir];
                let mut x = [0.0f32; 2];
                for s in 0..2 {
                    self.inputs[s].write(inp[s]);
                    let n = self.tap(&self.inputs[s], 2 + s, 0.0, false);
                    let v = n * self.input + self.hp[s].run(e[s] * self.feedback);
                    let v = self.svf[s].run(v, &self.svf_k);
                    let v = self.shape(s, v);
                    x[s] = self.diffuse(s, v);
                }
                self.lines[0].write(x[0]);
                self.lines[1].write(x[1]);
                let yl = self.tone[0].run(e[0]);
                let yr = self.tone[1].run(e[1]);
                self.out_mix(yl, yr, il, ir)
            }
            PING_PONG => {
                let e = [self.tap(&self.lines[0], 1, sway, false), self.tap(&self.lines[1], 1, sway, false)];
                let to = [ir * self.w_rl + il * self.w_ll, ir * self.w_rr + il * self.w_lr];
                let mut x = [0.0f32; 2];
                for s in 0..2 {
                    let v = to[s] * self.input + self.hp[s].run(e[s] * self.feedback);
                    let v = self.svf[s].run(v, &self.svf_k);
                    let v = self.shape(s, v);
                    x[s] = self.diffuse(s, v);
                }
                // Each side feeds the other's line.
                self.lines[1].write(x[0]);
                self.lines[0].write(x[1]);
                let y = [self.tone[0].run(e[0]), self.tone[1].run(e[1])];
                let mut o = [self.straight * y[0] + self.crossed * y[1], self.straight * y[1] + self.crossed * y[0]];
                if self.saturate {
                    for (s, o) in o.iter_mut().enumerate() {
                        let sh = &mut self.shaper[s];
                        let a = o.abs();
                        let d = a - sh.env;
                        sh.env = ftz(sh.env + d * if a >= sh.env { self.attack } else { self.release });
                        if sh.env > 1.0 {
                            *o /= sh.env;
                        }
                    }
                }
                ((il * self.dry) * 1.0 + o[0] * self.wet, (ir * self.dry) * 1.0 + o[1] * self.wet)
            }
            _ => {
                let mut y = [0.0f32; 2];
                for (s, v) in [il, ir].into_iter().enumerate() {
                    let f = self.svf[s].run(v, &self.svf_k);
                    let sh = self.shape(s, f);
                    y[s] = self.tone[s].run(sh);
                }
                ((self.crossed * y[1] + self.straight * y[0]) * self.wet + self.dry * il, (self.crossed * y[0] + self.straight * y[1]) * self.wet + self.dry * ir)
            }
        };
        self.advance_fade();
        self.frame += 1;
        out
    }

    /// Processes a stereo block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            (*l, *r) = self.frame(*l, *r);
        }
    }

    /// Runs `frames` of silence through the delay.
    pub fn preroll(&mut self, frames: usize) {
        for _ in 0..frames {
            self.frame(0.0, 0.0);
        }
    }

    /// What FL runs between loading a slot and the song: silence, then two resets.
    pub fn start(&mut self) {
        self.preroll(PREROLL);
        self.reset();
        self.reset();
        self.started = true;
    }

    /// The live delay of read `i` in the current set (frames; tests and the editor).
    pub fn live_delay(&self, i: usize) -> f64 {
        self.sets[self.current].live[i]
    }

    /// The two sets' gains (tests).
    pub fn set_gains(&self) -> [f32; 2] {
        self.gains
    }
}

impl Effect for Delay {
    fn process(&mut self, buf: &mut [f32]) {
        if !self.started {
            self.start();
        }
        for f in buf.chunks_exact_mut(2) {
            (f[0], f[1]) = self.frame(f[0], f[1]);
        }
    }

    fn reset(&mut self) {
        Delay::reset(self);
    }

    fn set_tempo(&mut self, bpm: f64) {
        Delay::set_tempo(self, bpm);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(pairs: &[(usize, i32)]) -> Settings {
        let mut s = Settings::default();
        for &(k, v) in pairs {
            s.v[k] = v;
        }
        s
    }

    /// A delay at 44.1 kHz past FL's start sequence.
    fn started(s: &Settings, bpm: f64) -> Delay {
        let mut d = Delay::with_settings(44100, s, bpm);
        d.start();
        d
    }

    fn impulse(d: &mut Delay, n: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        l[0] = 1.0;
        r[0] = 1.0;
        d.process(&mut l, &mut r);
        (l, r)
    }

    fn first_peak(v: &[f32], from: usize) -> usize {
        from + v[from..].iter().position(|x| x.abs() > 1e-3).expect("an echo")
    }

    #[test]
    fn states_round_trip_and_newer_versions_are_not_read() {
        let s = with(&[(TYPE, 2), (TIME, 192), (TONE, -1200), (RATE_RED, 0)]);
        let b = s.to_state();
        assert_eq!(b.len(), STATE_LEN);
        assert_eq!(Settings::from_state(&b), Some(s));
        let mut v0 = b.clone();
        v0[0] = 0;
        assert_eq!(Settings::from_state(&v0), Some(s));
        let mut v2 = b;
        v2[0] = 2;
        assert_eq!(Settings::from_state(&v2), None);
        assert_eq!(Settings::from_state(&[1, 0, 0, 0]), None);
    }

    #[test]
    fn normalised_values_and_laws() {
        assert_eq!(norm(TIME, 1), 0.0);
        assert_eq!(norm(TIME, 768), 1.0);
        assert_eq!(norm(TONE, 0), 0.5);
        assert_eq!(norm(TIME, 0), 0.0, "out of range: clamped");
        assert_eq!(norm(SYNC, 1), 1.0);
        assert_eq!(four_way(norm(TYPE, 2)), 2);
        assert!((delay_seconds(0.0, false, 120.0) - 0.001).abs() < 1e-12);
        assert!((delay_seconds(1.0, false, 120.0) - 1.0).abs() < 1e-9);
        assert!((delay_seconds(0.5, false, 120.0) - 0.1495).abs() < 1e-3);
        // Synced: v / 192 beats; 192 = one beat = 0.5 s at 120 BPM.
        assert_eq!(sync_steps(norm(TIME, 192)), 192);
        assert!((delay_seconds(norm(TIME, 192), true, 120.0) - 0.5).abs() < 1e-9);
        assert_eq!(bits(1.0), 0.0);
        assert!((bits(0.0) - 2.0).abs() < 1e-6);
        assert!((lfo_period(0.0) - 8.872).abs() < 1e-6);
    }

    #[test]
    fn echoes_arrive_after_the_delay_time() {
        // True stereo, 100 ms, no feedback, dry off.
        let x = (0..768).find(|&v| delay_seconds(norm(TIME, v), false, 120.0) >= 0.1).unwrap();
        let s = with(&[(TYPE, TRUE_STEREO), (TIME, x), (FEEDBACK, 0), (DRY, 0)]);
        let mut d = started(&s, 120.0);
        let (l, r) = impulse(&mut d, 10000);
        let want = (delay_seconds(norm(TIME, x), false, 120.0) * 44100.0).trunc() as usize;
        let at = first_peak(&l, 0);
        assert!((at as i64 - want as i64).abs() <= 2, "{at} vs {want}");
        assert_eq!(first_peak(&r, 0), at);
        // No feedback: one echo only.
        assert!(l[at + 200..].iter().all(|v| v.abs() < 1e-3));
    }

    #[test]
    fn feedback_repeats_and_decays() {
        let s = with(&[(TYPE, TRUE_STEREO), (TIME, 100), (FEEDBACK, 3000), (DRY, 0), (FILTER, FILTER_OFF)]);
        let mut d = started(&s, 120.0);
        let (l, _) = impulse(&mut d, 44100);
        let e1 = first_peak(&l, 0);
        let e2 = first_peak(&l, e1 + 50);
        let p1 = l[e1 - 2..e1 + 3].iter().fold(0f32, |a, v| a.max(v.abs()));
        let p2 = l[e2 - 2..e2 + 3].iter().fold(0f32, |a, v| a.max(v.abs()));
        assert!(p2 < p1 && p2 > p1 * 0.3, "{p1} {p2}");
    }

    #[test]
    fn ping_pong_alternates_sides() {
        let s = with(&[(TYPE, PING_PONG), (TIME, 100), (FEEDBACK, 3000), (DRY, 0), (OFFSET, 0), (FILTER, FILTER_OFF)]);
        let mut d = started(&s, 120.0);
        let mut l = vec![0.0f32; 44100];
        let mut r = vec![0.0f32; 44100];
        l[0] = 1.0;
        d.process(&mut l, &mut r);
        // Pan -100 %: the left input goes into the left side, whose loop writes the right line first.
        let a = first_peak(&r, 0);
        let b = first_peak(&l, 0);
        assert!(a < b, "the first echo is on the right ({a} vs {b})");
    }

    #[test]
    fn the_first_reset_keeps_the_delays_the_second_takes_new_ones() {
        let mut d = Delay::with_settings(44100, &with(&[(TYPE, TRUE_STEREO), (KEEP_PITCH, 1)]), 120.0);
        d.start();
        let before = d.live_delay(1);
        d.apply(TIME, 400);
        d.reset();
        assert_eq!(d.live_delay(1), before);
        d.reset();
        assert!(d.live_delay(1) > before * 10.0);
    }

    #[test]
    fn keep_pitch_crossfades_and_leaves_the_old_set_at_one_nth() {
        let mut d = started(&with(&[(TYPE, TRUE_STEREO), (KEEP_PITCH, 1)]), 120.0);
        let was = d.current;
        d.apply(TIME, 300);
        d.preroll(100);
        let g = d.set_gains();
        assert!(g[was] < 1.0 && g[1 - was] > 0.0, "fading: {g:?}");
        d.preroll(4410);
        let g = d.set_gains();
        assert_eq!(d.current, 1 - was);
        assert!((g[was] - 1.0 / 4410.0).abs() < 1e-7 && (g[1 - was] - (1.0 - 1.0 / 4410.0)).abs() < 1e-6, "{g:?}");
    }

    #[test]
    fn glide_moves_the_delay_without_keep_pitch() {
        let mut d = started(&with(&[(TYPE, TRUE_STEREO)]), 120.0);
        let a = d.live_delay(1);
        d.apply(TIME, 400);
        d.preroll(100);
        let b = d.live_delay(1);
        d.preroll(4 * 44100);
        let c = d.live_delay(1);
        assert!(a < b && b < c, "{a} {b} {c}");
        assert_eq!(c, f64::from(d.sets[0].target[1]));
    }

    #[test]
    fn a_tempo_change_moves_synced_echoes() {
        let mut d = started(&with(&[(TYPE, TRUE_STEREO), (SYNC, 1), (TIME, 192), (SMOOTH, 0)]), 120.0);
        let at120 = d.live_delay(1);
        d.set_tempo(60.0);
        d.preroll(1);
        assert!((d.live_delay(1) - (at120 + 22050.0)).abs() < 2.0, "{} {}", at120, d.live_delay(1));
    }

    #[test]
    fn smoothed_settings_move_in_steps() {
        let mut d = started(&Settings::default(), 120.0);
        d.apply(WET, 0);
        d.preroll(8);
        assert!(d.wet < 1.0 && d.wet > 0.9);
        d.preroll(44100);
        assert_eq!(d.wet, 0.0);
    }

    #[test]
    fn off_passes_the_dry_signal_and_shapes_the_wet() {
        let s = with(&[(TYPE, OFF), (FILTER, FILTER_OFF), (WET, 0)]);
        let mut d = started(&s, 120.0);
        let mut l = vec![0.25f32; 64];
        let mut r = vec![-0.5f32; 64];
        d.process(&mut l, &mut r);
        assert_eq!((l[63], r[63]), (0.25, -0.5));
    }

    #[test]
    fn the_saturator_bends_to_its_level_and_stays_flat() {
        let mut d = started(&with(&[(DIST_TYPE, 1), (HARDNESS, 3000)]), 120.0);
        d.preroll(44100);
        assert_eq!(d.shape(0, 0.25), 0.25);
        assert!((d.shape(0, 5.0) - 1.0).abs() < 1e-6);
        assert!((d.shape(0, -5.0) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn every_type_stays_finite_at_extreme_settings() {
        for kind in 0..4 {
            let s = with(&[(TYPE, kind), (TIME, 2), (FEEDBACK, 6000), (RESONANCE, 6000), (DRIVE, 6000), (RATE_RED, 0), (BITS, 0), (MOD_TIME, 6000), (MOD_RATE, 6000), (DIFF_LEVEL, 6000), (TONE, 3000)]);
            let mut d = started(&s, 120.0);
            let mut l: Vec<f32> = (0..20000).map(|i| ((i as f32) * 0.05).sin()).collect();
            let mut r = l.clone();
            d.process(&mut l, &mut r);
            assert!(l.iter().chain(&r).all(|v| v.is_finite() && v.abs() < 100.0), "type {kind}");
        }
    }
}
