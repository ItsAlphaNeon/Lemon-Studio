// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The played sample: what is done once to the loaded sample before any voice plays it. Start and length, the loop
//! points, files below 8000 Hz, time stretching in the methods that stretch first, and the precomputed effects, all
//! in the file's units (`os_model::SampleFx`) and in the order and arithmetic of the spec.
//!
// spec: audio/precomputed-effects.md, audio/time-stretch.md ("Where it runs", "Lemon Studio"),
// spec: plugins/lemon-sampler.md (§5.4 - §5.6), audio/conventions.md (Round, single)

use std::f64::consts::PI;

use os_dsp::resample::{self, Quality};
use os_engine::Sample;
use os_model::SampleFx;

use crate::params::FxKey;

/// What the steps need to know of the file beyond its frames.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Source {
    /// A mono file of 16-bit integer PCM (the one place that shows: stereo delay).
    pub int16_mono: bool,
    /// The file's own loop (the first loop of a WAV `smpl` chunk): start and end frame, both inclusive.
    pub file_loop: Option<(u32, u32)>,
}

/// The sample a voice plays and its loop (start and end frame, both inclusive), whether or not a voice uses it.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    pub sample: Sample,
    pub looped: Option<(usize, usize)>,
    /// Frames of the played sample per frame of the file at the file's start: what a time stretch in a method that
    /// stretches first and the conversion of a file below 8000 Hz make of the time (audio clip trims are file time).
    pub per_file_frame: f64,
}

/// A sample side at a time (0 = left, 1 = right).
type Sides = [Vec<f32>; 2];

/// Builds the played sample (spec: audio/precomputed-effects.md "Order"). `out_rate` is the host's rate.
pub fn build(raw: &Sample, src: &Source, k: &FxKey) -> Built {
    let fx = &k.fx;
    let mut s: Sides = [raw.left.clone(), raw.right.clone()];
    let mut rate = f64::from(raw.rate.max(1));
    let out_rate = f64::from(k.rate.max(1));
    let n0 = s[0].len();
    let mut lp: Option<(usize, usize)> = src.file_loop.map(|(a, b)| (a as usize, b as usize));
    let mut per_file_frame = 1.0;
    // 1. Start and length.
    let (start, length) = (fx.start(), fx.length());
    if n0 > 0 && (start != 0.0 || length != 1.0) {
        lp = cut(&mut s, lp, start, length, rate);
    }
    // 2. The loop points are limited to the sample.
    lp = limit(lp, s[0].len());
    // 3. A file below 8000 Hz is converted to the output rate.
    if rate < 8000.0 && !s[0].is_empty() {
        let ratio = out_rate / rate;
        (s, lp) = convert(&s, lp, ratio);
        per_file_frame = ratio;
        rate = out_rate;
    }
    // (Lemon Studio) The methods that stretch the sample first do it here, where FL stretches (spec:
    // audio/time-stretch.md "Where it runs"); the loop points move with the frames as in the conversion above.
    if k.stretch && !s[0].is_empty() {
        let ratio = k.ratio_for(s[0].len() as f64 / rate);
        let shift = 2f64.powf(f64::from(k.shift) / 1200.0);
        if (ratio - 1.0).abs() > 1e-4 || (shift - 1.0).abs() > 1e-4 {
            let frames = (s[0].len() as f64 * ratio).round().max(1.0) as usize;
            let (l, r) = os_dsp::stretch::whole(&s[0], &s[1], rate as u32, frames, shift);
            let scale = frames as f64 / s[0].len() as f64;
            lp = lp.map(|(a, b)| ((a as f64 * scale).round_ties_even() as usize, ((b + 1) as f64 * scale).round_ties_even() as usize - 1));
            lp = limit(lp, frames);
            s = [l, r];
            per_file_frame *= scale;
        }
    }
    // (Lemon Studio) The loop knobs, off their defaults, give the loop in place of the file's: fractions of the sample
    // as it is here, ahead of the steps that move loop points.
    if let Some((ks, ke)) = k.knob_loop {
        let n = s[0].len() as f64;
        let a = (f64::from(ks.clamp(0.0, 1.0)) * n).floor() as usize;
        let e = (f64::from(ke.clamp(0.0, 1.0)) * n).floor() as usize;
        lp = (e >= a + 2).then(|| (a, e - 1));
    }
    // 4. A crossfade without a loop makes the whole sample the loop.
    if lp.is_none() && fx.crossfade > 0 && !s[0].is_empty() {
        lp = Some((0, s[0].len() - 1));
    }
    // 5. Ping-pong.
    if fx.ping_pong {
        if let Some((a, b)) = lp.filter(|&(a, b)| a + 1 < b) {
            for side in s.iter_mut() {
                let mut v = Vec::with_capacity(side.len() + b - a - 1);
                v.extend_from_slice(&side[..=b]);
                v.extend(side[a + 1..b].iter().rev());
                v.extend_from_slice(&side[b + 1..]);
                *side = v;
            }
            lp = Some((a, b + (b - a - 1)));
        }
    }
    // 6. The effects, only on more than three frames.
    if s[0].len() > 3 {
        // rs = 44100 / (file rate * (1 / output rate) * output rate), left to right.
        let rs = 44100.0 / (rate * (1.0 / out_rate) * out_rate);
        effects(&mut s, &mut lp, fx, src, rs);
    }
    let [left, right] = s;
    Built { sample: Sample { rate: rate as u32, left, right }, looped: lp, per_file_frame }
}

/// **(Lemon Studio)** When the played sample is the file's own frames as they are (no step of [`build`] changes them;
/// a file of `frames` frames at `rate` Hz): its loop, as `build` makes it. `None` when a step changes the frames (Keep
/// on disk then plays the sample from memory). Erring towards `None` is safe: it only keeps a sample in memory.
pub fn as_is(src: &Source, k: &FxKey, rate: u32, frames: usize) -> Option<Option<(usize, usize)>> {
    let fx = &k.fx;
    if frames == 0 || fx.start() != 0.0 || fx.length() != 1.0 || rate < 8000 {
        return None;
    }
    if k.stretch {
        let ratio = k.ratio_for(frames as f64 / f64::from(rate));
        let shift = 2f64.powf(f64::from(k.shift) / 1200.0);
        if (ratio - 1.0).abs() > 1e-4 || (shift - 1.0).abs() > 1e-4 {
            return None;
        }
    }
    let changes = fx.stereo_delay != 2048
        || fx.flags & (SampleFx::REVERSE | SampleFx::CLIP | SampleFx::FADE_STEREO | SampleFx::SWAP) != 0
        || fx.pogo != 256
        || fx.ring_mix > 0
        || fx.cutoff < 1024
        || fx.resonance > 0
        || fx.fade_in > 0
        || fx.fade_out > 0
        || fx.boost > 0
        || fx.feedback != 128
        || fx.remove_dc
        || fx.polarity
        || fx.normalize
        || fx.crossfade > 0
        || fx.reverb > 0
        || fx.trim > 0;
    if changes {
        return None;
    }
    let mut lp = limit(src.file_loop.map(|(a, b)| (a as usize, b as usize)), frames);
    if let Some((ks, ke)) = k.knob_loop {
        let n = frames as f64;
        let a = (f64::from(ks.clamp(0.0, 1.0)) * n).floor() as usize;
        let e = (f64::from(ke.clamp(0.0, 1.0)) * n).floor() as usize;
        lp = (e >= a + 2).then(|| (a, e - 1));
    }
    if fx.ping_pong && lp.is_some_and(|(a, b)| a + 1 < b) {
        return None;
    }
    Some(lp)
}

/// `end = min(end, N - 1)`, then `start = min(start, end)`; no loop in an empty sample.
fn limit(lp: Option<(usize, usize)>, n: usize) -> Option<(usize, usize)> {
    let (a, b) = lp?;
    if n == 0 {
        return None;
    }
    let b = b.min(n - 1);
    Some((a.min(b), b))
}

/// Round: to nearest, halves to even (spec: audio/conventions.md).
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

// ---------------------------------------------------------------------------------------------------------------
// 1. Start and length

/// Keeps the frames the start and length select, declicks the new edges, and moves the loop with the frames.
fn cut(s: &mut Sides, lp: Option<(usize, usize)>, start: f64, length: f64, rate: f64) -> Option<(usize, usize)> {
    let n = s[0].len() as i64;
    let m = (n - 1) as f64;
    let mut i0 = round(start * m);
    let mut i1 = i0 + round(length * (1.0 - start) * m);
    if i1 - i0 < 41 {
        if i0 < n - 42 {
            i1 = (i1 + 41).min(n - 1);
        } else {
            i0 -= 41;
        }
    }
    // (Lemon Studio) Out-of-range values stay inside the sample.
    let i0 = i0.clamp(0, n - 1);
    let i1 = i1.clamp(i0, n - 1);
    let (a, b) = (i0 as usize, i1 as usize);
    for side in s.iter_mut() {
        *side = side[a..=b].to_vec();
    }
    // A loop whose start is cut away is gone; otherwise it moves with the frames, its end at most the last kept frame.
    let lp = lp.and_then(|(ls, le)| (ls >= a && ls <= b).then(|| (ls - a, le.min(b) - a)));
    if start > 0.0 {
        let l0 = round(rate * f64::from(0.005f32) * f64::from(0.1f32)).max(0) as usize;
        for side in s.iter_mut() {
            declick_edge(side, l0, rate, true);
        }
    }
    if length < 1.0 {
        let l0 = round(rate * f64::from(0.04f32) * f64::from(0.1f32)).max(0) as usize;
        for side in s.iter_mut() {
            declick_edge(side, l0, rate, false);
        }
    }
    lp
}

/// The declick at a cut edge (the beginning when `front`): a one-pole high-pass whose coefficient grows towards the
/// edge, with a cosine fade over the last millisecond, over up to `l0` frames from the first non-zero one inwards.
fn declick_edge(x: &mut [f32], l0: usize, rate: f64, front: bool) {
    let n = x.len();
    if n == 0 || l0 == 0 {
        return;
    }
    // The first non-zero frame from the edge (a side that is zero throughout: the edge itself), and the frames from it
    // to the other end of the sample.
    let (e, avail) = if front {
        let e = x.iter().position(|&v| v != 0.0).unwrap_or(0);
        (e, n - e)
    } else {
        let e = x.iter().rposition(|&v| v != 0.0).unwrap_or(n - 1);
        (e, e + 1)
    };
    let l = l0.min(avail);
    if l == 0 {
        return;
    }
    let m = l.min(round(rate * 0.001).max(0) as usize);
    let w = (22050f64.ln() / (2f64.ln() * l as f64)) * 2f64.ln();
    let mut lp = 0.0f64;
    // i = 1..L, from the innermost frame to the edge.
    for i in 1..=l {
        let at = if front { e + l - i } else { e + i - l };
        let c = ((2.0 / rate) * (i as f64 * w).exp()).min(1.0);
        let xv = f64::from(x[at]);
        lp += c * (xv - lp);
        let g = if i <= l - m { 1.0 } else { (((i - (l - m)) as f64 * (1.0 / (m + 1) as f64) * PI).cos() + 1.0) * 0.5 };
        x[at] = ((xv - lp) * g) as f32;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// 3. Files below 8000 Hz

/// Frames either side of the conversion's sinc.
const CONVERT_N: usize = 64;

/// Converts the sample to `ratio` times its rate with the sampler's resampler at 64 frames either side. A frame past
/// the last reads the last frame, or, when the loop ends on the last frame, the loop from its start.
fn convert(s: &Sides, lp: Option<(usize, usize)>, ratio: f64) -> (Sides, Option<(usize, usize)>) {
    let n = s[0].len();
    let frames = round(n as f64 * ratio).max(0) as usize;
    let r = 1.0 / ratio;
    let wrap = lp.filter(|&(_, b)| b == n - 1);
    let at = |j: i64| -> usize {
        if j < 0 {
            0
        } else if (j as usize) < n {
            j as usize
        } else if let Some((a, b)) = wrap {
            a + (j as usize - (b + 1)) % (b - a + 1)
        } else {
            n - 1
        }
    };
    let mut w = [0.0f32; 2 * CONVERT_N];
    let mut out: Sides = [vec![0.0; frames], vec![0.0; frames]];
    for k in 0..frames {
        // (Lemon Studio) The read position as a product; FL sums it call by call (audio/sampler.md).
        let pos = k as f64 * r;
        let k0 = pos.floor();
        resample::weights(pos - k0, r, CONVERT_N, Quality::Exact, &mut w);
        let first = k0 as i64 - CONVERT_N as i64 + 1;
        for (c, side) in s.iter().enumerate() {
            let mut acc = 0.0f32;
            for (t, &wt) in w.iter().enumerate() {
                acc += side[at(first + t as i64)] * wt;
            }
            out[c][k] = acc;
        }
    }
    let lp = lp.map(|(a, b)| (round(a as f64 * ratio).max(0) as usize, (round((b + 1) as f64 * ratio) - 1).max(0) as usize));
    (out, limit(lp, frames))
}

// ---------------------------------------------------------------------------------------------------------------
// 6. The effects

fn effects(s: &mut Sides, lp: &mut Option<(usize, usize)>, fx: &SampleFx, src: &Source, rs: f64) {
    // Stereo delay.
    if fx.stereo_delay != 2048 {
        if src.int16_mono {
            // A mono 16-bit file's values change once, before they are doubled.
            for side in s.iter_mut() {
                for x in side.iter_mut() {
                    let v = x.clamp(-1.0, 1.0);
                    *x = ((v * 32767.0).round_ties_even()) / 32768.0;
                }
            }
        }
        let d = round(f64::from((i32::from(fx.stereo_delay) - 2048).abs()) / rs).max(0) as usize;
        // Below 2048 the right side starts late, above it the left.
        let late = if fx.stereo_delay > 2048 { 0 } else { 1 };
        for (c, side) in s.iter_mut().enumerate() {
            if c == late {
                let mut v = vec![0.0f32; d];
                v.extend_from_slice(side);
                *side = v;
            } else {
                side.resize(side.len() + d, 0.0);
            }
        }
    }
    let n = s[0].len();
    let m = n - 1;
    // Reverse.
    if fx.flags & SampleFx::REVERSE != 0 {
        s.iter_mut().for_each(|x| x.reverse());
        if fx.loop_follows_reverse {
            *lp = lp.map(|(a, b)| (m - b, m - a));
        }
    }
    // The pass over each side.
    let pogo = fx.pogo != 256;
    let ring = fx.ring_mix > 0;
    let lowpass = fx.cutoff < 1024 || fx.resonance > 0;
    let (fin, fout) = (fx.fade_in > 0, fx.fade_out > 0);
    let boost = fx.boost > 0;
    let clip = fx.flags & SampleFx::CLIP != 0;
    if pogo || ring || lowpass || fin || fout || boost || clip {
        let tab = if ring { ring_table() } else { Vec::new() };
        let mf = m as f64;
        let pogo_b = (f64::from((i32::from(fx.pogo) - 256).abs()) / 256.0) as f32;
        let mix = f64::from(fx.ring_mix) / 256.0;
        let inc = (f64::from(fx.ring_freq) + 8.0) * 16384.0 * rs / 5760.0;
        // E(z) = Exp(double(single(z * single(ln 601)))) - 1.
        let ln601 = 601f64.ln() as f32;
        let e = |z: f64| f64::from((z * f64::from(ln601)) as f32).exp() - 1.0;
        let inv600 = f64::from((1.0 / 600.0f64) as f32);
        let res = f64::from(fx.resonance.min(640));
        let lp_a = f64::from((f64::from((e(f64::from(fx.cutoff) / 1024.0) * inv600) as f32) * rs) as f32);
        let lp_b = f64::from(1.0f32 - (e((1024.0 - res) / 1024.0) * inv600) as f32);
        let q = (1.0 / mf) / 256.0;
        let boost_g = (f64::from(fx.boost) / 16.0 + 1.0) as f32;
        for (c, side) in s.iter_mut().enumerate() {
            let t: Vec<f32> = if pogo { side.clone() } else { Vec::new() };
            let mut phase = c as f64 * 4096.0;
            let (mut v, mut o) = (0.0f64, 0.0f64);
            for k in 0..n {
                let mut x = side[k];
                if pogo {
                    x = pogo_read(&t, k, mf, fx.pogo, pogo_b);
                }
                if ring {
                    let i = (phase as f32) as usize;
                    let tv = tab[if i >= 16384 { 0 } else { i }];
                    let mv = (f64::from(tv * x) * mix) as f32;
                    phase += inc;
                    while phase >= 16384.0 {
                        phase -= 16384.0;
                    }
                    x = (f64::from(mv) + f64::from(x) * (1.0 - mix)) as f32;
                }
                if lowpass {
                    v += (f64::from(x) - o) * lp_a;
                    o += v;
                    v *= lp_b;
                    x = o as f32;
                }
                if fin {
                    let g = (1.0 - q * (mf - k as f64) * f64::from(fx.fade_in)) as f32;
                    x = if g <= 0.0 { 0.0 } else { x * g };
                }
                if fout {
                    let g = (1.0 - k as f64 * q * f64::from(fx.fade_out)) as f32;
                    x = if g <= 0.0 { 0.0 } else { x * g };
                }
                if boost {
                    x *= boost_g;
                }
                if clip {
                    x = x.clamp(-1.0, 1.0);
                }
                side[k] = x;
            }
        }
    }
    // The one-pole feedback.
    if fx.feedback != 128 {
        let c = ((f64::from(fx.feedback) * 192.0 / 256.0).floor() - 96.0) as f32 * (1.0f64 / 128.0) as f32;
        for side in s.iter_mut() {
            let mut y = 0.0f32;
            for x in side.iter_mut() {
                y = *x + y * c;
                *x = y;
            }
        }
    }
    // Fade stereo.
    if fx.flags & SampleFx::FADE_STEREO != 0 {
        let w = (PI / 2.0) / (n - 1) as f64;
        for k in 0..n {
            s[0][k] = (f64::from(s[0][k]) * (k as f64 * w).cos()) as f32;
            s[1][k] = (f64::from(s[1][k]) * ((n - 1 - k) as f64 * w).cos()) as f32;
        }
    }
    // Remove DC.
    if fx.remove_dc {
        for side in s.iter_mut() {
            let mut sum = 0.0f64;
            for &x in side.iter() {
                sum += f64::from(x);
            }
            let mean = sum / n as f64;
            side.iter_mut().for_each(|x| *x = (f64::from(*x) - mean) as f32);
        }
    }
    // Reverse polarity.
    if fx.polarity {
        s.iter_mut().flatten().for_each(|x| *x = -*x);
    }
    // Normalize.
    if fx.normalize {
        let peak = s.iter().flatten().fold(0.0f32, |p, x| p.max(x.abs()));
        if peak > 0.0 {
            let g = 1.0 / peak;
            s.iter_mut().flatten().for_each(|x| *x *= g);
        }
    }
    // Crossfade.
    if fx.crossfade > 0 {
        if let Some((a, b)) = *lp {
            let l = ((b - a + 1) as u64 * u64::from(fx.crossfade) / 2048) as usize;
            if l > 0 && b + 1 >= l {
                let inv = 1.0 / l as f64;
                for side in s.iter_mut() {
                    for k in 0..l {
                        let t = k as f64 * inv;
                        let (d, from) = (b + 1 - l + k, a + k);
                        if from < side.len() {
                            side[d] = (f64::from(side[d]) * (1.0 - t) + f64::from(side[from]) * t) as f32;
                        }
                    }
                }
                *lp = Some(((a + l).min(b), b));
            }
        }
    }
    // Swap stereo.
    if fx.flags & SampleFx::SWAP != 0 {
        s.swap(0, 1);
    }
    // Reverb.
    if fx.reverb > 0 {
        reverb(s, *lp, fx.reverb, fx.reverb_type);
    }
    // Trim.
    if fx.trim > 0 {
        trim(s, lp, fx.trim);
    }
}

/// Pogo: frame `k` read with a cubic at a warped position from `t`, the side as it was before the pass.
fn pogo_read(t: &[f32], k: usize, m: f64, v: u16, b: f32) -> f32 {
    let kf = k as f64;
    let p = if v < 256 {
        let u = kf / m - 1.0;
        m + u.atan2((1.0 - u * u).sqrt()) * m * (2.0 / PI)
    } else {
        (kf / m * (PI / 2.0)).sin() * m
    };
    let p = f64::from(k as f32 * (1.0 - b)) + p * f64::from(b);
    let i = (p as f32).trunc() as i64;
    let f = (p - i as f64) as f32;
    let at = |j: i64| if j < 0 || j as usize >= t.len() { 0.0 } else { t[j as usize] };
    let (ym, y0, y1, y2) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let a = (f64::from((y0 - y1) * 3.0 - ym + y2) * 0.5) as f32;
    let bb = (f64::from(y1 * 2.0 + ym) - f64::from(y0 * 5.0 + y2) * 0.5) as f32;
    let c = (f64::from(y1 - ym) * 0.5) as f32;
    y0 + ((a * f + bb) * f + c) * f
}

/// The ring modulator's sine table.
fn ring_table() -> Vec<f32> {
    (0..16384).map(|i| (2.0 * i as f64 * PI / 16384.0).sin() as f32).collect()
}

/// A delay line of nominal length `d`: a ring of `max(1, d - 1)` values.
struct Line {
    buf: Vec<f32>,
    at: usize,
}

impl Line {
    fn new(d: usize) -> Line {
        Line { buf: vec![0.0; d.saturating_sub(1).max(1)], at: 0 }
    }

    /// What the next push takes out.
    fn out(&self) -> f32 {
        self.buf[self.at]
    }

    fn push(&mut self, v: f32) {
        self.buf[self.at] = v;
        self.at = (self.at + 1) % self.buf.len();
    }

    fn comb(&mut self, input: f32, fb: f32) -> f32 {
        let t = (f64::from(input) + f64::from(fb) * f64::from(self.out())) as f32;
        self.push(t);
        t
    }

    fn allpass(&mut self, input: f32) -> f32 {
        let d = self.out();
        let u = input + d * G7;
        self.push(u);
        d - u * G7
    }
}

const G7: f32 = 0.7;
const G3: f32 = 0.3;

/// The reverb: the sample becomes stereo, 22050 zero frames are appended, the wet signal is mixed in, and the quiet
/// end is dropped again (not below the loop's end frame number, nor below 1 frame).
fn reverb(s: &mut Sides, lp: Option<(usize, usize)>, amount: u16, kind: u16) {
    let n = s[0].len();
    for side in s.iter_mut() {
        side.resize(n + 22050, 0.0);
    }
    let time = f64::from(amount) / 256.0;
    let fb = |d: usize| {
        let e = (-3.0 * d as f64 / (time * 44100.0)) as f32;
        (f64::from(e) * 10f64.ln()).exp() as f32
    };
    for (c, side) in s.iter_mut().enumerate() {
        if kind == 0 {
            const COMBS: [usize; 4] = [1777, 1847, 1993, 2137];
            let mut combs: Vec<(Line, f32)> = COMBS.iter().map(|&d| (Line::new(d), fb(d))).collect();
            let mut aps: Vec<Line> = [389, 127, 43].iter().map(|&d| Line::new(d)).collect();
            let (mut h1, mut h2) = (0.0f32, 0.0f32);
            for x in side.iter_mut() {
                let input = *x;
                let mut w = input;
                for a in aps.iter_mut() {
                    w = a.allpass(w);
                }
                let mut h0 = 0.0f32;
                for (line, g) in combs.iter_mut() {
                    h0 += line.comb(w, *g);
                }
                let y = (f64::from(h1) * 0.5 + f64::from(h0 + h2) * 0.25) as f32;
                (h2, h1) = (h1, h0);
                *x = y * G3 + input * G7;
            }
        } else {
            const COMBS: [usize; 6] = [2467, 2767, 3217, 3533, 3877, 4127];
            let mut combs: Vec<(Line, f32)> = COMBS.iter().map(|&d| (Line::new(d), fb(d))).collect();
            let mut aps: Vec<Line> = [599, 197, 67].iter().map(|&d| Line::new(d)).collect();
            let mut ap101 = Line::new(101);
            let mut last = Line::new(if c == 0 { 97 } else { 79 });
            let mut low = 0.0f32;
            for x in side.iter_mut() {
                let input = *x;
                let mut sum = 0.0f32;
                for (line, g) in combs.iter_mut() {
                    sum += line.comb(input, *g);
                }
                let mut a = sum;
                for ap in aps.iter_mut() {
                    a = ap.allpass(a);
                }
                low = (f64::from(low) * f64::from(G7) + f64::from(a) * f64::from(G3)) as f32;
                let wet = last.allpass(ap101.allpass(low)) * G3;
                *x = wet * G3 + input * G7;
            }
        }
    }
    let floor = lp.map_or(1, |(_, b)| b.max(1));
    let quiet = 1.0 / 512.0;
    let mut len = s[0].len();
    while len > floor && s[0][len - 1].abs() < quiet && s[1][len - 1].abs() < quiet {
        len -= 1;
    }
    s.iter_mut().for_each(|x| x.truncate(len));
}

/// Trim: quiet frames dropped at both ends, not past the loop; the loop points keep their frame numbers and are then
/// limited to the sample.
fn trim(s: &mut Sides, lp: &mut Option<(usize, usize)>, v: u32) {
    let n = s[0].len();
    if n == 0 {
        return;
    }
    let thr = (f64::from(v) / 32768.0) as f32;
    let level = |k: usize| s[0][k].abs().max(s[1][k].abs());
    let front_limit = lp.map_or(n - 1, |(a, _)| a.min(n - 1));
    let mut a = 0;
    while a < front_limit && level(a) < thr {
        a += 1;
    }
    let back_limit = lp.map_or(a, |(_, b)| b.min(n - 1).max(a));
    let mut b = n - 1;
    while b > back_limit && level(b) < thr {
        b -= 1;
    }
    for side in s.iter_mut() {
        side.truncate(b + 1);
        side.drain(..a);
    }
    *lp = limit(*lp, s[0].len());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(fx: SampleFx) -> FxKey {
        FxKey { fx, ..FxKey::neutral() }
    }

    /// Decaying noise with a DC offset on each side (the kind of file the spec's cases use).
    fn noise(n: usize, rate: u32) -> Sample {
        let mut x: u32 = 0x2545_f491;
        let mut v = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let decay = |i: usize| (-(i as f32) / n as f32 * 3.0).exp();
        let left: Vec<f32> = (0..n).map(|i| v() * 0.5 * decay(i) + 0.05).collect();
        let right: Vec<f32> = (0..n).map(|i| v() * 0.5 * decay(i) - 0.03).collect();
        Sample { rate, left, right }
    }

    fn plain(s: &Sample) -> Built {
        build(s, &Source::default(), &key(SampleFx::default()))
    }

    // Keep on disk streams a file only when `build` would play its frames as they are, with the loop `as_is` gives.
    #[test]
    fn as_is_names_exactly_the_files_build_leaves_alone() {
        let s = noise(2000, 44100);
        let src = Source { int16_mono: false, file_loop: Some((300, 1500)) };
        let with = |f: &dyn Fn(&mut SampleFx)| {
            let mut fx = SampleFx::default();
            f(&mut fx);
            key(fx)
        };
        let keys = [
            key(SampleFx::default()),
            FxKey { knob_loop: Some((0.25, 0.75)), ..key(SampleFx::default()) },
            FxKey { knob_loop: Some((0.5, 0.5)), ..key(SampleFx::default()) },
            FxKey { stretch: true, ..key(SampleFx::default()) },
            FxKey { stretch: true, ratio: 1.5, ..key(SampleFx::default()) },
            FxKey { stretch: true, shift: 100.0, ..key(SampleFx::default()) },
            with(&|f| f.flags |= SampleFx::REVERSE),
            with(&|f| f.flags |= SampleFx::SWAP),
            with(&|f| f.normalize = true),
            with(&|f| f.crossfade = 100),
            with(&|f| f.ping_pong = true),
            with(&|f| f.stereo_delay = 2100),
            with(&|f| f.fade_in = 10),
            with(&|f| f.reverb = 40),
            with(&|f| f.trim = 5),
            with(&|f| f.start_bits = 0.25f64.to_bits()),
        ];
        let mut kept = 0;
        for k in &keys {
            if let Some(lp) = as_is(&src, k, s.rate, s.frames()) {
                kept += 1;
                let b = build(&s, &src, k);
                assert_eq!((&b.sample, b.looped, b.per_file_frame), (&s, lp, 1.0), "{k:?}");
            }
        }
        assert_eq!(kept, 4, "the neutral key, the loop knobs and a stretch that does nothing");
        // A file below 8000 Hz is converted.
        assert_eq!(as_is(&src, &keys[0], 6000, 2000), None);
    }

    #[test]
    fn no_settings_plays_the_file_as_it_is() {
        let s = noise(1000, 44100);
        let b = plain(&s);
        assert_eq!(b.sample, s);
        assert_eq!((b.looped, b.per_file_frame), (None, 1.0));
    }

    // spec "Start and length": the kept frames and the 41-frame minimum; the declick reaches the edge.
    #[test]
    fn start_and_length_keep_the_frames_they_select() {
        let s = noise(17640, 44100);
        let fx = SampleFx { start_bits: 0.25f64.to_bits(), length_bits: 0.5f64.to_bits(), ..SampleFx::default() };
        let b = build(&s, &Source::default(), &key(fx));
        // i0 = Round(0.25 * 17639) = 4410, i1 = 4410 + Round(0.5 * 0.75 * 17639) = 4410 + 6615.
        assert_eq!(b.sample.frames(), 6616);
        // Past the declicks the frames are the file's.
        assert_eq!(b.sample.left[100..6000], s.left[4510..10410]);
        // The beginning's declick (22 frames at 44100 Hz) ends on a quiet edge.
        assert!(b.sample.left[0].abs() < s.left[4410].abs().max(1e-3));
        // Fewer than 42 frames selected: 41 more are kept.
        let fx = SampleFx { start_bits: 0.5f64.to_bits(), length_bits: 0.0001f64.to_bits(), ..SampleFx::default() };
        let b = build(&s, &Source::default(), &key(fx));
        assert_eq!(b.sample.frames(), 1 + 1 + 41);
        // A loop moves with the frames; one whose start is cut away is gone.
        let src = Source { file_loop: Some((5000, 9000)), ..Source::default() };
        let fx = SampleFx { start_bits: 0.25f64.to_bits(), length_bits: 0.5f64.to_bits(), ..SampleFx::default() };
        assert_eq!(build(&s, &src, &key(fx)).looped, Some((590, 4590)));
        let fx = SampleFx { start_bits: 0.5f64.to_bits(), ..SampleFx::default() };
        assert_eq!(build(&s, &src, &key(fx)).looped, None);
    }

    // spec "The pass over each side": fades, boost and clip in the file's units.
    #[test]
    fn fades_boost_and_clip_follow_the_formulas() {
        let s = Sample { rate: 44100, left: vec![0.5; 1025], right: vec![0.5; 1025] };
        // Fade in 1024: gain 1 - (1 - k/M) * 4, silent over the first three quarters.
        let b = build(&s, &Source::default(), &key(SampleFx { fade_in: 1024, ..SampleFx::default() }));
        assert_eq!(b.sample.left[700], 0.0);
        assert!((b.sample.left[1024] - 0.5).abs() < 1e-7);
        assert!((b.sample.left[896] - 0.25).abs() < 1e-6, "{}", b.sample.left[896]);
        // Fade out 256: reaches silence at the far end.
        let b = build(&s, &Source::default(), &key(SampleFx { fade_out: 256, ..SampleFx::default() }));
        assert!((b.sample.left[512] - 0.25).abs() < 1e-6 && b.sample.left[1024] == 0.0);
        // Boost 256 = x17, clipped to 1 when the flag is on; normalize afterwards brings the peak to 1.
        let b = build(&s, &Source::default(), &key(SampleFx { boost: 256, ..SampleFx::default() }));
        assert_eq!(b.sample.left[10], 8.5);
        let b = build(&s, &Source::default(), &key(SampleFx { boost: 256, flags: SampleFx::CLIP, ..SampleFx::default() }));
        assert_eq!(b.sample.left[10], 1.0);
    }

    // spec "One-pole feedback": c = -0.75 at 0, +0.75 at 256: DC gains 0.571 and 4.
    #[test]
    fn the_feedback_tilts_the_sample() {
        let s = Sample { rate: 44100, left: vec![0.1; 2000], right: vec![0.1; 2000] };
        let up = build(&s, &Source::default(), &key(SampleFx { feedback: 256, ..SampleFx::default() }));
        assert!((up.sample.left[1999] - 0.4).abs() < 1e-5, "{}", up.sample.left[1999]);
        let down = build(&s, &Source::default(), &key(SampleFx { feedback: 0, ..SampleFx::default() }));
        assert!((down.sample.left[1999] - 0.1 / 1.75).abs() < 1e-5);
    }

    // spec "Stereo delay", "Reverse", "Swap stereo", "Fade stereo": frames and sides.
    #[test]
    fn stereo_delay_reverse_and_swap() {
        let s = noise(1000, 44100);
        // 3072: the left side is 1024 frames late; the right ends with 1024 zeros.
        let b = build(&s, &Source::default(), &key(SampleFx { stereo_delay: 3072, ..SampleFx::default() }));
        assert_eq!(b.sample.frames(), 2024);
        assert!(b.sample.left[..1024].iter().all(|v| *v == 0.0) && b.sample.left[1024..] == s.left[..]);
        assert!(b.sample.right[..1000] == s.right[..] && b.sample.right[1000..].iter().all(|v| *v == 0.0));
        // Below the centre the right side is late.
        let b = build(&s, &Source::default(), &key(SampleFx { stereo_delay: 2048 - 100, ..SampleFx::default() }));
        assert!(b.sample.right[..100].iter().all(|v| *v == 0.0) && b.sample.right[100..] == s.right[..]);
        // A mono 16-bit file: Round(single(x * 32767)) / 32768, so positive values above 16384 come out one step lower
        // (16384 itself gives 16383.5, which rounds to the even 16384).
        let v = [16385.0 / 32768.0, 0.5, -0.5, 0.25];
        let m = Sample { rate: 44100, left: v.to_vec(), right: v.to_vec() };
        let src = Source { int16_mono: true, ..Source::default() };
        let b = build(&m, &src, &key(SampleFx { stereo_delay: 2049, ..SampleFx::default() }));
        // Above 2048 the left side is the late one; both sides carry the changed values.
        let want = [16384.0 / 32768.0, 0.5, -0.5, 0.25];
        assert_eq!((&b.sample.left[1..], &b.sample.right[..4]), (&want[..], &want[..]));
        // Not for a stereo file.
        let b = build(&m, &Source::default(), &key(SampleFx { stereo_delay: 2049, ..SampleFx::default() }));
        assert_eq!((b.sample.left[1], b.sample.right[0]), (v[0], v[0]));
        // Reverse with and without the loop following it; swap.
        let src = Source { file_loop: Some((100, 199)), ..Source::default() };
        let rev = SampleFx { flags: SampleFx::REVERSE, ..SampleFx::default() };
        let b = build(&s, &src, &key(rev));
        assert_eq!(b.sample.left[0], s.left[999]);
        assert_eq!(b.looped, Some((100, 199)));
        let b = build(&s, &src, &key(SampleFx { loop_follows_reverse: true, ..rev }));
        assert_eq!(b.looped, Some((800, 899)));
        let b = build(&s, &src, &key(SampleFx { flags: SampleFx::SWAP, ..SampleFx::default() }));
        assert_eq!((b.sample.left[5], b.sample.right[5]), (s.right[5], s.left[5]));
        let b = build(&s, &src, &key(SampleFx { flags: SampleFx::FADE_STEREO, ..SampleFx::default() }));
        assert_eq!((b.sample.left[0], b.sample.right[999]), (s.left[0], s.right[999]));
        assert!(b.sample.left[999].abs() < 1e-6 && b.sample.right[0].abs() < 1e-6);
    }

    // spec "Ping-pong", "Crossfade": the unfolded loop; the crossfade makes a loop of a file without one.
    #[test]
    fn ping_pong_and_crossfade_change_the_loop() {
        let s = Sample { rate: 44100, left: (0..20).map(|i| i as f32).collect(), right: (0..20).map(|i| i as f32).collect() };
        let src = Source { file_loop: Some((5, 9)), ..Source::default() };
        let b = build(&s, &src, &key(SampleFx { ping_pong: true, ..SampleFx::default() }));
        assert_eq!(&b.sample.left[..16], &[0., 1., 2., 3., 4., 5., 6., 7., 8., 9., 8., 7., 6., 10., 11., 12.]);
        assert_eq!(b.looped, Some((5, 12)));
        // Crossfade 1024 on a file without a loop: half the sample fades into its beginning, the loop starts there.
        let s = Sample { rate: 44100, left: vec![1.0; 100], right: vec![1.0; 100] };
        let mut s2 = s.clone();
        s2.left[..50].iter_mut().for_each(|v| *v = 0.0);
        let b = build(&s2, &Source::default(), &key(SampleFx { crossfade: 1024, ..SampleFx::default() }));
        assert_eq!(b.looped, Some((50, 99)));
        assert_eq!(b.sample.left[50], 1.0);
        assert!((b.sample.left[75] - 0.5).abs() < 1e-6, "{}", b.sample.left[75]);
    }

    // spec "Trim": quiet ends dropped, the loop points keep their numbers.
    #[test]
    fn trim_drops_the_quiet_ends() {
        let mut s = Sample { rate: 44100, left: vec![0.0; 300], right: vec![0.0; 300] };
        s.left[100..200].iter_mut().for_each(|v| *v = 0.5);
        let b = build(&s, &Source::default(), &key(SampleFx { trim: 1000, ..SampleFx::default() }));
        assert_eq!(b.sample.frames(), 100);
        // Not past the loop: kept from the loop start and to the loop end.
        let src = Source { file_loop: Some((50, 250)), ..Source::default() };
        let b = build(&s, &src, &key(SampleFx { trim: 1000, ..SampleFx::default() }));
        assert_eq!((b.sample.frames(), b.looped), (201, Some((50, 200))));
        // A threshold above every level leaves one frame.
        let b = build(&s, &Source::default(), &key(SampleFx { trim: 32768, ..SampleFx::default() }));
        assert_eq!(b.sample.frames(), 1);
    }

    // spec "Reverb": a tail of up to 22050 frames; the quiet end is dropped again.
    #[test]
    fn reverb_adds_a_tail() {
        let mut s = Sample { rate: 44100, left: vec![0.0; 2000], right: vec![0.0; 2000] };
        s.left[0] = 1.0;
        s.right[0] = 1.0;
        for kind in [0, 1] {
            let b = build(&s, &Source::default(), &key(SampleFx { reverb: 256, reverb_type: kind, ..SampleFx::default() }));
            assert!(b.sample.frames() > 3000 && b.sample.frames() <= 2000 + 22050, "type {kind}: {}", b.sample.frames());
            let last = b.sample.frames() - 1;
            assert!(b.sample.left[last].abs() >= 1.0 / 512.0 || b.sample.right[last].abs() >= 1.0 / 512.0, "type {kind}");
            // Type 1's right side has its own last all-pass; type 0 treats both sides alike.
            assert_eq!(b.sample.left[2500] == b.sample.right[2500], kind == 0, "type {kind}");
        }
        // Silence keeps one frame (not below 1), or the loop's end frame number.
        let z = Sample { rate: 44100, left: vec![0.0; 2000], right: vec![0.0; 2000] };
        assert_eq!(build(&z, &Source::default(), &key(SampleFx { reverb: 128, ..SampleFx::default() })).sample.frames(), 1);
        let src = Source { file_loop: Some((10, 1500)), ..Source::default() };
        assert_eq!(build(&z, &src, &key(SampleFx { reverb: 128, ..SampleFx::default() })).sample.frames(), 1500);
    }

    // spec "Files below 8000 Hz": converted to the output rate; the loop points move with the frames.
    #[test]
    fn files_below_8000_hz_are_converted() {
        let s = Sample { rate: 6000, left: (0..600).map(|i| (i as f32 * 0.1).sin()).collect(), right: vec![0.0; 600] };
        let src = Source { file_loop: Some((100, 199)), ..Source::default() };
        let b = build(&s, &src, &key(SampleFx::default()));
        assert_eq!((b.sample.rate, b.sample.frames()), (44100, 4410));
        assert_eq!(b.looped, Some((735, 1469)));
        assert_eq!(b.per_file_frame, 7.35);
        // A whole frame of the file reads that frame (within the weights' rounding).
        assert!((b.sample.left[735 * 2] - s.left[200]).abs() < 1e-3);
        let s8 = Sample { rate: 8000, ..s.clone() };
        assert_eq!(build(&s8, &src, &key(SampleFx::default())).sample, s8);
    }

    // spec "The pass over each side", "Low-pass": open passes the sample unchanged; a cutoff below 1024 smooths it.
    #[test]
    fn pogo_ring_and_low_pass() {
        let s = noise(4000, 44100);
        let b = build(&s, &Source::default(), &key(SampleFx { cutoff: 512, ..SampleFx::default() }));
        let rough = |x: &[f32]| x.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>();
        assert!(rough(&b.sample.left) < rough(&s.left) * 0.5);
        // Ring modulation at full mix: the right side starts a quarter turn ahead (sine at 4096 = 1).
        let b = build(&s, &Source::default(), &key(SampleFx { ring_mix: 256, ..SampleFx::default() }));
        assert_eq!(b.sample.left[0], 0.0);
        assert_eq!(b.sample.right[0], s.right[0]);
        // Pogo keeps the length and the first frame.
        for v in [0, 512] {
            let b = build(&s, &Source::default(), &key(SampleFx { pogo: v, ..SampleFx::default() }));
            assert_eq!(b.sample.frames(), 4000);
            assert!((b.sample.left[0] - s.left[0]).abs() < 1e-6, "pogo {v}");
        }
    }
}
