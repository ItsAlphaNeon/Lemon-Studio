// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The project's data folder (settings window, Project ▸ Data folder): a folder of the project's own that holds its
//! backups (`crate::backup`) and the copies of the samples it uses. The choice and the folder belong to the project
//! (events 167 and 202, `Project::data_folder`); Ask for data folder and Copy used samples to the project folder are
//! program settings (`prefs::ASK_DATA_FOLDER`, `prefs::COPY_USED_SAMPLES`).
//!
//! Where it is: with "Store data relative to the project file", the project file's own folder, whatever event 202 says
//! (none while the project was never saved); with "Store data in the specified folder", event 202's folder; with
//! "Don't use a data folder", none. A save writes the folder in use into 202 (`stamp`).
// spec: ui/data-folder.md

use std::path::{Path, PathBuf};

use os_model::{DataFolder, Project};

use crate::app::{App, State};

/// The Data folder combo's key (no label is drawn beside it: the group's title names it) and its choices, in the order
/// of event 167's values.
pub const COMBO: &str = "Data folder";
pub const CHOICES: [&str; 3] = ["Don't use a data folder", "Store data relative to the project file", "Store data in the specified folder"];
/// The Rename button and the folder button (with the path beside it), by their `SAct::Button` keys.
pub const RENAME: &str = "Rename";
pub const SET_FOLDER: &str = "Set data folder...";
/// What the path reads without a data folder.
pub const NONE_TEXT: &str = "(none)";

/// The hints of the group's controls (spec: ui/data-folder.md §1).
pub const COMBO_HINT: &str = "Choose where to store project data";
pub const RENAME_HINT: &str = "Rename the current project";
pub const ASK_HINT: &str = "When should the program ask you to select a data folder for a project";
pub const COPY_HINT: &str = "Copy used samples into the project folder";

/// The data folder's subfolder of backups (spec: ui/data-folder.md §3).
pub const BACKUP_DIR: &str = "Backup";

/// What a pick in one of the data folder's menus does (`playlist::Cmd::DataFolder`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DCmd {
    /// The Data folder combo: event 167's value.
    Choice(u32),
}

/// The project's choice (event 167): `DataFolder::NONE` when the file has none.
pub fn choice(p: &Project) -> u32 {
    p.data_folder.as_ref().map_or(DataFolder::NONE, |d| d.choice)
}

/// The Data folder combo's text for the project's choice (a value FL does not write shows as the first).
pub fn choice_text(p: &Project) -> &'static str {
    CHOICES.get(choice(p) as usize).copied().unwrap_or(CHOICES[0])
}

/// A folder as event 202 holds it: the path with a trailing backslash (the file format's, on every system).
pub fn folder_text(dir: &Path) -> String {
    let s = dir.to_string_lossy();
    format!("{}\\", s.trim_end_matches(['\\', '/']))
}

/// Event 202's folder as a path: its trailing separator gone; `None` for an empty text.
pub fn from_text(text: &str) -> Option<PathBuf> {
    let t = text.strip_suffix(['\\', '/']).unwrap_or(text);
    (!t.is_empty()).then(|| PathBuf::from(t))
}

/// The data folder of project `p` saved as `path`: the file's own folder for "relative" (none for a project never
/// saved), event 202's for "specified", none for "Don't use a data folder" (spec: ui/data-folder.md §2, §5).
pub fn resolve(p: &Project, path: Option<&Path>) -> Option<PathBuf> {
    let df = p.data_folder.as_ref()?;
    match df.choice {
        DataFolder::RELATIVE => path?.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf),
        DataFolder::SPECIFIED => from_text(&df.folder),
        _ => None,
    }
}

/// The open project's data folder (`resolve`).
pub fn folder(s: &State) -> Option<PathBuf> {
    resolve(&s.project, s.path.as_deref())
}

/// The path the Project page shows beside the folder button: the data folder with a trailing separator, or `(none)`.
pub fn path_text(s: &State) -> String {
    folder(s).map_or_else(|| NONE_TEXT.to_string(), |d| format!("{}{}", d.display(), std::path::MAIN_SEPARATOR))
}

/// Whether Rename can be used: the project has a data folder and was saved (spec: ui/data-folder.md §1).
pub fn can_rename(s: &State) -> bool {
    s.path.is_some() && choice(&s.project) != DataFolder::NONE && folder(s).is_some()
}

/// Before the project is written as `proj`: event 202 gets the data folder in use, which for "relative" is the project
/// file's folder, whatever 202 said (spec: ui/data-folder.md §2: "the next save wrote that folder into 202"). The
/// folder is the one of the file the project was opened from or last saved as, so Save as writes the folder it leaves
/// (flp/events/project-info.md, event 202). Other choices keep 202 as it is.
pub fn stamp(s: &State, proj: &mut Project) {
    if choice(&s.project) != DataFolder::RELATIVE {
        return;
    }
    if let (Some(dir), Some(df)) = (folder(s), proj.data_folder.as_mut()) {
        df.folder = folder_text(&dir);
    }
}

/// Where a backup of the open project, saved as `path`, goes: with File ▸ Backup ▸ Location's "Save to the project data
/// folder if there is one" and a data folder, `<data folder>/Backup`; else the global folder (`backup::folder`). Spec:
/// ui/data-folder.md §3. The data folder is the one of `path` (the file a save replaces), and only one that exists
/// counts: a project from another computer, whose specified folder is not on this one, backs up to the global folder
/// (Lemon Studio rule). Tests: only data folders in the system's temporary folder.
pub fn backup_folder(s: &State, path: Option<&Path>) -> Option<PathBuf> {
    let dir = resolve(&s.project, path).filter(|d| d.is_dir());
    #[cfg(test)]
    let dir = dir.filter(|d| d.starts_with(std::env::temp_dir()));
    match dir {
        Some(dir) if crate::prefs::index(&crate::prefs::BACKUP_LOCATION) == 1 => Some(dir.join(BACKUP_DIR)),
        _ => crate::backup::folder(),
    }
}

impl App {
    /// The Data folder combo was clicked: its three choices under its box (`x`, `y`), the project's marked.
    pub(crate) fn data_folder_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let now = choice(&self.state.project);
        let items = (0..CHOICES.len() as u32).map(|i| PItem::new(CHOICES[i as usize], Cmd::DataFolder(DCmd::Choice(i))).mark(Mark::Radio(i == now))).collect();
        self.state.playlist.popup = Some(Popup { x, y: y + 1.0, items, sub: None });
        self.invalidate();
    }

    /// A pick in one of the data folder's menus (`Cmd::DataFolder`); returns the message.
    pub(crate) fn data_folder_cmd(&mut self, c: DCmd) -> Option<String> {
        match c {
            DCmd::Choice(i) => self.set_data_folder_choice(i),
        }
    }

    /// The Data folder combo's choice `i` (spec: ui/data-folder.md §2, §5): Don't use a data folder empties event 202;
    /// relative makes the project file's folder the data folder (none until a project never saved is saved); the
    /// specified folder keeps the folder in use and opens no picker. A change counts as a change of the project.
    pub(crate) fn set_data_folder_choice(&mut self, i: u32) -> Option<String> {
        let s = &mut self.state;
        let Some(&label) = CHOICES.get(i as usize) else { return None };
        if i != choice(&s.project) {
            let now = folder(s);
            let df = s.project.data_folder.get_or_insert_with(DataFolder::default);
            df.choice = i;
            match i {
                DataFolder::NONE => df.folder.clear(),
                _ => {
                    // Relative: the project file's folder; specified: the folder in use, else the one 202 holds.
                    let dir = if i == DataFolder::RELATIVE { s.path.as_deref().and_then(Path::parent).map(Path::to_path_buf) } else { now };
                    if let Some(d) = dir {
                        df.folder = folder_text(&d);
                    }
                }
            }
        }
        self.invalidate();
        Some(format!("Data folder: {label}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::{self, SAct};

    /// A fresh scratch folder for one test (removed again by the caller).
    pub(crate) fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("os_datafolder_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Event 202's text: a trailing backslash, read back without it.
    #[test]
    fn folder_texts() {
        assert_eq!(folder_text(Path::new("C:\\Songs\\T2")), "C:\\Songs\\T2\\");
        assert_eq!(folder_text(Path::new("C:\\Songs\\T2\\")), "C:\\Songs\\T2\\");
        assert_eq!(folder_text(Path::new("/home/me/songs")), "/home/me/songs\\");
        assert_eq!(from_text("C:\\Songs\\T2\\"), Some(PathBuf::from("C:\\Songs\\T2")));
        assert_eq!(from_text("/home/me/songs\\"), Some(PathBuf::from("/home/me/songs")));
        assert_eq!((from_text(""), from_text("\\")), (None, None));
    }

    /// Relative: the opened file's own folder, whatever 202 says, none before the first save; specified: 202's folder;
    /// none: none (spec: ui/data-folder.md §2).
    #[test]
    fn where_the_data_folder_is() {
        let mut p = Project::default();
        let file = Path::new("/music/song/song.flp");
        assert_eq!(resolve(&p, Some(file)), None);
        p.data_folder = Some(DataFolder { choice: DataFolder::RELATIVE, folder: "C:\\Elsewhere\\".into() });
        assert_eq!(resolve(&p, Some(file)), Some(PathBuf::from("/music/song")));
        assert_eq!(resolve(&p, None), None, "never saved");
        p.data_folder = Some(DataFolder { choice: DataFolder::SPECIFIED, folder: "C:\\Elsewhere\\".into() });
        assert_eq!(resolve(&p, Some(file)), Some(PathBuf::from("C:\\Elsewhere")));
        assert_eq!(resolve(&p, None), Some(PathBuf::from("C:\\Elsewhere")));
        p.data_folder = Some(DataFolder { choice: DataFolder::NONE, folder: "C:\\Elsewhere\\".into() });
        assert_eq!(resolve(&p, Some(file)), None);
    }

    /// The combo's choices: relative takes the project's folder (none before a save), specified keeps the folder in
    /// use, none empties 202; each is a change of the project. A save writes the folder in use into 202, and a file
    /// whose 202 names another folder opens with its own folder.
    #[test]
    fn choosing_and_saving_the_data_folder() {
        let dir = scratch("choose");
        let mut app = App::new(None);
        assert_eq!((choice_text(&app.state.project), path_text(&app.state)), (CHOICES[0], NONE_TEXT.to_string()));
        app.set_data_folder_choice(DataFolder::RELATIVE);
        assert!(app.unsaved());
        assert_eq!(folder(&app.state), None, "never saved: none yet");
        let song = dir.join("song.flp");
        app.save_to(song.clone());
        assert_eq!(folder(&app.state), Some(dir.clone()));
        assert_eq!(path_text(&app.state), format!("{}{}", dir.display(), std::path::MAIN_SEPARATOR));
        app.save_to(song.clone());
        let opened = App::new(Some(song.clone()));
        assert_eq!(opened.state.project.data_folder, Some(DataFolder { choice: DataFolder::RELATIVE, folder: folder_text(&dir) }));
        // A copy elsewhere still names `dir` in 202: opened, its data folder is its own folder; saved, 202 follows.
        let other = dir.join("moved");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::copy(&song, other.join("song.flp")).unwrap();
        let mut moved = App::new(Some(other.join("song.flp")));
        assert_eq!(folder(&moved.state), Some(other.clone()));
        moved.save_to(other.join("song.flp"));
        assert_eq!(App::new(Some(other.join("song.flp"))).state.project.data_folder.unwrap().folder, folder_text(&other));
        // Specified keeps the folder; none empties 202.
        moved.set_data_folder_choice(DataFolder::SPECIFIED);
        assert_eq!(moved.state.project.data_folder.as_ref().unwrap().folder, folder_text(&other));
        assert_eq!(folder(&moved.state), Some(other.clone()));
        moved.set_data_folder_choice(DataFolder::NONE);
        assert_eq!(moved.state.project.data_folder, Some(DataFolder::default()));
        assert_eq!(path_text(&moved.state), NONE_TEXT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With a data folder and Location's "project data folder" (the default), a save first copies the file it replaces
    /// into `<data folder>/Backup`; with "Always save to the global folder", or without a data folder, into the global
    /// folder (spec: ui/data-folder.md §3).
    #[test]
    fn backups_go_to_the_data_folder() {
        let dir = scratch("backup");
        let global = dir.join("global");
        crate::backup::TEST_DIR.with(|d| *d.borrow_mut() = Some(global.clone()));
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let mut app = App::new(None);
        app.save_to(work.join("t.flp"));
        app.state.tempo = 99.0;
        app.save_to(work.join("t.flp"));
        let count = |d: &Path| std::fs::read_dir(d).map(|r| r.count()).unwrap_or(0);
        assert_eq!((count(&global), count(&work.join(BACKUP_DIR))), (1, 0), "no data folder: the global folder");
        app.set_data_folder_choice(DataFolder::RELATIVE);
        app.state.tempo = 98.0;
        app.save_to(work.join("t.flp"));
        assert_eq!((count(&global), count(&work.join(BACKUP_DIR))), (1, 1), "into <data folder>/Backup");
        let path = app.state.path.clone();
        assert_eq!(backup_folder(&app.state, path.as_deref()), Some(work.join(BACKUP_DIR)));
        crate::prefs::set_choice(&crate::prefs::BACKUP_LOCATION, 0);
        assert_eq!(backup_folder(&app.state, path.as_deref()), Some(global.clone()), "Always save to the global folder");
        crate::prefs::set_choice(&crate::prefs::BACKUP_LOCATION, 1);
        // A specified folder that does not exist (a project from another computer): the global folder.
        app.state.project.data_folder = Some(DataFolder { choice: DataFolder::SPECIFIED, folder: folder_text(&dir.join("gone")) });
        assert_eq!(backup_folder(&app.state, path.as_deref()), Some(global.clone()));
        crate::backup::TEST_DIR.with(|d| *d.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn hit(app: &App, a: &Action) -> Option<os_ui::Rect> {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect)
    }

    fn click(app: &mut App, c: &mut os_ui::Canvas, r: os_ui::Rect) {
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        app.paint(c, None);
    }

    /// The Project page's group: the combo (its menu marks the choice), Rename greyed without a saved project with a
    /// data folder, the path or `(none)`, Ask for data folder (default When saving unsaved projects only) and Copy used
    /// samples greyed without a data folder; the hints as the spec has them.
    #[test]
    fn the_project_page_group() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let dir = scratch("page");
        let mut app = App::new(None);
        app.open_settings(settings::PROJECT);
        app.paint(&mut c, None);
        let combo = app.hits.iter().find(|h| matches!(h.action, Action::Settings(SAct::Combo(COMBO, ..)))).expect("the combo works");
        assert_eq!(combo.hint, COMBO_HINT);
        assert!(hit(&app, &Action::Settings(SAct::Button(RENAME))).is_none(), "Rename is greyed");
        assert!(hit(&app, &Action::Settings(SAct::Check(crate::prefs::COPY_USED_SAMPLES.label))).is_none(), "Copy is greyed");
        assert!(hit(&app, &Action::Settings(SAct::Button(SET_FOLDER))).is_some(), "the folder button works");
        let ask = app.hits.iter().find(|h| matches!(h.action, Action::Settings(SAct::Combo(l, ..)) if l == crate::prefs::ASK_DATA_FOLDER.label)).expect("Ask works");
        assert_eq!(ask.hint, ASK_HINT);
        assert_eq!(crate::prefs::choice(&crate::prefs::ASK_DATA_FOLDER), "When saving unsaved projects only");
        // The combo's menu: three choices, the project's marked; a pick sets it.
        let r = combo.rect;
        click(&mut app, &mut c, r);
        let p = app.state.playlist.popup.as_ref().expect("its menu");
        assert_eq!(p.items.iter().map(|i| i.label.as_ref()).collect::<Vec<_>>(), CHOICES);
        assert_eq!(p.items[0].mark, crate::playlist::Mark::Radio(true));
        let r = hit(&app, &Action::PlPopup(1)).unwrap();
        click(&mut app, &mut c, r);
        assert_eq!(choice(&app.state.project), DataFolder::RELATIVE);
        assert_eq!(app.state.message.as_deref(), Some("Data folder: Store data relative to the project file"));
        // Saved: Rename and Copy work; the path shows the folder.
        app.save_to(dir.join("p.flp"));
        app.paint(&mut c, None);
        let r = hit(&app, &Action::Settings(SAct::Button(RENAME))).expect("Rename works");
        assert_eq!(app.hits.iter().rev().find(|h| h.rect == r).unwrap().hint, RENAME_HINT);
        let copy = app.hits.iter().find(|h| h.action == Action::Settings(SAct::Check(crate::prefs::COPY_USED_SAMPLES.label))).expect("Copy works");
        assert_eq!(copy.hint, COPY_HINT);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
