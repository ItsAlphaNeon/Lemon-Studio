// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Where Lemon Studio keeps its own files: settings, the remembered theme and design, presets, browser folders.
//!
//! One folder per user, shared by the application and its own plugins (a plugin runs in its own module and reads the
//! theme from here itself):
//!
//! - Windows: `%APPDATA%\Lemon Studio`
//! - macOS: `~/Library/Application Support/Lemon Studio`
//! - Linux and other Unix: `$XDG_CONFIG_HOME/Lemon Studio`, else `~/.config/Lemon Studio`
//!
//! `OPENSTUDIO_USER_DIR` overrides the folder on every platform. Targets without a file system (the web) have none.

use std::path::PathBuf;

/// The folder's own name inside the platform's per-user settings folder.
const APP: &str = "Lemon Studio";
/// Its name before the rename to Lemon Studio (`legacy_dir`).
const LEGACY: &str = "OpenStudio";

/// Lemon Studio's per-user folder, or `None` when the platform has none (or its variables are not set). It may not
/// exist yet: writers create it.
pub fn user_dir() -> Option<PathBuf> {
    if let Some(d) = var("OPENSTUDIO_USER_DIR") {
        return Some(d);
    }
    platform_dir().map(|d| d.join(APP))
}

/// The per-user folder from before the rename to Lemon Studio (`OpenStudio` in the same place), which the app moves to
/// [`user_dir`] once. `None` when `OPENSTUDIO_USER_DIR` chooses the folder.
pub fn legacy_dir() -> Option<PathBuf> {
    if var("OPENSTUDIO_USER_DIR").is_some() {
        return None;
    }
    platform_dir().map(|d| d.join(LEGACY))
}

/// A file or folder inside [`user_dir`].
pub fn user_file(name: &str) -> Option<PathBuf> {
    user_dir().map(|d| d.join(name))
}

/// A set, absolute environment variable (empty and relative values are ignored, as XDG asks).
fn var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute())
}

#[cfg(windows)]
fn platform_dir() -> Option<PathBuf> {
    var("APPDATA")
}

#[cfg(target_os = "macos")]
fn platform_dir() -> Option<PathBuf> {
    var("HOME").map(|h| h.join("Library").join("Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_dir() -> Option<PathBuf> {
    var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
}

#[cfg(not(any(windows, unix)))]
fn platform_dir() -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_is_named_after_the_app() {
        // Whatever the platform resolves to, the last part is ours (unless overridden).
        if std::env::var_os("OPENSTUDIO_USER_DIR").is_none() {
            if let Some(d) = user_dir() {
                assert_eq!(d.file_name().and_then(|n| n.to_str()), Some(APP));
                assert!(d.is_absolute());
            }
        }
        assert_eq!(user_file("monitor.txt").map(|f| f.file_name().unwrap().to_owned()), user_dir().map(|_| "monitor.txt".into()));
        // The old folder sits next to it.
        if let (Some(old), Some(new)) = (legacy_dir(), user_dir()) {
            assert_eq!(old.parent(), new.parent());
            assert_eq!(old.file_name().and_then(|n| n.to_str()), Some(LEGACY));
        }
    }
}
