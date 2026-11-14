// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::control::query` (step 9: the session's side stays there).

#![allow(unused_imports)]
use os_model::{ChannelKind, ClipSource, Project};
use serde::Serialize;
use serde_json::{json, Map, Value};
use crate::app::State;
use crate::control::command::color_text;
#[allow(unused_imports)]
use crate::control::query::*;

/// The project: settings, channels, mixer tracks in use, patterns and the playlist shown. Notes are read per pattern
/// (`notes`).
#[cfg_attr(not(test), allow(dead_code))] // the interface's own, used by tests
pub fn project(s: &State) -> Value {
    project_in(&s.session, s.selected_channel)
}
