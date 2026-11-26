// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The two LFOs of `pages/lfo.md`.

use crate::dsp::{exp2, Mwc};
use crate::settings::LfoSet;

#[derive(Clone, Copy, Debug, Default)]
pub struct LfoState {
    pub ph: f64,
    pub fade: f32,
    pub sh0: f32,
    pub sh1: f32,
    pub hi: bool,
    pub want_sync: bool,
}

/// The one state shared by the voices with Restart "single".
#[derive(Clone, Copy, Debug)]
pub struct SingleLfo {
    pub state: LfoState,
    pub stored: [f32; 16],
    /// The last value it computed.
    pub last: f32,
    pub valid: i64,
    pub reset_voice: i32,
    /// The channel's restart flag ("armed").
    pub armed: bool,
}

impl Default for SingleLfo {
    fn default() -> Self {
        SingleLfo { state: LfoState::default(), stored: [0.0; 16], last: 0.0, valid: 0, reset_voice: -1, armed: true }
    }
}

/// The song clock of a span.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clock {
    pub tempo: f64,
    pub beats: f64,
    /// Chunk counter of the block start.
    pub time: i64,
}

const CYCLES_PER_BEAT: [f64; 24] = [
    16.0,
    8.0,
    4.0,
    2.0,
    1.0,
    0.5,
    0.25,
    16.0 / 3.0,
    8.0 / 3.0,
    4.0 / 3.0,
    2.0 / 3.0,
    1.0 / 3.0,
    6.0,
    3.0,
    1.5,
    0.75,
    0.375,
    0.125,
    1.0 / 12.0,
    0.0625,
    0.05,
    1.0 / 24.0,
    1.0 / 28.0,
    0.03125,
];

impl LfoState {
    /// The restart of a note-on with retrigger; `restart` is the Restart setting (single behaves as gate on the voice's
    /// own state, unused).
    pub fn note_on(&mut self, restart: i32, rng: &mut Mwc) {
        match restart {
            0 => {
                self.sh0 = rng.unit() * 2.0 - 1.0;
                self.sh1 = rng.unit() * 2.0 - 1.0;
                self.hi = false;
                self.fade = 0.0;
                self.want_sync = true;
            }
            3 => {
                self.ph = 0.0;
                self.sh0 = rng.unit() * 2.0 - 1.0;
                self.sh1 = rng.unit() * 2.0 - 1.0;
                self.hi = false;
                self.ph = rng.unit() as f64;
                self.fade = 0.0;
            }
            _ => self.reset_gate(rng),
        }
    }

    /// One voice's part of the modules' reset (`lfo.md`, Reset).
    pub fn module_reset(&mut self, rng: &mut Mwc) {
        self.ph = 0.0;
        self.hi = false;
        self.sh0 = rng.unit() * 2.0 - 1.0;
        self.sh1 = rng.unit() * 2.0 - 1.0;
    }

    pub fn reset_gate(&mut self, rng: &mut Mwc) {
        self.ph = 0.0;
        self.sh0 = rng.unit() * 2.0 - 1.0;
        self.sh1 = rng.unit() * 2.0 - 1.0;
        self.hi = false;
        self.fade = 0.0;
    }

    /// One call of n chunks: `fm`, `dm` the modulators at the call's last step; writes `out[..4n]`.
    #[allow(clippy::too_many_arguments)]
    pub fn call(&mut self, set: &LfoSet, fm: f32, dm: f32, n: usize, fs: f32, dts: f32, clock: &Clock, rng: &mut Mwc, out: &mut [f32]) {
        let r = fm * set.freq_amt + set.rate;
        let e = exp2(r);
        let dtd = 1.0f64 / fs as f64;
        let bpm = clock.tempo;
        let mut inc = if set.sync < 0 {
            let f: f64 = match set.sync {
                -3 => 10.0,
                -2 => 1.0,
                _ => 0.1f32 as f64,
            };
            f * 4.0 * dtd * e as f64
        } else {
            let b = CYCLES_PER_BEAT[set.sync.clamp(0, 23) as usize];
            (b * 4.0 * dtd * bpm) / 60.0 * e as f64
        };
        if inc > 0.1 {
            inc = 0.1;
        }
        if !(inc > 0.0) {
            inc = 0.0;
        }
        if self.want_sync {
            self.want_sync = false;
            let cycles = (inc * clock.beats) / ((dtd * 4.0 * bpm) / 60.0);
            self.ph = cycles - cycles.floor();
        }
        let pk = (set.phase * 0.01) as f64;
        let d3 = set.depth_amt * 0.01;
        let d4 = d3 * dm;
        let depth = (1.0 - d3) + d4;
        let x0 = (2.5 * set.delay) * 0.01;
        let x1 = x0 * x0 * x0;
        let fstep = (n as f32 * 4.0) / ((x1 * 4.0 + dts) * fs);
        for o in out.iter_mut() {
            self.ph += inc;
            if self.ph > 1.0 {
                self.ph -= 1.0;
            }
            let mut p = pk + self.ph;
            if p > 1.0 {
                p -= 1.0;
            }
            let mut w: f32 = match set.wave {
                0 => (p * 6.2831854820251465f64).sin() as f32,
                1 => (1.0 - (p - 0.5).abs() * 4.0) as f32,
                2 => (2.0 * p - 1.0) as f32,
                3 => (1.0 - 2.0 * p) as f32,
                4 => {
                    if p > 0.5 {
                        -1.0
                    } else {
                        1.0
                    }
                }
                5 => {
                    if p > 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                6 => {
                    let h = p > 0.5;
                    if h != self.hi {
                        self.sh0 = self.sh1;
                        self.hi = h;
                        self.sh1 = rng.unit() * 2.0 - 1.0;
                    }
                    self.sh0
                }
                _ => {
                    let h = p > 0.5;
                    if h != self.hi {
                        self.sh0 = self.sh1;
                        self.hi = h;
                        self.sh1 = rng.unit() * 2.0 - 1.0;
                    }
                    let k = if h { 0.5 } else { 0.0 };
                    let a = (p - k) as f32;
                    (a + a) * (self.sh1 - self.sh0) + self.sh0
                }
            };
            if set.positive {
                w = (w + 1.0) * 0.5;
            }
            if self.fade < 1.0 {
                let f = self.fade + fstep;
                self.fade = if f < 1.0 { f } else { 1.0 };
            }
            let y = w * (self.fade * self.fade);
            *o = y * depth;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::MWC_SEED;

    #[test]
    fn startup_increment() {
        let set = LfoSet { sync: -2, restart: 1, wave: 2, rate: 1.6, ..Default::default() };
        let mut s = LfoState::default();
        let mut rng = MWC_SEED;
        let mut out = [0.0; 16];
        let c = Clock { tempo: 120.0, beats: 0.0, time: 0 };
        s.call(&set, 0.0, 0.0, 4, 48000.0, 1.0 / 48000.0, &c, &mut rng, &mut out);
        assert!((s.ph / 16.0 - 0.00025260).abs() < 1e-7);
    }
}
