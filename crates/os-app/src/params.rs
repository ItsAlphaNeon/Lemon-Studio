// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin parameters in the app: the parameter lists of hosted instances (shown in the browser, "Browse parameters"),
//! the last tweaked parameter (Tools / Add menus), and automation clips on plugin parameters (targets, names, value
//! texts).
//!
// spec: plugins/plugin-parameters.md §3 (targets), §4 (clip names), §5 (last tweaked), §6 (browse parameters)

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use os_model::target::{self, Target};

use crate::app::{Action, App, State};
use crate::playlist::{Cmd, PItem, Popup};
use crate::plugins::{fx_key, fx_of};

/// Browser tab of the project tree, and the sections plugin nodes live in (spec: ui/browser.md §2).
const PROJECT_TAB: usize = 1;
pub const ROOT: &str = "1:Current project";

/// One parameter of a hosted instance, as the browser and menus show it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: u32,
    pub name: String,
    /// Hidden parameters keep their index but are not listed (spec §1).
    pub hidden: bool,
}

/// A hosted instance's parameter list in plugin order (position = automation index k).
#[derive(Debug, Clone)]
pub struct List {
    pub plugin: Weak<Mutex<dyn os_host::Plugin>>,
    pub params: Vec<Entry>,
    /// When `params_tick` last took the instance's events (`Hosted::due`).
    pub seen: crate::plugins::Look,
}

/// Something the user moved last (spec §5).
#[derive(Debug, Clone, PartialEq)]
pub enum Tweak {
    /// A plugin parameter changed in the plugin's own editor: hosted key, parameter index.
    Plugin { key: usize, index: usize },
    /// A host control with an automation target (knob, fader, tempo).
    Host(Action),
}

/// What the app knows about plugin parameters.
#[derive(Debug, Default)]
pub struct ParamUi {
    /// By hosted key (`plugins::Plugins::hosted`).
    pub lists: BTreeMap<usize, List>,
    /// Newest first: "Last tweaked", "Next to last tweaked".
    pub last: Vec<Tweak>,
}

impl ParamUi {
    /// Visible parameters of hosted instance `key`: (index, entry).
    pub fn visible(&self, key: usize) -> impl Iterator<Item = (usize, &Entry)> {
        self.lists.get(&key).into_iter().flat_map(|l| l.params.iter().enumerate().filter(|(_, e)| !e.hidden))
    }

    pub fn has_params(&self, key: usize) -> bool {
        self.visible(key).next().is_some()
    }

    fn note(&mut self, t: Tweak) {
        if self.last.first() == Some(&t) {
            return;
        }
        self.last.retain(|x| *x != t);
        self.last.insert(0, t);
        self.last.truncate(2);
    }

    /// The instance behind `key`, if it is still hosted.
    fn plugin(&self, key: usize) -> Option<Arc<Mutex<dyn os_host::Plugin>>> {
        self.lists.get(&key)?.plugin.upgrade()
    }
}

/// Commands from the parameter menus.
#[derive(Debug, Clone, PartialEq)]
pub enum PCmd {
    /// Plugin window ▸ "Browse parameters": hosted key.
    Browse(usize),
    /// "Copy value" / "Paste value" of parameter (hosted key, index).
    Copy(usize, usize),
    Paste(usize, usize),
}

/// The hosted key of the instance an automation target points at, and the parameter index.
pub fn key_of(s: &State, t: Target) -> Option<(usize, usize)> {
    match t {
        Target::ChannelParam(ch, k) => Some((s.project.channels.iter().position(|c| c.index == ch)?, usize::from(k))),
        Target::SlotParam(track, slot, k) => Some((fx_key(track, slot), usize::from(k))),
        _ => None,
    }
}

/// The automation target of parameter `index` of hosted instance `key` (spec §3).
pub fn target_of(s: &State, key: usize, index: usize) -> Option<u32> {
    let k = u16::try_from(index).ok().filter(|&k| k < 0x8000)?;
    Some(match fx_of(key) {
        Some((track, slot)) => target::slot_param(track, slot, k),
        None => target::channel_param(s.project.channels.get(key)?.index, k),
    })
}

/// Who owns hosted instance `key`, as clip names write it (spec §4): the channel name, or "<slot plugin> - <track>".
fn owner_name(s: &State, key: usize) -> String {
    match fx_of(key) {
        Some((track, slot)) => {
            let plugin = crate::mixer::slot_name(s, track, slot).unwrap_or_else(|| format!("Slot {}", slot + 1));
            format!("{} - {}", crate::plugins::slot_label(&plugin), crate::mixer::name(s, track))
        }
        None => {
            let ch = s.project.channels.get(key);
            ch.and_then(|c| c.name.clone().or_else(|| c.plugin.clone())).unwrap_or_else(|| format!("Channel {}", key + 1))
        }
    }
}

/// The automation clip name for parameter `index` of instance `key` (spec §4).
pub fn clip_name(s: &State, key: usize, index: usize) -> String {
    let param = s.params.lists.get(&key).and_then(|l| l.params.get(index)).map_or_else(|| format!("Parameter {}", index + 1), |e| e.name.clone());
    format!("{} - {param}", owner_name(s, key))
}

/// Current normalized value of parameter `index` of instance `key`.
fn value_of(s: &State, key: usize, index: usize) -> Option<f64> {
    let id = s.params.lists.get(&key)?.params.get(index)?.id;
    let p = s.params.plugin(key)?;
    let v = p.lock().ok()?.param_value(id);
    v
}

/// Parameter `index` of instance `key`: its name, its normalized value, and setting it (the value box, spec:
/// ui/type-in-value.md).
pub(crate) fn param_name(s: &State, key: usize, index: usize) -> Option<String> {
    Some(s.params.lists.get(&key)?.params.get(index)?.name.clone())
}

pub(crate) fn param_value(s: &State, key: usize, index: usize) -> Option<f64> {
    value_of(s, key, index)
}

pub(crate) fn set_param_value(s: &State, key: usize, index: usize, v: f64) -> bool {
    let Some(id) = s.params.lists.get(&key).and_then(|l| l.params.get(index)).map(|e| e.id) else { return false };
    s.params.plugin(key).is_some_and(|p| p.lock().is_ok_and(|mut p| p.set_param(id, v)))
}

/// "Create automation clip" for parameter `index` of instance `key`: target, clip name, current value.
pub fn automation(s: &State, key: usize, index: usize) -> Option<(u32, String, f64)> {
    Some((target_of(s, key, index)?, clip_name(s, key, index), value_of(s, key, index).unwrap_or(0.5)))
}

/// The plugin's text for normalized `v` on a plugin-parameter target ("6.0dB"), when the instance is hosted.
pub fn value_text(s: &State, t: Target, v: f64) -> Option<String> {
    let (key, index) = key_of(s, t)?;
    let id = s.params.lists.get(&key)?.params.get(index)?.id;
    let p = s.params.plugin(key)?;
    let text = p.lock().ok()?.param_text(id, v);
    text
}

/// The hint of a parameter row in the browser (spec §6): "<parameter>: <value text>".
pub fn row_hint(s: &State, key: usize, index: usize) -> String {
    let Some(e) = s.params.lists.get(&key).and_then(|l| l.params.get(index)) else { return String::new() };
    let text = s.params.plugin(key).and_then(|p| {
        let p = p.lock().ok()?;
        let v = p.param_value(e.id)?;
        p.param_text(e.id, v)
    });
    match text {
        Some(t) => format!("{}: {t}", e.name),
        None => e.name.clone(),
    }
}

/// The parameter menu FL shows on a parameter row and in TOOLS ▸ Last tweaked (FL observed, spec §5, §6): automation,
/// remote control and value entries. `param` = (hosted key, index) of a plugin parameter; `create` = what "Create
/// automation clip" makes (`None` = disabled).
fn parameter_items(s: &State, param: Option<(usize, usize)>, create: Option<(u32, String, f64)>) -> Vec<PItem> {
    // spec: ui/settings-choices.md "Advanced: Initialize controls automatically"
    let [init, delete] = crate::init::items(s, create.as_ref().map(|c| (c.0, c.2)));
    let create = match create {
        Some((tgt, name, v)) => PItem::new("Create automation clip", Cmd::CreateAutomation(tgt, name, v)),
        None => PItem::new("Create automation clip", Cmd::Nothing).off(),
    };
    let copy = match param {
        Some((key, index)) => PItem::new("Copy value", Cmd::Param(PCmd::Copy(key, index))),
        None => PItem::new("Copy value", Cmd::Nothing).off(),
    };
    let paste = match param.filter(|_| s.value_clip.is_some()) {
        Some((key, index)) => PItem::new(crate::playlist::paste_label(s), Cmd::Param(PCmd::Paste(key, index))),
        None => PItem::new("Paste value", Cmd::Nothing).off(),
    };
    vec![
        PItem::not_yet("Edit events"),
        PItem::not_yet("Edit events in new window"),
        PItem::not_yet("Edit events in piano roll"),
        init,
        delete,
        PItem::sep(),
        create,
        PItem::not_yet("Edit automation clip..."),
        PItem::head("Remote control"),
        PItem::not_yet("Link to controller..."),
        PItem::not_yet("Override global link..."),
        PItem::head("Value"),
        copy,
        paste,
        // spec: ui/type-in-value.md §8 (normalized mode only)
        match param {
            Some((key, index)) => PItem::new("Type in value...", Cmd::TypeIn(crate::typein::Target::Param(key, index))),
            None => PItem::not_yet("Type in value..."),
        },
        PItem::not_yet("Set..."),
    ]
}

/// The menu of a parameter row (browser right-click, spec §6).
pub fn param_menu(s: &State, key: usize, index: usize, x: f32, y: f32) -> Option<Popup> {
    let create = automation(s, key, index)?;
    Some(Popup { x, y, items: parameter_items(s, Some((key, index)), Some(create)), sub: None })
}

/// The browser node key of instance `key` (spec §6): Generators ▸ channel, Effects ▸ "<track> - <slot>".
pub fn node_key(s: &State, key: usize) -> Option<String> {
    Some(match fx_of(key) {
        Some((track, slot)) => format!("{ROOT}/Effects/{}", effect_label(s, track, slot)?),
        None => format!("{ROOT}/Generators/{}", crate::views::browser::channel_label(s, key)?),
    })
}

/// A slot's row label under Effects: "<slot plugin> (<track name>)" (FL observed, spec §6).
pub fn effect_label(s: &State, track: u16, slot: u8) -> Option<String> {
    Some(format!("{} ({})", crate::mixer::slot_name(s, track, slot)?, crate::mixer::name(s, track)))
}

/// Main-menu entries handled here (spec §5).
pub const ADD_LAST: &str = "Automation for last tweaked parameter";
pub const TOOLS_LAST: &str = "Last tweaked";
pub const TOOLS_NEXT: &str = "Next to last tweaked";

pub fn menu_handles(label: &str) -> bool {
    matches!(label, ADD_LAST | TOOLS_LAST | TOOLS_NEXT)
}

impl App {
    /// Every plugin tick: keeps the parameter lists of hosted instances and records the parameters their editors moved.
    pub(crate) fn params_tick(&mut self) {
        let ui = &mut self.state.params;
        ui.lists.retain(|k, _| self.plugins.hosted.contains_key(k));
        let mut moved = None;
        let mut edited = Vec::new();
        let mut automate = Vec::new();
        let mut stretch = Vec::new();
        for (&key, h) in &self.plugins.hosted {
            // Only an instance with something new is locked, and never waited for: one the audio thread is rendering
            // is read again next tick (its events stay queued).
            let Some(look) = h.due(ui.lists.get(&key).map_or(crate::plugins::Look::never(), |l| l.seen)) else { continue };
            let Ok(mut p) = h.plugin.try_lock() else { continue };
            let weak = Arc::downgrade(&h.plugin);
            let count = p.param_count();
            let stale = ui.lists.get(&key).is_none_or(|l| l.params.len() != count || !Weak::ptr_eq(&l.plugin, &weak));
            if stale {
                let params = (0..count)
                    .map(|i| match p.param_info(i) {
                        Some(info) => Entry { id: info.id, name: info.name, hidden: info.hidden },
                        None => Entry { id: u32::MAX, name: String::new(), hidden: true },
                    })
                    .collect();
                ui.lists.insert(key, List { plugin: weak, params, seen: look });
            }
            if let Some(l) = ui.lists.get_mut(&key) {
                l.seen = look;
            }
            let sampler = key < crate::plugins::FX_KEY && p.desc().name == crate::plugins::LEMON_SAMPLER;
            for e in p.take_param_events() {
                if let (true, os_host::ParamEvent::Value(id, v)) = (sampler, &e) {
                    if let Some(x) = ui.lists.get(&key).and_then(|l| l.params.iter().find(|x| x.id == *id)) {
                        stretch.push((key, x.name.clone(), *v));
                    }
                }
                let (os_host::ParamEvent::Begin(id) | os_host::ParamEvent::Value(id, _)) = e else { continue };
                edited.push(key);
                if let Some(index) = ui.lists.get(&key).and_then(|l| l.params.iter().position(|x| x.id == id)) {
                    moved = Some(Tweak::Plugin { key, index });
                }
            }
            // "Create automation clip" picked in the plugin's own menu (CLAP context menu).
            for id in p.take_automation_requests() {
                if let Some(index) = ui.lists.get(&key).and_then(|l| l.params.iter().position(|x| x.id == id)) {
                    automate.push((key, index));
                }
            }
        }
        // A knob moved in a plugin's own window is the user's edit (`Plugins::mark_edited`).
        for key in edited {
            self.plugins.mark_edited(key);
        }
        // Lemon Sampler's time-stretch knobs are the channel's own settings: the channel takes the editor's edits, so
        // its audio clips get their new length (`playlist::fit_audio`), playback and saving follow (ui/playlist.md §9c).
        // So are its root note page's: the engine applies the main pitch switch.
        for (row, name, v) in stretch {
            let Some(ch) = self.state.project.channels.get_mut(row) else { continue };
            let mut st = ch.sound.stretch;
            let mut misc = ch.misc;
            if crate::plugins::stretch_param(&name, v, &mut st) && st != ch.sound.stretch {
                ch.sound.stretch = st;
                self.state.piano.dirty = true;
            } else if crate::plugins::misc_param(&name, v, &mut misc) && misc != ch.misc {
                ch.misc = misc;
                self.state.piano.dirty = true;
            }
        }
        if let Some(t) = moved {
            self.state.params.note(t);
            self.tweak_hint();
        }
        for (key, index) in automate {
            self.state.message = self.params_automate(key, index);
            self.invalidate();
        }
    }

    /// A press on a host control: it becomes the last tweaked parameter when it has an automation target.
    pub(crate) fn note_host_tweak(&mut self, a: &Action) {
        if self.automation_target(a).is_some() {
            self.state.params.note(Tweak::Host(a.clone()));
        }
    }

    /// Target, clip name and current value of a tweak.
    fn tweak_automation(&self, t: &Tweak) -> Option<(u32, String, f64)> {
        match t {
            Tweak::Plugin { key, index } => automation(&self.state, *key, *index),
            Tweak::Host(a) => self.automation_target(a),
        }
    }

    /// The hint bar shows the plugin parameter that just moved: "<owner> - <parameter>: <value>" (spec §5).
    fn tweak_hint(&mut self) {
        let Some(Tweak::Plugin { key, index }) = self.state.params.last.first().cloned() else { return };
        let s = &self.state;
        let Some(v) = value_of(s, key, index) else { return };
        let Some(id) = s.params.lists.get(&key).and_then(|l| l.params.get(index)).map(|e| e.id) else { return };
        let text = s.params.plugin(key).and_then(|p| p.lock().ok()?.param_text(id, v)).unwrap_or_else(|| format!("{:.0}%", v * 100.0));
        self.hint = std::borrow::Cow::Owned(format!("{}: {text}", clip_name(s, key, index)));
        self.damage_rect(crate::app::HINT_RECT);
    }

    /// ADD / TOOLS menu entries for the last tweaked parameters (spec §5); returns a message.
    pub(crate) fn params_menu(&mut self, label: &str) -> Option<String> {
        let nth = usize::from(label == TOOLS_NEXT);
        let tweak = self.state.params.last.get(nth).cloned();
        let create = tweak.as_ref().and_then(|t| self.tweak_automation(t));
        if label == ADD_LAST {
            let Some((tgt, name, v)) = create else {
                return Some("No parameter tweaked yet: move a plugin control or a knob first".into());
            };
            return crate::playlist::run(self, Cmd::CreateAutomation(tgt, name, v));
        }
        // The submenu, at the mouse (the main menu has no submenus of its own): the parameter's name, then the
        // parameter menu; "No parameter" and everything disabled while nothing was tweaked (FL observed).
        let (x, y) = self.mouse;
        let param = match tweak {
            Some(Tweak::Plugin { key, index }) => Some((key, index)),
            _ => None,
        };
        let head = create.as_ref().map_or_else(|| "No parameter".to_string(), |c| c.1.clone());
        let mut items = vec![PItem { head: true, ..PItem::new(head, Cmd::Nothing) }, PItem::head("Automation")];
        items.extend(parameter_items(&self.state, param, create));
        items.extend([PItem::sep(), PItem::not_yet("Override volatile link...")]);
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
        self.invalidate();
        None
    }

    /// "Create automation clip" from a plugin's own parameter menu (host link, spec §6): parameter `index` of instance
    /// `key`. Returns a message.
    pub(crate) fn params_automate(&mut self, key: usize, index: usize) -> Option<String> {
        self.params_tick();
        let Some((tgt, name, v)) = automation(&self.state, key, index) else {
            return Some("This parameter cannot be automated".into());
        };
        crate::playlist::run(self, Cmd::CreateAutomation(tgt, name, v))
    }

    /// Parameter commands (popup menus).
    pub(crate) fn params_cmd(&mut self, c: PCmd) -> Option<String> {
        match c {
            PCmd::Browse(key) => self.browse_params(key),
            PCmd::Copy(key, index) => {
                self.state.value_clip = value_of(&self.state, key, index);
                None
            }
            PCmd::Paste(key, index) => {
                let id = self.state.params.lists.get(&key).and_then(|l| l.params.get(index)).map(|e| e.id);
                let (Some(v), Some(id)) = (self.state.value_clip, id) else { return None };
                let p = self.state.params.plugin(key)?;
                let ok = p.lock().ok()?.set_param(id, v);
                if ok {
                    self.plugins.mark_edited(key);
                }
                (!ok).then(|| "The plugin did not take the value".to_string())
            }
        }
    }

    /// "Browse parameters" (spec §6): the browser shows the instance's parameters in the Current project tab.
    pub(crate) fn browse_params(&mut self, key: usize) -> Option<String> {
        self.params_tick();
        let s = &mut self.state;
        if !s.params.has_params(key) {
            return Some("This plugin has no parameters to browse".into());
        }
        let node = node_key(s, key)?;
        s.show_browser = true;
        s.browser_tab = PROJECT_TAB;
        s.browser_search.clear();
        let section = node.rsplit_once('/').map_or(ROOT.to_string(), |(a, _)| a.to_string());
        for k in [ROOT.to_string(), section, node.clone()] {
            s.browser_open.insert(k);
        }
        s.browser_sel = Some(node);
        crate::browser::reveal(s);
        self.invalidate();
        None
    }

    /// Right-click on a browser row: a parameter row opens its automation menu (spec §6). True when handled.
    pub(crate) fn params_browser_right(&mut self, row: usize, x: f32, y: f32) -> bool {
        let rows = crate::views::browser::rows(&self.state);
        let Some(crate::views::browser::Item::Param(key, index)) = rows.get(row).map(|r| r.item.clone()) else { return false };
        self.state.playlist.popup = param_menu(&self.state, key, index, x, y);
        self.invalidate();
        true
    }

    /// A parameter row dropped on the playlist at (`track`, `tick`): an automation clip there (spec §6).
    pub(crate) fn params_drop(&mut self, key: usize, index: usize, track: u16, tick: u32) -> Option<String> {
        let (tgt, name, v) = automation(&self.state, key, index)?;
        let s = &mut self.state;
        crate::playlist::create_automation(s, tgt, &name, v);
        let arr = crate::playlist::arr_mut(s);
        if let Some(c) = arr.clips.last_mut() {
            c.position = tick;
            c.track = track;
        }
        s.piano.dirty = true;
        Some(format!("Created automation clip \"{name}\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_and_names() {
        let mut app = App::new(None);
        let s = &mut app.state;
        s.project.channels[0].name = Some("Lead".into());
        let ch = s.project.channels[0].index;
        assert_eq!(target_of(s, 0, 7), Some(target::channel_param(ch, 7)));
        assert_eq!(target_of(s, fx_key(2, 3), 35), Some(target::slot_param(2, 3, 35)));
        assert_eq!(key_of(s, Target::ChannelParam(ch, 7)), Some((0, 7)));
        assert_eq!(key_of(s, Target::SlotParam(2, 3, 35)), Some((fx_key(2, 3), 35)));
        s.params.lists.insert(0, List { plugin: Weak::<Mutex<Dummy>>::new(), params: vec![Entry { id: 9, name: "Cutoff".into(), hidden: false }], seen: crate::plugins::Look::never() });
        assert_eq!(clip_name(s, 0, 0), "Lead - Cutoff");
        // Last tweaked keeps the two newest, newest first, without repeats.
        let p = &mut s.params;
        p.note(Tweak::Plugin { key: 0, index: 0 });
        p.note(Tweak::Host(Action::Tempo));
        p.note(Tweak::Plugin { key: 0, index: 0 });
        assert_eq!(p.last, vec![Tweak::Plugin { key: 0, index: 0 }, Tweak::Host(Action::Tempo)]);
    }

    /// The built-in EQ in a mixer slot: "Browse parameters" lists its parameters in the browser, a parameter row makes
    /// an automation clip (menu or drop on the playlist) aimed at the slot parameter and named after it, clip values read
    /// as the plugin's text, and the last tweaked parameter feeds the ADD / TOOLS entries.
    #[test]
    fn slot_plugin_parameters_browse_and_automate() {
        use crate::views::browser::{rows, Item};
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.reset_hosted();
        a.plugins.effects = crate::plugins::builtin_effects();
        let eq = os_model::SlotPlugin { name: crate::plugins::EQ_FILE_NAME.into(), state: lemon_parametric_eq_2::params::default_state() };
        a.state.project.mixer[2].slots[1] = Some(eq);
        a.sync_effects();
        let key = fx_key(2, 1);
        assert_eq!(a.browse_params(key), None);
        assert_eq!(a.state.params.visible(key).count(), 36);
        let node = "1:Current project/Effects/Lemon Parametric EQ (Insert 2)";
        assert_eq!((a.state.show_browser, a.state.browser_tab, a.state.browser_sel.as_deref()), (true, PROJECT_TAB, Some(node)));
        let list = rows(&a.state);
        let names: Vec<&str> = list.iter().filter(|r| matches!(r.item, Item::Param(k, _) if k == key)).map(|r| r.label.as_str()).collect();
        assert_eq!((names.len(), names[0], names[35]), (36, "Band 1 level", "Main level"));
        assert_eq!(row_hint(&a.state, key, 0), "Band 1 level: 0.0dB");

        // Right-click a parameter row: its automation menu.
        let row = list.iter().position(|r| r.item == Item::Param(key, 35)).unwrap();
        assert!(a.params_browser_right(row, 10.0, 10.0));
        let menu = a.state.playlist.popup.take().unwrap();
        let tgt = target::slot_param(2, 1, 35);
        let create = menu.items.iter().find(|i| i.label == "Create automation clip").unwrap();
        assert_eq!(create.cmd, Cmd::CreateAutomation(tgt, "Lemon Parametric EQ - Insert 2 - Main level".into(), 0.5));
        // A plugin parameter's stored units aren't known: no initial value (spec: ui/settings-choices.md "Advanced:
        // Initialize controls automatically").
        let init = menu.items.iter().find(|i| i.label == crate::init::INIT_LABEL).unwrap();
        assert!(!init.enabled && matches!(init.cmd, Cmd::NotYet(_)));
        // Copy value (main level), then paste another value onto band 1 level.
        crate::playlist::run(&mut a, Cmd::Param(PCmd::Copy(key, 35)));
        assert_eq!(a.state.value_clip, Some(0.5));
        a.state.value_clip = Some(0.75);
        assert_eq!(crate::playlist::run(&mut a, Cmd::Param(PCmd::Paste(key, 0))), None);
        a.plugins.hosted[&key].plugin.lock().unwrap().idle();
        assert!((value_of(&a.state, key, 0).unwrap() - 0.75).abs() < 1e-6);

        // Dropped on the playlist: a clip there, aimed at the parameter.
        let msg = a.params_drop(key, 35, 3, 384);
        assert_eq!(msg.as_deref(), Some("Created automation clip \"Lemon Parametric EQ - Insert 2 - Main level\""));
        let ch = a.state.project.channels.last().unwrap().index;
        assert_eq!(a.state.project.automations.last().map(|x| (x.channel, x.target())), Some((ch, Some(tgt))));
        let clip = crate::playlist::arr_mut(&mut a.state).clips.last().cloned().unwrap();
        assert_eq!((clip.position, clip.track, clip.source), (384, 3, os_model::ClipSource::Channel(ch)));
        assert_eq!(value_text(&a.state, Target::SlotParam(2, 1, 0), 1.0).as_deref(), Some("18.0dB"));

        // Last tweaked: nothing yet ("No parameter", disabled entries), then a plugin parameter; a second clip on it
        // gets a numbered name.
        assert!(a.params_menu(ADD_LAST).is_some_and(|m| m.starts_with("No parameter")));
        assert_eq!(a.params_menu(TOOLS_LAST), None);
        let menu = a.state.playlist.popup.take().unwrap();
        assert_eq!((menu.items[0].label.as_ref(), menu.items[0].head), ("No parameter", true));
        assert!(menu.items.iter().any(|i| i.label == "Create automation clip" && !i.enabled));
        a.state.params.note(Tweak::Plugin { key, index: 35 });
        a.params_menu(ADD_LAST);
        let last = a.state.project.channels.last().unwrap();
        assert_eq!(last.name.as_deref(), Some("Lemon Parametric EQ - Insert 2 - Main level #2"));
        // A plugin's own knob menu (host link): a clip on band 1 frequency.
        a.params_automate(key, 7);
        let last = a.state.project.channels.last().unwrap();
        assert_eq!(last.name.as_deref(), Some("Lemon Parametric EQ - Insert 2 - Band 1 freq"));
        assert_eq!(a.state.project.automations.last().and_then(|x| x.target()), Some(target::slot_param(2, 1, 7)));
        a.note_host_tweak(&Action::Tempo);
        assert_eq!(a.params_menu(TOOLS_NEXT), None);
        let menu = a.state.playlist.popup.take().unwrap();
        assert!(menu.items.iter().any(|i| matches!(&i.cmd, Cmd::CreateAutomation(t, _, _) if *t == tgt)));
    }

    /// Corpus check (OPENSTUDIO_FLP_CORPUS): automation clips made on Parametric EQ 2 slot parameters load as slot
    /// parameter targets of the hosted Lemon Parametric EQ, and every clip whose name still ends in the parameter
    /// name ("... - Band 7 freq") points at the parameter of that name: the index maps 1:1 (spec §3, §7).
    #[test]
    fn corpus_eq_automation_points_at_the_same_parameters() {
        let Some(list) = std::env::var_os("OPENSTUDIO_FLP_CORPUS") else { return };
        let text = std::fs::read_to_string(list).unwrap();
        let (mut checked, mut bad) = (0, Vec::new());
        for path in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let Some(project) = std::fs::read(path).ok().and_then(|b| os_flp::FlpFile::parse(&b).ok()).and_then(|f| os_flp::project::load(&f).ok()) else {
                continue;
            };
            let eq_slot = |t: u16, k: u8| {
                let m = project.mixer.iter().find(|m| m.index == t);
                m.and_then(|m| m.slots.get(usize::from(k))?.as_ref()).is_some_and(|p| p.name == crate::plugins::EQ_FILE_NAME)
            };
            let links: Vec<(u16, Target)> = project
                .automations
                .iter()
                .filter_map(|x| Some((x.channel, target::decode(x.target()?))))
                .filter(|&(_, t)| matches!(t, Target::SlotParam(tr, sl, _) if eq_slot(tr, sl)))
                .collect();
            if links.is_empty() {
                continue;
            }
            let mut a = App::new(None);
            a.state.project = project;
            a.reset_hosted();
            a.plugins.effects = crate::plugins::builtin_effects();
            a.sync_effects();
            a.params_tick();
            for (ch, t) in links {
                let s = &a.state;
                let (key, k) = key_of(s, t).unwrap();
                let param = s.params.lists.get(&key).and_then(|l| l.params.get(k)).map(|e| e.name.clone());
                assert!(param.is_some(), "{path}: {t:?} has no hosted parameter");
                let name = s.project.channels.iter().find(|c| c.index == ch).and_then(|c| c.name.clone()).unwrap_or_default();
                let base = name.rsplit_once(" #").filter(|(_, n)| n.parse::<u32>().is_ok()).map_or(name.as_str(), |(b, _)| b);
                let Some((_, tail)) = base.rsplit_once(" - ").filter(|(_, t)| t.starts_with("Band ") || t.starts_with("Main ")) else { continue };
                checked += 1;
                if param.as_deref() != Some(tail) {
                    bad.push(format!("{path}: {name:?} -> {param:?}"));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
        assert!(checked >= 10, "only {checked} named EQ clips");
    }

    /// Writes a project with an automation clip on a slot plugin parameter, for opening in other software:
    /// OS_PARAM_FLP_IN (a project with a parametric EQ in a mixer slot) -> OS_PARAM_FLP_OUT, one clip on the EQ's band 1
    /// frequency (parameter OS_PARAM_K, default 7) rising from 0 to 1 over its 4 bars (spec §7).
    #[test]
    #[ignore]
    fn write_parameter_automation_project() {
        let (Some(input), Some(out)) = (std::env::var_os("OS_PARAM_FLP_IN"), std::env::var_os("OS_PARAM_FLP_OUT")) else { return };
        let k: usize = std::env::var("OS_PARAM_K").ok().and_then(|v| v.parse().ok()).unwrap_or(7);
        let mut a = App::new(None);
        a.open(input.into());
        a.plugins.effects = crate::plugins::builtin_effects();
        a.sync_effects();
        a.params_tick();
        let key = a.state.params.lists.keys().copied().find(|&k| fx_of(k).is_some()).expect("a hosted EQ slot");
        println!("{:?}", a.params_automate(key, k));
        let auto = a.state.project.automations.last_mut().unwrap();
        println!("target {:#010x}", auto.target().unwrap());
        let end = auto.points.last().map_or(16.0, |p| p.beat.max(16.0));
        auto.points = vec![os_model::AutoPoint::new(0.0, 0.0), os_model::AutoPoint::new(end, 1.0)];
        a.save_to(out.into());
        println!("{:?}", a.state.message);
    }

    /// A stand-in instance for typing `Weak` (never upgraded).
    struct Dummy;
    impl os_engine::Instrument for Dummy {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_engine::HostedEffect for Dummy {
        fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_host::Plugin for Dummy {
        fn desc(&self) -> &os_host::Descriptor {
            unimplemented!()
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            false
        }
        fn editor_open(&self) -> bool {
            false
        }
        fn open_editor(&mut self, _: *mut std::ffi::c_void, _: f32) -> Result<(u32, u32), String> {
            Err(String::new())
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {}
        fn save_state(&self) -> Option<Vec<u8>> {
            None
        }
        fn load_state(&mut self, _: &[u8]) -> bool {
            false
        }
    }
}
