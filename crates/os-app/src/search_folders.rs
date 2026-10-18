// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! File ▸ Browser extra search folders: the browser's dropped folders (`State::browser_folders`, the All tab's folder
//! roots) as a list of Folder and Browser name, in the order they were added, then empty rows.
//!
//! - A row's folder button opens the folder dialog: in an empty row the folder is added (as if dropped), in a filled
//!   one it replaces the row's folder.
//! - A click on a cell selects it for typing; Enter (or a click elsewhere) commits it, Esc drops it. A Browser name
//!   replaces the folder's own name as the root's label and the root is sorted by it, at once; an empty name gives the
//!   folder's name back. A Folder cell emptied removes the row and its root; a folder typed into it replaces the row's.
//! - The names are kept with the folder list (`browser_folders.txt`, `crate::browser`).
// spec: ui/settings-choices.md "Browser extra search folders, External tools, Script editor (2026-10-16)",
// ui/browser.md §6

use std::path::{Path, PathBuf};

use winit::keyboard::{Key, NamedKey};

use crate::app::{App, State};
use crate::dialog::Field;
use crate::views::browser as view;
use crate::views::settings;

/// Rows the list shows at least: the filled ones, then empty ones (about 19 fit).
pub const MIN_ROWS: usize = 19;

/// A list column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Folder,
    Name,
}

/// The cell being typed into: its row, its column and the text (the file dialog's text field: caret, selection).
#[derive(Debug, Clone, PartialEq)]
pub struct CellEdit {
    pub row: usize,
    pub col: Col,
    pub text: Field,
}

/// The rows to show: every folder and at least one empty row after them, `MIN_ROWS` at least.
pub fn shown_rows(s: &State) -> usize {
    (s.browser_folders.len() + 1).max(MIN_ROWS)
}

/// The filled rows: the folder as its cell shows it and its Browser name ("" when it has none).
pub fn rows(s: &State) -> Vec<(String, String)> {
    s.browser_folders.iter().map(|f| (f.display().to_string(), s.browser_names.get(f).cloned().unwrap_or_default())).collect()
}

/// Cell `col` of row `row` as stored.
fn cell(s: &State, row: usize, col: Col) -> Option<String> {
    let path = s.browser_folders.get(row)?;
    Some(match col {
        Col::Folder => path.display().to_string(),
        Col::Name => s.browser_names.get(path).cloned().unwrap_or_default(),
    })
}

/// Row `row`'s folder becomes `new` (its Browser name stays with the row); false when `new` is another row's folder.
fn replace(s: &mut State, row: usize, new: PathBuf) -> bool {
    let Some(old) = s.browser_folders.get(row).cloned() else { return false };
    if old == new {
        return true;
    }
    if s.browser_folders.contains(&new) {
        return false;
    }
    forget_open(s, &old);
    if let Some(name) = s.browser_names.remove(&old) {
        s.browser_names.insert(new.clone(), name);
    }
    s.browser_folders[row] = new;
    crate::browser::save_folders(s);
    true
}

/// Forgets which of `path`'s folders are open in the browser: its root and the folders under it, not a sibling
/// whose name starts the same.
fn forget_open(s: &mut State, path: &Path) {
    let key = view::disk_key(path);
    let under = format!("{key}/");
    s.browser_open.retain(|k| *k != key && !k.starts_with(&under));
}

/// Removes row `row`: its folder leaves the browser with its name.
fn remove(s: &mut State, row: usize) {
    if row >= s.browser_folders.len() {
        return;
    }
    let path = s.browser_folders.remove(row);
    s.browser_names.remove(&path);
    forget_open(s, &path);
    s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
    crate::browser::save_folders(s);
    s.message = Some(format!("Removed {} from the browser", path.display()));
}

/// Commits the cell being typed into: a Browser name relabels its root (empty: the folder's own name); an emptied
/// Folder cell removes the row, another folder replaces the row's. No cell: nothing.
pub(crate) fn commit(s: &mut State) {
    let Some(e) = s.settings.folder_edit.take() else { return };
    let Some(path) = s.browser_folders.get(e.row).cloned() else { return };
    let text = e.text.text.replace(['\t', '\r', '\n'], " ");
    let text = text.trim();
    match e.col {
        Col::Name => {
            if s.browser_names.get(&path).map_or("", String::as_str) == text {
                return;
            }
            if text.is_empty() {
                s.browser_names.remove(&path);
            } else {
                s.browser_names.insert(path.clone(), text.to_string());
            }
            crate::browser::save_folders(s);
            s.message = Some(format!("Browser name of {}: {}", path.display(), view::root_label(s, &path)));
        }
        Col::Folder if text.is_empty() => remove(s, e.row),
        Col::Folder => {
            let new = PathBuf::from(text);
            if new == path {
                return;
            }
            s.message = Some(if !new.is_dir() {
                format!("{text} is not a folder")
            } else if replace(s, e.row, new) {
                format!("Browser folder: {text}")
            } else {
                format!("{text} is in the list already")
            });
        }
    }
}

impl App {
    /// Row `row`'s folder button: the folder dialog; the folder chosen goes to `folder_chosen`.
    pub(crate) fn search_folder_pick(&mut self, row: usize) {
        use crate::filedlg::{FileDlg, Mode};
        commit(&mut self.state);
        let title = if row < self.state.browser_folders.len() { "Replace a browser folder" } else { "Add a browser folder" };
        let start = self.state.browser_folders.get(row).filter(|p| p.is_dir()).cloned();
        let dlg = FileDlg::new(Mode::Folder, title, "browser", crate::dialog::Then::BrowserFolder(row));
        self.file_dialog(dlg.start(start.as_deref()));
        self.invalidate();
    }

    /// The folder chosen for row `row`: an empty row adds it to the browser (as a dropped folder), a filled one has
    /// its folder replaced.
    pub(crate) fn search_folder_chosen(&mut self, row: usize, path: PathBuf) {
        let s = &mut self.state;
        if s.browser_folders.get(row) == Some(&path) {
            return;
        }
        if s.browser_folders.contains(&path) {
            s.message = Some(format!("{} is in the list already", path.display()));
        } else if row < s.browser_folders.len() {
            replace(s, row, path.clone());
            s.message = Some(format!("Browser folder: {}", path.display()));
        } else {
            self.add_browser_folder(path);
        }
        self.invalidate();
    }

    /// A click on cell `col` of row `row`: selected for typing (all of its text: typing replaces it). Another cell
    /// being typed into is committed first; empty rows have no cells.
    pub(crate) fn search_folder_cell(&mut self, row: usize, col: Col) {
        let s = &mut self.state;
        if s.settings.folder_edit.as_ref().is_some_and(|e| e.row == row && e.col == col) {
            return;
        }
        commit(s);
        let Some(now) = cell(s, row, col) else { return };
        let mut text = Field::default();
        text.set(&now);
        text.select_all();
        s.settings.folder_edit = Some(CellEdit { row, col, text });
        self.invalidate();
    }

    /// Keys while a cell is typed into. It takes every key, so none reaches the typing keyboard or a shortcut; false
    /// when no cell has it.
    pub(crate) fn folder_cell_key(&mut self, key: &Key, ctrl: bool, alt: bool) -> bool {
        let s = &mut self.state;
        let Some(e) = s.settings.folder_edit.as_mut() else { return false };
        // The list is on screen only while the File page is.
        if !(s.settings.open && s.settings.tab == settings::FILE) {
            commit(s);
            return false;
        }
        match key {
            Key::Named(NamedKey::Escape) => s.settings.folder_edit = None,
            Key::Named(NamedKey::Enter) => commit(s),
            _ => {
                e.text.key(key, ctrl, alt);
            }
        }
        self.invalidate();
        true
    }

    /// Whether a cell of the list is typed into (held keys repeat in it).
    pub fn folder_cell_editing(&self) -> bool {
        self.state.settings.folder_edit.is_some()
    }
}

/// The folder list as `browser_folders.txt` keeps it: a folder per line, in order, its Browser name after a tab when
/// it has one.
pub fn file_text(folders: &[PathBuf], names: &std::collections::BTreeMap<PathBuf, String>) -> String {
    folders
        .iter()
        .map(|f| match names.get(f).filter(|n| !n.is_empty()) {
            Some(n) => format!("{}\t{n}\r\n", f.display()),
            None => format!("{}\r\n", f.display()),
        })
        .collect()
}

/// `browser_folders.txt` read: the folders and their names. A line without a tab is a folder without a name (the files
/// written before names were kept).
pub fn parse_file(text: &str) -> Vec<(PathBuf, String)> {
    text.lines()
        .filter_map(|l| {
            let (path, name) = l.split_once('\t').unwrap_or((l, ""));
            let path = path.trim();
            (!path.is_empty()).then(|| (Path::new(path).to_path_buf(), name.trim().to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::SAct;
    use os_ui::{Canvas, Rect};

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn click(app: &mut App, c: &mut Canvas, a: Action) {
        let r = hit_rect(app, &a);
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        app.paint(c, None);
    }

    fn typing(app: &mut App, c: &mut Canvas, text: &str) {
        for ch in text.chars() {
            app.key(&Key::Character(ch.to_string().into()), false, false);
        }
        app.paint(c, None);
    }

    fn key(app: &mut App, c: &mut Canvas, k: NamedKey) {
        app.key(&Key::Named(k), false, false);
        app.paint(c, None);
    }

    /// Temporary folders `names` under a folder of the test's own.
    fn folders(test: &str, names: &[&str]) -> (PathBuf, Vec<PathBuf>) {
        let root = std::env::temp_dir().join(format!("os_search_folders_{test}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dirs: Vec<PathBuf> = names.iter().map(|n| root.join(n)).collect();
        for d in &dirs {
            std::fs::create_dir_all(d).unwrap();
        }
        (root, dirs)
    }

    /// The labels of the All tab's folder roots, top to bottom.
    fn roots(app: &App) -> Vec<String> {
        view::rows(&app.state).into_iter().filter(|r| r.depth == 0 && r.path.is_some()).map(|r| r.label).collect()
    }

    fn file_page(app: &mut App, c: &mut Canvas) {
        app.open_settings(settings::FILE);
        app.state.settings.folded.extend(["Backup", "Plugins", "User data folder"]);
        app.paint(c, None);
    }

    /// The list shows the dropped folders in the order they were added, then empty rows (19 rows at least); every row
    /// has its folder button, the filled ones their cells (spec).
    #[test]
    fn the_list_shows_the_dropped_folders() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (root, dirs) = folders("list", &["Zebra", "Apple"]);
        let mut app = App::new(None);
        app.drop_file(dirs[0].clone());
        app.drop_file(dirs[1].clone());
        assert_eq!(rows(&app.state), [(dirs[0].display().to_string(), String::new()), (dirs[1].display().to_string(), String::new())]);
        assert_eq!(shown_rows(&app.state), MIN_ROWS);
        file_page(&mut app, &mut c);
        for i in 0..MIN_ROWS {
            assert!(app.hits.iter().any(|h| h.action == Action::Settings(SAct::FolderPick(i))), "row {i} has its folder button");
        }
        for col in [Col::Folder, Col::Name] {
            assert!(app.hits.iter().any(|h| h.action == Action::Settings(SAct::FolderCell(1, col))));
            assert!(!app.hits.iter().any(|h| h.action == Action::Settings(SAct::FolderCell(2, col))), "an empty row has no cells");
        }
        // The group is not greyed out any more; External tools still is.
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder("Browser extra search folders")));
        assert!(app.hits.iter().any(|h| h.action == Action::Placeholder("External tools")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Removing a row forgets which of its folders were open in the browser, not those of a folder whose name starts
    /// the same.
    #[test]
    fn removing_a_row_keeps_a_sibling_open() {
        let (root, dirs) = folders("forget", &["Kit", "Kit 2"]);
        let mut app = App::new(None);
        app.drop_file(dirs[0].clone());
        app.drop_file(dirs[1].clone());
        let s = &mut app.state;
        let (one, two) = (view::disk_key(&dirs[0]), view::disk_key(&dirs[1]));
        s.browser_open.extend([one.clone(), format!("{one}/Snares"), two.clone(), format!("{two}/Snares")]);
        remove(s, 0);
        assert!(!s.browser_open.contains(&one) && !s.browser_open.contains(&format!("{one}/Snares")));
        assert!(s.browser_open.contains(&two) && s.browser_open.contains(&format!("{two}/Snares")), "the sibling stays open");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The folder button: in an empty row the folder chosen is added to the browser, in a filled row it replaces the
    /// row's folder (its name stays with the row); a folder another row has changes nothing.
    #[test]
    fn the_folder_button_adds_and_replaces() {
        use crate::dialog::fake;
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (root, dirs) = folders("pick", &["One", "Two", "Three"]);
        let mut app = App::new(None);
        file_page(&mut app, &mut c);
        fake::answer(Some(dirs[0].clone()));
        click(&mut app, &mut c, Action::Settings(SAct::FolderPick(3)));
        assert_eq!(app.state.browser_folders, [dirs[0].clone()], "added");
        assert_eq!(roots(&app), ["One"]);
        app.state.browser_names.insert(dirs[0].clone(), "Mine".into());
        fake::answer(Some(dirs[1].clone()));
        click(&mut app, &mut c, Action::Settings(SAct::FolderPick(0)));
        assert_eq!(app.state.browser_folders, [dirs[1].clone()], "replaced");
        assert_eq!(roots(&app), ["Mine"]);
        fake::answer(Some(dirs[2].clone()));
        click(&mut app, &mut c, Action::Settings(SAct::FolderPick(1)));
        fake::answer(Some(dirs[1].clone()));
        click(&mut app, &mut c, Action::Settings(SAct::FolderPick(1)));
        assert_eq!(app.state.browser_folders, [dirs[1].clone(), dirs[2].clone()], "a folder is listed once");
        // Cancelled: nothing changes.
        click(&mut app, &mut c, Action::Settings(SAct::FolderPick(0)));
        assert_eq!(app.state.browser_folders, [dirs[1].clone(), dirs[2].clone()]);
        assert_eq!(fake::shown().0, 5);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Browser name relabels its root and sorts it by the name, as soon as Enter commits it; clearing it gives the
    /// folder's name back; Esc drops what was typed (spec: "OSTest" moved its root).
    #[test]
    fn a_browser_name_relabels_and_sorts_its_root() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (root, dirs) = folders("name", &["Rendered", "Sliced audio", "Packs"]);
        let mut app = App::new(None);
        for d in &dirs {
            app.drop_file(d.clone());
        }
        assert_eq!(roots(&app), ["Packs", "Rendered", "Sliced audio"]);
        file_page(&mut app, &mut c);
        // Rendered (row 0) named "OSTest": it moves before Packs.
        click(&mut app, &mut c, Action::Settings(SAct::FolderCell(0, Col::Name)));
        typing(&mut app, &mut c, "OSTest");
        assert_eq!(roots(&app), ["Packs", "Rendered", "Sliced audio"], "not before Enter");
        key(&mut app, &mut c, NamedKey::Enter);
        assert_eq!(roots(&app), ["OSTest", "Packs", "Sliced audio"]);
        assert_eq!(rows(&app.state)[0].1, "OSTest");
        assert!(app.state.settings.folder_edit.is_none());
        // Esc drops a new name.
        click(&mut app, &mut c, Action::Settings(SAct::FolderCell(0, Col::Name)));
        typing(&mut app, &mut c, "Zzz");
        key(&mut app, &mut c, NamedKey::Escape);
        assert_eq!(roots(&app), ["OSTest", "Packs", "Sliced audio"]);
        // A click elsewhere commits.
        click(&mut app, &mut c, Action::Settings(SAct::FolderCell(2, Col::Name)));
        typing(&mut app, &mut c, "Aaa");
        click(&mut app, &mut c, Action::Settings(SAct::FolderCell(1, Col::Name)));
        assert_eq!(roots(&app), ["Aaa", "OSTest", "Sliced audio"]);
        key(&mut app, &mut c, NamedKey::Escape);
        // Clearing a name gives the folder's name back.
        for row in [0, 2] {
            click(&mut app, &mut c, Action::Settings(SAct::FolderCell(row, Col::Name)));
            key(&mut app, &mut c, NamedKey::Backspace);
            key(&mut app, &mut c, NamedKey::Enter);
        }
        assert_eq!(roots(&app), ["Packs", "Rendered", "Sliced audio"]);
        assert!(app.state.browser_names.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Emptying a Folder cell removes the row and its root (with its name); a folder typed into it replaces the row's,
    /// anything else changes nothing.
    #[test]
    fn emptying_a_folder_cell_removes_the_row() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let (root, dirs) = folders("remove", &["Keep", "Gone", "New"]);
        let mut app = App::new(None);
        app.drop_file(dirs[0].clone());
        app.drop_file(dirs[1].clone());
        app.state.browser_names.insert(dirs[1].clone(), "Named".into());
        file_page(&mut app, &mut c);
        click(&mut app, &mut c, Action::Settings(SAct::FolderCell(1, Col::Folder)));
        key(&mut app, &mut c, NamedKey::Delete);
        key(&mut app, &mut c, NamedKey::Enter);
        assert_eq!(app.state.browser_folders, [dirs[0].clone()]);
        assert!(app.state.browser_names.is_empty());
        assert_eq!(roots(&app), ["Keep"]);
        // Typing another folder's path replaces the row's; a path that is no folder does not.
        click(&mut app, &mut c, Action::Settings(SAct::FolderCell(0, Col::Folder)));
        typing(&mut app, &mut c, &root.join("Nowhere").display().to_string());
        key(&mut app, &mut c, NamedKey::Enter);
        assert_eq!(app.state.browser_folders, [dirs[0].clone()]);
        click(&mut app, &mut c, Action::Settings(SAct::FolderCell(0, Col::Folder)));
        typing(&mut app, &mut c, &dirs[2].display().to_string());
        key(&mut app, &mut c, NamedKey::Enter);
        assert_eq!(app.state.browser_folders, [dirs[2].clone()]);
        assert_eq!(roots(&app), ["New"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `browser_folders.txt`: the names after a tab; a file without names (one folder per line) still reads.
    #[test]
    fn the_folder_file_keeps_names_and_reads_old_files() {
        let old = "C:\\Samples\\Kits\r\n\r\n  D:\\Loops  \r\n";
        assert_eq!(parse_file(old), [(PathBuf::from("C:\\Samples\\Kits"), String::new()), (PathBuf::from("D:\\Loops"), String::new())]);
        let folders = vec![PathBuf::from("C:\\Samples\\Kits"), PathBuf::from("/home/me/Loops")];
        let names = std::collections::BTreeMap::from([(folders[1].clone(), "My loops".to_string())]);
        let text = file_text(&folders, &names);
        assert_eq!(text, "C:\\Samples\\Kits\r\n/home/me/Loops\tMy loops\r\n");
        assert_eq!(parse_file(&text), [(folders[0].clone(), String::new()), (folders[1].clone(), "My loops".to_string())]);
    }
}
