// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The browser's everyday file handling (spec: ui/browser.md §12): Sort files by Name / Type / Date with Group, Ctrl and
//! Shift clicks selecting several rows (dragged together), and project and MIDI files opened by a double click, Enter
//! or the row menu's Open.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::app::{App, State};
use crate::playlist::{Cmd, Mark, PItem};
use crate::views::browser::{self as view, Entry, Item, Row};

/// The rows selected besides `State::browser_sel`, the row a Shift click reaches from, and the last plain click (for a
/// double click).
#[derive(Debug, Clone, Default)]
pub struct Pick {
    pub more: Vec<String>,
    pub anchor: Option<String>,
    last: Option<(String, Instant)>,
    /// A plain press on a row of several selected: the row selected alone at the release unless it was dragged.
    collapse: Option<String>,
}

/// Two clicks on a row closer than this are a double click.
const DOUBLE: Duration = Duration::from_millis(400);

/// Whether row `key` is selected.
pub fn selected(s: &State, key: &str) -> bool {
    s.browser_sel.as_deref() == Some(key) || s.browser_pick.more.iter().any(|k| k == key)
}

/// The sort choices: Name, Type, Date (`browser.txt` keeps Type and Date as options, Name is neither).
pub fn sort_mode(s: &State) -> u8 {
    if view::opt(s, view::OPT_SORT_TYPE) {
        1
    } else if view::opt(s, view::OPT_SORT_DATE) {
        2
    } else {
        0
    }
}

/// Sort files by: folders before files (as the browser always listed them), then while Group is on the items an `.nfo`
/// file of the folder ranks ("SortGroup=N", the lower nearer the top) before the others, then by name, by type (the
/// extension, then the name) or by date (the file's creation, the oldest first, then the name).
pub fn sort(s: &State, v: &mut [Entry]) {
    let ranks = if view::opt(s, view::OPT_NOGROUP) { HashMap::new() } else { sort_groups(v) };
    let rank = |e: &Entry| {
        let name = e.name.to_lowercase();
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
        ranks.get(&name).or_else(|| ranks.get(stem)).copied().unwrap_or(u32::MAX)
    };
    let ext = |e: &Entry| {
        if e.dir { String::new() } else { e.name.rsplit_once('.').map_or(String::new(), |(_, x)| x.to_lowercase()) }
    };
    match sort_mode(s) {
        1 => v.sort_by_cached_key(|e| (!e.dir, rank(e), ext(e), e.name.to_lowercase())),
        2 => v.sort_by_cached_key(|e| (!e.dir, rank(e), e.created, e.name.to_lowercase())),
        _ => v.sort_by_cached_key(|e| (!e.dir, rank(e), e.name.to_lowercase())),
    }
}

/// The ranks the folder's `.nfo` files give: "<name>.nfo" holding "SortGroup=N" ranks the item called `<name>` (its
/// lower-case name, with or without its extension).
fn sort_groups(v: &[Entry]) -> HashMap<String, u32> {
    v.iter()
        .filter(|e| !e.dir && e.name.to_lowercase().ends_with(".nfo"))
        .filter_map(|e| {
            let text = std::fs::read_to_string(&e.path).ok()?;
            let n = text.lines().find_map(|l| {
                let (k, v) = l.split_once('=')?;
                k.trim().eq_ignore_ascii_case("SortGroup").then(|| v.trim().parse::<u32>().ok()).flatten()
            })?;
            Some((e.name[..e.name.len() - 4].to_lowercase(), n))
        })
        .collect()
}

/// The options menu's Sort files by ▸ (FL's tab menu has it): Name, Type, Date, then Group.
pub fn sort_menu(s: &State) -> PItem {
    use crate::browser::BCmd;
    let m = sort_mode(s);
    let radio = |label: &'static str, k: u8| PItem::new(label, Cmd::Browser(BCmd::Sort(k))).mark(Mark::Radio(m == k));
    PItem::sub(
        "Sort files by",
        vec![
            radio("Name", 0),
            radio("Type", 1),
            radio("Date", 2),
            PItem::sep(),
            PItem::new("Group", Cmd::Browser(BCmd::Sort(3))).mark(Mark::Check(!view::opt(s, view::OPT_NOGROUP))),
        ],
    )
}

/// Sort files by ▸ entry `k` chosen: the options it sets (3 turns Group over).
pub fn set_sort(s: &mut State, k: u8) {
    match k {
        3 => {
            if !s.browser_opts.remove(view::OPT_NOGROUP) {
                s.browser_opts.insert(view::OPT_NOGROUP);
            }
        }
        _ => {
            s.browser_opts.remove(view::OPT_SORT_TYPE);
            s.browser_opts.remove(view::OPT_SORT_DATE);
            match k {
                1 => s.browser_opts.insert(view::OPT_SORT_TYPE),
                2 => s.browser_opts.insert(view::OPT_SORT_DATE),
                _ => false,
            };
        }
    }
    // The folders are listed again in the new order.
    s.browser_dirs.borrow_mut().clear();
}

/// What a click on a row does to the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// A plain click: the row alone selected, acted on as before.
    Plain,
    /// The second click of a double click on the row.
    Double,
    /// Ctrl or Shift: the selection changed, nothing else done.
    Extend,
    /// A plain press on one of several selected rows: they stay selected for a drag.
    Keep,
}

/// A press on row `i` of `rows` with Ctrl or Shift held: Ctrl adds the row to the selection or takes it out; Shift
/// selects the rows from the last clicked one to it, the earlier Ctrl ones dropped.
pub fn click(s: &mut State, rows: &[Row], i: usize, ctrl: bool, shift: bool) -> Click {
    let Some(row) = rows.get(i) else { return Click::Plain };
    let key = row.key.clone();
    if shift {
        let from = s.browser_pick.anchor.as_ref().and_then(|a| rows.iter().position(|r| &r.key == a)).unwrap_or(i);
        let (a, b) = (from.min(i), from.max(i));
        s.browser_sel = Some(key.clone());
        s.browser_pick.more = rows[a..=b].iter().map(|r| r.key.clone()).filter(|k| *k != key).collect();
        return Click::Extend;
    }
    if ctrl {
        if selected(s, &key) {
            s.browser_pick.more.retain(|k| *k != key);
            if s.browser_sel.as_deref() == Some(key.as_str()) {
                s.browser_sel = s.browser_pick.more.pop();
            }
        } else {
            if let Some(k) = s.browser_sel.take() {
                s.browser_pick.more.push(k);
            }
            s.browser_sel = Some(key.clone());
        }
        s.browser_pick.anchor = Some(key);
        return Click::Extend;
    }
    if !s.browser_pick.more.is_empty() && selected(s, &key) && draggable(&row.item) {
        s.browser_pick.collapse = Some(key.clone());
        s.browser_pick.anchor = Some(key);
        return Click::Keep;
    }
    let now = Instant::now();
    let double = s.browser_pick.last.as_ref().is_some_and(|(k, t)| *k == key && now.duration_since(*t) < DOUBLE);
    s.browser_pick.last = if double { None } else { Some((key.clone(), now)) };
    s.browser_pick.more.clear();
    s.browser_pick.anchor = Some(key);
    if double { Click::Double } else { Click::Plain }
}

/// A row that can be dragged out of the browser.
pub fn draggable(item: &Item) -> bool {
    !matches!(item, Item::None | Item::File(_) | Item::History(_))
}

/// The release of a browser press: after a `Click::Keep` that was not dragged, the row is selected alone.
pub fn release(s: &mut State, dragged: bool) {
    if let Some(k) = s.browser_pick.collapse.take()
        && !dragged
    {
        crate::browser::select(s, &k);
    }
}

/// The other selected samples, in the list's order, when `path` is one of the selected rows: they go where it is
/// dropped too.
pub fn other_samples(s: &State, path: &str) -> Vec<String> {
    if s.browser_pick.more.is_empty() {
        return Vec::new();
    }
    let rows = view::rows(s);
    let sel: Vec<&Row> = rows.iter().filter(|r| selected(s, &r.key)).collect();
    if !sel.iter().any(|r| matches!(&r.item, Item::Sample(p) if p == path)) {
        return Vec::new();
    }
    sel.iter()
        .filter_map(|r| match &r.item {
            Item::Sample(p) if p != path => Some(p.clone()),
            _ => None,
        })
        .collect()
}

/// A file the browser opens: a project (or a zipped one), a MIDI file.
pub fn opens(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    crate::project_files::is_project(path) || matches!(ext.as_str(), "zip" | "mid" | "midi")
}

/// The file of `row` when the browser opens it.
pub fn openable(row: &Row) -> Option<PathBuf> {
    match &row.item {
        Item::File(p) => Some(PathBuf::from(p)).filter(|p| opens(p)),
        _ => None,
    }
}

impl App {
    /// Opens a browser file: a project as FILE ▸ Open does (the unsaved changes asked about first), a MIDI file as
    /// FILE ▸ Import ▸ MIDI file does.
    pub fn browser_open_file(&mut self, path: PathBuf) {
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if ext == "mid" || ext == "midi" {
            return self.midi_file_chosen(path, false);
        }
        self.ask_discard("opening another project", crate::dialog::Next::Open(path));
    }

    /// Enter over the browser: the selected row's file opens. False when it is not one the browser opens.
    pub fn browser_enter(&mut self) -> bool {
        let rows = view::rows(&self.state);
        let Some(row) = rows.iter().find(|r| Some(&r.key) == self.state.browser_sel.as_ref()) else { return false };
        let Some(path) = openable(row) else { return false };
        self.browser_open_file(path);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::Icon;

    fn row(key: &str, item: Item) -> Row {
        Row {
            key: key.into(),
            depth: 1,
            label: key.into(),
            icon: Icon::FileAudio,
            cat: 0,
            open: None,
            bright: false,
            item,
            path: None,
        }
    }

    fn entry(dir: &Path, name: &str, folder: bool, age: u64) -> Entry {
        let at = Some(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000 - age));
        Entry { name: name.into(), path: dir.join(name), dir: folder, size: 0, modified: at, created: at }
    }

    /// Name, Type and Date orders, folders first; while Group is on, the items an `.nfo` ranks lead.
    #[test]
    fn sorts_by_name_type_and_date() {
        let dir = std::env::temp_dir().join(format!("lemon-browser-sort-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("b.nfo"), "Title=Bass\nSortGroup=1\n").unwrap();
        let mut app = App::new(None);
        let s = &mut app.state;
        let names = |s: &State| {
            let mut v = vec![
                entry(&dir, "b.wav", false, 5),
                entry(&dir, "Kit", true, 1),
                entry(&dir, "a.mid", false, 9),
                entry(&dir, "b.nfo", false, 3),
                entry(&dir, "Drums", true, 2),
                entry(&dir, "c.flp", false, 0),
            ];
            sort(s, &mut v);
            v.iter().map(|e| e.name.clone()).collect::<Vec<_>>()
        };
        s.browser_opts.remove(view::OPT_NOGROUP);
        set_sort(s, 0);
        assert_eq!(names(s), ["Drums", "Kit", "b.nfo", "b.wav", "a.mid", "c.flp"]);
        set_sort(s, 1);
        assert_eq!(names(s), ["Drums", "Kit", "b.nfo", "b.wav", "c.flp", "a.mid"]);
        set_sort(s, 2);
        assert_eq!(names(s), ["Drums", "Kit", "b.wav", "b.nfo", "a.mid", "c.flp"], "the oldest first");
        set_sort(s, 3);
        set_sort(s, 0);
        assert_eq!(names(s), ["Drums", "Kit", "a.mid", "b.nfo", "b.wav", "c.flp"], "Group off");
        let menu = sort_menu(s);
        let marks: Vec<Mark> = menu.children.iter().map(|i| i.mark).collect();
        assert_eq!(marks, [Mark::Radio(true), Mark::Radio(false), Mark::Radio(false), Mark::None, Mark::Check(false)]);
        set_sort(s, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ctrl adds and takes out; Shift selects from the last clicked row, the earlier Ctrl ones dropped; a plain click
    /// selects the row alone, and a second one on it soon after is a double click.
    #[test]
    fn ctrl_and_shift_clicks_select_several() {
        let mut app = App::new(None);
        let s = &mut app.state;
        let rows: Vec<Row> = (0..6).map(|i| row(&format!("r{i}"), Item::Sample(format!("s{i}.wav")))).collect();
        s.browser_sel = Some("r1".into());
        assert_eq!(click(s, &rows, 1, false, false), Click::Plain);
        assert_eq!(click(s, &rows, 4, true, false), Click::Extend);
        let sel = |s: &State| (0..6).filter(|i| selected(s, &format!("r{i}"))).collect::<Vec<_>>();
        assert_eq!(sel(s), [1, 4]);
        click(s, &rows, 2, false, true);
        assert_eq!(sel(s), [2, 3, 4], "from the Ctrl-clicked row; row 1 dropped");
        click(s, &rows, 3, true, false);
        assert_eq!(sel(s), [2, 4], "Ctrl takes a selected row out");
        assert_eq!(click(s, &rows, 4, false, false), Click::Keep, "a press on the selection keeps it for a drag");
        assert_eq!(sel(s), [2, 4]);
        release(s, true);
        assert_eq!(sel(s), [2, 4], "dragged");
        click(s, &rows, 2, false, false);
        release(s, false);
        assert_eq!(sel(s), [2], "not dragged: the row alone");
        assert_eq!(click(s, &rows, 5, false, false), Click::Plain);
        assert_eq!(click(s, &rows, 5, false, false), Click::Double);
        assert_eq!(s.browser_pick.more, Vec::<String>::new());
    }

    /// Projects, zipped projects and MIDI files open; other files do not.
    #[test]
    fn which_files_open() {
        for (name, yes) in [("song.flp", true), ("song.zip", true), ("tune.mid", true), ("notes.txt", false)] {
            assert_eq!(openable(&row("k", Item::File(name.into()))).is_some(), yes, "{name}");
        }
        assert!(openable(&row("k", Item::Sample("a.wav".into()))).is_none());
    }
}
