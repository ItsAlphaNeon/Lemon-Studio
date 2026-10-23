// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Limit tool (Tools ▸ Limit…, Alt+K, FL's "limiter / transposer"): its settings and the limiting
//! itself, free of any window. The dialog's session is `piano_dlg.rs`, the window `views/piano_dlg.rs`.
// spec: ui/piano-limit.md

use os_model::Note;

use crate::chords;
use crate::views::piano::TOP_KEY;

/// The Key box's names (§1).
pub const KEYS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// How a tie between two equally near allowed keys is broken (§3, step 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Snap {
    #[default]
    Above,
    Under,
    Alternate,
}

impl Snap {
    pub const ALL: [Snap; 3] = [Snap::Above, Snap::Under, Snap::Alternate];

    pub fn name(self) -> &'static str {
        match self {
            Snap::Above => "Above",
            Snap::Under => "Under",
            Snap::Alternate => "Alternate",
        }
    }
}

/// The dialog's controls (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settings {
    /// The Key box, 0 = C.
    pub key: u8,
    /// The scale box: 0 = "(none)", `k` = entry `k` of the chords file.
    pub scale: usize,
    pub snap: Snap,
    pub wrap: bool,
    /// The keyboard range, its lowest and highest key (both included); `None` until one is set, the targets' own then.
    pub range: Option<(u16, u16)>,
    /// Semitones added last: 60 minus the green key (§1 "Mouse on the keyboard").
    pub transpose: i32,
}

impl Settings {
    /// The green key: the one that becomes C5.
    pub fn green(&self) -> i32 {
        60 - self.transpose
    }

    /// The range the keyboard shows and its strip moves: C5…C6 while there is none (no target notes).
    pub fn shown_range(&self) -> (u16, u16) {
        self.range.unwrap_or((60, 72))
    }

    /// The scale box's text.
    pub fn scale_name(&self) -> &'static str {
        if self.scale == 0 { "(none)" } else { chords::label(self.scale) }
    }
}

/// The lowest and highest key of `notes` (Reset's range, §1 "What is kept").
pub fn span(notes: &[Note]) -> Option<(u16, u16)> {
    let lo = notes.iter().map(|n| n.key).min()?;
    Some((lo, notes.iter().map(|n| n.key).max()?))
}

/// Step 1 for key `k` (§3): itself when its pitch class is allowed, else the nearest allowed key, the upper one on a
/// tie when `up`. The flag says whether it was a tie.
fn restrict(k: i32, on: &[bool; 12], up: bool) -> (i32, bool) {
    let allowed = |k: i32| on[k.rem_euclid(12) as usize];
    if allowed(k) {
        return (k, false);
    }
    for d in 1..12 {
        match (allowed(k - d), allowed(k + d)) {
            (true, true) => return (if up { k + d } else { k - d }, true),
            (true, false) => return (k - d, false),
            (false, true) => return (k + d, false),
            (false, false) => {}
        }
    }
    (k, false)
}

/// Step 2 (§3): key `k` moved by whole octaves into `lo..=hi`, to the nearest octave in reach, or with `wrap` to the
/// far side; clamped when the range holds no key of its pitch class.
pub fn fold(k: i32, lo: i32, hi: i32, wrap: bool) -> i32 {
    if k > hi {
        if wrap {
            let f = k - 12 * ((k - lo).div_euclid(12));
            if f > hi { hi } else { f }
        } else {
            (k - 12 * (k - hi + 11).div_euclid(12)).max(lo)
        }
    } else if k < lo {
        if wrap {
            let f = k + 12 * ((hi - k).div_euclid(12));
            if f < lo { lo } else { f }
        } else {
            (k + 12 * (lo - k + 11).div_euclid(12)).min(hi)
        }
    } else {
        k
    }
}

/// Limit `notes` (the targets, in the pattern's order) to `range` (§3): the key / scale, the range, the transposition.
pub fn limit(notes: Vec<Note>, set: &Settings, range: (u16, u16)) -> Vec<Note> {
    let on = (set.scale > 0).then(|| chords::classes(set.scale, set.key));
    let (lo, hi) = (i32::from(range.0), i32::from(range.1));
    let mut ties = 0;
    notes
        .into_iter()
        .map(|n| {
            let mut k = i32::from(n.key);
            if let Some(on) = &on {
                let up = match set.snap {
                    Snap::Above => true,
                    Snap::Under => false,
                    Snap::Alternate => ties % 2 == 0,
                };
                let (to, tie) = restrict(k, on, up);
                ties += usize::from(tie);
                k = to;
            }
            let k = (fold(k, lo, hi, set.wrap) + set.transpose).clamp(0, TOP_KEY);
            Note { key: k as u16, ..n }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chromatic test project: G4…G6 (55…79), one note every 24 ticks.
    fn chromatic() -> Vec<Note> {
        (55..80).enumerate().map(|(i, k)| Note { length: 24, key: k, velocity: 100, ..Note::step(0, i as u32 * 24) }).collect()
    }

    fn keys(ns: &[Note]) -> String {
        ns.iter().map(|n| n.key.to_string()).collect::<Vec<_>>().join(" ")
    }

    fn run(f: impl FnOnce(&mut Settings)) -> String {
        let mut s = Settings::default();
        f(&mut s);
        let range = s.range.unwrap_or((55, 79));
        keys(&limit(chromatic(), &s, range))
    }

    const MAJOR: usize = chords::FIRST_SCALE;
    const PENTA: usize = chords::FIRST_SCALE + 1;

    /// §4, key / scale rows.
    #[test]
    fn scales_match_fl() {
        assert_eq!(run(|s| s.scale = MAJOR), "55 57 57 59 59 60 62 62 64 64 65 67 67 69 69 71 71 72 74 74 76 76 77 79 79");
        assert_eq!(run(|s| (s.scale, s.snap) = (MAJOR, Snap::Under)), "55 55 57 57 59 60 60 62 62 64 65 65 67 67 69 69 71 72 72 74 74 76 77 77 79");
        assert_eq!(run(|s| (s.scale, s.snap) = (MAJOR, Snap::Alternate)), "55 57 57 57 59 60 62 62 62 64 65 67 67 67 69 71 71 72 72 74 76 76 77 77 79");
        assert_eq!(run(|s| (s.scale, s.key) = (MAJOR, 2)), "55 57 57 59 59 61 61 62 64 64 66 66 67 69 69 71 71 73 73 74 76 76 78 78 79");
        assert_eq!(run(|s| s.key = 2), keys(&chromatic()), "a Key without a scale changes nothing");
        assert_eq!(run(|s| s.scale = 1), "55 55 55 60 60 60 60 64 64 64 64 67 67 67 67 72 72 72 72 76 76 76 76 79 79");
        assert_eq!(run(|s| (s.scale, s.snap) = (1, Snap::Under)), "55 55 55 60 60 60 60 60 64 64 64 67 67 67 67 72 72 72 72 72 76 76 76 79 79");
        assert_eq!(run(|s| (s.scale, s.snap) = (1, Snap::Alternate)), "55 55 55 60 60 60 60 64 64 64 64 67 67 67 67 72 72 72 72 72 76 76 76 79 79");
        assert_eq!(run(|s| (s.scale, s.snap) = (PENTA, Snap::Alternate)), "55 57 57 57 60 60 60 62 64 64 64 67 67 67 69 69 72 72 74 74 74 76 76 79 79");
    }

    /// §4, range and transposition rows.
    #[test]
    fn ranges_match_fl() {
        let r = |lo: u16, hi: u16, wrap: bool| run(|s| (s.range, s.wrap) = (Some((lo, hi)), wrap));
        assert_eq!(run(|s| s.transpose = 3), (58..83).map(|k| k.to_string()).collect::<Vec<_>>().join(" "));
        assert_eq!(run(|s| s.transpose = -5), (50..75).map(|k| k.to_string()).collect::<Vec<_>>().join(" "));
        assert_eq!(run(|s| (s.scale, s.transpose) = (MAJOR, 1)), "56 58 58 60 60 61 63 63 65 65 66 68 68 70 70 72 72 73 75 75 77 77 78 80 80");
        assert_eq!(r(60, 72, true), "67 68 69 70 71 60 61 62 63 64 65 66 67 68 69 70 71 72 61 62 63 64 65 66 67");
        assert_eq!(r(65, 77, false), "67 68 69 70 71 72 73 74 75 76 65 66 67 68 69 70 71 72 73 74 75 76 77 66 67");
        assert!(r(55, 76, false).ends_with("75 76 65 66 67"));
        assert!(r(55, 76, true).ends_with("75 76 65 66 55"));
        assert_eq!(r(65, 79, false), "67 68 69 70 71 72 73 74 75 76 65 66 67 68 69 70 71 72 73 74 75 76 77 78 79");
        assert_eq!(r(65, 79, true), "79 68 69 70 71 72 73 74 75 76 65 66 67 68 69 70 71 72 73 74 75 76 77 78 79");
        assert_eq!(r(62, 69, false), "67 68 69 69 69 69 69 62 63 64 65 66 67 68 69 62 62 62 62 62 63 64 65 66 67");
        assert_eq!(r(62, 69, true), "67 68 69 62 62 62 62 62 63 64 65 66 67 68 69 69 69 69 69 62 63 64 65 66 67");
        assert_eq!(r(61, 70, false), "67 68 69 70 70 70 61 62 63 64 65 66 67 68 69 70 61 61 61 62 63 64 65 66 67");
        assert_eq!(r(60, 64, false), "64 64 64 64 64 60 61 62 63 64 60 60 60 60 60 60 60 60 61 62 63 64 60 60 60");
        assert_eq!(r(60, 64, true), "60 60 60 60 60 60 61 62 63 64 64 64 64 64 64 64 64 60 61 62 63 64 64 64 64");
        assert_eq!(r(76, 79, false), "79 79 79 79 79 79 79 79 79 76 77 78 79 79 79 79 79 79 79 79 79 76 77 78 79");
        assert_eq!(r(76, 79, true), "79 76 76 76 76 76 76 76 76 76 77 78 79 76 76 76 76 76 76 76 76 76 77 78 79");
        assert_eq!(run(|s| (s.range, s.transpose) = (Some((59, 71)), 5)), "72 73 74 75 64 65 66 67 68 69 70 71 72 73 74 75 76 65 66 67 68 69 70 71 72");
        assert_eq!(run(|s| (s.range, s.transpose) = (Some((60, 67)), -2)), "65 65 65 65 65 58 59 60 61 62 63 64 65 58 58 58 58 58 59 60 61 62 63 64 65");
    }

    /// §4: the scale applies before the range; Alternate counts ties in the notes' stored order.
    #[test]
    fn order_of_steps_and_ties() {
        let p = |wrap: bool| run(|s| (s.scale, s.range, s.wrap) = (PENTA, Some((60, 65)), wrap));
        assert_eq!(p(false), "65 65 65 65 60 60 62 62 64 64 64 60 60 60 60 60 60 60 62 62 64 64 64 60 60");
        assert_eq!(p(true), "60 60 60 60 60 60 62 62 64 64 64 65 65 65 65 65 60 60 62 62 64 64 64 65 65");
        let stored = [(96, 63), (0, 61), (48, 68), (24, 66)].map(|(pos, key)| Note { length: 24, key, velocity: 100, ..Note::step(0, pos) }).to_vec();
        let set = Settings { scale: MAJOR, snap: Snap::Alternate, ..Settings::default() };
        let out = limit(stored.clone(), &set, span(&stored).unwrap());
        assert_eq!(out.iter().map(|n| (n.position, n.key)).collect::<Vec<_>>(), [(96, 64), (0, 68), (48, 61), (24, 65)]);
        assert_eq!(span(&stored), Some((61, 68)));
        assert_eq!(limit(vec![Note { key: 130, ..stored[0] }], &Settings { transpose: 5, ..Settings::default() }, (0, 131))[0].key, 131, "kept within 0…131");
    }
}
