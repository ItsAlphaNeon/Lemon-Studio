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
use crate::knob_id::KnobId;
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
pub fn set_members(s: &mut crate::session::Session, t: u16, k: u8, list: &[Member]) {
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
pub fn leveled(s: &crate::session::Session, t: u16, k: u8, id: u32, wet: bool, v: f32) -> Option<Vec<Member>> {
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
pub fn driven(s: &crate::session::Session, tg: u32) -> bool {
    let p = &s.project;
    let linked = match target::decode(tg) {
        Auto::SlotMix(t, k) => crate::remote::link_of(&s.settings, crate::typein::Target::Knob(KnobId::SlotMix(t, k))).is_some(),
        _ => false,
    };
    linked
        || p.automations.iter().any(|a| a.targets.contains(&tg))
        || p.patterns.iter().any(|q| q.controls.iter().any(|c| c.kind == tg))
        || p.controller_links.iter().any(|l| l.target() == tg)
}

/// After the wrap's undo step `name`: the remote control links on what moved follow it (they are the program's
/// settings, not the project's), and the step puts them back (`Taken::Links`).
pub fn relink(s: &mut crate::session::Session, name: &str, moved: &Moved) {
    let mut all = crate::remote::links(&s.settings);
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
    crate::remote::set_links(&s.settings, &all);
    let back = pairs.into_iter().map(|(a, b)| (b, a)).collect();
    crate::history::extend_newest(s, |cmds| cmds.push(crate::control::Command::Put(Box::new(crate::control::command::Taken::Links(back)))));
}

/// Points the project's automation clips, recorded pattern events, controller links and initial values on the moved
/// plugins' parameters at the members, and the remote control links (their moves go back with an undo, as a command's
/// do: `Session::link_moves`); the interface's own targets follow (`View::TargetsMoved`). Returns how many different
/// targets could not move (parameters past a member's room), which stay as they are.
pub fn retarget_in(moved: &Moved, s: &mut crate::session::Session) -> usize {
    let mut stuck = std::collections::BTreeSet::new();
    let mut f = |t: &mut u32| match moved.target(*t) {
        Some(Some(n)) => *t = n,
        Some(None) => {
            stuck.insert(*t);
        }
        None => {}
    };
    let p = s.project.get_mut();
    p.automations.iter_mut().flat_map(|a| a.targets.iter_mut()).for_each(&mut f);
    p.patterns.iter_mut().flat_map(|q| q.controls.iter_mut()).for_each(|c| f(&mut c.kind));
    for l in &mut p.controller_links {
        let mut t = l.target();
        f(&mut t);
        l.set_target(t);
    }
    p.init_controls.iter_mut().for_each(|c| f(&mut c.target));
    crate::remote::follow(s, |l| Some(moved.link(l).unwrap_or(l)));
    s.touched.views.push(crate::session::View::TargetsMoved(moved.clone()));
    stuck.len()
}

/// Wrap in container (`all` = false) and Wrap all in container on slot `slot` of track `t` (spec: plugins/patcher/
/// README.md §3.3, §4.5; `Command::WrapEffects`). The slot's effect, or every effect of the track in slot order, becomes
/// a member with the levels its slot blended it at and its slot's CPU switches; the container takes the slot, switched
/// on at full mix, and keeps the slot's name, colour and icon; the other wrapped slots are emptied, their names,
/// colours and icons going to their members. A slot whose plugin is not hosted stays where it is (it has no state to
/// move); a container wrapped by Wrap all brings its members, renumbered. The automation clips, recorded pattern events,
/// controller links, initial values and remote control links on the wrapped plugins' parameters (and on a wrapped
/// container's members) are pointed at the members. Wrap in container on a slot whose own switch or mix a clip or link
/// drives keeps the slot's switch and mix, its effect starting at wet 100 %, dry 0 %, so the clip blends it as before.
/// The message.
pub fn wrap_slot_in(s: &mut crate::session::Session, t: u16, slot: u8, all: bool) -> Result<String, String> {
    let track = crate::edit::mixer::track(s, t).ok_or("no such track")?;
    let slots: Vec<u8> = if all { (0..10u8).filter(|&k| track.slots[usize::from(k)].is_some()).collect() } else { vec![slot] };
    // A single slot whose switch or mix is driven keeps them: they blend the container as they blended the effect.
    let keep_mix = !all && (driven(s, target::slot(t, slot, target::SLOT_MIX)) || driven(s, target::slot(t, slot, target::SLOT_ON)));
    let kept = (track.slot_mix[usize::from(slot)], track.slot_on[usize::from(slot)]);
    let mut list = Vec::new();
    let mut taken = Vec::new();
    let mut left = Vec::new();
    let mut moves = Vec::new();
    for k in slots {
        let (mix, on) = (track.slot_mix[usize::from(k)], track.slot_on[usize::from(k)]);
        if let Some(inner) = members(s, t, k) {
            let first = list.len() as u32 + 1;
            moves.push(Move { from: (t, k), ids: Ids::Members(inner.iter().zip(first..).map(|(m, new)| (m.id, new)).collect()) });
            list.extend(inner);
            taken.push(k);
            continue;
        }
        let Some(h) = s.plugins.hosted.get(&fx_key(t, k)) else {
            left.push(k);
            continue;
        };
        let Ok(p) = h.plugin.lock() else {
            left.push(k);
            continue;
        };
        let (wet, dry) = if keep_mix { (1.0, 0.0) } else { levels(mix, on) };
        let mut m = Member { uid: p.desc().uid(), name: p.desc().name.clone(), wet, dry, state: p.save_state().unwrap_or_default(), ..Member::default() };
        m.cpu = track.slot_cpu[usize::from(k)];
        // An emptied slot's name, colour and icon go with its effect; the container's slot keeps its own.
        if k != slot {
            m.label = crate::edit::mixer::slot_label(s, t, k);
            m.color = crate::edit::mixer::slot_color(s, t, k);
            m.icon = crate::edit::playlist::icon_of(&s.project, crate::edit::playlist::IconOwner::Slot(t, k));
        }
        moves.push(Move { from: (t, k), ids: Ids::Plugin(list.len() as u32 + 1) });
        list.push(m);
        taken.push(k);
    }
    if !taken.contains(&slot) {
        return Err("This slot's plugin is not hosted: nothing to wrap".into());
    }
    if list.len() > MEMBER_IDS as usize {
        return Err(format!("A container holds at most {MEMBER_IDS} effects"));
    }
    for (i, m) in list.iter_mut().enumerate() {
        m.id = i as u32 + 1;
    }
    let n = list.len();
    // The slot's name, colour and icon go with the container (`clear_slot` forgets them).
    let name = crate::edit::mixer::slot_label(s, t, slot);
    let color = crate::edit::mixer::slot_color(s, t, slot);
    let icon = crate::edit::playlist::icon_of(&s.project, crate::edit::playlist::IconOwner::Slot(t, slot));
    for &k in &taken {
        // The hosted instances go with an undo kept, the slots' names, colours and icons too.
        crate::edit::mixer::clear_slot(s, t, k);
    }
    if let Some(m) = crate::edit::mixer::track_mut(s, t) {
        m.slots[usize::from(slot)] = Some(SlotPlugin { id: Id::NONE, name: CONTAINER.into(), state: encode(&list) });
        if keep_mix {
            (m.slot_mix[usize::from(slot)], m.slot_on[usize::from(slot)]) = kept;
        }
    }
    let moved = Moved { to: (t, slot), index: 0, moves };
    let stuck = retarget_in(&moved, s);
    if name.is_some() {
        crate::edit::mixer::set_slot_label(s, t, slot, name);
    }
    if color.is_some() {
        crate::edit::mixer::set_slot_color(s, t, slot, color);
    }
    if icon.is_some() {
        crate::edit::playlist::put_icon(s, crate::edit::playlist::IconOwner::Slot(t, slot), icon);
    }
    let mut msg = format!("{} effect{} in a container", n, if n == 1 { "" } else { "s" });
    if !left.is_empty() {
        let list: Vec<String> = left.iter().map(|k| (k + 1).to_string()).collect();
        msg += &format!("; not hosted, left in place: slot {}", list.join(", "));
    }
    msg += &stuck_text(stuck);
    Ok(msg)
}

/// The channel menu's Wrap in container (the original's Patcherize on a channel; spec: plugins/patcher/README.md §3.3,
/// §4.4; `Command::WrapInstrument`): rack row `row`'s instrument becomes the first member of a container on the channel,
/// at [`INSTRUMENT_LEVELS`], with its state; the channel keeps its name, colour, notes, steps, mixer track and settings,
/// which act on the container's output. The channel takes a new id, so the instrument kept under the old one
/// (`Plugins::keep`) comes back with an undo, as Replace's does. The clips, recorded pattern events and links on the
/// instrument's parameters follow it onto its member (spec §4.5); those on the channel's own settings stay. The message.
pub fn wrap_channel_in(s: &mut crate::session::Session, row: usize) -> Result<String, String> {
    let ch = s.project.channels.get(row).ok_or("no such channel")?;
    let wrappable = ch.kind == os_model::ChannelKind::Plugin && !is_container_channel(ch) && s.hosted_rows.contains(&row);
    if !wrappable || !s.plugins.hosted.contains_key(&row) {
        return Err("This channel's plugin is not hosted: nothing to wrap".into());
    }
    let member = s
        .plugins
        .hosted
        .get(&row)
        .and_then(|h| {
            let p = h.plugin.lock().ok()?;
            let (wet, dry) = INSTRUMENT_LEVELS;
            Some(Member { id: 1, instrument: true, uid: p.desc().uid(), name: p.desc().name.clone(), wet, dry, state: p.save_state().unwrap_or_default(), ..Member::default() })
        })
        .ok_or("This channel's plugin is not hosted: nothing to wrap")?;
    let t = u16::try_from(row).map_err(|_| "no such channel")?;
    let index = s.project.channels[row].index;
    let moved = Moved { to: (t, CHANNEL), index, moves: vec![Move { from: (t, CHANNEL), ids: Ids::Plugin(1) }] };
    let name = member.name.clone();
    crate::project_plugins::drop_row(s, row);
    let ch = &mut s.project.get_mut().channels[row];
    // A new id at once, before the container plays the channel.
    ch.id = Id::NONE;
    ch.plugin = Some(CONTAINER.into());
    ch.plugin_state = Some(encode(&[member]));
    s.control.settle(s.project.get_mut());
    let stuck = retarget_in(&moved, s);
    Ok(format!("{name} in a container{}", stuck_text(stuck)))
}
