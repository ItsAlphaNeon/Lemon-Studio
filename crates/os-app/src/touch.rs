// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Touch controller (VIEW > Touch controller, Alt+F7, the toolbar's touch button): a window of 4 x 4 note pads or a
//! two-octave keyboard; pressing one plays the selected channel like a MIDI key. The layout is here; the window is
//! `views/touch.rs`.
// spec: ui/touch-controller.md

use crate::app::{App, Win};

/// The pads' bottom-left note when first opened: G#4, as FL opens (spec §2 Window).
pub const PAD_ROOT: u8 = 56;
/// The keyboard's first C when first opened: C4.
pub const KEYS_LOW: u8 = 48;
/// Pads per row and rows.
pub const PADS: u8 = 4;
/// The keyboard's octaves.
pub const OCTAVES: u8 = 2;
/// Velocity of a press: FL's default note velocity (spec §2 Playing).
pub const VELOCITY: u8 = 100;

/// What the window's controls do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchAct {
    /// Keys (false) or Pads (true).
    Mode(bool),
    /// One octave down (-1) or up (1).
    Octave(i8),
    /// A pad or key: its note, played while held.
    Note(u8),
}

/// The window's state: where it is (centred the first time, then where it was left, also in later runs: `layout`), its
/// mode and range, and the note held by the mouse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchUi {
    pub pos: (f32, f32),
    pub placed: bool,
    pub pads: bool,
    pub pad_root: u8,
    pub keys_low: u8,
    /// The note the mouse holds and the rack row it plays.
    pub held: Option<(u8, usize)>,
}

impl Default for TouchUi {
    fn default() -> Self {
        TouchUi { pos: (0.0, 0.0), placed: false, pads: true, pad_root: PAD_ROOT, keys_low: KEYS_LOW, held: None }
    }
}

impl TouchUi {
    /// The lowest note in view and how many notes the mode shows.
    pub fn range(&self) -> (u8, u8) {
        if self.pads { (self.pad_root, PADS * PADS) } else { (self.keys_low, 12 * OCTAVES) }
    }

    /// Moves the range `d` octaves when every note stays within 0..=127; false at an end (spec §2 Window).
    pub fn shift(&mut self, d: i8) -> bool {
        let (low, n) = self.range();
        let new = i16::from(low) + 12 * i16::from(d);
        if new < 0 || new + i16::from(n) - 1 > 127 {
            return false;
        }
        if self.pads {
            self.pad_root = new as u8;
        } else {
            self.keys_low = new as u8;
        }
        true
    }
}

/// The note of pad (`col`, `row`), row 0 at the bottom: up a semitone left to right, then a row (spec §2 Window).
pub fn pad_note(root: u8, col: u8, row: u8) -> u8 {
    root + row * PADS + col
}

/// Whether `key` is a black key.
pub fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

impl App {
    /// VIEW > Touch controller, Alt+F7 and the toolbar button: open it (or bring it to the front), centred over the
    /// main window the first time (then where it was left, also in later runs: `layout`); close it when it is in front
    /// (spec §2 Opening).
    pub(crate) fn toggle_touch(&mut self) {
        let s = &mut self.state;
        if s.shown(Win::Touch) && s.in_front(Win::Touch) {
            s.hide(Win::Touch);
            self.touch_hold(None);
        } else {
            if !s.touch.placed {
                let (w, h) = crate::views::touch::SIZE;
                s.touch.pos = (((s.screen.0 - w) / 2.0).max(0.0).round(), ((s.screen.1 - h) / 2.0).max(crate::views::work_y(s)).round());
                s.touch.placed = true;
            }
            s.raise(Win::Touch);
        }
        self.invalidate();
    }

    /// A control of the window, pressed (notes) or released over (the others).
    pub(crate) fn touch_act(&mut self, a: TouchAct) {
        match a {
            TouchAct::Mode(pads) => {
                self.touch_hold(None);
                self.state.touch.pads = pads;
            }
            TouchAct::Octave(d) => {
                self.touch_hold(None);
                self.state.touch.shift(d);
            }
            TouchAct::Note(k) => {
                self.drag = Some(crate::app::Drag::Touch);
                self.touch_hold(Some(k));
                self.key_tap(); // a press taps, a glissando does not (spec: ui/tempo-tapper.md §8)
            }
        }
        self.damage_win(Win::Touch);
    }

    /// The mouse holds note `key` (None: no note): the note held before ends, and the new one plays the selected
    /// channel, lights on the piano roll's keyboard and goes to step edit, as a MIDI key does (spec §2 Playing).
    pub(crate) fn touch_hold(&mut self, key: Option<u8>) {
        if self.state.touch.held.map(|h| h.0) == key {
            return;
        }
        if let Some((k, row)) = self.state.touch.held.take() {
            self.transport.note_off(row, u16::from(k));
            self.step_edit_up(crate::step_edit::Src::Touch(k));
        }
        let row = self.state.selected_channel;
        if let Some(k) = key.filter(|_| row < self.state.project.channels.len()) {
            self.sync_live();
            self.transport.note_on(row, u16::from(k), VELOCITY);
            self.state.touch.held = Some((k, row));
            self.step_edit_down(crate::step_edit::Src::Touch(k), u16::from(k), VELOCITY);
        }
        self.typing_lit_changed();
        self.damage_win(Win::Touch);
    }

    /// While a pad or key is held: the note under the mouse, if any, takes over (a glissando).
    pub(crate) fn touch_drag(&mut self, x: f32, y: f32) {
        let under = self.top_hit(x, y).and_then(|h| match h.action {
            crate::app::Action::Touch(TouchAct::Note(k)) => Some(k),
            _ => None,
        });
        self.touch_hold(under);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{test_project, Action};
    use crate::views::menus::{self, VIEW};
    use os_ui::{Canvas, Rect};

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn hint(app: &App, a: &Action) -> String {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.hint.to_string()).unwrap_or_default()
    }

    /// Pads: G#4 at the bottom left, up a semitone to the right and a row up; octave steps stop inside 0..=127.
    #[test]
    fn layout_and_octaves() {
        assert_eq!((pad_note(PAD_ROOT, 0, 0), pad_note(PAD_ROOT, 3, 0), pad_note(PAD_ROOT, 0, 1), pad_note(PAD_ROOT, 3, 3)), (56, 59, 60, 71));
        let mut t = TouchUi::default();
        assert_eq!(t.range(), (56, 16));
        assert!(t.shift(-1) && t.shift(-1) && t.shift(-1) && t.shift(-1));
        assert_eq!(t.pad_root, 8);
        assert!(!t.shift(-1), "the bottom end");
        t.pads = false;
        assert_eq!(t.range(), (48, 24));
        while t.shift(1) {}
        assert_eq!(t.keys_low, 96, "C8 to B9: from C9 it would pass 127");
        assert!((0..12).filter(|&k| is_black(k)).eq([1, 3, 6, 8, 10]));
    }

    /// The toolbar button, VIEW and Alt+F7 open it; pressing a pad plays the selected channel, a drag onto another pad
    /// moves the note, the release ends it (spec §2).
    #[test]
    fn opens_and_plays() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.selected_channel = 1;
        app.paint(&mut c, None);
        let b = hit_rect(&app, &Action::ToggleWin(Win::Touch));
        assert_eq!(hint(&app, &Action::ToggleWin(Win::Touch)), "Touch controller (Alt+F7)");
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down();
        app.mouse_up();
        assert!(app.state.shown(Win::Touch) && app.state.in_front(Win::Touch));
        let i = (0..).map_while(|i| menus::item_at(VIEW, i)).position(|it| it.label == "Touch controller").expect("VIEW entry");
        assert!(menus::works(VIEW, menus::item_at(VIEW, i).unwrap()));
        app.paint(&mut c, None);

        let (g4s, a4) = (Action::Touch(TouchAct::Note(56)), Action::Touch(TouchAct::Note(57)));
        assert_eq!(hint(&app, &g4s), "Touch controller: G#4");
        let (p, q) = (hit_rect(&app, &g4s), hit_rect(&app, &a4));
        app.mouse_move(p.cx(), p.cy());
        app.mouse_down();
        assert_eq!(app.state.touch.held, Some((56, 1)));
        app.mouse_move(q.cx(), q.cy());
        assert_eq!(app.state.touch.held, Some((57, 1)), "a glissando");
        app.mouse_up();
        assert_eq!(app.state.touch.held, None);

        // Keys: two octaves from C4; the black keys are on top.
        let keys = hit_rect(&app, &Action::Touch(TouchAct::Mode(false)));
        app.mouse_move(keys.cx(), keys.cy());
        app.mouse_down();
        app.mouse_up();
        app.paint(&mut c, None);
        assert!(!app.state.touch.pads);
        let (c4, cs4) = (hit_rect(&app, &Action::Touch(TouchAct::Note(48))), hit_rect(&app, &Action::Touch(TouchAct::Note(49))));
        app.mouse_move(cs4.cx(), cs4.cy());
        app.mouse_down();
        assert_eq!(app.state.touch.held.map(|h| h.0), Some(49));
        app.mouse_up();
        app.mouse_move(c4.x + 2.0, c4.bottom() - 4.0);
        app.mouse_down();
        assert_eq!(app.state.touch.held.map(|h| h.0), Some(48));
        app.mouse_up();
        let up = hit_rect(&app, &Action::Touch(TouchAct::Octave(1)));
        app.mouse_move(up.cx(), up.cy());
        app.mouse_down();
        app.mouse_up();
        assert_eq!(app.state.touch.keys_low, 60);

        // Alt+F7 closes it while it is in front.
        app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::F7), false, true);
        assert!(!app.state.shown(Win::Touch));
    }
}
