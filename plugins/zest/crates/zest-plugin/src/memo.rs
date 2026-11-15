// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the editor keeps in the plugin's state, kept with or without the editor (the `editor` feature), so a state
//! saved by one build opens the same in the other.

use serde::{Deserialize, Serialize};
use zest_ui::view;

/// What the editor keeps in the plugin's state: its size (logical px, the standalone editor's), page and preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Memo {
    pub w: u32,
    pub h: u32,
    pub page: usize,
    pub preset: Option<String>,
    /// The analog spread of the preset loaded (`spread_text`), so a reopened project sounds the same.
    #[serde(default)]
    pub spread: Option<String>,
}

impl Default for Memo {
    fn default() -> Self {
        Memo { w: view::BASE_W as u32, h: view::BASE_H as u32, page: 0, preset: None, spread: None }
    }
}
