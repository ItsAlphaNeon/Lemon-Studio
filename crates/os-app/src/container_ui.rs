// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::container` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use crate::knobs::KnobUi;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
pub use crate::edit::container::*;
use os_engine::chain::{self, Chain};
use os_model::target::{self, Target as Auto, MEMBER_IDS, MEMBER_WET};
use os_model::{Id, Project, SlotPlugin};
use crate::app::{App, State, Win};
use crate::knobs::KnobId;
use crate::playlist::{Cmd, PItem, Popup};
use crate::plugins::{fx_key, fx_of, Hosted, Plugins, FX_KEY, MEMBER_KEY};
#[allow(unused_imports)]
use crate::container::*;

/// Shows the window of the container at place (`t`, `slot`).
pub fn show(s: &mut State, t: u16, slot: u8) {
    if s.container.at != (t, slot) {
        s.container.live.clear();
    }
    s.container.at = (t, slot);
    s.raise(Win::Container);
}

/// What a member's level knob shows: the level the chain plays while the window shows its container, else the list's.
pub fn shown_level(s: &State, t: u16, k: u8, id: u32, wet: bool) -> f32 {
    let live = s.container.live.get(&id).filter(|_| s.container.at == (t, k)).map(|l| if wet { l.0 } else { l.1 });
    live.or_else(|| level(&s.session, t, k, id, wet)).unwrap_or(0.0)
}

/// A level knob turned (`State::set_knob`): the list takes the level, no undo step (a drag's is taken at its press), and
/// the knob shows it until the chain plays it.
pub fn put_level(s: &mut State, t: u16, k: u8, id: u32, wet: bool, v: f32) {
    crate::follow::around(s, |s| s.session.set_knob(KnobId::Member(t, k, id, wet), v));
}

/// The open list's level knob shows `v` until the chain plays it (`put_level`).
pub fn show_level(s: &mut State, t: u16, k: u8, id: u32, wet: bool, v: f32) {
    if s.container.at == (t, k) {
        if let Some(l) = s.container.live.get_mut(&id) {
            *(if wet { &mut l.0 } else { &mut l.1 }) = v;
        }
    }
}

/// The right-click menu of a member's level knob: Reset, then its level's parameter menu (`params::level_items`).
pub fn knob_menu(s: &State, id: KnobId, x: f32, y: f32) -> Option<Popup> {
    let KnobId::Member(t, k, m, wet) = id else { return None };
    let mut items = vec![PItem::new("Reset", Cmd::Reset(crate::knobs::Control::Knob(id)))];
    items.extend(crate::params_ui::level_items(s, level_automation(&s.session, t, k, m, wet), crate::typein::Target::Member(t, k, m, wet)));
    Some(Popup { x, y, items, sub: None, scroll: [0; 2] })
}

/// Whether slot `slot` of the current track can be wrapped: a hosted effect, or (for Wrap all) a container.
pub fn can_wrap(app: &App, slot: u8, all: bool) -> bool {
    let t = app.state.selected_mixer;
    app.state.session.plugins.hosted.contains_key(&fx_key(t, slot)) || (all && members(&app.state.session, t, slot).is_some())
}

/// Whether rack row `row`'s instrument can be wrapped: a plugin channel whose plugin is hosted (not a sampler, whose
/// sample is the channel's).
pub fn can_wrap_channel(s: &State, row: usize) -> bool {
    let ch = s.session.project.channels.get(row);
    ch.is_some_and(|c| c.kind == os_model::ChannelKind::Plugin && !is_container_channel(c)) && s.session.hosted_rows.contains(&row)
}

/// Sets the wet (`wet`) or dry level of member `id` of the container in slot `k` of track `t` to `v` (0..1): one undo
/// step. The chain takes the level on the next tick, keeping its instances (`App::sync_containers`).
pub fn set_level(s: &mut State, t: u16, k: u8, id: u32, wet: bool, v: f32) {
    if leveled(&s.session, t, k, id, wet, v).is_none() {
        return;
    }
    if let Some(c) = level_cmd(s, t, k, id, wet, v) {
        member_edit(s, LEVEL, None, false, c);
    }
}

/// The id of the slot or channel holding the container at place (`t`, `k`), for commands.
fn container_id(s: &mut State, t: u16, k: u8) -> Option<u32> {
    crate::mixer::track_ids(s);
    crate::edit::container::place_id(&s.session, t, k).map(|i| i.0)
}

/// `Command::SetMember` on member `member` of the container at place (`t`, `k`) setting nothing yet.
fn member_cmd(s: &mut State, t: u16, k: u8, member: u32) -> Option<crate::control::command::Command> {
    let container = container_id(s, t, k)?;
    Some(crate::control::command::Command::SetMember { container, member, wet: None, dry: None, name: None, color: None, icon: None, auto_bypass: None, threaded: None })
}

/// `member_cmd` setting the wet (`wet`) or dry level to `v`.
fn level_cmd(s: &mut State, t: u16, k: u8, id: u32, wet: bool, v: f32) -> Option<crate::control::command::Command> {
    let mut c = member_cmd(s, t, k, id)?;
    if let crate::control::command::Command::SetMember { wet: w, dry: d, .. } = &mut c {
        *(if wet { w } else { d }) = Some(f64::from(v));
    }
    Some(c)
}

/// A container edit: command `c` as one undo step `name` (`run`: a controller's run joins it; `knob`: none while Undo
/// knob tweaks is off). The chain follows on the next tick, keeping its instances (`App::sync_containers`).
fn member_edit(s: &mut State, name: &'static str, run: Option<crate::history::Run>, knob: bool, c: crate::control::command::Command) -> Option<String> {
    let r = if knob { crate::control_ui::knob_command(s, name, run, vec![c]) } else { crate::control_ui::command(s, name, run, vec![c]) };
    r.err()
}

/// `set_level` from a controller (a remote link): one undo step while the same level keeps moving, none while Undo
/// knob tweaks is off, as a knob's.
pub fn set_level_run(s: &mut State, t: u16, k: u8, id: u32, wet: bool, v: f32) {
    if leveled(&s.session, t, k, id, wet, v).is_none() {
        return;
    }
    let run = crate::history::Run::Member(t, k, id, wet);
    if let Some(c) = level_cmd(s, t, k, id, wet, v) {
        member_edit(s, LEVEL, Some(run), true, c);
    }
}

/// Member `id` of the container at place (`t`, `k`) as `f` changes its own settings (its CPU switches, name, colour or
/// icon; spec: plugins/patcher/README.md §4.5): one undo step `name`, none when nothing changed. Saved with the
/// container; the chain takes an Auto-bypass switch on the next tick, keeping its instances.
pub fn edit_own(s: &mut State, t: u16, k: u8, id: u32, name: &'static str, f: impl FnOnce(&mut Member)) {
    let Some(was) = members(&s.session, t, k).and_then(|l| l.into_iter().find(|m| m.id == id)) else { return };
    let mut m = was.clone();
    f(&mut m);
    let Some(mut c) = member_cmd(s, t, k, id) else { return };
    if let crate::control::command::Command::SetMember { name: n, color, icon, auto_bypass, threaded, .. } = &mut c {
        if m.label != was.label {
            *n = Some(m.label.clone().unwrap_or_default());
        }
        if m.color != was.color {
            *color = Some(crate::control::command::color_text(m.color).unwrap_or_default());
        }
        if m.icon != was.icon {
            *icon = Some(crate::icon::ICONS.iter().find(|(_, i)| Some(*i) == m.icon).map_or(String::new(), |(n, _)| n.to_string()));
        }
        if m.cpu.smart_disable != was.cpu.smart_disable {
            *auto_bypass = Some(m.cpu.smart_disable);
        }
        if m.cpu.threaded != was.cpu.threaded {
            *threaded = Some(m.cpu.threaded);
        }
    }
    if let Some(e) = member_edit(s, name, None, false, c) {
        s.message = Some(e);
    }
}

/// A name typed for member `id` (the inline box of Rename, color and icon...): an empty one, or its plugin's name,
/// clears it.
pub fn rename(s: &mut State, t: u16, k: u8, id: u32, text: &str) {
    let text = text.trim();
    edit_own(s, t, k, id, NAME, |m| m.label = (!text.is_empty() && text != m.name).then(|| text.to_string()));
}

/// The slot menu's Container ▸ for a container in slot `slot` of the current track: its members, "1. Lemon Limiter
/// (wet 100%, dry 0%)", each opening its window, then Edit list... (the container's window, `show`).
pub fn list_items(s: &State, slot: u8) -> Option<Vec<PItem>> {
    let m = members(&s.session, s.selected_mixer, slot)?;
    let mx = |i: usize, op: Op| Cmd::Mixer(crate::mixer::MCmd::Member(slot, i, op));
    let mut v: Vec<PItem> = m.iter().enumerate().map(|(i, m)| PItem::new(label(i, m), mx(i, Op::Open))).collect();
    if v.is_empty() {
        v.push(PItem::new("(empty)", Cmd::Nothing).off());
    }
    v.push(PItem::sep());
    v.push(PItem::new("Edit list...", Cmd::Mixer(crate::mixer::MCmd::ContainerList(slot))));
    Some(v)
}

/// Member `i`'s menu in the container's window, on the container in slot `slot` of track `t` (its Move and Remove act on
/// the selected track's: `on_track` for another): Open editor, Wet: …% ▸ and Dry: …% ▸ (the parameter menu:
/// automation, remote control, the value box), Move up, Move down, Remove.
fn member_ops(s: &State, t: u16, slot: u8, m: &[Member], i: usize) -> Vec<PItem> {
    let mx = |op: Op| Cmd::Mixer(crate::mixer::MCmd::Member(slot, i, op));
    let on = |it: PItem, yes: bool| if yes { it } else { it.off() };
    // A level's submenu: the parameter menu of a knob (automation, remote control, value), Enter value... in percents.
    let level = |id: u32, wet: bool, v: f32| {
        let label = format!("{}: {}%", if wet { "Wet" } else { "Dry" }, pct(v));
        let create = level_automation(&s.session, t, slot, id, wet);
        PItem::sub(label, crate::params_ui::level_items(s, create, crate::typein::Target::Member(t, slot, id, wet)))
    };
    let mem = &m[i];
    vec![
        PItem::new("Open editor", mx(Op::Open)),
        level(mem.id, true, mem.wet),
        level(mem.id, false, mem.dry),
        on(PItem::new("Move up", mx(Op::Up)), i > 0),
        on(PItem::new("Move down", mx(Op::Down)), i + 1 < m.len()),
        PItem::new("Remove", mx(Op::Remove)),
    ]
}

/// Points the project's automation clips, recorded pattern events, controller links and initial values (and the
/// event editor's control) on the moved plugins' parameters at the members. Returns how many different targets
/// could not move (parameters past a member's room), which stay as they are.
fn retarget(moved: &Moved, s: &mut State) -> usize {
    let mut stuck = std::collections::BTreeSet::new();
    let mut f = |t: &mut u32| match moved.target(*t) {
        Some(Some(n)) => *t = n,
        Some(None) => {
            stuck.insert(*t);
        }
        None => {}
    };
    let p = s.session.project.get_mut();
    p.automations.iter_mut().flat_map(|a| a.targets.iter_mut()).for_each(&mut f);
    p.patterns.iter_mut().flat_map(|q| q.controls.iter_mut()).for_each(|c| f(&mut c.kind));
    for l in &mut p.controller_links {
        let mut t = l.target();
        f(&mut t);
        l.set_target(t);
    }
    p.init_controls.iter_mut().for_each(|c| f(&mut c.target));
    if let Some(Some(n)) = moved.target(s.events.target) {
        s.events.target = n;
    }
    stuck.len()
}

impl App {
    /// Wrap in container and Wrap all in container (`all`) on slot `slot` of the current track as one undo step, then
    /// the remote control links of what moved follow it, in the same step (`relink`).
    pub fn wrap_slot(&mut self, slot: u8, all: bool) -> Option<String> {
        let name = if all { WRAP_ALL } else { WRAP };
        let mut moved = None;
        let msg = crate::control_ui::edit_app(self, name, crate::control::command::Region::Project, |app| {
            let (msg, m) = app.wrap_in_container(slot, all);
            moved = m;
            msg
        });
        if let Some(m) = moved {
            relink(&mut self.state.session, name, &m);
        }
        msg
    }

    /// Wrap in container (`all` = false) and Wrap all in container on slot `slot` of the current track (spec: plugins/
    /// patcher/README.md §3.3, §4.5). The slot's effect, or every effect of the track in slot order, becomes a member
    /// with the levels its slot blended it at and its slot's CPU switches; the container takes the slot, switched on at
    /// full mix, and keeps the slot's name, colour and icon; the other wrapped slots are emptied, their names, colours
    /// and icons going to their members. A slot whose plugin is not hosted stays where it is (it has no state to move); a
    /// container wrapped by Wrap all brings its members, renumbered. The automation clips, recorded pattern events,
    /// controller links and initial values on the wrapped plugins' parameters (and on a wrapped container's members)
    /// are pointed at the members (`Moved`), which are returned for the remote control links. Wrap in container on a slot
    /// whose own switch or mix a clip or link drives keeps the slot's switch and mix, its effect starting at wet 100 %,
    /// dry 0 %, so the clip blends it as before. Run inside the undo step (`wrap_slot`).
    pub fn wrap_in_container(&mut self, slot: u8, all: bool) -> (Option<String>, Option<Moved>) {
        let t = self.state.selected_mixer;
        let Some(track) = crate::mixer::track(&self.state.session, t) else { return (None, None) };
        let slots: Vec<u8> = if all { (0..10u8).filter(|&k| track.slots[usize::from(k)].is_some()).collect() } else { vec![slot] };
        // A single slot whose switch or mix is driven keeps them: they blend the container as they blended the effect.
        let keep_mix = !all && (driven(&self.state.session, target::slot(t, slot, target::SLOT_MIX)) || driven(&self.state.session, target::slot(t, slot, target::SLOT_ON)));
        let kept = (track.slot_mix[usize::from(slot)], track.slot_on[usize::from(slot)]);
        let mut list = Vec::new();
        let mut taken = Vec::new();
        let mut left = Vec::new();
        let mut moves = Vec::new();
        for k in slots {
            let (mix, on) = (track.slot_mix[usize::from(k)], track.slot_on[usize::from(k)]);
            if let Some(inner) = members(&self.state.session, t, k) {
                let first = list.len() as u32 + 1;
                moves.push(Move { from: (t, k), ids: Ids::Members(inner.iter().zip(first..).map(|(m, new)| (m.id, new)).collect()) });
                list.extend(inner);
                taken.push(k);
                continue;
            }
            let Some(h) = self.state.session.plugins.hosted.get(&fx_key(t, k)) else {
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
                m.label = crate::mixer::slot_label(&self.state.session, t, k);
                m.color = crate::mixer::slot_color(&self.state.session, t, k);
                m.icon = crate::playlist::icon_of(&self.state.session.project, crate::playlist::IconOwner::Slot(t, k));
            }
            moves.push(Move { from: (t, k), ids: Ids::Plugin(list.len() as u32 + 1) });
            list.push(m);
            taken.push(k);
        }
        if !taken.contains(&slot) {
            return (Some("This slot's plugin is not hosted: nothing to wrap".into()), None);
        }
        if list.len() > MEMBER_IDS as usize {
            return (Some(format!("A container holds at most {MEMBER_IDS} effects")), None);
        }
        for (i, m) in list.iter_mut().enumerate() {
            m.id = i as u32 + 1;
        }
        let n = list.len();
        // The slot's name, colour and icon go with the container (`clear_slot` forgets them).
        let name = crate::mixer::slot_label(&self.state.session, t, slot);
        let color = crate::mixer::slot_color(&self.state.session, t, slot);
        let icon = crate::playlist::icon_of(&self.state.session.project, crate::playlist::IconOwner::Slot(t, slot));
        for &k in &taken {
            // The hosted instances go with an undo kept (`FxReq::Delete`), the slots' names, colours and icons too.
            crate::mixer::clear_slot(&mut self.state.session, t, k);
        }
        if let Some(m) = crate::mixer::track_mut(&mut self.state.session, t) {
            m.slots[usize::from(slot)] = Some(SlotPlugin { id: Id::NONE, name: CONTAINER.into(), state: encode(&list) });
            if keep_mix {
                (m.slot_mix[usize::from(slot)], m.slot_on[usize::from(slot)]) = kept;
            }
        }
        let moved = Moved { to: (t, slot), index: 0, moves };
        let stuck = retarget(&moved, &mut self.state);
        if name.is_some() {
            crate::mixer::set_slot_label(&mut self.state.session, t, slot, name);
        }
        if color.is_some() {
            crate::mixer::set_slot_color(&mut self.state.session, t, slot, color);
        }
        if icon.is_some() {
            crate::playlist::put_icon(&mut self.state.session, crate::playlist::IconOwner::Slot(t, slot), icon);
        }
        self.state.session.dirty = true;
        let mut msg = format!("{} effect{} in a container", n, if n == 1 { "" } else { "s" });
        if !left.is_empty() {
            let list: Vec<String> = left.iter().map(|k| (k + 1).to_string()).collect();
            msg += &format!("; not hosted, left in place: slot {}", list.join(", "));
        }
        msg += &stuck_text(stuck);
        (Some(msg), Some(moved))
    }

    /// The container list's entries on member `i` of the container in slot `slot` of the current track: Open editor, or
    /// (inside the undo step, `mixer::run`) Move up, Move down, Remove. The chain follows on the next tick.
    pub fn edit_member(&mut self, slot: u8, i: usize, op: Op) -> Option<String> {
        let t = self.state.selected_mixer;
        let list = members(&self.state.session, t, slot)?;
        if i >= list.len() {
            return None;
        }
        use crate::control::command::Command;
        let member = list[i].id;
        let s = &mut self.state;
        let container = container_id(s, t, slot)?;
        let (name, c) = match op {
            Op::Open => return self.open_member(t, slot, member),
            Op::Up if i > 0 => (MOVE, Command::MoveMember { container, member, to: i - 1 }),
            Op::Down if i + 1 < list.len() => (MOVE, Command::MoveMember { container, member, to: i + 1 }),
            Op::Remove => (REMOVE, Command::RemoveMember { container, member }),
            _ => return None,
        };
        member_edit(s, name, None, false, c)
    }

    /// Add effect ▸ in the container's list: effect `index` (of `Plugins::effects`; an instrument of
    /// `Plugins::available` for `instrument`, a channel's container only) at the end of the container in slot `slot` of
    /// the current track (the row in the track's place for a channel's), from the plugin's defaults: an effect wet
    /// only, an instrument at [`INSTRUMENT_LEVELS`]. Inside the undo step (`mixer::run`).
    pub fn add_member(&mut self, slot: u8, index: usize, instrument: bool) -> Option<String> {
        let t = self.state.selected_mixer;
        members(&self.state.session, t, slot)?;
        if instrument && slot != CHANNEL {
            return None;
        }
        let plugin = if instrument { self.state.session.plugins.available.get(index)? } else { self.state.session.plugins.effects.get(index)? }.desc.uid();
        let s = &mut self.state;
        let container = container_id(s, t, slot)?;
        let name = if instrument { ADD_INSTRUMENT } else { ADD };
        member_edit(s, name, None, false, crate::control::command::Command::AddMember { container, plugin, instrument })
    }

    /// A member window's Replace ▸: effect `index` (of `Plugins::effects`; an instrument of `Plugins::available` for
    /// `instrument`) in place of member `i` of the container in slot `slot` of the current track, from the plugin's
    /// defaults; the member keeps its place, id and levels, so its level's clips and links stay on it (as a slot's do
    /// when its effect is replaced). Inside the undo step (`mixer::run`).
    pub fn replace_member(&mut self, slot: u8, i: usize, index: usize, instrument: bool) -> Option<String> {
        let t = self.state.selected_mixer;
        let list = members(&self.state.session, t, slot)?;
        let plugin = if instrument { self.state.session.plugins.available.get(index)? } else { self.state.session.plugins.effects.get(index)? }.desc.uid();
        let member = list.get(i)?.id;
        let s = &mut self.state;
        let container = container_id(s, t, slot)?;
        member_edit(s, REPLACE, None, false, crate::control::command::Command::ReplaceMember { container, member, plugin, instrument })
    }

    /// The channel menu's Wrap in container (the original's Patcherize on a channel; spec: plugins/patcher/
    /// README.md §3.3, §4.4): rack row `row`'s instrument becomes the first member of a container on the channel, at
    /// [`INSTRUMENT_LEVELS`], with its state; the channel keeps its name, colour, notes, steps, mixer track and settings,
    /// which act on the container's output. One undo step: the channel takes a new id, so the instrument kept under the
    /// old one (`Plugins::keep`) comes back with an undo, as Replace's does. The clips, recorded pattern events and links
    /// on the instrument's parameters follow it onto its member (spec §4.5); those on the channel's own settings stay.
    pub fn wrap_channel(&mut self, row: usize) -> Option<String> {
        let mut moved = None;
        let msg = crate::control_ui::edit_app(self, WRAP, crate::control::command::Region::Project, |app| {
            let (msg, m) = app.wrap_channel_now(row);
            moved = m;
            msg
        });
        if let Some(m) = moved {
            relink(&mut self.state.session, WRAP, &m);
        }
        msg
    }

    fn wrap_channel_now(&mut self, row: usize) -> (Option<String>, Option<Moved>) {
        if !can_wrap_channel(&self.state, row) || !self.state.session.plugins.hosted.contains_key(&row) {
            return (Some("This channel's plugin is not hosted: nothing to wrap".into()), None);
        }
        let Some(member) = self.state.session.plugins.hosted.get(&row).and_then(|h| {
            let p = h.plugin.lock().ok()?;
            let (wet, dry) = INSTRUMENT_LEVELS;
            Some(Member { id: 1, instrument: true, uid: p.desc().uid(), name: p.desc().name.clone(), wet, dry, state: p.save_state().unwrap_or_default(), ..Member::default() })
        }) else {
            return (None, None);
        };
        let (Ok(t), Some(index)) = (u16::try_from(row), self.state.session.project.channels.get(row).map(|c| c.index)) else { return (None, None) };
        let moved = Moved { to: (t, CHANNEL), index, moves: vec![Move { from: (t, CHANNEL), ids: Ids::Plugin(1) }] };
        let name = member.name.clone();
        self.state.session.plugins.keep(row);
        self.state.session.plugins.drop_key(row);
        self.state.session.hosted_rows.retain(|&r| r != row);
        self.state.session.transport.set_instruments(self.state.session.plugins.instruments());
        let s = &mut self.state;
        let ch = &mut s.session.project.get_mut().channels[row];
        // A new id at once, before the container plays the channel.
        ch.id = Id::NONE;
        s.session.control.settle(s.session.project.get_mut());
        let ch = &mut s.session.project.get_mut().channels[row];
        ch.plugin = Some(CONTAINER.into());
        ch.plugin_state = Some(encode(&[member]));
        s.session.dirty = true;
        let stuck = retarget(&moved, s);
        show(s, t, CHANNEL);
        (Some(format!("{name} in a container{}", stuck_text(stuck))), Some(moved))
    }

    /// The browser's container entry dropped on the mixer (spec: plugins/patcher/README.md §3.3 "Shift+drop", §2.8):
    /// with Shift on a filled slot `slot` of the current track, Wrap in container on it, the same one undo step as the
    /// slot menu's; else an empty container in that slot (replacing its plugin, as an effect dropped there does) or in
    /// the track's first free slot, one step "Add effect". Lemon Studio's own: a Shift+drop on an empty slot or beside
    /// the slots is a plain drop, as there is no plugin to wrap.
    pub fn drop_container_on_slot(&mut self, slot: Option<u8>, shift: bool) -> Option<String> {
        let t = self.state.selected_mixer;
        let filled = |k: u8| crate::mixer::track(&self.state.session, t).is_some_and(|m| m.slots[usize::from(k)].is_some());
        if let Some(k) = slot.filter(|&k| shift && filled(k)) {
            return crate::mixer::run(self, crate::mixer::MCmd::Wrap(k, false));
        }
        let free = || crate::mixer::track(&self.state.session, t).and_then(|m| m.slots.iter().position(Option::is_none)).map(|k| k as u8);
        match slot.or_else(free) {
            Some(k) => crate::mixer::run(self, crate::mixer::MCmd::AddContainer(k)),
            None => Some("Every slot of this track is in use".into()),
        }
    }

    /// The browser's container entry dropped on the channel rack (spec: plugins/patcher/README.md §3.3 "Shift+drop",
    /// §2.8): with Shift on rack row `row`, the channel menu's Wrap in container on it (one undo step; a channel whose
    /// plugin cannot be wrapped says so and stays as it is); else a new channel at the bottom of the rack holding an
    /// empty container, as a generator dropped on the rack adds one, one step "Add channel". Lemon Studio's own: a
    /// Shift+drop beside the rows is a plain drop.
    pub fn drop_container_on_rack(&mut self, row: Option<usize>, shift: bool) -> Option<String> {
        if let Some(r) = row.filter(|_| shift) {
            return self.wrap_channel(r);
        }
        crate::control_ui::edit_app(self, crate::history::ADD_CHANNEL, crate::control::command::Region::Project, |app| {
            let s = &mut app.state;
            let row = crate::projects::add_plugin_channel(s.session.project.get_mut(), CONTAINER);
            s.session.project.get_mut().channels[row].plugin_state = Some(encode(&[]));
            s.selected_channel = row;
            s.rack.select_only(row);
            crate::rack::autosize(s);
            s.session.dirty = true;
            show(s, row as u16, CHANNEL);
            None
        })
    }

    /// The container window's controls (`CAct`).
    pub fn container_act(&mut self, a: CAct) {
        let (t, k) = self.state.container.at;
        let Some(list) = members(&self.state.session, t, k) else { return };
        let (x, y) = self.mouse;
        let s = &self.state;
        let mut items = match a {
            CAct::Open(i) => {
                let Some(id) = list.get(i).map(|m| m.id) else { return };
                if let Some(msg) = self.open_member(t, k, id) {
                    self.state.message = Some(msg);
                }
                return;
            }
            CAct::Menu(i) if i < list.len() => member_ops(s, t, k, &list, i),
            CAct::Menu(_) => return,
            CAct::Add if list.len() >= MAX_MEMBERS => {
                self.state.message = Some(format!("A container holds at most {MEMBER_IDS} effects"));
                return;
            }
            // The plugin palette, its picks going to this container's track (spec: ui/plugin-menu.md §6).
            CAct::Add => return self.open_plugin_palette(crate::plugin_menu::Target::Container(k), (x, y), Some(t)),
        };
        on_track(t, &mut items);
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None, scroll: [0; 2] });
    }

    /// A right click in the container's window: a member's name opens its menu, a level knob its parameter menu.
    /// Returns whether `a` was one.
    pub fn container_right(&mut self, a: &crate::app::Action) -> bool {
        use crate::app::Action;
        let (x, y) = self.mouse;
        match *a {
            Action::Container(CAct::Open(i) | CAct::Menu(i)) => self.container_act(CAct::Menu(i)),
            Action::Knob(id @ KnobId::Member(..)) => self.state.playlist.popup = knob_menu(&self.state, id, x, y),
            _ => return false,
        }
        true
    }

    /// Every tick while the container's window is shown: the levels its chain plays (an automated level moves its knob),
    /// repainted when they change. A container gone (an undo, a deleted slot) closes the window.
    fn mirror_levels(&mut self) {
        if !self.state.shown(Win::Container) {
            return;
        }
        let (t, k) = self.state.container.at;
        if members(&self.state.session, t, k).is_none() {
            self.state.hide(Win::Container);
            self.invalidate();
            return;
        }
        let Some(live) = self.state.session.plugins.containers.get(&place_key(t, k)) else { return };
        let Ok(c) = live.fx.try_lock() else { return };
        let now: BTreeMap<u32, (f32, f32)> = c.members.iter().map(|m| (m.id, (m.wet, m.dry))).collect();
        drop(c);
        if now != self.state.container.live {
            self.state.container.live = now;
            self.damage_win(Win::Container);
        }
    }

    /// Opens the window of member `id` of the container at place (`t`, `slot`).
    pub fn open_member(&mut self, t: u16, slot: u8, id: u32) -> Option<String> {
        let live = self.state.session.plugins.containers.get(&place_key(t, slot))?;
        let Some(key) = live.shape.iter().position(|s| s.0 == id).and_then(|at| live.keys[at]) else {
            return Some("This plugin is not loaded (it was not found)".into());
        };
        self.open_member_key(key)
    }

    /// Opens the window of the member hosted under `key`.
    pub fn open_member_key(&mut self, key: usize) -> Option<String> {
        #[cfg(not(plugin_windows))]
        {
            let _ = key;
            Some(crate::app::plugin_windows_off())
        }
        #[cfg(plugin_windows)]
        {
            let info = self.member_infos().remove(&key)?;
            match self.state.editor_host.open_window(&mut self.state.session.plugins, key, info) {
                Ok(()) => {
                    self.state.raise(crate::app::Win::Hosted(key));
                    None
                }
                Err(e) => Some(format!("Plugin: {e}")),
            }
        }
    }

    /// The plugin frame's title of every member hosted, by key: the member's name (the one typed for it, else its
    /// plugin's) over its container's slot's title, with the member's own icon.
    #[cfg(plugin_windows)]
    pub fn member_infos(&self) -> std::collections::HashMap<usize, crate::plugin_window::ChannelInfo> {
        let mut v = std::collections::HashMap::new();
        for (&slot_key, live) in &self.state.session.plugins.containers {
            let info = match fx_of(slot_key) {
                Some(_) => crate::mixer::fx_info(&self.state.session.project, slot_key),
                None => crate::app::channel_info(&self.state.session.project, slot_key),
            };
            let Some(info) = info else { continue };
            let list = key_place(slot_key).and_then(|(t, k)| members(&self.state.session, t, k)).unwrap_or_default();
            for ((id, _), key) in live.shape.iter().zip(&live.keys) {
                let Some(key) = key else { continue };
                let Some(h) = self.state.session.plugins.hosted.get(key) else { continue };
                let m = list.iter().find(|m| m.id == *id);
                let name = m.map_or(h.name.as_str(), Member::shown);
                v.insert(*key, crate::plugin_window::ChannelInfo { name: format!("{name} ({})", info.name), icon: m.and_then(|m| m.icon), ..info.clone() });
            }
        }
        v
    }

    /// Where member `key` plays, for the performance monitor: "Bus 2, slot 1, container", "Lemon Trio, container".
    pub fn member_place(&self, key: usize) -> Option<String> {
        let slot_key = self.state.session.plugins.containers.iter().find(|(_, l)| l.keys.contains(&Some(key)))?.0;
        Some(match key_place(*slot_key)? {
            (r, CHANNEL) => format!("{}, container", place_name(&self.state.session, r, CHANNEL)),
            (0, k) => format!("Master, slot {}, container", k + 1),
            (t, k) => format!("Bus {t}, slot {}, container", k + 1),
        })
    }

    /// Every tick: the containers played follow the project's container slots and channels (a wrap, an edit of the
    /// list, an undo, an opened project, a slot or channel moved). A container whose list changed is built again from
    /// it: a member still there (by its id and plugin) keeps its instance and window; a new one is an instance of its
    /// plugin started from its saved state, or left out of the chain when its plugin is not found (the list keeps it). A
    /// list whose levels or Auto-bypass switches alone changed sets them on the chain.
    pub fn sync_containers(&mut self) {
        if let Some(note) = self.host(sync_containers_in) {
            // After the hint of an opened project's Patcher slots (`open_patchers`), which it completes.
            self.state.message = Some(match self.state.message.take() {
                Some(m) if m.contains("Patcher: ") => format!("{m}. {note}"),
                _ => note,
            });
        }
        self.mirror_levels();
    }
}

/// The commands of the window's menus run with their container's track selected (as a slot window's do); a channel's
/// container's run with its row in the track's place, which is how they find it.
pub fn on_track(t: u16, items: &mut [PItem]) {
    for it in items.iter_mut() {
        if let Cmd::Mixer(_) = it.cmd {
            it.cmd = Cmd::SlotWin(t, Box::new(std::mem::replace(&mut it.cmd, Cmd::Nothing)));
        }
        on_track(t, &mut it.children);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::{run, MCmd};
    use crate::app::Win;

    /// The links of a serial chain through `n` nodes of [`patcher_state`], each at `level`.
    fn chain_links(n: u32, level: f32) -> Vec<(u32, u32, f32)> {
        let (input, output) = (|k: u32| 1003 + 3 * k, |k: u32| 1004 + 3 * k);
        let mut l = vec![(if n == 0 { 1001 } else { input(0) }, 1000, level)];
        l.extend((1..n).map(|k| (input(k), output(k - 1), level)));
        if n > 0 {
            l.push((1001, output(n - 1), level));
        }
        l
    }

    /// The command of entry `label` of a menu's items.
    fn cmd_of(items: &[PItem], label: &str) -> Cmd {
        items.iter().find(|i| i.label == label).unwrap_or_else(|| panic!("no {label:?}")).cmd.clone()
    }

    /// The first leaf of `items` (and their submenus) labelled `label`.
    fn leaf(items: &[PItem], label: &str) -> Option<Cmd> {
        items.iter().find_map(|i| if i.children.is_empty() { (i.label == label).then(|| i.cmd.clone()) } else { leaf(&i.children, label) })
    }

    /// The original's Patcher state built by hand (spec: plugins/patcher/state.md §1 to §5): `nodes` as (plugin, the
    /// node's own name, its state), node k with id 1002 + 3k, its audio input 1003 + 3k and output 1004 + 3k; Audio
    /// input 1000 and Audio output 1001; then `links` as (to, from, level).
    fn patcher_state(nodes: &[(&str, &str, &[u8])], links: &[(u32, u32, f32)]) -> Vec<u8> {
        let chunk = |id: u32, body: &[u8]| -> Vec<u8> { [&id.to_le_bytes()[..], &(body.len() as u64).to_le_bytes(), body].concat() };
        let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect() };
        let words = |w: &[u32]| -> Vec<u8> { w.iter().flat_map(|x| x.to_le_bytes()).collect() };
        let mut b = 4u32.to_le_bytes().to_vec();
        b.extend(chunk(1000, &[0; 64]));
        for (k, &(plugin, name, state)) in nodes.iter().enumerate() {
            let id = 1002 + 3 * k as u32;
            let mut g = chunk(5000, &words(&[2, id, 0, 0, 1, 5, 0, 0, 0, 0]));
            g.extend(chunk(5004, &utf16(plugin)));
            g.extend(chunk(5008, &utf16(plugin)));
            g.extend(chunk(5010, &utf16(name)));
            g.extend(chunk(5005, state));
            g.extend(chunk(5009, &words(&[0, 0, id + 1, 0, os_flp::patcher::AUDIO_IN, 0, 0])));
            g.extend(chunk(5009, &words(&[0, 0, id + 2, 0, os_flp::patcher::AUDIO_OUT, 0, 0])));
            g.extend(chunk(5001, &[]));
            b.extend(chunk(0, &g));
        }
        for (id, name) in [(1000, "Audio input"), (1001, "Audio output")] {
            b.extend(chunk(0, &[chunk(1400, &words(&[id, 0, 0, 7, 2, 0, 0])), chunk(1402, &utf16(name)), chunk(1401, &[])].concat()));
        }
        for &(to, from, level) in links {
            let d = [words(&[to, 0, from, 0]), level.to_le_bytes().to_vec(), words(&[2, 0, 0])].concat();
            b.extend(chunk(0, &[chunk(1300, &d), chunk(1301, &[])].concat()));
        }
        b.extend(chunk(1001, &[]));
        b
    }

    /// A tick as the program runs it for the mixer: ids for new slots, the mixer's requests, instances following ids.
    fn tick(a: &mut App) {
        a.state.session.control.settle(a.state.session.project.get_mut());
        a.mixer_tick();
        a.follow_ids(false, true);
    }

    /// An app with Lemon Studio's effects, the limiter in slot 1 and the delay in slot 2 of track 2; the two names.
    fn two_effects() -> (App, String, String) {
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.reset_hosted();
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        a.state.plugdb.rebuild(&a.state.session.plugins);
        let find = |a: &App, n: &str| a.state.session.plugins.effects.iter().position(|e| e.desc.name.contains(n)).unwrap();
        let (lim, del) = (find(&a, "Limiter"), find(&a, "Delay"));
        let names = (a.state.session.plugins.effects[lim].desc.name.clone(), a.state.session.plugins.effects[del].desc.name.clone());
        crate::mixer::select(&mut a.state, 2, false);
        run(&mut a, MCmd::AddEffect(0, lim));
        run(&mut a, MCmd::AddEffect(1, del));
        tick(&mut a);
        (a, names.0, names.1)
    }

    fn names(a: &App, slot: u8) -> Vec<String> {
        members(&a.state.session, 2, slot).unwrap_or_default().into_iter().map(|m| m.name).collect()
    }

    /// The container's list: moving a member keeps its instance; removing one drops it; Add effect ▸ adds the effect
    /// at the end from its defaults; each is one undo step, and an undo of a removal brings the member back with the
    /// state its instance had.
    #[test]
    fn edit_the_containers_list() {
        let (mut a, first, second) = two_effects();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        let keys = |a: &App| a.state.session.plugins.containers[&fx_key(2, 0)].keys.clone();
        let before = keys(&a);
        assert_eq!(label(1, &members(&a.state.session, 2, 0).unwrap()[1]), format!("2. {second} (wet 100%, dry 0%)"));
        let ops = &menu_of(&mut a, 1);
        assert_eq!(ops.iter().map(|i| i.label.as_ref()).collect::<Vec<_>>(), ["Open editor", "Wet: 100%", "Dry: 0%", "Move up", "Move down", "Remove"]);
        assert!(ops[3].enabled && !ops[4].enabled, "the last one moves up only");
        // Move up.
        crate::playlist::run(&mut a, ops[3].cmd.clone());
        tick(&mut a);
        assert_eq!(names(&a, 0), [second.clone(), first.clone()]);
        assert_eq!(keys(&a), vec![before[1], before[0]], "the instances are kept");
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(MOVE));
        assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(MOVE));
        tick(&mut a);
        assert_eq!(keys(&a), vec![before[1], before[0]], "and kept through an undo and a redo");
        // A member's state, as its window would leave it, then Remove.
        let k = before[1].unwrap();
        let state = a.state.session.plugins.hosted[&k].plugin.lock().unwrap().save_state().unwrap();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Member(0, 0, Op::Remove)));
        tick(&mut a);
        assert_eq!(names(&a, 0), [first.clone()]);
        assert!(!a.state.session.plugins.hosted.contains_key(&k), "its instance goes");
        assert_eq!(a.state.session.plugins.containers[&fx_key(2, 0)].fx.lock().unwrap().members.len(), 1);
        crate::history_ui::undo(&mut a.state);
        tick(&mut a);
        assert_eq!(names(&a, 0), [second.clone(), first.clone()]);
        assert_eq!(members(&a.state.session, 2, 0).unwrap()[0].state, state);
        // Add effect: the plugin palette of an empty slot, adding at the end.
        a.container_act(CAct::Add);
        let lim = crate::plugin_palette::cmd_of(&a.state, &first).expect("the limiter in the palette");
        assert!(matches!(&lim, Cmd::SlotWin(2, c) if matches!(**c, Cmd::Mixer(MCmd::MemberAdd(0, _)))));
        crate::playlist::run(&mut a, lim);
        tick(&mut a);
        assert_eq!(names(&a, 0), [second, first.clone(), first]);
        assert_eq!(members(&a.state.session, 2, 0).unwrap()[2].id, 3);
        // The smallest free id: a removed member's comes back to the next one added.
        let with = |ids: &[u32]| -> Vec<Member> { ids.iter().map(|&id| Member { id, ..members(&a.state.session, 2, 0).unwrap()[0].clone() }).collect() };
        assert_eq!(free_id(&with(&[2, 3])), Some(1));
        assert_eq!(free_id(&with(&[1, 2, 4])), Some(3));
        assert_eq!(free_id(&with(&(1..=MEMBER_IDS).collect::<Vec<_>>())), None, "at most 15");
        assert_eq!(a.state.session.plugins.containers[&fx_key(2, 0)].fx.lock().unwrap().members.len(), 3);
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(ADD));
        assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(ADD));
        tick(&mut a);
        // A save stores the members' states.
        let mut proj = a.state.session.project.clone();
        crate::project_plugins::store_hosted(&mut proj, &a.state.session.plugins);
        let saved = proj.mixer.iter().find(|m| m.index == 2).unwrap().slots[0].as_ref().map(|p| decode(&p.state).unwrap()).unwrap();
        assert!(saved.iter().all(|m| !m.state.is_empty()));
        // Every member window has a title.
        #[cfg(plugin_windows)]
        assert_eq!(a.member_infos().len(), 3);
    }

    /// The container window's menu of member `i`, the window showing the container in slot 1 of track 2.
    fn menu_of(a: &mut App, i: usize) -> Vec<PItem> {
        show(&mut a.state, 2, 0);
        a.container_act(CAct::Menu(i));
        a.state.playlist.popup.take().expect("the member's menu").items
    }

    /// The effects inside a container automate and link as a slot's effect does (spec: plugins/patcher/README.md §4.2):
    /// a member's parameter is a parameter of the container's slot naming the member and the index; the browser lists
    /// the member with its parameters, whose menu makes a clip on that target, links it by place and id and enters its
    /// value; the clip keeps naming the member when it moves; a controller drives it. Its levels have the parameter
    /// menu too, their clip named as the value box names them; a controller on one is one undo step while it moves.
    #[test]
    fn members_automate_and_link() {
        use crate::remote::{Link, RCmd};
        use crate::typein::Target as T;
        use crate::views::browser::{rows, Item};
        let (mut a, _, second) = two_effects();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        a.params_tick();
        let key = a.state.session.plugins.containers[&fx_key(2, 0)].keys[1].unwrap();
        assert_eq!(a.state.session.plugin_members.get(&key), Some(&(2, 0, 2)));
        let tgt = target::slot_param(2, 0, target::member_item(2, 0).unwrap());
        assert_eq!(crate::params::target_of(&a.state.session, key, 0), Some(tgt));
        assert_eq!(crate::params::key_of(&a.state.session, target::decode(tgt)), Some((key, 0)));
        assert_eq!(crate::params::target_of(&a.state.session, key, usize::from(MEMBER_WET)), None, "past the parameters' room");
        let pname = crate::params_ui::param_name(&a.state, key, 0).unwrap();

        // Show all parameters: the member's node under Effects, after its container.
        assert_eq!(a.browse_params(key), None);
        let node = format!("1:Current project/Effects/{second} in {}", crate::params::effect_label(&a.state.session, 2, 0).unwrap());
        assert_eq!(a.state.browser_sel.as_deref(), Some(node.as_str()));
        let row = rows(&a.state).iter().position(|r| r.item == Item::Param(key, 0)).expect("the parameter row");
        assert!(a.params_browser_right(row, 0.0, 0.0));
        let menu = a.state.playlist.popup.take().unwrap();
        let Cmd::CreateAutomation(t, name, _) = cmd_of(&menu.items, "Create automation clip") else { panic!() };
        assert_eq!(t, tgt);
        assert_eq!(name, format!("{second} in {} - {} - {pname}", crate::plugins::slot_label(&crate::mixer::slot_name(&a.state.session, 2, 0).unwrap()), crate::mixer::name(&a.state.session, 2)));
        let link = T::MemberParam(2, 0, 2, 0);
        assert_eq!(cmd_of(&menu.items, "MIDI learn..."), Cmd::Remote(RCmd::Link(link)));
        assert_eq!(cmd_of(&menu.items, "Enter value..."), Cmd::TypeIn(link));
        assert_eq!(crate::typein_ui::get(&a.state, link), crate::params_ui::param_value(&a.state, key, 0));
        crate::playlist::run(&mut a, Cmd::CreateAutomation(t, name.clone(), 0.5));
        assert_eq!(crate::params::driving_clip(&a.state.session, tgt), Some(a.state.session.project.channels.last().unwrap().index));
        assert_eq!(crate::init_ui::name(&a.state, tgt), name, "the clip's name, from its target");
        // Moved to the front of the list: the same member, the same target.
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Member(0, 1, Op::Up)));
        tick(&mut a);
        assert_eq!(a.state.session.plugins.containers[&fx_key(2, 0)].keys[0], Some(key));
        assert_eq!(crate::params::key_of(&a.state.session, target::decode(tgt)), Some((key, 0)));
        assert_eq!(crate::params::target_of(&a.state.session, key, 0), Some(tgt));

        // A controller on the parameter, kept by place and id.
        crate::remote::set_links(&crate::settings_file::store(), &[]);
        crate::remote::add(&crate::settings_file::store(), Link::new(0, 20, link), true);
        assert_eq!(crate::remote::links(&crate::settings_file::store()).iter().map(|l| l.target).collect::<Vec<_>>(), [link]);
        assert!(a.remote_cc("Test", 0, 20, 127));
        a.state.session.plugins.hosted[&key].plugin.lock().unwrap().idle();
        assert!((crate::params_ui::param_value(&a.state, key, 0).unwrap() - 1.0).abs() < 1e-6);
        assert!(crate::remote_ui::label(&a.state, &crate::remote::links(&crate::settings_file::store())[0]).starts_with(&format!("{second} in ")));

        // The levels: the parameter menu, a clip named as the value box names them, a controller.
        let ops = menu_of(&mut a, 0);
        let wet = &ops[1];
        assert_eq!(wet.label, "Wet: 100%");
        let level = level_target(&a.state.session, 2, 0, 2, true).unwrap();
        let caption = level_name(&a.state.session, 2, 0, 2, true);
        assert_eq!(cmd_of(&wet.children, "Create automation clip"), Cmd::CreateAutomation(level, caption.clone(), 1.0));
        assert_eq!(cmd_of(&wet.children, "MIDI learn..."), Cmd::Remote(RCmd::Link(T::Member(2, 0, 2, true))));
        assert_eq!(crate::init_ui::name(&a.state, level), caption);
        assert_eq!(level_of(&a.state.session, target::decode(level)), Some((2, 0, 2, true)));
        assert_eq!(crate::params::key_of(&a.state.session, target::decode(level)), None, "not a parameter of the effect");
        crate::remote::add(&crate::settings_file::store(), Link::new(0, 21, T::Member(2, 0, 2, false)), true);
        a.state.session.history.reset();
        assert!(a.remote_cc("Test", 0, 21, 64));
        assert!(a.remote_cc("Test", 0, 21, 127));
        assert_eq!(members(&a.state.session, 2, 0).unwrap()[0].dry, 1.0);
        assert_eq!(a.state.session.history.len(), 1, "one step while it moves");
        crate::remote::set_links(&crate::settings_file::store(), &[]);
    }

    /// The container's window (spec plugins/patcher/README.md §4.3): opening the slot shows it; each member has wet and
    /// dry knobs, a drag on one is one undo step on the list's level and the chain follows; a level the chain plays
    /// (automation) turns its knob without touching the list; a right click on a knob gives Reset and its level's
    /// parameter menu, on a name the member's menu; the window closes when its container goes. Painted, its controls
    /// are there.
    #[test]
    fn the_containers_window() {
        use crate::app::Action;
        use crate::typein::Target as T;
        let (mut a, _, second) = two_effects();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        crate::mixer::select(&mut a.state, 1, false);
        a.open_fx(2, 0);
        assert!(a.state.shown(Win::Container) && a.state.container.at == (2, 0));
        assert_eq!(member_count(&a.state.session, 2, 0), Some(2));
        tick(&mut a);
        assert_eq!(a.state.container.live.get(&2), Some(&(1.0, 0.0)), "the chain's levels");
        let (wet, dry) = (KnobId::Member(2, 0, 2, true), KnobId::Member(2, 0, 2, false));
        assert_eq!(a.state.knob_text(wet), format!("{second} wet: 100%"));
        assert_eq!((wet.target(), dry.default_value()), (T::Member(2, 0, 2, true), 0.0));

        // A drag: the list's level, one step; the chain plays it.
        a.state.session.history.reset();
        a.mouse = (500.0, 500.0);
        a.act(Action::Knob(wet));
        a.mouse_move(500.0, 600.0);
        a.mouse_move(500.0, 756.0);
        a.mouse_up();
        tick(&mut a);
        let want = 1.0 - 256.0 / 1024.0;
        assert!((members(&a.state.session, 2, 0).unwrap()[1].wet - want).abs() < 1e-5);
        assert_eq!(a.state.session.history.len(), 1, "one step for the drag");
        assert!((a.state.session.plugins.containers[&fx_key(2, 0)].fx.lock().unwrap().members[1].wet - want).abs() < 1e-5);
        assert!((a.state.knob(wet) - want).abs() < 1e-5);
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(LEVEL));
        tick(&mut a);
        assert_eq!(members(&a.state.session, 2, 0).unwrap()[1].wet, 1.0);
        assert_eq!(a.state.session.plugins.containers[&fx_key(2, 0)].fx.lock().unwrap().members[1].wet, 1.0);

        // A level the chain plays (as automation sets it) turns the knob; the list keeps its own.
        a.state.session.plugins.containers[&fx_key(2, 0)].fx.lock().unwrap().set_levels(1, 0.3, 0.7);
        tick(&mut a);
        assert!((a.state.knob(wet) - 0.3).abs() < 1e-6 && (a.state.knob(dry) - 0.7).abs() < 1e-6);
        assert_eq!(members(&a.state.session, 2, 0).unwrap()[1].wet, 1.0);

        // Right clicks: a knob's menu, a name's menu.
        assert!(a.container_right(&Action::Knob(dry)));
        let menu = a.state.playlist.popup.take().unwrap().items;
        assert_eq!(menu[0].label, "Reset");
        assert_eq!(cmd_of(&menu, "Enter value..."), Cmd::TypeIn(T::Member(2, 0, 2, false)));
        assert_eq!(cmd_of(&menu, "Create automation clip"), Cmd::CreateAutomation(level_target(&a.state.session, 2, 0, 2, false).unwrap(), level_name(&a.state.session, 2, 0, 2, false), 0.0));
        a.state.set_knob(dry, 0.4);
        a.state.session.history.reset();
        crate::playlist::run(&mut a, menu[0].cmd.clone());
        assert_eq!(members(&a.state.session, 2, 0).unwrap()[1].dry, 0.0, "Reset: back to dry 0");
        assert_eq!(a.state.session.history.newest_done(), Some(LEVEL));
        assert!(a.container_right(&Action::Container(CAct::Open(0))));
        let ops = a.state.playlist.popup.take().unwrap().items;
        assert_eq!(ops.iter().map(|i| i.label.as_ref()).collect::<Vec<_>>(), ["Open editor", "Wet: 100%", "Dry: 0%", "Move up", "Move down", "Remove"]);
        assert_eq!(ops[4].cmd, Cmd::SlotWin(2, Box::new(Cmd::Mixer(MCmd::Member(0, 0, Op::Down)))), "on the container's track");

        // Painted: the knobs, the menu buttons and Add effect are in the window.
        if let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) {
            a.state.raise(Win::Container);
            a.paint(&mut c, None);
            let win = crate::views::container::rect(&a.state);
            for act in [Action::Knob(wet), Action::Knob(dry), Action::Container(CAct::Menu(1)), Action::Container(CAct::Open(0)), Action::Container(CAct::Add)] {
                let r = a.hits.iter().rev().find(|h| h.action == act).unwrap_or_else(|| panic!("no hit for {act:?}")).rect;
                assert!(win.contains(r.cx(), r.cy()), "{act:?} in the window");
            }
        }

        // The container gone, the window goes.
        crate::playlist::run(&mut a, Cmd::SlotWin(2, Box::new(Cmd::Mixer(MCmd::SlotDelete(0)))));
        tick(&mut a);
        assert!(!a.state.shown(Win::Container));
    }

    /// An app with Lemon Studio's instruments and effects, rack row 0 played by Lemon Trio; its ticks.
    fn trio_channel() -> App {
        let mut a = App::new(None);
        a.state.session.plugins = crate::plugins::Plugins::default();
        a.state.session.plugins.available = crate::plugins::builtin();
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        a.state.session.plugins.set_rate(48000);
        a.state.plugdb.rebuild(&a.state.session.plugins);
        a.plugin_tick();
        a.plugin_tick();
        let trio = a.state.session.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        crate::plugin_menu::replace_generator(&mut a, trio, 0);
        ticks(&mut a);
        a
    }

    fn ticks(a: &mut App) {
        (0..3).for_each(|_| a.plugin_tick());
    }

    /// The channel menu's Wrap in container (spec plugins/patcher/README.md §4.4): the channel's instrument goes into a
    /// container on the channel, with its state, at wet and dry 100 %; the channel stays (its name, notes, mixer track),
    /// the container's chain plays it and its window opens; one undo step brings the instrument back, a redo the
    /// container. Add ▸ in its window offers instruments and effects; an instrument added layers over the first, an
    /// effect works on both. A member's parameters and levels are the channel's parameters naming it; its undo steps
    /// go by the channel's id. Saved to a .lemon file and opened again, the container comes back.
    #[test]
    fn wrap_a_channels_instrument() {
        use crate::typein::Target as T;
        use os_engine::Instrument;
        let mut a = App::new(None);
        a.state.session.plugins = crate::plugins::Plugins::default();
        a.state.session.plugins.available = crate::plugins::builtin();
        a.plugin_tick();
        let item = |a: &App| crate::rack::channel_menu(&a.state, 0, 0.0, 0.0).items.into_iter().find(|i| i.label == WRAP).expect("Wrap in container");
        assert!(!item(&a).enabled, "a sampler channel's sample is the channel's");
        let mut a = trio_channel();
        let before = a.state.session.project.channels[0].clone();
        let wrap = item(&a);
        assert!(wrap.enabled);
        crate::playlist::run(&mut a, wrap.cmd);
        ticks(&mut a);
        assert_eq!(a.state.session.history.newest_done(), Some(WRAP));
        let ch = &a.state.session.project.channels[0];
        assert!(is_container_channel(ch));
        assert_eq!((&ch.name, ch.mixer_track, ch.volume), (&before.name, before.mixer_track, before.volume), "the channel stays");
        let list = members(&a.state.session, 0, CHANNEL).unwrap();
        assert_eq!(list.len(), 1);
        let m = &list[0];
        assert_eq!((m.id, m.instrument, m.name.as_str(), m.wet, m.dry), (1, true, crate::osc3::LEMON_TRIO, 1.0, 1.0));
        assert!(!m.state.is_empty(), "with its state");
        assert!(!a.state.session.hosted_rows.contains(&0) && !a.state.session.plugins.hosted.contains_key(&0), "no instance under the row");
        assert!(a.state.shown(Win::Container) && a.state.container.at == (0, CHANNEL));
        assert_eq!(title(&a.state.session, 0, CHANNEL), Some(format!("Container ({})", place_name(&a.state.session, 0, CHANNEL))));
        assert!(!crate::rack::channel_menu(&a.state, 0, 0.0, 0.0).items.iter().any(|i| i.label == WRAP && i.enabled), "not twice");
        // The chain plays the channel's notes.
        let live = &a.state.session.plugins.containers[&0];
        assert!(live.channel && live.keys[0].is_some_and(|k| k >= MEMBER_KEY));
        assert!(a.state.session.plugins.instruments().iter().any(|(r, i)| *r == 0 && Arc::ptr_eq(i, &(live.fx.clone() as os_engine::SharedInstrument))));
        assert!(a.state.session.plugins.effects().is_empty(), "not a slot's");
        {
            let mut c = live.fx.lock().unwrap();
            c.note_on(0, 69, 0.8);
            let mut peak = 0.0f32;
            for _ in 0..8 {
                let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
                Instrument::process(&mut *c, 48000, &mut l, &mut r);
                peak = l.iter().chain(&r).fold(peak, |p, x| p.max(x.abs()));
            }
            assert!(peak > 0.01, "the instrument sounds through the chain: {peak}");
            Instrument::reset(&mut *c);
        }

        // Undone: the channel and its instrument come back; redone, the container.
        let wrapped = a.state.session.project.channels[0].clone();
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(WRAP));
        ticks(&mut a);
        assert_eq!(a.state.session.project.channels[0], before);
        assert!(a.state.session.plugins.containers.is_empty() && a.state.session.hosted_rows.contains(&0));
        assert_eq!(a.state.session.plugins.descriptor(0).map(|d| d.name), Some(crate::osc3::LEMON_TRIO.to_string()));
        assert!(!a.state.shown(Win::Container), "the window goes with its container");
        assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(WRAP));
        ticks(&mut a);
        assert_eq!(a.state.session.project.channels[0], wrapped);
        assert!(a.state.session.plugins.containers.contains_key(&0) && !a.state.session.plugins.hosted.contains_key(&0));

        // Add ▸: instruments and effects; an instrument at wet and dry 100 % (a layer), an effect wet only.
        a.open_channel_window(0);
        assert!(a.state.shown(Win::Container) && a.state.container.at == (0, CHANNEL));
        a.container_act(CAct::Add);
        let heads = crate::plugin_palette::rows(&a.state);
        let head = |label: &str| heads.contains(&crate::plugin_palette::Row::Head { label: label.into(), major: true });
        assert!(head("Instruments") && head("Effects"));
        let lim = a.state.session.plugins.effects.iter().find(|e| e.desc.name.contains("Limiter")).unwrap().desc.name.clone();
        let inst = crate::plugin_palette::cmd_of(&a.state, crate::osc3::LEMON_TRIO).expect("the instruments to pick");
        assert!(matches!(&inst, Cmd::SlotWin(0, c) if matches!(**c, Cmd::Mixer(MCmd::MemberAddInst(CHANNEL, _)))));
        let fx = crate::plugin_palette::cmd_of(&a.state, &lim).expect("the effects to pick");
        assert!(matches!(&fx, Cmd::SlotWin(0, c) if matches!(**c, Cmd::Mixer(MCmd::MemberAdd(CHANNEL, _)))));
        crate::playlist::run(&mut a, inst);
        assert_eq!(a.state.session.history.newest_done(), Some(ADD_INSTRUMENT));
        crate::playlist::run(&mut a, fx);
        ticks(&mut a);
        let list = members(&a.state.session, 0, CHANNEL).unwrap();
        assert_eq!(list.iter().map(|m| (m.id, m.instrument, m.wet, m.dry)).collect::<Vec<_>>(), [(1, true, 1.0, 1.0), (2, true, 1.0, 1.0), (3, false, 1.0, 0.0)]);
        let live = &a.state.session.plugins.containers[&0];
        assert_eq!(live.fx.lock().unwrap().members.len(), 3);
        assert!(matches!(live.fx.lock().unwrap().members[1].node, chain::Node::Inst(_)));

        // Parameters and levels: the channel's parameters naming the member.
        a.params_tick();
        let key = a.state.session.plugins.containers[&0].keys[0].unwrap();
        let index = a.state.session.project.channels[0].index;
        assert_eq!(a.state.session.plugin_members.get(&key), Some(&(0, CHANNEL, 1)));
        let tgt = target::channel_param(index, target::member_item(1, 0).unwrap());
        assert_eq!(crate::params::target_of(&a.state.session, key, 0), Some(tgt));
        assert_eq!(crate::params::key_of(&a.state.session, target::decode(tgt)), Some((key, 0)));
        let level = level_target(&a.state.session, 0, CHANNEL, 2, false).unwrap();
        assert_eq!(level, target::channel_param(index, target::member_item(2, target::MEMBER_DRY).unwrap()));
        assert_eq!(level_of(&a.state.session, target::decode(level)), Some((0, CHANNEL, 2, false)));
        assert_eq!(crate::typein_ui::text(&a.state, T::Member(0, CHANNEL, 2, false), false), "100%");
        assert!(crate::init_ui::name(&a.state, level).starts_with(&place_name(&a.state.session, 0, CHANNEL)));
        crate::typein_ui::accept(&mut a.state, T::Member(0, CHANNEL, 2, false), false, "25");
        assert_eq!(a.state.session.history.newest_done(), Some(LEVEL));
        ticks(&mut a);
        assert_eq!(a.state.session.plugins.containers[&0].fx.lock().unwrap().members[1].dry, 0.25, "the chain plays the level");
        // Undo steps of its own edits go by the channel's id.
        let owner = crate::control::command::member_owner(a.state.session.project.channels[0].id.0, 1).unwrap();
        assert_eq!(crate::control::command::key_owner(&a.state.session, key), Some(owner));
        assert_eq!(crate::control::command::owner_key(&a.state.session, owner), Some(key));
        assert_eq!(a.state.session.plugins.hosted[&key].owner.0, owner);
        // A member window's Replace ▸ lists instruments for an instrument.
        let mut m = crate::playlist::plugin_window_menu(key, false, 0.0, 0.0);
        crate::playlist::fill_plugin_window_menu(&a.state, key, &mut m);
        let replace = m.items.iter().find(|i| i.label == "Replace").unwrap();
        let pick = leaf(&replace.children, crate::osc3::LEMON_TRIO).expect("instruments");
        assert!(matches!(&pick, Cmd::SlotWin(0, c) if matches!(**c, Cmd::Mixer(MCmd::MemberReplaceInst(CHANNEL, 0, _)))));

        // Saved and opened again.
        let d = std::env::temp_dir().join(format!("lemon-container-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("Channel.lemon");
        a.save_to(path.clone());
        let saved = members(&a.state.session, 0, CHANNEL).unwrap();
        let mut back = App::new(None);
        back.state.session.plugins = crate::plugins::Plugins::default();
        back.state.session.plugins.available = crate::plugins::builtin();
        back.state.session.plugins.effects = crate::plugins::builtin_effects();
        back.open(path);
        ticks(&mut back);
        assert_eq!(members(&back.state.session, 0, CHANNEL), Some(saved));
        assert_eq!(back.state.session.plugins.containers[&0].fx.lock().unwrap().members.len(), 3);
        assert!(back.state.session.plugins.containers[&0].keys.iter().all(Option::is_some));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Wrapping moves the wrapped effects' automation and links onto their members (spec §4.5): an automation clip, a
    /// recorded pattern event, an internal controller link, an initial value and the remote control links on a slot's
    /// effect's parameters name its member's afterwards and keep driving the same parameter; a parameter past a
    /// member's room and the slot's own mix stay as they were. One undo puts all of them back, a redo moves them again.
    /// A container wrapped again by Wrap all brings its members' clips and links, renumbered.
    #[test]
    fn wrapping_moves_automation_and_links() {
        use crate::remote::Link;
        use crate::typein::Target as T;
        let (mut a, _, _) = two_effects();
        a.params_tick();
        let (lim, del) = (fx_key(2, 0), fx_key(2, 1));
        // The delay's parameter 0 automated by a clip, its parameter 3 by pattern events; the limiter's parameter 1 by
        // a controller, its parameter 2 by an initial value; the delay's parameter 2046 (no member room); slot 2's mix.
        let clip = target::slot_param(2, 1, 0);
        crate::playlist::run(&mut a, Cmd::CreateAutomation(clip, "Delay".into(), 0.5));
        let ch = a.state.session.project.channels.last().unwrap().index;
        let mix = target::slot(2, 1, target::SLOT_MIX);
        crate::playlist::run(&mut a, Cmd::CreateAutomation(mix, "Mix".into(), 0.5));
        let p = a.state.session.project.get_mut();
        p.patterns[0].controls.push(os_model::ControlPoint { id: os_model::Id::NONE, position: 0, kind: target::slot_param(2, 1, 3), value: 9 });
        p.controller_links.push(os_model::ControllerLink::new(0x7000 + 3 * 64, 0, target::slot_param(2, 0, 1)));
        p.init_controls.push(os_model::InitControl { head: 0, target: target::slot_param(2, 0, 2), value: 7 });
        p.init_controls.push(os_model::InitControl { head: 0, target: target::slot_param(2, 1, 2046), value: 1 });
        let links = [Link::new(0, 20, T::Param(del, 0)), Link::new(0, 21, T::Param(lim, 5)), Link::new(0, 22, T::Knob(KnobId::SlotMix(2, 1)))];
        crate::remote::set_links(&crate::settings_file::store(), &links);
        let before = a.state.session.project.clone();
        let targets = |a: &App| {
            let p = &a.state.session.project;
            (
                p.automations.iter().flat_map(|x| x.targets.clone()).collect::<Vec<_>>(),
                p.patterns[0].controls.iter().map(|c| c.kind).collect::<Vec<_>>(),
                p.controller_links.iter().map(|l| l.target()).collect::<Vec<_>>(),
                p.init_controls.iter().map(|c| c.target).collect::<Vec<_>>(),
                crate::remote::links(&crate::settings_file::store()).iter().map(|l| l.target).collect::<Vec<_>>(),
            )
        };
        let was = targets(&a);

        // Wrap all on slot 1: the limiter is member 1, the delay member 2.
        let msg = crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        assert!(msg.as_deref().is_some_and(|m| m.ends_with("1 automation target past parameter 2046 left as it was")), "{msg:?}");
        tick(&mut a);
        a.params_tick();
        let item = |id: u32, i: u16| target::slot_param(2, 0, target::member_item(id, i).unwrap());
        let moved = (
            vec![item(2, 0), mix],
            vec![item(2, 3)],
            vec![item(1, 1)],
            // (Creating the mix's clip gave the mix an initial value, which stays.)
            was.3.iter().map(|&t| if t == target::slot_param(2, 0, 2) { item(1, 2) } else { t }).collect::<Vec<_>>(),
            vec![T::MemberParam(2, 0, 2, 0), T::MemberParam(2, 0, 1, 5), T::Knob(KnobId::SlotMix(2, 1))],
        );
        assert_eq!(targets(&a), moved);
        assert_eq!(a.state.session.history.newest_done(), Some(WRAP_ALL), "one step");
        // The clip still drives the delay's parameter 0: its member's, found by its target.
        let key = a.state.session.plugins.containers[&fx_key(2, 0)].keys[1].unwrap();
        assert_eq!(crate::params::key_of(&a.state.session, target::decode(item(2, 0))), Some((key, 0)));
        assert_eq!(crate::params::driving_clip(&a.state.session, item(2, 0)), Some(ch));
        // And the remote link drives it.
        assert!(a.remote_cc("Test", 0, 20, 127));
        a.state.session.plugins.hosted[&key].plugin.lock().unwrap().idle();
        assert!((crate::params_ui::param_value(&a.state, key, 0).unwrap() - 1.0).abs() < 1e-6);

        // Undone: everything back on the slots; redone: on the members again.
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(WRAP_ALL));
        tick(&mut a);
        assert_eq!(targets(&a), was);
        assert_eq!((&a.state.session.project.automations, &a.state.session.project.patterns), (&before.automations, &before.patterns));
        assert_eq!(crate::history_ui::redo(&mut a.state).as_deref(), Some(WRAP_ALL));
        tick(&mut a);
        assert_eq!(targets(&a), moved);
        crate::history_ui::undo(&mut a.state);
        tick(&mut a);

        // The delay wrapped alone (member 1 of slot 2's container), then Wrap all on slot 1: its clip and link follow
        // it to member 2 of slot 1's container.
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(1, false)));
        tick(&mut a);
        let alone = target::slot_param(2, 1, target::member_item(1, 0).unwrap());
        assert_eq!(targets(&a).0, vec![alone, mix], "the slot's own mix stays the slot's");
        assert_eq!(targets(&a).4[0], T::MemberParam(2, 1, 1, 0));
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        assert_eq!(targets(&a).0, vec![item(2, 0), mix]);
        assert_eq!(targets(&a).4[0], T::MemberParam(2, 0, 2, 0));
        crate::remote::set_links(&crate::settings_file::store(), &[]);
    }

    /// The channel's Wrap in container moves the instrument's clips and links onto member 1 (spec §4.5); the channel's
    /// own volume clip stays the channel's; one undo puts them back.
    #[test]
    fn wrapping_a_channel_moves_its_automation() {
        use crate::remote::Link;
        use crate::typein::Target as T;
        let mut a = trio_channel();
        a.params_tick();
        let index = a.state.session.project.channels[0].index;
        let param = target::channel_param(index, 3);
        let volume = target::channel(index, target::CHANNEL_VOLUME);
        crate::playlist::run(&mut a, Cmd::CreateAutomation(param, "Param".into(), 0.5));
        crate::playlist::run(&mut a, Cmd::CreateAutomation(volume, "Volume".into(), 0.5));
        crate::remote::set_links(&crate::settings_file::store(), &[Link::new(0, 30, T::Param(0, 3)), Link::new(0, 31, T::Knob(KnobId::ChanVol(0)))]);
        let clips = |a: &App| a.state.session.project.automations.iter().flat_map(|x| x.targets.clone()).collect::<Vec<_>>();
        let links = || crate::remote::links(&crate::settings_file::store()).iter().map(|l| l.target).collect::<Vec<_>>();
        let wrap = crate::rack::channel_menu(&a.state, 0, 0.0, 0.0).items.into_iter().find(|i| i.label == WRAP).unwrap().cmd;
        crate::playlist::run(&mut a, wrap);
        ticks(&mut a);
        a.params_tick();
        assert_eq!(a.state.session.project.channels[0].index, index);
        let moved = target::channel_param(index, target::member_item(1, 3).unwrap());
        assert_eq!(clips(&a), vec![moved, volume]);
        assert_eq!(links(), vec![T::MemberParam(0, CHANNEL, 1, 3), T::Knob(KnobId::ChanVol(0))]);
        let key = a.state.session.plugins.containers[&0].keys[0].unwrap();
        assert_eq!(crate::params::key_of(&a.state.session, target::decode(moved)), Some((key, 3)));
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(WRAP));
        ticks(&mut a);
        assert_eq!(clips(&a), vec![param, volume]);
        assert_eq!(links(), vec![T::Param(0, 3), T::Knob(KnobId::ChanVol(0))]);
        crate::remote::set_links(&crate::settings_file::store(), &[]);
    }

    /// A member's own name, colour, icon and CPU switches (spec §4.5): the member window's entries set them, one undo
    /// step each; the name shows in the container's list, the window's title and clip names; Auto-bypass when silent
    /// reaches the chain without rebuilding it; Wrap all gives an emptied slot's name, colour, icon and switches to its
    /// member; all of it is saved with the container (state version 4) and opened again from a .lemon file. A version 3
    /// state opens with the defaults.
    #[test]
    fn a_members_own_name_colour_icon_and_cpu() {
        use crate::playlist::{fill_plugin_window_menu, plugin_window_menu};
        let (mut a, first, second) = two_effects();
        let red = os_model::Rgb::new(0xc8, 0x50, 0x4a);
        crate::mixer::set_slot_label(&mut a.state.session, 2, 0, Some("Glue".into()));
        crate::mixer::set_slot_color(&mut a.state.session, 2, 0, Some(red));
        if let Some(m) = crate::mixer::track_mut(&mut a.state.session, 2) {
            m.slot_cpu[0].smart_disable = true;
        }
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(1, true)));
        tick(&mut a);
        let list = members(&a.state.session, 2, 1).unwrap();
        assert_eq!((list[0].label.as_deref(), list[0].color, list[0].cpu.smart_disable), (Some("Glue"), Some(red), true), "the emptied slot's go with its effect");
        assert_eq!((list[1].label.as_deref(), list[1].cpu.smart_disable), (None, false));
        assert_eq!((list[0].shown(), list[1].shown()), ("Glue", second.as_str()));
        assert!(a.state.session.plugins.containers[&fx_key(2, 1)].fx.lock().unwrap().members[0].smart, "the chain plays the switch");
        let key = a.state.session.plugins.containers[&fx_key(2, 1)].keys[1].unwrap();
        crate::mixer::select(&mut a.state, 1, false);
        let menu = |a: &App| {
            let mut m = plugin_window_menu(key, false, 0.0, 0.0);
            fill_plugin_window_menu(&a.state, key, &mut m);
            m.items
        };
        let entry = |items: &[PItem], label: &str| items.iter().find(|i| i.label == label && !i.head).unwrap_or_else(|| panic!("{label}")).clone();
        let items = menu(&a);
        for label in ["Rename, color and icon...", "Change color...", "Random color", "Change icon...", "Allow threaded processing", "Auto-bypass when silent"] {
            assert!(entry(&items, label).enabled, "{label} works");
        }

        // Rename: the inline box, one step; shown in the list and the window's title, and in clip names.
        let rename = entry(&items, "Rename, color and icon...").cmd;
        assert_eq!(rename, Cmd::SlotWin(2, Box::new(Cmd::Mixer(MCmd::MemberRename(1, 1)))));
        crate::playlist::run(&mut a, rename);
        let e = a.state.playlist.edit.as_ref().unwrap();
        assert_eq!((e.target, e.text.as_str()), (crate::playlist::EditTarget::MemberName(2, 1, 2), second.as_str()));
        a.state.playlist.edit.as_mut().unwrap().text = "Echo".into();
        crate::playlist::commit_edit(&mut a.state);
        assert_eq!(a.state.session.history.newest_done(), Some(NAME));
        assert_eq!(label(1, &members(&a.state.session, 2, 1).unwrap()[1]), "2. Echo (wet 100%, dry 0%)");
        assert!(crate::params::owner_name(&a.state.session, key).starts_with("Echo in "));
        #[cfg(plugin_windows)]
        assert!(a.member_infos()[&key].name.starts_with("Echo ("));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(members(&a.state.session, 2, 1).unwrap()[1].label, None);
        crate::history_ui::redo(&mut a.state);

        // Change color... ▸ and Random color, Change icon... ▸: one step each.
        let colors = entry(&menu(&a), "Change color...");
        crate::playlist::run(&mut a, colors.children[1].cmd.clone());
        assert_eq!((members(&a.state.session, 2, 1).unwrap()[1].color, a.state.session.history.newest_done()), (Some(red), Some(COLOR)));
        let random = entry(&menu(&a), "Random color").cmd;
        crate::playlist::run(&mut a, random);
        assert!(members(&a.state.session, 2, 1).unwrap()[1].color.is_some());
        let icons = entry(&menu(&a), "Change icon...");
        let (name, icon) = crate::playlist::ICONS[2];
        let pick = icons.children.iter().find(|i| i.label == name).unwrap().cmd.clone();
        crate::playlist::run(&mut a, pick);
        assert_eq!((members(&a.state.session, 2, 1).unwrap()[1].icon, a.state.session.history.newest_done()), (Some(icon), Some(ICON)));
        assert_eq!(entry(&menu(&a), "Change icon...").children.iter().find(|i| i.label == name).unwrap().mark, crate::playlist::Mark::Check(true));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(members(&a.state.session, 2, 1).unwrap()[1].icon, None);
        crate::history_ui::redo(&mut a.state);

        // The CPU switches: one step each; Auto-bypass reaches the chain, keeping its instances.
        let keys = a.state.session.plugins.containers[&fx_key(2, 1)].keys.clone();
        let smart = entry(&menu(&a), "Auto-bypass when silent");
        assert_eq!((smart.cmd.clone(), smart.mark), (Cmd::PluginCpu(key, true), crate::playlist::Mark::Check(false)));
        crate::playlist::run(&mut a, smart.cmd);
        crate::playlist::run(&mut a, Cmd::PluginCpu(key, false));
        tick(&mut a);
        assert_eq!(members(&a.state.session, 2, 1).unwrap()[1].cpu, os_model::PluginCpu { threaded: false, smart_disable: true });
        assert!(a.state.session.plugins.containers[&fx_key(2, 1)].fx.lock().unwrap().members[1].smart);
        assert_eq!(a.state.session.plugins.containers[&fx_key(2, 1)].keys, keys);
        assert_eq!(entry(&menu(&a), "Allow threaded processing").mark, crate::playlist::Mark::Check(false));
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some("Threaded processing"));
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some("Auto-bypass when silent"));
        tick(&mut a);
        assert!(!a.state.session.plugins.containers[&fx_key(2, 1)].fx.lock().unwrap().members[1].smart);
        crate::history_ui::redo(&mut a.state);
        crate::history_ui::redo(&mut a.state);
        tick(&mut a);
        // Switch auto-bypass for all plugins switches the members' too (all on, then all off), one step.
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::AutoBypassAll));
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::AutoBypassAll));
        assert!(members(&a.state.session, 2, 1).unwrap().iter().all(|m| !m.cpu.smart_disable));
        crate::history_ui::undo(&mut a.state);
        assert!(members(&a.state.session, 2, 1).unwrap().iter().all(|m| m.cpu.smart_disable));
        tick(&mut a);

        // Replace ▸ starts the new effect afresh: no name, colour or icon, a new plugin's switches.
        let saved = members(&a.state.session, 2, 1).unwrap();
        let lim = a.state.session.plugins.effects.iter().position(|e| e.desc.name == first).unwrap();
        crate::playlist::run(&mut a, Cmd::SlotWin(2, Box::new(Cmd::Mixer(MCmd::MemberReplace(1, 0, lim)))));
        let m = &members(&a.state.session, 2, 1).unwrap()[0];
        assert_eq!((m.label.as_ref(), m.color, m.icon, m.cpu), (None, None, None, os_model::PluginCpu::default()));
        crate::history_ui::undo(&mut a.state);
        tick(&mut a);
        assert_eq!(members(&a.state.session, 2, 1).unwrap(), saved);

        // Saved in a .lemon file and opened again.
        let d = std::env::temp_dir().join(format!("lemon-container-own-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("Own.lemon");
        a.save_to(path.clone());
        let saved = members(&a.state.session, 2, 1).unwrap();
        let mut back = App::new(None);
        back.state.session.plugins = crate::plugins::Plugins::default();
        back.state.session.plugins.effects = crate::plugins::builtin_effects();
        back.open(path);
        tick(&mut back);
        let opened = members(&back.state.session, 2, 1).unwrap();
        assert_eq!(opened, saved);
        assert_eq!((opened[1].label.as_deref(), opened[1].icon, opened[1].cpu.smart_disable), (Some("Echo"), Some(icon), true));
        let _ = std::fs::remove_dir_all(&d);

        // A version 3 state: the members' defaults.
        let mut v3 = encode(&[Member { id: 1, uid: "clap:a".into(), name: "A".into(), wet: 1.0, ..Member::default() }]);
        v3[4..8].copy_from_slice(&3u32.to_le_bytes());
        v3.truncate(v3.len() - 16);
        let m = &decode(&v3).unwrap()[0];
        assert_eq!((m.cpu, m.label.as_ref(), m.color, m.icon), (os_model::PluginCpu::default(), None, None, None));
    }

    /// An opened project's Patcher slot whose patch is a plain serial chain becomes a container in chain order: an FL
    /// effect Lemon Studio recreates is that effect with its reader's state, a wrapped plugin the plugin found with its
    /// own state, anything else kept with its stream and passed by. A patch with link levels, and a Patcher channel,
    /// stay as they were; the hint names each.
    #[test]
    fn the_originals_serial_patches_open_as_containers() {
        crate::project_plugins::NATIVE_FIRST.with(|n| n.set(Some(false)));
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.reset_hosted();
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        let effect = |a: &App, n: &str| a.state.session.plugins.effects.iter().position(|e| e.desc.name == n).unwrap_or_else(|| panic!("{n}"));
        let (lim, delay) = (effect(&a, "Lemon Limiter"), effect(&a, "Lemon Delay"));
        let wrapped = os_flp::wrapper::encode(None, &crate::project_plugins::plugin_ref(&a.state.session.plugins.effects[delay].desc, vec![9, 9]));
        let fl_lim = [7u8; 169];
        let nodes: [(&str, &str, &[u8]); 3] = [("Fruity Limiter", "Compressor", &fl_lim), ("Fruity Squeeze", "Fruity Squeeze", &[5, 5]), (os_flp::wrapper::WRAPPER, "Echo", &wrapped)];
        let serial = patcher_state(&nodes, &chain_links(3, 1.0));
        let leveled = patcher_state(&nodes[..1], &chain_links(1, 0.5));
        let patcher = |state: &[u8]| Some(SlotPlugin { id: Id::NONE, name: os_flp::patcher::PATCHER.into(), state: state.to_vec() });
        let t = crate::mixer::track_mut(&mut a.state.session, 2).unwrap();
        (t.slots[0], t.slots[1]) = (patcher(&serial), patcher(&leveled));
        a.state.session.project.get_mut().channels[0].plugin = Some(os_flp::patcher::PATCHER.into());
        a.state.session.project.get_mut().channels[0].name = Some("Keys".into());
        let effects = a.state.session.plugins.effects.clone();
        let msg = open_patchers(&mut a.state.session, &effects).expect("a hint");
        let bus = crate::mixer::name(&a.state.session, 2);
        assert_eq!(msg, format!("Patcher: {bus}, slot 1 opened as a container; left as they were: {bus}, slot 2 (link levels other than 1.0), Keys (an instrument patch)"));
        let list = members(&a.state.session, 2, 0).expect("a container");
        let uid = |i: usize| a.state.session.plugins.effects[i].desc.uid();
        let shown: Vec<_> = list.iter().map(|m| (m.id, m.uid.clone(), m.wet, m.dry, m.label.clone())).collect();
        assert_eq!(
            shown,
            [(1, uid(lim), 1.0, 0.0, Some("Compressor".into())), (2, "native:Fruity Squeeze".into(), 1.0, 0.0, None), (3, uid(delay), 1.0, 0.0, Some("Echo".into()))]
        );
        assert_eq!(list[0].state, crate::plugins::recreation_state_from_fl("Lemon Limiter", &fl_lim).unwrap(), "the recreation's reader");
        assert_eq!((list[1].name.as_str(), list[1].state.as_slice()), ("Fruity Squeeze", &[5, 5][..]), "kept");
        assert_eq!(list[2].state, [9, 9], "the wrapped plugin's own state");
        let t = crate::mixer::track(&a.state.session, 2).unwrap();
        assert_eq!(t.slots[1], patcher(&leveled), "left as it was");
        assert_eq!(a.state.session.project.channels[0].plugin.as_deref(), Some(os_flp::patcher::PATCHER));
        // The chain plays the two plugins found and passes the other by; the hint says so after the opening's.
        a.state.message = Some(msg);
        a.sync_containers();
        let live = &a.state.session.plugins.containers[&fx_key(2, 0)];
        assert_eq!(live.keys.iter().map(Option::is_some).collect::<Vec<_>>(), [true, false, true]);
        let hint = a.state.message.clone().unwrap();
        assert!(hint.starts_with("Patcher: ") && hint.ends_with("Container: not found, left out of the chain: Fruity Squeeze"), "{hint}");

        // Audio input straight to Audio output (an unlinked surface beside it) is an empty container; 16 effects are
        // more than a container holds.
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        let empty = patcher_state(&[("Control Surface", "Surface", &[])], &[(1001, 1000, 1.0)]);
        let many: Vec<(&str, &str, &[u8])> = vec![("Fruity Limiter", "Fruity Limiter", &[][..]); MAX_MEMBERS + 1];
        let many = patcher_state(&many, &chain_links(MAX_MEMBERS as u32 + 1, 1.0));
        let t = crate::mixer::track_mut(&mut a.state.session, 2).unwrap();
        (t.slots[0], t.slots[1]) = (patcher(&empty), patcher(&many));
        let msg = open_patchers(&mut a.state.session, &[]).unwrap();
        assert!(msg.ends_with(&format!("slot 2 (more than {MAX_MEMBERS} effects)")), "{msg}");
        assert_eq!(members(&a.state.session, 2, 0), Some(vec![]));
        crate::project_plugins::NATIVE_FIRST.with(|n| n.set(None));
    }

    // spec: plugins/patcher/README.md "3.3 Patcherize (slot menu, channel menu)" (Shift+drop), "2.8 Presets and Patcherize"

    /// Wrap all in container: the track's two hosted effects become one container in the slot the command was used on,
    /// with their levels; the other slot is emptied; the chain plays the slot; one undo puts both slots and their
    /// instances back. Wrap in container on one slot does the same for that slot only.
    #[test]
    fn wrap_the_tracks_effects_in_a_container() {
        let (mut a, first, second) = two_effects();
        if let Some(m) = crate::mixer::track_mut(&mut a.state.session, 2) {
            m.slot_mix[1] = 0.5;
        }
        // The slot's name and colour go with the container; the emptied slot's go.
        crate::mixer::set_slot_label(&mut a.state.session, 2, 1, Some("Echo".into()));
        crate::mixer::set_slot_color(&mut a.state.session, 2, 0, Some(os_model::Rgb { r: 1, g: 2, b: 3 }));
        let menu = crate::mixer::slot_menu(&a, 1, 0.0, 0.0);
        let item = |label: &str| menu.items.iter().find(|i| i.label == label).unwrap().clone();
        assert!(item(WRAP).enabled && item(WRAP_ALL).enabled);
        assert_eq!(item(WRAP_ALL).cmd, Cmd::Mixer(MCmd::Wrap(1, true)));
        crate::playlist::run(&mut a, item(WRAP_ALL).cmd);
        tick(&mut a);
        let t = crate::mixer::track(&a.state.session, 2).unwrap();
        assert!(t.slots[0].is_none(), "the other slot is emptied");
        assert_eq!(t.slots[1].as_ref().map(|p| p.name.as_str()), Some(CONTAINER));
        assert_eq!((t.slot_mix[1], t.slot_on[1]), (1.0, true));
        assert_eq!(crate::mixer::slot_names_of(&a.state.session.project).get(&(2, 1)).map(String::as_str), Some("Echo"));
        assert!(crate::mixer::slot_colors_of(&a.state.session.project).is_empty());
        let list = members(&a.state.session, 2, 1).unwrap();
        assert_eq!(names(&a, 1), [first.clone(), second.clone()]);
        assert_eq!((list[0].id, list[1].id), (1, 2));
        assert_eq!((list[0].wet, list[0].dry), (1.0, 0.0));
        assert!((list[1].wet - 0.5).abs() < 1e-6 && (list[1].dry - 0.5).abs() < 1e-6);
        assert!(!a.state.session.plugins.hosted.contains_key(&fx_key(2, 0)) && !a.state.session.plugins.hosted.contains_key(&fx_key(2, 1)));
        let live = &a.state.session.plugins.containers[&fx_key(2, 1)];
        assert_eq!(live.fx.lock().unwrap().members.len(), 2);
        assert!(live.keys.iter().all(|k| k.is_some_and(|k| k >= MEMBER_KEY && a.state.session.plugins.hosted.contains_key(&k))), "members hosted under their own keys");
        assert_eq!(a.state.session.plugins.effects().iter().filter(|e| (e.0, e.1) == (2, 1)).count(), 1, "the chain plays the slot");
        assert_eq!(a.state.session.plugins.effects().len(), 1, "the members are not slots");
        // The slot menu lists the members.
        let menu = crate::mixer::slot_menu(&a, 1, 0.0, 0.0);
        let list = menu.items.iter().find(|i| i.label == "Container").expect("the list");
        assert_eq!(list.children[1].label, format!("2. {second} (wet 50%, dry 50%)"));

        // Undo: both slots and their instances come back; the members go.
        crate::history_ui::undo(&mut a.state);
        tick(&mut a);
        let t = crate::mixer::track(&a.state.session, 2).unwrap();
        assert!(t.slots[0].is_some() && t.slots[1].as_ref().is_some_and(|p| p.name != CONTAINER));
        assert_eq!(t.slot_mix[1], 0.5);
        assert!(a.state.session.plugins.containers.is_empty());
        assert!(a.state.session.plugins.hosted.contains_key(&fx_key(2, 0)) && a.state.session.plugins.hosted.contains_key(&fx_key(2, 1)));
        assert!(a.state.session.plugins.hosted.range(MEMBER_KEY..).next().is_none());

        // Wrap in container: that slot only.
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, false)));
        tick(&mut a);
        let t = crate::mixer::track(&a.state.session, 2).unwrap();
        assert_eq!(t.slots[0].as_ref().map(|p| p.name.as_str()), Some(CONTAINER));
        assert!(t.slots[1].as_ref().is_some_and(|p| p.name != CONTAINER));
        assert_eq!(members(&a.state.session, 2, 0).map(|m| m.len()), Some(1));
        assert!(a.state.session.plugins.hosted.contains_key(&fx_key(2, 1)) && a.state.session.plugins.containers.contains_key(&fx_key(2, 0)));
        // Wrap all then takes the container's member and the other effect.
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        assert_eq!(names(&a, 0), [first, second]);
        assert!(crate::mixer::track(&a.state.session, 2).unwrap().slots[1].is_none());
    }

    /// A member's wet and dry: the list's Wet: …% ▸ and Dry: …% ▸ Enter value... open the value box in percent; each
    /// typed value is one undo step; the chain takes the levels and keeps its instances.
    #[test]
    fn a_members_wet_and_dry() {
        use crate::typein::Target;
        use crate::typein_ui::accept;
        use crate::typein_ui::text;
        let (mut a, _, second) = two_effects();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        let keys = a.state.session.plugins.containers[&fx_key(2, 0)].keys.clone();
        let ops = &menu_of(&mut a, 1);
        let (wet, dry) = (Target::Member(2, 0, 2, true), Target::Member(2, 0, 2, false));
        assert_eq!((ops[1].label.as_ref(), cmd_of(&ops[1].children, "Enter value...")), ("Wet: 100%", Cmd::TypeIn(wet)));
        assert_eq!((ops[2].label.as_ref(), cmd_of(&ops[2].children, "Enter value...")), ("Dry: 0%", Cmd::TypeIn(dry)));
        assert_eq!(text(&a.state, wet, false), "100%");
        assert_eq!(crate::typein_ui::caption(&a.state, dry), format!("Value for {} - {second} dry", crate::mixer::name(&a.state.session, 2)));
        accept(&mut a.state, wet, false, "60");
        accept(&mut a.state, dry, false, "40%");
        tick(&mut a);
        let m = &members(&a.state.session, 2, 0).unwrap()[1];
        assert!((m.wet - 0.6).abs() < 1e-6 && (m.dry - 0.4).abs() < 1e-6);
        let live = &a.state.session.plugins.containers[&fx_key(2, 0)];
        assert_eq!(live.keys, keys, "the instances are kept");
        let c = live.fx.lock().unwrap();
        assert!((c.members[1].wet - 0.6).abs() < 1e-6 && (c.members[1].dry - 0.4).abs() < 1e-6, "the chain plays the levels");
        drop(c);
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(LEVEL));
        tick(&mut a);
        assert_eq!(members(&a.state.session, 2, 0).unwrap()[1].dry, 0.0);
        assert_eq!(a.state.session.plugins.containers[&fx_key(2, 0)].fx.lock().unwrap().members[1].dry, 0.0);
        assert_eq!(a.state.session.plugins.containers[&fx_key(2, 0)].keys, keys);
    }

    /// A member window's menu has a slot effect's entries (no Piano roll, no channel column): Replace ▸ puts another
    /// effect in the member's place, keeping its id and levels (one undo step); Delete... removes it from the container.
    #[test]
    fn a_member_windows_menu() {
        let (mut a, first, second) = two_effects();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        a.state.set_knob(KnobId::Member(2, 0, 2, false), 0.25);
        tick(&mut a);
        crate::mixer::select(&mut a.state, 1, false);
        let key = a.state.session.plugins.containers[&fx_key(2, 0)].keys[1].unwrap();
        let mut m = crate::playlist::plugin_window_menu(key, false, 0.0, 0.0);
        crate::playlist::fill_plugin_window_menu(&a.state, key, &mut m);
        assert!(!m.items.iter().any(|i| i.col || i.label == "Piano roll" || i.label == "Route to free mixer track"), "a slot's set");
        let replace = m.items.iter().find(|i| i.label == "Replace").unwrap();
        let lim = replace.children.iter().find(|i| i.label == first).expect("the effects to pick").cmd.clone();
        assert!(matches!(&lim, Cmd::SlotWin(2, c) if matches!(**c, Cmd::Mixer(MCmd::MemberReplace(0, 1, _)))));
        crate::playlist::run(&mut a, lim);
        tick(&mut a);
        assert_eq!(names(&a, 0), [first.clone(), first.clone()]);
        let m2 = members(&a.state.session, 2, 0).unwrap()[1].clone();
        assert_eq!((m2.id, m2.wet, m2.dry), (2, 1.0, 0.25), "its id and levels stay");
        assert_ne!(a.state.session.plugins.containers[&fx_key(2, 0)].keys[1], Some(key), "a new instance");
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(REPLACE));
        tick(&mut a);
        assert_eq!(names(&a, 0), [first.clone(), second]);
        // Delete...: out of the container.
        let key = a.state.session.plugins.containers[&fx_key(2, 0)].keys[1].unwrap();
        let mut m = crate::playlist::plugin_window_menu(key, false, 0.0, 0.0);
        crate::playlist::fill_plugin_window_menu(&a.state, key, &mut m);
        let del = m.items.iter().find(|i| i.label == "Delete...").unwrap().clone();
        assert_eq!(del.cmd, Cmd::SlotWin(2, Box::new(Cmd::Mixer(MCmd::Member(0, 1, Op::Remove)))));
        crate::playlist::run(&mut a, del.cmd);
        tick(&mut a);
        assert_eq!(names(&a, 0), [first]);
    }

    /// A member's window edits and presets are undo steps (spec §4.3): its instance's owner is the id made of its
    /// container's slot's and its own, which finds it again after the list is reordered; a parameter's undo and a
    /// state's undo reach it by that id.
    #[test]
    fn member_edits_and_presets_undo() {
        use crate::control::command::{key_owner, member_owner, owner_key, PluginSide, PLUGIN_PARAMETER, PLUGIN_PRESET};
        let (mut a, _, _) = two_effects();
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, true)));
        tick(&mut a);
        a.params_tick();
        let key = a.state.session.plugins.containers[&fx_key(2, 0)].keys[1].unwrap();
        let slot = crate::mixer::track(&a.state.session, 2).unwrap().slots[0].as_ref().unwrap().id.0;
        let owner = member_owner(slot, 2).unwrap();
        assert_eq!(a.state.session.plugins.hosted[&key].owner.0, owner, "hosted under its owner id");
        assert_eq!((owner_key(&a.state.session, owner), key_owner(&a.state.session, key)), (Some(key), Some(owner)));
        assert_eq!(member_owner(slot, 16), None);
        // Moved to the front: the same owner finds it.
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Member(0, 1, Op::Up)));
        tick(&mut a);
        assert_eq!(owner_key(&a.state.session, owner), Some(key));

        // A value typed in (or a window's gesture: the same step by owner): one step, undone on the instance.
        let id = a.state.session.plugins.param_infos(key)[0].id;
        let before = a.state.session.plugins.param_value(key, id).unwrap();
        let to = if before > 0.5 { 0.1 } else { 0.9 };
        assert!(a.state.session.plugins.hosted[&key].plugin.lock().unwrap().set_param(id, to));
        a.state.session.history.reset();
        crate::control_ui::param_set(&mut a.state, key, 0, Some(before), to);
        assert_eq!(a.state.session.history.newest_done(), Some(PLUGIN_PARAMETER));
        crate::history_ui::undo(&mut a.state);
        a.host_commands();
        assert!((a.state.session.plugins.param_value(key, id).unwrap() - before).abs() < 1e-6, "undone on the member");
        crate::history_ui::redo(&mut a.state);
        a.host_commands();
        assert!((a.state.session.plugins.param_value(key, id).unwrap() - to).abs() < 1e-6);

        // A preset: the state and the preset shown before it come back on the member (its window's Presets box names
        // the preset); no name is touched.
        let shown = std::path::PathBuf::from("C:/x/Before.fst");
        let side = PluginSide { state: a.state.session.plugins.state(key).unwrap(), preset: Some(shown.clone()), name: None, fill: 0 };
        let names = crate::mixer::slot_names_of(&a.state.session.project).clone();
        crate::control::state_edit(&mut a.state.session, key, PLUGIN_PRESET, side);
        assert_eq!(a.state.session.history.newest_done(), Some(PLUGIN_PRESET));
        assert_eq!(a.state.session.plugins.presets.get(&key), None);
        crate::history_ui::undo(&mut a.state);
        a.host_commands();
        assert_eq!(a.state.session.plugins.presets.get(&key), Some(&shown), "the state went back into the member");
        crate::history_ui::redo(&mut a.state);
        a.host_commands();
        assert_eq!(a.state.session.plugins.presets.get(&key), None, "and the redo takes the preset away again");
        assert_eq!(crate::mixer::slot_names_of(&a.state.session.project), names);
    }

    /// Wrap in container on a slot whose own mix a clip drives keeps the slot's switch and mix (the clip goes on
    /// blending the effect), the effect starting at wet 100 %, dry 0 %; without one, the mix moves into the member.
    #[test]
    fn a_driven_slot_mix_stays_on_the_slot() {
        let (mut a, _, _) = two_effects();
        if let Some(m) = crate::mixer::track_mut(&mut a.state.session, 2) {
            (m.slot_mix[0], m.slot_mix[1]) = (0.5, 0.5);
        }
        crate::playlist::run(&mut a, Cmd::CreateAutomation(target::slot(2, 1, target::SLOT_MIX), "Mix".into(), 0.5));
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(1, false)));
        crate::playlist::run(&mut a, Cmd::Mixer(MCmd::Wrap(0, false)));
        tick(&mut a);
        let t = crate::mixer::track(&a.state.session, 2).unwrap();
        assert_eq!((t.slot_mix[1], t.slot_mix[0]), (0.5, 1.0));
        assert_eq!(members(&a.state.session, 2, 1).map(|l| (l[0].wet, l[0].dry)), Some((1.0, 0.0)));
        assert!(members(&a.state.session, 2, 0).is_some_and(|l| (l[0].wet - 0.5).abs() < 1e-6 && (l[0].dry - 0.5).abs() < 1e-6));
    }

    /// The container dropped on a slot with Shift wraps its effect, the same one undo step as the slot menu's Wrap in
    /// container; dropped without Shift (or with Shift on an empty slot) it is an empty container in that slot, else in
    /// the first free one, one step "Add effect".
    #[test]
    fn a_shift_drop_on_a_slot_wraps_its_effect() {
        let (mut a, first, second) = two_effects();
        assert_eq!(a.drop_container_on_slot(Some(0), true).as_deref(), Some("1 effect in a container"));
        tick(&mut a);
        assert_eq!(a.state.session.history.newest_done(), Some(WRAP));
        assert_eq!(names(&a, 0), [first.clone()]);
        assert_eq!(crate::mixer::slot_name(&a.state.session, 2, 1).as_deref(), Some(second.as_str()), "the next slot stays as it is");
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(WRAP));
        tick(&mut a);
        assert!(members(&a.state.session, 2, 0).is_none());
        assert_eq!(crate::mixer::slot_name(&a.state.session, 2, 0).as_deref(), Some(first.as_str()));

        // Without Shift: an empty container in place of the delay.
        a.drop_container_on_slot(Some(1), false);
        tick(&mut a);
        assert_eq!(a.state.session.history.newest_done(), Some("Add effect"));
        assert_eq!(members(&a.state.session, 2, 1), Some(vec![]));
        assert!(a.state.session.plugins.containers.contains_key(&fx_key(2, 1)) && !a.state.session.plugins.hosted.contains_key(&fx_key(2, 1)));
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some("Add effect"));
        tick(&mut a);
        assert_eq!(crate::mixer::slot_name(&a.state.session, 2, 1).as_deref(), Some(second.as_str()));
        // Beside the slots, and with Shift on an empty slot: the first free slot, the slot itself.
        a.drop_container_on_slot(None, true);
        a.drop_container_on_slot(Some(5), true);
        assert!(members(&a.state.session, 2, 2).is_some() && members(&a.state.session, 2, 5).is_some());
    }

    /// The container dropped on a channel with Shift wraps its instrument as the channel menu's Wrap in container; a
    /// plain drop on the rack adds a channel holding an empty container.
    #[test]
    fn a_shift_drop_on_a_channel_wraps_its_instrument() {
        let mut a = trio_channel();
        let rows = a.state.session.project.channels.len();
        a.drop_container_on_rack(Some(0), true);
        ticks(&mut a);
        assert_eq!(a.state.session.history.newest_done(), Some(WRAP));
        assert!(is_container_channel(&a.state.session.project.channels[0]));
        assert_eq!(members(&a.state.session, 0, CHANNEL).unwrap()[0].name, crate::osc3::LEMON_TRIO);
        assert_eq!(a.state.session.project.channels.len(), rows);
        a.drop_container_on_rack(Some(0), false);
        ticks(&mut a);
        assert_eq!(a.state.session.history.newest_done(), Some(crate::history::ADD_CHANNEL));
        assert_eq!(a.state.session.project.channels.len(), rows + 1);
        assert!(is_container_channel(&a.state.session.project.channels[rows]) && members(&a.state.session, rows as u16, CHANNEL) == Some(vec![]));
        assert_eq!(a.state.selected_channel, rows);
        assert_eq!(crate::history_ui::undo(&mut a.state).as_deref(), Some(crate::history::ADD_CHANNEL));
        assert_eq!(a.state.session.project.channels.len(), rows);
    }
}

#[cfg(test)]
mod tests_moved {
    use crate::playlist::{Cmd, PItem};
    use super::*;

    /// The state round-trips; version 1 (no ids) numbers the members; other bytes are not a container.
    #[test]
    fn the_list_is_kept() {
        let m = vec![
            Member { id: 3, instrument: true, uid: "clap:a".into(), name: "A".into(), wet: 1.0, dry: 0.0, state: vec![1, 2, 3],
                cpu: os_model::PluginCpu { threaded: false, smart_disable: true },
                label: Some("Lead".into()),
                color: Some(os_model::Rgb::new(1, 2, 3)),
                icon: Some(crate::icon::ICONS[0].1),
            },
            Member { id: 1, instrument: false, uid: "vst3:b".into(), name: "B".into(), wet: 0.25, dry: 0.75, state: vec![], ..Member::default() },
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
        assert_eq!(decode(&v1), Some(vec![Member { id: 1, instrument: false, uid: "clap:a".into(), name: "A".into(), wet: 0.5, dry: 0.5, state: vec![], ..Member::default() }]));
        assert_eq!(decode(b"LCNT"), None);
        assert_eq!(decode(&[0; 16]), None);
        assert_eq!(levels(1.0, true), (1.0, 0.0));
        assert_eq!(levels(0.3, false), (0.0, 1.0));
        let (w, d) = levels(0.5, true);
        assert!((w - 0.5).abs() < 1e-6 && (d - 0.5).abs() < 1e-6);
    }

    /// The command of entry `label` of a menu's items.
    fn cmd_of(items: &[PItem], label: &str) -> Cmd {
        items.iter().find(|i| i.label == label).unwrap_or_else(|| panic!("no {label:?}")).cmd.clone()
    }

    /// The first leaf of `items` (and their submenus) labelled `label`.
    fn leaf(items: &[PItem], label: &str) -> Option<Cmd> {
        items.iter().find_map(|i| if i.children.is_empty() { (i.label == label).then(|| i.cmd.clone()) } else { leaf(&i.children, label) })
    }

    /// The original's Patcher state built by hand (spec: plugins/patcher/state.md §1 to §5): `nodes` as (plugin, the
    /// node's own name, its state), node k with id 1002 + 3k, its audio input 1003 + 3k and output 1004 + 3k; Audio
    /// input 1000 and Audio output 1001; then `links` as (to, from, level).
    fn patcher_state(nodes: &[(&str, &str, &[u8])], links: &[(u32, u32, f32)]) -> Vec<u8> {
        let chunk = |id: u32, body: &[u8]| -> Vec<u8> { [&id.to_le_bytes()[..], &(body.len() as u64).to_le_bytes(), body].concat() };
        let utf16 = |s: &str| -> Vec<u8> { s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect() };
        let words = |w: &[u32]| -> Vec<u8> { w.iter().flat_map(|x| x.to_le_bytes()).collect() };
        let mut b = 4u32.to_le_bytes().to_vec();
        b.extend(chunk(1000, &[0; 64]));
        for (k, &(plugin, name, state)) in nodes.iter().enumerate() {
            let id = 1002 + 3 * k as u32;
            let mut g = chunk(5000, &words(&[2, id, 0, 0, 1, 5, 0, 0, 0, 0]));
            g.extend(chunk(5004, &utf16(plugin)));
            g.extend(chunk(5008, &utf16(plugin)));
            g.extend(chunk(5010, &utf16(name)));
            g.extend(chunk(5005, state));
            g.extend(chunk(5009, &words(&[0, 0, id + 1, 0, os_flp::patcher::AUDIO_IN, 0, 0])));
            g.extend(chunk(5009, &words(&[0, 0, id + 2, 0, os_flp::patcher::AUDIO_OUT, 0, 0])));
            g.extend(chunk(5001, &[]));
            b.extend(chunk(0, &g));
        }
        for (id, name) in [(1000, "Audio input"), (1001, "Audio output")] {
            b.extend(chunk(0, &[chunk(1400, &words(&[id, 0, 0, 7, 2, 0, 0])), chunk(1402, &utf16(name)), chunk(1401, &[])].concat()));
        }
        for &(to, from, level) in links {
            let d = [words(&[to, 0, from, 0]), level.to_le_bytes().to_vec(), words(&[2, 0, 0])].concat();
            b.extend(chunk(0, &[chunk(1300, &d), chunk(1301, &[])].concat()));
        }
        b.extend(chunk(1001, &[]));
        b
    }

    /// The links of a serial chain through `n` nodes of [`patcher_state`], each at `level`.
    fn chain_links(n: u32, level: f32) -> Vec<(u32, u32, f32)> {
        let (input, output) = (|k: u32| 1003 + 3 * k, |k: u32| 1004 + 3 * k);
        let mut l = vec![(if n == 0 { 1001 } else { input(0) }, 1000, level)];
        l.extend((1..n).map(|k| (input(k), output(k - 1), level)));
        if n > 0 {
            l.push((1001, output(n - 1), level));
        }
        l
    }

    // spec: plugins/patcher/state.md "7. What slice 5 can do with this"; plugins/patcher/README.md "4. Implementation slices (proposed, smallest first)" (slice 5)

    // spec: plugins/patcher/README.md "3.3 Patcherize (slot menu, channel menu)" (Shift+drop), "2.8 Presets and Patcherize"
}
