// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the display shows, worked out from the signal on the editor's side: the newest samples kept in a history, the
//! oscilloscope's trace, the spectrum's columns, the meter's two levels with their peak holds, the vectorscope's
//! points. Platform independent and free of drawing.
//!
// spec: plugins/lemon-drop.md "Display", plugins/wave-candy.md "Modes"

use std::collections::VecDeque;

use crate::state::{State, NEVER};

/// Samples of history kept (per channel).
pub const HISTORY: usize = 1 << 16;
/// Spectrum columns kept: wider than any display.
pub const MAX_COLUMNS: usize = 2400;
/// The spectrum's lowest and highest frequencies.
pub const F_LOW: f32 = 20.0;
pub const F_HIGH: f32 = 20000.0;
/// Natural weighting: +3 dB per octave from this frequency (pink noise draws level).
pub const NATURAL_REF: f32 = 100.0;
/// The meter's floor.
pub const FLOOR_DB: f32 = -120.0;

/// The spectrum's bands for a resolution setting (0 .. 8).
pub fn bands(res: i32) -> usize {
    32usize << res.clamp(0, 8)
}

/// The oscilloscope's time span in seconds: block length times block repeat, over 10 ms (both stored in hundredths of
/// a ms). 10 ms and 20 ms show 20 ms.
pub fn scope_span(st: &State) -> f32 {
    let len = st.scope_length.clamp(500, 5000) as f32 / 100.0;
    let rep = st.scope_repeat.clamp(1000, 10000) as f32 / 100.0;
    len * rep / 10.0 / 1000.0
}

/// Where frequency `f` sits on the spectrum's axis, 0 (20 Hz) .. 1 (20 kHz): linear with the curve at 0, logarithmic
/// (of f + 100 Hz) at 256, mixed between.
pub fn freq_pos(f: f32, curve: i32) -> f32 {
    let c = curve.clamp(0, 256) as f32 / 256.0;
    let lin = (f - F_LOW) / (F_HIGH - F_LOW);
    let log = ((f + 100.0) / (F_LOW + 100.0)).ln() / ((F_HIGH + 100.0) / (F_LOW + 100.0)).ln();
    (1.0 - c) * lin + c * log
}

/// The frequency at axis position `p` (inverse of `freq_pos`, by bisection).
pub fn pos_freq(p: f32, curve: i32) -> f32 {
    let (mut lo, mut hi) = (F_LOW, F_HIGH);
    for _ in 0..40 {
        let m = 0.5 * (lo + hi);
        if freq_pos(m, curve) < p {
            lo = m;
        } else {
            hi = m;
        }
    }
    0.5 * (lo + hi)
}

/// An in-place radix-2 FFT.
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        let n = n.next_power_of_two().max(2);
        let cos = (0..n / 2).map(|k| (-2.0 * std::f64::consts::PI * k as f64 / n as f64).cos() as f32).collect();
        let sin = (0..n / 2).map(|k| (-2.0 * std::f64::consts::PI * k as f64 / n as f64).sin() as f32).collect();
        Fft { n, cos, sin }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for s in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (wr, wi) = (self.cos[k * step], self.sin[k * step]);
                    let (a, b) = (s + k, s + k + len / 2);
                    let (xr, xi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                    re[b] = re[a] - xr;
                    im[b] = im[a] - xi;
                    re[a] += xr;
                    im[a] += xi;
                }
            }
            len <<= 1;
        }
    }
}

/// One spectrum column: the level of each band in dB (a full-scale sine reads 0 dB), natural weighting applied when
/// it is on, and the bin width in Hz.
#[derive(Clone, Debug)]
pub struct Column {
    pub db: Vec<f32>,
    pub hz_per_bin: f32,
}

/// The meter's ballistics state for one channel.
#[derive(Clone, Debug, Default)]
struct Ballistics {
    /// One-pole / sliding state.
    env: f64,
    /// Sliding sum of squares and its window (RMS, Leq, ITU).
    sum: f64,
    window: VecDeque<f64>,
    /// A- or K-weighting filter states (two biquads).
    z: [[f64; 2]; 2],
}

/// A peak indicator: its level and how long it has held.
#[derive(Clone, Copy, Debug)]
pub struct Hold {
    pub db: f32,
    pub age: f32,
}

pub struct Analyzer {
    pub rate: f32,
    pub l: VecDeque<f32>,
    pub r: VecDeque<f32>,
    /// Samples taken in since the last spectrum column.
    since_column: usize,
    pub columns: VecDeque<Column>,
    fft: Fft,
    re: Vec<f32>,
    im: Vec<f32>,
    /// The meter: shown level per channel (dB) and its peak indicators.
    pub meter: [f32; 2],
    pub hold: [Hold; 2],
    ball: [Ballistics; 2],
    meter_mode: i32,
    /// Seconds since the last new sample (for fading after silence).
    pub silent: f32,
}

impl Analyzer {
    pub fn new(rate: f32) -> Analyzer {
        Analyzer {
            rate: rate.max(1.0),
            l: VecDeque::from(vec![0.0; HISTORY]),
            r: VecDeque::from(vec![0.0; HISTORY]),
            since_column: 0,
            columns: VecDeque::new(),
            fft: Fft::new(2048),
            re: Vec::new(),
            im: Vec::new(),
            meter: [FLOOR_DB; 2],
            hold: [Hold { db: FLOOR_DB, age: 0.0 }; 2],
            ball: Default::default(),
            meter_mode: -1,
            silent: 1e9,
        }
    }

    pub fn set_rate(&mut self, rate: f32) {
        if (rate - self.rate).abs() > 0.5 {
            *self = Analyzer::new(rate);
        }
    }

    /// New samples (after the visual gain): they go into the history, the spectrum and the meter. `dt` is the time
    /// since the last call, which drives the meter's fall and holds.
    pub fn push(&mut self, st: &State, l: &[f32], r: &[f32], dt: f32) {
        let g = 10f32.powf(st.gain.clamp(-120, 120) as f32 / 200.0);
        if l.is_empty() {
            self.silent += dt;
        } else {
            self.silent = 0.0;
        }
        for (&a, &b) in l.iter().zip(r) {
            self.l.pop_front();
            self.r.pop_front();
            self.l.push_back(a * g);
            self.r.push_back(b * g);
        }
        self.spectrum(st, l.len());
        self.meter(st, l.len(), dt);
    }

    /// The newest `n` samples of a channel, oldest first.
    pub fn latest(&self, right: bool, n: usize) -> impl Iterator<Item = f32> + '_ {
        let h = if right { &self.r } else { &self.l };
        let n = n.min(h.len());
        h.range(h.len() - n..).copied()
    }

    // ---- Oscilloscope

    /// The oscilloscope's samples for each channel (`span` seconds), ending at the newest rising zero crossing of the
    /// mixed signal within the newest block length so a steady wave stands still; `None` for a channel when mixed.
    pub fn trace(&self, st: &State) -> (Vec<f32>, Vec<f32>) {
        let span = ((scope_span(st) * self.rate) as usize).clamp(8, HISTORY / 2);
        let look = ((st.scope_length.clamp(500, 5000) as f32 / 100_000.0) * self.rate) as usize;
        let n = self.l.len();
        let mono = |i: usize| self.l[i] + self.r[i];
        let mut end = n;
        for i in (n.saturating_sub(look.max(2))..n - 1).rev() {
            if mono(i) <= 0.0 && mono(i + 1) > 0.0 {
                end = i + 1;
                break;
            }
        }
        let start = end.saturating_sub(span);
        let l = self.l.range(start..end).copied().collect();
        let r = self.r.range(start..end).copied().collect();
        (l, r)
    }

    // ---- Spectrum

    fn spectrum(&mut self, st: &State, new: usize) {
        let hop = ((st.spectrum_length.clamp(500, 10000) as f32 / 100_000.0) * self.rate).max(1.0) as usize;
        self.since_column += new;
        let size = 2 * bands(st.spectrum_res);
        if self.fft.len() != size {
            self.fft = Fft::new(size);
            self.columns.clear();
        }
        // At most a display's worth of columns from one call (a long gap is not replayed).
        let mut due = self.since_column / hop;
        self.since_column %= hop;
        due = due.min(64);
        for k in (0..due).rev() {
            let back = k * hop;
            let col = self.column(st, back);
            self.columns.push_back(col);
            if self.columns.len() > MAX_COLUMNS {
                self.columns.pop_front();
            }
        }
    }

    /// The column whose newest sample is `back` samples before the newest.
    fn column(&mut self, st: &State, back: usize) -> Column {
        let n = self.fft.len();
        let len = self.l.len();
        let end = len.saturating_sub(back);
        let start = end.saturating_sub(n);
        self.re.clear();
        self.im.clear();
        self.re.resize(n, 0.0);
        self.im.resize(n, 0.0);
        for (k, i) in (start..end).enumerate() {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * k as f32 / n as f32).cos();
            self.re[k] = 0.5 * (self.l[i] + self.r[i]) * w;
        }
        self.fft.run(&mut self.re, &mut self.im);
        let hz = self.rate / n as f32;
        let norm = 4.0 / n as f32;
        let db = (0..n / 2)
            .map(|k| {
                let m = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt() * norm;
                let mut d = 20.0 * m.max(1e-9).log10();
                if st.natural {
                    d += 3.0 * ((k as f32 * hz).max(F_LOW) / NATURAL_REF).log2();
                }
                d
            })
            .collect();
        Column { db, hz_per_bin: hz }
    }

    // ---- Meter

    fn meter(&mut self, st: &State, new: usize, dt: f32) {
        if st.meter_mode != self.meter_mode {
            self.meter_mode = st.meter_mode;
            self.ball = Default::default();
        }
        let rate = self.rate as f64;
        let n = self.l.len();
        for c in 0..2 {
            let h = if c == 0 { &self.l } else { &self.r };
            let b = &mut self.ball[c];
            let mut peak = 0f64;
            let level = match st.meter_mode {
                // PPM: about 5 ms of integration, fast attack.
                1 => {
                    let a = (-1.0 / (0.005 * rate)).exp();
                    for i in n - new..n {
                        let x = f64::from(h[i]).abs();
                        b.env = if x > b.env { x } else { a * b.env + (1.0 - a) * x };
                        peak = peak.max(b.env);
                    }
                    peak
                }
                // Vu: a slow (300 ms) average of the rectified signal, read as a sine's peak.
                2 => {
                    let a = (-1.0 / (0.3 * rate)).exp();
                    for i in n - new..n {
                        b.env = a * b.env + (1.0 - a) * f64::from(h[i]).abs();
                    }
                    b.env * std::f64::consts::FRAC_PI_2
                }
                // RMS over 300 ms; Leq(A) over 1 s, A-weighted; ITU-R BS.1770 over 3 s, K-weighted.
                3 | 4 | 5 => {
                    let (secs, weight) = match st.meter_mode {
                        4 => (0.3, 0),
                        3 => (1.0, 1),
                        _ => (3.0, 2),
                    };
                    let w = (secs * rate) as usize;
                    for i in n - new..n {
                        let mut x = f64::from(h[i]);
                        if weight != 0 {
                            x = weigh(x, weight, rate, &mut b.z);
                        }
                        let e = x * x;
                        b.sum += e;
                        b.window.push_back(e);
                        while b.window.len() > w {
                            b.sum -= b.window.pop_front().unwrap_or(0.0);
                        }
                    }
                    let ms = (b.sum / b.window.len().max(1) as f64).max(0.0);
                    // Read as a sine's peak: +3 dB.
                    (2.0 * ms).sqrt()
                }
                // Peak.
                _ => {
                    for i in n - new..n {
                        peak = peak.max(f64::from(h[i]).abs());
                    }
                    peak
                }
            };
            let db = (20.0 * level.max(1e-9).log10()) as f32;
            let db = db.max(FLOOR_DB);
            // The shown level rises at once and falls at the decay speed.
            let fall = decay_db_per_s(st.meter_decay) * dt;
            self.meter[c] = if db >= self.meter[c] { db } else { (self.meter[c] - fall).max(db) };
            let hold = &mut self.hold[c];
            hold.age += dt;
            let held = st.meter_hold.clamp(0, NEVER);
            if self.meter[c] >= hold.db || held == 0 {
                *hold = Hold { db: self.meter[c], age: 0.0 };
            } else if held < NEVER && hold.age * 1000.0 > held as f32 {
                hold.db = (hold.db - fall).max(self.meter[c]);
            }
        }
    }

    // ---- Vectorscope

    /// The vectorscope's points: (side, mid) per sample of the newest block length, both in -1 .. 1 for a full-scale
    /// channel: side is (R - L) / 2 (right of centre is R), mid is (L + R) / 2 (up).
    pub fn lissajous(&self, st: &State) -> Vec<(f32, f32)> {
        let n = ((st.vector_length.clamp(500, 5000) as f32 / 100_000.0) * self.rate) as usize;
        let n = n.clamp(2, HISTORY);
        let len = self.l.len();
        (len - n..len).map(|i| ((self.r[i] - self.l[i]) * 0.5, (self.l[i] + self.r[i]) * 0.5)).collect()
    }

    /// Fade after silence: 1 (shown) down to 0, over half a second once the signal has been silent for the fade time.
    pub fn fade(&self, st: &State) -> f32 {
        if st.fade >= NEVER || st.background {
            return 1.0;
        }
        let t = self.silent - st.fade.max(0) as f32 / 1000.0;
        (1.0 - t / 0.5).clamp(0.0, 1.0)
    }
}

/// The meter's fall in dB per second for a decay speed setting (0 .. 5000, shown as 0 .. 100 %).
pub fn decay_db_per_s(v: i32) -> f32 {
    let p = v.clamp(0, 5000) as f32 / 5000.0;
    6.0 * 50f32.powf(p)
}

/// A- (1) or K-weighting (2): two biquads per sample, coefficients for `rate`.
fn weigh(x: f64, kind: u8, rate: f64, z: &mut [[f64; 2]; 2]) -> f64 {
    let stages = if kind == 1 {
        // A-weighting as a high-pass pair at 20.6 Hz and 107.7 / 737.9 Hz poles, low-pass at 12.2 kHz: approximated
        // by a 2nd-order high-pass at 200 Hz (Q 0.5) and a 2nd-order low-pass at 12.2 kHz.
        [biquad(rate, 200.0, 0.5, false), biquad(rate, 12200.0, 0.5, true)]
    } else {
        // K-weighting: the 1500 Hz +4 dB shelf as a gentle peak, then a 38 Hz high-pass.
        [shelf(rate, 1500.0, 4.0), biquad(rate, 38.0, 0.5, false)]
    };
    let mut y = x;
    for (s, c) in stages.iter().enumerate() {
        let (b0, b1, b2, a1, a2) = *c;
        let out = b0 * y + z[s][0];
        z[s][0] = b1 * y - a1 * out + z[s][1];
        z[s][1] = b2 * y - a2 * out;
        y = out;
    }
    y
}

/// RBJ low- or high-pass coefficients (b0, b1, b2, a1, a2), normalized.
fn biquad(rate: f64, f: f64, q: f64, low: bool) -> (f64, f64, f64, f64, f64) {
    let w = 2.0 * std::f64::consts::PI * (f / rate).min(0.49);
    let (s, c) = w.sin_cos();
    let alpha = s / (2.0 * q);
    let a0 = 1.0 + alpha;
    let (b0, b1, b2) = if low { ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0) } else { ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0) };
    (b0 / a0, b1 / a0, b2 / a0, -2.0 * c / a0, (1.0 - alpha) / a0)
}

/// RBJ high shelf.
fn shelf(rate: f64, f: f64, gain_db: f64) -> (f64, f64, f64, f64, f64) {
    let a = 10f64.powf(gain_db / 40.0);
    let w = 2.0 * std::f64::consts::PI * (f / rate).min(0.49);
    let (s, c) = w.sin_cos();
    let alpha = s / 2.0 * std::f64::consts::SQRT_2;
    let sa = 2.0 * a.sqrt() * alpha;
    let a0 = (a + 1.0) - (a - 1.0) * c + sa;
    let b0 = a * ((a + 1.0) + (a - 1.0) * c + sa);
    let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * c);
    let b2 = a * ((a + 1.0) + (a - 1.0) * c - sa);
    let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * c);
    let a2 = (a + 1.0) - (a - 1.0) * c - sa;
    (b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f32, amp: f32, n: usize, rate: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * f * i as f32 / rate).sin()).collect()
    }

    #[test]
    fn the_scope_spans_length_times_repeat_over_ten_ms() {
        let st = State::new_default();
        assert!((scope_span(&st) - 0.020).abs() < 1e-6);
        let mut s = st.clone();
        s.scope_length = 4000;
        assert!((scope_span(&s) - 0.080).abs() < 1e-6);
    }

    #[test]
    fn a_full_scale_sine_reads_zero_db_in_its_band() {
        let st = State { natural: false, ..State::new_default() };
        let mut a = Analyzer::new(44100.0);
        let x = sine(1000.0, 1.0, 8192, 44100.0);
        a.push(&st, &x, &x, 0.1);
        let c = a.columns.back().unwrap();
        let k = (1000.0 / c.hz_per_bin).round() as usize;
        let peak = c.db[k - 1..=k + 1].iter().cloned().fold(f32::MIN, f32::max);
        assert!(peak > -1.5 && peak < 0.5, "{peak}");
    }

    #[test]
    fn the_meter_reads_peaks_and_falls_at_its_speed() {
        let st = State::new_default();
        let mut a = Analyzer::new(44100.0);
        let x = sine(440.0, 0.5, 4410, 44100.0);
        a.push(&st, &x, &x, 0.1);
        assert!((a.meter[0] + 6.02).abs() < 0.1, "{}", a.meter[0]);
        let z = vec![0.0; 441];
        a.push(&st, &z, &z, 0.01);
        assert!((a.meter[0] - (-6.02 - decay_db_per_s(st.meter_decay) * 0.01)).abs() < 0.1);
        assert!((a.hold[0].db + 6.02).abs() < 0.1, "the peak holds");
    }

    #[test]
    fn rms_of_a_sine_reads_its_peak() {
        let st = State { meter_mode: 4, ..State::new_default() };
        let mut a = Analyzer::new(44100.0);
        let x = sine(441.0, 0.5, 44100, 44100.0);
        a.push(&st, &x, &x, 1.0);
        assert!((a.meter[0] + 6.02).abs() < 0.2, "{}", a.meter[0]);
    }

    #[test]
    fn the_frequency_axis_runs_linear_to_logarithmic() {
        assert!(freq_pos(20.0, 0).abs() < 1e-6 && (freq_pos(20000.0, 256) - 1.0).abs() < 1e-6);
        assert!((freq_pos(10010.0, 0) - 0.5).abs() < 1e-3);
        // Logarithmic: 1 k .. 2 k takes about as much as 10 k .. 20 k.
        let a = freq_pos(2000.0, 256) - freq_pos(1000.0, 256);
        let b = freq_pos(20000.0, 256) - freq_pos(10000.0, 256);
        assert!((a / b - 0.94).abs() < 0.03, "{}", a / b);
        assert!((pos_freq(freq_pos(440.0, 128), 128) - 440.0).abs() < 0.5);
    }

    #[test]
    fn a_mono_signal_draws_a_vertical_line() {
        let st = State::new_default();
        let mut a = Analyzer::new(44100.0);
        let x = sine(440.0, 0.5, 4410, 44100.0);
        a.push(&st, &x, &x, 0.1);
        assert!(a.lissajous(&st).iter().all(|&(s, _)| s.abs() < 1e-6));
    }
}
