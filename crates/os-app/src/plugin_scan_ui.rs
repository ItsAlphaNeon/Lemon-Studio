// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::plugin_scan` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use os_host::Format;
use crate::app::App;
use crate::plugin_db::{self as db, Cache, Mode, Record, Scanned, Status};
#[allow(unused_imports)]
use crate::plugin_scan::*;

impl App {
    /// Startup (spec: ui/plugin-manager.md §4 "When"): the built-in plugins and the cached plugins are listed at once
    /// (no plugin file is loaded), then a background scan starts when "Scan for new plugins at startup" is on (always
    /// when there is no cache yet).
    pub fn start_plugins(&mut self) {
        let first = !self.load_plugins();
        if first || self.state.plugdb.prefs.startup_scan {
            self.start_scan(Mode::Changed);
        }
    }

    /// The built-in plugins plus the last scan's, and the user's choices. False when there is no scan cache yet.
    pub fn load_plugins(&mut self) -> bool {
        let (owner, scale) = (self.state.session.plugins.owner, self.state.session.plugins.scale);
        self.state.session.plugins = crate::plugins::Plugins::builtins();
        (self.state.session.plugins.owner, self.state.session.plugins.scale) = (owner, scale);
        let db = &mut self.state.plugdb;
        db.prefs = db::Prefs::load();
        let cached = db::Cache::load();
        let found = cached.is_some();
        db.cache = cached.unwrap_or_default();
        self.state.session.plugins.merge(&self.state.plugdb.cache);
        self.state.plugdb.rebuild(&self.state.session.plugins);
        found
    }

    /// Starts a scan, or queues it after the running one (one at a time).
    pub fn start_scan(&mut self, mode: Mode) {
        if self.sweep.is_some() {
            let q = &mut self.scan_queued;
            *q = Some(if *q == Some(Mode::All) { Mode::All } else { mode });
            return;
        }
        let db = &mut self.state.plugdb;
        let first = db.cache.files.is_empty();
        db.progress = Some(db::Progress { first, ..db::Progress::default() });
        if first {
            self.state.message = Some("Looking for plugins...".into());
        }
        self.sweep = Some(start(&self.state.plugdb.prefs, self.state.plugdb.cache.clone(), mode));
        self.invalidate();
    }

    /// UI tick: the running scan's progress and results.
    pub fn scan_tick(&mut self) {
        let Some(sw) = &self.sweep else { return };
        let mut done = None;
        let mut moved = false;
        while let Ok(m) = sw.rx.try_recv() {
            match m {
                Msg::Progress { done, total, file } => {
                    if let Some(p) = &mut self.state.plugdb.progress {
                        (p.done, p.total, p.file) = (done, total, file);
                    }
                    moved = true;
                }
                Msg::Done(c) => done = Some(c),
            }
        }
        let Some(cache) = done else {
            if moved && self.state.settings.open {
                self.invalidate();
            }
            return;
        };
        self.sweep = None;
        self.apply_scan(cache);
        if let Some(mode) = self.scan_queued.take() {
            self.start_scan(mode);
        }
    }

    /// A scan ended: its results are kept, the lists updated in place, the hint bar says what changed.
    fn apply_scan(&mut self, cache: Cache) {
        let report = db::compare(&self.state.plugdb.cache, &cache);
        cache.save();
        self.state.session.plugins.merge(&cache);
        let db = &mut self.state.plugdb;
        db.cache = cache;
        db.progress = None;
        db.rebuild(&self.state.session.plugins);
        self.state.message = Some(report.message());
        self.invalidate();
    }

    /// The user changed a plugin choice: saved, and the lists follow.
    pub fn prefs_changed(&mut self) {
        self.state.plugdb.prefs.save();
        self.state.plugdb.rebuild(&self.state.session.plugins);
        self.invalidate();
    }
}
