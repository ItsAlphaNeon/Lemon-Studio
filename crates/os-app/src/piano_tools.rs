// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Piano-roll editing commands: quantize, legato, chop, glue, discard lengths, the Strum, Arpeggiate, Flam, Claw
//! machine, Limit, Flip, Scale levels, LFO, Quantize, Chop and Randomize dialogs, duplicate, mute, ghost channels, time markers, channel cycling and the
//! selection keys, with the Tools menu that lists them.
// spec: ui/piano-roll.md §3a, §10a, §11; ui/piano-keys.md

use os_model::Note;

use crate::app::{App, State};
use crate::piano;
use crate::playlist::{Cmd, Mark, PItem, Popup};

/// A piano-roll command (keyboard shortcut or Tools menu entry). Note values are `1/n` notes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NCmd {
    /// Move starts (and lengths unless `starts_only`) `strength` percent of the way to the snap grid.
    Quantize { strength: u8, starts_only: bool },
    Legato,
    /// Cut notes into snap-unit pieces from their own starts (Ctrl+U, spec: ui/piano-keys.md §4 rule 6; one-tick pieces
    /// without a unit, ui/piano-snap.md §6).
    QuickChop,
    /// Mute (true) or unmute the notes (Alt+M / Alt+Shift+M).
    Mute(bool),
    Glue,
    DiscardLengths,
    Duplicate,
    /// Open the Strum dialog (spec: ui/piano-strum.md).
    Strum,
    /// Open the Arpeggiate dialog (spec: ui/piano-arpeggiate.md).
    Arpeggiate,
    /// Open one of the tool window's dialogs (spec: ui/piano-flam.md, ui/piano-flip.md, ui/piano-level-scaling.md,
    /// ui/piano-limit.md, ui/piano-claw.md, ui/piano-lfo.md, ui/piano-quantize.md, ui/piano-chop.md,
    /// ui/piano-randomize.md).
    Dialog(crate::piano_dlg::Tool),
    Ghosts,
    AddMarker,
    /// The magnet button's menus (spec: ui/piano-snap.md): the snap setting, a stock length for new notes.
    Snap(crate::snap::EditorSnap),
    StockLength(crate::playlist::Snap),
    /// Make a channel rack row the piano roll's channel (the title's channel menu).
    Target(usize),
}

fn item(label: &'static str, c: NCmd) -> PItem {
    PItem::new(label, Cmd::Piano(c))
}

/// The toolbar's Tools menu (§11).
pub fn tools_menu(s: &State, x: f32, y: f32) -> Popup {
    use crate::piano_dlg::Tool;
    use NCmd::*;
    let items = vec![
        PItem::head("File"),
        PItem::new("Import MIDI file...", Cmd::MidiImport(true)),
        PItem::new("Export as MIDI file...", Cmd::MidiExport(true)),
        PItem::head("Edit"),
        item("Duplicate", Duplicate).key("Ctrl+B"),
        item("Glue", Glue).key("Ctrl+G"),
        item("Discard lengths", DiscardLengths).key("Shift+D"),
        PItem::head("Tools"),
        item("Quick quantize", Quantize { strength: 100, starts_only: false }).key("Ctrl+Q"),
        item("Quick quantize start times", Quantize { strength: 100, starts_only: true }).key("Shift+Q"),
        item("Quantize...", Dialog(Tool::Quantize)).key("Alt+Q"),
        item("Quick legato", Legato).key("Ctrl+L"),
        item("Quick chop", QuickChop).key("Ctrl+U"),
        item("Chop...", Dialog(Tool::Chop)).key("Alt+U"),
        item("Strum...", Strum).key("Alt+S"),
        item("Arpeggiate...", Arpeggiate).key("Alt+A"),
        item("Flam...", Dialog(Tool::Flam)).key("Alt+F"),
        item("Claw machine...", Dialog(Tool::Claw)).key("Alt+W"),
        item("Limit...", Dialog(Tool::Limit)).key("Alt+K"),
        item("Flip...", Dialog(Tool::Flip)).key("Alt+Y"),
        item("Randomize...", Dialog(Tool::Randomize)).key("Alt+R"),
        item("Scale levels...", Dialog(Tool::Levels)).key("Alt+X"),
        item("LFO...", Dialog(Tool::Lfo)).key("Alt+O"),
        PItem::head("View"),
        item("Ghost channels", Ghosts).key("Alt+V").mark(Mark::Check(s.piano.ghosts)),
        item("Add time marker", AddMarker).key("Alt+T"),
    ];
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// The keys a held key repeats in the piano roll (spec: ui/piano-keys.md §4 rule 1): Shift / Ctrl / Alt + arrows (not
/// Ctrl+Shift+Up / Down), PgUp / PgDn, Ctrl+B and Ctrl+V.
pub fn repeats(key: &winit::keyboard::Key, ctrl: bool, alt: bool, shift: bool) -> bool {
    use winit::keyboard::{Key, NamedKey};
    match key {
        Key::Named(NamedKey::ArrowUp | NamedKey::ArrowDown) => (shift || ctrl || alt) && !(ctrl && shift),
        Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight) => shift || ctrl || alt,
        Key::Named(NamedKey::PageUp | NamedKey::PageDown) => true,
        Key::Character(c) => ctrl && !alt && (c.eq_ignore_ascii_case("b") || c.eq_ignore_ascii_case("v")),
        _ => false,
    }
}

/// The piano roll's keyboard shortcuts (spec: ui/piano-roll.md §3a, §9, §10a, §11; ui/piano-keys.md). Returns true
/// when the key was taken. Called while the piano roll is shown and the playlist is not in front of it.
pub fn key(app: &mut App, key: &winit::keyboard::Key, ctrl: bool, alt: bool) -> bool {
    use winit::keyboard::{Key, NamedKey};
    let shift = app.shift;
    let is = |c: &str, l: &str| c.eq_ignore_ascii_case(l);
    let cmd = match key {
        Key::Character(c) if ctrl && is(c, "q") => NCmd::Quantize { strength: 100, starts_only: false },
        Key::Character(c) if shift && !ctrl && !alt && is(c, "q") => NCmd::Quantize { strength: 100, starts_only: true },
        Key::Character(c) if ctrl && !alt && is(c, "l") => NCmd::Legato,
        Key::Character(c) if ctrl && !alt && is(c, "u") => NCmd::QuickChop,
        Key::Character(c) if ctrl && !alt && is(c, "g") => NCmd::Glue,
        Key::Character(c) if ctrl && !alt && is(c, "b") => NCmd::Duplicate,
        Key::Character(c) if shift && !ctrl && !alt && is(c, "d") => NCmd::DiscardLengths,
        Key::Character(c) if alt && !ctrl && is(c, "m") => NCmd::Mute(!shift),
        Key::Character(c) if alt && !ctrl && is(c, "q") => NCmd::Dialog(crate::piano_dlg::Tool::Quantize),
        Key::Character(c) if alt && !ctrl && is(c, "u") => NCmd::Dialog(crate::piano_dlg::Tool::Chop),
        Key::Character(c) if alt && !ctrl && is(c, "s") => NCmd::Strum,
        Key::Character(c) if alt && !ctrl && is(c, "a") => NCmd::Arpeggiate,
        Key::Character(c) if alt && !ctrl && is(c, "f") => NCmd::Dialog(crate::piano_dlg::Tool::Flam),
        Key::Character(c) if alt && !ctrl && is(c, "w") => NCmd::Dialog(crate::piano_dlg::Tool::Claw),
        Key::Character(c) if alt && !ctrl && is(c, "k") => NCmd::Dialog(crate::piano_dlg::Tool::Limit),
        Key::Character(c) if alt && !ctrl && is(c, "y") => NCmd::Dialog(crate::piano_dlg::Tool::Flip),
        Key::Character(c) if alt && !ctrl && is(c, "x") => NCmd::Dialog(crate::piano_dlg::Tool::Levels),
        Key::Character(c) if alt && !ctrl && is(c, "o") => NCmd::Dialog(crate::piano_dlg::Tool::Lfo),
        Key::Character(c) if alt && !ctrl && is(c, "r") => NCmd::Dialog(crate::piano_dlg::Tool::Randomize),
        Key::Character(c) if alt && !ctrl && is(c, "v") => NCmd::Ghosts,
        Key::Character(c) if alt && !ctrl && !shift && is(c, "t") => NCmd::AddMarker,
        Key::Character(c) if ctrl && !alt && is(c, "d") => {
            app.state.piano.clear_selection();
            app.invalidate();
            return true;
        }
        Key::Character(c) if ctrl && !alt && is(c, "x") => {
            let s = &mut app.state;
            let sel = s.piano.selection(s.pattern, piano::channel(s));
            if sel.is_empty() {
                return true;
            }
            let all = piano::notes(s);
            s.piano.clipboard = sel.iter().filter_map(|&i| all.get(i).copied()).collect();
            piano::snapshot(s);
            piano::delete(s, sel);
            app.invalidate();
            return true;
        }
        // Selection keys (spec: ui/piano-keys.md §4 rules 11, 12, 14).
        Key::Character(c) if shift && !ctrl && !alt && matches!(c.to_ascii_lowercase().as_str(), "i" | "r" | "m" | "c") => {
            return pick(app, &c.to_ascii_lowercase());
        }
        Key::Named(NamedKey::Enter) if ctrl && !alt && !shift => {
            span_to_range(&mut app.state);
            app.invalidate();
            return true;
        }
        // Channel cycling (§11): Ctrl+K and (typing keyboard off) G / K = previous / next channel, H / J = previous / next
        // channel with notes in the pattern; Shift+K / Shift+H = next channel / next with notes, also with the typing
        // keyboard on (spec: ui/piano-keys.md §4 rule 13).
        Key::Character(c) if ctrl && !alt && !shift && is(c, "k") => return cycle_channel(app, 1, false),
        Key::Character(c) if shift && !ctrl && !alt && is(c, "k") => return cycle_channel(app, 1, false),
        Key::Character(c) if shift && !ctrl && !alt && is(c, "h") => return cycle_channel(app, 1, true),
        Key::Character(c) if !ctrl && !alt && !shift && !app.typing_on() && matches!(c.to_ascii_lowercase().as_str(), "g" | "k" | "h" | "j") => {
            let l = c.to_ascii_lowercase();
            let dir = if l == "g" || l == "h" { -1 } else { 1 };
            return cycle_channel(app, dir, l == "h" || l == "j");
        }
        Key::Named(NamedKey::PageUp) if !ctrl && !alt => return zoom_key(app, 1.0),
        Key::Named(NamedKey::PageDown) if !ctrl && !alt => return zoom_key(app, -1.0),
        // The global snap on / off (spec: ui/global-snap.md §1).
        Key::Named(NamedKey::Backspace) if !ctrl && !alt => {
            app.state.message = Some(crate::snap::toggle(&mut app.state));
            app.invalidate();
            return true;
        }
        _ => return false,
    };
    if let Some(m) = run(app, cmd) {
        app.state.message = Some(m);
    }
    app.invalidate();
    true
}

/// PgUp / PgDn: zoom time around the mouse when it is over the grid, else around the grid's left edge (§3a).
fn zoom_key(app: &mut App, notches: f32) -> bool {
    let Some(grid) = app.piano_grid else { return false };
    let g = piano::Geom::new(&app.state, grid);
    let x = if grid.contains(app.mouse.0, app.mouse.1) { app.mouse.0 } else { grid.x };
    piano::zoom_time(&mut app.state, &g, x, notches);
    app.invalidate();
    true
}

/// Shift+I / R / M / C (spec: ui/piano-keys.md §4 rules 11 and 14): invert the selection among the channel's notes,
/// select one of them or about half of them at random, or the ones of the current colour (the colour group new notes
/// get, from the last note touched).
fn pick(app: &mut App, letter: &str) -> bool {
    let s = &mut app.state;
    let Some(ch) = piano::channel(s) else { return true };
    let sel = s.piano.selection(s.pattern, Some(ch));
    let all = piano::notes(s);
    let mine: Vec<usize> = (0..all.len()).filter(|&i| all[i].channel == ch).collect();
    let mut rng = s.piano.rng;
    let one = |rng: &mut u64| mine.get((next(rng) % mine.len().max(1) as u64) as usize).copied();
    let colour = |n: &Note| n.group & 0x0F; // the colour group: byte 19's low nibble
    let now = s.piano.last_controls.map_or(0, |n| colour(&n));
    let new: Vec<usize> = match letter {
        "i" => mine.iter().copied().filter(|i| !sel.contains(i)).collect(),
        "r" => one(&mut rng).into_iter().collect(),
        "m" => {
            let half: Vec<usize> = mine.iter().copied().filter(|_| next(&mut rng) & 1 == 1).collect();
            if half.is_empty() { one(&mut rng).into_iter().collect() } else { half }
        }
        _ => mine.iter().copied().filter(|&i| colour(&all[i]) == now).collect(),
    };
    s.piano.rng = rng;
    s.piano.select(s.pattern, ch, new);
    app.invalidate();
    true
}

/// Ctrl+Enter (spec: ui/piano-keys.md §4 rule 12): the time selection becomes the selected notes' span (first start to
/// last end) and the play-start marker goes to its start; nothing selected: nothing.
pub fn span_to_range(s: &mut State) {
    let sel = s.piano.selection(s.pattern, piano::channel(s));
    let all = piano::notes(s);
    let picked: Vec<&Note> = sel.iter().filter_map(|&i| all.get(i)).collect();
    let (Some(a), Some(b)) = (picked.iter().map(|n| n.position).min(), picked.iter().map(|n| n.position + piano::shown_len(s, n)).max()) else { return };
    s.piano.range = Some((a, b));
    s.piano.marker = a;
}

/// Select the channel `dir` rows away, wrapping; `with_notes` skips channels without notes in the pattern (§11).
pub(crate) fn cycle_channel(app: &mut App, dir: i32, with_notes: bool) -> bool {
    let s = &mut app.state;
    let n = s.project.channels.len();
    if n == 0 {
        return true;
    }
    let mut row = s.selected_channel.min(n - 1);
    for _ in 0..n {
        row = (row as i32 + dir).rem_euclid(n as i32) as usize;
        if !with_notes || has_notes(s, row) {
            s.message = Some(target(s, row));
            break;
        }
    }
    app.invalidate();
    true
}

/// Whether a channel rack row has notes in the current pattern.
fn has_notes(s: &State, row: usize) -> bool {
    let ch = s.project.channels[row].index;
    piano::notes(s).iter().any(|nt| nt.channel == ch)
}

fn channel_name(s: &State, row: usize) -> String {
    s.project.channels[row].name.clone().unwrap_or_else(|| format!("Channel {}", row + 1))
}

/// Make `row` the piano roll's channel; the rack selects it. Returns the status message.
pub fn target(s: &mut State, row: usize) -> String {
    s.selected_channel = row;
    s.rack.select_only(row);
    s.piano.clear_selection();
    format!("Piano roll: {}", channel_name(s, row))
}

/// The title's channel menu (spec: ui/piano-target-channel.md §2, §5): the channels with notes in the pattern,
/// then one section per rack filter group and Unsorted; a ✓ marks the channels with notes.
pub fn target_menu(s: &State, x: f32, y: f32) -> Popup {
    let n = s.project.channels.len();
    let row_item = |row: usize| {
        let it = PItem::new(channel_name(s, row), Cmd::Piano(NCmd::Target(row)));
        if has_notes(s, row) { it.mark(Mark::Check(true)) } else { it }
    };
    let head = |name: String| PItem { label: name.into(), ..PItem::head("") };
    let mut items = vec![PItem::head("Current pattern")];
    items.extend((0..n).filter(|&r| has_notes(s, r)).map(row_item));
    for g in &s.rack.groups {
        let rows: Vec<usize> = g.members.iter().copied().filter(|&r| r < n).collect();
        if !rows.is_empty() {
            items.push(head(g.name.clone()));
            items.extend(rows.into_iter().map(row_item));
        }
    }
    let unsorted: Vec<usize> = (0..n).filter(|r| !s.rack.groups.iter().any(|g| g.members.contains(r))).collect();
    if !unsorted.is_empty() {
        items.push(PItem::head("Unsorted"));
        items.extend(unsorted.into_iter().map(row_item));
    }
    items.push(PItem::sep());
    items.push(PItem::not_yet("Auto locate"));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// Run a command; returns a status message.
pub fn run(app: &mut App, cmd: NCmd) -> Option<String> {
    app.state.playlist.popup = None;
    let s = &mut app.state;
    // Quick quantize, Discard lengths and Quick chop use the piano roll's snap (spec: ui/piano-snap.md §6,
    // ui/global-snap.md §3).
    let unit = crate::piano_snap::unit(s);
    let done = |changed: bool, what: &str| if changed { format!("Piano roll: {what}") } else { format!("Piano roll: {what}: nothing to change") };
    let msg = match cmd {
        NCmd::Quantize { strength, starts_only } => {
            let k = f32::from(strength.min(100)) / 100.0;
            // Without a unit ((none), Events, Markers) nothing changes.
            let ch = unit.is_some_and(|u| apply(s, |_, ns| ns.into_iter().map(|n| quantize(n, u, k, starts_only)).collect()));
            done(ch, if starts_only { "quantized start times" } else { "quantized" })
        }
        NCmd::Legato => {
            let end = piano::pattern_end(s);
            done(apply(s, |_, ns| legato(ns, end)), "legato")
        }
        NCmd::QuickChop => done(apply(s, |_, ns| chop_from_start(ns, unit.unwrap_or(1))), "chopped"),
        NCmd::Mute(on) => {
            let f = |ns: Vec<Note>| ns.into_iter().map(|mut n| { n.set_muted(on); n }).collect();
            done(apply(s, |_, ns| f(ns)), if on { "muted" } else { "unmuted" })
        }
        NCmd::Glue => done(apply(s, |_, ns| glue(ns)), "glued"),
        NCmd::DiscardLengths => done(apply(s, |_, ns| ns.into_iter().map(|n| Note { length: unit.unwrap_or(0), ..n }).collect()), "discarded lengths"),
        NCmd::Duplicate => {
            duplicate(s);
            return None;
        }
        NCmd::Strum => {
            app.open_strum();
            return None;
        }
        NCmd::Arpeggiate => {
            app.open_arp();
            return None;
        }
        NCmd::Dialog(tool) => {
            app.open_piano_tool(tool);
            return None;
        }
        NCmd::Ghosts => {
            s.piano.ghosts = !s.piano.ghosts;
            format!("Ghost channels {}", if s.piano.ghosts { "on" } else { "off" })
        }
        NCmd::AddMarker => {
            let tick = if app.transport.playing() && !app.transport.song() { app.transport.tick() as u32 } else { app.state.piano.marker };
            return Some(add_marker(&mut app.state, tick));
        }
        NCmd::Snap(p) => return Some(crate::piano_snap::set(s, p)),
        NCmd::StockLength(sn) => return Some(crate::piano_snap::set_length(s, sn)),
        NCmd::Target(row) => {
            if row >= s.project.channels.len() {
                return None;
            }
            let m = target(s, row);
            app.invalidate();
            return Some(m);
        }
    };
    s.piano.dirty = true;
    Some(msg)
}

/// Add a time marker to the current pattern at `tick` snapped down to a step (§10a). Saved with the pattern
/// (flp/events/patterns.md, metadata block), kept sorted by position.
pub fn add_marker(s: &mut State, tick: u32) -> String {
    let step = u32::from(s.project.ppq.max(4)) / 4;
    let tick = tick - tick % step;
    let markers = &mut piano::pattern_mut(s).markers;
    let n = markers.iter().filter(|m| m.kind == 0).count() + 1;
    let name = format!("Marker {n}");
    let at = markers.partition_point(|m| m.position <= tick);
    markers.insert(at, os_model::TimeMarker::new(tick, &name));
    let bar = tick / (step * 16) + 1;
    format!("Piano roll: added \"{name}\" at bar {bar}")
}

/// Ctrl+B (§9): copy the selection (or every note of the channel) right after its end. With a time selection (§10) the
/// copies move by the range's length instead, so the space before and after the notes inside it is kept, and the range
/// moves on over them (as in the playlist); an empty range only moves on.
pub fn duplicate(s: &mut State) {
    let range = s.piano.range.filter(|(a, b)| b > a);
    let idx = match range {
        Some(_) => s.piano.selection(s.pattern, piano::channel(s)),
        None => piano::targets(s),
    };
    let all = piano::notes(s);
    let clip: Vec<Note> = idx.iter().filter_map(|&i| all.get(i).copied()).collect();
    let start = clip.iter().map(|n| n.position).min();
    let end = clip.iter().map(|n| n.position + piano::shown_len(s, n)).max();
    if let Some((r0, r1)) = range {
        let span = r1 - r0;
        if let Some(a) = start {
            piano::paste(s, &clip, a + span);
        }
        s.piano.range = Some((r0 + span, r1 + span));
        s.piano.dirty = true;
    } else if let (Some(a), Some(b)) = (start, end) {
        piano::paste(s, &clip, b.max(a + 1));
        s.piano.dirty = true;
    }
}

/// Replace the target notes (the selection, or every note of the channel) with `f(state, targets)`, as one undo
/// step; a selection then holds the resulting notes. Returns false when nothing changed.
fn apply(s: &mut State, f: impl FnOnce(&State, Vec<Note>) -> Vec<Note>) -> bool {
    apply_as(s, f, Some("Piano roll edit"))
}

/// `apply`, its undo step named `step`; none for a tool's preview (spec: ui/piano-strum.md §1).
pub(crate) fn apply_as(s: &mut State, f: impl FnOnce(&State, Vec<Note>) -> Vec<Note>, step: Option<&'static str>) -> bool {
    let Some(ch) = piano::channel(s) else { return false };
    let idx = piano::targets(s);
    if idx.is_empty() {
        return false;
    }
    let had_sel = !s.piano.selection(s.pattern, Some(ch)).is_empty();
    let all = piano::notes(s);
    let picked: Vec<Note> = idx.iter().filter_map(|&i| all.get(i).copied()).collect();
    let out = f(s, picked.clone());
    if out == picked {
        return false;
    }
    if let Some(name) = step {
        piano::snapshot_as(s, name);
    }
    let p = piano::pattern_mut(s);
    let mut notes: Vec<Note> = p.notes.iter().enumerate().filter(|(i, _)| !idx.contains(i)).map(|(_, n)| *n).collect();
    let start = notes.len();
    notes.extend(out);
    let end = notes.len();
    p.notes = notes;
    if had_sel {
        s.piano.select(s.pattern, ch, (start..end).collect());
    } else {
        s.piano.clear_selection();
    }
    piano::finish_edit(s);
    s.piano.dirty = true;
    true
}

fn len_of(n: &Note, step: u32) -> u32 {
    if n.length == 0 { step } else { n.length }
}

/// Move the start `k` (0..=1) of the way to the nearest `step` line; unless `starts_only`, round the length to
/// whole steps the same way (at least one step).
pub fn quantize(mut n: Note, step: u32, k: f32, starts_only: bool) -> Note {
    let st = step as f32;
    let pos = n.position as f32;
    n.position = (pos + ((pos / st).round() * st - pos) * k).round().max(0.0) as u32;
    if !starts_only && n.length > 0 {
        let len = n.length as f32;
        let q = ((len / st).round() * st).max(st);
        n.length = (len + (q - len) * k).round().max(1.0) as u32;
    }
    n
}

/// Each note lasts until the next later start among `notes`, the last ones until `end` (the pattern's end); slide notes
/// keep their length and are no next start (spec: ui/piano-keys.md §4 rule 5).
pub fn legato(notes: Vec<Note>, end: u32) -> Vec<Note> {
    let mut starts: Vec<u32> = notes.iter().filter(|n| !n.slide()).map(|n| n.position).collect();
    starts.sort_unstable();
    starts.dedup();
    notes
        .into_iter()
        .map(|mut n| {
            let next = starts.iter().copied().find(|&t| t > n.position).unwrap_or(end);
            if !n.slide() && next > n.position {
                n.length = next - n.position;
            }
            n
        })
        .collect()
}

/// Cut every note into `size`-tick pieces counted from its own start, the last piece keeping what is left (spec:
/// ui/piano-keys.md §4 rule 6).
pub fn chop_from_start(notes: Vec<Note>, size: u32) -> Vec<Note> {
    let size = size.max(1);
    let mut out = Vec::new();
    for n in notes {
        let end = n.position + n.length;
        let mut at = n.position;
        while at + size < end {
            out.push(Note { position: at, length: size, ..n });
            at += size;
        }
        out.push(Note { position: at, length: end.max(at) - at, ..n });
    }
    out
}

/// Join notes of the same key that touch or overlap into one (the first note's properties).
pub fn glue(mut notes: Vec<Note>) -> Vec<Note> {
    notes.sort_by_key(|n| (n.key, n.position));
    let mut out: Vec<Note> = Vec::new();
    for n in notes {
        if let Some(last) = out.last_mut().filter(|l| l.key == n.key && l.length > 0 && n.position <= l.position + l.length) {
            let end = (last.position + last.length).max(n.position + n.length);
            last.length = end - last.position;
            continue;
        }
        out.push(n);
    }
    out
}

/// splitmix64: a small, good-enough generator for humanizing.
pub(crate) fn next(rng: &mut u64) -> u64 {
    *rng = rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *rng;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, len: u32, key: u16) -> Note {
        Note { length: len, key, ..Note::step(0, pos) }
    }

    fn spans(ns: &[Note]) -> Vec<(u32, u32, u16)> {
        ns.iter().map(|n| (n.position, n.length, n.key)).collect()
    }

    #[test]
    fn quantize_moves_to_the_grid() {
        let n = quantize(note(30, 40, 60), 24, 1.0, false);
        assert_eq!((n.position, n.length), (24, 48));
        let n = quantize(note(30, 40, 60), 24, 1.0, true);
        assert_eq!((n.position, n.length), (24, 40), "start times only");
        let n = quantize(note(40, 10, 60), 24, 0.5, false);
        assert_eq!((n.position, n.length), (44, 17), "half way: 40 -> 48, 10 -> 24");
    }

    #[test]
    fn legato_chop_glue() {
        let l = legato(vec![note(0, 10, 60), note(0, 10, 64), note(48, 10, 62), note(96, 5, 60)], 384);
        assert_eq!(spans(&l), vec![(0, 48, 60), (0, 48, 64), (48, 48, 62), (96, 288, 60)], "the last note: to the pattern's end");
        // Ctrl+U counts from each note's start (FL 26: F5 581+30 -> 581+24, 605+6; D6 520+72 -> three of 24).
        let c = chop_from_start(vec![note(581, 30, 65), note(520, 72, 74), note(0, 24, 62), note(48, 0, 60)], 24);
        assert_eq!(spans(&c), vec![(581, 24, 65), (605, 6, 65), (520, 24, 74), (544, 24, 74), (568, 24, 74), (0, 24, 62), (48, 0, 60)]);
        let g = glue(vec![note(24, 24, 60), note(0, 24, 60), note(60, 10, 60), note(48, 10, 62)]);
        assert_eq!(spans(&g), vec![(0, 48, 60), (60, 10, 60), (48, 10, 62)]);
    }

    /// Ctrl+L as FL 26 does it (spec: ui/piano-keys.md §2 "Editing"): slide notes stay and are no next start; the
    /// last notes last until the pattern's end.
    #[test]
    fn legato_skips_slides_and_ends_at_the_pattern_end() {
        let mut slide = note(96, 24, 64);
        slide.set_slide(true);
        let l = legato(vec![note(0, 96, 60), slide, note(384, 48, 72), note(800, 48, 60)], 1152);
        assert_eq!(spans(&l), vec![(0, 384, 60), (96, 24, 64), (384, 416, 72), (800, 352, 60)]);
    }
}
