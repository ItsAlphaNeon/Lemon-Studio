// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Chop tool (Tools ▸ Chop…, Alt+U): its settings, its patterns and the chopper itself, free of any
//! window. Each note is cut along a pattern repeated over time: every pattern note it meets gives a piece, moved by the
//! pattern note's key and weighted by its levels. The dialog's session is `piano_dlg.rs`, the window
//! `views/piano_dlg.rs`.
// spec: ui/piano-chop.md

use std::path::{Path, PathBuf};

use os_model::Note;

use crate::arpeggiate::{Pattern, Row, TIME_MUL, TIME_MUL_NAMES};
use crate::playlist::{Cmd, PItem};

/// Wheel notches on the knobs' 0..1 positions (Lemon Studio's; FL's knobs take no wheel, §1): Time mul one step, the
/// Levels 1/32.
pub const TIME_MUL_NOTCH: f32 = 1.0 / 6.0;
pub const LEVEL_NOTCH: f32 = 1.0 / 32.0;

/// The dialog's controls (§1), Reset's values by default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Index into `arpeggiate::TIME_MUL` (3 = ×1).
    pub time_mul: u8,
    /// 0..1, in `quantize::LEVELS` order (PAN … PITCH).
    pub levels: [f32; 6],
    /// Absolute pattern: the pattern repeats from tick 0; off, from each note's own start.
    pub absolute: bool,
    /// Group notes: the pieces of one note become a note group.
    pub group: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { time_mul: 3, levels: [1.0; 6], absolute: true, group: true }
    }
}

impl Settings {
    pub fn time_mul_pos(&self) -> f32 {
        f32::from(self.time_mul.min(6)) / 6.0
    }

    pub fn set_time_mul_pos(&mut self, v: f32) {
        self.time_mul = (v.clamp(0.0, 1.0) * 6.0).round() as u8;
    }

    /// Time mul's hint: "Time multiplicator: x1".
    pub fn time_mul_text(&self) -> String {
        format!("Time multiplicator: {}", TIME_MUL_NAMES[usize::from(self.time_mul.min(6))])
    }
}

// ---------------------------------------------------------------- patterns (§2)

/// The dialog's pattern: a pattern file read as Arpeggiate reads its patterns (§2), or the built-in Default.
#[derive(Debug, Clone, PartialEq)]
pub struct ChopPattern(pub Pattern);

impl Default for ChopPattern {
    /// FL's Default chopping pattern: four one-step C5 notes, a beat.
    fn default() -> Self {
        let rows = (0..4).map(|i| Row { pos: 24 * i, len: 24, key: 60, colour: 0, slide: false, fine: 120, rel: 64, pan: 64, vel: 100, mod_x: 128, mod_y: 128 }).collect();
        ChopPattern(Pattern { name: "Default".into(), path: None, ppq: 96, rows })
    }
}

/// FL's Chopping folder (read in place when FL Studio is installed).
pub fn folder() -> PathBuf {
    os_native::default_fl_dir().join("Data").join("Patches").join("Scores").join("Chopping")
}

/// The ▸ menu (§1): the folder as Quantize lists its templates, the current pattern ticked.
pub fn pattern_menu(cur: &Pattern, root: &Path) -> Vec<PItem> {
    let mut items = crate::quantize::score_items(cur, root, Cmd::ChopPattern);
    crate::quantize::columns(&mut items);
    items
}

// ---------------------------------------------------------------- the chopper (§3-§6)

/// Yellow rows: the length markers (§2).
const MARKER: u8 = 12;
/// Rows in today's colour 4 (old 1): one piece that lasts to the note's end (§4).
const TIED: u8 = 4;

/// A piece before it becomes a note: its source, its pattern row and its span in ticks.
#[derive(Debug, Clone, Copy)]
struct Piece {
    src: usize,
    row: usize,
    s: i64,
    e: i64,
}

/// Chop `notes` (the targets) with pattern `pat` at `ppq` (§3-§6). `used` are the note group numbers in use.
/// Returns the notes that replace the targets: a target the pattern does not reach is gone.
pub fn chop(notes: Vec<Note>, pat: &Pattern, set: &Settings, ppq: u32, used: &[u16]) -> Vec<Note> {
    let (num, den) = TIME_MUL[usize::from(set.time_mul.min(6))];
    let scale = f64::from(ppq.max(1)) / f64::from(pat.ppq.max(1)) * f64::from(num) / f64::from(den);
    // §3: the pattern in the project's ticks, each row's start and length and the pattern's length rounded on their
    // own, halves to even.
    let tick = |t: u32| (f64::from(t) * scale).round_ties_even() as i64;
    let rows: Vec<(i64, i64)> = pat.rows.iter().map(|r| (tick(r.pos), tick(r.len))).collect();
    let marked = pat.rows.iter().any(|r| r.colour == MARKER);
    let len = tick(pat.rows.iter().filter(|r| !marked || r.colour == MARKER).map(|r| r.pos + r.len).max().unwrap_or(0));
    let body: Vec<usize> = (0..pat.rows.len()).filter(|&i| pat.rows[i].colour != MARKER).collect();
    if notes.is_empty() || body.is_empty() || len <= 0 {
        return notes;
    }
    let mut pieces: Vec<Piece> = Vec::new();
    for (i, n) in notes.iter().enumerate() {
        let (s, e) = (i64::from(n.position), i64::from(n.position + n.length));
        // §3: the pattern starts on the repetition the note starts in (from its own start without Absolute); its rows
        // are laid in the pattern's order, repetition after repetition, until one starts at or after the note's end.
        let mut base = if set.absolute { s.div_euclid(len) * len } else { s };
        let mut tied = vec![false; rows.len()];
        'laying: loop {
            for &j in &body {
                if base + rows[j].0 >= e {
                    break 'laying;
                }
                let (a, b) = ((base + rows[j].0).max(s), (base + rows[j].0 + rows[j].1).min(e));
                if b <= a {
                    continue;
                }
                // §4: a tied row's first piece lasts to the note's end, taking in its later ones.
                if pat.rows[j].colour == TIED {
                    if !tied[j] {
                        tied[j] = true;
                        pieces.push(Piece { src: i, row: j, s: a, e });
                    }
                    continue;
                }
                pieces.push(Piece { src: i, row: j, s: a, e: b });
            }
            base += len;
        }
    }
    // §6: a note in a group keeps it. With Group notes each other note gets a new group, in the targets' order from 1
    // past the numbers in use; a note the pattern does not reach still takes its number.
    let mut groups = vec![0u16; notes.len()];
    let mut next = 1u16;
    for (g, n) in groups.iter_mut().zip(&notes) {
        if n.unk14 != 0 {
            *g = n.unk14;
        } else if set.group {
            while used.contains(&next) {
                next += 1;
            }
            *g = next;
            next += 1;
        }
    }
    let mut out: Vec<Note> = pieces
        .iter()
        .map(|p| {
            let (n, r) = (notes[p.src], pat.rows[p.row]);
            let mut o = Note {
                position: p.s as u32,
                length: (p.e - p.s) as u32,
                // §3: the key moved by the row's offset from C5, kept on the keyboard.
                key: (i32::from(n.key) + i32::from(r.key) - 60).clamp(0, 131) as u16,
                flags: 0x4000 | if r.slide { 0x0008 } else { 0 },
                unk14: groups[p.src],
                ..n
            };
            weigh(&mut o, &r, &set.levels);
            o
        })
        .collect();
    out.sort_by_key(|n| (n.position, n.key));
    out
}

/// §5: each level moved the knob's share toward the row's, as Quantize moves them.
fn weigh(n: &mut Note, r: &Row, knobs: &[f32; 6]) {
    for (i, v) in [&mut n.pan, &mut n.velocity, &mut n.release, &mut n.mod_x, &mut n.mod_y, &mut n.fine_pitch].into_iter().enumerate() {
        *v = crate::quantize::level(*v, r.level(i), i, knobs[i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, len: u32, key: u16) -> Note {
        Note { length: len, key, ..Note::step(0, pos) }
    }

    fn notes(v: &[(u32, u32, u16)]) -> Vec<Note> {
        v.iter().map(|&(p, l, k)| note(p, l, k)).collect()
    }

    /// A pattern of (pos, len, key offset, colour, slide) rows with neutral levels at PPQ 96.
    fn pat(rows: &[(u32, u32, i32, u8, bool)]) -> Pattern {
        let mut p = ChopPattern::default().0;
        let base = p.rows[0];
        p.rows = rows.iter().map(|&(pos, len, off, colour, slide)| Row { pos, len, key: (60 + off) as u16, colour, slide, ..base }).collect();
        p
    }

    fn run(ns: Vec<Note>, p: &Pattern, f: impl FnOnce(&mut Settings)) -> Vec<(u32, u32, u16)> {
        let mut set = Settings::default();
        f(&mut set);
        chop(ns, p, &set, 96, &[]).iter().map(|n| (n.position, n.length, n.key)).collect()
    }

    /// The notes FL's batches start from (spec §7).
    const CN: [(u32, u32, u16); 6] = [(0, 96, 60), (96, 48, 62), (192, 96, 64), (288, 24, 65), (384, 192, 67), (600, 30, 69)];
    const ODD: [(u32, u32, u16); 5] = [(10, 100, 60), (130, 50, 62), (250, 7, 64), (300, 60, 65), (400, 200, 67)];

    fn steps(from: u32, to: u32, len: u32, key: u16) -> Vec<(u32, u32, u16)> {
        (from..to).step_by(len as usize).map(|p| (p, len.min(to - p), key)).collect()
    }

    /// §3: Reset on the Default pattern cuts every note on the steps counted from tick 0.
    #[test]
    fn reset_matches_fl() {
        let d = ChopPattern::default().0;
        let got = run(notes(&CN), &d, |_| {});
        let mut fl = steps(0, 96, 24, 60);
        fl.extend(steps(96, 144, 24, 62));
        fl.extend(steps(192, 288, 24, 64));
        fl.push((288, 24, 65));
        fl.extend(steps(384, 576, 24, 67));
        fl.extend([(600, 24, 69), (624, 6, 69)]);
        assert_eq!(got, fl);
        let odd = run(notes(&ODD), &d, |_| {});
        assert_eq!(&odd[..6], &[(10, 14, 60), (24, 24, 60), (48, 24, 60), (72, 24, 60), (96, 14, 60), (130, 14, 62)]);
        let rel = run(notes(&ODD), &d, |s| s.absolute = false);
        assert_eq!(&rel[..6], &[(10, 24, 60), (34, 24, 60), (58, 24, 60), (82, 24, 60), (106, 4, 60), (130, 24, 62)]);
    }

    /// §3: the pattern is laid until it reaches each note's end, from the repetition the note starts in (Absolute) or
    /// from its start; overlapping notes are chopped each on its own.
    #[test]
    fn every_note_is_covered_to_its_end() {
        let t = notes(&[(50, 90, 60), (70, 30, 62), (150, 50, 64), (300, 100, 65), (10, 180, 67)]);
        let d = ChopPattern::default().0;
        let fl = [
            (10, 14, 67), (24, 24, 67), (48, 24, 67), (50, 22, 60), (70, 2, 62), (72, 24, 60), (72, 24, 62), (72, 24, 67), (96, 24, 60), (96, 4, 62),
            (96, 24, 67), (120, 20, 60), (120, 24, 67), (144, 24, 67), (150, 18, 64), (168, 24, 64), (168, 22, 67), (192, 8, 64), (300, 12, 65),
            (312, 24, 65), (336, 24, 65), (360, 24, 65), (384, 16, 65),
        ];
        assert_eq!(run(t.clone(), &d, |_| {}), fl);
        let fl = [
            (10, 24, 67), (34, 24, 67), (50, 24, 60), (58, 24, 67), (70, 24, 62), (74, 24, 60), (82, 24, 67), (94, 6, 62), (98, 24, 60), (106, 24, 67),
            (122, 18, 60), (130, 24, 67), (150, 24, 64), (154, 24, 67), (174, 24, 64), (178, 12, 67), (198, 2, 64), (300, 24, 65), (324, 24, 65),
            (348, 24, 65), (372, 24, 65), (396, 4, 65),
        ];
        assert_eq!(run(t, &d, |s| s.absolute = false), fl);
        // At PPQ 192 the PPQ 96 pattern is twice as long.
        let hi: Vec<(u32, u32, u16)> = CN.iter().map(|&(p, l, k)| (2 * p, 2 * l, k)).collect();
        let got: Vec<_> = chop(notes(&hi), &d, &Settings::default(), 192, &[]).iter().map(|n| (n.position, n.length, n.key)).collect();
        let mut fl = steps(0, 192, 48, 60);
        fl.extend(steps(192, 288, 48, 62));
        fl.extend(steps(384, 576, 48, 64));
        fl.push((576, 48, 65));
        fl.extend(steps(768, 1152, 48, 67));
        fl.extend([(1200, 48, 69), (1248, 12, 69)]);
        assert_eq!(got, fl);
    }

    /// §2, §4: yellow rows only set the length; a tied row's first piece lasts to the note's end. FL's batches.
    #[test]
    fn markers_and_tied_rows_match_fl() {
        let part = pat(&[(0, 96, 0, MARKER, false), (0, 48, 0, TIED, false), (48, 24, 4, 0, false)]);
        let fl = [(0, 96, 60), (48, 24, 64), (96, 48, 62), (192, 96, 64), (240, 24, 68), (288, 24, 65), (384, 192, 67), (432, 24, 71), (528, 24, 71), (600, 30, 69), (624, 6, 73)];
        assert_eq!(run(notes(&CN), &part, |_| {}), fl);
        let mid = pat(&[(0, 96, 0, MARKER, false), (24, 24, 0, TIED, false)]);
        assert_eq!(run(notes(&CN), &mid, |_| {}), [(24, 72, 60), (120, 24, 62), (216, 72, 64), (408, 168, 67), (600, 30, 69)]);
        let pair = pat(&[(0, 48, 0, TIED, false), (48, 48, 0, TIED, false)]);
        let fl = [(0, 96, 60), (48, 48, 60), (96, 48, 62), (192, 96, 64), (240, 48, 64), (288, 24, 65), (384, 192, 67), (432, 144, 67), (600, 30, 69), (624, 6, 69)];
        assert_eq!(run(notes(&CN), &pair, |_| {}), fl);
        // §3: a row placed past the yellow length still counts from its own repetition, and the laying stops at the
        // first row that starts at or after the note's end, so it can leave a tail bare.
        let beyond = pat(&[(0, 48, 0, MARKER, false), (0, 24, 0, 0, false), (60, 24, 4, 0, false)]);
        let fl = [(0, 24, 60), (48, 24, 60), (60, 24, 64), (96, 24, 62), (192, 24, 64), (240, 24, 64), (252, 24, 68), (288, 24, 65), (384, 24, 67), (432, 24, 67), (444, 24, 71), (480, 24, 67), (492, 24, 71), (528, 24, 67), (540, 24, 71)];
        assert_eq!(run(notes(&CN), &beyond, |_| {}), fl);
        let far = pat(&[(0, 48, 0, MARKER, false), (0, 24, 0, 0, false), (100, 24, 4, 0, false)]);
        let fl = [(0, 24, 60), (96, 24, 62), (192, 24, 64), (288, 24, 65), (384, 24, 67), (432, 24, 67), (480, 24, 67), (484, 24, 71), (532, 24, 71)];
        assert_eq!(run(notes(&CN), &far, |_| {}), fl);
        let long = pat(&[(0, 48, 0, MARKER, false), (0, 24, 0, 0, false), (24, 60, 4, 0, false)]);
        assert_eq!(&run(notes(&CN), &long, |_| {})[..4], &[(0, 24, 60), (24, 60, 64), (48, 24, 60), (72, 24, 64)]);
        assert_eq!(&run(notes(&CN), &long, |_| {})[19..], &[(600, 30, 73), (624, 6, 69)]);
        // Other colours are plain rows.
        let one = pat(&[(0, 96, 0, 1, false), (0, 24, 7, 0, false)]);
        let got = run(notes(&CN), &one, |_| {});
        assert_eq!(&got[8..12], &[(384, 96, 67), (384, 24, 74), (480, 96, 67), (480, 24, 74)]);
        // A slide row's pieces slide.
        let slide = pat(&[(0, 96, 0, TIED, true)]);
        assert!(chop(notes(&CN), &slide, &Settings::default(), 96, &[]).iter().all(|n| n.flags == 0x4008));
    }

    /// §3: keys moved off the keyboard stay on its ends.
    #[test]
    fn keys_stay_on_the_keyboard() {
        let p = pat(&[(0, 24, 7, 0, false), (24, 24, -5, 0, false), (48, 24, 0, 0, false)]);
        let fl = [(0, 24, 131), (24, 24, 122), (48, 24, 127), (72, 24, 131), (96, 24, 0), (120, 24, 2), (144, 24, 9), (168, 24, 0)];
        assert_eq!(run(notes(&[(0, 96, 127), (96, 96, 2)]), &p, |_| {}), fl);
    }

    /// §3: with Time mul at 1/4 the rows' starts and lengths are rounded to ticks first, halves to even (12.5 → 12).
    #[test]
    fn fractional_ticks_round_half_to_even() {
        let p = pat(&[(0, 50, 0, 0, false), (50, 50, 7, 0, false)]);
        let quarter = |s: &mut Settings| s.time_mul = 1;
        let fl = [
            (0, 12, 60), (12, 12, 67), (25, 12, 60), (37, 12, 67), (50, 12, 60), (62, 12, 67), (75, 12, 60), (87, 9, 67), (96, 3, 69), (100, 12, 62),
            (112, 12, 69), (125, 12, 62), (137, 7, 69), (192, 7, 71), (200, 12, 64), (212, 12, 71), (225, 12, 64), (237, 12, 71), (250, 12, 64),
            (262, 12, 71), (275, 12, 64), (287, 1, 71), (288, 11, 72), (300, 12, 65), (384, 3, 67), (387, 12, 74), (400, 12, 67), (412, 12, 74),
            (425, 12, 67), (437, 12, 74), (450, 12, 67), (462, 12, 74), (475, 12, 67), (487, 12, 74), (500, 12, 67), (512, 12, 74), (525, 12, 67),
            (537, 12, 74), (550, 12, 67), (562, 12, 74), (575, 1, 67), (600, 12, 69), (612, 12, 76), (625, 5, 69),
        ];
        assert_eq!(run(notes(&CN), &p, quarter), fl);
        let got = run(notes(&CN), &p, |s| {
            quarter(s);
            s.absolute = false;
        });
        assert_eq!(&got[8..12], &[(96, 12, 62), (108, 12, 69), (121, 12, 62), (133, 11, 69)]);
        assert_eq!(&got[38..], &[(600, 12, 69), (612, 12, 76), (625, 5, 69)]);
        // 1.5 ticks round up to 2, 4.5 down to 4.
        let p = pat(&[(0, 6, 0, 0, false), (6, 6, 0, 0, false), (12, 6, 0, 0, false), (18, 6, 0, 0, false)]);
        assert_eq!(run(notes(&[(0, 6, 60)]), &p, quarter), [(0, 2, 60), (2, 2, 60), (3, 2, 60), (4, 2, 60)]);
    }

    /// §5: the Levels knobs move each level their share toward the row's, the move truncated; FL's knobs dragged down
    /// 0, 16, 48 and 100 px (3/512 a pixel).
    #[test]
    fn levels_match_fl() {
        let mut p = pat(&[(0, 24, 0, 0, false), (24, 24, 0, 0, false)]);
        (p.rows[0].vel, p.rows[0].pan, p.rows[0].rel, p.rows[0].mod_x, p.rows[0].mod_y, p.rows[0].fine) = (20, 0, 0, 0, 255, 0);
        (p.rows[1].vel, p.rows[1].pan, p.rows[1].rel, p.rows[1].mod_x, p.rows[1].mod_y, p.rows[1].fine) = (127, 128, 128, 255, 0, 240);
        let src = Note { length: 48, key: 60, velocity: 80, pan: 80, release: 50, mod_x: 100, mod_y: 150, fine_pitch: 100, ..Note::step(0, 0) };
        let fl = [
            (1.0, [(16, 16, 0, 0, 255, 0), (128, 101, 114, 227, 22, 220)]),
            (29.0 / 32.0, [(22, 22, 0, 0, 255, 0), (128, 99, 108, 215, 34, 208)]),
            (23.0 / 32.0, [(34, 34, 4, 8, 241, 14), (126, 95, 96, 191, 58, 186)]),
            (1.0 - 300.0 / 512.0, [(54, 54, 24, 47, 202, 51), (106, 88, 76, 152, 97, 149)]),
        ];
        for (k, want) in fl {
            let set = Settings { levels: [k; 6], ..Settings::default() };
            let got: Vec<_> = chop(vec![src], &p, &set, 96, &[]).iter().map(|n| (n.pan, n.velocity, n.release, n.mod_x, n.mod_y, n.fine_pitch)).collect();
            assert_eq!(got, want, "knobs at {k}");
        }
    }

    /// §6: a grouped note's pieces keep its group; Group notes gives each other note a new one in the targets' order,
    /// past the numbers in use, and a note that gives no piece still takes its number. FL's batches (their groups as
    /// FL renumbers them on loading).
    #[test]
    fn groups_match_fl() {
        let run = |v: &[(u32, u32, u16, u16)], group: bool, used: &[u16], p: &Pattern| -> Vec<(u32, u16, u16)> {
            let ns = v.iter().map(|&(p, l, k, g)| Note { unk14: g, ..note(p, l, k) }).collect();
            let set = Settings { group, ..Settings::default() };
            chop(ns, p, &set, 96, used).iter().map(|n| (n.position, n.key, n.unk14)).collect()
        };
        // The Default pattern cuts each of these notes in two: one entry per note.
        let firsts = |v: Vec<(u32, u16, u16)>| v.into_iter().filter(|n| n.0 % 96 == 0).collect::<Vec<_>>();
        let d = ChopPattern::default().0;
        let gr = [(0, 48, 60, 0), (96, 48, 62, 1), (192, 48, 64, 0), (288, 48, 65, 2), (384, 48, 67, 1)];
        assert_eq!(firsts(run(&gr, true, &[1, 2], &d)), [(0, 60, 3), (96, 62, 1), (192, 64, 4), (288, 65, 2), (384, 67, 1)]);
        assert_eq!(firsts(run(&gr, false, &[1, 2], &d)), [(0, 60, 0), (96, 62, 1), (192, 64, 0), (288, 65, 2), (384, 67, 1)]);
        assert_eq!(firsts(run(&[(96, 48, 60, 0), (0, 48, 64, 0), (192, 48, 62, 0)], true, &[], &d)), [(0, 64, 2), (96, 60, 1), (192, 62, 3)]);
        assert_eq!(firsts(run(&[(192, 48, 64, 1), (0, 48, 60, 2), (96, 48, 62, 0)], true, &[1, 2], &d)), [(0, 60, 2), (96, 62, 3), (192, 64, 1)]);
        let mid = pat(&[(0, 96, 0, MARKER, false), (24, 24, 0, TIED, false)]);
        let cn: Vec<_> = CN.iter().map(|&(p, l, k)| (p, l, k, 0)).collect();
        assert_eq!(run(&cn, true, &[], &mid), [(24, 60, 1), (120, 62, 2), (216, 64, 3), (408, 67, 5), (600, 69, 6)]);
    }
}
