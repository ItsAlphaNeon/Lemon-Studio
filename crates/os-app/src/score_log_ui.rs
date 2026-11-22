// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::score_log` (step 9: the session's side stays there).

#![allow(unused_imports)]
use crate::app::State;
use crate::playlist::{Cmd, PItem};
use crate::record::RecCmd;
use crate::step_edit::Src;
use os_model::Note;
use std::time::{Duration, Instant};
#[allow(unused_imports)]
use crate::score_log::*;

/// TOOLS > Dump score log to selected pattern ▸.
pub fn items(_: &State) -> Vec<PItem> {
    MINUTES.iter().map(|&m| PItem::new(format!("Last {m} minutes"), Cmd::Rec(RecCmd::Dump(m)))).collect()
}

/// Ticks a second at the project's tempo.
pub fn ticks_per_second(s: &State) -> f64 {
    let p = &s.session.project;
    f64::from(p.ppq.max(1)) * p.tick_tempo(s.tempo().max(1.0)) / 60.0
}
