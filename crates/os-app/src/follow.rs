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
    pub fn now(s: &State) -> RackView {
        RackView { sel: s.rack.sel.clone(), selected: s.selected_channel, source: s.playlist.source, picker: s.playlist.picker }
    }

    pub fn put(self, s: &mut State) {
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
    let clips = crate::edit::playlist::clips(&s.session);
    Before { clips: s.playlist.sel.iter().filter_map(|&i| clips.get(i).map(|c| c.id.0)).collect(), rack: RackView::now(s) }
}

/// The views brought in line with what the session changed since `b`; whether the window has to be drawn again.
pub fn after(s: &mut State, b: Before) -> bool {
    let t = std::mem::take(&mut s.session.touched);
    let repaint = t.repaint;
    for v in t.views {
        view(s, v);
    }
    if t.tracks {
        let tracks = crate::edit::mixer::inserts(&s.session);
        s.selected_mixer = s.selected_mixer.min(tracks);
        s.mixer.sel.retain(|&t| t <= tracks);
    }
    if t.clips {
        let clips = crate::edit::playlist::clips(&s.session);
        s.playlist.sel = clips.iter().enumerate().filter(|(_, c)| b.clips.contains(&c.id.0)).map(|(i, _)| i).collect();
        s.playlist.hover_clip = None;
    }
    if t.notes {
        s.piano.clear_selection();
    }
    // The step's rack view goes back, and the one before it is kept for the step's way back.
    if let Some(id) = t.rack_step.filter(|id| s.rack_views.contains_key(id)) {
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
            // The mixer's selection follows its tracks; a selected track that went leaves track 1 selected.
            s.selected_mixer = track(s.selected_mixer).unwrap_or(1);
            s.mixer.sel = s.mixer.sel.iter().filter_map(|&t| track(t)).collect();
            if let Some(t) = os_model::target::moved(s.events.target, &track, &slot) {
                s.events.target = t;
            }
            // "Most recent" parameters, and Multilink's controls waiting for their controller, follow what they drive.
            crate::params_ui::remap(&mut s.tweaks, |key| match os_session::plugins::fx_of(key) {
                Some((t, k)) => slot(t, k).map(|(t, k)| os_session::plugins::fx_key(t, k)),
                None => Some(key),
            });
            s.multilink.queue.retain_mut(|l| os_session::edit::mixer::moved_link(*l, &track, &slot).map(|n| *l = n).is_some());
            // The container window follows its slot, and the palette the track a container's pick goes to; a place that
            // goes closes them.
            let (t, k) = s.container.at;
            if k != crate::container::CHANNEL {
                match slot(t, k) {
                    Some(at) => s.container.at = at,
                    None => s.hide(crate::app::Win::Container),
                }
            }
            if let Some(t) = s.palette.track {
                match track(t) {
                    Some(n) => s.palette.track = Some(n),
                    None => s.hide(crate::app::Win::PluginPalette),
                }
            }
        }
        View::MixerSelect(n) => {
            s.mixer.sel.clear();
            s.selected_mixer = n;
        }
        View::Rows(map) => {
            crate::rack::remap_view(s, &map);
            let m = |r: usize| map.get(r).copied().flatten();
            crate::params_ui::remap(&mut s.tweaks, |key| if key >= os_session::plugins::FX_KEY { Some(key) } else { m(key) });
            s.multilink.queue.retain_mut(|l| os_session::edit::rack::moved_link(*l, &m).map(|n| *l = n).is_some());
        }
        View::RackAutosize => crate::rack::autosize(s),
        View::ClipsCleared => s.playlist.sel.clear(),
        View::TargetsMoved(moved) => {
            if let Some(Some(n)) = moved.target(s.events.target) {
                s.events.target = n;
            }
        }
        View::Retimed(from, to) => crate::app::retime_views(s, from, to),
        View::Message(m) => s.message = Some(m),
        View::AudioClipAdded { channel, clip } => {
            s.playlist.source = Some(os_model::ClipSource::Channel(channel));
            s.playlist.picker = crate::playlist::PickKind::Audio;
            s.playlist.sel = vec![clip];
        }
        View::Recorded { pattern, channel } => {
            let range = s.piano.range;
            s.piano.select(pattern, channel, Vec::new());
            s.piano.range = range;
        }
        View::MemberLevel { t, k, id, wet, v } => crate::container_ui::show_level(s, t, k, id, wet, v),
        View::PatternsMoved(map) => {
            // The playlist's Draw source and its clip brush follow their pattern; a pattern that goes takes them.
            let follow = |c: os_model::ClipSource| match c {
                os_model::ClipSource::Pattern(n) => map.get(usize::from(n)).copied().unwrap_or(Some(n)).map(os_model::ClipSource::Pattern),
                c => Some(c),
            };
            s.playlist.source = s.playlist.source.and_then(follow);
            s.playlist.brush = s.playlist.brush.and_then(|c| Some(os_model::Clip { source: follow(c.source)?, ..c }));
        }
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
