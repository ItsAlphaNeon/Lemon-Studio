// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The user folder files are kept in (control-plane step 9: the session's, moved out of `app`, which re-exports it).

use std::path::PathBuf;

/// The folder `current.flstheme` is kept in: the user folder; in tests this thread's folder, none unless one is set
/// (`THEME_DIR`).
pub(crate) fn theme_dir() -> Option<PathBuf> {
    #[cfg(test)]
    return THEME_DIR.with(|d| d.borrow().clone());
    #[cfg(not(test))]
    os_paths::user_dir()
}

#[cfg(test)]
thread_local! {
    /// Tests: the folder `theme_dir` gives.
    pub(crate) static THEME_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}
