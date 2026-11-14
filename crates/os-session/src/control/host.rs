// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Hosted instances follow their channels and slots by id. The plugin host keys instances by rack row and mixer
//! (track, slot); an edit that moves or removes channels or slots without telling the host (an undo that puts back
//! an older project, a command) leaves them under the wrong key. `App::follow_ids` puts them right on the next tick:
//! an instance whose channel or slot moved goes with it, one whose channel or slot is gone goes (its state kept in
//! `Plugins::stash`), and a channel or slot that comes back is hosted again from the kept state.

use std::collections::HashMap;

use os_model::Id;

use crate::plugins::{fx_key, fx_of, FX_KEY};

/// Where the session's owned instances belong now, and the channels and slots back from an undo (`App::follow_ids`).
pub struct FollowPlan {
    rows: HashMap<u32, usize>,
    slots: HashMap<u32, (u16, u8)>,
    pub row_map: HashMap<usize, Option<usize>>,
    fx_map: HashMap<(u16, u8), Option<(u16, u8)>>,
    rows_too: bool,
    fx_too: bool,
}

pub fn follow_plan(ses: &crate::session::Session, rows_too: bool, fx_too: bool) -> FollowPlan {
    let p = &ses.project;
    let rows: HashMap<u32, usize> = p.channels.iter().enumerate().map(|(r, c)| (c.id.0, r)).collect();
    let slots: HashMap<u32, (u16, u8)> = p
        .mixer
        .iter()
        .flat_map(|m| m.slots.iter().enumerate().filter_map(move |(k, s)| s.as_ref().map(|s| (s.id.0, (m.index, k as u8)))))
        .collect();

    // Where each owned instance belongs now.
    let mut row_map: HashMap<usize, Option<usize>> = HashMap::new();
    let mut fx_map: HashMap<(u16, u8), Option<(u16, u8)>> = HashMap::new();
    for (&key, h) in &ses.plugins.hosted {
        if h.owner.is_none() {
            continue;
        }
        if key < FX_KEY {
            if !rows_too {
                continue;
            }
            match rows.get(&h.owner.0) {
                Some(&r) if r == key => {}
                to => {
                    row_map.insert(key, to.copied());
                }
            }
        } else if let Some(at) = fx_of(key).filter(|_| fx_too) {
            match slots.get(&h.owner.0) {
                Some(&to) if to == at => {}
                to => {
                    fx_map.insert(at, to.copied());
                }
            }
        }
    }
    FollowPlan { rows, slots, row_map, fx_map, rows_too, fx_too }
}

/// `follow_plan` carried out: instances moved to where their channels and slots are, those back from an undo hosted
/// again from their kept state.
pub fn follow_apply(ses: &mut crate::session::Session, plan: FollowPlan) {
    let FollowPlan { rows, slots, row_map, fx_map, rows_too, fx_too } = plan;
    if !row_map.is_empty() {
        let m = |r: usize| row_map.get(&r).copied().unwrap_or(Some(r));
        crate::project_plugins::remap_rows_in(ses, &m);
    }
    if rows_too {
        // The rows hosted are the instances' rows: an edit may have moved or dropped rows of the list without
        // moving the instances (a command's delete, taken back before a tick).
        ses.hosted_rows = ses.plugins.hosted.range(..FX_KEY).map(|(&r, _)| r).collect();
    }
    if !fx_map.is_empty() {
        ses.plugins.remap_fx(&|t, k| fx_map.get(&(t, k)).copied().unwrap_or(Some((t, k))));
        ses.transport.set_effects(ses.plugins.effects());
        ses.dirty = true;
    }

    // Instances without an owner (just hosted) take the id of what is under their key; a container's effect, the id
    // made of its container's slot's and its own (`member_owner`), once the slot has one.
    let p = &ses.project;
    let row_id = |r: usize| p.channels.get(r).map(|c| c.id);
    let slot_id = |t: u16, k: u8| p.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots.get(usize::from(k))?.as_ref()).map(|s| s.id);
    let session = &*ses;
    let member_id = |key: usize| session.plugin_members.get(&key).and_then(|&(t, k, id)| crate::control::command::member_owner(crate::edit::container::place_id(session, t, k)?.0, id)).map(Id);
    let owners: Vec<(usize, Id)> = session
        .plugins
        .hosted
        .iter()
        .filter(|(_, h)| h.owner.is_none())
        .map(|(&key, _)| {
            let id = match fx_of(key) {
                None if key >= crate::plugins::MEMBER_KEY => member_id(key),
                None => row_id(key),
                Some((t, k)) => slot_id(t, k),
            };
            (key, id.unwrap_or(Id::NONE))
        })
        .collect();
    for (key, id) in owners {
        if let Some(h) = ses.plugins.hosted.get_mut(&key) {
            h.owner = id;
        }
    }

    // Channels and slots back from an undo: hosted again from the state kept when they went.
    if !(rows_too || fx_too) || ses.plugins.stash.is_empty() {
        return;
    }
    let back: Vec<(usize, Id, bool, String, Vec<u8>)> = ses.plugins
        .stash
        .iter()
        .filter(|e| if e.1 { fx_too } else { rows_too })
        .filter_map(|(id, fx, uid, state)| {
            let key = if *fx { slots.get(&id.0).map(|&(t, k)| fx_key(t, k)) } else { rows.get(&id.0).copied() }?;
            (!ses.plugins.hosted.contains_key(&key)).then(|| (key, *id, *fx, uid.clone(), state.clone()))
        })
        .collect();
    for (key, id, fx, uid, state) in back {
        ses.plugins.stash.retain(|e| e.0 .0 != id.0);
        let made = match fx_of(key) {
            Some((t, k)) if fx => ses.plugins.effect_index_of_uid(&uid).is_some_and(|i| ses.plugins.create_effect(t, k, i, Some(&state)).is_ok()),
            None if !fx => match ses.plugins.index_of_uid(&uid) {
                Some(i) => {
                    crate::project_plugins::plugin_paste_in(ses, key, i, &state);
                    ses.plugins.hosted.contains_key(&key)
                }
                None => false,
            },
            _ => false,
        };
        if let Some(h) = ses.plugins.hosted.get_mut(&key).filter(|_| made) {
            h.owner = id;
        }
        if made && fx {
            ses.transport.set_effects(ses.plugins.effects());
        }
        ses.dirty = true;
    }
}

/// The plugins commands added (`HostReq`) are hosted, and the parameter values they set reach their instances.
/// Unlike the plugin menus, nothing opens a window: the user didn't ask for one.
pub fn host_commands(ses: &mut crate::session::Session) {
    let reqs = std::mem::take(&mut ses.control.host);
    let mut relink = (false, false);
    for r in reqs {
        let p = &ses.project;
        match r {
            crate::control::HostReq::Instrument { channel, plugin } => {
                let Some(row) = p.channels.iter().position(|c| c.id.0 == channel.0) else { continue };
                if ses.plugins.hosted.contains_key(&row) {
                    continue;
                }
                let Some(index) = ses.plugins.index_of(&plugin) else {
                    ses.touched.views.push(crate::session::View::Message(format!("Plugin not found: {plugin}")));
                    continue;
                };
                match ses.plugins.create(row, index) {
                    Ok(_) => {
                        ses.hosted_rows.push(row);
                        relink.0 = true;
                    }
                    Err(e) => ses.touched.views.push(crate::session::View::Message(format!("Plugin: {e}"))),
                }
            }
            crate::control::HostReq::Effect { slot, plugin } => {
                let Some((t, k)) = p.mixer.iter().find_map(|m| {
                    let k = m.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.id.0 == slot.0))?;
                    Some((m.index, k as u8))
                }) else {
                    continue;
                };
                let Some(index) = ses.plugins.effect_index(&plugin) else {
                    ses.touched.views.push(crate::session::View::Message(format!("Effect not found: {plugin}")));
                    continue;
                };
                match ses.plugins.create_effect(t, k, index, None) {
                    Ok(_) => relink.1 = true,
                    Err(e) => ses.touched.views.push(crate::session::View::Message(format!("Effect: {e}"))),
                }
            }
        }
    }
    // Owners for what was just hosted.
    if relink.0 || relink.1 {
        follow_apply(ses, follow_plan(&*ses, false, false));
    }
    for row in std::mem::take(&mut ses.control.misc) {
        crate::misc::follow_in(ses, row);
    }
    for (owner, param, v) in std::mem::take(&mut ses.control.params) {
        let Some(key) = crate::control::command::owner_key(&*ses, owner) else { continue };
        let set = ses.plugins.hosted.get(&key).and_then(|h| h.plugin.lock().ok()).is_some_and(|mut p| p.set_param(param, v));
        if set {
            ses.plugins.mark_edited(key);
            ses.params.set_by_host(key, param, v);
        }
    }
    // Plugin states an undo or redo put back (`Taken::Plugin`): the state and preset each one replaces become the
    // other side of its step (or of a redo still in this list), then it goes in. A side never filled (its plugin
    // was not hosted) puts nothing back.
    let mut states = std::mem::take(&mut ses.control.states);
    for i in 0..states.len() {
        let (owner, fill) = (states[i].0, states[i].2);
        let Some(key) = crate::control::command::owner_key(&*ses, owner).filter(|k| ses.plugins.hosted.contains_key(k)) else { continue };
        let (now, preset) = (ses.plugins.state(key).unwrap_or_default(), ses.plugins.presets.get(&key).cloned());
        match states[i + 1..].iter_mut().find(|e| e.1.fill == fill) {
            Some((_, side, _)) => (side.state, side.preset, side.fill) = (now, preset, 0),
            None => crate::history::fill_plugin_side(ses, fill, &now, preset.as_deref()),
        }
        let side = &states[i].1;
        if side.fill != 0 || side.state.is_empty() || !ses.plugins.set_state(key, &side.state) {
            continue;
        }
        ses.plugins.set_preset(key, side.preset.clone());
        // The parameters as the state left them: what the window's next edit and a parameter's undo move from.
        ses.params.reread(key);
        for info in ses.plugins.param_infos(key) {
            if let Some(v) = ses.plugins.param_value(key, info.id) {
                ses.control.saw_param(owner, info.id, v);
            }
        }
        ses.touched.repaint = true;
    }
    if relink.0 {
        ses.transport.set_instruments(ses.plugins.instruments());
    }
    if relink.1 {
        ses.transport.set_effects(ses.plugins.effects());
    }
    if relink.0 || relink.1 {
        ses.dirty = true;
        ses.touched.repaint = true;
    }
}

/// Samples loaded into channels (`Session::sample_loads`) reach their hosted Lemon Sampler, with the root note and
/// stretch settings the load may have read from the file (`rack::read_tags`).
pub fn sample_loads(ses: &mut crate::session::Session) {
    for (row, path) in std::mem::take(&mut ses.sample_loads) {
        if !ses.hosted_rows.contains(&row) {
            continue;
        }
        let set = ses.project.channels.get(row).map(crate::plugins::tag_params).unwrap_or_default();
        // A row hosted this tick already started from its channel's sample.
        let new = ses.plugins.sampler_path(row).as_deref() != Some(path.as_str());
        if !ses.plugins.set_sampler(row, new.then_some(path.as_str()), &set) {
            ses.touched.views.push(crate::session::View::Message(format!("Lemon Sampler did not take {path}")));
            ses.touched.repaint = true;
        }
    }
}

/// The plugin the user picked for a channel (`Session::pending_plugin`) is hosted, once it is ready; until then the
/// index of the plugin waited for (a native generator waits for its first check in a child process,
/// `Plugins::instrument_ready`).
pub fn pending_plugin(ses: &mut crate::session::Session) -> Option<usize> {
    let (row, index) = ses.pending_plugin?;
    if !ses.plugins.instrument_ready(index) {
        return Some(index);
    }
    ses.pending_plugin = None;
    let msg = match ses.plugins.create(row, index) {
        Ok(name) => {
            ses.plugins.added(row);
            ses.hosted_rows.push(row);
            ses.transport.set_instruments(ses.plugins.instruments());
            ses.dirty = true;
            let format = ses.plugins.descriptor(row).map_or("", |d| crate::plugins::format_name(d.format));
            ses.pending_open = Some(row);
            format!("Added {name} (hosted {format} plugin)")
        }
        Err(e) => format!("Plugin: {e}"),
    };
    ses.touched.views.push(crate::session::View::Message(msg));
    ses.touched.repaint = true;
    None
}
