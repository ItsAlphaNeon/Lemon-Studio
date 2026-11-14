// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! FILE's project file rows (also TOOLS ▸ Delete unused files and save project): Revert to last backup, Save as
//! template..., New from template ▸, Save to new project folder... and Delete unused files and save project.
//!
//! Revert opens the newest backup of the project (autosaved or overwritten, by the time in its name) from the
//! project's backup folder and the global one, under the project's own name: the project file is not touched until the
//! next save. Lemon Studio has no factory templates: Save as template... writes a copy of the project into the
//! Templates folder of the user folder (`crate::templates`, category `User` unless a category folder was picked), and
//! New from template ▸ lists the templates there by category, then Empty. Save to new project folder... saves into a
//! new folder named after the project, in the folder picked, which becomes its data folder. Delete unused files looks
//! only in the data folder's Samples, Recorded and Rendered folders, and only when no other project file lies in the
//! data folder.
// spec: ui/project-templates.md, ui/menu-submenus.md §1, §4, ui/data-folder.md

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::path::{Path, PathBuf};



/// The category Save as template... writes to when no category folder was picked.
pub const USER_CATEGORY: &str = "User";
/// The data folder's folders Delete unused files looks in: the copied samples, the recordings and the renders.
pub const SCANNED: [&str; 3] = [crate::data_folder::SAMPLES_DIR, "Recorded", "Rendered"];
/// The names the delete question lists before "and N more".
pub(crate) const LISTED: usize = 10;

/// The newest backup of project `name` in folders `dirs`: an autosave or an "overwritten on" copy, by the time in its
/// name (an autosave wins a tie: it is the later state).
pub fn newest_backup(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let mut best: Option<((u32, u32, u32, u32, u32), bool, PathBuf)> = None;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let Some(file) = e.file_name().to_str().map(str::to_string) else { continue };
            let found = crate::backup::saved_at(&file, name, "autosaved").map(|t| (t, true)).or_else(|| crate::backup::saved_at(&file, name, "overwritten").map(|t| (t, false)));
            let Some((t, auto)) = found else { continue };
            if best.as_ref().is_none_or(|(bt, ba, _)| (t, auto) > (*bt, *ba)) {
                best = Some((t, auto, e.path()));
            }
        }
    }
    best.map(|(_, _, p)| p)
}

/// Two paths name the same file (Windows: whatever the case).
fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().replace('\\', "/");
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// The files under `dir`, every folder down.
fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => files_under(&p, out),
            Ok(t) if t.is_file() => out.push(p),
            _ => {}
        }
    }
}

/// The files of data folder `dir` the project does not use (`used`: the files it refers to): those in its Samples,
/// What Save writes: Lemon Studio's own project file (spec: flp/lemon-format.md).
pub const EXT: &str = "lemon";

/// The project files Lemon Studio opens: its own, and the FLPs it migrates from (spec: flp/lemon-format.md).
pub const PROJECT_EXTS: [&str; 2] = [EXT, "flp"];

/// Whether `p` is a project file by its extension (`.lemon` or `.flp`).
pub fn is_project(p: &Path) -> bool {
    p.extension().is_some_and(|e| PROJECT_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Whether `p` is an FLP: Save does not write over it but asks for a `.lemon` name (spec: flp/lemon-format.md
/// "Opening and saving").
pub fn is_flp(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("flp"))
}

/// `p` with the `.lemon` extension.
pub fn as_lemon(p: &Path) -> PathBuf {
    p.with_extension(EXT)
}

/// Recorded and Rendered folders, project files never. Sorted by path.
pub fn unused_files(dir: &Path, used: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for sub in SCANNED {
        files_under(&dir.join(sub), &mut found);
    }
    found.retain(|f| !is_project(f) && !used.iter().any(|u| same_file(f, u)));
    found.sort();
    found
}

/// Whether data folder `dir` holds a project file other than `own`: then it is not this project's alone, and Delete
/// unused files leaves it alone.
pub fn shared_folder(dir: &Path, own: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|r| {
        r.flatten().any(|e| is_project(&e.path()) && e.path().is_file() && !same_file(&e.path(), own))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("os_projfiles_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(m: u32, d: u32, h: u32, min: u32) -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, m, d).unwrap().and_hms_opt(h, min, 0).unwrap()
    }

}
