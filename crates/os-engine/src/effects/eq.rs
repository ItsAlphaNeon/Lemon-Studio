// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's 7-band parametric EQ: the processing of the original's every mode (plain, HQ Legacy, HQ+, HQ
//! Enhanced and LIN), shared by the engine's own slot effect and the Lemon Parametric EQ plugin, the editor's settings
//! and curves, and the effect built from a project's Parametric EQ 2 saved state.
//!
// spec: plugins/fruity-parametric-eq-2.md, plugins/parametric-eq-2.md (the state's option table)

pub mod design;
mod equalizer;
mod fft;
pub(crate) mod hq;
pub mod lin;
mod matched;

use std::f64::consts::TAU;

pub use design::{BandCoefs, Coefs, Design, Stored};
pub use equalizer::{Equalizer, Mode, Options, MAIN, PARAMS};

use super::Effect;

pub const BANDS: usize = 7;
/// Saved-state size and version (spec: "Saved state").
pub const STATE_LEN: usize = 354;
pub const STATE_VERSION: i32 = 8;
/// Byte of the band enable mask (bit k = band k+1 audible; mute / solo).
pub const MASK_BYTE: usize = 318;

/// Band types (stored values).
pub const DISABLED: i32 = 0;
pub const LOW_PASS: i32 = 1;
pub const BAND_PASS: i32 = 2;
pub const HIGH_PASS: i32 = 3;
pub const BAND_STOP: i32 = 4;
pub const LOW_SHELF: i32 = 5;
pub const PEAKING: i32 = 6;
pub const HIGH_SHELF: i32 = 7;

/// Types of a new instance's bands.
pub const DEFAULT_KINDS: [i32; BANDS] = [LOW_SHELF, PEAKING, PEAKING, PEAKING, PEAKING, PEAKING, HIGH_SHELF];
/// Level range in dB (both band levels and the main level).
pub const MAX_DB: f64 = 18.0;

/// Frequency of a frequency position `n` (0..1 = 20 Hz .. 20 kHz).
pub fn freq_hz(n: f64) -> f64 {
    20.0 * 1000f64.powf(n)
}

/// Frequency position of `hz`.
pub fn freq_pos(hz: f64) -> f64 {
    ((hz.max(1e-6) / 20.0).ln() / 1000f64.ln()).clamp(0.0, 1.0)
}

/// Frequency position of band `k` in a new instance (and after Reset).
pub fn default_freq(k: usize) -> f64 {
    (k as f64 + 1.5) / 9.0
}

/// Width a band of type `kind` resets to (stored values / 65536).
pub fn default_width(kind: i32) -> f64 {
    let stored = match kind {
        LOW_PASS | HIGH_PASS => 36068,
        LOW_SHELF | HIGH_SHELF => 40124,
        _ => 25412,
    };
    f64::from(stored) / 65536.0
}

/// True when the band's level knob does something (shelves and peaking).
pub fn has_level(kind: i32) -> bool {
    matches!(kind, LOW_SHELF | PEAKING | HIGH_SHELF)
}

/// A stored value from an editor value: `Round(v * scale)`, halves to even.
fn stored_of(v: f64, scale: f64) -> i32 {
    (v * scale).round_ties_even() as i32
}

/// One EQ band's settings as the editor shows them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    /// Level in dB (shelves and peaking).
    pub db: f64,
    /// Frequency position 0..1 (see [`freq_hz`]).
    pub freq: f64,
    /// Width 0..1.
    pub width: f64,
    pub kind: i32,
    /// -3 Steep 8 .. 0 "2" .. 3 Gentle 8.
    pub order: i32,
}

impl Band {
    pub fn default_for(k: usize) -> Band {
        let kind = DEFAULT_KINDS[k];
        Band { db: 0.0, freq: default_freq(k), width: default_width(kind), kind, order: 0 }
    }

    pub fn hz(&self) -> f64 {
        freq_hz(self.freq)
    }

    /// The values the state stores (each within its range).
    pub fn stored(&self) -> Stored {
        Stored {
            level: stored_of(self.db, 100.0),
            freq: stored_of(self.freq, 65536.0),
            width: stored_of(self.width, 65536.0),
            kind: self.kind,
            order: self.order,
        }
        .clamped()
    }

    /// The band's sections as `design` gives them.
    pub fn coefs(&self, design: Design) -> BandCoefs {
        design::band(&self.stored(), design)
    }

    /// The band's response at `hz` in dB.
    pub fn response_db(&self, hz: f64, design: Design) -> f64 {
        sections_db(&self.coefs(design), hz, design.rate())
    }
}

/// The response of a band's sections at `hz` in dB.
pub fn sections_db(c: &BandCoefs, hz: f64, rate: f64) -> f64 {
    c.as_slice().iter().map(|s| 10.0 * s.power(TAU * hz / rate).max(1e-24).log10()).sum()
}

/// The settings that shape the sound: seven bands, the main level and the band enable mask.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub bands: [Band; BANDS],
    pub main_db: f64,
    /// Bit k = band k audible.
    pub mask: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { bands: std::array::from_fn(Band::default_for), main_db: 0.0, mask: 0x7f }
    }
}

/// The audible bands' sections and the main level, worked out once for drawing a curve.
pub struct Curve {
    bands: Vec<BandCoefs>,
    main_db: f64,
    rate: f64,
}

impl Curve {
    /// The response at `hz` in dB.
    pub fn db(&self, hz: f64) -> f64 {
        self.main_db + self.bands.iter().map(|c| sections_db(c, hz, self.rate)).sum::<f64>()
    }
}

impl Settings {
    pub fn audible(&self, k: usize) -> bool {
        self.mask & (1 << k) != 0
    }

    /// The 36 parameters as the state stores them (fields 1..36).
    pub fn params(&self) -> [i32; PARAMS] {
        let mut p = [0; PARAMS];
        for (k, b) in self.bands.iter().enumerate() {
            let s = b.stored();
            [p[k], p[BANDS + k], p[2 * BANDS + k], p[3 * BANDS + k], p[4 * BANDS + k]] = [s.level, s.freq, s.width, s.kind, s.order];
        }
        p[MAIN] = stored_of(self.main_db, 100.0).clamp(-1800, 1800);
        p
    }

    /// The total response (audible bands and the main level) for `design`.
    pub fn curve(&self, design: Design) -> Curve {
        let bands = (0..BANDS).filter(|&k| self.audible(k)).map(|k| self.bands[k].coefs(design)).collect();
        Curve { bands, main_db: self.main_db, rate: design.rate() }
    }

    /// The total response at `hz` in dB.
    pub fn response_db(&self, hz: f64, design: Design) -> f64 {
        self.curve(design).db(hz)
    }
}

/// The engine's own slot effect: the original's processing for a project's saved state, with the buffers of silence FL
/// runs before a render already run (so LIN starts at full level, as in FL's exports).
pub struct ParametricEq {
    eq: Equalizer,
    l: Vec<f32>,
    r: Vec<f32>,
}

impl ParametricEq {
    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<Self> {
        let mut eq = Equalizer::from_state(state, f64::from(sample_rate))?;
        eq.preroll();
        Some(ParametricEq { eq, l: Vec::new(), r: Vec::new() })
    }

    pub fn equalizer(&self) -> &Equalizer {
        &self.eq
    }

    /// The delay the original reports, in frames: `3 N / 4` in LIN, 44 in HQ Enhanced, 0 otherwise. The engine
    /// compensates it through `Effect::latency`.
    pub fn latency(&self) -> usize {
        self.eq.latency()
    }
}

impl Effect for ParametricEq {
    fn reset(&mut self) {
        self.eq.reset();
        self.eq.preroll();
    }

    fn latency(&self) -> usize {
        self.eq.latency()
    }

    fn process(&mut self, buf: &mut [f32]) {
        self.l.clear();
        self.r.clear();
        self.l.extend(buf.chunks_exact(2).map(|f| f[0]));
        self.r.extend(buf.chunks_exact(2).map(|f| f[1]));
        self.eq.process(&mut self.l, &mut self.r);
        for (f, (&l, &r)) in buf.chunks_exact_mut(2).zip(self.l.iter().zip(&self.r)) {
            f[0] = l;
            f[1] = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_law_and_defaults() {
        assert!((freq_hz(0.0) - 20.0).abs() < 1e-9 && (freq_hz(1.0) - 20000.0).abs() < 1e-6);
        assert!((freq_pos(440.0) - (22.0f64.ln() / 1000f64.ln())).abs() < 1e-12);
        // Stored default frequencies 10923 .. 54613.
        let stored: Vec<i32> = (0..BANDS).map(|k| Band::default_for(k).stored().freq).collect();
        assert_eq!(stored, vec![10923, 18204, 25486, 32768, 40050, 47332, 54613]);
        assert_eq!(Settings::default().params()[2 * BANDS], 40124);
        // Editor values round to the stored ones, halves to even.
        let b = Band { db: 0.125, ..Band::default_for(0) };
        assert_eq!(b.stored().level, 12);
        assert_eq!(Band { db: 30.0, ..b }.stored().level, 1800);
    }

    #[test]
    fn curves_follow_the_design() {
        let rate = 44100.0;
        let b = Band { db: 12.0, freq: freq_pos(1000.0), width: 0.5, kind: PEAKING, order: 0 };
        let hz = f64::from(design::freq_hz(b.stored().freq));
        assert!((b.response_db(hz, Design::Ordinary(rate)) - 12.0).abs() < 1e-3);
        assert!((b.response_db(hz, Design::Matched(rate)) - 12.0).abs() < 1e-3);
        // Near the top the matched design keeps the prototype's shape (about +10 dB at the Nyquist frequency here),
        // where the bilinear one comes back to 0 dB.
        let top = Band { freq: freq_pos(15000.0), ..b };
        let f = 22049.0;
        assert!(top.response_db(f, Design::Matched(rate)) > 8.0, "{}", top.response_db(f, Design::Matched(rate)));
        assert!(top.response_db(f, Design::Ordinary(rate)).abs() < 0.1);
        // A shelf at 0 dB is (very nearly) flat.
        assert!(Band::default_for(0).response_db(50.0, Design::Ordinary(rate)).abs() < 1e-9);
    }

    #[test]
    fn mask_leaves_bands_out() {
        let mut s = Settings::default();
        s.bands[3].db = 6.0;
        let f = f64::from(design::freq_hz(s.bands[3].stored().freq));
        let d = Design::Ordinary(96000.0);
        assert!((s.response_db(f, d) - 6.0).abs() < 0.01);
        s.mask = 0x7f & !(1 << 3);
        assert!(s.response_db(f, d).abs() < 1e-6);
    }

    /// The slot effect plays the state's mode: HQ Enhanced is the input 44 frames late, scaled by the decimator's
    /// centre tap.
    #[test]
    fn slot_effect_plays_the_state() {
        let mut st = vec![0u8; STATE_LEN];
        let set = |b: &mut [u8], at: usize, v: i32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        set(&mut st, 0, 8);
        for k in 0..BANDS {
            set(&mut st, 4 * (1 + BANDS + k), 32768);
        }
        set(&mut st, 292, 1);
        st[317] = 0x02;
        st[MASK_BYTE] = 0x7f;
        let mut fx = ParametricEq::from_state(&st, 44100).expect("state");
        assert_eq!(fx.latency(), 44);
        assert_eq!(fx.equalizer().mode(), Mode::HqEnhanced);
        let x: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.01).sin() * 0.3).collect();
        let mut buf: Vec<f32> = x.iter().flat_map(|&v| [v, -v]).collect();
        fx.process(&mut buf);
        for i in 44..1000 {
            assert!((buf[2 * i] - x[i - 44]).abs() < 1e-3 && (buf[2 * i + 1] + x[i - 44]).abs() < 1e-3, "{i}");
        }
        assert!(ParametricEq::from_state(&st[..100], 44100).is_none());
    }
}
