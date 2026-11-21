// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::control::host` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::collections::HashMap;
use os_model::Id;
use crate::app::App;
use crate::plugins::{fx_key, fx_of, FX_KEY};
#[allow(unused_imports)]
use crate::control::host::*;

impl App {
    /// See the module. `rows_too`: rack instances and the channels that came back (before anything looks at rows on the
    /// tick, so a sampler comes back from its kept state, not from the channel's settings); `fx_too`: mixer slot
    /// instances (after the mixer's own requests moved them, `mixer_tick`) and the slots that came back.
    pub fn follow_ids(&mut self, rows_too: bool, fx_too: bool) {
        let plan = follow_plan(&self.state.session, rows_too, fx_too);
        // Notes held from the keyboard end before their instruments move (`remap_rows`).
        if !plan.row_map.is_empty() {
            self.typing_release_all();
        }
        self.host(|s| follow_apply(s, plan));
    }
}

impl App {
    /// The plugins commands added (`HostReq`) are hosted, and the parameter values they set reach their instances.
    /// Unlike the plugin menus, nothing opens a window: the user didn't ask for one.
    pub fn host_commands(&mut self) {
        self.host(host_commands);
    }
}
