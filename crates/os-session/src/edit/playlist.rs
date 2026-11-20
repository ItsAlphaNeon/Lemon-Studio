// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The session's half of `crate::playlist`: what reads and changes the project, apart from the interface (specs
//! `roadmap/control-plane.md`, part 2, step 6).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#![allow(unused_imports)]

use crate::icon::ICONS;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use os_model::{
    target, AutoMode, AutoPoint, Arrangement, Automation, Channel, ChannelKind, Clip, ClipSource, PlaylistTrack, Rgb, TimeMarker,
};

/// The icon `owner` shows: the project's (the channel's, the track's or the slot's `lemon_icon` / `slot_icons`, by
/// name; kept in `.lemon`).
pub fn icon_of(p: &os_model::Project, owner: IconOwner) -> Option<crate::icon::Icon> {
    let name = match owner {
        IconOwner::Channel(id) => p.channels.iter().find(|c| c.id.0 == id)?.lemon_icon.as_deref()?,
        IconOwner::Track(t) => p.mixer.iter().find(|m| m.index == t)?.lemon_icon.as_deref()?,
        IconOwner::Slot(t, k) => p.mixer.iter().find(|m| m.index == t)?.slot_icons.get(usize::from(k))?.as_deref()?,
    };
    crate::icon::ICONS.iter().find(|(n, _)| *n == name).map(|&(_, i)| i)
}

/// Sets (`None`: clears) the icon `owner` shows; the project is written only when it changes.
pub fn put_icon(s: &mut crate::session::Session, owner: IconOwner, icon: Option<crate::icon::Icon>) {
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

pub fn ppq(s: &crate::session::Session) -> u32 {
    u32::from(s.project.ppq.max(4))
}

/// The project's bar in ticks (its time signature: `Project::bar_ticks`).
pub fn bar(s: &crate::session::Session) -> u32 {
    s.project.bar_ticks()
}

/// Four quarter notes: the span the playlist's zoom is counted in (`State::playlist_bar_w` is the width of this many
/// ticks, whatever the time signature), so the width of a tick does not change with it.
pub fn ref_bar(s: &crate::session::Session) -> u32 {
    ppq(s) * 4
}

/// The song's bars. The project's bar from the start; a time-signature marker of the arrangement starts bars of its
/// own length at its position (the bar it lands in is cut short there), as a pattern's marker sets the pattern's bars
/// (spec: ui/pattern-time-signature.md). Bar numbers count from 0.
#[derive(Debug, Clone)]
pub struct Bars {
    /// (first tick, bar length in ticks), ascending; the first starts at 0.
    segs: Vec<(u32, u32)>,
}

impl Bars {
    pub fn new(s: &crate::session::Session) -> Bars {
        let ppq = u32::from(s.project.ppq.max(1));
        let mut marks: Vec<&TimeMarker> = markers(s).iter().filter(|m| m.kind == TimeMarker::SIGNATURE && m.numerator > 0 && m.denominator > 0).collect();
        marks.sort_by_key(|m| m.position);
        let mut segs = vec![(0, s.project.bar_ticks())];
        for m in marks {
            let len = (ppq * 4 * u32::from(m.numerator) / u32::from(m.denominator)).max(1);
            match segs.last_mut() {
                Some(l) if l.0 == m.position => l.1 = len,
                _ => segs.push((m.position, len)),
            }
        }
        Bars { segs }
    }

    fn seg(&self, t: u32) -> usize {
        self.segs.partition_point(|&(st, _)| st <= t).saturating_sub(1)
    }

    /// How many bars segment `i` holds (the last one cut short counts), `None` for the open-ended last segment.
    fn count(&self, i: usize) -> Option<u32> {
        let (st, len) = self.segs[i];
        self.segs.get(i + 1).map(|n| (n.0 - st).div_ceil(len))
    }

    /// The start of the bar `t` is in.
    pub fn floor(&self, t: u32) -> u32 {
        let (st, len) = self.segs[self.seg(t)];
        st + (t - st) / len * len
    }

    /// The nearest bar line to `t` (halfway goes up).
    pub fn nearest(&self, t: f32) -> u32 {
        let t = t.max(0.0);
        let f = self.floor(t as u32);
        let i = self.seg(f);
        let (_, len) = self.segs[i];
        let next = self.segs.get(i + 1).map_or(f + len, |n| (f + len).min(n.0));
        if t - f as f32 >= (next - f) as f32 / 2.0 { next } else { f }
    }

    /// The number of the bar `t` is in.
    pub fn index(&self, t: u32) -> i32 {
        let i = self.seg(t);
        let before: u32 = (0..i).filter_map(|k| self.count(k)).sum();
        (before + (t - self.segs[i].0) / self.segs[i].1) as i32
    }

    /// The bars that overlap `t0..t1`: (number, first tick, length).
    pub fn lines(&self, t0: u32, t1: u32) -> Vec<(i32, u32, u32)> {
        let mut out = Vec::new();
        let first = self.seg(t0);
        let mut before: u32 = (0..first).filter_map(|k| self.count(k)).sum();
        for i in first..self.segs.len() {
            let (st, len) = self.segs[i];
            let end = self.segs.get(i + 1).map(|n| n.0);
            if st >= t1 {
                break;
            }
            let mut k = if i == first { (t0 - st) / len } else { 0 };
            loop {
                let at = st + k * len;
                if at >= t1 || end.is_some_and(|e| at >= e) {
                    break;
                }
                out.push(((before + k) as i32, at, end.map_or(len, |e| len.min(e - at))));
                k += 1;
            }
            before += self.count(i).unwrap_or(0);
        }
        out
    }
}

pub fn channel_of(s: &crate::session::Session, ch: u16) -> Option<&Channel> {
    s.project.channel(ch)
}

/// The rack row of the channel an audio clip plays (`None` for pattern and automation clips).
pub fn audio_clip_row(s: &crate::session::Session, c: &Clip) -> Option<usize> {
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
#[cfg(any(test, feature = "test-util"))]
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
pub fn audio_ticks(s: &crate::session::Session, ch: &os_model::Channel) -> f64 {
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
pub fn sample_secs(s: &crate::session::Session, ch: &os_model::Channel) -> f64 {
    ch.sample_path.as_deref().and_then(|p| s.samples.wave(&s.settings, p)).map_or(0.0, |w| w.secs())
}

/// Seconds per tick at the project tempo (the ticks run at `Project::tick_tempo`).
pub fn secs_per_tick(s: &crate::session::Session) -> f64 {
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
                return ((beats * f64::from(ppq(s))).round() as u32).max(ppq(s));
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
    snapshot_as(s, ARRANGER_EDIT);
}

/// The undo step of most playlist edits.
pub const ARRANGER_EDIT: &str = "Arranger edit";

/// `snapshot`, the step named `name` (as the browser's History lists it; spec: ui/settings-choices.md "Undo history:
/// the four checks (2026-10-16)").
pub fn snapshot_as(s: &mut crate::session::Session, name: &'static str) {
    arr_mut(s);
    let snap = crate::history::Snap::Playlist(Box::new(capture(s)));
    crate::history::record(s, name, snap);
}

/// The playlist's undo state now (`snapshot`).
pub fn capture(s: &crate::session::Session) -> Snapshot {
    let (clips, markers, tracks) = s.project.arrangement().map(|a| (a.clips.clone(), a.markers.clone(), a.tracks.clone())).unwrap_or_default();
    let stretch = s.project.channels.iter().map(|c| (c.sound.stretch, c.mixer_track)).collect();
    (clips, markers, tracks, s.project.automations.clone(), stretch)
}

/// Puts an undo state back (undo and redo, `crate::history`).
pub fn restore(s: &mut crate::session::Session, (c, m, t, a, st): Snapshot) {
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
pub fn regroup_copies(s: &mut crate::session::Session, from: usize) {
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

/// Delete track `t` with its clips (the rest move up; an empty track comes in last).
pub fn delete_track_in(s: &mut crate::session::Session, t: u16) {
    let arr = arr_mut(s);
    arr.clips.retain(|c| c.track != t);
    for c in &mut arr.clips {
        if c.track > t {
            c.track -= 1;
        }
    }
    let n = arr.tracks.len();
    for k in usize::from(t)..n - 1 {
        let next = arr.tracks[k + 1].clone();
        arr.tracks[k] = PlaylistTrack { number: arr.tracks[k].number, ..next };
    }
    arr.tracks[n - 1] = PlaylistTrack::new(n as u32);
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

/// Add a point to automation channel `ch` at `beat` with `value`; returns its index.
pub fn add_point(s: &mut crate::session::Session, ch: u16, beat: f64, value: f64) -> Option<usize> {
    let a = s.project.get_mut().automation_mut(ch)?;
    let i = a.points.partition_point(|p| p.beat <= beat);
    // A new point takes the mode / tension of the segment it splits ("new segments use the last tension").
    let (mode, tension) = a.points.get(i).map_or((AutoMode::Single, 0.0), |q| (q.mode, q.tension));
    let flag = if mode.has_tension() { 2 } else { 0 };
    a.points.insert(i, AutoPoint { id: os_model::Id::NONE, beat: beat.max(0.0), value: value.clamp(0.0, 1.0), tension, mode, flag });
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

/// Point `i`'s tension, -1..1 (a drag's live change; the drag's end applies `set_points`).
pub fn set_point_tension(s: &mut crate::session::Session, ch: u16, i: usize, t: f32) {
    if let Some(p) = s.project.get_mut().automation_mut(ch).and_then(|a| a.points.get_mut(i)) {
        p.tension = t.clamp(-1.0, 1.0);
        s.dirty = true;
    }
}

/// Point `i`'s mode, its tension flag following (inside an edit `set_points` applies).
pub fn set_point_mode(s: &mut crate::session::Session, ch: u16, i: usize, m: AutoMode) {
    if let Some(p) = s.project.get_mut().automation_mut(ch).and_then(|a| a.points.get_mut(i)) {
        p.mode = m;
        p.flag = if m.has_tension() && !matches!(m, AutoMode::Stairs | AutoMode::SmoothStairs | AutoMode::Pulse | AutoMode::Wave) { 2 } else { 0 };
    }
}

/// Channel `ch`'s automation points become `points` (an edit's live changes put back before its command).
pub fn put_points(s: &mut crate::session::Session, ch: u16, points: Vec<AutoPoint>) {
    if let Some(a) = s.project.get_mut().automation_mut(ch) {
        a.points = points;
    }
}

/// Channel `ch`'s time stretch length in beats (a stretch-mode resize's live change).
pub fn set_stretch_time(s: &mut crate::session::Session, ch: u16, beats: f64) {
    if let Some(c) = s.project.get_mut().channels.iter_mut().find(|c| c.index == ch) {
        c.sound.set_stretch_time(beats);
    }
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

/// Milliseconds of a fade per tick at the tempo.
pub fn fade_ms_per_tick(s: &crate::session::Session) -> f64 {
    secs_per_tick(s) * 1000.0
}

/// Whether a clip's fade-in (`out` false) or fade-out belongs to the automatic crossfades: the switch is on and the
/// edge's bit is clear. Its length follows the overlaps, and a fade-out's tension too (spec: audio/audio-clips.md
/// "Automatic crossfades"), so it has no tension handle of its own.
pub fn managed(s: &crate::session::Session, c: &Clip, out: bool) -> bool {
    os_model::crossfade::on(&s.project) && !if out { c.fade_out_set() } else { c.fade_in_set() }
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
    pub fn wave(&self, st: &crate::settings_file::SettingsStore, path: &str) -> Option<Rc<Wave>> {
        if let Some(w) = self.waves.borrow().get(path) {
            return w.clone();
        }
        if self.wait {
            return None;
        }
        // The decode is shared with the sampler playing the file (decoded once, while the project loaded).
        let file = crate::plugins::resolve_sample(st, path);
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
    pub fn tones(&self, st: &crate::settings_file::SettingsStore, path: &str) -> Option<Rc<os_dsp::tone::Tones>> {
        if let Some(t) = self.tones.borrow().get(path) {
            return t.clone();
        }
        if self.wait {
            return None;
        }
        let file = crate::plugins::resolve_sample(st, path);
        let t = os_engine::sample::load_shared(std::path::Path::new(&file))
            .ok()
            .map(|s| Rc::new(os_dsp::tone::Tones::analyse_stereo(&s.left, &s.right, s.rate)));
        self.tones.borrow_mut().insert(path.to_string(), t.clone());
        t
    }
}

/// Number of tracks (flp playlist.md).
pub const TRACKS: u16 = 500;

/// Picker panel lists (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PickKind {
    #[default]
    Pattern,
    Audio,
    Automation,
}

/// A waveform summary for drawing audio clips: min/max per `per` frames (left channel + right mixed).
#[derive(Debug, Clone)]
pub struct Wave {
    pub rate: u32,
    pub frames: usize,
    pub per: usize,
    pub peaks: Vec<(f32, f32)>,
}

impl Wave {
    pub fn secs(&self) -> f64 {
        self.frames as f64 / f64::from(self.rate.max(1))
    }
}

/// What a playlist undo step keeps (`crate::history`): the shown arrangement's clips, markers and tracks, the
/// automation data and the channels' stretch settings and mixer tracks (an audio clip placed on an audio track moves
/// its channel to the track's mixer track, `track_links::route_clips`).
pub type Snapshot = (Vec<Clip>, Vec<TimeMarker>, Vec<PlaylistTrack>, Vec<Automation>, Vec<([u32; 4], u16)>);

/// Whose icon: a channel (by id, so it follows the channel when rows move), a mixer slot (track, slot) or a mixer track.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconOwner {
    Channel(u32),
    Slot(u16, u8),
    Track(u16),
}

/// A pastel of FL's audio-clip family (one component 0x68, one 0x7e, the third between), its hue from a hash of
/// `seed` (a Lemon Studio choice for clips whose file has no colour).
pub fn pastel(seed: &str) -> Rgb {
    let h = seed.bytes().fold(2_166_136_261u32, |h, b| (h ^ u32::from(b)).wrapping_mul(16_777_619));
    let hue = (h % 360) as f32 / 60.0;
    let (lo, hi) = (f32::from(0x68u8), f32::from(0x7eu8));
    let mid = lo + (hi - lo) * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (hi, mid, lo),
        1 => (mid, hi, lo),
        2 => (lo, hi, mid),
        3 => (lo, mid, hi),
        4 => (mid, lo, hi),
        _ => (hi, lo, mid),
    };
    Rgb { r: r.round() as u8, g: g.round() as u8, b: b.round() as u8 }
}

/// Per audio channel, the whole sample in ticks and in ms; `None` for a channel whose sample is not loaded.
pub type Fits = HashMap<u16, Option<(f64, f64)>>;

/// The number a new group gets: the lowest from 1 up that none of `used` holds (§3a, checked in FL).
pub fn free_group(used: &[u16]) -> u16 {
    (1..=u16::MAX).find(|g| !used.contains(g)).unwrap_or(u16::MAX)
}

/// The automation value of a tempo (the inverse of [`auto_tempo`]).
pub fn tempo_value(bpm: f64) -> f64 {
    ((bpm * 1000.0 - 10000.0) / 512000.0).clamp(0.0, 1.0)
}

/// Gives the audio clips of `arrangements` their lengths from `fits` (`fit_audio`). Returns whether one changed.
/// The playlist's view as the file keeps it (event 242: track height, time zoom, scroll), when it changed.
pub fn put_view(s: &mut crate::session::Session, v: os_model::PlaylistView) {
    if s.project.playlist_view != Some(v) {
        s.project.get_mut().playlist_view = Some(v);
    }
}

/// Arrangement number `n` becomes the one shown (event 100), a change of the project with no undo step.
pub fn show_arrangement_in(s: &mut crate::session::Session, n: u16) {
    s.project.get_mut().current_arrangement = Some(n);
    s.dirty = true;
}

/// The audio clips fitted now (Restretch all), in the project and in its saved copy alike: lengths worked out, not
/// edited, so no unsaved change. Whether any changed.
pub fn fit_all(s: &mut crate::session::Session) -> bool {
    let Some(fits) = audio_fits(s) else { return false };
    if !s.project.write_if(|p| fit_clips(&mut p.arrangements, &fits)) {
        return false;
    }
    fit_clips(&mut s.saved.0.arrangements, &fits);
    true
}

pub fn fit_clips(arrangements: &mut [os_model::Arrangement], fits: &Fits) -> bool {
    let mut changed = false;
    for a in arrangements {
        for c in &mut a.clips {
            let ClipSource::Channel(ch) = c.source else { continue };
            let Some(&Some((t, ms))) = fits.get(&ch) else { continue };
            let (w0, w1) = c.window.unwrap_or((0.0, ms));
            // Scaled from the whole length, so a point that falls on half a tick stays a tie and rounds up as in FL.
            let tick = |w: f64| (w * t / ms).round();
            let len = (tick(w1) - tick(w0)).max(1.0) as u32;
            if c.length != len {
                c.length = len;
                changed = true;
            }
        }
    }
    changed
}

/// The icon a project file's icon number opens as ([`FLP_ICONS`]), if it has one.
pub fn flp_icon(n: u32) -> Option<crate::icon::Icon> {
    FLP_ICONS.iter().find(|&&(k, _)| k == n).map(|&(_, i)| i)
}

/// Audio clips follow their channel's stretch, pitch and the tempo (ui/playlist.md §9c), then the automatic crossfades
/// follow the clips (`follow_crossfades`). The lengths are worked out, not edited: the saved copy gets them too, so a
/// fit alone (an opened FL project) is not an unsaved change. Returns whether a clip changed.
pub fn follow_audio(s: &mut crate::session::Session) -> bool {
    let mut changed = false;
    if let Some(fits) = audio_fits(s) {
        if s.project.write_if(|p| fit_clips(&mut p.arrangements, &fits)) {
            fit_clips(&mut s.saved.0.arrangements, &fits);
            s.dirty = true;
            changed = true;
        }
    }
    follow_crossfades(s) || changed
}

/// An audio clip's channel as one is made: no sample, name or colour yet, on the master, FL's sampler flags.
pub fn audio_clip_channel() -> Channel {
    Channel {
        id: os_model::Id::NONE,
        index: 0,
        kind: ChannelKind::AudioClip,
        name: None,
        plugin: None,
        plugin_state: None,
        color: None,
        enabled: true,
        volume: 0.78125,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track: 0,
        sample_path: None,
        layer_children: Vec::new(),
        envelopes: Default::default(),
        rack: Default::default(),
        misc: Default::default(),
        sound: os_model::ChannelSound { sampler_flags: Some(os_model::ChannelSound::AUDIO_CLIP_FLAGS), ..Default::default() },
        lemon_icon: None,
    }
}

/// `add_audio_clip_in` on the session (`lock_len` as `place_in` takes it): the playlist's clip source becomes the new
/// channel and the clip is selected (`View::AudioClipAdded`).
pub fn add_audio_clip_on(s: &mut crate::session::Session, path: &str, track: u16, tick: u32, mixer: Option<u16>, lock_len: Option<(ClipSource, u32)>) -> Option<usize> {
    s.samples.wave(&s.settings, path)?;
    // Not on an instrument track: the next free track below it (§6c); on an audio track, through its mixer track.
    let track = crate::track_links::audio_drop_track(s, track, tick, tick + 1);
    let mixer = mixer.or_else(|| crate::track_links::audio_mixer(s, track));
    let index = s.project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
    let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Audio".into());
    let mixer_track = mixer.unwrap_or_else(|| (s.project.channels.iter().map(|c| c.mixer_track).max().unwrap_or(0) + 1).min(crate::edit::mixer::max_insert(&s.project)));
    s.project.get_mut().channels.push(Channel {
        index,
        name: Some(name),
        // Spec: ui/settings-choices.md "Light mode and clip note colours (2026-10-16)" (`clip_colors`).
        color: Some(s.clip_colors.new_channel(index)),
        mixer_track,
        sample_path: Some(path.to_string()),
        ..audio_clip_channel()
    });
    let src = track_info(s, track).and_then(|t| t.clip_lock).unwrap_or(ClipSource::Channel(index));
    let i = place_in(s, src, track, tick, lock_len);
    s.touched.views.push(crate::session::View::AudioClipAdded { channel: index, clip: i });
    Some(i)
}

/// The icon numbers of a project file (a channel's event 155, a mixer track's event 95) whose pictures [`ICONS`] has,
/// and the icon each opens as (Lemon Studio's design, spec: ui/plugin-window.md "Change icon", the pictures in
/// flp/events/mixer.md "Icon numbers"): a snare drum or a drum kit the drum, the piano keys the keys, the microphone,
/// the waveform the wave, the note, a speaker (with a plug or without) the speaker, the jack cable the plug. Other
/// numbers open with no icon.
pub const FLP_ICONS: &[(u32, crate::icon::Icon)] = {
    use crate::icon::Icon::*;
    &[(4, Kick), (80, Kick), (71, PianoRoll), (30, Mic), (75, Wave), (70, Note), (27, Speaker), (29, Speaker), (31, Plug)]
};

/// Automatic crossfades follow the clips and the tempo (spec: audio/audio-clips.md "Automatic crossfades"): set again
/// from the overlaps on the tick an edit reaches playback (clips moved, resized, added, deleted or split, the tempo,
/// undo and redo, a project opened, the switch turned on), before the next undo step is taken, so they undo and redo
/// with the edit that made them. As with the fits, the saved copy gets them too: an opened project's crossfades are
/// not an unsaved change.
pub fn follow_crossfades(s: &mut crate::session::Session) -> bool {
    let tempo = s.tempo();
    if !(s.dirty || s.project.written_at().is_some()) || !s.project.write_if(|p| os_model::crossfade::derive(p, tempo)) {
        return false;
    }
    os_model::crossfade::derive(&mut s.saved.0, s.saved.1);
    true
}

/// `place` of source `src`; `lock_len` is the length the last clip of a source was given (`PlaylistUi::lock_len`).
pub fn place_in(s: &mut crate::session::Session, src: ClipSource, track: u16, tick: u32, lock_len: Option<(ClipSource, u32)>) -> usize {
    let len = match lock_len {
        Some((l, n)) if l == src => n,
        _ => source_len(s, src),
    };
    let audio = source_kind(s, src) == PickKind::Audio && matches!(src, ClipSource::Channel(_));
    let a = arr_mut(s);
    let mut c = Clip::new(tick, len, track, src);
    if audio {
        // FL writes gain 1.0 into a new audio clip (spec: flp/events/playlist.md, byte 52).
        c.set_gain(1.0);
    }
    a.clips.push(c);
    s.dirty = true;
    clips(s).len() - 1
}





