// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! FILE ▸ Recent projects: the project files last opened or saved, newest first, kept between runs in the program's
//! settings (`recent_project_1` .. `recent_project_50`, `crate::settings_file`). FILE lists the ten newest under a
//! "Recent projects" heading, Alt+1 .. Alt+9 and Alt+0 opening them wherever the main window has the keyboard; More ▸
//! lists the older ones (`views::menus` draws them). Opening one asks about unsaved changes first, as Open does; one
//! whose file has gone is said and taken off the list. A backup copy written and a project started from a template
//! leave the list as it was (`App::save_copy`, `App::new_from_template`).
// spec: ui/recent-projects.md

use std::path::{Path, PathBuf};

use crate::app::{App, State};
use crate::playlist::{Cmd, PItem};

/// How many are kept, as FL keeps (spec §1).
pub const KEEP: usize = 50;
/// How many FILE lists, each with its key; More ▸ has the others.
pub const SHOWN: usize = 10;
/// The keys of FILE's rows as the menu writes them: Alt+1 .. Alt+9, then Alt+0.
pub const KEYS: [&str; SHOWN] = ["Alt+1", "Alt+2", "Alt+3", "Alt+4", "Alt+5", "Alt+6", "Alt+7", "Alt+8", "Alt+9", "Alt+0"];
/// FILE's heading over the rows.
pub const HEADING: &str = "Recent projects";
/// The ▸ row with the older ones.
pub const MORE: &str = "More";

/// The settings key of entry `n`.
fn key(n: usize) -> String {
    format!("recent_project_{}", n + 1)
}

/// The projects, newest first.
pub fn list() -> Vec<PathBuf> {
    (0..KEEP).map_while(|n| crate::settings_file::get(&key(n))).map(PathBuf::from).collect()
}

/// Keeps `paths` as the list: one write of the settings file.
pub(crate) fn restore(paths: &[PathBuf]) {
    let pairs: Vec<(String, String)> = (0..KEEP).map(|n| (key(n), paths.get(n).map_or_else(String::new, |p| p.to_string_lossy().into_owned()))).collect();
    crate::settings_file::write_all(&pairs);
}

/// Whether `a` and `b` name the same file: on Windows letter case aside, as its file names are.
fn same(a: &Path, b: &Path) -> bool {
    if cfg!(windows) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b }
}

/// `path` was opened or saved: it heads the list, once; the oldest past fifty drop off.
pub(crate) fn add(path: &Path) {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut paths = list();
    paths.retain(|p| !same(p, &path));
    paths.insert(0, path);
    paths.truncate(KEEP);
    restore(&paths);
}

/// Takes `path` off the list.
fn remove(path: &Path) {
    let mut paths = list();
    paths.retain(|p| !same(p, path));
    restore(&paths);
}

/// What a row shows: the file's name with its extension, as FL's rows do.
pub fn label(path: &Path) -> String {
    path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}

/// More ▸: the projects after the tenth, by name, without keys.
pub(crate) fn more_items(_: &State) -> Vec<PItem> {
    list().iter().enumerate().skip(SHOWN).map(|(n, p)| PItem::new(label(p), Cmd::OpenRecent(n))).collect()
}

/// The row Alt+`c` opens: 1 .. 9 the first nine, 0 the tenth.
pub fn alt_digit(c: &str) -> Option<usize> {
    let mut chars = c.chars();
    let d = chars.next()?.to_digit(10)?;
    if chars.next().is_some() {
        return None;
    }
    Some(if d == 0 { 9 } else { d as usize - 1 })
}

impl App {
    /// Recent project `n` (FILE's row, its Alt key, More ▸): opened once unsaved changes are dealt with, as Open does. A
    /// file that has gone is said and taken off the list.
    pub(crate) fn open_recent(&mut self, n: usize) {
        let Some(path) = list().into_iter().nth(n) else { return };
        if !path.is_file() {
            remove(&path);
            let text = crate::dialog::spans(&["Lemon Studio cannot find ", &path.to_string_lossy(), ".\n\nIt was taken off the recent projects."]);
            return self.tell(crate::dialog::Kind::Error, "Project not found", text);
        }
        self.ask_discard("opening another project", crate::dialog::Next::Open(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::menus::RECENT_PICK;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lemon-recent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Newest first, each file once (a file opened again moves to the top), fifty at most; the keys as the menu writes
    /// them.
    #[test]
    fn newest_first_once_fifty_at_most() {
        assert!(list().is_empty(), "a test's settings start empty");
        let p = |n: usize| std::env::temp_dir().join(format!("song {n}.flp"));
        for n in 0..52 {
            add(&p(n));
        }
        let l = list();
        assert_eq!((l.len(), l[0].clone(), l[49].clone()), (KEEP, p(51), p(2)));
        add(&p(10));
        assert_eq!(list()[..2], [p(10), p(51)]);
        assert_eq!(list().len(), KEEP, "moved, not added");
        if cfg!(windows) {
            add(&PathBuf::from(p(51).to_string_lossy().to_uppercase()));
            assert_eq!(list().len(), KEEP, "Windows file names ignore letter case");
        }
        remove(&list()[0].clone());
        assert_eq!(list().len(), KEEP - 1);
        assert_eq!(label(&p(3)), "song 3.flp");
        assert_eq!((alt_digit("1"), alt_digit("9"), alt_digit("0"), alt_digit("a"), alt_digit("12")), (Some(0), Some(8), Some(9), None, None));
        assert_eq!(KEYS.map(|k| alt_digit(&k[4..]).unwrap()), std::array::from_fn(|i| i));
    }

    /// Saving and opening put the file on top; a backup copy does not. The rows open their project, so do Alt+1 ..
    /// Alt+0; More ▸ has the older ones; a file that has gone is said and dropped.
    #[test]
    fn opened_and_saved_projects_and_their_rows() {
        let d = dir("rows");
        let mut app = App::new(None);
        let files: Vec<PathBuf> = (0..12).map(|n| d.join(format!("p{n}.flp"))).collect();
        for f in &files {
            app.save_to(f.clone());
        }
        assert_eq!(list().len(), 12);
        assert_eq!(list()[0], std::path::absolute(&files[11]).unwrap());
        // A backup copy is not a project the user saved.
        assert_eq!(app.save_copy(d.join("copy.flp")), Some(Ok(())));
        assert_eq!(list()[0], std::path::absolute(&files[11]).unwrap());
        // The second row opens p10.
        app.menu_item(0, RECENT_PICK + 1);
        assert_eq!(app.state.path.as_deref(), Some(files[10].as_path()));
        assert_eq!(label(&list()[0]), "p10.flp", "opened: on top");
        // Alt+3 opens the third: p9 now (p10, p11, p9 ...).
        app.key(&winit::keyboard::Key::Character("3".into()), false, true);
        assert_eq!(app.state.path.as_deref().map(|p| label(p)).as_deref(), Some("p9.flp"));
        // More ▸ lists the two after the tenth.
        let more: Vec<String> = more_items(&app.state).iter().map(|i| i.label.to_string()).collect();
        assert_eq!(more, ["p1.flp", "p0.flp"]);
        crate::playlist::run(&mut app, Cmd::OpenRecent(11));
        assert_eq!(app.state.path.as_deref().map(|p| label(p)).as_deref(), Some("p0.flp"));
        // A file that has gone: said, dropped, nothing opened.
        std::fs::remove_file(&files[5]).unwrap();
        let at = list().iter().position(|p| label(p) == "p5.flp").unwrap();
        app.menu_item(0, RECENT_PICK + at);
        assert_eq!(app.top_message().map(|m| m.0).as_deref(), Some("Project not found"));
        assert!(!list().iter().any(|p| label(p) == "p5.flp"));
        assert_eq!(app.state.path.as_deref().map(|p| label(p)).as_deref(), Some("p0.flp"));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// FILE's section, between the exports and Revert to last backup: the heading, the ten newest with their keys (the
    /// full path as the hint), More ▸ opening the older ones; nothing while the list is empty. The native menu bar has
    /// the same rows.
    #[test]
    fn file_menu_section() {
        use crate::app::Action;
        use crate::views::menus::{self, Kind, FILE_EXIT, FILE_EXPORT_MIDI, RECENT_MORE};
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.menu_open = Some(0);
        app.paint(&mut c, None);
        let row = |app: &App, a: Action| app.hits.iter().rev().find(|h| h.action == a).map(|h| (h.rect, h.hint.to_string()));
        assert!(!app.hits.iter().any(|h| matches!(h.action, Action::MenuItem(0, i) if i >= RECENT_PICK)), "an empty list shows nothing");
        let exit_before = row(&app, Action::MenuItem(0, FILE_EXIT)).expect("Exit").0;
        for n in 0..12 {
            add(&std::env::temp_dir().join(format!("r{n}.flp")));
        }
        app.paint(&mut c, None);
        let rows: Vec<_> = (0..SHOWN).map(|n| row(&app, Action::MenuItem(0, RECENT_PICK + n)).expect("ten rows")).collect();
        assert!(row(&app, Action::MenuItem(0, RECENT_PICK + SHOWN)).is_none(), "the eleventh is in More");
        assert_eq!(rows[0].1, list()[0].to_string_lossy(), "the hint is the full path");
        let midi = row(&app, Action::MenuItem(0, FILE_EXPORT_MIDI)).expect("Export MIDI").0;
        let more = row(&app, Action::MenuSub(0, RECENT_MORE)).expect("More").0;
        let exit = row(&app, Action::MenuItem(0, FILE_EXIT)).expect("Exit").0;
        assert!(midi.bottom() < rows[0].0.y && rows[9].0.bottom() <= more.y && more.bottom() < exit.y);
        let mt = os_ui::kit::menu::metrics(1.0);
        assert_eq!(exit.y - exit_before.y, mt.head + 11.0 * mt.row, "the heading, ten rows and More");
        // More ▸ opens beside its row, the dropdown staying open.
        app.mouse_move(more.cx(), more.cy());
        app.mouse_down();
        app.mouse_up();
        let p = app.state.playlist.popup.as_ref().expect("More opens its submenu");
        assert_eq!(p.items.iter().map(|i| i.label.to_string()).collect::<Vec<_>>(), ["r1.flp", "r0.flp"]);
        assert_eq!((app.state.menu_open, app.state.menu_sub), (Some(0), Some(RECENT_MORE)));
        // The native menu bar: the same rows; More stays greyed out there (no native submenus yet).
        let bar = &menus::bar_model(&app.state)[0];
        let at = bar.rows.iter().position(|r| r.label == HEADING).expect("the heading");
        assert_eq!((bar.rows[at].kind, bar.rows[at - 1].label.as_str()), (Kind::Head, "Export MIDI..."));
        let first = &bar.rows[at + 1];
        assert_eq!((first.label.as_str(), first.shortcut, first.pick), ("r11.flp", "Alt+1", Some((0, RECENT_PICK))));
        assert_eq!((bar.rows[at + 10].shortcut, bar.rows[at + 11].label.as_str(), bar.rows[at + 11].enabled), ("Alt+0", MORE, false));
        assert_eq!(bar.rows[at + 12].kind, Kind::Sep);
        assert_eq!(menus::pick(0, RECENT_PICK + 3), Action::MenuItem(0, RECENT_PICK + 3));
    }
}
