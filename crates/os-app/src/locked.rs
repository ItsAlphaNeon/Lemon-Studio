// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The open project behind a lock: anyone reads it, and a write has to ask for it with `get_mut`, which is
//! deprecated so every module that still writes the project directly says so with `#![allow(deprecated)]`.
//! The edits move to commands one area at a time; when the last module's allowance goes, `get_mut` goes too
//! (specs `roadmap/control-plane.md`, part 2, steps 3 and 14).

use os_model::Project;

pub struct Locked(Project);

impl Locked {
    pub fn new(project: Project) -> Self {
        Locked(project)
    }

    /// The project to change in place, past the commands.
    #[deprecated(note = "a direct write to the project: to be replaced by a command (control-plane steps 3 and 14)")]
    pub fn get_mut(&mut self) -> &mut Project {
        &mut self.0
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
        Locked(Project::default())
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
