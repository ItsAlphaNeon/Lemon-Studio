// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon's transient detector and beat finder, shared by Lemon Slicer's auto slicing and Tesla's auto-slicing and
//! Detect beats: where the hits of a sound start, the zero-cross check, a loop's tempo from its length, and a sound's
//! tempo and beats from its hits. All of it is Lemon Studio's design (the originals' detectors were not measured).
//!
// spec: plugins/lemon-slicer.md §3, plugins/tesla/README.md "Slicing and beats in Tesla 1.4"

/// The auto slicing's sensitivities: Dull, Medium, Sharp (the rise in dB a slice needs).
pub const AUTO_DB: [f32; 3] = [12.0, 8.0, 5.0];
/// The detector's frame (s), floor (dBFS) and the shortest slice (s).
const FRAME_S: f64 = 0.005;
const FLOOR_DB: f32 = -40.0;
const MIN_GAP_S: f64 = 0.05;

/// The frames' levels (dBFS, both channels' mean power) of 5 ms frames, and the frame's length in samples.
fn levels(left: &[f32], right: &[f32], rate: u32) -> (Vec<f32>, usize) {
    let hop = ((f64::from(rate) * FRAME_S).round() as usize).max(1);
    let n = left.len().min(right.len());
    let db = (0..n.div_ceil(hop))
        .map(|f| {
            let (a, b) = (f * hop, ((f + 1) * hop).min(n));
            let e: f64 = (a..b).map(|i| f64::from(left[i]).powi(2) + f64::from(right[i]).powi(2)).sum::<f64>() / (2 * (b - a)).max(1) as f64;
            (10.0 * (e + 1e-12).log10()) as f32
        })
        .collect();
    (db, hop)
}

/// Lemon's transient detector: the level of both channels in 5 ms frames; a slice starts at a frame whose level is
/// `rise_db` above the previous frame's and above −40 dBFS, at least 50 ms after the previous slice, at the frame's
/// first sample that reaches a quarter of the frame's peak. The first slice starts at 0. A mono sound passes its one
/// channel twice.
pub fn detect(left: &[f32], right: &[f32], rate: u32, rise_db: f32) -> Vec<usize> {
    let (db, hop) = levels(left, right, rate);
    let gap = (f64::from(rate) * MIN_GAP_S).round() as usize;
    let n = left.len().min(right.len());
    let mut out = vec![0];
    let mut last = 0usize;
    for f in 1..db.len() {
        let (a, b) = (f * hop, ((f + 1) * hop).min(n));
        if db[f] > FLOOR_DB && db[f] - db[f - 1] >= rise_db && a >= last + gap {
            let mag = |i: usize| left[i].abs().max(right[i].abs());
            let peak = (a..b).map(mag).fold(0.0f32, f32::max);
            let at = (a..b).find(|&i| mag(i) >= peak / 4.0).unwrap_or(a);
            out.push(at);
            last = at;
        }
    }
    out
}

/// The zero-cross check: every start moves to the nearest sign change of the summed channels within 2 ms (the sample
/// where the new sign starts); a start with none nearby stays.
pub fn zero_cross(starts: &mut [usize], left: &[f32], right: &[f32], rate: u32) {
    let n = left.len().min(right.len());
    if n < 2 {
        return;
    }
    let reach = (f64::from(rate) * 0.002).round() as usize;
    let s = |i: usize| left[i] + right[i];
    let crosses = |i: usize| i > 0 && i < n && (s(i - 1) <= 0.0) != (s(i) <= 0.0);
    for st in starts.iter_mut() {
        let at = (*st).min(n - 1);
        if let Some(i) = (0..=reach).find_map(|d| [at + d, at.wrapping_sub(d)].into_iter().find(|&i| i <= at + reach && crosses(i))) {
            *st = i;
        }
    }
}

/// A loop's beats and tempo when nothing else gives them: a whole number of 4/4 bars at 70..180 BPM, the most beats
/// that keep the tempo at most 180; a loop too short for a bar is taken at 120 BPM in whole beats.
pub fn guess_tempo(frames: usize, rate: u32) -> (f32, f32) {
    let secs = frames as f64 / f64::from(rate.max(1));
    if secs <= 0.0 {
        return (4.0, 120.0);
    }
    let mut best = None;
    let mut beats = 4.0;
    while beats * 60.0 / secs <= 180.0 {
        if beats * 60.0 / secs >= 70.0 {
            best = Some(beats);
        }
        beats += 4.0;
    }
    let beats = best.unwrap_or_else(|| (secs * 2.0).round().max(1.0));
    (beats as f32, (beats * 60.0 / secs) as f32)
}

/// A sound's beats: its tempo, the frame of its first beat and a beat's length in frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beats {
    pub bpm: f64,
    pub first: usize,
    pub period: f64,
}

impl Beats {
    /// The beats' frames from the first to the sound's end (`frames` long).
    pub fn frames(&self, frames: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut k = 0.0;
        loop {
            let f = (self.first as f64 + k * self.period).round() as usize;
            if f >= frames || self.period <= 0.0 {
                return out;
            }
            out.push(f);
            k += 1.0;
        }
    }
}

/// The search range and the analysis length of [`find_beats`].
const BPM_LO: f64 = 60.0;
const BPM_HI: f64 = 200.0;
const MAX_SECS: f64 = 30.0;

/// The onset strength of a frame list `o` at fractional frame `x` (straight lines between frames, 0 outside).
fn at(o: &[f32], x: f64) -> f64 {
    if x < 0.0 {
        return 0.0;
    }
    let i = x.floor() as usize;
    let t = x - i as f64;
    let a = o.get(i).copied().unwrap_or(0.0) as f64;
    let b = o.get(i + 1).copied().unwrap_or(0.0) as f64;
    a + (b - a) * t
}

/// Detect beats (Lemon Studio's design):
///
/// 1. **Onset strength**: the rise of each 5 ms frame's level over the previous frame's, in dB, the previous level
///    taken as −40 dBFS when lower (and before the start); 0 when it falls or the frame is under −40 dBFS. Over the
///    first 30 s.
/// 2. **Tempo**: every tempo from 60 to 200 BPM in 0.05 BPM steps scores the sum, over all frames, of the strength
///    times the strength one, two, three and four beats later (fractional frames read between frames), weighted by a
///    bell over the octave around 120 BPM (log₂(bpm / 120) / 0.75, Gaussian), so half and double tempos lose; the
///    best wins.
/// 3. **The first beat**: of the phases across one beat (in frames), the one whose beats gather the most strength;
///    its frame, moved to the start of a detected hit (Medium sensitivity) within 20 ms if there is one.
/// 4. **Loops**: a sound whose first beat is within 30 ms of its start and whose length is within 3 % of a beat of a
///    whole number of beats at that tempo gets the tempo that fits it exactly, with the first beat at 0.
///
/// `None` when the sound has fewer than two hits.
pub fn find_beats(left: &[f32], right: &[f32], rate: u32) -> Option<Beats> {
    let n = left.len().min(right.len());
    let m = n.min((MAX_SECS * f64::from(rate)) as usize);
    let (db, hop) = levels(&left[..m], &right[..m], rate);
    let o: Vec<f32> = (0..db.len())
        .map(|f| {
            let prev = if f > 0 { db[f - 1].max(FLOOR_DB) } else { FLOOR_DB };
            if db[f] > FLOOR_DB { (db[f] - prev).max(0.0) } else { 0.0 }
        })
        .collect();
    let hits = detect(left, right, rate, AUTO_DB[1]);
    // The detector always starts a slice at 0: that one counts only when the sound starts loud.
    let real = hits.len() - 1 + usize::from(db.first().is_some_and(|d| *d > FLOOR_DB));
    if real < 2 {
        return None;
    }
    let fps = f64::from(rate) / hop as f64;
    let strong: Vec<usize> = (0..o.len()).filter(|&f| o[f] > 0.0).collect();
    let score = |bpm: f64| {
        let lag = 60.0 / bpm * fps;
        let mut s = 0.0;
        for &f in &strong {
            for k in 1..=4 {
                s += o[f] as f64 * at(&o, f as f64 + k as f64 * lag);
            }
        }
        let w = (bpm / 120.0).log2() / 0.75;
        s * (-0.5 * w * w).exp()
    };
    let mut best = (0.0, 0.0);
    let mut bpm = BPM_LO;
    while bpm <= BPM_HI + 1e-9 {
        let s = score(bpm);
        if s > best.1 {
            best = (bpm, s);
        }
        bpm += 0.05;
    }
    if best.1 <= 0.0 {
        return None;
    }
    let bpm = best.0;
    let lag = 60.0 / bpm * fps;
    // The phase: whole frames across one beat.
    let mut phase = (0usize, -1.0);
    for p in 0..(lag.ceil() as usize).max(1) {
        let mut s = 0.0;
        let mut x = p as f64;
        while x < o.len() as f64 {
            s += at(&o, x);
            x += lag;
        }
        if s > phase.1 {
            phase = (p, s);
        }
    }
    let mut first = phase.0 * hop;
    let near = (0.02 * f64::from(rate)) as usize;
    if let Some(&h) = hits.iter().filter(|&&h| h.abs_diff(first) <= near).min_by_key(|&&h| h.abs_diff(first)) {
        first = h;
    }
    let period = 60.0 / bpm * f64::from(rate);
    let mut beats = Beats { bpm, first, period };
    // A loop: snapped to fit its length.
    let len_beats = n as f64 / period;
    let whole = len_beats.round();
    if first as f64 <= 0.03 * f64::from(rate) && whole >= 1.0 && (len_beats - whole).abs() <= 0.03 {
        let period = n as f64 / whole;
        beats = Beats { bpm: 60.0 * f64::from(rate) / period, first: 0, period };
    }
    Some(beats)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Short decaying noise bursts ("hits") at `at` (frames), `len` frames of silence around them.
    fn hits(len: usize, at: &[usize], rate: u32) -> Vec<f32> {
        let mut x = vec![0f32; len];
        let mut seed = 0x1234_5678u32;
        let d = (0.04 * f64::from(rate)) as usize;
        for &a in at {
            for i in 0..d.min(len.saturating_sub(a)) {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let r = (seed >> 9) as f32 / (1u32 << 23) as f32 * 2.0 - 1.0;
                x[a + i] += r * 0.5 * (-(i as f32) / (0.008 * rate as f32)).exp();
            }
        }
        x
    }

    #[test]
    fn detect_finds_the_hits() {
        let rate = 44100;
        let at = [0, 11025, 22050, 30000, 30500];
        let x = hits(44100, &at, rate);
        let s = detect(&x, &x, rate, AUTO_DB[1]);
        // The hit 500 frames after another is under the 50 ms gap.
        assert_eq!(s.len(), 4, "{s:?}");
        for (got, want) in s.iter().zip([0, 11025, 22050, 30000]) {
            assert!(got.abs_diff(want) <= 220, "{got} vs {want}");
        }
        let mut z = s.clone();
        zero_cross(&mut z, &x, &x, rate);
        assert!(z.iter().zip(&s).all(|(a, b)| a.abs_diff(*b) <= 88));
    }

    #[test]
    fn guess_tempo_fits_whole_bars() {
        assert_eq!(guess_tempo(88200, 44100), (4.0, 120.0));
        assert_eq!(guess_tempo(0, 44100), (4.0, 120.0));
    }

    #[test]
    fn beats_at_a_tempo_and_a_phase() {
        let rate = 44100;
        let period = 60.0 / 128.0 * rate as f64;
        let start = (0.1 * rate as f64) as usize;
        let at: Vec<usize> = (0..17).map(|k| start + (k as f64 * period).round() as usize).collect();
        let x = hits(at[16] + 5000, &at, rate);
        let b = find_beats(&x, &x, rate).expect("beats");
        assert!((b.bpm - 128.0).abs() < 0.3, "{b:?}");
        assert!(b.first.abs_diff(start) < 220, "{b:?}");
        let f = b.frames(x.len());
        assert_eq!(f.len(), 17);
        assert!(f[16].abs_diff(at[16]) < 600, "{} vs {}", f[16], at[16]);
        // Silence and a single hit: no beats.
        assert!(find_beats(&vec![0.0; 44100], &vec![0.0; 44100], rate).is_none());
        let one = hits(44100, &[2000], rate);
        assert!(find_beats(&one, &one, rate).is_none());
    }

    #[test]
    fn a_loop_snaps_to_its_length() {
        let rate = 44100;
        // Two bars at 100 BPM: 8 beats in 4.8 s, hits on every beat and some off-beats.
        let n = (4.8 * rate as f64) as usize;
        let period = n as f64 / 8.0;
        let mut at: Vec<usize> = (0..8).map(|k| (k as f64 * period).round() as usize).collect();
        at.extend([(1.5 * period) as usize, (5.5 * period) as usize]);
        let x = hits(n, &at, rate);
        let b = find_beats(&x, &x, rate).expect("beats");
        assert_eq!(b.first, 0);
        assert!((b.bpm - 100.0).abs() < 1e-9, "{b:?}");
        assert_eq!(b.frames(n).len(), 8);
    }
}
