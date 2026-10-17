// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Note names, as General ▸ Language ▸ Note names and Black notes choose them: the pitch class's letter or syllable,
//! then the octave, MIDI note 60 in octave 5 (`C5`). Germanic only renames B to H; Solfege reads Do Re Mi Fa Sol La Si.
//! A black key is the white key below with `#` (Sharps), or the first letter of the white key above with `♭` (Flats).
//! A change of either setting renames at once (FL asks for a restart).
// spec: ui/settings-choices.md "Note names and Black notes"

use crate::prefs; // This code is a part of L e m o n S t u d i o.

/// The white keys C D E F G A B in each Note names scheme (English, Germanic, Solfege).
const WHITE: [[&str; 7]; 3] = [["C", "D", "E", "F", "G", "A", "B"], ["C", "D", "E", "F", "G", "A", "H"], ["Do", "Re", "Mi", "Fa", "Sol", "La", "Si"]];

/// Pitch class -> its white key, or the white key below a black one.
const BELOW: [usize; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];

/// The name of `key`'s pitch class (no octave) in scheme `scheme` (index of a Note names choice), with flats or sharps.
pub fn pitch_name(key: i32, scheme: usize, flats: bool) -> String {
    let pc = key.rem_euclid(12) as usize;
    let white = &WHITE[scheme.min(WHITE.len() - 1)];
    match (matches!(pc, 1 | 3 | 6 | 8 | 10), flats) {
        (false, _) => white[BELOW[pc]].to_string(),
        (true, false) => format!("{}#", white[BELOW[pc]]),
        (true, true) => format!("{}\u{266d}", &white[BELOW[pc] + 1][..1]),
    }
}

/// `key`'s name with its octave (MIDI 60 = octave 5) in scheme `scheme`, with flats or sharps.
pub fn name_in(key: i32, scheme: usize, flats: bool) -> String {
    format!("{}{}", pitch_name(key, scheme, flats), key.div_euclid(12))
}

/// `key`'s name as the settings choose it: `C5` for 60, `C#5` for 61 by default.
pub fn note_name(key: i32) -> String {
    name_in(key, prefs::index(&prefs::NOTE_NAMES), prefs::index(&prefs::BLACK_NOTES) == 1)
}

/// The hint over a key: `<name><octave> / <MIDI number>` (`C5 / 60`), as FL's piano roll keyboard reads.
pub fn key_hint(key: i32) -> String {
    format!("{} / {key}", note_name(key))
}

#[cfg(test)]
mod tests {
    use super::*;

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
