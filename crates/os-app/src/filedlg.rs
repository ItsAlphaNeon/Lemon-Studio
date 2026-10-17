// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The file dialog's model: the folder shown and its listing, the places column, the name and path fields,
//! navigation, and what Enter answers. `views::dialog` paints it; `dialog` routes the input to it.
// spec: ui/dialogs.md §4

use std::cell::Cell;
use std::path::{Component, Path, PathBuf};
use std::time::{Instant, SystemTime};

use os_ui::Rect;

use crate::dialog::{Field, Then};

/// List row height and the header row's (logical px).
pub const ROW_H: f32 = 20.0;
pub const HEAD_H: f32 = 22.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Pick an existing file.
    Open,
    /// Choose a file name (a new file, or one to replace).
    Save,
    /// Pick a folder.
    Folder,
}

/// A file type the dialog lists: its name and extensions (none: every file).
#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub name: String,
    pub exts: Vec<String>,
}

impl Filter {
    pub fn new(name: &str, exts: &[&str]) -> Filter {
        Filter { name: name.into(), exts: exts.iter().map(|e| e.to_ascii_lowercase()).collect() }
    }

    /// The type combo's text: "FLP project (*.flp)".
    pub fn label(&self) -> String {
        if self.exts.is_empty() { self.name.clone() } else { format!("{} (*.{})", self.name, self.exts.join(", *.")) }
    }

    fn matches(&self, name: &str) -> bool {
        self.exts.is_empty() || Path::new(name).extension().is_some_and(|e| self.exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
    }
}

/// One row of the listing.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// The list's sort column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    /// The folder the dialog started in, when it is none of the others.
    Start,
    Drive,
}

/// A row of the places column.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub label: String,
    pub path: PathBuf,
    pub kind: PlaceKind,
}

/// Which field has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Name,
    Path,
}

/// What Enter (or the main button) did.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Stay,
    /// The answer.
    Done(PathBuf),
    /// Save onto an existing file: ask first.
    Replace(PathBuf),
}

pub struct FileDlg {
    pub mode: Mode,
    pub title: String,
    /// The last folder used is remembered per purpose for the session ("project", "audio", ...).
    purpose: &'static str,
    /// The caller's types, then "All files" (none in Folder mode).
    pub filters: Vec<Filter>,
    pub filter: usize,
    /// Where the dialog starts (the caller's folder), set before `opened`.
    start: Option<PathBuf>,
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    /// Why the folder could not be read.
    pub error: Option<String>,
    pub sel: Option<usize>,
    /// The list's selection answers Enter (a row was clicked or picked with the arrow keys since the last typing).
    pub list_focus: bool,
    /// List scroll, px.
    pub scroll: f32,
    /// The list's rows area at the last paint (page keys, scrolling into view, the scroll bar).
    pub view: Cell<Rect>,
    pub name: Field,
    pub path: Field,
    pub focus: Focus,
    pub sort: SortBy,
    pub reversed: bool,
    back: Vec<PathBuf>,
    pub places: Vec<Place>,
    /// What went wrong last (shown in the band instead of the counts).
    pub status: Option<String>,
    /// The type combo's list is open.
    pub filter_open: bool,
    /// Last row click (row, time), for double clicks.
    pub last_click: Option<(usize, Instant)>,
    pub then: Then,
}

/// Double-click interval.
const DOUBLE: std::time::Duration = std::time::Duration::from_millis(450);

static LAST: std::sync::Mutex<Vec<(&'static str, PathBuf)>> = std::sync::Mutex::new(Vec::new());

impl FileDlg {
    pub fn new(mode: Mode, title: &str, purpose: &'static str, then: Then) -> FileDlg {
        FileDlg {
            mode,
            title: title.into(),
            purpose,
            filters: Vec::new(),
            filter: 0,
            start: None,
            dir: PathBuf::new(),
            entries: Vec::new(),
            error: None,
            sel: None,
            list_focus: false,
            scroll: 0.0,
            view: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            name: Field::default(),
            path: Field::default(),
            focus: Focus::Name,
            sort: SortBy::Name,
            reversed: false,
            back: Vec::new(),
            places: Vec::new(),
            status: None,
            filter_open: false,
            last_click: None,
            then,
        }
    }

    /// Adds a file type (the first one is chosen).
    pub fn filter(mut self, name: &str, exts: &[&str]) -> FileDlg {
        self.filters.push(Filter::new(name, exts));
        self
    }

    /// The folder to start in (when it exists).
    pub fn start(mut self, dir: Option<&Path>) -> FileDlg {
        self.start = dir.filter(|d| d.is_dir()).map(Path::to_path_buf);
        self
    }

    /// The name the field starts with (Save: selected, so typing replaces it).
    pub fn name(mut self, name: &str) -> FileDlg {
        self.name.set(name);
        self.name.select_all();
        self
    }

    /// Ready to show: adds "All files", picks the start folder (the caller's, else the last one used for this purpose,
    /// else the home folder) and lists it.
    pub fn opened(mut self) -> FileDlg {
        if self.mode != Mode::Folder && !self.filters.is_empty() {
            self.filters.push(Filter::new("All files", &[]));
        }
        let last = LAST.lock().ok().and_then(|l| l.iter().find(|(p, _)| *p == self.purpose).map(|(_, d)| d.clone())).filter(|d| d.is_dir());
        let dir = self.start.clone().or(last).or_else(home).unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" }));
        self.places = places(self.start.as_deref());
        self.show(dir);
        self
    }

    /// The type chosen in the combo.
    pub fn chosen(&self) -> Option<&Filter> {
        self.filters.get(self.filter)
    }

    /// The answer is `p`: its folder is remembered for this purpose.
    pub fn remember(&self, p: &Path) {
        let dir = p.parent().unwrap_or(&self.dir);
        if let Ok(mut l) = LAST.lock() {
            l.retain(|(k, _)| *k != self.purpose);
            l.push((self.purpose, dir.to_path_buf()));
        }
    }

    // ------------------------------------------------------------ navigation

    /// Shows `dir` (kept in the Back history).
    pub fn go(&mut self, dir: PathBuf) {
        if dir != self.dir {
            self.back.push(self.dir.clone());
        }
        self.show(dir);
    }

    fn show(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.path.set(&self.dir.display().to_string());
        self.status = None;
        self.list();
    }

    pub fn can_go_up(&self) -> bool {
        self.dir.parent().is_some()
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// The parent folder; the folder left is selected in it.
    pub fn up(&mut self) {
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else { return };
        let left = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
        self.go(parent);
        if let Some(i) = left.and_then(|n| self.entries.iter().position(|e| e.dir && e.name == n)) {
            self.select(i);
        }
    }

    pub fn go_back(&mut self) {
        if let Some(d) = self.back.pop() {
            self.show(d);
        }
    }

    /// Lists the folder again (F5, a new folder, another type).
    pub fn reread(&mut self) {
        let keep = self.sel.and_then(|i| self.entries.get(i)).map(|e| e.name.clone());
        let scroll = self.scroll;
        self.list();
        self.scroll = scroll;
        self.clamp_scroll();
        if let Some(i) = keep.and_then(|n| self.entries.iter().position(|e| e.name == n)) {
            self.sel = Some(i);
        }
    }

    fn list(&mut self) {
        self.entries.clear();
        self.error = None;
        self.sel = None;
        self.scroll = 0.0;
        self.last_click = None;
        let filter = self.chosen().cloned();
        match std::fs::read_dir(&self.dir) {
            Err(e) => self.error = Some(e.to_string()),
            Ok(rd) => {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if hidden(&name, &e) {
                        continue;
                    }
                    // Links are followed (a link to a folder opens like the folder).
                    let Ok(md) = std::fs::metadata(e.path()) else { continue };
                    let dir = md.is_dir();
                    if !dir && (self.mode == Mode::Folder || filter.as_ref().is_some_and(|f| !f.matches(&name))) {
                        continue;
                    }
                    self.entries.push(Entry { name, dir, size: if dir { 0 } else { md.len() }, modified: md.modified().ok() });
                }
            }
        }
        self.sort_entries();
    }

    /// Sorts by `by`; the same column again reverses the order. Folders stay first.
    pub fn sort_by(&mut self, by: SortBy) {
        if self.sort == by {
            self.reversed = !self.reversed;
        } else {
            self.sort = by;
            self.reversed = false;
        }
        let keep = self.sel.and_then(|i| self.entries.get(i)).map(|e| e.name.clone());
        self.sort_entries();
        self.sel = keep.and_then(|n| self.entries.iter().position(|e| e.name == n));
    }

    fn sort_entries(&mut self) {
        let (by, rev) = (self.sort, self.reversed);
        self.entries.sort_by(|a, b| {
            let name = || a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name));
            let o = match by {
                SortBy::Name => name(),
                SortBy::Size => a.size.cmp(&b.size).then_with(name),
                SortBy::Modified => a.modified.cmp(&b.modified).then_with(name),
            };
            b.dir.cmp(&a.dir).then(if rev { o.reverse() } else { o })
        });
    }

    /// Another type in the combo: the list follows; a typed name takes the type's extension.
    pub fn set_filter(&mut self, i: usize) {
        if i >= self.filters.len() {
            return;
        }
        self.filter = i;
        self.filter_open = false;
        if self.mode == Mode::Save {
            if let Some(ext) = self.filters[i].exts.first().cloned() {
                let n = self.name.text.trim().to_string();
                let stem = Path::new(&n).file_stem().map(|s| s.to_string_lossy().into_owned());
                if let Some(stem) = stem.filter(|_| Path::new(&n).extension().is_some()) {
                    self.name.set(&format!("{stem}.{ext}"));
                }
            }
        }
        self.reread();
    }

    // ------------------------------------------------------------ selection

    /// Selects row `i` and scrolls it into view: a file's name goes into the name field (a folder's in Folder mode).
    pub fn select(&mut self, i: usize) {
        let Some(e) = self.entries.get(i) else { return };
        self.sel = Some(i);
        if e.dir == (self.mode == Mode::Folder) {
            let n = e.name.clone();
            self.name.set(&n);
        }
        self.list_focus = true;
        self.into_view(i);
    }

    /// Moves the selection by `d` rows (clamped).
    pub fn step(&mut self, d: i64) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as i64 - 1;
        let i = match self.sel {
            Some(i) => (i as i64 + d).clamp(0, last),
            None if d < 0 => last,
            None => 0,
        };
        self.select(i as usize);
    }

    /// Rows the list shows.
    pub fn page(&self) -> i64 {
        ((self.view.get().h / ROW_H).floor() as i64).max(1)
    }

    fn into_view(&mut self, i: usize) {
        let h = self.view.get().h;
        let (top, bottom) = (i as f32 * ROW_H, (i + 1) as f32 * ROW_H);
        if top < self.scroll {
            self.scroll = top;
        } else if h > 0.0 && bottom > self.scroll + h {
            self.scroll = bottom - h;
        }
        self.clamp_scroll();
    }

    pub fn max_scroll(&self) -> f32 {
        (self.entries.len() as f32 * ROW_H - self.view.get().h).max(0.0)
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    /// A click on row `i`: selects it; the second click of a double click opens a folder or chooses a file.
    pub fn click_row(&mut self, i: usize) -> Outcome {
        let double = self.last_click.is_some_and(|(r, t)| r == i && t.elapsed() < DOUBLE);
        self.last_click = if double { None } else { Some((i, Instant::now())) };
        self.select(i);
        if double { self.activate(i) } else { Outcome::Stay }
    }

    /// Double click / Enter on row `i`.
    fn activate(&mut self, i: usize) -> Outcome {
        let Some(e) = self.entries.get(i).cloned() else { return Outcome::Stay };
        if e.dir {
            self.go(self.dir.join(&e.name));
            return Outcome::Stay;
        }
        self.name.set(&e.name);
        self.list_focus = false;
        self.accept()
    }

    // ------------------------------------------------------------ answering

    /// Enter in the path field: go to the folder typed.
    pub fn path_enter(&mut self) {
        let p = clean(&self.dir.join(expand(self.path.text.trim())));
        if p.is_dir() {
            self.go(p);
            self.focus = Focus::Name;
        } else {
            self.status = Some(format!("No folder {}", p.display()));
        }
    }

    /// Enter or the main button.
    pub fn accept(&mut self) -> Outcome {
        self.status = None;
        if self.list_focus {
            if let Some(e) = self.sel.and_then(|i| self.entries.get(i)).filter(|e| e.dir).cloned() {
                if self.mode != Mode::Folder {
                    self.go(self.dir.join(e.name));
                    return Outcome::Stay;
                }
            }
        }
        let typed = self.name.text.trim().to_string();
        if self.mode == Mode::Folder {
            if typed.is_empty() {
                return Outcome::Done(self.dir.clone());
            }
            let p = clean(&self.dir.join(expand(&typed)));
            return if p.is_dir() {
                Outcome::Done(p)
            } else {
                self.status = Some(format!("No folder {}", p.display()));
                Outcome::Stay
            };
        }
        if typed.is_empty() {
            return Outcome::Stay;
        }
        let mut p = clean(&self.dir.join(expand(&typed)));
        // A folder typed (or "..") is opened.
        if p.is_dir() {
            self.go(p);
            self.name.set("");
            return Outcome::Stay;
        }
        match self.mode {
            Mode::Open => {
                if p.is_file() {
                    Outcome::Done(p)
                } else {
                    self.status = Some(format!("{typed} was not found"));
                    Outcome::Stay
                }
            }
            _ => {
                // A name without the chosen type's extension gets it.
                if let Some(f) = self.chosen().filter(|f| !f.exts.is_empty()) {
                    let fits = p.extension().is_some_and(|e| f.exts.iter().any(|x| e.eq_ignore_ascii_case(x)));
                    if !fits {
                        let n = p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                        p.set_file_name(format!("{n}.{}", f.exts[0]));
                    }
                }
                if !p.parent().is_some_and(Path::is_dir) {
                    self.status = Some(format!("No folder {}", p.parent().unwrap_or(&p).display()));
                    return Outcome::Stay;
                }
                if p.is_dir() {
                    self.go(p);
                    return Outcome::Stay;
                }
                if p.exists() { Outcome::Replace(p) } else { Outcome::Done(p) }
            }
        }
    }

    /// New folder: "New folder", "New folder 2", ... in the folder shown, then selected.
    pub fn new_folder(&mut self) {
        let Some(name) = (1..1000).map(|k| if k == 1 { "New folder".to_string() } else { format!("New folder {k}") }).find(|n| !self.dir.join(n).exists()) else { return };
        match std::fs::create_dir(self.dir.join(&name)) {
            Ok(()) => {
                self.reread();
                if let Some(i) = self.entries.iter().position(|e| e.name == name) {
                    self.select(i);
                }
                self.status = Some(format!("Made {name}"));
            }
            Err(e) => self.status = Some(format!("Could not make a folder: {e}")),
        }
    }

    /// A file dropped on the window: a folder opens, a file's folder opens with the file selected.
    pub fn dropped(&mut self, p: &Path) {
        if p.is_dir() {
            self.go(p.to_path_buf());
        } else if let Some(d) = p.parent().filter(|d| d.is_dir()) {
            self.go(d.to_path_buf());
            let n = p.file_name().map(|n| n.to_string_lossy().into_owned());
            match n.as_ref().and_then(|n| self.entries.iter().position(|e| &e.name == n)) {
                Some(i) => self.select(i),
                None if self.mode != Mode::Folder => self.name.set(n.as_deref().unwrap_or("")),
                None => {}
            }
            self.list_focus = false;
        }
    }

    /// The band's text: what went wrong, else the counts.
    pub fn status_text(&self) -> String {
        if let Some(s) = &self.status {
            return s.clone();
        }
        let dirs = self.entries.iter().filter(|e| e.dir).count();
        let files = self.entries.len() - dirs;
        let n = |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
        if self.mode == Mode::Folder { n(dirs, "folder", "folders") } else { format!("{}, {}", n(dirs, "folder", "folders"), n(files, "file", "files")) }
    }

    /// The place that holds the folder shown (the deepest one).
    pub fn place_now(&self) -> Option<usize> {
        self.places.iter().enumerate().filter(|(_, p)| self.dir.starts_with(&p.path)).max_by_key(|(_, p)| p.path.components().count()).map(|(i, _)| i)
    }
}

/// A file size as the list shows it.
pub fn size_text(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.0} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / (K * K))
    } else {
        format!("{:.1} GB", f / (K * K * K))
    }
}

/// A modification time as the list shows it (local time).
pub fn time_text(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string()
}

/// `p` with "." and ".." folded away (no file system access, so links are not resolved).
pub fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// "~" at the start of a typed path is the home folder.
fn expand(typed: &str) -> PathBuf {
    match typed.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => home().map_or_else(|| PathBuf::from(typed), |h| h.join(rest.trim_start_matches(['/', '\\']))),
        _ => PathBuf::from(typed),
    }
}

/// Entries the list leaves out: names starting with a dot, and on Windows hidden or system files.
fn hidden(name: &str, _e: &std::fs::DirEntry) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const HIDDEN: u32 = 0x2;
        const SYSTEM: u32 = 0x4;
        if _e.metadata().is_ok_and(|m| m.file_attributes() & (HIDDEN | SYSTEM) != 0) {
            return true;
        }
    }
    false
}

/// The user's home folder.
pub fn home() -> Option<PathBuf> {
    let v = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    Some(PathBuf::from(v)).filter(|p| p.is_dir())
}

/// The places column: the user's folders, the start folder, the drives / mounted volumes.
fn places(start: Option<&Path>) -> Vec<Place> {
    let mut v = Vec::new();
    let home = home();
    if let Some(h) = &home {
        v.push(Place { label: "Home".into(), path: h.clone(), kind: PlaceKind::Home });
    }
    for (kind, label, path) in user_dirs(home.as_deref()) {
        if path.is_dir() && !v.iter().any(|p: &Place| p.path == path) {
            v.push(Place { label: label.into(), path, kind });
        }
    }
    if let Some(s) = start.filter(|s| !v.iter().any(|p| p.path == *s)) {
        let label = s.file_name().map_or_else(|| s.display().to_string(), |n| n.to_string_lossy().into_owned());
        v.push(Place { label, path: s.to_path_buf(), kind: PlaceKind::Start });
    }
    for (label, path) in drives() {
        v.push(Place { label, path, kind: PlaceKind::Drive });
    }
    v
}

/// Desktop, Documents, Downloads and Music.
#[cfg(windows)]
fn user_dirs(_home: Option<&Path>) -> Vec<(PlaceKind, &'static str, PathBuf)> {
    use windows_sys::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music};
    [(PlaceKind::Desktop, "Desktop", FOLDERID_Desktop), (PlaceKind::Documents, "Documents", FOLDERID_Documents), (PlaceKind::Downloads, "Downloads", FOLDERID_Downloads), (PlaceKind::Music, "Music", FOLDERID_Music)]
        .into_iter()
        .filter_map(|(k, l, id)| known_folder(&id).map(|p| (k, l, p)))
        .collect()
}

/// A Windows known folder (it may be moved, e.g. into OneDrive).
#[cfg(windows)]
fn known_folder(id: &windows_sys::core::GUID) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::SHGetKnownFolderPath;
    let mut out: *mut u16 = std::ptr::null_mut();
    // SAFETY: `out` receives a string the shell allocates; it is read up to its terminating zero and then freed with
    // CoTaskMemFree, as the function's contract asks (also on failure).
    unsafe {
        let hr = SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut out);
        let path = (hr >= 0 && !out.is_null()).then(|| {
            let len = (0..).take_while(|&i| *out.add(i) != 0).count();
            PathBuf::from(std::ffi::OsString::from_wide(std::slice::from_raw_parts(out, len)))
        });
        CoTaskMemFree(out as *const _);
        path
    }
}

/// Desktop, Documents, Downloads and Music: XDG's user folders (they may be named in the user's language).
#[cfg(all(unix, not(target_os = "macos")))]
fn user_dirs(home: Option<&Path>) -> Vec<(PlaceKind, &'static str, PathBuf)> {
    let Some(home) = home else { return Vec::new() };
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(cfg.join("user-dirs.dirs")).unwrap_or_default();
    let named = |key: &str| {
        text.lines().find_map(|l| {
            let v = l.trim().strip_prefix(key)?.trim_start().strip_prefix('=')?.trim().trim_matches('"');
            Some(match v.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => PathBuf::from(v),
            })
        })
    };
    [(PlaceKind::Desktop, "Desktop", "XDG_DESKTOP_DIR"), (PlaceKind::Documents, "Documents", "XDG_DOCUMENTS_DIR"), (PlaceKind::Downloads, "Downloads", "XDG_DOWNLOAD_DIR"), (PlaceKind::Music, "Music", "XDG_MUSIC_DIR")]
        .into_iter()
        .map(|(k, l, key)| (k, l, named(key).filter(|p| p != home).unwrap_or_else(|| home.join(l))))
        .collect()
}

#[cfg(target_os = "macos")]
fn user_dirs(home: Option<&Path>) -> Vec<(PlaceKind, &'static str, PathBuf)> {
    let Some(home) = home else { return Vec::new() };
    [(PlaceKind::Desktop, "Desktop"), (PlaceKind::Documents, "Documents"), (PlaceKind::Downloads, "Downloads"), (PlaceKind::Music, "Music")].into_iter().map(|(k, l)| (k, l, home.join(l))).collect()
}

/// Windows: every drive letter in use.
#[cfg(windows)]
fn drives() -> Vec<(String, PathBuf)> {
    (b'A'..=b'Z').map(|c| PathBuf::from(format!("{}:\\", c as char))).filter(|p| p.is_dir()).map(|p| (p.display().to_string(), p)).collect()
}

/// Linux: the file system and what is mounted under /media/<user>, /run/media/<user> and /mnt.
#[cfg(all(unix, not(target_os = "macos")))]
fn drives() -> Vec<(String, PathBuf)> {
    let mut v = vec![("File system".to_string(), PathBuf::from("/"))];
    let user = std::env::var("USER").unwrap_or_default();
    for root in [PathBuf::from("/media").join(&user), PathBuf::from("/run/media").join(&user), PathBuf::from("/mnt")] {
        v.extend(sub_dirs(&root));
    }
    v
}

/// macOS: the start-up disk and the volumes.
#[cfg(target_os = "macos")]
fn drives() -> Vec<(String, PathBuf)> {
    let mut v = vec![("Computer".to_string(), PathBuf::from("/"))];
    v.extend(sub_dirs(Path::new("/Volumes")).into_iter().filter(|(_, p)| std::fs::canonicalize(p).ok().as_deref() != Some(Path::new("/"))));
    v
}

#[cfg(unix)]
fn sub_dirs(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(root) else { return Vec::new() };
    let mut v: Vec<_> = rd.flatten().filter(|e| e.path().is_dir()).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).filter(|(n, _)| !n.starts_with('.')).collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("os-filedlg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lists_folders_first_and_filters_types() {
        let d = tmp("list");
        std::fs::create_dir(d.join("beats")).unwrap();
        std::fs::create_dir(d.join(".hidden")).unwrap();
        std::fs::write(d.join("b.flp"), b"x").unwrap();
        std::fs::write(d.join("A.flp"), b"xyz").unwrap();
        std::fs::write(d.join("notes.txt"), b"").unwrap();
        let f = FileDlg::new(Mode::Open, "Open", "test-list", Then::Nothing).filter("FLP project", &["flp"]).start(Some(&d)).opened();
        let names: Vec<_> = f.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["beats", "A.flp", "b.flp"]);
        assert_eq!(f.filters.last().unwrap().name, "All files");
        assert_eq!(f.status_text(), "1 folder, 2 files");
        let mut f = f;
        f.set_filter(1);
        assert_eq!(f.entries.len(), 4);
        f.sort_by(SortBy::Name);
        assert_eq!(f.entries[0].name, "beats", "folders stay first when reversed");
        assert_eq!(f.entries[1].name, "notes.txt");
        let folders = FileDlg::new(Mode::Folder, "Folder", "test-list", Then::Nothing).start(Some(&d)).opened();
        assert_eq!(folders.entries.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn save_adds_the_extension_and_asks_before_replacing() {
        let d = tmp("save");
        std::fs::write(d.join("song.flp"), b"x").unwrap();
        std::fs::create_dir(d.join("sub")).unwrap();
        let mut f = FileDlg::new(Mode::Save, "Save", "test-save", Then::Nothing).filter("FLP project", &["flp"]).start(Some(&d)).name("Untitled.flp").opened();
        assert!(f.name.all, "the name starts selected");
        f.name.insert("tune");
        assert_eq!(f.name.text, "tune");
        assert_eq!(f.accept(), Outcome::Done(d.join("tune.flp")));
        f.name.set("song");
        assert_eq!(f.accept(), Outcome::Replace(d.join("song.flp")));
        // A typed folder opens; ".." goes back up.
        f.name.set("sub");
        assert_eq!(f.accept(), Outcome::Stay);
        assert_eq!(f.dir, d.join("sub"));
        assert_eq!(f.name.text, "");
        f.name.set("..");
        assert_eq!(f.accept(), Outcome::Stay);
        assert_eq!(f.dir, clean(&d));
        // A missing folder is refused in the status.
        f.name.set("nowhere/x");
        assert_eq!(f.accept(), Outcome::Stay);
        assert!(f.status_text().starts_with("No folder"), "{}", f.status_text());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rows_select_and_double_clicks_open() {
        let d = tmp("rows");
        std::fs::create_dir(d.join("kit")).unwrap();
        std::fs::write(d.join("kit").join("kick.wav"), b"x").unwrap();
        std::fs::write(d.join("a.wav"), b"x").unwrap();
        let mut f = FileDlg::new(Mode::Open, "Open", "test-rows", Then::Nothing).filter("Audio", &["wav"]).start(Some(&d)).opened();
        assert_eq!(f.click_row(1), Outcome::Stay);
        assert_eq!(f.name.text, "a.wav", "a file's name fills the field");
        assert_eq!(f.click_row(0), Outcome::Stay);
        assert_eq!(f.name.text, "a.wav", "a folder's name does not");
        assert_eq!(f.accept(), Outcome::Stay, "Enter with a folder selected opens it");
        assert_eq!(f.dir, d.join("kit"));
        assert!(f.can_go_back());
        f.click_row(0);
        assert_eq!(f.click_row(0), Outcome::Done(d.join("kit").join("kick.wav")));
        f.up();
        assert_eq!(f.dir, d);
        assert_eq!(f.sel.map(|i| f.entries[i].name.as_str()), Some("kit"), "the folder left is selected");
        let _ = std::fs::remove_dir_all(&d);
    }
}
