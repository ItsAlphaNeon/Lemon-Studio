// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! PATTERNS > Quick render as audio clip, Render as audio clip... and Render and replace (spec: ui/pattern-render.md):
//! the current pattern rendered as the Export form renders a pattern, into a WAV in the data folder's Rendered folder,
//! then a new audio clip channel for it, named after the pattern; Render as audio clip... shows the Export form first;
//! Render and replace also puts a clip of it over every clip of the pattern in the playlist and mutes those.
// spec: ui/pattern-render.md

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::path::Path;



/// The folder the renders go to, in the data folder (§2).
pub const FOLDER: &str = "Rendered";
/// The warning box's title when there is nothing to render.
pub const TITLE: &str = "Render as audio clip";
/// The undo steps (§2, §3).
pub const QUICK: &str = "Render to audio clip";
pub const REPLACE: &str = "Render and replace";

/// A render under way: what to do with the file once it is written.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub pattern: u16,
    /// Render and replace: the pattern's clips get the audio.
    pub replace: bool,
}

/// The file name (without extension) for pattern `name` in `dir` (§2): the name with the characters a file name
/// cannot hold as `_`, `Pattern` for an empty one; ` #2`, ` #3`... when the WAV is already there (a render never
/// overwrites a file a channel may be playing).
pub fn file_stem(dir: &Path, name: &str) -> String {
    let safe: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let safe = safe.trim().trim_end_matches('.').trim_end().to_string();
    let safe = if safe.is_empty() { "Pattern".to_string() } else { safe };
    let free = |stem: &str| !dir.join(format!("{stem}.wav")).exists();
    if free(&safe) {
        return safe;
    }
    (2..).map(|k| format!("{safe} #{k}")).find(|n| free(n)).unwrap_or(safe)
}
