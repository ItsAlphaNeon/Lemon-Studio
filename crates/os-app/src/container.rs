// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Containers, Lemon Studio's Patcher (slice 1): a mixer slot holding a serial chain of effects, each with its own
//! wet and dry levels. The slot menu's Wrap in container moves the slot's effect into a new container in the same
//! slot; Wrap all in container moves the track's effects, in slot order, into one container in this slot and empties
//! the others. Each is one undo step.
//!
//! The project keeps a container as a slot plugin of its own name whose state lists the members (`Member`): each
//! effect's identity, name, levels and saved state. The slot's chain is built from that list (`App::sync_containers`)
//! and plays as the slot's hosted effect (`os_engine::chain::Chain`). Members have no windows, parameters, links or
//! automation of their own yet; the slot's menu lists them.
// spec: plugins/patcher/README.md §3.1, §3.3, §4 slice 1

use std::sync::{Arc, Mutex};

use os_engine::chain::{self, Chain};
use os_host::SharedPlugin;
use os_model::{Id, SlotPlugin};

use crate::app::{App, State};
use crate::plugins::{fx_key, fx_of};

/// The slot plugin name a container is kept under (Lemon Studio's own; the original's Patcher state is not read yet).
pub const CONTAINER: &str = "Lemon Container";

/// The state's first bytes and version.
const MAGIC: &[u8; 4] = b"LCNT";
const VERSION: u32 = 1;

/// One effect of a container, as the project keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    /// The effect plugin's identity (`os_host::Descriptor::uid`) and name.
    pub uid: String,
    pub name: String,
    /// The output is `wet` times the effect's output plus `dry` times its input.
    pub wet: f32,
    pub dry: f32,
    /// The plugin's own saved state.
    pub state: Vec<u8>,
}

/// The state of a container slot: the members in order.
pub fn encode(members: &[Member]) -> Vec<u8> {
    let mut b = MAGIC.to_vec();
    b.extend(VERSION.to_le_bytes());
    b.extend((members.len() as u32).to_le_bytes());
    let bytes = |b: &mut Vec<u8>, v: &[u8]| {
        b.extend((v.len() as u32).to_le_bytes());
        b.extend_from_slice(v);
    };
    for m in members {
        bytes(&mut b, m.uid.as_bytes());
        bytes(&mut b, m.name.as_bytes());
        b.extend(m.wet.to_le_bytes());
        b.extend(m.dry.to_le_bytes());
        bytes(&mut b, &m.state);
    }
    b
}

/// The members a container slot's state lists; `None` for bytes that are not one.
pub fn decode(b: &[u8]) -> Option<Vec<Member>> {
    if b.get(..4)? != MAGIC {
        return None;
    }
    let mut at = 4;
    let word = |at: &mut usize| -> Option<u32> {
        let w = u32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
        *at += 4;
        Some(w)
    };
    let bytes = |at: &mut usize| -> Option<Vec<u8>> {
        let n = word(at)? as usize;
        let s = b.get(*at..at.checked_add(n)?)?.to_vec();
        *at += n;
        Some(s)
    };
    if word(&mut at)? != VERSION {
        return None;
    }
    let count = word(&mut at)?;
    let mut v = Vec::new();
    for _ in 0..count {
        let uid = String::from_utf8(bytes(&mut at)?).ok()?;
        let name = String::from_utf8(bytes(&mut at)?).ok()?;
        let wet = f32::from_bits(word(&mut at)?);
        let dry = f32::from_bits(word(&mut at)?);
        let state = bytes(&mut at)?;
        v.push(Member { uid, name, wet, dry, state });
    }
    Some(v)
}

/// The members of slot `slot` of track `t`, when it holds a container.
pub fn members(s: &State, t: u16, slot: u8) -> Option<Vec<Member>> {
    let p = crate::mixer::track(s, t)?.slots.get(usize::from(slot))?.as_ref()?;
    (p.name == CONTAINER).then(|| decode(&p.state)).flatten()
}

/// The wet and dry levels of a slot at stored mix `mix` (0..1) and switch `on`, as the slot blends them (spec:
/// audio/mixer.md "An effect slot"): an effect switched off keeps only its dry part (the manual's "mix 0").
fn levels(mix: f64, on: bool) -> (f32, f32) {
    if !on {
        return (0.0, 1.0);
    }
    let d = os_dsp::mixer::dry_level((mix.clamp(0.0, 1.0) * 12800.0).round() as i32);
    (1.0 - d, d)
}

/// A container being played: the slot's id, the state its chain was built from, the members' instances and the chain.
pub struct Live {
    pub id: Id,
    /// The slot's state the chain was built from: compared every tick, decoded only when it changed.
    src: Vec<u8>,
    /// The members' identities and state lengths (another list rebuilds the chain; other levels only set them).
    shape: Vec<(String, usize)>,
    /// The members' instances by position (`None`: its effect was not found, left out of the chain).
    pub plugins: Vec<Option<SharedPlugin>>,
    pub fx: Arc<Mutex<Chain>>,
}

fn shape(members: &[Member]) -> Vec<(String, usize)> {
    members.iter().map(|m| (m.uid.clone(), m.state.len())).collect()
}

/// The undo step names of the two menu entries.
pub const WRAP: &str = "Wrap in container";
pub const WRAP_ALL: &str = "Wrap all in container";

/// Whether slot `slot` of the current track can be wrapped: a hosted effect, or (for Wrap all) a container.
pub fn can_wrap(app: &App, slot: u8, all: bool) -> bool {
    let t = app.state.selected_mixer;
    app.plugins.hosted.contains_key(&fx_key(t, slot)) || (all && members(&app.state, t, slot).is_some())
}

/// The list of the members of a container in slot `slot` of track `t` (the plain list of slice 1, in the slot menu and
/// shown when the slot is opened): "1. Lemon Limiter (wet 100%, dry 0%)".
pub fn list_items(s: &State, t: u16, slot: u8) -> Option<Vec<crate::playlist::PItem>> {
    let m = members(s, t, slot)?;
    let pct = |v: f32| (v * 100.0).round() as i32;
    let mut v: Vec<crate::playlist::PItem> = m
        .iter()
        .enumerate()
        .map(|(i, m)| crate::playlist::PItem::new(format!("{}. {} (wet {}%, dry {}%)", i + 1, m.name, pct(m.wet), pct(m.dry)), crate::playlist::Cmd::Nothing))
        .collect();
    if v.is_empty() {
        v.push(crate::playlist::PItem::new("(empty)", crate::playlist::Cmd::Nothing).off());
    }
    Some(v)
}

impl App {
    /// Wrap in container (`all` = false) and Wrap all in container on slot `slot` of the current track (spec: plugins/
    /// patcher/README.md §3.3). The slot's effect, or every effect of the track in slot order, becomes a member with the
    /// levels its slot blended it at; the container takes the slot, switched on at full mix, and the other wrapped
    /// slots are emptied. A slot whose plugin is not hosted stays where it is (it has no state to move); a container
    /// wrapped by Wrap all brings its members. Run inside the undo step (`mixer::run`).
    pub(crate) fn wrap_in_container(&mut self, slot: u8, all: bool) -> Option<String> {
        let t = self.state.selected_mixer;
        let track = crate::mixer::track(&self.state, t)?;
        let slots: Vec<u8> = if all { (0..10u8).filter(|&k| track.slots[usize::from(k)].is_some()).collect() } else { vec![slot] };
        let mut list = Vec::new();
        let mut taken = Vec::new();
        let mut left = Vec::new();
        for k in slots {
            let (mix, on) = (track.slot_mix[usize::from(k)], track.slot_on[usize::from(k)]);
            if let Some(inner) = members(&self.state, t, k) {
                list.extend(inner);
                taken.push(k);
                continue;
            }
            let Some(h) = self.plugins.hosted.get(&fx_key(t, k)) else {
                left.push(k);
                continue;
            };
            let Ok(p) = h.plugin.lock() else {
                left.push(k);
                continue;
            };
            let (wet, dry) = levels(mix, on);
            list.push(Member { uid: p.desc().uid(), name: p.desc().name.clone(), wet, dry, state: p.save_state().unwrap_or_default() });
            taken.push(k);
        }
        if !taken.contains(&slot) {
            return Some("This slot's plugin is not hosted: nothing to wrap".into());
        }
        let n = list.len();
        for &k in &taken {
            // The hosted instances go with an undo kept (`FxReq::Delete`), the slots' names, colours and icons too.
            crate::mixer::clear_slot(&mut self.state, t, k);
        }
        if let Some(m) = crate::mixer::track_mut(&mut self.state, t) {
            m.slots[usize::from(slot)] = Some(SlotPlugin { id: Id::NONE, name: CONTAINER.into(), state: encode(&list) });
        }
        self.state.piano.dirty = true;
        let mut msg = format!("{} effect{} in a container", n, if n == 1 { "" } else { "s" });
        if !left.is_empty() {
            let list: Vec<String> = left.iter().map(|k| (k + 1).to_string()).collect();
            msg += &format!("; not hosted, left in place: slot {}", list.join(", "));
        }
        Some(msg)
    }

    /// Every tick: the containers played follow the project's container slots (a wrap, an undo, an opened project, a
    /// slot moved). A container whose list changed is built again from it: each member an instance of its effect
    /// started from its saved state; one whose effect is not found is left out of the chain (the list keeps it). A
    /// list whose levels alone changed sets them on the chain.
    pub(crate) fn sync_containers(&mut self) {
        let mut want: Vec<(usize, Id)> = Vec::new();
        for m in &self.state.project.mixer {
            for (k, p) in m.slots.iter().enumerate() {
                if p.as_ref().is_some_and(|p| p.name == CONTAINER) {
                    want.push((fx_key(m.index, k as u8), p.as_ref().unwrap().id));
                }
            }
        }
        if want.is_empty() && self.plugins.containers.is_empty() {
            return;
        }
        let mut changed = false;
        // Gone, or moved to another slot (found by the slot's id).
        let keys: Vec<usize> = self.plugins.containers.keys().copied().collect();
        for key in keys {
            let id = self.plugins.containers[&key].id;
            if want.iter().any(|w| w.0 == key) {
                continue;
            }
            let live = self.plugins.containers.remove(&key).unwrap();
            if let Some(&(to, _)) = want.iter().find(|w| w.1 == id && !id.is_none() && !self.plugins.containers.contains_key(&w.0)) {
                self.plugins.containers.insert(to, live);
            }
            changed = true;
        }
        let mut missing = Vec::new();
        for (key, id) in want {
            let Some((t, k)) = fx_of(key) else { continue };
            let Some(p) = crate::mixer::track(&self.state, t).and_then(|m| m.slots[usize::from(k)].as_ref()) else { continue };
            if let Some(live) = self.plugins.containers.get_mut(&key) {
                live.id = id;
                if live.src == p.state {
                    continue;
                }
            }
            let list = decode(&p.state).unwrap_or_default();
            let src = p.state.clone();
            if let Some(live) = self.plugins.containers.get_mut(&key).filter(|l| l.shape == shape(&list)) {
                if let Ok(mut c) = live.fx.lock() {
                    let mut at = 0;
                    for (m, p) in list.iter().zip(&live.plugins) {
                        if p.is_some() {
                            c.set_levels(at, m.wet, m.dry);
                            at += 1;
                        }
                    }
                }
                live.src = src;
                continue;
            }
            let mut plugins = Vec::new();
            for m in &list {
                let made = self.plugins.effect_index_of_uid(&m.uid).and_then(|i| self.plugins.instantiate_effect(i, (!m.state.is_empty()).then_some(m.state.as_slice())).ok());
                if made.is_none() {
                    missing.push(m.name.clone());
                }
                plugins.push(made.map(|(p, _)| p));
            }
            let members = list.iter().zip(&plugins).filter_map(|(m, p)| Some(chain::Member::new(p.clone()? as os_engine::SharedEffect, m.wet, m.dry))).collect();
            let live = Live { id, src, shape: shape(&list), plugins, fx: Arc::new(Mutex::new(Chain::new(members))) };
            self.plugins.containers.insert(key, live);
            changed = true;
        }
        if !missing.is_empty() {
            self.state.message = Some(format!("Container: not found, left out of the chain: {}", missing.join(", ")));
        }
        if changed {
            self.transport.set_effects(self.plugins.effects());
            self.state.piano.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::{run, MCmd};

    /// The state round-trips; other bytes are not a container.
    #[test]
    fn the_list_is_kept() {
        let m = vec![
            Member { uid: "clap:a".into(), name: "A".into(), wet: 1.0, dry: 0.0, state: vec![1, 2, 3] },
            Member { uid: "vst3:b".into(), name: "B".into(), wet: 0.25, dry: 0.75, state: vec![] },
        ];
        assert_eq!(decode(&encode(&m)), Some(m));
        assert_eq!(decode(&encode(&[])), Some(vec![]));
        assert_eq!(decode(b"LCNT"), None);
        assert_eq!(decode(&[0; 16]), None);
        assert_eq!(levels(1.0, true), (1.0, 0.0));
        assert_eq!(levels(0.3, false), (0.0, 1.0));
        let (w, d) = levels(0.5, true);
        assert!((w - 0.5).abs() < 1e-6 && (d - 0.5).abs() < 1e-6);
    }

    /// A tick as the program runs it for the mixer: ids for new slots, the mixer's requests, instances following ids.
    fn tick(a: &mut App) {
        a.state.control.settle(&mut a.state.project);
        a.mixer_tick();
        a.follow_ids(false, true);
    }

    /// Wrap all in container: the track's two hosted effects become one container in the slot the command was used on,
    /// with their levels; the other slot is emptied; the chain plays the slot; one undo puts both slots and their
    /// instances back. Wrap in container on one slot does the same for that slot only.
    #[test]
    fn wrap_the_tracks_effects_in_a_container() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.reset_hosted();
        a.plugins.effects = crate::plugins::builtin_effects();
        let find = |a: &App, n: &str| a.plugins.effects.iter().position(|e| e.desc.name.contains(n)).unwrap();
        let (lim, del) = (find(&a, "Limiter"), find(&a, "Delay"));
        let (first, second) = (a.plugins.effects[lim].desc.name.clone(), a.plugins.effects[del].desc.name.clone());
        crate::mixer::select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, lim));
        run(&mut a, MCmd::AddEffect(1, del));
        tick(&mut a);
        if let Some(m) = crate::mixer::track_mut(&mut a.state, 2) {
            m.slot_mix[1] = 0.5;
        }
        let menu = crate::mixer::slot_menu(&a, 1, 0.0, 0.0);
        let item = |label: &str| menu.items.iter().find(|i| i.label == label).unwrap().clone();
        assert!(item(WRAP).enabled && item(WRAP_ALL).enabled);
        assert_eq!(item(WRAP_ALL).cmd, crate::playlist::Cmd::Mixer(MCmd::Wrap(1, true)));
        crate::playlist::run(&mut a, item(WRAP_ALL).cmd);
        tick(&mut a);
        let t = crate::mixer::track(&a.state, 2).unwrap();
        assert!(t.slots[0].is_none(), "the other slot is emptied");
        assert_eq!(t.slots[1].as_ref().map(|p| p.name.as_str()), Some(CONTAINER));
        assert_eq!((t.slot_mix[1], t.slot_on[1]), (1.0, true));
        let list = members(&a.state, 2, 1).unwrap();
        assert_eq!(list.iter().map(|m| m.name.clone()).collect::<Vec<_>>(), [first.clone(), second.clone()]);
        assert_eq!((list[0].wet, list[0].dry), (1.0, 0.0));
        assert!((list[1].wet - 0.5).abs() < 1e-6 && (list[1].dry - 0.5).abs() < 1e-6);
        assert!(!a.plugins.hosted.contains_key(&fx_key(2, 0)) && !a.plugins.hosted.contains_key(&fx_key(2, 1)));
        let live = &a.plugins.containers[&fx_key(2, 1)];
        assert_eq!(live.fx.lock().unwrap().members.len(), 2);
        assert_eq!(a.plugins.effects().iter().filter(|e| (e.0, e.1) == (2, 1)).count(), 1, "the chain plays the slot");
        // The slot menu lists the members.
        let menu = crate::mixer::slot_menu(&a, 1, 0.0, 0.0);
        let list = menu.items.iter().find(|i| i.label == "Container").expect("the list");
        assert_eq!(list.children[1].label, format!("2. {second} (wet 50%, dry 50%)"));

        // Undo: both slots and their instances come back.
        crate::history::undo(&mut a.state);
        tick(&mut a);
        let t = crate::mixer::track(&a.state, 2).unwrap();
        assert!(t.slots[0].is_some() && t.slots[1].as_ref().is_some_and(|p| p.name != CONTAINER));
        assert_eq!(t.slot_mix[1], 0.5);
        assert!(a.plugins.containers.is_empty());
        assert!(a.plugins.hosted.contains_key(&fx_key(2, 0)) && a.plugins.hosted.contains_key(&fx_key(2, 1)));

        // Wrap in container: that slot only.
        crate::playlist::run(&mut a, crate::playlist::Cmd::Mixer(MCmd::Wrap(0, false)));
        tick(&mut a);
        let t = crate::mixer::track(&a.state, 2).unwrap();
        assert_eq!(t.slots[0].as_ref().map(|p| p.name.as_str()), Some(CONTAINER));
        assert!(t.slots[1].as_ref().is_some_and(|p| p.name != CONTAINER));
        assert_eq!(members(&a.state, 2, 0).map(|m| m.len()), Some(1));
        assert!(a.plugins.hosted.contains_key(&fx_key(2, 1)) && a.plugins.containers.contains_key(&fx_key(2, 0)));
        // Wrap all then takes the container's member and the other effect.
        crate::playlist::run(&mut a, crate::playlist::Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        assert_eq!(members(&a.state, 2, 0).map(|m| m.iter().map(|m| m.name.clone()).collect::<Vec<_>>()), Some(vec![first, second]));
        assert!(crate::mixer::track(&a.state, 2).unwrap().slots[1].is_none());
    }
}
