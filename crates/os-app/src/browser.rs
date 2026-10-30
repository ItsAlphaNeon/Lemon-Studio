// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Browser behaviour: tabs, tree open/close, selection, scrolling and drag and drop into the project, favorites, the
//! folder index, and the menu bar's ways in.
// spec: ui/browser.md, ui/browser-commands.md

use os_model::{ChannelKind, ClipSource};

use crate::app::{Action, App, Drag, Win};
use crate::playlist::{Cmd, Mark, PItem, Popup};
use crate::playlist::{self as pl, PickKind};
use crate::views::browser::{self as view, Item};

/// Mouse travel before a press on an item becomes a drag (spec §3).
const DRAG_START: f32 = 4.0;
/// How long a clicked sample plays (spec §7).
const PREVIEW_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// The settings key of the browser's side in the tiling layout (`State::browser_right`).
pub(crate) const BROWSER_RIGHT_KEY: &str = "browser_right";

impl App {
    /// Browser actions; returns whether `a` was one.
    pub(crate) fn browser_act(&mut self, a: &Action) -> bool {
        if *a == Action::BrowserSplitter {
            self.browser_split_press();
            return true;
        }
        if *a == Action::BrowserMove {
            self.drag = Some(Drag::BrowserMove);
            return true;
        }
        let s = &mut self.state;
        match *a {
            Action::BrowserTab(i) => {
                s.browser_tab = i;
                s.browser_scroll = 0.0;
            }
            Action::BrowserCollapse => {
                let tab = format!("{}:", s.browser_tab);
                s.browser_open.retain(|k| !k.starts_with(&tab));
                s.browser_scroll = 0.0;
            }
            Action::BrowserReread => reread(s),
            Action::BrowserList => {}
            Action::BrowserMenu => {
                let (x, y) = view::menu_at(s);
                let open = s.playlist.popup.as_ref().is_some_and(|p| p.x == x && p.y == y);
                s.playlist.popup = if open { None } else { Some(Popup { x, y, items: options_menu(s), sub: None, scroll: [0; 2] }) };
            }
            Action::BrowserSearch => s.browser_search_focus = true,
            // The cross that stands in for the magnifier while there is text (spec §5).
            Action::BrowserClear => {
                s.browser_search.clear();
                s.browser_search_focus = false;
                s.browser_scroll = 0.0;
            }
            Action::BrowserRow(i) => {
                let Some(row) = view::rows(s).into_iter().nth(i) else { return true };
                select(s, &row.key);
                if row.open.is_some() && !s.browser_open.remove(&row.key) {
                    s.browser_open.insert(row.key.clone());
                }
                match row.item {
                    Item::Pattern(n) => {
                        s.pattern = n;
                    }
                    Item::Channel(c) => {
                        s.selected_channel = c;
                        s.rack.select_only(c);
                    }
                    Item::History(e) => s.message = crate::history::go_to(s, e),
                    Item::Version(ref f) => {
                        let f = f.clone();
                        self.ask_discard("opening an earlier version", crate::dialog::Next::Version(f));
                        return true;
                    }
                    _ => {}
                }
                if let Item::Sample(path) = &row.item {
                    self.preview_sample(path);
                }
                let s = &mut self.state;
                s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
                if !matches!(row.item, Item::None | Item::File(_) | Item::History(_) | Item::Version(_)) {
                    self.drag = Some(Drag::Browser { item: row.item, label: row.label, m0: self.mouse, started: false });
                }
            }
            _ => return false,
        }
        self.invalidate();
        true
    }

    /// Clicking a sample plays it once (spec §7): cut off after `PREVIEW_LIMIT` unless "Full sample preview" is ticked.
    pub(crate) fn preview_sample(&self, path: &str) {
        let mut p = crate::app::preview_project(path);
        p.tempo = Some(self.state.tempo);
        crate::preview_track::route(&mut p, &self.state.project, crate::prefs::number(&crate::prefs::BROWSER_PREVIEW_TRACK));
        let limit = (!view::opt(&self.state, view::OPT_PREVIEW)).then_some(PREVIEW_LIMIT);
        self.transport.preview_for(p, limit);
    }

    /// A browser options-menu command (spec §4); returns a message.
    pub(crate) fn browser_cmd(&mut self, c: BCmd) -> Option<String> {
        self.invalidate();
        let s = &mut self.state;
        match c {
            BCmd::Find(dir) => return find(s, dir),
            BCmd::Toggle(k) => {
                if !s.browser_opts.remove(k) {
                    s.browser_opts.insert(k);
                }
                save_opts(&s.browser_opts);
                s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
            }
            BCmd::History(dir) => {
                let pos = s.browser_hist_pos as isize + dir;
                let Some(key) = usize::try_from(pos).ok().and_then(|p| s.browser_hist.get(p)).cloned() else {
                    return Some("No more browser history".into());
                };
                s.browser_hist_pos = pos as usize;
                s.browser_sel = Some(key);
                reveal(s);
            }
            BCmd::Send(dir) => return send(s, dir),
            BCmd::Collapse => {
                let tab = format!("{}:", s.browser_tab);
                s.browser_open.retain(|k| !k.starts_with(&tab));
                s.browser_scroll = 0.0;
            }
            BCmd::Reread => reread(s),
            // A background scan (spec: ui/plugin-manager.md §4); plugins in use stay where they are.
            BCmd::Rescan => self.start_scan(crate::plugin_db::Mode::Changed),
            BCmd::Row(c) => return self.browser_row_cmd(c),
            BCmd::Jump(j) => self.browser_jump(j),
            // File ▸ Browser extra search folders (spec §4).
            BCmd::Configure => self.open_settings(crate::views::settings::FILE),
            BCmd::Star(key) => {
                let Some(d) = self.plugins.descriptor(key) else { return Some("No plugin here".into()) };
                return Some(self.star_plugin(&d.uid(), &d.name));
            }
        }
        None
    }

    /// The plugin manager's star on plugin `uid` (spec §10: a favorite plugin; ui/plugin-manager.md §2), toggled.
    fn star_plugin(&mut self, uid: &str, name: &str) -> String {
        self.state.plugdb.prefs.toggle_favorite(uid);
        self.prefs_changed();
        if self.state.plugdb.prefs.is_favorite(uid) {
            format!("{name} is a favorite: listed first in the plugin menus and in the browser's Favorites")
        } else {
            format!("{name} is no longer a favorite")
        }
    }

    /// The row's Favorite (spec §10): a plugin's star, or the file in or out of the favorites.
    fn toggle_favorite(&mut self, row: &view::Row) -> Option<String> {
        let s = &mut self.state;
        let path = match &row.item {
            Item::Generator(i) | Item::Effect(i) => {
                let list = if matches!(row.item, Item::Generator(_)) { &s.plugdb.lists.gens } else { &s.plugdb.lists.fx };
                let e = list.iter().find(|e| e.index == *i)?;
                let (uid, name) = (e.uid.clone(), e.label.clone());
                return Some(self.star_plugin(&uid, &name));
            }
            Item::Sample(p) | Item::Preset(p) | Item::File(p) => p.clone(),
            _ => return None,
        };
        let on = if let Some(i) = s.browser_favs.iter().position(|f| f.path == path) {
            s.browser_favs.remove(i);
            false
        } else {
            s.browser_favs.push(Fav { path, label: row.label.clone() });
            true
        };
        save_favs(&s.browser_favs);
        s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
        Some(if on { format!("{} added to the favorites", row.label) } else { format!("{} removed from the favorites", row.label) })
    }

    /// Keeps the folder index up to date (spec §11): read again when the indexed folders change, at once with "Re-index
    /// changed folders on startup", else when a search needs it; a search repaints when its index arrives.
    pub(crate) fn browser_index_tick(&mut self) {
        let s = &mut self.state;
        if s.browser_index.poll() {
            if s.browser_index.cut() {
                s.message = Some(format!("Browser index: the folders hold more than {} entries; a search finds the first ones", crate::browser_index::MAX_ENTRIES));
            }
            if !s.browser_search.is_empty() {
                s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
                self.invalidate();
            }
            return;
        }
        if !view::opt(s, view::OPT_INDEX) || s.browser_index.building() {
            return;
        }
        let roots = index_roots(s);
        if roots.is_empty() || s.browser_index.covers(&roots) {
            return;
        }
        if view::opt(s, view::OPT_REINDEX) || !s.browser_search.is_empty() {
            s.browser_index.start(roots);
            if cfg!(test) {
                self.invalidate();
            }
        }
    }

    /// A way into the browser from the menu bar or its keys (spec: ui/browser-commands.md): the browser is shown first
    /// when hidden.
    pub(crate) fn browser_jump(&mut self, j: Jump) {
        self.state.show_browser = true;
        let s = &mut self.state;
        match j {
            // §1: the Current project tab with that section alone open, and selected. The search is cleared so that it
            // shows (as Browse parameters does).
            Jump::Section(name) => {
                let Some(key) = view::section_key(name) else { return };
                if s.browser_tab != view::PROJECT_TAB {
                    s.browser_tab = view::PROJECT_TAB;
                    s.browser_scroll = 0.0;
                }
                s.browser_search.clear();
                for other in view::sections().filter(|&l| l != name).filter_map(view::section_key) {
                    s.browser_open.remove(&other);
                }
                s.browser_open.insert(view::project_root());
                s.browser_open.insert(key.clone());
                select(s, &key);
                reveal(s);
            }
            // §1-2: the tab as it was left, its open nodes and selection kept.
            Jump::PluginDb => {
                if s.browser_tab != view::PLUGIN_TAB {
                    s.browser_tab = view::PLUGIN_TAB;
                    s.browser_scroll = 0.0;
                }
            }
            // §3: exactly as a click in the search field.
            Jump::Find => {
                self.browser_act(&Action::BrowserSearch);
            }
            // ADD ▸ Browse all installed plugins, the mixer's Plugin database entry, ADD ▸ Browse presets (§2, §5): that
            // node of its tab opened and selected.
            Jump::Installed | Jump::DbEffects | Jump::Presets => {
                let (tab, key) = match j {
                    Jump::Installed => (view::PLUGIN_TAB, format!("{}:Plugin library/{}", view::PLUGIN_TAB, view::INSTALLED)),
                    Jump::DbEffects => (view::PLUGIN_TAB, format!("{}:Plugin library/Effects", view::PLUGIN_TAB)),
                    _ => (0, view::presets_key()),
                };
                if s.browser_tab != tab {
                    s.browser_tab = tab;
                    s.browser_scroll = 0.0;
                }
                s.browser_search.clear();
                if tab == view::PLUGIN_TAB {
                    s.browser_open.insert(format!("{tab}:Plugin library"));
                }
                s.browser_open.insert(key.clone());
                select(s, &key);
                reveal(s);
            }
            // §4-5: Patterns opened, the current pattern's row selected when listed (else the section's).
            Jump::Patterns => {
                let Some(key) = view::section_key("Patterns") else { return };
                if s.browser_tab != view::PROJECT_TAB {
                    s.browser_tab = view::PROJECT_TAB;
                    s.browser_scroll = 0.0;
                }
                s.browser_search.clear();
                s.browser_open.insert(view::project_root());
                s.browser_open.insert(key.clone());
                let cur = view::Item::Pattern(s.pattern);
                let row = view::rows(s).into_iter().find(|r| r.item == cur).map(|r| r.key);
                select(s, row.as_ref().unwrap_or(&key));
                reveal(s);
            }
        }
        self.invalidate();
    }

    /// Whether the mouse is over the docked browser.
    pub(crate) fn over_browser(&self) -> bool {
        self.state.show_browser && crate::views::browser_rect(&self.state).contains(self.mouse.0, self.mouse.1)
    }

    /// Whether typed letters belong to the browser search (the typing keyboard leaves them alone).
    pub(crate) fn browser_takes_typing(&self) -> bool {
        self.state.browser_search_focus || (view::opt(&self.state, view::OPT_TYPE) && self.over_browser())
    }

    /// Browser keys (spec §4, §5); returns whether the key was used.
    pub(crate) fn browser_key(&mut self, key: &winit::keyboard::Key, ctrl: bool, alt: bool) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let shift = self.shift;
        let focus = self.state.browser_search_focus;
        let over = self.over_browser() || focus;
        let cmd = match key {
            Key::Named(NamedKey::F2) if !self.state.browser_search.is_empty() => Some(BCmd::Find(-1)),
            Key::Named(NamedKey::F3) if !self.state.browser_search.is_empty() => Some(BCmd::Find(1)),
            Key::Named(NamedKey::ArrowUp) if over && ctrl => Some(BCmd::Collapse),
            Key::Named(NamedKey::ArrowUp) if over && shift => Some(BCmd::Send(-1)),
            Key::Named(NamedKey::ArrowDown) if over && shift => Some(BCmd::Send(1)),
            Key::Character(c) if over && ctrl && c.eq_ignore_ascii_case("r") => Some(BCmd::Reread),
            _ => None,
        };
        if let Some(c) = cmd {
            if let Some(m) = self.browser_cmd(c) {
                self.state.message = Some(m);
            }
            return true;
        }
        let s = &mut self.state;
        if focus {
            match key {
                Key::Named(NamedKey::Escape) => {
                    s.browser_search.clear();
                    s.browser_search_focus = false;
                }
                Key::Named(NamedKey::Enter) => {
                    if let Some(m) = find(s, 1) {
                        s.message = Some(m);
                    }
                }
                Key::Named(NamedKey::Backspace) => {
                    s.browser_search.pop();
                }
                Key::Named(NamedKey::Space) => s.browser_search.push(' '),
                Key::Character(c) if !ctrl && !alt => s.browser_search.push_str(c),
                _ => return false,
            }
            s.browser_scroll = 0.0;
            self.invalidate();
            return true;
        }
        // "Type to filter": letters typed over the browser start a search.
        if let Key::Character(c) = key {
            if !ctrl && !alt && view::opt(s, view::OPT_TYPE) && self.over_browser() {
                let s = &mut self.state;
                s.browser_search_focus = true;
                s.browser_search.push_str(c);
                s.browser_scroll = 0.0;
                self.invalidate();
                return true;
            }
        }
        false
    }

    /// The tiling layout: the browser's title bar dragged and let go on the window's other half moves it to that side
    /// (spec: the cutting-edge README, "tiling layout").
    pub(crate) fn browser_move_drop(&mut self) {
        let right = self.mouse.0 > self.state.screen.0 / 2.0;
        if right != self.state.browser_right {
            self.state.browser_right = right;
            crate::settings_file::set_flag(BROWSER_RIGHT_KEY, right);
            self.state.message = Some(format!("Browser: {}", if right { "right" } else { "left" }));
            self.invalidate();
        }
    }

    /// A press on the browser's splitter: drags it; a double click gives the browser its default width (spec §9).
    fn browser_split_press(&mut self) {
        let now = std::time::Instant::now();
        let s = &mut self.state;
        let double = s.browser_split_click.is_some_and(|t| now.duration_since(t).as_millis() < 400);
        s.browser_split_click = if double { None } else { Some(now) };
        if double {
            s.browser_w = crate::views::BROWSER_W;
            save_width(s.browser_w);
            return self.invalidate();
        }
        self.drag = Some(Drag::BrowserSplit { dx: self.mouse.0 - crate::views::browser_edge(&self.state) });
    }

    /// The splitter dragged to `edge`, the browser's inner edge (its splitter included): whole px, from its default
    /// width to half the window. Everything beside it moves, so the whole frame is repainted.
    pub(crate) fn browser_split_drag(&mut self, edge: f32) {
        let s = &mut self.state;
        let w = if crate::views::browser_right(s) { s.screen.0 - edge } else { edge };
        let w = w.min(s.screen.0 / 2.0).max(crate::views::BROWSER_W).round();
        if w != s.browser_w {
            s.browser_w = w;
            self.invalidate();
        }
    }

    /// A hovered row's scrolling label (spec §9): repainted whenever its offset moves; the scroll ends with the hover.
    pub(crate) fn marquee_tick(&mut self, now: std::time::Instant) {
        let hovered = self.state.show_browser && matches!(self.hover, Some(Action::BrowserRow(_)));
        let m = self.state.browser_marquee.get_mut();
        let Some(mq) = m.as_ref() else { return };
        if !hovered {
            *m = None;
            return;
        }
        let r = mq.rect;
        if view::marquee_offset(now.duration_since(mq.since).as_secs_f32(), mq.over) != mq.off {
            self.damage_rect(r);
        }
    }

    /// Wheel over the browser list.
    pub(crate) fn browser_wheel(&mut self, dy: f32) {
        let s = &mut self.state;
        s.browser_scroll = (s.browser_scroll - dy * 3.0 * view::ROW_H).clamp(0.0, view::max_scroll(s));
        self.invalidate();
    }

    /// Mouse moved while an item is pressed: start the drag, move the label.
    pub(crate) fn browser_drag_move(&mut self, label: String, m0: (f32, f32), started: bool, item: Item) {
        let (x, y) = self.mouse;
        if !started && (x - m0.0).hypot(y - m0.1) < DRAG_START {
            return;
        }
        if !started {
            self.drag = Some(Drag::Browser { item, label: label.clone(), m0, started: true });
        }
        if let Some((_, ox, oy)) = self.state.browser_drag {
            self.damage_rect(view::ghost_rect(ox, oy));
        }
        self.state.browser_drag = Some((label, x, y));
        self.damage_rect(view::ghost_rect(x, y));
    }

    /// Release: drop the dragged item on whatever is under the mouse (spec §3).
    pub(crate) fn browser_drop(&mut self, item: Item, started: bool) {
        self.state.browser_drag = None;
        self.invalidate();
        if !started {
            return;
        }
        // A preset goes into the plugin under the mouse, or a new instance of its plugin (spec: plugins/presets.md).
        if let Item::Preset(path) = &item {
            self.state.message = crate::presets::dropped(self, std::path::Path::new(path));
            return;
        }
        // Plugin windows float above the main window: a sample released over one goes into that plugin.
        #[cfg(any(windows, target_os = "macos"))]
        if let Item::Sample(path) = &item {
            if self.sample_to_plugin_window(path) {
                return;
            }
        }
        let (x, y) = self.mouse;
        let win = self.top_hit(x, y).and_then(|h| h.win);
        let grid = self.pl_grid().filter(|g| g.contains(x, y) && win == Some(Win::Playlist));
        let alt = self.alt;
        let s = &mut self.state;
        // Snapped playlist position under the mouse.
        let spot = grid.map(|grid| {
            let g = pl::PGeom::new(s, grid);
            (g.track_at(y), pl::snap_place(s, &g, g.tick_at(x), alt))
        });
        match item {
            Item::Pattern(n) => {
                if let Some((track, tick)) = spot {
                    pl::snapshot(s);
                    s.playlist.source = Some(ClipSource::Pattern(n));
                    s.playlist.picker = PickKind::Pattern;
                    let i = pl::place(s, track, tick);
                    s.playlist.sel = vec![i];
                } else if matches!(win, Some(Win::PianoRoll | Win::ChannelRack)) {
                    s.pattern = n;
                }
            }
            Item::Channel(c) => {
                let Some(ch) = s.project.channels.get(c) else { return };
                let (kind, index) = (ch.kind, ch.index);
                if let Some((track, tick)) = spot {
                    if matches!(kind, ChannelKind::AudioClip | ChannelKind::Automation) {
                        pl::snapshot(s);
                        s.playlist.source = Some(ClipSource::Channel(index));
                        s.playlist.picker = if kind == ChannelKind::AudioClip { PickKind::Audio } else { PickKind::Automation };
                        let i = pl::place(s, track, tick);
                        s.playlist.sel = vec![i];
                    } else {
                        s.message = Some("Only audio and automation channels go on the arranger; drop a pattern instead".into());
                    }
                } else if matches!(win, Some(Win::PianoRoll | Win::ChannelRack)) {
                    s.selected_channel = c;
                    s.rack.select_only(c);
                }
            }
            Item::Sample(path) => {
                if let Some((track, tick)) = spot {
                    let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
                    s.message = Some(match pl::add_audio_clip(s, &path, track, tick) {
                        Some(_) => format!("Added audio clip {name}"),
                        None => format!("Could not read {name} as audio"),
                    });
                } else if win == Some(Win::ChannelRack) {
                    self.sample_to_rack(path, y);
                }
            }
            // As if picked in the F8 menu (spec: ui/plugin-menu.md §5): a new channel at the bottom of the rack.
            Item::Generator(i) => {
                if spot.is_some() || win == Some(Win::ChannelRack) {
                    if let Some(m) = crate::plugin_menu::add_generator(self, i, None) {
                        self.state.message = Some(m);
                    }
                }
            }
            // An effect dropped on the mixer goes into the slot under the mouse, else the current track's first free
            // slot (spec §3).
            Item::Effect(i) => {
                if win == Some(Win::Mixer) {
                    let slot = match self.top_hit(x, y).map(|h| &h.action) {
                        Some(Action::Mix(crate::mixer::MHit::SlotName(k) | crate::mixer::MHit::SlotArrow(k))) => Some(*k),
                        _ => None,
                    };
                    if let Some(m) = crate::plugin_menu::add_effect(self, i, slot) {
                        self.state.message = Some(m);
                    }
                }
            }
            Item::Param(key, index) => {
                if let Some((track, tick)) = spot {
                    let msg = self.params_drop(key, index, track, tick);
                    self.state.message = msg;
                }
            }
            Item::None | Item::File(_) | Item::History(_) | Item::Version(_) | Item::Preset(_) => {}
        }
    }

    /// A folder dropped on the window becomes an All-tab root, opened and selected (spec §6).
    pub(crate) fn add_browser_folder(&mut self, path: std::path::PathBuf) {
        let s = &mut self.state;
        if !s.browser_folders.contains(&path) {
            s.browser_folders.push(path.clone());
            save_folders(s);
        }
        s.browser_tab = 0;
        s.browser_search.clear();
        let key = view::disk_key(&path);
        s.browser_open.insert(key.clone());
        select(s, &key);
        reveal(s);
        s.message = Some(format!("Browser: added {}", path.display()));
        self.invalidate();
    }

    /// Right click on row `i` (spec §8): selects it and opens its menu at the mouse; returns whether there was a row.
    pub(crate) fn browser_row_menu(&mut self, i: usize, x: f32, y: f32) -> bool {
        let s = &mut self.state;
        let Some(row) = view::rows(s).into_iter().nth(i) else { return false };
        select(s, &row.key);
        s.playlist.popup = Some(Popup { x, y, items: row_menu(s, &row), sub: None, scroll: [0; 2] });
        self.invalidate();
        true
    }

    /// Middle click on row `i` (spec §7): selects it, and a sample goes into the selected channel as the row menu's
    /// "Send to selected channel" sends it (§8). Returns whether there was a row.
    pub(crate) fn browser_row_middle(&mut self, i: usize) -> bool {
        let s = &mut self.state;
        let Some(row) = view::rows(s).into_iter().nth(i) else { return false };
        select(s, &row.key);
        if let Item::Sample(path) = &row.item {
            s.message = send_sample(s, path, &row.label);
        }
        self.invalidate();
        true
    }

    /// A row menu command on the selected row (spec §8); returns a message.
    fn browser_row_cmd(&mut self, c: RowCmd) -> Option<String> {
        let s = &mut self.state;
        let row = view::rows(s).into_iter().find(|r| s.browser_sel.as_ref() == Some(&r.key))?;
        let sample = match &row.item {
            Item::Sample(p) => Some(p.clone()),
            _ => None,
        };
        match c {
            RowCmd::Toggle => {
                if !s.browser_open.remove(&row.key) {
                    s.browser_open.insert(row.key);
                }
                s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
            }
            // What a click on the row does, without the click's open / close.
            RowCmd::Activate => match row.item {
                Item::Pattern(n) => s.pattern = n,
                Item::Channel(c) => {
                    s.selected_channel = c;
                    s.rack.select_only(c);
                }
                Item::History(e) => return crate::history::go_to(s, e),
                Item::Version(f) => {
                    self.ask_discard("opening an earlier version", crate::dialog::Next::Version(f));
                    return None;
                }
                _ => {}
            },
            RowCmd::Pattern(pc) => {
                let Item::Pattern(n) = row.item else { return None };
                s.pattern = n;
                return self.pattern_cmd(pc);
            }
            RowCmd::ChannelWindow => {
                let Item::Channel(i) = row.item else { return None };
                if s.hosted_rows.contains(&i) {
                    s.pending_open = Some(i);
                } else {
                    self.open_channel_window(i);
                }
            }
            RowCmd::Preview => self.preview_sample(&sample?),
            RowCmd::Send => return send_sample(s, &sample?, &row.label),
            RowCmd::NewChannel => self.sample_to_channel(sample?, None),
            RowCmd::OpenFolder => open_folder(&row.path?),
            RowCmd::ShowInFolder => crate::theme_library::show_in_folder(&row.path?),
            // File ▸ Browser extra search folders, the root's Browser name selected for typing.
            RowCmd::Rename => {
                let i = s.browser_folders.iter().position(|f| Some(f) == row.path.as_ref())?;
                self.open_settings(crate::views::settings::FILE);
                self.search_folder_cell(i, crate::search_folders::Col::Name);
            }
            RowCmd::Remove => return Some(remove_root(s, &row.path?)),
            RowCmd::Favorite => return self.toggle_favorite(&row),
        }
        None
    }
}

/// A dropped folder root out of the browser (spec §6, §8), with its Browser name and its open nodes.
fn remove_root(s: &mut crate::app::State, path: &std::path::Path) -> String {
    let key = view::disk_key(path);
    s.browser_folders.retain(|f| f != path);
    s.browser_names.remove(path);
    save_folders(s);
    let below = format!("{key}/");
    s.browser_open.retain(|k| *k != key && !k.starts_with(&below));
    s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
    format!("Removed {} from the browser", path.display())
}

/// Opens folder `dir` in the system's file manager (tests: nothing).
fn open_folder(dir: &std::path::Path) {
    if cfg!(test) {
        return;
    }
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

/// `browser.txt` in the user folder: `width=` the browser's width in px (anything else, or missing, = the default).
fn width_file() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    os_paths::user_file("browser.txt")
}

/// The value of `key=` in `browser.txt`'s text.
fn value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim())
}

fn parse_width(text: &str) -> f32 {
    let w = value(text, "width").and_then(|v| v.parse::<f32>().ok());
    w.filter(|w| w.is_finite()).map_or(crate::views::BROWSER_W, |w| w.max(crate::views::BROWSER_W))
}

/// `options=` the options-menu toggles that are on, by name, comma-separated (spec §4); missing = the defaults.
fn parse_opts(text: &str) -> std::collections::HashSet<&'static str> {
    match value(text, "options") {
        Some(v) => v.split(',').filter_map(|o| view::ALL_OPTS.iter().copied().find(|k| *k == o.trim())).collect(),
        None => view::DEFAULT_OPTS.iter().copied().collect(),
    }
}

/// The options kept from the last run (tests: the defaults).
pub(crate) fn load_opts() -> std::collections::HashSet<&'static str> {
    parse_opts(&width_file().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default())
}

/// `browser.txt` with `key=` set to `v`, its other lines kept.
fn with_value(text: &str, key: &str, v: &str) -> String {
    let mut out: String = text.lines().filter(|l| l.split_once('=').is_none_or(|(k, _)| k.trim() != key)).map(|l| format!("{l}\n")).collect();
    out.push_str(&format!("{key}={v}\n"));
    out
}

fn save_value(key: &str, v: &str) {
    let Some(p) = width_file() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let _ = std::fs::write(p, with_value(&old, key, v));
}

/// Keeps the options for the next run.
fn save_opts(opts: &std::collections::HashSet<&'static str>) {
    let on: Vec<&str> = view::ALL_OPTS.iter().copied().filter(|k| opts.contains(k)).collect();
    save_value("options", &on.join(","));
}

/// The browser's width kept from the last run (spec §9; tests: the default).
pub(crate) fn load_width() -> f32 {
    width_file().and_then(|p| std::fs::read_to_string(p).ok()).map_or(crate::views::BROWSER_W, |t| parse_width(&t))
}

/// Keeps the browser's width for the next run.
pub(crate) fn save_width(w: f32) {
    save_value("width", &w.to_string());
}

/// A favorite file (spec §10): its path (or a factory preset's) and the label its row had when it was starred.
#[derive(Debug, Clone, PartialEq)]
pub struct Fav {
    pub path: String,
    pub label: String,
}

/// `browser_favorites.txt` in the user folder: one favorite per line, its path, a tab, its label.
fn favs_file() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    os_paths::user_file("browser_favorites.txt")
}

fn parse_favs(text: &str) -> Vec<Fav> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (path, label) = l.split_once('\t').unwrap_or((l, ""));
            let label = if label.is_empty() { path.rsplit(['/', '\\']).next().unwrap_or(path) } else { label };
            Fav { path: path.to_string(), label: label.to_string() }
        })
        .collect()
}

fn favs_text(favs: &[Fav]) -> String {
    favs.iter().map(|f| format!("{}\t{}\n", f.path, f.label)).collect()
}

/// The favorite files kept from the last run.
pub(crate) fn load_favs() -> Vec<Fav> {
    favs_file().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse_favs(&t)).unwrap_or_default()
}

fn save_favs(favs: &[Fav]) {
    let Some(p) = favs_file() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, favs_text(favs));
}

/// The folders the index reads (spec §11): the dropped folders and the project's data folder.
pub(crate) fn index_roots(s: &crate::app::State) -> Vec<std::path::PathBuf> {
    let mut v = s.browser_folders.clone();
    v.extend(crate::data_folder::folder(s));
    v
}

/// `browser_folders.txt` in the user folder (`os_paths::user_dir`): one dropped folder per line (spec §6), its Browser
/// name after a tab when it has one (`crate::search_folders::file_text`).
fn folders_file() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    os_paths::user_file("browser_folders.txt")
}

fn read_folders() -> Vec<(std::path::PathBuf, String)> {
    let Some(text) = folders_file().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    crate::search_folders::parse_file(&text)
}

pub(crate) fn load_folders() -> Vec<std::path::PathBuf> {
    read_folders().into_iter().map(|f| f.0).collect()
}

/// The dropped folders' Browser names.
pub(crate) fn load_names() -> std::collections::BTreeMap<std::path::PathBuf, String> {
    read_folders().into_iter().filter(|f| !f.1.is_empty()).collect()
}

/// Keeps the dropped folders and their names.
pub(crate) fn save_folders(s: &crate::app::State) {
    let Some(p) = folders_file() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, crate::search_folders::file_text(&s.browser_folders, &s.browser_names));
}

/// The menu bar's ways into the browser (spec: ui/browser-commands.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jump {
    /// VIEW's Browser section: this section of the Current project tree (§1).
    Section(&'static str),
    /// VIEW > Plugin database (Shift+F8), ADD > Browse plugin database: the Plugin database tab (§1, §2).
    PluginDb,
    /// TOOLS > Browser find... (Ctrl+F): the keyboard goes to the search field (§3).
    Find,
    /// PATTERNS > Open in project browser (Shift+Ctrl+P): the Patterns section, the other sections left as they are (§4).
    Patterns,
    /// ADD > Browse all installed plugins: the Plugin database's Installed section (§2).
    Installed,
    /// The mixer's Plugin database entry (Shift+F8 in its effect list): the Plugin database's Effects.
    DbEffects,
    /// ADD > Browse presets: the All tab's Plugin presets.
    Presets,
}

impl Jump {
    /// Whether the jump has somewhere to go: a section the tree does not list (History, Remote control) has not, and
    /// its entry stays greyed out.
    pub fn available(self) -> bool {
        match self {
            Jump::Section(name) => view::section_key(name).is_some(),
            _ => true,
        }
    }
}

/// Options-menu commands (spec §4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BCmd {
    /// Find previous (-1) / next (1).
    Find(isize),
    Toggle(&'static str),
    /// Navigation history back (-1) / forward (1).
    History(isize),
    /// Send previous (-1) / next (1) sample to the selected channel.
    Send(isize),
    Collapse,
    Reread,
    Rescan,
    /// A row menu command (spec §8), on the selected row (the right click selected it).
    Row(RowCmd),
    /// A way into the browser from another menu (the mixer's Plugin database entry).
    Jump(Jump),
    /// Configure extra folders: File ▸ Browser extra search folders.
    Configure,
    /// "Add to plugin database (flag as favorite)" of the plugin at this hosted key: its star toggled (spec §10).
    Star(usize),
}

/// What a row menu entry does (spec §8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowCmd {
    /// Open / close the node.
    Toggle,
    /// What a click on the row does: the pattern or channel selected, the project taken to the history step.
    Activate,
    /// The row's pattern made current, then this PATTERNS command.
    Pattern(crate::patterns::Cmd),
    /// The channel's window (its plugin's, Lemon Sampler's, or the channel settings).
    ChannelWindow,
    /// A sample played (§7), loaded into the selected channel, or into a new Lemon Sampler channel.
    Preview,
    Send,
    NewChannel,
    /// A disk folder opened in the system's file manager; a disk file shown there, selected in its folder.
    OpenFolder,
    ShowInFolder,
    /// A dropped folder root: its Browser name typed in File ▸ Browser extra search folders; out of the browser.
    Rename,
    Remove,
    /// The row's Favorite check: a plugin's star, a file in or out of the favorites (spec §10).
    Favorite,
}

/// The row menu's way to the system's file manager: a folder, a file.
#[cfg(windows)]
const FILE_MANAGER: [&str; 2] = ["Open in Explorer", "Show in Explorer"];
#[cfg(target_os = "macos")]
const FILE_MANAGER: [&str; 2] = ["Open in Finder", "Show in Finder"];
#[cfg(all(unix, not(target_os = "macos")))]
const FILE_MANAGER: [&str; 2] = ["Open in file manager", "Show in file manager"];

/// A row's menu (spec §8): Open / Close for a node, what the row's kind offers, the file manager for a disk entry, then
/// the structure commands every row has.
pub fn row_menu(s: &crate::app::State, row: &view::Row) -> Vec<PItem> {
    use crate::patterns::Cmd as P;
    use crate::rack::RCmd;
    let r = |label: &'static str, c: RowCmd| PItem::new(label, Cmd::Browser(BCmd::Row(c)));
    let sep = |items: &mut Vec<PItem>| {
        if items.last().is_some_and(|i| !i.sep) {
            items.push(PItem::sep());
        }
    };
    let mut items = Vec::new();
    if let Some(open) = row.open {
        // While searching every node shows open.
        let it = r(if open { "Close" } else { "Open" }, RowCmd::Toggle);
        items.push(if s.browser_search.is_empty() { it } else { it.off() });
    }
    match row.item {
        Item::Pattern(_) => {
            let p = |label: &'static str, c: P| r(label, RowCmd::Pattern(c));
            items.extend([
                r("Select pattern", RowCmd::Activate),
                PItem::sep(),
                p("Rename and colour...", P::Rename),
                p("Change color...", P::ChangeColor),
                p("Clone", P::Clone),
                p("Delete", P::Delete),
                PItem::sep(),
                p("Select in arranger", P::SelectInPlaylist),
                p("Split by instrument", P::SplitByChannel),
            ]);
        }
        Item::Channel(c) => {
            let k = |label: &'static str, rc: RCmd| PItem::new(label, Cmd::Rack(rc));
            items.extend([
                r("Select channel", RowCmd::Activate),
                r("Show window", RowCmd::ChannelWindow),
                k("Piano roll", RCmd::PianoRoll(c)),
                PItem::sep(),
                k("Rename, color and icon...", RCmd::Rename(c)),
                k("Clone", RCmd::Clone(c)),
                k("Delete...", RCmd::Delete(c)),
            ]);
        }
        Item::Sample(_) => {
            let send = r("Send to selected channel", RowCmd::Send);
            let sampler = s.project.channels.get(s.selected_channel).is_some_and(|c| c.kind.is_sampler());
            items.extend([r("Preview", RowCmd::Preview), if sampler { send } else { send.off() }, r("Add to instrument rack", RowCmd::NewChannel)]);
        }
        Item::Generator(index) => items.push(PItem::new("Add to instrument rack", Cmd::Plugin(crate::plugin_menu::PlugCmd::Generator { index, after: None }))),
        Item::Effect(index) => items.push(PItem::new("Add to selected mixer track", Cmd::Plugin(crate::plugin_menu::PlugCmd::Effect { index, slot: None }))),
        Item::History(_) => items.push(r("Go to this step", RowCmd::Activate)),
        Item::Version(_) => items.push(r("Open this version", RowCmd::Activate)),
        // Into the selected channel's plugin (spec: plugins/presets.md "Browser").
        Item::Preset(ref p) => {
            let key = s.selected_channel;
            let it = PItem::new("Load into selected channel", Cmd::Preset(crate::presets::PCmd::Load(key, p.into())));
            items.push(if s.hosted_rows.contains(&key) { it } else { it.off() });
        }
        Item::None | Item::File(_) | Item::Param(..) => {}
    }
    if let Some(path) = &row.path {
        sep(&mut items);
        items.push(if row.open.is_some() { r(FILE_MANAGER[0], RowCmd::OpenFolder) } else { r(FILE_MANAGER[1], RowCmd::ShowInFolder) });
        if s.browser_folders.contains(path) && row.key == view::disk_key(path) {
            items.extend([r("Rename...", RowCmd::Rename), r("Remove from browser", RowCmd::Remove)]);
        }
    }
    // Files and plugins can be favorites (§10; folders cannot).
    if matches!(row.item, Item::Sample(_) | Item::Preset(_) | Item::File(_) | Item::Generator(_) | Item::Effect(_)) {
        sep(&mut items);
        items.push(r("Favorite", RowCmd::Favorite).mark(Mark::Check(view::is_favorite(s, row))));
    }
    sep(&mut items);
    items.extend([
        PItem::new("Collapse structure", Cmd::Browser(BCmd::Collapse)).key("Ctrl+Up"),
        PItem::new("Reread structure", Cmd::Browser(BCmd::Reread)).key("Ctrl+R"),
    ]);
    items
}

/// The browser options menu, from the FL screenshot (spec §4).
pub fn options_menu(s: &crate::app::State) -> Vec<PItem> {
    let b = |label: &'static str, c: BCmd| PItem::new(label, Cmd::Browser(c));
    let t = |label: &'static str, k: &'static str| b(label, BCmd::Toggle(k)).mark(Mark::Check(view::opt(s, k)));
    // Options with no effect yet: greyed out, showing their default tick.
    let dead = |label: &'static str, k: &'static str| PItem::not_yet(label).mark(Mark::Check(view::opt(s, k)));
    let searching = !s.browser_search.is_empty();
    let find = |label, key, dir| {
        let it = b(label, BCmd::Find(dir)).key(key);
        if searching { it } else { it.off() }
    };
    vec![
        PItem::head("Search"),
        find("Find previous", "F2", -1),
        find("Find next", "F3", 1),
        PItem::sep(),
        t("Show parent folders in search results", view::OPT_PARENTS),
        t("Expand folders", view::OPT_EXPAND),
        t("Search for folders", view::OPT_FOLDERS),
        t("Search box on top", view::OPT_TOP),
        t("Type to filter", view::OPT_TYPE),
        dead("Load plugin tags", view::OPT_TAGS),
        t("Enable indexing", view::OPT_INDEX),
        t("Re-index changed folders on startup", view::OPT_REINDEX),
        PItem::head("Navigation history"),
        b("Previous", BCmd::History(-1)),
        b("Next", BCmd::History(1)),
        PItem::head("Actions"),
        b("Send previous to selected channel", BCmd::Send(-1)).key("Shift+Up"),
        b("Send next to selected channel", BCmd::Send(1)).key("Shift+Down"),
        PItem::head("View"),
        dead("Auto-hide", view::OPT_AUTOHIDE),
        dead("Hide while dragging (when detached)", view::OPT_HIDEDRAG),
        dead("Auto contrast text", view::OPT_CONTRAST),
        PItem::sub("Show icons and text on tabs", vec![PItem::not_yet("Contents not captured yet")]),
        t("Full sample preview", view::OPT_PREVIEW),
        PItem::head("Structure"),
        b("Collapse structure", BCmd::Collapse).key("Ctrl+Up"),
        b("Reread structure", BCmd::Reread).key("Ctrl+R"),
        PItem::head("Content"),
        b("Configure extra folders", BCmd::Configure),
        b("Rescan plugins (fast)", BCmd::Rescan),
    ]
}

/// Reread structure (spec §4): folders are read from disk again, the index too (§11); open nodes and the selection
/// stay.
fn reread(s: &mut crate::app::State) {
    s.browser_dirs.borrow_mut().clear();
    s.browser_index.clear();
    s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
}

/// Select a row and record it in the navigation history (spec §4).
pub(crate) fn select(s: &mut crate::app::State, key: &str) {
    s.browser_sel = Some(key.to_string());
    if s.browser_hist.get(s.browser_hist_pos).map(String::as_str) != Some(key) {
        let keep = if s.browser_hist.is_empty() { 0 } else { s.browser_hist_pos + 1 };
        s.browser_hist.truncate(keep);
        s.browser_hist.push(key.to_string());
        s.browser_hist_pos = s.browser_hist.len() - 1;
    }
}

/// Ctrl+Z with "Show undo history in the browser when undoing changes": the browser shows Current project ▸ History,
/// opened, where the project now is selected (spec: ui/settings-choices.md "Undo history: the four checks
/// (2026-10-16)"). The All tab shows it too; the others switch to the project's.
pub(crate) fn show_history(s: &mut crate::app::State) {
    s.show_browser = true;
    if s.browser_tab > 1 {
        s.browser_tab = 1;
    }
    s.browser_search.clear();
    let root = format!("{}:Current project", s.browser_tab);
    let folder = format!("{root}/History");
    s.browser_sel = Some(format!("{folder}/{}", s.history.at()));
    s.browser_open.extend([root, folder]);
    reveal(s);
}

/// Scroll the selected row into view.
pub(crate) fn reveal(s: &mut crate::app::State) {
    let rows = view::rows(s);
    let Some(i) = rows.iter().position(|r| Some(&r.key) == s.browser_sel.as_ref()) else { return };
    let y = i as f32 * view::ROW_H;
    let h = view::list_h(s);
    if y < s.browser_scroll {
        s.browser_scroll = y;
    } else if y + view::ROW_H > s.browser_scroll + h {
        s.browser_scroll = y + view::ROW_H - h;
    }
    s.browser_scroll = s.browser_scroll.clamp(0.0, view::max_scroll(s));
}

/// Index of the next row after the selection (wrapping) for which `ok` holds.
fn step(s: &crate::app::State, rows: &[view::Row], dir: isize, ok: impl Fn(&view::Row) -> bool) -> Option<usize> {
    let n = rows.len() as isize;
    let cur = rows.iter().position(|r| Some(&r.key) == s.browser_sel.as_ref()).map_or(if dir > 0 { -1 } else { n }, |i| i as isize);
    (1..=n).map(|k| (cur + dir * k).rem_euclid(n.max(1)) as usize).find(|&i| ok(&rows[i]))
}

/// Find previous / next match of the search text.
fn find(s: &mut crate::app::State, dir: isize) -> Option<String> {
    let q = s.browser_search.to_lowercase();
    if q.is_empty() {
        return None;
    }
    let rows = view::rows(s);
    let Some(i) = step(s, &rows, dir, |r| view::matches(s, r, &q)) else {
        return Some(format!("\"{}\" not found", s.browser_search));
    };
    let key = rows[i].key.clone();
    select(s, &key);
    reveal(s);
    None
}

/// Send the previous / next sample row to the selected channel (`send_sample`).
fn send(s: &mut crate::app::State, dir: isize) -> Option<String> {
    let rows = view::rows(s);
    let Some(i) = step(s, &rows, dir, |r| matches!(r.item, Item::Sample(_))) else {
        return Some("No samples in the browser to send".into());
    };
    let Item::Sample(path) = rows[i].item.clone() else { return None };
    let key = rows[i].key.clone();
    select(s, &key);
    reveal(s);
    send_sample(s, &path, &rows[i].label)
}

/// Sample `path` (shown as `label`) into the selected channel, which takes the file's name while "Name channels after their sample or plugin"
/// is on (`rack::auto_name`). One history step (`history::load_sample`).
fn send_sample(s: &mut crate::app::State, path: &str, label: &str) -> Option<String> {
    let row = s.selected_channel;
    if !s.project.channels.get(row)?.kind.is_sampler() {
        return Some("The selected channel is not a sampler".into());
    }
    // With Copy used samples to the project folder: the sample's copy in the data folder (`data_folder::adopt`).
    let path = crate::data_folder::adopt(s, path);
    crate::history::load_sample(s, row);
    let ch = &mut s.project.channels[row];
    crate::rack::auto_name(ch, std::path::Path::new(&path));
    crate::rack::read_tags(ch, std::path::Path::new(&path));
    ch.sample_path = Some(path.clone());
    s.sample_loads.push((row, path));
    s.piano.dirty = true;
    Some(format!("Sent {label} to the selected channel"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::{Action, App};
    use os_ui::Canvas;

    /// A temporary folder holding `files` (paths relative to it; audio files get a short WAV, others nothing).
    fn folder(name: &str, files: &[&str]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("os_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for f in files {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            if f.ends_with(".wav") {
                os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&p).unwrap(), 44100, &[0.0; 8]).unwrap();
            } else {
                std::fs::write(&p, b"").unwrap();
            }
        }
        dir
    }

    /// Right-clicks the first row `find` accepts, as the mouse does; the menu it opens.
    fn right(app: &mut App, c: &mut Canvas, find: impl Fn(&view::Row) -> bool) -> Vec<PItem> {
        app.paint(c, None);
        let i = view::rows(&app.state).iter().position(find).expect("the row");
        let r = app.hits.iter().rev().find(|h| h.action == Action::BrowserRow(i)).expect("row on screen").rect;
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down_right();
        app.state.playlist.popup.take().expect("a menu").items
    }

    fn labels(app: &App) -> Vec<String> {
        view::rows(&app.state).into_iter().map(|r| r.label).collect()
    }

    /// Favorites (spec: ui/browser.md §10): a file's and a plugin's row menu tick Favorite; the Favorites tab lists the
    /// favorite plugins, then the files, and drops a row whose Favorite is unticked; folders cannot be favorites; the
    /// plugin windows' and mixer slots' "Add to plugin database (flag as favorite)" star the plugin.
    #[test]
    fn favorites() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let dir = folder("browser_favs", &["Kick 01.wav", "readme.txt"]);
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::builtins();
        app.state.plugdb.rebuild(&app.plugins);
        app.state.browser_folders.push(dir.clone());
        app.state.browser_open.insert(view::disk_key(&dir));
        let fav = |items: &[PItem]| items.iter().find(|i| i.label == "Favorite").map(|i| i.mark);
        assert_eq!(fav(&right(&mut app, &mut c, |r| r.label == "Kick 01")), Some(Mark::Check(false)));
        app.browser_cmd(BCmd::Row(RowCmd::Favorite));
        let kick = dir.join("Kick 01.wav").to_string_lossy().into_owned();
        assert_eq!(app.state.browser_favs, [Fav { path: kick.clone(), label: "Kick 01".into() }]);
        assert_eq!(fav(&right(&mut app, &mut c, |r| r.label == "Kick 01")), Some(Mark::Check(true)));
        assert_eq!(fav(&right(&mut app, &mut c, |r| r.key == view::disk_key(&dir))), None, "a folder is no favorite");
        // Lemon Sampler in the Plugin database: its star.
        app.state.browser_tab = view::PLUGIN_TAB;
        app.state.browser_open.extend(["2:Plugin library/Generators".into(), "2:Plugin library/Generators/Sampler".into()]);
        assert_eq!(fav(&right(&mut app, &mut c, |r| r.item == Item::Generator(0))), Some(Mark::Check(false)));
        app.browser_cmd(BCmd::Row(RowCmd::Favorite));
        let uid = app.plugins.available[0].desc.uid();
        assert!(app.state.plugdb.prefs.is_favorite(&uid) && app.state.plugdb.lists.gens.iter().any(|e| e.index == 0 && e.favorite));
        // The Favorites tab: the plugin, then the file, each starred; the file row shows its info.
        app.act(Action::BrowserTab(view::FAV_TAB));
        assert_eq!(labels(&app), [crate::plugins::LEMON_SAMPLER, "Kick 01"]);
        let rows = view::rows(&app.state);
        assert!(rows.iter().all(|r| view::is_favorite(&app.state, r)));
        assert_eq!(rows[1].item, Item::Sample(kick.clone()));
        assert_eq!(rows[1].path.as_deref(), Some(dir.join("Kick 01.wav").as_path()));
        // A favorite dragged from the tab is the sample it stands for (as in any tab).
        app.paint(&mut c, None);
        assert!(app.hits.iter().any(|h| h.action == Action::BrowserRow(1)));
        // Unticked in the tab, the row goes.
        right(&mut app, &mut c, |r| r.label == "Kick 01");
        app.browser_cmd(BCmd::Row(RowCmd::Favorite));
        assert_eq!(labels(&app), [crate::plugins::LEMON_SAMPLER]);
        assert!(app.state.browser_favs.is_empty());
        // "Add to plugin database (flag as favorite)" of a hosted plugin toggles the same star.
        app.state.project = crate::app::test_project();
        let row = 0;
        app.plugins.create(row, 0).expect("Lemon Sampler hosted");
        assert!(app.browser_cmd(BCmd::Star(row)).is_some_and(|m| m.contains("no longer")));
        assert!(!app.state.plugdb.prefs.is_favorite(&uid));
        assert_eq!(labels(&app), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Search with the folder index (spec §5, §11): files in folders nobody opened are found, flat or under their
    /// folders; Expand folders lists a found folder's contents; without indexing only opened folders are searched;
    /// Reread structure reads the index again; the cross clears the search.
    #[test]
    fn search_finds_files_in_closed_folders() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let dir = folder("browser_find", &["A/Bzz/Qzx Kick.wav", "A/Bzz/notes.txt", "Other/Snare.wav"]);
        let mut app = App::new(None);
        app.state.browser_folders.push(dir.clone());
        app.state.browser_search = "qzx".into();
        assert!(view::rows(&app.state).is_empty(), "no index yet");
        app.browser_index_tick();
        assert_eq!(labels(&app), ["Qzx Kick"]);
        let root = view::root_label(&app.state, &dir);
        app.browser_cmd(BCmd::Toggle(view::OPT_PARENTS));
        assert_eq!(labels(&app), [root.as_str(), "A", "Bzz", "Qzx Kick"]);
        let depths: Vec<u8> = view::rows(&app.state).iter().map(|r| r.depth).collect();
        assert_eq!(depths, [0, 1, 2, 3]);
        app.browser_cmd(BCmd::Toggle(view::OPT_PARENTS));
        // Find next selects the match; a click on it is a sample row (preview, drag).
        assert_eq!(app.browser_cmd(BCmd::Find(1)), None);
        assert_eq!(app.state.browser_sel.as_ref(), Some(&view::rows(&app.state)[0].key));
        // Expand folders: the found folder with what it holds.
        app.state.browser_search = "bzz".into();
        assert_eq!(labels(&app), ["Bzz"]);
        app.browser_cmd(BCmd::Toggle(view::OPT_EXPAND));
        assert_eq!(labels(&app), ["Bzz", "notes.txt", "Qzx Kick"]);
        assert_eq!(view::rows(&app.state)[1].depth, 1);
        app.browser_cmd(BCmd::Toggle(view::OPT_EXPAND));
        // Indexing off: closed folders are not searched.
        app.state.browser_search = "qzx".into();
        app.browser_cmd(BCmd::Toggle(view::OPT_INDEX));
        assert!(view::rows(&app.state).is_empty());
        app.browser_cmd(BCmd::Toggle(view::OPT_INDEX));
        // Reread structure forgets the index; the tick reads the folders again (a file added meanwhile is found).
        std::fs::write(dir.join("Other").join("Qzx Snare.wav"), b"").unwrap();
        app.browser_cmd(BCmd::Reread);
        assert!(view::rows(&app.state).is_empty());
        app.browser_index_tick();
        assert_eq!(labels(&app), ["Qzx Kick", "Qzx Snare"]);
        // The cross right of the field clears the search.
        app.paint(&mut c, None);
        assert!(!app.hits.iter().any(|h| h.action == Action::BrowserSearch && h.rect.w < 30.0), "the magnifier gave way");
        app.act(Action::BrowserClear);
        assert!(app.state.browser_search.is_empty());
        app.paint(&mut c, None);
        assert!(!app.hits.iter().any(|h| h.action == Action::BrowserClear));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The browser's new pages to look at: `OS_SHOT_DIR=dir cargo test -p os-app browser_shots` writes the window with
    /// the Plugin database opened, the Favorites tab and an indexed search.
    #[test]
    fn browser_shots() {
        let Some(out) = std::env::var_os("OS_SHOT_DIR").map(std::path::PathBuf::from) else { return };
        let Ok(mut c) = Canvas::new(1280, 900, 1.0) else { return };
        let dir = folder("browser_shots", &["Drums/Kicks/Big Kick.wav", "Drums/Kicks/Kick Tail.wav", "Drums/Snares/Kick Snare Combo.wav", "readme.txt"]);
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::builtins();
        app.state.plugdb.rebuild(&app.plugins);
        app.state.browser_folders.push(dir.clone());
        let mut shot = |app: &mut App, name: &str| {
            app.paint(&mut c, None);
            c.pixmap.save_png(out.join(format!("browser_{name}.png"))).unwrap();
        };
        app.state.browser_tab = view::PLUGIN_TAB;
        let root = "2:Plugin library";
        app.state.browser_open.extend([format!("{root}/Effects"), format!("{root}/Effects/Dynamics"), format!("{root}/{}", view::INSTALLED), format!("{root}/{}/Generators", view::INSTALLED), format!("{root}/{}/Generators/{}", view::INSTALLED, view::BUILT_IN)]);
        let star = view::rows(&app.state).iter().position(|r| r.depth == 3).unwrap();
        app.state.browser_sel = Some(view::rows(&app.state)[star].key.clone());
        app.browser_cmd(BCmd::Row(RowCmd::Favorite));
        shot(&mut app, "plugin_database");
        app.state.browser_favs.push(Fav { path: dir.join("Drums/Kicks/Big Kick.wav").to_string_lossy().into_owned(), label: "Big Kick".into() });
        app.act(Action::BrowserTab(view::FAV_TAB));
        shot(&mut app, "favorites");
        app.act(Action::BrowserTab(0));
        app.state.browser_search = "kick".into();
        app.browser_cmd(BCmd::Toggle(view::OPT_PARENTS));
        app.browser_index_tick();
        shot(&mut app, "search");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Plugin database (spec §2): Generators and Effects by category, Installed by format; the mixer effect list's
    /// Plugin database entry opens its Effects, and Configure extra folders opens File ▸ Browser extra search folders.
    #[test]
    fn plugin_database_by_category_and_format() {
        let mut app = App::new(None);
        app.plugins = crate::plugins::Plugins::builtins();
        app.state.plugdb.rebuild(&app.plugins);
        app.state.browser_tab = view::PLUGIN_TAB;
        let root = "2:Plugin library";
        app.state.browser_open.extend([format!("{root}/Generators"), format!("{root}/{}", view::INSTALLED), format!("{root}/{}/Generators", view::INSTALLED)]);
        let rows = view::rows(&app.state);
        let at = |depth: u8| rows.iter().filter(|r| r.depth == depth).map(|r| r.label.as_str()).collect::<Vec<_>>();
        assert_eq!(at(1), ["Generators", "Effects", view::INSTALLED]);
        assert!(at(2).starts_with(&["Sampler", "Synth"]), "{:?}", at(2));
        assert!(at(3).contains(&view::BUILT_IN));
        app.state.browser_open.insert(format!("{root}/{}/Generators/{}", view::INSTALLED, view::BUILT_IN));
        let built_in: Vec<String> = view::rows(&app.state).into_iter().filter(|r| r.depth == 4).map(|r| r.label).collect();
        assert!(built_in.iter().any(|l| l == crate::plugins::LEMON_SAMPLER), "{built_in:?}");
        // The mixer's empty-slot menu: Plugin database works and opens the Effects section.
        let menu = crate::mixer::effect_menu(&app.state, 0, 0.0, 0.0);
        let first = &menu.items[0];
        assert!(first.label == "Plugin library" && first.enabled);
        app.state.browser_tab = 0;
        app.browser_cmd(BCmd::Jump(Jump::DbEffects));
        let fx = format!("{root}/Effects");
        assert!(app.state.browser_tab == view::PLUGIN_TAB && app.state.browser_open.contains(&fx) && app.state.browser_sel.as_ref() == Some(&fx));
        // The options menu: Expand folders and the indexing options work; tags stay greyed out.
        let opts = options_menu(&app.state);
        let on = |l: &str| opts.iter().find(|i| i.label == l).unwrap().enabled;
        assert!(on("Expand folders") && on("Enable indexing") && on("Re-index changed folders on startup") && on("Configure extra folders"));
        assert!(!on("Load plugin tags"));
        app.browser_cmd(BCmd::Configure);
        assert!(app.state.settings.open);
    }

    /// `browser.txt` (spec: ui/browser.md §9): the dragged width, never below the default; anything else is the default.
    #[test]
    fn kept_width() {
        assert_eq!(parse_width("width=359\n"), 359.0);
        assert_eq!(parse_width("width=120"), crate::views::BROWSER_W);
        assert_eq!(parse_width("width=wide\n"), crate::views::BROWSER_W);
        assert_eq!(parse_width(""), crate::views::BROWSER_W);
        assert_eq!(parse_width("options=top\nwidth=300\n"), 300.0);
    }

    /// The options are kept in `browser.txt` beside the width (spec §4): missing = the defaults, unknown names dropped.
    #[test]
    fn kept_options() {
        assert_eq!(parse_opts("width=300\n"), view::DEFAULT_OPTS.iter().copied().collect());
        assert_eq!(parse_opts("options=top, expand,nonsense\n"), [view::OPT_TOP, view::OPT_EXPAND].into_iter().collect());
        assert!(parse_opts("options=\n").is_empty());
        let text = with_value("width=300\noptions=top\n", "options", "expand");
        assert_eq!(text, "width=300\noptions=expand\n");
        assert_eq!(parse_width(&with_value(&text, "width", "250")), 250.0);
    }

    /// `browser_favorites.txt` (spec §10): path and label per line; a line without a label shows the file's name.
    #[test]
    fn kept_favorites() {
        let favs = vec![Fav { path: "C:\\Kits\\Kick.wav".into(), label: "Kick".into() }, Fav { path: "/x/y.fxp".into(), label: "y".into() }];
        assert_eq!(parse_favs(&favs_text(&favs)), favs);
        assert_eq!(parse_favs("C:\\Kits\\Snare.wav\n\n")[0].label, "Snare.wav");
    }
}
