// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio


//! The interface's views follow the session: after a command or an undo, what the session says it changed
//! (`session::Touched`) brings the selections and windows back in line with the project, and what an undo step puts
//! back in the interface is kept here, filed under the step (specs `roadmap/control-plane.md`, part 2, step 6).

use crate::app::State;
use std::collections::BTreeSet;

/// The rack's selection and the playlist's clip source as they were around an undo step that adds a channel (ADD >
/// Audio clip / Automation clip: spec ui/add-clip-channels.md §3); an undo or redo of it puts them back.
#[derive(Debug, Clone)]
pub struct RackView {
    sel: BTreeSet<usize>,
    selected: usize,
    source: Option<os_model::ClipSource>,
    picker: crate::playlist::PickKind,
}

impl RackView {
    fn now(s: &State) -> RackView {
        RackView { sel: s.rack.sel.clone(), selected: s.selected_channel, source: s.playlist.source, picker: s.playlist.picker }
    }

    fn put(self, s: &mut State) {
        (s.rack.sel, s.selected_channel) = (self.sel, self.selected);
        (s.playlist.source, s.playlist.picker) = (self.source, self.picker);
    }
}

/// The views before a change, by id where the change can move things.
pub struct Before {
    clips: Vec<u32>,
    rack: RackView,
}

pub fn before(s: &State) -> Before {
    let clips = crate::playlist::clips(&s.session);
    Before { clips: s.playlist.sel.iter().filter_map(|&i| clips.get(i).map(|c| c.id.0)).collect(), rack: RackView::now(s) }
}

/// The views brought in line with what the session changed since `b`.
pub fn after(s: &mut State, b: Before) {
    let t = std::mem::take(&mut s.session.touched);
    let whole = t.rows.is_some();
    if let Some(old) = t.rows {
        let row: std::collections::HashMap<u32, usize> = s.session.project.channels.iter().enumerate().map(|(r, c)| (c.id.0, r)).collect();
        let map: Vec<Option<usize>> = old.iter().map(|id| row.get(id).copied()).collect();
        crate::rack::remap_view(s, &map);
    }
    if t.tracks {
        let tracks = crate::mixer::inserts(&s.session);
        s.selected_mixer = s.selected_mixer.min(tracks);
        s.mixer.sel.retain(|&t| t <= tracks);
    }
    if whole || t.clips {
        let clips = crate::playlist::clips(&s.session);
        s.playlist.sel = clips.iter().enumerate().filter(|(_, c)| b.clips.contains(&c.id.0)).map(|(i, _)| i).collect();
        s.playlist.hover_clip = None;
    }
    if t.notes {
        s.piano.clear_selection();
    }
    // The step's rack view goes back, and the one before it is kept for the step's way back.
    if let Some(id) = t.rack_step {
        if let Some(v) = s.rack_views.insert(id, b.rack) {
            v.put(s);
        }
    }
}

/// `f`, a command or an undo, with the views following it.
pub fn around<R>(s: &mut State, f: impl FnOnce(&mut State) -> R) -> R {
    let b = before(s);
    let r = f(s);
    after(s, b);
    r
}

/// The newest undo step holds the rack's side of adding a channel: the rack's view as it is now goes back with it.
pub fn keep_rack_view(s: &mut State) {
    let Some(id) = s.session.history.newest_id() else { return };
    let h = &s.session.history;
    s.rack_views.retain(|&k, _| h.has_step(k));
    s.rack_views.insert(id, RackView::now(s));
}
