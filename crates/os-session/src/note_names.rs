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
pub fn note_name(st: &crate::settings_file::SettingsStore, key: i32) -> String {
    name_in(key, st.index(&prefs::NOTE_NAMES), st.index(&prefs::BLACK_NOTES) == 1)
}

/// The hint over a key: `<name><octave> / <MIDI number>` (`C5 / 60`), as FL's piano roll keyboard reads.
pub fn key_hint(st: &crate::settings_file::SettingsStore, key: i32) -> String {
    format!("{} / {key}", note_name(st, key))
}

