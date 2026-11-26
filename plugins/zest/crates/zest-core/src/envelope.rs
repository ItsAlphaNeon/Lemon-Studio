// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The two envelopes of `pages/envelope.md`.

use crate::dsp::{exp2, tclip};
use crate::settings::EnvSet;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Curve {
    Moog,
    /// Linear, ends the attack on phase (ADS).
    LinearPhase,
    /// Linear, ends the attack on level (digital).
    LinearLevel,
    Rc,
    Exp,
}

#[derive(Clone, Copy, Debug)]
struct StageDef {
    a: f32,
    b: f32,
    c: f32,
    curve: Curve,
}

const STAGE_IDLE: u8 = 5;

#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    pub v: f32,
    phase: f32,
    pub stage: u8,
    pending: u8,
    trig: bool,
    hold: f32,
    cnt: i32,
    last_t: i64,
    r_last: f32,
}

const NONE: u8 = 255;

impl Default for Envelope {
    fn default() -> Self {
        Envelope { v: 0.0, phase: 0.0, stage: STAGE_IDLE, pending: NONE, trig: false, hold: 0.0, cnt: 0, last_t: 0, r_last: 0.0 }
    }
}

/// d(knob): knob * 0.01 in double, converted to single.
#[inline]
fn d(knob: f32) -> f32 {
    (knob as f64 * 0.01f64) as f32
}

/// S(x, k), the soft clipper with a linear middle.
fn s_clip(x: f32, k: f32) -> f32 {
    let mut y = (k + 1.0) * x;
    if y < -k {
        y = tclip(k + y) - k;
    }
    if y > k {
        y = tclip(y - k) + k;
    }
    y / (k + 1.0)
}

fn digital_curve(v: f32) -> f32 {
    const K: f32 = 0.565;
    let top = s_clip(1.13, K);
    let bot = s_clip(-1.13, K);
    let a = (((if v > 0.0 { v } else { 0.0 }) as f64 * 2.26f32 as f64) - 1.13f32 as f64) as f32;
    let inv = (1.0f64 / (top - bot) as f64) as f32;
    (s_clip(a, K) - bot) * inv
}

/// One call's constants of a stage.
struct Stage {
    r: f32,
    t: f32,
    curve: Curve,
}

impl Envelope {
    fn def(which: usize, model: i32, stage: u8, release_on: bool) -> StageDef {
        match (model, stage) {
            (0, 0) => {
                StageDef { a: 2290.5, b: 0.6, c: 17.8, curve: if which == 0 { Curve::Moog } else { Curve::LinearPhase } }
            }
            (0, 1) => StageDef { a: if which == 0 { 1863.71 } else { 9913.35 }, b: 0.36, c: 21.0, curve: Curve::Exp },
            (0, _) => {
                if release_on {
                    StageDef { a: if which == 0 { 1863.71 } else { 9913.35 }, b: 0.36, c: 21.0, curve: Curve::Exp }
                } else {
                    StageDef { a: 66441.05, b: 66461.05, c: 21.0, curve: Curve::LinearPhase }
                }
            }
            (1, 0) => StageDef { a: 3383.08, b: 1.19, c: 24.0, curve: Curve::Rc },
            (1, _) => StageDef { a: 4400.31, b: 0.85, c: 19.5, curve: Curve::Exp },
            (_, 0) => StageDef { a: 853.92, b: 0.15, c: 13.5, curve: Curve::LinearLevel },
            (_, _) => StageDef { a: 12923.14, b: 0.44, c: 22.5, curve: Curve::Exp },
        }
    }

    #[inline]
    fn rate(which: usize, def: &StageDef, kn: f32, keyoff: f32, slop: f32, dt: f32) -> f32 {
        let mut x = d(kn) + keyoff;
        if which == 1 {
            x = x * (slop * 0.15 + 1.0);
        }
        if x > 1.0 {
            x = 1.0;
        }
        let e = exp2(def.c);
        let xx = exp2((if x > 0.0 { x } else { 0.0 }) * -def.c);
        let inv = (1.0f64 / (e - 1.0) as f64) as f32;
        (((def.a - def.b) * e) * xx + (e * def.b - def.a)) * inv * dt
    }

    fn stage_values(&self, which: usize, set: &EnvSet, peak: f32, keyoff: f32, slop: f32, dt: f32) -> Stage {
        let def = Self::def(which, set.model, self.stage, set.release_on);
        let o: f32 = if set.model == 2 { -0.058 } else { -0.013 };
        let knob = match (self.stage, set.model) {
            (0, _) => set.attack,
            (1, _) => set.decay,
            (4, _) => 0.0,
            (_, 0) => {
                if set.release_on {
                    set.decay
                } else {
                    0.0
                }
            }
            (_, _) => set.release,
        };
        let t = match self.stage {
            0 => {
                let over: f32 = match set.model {
                    0 => 1.3,
                    1 => 1.4,
                    _ => 1.0,
                };
                over * peak
            }
            1 => ((1.0 / (o + 1.0)) * peak) * (set.sustain * 0.01 + o),
            _ => o,
        };
        Stage { r: Self::rate(which, &def, knob, keyoff, slop, dt), t, curve: def.curve }
    }

    #[inline]
    pub fn note_on(&mut self) {
        self.pending = 0;
        self.phase = 0.0;
        self.trig = true;
    }

    #[inline]
    pub fn note_off(&mut self) {
        if self.stage < STAGE_IDLE {
            self.pending = 3;
        }
    }

    #[inline]
    pub fn fast_release(&mut self) {
        if self.stage < STAGE_IDLE {
            self.pending = 4;
        }
    }

    /// One call: writes `out[0 .. 4n]`, returns whether the row was active (the envelope was not idle at the call's start).
    #[allow(clippy::too_many_arguments)]
    pub fn call(&mut self, which: usize, set: &EnvSet, out: &mut [f32], vel: f32, key: f32, slop: f32, dt: f32, now: i64) -> bool {
        out.fill(0.0);
        if self.stage == STAGE_IDLE && self.pending == NONE {
            return false;
        }
        // catch-up after a voice stopped
        let o: f32 = if set.model == 2 { -0.058 } else { -0.013 };
        if now > self.last_t + 16 {
            let gap = (now - self.last_t) as i32 * 16 - 256;
            let m = (gap as f64 * 0.25) as f32;
            let base = (1.0 - self.r_last) as f64;
            let q1 = base.powf(m as f64 - 1.0) as f32;
            let q2 = base.powf(m as f64) as f32;
            self.v = q1 * self.v - (q2 - 1.0) * o;
        }
        self.last_t = now;
        if self.pending != NONE {
            self.stage = self.pending;
            self.pending = NONE;
        }
        let keyoff = -key * (set.key_follow * 0.003);
        let va = set.velocity * 0.01;
        let peak = (exp2(vel) - 1.0) * va + (1.0 - va);
        if self.trig {
            match (set.model, which) {
                (0, 0) => {
                    let x = self.v / (peak + 0.01);
                    self.phase = if x > 1.0 {
                        1.0
                    } else if x > 0.0 {
                        x
                    } else {
                        0.0
                    };
                }
                (0, _) => {
                    if !set.release_on {
                        self.v = 0.0;
                    }
                    self.phase = (tclip(set.attack / 10.0) * 0.5 + 0.5) * self.v;
                }
                (2, _) => self.phase = self.v,
                _ => {}
            }
        }
        let mut st = self.stage_values(which, set, peak, keyoff, slop, dt);
        if self.stage == 3 {
            self.r_last = st.r;
        }
        let n = out.len();
        let mut i = 0;
        while i < n {
            // a. the stage's curve; `ended`: 1 attack, 2 envelope
            let mut ended = 0u8;
            match st.curve {
                Curve::Exp => {
                    self.v = (st.t - self.v) * st.r + self.v;
                    if self.stage == 3 && self.v <= 1e-7 {
                        ended = 2;
                    }
                }
                Curve::LinearPhase | Curve::LinearLevel => {
                    self.phase = self.phase + st.r;
                    self.v = st.r * st.t + self.v;
                    if self.stage == 3 {
                        if self.v <= 1e-7 {
                            ended = 2;
                        }
                    } else if set.model == 0 {
                        if self.phase >= 1.0 {
                            ended = 1;
                        }
                    } else if self.v >= peak {
                        ended = 1;
                    }
                }
                Curve::Rc => {
                    self.v = (1.0 - st.r) * self.v + st.r * st.t;
                    if self.v >= peak {
                        ended = 1;
                    }
                }
                Curve::Moog => {
                    let lim = {
                        let c = if self.v < 0.99 { self.v } else { 0.99 };
                        if c > 0.0 {
                            c
                        } else {
                            0.0
                        }
                    };
                    self.v = lim;
                    let mut w = self.v;
                    let mut passes = 0;
                    loop {
                        let p = if self.phase < 0.99 { self.phase } else { 0.99 };
                        let l = ((1.0 - w) * (st.r * st.t)) / (1.0 - p);
                        let e = (((p * 69.0 - 10.0) as f64).exp()) as f32;
                        if l > (p * 69.0 + 1.0) * e && l > 0.0 {
                            w = (p * st.t) * e;
                        } else {
                            w = l * st.t + self.v;
                        }
                        self.phase = self.phase + st.r;
                        self.v = w;
                        passes += 1;
                        if !(self.trig && self.phase < 1.0 && lim > w && passes < 1001) {
                            break;
                        }
                    }
                    if self.phase >= 1.0 {
                        ended = 1;
                    }
                }
            }
            if ended == 1 {
                self.stage = 1;
                st = self.stage_values(which, set, peak, keyoff, slop, dt);
                continue;
            }
            if ended == 2 {
                self.stage = STAGE_IDLE;
                return true;
            }
            // d. output
            let a = if self.v > 0.0 { self.v } else { 0.0 };
            let mut y = match set.model {
                0 if which == 0 => {
                    let z = tclip(1.3 * self.v) * 1.291;
                    let z = if z < 1.0 { z } else { 1.0 };
                    if z > 0.0 {
                        z
                    } else {
                        0.0
                    }
                }
                2 if set.curve => digital_curve(self.v),
                _ => a,
            };
            if set.model == 2 && set.quantise {
                if self.cnt > 24 {
                    self.cnt = 1;
                    self.hold = y;
                } else {
                    if self.cnt == 0 {
                        self.hold = y;
                    }
                    self.cnt += 1;
                    y = self.hold;
                }
            }
            out[i] = y;
            self.trig = false;
            i += 1;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate_of(which: usize, model: i32, stage: u8, kn: f32) -> f32 {
        let def = Envelope::def(which, model, stage, true);
        Envelope::rate(which, &def, kn, 0.0, 0.0, 1.0 / 48000.0)
    }

    #[test]
    fn page_rates() {
        let close = |a: f32, b: f32| (a / b - 1.0).abs() < 2e-6;
        assert!(close(rate_of(0, 0, 0, 25.0), 0.002194806));
        assert!(close(rate_of(0, 0, 1, 50.0), 3.4285476e-05));
        assert!(close(rate_of(1, 0, 1, 25.0), 0.0054345112));
        assert!(close(rate_of(0, 1, 0, 0.0), 0.070480838));
        assert!(close(rate_of(0, 1, 1, 75.0), 2.1211947e-05));
        assert!(close(rate_of(0, 2, 0, 50.0), 0.00016686719));
        assert!(close(rate_of(0, 2, 1, 100.0), 9.1666589e-06));
    }

    #[test]
    fn release_off_is_quick() {
        let def = Envelope::def(0, 0, 3, false);
        let r = Envelope::rate(0, &def, 0.0, 0.0, 0.0, 1.0 / 48000.0);
        assert!((r - 1.3841884).abs() < 1e-5);
    }
}
