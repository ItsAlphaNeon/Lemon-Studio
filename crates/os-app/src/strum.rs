// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Strum tool (Tools ▸ Strum…, Alt+S): its settings, the strum itself (free of any window) and the
//! dialog's session, which previews every change on the notes and puts them back on ×. The window is `views/strum.rs`.
// spec: ui/piano-strum.md

use os_model::Note;

use crate::app::State;
use crate::piano;

/// The knob curve's base (§3): `g(x) = (B^|x| − 1) / (B − 1)`.
const B: f64 = 51.0;
/// Velocities the Velocity knob lowers stop here (§4).
const VEL_FLOOR: u8 = 8;

/// The dialog's controls (§1). Knobs are bipolar, −1..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub start: bool,
    pub time: f32,
    pub time_tension: f32,
    pub vel: f32,
    pub vel_tension: f32,
    pub preserve_end: bool,
    pub ahead: bool,
    pub end: bool,
    pub end_time: f32,
    pub end_tension: f32,
    pub chop: bool,
    pub alternate: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            start: true,
            time: 0.0,
            time_tension: 0.0,
            vel: 0.0,
            vel_tension: 0.0,
            preserve_end: false,
            ahead: false,
            end: true,
            end_time: 0.0,
            end_tension: 0.0,
            chop: false,
            alternate: false,
        }
    }
}

/// A knob of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Knob {
    Time,
    TimeTension,
    Vel,
    VelTension,
    EndTime,
    EndTension,
}

impl Knob {
    pub fn tension(self) -> bool {
        matches!(self, Knob::TimeTension | Knob::VelTension | Knob::EndTension)
    }

    /// Change of the 0..1 knob position per wheel notch: a strength's side is 32 notches, a tension's 16 (§1).
    pub fn notch(self) -> f32 {
        if self.tension() { 1.0 / 32.0 } else { 1.0 / 64.0 }
    }

    pub fn name(self) -> &'static str {
        match self {
            Knob::Time => "Start strength",
            Knob::TimeTension => "Start tension",
            Knob::Vel => "Velocity strength",
            Knob::VelTension => "Velocity tension",
            Knob::EndTime => "End strength",
            Knob::EndTension => "End tension",
        }
    }
}

/// A switch or button of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Start,
    PreserveEnd,
    Ahead,
    End,
    Chop,
    Alternate,
    Reset,
    Accept,
}

impl Settings {
    pub fn get(&self, k: Knob) -> f32 {
        match k {
            Knob::Time => self.time,
            Knob::TimeTension => self.time_tension,
            Knob::Vel => self.vel,
            Knob::VelTension => self.vel_tension,
            Knob::EndTime => self.end_time,
            Knob::EndTension => self.end_tension,
        }
    }

    pub fn set(&mut self, k: Knob, v: f32) {
        let v = v.clamp(-1.0, 1.0);
        match k {
            Knob::Time => self.time = v,
            Knob::TimeTension => self.time_tension = v,
            Knob::Vel => self.vel = v,
            Knob::VelTension => self.vel_tension = v,
            Knob::EndTime => self.end_time = v,
            Knob::EndTension => self.end_tension = v,
        }
    }

    /// The switch `a` (None for the buttons).
    pub fn switch(&mut self, a: Act) -> Option<&mut bool> {
        Some(match a {
            Act::Start => &mut self.start,
            Act::PreserveEnd => &mut self.preserve_end,
            Act::Ahead => &mut self.ahead,
            Act::End => &mut self.end,
            Act::Chop => &mut self.chop,
            Act::Alternate => &mut self.alternate,
            Act::Reset | Act::Accept => return None,
        })
    }

    /// The hint-bar text of a knob (§1): the time strengths as `step:tick` of the per-note delta at `ppq`, the velocity
    /// strength as a percentage, the tensions as one.
    pub fn text(&self, k: Knob, ppq: u32) -> String {
        let v = self.get(k);
        let time = |max: f64| {
            let d = (curve(v) * max).round() as u32;
            let step = (ppq / 4).max(1);
            format!("{}: {}:{:02}", k.name(), d / step, d % step)
        };
        match k {
            Knob::Time => time(f64::from(ppq) / 3.0),
            Knob::EndTime => time(f64::from(ppq) / 2.0),
            _ => format!("{}: {:.0}%", k.name(), v * 100.0),
        }
    }
}

/// The knob curve (§3), 0..1 for a knob at −1..1 (either side).
pub fn curve(x: f32) -> f64 {
    (B.powf(f64::from(x.abs())) - 1.0) / (B - 1.0)
}

/// Ratio between successive deltas for tension `t` (§3).
fn ratio(t: f32) -> f64 {
    let t = f64::from(t);
    if t >= 0.0 { 1.0 + t } else { 1.0 + t / 2.0 }
}

/// The k-th note's total move: `d (1 + r + … + r^(k−1))` (§3).
fn cum(d: f64, r: f64, k: usize) -> f64 {
    (0..k).map(|i| d * r.powi(i as i32)).sum()
}

/// Chop chords (§2): every note still sounding where another starts is cut there.
fn chop(notes: Vec<Note>) -> Vec<Note> {
    let mut starts: Vec<u32> = notes.iter().map(|n| n.position).collect();
    starts.sort_unstable();
    starts.dedup();
    let mut out = Vec::new();
    for n in notes {
        let end = n.position + n.length;
        let mut at = n.position;
        for &t in starts.iter().filter(|&&t| t > n.position && t < end) {
            out.push(Note { position: at, length: t - at, ..n });
            at = t;
        }
        out.push(Note { position: at, length: end.max(at) - at, ..n });
    }
    out
}

/// Strum `notes` (the targets) at `ppq` ticks per quarter (spec §2-§6).
pub fn strum(notes: Vec<Note>, set: &Settings, ppq: u32) -> Vec<Note> {
    let notes = if set.chop { chop(notes) } else { notes };
    let mut sorted = notes;
    sorted.sort_by_key(|n| (n.position, n.key));
    let mut groups: Vec<Vec<Note>> = Vec::new();
    for n in sorted {
        match groups.last_mut() {
            Some(g) if g[0].position == n.position => g.push(n),
            _ => groups.push(vec![n]),
        }
    }
    let ppq = f64::from(ppq);
    let (d, r) = (curve(set.time) * ppq / 3.0, ratio(set.time_tension));
    let (dv, rv) = (curve(set.vel) * 32.0, ratio(set.vel_tension));
    let (de, re) = (curve(set.end_time) * ppq / 2.0, ratio(set.end_tension));
    let mut flip = false;
    let mut out = Vec::new();
    for mut g in groups {
        if g.len() < 2 {
            out.extend(g);
            continue;
        }
        let n = g.len();
        // Start (§4): the strum order, flipped every second group by Alternate direction (§6).
        let up = (set.time >= 0.0) != (set.alternate && flip);
        flip = !flip;
        if !up {
            g.reverse();
        }
        if set.start {
            let back = if set.ahead { cum(d, r, n - 1) } else { 0.0 };
            for (k, note) in g.iter_mut().enumerate() {
                let off = (cum(d, r, k) - back).round() as i64;
                note.position = (i64::from(note.position) + off).max(0) as u32;
                if set.preserve_end && note.length > 0 {
                    note.length = (i64::from(note.length) - off).max(1) as u32;
                }
                let idx = if set.vel >= 0.0 { k } else { n - 1 - k };
                let drop = cum(dv, rv, idx).round() as i64;
                if drop > 0 {
                    let v = i64::from(note.velocity);
                    note.velocity = (v - drop).max(i64::from(VEL_FLOOR)).min(v) as u8;
                }
            }
        }
        // End (§5): by key, the top note's end fixed for a positive knob, the bottom one's for a negative one.
        if set.end && set.end_time != 0.0 {
            g.sort_by_key(|n| n.key);
            if set.end_time > 0.0 {
                g.reverse();
            }
            for (j, note) in g.iter_mut().enumerate() {
                if note.length > 0 {
                    note.length = (i64::from(note.length) - cum(de, re, j).round() as i64).max(1) as u32;
                }
            }
        }
        out.extend(g);
    }
    out
}

/// The window's place and the dialog's state: the settings shown, the ones last accepted (an opening starts from
/// them, §1) and the open session.
#[derive(Default)]
pub struct StrumUi {
    /// Top-left corner (logical px).
    pub pos: (f32, f32),
    pub placed: bool,
    pub set: Settings,
    pub kept: Settings,
    session: Option<Session>,
}

/// What an open dialog previews on: the pattern's notes and selection as they were, and what the last preview left.
struct Session {
    pattern: u16,
    channel: u16,
    base: Vec<Note>,
    base_sel: Vec<usize>,
    last: Vec<Note>,
}

/// Start a session on the piano roll's channel and preview the settings last accepted.
pub fn open(s: &mut State) {
    s.strum.set = s.strum.kept;
    s.strum.session = None;
    preview(s);
}

/// Put the notes back to the session's start and strum them with the current settings, without an undo step. When the
/// notes were changed by something else since the last preview (another pattern or channel, an edit, an undo), the
/// preview starts again from them.
pub fn preview(s: &mut State) {
    let Some(ch) = piano::channel(s) else { return };
    let now = piano::notes(s).to_vec();
    let fresh = match &s.strum.session {
        Some(se) => se.pattern != s.pattern || se.channel != ch || se.last != now,
        None => true,
    };
    if fresh {
        let base_sel = s.piano.selection(s.pattern, Some(ch));
        s.strum.session = Some(Session { pattern: s.pattern, channel: ch, base: now, base_sel, last: Vec::new() });
    }
    restore(s);
    let (set, ppq) = (s.strum.set, u32::from(s.project.ppq.max(4)));
    crate::piano_tools::apply_as(s, |_, ns| strum(ns, &set, ppq), None);
    let last = piano::notes(s).to_vec();
    if let Some(se) = &mut s.strum.session {
        se.last = last;
    }
}

/// The session's notes and selection back, when the notes are still what its last preview left.
fn restore(s: &mut State) -> bool {
    let Some(se) = &s.strum.session else { return false };
    if se.pattern != s.pattern || piano::notes(s) != se.last.as_slice() && !se.last.is_empty() {
        return false;
    }
    let (base, sel, ch) = (se.base.clone(), se.base_sel.clone(), se.channel);
    piano::pattern_mut(s).notes = base;
    if sel.is_empty() {
        s.piano.clear_selection();
    } else {
        s.piano.select(s.pattern, ch, sel);
    }
    s.piano.dirty = true;
    true
}

/// Accept (§1): the strum becomes one undo step, the settings are kept for the next opening. Returns the status text.
pub fn accept(s: &mut State) -> String {
    if !restore(s) {
        s.strum.session = None;
        return "Piano roll: nothing to strum".into();
    }
    s.strum.session = None;
    s.strum.kept = s.strum.set;
    let (set, ppq) = (s.strum.set, u32::from(s.project.ppq.max(4)));
    if crate::piano_tools::apply_as(s, |_, ns| strum(ns, &set, ppq), Some("Piano roll strum")) {
        "Piano roll: strummed".into()
    } else {
        "Piano roll: strummed: nothing to change".into()
    }
}

/// The × button (§1): the notes as they were before the dialog opened.
pub fn cancel(s: &mut State) {
    restore(s);
    s.strum.session = None;
}

/// A dialog control: a switch flips, Reset puts every control back to its default; the notes follow.
pub fn act(s: &mut State, a: Act) {
    match a {
        Act::Reset => s.strum.set = Settings::default(),
        Act::Accept => return,
        a => {
            if let Some(b) = s.strum.set.switch(a) {
                *b = !*b;
            }
        }
    }
    preview(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, len: u32, key: u16, vel: u8) -> Note {
        Note { length: len, key, velocity: vel, ..Note::step(0, pos) }
    }

    /// The first FL test project (PPQ 96): a 3-note chord, a 4-note chord, a single note, a chord of unequal lengths
    /// and a long note starting with a chord, then a chord under it.
    fn proj1() -> Vec<Note> {
        let mut v = vec![];
        for k in [60, 64, 67] {
            v.push(note(0, 384, k, 100));
        }
        for k in [62, 65, 69, 72] {
            v.push(note(384, 192, k, 100));
        }
        v.push(note(768, 96, 50, 100));
        v.extend([note(960, 96, 48, 100), note(960, 192, 55, 100), note(960, 192, 60, 100)]);
        v.extend([note(1152, 768, 36, 100), note(1152, 192, 60, 100), note(1152, 192, 64, 100)]);
        v.extend([note(1344, 192, 62, 100), note(1344, 192, 65, 100)]);
        v
    }

    fn by(ns: &[Note]) -> Vec<(u32, u32, u16, u8)> {
        let mut v: Vec<_> = ns.iter().map(|n| (n.position, n.length, n.key, n.velocity)).collect();
        v.sort_unstable();
        v
    }

    fn set(f: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = Settings::default();
        f(&mut s);
        s
    }

    /// Knob positions as FL's wheel notches (1/32 of a strength's side, 1/16 of a tension's).
    fn st(n: i32) -> f32 {
        n as f32 / 32.0
    }
    fn tn(n: i32) -> f32 {
        n as f32 / 16.0
    }

    /// The starts of the 4-note chord at 384 (spec §3's measured table).
    fn chord4(s: &Settings) -> Vec<u32> {
        let mut v: Vec<_> = strum(proj1(), s, 96).iter().filter(|n| n.position >= 384 && n.position < 768 && n.key != 50).map(|n| n.position).collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn time_curve_matches_fl() {
        for (n, want) in [(6, [384, 385, 385, 386]), (7, [384, 385, 386, 387]), (11, [384, 386, 388, 389]), (16, [384, 388, 392, 396]), (20, [384, 391, 398, 404]), (24, [384, 396, 407, 419]), (29, [384, 406, 428, 450]), (31, [384, 412, 440, 469])] {
            assert_eq!(chord4(&set(|s| s.time = st(n))), want, "Start time at {n} notches");
        }
        assert_eq!(by(&strum(proj1(), &set(|s| s.time = 1.0), 96))[1].0, 32, "full right: 32 ticks per note at PPQ 96");
    }

    #[test]
    fn tension_matches_fl() {
        for (t, want) in [(4, [384, 406, 433, 468]), (8, [384, 406, 439, 488]), (12, [384, 406, 444, 512]), (16, [384, 406, 450, 538]), (-4, [384, 406, 425, 442]), (-8, [384, 406, 422, 435]), (-12, [384, 406, 420, 428]), (-16, [384, 406, 417, 422])] {
            assert_eq!(chord4(&set(|s| (s.time, s.time_tension) = (st(29), tn(t)))), want, "tension {t}");
        }
    }

    #[test]
    fn negative_time_strums_down_and_preserve_end_keeps_ends() {
        let out = by(&strum(proj1(), &set(|s| s.time = st(-29)), 96));
        assert!(out.contains(&(0, 384, 67, 100)) && out.contains(&(44, 384, 60, 100)), "high to low");
        assert!(out.contains(&(1196, 768, 36, 100)), "the long note is the chord's lowest");
        let out = by(&strum(proj1(), &set(|s| (s.time, s.preserve_end) = (st(29), true)), 96));
        assert!(out.contains(&(22, 362, 64, 100)) && out.contains(&(450, 126, 72, 100)) && out.contains(&(768, 96, 50, 100)));
    }

    #[test]
    fn velocity_matches_fl() {
        let vels = |s: &Settings| strum(proj1(), s, 96).iter().filter(|n| n.position == 384).map(|n| n.velocity).collect::<Vec<_>>();
        assert_eq!(vels(&set(|s| s.vel = st(8))), [100, 99, 98, 97]);
        assert_eq!(vels(&set(|s| s.vel = st(24))), [100, 88, 77, 65]);
        assert_eq!(vels(&set(|s| s.vel = st(32))), [100, 68, 36, 8]);
        assert_eq!(vels(&set(|s| s.vel = st(-16))), [88, 92, 96, 100]);
        assert_eq!(vels(&set(|s| (s.vel, s.vel_tension) = (st(16), tn(8)))), [100, 96, 90, 81]);
        assert_eq!(vels(&set(|s| (s.vel, s.vel_tension) = (st(16), tn(-8)))), [100, 96, 93, 91]);
        // Each note's own velocity (FL: 40 / 120 / 80 at +16 -> 40, 116, 72; at -32 -> 8, 88, 80).
        let mixed = vec![note(22, 192, 60, 40), note(22, 192, 64, 120), note(22, 192, 67, 80)];
        assert_eq!(by(&strum(mixed.clone(), &set(|s| s.vel = st(16)), 96)).iter().map(|n| n.3).collect::<Vec<_>>(), [40, 116, 72]);
        assert_eq!(by(&strum(mixed, &set(|s| s.vel = st(-32)), 96)).iter().map(|n| n.3).collect::<Vec<_>>(), [8, 88, 80]);
        // Velocity follows the strum order (FL: Start -29 with Velocity +16 -> the top note keeps 100).
        let out = by(&strum(proj1(), &set(|s| (s.time, s.vel) = (st(-29), st(16))), 96));
        assert!(out.contains(&(0, 384, 67, 100)) && out.contains(&(44, 384, 60, 92)));
    }

    #[test]
    fn trigger_ahead_ends_on_the_start() {
        let s2 = vec![note(22, 192, 60, 40), note(22, 192, 64, 120), note(22, 192, 67, 80), note(384, 96, 48, 100), note(384, 96, 52, 100), note(384, 96, 55, 100), note(384, 96, 59, 100), note(384, 96, 62, 100)];
        let out = by(&strum(s2.clone(), &set(|s| (s.time, s.ahead) = (st(29), true)), 96));
        assert_eq!(out.iter().map(|n| n.0).collect::<Vec<_>>(), [0, 0, 22, 296, 318, 340, 362, 384]);
        let out = by(&strum(s2, &set(|s| (s.time, s.ahead, s.preserve_end) = (st(29), true, true)), 96));
        assert!(out.contains(&(0, 236, 60, 40)) && out.contains(&(0, 214, 64, 120)) && out.contains(&(22, 192, 67, 80)), "lengths from the unclamped moves");
        assert_eq!(out[3].1, 184);
    }

    #[test]
    fn end_section_matches_fl() {
        let lens = |s: &Settings| {
            let mut v: Vec<_> = strum(proj1(), s, 96).iter().filter(|n| n.position >= 384 && n.position < 768).map(|n| (n.key, n.length)).collect();
            v.sort_unstable();
            v.into_iter().map(|(_, l)| l).collect::<Vec<_>>()
        };
        assert_eq!(lens(&set(|s| s.end_time = st(16))), [174, 180, 186, 192]);
        assert_eq!(lens(&set(|s| s.end_time = st(29))), [93, 126, 159, 192]);
        assert_eq!(lens(&set(|s| s.end_time = st(-29))), [192, 159, 126, 93]);
        assert_eq!(lens(&set(|s| (s.end_time, s.end_tension) = (st(29), tn(8)))), [36, 110, 159, 192]);
        assert_eq!(lens(&set(|s| (s.end_time, s.end_tension) = (st(29), tn(-8)))), [116, 134, 159, 192]);
        assert_eq!(lens(&set(|s| (s.end_time, s.end) = (st(29), false))), [192, 192, 192, 192], "the LED off");
        let five: Vec<_> = [48, 52, 55, 59, 62].map(|k| note(384, 96, k, 100)).into();
        let out = by(&strum(five, &set(|s| (s.end_time, s.end_tension) = (1.0, 1.0)), 96));
        assert_eq!(out.iter().map(|n| n.1).collect::<Vec<_>>(), [1, 1, 1, 48, 96]);
        // After Start and Preserve end (FL: 318, 329, 340 for the 3-note chord).
        let out = by(&strum(proj1(), &set(|s| (s.time, s.preserve_end, s.end_time) = (st(29), true, st(29))), 96));
        assert_eq!(out[..3].iter().map(|n| n.1).collect::<Vec<_>>(), [318, 329, 340]);
    }

    #[test]
    fn alternate_and_chop_match_fl() {
        let out = strum(proj1(), &set(|s| (s.time, s.alternate) = (st(29), true)), 96);
        let first = |at: u32| out.iter().find(|n| n.position == at).map(|n| n.key);
        assert_eq!([first(0), first(384), first(960), first(1152), first(1344)], [Some(60), Some(72), Some(48), Some(64), Some(62)], "up, down, (single skipped), up, down, up");
        // PPQ 192: a long note under a later chord, a single note under a held chord.
        let p3 = vec![note(0, 1536, 36, 100), note(384, 384, 60, 100), note(384, 384, 64, 100), note(384, 384, 67, 100), note(1920, 768, 48, 100), note(1920, 768, 52, 100), note(2112, 192, 60, 100), note(3072, 384, 50, 100), note(3072, 384, 53, 100)];
        let out = by(&strum(p3.clone(), &set(|s| s.time = st(29)), 192));
        assert!(out.contains(&(0, 1536, 36, 100)) && out.contains(&(472, 384, 67, 100)), "44 ticks per note at PPQ 192");
        let out = by(&strum(p3.clone(), &set(|s| (s.time, s.chop) = (st(29), true)), 192));
        let want = [(0, 384, 36), (384, 1152, 36), (428, 384, 60), (472, 384, 64), (516, 384, 67), (1920, 192, 48), (1964, 192, 52), (2112, 576, 48), (2156, 576, 52), (2200, 192, 60), (3072, 384, 50), (3116, 384, 53)];
        assert_eq!(out.iter().map(|n| (n.0, n.1, n.2)).collect::<Vec<_>>(), want);
        let out = strum(p3, &set(|s| (s.time, s.chop, s.alternate) = (st(29), true, true)), 192);
        let first = |at: u32| out.iter().find(|n| n.position == at).map(|n| n.key);
        assert_eq!([first(384), first(1920), first(2112), first(3072)], [Some(36), Some(52), Some(48), Some(53)]);
    }

    #[test]
    fn start_led_off_and_texts() {
        assert_eq!(strum(proj1(), &set(|s| (s.time, s.vel, s.start) = (st(29), 1.0, false)), 96), proj1());
        let s = set(|s| (s.time, s.vel, s.end_time) = (1.0, -0.5, 1.0));
        assert_eq!(s.text(Knob::Time, 96), "Start strength: 1:08");
        assert_eq!(s.text(Knob::EndTime, 96), "End strength: 2:00");
        assert_eq!(s.text(Knob::Vel, 96), "Velocity strength: -50%");
        assert_eq!(set(|s| s.time = st(29)).text(Knob::Time, 96), "Start strength: 0:22");
    }
}
