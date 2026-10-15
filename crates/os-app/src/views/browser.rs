// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Docked browser (left): five tabs, a tree of the project / plugin database / folders, drag sources.
// spec: ui/main-window.md §4, ui/browser.md (§4 options menu, §5 search, §6 dropped folders)

use std::path::{Path, PathBuf};
use std::rc::Rc;

use os_ui::{Color, Rect};

use crate::app::{Action, State, Ui};
use crate::icons::{self, Icon};

/// Row pitch and indent per depth level (spec: ui/browser.md §2).
pub const ROW_H: f32 = 23.0;
const INDENT: f32 = 13.0;
/// Top of the list area.
pub const LIST_Y: f32 = 142.0;
/// Footer height below the list.
pub const FOOTER_H: f32 = 28.0;

/// Category colour indices (`cat_color`); 255 = folder.
const FOLDER: u8 = 255;
/// Category index of files in dropped folders (text colour, spec §6).
const FILE: u8 = 254;
/// Height of the info panel above the footer (spec §6).
pub const INFO_H: f32 = 84.0;

/// (label, icon, category colour index). Only the roots with content: FL's disk-folder roots (Recent files, presets,
/// Packs...) are left out until Lemon Studio has content for them (owner decision); dropped folders follow these.
const ROOTS: &[(&str, Icon, u8)] = &[("Current project", Icon::Page, 0), ("Plugin database", Icon::Speaker, 1)];

/// The "Current project" tree's sections, in screenshot order (spec §2), without the ones nothing is tracked in yet
/// (History, Remote control, Initialized controls).
const PROJECT: &[(&str, Icon)] = &[("Patterns", Icon::Note), ("Effects", Icon::Speaker), ("Generators", Icon::Plug), ("Samples", Icon::Wave)];

/// What a row stands for; everything but `None` can be dragged (spec §3).
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    None,
    /// 1-based pattern number.
    Pattern(u16),
    /// Rack row.
    Channel(usize),
    /// Sample file path.
    Sample(String),
    /// A generator (index into `Plugins::available`).
    Generator(usize),
    /// An effect (index into `Plugins::effects`).
    Effect(usize),
    /// A non-audio file in a dropped folder (not draggable, spec §6).
    File(String),
    /// A parameter of a hosted plugin: hosted key, parameter index (spec: plugins/plugin-parameters.md §6).
    Param(usize, usize),
}

#[derive(Debug, Clone)]
pub struct Row {
    /// Open/closed state key (tab-qualified path).
    pub key: String,
    pub depth: u8,
    pub label: String,
    pub icon: Icon,
    pub cat: u8,
    /// `Some(open)` for an expandable row.
    pub open: Option<bool>,
    /// Drawn brighter (current pattern, opened roots).
    pub bright: bool,
    pub item: Item,
    /// Disk entry (dropped folders, spec §6): shown in the info panel.
    pub path: Option<PathBuf>,
}

/// Keys of the rows that start opened (the project tab shows Patterns, like the screenshot).
pub fn default_open() -> std::collections::HashSet<String> {
    ["1:Current project", "1:Current project/Patterns", "2:Plugin database"].iter().map(|k| k.to_string()).collect()
}

/// Options-menu toggles (spec §4); `DEFAULT_OPTS` are on in a fresh browser.
pub const OPT_PARENTS: &str = "parents";
pub const OPT_EXPAND: &str = "expand";
pub const OPT_FOLDERS: &str = "folders";
pub const OPT_TOP: &str = "top";
pub const OPT_TYPE: &str = "type";
pub const OPT_TAGS: &str = "tags";
pub const OPT_INDEX: &str = "index";
pub const OPT_REINDEX: &str = "reindex";
pub const OPT_AUTOHIDE: &str = "autohide";
pub const OPT_HIDEDRAG: &str = "hidedrag";
pub const OPT_CONTRAST: &str = "contrast";
pub const OPT_PREVIEW: &str = "preview";
pub const DEFAULT_OPTS: &[&str] = &[OPT_FOLDERS, OPT_TAGS, OPT_INDEX, OPT_REINDEX, OPT_HIDEDRAG];

pub fn opt(s: &State, k: &str) -> bool {
    s.browser_opts.contains(k)
}

/// Whether `row` matches the search text `q` (lower case), spec §5.
pub fn matches(s: &State, row: &Row, q: &str) -> bool {
    !q.is_empty() && (opt(s, OPT_FOLDERS) || row.open.is_none()) && row.label.to_lowercase().contains(q)
}

/// A node is open when the user opened it, or always while searching (search covers the whole tree).
fn is_open(s: &State, key: &str) -> bool {
    !s.browser_search.is_empty() || s.browser_open.contains(key)
}

/// The visible rows of the active tab, top to bottom (filtered while searching, spec §5).
pub fn rows(s: &State) -> Vec<Row> {
    let all = tree(s);
    if s.browser_search.is_empty() {
        return all;
    }
    let q = s.browser_search.to_lowercase();
    let hit: Vec<bool> = all.iter().map(|r| matches(s, r, &q)).collect();
    if !opt(s, OPT_PARENTS) {
        return all.into_iter().zip(hit).filter(|(_, h)| *h).map(|(mut r, _)| {
            r.depth = 0;
            r
        }).collect();
    }
    // Keep matches and their ancestors.
    let mut keep = hit.clone();
    let mut stack: Vec<usize> = Vec::new();
    for (i, r) in all.iter().enumerate() {
        while stack.last().is_some_and(|&p| all[p].depth >= r.depth) {
            stack.pop();
        }
        if hit[i] {
            for &p in &stack {
                keep[p] = true;
            }
        }
        stack.push(i);
    }
    all.into_iter().zip(keep).filter(|(_, k)| *k).map(|(r, _)| r).collect()
}

fn tree(s: &State) -> Vec<Row> {
    let mut out = Vec::new();
    let tab = s.browser_tab;
    let mut roots: Vec<Result<usize, usize>> = match tab {
        0 => (0..ROOTS.len()).map(Ok).collect(),
        1 => vec![Ok(0)],
        2 => vec![Ok(1)],
        _ => Vec::new(),
    };
    if tab == 0 {
        // Dropped folders follow the roots, in alphabetical order (spec §6).
        let name = |r: &Result<usize, usize>| match *r {
            Ok(i) => ROOTS[i].0.to_lowercase(),
            Err(f) => folder_name(&s.browser_folders[f]).to_lowercase(),
        };
        for f in 0..s.browser_folders.len() {
            let me = folder_name(&s.browser_folders[f]).to_lowercase();
            let at = (ROOTS.len()..roots.len()).find(|&j| name(&roots[j]) > me).unwrap_or(roots.len());
            roots.insert(at, Err(f));
        }
    }
    for r in roots {
        let r = match r {
            Ok(r) => r,
            Err(f) => {
                let path = &s.browser_folders[f];
                let key = disk_key(path);
                let open = s.browser_open.contains(&key);
                let icon = if open { Icon::FolderOpen } else { Icon::Folder };
                out.push(Row { key: key.clone(), depth: 0, label: folder_name(path), icon, cat: FOLDER, open: Some(open), bright: true, item: Item::None, path: Some(path.clone()) });
                if open {
                    disk_rows(s, path, &key, 1, &mut out);
                }
                continue;
            }
        };
        let (label, icon, cat) = ROOTS[r];
        let key = format!("{tab}:{label}");
        let open = is_open(s, &key);
        out.push(Row { key: key.clone(), depth: 0, label: label.into(), icon, cat, open: Some(open), bright: open, item: Item::None, path: None });
        if !open {
            continue;
        }
        match r {
            0 => project_rows(s, &key, cat, &mut out),
            _ => plugin_rows(s, &key, cat, &mut out),
        }
    }
    out
}

fn section(s: &State, parent: &str, label: &str, icon: Icon, cat: u8, out: &mut Vec<Row>) -> bool {
    let key = format!("{parent}/{label}");
    let open = is_open(s, &key);
    out.push(Row { key, depth: 1, label: label.into(), icon, cat, open: Some(open), bright: open, item: Item::None, path: None });
    open
}

fn leaf(out: &mut Vec<Row>, parent: &str, label: String, icon: Icon, cat: u8, bright: bool, item: Item) {
    out.push(Row { key: format!("{parent}/{label}"), depth: 2, label, icon, cat, open: None, bright, item, path: None });
}

/// Tree key of a dropped folder root (All tab).
pub fn disk_key(path: &Path) -> String {
    format!("0:@{}", path.display())
}

/// A dropped folder's label: its name (the whole path for a drive root).
pub fn folder_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// One directory entry of a dropped folder (spec §6).
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

/// The listing of `dir`: subfolders, then files, each sorted by name, hidden/system entries left out
/// (spec §6). Cached until the structure is reread (Ctrl+R).
pub fn list_dir(s: &State, dir: &Path) -> Rc<Vec<Entry>> {
    if let Some(v) = s.browser_dirs.borrow().get(dir) {
        return v.clone();
    }
    let mut v: Vec<Entry> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let low = name.to_ascii_lowercase();
                    let md = e.metadata().ok()?;
                    #[cfg(windows)]
                    {
                        use std::os::windows::fs::MetadataExt;
                        // FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
                        if md.file_attributes() & 0x6 != 0 {
                            return None;
                        }
                    }
                    if name.starts_with('.') || low == "desktop.ini" || low == "thumbs.db" {
                        return None;
                    }
                    Some(Entry { name, path: e.path(), dir: md.is_dir(), size: md.len(), modified: md.modified().ok() })
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_cached_key(|e| (!e.dir, e.name.to_lowercase()));
    let v = Rc::new(v);
    s.browser_dirs.borrow_mut().insert(dir.to_path_buf(), v.clone());
    v
}

/// Icon for a file by extension (spec §6 table).
pub fn file_icon(name: &str) -> Icon {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "wav" | "mp3" | "ogg" | "flac" | "aif" | "aiff" => Icon::FileAudio,
        "mid" | "midi" => Icon::PianoRoll,
        "flp" => Icon::FileProject,
        "txt" | "url" | "pdf" | "nfo" | "rtf" | "htm" | "html" | "md" => Icon::FileText,
        "fxp" | "fxb" | "fst" | "vstpreset" | "nmsv" | "spf" | "h2p" | "clap-preset" => Icon::FilePreset,
        "zip" | "rar" | "7z" => Icon::FileZip,
        _ => Icon::FileBlank,
    }
}

/// Rows of an opened disk folder: its subfolders (recursing into opened ones), then its files.
fn disk_rows(s: &State, dir: &Path, key: &str, depth: u8, out: &mut Vec<Row>) {
    for e in list_dir(s, dir).iter() {
        let k = format!("{key}/{}", e.name);
        if e.dir {
            let open = s.browser_open.contains(&k);
            let icon = if open { Icon::FolderOpen } else { Icon::Folder };
            out.push(Row { key: k.clone(), depth, label: e.name.clone(), icon, cat: FOLDER, open: Some(open), bright: open, item: Item::None, path: Some(e.path.clone()) });
            if open {
                disk_rows(s, &e.path, &k, depth.saturating_add(1), out);
            }
        } else {
            let icon = file_icon(&e.name);
            let p = e.path.to_string_lossy().into_owned();
            let (label, item) = if icon == Icon::FileAudio {
                (e.name.rsplit_once('.').map_or(e.name.as_str(), |(a, _)| a).to_string(), Item::Sample(p))
            } else {
                (e.name.clone(), Item::File(p))
            };
            out.push(Row { key: k, depth, label, icon, cat: FILE, open: None, bright: false, item, path: Some(e.path.clone()) });
        }
    }
}

fn project_rows(s: &State, root: &str, cat: u8, out: &mut Vec<Row>) {
    let p = &s.project;
    for &(name, icon) in PROJECT {
        if !section(s, root, name, icon, cat, out) {
            continue;
        }
        let parent = format!("{root}/{name}");
        match name {
            "Patterns" => {
                let mut pats: Vec<_> = p.patterns.iter().collect();
                pats.sort_by_key(|p| p.number);
                for pat in pats {
                    let label = pat.name.clone().unwrap_or_else(|| format!("Pattern {}", pat.number));
                    leaf(out, &parent, label, Icon::Note, cat, pat.number == s.pattern, Item::Pattern(pat.number));
                }
            }
            // Hosted plugins open into their parameters (spec: plugins/plugin-parameters.md §6).
            "Generators" => {
                for i in 0..p.channels.len() {
                    let label = channel_label(s, i).unwrap_or_default();
                    plugin_node(s, out, &parent, label, Icon::Plug, cat, i == s.selected_channel, Item::Channel(i), i);
                }
            }
            "Effects" => {
                for m in &p.mixer {
                    for k in (0..m.slots.len()).filter(|&k| m.slots[k].is_some()) {
                        let label = crate::params::effect_label(s, m.index, k as u8).unwrap_or_default();
                        plugin_node(s, out, &parent, label, Icon::Speaker, cat, false, Item::None, crate::plugins::fx_key(m.index, k as u8));
                    }
                }
            }
            "Samples" => {
                for c in &p.channels {
                    if let Some(path) = c.sample_path.as_ref().filter(|p| !p.is_empty()) {
                        let file = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
                        leaf(out, &parent, file, Icon::Wave, cat, false, Item::Sample(path.clone()));
                    }
                }
            }
            _ => {}
        }
    }
}

/// A channel's row label under Generators.
pub fn channel_label(s: &State, i: usize) -> Option<String> {
    let c = s.project.channels.get(i)?;
    Some(c.name.clone().or_else(|| crate::project_plugins::plugin_title(c)).unwrap_or_else(|| format!("Channel {}", i + 1)))
}

/// A generator / effect row: a leaf, or a node listing its parameters when it is a hosted plugin that has some.
#[allow(clippy::too_many_arguments)]
fn plugin_node(s: &State, out: &mut Vec<Row>, parent: &str, label: String, icon: Icon, cat: u8, bright: bool, item: Item, key: usize) {
    if !s.params.has_params(key) {
        return leaf(out, parent, label, icon, cat, bright, item);
    }
    let k = format!("{parent}/{label}");
    let open = is_open(s, &k);
    out.push(Row { key: k.clone(), depth: 2, label, icon, cat, open: Some(open), bright: bright || open, item, path: None });
    if open {
        for (index, e) in s.params.visible(key) {
            out.push(Row { key: format!("{k}/{index}"), depth: 3, label: e.name.clone(), icon: Icon::Slide, cat, open: None, bright: false, item: Item::Param(key, index), path: None });
        }
    }
}

/// The plugin menus' generators and effects (spec: ui/plugin-menu.md §5).
fn plugin_rows(s: &State, root: &str, cat: u8, out: &mut Vec<Row>) {
    if section(s, root, "Generators", Icon::Plug, cat, out) {
        let parent = format!("{root}/Generators");
        for e in &s.plugdb.lists.gens {
            leaf(out, &parent, e.label.clone(), Icon::Plug, cat, false, Item::Generator(e.index));
        }
    }
    if section(s, root, "Effects", Icon::Speaker, cat, out) {
        let parent = format!("{root}/Effects");
        for e in &s.plugdb.lists.fx {
            leaf(out, &parent, e.label.clone(), Icon::Speaker, cat, false, Item::Effect(e.index));
        }
    }
}

/// Height of the list viewport for a browser of height `h` (from `WORK_Y`).
pub fn list_h(s: &State) -> f32 {
    (s.screen.1 - list_top(s) - FOOTER_H - info_h(&rows(s), s)).max(0.0)
}

/// The selected disk entry, shown in the info panel (spec §6).
fn info_path(rows: &[Row], s: &State) -> Option<PathBuf> {
    let k = s.browser_sel.as_deref()?;
    rows.iter().find(|r| r.key == k)?.path.clone()
}

fn info_h(rows: &[Row], s: &State) -> f32 {
    if info_path(rows, s).is_some() { INFO_H } else { 0.0 }
}

/// `35 B`, `12.4 KB`, `2.29 MB` (spec §6).
pub fn size_text(n: u64) -> String {
    let f = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if f < 1024.0 * 1024.0 {
        format!("{:.2} KB", f / 1024.0)
    } else if f < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2} MB", f / 1024.0 / 1024.0)
    } else {
        format!("{:.2} GB", f / 1024.0 / 1024.0 / 1024.0)
    }
}

/// Local `M/D/YYYY h:mm:ss AM` (spec §6).
pub fn date_text(t: std::time::SystemTime) -> String {
    let Some((y, mo, d, h, mi, se)) = local_time(t) else { return String::new() };
    let (h12, ap) = match h {
        0 => (12, "AM"),
        1..=11 => (h, "AM"),
        12 => (12, "PM"),
        _ => (h - 12, "PM"),
    };
    format!("{mo}/{d}/{y} {h12}:{mi:02}:{se:02} {ap}")
}

/// A file time in the local time zone: (year, month, day, hour, minute, second).
fn local_time(t: std::time::SystemTime) -> Option<(u16, u16, u16, u16, u16, u16)> {
    use chrono::{Datelike, Timelike};
    let l = chrono::DateTime::<chrono::Local>::from(t);
    let n = |v: u32| u16::try_from(v).ok();
    Some((u16::try_from(l.year()).ok()?, n(l.month())?, n(l.day())?, n(l.hour())?, n(l.minute())?, n(l.second())?))
}

/// The info panel above the footer: title, date, size, path of the selected disk entry (spec §6).
fn paint_info(ui: &mut Ui, s: &State, path: &Path, r: Rect) {
    let t = ui.t;
    ui.c.fill_rect(r, t.browser_tabs);
    icons::draw(ui.c, Icon::FolderPlus, Rect::new(r.right() - 52.0, r.bottom() - 50.0, 44.0, 44.0), t.browser_tabs.shade(0.1));
    let md = std::fs::metadata(path).ok();
    let dir = md.as_ref().is_some_and(|m| m.is_dir());
    let size = if dir { list_dir(s, path).iter().filter(|e| !e.dir).map(|e| e.size).sum() } else { md.as_ref().map_or(0, |m| m.len()) };
    let date = md.and_then(|m| m.modified().ok()).map(date_text).unwrap_or_default();
    let f = os_ui::Font::new(13);
    let (lx, vx, lh) = (r.x + 5.0, r.x + 40.0, 15.0);
    let vw = r.right() - vx - 4.0;
    let mut y = r.y + 3.0;
    let title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    for (k, v) in [("Title", title), ("Date", date), ("Size", size_text(size))] {
        ui.c.text(f, lx, y, t.text_dim, k);
        ui.c.text_clip(f, vx, y, vw, t.text, &v);
        y += lh;
    }
    ui.c.text(f, lx, y, t.text_dim, "Path");
    // Wrap the path at any character (FL breaks mid-word too).
    let mut line = String::new();
    for ch in path.display().to_string().chars() {
        line.push(ch);
        if ui.c.text_width(f, &line) > vw {
            let last = line.pop().unwrap_or(' ');
            ui.c.text(f, vx, y, t.text, &line);
            y += lh;
            line = last.to_string();
            if y + lh > r.bottom() {
                return;
            }
        }
    }
    ui.c.text(f, vx, y, t.text, &line);
}

/// Top of the rows: below the search box when "Search box on top" is on (spec §4).
pub fn list_top(s: &State) -> f32 {
    if opt(s, OPT_TOP) { LIST_Y + SEARCH_ROW } else { LIST_Y }
}

/// Height of the search row above the list ("Search box on top").
const SEARCH_ROW: f32 = 27.0;

/// Largest useful scroll offset.
pub fn max_scroll(s: &State) -> f32 {
    (rows(s).len() as f32 * ROW_H - list_h(s)).max(0.0)
}

pub fn paint(ui: &mut Ui, s: &State, r: Rect) {
    let t = ui.t;
    ui.hit(r, Action::Swallow, "");
    // Title bar.
    ui.c.fill_rect(Rect::span(0.0, 200.0, 83.0, 105.0), t.browser_title);
    let menu_open = s.playlist.popup.as_ref().is_some_and(|p| p.x == MENU_AT.0 && p.y == MENU_AT.1);
    let mhov = ui.hit(Rect::new(2.0, 85.0, 16.0, 18.0), Action::BrowserMenu, "Browser options");
    if mhov || menu_open {
        ui.c.stroke_rrect(Rect::new(3.0, 86.0, 15.0, 16.0), 2.0, 1.2, t.browser_tab_outline);
    }
    icons::draw(ui.c, Icon::ArrowRight, Rect::new(5.0, 87.0, 12.0, 14.0), if mhov || menu_open { t.glyph } else { t.text_dim });
    icons::draw(ui.c, Icon::ArrowUp, Rect::new(19.0, 87.0, 12.0, 14.0), t.text_dim);
    ui.hit(Rect::new(17.0, 85.0, 16.0, 18.0), Action::BrowserCollapse, "Collapse all");
    // Back: not available yet, greyed out.
    ui.off(Rect::new(33.0, 85.0, 15.0, 18.0), "Back");
    icons::draw(ui.c, Icon::Back, Rect::new(34.0, 87.0, 12.0, 14.0), t.text_faint);
    ui.c.text(os_ui::Font::new(14), 52.0, 85.0, t.text_dim, "Browser");
    // Tabs (spec ui/browser.md §1).
    ui.c.fill_rect(Rect::span(0.0, 200.0, 106.0, 141.0), t.browser_tabs);
    const TIPS: [&str; 5] = ["All", "Current project", "Plugin database", "Online content", "Favorites"];
    for (i, icon) in [Icon::Wave, Icon::Page, Icon::Speaker, Icon::Globe, Icon::Star].into_iter().enumerate() {
        let cx = 20.0 + 40.0 * i as f32;
        let rr = Rect::new(cx - 18.0, 108.0, 36.0, 30.0);
        // Online content and favorites: not available yet, greyed out.
        let hov = if i >= OFF_TABS {
            ui.off(rr, TIPS[i]);
            false
        } else {
            ui.hit(rr, Action::BrowserTab(i), TIPS[i])
        };
        if i == s.browser_tab {
            ui.c.stroke_rrect(rr, 4.0, 1.5, t.browser_tab_outline);
        }
        let c = if i >= OFF_TABS { t.text_faint } else if hov || i == s.browser_tab { t.glyph } else { t.glyph.shade(-0.1) };
        icons::draw(ui.c, icon, Rect::new(cx - 9.0, 114.0, 18.0, 18.0), c);
    }
    // List.
    let top = list_top(s);
    if top > LIST_Y {
        ui.c.fill_rect(Rect::new(0.0, LIST_Y, 201.0, SEARCH_ROW), t.browser_footer);
        search_box(ui, s, Rect::new(13.0, LIST_Y + 4.0, 163.0, 20.0));
    }
    let rows = rows(s);
    let info = info_path(&rows, s);
    let ih = if info.is_some() { INFO_H } else { 0.0 };
    let list = Rect::span(13.0, 200.0, top, r.bottom() - FOOTER_H - ih);
    ui.c.fill_rect(Rect::new(0.0, top, 13.0, list.h), t.browser_tabs.shade(0.02));
    ui.c.fill_rect(list, t.browser_list);
    ui.hit(Rect::new(0.0, top, 201.0, list.h), Action::BrowserList, "");
    ui.c.push_clip(Rect::new(0.0, top, 201.0, list.h));
    let cats: [Color; 2] = [t.cat_project, t.cat_plugin_db];
    let scroll = s.browser_scroll.min((rows.len() as f32 * ROW_H - list.h).max(0.0)).max(0.0);
    let sel = s.browser_sel.as_deref();
    for (i, row) in rows.iter().enumerate() {
        let y = top + ROW_H * i as f32 - scroll;
        if y + ROW_H < list.y || y > list.bottom() {
            continue;
        }
        let x = 13.0 + INDENT * f32::from(row.depth);
        let rect = Rect::new(13.0, y, 188.0, ROW_H);
        // A parameter row's hint carries its value (spec: plugins/plugin-parameters.md §6); read for the hovered row only.
        let hint = match row.item {
            Item::Param(key, index) if ui.hovering(rect) => crate::params::row_hint(s, key, index),
            _ => row.label.clone(),
        };
        let hov = ui.hit(rect, Action::BrowserRow(i), hint);
        if sel == Some(row.key.as_str()) || hov {
            ui.c.fill_rect(rect, t.browser_row_hi);
        }
        let base = match row.cat {
            FOLDER => t.browser_folder,
            FILE => t.text.shade(-0.08),
            c => cats[usize::from(c).min(1)],
        };
        let c = if row.bright { base.shade(0.12) } else if row.depth > 0 { base.shade(-0.06) } else { base };
        // Expander: only an opened node shows its ▾ in the gutter (FL screenshots, spec §2).
        if row.open == Some(true) && s.browser_search.is_empty() {
            let (ax, cy) = (x - 9.0, y + ROW_H / 2.0);
            for k in 0..4 {
                let k = k as f32;
                ui.c.fill_rect(Rect::new(ax + k, cy - 2.0 + k, 7.0 - 2.0 * k, 1.0), t.text_dim);
            }
        }
        // File-type tints (spec §6): audio / project orange, MIDI blue.
        let ic = match row.icon {
            Icon::FileAudio | Icon::FileProject => t.browser_search,
            Icon::PianoRoll if row.cat == FILE => t.cat_plugin_db,
            _ => c,
        };
        icons::draw(ui.c, row.icon, Rect::new(x + 6.0, y + 3.0, 17.0, 17.0), ic);
        let (f, w) = (os_ui::Font::new(17), 200.0 - x - 27.0);
        let label = if row.path.is_some() { middle_cut(ui, f, &row.label, w) } else { row.label.clone() };
        ui.c.text_clip(f, x + 25.0, y, w, c, &label);
    }
    let empty = if !s.browser_search.is_empty() { "No matches" } else if s.browser_tab == 3 { "No online content" } else { "No favorites yet" };
    if rows.is_empty() && (s.browser_tab >= 3 || !s.browser_search.is_empty()) {
        ui.c.text(os_ui::Font::new(14).italic(), 22.0, top + 6.0, t.text_faint, empty);
    }
    ui.c.pop_clip();
    if let Some(p) = &info {
        paint_info(ui, s, p, Rect::new(0.0, list.bottom(), 201.0, ih));
    }
    // Footer.
    let fy = r.bottom() - 27.0;
    ui.c.fill_rect(Rect::new(0.0, fy, 201.0, 27.0), t.browser_footer);
    // Tags: not available yet, greyed out.
    let tags = Rect::new(13.0, fy + 4.0, 41.0, 20.0);
    ui.off(tags, "Tags");
    ui.c.fill_rrect(tags, 3.0, t.sunken);
    ui.c.text_center(os_ui::Font::condensed(11, 6).bold(), tags, ui.t.tx(0xc0c8cc), "TAGS");
    ui.veil(tags, t.browser_footer);
    if top == LIST_Y {
        search_box(ui, s, Rect::new(57.0, fy + 4.0, 119.0, 20.0));
    } else {
        ui.c.fill_rrect(Rect::new(57.0, fy + 4.0, 119.0, 20.0), 3.0, t.sunken);
    }
    icons::draw(ui.c, Icon::Folder, Rect::new(158.0, fy + 7.0, 14.0, 14.0), t.text_faint);
    let glass = Rect::new(178.0, fy + 3.0, 21.0, 21.0);
    let hov = ui.hit(glass, Action::BrowserSearch, "Search the browser (Enter = find next, Esc = clear)");
    icons::draw(ui.c, Icon::Search, Rect::new(180.0, fy + 5.0, 17.0, 17.0), if hov { t.browser_search.shade(0.2) } else { t.browser_search });
    // Splitter.
    ui.c.fill_rect(Rect::new(201.0, 83.0, 6.0, r.h), t.splitter);
    ui.c.fill_rect(Rect::new(207.0, 83.0, 2.0, r.h), t.splitter_dark);
}

/// A long disk name shortened in the middle with ".." to fit `w` (FL screenshots, spec §6).
fn middle_cut(ui: &mut Ui, f: os_ui::Font, text: &str, w: f32) -> String {
    if ui.c.text_width(f, text) <= w {
        return text.to_string();
    }
    let ch: Vec<char> = text.chars().collect();
    for keep in (1..ch.len()).rev() {
        let head = keep.div_ceil(2);
        let s: String = ch[..head].iter().chain([&'.', &'.']).chain(&ch[ch.len() - (keep - head)..]).collect();
        if ui.c.text_width(f, &s) <= w {
            return s;
        }
    }
    String::new()
}

/// Tabs from this one on (online content, favorites) are not available yet (spec §1).
const OFF_TABS: usize = 3;

/// Where the options menu opens (just below the title bar's ▸, spec §4).
pub const MENU_AT: (f32, f32) = (1.0, 105.0);

/// The search field (spec §5): typed text, a caret while focused; click focuses.
fn search_box(ui: &mut Ui, s: &State, r: Rect) {
    let t = ui.t;
    ui.hit(r, Action::BrowserSearch, "Search the browser (Enter = find next, Esc = clear)");
    ui.c.fill_rrect(r, 3.0, t.sunken);
    if s.browser_search_focus {
        ui.c.stroke_rrect(r, 3.0, 1.0, t.browser_search);
    }
    let f = os_ui::Font::new(14);
    let text_w = r.w - 26.0;
    // Show the end of a long query.
    let mut shown: &str = &s.browser_search;
    while ui.c.text_width(f, shown) > text_w && !shown.is_empty() {
        let cut = shown.char_indices().nth(1).map_or(shown.len(), |(i, _)| i);
        shown = &shown[cut..];
    }
    ui.c.text_clip(f, r.x + 5.0, r.y + 1.0, text_w, t.glyph, shown);
    if s.browser_search_focus {
        let cx = r.x + 6.0 + ui.c.text_width(f, shown);
        ui.c.fill_rect(Rect::new(cx, r.y + 4.0, 1.0, r.h - 8.0), t.glyph);
    }
}

/// Rect of the drag label at mouse position (x, y).
pub fn ghost_rect(x: f32, y: f32) -> Rect {
    Rect::new(x + 12.0, y + 6.0, 190.0, 22.0)
}

/// The label that follows the cursor during a browser drag (spec §3).
pub fn paint_ghost(ui: &mut Ui, s: &State) {
    let Some((label, x, y)) = &s.browser_drag else { return };
    let t = ui.t;
    let r = ghost_rect(*x, *y);
    let f = os_ui::Font::new(15);
    let w = (ui.c.text_width(f, label) + 14.0).min(r.w);
    let r = Rect::new(r.x, r.y, w, r.h);
    ui.c.fill_rrect(r, 3.0, t.browser_row_hi.shade(0.08));
    ui.c.stroke_rrect(r, 3.0, 1.0, t.browser_tab_outline);
    ui.c.text_clip(f, r.x + 7.0, r.y + 2.0, r.w - 10.0, t.glyph, label);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// File dates read in the local time zone on every platform (spec §6): within a day of the UTC date.
    #[test]
    fn dates_are_local_and_twelve_hour() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000); // 2023-11-14 22:13:20 UTC
        let text = date_text(t);
        assert!(text.contains("/2023 ") && (text.starts_with("11/14/") || text.starts_with("11/15/")), "{text}");
        assert!(text.ends_with(" AM") || text.ends_with(" PM"), "{text}");
        assert_eq!(size_text(1536), "1.50 KB");
    }
}
