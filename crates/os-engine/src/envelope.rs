// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Channel envelopes: delay / attack / hold / decay / sustain / release levels for the volume and
//! pitch records of a sampler channel.
//!
// spec: audio/channel-envelopes.md (measured in FL renders of patched projects)

use os_model::Envelope;

/// Seconds for a rising stage's time knob (delay, attack, hold): `0.0639 * (2^(v / 6553.6) - 2^(100 / 6553.6))`,
/// so the knob's minimum, 100, means none.
pub fn knob_seconds(v: i32) -> f64 {
    let g = |v: f64| 2f64.powf(v / 6553.6);
    (0.063922 * (g(f64::from(v)) - g(100.0))).max(0.0)
}

/// Seconds for a falling stage's knob (decay, release): the rising law plus a short minimum fall (9 ms).
pub fn fall_seconds(v: i32) -> f64 {
    knob_seconds(v) + 0.0090
}

/// Curvature `k` of a tension value, signed like the tension (measured at 64, 101 and 128; linear between).
pub fn tension_k(t: i32) -> f64 {
    const TABLE: [(f64, f64); 4] = [(0.0, 0.0), (64.0, 2.2), (101.0, 4.85), (128.0, 6.3)];
    let a = f64::from(t.clamp(-128, 128)).abs();
    let mut k = TABLE[3].1;
    for w in TABLE.windows(2) {
        if a <= w[1].0 {
            k = w[0].1 + (w[1].1 - w[0].1) * (a - w[0].0) / (w[1].0 - w[0].0);
            break;
        }
    }
    if t < 0 {
        -k
    } else {
        k
    }
}

/// Progress of a move through its stage: `u` in 0..=1 maps to 0..=1; `k` > 0 starts fast, 0 is linear.
pub fn shape(u: f64, k: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    if k.abs() < 1e-6 {
        u
    } else {
        (1.0 - (-k * u).exp()) / (1.0 - (-k).exp())
    }
}

/// An enabled envelope in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    delay: f64,
    attack: f64,
    hold: f64,
    decay: f64,
    release: f64,
    sustain: f64,
    /// Pitch record: cents at full level.
    pub amount: f64,
    att_k: f64,
    dec_k: f64,
    rel_k: f64,
}

impl Params {
    /// `None` when the record is switched off.
    pub fn new(e: &Envelope) -> Option<Self> {
        e.enabled.then(|| Params {
            delay: knob_seconds(e.delay),
            attack: knob_seconds(e.attack),
            hold: knob_seconds(e.hold),
            decay: fall_seconds(e.decay),
            release: fall_seconds(e.release),
            sustain: f64::from(e.sustain.clamp(0, 128)) / 128.0,
            amount: f64::from(e.amount),
            att_k: tension_k(e.attack_tension),
            // At the knob's minimum the short fall is a plain linear fade whatever the tension (measured).
            dec_k: if e.decay > 100 { tension_k(e.decay_tension) } else { 0.0 },
            rel_k: if e.release > 100 { tension_k(e.release_tension) } else { 0.0 },
        })
    }

    /// The same envelope with every stage time multiplied by `f` (tempo-synced envelopes).
    pub fn scaled(mut self, f: f64) -> Self {
        self.delay *= f;
        self.attack *= f;
        self.hold *= f;
        self.decay *= f;
        self.release *= f;
        self
    }

    /// Level while the note is held, `t` seconds after it started.
    fn held(&self, t: f64) -> f64 {
        if t < self.delay {
            return 0.0;
        }
        let mut a = t - self.delay;
        if a < self.attack {
            return shape(a / self.attack, self.att_k);
        }
        a -= self.attack;
        if a < self.hold {
            return 1.0;
        }
        a -= self.hold;
        // The decay moves at full-scale speed: it takes `decay * (1 - sustain)` to reach the sustain.
        let span = self.decay * (1.0 - self.sustain);
        if span > 0.0 && a < span {
            return 1.0 - (1.0 - self.sustain) * shape(a / span, -self.dec_k);
        }
        self.sustain
    }

    /// Level `t` seconds after the note started, the note-off having come at `off` seconds.
    /// `None` once the release has finished.
    pub fn level(&self, t: f64, off: Option<f64>) -> Option<f64> {
        match off {
            Some(o) if t >= o => {
                let from = self.held(o);
                // The release also moves at full-scale speed, so it takes `release * level`.
                let span = self.release * from;
                let r = t - o;
                if span <= 0.0 || r >= span {
                    return None;
                }
                Some(from * (1.0 - shape(r / span, -self.rel_k)))
            }
            _ => Some(self.held(t)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> Envelope {
        Envelope { enabled: true, delay: 100, attack: 100, hold: 100, decay: 100, sustain: 128, release: 100, ..Envelope::default() }
    }

    #[test]
    fn time_knob_matches_measurements() {
        // Falls measured as linear decays to zero; rises from onset / ramp times.
        for (v, s) in [(8000, 0.0925), (22500, 0.6345), (35000, 2.5406), (48000, 10.1716)] {
            let t = fall_seconds(v);
            assert!((t / s - 1.0).abs() < 0.015, "{v}: {t} vs {s}");
        }
        assert!((knob_seconds(20000) - 0.465).abs() < 0.01 && (knob_seconds(5000) - 0.0436).abs() < 0.003);
        assert_eq!(knob_seconds(100), 0.0);
    }

    #[test]
    fn stages_follow_the_measured_shapes() {
        let mut e = rec();
        e.attack = 27500;
        let p = Params::new(&e).unwrap();
        let t = knob_seconds(27500);
        assert!((p.level(t / 2.0, None).unwrap() - 0.5).abs() < 1e-9, "linear attack");
        assert_eq!(p.level(t * 2.0, None), Some(1.0));
        // Sustain 64 of 128, decay to it takes half of the full-scale time.
        e = rec();
        e.decay = 22500;
        e.sustain = 64;
        let p = Params::new(&e).unwrap();
        let t = fall_seconds(22500);
        assert!((p.level(t * 0.25, None).unwrap() - 0.75).abs() < 1e-9);
        assert_eq!(p.level(t, None), Some(0.5));
        // Release from the sustain takes release * level, then the voice is over.
        e.release = 27500;
        let p = Params::new(&e).unwrap();
        let r = fall_seconds(27500);
        assert!((p.level(5.0 + r * 0.25, Some(5.0)).unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(p.level(5.0 + r * 0.5 + 1e-6, Some(5.0)), None);
        // Released mid-attack: continues from the level reached.
        e = rec();
        e.attack = 30000;
        e.release = 22500;
        let p = Params::new(&e).unwrap();
        let a = knob_seconds(30000);
        let from = p.level(a * 0.3, Some(a * 0.3)).unwrap();
        assert!((from - 0.3).abs() < 1e-9);
        assert_eq!(p.level(a * 0.3 + fall_seconds(22500) * 0.3 + 1e-6, Some(a * 0.3)), None);
    }

    #[test]
    fn tension_bends_the_curve() {
        assert_eq!(tension_k(0), 0.0);
        assert!((tension_k(-101) + 4.85).abs() < 1e-9);
        assert!(shape(0.1, tension_k(128)) > 0.4, "fast start");
        assert!(shape(0.5, 0.0) == 0.5);
        // The volume record's default release tension (-101) falls quickly, then tails off.
        let mut e = rec();
        e.release = 27500;
        e.release_tension = -101;
        let p = Params::new(&e).unwrap();
        let r = fall_seconds(27500);
        let l = p.level(r * 0.1, Some(0.0)).unwrap_or(1.0);
        assert!(l < 0.7 && l > 0.5, "{l}");
    }

    #[test]
    fn disabled_record_gives_nothing() {
        assert!(Params::new(&Envelope::default()).is_none());
    }
}
