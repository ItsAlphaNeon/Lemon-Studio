// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::control` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use serde::Serialize;
use crate::app::State;
pub use command::Command;
#[allow(unused_imports)]
use crate::control::*;

/// Applies `commands` in order, all or none; returns each one's result and the commands that undo them all (in the
/// order to apply them). Records nothing.
#[cfg_attr(not(test), allow(dead_code))] // the interface's own, used by tests
pub fn run(s: &mut State, commands: Vec<Command>) -> Result<(Vec<serde_json::Value>, Vec<Command>), String> {
    crate::follow::around(s, |s| run_in(&mut s.session, commands))
}

/// The one entry point: applies `commands` as one transaction (see `run`) and records it as one step of the undo
/// history, named `name` (else the single command's name), after the client for a client's changes.
#[cfg_attr(not(test), allow(dead_code))] // the interface's own, used by tests
pub fn apply(s: &mut State, by: &By, name: Option<&str>, commands: Vec<Command>) -> Result<Vec<serde_json::Value>, String> {
    crate::follow::around(s, |s| apply_in(&mut s.session, by, name, commands))
}

/// An edit in the interface: `region` is taken, `f` changes it, and the change is one undo step named `name` (none
/// when nothing changed). The interface's edits thus have the commands' undo: what was taken is put back by
/// `Command::Put`, as a command's own undo is.
pub fn edit<R>(s: &mut State, name: &'static str, region: command::Region, f: impl FnOnce(&mut State) -> R) -> R {
    s.session.link_moves.clear();
    // A pressed step that changed nothing goes before this edit, judged on the project as it is now: after the edit
    // its region may name other things (tracks renumbered by a delete).
    crate::history::settle(&mut s.session);
    let before = command::Taken::of(&s.session, region);
    let r = f(s);
    if let Some(before) = before {
        if command::Taken::of(&s.session, region).as_ref() != Some(&before) {
            record_edit(&mut s.session, name, before);
        }
    }
    r
}

/// An edit in the interface made of `commands`: applied as one undo step named `name` holding their undo, none when
/// they changed nothing. With `run`, the edit joins the step of that run just before it (wheel notches on a value box,
/// the tempo tapper's taps): the first change records the step, the others go into it. An error when the list can't
/// be applied (nothing changed then).
pub fn command(s: &mut State, name: &'static str, run: Option<crate::history::Run>, commands: Vec<Command>) -> Result<Vec<serde_json::Value>, String> {
    if run.is_some_and(|r| s.session.history.joins(r)) {
        let (results, _) = crate::follow::around(s, |s| run_in(&mut s.session, commands))?;
        s.session.dirty = true;
        return Ok(results);
    }
    s.session.link_moves.clear();
    crate::history::settle(&mut s.session);
    let taken = |s: &State, commands: &[Command]| commands.iter().map(|c| c.undo_now(&s.session).ok()).collect::<Vec<_>>();
    let before = taken(s, &commands);
    let adds = commands.iter().any(Command::adds);
    let (results, undo) = crate::follow::around(s, |s| run_in(&mut s.session, commands.clone()))?;
    if adds || taken(s, &commands) != before {
        s.session.dirty = true;
        crate::history::record(&mut s.session, name, crate::history::Snap::Commands(undo));
        crate::history::done_with(&mut s.session);
        if let Some(r) = run {
            s.session.history.set_run(r);
        }
    }
    Ok(results)
}

/// `command` in two parts: `first`, then the commands `then` makes from first's results (the ids of what it added),
/// as one undo step named `name`. An error takes both back.
pub fn command_then(s: &mut State, name: &'static str, first: Vec<Command>, then: impl FnOnce(&State, &[serde_json::Value]) -> Vec<Command>) -> Result<Vec<serde_json::Value>, String> {
    s.session.link_moves.clear();
    crate::history::settle(&mut s.session);
    let (mut results, mut undo) = crate::follow::around(s, |s| run_in(&mut s.session, first))?;
    let more = then(s, &results);
    match crate::follow::around(s, |s| run_in(&mut s.session, more)) {
        Ok((r, mut u)) => {
            results.extend(r);
            u.append(&mut undo);
            undo = u;
        }
        Err(e) => {
            let _ = crate::follow::around(s, |s| take_back_in(&mut s.session, undo));
            return Err(e);
        }
    }
    s.session.dirty = true;
    crate::history::record(&mut s.session, name, crate::history::Snap::Commands(undo));
    crate::history::done_with(&mut s.session);
    Ok(results)
}

/// Every thing in the project has its id (things just added get theirs on the next tick): for an edit that names
/// them in commands.
pub fn settle_ids(s: &mut State) {
    crate::control::settle_in(&mut s.session);
}

/// `command` for a knob-like control (a mute light, a typed fader value): applied without a step while Undo knob tweaks
/// is off.
pub fn knob_command(s: &mut State, name: &'static str, run: Option<crate::history::Run>, commands: Vec<Command>) -> Result<Vec<serde_json::Value>, String> {
    if crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS) {
        return command(s, name, run, commands);
    }
    let before: Vec<_> = commands.iter().map(|c| c.undo_now(&s.session).ok()).collect();
    let check = commands.clone();
    let (results, _) = crate::follow::around(s, |s| run_in(&mut s.session, commands))?;
    if check.iter().map(|c| c.undo_now(&s.session).ok()).collect::<Vec<_>>() != before {
        s.session.dirty = true;
    }
    Ok(results)
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
    if (knob && !crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS)) || s.session.history.joins(run) {
        return f(s);
    }
    s.session.link_moves.clear();
    // A pressed step that changed nothing goes before this edit, judged on the project as it is now: after the edit
    // its region may name other things (tracks renumbered by a delete).
    crate::history::settle(&mut s.session);
    let before = command::Taken::of(&s.session, region);
    let r = f(s);
    if let Some(before) = before {
        if command::Taken::of(&s.session, region).as_ref() != Some(&before) {
            record_edit(&mut s.session, name, before);
            s.session.history.set_run(run);
        }
    }
    r
}

/// `edit` for an edit that needs the app (a mixer menu command hosting or dropping plugins).
pub fn edit_app<R>(app: &mut crate::app::App, name: &'static str, region: command::Region, f: impl FnOnce(&mut crate::app::App) -> R) -> R {
    app.state.session.link_moves.clear();
    crate::history::settle(&mut app.state.session);
    let before = command::Taken::of(&app.state.session, region);
    let r = f(app);
    let s = &mut app.state;
    if let Some(before) = before {
        if command::Taken::of(&s.session, region).as_ref() != Some(&before) {
            record_edit(&mut s.session, name, before);
        }
    }
    r
}

/// Lemon Studio set parameter `index` of plugin instance `key` from `before` to `to` (Type in value, the parameter
/// menu's Set... and Paste value): one step, "Plugin parameter", as a client's `set_param` makes, unless Undo knob
/// tweaks is off or the value stays. The step comes from no change the release could judge: a plugin may take the
/// value on its next idle.
pub fn param_set(s: &mut State, key: usize, index: usize, before: Option<f64>, to: f64) {
    let Some(param) = s.session.params.lists.get(&key).and_then(|l| l.params.get(index)).map(|e| e.id) else { return };
    // The owner's id (one hosted since the last settle has none yet).
    if command::key_owner(&s.session, key) == Some(0) {
        crate::control::settle_in(&mut s.session);
    }
    let Some(owner) = command::key_owner(&s.session, key) else { return };
    // Known for the undo's other side (`Command::apply`).
    s.session.control.saw_param(owner, param, to);
    s.session.plugin_edited |= before.is_some_and(|v| v != to);
    let Some(v) = before.filter(|&v| v != to && crate::prefs::check(&crate::prefs::UNDO_KNOB_TWEAKS)) else { return };
    let put = Command::Put(Box::new(command::Taken::Param(owner, param, v)));
    crate::history::record(&mut s.session, command::PLUGIN_PARAMETER, crate::history::Snap::Commands(vec![put]));
    crate::history::done_with(&mut s.session);
}

/// Applies `commands` to see what they change and takes them back at once: the project is as it was. Returns the
/// changes; an error when the list can't be applied.
#[cfg_attr(not(test), allow(dead_code))] // the interface's own, used by tests
pub fn dry_run(s: &mut State, commands: Vec<Command>) -> Result<Vec<query::Change>, String> {
    let selected = s.selected_channel;
    dry_run_in(&mut s.session, commands, selected)
}

/// A proposal: `commands` checked by a dry run and kept for `accept` or `discard`. Returns its number and changes.
#[cfg_attr(not(test), allow(dead_code))] // the interface's own, used by tests
pub fn propose(s: &mut State, by: &By, name: &str, commands: Vec<Command>) -> Result<(u64, Vec<query::Change>), String> {
    let selected = s.selected_channel;
    crate::follow::around(s, |s| propose_in(&mut s.session, by, name, commands, selected))
}

/// Accepts proposal `n`: applied as one step named after it and its client.
pub fn accept(s: &mut State, n: u64) -> Result<Vec<serde_json::Value>, String> {
    crate::follow::around(s, |s| accept_in(&mut s.session, n))
}
