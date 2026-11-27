// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project's undo history: one list of named steps for the whole project, oldest first, after "Last reset" (the
//! project as opened or made). An edit records a step holding what it is about to change (`record`); undo and redo
//! swap that with the project's, so a step always holds the other side of itself: the state before it while it is
//! done, the state after it while it is undone. Covered: a pattern's notes, control points and length (the piano roll
//! and the step sequencer), the playlist (`playlist::snapshot`), knob turns, and a timebase change with the ticks it
//! holds (`timebase`). Everything the control plane's commands change (`crate::control`) is
//! covered too: their steps hold the commands that undo them (`Snap::Commands`), as do the interface's edits that go
//! through `control::edit` (the mixer and its faders; the rack's channels added, moved and deleted, FILE ▸ Import ▸
//! MIDI file into the open project among them; the tempo and Settings > Project's values), the plugin parameters
//! Lemon Studio sets (`control::param_set`), those moved in a plugin's own window (`control::param_edit`) and the
//! presets loaded into a plugin from its window or dropped on it (`control::state_edit`). The timebase has a step of
//! its own (`timebase`), which brings back the ticks a lower timebase rounded.
//!
//! The settings of General ▸ Undo history (`crate::prefs`): Maximum undo levels (past it the oldest step goes), Undo
//! knob tweaks, Alternate undo mode (what Ctrl+Z and Ctrl+Alt+Z do), Show undo history in the browser when undoing
//! changes and Show recent changes at the top (the browser's History folder, `views::browser`).
// spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)"

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::borrow::Cow;

use os_model::{ControlPoint, Note};

use crate::knob_id::KnobId;
use crate::prefs;

/// The history's first entry: the project as opened or reset.
pub const LAST_RESET: &str = "Last reset";
/// A step-sequencer step click (FL's name).
/// The loop icon and the loop type (spec: ui/undo-steps.md).
pub const LOOP_MODE: &str = "Set pattern loop mode";
/// A loop box (spec: ui/undo-steps.md).
pub const LOOP_POINT: &str = "Set advanced looping settings";

pub const STEP_SEQ_EDIT: &str = "Step seq edit";
/// A sample loaded into a channel (FL's name; spec: ui/settings-choices.md "Undo history: the four checks").
pub const LOAD_SAMPLE: &str = "Load channel sample(s)";
/// "Create automation clip" (FL's name), with the control's initial value while "Set start values automatically" is
/// on (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
pub const CREATE_AUTOMATION: &str = "Create automation clip";
/// A chord written by step edit (spec: ui/step-edit.md).
pub const STEP_EDIT: &str = "Step edit";
/// A mixer track's volume fader moved (named like the knobs' steps, "Mixer track panning"; `control::edit_press`).
pub const MIXER_VOLUME: &str = "Mixer track volume";
/// A generator added to the rack (`plugin_menu::add_generator`).
pub const ADD_CHANNEL: &str = "Add channel";
/// Channels deleted from the rack (`rack::delete_step`).
pub const DELETE_CHANNEL: &str = "Delete channel";
/// A value of Settings > Project changed (`timebase`).
pub const PROJECT_SETTINGS: &str = "Project settings";
/// A channel's generator replaced (`plugin_menu::replace_generator`).
pub const REPLACE_CHANNEL: &str = "Replace channel";
/// A mixer track's audio input, output, record source or monitor mode changed (`track_io`).
pub const MIXER_TRACK_IO: &str = "Mixer track audio input / output";

/// What a step changes, as it is on the other side of the step.
#[derive(Debug, Clone, PartialEq)]
pub enum Snap {
    /// Pattern `number`'s notes, control points and length (the piano roll, the step sequencer).
    Pattern { number: u16, notes: Vec<Note>, controls: Vec<ControlPoint>, length: Option<u32>, loops: (Vec<(u16, u32)>, Option<u32>) },
    /// The playlist (`playlist::capture`).
    Playlist(Box<crate::edit::playlist::Snapshot>),
    /// Several of these in one step, put back last first ("Create automation clip": the playlist and the initial
    /// value).
    Several(Vec<Snap>),
    /// The control plane's commands that take a step back (`control::run`'s undo); applying them gives the commands
    /// for the other way (`swap`).
    Commands(Vec<crate::control::Command>),
    /// The timebase on one side of its change, with the ticks it holds (`Timebase`).
    Timebase(Box<Timebase>),
}

/// The timebase on one side of its change (`timebase`), with the patterns and arrangements at that timebase, so an undo
/// brings back the ticks a lower timebase rounded. The history's other steps follow the timebase (`rescale`); these
/// keep their own.
#[derive(Debug, Clone, PartialEq)]
pub struct Timebase {
    pub ppq: u16,
    pub patterns: Vec<os_model::Pattern>,
    pub arrangements: Vec<os_model::Arrangement>,
}

impl Timebase {
    /// The project's timebase and ticks now.
    pub fn now(s: &crate::session::Session) -> Timebase {
        let p = &s.project;
        Timebase { ppq: p.ppq, patterns: p.patterns.clone(), arrangements: p.arrangements.clone() }
    }
}

/// Before the timebase changes (Settings > Project, a new project's Save as form): one step, "Project settings",
/// holding the timebase and the project's ticks, so an undo brings back the ticks exactly.
pub fn timebase(s: &mut crate::session::Session) {
    let snap = Snap::Timebase(Box::new(Timebase::now(s)));
    record(s, PROJECT_SETTINGS, snap);
}

/// A knob step's name.
fn knob_name(id: KnobId) -> &'static str {
    match id {
        KnobId::ChanVol(_) => "Channel volume",
        KnobId::ChanPan(_) => "Channel panning",
        KnobId::MasterVol => "Main volume",
        KnobId::MasterPitch => "Main pitch",
        KnobId::MixVol(_) => MIXER_VOLUME,
        KnobId::MixPan(_) => "Mixer track panning",
        KnobId::MixSep(_) => "Mixer track stereo separation",
        KnobId::SlotMix(..) => "Effect mix level",
        KnobId::Send(..) => "Send level",
        KnobId::Swing => "Main swing",
        KnobId::Eq(..) => "Mixer track equalizer",
        KnobId::Strum(_) => "Piano roll strum",
        KnobId::Arp(_) => "Piano roll arpeggiate",
        KnobId::PianoDlg(_) => "Piano roll tool",
        KnobId::NoteProp(_) => "Note properties",
        KnobId::AutoLfo(..) => "Automation clip LFO",
        KnobId::MidiOut(..) => "MIDI Out setting",
        KnobId::Member(..) => crate::container::LEVEL,
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    pub name: Cow<'static, str>,
    snap: Snap,
    /// Its number, never reused: what the interface keeps for a step is filed under it (`crate::follow`).
    id: u64,
}

/// The project's undo history (`State::history`).
#[derive(Debug, Default)]
pub struct History {
    pub steps: Vec<Step>,
    /// Steps `..done` are done; the rest are undone (they can be redone).
    pub done: usize,
    /// The last move was an undo: with Alternate undo mode off, Ctrl+Z redoes it.
    undid: bool,
    /// What the newest step's run of changes goes to: a knob's mouse-wheel notches, the tempo tapper's taps (one step
    /// for the run).
    run: Option<Run>,
    /// The newest step was recorded since the last button release (`settle` looks at it then).
    fresh: bool,
    /// The number the next step gets (`Step::id`).
    next_id: u64,
    /// Counts steps recorded, joined, undone and redone (`changes`).
    changes: u64,
}

/// A run of changes that make one step (`History::run`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Run {
    Wheel(KnobId),
    Tap,
    /// Mouse-wheel notches on Settings > Project's time boxes.
    Bar,
    /// Values plugin `.0` sends outside a gesture (no begin and end of an edit).
    Param(u32),
    /// Changes of control `.1` of the channel on row `.0` (`ChannelControl`): a plugin window's strip knob dragged or
    /// turned, the rack's mixer track box.
    Channel(usize, ChannelControl),
    /// A controller moving the wet (`.3`) or dry level of member `.2` of the container in slot `.1` of track `.0`.
    Member(u16, u8, u32, bool),
    /// A controller moving the tempo.
    Tempo,
    /// A client's gesture (`control::begin_in`).
    Gesture(u64),
    /// The steps a press in the step sequencer paints, with the drag that follows.
    Steps,
    /// A loop box dragged.
    Loop(usize),
    /// The graph editor's bars dragged.
    Graph,
}

/// A channel control a run of changes goes to (`Run::Channel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelControl {
    Volume,
    Pan,
    Pitch,
    Track,
}

impl History {
    /// Moves on with every step recorded, joined, undone or redone: what a plugin window last knew of its instance's
    /// state is read again then (`EditorHost::refresh_known`).
    pub fn changes(&self) -> u64 {
        self.changes
    }

    /// Whether a change of run `r` joins the newest step: the step is of that run and nothing came after it.
    pub fn joins(&self, r: Run) -> bool {
        self.run == Some(r) && self.done == self.steps.len()
    }

    /// The newest step's run goes on with `r` (`joins`).
    pub fn set_run(&mut self, r: Run) {
        self.run = Some(r);
    }

    /// The run ends: the next change is a step of its own.
    pub fn end_run(&mut self) {
        self.run = None;
    }

    /// A drag was let go or the wheel paused (`WHEEL_PAUSE`): the run of a knob, fader, box or the mixer track box
    /// ends, so the next drag or run of notches is a step of its own, as in FL (spec: ui/undo-steps.md). The tempo
    /// tapper's taps, a plugin's own values, a controller's and a client's gesture run on.
    pub fn end_gesture_run(&mut self) {
        if matches!(self.run, Some(Run::Wheel(_) | Run::Bar | Run::Channel(..) | Run::Steps | Run::Loop(_) | Run::Graph)) {
            self.run = None;
        }
    }

    /// Run `r` ends, if it is the newest step's.
    pub fn end_run_of(&mut self, r: Run) {
        if self.run == Some(r) {
            self.run = None;
        }
    }

    /// How many steps there are ("Last reset" not counted).
    #[cfg(any(test, feature = "test-util"))]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// How many steps are done (undone ones not counted).
    pub fn done(&self) -> usize {
        self.done
    }

    /// The name of the newest done step.
    pub fn newest_done(&self) -> Option<&str> {
        self.done.checked_sub(1).map(|i| &*self.steps[i].name)
    }

    /// Whether the newest done step is made of commands (`Snap::Commands`).
    pub fn newest_is_commands(&self) -> bool {
        self.done.checked_sub(1).is_some_and(|i| matches!(self.steps[i].snap, Snap::Commands(_)))
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
    pub fn entries(&self, st: &crate::settings_file::SettingsStore) -> Vec<(usize, &str)> {
        let mut v: Vec<(usize, &str)> = std::iter::once(LAST_RESET).chain(self.steps.iter().map(|st| &*st.name)).enumerate().collect();
        if st.check(&prefs::RECENT_CHANGES_TOP) {
            v.reverse();
        }
        v
    }

    /// Drops the oldest steps past Maximum undo levels (undone steps last).
    /// The newest step's number (`Step::id`).
    pub fn newest_id(&self) -> Option<u64> {
        self.steps.last().map(|st| st.id)
    }

    /// Whether step `id` is still in the history.
    pub fn has_step(&self, id: u64) -> bool {
        self.steps.iter().any(|st| st.id == id)
    }

    fn trim(&mut self, st: &crate::settings_file::SettingsStore) {
        let max = usize::from(st.number(&prefs::MAX_UNDO_LEVELS));
        while self.steps.len() > max && self.done > 0 {
            self.steps.remove(0);
            self.done -= 1;
        }
        self.steps.truncate(max.max(self.done));
    }

    /// What Ctrl+Z does next: redo (Alternate undo mode off, after an undo) or undo.
    pub fn ctrl_z_redoes(&self, st: &crate::settings_file::SettingsStore) -> bool {
        !st.check(&prefs::ALTERNATE_UNDO) && self.undid && self.done < self.steps.len()
    }
}

/// Records step `name`: `snap` holds what the edit is about to change. Undone steps are dropped (a new action ends
/// their branch), and the oldest step past Maximum undo levels.
pub fn record(s: &mut crate::session::Session, name: &'static str, snap: Snap) {
    record_named(s, Cow::Borrowed(name), snap);
}

/// `record` with a name made at run time (a client's step: "Assistant: build drum kit").
pub fn record_named(s: &mut crate::session::Session, name: Cow<'static, str>, snap: Snap) {
    let h = &mut s.history;
    h.steps.truncate(h.done);
    h.next_id += 1;
    h.steps.push(Step { name, snap, id: h.next_id });
    h.done = h.steps.len();
    h.undid = false;
    h.run = None;
    h.fresh = true;
    h.changes += 1;
    h.trim(&s.settings);
}

/// Adds to the newest step's commands (a plugin's values joining its step, `control::param_edit`), when it is done
/// and holds commands.
pub fn extend_newest(s: &mut crate::session::Session, f: impl FnOnce(&mut Vec<crate::control::Command>)) {
    let h = &mut s.history;
    h.changes += 1;
    if h.done > 0 && h.done == h.steps.len() {
        if let Snap::Commands(c) = &mut h.steps[h.done - 1].snap {
            f(c);
        }
    }
}

/// The plugin state side numbered `fill` (`PluginSide::fill`), in whichever step holds it, takes the state and preset
/// the app found in the plugin as it put the other side in (`App::host_commands`).
pub fn fill_plugin_side(s: &mut crate::session::Session, fill: u64, state: &[u8], preset: Option<&std::path::Path>) {
    use crate::control::command::Taken;
    for st in &mut s.history.steps {
        let Snap::Commands(v) = &mut st.snap else { continue };
        for c in v {
            if let crate::control::Command::Put(t) = c {
                if let Taken::Plugin(_, side) = &mut **t {
                    if side.fill == fill {
                        (side.state, side.preset, side.fill) = (state.to_vec(), preset.map(Into::into), 0);
                        return;
                    }
                }
            }
        }
    }
}

/// The newest step is done with: no button release drops it (`settle`), a client's change, made without a press.
pub fn done_with(s: &mut crate::session::Session) {
    s.history.fresh = false;
}

/// Names the newest step `name` while its press is held (recorded since the last release): a gesture that turned into
/// another edit, as a held click's clone (`crate::hold`). The step keeps what it holds.
pub fn rename(s: &mut crate::session::Session, name: &'static str) {
    let h = &mut s.history;
    if h.fresh && h.done > 0 && h.done == h.steps.len() {
        h.steps[h.done - 1].name = Cow::Borrowed(name);
    }
}

/// Drops the newest step: its edit changed nothing after all.
pub fn unrecord(s: &mut crate::session::Session) {
    let h = &mut s.history;
    if h.done > 0 && h.done == h.steps.len() {
        h.steps.pop();
        h.done -= 1;
    }
    (h.run, h.fresh) = (None, false);
}

/// A button was released: the step recorded since the last release goes when its edit changed nothing (a click on a
/// note that did not move it, a knob clicked without a turn).
pub fn settle(s: &mut crate::session::Session) {
    let h = &s.history;
    if h.fresh && h.done > 0 && h.done == h.steps.len() {
        if h.steps[h.done - 1].snap.now(s) == h.steps[h.done - 1].snap {
            unrecord(s);
        } else {
            pattern_to_commands(s, h.done - 1);
        }
    }
    s.history.fresh = false;
    s.history.end_gesture_run();
}

/// An edit of a pattern the interface made on it, step `i` holding the pattern as it was (`Snap::Pattern`), turned
/// into the commands a client would apply for it: the pattern goes back as it was and `set_notes` and `set_pattern`
/// bring it to what the edit made (with `set_controls` and `set_loops` for its events and loops), the step holding
/// their undo. Left as it is when a note or a loop names a channel the rack has not.
fn pattern_to_commands(s: &mut crate::session::Session, i: usize) {
    use crate::control::command::{Command, NoteIn};
    let Snap::Pattern { number, notes, controls, length, loops } = &s.history.steps[i].snap else { return };
    let Some(p) = s.project.pattern(*number) else { return };
    let ids: std::collections::HashMap<u16, u32> = s.project.channels.iter().map(|c| (c.index, c.id.0)).collect();
    if p.id.is_none() || p.notes.iter().any(|n| ids.get(&n.channel).is_none_or(|&id| id == 0)) {
        return;
    }
    // Loops name channels by index: each one's id, before and after.
    let loop_channels: std::collections::BTreeSet<u16> = loops.0.iter().chain(&p.loops).map(|l| l.0).collect();
    if loop_channels.iter().any(|ch| ids.get(ch).is_none_or(|&id| id == 0)) {
        return;
    }
    let after = p.clone();
    let mut commands = Vec::new();
    let same_controls = after.controls.len() == controls.len() && after.controls.iter().zip(controls).all(|(a, b)| a == b && a.id.0 == b.id.0);
    if !same_controls {
        let list = after.controls.iter().map(|c| crate::control::command::ControlIn { position: c.position, value: c.value, channel: None, param: None, kind: Some(c.kind), id: (c.id.0 != 0).then_some(c.id.0) }).collect();
        commands.push(Command::SetControls { pattern: after.id.0, controls: list });
    }
    if (after.loops.clone(), after.loop_mode) != *loops {
        let value = |ch: u16| after.loops.iter().find(|l| l.0 == ch).map(|l| l.1);
        let list = loop_channels.iter().map(|&ch| crate::control::command::LoopIn { channel: ids[&ch], value: value(ch) }).collect();
        commands.push(Command::SetLoops { pattern: after.id.0, mode: Some(after.loop_mode.map_or(0, |m| m as u8)), loops: list });
    }
    let same_notes = after.notes.len() == notes.len() && after.notes.iter().zip(notes).all(|(a, b)| a == b && a.id.0 == b.id.0);
    if !same_notes {
        commands.push(Command::SetNotes { pattern: after.id.0, channel: None, notes: after.notes.iter().map(|n| NoteIn::of(n, ids[&n.channel])).collect() });
    }
    if after.length != *length {
        commands.push(Command::SetPattern { pattern: after.id.0, name: None, color: None, length: Some(after.length.unwrap_or(0)) });
    }
    if commands.is_empty() {
        return;
    }
    // The pattern as it was, then the commands. It ends as the edit left it: nothing for the views to follow (the
    // piano roll keeps its selection), so what the two say they touched is dropped.
    let touched = std::mem::take(&mut s.touched);
    let snap = std::mem::replace(&mut s.history.steps[i].snap, Snap::Commands(Vec::new()));
    snap.clone().put(s);
    match crate::control::run_in(s, commands) {
        Ok((_, undo)) => s.history.steps[i].snap = Snap::Commands(undo),
        Err(_) => {
            // Not expressible after all: the edit's pattern back, the step as it was.
            if let Some(place) = s.project.patterns.iter().position(|q| q.number == after.number) {
                s.project.get_mut().patterns[place] = after;
            }
            s.history.steps[i].snap = snap;
        }
    }
    s.touched = touched;
}

/// How long the wheel may rest between notches that make one step. FL joined notches about 120 ms apart and made a
/// step of each 2.5 s apart (spec: ui/undo-steps.md); the time between was not measured.
pub const WHEEL_PAUSE: std::time::Duration = std::time::Duration::from_secs(1);

/// A mixer track's input, output, record source and monitor mode.
pub fn track_io_of(m: &os_model::MixerTrack) -> [i32; 4] {
    [m.audio_in, m.audio_out, i32::from(m.rec_source), i32::from(m.monitor)]
}

/// Before knob `id` turns: a step unless Undo knob tweaks is off. `wheel`: a mouse-wheel notch, which joins the step
/// of the notches just before it on the same knob.
pub fn knob(s: &mut crate::session::Session, id: KnobId, wheel: bool) {
    // The piano roll tool dialogs' knobs only preview; their Accept is the step (spec: ui/piano-strum.md §1).
    if !s.settings.check(&prefs::UNDO_KNOB_TWEAKS)
        || matches!(id, KnobId::Strum(_) | KnobId::Arp(_) | KnobId::PianoDlg(_) | KnobId::NoteProp(_))
    {
        return;
    }
    let h = &s.history;
    if wheel && h.joins(Run::Wheel(id)) {
        return;
    }
    // What the knob's command changes, as it is before the drag (`KnobId::command`); none when what it names is gone.
    crate::control::settle_in(s);
    let Some(undo) = id.command(s, 0.0).and_then(|c| c.undo_now(s).ok()) else { return };
    record(s, knob_name(id), Snap::Commands(vec![undo]));
    if wheel {
        s.history.run = Some(Run::Wheel(id));
    }
}

fn swap_step(s: &mut crate::session::Session, i: usize) {
    if let Snap::Commands(v) = &mut s.history.steps[i].snap {
        let v = std::mem::take(v);
        let other = crate::control::take_back_in(s, v);
        s.history.steps[i].snap = Snap::Commands(other);
        s.dirty = true;
        // The interface puts back the rack's view it kept for the step, if it kept one.
        s.touched.rack_step = Some(s.history.steps[i].id);
        return;
    }
    let now = s.history.steps[i].snap.now(s);
    let old = std::mem::replace(&mut s.history.steps[i].snap, now);
    old.put(s);
}

/// `undo` in the session.
pub fn undo_in(s: &mut crate::session::Session) -> Option<String> {
    let h = &mut s.history;
    if h.done == 0 {
        return None;
    }
    h.done -= 1;
    (h.undid, h.run, h.fresh) = (true, None, false);
    h.changes += 1;
    let i = h.done;
    swap_step(s, i);
    Some(s.history.steps[i].name.to_string())
}

/// `redo` in the session.
pub fn redo_in(s: &mut crate::session::Session) -> Option<String> {
    let h = &mut s.history;
    if h.done == h.steps.len() {
        return None;
    }
    let i = h.done;
    swap_step(s, i);
    let h = &mut s.history;
    h.done += 1;
    (h.undid, h.run, h.fresh) = (false, None, false);
    h.changes += 1;
    Some(h.steps[i].name.to_string())
}

/// "Step seq edit" as a menu entry writes it after Undo / Redo: "step seq edit".
pub fn lower(name: &str) -> String {
    let mut c = name.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

/// The EDIT menu's entries (spec: ui/menus.md "EDIT"): the first says what Ctrl+Z does next ("Undo step seq edit",
/// "Redo step seq edit" with Alternate undo mode off after an undo), `redo` the second (Ctrl+Alt+Z's redo, Alternate
/// undo mode only). Without a step to act on, plain "Undo" / "Redo".
pub fn edit_label(s: &crate::session::Session, redo_entry: bool) -> String {
    let h = &s.history;
    let redo_it = redo_entry || h.ctrl_z_redoes(&s.settings);
    let (verb, step) = if redo_it { ("Redo", h.steps.get(h.done)) } else { ("Undo", h.done.checked_sub(1).and_then(|i| h.steps.get(i))) };
    match step {
        Some(st) => format!("{verb} {}", lower(&st.name)),
        None => verb.into(),
    }
}

/// Whether EDIT's entry has a step to act on.
pub fn edit_enabled(s: &crate::session::Session, redo_entry: bool) -> bool {
    let h = &s.history;
    if redo_entry || h.ctrl_z_redoes(&s.settings) { h.done < h.steps.len() } else { h.done > 0 }
}

/// Whether EDIT shows its Redo entry: Alternate undo mode on and a history to act on (only Undo while it is empty).
pub fn redo_shown(s: &crate::session::Session) -> bool {
    s.settings.check(&prefs::ALTERNATE_UNDO) && !s.history.steps.is_empty()
}

/// Maximum undo levels changed: the history keeps that many.
pub fn trim(s: &mut crate::session::Session) {
    s.history.trim(&s.settings);
}

/// Another arrangement is shown: the playlist's steps belong to the one left (spec: ui/playlist.md §10).
pub fn forget_playlist(s: &mut crate::session::Session) {
    let h = &mut s.history;
    let mut done = 0;
    let mut kept = Vec::with_capacity(h.steps.len());
    for (i, st) in std::mem::take(&mut h.steps).into_iter().enumerate() {
        if !st.snap.has_playlist() {
            done += usize::from(i < h.done);
            kept.push(st);
        }
    }
    (h.steps, h.done, h.run) = (kept, done, None);
}

/// The timebase changed from `from` to `to` ticks per quarter: the steps' notes, control points, lengths, clips and
/// markers follow, so no undo brings back ticks of the old timebase (a timebase step keeps its own, `Timebase`).
pub fn rescale(s: &mut crate::session::Session, from: u16, to: u16) {
    for st in &mut s.history.steps {
        st.snap.rescale(from, to);
    }
}

impl Snap {
    /// Pattern `number` as it is now.
    pub fn pattern(s: &crate::session::Session, number: u16) -> Snap {
        let p = s.project.pattern(number);
        Snap::Pattern {
            number,
            notes: p.map(|p| p.notes.clone()).unwrap_or_default(),
            controls: p.map(|p| p.controls.clone()).unwrap_or_default(),
            length: p.and_then(|p| p.length),
            // Its loops and loop mode (FL's loop edits are steps of this kind, `rack::loop_step`).
            loops: p.map_or((Vec::new(), None), |p| (p.loops.clone(), p.loop_mode)),
        }
    }

    /// What this covers, as the project has it now.
    fn now(&self, s: &crate::session::Session) -> Snap {
        match self {
            Snap::Pattern { number, .. } => Snap::pattern(s, *number),
            Snap::Playlist(_) => Snap::Playlist(Box::new(crate::edit::playlist::capture(s))),
            Snap::Several(v) => Snap::Several(v.iter().map(|x| x.now(s)).collect()),
            // What each command would put back, as it is now (`swap` applies them).
            Snap::Commands(v) => Snap::Commands(v.iter().map(|c| c.now(s)).collect()),
            Snap::Timebase(_) => Snap::Timebase(Box::new(Timebase::now(s))),
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
            Snap::Several(v) => v.iter_mut().for_each(|x| x.rescale(from, to)),
            Snap::Commands(v) => v.iter_mut().for_each(|c| c.rescale(from, to)),
            // A timebase step's ticks are at its own timebase.
            Snap::Timebase(_) => {}
        }
    }

    /// Puts this into the project. A pattern's step leaves the pattern shown as it is, as in FL (24.1.1, 2026-11-09: a
    /// step sequencer and a piano roll edit in one pattern, undone and redone while another was shown, kept that one).
    fn put(self, s: &mut crate::session::Session) {
        match self {
            Snap::Pattern { number, notes, controls, length, loops } => {
                let p = crate::edit::patterns::pat_mut(s, number);
                (p.notes, p.controls, p.length) = (notes, controls, length);
                (p.loops, p.loop_mode) = loops;
                s.touched.notes = true;
            }
            Snap::Playlist(p) => crate::edit::playlist::restore(s, *p),
            Snap::Several(v) => v.into_iter().rev().for_each(|x| x.put(s)),
            Snap::Commands(v) => {
                crate::control::take_back_in(s, v);
            }
            Snap::Timebase(t) => {
                let Timebase { ppq, patterns, arrangements } = *t;
                let from = s.project.ppq.max(1);
                crate::projects::retime_in(s, from, ppq.max(1));
                (s.project.get_mut().patterns, s.project.get_mut().arrangements) = (patterns, arrangements);
                // Playback stops on the next tick, as for the change (`App::set_timebase`).
                s.retimed = true;
            }
        }
        s.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;


    fn entry(label: &str, shortcut: &'static str, enabled: bool) -> (String, &'static str, bool) {
        (label.into(), shortcut, enabled)
    }

    /// A fader's value in dB.
    fn db(v: f64) -> f64 {
        20.0 * os_engine::laws::fader_gain(v).log10()
    }

    /// Put undone recordings in the recycle bin: stored and shown, on by default; Lemon Studio does not record yet
    /// (spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn recycle_bin_check_is_kept() {
        let st = crate::settings_file::SettingsStore::memory();
        assert!(st.check(&prefs::UNDONE_RECORDINGS_TO_BIN));
        assert!(!st.toggle(&prefs::UNDONE_RECORDINGS_TO_BIN));
        assert!(!st.check(&prefs::UNDONE_RECORDINGS_TO_BIN));
        st.set_check(&prefs::UNDONE_RECORDINGS_TO_BIN, true);
    }
}
