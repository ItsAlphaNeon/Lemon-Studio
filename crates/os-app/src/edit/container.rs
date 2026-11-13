// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The session's half of `crate::container`: what reads and changes the project, apart from the interface (specs
//! `roadmap/control-plane.md`, part 2, step 6).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
#![allow(unused_imports)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use os_engine::chain::{self, Chain};
use os_model::target::{self, Target as Auto, MEMBER_DRY, MEMBER_IDS, MEMBER_WET};
use os_model::{Id, Project, SlotPlugin};
use crate::app::{App, State, Win};
use crate::knob_id::KnobId;
use crate::playlist::{Cmd, PItem, Popup};
use crate::plugins::{fx_key, fx_of, Hosted, Plugins, FX_KEY, MEMBER_KEY};
use crate::container::*;

/// The members of the container at place (`t`, `k`) (a mixer slot, or a channel's for `k` = [`CHANNEL`]).
pub fn members(s: &crate::session::Session, t: u16, k: u8) -> Option<Vec<Member>> {
    let b = state_in(&s.project, t, k)?;
    // A channel just made a container and not saved yet holds no list: an empty one.
    if b.is_empty() && k == CHANNEL {
        return Some(Vec::new());
    }
    decode(b)
}

/// How many members the container at place (`t`, `k`) holds, read from its state's header alone (the window's size,
/// worked out on every frame without decoding the members' states).
pub fn member_count(s: &crate::session::Session, t: u16, k: u8) -> Option<usize> {
    let b = state_in(&s.project, t, k)?;
    let word = |at: usize| Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    (b.get(..4)? == MAGIC && (1..=VERSION).contains(&word(4)?)).then(|| word(8).map(|n| n as usize)).flatten()
}

/// Writes `list` as the state of the container at place (`t`, `k`).
pub(crate) fn set_members(s: &mut crate::session::Session, t: u16, k: u8, list: &[Member]) {
    if let Some(b) = state_in_mut(s.project.get_mut(), t, k) {
        *b = encode(list);
    }
}

/// The project id of the slot plugin or channel holding the container at place (`t`, `k`).
pub fn place_id(s: &crate::session::Session, t: u16, k: u8) -> Option<Id> {
    let p = &s.project;
    if k == CHANNEL {
        return p.channels.get(usize::from(t)).map(|c| c.id);
    }
    p.mixer.iter().find(|m| m.index == t)?.slots.get(usize::from(k))?.as_ref().map(|s| s.id)
}

/// The place of the container held by the slot plugin or channel with project id `id` (`place_id`'s other way).
pub fn id_place(s: &crate::session::Session, id: u32) -> Option<(u16, u8)> {
    let p = &s.project;
    if let Some(r) = p.channels.iter().position(|c| c.id.0 == id) {
        return Some((u16::try_from(r).ok()?, CHANNEL));
    }
    p.mixer.iter().find_map(|m| Some((m.index, m.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.id.0 == id))? as u8)))
}

/// What the container at place (`t`, `k`) is shown as: its track's name for a slot's ("Bus 2"), its channel's name.
pub fn place_name(s: &crate::session::Session, t: u16, k: u8) -> String {
    if k != CHANNEL {
        return crate::edit::mixer::name(s, t);
    }
    let ch = s.project.channels.get(usize::from(t));
    ch.and_then(|c| c.name.clone()).unwrap_or_else(|| format!("Channel {}", usize::from(t) + 1))
}

/// The container's title, "Container (Bus 2)": its slot's label, or for a channel's "Container (Lemon Trio)".
pub fn title(s: &crate::session::Session, t: u16, k: u8) -> Option<String> {
    if k != CHANNEL {
        return crate::params::effect_label(s, t, k);
    }
    state_in(&s.project, t, k)?;
    Some(format!("Container ({})", place_name(s, t, k)))
}

/// The automation target of item `item` of the container at place (`t`, `k`): a parameter of its slot or channel.
pub fn item_target(s: &crate::session::Session, t: u16, k: u8, item: u16) -> Option<u32> {
    if k == CHANNEL {
        return Some(target::channel_param(s.project.channels.get(usize::from(t))?.index, item));
    }
    Some(target::slot_param(t, k, item))
}

/// The container at place (`t`, `k`) and the item an automation target on its slot's or channel's parameters names.
pub fn target_place(s: &crate::session::Session, tg: Auto) -> Option<(u16, u8, u16)> {
    let (t, k, i) = match tg {
        Auto::SlotParam(m, k, i) => (m, k, i),
        Auto::ChannelParam(ch, i) => (u16::try_from(s.project.channels.iter().position(|c| c.index == ch)?).ok()?, CHANNEL, i),
        _ => return None,
    };
    state_in(&s.project, t, k)?;
    Some((t, k, i))
}

/// `put_level` in the session; the open list's knob shows the level (`session::View::MemberLevel`).
pub fn put_level_in(s: &mut crate::session::Session, t: u16, k: u8, id: u32, wet: bool, v: f32) {
    let Some(list) = leveled(s, t, k, id, wet, v) else { return };
    set_members(s, t, k, &list);
    s.touched.views.push(crate::session::View::MemberLevel { t, k, id, wet, v });
    s.dirty = true;
}

/// The wet (`wet`) or dry level of member `id` of the container in slot `k` of track `t`.
pub fn level(s: &crate::session::Session, t: u16, k: u8, id: u32, wet: bool) -> Option<f32> {
    members(s, t, k)?.into_iter().find(|m| m.id == id).map(|m| if wet { m.wet } else { m.dry })
}

/// The type-in box's caption: "Bus 2 - Lemon Limiter wet" (a channel's container: the channel's name first).
pub fn level_name(s: &crate::session::Session, t: u16, k: u8, id: u32, wet: bool) -> String {
    let name = members(s, t, k).and_then(|l| l.into_iter().find(|m| m.id == id)).map_or_else(|| "Effect".into(), |m| m.shown().to_string());
    format!("{} - {name} {}", place_name(s, t, k), if wet { "wet" } else { "dry" })
}

/// The member hosted under `key`: its container's place and its id.
pub fn of_key(s: &crate::session::Session, key: usize) -> Option<(u16, u8, u32)> {
    s.plugin_members.get(&key).copied()
}

/// The member with id `id` of the container at place (`t`, `k`).
pub fn member(s: &crate::session::Session, t: u16, k: u8, id: u32) -> Option<Member> {
    members(s, t, k)?.into_iter().find(|m| m.id == id)
}

/// The list with the level set (`None`: no such member, or the level is `v` already).
pub(crate) fn leveled(s: &crate::session::Session, t: u16, k: u8, id: u32, wet: bool, v: f32) -> Option<Vec<Member>> {
    let mut list = members(s, t, k)?;
    let m = list.iter_mut().find(|m| m.id == id)?;
    let level = if wet { &mut m.wet } else { &mut m.dry };
    if *level == v {
        return None;
    }
    *level = v;
    Some(list)
}

/// The automation target of member `id`'s wet (`wet`) or dry level in the container at place (`t`, `k`) (spec:
/// plugins/patcher/README.md §4.2, §4.4).
pub fn level_target(s: &crate::session::Session, t: u16, k: u8, id: u32, wet: bool) -> Option<u32> {
    item_target(s, t, k, target::member_item(id, if wet { MEMBER_WET } else { MEMBER_DRY })?)
}

/// The member level an automation target names, when it is one: track (or row), slot (or [`CHANNEL`]), member id, wet.
pub fn level_of(s: &crate::session::Session, tg: Auto) -> Option<(u16, u8, u32, bool)> {
    let (m, k, i) = target_place(s, tg)?;
    let (id, item) = target::member_of(i)?;
    (item >= MEMBER_WET && members(s, m, k)?.iter().any(|x| x.id == id)).then_some((m, k, id, item == MEMBER_WET))
}

/// "Create automation clip" on a member's level: target, clip name ("Bus 2 - Lemon Limiter wet"), current value.
pub fn level_automation(s: &crate::session::Session, t: u16, k: u8, id: u32, wet: bool) -> Option<(u32, String, f64)> {
    Some((level_target(s, t, k, id, wet)?, level_name(s, t, k, id, wet), f64::from(level(s, t, k, id, wet)?)))
}

/// The member that the container under `Plugins::containers` key `place` holds under id `id`: its hosted key, when its
/// plugin is loaded.
pub fn member_key(s: &crate::session::Session, place: usize, id: u32) -> Option<usize> {
    let (t, k) = key_place(place)?;
    s.plugin_members.iter().find(|(_, m)| **m == (t, k, id)).map(|(&key, _)| key)
}

/// The name of the member hosted under `key` and its container's title, "Lemon Limiter in Container (Bus 2)".
pub fn member_label(s: &crate::session::Session, key: usize) -> Option<String> {
    let &(t, k, id) = s.plugin_members.get(&key)?;
    let name = member(s, t, k, id)?.shown().to_string();
    Some(format!("{name} in {}", title(s, t, k)?))
}

/// Whether member `id` of the container at place (`t`, `k`) is an instrument.
pub fn is_instrument(s: &crate::session::Session, t: u16, k: u8, id: u32) -> bool {
    members(s, t, k).is_some_and(|l| l.iter().any(|m| m.id == id && m.instrument))
}

/// Whether an automation clip, a recorded pattern event or a controller link drives target `tg`, or (a slot's mix) a
/// remote control link.
pub(crate) fn driven(s: &crate::session::Session, tg: u32) -> bool {
    let p = &s.project;
    let linked = match target::decode(tg) {
        Auto::SlotMix(t, k) => crate::remote::link_of(crate::typein::Target::Knob(KnobId::SlotMix(t, k))).is_some(),
        _ => false,
    };
    linked
        || p.automations.iter().any(|a| a.targets.contains(&tg))
        || p.patterns.iter().any(|q| q.controls.iter().any(|c| c.kind == tg))
        || p.controller_links.iter().any(|l| l.target() == tg)
}

/// After the wrap's undo step `name`: the remote control links on what moved follow it (they are the program's
/// settings, not the project's), and the step puts them back (`Taken::Links`).
pub(crate) fn relink(s: &mut crate::session::Session, name: &str, moved: &Moved) {
    let mut all = crate::remote::links();
    let mut pairs = Vec::new();
    for l in &mut all {
        if let Some(to) = moved.link(l.target) {
            pairs.push((l.target, to));
            l.target = to;
        }
    }
    if pairs.is_empty() || s.history.newest_done() != Some(name) {
        return;
    }
    crate::remote::set_links(&all);
    let back = pairs.into_iter().map(|(a, b)| (b, a)).collect();
    crate::history::extend_newest(s, |cmds| cmds.push(crate::control::Command::Put(Box::new(crate::control::command::Taken::Links(back)))));
}
