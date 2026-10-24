// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Click-and-hold & special gesture functions (General ▸ Input): with the check on, a held click that doesn't move
//! does more.
//! - Piano roll, Draw tool, on empty grid: the note placed on the press changes type every [`PERIOD`] while the button
//!   stays down, plain, slide, porta, plain again and so on, and keeps the type it has when the button comes up.
//! - Piano roll, on a note (the notes that press moves): after [`PERIOD`] a copy is picked up and follows the mouse as
//!   a moved note does; the release drops it and the original stays. The copy starts one snap step after the
//!   original, so without a move it lands there, never hidden under it.
//! - Playlist, on a clip: the same pick-up of a copy. On empty grid nothing new happens.
//!
//! A move of more than [`SLOP`] pixels first is an ordinary drag and ends the hold; short clicks work as before. The
//! app's tick drives the timer ([`App::hold_tick`]), as the hold happens without mouse events. A gesture is one undo
//! step: the snapshot its press already takes, which a clone renames to its history name ("Piano roll clone note",
//! "Playlist clone clip"; spec: ui/settings-choices.md "Undo history: the four checks").
//!
// spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)"

use std::time::{Duration, Instant};

use os_model::Note;

use crate::app::{App, Drag};
use crate::playlist::{self as pl, PlDrag};
use crate::{piano, prefs};

/// How long a click is held before its note changes type, or its copy is picked up; then again for each change.
pub const PERIOD: Duration = Duration::from_millis(750);

/// Logical pixels the mouse may move, across or down, before a hold becomes an ordinary drag.
pub const SLOP: f32 = 4.0;

/// What a held click does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The Draw tool's new note: its type cycles.
    NewNote,
    /// Notes being moved: copies are picked up.
    Notes,
    /// Clips being moved: copies are picked up.
    Clips,
}

/// A held click that has not moved yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    /// When the button went down.
    pub at: Instant,
    /// Where the mouse was then.
    pub from: (f32, f32),
    pub kind: Kind,
    /// Whole periods counted so far.
    pub periods: u32,
}

/// A note's type as a held click cycles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteType {
    Plain,
    /// A slide note (flags bit 0x0008).
    Slide,
    /// The note's own portamento switch (byte 19, value 16).
    Porta,
}

impl NoteType {
    /// The type after `periods` whole periods of a hold: plain, slide, porta, then over again.
    pub fn after(periods: u32) -> NoteType {
        [NoteType::Plain, NoteType::Slide, NoteType::Porta][(periods % 3) as usize]
    }

    /// Note `n`'s type.
    #[cfg(test)]
    pub fn of(n: &Note) -> NoteType {
        if n.slide() {
            NoteType::Slide
        } else if n.porta() {
            NoteType::Porta
        } else {
            NoteType::Plain
        }
    }

    pub fn apply(self, n: &mut Note) {
        n.set_slide(self == NoteType::Slide);
        n.set_porta(self == NoteType::Porta);
    }
}

impl App {
    /// A press just started a drag of `kind`'s sort: with the check on, it is held from now.
    pub(crate) fn hold_start(&mut self, kind: Kind) {
        self.hold = prefs::check(&prefs::CLICK_AND_HOLD).then(|| Hold { at: Instant::now(), from: self.mouse, kind, periods: 0 });
    }

    /// The mouse moved to (`x`, `y`): further than [`SLOP`] from the press, the hold ends and the drag goes on as one.
    pub(crate) fn hold_moved(&mut self, x: f32, y: f32) {
        if self.hold.is_some_and(|h| (x - h.from.0).abs().max((y - h.from.1).abs()) > SLOP) {
            self.hold = None;
        }
    }

    /// The hold's timer at `now` (the tick's clock; tests pass their own).
    pub fn hold_tick(&mut self, now: Instant) {
        let Some(h) = self.hold else { return };
        let periods = (now.saturating_duration_since(h.at).as_secs_f64() / PERIOD.as_secs_f64()) as u32;
        if periods == h.periods {
            return;
        }
        match h.kind {
            Kind::NewNote => {
                let Some(Drag::NoteMove { orig, .. }) = &self.drag else {
                    self.hold = None;
                    return;
                };
                let Some(&(i, _)) = orig.first() else { return };
                if let Some(n) = piano::pattern_mut(&mut self.state).notes.get_mut(i) {
                    NoteType::after(periods).apply(n);
                }
                self.state.piano.dirty = true;
                self.hold = Some(Hold { periods, ..h });
                self.damage_piano();
            }
            Kind::Notes => {
                self.hold = None;
                self.pick_up_notes(true);
            }
            Kind::Clips => {
                self.hold = None;
                self.pick_up_clips();
            }
        }
    }

    /// The moved notes are copied: the originals go back to where they were, the copies (selected when the
    /// originals were) are dragged from here on, starting one snap step later (a step without a snap) when `step`.
    /// A Shift+drag picks them up on its first move, without the step (spec: ui/piano-roll.md §8).
    pub(crate) fn pick_up_notes(&mut self, step: bool) {
        let Some(Drag::NoteMove { x0, y0, orig }) = self.drag.clone() else { return };
        let (Some(g), Some(ch)) = (self.piano_geom(), piano::channel(&self.state)) else { return };
        let s = &mut self.state;
        crate::history::rename(s, "Piano roll clone note");
        let selected = !s.piano.selection(s.pattern, Some(ch)).is_empty();
        let p = piano::pattern_mut(s);
        for &(i, o) in &orig {
            if let Some(n) = p.notes.get_mut(i) {
                n.position = o.position;
                n.key = o.key;
            }
        }
        let first = p.notes.len();
        p.notes.extend(orig.iter().map(|&(_, o)| o));
        let copies: Vec<(usize, Note)> = orig.iter().enumerate().map(|(k, &(_, o))| (first + k, o)).collect();
        if selected {
            s.piano.select(s.pattern, ch, (first..first + copies.len()).collect());
        }
        let unit = crate::piano_snap::unit(s).unwrap_or(u32::from(s.project.ppq.max(4)) / 4);
        let x0 = if step { x0 - unit as f32 * g.ppt } else { x0 };
        s.piano.dirty = true;
        self.drag = Some(Drag::NoteMove { x0, y0, orig: copies.clone() });
        self.drag_notes(x0, y0, &copies, false);
        self.damage_piano();
    }

    /// The moved clips are copied (`playlist::clone_clips`); the copies are dragged from here on, starting one snap
    /// step later (a step without a snap).
    fn pick_up_clips(&mut self) {
        let Some(Drag::Pl(PlDrag::Move { x0, y0, orig, fresh: false })) = self.drag.clone() else { return };
        let Some(grid) = self.pl_grid() else { return };
        crate::history::rename(&mut self.state, "Playlist clone clip");
        let copies = pl::clone_clips(&mut self.state, &orig);
        let g = pl::PGeom::new(&self.state, grid);
        let unit = match pl::grid_snap(&self.state) {
            pl::Snap::None => g.ppq / 4,
            sn => sn.ticks(g.ppq, g.bar_w),
        };
        // `fresh`: the copies are a change even where they end up.
        let d = PlDrag::Move { x0: x0 - unit as f32 * g.ppt, y0, orig: copies, fresh: true };
        self.drag = Some(Drag::Pl(d.clone()));
        pl::drag_move(self, d, grid);
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Win;
    use crate::prefs::{set_check, CLICK_AND_HOLD};
    use os_ui::{Canvas, Rect};
    use winit::keyboard::Key;

    fn at(app: &App, ms: u64) -> Instant {
        app.hold.expect("held").at + Duration::from_millis(ms)
    }

    fn press(app: &mut App, x: f32, y: f32) {
        app.mouse_move(x, y);
        app.mouse_down();
    }

    fn undo(app: &mut App) {
        app.key(&Key::Character("z".into()), true, false);
    }

    /// The piano roll in front, its first channel's notes empty.
    fn piano_app(c: &mut Canvas) -> App {
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.paint(c, None);
        piano::pattern_mut(&mut app.state).notes.clear();
        app.paint(c, None);
        app
    }

    /// A key in the middle of the visible rows.
    fn mid_key(app: &App) -> i32 {
        let (lo, hi) = app.piano_geom().expect("piano roll shown").visible_keys();
        (lo + hi) / 2
    }

    fn grid_point(app: &App, step: u32, key: i32) -> (f32, f32) {
        let g = app.piano_geom().expect("piano roll shown");
        let x = g.x_of((step * g.step) as f32) + 5.0;
        (x, g.y_of(key) + g.row / 2.0)
    }

    fn notes(app: &App) -> Vec<Note> {
        piano::notes(&app.state).to_vec()
    }

    // spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)"
    /// Held on empty grid with the Draw tool, the new note's type changes every 0.75 s: plain, slide, porta, plain,
    /// slide; the release keeps the type it has (slide, then porta on a second note). Each is one undo step.
    #[test]
    fn a_held_new_note_cycles_its_type() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        set_check(&CLICK_AND_HOLD, true);
        let mut app = piano_app(&mut c);
        let k = mid_key(&app);
        let (x, y) = grid_point(&app, 2, k);
        press(&mut app, x, y);
        assert_eq!(notes(&app).len(), 1, "placed on the press");
        let ty = |app: &App| NoteType::of(&notes(app)[0]);
        let mut seen = Vec::new();
        for ms in [500, 760, 1510, 2290, 3020, 3770] {
            let now = at(&app, ms);
            app.hold_tick(now);
            seen.push(ty(&app));
        }
        use NoteType::{Plain, Porta, Slide};
        assert_eq!(seen, [Plain, Slide, Porta, Plain, Slide, Porta]);
        // Plain at 4.5 s, slide again at 5.25 s; released then, it stays a slide note (flags bit 0x0008).
        let now = at(&app, 4520);
        app.hold_tick(now);
        assert_eq!(ty(&app), Plain);
        let now = at(&app, 5300);
        app.hold_tick(now);
        app.mouse_up();
        assert!(app.hold.is_none());
        assert_eq!(notes(&app)[0].flags & Note::SLIDE, Note::SLIDE);
        assert!(!notes(&app)[0].porta());
        // A second note held to porta keeps its own portamento switch (byte 19 value 16).
        let (x, y) = grid_point(&app, 8, k + 4);
        press(&mut app, x, y);
        let now = at(&app, 1600);
        app.hold_tick(now);
        app.mouse_up();
        let n = *notes(&app).iter().find(|n| i32::from(n.key) == k + 4).unwrap();
        assert_eq!((n.group & Note::PORTA, n.slide()), (Note::PORTA, false));
        // The piano roll draws the two marks: the notes' pictures differ from a plain note's at their right ends.
        app.paint(&mut c, None);
        let g = app.piano_geom().unwrap();
        let shot = |c: &Canvas, r: Rect| -> Vec<u32> {
            let mut px = vec![0u32; 1920 * 1080];
            c.copy_to_xrgb(&mut px);
            let (x0, x1, y0, y1) = (r.right() as usize - 10, r.right() as usize - 1, r.y as usize + 1, r.bottom() as usize - 1);
            (y0..y1).flat_map(|y| px[y * 1920 + x0..y * 1920 + x1].to_vec()).collect()
        };
        let (slide, porta) = (notes(&app)[0], n);
        let marked = (shot(&c, g.note_rect(&slide).unwrap()), shot(&c, g.note_rect(&porta).unwrap()));
        for n in piano::pattern_mut(&mut app.state).notes.iter_mut() {
            NoteType::Plain.apply(n);
        }
        app.paint(&mut c, None);
        assert_ne!(marked.0, shot(&c, g.note_rect(&slide).unwrap()), "the slide triangle");
        assert_ne!(marked.1, shot(&c, g.note_rect(&porta).unwrap()), "the porta stroke");
        // One undo step per note: the second note goes, then the first.
        undo(&mut app);
        assert_eq!(notes(&app).len(), 1);
        undo(&mut app);
        assert!(notes(&app).is_empty());
        set_check(&CLICK_AND_HOLD, false);
    }

    // spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)"
    /// Held on a note for 0.75 s, a copy is picked up; without a move it lands one snap step after the original,
    /// which stays. A move after the pick-up takes the copy along. Undo removes the copy in one step.
    #[test]
    fn a_held_note_is_cloned() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        set_check(&CLICK_AND_HOLD, true);
        let mut app = piano_app(&mut c);
        let (x, y) = grid_point(&app, 2, mid_key(&app));
        press(&mut app, x, y);
        app.mouse_up();
        let orig = notes(&app)[0];
        app.paint(&mut c, None);
        let unit = crate::piano_snap::unit(&app.state).expect("a snap");
        // Held, with a wobble under the slop: copied at 0.75 s, not before.
        let steps = app.state.history.len();
        press(&mut app, x, y);
        app.mouse_move(x + 2.0, y - 1.0);
        let now = at(&app, 700);
        app.hold_tick(now);
        assert_eq!(notes(&app).len(), 1);
        let now = at(&app, 760);
        app.hold_tick(now);
        assert_eq!(notes(&app).len(), 2, "a copy");
        assert!(app.hold.is_none());
        app.mouse_up();
        let ns = notes(&app);
        assert_eq!(ns[0], orig, "the original stays");
        assert_eq!((ns[1].position, ns[1].key), (orig.position + unit, orig.key), "one snap step later");
        assert_eq!(Note { position: orig.position, ..ns[1] }, orig, "a copy of it");
        assert_eq!(app.state.history.len(), steps + 1, "one step");
        assert_eq!(app.state.history.entries().last().map(|e| e.1), Some("Piano roll clone note"));
        undo(&mut app);
        assert_eq!(notes(&app), vec![orig], "one undo step");
        // A move after the pick-up: a semitone up and three steps later than the copy's start.
        app.paint(&mut c, None);
        let g = app.piano_geom().unwrap();
        press(&mut app, x, y);
        let now = at(&app, 800);
        app.hold_tick(now);
        app.mouse_move(x + 3.0 * g.step as f32 * g.ppt, y - g.row);
        app.mouse_up();
        let copy = *notes(&app).iter().find(|n| n.key == orig.key + 1).expect("moved copy");
        assert_eq!(copy.position, orig.position + 4 * unit);
        assert!(notes(&app).contains(&orig));
        set_check(&CLICK_AND_HOLD, false);
    }

    // spec: ui/piano-roll.md §8
    /// Shift+drag on a note drags a copy of it: the original stays, the copy keeps its key while Shift is held, and undo
    /// takes the copy away in one step. A Shift+click without a move copies nothing. On a selected note the whole
    /// selection is copied and the copies become the selection. Shift pressed during a plain drag keeps the notes on
    /// their keys and copies nothing.
    #[test]
    fn shift_drag_clones_notes() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = piano_app(&mut c);
        let k = mid_key(&app);
        let (x, y) = grid_point(&app, 2, k);
        press(&mut app, x, y);
        app.mouse_up();
        let a = notes(&app)[0];
        app.paint(&mut c, None);
        let g = app.piano_geom().unwrap();
        let unit = crate::piano_snap::unit(&app.state).expect("a snap");
        let step = unit as f32 * g.ppt;
        app.shift = true;
        press(&mut app, x, y);
        app.mouse_up();
        assert_eq!(notes(&app), vec![a], "a Shift+click copies nothing");
        let steps = app.state.history.len();
        press(&mut app, x, y);
        app.mouse_move(x + 4.0 * step, y + 2.0 * g.row);
        app.mouse_up();
        app.shift = false;
        assert_eq!(notes(&app), vec![a, Note { position: a.position + 4 * unit, ..a }], "a copy, on the original's key");
        assert_eq!(app.state.history.len(), steps + 1, "one step");
        assert_eq!(app.state.history.entries().last().map(|e| e.1), Some("Piano roll clone note"));
        undo(&mut app);
        assert_eq!(notes(&app), vec![a], "one undo step");
        // Two notes selected: both are copied, and the copies are the selection.
        let (x2, y2) = grid_point(&app, 6, k + 3);
        press(&mut app, x2, y2);
        app.mouse_up();
        let b = *notes(&app).iter().find(|n| *n != &a).unwrap();
        let ch = piano::channel(&app.state).unwrap();
        app.state.piano.select(app.state.pattern, ch, vec![0, 1]);
        app.paint(&mut c, None);
        app.shift = true;
        press(&mut app, x, y);
        app.mouse_move(x + 8.0 * step, y - g.row);
        app.mouse_up();
        app.shift = false;
        let ns = notes(&app);
        let moved = |n: Note| Note { position: n.position + 8 * unit, ..n };
        assert_eq!(ns.len(), 4);
        assert!([a, b, moved(a), moved(b)].iter().all(|n| ns.contains(n)), "{ns:?}");
        let sel: Vec<Note> = app.state.piano.selection(app.state.pattern, Some(ch)).iter().map(|&i| ns[i]).collect();
        assert!(sel.len() == 2 && sel.contains(&moved(a)) && sel.contains(&moved(b)), "the copies are selected");
        undo(&mut app);
        // Shift pressed during a plain drag: the note stays on its key, nothing is copied.
        app.state.piano.clear_selection();
        app.paint(&mut c, None);
        press(&mut app, x, y);
        app.shift = true;
        app.mouse_move(x + step, y + 2.0 * g.row);
        app.mouse_up();
        app.shift = false;
        let ns = notes(&app);
        assert_eq!(ns.len(), 2, "no copy");
        assert!(ns.contains(&Note { position: a.position + unit, ..a }), "moved in time only: {ns:?}");
    }

    // spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)"
    /// A move of more than a few pixels before 0.75 s is an ordinary drag: the hold ends, nothing is copied and the
    /// new note keeps its type.
    #[test]
    fn a_move_cancels_the_hold() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        set_check(&CLICK_AND_HOLD, true);
        let mut app = piano_app(&mut c);
        let (x, y) = grid_point(&app, 2, mid_key(&app));
        press(&mut app, x, y);
        app.mouse_up();
        app.paint(&mut c, None);
        press(&mut app, x, y);
        let t0 = app.hold.unwrap().at;
        app.mouse_move(x + SLOP + 2.0, y);
        assert!(app.hold.is_none(), "an ordinary drag");
        app.hold_tick(t0 + Duration::from_secs(2));
        app.mouse_up();
        assert_eq!(notes(&app).len(), 1, "no copy");
        // A new note dragged early keeps its plain type.
        let (x, y) = grid_point(&app, 10, mid_key(&app) + 2);
        press(&mut app, x, y);
        let t0 = app.hold.unwrap().at;
        app.mouse_move(x, y + 12.0);
        app.hold_tick(t0 + Duration::from_secs(1));
        app.mouse_up();
        assert!(notes(&app).iter().all(|n| NoteType::of(n) == NoteType::Plain));
        set_check(&CLICK_AND_HOLD, false);
    }

    // spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)"
    /// With the check off (the default), holds do nothing: a plain note, no copy of a note or of a clip.
    #[test]
    fn nothing_happens_with_the_check_off() {
        assert!(!prefs::check(&CLICK_AND_HOLD), "off by default");
        assert_eq!(prefs::find_check("Click-and-hold & special gesture functions"), Some(&CLICK_AND_HOLD));
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = piano_app(&mut c);
        let (x, y) = grid_point(&app, 2, mid_key(&app));
        press(&mut app, x, y);
        assert!(app.hold.is_none());
        app.hold_tick(Instant::now() + Duration::from_secs(2));
        app.mouse_up();
        app.paint(&mut c, None);
        press(&mut app, x, y);
        assert!(app.hold.is_none());
        app.hold_tick(Instant::now() + Duration::from_secs(2));
        app.mouse_up();
        assert_eq!(notes(&app).len(), 1);
        assert_eq!(NoteType::of(&notes(&app)[0]), NoteType::Plain);
        let (mut app, clip) = playlist_app(&mut c);
        press(&mut app, clip.0, clip.1);
        assert!(app.hold.is_none());
        app.hold_tick(Instant::now() + Duration::from_secs(2));
        app.mouse_up();
        assert_eq!(pl::clips(&app.state).len(), 1);
    }

    /// The playlist in front with one pattern clip on track 1 at bar 2, and where to press on it.
    fn playlist_app(c: &mut Canvas) -> (App, (f32, f32)) {
        let mut app = App::new(None);
        app.state.z = vec![Win::Playlist];
        app.paint(c, None);
        pl::arr_mut(&mut app.state).clips.clear();
        let bar = 4 * u32::from(app.state.project.ppq);
        pl::place(&mut app.state, 0, bar);
        app.paint(c, None);
        let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
        let body = g.body(&pl::clips(&app.state)[0]);
        (app, (body.cx(), body.cy()))
    }

    // spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)"
    /// Held on a playlist clip for 0.75 s, a copy is picked up and lands one snap step later; the original stays and
    /// undo removes the copy. Held on empty grid: only the usual clip placed on the press.
    #[test]
    fn a_held_clip_is_cloned() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        set_check(&CLICK_AND_HOLD, true);
        let (mut app, (x, y)) = playlist_app(&mut c);
        let orig = pl::clips(&app.state)[0];
        let g = pl::PGeom::new(&app.state, app.pl_grid().unwrap());
        let unit = pl::grid_snap(&app.state).ticks(g.ppq, g.bar_w);
        let steps = app.state.history.len();
        press(&mut app, x, y);
        let now = at(&app, 700);
        app.hold_tick(now);
        assert_eq!(pl::clips(&app.state).len(), 1);
        let now = at(&app, 760);
        app.hold_tick(now);
        app.mouse_up();
        let cs = pl::clips(&app.state).to_vec();
        assert_eq!(cs.len(), 2, "a copy");
        assert_eq!(cs[0], orig, "the original stays");
        assert_eq!((cs[1].position, cs[1].track, cs[1].source), (orig.position + unit, orig.track, orig.source), "one snap step later");
        assert_eq!(app.state.history.len(), steps + 1, "one step");
        assert_eq!(app.state.history.entries().last().map(|e| e.1), Some("Playlist clone clip"));
        undo(&mut app);
        assert_eq!(pl::clips(&app.state).to_vec(), vec![orig], "one undo step");
        // Empty grid: the usual new clip, and nothing more while held.
        app.paint(&mut c, None);
        let empty = (g.grid.x + 10.0, y + 3.0 * g.rows.clip_h());
        press(&mut app, empty.0, empty.1);
        assert!(app.hold.is_none());
        app.hold_tick(Instant::now() + Duration::from_millis(2500));
        app.mouse_up();
        assert_eq!(pl::clips(&app.state).len(), 2, "the placed clip only");
        set_check(&CLICK_AND_HOLD, false);
    }
}
