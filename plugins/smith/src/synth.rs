// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Smith's sound. Each note builds its spectrum once, from a seed pattern and the EQ maps, and lets every harmonic
//! decay at its own rate. Summing up to 32767 sines one by one would be too slow, so a note renders tables: one period
//! of the wave, built by an inverse FFT, read at the note's rate by every unison voice. What all harmonics share of
//! their decay is a gain ramp; what differs is carried by a new table each frame, faded into from the last.
//
// spec: plugins/ogun.md ("Harmonics" to "Unison"), plugins/smith.md ("Rendering")

use std::sync::{Arc, OnceLock};

use citrus::filter::{FilterRun, Inputs, SvfTables};
use citrus::fx::{Bus, Section, HOST_BLOCK};
use citrus::state as cs;
use citrus::voice::Levels;
use os_dsp::curve::{map_at, map_table, Lfo, Walk, SYNCED};
use os_dsp::num::round;

use crate::fft::{Cpx, Ifft, Plan};
use crate::state::*;

/// The harmonic at the end of the linear EQ scale.
pub const MAX_LINE: usize = 32767;
/// Harmonics a note can have at most: RICH 16's run past line `n`.
pub const MAX_TOP: usize = 61440;
/// Notes kept at most (the oldest goes first beyond it).
pub const MAX_NOTES: usize = 64;
/// The level at 128: the rms of a note at velocity 100 and level 128.
const RMS: f64 = 0.2114;
/// The harmonics above line `n` go on to this many times the key's frequency.
const TOP_U: f64 = 60.0;
/// A note whose line `n` lies above this has no lines above it.
const CUT_HZ: f64 = 19920.0;
/// How long after a note's end its lines take up the release's rates, in seconds.
const RELEASE_LATE: f64 = 0.05;
/// A note below this level has ended.
const SILENCE: f64 = 1e-5;
const LN100: f64 = 4.605_170_185_988_09;

/// `(3^x - 1) / 2`: the master level, and an EQ map's gain and time factor at `x = 2 l`.
#[inline]
pub fn c3(x: f64) -> f64 {
    ((x * 3f64.ln()).exp() - 1.0) * 0.5
}

/// `ln x` of a positive `x`, within about 2e-4. A note start reads this and `fast_exp` for every harmonic.
#[inline(always)]
fn fast_ln(x: f32) -> f32 {
    let bits = x.to_bits();
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000) - 1.0;
    let e = (bits >> 23) as i32 - 127;
    (e as f32 + m + m * (1.0 - m) * (0.439_987 + m * (-0.241_704 + m * 0.082_316))) * std::f32::consts::LN_2
}

/// `e^x`, within about 2e-4.
#[inline(always)]
fn fast_exp(x: f32) -> f32 {
    let y = (x * std::f32::consts::LOG2_E).clamp(-126.0, 127.0);
    let i = y.floor();
    let f = y - i;
    (1.0 + f + f * (1.0 - f) * (-0.304_661 - f * 0.079_252)) * f32::from_bits(((i as i32 + 127) as u32) << 23)
}

/// `C(2 l)` by table: what a map's level does to a harmonic's gain or time.
fn map_gains() -> &'static [f32; 1025] {
    static T: OnceLock<[f32; 1025]> = OnceLock::new();
    T.get_or_init(|| std::array::from_fn(|i| c3(2.0 * i as f64 / 1024.0) as f32))
}

#[inline(always)]
fn map_gain(t: &[f32; 1025], l: f32) -> f32 {
    let p = l.clamp(0.0, 1.0) * 1024.0;
    let i = (p as usize).min(1023);
    t[i] + (t[i + 1] - t[i]) * (p - i as f32)
}

/// Linear interpolation in a table of `(x, y)`, flat outside it.
fn table_at(t: &[(f64, f64)], x: f64) -> f64 {
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

/// The time in seconds a decay or release of `x` (0..1) takes to fall by 40 dB.
pub fn time_of(x: f64) -> f64 {
    const SLOPE: [(f64, f64); 9] =
        [(0.0, 2.7), (0.1, 3.44), (0.2, 4.18), (0.4, 5.935), (0.6, 8.567), (0.8, 12.937), (0.9, 15.944), (0.99, 19.444), (1.0, 19.95)];
    0.05 + x * table_at(&SLOPE, x)
}

/// The factor a harmonic's pattern value takes at fullness `full`.
fn full_factor(full: f64) -> f64 {
    const T: [(f64, f64); 21] = [
        (-1000.0, 19.9),
        (-900.0, 15.6),
        (-800.0, 11.9),
        (-700.0, 8.78),
        (-600.0, 6.13),
        (-500.0, 3.99),
        (-400.0, 2.39),
        (-300.0, 1.47),
        (-200.0, 1.12),
        (-100.0, 1.04),
        (0.0, 1.0),
        (100.0, 0.934),
        (200.0, 0.908),
        (300.0, 0.662),
        (400.0, 0.353),
        (500.0, 0.152),
        (600.0, 0.059),
        (700.0, 0.021),
        (800.0, 0.0077),
        (900.0, 0.0025),
        (1000.0, 0.001),
    ];
    let logs = T.map(|(x, y)| (x, y.ln()));
    table_at(&logs, full).exp()
}

/// The part of a harmonic's fall that low fullness adds with PRE at 0, at pattern value `g`.
fn static_part(g: f64, full: f64) -> f64 {
    const G: [f64; 6] = [0.00264, 0.00459, 0.00816, 0.0103, 0.02368, 0.03529];
    const ROWS: [(f64, [f64; 6]); 4] = [
        (-1000.0, [0.308, 1.456, 7.307, 14.9, 179.0, 592.0]),
        (-800.0, [0.092, 0.291, 0.972, 1.537, 6.383, 10.6]),
        (-600.0, [0.026, 0.059, 0.143, 0.188, 0.58, 0.925]),
        (-400.0, [0.007, 0.013, 0.03, 0.035, 0.089, 0.121]),
    ];
    if g <= 0.0 {
        return 0.0;
    }
    let lg = g.ln();
    let row = |r: &[f64; 6]| -> f64 {
        let i = G.iter().position(|&x| g < x).unwrap_or(G.len()).clamp(1, G.len() - 1);
        let (x0, x1) = (G[i - 1].ln(), G[i].ln());
        let (y0, y1) = (r[i - 1].ln(), r[i].ln());
        y0 + (y1 - y0) * (lg - x0) / (x1 - x0)
    };
    let full = full.clamp(-1000.0, 1000.0);
    if full >= -400.0 {
        // About 1 / 1.56 per 50 above the last row.
        return (row(&ROWS[3].1) - (full + 400.0) / 50.0 * 1.56f64.ln()).exp();
    }
    let i = ROWS.iter().position(|r| full <= r.0).unwrap_or(1).max(1);
    let (a, b) = (&ROWS[i - 1], &ROWS[i]);
    let t = (full - a.0) / (b.0 - a.0);
    (row(&a.1) * (1.0 - t) + row(&b.1) * t).exp()
}

/// How far a forced harmonic rises at map level `l`, relative to a full one.
fn forced_gain(l: f64) -> f64 {
    const T: [(f64, f64); 10] =
        [(0.0, 0.0), (0.002, 0.15), (0.005, 0.55), (0.01, 0.79), (0.02, 0.906), (0.03, 0.943), (0.05, 0.969), (0.07, 0.98), (0.1, 0.99), (1.0, 1.0)];
    table_at(&T, l)
}

/// The share of a cycle that spreads the unison's voices at PHS `x` (0..1, at most 1 with its map) and RICH `r`.
fn phase_law(x: f64, r: i32) -> f64 {
    let h = (101.0 - 101f64.powf(1.0 - x)) / 100.0;
    h.powi(r)
}

/// The share of a pattern below each value, in steps: the measured percentiles (the top, clipped by the measurement,
/// is Lemon Studio's choice).
fn quantile(u: f64) -> f64 {
    const T: [(f64, f64); 9] = [(0.0, 0.00002), (0.05, 0.0003), (0.1, 0.0006), (0.25, 0.0016), (0.5, 0.0052), (0.75, 0.0147), (0.9, 0.0359), (0.99, 0.08), (1.0, 0.12)];
    let logs = T.map(|(x, y)| (x, y.ln()));
    table_at(&logs, u).exp()
}

fn quantile_table() -> &'static [f32; 4096] {
    static T: OnceLock<Box<[f32; 4096]>> = OnceLock::new();
    T.get_or_init(|| Box::new(std::array::from_fn(|i| quantile((i as f64 + 0.5) / 4096.0) as f32)))
}

/// How far the lines of RICH 10 and up droop, in dB, at `u` times the key's frequency.
fn droop_db(u: f64) -> f64 {
    const T: [(f64, f64); 26] = [
        (10.0, 0.0),
        (12.0, -0.17),
        (14.0, -0.29),
        (16.0, -0.34),
        (18.0, -0.45),
        (20.0, -0.76),
        (22.0, -1.26),
        (24.0, -1.88),
        (26.0, -2.56),
        (28.0, -3.44),
        (30.0, -4.65),
        (32.0, -6.2),
        (34.0, -7.95),
        (36.0, -9.91),
        (38.0, -12.25),
        (40.0, -15.05),
        (42.0, -18.35),
        (44.0, -22.08),
        (46.0, -26.26),
        (48.0, -31.16),
        (50.0, -36.95),
        (52.0, -43.84),
        (54.0, -51.98),
        (56.0, -61.88),
        (58.0, -74.75),
        (60.0, -91.07),
    ];
    table_at(&T, u)
}

/// The line that sounds at line `k`: above line `n`, from RICH 10 up, an image of the line as far below the table's
/// middle. The middle itself is empty: line 0.
fn mirror_line(r: i32, k: usize) -> usize {
    let middle = 1usize << (r - 1);
    if r < 10 || k < middle {
        k
    } else if k == middle {
        0
    } else {
        (2 * middle).saturating_sub(k)
    }
}

/// The droop's gain per line of RICH 16; line `k` of RICH `r` reads it at `k << (16 - r)`.
fn droops() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| {
        (0..=MAX_TOP)
            .map(|i| {
                let u = i as f64 / 1024.0;
                if u < 10.0 { 1.0 } else { (droop_db(u) * std::f64::consts::LN_10 * 0.05).exp() as f32 }
            })
            .collect()
    })
}

/// A small generator (SplitMix64) for the patterns and the phases.
#[derive(Debug, Clone, Copy)]
pub struct Mix(pub u64);

impl Mix {
    #[inline]
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// A seed's pattern: per harmonic `k` (index `k`; 0 unused), a value relative to the strongest. The first lines stay
/// the same whatever the length.
pub fn pattern(seed: u64, top: usize, out: &mut Vec<f32>) {
    let q = quantile_table();
    let mut r = Mix(seed.wrapping_mul(0xD134_2543_DE82_EF95).wrapping_add(0x1234_5678));
    out.clear();
    out.reserve(top + 1);
    out.push(0.0);
    for _ in 1..=top {
        out.push(q[(r.next() >> 52) as usize]);
    }
}

/// What "a new random seed per note" reads: one long pattern, each note from an offset of its own.
fn random_pool() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| {
        let mut g = Vec::new();
        pattern(0x5EED_CAFE, 2 * MAX_TOP + 1, &mut g);
        g
    })
}

/// Every harmonic's phase, the same for all notes.
fn phasors() -> &'static [Cpx] {
    static T: OnceLock<Vec<Cpx>> = OnceLock::new();
    T.get_or_init(|| {
        let mut r = Mix(0x0123_4567_89AB_CDEF);
        (0..=MAX_TOP)
            .map(|_| {
                let a = (r.next() >> 11) as f64 / (1u64 << 53) as f64 * std::f64::consts::TAU;
                [a.cos() as f32, a.sin() as f32]
            })
            .collect()
    })
}

/// A harmonic's place on the linear EQ scale: the first line reads the map's left end.
#[inline]
fn linear_x(k: f64) -> f32 {
    ((k - 1.0) / (MAX_LINE + 1) as f64).clamp(0.0, 1.0) as f32
}

/// A harmonic's place on the natural EQ scale.
fn natural_x() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| {
        let mut v = vec![0.0f32; MAX_TOP + 1];
        for (k, x) in v.iter_mut().enumerate().skip(1) {
            *x = ((k as f64).log2() / 15.0).min(1.0) as f32;
        }
        v
    })
}

/// A state made ready to play: its maps as tables, its seeds' patterns, its effect settings.
#[derive(Debug, Clone)]
pub struct Patch {
    pub st: State,
    /// The final maps along their axis, levels as stored: a table's last entry is at the axis's end.
    finals: Arc<[Vec<f32>; FINALS]>,
    /// Mod X and Y tables per articulator (513 entries), `None` when a map is at its default.
    mods: Arc<Vec<[Option<Vec<f32>>; 2]>>,
    /// The seeds' patterns (`None` for "none").
    seeds: [Option<Arc<Vec<f32>>>; 2],
    fx: Arc<cs::State>,
}

/// The gain of one side of the chorus's crossfade at `x`: 1 at `x` = 0, 0 at `x` = 1, on past that.
fn chorus_law(x: f64) -> f64 {
    (5.0 - (x * 5f64.ln()).exp()) * 0.25
}

/// The effect settings in Citrus's terms.
fn fx_state(st: &State) -> cs::State {
    let mut f = cs::State::default();
    let i = &st.ints;
    f.switches[1] = 0;
    for k in 0..7 {
        f.s[16 + k] = i[CHORUS + k];
    }
    // Smith's chorus crossfades: its volume sets the wet path by the law the dry path reads backwards.
    f.s[22] = round(128.0 * chorus_law(1.0 - f64::from(i[CHORUS + 6]) / 128.0)) as i32;
    f.s[24] = 0;
    f.delays[0] = cs::Delay {
        feedback: i[DELAY],
        time: i[DELAY + 1],
        stereo: i[DELAY + 2],
        level: i[DELAY + 3],
        mode: i[DELAY + 4],
        synced: st.delay_tempo,
        serial: false,
        on: st.delay_on,
    };
    f.reverb = cs::ReverbSettings { knobs: std::array::from_fn(|k| i[REVERB + k]), on: st.reverb_on, tempo: st.reverb_tempo };
    let gains = [i[EQ_LEVEL], i[EQ_LEVEL + 1], i[EQ_LEVEL + 2]];
    f.eq = cs::EqSettings {
        mode: if gains.iter().any(|&g| g != 0) { 1 } else { 0 },
        bands: std::array::from_fn(|k| [gains[k], i[EQ_FREQ + k], i[EQ_WIDTH + k], st.eq_types[k]]),
    };
    f
}

impl Patch {
    pub fn new(st: State) -> Patch {
        // One entry per harmonic on the linear scale or for a long map (a resynthesized one); else coarser.
        let finals = Arc::new(std::array::from_fn(|f| {
            let c = &st.finals[f];
            let n = if !st.natural_scale || c.points.len() > 256 { MAX_LINE + 1 } else { 4095 };
            map_table(c, 0.0, 1.0 / n as f64, n)
        }));
        let mods = (0..ARTS)
            .map(|a| {
                let centre = if bipolar(a) { 0.5 } else { 0.0 };
                let def = if bipolar(a) { 0.5 } else { 1.0 };
                let table = |m: usize| {
                    let c = &st.arts[a].maps[m];
                    (!c.points.iter().all(|p| f64::from(p.level) == def)).then(|| map_table(c, centre, 1.0 / 512.0, 512))
                };
                [table(MAP_X), table(MAP_Y)]
            })
            .collect();
        let seeds = [SEED1, SEED2].map(|f| {
            let seed = st.ints[f];
            (seed != 0).then(|| {
                let mut g = Vec::new();
                pattern(seed as u64, MAX_TOP, &mut g);
                Arc::new(g)
            })
        });
        let fx = Arc::new(fx_state(&st));
        Patch { st, finals, mods: Arc::new(mods), seeds, fx }
    }

    /// The default patch, as a new instance plays it.
    pub fn default_patch() -> Patch {
        Patch::new(State::default())
    }

    /// A final map's level at `x` (0..1).
    #[inline]
    fn final_at(&self, f: usize, x: f32) -> f32 {
        let t = &self.finals[f];
        t[((x * (t.len() - 1) as f32 + 0.5) as usize).min(t.len() - 1)]
    }

    /// Where a harmonic reads the final maps.
    #[inline]
    fn line_x(&self, k: f64) -> f32 {
        if k <= 1.0 {
            0.0
        } else if self.st.natural_scale {
            (k.log2() / 15.0).min(1.0) as f32
        } else {
            linear_x(k)
        }
    }
}

/// A note-on from the host.
#[derive(Debug, Clone, Copy)]
pub struct NoteOn {
    pub levels: Levels,
    /// Velocity 0..1 (velocity / 128).
    pub velocity: f32,
}

/// What the rendering shares (one engine's rate and tables).
struct Ctx<'a> {
    patch: &'a Arc<Patch>,
    rate: f64,
    big_b: usize,
    seconds: f64,
    tempo: f64,
    offline: bool,
    kf: f32,
    ramp_a: f32,
    svf: &'a SvfTables,
}

impl Ctx<'_> {
    fn step_of(&self, flags: u8) -> f64 {
        if flags & SYNCED != 0 { self.seconds * self.tempo / 60.0 } else { self.seconds * 2.0 }
    }
}

/// One live articulator.
#[derive(Debug, Clone)]
struct ArtRun {
    env: Walk,
    lfo: Lfo,
    env_on: bool,
    lfo_on: bool,
    bip: bool,
    fixed: f32,
}

impl ArtRun {
    fn new(ctx: &Ctx, patch: &Patch, a: usize, kmap: f64, vel: f64) -> ArtRun {
        let art = &patch.st.arts[a];
        let bip = bipolar(a);
        let centre = if bip { 0.5 } else { 0.0 };
        let kv = map_at(&art.maps[MAP_KEY], kmap, centre);
        let vv = map_at(&art.maps[MAP_VEL], vel, centre);
        let fixed = if bip { kv + vv } else { kv * vv };
        let limit = a != A_PITCH;
        ArtRun {
            env: Walk::new(&art.env, ctx.step_of(art.env.flags), centre, limit),
            lfo: Lfo::new(&art.lfo, ctx.step_of(art.lfo.flags), None),
            env_on: art.env.enabled,
            lfo_on: art.lfo.enabled,
            bip,
            fixed,
        }
    }

    /// The value now; `dt` control blocks later than the last call.
    fn advance(&mut self, patch: &Patch, a: usize, dt: f64, idx: [usize; 2]) -> f32 {
        let art = &patch.st.arts[a];
        let mut v = self.fixed;
        for m in 0..2 {
            if let Some(t) = &patch.mods[a][m] {
                let y = t[idx[m]];
                v = if self.bip { v + y } else { v * y };
            }
        }
        if self.env_on {
            self.env.advance(&art.env, dt);
            let y = self.env.out * art.env_amount;
            v = if self.bip { v + y } else { v * y };
        }
        if self.lfo_on {
            self.lfo.advance(&art.lfo, dt);
            let y = self.lfo.value * art.lfo_amount;
            v = if self.bip { v + y } else { v * (y * 2.0 + 1.0) };
        }
        v
    }
}

/// An articulator's value at note-on, without its envelope or LFO running.
fn note_on_value(ctx: &Ctx, patch: &Patch, a: usize, kmap: f64, vel: f64, idx: [usize; 2]) -> f64 {
    let mut r = ArtRun::new(ctx, patch, a, kmap, vel);
    f64::from(r.advance(patch, a, 0.0, idx))
}

/// One unison voice.
#[derive(Debug, Clone, Copy, Default)]
struct UniVoice {
    pos: f64,
    inc: f64,
    target: f64,
    dinc: f64,
    gl: f32,
    gr: f32,
    /// `sign * |f| * D`, the detune's shape.
    shape: f64,
}

const DETUNE: [f64; 9] = [0.877, 1.069, 0.929, 1.181, 1.031, 1.129, 0.887, 1.117, 0.977];

/// The unison's positions, gains and detune shape.
fn unison(st: &State, table_len: f64, phase: f64) -> Vec<UniVoice> {
    let o = st.ints[U_ORD].clamp(1, 9) as usize;
    let mut out = vec![UniVoice::default(); o];
    if o == 1 {
        out[0].gl = 1.0;
        out[0].gr = 1.0;
        return out;
    }
    let m = f64::from(st.ints[U_VOL]) / 128.0;
    let gain = if o == 2 { 1.0 } else { 1.0 / (o as f64).sqrt() };
    let pan = f64::from(st.ints[U_PAN]) / 128.0;
    for (j, u) in out.iter_mut().enumerate() {
        let f = 2.0 * j as f64 / (o - 1) as f64 - 1.0;
        let p = (1.0 + f * pan) * 0.5;
        let a = std::f64::consts::FRAC_PI_2 * p;
        let v = gain * (m + 2.0 * (1.0 - f.abs()) * (1.0 - m));
        u.gl = (v * a.cos()) as f32;
        u.gr = (v * a.sin()) as f32;
        let mut sign = f.abs();
        if j % 2 == 1 {
            sign = -sign;
        }
        if o % 2 == 1 && 2 * j > o {
            sign = -sign;
        }
        u.shape = sign * DETUNE[j % 9];
        // The voices sit evenly about the one-voice phase, the first (the left one) behind and the last ahead.
        let ahead = (j as f64 - (o - 1) as f64 / 2.0) / o as f64 * phase;
        u.pos = (ahead * table_len).rem_euclid(table_len);
    }
    out
}

/// A table with its guard points: `t[0]` is the last point, `t[1 ..= len]` the wave, then the first three again.
fn guarded(wave: &Ifft, t: &mut Vec<f32>) {
    let len = wave.len();
    t.resize(len + 4, 0.0);
    wave.take(&mut t[1..=len]);
    t[0] = t[len];
    t[len + 1] = t[1];
    t[len + 2] = t[2];
    t[len + 3] = t[3];
}

#[inline(always)]
fn hermite(t: &[f32], i: usize, f: f32) -> f32 {
    let w = &t[i..i + 4];
    let c = (w[2] - w[0]) * 0.5;
    let v = w[1] - w[2];
    let u = c + v;
    let a = u + v + (w[3] - w[1]) * 0.5;
    let b = u + a;
    ((a * f - b) * f + c) * f + w[1]
}

/// The frames' base length at RICH `r`, at 44.1 kHz.
fn frame_base(r: i32, quality: i32) -> f64 {
    let base = match r {
        ..=12 => 1024.0,
        13 => 2048.0,
        _ => 4096.0,
    };
    base * [2.0, 1.0, 0.5, 0.25, 0.125][quality.clamp(0, 4) as usize]
}

/// A note's table length for `top` lines.
fn table_len(top: usize, quality: i32) -> usize {
    let over = [2.0, 4.0, 8.0, 8.0, 16.0][quality.clamp(0, 4) as usize];
    (((top as f64 * over) as usize).max(4096).next_power_of_two()).min(1 << 18)
}

/// A playing note.
struct Note {
    id: u64,
    key: i32,
    patch: Arc<Patch>,
    levels: Levels,
    released: bool,
    /// The release's rates are in use.
    timbre_released: bool,
    finished: bool,
    first: bool,
    block_left: usize,
    /// Control blocks until the release's rates take over; 0 when none is due.
    release_wait: usize,
    arts: Vec<ArtRun>,
    mod_now: [f64; 2],
    // Spectrum.
    r: i32,
    top: usize,
    /// Amplitude per line, with the common decay kept out, this frame's factor per line, the pattern value times the
    /// fullness factor, and (alt decay) what the line settles at.
    a: Vec<f32>,
    fac: Vec<f32>,
    gb: Vec<f32>,
    settle: Vec<f32>,
    power: f64,
    tau_dec: f64,
    tau_rel: f64,
    shift: f64,
    // Decay.
    common_rate: f64,
    common: f64,
    is_static: bool,
    alt_on: bool,
    // Tables.
    plan: Arc<Plan>,
    ifft: Option<Ifft>,
    spec: Vec<Cpx>,
    ta: Vec<f32>,
    tb: Vec<f32>,
    /// The next table's first step applies the frame's factor this many times.
    steps_next: u32,
    /// The first frame reads only `ta`.
    hold: bool,
    frame_blocks: usize,
    frame_left: usize,
    passes_done: usize,
    loaded: bool,
    w: f32,
    dw: f32,
    // Output.
    uni: Vec<UniVoice>,
    gain: f64,
    dgain: f64,
    declick: usize,
    declick_len: usize,
    filters: Option<[FilterRun; 2]>,
    f_key: (i32, i32, bool),
    inputs: Inputs,
    /// The pitch of the fundamental last set.
    f0: f64,
    top_now: usize,
}

impl Note {
    #[allow(clippy::too_many_arguments)]
    fn new(ctx: &Ctx, n: NoteOn, id: u64, previous: Option<f32>, knob_mods: [f64; 2], salt: u64, spare: &mut Spare) -> Note {
        let patch = ctx.patch.clone();
        let st = &patch.st;
        let key = round(f64::from(n.levels.pitch) * 0.01) as i32 + 60;
        let kmap = ((f64::from(n.levels.pitch) * 0.01 + 60.0).max(0.0)) / 120.0;
        let vel = f64::from(n.velocity);
        let idx = mod_index(knob_mods, n.levels);
        let value = |a: usize| note_on_value(ctx, &patch, a, kmap, vel, idx);

        let r = st.ints[RICH].clamp(6, 16);
        let lines_n = (1usize << (r - 1)) - 1;
        let cents = f64::from(n.levels.pitch) + 100.0 * f64::from(st.ints[COARSE]) + f64::from(st.ints[FINE]);
        let f_key = 523.25 * (cents / 1200.0).exp2();
        let f0 = f_key * f64::from(6 - r).exp2();

        // The top line: at Nyquist of the note's pitch, further if an articulator can lower it.
        let pitch_moves = st.arts[A_PITCH].env.enabled || st.arts[A_PITCH].lfo.enabled;
        let upper = if r <= 9 { lines_n } else { ((TOP_U * f64::from(r - 6).exp2()) as usize).min(MAX_TOP) };
        let nyquist = (0.5 * ctx.rate / f0) as usize;
        let cut = if lines_n as f64 * f0 > CUT_HZ { (CUT_HZ / f0) as usize } else { nyquist };
        let reach = if pitch_moves { cut.saturating_mul(4) } else { cut };
        let top = upper.min(reach).max(1);
        // The level counts every line up to `n`, heard or not. A flat impulse map with no comb leaves the spectrum as the
        // seed stage made it. Any shape is normalised over half as many lines again, though those past `n` are not heard
        // as themselves: it reads quieter.
        let shaped = st.combs[M_IMPULSE] != 0 || st.finals[M_IMPULSE].points.windows(2).any(|p| p[0].level != p[1].level);
        let counted = if shaped { (3usize << (r - 2)).min(MAX_LINE) } else { lines_n };
        let calc = top.max(upper).max(counted);

        // The pattern: morphed, rotated and turned into a fall.
        let morph = (f64::from(st.ints[MORPH]) / 1000.0 * value(A_MORPH)).clamp(0.0, 1.0) as f32;
        // With no seed 1, morphing brings in the second pattern alone.
        let seed_on = patch.seeds[0].is_some() || morph > 0.0;
        let full = (f64::from(st.ints[FULL]) + 2000.0 * value(A_FULL)).clamp(-1000.0, 1000.0);
        let b_full = full_factor(full);
        let pre = (f64::from(st.ints[PRE]) * value(A_PRE)).max(0.0);
        let mut g = spare.lines.pop().unwrap_or_default();
        g.clear();
        g.resize(calc + 1, 0.0);
        // What the lines fall by while they play: the two patterns, mixed in log.
        let mut time_g = Vec::new();
        if seed_on {
            let t = morph;
            let g2: &[f32] = match &patch.seeds[1] {
                Some(g2) => g2,
                None if t > 0.0 => {
                    let mut r = Mix(salt.wrapping_add(id).wrapping_mul(0x2545_F491_4F6C_DD1D));
                    &random_pool()[(r.next() % (MAX_TOP as u64 + 1)) as usize..]
                }
                None => &[],
            };
            let shift = round(f64::from(value(A_ROT)) * (lines_n + 1) as f64);
            let wrap = (lines_n + 1).min(MAX_TOP) as i64;
            for k in 1..=calc {
                let at = if shift == 0 { k } else { ((k as i64 - 1 - shift).rem_euclid(wrap) + 1) as usize };
                let g2v = g2.get(at).copied().unwrap_or(0.0);
                g[k] = match &patch.seeds[0] {
                    None => t * g2v,
                    Some(g1) if t <= 0.0 => g1[at],
                    Some(_) if t >= 1.0 => g2v,
                    Some(g1) => 1.0 / ((1.0 - t) / g1[at] + t / g2v),
                };
                if t > 0.0 && t < 1.0 {
                    if let Some(g1) = &patch.seeds[0] {
                        if time_g.is_empty() {
                            time_g.resize(calc + 1, 0.0f32);
                        }
                        time_g[k] = fast_exp((1.0 - t) * fast_ln(g1[at]) + t * fast_ln(g2v));
                    }
                }
            }
        }
        // The fall at note-on, read through a table of the static part.
        let mut lut = [1.0f32; 513];
        if seed_on {
            for (i, v) in lut.iter_mut().enumerate() {
                let lg = -11.0 + 9.0 * i as f64 / 512.0;
                let gv = lg.exp();
                *v = (-(static_part(gv, full) + gv * b_full * pre)).exp() as f32;
            }
        }
        let fall = |gv: f32| -> f32 {
            if gv <= 0.0 {
                return 1.0;
            }
            let p = ((fast_ln(gv) + 11.0) * (512.0 / 9.0)).clamp(0.0, 511.999);
            let i = p as usize;
            lut[i] + (lut[i + 1] - lut[i]) * (p - i as f32)
        };

        // Where each line reads the maps.
        let shift = f64::from(value(A_SHIFT)) * (lines_n + 1) as f64;
        let nat = natural_x();
        let line_at = |patch: &Patch, k: usize| -> f32 {
            if shift == 0.0 {
                if patch.st.natural_scale { nat[k] } else { linear_x(k as f64) }
            } else {
                patch.line_x(k as f64 - shift)
            }
        };

        let [comb_forced, comb_impulse, comb_decay, _] = st.combs;
        let forced_flat = st.finals[M_FORCED].points.iter().all(|p| p.level == 0.0);
        let weigh = st.natural_weight;
        let mut a = spare.lines.pop().unwrap_or_default();
        a.clear();
        a.resize(calc + 1, 0.0);
        let mut settle = Vec::new();
        let alt = st.alt_decay;
        if alt {
            settle.resize(calc + 1, 0.0);
        }
        let gains = map_gains();
        // The seed stage alone, for the level without auto loudness.
        let mut seed_power = 0.0f64;
        for k in 1..=calc {
            let mut v = fall(g[k]);
            if !forced_flat && comb_gate(comb_forced, k) {
                let l = f64::from(patch.final_at(M_FORCED, line_at(&patch, k)));
                v += (1.0 - v) * forced_gain(l) as f32;
            }
            if k <= lines_n {
                seed_power += f64::from(v) * f64::from(v);
            }
            let w = if weigh { 1.0 / (k as f32).sqrt() } else { 1.0 };
            let x = line_at(&patch, k);
            if alt {
                let c = if comb_gate(comb_decay, k) { map_gain(gains, patch.final_at(M_DECAY, x)) } else { 1.0 };
                settle[k] = v * w * c;
            }
            // A comb on the impulse map silences the lines it leaves out.
            a[k] = if comb_gate(comb_impulse, k) { v * w * map_gain(gains, patch.final_at(M_IMPULSE, x)) } else { 0.0 };
        }
        let powers = |v: &[f32], last: usize| -> f64 { v.iter().take(last + 1).skip(1).map(|&y| f64::from(y) * f64::from(y)).sum::<f64>() * 0.5 };
        let scale = if st.auto_loudness {
            let p = powers(&a, counted).max(if alt { powers(&settle, counted) } else { 0.0 });
            if p > 0.0 { RMS / p.sqrt() } else { 1.0 }
        } else {
            let p = seed_power * 0.5;
            if p > 0.0 { 2.0 * RMS / p.sqrt() } else { 1.0 }
        };
        // Past line `n` what sounds is an image of the lines below it, drooping.
        for k in lines_n + 1..=calc {
            let m = mirror_line(r, k);
            a[k] = a.get(m).copied().unwrap_or(0.0);
            g[k] = g.get(m).copied().unwrap_or(0.0);
            if !time_g.is_empty() {
                time_g[k] = time_g.get(m).copied().unwrap_or(0.0);
            }
            if alt {
                settle[k] = settle.get(m).copied().unwrap_or(0.0);
            }
        }
        let droops = droops();
        let droop = |k: usize| -> f32 { if r < 10 { 1.0 } else { droops[(k << (16 - r)).min(MAX_TOP)] } };
        let mut power = 0.0f64;
        for k in 1..=top {
            let d = droop(k) * scale as f32;
            a[k] *= d;
            power += f64::from(a[k]) * f64::from(a[k]);
            if alt {
                settle[k] *= d;
            }
        }
        a.truncate(top + 1);
        g.truncate(top + 1);
        settle.truncate(top + 1);
        // Keep the fall's rate term.
        if !time_g.is_empty() {
            g.copy_from_slice(&time_g[..=top]);
        }
        for v in g.iter_mut() {
            *v *= b_full as f32;
        }
        if !seed_on {
            g.iter_mut().for_each(|v| *v = 0.0);
        }

        // Decay and release times, with their maps.
        let time = |field: usize, art: usize| -> f64 {
            let x = (f64::from(st.ints[field]) / 10000.0).clamp(0.0, 1.0);
            let t0 = time_of(x);
            0.05 + (t0 - 0.05) * value(art).max(0.0)
        };
        let tau_dec = if st.ints[DEC] >= 10000 { f64::INFINITY } else { time(DEC, A_DEC) / LN100 };
        let tau_rel = time(REL, A_REL) / LN100;

        // The live articulators.
        let mut arts: Vec<ArtRun> = (0..=A_U_PITCH).map(|i| ArtRun::new(ctx, &patch, i, kmap, vel)).collect();
        if st.porta && st.arts[A_PITCH].env.enabled && st.arts[A_PITCH].env.decay >= 0 {
            if let Some(prev) = previous {
                let mut d = f64::from(prev) - f64::from(n.levels.pitch);
                match st.porta_limit {
                    1 => {
                        while d.abs() > 1250.0 {
                            d -= 1200.0 * d.signum();
                        }
                    }
                    k @ 2..=5 => {
                        let lim = f64::from(6 - i32::from(k)) * 100.0;
                        d = d.clamp(-lim, lim);
                    }
                    _ => {}
                }
                let run = &mut arts[A_PITCH];
                let c = &st.arts[A_PITCH].env;
                if st.porta_variable {
                    run.env.scales[0] *= (d.abs() * (1.0 / 1200.0)) as f32;
                }
                run.env.reset(c, (d / 2400.0) as f32);
            }
        }

        // The first table.
        let quality = if ctx.offline { 4 } else { st.quality };
        let len = table_len(top, quality);
        let plan = plan_for(len);
        let mut ifft = match spare.iffts.iter().position(|f| f.len() == len) {
            Some(i) => spare.iffts.swap_remove(i),
            None => Ifft::new(plan.clone()),
        };
        let mut spec = spare.specs.pop().unwrap_or_default();
        spec.clear();
        spec.resize(top + 1, [0.0f32; 2]);
        let top_now = fill_spec(&mut spec, &a, top, nyquist);
        ifft.load(&spec[..=top_now]);
        ifft.finish();
        let mut ta = spare.tables.pop().unwrap_or_default();
        guarded(&ifft, &mut ta);

        // The unison.
        let phs = (f64::from(st.ints[U_PHS]) / 128.0 * value(A_U_PHASE)).clamp(0.0, 1.0);
        let uni = unison(st, len as f64, phase_law(phs, r));
        let declick_len = if st.declick { (0.002 * ctx.rate) as usize } else { 0 };

        let fac = spare.lines.pop().unwrap_or_default();
        let mut note = Note {
            id,
            key,
            levels: n.levels,
            released: false,
            timbre_released: false,
            finished: false,
            first: true,
            block_left: 0,
            release_wait: 0,
            arts,
            mod_now: knob_mods,
            r,
            top,
            a,
            fac,
            gb: g,
            settle,
            power: power * 0.5,
            tau_dec,
            tau_rel,
            shift,
            common_rate: 0.0,
            common: 1.0,
            is_static: true,
            alt_on: alt,
            plan,
            ifft: Some(ifft),
            spec,
            ta,
            tb: spare.tables.pop().map(|mut t| {
                t.clear();
                t
            }).unwrap_or_default(),
            steps_next: 2,
            hold: true,
            frame_blocks: 1,
            frame_left: 0,
            passes_done: 0,
            loaded: false,
            w: 0.0,
            dw: 0.0,
            uni,
            gain: 0.0,
            dgain: 0.0,
            declick: 0,
            declick_len,
            filters: None,
            f_key: (-1, -1, false),
            inputs: Inputs::default(),
            f0,
            top_now,
            patch,
        };
        note.set_rates(ctx, false);
        note
    }

    /// Works out the rates of the phase: what is common goes to the gain, the rest to the per-line factors.
    fn set_rates(&mut self, ctx: &Ctx, release: bool) {
        let tau = if release { self.tau_rel } else { self.tau_dec };
        let which = if release { M_RELEASE } else { M_DECAY };
        self.alt_on = !release && self.patch.st.alt_decay;
        let quality = if ctx.offline { 4 } else { self.patch.st.quality };
        let base = (frame_base(self.r, quality) * ctx.rate / 44100.0).max(256.0);
        if !tau.is_finite() {
            self.common_rate = 0.0;
            self.is_static = true;
            return;
        }
        let top = self.top;
        self.fac.clear();
        self.fac.resize(top + 1, 1.0);
        let rtime = 200.0 / tau;
        let eq_flat = self.patch.st.finals[which].points.iter().all(|p| p.level == 0.5);
        let nat = natural_x();
        let (natural, shift) = (self.patch.st.natural_scale, self.shift);
        let comb = self.patch.st.combs[which];
        let mut rates = std::mem::take(&mut self.fac);
        let peak = self.a.iter().fold(0.0f32, |m, &v| m.max(v));
        let floor = peak * 1e-4;
        let mut low = f64::INFINITY;
        let gains = map_gains();
        for k in 1..=top {
            // A comb on the decay or release map leaves the lines it skips on the plain time.
            let j = mirror_line(self.r, k);
            let inv = if eq_flat || !comb_gate(comb, j) {
                1.0 / tau
            } else {
                let x = if shift == 0.0 {
                    if natural { nat[j] } else { linear_x(j as f64) }
                } else {
                    self.patch.line_x(j as f64 - shift)
                };
                let c = f64::from(map_gain(gains, self.patch.final_at(which, x)));
                let t = (tau * c + 0.005 * (1.0 - c).max(0.0)).max(1e-4);
                1.0 / t
            };
            let v = inv + f64::from(self.gb[k]) * rtime;
            rates[k] = v as f32;
            if self.a[k] >= floor {
                low = low.min(v);
            }
        }
        if !low.is_finite() {
            low = 1.0 / tau;
        }
        self.common_rate = if self.alt_on { 0.0 } else { low };
        // The energy-weighted extra rate sets the frame.
        let (mut sum_e, mut sum_p) = (0.0f64, 0.0f64);
        let mut any = self.alt_on;
        for k in 1..=top {
            let e = (f64::from(rates[k]) - low).max(0.0);
            rates[k] = e as f32;
            let p = f64::from(self.a[k]) * f64::from(self.a[k]);
            sum_e += p * e;
            sum_p += p;
            if e * base / ctx.rate > 1e-5 && self.a[k] >= floor {
                any = true;
            }
        }
        let mean = if sum_p > 0.0 { sum_e / sum_p } else { 0.0 };
        let mut frame = base;
        if mean > 0.0 {
            frame = frame.min(0.5 * ctx.rate / mean);
        }
        let pitch_moves = self.patch.st.arts[A_PITCH].env.enabled || self.patch.st.arts[A_PITCH].lfo.enabled;
        if pitch_moves {
            frame = frame.min(1024.0 * ctx.rate / 44100.0);
        }
        if self.alt_on {
            frame = frame.min((tau * 0.25 * ctx.rate).max(256.0));
        }
        let frame = frame.max(256.0);
        let blocks = ((frame / ctx.big_b as f64).round() as usize).max(1);
        let span = (blocks * ctx.big_b) as f64 / ctx.rate;
        if self.alt_on {
            let f = (-span / tau).exp() as f32;
            rates.iter_mut().for_each(|v| *v = f);
        } else {
            for v in rates.iter_mut() {
                *v = (-f64::from(*v) * span).exp() as f32;
            }
        }
        self.fac = rates;
        let was_static = self.is_static;
        // Once fading, a note keeps fading (the fade in progress must end).
        self.is_static = !any && was_static;
        self.frame_blocks = blocks;
        if self.is_static {
            return;
        }
        if was_static || self.tb.is_empty() {
            // Starts moving: this frame holds the table in use, the next one reaches for two frames on.
            self.hold = true;
            self.steps_next = 2;
            self.frame_left = blocks;
            self.loaded = false;
            self.passes_done = 0;
        } else if self.frame_left > blocks {
            self.frame_left = blocks;
        }
    }

    /// Runs a share of the next table's work, finishing the table at the frame's end.
    fn table_work(&mut self, ctx: &Ctx) {
        if self.is_static {
            return;
        }
        if !self.loaded {
            let steps = self.steps_next;
            self.steps_next = 1;
            self.step_lines(steps);
            let nyquist = (0.5 * ctx.rate / (self.f0 * self.widest())) as usize;
            let top = self.top;
            self.top_now = fill_spec(&mut self.spec, &self.a, top, nyquist);
            let ifft = self.ifft.get_or_insert_with(|| Ifft::new(self.plan.clone()));
            ifft.load(&self.spec[..=self.top_now]);
            self.loaded = true;
            self.passes_done = 0;
        }
        let Some(ifft) = &mut self.ifft else { return };
        let total = ifft.passes();
        let share = self.frame_blocks.saturating_sub(self.frame_left) + 1;
        let due = (total * share).div_ceil(self.frame_blocks).min(total);
        while self.passes_done < due && !ifft.done() {
            ifft.pass();
            self.passes_done += 1;
        }
    }

    /// One step of the lines' decay; `power` follows.
    fn step_lines(&mut self, steps: u32) {
        let mut sum = 0.0f64;
        if self.alt_on {
            for _ in 0..steps {
                for k in 1..=self.top {
                    let f = self.fac[k];
                    self.a[k] = self.settle[k] + (self.a[k] - self.settle[k]) * f;
                }
            }
            for k in 1..=self.top {
                sum += f64::from(self.a[k]) * f64::from(self.a[k]);
            }
        } else {
            let p = steps as i32;
            for k in 1..=self.top {
                let f = if p == 1 { self.fac[k] } else { self.fac[k].powi(p) };
                let v = self.a[k] * f;
                self.a[k] = v;
                sum += f64::from(v) * f64::from(v);
            }
        }
        self.power = sum * 0.5;
    }

    /// The widest detune now: for the Nyquist cut.
    fn widest(&self) -> f64 {
        let top = self.uni.iter().map(|u| u.target).fold(0.0, f64::max);
        let low = self.uni.iter().map(|u| u.target).fold(f64::INFINITY, f64::min);
        if low > 0.0 && low.is_finite() { (top / low).sqrt().max(1.0) } else { 1.0 }
    }

    /// The control block's work.
    fn control(&mut self, ctx: &Ctx) {
        let live = &ctx.patch.st;
        let dt = if self.first { 0.0 } else { 1.0 };
        // Mod X and Y.
        let knobs = [f64::from(live.ints[MOD_X]), f64::from(live.ints[MOD_Y])];
        for m in 0..2 {
            self.mod_now[m] = if live.smooth {
                let c = (0.01f64.ln() * ctx.big_b as f64 / 96.0 * 44100.0 / ctx.rate).exp();
                knobs[m] + (self.mod_now[m] - knobs[m]) * c
            } else {
                knobs[m]
            };
        }
        let idx = mod_index(self.mod_now, self.levels);
        let mut values = [0.0f32; 5];
        let patch = self.patch.clone();
        for (a, run) in self.arts.iter_mut().enumerate() {
            values[a] = run.advance(&patch, a, dt, idx);
        }
        if self.release_wait > 0 {
            self.release_wait -= 1;
            if self.release_wait == 0 {
                self.set_rates(ctx, true);
            }
        }
        // Link timbre release to volume decay.
        if live.link_release && !self.timbre_released && self.arts[A_VOLUME].env_on && self.arts[A_VOLUME].env.passed_decay {
            self.timbre_released = true;
            self.release_wait = ((RELEASE_LATE * ctx.rate / ctx.big_b as f64).round() as usize).max(1);
        }
        // Pitch.
        let cents = f64::from(self.levels.pitch) + 100.0 * f64::from(live.ints[COARSE]) + f64::from(live.ints[FINE]) + f64::from(values[A_PITCH]) * 4800.0;
        let f0 = 523.25 * (cents / 1200.0).exp2() * f64::from(6 - self.r).exp2();
        self.f0 = f0;
        let len = self.plan.len as f64;
        let base = f0 * len / ctx.rate;
        let range = ((f64::from(live.ints[U_PITCH]) * f64::from(values[A_U_PITCH].max(0.0)) / 256.0 * 11f64.ln()).exp() - 1.0) * 5.0;
        let b = ctx.big_b as f64;
        for u in self.uni.iter_mut() {
            let target = (base * (u.shape * range / 1200.0).exp2()).min(len * 0.9);
            u.target = target;
            if self.first {
                u.inc = target;
                u.dinc = 0.0;
            } else {
                u.dinc = (target - u.inc) / b;
            }
        }
        // The gain.
        self.common *= (-self.common_rate * ctx.seconds).exp();
        let level = c3(f64::from(live.ints[LEVEL]) / 128.0);
        let target = level * f64::from(self.levels.vol) * f64::from(values[A_VOLUME]).max(0.0) * self.common;
        if self.first {
            self.gain = target;
            self.dgain = 0.0;
        } else {
            self.dgain = (target - self.gain) / b;
        }
        // The filter.
        let fk = (live.ints[F_TYPE].clamp(0, 3), live.ints[F_MODE].clamp(0, 3), live.ints[FLAT] != 0);
        if fk != self.f_key {
            self.f_key = fk;
            self.filters = (fk.1 != 0).then(|| {
                let mut h = [0i32; 10];
                h[0] = fk.1;
                let kind = [3u8, 4, 5, 6][fk.0 as usize];
                let make = || FilterRun::new(h, kind, false, fk.2, ctx.rate as u32, 1.0);
                [make(), make()]
            });
        }
        self.inputs = Inputs {
            c: live.ints[CUT] as f32 / 256.0 + values[A_CUT],
            r: live.ints[RES] as f32 / 256.0 + values[A_RES],
            volume: 1.0,
            ..Inputs::default()
        };
        // The tables.
        if !self.is_static {
            if self.frame_left == 0 {
                self.frame_left = self.frame_blocks;
                self.next_frame();
            }
            self.table_work(ctx);
            self.dw = if self.hold { 0.0 } else { (1.0 - self.w) / (self.frame_left * ctx.big_b) as f32 };
            self.frame_left -= 1;
        } else {
            self.dw = 0.0;
        }
        // The end: the volume envelope after the release, or silence.
        let vol_done = self.arts[A_VOLUME].env_on && self.arts[A_VOLUME].env.finished();
        let quiet = target.abs() * self.power.sqrt() < SILENCE && !self.first;
        if (self.released && vol_done) || (quiet && (self.released || self.common < 1e-3 || self.power < 1e-12)) {
            self.finished = true;
        }
        self.first = false;
    }

    /// A frame ends: the wave being built becomes the fade's target.
    fn next_frame(&mut self) {
        // Reach this only at a frame's end, with the fade done.
        let Some(ifft) = &mut self.ifft else { return };
        ifft.finish();
        if self.hold {
            // The held table stays the source; the new one is the first target.
            guarded(ifft, &mut self.tb);
            self.hold = false;
        } else {
            std::mem::swap(&mut self.ta, &mut self.tb);
            guarded(ifft, &mut self.tb);
        }
        self.w = 0.0;
        self.loaded = false;
        self.passes_done = 0;
    }

    /// Sums `m` frames into `l` and `r` (which hold 0s), gain and filter applied.
    fn mix(&mut self, ctx: &Ctx, l: &mut [f32], r: &mut [f32]) {
        let m = l.len();
        let len = self.plan.len as f64;
        let fade = !self.hold && !self.tb.is_empty() && !self.is_static;
        let single = self.uni.len() == 1;
        let (w0, dw) = (self.w, self.dw);
        let (ta, tb) = (&self.ta[..], &self.tb[..]);
        for u in self.uni.iter_mut() {
            let (mut pos, mut inc) = (u.pos, u.inc);
            let dinc = u.dinc;
            let (gl, gr) = (u.gl, u.gr);
            let mut w = w0;
            if fade {
                for j in 0..m {
                    let i = pos as usize;
                    let f = (pos - i as f64) as f32;
                    let a = hermite(ta, i, f);
                    let b = hermite(tb, i, f);
                    let v = a + (b - a) * w;
                    l[j] += v * gl;
                    r[j] += v * gr;
                    pos += inc;
                    if pos >= len {
                        pos -= len;
                    }
                    inc += dinc;
                    w += dw;
                }
            } else if single {
                for x in l.iter_mut() {
                    let i = pos as usize;
                    let f = (pos - i as f64) as f32;
                    *x += hermite(ta, i, f);
                    pos += inc;
                    if pos >= len {
                        pos -= len;
                    }
                    inc += dinc;
                }
                r.copy_from_slice(l);
            } else {
                for j in 0..m {
                    let i = pos as usize;
                    let f = (pos - i as f64) as f32;
                    let v = hermite(ta, i, f);
                    l[j] += v * gl;
                    r[j] += v * gr;
                    pos += inc;
                    if pos >= len {
                        pos -= len;
                    }
                    inc += dinc;
                }
            }
            u.pos = pos;
            u.inc = inc;
        }
        self.w += dw * m as f32;
        // The gain, with the fade-in.
        let (mut g, dg) = (self.gain, self.dgain);
        for j in 0..m {
            let mut v = g as f32;
            if self.declick < self.declick_len {
                v *= self.declick as f32 / self.declick_len as f32;
                self.declick += 1;
            }
            l[j] *= v;
            r[j] *= v;
            g += dg;
        }
        self.gain = g;
        // The filter.
        if let Some(f) = &mut self.filters {
            f[0].run(l, self.inputs, ctx.kf, ctx.ramp_a, ctx.svf, None, false);
            f[1].run(r, self.inputs, ctx.kf, ctx.ramp_a, ctx.svf, None, false);
        }
    }

    fn release(&mut self, ctx: &Ctx) {
        if self.released {
            return;
        }
        self.released = true;
        for (a, run) in self.arts.iter_mut().enumerate() {
            let art = &self.patch.st.arts[a];
            run.env.release(&art.env);
            run.lfo.walk.release(&art.lfo);
        }
        // The lines keep their rates a moment after the note ends.
        if !self.timbre_released {
            self.timbre_released = true;
            self.release_wait = ((RELEASE_LATE * ctx.rate / ctx.big_b as f64).round() as usize).max(1);
        }
    }
}

/// Fills `spec` from the amplitudes, zero above `nyquist`; returns the top line in use.
fn fill_spec(spec: &mut [Cpx], a: &[f32], top: usize, nyquist: usize) -> usize {
    let h = phasors();
    let used = top.min(nyquist).max(1);
    for k in 1..=used {
        let v = 0.5 * a[k];
        spec[k] = [v * h[k][0], v * h[k][1]];
    }
    used
}

/// The mod tables' index for a note.
fn mod_index(knobs: [f64; 2], l: Levels) -> [usize; 2] {
    [
        round(knobs[0] + f64::from(l.mod_x) * 256.0).clamp(0, 512) as usize,
        round(knobs[1] + f64::from(l.mod_y) * 256.0).clamp(0, 512) as usize,
    ]
}

/// The FFT plan of a table length (`table_len`'s 2^12 to 2^18), all made when the first instance is: a note starts on
/// the audio thread.
fn plan_for(len: usize) -> Arc<Plan> {
    static T: OnceLock<Vec<Arc<Plan>>> = OnceLock::new();
    let plans = T.get_or_init(|| (12..=18).map(|b| Plan::new(1 << b)).collect());
    plans[(len.trailing_zeros() as usize).clamp(12, 18) - 12].clone()
}

/// What finished notes leave for the next ones, so a note start rarely asks the system for memory.
#[derive(Default)]
struct Spare {
    lines: Vec<Vec<f32>>,
    tables: Vec<Vec<f32>>,
    specs: Vec<Vec<Cpx>>,
    iffts: Vec<Ifft>,
}

impl Spare {
    fn keep(&mut self, note: Note) {
        self.lines.extend([note.a, note.fac, note.gb]);
        self.lines.truncate(12);
        self.tables.extend([note.ta, note.tb]);
        self.tables.truncate(8);
        self.specs.push(note.spec);
        self.specs.truncate(4);
        self.iffts.extend(note.ifft);
        self.iffts.truncate(4);
    }
}

/// A note-on waiting for its turn to start.
struct Waiting {
    n: NoteOn,
    id: u64,
    released: bool,
}

/// How many lines the audio thread takes on starting per second: half its time, at about 40 ns per line. A note always
/// fits when none has started for a while; the others of a big chord at high RICH wait a block or so rather than miss
/// the block's deadline.
const START_LINES_PER_SECOND: f64 = 12.5e6;

/// What starting a note of RICH `r` takes, in lines at most.
fn start_cost(r: i32) -> f64 {
    let r = r.clamp(6, 16);
    let lines = if r <= 9 { (1usize << (r - 1)) - 1 } else { ((TOP_U * f64::from(r - 6).exp2()) as usize).min(MAX_TOP) };
    lines.max(1024) as f64
}

/// The synthesizer as a host sees it: a patch, notes in and out, and render calls of stereo.
pub struct Smith {
    rate: f64,
    offline: bool,
    tempo: f64,
    patch: Arc<Patch>,
    big_b: usize,
    seconds: f64,
    kf: f32,
    ramp_a: f32,
    svf: SvfTables,
    section: Section,
    fx_used: bool,
    notes: Vec<Note>,
    next_id: u64,
    previous: Option<f32>,
    salt: u64,
    spare: Spare,
    waiting: Vec<Waiting>,
    start_credit: f64,
    dry: Bus,
    fx: Bus,
    nl: Vec<f32>,
    nr: Vec<f32>,
}

/// Whether the effects run at all.
fn fx_used(f: &cs::State) -> bool {
    f.s[16] != 0 || f.delays[0].on || f.reverb.on || f.eq.mode != 0
}

impl Smith {
    pub fn new(rate: u32) -> Smith {
        let rate = rate.max(1);
        let patch = Arc::new(Patch::default_patch());
        let section = Section::new(&patch.fx, rate, 0, 120.0);
        let used = fx_used(&patch.fx);
        let mut s = Smith {
            rate: f64::from(rate),
            offline: false,
            tempo: 120.0,
            patch,
            big_b: 96,
            seconds: 96.0 / 44100.0,
            kf: 1.0,
            ramp_a: 0.01,
            svf: SvfTables::new(1.0),
            section,
            fx_used: used,
            notes: Vec::new(),
            next_id: 1,
            previous: None,
            salt: 0x5EED,
            spare: Spare::default(),
            waiting: Vec::new(),
            start_credit: MAX_TOP as f64,
            dry: [vec![0.0; HOST_BLOCK], vec![0.0; HOST_BLOCK]],
            fx: [vec![0.0; HOST_BLOCK], vec![0.0; HOST_BLOCK]],
            nl: vec![0.0; HOST_BLOCK],
            nr: vec![0.0; HOST_BLOCK],
        };
        let _ = (phasors(), natural_x(), quantile_table(), plan_for(1 << 12), map_gains(), droops(), random_pool());
        s.setup_rate();
        s
    }

    fn setup_rate(&mut self) {
        let rate = self.rate;
        self.big_b = (round(96.0 * rate / 44100.0 / 4.0) * 4).max(4) as usize;
        self.seconds = self.big_b as f64 / rate;
        let k44 = 44100.0 / rate;
        self.kf = k44 as f32;
        self.ramp_a = (f64::from(0.001f32) * k44 * 10.0) as f32;
        self.svf = SvfTables::new(self.kf);
        self.section = Section::new(&self.patch.fx, rate as u32, 0, self.tempo as f32);
    }

    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// Loads a state (event 213's bytes); stops what plays. `false` when it is no state.
    pub fn load_state(&mut self, bytes: &[u8]) -> bool {
        match State::from_bytes(bytes) {
            Some(st) => {
                self.set_patch(Patch::new(st));
                true
            }
            None => false,
        }
    }

    /// Plays `p` from now on; stops what plays.
    pub fn set_patch(&mut self, p: Patch) {
        self.notes.clear();
        self.waiting.clear();
        self.patch = Arc::new(p);
        self.fx_used = fx_used(&self.patch.fx);
        self.section = Section::new(&self.patch.fx, self.rate as u32, 0, self.tempo as f32);
    }

    /// Plays an edited patch: the notes keep playing as they began; live controls follow at once. The effects
    /// section is rebuilt only when an effects setting changes.
    pub fn edit(&mut self, p: Patch) {
        let same = fx_same(&self.patch.fx, &p.fx);
        self.patch = Arc::new(p);
        if !same {
            self.fx_used = fx_used(&self.patch.fx);
            self.section = Section::new(&self.patch.fx, self.rate as u32, 0, self.tempo as f32);
        }
    }

    pub fn set_rate(&mut self, rate: u32) {
        let rate = f64::from(rate.max(1));
        if rate != self.rate {
            self.rate = rate;
            self.notes.clear();
            self.waiting.clear();
            self.setup_rate();
        }
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        if (bpm - self.tempo).abs() > 1e-3 && bpm > 0.0 {
            self.tempo = bpm;
            if self.fx_used {
                self.section = Section::new(&self.patch.fx, self.rate as u32, 0, self.tempo as f32);
            }
        }
    }

    /// Rendering offline uses the top quality.
    pub fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    #[allow(clippy::too_many_arguments)]
    fn ctx<'a>(patch: &'a Arc<Patch>, svf: &'a SvfTables, rate: f64, big_b: usize, seconds: f64, tempo: f64, offline: bool, kf: f32, ramp_a: f32) -> Ctx<'a> {
        Ctx { patch, rate, big_b, seconds, tempo, offline, kf, ramp_a, svf }
    }

    /// Starts a note, now or when the audio thread has the time; returns its id.
    pub fn note_on(&mut self, n: NoteOn) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        if self.waiting.is_empty() && self.can_start() {
            self.start_note(n, id);
        } else {
            self.waiting.push(Waiting { n, id, released: false });
        }
        id
    }

    /// Whether a note may start now; takes its cost if so.
    fn can_start(&mut self) -> bool {
        if self.offline {
            return true;
        }
        let cost = start_cost(self.patch.st.ints[RICH]);
        let can = self.start_credit >= cost;
        if can {
            self.start_credit -= cost;
        }
        can
    }

    /// Before a render: the time of `n` samples for starting notes, and the notes it covers.
    fn start_waiting(&mut self, n: usize) {
        let lines = n as f64 * START_LINES_PER_SECOND / self.rate;
        self.start_credit = (self.start_credit + lines).min(lines.max(MAX_TOP as f64));
        while !self.waiting.is_empty() && self.can_start() {
            let w = self.waiting.remove(0);
            self.start_note(w.n, w.id);
            if w.released {
                self.note_off(w.id);
            }
        }
    }

    fn start_note(&mut self, n: NoteOn, id: u64) {
        let ctx = Self::ctx(&self.patch, &self.svf, self.rate, self.big_b, self.seconds, self.tempo, self.offline, self.kf, self.ramp_a);
        let knobs = [f64::from(self.patch.st.ints[MOD_X]), f64::from(self.patch.st.ints[MOD_Y])];
        let note = Note::new(&ctx, n, id, self.previous, knobs, self.salt, &mut self.spare);
        if self.patch.st.one_voice {
            for other in self.notes.iter_mut().filter(|o| o.key == note.key && !o.released) {
                other.release(&ctx);
            }
        }
        self.previous = Some(n.levels.pitch);
        self.section.started_send = true;
        self.notes.push(note);
        if self.notes.len() > MAX_NOTES {
            self.notes.remove(0);
        }
    }

    pub fn note_off(&mut self, id: u64) {
        for w in self.waiting.iter_mut().filter(|w| w.id == id) {
            w.released = true;
        }
        let ctx = Self::ctx(&self.patch, &self.svf, self.rate, self.big_b, self.seconds, self.tempo, self.offline, self.kf, self.ramp_a);
        for n in self.notes.iter_mut().filter(|n| n.id == id) {
            n.release(&ctx);
        }
    }

    pub fn all_off(&mut self) {
        self.waiting.iter_mut().for_each(|w| w.released = true);
        let ctx = Self::ctx(&self.patch, &self.svf, self.rate, self.big_b, self.seconds, self.tempo, self.offline, self.kf, self.ramp_a);
        for n in self.notes.iter_mut() {
            n.release(&ctx);
        }
    }

    /// Stops everything at once.
    pub fn panic(&mut self) {
        self.notes.clear();
        self.waiting.clear();
    }

    /// Stops everything, tails included.
    pub fn reset(&mut self) {
        self.notes.clear();
        self.waiting.clear();
        self.previous = None;
        self.section = Section::new(&self.patch.fx, self.rate as u32, 0, self.tempo as f32);
    }

    pub fn note_pan(&mut self, id: u64, pan: f32) {
        for w in self.waiting.iter_mut().filter(|w| w.id == id) {
            w.n.levels.pan = pan;
        }
        for n in self.notes.iter_mut().filter(|n| n.id == id) {
            n.levels.pan = pan;
        }
    }

    /// A note's mod X (`k` 0) or mod Y (1), -1..1.
    pub fn note_mod(&mut self, id: u64, k: usize, value: f32) {
        for w in self.waiting.iter_mut().filter(|w| w.id == id) {
            if k == 0 {
                w.n.levels.mod_x = value;
            } else {
                w.n.levels.mod_y = value;
            }
        }
        for n in self.notes.iter_mut().filter(|n| n.id == id) {
            if k == 0 {
                n.levels.mod_x = value;
            } else {
                n.levels.mod_y = value;
            }
        }
    }

    /// A note's pitch in cents from key 60.
    pub fn bend(&mut self, id: u64, pitch: f32) {
        for w in self.waiting.iter_mut().filter(|w| w.id == id) {
            w.n.levels.pitch = pitch;
        }
        for n in self.notes.iter_mut().filter(|n| n.id == id) {
            n.levels.pitch = pitch;
        }
    }

    pub fn voices(&self) -> usize {
        self.notes.len() + self.waiting.len()
    }

    /// Renders into `left` and `right` (overwritten).
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        self.start_waiting(n);
        let mut at = 0;
        while at < n {
            let m = (n - at).min(HOST_BLOCK);
            self.render_block(&mut left[at..at + m], &mut right[at..at + m]);
            at += m;
        }
    }

    fn render_block(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len();
        left.fill(0.0);
        right.fill(0.0);
        let alive = !self.notes.is_empty();
        let mut nl = std::mem::take(&mut self.nl);
        let mut nr = std::mem::take(&mut self.nr);
        {
            let ctx = Self::ctx(&self.patch, &self.svf, self.rate, self.big_b, self.seconds, self.tempo, self.offline, self.kf, self.ramp_a);
            for note in self.notes.iter_mut() {
                let mut done = 0;
                nl[..n].fill(0.0);
                nr[..n].fill(0.0);
                while done < n && !note.finished {
                    if note.block_left == 0 {
                        note.control(&ctx);
                        note.block_left = ctx.big_b;
                        if note.finished {
                            break;
                        }
                    }
                    let m = note.block_left.min(n - done);
                    note.mix(&ctx, &mut nl[done..done + m], &mut nr[done..done + m]);
                    note.block_left -= m;
                    done += m;
                }
                // The note's pan, unity at the centre.
                let pan = note.levels.pan;
                if pan == 0.0 {
                    for j in 0..n {
                        left[j] += nl[j];
                        right[j] += nr[j];
                    }
                } else {
                    let (gl, gr) = os_dsp::level::pan_split(std::f32::consts::SQRT_2, pan);
                    for j in 0..n {
                        left[j] += nl[j] * gl;
                        right[j] += nr[j] * gr;
                    }
                }
            }
        }
        self.nl = nl;
        self.nr = nr;
        for i in (0..self.notes.len()).rev() {
            if self.notes[i].finished {
                let note = self.notes.remove(i);
                self.spare.keep(note);
            }
        }
        if self.fx_used {
            let (dry, fx) = (&mut self.dry, &mut self.fx);
            for ch in 0..2 {
                dry[ch].clear();
                fx[ch].clear();
            }
            let keep = if self.patch.st.ints[CHORUS] > 0 { chorus_law(f64::from(self.patch.st.ints[CHORUS + 6]) / 128.0) as f32 } else { 1.0 };
            dry[0].extend(left.iter().map(|v| v * keep));
            dry[1].extend(right.iter().map(|v| v * keep));
            fx[0].extend_from_slice(left);
            fx[1].extend_from_slice(right);
            self.section.process(dry, fx, n, alive);
            left.copy_from_slice(&dry[0][..n]);
            right.copy_from_slice(&dry[1][..n]);
        }
    }
}

fn fx_same(a: &cs::State, b: &cs::State) -> bool {
    a.s[16..=24] == b.s[16..=24] && a.delays == b.delays && a.reverb == b.reverb && a.eq == b.eq
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(key: i32, velocity: f32) -> NoteOn {
        NoteOn { levels: Levels { vol: 1.0, pan: 0.0, pitch: (key - 60) as f32 * 100.0, mod_x: 0.0, mod_y: 0.0 }, velocity }
    }

    /// A chord that would not fit in a block at RICH 16 starts over blocks, a note let go while it waits still sounds
    /// and ends, and offline every note starts at once.
    #[test]
    fn a_big_chord_starts_over_blocks() {
        let mut st = State::default();
        st.ints[RICH] = 16;
        let mut s = Smith::new(48000);
        s.set_patch(Patch::new(st.clone()));
        let ids: Vec<u64> = (0..3).map(|k| s.note_on(note(60 + k, 0.8))).collect();
        assert_eq!((s.notes.len(), s.voices()), (1, 3));
        s.note_off(ids[2]);
        let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
        s.render(&mut l, &mut r);
        assert_eq!(s.notes.len(), 2);
        s.render(&mut l, &mut r);
        assert_eq!((s.notes.len(), s.voices()), (3, 3));
        assert_eq!(s.notes.iter().map(|n| n.released).collect::<Vec<_>>(), [false, false, true]);
        assert!(s.notes.iter().map(|n| n.id).eq(ids.iter().copied()));

        let mut s = Smith::new(48000);
        s.set_offline(true);
        s.set_patch(Patch::new(st));
        for k in 0..3 {
            s.note_on(note(60 + k, 0.8));
        }
        assert_eq!(s.notes.len(), 3);
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn render(s: &mut Smith, n: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        let mut at = 0;
        while at < n {
            let m = (n - at).min(256);
            s.render(&mut l[at..at + m], &mut r[at..at + m]);
            at += m;
        }
        (l, r)
    }

    /// A flat patch: no seed, no decay, level 128.
    fn flat() -> State {
        let mut st = State::default();
        st.ints[SEED1] = 0;
        st.ints[MORPH] = 0;
        st.ints[DEC] = 10000;
        st.ints[LEVEL] = 128;
        st.ints[U_ORD] = 1;
        st.ints[RICH] = 8;
        st.arts[A_VOLUME].env.enabled = false;
        st
    }

    #[test]
    fn a_flat_note_has_the_measured_level() {
        let mut s = Smith::new(44100);
        s.set_patch(Patch::new(flat()));
        s.note_on(note(60, 0.78));
        let (l, r) = render(&mut s, 44100);
        assert_eq!(l, r, "no pan with one voice");
        let v = rms(&l[4410..]);
        assert!((v - RMS).abs() < 0.004, "{v}");
    }

    #[test]
    fn the_master_level_follows_its_law() {
        assert!((c3(1.0) - 1.0).abs() < 1e-12);
        assert!((c3(0.25) * RMS - 0.0334).abs() < 0.001);
        assert!((c3(1.25) * RMS - 0.3116).abs() < 0.002);
        assert!((c3(2.0) * RMS - 0.8454).abs() < 0.002);
    }

    #[test]
    fn times_match_the_table() {
        for (x, t) in [(0.0, 0.05), (0.1, 0.394), (0.2, 0.886), (0.4, 2.424), (0.6, 5.19), (0.8, 10.4), (0.9, 14.4), (0.99, 19.3), (1.0, 20.0)] {
            assert!((time_of(x) - t).abs() < 0.01, "{x}: {} vs {t}", time_of(x));
        }
    }

    #[test]
    fn a_decay_falls_40_db_in_its_time() {
        let mut st = flat();
        st.ints[DEC] = 2000;
        let mut s = Smith::new(44100);
        s.set_patch(Patch::new(st));
        s.note_on(note(60, 0.78));
        let (l, _) = render(&mut s, 66150);
        let at = |t: f64| rms(&l[(t * 44100.0) as usize..][..2205]);
        let drop = (at(0.1) / at(0.1 + 0.886)).log10() * 20.0;
        assert!((drop - 40.0).abs() < 1.5, "{drop}");
    }

    #[test]
    fn lines_with_their_own_decay_fall_apart() {
        let mut st = flat();
        st.ints[DEC] = 4000;
        st.ints[RICH] = 6;
        // The decay EQ shortens the high lines' times.
        st.finals[M_DECAY].points[1].level = 0.0;
        st.finals[M_DECAY].keep_sytrus();
        let mut s = Smith::new(44100);
        s.set_patch(Patch::new(st));
        s.note_on(note(48, 0.78));
        let (l, _) = render(&mut s, 44100 * 2);
        let amp = |t0: usize, f: f64| -> f64 {
            let n = 8820;
            let (mut c, mut d) = (0.0, 0.0);
            for j in 0..n {
                let a = std::f64::consts::TAU * f * j as f64 / 44100.0;
                c += f64::from(l[t0 + j]) * a.cos();
                d += f64::from(l[t0 + j]) * a.sin();
            }
            2.0 * (c * c + d * d).sqrt() / n as f64
        };
        let f = 261.625;
        let ratio = |t| amp(t, f * 8.0) / amp(t, f);
        assert!(ratio(44100) < ratio(4410) * 0.7, "{} then {}", ratio(4410), ratio(44100));
    }

    /// The size of the partial at `f` in the 0.2 s from sample `t0`.
    fn partial(x: &[f32], t0: usize, f: f64) -> f64 {
        let n = 8820;
        let (mut c, mut d) = (0.0, 0.0);
        for j in 0..n {
            let a = std::f64::consts::TAU * f * j as f64 / 44100.0;
            c += f64::from(x[t0 + j]) * a.cos();
            d += f64::from(x[t0 + j]) * a.sin();
        }
        2.0 * (c * c + d * d).sqrt() / n as f64
    }

    fn set_flat_map(st: &mut State, m: usize, level: f32) {
        st.finals[m].points.iter_mut().for_each(|p| p.level = level);
        st.finals[m].keep_sytrus();
    }

    #[test]
    fn a_comb_on_the_impulse_map_silences_what_it_skips() {
        for (comb, on) in [(1, [true, false, true, false]), (2, [true, true, false, true]), (5, [true, false, false, true])] {
            let mut st = flat();
            st.ints[RICH] = 6;
            st.combs[M_IMPULSE] = comb;
            let mut s = Smith::new(44100);
            s.set_patch(Patch::new(st));
            s.note_on(note(60, 0.78));
            let (l, _) = render(&mut s, 22050);
            for (i, &v) in on.iter().enumerate() {
                let a = partial(&l, 2205, 523.25 * (i + 1) as f64);
                assert_eq!(a > 1e-3, v, "comb {comb} line {} reads {a}", i + 1);
            }
        }
    }

    #[test]
    fn a_comb_on_a_time_map_leaves_what_it_skips_on_the_plain_time() {
        for m in [M_DECAY, M_RELEASE] {
            let mut st = flat();
            st.ints[RICH] = 6;
            st.ints[DEC] = if m == M_DECAY { 6000 } else { 9000 };
            st.ints[REL] = 6000;
            set_flat_map(&mut st, m, 0.125);
            // "-2": the odd lines take the map.
            st.combs[m] = 1;
            let mut s = Smith::new(44100);
            s.set_patch(Patch::new(st));
            let id = s.note_on(note(60, 0.78));
            if m == M_RELEASE {
                s.note_off(id);
            }
            let (l, _) = render(&mut s, 44100);
            let (one, two) = (partial(&l, 30870, 523.25), partial(&l, 30870, 1046.5));
            assert!(two > one * 20.0, "map {m}: line 1 {one}, line 2 {two}");
        }
    }

    #[test]
    fn a_shaped_impulse_map_counts_half_as_many_lines_again() {
        let level = |r: i32, shape: bool, comb: i32| -> f64 {
            let mut st = flat();
            st.ints[RICH] = r;
            st.combs[M_IMPULSE] = comb;
            if shape {
                st.finals[M_IMPULSE].points[1].level = 0.5005;
                st.finals[M_IMPULSE].keep_sytrus();
            }
            let mut s = Smith::new(44100);
            s.set_patch(Patch::new(st));
            s.note_on(note(60, 0.78));
            rms(&render(&mut s, 4410).0)
        };
        for r in [8, 10, 12] {
            let plain = level(r, false, 0);
            for got in [level(r, true, 0), level(r, false, 1)] {
                let db = 20.0 * (got / plain).log10();
                assert!((db + 1.76).abs() < 0.3, "r{r}: {db}");
            }
        }
        // At RICH 16 line 1.5 n is past the last line a map has.
        let db = 20.0 * (level(16, true, 0) / level(16, false, 0)).log10();
        assert!(db.abs() < 0.3, "r16: {db}");
    }

    #[test]
    fn lines_past_n_are_images() {
        assert_eq!(mirror_line(9, 400), 400);
        assert_eq!(mirror_line(10, 511), 511);
        assert_eq!(mirror_line(10, 512), 0);
        assert_eq!(mirror_line(10, 513), 511);
        assert_eq!(mirror_line(16, 61440), 4096);
    }

    #[test]
    fn notes_end_after_their_release() {
        let mut st = flat();
        st.ints[DEC] = 4000;
        st.ints[REL] = 0;
        let mut s = Smith::new(44100);
        s.set_patch(Patch::new(st));
        let id = s.note_on(note(60, 0.78));
        render(&mut s, 4410);
        s.note_off(id);
        render(&mut s, 44100);
        assert_eq!(s.voices(), 0);
    }

    #[test]
    fn two_unison_voices_pan_apart() {
        let mut st = flat();
        st.ints[U_ORD] = 2;
        st.ints[U_PITCH] = 0;
        st.ints[U_PHS] = 0;
        let mut s = Smith::new(44100);
        s.set_patch(Patch::new(st));
        s.note_on(note(60, 0.78));
        let (l, r) = render(&mut s, 44100);
        assert!((rms(&l[4410..]) - RMS).abs() < 0.004 && (rms(&r[4410..]) - RMS).abs() < 0.004);
    }

    #[test]
    fn the_unison_spread_follows_phs_and_rich() {
        assert_eq!(phase_law(0.0, 9), 0.0);
        assert!((phase_law(1.0, 9) - 1.0).abs() < 1e-12);
        // Read from Ogun: PHS 64 at r 9 and r 11.
        assert!((phase_law(0.5, 9) - 0.42582).abs() < 1e-4);
        assert!((phase_law(0.5, 11) - 0.35224).abs() < 1e-4);
    }

    #[test]
    fn the_first_line_reads_the_left_end_of_the_linear_scale() {
        assert_eq!(linear_x(1.0), 0.0);
        assert!((linear_x(4.0) - 3.0 / 32768.0).abs() < 1e-9);
    }

    #[test]
    fn a_seed_keeps_its_first_lines() {
        let (mut a, mut b) = (Vec::new(), Vec::new());
        pattern(7, 100, &mut a);
        pattern(7, 2000, &mut b);
        assert_eq!(a[1..=100], b[1..=100]);
    }

    #[test]
    fn a_seeds_values_have_the_measured_spread() {
        let mut g = Vec::new();
        pattern(3, MAX_LINE, &mut g);
        let mut v = g[1..].to_vec();
        v.sort_by(f32::total_cmp);
        let q = |p: f64| f64::from(v[(p * v.len() as f64) as usize]);
        assert!((q(0.5) / 0.0052 - 1.0).abs() < 0.1, "{}", q(0.5));
        assert!((q(0.9) / 0.0359 - 1.0).abs() < 0.1, "{}", q(0.9));
    }

    /// Below line `n`, every default-patch note plays, stops and leaves nothing behind.
    #[test]
    fn the_default_patch_plays() {
        let mut s = Smith::new(44100);
        let id = s.note_on(note(60, 0.78));
        let (l, _) = render(&mut s, 22050);
        assert!(l.iter().all(|v| v.is_finite()));
        assert!(rms(&l) > 0.01);
        s.note_off(id);
        render(&mut s, 44100 * 3);
        assert_eq!(s.voices(), 0);
    }
}
