// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! After a run that ended without closing (a crash, a kill) while its project had unsaved changes: at the next start
//! Lemon Studio says so, and offers that project's newest autosave (`crate::backup`) when one was written after the
//! project was last opened or saved. Opening it works as FILE ▸ Revert to last backup: the project keeps its file.
//!
//! The run is read from its journal (`crate::journal`): a clean exit writes `closed` there, and the question once
//! asked writes `crash_seen`, so it is asked once.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use crate::app::App;
use crate::dialog::{Answer, Kind, Msg, Then};

/// A run that ended with unsaved changes: its project file (None: never saved), when its journal's last entry was
/// written and when the project was last opened, made or saved (Unix ms).
#[derive(Debug, Clone, PartialEq)]
pub struct Ended {
    pub project: Option<PathBuf>,
    pub at: u64,
    pub since: u64,
}

/// The run journal `entries` tell of, when it ended without closing and changed its project after it was last opened,
/// made new or saved (an undo step, an undo or redo, a change, a note).
pub fn unsaved_end(entries: &[Value]) -> Option<Ended> {
    let kind = |e: &Value| e["kind"].as_str().unwrap_or("").to_string();
    if entries.iter().any(|e| matches!(kind(e).as_str(), "closed" | "crash_seen")) {
        return None;
    }
    let base = entries.iter().rposition(|e| matches!(kind(e).as_str(), "opened" | "new" | "saved"))?;
    let edited = entries[base + 1..].iter().any(|e| matches!(kind(e).as_str(), "step" | "undo" | "redo" | "changes" | "notes"));
    if !edited {
        return None;
    }
    let t = |e: &Value| e["t"].as_u64().unwrap_or(0);
    let project = entries[base]["path"].as_str().map(PathBuf::from);
    Some(Ended { project, at: entries.last().map_or(0, t), since: t(&entries[base]) })
}

/// The entries of journal file `path`.
fn read(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()).unwrap_or_default()
}

/// Whether process `pid` still runs (another Lemon Studio, whose journal is still being written).
fn running(pid: u64) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "lemonstudio")
}

/// The newest autosave of `project` (None: an untitled one) written after `since` (Unix ms): in the project's folder's
/// `Backup` (a data folder's) and the global backup folder.
fn backup_after(project: Option<&Path>, since: u64) -> Option<PathBuf> {
    // A project opened converted has no path while it plays: its backups are an untitled project's.
    let names = [crate::backup::project_name(project), crate::backup::project_name(None)];
    let dirs: Vec<PathBuf> = project.and_then(Path::parent).map(|d| d.join(crate::data_folder::BACKUP_DIR)).into_iter().chain(crate::backup::folder()).collect();
    let written = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
    dirs.iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .filter(|e| e.file_name().to_str().is_some_and(|f| names.iter().any(|n| crate::backup::saved_at(f, n, "autosaved").is_some())))
        .map(|e| (written(&e.path()), e.path()))
        .filter(|(t, _)| *t > since)
        .max()
        .map(|(_, p)| p)
}

/// Unix ms as local `HH:MM`, with the date when not today.
fn clock(ms: u64) -> String {
    use chrono::TimeZone;
    let Some(t) = chrono::Local.timestamp_millis_opt(ms as i64).single() else { return String::new() };
    if t.date_naive() == chrono::Local::now().date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%-m-%-d-%Y %H:%M").to_string()
    }
}

impl App {
    /// At startup, after the first project opened: the run before this one, when it ended with unsaved changes.
    pub(crate) fn crash_notice(&mut self) {
        let Some(prev) = self.journal.previous.clone() else { return };
        let entries = read(&prev);
        let pid = entries.iter().find(|e| e["kind"] == "session").and_then(|e| e["pid"].as_u64());
        if pid.is_some_and(running) {
            return;
        }
        let Some(ended) = unsaved_end(&entries) else { return };
        // Asked once.
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&prev) {
            use std::io::Write as _;
            let _ = writeln!(f, "{}", json!({ "kind": "crash_seen", "t": std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64) }));
        }
        let name = crate::backup::project_name(ended.project.as_deref());
        let head = |s: &mut Vec<(String, bool)>| {
            s.extend([("Lemon Studio ended at ".into(), false), (clock(ended.at), false), (" with unsaved changes to ".into(), false), (name.clone(), true)]);
        };
        let mut text = Vec::new();
        head(&mut text);
        match backup_after(ended.project.as_deref(), ended.since) {
            Some(b) => {
                let when = std::fs::metadata(&b).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
                text.extend([(".\nOpen its backup from ".into(), false), (clock(when), false), ("?".into(), false)]);
                self.show_msg(Msg::new(Kind::Question, "Restore backup", text, &[Answer::Yes, Answer::No], Then::CrashBackup(ended.project, b)));
            }
            None => {
                text.push((".\nNo backup was written after it was last saved.".into(), false));
                self.tell(Kind::Info, "Restore backup", text);
            }
        }
    }

    /// The crash notice's Yes: backup `b` opened as `project` (None: as itself, for a project never saved).
    pub(crate) fn open_crash_backup(&mut self, project: Option<PathBuf>, b: PathBuf) {
        match project.filter(|p| p.is_file()) {
            Some(p) => {
                if self.state.path.as_deref() != Some(p.as_path()) {
                    self.open(p.clone());
                }
                if self.state.path.as_deref() == Some(p.as_path()) {
                    self.revert_from(b);
                } else {
                    // Opened converted, without its path: the backup as itself.
                    self.open(b);
                }
            }
            None => {
                self.open(b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: &str, t: u64, path: Option<&str>) -> Value {
        let mut v = json!({ "kind": kind, "t": t });
        if let Some(p) = path {
            v["path"] = json!(p);
        }
        v
    }

    #[test]
    fn a_run_that_ended_with_changes_after_its_last_save() {
        let opened = e("opened", 10, Some("/p/song.lemon"));
        let run = [e("session", 1, None), opened.clone(), e("step", 20, None), e("changes", 21, None), e("plugin_state", 30, None)];
        assert_eq!(unsaved_end(&run), Some(Ended { project: Some(PathBuf::from("/p/song.lemon")), at: 30, since: 10 }));
        // Closed, or already asked about: nothing.
        let mut closed = run.to_vec();
        closed.push(e("closed", 40, None));
        assert_eq!(unsaved_end(&closed), None);
        let mut seen = run.to_vec();
        seen.push(e("crash_seen", 50, None));
        assert_eq!(unsaved_end(&seen), None);
        // Saved after the last change, or only a plugin read since opening: nothing unsaved.
        let saved = [opened.clone(), e("step", 20, None), e("saved", 25, Some("/p/song.lemon")), e("plugin_state", 26, None)];
        assert_eq!(unsaved_end(&saved), None);
        // A copy written (a backup) is no save.
        let copied = [opened, e("step", 20, None), e("copied", 25, Some("/b/x.lemon"))];
        assert_eq!(unsaved_end(&copied).map(|x| x.since), Some(10));
        // A new project never saved.
        let new = [e("new", 5, None), e("notes", 6, None)];
        assert_eq!(unsaved_end(&new), Some(Ended { project: None, at: 6, since: 5 }));
    }

    #[test]
    fn the_backup_is_the_newest_autosave_after_the_save() {
        let dir = std::env::temp_dir().join(format!("lemon-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let backup = dir.join(crate::data_folder::BACKUP_DIR);
        std::fs::create_dir_all(&backup).unwrap();
        let song = dir.join("song.lemon");
        std::fs::write(&song, b"x").unwrap();
        let at = |h| chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap().and_hms_opt(h, 0, 0).unwrap();
        let old = backup.join(crate::backup::file_name("song", "autosaved", at(1)));
        let new = backup.join(crate::backup::file_name("song", "autosaved", at(2))).with_extension("lemon");
        std::fs::write(&old, b"x").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&new, b"x").unwrap();
        std::fs::write(backup.join(crate::backup::file_name("song", "overwritten", at(3))), b"x").unwrap();
        let ms = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        assert_eq!(backup_after(Some(&song), 0), Some(new.clone()), "the newest autosave, a .lemon one too; not an overwritten copy");
        assert_eq!(backup_after(Some(&song), ms(&new)), None, "none written after the save");
        // A project that opened converted backs up as untitled.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let untitled = backup.join(crate::backup::file_name("untitled", "autosaved", at(4))).with_extension("lemon");
        std::fs::write(&untitled, b"x").unwrap();
        assert_eq!(backup_after(Some(&song), ms(&new)), Some(untitled));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
