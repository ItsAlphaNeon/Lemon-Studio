// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! A channel's five articulators (pan, volume, mod X, mod Y, pitch): an envelope and an LFO each, stepped once per
//! sequencer tick for every voice, and the parts they give a voice's levels.
//!
//! The steps, the tension constants and the parts are singles, the exponentials and the curves doubles rounded once,
//! as the spec gives them.
// spec: audio/envelopes-and-lfos.md

use std::f64::consts::PI;

/// A level below this (2^-24) after the hold stage ends the envelope.
const TINY: f32 = 1.0 / 16_777_216.0;

/// One of a channel's five records (channel event 218, seventeen ints) as stored (spec "Settings").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub flags: i32,
    /// The envelope is switched on.
    pub on: bool,
    /// Time knobs (0..65536).
    pub delay: i32,
    pub attack: i32,
    pub hold: i32,
    pub decay: i32,
    /// 0..128.
    pub sustain: i32,
    pub release: i32,
    /// -128..128 (the pitch record: cents).
    pub amount: i32,
    /// Time knobs.
    pub lfo_delay: i32,
    pub lfo_attack: i32,
    /// -128..128 (the pitch record: cents).
    pub lfo_amount: i32,
    /// A time knob: the LFO's period.
    pub lfo_speed: i32,
    /// 0 sine, 1 triangle, 2 square (limited to that range when used).
    pub lfo_shape: i32,
    /// -128..128.
    pub attack_tension: i32,
    pub decay_tension: i32,
    pub release_tension: i32,
}

impl Record {
    /// The records, in the order a channel stores them.
    pub const PAN: usize = 0;
    pub const VOLUME: usize = 1;
    pub const MOD_X: usize = 2;
    pub const MOD_Y: usize = 3;
    pub const PITCH: usize = 4;

    /// Flag values (int 0): envelope times follow the tempo; LFO times follow the tempo; the envelope's value is 1 while
    /// it is off; one LFO phase for all voices.
    pub const TIMES_FOLLOW_TEMPO: i32 = 1;
    pub const LFO_FOLLOWS_TEMPO: i32 = 2;
    pub const ONE_WHILE_OFF: i32 = 4;
    pub const GLOBAL_LFO: i32 = 0x20;

    /// A new channel's record (spec "Settings", "A new channel's records"): envelope off, attack, hold and release at
    /// 20000, decay 30000, sustain 50, LFO speed 32950; the volume record has flags 4 and a release tension of -101.
    pub fn new_channel(which: usize) -> Record {
        let volume = which == Record::VOLUME;
        Record {
            flags: if volume { Record::ONE_WHILE_OFF } else { 0 },
            on: false,
            delay: 100,
            attack: 20000,
            hold: 20000,
            decay: 30000,
            sustain: 50,
            release: 20000,
            amount: 0,
            lfo_delay: 100,
            lfo_attack: 20000,
            lfo_amount: 0,
            lfo_speed: 32950,
            lfo_shape: 0,
            attack_tension: 0,
            decay_tension: 0,
            release_tension: if volume { -101 } else { 0 },
        }
    }

    /// A new channel's five records.
    pub fn new_channel_records() -> [Record; 5] {
        std::array::from_fn(Record::new_channel)
    }
}

/// The whole ticks of a time knob (spec "Time knobs"): 0 below 101, else
/// `Round(single(exp(single(knob / 65536) * ln 1001) - 1) * scale)` with `scale = (Floor(PPQ / 4) / 48) * 24.576`,
/// times `tempo / 120` when the time does not follow the tempo.
pub fn knob_ticks(knob: i32, ppq: u16, tempo: f64, follows_tempo: bool) -> u32 {
    if knob < 101 {
        return 0;
    }
    let x = f64::from(knob as f32 / 65536.0);
    let e = ((x * 1001f64.ln()).exp() - 1.0) as f32;
    let mut scale = f64::from(ppq / 4) / 48.0 * 24.576;
    if !follows_tempo {
        scale = scale * tempo / 120.0;
    }
    (f64::from(e) * scale).round_ties_even().max(0.0) as u32
}

/// A tension knob's curve (spec "Tension").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tension {
    /// `sign(t) * single(single(exp(single(|t| / 128) * ln 6347) - 1) * single(0.1))`.
    v: f32,
    /// `single(ln(|v| + 1))`, `single(1 / |v|)`.
    l: f32,
    i: f32,
}

impl Tension {
    pub fn new(t: i32) -> Tension {
        if t == 0 {
            return Tension { v: 0.0, l: 0.0, i: 0.0 };
        }
        let a = f64::from(t.unsigned_abs() as f32 / 128.0);
        let m = ((a * 6347f64.ln()).exp() - 1.0) as f32 * 0.1f32;
        Tension { v: if t < 0 { -m } else { m }, l: (f64::from(m) + 1.0).ln() as f32, i: (1.0 / f64::from(m)) as f32 }
    }

    /// The level at linear position `x` (0..1).
    pub fn shape(&self, x: f32) -> f32 {
        let (x, l, i) = (f64::from(x), f64::from(self.l), f64::from(self.i));
        if self.v == 0.0 {
            x as f32
        } else if self.v > 0.0 {
            (1.0 - (((1.0 - x) * l).exp() - 1.0) * i) as f32
        } else {
            (((x * l).exp() - 1.0) * i) as f32
        }
    }

    /// The linear position that gives level `y`.
    pub fn inverse(&self, y: f32) -> f32 {
        let (y, m, l) = (f64::from(y), f64::from(self.v.abs()), f64::from(self.l));
        if self.v == 0.0 {
            y as f32
        } else if self.v > 0.0 {
            (1.0 - ((1.0 - y) * m + 1.0).ln() / l) as f32
        } else {
            ((y * m + 1.0).ln() / l) as f32
        }
    }
}

/// A record's times as whole ticks at one tempo and PPQ, and its curves. Worked out again when the tempo changes;
/// voices carry on from where they are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    delay: u32,
    hold: u32,
    /// `single(1 / (ticks + 1))` of the attack, decay and release.
    attack_step: f32,
    decay_step: f32,
    release_step: f32,
    /// The attack knob is exactly 100: the envelope starts at its top.
    instant: bool,
    /// `sustain / 128`.
    sustain: f32,
    /// Attack, decay and release tension.
    curves: [Tension; 3],
    lfo_delay: u32,
    lfo_fade_step: f32,
    /// `Floor(2^32 / period)`, the period at least 3 ticks.
    lfo_step: u32,
}

impl Timing {
    pub fn new(r: &Record, ppq: u16, tempo: f64) -> Timing {
        let env = |k: i32| knob_ticks(k, ppq, tempo, r.flags & Record::TIMES_FOLLOW_TEMPO != 0);
        let lfo = |k: i32| knob_ticks(k, ppq, tempo, r.flags & Record::LFO_FOLLOWS_TEMPO != 0);
        let step = |ticks: u32| 1.0f32 / (ticks as f32 + 1.0);
        let period = lfo(r.lfo_speed).max(3);
        Timing {
            delay: env(r.delay),
            hold: env(r.hold),
            attack_step: step(env(r.attack)),
            decay_step: step(env(r.decay)),
            release_step: step(env(r.release)),
            instant: r.attack == 100,
            sustain: r.sustain as f32 / 128.0,
            curves: [Tension::new(r.attack_tension), Tension::new(r.decay_tension), Tension::new(r.release_tension)],
            lfo_delay: lfo(r.lfo_delay),
            lfo_fade_step: step(lfo(r.lfo_attack)),
            lfo_step: ((1u64 << 32) / u64::from(period)) as u32,
        }
    }

    /// The five records' timings.
    pub fn all(records: &[Record; 5], ppq: u16, tempo: f64) -> [Timing; 5] {
        std::array::from_fn(|k| Timing::new(&records[k], ppq, tempo))
    }

    /// The LFO's phase step per tick.
    pub fn lfo_step(&self) -> u32 {
        self.lfo_step
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    Finished,
}

/// One voice's envelope: its stage, the ticks counted in it, the linear position and the output (spec "The envelope,
/// per tick").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    stage: Stage,
    count: u32,
    lin: f32,
    out: f32,
}

impl Envelope {
    /// At a voice's start: in the delay stage with a delay of a tick or more (envelope on); else in the attack from 0,
    /// or at its top in the hold (or decay) stage with an instant attack or, switched off, with the flag of value 4.
    pub fn start(r: &Record, t: &Timing) -> Envelope {
        if r.on && t.delay >= 1 {
            return Envelope { stage: Stage::Delay, count: 0, lin: 0.0, out: 0.0 };
        }
        if t.instant || (!r.on && r.flags & Record::ONE_WHILE_OFF != 0) {
            let stage = if t.hold > 0 { Stage::Hold } else { Stage::Decay };
            return Envelope { stage, count: 0, lin: 1.0, out: 1.0 };
        }
        Envelope { stage: Stage::Attack, count: 0, lin: 0.0, out: 0.0 }
    }

    /// One tick (only while the envelope is on).
    fn step(&mut self, t: &Timing) {
        match self.stage {
            Stage::Delay if self.count < t.delay => self.count += 1,
            Stage::Delay | Stage::Attack => {
                self.stage = Stage::Attack;
                self.lin += t.attack_step;
                if self.lin >= 1.0 {
                    self.lin = 1.0;
                    self.count = 0;
                    self.stage = if t.hold > 0 { Stage::Hold } else { Stage::Decay };
                }
                self.out = t.curves[0].shape(self.lin);
            }
            Stage::Hold if self.count < t.hold => self.count += 1,
            Stage::Hold | Stage::Decay => {
                self.stage = Stage::Decay;
                self.lin -= t.decay_step;
                self.out = t.curves[1].shape(self.lin);
                if self.out <= t.sustain {
                    self.out = t.sustain;
                    self.lin = t.curves[1].inverse(t.sustain);
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                self.lin -= t.release_step;
                self.out = t.curves[2].shape(self.lin);
            }
            Stage::Finished => return,
        }
        if matches!(self.stage, Stage::Decay | Stage::Sustain | Stage::Release) && self.out < TINY {
            self.out = 0.0;
            self.stage = Stage::Finished;
        }
    }

    /// The note is released: from the position that gives the current output on the release curve.
    fn release(&mut self, t: &Timing) {
        if self.stage != Stage::Finished {
            self.lin = t.curves[2].inverse(self.out);
            self.stage = Stage::Release;
        }
    }

    pub fn out(&self) -> f32 {
        self.out
    }

    /// It has run out (after the hold stage its output fell below 2^-24).
    pub fn finished(&self) -> bool {
        self.stage == Stage::Finished
    }
}

/// The value of a shape's table at entry `i` (0..16384; spec "The three tables"). The sine is the double-precision
/// sine of the entry's angle, rounded once, as the table holds it; shapes below 0 are the sine, above 2 the square.
pub fn table(shape: i32, i: usize) -> f32 {
    match shape.clamp(0, 2) {
        0 => ((2.0 * i as f64 * PI) / 16384.0).sin() as f32,
        1 => {
            let x = 4.0 * i as f32 / 16384.0;
            if i < 4096 {
                x
            } else if i < 12288 {
                2.0 - x
            } else {
                x - 4.0
            }
        }
        _ => {
            if i < 8192 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

/// One voice's LFO (spec "The LFO, per tick").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lfo {
    /// Ticks waited of its delay.
    waited: u32,
    phase: u32,
    /// The fade-in level.
    level: f32,
    value: f32,
}

impl Lfo {
    pub fn start() -> Lfo {
        Lfo { waited: 0, phase: 0, level: 0.0, value: 0.0 }
    }

    /// One tick: after its delay the phase moves on by the step (or takes the shared phase of a global LFO), the
    /// shape's table is read at the phase's top 14 bits, and the fade-in level scales it until it reaches 1.
    fn step(&mut self, r: &Record, t: &Timing, shared: Option<u32>) {
        if self.waited < t.lfo_delay {
            self.waited += 1;
            return;
        }
        self.phase = shared.unwrap_or_else(|| self.phase.wrapping_add(t.lfo_step));
        let mut wave = table(r.lfo_shape, (self.phase >> 18) as usize);
        if self.level != 1.0 {
            self.level = (self.level + t.lfo_fade_step).min(1.0);
            wave *= self.level;
        }
        self.value = wave;
    }

    pub fn value(&self) -> f32 {
        self.value
    }
}

/// What the five give a voice's levels (spec "What a voice gets from them").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parts {
    /// Added to the pan.
    pub pan: f32,
    /// Multiplies the volume (0 or more).
    pub volume: f32,
    /// Cents added to the pitch.
    pub pitch: f32,
    /// Added to the mod X / mod Y knob positions.
    pub mod_x: f32,
    pub mod_y: f32,
}

impl Parts {
    /// No change to the levels.
    pub const NONE: Parts = Parts { pan: 0.0, volume: 1.0, pitch: 0.0, mod_x: 0.0, mod_y: 0.0 };
}

/// A voice's five envelopes and LFOs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voice {
    env: [Envelope; 5],
    lfo: [Lfo; 5],
}

impl Voice {
    /// At the voice's start, before its first tick.
    pub fn start(records: &[Record; 5], timing: &[Timing; 5]) -> Voice {
        Voice { env: std::array::from_fn(|k| Envelope::start(&records[k], &timing[k])), lfo: [Lfo::start(); 5] }
    }

    /// One tick: every envelope that is on steps, every LFO steps (whether its envelope is on or not). `shared` holds
    /// the shared phase of each record with the global LFO.
    pub fn step(&mut self, records: &[Record; 5], timing: &[Timing; 5], shared: &[Option<u32>; 5]) {
        for k in 0..5 {
            if records[k].on {
                self.env[k].step(&timing[k]);
            }
            self.lfo[k].step(&records[k], &timing[k], shared[k]);
        }
    }

    /// The note is released: every envelope that is on goes into its release.
    pub fn release(&mut self, records: &[Record; 5], timing: &[Timing; 5]) {
        for k in 0..5 {
            if records[k].on {
                self.env[k].release(&timing[k]);
            }
        }
    }

    /// The volume envelope is on and has run out.
    pub fn volume_ended(&self, records: &[Record; 5]) -> bool {
        records[Record::VOLUME].on && self.env[Record::VOLUME].finished()
    }

    pub fn envelope(&self, k: usize) -> &Envelope {
        &self.env[k]
    }

    pub fn lfo(&self, k: usize) -> &Lfo {
        &self.lfo[k]
    }

    /// The parts, every operation a single. The pan record's envelope reaches nothing: only its LFO moves the pan.
    pub fn parts(&self, records: &[Record; 5]) -> Parts {
        let over = |a: i32| a as f32 / 128.0;
        let (pan, vol, pitch) = (&records[Record::PAN], &records[Record::VOLUME], &records[Record::PITCH]);
        let lfo = |k: usize| self.lfo[k].value;
        let env = |k: usize| self.env[k].out;
        let volume = env(Record::VOLUME) * (lfo(Record::VOLUME) * over(vol.lfo_amount) + 1.0);
        let m = |k: usize| env(k) * over(records[k].amount) + lfo(k) * over(records[k].lfo_amount);
        Parts {
            pan: lfo(Record::PAN) * over(pan.lfo_amount) * 2.0,
            volume: if volume < 0.0 { 0.0 } else { volume },
            pitch: env(Record::PITCH) * pitch.amount as f32 + lfo(Record::PITCH) * pitch.lfo_amount as f32,
            mod_x: m(Record::MOD_X),
            mod_y: m(Record::MOD_Y),
        }
    }
}

/// The shared phases of the records with the global LFO, kept as an instrument plugin keeps them (spec "The global
/// LFO", "On 3x Osc"): 0 until the plugin's first note, then one step on every tick after that note's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shared {
    phase: [u32; 5],
    started: bool,
}

impl Shared {
    /// A tick starts, before any voice steps.
    pub fn tick(&mut self, timing: &[Timing; 5]) {
        if self.started {
            for k in 0..5 {
                self.phase[k] = self.phase[k].wrapping_add(timing[k].lfo_step);
            }
        }
    }

    /// A note starts (the first one starts the phases).
    pub fn note(&mut self) {
        self.started = true;
    }

    /// The shared phase of each record that has the global LFO.
    pub fn phases(&self, records: &[Record; 5]) -> [Option<u32>; 5] {
        std::array::from_fn(|k| (records[k].flags & Record::GLOBAL_LFO != 0).then_some(self.phase[k]))
    }

    /// A new render or playback: back to 0, waiting for the first note.
    pub fn reset(&mut self) {
        *self = Shared::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(mut r: Record) -> Record {
        r.on = true;
        r
    }

    /// The time knobs: below 101 nothing; at 120 BPM and PPQ 96 a knob is `(1001^(knob / 65536) - 1) * 64` ms; times
    /// that follow the tempo do not depend on it.
    #[test]
    fn time_knobs_are_whole_ticks() {
        assert_eq!(knob_ticks(100, 96, 120.0, false), 0);
        assert_eq!(knob_ticks(-5, 96, 120.0, false), 0);
        for knob in [101, 5000, 20000, 32950, 65536] {
            let ms = (1001f64.powf(f64::from(knob) / 65536.0) - 1.0) * 64.0;
            let ticks = ms / 1000.0 * 192.0;
            assert!((f64::from(knob_ticks(knob, 96, 120.0, false)) - ticks).abs() <= 0.5 + 1e-3 * ticks, "{knob}");
            assert_eq!(knob_ticks(knob, 96, 60.0, true), knob_ticks(knob, 96, 120.0, false));
        }
        // The scale's Floor(PPQ / 4): PPQ 98 counts like 96.
        assert_eq!(knob_ticks(30000, 98, 140.0, false), knob_ticks(30000, 96, 140.0, false));
        assert_eq!(knob_ticks(65536, 96, 120.0, true), (1000.0f64 * 12.288).round() as u32);
    }

    /// Tension 0 is a straight line; the curves meet the ends and their inverses give the position back.
    #[test]
    fn tension_curves() {
        let line = Tension::new(0);
        assert_eq!((line.shape(0.3), line.inverse(0.3)), (0.3, 0.3));
        for t in [-128, -101, -20, 1, 64, 128] {
            let c = Tension::new(t);
            assert!(c.shape(0.0).abs() < 1e-6 && (c.shape(1.0) - 1.0).abs() < 1e-6, "{t}");
            for x in [0.1f32, 0.5, 0.9] {
                assert!((c.inverse(c.shape(x)) - x).abs() < 1e-4, "{t} {x}");
            }
            // Positive tension rises fast and ends slowly; negative the other way.
            assert_eq!(c.shape(0.5) > 0.5, t > 0, "{t}");
        }
        // single(single(exp(1 * ln 6347) - 1) * single(0.1)) = 634.6
        assert!((Tension::new(128).v - 634.6).abs() < 1e-3);
    }

    /// The stages: a two-tick delay waits, the attack climbs in steps of 1 / (ticks + 1), the hold waits, the decay
    /// falls to the sustain level, the release falls to 0 and ends the envelope.
    #[test]
    fn an_envelope_goes_through_its_stages() {
        let r = on(Record { delay: 3000, attack: 2000, hold: 2000, decay: 4000, sustain: 64, release: 3000, ..Record::new_channel(0) });
        let t = Timing::new(&r, 96, 120.0);
        assert_eq!((t.delay, t.hold), (knob_ticks(3000, 96, 120.0, false), knob_ticks(2000, 96, 120.0, false)));
        let (d, a, h) = (t.delay, knob_ticks(2000, 96, 120.0, false), t.hold);
        assert!(d >= 1 && a >= 1 && h >= 1);
        let mut e = Envelope::start(&r, &t);
        let mut outs = Vec::new();
        for _ in 0..200 {
            e.step(&t);
            outs.push(e.out);
        }
        assert!(outs[..d as usize].iter().all(|&o| o == 0.0), "the delay waits");
        assert!((outs[d as usize] - 1.0 / (a as f32 + 1.0)).abs() < 1e-6, "the attack's first step");
        let top = outs.iter().position(|&o| o == 1.0).unwrap();
        assert!(top >= d as usize + a as usize && top <= d as usize + a as usize + 1);
        assert!(outs[top..=top + h as usize].iter().all(|&o| o == 1.0), "the hold");
        assert!(outs[top + h as usize + 1] < 1.0, "the decay");
        assert_eq!(*outs.last().unwrap(), 0.5, "the sustain level");
        e.release(&t);
        let mut n = 0;
        while !e.finished() {
            let before = e.out;
            e.step(&t);
            assert!(e.out < before);
            n += 1;
            assert!(n < 10_000);
        }
        assert_eq!(e.out, 0.0);
        // Half way down a straight release from 0.5 takes the release's ticks + 1 / 2 or so.
        let rel = knob_ticks(3000, 96, 120.0, false);
        assert!((n as i64 - (rel as i64 + 1) / 2).abs() <= 2, "{n} {rel}");
    }

    /// An instant attack (knob exactly 100) or a switched-off envelope with flag 4 starts at its top; switched off
    /// without the flag it stays at 0.
    #[test]
    fn envelope_starts() {
        let t = |r: &Record| Timing::new(r, 96, 120.0);
        let instant = on(Record { attack: 100, ..Record::new_channel(0) });
        assert_eq!(Envelope::start(&instant, &t(&instant)).out, 1.0);
        let off = Record::new_channel(Record::VOLUME);
        assert_eq!(Envelope::start(&off, &t(&off)).out, 1.0);
        let plain = Record::new_channel(Record::PITCH);
        assert_eq!(Envelope::start(&plain, &t(&plain)).out, 0.0);
        // A new channel's records change nothing.
        let records = Record::new_channel_records();
        let timing = Timing::all(&records, 96, 130.0);
        let mut v = Voice::start(&records, &timing);
        for _ in 0..50 {
            v.step(&records, &timing, &[None; 5]);
            assert_eq!(v.parts(&records), Parts::NONE);
        }
    }

    /// The sine table is the double sine rounded once; the triangle and square are exact.
    #[test]
    fn lfo_tables() {
        assert_eq!(table(0, 4096), 1.0);
        assert_eq!(table(0, 0), 0.0);
        assert_eq!(table(0, 1), (2.0 * PI / 16384.0).sin() as f32);
        assert_eq!([table(1, 0), table(1, 4096), table(1, 8192), table(1, 12288), table(1, 2048)], [0.0, 1.0, 0.0, -1.0, 0.5]);
        assert_eq!([table(2, 8191), table(2, 8192)], [1.0, -1.0]);
        assert_eq!(table(-1, 4096), table(0, 4096));
        assert_eq!(table(100, 9000), -1.0);
    }

    /// An LFO waits its delay, then moves by Floor(2^32 / period) per tick, fading in over its attack; a global LFO
    /// reads the shared phase.
    #[test]
    fn lfos_step_their_phase() {
        let r = Record { lfo_speed: 30000, lfo_attack: 0, lfo_amount: 128, lfo_shape: 1, ..Record::new_channel(0) };
        let t = Timing::new(&r, 96, 120.0);
        let period = knob_ticks(30000, 96, 120.0, false).max(3);
        assert_eq!(t.lfo_step, ((1u64 << 32) / u64::from(period)) as u32);
        let mut l = Lfo::start();
        for k in 1..=5u32 {
            l.step(&r, &t, None);
            assert_eq!(l.phase, t.lfo_step.wrapping_mul(k));
            assert_eq!(l.value, table(1, (l.phase >> 18) as usize));
        }
        // With a delay and a fade-in.
        let slow = Record { lfo_delay: 3000, lfo_attack: 3000, ..r };
        let ts = Timing::new(&slow, 96, 120.0);
        let mut l = Lfo::start();
        for _ in 0..ts.lfo_delay {
            l.step(&slow, &ts, None);
            assert_eq!((l.value, l.phase), (0.0, 0));
        }
        l.step(&slow, &ts, None);
        assert_eq!(l.level, ts.lfo_fade_step);
        assert_eq!(l.value, table(1, (ts.lfo_step >> 18) as usize) * ts.lfo_fade_step);
        // A global LFO.
        let mut l = Lfo::start();
        l.step(&r, &t, Some(12345 << 18));
        assert_eq!(l.value, table(1, 12345));
    }

    /// The shared phase stands at 0 until the first note and then moves on every tick, whether a voice sounds or not.
    #[test]
    fn the_shared_phase_starts_with_the_first_note() {
        let mut records = Record::new_channel_records();
        records[Record::VOLUME].flags |= Record::GLOBAL_LFO;
        records[Record::VOLUME].lfo_amount = 64;
        let timing = Timing::all(&records, 96, 120.0);
        let step = timing[Record::VOLUME].lfo_step;
        let mut s = Shared::default();
        s.tick(&timing);
        assert_eq!(s.phases(&records)[Record::VOLUME], Some(0));
        assert_eq!(s.phases(&records)[Record::PAN], None);
        s.note();
        assert_eq!(s.phases(&records)[Record::VOLUME], Some(0));
        for k in 1..=47u32 {
            s.tick(&timing);
            assert_eq!(s.phases(&records)[Record::VOLUME], Some(step.wrapping_mul(k)));
        }
        s.reset();
        s.tick(&timing);
        assert_eq!(s.phases(&records)[Record::VOLUME], Some(0));
    }

    /// The parts: the pan LFO doubled, the volume envelope times one plus the volume LFO, the pitch in cents from the
    /// knobs as they are, mod X / Y over 128; the pan envelope reaches nothing.
    #[test]
    fn parts_of_a_voice() {
        let mut records = Record::new_channel_records();
        records[Record::PAN] = on(Record { attack: 100, amount: 128, lfo_amount: 64, lfo_shape: 2, ..records[Record::PAN] });
        records[Record::VOLUME].lfo_amount = -128;
        records[Record::VOLUME].lfo_shape = 2;
        records[Record::PITCH] = on(Record { attack: 100, amount: 1200, ..records[Record::PITCH] });
        records[Record::MOD_X] = on(Record { attack: 100, amount: -64, lfo_amount: 32, lfo_shape: 2, ..records[Record::MOD_X] });
        let timing = Timing::all(&records, 96, 120.0);
        let mut v = Voice::start(&records, &timing);
        v.step(&records, &timing, &[None; 5]);
        let p = v.parts(&records);
        // Square LFOs read +1 on their first tick, times the fade-in's first step.
        let fade = timing[0].lfo_fade_step;
        assert_eq!(p.pan, fade * 0.5 * 2.0);
        assert_eq!(p.volume, (1.0 - fade).max(0.0));
        assert_eq!(p.pitch, 1200.0);
        assert_eq!(p.mod_x, -0.5 + fade * 0.25);
        assert_eq!(p.mod_y, 0.0);
    }

    /// With the volume envelope on, a released voice's envelope runs out.
    #[test]
    fn the_volume_envelope_runs_out_after_the_release() {
        let mut records = Record::new_channel_records();
        records[Record::VOLUME] = on(Record { attack: 100, release: 2000, ..records[Record::VOLUME] });
        let timing = Timing::all(&records, 96, 120.0);
        let mut v = Voice::start(&records, &timing);
        for _ in 0..20 {
            v.step(&records, &timing, &[None; 5]);
        }
        assert!(!v.volume_ended(&records));
        v.release(&records, &timing);
        let mut n = 0;
        while !v.volume_ended(&records) {
            v.step(&records, &timing, &[None; 5]);
            n += 1;
            assert!(n < 1000);
        }
        assert_eq!(v.parts(&records).volume, 0.0);
    }
}
