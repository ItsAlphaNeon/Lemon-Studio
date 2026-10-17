// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Time stretching and pitch shifting: Lemon Studio's stretcher wherever a project asks for one of FL Studio's own
//! stretch methods, which are proprietary. Built on Signalsmith Stretch (MIT licence, Signalsmith Audio) through the
//! `signalsmith-stretch` binding (MIT licence, Colin Marc).
//!
//! - [`whole`]: a whole buffer at once, to a given length and pitch (the methods that stretch a sample before it
//!   plays).
//! - [`Stream`]: a stretch that runs while a voice plays (the realtime methods), its pitch free to move.
//!
// spec: audio/time-stretch.md

use signalsmith_stretch::Stretch;

/// Stretches the stereo buffer `left` / `right` (`rate` Hz) to `frames` frames with its pitch multiplied by
/// `transpose`. The result starts where the input starts: the stretcher's latency is taken out.
pub fn whole(left: &[f32], right: &[f32], rate: u32, frames: usize, transpose: f64) -> (Vec<f32>, Vec<f32>) {
    let n = left.len().min(right.len());
    if frames == 0 || n == 0 {
        return (vec![0.0; frames], vec![0.0; frames]);
    }
    let mut st = Stretch::preset_default(2, rate.max(1));
    st.set_transpose_factor(transpose as f32, None);
    // The stretcher needs an output of at least twice its latency: a shorter one is made from the input padded with
    // silence in proportion (the same ratio), and the padding's part of the output is cut off.
    let out_n = frames.max(2 * st.output_latency() + 1);
    let in_n = if out_n > frames { ((n as f64 * out_n as f64 / frames as f64).round() as usize).max(n) } else { n };
    let mut input = vec![0.0f32; in_n * 2];
    for i in 0..n {
        input[2 * i] = left[i];
        input[2 * i + 1] = right[i];
    }
    let mut out = vec![0.0f32; out_n * 2];
    let done = st.exact(&input, &mut out);
    debug_assert!(done, "the output is at least twice the latency");
    let l = out.iter().step_by(2).take(frames).copied().collect();
    let r = out.iter().skip(1).step_by(2).take(frames).copied().collect();
    (l, r)
}

/// A stretch that runs while a voice plays: stereo frames in, stereo frames out, in any amounts per call, the pitch
/// free to change between calls. After [`Stream::start`] the first [`Stream::delay`] frames out belong to the time
/// before the starting point and are dropped by the caller; from then on output frame `k` belongs to the input
/// `k * speed` frames after the starting point.
pub struct Stream {
    st: Stretch,
    rate: u32,
    out: Vec<f32>,
}

impl Stream {
    /// A stretcher for audio at `rate` Hz.
    pub fn new(rate: u32) -> Stream {
        Stream { st: Stretch::preset_default(2, rate.max(1)), rate, out: Vec::new() }
    }

    /// The rate it was made for.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Frames from the starting point on that [`Stream::start`] reads ahead.
    pub fn lead(&self) -> usize {
        self.st.input_latency()
    }

    /// Output frames after [`Stream::start`] that come before the starting point.
    pub fn delay(&self) -> usize {
        self.st.output_latency()
    }

    /// Frames before the starting point that [`Stream::start`] can take as well (the input it has heard: its block
    /// and one interval in all, the lead included).
    pub fn history(&self) -> usize {
        ((f64::from(self.rate) * 0.15).ceil() as usize).saturating_sub(self.lead())
    }

    /// Output frames between two analyses (30 ms at the default settings).
    pub fn interval(&self) -> usize {
        (f64::from(self.rate) * 0.03).round() as usize
    }

    /// Starts afresh at a starting point: `ahead` holds the first [`Stream::lead`] frames from it on (interleaved
    /// stereo), after up to [`Stream::history`] frames before it; `speed` is the input frames per output frame to
    /// come. Input then goes on from the frame after `ahead`.
    pub fn start(&mut self, ahead: &[f32], speed: f64) {
        self.st.reset();
        self.st.seek(ahead, speed);
    }

    /// Multiplies the pitch by `factor` from the next frames on.
    pub fn set_transpose(&mut self, factor: f64) {
        self.st.set_transpose_factor(factor as f32, None);
    }

    /// Feeds `input` (interleaved stereo) and writes `left.len()` frames out.
    pub fn process(&mut self, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if self.out.len() < n * 2 {
            self.out.resize(n * 2, 0.0);
        }
        let out = &mut self.out[..n * 2];
        self.st.process(&input[..input.len() / 2 * 2], &mut *out);
        for i in 0..n {
            left[i] = out[2 * i];
            right[i] = out[2 * i + 1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine(hz: f64, secs: f64, rate: u32) -> Vec<f32> {
        (0..(secs * f64::from(rate)) as usize).map(|i| ((2.0 * PI * hz * i as f64 / f64::from(rate)).sin() * 0.5) as f32).collect()
    }

    /// Frequency from the rising zero crossings of `x`.
    fn freq(x: &[f32], rate: u32) -> f64 {
        let ups: Vec<usize> = x.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
        let (a, b) = (ups[0], ups[ups.len() - 1]);
        (ups.len() - 1) as f64 * f64::from(rate) / (b - a) as f64
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt()
    }

    #[test]
    fn whole_changes_the_length_and_keeps_the_pitch_and_level() {
        let rate = 48000;
        let s = sine(440.0, 1.0, rate);
        let (l, r) = whole(&s, &s, rate, 96000, 1.0);
        assert_eq!((l.len(), r.len()), (96000, 96000));
        let mid = &l[24000..72000];
        assert!((freq(mid, rate) - 440.0).abs() < 2.0, "{}", freq(mid, rate));
        let db = 20.0 * (rms(mid) / rms(&s[12000..36000])).log10();
        assert!(db.abs() < 1.5, "level {db} dB");
    }

    #[test]
    fn whole_shifts_the_pitch_at_the_same_length() {
        let rate = 44100;
        let s = sine(440.0, 1.0, rate);
        let up = 2f64.powf(7.0 / 12.0);
        let (l, _) = whole(&s, &s, rate, s.len(), up);
        assert_eq!(l.len(), s.len());
        let f = freq(&l[11025..33075], rate);
        assert!((f - 440.0 * up).abs() < 3.0, "{f}");
    }

    #[test]
    fn whole_handles_samples_shorter_than_its_latency() {
        let rate = 44100;
        let s = sine(1000.0, 0.05, rate);
        let (l, _) = whole(&s, &s, rate, 2 * s.len(), 1.0);
        assert_eq!(l.len(), 2 * s.len());
        assert!(rms(&l[1000..3000]) > 0.1, "the sound is there");
        let (l, _) = whole(&s, &s, rate, 0, 1.0);
        assert!(l.is_empty());
    }

    #[test]
    fn stream_stretches_while_it_runs() {
        let rate = 48000;
        let s = sine(440.0, 2.0, rate);
        let st: Vec<f32> = s.iter().flat_map(|&v| [v, v]).collect();
        let mut x = Stream::new(rate);
        // Half speed, a fifth up.
        let speed = 0.5;
        x.set_transpose(1.5);
        let lead = x.lead();
        x.start(&st[..lead * 2], speed);
        let (mut at, mut owed) = (lead, 0.0);
        let mut out = Vec::new();
        let (mut l, mut r) = (vec![0.0; 128], vec![0.0; 128]);
        for _ in 0..400 {
            owed += 128.0 * speed;
            let k = owed as usize;
            owed -= k as f64;
            let end = (at + k).min(s.len());
            x.process(&st[at * 2..end * 2], &mut l, &mut r);
            at = end;
            out.extend_from_slice(&l);
        }
        let out = &out[x.delay()..];
        let f = freq(&out[20000..40000], rate);
        assert!((f - 660.0).abs() < 4.0, "{f}");
        assert!(rms(&out[20000..40000]) > 0.25, "level kept");
    }
}
