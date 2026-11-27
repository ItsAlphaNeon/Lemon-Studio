// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Chord Stamp: a chord or scale picked from the toolbar's stamp menu; a click in the grid draws all
//! its notes from the clicked key, as a selected group. Also the chord name of the selected notes.
// spec: ui/piano-chord-stamp.md

use os_model::Note;

use crate::chords;

/// What the stamp menu picked (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pick {
    /// "(none)" (Shift+N): single notes.
    #[default]
    None,
    /// One of "Melodic - chords" (a place in `NAMED`).
    Named(usize),
    /// Advanced ▸: an entry of the chords file (1..FIRST_SCALE).
    Advanced(usize),
    /// One of "Melodic - scales" (a place in `os_model::SCALES`).
    Scale(usize),
}

/// "Melodic - chords" (§1), with the keys each stamps above its root. FL measured: Major = root, +4, +7 (§3); the
/// others are the chords their names say, root position.
pub const NAMED: [(&str, &[i32]); 13] = [
    ("Augmented", &[0, 4, 8]),
    ("Diminished", &[0, 3, 6]),
    ("Fifth", &[0, 7]),
    ("Fifth 9th", &[0, 7, 14]),
    ("Fifth Octave", &[0, 7, 12]),
    ("Major", &[0, 4, 7]),
    ("Major 7th", &[0, 4, 7, 11]),
    ("Minor", &[0, 3, 7]),
    ("Minor 7th", &[0, 3, 7, 10]),
    ("Minor 9th", &[0, 3, 7, 10, 14]),
    ("Octave", &[0, 12]),
    ("Suspended 2", &[0, 2, 7]),
    ("Suspended 4", &[0, 5, 7]),
];

/// The stamp: what is picked, and "Only one" (back to (none) after one stamp).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stamp {
    pub pick: Pick,
    pub only_one: bool,
}

impl Stamp {
    pub fn on(&self) -> bool {
        self.pick != Pick::None
    }
}

/// The undo step of a stamp.
pub const STEP: &str = "Piano roll add note";

/// An Advanced ▸ entry's label: the chords file's short name, as FL's menu shows it (§2: "MajB5").
pub fn advanced_label(k: usize) -> &'static str {
    match chords::label(k) {
        "Majb5" => "MajB5",
        l => l,
    }
}

/// The Advanced ▸ entries in menu order (§2): by label, ignoring case and hyphens, digit runs taken as numbers.
pub fn advanced_order() -> Vec<usize> {
    let mut ks: Vec<usize> = (1..chords::FIRST_SCALE).collect();
    ks.sort_by(|&a, &b| natural_key(advanced_label(a)).cmp(&natural_key(advanced_label(b))));
    ks
}

/// A sort key for natural order: runs of digits as numbers, other characters lower-cased, hyphens dropped.
fn natural_key(s: &str) -> Vec<(u32, char)> {
    let mut out = Vec::new();
    let mut num: Option<u32> = None;
    for c in s.chars() {
        if let Some(d) = c.to_digit(10) {
            num = Some(num.unwrap_or(0) * 10 + d);
            continue;
        }
        if let Some(n) = num.take() {
            out.push((0, char::from_u32(n).unwrap_or('\0')));
        }
        if c != '-' {
            out.push((1, c.to_ascii_lowercase()));
        }
    }
    if let Some(n) = num {
        out.push((0, char::from_u32(n).unwrap_or('\0')));
    }
    out
}

/// The keys a pick stamps from `root` (§3): a chord's keys; a scale's keys over one octave up from the root.
pub fn keys(p: Pick, root: i32) -> Vec<i32> {
    match p {
        Pick::None => vec![root],
        Pick::Named(i) => NAMED.get(i).map_or(vec![root], |(_, iv)| iv.iter().map(|d| root + d).collect()),
        Pick::Advanced(k) => {
            let mut out = vec![root];
            let mut key = root;
            for &iv in chords::entry(k).map_or(&[][..], |e| e.intervals) {
                key += i32::from(iv);
                out.push(key);
            }
            out
        }
        Pick::Scale(i) => {
            let set = os_model::scale_keys(i, root.rem_euclid(12) as u8);
            (0..12).map(|d| root + d).filter(|k| set & (1 << k.rem_euclid(12)) != 0).collect()
        }
    }
}

/// A click at `tick` on row `key` with a stamp picked (§3): its notes from the row's key, each on the scale while Snap
/// to scale is on, all as long as a new note, made one new group. Returns their indices, the clicked key's first.
pub fn stamp(s: &mut crate::app::State, ch: u16, tick: u32, key: i32, st: Stamp) -> Vec<usize> {
    let mut done: Vec<i32> = Vec::new();
    for k in keys(st.pick, key) {
        let k = crate::scale_snap::drawn(s, tick, k, 0.5);
        if (0..=crate::piano::TOP_KEY).contains(&k) && !done.contains(&k) {
            done.push(k);
        }
    }
    let group = if done.len() > 1 { crate::note_groups::free_group(crate::piano::notes(&s.session)) } else { 0 };
    let mut out = Vec::new();
    for k in done {
        let i = crate::piano::add_note(s, ch, tick, k);
        if let Some(n) = crate::piano::pattern_mut(&mut s.session).notes.get_mut(i) {
            n.unk14 = group;
        }
        out.push(i);
    }
    out
}

/// The chord the keys make (§4): the chords file's first chord (1..FIRST_SCALE) whose pitch classes are exactly the
/// keys', with the root it has. The lowest key's pitch class is tried as the root first.
pub fn detect(keys: &[i32]) -> Option<(u8, usize)> {
    let mut pcs = [false; 12];
    for k in keys {
        pcs[k.rem_euclid(12) as usize] = true;
    }
    if pcs.iter().filter(|&&b| b).count() < 2 {
        return None;
    }
    let low = keys.iter().min()?.rem_euclid(12) as u8;
    let roots = std::iter::once(low).chain((0..12u8).filter(move |&r| r != low));
    for root in roots {
        if !pcs[usize::from(root)] {
            continue;
        }
        if let Some(k) = (1..chords::FIRST_SCALE).find(|&k| chords::classes(k, root) == pcs) {
            return Some((root, k));
        }
    }
    None
}

/// The chord name of notes (§4): the root's name, then the chord's short name.
pub fn name_of(notes: &[Note]) -> Option<String> {
    let keys: Vec<i32> = notes.iter().map(|n| i32::from(n.key)).collect();
    let (root, k) = detect(&keys)?;
    Some(format!("{} {}", os_model::NOTE_NAMES[usize::from(root)], advanced_label(k)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_stamp_their_keys() {
        assert_eq!(keys(Pick::Named(5), 48), [48, 52, 55], "Major on C4, as FL stamped it");
        assert_eq!(keys(Pick::Advanced(1), 60), [60, 64, 67], "the chords file's Major");
        assert_eq!(keys(Pick::Scale(0), 60), [60, 62, 64, 65, 67, 69, 71], "Major (Ionian) over one octave");
        assert_eq!(keys(Pick::Scale(0), 62), [62, 64, 66, 67, 69, 71, 73], "D major from D");
    }

    #[test]
    fn the_advanced_list_is_in_fl_order() {
        let labels: Vec<&str> = advanced_order().into_iter().map(advanced_label).collect();
        assert_eq!(labels.len(), 66);
        assert_eq!(&labels[..8], ["6", "6add9", "6sus4", "7", "7#5", "7#5#9", "7#5b9", "7#9"]);
        assert_eq!(labels[52], "Maj11", "the first column ends with Maj11");
        assert_eq!(
            &labels[53..],
            [
                "Maj13",
                "MajB5",
                "Major",
                "mb5",
                "minor",
                "m-Maj7",
                "m-Maj7add11",
                "m-Maj7add13",
                "m-Maj11",
                "m-Maj13",
                "sus2",
                "sus4",
                "tri"
            ]
        );
    }

    #[test]
    fn chords_are_named_from_their_pitch_classes() {
        assert_eq!(detect(&[48, 52, 55]), Some((0, 1)), "C Major");
        assert_eq!(detect(&[52, 55, 60]), Some((0, 1)), "an inversion still names C");
        assert_eq!(detect(&[57, 60, 64]), Some((9, 5)), "A minor");
        assert_eq!(detect(&[60]), None);
    }
}
