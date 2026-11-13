// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The session's half of `crate::piano`: what reads and changes the project, apart from the interface (specs
//! `roadmap/control-plane.md`, part 2, step 6).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#![allow(unused_imports)]

use os_model::{ControlPoint, Note, Pattern};
use os_ui::Rect;
use crate::app::State;
use crate::piano::*;

/// Where the current pattern plays in the song: each clip of it that sounds (not muted, on a track that is on), as
/// (song tick of the clip's start, pattern tick there, length), in song order. A clip that plays its whole pattern
/// (no window) lasts the pattern's length.
pub fn song_spans(s: &crate::session::Session) -> Vec<(u32, u32, u32)> {
    let whole = pattern_length(s, s.pattern());
    let track_on = |t: u16| s.project.arrangement().and_then(|a| a.tracks.get(usize::from(t))).is_none_or(|t| t.enabled);
    let mut v: Vec<(u32, u32, u32)> = crate::edit::playlist::clips(s)
        .iter()
        .filter(|c| c.source == os_model::ClipSource::Pattern(s.pattern()) && !c.muted && track_on(c.track))
        .map(|c| match c.window {
            Some(w) => (c.position, w.0.max(0.0).round() as u32, c.length),
            None => (c.position, 0, whole),
        })
        .collect();
    v.sort_unstable();
    v
}

/// The pattern tick at song tick `t`, when a clip of the current pattern plays there (the latest-starting one).
pub fn pattern_tick_in_song(s: &crate::session::Session, t: f64) -> Option<f64> {
    song_spans(s)
        .into_iter()
        .rev()
        .find(|&(at, _, len)| t >= f64::from(at) && t < f64::from(at) + f64::from(len))
        .map(|(at, from, _)| f64::from(from) + (t - f64::from(at)))
}

/// The song tick where pattern tick `t` plays: in the clip that plays at song tick `near` if it shows `t`, else in the
/// first clip after `near` that does, else in the first one. None when no clip of the pattern shows `t`.
pub fn song_tick_of(s: &crate::session::Session, t: u32, near: u32) -> Option<u32> {
    let shows: Vec<(u32, u32, u32)> = song_spans(s).into_iter().filter(|&(_, from, len)| t >= from && t < from + len).collect();
    let place = |&(at, from, _): &(u32, u32, u32)| at + (t - from);
    shows
        .iter()
        .find(|&&(at, _, len)| near >= at && near < at + len)
        .or_else(|| shows.iter().find(|&&(at, _, _)| at >= near))
        .or_else(|| shows.first())
        .map(place)
}

/// What the Playback tool's line sounds on channel `ch` moving from tick `from` to `to` (the same tick for a press): the
/// notes it is on now and those whose start it just passed, as (key, velocity), a key once at its loudest note's velocity.
pub fn scrub_keys(s: &crate::session::Session, ch: u16, from: f32, to: f32) -> Vec<(u16, u8)> {
    let (a, b) = (from.min(to), from.max(to));
    let mut keys: Vec<(u16, u8)> = notes(s)
        .iter()
        .filter(|n| n.channel == ch && !n.muted())
        .filter(|n| {
            let p = n.position as f32;
            let on = p <= to && to < p + shown_len(s, n) as f32;
            on || (p > a && p <= b)
        })
        .map(|n| (n.key, n.velocity))
        .collect();
    keys.sort_unstable_by_key(|&(k, v)| (k, std::cmp::Reverse(v)));
    keys.dedup_by_key(|n| n.0);
    keys
}

/// Length used for drawing: a plain step (length 0) is shown one step long (Lemon Studio choice).
pub fn shown_len(s: &crate::session::Session, n: &Note) -> u32 {
    if n.length == 0 { u32::from(s.project.ppq.max(4)) / 4 } else { n.length }
}

/// Pattern `n`'s length in ticks as the engine loops it: the rack's explicit length, else its last note's end rounded
/// up to a bar line of its own time signature (`Project::pattern_length`, spec: audio/song-structure.md).
pub fn pattern_length(s: &crate::session::Session, n: u16) -> u32 {
    s.project.pattern_length(n)
}

/// The current pattern, created if it has no data yet.
pub fn pattern_mut(s: &mut crate::session::Session) -> &mut Pattern {
    let number = s.pattern();
    match s.project.patterns.iter().position(|p| p.number == number) {
        Some(i) => &mut s.project.get_mut().patterns[i],
        None => {
            s.project.get_mut().patterns.push(Pattern { number, ..Pattern::default() });
            s.project.get_mut().patterns.last_mut().expect("pushed")
        }
    }
}

pub fn notes(s: &crate::session::Session) -> &[Note] {
    s.project.patterns.iter().find(|p| p.number == s.pattern()).map_or(&[], |p| &p.notes)
}

/// The current pattern's channel control points.
pub fn controls(s: &crate::session::Session) -> &[ControlPoint] {
    s.project.patterns.iter().find(|p| p.number == s.pattern()).map_or(&[], |p| &p.controls)
}

/// Set (or replace) the control point of `channel`/`param` at `tick`, keeping the list ordered by
/// position (spec: flp/events/patterns.md, "Control points").
pub fn set_control(s: &mut crate::session::Session, channel: u16, param: u16, tick: u32, value: i32) {
    let point = ControlPoint::new(tick, channel, param, value);
    let p = pattern_mut(s);
    match p.controls.iter_mut().find(|c| c.position == tick && c.kind == point.kind) {
        Some(c) => c.value = value,
        None => {
            let at = p.controls.partition_point(|c| c.position <= tick);
            p.controls.insert(at, point);
        }
    }
    s.dirty = true;
}

/// Delete the lane's points whose position lies in `from..=to`; returns how many went.
pub fn erase_controls(s: &mut crate::session::Session, channel: u16, param: u16, from: u32, to: u32) -> usize {
    let kind = ControlPoint::new(0, channel, param, 0).kind;
    let p = pattern_mut(s);
    let before = p.controls.len();
    p.controls.retain(|c| !(c.kind == kind && (from..=to).contains(&c.position)));
    let gone = before - p.controls.len();
    s.dirty = true;
    gone
}

/// Remember the current pattern's notes and control points for undo: a step of the project's undo history
/// (`crate::history`).
pub fn snapshot(s: &mut crate::session::Session) {
    snapshot_as(s, "Piano roll edit");
}

/// `snapshot`, the step named `name` (as the browser's History lists it; spec: ui/settings-choices.md "Undo history:
/// the four checks (2026-10-16)").
pub fn snapshot_as(s: &mut crate::session::Session, name: &'static str) {
    s.dirty = true;
    let snap = crate::history::Snap::pattern(s, s.pattern());
    crate::history::record(s, name, snap);
}

/// The end of the current pattern: its length rounded up to whole bars of its own grid, at least one bar. Ctrl+A's
/// time selection, Ctrl+L's last notes and Ctrl+Shift+Left / Right's rotation stop there (spec: ui/piano-keys.md §4).
pub fn pattern_end(s: &crate::session::Session) -> u32 {
    s.project.pattern_bar_end(s.pattern(), pattern_length(s, s.pattern()))
}

/// Where the bar holding `tick` starts, on the current pattern's own bar grid.
pub fn bar_start(s: &crate::session::Session, tick: u32) -> u32 {
    let regions = s.project.pattern_bars(s.pattern());
    let (start, bar, _) = regions.iter().rev().find(|r| r.0 <= tick).copied().unwrap_or(regions[0]);
    start + (tick.saturating_sub(start) / bar.max(1)) * bar.max(1)
}
