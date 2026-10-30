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

use crate::app::App;
use crate::plugins::{fx_key, fx_of, FX_KEY};

impl App {
    /// See the module. `rows_too`: rack instances and the channels that came back (before anything looks at rows on the
    /// tick, so a sampler comes back from its kept state, not from the channel's settings); `fx_too`: mixer slot
    /// instances (after the mixer's own requests moved them, `mixer_tick`) and the slots that came back.
    pub(crate) fn follow_ids(&mut self, rows_too: bool, fx_too: bool) {
        let p = &self.state.project;
        let rows: HashMap<u32, usize> = p.channels.iter().enumerate().map(|(r, c)| (c.id.0, r)).collect();
        let slots: HashMap<u32, (u16, u8)> = p
            .mixer
            .iter()
            .flat_map(|m| m.slots.iter().enumerate().filter_map(move |(k, s)| s.as_ref().map(|s| (s.id.0, (m.index, k as u8)))))
            .collect();

        // Where each owned instance belongs now.
        let mut row_map: HashMap<usize, Option<usize>> = HashMap::new();
        let mut fx_map: HashMap<(u16, u8), Option<(u16, u8)>> = HashMap::new();
        for (&key, h) in &self.plugins.hosted {
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
        if !row_map.is_empty() {
            let m = |r: usize| row_map.get(&r).copied().unwrap_or(Some(r));
            self.remap_rows(&m);
        }
        if rows_too {
            // The rows hosted are the instances' rows: an edit may have moved or dropped rows of the list without
            // moving the instances (a command's delete, taken back before a tick).
            self.state.hosted_rows = self.plugins.hosted.range(..FX_KEY).map(|(&r, _)| r).collect();
        }
        if !fx_map.is_empty() {
            self.plugins.remap_fx(&|t, k| fx_map.get(&(t, k)).copied().unwrap_or(Some((t, k))));
            self.transport.set_effects(self.plugins.effects());
            self.state.piano.dirty = true;
        }

        // Instances without an owner (just hosted) take the id of what is under their key.
        let p = &self.state.project;
        let row_id = |r: usize| p.channels.get(r).map(|c| c.id);
        let slot_id = |t: u16, k: u8| p.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots.get(usize::from(k))?.as_ref()).map(|s| s.id);
        for (&key, h) in self.plugins.hosted.iter_mut() {
            if h.owner.is_none() {
                h.owner = match fx_of(key) {
                    None => row_id(key),
                    Some((t, k)) => slot_id(t, k),
                }
                .unwrap_or(Id::NONE);
            }
        }

        // Channels and slots back from an undo: hosted again from the state kept when they went.
        if !(rows_too || fx_too) || self.plugins.stash.is_empty() {
            return;
        }
        let back: Vec<(usize, Id, bool, String, Vec<u8>)> = self
            .plugins
            .stash
            .iter()
            .filter(|e| if e.1 { fx_too } else { rows_too })
            .filter_map(|(id, fx, uid, state)| {
                let key = if *fx { slots.get(&id.0).map(|&(t, k)| fx_key(t, k)) } else { rows.get(&id.0).copied() }?;
                (!self.plugins.hosted.contains_key(&key)).then(|| (key, *id, *fx, uid.clone(), state.clone()))
            })
            .collect();
        for (key, id, fx, uid, state) in back {
            self.plugins.stash.retain(|e| e.0 .0 != id.0);
            let made = match fx_of(key) {
                Some((t, k)) if fx => self.plugins.effect_index_of_uid(&uid).is_some_and(|i| self.plugins.create_effect(t, k, i, Some(&state)).is_ok()),
                None if !fx => match self.plugins.index_of_uid(&uid) {
                    Some(i) => {
                        self.plugin_paste(key, i, &state);
                        self.plugins.hosted.contains_key(&key)
                    }
                    None => false,
                },
                _ => false,
            };
            if let Some(h) = self.plugins.hosted.get_mut(&key).filter(|_| made) {
                h.owner = id;
            }
            if made && fx {
                self.transport.set_effects(self.plugins.effects());
            }
            self.state.piano.dirty = true;
        }
    }
}

impl App {
    /// The plugins commands added (`HostReq`) are hosted, and the parameter values they set reach their instances.
    /// Unlike the plugin menus, nothing opens a window: the user didn't ask for one.
    pub(crate) fn host_commands(&mut self) {
        let reqs = std::mem::take(&mut self.state.control.host);
        let mut relink = (false, false);
        for r in reqs {
            let p = &self.state.project;
            match r {
                crate::control::HostReq::Instrument { channel, plugin } => {
                    let Some(row) = p.channels.iter().position(|c| c.id.0 == channel.0) else { continue };
                    if self.plugins.hosted.contains_key(&row) {
                        continue;
                    }
                    let Some(index) = self.plugins.index_of(&plugin) else {
                        self.state.message = Some(format!("Plugin not found: {plugin}"));
                        continue;
                    };
                    match self.plugins.create(row, index) {
                        Ok(_) => {
                            self.state.hosted_rows.push(row);
                            relink.0 = true;
                        }
                        Err(e) => self.state.message = Some(format!("Plugin: {e}")),
                    }
                }
                crate::control::HostReq::Effect { slot, plugin } => {
                    let Some((t, k)) = p.mixer.iter().find_map(|m| {
                        let k = m.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.id.0 == slot.0))?;
                        Some((m.index, k as u8))
                    }) else {
                        continue;
                    };
                    let Some(index) = self.plugins.effect_index(&plugin) else {
                        self.state.message = Some(format!("Effect not found: {plugin}"));
                        continue;
                    };
                    match self.plugins.create_effect(t, k, index, None) {
                        Ok(_) => relink.1 = true,
                        Err(e) => self.state.message = Some(format!("Effect: {e}")),
                    }
                }
            }
        }
        // Owners for what was just hosted.
        if relink.0 || relink.1 {
            self.follow_ids(false, false);
        }
        for row in std::mem::take(&mut self.state.control.misc) {
            crate::misc::follow(self, row);
        }
        for (owner, param, v) in std::mem::take(&mut self.state.control.params) {
            let Some(key) = crate::control::command::owner_key(&self.state.project, owner) else { continue };
            let set = self.plugins.hosted.get(&key).and_then(|h| h.plugin.lock().ok()).is_some_and(|mut p| p.set_param(param, v));
            if set {
                self.plugins.mark_edited(key);
                self.state.params.set_by_host(key, param, v);
            }
        }
        // Plugin states an undo or redo put back (`Taken::Plugin`): the state and preset each one replaces become the
        // other side of its step (or of a redo still in this list), then it goes in. A side never filled (its plugin
        // was not hosted) puts nothing back.
        let mut states = std::mem::take(&mut self.state.control.states);
        for i in 0..states.len() {
            let (owner, fill) = (states[i].0, states[i].2);
            let Some(key) = crate::control::command::owner_key(&self.state.project, owner).filter(|k| self.plugins.hosted.contains_key(k)) else { continue };
            let (now, preset) = (self.plugins.state(key).unwrap_or_default(), self.plugins.presets.get(&key).cloned());
            match states[i + 1..].iter_mut().find(|e| e.1.fill == fill) {
                Some((_, side, _)) => (side.state, side.preset, side.fill) = (now, preset, 0),
                None => crate::history::fill_plugin_side(&mut self.state, fill, &now, preset.as_deref()),
            }
            let side = &states[i].1;
            if side.fill != 0 || side.state.is_empty() || !self.plugins.set_state(key, &side.state) {
                continue;
            }
            self.plugins.set_preset(key, side.preset.clone());
            // The parameters as the state left them: what the window's next edit and a parameter's undo move from.
            self.state.params.reread(key);
            for info in self.plugins.param_infos(key) {
                if let Some(v) = self.plugins.param_value(key, info.id) {
                    self.state.control.saw_param(owner, info.id, v);
                }
            }
            self.invalidate();
        }
        if relink.0 {
            self.transport.set_instruments(self.plugins.instruments());
        }
        if relink.1 {
            self.transport.set_effects(self.plugins.effects());
        }
        if relink.0 || relink.1 {
            self.state.piano.dirty = true;
            self.invalidate();
        }
    }
}
