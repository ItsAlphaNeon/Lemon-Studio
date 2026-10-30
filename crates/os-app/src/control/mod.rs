// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The control plane (spec: roadmap/control-plane.md): one way to change a project from outside the interface
//! (scripts, tests, controllers, AI assistants) with undo built in.
//!
//! - `ids`: stable ids for the things commands name.
//! - `command`: the commands, each undoable.
//! - Here: `apply`, the one entry point. A list of commands is one transaction: one named step in the undo history,
//!   applied whole or not at all. A proposal applies a list to see what it changes and takes it back at once; the
//!   user (or the client) accepts or discards it later. Every applied list is an event clients can read.
//! - `host`: hosted plugin instances follow their channels and slots by id.
//! - `query`: the project as clients read it.
//! - `rpc`: JSON-RPC 2.0 over a local socket (`server`), the adapter clients speak.

pub mod command;
pub mod host;
pub mod ids;
pub mod query;
pub mod rpc;
pub mod server;

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};

use serde::Serialize;

use crate::app::State;
pub use command::Command;

/// What a client may do (spec: roadmap/control-plane.md "Safety model").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Read the project and its events.
    Read,
    /// Propose changes; the user accepts or discards them.
    Propose,
    /// Change the project (every change undoable).
    Edit,
}

impl Level {
    pub fn parse(t: &str) -> Option<Level> {
        match t {
            "read" => Some(Level::Read),
            "propose" => Some(Level::Propose),
            "edit" => Some(Level::Edit),
            _ => None,
        }
    }
}

/// Who changes the project: the user (the interface), or a client by the name it gave (the undo history names its
/// steps after it: "Assistant: build drum kit").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum By {
    User,
    Client(String),
}

impl By {
    fn label(&self) -> String {
        match self {
            By::User => "user".into(),
            By::Client(n) => n.clone(),
        }
    }
}

/// A plugin a command added, for the app to host on its next tick (it knows the plugins).
#[derive(Debug, Clone, PartialEq)]
pub enum HostReq {
    Instrument { channel: os_model::Id, plugin: String },
    Effect { slot: os_model::Id, plugin: String },
}

/// One applied change (a transaction, an undo, a redo, an edit in the interface), as clients read it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    pub seq: u64,
    pub by: String,
    /// "apply", "undo", "redo", "edit" (an edit in the interface), "proposal", "accepted", "discarded", "opened".
    pub kind: &'static str,
    /// The step's name, the proposal's name.
    pub name: String,
    /// The commands' ops.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ops: Vec<&'static str>,
}

/// A change waiting for the user (or the client) to accept or discard it.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub n: u64,
    pub by: By,
    pub name: String,
    pub commands: Vec<Command>,
    pub changes: Vec<query::Change>,
}

/// How many events are kept for clients to read.
const EVENTS_KEPT: usize = 1000;

/// The control plane's part of the app state (`State::control`).
#[derive(Debug, Default)]
pub struct Control {
    pub ids: ids::Ids,
    /// Plugins commands added, hosted by the app on its next tick.
    pub host: Vec<HostReq>,
    /// Parameter values commands set, for the app to pass on: (owner, parameter, value).
    pub params: Vec<(u32, u32, f64)>,
    /// Rack rows whose Misc values an undo put back, for their Lemon Sampler to follow (`misc::follow`).
    pub misc: Vec<usize>,
    /// Plugin states an undo or redo put back, for the app to load: (owner, the side put back, the `PluginSide::fill`
    /// of its other side, which takes the state it replaces).
    pub states: Vec<(u32, command::PluginSide, u64)>,
    /// The last `PluginSide::fill` given.
    fills: u64,
    /// The values parameters were last set to or read at (the undo of a parameter change puts the value back).
    known: HashMap<(u32, u32), f64>,
    events: VecDeque<Event>,
    seq: u64,
    pub proposals: Vec<Proposal>,
    proposal_seq: u64,
    /// The project (and tempo) as the last command left it or the last tick saw it: a tick that finds it changed
    /// otherwise tells clients of an edit in the interface (`App::tick`). Kept only while `watched`.
    seen: Option<(os_model::Project, f64)>,
    /// A client may be connected (the app listens, `--control` / `--serve`): edits in the interface become events.
    pub watched: bool,
    /// How many things the project had at the last `settle` (`settle_tick`).
    counted: [usize; 6],
}

impl Control {
    /// Gives ids to new things (`ids::settle`).
    pub fn settle(&mut self, p: &mut os_model::Project) -> bool {
        self.counted = count(p);
        ids::settle(&mut self.ids, p)
    }

    /// `settle` on the app's tick: when the project was edited (`dirty`) or holds another number of things (one was
    /// added or copied), else nothing to do.
    pub fn settle_tick(&mut self, p: &mut os_model::Project, dirty: bool) {
        if dirty || count(p) != self.counted {
            self.settle(p);
        }
    }

    pub fn set_param(&mut self, owner: u32, param: u32, v: f64) {
        self.params.push((owner, param, v));
        self.known.insert((owner, param), v);
    }

    /// Plugin `owner`'s state goes back to `side` on the app's next tick; returns the `PluginSide::fill` for the side it
    /// replaces.
    pub fn put_state(&mut self, owner: u32, side: command::PluginSide) -> u64 {
        self.fills += 1;
        self.states.push((owner, side, self.fills));
        self.fills
    }

    /// A parameter's value as last set or read.
    pub fn param_value(&self, owner: u32, param: u32) -> Option<f64> {
        self.known.get(&(owner, param)).copied()
    }

    /// The app read a parameter's value.
    pub fn saw_param(&mut self, owner: u32, param: u32, v: f64) {
        self.known.insert((owner, param), v);
    }

    pub fn event(&mut self, by: &By, kind: &'static str, name: impl Into<String>, ops: Vec<&'static str>) {
        self.seq += 1;
        self.events.push_back(Event { seq: self.seq, by: by.label(), kind, name: name.into(), ops });
        while self.events.len() > EVENTS_KEPT {
            self.events.pop_front();
        }
    }

    /// The events after `since` (0: all kept), and the newest's number.
    pub fn events_since(&self, since: u64) -> (Vec<Event>, u64) {
        (self.events.iter().filter(|e| e.seq > since).cloned().collect(), self.seq)
    }

    /// The project as it is now is known to clients (a command's event told them).
    pub fn saw(&mut self, p: &os_model::Project, tempo: f64) {
        if self.watched {
            self.seen = Some((p.clone(), tempo));
        }
    }

    /// Whether the project changed since `saw` (and notes it as seen). Always false while no one watches.
    pub fn changed_since_seen(&mut self, p: &os_model::Project, tempo: f64) -> bool {
        if !self.watched || self.seen.as_ref().is_some_and(|(q, t)| q == p && *t == tempo) {
            return false;
        }
        let first = self.seen.is_none();
        self.saw(p, tempo);
        !first
    }

    /// The project was replaced (opened, made): its proposals go, ids start again from where they are.
    pub fn reset(&mut self) {
        ids::forget(&mut self.ids);
        self.host.clear();
        self.params.clear();
        self.states.clear();
        self.known.clear();
        self.proposals.clear();
        self.seen = None;
    }
}

/// The things `settle` gives ids to, counted.
fn count(p: &os_model::Project) -> [usize; 6] {
    let clips = p.arrangements.iter().map(|a| a.clips.len()).sum();
    let slots = p.mixer.iter().map(|m| m.slots.iter().flatten().count()).sum();
    [p.channels.len(), p.patterns.len(), p.mixer.len(), p.arrangements.len(), clips, slots]
}

fn op(c: &Command) -> &'static str {
    match c {
        Command::SetTempo { .. } => "set_tempo",
        Command::SetSwing { .. } => "set_swing",
        Command::SetMainVolume { .. } => "set_main_volume",
        Command::SetMainPitch { .. } => "set_main_pitch",
        Command::SetTimeSignature { .. } => "set_time_signature",
        Command::SetInfo { .. } => "set_info",
        Command::AddChannel { .. } => "add_channel",
        Command::DeleteChannel { .. } => "delete_channel",
        Command::MoveChannel { .. } => "move_channel",
        Command::SetChannel { .. } => "set_channel",
        Command::LoadSample { .. } => "load_sample",
        Command::AddTrack { .. } => "add_track",
        Command::DeleteTrack { .. } => "delete_track",
        Command::SetTrack { .. } => "set_track",
        Command::SetSend { .. } => "set_send",
        Command::SetEq { .. } => "set_eq",
        Command::AddEffect { .. } => "add_effect",
        Command::RemoveEffect { .. } => "remove_effect",
        Command::SetSlot { .. } => "set_slot",
        Command::AddPattern { .. } => "add_pattern",
        Command::DeletePattern { .. } => "delete_pattern",
        Command::SetPattern { .. } => "set_pattern",
        Command::SetNotes { .. } => "set_notes",
        Command::AddNotes { .. } => "add_notes",
        Command::AddClip { .. } => "add_clip",
        Command::DeleteClip { .. } => "delete_clip",
        Command::SetClip { .. } => "set_clip",
        Command::SetPlaylistTrack { .. } => "set_playlist_track",
        Command::SetParam { .. } => "set_param",
        Command::Put(_) => "put",
    }
}

/// Applies `commands` in order, all or none; returns each one's result and the commands that undo them all (in the
/// order to apply them). Records nothing.
pub fn run(s: &mut State, commands: Vec<Command>) -> Result<(Vec<serde_json::Value>, Vec<Command>), String> {
    s.control.settle(&mut s.project);
    let queues = Queues::of(s);
    let known = s.control.known.clone();
    let mut undo = Vec::with_capacity(commands.len());
    let mut results = Vec::with_capacity(commands.len());
    for (i, c) in commands.into_iter().enumerate() {
        match c.apply(s) {
            Ok(done) => {
                undo.push(done.undo);
                results.push(done.result);
                s.control.settle(&mut s.project);
            }
            Err(e) => {
                // Back to where the list started.
                undo.reverse();
                take_back(s, undo);
                queues.put(s);
                s.control.known = known;
                return Err(format!("command {i}: {e}"));
            }
        }
    }
    undo.reverse();
    Ok((results, undo))
}

/// Applies undo commands (from `run`, in their order), returning the commands that redo them (in theirs). An undo
/// that fails (the project was changed under it in a way it can't follow) stops there.
pub fn take_back(s: &mut State, undo: Vec<Command>) -> Vec<Command> {
    let mut redo = Vec::with_capacity(undo.len());
    for c in undo {
        match c.apply(s) {
            Ok(d) => redo.push(d.undo),
            Err(e) => {
                s.message = Some(format!("Undo: {e}"));
                break;
            }
        }
        s.control.settle(&mut s.project);
    }
    redo.reverse();
    redo
}

/// The one entry point: applies `commands` as one transaction (see `run`) and records it as one step of the undo
/// history, named `name` (else the single command's name), after the client for a client's changes.
pub fn apply(s: &mut State, by: &By, name: Option<&str>, commands: Vec<Command>) -> Result<Vec<serde_json::Value>, String> {
    if commands.is_empty() {
        return Ok(Vec::new());
    }
    let ops: Vec<&'static str> = commands.iter().map(op).collect();
    let step = name.map(str::to_string).unwrap_or_else(|| if commands.len() == 1 { commands[0].name().to_string() } else { "Edit".into() });
    let (results, undo) = run(s, commands)?;
    let label: Cow<'static, str> = match by {
        By::User => step.clone().into(),
        By::Client(n) => format!("{n}: {}", lower(&step)).into(),
    };
    crate::history::record_named(s, label, crate::history::Snap::Commands(undo));
    crate::history::done_with(s);
    s.control.saw(&s.project, s.tempo);
    s.control.event(by, "apply", step, ops);
    Ok(results)
}

/// An edit in the interface: `region` is taken, `f` changes it, and the change is one undo step named `name` (none
/// when nothing changed). The interface's edits thus have the commands' undo: what was taken is put back by
/// `Command::Put`, as a command's own undo is.
pub fn edit<R>(s: &mut State, name: &'static str, region: command::Region, f: impl FnOnce(&mut State) -> R) -> R {
    let before = command::Taken::of(s, region);
    let r = f(s);
    if let Some(before) = before {
        if command::Taken::of(s, region).as_ref() != Some(&before) {
            record_edit(s, name, before);
        }
    }
    r
}

/// `edit` for a knob-like control (a mixer fader's typed value): no step while Undo knob tweaks is off.
pub fn knob_edit<R>(s: &mut State, name: &'static str, region: command::Region, f: impl FnOnce(&mut State) -> R) -> R {
    if crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS) {
        edit(s, name, region, f)
    } else {
        f(s)
    }
}

/// An edit of `region` that joins the step of run `run` just before it (a knob dragged, a box's wheel notches): its
/// first change records the step holding `region` as it was, the others go into it. `knob`: a knob-like control, no
/// step while Undo knob tweaks is off.
pub fn run_edit<R>(s: &mut State, name: &'static str, region: command::Region, run: crate::history::Run, knob: bool, f: impl FnOnce(&mut State) -> R) -> R {
    if (knob && !crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS)) || s.history.joins(run) {
        return f(s);
    }
    let before = command::Taken::of(s, region);
    let r = f(s);
    if let Some(before) = before {
        if command::Taken::of(s, region).as_ref() != Some(&before) {
            record_edit(s, name, before);
            s.history.set_run(run);
        }
    }
    r
}

/// Records an interface edit's step. A step of the same press that changed nothing goes first (a text box's playlist
/// step when the box named a mixer track), as the press's release would have dropped it.
fn record_edit(s: &mut State, name: &'static str, before: command::Taken) {
    crate::history::settle(s);
    crate::history::record(s, name, crate::history::Snap::Commands(vec![Command::Put(Box::new(before))]));
    crate::history::done_with(s);
}

/// `edit` for an edit that needs the app (a mixer menu command hosting or dropping plugins).
pub fn edit_app<R>(app: &mut crate::app::App, name: &'static str, region: command::Region, f: impl FnOnce(&mut crate::app::App) -> R) -> R {
    let before = command::Taken::of(&app.state, region);
    let r = f(app);
    let s = &mut app.state;
    if let Some(before) = before {
        if command::Taken::of(s, region).as_ref() != Some(&before) {
            record_edit(s, name, before);
        }
    }
    r
}

/// Parameters of plugin `owner` moved in its own window, each from its value `before`: one step, unless Undo knob
/// tweaks is off. `run`: values the plugin sent outside a gesture (no begin and end of the edit), which join the step
/// of the plugin's values just before them, as a knob's wheel notches do; a parameter not in that step yet joins it
/// with its value before. The step comes from no press of the app's, so no release looks at it.
pub fn param_edit(s: &mut State, owner: u32, before: &[(u32, f64)], run: bool) {
    if !crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS) {
        return;
    }
    let put = |&(param, v): &(u32, f64)| Command::Put(Box::new(command::Taken::Param(owner, param, v)));
    let r = crate::history::Run::Param(owner);
    if run && s.history.joins(r) {
        crate::history::extend_newest(s, |cmds| {
            for b in before {
                let has = cmds.iter().any(|c| matches!(c, Command::Put(t) if matches!(**t, command::Taken::Param(_, p, _) if p == b.0)));
                if !has {
                    cmds.push(put(b));
                }
            }
        });
        return;
    }
    crate::history::record(s, command::PLUGIN_PARAMETER, crate::history::Snap::Commands(before.iter().map(put).collect()));
    crate::history::done_with(s);
    if run {
        s.history.set_run(r);
    }
}

/// Lemon Studio set parameter `index` of plugin instance `key` from `before` to `to` (Type in value, the parameter
/// menu's Set... and Paste value): one step, "Plugin parameter", as a client's `set_param` makes, unless Undo knob
/// tweaks is off or the value stays. The step comes from no change the release could judge: a plugin may take the
/// value on its next idle.
pub fn param_set(s: &mut State, key: usize, index: usize, before: Option<f64>, to: f64) {
    let Some(param) = s.params.lists.get(&key).and_then(|l| l.params.get(index)).map(|e| e.id) else { return };
    // The owner's id (one hosted since the last settle has none yet).
    if command::key_owner(&s.project, key) == Some(0) {
        s.control.settle(&mut s.project);
    }
    let Some(owner) = command::key_owner(&s.project, key) else { return };
    // Known for the undo's other side (`Command::apply`).
    s.control.saw_param(owner, param, to);
    let Some(v) = before.filter(|&v| v != to && crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS)) else { return };
    let put = Command::Put(Box::new(command::Taken::Param(owner, param, v)));
    crate::history::record(s, command::PLUGIN_PARAMETER, crate::history::Snap::Commands(vec![put]));
    crate::history::done_with(s);
}

/// Plugin instance `key` took a whole new state at the user's request (a preset from its window's Presets box, ◀ ▶ or
/// window menu, a preset file dropped on it): one step, `name`, that puts `before` back (the state, the preset shown
/// and its channel's or slot's name), keyed by the owner's id so it follows moves. The step comes from no change the
/// release could judge.
pub fn state_edit(s: &mut State, key: usize, name: &'static str, before: command::PluginSide) {
    if command::key_owner(&s.project, key) == Some(0) {
        s.control.settle(&mut s.project);
    }
    let Some(owner) = command::key_owner(&s.project, key).filter(|&o| o != 0) else { return };
    crate::history::record(s, name, crate::history::Snap::Commands(vec![Command::Put(Box::new(command::Taken::Plugin(owner, Box::new(before))))]));
    crate::history::done_with(s);
}

/// `edit_app` for a run of changes that make one step (the tempo tapper's taps, wheel notches on a value box): a change
/// that joins the newest step of run `run` (`History::joins`) records none, as that step holds the region from before
/// the run.
pub fn edit_run<R>(app: &mut crate::app::App, name: &'static str, region: command::Region, run: crate::history::Run, f: impl FnOnce(&mut crate::app::App) -> R) -> R {
    if app.state.history.joins(run) {
        return f(app);
    }
    let before = command::Taken::of(&app.state, region);
    let r = f(app);
    let s = &mut app.state;
    if let Some(before) = before {
        if command::Taken::of(s, region).as_ref() != Some(&before) {
            record_edit(s, name, before);
            s.history.set_run(run);
        }
    }
    r
}

/// A press that starts an edit of `region` (a fader drag): the step named `name` is recorded now, holding the region
/// as it is, and goes again at the release when the drag changed nothing (`history::settle`).
pub fn edit_press(s: &mut State, name: &'static str, region: command::Region) {
    if let Some(before) = command::Taken::of(s, region) {
        crate::history::record(s, name, crate::history::Snap::Commands(vec![Command::Put(Box::new(before))]));
    }
}

/// "Build Drum Kit" in a client's step name keeps its case; only the first letter goes lower ("Tempo" > "tempo").
fn lower(t: &str) -> String {
    let mut c = t.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

/// What the app's queues and the interface's selections were before commands that are taken back at once (a dry
/// run, a transaction that fails), so they are left as they were: no plugin is dropped and hosted again, no
/// selection moves.
struct Queues {
    host: usize,
    params: usize,
    states: usize,
    sample_loads: usize,
    fx: usize,
    dirty: bool,
    message: Option<String>,
    hosted_rows: Vec<usize>,
    selected_channel: usize,
    rack_sel: std::collections::BTreeSet<usize>,
    selected_mixer: u16,
    mixer_sel: Vec<u16>,
    windows: Vec<crate::app::Win>,
    plugin_channel: usize,
    clips: Vec<usize>,
    piano: crate::piano::PianoState,
}

impl Queues {
    fn of(s: &State) -> Queues {
        Queues {
            host: s.control.host.len(),
            params: s.control.params.len(),
            states: s.control.states.len(),
            sample_loads: s.sample_loads.len(),
            fx: s.mixer.pending.len(),
            dirty: s.piano.dirty,
            message: s.message.clone(),
            hosted_rows: s.hosted_rows.clone(),
            selected_channel: s.selected_channel,
            rack_sel: s.rack.sel.clone(),
            selected_mixer: s.selected_mixer,
            mixer_sel: s.mixer.sel.clone(),
            windows: s.z.clone(),
            plugin_channel: s.plugin_channel,
            clips: s.playlist.sel.clone(),
            piano: s.piano.clone(),
        }
    }

    fn put(self, s: &mut State) {
        s.control.host.truncate(self.host);
        s.control.params.truncate(self.params);
        s.control.states.truncate(self.states);
        s.sample_loads.truncate(self.sample_loads);
        s.mixer.pending.truncate(self.fx);
        s.message = self.message;
        s.hosted_rows = self.hosted_rows;
        (s.selected_channel, s.rack.sel, s.selected_mixer, s.mixer.sel) = (self.selected_channel, self.rack_sel, self.selected_mixer, self.mixer_sel);
        (s.z, s.plugin_channel, s.playlist.sel) = (self.windows, self.plugin_channel, self.clips);
        s.piano = self.piano;
        s.piano.dirty = self.dirty;
    }
}

/// Applies `commands` to see what they change and takes them back at once: the project is as it was. Returns the
/// changes; an error when the list can't be applied.
pub fn dry_run(s: &mut State, commands: Vec<Command>) -> Result<Vec<query::Change>, String> {
    let queues = Queues::of(s);
    let known = s.control.known.clone();
    let before = query::project(s);
    let (_, undo) = run(s, commands)?;
    let after = query::project(s);
    take_back(s, undo);
    queues.put(s);
    s.control.known = known;
    Ok(query::changes(&before, &after))
}

/// A proposal: `commands` checked by a dry run and kept for `accept` or `discard`. Returns its number and changes.
pub fn propose(s: &mut State, by: &By, name: &str, commands: Vec<Command>) -> Result<(u64, Vec<query::Change>), String> {
    let changes = dry_run(s, commands.clone())?;
    let c = &mut s.control;
    c.proposal_seq += 1;
    let n = c.proposal_seq;
    c.proposals.push(Proposal { n, by: by.clone(), name: name.to_string(), commands, changes: changes.clone() });
    c.event(by, "proposal", name, Vec::new());
    s.message = Some(format!("{} proposes: {name}", by.label()));
    Ok((n, changes))
}

/// Accepts proposal `n`: applied as one step named after it and its client.
pub fn accept(s: &mut State, n: u64) -> Result<Vec<serde_json::Value>, String> {
    let i = s.control.proposals.iter().position(|p| p.n == n).ok_or_else(|| format!("no proposal {n}"))?;
    let p = s.control.proposals.remove(i);
    match apply(s, &p.by, Some(&p.name), p.commands.clone()) {
        Ok(r) => {
            s.control.event(&p.by, "accepted", p.name, Vec::new());
            Ok(r)
        }
        // The project changed since it was proposed: it waits on, for the client to discard or the user to see why.
        Err(e) => {
            s.control.proposals.insert(i, p);
            Err(e)
        }
    }
}

/// Discards proposal `n`.
pub fn discard(s: &mut State, n: u64) -> Result<(), String> {
    let i = s.control.proposals.iter().position(|p| p.n == n).ok_or_else(|| format!("no proposal {n}"))?;
    let p = s.control.proposals.remove(i);
    s.control.event(&p.by, "discarded", p.name, Vec::new());
    Ok(())
}

#[cfg(test)]
mod tests;
