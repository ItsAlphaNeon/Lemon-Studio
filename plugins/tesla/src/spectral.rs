// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The spectral tools: Equalize, Blur and Clean up (denoise), on one short-time Fourier transform.
//!
//! Edison's dialogs are listed in `plugins/edison/tools.md` ("Tools with a dialog"), but their DSP was not measured:
//! everything here is **Lemon Studio's design**.
//!
//! - **The transform**: frames of about 46 ms (2048 frames at 44.1 kHz, the power of two nearest 46 ms), a hop of a
//!   quarter frame, Hann windows on analysis and synthesis (their overlapped squares sum to 1.5, divided out), the
//!   signal padded by a frame at both ends so every frame is reconstructed exactly when nothing is changed.
//! - **Equalize**: a gain curve over a log frequency axis from 20 Hz to 20 kHz (flat beyond), its height v giving
//!   (v − 0.5) · 48 dB (±24 dB, mid height 0 dB), straight or smoothed (a spline) between points; Mix fades from the dry
//!   signal to the equalized one. Zero-phase.
//! - **Blur**: each frequency bin's magnitude smeared over time by the impulse curve (a bell by default) stretched over
//!   the Amount (10 ms to 2 s), placed by the Offset (−1: all before, 0: centred, 1: all after); each frame keeps its
//!   phases. Mix as above.
//! - **Clean up (denoise)**: a noise profile (each bin's mean magnitude over the range it was acquired from); each bin
//!   then keeps (SNR − 1) / SNR of its power (read over the bin and its two neighbours) above the profile raised by the
//!   Threshold (0 to 24 dB) and goes down by the Amount (0 to 60 dB) below it, the gain falling back over 60 ms rather
//!   than at once (less "musical noise"). Output noise only gives what was taken away instead.
//! - **Declipper** (in Clean up): runs of two or more samples at or over a share of the range's peak, redrawn as cubics
//!   (see [`declip`]).
//! - All of them hear a frame of audio on both sides of the range (Edison's "Use data outside selection", always on)
//!   and change only the range.
//!
// spec: plugins/tesla/README.md "Spectral tools in Tesla 1.3", plugins/edison/tools.md "Tools with a dialog"

use crate::env::Point;
use crate::spectrum::Fft;

/// The frame size for a sample rate: the power of two nearest 46 ms, 256 at least.
pub fn frame_size(rate: u32) -> usize {
    let want = rate as f64 * 0.046;
    let lo = (want as usize).next_power_of_two().max(256) / 2;
    let hi = lo * 2;
    if (want - lo as f64).abs() <= (hi as f64 - want).abs() { lo.max(256) } else { hi }
}

/// A signal's short-time spectra: `frames` frames of `n / 2 + 1` bins each, real and imaginary parts.
pub struct Stft {
    pub n: usize,
    pub frames: usize,
    pub re: Vec<f32>,
    pub im: Vec<f32>,
    len: usize,
}

fn window(n: usize) -> Vec<f32> {
    (0..n).map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()) as f32).collect()
}

impl Stft {
    pub fn bins(&self) -> usize {
        self.n / 2 + 1
    }

    /// Frame `t`'s bins (real, imaginary).
    pub fn frame(&mut self, t: usize) -> (&mut [f32], &mut [f32]) {
        let b = self.bins();
        (&mut self.re[t * b..(t + 1) * b], &mut self.im[t * b..(t + 1) * b])
    }

    pub fn analyze(x: &[f32], n: usize) -> Stft {
        let hop = n / 4;
        let padded = x.len() + 2 * n;
        let frames = (padded - n) / hop + 1;
        let b = n / 2 + 1;
        let w = window(n);
        let fft = Fft::new(n);
        let (mut re, mut im) = (vec![0f32; frames * b], vec![0f32; frames * b]);
        let (mut fr, mut fi) = (vec![0f32; n], vec![0f32; n]);
        for t in 0..frames {
            let start = (t * hop) as isize - n as isize;
            for i in 0..n {
                let j = start + i as isize;
                fr[i] = if j >= 0 && (j as usize) < x.len() { x[j as usize] * w[i] } else { 0.0 };
            }
            fi.fill(0.0);
            fft.run(&mut fr, &mut fi);
            re[t * b..(t + 1) * b].copy_from_slice(&fr[..b]);
            im[t * b..(t + 1) * b].copy_from_slice(&fi[..b]);
        }
        Stft { n, frames, re, im, len: x.len() }
    }

    /// The signal back, as long as the one analyzed.
    pub fn synthesize(&self) -> Vec<f32> {
        let (n, hop, b) = (self.n, self.n / 4, self.bins());
        let w = window(n);
        let fft = Fft::new(n);
        let mut out = vec![0f32; self.len];
        let (mut fr, mut fi) = (vec![0f32; n], vec![0f32; n]);
        // Hann² overlapped at a quarter frame sums to 1.5; the inverse transform divides by n.
        let scale = 1.0 / (1.5 * n as f32);
        for t in 0..self.frames {
            fr[..b].copy_from_slice(&self.re[t * b..(t + 1) * b]);
            fi[..b].copy_from_slice(&self.im[t * b..(t + 1) * b]);
            for k in 1..n / 2 {
                fr[n - k] = fr[k];
                fi[n - k] = -fi[k];
            }
            // The inverse through the forward transform: conjugate in and out (only the real part is kept).
            for v in fi.iter_mut() {
                *v = -*v;
            }
            fft.run(&mut fr, &mut fi);
            let start = (t * hop) as isize - n as isize;
            for i in 0..n {
                let j = start + i as isize;
                if j >= 0 && (j as usize) < out.len() {
                    out[j as usize] += fr[i] * w[i] * scale;
                }
            }
        }
        out
    }
}

/// The height of a panel curve at `x` (0 to 1): straight lines between the points, or when `smooth` a spline through
/// them (each point's slope that of the line between its neighbours, an end's that of its one segment), kept in 0 to 1.
pub fn curve_at(p: &[Point], x: f64, smooth: bool) -> f32 {
    let Some(first) = p.first() else { return 0.5 };
    if x <= first.pos {
        return first.v;
    }
    let slope = |i: usize| {
        let (a, b) = (p[i.saturating_sub(1)], p[(i + 1).min(p.len() - 1)]);
        if b.pos > a.pos { (b.v - a.v) as f64 / (b.pos - a.pos) } else { 0.0 }
    };
    for (i, w) in p.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        if x <= b.pos {
            let dx = b.pos - a.pos;
            if dx <= 0.0 {
                return b.v;
            }
            let t = (x - a.pos) / dx;
            if !smooth {
                return a.v + (b.v - a.v) * t as f32;
            }
            let (t2, t3) = (t * t, t * t * t);
            let h = (2.0 * t3 - 3.0 * t2 + 1.0) * a.v as f64
                + (t3 - 2.0 * t2 + t) * dx * slope(i)
                + (-2.0 * t3 + 3.0 * t2) * b.v as f64
                + (t3 - t2) * dx * slope(i + 1);
            return (h as f32).clamp(0.0, 1.0);
        }
    }
    p[p.len() - 1].v
}

/// The default curves: Equalize's flat line with three points, Blur's bell with five.
pub fn flat_curve() -> Vec<Point> {
    [0.0, 0.5, 1.0].iter().map(|&pos| Point { pos, v: 0.5 }).collect()
}

pub fn bell_curve() -> Vec<Point> {
    [(0.0, 0.0), (0.25, 0.5), (0.5, 1.0), (0.75, 0.5), (1.0, 0.0)].iter().map(|&(pos, v)| Point { pos, v }).collect()
}

/// The equalizer's frequency axis: x (0 to 1) to Hz, 20 Hz to 20 kHz on a log scale, and back.
pub fn eq_hz(x: f64) -> f64 {
    20.0 * 1000f64.powf(x)
}

pub fn eq_x(hz: f64) -> f64 {
    if hz <= 20.0 { 0.0 } else { ((hz / 20.0).log10() / 3.0).min(1.0) }
}

/// An equalizer curve height as dB.
pub fn eq_db(v: f32) -> f64 {
    (v.clamp(0.0, 1.0) as f64 - 0.5) * 48.0
}

/// Equalize... settings: Mix 0 to 1 (1: all equalized), Smooth (a spline through the points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Equalize {
    pub mix: f32,
    pub smooth: bool,
}

impl Default for Equalize {
    fn default() -> Self {
        Equalize { mix: 1.0, smooth: true }
    }
}

/// Blur... settings: Amount 0 to 1 (the blur's length, 10 ms · 200^amount: 10 ms to 2 s, about 141 ms at ½), Offset
/// −1 to 1, Mix 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blur {
    pub amount: f32,
    pub offset: f32,
    pub mix: f32,
}

impl Default for Blur {
    fn default() -> Self {
        Blur { amount: 0.5, offset: 0.0, mix: 1.0 }
    }
}

impl Blur {
    pub fn seconds(&self) -> f64 {
        0.01 * 200f64.powf(self.amount.clamp(0.0, 1.0) as f64)
    }
}

/// Clean up (denoise)... settings: Threshold 0 to 1 (0 to 24 dB over the profile, 6 dB by default), Amount 0 to 1 (0
/// to 60 dB of reduction, 30 dB by default), Output noise only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Denoise {
    pub threshold: f32,
    pub amount: f32,
    pub noise_only: bool,
}

impl Default for Denoise {
    fn default() -> Self {
        Denoise { threshold: 0.25, amount: 0.5, noise_only: false }
    }
}

impl Denoise {
    pub fn threshold_db(&self) -> f64 {
        self.threshold.clamp(0.0, 1.0) as f64 * 24.0
    }

    pub fn reduction_db(&self) -> f64 {
        self.amount.clamp(0.0, 1.0) as f64 * 60.0
    }
}

/// A noise profile: each bin's mean magnitude, for frames of `n` at `rate`.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub n: usize,
    pub rate: u32,
    /// One list of bins per channel.
    pub mag: Vec<Vec<f32>>,
}

/// Acquire noise profile: the mean magnitude of each bin over the channels' frames (the padding frames at the ends,
/// which hold only part of the signal, left out when there are others).
pub fn acquire(ch: &[&[f32]], rate: u32) -> Profile {
    let n = frame_size(rate);
    let mag = ch
        .iter()
        .map(|x| {
            let s = Stft::analyze(x, n);
            let b = s.bins();
            let inner: Vec<usize> = if s.frames > 8 { (4..s.frames - 4).collect() } else { (0..s.frames).collect() };
            let mut m = vec![0f32; b];
            for &t in &inner {
                for k in 0..b {
                    m[k] += s.re[t * b + k].hypot(s.im[t * b + k]);
                }
            }
            m.iter_mut().for_each(|v| *v /= inner.len().max(1) as f32);
            m
        })
        .collect();
    Profile { n, rate, mag }
}

/// Equalize one channel by `curve` at `rate`.
pub fn equalize(x: &[f32], rate: u32, curve: &[Point], eq: &Equalize) -> Vec<f32> {
    let n = frame_size(rate);
    let mut s = Stft::analyze(x, n);
    let b = s.bins();
    let mix = eq.mix.clamp(0.0, 1.0);
    let gains: Vec<f32> = (0..b)
        .map(|k| {
            let hz = k as f64 * rate as f64 / n as f64;
            let g = 10f64.powf(eq_db(curve_at(curve, eq_x(hz), eq.smooth)) / 20.0) as f32;
            1.0 - mix + mix * g
        })
        .collect();
    for t in 0..s.frames {
        let (re, im) = s.frame(t);
        for k in 0..b {
            re[k] *= gains[k];
            im[k] *= gains[k];
        }
    }
    s.synthesize()
}

/// The blur's weights over frames: the impulse curve sampled at each frame offset, from `first` (frames, may be
/// negative), summing to 1.
pub fn blur_kernel(curve: &[Point], b: &Blur, rate: u32, hop: usize) -> (isize, Vec<f32>) {
    let len = ((b.seconds() * rate as f64 / hop as f64).round() as usize).max(1);
    let first = (-((1.0 - b.offset.clamp(-1.0, 1.0) as f64) / 2.0 * (len - 1) as f64)).round() as isize;
    let mut w: Vec<f32> = (0..len).map(|i| curve_at(curve, if len == 1 { 0.5 } else { i as f64 / (len - 1) as f64 }, true).max(0.0)).collect();
    let sum: f32 = w.iter().sum();
    if sum <= 0.0 {
        w = vec![0.0; len];
        w[((-first) as usize).min(len - 1)] = 1.0;
    } else {
        w.iter_mut().for_each(|v| *v /= sum);
    }
    (first, w)
}

/// Blur one channel.
pub fn blur(x: &[f32], rate: u32, curve: &[Point], b: &Blur) -> Vec<f32> {
    let n = frame_size(rate);
    let mut s = Stft::analyze(x, n);
    let bins = s.bins();
    let (first, w) = blur_kernel(curve, b, rate, n / 4);
    let frames = s.frames;
    let mag: Vec<f32> = s.re.iter().zip(&s.im).map(|(r, i)| r.hypot(*i)).collect();
    let mix = b.mix.clamp(0.0, 1.0);
    for t in 0..s.frames {
        let (re, im) = s.frame(t);
        for k in 0..bins {
            // The blurred magnitude: frame t gathers what the kernel spreads into it from frame t − (first + j).
            let mut m = 0.0f32;
            for (j, wj) in w.iter().enumerate() {
                let src = t as isize - (first + j as isize);
                if src >= 0 && (src as usize) < frames {
                    m += wj * mag[src as usize * bins + k];
                }
            }
            let a = mag[t * bins + k];
            let (c, si) = if a > 1e-12 { (re[k] / a, im[k] / a) } else { (1.0, 0.0) };
            re[k] = (1.0 - mix) * re[k] + mix * m * c;
            im[k] = (1.0 - mix) * im[k] + mix * m * si;
        }
    }
    s.synthesize()
}

/// Denoise one channel against its profile `prof` (bins of the same frame size).
pub fn denoise(x: &[f32], rate: u32, prof: &[f32], d: &Denoise) -> Vec<f32> {
    let n = frame_size(rate);
    let mut s = Stft::analyze(x, n);
    let b = s.bins();
    if prof.len() != b {
        return x.to_vec();
    }
    let thr = 10f32.powf(d.threshold_db() as f32 / 20.0);
    let floor = 10f32.powf(-d.reduction_db() as f32 / 20.0);
    // The gain falls back by this much per hop: 60 dB in 60 ms.
    let hop_s = (n / 4) as f32 / rate.max(1) as f32;
    let release = 10f32.powf(-3.0 * hop_s / 0.06);
    let mut prev = vec![floor; b];
    let mut pw = vec![0f32; b];
    for t in 0..s.frames {
        let (re, im) = s.frame(t);
        for k in 0..b {
            pw[k] = re[k] * re[k] + im[k] * im[k];
        }
        for k in 0..b {
            // The power is judged over the bin and its neighbours (a noise bin's own reading swings widely).
            let (lo, hi) = (k.saturating_sub(1), (k + 1).min(b - 1));
            let p = pw[lo..=hi].iter().sum::<f32>() / (hi + 1 - lo) as f32;
            let noise = (prof[k] * thr).powi(2);
            let snr = p / noise.max(1e-30);
            let g = if snr > 1.0 { ((snr - 1.0) / snr).sqrt().max(floor) } else { floor };
            let g = g.max(prev[k] * release).min(1.0);
            prev[k] = g;
            let g = if d.noise_only { 1.0 - g } else { g };
            re[k] *= g;
            im[k] *= g;
        }
    }
    s.synthesize()
}

/// A panel's editable curve (Equalize's gain, Blur's impulse): up to `MAX` points over x 0 to 1, the first at 0 and the
/// last at 1. A fixed array, so the open panel's state stays a plain value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Curve {
    n: usize,
    p: [Point; Curve::MAX],
}

impl Default for Curve {
    fn default() -> Self {
        Curve::from_points(&flat_curve())
    }
}

impl Curve {
    pub const MAX: usize = 16;

    pub fn from_points(p: &[Point]) -> Curve {
        let mut c = Curve { n: 0, p: [Point { pos: 0.0, v: 0.5 }; Curve::MAX] };
        for q in p.iter().take(Curve::MAX) {
            c.p[c.n] = *q;
            c.n += 1;
        }
        c
    }

    pub fn flat() -> Curve {
        Curve::from_points(&flat_curve())
    }

    pub fn bell() -> Curve {
        Curve::from_points(&bell_curve())
    }

    pub fn points(&self) -> &[Point] {
        &self.p[..self.n]
    }

    /// Adds a point at (x, v); its index, or None when the curve is full.
    pub fn insert(&mut self, x: f64, v: f32) -> Option<usize> {
        if self.n >= Curve::MAX || self.n < 2 {
            return None;
        }
        let x = x.clamp(0.0, 1.0);
        let i = self.points().iter().position(|q| q.pos > x).unwrap_or(self.n - 1).max(1);
        self.p.copy_within(i..self.n, i + 1);
        self.p[i] = Point { pos: x, v: v.clamp(0.0, 1.0) };
        self.n += 1;
        Some(i)
    }

    /// Moves point `i`: the ends only up and down, the others between their neighbours.
    pub fn move_point(&mut self, i: usize, x: f64, v: f32) {
        if i >= self.n {
            return;
        }
        let pos = if i == 0 {
            0.0
        } else if i == self.n - 1 {
            1.0
        } else {
            x.clamp(self.p[i - 1].pos, self.p[i + 1].pos)
        };
        self.p[i] = Point { pos, v: v.clamp(0.0, 1.0) };
    }

    /// Removes point `i` (not an end); whether it went.
    pub fn remove(&mut self, i: usize) -> bool {
        if i == 0 || i + 1 >= self.n {
            return false;
        }
        self.p.copy_within(i + 1..self.n, i);
        self.n -= 1;
        true
    }
}

/// Clean up (denoise)... as a whole: the denoiser (runs when there is a noise profile) and the declipper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clean {
    pub denoise: Denoise,
    pub declip: bool,
    /// The declipper's threshold, 0 to 1: samples at or over (0.5 + level / 2) of the range's peak count as clipped
    /// (0.95 by default).
    pub level: f32,
    /// The declipper's mode, and Safe mode (Lemon Studio's design).
    pub mode: ClipMode,
    pub safe: bool,
    /// The declicker and its sensitivity, 0 to 1 (½ by default).
    pub declick: bool,
    pub sensitivity: f32,
}

impl Default for Clean {
    fn default() -> Self {
        Clean { denoise: Denoise::default(), declip: false, level: 0.9, mode: ClipMode::General, safe: false, declick: false, sensitivity: 0.5 }
    }
}

/// The declipper's modes (Lemon Studio's set; the original's list past its default "General" was not opened):
/// which runs count as clipped and how far around them the redraw reaches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipMode {
    /// Two or more samples of one sign at or over the clip level.
    #[default]
    General,
    /// Digital clipping only: two or more samples at or over the clip level and equal (within 0.01 %), flat tops.
    Digital,
    /// Soft (analog) clipping: one sample or more at or over the level, redrawn from one sample further out on each
    /// side, for rounder tops.
    Soft,
}

impl ClipMode {
    pub const ALL: [ClipMode; 3] = [ClipMode::General, ClipMode::Digital, ClipMode::Soft];

    pub fn name(self) -> &'static str {
        match self {
            ClipMode::General => "General",
            ClipMode::Digital => "Digital",
            ClipMode::Soft => "Soft",
        }
    }

    pub fn next(self) -> ClipMode {
        ClipMode::ALL[(ClipMode::ALL.iter().position(|&m| m == self).unwrap_or(0) + 1) % ClipMode::ALL.len()]
    }
}

/// Safe mode: a run longer than this is left alone (a held note, not a clip), ...
const SAFE_RUN_MS: f64 = 5.0;
/// ... and a redrawn top is kept within this factor of the clip level (+3 dB) and full scale.
const SAFE_OVER: f32 = std::f32::consts::SQRT_2;

/// Redraws `x[i..j]` as a cubic from `x[i - 1]` to `x[j]`, its slopes those of the sample pairs just outside (`x[i -
/// 2]` and `x[j + 1]` must exist).
fn redraw(x: &[f32], i: usize, j: usize) -> Vec<(usize, f32)> {
    let (p0, p1) = (x[i - 1] as f64, x[j] as f64);
    let len = (j - (i - 1)) as f64;
    let (m0, m1) = ((x[i - 1] - x[i - 2]) as f64 * len, (x[j + 1] - x[j]) as f64 * len);
    (i..j).map(move |k| {
        let t = (k + 1 - i) as f64 / len;
        let (t2, t3) = (t * t, t * t * t);
        let h = (2.0 * t3 - 3.0 * t2 + 1.0) * p0 + (t3 - 2.0 * t2 + t) * m0 + (-2.0 * t3 + 3.0 * t2) * p1 + (t3 - t2) * m1;
        (k, h as f32)
    })
    .collect()
}

impl Clean {
    pub fn clip_ratio(&self) -> f32 {
        0.5 + self.level.clamp(0.0, 1.0) / 2.0
    }
}

/// The declipper (Lemon Studio's design): every run of two or more samples at or over `limit` (in size, one sign) is
/// redrawn as a cubic through the samples on both sides, its slopes those of the two samples before and after, so the
/// flattened tops come back rounded (and may go over the old peak: normalize after).
pub fn declip(x: &mut [f32], limit: f32) -> usize {
    declip_with(x, limit, ClipMode::General, None)
}

/// The declipper in mode `mode`; `safe`: Safe mode at this rate (runs over 5 ms left alone, tops kept to +3 dB over
/// the clip level and full scale). The number of runs redrawn.
pub fn declip_with(x: &mut [f32], limit: f32, mode: ClipMode, safe: Option<u32>) -> usize {
    let n = x.len();
    let mut runs = 0;
    let mut i = 0;
    let longest = safe.map(|r| ((f64::from(r) * SAFE_RUN_MS / 1000.0).round() as usize).max(2));
    while i < n {
        if x[i].abs() < limit || limit <= 0.0 {
            i += 1;
            continue;
        }
        let sign = x[i].signum();
        let first = x[i];
        let mut j = i;
        let same = |v: f32| mode != ClipMode::Digital || (v - first).abs() <= first.abs() * 1e-4;
        while j < n && x[j].abs() >= limit && x[j].signum() == sign && same(x[j]) {
            j += 1;
        }
        let (min, reach) = if mode == ClipMode::Soft { (1, 1) } else { (2, 0) };
        let ok = j - i >= min && longest.is_none_or(|m| j - i <= m);
        // Samples i..j are clipped: redraw them (and `reach` more on each side) when both sides are there.
        let (a, b) = (i.saturating_sub(reach), j + reach);
        if ok && i >= reach && a >= 2 && b + 1 < n {
            let new = redraw(x, a, b);
            for (k, h) in new {
                let mut h = if h.abs() < limit && (i..j).contains(&k) { limit * sign } else { h };
                if safe.is_some() {
                    let cap = (limit * SAFE_OVER).min(1.0).max(limit);
                    h = h.clamp(-cap, cap);
                }
                x[k] = h;
            }
            runs += 1;
        }
        i = j.max(i + 1);
    }
    runs
}

/// The declicker's frame for its local level (s), and the longest click it repairs (s).
const CLICK_WINDOW_S: f64 = 0.01;
const CLICK_MAX_S: f64 = 0.002;

/// The declicker (Lemon Studio's design): a sample is part of a click where the signal's second difference is
/// `k` times its mean size over the 10 ms around it, k from 40 (sensitivity 0) to 5 (1), 14 at ½; each click, widened
/// by two samples on both sides and at most 2 ms long, is redrawn as the declipper redraws a run. The number of clicks.
pub fn declick(x: &mut [f32], rate: u32, sensitivity: f32) -> usize {
    let n = x.len();
    if n < 8 {
        return 0;
    }
    let k = 40.0 * (5.0f64 / 40.0).powf(f64::from(sensitivity.clamp(0.0, 1.0)));
    let d: Vec<f64> = (0..n).map(|i| if i == 0 || i + 1 == n { 0.0 } else { (f64::from(x[i - 1]) - 2.0 * f64::from(x[i]) + f64::from(x[i + 1])).abs() }).collect();
    let half = ((f64::from(rate) * CLICK_WINDOW_S / 2.0).round() as usize).max(4);
    let longest = ((f64::from(rate) * CLICK_MAX_S).round() as usize).max(4);
    // A running sum for the local mean.
    let mut sum = vec![0.0f64; n + 1];
    for i in 0..n {
        sum[i + 1] = sum[i] + d[i];
    }
    let click: Vec<bool> = (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(n));
            let mean = (sum[b] - sum[a]) / (b - a) as f64;
            d[i] > k * mean.max(1e-6)
        })
        .collect();
    let mut found = 0;
    let mut i = 0;
    while i < n {
        if !click[i] {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < n && (click[j] || (j + 1 < n && click[j + 1]) || (j + 2 < n && click[j + 2])) {
            j += 1;
        }
        let (a, b) = (i.saturating_sub(2), (j + 2).min(n));
        if b - a <= longest && a >= 2 && b + 1 < n {
            let new = redraw(x, a, b);
            for (k, h) in new {
                x[k] = h;
            }
            found += 1;
        }
        i = j.max(i + 1);
    }
    found
}

/// Runs `f` on frames `a..=b` of each channel with a frame of context on both sides (where the sample has it), and
/// writes back only `a..=b`: the tools hear the audio around the selection, as Edison's "Use data outside selection".
pub fn in_context(s: &mut crate::sample::Sample, a: usize, b: usize, mut f: impl FnMut(usize, &[f32]) -> Vec<f32>) {
    let ctx = frame_size(s.rate);
    for (c, ch) in s.ch.iter_mut().enumerate() {
        let lo = a.saturating_sub(ctx);
        let hi = (b + 1 + ctx).min(ch.len());
        let y = f(c, &ch[lo..hi]);
        ch[a..=b].copy_from_slice(&y[a - lo..b + 1 - lo]);
    }
}

/// Equalize... over frames `a..=b`.
pub fn equalize_range(s: &mut crate::sample::Sample, a: usize, b: usize, curve: &Curve, eq: &Equalize) {
    let rate = s.rate;
    in_context(s, a, b, |_, x| equalize(x, rate, curve.points(), eq));
}

/// Blur... over frames `a..=b`.
pub fn blur_range(s: &mut crate::sample::Sample, a: usize, b: usize, curve: &Curve, bl: &Blur) {
    let rate = s.rate;
    in_context(s, a, b, |_, x| blur(x, rate, curve.points(), bl));
}

/// Clean up (denoise)... over frames `a..=b`: the denoiser when `prof` is given, then the declipper and the declicker
/// when they are on.
pub fn clean_range(s: &mut crate::sample::Sample, a: usize, b: usize, prof: Option<&Profile>, c: &Clean) {
    let rate = s.rate;
    if let Some(p) = prof.filter(|p| p.rate == rate && p.n == frame_size(rate) && !p.mag.is_empty()) {
        in_context(s, a, b, |k, x| denoise(x, rate, &p.mag[k.min(p.mag.len() - 1)], &c.denoise));
    }
    if c.declip {
        let limit = s.peak(a, b) * c.clip_ratio();
        for ch in &mut s.ch {
            declip_with(&mut ch[a..=b], limit, c.mode, c.safe.then_some(rate));
        }
    }
    if c.declick {
        for ch in &mut s.ch {
            declick(&mut ch[a..=b], rate, c.sensitivity);
        }
    }
}

/// The mean magnitude spectrum `mag` (bins of a frame of `n` at `rate`) as `cols` heights over the equalizer's axis:
/// 0 at −96 dB or less, 1 at 0 dB (a full-scale sine).
pub fn columns(mag: &[f32], n: usize, rate: u32, cols: usize) -> Vec<f32> {
    if mag.is_empty() || n == 0 {
        return Vec::new();
    }
    // A Hann window's coherent gain is ½.
    let norm = 4.0 / n as f32;
    (0..cols)
        .map(|i| {
            let x = (i as f64 + 0.5) / cols as f64;
            let (lo, hi) = (eq_hz(x - 0.5 / cols as f64), eq_hz(x + 0.5 / cols as f64));
            let bin = |hz: f64| ((hz * n as f64 / rate.max(1) as f64).round() as usize).min(mag.len() - 1);
            let (k0, k1) = (bin(lo), bin(hi).max(bin(lo)));
            let m = mag[k0..=k1].iter().cloned().fold(0f32, f32::max) * norm;
            ((20.0 * m.max(1e-9).log10() + 96.0) / 96.0).clamp(0.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Rng;

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut r = Rng::new(seed);
        (0..n).map(|_| r.gauss() as f32 * 0.1).collect()
    }

    fn sine(n: usize, hz: f64, rate: u32) -> Vec<f32> {
        (0..n).map(|i| (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32 * 0.5).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
    }

    #[test]
    fn frame_sizes_and_exact_reconstruction() {
        assert_eq!(frame_size(44100), 2048);
        assert_eq!(frame_size(48000), 2048);
        assert_eq!(frame_size(96000), 4096);
        assert_eq!(frame_size(8000), 256);
        for len in [1, 100, 2048, 10_000] {
            let x = noise(len, 3);
            let y = Stft::analyze(&x, 2048).synthesize();
            assert_eq!(y.len(), len);
            assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-5), "length {len}");
        }
    }

    #[test]
    fn curves_and_axes() {
        let flat = flat_curve();
        assert_eq!(curve_at(&flat, 0.3, true), 0.5);
        let bell = bell_curve();
        assert_eq!(curve_at(&bell, 0.5, true), 1.0);
        assert_eq!(curve_at(&bell, 0.125, false), 0.25);
        assert!((curve_at(&bell, 0.125, true) - 0.25).abs() < 1e-6, "the bell's first segment is straight");
        assert!(curve_at(&bell, 0.26, true) - curve_at(&bell, 0.24, true) > 0.03, "no shoulder at the quarter points");
        assert!((curve_at(&bell, 0.375, true) - 0.8125).abs() < 1e-6);
        assert!((eq_hz(0.5) - 632.455).abs() < 1e-3);
        assert!((eq_x(2000.0) - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(eq_x(5.0), 0.0);
        assert_eq!(eq_db(0.5), 0.0);
        assert_eq!(eq_db(1.0), 24.0);
    }

    #[test]
    fn equalize_cuts_and_boosts_by_frequency() {
        let rate = 44100;
        let lo = sine(44100, 100.0, rate);
        let hi = sine(44100, 5000.0, rate);
        // Flat: unchanged.
        let y = equalize(&lo, rate, &flat_curve(), &Equalize::default());
        assert!(lo.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-4));
        // A curve from +24 dB at the bottom to −24 dB at the top: the low tone up, the high tone down.
        let tilt = vec![Point { pos: 0.0, v: 1.0 }, Point { pos: 1.0, v: 0.0 }];
        let eq = Equalize { mix: 1.0, smooth: false };
        let g = |x: &[f32]| 20.0 * (rms(&equalize(x, rate, &tilt, &eq)[4000..40000]) / rms(&x[4000..40000])).log10();
        let (glo, ghi) = (g(&lo), g(&hi));
        let want = |hz: f64| eq_db(curve_at(&tilt, eq_x(hz), false) as f32 as f32);
        assert!((glo - want(100.0)).abs() < 0.5, "{glo} vs {}", want(100.0));
        assert!((ghi - want(5000.0)).abs() < 0.5, "{ghi} vs {}", want(5000.0));
        // Mix 0: dry.
        let y = equalize(&hi, rate, &tilt, &Equalize { mix: 0.0, smooth: false });
        assert!(hi.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-4));
    }

    #[test]
    fn blur_smears_a_click_over_its_length() {
        let rate = 44100;
        let b = Blur { amount: 0.5, offset: 1.0, mix: 1.0 };
        let (first, w) = blur_kernel(&bell_curve(), &b, rate, 512);
        assert_eq!(first, 0, "offset 1: all after");
        assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        let (first, _) = blur_kernel(&bell_curve(), &Blur { offset: -1.0, ..b }, rate, 512);
        assert_eq!(first, -(w.len() as isize - 1));
        let mut x = vec![0f32; 44100];
        x[10000] = 1.0;
        let y = blur(&x, rate, &bell_curve(), &b);
        // The energy moves later: little before the click, a lot from 50 to 150 ms after it.
        let e = |a: usize, c: usize| y[a..c].iter().map(|v| v * v).sum::<f32>();
        assert!(e(10000 + 2205, 10000 + 6615) > 10.0 * e(10000 - 4410 - 2205, 10000 - 2205));
        // Mix 0: unchanged.
        let y = blur(&x, rate, &bell_curve(), &Blur { mix: 0.0, ..b });
        assert!(x.iter().zip(&y).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    #[test]
    fn denoise_keeps_the_tone_and_removes_the_noise() {
        let rate = 44100;
        let hiss = noise(44100, 9);
        let prof = acquire(&[&hiss[..]], rate);
        assert_eq!(prof.mag[0].len(), 1025);
        let tone = sine(44100, 1000.0, rate);
        let x: Vec<f32> = hiss.iter().zip(&tone).map(|(a, b)| a + b).collect();
        let fresh = noise(44100, 77);
        let d = Denoise { threshold: 0.25, amount: 1.0, noise_only: false };
        // Noise alone, of the same kind: down by over 18 dB (some of its bins always peak over the threshold).
        let y = denoise(&fresh, rate, &prof.mag[0], &d);
        let down = 20.0 * (rms(&y[4000..40000]) / rms(&fresh[4000..40000])).log10();
        assert!(down < -18.0, "{down}");
        // The tone in the noise: kept within 1 dB, and the residual is far below the noise.
        let y = denoise(&x, rate, &prof.mag[0], &d);
        let tone_db = 20.0 * (rms(&y[4000..40000]) / rms(&tone[4000..40000])).log10();
        assert!(tone_db.abs() < 1.0, "{tone_db}");
        let resid: Vec<f32> = y.iter().zip(&tone).map(|(a, b)| a - b).collect();
        assert!(rms(&resid[4000..40000]) < 0.3 * rms(&hiss[4000..40000]));
        // Output noise only: what was removed, so the two add back up to the input.
        let n_only = denoise(&x, rate, &prof.mag[0], &Denoise { noise_only: true, ..d });
        assert!(x.iter().zip(y.iter().zip(&n_only)).skip(100).take(40000).all(|(a, (b, c))| (a - b - c).abs() < 1e-4));
    }

    #[test]
    fn curves_edit_within_their_ends() {
        let mut c = Curve::flat();
        assert_eq!(c.points().len(), 3);
        assert_eq!(c.insert(0.25, 0.9), Some(1));
        assert_eq!(c.points()[1], Point { pos: 0.25, v: 0.9 });
        c.move_point(1, 0.8, 0.1);
        assert_eq!(c.points()[1].pos, 0.5, "kept left of its right neighbour");
        c.move_point(0, 0.3, 1.0);
        assert_eq!(c.points()[0], Point { pos: 0.0, v: 1.0 }, "an end moves only up and down");
        assert!(!c.remove(0) && !c.remove(3));
        assert!(c.remove(1));
        assert_eq!(c.points().len(), 3);
        while c.insert(0.7, 0.5).is_some() {}
        assert_eq!(c.points().len(), Curve::MAX);
    }

    #[test]
    fn declip_rounds_off_flattened_tops() {
        let rate = 44100;
        let clean = sine(4410, 441.0, rate);
        let mut x: Vec<f32> = clean.iter().map(|v| v.clamp(-0.35, 0.35)).collect();
        let before = rms(&x.iter().zip(&clean).map(|(a, b)| a - b).collect::<Vec<_>>());
        let runs = declip(&mut x, 0.35 * 0.999);
        assert!(runs >= 19, "{runs} runs");
        let after = rms(&x.iter().zip(&clean).map(|(a, b)| a - b).collect::<Vec<_>>());
        assert!(after < 0.25 * before, "{after} vs {before}");
        assert!(x.iter().cloned().fold(0f32, f32::max) > 0.45);
        // Nothing clipped: nothing changes.
        let mut y = clean.clone();
        assert_eq!(declip(&mut y, 0.6), 0);
        assert_eq!(y, clean);
    }

    #[test]
    fn declipper_modes_and_safe_mode() {
        let rate = 44100;
        let clean = sine(4410, 441.0, rate);
        let clipped: Vec<f32> = clean.iter().map(|v| v.clamp(-0.35, 0.35)).collect();
        // Digital: the flat tops are found as in General.
        let (mut g, mut d) = (clipped.clone(), clipped.clone());
        let runs = declip_with(&mut g, 0.35 * 0.999, ClipMode::General, None);
        assert_eq!(declip_with(&mut d, 0.35 * 0.999, ClipMode::Digital, None), runs);
        assert_eq!(g, d);
        // A sine's own (round) tops over the level: General redraws them, Digital leaves them.
        let mut y = clean.clone();
        assert_eq!(declip_with(&mut y, 0.45, ClipMode::Digital, None), 0);
        assert_eq!(y, clean);
        assert!(declip_with(&mut y, 0.45, ClipMode::General, None) > 0);
        // Soft: single samples count too, and the redraw reaches one sample further.
        let mut s = clipped.clone();
        assert!(declip_with(&mut s, 0.35 * 0.999, ClipMode::Soft, None) >= runs);
        assert_ne!(s, g);
        // Safe: the tops stay within +3 dB of the clip level; a run over 5 ms is left alone.
        let mut f = clipped.clone();
        declip_with(&mut f, 0.35 * 0.999, ClipMode::General, Some(rate));
        assert!(f.iter().all(|v| v.abs() <= 0.35 * 0.999 * std::f32::consts::SQRT_2 + 1e-6));
        let mut held = vec![0.0f32; 1000];
        held.extend(vec![0.5f32; 441]);
        held.extend(vec![0.0f32; 1000]);
        let before = held.clone();
        assert_eq!(declip_with(&mut held, 0.45, ClipMode::General, Some(rate)), 0);
        assert_eq!(held, before);
    }

    #[test]
    fn declicker_redraws_clicks_and_leaves_the_rest() {
        let rate = 44100;
        let clean = sine(44100, 441.0, rate);
        let mut x = clean.clone();
        for at in [5000, 17000, 30001] {
            x[at] += 0.7;
            x[at + 1] -= 0.4;
        }
        let n = declick(&mut x, rate, 0.5);
        assert_eq!(n, 3);
        for at in [5000, 17000, 30001] {
            assert!((x[at] - clean[at]).abs() < 0.02 && (x[at + 1] - clean[at + 1]).abs() < 0.02, "{} vs {}", x[at], clean[at]);
        }
        assert_eq!(x[..4990], clean[..4990]);
        assert_eq!(x[30010..], clean[30010..]);
        // A clean sine has no clicks, at any sensitivity.
        let mut y = clean.clone();
        assert_eq!(declick(&mut y, rate, 1.0), 0);
        assert_eq!(y, clean);
    }

    #[test]
    fn spectrum_columns_read_a_sine_at_its_frequency() {
        let rate = 44100;
        let p = acquire(&[&sine(44100, 1000.0, rate)[..]], rate);
        let cols = columns(&p.mag[0], p.n, rate, 300);
        let peak = cols.iter().enumerate().fold((0, 0f32), |m, (i, v)| if *v > m.1 { (i, *v) } else { m });
        assert_eq!(peak.0, (eq_x(1000.0) * 300.0) as usize);
        // A 0.5 sine: −6 dB.
        assert!((peak.1 - 90.0 / 96.0).abs() < 0.03, "{}", peak.1);
    }
}
