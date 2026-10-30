// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The voices: a note plays its key's slice from its start to the next slice's start (or the sample's end), read with
//! linear interpolation at the sample's own speed times the pitch shift, with the attack / decay fades, the declick
//! fades and a short fade when the note ends.
//!
// spec: plugins/lemon-slicer.md §2

use std::sync::Arc;

use os_engine::sample::Sample;

use crate::state::State;

/// Voices at most; the oldest gives way.
pub const VOICES: usize = 32;
/// The fade when a note ends, and the declick fades (s).
const RELEASE_S: f64 = 0.005;
const DECLICK_S: f64 = 0.002;

/// One slice as the voices play it: frames `start..end` of the sample (`next` = where the next slice starts).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: usize,
    pub next: usize,
    pub reversed: bool,
}

/// What the voices play: the sample, its slices and which slice each key plays.
#[derive(Clone, Debug)]
pub struct Kit {
    pub sample: Arc<Sample>,
    pub spans: Vec<Span>,
    pub keys: [Option<u16>; 128],
}

impl Kit {
    pub fn new(sample: Arc<Sample>, st: &State) -> Self {
        let frames = sample.frames();
        let n = st.slices.len();
        let spans = (0..n)
            .map(|i| {
                let start = (st.slices[i].start as usize).min(frames);
                let next = st.slices.get(i + 1).map_or(frames, |s| (s.start as usize).min(frames)).max(start);
                Span { start, next, reversed: st.slices[i].reversed() }
            })
            .collect();
        let mut keys = [None; 128];
        for (k, slot) in keys.iter_mut().enumerate() {
            *slot = st.slice_for(k as i32).map(|i| i as u16);
        }
        Kit { sample, spans, keys }
    }
}

/// The settings a block plays with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    /// Pitch shift, cents.
    pub cents: f64,
    pub attack_ms: f64,
    pub decay_ms: f64,
    pub declick: bool,
    /// PLAY TO END: a slice plays on to the sample's end.
    pub to_end: bool,
}

impl Default for Block {
    fn default() -> Self {
        Block { cents: 0.0, attack_ms: 0.0, decay_ms: 4.0, declick: true, to_end: false }
    }
}

#[derive(Clone, Debug)]
struct Voice {
    key: u8,
    id: Option<i32>,
    slice: usize,
    /// Frames into the slice (sample frames).
    pos: f64,
    gain: f32,
    /// Output samples left of the end fade, and its length.
    release: Option<(u32, u32)>,
    age: u64,
}

pub struct Engine {
    rate: f32,
    voices: Vec<Voice>,
    clock: u64,
    ended: Vec<(u8, Option<i32>)>,
}

impl Engine {
    pub fn new(rate: f32) -> Self {
        Engine { rate: rate.max(1.0), voices: Vec::with_capacity(VOICES), clock: 0, ended: Vec::new() }
    }

    pub fn set_rate(&mut self, rate: f32) {
        self.rate = rate.max(1.0);
    }

    pub fn reset(&mut self) {
        self.voices.clear();
    }

    pub fn has_voices(&self) -> bool {
        !self.voices.is_empty()
    }

    pub fn sounding(&self) -> impl Iterator<Item = u8> + '_ {
        self.voices.iter().filter(|v| v.release.is_none()).map(|v| v.key)
    }

    /// Voices that ended since the last call (key, host voice id).
    pub fn take_ended(&mut self) -> Vec<(u8, Option<i32>)> {
        std::mem::take(&mut self.ended)
    }

    /// Starts a voice for `key`; keys without a slice are silent.
    pub fn note_on(&mut self, kit: Option<&Kit>, key: u8, velocity: f32, id: Option<i32>) {
        let Some(slice) = kit.and_then(|k| k.keys.get(usize::from(key)).copied().flatten()) else { return };
        if self.voices.len() >= VOICES
            && let Some(i) = (0..self.voices.len()).min_by_key(|&i| self.voices[i].age)
        {
            let v = self.voices.remove(i);
            self.ended.push((v.key, v.id));
        }
        self.clock += 1;
        self.voices.push(Voice { key, id, slice: usize::from(slice), pos: 0.0, gain: velocity.clamp(0.0, 1.0), release: None, age: self.clock });
    }

    /// The note ends: its voices fade out.
    pub fn note_off(&mut self, key: u8, id: Option<i32>, b: &Block) {
        let n = ((RELEASE_S + if b.declick { DECLICK_S } else { 0.0 }) * f64::from(self.rate)).round().max(1.0) as u32;
        for v in &mut self.voices {
            if v.key == key && v.release.is_none() && (id.is_none() || v.id.is_none() || v.id == id) {
                v.release = Some((n, n));
            }
        }
    }

    /// Every voice stops at once.
    pub fn all_off(&mut self) {
        for v in self.voices.drain(..) {
            self.ended.push((v.key, v.id));
        }
    }

    /// Adds the voices into `left` / `right`.
    pub fn render(&mut self, kit: Option<&Kit>, left: &mut [f32], right: &mut [f32], b: &Block) {
        let Some(kit) = kit else {
            self.all_off();
            return;
        };
        let s = &kit.sample;
        let sr = f64::from(s.rate.max(1));
        let step = sr / f64::from(self.rate) * (b.cents / 1200.0).exp2();
        let (att, dec) = (b.attack_ms / 1000.0 * sr, b.decay_ms / 1000.0 * sr);
        let click = if b.declick { DECLICK_S * sr } else { 0.0 };
        let frames = s.frames();
        let mut i = 0;
        while i < self.voices.len() {
            let v = &mut self.voices[i];
            let Some(span) = kit.spans.get(v.slice).copied() else {
                v.pos = f64::MAX;
                i += 1;
                continue;
            };
            let end = if b.to_end { frames } else { span.next };
            let len = end.saturating_sub(span.start) as f64;
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                if v.pos >= len - 1.0 {
                    v.pos = len;
                    break;
                }
                let mut g = v.gain;
                let left_of = len - v.pos;
                for (fade, at) in [(att, v.pos), (dec, left_of), (click, v.pos), (click, left_of)] {
                    if fade > 0.0 && at < fade {
                        g *= (at / fade) as f32;
                    }
                }
                if let Some((n, total)) = v.release.as_mut() {
                    if *n == 0 {
                        v.pos = len;
                        break;
                    }
                    g *= *n as f32 / *total as f32;
                    *n -= 1;
                }
                // Reversed: from the slice's last frame back to its first.
                let x = if span.reversed { (end as f64 - 1.0 - v.pos).max(span.start as f64) } else { span.start as f64 + v.pos };
                let k = x.floor() as usize;
                let f = (x - k as f64) as f32;
                let at = |ch: &[f32], j: usize| ch.get(j.min(end.saturating_sub(1))).copied().unwrap_or(0.0);
                let (l0, r0) = (at(&s.left, k), at(&s.right, k));
                let (l1, r1) = (at(&s.left, k + 1), at(&s.right, k + 1));
                *l += g * (l0 + (l1 - l0) * f);
                *r += g * (r0 + (r1 - r0) * f);
                v.pos += step;
            }
            i += 1;
        }
        let ended = &mut self.ended;
        self.voices.retain(|v| {
            let len = kit.spans.get(v.slice).map_or(0.0, |sp| ((if b.to_end { frames } else { sp.next }).saturating_sub(sp.start)) as f64);
            let done = v.pos >= len - 1.0 || v.release.is_some_and(|(n, _)| n == 0);
            if done {
                ended.push((v.key, v.id));
            }
            !done
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Slice;

    /// A sample whose frame `i` is `i / 1000` (left) and `-i / 1000` (right), at 1000 Hz, cut at 0, 100, 300.
    fn kit() -> Kit {
        let left: Vec<f32> = (0..1000).map(|i| i as f32 / 1000.0).collect();
        let right = left.iter().map(|v| -v).collect();
        let mut st = State::default();
        st.slices = vec![Slice::at(0), Slice::at(100), Slice::at(300)];
        st.slices[2].flags = 1;
        Kit::new(Arc::new(Sample { rate: 1000, left, right }), &st)
    }

    fn plain() -> Block {
        Block { cents: 0.0, attack_ms: 0.0, decay_ms: 0.0, declick: false, to_end: false }
    }

    fn play(e: &mut Engine, k: &Kit, n: usize, b: &Block) -> Vec<f32> {
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        e.render(Some(k), &mut l, &mut r, b);
        l
    }

    #[test]
    fn a_key_plays_its_slice_to_the_next_start() {
        let k = kit();
        assert_eq!((k.keys[60], k.keys[61], k.keys[62], k.keys[63]), (Some(0), Some(1), Some(2), None));
        let mut e = Engine::new(1000.0);
        e.note_on(Some(&k), 61, 1.0, None);
        let out = play(&mut e, &k, 250, &plain());
        assert!((out[0] - 0.1).abs() < 1e-6 && (out[10] - 0.11).abs() < 1e-6);
        // Frames 100..300, the last one excepted (the interpolation needs the next frame): 199 output samples.
        assert!(out[198] > 0.29 && out[199] == 0.0, "{} {}", out[198], out[199]);
        assert!(!e.has_voices());
        assert_eq!(e.take_ended(), vec![(61, None)]);
        // Keys without a slice are silent.
        e.note_on(Some(&k), 63, 1.0, None);
        assert!(!e.has_voices());
    }

    #[test]
    fn reversed_slices_play_backwards_and_pitch_changes_the_speed() {
        let k = kit();
        let mut e = Engine::new(1000.0);
        e.note_on(Some(&k), 62, 0.5, None);
        let out = play(&mut e, &k, 4, &plain());
        assert!((out[0] - 0.5 * 0.999).abs() < 1e-6 && (out[1] - 0.5 * 0.998).abs() < 1e-6, "{out:?}");
        let mut e = Engine::new(1000.0);
        e.note_on(Some(&k), 60, 1.0, None);
        let out = play(&mut e, &k, 4, &Block { cents: 1200.0, ..plain() });
        assert!((out[1] - 0.002).abs() < 1e-6 && (out[2] - 0.004).abs() < 1e-6, "an octave up reads two frames a sample");
    }

    #[test]
    fn fades_and_note_ends() {
        let k = kit();
        let mut e = Engine::new(1000.0);
        e.note_on(Some(&k), 61, 1.0, None);
        // Attack 10 ms: half way up after 5 frames.
        let out = play(&mut e, &k, 6, &Block { attack_ms: 10.0, ..plain() });
        assert!((out[5] - 0.105 * 0.5).abs() < 1e-6, "{}", out[5]);
        // The note ends: 5 ms (5 samples at 1 kHz) to silence.
        e.note_off(61, None, &plain());
        let out = play(&mut e, &k, 8, &plain());
        assert!(out[0] > 0.0 && out[5] == 0.0, "{out:?}");
        assert!(!e.has_voices());
        // PLAY TO END: slice 0 plays on past 100.
        e.note_on(Some(&k), 60, 1.0, None);
        let out = play(&mut e, &k, 400, &Block { to_end: true, ..plain() });
        assert!(out[350] > 0.34);
    }

    #[test]
    fn the_oldest_voice_gives_way() {
        let k = kit();
        let mut e = Engine::new(1000.0);
        for _ in 0..VOICES + 1 {
            e.note_on(Some(&k), 60, 1.0, None);
        }
        assert_eq!(e.voices.len(), VOICES);
        assert_eq!(e.take_ended().len(), 1);
    }
}
