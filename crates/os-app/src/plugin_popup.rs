// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Menus a hosted plugin asks the host to pop up over its editor (VST 3 `IContextMenu::popup`, `os_host::Menu`): shown
//! as one of the app's own popups at the editor point, never a native menu. The plugin's entries come first, then the
//! host's ("Create automation clip" on an automatable parameter). The plugin is told on the UI thread when the popup
//! closes: a pick runs its entry then, closing without one runs nothing, and a newer menu replaces an open one.
// spec: plugins/vst3-hosting.md §9, plugins/plugin-parameters.md §6 (one menu per parameter)

use crate::app::App;
use crate::playlist::{Cmd, Mark, PItem, Popup};
use os_host::{Menu, MenuItem};

/// Menu `m` of hosted instance `key` as a popup at canvas point (`x`, `y`). Popups open one submenu level: a submenu
/// inside a submenu is shown inline in it, under a header with its title.
#[cfg_attr(not(plugin_windows), allow(dead_code))]
pub fn popup(key: usize, m: &Menu, x: f32, y: f32) -> Popup {
    Popup { x, y, items: rows(key, m.id, &m.items, 0), sub: None, scroll: [0; 2] }
}

fn rows(key: usize, id: u64, items: &[MenuItem], depth: usize) -> Vec<PItem> {
    let mut out = Vec::new();
    for it in items {
        match it {
            MenuItem::Entry { label, pick, enabled, checked } => {
                let row = PItem::new(label.clone(), Cmd::PluginMenu(key, id, *pick)).mark(if *checked { Mark::Check(true) } else { Mark::None });
                out.push(if *enabled { row } else { row.off() });
            }
            MenuItem::Separator => out.push(PItem::sep()),
            MenuItem::Sub { label, items } if depth == 0 => {
                let sub = PItem::sub(label.clone(), rows(key, id, items, 1));
                // Nothing in it: shown greyed, it opens nothing.
                out.push(if sub.children.is_empty() { sub.off() } else { sub });
            }
            MenuItem::Sub { label, items } => {
                out.push(PItem::sep());
                out.push(PItem { head: true, ..PItem::new(label.clone(), Cmd::Nothing) });
                out.extend(rows(key, id, items, depth + 1));
                out.push(PItem::sep());
            }
        }
    }
    tidy(out)
}

/// Separators at either end of a list, or next to another, are left out.
fn tidy(items: Vec<PItem>) -> Vec<PItem> {
    let mut out: Vec<PItem> = Vec::with_capacity(items.len());
    for it in items {
        if !(it.sep && out.last().is_none_or(|l| l.sep)) {
            out.push(it);
        }
    }
    if out.last().is_some_and(|l| l.sep) {
        out.pop();
    }
    out
}

/// Popup `p` shows plugin menu `id`.
pub fn holds(p: &Popup, id: u64) -> bool {
    fn any(items: &[PItem], id: u64) -> bool {
        items.iter().any(|i| matches!(i.cmd, Cmd::PluginMenu(_, m, _) if m == id) || any(&i.children, id))
    }
    any(&p.items, id)
}

impl App {
    /// Shows menu `m` of hosted instance `key` at canvas point `at`, in place of any open popup: a plugin menu still
    /// open closes unpicked (the same plugin's was replaced by this one). A menu with no entry closes at once. True
    /// when shown.
    #[cfg_attr(not(plugin_windows), allow(dead_code))]
    pub(crate) fn show_plugin_menu(&mut self, key: usize, m: &Menu, at: (f32, f32)) -> bool {
        let p = popup(key, m, at.0, at.1);
        if let Some((k, id)) = self.plugin_menu.take() {
            if self.state.playlist.popup.as_ref().is_some_and(|q| holds(q, id)) {
                self.state.playlist.popup = None;
            }
            if id != m.id {
                self.plugins.menu_done(k, id, None);
            }
        }
        if !holds(&p, m.id) {
            self.plugins.menu_done(key, m.id, None);
            return false;
        }
        self.state.playlist.popup = Some(p);
        self.plugin_menu = Some((key, m.id));
        true
    }

    /// Each tick: the plugin menu's popup closed without a pick (a click elsewhere, Escape, another popup), or the
    /// plugin's editor closed under it (the menu went with it, so the popup goes too). The plugin is told.
    pub(crate) fn plugin_menu_tick(&mut self) {
        let Some((key, id)) = self.plugin_menu else { return };
        let shown = self.state.playlist.popup.as_ref().is_some_and(|p| holds(p, id));
        // An instance busy on the audio thread is taken as still showing its editor.
        let editor = self.plugins.hosted.get(&key).is_some_and(|h| h.plugin.try_lock().map_or(true, |p| p.editor_open()));
        if shown && editor {
            return;
        }
        if shown {
            self.state.playlist.popup = None;
            self.invalidate();
        }
        self.plugin_menu = None;
        self.plugins.menu_done(key, id, None);
    }

    /// An entry picked in a plugin's menu (`Cmd::PluginMenu`, the popup already closed): the plugin runs it now, on the
    /// UI thread. The host's "Create automation clip" comes back as an automation request (`App::params_tick`).
    pub(crate) fn plugin_menu_pick(&mut self, key: usize, id: u64, pick: usize) {
        if self.plugin_menu.is_some_and(|(_, m)| m == id) {
            self.plugin_menu = None;
        }
        self.plugins.menu_done(key, id, Some(pick));
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::Hosted;
    use std::sync::{Arc, Mutex};

    fn entry(label: &str, pick: usize) -> MenuItem {
        MenuItem::Entry { label: label.into(), pick, enabled: true, checked: false }
    }

    /// What a VST 3 plugin's menu looks like after `IContextMenu::popup`: its entries, a submenu with a submenu in it,
    /// a disabled and a checked entry, then the host's entry for the parameter.
    fn menu(id: u64) -> Menu {
        let sub = |label: &str, items| MenuItem::Sub { label: label.into(), items };
        Menu {
            id,
            at: (10, 20),
            items: vec![
                MenuItem::Entry { label: "Soft".into(), pick: 0, enabled: true, checked: true },
                sub("More", vec![entry("Deep", 2), sub("Deeper", vec![entry("Deepest", 4)]), entry("Last", 6)]),
                sub("Empty", vec![]),
                MenuItem::Separator,
                MenuItem::Entry { label: "Off".into(), pick: 9, enabled: false, checked: false },
                MenuItem::Separator,
                entry("Create automation clip", 11),
            ],
        }
    }

    /// The plugin's rows in order as the app's popup rows: checks, greyed entries, separators, one submenu level with
    /// a deeper submenu inline under a header, and each entry's pick in its command.
    #[test]
    fn plugin_menus_become_app_popups() {
        let p = popup(3, &menu(9), 40.0, 50.0);
        assert_eq!((p.x, p.y, p.sub), (40.0, 50.0, None));
        let row = |i: &PItem| (i.label.to_string(), i.cmd.clone(), i.mark, i.enabled, i.sep, i.head);
        let pick = |n| Cmd::PluginMenu(3, 9, n);
        assert_eq!(
            p.items.iter().map(row).collect::<Vec<_>>(),
            [
                ("Soft".into(), pick(0), Mark::Check(true), true, false, false),
                ("More".into(), Cmd::Nothing, Mark::None, true, false, false),
                ("Empty".into(), Cmd::Nothing, Mark::None, false, false, false),
                (String::new(), Cmd::Nothing, Mark::None, true, true, false),
                ("Off".into(), pick(9), Mark::None, false, false, false),
                (String::new(), Cmd::Nothing, Mark::None, true, true, false),
                ("Create automation clip".into(), pick(11), Mark::None, true, false, false),
            ]
        );
        assert_eq!(
            p.items[1].children.iter().map(row).collect::<Vec<_>>(),
            [
                ("Deep".into(), pick(2), Mark::None, true, false, false),
                (String::new(), Cmd::Nothing, Mark::None, true, true, false),
                ("Deeper".into(), Cmd::Nothing, Mark::None, true, false, true),
                ("Deepest".into(), pick(4), Mark::None, true, false, false),
                (String::new(), Cmd::Nothing, Mark::None, true, true, false),
                ("Last".into(), pick(6), Mark::None, true, false, false),
            ]
        );
        assert!(holds(&p, 9) && !holds(&p, 8));
        // A menu of separators only has nothing to show.
        let bare = Menu { id: 1, at: (0, 0), items: vec![MenuItem::Separator, MenuItem::Sub { label: "x".into(), items: vec![MenuItem::Separator] }] };
        assert!(!holds(&popup(0, &bare, 0.0, 0.0), 1));
    }

    /// A stand-in instance that records what the app tells it about its menus, has one parameter (id 7), and asks for
    /// an automation clip on it when the host's entry (pick 11) is picked, as the VST 3 host's entry does.
    struct Fake {
        desc: os_host::Descriptor,
        done: Arc<Mutex<Vec<(u64, Option<usize>)>>>,
        automate: Vec<u32>,
        editor: bool,
        /// A menu it asked to pop up.
        asks: Option<Menu>,
    }
    impl os_engine::Instrument for Fake {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_engine::HostedEffect for Fake {
        fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }
    impl os_host::Plugin for Fake {
        fn desc(&self) -> &os_host::Descriptor {
            &self.desc
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            true
        }
        fn editor_open(&self) -> bool {
            self.editor
        }
        fn open_editor(&mut self, _: *mut std::ffi::c_void, _: f32) -> Result<(u32, u32), String> {
            Err(String::new())
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {
            self.editor = false;
        }
        fn save_state(&self) -> Option<Vec<u8>> {
            None
        }
        fn load_state(&mut self, _: &[u8]) -> bool {
            false
        }
        fn param_count(&self) -> usize {
            1
        }
        fn param_info(&self, index: usize) -> Option<os_host::ParamInfo> {
            (index == 0).then(|| os_host::ParamInfo {
                id: 7,
                name: "Cutoff".into(),
                unit: String::new(),
                group: String::new(),
                min: 0.0,
                max: 1.0,
                default: 0.5,
                steps: 0,
                automatable: true,
                read_only: false,
                hidden: false,
                bypass: false,
                enumerated: false,
            })
        }
        fn take_automation_requests(&mut self) -> Vec<u32> {
            std::mem::take(&mut self.automate)
        }
        fn take_menu(&mut self) -> Option<Menu> {
            self.asks.take()
        }
        fn menu_done(&mut self, id: u64, pick: Option<usize>) {
            self.done.lock().unwrap().push((id, pick));
            if pick == Some(11) {
                self.automate.push(7);
            }
        }
    }

    /// An app hosting a `Fake` on channel 0, and what the fake is told.
    fn app() -> (App, Arc<Mutex<Fake>>, Arc<Mutex<Vec<(u64, Option<usize>)>>>) {
        let mut a = App::new(None);
        let done = Arc::new(Mutex::new(Vec::new()));
        let desc = crate::plugins::builtin()[0].desc.clone();
        let fake = Arc::new(Mutex::new(Fake { desc, done: done.clone(), automate: Vec::new(), editor: true, asks: None }));
        a.plugins.hosted.insert(0, Hosted::new(fake.clone()));
        (a, fake, done)
    }

    /// A pick runs on the UI thread through the popup's command and tells the plugin which entry; the popup closing
    /// without one tells it nothing was picked; a newer menu replaces the open one.
    #[test]
    fn picks_and_closes_reach_the_plugin() {
        let (mut a, fake, done) = app();
        assert!(a.show_plugin_menu(0, &menu(5), (40.0, 50.0)));
        assert_eq!(a.plugin_menu, Some((0, 5)));
        a.plugin_menu_tick();
        assert!(done.lock().unwrap().is_empty(), "still open");
        // A click on "Deep" in the More submenu.
        let deep = a.state.playlist.popup.as_ref().unwrap().items[1].children[0].cmd.clone();
        assert!(crate::playlist::run(&mut a, deep).is_none());
        assert!(a.state.playlist.popup.is_none());
        assert_eq!(*done.lock().unwrap(), [(5, Some(2))]);
        a.plugin_menu_tick();
        assert_eq!(done.lock().unwrap().len(), 1, "told once");
        // Closed by a click elsewhere: the next tick tells the plugin.
        assert!(a.show_plugin_menu(0, &menu(6), (0.0, 0.0)));
        a.state.playlist.popup = None;
        a.plugin_menu_tick();
        assert_eq!(done.lock().unwrap()[1..], [(6, None)]);
        assert_eq!(a.plugin_menu, None);
        // A newer menu replaces the open one; one with nothing to pick closes at once.
        assert!(a.show_plugin_menu(0, &menu(7), (0.0, 0.0)));
        assert!(a.show_plugin_menu(0, &menu(8), (0.0, 0.0)));
        assert_eq!(done.lock().unwrap()[2..], [(7, None)]);
        assert!(holds(a.state.playlist.popup.as_ref().unwrap(), 8));
        assert!(!a.show_plugin_menu(0, &Menu { id: 9, at: (0, 0), items: vec![] }, (0.0, 0.0)));
        assert_eq!(done.lock().unwrap()[3..], [(8, None), (9, None)]);
        assert!(a.state.playlist.popup.is_none(), "the replaced menu's popup went");
        // The editor closed under an open menu: the popup goes with it.
        assert!(a.show_plugin_menu(0, &menu(10), (0.0, 0.0)));
        fake.lock().unwrap().editor = false;
        a.plugin_menu_tick();
        assert!(a.state.playlist.popup.is_none());
        assert_eq!(done.lock().unwrap()[5..], [(10, None)]);
    }

    /// The host's entry picked in the plugin's menu makes an automation clip on the parameter, as the CLAP entry does
    /// (spec: plugins/plugin-parameters.md §6).
    #[test]
    fn the_host_entry_creates_an_automation_clip() {
        let (mut a, _fake, done) = app();
        let clips = a.state.project.automations.len();
        a.params_tick();
        assert!(a.show_plugin_menu(0, &menu(5), (0.0, 0.0)));
        let create = a.state.playlist.popup.as_ref().unwrap().items.iter().find(|i| i.label == "Create automation clip").unwrap().cmd.clone();
        crate::playlist::run(&mut a, create);
        assert_eq!(*done.lock().unwrap(), [(5, Some(11))]);
        a.params_tick();
        assert_eq!(a.state.project.automations.len(), clips + 1);
        let ch = a.state.project.channels[0].index;
        assert_eq!(a.state.project.automations.last().and_then(|x| x.target()), Some(os_model::target::channel_param(ch, 0)));
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Created automation clip")), "{:?}", a.state.message);
    }

    /// The app takes a menu the instance asked for on its next look at it; without a window to show it over, it closes
    /// at once, unpicked.
    #[test]
    fn a_menu_without_a_window_closes_unpicked() {
        let (mut a, fake, done) = app();
        fake.lock().unwrap().asks = Some(menu(4));
        let requests = a.plugins.idle(|_| None);
        assert!(!requests.iter().any(|r| matches!(r, crate::plugins::Request::PluginMenu(..))));
        assert_eq!(*done.lock().unwrap(), [(4, None)]);
        assert!(fake.lock().unwrap().asks.is_none());
    }
}
