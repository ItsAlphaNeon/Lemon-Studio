// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The one sample Tesla holds: 1 or 2 channels at any rate, kept as 16-bit int or 32-bit float, with its markers,
//! regions and loop. Positions are in sample frames from 0; ranges are inclusive at both ends, as Edison saves them.
//!
// spec: plugins/edison/README.md "Words", plugins/edison/wav-format.md, plugins/tesla/README.md "Version 1"

/// How the sample is kept (and saved): Format > Precision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    Int16,
    #[default]
    Float32,
}

impl Format {
    pub fn bits(self) -> u16 {
        match self {
            Format::Int16 => 16,
            Format::Float32 => 32,
        }
    }
}

/// A marker (a named position) or, with a length, a region (a named range).
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    /// Counts from 1 in the order the markers and regions were made (the WAV's cue ID).
    pub id: u32,
    pub name: String,
    pub pos: usize,
    /// A region's length in frames; `None` for a marker.
    pub len: Option<usize>,
}

impl Cue {
    pub fn is_region(&self) -> bool {
        self.len.is_some()
    }

    /// The last frame of a region (its position for a marker).
    pub fn end(&self) -> usize {
        self.pos + self.len.unwrap_or(1).max(1) - 1
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub rate: u32,
    /// One vector per channel (1 or 2), all the same length.
    pub ch: Vec<Vec<f32>>,
    pub format: Format,
    /// In creation order (their IDs rise).
    pub cues: Vec<Cue>,
    /// The loop, first and last frame.
    pub looped: Option<(usize, usize)>,
    /// MIDI unity note (Sample properties' Middle note; C5 = 60).
    pub note: u8,
    /// Tempo in BPM, 0 when none is set.
    pub tempo: f32,
}

impl Default for Sample {
    /// An empty Edison: stereo 32-bit float at 44.1 kHz, no audio.
    fn default() -> Self {
        Sample::new(44100, 2, Format::Float32)
    }
}

impl Sample {
    pub fn new(rate: u32, channels: usize, format: Format) -> Sample {
        Sample { rate, ch: vec![Vec::new(); channels.clamp(1, 2)], format, cues: Vec::new(), looped: None, note: 60, tempo: 0.0 }
    }

    pub fn from_channels(rate: u32, ch: Vec<Vec<f32>>, format: Format) -> Sample {
        let mut s = Sample::new(rate, ch.len(), format);
        let n = ch.iter().map(Vec::len).min().unwrap_or(0);
        s.ch = ch.into_iter().take(2).map(|mut c| {
            c.truncate(n);
            c
        }).collect();
        if s.ch.is_empty() {
            s.ch.push(Vec::new());
        }
        s
    }

    pub fn frames(&self) -> usize {
        self.ch.first().map_or(0, Vec::len)
    }

    pub fn channels(&self) -> usize {
        self.ch.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames() == 0
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }

    /// The length in beats, as `acid` stores it: at the sample's tempo, or at 120 BPM with none set, rounded down.
    pub fn beats(&self) -> u32 {
        let bpm = if self.tempo > 0.0 { self.tempo as f64 } else { 120.0 };
        (self.seconds() * bpm / 60.0 + 1e-9).floor() as u32
    }

    /// The ID the next marker or region gets.
    pub fn next_id(&self) -> u32 {
        self.cues.iter().map(|c| c.id).max().unwrap_or(0) + 1
    }

    pub fn add_cue(&mut self, name: &str, pos: usize, len: Option<usize>) -> u32 {
        let id = self.next_id();
        self.cues.push(Cue { id, name: name.to_string(), pos, len });
        id
    }

    /// The markers and regions by position (ties keep creation order).
    pub fn cues_by_pos(&self) -> Vec<&Cue> {
        let mut v: Vec<&Cue> = self.cues.iter().collect();
        v.sort_by_key(|c| (c.pos, c.id));
        v
    }

    /// A frame's value in channel `c` (the left channel for a mono sample's right).
    pub fn at(&self, c: usize, i: usize) -> f32 {
        let ch = &self.ch[c.min(self.ch.len() - 1)];
        ch.get(i).copied().unwrap_or(0.0)
    }

    /// The largest absolute value over `a..=b` in every channel.
    pub fn peak(&self, a: usize, b: usize) -> f32 {
        let n = self.frames();
        if n == 0 || a >= n {
            return 0.0;
        }
        let b = b.min(n - 1);
        self.ch.iter().flat_map(|c| c[a..=b].iter()).fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Rounds every value to 16 bits (Format > 16Bit int keeps what a 16-bit file would).
    pub fn quantize(&mut self) {
        if self.format == Format::Int16 {
            for c in &mut self.ch {
                for v in c.iter_mut() {
                    *v = q16(*v);
                }
            }
        }
    }

    /// Positions after frame `at` move by `delta` frames (an insert, or a delete when negative). Markers inside a
    /// deleted span go to its start, regions shrink, the loop is clipped; a region that no longer has frames is kept as
    /// a marker at the cut.
    pub fn shift_after(&mut self, at: usize, delta: isize) {
        let mv = |p: usize| -> usize {
            if p < at {
                p
            } else if delta >= 0 {
                p + delta as usize
            } else {
                let d = (-delta) as usize;
                if p < at + d { at } else { p - d }
            }
        };
        for c in &mut self.cues {
            match c.len {
                None => c.pos = mv(c.pos),
                Some(len) => {
                    let (a, b) = (c.pos, c.pos + len.max(1) - 1);
                    let (na, nb) = if delta >= 0 && a < at && b >= at { (a, b + delta as usize) } else { (mv(a), mv(b + 1).saturating_sub(1).max(mv(a))) };
                    c.pos = na;
                    let l = nb + 1 - na;
                    c.len = Some(l.max(1));
                }
            }
        }
        if let Some((a, b)) = self.looped {
            let na = mv(a);
            let nb = if delta >= 0 && a < at && b >= at { b + delta as usize } else { mv(b + 1).saturating_sub(1) };
            self.looped = if nb >= na { Some((na, nb)) } else { None };
        }
        self.clip_marks();
    }

    /// Keeps markers, regions and the loop inside the sample.
    pub fn clip_marks(&mut self) {
        let n = self.frames();
        if n == 0 {
            self.looped = None;
            for c in &mut self.cues {
                c.pos = 0;
                if c.len.is_some() {
                    c.len = Some(1);
                }
            }
            return;
        }
        for c in &mut self.cues {
            c.pos = c.pos.min(n - 1);
            if let Some(l) = c.len {
                c.len = Some(l.clamp(1, n - c.pos));
            }
        }
        if let Some((a, b)) = self.looped {
            self.looped = if a < n { Some((a, b.min(n - 1))) } else { None };
        }
    }
}

/// A value as 16-bit int keeps it.
pub fn q16(v: f32) -> f32 {
    (v * 32768.0).round().clamp(-32768.0, 32767.0) / 32768.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: usize) -> Sample {
        Sample::from_channels(44100, vec![vec![0.0; n], vec![0.0; n]], Format::Float32)
    }

    #[test]
    fn beats_and_ids() {
        let mut a = s(44100);
        assert_eq!(a.beats(), 2);
        a.tempo = 90.0;
        assert_eq!(a.beats(), 1);
        assert_eq!(a.add_cue("mk1", 0, None), 1);
        assert_eq!(a.add_cue("rg1", 7528, Some(11855)), 2);
        assert_eq!(a.cues[1].end(), 19382);
        assert_eq!(a.next_id(), 3);
    }

    #[test]
    fn edits_move_marks() {
        let mut a = s(1000);
        a.add_cue("m", 500, None);
        a.add_cue("r", 400, Some(200));
        a.looped = Some((0, 999));
        // Insert 100 frames at 450: the marker moves, the region grows (the insert is inside it), the loop grows.
        a.ch.iter_mut().for_each(|c| {
            c.splice(450..450, vec![0.0; 100]);
        });
        a.shift_after(450, 100);
        assert_eq!(a.cues[0].pos, 600);
        assert_eq!((a.cues[1].pos, a.cues[1].len), (400, Some(300)));
        assert_eq!(a.looped, Some((0, 1099)));
        // Delete 450..550: back as before.
        a.ch.iter_mut().for_each(|c| drop(c.drain(450..550)));
        a.shift_after(450, -100);
        assert_eq!(a.cues[0].pos, 500);
        assert_eq!((a.cues[1].pos, a.cues[1].len), (400, Some(200)));
        assert_eq!(a.looped, Some((0, 999)));
        // Delete the whole region: it stays as a 1-frame region at the cut.
        a.ch.iter_mut().for_each(|c| drop(c.drain(300..700)));
        a.shift_after(300, -400);
        assert_eq!((a.cues[1].pos, a.cues[1].len), (300, Some(1)));
        assert_eq!(a.cues[0].pos, 300);
    }

    #[test]
    fn sixteen_bit_rounding() {
        assert_eq!(q16(1.0), 32767.0 / 32768.0);
        assert_eq!(q16(-1.0), -1.0);
        assert_eq!(q16(0.5), 0.5);
    }
}
