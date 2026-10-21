// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Step edit (OPTIONS > Step edit, Ctrl+E, the toolbar's third mode button): while it is on and the transport is
//! stopped, the notes played on the typing keyboard or a MIDI keyboard are collected while any of them is held, and
//! written into the current pattern at the song position when the last one is released; then the position moves on
//! one step. The switch is a program setting, like its neighbours Auto scrolling and Enable groups: kept in
//! `settings.txt` (`step_edit`), off by default.
// spec: ui/step-edit.md, ui/global-snap.md "Step edit step"

use std::borrow::Cow;

use winit::keyboard::KeyCode;

use crate::app::{App, State, HINT_RECT};
use crate::playlist::Snap;

/// The switch's key in `settings.txt`.
const KEY: &str = "step_edit";

/// The switch (missing: off).
pub fn on() -> bool {
    crate::settings_file::flag(KEY, false)
}

/// Toggles the switch and keeps it; returns the new state.
pub fn toggle() -> bool {
    let on = !on();
    crate::settings_file::set_flag(KEY, on);
    on
}

/// The step edit step in ticks: each note's length and the position's move, one main snap step (ui/global-snap.md).
/// Line counts as one step whatever the zoom shows; Cell and (none), which have no length of their own, do too.
pub fn step(s: &State) -> u32 {
    let unit = match s.global_snap.now {
        Snap::Line | Snap::Cell | Snap::None => Snap::Step,
        sn => sn,
    };
    unit.ticks(u32::from(s.project.ppq.max(4)), 0.0)
}

/// Where a held note comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// A typing-keyboard key.
    Typing(KeyCode),
    /// A MIDI keyboard's key.
    Midi(u8),
    /// A MIDI key on the Omni preview MIDI channel (spec: ui/midi-settings.md §3).
    Omni(u8),
    /// A Touch controller pad or key (spec: ui/touch-controller.md §2).
    Touch(u8),
}

/// The chord being collected: the notes held now, and every key pressed since the first of them, with its velocity.
#[derive(Debug, Default)]
pub struct Chord {
    held: Vec<Src>,
    keys: Vec<(u16, u8)>,
}

impl Chord {
    /// A note went down: it is held, and its key joins the chord (once; a key pressed again takes the new velocity).
    pub fn down(&mut self, src: Src, key: u16, velocity: u8) {
        if !self.held.contains(&src) {
            self.held.push(src);
        }
        match self.keys.iter_mut().find(|k| k.0 == key) {
            Some(k) => k.1 = velocity,
            None => self.keys.push((key, velocity)),
        }
    }

    /// A note went up: when it was the last one held, the chord to write, in the order pressed. A note not collected
    /// (pressed while step editing was off or playing) gives nothing.
    pub fn up(&mut self, src: Src) -> Option<Vec<(u16, u8)>> {
        let i = self.held.iter().position(|&h| h == src)?;
        self.held.remove(i);
        self.held.is_empty().then(|| std::mem::take(&mut self.keys))
    }

    /// Forgets the held notes and the chord: nothing is written.
    pub fn clear(&mut self) {
        self.held.clear();
        self.keys.clear();
    }
}

impl App {
    /// Step edit switch (toolbar, OPTIONS menu, Ctrl+E); a chord being collected is dropped.
    pub fn toggle_step_edit(&mut self) {
        toggle();
        self.step_chord.clear();
        self.invalidate();
    }

    /// Whether step editing takes notes now: the switch is on and the transport is stopped (while playing it writes
    /// nothing: Lemon Studio's choice).
    fn step_edit_live(&self) -> bool {
        on() && !self.transport.playing()
    }

    /// A note started sounding (typing keyboard or MIDI): collected while step editing takes notes.
    pub(crate) fn step_edit_down(&mut self, src: Src, key: u16, velocity: u8) {
        self.step_edit_down_with(src, key, velocity, crate::record::Extra::default());
    }

    /// `step_edit_down` for a note with more than a key and velocity (a MIDI note's Mod X / Y, another channel).
    pub(crate) fn step_edit_down_with(&mut self, src: Src, key: u16, velocity: u8, extra: crate::record::Extra) {
        // While playing, Record takes the note instead (spec: ui/recording.md §2 Taking notes).
        self.record_down_with(src, key, velocity, extra);
        if self.step_edit_live() && extra.channel.is_none() {
            self.step_chord.down(src, key, velocity);
        }
    }

    /// A note was released: the release of the last held one writes the chord.
    pub(crate) fn step_edit_up(&mut self, src: Src) {
        self.step_edit_up_with(src, None);
    }

    /// `step_edit_up` with the note's release velocity, when it has one.
    pub(crate) fn step_edit_up_with(&mut self, src: Src, release: Option<u8>) {
        self.record_up_with(src, release);
        let Some(keys) = self.step_chord.up(src) else { return };
        if self.step_edit_live() && !keys.is_empty() {
            self.step_edit_write(&keys);
        }
    }

    /// Writes `keys` as notes at the song position, one step long, for the selected channel; they become the
    /// selection, the position moves on one step (the start is where the position was, not snapped). One undo step.
    /// In SONG mode the notes go where the pattern plays at the song position, as the piano roll shows it
    /// (`piano::pattern_tick_in_song`); where no clip of it plays, nothing is written.
    fn step_edit_write(&mut self, keys: &[(u16, u8)]) {
        let s = &mut self.state;
        let Some(ch) = crate::piano::channel(s) else { return };
        let pos = crate::song_pos::stored(s);
        let at = if s.song_mode {
            let Some(t) = crate::piano::pattern_tick_in_song(s, f64::from(pos)) else {
                let m = format!("Step edit: {} does not play at the song position", crate::patterns::name_of(s, s.pattern));
                return self.say(m);
            };
            t as u32
        } else {
            pos
        };
        let len = step(s);
        let snap = crate::history::Snap::pattern(s, s.pattern);
        crate::history::record(s, crate::history::STEP_EDIT, snap);
        let mut new = Vec::with_capacity(keys.len());
        for &(key, velocity) in keys {
            let i = crate::piano::add_note(s, ch, at, i32::from(key));
            let n = &mut crate::piano::pattern_mut(s).notes[i];
            (n.length, n.velocity) = (len, velocity);
            new.push(i);
        }
        s.piano.select(s.pattern, ch, new);
        s.piano.dirty = true;
        let to = pos + len;
        if s.song_mode {
            s.playlist.pos = to;
            // The initial values follow a song position move, as with the slider (`App::set_song_pos`).
            crate::init::apply(s);
        } else {
            s.piano.marker = to;
        }
        self.hint = Cow::Owned(crate::song_pos::hint(&self.state, to));
        self.damage_rect(HINT_RECT);
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, Win};
    use crate::midi_in::Msg;
    use crate::piano;
    use crate::views::menus::{bar_model, item_at, Mark, OPTIONS_MENU};
    use os_ui::{Canvas, Rect};
    use winit::keyboard::Key;

    /// A new project in PAT mode with the piano roll in front and an empty pattern; the typing keyboard is on and step
    /// edit off, as at start; then Ctrl+E turns step edit on.
    fn app() -> App {
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.state.song_mode = false;
        app.state.selected_channel = 0;
        app.state.project.patterns.iter_mut().for_each(|p| p.notes.clear());
        assert!(app.typing_on() && !on(), "the typing keyboard is on and step edit off at start");
        ctrl_e(&mut app);
        assert!(on());
        app
    }

    fn ctrl_e(app: &mut App) {
        app.key(&Key::Character("e".into()), true, false);
    }

    /// A typing-keyboard key going down or up, as the shell sends it.
    fn typed(app: &mut App, code: KeyCode, down: bool) {
        assert!(app.typing_key(code, down, false, false), "the typing keyboard takes {code:?}");
    }

    /// A key pressed and released.
    fn tap(app: &mut App, code: KeyCode) {
        typed(app, code, true);
        typed(app, code, false);
    }

    /// The pattern's notes as (position, length, key, velocity).
    fn notes(app: &App) -> Vec<(u32, u32, u16, u8)> {
        piano::notes(&app.state).iter().map(|n| (n.position, n.length, n.key, n.velocity)).collect()
    }

    fn selected(app: &App) -> Vec<usize> {
        app.state.piano.selection(app.state.pattern, piano::channel(&app.state))
    }

    /// One step in ticks.
    fn one_step(app: &App) -> u32 {
        u32::from(app.state.project.ppq) / 4
    }

    /// The chord is written on the last release only, with every key pressed since the first (spec "Entering notes").
    #[test]
    fn the_last_release_gives_the_chord() {
        let (a, b, m) = (Src::Typing(KeyCode::KeyZ), Src::Typing(KeyCode::KeyC), Src::Midi(67));
        let mut c = Chord::default();
        c.down(a, 48, 100);
        c.down(b, 52, 100);
        assert_eq!(c.up(a), None, "a key is still held");
        c.down(m, 67, 90);
        assert_eq!(c.up(b), None);
        assert_eq!(c.up(m), Some(vec![(48, 100), (52, 100), (67, 90)]));
        assert_eq!(c.up(a), None, "a note not held gives nothing");
        c.down(a, 48, 100);
        c.clear();
        assert_eq!(c.up(a), None, "cleared: nothing is written");
    }

    /// One key gives one note at the song position, one step long at Line, for the selected channel, with the typing
    /// keyboard's velocity; nothing while it is held, and it sounds meanwhile. The position moves on one step, 1:01 to
    /// 1:02 to 1:03; the new note is selected and the earlier one loses that (spec: ui/step-edit.md "Entering notes").
    #[test]
    fn one_key_gives_a_step_long_note_and_the_position_moves_one_step() {
        let mut app = app();
        let (ch, st) = (piano::channel(&app.state).unwrap(), one_step(&app));
        assert_eq!((app.state.global_snap.now, step(&app.state)), (Snap::Line, st), "Line counts as one step");
        typed(&mut app, KeyCode::KeyZ, true);
        assert!(notes(&app).is_empty(), "nothing while the key is held");
        assert_eq!(app.state.piano.typed, [(ch, 48)], "the typing keyboard plays it");
        typed(&mut app, KeyCode::KeyZ, false);
        assert_eq!(notes(&app), [(0, st, 48, 100)]);
        assert_eq!(piano::notes(&app.state)[0].channel, ch);
        assert_eq!((app.state.piano.marker, selected(&app)), (st, vec![0]));
        assert_eq!(app.hint, "Song position: 1:02:00");
        tap(&mut app, KeyCode::KeyX);
        assert_eq!(notes(&app), [(0, st, 48, 100), (st, st, 50, 100)]);
        assert_eq!((app.state.piano.marker, selected(&app)), (2 * st, vec![1]), "the earlier note is deselected");
        assert_eq!(app.hint, "Song position: 1:03:00");
        // In SONG mode the song position is the playlist's, and the notes go where the pattern plays there, as the
        // piano roll shows it: a clip of it at bar 3, the position a beat into it.
        app.state.song_mode = true;
        let bar = app.state.project.bar_ticks();
        let clip = os_model::Clip::new(2 * bar, bar, 0, os_model::ClipSource::Pattern(app.state.pattern));
        crate::playlist::arr_mut(&mut app.state).clips = vec![clip];
        app.state.playlist.pos = 2 * bar + 4 * st;
        tap(&mut app, KeyCode::KeyC);
        assert_eq!(notes(&app)[2], (4 * st, st, 52, 100));
        assert_eq!((app.state.playlist.pos, app.state.piano.marker), (2 * bar + 5 * st, 2 * st));
        // Where no clip of the pattern plays, nothing is written and the position stays.
        app.state.playlist.pos = bar / 2;
        tap(&mut app, KeyCode::KeyV);
        assert_eq!((notes(&app).len(), app.state.playlist.pos), (3, bar / 2));
        let m = "Step edit: Pattern 1 does not play at the song position";
        assert_eq!(app.state.message.as_deref(), Some(m));
        // The hint bar shows it over the hint of what the pointer rests on, until the pointer moves.
        assert_eq!(app.hint, m);
        app.mouse_move(app.mouse.0 + 1.0, app.mouse.1);
        assert_ne!(app.hint, m);
    }

    /// Keys held together give one chord, written on the release of the last one with every key pressed since the
    /// first (a MIDI note joins in, with its own velocity): one write, one step on, one undo step "Step edit", and one
    /// undo (Ctrl+Z) removes the whole chord (spec "Entering notes", "Lemon Studio").
    #[test]
    fn a_chord_is_one_write_on_the_last_release_and_one_undo() {
        let mut app = app();
        let st = one_step(&app);
        let at = app.state.history.at();
        typed(&mut app, KeyCode::KeyZ, true);
        typed(&mut app, KeyCode::KeyC, true);
        typed(&mut app, KeyCode::KeyZ, false);
        assert!(notes(&app).is_empty(), "a key is still held");
        app.midi_notes(vec![Msg::On { ch: 0, key: 67, vel: 90 }]);
        typed(&mut app, KeyCode::KeyV, true);
        typed(&mut app, KeyCode::KeyC, false);
        typed(&mut app, KeyCode::KeyV, false);
        assert!(notes(&app).is_empty(), "the MIDI key is still held");
        app.midi_notes(vec![Msg::Off { ch: 0, key: 67, vel: 64 }]);
        assert_eq!(notes(&app), [(0, st, 48, 100), (0, st, 52, 100), (0, st, 67, 90), (0, st, 53, 100)]);
        assert_eq!((app.state.piano.marker, selected(&app)), (st, vec![0, 1, 2, 3]));
        let h = &app.state.history;
        assert_eq!(h.at(), at + 1, "one undo step");
        assert_eq!(h.entries().into_iter().find(|e| e.0 == h.at()).map(|e| e.1), Some("Step edit"));
        app.key(&Key::Character("z".into()), true, false);
        assert!(notes(&app).is_empty(), "one undo removes the chord");
        assert_eq!(app.state.history.at(), at);
    }

    /// At Beat each note is a beat long and the position moves a beat; the start is where the position was, not
    /// snapped: after two steps at Line, the note starts at 1:03 and the position goes to 1:07 (spec "Entering notes").
    #[test]
    fn beat_snap_gives_beat_long_notes_from_where_the_position_was() {
        let mut app = app();
        let (ppq, st) = (u32::from(app.state.project.ppq), one_step(&app));
        tap(&mut app, KeyCode::KeyZ);
        tap(&mut app, KeyCode::KeyX);
        assert_eq!(app.state.piano.marker, 2 * st, "1:03");
        crate::snap::set(&mut app.state, Snap::Beat);
        assert_eq!(step(&app.state), ppq);
        tap(&mut app, KeyCode::KeyC);
        assert_eq!(notes(&app)[2], (2 * st, ppq, 52, 100), "from 1:03, a beat long");
        assert_eq!((app.state.piano.marker, selected(&app)), (2 * st + ppq, vec![2]));
        assert_eq!(app.hint, "Song position: 1:07:00");
    }

    /// Nothing is written while the switch is off or the transport plays (Lemon Studio's choice), nor when playback
    /// starts or the switch goes off while keys are held; the notes still sound (spec "Lemon Studio").
    #[test]
    fn nothing_while_off_or_playing() {
        let mut app = app();
        let nothing = |app: &App| notes(app).is_empty() && app.state.piano.marker == 0;
        ctrl_e(&mut app);
        assert!(!on());
        tap(&mut app, KeyCode::KeyZ);
        assert!(nothing(&app), "off");
        ctrl_e(&mut app);
        app.transport.pretend_playing();
        typed(&mut app, KeyCode::KeyZ, true);
        assert_eq!(app.state.piano.typed.len(), 1, "the note sounds");
        typed(&mut app, KeyCode::KeyZ, false);
        assert!(nothing(&app), "playing");
        app.transport.stop();
        typed(&mut app, KeyCode::KeyZ, true);
        app.transport.pretend_playing();
        typed(&mut app, KeyCode::KeyZ, false);
        assert!(nothing(&app), "playback started while the key was held");
        app.transport.stop();
        typed(&mut app, KeyCode::KeyZ, true);
        ctrl_e(&mut app);
        ctrl_e(&mut app);
        typed(&mut app, KeyCode::KeyZ, false);
        assert!(nothing(&app), "switched off and on while the key was held");
        tap(&mut app, KeyCode::KeyZ);
        assert_eq!(notes(&app).len(), 1, "stopped and on: written");
    }

    /// The toolbar button's pixels with the pointer away from it.
    fn look(app: &mut App, c: &mut Canvas, r: Rect) -> Vec<u8> {
        app.mouse_move(5.0, 300.0);
        app.paint(c, None);
        let (w, d) = (c.pixmap.width() as usize, c.pixmap.data());
        let (x0, x1) = (r.x as usize, (r.x + r.w) as usize);
        (r.y as usize..(r.y + r.h) as usize).flat_map(|y| d[(y * w + x0) * 4..(y * w + x1) * 4].to_vec()).collect()
    }

    /// Ctrl+E, OPTIONS > Step edit and the toolbar's third mode button toggle the one switch, off at start; the menu
    /// ticks it and the button is lit while it is on (spec "The switch").
    #[test]
    fn ctrl_e_the_menu_and_the_button_toggle_it() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.paint(&mut c, None);
        assert!(!on(), "off at start");
        let ticked = |app: &App| {
            let rows = &bar_model(&app.state)[OPTIONS_MENU].rows;
            let row = rows.iter().find(|r| r.label == "Step edit").expect("OPTIONS > Step edit");
            assert!(row.enabled && row.shortcut == "Ctrl+E");
            row.mark == Mark::Check(true)
        };
        assert!(!ticked(&app));
        let r = app.hits.iter().find(|h| h.action == Action::StepEdit).map(|h| h.rect).expect("the button");
        assert_eq!((r.x, r.y), (646.0, 45.0));
        let unlit = look(&mut app, &mut c, r);
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        assert!(on() && ticked(&app), "the button");
        assert_ne!(look(&mut app, &mut c, r), unlit, "lit while on");
        let opt = (0..60).find(|&i| item_at(OPTIONS_MENU, i).is_some_and(|it| it.label == "Step edit")).unwrap();
        app.menu_item(OPTIONS_MENU, opt);
        assert!(!on() && !ticked(&app), "the menu");
        assert_eq!(look(&mut app, &mut c, r), unlit);
        ctrl_e(&mut app);
        assert!(on() && ticked(&app), "Ctrl+E");
        ctrl_e(&mut app);
        assert!(!on());
    }
}
