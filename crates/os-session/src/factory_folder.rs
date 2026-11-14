// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The FL Studio factory content folder (Settings ▸ File): the FL Studio install folder that sample paths starting with
//! `%FLStudioFactoryData%` or `%FLStudioData%` point into. A program setting (`settings.txt` key `factory_folder`);
//! while it is not set, the install found on this computer is used (`SampleLibrary::default_factory_dir`: the folder
//! `OPENSTUDIO_FACTORY` names, else the default install location). Lemon Studio never ships FL Studio's samples: it
//! only reads them from the user's own installation.
//!
//! FL's Arpeggiator patterns and Sytrus presets are read from the same folder (`crate::arpeggiate`, `crate::citrus`).
//!
//! The group shows Reset (greyed while nothing is chosen), the folder button and the folder in use; the folder button
//! and the path open a folder picker. A change reaches the playing engines and the hosted Lemon Samplers at once.
// spec: flp/sample-paths.md "For Lemon Studio"

use std::path::PathBuf;


/// The setting's key in `settings.txt`.
pub const KEY: &str = "factory_folder";
/// The File page's group.
pub const GROUP: &str = "FL Studio factory content folder";
/// The Reset button and the folder button (with the path beside it), by their `SAct::Button` keys; Reset's label.
pub const RESET: &str = "Reset factory content folder";
pub const RESET_LABEL: &str = "Reset";
pub const SET_FOLDER: &str = "Set FL Studio factory content folder...";
/// What the path reads when no folder is chosen and no install is found.
pub const NONE_TEXT: &str = "(not found)";
/// The hints of Reset and of the folder button.
pub const RESET_HINT: &str = "Use the FL Studio installation found on this computer";
pub const FOLDER_HINT: &str = "The FL Studio installation folder that projects' factory samples are read from";
/// The folder picker's title.
pub const PICK_TITLE: &str = "Select a directory";

/// The folder the user chose, if any.
pub fn chosen() -> Option<PathBuf> {
    crate::settings_file::get(KEY).map(PathBuf::from)
}

/// The install folder in use: the chosen one, else the one found on this computer.
pub fn dir() -> Option<PathBuf> {
    chosen().or_else(os_engine::SampleLibrary::default_factory_dir)
}

/// The path shown beside the folder button: the folder in use with a trailing separator, or `(not found)`.
pub fn path_text() -> String {
    dir().map_or_else(|| NONE_TEXT.to_string(), |d| format!("{}{}", d.display(), std::path::MAIN_SEPARATOR))
}
