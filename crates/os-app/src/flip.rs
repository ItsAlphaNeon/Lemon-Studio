// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Flip tool (Tools ▸ Flip…, Alt+Y): its switches and the flip itself, free of any window. The
//! dialog's session is `piano_dlg.rs`, the window `views/piano_dlg.rs`.
// spec: ui/piano-flip.md

use os_model::Note;

/// The dialog's switches (§1): independent, though drawn like radio buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub horizontal: bool,
    /// Only counts while `horizontal` is on.
    pub preserve: bool,
    pub vertical: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { horizontal: true, preserve: true, vertical: false }
    }
}

/// Mirror `notes` (the targets) in time and / or pitch between their own bounds (§2, §3).
pub fn flip(mut notes: Vec<Note>, set: &Settings) -> Vec<Note> {
    let Some(first) = notes.iter().map(|n| n.position).min() else { return notes };
    let last_start = notes.iter().map(|n| n.position).max().unwrap_or(first);
    let last_end = notes.iter().map(|n| n.position + n.length).max().unwrap_or(first);
    let low = notes.iter().map(|n| n.key).min().unwrap_or(0);
    let high = notes.iter().map(|n| n.key).max().unwrap_or(0);
    for n in &mut notes {
        if set.horizontal {
            n.position = if set.preserve { first + (last_start - n.position) } else { first + (last_end - (n.position + n.length)) };
        }
        if set.vertical {
            n.key = low + (high - n.key);
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, len: u32, key: u16, vel: u8) -> Note {
        Note { length: len, key, velocity: vel, ..Note::step(0, pos) }
    }

    /// The FL test project (PPQ 96).
    fn proj() -> Vec<Note> {
        vec![note(0, 96, 60, 100), note(96, 48, 64, 80), note(192, 96, 67, 110), note(384, 192, 72, 60), note(384, 96, 64, 127)]
    }

    /// start:length:key, sorted as the spec's table.
    fn show(ns: &[Note]) -> String {
        let mut v: Vec<_> = ns.iter().map(|n| (n.position, n.length, n.key)).collect();
        v.sort_unstable();
        v.iter().map(|&(p, l, k)| format!("{p}:{l}:{k}")).collect::<Vec<_>>().join(" ")
    }

    fn run(horizontal: bool, preserve: bool, vertical: bool) -> String {
        show(&flip(proj(), &Settings { horizontal, preserve, vertical }))
    }

    #[test]
    fn flips_match_fl() {
        assert_eq!(run(true, true, false), "0:96:64 0:192:72 192:96:67 288:48:64 384:96:60");
        assert_eq!(run(true, false, false), "0:192:72 96:96:64 288:96:67 432:48:64 480:96:60");
        assert_eq!(run(false, true, true), "0:96:72 96:48:68 192:96:65 384:96:68 384:192:60");
        assert_eq!(run(true, true, true), "0:96:68 0:192:60 192:96:65 288:48:68 384:96:72");
        assert_eq!(run(true, false, true), "0:192:60 96:96:68 288:96:65 432:48:68 480:96:72");
        assert_eq!(run(false, false, false), show(&proj()), "every switch off: nothing moves");
    }

    #[test]
    fn a_selection_flips_between_its_own_bounds() {
        let sel = vec![note(96, 48, 64, 80), note(192, 96, 67, 110)];
        assert_eq!(show(&flip(sel.clone(), &Settings::default())), "96:96:67 192:48:64");
        assert_eq!(show(&flip(sel.clone(), &Settings { preserve: false, ..Settings::default() })), "96:96:67 240:48:64");
        assert_eq!(show(&flip(sel, &Settings { horizontal: false, preserve: true, vertical: true })), "96:48:67 192:96:64");
    }

    #[test]
    fn properties_stay_with_their_note() {
        let out = flip(proj(), &Settings { horizontal: true, preserve: true, vertical: true });
        let c6 = out.iter().find(|n| n.velocity == 60).unwrap();
        assert_eq!((c6.position, c6.length, c6.key), (0, 192, 60));
        assert!(flip(Vec::new(), &Settings::default()).is_empty());
    }
}
