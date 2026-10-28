// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What an automation clip hands on: the curve's level put into the clip's range (its channel's two level knobs) and
//! moved by the clip's LFO, in the precision the host uses (every step a single unless the page says otherwise).
//!
// spec: audio/automation.md ("Range", "The LFO", "What a clip hands on", "The LFO's value")

use crate::{AutoMode, AutoPoint};

/// The LFO of an automation clip as stored (spec: audio/automation.md "The LFO"; flp/layouts.md "An automation
/// clip's record"). Values are the stored ones, kept as read.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClipLfo {
    /// 0 = off, -1 = on.
    pub switch: i32,
    /// 128 = the clip's whole range.
    pub amount: i32,
    /// 0: the LFO multiplies the curve; 1: it is added.
    pub mode: u8,
    /// 45819 = one cycle per beat (`cycles_per_beat`).
    pub speed: i32,
    /// -128..128: bends the wave.
    pub tension: i32,
    /// In 1/256: moves the wave's turning point.
    pub skew: i32,
    /// In 1/128: positive holds the wave at its top, negative at its bottom.
    pub pulse_width: i32,
    /// Added to the starting phase (one cycle = 2^32).
    pub phase: i32,
    /// Bit 0x02: the phase is not restarted with the clip; 0x04 one-sided shaping; 0x08 frozen (kept, not played:
    /// the page reads them only).
    pub flags: u8,
    /// The LFO's own envelope, in the form of the curve's points (beats from its start): the LFO is scaled by its level.
    pub envelope: Vec<AutoPoint>,
}

impl Default for ClipLfo {
    /// A new clip's: off, amount 64, multiplying, one cycle per beat, the envelope one point at level 1.
    fn default() -> Self {
        let mut one = AutoPoint::new(0.0, 1.0);
        one.flag = 0;
        ClipLfo { switch: 0, amount: 64, mode: 0, speed: 45819, tension: 0, skew: 0, pulse_width: 0, phase: 0, flags: 1, envelope: vec![one] }
    }
}

impl ClipLfo {
    pub const SPEED_DEFAULT: i32 = 45819;

    pub fn on(&self) -> bool {
        self.switch != 0
    }

    pub fn adds(&self) -> bool {
        self.mode == 1
    }

    /// Cycles per beat at stored `speed`: `(10001^(speed / 65536) - 1) / 625`.
    pub fn cycles_per_beat(speed: i32) -> f64 {
        (10001f64.powf(f64::from(speed) / 65536.0) - 1.0) / 625.0
    }
}

/// `x` rounded to the nearest whole number, halves to even (the host's rounding).
fn round(x: f64) -> f64 {
    x.round_ties_even()
}

/// A clip's range from its channel's levels as the model holds them (`Channel::pan` -1..1 for stored 0..12800,
/// `Channel::volume` stored / 12800): `(min, range)`, both singles (spec: audio/automation.md "Range").
pub fn range(pan: f64, volume: f64) -> (f32, f32) {
    let (p, v) = (((pan + 1.0) * 6400.0).round(), (volume * 12800.0).round());
    let r = 1.0f32 / 12800.0;
    let min = p as f32 * r;
    (min, v as f32 * r - min)
}

/// The wave's fixed numbers, worked out once from the shape settings (spec: audio/automation.md "The LFO's value").
#[derive(Debug, Clone, Copy, PartialEq)]
struct Wave {
    end: i64,
    rise: i64,
    fall: i64,
    /// `single(2 / fall)` and `single(2 / (end - rise))`.
    down: f32,
    up: f32,
    /// The tension's bend: below 0, and `c1`, `c2`; `None` at tension 0.
    bend: Option<(bool, f32, f32)>,
}

impl Wave {
    fn new(lfo: &ClipLfo) -> Wave {
        const C: f64 = 4_294_967_296.0;
        let pw = lfo.pulse_width as f32 * (1.0f32 / 128.0);
        let sk = lfo.skew as f32 * (1.0f32 / 256.0);
        // `single((1 - pw) * 2^32)` in singles, the 1 taken off that single.
        let end = if pw <= 0.0 { (1i64 << 32) - 1 } else { round(f64::from((1.0 - pw) * C as f32) - 1.0) as i64 };
        let half = f64::from(((1.0 - f64::from(pw)) / 2.0) as f32);
        let r = (half - f64::from(sk) * (1.0 - f64::from(pw.abs()))) * C;
        let r = r.max(1.0).min((end - 1) as f64);
        let rise = round(r) as i64;
        let fall = if pw >= 0.0 { rise } else { (round(r + f64::from(pw * C as f32)) as i64).max(1) };
        let bend = (lfo.tension != 0).then(|| {
            let q = (lfo.tension.unsigned_abs() as f32) / 128.0;
            let s1 = ((f64::from(q) * 21f64.ln()).exp() - 1.0) as f32;
            let s = (f64::from(s1) * 0.05) as f32;
            let a = ((f64::from(s) * f64::from(1e18f32).ln()).exp() - 1.0) as f32;
            (lfo.tension < 0, (1.0 / f64::from(a)) as f32, (f64::from(a) + 1.0).ln() as f32)
        });
        let up = (2.0 / (end - rise) as f64) as f32;
        Wave { end, rise, fall, down: (2.0 / fall as f64) as f32, up, bend }
    }

    /// The wave at `phase`, -1..1.
    fn at(&self, phase: u32) -> f32 {
        let p = i64::from(phase);
        let x = if p > self.end {
            return 1.0;
        } else if p < self.fall {
            1.0 - p as f32 * self.down
        } else if p < self.rise {
            return -1.0;
        } else {
            (p - self.rise) as f32 * self.up - 1.0
        };
        // The tension bends the two slopes; the flat parts stay.
        let Some((neg, c1, c2)) = self.bend else { return x };
        let (m, c1, c2) = (f64::from(x.abs()), f64::from(c1), f64::from(c2));
        let m = if neg { ((m * c2).exp() - 1.0) * c1 } else { 1.0 - (((1.0 - m) * c2).exp() - 1.0) * c1 };
        (m as f32).copysign(x)
    }
}

/// The LFO as it plays: its wave, its phase step per tick and starting phase, its amount in the clip's range.
#[derive(Debug, Clone, PartialEq)]
struct Running {
    wave: Wave,
    step: u32,
    start: u32,
    adds: bool,
    /// `single(stored amount) * single(1 / 128) * range`.
    amount: f32,
    envelope: Vec<AutoPoint>,
}

/// The phase step per tick at `ppq`: `Round(min(single(exp(single(speed / 65536) * ln 10001) - 1) * 2^32 / 625 * b,
/// 805306368))` with `b = single(1 / PPQ)`.
pub fn phase_step(speed: i32, ppq: u16) -> u32 {
    let q = (f64::from(speed) / 65536.0) as f32;
    let e = ((f64::from(q) * 10001f64.ln()).exp() - 1.0) as f32;
    let s = f64::from(e) * (4_294_967_296.0 / 625.0) * crate::curve::tick_step(ppq);
    round(s.min(805_306_368.0)).max(0.0) as u32
}

/// What an automation clip hands on, per tick, from the curve's level (spec: audio/automation.md "What a clip hands
/// on"): `out = min + level * range`, then the LFO when it is on.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipOut {
    min: f32,
    range: f32,
    lfo: Option<Running>,
}

impl Default for ClipOut {
    /// The whole range 0..1 and no LFO: the curve's level as it is.
    fn default() -> Self {
        ClipOut { min: 0.0, range: 1.0, lfo: None }
    }
}

/// Where the walk along the LFO's own envelope is, and how many tick steps it has taken (`ClipOut::envelope`).
pub type EnvWalk = Option<(crate::curve::Walk, u64)>;

impl ClipOut {
    /// The clip with the LFO `lfo` on a channel whose levels are `pan` and `volume` (as the model holds them).
    pub fn new(lfo: &ClipLfo, pan: f64, volume: f64, ppq: u16) -> ClipOut {
        let (min, range) = range(pan, volume);
        let lfo = lfo.on().then(|| {
            let wave = Wave::new(lfo);
            // The middle of the rising part, where the wave passes 0 going up, plus the phase offset.
            let start = wave.rise + (wave.end - wave.rise).div_euclid(2) + 1 + i64::from(lfo.phase);
            let amount = lfo.amount as f32 * (1.0f32 / 128.0) * range;
            let envelope = if lfo.envelope.is_empty() { ClipLfo::default().envelope } else { lfo.envelope.clone() };
            Running { wave, step: phase_step(lfo.speed, ppq), start: start as u32, adds: lfo.adds(), amount, envelope }
        });
        ClipOut { min, range, lfo }
    }

    pub fn has_lfo(&self) -> bool {
        self.lfo.is_some()
    }

    /// The LFO's phase for the clip's `k`-th tick (from 1): `start + k * step`, wrapping at 2^32.
    pub fn phase(&self, k: u64) -> Option<u32> {
        self.lfo.as_ref().map(|l| l.start.wrapping_add((k as u32).wrapping_mul(l.step)))
    }

    /// The level of the LFO's own envelope for the clip's `k`-th tick: its walk after `k` tick steps of
    /// `single(1 / PPQ)` from the clip's start (1 with the default single point). `walk` keeps the place between calls.
    pub fn envelope(&self, walk: &mut EnvWalk, k: u64, ppq: u16) -> f32 {
        let Some(l) = &self.lfo else { return 1.0 };
        let pts = &l.envelope;
        if let [one] = pts.as_slice() {
            return one.value as f32;
        }
        let (mut w, mut taken) = match *walk {
            Some(w) if w.1 <= k => w,
            _ => {
                let Some(w) = crate::curve::Walk::new(pts) else { return 1.0 };
                (w, 0)
            }
        };
        let b = crate::curve::tick_step(ppq);
        while taken < k {
            w.step(pts, b);
            taken += 1;
        }
        *walk = Some((w, taken));
        w.value(pts)
    }

    /// The envelope's level `ticks` ticks into the clip, read off the curve (between ticks, for values sampled inside
    /// a tick).
    pub fn envelope_at(&self, ticks: f64, ppq: u16) -> f32 {
        let Some(l) = &self.lfo else { return 1.0 };
        crate::curve::value_at(&l.envelope, ticks * crate::curve::tick_step(ppq)).map_or(1.0, |v| v as f32)
    }

    /// The LFO's value at `phase` scaled by the envelope's level `env`: `single(0.5 * wave(phase)) * env`.
    fn lfo(l: &Running, phase: u32, env: f32) -> f32 {
        0.5 * l.wave.at(phase) * env
    }

    /// What the clip hands on for the curve's `level` with the LFO at `phase` and its envelope at `env`, every step a
    /// single: `min + level * range`; adding, `out + lfo * amount`; multiplying, `out * (lfo * amount * 2 + 1)`.
    pub fn out_at(&self, level: f32, phase: u32, env: f32) -> f32 {
        let out = self.min + level * self.range;
        match &self.lfo {
            None => out,
            Some(l) if l.adds => out + Self::lfo(l, phase, env) * l.amount,
            Some(l) => out * (Self::lfo(l, phase, env) * l.amount * 2.0 + 1.0),
        }
    }

    /// What the clip hands on for its `k`-th tick (from 1) at the curve's `level`.
    pub fn out(&self, level: f32, k: u64, env: f32) -> f32 {
        self.out_at(level, self.phase(k).unwrap_or(0), env)
    }

    /// The value at `ticks` ticks into the clip (a fraction: between ticks the phase moves on in proportion; a Lemon
    /// Studio choice for values sampled inside a tick).
    pub fn out_between(&self, level: f32, ticks: f64, ppq: u16) -> f32 {
        let Some(l) = &self.lfo else { return self.out_at(level, 0, 1.0) };
        let whole = ticks.floor().max(0.0);
        let part = (ticks - whole) * f64::from(l.step);
        let phase = l.start.wrapping_add((whole as u64 as u32).wrapping_mul(l.step)).wrapping_add(part as u32);
        self.out_at(level, phase, self.envelope_at(ticks, ppq))
    }
}

/// The whole number handed on to a target: `Round(out * 2^30)`, kept within 0..2^30.
pub fn handed_on(out: f32) -> i64 {
    ((f64::from(out) * 1_073_741_824.0).round_ties_even() as i64).clamp(0, 1 << 30)
}

/// The envelope point form `ClipLfo::envelope` holds (a mode the page's walk shapes like the curve's).
pub fn envelope_point(beat: f64, value: f64) -> AutoPoint {
    AutoPoint { beat, value, tension: 0.0, mode: AutoMode::Single, flag: 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> ClipLfo {
        ClipLfo { switch: -1, ..ClipLfo::default() }
    }

    /// The page's checks: the defaults give a triangle whose start is where it passes 0 going up; one cycle per beat
    /// at 45819 (95.99 ticks at 96 PPQ) and one in 1535.99 ticks at 26259.
    #[test]
    fn the_default_wave_and_its_speed() {
        let w = Wave::new(&on());
        assert_eq!((w.end, w.rise, w.fall), ((1 << 32) - 1, 1 << 31, 1 << 31));
        let out = ClipOut::new(&on(), -1.0, 1.0, 96);
        let l = out.lfo.as_ref().unwrap();
        assert_eq!(l.start, 3 << 30);
        assert!(w.at(l.start).abs() < 1e-6, "{}", w.at(l.start));
        assert_eq!((w.at(0), w.at(1 << 30), w.at(1 << 31), w.at(u32::MAX)), (1.0, 0.0, -1.0, 1.0));
        let ticks = |speed| 4_294_967_296.0 / f64::from(phase_step(speed, 96));
        assert!((ticks(45819) - 95.99).abs() < 0.005, "{}", ticks(45819));
        assert!((ticks(26259) - 1535.99).abs() < 0.005, "{}", ticks(26259));
        assert!((ClipLfo::cycles_per_beat(45819) - 1.0).abs() < 1e-3);
        // Fast speeds are held at 805306368 per tick (5.33 ticks a cycle); 65536 is 16 cycles a beat.
        assert_eq!(phase_step(65536, 96), 715_827_904);
        assert_eq!(phase_step(70000, 96), 805_306_368);
    }

    /// "At the default amount of 64 a multiplying LFO swings a level of 0.5 between 0.25 and 0.75."
    #[test]
    fn the_default_amount_swings_half_a_level_by_half() {
        let out = ClipOut::new(&on(), -1.0, 1.0, 96);
        // Many cycles, so that the ticks come close to the wave's corners.
        let v: Vec<f32> = (1..=20000).map(|k| out.out(0.5, k, 1.0)).collect();
        let (lo, hi) = v.iter().fold((1f32, 0f32), |(a, b), &x| (a.min(x), b.max(x)));
        assert!((lo - 0.25).abs() < 1e-3 && (hi - 0.75).abs() < 1e-3, "{lo} {hi}");
        // The first tick is a step past the zero crossing, going up: 0.5 * (1 + 2 * 0.5 * 0.5 * step / 2^30).
        let x = 0.5 * (1.0 + 0.5 * f64::from(phase_step(45819, 96)) / 1_073_741_824.0);
        assert!((f64::from(v[0]) - x).abs() < 1e-6, "{} {x}", v[0]);
        // Adding: 0.5 +- 0.25 as well (amount 64 = half the range, lfo +-0.5).
        let add = ClipOut::new(&ClipLfo { mode: 1, ..on() }, -1.0, 1.0, 96);
        let hi = (1..=20000).map(|k| add.out(0.5, k, 1.0)).fold(0f32, f32::max);
        assert!((hi - 0.75).abs() < 1e-3, "{hi}");
        // Off: the level unchanged, every bit.
        assert_eq!(ClipOut::new(&ClipLfo::default(), -1.0, 1.0, 96).out(0.3, 7, 1.0), 0.3);
    }

    /// A range of 0.2..0.8 (stored pan 2560, volume 10240): `min + level * range` in singles, the LFO's amount a share of
    /// the range.
    #[test]
    fn a_range_of_point_two_to_point_eight() {
        let (pan, vol) = (2560.0 / 6400.0 - 1.0, 10240.0 / 12800.0);
        let (min, range) = range(pan, vol);
        assert_eq!((min, range), (2560.0f32 * (1.0 / 12800.0), 10240.0f32 * (1.0 / 12800.0) - 2560.0f32 * (1.0 / 12800.0)));
        let out = ClipOut::new(&ClipLfo::default(), pan, vol, 96);
        assert_eq!(out.out(0.0, 1, 1.0), min);
        assert_eq!(out.out(0.5, 1, 1.0), min + 0.5 * range);
        assert!((out.out(1.0, 1, 1.0) - 0.8).abs() < 1e-6);
        // Adding at amount 128: +-0.5 of the 0.6 range around the level.
        let add = ClipOut::new(&ClipLfo { mode: 1, amount: 128, ..on() }, pan, vol, 96);
        let v: Vec<f32> = (1..=20000).map(|k| add.out(0.5, k, 1.0)).collect();
        let (lo, hi) = v.iter().fold((1f32, 0f32), |(a, b), &x| (a.min(x), b.max(x)));
        assert!((lo - 0.2).abs() < 1e-3 && (hi - 0.8).abs() < 1e-3, "{lo} {hi}");
        // The defaults: 0..1, and a channel stored 12800 / 0 runs the curve upside down.
        assert_eq!(super::range(-1.0, 1.0), (0.0, 1.0));
        assert_eq!(super::range(1.0, 0.0), (1.0, -1.0));
        assert_eq!(handed_on(ClipOut::new(&ClipLfo::default(), 1.0, 0.0, 96).out(0.25, 1, 1.0)), 3 << 28);
    }

    /// The values handed on stay within 0..2^30 and round to the nearest whole number.
    #[test]
    fn handed_on_rounds_and_clamps() {
        assert_eq!((handed_on(-0.1), handed_on(1.5), handed_on(0.5)), (0, 1 << 30, 1 << 29));
        let amount_128 = ClipOut::new(&ClipLfo { amount: 128, ..on() }, -1.0, 1.0, 96);
        // Multiplying at amount 128: out * (lfo * 2 + 1), from 0 to twice the level.
        let hi = (1..=20000).map(|k| amount_128.out(0.75, k, 1.0)).fold(0f32, f32::max);
        assert!((hi - 1.5).abs() < 1e-3 && handed_on(hi) == 1 << 30, "{hi}");
        let zero = ClipOut::new(&ClipLfo { amount: 0, ..on() }, -1.0, 1.0, 96);
        assert!((1..=200).all(|k| zero.out(0.4, k, 1.0) == 0.4));
    }

    /// Pulse width and skew move the wave's corners; tension bends the slopes but keeps the ends and the middle.
    #[test]
    fn shape_settings() {
        // Pulse width 64 (+0.5): the wave sits at its top for the last half of the cycle.
        let w = Wave::new(&ClipLfo { pulse_width: 64, ..on() });
        assert_eq!(w.end, (1 << 31) - 1, "single((1 - 0.5) * 2^32) - 1, rounded");
        assert_eq!(w.rise, 1 << 30);
        assert_eq!((w.at(1 << 31), w.at(u32::MAX)), (1.0, 1.0));
        assert_eq!(w.at(0), 1.0);
        // -64: held at its bottom between fall and rise.
        let w = Wave::new(&ClipLfo { pulse_width: -64, ..on() });
        assert_eq!((w.end, w.rise, w.fall), ((1 << 32) - 1, 3 << 30, 1 << 30));
        assert_eq!((w.at(1 << 30), w.at((3 << 30) - 1)), (-1.0, -1.0));
        // Skew 64 (+0.25): the turning point moves to a quarter of the cycle.
        let w = Wave::new(&ClipLfo { skew: 64, ..on() });
        assert_eq!(w.rise, 1 << 30);
        assert_eq!((w.at(0), w.at(1 << 30)), (1.0, -1.0));
        // Skew -128: the turning point at the cycle's end, clamped to end - 1.
        assert_eq!(Wave::new(&ClipLfo { skew: -128, ..on() }).rise, (1 << 32) - 2);
        for t in [32, -64, 128, -128] {
            let w = Wave::new(&ClipLfo { tension: t, ..on() });
            let plain = Wave::new(&on());
            // The ends stay, within the singles the bend is worked out in.
            assert!((w.at(0) - 1.0).abs() < 1e-4 && (w.at(1 << 31) + 1.0).abs() < 1e-4, "{t}: {} {}", w.at(0), w.at(1 << 31));
            // Bent towards one side: positive tension leaves |x| larger, negative smaller, on the slopes.
            let (a, b) = (w.at(1 << 29).abs(), plain.at(1 << 29).abs());
            assert!(if t > 0 { a > b } else { a < b }, "{t}: {a} {b}");
        }
    }

    /// The envelope scales the LFO: a single point at level 0 silences it; two points fade it in over their walk.
    #[test]
    fn the_envelope_scales_the_lfo() {
        let lfo = ClipLfo { envelope: vec![envelope_point(0.0, 0.0)], ..on() };
        let out = ClipOut::new(&lfo, -1.0, 1.0, 96);
        let mut w = None;
        assert!((1..=96).all(|k| out.out(0.5, k, out.envelope(&mut w, k, 96)) == 0.5));
        let lfo = ClipLfo { envelope: vec![envelope_point(0.0, 0.0), envelope_point(1.0, 1.0)], ..on() };
        let out = ClipOut::new(&lfo, -1.0, 1.0, 96);
        let mut w = None;
        assert!((out.envelope(&mut w, 48, 96) - 0.5).abs() < 1e-6);
        assert_eq!(out.envelope(&mut w, 200, 96), 1.0);
        assert_eq!(out.envelope(&mut w, 1, 96), out.envelope(&mut None, 1, 96), "a walk back starts again");
    }
}
