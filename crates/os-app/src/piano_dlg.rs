// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Flam (Alt+F), Flip (Alt+Y), Scale levels (Alt+X) and Limit (Alt+K) dialogs: one tool window shows
//! whichever is open. As with Strum, its session previews every change on the notes, Accept keeps the result as one
//! undo step and × puts the notes back. The tools are `flam.rs`, `flip.rs`, `levels.rs` and `limit.rs`; the window is
//! `views/piano_dlg.rs`.
// spec: ui/piano-flam.md, ui/piano-flip.md, ui/piano-level-scaling.md, ui/piano-limit.md

use os_model::Note;

use crate::app::State;
use crate::piano::{self, Lane};
use crate::playlist::{Cmd, Mark, PItem};
use crate::{chords, flam, flip, levels, limit};

/// Which dialog the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Flam,
    Flip,
    Levels,
    Limit,
}

impl Tool {
    pub fn title(self) -> &'static str {
        match self {
            Tool::Flam => "Piano roll - flamer",
            Tool::Flip => "Piano roll - score flipper",
            Tool::Levels => "Piano roll - level scaling",
            Tool::Limit => "Piano roll - limiter / transposer",
        }
    }

    /// The undo step an Accept makes.
    fn step(self) -> &'static str {
        match self {
            Tool::Flam => "Piano roll flam",
            Tool::Flip => "Piano roll flip",
            Tool::Levels => "Piano roll level scaling",
            Tool::Limit => "Piano roll limit",
        }
    }

    fn done(self) -> &'static str {
        match self {
            Tool::Flam => "flammed",
            Tool::Flip => "flipped",
            Tool::Levels => "levels scaled",
            Tool::Limit => "limited",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// A knob or fader of the dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Knob {
    FlamTime,
    FlamVel,
    Center,
    Mult,
    Offset,
    Tension,
}

impl Knob {
    /// Change of the 0..1 position per wheel notch (piano-flam.md §1, piano-level-scaling.md §1).
    pub fn notch(self) -> f32 {
        match self {
            Knob::FlamTime => flam::TIME_NOTCH,
            Knob::FlamVel => flam::VEL_NOTCH,
            Knob::Center => 1.0 / 32.0,
            Knob::Mult | Knob::Offset | Knob::Tension => 1.0 / 64.0,
        }
    }

    /// Track length in px of the faders (`views/piano_dlg.rs`): a drag moves the handle with the mouse.
    pub fn travel(self) -> Option<f32> {
        match self {
            Knob::Center => Some(72.0),
            Knob::Mult | Knob::Offset => Some(144.0),
            _ => None,
        }
    }

    /// Position after Reset; Center's is on the velocity lane.
    pub fn default_pos(self) -> f32 {
        let (f, l) = (flam::Settings::default(), levels::Settings::default());
        Ui::pos_of(self, &f, &l)
    }
}

/// A switch or button of the dialogs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// Flam's Options ▸ menu of presets.
    Options,
    Absolute,
    Before,
    Group,
    Horizontal,
    Preserve,
    Vertical,
    /// Limit's Key, scale and Snap ▸ menus.
    KeyMenu,
    ScaleMenu,
    SnapMenu,
    Wrap,
    /// Limit's section header arrows: 0 = Key / scale restriction, 1 = Keyboard range.
    Collapse(u8),
    /// A press on Limit's keys (the green key follows), its name strip (the nearer range end follows) or its scroll
    /// bar's handle.
    Keys,
    Ruler,
    Scroll,
    Reset,
    Accept,
}

/// A pick from one of Limit's menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LCmd {
    Key(u8),
    Scale(usize),
    Snap(limit::Snap),
}

/// The window's place for each dialog and the dialogs' state: the settings shown, the ones last accepted (an opening
/// starts from them) and the open session.
#[derive(Default)]
pub struct Ui {
    pub tool: Tool,
    /// Top-left corner (logical px) of each dialog, by `Tool::index`.
    pub pos: [(f32, f32); 4],
    pub placed: [bool; 4],
    pub flam: flam::Settings,
    pub flip: flip::Settings,
    pub levels: levels::Settings,
    pub limit: limit::Settings,
    /// Limit's collapsed sections (`Act::Collapse`); a view setting, kept as it is.
    pub collapsed: [bool; 2],
    /// Limit's keyboard view: px from C0's left edge to the view's.
    pub kb_scroll: f32,
    kept_flam: flam::Settings,
    kept_flip: flip::Settings,
    kept_levels: levels::Settings,
    kept_limit: limit::Settings,
    /// The Control lane Scale levels was last accepted on (its Center follows a new lane).
    kept_lane: Option<Lane>,
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

fn pct(v: f32) -> i64 {
    (f64::from(v) * 100.0).round() as i64
}

impl Ui {
    fn pos_of(k: Knob, f: &flam::Settings, l: &levels::Settings) -> f32 {
        match k {
            Knob::FlamTime => f.time,
            Knob::FlamVel => f.vel,
            Knob::Center => l.center,
            Knob::Mult => (l.mult + 2.0) / 4.0,
            Knob::Offset => (l.offset + 1.0) / 2.0,
            Knob::Tension => (l.tension + 1.0) / 2.0,
        }
    }

    /// The 0..1 position of knob `k`.
    pub fn get(&self, k: Knob) -> f32 {
        Self::pos_of(k, &self.flam, &self.levels)
    }

    pub fn set(&mut self, k: Knob, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match k {
            Knob::FlamTime => self.flam.time = v,
            Knob::FlamVel => self.flam.vel = v,
            Knob::Center => self.levels.center = v,
            Knob::Mult => self.levels.mult = v * 4.0 - 2.0,
            Knob::Offset => self.levels.offset = v * 2.0 - 1.0,
            Knob::Tension => self.levels.tension = v * 2.0 - 1.0,
        }
    }

    /// The hint-bar text of a knob (piano-flam.md §1, piano-level-scaling.md §1).
    pub fn text(&self, k: Knob, ppq: u32) -> String {
        let l = &self.levels;
        match k {
            Knob::FlamTime => self.flam.time_text(ppq),
            Knob::FlamVel => self.flam.vel_text(),
            Knob::Center => format!("Center: {}%", pct(l.center)),
            Knob::Mult => format!("Multiplier: {}%", pct(l.mult)),
            Knob::Offset => format!("Offset: {}%", pct(l.offset)),
            Knob::Tension => format!("Tension: {}%", pct(l.tension)),
        }
    }

    /// Whether `a` is a switch that is on.
    pub fn on(&self, a: Act) -> bool {
        match a {
            Act::Absolute => self.flam.absolute,
            Act::Before => self.flam.before,
            Act::Group => self.flam.group,
            Act::Horizontal => self.flip.horizontal,
            Act::Preserve => self.flip.preserve,
            Act::Vertical => self.flip.vertical,
            Act::Wrap => self.limit.wrap,
            _ => false,
        }
    }
}

/// Options ▸ (piano-flam.md §5).
pub fn options_menu() -> Vec<PItem> {
    flam::Preset::ALL.iter().map(|&p| PItem::new(p.name(), Cmd::FlamPreset(p))).collect()
}

/// Start `tool`'s session on the piano roll's channel and preview the settings last accepted; another dialog's
/// preview is put back first. Scale levels' Center takes the lane's Reset value when the Control lane changed since
/// its last Accept (piano-level-scaling.md §4); Limit's range is the targets' own until one is kept (piano-limit.md
/// §5).
pub fn open(s: &mut State, tool: Tool) {
    cancel(s);
    let span = limit::span(&target_notes(s));
    let d = &mut s.piano_dlg;
    d.tool = tool;
    match tool {
        Tool::Flam => d.flam = d.kept_flam,
        Tool::Flip => d.flip = d.kept_flip,
        Tool::Levels => {
            d.levels = d.kept_levels;
            if d.kept_lane != Some(s.piano.lane) {
                d.levels.center = levels::reset_center(s.piano.lane);
            }
        }
        Tool::Limit => {
            d.limit = d.kept_limit;
            if d.limit.range.is_none() {
                d.limit.range = span;
            }
            show_range(s);
        }
    }
    preview(s);
}

/// The notes a tool would change now (`piano::targets`).
fn target_notes(s: &State) -> Vec<Note> {
    let all = piano::notes(s);
    piano::targets(s).iter().filter_map(|&i| all.get(i).copied()).collect()
}

/// The targets as the open session found them: its selection, or every note of its channel.
fn base_targets(s: &State) -> Vec<Note> {
    match &s.piano_dlg.session {
        Some(se) if !se.base_sel.is_empty() => se.base_sel.iter().filter_map(|&i| se.base.get(i).copied()).collect(),
        Some(se) => se.base.iter().filter(|n| n.channel == se.channel).copied().collect(),
        None => target_notes(s),
    }
}

/// Scroll Limit's keyboard so its range shows, centred, when it would not (a Lemon Studio choice, piano-limit.md §5).
fn show_range(s: &mut State) {
    let d = &mut s.piano_dlg;
    let Some((lo, hi)) = d.limit.range else { return };
    let (a, b) = (white_x(i32::from(lo)), white_x(i32::from(hi)) + KEY_W);
    if a < d.kb_scroll || b > d.kb_scroll + KB_W {
        d.kb_scroll = ((a + b - KB_W) / 2.0).round();
    }
    d.kb_scroll = d.kb_scroll.clamp(0.0, kb_max());
}

/// Limit's keyboard (piano-limit.md §1): white keys 13 px wide, a view 416 px wide over C0…B10.
pub const KEY_W: f32 = 13.0;
pub const KB_W: f32 = 416.0;
/// The view FL opens on: C5's left edge 202 px into it.
pub const KB_HOME: f32 = 35.0 * KEY_W - 202.0;

/// Where white key `k`'s left edge is (for a black key: the white key under it), px from C0's.
pub fn white_x(k: i32) -> f32 {
    const WHITE: [i32; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
    (7 * k.div_euclid(12) + WHITE[k.rem_euclid(12) as usize]) as f32 * KEY_W
}

/// The largest keyboard scroll.
pub fn kb_max() -> f32 {
    white_x(crate::views::piano::TOP_KEY) + KEY_W - KB_W
}

/// Apply the open dialog to the targets; `step` names the undo step (none for a preview).
fn run(s: &mut State, step: Option<&'static str>) -> bool {
    let d = &s.piano_dlg;
    match d.tool {
        Tool::Flam => {
            let set = d.flam;
            let off = set.offset(u32::from(s.project.ppq.max(4)), s.tempo);
            let mut used: Vec<u16> = piano::notes(s).iter().map(|n| n.unk14).filter(|&g| g != 0).collect();
            used.sort_unstable();
            used.dedup();
            crate::piano_tools::apply_as(s, |_, ns| flam::flam(ns, &set, off, &used), step)
        }
        Tool::Flip => {
            let set = d.flip;
            crate::piano_tools::apply_as(s, |_, ns| flip::flip(ns, &set), step)
        }
        Tool::Levels => {
            let (set, lane) = (d.levels, s.piano.lane);
            crate::piano_tools::apply_as(s, |_, ns| levels::scale(ns, lane, &set), step)
        }
        Tool::Limit => {
            let set = d.limit;
            crate::piano_tools::apply_as(
                s,
                |_, ns| {
                    let range = set.range.or_else(|| limit::span(&ns)).unwrap_or((0, 131));
                    limit::limit(ns, &set, range)
                },
                step,
            )
        }
    }
}

/// Put the notes back to the session's start and apply the dialog to them, without an undo step. Notes changed by
/// something else since the last preview (another pattern or channel, an edit, an undo) start the session again from
/// them.
pub fn preview(s: &mut State) {
    let Some(ch) = piano::channel(s) else { return };
    let now = piano::notes(s).to_vec();
    let fresh = match &s.piano_dlg.session {
        Some(se) => se.pattern != s.pattern || se.channel != ch || se.last != now,
        None => true,
    };
    if fresh {
        let base_sel = s.piano.selection(s.pattern, Some(ch));
        s.piano_dlg.session = Some(Session { pattern: s.pattern, channel: ch, base: now, base_sel, last: Vec::new() });
    }
    restore(s);
    run(s, None);
    let last = piano::notes(s).to_vec();
    if let Some(se) = &mut s.piano_dlg.session {
        se.last = last;
    }
}

/// The session's notes and selection back, when the notes are still what its last preview left.
fn restore(s: &mut State) -> bool {
    let Some(se) = &s.piano_dlg.session else { return false };
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

/// Accept: the result becomes one undo step, the settings are kept for the next opening. Returns the status text.
pub fn accept(s: &mut State) -> String {
    let tool = s.piano_dlg.tool;
    if !restore(s) {
        s.piano_dlg.session = None;
        return "Piano roll: nothing to change".into();
    }
    let d = &mut s.piano_dlg;
    d.session = None;
    match tool {
        Tool::Flam => d.kept_flam = d.flam,
        Tool::Flip => d.kept_flip = d.flip,
        Tool::Levels => (d.kept_levels, d.kept_lane) = (d.levels, Some(s.piano.lane)),
        Tool::Limit => d.kept_limit = d.limit,
    }
    if run(s, Some(tool.step())) {
        format!("Piano roll: {}", tool.done())
    } else {
        format!("Piano roll: {}: nothing to change", tool.done())
    }
}

/// The × button: the notes as they were before the dialog opened.
pub fn cancel(s: &mut State) {
    restore(s);
    s.piano_dlg.session = None;
}

/// A dialog control: a switch flips (Preserve start times only while Flip horizontally is on), Reset puts the open
/// dialog's controls back to their defaults; the notes follow.
pub fn act(s: &mut State, a: Act) {
    let span = limit::span(&base_targets(s));
    let (d, lane) = (&mut s.piano_dlg, s.piano.lane);
    match a {
        Act::Reset => match d.tool {
            Tool::Flam => d.flam = flam::Settings::default(),
            Tool::Flip => d.flip = flip::Settings::default(),
            Tool::Levels => d.levels = levels::Settings::reset(lane),
            Tool::Limit => {
                d.limit = limit::Settings { range: span, ..limit::Settings::default() };
                show_range(s);
            }
        },
        Act::Absolute => d.flam.absolute = !d.flam.absolute,
        Act::Before => d.flam.before = !d.flam.before,
        Act::Group => d.flam.group = !d.flam.group,
        Act::Horizontal => d.flip.horizontal = !d.flip.horizontal,
        Act::Preserve if d.flip.horizontal => d.flip.preserve = !d.flip.preserve,
        Act::Vertical => d.flip.vertical = !d.flip.vertical,
        Act::Wrap => d.limit.wrap = !d.limit.wrap,
        Act::Collapse(i) => {
            d.collapsed[usize::from(i & 1)] ^= true;
            return;
        }
        _ => return,
    }
    preview(s);
}

/// Limit's Key ▸ menu (piano-limit.md §1).
pub fn key_menu(set: &limit::Settings) -> Vec<PItem> {
    limit::KEYS.iter().enumerate().map(|(i, &n)| PItem::new(n, Cmd::Limit(LCmd::Key(i as u8))).mark(Mark::Radio(usize::from(set.key) == i))).collect()
}

/// Limit's scale ▸ menu: "(none)", then the chords file in two columns with its headings and separators (§1).
pub fn scale_menu(set: &limit::Settings) -> Vec<PItem> {
    let pick = |k: usize, label: &'static str| PItem::new(label, Cmd::Limit(LCmd::Scale(k))).mark(Mark::Radio(set.scale == k));
    let mut items = vec![pick(0, "(none)"), PItem::head("Chords")];
    for k in 1..=chords::ENTRIES.len() {
        if chords::SEPARATED.contains(&k) {
            items.push(PItem::sep());
        }
        if k == chords::FIRST_SCALE {
            items.push(PItem::head("Scales"));
        }
        items.push(pick(k, chords::label(k)));
        if k == 50 {
            items.last_mut().expect("just pushed").col = true;
        }
    }
    items
}

/// Limit's Snap ▸ menu.
pub fn snap_menu(set: &limit::Settings) -> Vec<PItem> {
    limit::Snap::ALL.iter().map(|&m| PItem::new(m.name(), Cmd::Limit(LCmd::Snap(m))).mark(Mark::Radio(set.snap == m))).collect()
}

/// A pick from Limit's menus: it previews at once.
pub fn limit_cmd(s: &mut State, c: LCmd) {
    let set = &mut s.piano_dlg.limit;
    match c {
        LCmd::Key(k) => set.key = k % 12,
        LCmd::Scale(k) => set.scale = k.min(chords::ENTRIES.len()),
        LCmd::Snap(m) => set.snap = m,
    }
    preview(s);
}

/// Limit's keys: key `k` becomes the green key, the transposition `60 − k` (§1 "Mouse on the keyboard").
pub fn set_green(s: &mut State, k: i32) {
    let t = 60 - k.clamp(0, crate::views::piano::TOP_KEY);
    if s.piano_dlg.limit.transpose != t {
        s.piano_dlg.limit.transpose = t;
        preview(s);
    }
}

/// Which range end a press on the name strip at key `k` takes: the nearer one (true = the highest key); the lower
/// one when both are as near.
pub fn nearer_end(s: &State, k: i32) -> bool {
    let (lo, hi) = s.piano_dlg.limit.shown_range();
    (k - i32::from(lo)).abs() > (k - i32::from(hi)).abs()
}

/// Move a range end to key `k`; it stops at the other end (a Lemon Studio choice, piano-limit.md §5).
pub fn set_end(s: &mut State, high: bool, k: i32) {
    let (lo, hi) = s.piano_dlg.limit.shown_range();
    let k = k.clamp(0, crate::views::piano::TOP_KEY) as u16;
    let range = if high { (lo, k.max(lo)) } else { (k.min(hi), hi) };
    if s.piano_dlg.limit.range != Some(range) {
        s.piano_dlg.limit.range = Some(range);
        preview(s);
    }
}

/// An Options ▸ preset picked: it previews at once (piano-flam.md §5).
pub fn preset(s: &mut State, p: flam::Preset) {
    s.piano_dlg.flam.preset(p);
    preview(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knob_positions_round_trip() {
        let mut d = Ui::default();
        assert_eq!([Knob::FlamTime, Knob::FlamVel, Knob::Center, Knob::Mult, Knob::Offset, Knob::Tension].map(|k| d.get(k)), [1.0 / 3.0, 85.0 / 128.0, 0.0, 0.75, 0.5, 0.5]);
        d.set(Knob::Mult, 0.75 - 5.0 / 64.0);
        assert_eq!(d.levels.mult, 0.6875);
        assert_eq!(d.text(Knob::Mult, 96), "Multiplier: 69%");
        d.set(Knob::Offset, 0.0);
        assert_eq!(d.text(Knob::Offset, 96), "Offset: -100%");
        d.set(Knob::Tension, 0.5 + 5.0 / 64.0);
        assert_eq!(d.levels.tension, 0.15625);
        assert_eq!(d.text(Knob::Center, 96), "Center: 0%");
        assert_eq!(Knob::Center.default_pos(), 0.0);
        assert!(d.on(Act::Horizontal) && d.on(Act::Preserve) && !d.on(Act::Vertical) && d.on(Act::Absolute) && d.on(Act::Group));
    }
}
