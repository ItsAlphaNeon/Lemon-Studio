// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

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
use crate::dialog::{spans, Answer, Kind, Msg, Reply, Then};

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

/// The data folder's subfolders: the backups and the copied samples (spec: ui/data-folder.md §3).
pub const BACKUP_DIR: &str = "Backup";
pub const SAMPLES_DIR: &str = "Samples";

/// The questions of the Copy check and of the folder button, with their buttons' labels (Yes, No), and the folder
/// picker's title (spec: ui/data-folder.md §4, §5).
pub const ATTENTION: &str = "Attention";
pub const COPY_QUESTION: &str = "What should be done with the samples used in this project?";
pub const COPY_LABELS: [&str; 2] = ["Copy to the data folder", "Don't copy"];
pub const PICK_TITLE: &str = "Select a directory";
pub const CHANGED_QUESTION: &str = "The project data folder has been changed. What do you want to do with the current project?";
pub const CHANGED_LABELS: [&str; 2] = ["Save to the folder", "Don't save to the folder"];

/// What waits for one of the data folder's dialogs (`Then::DataFolder`).
#[derive(Debug, Clone, PartialEq)]
pub enum DThen {
    /// The Copy check's question: Yes copies every sample the project uses.
    CopySamples,
    /// The folder button's picker.
    Picked,
    /// The question after a folder was picked: Yes saves the project there.
    Changed(PathBuf),
    /// Rename's confirmation: the project's new name and, with Rename data folder ticked, the folder's.
    Rename { name: String, folder: Option<String> },
    /// The Save as and New project forms' folder button: the picker of their root folder (`crate::save_form`).
    FormRoot,
}

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

/// Where sample `src` goes in folder `samples` (spec: ui/data-folder.md §4, Lemon Studio rule for name clashes): a file
/// of its name and size there is used as it is (the sample itself, or a copy made before); a different file of that
/// name gives `name (2).ext`, `name (3).ext`, and so on. Returns the place and whether a copy must be made there.
pub fn copy_target(src: &Path, samples: &Path) -> Option<(PathBuf, bool)> {
    let size = std::fs::metadata(src).ok().filter(|m| m.is_file())?.len();
    let stem = src.file_stem()?.to_string_lossy().into_owned();
    let ext = src.extension().map_or_else(String::new, |e| format!(".{}", e.to_string_lossy()));
    for n in 1u32.. {
        let to = if n == 1 { samples.join(src.file_name()?) } else { samples.join(format!("{stem} ({n}){ext}")) };
        match std::fs::metadata(&to) {
            Ok(m) if m.is_file() && m.len() == size => return Some((to, false)),
            Ok(_) => continue,
            Err(_) => return Some((to, true)),
        }
    }
    None
}

/// Copies `src` to `to`, which keeps the original's file times (spec: ui/data-folder.md §4).
pub fn copy_keeping_times(src: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::copy(src, to)?;
    let m = std::fs::metadata(src)?;
    let mut times = std::fs::FileTimes::new();
    if let Ok(t) = m.modified() {
        times = times.set_modified(t);
    }
    if let Ok(t) = m.accessed() {
        times = times.set_accessed(t);
    }
    #[cfg(windows)]
    if let Ok(t) = m.created() {
        use std::os::windows::fs::FileTimesExt;
        times = times.set_created(t);
    }
    std::fs::OpenOptions::new().write(true).open(to)?.set_times(times)
}

/// Copies sample `src` into `<dir>/Samples` (`copy_target`): where the channel finds it now.
pub fn copy_sample(src: &Path, dir: &Path) -> std::io::Result<PathBuf> {
    let samples = dir.join(SAMPLES_DIR);
    let Some((to, copy)) = copy_target(src, &samples) else { return Err(std::io::ErrorKind::NotFound.into()) };
    if copy {
        std::fs::create_dir_all(&samples)?;
        copy_keeping_times(src, &to)?;
    }
    Ok(to)
}

/// A sample about to be loaded into a channel: while Copy used samples to the project folder is ticked and the project
/// has a data folder, its copy in `<data folder>/Samples`, made now (spec: ui/data-folder.md §4); else `path` itself
/// (also when the copy fails).
pub fn adopt(s: &State, path: &str) -> String {
    if !crate::prefs::check(&crate::prefs::COPY_USED_SAMPLES) {
        return path.to_string();
    }
    let Some(dir) = folder(s) else { return path.to_string() };
    reread(s);
    copy_sample(Path::new(&crate::plugins::resolve_sample(path)), &dir).map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned())
}

/// Files were made or renamed in the data folder: the browser reads its folders again (`views::browser::list_dir`).
pub(crate) fn reread(s: &State) {
    s.browser_dirs.borrow_mut().clear();
}

/// Whether `name` can name a file or folder: not empty, not only dots, none of the characters Windows refuses.
pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.chars().all(|c| c == '.') && !n.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
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

    /// The program setting Copy used samples to the project folder was clicked: ticking it, with a data folder, asks
    /// what to do with the samples the project uses (spec: ui/data-folder.md §4).
    pub(crate) fn toggle_copy_samples(&mut self) {
        self.pref_toggle(crate::prefs::COPY_USED_SAMPLES.label);
        if crate::prefs::check(&crate::prefs::COPY_USED_SAMPLES) && folder(&self.state).is_some() {
            let m = Msg::new(Kind::Question, ATTENTION, spans(&[COPY_QUESTION]), &[Answer::Yes, Answer::No], Then::DataFolder(DThen::CopySamples));
            self.show_msg(m.labelled(&COPY_LABELS));
        }
    }

    /// Copies every sample the project uses into `<data folder>/Samples` (spec: ui/data-folder.md §4): the channels use
    /// the copies from now on (Lemon Sampler loads them), which counts as a change of the project. Returns the message.
    pub(crate) fn copy_used_samples(&mut self) -> Option<String> {
        let dir = folder(&self.state)?;
        Some(self.copy_used_samples_to(&dir))
    }

    /// Copies every sample the project uses into `<dir>/Samples`, as `copy_used_samples` does for the data folder (the
    /// Save as and New project forms copy into the folder they make). Returns the message.
    pub(crate) fn copy_used_samples_to(&mut self, dir: &Path) -> String {
        let (mut copied, mut failed) = (0, 0);
        for row in 0..self.state.project.channels.len() {
            // A hosted Lemon Sampler's own sample is the one it plays; it loads the copy too.
            let hosted = self.plugins.sampler_path(row);
            let listed = self.state.project.channels[row].sample_path.as_deref().filter(|p| !p.is_empty()).map(crate::plugins::resolve_sample);
            let Some(src) = hosted.clone().filter(|p| !p.is_empty()).or(listed) else { continue };
            match copy_sample(Path::new(&src), dir) {
                Ok(to) => {
                    let to = to.to_string_lossy().into_owned();
                    if hosted.is_some_and(|h| h != to) {
                        self.plugins.load_sample(row, &to);
                    }
                    self.state.project.channels[row].sample_path = Some(to);
                    copied += 1;
                }
                Err(_) => failed += 1,
            }
        }
        reread(&self.state);
        self.invalidate();
        let to = dir.join(SAMPLES_DIR);
        match failed {
            0 => format!("Copied {copied} samples to {}", to.display()),
            n => format!("Copied {copied} samples to {} ({n} not found or not copied)", to.display()),
        }
    }

    /// The folder button (or the path beside it): a folder picker, then what to do with the project (spec:
    /// ui/data-folder.md §5).
    pub(crate) fn data_folder_pick(&mut self) {
        use crate::filedlg::{FileDlg, Mode};
        let start = folder(&self.state).filter(|d| d.is_dir());
        let dlg = FileDlg::new(Mode::Folder, PICK_TITLE, "data folder", Then::DataFolder(DThen::Picked)).start(start.as_deref());
        self.file_dialog(dlg);
    }

    /// The data folder becomes `dir`, with the choice "Store data in the specified folder" (a change of the project).
    pub(crate) fn set_data_folder(&mut self, dir: &Path) {
        let df = self.state.project.data_folder.get_or_insert_with(DataFolder::default);
        df.choice = DataFolder::SPECIFIED;
        df.folder = folder_text(dir);
        self.invalidate();
    }

    /// One of the data folder's dialogs was answered (`Then::DataFolder`).
    pub(crate) fn data_folder_done(&mut self, then: DThen, reply: Reply) {
        let yes = reply == Reply::Button(Answer::Yes);
        match (then, reply) {
            // Don't copy: nothing is copied, and the check stays ticked (Lemon Studio rule).
            (DThen::CopySamples, _) => {
                if yes {
                    self.state.message = self.copy_used_samples();
                }
            }
            (DThen::Picked, Reply::File(Some(dir))) => {
                let m = Msg::new(Kind::Question, ATTENTION, spans(&[CHANGED_QUESTION]), &[Answer::Yes, Answer::No], Then::DataFolder(DThen::Changed(dir)));
                self.show_msg(m.labelled(&CHANGED_LABELS));
            }
            (DThen::Picked, _) => {}
            // Save to the folder: the project goes to `<folder>/<name>.flp`, its samples to `<folder>/Samples` while Copy
            // is ticked; the old folder and file stay. Don't save to the folder: the folder only (Lemon Studio rule).
            (DThen::Changed(dir), _) => {
                self.set_data_folder(&dir);
                self.state.message = Some(format!("Data folder: {}", dir.display()));
                if yes {
                    if crate::prefs::check(&crate::prefs::COPY_USED_SAMPLES) {
                        self.copy_used_samples();
                    }
                    let name = crate::backup::project_name(self.state.path.as_deref());
                    self.save_to(dir.join(format!("{name}.flp")));
                    reread(&self.state);
                }
            }
            (DThen::Rename { name, folder }, _) => {
                if yes {
                    self.rename_project(&name, folder.as_deref());
                }
            }
            (DThen::FormRoot, Reply::File(Some(dir))) => self.form_root_chosen(dir),
            (DThen::FormRoot, _) => {}
        }
        self.invalidate();
    }

    /// Rename (spec: ui/data-folder.md §6), confirmed: with `folder_name`, the data folder is renamed to it, the project
    /// file inside it to `<name>.flp`, and the samples inside it are found in the renamed folder; without, only the
    /// project file is renamed, in its folder (Lemon Studio rule). Then the project is saved (the file it replaces goes
    /// to the backups, as on any save). A name that is taken is refused with a message, and nothing changes.
    pub(crate) fn rename_project(&mut self, name: &str, folder_name: Option<&str>) {
        let Some(file) = self.state.path.clone() else { return };
        // The folder's rename (old, new), where the project file is after it, and its new name.
        let folder_move = match folder_name.map(str::trim) {
            Some(f) => {
                let Some(old) = folder(&self.state) else { return };
                let new = old.with_file_name(f);
                (new != old).then_some((old, new))
            }
            None => None,
        };
        let file_after = match &folder_move {
            Some((old, new)) => file.strip_prefix(old).map_or_else(|_| file.clone(), |rel| new.join(rel)),
            None => file.clone(),
        };
        let target = file_after.with_file_name(format!("{}.flp", name.trim()));
        // Both names are checked before anything is renamed (the file's new name where the file is now).
        if let Some((_, new)) = folder_move.as_ref().filter(|(_, new)| new.exists()) {
            return self.rename_refused(new);
        }
        let target_now = match &folder_move {
            Some((old, new)) => target.strip_prefix(new).map_or_else(|_| target.clone(), |rel| old.join(rel)),
            None => target.clone(),
        };
        if target_now != file && target_now.exists() {
            return self.rename_refused(&target_now);
        }
        if let Some((old, new)) = &folder_move {
            if let Err(e) = std::fs::rename(old, new) {
                return self.rename_failed(old, &e);
            }
            self.state.path = Some(file_after.clone());
            // The samples inside the folder are found in the renamed one.
            for row in 0..self.state.project.channels.len() {
                let hosted = self.plugins.sampler_path(row);
                let ch = &mut self.state.project.channels[row];
                let Some(p) = hosted.clone().filter(|p| !p.is_empty()).or_else(|| ch.sample_path.as_deref().map(crate::plugins::resolve_sample)) else { continue };
                let Ok(rel) = Path::new(&p).strip_prefix(old) else { continue };
                let moved = new.join(rel).to_string_lossy().into_owned();
                ch.sample_path = Some(moved.clone());
                if hosted.is_some() {
                    self.plugins.load_sample(row, &moved);
                }
            }
            if let Some(df) = self.state.project.data_folder.as_mut().filter(|d| d.choice == DataFolder::SPECIFIED) {
                df.folder = folder_text(new);
            }
        }
        if target != file_after {
            if let Err(e) = std::fs::rename(&file_after, &target) {
                return self.rename_failed(&file_after, &e);
            }
            self.state.path = Some(target.clone());
        }
        self.save_to(target);
        reread(&self.state);
    }

    fn rename_refused(&mut self, taken: &Path) {
        let n = taken.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.tell(Kind::Warning, "Rename project", spans(&["", &n, " already exists. Nothing was renamed."]));
    }

    fn rename_failed(&mut self, what: &Path, e: &std::io::Error) {
        let n = what.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.tell(Kind::Error, "Rename project", spans(&["", &n, &format!(" could not be renamed: {e}")]));
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

    /// A file of `n` bytes at `p` (a stand-in sample; nothing reads its sound here).
    fn file(p: &Path, n: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![7u8; n]).unwrap();
    }

    /// A project saved as `<dir>/<name>/<name>.flp` with "relative": its data folder is `<dir>/<name>`.
    fn saved_with_folder(dir: &Path, name: &str) -> App {
        let mut app = App::new(None);
        app.set_data_folder_choice(DataFolder::RELATIVE);
        std::fs::create_dir_all(dir.join(name)).unwrap();
        app.save_to(dir.join(name).join(format!("{name}.flp")));
        assert_eq!(folder(&app.state), Some(dir.join(name)));
        app
    }

    /// The top message box's title, text and button labels.
    fn top_msg(app: &App) -> Option<(String, String, Vec<&'static str>)> {
        match &app.state.dialogs.last()?.body {
            crate::dialog::Body::Msg(m) => Some((m.title.clone(), m.plain(), (0..m.buttons.len()).map(|i| m.label(i)).collect())),
            _ => None,
        }
    }

    /// Name clashes in Samples (Lemon Studio rule): a file of the same name and size is used as it is; another file of
    /// that name gives `name (2).ext`, then `name (3).ext`. A copy keeps the original's modified time.
    #[test]
    fn copies_and_name_clashes() {
        let dir = scratch("clash");
        let samples = dir.join("df").join(SAMPLES_DIR);
        let kick = dir.join("a").join("kick.wav");
        file(&kick, 10);
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
        std::fs::OpenOptions::new().write(true).open(&kick).unwrap().set_modified(old).unwrap();
        assert_eq!(copy_target(&kick, &samples), Some((samples.join("kick.wav"), true)));
        let to = copy_sample(&kick, &dir.join("df")).unwrap();
        assert_eq!(to, samples.join("kick.wav"));
        assert_eq!(std::fs::metadata(&to).unwrap().modified().unwrap(), old, "the copy keeps the file time");
        assert_eq!(copy_target(&kick, &samples), Some((samples.join("kick.wav"), false)), "same name and size: used as it is");
        assert_eq!(copy_target(&to, &samples), Some((to.clone(), false)), "a sample already there");
        let other = dir.join("b").join("kick.wav");
        file(&other, 20);
        assert_eq!(copy_sample(&other, &dir.join("df")).unwrap(), samples.join("kick (2).wav"));
        let third = dir.join("c").join("kick.wav");
        file(&third, 30);
        assert_eq!(copy_target(&third, &samples), Some((samples.join("kick (3).wav"), true)));
        assert_eq!(copy_target(&other, &samples), Some((samples.join("kick (2).wav"), false)));
        assert_eq!(copy_target(&dir.join("missing.wav"), &samples), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ticking Copy used samples asks (Attention, Copy to the data folder / Don't copy); Copy copies every sample into
    /// `<data folder>/Samples`, the channels use the copies and the project counts as changed; Don't copy copies
    /// nothing and the check stays ticked. While it is ticked, a sample loaded into a channel is copied at once.
    #[test]
    fn copy_used_samples() {
        let dir = scratch("copy");
        let mut app = saved_with_folder(&dir, "song");
        let kick = dir.join("lib").join("kick.wav");
        file(&kick, 12);
        app.state.project.channels[0].sample_path = Some(kick.to_string_lossy().into_owned());
        app.mark_saved();
        // Don't copy.
        app.toggle_copy_samples();
        assert_eq!(top_msg(&app), Some((ATTENTION.into(), COPY_QUESTION.into(), COPY_LABELS.to_vec())));
        app.dialog_click(crate::dialog::DHit::Button(1));
        assert!(app.state.dialogs.is_empty());
        assert!(crate::prefs::check(&crate::prefs::COPY_USED_SAMPLES), "stays ticked");
        assert!(!dir.join("song").join(SAMPLES_DIR).exists() && !app.unsaved());
        // Off, then on again: Copy to the data folder.
        app.toggle_copy_samples();
        assert!(app.state.dialogs.is_empty(), "unticking asks nothing");
        app.toggle_copy_samples();
        app.dialog_click(crate::dialog::DHit::Button(0));
        let copy = dir.join("song").join(SAMPLES_DIR).join("kick.wav");
        assert!(copy.is_file());
        assert_eq!(app.state.project.channels[0].sample_path.as_deref(), Some(copy.to_string_lossy().as_ref()));
        assert!(app.unsaved(), "a change of the project");
        // The next save writes the new path.
        app.save_to(dir.join("song").join("song.flp"));
        let opened = App::new(Some(dir.join("song").join("song.flp")));
        assert_eq!(opened.state.project.channels[0].sample_path.as_deref(), Some(copy.to_string_lossy().as_ref()));
        // A sample loaded while it is ticked.
        let snare = dir.join("lib").join("snare.wav");
        file(&snare, 5);
        crate::rack::sample_chosen(&mut app, 0, snare.clone());
        let copy = dir.join("song").join(SAMPLES_DIR).join("snare.wav");
        assert!(copy.is_file());
        assert_eq!(app.state.project.channels[0].sample_path.as_deref(), Some(copy.to_string_lossy().as_ref()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The folder button: a picker titled "Select a directory", then Attention. Save to the folder saves the project as
    /// `<folder>/<name>.flp` with the specified folder (and copies the samples while Copy is ticked); the old file stays.
    /// Don't save to the folder only sets the folder.
    #[test]
    fn the_folder_button() {
        let dir = scratch("pick");
        let mut app = saved_with_folder(&dir, "song");
        let kick = dir.join("lib").join("kick.wav");
        file(&kick, 12);
        app.state.project.channels[0].sample_path = Some(kick.to_string_lossy().into_owned());
        let picked = dir.join("picked");
        std::fs::create_dir_all(&picked).unwrap();
        // Don't save to the folder.
        crate::dialog::fake::answer(Some(picked.clone()));
        app.data_folder_pick();
        assert_eq!(top_msg(&app), Some((ATTENTION.into(), CHANGED_QUESTION.into(), CHANGED_LABELS.to_vec())));
        app.dialog_click(crate::dialog::DHit::Button(1));
        assert_eq!(app.state.project.data_folder, Some(DataFolder { choice: DataFolder::SPECIFIED, folder: folder_text(&picked) }));
        assert_eq!(app.state.path, Some(dir.join("song").join("song.flp")), "nothing saved");
        assert!(std::fs::read_dir(&picked).unwrap().next().is_none());
        // Save to the folder, with Copy ticked.
        crate::prefs::set_check(&crate::prefs::COPY_USED_SAMPLES, true);
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        crate::dialog::fake::answer(Some(other.clone()));
        app.data_folder_pick();
        app.dialog_click(crate::dialog::DHit::Button(0));
        assert_eq!(app.state.path, Some(other.join("song.flp")));
        assert!(!app.unsaved());
        let opened = App::new(Some(other.join("song.flp")));
        assert_eq!(opened.state.project.data_folder, Some(DataFolder { choice: DataFolder::SPECIFIED, folder: folder_text(&other) }));
        let copy = other.join(SAMPLES_DIR).join("kick.wav");
        assert_eq!(opened.state.project.channels[0].sample_path.as_deref(), Some(copy.to_string_lossy().as_ref()));
        assert!(dir.join("song").join("song.flp").is_file(), "the old file stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Rename (spec: ui/data-folder.md §6): the form holds the names (the project's selected), Rename asks for
    /// confirmation; Yes renames the folder and the file in it, the samples inside follow, and the project is saved (the
    /// replaced file goes to Backup). A name that is taken is refused and nothing changes; unticked, only the file is
    /// renamed.
    #[test]
    fn renaming_the_project() {
        use crate::project_forms::{Form, Rename};
        use winit::keyboard::{Key, NamedKey};
        let dir = scratch("rename");
        let mut app = saved_with_folder(&dir, "t2spec");
        let kick = dir.join("t2spec").join(SAMPLES_DIR).join("kick.wav");
        file(&kick, 12);
        app.state.project.channels[0].sample_path = Some(kick.to_string_lossy().into_owned());
        app.save_to(dir.join("t2spec").join("t2spec.flp"));
        app.open_rename();
        let form = |app: &App| match &app.state.dialogs.last().unwrap().body {
            crate::dialog::Body::Project(Form::Rename(f)) => f.clone(),
            _ => panic!("the Rename form is on top"),
        };
        let f = form(&app);
        assert_eq!((f.name.text.as_str(), f.name.all, f.folder.text.as_str(), f.rename_folder, f.use_name), ("t2spec", true, "t2spec", true, false));
        // Use the project name: the folder's name follows the project's.
        app.form_click(crate::project_forms::FHit::Check(1));
        app.key(&Key::Character("T3".into()), false, false);
        assert_eq!(form(&app).folder_name(), "T3");
        app.key(&Key::Named(NamedKey::Enter), false, false);
        assert_eq!(top_msg(&app).map(|m| (m.0, m.1)), Some(("Confirmation".into(), "Are you sure you want to rename this project and its data folder?".into())));
        app.dialog_click(crate::dialog::DHit::Button(0));
        let new = dir.join("T3");
        assert!(!dir.join("t2spec").exists() && new.join("T3.flp").is_file() && !new.join("t2spec.flp").exists());
        assert_eq!(app.state.path, Some(new.join("T3.flp")));
        let moved = new.join(SAMPLES_DIR).join("kick.wav");
        let opened = App::new(Some(new.join("T3.flp")));
        assert_eq!(opened.state.project.channels[0].sample_path.as_deref(), Some(moved.to_string_lossy().as_ref()));
        assert_eq!(opened.state.project.data_folder.unwrap().folder, folder_text(&new), "the save wrote the folder");
        let backups: Vec<String> = std::fs::read_dir(new.join(BACKUP_DIR)).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(backups.iter().any(|n| n.starts_with("T3 (overwritten on ")), "the replaced file went to Backup: {backups:?}");
        // A folder name that is taken: refused, nothing changes.
        std::fs::create_dir_all(dir.join("Taken")).unwrap();
        app.rename_project("Taken", Some("Taken"));
        assert_eq!(top_msg(&app).map(|m| m.0), Some(Rename::TITLE.to_string()));
        assert!(new.join("T3.flp").is_file() && app.state.path == Some(new.join("T3.flp")));
        app.dialog_click(crate::dialog::DHit::Button(0));
        // Unticked: only the file.
        app.open_rename();
        app.form_click(crate::project_forms::FHit::Check(0));
        app.key(&Key::Character("T4".into()), false, false);
        app.key(&Key::Named(NamedKey::Enter), false, false);
        assert_eq!(top_msg(&app).map(|m| m.1), Some("Are you sure you want to rename this project?".into()));
        app.dialog_click(crate::dialog::DHit::Button(0));
        assert!(new.join("T4.flp").is_file() && !new.join("T3.flp").exists());
        // Cancel and an empty name do nothing.
        app.open_rename();
        app.key(&Key::Named(NamedKey::Backspace), false, false);
        app.key(&Key::Named(NamedKey::Enter), false, false);
        assert_eq!(top_msg(&app).map(|m| m.0), Some(Rename::TITLE.to_string()), "an empty name is refused");
        app.dialog_click(crate::dialog::DHit::Button(0));
        app.key(&Key::Named(NamedKey::Escape), false, false);
        assert!(app.state.dialogs.is_empty());
        assert!(new.join("T4.flp").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Rename form is drawn with its parts; its buttons and checks take clicks.
    #[test]
    fn the_rename_form_is_drawn() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let dir = scratch("rename_ui");
        let mut app = saved_with_folder(&dir, "p");
        app.open_settings(settings::PROJECT);
        app.paint(&mut c, None);
        let r = hit(&app, &Action::Settings(SAct::Button(RENAME))).expect("Rename works");
        click(&mut app, &mut c, r);
        use crate::dialog::DHit;
        use crate::project_forms::FHit;
        for h in [DHit::Form(FHit::Field(0)), DHit::Form(FHit::Field(1)), DHit::Form(FHit::Check(0)), DHit::Form(FHit::Check(1)), DHit::Form(FHit::Main), DHit::Cancel] {
            assert!(hit(&app, &Action::Dlg(h)).is_some(), "{h:?}");
        }
        let r = hit(&app, &Action::Dlg(DHit::Cancel)).unwrap();
        click(&mut app, &mut c, r);
        assert!(app.state.dialogs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The browser's Current project tree: a Project data entry just before Samples, only while the project has a data
    /// folder; it opens like a folder (spec: ui/data-folder.md §3).
    #[test]
    fn the_browser_lists_the_data_folder() {
        use crate::views::browser as view;
        let dir = scratch("browser");
        let mut app = App::new(None);
        let key = format!("{}/{}", view::project_root(), view::PROJECT_DATA);
        let shown = |app: &mut App| {
            app.state.browser_tab = view::PROJECT_TAB;
            app.state.browser_open.insert(view::project_root());
            view::rows(&app.state)
        };
        assert!(!shown(&mut app).iter().any(|r| r.key == key), "no data folder: no entry");
        let mut app = saved_with_folder(&dir, "song");
        file(&dir.join("song").join(SAMPLES_DIR).join("kick.wav"), 4);
        let rows = shown(&mut app);
        let i = rows.iter().position(|r| r.key == key).expect("Project data");
        assert_eq!((rows[i].label.as_str(), rows[i].open, rows[i].path.clone()), (view::PROJECT_DATA, Some(false), Some(dir.join("song"))));
        assert_eq!(rows[i + 1].label, "Samples", "just before Samples");
        app.state.browser_open.insert(key.clone());
        app.state.browser_open.insert(format!("{key}/{SAMPLES_DIR}"));
        let rows = shown(&mut app);
        let inside: Vec<_> = rows.iter().filter(|r| r.key.starts_with(&format!("{key}/"))).map(|r| (r.depth, r.label.clone())).collect();
        assert_eq!(inside, vec![(2, SAMPLES_DIR.to_string()), (3, "kick".to_string()), (2, "song.flp".to_string())]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
