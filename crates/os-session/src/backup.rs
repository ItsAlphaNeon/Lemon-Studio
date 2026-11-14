// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Project backups (settings window, File ▸ Backup). While a project is open the interval of the Autosave choice runs;
//! when it ends and the project changed since the last backup or save, the project is written to
//! `<user folder>/Projects/Backup` as `<name> (autosaved on M-D-YYYY at HhMM).flp`, and the oldest autosaves of that
//! name beyond Maximum are deleted. Saving over a file first copies the old file there as
//! `<name> (overwritten on M-D-YYYY at HhMM).flp`. With Location's "project data folder" (the default), a project with a
//! data folder backs up to `<data folder>/Backup` instead (`crate::data_folder::backup_folder`). Not yet: Never's
//! reminder and the backups "before risky operations".
// spec: ui/settings-choices.md "Backup files", ui/data-folder.md §3

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{Datelike, NaiveDateTime, Timelike};
use os_model::Project; // This code is a part of L e m o n S t u d i o.

use crate::prefs;

/// The Autosave timer and what the last backup wrote.
#[derive(Debug, Default)]
pub struct Backup {
    /// The running interval (None: Never) and when it ends.
    every: Option<Duration>,
    due: Option<Instant>,
    /// The project and tempo of the last backup.
    last: Option<(Project, f64)>,
}

/// The interval of Autosave entry `i`: Never none, Rarely 15, Occasionally 10, Regularly 5, Frequently 5, Very
/// frequently 1 minute.
pub fn interval(i: usize) -> Option<Duration> {
    const MINUTES: [u64; 6] = [0, 15, 10, 5, 5, 1];
    MINUTES.get(i).copied().filter(|&m| m > 0).map(|m| Duration::from_secs(m * 60))
}

/// The global backup folder: `<user folder>/Projects/Backup` (a project with a data folder may back up there instead:
/// `crate::data_folder::backup_folder`). Tests: the folder their thread set in `TEST_DIR`, else none (nothing is
/// written).
pub fn folder() -> Option<PathBuf> {
    #[cfg(any(test, feature = "test-util"))]
    return TEST_DIR.with(|d| d.borrow().clone());
    #[cfg(not(any(test, feature = "test-util")))]
    os_paths::user_dir().map(|d| d.join("Projects").join("Backup"))
}

#[cfg(any(test, feature = "test-util"))]
thread_local! {
    pub static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// The time stamp in backup names, the reference's format (spec: ui/settings-choices.md "File"): `M-D-YYYY at HhMM`,
/// 24-hour clock, no leading zeros but the minutes'.
pub fn stamp(t: NaiveDateTime) -> String {
    format!("{}-{}-{} at {}h{:02}", t.month(), t.day(), t.year(), t.hour(), t.minute())
}

/// A backup's file name: `<name> (<what> on <stamp>).lemon`; `what` is "autosaved" or "overwritten" (FL's names,
/// Lemon Studio's file: spec flp/lemon-format.md "Backups").
pub fn file_name(name: &str, what: &str, t: NaiveDateTime) -> String {
    format!("{name} ({what} on {}).{}", stamp(t), crate::project_files::EXT)
}

/// The name backups give the project: its file's, or `untitled` for a project never saved.
pub fn project_name(path: Option<&Path>) -> String {
    path.and_then(Path::file_stem).map_or_else(|| "untitled".to_string(), |n| n.to_string_lossy().into_owned())
}

/// When file `file` was autosaved, if it is an autosave of `name`: (year, month, day, hour, minute).
pub fn autosaved_at(file: &str, name: &str) -> Option<(u32, u32, u32, u32, u32)> {
    saved_at(file, name, "autosaved")
}

/// When backup `file` of `name` was written, if it is one of kind `what` ("autosaved" or "overwritten"): (year,
/// month, day, hour, minute).
pub fn saved_at(file: &str, name: &str, what: &str) -> Option<(u32, u32, u32, u32, u32)> {
    // Older backups are FLPs.
    let rest = file.strip_prefix(name)?.strip_prefix(" (")?.strip_prefix(what)?.strip_prefix(" on ")?;
    let rest = rest.strip_suffix(").lemon").or_else(|| rest.strip_suffix(").flp"))?;
    let (date, time) = rest.split_once(" at ")?;
    let mut d = date.split('-').map(str::parse::<u32>);
    let (Some(Ok(m)), Some(Ok(day)), Some(Ok(y)), None) = (d.next(), d.next(), d.next(), d.next()) else { return None };
    let (h, min) = time.split_once('h')?;
    Some((y, m, day, h.parse().ok()?, min.parse().ok()?))
}

/// Deletes the oldest autosaves of `name` in `dir` beyond `max` (by the time in their names). Other projects' backups
/// and "overwritten on" copies are left alone.
pub fn prune(dir: &Path, name: &str, max: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut found: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let at = autosaved_at(e.file_name().to_str()?, name)?;
            Some((at, e.path()))
        })
        .collect();
    if found.len() > max {
        found.sort();
        for (_, p) in &found[..found.len() - max] {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Before the project is saved over `target`: an existing file there is copied to the backup folder `dir` as
/// `<name> (overwritten on ...).flp` (a copy of that name made earlier in the same minute is kept).
pub fn keep_overwritten(target: &Path, dir: Option<PathBuf>) {
    let Some(dir) = dir.filter(|_| target.is_file()) else { return };
    let to = dir.join(file_name(&project_name(Some(target)), "overwritten", chrono::Local::now().naive_local()));
    if !to.exists() && std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::copy(target, to);
    }
}

/// Whether the Autosave timer ran out (it follows the interval set: Never stops it).
pub fn due(b: &mut Backup, now: Instant) -> bool {
    let every = interval(prefs::index(&prefs::AUTOSAVE));
    if every != b.every {
        b.every = every;
        b.due = every.map(|e| now + e);
    }
    b.due.is_some_and(|due| now >= due)
}

/// The timer starts again after a backup (or after finding none was needed).
pub fn rearm(b: &mut Backup, now: Instant) {
    if let Some(every) = b.every {
        b.due = Some(now + every);
    }
}

/// Whether the project changed since it was last backed up; `unsaved`: since it was last saved (or opened, or made).
pub fn changed_since(ses: &crate::session::Session, unsaved: bool) -> bool {
    unsaved && ses.backup.last.as_ref().is_none_or(|(p, t)| *p != ses.project || *t != ses.tempo())
}

/// Where a backup named for local time `t` goes (`place`).
pub enum Place {
    /// Write it there; the project and tempo it backs up.
    Write(PathBuf, (Project, f64)),
    /// One was written there in the same minute already: not overwritten.
    Done(PathBuf),
}

/// Where the backup named for local time `t` goes, its folder made.
pub fn place(ses: &crate::session::Session, t: NaiveDateTime) -> Result<Place, String> {
    let dir = crate::data_folder::backup_folder(ses, ses.path.as_deref()).ok_or_else(|| "no user folder".to_string())?;
    let name = project_name(ses.path.as_deref());
    let target = dir.join(file_name(&name, "autosaved", t));
    if target.exists() {
        return Ok(Place::Done(target));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(Place::Write(target, (ses.project.clone(), ses.tempo())))
}

/// A backup of `snap` was written to `target`: it is the last one, and the oldest autosaves beyond Maximum go.
pub fn wrote(ses: &mut crate::session::Session, target: &Path, snap: (Project, f64)) {
    ses.backup.last = Some(snap);
    if let Some(dir) = target.parent() {
        prune(dir, &project_name(ses.path.as_deref()), usize::from(prefs::number(&prefs::BACKUP_MAXIMUM)));
    }
}

/// The Autosave timer of a session without an interface (`--serve`): as `App::backup_tick`, the copy written by the
/// session (`project_io::copy`). Returns what failed.
pub fn tick_headless(ses: &mut crate::session::Session, now: Instant) -> Option<String> {
    if !due(&mut ses.backup, now) {
        return None;
    }
    let mut failed = None;
    if ses.loading.is_some() || ses.export.is_some() {
        return None;
    }
    if changed_since(ses, crate::project_io::unsaved(ses, &crate::lemon_state::Looks::none())) {
        match place(ses, chrono::Local::now().naive_local()) {
            Ok(Place::Write(target, snap)) => match crate::project_io::copy(ses, &target) {
                Ok(()) => wrote(ses, &target, snap),
                Err(e) => failed = Some(format!("Backup failed: {e}")),
            },
            Ok(Place::Done(_)) => {}
            Err(e) => failed = Some(format!("Backup failed: {e}")),
        }
    }
    rearm(&mut ses.backup, now);
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, m, d).unwrap().and_hms_opt(h, min, 30).unwrap()
    }

    /// A fresh folder for this test's backups (this thread's `folder`).
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("os_backup_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.clone()));
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).map(|r| r.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn names_follow_fl() {
        assert_eq!(file_name("gs7", "autosaved", at(10, 15, 0, 13)), "gs7 (autosaved on 10-15-2026 at 0h13).lemon");
        assert_eq!(file_name("untitled", "overwritten", at(3, 4, 18, 5)), "untitled (overwritten on 3-4-2026 at 18h05).lemon");
        assert_eq!(project_name(None), "untitled");
        assert_eq!(project_name(Some(Path::new("/music/My song.flp"))), "My song");
        assert_eq!(autosaved_at("gs7 (autosaved on 10-15-2026 at 0h13).lemon", "gs7"), Some((2026, 10, 15, 0, 13)));
        assert_eq!(autosaved_at("gs7 (autosaved on 10-15-2026 at 0h13).flp", "gs7"), Some((2026, 10, 15, 0, 13)), "an older backup");
        for other in ["gs7 (overwritten on 10-15-2026 at 0h13).flp", "gs70 (autosaved on 10-15-2026 at 0h13).flp", "gs7.flp"] {
            assert_eq!(autosaved_at(other, "gs7"), None, "{other}");
        }
        assert_eq!(interval(2), Some(Duration::from_secs(600)), "Occasionally");
        assert_eq!((interval(0), interval(5), interval(6)), (None, Some(Duration::from_secs(60)), None));
    }

    /// The oldest autosaves of the project's name go; other names and "overwritten on" copies stay.
    #[test]
    fn prune_keeps_the_newest() {
        let dir = scratch("prune");
        std::fs::create_dir_all(&dir).unwrap();
        let files = [
            file_name("a", "autosaved", at(9, 30, 23, 59)),
            file_name("a", "autosaved", at(10, 1, 0, 0)),
            file_name("a", "autosaved", at(10, 1, 9, 5)),
            file_name("a", "autosaved", at(10, 1, 10, 0)),
            file_name("a", "overwritten", at(1, 1, 0, 0)),
            file_name("ab", "autosaved", at(1, 1, 0, 0)),
        ];
        for f in &files {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        prune(&dir, "a", 2);
        let mut want = vec![files[2].clone(), files[3].clone(), files[4].clone(), files[5].clone()];
        want.sort();
        assert_eq!(names(&dir), want);
        let _ = std::fs::remove_dir_all(&dir);
    }

}
