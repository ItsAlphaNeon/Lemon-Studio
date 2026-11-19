// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio tracks and instrument tracks: playlist tracks linked to a mixer track or to a channel. The link is the track
//! record's (`os_model::TrackLink`); this module makes and drops links (the mixer's Assign to new Audio track(s) and
//! Unassign from Audio track(s), the rack's Assign to new instrument track, the playlist track menu's Track mode ▸),
//! and keeps clips to their track's kind: an audio clip placed or moved onto an audio track plays through its mixer
//! track, and an instrument track takes no audio clips.
//!
// spec: ui/playlist.md §6c, ui/mixer-track-commands.md §8, ui/channel-rack.md §18, flp/events/playlist.md

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{ChannelKind, ClipSource, TrackLink};

use crate::edit::playlist::TRACKS;
use crate::edit::playlist::{arr_mut, clips, track_info};

/// Undo step names: the original's History entry for linking (spec: ui/one-click-recording.md §1), and Lemon Studio's
/// for the others.
pub const ASSIGN_AUDIO: &str = "Assign to audio track(s)";
pub const UNASSIGN_AUDIO: &str = "Unassign audio track(s)";
pub const ASSIGN_INSTRUMENT: &str = "Assign to instrument track";
pub const TRACK_MODE: &str = "Playlist track mode";

/// The original's message when an audio clip is dropped on an instrument track (it then offers the next empty track).
pub const NOT_FOR_AUDIO: &str = "Instrument tracks are not for audio clips";

/// Playlist track `t`'s link in the current arrangement.
pub fn link(s: &crate::session::Session, t: u16) -> TrackLink {
    track_info(s, t).map_or(TrackLink::NONE, |x| x.link)
}

/// The mixer track audio track `t` plays through.
pub fn audio_mixer(s: &crate::session::Session, t: u16) -> Option<u16> {
    link(s, t).audio_mixer()
}

pub fn is_instrument(s: &crate::session::Session, t: u16) -> bool {
    link(s, t).kind == TrackLink::INSTRUMENT
}

pub fn set_link(s: &mut crate::session::Session, t: u16, l: TrackLink) {
    if let Some(tr) = arr_mut(s).tracks.get_mut(usize::from(t)) {
        tr.link = l;
    }
}

/// The track a new audio or instrument track takes: the first one with no clips, no link and no name (measured: the
/// original skipped a track it had named, one with clips and linked ones).
pub fn new_track(s: &crate::session::Session) -> Option<u16> {
    new_tracks(s).next()
}

/// The tracks new audio or instrument tracks take, one after the other (`new_track`).
pub fn new_tracks(s: &crate::session::Session) -> impl Iterator<Item = u16> + '_ {
    (0..TRACKS).filter(move |&t| {
        let info = track_info(s, t);
        info.is_none_or(|x| x.link.is_none() && x.name.as_deref().is_none_or(str::is_empty)) && !clips(s).iter().any(|c| c.track == t)
    })
}

/// The audio tracks linked to mixer track `m`, top to bottom.
pub fn audio_tracks_of(s: &crate::session::Session, m: u16) -> Vec<u16> {
    (0..TRACKS).filter(|&t| audio_mixer(s, t) == Some(m)).collect()
}

/// The instrument track of channel `ch` (its index), the first when there are several.
pub fn instrument_track_of(s: &crate::session::Session, ch: u16) -> Option<u16> {
    (0..TRACKS).find(|&t| link(s, t).instrument_channel() == Some(ch))
}

/// Mixer tracks Assign to new Audio track(s) can take: inserts not yet linked (the master is left out).
pub fn assignable(s: &crate::session::Session, mixers: &[u16]) -> Vec<u16> {
    mixers.iter().copied().filter(|&m| m >= 1 && m <= crate::edit::mixer::inserts(s) && audio_tracks_of(s, m).is_empty()).collect()
}

/// The mixer track menu's Assign to new Audio track(s) is greyed when none of `mixers` can take one (the original
/// greys it on a linked track).
pub fn can_assign_audio(s: &crate::session::Session, mixers: &[u16]) -> bool {
    !assignable(s, mixers).is_empty()
}

pub fn can_unassign_audio(s: &crate::session::Session, mixers: &[u16]) -> bool {
    mixers.iter().any(|&m| !audio_tracks_of(s, m).is_empty())
}

/// A channel an instrument track can play: a sampler, plugin or layer channel (not an audio or automation clip's).
pub fn instrument_kind(k: ChannelKind) -> bool {
    !matches!(k, ChannelKind::AudioClip | ChannelKind::Automation)
}

/// Rack rows Assign to new instrument track can take: instrument channels without an instrument track.
pub fn instrument_rows(s: &crate::session::Session, rows: &[usize]) -> Vec<usize> {
    rows.iter()
        .copied()
        .filter(|&r| s.project.channels.get(r).is_some_and(|c| instrument_kind(c.kind) && instrument_track_of(s, c.index).is_none()))
        .collect()
}

pub fn can_assign_instrument(s: &crate::session::Session, rows: &[usize]) -> bool {
    !instrument_rows(s, rows).is_empty()
}

/// The playlist track menu's Track mode ▸ (spec: ui/playlist.md §6c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Unassigned,
    /// An audio track on this mixer track.
    Audio(u16),
    /// An audio track on a mixer track added for it.
    NewAudio,
    /// An instrument track of this rack row.
    Instrument(usize),
    /// An instrument track of a new channel playing generator `.0` (an index of the available plugins).
    NewInstrument(usize),
}

/// Whether a clip of `src` may sit on track `t`: an instrument track takes no audio clips.
pub fn fits(s: &crate::session::Session, src: ClipSource, t: u16) -> bool {
    !(is_instrument(s, t) && is_audio_source(s, src))
}

fn is_audio_source(s: &crate::session::Session, src: ClipSource) -> bool {
    matches!(src, ClipSource::Channel(ch) if s.project.channel(ch).is_some_and(|c| c.kind == ChannelKind::AudioClip))
}

/// The track an audio clip dropped on `t` over ticks `a..b` goes to: `t`, or for an instrument track the first track
/// below it that is free over the span and not an instrument track (the original asks first, then does this).
pub fn audio_drop_track(s: &crate::session::Session, t: u16, a: u32, b: u32) -> u16 {
    if !is_instrument(s, t) {
        return t;
    }
    (t + 1..TRACKS)
        .find(|&k| !is_instrument(s, k) && !clips(s).iter().any(|c| c.track == k && c.position < b && c.end() > a))
        .unwrap_or(t)
}

/// Audio clips `idx` (of the current arrangement) on audio tracks: their channels go to the track's mixer track, as
/// the original reroutes a clip's channel when it lands on an audio track (moving it off again leaves the channel
/// where it is).
pub fn route_clips(s: &mut crate::session::Session, idx: &[usize]) {
    let moves = route_moves(s, idx);
    let max = crate::edit::mixer::max_insert(&s.project);
    for (ch, m) in moves {
        if let Some(c) = s.project.get_mut().channels.iter_mut().find(|c| c.index == ch) {
            if c.mixer_track != m && m <= max {
                c.mixer_track = m;
                s.dirty = true;
            }
        }
    }
}

/// What `route_clips` changes: each audio channel (its index) of clips `idx` that lands on an audio track, and that
/// track's mixer track, where it is another one.
pub fn route_moves(s: &crate::session::Session, idx: &[usize]) -> Vec<(u16, u16)> {
    let moves: Vec<(u16, u16)> = idx
        .iter()
        .filter_map(|&i| clips(s).get(i))
        .filter_map(|c| match c.source {
            ClipSource::Channel(ch) if is_audio_source(s, c.source) => audio_mixer(s, c.track).map(|m| (ch, m)),
            _ => None,
        })
        .collect();
    let max = crate::edit::mixer::max_insert(&s.project);
    moves.into_iter().filter(|&(ch, m)| m <= max && s.project.channels.iter().any(|c| c.index == ch && c.mixer_track != m)).collect()
}

/// The playlist track a take recorded on mixer track `m` over ticks `a..b` goes to: `m`'s first audio track when it
/// is free over the span (recording into an audio track), else the first track free over it.
pub fn record_track(s: &crate::session::Session, m: u16, a: u32, b: u32) -> u16 {
    audio_tracks_of(s, m)
        .into_iter()
        .find(|&t| !clips(s).iter().any(|c| c.track == t && c.position < b && c.end() > a))
        .unwrap_or_else(|| crate::edit::playlist::free_track(s, 0, a, b))
}

/// Select ▸ Channels and playlist tracks routed to this track: the playlist tracks linked to `mixers` (audio tracks)
/// or to a channel using them (instrument tracks), top to bottom.
pub fn tracks_routed_to(s: &crate::session::Session, mixers: &[u16]) -> Vec<u16> {
    (0..TRACKS)
        .filter(|&t| {
            let l = link(s, t);
            l.audio_mixer().is_some_and(|m| mixers.contains(&m))
                || l.instrument_channel().and_then(|ch| s.project.channel(ch)).is_some_and(|c| mixers.contains(&c.mixer_track))
        })
        .collect()
}

#[cfg(any(test, feature = "test-util"))]
mod tests {
    use super::*;
    use os_model::{Clip, Id};

    fn clip(track: u16, position: u32, source: ClipSource) -> Clip {
        Clip { id: Id::NONE, position, length: 96, track, source, muted: false, window: None, raw: Clip::TEMPLATE }
    }

}
