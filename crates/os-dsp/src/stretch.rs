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
    exact(&mut st, &input, &mut out);
    let l = out.iter().step_by(2).take(frames).copied().collect();
    let r = out.iter().skip(1).step_by(2).take(frames).copied().collect();
    (l, r)
}

/// Signalsmith Stretch's `exact` (the whole of `input` into the whole of `out`, latency taken out; interleaved
/// stereo; `out` at least twice the output latency), step by step as the library does it, but with the first input
/// latency's worth of input it seeks on zero-padded: the library reads that many frames whatever the input's length,
/// so a sample shorter than it (a one-shot of a few ms) was read past its end, heard as garbage on every hit or a
/// crash.
fn exact(st: &mut Stretch, input: &[f32], out: &mut [f32]) {
    let (n, frames) = (input.len() / 2, out.len() / 2);
    let (lat_in, lat_out) = (st.input_latency(), st.output_latency());
    debug_assert!(frames >= 2 * lat_out, "the output is at least twice the latency");
    // Start positioned on the centre of the input.
    let mut head = vec![0.0f32; lat_in * 2];
    let m = n.min(lat_in) * 2;
    head[..m].copy_from_slice(&input[..m]);
    st.seek(&head, n as f64 / frames as f64);
    // Then the input after that, `n` frames of it, silence past its end.
    let mut rest = vec![0.0f32; n * 2];
    if n > lat_in {
        rest[..(n - lat_in) * 2].copy_from_slice(&input[lat_in * 2..]);
    }
    st.process(&rest, &mut *out);
    // Fold the first bit of the output back onto itself, then shift everything along by the output latency.
    for i in 0..(frames - lat_out).min(lat_out) {
        for c in 0..2 {
            out[(i + lat_out) * 2 + c] -= out[(lat_out - 1 - i) * 2 + c];
        }
    }
    out.copy_within(lat_out * 2.., 0);
    // The final chunk.
    st.flush(&mut out[(frames - lat_out) * 2..]);
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

    /// `exact` gives the library's own result wherever that one is safe (an input at least its input latency long).
    #[test]
    fn exact_matches_the_library_on_long_inputs() {
        for (rate, n, frames, t) in [(44100u32, 44100usize, 88200usize, 1.0f32), (48000, 30000, 12000, 1.3), (22050, 9000, 9000, 0.8)] {
            let s = sine(330.0, n as f64 / f64::from(rate), rate);
            let input: Vec<f32> = s.iter().flat_map(|&v| [v, -v]).collect();
            let run = |ours: bool| {
                let mut st = Stretch::preset_default(2, rate);
                st.set_transpose_factor(t, None);
                assert!(n >= st.input_latency());
                let mut out = vec![0.0f32; frames * 2];
                if ours {
                    exact(&mut st, &input, &mut out);
                } else {
                    assert!(st.exact(&input, &mut out));
                }
                out
            };
            // The library is not bit-exact from run to run, and differs more on some platforms (a single sample up to
            // about 0.006 on Linux): the difference's RMS against the signal's.
            let (a, b) = (run(true), run(false));
            let rms = |v: &mut dyn Iterator<Item = f32>| {
                let (sum, n) = v.fold((0.0f64, 0usize), |(s, n), x| (s + f64::from(x) * f64::from(x), n + 1));
                (sum / n.max(1) as f64).sqrt()
            };
            let d = rms(&mut a.iter().zip(&b).map(|(p, q)| p - q)) / rms(&mut b.iter().copied());
            assert!(d < 0.01, "rate {rate}, {n} -> {frames}: relative RMS difference {d}");
        }
    }

    /// A sample shorter than the stretcher's input latency, stretched long (a one-shot of a few ms fitted to a beat):
    /// the stretcher used to read past the sample's end (garbage on every hit, or a crash). Many times over, so a read
    /// past the end would show.
    #[test]
    fn whole_handles_samples_shorter_than_its_input_latency() {
        for rate in [22050, 44100, 48000] {
            for n in [1usize, 8, 100, 480, 2000] {
                let s: Vec<f32> = sine(220.0, n as f64 / f64::from(rate), rate).into_iter().chain(std::iter::repeat(0.25)).take(n).collect();
                for frames in [n, 4 * n, 22150, 88600] {
                    let (l, r) = whole(&s, &s, rate, frames, 1.0);
                    assert_eq!((l.len(), r.len()), (frames, frames));
                    let peak = l.iter().chain(&r).fold(0.0f32, |m, v| if v.is_finite() { m.max(v.abs()) } else { f32::INFINITY });
                    assert!(peak < 2.0, "rate {rate}, {n} frames to {frames}: peak {peak}");
                }
            }
        }
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
