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

/// The views brought in line with what the session changed since `b`; whether the window has to be drawn again.
pub fn after(s: &mut State, b: Before) -> bool {
    let t = std::mem::take(&mut s.session.touched);
    let repaint = t.repaint;
    for v in t.views {
        view(s, v);
    }
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
    repaint
}

fn view(s: &mut State, v: crate::session::View) {
    use crate::session::View;
    let at = |map: &[Option<u16>], t: u16| map.get(usize::from(t)).copied().unwrap_or(Some(t));
    match v {
        View::Places { tracks, slots } => {
            let track = |t: u16| at(&tracks, t);
            let slot = |t: u16, k: u8| slots.get(&(t, k)).copied().unwrap_or(Some((t, k)));
            if let Some(t) = os_model::target::moved(s.events.target, &track, &slot) {
                s.events.target = t;
            }
        }
        View::MixerRenumbered(tracks) => {
            let m = |t: u16| at(&tracks, t).unwrap_or(t);
            s.selected_mixer = m(s.selected_mixer);
            s.mixer.sel = s.mixer.sel.iter().map(|&t| m(t)).collect();
        }
        View::MixerSelect(n) => {
            s.mixer.sel.clear();
            s.selected_mixer = n;
        }
        View::Rows(map) => crate::rack::remap_view(s, &map),
        View::RackAutosize => crate::rack::autosize(s),
        View::ClipsCleared => s.playlist.sel.clear(),
        View::Retimed(from, to) => crate::app::retime_views(s, from, to),
        View::Message(m) => s.message = Some(m),
        View::MemberLevel { t, k, id, wet, v } => crate::container::show_level(s, t, k, id, wet, v),
        View::PatternsRenumbered => {
            s.playlist.sel.clear();
            s.playlist.hover_clip = None;
            s.piano.clear_selection();
        }
    }
}

/// `f`, a command or an undo, with the views following it.
pub fn around<R>(s: &mut State, f: impl FnOnce(&mut State) -> R) -> R {
    let b = before(s);
    let r = f(s);
    // The input that led here draws the window again anyway (`App::host` asks for the hosting's own).
    let _ = after(s, b);
    r
}

/// The newest undo step holds the rack's side of adding a channel: the rack's view as it is now goes back with it.
pub fn keep_rack_view(s: &mut State) {
    let Some(id) = s.session.history.newest_id() else { return };
    let h = &s.session.history;
    s.rack_views.retain(|&k, _| h.has_step(k));
    s.rack_views.insert(id, RackView::now(s));
}
