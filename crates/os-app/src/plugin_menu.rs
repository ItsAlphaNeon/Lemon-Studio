// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Plugin menus (the full-screen plugin picker is retired): F8, the toolbar's plugin button, the channel rack's "+",
//! Add one ▸ / Insert ▸ and the mixer slots open the app's own popup listing the installed plugins, and a pick adds
//! the plugin where that menu says.
// spec: ui/plugin-menu.md

use std::collections::BTreeMap;

use crate::app::{App, State};
use crate::playlist::{Cmd, Mark, PItem, Popup};
use crate::plugin_db::{self as db, Entry, MenuView, Mode};

/// What a plugin menu entry does (`Cmd::Plugin`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlugCmd {
    /// Generator `index` (into `Plugins::available`): a new channel at the bottom of the rack, or right below row
    /// `after`.
    Generator { index: usize, after: Option<usize> },
    /// Effect `index` (into `Plugins::effects`) into the selected mixer track: slot `slot`, else its first empty slot.
    Effect { index: usize, slot: Option<u8> },
    /// Find new plugins (a background scan).
    Find,
    /// Settings > Plugins.
    Manage,
    View(MenuView),
}

/// Which plugins a menu lists and where a pick goes (spec §1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    /// F8, the toolbar button, VIEW / ADD > Plugin picker: generators and effects.
    All,
    /// Generators: a new channel at the bottom (`None`) or right below row `r`.
    Gen(Option<usize>),
    /// Effects into the selected track: slot `k`, else its first empty slot.
    Fx(Option<u8>),
}

fn pick(target: Target, effect: bool, index: usize) -> Cmd {
    Cmd::Plugin(match (target, effect) {
        (Target::Fx(slot), _) => PlugCmd::Effect { index, slot },
        (_, true) => PlugCmd::Effect { index, slot: None },
        (Target::Gen(after), false) => PlugCmd::Generator { index, after },
        (Target::All, false) => PlugCmd::Generator { index, after: None },
    })
}

fn item(e: &Entry, target: Target, effect: bool) -> PItem {
    PItem::new(e.label.clone(), pick(target, effect, e.index))
}

/// A header with a run-time label.
fn head(label: String) -> PItem {
    PItem { head: true, ..PItem::new(label, Cmd::Nothing) }
}

/// One section's entries (spec §2): favourites, a separator, then the rest by display mode. `nested`: the list is a
/// submenu itself, so "By vendor" uses headers instead of submenus (popups open one submenu level).
fn section(s: &State, effect: bool, target: Target, nested: bool) -> Vec<PItem> {
    let list = if effect { &s.plugdb.lists.fx } else { &s.plugdb.lists.gens };
    if list.is_empty() {
        return vec![PItem::new(if effect { "(no effect plugins found)" } else { "(no instruments found)" }, Cmd::Nothing).off()];
    }
    let mut v: Vec<PItem> = list.iter().filter(|e| e.favorite).map(|e| item(e, target, effect)).collect();
    let rest: Vec<&Entry> = list.iter().filter(|e| !e.favorite).collect();
    if !v.is_empty() && !rest.is_empty() {
        v.push(PItem::sep());
    }
    match s.plugdb.prefs.menu_view {
        MenuView::Simple => v.extend(rest.iter().map(|e| item(e, target, effect))),
        MenuView::Categories => {
            for cat in db::categories(effect) {
                let in_cat: Vec<&&Entry> = rest.iter().filter(|e| e.category == cat).collect();
                if !in_cat.is_empty() {
                    v.push(PItem::head(cat));
                    v.extend(in_cat.iter().map(|e| item(e, target, effect)));
                }
            }
        }
        MenuView::Vendor => {
            let mut by: BTreeMap<String, (String, Vec<&Entry>)> = BTreeMap::new();
            for e in &rest {
                by.entry(e.vendor.to_lowercase()).or_insert_with(|| (e.vendor.clone(), Vec::new())).1.push(e);
            }
            for (_, (vendor, es)) in by {
                let items: Vec<PItem> = es.iter().map(|e| item(e, target, effect)).collect();
                if nested {
                    v.push(head(vendor));
                    v.extend(items);
                } else {
                    v.push(PItem { children: items, ..PItem::new(vendor, Cmd::Nothing) });
                }
            }
        }
    }
    v
}

/// After the lists (spec §2): Find new plugins, Manage plugins..., the display modes.
fn footer(s: &State) -> Vec<PItem> {
    let view = s.plugdb.prefs.menu_view;
    let scanning = s.plugdb.progress.is_some();
    let find = PItem::new(if scanning { "Find new plugins (scanning...)" } else { "Find new plugins" }, Cmd::Plugin(PlugCmd::Find));
    vec![
        PItem::sep(),
        if scanning { find.off() } else { find },
        PItem::new("Manage plugins...", Cmd::Plugin(PlugCmd::Manage)),
        PItem::sep(),
        PItem::new("Categories", Cmd::Plugin(PlugCmd::View(MenuView::Categories))).mark(Mark::Radio(view == MenuView::Categories)),
        PItem::new("Simple", Cmd::Plugin(PlugCmd::View(MenuView::Simple))).mark(Mark::Radio(view == MenuView::Simple)),
        PItem::new("By vendor", Cmd::Plugin(PlugCmd::View(MenuView::Vendor))).mark(Mark::Radio(view == MenuView::Vendor)),
    ]
}

/// The entries of a plugin menu for `target`; `nested` when they fill a submenu.
pub fn items(s: &State, target: Target, nested: bool) -> Vec<PItem> {
    let mut v = Vec::new();
    match target {
        Target::All => {
            v.push(PItem::head("Generators"));
            v.extend(section(s, false, target, nested));
            v.push(PItem::col());
            v.push(PItem::head("Effects"));
            v.extend(section(s, true, target, nested));
        }
        Target::Gen(_) => v.extend(section(s, false, target, nested)),
        Target::Fx(_) => v.extend(section(s, true, target, nested)),
    }
    v.extend(footer(s));
    v
}

/// A plugin menu for `target` at (x, y).
pub fn menu(s: &State, target: Target, x: f32, y: f32) -> Popup {
    Popup { x, y, items: items(s, target, false), sub: None }
}

/// Whether the open popup is a plugin menu (F8 closes it).
pub fn is_open(s: &State) -> bool {
    s.playlist.popup.as_ref().is_some_and(|p| p.items.iter().any(|i| i.cmd == Cmd::Plugin(PlugCmd::Manage)))
}

impl App {
    /// Opens the plugin menu for `target` at (x, y); F8 while one is open closes it.
    pub fn open_plugin_menu(&mut self, target: Target, at: (f32, f32)) {
        self.state.menu_open = None;
        self.state.playlist.popup = Some(menu(&self.state, target, at.0, at.1));
        self.invalidate();
    }

    /// F8 (spec §1): the all-plugins menu at the mouse, or closes an open plugin menu.
    pub fn toggle_plugin_menu(&mut self) {
        if is_open(&self.state) {
            self.state.playlist.popup = None;
            self.invalidate();
        } else {
            self.open_plugin_menu(Target::All, self.mouse);
        }
    }
}

/// Runs a plugin menu command. Returns a status message.
pub fn run(app: &mut App, c: PlugCmd) -> Option<String> {
    let m = match c {
        PlugCmd::Generator { index, after } => add_generator(app, index, after),
        PlugCmd::Effect { index, slot } => add_effect(app, index, slot),
        PlugCmd::Find => {
            app.start_scan(Mode::Changed);
            None
        }
        PlugCmd::Manage => {
            app.open_settings(crate::views::settings::PLUGINS);
            None
        }
        PlugCmd::View(v) => {
            app.state.plugdb.prefs.menu_view = v;
            app.prefs_changed();
            None
        }
    };
    app.invalidate();
    m
}

/// A new channel for generator `index` (spec §1): at the bottom of the rack, or right below row `after`; selected and
/// in view. Lemon Sampler makes an empty sampler channel (hosted on the next tick); any other generator a plugin
/// channel hosting it. Its plugin window opens once it is hosted.
pub fn add_generator(app: &mut App, index: usize, after: Option<usize>) -> Option<String> {
    let d = app.plugins.available.get(index)?.desc.clone();
    let s = &mut app.state;
    let sampler = d.name == crate::plugins::LEMON_SAMPLER && crate::project_plugins::is_builtin(&d);
    let mut ch = if sampler { crate::app::add_channel(&mut s.project) } else { crate::app::add_plugin_channel(&mut s.project, &d.name) };
    if sampler {
        s.pending_open = Some(ch);
    } else {
        s.pending_plugin = Some((ch, index));
    }
    s.piano.dirty = true;
    if let Some(r) = after.filter(|&r| r + 1 < ch) {
        let order: Vec<usize> = (0..=r).chain([ch]).chain(r + 1..ch).collect();
        crate::rack::reorder(app, &order);
        ch = r + 1;
    }
    let s = &mut app.state;
    s.selected_channel = ch;
    s.rack.select_only(ch);
    crate::rack::autosize(s);
    if after.is_none() {
        // Keep the new row in view.
        let r = crate::views::rack::rect(s);
        let view_h = crate::views::rack::panel(r).h;
        s.rack_scroll = (crate::views::rack::content_h(s) - view_h).max(0.0);
    }
    sampler.then(|| "Added a sampler channel (Lemon Sampler)".into())
}

/// Effect `index` into the selected mixer track (spec §1): slot `slot`, else the first empty one.
pub fn add_effect(app: &mut App, index: usize, slot: Option<u8>) -> Option<String> {
    let s = &app.state;
    let t = s.selected_mixer;
    let slot = slot.or_else(|| s.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots.iter().position(Option::is_none)).map(|k| k as u8));
    match slot {
        Some(k) => crate::mixer::run(app, crate::mixer::MCmd::AddEffect(k, index)),
        None => Some("Every slot of this track is in use".into()),
    }
}
