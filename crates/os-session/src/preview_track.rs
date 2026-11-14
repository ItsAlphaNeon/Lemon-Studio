// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio ▸ Mixer ▸ Browser preview track (`prefs::BROWSER_PREVIEW_TRACK`): the browser's previews enter the open
//! project's mixer at the chosen insert track, as a channel routed there would, and reach the master through that
//! track's routing, so its effects, volume and pan apply. "---" (the default) and a track the open project lacks play
//! the preview straight into a master of its own, as before.
//!
//! The preview plays on its own stream (`Transport::preview_for`), which hosts no plugin instance (one instance must
//! not be processed by two streams): the effects the engine runs itself apply, hosted effect plugins in the slots on
//! the way are passed by, dry. The preview stream lights no mixer meter.
// spec: ui/settings-choices.md "Mixer: Browser preview track"

use os_model::Project;

/// Routes the preview project `p` (`app::preview_project`) into insert `track` of the open project `open`: `p` takes
/// `open`'s mixer and panning law, and its channels play into `track`. 0 ("---") or a track `open` lacks leaves `p` as
/// it is.
pub fn route(p: &mut Project, open: &Project, track: u16) {
    if !has_insert(open, track) {
        return;
    }
    p.mixer = open.mixer.clone();
    p.panning_law = open.panning_law;
    for c in &mut p.channels {
        c.mixer_track = track;
    }
}

/// Whether `open` has insert track `n`: one of the inserts its mixer shows (not the master, not the hidden last
/// track; `mixer::inserts`), stored at its own place (the engine finds a channel's track by place).
fn has_insert(open: &Project, n: u16) -> bool {
    n >= 1 && usize::from(n) + 2 <= open.mixer.len() && open.mixer.get(usize::from(n)).is_some_and(|m| m.index == n)
}

