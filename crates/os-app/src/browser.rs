// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Browser behaviour: tabs, tree open/close, selection, scrolling and drag and drop into the project.
// spec: ui/browser.md

use os_model::{ChannelKind, ClipSource};

use crate::app::{Action, App, Drag, Win};
use crate::playlist::{Cmd, Mark, PItem, Popup};
use crate::playlist::{self as pl, PickKind};
use crate::views::browser::{self as view, Item};

/// Mouse travel before a press on an item becomes a drag (spec §3).
const DRAG_START: f32 = 4.0;
/// How long a clicked sample plays (spec §7).
const PREVIEW_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

impl App {
    /// Browser actions; returns whether `a` was one.
    pub(crate) fn browser_act(&mut self, a: &Action) -> bool {
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
                let (x, y) = view::MENU_AT;
                let open = s.playlist.popup.as_ref().is_some_and(|p| p.x == x && p.y == y);
                s.playlist.popup = if open { None } else { Some(Popup { x, y, items: options_menu(s), sub: None }) };
            }
            Action::BrowserSearch => s.browser_search_focus = true,
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
                    _ => {}
                }
                if let Item::Sample(path) = &row.item {
                    self.preview_sample(path);
                }
                let s = &mut self.state;
                s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
                if !matches!(row.item, Item::None | Item::File(_)) {
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
        }
        None
    }

    /// Whether the mouse is over the docked browser.
    pub(crate) fn over_browser(&self) -> bool {
        let (x, y) = self.mouse;
        self.state.show_browser && x < crate::views::BROWSER_W && y >= crate::views::WORK_Y
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
                        s.message = Some("Only audio and automation channels go on the playlist; drop a pattern instead".into());
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
            Item::None | Item::File(_) => {}
        }
    }

    /// A folder dropped on the window becomes an All-tab root, opened and selected (spec §6).
    pub(crate) fn add_browser_folder(&mut self, path: std::path::PathBuf) {
        let s = &mut self.state;
        if !s.browser_folders.contains(&path) {
            s.browser_folders.push(path.clone());
            save_folders(&s.browser_folders);
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

    /// Right-click on a dropped folder root removes it from the browser (spec §6); returns whether it was one.
    pub(crate) fn browser_remove_root(&mut self, i: usize) -> bool {
        let s = &mut self.state;
        let Some(row) = view::rows(s).into_iter().nth(i) else { return false };
        let Some(path) = row.path.filter(|_| row.depth == 0) else { return false };
        s.browser_folders.retain(|f| *f != path);
        save_folders(&s.browser_folders);
        s.browser_open.retain(|k| !k.starts_with(&row.key));
        s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
        s.message = Some(format!("Removed {} from the browser", path.display()));
        self.invalidate();
        true
    }
}

/// `browser_folders.txt` in the user folder (`os_paths::user_dir`): one dropped folder per line (spec §6).
fn folders_file() -> Option<std::path::PathBuf> {
    if cfg!(test) {
        return None;
    }
    os_paths::user_file("browser_folders.txt")
}

pub(crate) fn load_folders() -> Vec<std::path::PathBuf> {
    let Some(text) = folders_file().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    text.lines().map(str::trim).filter(|l| !l.is_empty()).map(std::path::PathBuf::from).collect()
}

fn save_folders(folders: &[std::path::PathBuf]) {
    let Some(p) = folders_file() else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let text: String = folders.iter().map(|f| format!("{}\r\n", f.display())).collect();
    let _ = std::fs::write(p, text);
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
        dead("Expand folders", view::OPT_EXPAND),
        t("Search for folders", view::OPT_FOLDERS),
        t("Search box on top", view::OPT_TOP),
        t("Type to filter", view::OPT_TYPE),
        dead("Load plugin tags", view::OPT_TAGS),
        dead("Enable indexing", view::OPT_INDEX),
        dead("Re-index changed folders on startup", view::OPT_REINDEX),
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
        PItem::not_yet("Configure extra folders"),
        b("Refresh plugin list (fast scan)", BCmd::Rescan),
    ]
}

/// Reread structure (spec §4): folders are read from disk again; open nodes and the selection stay.
fn reread(s: &mut crate::app::State) {
    s.browser_dirs.borrow_mut().clear();
    s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
}

/// Select a row and record it in the navigation history (spec §4).
fn select(s: &mut crate::app::State, key: &str) {
    s.browser_sel = Some(key.to_string());
    if s.browser_hist.get(s.browser_hist_pos).map(String::as_str) != Some(key) {
        let keep = if s.browser_hist.is_empty() { 0 } else { s.browser_hist_pos + 1 };
        s.browser_hist.truncate(keep);
        s.browser_hist.push(key.to_string());
        s.browser_hist_pos = s.browser_hist.len() - 1;
    }
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

/// Send the previous / next sample row to the selected channel.
fn send(s: &mut crate::app::State, dir: isize) -> Option<String> {
    let rows = view::rows(s);
    let Some(i) = step(s, &rows, dir, |r| matches!(r.item, Item::Sample(_))) else {
        return Some("No samples in the browser to send".into());
    };
    let Item::Sample(path) = rows[i].item.clone() else { return None };
    let key = rows[i].key.clone();
    select(s, &key);
    reveal(s);
    let Some(ch) = s.project.channels.get_mut(s.selected_channel) else { return None };
    if !ch.kind.is_sampler() {
        return Some("The selected channel is not a sampler".into());
    }
    ch.sample_path = Some(path.clone());
    let row = s.selected_channel;
    s.sample_loads.push((row, path));
    s.piano.dirty = true;
    Some(format!("Sent {} to the selected channel", rows[i].label))
}
