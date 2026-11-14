// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::backup` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use chrono::{Datelike, NaiveDateTime, Timelike};
use os_model::Project; // This code is a part of L e m o n S t u d i o.
use crate::app::App;
use crate::prefs;
#[allow(unused_imports)]
use crate::backup::*;

impl App {
    /// The Autosave timer (spec: ui/settings-choices.md "Backup files"): when it runs out, a project changed since it was
    /// saved and since its last backup is backed up (`write_backup`).
    pub(crate) fn backup_tick(&mut self, now: Instant) {
        if !due(&self.state.session.settings, &mut self.state.session.backup, now) {
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
        rearm(&mut self.state.session.backup, now);
    }

    /// Whether the project changed since it was last backed up, and since it was last saved (or opened, or created).
    pub(crate) fn changed_since_backup(&self) -> bool {
        changed_since(&self.state.session, self.unsaved())
    }

    /// Writes a backup of the project as it is now, named for local time `t`, then deletes its oldest autosaves beyond
    /// Maximum (`prefs::BACKUP_MAXIMUM`). Returns the file (one already written in the same minute is not
    /// overwritten); None while the project cannot be written now.
    pub(crate) fn write_backup(&mut self, t: NaiveDateTime) -> Option<Result<PathBuf, String>> {
        let (target, snap) = match place(&self.state.session, t) {
            Ok(Place::Write(target, snap)) => (target, snap),
            Ok(Place::Done(target)) => return Some(Ok(target)),
            Err(e) => return Some(Err(e)),
        };
        let done = self.save_copy(target.clone())?;
        if done.is_ok() {
            wrote(&mut self.state.session, &target, snap);
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

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).map(|r| r.flatten().filter_map(|e| e.file_name().into_string().ok()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    /// A fresh folder for this test's backups (this thread's `folder`).
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("os_backup_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.clone()));
        dir
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
        app.state.set_tempo(app.state.tempo() + 7.0);
        app.state.message = Some("hello".into());
        app.backup_tick(t0 + minute + Duration::from_secs(30));
        assert!(names(&dir).is_empty(), "the interval runs on");
        app.backup_tick(t0 + 2 * minute);
        let files = names(&dir);
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(autosaved_at(&files[0], "untitled").is_some(), "{files:?}");
        assert!(app.unsaved() && app.state.session.path.is_none() && app.state.message.as_deref() == Some("hello"), "a copy, not a save");
        let opened = App::new(Some(dir.join(&files[0])));
        assert_eq!(opened.state.tempo(), app.state.tempo(), "the backup holds the change");
        // Backed up already: the next interval writes nothing.
        assert!(!app.changed_since_backup());
        app.backup_tick(t0 + 3 * minute);
        assert_eq!(names(&dir).len(), 1);
        // Never: no timer.
        prefs::set_choice(&prefs::AUTOSAVE, 0);
        app.state.set_tempo(app.state.tempo() + 1.0);
        app.backup_tick(t0 + 4 * minute);
        app.backup_tick(t0 + 9 * minute);
        assert_eq!(names(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Backups are named for their time; past Maximum the oldest go. Saving over a file keeps the old one.
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
        app.state.set_tempo(99.0);
        app.save_to(song.clone());
        let over: Vec<String> = names(&dir).into_iter().filter(|n| n.starts_with("song (overwritten on ")).collect();
        assert_eq!(over.len(), 1, "{:?}", names(&dir));
        assert_ne!(std::fs::read(dir.join(&over[0])).unwrap(), std::fs::read(&song).unwrap(), "the file as it was");
        // Autosaves of "song", newest kept: 100 by default, here 5.
        assert_eq!(prefs::number(&prefs::BACKUP_MAXIMUM), 100);
        prefs::set_number(&prefs::BACKUP_MAXIMUM, 5);
        for i in 0..7 {
            app.state.set_tempo(100.0 + f64::from(i));
            let t = at(10, 1 + i / 60, 12, i % 60);
            assert_eq!(app.write_backup(t), Some(Ok(dir.join(file_name("song", "autosaved", t)))));
        }
        let auto: Vec<String> = names(&dir).into_iter().filter(|n| autosaved_at(n, "song").is_some()).collect();
        assert_eq!(auto.len(), 5);
        assert!(!auto.contains(&file_name("song", "autosaved", at(10, 1, 12, 0))) && !auto.contains(&file_name("song", "autosaved", at(10, 1, 12, 1))));
        assert!(auto.contains(&file_name("song", "autosaved", at(10, 1, 12, 6))), "the newest");
        // Lowered, nothing goes until the next backup.
        prefs::set_number(&prefs::BACKUP_MAXIMUM, 2);
        assert_eq!(names(&dir).into_iter().filter(|n| autosaved_at(n, "song").is_some()).count(), 5);
        app.state.set_tempo(1.0);
        app.write_backup(at(10, 1, 13, 0)).unwrap().unwrap();
        let auto: Vec<String> = names(&dir).into_iter().filter(|n| autosaved_at(n, "song").is_some()).collect();
        assert_eq!(auto, [file_name("song", "autosaved", at(10, 1, 12, 6)), file_name("song", "autosaved", at(10, 1, 13, 0))]);
        prefs::set_number(&prefs::BACKUP_MAXIMUM, prefs::BACKUP_MAXIMUM.default);
        assert_eq!(app.state.session.path.as_deref(), Some(song.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// File ▸ Backup: Autosave and Location are live combos, Maximum a live value box the wheel moves within 1 to 999.
    #[test]
    fn the_file_page() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::FILE);
        app.paint(&mut c, None);
        for c in [&prefs::AUTOSAVE, &prefs::BACKUP_LOCATION] {
            assert!(app.hits.iter().any(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == c.label)), "{}", c.label);
        }
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder("Maximum")));
        let r = app.hits.iter().find(|h| h.action == Action::Settings(SAct::Number(prefs::BACKUP_MAXIMUM.label))).expect("the Maximum box").rect;
        app.mouse_move(r.cx(), r.cy());
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(prefs::number(&prefs::BACKUP_MAXIMUM), 99);
        assert_eq!(app.state.message.as_deref(), Some("Maximum: 99"));
        prefs::set_number(&prefs::BACKUP_MAXIMUM, 1);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(prefs::number(&prefs::BACKUP_MAXIMUM), 1, "at least one");
        prefs::set_number(&prefs::BACKUP_MAXIMUM, 999);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(prefs::number(&prefs::BACKUP_MAXIMUM), 999, "at most 999");
        prefs::set_number(&prefs::BACKUP_MAXIMUM, prefs::BACKUP_MAXIMUM.default);
        prefs::set_number(&prefs::BACKUP_MAXIMUM, 42);
        let shown = settings::page(settings::FILE, &app.state).into_iter().flat_map(|g| g.rows).any(|r| match r {
            settings::Row::Pair(_, b) => matches!(*b, settings::Row::ValueR("Maximum", ref v) if v == "42"),
            _ => false,
        });
        assert!(shown, "the page shows the value");
        prefs::set_number(&prefs::BACKUP_MAXIMUM, prefs::BACKUP_MAXIMUM.default);
    }
}
