// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The playlist's time markers in song playback: where the song's end goes on, the jumps of skips and marker loops,
//! pauses, the punch-in run-on (live, spec: audio/transport.md "Time markers in SONG mode"), and the spans a render
//! plays (start and skip markers, spec: audio/song-structure.md "Time markers").

use os_model::TimeMarker;

/// What a marker does on the tick playback has reached by playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Act {
    /// Go on at this tick (a skip, or the end of a marker loop's section).
    Jump(u64),
    /// Hold on this tick (a Marker pause).
    Pause,
    /// A Start recording marker: from now on the song runs on past its end.
    PunchIn,
}

/// The arrangement's markers as (tick, kind), sorted by tick (markers on one tick keep their order).
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Marks {
    list: Vec<(u64, u8)>,
}

impl Marks {
    pub(super) fn new(markers: &[TimeMarker]) -> Self {
        let mut list: Vec<(u64, u8)> = markers.iter().map(|m| (u64::from(m.position), m.kind)).collect();
        list.sort_by_key(|m| m.0);
        Marks { list }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    fn has(&self, tick: u64, kind: u8) -> bool {
        self.list.iter().any(|&m| m == (tick, kind))
    }

    /// The last marker of `kind` (two of a kind: the later one counts).
    fn last_of(&self, kind: u8) -> Option<u64> {
        self.list.iter().rev().find(|m| m.1 == kind).map(|m| m.0)
    }

    /// The Start marker a song starts at (the later of two).
    pub(super) fn start(&self) -> Option<u64> {
        self.last_of(TimeMarker::START)
    }

    /// Where live playback goes on at the song's end (tick `end`): the Loop marker, else the Start marker, else tick 0.
    pub(super) fn end_target(&self, end: u64) -> u64 {
        self.last_of(TimeMarker::LOOP).filter(|&t| t < end).or(self.start().filter(|&t| t < end)).unwrap_or(0)
    }

    /// The marker after `tick` by position; after the last one, the first.
    fn next_after(&self, tick: u64) -> Option<u64> {
        self.list.iter().map(|m| m.0).find(|&p| p > tick).or(self.list.first().map(|m| m.0))
    }

    /// What acts at the start of tick `tick` of live playback, reached by playing from the tick before it, in a song
    /// that ends at `end`. A marker loop's section runs to the next marker (or the song's end): reaching its end goes
    /// back to the marker loop. A skip goes to the next marker; one at or past the song's end, where the song's end
    /// goes. A skip whose next marker is itself does nothing.
    pub(super) fn live(&self, tick: u64, end: u64) -> Option<Act> {
        if tick == 0 {
            return None;
        }
        // The section playback was in: from the last marker before this tick.
        if let Some(&(from, _)) = self.list.iter().rev().find(|m| m.0 < tick) {
            let closes = self.list.iter().any(|m| m.0 == tick) || (tick == end && !self.list.iter().any(|m| m.0 > from && m.0 < end));
            if closes && from < end && self.has(from, TimeMarker::MARKER_LOOP) {
                return Some(Act::Jump(from));
            }
        }
        if self.has(tick, TimeMarker::SKIP) {
            if let Some(to) = self.next_after(tick).map(|t| if t >= end { self.end_target(end) } else { t }).filter(|&t| t != tick) {
                return Some(Act::Jump(to));
            }
        }
        if self.has(tick, TimeMarker::PAUSE) {
            return Some(Act::Pause);
        }
        if self.has(tick, TimeMarker::PUNCH_IN) {
            return Some(Act::PunchIn);
        }
        None
    }

    /// The first Marker pause after `tick` when no other marker lies between them: where a pause that is coming
    /// starts its fade early, so the hold falls on the marker.
    pub(super) fn pause_ahead(&self, tick: u64) -> Option<u64> {
        let next = self.list.iter().find(|m| m.0 > tick)?.0;
        self.has(next, TimeMarker::PAUSE).then_some(next)
    }

    /// The spans of ticks a render plays, in order, from `start` (the Start marker, or 0) in a song ending at `end`: a
    /// skip ends a span and the next one begins at the marker after it; a skip whose goal is at or past the song's
    /// end ends the song there. Lemon Studio jumps forward only: a skip with its goal before it (the first marker,
    /// after the last one) is passed over, since the render would never end.
    pub(super) fn rendered(&self, start: u64, end: u64) -> Vec<(u64, u64)> {
        let mut spans = Vec::new();
        let (mut from, mut at) = (start, start);
        loop {
            // The next skip after `at` (the tick a span starts on does not act).
            let Some(k) = self.list.iter().find(|m| m.0 > at && m.0 < end && m.1 == TimeMarker::SKIP).map(|m| m.0) else {
                spans.push((from, end));
                return spans;
            };
            match self.next_after(k) {
                Some(to) if to >= end => {
                    spans.push((from, k));
                    return spans;
                }
                Some(to) if to > k => {
                    spans.push((from, k));
                    (from, at) = (to, to);
                }
                _ => at = k,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(list: &[(u32, u8)]) -> Marks {
        Marks::new(&list.iter().map(|&(p, k)| TimeMarker { kind: k, ..TimeMarker::new(p, "") }).collect::<Vec<_>>())
    }

    const END: u64 = 1920;

    /// spec: audio/transport.md "Time markers in SONG mode": the song's end goes on at the Loop marker, else the Start
    /// marker, else 0; with two of a kind the later one counts.
    #[test]
    fn the_song_end_goes_to_loop_then_start_then_zero() {
        assert_eq!(marks(&[]).end_target(END), 0);
        assert_eq!(marks(&[(768, TimeMarker::START)]).end_target(END), 768);
        assert_eq!(marks(&[(1152, TimeMarker::START), (384, TimeMarker::START)]).end_target(END), 1152);
        assert_eq!(marks(&[(384, TimeMarker::LOOP), (1152, TimeMarker::LOOP)]).end_target(END), 1152);
        assert_eq!(marks(&[(384, TimeMarker::START), (1152, TimeMarker::LOOP)]).end_target(END), 1152);
        assert_eq!(marks(&[(384, TimeMarker::PLAIN)]).end_target(END), 0);
    }

    /// A skip goes to the next marker by position, after the last one to the first; past the song's end, where the
    /// song's end goes.
    #[test]
    fn a_skip_goes_to_the_next_marker() {
        let m = marks(&[(1152, TimeMarker::PLAIN), (384, TimeMarker::SKIP)]);
        assert_eq!(m.live(384, END), Some(Act::Jump(1152)));
        assert_eq!(m.live(385, END), None);
        let last = marks(&[(384, TimeMarker::PLAIN), (1152, TimeMarker::SKIP)]);
        assert_eq!(last.live(1152, END), Some(Act::Jump(384)));
        let past = marks(&[(384, TimeMarker::SKIP), (2304, TimeMarker::PLAIN)]);
        assert_eq!(past.live(384, END), Some(Act::Jump(0)));
        let alone = marks(&[(384, TimeMarker::SKIP)]);
        assert_eq!(alone.live(384, END), None, "its next marker is itself");
    }

    /// A marker loop's section ends at the next marker, or at the song's end when it is the last marker.
    #[test]
    fn a_marker_loop_goes_back_at_its_sections_end() {
        let m = marks(&[(768, TimeMarker::MARKER_LOOP), (1152, TimeMarker::PLAIN)]);
        assert_eq!(m.live(1152, END), Some(Act::Jump(768)));
        assert_eq!(m.live(1000, END), None);
        assert_eq!(m.live(END, END), None, "the plain marker's section reaches the end");
        let last = marks(&[(384, TimeMarker::PLAIN), (1152, TimeMarker::MARKER_LOOP)]);
        assert_eq!(last.live(END, END), Some(Act::Jump(1152)));
        assert_eq!(last.live(1152, END), None);
    }

    #[test]
    fn pauses_and_punch_ins_act_on_their_tick() {
        let m = marks(&[(768, TimeMarker::PAUSE), (1152, TimeMarker::PUNCH_IN), (1536, TimeMarker::PUNCH_OUT)]);
        assert_eq!(m.live(768, END), Some(Act::Pause));
        assert_eq!(m.live(1152, END), Some(Act::PunchIn));
        assert_eq!(m.live(1536, END), None);
        assert_eq!(m.pause_ahead(700), Some(768));
        assert_eq!(m.pause_ahead(768), None);
        assert_eq!(marks(&[(700, TimeMarker::PLAIN), (768, TimeMarker::PAUSE)]).pause_ahead(600), None, "another marker comes first");
    }

    /// spec: audio/song-structure.md "Time markers": a render jumps at skips, ends at a skip whose goal is at or past
    /// the song's end, and (Lemon Studio) passes over a skip that would go back.
    #[test]
    fn a_render_plays_spans_between_skips() {
        let m = marks(&[(384, TimeMarker::SKIP), (1152, TimeMarker::PLAIN)]);
        assert_eq!(m.rendered(0, END), vec![(0, 384), (1152, END)]);
        assert_eq!(m.rendered(768, END), vec![(768, END)]);
        let past = marks(&[(384, TimeMarker::SKIP), (2304, TimeMarker::PLAIN)]);
        assert_eq!(past.rendered(0, END), vec![(0, 384)]);
        let back = marks(&[(384, TimeMarker::PLAIN), (1152, TimeMarker::SKIP)]);
        assert_eq!(back.rendered(0, END), vec![(0, END)]);
        let two = marks(&[(192, TimeMarker::SKIP), (576, TimeMarker::SKIP), (768, TimeMarker::PLAIN)]);
        assert_eq!(two.rendered(0, END), vec![(0, 192), (576, END)], "a skip landed on does not act");
    }
}
