// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon's pitch tracker, for Tesla's Detect pitch regions: the YIN method (de Cheveigné and Kawahara, 2002) over
//! 46 ms frames, and the notes it finds held long enough. Lemon Studio's design (the original's detector was not
//! measured).
//!
// spec: plugins/tesla/README.md "Declicker, declipper modes, envelope tools and pitch regions in Tesla 1.6"

/// The frame (s) and hop (s) of the tracker; the lowest pitch it reports is the rate over half the frame (43 Hz).
const FRAME_S: f64 = 0.0464;
const HOP_S: f64 = 0.0232;
/// YIN's threshold on the cumulative-mean-normalized difference.
const YIN_THRESHOLD: f64 = 0.15;
/// Frames quieter than this (RMS, dBFS) have no pitch.
const QUIET_DB: f64 = -50.0;
/// The shortest note kept, in frames (about 70 ms).
const MIN_FRAMES: usize = 3;

/// The pitch of `x` (Hz) by YIN over its whole length (two periods of the lowest pitch must fit), `None` when it is
/// quiet or has no clear period.
pub fn yin(x: &[f32], rate: u32) -> Option<f64> {
    let w = x.len() / 2;
    if w < 4 {
        return None;
    }
    let rms = (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64).sqrt();
    if 20.0 * rms.max(1e-12).log10() < QUIET_DB {
        return None;
    }
    let d: Vec<f64> = (0..w)
        .map(|tau| (0..w).map(|j| (f64::from(x[j]) - f64::from(x[j + tau])).powi(2)).sum())
        .collect();
    // Cumulative mean normalized difference.
    let mut cm = vec![1.0f64; w];
    let mut sum = 0.0;
    for tau in 1..w {
        sum += d[tau];
        cm[tau] = if sum > 0.0 { d[tau] * tau as f64 / sum } else { 1.0 };
    }
    let mut tau = (2..w).find(|&t| cm[t] < YIN_THRESHOLD)?;
    while tau + 1 < w && cm[tau + 1] < cm[tau] {
        tau += 1;
    }
    // Parabolic interpolation around the minimum.
    let (a, b, c) = (cm[tau - 1], cm[tau], cm.get(tau + 1).copied().unwrap_or(cm[tau]));
    let den = a - 2.0 * b + c;
    let shift = if den.abs() > 1e-12 { (0.5 * (a - c) / den).clamp(-1.0, 1.0) } else { 0.0 };
    Some(f64::from(rate) / (tau as f64 + shift))
}

/// The MIDI note nearest `hz` (A4 at 440 Hz = 69).
pub fn note_of(hz: f64) -> i32 {
    (69.0 + 12.0 * (hz / 440.0).log2()).round() as i32
}

/// A note's name with FL's octave numbering (note 60 = "C5").
pub fn note_name(note: i32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[note.rem_euclid(12) as usize], note.div_euclid(12))
}

/// The notes of a sound (the mean of `left` and `right`): runs of frames whose pitch rounds to the same note, at least
/// three frames (about 70 ms) long, as (first frame, frames, note). A run starts at its first frame's start and ends at
/// its last frame's hop.
pub fn regions(left: &[f32], right: &[f32], rate: u32) -> Vec<(usize, usize, i32)> {
    let n = left.len().min(right.len());
    let frame = ((f64::from(rate) * FRAME_S).round() as usize).max(8);
    let hop = ((f64::from(rate) * HOP_S).round() as usize).max(1);
    let mono: Vec<f32> = (0..n).map(|i| 0.5 * (left[i] + right[i])).collect();
    let notes: Vec<Option<i32>> = (0..)
        .map(|k| k * hop)
        .take_while(|&a| a + frame <= n)
        .map(|a| yin(&mono[a..a + frame], rate).map(note_of))
        .collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k < notes.len() {
        let Some(m) = notes[k] else {
            k += 1;
            continue;
        };
        let mut j = k;
        while j < notes.len() && notes[j] == Some(m) {
            j += 1;
        }
        if j - k >= MIN_FRAMES {
            let a = k * hop;
            let b = ((j - 1) * hop + frame).min(n);
            out.push((a, b - a, m));
        }
        k = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (0.5 * (std::f64::consts::TAU * hz * i as f64 / 44100.0).sin()) as f32).collect()
    }

    #[test]
    fn yin_finds_a_sines_pitch() {
        let p = yin(&sine(220.0, 2048), 44100).unwrap();
        assert!((p - 220.0).abs() < 1.0, "{p}");
        assert_eq!(note_of(p), 57);
        assert_eq!(note_name(57), "A4");
        assert_eq!(note_name(60), "C5");
        assert_eq!(yin(&[0.0; 2048], 44100), None);
    }

    #[test]
    fn regions_follow_the_notes() {
        // A4 for half a second, silence, then E5 (76) for half a second.
        let mut x = sine(440.0, 22050);
        x.extend(vec![0.0; 11025]);
        x.extend(sine(659.255, 22050));
        let r = regions(&x, &x, 44100);
        assert_eq!(r.iter().map(|g| g.2).collect::<Vec<_>>(), vec![69, 76]);
        assert!(r[0].0 == 0 && (r[0].1 as i64 - 22050).abs() < 2100, "{r:?}");
        assert!((r[1].0 as i64 - 33075).abs() < 2100, "{r:?}");
    }
}
