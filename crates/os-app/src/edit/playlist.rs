// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The session's half of `crate::playlist`: what reads and changes the project, apart from the interface (specs
//! `roadmap/control-plane.md`, part 2, step 6).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#![allow(unused_imports)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use os_model::{
    target, AutoMode, AutoPoint, Arrangement, Automation, Channel, ChannelKind, Clip, ClipSource, PlaylistTrack, Rgb, TimeMarker,
};
use os_ui::Rect;
use crate::app::{App, Drag, State};
use crate::snap::EditorSnap;
use crate::playlist::*;

/// Sets (`None`: clears) the icon `owner` shows; the project is written only when it changes.
pub fn put_icon(s: &mut crate::session::Session, owner: IconOwner, icon: Option<crate::icons::Icon>) {
    let name = icon.and_then(|i| ICONS.iter().find(|(_, x)| *x == i)).map(|&(n, _)| n.to_string());
    s.project.write_if(|p| {
        let slot = match owner {
            IconOwner::Channel(id) => p.channels.iter_mut().find(|c| c.id.0 == id).map(|c| &mut c.lemon_icon),
            IconOwner::Track(t) => p.mixer.iter_mut().find(|m| m.index == t).map(|m| &mut m.lemon_icon),
            IconOwner::Slot(t, k) => p.mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slot_icons.get_mut(usize::from(k))),
        };
        slot.is_some_and(|v| std::mem::replace(v, name) != *v)
    });
}

/// Change icon... ▸: None, then [`ICONS`], the current one ticked; each sets the icon of `key` (a rack row, a mixer
/// track's `TRACK_ICON_KEY + track` or an `fx_key`).
pub fn icon_items(s: &crate::session::Session, key: usize) -> Vec<PItem> {
    // A container's member: its own icon, saved with the container (`crate::container::Member::icon`).
    let cur = match crate::edit::container::of_key(s, key) {
        Some((t, k, id)) => crate::edit::container::member(s, t, k, id).and_then(|m| m.icon),
        None => window_icon(&s.project, key),
    };
    let mut v = vec![PItem::new("None", Cmd::SetIcon(key, None)).mark(Mark::Check(cur.is_none()))];
    v.extend(ICONS.iter().enumerate().map(|(i, &(name, icon))| PItem::new(name, Cmd::SetIcon(key, Some(i))).mark(Mark::Check(cur == Some(icon)))));
    v
}

/// The arrangement being edited, created (with FL's 500 default tracks) when the project has none.
pub fn arr_mut(s: &mut crate::session::Session) -> &mut Arrangement {
    if s.project.arrangements.is_empty() {
        s.project.get_mut().arrangements.push(os_flp::project::default_arrangement());
    }
    let a = s.project.get_mut().arrangement_mut().expect("not empty");
    while a.tracks.len() < usize::from(TRACKS) {
        let n = a.tracks.len() as u32 + 1;
        a.tracks.push(PlaylistTrack::new(n));
    }
    a
}

pub fn clips(s: &crate::session::Session) -> &[Clip] {
    s.project.arrangement().map(|a| a.clips.as_slice()).unwrap_or(&[])
}

pub fn markers(s: &crate::session::Session) -> &[TimeMarker] {
    s.project.arrangement().map(|a| a.markers.as_slice()).unwrap_or(&[])
}

/// Adds a marker among the others, which stay in position order as FL saves them (spec: flp/events/playlist.md
/// "Marker kinds"); returns its index.
pub fn add_marker(s: &mut crate::session::Session, m: TimeMarker) -> usize {
    sort_markers(s);
    let list = &mut arr_mut(s).markers;
    let i = list.partition_point(|o| o.position <= m.position);
    list.insert(i, m);
    s.dirty = true;
    i
}

/// Puts the markers in position order (markers on one tick keep theirs).
pub fn sort_markers(s: &mut crate::session::Session) {
    if let Some(a) = s.project.get_mut().arrangement_mut() {
        a.markers.sort_by_key(|m| m.position);
    }
}

/// The Start marker's tick, the later one of two (§7a; spec: audio/transport.md "Time markers in SONG mode").
pub fn start_marker(s: &crate::session::Session) -> Option<u32> {
    markers(s).iter().filter(|m| m.kind == TimeMarker::START).map(|m| m.position).max()
}

/// Place loop (§7a): a Loop marker named with one space at `tick`; one already there goes instead.
pub(crate) fn place_loop(s: &mut crate::session::Session, tick: u32) {
    let list = &mut arr_mut(s).markers;
    let before = list.len();
    list.retain(|m| !(m.kind == TimeMarker::LOOP && m.position == tick));
    if list.len() == before {
        add_marker(s, TimeMarker { kind: TimeMarker::LOOP, ..TimeMarker::new(tick, " ") });
    }
    s.dirty = true;
}

/// Move content right / left (§7a): marker `i`'s section (from it to the next marker) trades places with the section
/// after it (to the marker after that, or the song's end), or with the one before it (from the previous marker). The
/// clips that start in the two sections move, automation clips too, and so do the markers at their starts. False
/// when there is no section to trade with.
pub fn move_content(s: &mut crate::session::Session, i: usize, right: bool) -> bool {
    let Some(here) = markers(s).get(i).map(|m| m.position) else { return false };
    let mut cuts: Vec<u32> = markers(s).iter().map(|m| m.position).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let end = clips(s).iter().map(|c| c.end()).max().unwrap_or(0);
    let k = cuts.partition_point(|&p| p < here);
    // Where the section from cut j ends: the next cut, else the song's end.
    let after = |j: usize| cuts.get(j + 1).copied().unwrap_or(end);
    let (a, b, c) = if right {
        let Some(&b) = cuts.get(k + 1) else { return false };
        (here, b, after(k + 1))
    } else {
        let Some(&a) = k.checked_sub(1).and_then(|j| cuts.get(j)) else { return false };
        (a, here, after(k))
    };
    if !(a < b && b < c) {
        return false;
    }
    let (first, second) = (b - a, c - b);
    let to = |t: u32| match t {
        _ if t >= a && t < b => Some(t + second),
        _ if t >= b && t < c => Some(t - first),
        _ => None,
    };
    let arr = arr_mut(s);
    for cl in &mut arr.clips {
        if let Some(t) = to(cl.position) {
            cl.position = t;
        }
    }
    for m in &mut arr.markers {
        if let Some(t) = to(m.position) {
            m.position = t;
        }
    }
    sort_markers(s);
    s.dirty = true;
    true
}

/// The group track leading track `t`'s group (§6a): the nearest ungrouped track above a grouped track.
pub fn group_track(s: &crate::session::Session, t: u16) -> Option<u16> {
    if t == 0 || !track_info(s, t).is_some_and(|x| x.grouped) {
        return None;
    }
    (0..t).rev().find(|&k| !track_info(s, k).is_some_and(|x| x.grouped))
}

/// Track `t` leads a group: it is not grouped and the track below it is.
pub fn is_group_track(s: &crate::session::Session, t: u16) -> bool {
    !track_info(s, t).is_some_and(|x| x.grouped) && track_info(s, t + 1).is_some_and(|x| x.grouped)
}

pub fn track_info(s: &crate::session::Session, t: u16) -> Option<&PlaylistTrack> {
    s.project.arrangement().and_then(|a| a.tracks.get(usize::from(t)))
}

pub fn track_on(s: &crate::session::Session, t: u16) -> bool {
    track_info(s, t).is_none_or(|t| t.enabled)
}

pub fn track_name(s: &crate::session::Session, t: u16) -> String {
    track_info(s, t).and_then(|t| t.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Track {}", t + 1))
}

/// TOOLS ▸ Lock all tracks to their clips (Lemon Studio's design; spec: ui/playlist.md "Locking tracks to their
/// clips"): each track of the arrangement whose clips all share one source is locked to it; a track without clips or
/// with clips of several sources is left as it is. Returns (tracks locked, tracks skipped for mixed sources).
pub fn lock_tracks(s: &mut crate::session::Session) -> (usize, usize) {
    let mut one: std::collections::BTreeMap<u16, Option<ClipSource>> = std::collections::BTreeMap::new();
    for c in clips(s) {
        one.entry(c.track).and_modify(|e| if *e != Some(c.source) { *e = None }).or_insert(Some(c.source));
    }
    let tracks = &mut arr_mut(s).tracks;
    let (mut locked, mut mixed) = (0, 0);
    for (t, src) in one {
        match (src, tracks.get_mut(usize::from(t))) {
            (Some(src), Some(tr)) => {
                tr.clip_lock = Some(src);
                locked += 1;
            }
            (None, Some(_)) => mixed += 1,
            _ => {}
        }
    }
    (locked, mixed)
}

/// TOOLS ▸ Unlock all tracks from their clips: clears every lock of the arrangement; returns how many there were.
pub fn unlock_tracks(s: &mut crate::session::Session) -> usize {
    arr_mut(s).tracks.iter_mut().filter_map(|t| t.clip_lock.take()).count()
}

pub(crate) fn ppq(s: &crate::session::Session) -> u32 {
    u32::from(s.project.ppq.max(4))
}

pub(crate) fn bar(s: &crate::session::Session) -> u32 {
    ppq(s) * 4
}

pub(crate) fn channel_of(s: &crate::session::Session, ch: u16) -> Option<&Channel> {
    s.project.channel(ch)
}

/// The rack row of the channel an audio clip plays (`None` for pattern and automation clips).
pub(crate) fn audio_clip_row(s: &crate::session::Session, c: &Clip) -> Option<usize> {
    let ClipSource::Channel(ch) = c.source else { return None };
    s.project.channels.iter().position(|x| x.index == ch && x.kind.is_sampler())
}

pub fn source_name(s: &crate::session::Session, src: ClipSource) -> String {
    match src {
        ClipSource::Pattern(n) => s.project.pattern(n).and_then(|p| p.name.clone()).unwrap_or_else(|| format!("Pattern {n}")),
        ClipSource::Channel(c) => channel_of(s, c).and_then(|ch| ch.name.clone()).unwrap_or_else(|| format!("Channel {}", c + 1)),
    }
}

pub fn source_color(s: &crate::session::Session, src: ClipSource) -> Option<Rgb> {
    match src {
        ClipSource::Pattern(n) => s.project.pattern(n).and_then(|p| p.color),
        ClipSource::Channel(c) => channel_of(s, c).and_then(|ch| ch.color),
    }
}

/// The colour an audio or automation clip's title strip and waveform take: its channel's colour as the file stores
/// it (custom or not), else a pastel picked from the channel's name (spec: ui/playlist.md §2, "Channel-colour rule").
/// `None` for pattern clips (neutral titles).
pub fn clip_tint(s: &crate::session::Session, src: ClipSource) -> Option<Rgb> {
    let ClipSource::Channel(c) = src else { return None };
    let ch = channel_of(s, c)?;
    ch.color.or(ch.rack.stored_color).or_else(|| Some(pastel(ch.name.as_deref().or(ch.sample_path.as_deref()).unwrap_or(""))))
}

pub fn source_kind(s: &crate::session::Session, src: ClipSource) -> PickKind {
    match src {
        ClipSource::Pattern(_) => PickKind::Pattern,
        ClipSource::Channel(c) => match channel_of(s, c).map(|ch| ch.kind) {
            Some(ChannelKind::Automation) => PickKind::Automation,
            _ => PickKind::Audio,
        },
    }
}

/// How fast an audio clip's channel goes through its sample: its stretch in any method (§9b, audio/time-stretch.md),
/// times the channel's pitch where that moves the sample (no stretch time, not a realtime method), as the engine
/// plays it (audio/audio-clips.md "Length for the song").
pub fn audio_speed(s: &crate::session::Session, src: ClipSource) -> f64 {
    let ClipSource::Channel(c) = src else { return 1.0 };
    let Some(ch) = channel_of(s, c) else { return 1.0 };
    let sound = &ch.sound;
    let stretch = if sound.stretch == [0; 4] {
        1.0
    } else {
        // A fitted time needs the sample's duration (the multiplier alone does not).
        let duration = if sound.stretch_time().is_some() { sample_secs(s, ch) } else { 0.0 };
        1.0 / sound.stretch_factor(duration, s.project.tick_tempo(s.tempo()))
    };
    let pitched = sound.channel_pitch_applies() && !os_model::stretch_mode::realtime(sound.stretch_mode());
    if !pitched {
        return stretch;
    }
    let main = ch.misc.main_pitch.then(|| i32::from(s.project.main_pitch.unwrap_or(0)));
    let cents = f64::from(os_engine::laws::channel_pitch(f64::from(ch.pitch_cents), ch.misc.fine, ch.misc.root, main)) + 6000.0;
    stretch * os_engine::laws::cents_rate(cents)
}

/// Works every audio clip's length out again from its trims and its channel's stretch, pitch and the tempo, as FL
/// does (the stored length is not used for an audio clip): each trim point becomes ticks on its own, rounded; an
/// untrimmed clip spans the whole sample (ui/playlist.md §9c, audio/audio-clips.md). Clips whose sample is not
/// loaded keep their length. Returns whether a length changed. (The app's tick uses `audio_fits` + `fit_clips`.)
#[cfg(test)]
pub fn fit_audio(s: &mut crate::session::Session) -> bool {
    audio_fits(s).is_some_and(|fits| fit_clips(&mut s.project.get_mut().arrangements, &fits))
}

/// The fits of every channel with clips (`fit_audio`); `None` while waveforms are loading.
pub fn audio_fits(s: &crate::session::Session) -> Option<Fits> {
    if s.samples.wait {
        return None;
    }
    let mut per = Fits::new();
    for a in &s.project.arrangements {
        for c in &a.clips {
            let ClipSource::Channel(ch) = c.source else { continue };
            if per.contains_key(&ch) {
                continue;
            }
            let fit = (source_kind(s, c.source) == PickKind::Audio)
                .then(|| channel_of(s, ch))
                .flatten()
                .map(|x| (audio_ticks(s, x), sample_secs(s, x) * 1000.0))
                .filter(|&(t, ms)| ms > 0.0 && t > 0.0 && t.is_finite());
            per.insert(ch, fit);
        }
    }
    Some(per)
}

/// The project's automatic crossfades were switched (`App::toggle_auto_crossfades`): turned off, the crossfades'
/// lengths go to 0 and their tensions stay; turned on, they are set from the overlaps on the next tick.
pub fn crossfades_switched(s: &mut crate::session::Session) {
    if !os_model::crossfade::on(&s.project) {
        os_model::crossfade::clear(s.project.get_mut());
    }
    s.dirty = true;
}

/// The channel's whole sample in ticks, unrounded: the stretch time (whole 1/192 beats) or the sample's duration at
/// the tempo, times the multiplier, over the channel pitch's rate where that moves the sample (`audio_speed`). 0 while
/// the waveform is not loaded.
pub(crate) fn audio_ticks(s: &crate::session::Session, ch: &os_model::Channel) -> f64 {
    let secs = sample_secs(s, ch);
    if secs <= 0.0 {
        return 0.0;
    }
    let sound = &ch.sound;
    let ppq = f64::from(ppq(s));
    let base = match sound.stretch_time() {
        Some(_) => f64::from(f32::from_bits(sound.stretch[0])) * ppq / 192.0,
        None => secs * s.project.tick_tempo(s.tempo().max(1.0)) / 60.0 * ppq,
    };
    let ticks = base * sound.stretch_multiplier();
    if !sound.channel_pitch_applies() || os_model::stretch_mode::realtime(sound.stretch_mode()) {
        return ticks;
    }
    let main = ch.misc.main_pitch.then(|| i32::from(s.project.main_pitch.unwrap_or(0)));
    let cents = f64::from(os_engine::laws::channel_pitch(f64::from(ch.pitch_cents), ch.misc.fine, ch.misc.root, main)) + 6000.0;
    ticks / os_engine::laws::cents_rate(cents)
}

/// Seconds the channel's sample lasts (0 while its waveform is not loaded).
pub(crate) fn sample_secs(s: &crate::session::Session, ch: &os_model::Channel) -> f64 {
    ch.sample_path.as_deref().and_then(|p| s.samples.wave(p)).map_or(0.0, |w| w.secs())
}

/// Seconds per tick at the project tempo (the ticks run at `Project::tick_tempo`).
pub(crate) fn secs_per_tick(s: &crate::session::Session) -> f64 {
    60.0 / s.project.tick_tempo(s.tempo().max(1.0)) / f64::from(ppq(s))
}

/// Natural length of a source in ticks: a pattern's length (its bars, the engine's rule: `Project::pattern_length`,
/// spec: audio/song-structure.md), a sample's duration, an automation's last point.
pub fn source_len(s: &crate::session::Session, src: ClipSource) -> u32 {
    let bar = bar(s);
    match src {
        ClipSource::Pattern(n) => s.project.pattern_length(n),
        ClipSource::Channel(c) => {
            if let Some(a) = s.project.automation(c) {
                let beats = a.points.last().map_or(0.0, |p| p.beat);
                return ((beats * f64::from(ppq(s))).round() as u32).max(bar / 4);
            }
            // The whole sample at the channel's speed (`fit_audio`).
            let t = channel_of(s, c).map_or(0.0, |ch| audio_ticks(s, ch));
            if t > 0.0 && t.is_finite() { (t.round() as u32).max(1) } else { bar }
        }
    }
}

/// Pattern `n`'s length changed from `old` (its time signature, spec: ui/pattern-time-signature.md §5): its clips that
/// were not resized by hand (uncut, at the old natural length) take the new one, in every arrangement.
pub fn refit_pattern_clips(s: &mut crate::session::Session, n: u16, old: u32) {
    let new = s.project.pattern_length(n);
    for a in &mut s.project.get_mut().arrangements {
        for c in a.clips.iter_mut().filter(|c| c.source == ClipSource::Pattern(n) && c.window.is_none() && c.length == old) {
            c.length = new;
        }
    }
    s.dirty = true;
}

/// A clip's window in its source's unit (pattern ticks, audio ms, automation beats), filled in when unset.
pub fn window(s: &crate::session::Session, c: &Clip) -> (f64, f64) {
    c.window.unwrap_or_else(|| (0.0, to_src(s, c.source, f64::from(c.length))))
}

/// Ticks to the source's unit.
pub fn to_src(s: &crate::session::Session, src: ClipSource, ticks: f64) -> f64 {
    match source_kind(s, src) {
        PickKind::Pattern => ticks,
        PickKind::Audio => ticks * secs_per_tick(s) * 1000.0 * audio_speed(s, src),
        PickKind::Automation => ticks / f64::from(ppq(s)),
    }
}

/// Undo snapshot of everything the playlist edits: a step of the project's undo history (`crate::history`).
pub fn snapshot(s: &mut crate::session::Session) {
    snapshot_as(s, "Arranger edit");
}

/// `snapshot`, the step named `name` (as the browser's History lists it; spec: ui/settings-choices.md "Undo history:
/// the four checks (2026-10-16)").
pub fn snapshot_as(s: &mut crate::session::Session, name: &'static str) {
    arr_mut(s);
    let snap = crate::history::Snap::Playlist(Box::new(capture(s)));
    crate::history::record(s, name, snap);
}

/// `snapshot_as` for a command that changes more than the playlist: one step named `name`, holding the playlist
/// and `extra`, what the command changes beyond it.
pub fn snapshot_with(s: &mut crate::session::Session, name: &'static str, extra: crate::history::Snap) {
    arr_mut(s);
    let snap = crate::history::Snap::Several(vec![crate::history::Snap::Playlist(Box::new(capture(s))), extra]);
    crate::history::record(s, name, snap);
}

/// The playlist's undo state now (`snapshot`).
pub(crate) fn capture(s: &crate::session::Session) -> Snapshot {
    let (clips, markers, tracks) = s.project.arrangement().map(|a| (a.clips.clone(), a.markers.clone(), a.tracks.clone())).unwrap_or_default();
    let stretch = s.project.channels.iter().map(|c| (c.sound.stretch, c.mixer_track)).collect();
    (clips, markers, tracks, s.project.automations.clone(), stretch)
}

/// Puts an undo state back (undo and redo, `crate::history`).
pub(crate) fn restore(s: &mut crate::session::Session, (c, m, t, a, st): Snapshot) {
    let arr = arr_mut(s);
    arr.clips = c;
    arr.markers = m;
    arr.tracks = t;
    s.project.get_mut().automations = a;
    for (ch, (v, m)) in s.project.get_mut().channels.iter_mut().zip(st) {
        ch.sound.stretch = v;
        ch.mixer_track = m;
    }
    s.touched.views.push(crate::session::View::ClipsCleared);
    s.dirty = true;
}

/// Topmost clip under (x, y): the last one in the list (drawn on top).
pub fn clip_at(s: &crate::session::Session, g: &PGeom, x: f32, y: f32) -> Option<usize> {
    clips(s).iter().enumerate().rev().find(|(_, c)| g.clip_rect(c).contains(x, y)).map(|(i, _)| i)
}

/// `idx` plus every clip that shares a group with one of them (grouped clips select and move as one, §3a); just `idx`
/// while Enable groups is off (spec: ui/enable-groups.md §3).
pub fn with_groups(s: &crate::session::Session, mut idx: Vec<usize>) -> Vec<usize> {
    if !crate::enable_groups::on() {
        return idx;
    }
    let groups: Vec<u16> = idx.iter().filter_map(|&i| clips(s).get(i)).map(Clip::group).filter(|&g| g != 0).collect();
    if groups.is_empty() {
        return idx;
    }
    for (i, c) in clips(s).iter().enumerate() {
        if groups.contains(&c.group()) && !idx.contains(&i) {
            idx.push(i);
        }
    }
    idx
}

/// Copies of grouped clips (`from..` in the clip list) form new groups of their own, numbered as Shift+G numbers
/// a new group (§3a).
pub(crate) fn regroup_copies(s: &mut crate::session::Session, from: usize) {
    let arr = arr_mut(s);
    let mut used: Vec<u16> = arr.clips[..from.min(arr.clips.len())].iter().map(Clip::group).collect();
    let mut map: HashMap<u16, u16> = HashMap::new();
    for c in arr.clips.iter_mut().skip(from) {
        let g = c.group();
        if g != 0 {
            let n = *map.entry(g).or_insert_with(|| {
                let n = free_group(&used);
                used.push(n);
                n
            });
            c.set_group(n);
        }
    }
}

/// Copies of the moved clips `orig` (index, clip as the move began) for a held click's pick-up (spec:
/// ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)"): the originals go back to where
/// they were and the copies are added after every clip, grouped copies in groups of their own as a Shift+drag's are.
/// Returns the copies.
pub fn clone_clips(s: &mut crate::session::Session, orig: &[(usize, Clip)]) -> Vec<(usize, Clip)> {
    let arr = arr_mut(s);
    for &(i, o) in orig {
        if let Some(c) = arr.clips.get_mut(i) {
            c.position = o.position;
            c.track = o.track;
        }
    }
    let first = arr.clips.len();
    for &(_, o) in orig {
        let mut cp = o;
        cp.set_id(0);
        arr.clips.push(cp);
    }
    regroup_copies(s, first);
    s.dirty = true;
    clips(s).iter().enumerate().skip(first).map(|(i, c)| (i, *c)).collect()
}

/// Split clip `i` at `tick` (inside it); the right part keeps the content in place. Returns the new clip's index.
pub fn split(s: &mut crate::session::Session, i: usize, tick: u32) -> Option<usize> {
    let c = *clips(s).get(i)?;
    if tick <= c.position || tick >= c.end() {
        return None;
    }
    let (w0, w1) = window(s, &c);
    let left_len = tick - c.position;
    let cut = to_src(s, c.source, f64::from(left_len));
    let mut left = c;
    left.length = left_len;
    left.window = Some((w0, w0 + cut));
    let mut right = c;
    right.position = tick;
    right.length = c.end() - tick;
    right.window = Some((w0 + cut, w1.max(w0 + cut)));
    right.set_id(0);
    let a = arr_mut(s);
    a.clips[i] = left;
    a.clips.push(right);
    let n = a.clips.len();
    s.dirty = true;
    Some(n - 1)
}

/// Slice every clip crossing `tick` on tracks `t0..=t1`.
pub fn slice(s: &mut crate::session::Session, tick: u32, t0: u16, t1: u16) -> usize {
    let hit: Vec<usize> = clips(s).iter().enumerate().filter(|(_, c)| c.track >= t0 && c.track <= t1 && c.position < tick && c.end() > tick).map(|(i, _)| i).collect();
    hit.iter().filter(|&&i| split(s, i, tick).is_some()).count()
}

/// Move the content of a clip by `dt` ticks inside it (Slip tool).
pub fn slip(s: &mut crate::session::Session, i: usize, orig: &Clip, dt: i64) {
    let (w0, w1) = window(s, orig);
    let d = to_src(s, orig.source, -dt as f64);
    let (mut a, mut b) = (w0 + d, w1 + d);
    if a < 0.0 {
        b -= a;
        a = 0.0;
    }
    if let Some(c) = arr_mut(s).clips.get_mut(i) {
        c.window = Some((a, b));
    }
    s.dirty = true;
}

/// Resize from the right: the window end follows the length.
pub fn resize_right(s: &mut crate::session::Session, i: usize, orig: &Clip, len: u32) {
    let (w0, _) = window(s, orig);
    let end = w0 + to_src(s, orig.source, f64::from(len));
    // An audio clip without trims plays its whole sample (`fit_audio`), and an uncut pattern clip its whole pattern
    // whatever its length (spec: audio/song-structure.md): resized, either gets a window and plays only that. An
    // unset automation window already means "from the start for the clip's length".
    let cuts = match orig.source {
        ClipSource::Pattern(_) => true,
        ClipSource::Channel(_) => source_kind(s, orig.source) == PickKind::Audio,
    };
    if let Some(c) = arr_mut(s).clips.get_mut(i) {
        c.length = len.max(1);
        if orig.window.is_some() || cuts {
            c.window = Some((w0, end));
        }
    }
    s.dirty = true;
}

/// Resize from the left (trim start): the content stays in place (§2a).
pub fn resize_left(s: &mut crate::session::Session, i: usize, orig: &Clip, dt: i64) {
    let (w0, w1) = window(s, orig);
    // The window start may not go below the source start.
    let min_dt = -(w0 / to_src(s, orig.source, 1.0)).floor() as i64;
    let dt = dt.max(min_dt).max(-i64::from(orig.position)).min(i64::from(orig.length) - 1);
    let d = to_src(s, orig.source, dt as f64);
    if let Some(c) = arr_mut(s).clips.get_mut(i) {
        c.position = (i64::from(orig.position) + dt) as u32;
        c.length = (i64::from(orig.length) - dt) as u32;
        c.window = Some(((w0 + d).max(0.0), w1));
    }
    s.dirty = true;
}

/// `c` as drawn with its edges `left` and `right` px from where they are, for a resize on its way
/// (spec: ui/animations.md §4.10). The window goes with the edges, as in [`resize_left`] and [`resize_right`], so that
/// the content stays put.
pub fn with_edges(s: &crate::session::Session, g: &PGeom, c: &Clip, left: f32, right: f32) -> Clip {
    let dl = f64::from(left / g.ppt).round() as i64;
    let dr = f64::from(right / g.ppt).round() as i64;
    let end = (i64::from(c.end()) + dr).max(1);
    let pos = (i64::from(c.position) + dl).clamp(0, end - 1);
    let mut drawn = *c;
    drawn.position = pos as u32;
    drawn.length = (end - pos) as u32;
    if c.window.is_some() || dl != 0 {
        let (w0, w1) = window(s, c);
        let from = (w0 + to_src(s, c.source, (pos - i64::from(c.position)) as f64)).max(0.0);
        drawn.window = Some((from, w1 + to_src(s, c.source, dr as f64)));
    }
    drawn
}

/// Swap two tracks with their clips.
pub fn swap_tracks(s: &mut crate::session::Session, a: u16, b: u16) {
    let arr = arr_mut(s);
    let (ta, tb) = (arr.tracks[usize::from(a)].clone(), arr.tracks[usize::from(b)].clone());
    arr.tracks[usize::from(a)] = PlaylistTrack { number: ta.number, ..tb };
    arr.tracks[usize::from(b)] = PlaylistTrack { number: tb.number, ..ta };
    for c in &mut arr.clips {
        if c.track == a {
            c.track = b;
        } else if c.track == b {
            c.track = a;
        }
    }
    s.dirty = true;
}

/// Insert an empty track at `t` (the rest move down; the last track's clips stay on it).
pub fn insert_track(s: &mut crate::session::Session, t: u16) {
    let arr = arr_mut(s);
    for c in &mut arr.clips {
        if c.track >= t && c.track < TRACKS - 1 {
            c.track += 1;
        }
    }
    let n = arr.tracks.len();
    for k in (usize::from(t) + 1..n).rev() {
        let prev = arr.tracks[k - 1].clone();
        arr.tracks[k] = PlaylistTrack { number: arr.tracks[k].number, ..prev };
    }
    arr.tracks[usize::from(t)] = PlaylistTrack::new(u32::from(t) + 1);
    s.dirty = true;
}

/// First track at or below `from` with no clips overlapping `[a, b)`.
pub fn free_track(s: &crate::session::Session, from: u16, a: u32, b: u32) -> u16 {
    (from..TRACKS).find(|&t| !clips(s).iter().any(|c| c.track == t && c.position < b && c.end() > a)).unwrap_or(from)
}

/// The track below the last one that has clips.
pub fn track_after_used(s: &crate::session::Session) -> u16 {
    clips(s).iter().map(|c| c.track + 1).max().unwrap_or(0).min(TRACKS - 1)
}

/// `name`, or `name #2`, `name #3`... when a channel already has it (spec: plugins/plugin-parameters.md §4).
pub fn unique_channel_name(s: &crate::session::Session, name: &str) -> String {
    let taken = |n: &str| s.project.channels.iter().any(|c| c.name.as_deref() == Some(n));
    if !taken(name) {
        return name.to_string();
    }
    (2..).map(|k| format!("{name} #{k}")).find(|n| !taken(n)).unwrap_or_default()
}

/// Screen x of an automation point of clip `c`.
pub fn point_x(s: &crate::session::Session, g: &PGeom, c: &Clip, beat: f64) -> f32 {
    let (w0, _) = window(s, c);
    g.x_of(c.position as f32 + ((beat - w0) * f64::from(g.ppq)) as f32)
}

/// Beat in the automation under screen x for clip `c`.
pub fn beat_at(s: &crate::session::Session, g: &PGeom, c: &Clip, x: f32) -> f64 {
    let (w0, _) = window(s, c);
    w0 + f64::from(g.tick_at(x) - c.position as f32) / f64::from(g.ppq)
}

/// The point of an automation clip near (x, y) (8 px).
pub fn point_near(s: &crate::session::Session, g: &PGeom, ci: usize, x: f32, y: f32) -> Option<usize> {
    let c = clips(s).get(ci)?;
    let ClipSource::Channel(ch) = c.source else { return None };
    let a = s.project.automation(ch)?;
    let body = g.body(c);
    a.points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, (point_x(s, g, c, p.beat) - x).hypot(value_y(body, p.value) - y)))
        .filter(|&(_, d)| d <= 8.0)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The tension handle (the segment's midpoint on the curve) near (x, y): index of the segment's end point.
pub fn handle_near(s: &crate::session::Session, g: &PGeom, ci: usize, x: f32, y: f32) -> Option<usize> {
    let c = clips(s).get(ci)?;
    let ClipSource::Channel(ch) = c.source else { return None };
    let a = s.project.automation(ch)?;
    let body = g.body(c);
    (1..a.points.len()).find(|&i| {
        let (p, q) = (a.points[i - 1], a.points[i]);
        if !q.mode.has_tension() || matches!(q.mode, AutoMode::Hold) {
            return false;
        }
        let mid = (p.beat + q.beat) / 2.0;
        let hx = point_x(s, g, c, mid);
        let hy = value_y(body, a.value_at(mid).unwrap_or(0.0));
        (hx - x).hypot(hy - y) <= 6.0 && (point_x(s, g, c, q.beat) - point_x(s, g, c, p.beat)) > 16.0
    })
}

/// Distance from (x, y) to the curve at x.
pub fn curve_dist(s: &crate::session::Session, g: &PGeom, ci: usize, x: f32, y: f32) -> Option<f32> {
    let c = clips(s).get(ci)?;
    let ClipSource::Channel(ch) = c.source else { return None };
    let a = s.project.automation(ch)?;
    let v = a.value_at(beat_at(s, g, c, x))?;
    Some((value_y(g.body(c), v) - y).abs())
}

/// Add a point to automation channel `ch` at `beat` with `value`; returns its index.
pub fn add_point(s: &mut crate::session::Session, ch: u16, beat: f64, value: f64) -> Option<usize> {
    let a = s.project.get_mut().automation_mut(ch)?;
    let i = a.points.partition_point(|p| p.beat <= beat);
    // A new point takes the mode / tension of the segment it splits ("new segments use the last tension").
    let (mode, tension) = a.points.get(i).map_or((AutoMode::Single, 0.0), |q| (q.mode, q.tension));
    let flag = if mode.has_tension() { 2 } else { 0 };
    a.points.insert(i, AutoPoint { beat: beat.max(0.0), value: value.clamp(0.0, 1.0), tension, mode, flag });
    s.dirty = true;
    Some(i)
}

/// Move point `i` to (beat, value), kept between its neighbours; the first point stays at beat 0.
pub fn move_point(s: &mut crate::session::Session, ch: u16, i: usize, beat: f64, value: f64) {
    let Some(a) = s.project.get_mut().automation_mut(ch) else { return };
    let n = a.points.len();
    if i >= n {
        return;
    }
    let lo = if i == 0 { 0.0 } else { a.points[i - 1].beat };
    let hi = if i + 1 < n { a.points[i + 1].beat } else { f64::MAX };
    a.points[i].beat = if i == 0 { 0.0 } else { beat.clamp(lo, hi) };
    a.points[i].value = value.clamp(0.0, 1.0);
    s.dirty = true;
}

pub fn delete_point(s: &mut crate::session::Session, ch: u16, i: usize) {
    let Some(a) = s.project.get_mut().automation_mut(ch) else { return };
    // The first and a lone second point stay (a clip needs a curve).
    if a.points.len() > 2 && i > 0 && i < a.points.len() {
        a.points.remove(i);
        s.dirty = true;
    }
}

/// Parse a typed value (percent, BPM or 0..1) back to 0..1.
pub fn parse_value(s: &crate::session::Session, ch: u16, text: &str) -> Option<f64> {
    let t = text.trim().trim_end_matches('%').trim_end_matches("BPM").trim();
    let v: f64 = t.parse().ok()?;
    let tgt = s.project.automation(ch).and_then(|a| a.target()).map(target::decode);
    Some(match tgt {
        Some(target::Target::Tempo) => tempo_value(v),
        _ if v > 1.0 || text.contains('%') => v / 100.0,
        _ => v,
    })
    .map(|v| v.clamp(0.0, 1.0))
}

pub fn clip_menu(s: &crate::session::Session, i: usize, x: f32, y: f32) -> Popup {
    let c = clips(s)[i];
    let head = match source_kind(s, c.source) {
        PickKind::Pattern => "Pattern clip",
        PickKind::Audio => "Audio clip",
        PickKind::Automation => "Automation clip",
    };
    let mut items = vec![
        PItem::head(head),
        PItem::new("Preview", Cmd::ClipPreview(i)),
        PItem::sep(),
        PItem::new("Muted", Cmd::ClipMute(i)).mark(Mark::Check(c.muted)),
        PItem::sep(),
        PItem::new("Rename...", Cmd::ClipRename(i)),
        PItem::new("Random color", Cmd::ClipRandomColor(i)),
        PItem::sep(),
    ];
    if let ClipSource::Pattern(_) = c.source {
        items.push(PItem::new("Edit pattern", Cmd::ClipEditSource(i)));
        items.push(PItem::new("Make unique", Cmd::ClipUnique(i)));
    }
    items.push(PItem::new("Select all similar clips", Cmd::ClipSelectSimilar(i)));
    items.push(PItem::new("Delete", Cmd::ClipDelete(i)));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// Right-click on an audio or automation clip in the picker (§10a): its channel's menu, as FL shows it there.
pub fn pick_menu(s: &crate::session::Session, ch: u16, x: f32, y: f32) -> Popup {
    let audio = source_kind(s, ClipSource::Channel(ch)) == PickKind::Audio;
    let it = |label: &'static str, c: PickCmd| PItem::new(label, Cmd::Pick(c, ch));
    let mut items = Vec::new();
    if audio {
        items.push(it("Preview", PickCmd::Preview));
    }
    items.extend([
        it("Select in arranger", PickCmd::SelectInPlaylist),
        it("Group selected...", PickCmd::Group),
        PItem::sep(),
        it("Rename, color and icon...", PickCmd::Rename),
        it("Change color...", PickCmd::ChangeColor),
        it("Random color", PickCmd::RandomColor),
        match s.project.channels.iter().position(|c| c.index == ch) {
            Some(row) => PItem::sub("Change icon...", icon_items(s, row)),
            None => PItem::not_yet("Change icon..."),
        },
        PItem::sep(),
    ]);
    if audio {
        // Greyed while the channel has no sample to open.
        let edit = it("Edit sample...", PickCmd::EditSample);
        let has = s.project.channel(ch).is_some_and(|c| c.sample_path.as_deref().is_some_and(|p| !p.is_empty()));
        items.extend([it("Load sample...", PickCmd::LoadSample), if has { edit } else { edit.off() }]);
    }
    // Ticked while the channel's cut and cut by are one group, as on the rack's channel menu (ui/channel-rack.md §18).
    let cuts = s.project.channel(ch).is_some_and(|c| crate::rack::cuts_itself(&c.misc));
    items.extend([it("Chokes itself", PickCmd::CutItself).mark(Mark::Check(cuts)), PItem::sep(), it("Clone", PickCmd::Clone)]);
    if !audio {
        items.push(it("Clone with links", PickCmd::Clone));
    }
    items.push(it("Delete...", PickCmd::Delete));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// A track's Performance settings ▸ (spec: ui/performance-mode.md §4): Press, Motion, Trigger sync with Queue and
/// Tolerant, Position sync, a column each.
pub(crate) fn perf_items(s: &crate::session::Session, t: u16) -> Vec<PItem> {
    use os_model::PerfSettings as P;
    let p = track_info(s, t).map(|x| x.perf).unwrap_or_default();
    let list = |head: &'static str, which: u8, labels: &[&'static str], on: u8, col: bool| {
        let mut v = vec![PItem { col, ..PItem::head(head) }];
        v.extend(labels.iter().enumerate().map(|(k, l)| PItem::new(*l, Cmd::PerfSet(t, which, k as u8)).mark(Mark::Radio(on == k as u8))));
        v
    };
    let mut items = list("Press", 0, &P::PRESSES, p.press, false);
    items.extend(list("Motion", 1, &P::MOTIONS, p.motion, true));
    items.extend(list("Trigger sync", 2, &P::SYNCS, p.trigger_sync, true));
    items.push(PItem::new("Queue", Cmd::PerfSet(t, 4, u8::from(!p.queue))).mark(Mark::Check(p.queue)));
    items.push(PItem::new("Tolerant", Cmd::PerfSet(t, 5, u8::from(!p.tolerant))).mark(Mark::Check(p.tolerant)));
    items.extend(list("Position sync", 3, &P::SYNCS, p.position_sync, true));
    items
}

/// Marker `i`'s menu (§7a): the row's items at `tick`, a header "<name> at <bar>:<step>:<tick>", Delete, Rename...,
/// Move content left / right, then its action.
pub fn marker_menu(s: &crate::session::Session, i: usize, tick: u32, x: f32, y: f32) -> Popup {
    let Some(m) = markers(s).get(i) else { return lane_menu(tick, x, y) };
    let (b, st, t) = crate::views::toolbar::bst_ticks(f64::from(m.position) * 96.0 / f64::from(ppq(s)));
    let mut items = lane_items(tick);
    items.extend([
        PItem { head: true, ..PItem::new(format!("{} at {b}:{st:02}:{t:02}", m.name), Cmd::Nothing) },
        PItem::new("Delete", Cmd::DeleteMarker(i)),
        PItem::new("Rename...", Cmd::RenameMarker(i)),
        PItem::sep(),
        PItem::new("Move content left", Cmd::MoveContent(i, false)),
        PItem::new("Move content right", Cmd::MoveContent(i, true)),
        PItem::head("Action"),
    ]);
    items.extend(MARKER_ACTIONS.iter().map(|&(l, k)| PItem::new(l, Cmd::MarkerAction(i, k)).mark(Mark::Radio(m.kind == k))));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// An audio clip (not a collapsed group's strip): the clips with fades and gain.
pub fn is_audio_clip(s: &crate::session::Session, g: &PGeom, c: &Clip) -> bool {
    source_kind(s, c.source) == PickKind::Audio && matches!(c.source, ClipSource::Channel(_)) && !g.rows.strip(c.track)
}

/// Pixels of `ms` milliseconds of an audio clip's fades: the song's time at the tempo, as the engine plays them
/// (spec: audio/audio-clips.md "Fades and clip gain"), whatever the channel's pitch or stretch.
pub fn ms_px(s: &crate::session::Session, g: &PGeom, ms: f32) -> f32 {
    (f64::from(ms) / fade_ms_per_tick(s)) as f32 * g.ppt
}

/// Milliseconds of a fade per tick at the tempo.
pub fn fade_ms_per_tick(s: &crate::session::Session) -> f64 {
    secs_per_tick(s) * 1000.0
}

/// Where an audio clip's fades end on screen: (fade-in end x, fade-out start x). The fades the record holds, bits or
/// not: automatic crossfades keep them in step with the clips (`follow_crossfades`), and with the switch off every
/// stored fade plays.
pub fn fade_xs(s: &crate::session::Session, g: &PGeom, c: &Clip) -> (f32, f32) {
    let body = g.body(c);
    let fin = c.stored_fade_in().map_or(0.0, |f| ms_px(s, g, f.0)).min(body.w);
    let fout = c.stored_fade_out().map_or(0.0, |f| ms_px(s, g, f.0)).min(body.w);
    (snap(f64::from(body.x) + f64::from(fin)), snap(f64::from(body.right()) - f64::from(fout)))
}

/// Whether a clip's fade-in (`out` false) or fade-out belongs to the automatic crossfades: the switch is on and the
/// edge's bit is clear. Its length follows the overlaps, and a fade-out's tension too (spec: audio/audio-clips.md
/// "Automatic crossfades"), so it has no tension handle of its own.
pub fn managed(s: &crate::session::Session, c: &Clip, out: bool) -> bool {
    os_model::crossfade::on(&s.project) && !if out { c.fade_out_set() } else { c.fade_in_set() }
}

/// The crossfade audio clip `b` makes with an earlier clip, when there is one to draw: the switch is on, both edges
/// belong to it and each is the other's longest overlap (`os_model::crossfade::partner_before`), neither clip is muted.
pub fn crossfade(s: &crate::session::Session, g: &PGeom, b: usize) -> Option<Xfade> {
    let all = clips(s);
    let c = all.get(b).filter(|c| !c.muted && is_audio_clip(s, g, c))?;
    let a = os_model::crossfade::partner_before(&s.project, all, b).filter(|&a| !all[a].muted)?;
    let (_, t_in) = c.stored_fade_in()?;
    let t_out = all[a].fade_out_tension();
    Some(Xfade { a, b, x0: g.body(c).x, x1: fade_xs(s, g, c).0, t_in, t_out })
}

/// The current arrangement's name.
pub fn arrangement_name(s: &crate::session::Session) -> String {
    s.project.arrangement().and_then(|a| a.name.clone()).unwrap_or_else(|| "Arrangement".into())
}

/// Collapses or expands the group led by track `t` or by `t`'s group track (§6a); no-op for an ungrouped track
/// without members.
pub fn toggle_collapse(s: &mut crate::session::Session, t: u16) {
    let Some(g) = group_track(s, t).or_else(|| is_group_track(s, t).then_some(t)) else { return };
    snapshot(s);
    let tr = &mut arr_mut(s).tracks[usize::from(g)];
    tr.collapsed = !tr.collapsed;
}

/// The CPU switches of the plugin in window `key` (a channel row or an `fx_key`; `None`: no plugin there, or a
/// channel that hosts none, a sampler's or an audio clip's).
pub fn plugin_cpu(s: &crate::session::Session, key: usize) -> Option<os_model::PluginCpu> {
    // A container's member: its own (Lemon Studio's design, `crate::container::Member::cpu`).
    if let Some((t, k, id)) = crate::edit::container::of_key(s, key) {
        return crate::edit::container::member(s, t, k, id).map(|m| m.cpu);
    }
    match crate::plugins::fx_of(key) {
        Some((t, k)) => {
            let m = s.project.mixer.iter().find(|m| m.index == t)?;
            m.slots.get(usize::from(k))?.as_ref().map(|_| m.slot_cpu[usize::from(k)])
        }
        None => s.project.channels.get(key).filter(|c| c.kind == os_model::ChannelKind::Plugin).map(|c| c.misc.cpu),
    }
}

/// The window menu's CPU entries, ticked from the plugin's switches; greyed (with the defaults' ticks) for a channel
/// hosting no plugin, as in the original.
pub(crate) fn fill_cpu(s: &crate::session::Session, key: usize, menu: &mut Popup) {
    let Some(c) = plugin_cpu(s, key) else { return };
    for it in menu.items.iter_mut() {
        let (on, smart) = match &*it.label {
            "Allow threaded processing" => (c.threaded, false),
            "Auto-bypass when silent" => (c.smart_disable, true),
            _ => continue,
        };
        it.cmd = Cmd::PluginCpu(key, smart);
        it.enabled = true;
        it.mark = Mark::Check(on);
    }
}

/// The project's samples as decoded once (a waveform's peaks and length, its tones), shared with the samplers playing
/// them: the session's, since what a clip's length is follows from them (`source_len`).
#[derive(Debug, Default)]
pub struct Samples {
    /// Waveform cache by sample path (`None` = could not be loaded).
    pub waves: RefCell<HashMap<String, Option<Rc<Wave>>>>,
    /// Tones of the same samples for colourful waveforms, analysed when first needed (`tones`).
    pub tones: RefCell<HashMap<String, Option<Rc<os_dsp::tone::Tones>>>>,
    /// A project is loading and its files decode off the UI thread: waveforms wait (spec: ui/project-loading.md).
    pub wait: bool,
}

impl Samples {
    /// The waveform of a sample path (loaded and summarised once).
    pub fn wave(&self, path: &str) -> Option<Rc<Wave>> {
        if let Some(w) = self.waves.borrow().get(path) {
            return w.clone();
        }
        if self.wait {
            return None;
        }
        // The decode is shared with the sampler playing the file (decoded once, while the project loaded).
        let file = crate::plugins::resolve_sample(path);
        let w = os_engine::sample::load_shared(std::path::Path::new(&file)).ok().map(|s| {
            let per = 64usize;
            let peaks = s
                .left
                .chunks(per)
                .zip(s.right.chunks(per))
                .map(|(l, r)| {
                    // The chunk's own range (a DC sample is one value, not a band from zero; §9a signed waveforms).
                    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                    for (a, b) in l.iter().zip(r) {
                        let v = (a + b) * 0.5;
                        lo = lo.min(v);
                        hi = hi.max(v);
                    }
                    (lo, hi)
                })
                .collect();
            Rc::new(Wave { rate: s.rate, frames: s.frames(), per, peaks })
        });
        self.waves.borrow_mut().insert(path.to_string(), w.clone());
        w
    }

    /// The tones of a sample path, block by block (spec: ui/settings-choices.md "Display: Colorful waveforms
    /// (2026-10-16)"): analysed once, the first time a waveform is drawn in colour, and kept with the peaks.
    pub fn tones(&self, path: &str) -> Option<Rc<os_dsp::tone::Tones>> {
        if let Some(t) = self.tones.borrow().get(path) {
            return t.clone();
        }
        if self.wait {
            return None;
        }
        let file = crate::plugins::resolve_sample(path);
        let t = os_engine::sample::load_shared(std::path::Path::new(&file))
            .ok()
            .map(|s| Rc::new(os_dsp::tone::Tones::analyse_stereo(&s.left, &s.right, s.rate)));
        self.tones.borrow_mut().insert(path.to_string(), t.clone());
        t
    }
}

