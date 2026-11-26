// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The sound the editor's visualizer shows: the audio thread copies the last samples it played into a ring
//! (`Scope::push`, lock-free and allocation-free), and the editor turns the newest of them into band levels on its
//! timer (`Bands::next`): sixteen log-spaced bands from 40 Hz to 16 kHz (-60..0 dB) and the sound's pitch (where its
//! spectrum is centred, 150 Hz to 6 kHz on a log scale). How they show is `zest_ui::viz`.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Instant;

use zest_ui::viz::BANDS;

/// Samples kept (a power of two, at least `FFT`).
const LEN: usize = 4096;
/// The analysis length.
const FFT: usize = 1024;
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16000.0;
/// The level shown at the bottom of a column (dB re a full-scale sine).
const FLOOR_DB: f32 = -60.0;
/// Higher bands lift by this much each (music falls with frequency; bands are about half an octave wide).
const TILT_DB: f32 = 1.5;
/// The pitch's range (the spectrum's centre, Hz): 0 at the low end, 1 at the high one.
const PITCH_LOW: f32 = 150.0;
const PITCH_HIGH: f32 = 6000.0;

pub struct Scope {
    buf: Vec<AtomicU32>,
    pos: AtomicUsize,
    rate: AtomicU32,
}

impl Default for Scope {
    fn default() -> Self {
        Scope { buf: (0..LEN).map(|_| AtomicU32::new(0)).collect(), pos: AtomicUsize::new(0), rate: AtomicU32::new(48000f32.to_bits()) }
    }
}

impl Scope {
    pub fn set_rate(&self, rate: f32) {
        self.rate.store(rate.to_bits(), Ordering::Relaxed);
    }

    /// Audio thread: the samples just played (their mid).
    pub fn push(&self, left: &[f32], right: &[f32]) {
        let mut p = self.pos.load(Ordering::Relaxed);
        for (l, r) in left.iter().zip(right) {
            self.buf[p % LEN].store((0.5 * (l + r)).to_bits(), Ordering::Relaxed);
            p = p.wrapping_add(1);
        }
        self.pos.store(p, Ordering::Release);
    }
}

/// The editor's side: what computing the levels needs.
pub struct Bands {
    last: Option<Instant>,
    /// The ring position last analysed: no new sound, no new analysis.
    seen: usize,
    re: Vec<f32>,
    im: Vec<f32>,
    window: Vec<f32>,
}

impl Default for Bands {
    fn default() -> Self {
        let window = (0..FFT).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT as f32).cos()).collect();
        Bands { last: None, seen: 0, re: vec![0.0; FFT], im: vec![0.0; FFT], window }
    }
}

impl Bands {
    /// The bands' levels for the sound played since the last call (none: all 0), its pitch (0..1; `None` while
    /// silent), and the seconds since that call.
    pub fn next(&mut self, scope: &Scope) -> ([f32; BANDS], Option<f32>, f32) {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(0.1);
        self.last = Some(now);
        let pos = scope.pos.load(Ordering::Acquire);
        let (levels, pitch) = if pos == self.seen { ([0.0; BANDS], None) } else { self.analyse(scope, pos) };
        self.seen = pos;
        (levels, pitch, dt)
    }

    fn analyse(&mut self, scope: &Scope, pos: usize) -> ([f32; BANDS], Option<f32>) {
        for i in 0..FFT {
            let s = f32::from_bits(scope.buf[pos.wrapping_sub(FFT - i) % LEN].load(Ordering::Relaxed));
            self.re[i] = s * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        let rate = f32::from_bits(scope.rate.load(Ordering::Relaxed)).max(8000.0);
        let hz_per_bin = rate / FFT as f32;
        // a full-scale sine's peak bin through the Hann window: N / 4
        let norm = (FFT as f32 / 4.0).powi(2);
        let mut out = [0.0; BANDS];
        for (b, o) in out.iter_mut().enumerate() {
            let edge = |k: usize| LOW_HZ * (HIGH_HZ / LOW_HZ).powf(k as f32 / BANDS as f32);
            let lo = ((edge(b) / hz_per_bin).floor() as usize).max(1);
            let hi = ((edge(b + 1) / hz_per_bin).ceil() as usize).clamp(lo + 1, FFT / 2);
            let power: f32 = (lo..hi).map(|k| self.re[k] * self.re[k] + self.im[k] * self.im[k]).sum::<f32>() / norm;
            let db = 10.0 * power.max(1e-12).log10() + TILT_DB * b as f32;
            *o = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        }
        // the pitch: the power-weighted mean frequency over the bands' range, on a log scale
        let (lo, hi) = (((LOW_HZ / hz_per_bin) as usize).max(1), ((HIGH_HZ / hz_per_bin) as usize).min(FFT / 2));
        let (mut sum, mut weighted) = (0.0f32, 0.0f32);
        for k in lo..hi {
            let p = self.re[k] * self.re[k] + self.im[k] * self.im[k];
            sum += p;
            weighted += p * k as f32 * hz_per_bin;
        }
        let audible = 10.0 * (sum / norm).max(1e-12).log10() > FLOOR_DB;
        let pitch = audible.then(|| ((weighted / sum).max(1.0) / PITCH_LOW).log2() / (PITCH_HIGH / PITCH_LOW).log2());
        (out, pitch.map(|p| p.clamp(0.0, 1.0)))
    }
}

/// In-place radix-2 FFT (`re.len()` a power of two).
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
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
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full-scale sine lights its own band to the top and leaves the far ones dark; no new sound, no levels.
    #[test]
    fn a_sine_lights_its_band() {
        let scope = Scope::default();
        scope.set_rate(48000.0);
        let sine: Vec<f32> = (0..2048).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / 48000.0).sin()).collect();
        scope.push(&sine, &sine);
        let mut b = Bands::default();
        let (levels, pitch, _) = b.next(&scope);
        // 1 kHz: band edges 40 * 400^(k/16); 1000 Hz lies in band 8 (800..1163 Hz)
        let top = (0..BANDS).max_by(|&x, &y| levels[x].total_cmp(&levels[y])).unwrap();
        assert_eq!(top, 8, "{levels:?}");
        assert!(levels[8] > 0.95 && levels[0] < 0.2, "{levels:?}");
        // 1 kHz on the pitch scale (150 Hz..6 kHz, log): about 0.51
        assert!(pitch.is_some_and(|p| (p - 0.514).abs() < 0.03), "{pitch:?}");
        let (silent, none, _) = b.next(&scope);
        assert_eq!((silent, none), ([0.0; BANDS], None));
    }
}
