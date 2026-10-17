// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's 7-band parametric EQ: the band filter designs (shared by the engine's own slot effect and the Open
//! Parametric EQ 2 plugin) and the effect built from a project's Parametric EQ 2 saved state.
//!
// spec: plugins/parametric-eq-2.md

use super::{field, Biquad, Effect};

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

/// Pass filters: (sections, Q of each at width 50 %, frequency factor) per order -3..=3 (spec: fit table).
fn pass_shape(order: i32) -> (usize, f64, f64) {
    match order {
        -3 => (4, 0.94, 1.0),
        -2 => (3, 0.895, 1.0),
        -1 => (2, 0.96, 1.0),
        1 => (2, 0.825, 1.0),
        2 => (3, 0.78, 1.0),
        3 => (4, 0.77, 1.0),
        _ => (1, 0.80, 1.11), // "2": one 12 dB/oct section
    }
}

/// The Steep 4 width law `ln Q = 1.046 - 1.495 w - 1.36 w²`, applied to every order relative to width 50 %.
fn width_factor(w: f64) -> f64 {
    let q4 = |w: f64| (1.046 - 1.495 * w - 1.36 * w * w).exp();
    q4(w) / q4(0.5)
}

/// One EQ band's settings.
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

    /// The band's biquad sections at `sr` (none when it is transparent).
    pub fn sections(&self, sr: f64) -> Sections {
        let mut out = Sections::default();
        let f0 = self.hz();
        let w = self.width.clamp(0.01, 1.0);
        let q = 0.36 / w;
        let db = self.db.clamp(-MAX_DB, MAX_DB);
        match self.kind {
            LOW_PASS | HIGH_PASS => {
                let (n, q50, fm) = pass_shape(self.order);
                let pq = (q50 * width_factor(w)).max(0.05);
                for _ in 0..n {
                    out.push(if self.kind == LOW_PASS { Biquad::low_pass(f0 * fm, pq, sr) } else { Biquad::high_pass(f0 / fm, pq, sr) });
                }
            }
            BAND_PASS => out.push(Biquad::band_pass(f0, q, sr)),
            BAND_STOP => out.push(Biquad::notch(f0, q, sr)),
            LOW_SHELF if db != 0.0 => out.push(Biquad::low_shelf(f0, 0.575 / w, db, sr)),
            PEAKING if db != 0.0 => out.push(Biquad::peaking(f0, q, db, sr)),
            HIGH_SHELF if db != 0.0 => out.push(Biquad::high_shelf(f0, 0.575 / w, db, sr)),
            _ => {} // disabled, or a shelf/peak at 0 dB (transparent)
        }
        out
    }

    /// The band's response at `hz` in dB.
    pub fn response_db(&self, hz: f64, sr: f64) -> f64 {
        self.sections(sr).response_db(hz, sr)
    }
}

/// Up to four biquad sections of one band.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sections {
    pub s: [Biquad; 4],
    pub n: usize,
}

impl Sections {
    fn push(&mut self, b: Biquad) {
        if self.n < 4 {
            self.s[self.n] = b;
            self.n += 1;
        }
    }

    pub fn as_slice(&self) -> &[Biquad] {
        &self.s[..self.n]
    }

    pub fn response_db(&self, hz: f64, sr: f64) -> f64 {
        self.as_slice().iter().map(|b| 20.0 * b.magnitude(hz, sr).max(1e-12).log10()).sum()
    }
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

impl Settings {
    /// From a saved state (fields 1..36 = parameters 0..35, byte 318 = enable mask).
    pub fn from_state(state: &[u8]) -> Option<Settings> {
        let param = |k: usize| field(state, k + 1);
        let mut bands = [Band::default_for(0); BANDS];
        for (b, band) in bands.iter_mut().enumerate() {
            *band = Band {
                db: f64::from(param(b)?) / 100.0,
                freq: f64::from(param(7 + b)?) / 65536.0,
                width: f64::from(param(14 + b)?) / 65536.0,
                kind: param(21 + b)?,
                order: param(28 + b)?,
            };
        }
        let main_db = f64::from(param(35)?) / 100.0;
        let mask = state.get(MASK_BYTE).copied().unwrap_or(0x7f);
        Some(Settings { bands, main_db, mask })
    }

    pub fn audible(&self, k: usize) -> bool {
        self.mask & (1 << k) != 0
    }

    /// The total response at `hz` in dB (audible bands and the main level).
    pub fn response_db(&self, hz: f64, sr: f64) -> f64 {
        self.main_db + (0..BANDS).filter(|&k| self.audible(k)).map(|k| self.bands[k].response_db(hz, sr)).sum::<f64>()
    }
}

pub struct ParametricEq {
    sections: Vec<Biquad>,
    main_gain: f32,
}

impl ParametricEq {
    pub fn from_state(state: &[u8], sample_rate: u32) -> Option<Self> {
        Some(Self::new(&Settings::from_state(state)?, sample_rate))
    }

    pub fn new(s: &Settings, sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate);
        let sections = (0..BANDS).filter(|&k| s.audible(k)).flat_map(|k| s.bands[k].sections(sr).as_slice().to_vec()).collect();
        ParametricEq { sections, main_gain: 10f64.powf(s.main_db / 20.0) as f32 }
    }
}

impl Effect for ParametricEq {
    fn reset(&mut self) {
        for s in &mut self.sections {
            s.reset();
        }
    }

    fn process(&mut self, buf: &mut [f32]) {
        for s in &mut self.sections {
            s.process(buf);
        }
        if self.main_gain != 1.0 {
            for x in buf.iter_mut() {
                *x *= self.main_gain;
            }
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
        let stored: Vec<i32> = (0..BANDS).map(|k| (default_freq(k) * 65536.0).round() as i32).collect();
        assert_eq!(stored, vec![10923, 18204, 25486, 32768, 40050, 47332, 54613]);
    }

    #[test]
    fn band_shapes_follow_the_fits() {
        let sr = 44100.0;
        // Peaking +12 dB peaks at +12 dB.
        let b = Band { db: 12.0, freq: freq_pos(1000.0), width: 0.5, kind: PEAKING, order: 0 };
        assert!((b.response_db(1000.0, sr) - 12.0).abs() < 0.05);
        // Steep 4 low pass at width 50 %: ~0 dB at the cutoff (two Q 0.96 sections), -24 dB/oct beyond.
        let lp = Band { db: 0.0, freq: freq_pos(1000.0), width: 0.5, kind: LOW_PASS, order: -1 };
        assert!(lp.response_db(1000.0, sr).abs() < 1.0, "{}", lp.response_db(1000.0, sr));
        assert!(lp.response_db(4000.0, sr) < -40.0);
        // A narrow width makes it resonant, a wide one dull.
        let narrow = Band { width: 0.2, ..lp };
        let wide = Band { width: 0.8, ..lp };
        assert!(narrow.response_db(1000.0, sr) > 9.0);
        assert!(wide.response_db(1000.0, sr) < -12.0);
        // Order "2" is one 12 dB/oct section.
        let two = Band { order: 0, ..lp };
        assert_eq!(two.sections(sr).n, 1);
        assert_eq!(Band { order: -2, ..lp }.sections(sr).n, 3);
        // A shelf at 0 dB is transparent.
        assert_eq!(Band::default_for(0).sections(sr).n, 0);
    }

    #[test]
    fn mask_leaves_bands_out() {
        let mut s = Settings::default();
        s.bands[3].db = 6.0;
        let f = s.bands[3].hz();
        assert!((s.response_db(f, 48000.0) - 6.0).abs() < 0.05);
        s.mask = 0x7f & !(1 << 3);
        assert!(s.response_db(f, 48000.0).abs() < 1e-9);
    }
}
