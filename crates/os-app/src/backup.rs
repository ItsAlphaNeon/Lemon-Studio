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

use crate::app::App;
use crate::prefs;

/// Maximum: autosaves kept per project name (its value box is greyed out: value boxes do not work yet).
pub const MAXIMUM: usize = 100;

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
    #[cfg(test)]
    return TEST_DIR.with(|d| d.borrow().clone());
    #[cfg(not(test))]
    os_paths::user_dir().map(|d| d.join("Projects").join("Backup"))
}

#[cfg(test)]
thread_local! {
    pub static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// FL's time stamp in backup names: `M-D-YYYY at HhMM`, 24-hour clock, no leading zeros but the minutes'.
pub fn stamp(t: NaiveDateTime) -> String {
    format!("{}-{}-{} at {}h{:02}", t.month(), t.day(), t.year(), t.hour(), t.minute())
}

/// A backup's file name: `<name> (<what> on <stamp>).flp`; `what` is "autosaved" or "overwritten".
pub fn file_name(name: &str, what: &str, t: NaiveDateTime) -> String {
    format!("{name} ({what} on {}).flp", stamp(t))
}

/// The name backups give the project: its file's, or `untitled` for a project never saved.
pub fn project_name(path: Option<&Path>) -> String {
    path.and_then(Path::file_stem).map_or_else(|| "untitled".to_string(), |n| n.to_string_lossy().into_owned())
}

/// When file `file` was autosaved, if it is an autosave of `name`: (year, month, day, hour, minute).
fn autosaved_at(file: &str, name: &str) -> Option<(u32, u32, u32, u32, u32)> {
    let rest = file.strip_prefix(name)?.strip_prefix(" (autosaved on ")?.strip_suffix(").flp")?;
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

impl App {
    /// The Autosave timer (`App::tick`): when its interval ends and the project changed since the last backup or save,
    /// a backup is written. A new Autosave choice starts the timer again.
    pub(crate) fn backup_tick(&mut self, now: Instant) {
        let every = interval(prefs::index(&prefs::AUTOSAVE));
        if every != self.backup.every {
            self.backup.every = every;
            self.backup.due = every.map(|e| now + e);
        }
        let (Some(every), Some(due)) = (every, self.backup.due) else { return };
        if now < due {
            return;
        }
        if self.changed_since_backup() {
            match self.write_backup(chrono::Local::now().naive_local()) {
                // A project loading or an export rendering: at a later tick.
                None => return,
                Some(Err(e)) => {
                    self.state.message = Some(format!("Backup failed: {e}"));
                    self.invalidate();
                }
                Some(Ok(_)) => {}
            }
        }
        self.backup.due = Some(now + every);
    }

    /// Whether the project changed since it was last backed up, and since it was last saved (or opened, or created).
    pub(crate) fn changed_since_backup(&self) -> bool {
        self.unsaved() && self.backup.last.as_ref().is_none_or(|(p, t)| *p != self.state.project || *t != self.state.tempo)
    }

    /// Writes a backup of the project as it is now, named for local time `t`, then deletes its oldest autosaves beyond
    /// `MAXIMUM`. Returns the file (one already written in the same minute is not overwritten); None while the project
    /// cannot be written now.
    pub(crate) fn write_backup(&mut self, t: NaiveDateTime) -> Option<Result<PathBuf, String>> {
        let Some(dir) = crate::data_folder::backup_folder(&self.state, self.state.path.as_deref()) else { return Some(Err("no user folder".into())) };
        let name = project_name(self.state.path.as_deref());
        let target = dir.join(file_name(&name, "autosaved", t));
        if target.exists() {
            return Some(Ok(target));
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return Some(Err(format!("{}: {e}", dir.display())));
        }
        let snap = (self.state.project.clone(), self.state.tempo);
        let done = self.save_copy(target.clone())?;
        if done.is_ok() {
            self.backup.last = Some(snap);
            prune(&dir, &name, MAXIMUM);
        }
        Some(done.map(|()| target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::{self, SAct};

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
        assert_eq!(file_name("gs7", "autosaved", at(10, 15, 0, 13)), "gs7 (autosaved on 10-15-2026 at 0h13).flp");
        assert_eq!(file_name("untitled", "overwritten", at(3, 4, 18, 5)), "untitled (overwritten on 3-4-2026 at 18h05).flp");
        assert_eq!(project_name(None), "untitled");
        assert_eq!(project_name(Some(Path::new("/music/My song.flp"))), "My song");
        assert_eq!(autosaved_at("gs7 (autosaved on 10-15-2026 at 0h13).flp", "gs7"), Some((2026, 10, 15, 0, 13)));
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

    /// After the interval, a changed project is backed up once; unchanged or just saved, it is not. A backup is not a
    /// save: the project keeps its name, its unsaved changes and the message.
    #[test]
    fn autosave_writes_changed_projects() {
        let dir = scratch("timer");
        let mut app = App::new(None);
        prefs::set_choice(&prefs::AUTOSAVE, 5);
        let t0 = Instant::now();
        let minute = Duration::from_secs(61);
        app.backup_tick(t0);
        app.backup_tick(t0 + minute);
        assert!(names(&dir).is_empty(), "nothing changed");
        app.state.tempo += 7.0;
        app.state.message = Some("hello".into());
        app.backup_tick(t0 + minute + Duration::from_secs(30));
        assert!(names(&dir).is_empty(), "the interval runs on");
        app.backup_tick(t0 + 2 * minute);
        let files = names(&dir);
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(autosaved_at(&files[0], "untitled").is_some(), "{files:?}");
        assert!(app.unsaved() && app.state.path.is_none() && app.state.message.as_deref() == Some("hello"), "a copy, not a save");
        let opened = App::new(Some(dir.join(&files[0])));
        assert_eq!(opened.state.tempo, app.state.tempo, "the backup holds the change");
        // Backed up already: the next interval writes nothing.
        assert!(!app.changed_since_backup());
        app.backup_tick(t0 + 3 * minute);
        assert_eq!(names(&dir).len(), 1);
        // Never: no timer.
        prefs::set_choice(&prefs::AUTOSAVE, 0);
        app.state.tempo += 1.0;
        app.backup_tick(t0 + 4 * minute);
        app.backup_tick(t0 + 9 * minute);
        assert_eq!(names(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Backups are named for their time; past `MAXIMUM` the oldest go. Saving over a file keeps the old one.
    #[test]
    fn backups_and_overwritten_copies() {
        let dir = scratch("files");
        let mut app = App::new(None);
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let song = work.join("song.flp");
        app.save_to(song.clone());
        assert!(song.is_file());
        assert_eq!(names(&dir), ["work"], "a new file: nothing saved over");
        app.state.tempo = 99.0;
        app.save_to(song.clone());
        let over: Vec<String> = names(&dir).into_iter().filter(|n| n.starts_with("song (overwritten on ")).collect();
        assert_eq!(over.len(), 1, "{:?}", names(&dir));
        assert_ne!(std::fs::read(dir.join(&over[0])).unwrap(), std::fs::read(&song).unwrap(), "the file as it was");
        // Autosaves of "song", newest kept.
        for i in 0..MAXIMUM as u32 + 2 {
            app.state.tempo = 100.0 + f64::from(i);
            let t = at(10, 1 + i / 60, 12, i % 60);
            assert_eq!(app.write_backup(t), Some(Ok(dir.join(file_name("song", "autosaved", t)))));
        }
        let auto: Vec<String> = names(&dir).into_iter().filter(|n| autosaved_at(n, "song").is_some()).collect();
        assert_eq!(auto.len(), MAXIMUM);
        assert!(!auto.contains(&file_name("song", "autosaved", at(10, 1, 12, 0))) && !auto.contains(&file_name("song", "autosaved", at(10, 1, 12, 1))));
        assert!(auto.contains(&file_name("song", "autosaved", at(10, 2, 12, 41))), "the newest");
        assert_eq!(app.state.path.as_deref(), Some(song.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// File ▸ Backup: Autosave and Location are live combos; Maximum stays greyed out (no value boxes yet).
    #[test]
    fn the_file_page() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::FILE);
        app.paint(&mut c, None);
        for c in [&prefs::AUTOSAVE, &prefs::BACKUP_LOCATION] {
            assert!(app.hits.iter().any(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == c.label)), "{}", c.label);
        }
        assert!(app.hits.iter().any(|h| h.action == Action::Placeholder("Maximum")));
    }
}
