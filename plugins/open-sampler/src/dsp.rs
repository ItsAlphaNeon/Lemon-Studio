//! Open Sampler sound: the precomputed tools / effects / time stretch that build the played sample, and the voices
//! (pitch, loop, declicking, envelopes and LFOs of the five targets, channel filter).
//!
// spec: plugins/open-sampler.md (§5 Sound), audio/sampler-playback.md (pitch, velocity, resampling),
// spec: audio/channel-envelopes.md (envelope laws), audio/sampler.md (the resampler, the end of a voice),
// spec: audio/declick.md (opening and closing fades)

use std::f64::consts::PI;
use std::sync::Arc;

use os_dsp::resample::{self, Quality};
use os_dsp::stretch::Stream;
use os_engine::{envelope, laws, Sample};

use crate::params::FxKey;

// ---------------------------------------------------------------------------------------------------------------
// Precomputed tools and effects (spec §5.4, §5.5, §5.6)

/// Builds the played sample from the loaded one and the precomputed settings, in the spec's order.
pub fn precompute(raw: &Sample, k: &FxKey) -> Sample {
    let sr = raw.rate.max(1) as f64;
    let (mut l, mut r) = cut(raw, k.start, k.length, sr);
    // Time stretching runs right after the start and length (spec §5.6).
    if k.stretch {
        let ratio = k.ratio_for(l.len() as f64 / sr);
        let shift = 2f64.powf(f64::from(k.shift) / 1200.0);
        if (ratio - 1.0).abs() > 1e-4 || (shift - 1.0).abs() > 1e-4 {
            let st = stretch(&Sample { rate: raw.rate, left: l, right: r }, ratio, shift);
            (l, r) = (st.left, st.right);
        }
    }
    if k.dc {
        remove_dc(&mut l);
        remove_dc(&mut r);
    }
    if k.reverse {
        l.reverse();
        r.reverse();
    }
    if k.polarity {
        l.iter_mut().chain(r.iter_mut()).for_each(|v| *v = -*v);
    }
    if k.swap {
        std::mem::swap(&mut l, &mut r);
    }
    if k.fade_stereo {
        fade_stereo(&mut l, &mut r);
    }
    // Effects page.
    if k.eq.abs() > 1e-4 {
        eq_tilt(&mut l, &mut r, k.eq, sr);
    }
    if k.rm_mix > 1e-4 {
        ring_mod(&mut l, &mut r, k.rm_mix, k.rm_freq, sr);
    }
    if k.cut < 0.999 {
        let fc = 8.0 * 15625f64.powf(f64::from(k.cut));
        let damp = 4.0 - 3.8 * f64::from(k.res);
        for ch in [&mut l, &mut r] {
            Svf::new(fc, damp, sr).run_lp(ch);
        }
    }
    if k.pogo.abs() > 1e-4 {
        pogo(&mut l, &mut r, k.pogo, sr);
    }
    if k.sdelay.abs() > 1e-4 {
        stereo_delay(&mut l, &mut r, k.sdelay, sr);
    }
    if k.reverb > 1e-4 {
        reverb(&mut l, &mut r, k.reverb, k.reverb_b, sr);
    }
    if k.normalize {
        normalize(&mut l, &mut r);
    }
    if k.boost > 1e-4 {
        let g = 1.0 + 16.0 * k.boost;
        l.iter_mut().chain(r.iter_mut()).for_each(|v| *v *= g);
    }
    if k.clip {
        l.iter_mut().chain(r.iter_mut()).for_each(|v| *v = v.clamp(-1.0, 1.0));
    }
    fades(&mut l, &mut r, k.fade_in, k.fade_out);
    if k.crossfade > 1e-4 {
        crossfade(&mut l, &mut r, k);
    }
    if k.trim > 1e-4 {
        trim(&mut l, &mut r, k.trim / 128.0);
    }
    Sample { rate: raw.rate, left: l, right: r }
}

/// Raised-cosine edge of `n` frames: 0 at `i` = 0, 1 at `n`.
fn edge(i: usize, n: usize) -> f32 {
    if n == 0 {
        return 1.0;
    }
    (0.5 - 0.5 * (PI * i as f64 / n as f64).cos()) as f32
}

/// SMP START / LENGTH: keep frames `start·n .. length·n` with 3.5 ms raised-cosine edges at the cuts.
fn cut(raw: &Sample, start: f32, length: f32, sr: f64) -> (Vec<f32>, Vec<f32>) {
    let n = raw.frames();
    let s0 = ((f64::from(start.clamp(0.0, 1.0)) * n as f64) as usize).min(n);
    let e0 = ((f64::from(length.clamp(0.0, 1.0)) * n as f64).round() as usize).clamp(s0, n);
    let mut l = raw.left[s0..e0].to_vec();
    let mut r = raw.right[s0..e0].to_vec();
    let fade = (0.0035 * sr).round() as usize;
    let len = l.len();
    if s0 > 0 {
        for i in 0..fade.min(len) {
            let g = edge(i, fade);
            l[i] *= g;
            r[i] *= g;
        }
    }
    if e0 < n {
        for i in 0..fade.min(len) {
            let g = edge(i, fade);
            l[len - 1 - i] *= g;
            r[len - 1 - i] *= g;
        }
    }
    (l, r)
}

fn remove_dc(ch: &mut [f32]) {
    if ch.is_empty() {
        return;
    }
    let mean = (ch.iter().map(|&v| f64::from(v)).sum::<f64>() / ch.len() as f64) as f32;
    ch.iter_mut().for_each(|v| *v -= mean);
}

/// Left fades out and right fades in over the sample (quarter cosine / sine).
fn fade_stereo(l: &mut [f32], r: &mut [f32]) {
    let n = l.len().max(1) as f64;
    for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
        let t = i as f64 / n;
        *a *= (PI * t / 2.0).cos() as f32;
        *b *= (PI * t / 2.0).sin() as f32;
    }
}

/// EQ tilt gains (low A, high B) at knob `v` (−1..1), interpolated in dB between the measured points.
pub fn eq_gains(v: f32) -> (f64, f64) {
    const PTS: [(f64, f64, f64); 5] = [(-1.0, 0.571, 1.93), (-0.5, 0.727, 1.38), (0.0, 1.0, 1.0), (0.5, 1.6, 0.76), (1.0, 4.0, 0.55)];
    let v = f64::from(v.clamp(-1.0, 1.0));
    for w in PTS.windows(2) {
        let ((x0, a0, b0), (x1, a1, b1)) = (w[0], w[1]);
        if v <= x1 {
            let u = (v - x0) / (x1 - x0);
            let db = |p: f64, q: f64| 10f64.powf((20.0 * p.log10() * (1.0 - u) + 20.0 * q.log10() * u) / 20.0);
            return (db(a0, a1), db(b0, b1));
        }
    }
    (4.0, 0.55)
}

fn eq_tilt(l: &mut [f32], r: &mut [f32], v: f32, sr: f64) {
    let (a, b) = eq_gains(v);
    let k = 1.0 - (-2.0 * PI * 1350.0 / sr).exp();
    for ch in [l, r] {
        let mut low = 0.0f64;
        for x in ch.iter_mut() {
            let xv = f64::from(*x);
            low += k * (xv - low);
            *x = (a * low + b * (xv - low)) as f32;
        }
    }
}

/// Ring modulation frequency (Hz) of the RM FREQ knob.
pub fn rm_hz(v: f32) -> f64 {
    61.25 + 1943.0 * f64::from(v.clamp(0.0, 1.0))
}

fn ring_mod(l: &mut [f32], r: &mut [f32], mix: f32, freq: f32, sr: f64) {
    let w = 2.0 * PI * rm_hz(freq) / sr;
    let m = f64::from(mix.clamp(0.0, 1.0));
    for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
        let ph = w * i as f64;
        *a = (f64::from(*a) * ((1.0 - m) + ph.sin() * m)) as f32;
        *b = (f64::from(*b) * ((1.0 - m) + ph.cos() * m)) as f32;
    }
}

/// Pogo: a time warp that starts fast (or slow) and settles; the length stays, silence once the source ends.
fn pogo(l: &mut Vec<f32>, r: &mut Vec<f32>, v: f32, sr: f64) {
    let v = f64::from(v);
    let (a, tau) = if v > 0.0 { (0.57 * v, 0.6) } else { (3.2 * -v, 0.023) };
    let n = l.len();
    let (src_l, src_r) = (std::mem::take(l), std::mem::take(r));
    *l = vec![0.0; n];
    *r = vec![0.0; n];
    for i in 0..n {
        let t = i as f64 / sr;
        // Integral of 1 + a·e^(−t/τ).
        let p = i as f64 + a * tau * sr * (1.0 - (-t / tau).exp());
        if p >= (n - 1) as f64 {
            break;
        }
        let j = p as usize;
        let f = (p - j as f64) as f32;
        l[i] = src_l[j] + (src_l[j + 1] - src_l[j]) * f;
        r[i] = src_r[j] + (src_r[j + 1] - src_r[j]) * f;
    }
}

/// Delays one channel by |v|·2048 frames (at 44.1 kHz); the sample grows by the delay. Positive delays the left.
fn stereo_delay(l: &mut Vec<f32>, r: &mut Vec<f32>, v: f32, sr: f64) {
    let d = (f64::from(v.abs()) * 2048.0 * sr / 44100.0).round() as usize;
    if d == 0 {
        return;
    }
    let n = l.len();
    let (late, early) = if v > 0.0 { (l, r) } else { (r, l) };
    let mut nl = vec![0.0; n + d];
    nl[d..].copy_from_slice(late);
    *late = nl;
    early.resize(n + d, 0.0);
}

fn normalize(l: &mut [f32], r: &mut [f32]) {
    let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
    if peak > 1e-9 {
        l.iter_mut().chain(r.iter_mut()).for_each(|v| *v /= peak);
    }
}

/// IN: gain `clamp(1 − (1 − t)·4v)`; OUT: gain `clamp(1 − t·4v)` (t = position / length).
fn fades(l: &mut [f32], r: &mut [f32], fin: f32, fout: f32) {
    if fin <= 0.0 && fout <= 0.0 {
        return;
    }
    let n = l.len().max(1) as f32;
    for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
        let t = i as f32 / n;
        let g = (1.0 - (1.0 - t) * 4.0 * fin).clamp(0.0, 1.0) * (1.0 - t * 4.0 * fout).clamp(0.0, 1.0);
        *a *= g;
        *b *= g;
    }
}

/// The last `(v/2)·n` frames before the loop end crossfade linearly into the frames from the loop start.
fn crossfade(l: &mut [f32], r: &mut [f32], k: &FxKey) {
    let n = l.len();
    let (s, e) = if k.loop_on {
        let s = (f64::from(k.loop_start) * n as f64) as usize;
        let e = ((f64::from(k.loop_end) * n as f64) as usize).min(n);
        (s.min(e), e)
    } else {
        (0, n)
    };
    let len = ((f64::from(k.crossfade) / 2.0 * n as f64) as usize).min(e - s).min(e.saturating_sub(s));
    if len < 2 {
        return;
    }
    for ch in [l, r] {
        for kk in 0..len {
            let w = kk as f32 / len as f32;
            let d = e - len + kk;
            ch[d] = ch[d] * (1.0 - w) + ch[s + kk] * w;
        }
    }
}

/// Removes leading and trailing frames below `thr` (linear) on both channels.
fn trim(l: &mut Vec<f32>, r: &mut Vec<f32>, thr: f32) {
    let loud = |i: usize| l[i].abs() >= thr || r[i].abs() >= thr;
    let n = l.len();
    let Some(a) = (0..n).find(|&i| loud(i)) else {
        l.clear();
        r.clear();
        return;
    };
    let b = (0..n).rev().find(|&i| loud(i)).unwrap_or(a) + 1;
    l.truncate(b);
    r.truncate(b);
    l.drain(..a);
    r.drain(..a);
}

/// Topology-preserving 2-pole state-variable filter (one channel).
#[derive(Clone, Copy, Default)]
struct Svf {
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
    ic1: f64,
    ic2: f64,
}

impl Svf {
    fn new(fc: f64, k: f64, sr: f64) -> Svf {
        let mut s = Svf::default();
        s.set(fc, k, sr);
        s
    }
    fn set(&mut self, fc: f64, k: f64, sr: f64) {
        let g = (PI * fc.clamp(10.0, sr * 0.45) / sr).tan();
        self.k = k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
    /// One sample: (low, band, high).
    #[inline]
    fn tick(&mut self, x: f64) -> (f64, f64, f64) {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        (v2, v1, x - self.k * v1 - v2)
    }
    fn run_lp(&mut self, ch: &mut [f32]) {
        for x in ch.iter_mut() {
            *x = self.tick(f64::from(*x)).0 as f32;
        }
    }
}

/// Reverb RT60 in seconds for an amount (type B: 0.45 s at 50 %, 1.1 s at 100 %; type A: 1.0 s at 100 %).
pub fn reverb_rt60(v: f32, b: bool) -> f64 {
    (if b { 1.1 } else { 1.0 }) * f64::from(v.clamp(0.0, 1.0)).powf(1.29)
}

/// Freeverb-like reverb with the measured dry level, wet level and decay; the sample grows by the audible tail.
fn reverb(l: &mut Vec<f32>, r: &mut Vec<f32>, v: f32, b: bool, sr: f64) {
    const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const ALLPASS: [usize; 4] = [556, 441, 341, 225];
    const SPREAD: usize = 23;
    let rt = reverb_rt60(v, b).max(0.05);
    // Dry / wet as measured at 50 % and up; below that both ease in from no effect (Lemon Studio choice).
    let ease = f64::from((v / 0.5).min(1.0));
    let (dry_full, wet_db) = if b { (0.945, -17.0) } else { (0.8, -9.5) };
    let dry = 1.0 - (1.0 - dry_full) * ease;
    let wet_target = 10f64.powf(wet_db / 20.0) * ease;
    let n = l.len();
    let tail = ((rt * 1.5 * sr) as usize).min((20.0 * sr) as usize);
    let total = n + tail;
    let scale = sr / 44100.0;
    let mut out = [vec![0.0f32; total], vec![0.0f32; total]];
    for (ch, outc) in out.iter_mut().enumerate() {
        let off = if ch == 0 { 0 } else { SPREAD };
        let mut combs: Vec<(Vec<f64>, usize, f64, f64)> = COMBS
            .iter()
            .map(|&d| {
                let len = (((d + off) as f64) * scale).round().max(1.0) as usize;
                let fb = 10f64.powf(-3.0 * len as f64 / (sr * rt));
                (vec![0.0; len], 0, fb, 0.0)
            })
            .collect();
        let mut aps: Vec<(Vec<f64>, usize)> =
            ALLPASS.iter().map(|&d| (vec![0.0; (((d + off) as f64) * scale).round().max(1.0) as usize], 0)).collect();
        let damp = 0.2;
        for (i, o) in outc.iter_mut().enumerate() {
            let x = if i < n { 0.5 * (f64::from(l[i]) + f64::from(r[i])) * 0.015 } else { 0.0 };
            let mut acc = 0.0;
            for (buf, idx, fb, store) in combs.iter_mut() {
                let y = buf[*idx];
                *store = y * (1.0 - damp) + *store * damp;
                buf[*idx] = x + *store * *fb;
                *idx = (*idx + 1) % buf.len();
                acc += y;
            }
            for (buf, idx) in aps.iter_mut() {
                let bv = buf[*idx];
                buf[*idx] = acc + bv * 0.5;
                *idx = (*idx + 1) % buf.len();
                acc = bv - acc;
            }
            *o = acc as f32;
        }
    }
    // Wet level relative to the input.
    let rms = |a: &[f32]| (a.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / a.len().max(1) as f64).sqrt();
    let rin = 0.5 * (rms(l) + rms(r));
    let rwet = 0.5 * (rms(&out[0][..n.max(1).min(total)]) + rms(&out[1][..n.max(1).min(total)]));
    let g = if rwet > 1e-12 { wet_target * rin / rwet } else { 0.0 };
    l.resize(total, 0.0);
    r.resize(total, 0.0);
    for i in 0..total {
        l[i] = (f64::from(l[i]) * dry + f64::from(out[0][i]) * g) as f32;
        r[i] = (f64::from(r[i]) * dry + f64::from(out[1][i]) * g) as f32;
    }
    // Cut the tail where it falls below −54 dB.
    let thr = 10f32.powf(-54.0 / 20.0);
    let mut end = total;
    while end > n && l[end - 1].abs() < thr && r[end - 1].abs() < thr {
        end -= 1;
    }
    l.truncate(end);
    r.truncate(end);
}

/// Time stretch to `ratio` × the length, the pitch moved by the factor `shift` (spec §5.6: Signalsmith Stretch).
pub fn stretch(s: &Sample, ratio: f64, shift: f64) -> Sample {
    let frames = (s.frames() as f64 * ratio).round().max(1.0) as usize;
    let (left, right) = os_dsp::stretch::whole(&s.left, &s.right, s.rate, frames, shift);
    Sample { rate: s.rate, left, right }
}

// ---------------------------------------------------------------------------------------------------------------
// Voices (spec §5.1 - §5.3)

/// One LFO (spec §5.2).
#[derive(Clone, Copy, Debug, Default)]
pub struct Lfo {
    /// 0 sine, 1 triangle, 2 pulse.
    pub shape: u8,
    pub delay: f64,
    pub attack: f64,
    pub amount: f64,
    pub hz: f64,
    pub global: bool,
}

impl Lfo {
    pub fn active(&self) -> bool {
        self.amount.abs() > 1e-4
    }

    fn wave(&self, phase: f64) -> f64 {
        let ph = phase.fract();
        match self.shape {
            0 => (2.0 * PI * ph).sin(),
            1 => 1.0 - 4.0 * ((ph + 0.25).fract() - 0.5).abs(),
            _ => {
                if ph < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }

    /// Value −amount..amount at voice time `t` with the given phase.
    fn value(&self, t: f64, phase: f64) -> f64 {
        if !self.active() || t < self.delay {
            return 0.0;
        }
        let fade = if self.attack > 0.0 { ((t - self.delay) / self.attack).min(1.0) } else { 1.0 };
        self.amount * fade * self.wave(phase)
    }
}

/// An envelope target: the envelope (when on) with its amount, and its LFO.
#[derive(Clone, Copy, Debug, Default)]
pub struct Target {
    pub env: Option<envelope::Params>,
    /// Envelope amount (−1..1; the Volume target ignores it).
    pub amount: f64,
    pub lfo: Lfo,
}

pub const PAN: usize = 0;
pub const VOL: usize = 1;
pub const MODX: usize = 2;
pub const MODY: usize = 3;
pub const PITCH: usize = 4;

/// Settings read once per block.
#[derive(Clone, Copy, Debug)]
pub struct Block {
    pub root: i32,
    /// Fine tune plus pitch bend, in cents.
    pub cents: f64,
    /// Resample and the realtime stretch methods: the stretch ratio (MUL) and TIME's target length in seconds (which
    /// overrides the ratio); the realtime methods also the PITCH shift in cents (spec §5.6).
    pub ts_cents: f64,
    pub ts_ratio: f64,
    pub ts_time: Option<f64>,
    /// A realtime stretch method: voices stretch while they play.
    pub realtime: bool,
    pub targets: [Target; 5],
    /// Filter type (index into `params::FILTERS`), MOD X / MOD Y positions.
    pub f_type: u8,
    pub f_cut: f64,
    pub f_res: f64,
    pub start_offset: f64,
    pub loop_on: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub pingpong: bool,
    pub declick: i32,
    pub zone: (i32, i32),
    pub add_key: bool,
}

impl Default for Block {
    fn default() -> Self {
        Block {
            root: 60,
            cents: 0.0,
            ts_cents: 0.0,
            ts_ratio: 1.0,
            ts_time: None,
            realtime: false,
            targets: [Target::default(); 5],
            f_type: 0,
            f_cut: 1.0,
            f_res: 0.0,
            start_offset: 0.0,
            loop_on: false,
            loop_start: 0.0,
            loop_end: 1.0,
            pingpong: false,
            declick: 0,
            zone: (0, crate::params::KEY_MAX),
            add_key: false,
        }
    }
}

/// A declick mode's fades (spec: audio/declick.md, the table of modes): the opening's length (ms; step removal in
/// modes 1 and 2, a gain fade otherwise), the closing's length (ms; the sweep in mode 1, a gain fade otherwise), the
/// fades' tension, and whether the voice bleeds (plays on past a clip's trimmed end under the closing).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Declick {
    pub open_ms: f64,
    pub step_removal: bool,
    pub close_ms: f64,
    pub sweep: bool,
    pub tension: f32,
    pub bleed: bool,
}

pub fn declick(mode: i32) -> Declick {
    let d = |open_ms, close_ms, tension, bleed| Declick { open_ms, step_removal: false, close_ms, sweep: false, tension, bleed };
    match mode {
        1 => Declick { step_removal: true, sweep: true, ..d(4.0, 10.0, 0.0, false) },
        2 => Declick { step_removal: true, ..d(4.0, 20.0, 0.0, true) },
        3 => d(20.0, 20.0, 0.0, true),
        4 => d(100.0, 100.0, 0.0, true),
        5 => d(200.0, 200.0, 5.0, true),
        // Not offered in the settings, valid as stored data. 7 and up name no preset in FL; they play as mode 0 here.
        6 => d(0.0, 2.0, 0.0, true),
        _ => d(0.0, 10.0, 0.0, false),
    }
}

/// Samples in a fade of `ms` at `rate`: `Round(ms * 0.001 * rate)`, at least 1.
fn fade_len(ms: f64, rate: f64) -> u32 {
    ((ms * 0.001 * rate).round() as u32).max(1)
}

/// A gain fade's value at table index `j` (1..=n) of a fade of `n` samples: a raised cosine, bent by the tension.
/// Closing: `j = i + 1`; opening: `j = n - i` (spec: audio/declick.md "Gain fade").
fn fade_gain(j: u32, n: u32, tension: f32) -> f32 {
    let scale = (std::f64::consts::PI / f64::from(n + 1)) as f32;
    let g = ((j as f32 * scale).cos() + 1.0) * 0.5;
    if tension == 0.0 {
        return g;
    }
    let (k4, k8) = (1.0 / tension.abs(), (tension.abs() + 1.0).ln());
    if tension > 0.0 {
        1.0 - (((1.0 - g) * k8).exp() - 1.0) * k4
    } else {
        ((g * k8).exp() - 1.0) * k4
    }
}

/// The closing of a voice: its declick mode's fade (or sweep), `n` samples long, from the sample it starts on.
#[derive(Clone, Copy, Debug)]
struct Close {
    /// Samples done.
    i: u32,
    n: u32,
    tension: f32,
    /// Mode 1's sweep: the corner's growth per sample and the length of the last millisecond's cosine.
    sweep: Option<(f64, u32)>,
    /// The sweep's low-pass memory per side.
    lp: [f32; 2],
}

impl Close {
    fn new(d: &Declick, rate: f64) -> Close {
        let n = fade_len(d.close_ms, rate);
        let sweep = d.sweep.then(|| (22050f64.ln() / (2f64.ln() * f64::from(n)) * 2f64.ln(), n.min(((rate * 0.001).round() as u32).max(1))));
        Close { i: 0, n, tension: d.tension, sweep, lp: [0.0; 2] }
    }

    /// Ends every voice quickly (Stop, all notes off): mode 0's 10 ms fade.
    fn quick(rate: f64) -> Close {
        Close::new(&declick(0), rate)
    }

    /// The next closing sample of a stereo frame, or `None` once the closing has run through.
    fn apply(&mut self, l: f32, r: f32, rate: f64) -> Option<(f32, f32)> {
        if self.i >= self.n {
            return None;
        }
        let i = self.i;
        self.i += 1;
        Some(match self.sweep {
            // A one-pole high-pass whose corner rises from 1 Hz to 22050 Hz, then a cosine over the last millisecond
            // (spec: audio/declick.md "Sweep").
            Some((k, m)) => {
                let c = ((2.0 / rate) * (f64::from(i + 1) * k).exp()).min(1.0) as f32;
                let w = if i < self.n - m {
                    1.0
                } else {
                    (((f64::from(i + 1 - (self.n - m)) * (1.0 / f64::from(m + 1)) * std::f64::consts::PI).cos() + 1.0) * 0.5) as f32
                };
                self.lp[0] += c * (l - self.lp[0]);
                self.lp[1] += c * (r - self.lp[1]);
                ((l - self.lp[0]) * w, (r - self.lp[1]) * w)
            }
            None => {
                let g = fade_gain(i + 1, self.n, self.tension);
                (l * g, r * g)
            }
        })
    }
}

/// Frames between modulation updates.
const CTL: usize = 32;

#[derive(Clone, Copy)]
struct Voice {
    key: u8,
    /// Host note id (per-note modulation), if the host gave one.
    id: Option<i32>,
    vel: f32,
    pos: f64,
    /// +1 forward, −1 backward (ping-pong loop).
    dir: f64,
    /// Key rate times the sample / host rate ratio.
    key_step: f64,
    /// Host tuning of this key's voices, semitones (portamento glides; spec: audio/channel-voices.md).
    tune: f64,
    age: u64,
    off: Option<u64>,
    /// Opening length in samples: only a voice that starts inside its sample has one (0 = none).
    open: u32,
    /// The closing, once it has started (note end, choke, the end of the sample).
    close: Option<Close>,
    phase: [f64; 5],
    filt: [[Svf; 2]; 2],
    /// Gain at the end of the last control span (ramps start here).
    gain: f32,
    pan: (f32, f32),
    /// Its stretcher in `Engine::streams` (realtime stretch methods).
    rt: Option<u16>,
}

/// Output frames a realtime stretch voice makes at a time.
const RT_BLOCK: usize = 128;

/// The stretcher of a voice in a realtime stretch method and the reader that feeds it (spec §5.6).
struct Rt {
    st: Stream,
    busy: bool,
    started: bool,
    /// Next input frame and direction: the reader runs the stretcher's lead ahead of what is heard.
    rpos: i64,
    rdir: i64,
    /// Input frames owed (fractions of a frame carried to the next block).
    owed: f64,
    input: Vec<f32>,
    l: Vec<f32>,
    r: Vec<f32>,
    at: usize,
    /// Output frames still to drop: the stretcher's latency.
    skip: usize,
}

impl Rt {
    fn new(rate: u32) -> Rt {
        Rt {
            st: Stream::new(rate),
            busy: false,
            started: false,
            rpos: 0,
            rdir: 1,
            owed: 0.0,
            input: Vec::with_capacity(RT_BLOCK * 16),
            l: vec![0.0; RT_BLOCK],
            r: vec![0.0; RT_BLOCK],
            at: RT_BLOCK,
            skip: 0,
        }
    }

    /// Pushes the next input frame onto `input`: loops and ping-pong followed, silence past the end.
    fn feed(&mut self, s: &Sample, (ls, le): (f64, Option<f64>), pingpong: bool) {
        if let Some(le) = le {
            let (ls, le) = (ls as i64, le as i64);
            if self.rdir > 0 && self.rpos >= le {
                if pingpong {
                    self.rdir = -1;
                    self.rpos = (2 * le - self.rpos - 1).max(ls);
                } else {
                    self.rpos -= le - ls;
                }
            } else if self.rdir < 0 && self.rpos < ls {
                self.rdir = 1;
                self.rpos = (2 * ls - self.rpos).min(le - 1);
            }
        }
        let (a, b) = match usize::try_from(self.rpos) {
            Ok(i) if i < s.frames() => (s.left[i], s.right[i]),
            _ => (0.0, 0.0),
        };
        self.input.push(a);
        self.input.push(b);
        self.rpos += self.rdir;
    }

    /// The next output frame of a voice heard from `pos` on: `transpose` = the pitch factor, `speed` = input frames per
    /// output frame.
    fn frame(&mut self, s: &Sample, pos: f64, transpose: f64, speed: f64, lp: (f64, Option<f64>), pingpong: bool) -> (f32, f32) {
        if !self.started {
            self.started = true;
            (self.rpos, self.rdir, self.owed) = (pos.max(0.0).floor() as i64, 1, 0.0);
            self.input.clear();
            for _ in 0..self.st.lead() {
                self.feed(s, lp, pingpong);
            }
            self.st.set_transpose(transpose);
            self.st.start(&self.input, speed);
            self.skip = self.st.delay();
            self.at = RT_BLOCK;
        }
        loop {
            if self.at < RT_BLOCK {
                let f = (self.l[self.at], self.r[self.at]);
                self.at += 1;
                if self.skip > 0 {
                    self.skip -= 1;
                    continue;
                }
                return f;
            }
            self.owed += RT_BLOCK as f64 * speed;
            let k = self.owed.floor();
            self.owed -= k;
            self.input.clear();
            for _ in 0..k as usize {
                self.feed(s, lp, pingpong);
            }
            self.st.set_transpose(transpose);
            self.st.process(&self.input, &mut self.l, &mut self.r);
            self.at = 0;
        }
    }
}

pub struct Engine {
    pub sample: Option<Arc<Sample>>,
    /// How the resampler gets its weights: tables for live playback, the formula for an offline render.
    pub quality: Quality,
    rate: f32,
    voices: Vec<Voice>,
    /// Free-running phases of the global LFOs.
    global_phase: [f64; 5],
    /// Stretchers of the realtime stretch voices (made when first needed, then reused).
    streams: Vec<Rt>,
}

pub const MAX_VOICES: usize = 128;

impl Engine {
    pub fn new(rate: f32) -> Self {
        Engine { sample: None, quality: Quality::Table, rate, voices: Vec::with_capacity(MAX_VOICES), global_phase: [0.0; 5], streams: Vec::new() }
    }

    pub fn set_rate(&mut self, rate: f32) {
        if rate != self.rate {
            self.voices.clear();
            self.streams.clear();
        }
        self.rate = rate;
    }

    pub fn reset(&mut self) {
        self.voices.clear();
        self.streams.iter_mut().for_each(|r| r.busy = false);
    }

    /// A free stretcher for a new realtime voice.
    fn take_stream(&mut self) -> Option<u16> {
        let i = match self.streams.iter().position(|r| !r.busy) {
            Some(i) => i,
            None if self.streams.len() < MAX_VOICES => {
                self.streams.push(Rt::new(self.rate as u32));
                self.streams.len() - 1
            }
            None => return None,
        };
        let r = &mut self.streams[i];
        (r.busy, r.started) = (true, false);
        Some(i as u16)
    }

    /// Frees the stretcher of a voice that ended.
    fn free(&mut self, v: &Voice) {
        if let Some(r) = v.rt.and_then(|i| self.streams.get_mut(usize::from(i))) {
            r.busy = false;
        }
    }

    /// True while any voice is playing.
    pub fn has_voices(&self) -> bool {
        !self.voices.is_empty()
    }

    /// Keys currently sounding.
    pub fn sounding(&self) -> impl Iterator<Item = u8> + '_ {
        self.voices.iter().filter(|v| v.close.is_none()).map(|v| v.key)
    }

    /// How many times its length the current sample lasts in Resample or a realtime method: TIME's length when set,
    /// else MUL (spec §5.6).
    fn stretch_factor(b: &Block, s: &Sample) -> f64 {
        let ratio = match b.ts_time {
            Some(t) if s.frames() > 0 => t / (s.frames() as f64 / f64::from(s.rate.max(1))),
            _ => b.ts_ratio,
        };
        ratio.max(1e-3)
    }

    pub fn note_on(&mut self, key: u8, velocity: f32, b: &Block) {
        self.note_on_id(key, velocity, None, b);
    }

    /// `note_on` for a host note `id` (per-note modulation reaches it by that id).
    pub fn note_on_id(&mut self, key: u8, velocity: f32, id: Option<i32>, b: &Block) {
        let Some(s) = self.sample.clone() else { return };
        let zkey = if b.add_key { i32::from(key) - (b.root - 60) } else { i32::from(key) };
        if zkey < b.zone.0 || zkey > b.zone.1 || s.frames() == 0 {
            return;
        }
        // A new note leaves the channel's other voices alone, the same key's too (spec: audio/sampler.md "A voice");
        // cut groups and the voice limit are the host's note rules (audio/note-rules.md).
        if self.voices.len() >= MAX_VOICES {
            let v = self.voices.remove(0);
            self.free(&v);
        }
        let ratio = f64::from(s.rate) / f64::from(self.rate);
        let key_step = 2f64.powf((f64::from(key) - f64::from(b.root)) / 12.0) * ratio;
        // FL velocity byte = plugin velocity × 128; 100 = unity (spec: audio/sampler-playback.md).
        let vel = laws::velocity_gain((velocity.clamp(0.0, 1.0) * 128.0).round() as u8) as f32;
        let pos = (b.start_offset.clamp(0.0, 1.0) * s.frames() as f64).floor();
        let open = if pos > 0.0 { fade_len(declick(b.declick).open_ms, f64::from(self.rate)) } else { 0 };
        let mut phase = [0.0; 5];
        for (i, t) in b.targets.iter().enumerate() {
            if t.lfo.global {
                phase[i] = self.global_phase[i];
            }
        }
        let rt = if b.realtime { self.take_stream() } else { None };
        self.voices.push(Voice {
            key,
            id,
            vel,
            pos,
            dir: 1.0,
            key_step,
            tune: 0.0,
            age: 0,
            off: None,
            open,
            close: None,
            phase,
            filt: [[Svf::default(); 2]; 2],
            gain: -1.0,
            pan: (1.0, 1.0),
            rt,
        });
    }

    pub fn note_off(&mut self, key: u8, b: &Block) {
        let close = Close::new(&declick(b.declick), f64::from(self.rate));
        let vol_env = b.targets[VOL].env.is_some();
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.off.is_none()) {
            v.off = Some(v.age);
            // A looping voice without a volume envelope closes at the note's end; a one-shot voice without one
            // ignores it (spec: audio/sampler.md "The end of a voice").
            if b.loop_on && !vol_env && v.close.is_none() {
                v.close = Some(close);
            }
        }
    }

    /// A note that has not played yet starts `extra` (0..1 of the sample) further in, on top of START OFFSET
    /// (spec §7: a trimmed audio clip).
    pub fn start_later(&mut self, id: i32, extra: f32, b: &Block) {
        let Some(s) = &self.sample else { return };
        let open = fade_len(declick(b.declick).open_ms, f64::from(self.rate));
        for v in self.voices.iter_mut().filter(|v| v.id == Some(id) && v.age == 0) {
            let at = (b.start_offset + f64::from(extra)).clamp(0.0, 1.0);
            v.pos = (at * s.frames() as f64).floor();
            v.open = if v.pos > 0.0 { open } else { 0 };
        }
    }

    /// Tunes the voices of `key` to `semitones` from the key's pitch (the host's tuning expression: glides).
    pub fn tune(&mut self, key: u8, semitones: f64) {
        for v in self.voices.iter_mut().filter(|v| v.key == key) {
            v.tune = semitones;
        }
    }

    /// Closes the voices of `key` with the declick mode's closing, whatever their envelopes (the host's choke: an
    /// audio clip ends, a cut group, the voice limit; spec: audio/note-rules.md).
    pub fn choke(&mut self, key: u8, b: &Block) {
        let close = Close::new(&declick(b.declick), f64::from(self.rate));
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.close.is_none()) {
            v.close = Some(close);
        }
    }

    /// Ends every voice with a short fade.
    pub fn all_off(&mut self) {
        let close = Close::quick(f64::from(self.rate));
        for v in self.voices.iter_mut().filter(|v| v.close.is_none()) {
            v.close = Some(close);
        }
    }

    /// Adds the voices into `out_l` / `out_r`.
    pub fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32], b: &Block) {
        let len = out_l.len();
        let rate = f64::from(self.rate);
        for (i, t) in b.targets.iter().enumerate() {
            self.global_phase[i] = (self.global_phase[i] + t.lfo.hz * len as f64 / rate).fract();
        }
        let Some(s) = self.sample.clone() else {
            self.voices.clear();
            return;
        };
        let n = s.frames();
        if n == 0 {
            self.voices.clear();
            return;
        }
        // Resample plays the sample faster or slower; a realtime voice reads `speed` input frames per output frame
        // and its stretcher keeps the pitch.
        let factor = Self::stretch_factor(b, &s);
        let rs = if b.realtime { 1.0 } else { 1.0 / factor };
        let speed = f64::from(s.rate) / rate / factor;
        let (ls, le) = loop_frames(b, n);
        let mut i = 0;
        while i < self.voices.len() {
            let v = &mut self.voices[i];
            let rt = v.rt.and_then(|k| self.streams.get_mut(usize::from(k)));
            let alive = render_voice(v, rt, &s, b, (rs, speed), (ls, le), rate, self.quality, out_l, out_r);
            if alive {
                i += 1;
            } else {
                let v = self.voices.swap_remove(i);
                self.free(&v);
            }
        }
    }
}

/// Loop start / end in frames, `None` when no loop plays.
fn loop_frames(b: &Block, n: usize) -> (f64, Option<f64>) {
    if !b.loop_on {
        return (0.0, None);
    }
    let ls = (b.loop_start.clamp(0.0, 1.0) * n as f64).floor();
    let le = (b.loop_end.clamp(0.0, 1.0) * n as f64).floor().min(n as f64);
    if le - ls < 2.0 {
        (0.0, None)
    } else {
        (ls, Some(le))
    }
}

/// Modulation at a voice time: gain, pan gains, extra cents, filter positions. `None` = the envelope has ended.
struct Mods {
    gain: f64,
    pan: f64,
    cents: f64,
    cut: f64,
    res: f64,
}

fn mods(v: &Voice, b: &Block, t: f64, off: Option<f64>) -> Option<Mods> {
    let tg = &b.targets;
    let lfo = |i: usize| tg[i].lfo.value(t, v.phase[i]);
    let env = |i: usize| tg[i].env.map(|p| p.level(t, off));
    let mut gain = 1.0;
    if let Some(l) = env(VOL) {
        gain = l?;
    }
    let lv = lfo(VOL);
    if lv != 0.0 {
        // Tremolo: down to 1 − |amount| (spec §5.2).
        let a = tg[VOL].lfo.amount.abs();
        let w = lv / a.max(1e-9);
        gain *= 1.0 - a * (1.0 - w) / 2.0;
    }
    let pan = lfo(PAN).clamp(-1.0, 1.0);
    let mut cents = lfo(PITCH) * 1200.0;
    if let Some(l) = env(PITCH) {
        cents += l.unwrap_or(0.0) * tg[PITCH].amount * 2400.0;
    }
    let mut cut = b.f_cut + lfo(MODX) * 0.5;
    if let Some(l) = env(MODX) {
        cut += l.unwrap_or(0.0) * tg[MODX].amount;
    }
    let mut res = b.f_res + lfo(MODY) * 0.5;
    if let Some(l) = env(MODY) {
        res += l.unwrap_or(0.0) * tg[MODY].amount;
    }
    Some(Mods { gain, pan, cents, cut: cut.clamp(0.0, 1.0), res: res.clamp(0.0, 1.0) })
}

/// Channel filter cutoff (Hz) of a MOD X position (spec §5.3).
pub fn filter_hz(cut: f64) -> f64 {
    8.0 * 15625f64.powf(cut.clamp(0.0, 1.0))
}

#[allow(clippy::too_many_arguments)]
fn render_voice(
    v: &mut Voice,
    mut rt: Option<&mut Rt>,
    s: &Sample,
    b: &Block,
    (rs, speed): (f64, f64),
    (ls, le): (f64, Option<f64>),
    rate: f64,
    quality: Quality,
    out_l: &mut [f32],
    out_r: &mut [f32],
) -> bool {
    let len = out_l.len();
    let n = s.frames() as f64;
    let dc = declick(b.declick);
    let frame_s = 1.0 / rate;
    // Low-pass types fully open and not modulated: no filter.
    let lp_type = matches!(b.f_type, 0 | 1 | 5 | 6 | 7);
    let modulated = b.targets[MODX].env.is_some() && b.targets[MODX].amount != 0.0
        || b.targets[MODX].lfo.active()
        || b.targets[MODY].env.is_some() && b.targets[MODY].amount != 0.0
        || b.targets[MODY].lfo.active();
    let bypass = lp_type && b.f_cut >= 0.999 && b.f_res <= 0.001 && !modulated;
    let stages = if matches!(b.f_type, 5 | 7) { 2 } else { 1 };
    let mut f = 0;
    while f < len {
        let chunk = CTL.min(len - f);
        let t0 = v.age as f64 * frame_s;
        let off = v.off.map(|o| o as f64 * frame_s);
        let Some(m0) = mods(v, b, t0, off) else { return false };
        let g1 = match mods(v, b, t0 + chunk as f64 * frame_s, off) {
            Some(m) => m.gain,
            None => 0.0,
        };
        let g0 = if v.gain < 0.0 { m0.gain as f32 } else { v.gain };
        let g1 = g1 as f32;
        let pan = (((1.0 - m0.pan).min(1.0)) as f32, ((1.0 + m0.pan).min(1.0)) as f32);
        let (p0, p1) = (if v.gain < 0.0 { pan } else { v.pan }, pan);
        if !bypass {
            let fc = filter_hz(m0.cut);
            let k = 2.0 - 1.95 * m0.res;
            for st in v.filt.iter_mut().take(stages) {
                for ch in st.iter_mut() {
                    ch.set(fc, k, rate);
                }
            }
        }
        // The pitch factor: a resampling step, or a realtime voice's transposition (with the stretch's PITCH).
        let step = v.key_step * 2f64.powf((b.cents + m0.cents + v.tune * 100.0 + b.ts_cents) / 1200.0) * rs;
        // What is heard moves on by `step` source frames per frame, or by `speed` in a realtime voice.
        let advance = if rt.is_some() { speed } else { step };
        for j in 0..chunk {
            // Loop / end handling.
            if let Some(le) = le {
                if v.dir > 0.0 && v.pos >= le {
                    if b.pingpong {
                        v.dir = -1.0;
                        v.pos = (2.0 * le - v.pos).max(ls);
                    } else {
                        v.pos -= le - ls;
                    }
                } else if v.dir < 0.0 && v.pos < ls {
                    v.dir = 1.0;
                    v.pos = (2.0 * ls - v.pos).min(le);
                }
            }
            if v.pos < 0.0 {
                return false;
            }
            // The end of the sample: the closing starts on the first sample at or past it and works on the held
            // last frame (spec: audio/declick.md "When the closing starts").
            if v.pos >= n && v.close.is_none() {
                v.close = Some(Close::new(&dc, rate));
            }
            let (mut l, mut r) = match rt.as_mut() {
                Some(x) => x.frame(s, v.pos, step, speed, (ls, le), b.pingpong),
                None => resample::read(&s.left, &s.right, v.pos, step.abs(), quality),
            };
            if !bypass {
                let mut out = [f64::from(l), f64::from(r)];
                for st in v.filt.iter_mut().take(stages) {
                    for (c, x) in out.iter_mut().enumerate() {
                        let (lo, bp, hi) = st[c].tick(*x);
                        *x = match b.f_type {
                            2 => bp,
                            3 => hi,
                            4 => lo + hi,
                            _ => lo,
                        };
                    }
                }
                (l, r) = (out[0] as f32, out[1] as f32);
            }
            // The fades act on the signal before the gains (spec: audio/declick.md "Where the fades sit"). FL works on
            // a voice's fades only once it closes, so an opening shows only on a voice that closes while it is younger
            // than its opening ("When the opening applies"); the step removal of modes 1 and 2 is not done here.
            if let Some(c) = v.close.as_mut() {
                if v.open > 0 && !dc.step_removal && v.age < u64::from(v.open) {
                    let o = fade_gain(v.open - v.age as u32, v.open, dc.tension);
                    (l, r) = (l * o, r * o);
                }
                match c.apply(l, r, rate) {
                    Some(x) => (l, r) = x,
                    None => return false,
                }
            }
            let u = j as f32 / chunk as f32;
            let g = v.vel * (g0 + (g1 - g0) * u);
            let (pl, pr) = (p0.0 + (p1.0 - p0.0) * u, p0.1 + (p1.1 - p0.1) * u);
            out_l[f + j] += l * g * pl;
            out_r[f + j] += r * g * pr;
            v.pos += advance * v.dir;
            v.age += 1;
        }
        v.gain = g1;
        v.pan = p1;
        for (i, t) in b.targets.iter().enumerate() {
            v.phase[i] = (v.phase[i] + t.lfo.hz * chunk as f64 * frame_s).fract();
        }
        f += chunk;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize) -> Sample {
        let left: Vec<f32> = (0..n).map(|i| i as f32 / n as f32).collect();
        Sample { rate: 48000, right: left.clone(), left }
    }

    fn noise(n: usize, rate: u32) -> Sample {
        let mut x: u32 = 12345;
        let mut v = || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        let left: Vec<f32> = (0..n).map(|_| v() * 0.5).collect();
        let right: Vec<f32> = (0..n).map(|_| v() * 0.5).collect();
        Sample { rate, left, right }
    }

    fn fx() -> FxKey {
        FxKey::neutral()
    }

    #[test]
    fn root_key_plays_the_sample_exactly() {
        let s = ramp(100);
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(s.clone()));
        let b = Block::default();
        e.note_on(60, 100.0 / 128.0, &b);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        e.render(&mut l, &mut r, &b);
        assert_eq!(&l[..], &s.left[..64]);
        let (mut l2, mut r2) = (vec![0.0; 64], vec![0.0; 64]);
        e.render(&mut l2, &mut r2, &b);
        assert_eq!(l2[35], s.left[99]);
        // Past the end the last frame is held under the 10 ms closing (spec: audio/declick.md).
        assert!((l2[36] - s.left[99]).abs() < 1e-3 && l2[63] < l2[36], "{} {}", l2[36], l2[63]);
        assert_eq!(e.sounding().count(), 0, "closing");
        let (mut l3, mut r3) = (vec![0.0; 480], vec![0.0; 480]);
        e.render(&mut l3, &mut r3, &b);
        assert!(l3[451] > 0.0 && l3[452..].iter().all(|v| *v == 0.0), "480 closing samples");
        assert!(!e.has_voices());
    }

    #[test]
    fn octave_up_plays_twice_as_fast() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(ramp(1000)));
        let b = Block::default();
        e.note_on(72, 100.0 / 128.0, &b);
        let (mut l, mut r) = (vec![0.0; 600], vec![0.0; 600]);
        e.render(&mut l, &mut r, &b);
        assert!((l[200] - 0.4).abs() < 0.01, "{}", l[200]);
        // The end comes after 500 samples; the closing fades the held last frame.
        assert!(l[505] > 0.99 && l[550] < l[505], "{} {}", l[505], l[550]);
        let (mut l, mut r) = (vec![0.0; 600], vec![0.0; 600]);
        e.render(&mut l, &mut r, &b);
        assert!(!e.has_voices());
    }

    // spec: audio/sampler.md "A voice": a channel's voices do not affect one another, the same key's neither.
    #[test]
    fn a_repeated_key_leaves_the_ringing_voice_alone() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(440.0, 48000)));
        let b = Block::default();
        e.note_on(60, 0.78, &b);
        let (mut l, mut r) = (vec![0.0; 480], vec![0.0; 480]);
        e.render(&mut l, &mut r, &b);
        e.note_on(60, 0.78, &b);
        e.render(&mut l, &mut r, &b);
        assert_eq!(e.voices.len(), 2);
        assert_eq!(e.sounding().count(), 2);
        // A choke closes both through the mode's closing: 20 ms in mode 2.
        let b2 = Block { declick: 2, ..Block::default() };
        e.choke(60, &b2);
        for _ in 0..2 {
            l.fill(0.0);
            e.render(&mut l, &mut r, &b2);
        }
        assert!(e.has_voices(), "20 ms not over after 10");
        e.render(&mut l, &mut r, &b2);
        e.render(&mut l, &mut r, &b2);
        assert!(!e.has_voices());
    }

    // spec: audio/declick.md "Gain fade": the closing falls from just under 1 to just over 0; tension 5 bends it.
    #[test]
    fn closing_fades_follow_the_tables() {
        assert_eq!(fade_len(10.0, 44100.0), 441);
        assert_eq!(fade_len(4.0, 44100.0), 176);
        assert_eq!(fade_len(2.0, 44100.0), 88);
        let n = 441;
        assert!(fade_gain(1, n, 0.0) > 0.9999 && fade_gain(1, n, 0.0) < 1.0);
        assert!(fade_gain(n, n, 0.0) > 0.0 && fade_gain(n, n, 0.0) < 1e-4);
        assert!((fade_gain(221, n, 0.0) - 0.5).abs() < 1e-6);
        let mid = fade_gain(221, n, 5.0);
        assert!(mid > 0.5 && mid < 1.0, "{mid}");
        // The sweep ends silent and keeps a constant from jumping.
        let mut c = Close::new(&declick(1), 44100.0);
        let mut last = (1.0, 1.0);
        while let Some(x) = c.apply(1.0, 1.0, 44100.0) {
            last = x;
        }
        assert!(last.0.abs() < 1e-3);
    }

    #[test]
    fn zone_and_loop() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(ramp(100)));
        let b = Block { zone: (48, 59), ..Block::default() };
        e.note_on(60, 0.78, &b);
        assert!(!e.has_voices(), "outside the zone");
        let b = Block { loop_on: true, loop_start: 0.5, loop_end: 1.0, ..Block::default() };
        e.note_on(60, 100.0 / 128.0, &b);
        let (mut l, mut r) = (vec![0.0; 400], vec![0.0; 400]);
        e.render(&mut l, &mut r, &b);
        assert!(e.has_voices(), "a loop keeps playing");
        assert_eq!(l[100], l[50], "jumps back to the loop start");
        e.note_off(60, &b);
        let (mut l, mut r) = (vec![0.0; 2000], vec![0.0; 2000]);
        e.render(&mut l, &mut r, &b);
        assert!(!e.has_voices(), "note-off ends a loop without a volume envelope");
    }

    #[test]
    fn tools_follow_the_measured_laws() {
        let s = ramp(10);
        assert_eq!(precompute(&s, &FxKey { reverse: true, ..fx() }).left[0], 0.9);
        assert_eq!(precompute(&s, &FxKey { normalize: true, ..fx() }).left[9], 1.0);
        // IN 39 %: gain 1 − (1 − t)·1.56.
        let s = Sample { rate: 44100, left: vec![1.0; 1000], right: vec![1.0; 1000] };
        let p = precompute(&s, &FxKey { fade_in: 0.39, ..fx() });
        assert!((p.left[525] - 0.259).abs() < 0.01, "{}", p.left[525]);
        let p = precompute(&s, &FxKey { fade_out: 0.39, ..fx() });
        assert!(p.left[700] == 0.0 && (p.left[200] - 0.688).abs() < 0.01);
        // Boost + clip.
        let p = precompute(&Sample { rate: 44100, left: vec![0.1; 10], right: vec![0.1; 10] }, &FxKey { boost: 1.0, ..fx() });
        assert!((p.left[0] - 1.7).abs() < 1e-5);
        let p = precompute(&Sample { rate: 44100, left: vec![0.1; 10], right: vec![0.1; 10] }, &FxKey { boost: 1.0, clip: true, ..fx() });
        assert_eq!(p.left[0], 1.0);
        // Stereo delay grows the sample and delays the left side.
        let p = precompute(&noise(1000, 44100), &FxKey { sdelay: 0.5, ..fx() });
        assert_eq!(p.frames(), 2024);
        assert_eq!(p.left[..1024].iter().filter(|v| **v != 0.0).count(), 0);
        // Trim removes silence.
        let mut q = noise(100, 44100);
        q.left.splice(0..0, vec![0.0; 50]);
        q.right.splice(0..0, vec![0.0; 50]);
        assert!(precompute(&q, &FxKey { trim: 0.5, ..fx() }).frames() <= 100);
    }

    #[test]
    fn effects_keep_their_shapes() {
        let s = noise(44100, 44100);
        let rev = precompute(&s, &FxKey { reverb: 1.0, ..fx() });
        assert!(rev.frames() > s.frames(), "the reverb adds a tail");
        let pg = precompute(&s, &FxKey { pogo: 1.0, ..fx() });
        assert_eq!(pg.frames(), s.frames());
        let (a, b) = eq_gains(0.5);
        assert!((a - 1.6).abs() < 1e-9 && (b - 0.76).abs() < 1e-9);
        assert!((rm_hz(0.5) - 1032.75).abs() < 1e-6);
    }

    fn sine(hz: f64, n: usize) -> Sample {
        let x: Vec<f32> = (0..n).map(|i| (2.0 * PI * hz * i as f64 / 48000.0).sin() as f32 * 0.5).collect();
        Sample { rate: 48000, left: x.clone(), right: x }
    }

    /// Frequency from the rising zero crossings.
    fn hz(x: &[f32]) -> f64 {
        let ups: Vec<usize> = x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
        (ups.len() - 1) as f64 * 48000.0 / (ups[ups.len() - 1] - ups[0]) as f64
    }

    /// Renders a held note of `key` for `secs` seconds at 48 kHz.
    fn play(e: &mut Engine, key: u8, b: &Block, secs: f64) -> Vec<f32> {
        e.note_on(key, 100.0 / 128.0, b);
        let mut out = Vec::new();
        let (mut l, mut r) = (vec![0.0; 480], vec![0.0; 480]);
        for _ in 0..(secs * 100.0) as usize {
            l.fill(0.0);
            r.fill(0.0);
            e.render(&mut l, &mut r, b);
            out.extend_from_slice(&l);
        }
        out
    }

    // spec §5.6: the realtime methods stretch while the voice plays; the key and PITCH move only the pitch.
    #[test]
    fn realtime_voices_keep_the_length_and_move_the_pitch() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(440.0, 48000)));
        // Twice as long, an octave up by the key; then by PITCH.
        for (key, cents) in [(72u8, 0.0), (60, 1200.0)] {
            let b = Block { realtime: true, ts_ratio: 2.0, ts_cents: cents, ..Block::default() };
            let out = play(&mut e, key, &b, 2.5);
            let f = hz(&out[24000..72000]);
            assert!((f - 880.0).abs() < 4.0, "key {key}: {f} Hz");
            let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
            assert!(rms(&out[80000..92000]) > 0.1, "key {key}: still playing after the sample's own length");
            assert!(!e.has_voices(), "key {key}: over after twice its length");
            e.reset();
        }
        assert!(e.streams.iter().all(|r| !r.busy), "stretchers freed");
    }

    // spec §5.6: Resample fits TIME by speed; PITCH plays no part.
    #[test]
    fn resample_fits_the_time_by_speed() {
        let mut e = Engine::new(48000.0);
        e.sample = Some(Arc::new(sine(440.0, 48000)));
        let b = Block { ts_time: Some(2.0), ts_cents: 0.0, ..Block::default() };
        let out = play(&mut e, 60, &b, 1.0);
        assert!((hz(&out[4800..43200]) - 220.0).abs() < 1.0);
    }

    #[test]
    fn stretch_keeps_pitch_and_changes_length() {
        let n = 48000;
        let sine: Vec<f32> = (0..n).map(|i| (2.0 * PI * 440.0 * i as f64 / 48000.0).sin() as f32 * 0.5).collect();
        let s = Sample { rate: 48000, left: sine.clone(), right: sine };
        let st = stretch(&s, 2.0, 1.0);
        assert_eq!(st.frames(), 2 * n);
        // A fifth up at the same length.
        let up = stretch(&s, 1.0, 1.5);
        assert_eq!(up.frames(), n);
        assert!((hz(&up.left[12000..36000]) - 660.0).abs() < 3.0);
        // Count zero crossings in the middle: the same frequency.
        let zc = |v: &[f32]| v.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let c = zc(&st.left[24000..72000]);
        assert!((c as i32 - 440).abs() <= 6, "{c}");
    }
}
