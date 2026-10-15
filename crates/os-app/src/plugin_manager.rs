// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The plugin manager (Settings > Plugins): search folders, scan options, the format preference and the plugin table.
// spec: ui/plugin-manager.md §2, §3

use winit::keyboard::{Key, NamedKey};

use crate::app::App;
use crate::plugin_db::{self as db, DirKind, Mode};
use crate::views::settings::{self, Plug};

impl App {
    /// The plugin manager's checks (`settings::plugin_check`).
    pub(crate) fn plugin_check(&mut self, key: &str) {
        let p = &mut self.state.plugdb.prefs;
        match key {
            settings::STARTUP_CHECK => p.startup_scan = !p.startup_scan,
            settings::ERRORS_CHECK => p.rescan_errors = !p.rescan_errors,
            settings::COMBINE_CHECK => p.combine = !p.combine,
            _ => return,
        }
        self.prefs_changed();
    }

    /// The plugin manager's other controls.
    pub(crate) fn plugin_act(&mut self, a: Plug) {
        if a != Plug::Find {
            self.state.plugdb.finding = false;
        }
        let dir = |app: &App, i: usize| db::search_dirs(&app.state.plugdb.prefs).into_iter().nth(i);
        match a {
            // Any folder change starts a scan (§2).
            Plug::Folder(i) => {
                let Some(d) = dir(self, i) else { return };
                self.state.plugdb.prefs.toggle_folder(&d.path);
                self.prefs_changed();
                self.start_scan(Mode::Changed);
            }
            Plug::Remove(i) => {
                let Some(d) = dir(self, i).filter(|d| d.kind == DirKind::Added) else { return };
                self.state.plugdb.prefs.remove_folder(&d.path);
                self.prefs_changed();
                self.start_scan(Mode::Changed);
                self.state.message = Some(format!("Removed search folder {}", d.path.display()));
            }
            Plug::Prefer(p) => {
                self.state.plugdb.prefs.prefer = p;
                self.prefs_changed();
            }
            Plug::Sort(c) => {
                let d = &mut self.state.plugdb;
                d.sort = if d.sort.0 == c { (c, !d.sort.1) } else { (c, false) };
                d.refilter();
            }
            Plug::Table | Plug::Row(_) => {}
            Plug::Star(i) | Plug::Show(i) => {
                let d = &mut self.state.plugdb;
                let Some(uid) = d.rows.get(i).filter(|r| r.toggles).map(|r| r.uid.clone()) else { return };
                if matches!(a, Plug::Star(_)) {
                    d.prefs.toggle_favorite(&uid);
                } else {
                    d.prefs.toggle_hidden(&uid);
                }
                self.prefs_changed();
            }
            Plug::Find => self.state.plugdb.finding = true,
        }
        self.invalidate();
    }

    /// Add folder... (§2): a folder dialog; the folder and its sub-folders are searched for every format.
    pub(crate) fn add_plugin_folder(&mut self) {
        use crate::filedlg::{FileDlg, Mode};
        self.file_dialog(FileDlg::new(Mode::Folder, "Add a plugin folder", "plugins", crate::dialog::Then::PluginFolder));
    }

    /// The folder chosen in Add folder...
    pub(crate) fn plugin_folder_chosen(&mut self, path: std::path::PathBuf) {
        self.state.message = Some(if self.state.plugdb.prefs.add_folder(&path) {
            self.prefs_changed();
            self.start_scan(Mode::Changed);
            format!("Added search folder {}: looking for plugins", path.display())
        } else {
            format!("{} is searched already", path.display())
        });
    }

    /// Typing into the plugin table's Find box (§2). True when the key was taken.
    pub(crate) fn plugin_find_key(&mut self, key: &Key, ctrl: bool) -> bool {
        let s = &mut self.state;
        if !(s.settings.open && s.settings.tab == settings::PLUGINS && s.plugdb.finding) {
            s.plugdb.finding = false;
            return false;
        }
        let d = &mut s.plugdb;
        match key {
            Key::Named(NamedKey::Enter) => d.finding = false,
            Key::Named(NamedKey::Escape) => {
                d.finding = false;
                d.find.clear();
            }
            Key::Named(NamedKey::Backspace) => {
                d.find.pop();
            }
            Key::Named(NamedKey::Space) => d.find.push(' '),
            Key::Character(c) if !ctrl => d.find.push_str(c),
            _ => return true,
        }
        d.top = 0;
        d.refilter();
        self.invalidate();
        true
    }
}
