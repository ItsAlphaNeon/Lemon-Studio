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

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
pub mod command;
pub mod host;
pub mod ids;
pub mod query;
pub mod removal;
pub mod rpc;
pub mod server;

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};

use serde::Serialize;

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
    counted: [usize; 11],
    /// Clients' gestures under way (`begin_in`): their number and the name of their step.
    gestures: std::collections::BTreeMap<u64, Option<String>>,
    gesture_seq: u64,
}

impl Control {
    /// A new id for a note a command makes (it names it in its result; `ids::Ids::part`).
    pub fn fresh_note(&mut self) -> os_model::Id {
        self.ids.part()
    }

    /// Gives ids to new things (`ids::settle`).
    pub fn settle(&mut self, p: &mut os_model::Project) -> bool {
        self.counted = count(p);
        ids::settle(&mut self.ids, p)
    }

    /// `settle` on the app's tick: when the project was edited (`dirty`) or holds another number of things (one was
    /// added or copied), else nothing to do.
    pub fn settle_tick(&mut self, p: &mut crate::locked::Locked, dirty: bool) {
        if dirty || count(p) != self.counted {
            p.write_if(|p| self.settle(p));
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
        self.gestures.clear();
        self.seen = None;
    }
}

/// The things `settle` gives ids to, counted.
fn count(p: &os_model::Project) -> [usize; 11] {
    let arr = |f: fn(&os_model::Arrangement) -> usize| p.arrangements.iter().map(f).sum();
    let pat = |f: fn(&os_model::Pattern) -> usize| p.patterns.iter().map(f).sum();
    let slots = p.mixer.iter().map(|m| m.slots.iter().flatten().count()).sum();
    [
        p.channels.len(),
        p.patterns.len(),
        p.mixer.len(),
        p.arrangements.len(),
        arr(|a| a.clips.len()),
        slots,
        pat(|q| q.notes.len()),
        pat(|q| q.controls.len()),
        arr(|a| a.markers.len()),
        arr(|a| a.tracks.len()),
        p.automations.iter().map(|a| a.points.len()).sum(),
    ]
}

fn op(c: &Command) -> &'static str {
    match c {
        Command::SetTempo { .. } => "set_tempo",
        Command::SetSwing { .. } => "set_swing",
        Command::SetMainVolume { .. } => "set_main_volume",
        Command::SetMainPitch { .. } => "set_main_pitch",
        Command::SetTimeSignature { .. } => "set_time_signature",
        Command::SetInfo { .. } => "set_info",
        Command::SetProject { .. } => "set_project",
        Command::AddChannel { .. } => "add_channel",
        Command::CloneChannel { .. } => "clone_channel",
        Command::ReplaceInstrument { .. } => "replace_instrument",
        Command::DeleteChannel { .. } => "delete_channel",
        Command::MoveChannel { .. } => "move_channel",
        Command::SetChannel { .. } => "set_channel",
        Command::LoadSample { .. } => "load_sample",
        Command::AddTrack { .. } => "add_track",
        Command::DeleteTrack { .. } => "delete_track",
        Command::SetTrack { .. } => "set_track",
        Command::ResetTrack { .. } => "reset_track",
        Command::RemoveTracks { .. } => "remove_tracks",
        Command::MoveTrack { .. } => "move_track",
        Command::LoadTrackState { .. } => "load_track_state",
        Command::SetAutoBypass { .. } => "set_auto_bypass",
        Command::SetSend { .. } => "set_send",
        Command::SetEq { .. } => "set_eq",
        Command::AddEffect { .. } => "add_effect",
        Command::RemoveEffect { .. } => "remove_effect",
        Command::MoveEffect { .. } => "move_effect",
        Command::SetSlot { .. } => "set_slot",
        Command::AddPattern { .. } => "add_pattern",
        Command::ShiftPatterns { .. } => "shift_patterns",
        Command::MovePattern { .. } => "move_pattern",
        Command::ClonePattern { .. } => "clone_pattern",
        Command::TransposePattern { .. } => "transpose_pattern",
        Command::SplitPattern { .. } => "split_pattern",
        Command::SetPatternSignature { .. } => "set_pattern_signature",
        Command::DeletePattern { .. } => "delete_pattern",
        Command::SetPattern { .. } => "set_pattern",
        Command::SetNotes { .. } => "set_notes",
        Command::AddNotes { .. } => "add_notes",
        Command::SetStep { .. } => "set_step",
        Command::SetLoops { .. } => "set_loops",
        Command::SetGroups { .. } => "set_groups",
        Command::AddClip { .. } => "add_clip",
        Command::DeleteClip { .. } => "delete_clip",
        Command::SetClip { .. } => "set_clip",
        Command::SetPlaylistTrack { .. } => "set_playlist_track",
        Command::SetPoints { .. } => "set_points",
        Command::AddArrangement { .. } => "add_arrangement",
        Command::DeleteArrangement { .. } => "delete_arrangement",
        Command::SetArrangement { .. } => "set_arrangement",
        Command::SetClips { .. } => "set_clips",
        Command::SetMarkers { .. } => "set_markers",
        Command::AddMarker { .. } => "add_marker",
        Command::SetMarker { .. } => "set_marker",
        Command::DeleteMarker { .. } => "delete_marker",
        Command::MoveContent { .. } => "move_content",
        Command::SwapPlaylistTracks { .. } => "swap_playlist_tracks",
        Command::InsertPlaylistTrack { .. } => "insert_playlist_track",
        Command::DeletePlaylistTrack { .. } => "delete_playlist_track",
        Command::ResetPlaylistTrack { .. } => "reset_playlist_track",
        Command::SetClipLfo { .. } => "set_clip_lfo",
        Command::SetMidiOut { .. } => "set_midi_out",
        Command::SetMember { .. } => "set_member",
        Command::WrapEffects { .. } => "wrap_effects",
        Command::WrapInstrument { .. } => "wrap_instrument",
        Command::MoveMember { .. } => "move_member",
        Command::RemoveMember { .. } => "remove_member",
        Command::AddMember { .. } => "add_member",
        Command::ReplaceMember { .. } => "replace_member",
        Command::SetParam { .. } => "set_param",
        Command::Put(_) => "put",
    }
}

/// Gives ids to what has none (`Control::settle`), so commands can name it.
pub fn settle_in(s: &mut crate::session::Session) {
    s.control.settle(s.project.get_mut());
}

/// `run` on the session: what the views are to follow is reported (`Session::touched`); a list that fails reports
/// nothing.
pub fn run_in(s: &mut crate::session::Session, commands: Vec<Command>) -> Result<(Vec<serde_json::Value>, Vec<Command>), String> {
    s.control.settle(s.project.get_mut());
    let queues = Queues::of(s);
    let known = s.control.known.clone();
    let mut undo = Vec::with_capacity(commands.len());
    let mut results = Vec::with_capacity(commands.len());
    for (i, c) in commands.into_iter().enumerate() {
        let on_off = c.on_off_undo(s);
        s.link_moves.clear();
        match c.apply(s) {
            Ok(done) => {
                // The remote control links the command moved with what they drive (`remote::follow`) go back after
                // it is undone.
                let links: Vec<_> = std::mem::take(&mut s.link_moves).into_iter().map(|(from, to)| (to, from)).collect();
                if !links.is_empty() {
                    undo.push(Command::Put(Box::new(command::Taken::Links(links))));
                }
                undo.push(done.undo);
                undo.extend(on_off);
                results.push(done.result);
                s.control.settle(s.project.get_mut());
            }
            Err(e) => {
                // Back to where the list started.
                undo.reverse();
                take_back_in(s, undo);
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
pub fn take_back_in(s: &mut crate::session::Session, undo: Vec<Command>) -> Vec<Command> {
    let mut redo = Vec::with_capacity(undo.len());
    for c in undo {
        match c.apply(s) {
            Ok(d) => redo.push(d.undo),
            Err(e) => {
                s.touched.views.push(crate::session::View::Message(format!("Undo: {e}")));
                break;
            }
        }
        s.control.settle(s.project.get_mut());
    }
    redo.reverse();
    redo
}

/// `apply` on the session.
pub fn apply_in(s: &mut crate::session::Session, by: &By, name: Option<&str>, commands: Vec<Command>) -> Result<Vec<serde_json::Value>, String> {
    if commands.is_empty() {
        return Ok(Vec::new());
    }
    let ops: Vec<&'static str> = commands.iter().map(op).collect();
    let step = name.map(str::to_string).unwrap_or_else(|| if commands.len() == 1 { commands[0].name().to_string() } else { "Edit".into() });
    let (results, undo) = run_in(s, commands)?;
    let label: Cow<'static, str> = match by {
        By::User => step.clone().into(),
        By::Client(n) => format!("{n}: {}", lower(&step)).into(),
    };
    crate::history::record_named(s, label, crate::history::Snap::Commands(undo));
    crate::history::done_with(s);
    let tempo = s.tempo();
    s.control.saw(&s.project, tempo);
    s.control.event(by, "apply", step, ops);
    Ok(results)
}

/// Starts a gesture: a change a client streams as it is made (a fader dragged), commands applied in it
/// (`apply_run_in`) making one undo step, named `name` (by default its first commands' name), until `end_in`. Returns
/// its number.
pub fn begin_in(s: &mut crate::session::Session, name: Option<&str>) -> u64 {
    let c = &mut s.control;
    c.gesture_seq += 1;
    c.gestures.insert(c.gesture_seq, name.map(str::to_string));
    c.gesture_seq
}

/// Applies `commands` in gesture `n` (`begin_in`): they join the gesture's step while it is the newest, keeping what
/// each region was before the gesture; after any other step (or an undo) they make a new step that the gesture goes on
/// in.
pub fn apply_run_in(s: &mut crate::session::Session, by: &By, n: u64, commands: Vec<Command>) -> Result<Vec<serde_json::Value>, String> {
    let name = s.control.gestures.get(&n).ok_or_else(|| format!("no gesture {n}"))?.clone();
    if commands.is_empty() {
        return Ok(Vec::new());
    }
    let r = crate::history::Run::Gesture(n);
    if !s.history.joins(r) {
        let results = apply_in(s, by, name.as_deref(), commands)?;
        s.history.set_run(r);
        return Ok(results);
    }
    let ops: Vec<&'static str> = commands.iter().map(op).collect();
    let (results, undo) = run_in(s, commands)?;
    crate::history::extend_newest(s, |held| join_undo(held, undo));
    let tempo = s.tempo();
    s.control.saw(&s.project, tempo);
    let step = s.history.newest_done().unwrap_or_default().to_string();
    s.control.event(by, "apply", step, ops);
    Ok(results)
}

/// Ends gesture `n`: the next command is a step of its own.
pub fn end_in(s: &mut crate::session::Session, n: u64) -> Result<(), String> {
    s.control.gestures.remove(&n).ok_or_else(|| format!("no gesture {n}"))?;
    s.history.end_run_of(crate::history::Run::Gesture(n));
    Ok(())
}

/// A gesture step's undo commands `held` with the newer `undo` before them: a region the step already puts back is
/// not taken again (the step puts it back as it was before the gesture), so a long drag keeps one of each.
fn join_undo(held: &mut Vec<Command>, undo: Vec<Command>) {
    let keys: std::collections::HashSet<_> = held.iter().filter_map(region_key).collect();
    let mut v: Vec<Command> = undo.into_iter().filter(|c| region_key(c).is_none_or(|k| !keys.contains(&k))).collect();
    v.append(held);
    *held = v;
}

/// What an undo command puts back, for `join_undo`: two with the same key put back the same thing. `None`: one that
/// is kept whatever else the step holds (a whole project, links, an add's undo).
fn region_key(c: &Command) -> Option<(u8, u32, u32)> {
    use command::Taken;
    let Command::Put(t) = c else { return None };
    Some(match &**t {
        Taken::Settings(_) => (0, 0, 0),
        Taken::Channel(c) => (1, c.id.0, 0),
        Taken::Track(m) => (2, m.id.0, 0),
        Taken::Pattern(p) => (3, p.id.0, 0),
        Taken::Lfo(id, _) => (10, *id, 0),
        Taken::Arrangement(a) => (4, a.id.0, 0),
        Taken::Param(owner, param, _) => (5, *owner, *param),
        Taken::Plugin(owner, _) => (6, *owner, 0),
        Taken::Mutes(_) => (7, 0, 0),
        Taken::Points(ch, _) => (14, *ch, 0),
        Taken::Locks(_) => (11, 0, 0),
        Taken::Zips(_) => (12, 0, 0),
        Taken::Groups(_) => (13, 0, 0),
        Taken::TracksOn(_) => (9, 0, 0),
        Taken::Row(id, _) => (8, *id, 0),
        Taken::Project(_) | Taken::Links(_) | Taken::Gone(_) | Taken::Back(_) | Taken::Removed(_) | Taken::Deleted(..) | Taken::WithOn(..) | Taken::Swap(..) => return None,
    })
}

/// Records an interface edit's step. A step of the same press that changed nothing goes first (a text box's playlist
/// step when the box named a mixer track), as the press's release would have dropped it.
pub fn record_edit(s: &mut crate::session::Session, name: &'static str, before: command::Taken) {
    let mut undo = vec![Command::Put(Box::new(before))];
    // The remote control links the edit moved with what they drive (`remote::follow`) go back with it.
    let links: Vec<_> = std::mem::take(&mut s.link_moves).into_iter().map(|(from, to)| (to, from)).collect();
    if !links.is_empty() {
        undo.push(Command::Put(Box::new(command::Taken::Links(links))));
    }
    crate::history::record(s, name, crate::history::Snap::Commands(undo));
    crate::history::done_with(s);
}

/// Parameters of plugin `owner` moved in its own window, each from its value `before`: one step, unless Undo knob
/// tweaks is off. `run`: values the plugin sent outside a gesture (no begin and end of the edit), which join the step
/// of the plugin's values just before them, as a knob's wheel notches do; a parameter not in that step yet joins it
/// with its value before. The step comes from no press of the app's, so no release looks at it.
pub fn param_edit(s: &mut crate::session::Session, owner: u32, before: &[(u32, f64)], run: bool) {
    s.plugin_edited = true;
    if !s.settings.check(&crate::prefs::UNDO_KNOB_TWEAKS) {
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

/// Plugin instance `key` took a whole new state at the user's request (a preset from its window's Presets box, ◀ ▶ or
/// window menu, a preset file dropped on it): one step, `name`, that puts `before` back (the state, the preset shown
/// and its channel's or slot's name), keyed by the owner's id so it follows moves. The step comes from no change the
/// release could judge.
pub fn state_edit(s: &mut crate::session::Session, key: usize, name: &'static str, before: command::PluginSide) {
    if command::key_owner(s, key) == Some(0) {
        s.control.settle(s.project.get_mut());
    }
    let Some(owner) = command::key_owner(s, key).filter(|&o| o != 0) else { return };
    crate::history::record(s, name, crate::history::Snap::Commands(vec![Command::Put(Box::new(command::Taken::Plugin(owner, Box::new(before))))]));
    crate::history::done_with(s);
}

/// A press that starts an edit of `region` (a fader drag): the step named `name` is recorded now, holding the region
/// as it is, and goes again at the release when the drag changed nothing (`history::settle`).
pub fn edit_press(s: &mut crate::session::Session, name: &'static str, region: command::Region) {
    edit_press_all(s, name, [region]);
}

/// `edit_press` for an edit of several regions (a drag on the fader of a selected track moves them all): one step.
pub fn edit_press_all(s: &mut crate::session::Session, name: &'static str, regions: impl IntoIterator<Item = command::Region>) {
    let before: Vec<Command> = regions.into_iter().filter_map(|r| command::Taken::of(s, r)).map(|t| Command::Put(Box::new(t))).collect();
    if !before.is_empty() {
        crate::history::record(s, name, crate::history::Snap::Commands(before));
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
    hosted_rows: Vec<usize>,
    written: Option<&'static std::panic::Location<'static>>,
    /// What the views were told before: what a failed list (or a dry run) told them goes.
    touched: crate::session::Touched,
}

impl Queues {
    fn of(s: &crate::session::Session) -> Queues {
        Queues {
            host: s.control.host.len(),
            params: s.control.params.len(),
            states: s.control.states.len(),
            sample_loads: s.sample_loads.len(),
            fx: s.fx_requests.len(),
            dirty: s.dirty,
            written: s.project.written_at(),
            hosted_rows: s.hosted_rows.clone(),
            touched: s.touched.clone(),
        }
    }

    pub fn put(self, s: &mut crate::session::Session) {
        s.control.host.truncate(self.host);
        s.control.params.truncate(self.params);
        s.control.states.truncate(self.states);
        s.sample_loads.truncate(self.sample_loads);
        s.fx_requests.truncate(self.fx);
        s.hosted_rows = self.hosted_rows;
        s.dirty = self.dirty;
        s.project.rewind_written(self.written);
        s.touched = self.touched;
    }
}

/// `dry_run` on the session; `selected` is the channel the interface has selected (`query::project_in`). The views are
/// told nothing.
pub fn dry_run_in(s: &mut crate::session::Session, commands: Vec<Command>, selected: usize) -> Result<Vec<query::Change>, String> {
    let queues = Queues::of(s);
    let known = s.control.known.clone();
    let before = query::project_in(s, selected);
    let (_, undo) = run_in(s, commands)?;
    let after = query::project_in(s, selected);
    take_back_in(s, undo);
    queues.put(s);
    s.control.known = known;
    Ok(query::changes(&before, &after))
}

/// `propose` on the session.
pub fn propose_in(s: &mut crate::session::Session, by: &By, name: &str, commands: Vec<Command>, selected: usize) -> Result<(u64, Vec<query::Change>), String> {
    let changes = dry_run_in(s, commands.clone(), selected)?;
    let c = &mut s.control;
    c.proposal_seq += 1;
    let n = c.proposal_seq;
    c.proposals.push(Proposal { n, by: by.clone(), name: name.to_string(), commands, changes: changes.clone() });
    c.event(by, "proposal", name, Vec::new());
    s.touched.views.push(crate::session::View::Message(format!("{} proposes: {name}", by.label())));
    Ok((n, changes))
}

/// `accept` on the session.
pub fn accept_in(s: &mut crate::session::Session, n: u64) -> Result<Vec<serde_json::Value>, String> {
    let i = s.control.proposals.iter().position(|p| p.n == n).ok_or_else(|| format!("no proposal {n}"))?;
    let p = s.control.proposals.remove(i);
    match apply_in(s, &p.by, Some(&p.name), p.commands.clone()) {
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
pub fn discard(s: &mut crate::session::Session, n: u64) -> Result<(), String> {
    let i = s.control.proposals.iter().position(|p| p.n == n).ok_or_else(|| format!("no proposal {n}"))?;
    let p = s.control.proposals.remove(i);
    s.control.event(&p.by, "discarded", p.name, Vec::new());
    Ok(())
}

