// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The direct tools, on a range of the sample (the selection, or the whole sample): normalize, reverse polarity,
//! center, reverse, swap channels, the mono conversions, generate noise, the fades and declicks with Edison's
//! measured curves, and Amp with Edison's measured laws.
//!
// spec: plugins/edison/tools.md ("Measured direct tools", "Fade curves", "Declick tables", "Amp, measured"),
// plugins/tesla/README.md "Version 1" (tools)

use crate::sample::Sample;

/// Every column of the fade tables has 33 points, x = 0 to 1 by 1/32.
const POINTS: usize = 33;

/// The range lengths (frames) of the long fade columns.
const LENGTHS: [f64; 8] = [2206.0, 4410.0, 14000.0, 44100.0, 140000.0, 441000.0, 1400000.0, 4410000.0];

/// Fade in: the "short" column (ranges of 50 ms or less), then one column per length in [`LENGTHS`].
#[rustfmt::skip]
const FADE_IN: [[f32; POINTS]; 9] = [
    [0.00000, 0.00248, 0.00974, 0.02172, 0.03829, 0.05929, 0.08454, 0.11378, 0.14672, 0.18307, 0.22246, 0.26452, 0.30884, 0.35500, 0.40255, 0.45104, 0.50000, 0.54896, 0.59745, 0.64500, 0.69116, 0.73548, 0.77754, 0.81693, 0.85328, 0.88622, 0.91546, 0.94071, 0.96171, 0.97828, 0.99026, 0.99752, 1.00000],
    [0.00045, 0.03217, 0.06632, 0.10408, 0.14573, 0.19098, 0.23917, 0.28949, 0.34106, 0.39304, 0.44464, 0.49519, 0.54409, 0.59091, 0.63528, 0.67695, 0.71577, 0.75165, 0.78458, 0.81458, 0.84174, 0.86617, 0.88801, 0.90741, 0.92454, 0.93957, 0.95267, 0.96402, 0.97377, 0.98210, 0.98914, 0.99504, 0.99994],
    [0.00023, 0.03196, 0.06610, 0.10386, 0.14552, 0.19076, 0.23895, 0.28928, 0.34086, 0.39285, 0.44446, 0.49501, 0.54394, 0.59076, 0.63515, 0.67684, 0.71568, 0.75157, 0.78451, 0.81452, 0.84169, 0.86614, 0.88799, 0.90740, 0.92454, 0.93957, 0.95268, 0.96404, 0.97380, 0.98212, 0.98917, 0.99507, 0.99997],
    [0.00007, 0.03181, 0.06597, 0.10376, 0.14545, 0.19075, 0.23901, 0.28940, 0.34105, 0.39310, 0.44477, 0.49537, 0.54433, 0.59119, 0.63560, 0.67730, 0.71613, 0.75202, 0.78494, 0.81494, 0.84209, 0.86650, 0.88832, 0.90770, 0.92480, 0.93980, 0.95288, 0.96420, 0.97393, 0.98222, 0.98924, 0.99512, 0.99999],
    [0.00002, 0.03177, 0.06598, 0.10388, 0.14573, 0.19123, 0.23971, 0.29033, 0.34221, 0.39447, 0.44632, 0.49708, 0.54615, 0.59308, 0.63753, 0.67923, 0.71803, 0.75386, 0.78670, 0.81659, 0.84361, 0.86789, 0.88957, 0.90881, 0.92577, 0.94063, 0.95356, 0.96475, 0.97435, 0.98253, 0.98944, 0.99521, 1.00000],
    [0.00001, 0.03179, 0.06616, 0.10439, 0.14676, 0.19290, 0.24211, 0.29348, 0.34608, 0.39901, 0.45143, 0.50264, 0.55206, 0.59921, 0.64374, 0.68542, 0.72410, 0.75972, 0.79227, 0.82180, 0.84843, 0.87228, 0.89351, 0.91228, 0.92877, 0.94318, 0.95567, 0.96644, 0.97564, 0.98345, 0.99002, 0.99549, 1.00000],
    [0.00000, 0.03188, 0.06678, 0.10612, 0.15012, 0.19830, 0.24976, 0.30343, 0.35823, 0.41313, 0.46722, 0.51973, 0.57005, 0.61772, 0.66240, 0.70389, 0.74207, 0.77693, 0.80851, 0.83692, 0.86230, 0.88482, 0.90467, 0.92206, 0.93719, 0.95027, 0.96150, 0.97107, 0.97917, 0.98596, 0.99160, 0.99623, 1.00000],
    [0.00000, 0.03222, 0.06897, 0.11209, 0.16150, 0.21615, 0.27450, 0.33491, 0.39583, 0.45589, 0.51399, 0.56929, 0.62117, 0.66923, 0.71326, 0.75319, 0.78908, 0.82107, 0.84935, 0.87417, 0.89580, 0.91453, 0.93062, 0.94437, 0.95603, 0.96585, 0.97406, 0.98088, 0.98649, 0.99106, 0.99475, 0.99769, 1.00000],
    [0.00000, 0.03365, 0.07757, 0.13386, 0.20028, 0.27310, 0.34855, 0.42343, 0.49532, 0.56257, 0.62418, 0.67968, 0.72897, 0.77224, 0.80984, 0.84222, 0.86990, 0.89340, 0.91322, 0.92983, 0.94369, 0.95518, 0.96467, 0.97245, 0.97880, 0.98395, 0.98811, 0.99144, 0.99409, 0.99617, 0.99780, 0.99905, 1.00000],
];

/// Fade out, the same columns.
#[rustfmt::skip]
const FADE_OUT: [[f32; POINTS]; 9] = [
    [1.00000, 0.99752, 0.99026, 0.97828, 0.96171, 0.94071, 0.91546, 0.88622, 0.85328, 0.81693, 0.77754, 0.73548, 0.69116, 0.64500, 0.59745, 0.54896, 0.50000, 0.45104, 0.40255, 0.35500, 0.30884, 0.26452, 0.22246, 0.18307, 0.14672, 0.11378, 0.08454, 0.05929, 0.03829, 0.02172, 0.00974, 0.00248, 0.00000],
    [0.99955, 0.96776, 0.93324, 0.89469, 0.85186, 0.80514, 0.75530, 0.70328, 0.65006, 0.59658, 0.54368, 0.49211, 0.44244, 0.39515, 0.35059, 0.30897, 0.27045, 0.23507, 0.20281, 0.17362, 0.14737, 0.12392, 0.10311, 0.08476, 0.06868, 0.05468, 0.04257, 0.03218, 0.02331, 0.01582, 0.00954, 0.00433, 0.00006],
    [0.99977, 0.96791, 0.93300, 0.89365, 0.84963, 0.80143, 0.74996, 0.69627, 0.64146, 0.58656, 0.53247, 0.47997, 0.42965, 0.38199, 0.33733, 0.29586, 0.25770, 0.22286, 0.19130, 0.16291, 0.13756, 0.11506, 0.09522, 0.07785, 0.06274, 0.04968, 0.03846, 0.02890, 0.02082, 0.01404, 0.00841, 0.00378, 0.00002],
    [0.99993, 0.96772, 0.93095, 0.88783, 0.83841, 0.78375, 0.72540, 0.66498, 0.60407, 0.54400, 0.48590, 0.43061, 0.37874, 0.33068, 0.28666, 0.24673, 0.21085, 0.17887, 0.15060, 0.12578, 0.10416, 0.08544, 0.06935, 0.05561, 0.04395, 0.03414, 0.02593, 0.01911, 0.01351, 0.00894, 0.00525, 0.00231, 0.00000],
    [0.99998, 0.96633, 0.92241, 0.86611, 0.79968, 0.72685, 0.65140, 0.57653, 0.50464, 0.43739, 0.37578, 0.32029, 0.27100, 0.22773, 0.19014, 0.15776, 0.13008, 0.10659, 0.08677, 0.07016, 0.05630, 0.04481, 0.03533, 0.02755, 0.02120, 0.01604, 0.01189, 0.00856, 0.00591, 0.00383, 0.00220, 0.00095, 0.00000],
    [0.99999, 0.95920, 0.88773, 0.79368, 0.69169, 0.59217, 0.50080, 0.41990, 0.34990, 0.29019, 0.23975, 0.19742, 0.16207, 0.13265, 0.10823, 0.08801, 0.07131, 0.05755, 0.04624, 0.03696, 0.02936, 0.02317, 0.01813, 0.01404, 0.01074, 0.00809, 0.00597, 0.00428, 0.00295, 0.00190, 0.00109, 0.00047, 0.00000],
    [1.00000, 0.92983, 0.80205, 0.67349, 0.56100, 0.46589, 0.38620, 0.31963, 0.26411, 0.21785, 0.17935, 0.14735, 0.12079, 0.09877, 0.08053, 0.06547, 0.05303, 0.04279, 0.03438, 0.02747, 0.02183, 0.01722, 0.01348, 0.01044, 0.00799, 0.00601, 0.00444, 0.00318, 0.00219, 0.00141, 0.00081, 0.00035, 0.00000],
    [1.00000, 0.87645, 0.73063, 0.60790, 0.50519, 0.41929, 0.34752, 0.28761, 0.23764, 0.19602, 0.16138, 0.13259, 0.10868, 0.08887, 0.07246, 0.05891, 0.04772, 0.03850, 0.03093, 0.02472, 0.01964, 0.01550, 0.01213, 0.00939, 0.00719, 0.00541, 0.00399, 0.00286, 0.00197, 0.00127, 0.00073, 0.00031, 0.00000],
    [1.00000, 0.84747, 0.70590, 0.58732, 0.48808, 0.40509, 0.33575, 0.27787, 0.22960, 0.18938, 0.15592, 0.12810, 0.10500, 0.08586, 0.07001, 0.05691, 0.04610, 0.03720, 0.02988, 0.02388, 0.01898, 0.01497, 0.01171, 0.00907, 0.00694, 0.00523, 0.00386, 0.00277, 0.00191, 0.00123, 0.00071, 0.00030, 0.00000],
];

/// Declick in: gain by time from the range start, every 0.25 ms over 5 ms.
const DECLICK_IN: [f32; 21] = [0.0, 0.0, 0.0001, 0.0042, 0.0433, 0.1534, 0.3244, 0.5076, 0.6642, 0.7811, 0.8615, 0.9141, 0.9474, 0.9682, 0.9810, 0.9889, 0.9936, 0.9965, 0.9983, 0.9994, 1.0];
const DECLICK_IN_MS: f64 = 5.0;

/// Declick out: gain by time before the range's last frame, every 2 ms from 10 ms (0) to 40 ms (1).
const DECLICK_OUT: [f32; 16] = [0.0, 0.0001, 0.0046, 0.0390, 0.1406, 0.3057, 0.4891, 0.6502, 0.7727, 0.8580, 0.9142, 0.9500, 0.9724, 0.9863, 0.9948, 1.0];
const DECLICK_OUT_MS: (f64, f64) = (10.0, 40.0);

/// A cubic spline with not-a-knot ends through evenly spaced points over 0..1.
pub struct Spline {
    y: Vec<f64>,
    m: Vec<f64>,
}

impl Spline {
    pub fn new(y: &[f64]) -> Spline {
        let n = y.len();
        assert!(n >= 4);
        let h = 1.0 / (n - 1) as f64;
        // Second derivatives M: interior rows M[i-1] + 4 M[i] + M[i+1] = 6 / h² · (second difference); the ends are
        // not-a-knot (the third derivative doesn't jump at the second and second-to-last points).
        let mut a = vec![vec![0.0; n + 1]; n];
        a[0][0] = 1.0;
        a[0][1] = -2.0;
        a[0][2] = 1.0;
        a[n - 1][n - 3] = 1.0;
        a[n - 1][n - 2] = -2.0;
        a[n - 1][n - 1] = 1.0;
        for i in 1..n - 1 {
            a[i][i - 1] = 1.0;
            a[i][i] = 4.0;
            a[i][i + 1] = 1.0;
            a[i][n] = 6.0 / (h * h) * (y[i - 1] - 2.0 * y[i] + y[i + 1]);
        }
        // Gaussian elimination with partial pivoting.
        for col in 0..n {
            let p = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs())).unwrap();
            a.swap(col, p);
            let d = a[col][col];
            if d.abs() < 1e-300 {
                continue;
            }
            for row in 0..n {
                if row != col {
                    let f = a[row][col] / d;
                    if f != 0.0 {
                        for k in col..=n {
                            a[row][k] -= f * a[col][k];
                        }
                    }
                }
            }
        }
        let m = (0..n).map(|i| if a[i][i].abs() < 1e-300 { 0.0 } else { a[i][n] / a[i][i] }).collect();
        Spline { y: y.to_vec(), m }
    }

    pub fn at(&self, x: f64) -> f64 {
        let n = self.y.len();
        let h = 1.0 / (n - 1) as f64;
        let x = x.clamp(0.0, 1.0);
        let k = ((x / h) as usize).min(n - 2);
        let (x0, x1) = (k as f64 * h, (k + 1) as f64 * h);
        let (a, b) = (x1 - x, x - x0);
        self.m[k] * a * a * a / (6.0 * h) + self.m[k + 1] * b * b * b / (6.0 * h) + (self.y[k] / h - self.m[k] * h / 6.0) * a + (self.y[k + 1] / h - self.m[k + 1] * h / 6.0) * b
    }
}

/// The 33 points of the fade curve for a range of `n` frames at `rate`: the short S-curve up to 50 ms, else the long
/// columns interpolated linearly in log n (clamped to the measured lengths).
fn fade_points(table: &[[f32; POINTS]; 9], n: usize, rate: u32) -> Vec<f64> {
    if n as u64 * 20 <= rate as u64 {
        return table[0].iter().map(|&v| v as f64).collect();
    }
    let ln = (n as f64).ln();
    let k = LENGTHS.iter().position(|&l| l.ln() >= ln).unwrap_or(LENGTHS.len());
    if k == 0 {
        return table[1].iter().map(|&v| v as f64).collect();
    }
    if k == LENGTHS.len() {
        return table[LENGTHS.len()].iter().map(|&v| v as f64).collect();
    }
    let (l0, l1) = (LENGTHS[k - 1].ln(), LENGTHS[k].ln());
    let t = (ln - l0) / (l1 - l0);
    (0..POINTS).map(|i| table[k][i] as f64 * (1.0 - t) + table[k + 1][i] as f64 * t).collect()
}

/// Fade gains for a range of `n` frames, in (`true`) or out.
pub fn fade_curve(n: usize, rate: u32, fade_in: bool) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![if fade_in { 0.0 } else { 1.0 }];
    }
    let sp = Spline::new(&fade_points(if fade_in { &FADE_IN } else { &FADE_OUT }, n, rate));
    (0..n).map(|i| sp.at(i as f64 / (n - 1) as f64).clamp(0.0, 1.0) as f32).collect()
}

fn spline_of(t: &[f32]) -> Spline {
    Spline::new(&t.iter().map(|&v| v as f64).collect::<Vec<_>>())
}

/// Declick-in gains for a range of `n` frames: the first 5 ms (a range shorter than that gets the curve squeezed into
/// it, Tesla's own choice), then 1.
pub fn declick_in_curve(n: usize, rate: u32) -> Vec<f32> {
    let sp = spline_of(&DECLICK_IN);
    let span = (DECLICK_IN_MS * rate as f64 / 1000.0).min((n.max(1) - 1) as f64).max(1.0);
    let mut g: Vec<f32> = (0..n).map(|i| if i as f64 >= span { 1.0 } else { sp.at(i as f64 / span).clamp(0.0, 1.0) as f32 }).collect();
    // The spline may ripple between the measured points: a rising gain never dips.
    for i in 1..g.len() {
        g[i] = g[i].max(g[i - 1]);
    }
    g
}

/// Declick-out gains for a range of `n` frames: 1 until 40 ms before the last frame, the measured fall, 0 for the last
/// 10 ms. On a range shorter than 40 ms the 40 ms curve is squeezed into the range (Tesla's own rule; Edison's
/// differs there and isn't characterised).
pub fn declick_out_curve(n: usize, rate: u32) -> Vec<f32> {
    let sp = spline_of(&DECLICK_OUT);
    let ms_per = 1000.0 / rate as f64;
    let range_ms = (n.max(1) - 1) as f64 * ms_per;
    let squeeze = if range_ms < DECLICK_OUT_MS.1 && range_ms > 0.0 { DECLICK_OUT_MS.1 / range_ms } else { 1.0 };
    let mut g: Vec<f32> = (0..n)
        .map(|i| {
            let d = (n - 1 - i) as f64 * ms_per * squeeze;
            if d >= DECLICK_OUT_MS.1 - 1e-9 {
                1.0
            } else if d <= DECLICK_OUT_MS.0 + 1e-9 {
                0.0
            } else {
                sp.at((d - DECLICK_OUT_MS.0) / (DECLICK_OUT_MS.1 - DECLICK_OUT_MS.0)).clamp(0.0, 1.0) as f32
            }
        })
        .collect();
    // A falling gain never rises again (the spline may ripple between the measured points).
    for i in 1..g.len() {
        g[i] = g[i].min(g[i - 1]);
    }
    g
}

/// Applies gains to frames `a..` of every channel.
fn apply(s: &mut Sample, a: usize, g: &[f32]) {
    for c in &mut s.ch {
        for (v, k) in c[a..a + g.len()].iter_mut().zip(g) {
            *v *= k;
        }
    }
}

pub fn fade_in(s: &mut Sample, a: usize, b: usize) {
    let g = fade_curve(b + 1 - a, s.rate, true);
    apply(s, a, &g);
}

pub fn fade_out(s: &mut Sample, a: usize, b: usize) {
    let g = fade_curve(b + 1 - a, s.rate, false);
    apply(s, a, &g);
}

pub fn declick_in(s: &mut Sample, a: usize, b: usize) {
    let g = declick_in_curve(b + 1 - a, s.rate);
    apply(s, a, &g);
}

pub fn declick_out(s: &mut Sample, a: usize, b: usize) {
    let g = declick_out_curve(b + 1 - a, s.rate);
    apply(s, a, &g);
}

/// One gain for every channel so the larger peak becomes exactly 1.0. False when the range is silent.
pub fn normalize(s: &mut Sample, a: usize, b: usize) -> bool {
    let p = s.peak(a, b);
    if p <= 0.0 {
        return false;
    }
    let g = 1.0 / p;
    for c in &mut s.ch {
        for v in &mut c[a..=b] {
            *v *= g;
        }
    }
    true
}

pub fn reverse_polarity(s: &mut Sample, a: usize, b: usize) {
    for c in &mut s.ch {
        for v in &mut c[a..=b] {
            *v = -*v;
        }
    }
}

/// Removes DC: each channel's mean over the range (Tesla's method; Edison's on non-constant signals is open).
pub fn center(s: &mut Sample, a: usize, b: usize) {
    for c in &mut s.ch {
        let mean = c[a..=b].iter().map(|&v| v as f64).sum::<f64>() / (b + 1 - a) as f64;
        for v in &mut c[a..=b] {
            *v = (*v as f64 - mean) as f32;
        }
    }
}

pub fn reverse(s: &mut Sample, a: usize, b: usize) {
    for c in &mut s.ch {
        c[a..=b].reverse();
    }
}

pub fn swap_channels(s: &mut Sample, a: usize, b: usize) {
    if s.channels() == 2 {
        let (l, r) = s.ch.split_at_mut(1);
        l[0][a..=b].swap_with_slice(&mut r[0][a..=b]);
    }
}

/// Convert left (0) or right (1) channel to mono: the sample keeps that channel only.
pub fn channel_to_mono(s: &mut Sample, c: usize) {
    if s.channels() == 2 {
        let keep = s.ch.swap_remove(c.min(1));
        s.ch = vec![keep];
    }
}

/// Convert mono signal to mono format: a stereo sample whose channels are identical becomes one channel. False when
/// they differ (the sample is left as it was).
pub fn mono_signal_to_mono(s: &mut Sample) -> bool {
    if s.channels() == 2 && s.ch[0] == s.ch[1] {
        s.ch.truncate(1);
        return true;
    }
    false
}

/// Format > Mono / Stereo: stereo becomes the channels' average (Tesla's choice; Edison's conversion isn't measured),
/// mono becomes two copies.
pub fn set_channels(s: &mut Sample, n: usize) {
    if n == 1 && s.channels() == 2 {
        let m: Vec<f32> = s.ch[0].iter().zip(&s.ch[1]).map(|(l, r)| (l + r) * 0.5).collect();
        s.ch = vec![m];
    } else if n == 2 && s.channels() == 1 {
        let c = s.ch[0].clone();
        s.ch.push(c);
    }
}

/// A small, fast generator (xorshift64*) for the noise tool.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in (0, 1].
    fn unit(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 1.0) / (1u64 << 53) as f64
    }

    /// Standard Gaussian (Box-Muller).
    pub fn gauss(&mut self) -> f64 {
        let (u, v) = (self.unit(), self.unit());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

/// Replaces the range with white Gaussian noise, the channels independent, then normalized together (the louder
/// channel peaks at exactly 1.0).
pub fn generate_noise(s: &mut Sample, a: usize, b: usize, seed: u64) {
    let mut rng = Rng::new(seed);
    for c in &mut s.ch {
        for v in &mut c[a..=b] {
            *v = rng.gauss() as f32;
        }
    }
    normalize(s, a, b);
}

/// The Amp dialog's panning law.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Law {
    #[default]
    Circular,
    Triangular,
}

/// The Amp dialog's controls: pan and stereo separation from −1 to 1; fader positions from −1 to 1 (½ = ×1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Amp {
    pub pan: f32,
    pub law: Law,
    pub left: f32,
    pub volume: f32,
    pub right: f32,
    pub sep: f32,
}

impl Default for Amp {
    fn default() -> Self {
        Amp { pan: 0.0, law: Law::Circular, left: 0.5, volume: 0.5, right: 0.5, sep: 0.0 }
    }
}

/// A fader position's gain: sign(v) · (9^|v| − 1) / 2 (½ = ×1, the top ×4, the middle silent, the lower half inverted).
pub fn fader_gain(v: f32) -> f64 {
    let v = v.clamp(-1.0, 1.0) as f64;
    v.signum() * (9f64.powf(v.abs()) - 1.0) / 2.0
}

/// Stereo separation's width factor on the side signal: 1 − s turned left (up to 2), (5 − 5^s) / 4 turned right
/// (down to 0, mono).
pub fn width(s: f32) -> f64 {
    let s = s.clamp(-1.0, 1.0) as f64;
    if s < 0.0 { 1.0 - s } else { (5.0 - 5f64.powf(s)) / 4.0 }
}

impl Amp {
    /// The 2 × 2 matrix from (L, R) in to (L, R) out: `[[ll, lr], [rl, rr]]`.
    pub fn matrix(&self) -> [[f64; 2]; 2] {
        let q = self.pan.abs().clamp(0.0, 1.0) as f64;
        let (a, b) = match self.law {
            Law::Circular => {
                let x = q * std::f64::consts::FRAC_PI_4;
                (x.cos() + x.sin(), x.cos() - x.sin())
            }
            Law::Triangular => (1.0, 1.0 - q),
        };
        let s = width(self.sep);
        let k = (a * (s - 1.0 + b)).max(0.0);
        // M = (L + R) / 2, D = (L − R) / 2.
        let (l, r) = if self.pan <= 0.0 {
            // L' = A·M + K·D, R' = B·(M − S·D)
            ([(a + k) / 2.0, (a - k) / 2.0], [b * (1.0 - s) / 2.0, b * (1.0 + s) / 2.0])
        } else {
            // L' = B·(M + S·D), R' = A·M − K·D
            ([b * (1.0 + s) / 2.0, b * (1.0 - s) / 2.0], [(a - k) / 2.0, (a + k) / 2.0])
        };
        let v = fader_gain(self.volume);
        let (gl, gr) = (v * fader_gain(self.left), v * fader_gain(self.right));
        [[gl * l[0], gl * l[1]], [gr * r[0], gr * r[1]]]
    }
}

/// Applies Amp to the range. A mono sample takes the left output of its signal fed to both inputs.
pub fn amp(s: &mut Sample, a: usize, b: usize, amp: &Amp) {
    let m = amp.matrix();
    if s.channels() == 2 {
        let (l, r) = s.ch.split_at_mut(1);
        for (x, y) in l[0][a..=b].iter_mut().zip(&mut r[0][a..=b]) {
            let (lx, rx) = (*x as f64, *y as f64);
            *x = (m[0][0] * lx + m[0][1] * rx) as f32;
            *y = (m[1][0] * lx + m[1][1] * rx) as f32;
        }
    } else {
        let g = m[0][0] + m[0][1];
        for v in &mut s.ch[0][a..=b] {
            *v = (*v as f64 * g) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Format;

    fn ones(n: usize, rate: u32) -> Sample {
        Sample::from_channels(rate, vec![vec![1.0; n], vec![1.0; n]], Format::Float32)
    }

    #[test]
    fn spline_goes_through_its_points() {
        let y: Vec<f64> = FADE_IN[4].iter().map(|&v| v as f64).collect();
        let sp = Spline::new(&y);
        for (i, v) in y.iter().enumerate() {
            assert!((sp.at(i as f64 / 32.0) - v).abs() < 1e-9);
        }
        // A cubic is reproduced exactly by a not-a-knot spline.
        let cubic: Vec<f64> = (0..9).map(|i| (i as f64 / 8.0).powi(3)).collect();
        let sp = Spline::new(&cubic);
        assert!((sp.at(0.3) - 0.027).abs() < 1e-9);
    }

    #[test]
    fn fades_follow_the_measured_columns() {
        // 1 s at 44.1 kHz: the 44,100 column; the curve passes its points.
        let g = fade_curve(44100, 44100, true);
        assert!((g[0] - 0.00002).abs() < 1e-5);
        assert!((g[44099] - 1.0).abs() < 1e-5);
        let at = |g: &[f32], x: f64| g[(x * (g.len() - 1) as f64).round() as usize];
        assert!((at(&g, 0.25) - 0.34221).abs() < 2e-4);
        let o = fade_curve(44100, 44100, false);
        assert!((at(&o, 0.25) - 0.50464).abs() < 2e-4);
        assert_eq!(o[44099], 0.0);
        // 50 ms or less: the symmetric S-curve, fade in + fade out = 1.
        let fi = fade_curve(2205, 44100, true);
        let fo = fade_curve(2205, 44100, false);
        assert!(fi.iter().zip(&fo).all(|(a, b)| (a + b - 1.0).abs() < 1e-4));
        assert!((at(&fi, 0.5) - 0.5).abs() < 1e-3);
        // One frame more: the long curve.
        let fo2 = fade_curve(2206, 44100, false);
        assert!((at(&fo2, 0.25) - 0.65006).abs() < 1e-3);
        // Rate doesn't matter for the long curves; 48,000 frames sit between the 44,100 and 140,000 columns.
        assert_eq!(fade_curve(48000, 44100, false), fade_curve(48000, 48000, false));
        let mid = at(&fade_curve(48000, 48000, false), 0.25) as f64;
        assert!(mid < 0.50464 && mid > 0.34990);
        // Beyond the last column: clamped.
        let big = fade_points(&FADE_OUT, 9_000_000, 44100);
        assert_eq!(big[8], FADE_OUT[8][8] as f64);
    }

    #[test]
    fn declicks_are_fixed_times() {
        let g = declick_in_curve(44100, 44100);
        assert_eq!(g[0], 0.0);
        assert!((g[44] - 0.0433).abs() < 2e-3, "{}", g[44]);
        assert_eq!(g[221], 1.0);
        let g96 = declick_in_curve(96000, 96000);
        assert!((g96[96] - 0.0433).abs() < 2e-3);
        let o = declick_out_curve(44100, 44100);
        assert_eq!(o[44099], 0.0);
        assert_eq!(o[44099 - 441], 0.0);
        assert!((o[44099 - 882] - 0.3057).abs() < 2e-3, "{}", o[44099 - 882]);
        assert_eq!(o[44099 - 1764], 1.0);
        // A 20 ms range gets the whole curve squeezed in: 1 at the start, 0 at the end, monotone.
        let sq = declick_out_curve(882, 44100);
        assert_eq!(sq[0], 1.0);
        assert_eq!(sq[881], 0.0);
        assert!(sq.windows(2).all(|w| w[1] <= w[0] + 1e-6));
    }

    #[test]
    fn amp_matches_the_measured_matrices() {
        let close = |m: [[f64; 2]; 2], w: [[f64; 2]; 2]| (0..2).all(|i| (0..2).all(|j| (m[i][j] - w[i][j]).abs() < 1e-4));
        assert!(close(Amp::default().matrix(), [[1.0, 0.0], [0.0, 1.0]]));
        // Hard left, Circular: L' = 0.7071 (L + R), R' = 0. Triangular: 0.5 (L + R).
        let hl = Amp { pan: -1.0, ..Amp::default() };
        assert!(close(hl.matrix(), [[0.70711, 0.70711], [0.0, 0.0]]), "{:?}", hl.matrix());
        assert!(close(Amp { law: Law::Triangular, ..hl }.matrix(), [[0.5, 0.5], [0.0, 0.0]]));
        // 10 notches right (p = 0.3125).
        let p = Amp { pan: 0.3125, ..Amp::default() };
        assert!(close(p.matrix(), [[0.7271, 0.0], [0.1656, 1.0475]]), "{:?}", p.matrix());
        assert!(close(Amp { law: Law::Triangular, ..p }.matrix(), [[0.6875, 0.0], [0.15625, 0.84375]]));
        // Full mono and S = 2.
        assert!(close(Amp { sep: 1.0, ..Amp::default() }.matrix(), [[0.5, 0.5], [0.5, 0.5]]));
        assert!(close(Amp { sep: -1.0, ..Amp::default() }.matrix(), [[1.5, -0.5], [-0.5, 1.5]]));
        // The clamp: hard left with S = 0.69 still gives 0.7071 (L + R).
        let s = (5.0 - 0.69 * 4.0f64).ln() / 5f64.ln();
        assert!(close(Amp { pan: -1.0, sep: s as f32, ..Amp::default() }.matrix(), [[0.70711, 0.70711], [0.0, 0.0]]));
        // Faders: ½ = ×1, top ×4, bottom ×−4, middle 0; measured widths.
        assert!((fader_gain(0.5) - 1.0).abs() < 1e-12 && (fader_gain(1.0) - 4.0).abs() < 1e-12 && fader_gain(0.0) == 0.0 && (fader_gain(-1.0) + 4.0).abs() < 1e-12);
        assert!((width(5.0 / 32.0) - 0.9285).abs() < 1e-4 && (width(28.0 / 32.0) - 0.2278).abs() < 1e-4);
    }

    #[test]
    fn direct_tools() {
        let mut s = Sample::from_channels(44100, vec![vec![0.5; 100], vec![-0.25; 100]], Format::Float32);
        assert!(normalize(&mut s, 0, 99));
        assert_eq!((s.ch[0][0], s.ch[1][0]), (1.0, -0.5));
        center(&mut s, 0, 99);
        assert!(s.ch[0].iter().chain(&s.ch[1]).all(|&v| v.abs() < 1e-7));
        let mut r = Sample::from_channels(44100, vec![(0..10).map(|i| i as f32).collect(), vec![9.0; 10]], Format::Float32);
        reverse(&mut r, 0, 9);
        assert_eq!(r.ch[0][0], 9.0);
        swap_channels(&mut r, 0, 9);
        assert_eq!(r.ch[1][0], 9.0);
        assert_eq!(r.ch[0][1], 9.0);
        reverse_polarity(&mut r, 0, 0);
        assert_eq!(r.ch[0][0], -9.0);
        let mut m = ones(10, 44100);
        assert!(mono_signal_to_mono(&mut m));
        assert_eq!(m.channels(), 1);
        let mut n = ones(44100, 44100);
        generate_noise(&mut n, 0, 44099, 7);
        assert!((n.peak(0, 44099) - 1.0).abs() < 1e-6);
        let corr: f64 = n.ch[0].iter().zip(&n.ch[1]).map(|(a, b)| (*a as f64) * (*b as f64)).sum::<f64>() / 44100.0;
        assert!(corr.abs() < 0.01);
        let mut f = ones(44100, 44100);
        fade_out(&mut f, 0, 44099);
        assert_eq!(f.ch[1][44099], 0.0);
        channel_to_mono(&mut f, 1);
        assert_eq!(f.channels(), 1);
    }
}
