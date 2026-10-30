// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Performance mode (spec: ui/performance-mode.md): the arranger's time before the Start marker is a grid of clips
//! launched while the song plays. A launched clip plays as copies of itself placed on its track from the time it
//! starts (`Live::passes`); `rack::play_project` adds them to what the engine plays (`apply`), so playback needs
//! nothing of its own. The zone itself is never played as part of the song.

use std::collections::BTreeMap;

use os_model::{Clip, PerfSettings, Project, TimeMarker};

use crate::app::State;
use crate::playlist::{arr_mut, clips, markers, start_marker, to_src, track_info, window};

/// How far ahead of its start a launched clip's copies are placed, in bars of 4 beats (Lemon Studio's design: the
/// engine plays what it is given; a trigger, a stop or an edit places them again).
const AHEAD_BARS: u32 = 256;

/// What the zone is playing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Perform {
    /// Each playing track's (0-based) clip.
    pub live: BTreeMap<u16, Live>,
    /// Round robin (§2): the next clip of each slot (track, column).
    pub turn: BTreeMap<(u16, usize), usize>,
    /// The track whose slot is held with the mouse (Press Hold & stop, Hold & motion).
    pub held: Option<u16>,
}

/// A track's launched clip.
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    /// The copies placed, in time order: one per pass, the first starting `Position sync` into its clip.
    pub passes: Vec<Clip>,
    /// The column of the first pass (the slot triggered).
    pub column: usize,
    /// Silent from this tick on (a stop at a sync point); `None` = until the copies end.
    pub end: Option<u32>,
    /// Triggered while stopped: it starts with playback (§3).
    pub armed: bool,
}

impl Live {
    pub fn start(&self) -> u32 {
        self.passes.first().map_or(0, |c| c.position)
    }

    /// Where it falls silent.
    pub fn until(&self) -> u32 {
        let last = self.passes.last().map_or(0, Clip::end);
        self.end.map_or(last, |e| e.min(last))
    }

    /// The pass sounding at `tick`, if any.
    fn pass_at(&self, tick: u32) -> Option<&Clip> {
        self.passes.iter().find(|c| c.position <= tick && tick < c.end()).filter(|_| tick < self.until())
    }
}

pub fn on(s: &State) -> bool {
    s.project.performance == Some(true)
}

/// The performance zone's end (the Start marker), while performance mode is on (§2).
pub fn zone(s: &State) -> Option<u32> {
    on(s).then(|| start_marker(s)).flatten().filter(|&z| z > 0)
}

fn ppq(s: &State) -> u32 {
    u32::from(s.project.ppq.max(4))
}

/// The zone's columns: where each starts (§2). The time markers before the Start marker start them; without any,
/// every bar does. Lemon Studio: what lies before the first marker belongs to the first column.
pub fn columns(s: &State) -> Vec<u32> {
    let Some(z) = zone(s) else { return Vec::new() };
    let mut at: Vec<u32> = markers(s).iter().filter(|m| m.kind != TimeMarker::START && m.position < z).map(|m| m.position).collect();
    if at.is_empty() {
        at = (0..z).step_by(4 * ppq(s) as usize).collect();
    }
    at.sort_unstable();
    at.dedup();
    if at.first() != Some(&0) {
        at.insert(0, 0);
    }
    at
}

/// An unnamed plain marker in the zone is drawn as "Pad #n", its column's number (§1).
pub fn pad_name(s: &State, m: &TimeMarker) -> Option<String> {
    zone(s).filter(|&z| m.kind == TimeMarker::PLAIN && m.name.is_empty() && m.position < z)?;
    let cols = columns(s);
    cols.binary_search(&m.position).ok().map(|k| format!("Pad #{}", k + 1))
}

fn column_of(cols: &[u32], tick: u32) -> usize {
    cols.partition_point(|&c| c <= tick).saturating_sub(1)
}

/// Clip `i`'s slot (track, column) when it lies in the zone.
pub fn slot_of(s: &State, i: usize) -> Option<(u16, usize)> {
    let z = zone(s)?;
    let c = clips(s).get(i).filter(|c| c.position < z)?;
    Some((c.track, column_of(&columns(s), c.position)))
}

/// The clips of a slot, in time order.
fn slot_clips(s: &State, cols: &[u32], track: u16, col: usize) -> Vec<Clip> {
    let (a, b) = (cols.get(col).copied().unwrap_or(u32::MAX), cols.get(col + 1).copied().or(zone(s)).unwrap_or(u32::MAX));
    let mut v: Vec<Clip> = clips(s).iter().filter(|c| c.track == track && c.position >= a && c.position < b).copied().collect();
    v.sort_by_key(|c| c.position);
    v
}

fn settings(s: &State, track: u16) -> PerfSettings {
    track_info(s, track).map(|t| t.perf).unwrap_or_default()
}

/// The next point of a grid of `unit` ticks that starts at `z`, at or after `now`. With Tolerant, a point passed by
/// less than a 16th of the unit is taken instead: the clip starts at once with its beginning skipped (§4).
fn sync_point(now: u32, z: u32, unit: u32, tolerant: bool) -> u32 {
    let unit = i64::from(unit.max(1));
    let rel = i64::from(now) - i64::from(z);
    let past = rel.rem_euclid(unit);
    let at = if past == 0 || (tolerant && past < unit / 16) { rel - past } else { rel - past + unit };
    (i64::from(z) + at).max(0) as u32
}

/// Where in a clip of `len` ticks starting at `at` it begins playing, by Position sync (§4), on the song's grid from the
/// Start marker.
fn position_offset(sync: u8, at: u32, z: u32, len: u32, ppq: u32) -> u32 {
    let Some(unit) = PerfSettings::sync_ticks(sync, ppq, len, false) else { return 0 };
    let rel = i64::from(at) - i64::from(z);
    (rel.rem_euclid(i64::from(unit.max(1))) as u32) % len.max(1)
}

/// Clip `c` placed on `track` at `at`, its first `skip` ticks cut off (its window moved by as much).
fn placed(s: &State, c: &Clip, track: u16, at: u32, skip: u32) -> Clip {
    let mut k = *c;
    let (w0, w1) = window(s, c);
    let d = to_src(s, c.source, f64::from(skip));
    k.position = at;
    k.length = c.length - skip;
    k.track = track;
    k.muted = false;
    if skip > 0 {
        k.window = Some((w0 + d, w1.max(w0 + d)));
    }
    k.set_id(0);
    k
}

/// A small generator for Random and Exclusive random, seeded by the trigger.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n.max(1) as u64) as usize
    }
}

/// The columns of `track` that hold clips.
fn used(s: &State, cols: &[u32], track: u16) -> Vec<usize> {
    (0..cols.len()).filter(|&k| !slot_clips(s, cols, track, k).is_empty()).collect()
}

/// The column played after `col` by `motion` (§4); `None` = stop.
fn step(motion: u8, col: usize, used: &[usize], rng: &mut Rng) -> Option<usize> {
    match motion {
        1 => None,
        2 => used.iter().copied().find(|&k| k > col).or(used.first().copied()),
        3 => Some(used.iter().copied().find(|&k| k > col).unwrap_or(col)),
        4 => used.iter().copied().find(|&k| k > col),
        5 if !used.is_empty() => Some(used[rng.below(used.len())]),
        6 if used.len() > 1 => {
            let others: Vec<usize> = used.iter().copied().filter(|&k| k != col).collect();
            Some(others[rng.below(others.len())])
        }
        _ => Some(col),
    }
}

fn rng(at: u32, track: u16) -> Rng {
    Rng(u64::from(at) << 16 ^ u64::from(track) ^ 0x9e37_79b9_7f4a_7c15)
}

/// The passes a track plays from `at`, starting with `first` (of column `col`) `skip` ticks in, on by `motion` (§4):
/// Stay repeats `first`, the others take each column's next clip.
fn passes(s: &mut State, track: u16, col: usize, first: Clip, at: u32, skip: u32, motion: u8) -> Vec<Clip> {
    let cols = columns(s);
    let used = used(s, &cols, track);
    let limit = at.saturating_add(AHEAD_BARS * 4 * ppq(s));
    let mut rng = rng(at, track);
    let (mut out, mut t, mut col, mut clip, mut skip) = (Vec::new(), at, col, first, skip.min(first.length.saturating_sub(1)));
    loop {
        out.push(placed(s, &clip, track, t, skip));
        t += clip.length - skip;
        skip = 0;
        if t >= limit || clip.length == 0 {
            break;
        }
        let Some(k) = step(motion, col, &used, &mut rng) else { break };
        if motion != 0 {
            col = k;
            match next_in_slot(s, &cols, track, col) {
                Some(c) => clip = c,
                None => break,
            }
        }
    }
    out
}

/// The slot's next clip by round robin (§2), and the turn moved on.
fn next_in_slot(s: &mut State, cols: &[u32], track: u16, col: usize) -> Option<Clip> {
    let list = slot_clips(s, cols, track, col);
    if list.is_empty() {
        return None;
    }
    let turn = s.playlist.perform.turn.entry((track, col)).or_insert(0);
    let c = list[*turn % list.len()];
    *turn = (*turn + 1) % list.len();
    Some(c)
}

/// Left click on zone clip `i` (§3): its slot plays on its track, from the next Trigger sync point after `now` (the
/// song position while playing; `None` while stopped, when it starts with playback), at once with `instant` (Alt).
/// `queue` (Shift) waits for the playing clip to end its pass. Returns the hint, `None` when `i` is not in the zone.
pub fn trigger(s: &mut State, i: usize, now: Option<u32>, instant: bool, queue: bool) -> Option<String> {
    let (track, col) = slot_of(s, i)?;
    trigger_slot(s, track, col, now, instant, queue)
}

/// `trigger` for slot (`track`, `col`); `None` when the slot has no clips.
fn trigger_slot(s: &mut State, track: u16, col: usize, now: Option<u32>, instant: bool, queue: bool) -> Option<String> {
    let z = zone(s)?;
    let set = settings(s, track);
    let cols = columns(s);
    // Latch: the playing slot's press stops it.
    if set.press == 3 && s.playlist.perform.live.get(&track).is_some_and(|l| l.column == col && now.is_none_or(|t| t < l.until())) {
        stop(s, track, now, instant);
        return Some(format!("{}: stopped", crate::playlist::track_name(s, track)));
    }
    let first = next_in_slot(s, &cols, track, col)?;
    let p = ppq(s);
    let at = match now {
        None => s.playlist.pos.max(z),
        Some(t) if instant => t,
        Some(t) => {
            let playing = s.playlist.perform.live.get(&track).and_then(|l| l.pass_at(t)).map(Clip::end);
            match playing.filter(|_| queue || set.queue) {
                Some(end) => end,
                None => match PerfSettings::sync_ticks(set.trigger_sync, p, first.length, true) {
                    Some(unit) => sync_point(t, z, unit, set.tolerant),
                    None => t,
                },
            }
        }
    };
    let skip = position_offset(set.position_sync, at, z, first.length, p);
    // Hold & motion stays on the slot while held, then moves on (`release`).
    let motion = if set.press == 2 { 0 } else { set.motion };
    let mut list = passes(s, track, col, first, at, skip, motion);
    // A queued or synced start keeps what plays until then.
    let mut kept: Vec<Clip> = s.playlist.perform.live.get(&track).filter(|_| now.is_some()).map_or(Vec::new(), |l| cut_at(s, l, at));
    kept.append(&mut list);
    s.playlist.perform.live.insert(track, Live { passes: kept, column: col, end: None, armed: now.is_none() });
    if matches!(set.press, 1 | 2) {
        s.playlist.perform.held = Some(track);
    }
    s.piano.dirty = true;
    let (bar, step, _) = crate::views::toolbar::bst_ticks(f64::from(at) * 96.0 / f64::from(p));
    Some(format!("{}: {} at {bar}:{step:02}", crate::playlist::track_name(s, track), PerfSettings::PRESSES[usize::from(set.press)]))
}

/// `l`'s passes before `at` (or before it falls silent), the one sounding there cut short.
fn cut_at(s: &State, l: &Live, at: u32) -> Vec<Clip> {
    let at = at.min(l.until());
    let mut v: Vec<Clip> = l.passes.iter().filter(|c| c.position < at).copied().collect();
    if let Some(c) = v.last_mut().filter(|c| c.end() > at) {
        let keep = at - c.position;
        let (w0, _) = window(s, c);
        c.window = Some((w0, w0 + to_src(s, c.source, f64::from(keep))));
        c.length = keep;
    }
    v
}

/// Stop `track`'s clip (§3): at the next Trigger sync point after `now`, or at once with `instant` (or while stopped).
pub fn stop(s: &mut State, track: u16, now: Option<u32>, instant: bool) -> bool {
    let Some(l) = s.playlist.perform.live.get(&track) else { return false };
    let Some(t) = now else {
        s.playlist.perform.live.remove(&track);
        s.piano.dirty = true;
        return true;
    };
    let set = settings(s, track);
    let z = zone(s).unwrap_or(0);
    let len = l.pass_at(t).map_or(4 * ppq(s), |c| c.length);
    let at = match PerfSettings::sync_ticks(set.trigger_sync, ppq(s), len, true).filter(|_| !instant) {
        Some(unit) => sync_point(t, z, unit, false),
        None => t,
    };
    let passes = cut_at(s, l, at);
    if passes.is_empty() || at <= t {
        s.playlist.perform.live.remove(&track);
    } else if let Some(l) = s.playlist.perform.live.get_mut(&track) {
        (l.passes, l.end) = (passes, Some(at));
    }
    s.piano.dirty = true;
    true
}

/// The mouse button is up after a press on a slot (§4 Press): Hold & stop stops at once, Hold & motion moves on as
/// Motion says when the pass playing ends.
pub fn release(s: &mut State, now: Option<u32>) {
    if let Some(track) = s.playlist.perform.held.take() {
        release_track(s, track, now);
    }
}

/// A note on the Performance mode MIDI channel (§3): notes 0–119 are 10 tracks of 12 slots, the note / 12 the track
/// and the note % 12 the column. A key's release ends Press Hold & stop and Hold & motion as the mouse button's does.
pub fn midi_note(s: &mut State, key: u8, down: bool, now: Option<u32>) -> Option<String> {
    zone(s)?;
    let (track, col) = (u16::from(key / 12), usize::from(key % 12));
    if key >= 120 {
        return None;
    }
    if !down {
        if s.playlist.perform.live.get(&track).is_some_and(|l| l.column == col) {
            release_track(s, track, now);
        }
        return None;
    }
    // The mouse's held slot is not this key's.
    let held = s.playlist.perform.held;
    let hint = trigger_slot(s, track, col, now, false, false);
    s.playlist.perform.held = held;
    hint
}

fn release_track(s: &mut State, track: u16, now: Option<u32>) {
    let set = settings(s, track);
    match set.press {
        1 => {
            stop(s, track, now, true);
        }
        2 => {
            let (Some(l), Some(t)) = (s.playlist.perform.live.get(&track).cloned(), now) else { return };
            let Some(cur) = l.pass_at(t).copied() else { return };
            // The pass playing finishes; Motion picks what follows it.
            let cols = columns(s);
            let mut kept = cut_at(s, &l, cur.end());
            let mut column = l.column;
            if let Some(col) = step(set.motion, l.column, &used(s, &cols, track), &mut rng(t, track)) {
                if let Some(c) = next_in_slot(s, &cols, track, col) {
                    kept.extend(passes(s, track, col, c, cur.end(), 0, set.motion));
                    column = col;
                }
            }
            s.playlist.perform.live.insert(track, Live { passes: kept, column, end: None, ..l });
            s.piano.dirty = true;
        }
        _ => {}
    }
}

/// Playback starts (§3): clips triggered while stopped start with it; what played before is over.
pub fn starting(s: &mut State) {
    let p = &mut s.playlist.perform;
    p.live.retain(|_, l| l.armed);
    for l in p.live.values_mut() {
        l.armed = false;
    }
    p.held = None;
}

/// Ctrl+P and the Arranger ▸ menu's Performance mode (§1): switching it on switches to SONG. Off, nothing is launched.
pub fn toggle(s: &mut State) -> String {
    let on = !on(s);
    s.project.performance = Some(on);
    if on {
        s.song_mode = true;
    } else {
        s.playlist.perform = Perform::default();
    }
    s.piano.dirty = true;
    format!("Performance mode: {}", if on { "on" } else { "off" })
}

/// Prepare for performance mode (§1), one undo step: 16 unnamed markers 4 bars apart from bar 1, a Start marker one
/// bar after the last column, SONG, performance mode on. Lemon Studio does not ask first or show the Marker settings
/// dialog yet: it uses the dialog's defaults.
pub fn prepare(s: &mut State) -> String {
    const COUNT: u32 = 16;
    const SPACING_BARS: u32 = 4;
    crate::playlist::snapshot_as(s, "Prepare for performance mode");
    let bar = 4 * ppq(s);
    let start = COUNT * SPACING_BARS * bar + bar;
    let a = arr_mut(s);
    a.markers.retain(|m| !(m.kind == TimeMarker::START || (m.kind == TimeMarker::PLAIN && m.position < start)));
    for k in 0..COUNT {
        a.markers.push(TimeMarker::new(k * SPACING_BARS * bar, ""));
    }
    a.markers.push(TimeMarker { kind: TimeMarker::START, ..TimeMarker::new(start, "Start") });
    a.markers.sort_by_key(|m| m.position);
    s.project.performance = Some(true);
    s.song_mode = true;
    s.playlist.pos = start;
    s.piano.dirty = true;
    format!("Prepared for performance mode: {COUNT} columns, Start marker at bar {}", start / bar + 1)
}

/// The project as played (§3): the zone silent, each launched clip's passes on its track, and that track's own clips
/// silent while they play (Lemon Studio's design).
pub fn apply(s: &State, p: &mut Project) {
    let Some(z) = zone(s) else { return };
    let Some(a) = p.arrangement_mut() else { return };
    for c in a.clips.iter_mut().filter(|c| c.position < z) {
        c.muted = true;
    }
    let mut add = Vec::new();
    for (&t, l) in &s.playlist.perform.live {
        let (from, to) = (l.start(), l.until());
        if from >= to {
            continue;
        }
        for c in a.clips.iter_mut().filter(|c| c.track == t && c.position >= z && c.position < to && c.end() > from) {
            if c.position < from {
                let (w0, _) = window(s, c);
                c.window = Some((w0, w0 + to_src(s, c.source, f64::from(from - c.position))));
                c.length = from - c.position;
            } else {
                c.muted = true;
            }
        }
        add.extend(l.passes.iter().filter(|c| c.position < to).map(|c| {
            let mut c = *c;
            if c.end() > to {
                c.length = to - c.position;
            }
            c
        }));
    }
    a.clips.extend(add);
    a.clips.sort_by_key(|c| c.position);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::midi_in::Msg;
    use crate::prefs;
    use os_model::ClipSource;

    #[test]
    fn sync_points() {
        // 4 beats at PPQ 96 from a Start marker at bar 65.
        let z = 64 * 384;
        assert_eq!(sync_point(z + 10, z, 384, false), z + 384);
        assert_eq!(sync_point(z + 10, z, 384, true), z, "within a 16th: at once");
        assert_eq!(sync_point(z + 30, z, 384, true), z + 384);
        assert_eq!(sync_point(z + 384, z, 384, false), z + 384);
        assert_eq!(position_offset(3, z + 96 * 2 + 48, z, 96, 96), 48, "1 beat: half-way into a 1-beat clip");
        assert_eq!(position_offset(6, z + 384 + 100, z, 384, 96), 100, "Auto: modulo the clip's length");
        assert_eq!(position_offset(0, z + 100, z, 384, 96), 0);
    }

    fn app() -> App {
        let mut app = App::new(None);
        let s = &mut app.state;
        s.project.ppq = 96;
        let a = arr_mut(s);
        // Two columns of 4 bars on track 0: pattern 1 (1 bar) in column 0, pattern 2 (2 bars) in column 1. After the
        // Start marker at bar 9, pattern 3 on track 0 and 1.
        a.clips = vec![
            Clip::new(0, 384, 0, ClipSource::Pattern(1)),
            Clip::new(1536, 768, 0, ClipSource::Pattern(2)),
            Clip::new(3072, 1536, 0, ClipSource::Pattern(3)),
            Clip::new(3072, 1536, 1, ClipSource::Pattern(3)),
        ];
        a.markers = vec![TimeMarker::new(0, ""), TimeMarker::new(1536, ""), TimeMarker { kind: TimeMarker::START, ..TimeMarker::new(3072, "Start") }];
        s.project.performance = Some(true);
        app
    }

    /// §3: the Performance mode MIDI channel's notes trigger slots; their release ends Hold & stop.
    #[test]
    fn midi_notes_trigger_slots() {
        let mut app = app();
        prefs::set_number(&prefs::PERFORMANCE_CHANNEL, 6);
        let note = |key, down| if down { Msg::On { ch: 5, key, vel: 100 } } else { Msg::Off { ch: 5, key, vel: 64 } };
        app.midi_notes(vec![note(1, true), note(1, false)]);
        let l = &app.state.playlist.perform.live[&0];
        assert_eq!((l.column, l.armed), (1, true), "note 1: track 1, column 2, armed while stopped");
        app.midi_notes(vec![note(12, true), note(120, true)]);
        assert!(!app.state.playlist.perform.live.contains_key(&1), "track 2 has no clips in the zone");
        arr_mut(&mut app.state).tracks[0].perf.press = 1;
        app.midi_notes(vec![note(0, true)]);
        assert_eq!(app.state.playlist.perform.live[&0].column, 0);
        app.midi_notes(vec![note(1, false)]);
        assert!(app.state.playlist.perform.live.contains_key(&0), "another key's release");
        app.midi_notes(vec![note(0, false)]);
        assert!(!app.state.playlist.perform.live.contains_key(&0), "Hold & stop: the release stops it");
        app.state.project.performance = Some(false);
        app.midi_notes(vec![note(0, true)]);
        assert!(app.state.playlist.perform.live.is_empty(), "off: nothing is triggered");
        prefs::set_number(&prefs::PERFORMANCE_CHANNEL, 0);
    }

    #[test]
    fn zone_and_columns() {
        let mut app = app();
        let s = &mut app.state;
        assert_eq!((zone(s), columns(s)), (Some(3072), vec![0, 1536]));
        assert_eq!((slot_of(&s, 0), slot_of(&s, 1), slot_of(&s, 2)), (Some((0, 0)), Some((0, 1)), None));
        // Without markers, every bar is a column.
        arr_mut(s).markers.retain(|m| m.kind == TimeMarker::START);
        assert_eq!(columns(s).len(), 8);
        s.project.performance = Some(false);
        assert_eq!((zone(s), slot_of(&s, 0)), (None, None));
    }

    #[test]
    fn a_trigger_loops_the_clip_on_its_track() {
        let mut app = app();
        let s = &mut app.state;
        // Trigger sync 4 beats: a click at 3072 + 30 starts at the next bar (past Tolerant's 24 ticks).
        let hint = trigger(s, 0, Some(3102), false, false).unwrap();
        assert!(hint.contains("Retrigger"), "{hint}");
        let l = &s.playlist.perform.live[&0];
        assert_eq!((l.start(), l.passes[1].position, l.passes.len() as u32), (3456, 3840, AHEAD_BARS));
        let mut p = s.project.clone();
        apply(&s, &mut p);
        let a = p.arrangement().unwrap();
        assert!(a.clips.iter().filter(|c| c.position < 3072).all(|c| c.muted), "the zone is silent");
        // Track 0's own clip plays until the launched one starts; track 1's is untouched.
        let own = a.clips.iter().find(|c| c.track == 0 && c.position == 3072).unwrap();
        assert_eq!((own.length, own.muted, own.window), (384, false, Some((0.0, 384.0))));
        assert!(a.clips.iter().any(|c| c.track == 1 && c.position == 3072 && c.length == 1536 && !c.muted));
        assert!(a.clips.iter().any(|c| c.track == 0 && c.position == 3456 && c.source == ClipSource::Pattern(1) && !c.muted));

        // Alt: at once; Position sync 1 beat starts 10 ticks in.
        arr_mut(s).tracks[0].perf.position_sync = 3;
        trigger(s, 1, Some(3082), true, false);
        let l = &s.playlist.perform.live[&0];
        assert_eq!((l.passes[0].position, l.passes[0].length, l.passes[0].window), (3082, 758, Some((10.0, 768.0))));
        assert_eq!((l.passes[1].position, l.passes[1].length), (3840, 768));

        // A right click stops it at the next sync point (4 beats); Alt at once.
        assert!(stop(s, 0, Some(3100), false));
        assert_eq!(s.playlist.perform.live[&0].until(), 3456);
        assert!(stop(s, 0, Some(3100), true));
        assert!(!s.playlist.perform.live.contains_key(&0));
    }

    #[test]
    fn motion_and_press() {
        let mut app = app();
        let s = &mut app.state;
        // March & wrap: column 0, column 1, column 0, ...
        arr_mut(s).tracks[0].perf = PerfSettings { motion: 2, trigger_sync: 0, ..PerfSettings::default() };
        trigger(s, 0, Some(3072), false, false);
        let l = &s.playlist.perform.live[&0];
        let srcs: Vec<ClipSource> = l.passes.iter().take(4).map(|c| c.source).collect();
        assert_eq!(srcs, [1, 2, 1, 2].map(ClipSource::Pattern));
        assert_eq!(l.passes[1].position, 3072 + 384);
        // One shot: one pass.
        arr_mut(s).tracks[0].perf.motion = 1;
        trigger(s, 1, Some(3072), false, false);
        assert_eq!(s.playlist.perform.live[&0].passes.len(), 1);
        // March & stop from the last column: one pass.
        arr_mut(s).tracks[0].perf.motion = 4;
        trigger(s, 1, Some(3072), false, false);
        assert_eq!(s.playlist.perform.live[&0].passes.len(), 1);
        // Latch: pressing the playing slot again stops it.
        arr_mut(s).tracks[0].perf = PerfSettings { press: 3, trigger_sync: 0, ..PerfSettings::default() };
        trigger(s, 0, Some(3072), false, false);
        trigger(s, 0, Some(3100), false, false);
        assert!(!s.playlist.perform.live.contains_key(&0));
        // Hold & stop: the release stops it.
        arr_mut(s).tracks[0].perf.press = 1;
        trigger(s, 0, Some(3072), false, false);
        assert_eq!(s.playlist.perform.held, Some(0));
        release(s, Some(3200));
        assert!(!s.playlist.perform.live.contains_key(&0));
        // Queue: waits for the pass playing to end.
        arr_mut(s).tracks[0].perf = PerfSettings { trigger_sync: 0, ..PerfSettings::default() };
        trigger(s, 0, Some(3072), false, false);
        trigger(s, 1, Some(3100), false, true);
        let l = &s.playlist.perform.live[&0];
        assert_eq!((l.passes[0].source, l.passes[0].length, l.passes[1].source, l.passes[1].position), (ClipSource::Pattern(1), 384, ClipSource::Pattern(2), 3456));
    }

    /// The ▸ menu's entries and a track's Performance settings ▸ (§1, §4); what plays goes through `play_project`.
    #[test]
    fn menus_and_settings() {
        use crate::playlist::{run, Cmd, Mark};
        let mut app = app();
        let pop = crate::playlist::options_menu(&app.state, 0.0, 0.0);
        let item = pop.items.iter().find(|i| i.label == "Performance mode").unwrap();
        assert!(item.key == "Ctrl+P" && matches!(item.mark, Mark::Check(true)));
        assert!(pop.items.iter().any(|i| i.label == "Prepare for performance mode"));
        let menu = crate::playlist::track_menu(&app.state, 2, 0.0, 0.0);
        let sub = &menu.items.iter().find(|i| i.label == "Performance settings").unwrap().children;
        let heads: Vec<&str> = sub.iter().filter(|i| i.head).map(|i| i.label.as_ref()).collect();
        assert_eq!(heads, ["Press", "Motion", "Trigger sync", "Position sync"]);
        assert_eq!(sub.iter().filter(|i| i.col).count(), 3, "four columns");
        assert!(sub.iter().any(|i| i.label == "4 beats" && matches!(i.mark, Mark::Radio(true))));
        assert!(sub.iter().any(|i| i.label == "Tolerant" && matches!(i.mark, Mark::Check(true))));
        run(&mut app, Cmd::PerfSet(2, 1, 5));
        run(&mut app, Cmd::PerfSet(2, 5, 0));
        let p = track_info(&app.state, 2).unwrap().perf;
        assert_eq!((p.motion, p.tolerant), (5, false));

        // play_project: the zone is silent; a launched clip plays.
        trigger(&mut app.state, 0, Some(3072), true, false);
        let played = crate::rack::play_project(&app.state);
        let a = played.arrangement().unwrap();
        assert!(a.clips.iter().filter(|c| c.position < 3072).all(|c| c.muted));
        assert!(a.clips.iter().any(|c| c.position == 3072 && c.track == 0 && c.source == ClipSource::Pattern(1) && !c.muted));
        assert!(a.clips.windows(2).all(|w| w[0].position <= w[1].position), "kept in position order");
    }

    #[test]
    fn stopped_triggers_start_with_playback() {
        let mut app = app();
        let s = &mut app.state;
        s.playlist.pos = 3072;
        trigger(s, 0, None, false, false);
        assert!(s.playlist.perform.live[&0].armed);
        starting(s);
        assert_eq!(s.playlist.perform.live[&0].start(), 3072);
        starting(s);
        assert!(s.playlist.perform.live.is_empty(), "a second start: what played is over");
    }

    #[test]
    fn prepare_and_toggle() {
        let mut app = App::new(None);
        let s = &mut app.state;
        s.project.ppq = 96;
        let msg = prepare(s);
        assert!(msg.contains("bar 66"), "{msg}");
        let m = markers(s);
        assert_eq!(m.len(), 17);
        assert_eq!((m[1].position, m[15].position, m[16].kind, m[16].position), (1536, 23040, TimeMarker::START, 24960));
        assert_eq!((s.project.performance, s.song_mode, s.playlist.pos), (Some(true), true, 24960));
        assert_eq!(columns(s).len(), 16);
        assert_eq!(toggle(s), "Performance mode: off");
        assert_eq!(zone(s), None);
        s.song_mode = false;
        assert_eq!(toggle(s), "Performance mode: on");
        assert!(s.song_mode);
    }
}
