// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The open project behind a lock: anyone reads it, and a write has to ask for it with `get_mut`, which is
//! deprecated so every module that still writes the project directly says so with `#![allow(deprecated)]`.
//! The edits move to commands one area at a time; when the last module's allowance goes, `get_mut` goes too
//! (specs `roadmap/control-plane.md`, part 2, steps 3 and 14).

use os_model::Project;

pub struct Locked(Project, Option<&'static std::panic::Location<'static>>);

impl Locked {
    pub fn new(project: Project) -> Self {
        Locked(project, None)
    }

    /// The project to change in place, past the commands. Marks it written, from where (`take_written`).
    #[deprecated(note = "a direct write to the project: to be replaced by a command (control-plane steps 3 and 14)")]
    #[track_caller]
    pub fn get_mut(&mut self) -> &mut Project {
        self.1.get_or_insert(std::panic::Location::caller());
        &mut self.0
    }

    /// `f` changes the project in place and says whether it changed anything; only then is the project written.
    #[deprecated(note = "a direct write to the project: to be replaced by a command (control-plane steps 3 and 14)")]
    #[track_caller]
    pub fn write_if(&mut self, f: impl FnOnce(&mut Project) -> bool) -> bool {
        let changed = f(&mut self.0);
        if changed {
            self.1.get_or_insert(std::panic::Location::caller());
        }
        changed
    }

    /// Where the project was first written since the last `take_written`, if it was (left as it is).
    pub fn written_at(&self) -> Option<&'static std::panic::Location<'static>> {
        self.1
    }

    /// The lock on `project` put back as it was, written at `at` (`written_at`): for what changes the project only to
    /// take it back (a dry run, a saved copy).
    pub fn restored(project: Project, at: Option<&'static std::panic::Location<'static>>) -> Self {
        Locked(project, at)
    }

    /// The mark put back as it was (`written_at`) after a change that was taken back.
    pub fn rewind_written(&mut self, at: Option<&'static std::panic::Location<'static>>) {
        self.1 = at;
    }

    /// Where the project was first written since the last call, if it was.
    pub fn take_written(&mut self) -> Option<&'static std::panic::Location<'static>> {
        self.1.take()
    }
}

impl std::ops::Deref for Locked {
    type Target = Project;
    fn deref(&self) -> &Project {
        &self.0
    }
}

/// Shown as the project itself: the lock is not part of what the project is.
impl std::fmt::Debug for Locked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl Default for Locked {
    fn default() -> Self {
        Locked(Project::default(), None)
    }
}

impl PartialEq<Project> for Locked {
    fn eq(&self, other: &Project) -> bool {
        self.0 == *other
    }
}

impl PartialEq<Locked> for Project {
    fn eq(&self, other: &Locked) -> bool {
        *self == other.0
    }
}
