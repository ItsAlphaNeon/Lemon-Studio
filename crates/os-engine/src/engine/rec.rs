// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Disk recording into mixer tracks: what an armed track hands to the recorder each tick of playback, taken where its
//! record source says (spec: audio/track-io.md §2.5).

use os_model::MixerTrack;

/// Where a track's recording is taken (event 165).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Point {
    /// Its device input alone, heard or not.
    External,
    /// Its device input with what the channels, the playlist and the routed tracks bring, before its slots.
    WithMixer,
    /// After its effect slots.
    PostFx,
    /// After its equalizer.
    PostEq,
    /// After the fader matrix (volume, pan, separation, swap, polarity).
    PostLevel,
}

impl Point {
    /// The point of record source `source` (Post track takes what leaves the track, as Post level and panning).
    pub fn of(source: u8) -> Point {
        match source {
            0 => Point::External,
            1 => Point::WithMixer,
            2 => Point::PostFx,
            3 => Point::PostEq,
            _ => Point::PostLevel,
        }
    }
}

/// Takes what armed tracks record. The engine calls `push` from the thread that renders the track; a track is only
/// ever pushed by one thread at a time, so an implementation may keep one single-producer ring per track.
pub trait RecordTap: Send + Sync {
    /// `frames`: interleaved stereo, the next part of track `track`'s take, mixed on song tick `tick` (from its start).
    fn push(&self, track: usize, tick: u64, frames: &[f32]);
}

/// A track's recording: where it is taken and the device input it records (numbered as stored; -1 none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Arm {
    pub point: Point,
    pub input: i32,
}

impl Arm {
    /// An armed track records; one that is not, nothing.
    pub fn of(m: &MixerTrack) -> Option<Arm> {
        m.armed.then(|| Arm { point: Point::of(m.rec_source), input: m.audio_in })
    }
}
