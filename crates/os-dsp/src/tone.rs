// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Tone: what a stretch of a sound is made of, for colouring its waveform (General ▸ Display ▸ "Colorful
//! waveforms"). A sound is cut into blocks of [`BLOCK`] frames; each block's Hann-windowed power spectrum is summed
//! into its total power, its first moment (for the spectral centroid) and three bands (below 250 Hz, 250 Hz to
//! 4 kHz, above 4 kHz). The sums add up, so the blocks a drawn column covers merge into that column's tone, at any
//! zoom, without another FFT.
//!
// spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"

use std::f32::consts::PI;

/// Frames per analysis block, and the FFT size (11.6 ms at 44.1 kHz; 86 Hz between bins).
pub const BLOCK: usize = 512;

/// Band edges in Hz: low below the first, high above the second.
pub const BAND_EDGES: [f32; 2] = [250.0, 4000.0];

/// Power below which a stretch counts as silence (about −100 dBFS): it has no tone.
const SILENCE: f32 = 1e-10;

/// The tone of a stretch: sums over its power spectrum. Powers are scaled so that a full-scale sine sums to about 0.75
/// per block (the Hann window's 1.5 bins of 0.5).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tone {
    /// Total power.
    pub power: f32,
    /// Sum of frequency (Hz) times power.
    pub moment: f32,
    /// Power below 250 Hz, from 250 Hz to 4 kHz, and above 4 kHz.
    pub bands: [f32; 3],
}

impl Tone {
    /// The spectral centroid in Hz; `None` for silence.
    pub fn centroid(&self) -> Option<f32> {
        (self.power > SILENCE).then(|| self.moment / self.power)
    }

    /// The band powers; `None` for silence.
    pub fn band_powers(&self) -> Option<[f32; 3]> {
        (self.power > SILENCE).then_some(self.bands)
    }

    /// Adds `w` times `o`.
    fn add(&mut self, o: &Tone, w: f32) {
        self.power += o.power * w;
        self.moment += o.moment * w;
        for (a, b) in self.bands.iter_mut().zip(o.bands) {
            *a += b * w;
        }
    }
}

/// The tones of a whole sound, block by block.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tones {
    /// Sample rate of the sound (Hz).
    pub rate: u32,
    /// One tone per [`BLOCK`] frames; the last block may be short (padded with silence).
    pub blocks: Vec<Tone>,
}

impl Tones {
    /// Analyses a mono sound.
    pub fn analyse(mono: &[f32], rate: u32) -> Tones {
        Tones::analyse_with(mono.len(), rate, |i| mono[i])
    }

    /// Analyses the mid (`(left + right) / 2`) of a stereo sound; `right` as long as `left`.
    pub fn analyse_stereo(left: &[f32], right: &[f32], rate: u32) -> Tones {
        let n = left.len().min(right.len());
        Tones::analyse_with(n, rate, |i| 0.5 * (left[i] + right[i]))
    }

    fn analyse_with(frames: usize, rate: u32, at: impl Fn(usize) -> f32) -> Tones {
        let fft = Fft::new(BLOCK);
        let window: Vec<f32> = (0..BLOCK).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / BLOCK as f32).cos()).collect();
        let (mut re, mut im) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
        let hz = rate.max(1) as f32 / BLOCK as f32;
        // |X|² over (N/2)²: a full-scale sine's peak bin is about 0.25.
        let norm = 1.0 / ((BLOCK / 2) * (BLOCK / 2)) as f32;
        let mut blocks = Vec::with_capacity(frames.div_ceil(BLOCK));
        for start in (0..frames).step_by(BLOCK) {
            for i in 0..BLOCK {
                let j = start + i;
                re[i] = if j < frames { at(j) * window[i] } else { 0.0 };
                im[i] = 0.0;
            }
            fft.run(&mut re, &mut im);
            let mut t = Tone::default();
            for k in 0..=BLOCK / 2 {
                // Bins 1 .. N/2 - 1 stand for both their positive and negative frequency.
                let both = if k == 0 || k == BLOCK / 2 { 1.0 } else { 2.0 };
                let p = (re[k] * re[k] + im[k] * im[k]) * norm * both;
                let f = k as f32 * hz;
                t.power += p;
                t.moment += p * f;
                t.bands[BAND_EDGES.iter().filter(|&&e| f >= e).count()] += p;
            }
            blocks.push(t);
        }
        Tones { rate, blocks }
    }

    /// The tone of frames `a .. b` (fractions allowed): the blocks it overlaps, each weighted by how much of it lies
    /// inside. A stretch of more than 32 blocks samples them evenly (a column of a far zoomed-out view). Outside the
    /// sound there is silence.
    pub fn span(&self, a: f64, b: f64) -> Tone {
        let mut t = Tone::default();
        let n = self.blocks.len();
        let blk = BLOCK as f64;
        let (a, b) = (a.max(0.0), b.min((n * BLOCK) as f64));
        if n == 0 || b <= a {
            return t;
        }
        let first = (a / blk).floor() as usize;
        let last = ((b / blk).ceil() as usize).clamp(first + 1, n);
        let stride = ((last - first) / 32).max(1);
        for i in (first..last).step_by(stride) {
            let lo = (i as f64 * blk).max(a);
            let hi = ((i + 1) as f64 * blk).min(b);
            let w = if stride > 1 { 1.0 } else { ((hi - lo) / blk).max(0.0) as f32 };
            t.add(&self.blocks[i], w);
        }
        t
    }
}

/// An in-place radix-2 complex FFT of a fixed power-of-two size, its twiddles computed once.
struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    fn new(n: usize) -> Fft {
        debug_assert!(n.is_power_of_two());
        let a = |k: usize| -2.0 * std::f64::consts::PI * k as f64 / n as f64;
        Fft { n, cos: (0..n / 2).map(|k| a(k).cos() as f32).collect(), sin: (0..n / 2).map(|k| a(k).sin() as f32).collect() }
    }

    fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        let bits = n.trailing_zeros();
        for i in 0..n {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for s in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (c, si) = (self.cos[k * step], self.sin[k * step]);
                    let (u, v) = (s + k, s + k + len / 2);
                    let tr = re[v] * c - im[v] * si;
                    let ti = re[v] * si + im[v] * c;
                    re[v] = re[u] - tr;
                    im[v] = im[u] - ti;
                    re[u] += tr;
                    im[u] += ti;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).map(|i| (2.0 * PI * hz * i as f32 / rate as f32).sin() * 0.5).collect()
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// The FFT puts a bin-centred sine in its bin, with the energy Parseval's theorem gives.
    #[test]
    fn fft_finds_a_bin_centred_sine() {
        let n = 64;
        let fft = Fft::new(n);
        let mut re: Vec<f32> = (0..n).map(|i| (2.0 * PI * 5.0 * i as f32 / n as f32).cos()).collect();
        let mut im = vec![0.0; n];
        fft.run(&mut re, &mut im);
        let mag = |k: usize| (re[k] * re[k] + im[k] * im[k]).sqrt();
        assert!((mag(5) - 32.0).abs() < 1e-3 && (mag(59) - 32.0).abs() < 1e-3, "{} {}", mag(5), mag(59));
        assert!((0..n).filter(|&k| k != 5 && k != 59).all(|k| mag(k) < 1e-3));
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// Sines' centroids land at their frequencies (within the low end's coarse bins) and their power in their band;
    /// silence has no tone; the blocks a stretch overlaps merge.
    #[test]
    fn centroids_and_bands_of_sines() {
        let rate = 44100;
        for (hz, band, tol) in [(100.0, 0, 40.0), (1000.0, 1, 30.0), (8000.0, 2, 60.0)] {
            let t = Tones::analyse(&sine(hz, rate, 0.5), rate);
            let mid = t.span(4096.0, 16384.0);
            let c = mid.centroid().unwrap();
            assert!((c - hz).abs() < tol, "{hz} Hz: centroid {c}");
            let b = mid.band_powers().unwrap();
            assert!(b[band] > 0.95 * mid.power, "{hz} Hz: bands {b:?}");
            // Amplitude 0.5: a quarter of a full-scale sine's power, per block.
            let per_block = mid.power / 24.0;
            assert!((per_block - 0.75 / 4.0).abs() < 0.01, "{hz} Hz: power per block {per_block}");
        }
        let quiet = Tones::analyse(&[0.0; 4000], rate);
        assert_eq!(quiet.blocks.len(), 8, "a short last block");
        assert_eq!(quiet.span(0.0, 4000.0).centroid(), None);
        // Half a block weighs half; outside the sound there is nothing.
        let t = Tones::analyse(&sine(1000.0, rate, 0.1), rate);
        let (whole, half) = (t.span(1024.0, 1536.0), t.span(1024.0, 1280.0));
        assert!((half.power * 2.0 - whole.power).abs() < 1e-6);
        assert_eq!(t.span(1e9, 2e9), Tone::default());
        // Far zoomed out: sampled blocks, the same tone.
        let long = Tones::analyse(&sine(1000.0, rate, 3.0), rate);
        let c = long.span(0.0, 3.0 * 44100.0).centroid().unwrap();
        assert!((c - 1000.0).abs() < 30.0, "{c}");
    }
}
