// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Quantize tool (Tools ▸ Quantize…, Alt+Q): its settings, its templates and the quantizer itself, free
//! of any window. Each note is matched to the nearest note of a template that repeats over the pattern, and moves a
//! share of the way to it. The dialog's session is `piano_dlg.rs`, the window `views/piano_dlg.rs`.
// spec: ui/piano-quantize.md

use std::path::{Path, PathBuf};

use os_model::Note;

use crate::arpeggiate::{self, Pattern, Row};
use crate::playlist::{Cmd, Mark, PItem};

/// Wheel notches on the knobs' 0..1 positions (§1): Start time and Duration 1/32, the Levels 3/64, Sensitivity one of
/// its 45 steps.
pub const STRENGTH_NOTCH: f32 = 1.0 / 32.0;
pub const LEVEL_NOTCH: f32 = 3.0 / 64.0;
pub const SENS_NOTCH: f32 = 1.0 / (SENS_MAX - SENS_MIN) as f32;
/// Sensitivity in 1/48 of a beat (2 ticks at PPQ 96): 3..48, 9 after Reset (§1).
pub const SENS_MIN: u8 = 3;
pub const SENS_MAX: u8 = 48;
const SENS_RESET: u8 = 9;

/// The duration menu (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    LeaveDuration,
    LeaveEnd,
    QuantizeDuration,
    #[default]
    QuantizeEnd,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::LeaveDuration, Mode::LeaveEnd, Mode::QuantizeDuration, Mode::QuantizeEnd];

    pub fn name(self) -> &'static str {
        match self {
            Mode::LeaveDuration => "Leave duration",
            Mode::LeaveEnd => "Leave end time",
            Mode::QuantizeDuration => "Quantize duration",
            Mode::QuantizeEnd => "Quantize end time",
        }
    }
}

/// The Levels knobs (§6): label and hint, in the dialog's order (PAN … PITCH).
pub const LEVELS: [(&str, &str); 6] = [
    ("PAN", "Panning"),
    ("VEL", "Velocity"),
    ("REL", "Release"),
    ("MOD X", "Mod X"),
    ("MOD Y", "Mod Y"),
    ("PITCH", "Fine pitch"),
];

/// The dialog's controls (§1), Reset's values by default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Start time: the share of the way a start moves, 0..1.
    pub start: f32,
    /// Sensitivity in 1/48 beat, `SENS_MIN..=SENS_MAX`.
    pub sens: u8,
    /// Duration: the share of the way a length or an end moves, 0..1.
    pub dur: f32,
    pub mode: Mode,
    /// 0..1, in `LEVELS` order.
    pub levels: [f32; 6],
}

impl Default for Settings {
    fn default() -> Self {
        Settings { start: 0.5, sens: SENS_RESET, dur: 0.5, mode: Mode::QuantizeEnd, levels: [1.0; 6] }
    }
}

/// Whole percent, halves to even, as FL's hints.
fn pct(v: f32) -> i64 {
    (f64::from(v) * 100.0).round_ties_even() as i64
}

impl Settings {
    /// Sensitivity in ticks at `ppq`: a note is quantized only when it lies nearer than this (§3).
    pub fn sens_ticks(&self, ppq: u32) -> f64 {
        f64::from(self.sens.clamp(SENS_MIN, SENS_MAX)) * f64::from(ppq.max(1)) / 48.0
    }

    /// Sensitivity's knob position, 0..1.
    pub fn sens_pos(&self) -> f32 {
        f32::from(self.sens.clamp(SENS_MIN, SENS_MAX) - SENS_MIN) / f32::from(SENS_MAX - SENS_MIN)
    }

    pub fn set_sens_pos(&mut self, v: f32) {
        self.sens = SENS_MIN + (v.clamp(0.0, 1.0) * f32::from(SENS_MAX - SENS_MIN)).round() as u8;
    }

    /// Start time's and Duration's hint: "50%".
    pub fn pct_text(v: f32) -> String {
        format!("{}%", pct(v))
    }

    /// Sensitivity's hint: the distance as steps:ticks at `ppq` ("0:18" after Reset at PPQ 96).
    pub fn sens_text(&self, ppq: u32) -> String {
        let step = f64::from(ppq.max(4)) / 4.0;
        let t = self.sens_ticks(ppq).round();
        format!("{}:{:02}", (t / step).floor(), t - (t / step).floor() * step)
    }
}

// ---------------------------------------------------------------- templates (§2)

/// The dialog's template: a pattern file read as Arpeggiate reads its patterns (§2), or the built-in Default.
#[derive(Debug, Clone, PartialEq)]
pub struct Template(pub Pattern);

impl Default for Template {
    /// FL's Default template: sixteen one-step notes, one on every step of a bar, with neutral levels.
    fn default() -> Self {
        Template::grid("Default", 24)
    }
}

/// The ▸ menu's Snap entries (§2): their names and grids in ticks at PPQ 96.
pub const SNAPS: [(&str, u32); 8] = [("Bar", 384), ("Beat", 96), ("Half-beat", 48), ("Half-step", 12), ("Quarter-step", 6), ("Sixth-beat", 16), ("Step", 24), ("Third-beat", 32)];

impl Template {
    /// A bar of notes every `step` ticks at PPQ 96, each `step` long, with neutral levels.
    fn grid(name: &str, step: u32) -> Self {
        let rows = (0..384 / step).map(|i| Row { pos: step * i, len: step, key: 60, colour: 0, slide: false, fine: 120, rel: 64, pan: 64, vel: 100, mod_x: 128, mod_y: 128 }).collect();
        Template(Pattern { name: name.into(), path: None, ppq: 96, rows })
    }

    /// The Snap entry `i` of `SNAPS`.
    pub fn snap(i: usize) -> Self {
        let (name, step) = SNAPS[i.min(SNAPS.len() - 1)];
        Template::grid(name, step)
    }
}

/// FL's Quantization folder (read in place when FL Studio is installed).
pub fn folder() -> PathBuf {
    os_native::default_fl_dir().join("Data").join("Patches").join("Scores").join("Quantization")
}

/// A pick from the dialog's menus.
#[derive(Debug, Clone, PartialEq)]
pub enum QCmd {
    /// A template file; None for the built-in Default.
    Template(Option<PathBuf>),
    /// One of the Snap entries (an index into `SNAPS`).
    Snap(u8),
    Mode(Mode),
}

/// The ▸ menu (§2): the folder's templates, then each sub-folder under its name as a heading with its templates, deeper
/// folders as submenus, then the Snap grids under their heading. Entries named as the current template are ticked (FL
/// marks every "Default" when it is current). Without a Default file in the folder the built-in one comes first.
pub fn template_menu(cur: &Pattern, root: &Path) -> Vec<PItem> {
    let mut items = score_items(cur, root, |p| Cmd::Quant(QCmd::Template(p)));
    items.push(PItem::head("Snap"));
    for (i, &(name, _)) in SNAPS.iter().enumerate() {
        let on = cur.path.is_none() && cur.name == name;
        items.push(PItem::new(name, Cmd::Quant(QCmd::Snap(i as u8))).mark(Mark::Radio(on)));
    }
    columns(&mut items);
    items
}

/// A score folder as FL's Quantize and Chop menus list it (§2): its files, then each sub-folder under its name as a
/// heading with its files, deeper folders as submenus. `pick` makes an entry's command (None: the built-in Default,
/// listed first when the folder has no Default file). Entries named as the current pattern are ticked.
pub fn score_items(cur: &Pattern, root: &Path, pick: impl Fn(Option<PathBuf>) -> Cmd) -> Vec<PItem> {
    let item = |name: String, path: Option<PathBuf>| {
        let on = cur.name == name;
        PItem::new(name, pick(path)).mark(Mark::Radio(on))
    };
    let stem = |p: &Path| p.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let name = |p: &Path| p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut items = Vec::new();
    let (files, dirs) = arpeggiate::listing(root);
    if !files.iter().any(|p| stem(p).eq_ignore_ascii_case("Default")) {
        items.push(item("Default".into(), None));
    }
    items.extend(files.into_iter().map(|p| item(stem(&p), Some(p))));
    for d in dirs {
        let (files, subs) = arpeggiate::listing(&d);
        let mut part: Vec<PItem> = files.into_iter().map(|p| item(stem(&p), Some(p))).collect();
        for sub in subs {
            let (inner, _) = arpeggiate::listing(&sub);
            if !inner.is_empty() {
                part.push(PItem::sub(name(&sub), inner.into_iter().map(|p| item(stem(&p), Some(p))).collect()));
            }
        }
        if !part.is_empty() {
            items.push(PItem::head(name(&d)));
            items.extend(part);
        }
    }
    items
}

/// A long menu goes on in further columns (FL's own breaks where the screen ends).
pub fn columns(items: &mut [PItem]) {
    for i in (MENU_COLUMN..items.len()).step_by(MENU_COLUMN) {
        items[i - 1].col = true;
    }
}

/// The template menu's entries a column.
const MENU_COLUMN: usize = 48;

/// The duration menu, the current mode ticked.
pub fn mode_menu(set: &Settings) -> Vec<PItem> {
    Mode::ALL.iter().map(|&m| PItem::new(m.name(), Cmd::Quant(QCmd::Mode(m))).mark(Mark::Radio(set.mode == m))).collect()
}

// ---------------------------------------------------------------- the quantizer (§3-§6)

/// A template note at the project's PPQ: its start, end, length and levels.
struct TNote {
    start: i64,
    end: i64,
    len: i64,
    row: Row,
}

/// The template's notes in project ticks, and the period they repeat at: their largest end rounded up to whole beats
/// (§2). Times are rounded to the nearest tick, halves to the even one; a length is the rounded end less the rounded start.
fn marks(tpl: &Pattern, ppq: u32) -> (Vec<TNote>, i64) {
    let k = f64::from(ppq.max(1)) / f64::from(tpl.ppq.max(1));
    let at = |t: u32| (f64::from(t) * k).round_ties_even() as i64;
    let ms: Vec<TNote> = tpl.rows.iter().map(|r| TNote { start: at(r.pos), end: at(r.pos + r.len), len: at(r.pos + r.len) - at(r.pos), row: *r }).collect();
    let beat = i64::from(ppq.max(1));
    let last = ms.iter().map(|m| m.end).max().unwrap_or(0).max(1);
    (ms, (last + beat - 1) / beat * beat)
}

/// The nearest of `points` repeated every `period` to `x`, and which point it is; a tie goes to the earlier.
fn nearest(x: i64, points: impl Iterator<Item = i64> + Clone, period: i64) -> Option<(i64, usize)> {
    let k0 = x.div_euclid(period);
    let mut best: Option<(i64, i64, usize)> = None;
    for k in k0 - 1..=k0 + 1 {
        for (i, p) in points.clone().enumerate() {
            let t = p + k * period;
            let key = ((t - x).abs(), t);
            if best.is_none() || best.is_some_and(|b| key < (b.0, b.1)) {
                best = Some((key.0, t, i));
            }
        }
    }
    best.map(|b| (b.1, b.2))
}

/// `x` moved the share `k` of the way to `target`, the move truncated toward `x` (§3).
fn toward(x: i64, target: f64, k: f32) -> i64 {
    x + ((target - x as f64) * f64::from(k)).trunc() as i64
}

/// A level moved toward the template row's (§6): velocity scales by the row's / 100 (the product truncated), the others
/// add the row's offset from its centre; the result is clamped to the level's range. Chop weighs its pieces the same
/// way (piano-chop.md §5).
pub(crate) fn level(x: u8, row: u8, i: usize, k: f32) -> u8 {
    let (centre, top) = [(64.0, 128), (100.0, 128), (64.0, 128), (128.0, 255), (128.0, 255), (120.0, 240)][i];
    let x64 = f64::from(x);
    let target = if i == 1 { (x64 * f64::from(row) / 100.0).trunc() } else { x64 + f64::from(row) - centre };
    toward(i64::from(x), target, k).clamp(0, top) as u8
}

/// Quantize `notes` to `tpl` at `ppq` (§3-§6); the order is kept.
pub fn quantize(notes: Vec<Note>, tpl: &Pattern, set: &Settings, ppq: u32) -> Vec<Note> {
    let (ms, period) = marks(tpl, ppq);
    if ms.is_empty() {
        return notes;
    }
    let sens = set.sens_ticks(ppq);
    let min = i64::from((ppq / 24).max(1));
    notes
        .into_iter()
        .map(|mut n| {
            let (s, len) = (i64::from(n.position), i64::from(n.length));
            let e = s + len;
            let hit = nearest(s, ms.iter().map(|m| m.start), period).filter(|&(t, _)| ((t - s).abs() as f64) < sens);
            let mut s2 = s;
            if let Some((t, i)) = hit {
                // Lemon Studio keeps starts at or after tick 0 (§7).
                s2 = toward(s, t as f64, set.start).max(0);
                let row = ms[i].row;
                let levels = [row.pan, row.vel, row.rel, row.mod_x, row.mod_y, row.fine];
                let mine = [&mut n.pan, &mut n.velocity, &mut n.release, &mut n.mod_x, &mut n.mod_y, &mut n.fine_pitch];
                for (j, x) in mine.into_iter().enumerate() {
                    *x = level(*x, levels[j], j, set.levels[j]);
                }
            }
            let len2 = match set.mode {
                Mode::LeaveDuration => len,
                Mode::LeaveEnd => (e - s2).max(min),
                Mode::QuantizeDuration => hit.map_or(len, |(_, i)| toward(len, ms[i].len as f64, set.dur)),
                Mode::QuantizeEnd => {
                    let e2 = nearest(e, ms.iter().map(|m| m.end), period).filter(|&(t, _)| ((t - e).abs() as f64) < sens).map_or(e, |(t, _)| toward(e, t as f64, set.dur));
                    (e2 - s2).max(min)
                }
            };
            n.position = s2 as u32;
            n.length = len2.max(0) as u32;
            n
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, len: u32) -> Note {
        Note { length: len, ..Note::step(0, pos) }
    }

    fn notes(v: &[(u32, u32)]) -> Vec<Note> {
        v.iter().map(|&(p, l)| note(p, l)).collect()
    }

    /// A template of (pos, len) rows with neutral levels at `ppq`.
    fn tpl(rows: &[(u32, u32)], ppq: u32) -> Pattern {
        let mut p = Template::default().0;
        let base = p.rows[0];
        p.rows = rows.iter().map(|&(pos, len)| Row { pos, len, ..base }).collect();
        p.ppq = ppq;
        p
    }

    fn run(ns: Vec<Note>, tpl: &Pattern, ppq: u32, f: impl FnOnce(&mut Settings)) -> Vec<(u32, u32)> {
        let mut set = Settings::default();
        f(&mut set);
        quantize(ns, tpl, &set, ppq).iter().map(|n| (n.position, n.length)).collect()
    }

    /// The notes FL's batches start from (spec §8).
    const QN: [(u32, u32); 12] = [(5, 20), (30, 30), (50, 10), (70, 40), (100, 24), (130, 50), (200, 7), (250, 100), (301, 13), (347, 60), (395, 2), (420, 36)];

    /// §3, §5: Reset's settings on the Default template (Start 50 %, Duration 50 %, Quantize end time, 0:18).
    #[test]
    fn reset_matches_fl() {
        let got = run(notes(&QN), &Template::default().0, 96, |_| {});
        let fl = [(3, 22), (27, 27), (49, 5), (71, 44), (98, 24), (125, 49), (196, 15), (245, 110), (306, 7), (342, 65), (390, 12), (414, 42)];
        assert_eq!(got, fl);
    }

    /// §5: Leave end time keeps the end, at least a 24th of a beat; Quantize duration takes the template's length.
    #[test]
    fn duration_modes_match_fl() {
        let d = Template::default().0;
        let leave_end = run(notes(&QN), &d, 96, |s| (s.start, s.mode) = (1.0, Mode::LeaveEnd));
        assert_eq!(leave_end, [(0, 25), (24, 36), (48, 12), (72, 38), (96, 28), (120, 60), (192, 15), (240, 110), (312, 4), (336, 71), (384, 13), (408, 48)]);
        let q_dur = run(notes(&QN), &d, 96, |s| (s.start, s.dur, s.mode) = (1.0, 1.0, Mode::QuantizeDuration));
        assert!(q_dur.iter().all(|n| n.1 == 24));
        // A start moved past the old end: both modes give the minimum.
        let back = notes(&[(20, 2), (70, 1), (118, 3), (141, 2)]);
        for mode in [Mode::LeaveEnd, Mode::QuantizeEnd] {
            let got = run(back.clone(), &d, 96, |s| (s.start, s.dur, s.mode) = (1.0, 0.5, mode));
            assert_eq!(got, [(24, 4), (72, 4), (120, 4), (144, 4)]);
        }
    }

    /// §3: only notes nearer than Sensitivity move; at its least (0:06) most stay.
    #[test]
    fn sensitivity_matches_fl() {
        let got = run(notes(&QN), &Template::default().0, 96, |s| (s.start, s.sens) = (1.0, SENS_MIN));
        assert_eq!(got, [(0, 25), (30, 30), (48, 12), (72, 38), (96, 26), (130, 50), (200, 7), (250, 100), (301, 12), (347, 60), (395, 4), (420, 36)]);
        assert_eq!(Settings::default().sens_text(96), "0:18");
        assert_eq!(Settings::default().sens_ticks(192), 36.0);
    }

    /// §2: the period is the largest end rounded up to whole beats (250 → 288), repeated from tick 0.
    #[test]
    fn period_matches_fl() {
        let t = tpl(&[(0, 50), (100, 50), (200, 50)], 96);
        let got = run(notes(&[(285, 10), (380, 10), (475, 10), (570, 10)]), &t, 96, |s| (s.start, s.mode) = (1.0, Mode::LeaveDuration));
        assert_eq!(got, [(288, 10), (388, 10), (488, 10), (576, 10)]);
    }

    /// §3: on a tie the earlier template note wins.
    #[test]
    fn ties_go_to_the_earlier() {
        let got = run(notes(&[(0, 36), (96, 60), (200, 16), (300, 2), (420, 1)]), &Template::default().0, 96, |s| (s.start, s.dur) = (1.0, 1.0));
        assert_eq!(got, [(0, 24), (96, 48), (192, 24), (288, 24), (408, 24)]);
    }

    /// §2: a PPQ 768 template's halves round to the even tick; a length is the rounded end less the rounded start.
    #[test]
    fn halves_round_to_even() {
        let t = tpl(&[(12, 400), (780, 396), (1548, 404), (2316, 400)], 768);
        let starts = run(notes(&[(10, 10), (105, 10), (200, 10), (295, 10)]), &t, 96, |s| (s.start, s.mode) = (1.0, Mode::LeaveDuration));
        assert_eq!(starts, [(2, 10), (98, 10), (194, 10), (290, 10)]);
        let ends = run(notes(&[(40, 20), (100, 40), (230, 20), (330, 15)]), &t, 96, |s| (s.start, s.dur) = (0.0, 1.0));
        assert_eq!(ends, [(40, 12), (100, 47), (230, 14), (330, 10)]);
        let lens = run(notes(&[(5, 20), (100, 20), (196, 20), (292, 20)]), &t, 96, |s| (s.start, s.dur, s.mode) = (0.0, 1.0, Mode::QuantizeDuration));
        assert_eq!(lens, [(5, 50), (100, 49), (196, 50), (292, 50)]);
    }

    /// §3: two template notes on one start: the first in the file wins (its length, its levels).
    #[test]
    fn duplicate_starts_take_the_first() {
        let mut a = tpl(&[(0, 24), (0, 48), (96, 24), (192, 24), (192, 72)], 96);
        for (i, v) in [(0, 20), (1, 120), (3, 20), (4, 120)] {
            a.rows[i].vel = v;
        }
        let mut b = a.clone();
        b.rows.swap(0, 1);
        b.rows.swap(3, 4);
        let ns = notes(&[(5, 30), (100, 30), (190, 30), (380, 30)]);
        let go = |p: &Pattern| {
            let set = Settings { start: 1.0, dur: 1.0, mode: Mode::QuantizeDuration, ..Settings::default() };
            quantize(ns.clone(), p, &set, 96).iter().map(|n| (n.position, n.length, n.velocity)).collect::<Vec<_>>()
        };
        assert_eq!(go(&a), [(0, 24, 20), (96, 24, 100), (192, 24, 20), (384, 24, 100)]);
        assert_eq!(go(&b), [(0, 48, 120), (96, 24, 100), (192, 72, 120), (384, 24, 100)]);
    }

    /// §6: each level moves the knob's share toward the template note's, truncated; velocity scales by the row's / 100.
    #[test]
    fn levels_match_fl() {
        let mut t = tpl(&[(0, 96), (96, 96), (192, 96), (288, 96)], 96);
        // (pan, vel, rel, mod X, mod Y, fine)
        let lv = [(0, 20, 10, 0, 255, 0), (128, 127, 127, 255, 0, 240), (32, 60, 30, 64, 200, 60), (96, 90, 100, 192, 50, 180)];
        for (r, &(pan, vel, rel, mod_x, mod_y, fine)) in t.rows.iter_mut().zip(lv.iter()) {
            (r.pan, r.vel, r.rel, r.mod_x, r.mod_y, r.fine) = (pan, vel, rel, mod_x, mod_y, fine);
        }
        let ns: Vec<Note> = [5, 100, 150, 190, 300, 380]
            .iter()
            .map(|&p| Note { velocity: 80, pan: 80, release: 50, mod_x: 100, mod_y: 150, fine_pitch: 100, ..note(p, 20) })
            .collect();
        let set = Settings { start: 1.0, levels: [31.0 / 64.0; 6], ..Settings::default() };
        let got: Vec<_> = quantize(ns, &t, &set, 96).iter().map(|n| (n.position, n.velocity, n.pan, n.release, n.mod_x, n.mod_y, n.fine_pitch)).collect();
        let fl = [
            (0, 49, 49, 24, 38, 211, 42),
            (96, 90, 111, 80, 161, 88, 158),
            (150, 80, 80, 50, 100, 150, 100),
            (192, 65, 65, 34, 69, 184, 71),
            (288, 77, 95, 67, 131, 113, 129),
            (384, 49, 49, 24, 38, 211, 42),
        ];
        assert_eq!(got, fl);
        // The velocity target is truncated before the move: FL's 16_tight took velocity 90 with rows 33, 99 and 63 to
        // 29, 89 and 56.
        assert_eq!([33, 99, 63].map(|r| level(90, r, 1, 1.0)), [29, 89, 56]);
    }

    /// §2: the Snap entries are bars of notes on their grid, each a grid step long.
    #[test]
    fn snap_grids_match_fl() {
        let probes = [5, 13, 20, 29, 37, 50, 70, 100, 130, 200, 250, 301, 347, 395, 420];
        let go = |i: usize| {
            let set = Settings { start: 1.0, dur: 1.0, sens: SENS_MAX, mode: Mode::QuantizeDuration, ..Settings::default() };
            let ns = probes.iter().map(|&p| note(p, 10)).collect();
            quantize(ns, &Template::snap(i).0, &set, 96).iter().map(|n| (n.position, n.length)).collect::<Vec<_>>()
        };
        let starts = |i: usize| go(i).iter().map(|n| n.0).collect::<Vec<_>>();
        assert_eq!(starts(5), [0, 16, 16, 32, 32, 48, 64, 96, 128, 192, 256, 304, 352, 400, 416], "Sixth-beat");
        assert_eq!(starts(4), [6, 12, 18, 30, 36, 48, 72, 102, 132, 198, 252, 300, 348, 396, 420], "Quarter-step");
        assert_eq!(starts(7), [0, 0, 32, 32, 32, 64, 64, 96, 128, 192, 256, 288, 352, 384, 416], "Third-beat");
        // Bar: notes over a beat from a bar line stay where they were.
        assert_eq!(go(0)[6..10], [(0, 384), (100, 10), (130, 10), (200, 10)]);
        assert!(go(3).iter().all(|n| n.1 == 12));
    }

    /// §2: the menu ends with the Snap grids; a missing folder still gives the built-in Default.
    #[test]
    fn template_menu_without_fl() {
        let items = template_menu(&Template::default().0, Path::new("/nonexistent"));
        let labels: Vec<_> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Default", "Snap", "Bar", "Beat", "Half-beat", "Half-step", "Quarter-step", "Sixth-beat", "Step", "Third-beat"]);
        assert!(items[0].mark == Mark::Radio(true) && items[3].mark == Mark::Radio(false));
        let beat = template_menu(&Template::snap(1).0, Path::new("/nonexistent"));
        assert!(beat[3].mark == Mark::Radio(true) && beat[0].mark == Mark::Radio(false));
    }

    /// §1: the knobs' hints and Sensitivity's steps.
    #[test]
    fn knob_text() {
        let mut s = Settings::default();
        assert_eq!(Settings::pct_text(s.start), "50%");
        assert_eq!(Settings::pct_text(1.0 - LEVEL_NOTCH), "95%");
        s.set_sens_pos(1.0);
        assert_eq!((s.sens, s.sens_text(96)), (SENS_MAX, "4:00".to_string()));
        s.set_sens_pos(0.0);
        assert_eq!(s.sens_text(96), "0:06");
    }
}
