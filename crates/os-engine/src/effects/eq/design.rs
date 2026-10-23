// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A band's sections from its stored values: the band's settings, the ordinary coefficients (the bilinear designs of
//! plain processing, HQ Legacy and HQ Enhanced) and the three arguments HQ+'s matched design takes, with the tables
//! of the steep variants. Precision as the page gives it: singles where it says so, doubles otherwise, evaluated as
//! bracketed.
//!
// spec: plugins/fruity-parametric-eq-2.md ("A band's settings", "Coefficients", "Tables of the steep variants",
// "HQ+": "The three arguments")

use std::f64::consts::{LN_2, TAU};

use super::matched;
use super::{BAND_PASS, BAND_STOP, HIGH_PASS, HIGH_SHELF, LOW_PASS, LOW_SHELF, PEAKING};

/// `ln 1000`.
const LN_1000: f64 = 6.907755278982137;
/// `ln 2 / 2`.
const HALF_LN_2: f64 = LN_2 / 2.0;

/// `single(ln(10) * 0.05)`: the natural exponent of one decibel of amplitude.
pub fn db_unit() -> f32 {
    (std::f64::consts::LN_10 * 0.05) as f32
}

/// A band's five parameters as the state stores them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// Hundredths of a dB, -1800..1800.
    pub level: i32,
    /// 0..65536: 20 Hz * 1000^(freq / 65536).
    pub freq: i32,
    /// 0..65536.
    pub width: i32,
    /// 0 off .. 7 high shelf.
    pub kind: i32,
    /// -3..3: `|order| + 1` sections, negative = steep.
    pub order: i32,
}

impl Stored {
    /// Each value kept within its range (as the state holds them).
    pub fn clamped(self) -> Stored {
        Stored {
            level: self.level.clamp(-1800, 1800),
            freq: self.freq.clamp(0, 65536),
            width: self.width.clamp(0, 65536),
            kind: self.kind.clamp(0, 7),
            order: self.order.clamp(-3, 3),
        }
    }

    /// Sections: `|order| + 1`, none for a band of type 0.
    pub fn sections(&self) -> usize {
        if self.kind == 0 { 0 } else { self.order.unsigned_abs() as usize + 1 }
    }

    /// The shelves and the peaking type, whose level counts.
    pub fn has_gain(&self) -> bool {
        matches!(self.kind, LOW_SHELF | PEAKING | HIGH_SHELF)
    }
}

/// How the sections are designed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Design {
    /// The ordinary (bilinear) coefficients for the rate the bands run at.
    Ordinary(f64),
    /// HQ+: the matched design, for the project's rate.
    Matched(f64),
}

impl Design {
    /// The rate the bands run at.
    pub fn rate(self) -> f64 {
        match self {
            Design::Ordinary(r) | Design::Matched(r) => r,
        }
    }
}

/// One section's coefficients (`a0 = 1`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefs {
    /// A section that passes its input on.
    pub const UNITY: Coefs = Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// The squared magnitude at `w` radians per sample.
    pub fn power(&self, w: f64) -> f64 {
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let (nr, ni) = (self.b0 * c2 + self.b1 * c1 + self.b2, self.b0 * s2 + self.b1 * s1);
        let (dr, di) = (c2 + self.a1 * c1 + self.a2, s2 + self.a1 * s1);
        (nr * nr + ni * ni) / (dr * dr + di * di).max(1e-300)
    }
}

/// A band's sections, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BandCoefs {
    pub s: [Coefs; 4],
    pub n: usize,
}

impl BandCoefs {
    pub fn as_slice(&self) -> &[Coefs] {
        &self.s[..self.n]
    }
}

/// Rows `m = 1..3` of the steep variants (row 0 unused). All singles.
const PASS_Q: [[f32; 4]; 4] =
    [[0.0; 4], [1.9834017, 0.74921465, 0.0, 0.0], [2.64304, 1.0815187, 0.69424474, 0.0], [3.5764515, 1.1417328, 1.0447108, 0.6551186]];
const SHELF_SLOPE: [[f32; 4]; 4] =
    [[0.0; 4], [2.6304471, 0.61564445, 0.0, 0.0], [5.161228, 1.109271, 0.5098673, 0.0], [10.319123, 1.5144353, 0.69403285, 0.5529967]];
const PEAK_SHIFT: [[f32; 4]; 4] = [[0.0; 4], [-0.25, 0.25, 0.0, 0.0], [-0.475, 0.0, 0.475, 0.0], [-0.775, -0.3, 0.3, 0.775]];
const PEAK_WIDTH: [[f32; 4]; 4] = [[0.0; 4], [1.0, 1.0, 0.0, 0.0], [1.0, 1.5707964, 1.0, 0.0], [1.0, 1.5707964, 1.5707964, 1.0]];
const PEAK_GAIN: [f32; 4] = [0.0, 0.32125, 0.2442, 0.214445];
const PEAK_NARROW: [f32; 4] = [0.0, 0.5, 0.33333334, 0.25];

/// The band's frequency in Hz: `single(exp(double(single(frequency)) * (ln 1000 / (ln 2 * 65536)) * ln 2) * 20)`.
pub fn freq_hz(freq: i32) -> f32 {
    ((f64::from(freq as f32) * (LN_1000 / (LN_2 * 65536.0)) * LN_2).exp() * 20.0) as f32
}

/// `sinh` as the band's settings use it: 0 for arguments within 1e-12 of 0.
fn sinh(z: f64) -> f64 {
    if z.abs() <= 1e-12 { 0.0 } else { (z.exp() - (-z).exp()) / 2.0 }
}

/// The settings shared by the band's sections ("A band's settings").
struct Shared {
    freq: f32,
    width: f32,
    gain: f32,
    m: usize,
    share: f32,
    steep: bool,
}

impl Shared {
    fn of(b: &Stored) -> Shared {
        let m = b.order.unsigned_abs() as usize;
        Shared {
            freq: freq_hz(b.freq),
            width: (b.width as f32) * (1.0 / 65536.0),
            gain: if b.has_gain() { (b.level as f32) * 0.01 } else { 0.0 },
            m,
            share: (1.0 / (m as f64 + 1.0)) as f32,
            steep: b.order < 0,
        }
    }

    /// `x = double(single(1 - width))`.
    fn inverse_width(&self) -> f64 {
        f64::from((1.0 - f64::from(self.width)) as f32)
    }

    /// The pass filters' `q`, after its `m > 0` and steep lines.
    fn pass_q(&self, k: usize) -> f64 {
        let x = self.inverse_width();
        let mut q = x * x * x * 9.9 + 0.1;
        if self.m > 0 {
            q = f64::from((f64::from(self.share) * f64::from(q as f32).ln()).exp() as f32);
        }
        if self.steep {
            q *= f64::from(PASS_Q[self.m][k]);
        }
        q
    }

    /// Band pass and notch: the octave width scaled by the number of sections.
    fn spread_width(&self, kind: i32) -> f64 {
        let db = db_unit();
        let ratio = ((f64::from(db * 3.0)).exp() as f32) / ((f64::from(db * 6.0)).exp() as f32);
        let spread = (f64::from(ratio) * (self.m as f64 + 1.0).sqrt()) as f32;
        let factor = if kind == BAND_PASS { spread } else { 1.0 / spread };
        (f64::from(self.width) * 4.9 + 0.1) * f64::from(factor)
    }

    /// The shelves' `A` (`single(exp(double(gain * share * 0.5 * dB)))`, the product in singles) and `s`.
    fn shelf(&self, k: usize) -> (f64, f64) {
        let a = f64::from((f64::from(self.gain * self.share * 0.5 * db_unit())).exp() as f32);
        let mut s = self.inverse_width() * 1.7192982456140353 + 0.3333333333333333;
        if self.steep {
            s *= f64::from(SHELF_SLOPE[self.m][k]);
        }
        (a, s)
    }

    /// A steep peaking section's narrowed width, shifted frequency (not rounded) and gain factor.
    fn steep_peak(&self, k: usize) -> (f64, f64, f64) {
        let m = self.m;
        let bwn = (f64::from(self.width) * 4.9 + 0.1) * f64::from(PEAK_NARROW[m]);
        let shifted = f64::from(self.freq) * ((f64::from(PEAK_SHIFT[m][k]) * bwn) * LN_2).exp();
        let bw = bwn * f64::from(PEAK_WIDTH[m][k]);
        let g = if m == 1 { f64::from(PEAK_GAIN[1]) - (bw - 1.0) * 0.01 } else { f64::from(PEAK_GAIN[m]) };
        (bw, shifted, g)
    }
}

/// The band's sections for `design` (none for a band of type 0).
pub fn band(b: &Stored, design: Design) -> BandCoefs {
    let b = b.clamped();
    let mut out = BandCoefs { n: b.sections(), ..BandCoefs::default() };
    if out.n == 0 {
        return out;
    }
    let sh = Shared::of(&b);
    for k in 0..out.n {
        out.s[k] = match design {
            Design::Ordinary(rate) => ordinary(&sh, b.kind, k, rate),
            Design::Matched(rate) => {
                let (w, q, g) = matched_args(&sh, b.kind, k, rate);
                matched::section(b.kind, f64::from(w), f64::from(q), f64::from(g))
            }
        };
    }
    out
}

/// Section `k` of the ordinary form ("A band's settings", "Coefficients").
fn ordinary(sh: &Shared, kind: i32, k: usize, rate: f64) -> Coefs {
    let angle = (1.0 / rate) * TAU;
    let lim = if kind == PEAKING { 2.1991147994995117 } else { 3.138451099395752 };
    let mut w = (f64::from(sh.freq) * angle).min(lim);
    let (mut sn, mut cs) = (w.sin(), w.cos());
    let oct = |bw: f64, w: f64, sn: f64| sn * sinh(HALF_LN_2 * bw * w / sn);
    match kind {
        LOW_PASS | HIGH_PASS => {
            let alpha = sn * (1.0 / (0.5f64.sqrt() * 2.0)) / sh.pass_q(k);
            let n = 1.0 / (alpha + 1.0);
            let (a1, a2) = (n * cs * -2.0, n * (1.0 - alpha));
            if kind == LOW_PASS {
                let b1 = n * (1.0 - cs);
                Coefs { b0: b1 * 0.5, b1, b2: b1 * 0.5, a1, a2 }
            } else {
                let b1 = n * -(cs + 1.0);
                Coefs { b0: -b1 * 0.5, b1, b2: -b1 * 0.5, a1, a2 }
            }
        }
        BAND_PASS | BAND_STOP => {
            let alpha = oct(sh.spread_width(kind), w, sn);
            let n = 1.0 / (alpha + 1.0);
            let (a1, a2) = (n * cs * -2.0, n * (1.0 - alpha));
            if kind == BAND_PASS { Coefs { b0: n * alpha, b1: 0.0, b2: n * -alpha, a1, a2 } } else { Coefs { b0: n, b1: a1, b2: n, a1, a2 } }
        }
        PEAKING => {
            let mut bw = f64::from(sh.width) * 4.9 + 0.1;
            let mut g = f64::from(sh.gain);
            if sh.m > 0 && sh.steep {
                let (bws, shifted, factor) = sh.steep_peak(k);
                bw = bws;
                g *= factor;
                w = (f64::from(shifted as f32) * angle).min(2.5132741928100586);
                (sn, cs) = (w.sin(), w.cos());
            } else {
                g = g * f64::from(sh.share) * 0.5;
            }
            let a = f64::from((f64::from((g as f32) * db_unit())).exp() as f32);
            let alpha = oct(bw, w, sn);
            let (down, up) = (alpha / a, alpha * a);
            let n = 1.0 / (down + 1.0);
            let a1 = n * cs * -2.0;
            Coefs { b0: n * (up + 1.0), b1: a1, b2: n * (1.0 - up), a1, a2: n * (1.0 - down) }
        }
        LOW_SHELF | HIGH_SHELF => {
            let (a, s) = sh.shelf(k);
            let big_s = a.sqrt() * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
            let d = (a - 1.0) * cs;
            let beta = big_s * sn;
            if kind == LOW_SHELF {
                let n = 1.0 / (a + 1.0 + d + beta);
                Coefs {
                    a1: n * ((a - 1.0) + (a + 1.0) * cs) * -2.0,
                    a2: n * (a + 1.0 + d - beta),
                    b0: n * (a * (a + 1.0 - d + beta)),
                    b1: n * (a * 2.0 * ((a - 1.0) - (a + 1.0) * cs)),
                    b2: n * (a * (a + 1.0 - d - beta)),
                }
            } else {
                let n = 1.0 / (a + 1.0 - d + beta);
                Coefs {
                    a1: n * ((a - 1.0) - (a + 1.0) * cs) * 2.0,
                    a2: n * (a + 1.0 - d - beta),
                    b0: n * (a * (a + 1.0 + d + beta)),
                    b1: n * (a * -2.0 * ((a - 1.0) + (a + 1.0) * cs)),
                    b2: n * (a * (a + 1.0 + d - beta)),
                }
            }
        }
        _ => Coefs::UNITY,
    }
}

/// HQ+: the three singles the matched design takes for section `k` ("The three arguments"): the frequency as a
/// fraction of the Nyquist frequency, the quality and the gain in dB.
fn matched_args(sh: &Shared, kind: i32, k: usize, rate: f64) -> (f32, f32, f32) {
    let band_q = |bw: f64| 1.0 / (sinh(HALF_LN_2 * bw) * 2.0);
    let (f, q, g) = match kind {
        LOW_PASS | HIGH_PASS => (f64::from(sh.freq), sh.pass_q(k) * 0.5f64.sqrt(), 0.0),
        BAND_PASS | BAND_STOP => (f64::from(sh.freq), band_q(sh.spread_width(kind)), 0.0),
        PEAKING if sh.m > 0 && sh.steep => {
            let (bw, shifted, factor) = sh.steep_peak(k);
            (shifted, band_q(bw), (f64::from(sh.gain) * factor) * 2.0)
        }
        PEAKING => (f64::from(sh.freq), band_q(f64::from(sh.width) * 4.9 + 0.1), (f64::from(sh.gain) * f64::from(sh.share) * 0.5) * 2.0),
        LOW_SHELF | HIGH_SHELF => {
            let (a, s) = sh.shelf(k);
            (f64::from(sh.freq), 1.0 / ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt(), f64::from(sh.gain * sh.share))
        }
        _ => (f64::from(sh.freq), 1.0, 0.0),
    };
    (((f / rate) * 2.0) as f32, q as f32, g as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(c: &BandCoefs, hz: f64, rate: f64) -> f64 {
        c.as_slice().iter().map(|s| 10.0 * s.power(TAU * hz / rate).log10()).sum()
    }

    fn stored(level: i32, hz: f64, width: i32, kind: i32, order: i32) -> Stored {
        let freq = ((hz / 20.0).ln() / 1000f64.ln() * 65536.0).round() as i32;
        Stored { level, freq, width, kind, order }
    }

    // spec: plugins/fruity-parametric-eq-2.md "A band's settings", "Settings" (the factory preset "Default")
    #[test]
    fn frequency_law_and_settings() {
        // 20 Hz * 1000^(f / 65536), rounded to single.
        assert_eq!(freq_hz(0), 20.0);
        assert!((f64::from(freq_hz(65536)) - 20000.0).abs() < 0.01);
        assert_eq!(freq_hz(32768), 632.4555_f32);
        // `share` and the width are singles.
        let sh = Shared::of(&Stored { level: 600, freq: 32768, width: 25412, kind: PEAKING, order: 2 });
        assert_eq!((sh.m, sh.share, sh.steep), (2, (1.0f64 / 3.0) as f32, false));
        assert_eq!(sh.width, 25412.0 / 65536.0);
        assert_eq!(sh.gain, 6.0);
        assert_eq!(db_unit(), 0.115129254758358);
        // Levels count only for shelves and peaking.
        assert_eq!(Shared::of(&Stored { level: 600, kind: LOW_PASS, ..Stored::default() }).gain, 0.0);
        assert_eq!(Stored { kind: HIGH_PASS, order: -3, ..Stored::default() }.sections(), 4);
        assert_eq!(Stored { kind: 0, order: 3, ..Stored::default() }.sections(), 0);
    }

    // spec: plugins/fruity-parametric-eq-2.md "Coefficients"
    #[test]
    fn ordinary_sections_follow_the_formulas() {
        let rate = 44100.0;
        // Peaking +12 dB, order 0: the cookbook peak, +12 dB at the centre.
        let pk = stored(1200, 1000.0, 25412, PEAKING, 0);
        let c = band(&pk, Design::Ordinary(rate));
        let f = f64::from(freq_hz(pk.freq));
        assert!((db(&c, f, rate) - 12.0).abs() < 1e-3, "{}", db(&c, f, rate));
        // The same gain over three sections (Gentle 6) still peaks at +12 dB.
        let g6 = band(&Stored { order: 2, ..pk }, Design::Ordinary(rate));
        assert_eq!(g6.n, 3);
        assert!((db(&g6, f, rate) - 12.0).abs() < 1e-3);
        // A low pass passes DC, a steep high pass blocks it.
        let lp = band(&stored(0, 1000.0, 32768, LOW_PASS, 0), Design::Ordinary(rate));
        assert!(db(&lp, 10.0, rate).abs() < 1e-3 && db(&lp, 10000.0, rate) < -30.0);
        let hp = band(&stored(0, 1000.0, 32768, HIGH_PASS, -3), Design::Ordinary(rate));
        assert!(db(&hp, 20000.0, rate).abs() < 0.5 && db(&hp, 100.0, rate) < -100.0);
        // Hand-worked low pass at width 1 (x = 0, q = 0.1), one section: alpha = sn / (sqrt 2 * 0.1).
        let one = Stored { level: 0, freq: 32768, width: 65536, kind: LOW_PASS, order: 0 };
        let w = f64::from(freq_hz(32768)) * ((1.0 / rate) * TAU);
        let alpha = w.sin() * (1.0 / (0.5f64.sqrt() * 2.0)) / 0.1;
        let n = 1.0 / (alpha + 1.0);
        let c = band(&one, Design::Ordinary(rate)).s[0];
        assert_eq!(c.a1, n * w.cos() * -2.0);
        assert_eq!(c.a2, n * (1.0 - alpha));
        assert_eq!(c.b1, n * (1.0 - w.cos()));
        assert_eq!(c.b0, c.b1 * 0.5);
        // Band pass: unity at the centre; notch: a deep cut there.
        let bp = band(&stored(0, 1000.0, 25412, BAND_PASS, 0), Design::Ordinary(rate));
        let f = f64::from(freq_hz(stored(0, 1000.0, 0, 0, 0).freq));
        assert!(db(&bp, f, rate).abs() < 1e-6);
        let notch = band(&stored(0, 1000.0, 25412, BAND_STOP, 0), Design::Ordinary(rate));
        assert!(db(&notch, f, rate) < -100.0);
        // Shelves: the full gain far from the corner, none on the other side.
        let ls = band(&stored(-900, 200.0, 40124, LOW_SHELF, 0), Design::Ordinary(rate));
        assert!((db(&ls, 10.0, rate) + 9.0).abs() < 0.05 && db(&ls, 15000.0, rate).abs() < 0.05);
        let hs = band(&stored(600, 5000.0, 40124, HIGH_SHELF, -2), Design::Ordinary(rate));
        assert!((db(&hs, 21000.0, rate) - 6.0).abs() < 0.3 && db(&hs, 50.0, rate).abs() < 0.05);
        // A shelf or peak at 0 dB is (very nearly) flat.
        let flat = band(&stored(0, 1000.0, 25412, PEAKING, 0), Design::Ordinary(rate));
        assert!((1..200).all(|i| db(&flat, f64::from(i) * 100.0, rate).abs() < 1e-9));
    }

    // spec: plugins/fruity-parametric-eq-2.md "Coefficients" (the limit L) and "Tables of the steep variants"
    #[test]
    fn steep_variants_and_limits() {
        let rate = 44100.0;
        // A steep peaking band: three sections, the outer two shifted down and up.
        let pk = stored(1200, 1000.0, 32768, PEAKING, -2);
        let c = band(&pk, Design::Ordinary(rate));
        assert_eq!(c.n, 3);
        let centre = |c: &Coefs| (-c.a1 / (2.0 * c.a2.sqrt())).acos() * rate / TAU;
        assert!(centre(&c.s[0]) < 900.0 && centre(&c.s[2]) > 1100.0, "{} {}", centre(&c.s[0]), centre(&c.s[2]));
        // Frequencies at the top are held at L (20 kHz at 32 kHz is above the Nyquist frequency).
        let top = band(&Stored { level: 0, freq: 65536, width: 32768, kind: LOW_PASS, order: 0 }, Design::Ordinary(32000.0));
        let w: f64 = 3.138451099395752;
        let alpha = w.sin() * (1.0 / (0.5f64.sqrt() * 2.0)) / (f64::from((1.0 - 0.5f64) as f32).powi(3) * 9.9 + 0.1);
        assert_eq!(top.s[0].a2, (1.0 / (alpha + 1.0)) * (1.0 - alpha));
    }
}
