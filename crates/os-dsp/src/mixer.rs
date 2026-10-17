// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A mixer track's fader matrix and its three-band equaliser.
//!
// spec: audio/mixer.md ("Fader matrix", "Equaliser", "Sends")

use crate::glide::Ramp;
use crate::level::{PanLaw, R12800, R6400, R65536, curve, knob, pan_split_law, unity_at_centre};

/// Stored defaults of the three equaliser bands: gain (0.01 dB), frequency and width positions (0..65536).
pub const EQ_DEFAULT: [[i32; 3]; 3] = [[0, 5777, 17500], [0, 33145, 17500], [0, 55825, 17500]];

/// The fader matrix of a track as four singles `[LL, RL, RR, LR]`: `left' = LL * left + RL * right`,
/// `right' = RR * right + LR * left`. `volume` 0..16000 (12800 = unity), `pan` -6400..6400, `separation` -64..64
/// (positive merges towards mono, negative widens). A track that is switched off, or at volume 0, has all four at 0.
/// Centred without separation `LL = RR = vol`; a panned track attenuates one side by the constant-power law and
/// feeds half of what that side lost across, so a hard-panned track still carries both inputs. This is the Circular
/// panning law, the default: [`fader_matrix_law`].
pub fn fader_matrix(volume: i32, pan: i32, separation: i32, on: bool) -> [f32; 4] {
    fader_matrix_law(volume, pan, separation, on, PanLaw::Circular)
}

/// [`fader_matrix`] under the project's panning law (spec: audio/gain-laws.md "Panning law: Triangular"): the side a
/// track pans away from is attenuated by the law's split, and half of what it lost crosses over as before. Triangular
/// keeps the louder side at `vol`, so a track panned right by `q` gives `L = (1 - q) L_in` and
/// `R = (1 - q/2) R_in + (q/2) L_in` (left mirrors it).
pub fn fader_matrix_law(volume: i32, pan: i32, separation: i32, on: bool, law: PanLaw) -> [f32; 4] {
    if !on || volume <= 0 {
        return [0.0; 4];
    }
    let vol = curve(knob(volume, R12800));
    let p = knob(pan, R6400);
    let (g0, g1) = pan_split_law(unity_at_centre(vol), p, law);
    let (mut xl, mut xr) = (0.0f64, 0.0f64);
    if p > 0.0 {
        xr = (1.0 - f64::from(g0) / f64::from(vol)) * 0.5;
    } else if p < 0.0 {
        xl = (1.0 - f64::from(g1) / f64::from(vol)) * 0.5;
    }
    if separation != 0 {
        let sep = f64::from(separation) / 64.0;
        let add = if separation > 0 { (5f64.powf(sep) - 1.0) * 0.25 } else { sep };
        xl = (2.0 * xl + add).min(1.0) * 0.5;
        xr = (2.0 * xr + add).min(1.0) * 0.5;
    }
    let (g0, g1) = (f64::from(g0), f64::from(g1));
    [(g0 * (1.0 - xl)) as f32, (g0 * xl) as f32, (g1 * (1.0 - xr)) as f32, (g1 * xr) as f32]
}

/// A send's gain from its stored level (0..16000, 12800 = unity): the level curve.
pub fn send_gain(level: i32) -> f32 {
    curve(knob(level, R12800))
}

/// A send's gain from an automation clip's value `v` (0..1): `curve(single(v * 1.25))` (spec: audio/mixer.md "Changes
/// while the song plays").
pub fn send_gain_auto(v: f64) -> f32 {
    curve((v * 1.25) as f32)
}

/// The dry level of an effect slot at stored mix level `mix` (0..12800): `single(12800 - mix) * single(1 / 12800)`,
/// 0 at full mix and 1 at mix 0 (spec: audio/mixer.md "An effect slot").
pub fn dry_level(mix: i32) -> f32 {
    knob(12800 - mix, R12800)
}

/// Mixes one tick of an effect slot: `wet` (the effect's output, interleaved stereo) becomes the slot's output, with
/// `dry` its input and `ramp` the tick's plan of the dry level (spec: audio/mixer.md "An effect slot", "A slot's mix
/// level while it moves"). Without a step `out = single(single(wet * w) + single(dry * d))` with `w = single(1 - d)`,
/// and a dry level of 0 leaves the effect's output as it is. With a step the dry side is weighted by two interleaved
/// running sums of singles and the wet side by a running sum of doubles.
pub fn slot_blend(wet: &mut [f32], dry: &[f32], ramp: Ramp) {
    let d = ramp.first();
    let step = ramp.step();
    if step == 0.0 {
        if d == 0.0 {
            return;
        }
        let w = 1.0 - d;
        for (o, &x) in wet.iter_mut().zip(dry) {
            *o = *o * w + x * d;
        }
        return;
    }
    let n = wet.len().min(dry.len()) / 2;
    let paired = n - n % 8;
    let twice = step + step;
    let (mut even, mut odd) = (d, d + step);
    let mut weight = f64::from(1.0 - d);
    let fall = f64::from(-step);
    for k in 0..n {
        let g = if k < paired {
            let series = if k % 2 == 0 { &mut even } else { &mut odd };
            let g = *series;
            *series += twice;
            g
        } else {
            // The last n mod 8 samples carry on the even series, a step at a time.
            let g = even;
            even += step;
            g
        };
        for c in 0..2 {
            let i = 2 * k + c;
            wet[i] = (f64::from(dry[i] * g) + f64::from(wet[i]) * weight) as f32;
        }
        weight += fall;
    }
}

/// Samples an equaliser band keeps running at 0 dB before its memory is emptied: `rate div 5`.
fn idle_count(rate: u32) -> i64 {
    i64::from(rate / 5)
}

/// One biquad band: its stored settings, its coefficients and its memory (last two inputs and outputs per side, as
/// doubles), and whether it runs.
#[derive(Debug, Clone, Copy)]
struct Band {
    /// Gain (0.01 dB), frequency and width positions as stored.
    settings: [i32; 3],
    /// b0, b1, b2, a1, a2.
    coef: [f64; 5],
    /// Per side: x1, x2, y1, y2.
    state: [[f64; 4]; 2],
    running: bool,
    /// The settings changed since the coefficients were worked out.
    dirty: bool,
    /// Samples left before a band at gain 0 is emptied and goes idle.
    count: Option<i64>,
}

impl Band {
    /// `y = x b0 + (((x1 b1 - y2 a2) + x2 b2) - y1 a1)`, summed in exactly this order (spec: "Equaliser").
    #[inline]
    fn run(&mut self, side: usize, x: f32) -> f32 {
        let [b0, b1, b2, a1, a2] = self.coef;
        let s = &mut self.state[side];
        let x = f64::from(x);
        let y = x * b0 + (((s[0] * b1 - s[3] * a2) + s[1] * b2) - s[2] * a1);
        *s = [x, s[0], y, s[2]];
        y as f32
    }
}

/// The track's equaliser: band 1 a low shelf, band 2 a peaking filter, band 3 a high shelf, in series. At the start a
/// band whose gain is 0 is idle with an empty memory. Values that arrive while the song plays replace a band's
/// coefficients at the next tick and keep its memory; a band brought to gain 0 runs on at 0 dB for `rate div 5`
/// samples (counted in whole ticks), then is emptied and idle (spec: audio/mixer.md "Equaliser", "Changes while the
/// song plays").
#[derive(Debug, Clone)]
pub struct TrackEq {
    bands: [Band; 3],
    rate: u32,
}

impl Default for TrackEq {
    fn default() -> Self {
        TrackEq::new(EQ_DEFAULT, 44100)
    }
}

impl TrackEq { // This code is a part of L e m o n S t u d i o.
    /// The equaliser for stored band settings `[gain, frequency, width]` (gain -1800..1800 in 0.01 dB, frequency and
    /// width 0..65536) at `rate`.
    pub fn new(settings: [[i32; 3]; 3], rate: u32) -> Self {
        let band = |k: usize| {
            let s = settings[k];
            Band { settings: s, coef: design(k, s, rate), state: [[0.0; 4]; 2], running: s[0] != 0, dirty: false, count: None }
        };
        TrackEq { bands: [band(0), band(1), band(2)], rate }
    }

    /// True when no band runs (the equaliser passes its input unchanged).
    pub fn is_flat(&self) -> bool {
        self.bands.iter().all(|b| !b.running)
    }

    /// The stored settings of band `band` (0..3): gain, frequency, width.
    pub fn settings(&self, band: usize) -> [i32; 3] {
        self.bands[band.min(2)].settings
    }

    /// A value for setting `field` (0 gain, 1 frequency, 2 width) of band `band` that arrives at a tick start. A band
    /// whose gain is not 0 afterwards runs on from the memory it has; one whose gain is 0 starts its count afresh and
    /// runs at 0 dB until it ends.
    pub fn set(&mut self, band: usize, field: usize, value: i32) {
        let (Some(b), true) = (self.bands.get_mut(band), field < 3) else { return };
        b.settings[field] = value;
        b.dirty = true;
        b.running = true;
        b.count = if b.settings[0] == 0 { Some(idle_count(self.rate)) } else { None };
    }

    /// Processes one tick of interleaved stereo in place; each band hands the next its output rounded to single. After
    /// the tick a running band's memory values below 2^-24 are set to 0, and a counting band's count goes down by the
    /// tick's length.
    pub fn process(&mut self, buf: &mut [f32]) {
        let n = (buf.len() / 2) as i64;
        let rate = self.rate;
        for (k, band) in self.bands.iter_mut().enumerate() {
            if !band.running {
                continue;
            }
            if band.dirty {
                band.coef = design(k, band.settings, rate);
                band.dirty = false;
            }
            for f in buf.chunks_exact_mut(2) {
                f[0] = band.run(0, f[0]);
                f[1] = band.run(1, f[1]);
            }
            for v in band.state.iter_mut().flatten() {
                if v.abs() < 1.0 / 16_777_216.0 {
                    *v = 0.0;
                }
            }
            if let Some(c) = &mut band.count {
                *c -= n;
                if *c < 1 {
                    band.state = [[0.0; 4]; 2];
                    band.running = false;
                    band.count = None;
                }
            }
        }
    }
}

/// The frequency in Hz of a stored band frequency position (0..65536): 10 Hz to 16000 Hz (spec: audio/mixer.md
/// "Equaliser").
pub fn eq_frequency(freq: i32) -> f32 {
    let x = knob(freq, R65536);
    (((f64::from(x) * 101f64.ln()).exp() - 1.0) as f32 as f64 * 159.9 + 10.0) as f32
}

/// The equaliser's gain in dB at `hz` for stored settings at `rate`: the bands that run (gain not 0), in series. For
/// drawing its response.
pub fn eq_response_db(settings: [[i32; 3]; 3], rate: u32, hz: f64) -> f64 {
    let w = hz * 2.0 * std::f64::consts::PI / f64::from(rate);
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let mut db = 0.0;
    for (k, &s) in settings.iter().enumerate().filter(|(_, s)| s[0] != 0) {
        let [b0, b1, b2, a1, a2] = design(k, s, rate);
        let num = (b0 + b1 * c1 + b2 * c2).powi(2) + (b1 * s1 + b2 * s2).powi(2);
        let den = (1.0 + a1 * c1 + a2 * c2).powi(2) + (a1 * s1 + a2 * s2).powi(2);
        db += 10.0 * (num / den).log10();
    }
    db
}

/// Coefficients `[b0, b1, b2, a1, a2]` of band `k` (0 = low shelf, 1 = peaking, 2 = high shelf) for stored settings
/// `[gain, frequency, width]`.
fn design(k: usize, [gain, freq, width]: [i32; 3], rate: u32) -> [f64; 5] {
    // Parameters scaled in single precision.
    let db = gain as f32 * 0.01f32;
    let f = eq_frequency(freq).min((f64::from(rate) * 0.4999) as f32);
    let q = knob(width, R65536);
    // Then in doubles.
    let w = f64::from(f) * 2.0 * std::f64::consts::PI / f64::from(rate);
    let (sn, cs) = (w.sin(), w.cos());
    let a = 10f64.powf(f64::from(db) / 40.0);
    let (b0, b1, b2, a1, a2) = if k == 1 {
        let qq = f64::from((((f64::from(q) * 1001f64.ln()).exp()) - 1.0) as f32) * 0.0999 + 0.1;
        let alpha = sn / (2.0 * qq);
        let n = 1.0 / (alpha / a + 1.0);
        (n * (1.0 + alpha * a), -2.0 * n * cs, n * (1.0 - alpha * a), -2.0 * n * cs, n * (1.0 - alpha / a))
    } else {
        let s = f64::from(q) * 1.9 + 0.1;
        let beta = ((a * a + 1.0) / s - (a - 1.0) * (a - 1.0)).sqrt() * sn;
        let d = (a - 1.0) * cs;
        if k == 0 {
            let n = 1.0 / (a + 1.0 + d + beta);
            (
                n * a * ((a + 1.0) - d + beta),
                2.0 * n * a * ((a - 1.0) - (a + 1.0) * cs),
                n * a * ((a + 1.0) - d - beta),
                -2.0 * n * ((a - 1.0) + (a + 1.0) * cs),
                n * ((a + 1.0 + d) - beta),
            )
        } else {
            let n = 1.0 / ((a + 1.0) - d + beta);
            (
                n * a * (a + 1.0 + d + beta),
                -2.0 * n * a * ((a - 1.0) + (a + 1.0) * cs),
                n * a * ((a + 1.0 + d) - beta),
                2.0 * n * ((a - 1.0) - (a + 1.0) * cs),
                n * (((a + 1.0) - d) - beta),
            )
        }
    };
    [b0, b1, b2, a1, a2]
}

#[cfg(test)]
mod tests { // This code is a part of L e m o n S t u d i o.
    use super::*;

    /// The default bands sit at 90 Hz, 1500 Hz and 8000 Hz (spec: ui/mixer.md §8); the knob spans 10 Hz .. 16 kHz.
    #[test]
    fn eq_frequency_law() {
        let hz: Vec<f32> = EQ_DEFAULT.iter().map(|b| eq_frequency(b[1]).round()).collect();
        assert_eq!(hz, [90.0, 1500.0, 8000.0]);
        assert_eq!(eq_frequency(0), 10.0);
        assert!((eq_frequency(65536) - 16000.0).abs() < 1.0);
    }

    /// Flat at zero gain; a band's gain shows at its frequency and fades far from it.
    #[test]
    fn eq_response_follows_the_bands() {
        for hz in [20.0, 1000.0, 15000.0] {
            assert_eq!(eq_response_db(EQ_DEFAULT, 44100, hz), 0.0);
        }
        let mut s = EQ_DEFAULT;
        s[1][0] = 900;
        let at = |hz: f64| eq_response_db(s, 44100, hz);
        assert!((at(1500.0) - 9.0).abs() < 0.05, "{}", at(1500.0));
        assert!(at(30.0).abs() < 0.5 && at(16000.0).abs() < 0.5, "{} {}", at(30.0), at(16000.0));
        s[1][0] = 0;
        s[0][0] = -1200;
        assert!((eq_response_db(s, 44100, 15.0) + 12.0).abs() < 0.3, "low shelf: {}", eq_response_db(s, 44100, 15.0));
    }

    /// Centred at unity the matrix is the identity; the pan law matches the measured mixer pan (at -0.5:
    /// 1.0068 / 0.2997 / 0.5412, spec: audio/gain-laws.md).
    #[test]
    fn fader_matrix_points() {
        assert_eq!(fader_matrix(12800, 0, 0, true), [1.0, 0.0, 1.0, 0.0]);
        let [ll, rl, rr, lr] = fader_matrix(12800, -3200, 0, true);
        assert!((ll - 1.0068).abs() < 1e-4 && (rl - 0.2997).abs() < 1e-4 && (rr - 0.5412).abs() < 1e-4 && lr == 0.0);
        assert_eq!(fader_matrix(12800, 0, 0, false), [0.0; 4]);
        assert_eq!(fader_matrix(0, 0, 0, true), [0.0; 4]);
        // 16000 is curve(1.25).
        assert!((fader_matrix(16000, 0, 0, true)[0] - 1.90327).abs() < 1e-5);
    }

    /// spec: audio/gain-laws.md "Panning law: Triangular", the mixer table (to 4 decimals): `[L_in->L, R_in->L,
    /// L_in->R, R_in->R]` at right 0.5, left 0.5, right 1, left 1; centred the same as Circular, which is
    /// `fader_matrix`.
    #[test]
    fn fader_matrix_triangular() {
        let tri = |pan: i32| {
            let [ll, rl, rr, lr] = fader_matrix_law(12800, pan, 0, true, PanLaw::Triangular);
            [ll, rl, lr, rr]
        };
        let table = [
            (3200, [0.5001, -0.0001, 0.2501, 0.7499]),
            (-3200, [0.7501, 0.2499, 0.0001, 0.4999]),
            (6400, [0.0, 0.0, 0.5, 0.5]),
            (-6400, [0.5, 0.5, 0.0, 0.0]),
        ];
        for (pan, want) in table {
            let got = tri(pan);
            assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 2e-4), "{pan}: {got:?}");
        }
        assert_eq!(tri(0), [1.0, 0.0, 0.0, 1.0]);
        for (vol, pan, sep) in [(12800, 0, 0), (10000, -3200, 0), (16000, 1600, 20), (12800, 6400, -30)] {
            assert_eq!(fader_matrix_law(vol, pan, sep, true, PanLaw::Circular), fader_matrix(vol, pan, sep, true));
        }
        assert_eq!(fader_matrix_law(10000, 0, 0, true, PanLaw::Triangular), fader_matrix(10000, 0, 0, true), "the same centre");
    }

    /// Positive separation merges (mono at 64), negative widens (the side doubled at -64).
    #[test]
    fn separation_merges_and_widens() {
        let [ll, rl, rr, lr] = fader_matrix(12800, 0, 64, true);
        assert!((ll - 0.5).abs() < 1e-6 && (rl - 0.5).abs() < 1e-6 && (rr - 0.5).abs() < 1e-6 && (lr - 0.5).abs() < 1e-6);
        let [ll, rl, ..] = fader_matrix(12800, 0, -64, true);
        assert!((ll - 1.5).abs() < 1e-6 && (rl + 0.5).abs() < 1e-6);
        // Halfway merged follows 5^sep.
        let [_, rl, ..] = fader_matrix(12800, 0, 32, true);
        assert!((f64::from(rl) - (5f64.sqrt() - 1.0) * 0.25 * 0.5).abs() < 1e-6);
    }

    #[test]
    fn sends_use_the_level_curve() {
        assert_eq!(send_gain(12800), 1.0);
        assert_eq!(send_gain(0), 0.0);
    }

    /// A flat equaliser passes everything; a boosted peaking band at its centre frequency gains about its boost.
    #[test]
    fn equaliser_bands() {
        let mut flat = TrackEq::new(EQ_DEFAULT, 44100);
        assert!(flat.is_flat());
        let mut x = vec![0.25f32; 64];
        flat.process(&mut x);
        assert!(x.iter().all(|&v| v == 0.25));
        // +12 dB on band 2 at its default frequency: measure the gain on a sine there.
        let mut s = EQ_DEFAULT;
        s[1][0] = 1200;
        let mut eq = TrackEq::new(s, 44100);
        assert!(!eq.is_flat());
        let f = f64::from(((((33145.0f32 / 65536.0) as f64 * 101f64.ln()).exp() - 1.0) as f32 as f64 * 159.9 + 10.0) as f32);
        let n = 44100;
        let mut buf: Vec<f32> = (0..n).flat_map(|i| {
            let v = (2.0 * std::f64::consts::PI * f * i as f64 / 44100.0).sin() as f32 * 0.1;
            [v, v]
        }).collect();
        eq.process(&mut buf);
        let peak = buf[n..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((20.0 * (peak / 0.1).log10() - 12.0).abs() < 0.2, "{} dB at {f} Hz", 20.0 * (peak / 0.1).log10());
    }

    // spec: audio/mixer.md "Changes while the song plays" (the equaliser)
    #[test]
    fn equaliser_changes_keep_the_memory_and_idle_bands_empty_after_their_count() {
        let tick = |eq: &mut TrackEq, n: usize| {
            let mut b = vec![0.5f32; 2 * n];
            eq.process(&mut b);
            b
        };
        // A band raised during the song runs from an empty memory; a new frequency keeps the memory.
        let mut eq = TrackEq::new(EQ_DEFAULT, 44100);
        eq.set(1, 0, 600);
        assert!(!eq.is_flat());
        tick(&mut eq, 100);
        let memory = eq.bands[1].state;
        assert!(memory[0][0] != 0.0);
        eq.set(1, 1, 20000);
        assert_eq!(eq.bands[1].state, memory, "coefficients change, memory stays");
        tick(&mut eq, 100);
        assert_eq!(eq.settings(1), [600, 20000, 17500]);
        // Back to 0 dB: it runs on for rate div 5 = 8820 samples counted in whole ticks: at 149 samples a tick the
        // 60th tick empties it, a return after 59 finds the memory.
        eq.set(1, 0, 0);
        for _ in 0..59 {
            tick(&mut eq, 149);
        }
        assert!(eq.bands[1].running && eq.bands[1].state[0][0] != 0.0);
        tick(&mut eq, 149);
        assert!(!eq.bands[1].running && eq.bands[1].state == [[0.0; 4]; 2] && eq.is_flat());
        // A width value for an idle band at 0 dB starts the count afresh: it runs (passing the signal) for a while.
        eq.set(1, 2, 30000);
        assert!(eq.bands[1].running);
        let out = tick(&mut eq, 64);
        assert!(out.iter().all(|&v| (v - 0.5).abs() < 1e-6));
    }

    // spec: audio/mixer.md "An effect slot", "A slot's mix level while it moves"
    #[test]
    fn slot_mix_blends_and_ramps() {
        use crate::glide::Glide;
        assert_eq!(dry_level(12800), 0.0);
        assert_eq!(dry_level(0), 1.0);
        assert_eq!(dry_level(6400), 0.5);
        let dry = vec![1.0f32; 2 * 20];
        // Full mix, not moving: the effect's output as it is.
        let mut wet = vec![0.25f32; 40];
        slot_blend(&mut wet, &dry, Glide::new(0.0).block(0.0, 20, 0.001));
        assert!(wet.iter().all(|&v| v == 0.25));
        // Half: single(wet * w) + single(dry * d).
        let mut wet = vec![0.25f32; 40];
        slot_blend(&mut wet, &dry, Glide::new(0.5).block(0.5, 20, 0.001));
        assert!(wet.iter().all(|&v| v == 0.625));
        // Moving from 0 to 0.01 over 20 samples (0.0005 a sample): the first sample has the old level, the 16 paired
        // samples follow two series of 2 steps, the last 4 carry on the even one.
        let mut wet = vec![0.0f32; 40];
        let mut g = Glide::new(0.0);
        slot_blend(&mut wet, &dry, g.block(0.01, 20, 0.001));
        assert_eq!(g.current, 0.01);
        assert_eq!(wet[0], 0.0);
        for k in 0..20 {
            assert!((wet[2 * k] - 0.0005 * k as f32).abs() < 1e-7, "{k}: {}", wet[2 * k]);
            assert_eq!(wet[2 * k], wet[2 * k + 1]);
        }
        // The wet side is weighted by a running sum in doubles from single(1 - start).
        let mut wet = vec![1.0f32; 40];
        slot_blend(&mut wet, &vec![0.0; 40], Glide::new(0.0).block(0.01, 20, 0.001));
        assert_eq!(wet[0], 1.0);
        assert!((wet[38] - (1.0 - 19.0 * 0.0005)).abs() < 1e-6);
    }
}
