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

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::path::{Path, PathBuf};

use os_model::{DataFolder, Project};


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

/// Where a backup of the open project, saved as `path`, goes: with File ▸ Backup ▸ Location's "Save to the project data
/// folder if there is one" and a data folder, `<data folder>/Backup`; else the global folder (`backup::folder`). Spec:
/// ui/data-folder.md §3. The data folder is the one of `path` (the file a save replaces), and only one that exists
/// counts: a project from another computer, whose specified folder is not on this one, backs up to the global folder
/// (Lemon Studio rule). Tests: only data folders in the system's temporary folder.
pub fn backup_folder(s: &crate::session::Session, path: Option<&Path>) -> Option<PathBuf> {
    let dir = resolve(&s.project, path).filter(|d| d.is_dir());
    #[cfg(any(test, feature = "test-util"))]
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

/// Whether `name` can name a file or folder: not empty, not only dots, none of the characters Windows refuses.
pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.chars().all(|c| c == '.') && !n.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh scratch folder for one test (removed again by the caller).
    pub fn scratch(tag: &str) -> PathBuf {
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

    /// A file of `n` bytes at `p` (a stand-in sample; nothing reads its sound here).
    fn file(p: &Path, n: usize) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, vec![7u8; n]).unwrap();
    }

}
