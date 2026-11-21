// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! FL's chords file (`System/Config/Chords.map`) as FL 24.1 and 2026 ship it: 66 chords, then 22 scales, numbered from
//! 1 in that order. The piano roll's Limit dialog lists them as its scales; the arpeggiator's stored chord `k` names
//! entry `k`.
// spec: audio/channel-misc.md "The chords file"; ui/piano-limit.md §1

/// One entry: its name, its short name (empty for a scale, which shows its name) and its intervals in semitones, each
/// from one note of the chord to the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub name: &'static str,
    pub short: &'static str,
    pub intervals: &'static [u8],
}

const fn e(name: &'static str, short: &'static str, intervals: &'static [u8]) -> Entry {
    Entry { name, short, intervals }
}

/// Entries 1..=88, by index `k - 1`.
pub const ENTRIES: [Entry; 88] = [
    e("Major", "Major", &[4, 3]),
    e("Suspended 2nd", "sus2", &[2, 5]),
    e("Suspended 4th", "sus4", &[5, 2]),
    e("Major Flat 5th", "Majb5", &[4, 2]),
    e("minor", "minor", &[3, 4]),
    e("Diminished", "mb5", &[3, 3]),
    e("Augmented", "aug", &[4, 4]),
    e("Augmented Suspended 4th", "augsus4", &[5, 3]),
    e("Triton", "tri", &[3, 3, 3]),
    e("Sixth", "6", &[4, 3, 2]),
    e("Sixth Suspended 4th", "6sus4", &[5, 2, 2]),
    e("Sixth add 9th", "6add9", &[4, 3, 2, 5]),
    e("Minor Sixth", "m6", &[3, 4, 2]),
    e("Minor Sixth 9th", "m6add9", &[3, 4, 2, 5]),
    e("Seventh", "7", &[4, 3, 3]),
    e("Seventh Suspended 4th", "7sus4", &[5, 2, 3]),
    e("Seventh Sharp 5th", "7#5", &[4, 4, 2]),
    e("Seventh Flat 5th", "7b5", &[4, 2, 4]),
    e("Seventh Sharp 9th", "7#9", &[4, 3, 3, 5]),
    e("Seventh Flat 9th", "7b9", &[4, 3, 3, 3]),
    e("Seventh Sharp 5th Sharp 9th", "7#5#9", &[4, 4, 2, 5]),
    e("Seventh Sharp 5th Flat 9th", "7#5b9", &[4, 4, 2, 3]),
    e("Seventh Flat 5th Flat 9th", "7b5b9", &[4, 2, 4, 3]),
    e("Seventh add 11th", "7add11", &[4, 3, 3, 7]),
    e("Seventh add 13th", "7add13", &[4, 3, 3, 11]),
    e("Seventh Sharp 11th", "7#11", &[4, 3, 3, 8]),
    e("Major Seventh", "Maj7", &[4, 3, 4]),
    e("Major Seventh Flat 5th", "Maj7b5", &[4, 2, 5]),
    e("Major Seventh Sharp 5th", "Maj7#5", &[4, 4, 3]),
    e("Major Seventh Sharp 11th", "Maj7#11", &[4, 3, 4, 7]),
    e("Major Seventh add 13th", "Maj7add13", &[4, 3, 4, 10]),
    e("Minor Seventh", "m7", &[3, 4, 3]),
    e("Minor Seventh Flat 5th", "m7b5", &[3, 3, 4]),
    e("Minor Seventh Flat 9th", "m7b9", &[3, 4, 3, 3]),
    e("Minor Seventh add 11th", "m7add11", &[3, 4, 3, 7]),
    e("Minor Seventh add 13th", "m7add13", &[3, 4, 3, 11]),
    e("Minor Major Seventh", "m-Maj7", &[3, 4, 4]),
    e("Minor Major Seventh add 11th", "m-Maj7add11", &[3, 4, 4, 6]),
    e("Minor Major Seventh add 13th", "m-Maj7add13", &[3, 4, 4, 10]),
    e("Ninth", "9", &[4, 3, 3, 4]),
    e("Ninth Suspended 4th", "9sus4", &[5, 2, 3, 4]),
    e("Ninth Add", "add9", &[4, 3, 7]),
    e("Ninth Sharp 5th", "9#5", &[4, 4, 2, 4]),
    e("Ninth Flat 5th", "9b5", &[4, 2, 4, 4]),
    e("Ninth Sharp 11th", "9#11", &[4, 3, 3, 4, 4]),
    e("Ninth Flat 13th", "9b13", &[4, 3, 3, 4, 6]),
    e("Major Ninth", "Maj9", &[4, 3, 4, 3]),
    e("Major Ninth Suspended 4th", "Maj9sus4", &[5, 2, 4, 3]),
    e("Major Ninth Sharp 5th", "Maj9#5", &[4, 4, 3, 3]),
    e("Major Ninth Sharp 11th", "Maj9#11", &[4, 3, 4, 3, 4]),
    e("Minor Ninth", "m9", &[3, 4, 3, 4]),
    e("Minor add 9th", "madd9", &[3, 4, 7]),
    e("Minor Ninth Flat 5th", "m9b5", &[3, 3, 4, 4]),
    e("Minor Major Ninth", "m9-Maj7", &[3, 4, 4, 3]),
    e("Eleventh", "11", &[4, 3, 3, 4, 3]),
    e("Eleventh Flat 9th", "11b9", &[4, 3, 3, 3, 4]),
    e("Major Eleventh", "Maj11", &[4, 3, 4, 3, 3]),
    e("Minor Eleventh", "m11", &[3, 4, 3, 4, 3]),
    e("Minor Major Eleventh", "m-Maj11", &[3, 4, 4, 3, 3]),
    e("Thirteenth", "13", &[4, 3, 3, 4, 7]),
    e("Thirteenth Sharp 9th", "13#9", &[4, 3, 3, 5, 6]),
    e("Thirteenth Flat 9th", "13b9", &[4, 3, 3, 3, 8]),
    e("Thirteenth Flat 5th Flat 9th", "13b5b9", &[4, 2, 4, 3, 8]),
    e("Major Thirteenth", "Maj13", &[4, 3, 4, 3, 7]),
    e("Minor Thirteenth", "m13", &[3, 4, 3, 4, 7]),
    e("Minor Major Thirteenth", "m-Maj13", &[3, 4, 4, 3, 7]),
    e("Major", "", &[2, 2, 1, 2, 2, 2]),
    e("Major pentatonic", "", &[2, 2, 3, 2]),
    e("Major bebop", "", &[2, 2, 1, 2, 1, 1, 2]),
    e("Minor Harmonic", "", &[2, 1, 2, 2, 1, 3]),
    e("Minor Melodic", "", &[2, 1, 2, 2, 2, 2]),
    e("Minor pentatonic", "", &[3, 2, 2, 3]),
    e("Minor Natural (Aeolian)", "", &[2, 1, 2, 2, 1, 2]),
    e("Minor Neapolitan", "", &[1, 2, 2, 2, 1, 3]),
    e("Minor Hungarian", "", &[2, 1, 3, 1, 1, 3]),
    e("Whole tone", "", &[2, 2, 2, 2, 2]),
    e("Diminished", "", &[2, 1, 2, 1, 2, 1, 2]),
    e("Dominant bebop", "", &[2, 2, 1, 2, 2, 1, 1]),
    e("Jap in sen", "", &[1, 4, 2, 3]),
    e("Blues", "", &[3, 2, 1, 1, 3]),
    e("Arabic", "", &[1, 3, 1, 2, 1, 3]),
    e("Enigmatic", "", &[1, 3, 2, 2, 2, 1]),
    e("Neapolitan", "", &[1, 2, 2, 2, 2, 2]),
    e("Dorian", "", &[2, 1, 2, 2, 2, 1]),
    e("Phrygian", "", &[1, 2, 2, 2, 1, 2]),
    e("Lydian", "", &[2, 2, 2, 1, 2, 2]),
    e("Mixolydian", "", &[2, 2, 1, 2, 2, 1]),
    e("Locrian", "", &[1, 2, 2, 1, 2, 2]),
];

/// The first scale; the entries before it are chords.
pub const FIRST_SCALE: usize = 67;

/// Entries the file's separators come before (the headings "Chords" before 1 and "Scales" before 67 aside).
pub const SEPARATED: [usize; 5] = [10, 15, 40, 55, 60];

/// Entry `k` (1..=88).
pub fn entry(k: usize) -> Option<&'static Entry> {
    k.checked_sub(1).and_then(|i| ENTRIES.get(i))
}

/// The name a menu shows for entry `k`: a chord's short name, a scale's name.
pub fn label(k: usize) -> &'static str {
    entry(k).map_or("", |e| if e.short.is_empty() { e.name } else { e.short })
}

/// The pitch classes entry `k` allows from `root` (0 = C): the root, then each interval added on, mod 12.
pub fn classes(k: usize, root: u8) -> [bool; 12] {
    let mut on = [false; 12];
    let mut pc = usize::from(root % 12);
    on[pc] = true;
    for &iv in entry(k).map_or(&[][..], |e| e.intervals) {
        pc = (pc + usize::from(iv)) % 12;
        on[pc] = true;
    }
    on
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spec's spot checks: entries 1, 7, 10 and 72, the scales' first, the labels and a chord wider than an octave.
    #[test]
    fn entries_and_classes() {
        assert_eq!([entry(1), entry(7), entry(10), entry(72)].map(|e| e.unwrap().intervals), [&[4, 3][..], &[4, 4], &[4, 3, 2], &[3, 2, 2, 3]]);
        assert_eq!((label(1), label(FIRST_SCALE), label(88), label(50)), ("Major", "Major", "Locrian", "Maj9#11"));
        assert!(entry(0).is_none() && entry(89).is_none());
        let major = classes(FIRST_SCALE, 0);
        assert_eq!((0..12).filter(|&i| major[i]).collect::<Vec<_>>(), [0, 2, 4, 5, 7, 9, 11]);
        let d = classes(FIRST_SCALE, 2);
        assert_eq!((0..12).filter(|&i| d[i]).collect::<Vec<_>>(), [1, 2, 4, 6, 7, 9, 11]);
        // 7add13 (4 3 3 11): C E G Bb and A.
        let c = classes(25, 0);
        assert_eq!((0..12).filter(|&i| c[i]).collect::<Vec<_>>(), [0, 4, 7, 9, 10]);
    }
}
