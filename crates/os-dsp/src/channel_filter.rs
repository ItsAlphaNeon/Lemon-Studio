// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The channel filter a voice runs through: a fast resonant low-pass, four biquads, the low-pass twice, and a
//! state-variable low-pass single and doubled. The cutoff is mod X and the resonance mod Y; the coefficients are
//! worked out once per tick from the knob positions.
// spec: audio/channel-filter.md

/// A level below this (2^-24) left in the state-variable filter at a tick's end is set to 0.
const TINY: f64 = 1.0 / 16_777_216.0;

/// A knob position (spec "Knob positions"): `single(single(note + knob) + part)`, with `knob` the stored knob and the
/// channel's offset as one single (`single((cutoff + offset) / 256)`), `note` the note's own mod X / Y and `part` the
/// envelope's and LFO's.
pub fn position(note: f32, knob: f32, part: f32) -> f32 {
    (note + knob) + part
}

/// `exp(single(z * c)) - 1` in doubles, `c` a single.
fn e(z: f32, c: f32) -> f64 {
    f64::from(z * c).exp() - 1.0
}

/// One voice's filter, both channels.
#[derive(Debug, Clone)]
pub struct Filter {
    /// Type 0..7 (anything else plays as type 0).
    kind: i32,
    /// `single(44100 / rate)`.
    k: f32,
    /// The fast low-pass's glide limit per sample.
    limit: f64,
    /// Samples run since the tick started.
    done: u64,
    /// The voice's first tick is still to come.
    first: bool,
    /// Type 0: off until a setting first gives `c != 1` or `r != 0`.
    on: bool,
    /// Type 0: `c` at the tick's start (a single), its step per sample, `r`.
    c: f64,
    step: f64,
    r: f64,
    /// Biquad coefficients b0, b1, b2, a1, a2.
    coef: [f32; 5],
    /// Type 5's correction of the voice's gains.
    gain: f32,
    /// State-variable `f` and `d` (and the second stage's `d`).
    f: f64,
    d: [f64; 2],
    /// Per stage and channel: type 0 `v, o`; the biquads `x1, x2, y1, y2`; the state-variable `low, band`.
    st: [[[f64; 4]; 2]; 2],
}

impl Filter {
    pub fn new(kind: i32, rate: u32) -> Filter {
        Filter {
            kind: if (0..=7).contains(&kind) { kind } else { 0 },
            k: (44100.0 / f64::from(rate.max(1))) as f32,
            limit: f64::from(crate::glide::engine_limit(rate)),
            done: 0,
            first: true,
            on: false,
            c: 1.0,
            step: 0.0,
            r: 0.0,
            coef: [1.0, 0.0, 0.0, 0.0, 0.0],
            gain: 1.0,
            f: 0.0,
            d: [1.0, 1.0],
            st: [[[0.0; 4]; 2]; 2],
        }
    }

    pub fn kind(&self) -> i32 {
        self.kind
    }

    /// The correction of the voice's gains (type 5; 1 for the other types).
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// A tick starts: the coefficients from the knob positions `x` (cutoff) and `y` (resonance). `nominal` is the
    /// tick's nominal length in samples (the fast low-pass's glide). A changed type starts with empty memory.
    pub fn tick(&mut self, kind: i32, x: f32, y: f32, nominal: f64) {
        let kind = if (0..=7).contains(&kind) { kind } else { 0 };
        if kind != self.kind {
            self.kind = kind;
            self.st = [[[0.0; 4]; 2]; 2];
            self.gain = 1.0;
        }
        // The last tick ends: the fast low-pass's `c` where its glide took it; the state-variable memory as singles.
        if !self.first {
            self.c = f64::from((self.c + self.step * self.done as f64) as f32);
            if matches!(self.kind, 6 | 7) {
                for v in self.st.iter_mut().flatten().flat_map(|s| &mut s[..2]) {
                    let s = f64::from(*v as f32);
                    *v = if s.abs() < TINY { 0.0 } else { s };
                }
            }
        }
        self.done = 0;
        match self.kind {
            1..=5 => self.biquad(x, y),
            6 | 7 => self.state_variable(x, y),
            _ => self.fast(x, y, nominal),
        }
        self.first = false;
    }

    /// Type 0's `c` and `r` (spec "Type 0").
    fn fast(&mut self, x: f32, y: f32, nominal: f64) {
        let ln601 = 601f64.ln() as f32;
        let sixth = 1.0f32 / 600.0;
        let target = if x <= 0.0 {
            0.0
        } else if x < 1.0 {
            (f64::from((e(x, ln601) * f64::from(sixth)) as f32) * f64::from(self.k)).min(1.0)
        } else {
            1.0
        };
        self.r = if y > 0.0 {
            let z = (1.0 - 0.625 * f64::from(y.min(1.0))) as f32;
            1.0 - f64::from((e(z, ln601) * f64::from(sixth)) as f32)
        } else {
            0.0
        };
        if !self.on && (target != 1.0 || self.r != 0.0) {
            self.on = true;
            self.st = [[[0.0; 4]; 2]; 2];
        }
        if self.first {
            self.c = target;
            self.step = 0.0;
        } else {
            self.step = ((target - self.c) / nominal.max(1.0)).clamp(-self.limit, self.limit);
        }
    }

    /// Types 1 to 5 (spec "Types 1 to 5").
    fn biquad(&mut self, x: f32, y: f32) {
        let k = f64::from(self.k);
        let ln51 = 51f64.ln() as f32;
        let w = ((e(x, ln51) * f64::from(0.0596f32) + 0.02) * k) as f32;
        let w = w.max((0.02 * k) as f32).min(3.0);
        let q = if y > 0.0 { (e(y.min(1.0), (0.625 * 51f64.ln()) as f32) * f64::from(0.4f32) + 0.1) as f32 } else { 0.1 };
        let sn = f64::from(w).sin() as f32;
        let cs = f64::from(f64::from(w).cos() as f32);
        let alpha = sn / (2.0 * q);
        let n = 1.0 / (f64::from(alpha) + 1.0);
        let a1 = (n * cs * -2.0) as f32;
        let a2 = (n * (1.0 - f64::from(alpha))) as f32;
        self.coef = match self.kind {
            2 => {
                let b0 = (n * f64::from(alpha)) as f32;
                [b0, 0.0, -b0, a1, a2]
            }
            3 => {
                let b1 = (-(n * (cs + 1.0))) as f32;
                [-b1 * 0.5, b1, -b1 * 0.5, a1, a2]
            }
            4 => {
                let b0 = n as f32;
                [b0, a1, b0, a1, a2]
            }
            _ => {
                let b1 = (n * (1.0 - cs)) as f32;
                [b1 * 0.5, b1, b1 * 0.5, a1, a2]
            }
        };
        self.gain = if self.kind == 5 { (1.0 - f64::from(q) * (3.2 - f64::from(w)) * 0.046875) as f32 } else { 1.0 };
    }

    /// Types 6 and 7 (spec "Types 6 and 7").
    fn state_variable(&mut self, x: f32, y: f32) {
        self.f = if x > 0.0 {
            let s = ((f64::from(x) * 26f64.ln()).exp() - 1.0) as f32;
            f64::from((s * 0.04f32 * self.k).min(1.0))
        } else {
            0.0
        };
        let d = if y <= 0.0 {
            1.0
        } else if y < 1.0 {
            1.0 - 0.8 * y
        } else {
            0.2
        };
        self.d = [f64::from(d), f64::from(d + (1.0 - d) * 0.25)];
    }

    /// Runs `buf` (interleaved stereo) through the filter, in place, going on with the tick.
    pub fn run(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        match self.kind {
            1..=5 => {
                let [b0, b1, b2, a1, a2] = self.coef.map(f64::from);
                let stages = if self.kind == 5 { 2 } else { 1 };
                for frame in buf.chunks_exact_mut(2) {
                    for (ch, s) in frame.iter_mut().enumerate() {
                        let mut x = *s;
                        for stage in 0..stages {
                            let m = &mut self.st[stage][ch];
                            let y = (f64::from(x) * b0 + (((m[1] * b2 + m[0] * b1) - m[2] * a1) - m[3] * a2)) as f32;
                            *m = [f64::from(x), m[0], f64::from(y), m[2]];
                            x = y;
                        }
                        *s = x;
                    }
                }
            }
            6 | 7 => {
                let stages = if self.kind == 7 { 2 } else { 1 };
                let f = self.f;
                for frame in buf.chunks_exact_mut(2) {
                    for (ch, s) in frame.iter_mut().enumerate() {
                        let mut x = *s;
                        for stage in 0..stages {
                            let d = self.d[stage];
                            let m = &mut self.st[stage][ch];
                            m[0] += m[1] * f;
                            m[1] += (f64::from(x) - (m[1] * d + m[0])) * f;
                            x = m[0] as f32;
                        }
                        *s = x;
                    }
                }
            }
            _ => {
                if self.on {
                    for (j, frame) in buf.chunks_exact_mut(2).enumerate() {
                        let c = self.c + (self.done + j as u64) as f64 * self.step;
                        for (ch, s) in frame.iter_mut().enumerate() {
                            let m = &mut self.st[0][ch];
                            m[0] += (f64::from(*s) - m[1]) * c;
                            m[1] += m[0];
                            m[0] *= self.r;
                            *s = m[1] as f32;
                        }
                    }
                }
            }
        }
        self.done += frames as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `kind` with knob positions `x`, `y` over `input` (mono, copied to both channels) in ticks of `tick` frames.
    fn run(kind: i32, x: f32, y: f32, input: &[f32], tick: usize) -> Vec<f32> {
        let mut f = Filter::new(kind, 44100);
        let mut out = Vec::new();
        for block in input.chunks(tick) {
            f.tick(kind, x, y, tick as f64);
            let mut buf: Vec<f32> = block.iter().flat_map(|&s| [s, s]).collect();
            f.run(&mut buf);
            out.extend(buf.chunks(2).map(|c| {
                assert_eq!(c[0], c[1]);
                c[0]
            }));
        }
        out
    }

    fn noise(n: usize) -> Vec<f32> {
        let mut seed = 7u32;
        (0..n)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    }

    /// The energy of the first difference (the highs).
    fn highs(v: &[f32]) -> f64 {
        v.windows(2).map(|w| f64::from(w[1] - w[0]).powi(2)).sum()
    }

    /// A new channel's settings (cutoff 256, resonance 0: x = 1, y = 0) leave type 0 off: the voice passes unfiltered.
    #[test]
    fn default_settings_do_not_filter() {
        let n = noise(4000);
        assert_eq!(run(0, 1.0, 0.0, &n, 220), n);
    }

    /// Every type at a low cutoff takes highs out of noise (the high-pass the lows, the band-stop a band).
    #[test]
    fn every_type_filters() {
        let n = noise(20000);
        let base = highs(&n);
        for kind in [0, 1, 5, 6, 7] {
            let out = run(kind, 0.3, 0.2, &n, 220);
            assert!(out.iter().all(|s| s.is_finite()));
            assert!(highs(&out) < base * 0.2, "type {kind}: {} vs {base}", highs(&out));
        }
        let hp = run(3, 0.3, 0.2, &n, 220);
        assert!(highs(&hp) > base * 0.5);
        let bp = run(2, 0.3, 0.2, &n, 220);
        assert!(highs(&bp) < base * 0.5);
        // Fully open (with some resonance: Q 0.1 at none is a dull filter) the low-pass passes nearly everything.
        let open = run(1, 1.0, 0.5, &n, 220);
        assert!(highs(&open) > base * 0.5);
    }

    /// Type 0's coefficients: `c = min(1, single(E(x) / 600) * k)`, `r = 0` without resonance; the first tick sets `c`,
    /// later ticks glide at most the engine's limit per sample.
    #[test]
    fn the_fast_low_pass_glides_its_cutoff() {
        let mut f = Filter::new(0, 44100);
        f.tick(0, 0.5, 0.0, 220.0);
        let want = ((f64::from(0.5f32 * 601f64.ln() as f32).exp() - 1.0) * f64::from(1.0f32 / 600.0)) as f32;
        assert_eq!(f.c, f64::from(want));
        assert!(f.on);
        let mut buf = vec![0.0f32; 440];
        f.run(&mut buf);
        f.tick(0, 1.0, 0.0, 220.0);
        assert_eq!(f.step, 0.001_f32 as f64);
        f.run(&mut buf);
        f.tick(0, 1.0, 0.0, 220.0);
        // After a tick of 220 samples: single(c + 220 * step).
        assert_eq!(f.c, f64::from((f64::from(want) + 220.0 * 0.001_f32 as f64) as f32));
    }

    /// Type 5 corrects the voice's gains by `single(1 - Q * (3.2 - w) * 0.046875)`; the other types leave them.
    #[test]
    fn type_5_corrects_the_gain() {
        let mut f = Filter::new(5, 44100);
        f.tick(5, 0.5, 0.5, 220.0);
        assert!(f.gain() < 1.0 && f.gain() > 0.8, "{}", f.gain());
        f.tick(1, 0.5, 0.5, 220.0);
        assert_eq!(f.gain(), 1.0);
    }

    /// The state-variable memory is rounded to singles at a tick's end, and tiny values become 0.
    #[test]
    fn state_variable_memory_is_single_between_ticks() {
        let mut f = Filter::new(6, 44100);
        f.tick(6, 0.5, 0.5, 220.0);
        let mut buf: Vec<f32> = noise(440).iter().flat_map(|&s| [s, s]).collect();
        f.run(&mut buf);
        f.tick(6, 0.5, 0.5, 220.0);
        for v in f.st.iter().flatten().flat_map(|s| &s[..2]) {
            assert_eq!(*v, f64::from(*v as f32));
        }
        f.st[0][0][0] = 1e-9;
        f.tick(6, 0.5, 0.5, 220.0);
        assert_eq!(f.st[0][0][0], 0.0);
    }
}
