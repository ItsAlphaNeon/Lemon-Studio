// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project's undo history: one list of named steps for the whole project, oldest first, after "Last reset" (the
//! project as opened or made). An edit records a step holding what it is about to change (`record`); undo and redo
//! swap that with the project's, so a step always holds the other side of itself: the state before it while it is
//! done, the state after it while it is undone. Covered: a pattern's notes, control points and length (the piano roll
//! and the step sequencer), the playlist (`playlist::snapshot`), knob turns, sample loads (`load_sample`), the
//! project's initial values (`crate::init`), and with the playlist, PATTERNS > Split by channel's patterns and ADD's
//! clip channels (`playlist::snapshot_with`).
//!
//! The settings of General ▸ Undo history (`crate::prefs`): Maximum undo levels (past it the oldest step goes), Undo
//! knob tweaks, Alternate undo mode (what Ctrl+Z and Ctrl+Alt+Z do), Show undo history in the browser when undoing
//! changes and Show recent changes at the top (the browser's History folder, `views::browser`).
// spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)"

use os_model::{ControlPoint, Note};

use crate::app::State;
use crate::knobs::KnobId;
use crate::prefs;

/// The history's first entry: the project as opened or reset.
pub const LAST_RESET: &str = "Last reset";
/// A step-sequencer step click (FL's name).
pub const STEP_SEQ_EDIT: &str = "Step seq edit";
/// A sample loaded into a channel (FL's name; spec: ui/settings-choices.md "Undo history: the four checks").
pub const LOAD_SAMPLE: &str = "Load channel sample(s)";
/// "Create automation clip" (FL's name), with the control's initial value while "Initialize controls automatically" is
/// on (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
pub const CREATE_AUTOMATION: &str = "Create automation clip";
/// A chord written by step edit (spec: ui/step-edit.md).
pub const STEP_EDIT: &str = "Step edit";

/// What a step changes, as it is on the other side of the step.
#[derive(Debug, Clone, PartialEq)]
pub enum Snap {
    /// Pattern `number`'s notes, control points and length (the piano roll, the step sequencer).
    Pattern { number: u16, notes: Vec<Note>, controls: Vec<ControlPoint>, length: Option<u32> },
    /// The playlist (`playlist::capture`).
    Playlist(Box<crate::playlist::Snapshot>),
    /// A knob's value: the model's own for the channel knobs, else the knob's 0..1.
    Knob(KnobId, f64),
    /// What a sample load changes on channel `row`: its sample, its name ("Auto name channels"), its root note and its
    /// stretch ("Read sample root note" and "Read sample tempo information", `rack::read_tags`) and its sampler flags
    /// (Keep on disk, "Auto keep long audio on disk").
    Sample { row: usize, path: Option<String>, name: Option<String>, root: u8, stretch: [u32; 4], flags: Option<u32> },
    /// Channel `row` as a sample dropped below the rack's rows made it (`App::sample_to_rack`); `None` while it
    /// doesn't exist. Only the last row comes and goes.
    Channel { row: usize, ch: Option<Box<os_model::Channel>> },
    /// Control `.0`'s entry in the project's initial values; `None` while it isn't in the list (`crate::init`).
    Init(u32, Option<os_model::InitControl>),
    /// All the patterns and the current one (PATTERNS > Split by channel, spec: ui/split-by-channel.md §2).
    Patterns { patterns: Vec<os_model::Pattern>, current: u16 },
    /// What ADD > Audio clip / Automation clip changes in the rack (`RackSide`).
    Rack(Box<RackSide>),
    /// Several of these in one step, put back last first ("Create automation clip": the playlist and the initial
    /// value).
    Several(Vec<Snap>),
}

/// The rack's side of ADD > Audio clip / Automation clip (spec: ui/add-clip-channels.md §3): the added channel at `row`
/// (`None` while it doesn't exist; only the last row comes and goes), the rack's filter groups, filter and selection,
/// and the playlist's clip source.
#[derive(Debug, Clone, PartialEq)]
pub struct RackSide {
    pub row: usize,
    pub ch: Option<os_model::Channel>,
    pub groups: Vec<crate::rack::Group>,
    pub filter: crate::rack::Filter,
    pub unsorted_at: Option<usize>,
    pub sel: std::collections::BTreeSet<usize>,
    pub selected: usize,
    pub source: Option<os_model::ClipSource>,
    pub picker: crate::playlist::PickKind,
}

impl RackSide {
    /// Row `row`'s side as the rack has it now.
    pub fn now(s: &State, row: usize) -> RackSide {
        let r = &s.rack;
        RackSide {
            row,
            ch: s.project.channels.get(row).cloned(),
            groups: r.groups.clone(),
            filter: r.filter,
            unsorted_at: r.unsorted_at,
            sel: r.sel.clone(),
            selected: s.selected_channel,
            source: s.playlist.source,
            picker: s.playlist.picker,
        }
    }
}

impl Snap {
    /// Pattern `number` as it is now.
    pub fn pattern(s: &State, number: u16) -> Snap {
        let p = s.project.pattern(number);
        Snap::Pattern {
            number,
            notes: p.map(|p| p.notes.clone()).unwrap_or_default(),
            controls: p.map(|p| p.controls.clone()).unwrap_or_default(),
            length: p.and_then(|p| p.length),
        }
    }

    /// Channel `row`'s side of a sample load as it is now (`Snap::Sample`).
    pub fn sample(s: &State, row: usize) -> Option<Snap> {
        let ch = s.project.channels.get(row)?;
        let (path, name) = (ch.sample_path.clone(), ch.name.clone());
        Some(Snap::Sample { row, path, name, root: ch.misc.root, stretch: ch.sound.stretch, flags: ch.sound.sampler_flags })
    }

    /// What this covers, as the project has it now.
    fn now(&self, s: &State) -> Snap {
        match self {
            Snap::Pattern { number, .. } => Snap::pattern(s, *number),
            Snap::Playlist(_) => Snap::Playlist(Box::new(crate::playlist::capture(s))),
            Snap::Knob(id, _) => Snap::Knob(*id, knob_value(s, *id)),
            // A channel gone since is left as the step has it (`put` does nothing then).
            Snap::Sample { row, .. } => Snap::sample(s, *row).unwrap_or_else(|| self.clone()),
            Snap::Channel { row, .. } => Snap::Channel { row: *row, ch: s.project.channels.get(*row).cloned().map(Box::new) },
            Snap::Init(t, _) => Snap::Init(*t, crate::init::entry(s, *t)),
            Snap::Patterns { .. } => Snap::Patterns { patterns: s.project.patterns.clone(), current: s.pattern },
            Snap::Rack(r) => Snap::Rack(Box::new(RackSide::now(s, r.row))),
            Snap::Several(v) => Snap::Several(v.iter().map(|x| x.now(s)).collect()),
        }
    }

    /// Whether this holds the playlist (`forget_playlist`).
    fn has_playlist(&self) -> bool {
        match self {
            Snap::Playlist(_) => true,
            Snap::Several(v) => v.iter().any(Snap::has_playlist),
            _ => false,
        }
    }

    /// `rescale` for this.
    fn rescale(&mut self, from: u16, to: u16) {
        match self {
            Snap::Pattern { notes, controls, length, .. } => {
                notes.iter_mut().for_each(|n| n.rescale(from, to));
                controls.iter_mut().for_each(|c| c.rescale(from, to));
                *length = length.map(|l| os_model::rescale_length(l, from, to));
            }
            Snap::Playlist(p) => {
                p.0.iter_mut().for_each(|c| c.rescale(from, to));
                p.1.iter_mut().for_each(|m| m.rescale(from, to));
            }
            Snap::Patterns { patterns, .. } => {
                for p in patterns {
                    p.notes.iter_mut().for_each(|n| n.rescale(from, to));
                    p.controls.iter_mut().for_each(|c| c.rescale(from, to));
                    p.markers.iter_mut().for_each(|m| m.rescale(from, to));
                    p.length = p.length.map(|l| os_model::rescale_length(l, from, to));
                }
            }
            Snap::Several(v) => v.iter_mut().for_each(|x| x.rescale(from, to)),
            // A channel or an initial value holds no ticks.
            Snap::Knob(..) | Snap::Sample { .. } | Snap::Channel { .. } | Snap::Init(..) | Snap::Rack(_) => {}
        }
    }

    /// Puts this into the project. A pattern's step also shows that pattern, as the piano roll's undo always did.
    fn put(self, s: &mut State) {
        match self {
            Snap::Pattern { number, notes, controls, length } => {
                s.pattern = number;
                let p = crate::patterns::pat_mut(s, number);
                (p.notes, p.controls, p.length) = (notes, controls, length);
                s.piano.clear_selection();
            }
            Snap::Playlist(p) => crate::playlist::restore(s, *p),
            Snap::Knob(id, v) => set_knob_value(s, id, v),
            Snap::Sample { row, path, name, root, stretch, flags } => {
                if let Some(ch) = s.project.channels.get_mut(row) {
                    (ch.sample_path, ch.name, ch.misc.root, ch.sound.stretch) = (path.clone(), name, root, stretch);
                    ch.sound.sampler_flags = flags;
                    // Its Lemon Sampler follows on the next tick: the sample (none: cleared), root note, stretch and
                    // Keep on disk.
                    s.sample_loads.push((row, path.unwrap_or_default()));
                }
            }
            // The channel goes or comes back only as the last row: rows below it would move.
            Snap::Channel { row, ch } => match ch {
                None if row + 1 == s.project.channels.len() => crate::rack::remove_last(s),
                Some(c) if row == s.project.channels.len() => crate::rack::push_channel(s, *c),
                _ => {}
            },
            Snap::Init(t, e) => crate::init::put(s, t, e),
            Snap::Patterns { patterns, current } => {
                s.project.patterns = patterns;
                s.pattern = current;
                s.piano.clear_selection();
            }
            Snap::Rack(r) => {
                let RackSide { row, ch, groups, filter, unsorted_at, sel, selected, source, picker } = *r;
                match ch {
                    None if row + 1 == s.project.channels.len() => crate::rack::remove_last(s),
                    Some(c) if row == s.project.channels.len() => crate::rack::push_channel(s, c),
                    _ => {}
                }
                (s.rack.groups, s.rack.filter, s.rack.unsorted_at) = (groups, filter, unsorted_at);
                (s.rack.sel, s.selected_channel) = (sel, selected);
                (s.playlist.source, s.playlist.picker) = (source, picker);
            }
            Snap::Several(v) => v.into_iter().rev().for_each(|x| x.put(s)),
        }
        s.piano.dirty = true;
    }
}

/// The model's value of a knob: the channels' volume and pan as stored (exact), the others as the knob reads them.
fn knob_value(s: &State, id: KnobId) -> f64 {
    match id {
        KnobId::ChanVol(i) => s.project.channels.get(i).map_or(0.0, |c| c.volume),
        KnobId::ChanPan(i) => s.project.channels.get(i).map_or(0.0, |c| c.pan),
        _ => f64::from(s.knob(id)),
    }
}

fn set_knob_value(s: &mut State, id: KnobId, v: f64) {
    match id {
        KnobId::ChanVol(i) => s.project.channels.get_mut(i).into_iter().for_each(|c| c.volume = v),
        KnobId::ChanPan(i) => s.project.channels.get_mut(i).into_iter().for_each(|c| c.pan = v),
        _ => s.set_knob(id, v as f32),
    }
}

/// A knob step's name.
fn knob_name(id: KnobId) -> &'static str {
    match id {
        KnobId::ChanVol(_) => "Channel volume",
        KnobId::ChanPan(_) => "Channel panning",
        KnobId::MasterVol => "Main volume",
        KnobId::MasterPitch => "Main pitch",
        KnobId::MixPan(_) => "Mixer track panning",
        KnobId::MixSep(_) => "Mixer track stereo separation",
        KnobId::SlotMix(..) => "Effect mix level",
        KnobId::Send(..) => "Send level",
        KnobId::Swing => "Main swing",
        KnobId::Eq(..) => "Mixer track equalizer",
    }
}

#[derive(Debug, Clone)]
struct Step {
    name: &'static str,
    snap: Snap,
}

/// The project's undo history (`State::history`).
#[derive(Debug, Default)]
pub struct History {
    steps: Vec<Step>,
    /// Steps `..done` are done; the rest are undone (they can be redone).
    done: usize,
    /// The last move was an undo: with Alternate undo mode off, Ctrl+Z redoes it.
    undid: bool,
    /// The knob the newest step's mouse-wheel turns go to (one step for a run of notches).
    wheel: Option<KnobId>,
    /// The newest step was recorded since the last button release (`settle` looks at it then).
    fresh: bool,
}

impl History {
    /// How many steps there are ("Last reset" not counted).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Forgets every step: the project was opened or made ("Last reset").
    pub fn reset(&mut self) {
        *self = History::default();
    }

    /// Whether entry `p` of the list (0 = "Last reset", `k` = step `k`) is undone: drawn grey.
    pub fn undone(&self, p: usize) -> bool {
        p > self.done
    }

    /// Whether entry `p` is where the project is now.
    pub fn current(&self, p: usize) -> bool {
        p == self.done
    }

    /// The entry where the project is now (0 = "Last reset").
    pub fn at(&self) -> usize {
        self.done
    }

    /// The list as the browser's History folder shows it: (entry, name), oldest first from "Last reset", or newest
    /// first with Show recent changes at the top.
    pub fn entries(&self) -> Vec<(usize, &'static str)> {
        let mut v: Vec<(usize, &'static str)> = std::iter::once(LAST_RESET).chain(self.steps.iter().map(|st| st.name)).enumerate().collect();
        if prefs::check(&prefs::RECENT_CHANGES_TOP) {
            v.reverse();
        }
        v
    }

    /// Drops the oldest steps past Maximum undo levels (undone steps last).
    fn trim(&mut self) {
        let max = usize::from(prefs::number(&prefs::MAX_UNDO_LEVELS));
        while self.steps.len() > max && self.done > 0 {
            self.steps.remove(0);
            self.done -= 1;
        }
        self.steps.truncate(max.max(self.done));
    }

    /// What Ctrl+Z does next: redo (Alternate undo mode off, after an undo) or undo.
    fn ctrl_z_redoes(&self) -> bool {
        !prefs::check(&prefs::ALTERNATE_UNDO) && self.undid && self.done < self.steps.len()
    }
}

/// Records step `name`: `snap` holds what the edit is about to change. Undone steps are dropped (a new action ends
/// their branch), and the oldest step past Maximum undo levels.
pub fn record(s: &mut State, name: &'static str, snap: Snap) {
    let h = &mut s.history;
    h.steps.truncate(h.done);
    h.steps.push(Step { name, snap });
    h.done = h.steps.len();
    h.undid = false;
    h.wheel = None;
    h.fresh = true;
    h.trim();
}

/// Names the newest step `name` while its press is held (recorded since the last release): a gesture that turned into
/// another edit, as a held click's clone (`crate::hold`). The step keeps what it holds.
pub fn rename(s: &mut State, name: &'static str) {
    let h = &mut s.history;
    if h.fresh && h.done > 0 && h.done == h.steps.len() {
        h.steps[h.done - 1].name = name;
    }
}

/// Drops the newest step: its edit changed nothing after all.
pub fn unrecord(s: &mut State) {
    let h = &mut s.history;
    if h.done > 0 && h.done == h.steps.len() {
        h.steps.pop();
        h.done -= 1;
    }
    (h.wheel, h.fresh) = (None, false);
}

/// A button was released: the step recorded since the last release goes when its edit changed nothing (a click on a
/// note that did not move it, a knob clicked without a turn).
pub fn settle(s: &mut State) {
    let h = &s.history;
    if h.fresh && h.done > 0 && h.done == h.steps.len() && h.steps[h.done - 1].snap.now(s) == h.steps[h.done - 1].snap {
        unrecord(s);
    }
    s.history.fresh = false;
}

/// Before a sample loads into channel `row` (the file dialog, a drop, the browser's send): one step, "Load channel
/// sample(s)", holding what the load changes. `row` past the last channel: the load makes that channel.
pub fn load_sample(s: &mut State, row: usize) {
    let snap = Snap::sample(s, row).unwrap_or(Snap::Channel { row: s.project.channels.len(), ch: None });
    record(s, LOAD_SAMPLE, snap);
}

/// Before knob `id` turns: a step unless Undo knob tweaks is off. `wheel`: a mouse-wheel notch, which joins the step
/// of the notches just before it on the same knob.
pub fn knob(s: &mut State, id: KnobId, wheel: bool) {
    if !prefs::check(&prefs::UNDO_KNOB_TWEAKS) {
        return;
    }
    let h = &s.history;
    if wheel && h.wheel == Some(id) && h.done == h.steps.len() {
        return;
    }
    let snap = Snap::Knob(id, knob_value(s, id));
    record(s, knob_name(id), snap);
    if wheel {
        s.history.wheel = Some(id);
    }
}

fn swap(s: &mut State, i: usize) {
    let now = s.history.steps[i].snap.now(s);
    let old = std::mem::replace(&mut s.history.steps[i].snap, now);
    old.put(s);
}

/// Undoes the newest done step; its name, or `None` when nothing is done.
pub fn undo(s: &mut State) -> Option<&'static str> {
    let h = &mut s.history;
    if h.done == 0 {
        return None;
    }
    h.done -= 1;
    (h.undid, h.wheel, h.fresh) = (true, None, false);
    let i = h.done;
    swap(s, i);
    Some(s.history.steps[i].name)
}

/// Redoes the oldest undone step; its name, or `None` when nothing is undone.
pub fn redo(s: &mut State) -> Option<&'static str> {
    let h = &mut s.history;
    if h.done == h.steps.len() {
        return None;
    }
    let i = h.done;
    swap(s, i);
    let h = &mut s.history;
    h.done += 1;
    (h.undid, h.wheel, h.fresh) = (false, None, false);
    Some(h.steps[i].name)
}

/// "Step seq edit" as a menu entry writes it after Undo / Redo: "step seq edit".
fn lower(name: &str) -> String {
    let mut c = name.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

fn did(s: &mut State, redo_it: bool) -> String {
    let (verb, name) = if redo_it { ("Redo", redo(s)) } else { ("Undo", undo(s)) };
    match name {
        Some(n) => format!("{verb}: {}", lower(n)),
        None => format!("Nothing to {}", verb.to_lowercase()),
    }
}

/// Ctrl+Z (and EDIT's first entry): with Alternate undo mode on, one more step back; off, it toggles the last move
/// (undo, redo, undo). With Show undo history in the browser when undoing changes on, the browser shows the history.
/// Returns the hint bar's message.
pub fn ctrl_z(s: &mut State) -> Option<String> {
    let m = did(s, s.history.ctrl_z_redoes());
    shown_in_browser(s);
    Some(m)
}

/// Ctrl+Alt+Z (and EDIT's Redo): with Alternate undo mode on, redo; off, one more step back.
pub fn ctrl_alt_z(s: &mut State) -> Option<String> {
    let m = did(s, prefs::check(&prefs::ALTERNATE_UNDO));
    shown_in_browser(s);
    Some(m)
}

/// Ctrl+Shift+Z (Command+Shift+Z, macOS's usual redo): redo, in either mode. A Lemon Studio addition: FL redoes with
/// Ctrl+Alt+Z only.
pub fn ctrl_shift_z(s: &mut State) -> String {
    let m = did(s, true);
    shown_in_browser(s);
    m
}

fn shown_in_browser(s: &mut State) {
    if prefs::check(&prefs::UNDO_HISTORY_IN_BROWSER) {
        crate::browser::show_history(s);
    }
}

/// A click on entry `p` of the browser's History folder: undoes or redoes up to it.
pub fn go_to(s: &mut State, p: usize) -> Option<String> {
    let p = p.min(s.history.steps.len());
    while s.history.done > p {
        undo(s);
    }
    while s.history.done < p {
        redo(s);
    }
    let name = if p == 0 { LAST_RESET } else { s.history.steps[p - 1].name };
    Some(format!("History: {}", lower(name)))
}

/// The EDIT menu's entries (spec: ui/menus.md "EDIT"): the first says what Ctrl+Z does next ("Undo step seq edit",
/// "Redo step seq edit" with Alternate undo mode off after an undo), `redo` the second (Ctrl+Alt+Z's redo, Alternate
/// undo mode only). Without a step to act on, plain "Undo" / "Redo".
pub fn edit_label(s: &State, redo_entry: bool) -> String {
    let h = &s.history;
    let redo_it = redo_entry || h.ctrl_z_redoes();
    let (verb, step) = if redo_it { ("Redo", h.steps.get(h.done)) } else { ("Undo", h.done.checked_sub(1).and_then(|i| h.steps.get(i))) };
    match step {
        Some(st) => format!("{verb} {}", lower(st.name)),
        None => verb.into(),
    }
}

/// Whether EDIT's entry has a step to act on.
pub fn edit_enabled(s: &State, redo_entry: bool) -> bool {
    let h = &s.history;
    if redo_entry || h.ctrl_z_redoes() { h.done < h.steps.len() } else { h.done > 0 }
}

/// Whether EDIT shows its Redo entry: Alternate undo mode on and a history to act on (only Undo while it is empty).
pub fn redo_shown(s: &State) -> bool {
    prefs::check(&prefs::ALTERNATE_UNDO) && !s.history.steps.is_empty()
}

/// Maximum undo levels changed: the history keeps that many.
pub fn trim(s: &mut State) {
    s.history.trim();
}

/// Another arrangement is shown: the playlist's steps belong to the one left (spec: ui/playlist.md §10).
pub fn forget_playlist(s: &mut State) {
    let h = &mut s.history;
    let mut done = 0;
    let mut kept = Vec::with_capacity(h.steps.len());
    for (i, st) in std::mem::take(&mut h.steps).into_iter().enumerate() {
        if !st.snap.has_playlist() {
            done += usize::from(i < h.done);
            kept.push(st);
        }
    }
    (h.steps, h.done, h.wheel) = (kept, done, None);
}

/// The timebase changed from `from` to `to` ticks per quarter: the steps' notes, control points, lengths, clips and
/// markers follow, so no undo brings back ticks of the old timebase.
pub fn rescale(s: &mut State, from: u16, to: u16) {
    for st in &mut s.history.steps {
        st.snap.rescale(from, to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn note_count(s: &State) -> usize {
        s.project.pattern(s.pattern).map_or(0, |p| p.notes.len())
    }

    /// Two recorded edits: undo and redo swap them back and forth; a new action drops the undone branch; past
    /// Maximum undo levels the oldest step goes (spec: ui/settings-choices.md "Undo history: the four checks").
    #[test]
    fn steps_swap_branch_and_limit() {
        let mut app = App::new(None);
        let s = &mut app.state;
        let start = note_count(s);
        for k in 0..2 {
            let snap = Snap::pattern(s, s.pattern);
            record(s, STEP_SEQ_EDIT, snap);
            crate::rack::set_step(s, 0, 10 + k, true);
        }
        assert_eq!(note_count(s), start + 2);
        assert_eq!((undo(s), note_count(s)), (Some(STEP_SEQ_EDIT), start + 1));
        assert_eq!((undo(s), note_count(s)), (Some(STEP_SEQ_EDIT), start));
        assert_eq!(undo(s), None);
        assert_eq!((redo(s), note_count(s)), (Some(STEP_SEQ_EDIT), start + 1));
        assert!(s.history.undone(2) && s.history.current(1) && !s.history.undone(1));
        // A new action: the undone step is gone.
        let snap = Snap::pattern(s, s.pattern);
        record(s, "Piano roll add note", snap);
        crate::rack::set_step(s, 0, 14, true);
        assert_eq!(s.history.entries().iter().map(|e| e.1).collect::<Vec<_>>(), [LAST_RESET, STEP_SEQ_EDIT, "Piano roll add note"]);
        assert_eq!(redo(s), None);
        // Three levels kept: the oldest go.
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 3);
        for k in 0..3 {
            let snap = Snap::pattern(s, s.pattern);
            record(s, "Piano roll edit", snap);
            crate::rack::set_step(s, 0, 20 + k, true);
        }
        assert_eq!(s.history.len(), 3);
        assert!(s.history.entries().iter().skip(1).all(|e| e.1 == "Piano roll edit"), "{:?}", s.history.entries());
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 2);
        trim(s);
        assert_eq!((s.history.len(), s.history.done), (2, 2));
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, prefs::MAX_UNDO_LEVELS.default);
    }

    /// A step whose edit changed nothing is dropped when the button comes up (a step of an earlier press stays); a
    /// knob's wheel notches make one step.
    #[test]
    fn settle_and_wheel_runs() {
        let mut app = App::new(None);
        let s = &mut app.state;
        let snap = Snap::pattern(s, s.pattern);
        record(s, "Piano roll edit", snap);
        settle(s);
        assert_eq!(s.history.len(), 0, "nothing changed");
        let snap = Snap::pattern(s, s.pattern);
        record(s, "Piano roll edit", snap);
        crate::rack::set_step(s, 0, 9, !crate::rack::row_steps(s, 0, 10)[9]);
        settle(s);
        crate::rack::set_step(s, 0, 9, !crate::rack::row_steps(s, 0, 10)[9]);
        settle(s);
        assert_eq!(s.history.len(), 1, "an earlier press's step stays");
        s.history.reset();
        let id = KnobId::ChanVol(0);
        let v0 = s.project.channels[0].volume;
        for _ in 0..3 {
            knob(s, id, true);
            let v = s.knob(id) - 0.03;
            s.set_knob(id, v);
        }
        assert_eq!(s.history.len(), 1, "one step for the notches");
        assert_eq!(undo(s), Some("Channel volume"));
        assert_eq!(s.project.channels[0].volume, v0, "exactly back");
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Through the app, as the spec measured it: two step-sequencer clicks (row 0 step 1, row 1 step 2), Ctrl+Z,
    // Ctrl+Alt+Z, the first channel's volume knob and the EDIT menu (spec: ui/settings-choices.md "Undo history: the
    // four checks (2026-10-16)").

    use crate::app::Action;
    use crate::icons::Icon;
    use crate::views::browser::Item;
    use crate::views::menus::{bar_model, EDIT_MENU};
    use winit::keyboard::Key;

    /// A click on step `k` of rack row `row`: press and release.
    fn click_step(app: &mut App, row: usize, k: u32) {
        app.act(Action::Step(row, k));
        app.mouse_up();
    }

    /// Rack rows 0 and 1, their first steps.
    fn steps(app: &App) -> [Vec<bool>; 2] {
        [0, 1].map(|r| crate::rack::row_steps(&app.state, r, 4))
    }

    fn ctrl_z(app: &mut App) {
        app.key(&Key::Character("z".into()), true, false);
    }

    fn ctrl_alt_z(app: &mut App) {
        app.key(&Key::Character("z".into()), true, true);
    }

    fn ctrl_shift_z(app: &mut App) {
        app.shift = true;
        app.key(&Key::Character("Z".into()), true, false);
        app.shift = false;
    }

    /// A drag on knob `id`, `dy` pixels down.
    fn turn(app: &mut App, id: KnobId, dy: f32) {
        app.mouse_move(100.0, 300.0);
        app.act(Action::Knob(id));
        app.mouse_move(100.0, 300.0 + dy);
        app.mouse_up();
    }

    /// EDIT as it shows now: (entry, shortcut, can be chosen).
    fn edit_menu(app: &App) -> Vec<(String, &'static str, bool)> {
        bar_model(&app.state)[EDIT_MENU].rows.iter().map(|r| (r.label.clone(), r.shortcut, r.enabled)).collect()
    }

    fn entry(label: &str, shortcut: &'static str, enabled: bool) -> (String, &'static str, bool) {
        (label.into(), shortcut, enabled)
    }

    /// The browser's Current project ▸ History, opened: (name, mirrored arrow, drawn grey, where the project is).
    fn history_rows(app: &mut App) -> Vec<(String, bool, bool, bool)> {
        app.state.browser_tab = 1;
        app.state.browser_open.extend(["1:Current project".to_string(), "1:Current project/History".to_string()]);
        let h = &app.state.history;
        crate::views::browser::rows(&app.state)
            .into_iter()
            .filter_map(|r| match r.item {
                Item::History(e) => Some((r.label, r.icon == Icon::Redo, h.undone(e), r.bright)),
                _ => None,
            })
            .collect()
    }

    /// Alternate undo mode on (classic): each Ctrl+Z steps one further back, the third does nothing; Ctrl+Alt+Z
    /// redoes; EDIT shows "Undo step seq edit  Ctrl+Z" and "Redo step seq edit  Ctrl+Alt+Z" (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Alternate undo mode On).
    #[test]
    fn alternate_mode_on_steps_back_and_redoes() {
        let mut app = App::new(None);
        assert!(prefs::check(&prefs::ALTERNATE_UNDO), "on by default");
        let start = steps(&app);
        click_step(&mut app, 0, 0);
        let one = steps(&app);
        click_step(&mut app, 1, 1);
        let two = steps(&app);
        assert!(start != one && one != two);
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true), entry("Redo", "Ctrl+Alt+Z", false)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "the second click undone");
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true), entry("Redo step seq edit", "Ctrl+Alt+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), start, "then the first");
        assert_eq!(edit_menu(&app), [entry("Undo", "Ctrl+Z", false), entry("Redo step seq edit", "Ctrl+Alt+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!((steps(&app), app.state.message.as_deref()), (start.clone(), Some("Nothing to undo")), "a third does nothing");
        ctrl_alt_z(&mut app);
        assert_eq!(steps(&app), one, "Ctrl+Alt+Z redoes");
        // EDIT's Redo does what Ctrl+Alt+Z does.
        app.menu_item(EDIT_MENU, 1);
        assert_eq!(steps(&app), two);
        app.menu_item(EDIT_MENU, 0);
        assert_eq!(steps(&app), one);
    }

    /// Alternate undo mode off: Ctrl+Z three times undoes the second click, redoes it and undoes it again; Ctrl+Alt+Z
    /// then undoes the first. EDIT has one entry naming what Ctrl+Z does next, "Redo step seq edit  Ctrl+Z" after an
    /// undo (spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Alternate undo mode Off).
    #[test]
    fn alternate_mode_off_toggles_the_last_step() {
        let mut app = App::new(None);
        prefs::set_check(&prefs::ALTERNATE_UNDO, false);
        let start = steps(&app);
        click_step(&mut app, 0, 0);
        let one = steps(&app);
        click_step(&mut app, 1, 1);
        let two = steps(&app);
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "undid the second click");
        assert_eq!(edit_menu(&app), [entry("Redo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), two, "redid it");
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "undid it again");
        ctrl_alt_z(&mut app);
        assert_eq!(steps(&app), start, "Ctrl+Alt+Z steps back further: the first click undone");
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "Ctrl+Z toggles that back");
        // EDIT's one entry does what Ctrl+Z does.
        app.menu_item(EDIT_MENU, 0);
        assert_eq!(steps(&app), start);
        prefs::set_check(&prefs::ALTERNATE_UNDO, true);
    }

    /// Ctrl+Shift+Z (Command+Shift+Z on macOS) redoes in either mode, where Ctrl+Z alone would undo or toggle (a Lemon
    /// Studio addition; spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn ctrl_shift_z_redoes_in_either_mode() {
        for alternate in [true, false] {
            let mut app = App::new(None);
            prefs::set_check(&prefs::ALTERNATE_UNDO, alternate);
            click_step(&mut app, 0, 0);
            let one = steps(&app);
            click_step(&mut app, 1, 1);
            let two = steps(&app);
            ctrl_z(&mut app);
            assert_eq!(steps(&app), one, "alternate undo mode {alternate}: undone");
            ctrl_shift_z(&mut app);
            assert_eq!(steps(&app), two, "alternate undo mode {alternate}: redone");
            ctrl_shift_z(&mut app);
            let nothing = (two.clone(), Some("Nothing to redo"));
            assert_eq!((steps(&app), app.state.message.as_deref()), nothing, "alternate undo mode {alternate}");
        }
        prefs::set_check(&prefs::ALTERNATE_UNDO, true);
    }

    /// EDIT with an empty history: only Undo, greyed out (spec: ui/menus.md "EDIT"; ui/settings-choices.md "Undo
    /// history: the four checks (2026-10-16)").
    #[test]
    fn edit_menu_with_no_history() {
        let app = App::new(None);
        assert_eq!(edit_menu(&app), [entry("Undo", "Ctrl+Z", false)]);
    }

    /// Undo knob tweaks: on, a turn of a channel knob is a step and Ctrl+Z puts the knob back, the click before it
    /// staying; off, the turn makes no step and Ctrl+Z leaves the knob, undoing the click made before it (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Undo knob tweaks).
    #[test]
    fn knob_tweaks_on_and_off() {
        let id = KnobId::ChanVol(0);
        for on in [true, false] {
            let mut app = App::new(None);
            prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, on);
            let start = steps(&app);
            click_step(&mut app, 0, 0);
            let clicked = steps(&app);
            let v0 = app.state.project.channels[0].volume;
            turn(&mut app, id, 40.0);
            let v1 = app.state.project.channels[0].volume;
            assert_ne!(v1, v0, "the knob turned");
            assert_eq!(app.state.history.len(), if on { 2 } else { 1 });
            ctrl_z(&mut app);
            let vol = app.state.project.channels[0].volume;
            if on {
                assert_eq!((vol, steps(&app)), (v0, clicked), "the knob back, the click kept");
            } else {
                assert_eq!((vol, steps(&app)), (v1, start), "the knob kept, the click undone");
            }
        }
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, true);
    }

    /// The browser's History: "Last reset" then the steps, oldest first; undone steps grey with the mirrored arrow; a
    /// new action drops them; newest first with Show recent changes at the top; a click on an entry goes there (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Show undo history in the browser, Show
    /// recent changes at the top).
    #[test]
    fn history_folder_lists_greys_and_drops() {
        let mut app = App::new(None);
        let start = steps(&app);
        assert_eq!(history_rows(&mut app), [(LAST_RESET.into(), false, false, true)]);
        click_step(&mut app, 0, 0);
        click_step(&mut app, 1, 1);
        ctrl_z(&mut app);
        let seq = |undone: bool, now: bool| (STEP_SEQ_EDIT.to_string(), undone, undone, now);
        assert_eq!(history_rows(&mut app), [(LAST_RESET.into(), false, false, false), seq(false, true), seq(true, false)]);
        // Newest first: "Last reset" at the bottom.
        prefs::set_check(&prefs::RECENT_CHANGES_TOP, true);
        assert_eq!(history_rows(&mut app), [seq(true, false), seq(false, true), (LAST_RESET.into(), false, false, false)]);
        prefs::set_check(&prefs::RECENT_CHANGES_TOP, false);
        // A new action (a knob turn) drops the undone step: nothing to redo.
        turn(&mut app, KnobId::ChanVol(0), 40.0);
        let names: Vec<String> = history_rows(&mut app).into_iter().map(|r| r.0).collect();
        assert_eq!(names, [LAST_RESET, STEP_SEQ_EDIT, "Channel volume"]);
        assert!(history_rows(&mut app).iter().all(|r| !r.2), "none undone");
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.message.as_deref(), Some("Nothing to redo"));
        // A click on "Last reset" undoes everything; on the newest entry, redoes it all.
        let rows = crate::views::browser::rows(&app.state);
        let at = |e: usize| rows.iter().position(|r| r.item == Item::History(e)).expect("history row");
        let (first, last) = (at(0), at(2));
        app.act(Action::BrowserRow(first));
        app.mouse_up();
        assert_eq!(steps(&app), start);
        assert!(app.state.history.undone(1) && app.state.history.undone(2) && app.state.history.current(0));
        assert!(app.drag.is_none(), "a history entry is not dragged");
        app.act(Action::BrowserRow(last));
        app.mouse_up();
        assert!(app.state.history.current(2));
    }

    /// Show undo history in the browser when undoing changes: off, Ctrl+Z leaves the browser where it is; on, the
    /// browser shows Current project ▸ History, opened (spec: ui/settings-choices.md "Undo history: the four checks
    /// (2026-10-16)").
    #[test]
    fn ctrl_z_shows_the_history_in_the_browser() {
        let mut app = App::new(None);
        click_step(&mut app, 0, 0);
        click_step(&mut app, 1, 1);
        app.state.show_browser = false;
        app.state.browser_tab = 2;
        let open = app.state.browser_open.clone();
        ctrl_z(&mut app);
        assert_eq!((app.state.show_browser, app.state.browser_tab, &app.state.browser_open), (false, 2, &open), "off by default");
        prefs::set_check(&prefs::UNDO_HISTORY_IN_BROWSER, true);
        ctrl_z(&mut app);
        assert!(app.state.show_browser);
        assert_eq!(app.state.browser_tab, 1);
        assert!(app.state.browser_open.contains("1:Current project/History"));
        assert_eq!(app.state.browser_sel.as_deref(), Some("1:Current project/History/0"), "where the project now is");
        prefs::set_check(&prefs::UNDO_HISTORY_IN_BROWSER, false);
    }

    /// Maximum undo levels: past it the oldest steps go, so those edits stay; the General page shows the value (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn max_levels_drop_the_oldest() {
        let mut app = App::new(None);
        assert_eq!(prefs::number(&prefs::MAX_UNDO_LEVELS), 100, "100 by default");
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 3);
        let shown = crate::views::settings::page(crate::views::settings::GENERAL, &app.state)
            .into_iter()
            .flat_map(|g| g.rows)
            .any(|r| matches!(r, crate::views::settings::Row::Value(v, "Maximum undo levels") if v == "3"));
        assert!(shown, "the General page shows the value");
        let start = crate::rack::row_steps(&app.state, 0, 5);
        for k in 0..5 {
            click_step(&mut app, 0, k);
        }
        assert_eq!(app.state.history.len(), 3);
        for _ in 0..4 {
            ctrl_z(&mut app);
        }
        let now = crate::rack::row_steps(&app.state, 0, 5);
        assert_eq!(now[..2], start[..2].iter().map(|b| !b).collect::<Vec<_>>()[..], "the two oldest clicks can no longer be undone");
        assert_eq!(now[2..], start[2..]);
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, prefs::MAX_UNDO_LEVELS.default);
    }

    /// Put undone recordings in the recycle bin: stored and shown, on by default; Lemon Studio does not record yet
    /// (spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn recycle_bin_check_is_kept() {
        assert!(prefs::check(&prefs::UNDONE_RECORDINGS_TO_BIN));
        assert!(!prefs::toggle(&prefs::UNDONE_RECORDINGS_TO_BIN));
        assert!(!prefs::check(&prefs::UNDONE_RECORDINGS_TO_BIN));
        prefs::set_check(&prefs::UNDONE_RECORDINGS_TO_BIN, true);
    }
}
