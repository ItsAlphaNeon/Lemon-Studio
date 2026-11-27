// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Piano roll note groups: ▸ Group > Group (Shift+G) puts the selected notes in a group of their own, Ungroup (Alt+G)
//! takes the selected notes' groups apart. A click on a grouped note highlights its whole group (without selecting
//! it, as FL), and grouped notes move, resize and delete together while Enable groups is on. The group number is the
//! note record's bytes 14-15 (`Note::unk14`), as FL saves it; Flam, Chop and Arpeggiate's "Group notes" write the
//! same numbers.
// spec: ui/piano-note-groups.md; flp/events/patterns.md (note record, offset 14)

use os_model::Note;

use crate::app::State;
use crate::piano;

/// `idx` (note indices into the pattern) and every note of the same channel sharing a group with one of them (spec:
/// ui/piano-note-groups.md §3).
pub fn with_groups(notes: &[Note], mut idx: Vec<usize>) -> Vec<usize> {
    // Enable groups off: each note on its own, as the playlist's clips (the groups are kept). FL not measured here.
    if !crate::enable_groups::on() {
        return idx;
    }
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

/// The group a click on a grouped note highlights (§3): FL outlines it without selecting it (no selection bit in the
/// file). The pattern, channel and group number; cleared by a click elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Highlight {
    pub pattern: u16,
    pub channel: u16,
    pub group: u16,
}

/// A press on note `i` (Draw or Select tool): its group is highlighted, a loose note clears the highlight.
pub fn press(s: &mut State, i: usize) {
    let n = piano::notes(&s.session).get(i).copied();
    s.piano.group_hl = n.filter(|n| n.unk14 != 0 && crate::enable_groups::on()).map(|n| Highlight {
        pattern: s.pattern(),
        channel: n.channel,
        group: n.unk14,
    });
}

/// The notes the group highlight outlines, in pattern order.
pub fn highlighted(s: &State) -> Vec<usize> {
    let Some(h) = s.piano.group_hl.filter(|h| h.pattern == s.pattern() && crate::enable_groups::on()) else {
        return Vec::new();
    };
    piano::notes(&s.session)
        .iter()
        .enumerate()
        .filter(|(_, n)| n.channel == h.channel && n.unk14 == h.group)
        .map(|(i, _)| i)
        .collect()
}

/// Copies `from..` of the pattern's notes form new groups of their own, one new number per group copied, numbered as
/// Shift+G numbers a new group (as the playlist's clip copies; §2).
pub fn regroup_copies(notes: &mut [Note], from: usize) {
    let from = from.min(notes.len());
    let mut used: Vec<u16> = notes[..from].iter().map(|n| n.unk14).collect();
    let mut map: Vec<(u16, u16)> = Vec::new();
    for n in &mut notes[from..] {
        if n.unk14 == 0 {
            continue;
        }
        let g = match map.iter().find(|(old, _)| *old == n.unk14) {
            Some(&(_, new)) => new,
            None => {
                let new = (1..=u16::MAX).find(|g| !used.contains(g)).unwrap_or(u16::MAX);
                used.push(new);
                map.push((n.unk14, new));
                new
            }
        };
        n.unk14 = g;
    }
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
