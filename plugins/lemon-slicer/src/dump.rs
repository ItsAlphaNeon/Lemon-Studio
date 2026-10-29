// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The dump to the piano roll: one note per slice, in slice order, on the slice's key at velocity 100, placed by the
//! loop's beats (Normal) or by the slice's time at the project's tempo (Original length). Positions are in beats; the
//! host rounds them to its ticks.
//!
// spec: plugins/fruity-slicer.md §4, plugins/lemon-slicer.md §4

use os_hostlink::NoteOut;

use crate::state::State;

/// The velocity of dumped notes.
pub const VELOCITY: u8 = 100;

/// The notes for the slices of `st` whose starts are at `beat(start)`; the last ends at `beat(frames)`.
fn notes(st: &State, frames: usize, beat: impl Fn(f64) -> f64) -> Vec<NoteOut> {
    let n = st.slices.len();
    (0..n)
        .filter_map(|i| {
            let start = f64::from(st.slices[i].start).min(frames as f64);
            let end = st.slices.get(i + 1).map_or(frames as f64, |s| f64::from(s.start).min(frames as f64));
            let key = u8::try_from(st.key_of(i)).ok().filter(|&k| k < 128)?;
            Some(NoteOut { start: beat(start), end: beat(end.max(start)), key, velocity: VELOCITY })
        })
        .collect()
}

/// Normal: slice `i` at start / frames × the loop's beats; the notes fill the loop's beats whatever the tempo.
pub fn normal(st: &State, frames: usize) -> Vec<NoteOut> {
    if frames == 0 {
        return Vec::new();
    }
    let beats = st.beats();
    notes(st, frames, |f| f / frames as f64 * beats)
}

/// Original length: slice `i` at its time in seconds at the project's `tempo`, start / rate × tempo / 60 beats.
pub fn original(st: &State, frames: usize, rate: u32, tempo: f64) -> Vec<NoteOut> {
    if frames == 0 || rate == 0 {
        return Vec::new();
    }
    notes(st, frames, |f| f / f64::from(rate) * tempo / 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Slice, H_BEATS};

    fn ticks(n: &[NoteOut], ppq: f64) -> Vec<(i64, i64, u8)> {
        n.iter().map(|n| ((n.start * ppq).round() as i64, (n.end * ppq).round() as i64, n.key)).collect()
    }

    /// The measured Normal dump (fruity-slicer.md §4): a loop of 352 800 frames at 9 beats, PPQ 96.
    #[test]
    fn normal_follows_the_loops_beats() {
        let mut st = State::default();
        st.set_f32(H_BEATS, 9.0);
        st.slices = [0u32, 22050, 44100, 66150, 78000].into_iter().map(Slice::at).collect();
        let t = ticks(&normal(&st, 352_800), 96.0);
        let starts: Vec<i64> = t.iter().map(|t| t.0).collect();
        assert_eq!(starts, [0, 54, 108, 162, 191]);
        assert_eq!(t.last().unwrap().1, 864);
        assert_eq!(t.iter().map(|t| t.2).collect::<Vec<_>>(), [60, 61, 62, 63, 64]);
        assert!(normal(&st, 352_800).iter().all(|n| n.velocity == 100));
        // A slice's own root key is its note's key.
        st.slices[1].root = 36;
        assert_eq!(normal(&st, 352_800)[1].key, 36);
    }

    /// Original length: 22 069 frames at 44.1 kHz and 120 BPM are 96 ticks.
    #[test]
    fn original_length_keeps_the_loops_speed() {
        let mut st = State::default();
        st.slices = vec![Slice::at(0), Slice::at(22069)];
        let t = ticks(&original(&st, 352_800, 44100, 120.0), 96.0);
        assert_eq!(t, [(0, 96, 60), (96, 1536, 61)]);
        assert!(original(&st, 0, 44100, 120.0).is_empty());
    }
}
