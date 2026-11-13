// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::control` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
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
        s.session.control.settle(s.session.project.get_mut());
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

/// `edit_app` for a run of changes that make one step (the tempo tapper's taps, wheel notches on a value box): a change
/// that joins the newest step of run `run` (`History::joins`) records none, as that step holds the region from before
/// the run.
pub fn edit_run<R>(app: &mut crate::app::App, name: &'static str, region: command::Region, run: crate::history::Run, f: impl FnOnce(&mut crate::app::App) -> R) -> R {
    if app.state.session.history.joins(run) {
        return f(app);
    }
    app.state.session.link_moves.clear();
    crate::history::settle(&mut app.state.session);
    let before = command::Taken::of(&app.state.session, region);
    let r = f(app);
    let s = &mut app.state;
    if let Some(before) = before {
        if command::Taken::of(&s.session, region).as_ref() != Some(&before) {
            record_edit(&mut s.session, name, before);
            s.session.history.set_run(run);
        }
    }
    r
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
