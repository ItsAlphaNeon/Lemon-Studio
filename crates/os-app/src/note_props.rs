// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Note properties window. A double-click on a note opens it on that note: its seven levels, its
//! slide and portamento switches, its colour group, its start time and length. A double-click on a note of a selection
//! of several notes opens it as "Step properties": level offsets for all of them, nothing else. The window edits a copy;
//! the notes change only on Accept, as one undo step. Reset puts the copy back. The window is `views/note_props.rs`.
// spec: ui/piano-note-properties.md

use os_model::Note;

use crate::app::State;
use crate::piano::{self, Lane};

/// The undo step's name (ui/undo-steps.md).
pub const STEP: &str = "Piano roll note properties edit";

/// The levels the window sets, in its order (§2): PAN, VEL, REL, MOD X, MOD Y, PITCH, REPEAT.
pub const LEVELS: [Lane; 7] =
    [Lane::Pan, Lane::Velocity, Lane::Release, Lane::Cutoff, Lane::Resonance, Lane::FinePitch, Lane::Repeat];

/// A control of the window that is not a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Slide,
    Porta,
    /// The colour tab: opens or closes the grid of colour groups (§3).
    ColourGrid,
    /// A colour group of the grid, 0..=15.
    Colour(u8),
    Reset,
    Accept,
}

/// The window's place and, while it is open, what it edits.
#[derive(Debug, Clone, Default)]
pub struct NotePropsUi {
    /// Top-left corner (logical px).
    pub pos: (f32, f32),
    pub placed: bool,
    session: Option<Session>,
    /// The last press on a note (pattern, note index, when), for the double-click that opens the window.
    last: Option<(u16, usize, std::time::Instant)>,
}

/// An open window: the notes it edits (their ids, the double-clicked one first: the roll stays usable meanwhile, so
/// their places in the list can change), the edited copy of the double-clicked note, and (Step properties) the offset
/// knobs' places.
#[derive(Debug, Clone)]
struct Session {
    pattern: u16,
    targets: Vec<os_model::Id>,
    base: Note,
    draft: Note,
    offsets: [f32; 7],
    grid: bool,
}

impl NotePropsUi {
    /// Whether the window is Step properties: several notes, level offsets only (§4).
    pub fn many(&self) -> bool {
        self.session.as_ref().is_some_and(|se| se.targets.len() > 1)
    }

    pub fn is_open(&self) -> bool {
        self.session.is_some()
    }

    /// Whether the colour grid is open over the window (§3).
    pub fn grid(&self) -> bool {
        self.session.as_ref().is_some_and(|se| se.grid)
    }
}

/// The notes a double-click on note `i` edits (§1): the selection when `i` is in it, else `i` alone; `i` first.
pub fn targets(s: &State, i: usize) -> Vec<usize> {
    let ch = piano::channel(s);
    let sel = s.piano.selection(s.pattern(), ch);
    let mut out = vec![i];
    if sel.contains(&i) {
        out.extend(sel.into_iter().filter(|&j| j != i));
    }
    out
}

/// A press on note `i`: whether it is the second of a double-click (two presses on the note within 450 ms, as the
/// playlist's clips; §1). A press that does not `count` (a tool or modifier that does not open the window), and a
/// drag that moved the notes (`moved`), start over.
pub fn double_click(s: &mut State, i: usize, count: bool) -> bool {
    if !count {
        s.note_props.last = None;
        return false;
    }
    let now = std::time::Instant::now();
    let p = s.pattern();
    let double =
        s.note_props.last.is_some_and(|(lp, li, t)| lp == p && li == i && now.duration_since(t).as_millis() < 450);
    s.note_props.last = (!double).then_some((p, i, now));
    double
}

/// The notes moved under the mouse: the next press is a first click again.
pub fn moved(s: &mut State) {
    s.note_props.last = None;
}

/// Open the window on note `i` (§1).
pub fn open(s: &mut State, i: usize) -> bool {
    // Every note gets its id first (`os_session::control::ids`): the window finds its notes by them at Accept.
    os_session::control::settle_in(&mut s.session);
    let Some(&n) = piano::notes(&s.session).get(i) else { return false };
    let notes = piano::notes(&s.session);
    let targets = targets(s, i).into_iter().filter_map(|j| notes.get(j).map(|m| m.id)).collect();
    s.note_props.session =
        Some(Session { pattern: s.pattern(), targets, base: n, draft: n, offsets: default_offsets(), grid: false });
    true
}

/// Where the window's notes are now in the pattern shown, the double-clicked one first; notes deleted meanwhile are
/// left out.
fn places(s: &State, ids: &[os_model::Id]) -> Vec<usize> {
    let notes = piano::notes(&s.session);
    ids.iter().filter_map(|id| notes.iter().position(|n| !n.id.is_none() && n.id.0 == id.0)).collect()
}

/// The pattern shown is no longer the window's: the window closes, changing nothing (run on the app's tick).
pub fn follow(s: &mut State) -> bool {
    let gone = s.note_props.session.as_ref().is_some_and(|se| se.pattern != s.pattern());
    if gone {
        close(s);
        s.hide(crate::app::Win::NoteProps);
    }
    gone
}

/// What the window edits of a note (§2): its levels, slide and portamento switches and colour group, put on `n`;
/// its place, key and length stay `n`'s own.
fn put_edits(n: &mut Note, from: &Note) {
    for l in LEVELS {
        l.set(n, l.get(from));
    }
    n.set_slide(from.slide());
    n.set_porta(from.porta());
    n.group = n.group & !0x0F | from.colour();
}

fn default_offsets() -> [f32; 7] {
    std::array::from_fn(|k| default_pos(k as u8))
}

/// The note the window shows: the edited copy of the double-clicked note.
pub fn shown(s: &State) -> Option<Note> {
    // Drawn whole until the tick closes a window whose pattern is no longer shown (`follow`).
    s.note_props.session.as_ref().map(|se| se.draft)
}

fn lane(k: u8) -> Lane {
    LEVELS[usize::from(k).min(LEVELS.len() - 1)]
}

/// Level `k` (its place in `LEVELS`) as a knob position 0..1: the copy's value over the property's scale, or the
/// offset knob's place (§4).
pub fn knob(s: &State, k: u8) -> f32 {
    let Some(se) = s.note_props.session.as_ref() else { return default_pos(k) };
    if se.targets.len() > 1 {
        return se.offsets[usize::from(k).min(6)];
    }
    f32::from(lane(k).get(&se.draft)) / lane(k).max()
}

/// Knob `k` turned to position `v` (0..1). Nothing in the roll changes until Accept (§2).
pub fn set_knob(s: &mut State, k: u8, v: f32) {
    let Some(se) = s.note_props.session.as_mut() else { return };
    let v = v.clamp(0.0, 1.0);
    if se.targets.len() > 1 {
        se.offsets[usize::from(k).min(6)] = v;
    } else {
        let l = lane(k);
        l.set(&mut se.draft, (v * l.max()).round() as u8);
    }
}

/// Where knob `k` goes back to: a new note's level (§2), which is also where the offset knobs start (§4).
pub fn default_pos(k: u8) -> f32 {
    let l = lane(k);
    f32::from(l.get(&Note::step(0, 0))) / l.max()
}

/// The hint-bar text of knob `k`.
pub fn knob_text(s: &State, k: u8) -> String {
    let l = lane(k);
    if s.note_props.many() {
        let pos = knob(s, k);
        let d = default_pos(k);
        return match l.center() {
            Some(_) => format!("{} offset: {:+.0}%", l.name(), (pos - d) * 100.0),
            None if d > 0.0 => format!("{} offset: x{:.2}", l.name(), pos / d),
            None => format!("{} offset: {:+.0}", l.name(), pos * l.max()),
        };
    }
    let v = shown(s).map_or(0, |n| l.get(&n));
    match l.center() {
        Some(c) => {
            let d = f32::from(v) - f32::from(c);
            format!("{}: {:+.0}%", l.name(), d / f32::from(c) * 100.0)
        }
        None => format!("{}: {:.0}%", l.name(), f32::from(v) / l.max() * 100.0),
    }
}

/// A level offset at knob place `pos` applied to `v` (§4): a level with a centre (pan, pitch) moves by the knob's
/// distance from its default; the others are scaled by knob over default (velocity measured: ×0.85 for 85 %).
fn offset(l: Lane, v: u8, pos: f32) -> u8 {
    let d = f32::from(l.get(&Note::step(0, 0))) / l.max();
    let out = match l.center() {
        Some(_) => (f32::from(v) + (pos - d) * l.max()).round(),
        // Rounded down, as FL: 122 × 0.85 = 103.7 gave 103 (§4).
        None if d > 0.0 => (f32::from(v) * pos / d + 1e-3).floor(),
        None => (f32::from(v) + pos * l.max()).round(),
    };
    out.clamp(0.0, l.max()) as u8
}

/// A switch or button (§2, §3).
pub fn act(s: &mut State, a: Act) {
    let Some(se) = s.note_props.session.as_mut() else { return };
    let many = se.targets.len() > 1;
    match a {
        Act::Slide if !many => {
            let on = !se.draft.slide();
            se.draft.set_slide(on);
        }
        Act::Porta if !many => {
            let on = !se.draft.porta();
            se.draft.set_porta(on);
        }
        Act::ColourGrid if !many => se.grid = !se.grid,
        Act::Colour(c) if !many => {
            se.draft.group = se.draft.group & !0x0F | c & 0x0F;
            se.grid = false;
        }
        Act::Reset => {
            se.draft = se.base;
            se.offsets = default_offsets();
            se.grid = false;
        }
        _ => {}
    }
}

/// Accept (§2, §4): the copy (or the offsets) go to the notes as one undo step; the window closes. Returns the status
/// message.
pub fn accept(s: &mut State) -> String {
    let Some(se) = s.note_props.session.take() else { return String::new() };
    if se.pattern != s.pattern() {
        return String::new();
    }
    os_session::control::settle_in(&mut s.session);
    let at = places(s, &se.targets);
    let mut notes = piano::notes(&s.session).to_vec();
    if se.targets.len() > 1 {
        for &i in &at {
            let Some(n) = notes.get_mut(i) else { continue };
            for (k, &l) in LEVELS.iter().enumerate() {
                let pos = se.offsets[k];
                if (pos - default_pos(k as u8)).abs() > 1e-6 {
                    let v = offset(l, l.get(n), pos);
                    l.set(n, v);
                }
            }
        }
    } else if let Some(n) = at.first().and_then(|&i| notes.get_mut(i)) {
        put_edits(n, &se.draft);
    }
    if notes.as_slice() == piano::notes(&s.session) {
        return "Note properties: nothing changed".into();
    }
    piano::snapshot_as(&mut s.session, STEP);
    piano::pattern_mut(&mut s.session).notes = notes;
    s.session.dirty = true;
    s.piano.last_controls = at.first().and_then(|&i| piano::notes(&s.session).get(i)).copied();
    let k = at.len();
    format!("Note properties: {k} note{} changed", if k == 1 { "" } else { "s" })
}

/// The window closed without Accept: nothing changes (§2).
pub fn close(s: &mut State) {
    s.note_props.session = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn app_with(notes: Vec<Note>) -> App {
        let mut app = App::new(None);
        app.state.selected_channel = 4;
        piano::pattern_mut(&mut app.state.session).notes = notes;
        app
    }

    fn note(pos: u32, key: u16, vel: u8) -> Note {
        Note { length: 96, key, velocity: vel, ..Note::step(4, pos) }
    }

    fn vels(s: &State) -> Vec<u8> {
        piano::notes(&s.session).iter().map(|n| n.velocity).collect()
    }

    #[test]
    fn one_note_changes_only_on_accept_as_one_undo_step() {
        let mut app = app_with(vec![note(0, 60, 100), note(96, 64, 80)]);
        let s = &mut app.state;
        let ch = piano::channel(s).unwrap();
        s.piano.select(s.pattern(), ch, vec![1]);
        assert!(open(s, 0));
        assert!(!s.note_props.many(), "note 0 is not in the selection");
        set_knob(s, 1, 30.0 / 127.0);
        act(s, Act::Slide);
        act(s, Act::Colour(3));
        assert_eq!(vels(s), vec![100, 80], "no preview");
        assert_eq!(shown(s).unwrap().velocity, 30);
        assert!(accept(s).contains("1 note"));
        let n = piano::notes(&s.session)[0];
        assert!(n.velocity == 30 && n.slide() && n.colour() == 3);
        assert_eq!(piano::notes(&s.session)[1], note(96, 64, 80));
        assert!(piano::undo(s));
        assert_eq!(piano::notes(&s.session)[0], note(0, 60, 100));
    }

    #[test]
    fn step_properties_scale_the_velocity_of_every_selected_note() {
        let mut app = app_with(vec![note(0, 60, 122), note(96, 64, 80), note(192, 67, 60), note(288, 67, 40)]);
        let s = &mut app.state;
        let ch = piano::channel(s).unwrap();
        s.piano.select(s.pattern(), ch, vec![0, 1, 2]);
        assert!(open(s, 1));
        assert!(s.note_props.many());
        assert_eq!(knob(s, 1), default_pos(1), "offsets start at the defaults");
        act(s, Act::Slide);
        set_knob(s, 1, default_pos(1) * 0.85);
        accept(s);
        assert_eq!(vels(s), vec![103, 68, 51, 40], "x0.85 rounded down, as FL: 122 -> 103, 80 -> 68, 60 -> 51");
        assert!(!piano::notes(&s.session)[0].slide(), "no slide switch in Step properties");
    }

    /// The roll stays usable while the window is open: Accept edits the note the window opened on wherever it went,
    /// with the window's levels and switches only, never its place, key or length.
    #[test]
    fn accept_follows_its_note_not_its_place() {
        let mut app = app_with(vec![note(0, 60, 100), note(96, 64, 80)]);
        let s = &mut app.state;
        assert!(open(s, 1));
        set_knob(s, 1, 30.0 / 127.0);
        act(s, Act::Slide);
        // A note drawn before it lands at the list's front, and the note itself is dragged and resized.
        let p = piano::pattern_mut(&mut s.session);
        p.notes.insert(0, note(48, 50, 90));
        (p.notes[2].key, p.notes[2].position, p.notes[2].length) = (66, 192, 48);
        accept(s);
        let n = piano::notes(&s.session);
        assert_eq!(n.len(), 3);
        assert_eq!((n[0], n[1]), (note(48, 50, 90), note(0, 60, 100)), "the other notes are untouched");
        assert_eq!((n[2].velocity, n[2].slide()), (30, true), "the window's levels and switches");
        assert_eq!((n[2].key, n[2].position, n[2].length), (66, 192, 48), "the note's own place");
    }

    /// A note deleted while its window is open is not brought back by Accept.
    #[test]
    fn accept_skips_a_deleted_note() {
        let mut app = app_with(vec![note(0, 60, 100), note(96, 64, 80)]);
        let s = &mut app.state;
        assert!(open(s, 1));
        set_knob(s, 1, 30.0 / 127.0);
        piano::pattern_mut(&mut s.session).notes.remove(1);
        assert!(accept(s).contains("nothing"));
        assert_eq!(piano::notes(&s.session), [note(0, 60, 100)]);
    }

    #[test]
    fn reset_and_close_change_nothing() {
        let mut app = app_with(vec![note(0, 60, 100)]);
        let s = &mut app.state;
        open(s, 0);
        set_knob(s, 0, 0.0);
        act(s, Act::Reset);
        assert!(accept(s).contains("nothing"));
        open(s, 0);
        set_knob(s, 0, 0.0);
        close(s);
        assert_eq!(piano::notes(&s.session)[0], note(0, 60, 100));
    }
}
