// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A project's Fruity Peak Controller slots, played by Lemon Peak Controller's processing: a peak follower and an LFO
//! that work once per tick and put out three controller values (Peak + LFO, Peak, LFO) for the links that read them
//! (`engine/controllers.rs`). The sound passes through, or is muted.
//!
// spec: plugins/fruity-peak-controller.md, plugins/lemon-peak-controller.md

use super::Effect;

/// The saved state's size: ten little-endian int32 fields and two switch bytes.
pub const STATE_LEN: usize = 42;
/// The outputs, in link order (a link's output number).
pub const OUTPUTS: usize = 3;
pub const OUTPUT_NAMES: [&str; OUTPUTS] = ["Peak+LFO", "Peak", "LFO"];
/// The largest change of an output per tick with Ramp controllers on.
pub const RAMP_STEP: f64 = 0.125;
/// The LFO shapes, in the state's order.
pub const SHAPES: [&str; 5] = ["Sine", "Triangle", "Square", "Saw", "Random"];

/// The settings as the state stores them (ranges in `Settings::clamped`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Peak base level, 0..65536 = 0..1.
    pub base: i32,
    /// Peak amount, -256..256 (179 = 100 %, 256 = 200 %).
    pub amount: i32,
    /// Peak tension, -128..128.
    pub tension: i32,
    /// Peak decay speed, 0..128 (128: the value falls at once).
    pub decay: i32,
    pub lfo_base: i32,
    /// LFO amount, -256..256 (256 = 100 %).
    pub lfo_amount: i32,
    pub lfo_tension: i32,
    /// LFO speed, 200..65536 (the period from 1/96 beat to 128 beats).
    pub speed: i32,
    /// LFO shape, an index into [`SHAPES`].
    pub shape: i32,
    /// LFO phase, 0..65536 = one cycle.
    pub phase: i32,
    pub mute: bool,
    pub ramp: bool,
}

impl Default for Settings {
    /// The original's Default preset.
    fn default() -> Self {
        Settings {
            base: 0,
            amount: 179,
            tension: 0,
            decay: 66,
            lfo_base: 0,
            lfo_amount: 0,
            lfo_tension: 0,
            speed: 32950,
            shape: 0,
            phase: 0,
            mute: false,
            ramp: false,
        }
    }
}

impl Settings {
    /// Reads a saved state: the ten fields (a shorter state is not one), then the Mute and Ramp bytes when present.
    /// Values out of range are put into range.
    pub fn from_state(state: &[u8]) -> Option<Settings> {
        let v = super::ints::<10>(state, 0)?;
        let byte = |k: usize| state.get(k).is_some_and(|&b| b != 0);
        Some(
            Settings {
                base: v[0],
                amount: v[1],
                tension: v[2],
                decay: v[3],
                lfo_base: v[4],
                lfo_amount: v[5],
                lfo_tension: v[6],
                speed: v[7],
                shape: v[8],
                phase: v[9],
                mute: byte(40),
                ramp: byte(41),
            }
            .clamped(),
        )
    }

    /// The state the original saves: [`STATE_LEN`] bytes.
    pub fn to_state(&self) -> Vec<u8> {
        let s = self;
        let mut v = super::state_of(&[s.base, s.amount, s.tension, s.decay, s.lfo_base, s.lfo_amount, s.lfo_tension, s.speed, s.shape, s.phase]);
        v.extend([u8::from(s.mute), u8::from(s.ramp)]);
        v
    }

    pub fn clamped(self) -> Settings {
        Settings {
            base: self.base.clamp(0, 65536),
            amount: self.amount.clamp(-256, 256),
            tension: self.tension.clamp(-128, 128),
            decay: self.decay.clamp(0, 128),
            lfo_base: self.lfo_base.clamp(0, 65536),
            lfo_amount: self.lfo_amount.clamp(-256, 256),
            lfo_tension: self.lfo_tension.clamp(-128, 128),
            speed: self.speed.clamp(200, 65536),
            shape: self.shape.clamp(0, SHAPES.len() as i32 - 1),
            phase: self.phase.clamp(0, 65536),
            ..self
        }
    }
}

/// The Peak amount's factor: `0.4 (6^|a/256| - 1)` with a's sign (179 gives 1.0, 256 gives 2).
pub fn amount_factor(a: i32) -> f64 {
    let x = f64::from(a) / 256.0;
    x.signum() * 0.4 * (6f64.powf(x.abs()) - 1.0)
}

/// The LFO amount's factor: half the Peak amount's law (256 gives 1).
pub fn lfo_factor(a: i32) -> f64 {
    amount_factor(a) * 0.5
}

/// What the peak value falls by per tick: `2 (10000^(d/128) - 1) / 9999`.
pub fn decay_per_tick(d: i32) -> f64 {
    2.0 * (10000f64.powf(f64::from(d) / 128.0) - 1.0) / 9999.0
}

/// A tension curve on 0..1: an exponential bend `M = (1000^(|t|/128) - 1) / 10` (none at 0); negative tensions bend
/// down (`((1 + M)^p - 1) / M`), positive ones up (the mirror image).
pub fn tension(p: f64, t: i32) -> f64 {
    if t == 0 {
        return p;
    }
    let m = (1000f64.powf(f64::from(t.abs()) / 128.0) - 1.0) / 10.0;
    let k = (1.0 + m).ln();
    if t < 0 { ((k * p).exp() - 1.0) / m } else { 1.0 - ((k * (1.0 - p)).exp() - 1.0) / m }
}

/// The LFO's period in beats: `128 (1000^(s/65536) - 1) / 999`.
pub fn lfo_beats(speed: i32) -> f64 {
    128.0 * (1000f64.powf(f64::from(speed) / 65536.0) - 1.0) / 999.0
}

/// The LFO's period in ticks (of 1/96 beat): whole ticks, at least 3.
pub fn lfo_ticks(speed: i32) -> f64 {
    (96.0 * lfo_beats(speed)).round().max(3.0)
}

/// The phase just before `x` (cycles) when `x` is a whole number of them: the LFO's own count ends a cycle a hair late.
const LATE: f64 = 1e-9;

/// The peak follower and LFO, advanced once per tick.
#[derive(Debug, Clone)]
pub struct Core {
    pub s: Settings,
    env: f64,
    /// Ticks played.
    pos: f64,
    /// The random shape's value and the cycle it was drawn for.
    held: (f64, i64),
    seed: u32,
    out: [f64; OUTPUTS],
}

impl Core {
    pub fn new(s: Settings) -> Core {
        let mut c = Core { s, env: 0.0, pos: 0.0, held: (0.0, 0), seed: 0x9E37_79B9, out: [0.0; OUTPUTS] };
        c.held = (c.random(), c.cycle(0.0));
        c.out = c.values(0.0);
        c
    }

    /// Back to the start: the peak value 0, the LFO at its phase.
    pub fn reset(&mut self) {
        *self = Core::new(self.s);
    }

    pub fn set(&mut self, s: Settings) {
        self.s = s;
    }

    /// The outputs (0..1) as they stand: Peak + LFO, Peak, LFO.
    pub fn outputs(&self) -> [f64; OUTPUTS] {
        self.out
    }

    /// The peak value the follower holds (before its amount and tension).
    pub fn envelope(&self) -> f64 {
        self.env
    }

    /// One tick (or `ticks` of them) with the largest sample magnitude `peak` heard in it: the peak value takes the
    /// peak or falls by the decay, the LFO moves on, then the outputs are worked out (and ramp when asked to).
    pub fn advance(&mut self, peak: f64, ticks: f64) {
        let peak = if peak.is_finite() { peak.abs() } else { 0.0 };
        self.env = peak.max(self.env - decay_per_tick(self.s.decay) * ticks).max(0.0);
        self.pos += ticks;
        let c = self.cycle(LATE);
        if c != self.held.1 {
            self.held = (self.random(), c);
        }
        let new = self.values(LATE);
        for (o, n) in self.out.iter_mut().zip(new) {
            *o = if self.s.ramp { *o + (n - *o).clamp(-RAMP_STEP * ticks, RAMP_STEP * ticks) } else { n };
        }
    }

    /// The LFO's phase (cycles) at the current position, `late` before a whole cycle.
    fn phase(&self, late: f64) -> f64 {
        self.pos / lfo_ticks(self.s.speed) + f64::from(self.s.phase) / 65536.0 - late
    }

    fn cycle(&self, late: f64) -> i64 {
        self.phase(late).floor() as i64
    }

    /// The LFO's shape at its phase, 0..1.
    fn lfo(&self, late: f64) -> f64 {
        let p = self.phase(late).rem_euclid(1.0);
        match self.s.shape {
            0 => 0.5 + 0.5 * (std::f64::consts::TAU * p).sin(),
            1 => match p {
                p if p < 0.25 => 0.5 + 2.0 * p,
                p if p < 0.75 => 1.5 - 2.0 * p,
                p => 2.0 * p - 1.5,
            },
            2 => f64::from(u8::from(p < 0.5)),
            3 => p,
            _ => self.held.0,
        }
    }

    fn values(&self, late: f64) -> [f64; OUTPUTS] {
        let s = &self.s;
        let peak = f64::from(s.base) / 65536.0 + amount_factor(s.amount) * tension(self.env, s.tension);
        let lfo = f64::from(s.lfo_base) / 65536.0 + lfo_factor(s.lfo_amount) * tension(self.lfo(late), s.lfo_tension);
        [(peak + lfo).clamp(0.0, 1.0), peak.clamp(0.0, 1.0), lfo.clamp(0.0, 1.0)]
    }

    /// A uniform value in 0..1 (xorshift).
    fn random(&mut self) -> f64 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.seed = x;
        f64::from(x) / f64::from(u32::MAX)
    }
}

/// The largest sample magnitude of an interleaved stereo block.
pub fn block_peak(buf: &[f32]) -> f64 {
    f64::from(buf.iter().fold(0f32, |m, s| if s.is_finite() { m.max(s.abs()) } else { m }))
}

/// A Fruity Peak Controller slot in the engine, which calls `process` once per tick.
pub struct PeakController {
    core: Core,
}

impl PeakController {
    pub fn from_state(state: &[u8]) -> Option<Self> {
        Some(PeakController { core: Core::new(Settings::from_state(state)?) })
    }
}

impl Effect for PeakController {
    fn process(&mut self, buf: &mut [f32]) {
        self.core.advance(block_peak(buf), 1.0);
        if self.core.s.mute {
            buf.fill(0.0);
        }
    }

    fn reset(&mut self) {
        self.core.reset();
    }

    fn control(&self, k: usize) -> Option<f32> {
        self.core.outputs().get(k).map(|&v| v as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn state_round_trip_and_odd_states() {
        let s = Settings { base: 13568, amount: 185, tension: 31, decay: 128, ramp: true, ..Settings::default() };
        let st = s.to_state();
        assert_eq!(st.len(), STATE_LEN);
        assert_eq!(Settings::from_state(&st), Some(s));
        // The demo templates' state, byte for byte.
        let demo = [0u32, 179, 0, 128, 0, 0, 0, 32950, 0, 0].iter().flat_map(|v| v.to_le_bytes()).chain([0, 0]).collect::<Vec<u8>>();
        let d = Settings::from_state(&demo).unwrap();
        assert_eq!((d.amount, d.decay, d.speed), (179, 128, 32950));
        assert_eq!(d.to_state(), demo);
        // Too short: not a state; without the switch bytes: off; out of range: into range.
        assert!(Settings::from_state(&st[..39]).is_none());
        assert!(!Settings::from_state(&st[..40]).unwrap().ramp);
        let wild = [i32::MIN, 9999, -9999, 500, -5, 300, 200, 0, 77, 1 << 20].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let w = Settings::from_state(&wild).unwrap();
        assert_eq!((w.base, w.amount, w.tension, w.decay, w.lfo_base), (0, 256, -128, 128, 0));
        assert_eq!((w.lfo_amount, w.lfo_tension, w.speed, w.shape, w.phase), (256, 128, 200, 4, 65536));
    }

    /// The laws, at the values measured from the original (spec: plugins/fruity-peak-controller.md).
    #[test]
    fn laws() {
        assert!(close(amount_factor(179), 1.0, 0.002));
        assert!(close(amount_factor(256), 2.0, 1e-12));
        assert!(close(amount_factor(128), 0.5797, 2e-4));
        assert!(close(amount_factor(64), 0.2260, 2e-4));
        assert!(close(amount_factor(-128), -0.5797, 2e-4));
        assert!(close(lfo_factor(128), 0.2899, 2e-4));
        assert!(close(decay_per_tick(66), 0.0229, 1e-4));
        assert!(close(decay_per_tick(16), 0.000433, 1e-5));
        assert!(close(decay_per_tick(80), 0.06306, 1e-4));
        assert!(decay_per_tick(128) >= 2.0 - 1e-12);
        assert!(close(tension(0.5, 64), 0.6684, 2e-4));
        assert!(close(tension(0.5, -64), 0.3316, 2e-4));
        assert!(close(tension(0.3896, 128), 0.8423, 5e-4));
        assert!(close(tension(0.3896, -128), 0.0504, 5e-4));
        assert_eq!(tension(0.3, 0), 0.3);
        for t in [-128, -64, -1, 1, 64, 128] {
            assert!(close(tension(0.0, t), 0.0, 1e-12) && close(tension(1.0, t), 1.0, 1e-12), "{t}");
        }
        for (speed, ticks) in [(2760, 4.0), (5000, 9.0), (7880, 16.0), (10272, 24.0), (13000, 36.0), (15087, 48.0), (20643, 96.0), (23240, 130.0), (24109, 144.0), (32950, 384.0)] {
            assert_eq!(lfo_ticks(speed), ticks, "{speed}");
        }
        assert!(close(lfo_beats(65536), 128.0, 1e-9));
    }

    /// The peak follower: the tick's peak at once, then a fall of the decay per tick; the outputs sum Peak and LFO.
    #[test]
    fn follows_peaks_once_per_tick() {
        let mut c = Core::new(Settings::default());
        assert_eq!(c.outputs(), [0.0; 3]);
        c.advance(0.5, 1.0);
        assert!(close(c.outputs()[1], 0.5 * amount_factor(179), 1e-12));
        let top = c.outputs()[1];
        for k in 1..5 {
            c.advance(0.0, 1.0);
            assert!(close(c.outputs()[1], top - k as f64 * decay_per_tick(66) * amount_factor(179), 1e-9));
        }
        // A smaller peak than what is held does not lower it.
        let held = c.envelope();
        c.advance(held * 0.5, 1.0);
        assert!(close(c.envelope(), held - decay_per_tick(66), 1e-12));
        // Decay 128 falls at once.
        let mut c = Core::new(Settings { decay: 128, ..Settings::default() });
        c.advance(0.8, 1.0);
        c.advance(0.0, 1.0);
        assert_eq!(c.outputs()[1], 0.0);
        // Base and a negative amount: 1 - peak; Peak + LFO adds the LFO's value.
        let mut c = Core::new(Settings { base: 65536, amount: -179, lfo_base: 16384, ..Settings::default() });
        c.advance(0.25, 1.0);
        let peak = 1.0 + amount_factor(-179) * 0.25;
        assert!(close(c.outputs()[1], peak, 1e-12));
        assert!(close(c.outputs()[2], 0.25, 1e-12));
        assert!(close(c.outputs()[0], (peak + 0.25).min(1.0), 1e-12));
    }

    /// The LFO: shapes over a 36-tick period, phase, the square's and saw's ends a hair late, ramping.
    #[test]
    fn lfo_shapes() {
        let run = |shape: i32, phase: i32, ramp: bool| {
            let mut c = Core::new(Settings { amount: 0, lfo_amount: 256, speed: 13000, shape, phase, ramp, ..Settings::default() });
            let mut v = vec![c.outputs()[2]];
            for _ in 0..72 {
                c.advance(0.0, 1.0);
                v.push(c.outputs()[2]);
            }
            v
        };
        let sine = run(0, 0, false);
        assert!(close(sine[0], 0.5, 1e-12) && close(sine[9], 1.0, 1e-6) && close(sine[27], 0.0, 1e-6));
        let tri = run(1, 0, false);
        assert!(close(tri[1], 0.5 + 2.0 / 36.0, 1e-6) && close(tri[9], 1.0, 1e-6) && close(tri[27], 0.0, 1e-6));
        let sq = run(2, 0, false);
        assert!(sq[..=18].iter().all(|&v| v == 1.0) && sq[19..=36].iter().all(|&v| v == 0.0) && sq[37] == 1.0);
        let saw = run(3, 0, false);
        assert!(close(saw[1], 1.0 / 36.0, 1e-9) && saw[36] > 0.999 && close(saw[37], 1.0 / 36.0, 1e-6));
        let rnd = run(4, 0, false);
        assert!(rnd[1..=36].iter().all(|&v| v == rnd[1]) && rnd[37] != rnd[36] && rnd[37..=72].iter().all(|&v| v == rnd[37]));
        let quarter = run(0, 16384, false);
        assert!(close(quarter[0], 1.0, 1e-12) && close(quarter[18], 0.0, 1e-6));
        let ramped = run(2, 0, true);
        assert!(close(ramped[19], 1.0 - RAMP_STEP, 1e-12) && close(ramped[20], 1.0 - 2.0 * RAMP_STEP, 1e-12));
    }

    #[test]
    fn passes_or_mutes_the_sound_and_reports_its_outputs() {
        let s = Settings { mute: true, ..Settings::default() };
        let mut fx = PeakController::from_state(&s.to_state()).unwrap();
        let mut buf = vec![0.5, -0.25, 0.1, 0.2];
        fx.process(&mut buf);
        assert_eq!(buf, vec![0.0; 4]);
        assert!(close(f64::from(fx.control(1).unwrap()), 0.5 * amount_factor(179), 1e-6));
        assert_eq!(fx.control(3), None);
        let mut fx = PeakController::from_state(&Settings::default().to_state()).unwrap();
        let mut buf = vec![0.5, f32::NAN, 0.1, -0.7];
        fx.process(&mut buf);
        assert_eq!(buf[0], 0.5);
        assert!(close(f64::from(fx.control(1).unwrap()), 0.7 * amount_factor(179), 1e-6));
    }
}
