// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Tools > Spectral > Vocal denoise / isolator... and Deverb...: voice restoration on the spectral tools' short-time
//! Fourier transform ([`crate::spectral::Stft`], frames of about 46 ms, a quarter-frame hop).
//!
//! The original's two entries were never opened (they ask to download the feature first, `plugins/edison/tools.md`);
//! its manual gives only what they are for. Everything here is **Lemon Studio's design**:
//!
//! - **Vocal denoise / isolator**: each time-frequency bin gets a voice likeness from 0 to 1, the product of
//!   - how centred it is (the channels' sum against their power, smoothed over three bins; a mono sample is all
//!     centre),
//!   - the voice band (full from 90 Hz to 9 kHz, falling off by octaves outside it),
//!   - how harmonic it is (the median over 17 frames against the median over 17 bins: sustained partials against
//!     clicks and hiss), and
//!   - how far it stands over the steady noise (each bin's 10th-percentile power over the range, tripled), which a
//!     harmonic bin is spared.
//!
//!   The likeness falls back over 60 ms rather than at once. The MIX knob goes from −100% (what is not voice: the
//!   voice taken out) through 0 (unchanged) to +100% (the voice alone, the default).
//! - **Deverb**: late-reverb suppression. The reverb time (RT60) is estimated from the range itself: the steep end of
//!   the slopes of its level over 150 ms windows that fall in a straight line (0.2 to 3 s; 0.8 s when none is found).
//!   Each bin's late reverb is its power (smoothed over three bins and three frames) 50 ms earlier, decayed over
//!   those 50 ms at that reverb time; the bin keeps what is above it (spectral subtraction, the late reverb counted
//!   twice), down to a floor. SOURCE: General (floor −18 dB) or Vocal (40 ms, counted three times, floor −24 dB).
//!   MIX: 0 to 100% (default 100%).
//! - Both hear a frame of audio on both sides of the range and change only the range, as the other spectral tools.
//!
// spec: plugins/tesla/README.md "Vocal denoise / isolator, Deverb and Undo mix in Tesla 1.8"

use crate::sample::Sample;
use crate::spectral::{frame_size, Stft};

/// Vocal denoise / isolator's setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Isolate {
    /// −1 (what is not voice) through 0 (unchanged) to 1 (the voice alone).
    pub mix: f32,
}

impl Default for Isolate {
    fn default() -> Self {
        Isolate { mix: 1.0 }
    }
}

/// Deverb's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deverb {
    /// 0 (unchanged) to 1 (all the reverb found taken out).
    pub mix: f32,
    /// SOURCE: Vocal rather than General.
    pub vocal: bool,
}

impl Default for Deverb {
    fn default() -> Self {
        Deverb { mix: 1.0, vocal: false }
    }
}

impl Deverb {
    pub fn source(&self) -> &'static str {
        if self.vocal { "Vocal" } else { "General" }
    }

    /// How far back the late reverb is read (s), the over-subtraction and the gain floor.
    fn params(&self) -> (f64, f32, f32) {
        if self.vocal { (0.04, 3.0, 10f32.powf(-24.0 / 20.0)) } else { (0.05, 2.0, 10f32.powf(-18.0 / 20.0)) }
    }
}

/// The reverb time when none can be estimated.
pub const RT60_FALLBACK: f64 = 0.8;

/// Runs `f` on all the channels of frames `a..=b` at once, with a frame of context on both sides (where the sample has
/// it), and writes back only `a..=b`.
fn in_context_all(s: &mut Sample, a: usize, b: usize, f: impl FnOnce(&[&[f32]], u32) -> Vec<Vec<f32>>) {
    let n = s.frames();
    if n == 0 || a > b || b >= n {
        return;
    }
    let ctx = frame_size(s.rate);
    let (lo, hi) = (a.saturating_sub(ctx), (b + 1 + ctx).min(n));
    let y = {
        let x: Vec<&[f32]> = s.ch.iter().map(|c| &c[lo..hi]).collect();
        f(&x, s.rate)
    };
    for (ch, v) in s.ch.iter_mut().zip(y) {
        ch[a..=b].copy_from_slice(&v[a - lo..b + 1 - lo]);
    }
}

/// The median of `v` (reordered).
fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let m = v.len() / 2;
    *v.select_nth_unstable_by(m, |a, b| a.total_cmp(b)).1
}

/// The gain falls back by this much per hop: 60 dB in 60 ms.
fn release(rate: u32, n: usize) -> f32 {
    let hop_s = (n / 4) as f32 / rate.max(1) as f32;
    10f32.powf(-3.0 * hop_s / 0.06)
}

/// The voice band's weight at `hz`: 1 from 90 Hz to 9 kHz, e^(−1.5 o²) at `o` octaves outside it.
pub fn band(hz: f64) -> f32 {
    if hz <= 0.0 {
        return 0.0;
    }
    let o = (90.0 / hz).log2().max((hz / 9000.0).log2()).max(0.0);
    (-1.5 * o * o).exp() as f32
}

/// Each bin's voice likeness (0 to 1), frame by frame, for the channels' transforms `st` at `rate`.
pub fn voice_mask(st: &[Stft], rate: u32) -> Vec<f32> {
    let Some(s0) = st.first() else { return Vec::new() };
    let (frames, bins, n) = (s0.frames, s0.bins(), s0.n);
    let cn = st.len() as f32;
    let size = frames * bins;
    // The mid's power, and the centred share of it: |ΣX|² against c·Σ|X|² (1 for equal channels, ½ on average for
    // unrelated ones, 0 for opposite ones).
    let (mut pw, mut num, mut den) = (vec![0f32; size], vec![0f32; size], vec![0f32; size]);
    for i in 0..size {
        let (mut sr, mut si, mut sum) = (0f32, 0f32, 0f32);
        for s in st {
            sr += s.re[i];
            si += s.im[i];
            sum += s.re[i] * s.re[i] + s.im[i] * s.im[i];
        }
        num[i] = sr * sr + si * si;
        den[i] = cn * sum;
        pw[i] = num[i] / (cn * cn);
    }
    let mag: Vec<f32> = pw.iter().map(|p| p.sqrt()).collect();
    // The steady noise: each bin's 10th-percentile power over the frames (smoothed over three), tripled.
    let smooth: Vec<f32> = (0..size)
        .map(|i| {
            let (t, k) = (i / bins, i % bins);
            let (a, b) = (t.saturating_sub(1), (t + 1).min(frames - 1));
            (a..=b).map(|u| pw[u * bins + k]).sum::<f32>() / (b + 1 - a) as f32
        })
        .collect();
    let steady: Vec<f32> = if frames >= 10 {
        (0..bins)
            .map(|k| {
                let mut v: Vec<f32> = (0..frames).map(|t| smooth[t * bins + k]).collect();
                let q = frames / 10;
                3.0 * *v.select_nth_unstable_by(q, |a, b| a.total_cmp(b)).1
            })
            .collect()
    } else {
        vec![0.0; bins]
    };
    const REACH: usize = 8;
    let mut buf = Vec::with_capacity(2 * REACH + 1);
    let fall = release(rate, n);
    let mut prev = vec![0f32; bins];
    let mut m = vec![0f32; size];
    for t in 0..frames {
        for k in 0..bins {
            let i = t * bins + k;
            let centre = if st.len() < 2 {
                1.0
            } else {
                let (a, b) = (k.saturating_sub(1), (k + 1).min(bins - 1));
                let (nu, de) = (a..=b).fold((0f32, 0f32), |(x, y), j| (x + num[t * bins + j], y + den[t * bins + j]));
                let r = if de > 1e-30 { nu / de } else { 1.0 };
                ((r - 0.5) * 2.0).clamp(0.0, 1.0).powi(2)
            };
            // Harmonic against percussive: medians along time and along frequency.
            buf.clear();
            buf.extend((t.saturating_sub(REACH)..=(t + REACH).min(frames - 1)).map(|u| mag[u * bins + k]));
            let h = median(&mut buf);
            buf.clear();
            buf.extend((k.saturating_sub(REACH)..=(k + REACH).min(bins - 1)).map(|j| mag[t * bins + j]));
            let p = median(&mut buf);
            let tonal = if h + p > 1e-15 { h * h / (h * h + p * p) } else { 0.0 };
            let over = if smooth[i] > 1e-30 { (1.0 - steady[k] / smooth[i]).max(0.0).sqrt() } else { 0.0 };
            let hz = k as f64 * rate as f64 / n as f64;
            let v = centre * band(hz) * (0.25 + 0.75 * tonal) * over.max(tonal * tonal);
            let v = v.max(prev[k] * fall).min(1.0);
            prev[k] = v;
            m[i] = v;
        }
    }
    m
}

/// Vocal denoise / isolator on the channels `ch` (each the same length) at `rate`.
pub fn isolate(ch: &[&[f32]], rate: u32, iso: &Isolate) -> Vec<Vec<f32>> {
    let a = iso.mix.clamp(-1.0, 1.0);
    if ch.is_empty() || a == 0.0 {
        return ch.iter().map(|c| c.to_vec()).collect();
    }
    let n = frame_size(rate);
    let mut st: Vec<Stft> = ch.iter().map(|x| Stft::analyze(x, n)).collect();
    let m = voice_mask(&st, rate);
    let bins = st[0].bins();
    for s in &mut st {
        for t in 0..s.frames {
            let (re, im) = s.frame(t);
            for k in 0..bins {
                let v = m[t * bins + k];
                // Towards the voice alone, the rest goes down; towards the rest, the voice does.
                let g = if a > 0.0 { 1.0 - a * (1.0 - v) } else { 1.0 + a * v };
                re[k] *= g;
                im[k] *= g;
            }
        }
    }
    st.iter().map(|s| s.synthesize()).collect()
}

/// Vocal denoise / isolator... over frames `a..=b`.
pub fn isolate_range(s: &mut Sample, a: usize, b: usize, iso: &Isolate) {
    in_context_all(s, a, b, |x, rate| isolate(x, rate, iso));
}

/// The reverb time (RT60, s) of `x` at `rate`, from the steep end of its straight level decays; `None` when fewer
/// than three are found.
pub fn rt60(x: &[f32], rate: u32) -> Option<f64> {
    // The level in dB over 10 ms blocks.
    let blk = ((rate as f64 * 0.01).round() as usize).max(1);
    let db: Vec<f64> = x
        .chunks(blk)
        .filter(|c| c.len() == blk)
        .map(|c| 10.0 * (c.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / blk as f64).max(1e-20).log10())
        .collect();
    const W: usize = 15;
    if db.len() < W + 2 {
        return None;
    }
    let mut sorted = db.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let floor = sorted[sorted.len() / 20];
    let top = sorted[sorted.len() - 1];
    let dt = blk as f64 / rate as f64;
    let xm = (W - 1) as f64 / 2.0;
    let sxx: f64 = (0..W).map(|i| (i as f64 - xm).powi(2)).sum();
    let mut slopes = Vec::new();
    for w in db.windows(W) {
        // Above the noise floor by 10 dB at the end, and not far under the loudest.
        if w[W - 1] < floor + 10.0 || w[0] < top - 50.0 {
            continue;
        }
        let ym = w.iter().sum::<f64>() / W as f64;
        let sxy: f64 = w.iter().enumerate().map(|(i, y)| (i as f64 - xm) * (y - ym)).sum();
        let slope = sxy / sxx;
        let syy: f64 = w.iter().map(|y| (y - ym).powi(2)).sum();
        if slope >= 0.0 || syy <= 0.0 {
            continue;
        }
        let r2 = sxy * sxy / (sxx * syy);
        // A straight fall of 3 dB or more.
        if r2 > 0.95 && -slope * (W - 1) as f64 >= 3.0 {
            slopes.push(slope / dt);
        }
    }
    if slopes.len() < 3 {
        return None;
    }
    // A sound cannot die away faster than the room lets it: the steep end (the 20th percentile) is the room's.
    slopes.sort_by(|a, b| a.total_cmp(b));
    let s = slopes[slopes.len() / 5];
    Some((-60.0 / s).clamp(0.2, 3.0))
}

/// Deverb one channel at `rate`, for a reverb time `rt60` (s).
pub fn deverb(x: &[f32], rate: u32, rt60: f64, d: &Deverb) -> Vec<f32> {
    let mix = d.mix.clamp(0.0, 1.0);
    if mix == 0.0 || x.is_empty() {
        return x.to_vec();
    }
    let n = frame_size(rate);
    let hop = n / 4;
    let mut s = Stft::analyze(x, n);
    let (frames, bins) = (s.frames, s.bins());
    let (late, over, floor) = d.params();
    let lag = ((late * rate as f64 / hop as f64).round() as usize).max(1);
    // The power decays by 60 dB over the reverb time: over the lag, by e^(−2δT) with δ = 3 ln 10 / RT60.
    let delta = 3.0 * std::f64::consts::LN_10 / rt60.max(0.05);
    let decay = (-2.0 * delta * (lag * hop) as f64 / rate as f64).exp() as f32;
    // Each bin's power, smoothed over the bin and its neighbours, and over the frame and its neighbours.
    let raw: Vec<f32> = s.re.iter().zip(&s.im).map(|(r, i)| r * r + i * i).collect();
    let pw: Vec<f32> = (0..frames * bins)
        .map(|i| {
            let (t, k) = (i / bins, i % bins);
            let (a, b) = (k.saturating_sub(1), (k + 1).min(bins - 1));
            let (u, v) = (t.saturating_sub(1), (t + 1).min(frames - 1));
            let sum: f32 = (u..=v).map(|f| raw[f * bins + a..=f * bins + b].iter().sum::<f32>()).sum();
            sum / ((b + 1 - a) * (v + 1 - u)) as f32
        })
        .collect();
    for t in 0..frames {
        let (re, im) = s.frame(t);
        for k in 0..bins {
            let p = pw[t * bins + k];
            let tail = if t >= lag { decay * pw[(t - lag) * bins + k] } else { 0.0 };
            let g2 = if p > 1e-30 { (1.0 - over * tail / p).max(floor * floor) } else { floor * floor };
            let g = 1.0 - mix + mix * g2.sqrt();
            re[k] *= g;
            im[k] *= g;
        }
    }
    s.synthesize()
}

/// The reverb time (s) of frames `a..=b`'s mid, with the frame of context on both sides the tools hear; `None` as
/// [`rt60`].
pub fn range_rt60(s: &Sample, a: usize, b: usize) -> Option<f64> {
    let n = s.frames();
    if n == 0 || a > b || b >= n || s.ch.is_empty() {
        return None;
    }
    let ctx = frame_size(s.rate);
    let (lo, hi) = (a.saturating_sub(ctx), (b + 1 + ctx).min(n));
    let k = s.ch.len() as f32;
    let mid: Vec<f32> = (lo..hi).map(|i| s.ch.iter().map(|c| c[i]).sum::<f32>() / k).collect();
    rt60(&mid, s.rate)
}

/// Deverb... over frames `a..=b`, at the range's reverb time ([`range_rt60`], else [`RT60_FALLBACK`]), which it
/// returns.
pub fn deverb_range(s: &mut Sample, a: usize, b: usize, d: &Deverb) -> f64 {
    let rt = range_rt60(s, a, b).unwrap_or(RT60_FALLBACK);
    in_context_all(s, a, b, |x, rate| x.iter().map(|c| deverb(c, rate, rt, d)).collect());
    rt
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Rng;

    fn energy(x: &[f32]) -> f64 {
        x.iter().map(|v| (*v as f64).powi(2)).sum()
    }

    fn db(a: f64, b: f64) -> f64 {
        10.0 * (a / b).log10()
    }

    /// A voice-like tone: 220 Hz with four overtones, 300 ms on and 100 ms off (10 ms ramps).
    fn voice(n: usize, rate: u32) -> Vec<f32> {
        let r = rate as f64;
        (0..n)
            .map(|i| {
                let t = i as f64 / r;
                let ph = t % 0.4;
                let gate = if ph >= 0.3 { 0.0 } else { (ph / 0.01).min(1.0).min((0.3 - ph) / 0.01) };
                let s: f64 = (1..=5).map(|h| (std::f64::consts::TAU * 220.0 * h as f64 * t).sin() / h as f64).sum();
                (0.2 * gate * s) as f32
            })
            .collect()
    }

    fn noise(n: usize, seed: u64, a: f32) -> Vec<f32> {
        let mut r = Rng::new(seed);
        (0..n).map(|_| r.gauss() as f32 * a).collect()
    }

    #[test]
    fn isolator_keeps_the_centred_voice_and_takes_it_out() {
        let rate = 16000;
        let n = 2 * rate as usize;
        let v = voice(n, rate);
        let (nl, nr) = (noise(n, 1, 0.05), noise(n, 2, 0.05));
        let l: Vec<f32> = v.iter().zip(&nl).map(|(a, b)| a + b).collect();
        let r: Vec<f32> = v.iter().zip(&nr).map(|(a, b)| a + b).collect();
        // The voice alone: what is left besides it is well under the noise that was there.
        let y = isolate(&[&l, &r], rate, &Isolate::default());
        let rest: Vec<f32> = y[0].iter().zip(&v).map(|(a, b)| a - b).collect();
        let cut = db(energy(&nl), energy(&rest));
        assert!(cut > 8.0, "noise down by {cut:.1} dB");
        assert!(db(energy(&v), energy(&y[0])).abs() < 3.0);
        // The voice taken out: what is left is mostly the noise.
        let y = isolate(&[&l, &r], rate, &Isolate { mix: -1.0 });
        let rest: Vec<f32> = y[1].iter().zip(&nr).map(|(a, b)| a - b).collect();
        let cut = db(energy(&v), energy(&rest));
        assert!(cut > 8.0, "voice down by {cut:.1} dB");
        // At 0, nothing changes.
        assert_eq!(isolate(&[&l, &r], rate, &Isolate { mix: 0.0 }), vec![l.clone(), r.clone()]);
        // Mono: steady hiss under the voice still goes down.
        let y = isolate(&[&l], rate, &Isolate::default());
        let rest: Vec<f32> = y[0].iter().zip(&v).map(|(a, b)| a - b).collect();
        let cut = db(energy(&nl), energy(&rest));
        assert!(cut > 6.0, "mono noise down by {cut:.1} dB");
        assert!(band(1000.0) == 1.0 && band(45.0) < 0.3 && band(0.0) == 0.0 && band(18000.0) < 0.3);
    }

    /// Clicks of noise every 600 ms through a room of `rt` s: (dry, wet).
    fn room(rate: u32, rt: f64) -> (Vec<f32>, Vec<f32>) {
        let n = 3 * rate as usize;
        let burst = (0.02 * rate as f64) as usize;
        let mut dry = vec![0f32; n];
        let mut r = Rng::new(7);
        for start in (rate as usize / 10..n - burst).step_by((0.6 * rate as f64) as usize) {
            for v in &mut dry[start..start + burst] {
                *v = r.gauss() as f32 * 0.3;
            }
        }
        // The impulse: the direct sound, then noise dying away by 60 dB over `rt`.
        let len = (rt * 1.2 * rate as f64) as usize;
        let delta = 3.0 * std::f64::consts::LN_10 / rt;
        let tail = |i: usize, g: f64| (g * 0.15 * (-delta * i as f64 / rate as f64).exp()) as f32;
        let ir: Vec<f32> = (0..len).map(|i| if i == 0 { 1.0 } else { tail(i, r.gauss()) }).collect();
        let mut wet = vec![0f32; n];
        for (i, &d) in dry.iter().enumerate() {
            if d != 0.0 {
                for (j, &h) in ir.iter().enumerate().take(n - i) {
                    wet[i + j] += d * h;
                }
            }
        }
        (dry, wet)
    }

    #[test]
    fn deverb_estimates_the_room_and_takes_its_tail_out() {
        let rate = 8000;
        let (dry, wet) = room(rate, 0.8);
        let est = rt60(&wet, rate).expect("a reverb time");
        assert!((0.5..1.3).contains(&est), "rt60 {est:.2} s");
        let y = deverb(&wet, rate, est, &Deverb::default());
        // The tails (100 to 500 ms after each click) go down far more than the clicks themselves.
        let (mut tw, mut ty, mut bw, mut by) = (0.0, 0.0, 0.0, 0.0);
        let burst = (0.02 * rate as f64) as usize;
        for start in (rate as usize / 10..dry.len() - burst).step_by((0.6 * rate as f64) as usize) {
            let (a, b) = (start + (0.1 * rate as f64) as usize, (start + (0.5 * rate as f64) as usize).min(dry.len()));
            tw += energy(&wet[a..b]);
            ty += energy(&y[a..b]);
            bw += energy(&wet[start..start + burst]);
            by += energy(&y[start..start + burst]);
        }
        let (tail, click) = (db(tw, ty), db(bw, by));
        assert!(tail > 8.0, "tail down by {tail:.1} dB");
        assert!(click < 3.0, "clicks down by {click:.1} dB");
        // Vocal goes further; no mix changes nothing.
        let yv = deverb(&wet, rate, est, &Deverb { vocal: true, ..Deverb::default() });
        assert!(energy(&yv) < energy(&y));
        assert_eq!(deverb(&wet, rate, est, &Deverb { mix: 0.0, vocal: false }), wet);
        // Without decays to read, the fallback.
        assert_eq!(rt60(&vec![0.1; 4000], rate), None);
    }

    #[test]
    fn ranges_change_only_the_range() {
        let rate = 8000;
        let (_, wet) = room(rate, 0.8);
        let mut s = Sample::from_channels(rate, vec![wet.clone(), wet.clone()], crate::sample::Format::Float32);
        let used = deverb_range(&mut s, 4000, 15999, &Deverb::default());
        assert!((0.2..=3.0).contains(&used));
        assert_eq!(&s.ch[0][..4000], &wet[..4000]);
        assert_eq!(&s.ch[1][16000..], &wet[16000..]);
        assert_ne!(&s.ch[0][4000..16000], &wet[4000..16000]);
        let mut s = Sample::from_channels(rate, vec![wet.clone()], crate::sample::Format::Float32);
        isolate_range(&mut s, 100, 199, &Isolate::default());
        assert_eq!(&s.ch[0][200..], &wet[200..]);
    }
}
