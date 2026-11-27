// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Loop recorded audio takes in the playlist (`crate::track_rec`): the tracks a take's passes go on, grouped under the
//! audio track (or the first pass's track), and the passes muted when Blend is off.
// spec: audio/disk-recording.md §1

use os_model::ClipSource;

use crate::edit::playlist::{arr_mut, track_info, TRACKS};
use crate::session::Session;

/// The last track of the group track `t` leads (`t` itself when nothing is grouped under it).
pub fn group_end(s: &Session, t: u16) -> u16 {
    let mut last = t;
    while last + 1 < TRACKS && track_info(s, last + 1).is_some_and(|x| x.grouped) {
        last += 1;
    }
    last
}

/// The track at `at`, grouped with the track above, for a take's pass: used as it is when it is an empty plain track
/// leading no group, else a new track inserted there (the tracks from there move down with their clips). At the last
/// track, that track is used as it is.
pub fn take_track(s: &mut Session, at: u16) -> u16 {
    let at = at.min(TRACKS - 1);
    let empty = !crate::edit::playlist::clips(s).iter().any(|c| c.track == at)
        && crate::track_links::link(s, at).is_none()
        && !track_info(s, at + 1).is_some_and(|x| x.grouped);
    if !empty {
        crate::edit::playlist::insert_track(s, at);
    }
    let arr = arr_mut(s);
    if let Some(tr) = arr.tracks.get_mut(usize::from(at)) {
        tr.grouped = true;
    }
    at
}

/// Mutes the clips of the audio clip channels `channels` (a take's passes, with Blend off).
pub fn mute_takes(s: &mut Session, channels: &[u16]) {
    if channels.is_empty() {
        return;
    }
    for c in arr_mut(s).clips.iter_mut() {
        if matches!(c.source, ClipSource::Channel(ch) if channels.contains(&ch)) {
            c.muted = true;
        }
    }
}

/// Names track `t` after the mixer track it recorded (Auto name playlist tracks), unless it has a name.
pub fn name_take_track(s: &mut Session, t: u16, name: &str) {
    let arr = arr_mut(s);
    if let Some(tr) = arr.tracks.get_mut(usize::from(t)) {
        if tr.name.as_deref().is_none_or(str::is_empty) {
            tr.name = Some(name.to_string());
        }
    }
}
