// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Containers, Lemon Studio's Patcher (slice 1): a mixer slot holding a serial chain of effects, each with its own
//! wet and dry levels. The slot menu's Wrap in container moves the slot's effect into a new container in the same
//! slot; Wrap all in container moves the track's effects, in slot order, into one container in this slot and empties
//! the others. The container's list (`list_popup`) removes, moves and adds effects and opens their windows. Each edit
//! is one undo step.
//!
//! The project keeps a container as a slot plugin of its own name whose state lists the members (`Member`): each
//! effect's identity, name, levels and saved state. The slot's chain is built from that list (`App::sync_containers`)
//! and plays as the slot's effect (`os_engine::chain::Chain`). The members' instances are hosted under keys of their
//! own (`plugins::MEMBER_KEY`), for their windows and housekeeping; they have no automation, links or presets yet.
// spec: plugins/patcher/README.md §3.1, §3.3, §4 slice 1, §4.1

use std::sync::{Arc, Mutex};

use os_engine::chain::{self, Chain};
use os_model::{Id, Project, SlotPlugin};

use crate::app::{App, State};
use crate::playlist::{Cmd, PItem, Popup};
use crate::plugins::{fx_key, fx_of, Hosted, Plugins, MEMBER_KEY};

/// The slot plugin name a container is kept under (Lemon Studio's own; the original's Patcher state is not read yet).
pub const CONTAINER: &str = "Lemon Container";

/// The state's first bytes and version (1: without the members' ids).
const MAGIC: &[u8; 4] = b"LCNT";
const VERSION: u32 = 2;

/// One effect of a container, as the project keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    /// Its number in the container, kept while it is moved (an edit keeps its instance and window).
    pub id: u32,
    /// The effect plugin's identity (`os_host::Descriptor::uid`) and name.
    pub uid: String,
    pub name: String,
    /// The output is `wet` times the effect's output plus `dry` times its input.
    pub wet: f32,
    pub dry: f32,
    /// The plugin's own saved state (empty: the plugin's defaults).
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
        b.extend(m.id.to_le_bytes());
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
    let version = word(&mut at)?;
    if !(1..=VERSION).contains(&version) {
        return None;
    }
    let count = word(&mut at)?;
    let mut v = Vec::new();
    for i in 0..count {
        let id = if version >= 2 { word(&mut at)? } else { i + 1 };
        let uid = String::from_utf8(bytes(&mut at)?).ok()?;
        let name = String::from_utf8(bytes(&mut at)?).ok()?;
        let wet = f32::from_bits(word(&mut at)?);
        let dry = f32::from_bits(word(&mut at)?);
        let state = bytes(&mut at)?;
        v.push(Member { id, uid, name, wet, dry, state });
    }
    Some(v)
}

/// The members of slot `slot` of track `t`, when it holds a container.
pub fn members(s: &State, t: u16, slot: u8) -> Option<Vec<Member>> {
    let p = crate::mixer::track(s, t)?.slots.get(usize::from(slot))?.as_ref()?;
    (p.name == CONTAINER).then(|| decode(&p.state)).flatten()
}

/// Writes `list` as the state of the container in slot `slot` of track `t`.
fn set_members(s: &mut State, t: u16, slot: u8, list: &[Member]) {
    if let Some(p) = crate::mixer::track_mut(s, t).and_then(|m| m.slots[usize::from(slot)].as_mut()).filter(|p| p.name == CONTAINER) {
        p.state = encode(list);
    }
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
    /// The members' ids and identities by position (another list rebuilds the chain; other levels only set them).
    shape: Vec<(u32, String)>,
    /// The members' `hosted` keys by position (`None`: its effect was not found, left out of the chain).
    pub keys: Vec<Option<usize>>,
    pub fx: Arc<Mutex<Chain>>,
}

fn shape(members: &[Member]) -> Vec<(u32, String)> {
    members.iter().map(|m| (m.id, m.uid.clone())).collect()
}

/// The undo step names of the menu entries.
pub const WRAP: &str = "Wrap in container";
pub const WRAP_ALL: &str = "Wrap all in container";
pub const REMOVE: &str = "Remove effect from container";
pub const MOVE: &str = "Move effect in container";
pub const ADD: &str = "Add effect to container";

/// What the container's list does to a member.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    /// Opens its plugin window (no undo step).
    Open,
    Up,
    Down,
    Remove,
}

/// Whether slot `slot` of the current track can be wrapped: a hosted effect, or (for Wrap all) a container.
pub fn can_wrap(app: &App, slot: u8, all: bool) -> bool {
    let t = app.state.selected_mixer;
    app.plugins.hosted.contains_key(&fx_key(t, slot)) || (all && members(&app.state, t, slot).is_some())
}

fn pct(v: f32) -> i32 {
    (v * 100.0).round() as i32
}

fn label(i: usize, m: &Member) -> String {
    format!("{}. {} (wet {}%, dry {}%)", i + 1, m.name, pct(m.wet), pct(m.dry))
}

/// The slot menu's Container ▸ for a container in slot `slot` of the current track: its members, "1. Lemon Limiter
/// (wet 100%, dry 0%)", each opening its window, then Edit list... (`list_popup`).
pub fn list_items(s: &State, slot: u8) -> Option<Vec<PItem>> {
    let m = members(s, s.selected_mixer, slot)?;
    let mx = |i: usize, op: Op| Cmd::Mixer(crate::mixer::MCmd::Member(slot, i, op));
    let mut v: Vec<PItem> = m.iter().enumerate().map(|(i, m)| PItem::new(label(i, m), mx(i, Op::Open))).collect();
    if v.is_empty() {
        v.push(PItem::new("(empty)", Cmd::Nothing).off());
    }
    v.push(PItem::sep());
    v.push(PItem::new("Edit list...", Cmd::Mixer(crate::mixer::MCmd::ContainerList(slot))));
    Some(v)
}

/// The container's list (Lemon Studio's plain list window of slice 1, as a menu): each member ▸ Open editor, Move up,
/// Move down, Remove; then Add effect ▸, the effect picker of an empty slot.
pub fn list_popup(s: &State, slot: u8, x: f32, y: f32) -> Option<Popup> {
    let m = members(s, s.selected_mixer, slot)?;
    let n = m.len();
    let mx = |i: usize, op: Op| Cmd::Mixer(crate::mixer::MCmd::Member(slot, i, op));
    let on = |it: PItem, yes: bool| if yes { it } else { it.off() };
    let mut items = vec![PItem::head("Container")];
    for (i, mem) in m.iter().enumerate() {
        let ops = vec![
            PItem::new("Open editor", mx(i, Op::Open)),
            on(PItem::new("Move up", mx(i, Op::Up)), i > 0),
            on(PItem::new("Move down", mx(i, Op::Down)), i + 1 < n),
            PItem::new("Remove", mx(i, Op::Remove)),
        ];
        items.push(PItem::sub(label(i, mem), ops));
    }
    if n == 0 {
        items.push(PItem::new("(empty)", Cmd::Nothing).off());
    }
    items.push(PItem::sep());
    items.push(PItem::sub("Add effect", crate::plugin_menu::items(s, crate::plugin_menu::Target::Container(slot), true)));
    Some(Popup { x, y, items, sub: None, scroll: [0; 2] })
}

/// Before an edit of the containers' lists: each member's state as its instance holds it now, so that the edit (and its
/// undo) keeps what was set in the members' windows. A save stores them the same way (`project_plugins::store_hosted`).
pub fn store_members(proj: &mut Project, plugins: &Plugins) {
    for (&key, live) in &plugins.containers {
        let Some((t, k)) = fx_of(key) else { continue };
        let Some(p) = proj.mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slots[usize::from(k)].as_mut()).filter(|p| p.name == CONTAINER) else {
            continue;
        };
        let Some(mut list) = decode(&p.state) else { continue };
        let mut changed = false;
        for m in &mut list {
            let Some(at) = live.shape.iter().position(|s| (s.0, s.1.as_str()) == (m.id, m.uid.as_str())) else { continue };
            let Some(h) = live.keys[at].and_then(|k| plugins.hosted.get(&k)) else { continue };
            if let Some(state) = h.plugin.lock().ok().and_then(|p| p.save_state()).filter(|st| *st != m.state) {
                m.state = state;
                changed = true;
            }
        }
        if changed {
            p.state = encode(&list);
        }
    }
}

impl App {
    /// Wrap in container (`all` = false) and Wrap all in container on slot `slot` of the current track (spec: plugins/
    /// patcher/README.md §3.3). The slot's effect, or every effect of the track in slot order, becomes a member with the
    /// levels its slot blended it at; the container takes the slot, switched on at full mix, and keeps the slot's name,
    /// colour and icon; the other wrapped slots are emptied with theirs. A slot whose plugin is not hosted stays where it
    /// is (it has no state to move); a container wrapped by Wrap all brings its members. Run inside the undo step
    /// (`mixer::run`).
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
            list.push(Member { id: 0, uid: p.desc().uid(), name: p.desc().name.clone(), wet, dry, state: p.save_state().unwrap_or_default() });
            taken.push(k);
        }
        if !taken.contains(&slot) {
            return Some("This slot's plugin is not hosted: nothing to wrap".into());
        }
        for (i, m) in list.iter_mut().enumerate() {
            m.id = i as u32 + 1;
        }
        let n = list.len();
        // The slot's name, colour and icon go with the container (`clear_slot` forgets them).
        let name = self.state.mixer.slot_names.get(&(t, slot)).cloned();
        let color = self.state.mixer.slot_colors.get(&(t, slot)).copied();
        let icon = self.state.playlist.icons.get(&crate::playlist::IconOwner::Slot(t, slot)).copied();
        for &k in &taken {
            // The hosted instances go with an undo kept (`FxReq::Delete`), the slots' names, colours and icons too.
            crate::mixer::clear_slot(&mut self.state, t, k);
        }
        if let Some(m) = crate::mixer::track_mut(&mut self.state, t) {
            m.slots[usize::from(slot)] = Some(SlotPlugin { id: Id::NONE, name: CONTAINER.into(), state: encode(&list) });
        }
        if let Some(v) = name {
            self.state.mixer.slot_names.insert((t, slot), v);
        }
        if let Some(v) = color {
            self.state.mixer.slot_colors.insert((t, slot), v);
        }
        if let Some(v) = icon {
            self.state.playlist.icons.insert(crate::playlist::IconOwner::Slot(t, slot), v);
        }
        self.state.piano.dirty = true;
        let mut msg = format!("{} effect{} in a container", n, if n == 1 { "" } else { "s" });
        if !left.is_empty() {
            let list: Vec<String> = left.iter().map(|k| (k + 1).to_string()).collect();
            msg += &format!("; not hosted, left in place: slot {}", list.join(", "));
        }
        Some(msg)
    }

    /// The container list's entries on member `i` of the container in slot `slot` of the current track: Open editor, or
    /// (inside the undo step, `mixer::run`) Move up, Move down, Remove. The chain follows on the next tick.
    pub(crate) fn edit_member(&mut self, slot: u8, i: usize, op: Op) -> Option<String> {
        let t = self.state.selected_mixer;
        let mut list = members(&self.state, t, slot)?;
        if i >= list.len() {
            return None;
        }
        match op {
            Op::Open => return self.open_member(t, slot, list[i].id),
            Op::Up if i > 0 => list.swap(i, i - 1),
            Op::Down if i + 1 < list.len() => list.swap(i, i + 1),
            Op::Remove => {
                list.remove(i);
            }
            _ => return None,
        }
        set_members(&mut self.state, t, slot, &list);
        self.state.piano.dirty = true;
        None
    }

    /// Add effect ▸ in the container's list: effect `index` (of `Plugins::effects`) at the end of the container in slot
    /// `slot` of the current track, wet only, from the plugin's defaults. Inside the undo step (`mixer::run`).
    pub(crate) fn add_member(&mut self, slot: u8, index: usize) -> Option<String> {
        let t = self.state.selected_mixer;
        let mut list = members(&self.state, t, slot)?;
        let a = self.plugins.effects.get(index)?;
        let id = list.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        list.push(Member { id, uid: a.desc.uid(), name: a.desc.name.clone(), wet: 1.0, dry: 0.0, state: Vec::new() });
        set_members(&mut self.state, t, slot, &list);
        self.state.piano.dirty = true;
        None
    }

    /// Opens the window of member `id` of the container in slot `slot` of track `t`.
    pub(crate) fn open_member(&mut self, t: u16, slot: u8, id: u32) -> Option<String> {
        let live = self.plugins.containers.get(&fx_key(t, slot))?;
        let Some(key) = live.shape.iter().position(|s| s.0 == id).and_then(|at| live.keys[at]) else {
            return Some("This effect is not loaded (its plugin was not found)".into());
        };
        self.open_member_key(key)
    }

    /// Opens the window of the member hosted under `key`.
    pub(crate) fn open_member_key(&mut self, key: usize) -> Option<String> {
        #[cfg(not(plugin_windows))]
        {
            let _ = key;
            Some(crate::app::off_text(crate::app::PLUGIN_WINDOWS))
        }
        #[cfg(plugin_windows)]
        {
            let info = self.member_infos().remove(&key)?;
            match self.plugins.open_window(key, info) {
                Ok(()) => {
                    self.state.raise(crate::app::Win::Hosted(key));
                    None
                }
                Err(e) => Some(format!("Plugin: {e}")),
            }
        }
    }

    /// The plugin frame's title of every member hosted, by key: the member's name over its container's slot's title.
    #[cfg(plugin_windows)]
    pub(crate) fn member_infos(&self) -> std::collections::HashMap<usize, crate::plugin_window::ChannelInfo> {
        let mut v = std::collections::HashMap::new();
        for (&slot_key, live) in &self.plugins.containers {
            let Some(info) = crate::mixer::fx_info(&self.state.project, &self.state.mixer, slot_key) else { continue };
            for key in live.keys.iter().flatten() {
                let Some(h) = self.plugins.hosted.get(key) else { continue };
                v.insert(*key, crate::plugin_window::ChannelInfo { name: format!("{} ({})", h.name, info.name), ..info.clone() });
            }
        }
        v
    }

    /// Where member `key` plays, for the performance monitor: "Bus 2, slot 1, container".
    pub(crate) fn member_place(&self, key: usize) -> Option<String> {
        let slot_key = self.plugins.containers.iter().find(|(_, l)| l.keys.contains(&Some(key)))?.0;
        Some(match fx_of(*slot_key)? {
            (0, k) => format!("Master, slot {}, container", k + 1),
            (t, k) => format!("Bus {t}, slot {}, container", k + 1),
        })
    }

    /// Every tick: the containers played follow the project's container slots (a wrap, an edit of the list, an undo,
    /// an opened project, a slot moved). A container whose list changed is built again from it: a member still there
    /// (by its id and effect) keeps its instance and window; a new one is an instance of its effect started from its
    /// saved state, or left out of the chain when its effect is not found (the list keeps it). A list whose levels alone
    /// changed sets them on the chain.
    pub(crate) fn sync_containers(&mut self) {
        let mut want: Vec<(usize, Id)> = Vec::new();
        for m in &self.state.project.mixer {
            for (k, p) in m.slots.iter().enumerate() {
                if let Some(p) = p.as_ref().filter(|p| p.name == CONTAINER) {
                    want.push((fx_key(m.index, k as u8), p.id));
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
            if want.iter().any(|w| w.0 == key) {
                continue;
            }
            let live = self.plugins.containers.remove(&key).unwrap();
            let to = want.iter().find(|w| w.1 == live.id && !live.id.is_none() && !self.plugins.containers.contains_key(&w.0)).map(|w| w.0);
            match to {
                Some(to) => {
                    self.plugins.containers.insert(to, live);
                }
                None => {
                    for k in live.keys.into_iter().flatten() {
                        self.plugins.drop_key(k);
                    }
                }
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
                    let playing = list.iter().zip(&live.keys).filter(|(_, k)| k.is_some());
                    for (at, (m, _)) in playing.enumerate() {
                        c.set_levels(at, m.wet, m.dry);
                    }
                }
                live.src = src;
                continue;
            }
            // Built again: the instances of the members still there are kept.
            let mut pool: Vec<((u32, String), usize)> = match self.plugins.containers.remove(&key) {
                Some(old) => old.shape.into_iter().zip(old.keys).filter_map(|(s, k)| Some((s, k?))).collect(),
                None => Vec::new(),
            };
            let mut keys = Vec::new();
            for m in &list {
                if let Some(at) = pool.iter().position(|(s, _)| (s.0, s.1.as_str()) == (m.id, m.uid.as_str())) {
                    keys.push(Some(pool.remove(at).1));
                    continue;
                }
                let made = self.plugins.effect_index_of_uid(&m.uid).and_then(|i| self.plugins.instantiate_effect(i, (!m.state.is_empty()).then_some(m.state.as_slice())).ok());
                match made {
                    Some((plugin, _)) => {
                        let k = MEMBER_KEY + self.plugins.next_member;
                        self.plugins.next_member += 1;
                        self.plugins.hosted.insert(k, Hosted::new(plugin));
                        keys.push(Some(k));
                    }
                    None => {
                        missing.push(m.name.clone());
                        keys.push(None);
                    }
                }
            }
            for (_, k) in pool {
                self.plugins.drop_key(k);
            }
            let hosted = &self.plugins.hosted;
            let fx = list.iter().zip(&keys).filter_map(|(m, k)| Some(chain::Member::new(hosted.get(&(*k)?)?.plugin.clone() as os_engine::SharedEffect, m.wet, m.dry))).collect();
            let live = Live { id, src, shape: shape(&list), keys, fx: Arc::new(Mutex::new(Chain::new(fx))) };
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

    /// The state round-trips; version 1 (no ids) numbers the members; other bytes are not a container.
    #[test]
    fn the_list_is_kept() {
        let m = vec![
            Member { id: 3, uid: "clap:a".into(), name: "A".into(), wet: 1.0, dry: 0.0, state: vec![1, 2, 3] },
            Member { id: 1, uid: "vst3:b".into(), name: "B".into(), wet: 0.25, dry: 0.75, state: vec![] },
        ];
        assert_eq!(decode(&encode(&m)), Some(m.clone()));
        assert_eq!(decode(&encode(&[])), Some(vec![]));
        let mut v1 = b"LCNT".to_vec();
        v1.extend(1u32.to_le_bytes());
        v1.extend(1u32.to_le_bytes());
        for f in [&b"clap:a"[..], b"A"] {
            v1.extend((f.len() as u32).to_le_bytes());
            v1.extend(f);
        }
        v1.extend(0.5f32.to_le_bytes());
        v1.extend(0.5f32.to_le_bytes());
        v1.extend(0u32.to_le_bytes());
        assert_eq!(decode(&v1), Some(vec![Member { id: 1, uid: "clap:a".into(), name: "A".into(), wet: 0.5, dry: 0.5, state: vec![] }]));
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

    /// An app with Lemon Studio's effects, the limiter in slot 1 and the delay in slot 2 of track 2; the two names.
    fn two_effects() -> (App, String, String) {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.reset_hosted();
        a.plugins.effects = crate::plugins::builtin_effects();
        a.state.plugdb.rebuild(&a.plugins);
        let find = |a: &App, n: &str| a.plugins.effects.iter().position(|e| e.desc.name.contains(n)).unwrap();
        let (lim, del) = (find(&a, "Limiter"), find(&a, "Delay"));
        let names = (a.plugins.effects[lim].desc.name.clone(), a.plugins.effects[del].desc.name.clone());
        crate::mixer::select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, lim));
        run(&mut a, MCmd::AddEffect(1, del));
        tick(&mut a);
        (a, names.0, names.1)
    }

    fn names(a: &App, slot: u8) -> Vec<String> {
        members(&a.state, 2, slot).unwrap_or_default().into_iter().map(|m| m.name).collect()
    }

    /// Wrap all in container: the track's two hosted effects become one container in the slot the command was used on,
    /// with their levels; the other slot is emptied; the chain plays the slot; one undo puts both slots and their
    /// instances back. Wrap in container on one slot does the same for that slot only.
    #[test]
    fn wrap_the_tracks_effects_in_a_container() {
        let (mut a, first, second) = two_effects();
        if let Some(m) = crate::mixer::track_mut(&mut a.state, 2) {
            m.slot_mix[1] = 0.5;
        }
        // The slot's name and colour go with the container; the emptied slot's go.
        a.state.mixer.slot_names.insert((2, 1), "Echo".into());
        a.state.mixer.slot_colors.insert((2, 0), os_model::Rgb { r: 1, g: 2, b: 3 });
        let menu = crate::mixer::slot_menu(&a, 1, 0.0, 0.0);
        let item = |label: &str| menu.items.iter().find(|i| i.label == label).unwrap().clone();
        assert!(item(WRAP).enabled && item(WRAP_ALL).enabled);
        assert_eq!(item(WRAP_ALL).cmd, Cmd::Mixer(MCmd::Wrap(1, true)));
        crate::playlist::run(&mut a, item(WRAP_ALL).cmd);
        tick(&mut a);
        let t = crate::mixer::track(&a.state, 2).unwrap();
        assert!(t.slots[0].is_none(), "the other slot is emptied");
        assert_eq!(t.slots[1].as_ref().map(|p| p.name.as_str()), Some(CONTAINER));
        assert_eq!((t.slot_mix[1], t.slot_on[1]), (1.0, true));
        assert_eq!(a.state.mixer.slot_names.get(&(2, 1)).map(String::as_str), Some("Echo"));
        assert!(a.state.mixer.slot_colors.is_empty());
        let list = members(&a.state, 2, 1).unwrap();
        assert_eq!(names(&a, 1), [first.clone(), second.clone()]);
        assert_eq!((list[0].id, list[1].id), (1, 2));
        assert_eq!((list[0].wet, list[0].dry), (1.0, 0.0));
        assert!((list[1].wet - 0.5).abs() < 1e-6 && (list[1].dry - 0.5).abs() < 1e-6);
        assert!(!a.plugins.hosted.contains_key(&fx_key(2, 0)) && !a.plugins.hosted.contains_key(&fx_key(2, 1)));
        let live = &a.plugins.containers[&fx_key(2, 1)];
        assert_eq!(live.fx.lock().unwrap().members.len(), 2);
        assert!(live.keys.iter().all(|k| k.is_some_and(|k| k >= MEMBER_KEY && a.plugins.hosted.contains_key(&k))), "members hosted under their own keys");
        assert_eq!(a.plugins.effects().iter().filter(|e| (e.0, e.1) == (2, 1)).count(), 1, "the chain plays the slot");
        assert_eq!(a.plugins.effects().len(), 1, "the members are not slots");
        // The slot menu lists the members.
        let menu = crate::mixer::slot_menu(&a, 1, 0.0, 0.0);
        let list = menu.items.iter().find(|i| i.label == "Container").expect("the list");
        assert_eq!(list.children[1].label, format!("2. {second} (wet 50%, dry 50%)"));

        // Undo: both slots and their instances come back; the members go.
        crate::history::undo(&mut a.state);
        tick(&mut a);
        let t = crate::mixer::track(&a.state, 2).unwrap();
        assert!(t.slots[0].is_some() && t.slots[1].as_ref().is_some_and(|p| p.name != CONTAINER));
        assert_eq!(t.slot_mix[1], 0.5);
        assert!(a.plugins.containers.is_empty());
        assert!(a.plugins.hosted.contains_key(&fx_key(2, 0)) && a.plugins.hosted.contains_key(&fx_key(2, 1)));
        assert!(a.plugins.hosted.range(MEMBER_KEY..).next().is_none());

        // Wrap in container: that slot only.
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, false)));
        tick(&mut a);
        let t = crate::mixer::track(&a.state, 2).unwrap();
        assert_eq!(t.slots[0].as_ref().map(|p| p.name.as_str()), Some(CONTAINER));
        assert!(t.slots[1].as_ref().is_some_and(|p| p.name != CONTAINER));
        assert_eq!(members(&a.state, 2, 0).map(|m| m.len()), Some(1));
        assert!(a.plugins.hosted.contains_key(&fx_key(2, 1)) && a.plugins.containers.contains_key(&fx_key(2, 0)));
        // Wrap all then takes the container's member and the other effect.
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        assert_eq!(names(&a, 0), [first, second]);
        assert!(crate::mixer::track(&a.state, 2).unwrap().slots[1].is_none());
    }

    /// The container's list: moving a member keeps its instance; removing one drops it; Add effect ▸ adds the effect
    /// at the end from its defaults; each is one undo step, and an undo of a removal brings the member back with the
    /// state its instance had.
    #[test]
    fn edit_the_containers_list() {
        let (mut a, first, second) = two_effects();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        let keys = |a: &App| a.plugins.containers[&fx_key(2, 0)].keys.clone();
        let before = keys(&a);
        let pop = list_popup(&a.state, 0, 0.0, 0.0).unwrap();
        let ops = &pop.items[2].children;
        assert_eq!(pop.items[2].label, format!("2. {second} (wet 100%, dry 0%)"));
        assert!(ops[1].enabled && !ops[2].enabled, "the last one moves up only");
        // Move up.
        crate::playlist::run(&mut a, ops[1].cmd.clone());
        tick(&mut a);
        assert_eq!(names(&a, 0), [second.clone(), first.clone()]);
        assert_eq!(keys(&a), vec![before[1], before[0]], "the instances are kept");
        assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some(MOVE));
        assert_eq!(crate::history::redo(&mut a.state).as_deref(), Some(MOVE));
        tick(&mut a);
        assert_eq!(keys(&a), vec![before[1], before[0]], "and kept through an undo and a redo");
        // A member's state, as its window would leave it, then Remove.
        let k = before[1].unwrap();
        let state = a.plugins.hosted[&k].plugin.lock().unwrap().save_state().unwrap();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Member(0, 0, Op::Remove)));
        tick(&mut a);
        assert_eq!(names(&a, 0), [first.clone()]);
        assert!(!a.plugins.hosted.contains_key(&k), "its instance goes");
        assert_eq!(a.plugins.containers[&fx_key(2, 0)].fx.lock().unwrap().members.len(), 1);
        crate::history::undo(&mut a.state);
        tick(&mut a);
        assert_eq!(names(&a, 0), [second.clone(), first.clone()]);
        assert_eq!(members(&a.state, 2, 0).unwrap()[0].state, state);
        // Add effect ▸: the picker of an empty slot, adding at the end.
        let pop = list_popup(&a.state, 0, 0.0, 0.0).unwrap();
        let add = pop.items.iter().find(|i| i.label == "Add effect").unwrap();
        let lim = add.children.iter().find(|i| i.label == first).expect("the limiter in the picker").cmd.clone();
        crate::playlist::run(&mut a, lim);
        tick(&mut a);
        assert_eq!(names(&a, 0), [second, first.clone(), first]);
        assert_eq!(members(&a.state, 2, 0).unwrap()[2].id, 3);
        assert_eq!(a.plugins.containers[&fx_key(2, 0)].fx.lock().unwrap().members.len(), 3);
        assert_eq!(crate::history::undo(&mut a.state).as_deref(), Some(ADD));
        assert_eq!(crate::history::redo(&mut a.state).as_deref(), Some(ADD));
        tick(&mut a);
        // A save stores the members' states.
        let mut proj = a.state.project.clone();
        crate::project_plugins::store_hosted(&mut proj, &a.plugins);
        let saved = proj.mixer.iter().find(|m| m.index == 2).unwrap().slots[0].as_ref().map(|p| decode(&p.state).unwrap()).unwrap();
        assert!(saved.iter().all(|m| !m.state.is_empty()));
        // Every member window has a title.
        #[cfg(plugin_windows)]
        assert_eq!(a.member_infos().len(), 3);
    }
}
