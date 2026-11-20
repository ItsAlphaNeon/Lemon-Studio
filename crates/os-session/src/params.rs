// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin parameters in the app: the parameter lists of hosted instances (shown in the browser, "Show all parameters"),
//! the last tweaked parameter (Tools / Add menus), the parameter menu, and automation clips on plugin parameters
//! (targets, names, value texts).
//!
// spec: plugins/plugin-parameters.md §3 (targets), §4 (clip names), §5 (last tweaked; §5.1, §5.2 the parameter
// menu), §6 (browse parameters); plugins/patcher/README.md §4.2 (the effects inside a container)

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use os_model::target::{self, Target};

use crate::plugins::{fx_key, fx_of, MEMBER_KEY};

/// The values of an instance's parameters, by id.
fn read_values(p: &dyn os_host::Plugin, params: &[Entry]) -> BTreeMap<u32, f64> {
    let ids: Vec<u32> = params.iter().map(|e| e.id).collect();
    ids.iter().zip(p.param_values(&ids)).filter_map(|(&id, v)| Some((id, v?))).collect()
}

/// Browser tab of the project tree, and the sections plugin nodes live in (spec: ui/browser.md §2).
pub const PROJECT_TAB: usize = 1;
pub const ROOT: &str = "1:Current project";

/// One parameter of a hosted instance, as the browser and menus show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: u32,
    pub name: String,
    /// Hidden parameters keep their index but are not listed (spec §1).
    pub hidden: bool,
    /// The default value (normalized) and the number of steps (0 = continuous), for "Set..." (§5.2).
    pub default: f64,
    pub steps: u32,
}

/// A hosted instance's parameter list in plugin order (position = automation index k).
#[derive(Debug, Clone)]
pub struct List {
    pub plugin: Weak<Mutex<dyn os_host::Plugin>>,
    pub params: Vec<Entry>,
    /// When `params_tick` last took the instance's events (`Hosted::due`).
    pub seen: crate::plugins::Look,
    /// The parameters' values as last read or sent by the plugin, by id: what an edit in the plugin's window moves
    /// from (its undo step's value).
    pub values: BTreeMap<u32, f64>,
    /// The parameters being edited in the plugin's window (a gesture begun and not ended): the value each moves from.
    pub open: BTreeMap<u32, f64>,
    /// When `values` were last read from the plugin, and whether they are to be read again (the host set a parameter
    /// whose value it does not know: `ParamUi::reread`).
    pub read_at: std::time::Instant,
    pub reread: bool,
}

/// `values` are read again at most this often when the plugin reports something other than its editor's edits (an
/// automation clip moving a parameter does on every tick).
const REREAD_EVERY: std::time::Duration = std::time::Duration::from_millis(250);


impl ParamUi {
    /// The host set parameter `id` of instance `key` to `v` (an undo, the value box, Paste): what the plugin's window
    /// moves it from next.
    pub fn set_by_host(&mut self, key: usize, id: u32, v: f64) {
        if let Some(l) = self.lists.get_mut(&key) {
            l.values.insert(id, v);
        }
    }

    /// The host set parameters of instance `key` it does not know the value of: they are read again next tick.
    pub fn reread(&mut self, key: usize) {
        if let Some(l) = self.lists.get_mut(&key) {
            l.reread = true;
        }
    }
}

/// What the app knows about plugin parameters.
#[derive(Debug, Default)]
pub struct ParamUi {
    /// By hosted key (`plugins::Plugins::hosted`).
    pub lists: BTreeMap<usize, List>,
}

impl ParamUi {
    /// Visible parameters of hosted instance `key`: (index, entry).
    pub fn visible(&self, key: usize) -> impl Iterator<Item = (usize, &Entry)> {
        self.lists.get(&key).into_iter().flat_map(|l| l.params.iter().enumerate().filter(|(_, e)| !e.hidden))
    }

    pub fn has_params(&self, key: usize) -> bool {
        self.visible(key).next().is_some()
    }


    /// The instance behind `key`, if it is still hosted.
    pub fn plugin(&self, key: usize) -> Option<Arc<Mutex<dyn os_host::Plugin>>> {
        self.lists.get(&key)?.plugin.upgrade()
    }
}

/// Commands from the parameter menus.
#[derive(Debug, Clone, PartialEq)]
pub enum PCmd {
    /// Plugin window ▸ "Show all parameters": hosted key.
    Browse(usize),
    /// "Copy value" / "Paste value" of parameter (hosted key, index).
    Copy(usize, usize),
    Paste(usize, usize),
    /// A value of the "Set..." submenu: parameter (hosted key, index), normalized value.
    Set(usize, usize, f64),
    /// "Edit automation clip...": the automation clip channel (its index) that drives the parameter.
    EditClip(u16),
}

/// The hosted key of the instance an automation target points at, and the parameter index. A container's parameter
/// (a slot's or a channel's) is its member's (`None` for a member's level, or a member whose plugin is not loaded).
pub fn key_of(s: &crate::session::Session, t: Target) -> Option<(usize, usize)> {
    if let Some((track, slot, k)) = crate::edit::container::target_place(s, t) {
        let (id, item) = target::member_of(k).filter(|m| m.1 < target::MEMBER_WET)?;
        return Some((crate::edit::container::member_key(s, crate::container::place_key(track, slot), id)?, usize::from(item)));
    }
    match t {
        Target::ChannelParam(ch, k) => Some((s.project.channels.iter().position(|c| c.index == ch)?, usize::from(k))),
        Target::SlotParam(track, slot, k) => Some((fx_key(track, slot), usize::from(k))),
        _ => None,
    }
}

/// The automation target of parameter `index` of hosted instance `key` (spec §3). A plugin inside a container's is
/// the container slot's or channel's parameter that names the member and the index (`os_model::target::member_item`).
pub fn target_of(s: &crate::session::Session, key: usize, index: usize) -> Option<u32> {
    let k = u16::try_from(index).ok().filter(|&k| k < 0x8000)?;
    if key >= MEMBER_KEY {
        let &(track, slot, id) = s.plugin_members.get(&key)?;
        return crate::edit::container::item_target(s, track, slot, target::member_item(id, k).filter(|_| k < target::MEMBER_WET)?);
    }
    Some(match fx_of(key) {
        Some((track, slot)) => target::slot_param(track, slot, k),
        None => target::channel_param(s.project.channels.get(key)?.index, k),
    })
}

/// What a remote control link and the value box take parameter `index` of instance `key` as: an effect inside a
/// container by its place and id, which keep (its hosted key is the session's), the others by key.
pub fn link_target(s: &crate::session::Session, key: usize, index: usize) -> crate::typein::Target {
    match s.plugin_members.get(&key) {
        Some(&(track, slot, id)) => crate::typein::Target::MemberParam(track, slot, id, index),
        None => crate::typein::Target::Param(key, index),
    }
}

/// Who owns hosted instance `key`, as clip names write it (spec §4): the channel name, or "<slot plugin> - <track>",
/// or for an effect inside a container "<effect> in <container> - <track>".
pub fn owner_name(s: &crate::session::Session, key: usize) -> String {
    if let Some(&(track, slot, id)) = s.plugin_members.get(&key) {
        let name = crate::edit::container::members(s, track, slot).and_then(|l| l.into_iter().find(|m| m.id == id)).map_or_else(|| "Effect".into(), |m| m.shown().to_string());
        if slot == crate::container::CHANNEL {
            return format!("{name} in Container - {}", crate::edit::container::place_name(s, track, slot));
        }
        let plugin = crate::edit::mixer::slot_name(s, track, slot).unwrap_or_else(|| format!("Slot {}", slot + 1));
        return format!("{name} in {} - {}", crate::plugins::slot_label(&plugin), crate::edit::mixer::name(s, track));
    }
    match fx_of(key) {
        Some((track, slot)) => {
            let plugin = crate::edit::mixer::slot_name(s, track, slot).unwrap_or_else(|| format!("Slot {}", slot + 1));
            format!("{} - {}", crate::plugins::slot_label(&plugin), crate::edit::mixer::name(s, track))
        }
        None => {
            let ch = s.project.channels.get(key);
            ch.and_then(|c| c.name.clone().or_else(|| c.plugin.clone())).unwrap_or_else(|| format!("Channel {}", key + 1))
        }
    }
}

/// The automation clip channel that drives target `t`: the first in rack order whose clip is linked to it.
pub fn driving_clip(s: &crate::session::Session, t: u32) -> Option<u16> {
    s.project.channels.iter().map(|c| c.index).find(|&ch| s.project.automation(ch).is_some_and(|a| a.targets.contains(&t)))
}

/// Stepped parameters with up to this many steps list every value under "Set..." (a Lemon Studio choice).
pub const SET_STEPS: u32 = 32;

/// A slot's row label under Effects: "<slot plugin> (<track name>)" (FL observed, spec §6).
pub fn effect_label(s: &crate::session::Session, track: u16, slot: u8) -> Option<String> {
    Some(format!("{} ({})", crate::edit::mixer::slot_name(s, track, slot)?, crate::edit::mixer::name(s, track)))
}

/// The menu entries for the last tweaked parameters (spec §5; `views::menus::MenuCmd::Params`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuEntry {
    /// ADD > Automation for the most recent parameter: its automation clip at once.
    Automation,
    /// TOOLS > Most recent ▸ / Second most recent ▸: the parameter's menu, at the mouse.
    Last,
    NextToLast,
}

/// What a plugin tick of the parameters leaves to the app (`App::params_tick`).
pub struct Tick {
    /// A parameter was tweaked in its window, (key, index): the hint shows it and it is the last tweaked.
    pub moved: Option<(usize, usize)>,
    /// Values for automation recording: (key, index, range, value).
    pub recorded: Vec<(usize, usize, (f64, f64), f64)>,
    /// "Create automation clip" picked in a plugin's own menu: (key, index).
    pub automate: Vec<(usize, usize)>,
}

/// `App::params_tick` in the session: the hosted instances' parameter lists kept, their editors' edits as undo steps,
/// the user's edits marked, Lemon Sampler's own settings taken by its channel.
pub fn params_in(ses: &mut crate::session::Session) -> Tick {
    let ui = &mut ses.params;
    ui.lists.retain(|k, _| ses.plugins.hosted.contains_key(k));
    let mut moved = None;
    let mut edited = Vec::new();
    let mut automate = Vec::new();
    let mut recorded = Vec::new();
    let mut stretch = Vec::new();
    let (mut seen, mut steps) = (Vec::new(), Vec::new());
    for (&key, h) in &ses.plugins.hosted {
        // Only an instance with something new is locked, and never waited for: one the audio thread is rendering
        // is read again next tick (its events stay queued).
        let editor = ses.plugins.editors_open.contains(&key);
        let Some(look) = h.due(ui.lists.get(&key).map_or(crate::plugins::Look::never(), |l| l.seen), editor) else { continue };
        // A plugin's parameters can take a few ms to read (`lock_for_tick`).
        let Some(mut p) = crate::plugins::lock_for_tick(&h.plugin, h.meter.as_deref()) else { continue };
        // The dropout log is Linux only; elsewhere the hold is only timed.
        #[cfg(target_os = "linux")]
        let t0 = crate::dropout_log::Hold::start();
        #[cfg(not(target_os = "linux"))]
        let t0 = std::time::Instant::now();
        let weak = Arc::downgrade(&h.plugin);
        let count = p.param_count();
        let stale = ui.lists.get(&key).is_none_or(|l| l.params.len() != count || !Weak::ptr_eq(&l.plugin, &weak));
        if stale {
            let params: Vec<Entry> = (0..count)
                .map(|i| match p.param_info(i) {
                    Some(info) => Entry { id: info.id, name: info.name, hidden: info.hidden, default: info.default, steps: info.steps },
                    None => Entry { id: u32::MAX, name: String::new(), hidden: true, default: 0.0, steps: 0 },
                })
                .collect();
            let values = read_values(&*p, &params);
            let now = std::time::Instant::now();
            ui.lists.insert(key, List { plugin: weak, params, seen: look, values, open: BTreeMap::new(), read_at: now, reread: false });
        }
        let events = p.take_param_events();
        if let Some(l) = ui.lists.get_mut(&key) {
            let moved_count = l.seen.count != look.count;
            l.seen = look;
            let owner = h.owner;
            // What this tick's edits in the window change, by parameter: the value each moved from and to, and
            // whether a value came without a gesture.
            let mut changed: BTreeMap<u32, (f64, f64)> = BTreeMap::new();
            let mut loose = false;
            for e in &events {
                match *e {
                    os_host::ParamEvent::Begin(id) => {
                        let from = l.values.get(&id).copied().or_else(|| p.param_value(id));
                        if let Some(from) = from {
                            l.open.entry(id).or_insert(from);
                        }
                    }
                    os_host::ParamEvent::Value(id, v) => {
                        let from = l.values.insert(id, v);
                        seen.push((owner, id, v));
                        if let (false, Some(from)) = (l.open.contains_key(&id), from) {
                            changed.entry(id).or_insert((from, v)).1 = v;
                            loose = true;
                        }
                    }
                    os_host::ParamEvent::End(id) => {
                        if let Some(from) = l.open.remove(&id) {
                            let now = l.values.get(&id).copied().unwrap_or(from);
                            changed.entry(id).or_insert((from, now)).1 = now;
                        }
                    }
                }
            }
            let changed: Vec<(u32, f64)> = changed.into_iter().filter(|(_, (from, now))| from != now).map(|(id, (from, _))| (id, from)).collect();
            if !changed.is_empty() && !owner.is_none() {
                steps.push((owner.0, changed, loose));
            }
            // The plugin reported something other than its editor's edits (a preset it loaded, a restart,
            // automation), or the host set what it doesn't know: the values are read again, so an undo moves
            // back to what was there. Never during a gesture, whose start is kept.
            let due = l.reread || (events.is_empty() && moved_count && l.read_at.elapsed() >= REREAD_EVERY);
            if !stale && due && l.open.is_empty() {
                l.values = read_values(&*p, &l.params);
                (l.read_at, l.reread) = (std::time::Instant::now(), false);
            }
        }
        let sampler = key < crate::plugins::FX_KEY && p.desc().name == crate::plugins::LEMON_SAMPLER;
        for e in events {
            if let (true, os_host::ParamEvent::Value(id, v)) = (sampler, &e) {
                if let Some(x) = ui.lists.get(&key).and_then(|l| l.params.iter().find(|x| x.id == *id)) {
                    stretch.push((key, x.name.clone(), *v));
                }
            }
            let (os_host::ParamEvent::Begin(id) | os_host::ParamEvent::Value(id, _)) = e else { continue };
            edited.push(key);
            if let Some(index) = ui.lists.get(&key).and_then(|l| l.params.iter().position(|x| x.id == id)) {
                moved = Some((key, index));
                // What automation recording takes: the value with the parameter's stated range.
                if let (os_host::ParamEvent::Value(_, v), Some(info)) = (e, p.param_info(index)) {
                    recorded.push((key, index, (info.min, info.max), v));
                }
            }
        }
        // "Create automation clip" picked in the plugin's own menu (CLAP context menu).
        for id in p.take_automation_requests() {
            if let Some(index) = ui.lists.get(&key).and_then(|l| l.params.iter().position(|x| x.id == id)) {
                automate.push((key, index));
            }
        }
        if let Some(m) = &h.meter {
            m.note_hold(t0.elapsed());
        }
        #[cfg(target_os = "linux")]
        crate::dropout_log::held(&p.desc().name, "reading its parameters", t0);
    }
    // An edit in a plugin's window is an undo step (by the owner's id, so it follows moves): a gesture's from its
    // begin to its end, values sent without one joined while they come in a row.
    for (owner, id, v) in seen {
        if !owner.is_none() {
            ses.control.saw_param(owner.0, id, v);
        }
    }
    for (owner, changed, run) in steps {
        crate::control::param_edit(ses, owner, &changed, run);
    }
    // A knob moved in a plugin's own window is the user's edit (`Plugins::mark_edited`).
    for key in edited {
        ses.plugins.mark_edited(key);
    }
    // Lemon Sampler's time-stretch knobs are the channel's own settings: the channel takes the editor's edits, so
    // its audio clips get their new length (`playlist::fit_audio`), playback and saving follow (ui/playlist.md §9c).
    // So are its root note page's: the engine applies the main pitch switch.
    for (row, name, v) in stretch {
        let Some(ch) = ses.project.get_mut().channels.get_mut(row) else { continue };
        let mut st = ch.sound.stretch;
        let mut misc = ch.misc;
        if crate::plugins::stretch_param(&name, v, &mut st) && st != ch.sound.stretch {
            ch.sound.stretch = st;
            ses.dirty = true;
        } else if crate::plugins::misc_param(&name, v, &mut misc) && misc != ch.misc {
            ch.misc = misc;
            ses.dirty = true;
        }
    }
    Tick { moved, recorded, automate }
}
