// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::note_names` (step 9: the session's side stays there).

#![allow(unused_imports)]
use crate::note_names::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs;

    /// The three schemes with sharps and with flats (spec: ui/settings-choices.md "Note names and Black notes").
    #[test]
    fn schemes_and_black_keys() {
        let keys = [60, 61, 58, 66, 70, 56, 71, 0, 131];
        let table = |scheme, flats| keys.map(|k| name_in(k, scheme, flats));
        assert_eq!(table(0, false), ["C5", "C#5", "A#4", "F#5", "A#5", "G#4", "B5", "C0", "B10"]);
        assert_eq!(table(0, true), ["C5", "D♭5", "B♭4", "G♭5", "B♭5", "A♭4", "B5", "C0", "B10"]);
        assert_eq!(table(1, false), ["C5", "C#5", "A#4", "F#5", "A#5", "G#4", "H5", "C0", "H10"]);
        assert_eq!(table(1, true), ["C5", "D♭5", "H♭4", "G♭5", "H♭5", "A♭4", "H5", "C0", "H10"]);
        assert_eq!(table(2, false), ["Do5", "Do#5", "La#4", "Fa#5", "La#5", "Sol#4", "Si5", "Do0", "Si10"]);
        assert_eq!(table(2, true), ["Do5", "R♭5", "S♭4", "S♭5", "S♭5", "L♭4", "Si5", "Do0", "Si10"]);
        assert_eq!(name_in(63, 2, true), "M♭5");
    }

    /// The settings choose the names; the defaults are English with sharps.
    #[test]
    fn follows_the_settings() {
        assert_eq!((note_name(60), note_name(61), key_hint(60)), ("C5".to_string(), "C#5".to_string(), "C5 / 60".to_string()));
        prefs::set_choice(&prefs::NOTE_NAMES, 2);
        prefs::set_choice(&prefs::BLACK_NOTES, 1);
        assert_eq!((note_name(70), key_hint(61)), ("S♭5".to_string(), "R♭5 / 61".to_string()));
        prefs::set_choice(&prefs::NOTE_NAMES, 0);
        prefs::set_choice(&prefs::BLACK_NOTES, 0);
    }

    /// The hint over a piano roll key names it and its MIDI number, in the chosen names at once.
    #[test]
    fn piano_keys_hint_name_and_number() {
        use crate::app::{Action, App, Win};
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.paint(&mut c, None);
        let keys = app.hits.iter().rev().find(|h| h.action == Action::PianoKeys).map(|h| h.rect).expect("the keyboard");
        let g = crate::piano::Geom::new(&app.state, app.piano_grid.expect("the grid"));
        // A black key in view: its row's middle.
        let key = (0..=g.key_at(keys.y + 4.0)).rev().find(|&k| matches!(k.rem_euclid(12), 1 | 3 | 6 | 8 | 10) && g.y_of(k) >= keys.y).expect("a black key");
        let y = g.y_of(key) + g.row / 2.0;
        app.mouse_move(keys.cx(), y);
        assert_eq!(app.hint, name_in(key, 0, false) + " / " + &key.to_string());
        prefs::set_choice(&prefs::NOTE_NAMES, 1);
        prefs::set_choice(&prefs::BLACK_NOTES, 1);
        app.mouse_move(keys.cx(), y + 0.5);
        assert_eq!(app.hint, format!("{} / {key}", name_in(key, 1, true)));
        assert!(app.hint.contains('\u{266d}'));
        prefs::set_choice(&prefs::NOTE_NAMES, 0);
        prefs::set_choice(&prefs::BLACK_NOTES, 0);
    }
}
