// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Piano roll note groups: ▸ Group > Group (Shift+G) puts the selected notes in a group of their own, Ungroup (Alt+G)
//! takes the selected notes' groups apart. A click on a grouped note selects its whole group, and grouped notes move
//! and resize together. The group number is the note record's bytes 14-15 (`Note::unk14`), as FL saves it; Flam, Chop
//! and Arpeggiate's "Group notes" write the same numbers.
// spec: ui/piano-note-groups.md; flp/events/patterns.md (note record, offset 14)

use os_model::Note;

use crate::app::State;
use crate::piano;

/// `idx` (note indices into the pattern) and every note of the same channel sharing a group with one of them (spec:
/// ui/piano-note-groups.md §3).
pub fn with_groups(notes: &[Note], mut idx: Vec<usize>) -> Vec<usize> {
    let groups: Vec<(u16, u16)> =
        idx.iter().filter_map(|&i| notes.get(i)).filter(|n| n.unk14 != 0).map(|n| (n.channel, n.unk14)).collect();
    if groups.is_empty() {
        return idx;
    }
    for (i, n) in notes.iter().enumerate() {
        if groups.contains(&(n.channel, n.unk14)) && !idx.contains(&i) {
            idx.push(i);
        }
    }
    idx
}

/// The lowest group number no note of the pattern uses (§2).
pub fn free_group(notes: &[Note]) -> u16 {
    (1..=u16::MAX).find(|g| !notes.iter().any(|n| n.unk14 == *g)).unwrap_or(u16::MAX)
}

/// Shift+G (§2): the selected notes (with the groups they are in) become one new group. Returns how many notes.
pub fn group(s: &mut State) -> usize {
    let Some(ch) = piano::channel(s) else { return 0 };
    let sel = s.piano.selection(s.pattern(), Some(ch));
    let idx = with_groups(piano::notes(&s.session), sel);
    if idx.len() < 2 {
        return 0;
    }
    piano::snapshot_as(&mut s.session, STEP_GROUP);
    let g = free_group(piano::notes(&s.session));
    let p = piano::pattern_mut(&mut s.session);
    for &i in &idx {
        if let Some(n) = p.notes.get_mut(i) {
            n.unk14 = g;
        }
    }
    s.piano.select(s.pattern(), ch, idx.clone());
    s.session.dirty = true;
    idx.len()
}

/// Alt+G (§2): the selected notes' groups are taken apart. Returns how many notes left a group.
pub fn ungroup(s: &mut State) -> usize {
    let Some(ch) = piano::channel(s) else { return 0 };
    let sel = s.piano.selection(s.pattern(), Some(ch));
    let idx = with_groups(piano::notes(&s.session), sel);
    let grouped: Vec<usize> =
        idx.into_iter().filter(|&i| piano::notes(&s.session).get(i).is_some_and(|n| n.unk14 != 0)).collect();
    if grouped.is_empty() {
        return 0;
    }
    piano::snapshot_as(&mut s.session, STEP_UNGROUP);
    let p = piano::pattern_mut(&mut s.session);
    for &i in &grouped {
        if let Some(n) = p.notes.get_mut(i) {
            n.unk14 = 0;
        }
    }
    s.session.dirty = true;
    grouped.len()
}

/// The undo steps' names.
pub const STEP_GROUP: &str = "Group notes";
pub const STEP_UNGROUP: &str = "Ungroup notes";

#[cfg(test)]
mod tests {
    use super::*;

    fn n(ch: u16, g: u16) -> Note {
        Note { unk14: g, key: 60, length: 24, ..Note::step(ch, 0) }
    }

    #[test]
    fn a_grouped_note_brings_its_group_of_its_channel() {
        let notes = [n(0, 0), n(0, 1), n(0, 1), n(1, 1), n(0, 2)];
        let mut got = with_groups(&notes, vec![1]);
        got.sort_unstable();
        assert_eq!(got, vec![1, 2], "group 1 of channel 0 only");
        assert_eq!(with_groups(&notes, vec![0]), vec![0], "a loose note stays alone");
    }

    #[test]
    fn the_new_group_takes_the_lowest_free_number() {
        assert_eq!(free_group(&[n(0, 0)]), 1);
        assert_eq!(free_group(&[n(0, 1), n(0, 3)]), 2);
    }
}
