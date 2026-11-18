// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The voices: a SoundFont 2 synthesizer (sample playback with loops, pitch, the volume and modulation envelopes,
//! the two LFOs, the resonant low-pass filter, modulators), with the interpolation and level laws of the chosen
//! playback mode.
//!
// spec: plugins/lemon-soundfont.md §2, §3

use std::f64::consts::PI;
use std::sync::{Arc, OnceLock};

use crate::profile::{CutoffSetting, Mode};
use crate::sf2::{g, Font, Modulator, VoiceSpec};

const MAX_VOICES: usize = 128;

/// What the voices play: a font and the chosen preset in it.
pub struct Patch {
    pub font: Arc<Font>,
    pub preset: Option<usize>,
    pub overrides: Overrides,
}

/// The player's own controls, as stored (`None` = off: the font's value plays).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Overrides {
    /// Volume envelope: attack in ms (linear), decay and release in 4 ms units per 96 dB (0 = at once), sustain
    /// 0..=127.
    pub attack: Option<i32>,
    pub decay: Option<i32>,
    pub sustain: Option<i32>,
    pub release: Option<i32>,
    /// Vibrato LFO: delay in 4 ms units, depth −127..=127, speed 0..=127.
    pub lfo_delay: Option<i32>,
    pub lfo_amount: Option<i32>,
    pub lfo_speed: Option<i32>,
    /// Filter cutoff 0..=127.
    pub cutoff: Option<i32>,
    /// The modulation wheel, 0..=127.
    pub mod_wheel: i32,
    /// The built-in reverb and chorus: on, and their levels (0..=2560, 128 = the default).
    pub reverb_on: bool,
    pub chorus_on: bool,
    pub reverb_level: i32,
    pub chorus_level: i32,
}

fn timecents_to_secs(tc: i32) -> f64 {
    if tc <= -12000 { 0.0 } else { 2f64.powf(f64::from(tc) / 1200.0) }
}

fn cents_to_hz(c: f64) -> f64 {
    8.176 * 2f64.powf(c / 1200.0)
}

/// The modern mode's kernel: a Kaiser-windowed sinc, half-width 3.542 source frames, β 11.607 (a table at 1/4096
/// frame, read with linear interpolation).
struct Kaiser {
    table: Vec<f32>,
}

const KAISER_HALF: f64 = 3.542;
const KAISER_BETA: f64 = 11.607;
const KAISER_RES: f64 = 4096.0;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let y = x * x / 4.0;
    for k in 1..60 {
        term *= y / (k as f64 * k as f64);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 { 1.0 } else { (PI * x).sin() / (PI * x) }
}

impl Kaiser {
    fn get() -> &'static Kaiser {
        static K: OnceLock<Kaiser> = OnceLock::new();
        K.get_or_init(|| {
            let n = (KAISER_HALF * KAISER_RES) as usize + 2;
            let i0b = bessel_i0(KAISER_BETA);
            let table = (0..n)
                .map(|i| {
                    let x = i as f64 / KAISER_RES;
                    if x >= KAISER_HALF {
                        return 0.0;
                    }
                    let u = 1.0 - (x / KAISER_HALF).powi(2);
                    (sinc(x) * bessel_i0(KAISER_BETA * u.max(0.0).sqrt()) / i0b) as f32
                })
                .collect();
            Kaiser { table }
        })
    }

    fn at(&self, x: f64) -> f32 {
        let p = x.abs() * KAISER_RES;
        let i = p as usize;
        if i + 1 >= self.table.len() {
            return 0.0;
        }
        let f = (p - i as f64) as f32;
        self.table[i] + (self.table[i + 1] - self.table[i]) * f
    }
}

/// The classic mode's pitch-up kernel: a sinc at the output's Nyquist under a Hann window 32 source frames each side.
const HANN_HALF: f64 = 32.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    Done,
}

/// The volume envelope: a linear attack, then decay and release that fall in decibels (96 dB per the stage's time).
#[derive(Clone, Debug)]
struct VolEnv {
    stage: Stage,
    left: f64,
    level: f64,
    attack_step: f64,
    hold: f64,
    decay_mul: f64,
    sustain: f64,
    release_mul: f64,
}

impl VolEnv {
    fn next(&mut self) -> f64 {
        loop {
            match self.stage {
                Stage::Delay => {
                    if self.left >= 1.0 {
                        self.left -= 1.0;
                        return 0.0;
                    }
                    self.stage = Stage::Attack;
                }
                Stage::Attack => {
                    self.level += self.attack_step;
                    if self.level >= 1.0 {
                        self.level = 1.0;
                        self.stage = Stage::Hold;
                        self.left = self.hold;
                    }
                    return self.level;
                }
                Stage::Hold => {
                    if self.left >= 1.0 {
                        self.left -= 1.0;
                        return self.level;
                    }
                    self.stage = Stage::Decay;
                }
                Stage::Decay => {
                    if self.level <= self.sustain {
                        self.level = self.sustain;
                        self.stage = Stage::Sustain;
                        continue;
                    }
                    self.level *= self.decay_mul;
                    return self.level.max(self.sustain);
                }
                Stage::Sustain => {
                    if self.level <= 0.0 {
                        self.stage = Stage::Done;
                        continue;
                    }
                    return self.level;
                }
                Stage::Release => {
                    self.level *= self.release_mul;
                    if self.level < 1e-5 {
                        self.stage = Stage::Done;
                        return 0.0;
                    }
                    return self.level;
                }
                Stage::Done => return 0.0,
            }
        }
    }

    fn release(&mut self) {
        if !matches!(self.stage, Stage::Done) {
            self.stage = if self.level <= 0.0 { Stage::Done } else { Stage::Release };
        }
    }
}

/// The modulation envelope: linear stages between 0 and 1.
#[derive(Clone, Debug)]
struct ModEnv {
    stage: Stage,
    left: f64,
    level: f64,
    attack_step: f64,
    hold: f64,
    decay_step: f64,
    sustain: f64,
    release_step: f64,
}

impl ModEnv {
    fn advance(&mut self, n: f64) -> f64 {
        match self.stage {
            Stage::Delay => {
                self.left -= n;
                if self.left <= 0.0 {
                    self.stage = Stage::Attack;
                }
            }
            Stage::Attack => {
                self.level += self.attack_step * n;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Hold;
                    self.left = self.hold;
                }
            }
            Stage::Hold => {
                self.left -= n;
                if self.left <= 0.0 {
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level -= self.decay_step * n;
                if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain | Stage::Done => {}
            Stage::Release => {
                self.level = (self.level - self.release_step * n).max(0.0);
                if self.level <= 0.0 {
                    self.stage = Stage::Done;
                }
            }
        }
        self.level
    }
}

/// A triangle LFO (−1..1) that starts after its delay, rising from 0.
#[derive(Clone, Debug)]
struct Lfo {
    delay: f64,
    phase: f64,
    step: f64,
}

impl Lfo {
    fn advance(&mut self, n: f64) -> f64 {
        if self.delay > 0.0 {
            self.delay -= n;
            return 0.0;
        }
        self.phase = (self.phase + self.step * n).fract();
        let p = self.phase;
        if p < 0.25 { 4.0 * p } else if p < 0.75 { 2.0 - 4.0 * p } else { 4.0 * p - 4.0 }
    }
}

/// The Q of a 2-pole low-pass whose peak is `db` above its passband (0 dB → Butterworth).
fn q_for_peak(db: f64) -> f64 {
    let g2 = 10f64.powf(db.max(0.0) / 10.0);
    ((g2 + (g2 * g2 - g2).max(0.0).sqrt()) / 2.0).sqrt()
}

/// Ratio of the classic filter's −3 dB point to its SoundFont cutoff, by cutoff (cents).
const CLASSIC_F3: [(f32, f32); 7] = [(5000.0, 1.30), (6000.0, 1.27), (7000.0, 1.27), (8000.0, 1.298), (9000.0, 1.298), (10000.0, 1.385), (11000.0, 1.545)];

fn lerp(t: &[(f32, f32)], x: f32) -> f32 {
    if x <= t[0].0 {
        return t[0].1;
    }
    for w in t.windows(2) {
        if x <= w[1].0 {
            return w[0].1 + (w[1].1 - w[0].1) * (x - w[0].0) / (w[1].0 - w[0].0);
        }
    }
    t[t.len() - 1].1
}

/// The −3 dB point of a low-pass at f0 with resonance `q`, as a multiple of f0.
fn f3_ratio(q: f64) -> f64 {
    let b = 1.0 / (q * q) - 2.0;
    ((-b + (b * b + 4.0).sqrt()) / 2.0).sqrt()
}

/// The resonant 2-pole low-pass (a biquad).
#[derive(Clone, Debug, Default)]
struct Filter {
    on: bool,
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Filter {
    /// A low-pass at `f0` whose resonance peak is `peak_db` above the passband; the level is kept by
    /// `sqrt(base_q / q)` (the mode's unresonant filter keeps unity).
    fn set(&mut self, f0: f64, peak_db: f64, base_q: f64, rate: f64) {
        let fc = f0.clamp(5.0, rate * 0.45);
        let q = q_for_peak(peak_db);
        let w0 = 2.0 * PI * fc / rate;
        let alpha = w0.sin() / (2.0 * q);
        let cw = w0.cos();
        let a0 = 1.0 + alpha;
        let gain = (base_q / q).sqrt().min(1.0);
        self.b0 = (1.0 - cw) / 2.0 / a0 * gain;
        self.b1 = (1.0 - cw) / a0 * gain;
        self.b2 = self.b0;
        self.a1 = -2.0 * cw / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Controller values a modulator reads (0..1 unless said).
#[derive(Clone, Copy, Debug)]
pub struct Controls {
    pub cc: [f32; 128],
    pub pressure: f32,
    /// Pitch wheel, −1..1.
    pub bend: f32,
    /// Pitch wheel range, semitones.
    pub bend_range: f32,
}

impl Default for Controls {
    fn default() -> Self {
        let mut cc = [0.0; 128];
        cc[7] = 1.0;
        cc[10] = 0.5;
        cc[11] = 1.0;
        Controls { cc, pressure: 0.0, bend: 0.0, bend_range: 2.0 }
    }
}

fn concave(x: f64) -> f64 {
    if x <= 0.0 {
        0.0
    } else if x >= 1.0 {
        1.0
    } else {
        (-(40.0 / 96.0) * (1.0 - x).log10()).min(1.0)
    }
}

fn convex(x: f64) -> f64 {
    1.0 - concave(1.0 - x)
}

/// A modulator source's value (§8.2.1), with its direction, polarity and curve applied.
fn source(src: u16, key: u8, vel: f64, c: &Controls) -> f64 {
    let index = src & 0x7F;
    let x = if src & 0x80 != 0 {
        f64::from(c.cc[index as usize])
    } else {
        match index {
            0 => 1.0,
            2 => vel / 128.0,
            3 => f64::from(key) / 128.0,
            13 => f64::from(c.pressure),
            14 => (f64::from(c.bend) + 1.0) / 2.0,
            16 => f64::from(c.bend_range) / 128.0,
            _ => 0.0,
        }
    };
    let x = if src & 0x100 != 0 { 1.0 - x } else { x };
    let bipolar = src & 0x200 != 0;
    let curve = |v: f64| match src >> 10 {
        1 => concave(v),
        2 => convex(v),
        3 => {
            if v >= 0.5 {
                1.0
            } else {
                0.0
            }
        }
        _ => v,
    };
    if bipolar {
        let v = 2.0 * x - 1.0;
        let m = curve(v.abs());
        if v < 0.0 { -m } else { m }
    } else {
        curve(x)
    }
}

/// Whether a modulator is one the player handles itself: the velocity → attenuation default (the mode's measured
/// velocity law), the pitch wheel default (the host's bend), and in classic mode the velocity → cutoff default
/// (which that player does not apply).
fn handled(m: &Modulator, mode: Mode) -> bool {
    (m.src == 0x0502 && m.dest == g::ATTENUATION as u16 && m.amt_src == 0 && m.transform == 0)
        || (m.src == 0x020E && m.dest == g::FINE_TUNE as u16 && m.amt_src == 0x0010)
        || (!mode.velocity_filters() && m.src == 0x0102 && m.dest == g::FILTER_FC as u16 && m.amt_src == 0)
}

/// The generator offsets of a voice's modulators.
fn modulate(spec: &VoiceSpec, key: u8, vel: f64, c: &Controls, mode: Mode) -> [f64; g::COUNT] {
    let mut out = [0.0; g::COUNT];
    for m in &spec.mods {
        if handled(m, mode) || (m.dest as usize) >= g::COUNT {
            continue;
        }
        let v = source(m.src, key, vel, c) * f64::from(m.amount) * source(m.amt_src, key, vel, c);
        let v = if m.transform == 2 { v.abs() } else { v };
        out[classic_dest(m, mode)] += v;
    }
    out
}

/// Where a modulator lands. Classic: the mod wheel's vibrato (CC1 → vibrato LFO pitch) goes to the modulation LFO's
/// pitch instead, so it runs at that LFO's rate and after its delay (measured: FL 12 with a font whose modulation LFO
/// is 3 Hz and its vibrato LFO 8.18 Hz vibrates at 3 Hz on MOD, and starts after the modulation LFO's delay, not the
/// vibrato LFO's).
fn classic_dest(m: &Modulator, mode: Mode) -> usize {
    let d = m.dest as usize;
    if mode == Mode::Classic && m.src & 0xFF == 0x81 && d == g::VIBLFO_TO_PITCH { g::MODLFO_TO_PITCH } else { d }
}

pub struct Voice {
    pub key: u8,
    pub id: Option<i32>,
    age: u64,
    data: Arc<Vec<i16>>,
    start: i64,
    end: i64,
    loop_start: i64,
    loop_end: i64,
    loop_mode: i32,
    exclusive: i32,
    pos: f64,
    /// Source frames per output frame, unmodulated.
    inc: f64,
    released: bool,
    mode: Mode,
    gain: f64,
    pan: (f64, f64),
    tuning: f64,
    vol: VolEnv,
    menv: ModEnv,
    mod_lfo: Lfo,
    vib_lfo: Lfo,
    filter: Filter,
    fc: f64,
    q: f64,
    cut: Option<CutoffSetting>,
    mod_lfo_pitch: f64,
    vib_lfo_pitch: f64,
    menv_pitch: f64,
    mod_lfo_fc: f64,
    menv_fc: f64,
    mod_lfo_vol: f64,
    /// Frames until the modulation is next worked out.
    block_left: u32,
    factor: f64,
    amp: f64,
}

const BLOCK: u32 = 16;

impl Voice {
    #[allow(clippy::too_many_arguments)]
    fn new(font: &Font, spec: &VoiceSpec, key: u8, vel: f32, id: Option<i32>, rate: f64, mode: Mode, ov: &Overrides, c: &Controls, age: u64) -> Voice {
        let velb = vel * 128.0;
        let m = modulate(spec, key, f64::from(velb), c, mode);
        let gv = |op: usize| f64::from(spec.gens[op]) + m[op];
        let sh = &font.samples[spec.sample];
        let coarse = |a: usize, b: usize| i64::from(spec.gens[a]) + 32768 * i64::from(spec.gens[b]);
        let start = i64::from(sh.start) + coarse(g::START_OFS, g::START_COARSE);
        let end = i64::from(sh.end) + coarse(g::END_OFS, g::END_COARSE);
        let loop_start = i64::from(sh.loop_start) + coarse(g::STARTLOOP_OFS, g::STARTLOOP_COARSE);
        let loop_end = i64::from(sh.loop_end) + coarse(g::ENDLOOP_OFS, g::ENDLOOP_COARSE);
        let n = font.data.len() as i64;
        let (start, end) = (start.clamp(0, n), end.clamp(0, n));
        let mut loop_mode = spec.gens[g::SAMPLE_MODES] & 3;
        if loop_mode == 2 || loop_end - loop_start < 1 || loop_start < start || loop_end > end {
            loop_mode = 0;
        }
        let keynum = if spec.gens[g::KEYNUM] >= 0 { spec.gens[g::KEYNUM] } else { i32::from(key) };
        let root = if spec.gens[g::ROOT_KEY] >= 0 { spec.gens[g::ROOT_KEY] } else if sh.root <= 127 { i32::from(sh.root) } else { 60 };
        let tuning = gv(g::SCALE_TUNING) * f64::from(keynum - root) + 100.0 * gv(g::COARSE_TUNE) + gv(g::FINE_TUNE) + f64::from(sh.correction);
        let inc = f64::from(sh.rate.max(1)) / rate;

        let t = |tc: f64| timecents_to_secs(tc.round() as i32) * rate;
        let key_scale = |base: usize, by: usize| gv(base) + gv(by) * f64::from(60 - keynum);
        let r441 = rate / 44100.0;
        // The player's controls: attack in ms; decay, release and LFO delay in 4 ms units.
        let ms = |v: i32, unit: f64| f64::from(v.max(0)) * unit / 1000.0 * rate;
        let attack = ov.attack.map_or_else(|| t(gv(g::VOLENV_ATTACK)), |a| ms(a, 1.0)).max(mode.min_attack() * r441);
        let decay = ov.decay.map_or_else(|| t(key_scale(g::VOLENV_DECAY, g::KEY_TO_VOLENV_DECAY)), |d| ms(d, 4.0)).max(1.0);
        let release = ov.release.map_or_else(|| t(gv(g::VOLENV_RELEASE)), |r| ms(r, 4.0)).max(1.0);
        let sustain = match ov.sustain {
            Some(s) => f64::from(mode.sustain_control(s)),
            None => f64::from(mode.sustain_gain(gv(g::VOLENV_SUSTAIN) as f32)),
        };
        // 96 dB per stage time.
        let per = |frames: f64| 10f64.powf(-96.0 / 20.0 / frames);
        let vol = VolEnv {
            stage: Stage::Delay,
            left: t(gv(g::VOLENV_DELAY)) + mode.start_delay() * r441,
            level: 0.0,
            attack_step: 1.0 / attack,
            hold: t(key_scale(g::VOLENV_HOLD, g::KEY_TO_VOLENV_HOLD)),
            decay_mul: per(decay),
            sustain,
            release_mul: per(release),
        };
        let msus = 1.0 - (gv(g::MODENV_SUSTAIN) / 1000.0).clamp(0.0, 1.0);
        let menv = ModEnv {
            stage: Stage::Delay,
            left: t(gv(g::MODENV_DELAY)),
            level: 0.0,
            attack_step: 1.0 / t(gv(g::MODENV_ATTACK)).max(1.0),
            hold: t(key_scale(g::MODENV_HOLD, g::KEY_TO_MODENV_HOLD)),
            decay_step: 1.0 / t(key_scale(g::MODENV_DECAY, g::KEY_TO_MODENV_DECAY)).max(1.0),
            sustain: msus,
            release_step: 1.0 / t(gv(g::MODENV_RELEASE)).max(1.0),
        };
        let lfo = |delay: f64, freq: f64| Lfo { delay: t(delay), phase: 0.0, step: cents_to_hz(freq) / rate };
        let mod_lfo = lfo(gv(g::MODLFO_DELAY), gv(g::MODLFO_FREQ));
        let mut vib_lfo = lfo(gv(g::VIBLFO_DELAY), gv(g::VIBLFO_FREQ));
        if let Some(d) = ov.lfo_delay {
            vib_lfo.delay = ms(d, 4.0);
        }
        if let Some(s) = ov.lfo_speed {
            vib_lfo.step = mode.lfo_rate(s) / rate;
        }
        let fc = gv(g::FILTER_FC);
        let q = gv(g::FILTER_Q);
        let att = mode.attenuation_gain(gv(g::ATTENUATION) as f32) as f64;
        let gain = f64::from(mode.gain()) * f64::from(mode.velocity_gain(velb)) * att;
        let (pl, pr) = mode.pan_gains(gv(g::PAN) as f32);
        let mut v = Voice {
            key,
            id,
            age,
            data: font.data.clone(),
            start,
            end,
            loop_start,
            loop_end,
            loop_mode,
            exclusive: spec.gens[g::EXCLUSIVE_CLASS],
            // Classic reads from the first frame; modern half a frame earlier.
            pos: match mode {
                Mode::Classic => start as f64,
                Mode::Modern => start as f64 - 0.5,
            },
            inc,
            released: false,
            mode,
            gain,
            pan: (f64::from(pl), f64::from(pr)),
            tuning,
            vol,
            menv,
            mod_lfo,
            vib_lfo,
            filter: Filter::default(),
            fc,
            q,
            cut: None,
            mod_lfo_pitch: gv(g::MODLFO_TO_PITCH),
            vib_lfo_pitch: ov.lfo_amount.map_or(gv(g::VIBLFO_TO_PITCH), |a| mode.lfo_depth(a)),
            menv_pitch: gv(g::MODENV_TO_PITCH),
            mod_lfo_fc: gv(g::MODLFO_TO_FC),
            menv_fc: gv(g::MODENV_TO_FC),
            mod_lfo_vol: gv(g::MODLFO_TO_VOL),
            block_left: 0,
            factor: 1.0,
            amp: 1.0,
        };
        v.cut = ov.cutoff.map(|c| mode.cutoff_control(c));
        let open = match mode {
            Mode::Classic => fc >= 12000.0,
            Mode::Modern => fc >= 13500.0 && q <= 0.0,
        };
        v.filter.on = v.cut.is_some_and(|c| c != CutoffSetting::Open) || !open || v.mod_lfo_fc != 0.0 || v.menv_fc != 0.0;
        v
    }

    /// The frames `lo..=hi` are all inside the sample and before any loop wrap: they can be read directly.
    fn plain(&self, lo: i64, hi: i64) -> bool {
        let wraps = self.loop_mode != 0 && (self.loop_mode == 1 || !self.released);
        lo >= self.start && hi < if wraps { self.loop_end.min(self.end) } else { self.end }
    }

    fn sample(&self, k: i64) -> f32 {
        let mut k = k;
        if self.loop_mode != 0 && (self.loop_mode == 1 || !self.released) && k >= self.loop_end {
            let len = self.loop_end - self.loop_start;
            k = self.loop_start + (k - self.loop_start) % len;
        }
        if k < self.start || k >= self.end { 0.0 } else { f32::from(self.data[k as usize]) / 32768.0 }
    }

    fn interpolate(&self, step: f64) -> f64 {
        let p = self.pos;
        match self.mode {
            Mode::Classic if step <= 1.0 => {
                let i = p.floor();
                let f = p - i;
                let i = i as i64;
                f64::from(self.sample(i)) * (1.0 - f) + f64::from(self.sample(i + 1)) * f
            }
            Mode::Classic => {
                // Pitching up: band-limited to the output's Nyquist, `c * sinc(c * x)` under a Hann window. The taps
                // are a frame apart, so the sinc's sine and the window's cosine turn by fixed angles from tap to tap:
                // they are rotated rather than worked out again (64 taps a frame on every voice pitched up).
                let c = 1.0 / step;
                let lo = (p - HANN_HALF).ceil() as i64;
                let hi = (p + HANN_HALF).floor() as i64;
                let x0 = p - lo as f64;
                let (a, b) = (PI * c, PI / HANN_HALF);
                let (mut sn, mut cs) = (a * x0).sin_cos();
                let (mut wsn, mut wcs) = (b * x0).sin_cos();
                let (da_s, da_c) = a.sin_cos();
                let (db_s, db_c) = b.sin_cos();
                let plain = self.plain(lo, hi);
                let mut acc = 0.0;
                for k in lo..=hi {
                    let s = if plain { f32::from(self.data[k as usize]) / 32768.0 } else { self.sample(k) };
                    if s != 0.0 {
                        let ax = a * (p - k as f64);
                        let si = if ax.abs() < 1e-12 * PI { 1.0 } else { sn / ax };
                        acc += f64::from(s) * c * si * (0.5 + 0.5 * wcs);
                    }
                    // x goes down by 1: both angles turn back by their step.
                    (sn, cs) = (sn * da_c - cs * da_s, cs * da_c + sn * da_s);
                    (wsn, wcs) = (wsn * db_c - wcs * db_s, wcs * db_c + wsn * db_s);
                }
                acc
            }
            Mode::Modern => {
                let k = Kaiser::get();
                let lo = (p - KAISER_HALF).ceil() as i64;
                let hi = (p + KAISER_HALF).floor() as i64;
                let plain = self.plain(lo, hi);
                let mut acc = 0.0;
                for i in lo..=hi {
                    let s = if plain { f32::from(self.data[i as usize]) / 32768.0 } else { self.sample(i) };
                    acc += f64::from(s) * f64::from(k.at(p - i as f64));
                }
                acc
            }
        }
    }

    /// Works out the modulated pitch, filter and level for the next block.
    fn update(&mut self, rate: f64, c: &Controls, tuning: f64) {
        let n = f64::from(BLOCK);
        let menv = self.menv.advance(n);
        let ml = self.mod_lfo.advance(n);
        let vl = self.vib_lfo.advance(n);
        let cents = self.tuning + tuning + f64::from(c.bend * c.bend_range) * 100.0 + menv * self.menv_pitch + ml * self.mod_lfo_pitch + vl * self.vib_lfo_pitch;
        self.factor = 2f64.powf(cents / 1200.0);
        self.amp = if self.mod_lfo_vol != 0.0 { 10f64.powf(-(ml * self.mod_lfo_vol) / 200.0) } else { 1.0 };
        if self.filter.on {
            let cents = self.fc + menv * self.menv_fc + ml * self.mod_lfo_fc;
            let peak = self.mode.resonance_peak(self.q);
            let base = q_for_peak(self.mode.resonance_peak(0.0));
            let q = q_for_peak(peak);
            let f0 = match (self.mode, self.cut) {
                (_, Some(CutoffSetting::Open)) => rate,
                (Mode::Classic, Some(CutoffSetting::Hz(f3))) => f3 / f3_ratio(q),
                (_, Some(CutoffSetting::Cents(c))) => cents_to_hz(c),
                // Classic: held to 5000..12000 cents, its -3 dB point where the original's was.
                (Mode::Classic, _) => {
                    if cents >= 12000.0 {
                        rate
                    } else {
                        let c = cents.max(5000.0);
                        cents_to_hz(c) * f64::from(lerp(&CLASSIC_F3, c as f32)) / f3_ratio(q)
                    }
                }
                (Mode::Modern, _) => cents_to_hz(cents.clamp(1500.0, 13500.0)),
            };
            if f0 >= rate * 0.45 {
                self.filter.b0 = 1.0;
                self.filter.b1 = 0.0;
                self.filter.b2 = 0.0;
                self.filter.a1 = 0.0;
                self.filter.a2 = 0.0;
            } else {
                self.filter.set(f0, peak, base, rate);
            }
        }
    }

    /// Adds `l.len()` frames to the outputs; false once the voice has ended.
    fn render(&mut self, l: &mut [f32], r: &mut [f32], rate: f64, c: &Controls, tuning: f64) -> bool {
        for i in 0..l.len() {
            if self.block_left == 0 {
                self.update(rate, c, tuning);
                self.block_left = BLOCK;
            }
            self.block_left -= 1;
            let env = self.vol.next();
            if self.vol.stage == Stage::Done {
                return false;
            }
            let step = self.inc * self.factor;
            let mut x = self.interpolate(step);
            if self.filter.on {
                x = self.filter.run(x);
            }
            let y = x * env * self.gain * self.amp;
            l[i] += (y * self.pan.0) as f32;
            r[i] += (y * self.pan.1) as f32;
            self.pos += step;
            // Looping: wrap once the loop's end is passed (until release in mode 3).
            if self.loop_mode != 0 && (self.loop_mode == 1 || !self.released) {
                let len = (self.loop_end - self.loop_start) as f64;
                while self.pos >= self.loop_end as f64 {
                    self.pos -= len;
                }
            } else if self.pos >= self.end as f64 + KAISER_HALF.max(1.0) + if self.mode == Mode::Classic && step > 1.0 { HANN_HALF } else { 0.0 } {
                return false;
            }
        }
        true
    }

    fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        // Mode 3 plays on from where it is to the sample's end.
        self.vol.release();
        if !matches!(self.menv.stage, Stage::Done) {
            self.menv.stage = Stage::Release;
        }
    }

    pub fn is_released(&self) -> bool {
        self.released
    }
}

pub struct Engine {
    rate: f64,
    pub mode: Mode,
    patch: Option<Arc<Patch>>,
    pub overrides: Overrides,
    pub controls: Controls,
    voices: Vec<Voice>,
    ended: Vec<(u8, Option<i32>)>,
    age: u64,
    /// Tuning from the host (semitones), by voice id and key. An entry without an id is for every voice of its key
    /// (Lemon Studio sends a note's tuning so: CLAP's note id -1); a voice's own entry comes first.
    tunings: Vec<(Option<i32>, u8, f64)>,
    chorus: Option<crate::effects::Chorus>,
    reverb: Option<crate::effects::Reverb>,
    /// Frames the effects' tails still ring after the last voice.
    tail: usize,
}

impl Engine {
    pub fn new(rate: f32) -> Engine {
        Engine {
            rate: f64::from(rate),
            mode: Mode::Classic,
            patch: None,
            overrides: Overrides::default(),
            controls: Controls::default(),
            voices: Vec::with_capacity(MAX_VOICES),
            ended: Vec::new(),
            age: 0,
            tunings: Vec::new(),
            chorus: None,
            reverb: None,
            tail: 0,
        }
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.rate = f64::from(rate);
        self.chorus = None;
        self.reverb = None;
    }

    /// Whether the built-in effects play: the modern mode, switched on, above level 0.
    fn effects(&self) -> (bool, bool) {
        let o = &self.overrides;
        let m = self.mode == Mode::Modern;
        (m && o.chorus_on && o.chorus_level > 0, m && o.reverb_on && o.reverb_level > 0)
    }

    pub fn set_patch(&mut self, p: Option<Arc<Patch>>) {
        let ov = p.as_ref().map(|p| p.overrides).unwrap_or_default();
        self.overrides = ov;
        self.controls.cc[1] = ov.mod_wheel.clamp(0, 127) as f32 / 128.0;
        self.patch = p;
    }

    pub fn reset(&mut self) {
        for v in self.voices.drain(..) {
            self.ended.push((v.key, v.id));
        }
    }

    /// Whether anything still sounds: a voice, or an effect's tail.
    pub fn has_voices(&self) -> bool {
        !self.voices.is_empty() || self.tail > 0
    }

    pub fn take_ended(&mut self) -> Vec<(u8, Option<i32>)> {
        std::mem::take(&mut self.ended)
    }

    pub fn note_on(&mut self, key: u8, vel: f32, id: Option<i32>) {
        let Some(patch) = self.patch.clone() else {
            self.ended.push((key, id));
            return;
        };
        let Some(p) = patch.preset else {
            self.ended.push((key, id));
            return;
        };
        let midi_vel = (vel * 128.0).round().clamp(0.0, 127.0) as u8;
        if midi_vel == 0 {
            self.ended.push((key, id));
            return;
        }
        let specs = patch.font.voices(p, key, midi_vel);
        if specs.is_empty() {
            self.ended.push((key, id));
            return;
        }
        self.pin_key_tuning(key);
        // Exclusive classes cut each other's voices.
        for s in &specs {
            let ex = s.gens[g::EXCLUSIVE_CLASS];
            if ex != 0 {
                for v in self.voices.iter_mut().filter(|v| v.exclusive == ex) {
                    v.vol.stage = Stage::Done;
                }
            }
        }
        for s in &specs {
            if self.voices.len() >= MAX_VOICES {
                // Steal the oldest, released ones first.
                let i = (0..self.voices.len()).min_by_key(|&i| (!self.voices[i].released, self.voices[i].age)).unwrap_or(0);
                let v = self.voices.remove(i);
                if !self.voices.iter().any(|o| o.id == v.id && o.key == v.key) {
                    self.ended.push((v.key, v.id));
                }
            }
            self.age += 1;
            let v = Voice::new(&patch.font, s, key, vel, id, self.rate, self.mode, &self.overrides, &self.controls, self.age);
            self.voices.push(v);
        }
    }

    /// A new note of `key` starts untuned: the key's tuning so far stays with the voices sounding (a note keeps the
    /// pitch it started with), each as its own.
    fn pin_key_tuning(&mut self, key: u8) {
        let Some(i) = self.tunings.iter().position(|t| t.0.is_none() && t.1 == key) else { return };
        let (_, _, st) = self.tunings.remove(i);
        let mut ids: Vec<i32> = self.voices.iter().filter(|v| v.key == key).filter_map(|v| v.id).collect();
        ids.dedup();
        for id in ids {
            if !self.tunings.iter().any(|t| t.0 == Some(id) && t.1 == key) {
                self.tunings.push((Some(id), key, st));
            }
        }
    }

    fn tuning_of(&self, v: &Voice) -> f64 {
        let own = self.tunings.iter().find(|t| t.0.is_some() && t.0 == v.id && t.1 == v.key);
        own.or_else(|| self.tunings.iter().find(|t| t.0.is_none() && t.1 == v.key)).map_or(0.0, |t| t.2 * 100.0)
    }

    pub fn note_off(&mut self, key: u8, id: Option<i32>) {
        for v in self.voices.iter_mut().filter(|v| if id.is_some() { v.id == id } else { v.key == key && !v.released }) {
            v.release();
        }
    }

    /// Sets a voice's tuning from the host (semitones).
    pub fn set_tuning(&mut self, key: u8, id: Option<i32>, semitones: f64) {
        self.tunings.retain(|t| !(t.0 == id && t.1 == key));
        if semitones != 0.0 {
            self.tunings.push((id, key, semitones));
        }
    }

    pub fn all_off(&mut self) {
        self.reset();
    }

    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        let (rate, c) = (self.rate, self.controls);
        let sounding = !self.voices.is_empty();
        let mut i = 0;
        while i < self.voices.len() {
            let tune = self.tuning_of(&self.voices[i]);
            if self.voices[i].render(l, r, rate, &c, tune) {
                i += 1;
            } else {
                let v = self.voices.remove(i);
                if !self.voices.iter().any(|o| o.id == v.id && o.key == v.key) {
                    self.tunings.retain(|t| !(t.0.is_some() && t.0 == v.id && t.1 == v.key));
                    self.ended.push((v.key, v.id));
                }
                if !self.voices.iter().any(|o| o.key == v.key) {
                    self.tunings.retain(|t| !(t.0.is_none() && t.1 == v.key));
                }
            }
        }
        let (ch, rv) = self.effects();
        if !(ch || rv) {
            self.tail = 0;
            return;
        }
        // A voice that sounded in this block starts (or restarts) the tails.
        if sounding {
            let (rt, _) = crate::effects::Reverb::settings(self.overrides.reverb_level);
            self.tail = ((rt * 1.2 + 0.1) * rate) as usize;
        }
        if self.tail == 0 {
            return;
        }
        let rate = self.rate;
        let cg = crate::effects::Chorus::gain(self.overrides.chorus_level);
        let (rt, rg) = crate::effects::Reverb::settings(self.overrides.reverb_level);
        let rg = crate::effects::reverb_gain(rt, rg) as f32;
        let chorus = self.chorus.get_or_insert_with(|| crate::effects::Chorus::new(rate));
        let reverb = self.reverb.get_or_insert_with(|| crate::effects::Reverb::new(rate));
        for i in 0..l.len() {
            let x = 0.5 * (l[i] + r[i]);
            let (mut a, mut b) = (l[i], r[i]);
            if ch {
                chorus.run(x, cg, &mut a, &mut b);
            }
            if rv {
                reverb.run(x, rt, rg, &mut a, &mut b);
            }
            l[i] = a;
            r[i] = b;
        }
        self.tail = self.tail.saturating_sub(l.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sf2::tests::font_bytes;

    fn engine(data: &[i16], rate: u32, igens: &[(u16, i16)], mode: Mode) -> Engine {
        let font = Arc::new(Font::parse(&font_bytes(data, rate, (0, 0), igens, 0)).unwrap());
        let mut e = Engine::new(44100.0);
        e.mode = mode;
        e.set_patch(Some(Arc::new(Patch { font, preset: Some(0), overrides: Overrides::default() })));
        e
    }

    fn run(e: &mut Engine, n: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        e.render(&mut l, &mut r);
        (l, r)
    }

    /// Classic: a full-scale DC sample at its own rate and key plays at the measured gain after an 88-frame ramp.
    #[test]
    fn classic_plays_dc_at_its_gain() {
        let mut e = engine(&[16384; 8000], 44100, &[], Mode::Classic);
        e.note_on(60, 127.0 / 128.0, None);
        let (l, r) = run(&mut e, 4000);
        let g = 0.5 * crate::tables::CLASSIC_GAIN;
        assert!((l[0] - g / 88.0).abs() < 1e-4, "first frame {}", l[0]);
        assert!((l[87] - g).abs() < 1e-4 && (l[2000] - g).abs() < 1e-4, "{} {}", l[87], l[2000]);
        assert_eq!(l, r);
    }

    /// Classic pitching up: the kernel worked out by rotation gives what the direct `cos` / `sinc` sum gives, inside a
    /// sample, across a loop's wrap and at its ends.
    #[test]
    fn classic_pitch_up_kernel_matches_the_direct_sum() {
        let data: Vec<i16> = (0..3000).map(|i| ((i as f64 * 0.37).sin() * 12000.0 + (i as f64 * 0.011).cos() * 9000.0) as i16).collect();
        let font = Arc::new(Font::parse(&font_bytes(&data, 44100, (500, 2500), &[(54, 1)], 0)).unwrap());
        let s = font.voices(0, 60, 100);
        let mut v = Voice::new(&font, &s[0], 60, 100.0 / 128.0, None, 44100.0, Mode::Classic, &Overrides::default(), &Controls::default(), 1);
        for &(pos, step) in &[(1000.3, 1.7), (2480.75, 2.3), (2499.0, 1.01), (10.5, 3.9), (1500.0, 1.25), (2520.2, 5.0)] {
            v.pos = pos;
            let got = v.interpolate(step);
            let c = 1.0 / step;
            let mut want = 0.0;
            for k in (pos - HANN_HALF).ceil() as i64..=(pos + HANN_HALF).floor() as i64 {
                let x = pos - k as f64;
                want += f64::from(v.sample(k)) * c * sinc(c * x) * (0.5 + 0.5 * (PI * x / HANN_HALF).cos());
            }
            assert!((got - want).abs() < 1e-9, "pos {pos} step {step}: {got} vs {want}");
        }
    }

    /// Classic pitching down interpolates linearly; an impulse becomes a triangle.
    #[test]
    fn classic_pitching_down_is_linear() {
        let mut d = vec![0i16; 400];
        d[100] = 16384;
        let mut e = engine(&d, 4410, &[(34, -12000)], Mode::Classic);
        e.note_on(60, 127.0 / 128.0, None);
        let (l, _) = run(&mut e, 2000);
        let pk = 0.5 * crate::tables::CLASSIC_GAIN;
        assert!((l[1000] - pk).abs() < 1e-4);
        assert!((l[1005] - pk * 0.5).abs() < 1e-4, "{}", l[1005]);
        assert!(l[1010].abs() < 1e-6 && l[990].abs() < 1e-6);
    }

    /// Modern starts 32 frames late, ramps over 64, and plays a Kaiser-sinc kernel.
    #[test]
    fn modern_starts_late_and_rings() {
        let mut e = engine(&[16384; 8000], 44100, &[], Mode::Modern);
        e.note_on(60, 127.0 / 128.0, None);
        let (l, _) = run(&mut e, 400);
        assert_eq!(l[31], 0.0);
        assert!(l[32] > 0.0);
        let g = 0.5 * crate::tables::MODERN_GAIN;
        assert!((l[200] - g).abs() < 1e-3, "{}", l[200]);
        let k = Kaiser::get();
        assert!((k.at(0.0) - 1.0).abs() < 1e-6);
        assert!(k.at(1.0).abs() < 2e-3 && k.at(2.0).abs() < 2e-3);
        assert!((k.at(0.5) - 0.5697).abs() < 0.01, "{}", k.at(0.5));
    }

    /// Lemon Studio tunes a note for every voice of its key (no voice id): a voice started with an id of its own
    /// follows it, and a later note of that key starts untuned while the first keeps its tuning.
    #[test]
    fn a_key_wide_tuning_reaches_voices_with_ids() {
        let mut d = vec![0i16; 400];
        d[100] = 16384;
        let peak = |l: &[f32]| (0..l.len()).max_by(|&a, &b| l[a].total_cmp(&l[b])).unwrap();
        // Untuned: one source frame per 10 output frames, the impulse at frame 1000.
        let mut e = engine(&d, 4410, &[(34, -12000)], Mode::Classic);
        e.note_on(60, 127.0 / 128.0, Some(7));
        assert_eq!(peak(&run(&mut e, 2000).0), 1000);
        // An octave up: at frame 500.
        let mut e = engine(&d, 4410, &[(34, -12000)], Mode::Classic);
        e.note_on(60, 127.0 / 128.0, Some(7));
        e.set_tuning(60, None, 12.0);
        assert_eq!(peak(&run(&mut e, 2000).0), 500);
        // A second note of the key: untuned (its impulse 1000 frames after it starts), the first still an octave up.
        let mut e = engine(&d, 4410, &[(34, -12000)], Mode::Classic);
        e.note_on(60, 127.0 / 128.0, Some(7));
        e.set_tuning(60, None, 12.0);
        let _ = run(&mut e, 100);
        e.note_on(60, 127.0 / 128.0, Some(8));
        let (l, _) = run(&mut e, 1900);
        assert_eq!(peak(&l[..600]), 400, "the first voice keeps its octave");
        assert_eq!(peak(&l[600..]) + 600, 1000, "the second starts untuned");
    }

    /// The mod wheel's vibrato: on the modulation LFO in Classic (FL 12), on the vibrato LFO in Modern (the SF2
    /// default modulator).
    #[test]
    fn the_mod_wheel_vibrato_is_on_the_mod_lfo_in_classic() {
        let font = Font::parse(&font_bytes(&[16384; 8000], 44100, (0, 0), &[], 0)).unwrap();
        let spec = &font.voices(0, 60, 100)[0];
        let mut c = Controls::default();
        c.cc[1] = 1.0;
        let m = modulate(spec, 60, 100.0, &c, Mode::Classic);
        assert_eq!((m[g::MODLFO_TO_PITCH], m[g::VIBLFO_TO_PITCH]), (50.0, 0.0));
        let m = modulate(spec, 60, 100.0, &c, Mode::Modern);
        assert_eq!((m[g::MODLFO_TO_PITCH], m[g::VIBLFO_TO_PITCH]), (0.0, 50.0));
    }

    /// The release falls 96 dB per release time.
    #[test]
    fn release_falls_96_db_per_its_time() {
        let mut e = engine(&[16384; 88200], 44100, &[(54, 0), (38, 0)], Mode::Classic);
        e.note_on(60, 127.0 / 128.0, None);
        let (a, _) = run(&mut e, 1000);
        e.note_off(60, None);
        let (b, _) = run(&mut e, 4410);
        let drop = 20.0 * (b[4409] / a[999]).log10();
        assert!((drop + 9.6).abs() < 0.1, "{drop}");
    }
}
