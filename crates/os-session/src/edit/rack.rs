// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The session's half of `crate::rack`: what reads and changes the project, apart from the interface (specs
//! `roadmap/control-plane.md`, part 2, step 6).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#![allow(unused_imports)]

use std::collections::{BTreeMap, BTreeSet};
use os_model::{target, ChannelKind, ClipSource, Note, Project, Rgb};
use crate::knob_id::KnobId;

/// Whether row `r` passes the display filter.
pub fn in_filter(s: &crate::session::Session, r: usize) -> bool {
    let group = s.project.channels.get(r).and_then(|c| c.rack.group);
    match filter(&s.project) {
        Filter::All => true,
        Filter::Group(g) => group.is_some() && group == file_group(&s.project, g),
        Filter::Unsorted => group.is_none(),
    }
}

pub fn set_filter(s: &mut crate::session::Session, f: Filter) {
    s.project.write_if(|p| {
        let now = match f {
            Filter::All => None,
            Filter::Group(g) => file_group(p, g),
            Filter::Unsorted => Some(p.unsorted_group()),
        };
        std::mem::replace(&mut p.current_group, now) != now
    });
}

/// Rename filter group: the rack's group `g` named `name`.
pub fn rename_group(s: &mut crate::session::Session, g: usize, name: &str) {
    s.project.write_if(|p| match file_group(p, g).and_then(|i| p.groups.get_mut(i as usize)) {
        Some(n) if n != name => {
            *n = name.to_string();
            true
        }
        _ => false,
    });
}

/// Tests: the groups `groups` in this order, "Unsorted" after them (or at `unsorted_at`), each channel in its first.
#[cfg(any(test, feature = "test-util"))]
pub fn set_groups(s: &mut crate::session::Session, groups: &[Group], unsorted_at: Option<usize>) {
    let p = s.project.get_mut();
    p.groups = groups.iter().map(|g| g.name.clone()).collect();
    let u = unsorted_at.unwrap_or(p.groups.len()).min(p.groups.len());
    p.groups.insert(u, os_model::UNSORTED_GROUP.to_string());
    for (r, c) in p.channels.iter_mut().enumerate() {
        c.rack.group = groups.iter().position(|g| g.members.contains(&r)).map(|g| if g < u { g } else { g + 1 } as u32);
    }
    if groups.is_empty() {
        p.groups.clear();
    }
}

/// An imported MIDI file's channels: a group named `name` first in the list, "Unsorted" last, shown.
pub fn import_group(s: &mut crate::session::Session, name: &str, rows: impl IntoIterator<Item = usize>) {
    s.project.write_if(|p| {
        if let Some(u) = unsorted_at(p) {
            p.groups.remove(u);
            let u = u as u32;
            for c in &mut p.channels {
                if let Some(g) = &mut c.rack.group {
                    if *g > u {
                        *g -= 1;
                    }
                }
            }
        }
        let g = insert_group(p, Some(0), name);
        let last = p.groups.iter().rposition(|g| g == os_model::UNSORTED_GROUP);
        if last.is_some_and(|l| l + 1 < p.groups.len()) {
            let l = last.unwrap();
            p.groups.remove(l);
            p.groups.push(os_model::UNSORTED_GROUP.to_string());
            for c in &mut p.channels {
                if let Some(x) = &mut c.rack.group {
                    if *x > l as u32 {
                        *x -= 1;
                    }
                }
            }
        }
        put_in_group(p, rows, Some(g));
        p.current_group = Some(g);
        true
    });
}

/// The rows the rack shows, top to bottom.
pub fn shown(s: &crate::session::Session) -> Vec<usize> {
    (0..s.project.channels.len()).filter(|&r| in_filter(s, r)).collect()
}

/// Ticks per step.
pub fn per_step(s: &crate::session::Session) -> u32 {
    (u32::from(s.project.ppq.max(4)) / 4).max(1)
}

/// The current pattern's explicit length in steps (`None` = Auto).
pub fn explicit_len(s: &crate::session::Session) -> Option<u32> {
    let ps = per_step(s);
    s.project.pattern(s.pattern()).and_then(|p| p.length).filter(|&l| l > 0).map(|l| (l / ps).max(1))
}

/// Pattern length in steps (§3, §7, §23): the explicit length, else the content rounded up to whole bars of the
/// pattern's own signature (the project's where it has none), at least one bar: the length the engine plays.
pub fn pattern_steps(s: &crate::session::Session) -> u32 {
    if let Some(l) = explicit_len(s) {
        return l;
    }
    s.project.pattern_length(s.pattern()).div_ceil(per_step(s)).max(1)
}

/// Steps in a bar of the project's signature: what the length box's Set to ▸ N bars counts (§23).
pub fn bar_steps(s: &crate::session::Session) -> u32 {
    (s.project.bar_ticks() / per_step(s)).max(1)
}

/// Steps in a beat of the project's bar (16 / denominator for a time signature): the length of the step colour
/// groups (§3, §23).
pub fn beat_steps(s: &crate::session::Session) -> u32 {
    u32::from(s.project.bar().1).max(1)
}

/// Longest pattern the length box sets: 32 bars of the project's signature, at least [`MAX_STEPS`].
pub fn max_steps(s: &crate::session::Session) -> u32 {
    MAX_STEPS.max(32 * bar_steps(s))
}

/// The current pattern's bars up to tick `end`, as `(start, end)` ticks (§6, §23): its own signature's from each
/// marker on, the project's before the first; a marker cuts the bar it falls in.
pub fn pattern_bars(s: &crate::session::Session, end: u32) -> Vec<(u32, u32)> {
    let regions = s.project.pattern_bars(s.pattern());
    let mut bars = Vec::new();
    for (i, &(start, bar, _)) in regions.iter().enumerate() {
        let stop = regions.get(i + 1).map_or(end, |r| r.0.min(end));
        let mut t = start;
        while t < stop {
            bars.push((t, (t + bar).min(stop)));
            t += bar;
        }
    }
    bars
}

/// Set the current pattern's length (`None` = Auto).
pub fn set_len(s: &mut crate::session::Session, steps: Option<u32>) {
    let ps = per_step(s);
    let n = s.pattern().max(1);
    let max = max_steps(s);
    // No undo step, as in FL (measured 2026-10-28).
    crate::edit::patterns::pat_mut(s, n).length = steps.filter(|&v| v > 0).map(|v| v.min(max) * ps);
    s.dirty = true;
}

/// Length box value: 0 = Auto.
pub fn len_value(s: &crate::session::Session) -> u32 {
    explicit_len(s).unwrap_or(0)
}

/// A row's own length in steps: its loop length in loop mode, else the pattern length (§15).
pub fn row_len(s: &crate::session::Session, r: usize) -> u32 {
    if !loop_mode(s) {
        return pattern_steps(s);
    }
    if advanced(s) {
        return unit_len(s, r).unwrap_or_else(|| pattern_steps(s));
    }
    loop_box_len(s, r)
}

/// Under Advanced looping, row `r`'s loop length in the current pattern (`None`: it does not loop there).
pub fn unit_len(s: &crate::session::Session, r: usize) -> Option<u32> {
    let notes = s.project.pattern(s.pattern()).map_or(&[][..], |p| &p.notes[..]);
    // A loop point, else a length of steps (FL's Advanced looping takes both).
    unit_steps(s, notes, ch_index(s, r)?, loop_unit_of(s, r)).or_else(|| loop_len_of(s, r))
}

/// Whether the loop type is Advanced looping.
pub fn advanced(s: &crate::session::Session) -> bool {
    loop_type(s) == 2
}

/// The loop length, in steps, of channel `ch` among `notes` with loop point `unit`: its last used step (a note covers
/// the steps it lasts) rounded up to whole units. `None` without a unit or without notes.
pub fn unit_steps(s: &crate::session::Session, notes: &[Note], ch: u16, unit: Option<LoopUnit>) -> Option<u32> {
    let ps = per_step(s);
    let u = unit?.steps(s).max(1);
    let used = notes.iter().filter(|n| n.channel == ch && !n.muted()).map(|n| (n.position + n.length.max(ps) - 1) / ps + 1).max()?;
    Some(used.div_ceil(u) * u)
}

/// The rows that loop: those with a loop point under Advanced looping, else those with a loop length.
pub fn loop_rows(s: &crate::session::Session) -> Vec<usize> {
    if advanced(s) { loop_units(s).into_keys().chain(loop_lens(s).into_keys()).collect::<BTreeSet<usize>>().into_iter().collect() } else { loop_lens(s).into_keys().collect() }
}

/// What row `r`'s loop box shows: the loop length, or under Advanced looping its loop point ("···" for none).
pub fn loop_box_text(s: &crate::session::Session, r: usize) -> String {
    if advanced(s) { loop_unit_of(s, r).map_or("···", |u| u.name()).to_string() } else { loop_box_len(s, r).to_string() }
}

/// The value a loop box's wheel and drag start from: the loop length, or under Advanced looping the loop point's place
/// in [`LOOP_UNITS`].
pub fn loop_box_value(s: &crate::session::Session, r: usize) -> i32 {
    if advanced(s) {
        let u = loop_unit_of(s, r);
        LOOP_UNITS.iter().position(|&x| x == u).unwrap_or(0) as i32
    } else {
        loop_box_len(s, r) as i32
    }
}

/// A loop box's wheel or drag reached value `v` (as [`loop_box_value`] counts): row `r` takes that length or loop
/// point. Returns the hint to show.
pub fn set_loop_box(s: &mut crate::session::Session, r: usize, v: i32) -> String {
    s.dirty = true;
    if advanced(s) {
        match LOOP_UNITS[v.clamp(0, LOOP_UNITS.len() as i32 - 1) as usize] {
            Some(u) => {
                put_loop(s, r, Some(unit_code(u)));
                format!("Loop point: {}", u.name())
            }
            None => {
                put_loop(s, r, None);
                "Loop point: don't loop".into()
            }
        }
    } else {
        let v = v.clamp(1, MAX_STEPS as i32) as u32;
        put_loop(s, r, Some(v));
        format!("Loop point: {v} steps")
    }
}

/// What a row's loop box shows and its wheel and drag start from: the row's loop length, else the pattern length; also
/// with loop mode off, where the boxes show while "Always show advanced controls" is ticked (§15).
pub fn loop_box_len(s: &crate::session::Session, r: usize) -> u32 {
    loop_len_of(s, r).unwrap_or_else(|| pattern_steps(s)).max(1)
}

/// Channel index (note `channel`) of row `r`.
pub fn ch_index(s: &crate::session::Session, r: usize) -> Option<u16> {
    s.project.channels.get(r).map(|c| c.index)
}

/// The current pattern's notes of row `r`.
pub fn row_notes(s: &crate::session::Session, r: usize) -> Vec<Note> {
    let Some(ch) = ch_index(s, r) else { return Vec::new() };
    s.project.pattern(s.pattern()).map(|p| p.notes.iter().filter(|n| n.channel == ch).copied().collect()).unwrap_or_default()
}

/// Replace row `r`'s notes in the current pattern.
pub fn set_row_notes(s: &mut crate::session::Session, r: usize, notes: Vec<Note>) {
    let Some(ch) = ch_index(s, r) else { return };
    let n = s.pattern().max(1);
    let p = crate::edit::patterns::pat_mut(s, n);
    p.notes.retain(|x| x.channel != ch);
    p.notes.extend(notes.into_iter().map(|x| Note { channel: ch, ..x }));
    p.notes.sort_by_key(|x| (x.position, x.channel));
    s.dirty = true;
}

/// Before a step-sequencer edit of the current pattern: a "Step seq edit" step of the project's undo history
/// (`crate::history`; spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
pub fn snapshot(s: &mut crate::session::Session) {
    let snap = crate::history::Snap::pattern(s, s.pattern().max(1));
    crate::history::record(s, crate::history::STEP_SEQ_EDIT, snap);
}

/// Fill each N steps (§18): the row's notes inside the pattern length become steps 1, 1+N, 1+2N, …
pub fn fill_each(s: &mut crate::session::Session, r: usize, every: u32) {
    let notes = filled(s, r, every);
    set_row_notes(s, r, notes);
}

/// The row's notes `fill_each` leaves.
pub fn filled(s: &crate::session::Session, r: usize, every: u32) -> Vec<Note> {
    let (len, ps) = (row_len(s, r), per_step(s));
    let mut notes: Vec<Note> = row_notes(s, r).into_iter().filter(|n| n.position >= len * ps).collect();
    notes.extend((0..len).step_by(every.max(1) as usize).map(|k| Note::step(0, k * ps)));
    notes
}

/// Write a step sequence (Advanced fill's Accept) over the row's notes inside the sequence length.
pub fn write_sequence(s: &mut crate::session::Session, r: usize, steps: &[bool]) {
    let notes = sequenced(s, r, steps);
    set_row_notes(s, r, notes);
}

/// The row's notes `write_sequence` leaves.
pub fn sequenced(s: &crate::session::Session, r: usize, steps: &[bool]) -> Vec<Note> {
    let ps = per_step(s);
    let len = steps.len() as u32;
    let mut notes: Vec<Note> = row_notes(s, r).into_iter().filter(|n| n.position >= len * ps).collect();
    notes.extend(steps.iter().enumerate().filter(|(_, on)| **on).map(|(k, _)| Note::step(0, k as u32 * ps)));
    notes
}

/// Rotate the row's notes by one step, wrapping inside the pattern length (§18).
pub fn rotate(s: &mut crate::session::Session, r: usize, dir: i32) {
    let notes = rotated(s, r, dir);
    set_row_notes(s, r, notes);
}

/// The row's notes `rotate` leaves.
pub fn rotated(s: &crate::session::Session, r: usize, dir: i32) -> Vec<Note> {
    let (len, ps) = (row_len(s, r), per_step(s));
    let span = i64::from(len * ps);
    row_notes(s, r)
        .into_iter()
        .map(|mut n| {
            if i64::from(n.position) < span {
                n.position = (i64::from(n.position) + i64::from(dir) * i64::from(ps)).rem_euclid(span) as u32;
            }
            n
        })
        .collect()
}

/// Step `k` of channel `ch` (its index) in pattern `n` on or off (`Command::SetStep`): on adds a step note, or brings
/// back the notes a removal muted (Lemon Studio's design); off removes the notes inside the step, or mutes them with
/// `mute` ("Mute removed steps").
pub fn set_step_in(s: &mut crate::session::Session, n: u16, ch: u16, k: u32, on: bool, mute: bool) {
    let ps = per_step(s);
    let p = crate::edit::patterns::pat_mut(s, n);
    let has = p.notes.iter().any(|x| x.channel == ch && x.position / ps == k && !x.muted());
    let at = |x: &Note| x.channel == ch && x.position / ps == k;
    if on && !has {
        if p.notes.iter().any(at) {
            p.notes.iter_mut().filter(|x| at(x)).for_each(|x| x.set_muted(false));
        } else {
            p.notes.push(Note::step(ch, k * ps));
            p.notes.sort_by_key(|x| (x.position, x.channel));
        }
    } else if !on && has {
        if mute {
            p.notes.iter_mut().filter(|x| at(x)).for_each(|x| x.set_muted(true));
        } else {
            p.notes.retain(|x| !at(x));
        }
    }
    s.dirty = true;
}

/// The steps the row shows: on/off for steps `0..len` (used by the Advanced fill dialog).
pub fn row_steps(s: &crate::session::Session, r: usize, len: u32) -> Vec<bool> {
    let ps = per_step(s);
    let notes = row_notes(s, r);
    (0..len).map(|k| notes.iter().any(|n| n.position / ps == k)).collect()
}

pub fn channel_name(s: &crate::session::Session, r: usize) -> String {
    s.project.channels.get(r).map(|c| c.name.clone().unwrap_or_else(|| format!("Channel {}", c.index + 1))).unwrap_or_default()
}

/// "<name> #k" with the lowest free k >= 2 (§18 Clone).
pub fn clone_name(s: &crate::session::Session, r: usize) -> String {
    let name = channel_name(s, r);
    let base = match name.rsplit_once(" #") {
        Some((b, k)) if k.parse::<u32>().is_ok() => b.to_string(),
        _ => name,
    };
    let taken = |cand: &str| s.project.channels.iter().any(|c| c.name.as_deref() == Some(cand));
    (2..).map(|k| format!("{base} #{k}")).find(|c| !taken(c)).expect("unbounded")
}

/// Clone of row `row` (§18): the copy goes right below it, named "<name> #2", with the same settings and mixer track,
/// unlocked and unzipped, no steps; an automation clip's copy has its curve and targets (spec: ui/playlist.md §10a).
/// Its row.
pub fn clone_channel_in(s: &mut crate::session::Session, row: usize, at_end: bool) -> usize {
    let mut c = s.project.channels[row].clone();
    let from = c.index;
    c.id = os_model::Id::NONE;
    c.name = Some(clone_name(s, row));
    (c.rack.locked, c.rack.zipped) = (false, false);
    let auto = s.project.automation(from).cloned();
    let at = if at_end { s.project.channels.len() } else { row + 1 };
    crate::control::command::put_channel_back(s, at, c);
    if let Some(mut a) = auto {
        a.channel = s.project.channels[at].index;
        a.points.iter_mut().for_each(|p| p.id = os_model::Id::NONE);
        s.project.get_mut().automations.push(a);
    }
    at
}

/// The remote control links on rack rows follow them (`map`: each old row's new row, `None` for one gone; spec:
/// audio/automation.md "Targets follow moves"). A link whose row went goes.
pub fn follow_links(s: &mut crate::session::Session, map: &[Option<usize>]) {
    let m = |r: usize| map.get(r).copied().flatten();
    crate::remote::follow(s, |l| moved_link(l, &m));
}

/// Where remote control link target `l` goes as rack rows move (`m`: a row's new row, `None` for one deleted);
/// `None`: its row went.
pub fn moved_link(l: crate::typein::Target, m: &dyn Fn(usize) -> Option<usize>) -> Option<crate::typein::Target> {
    use crate::knob_id::KnobId;
    use crate::typein::Target as T;
    Some(match l {
        T::Knob(KnobId::ChanVol(r)) => T::Knob(KnobId::ChanVol(m(r)?)),
        T::Knob(KnobId::ChanPan(r)) => T::Knob(KnobId::ChanPan(m(r)?)),
        T::Knob(KnobId::AutoLfo(r, k)) => T::Knob(KnobId::AutoLfo(m(r)?, k)),
        T::Knob(KnobId::MidiOut(r, k)) => T::Knob(KnobId::MidiOut(m(r)?, k)),
        T::RackTrack(r) => T::RackTrack(m(r)?),
        T::RackLed(r) => T::RackLed(m(r)?),
        // A channel's plugin is hosted by its row.
        T::Param(key, i) if key < crate::plugins::FX_KEY => T::Param(m(key)?, i),
        l => l,
    })
}

/// The highest declick mode (spec: audio/declick.md).
pub const DECLICK_MAX: u8 = 5;

/// Lemon Sampler's declick mode parameter (its index is the mode).
pub const SAMPLER_DECLICK: &str = "Click removal";

/// Row `row`'s Lemon Sampler takes the channel's declick mode, which it plays in the engine's place (spec:
/// plugins/lemon-sampler.md §7).
pub fn sync_declick(s: &mut crate::session::Session, row: usize) {
    let Some(mode) = s.project.channels.get(row).filter(|c| c.kind.is_sampler()).map(|c| c.sound.declick) else { return };
    s.plugins.set_param_named(row, SAMPLER_DECLICK, f64::from(mode));
    s.params.reread(row);
}

pub fn random_color(s: &mut crate::session::Session, r: usize, salt: u64) {
    if let Some(c) = s.project.get_mut().channels.get_mut(r) {
        c.color = Some(random_color_of(salt));
    }
}

/// A random channel colour (`random_color`), `salt` telling apart the channels coloured at once.
pub fn random_color_of(salt: u64) -> os_model::Rgb {
    crate::edit::patterns::random_color(seed() ^ salt.wrapping_mul(0x9e37_79b9)).for_channel()
}

/// Channel `r` is the only unlocked channel on.
pub fn solo_of(s: &crate::session::Session, r: usize) -> bool {
    s.project.channels.get(r).is_some_and(|c| c.enabled) && s.project.channels.iter().enumerate().all(|(i, c)| i == r || c.rack.locked || !c.enabled)
}

/// Zips `rows` and unzips the others; the project is written only when that changes something.
pub fn set_zipped(s: &mut crate::session::Session, rows: &BTreeSet<usize>) {
    s.project.write_if(|p| {
        let mut changed = false;
        for (r, c) in p.channels.iter_mut().enumerate() {
            changed |= c.rack.zipped != rows.contains(&r);
            c.rack.zipped = rows.contains(&r);
        }
        changed
    });
}

/// Whether row `r`'s mute state is locked.
pub fn is_locked(s: &crate::session::Session, r: usize) -> bool {
    s.project.channels.get(r).is_some_and(|c| c.rack.locked)
}

/// Lock active/mute state: no undo step and no unsaved change, as in FL (`ui/undo-steps.md`), though the file keeps it
/// (`App::unsaved` leaves it out; an undo or redo of another step leaves it as it is, `control::command::Mutes`).
pub fn toggle_lock(s: &mut crate::session::Session, r: usize) {
    if let Some(c) = s.project.get_mut().channels.get_mut(r) {
        c.rack.locked = !c.rack.locked;
    }
}

/// Channels with no notes in any pattern that no playlist clip uses (Select unused channels,
/// spec: ui/unused-channels.md §1).
pub fn unused(s: &crate::session::Session) -> BTreeSet<usize> {
    let used_clip = |i: u16| s.project.arrangements.iter().any(|a| a.clips.iter().any(|c| c.source == ClipSource::Channel(i)));
    s.project
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| !s.project.patterns.iter().any(|p| p.notes.iter().any(|n| n.channel == c.index)) && !used_clip(c.index))
        .map(|(r, _)| r)
        .collect()
}

/// Chokes itself on a channel's menu (§18), as the original does: ticking gives the channel the lowest cut group no
/// channel uses yet, as both its cut and cut by; unticking sets both to none (spec: ui/channel-rack.md §18).
pub fn cut_itself(s: &mut crate::session::Session, r: usize) {
    let Some(group) = cut_itself_group(s, r) else { return };
    if let Some(c) = s.project.get_mut().channels.get_mut(r) {
        c.misc.cut = group;
        c.misc.cut_by = group;
    }
    s.dirty = true;
}

/// The cut and cut-by group Cut itself gives row `r` (`cut_itself`): none when it cuts itself, else the lowest group
/// no channel uses.
pub fn cut_itself_group(s: &crate::session::Session, r: usize) -> Option<u32> {
    let on = s.project.channels.get(r).map(|c| cuts_itself(&c.misc))?;
    if on {
        return Some(0);
    }
    let used: BTreeSet<u32> = s.project.channels.iter().flat_map(|c| [c.misc.cut, c.misc.cut_by]).collect();
    Some((1..).find(|g| !used.contains(g)).unwrap_or(1))
}

/// The last row's channel goes (an undone drop that made it, `history`): `reorder` without the app, whose instrument
/// for the row goes on the next tick (`App::plugin_tick`). A project keeps at least one channel.
pub fn remove_last(s: &mut crate::session::Session) {
    let n = s.project.channels.len();
    if n < 2 {
        return;
    }
    let map: Vec<Option<usize>> = (0..n).map(|r| (r + 1 < n).then_some(r)).collect();
    remap_in(s, &map);
}

/// `push_channel` in the session; the rack's height follows.
pub fn push_channel_in(s: &mut crate::session::Session, mut ch: os_model::Channel) {
    ch.index = s.project.channels.len() as u16;
    s.project.get_mut().channels.push(ch);
    s.touched.views.push(crate::session::View::RackAutosize);
}

/// `remap_state` in the session; the views follow the rows.
pub fn remap_in(s: &mut crate::session::Session, map: &[Option<usize>]) {
    remap_project(s.project.get_mut(), map);
    remap_session(s, map);
    s.touched.views.push(crate::session::View::Rows(map.to_vec()));
}

/// `remap_ui`'s part in the session: the samples queued for loading and the plugin host's requests follow their rows.
pub fn remap_session(s: &mut crate::session::Session, map: &[Option<usize>]) {
    let m = |r: usize| map.get(r).copied().flatten();
    s.sample_loads = std::mem::take(&mut s.sample_loads).into_iter().filter_map(|(r, p)| m(r).map(|r| (r, p))).collect();
    s.hosted_rows = s.hosted_rows.iter().filter_map(|&r| m(r)).collect();
    s.pending_open = s.pending_open.and_then(m);
    s.pending_plugin = s.pending_plugin.and_then(|(r, i)| m(r).map(|r| (r, i)));
    s.dirty = true;
}

/// One undo step for a loop edit in the shown pattern (a drag's press takes it; one that changes nothing goes on
/// release, as the graph editor's).
pub fn loop_step(s: &mut crate::session::Session, name: &'static str) {
    let snap = crate::history::Snap::pattern(s, s.pattern().max(1));
    crate::history::record(s, name, snap);
}

/// The shown pattern, where the rack's loops are.
pub fn shown_pattern(s: &crate::session::Session) -> Option<&os_model::Pattern> {
    s.project.pattern(s.pattern())
}

/// Row `r`'s value in the shown pattern's loops (FL 24.1.1's form: a length, 0, or a loop point).
pub fn loop_value(s: &crate::session::Session, r: usize) -> Option<u32> {
    let ch = ch_index(s, r)?;
    shown_pattern(s)?.loops.iter().find(|l| l.0 == ch).map(|l| l.1)
}

/// Whether the shown pattern is in loop mode (§15).
pub fn loop_mode(s: &crate::session::Session) -> bool {
    shown_pattern(s).is_some_and(|p| p.loop_mode.is_some())
}

/// The loop type (0 Loop step channels, 1 Loop all channels, 2 Advanced looping): the shown pattern's while loop mode
/// is on, else the one last chosen in it (as FL, which keeps that in memory; another pattern shows Loop step channels).
pub fn loop_type(s: &crate::session::Session) -> u8 {
    match shown_pattern(s).and_then(|p| p.loop_mode) {
        Some(code) => loop_type_of(code),
        None if s.loop_type_of == s.pattern() => s.loop_type,
        None => 0,
    }
}

/// Row `r`'s loop length in steps (§15).
pub fn loop_len_of(s: &crate::session::Session, r: usize) -> Option<u32> {
    loop_value(s, r).and_then(length_of)
}

/// Row `r`'s Advanced looping loop point (Bar under Loop all channels is that type's "loops", not a point).
pub fn loop_unit_of(s: &crate::session::Session, r: usize) -> Option<LoopUnit> {
    loop_value(s, r).filter(|&v| !(loop_type(s) == 1 && v == os_model::Pattern::LOOP_BAR)).and_then(unit_of)
}

/// The rows' loop lengths.
pub fn loop_lens(s: &crate::session::Session) -> BTreeMap<usize, u32> {
    (0..s.project.channels.len()).filter_map(|r| Some((r, loop_len_of(s, r)?))).collect()
}

/// The rows' Advanced looping loop points.
pub fn loop_units(s: &crate::session::Session) -> BTreeMap<usize, LoopUnit> {
    (0..s.project.channels.len()).filter_map(|r| Some((r, loop_unit_of(s, r)?))).collect()
}

/// Row `r`'s value in the shown pattern set to `v` (`None`: none; under Loop all channels in loop mode, Bar, as FL
/// writes a channel that loops). Only that row's value changes, so a pattern keeps the file's form elsewhere.
pub fn put_loop(s: &mut crate::session::Session, r: usize, v: Option<u32>) {
    let Some(ch) = ch_index(s, r) else { return };
    let v = v.or((loop_mode(s) && loop_type(s) == 1).then_some(os_model::Pattern::LOOP_BAR));
    if loop_value(s, r) == v {
        return;
    }
    let n = s.pattern();
    let pat = crate::edit::patterns::pat_mut(s, n);
    pat.loops.retain(|l| l.0 != ch);
    if let Some(v) = v {
        pat.loops.push((ch, v));
        pat.loops.sort_by_key(|l| l.0);
    }
}

/// Under Loop all channels in loop mode, every row without a value gets Bar (FL writes one for each channel).
pub fn fill_all(s: &mut crate::session::Session) {
    for r in 0..s.project.channels.len() {
        if loop_value(s, r).is_none() {
            put_loop(s, r, None);
        }
    }
}

/// A change of the shown pattern's loops (`Command::SetLoops`): the mode (0 off, else its code), and channels' values
/// by channel index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoopChange {
    pub mode: Option<u8>,
    pub loops: Vec<(u16, Option<u32>)>,
}

/// What `put_loop` changes: row `r`'s value, or nothing when it has it already.
pub fn loop_put(s: &crate::session::Session, r: usize, v: Option<u32>) -> Option<(u16, Option<u32>)> {
    let ch = ch_index(s, r)?;
    let v = v.or((loop_mode(s) && loop_type(s) == 1).then_some(os_model::Pattern::LOOP_BAR));
    (loop_value(s, r) != v).then_some((ch, v))
}

/// What `set_loop_box` changes, and the hint it shows.
pub fn loop_box_change(s: &crate::session::Session, r: usize, v: i32) -> (Option<(u16, Option<u32>)>, String) {
    if advanced(s) {
        match LOOP_UNITS[v.clamp(0, LOOP_UNITS.len() as i32 - 1) as usize] {
            Some(u) => (loop_put(s, r, Some(unit_code(u))), format!("Loop point: {}", u.name())),
            None => (loop_put(s, r, None), "Loop point: don't loop".into()),
        }
    } else {
        let v = v.clamp(1, MAX_STEPS as i32) as u32;
        (loop_put(s, r, Some(v)), format!("Loop point: {v} steps"))
    }
}

/// What `set_loop_mode` changes.
pub fn loop_mode_change(s: &crate::session::Session, on: bool) -> LoopChange {
    if loop_mode(s) == on {
        return LoopChange::default();
    }
    let t = loop_type(s);
    let mode = if on { LOOP_TYPE_CODES[usize::from(t.min(2))] as u8 } else { 0 };
    let loops = if on && t == 1 { bar_for_all(s) } else { Vec::new() };
    LoopChange { mode: Some(mode), loops }
}

/// What `set_loop_type` changes in the project, the type remembered (`Session::loop_type`) set already.
pub fn loop_type_change(s: &mut crate::session::Session, t: u8) -> LoopChange {
    (s.loop_type, s.loop_type_of) = (t, s.pattern());
    if t == 2 && !loop_mode(s) {
        return loop_mode_change(s, true);
    }
    if !loop_mode(s) {
        return LoopChange::default();
    }
    let loops = if t == 1 { bar_for_all(s) } else { Vec::new() };
    LoopChange { mode: Some(LOOP_TYPE_CODES[usize::from(t.min(2))] as u8), loops }
}

/// Bar for every row without a value (Loop all channels: FL writes one for each channel).
fn bar_for_all(s: &crate::session::Session) -> Vec<(u16, Option<u32>)> {
    (0..s.project.channels.len()).filter(|&r| loop_value(s, r).is_none()).filter_map(|r| Some((ch_index(s, r)?, Some(os_model::Pattern::LOOP_BAR)))).collect()
}

/// The loop icon (§15): loop mode on or off in the shown pattern, its type written while it is on.
pub fn set_loop_mode(s: &mut crate::session::Session, on: bool) {
    if loop_mode(s) == on {
        return;
    }
    let (n, t) = (s.pattern(), loop_type(s));
    crate::edit::patterns::pat_mut(s, n).loop_mode = on.then(|| LOOP_TYPE_CODES[usize::from(t.min(2))]);
    if on && t == 1 {
        fill_all(s);
    }
}

/// The loop menu's Loop type (§7). Advanced looping also turns loop mode on, as in FL (spec: ui/undo-steps.md).
pub fn set_loop_type(s: &mut crate::session::Session, t: u8) {
    (s.loop_type, s.loop_type_of) = (t, s.pattern());
    if t == 2 && !loop_mode(s) {
        set_loop_mode(s, true);
    } else if loop_mode(s) {
        let n = s.pattern();
        crate::edit::patterns::pat_mut(s, n).loop_mode = Some(LOOP_TYPE_CODES[usize::from(t.min(2))]);
        if t == 1 {
            fill_all(s);
        }
    }
}

/// `play_project` on the session.
pub fn play_project_in(s: &crate::session::Session) -> Project {
    let mut p = s.project.clone();
    // Playback reads nothing of the rack's locks and zips: without them, locking or zipping builds no new engine.
    for c in &mut p.channels {
        (c.rack.locked, c.rack.zipped) = (false, false);
    }
    p.tempo = Some(s.tempo());
    // The pattern shown: a song with no clips plays it (os-engine), not the one the project was opened or saved on.
    p.current_pattern = Some(s.pattern().max(1));
    // Each pattern plays its own loops, in its own loop mode (FL keeps them per pattern).
    expand_loops(&mut p, s, None);
    crate::perform::apply(s, &mut p);
    p
}

/// In each pattern in loop mode, a looping row repeats its first loop length's steps up to the end of the pattern (its
/// length as played, §23); notes past the loop length go. The pattern's own loops (FL 24.1.1's form): a length, and
/// under Advanced looping a loop point too, which is the row's notes there rounded up to the unit. `rows`: only those.
pub fn expand_loops(p: &mut Project, s: &crate::session::Session, rows: Option<&[usize]>) {
    let ps = per_step(s);
    let ends: Vec<u32> = p.patterns.iter().map(|pat| p.pattern_length(pat.number)).collect();
    let chans: Vec<u16> = p.channels.iter().enumerate().filter(|(r, _)| rows.is_none_or(|rs| rs.contains(r))).map(|(_, c)| c.index).collect();
    for (pat, &end) in p.patterns.iter_mut().zip(&ends) {
        let Some(code) = pat.loop_mode else { continue };
        let advanced = loop_type_of(code) == 2;
        for &ch in &chans {
            let Some(v) = pat.loops.iter().find(|l| l.0 == ch).map(|l| l.1) else { continue };
            let len = if advanced { unit_steps(s, &pat.notes, ch, unit_of(v)).or(length_of(v)) } else { length_of(v) };
            let Some(len) = len else { continue };
            let span = len.max(1) * ps;
            let src: Vec<Note> = pat.notes.iter().filter(|n| n.channel == ch && n.position < span).copied().collect();
            if src.is_empty() {
                continue;
            }
            pat.notes.retain(|n| n.channel != ch);
            let mut off = 0;
            while off < end {
                pat.notes.extend(src.iter().filter(|n| n.position + off < end).map(|n| Note { position: n.position + off, ..*n }));
                off += span;
            }
            pat.notes.sort_by_key(|n| (n.position, n.channel));
        }
    }
}

/// Longest pattern the length box sets in a 4/4 project (32 bars, §7); see [`max_steps`].
pub const MAX_STEPS: u32 = 512;

/// Display filter (§21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Group(usize),
    Unsorted,
}

/// The file's index of the rack's group `g`.
pub fn file_group(p: &os_model::Project, g: usize) -> Option<u32> {
    let u = p.unsorted_group() as usize;
    (0..p.groups.len()).filter(|&i| i != u).nth(g).map(|i| i as u32)
}

/// Where "Unsorted" is among the rack's groups, if the file lists it before others (`None`: last).
pub fn unsorted_at(p: &os_model::Project) -> Option<usize> {
    p.groups.iter().position(|g| g == os_model::UNSORTED_GROUP)
}

/// The filter shown.
pub fn filter(p: &os_model::Project) -> Filter {
    match p.current_group {
        None => Filter::All,
        Some(i) if i == p.unsorted_group() => Filter::Unsorted,
        Some(i) => rack_group(p, i).map_or(Filter::All, Filter::Group),
    }
}

/// A new group named `name` in the file's list at `at` (`None`: after the others, before "Unsorted" if it is last),
/// with the channels after it renumbered; the list gets "Unsorted" if it had none. Returns its file index.
pub fn insert_group(p: &mut os_model::Project, at: Option<usize>, name: &str) -> u32 {
    if !p.groups.iter().any(|g| g == os_model::UNSORTED_GROUP) {
        p.groups.push(os_model::UNSORTED_GROUP.to_string());
    }
    let u = p.unsorted_group() as usize;
    let at = at.unwrap_or(if u + 1 == p.groups.len() { u } else { p.groups.len() }).min(p.groups.len());
    p.groups.insert(at, name.to_string());
    let at32 = at as u32;
    for c in &mut p.channels {
        if let Some(g) = &mut c.rack.group {
            if *g >= at32 {
                *g += 1;
            }
        }
    }
    if let Some(g) = &mut p.current_group {
        if *g >= at32 {
            *g += 1;
        }
    }
    at32
}

/// Puts `rows` in the file's group `g` (out of any other: a channel is in one).
pub fn put_in_group(p: &mut os_model::Project, rows: impl IntoIterator<Item = usize>, g: Option<u32>) {
    for r in rows {
        if let Some(c) = p.channels.get_mut(r) {
            c.rack.group = g;
        }
    }
}

/// Advanced looping's loop point (§7 Loop type, §15; the original's manual: a row plays to the end of its last used
/// step, beat or bar, then loops; "···" does not loop). How the boxes set it is Lemon Studio's design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopUnit {
    Step,
    Beat,
    Bar,
}

/// The loop box's values under Advanced looping, in its wheel's order: no loop, then the units.
pub const LOOP_UNITS: [Option<LoopUnit>; 4] = [None, Some(LoopUnit::Step), Some(LoopUnit::Beat), Some(LoopUnit::Bar)];

impl LoopUnit {
    pub fn name(self) -> &'static str {
        match self {
            LoopUnit::Step => "Step",
            LoopUnit::Beat => "Beat",
            LoopUnit::Bar => "Bar",
        }
    }

    /// Steps in one unit: a beat and a bar of the project's signature (§23).
    pub fn steps(self, s: &crate::session::Session) -> u32 {
        match self {
            LoopUnit::Step => 1,
            LoopUnit::Beat => beat_steps(s),
            LoopUnit::Bar => bar_steps(s),
        }
    }
}

pub fn seed() -> u64 {
    crate::clip_colors::clock()
}

/// Whether a channel cuts itself: it cuts and is cut by the same group (spec: audio/note-rules.md).
pub fn cuts_itself(m: &os_model::ChannelMisc) -> bool {
    m.cut != 0 && m.cut == m.cut_by
}

/// Reorder / remove channels in the project: `map[old row] = new row`. Every reference follows (notes, control
/// points, automation clips and their targets, playlist clips, layer children); indices become positions.
pub fn remap_project(p: &mut Project, map: &[Option<usize>]) {
    let idx: BTreeMap<u16, u16> =
        p.channels.iter().enumerate().filter_map(|(pos, c)| map.get(pos).copied().flatten().map(|n| (c.index, n as u16))).collect();
    let mut chans: Vec<(usize, os_model::Channel)> =
        std::mem::take(&mut p.channels).into_iter().enumerate().filter_map(|(pos, c)| map.get(pos).copied().flatten().map(|n| (n, c))).collect();
    chans.sort_by_key(|(n, _)| *n);
    p.channels = chans
        .into_iter()
        .map(|(n, mut c)| {
            c.index = n as u16;
            c.layer_children = c.layer_children.iter().filter_map(|k| idx.get(k).copied()).collect();
            c
        })
        .collect();
    for pat in &mut p.patterns {
        pat.notes.retain_mut(|n| idx.get(&n.channel).map(|&m| n.channel = m).is_some());
        // Points of no channel (the tempo's, channel 0x4000) stay as they are.
        pat.controls.retain_mut(|c| {
            c.channel() >= 0x4000 || idx.get(&c.channel()).map(|&m| *c = os_model::ControlPoint::new(c.position, m, c.param(), c.value)).is_some()
        });
    }
    p.automations.retain_mut(|a| {
        let Some(&m) = idx.get(&a.channel) else { return false };
        a.channel = m;
        // Channel targets keep the channel in the high half (spec: flp/events/automation.md "Targets").
        a.targets.retain_mut(|t| {
            let hi = (*t >> 16) as u16;
            if *t == target::TEMPO || hi >= 0x4000 {
                return true;
            }
            idx.get(&hi).map(|&n| *t = target::channel(n, *t as u16)).is_some()
        });
        true
    });
    for arr in &mut p.arrangements {
        arr.clips.retain_mut(|c| match c.source {
            ClipSource::Channel(ch) => idx.get(&ch).map(|&m| c.source = ClipSource::Channel(m)).is_some(),
            ClipSource::Pattern(_) => true,
        });
    }
    // Instrument tracks follow their channel; one whose channel goes is unassigned (spec: ui/playlist.md §6c).
    p.remap_link_channels(|c| idx.get(&c).copied());
    // Initial values: a place below 0x2000 is a channel's index (spec: flp/layouts.md "Mixer places").
    p.init_controls.retain_mut(|c| {
        let hi = (c.target >> 16) as u16;
        hi >= 0x2000 || idx.get(&hi).map(|&n| c.target = u32::from(n) << 16 | (c.target & 0xFFFF)).is_some()
    });
    // Controller links: a channel's target, and a controller that is a channel, follow it; a link whose channel goes
    // goes (spec: audio/automation.md "Targets follow moves").
    p.controller_links.retain_mut(|l| {
        let (hi, source) = ((l.target() >> 16) as u16, l.source());
        if hi < 0x2000 {
            let Some(&n) = idx.get(&hi) else { return false };
            l.set_target(u32::from(n) << 16 | (l.target() & 0xFFFF));
        }
        if source < 0x2000 {
            let Some(&n) = idx.get(&source) else { return false };
            l.set_source(n);
        }
        true
    });
}

/// Loop type values of event 162 for the rack's loop types (Loop step channels, Loop all channels, Advanced looping),
/// as FL 24.1.1 writes them while loop mode is on (spec: flp/events/patterns.md "Loop settings on FL 24.1.1").
pub const LOOP_TYPE_CODES: [u32; 3] = [1, 2, 3];

/// The rack's loop type of an event 162 value (Loop step channels when it is none of them).
pub fn loop_type_of(code: u32) -> u8 {
    LOOP_TYPE_CODES.iter().position(|&c| c == code).unwrap_or(0) as u8
}

/// An Advanced looping loop point as the file stores it, and back.
pub fn unit_code(u: LoopUnit) -> u32 {
    match u {
        LoopUnit::Step => os_model::Pattern::LOOP_STEP,
        LoopUnit::Beat => os_model::Pattern::LOOP_BEAT,
        LoopUnit::Bar => os_model::Pattern::LOOP_BAR,
    }
}

pub fn unit_of(v: u32) -> Option<LoopUnit> {
    match v {
        os_model::Pattern::LOOP_STEP => Some(LoopUnit::Step),
        os_model::Pattern::LOOP_BEAT => Some(LoopUnit::Beat),
        os_model::Pattern::LOOP_BAR => Some(LoopUnit::Bar),
        _ => None,
    }
}

/// A loop length of the file's values (a step count; 0 and the units are none).
pub fn length_of(v: u32) -> Option<u32> {
    (v != 0 && unit_of(v).is_none()).then_some(v)
}

/// The rack's group of the file's index `i`.
pub fn rack_group(p: &os_model::Project, i: u32) -> Option<usize> {
    let u = p.unsorted_group();
    (i != u && (i as usize) < p.groups.len()).then(|| (0..i).filter(|&j| j != u).count())
}

/// A named set of channels (§21).
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub name: String,
    pub members: BTreeSet<usize>,
}

/// The groups as the rack lists them (all but "Unsorted"), each with its rows.
pub fn groups(p: &os_model::Project) -> Vec<Group> {
    let u = p.unsorted_group() as usize;
    p.groups
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != u)
        .map(|(i, name)| Group {
            name: name.clone(),
            members: p.channels.iter().enumerate().filter(|(_, c)| c.rack.group == Some(i as u32)).map(|(r, _)| r).collect(),
        })
        .collect()
}

/// The project as a save writes it: a file without groups shows All (its filter would name no group).
pub fn as_saved(p: &mut os_model::Project) {
    if p.groups.is_empty() {
        p.current_group = None;
    }
}

/// A sample or channel preset at `path` went into the existing channel `ch`: while "Name channels after their sample or plugin" is on, the
/// channel takes the file's name without its extension; off, it keeps its name (spec: ui/settings-choices.md
/// "Miscellaneous: naming, detached plugins and startup"). A channel made new for a file is named after it either way.
pub fn auto_name(st: &crate::settings_file::SettingsStore, ch: &mut os_model::Channel, path: &std::path::Path) {
    if st.check(&crate::prefs::AUTO_NAME_CHANNELS) {
        ch.name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
    }
}

/// Sample `path` went into sampler channel `ch` from the file dialog, the browser or a drop: what its WAV chunks say
/// is read into the channel (spec: ui/settings-choices.md "Advanced: Read sample root note and Read sample tempo
/// information (2026-10-16)"; `os_engine::sample::wav_tags`).
/// - "Read sample root note" on: the root note becomes the `smpl` chunk's unity note, C5 (60) without one (any file
///   that isn't a WAV too). Off: it stays.
/// - "Read sample tempo information" on: an `acid` chunk's beat count (above 0) sets the stretch method to Auto and
///   TIME to the beats, in 1/192 of a beat; PITCH and the multiplier stay. Without one, or off, the stretch stays.
/// - "Auto keep long audio on disk" on: a file of 30 s or more turns Keep on disk on (`keep_on_disk::keep_long`).
///
/// Part of the load: it has no undo step of its own. Project loading and previews don't come here; audio clip channels
/// keep their root note and stretch (not measured).
pub fn read_tags(st: &crate::settings_file::SettingsStore, ch: &mut os_model::Channel, path: &std::path::Path) {
    crate::keep_on_disk::keep_long(st, ch, path);
    use crate::prefs::{READ_ROOT_NOTE, READ_TEMPO};
    let (root, tempo) = (st.check(&READ_ROOT_NOTE), st.check(&READ_TEMPO));
    if ch.kind != os_model::ChannelKind::Sampler || !(root || tempo) {
        return;
    }
    let tags = os_engine::sample::wav_tags(path);
    if root {
        ch.misc.root = tags.unity_note.unwrap_or(60);
    }
    if let Some(beats) = tags.beats.filter(|_| tempo) {
        ch.sound.stretch[0] = ((f64::from(beats) * 192.0) as f32).to_bits();
        ch.sound.stretch[3] = os_model::stretch_mode::AUTO as u32;
    }
}

/// Edits reach playback right away, also during a drag (at most once per tick): every write to the project
/// (`locked::Locked`), and the flag for what playback reads outside it (`Session::dirty`). Returns whether playback was
/// given the project again.
pub fn sync_playback(ses: &mut crate::session::Session) -> bool {
    let written = ses.project.take_written();
    // A write alone that changes nothing playback reads (the project's info, a plugin state stored for a save) builds no
    // new engine.
    let play = (written.is_some() && !ses.dirty).then(|| play_project_in(ses));
    if ses.dirty || play.as_ref().is_some_and(|p| !ses.transport.holds(p)) {
        ses.dirty = false;
        // An edit in the interface (a command's change is its event already; an undo or redo is one too).
        let tempo = ses.tempo();
        if ses.control.changed_since_seen(&ses.project, tempo) {
            ses.control.event(&crate::control::By::User, "edit", "", Vec::new());
        }
        // Also while Play is still starting (not `playing` yet); ignored when stopped.
        ses.transport.update(&play.unwrap_or_else(|| play_project_in(ses)));
        return true;
    }
    false
}






