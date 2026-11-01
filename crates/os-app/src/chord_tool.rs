// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! TOOLS > Chord progression tool... (Lemon Studio's design; spec: ui/piano-tools.md §4 "Lemon Studio"): a typed
//! progression written into the current pattern on the selected channel, one chord a bar. The original's tool
//! generates progressions with a model of its own; only its deterministic part, typed progressions, is done here.
//!
//! A progression is chords separated by spaces, commas or `|`, with an optional key first (`A:` or `F#m:`; C major
//! without one):
//! - chord names: a root `C`..`B` with `#` or `b`, then `m`, `dim`, `aug`, `sus2`, `sus4`, and `7`, `maj7`, `6`, `9`,
//!   `add9`; `/E` puts that bass note under the chord;
//! - Roman numerals counted in the key (its major or natural minor scale), `b` or `#` before them: upper case major,
//!   lower case minor, `o` diminished, `+` augmented, with the same endings (`V7`, `ii7`, `viio`, `IVsus2`);
//! - `_`: a bar without a chord.

use std::sync::Mutex;

use os_model::Note;

use crate::app::State;

/// The undo step's name.
pub const STEP: &str = "Chord progression";

/// What the box starts with: the last progression written, at first `I V vi IV`.
static LAST: Mutex<String> = Mutex::new(String::new());

pub fn last() -> String {
    let l = LAST.lock().map(|l| l.clone()).unwrap_or_default();
    if l.is_empty() { "I V vi IV".into() } else { l }
}

/// A chord's keys relative to its root (root position), and the bass under it (semitones above C), if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Chord {
    pub root: u8,
    pub intervals: Vec<u8>,
    pub bass: Option<u8>,
}

fn letter(c: char) -> Option<u8> {
    Some(match c {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    })
}

/// A note name (`C`, `F#`, `Bb`) at the start of `t`: its pitch class and the rest.
fn note_name(t: &str) -> Option<(u8, &str)> {
    let mut cs = t.chars();
    let pc = letter(cs.next()?)?;
    let rest = cs.as_str();
    Some(match rest.chars().next() {
        Some('#') => ((pc + 1) % 12, &rest[1..]),
        Some('b') => ((pc + 11) % 12, &rest[1..]),
        _ => (pc, rest),
    })
}

/// A chord's quality and endings (after the root): the intervals, or `None` for text it does not know.
fn quality(mut t: &str, minor: bool) -> Option<Vec<u8>> {
    let take = |p: &str, t: &mut &str| {
        let ok = t.starts_with(p);
        if ok {
            *t = &t[p.len()..];
        }
        ok
    };
    let mut third = if minor { 3 } else { 4 };
    let mut fifth = 7;
    let mut sus = None;
    if take("maj7", &mut t) || take("M7", &mut t) {
        return (t.is_empty() && !minor).then(|| vec![0, 4, 7, 11]);
    }
    if take("min", &mut t) || take("m", &mut t) || take("-", &mut t) {
        third = 3;
    } else if take("dim", &mut t) || take("o", &mut t) || take("°", &mut t) {
        (third, fifth) = (3, 6);
    } else if take("aug", &mut t) || take("+", &mut t) {
        (third, fifth) = (4, 8);
    }
    if take("sus2", &mut t) {
        sus = Some(2);
    } else if take("sus4", &mut t) || take("sus", &mut t) {
        sus = Some(5);
    }
    let mut v = vec![0, sus.unwrap_or(third), fifth];
    match t {
        "" => {}
        "7" if fifth == 6 && third == 3 => v.push(9),
        "7" => v.push(10),
        "maj7" | "M7" => v.push(11),
        "6" => v.push(9),
        "9" => v.extend([10, 14]),
        "add9" => v.push(14),
        _ => return None,
    }
    Some(v)
}

/// A Roman numeral (I..VII, either case) at the start of `t`: degree 0..6, upper case, the rest.
fn numeral(t: &str) -> Option<(usize, bool, &str)> {
    const N: [&str; 7] = ["VII", "III", "IV", "VI", "V", "II", "I"];
    const DEG: [usize; 7] = [6, 2, 3, 5, 4, 1, 0];
    for (i, n) in N.iter().enumerate() {
        if t.len() >= n.len() && t.is_char_boundary(n.len()) {
            let head = &t[..n.len()];
            if head == *n || head == n.to_lowercase() {
                return Some((DEG[i], head == *n, &t[n.len()..]));
            }
        }
    }
    None
}

/// One chord of a progression in key `key` (root pitch class, minor); `Ok(None)` is a bar without a chord.
pub fn parse_chord(t: &str, key: (u8, bool)) -> Result<Option<Chord>, ()> {
    if t == "_" {
        return Ok(None);
    }
    let (head, bass) = match t.split_once('/') {
        Some((h, b)) => (h, Some(note_name(b).filter(|(_, r)| r.is_empty()).ok_or(())?.0)),
        None => (t, None),
    };
    if let Some((root, rest)) = note_name(head) {
        let intervals = quality(rest, false).ok_or(())?;
        return Ok(Some(Chord { root, intervals, bass }));
    }
    let (shift, body) = match head.chars().next() {
        Some('b') => (11u8, &head[1..]),
        Some('#') => (1, &head[1..]),
        _ => (0, head),
    };
    let (deg, upper, rest) = numeral(body).ok_or(())?;
    const MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
    const MINOR: [u8; 7] = [0, 2, 3, 5, 7, 8, 10];
    let scale = if key.1 { MINOR } else { MAJOR };
    let root = (key.0 + scale[deg] + shift) % 12;
    let intervals = quality(rest, !upper).ok_or(())?;
    Ok(Some(Chord { root, intervals, bass }))
}

/// A progression: its chords (`None` for a bar without one), or the first word it does not read.
pub fn parse(text: &str) -> Result<Vec<Option<Chord>>, String> {
    let mut words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == ',' || c == '|').filter(|w| !w.is_empty()).collect();
    let mut key = (0, false);
    if let Some(k) = words.first().and_then(|w| w.strip_suffix(':')) {
        let (pc, rest) = note_name(k).ok_or_else(|| words[0].to_string())?;
        key = match rest {
            "" => (pc, false),
            "m" => (pc, true),
            _ => return Err(words[0].to_string()),
        };
        words.remove(0);
    }
    words.iter().map(|w| parse_chord(w, key).map_err(|_| (*w).to_string())).collect()
}

/// The notes of `chords` on `channel` from tick `at`, `bar` ticks each: the root between C4 and B4 (keys 48..59), the
/// chord above it, a bass an octave below.
pub fn notes(chords: &[Option<Chord>], channel: u16, at: u32, bar: u32) -> Vec<Note> {
    let mut v = Vec::new();
    for (i, c) in chords.iter().enumerate() {
        let Some(c) = c else { continue };
        let position = at + i as u32 * bar;
        let note = |key: u8| Note { key: u16::from(key), length: bar, ..Note::step(channel, position) };
        if let Some(b) = c.bass {
            v.push(note(36 + b));
        }
        v.extend(c.intervals.iter().map(|&k| note(48 + c.root + k)));
    }
    v
}

/// Writes the progression in `text` into the current pattern on the selected channel, after the channel's last note
/// (from the next bar; from the pattern's start when it has none), one undo step. The message to show.
pub fn write(s: &mut State, text: &str) -> String {
    let chords = match parse(text) {
        Ok(c) if c.iter().any(Option::is_some) => c,
        Ok(_) => return "Chord progression: type chords, as C G Am F or I V vi IV".into(),
        Err(w) => return format!("Chord progression: \"{w}\" is not a chord"),
    };
    let Some(ch) = crate::piano::channel(s) else { return "Chord progression: no instrument to write to".into() };
    if let Ok(mut l) = LAST.lock() {
        *l = text.trim().to_string();
    }
    let bar = s.project.bar_ticks();
    let end = crate::piano::notes(s).iter().filter(|n| n.channel == ch).map(|n| n.position + n.length.max(1)).max();
    let at = end.map_or(0, |e| e.div_ceil(bar) * bar);
    let new = notes(&chords, ch, at, bar);
    crate::piano::snapshot_as(s, STEP);
    let p = crate::piano::pattern_mut(s);
    let first = p.notes.len();
    p.notes.extend(new);
    let last = p.notes.len();
    s.piano.select(s.pattern, ch, (first..last).collect());
    crate::piano::finish_edit(s);
    let n = chords.iter().filter(|c| c.is_some()).count();
    format!("Wrote {n} chord{} into the pattern", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<Vec<u8>> {
        parse(text).unwrap().into_iter().map(|c| c.map_or(vec![], |c| c.intervals.iter().map(|k| c.root + k).collect())).collect()
    }

    /// Names and numerals give the same chords; endings, slash bass, keys and rests.
    #[test]
    fn progressions_are_read() {
        assert_eq!(keys("C G Am F"), keys("I, V, vi, IV"));
        assert_eq!(keys("C G Am F"), vec![vec![0, 4, 7], vec![7, 11, 14], vec![9, 12, 16], vec![5, 9, 12]]);
        assert_eq!(keys("G7 | Cmaj7 Dm7 Bdim Bdim7 Caug Dsus4 Csus2 C6 Cadd9"), vec![
            vec![7, 11, 14, 17],
            vec![0, 4, 7, 11],
            vec![2, 5, 9, 12],
            vec![11, 14, 17],
            vec![11, 14, 17, 20],
            vec![0, 4, 8],
            vec![2, 7, 9],
            vec![0, 2, 7],
            vec![0, 4, 7, 9],
            vec![0, 4, 7, 14],
        ]);
        // In A minor: i iv v and bVII... III is C major, VII G major.
        assert_eq!(keys("Am: i iv V7 III VII"), keys("Am Dm E7 C G"));
        assert_eq!(keys("D: I bVII ii7 viio"), keys("D C Em7 C#dim"));
        assert_eq!(keys("C _ F"), vec![vec![0, 4, 7], vec![], vec![5, 9, 12]]);
        assert_eq!(parse("C/E").unwrap()[0].as_ref().unwrap().bass, Some(4));
        assert_eq!(parse("C H"), Err("H".into()));
        assert_eq!(parse("Cx"), Err("Cx".into()));
        assert_eq!(parse("Q: I"), Err("Q:".into()));
    }

    /// The chords go into the pattern a bar each after the channel's last note, as one undo step.
    #[test]
    fn a_progression_is_written_after_the_notes() {
        let mut app = crate::app::App::new(None);
        let s = &mut app.state;
        let ch = crate::piano::channel(s).unwrap();
        let bar = s.project.bar_ticks();
        crate::piano::pattern_mut(s).notes = vec![Note { length: 10, ..Note::step(ch, bar / 2) }];
        let steps = s.history.at();
        assert_eq!(write(s, "C/E G"), "Wrote 2 chords into the pattern");
        let notes: Vec<(u32, u16, u32)> = crate::piano::notes(s).iter().skip(1).map(|n| (n.position, n.key, n.length)).collect();
        assert_eq!(notes, vec![(bar, 40, bar), (bar, 48, bar), (bar, 52, bar), (bar, 55, bar), (2 * bar, 55, bar), (2 * bar, 59, bar), (2 * bar, 62, bar)]);
        assert_eq!(s.history.at(), steps + 1);
        assert_eq!(last(), "C/E G");
        assert_eq!(write(s, "C X"), "Chord progression: \"X\" is not a chord");
        assert_eq!(s.history.at(), steps + 1, "nothing written");
        crate::history::undo(s);
        assert_eq!(crate::piano::notes(s).len(), 1);
    }
}
