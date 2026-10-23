// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Flam (Alt+F), Flip (Alt+Y) and Scale levels (Alt+X) dialogs: one small tool window shows whichever
//! is open. As with Strum, its session previews every change on the notes, Accept keeps the result as one undo step and
//! × puts the notes back. The tools are `flam.rs`, `flip.rs` and `levels.rs`; the window is `views/piano_dlg.rs`.
// spec: ui/piano-flam.md, ui/piano-flip.md, ui/piano-level-scaling.md

use os_model::Note;

use crate::app::State;
use crate::piano::{self, Lane};
use crate::playlist::{Cmd, PItem};
use crate::{flam, flip, levels};

/// Which dialog the window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    #[default]
    Flam,
    Flip,
    Levels,
}

impl Tool {
    pub fn title(self) -> &'static str {
        match self {
            Tool::Flam => "Piano roll - flamer",
            Tool::Flip => "Piano roll - score flipper",
            Tool::Levels => "Piano roll - level scaling",
        }
    }

    /// The undo step an Accept makes.
    fn step(self) -> &'static str {
        match self {
            Tool::Flam => "Piano roll flam",
            Tool::Flip => "Piano roll flip",
            Tool::Levels => "Piano roll level scaling",
        }
    }

    fn done(self) -> &'static str {
        match self {
            Tool::Flam => "flammed",
            Tool::Flip => "flipped",
            Tool::Levels => "levels scaled",
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
    Reset,
    Accept,
}

/// The window's place for each dialog and the dialogs' state: the settings shown, the ones last accepted (an opening
/// starts from them) and the open session.
#[derive(Default)]
pub struct Ui {
    pub tool: Tool,
    /// Top-left corner (logical px) of each dialog, by `Tool::index`.
    pub pos: [(f32, f32); 3],
    pub placed: [bool; 3],
    pub flam: flam::Settings,
    pub flip: flip::Settings,
    pub levels: levels::Settings,
    kept_flam: flam::Settings,
    kept_flip: flip::Settings,
    kept_levels: levels::Settings,
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
            Act::Options | Act::Reset | Act::Accept => false,
        }
    }
}

/// Options ▸ (piano-flam.md §5).
pub fn options_menu() -> Vec<PItem> {
    flam::Preset::ALL.iter().map(|&p| PItem::new(p.name(), Cmd::FlamPreset(p))).collect()
}

/// Start `tool`'s session on the piano roll's channel and preview the settings last accepted; another dialog's
/// preview is put back first. Scale levels' Center takes the lane's Reset value when the Control lane changed since
/// its last Accept (piano-level-scaling.md §4).
pub fn open(s: &mut State, tool: Tool) {
    cancel(s);
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
    }
    preview(s);
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
    let (d, lane) = (&mut s.piano_dlg, s.piano.lane);
    match a {
        Act::Reset => match d.tool {
            Tool::Flam => d.flam = flam::Settings::default(),
            Tool::Flip => d.flip = flip::Settings::default(),
            Tool::Levels => d.levels = levels::Settings::reset(lane),
        },
        Act::Absolute => d.flam.absolute = !d.flam.absolute,
        Act::Before => d.flam.before = !d.flam.before,
        Act::Group => d.flam.group = !d.flam.group,
        Act::Horizontal => d.flip.horizontal = !d.flip.horizontal,
        Act::Preserve if d.flip.horizontal => d.flip.preserve = !d.flip.preserve,
        Act::Vertical => d.flip.vertical = !d.flip.vertical,
        Act::Preserve | Act::Options | Act::Accept => return,
    }
    preview(s);
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
