// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What is dropped on the mixer: an effect slot copied by dragging the slot menu's "Save preset as..." (spec:
//! ui/mixer-selection.md §2) and a browser sample (§3). The track state opened on a selection is `load_state`.

use super::*;
use crate::app::{Action, Drag, Win};

/// The strip (`x`, `y`) is on, over the mixer, in the last painted frame: inside the strip itself (its controls
/// included), the part of a scrolled strip outside the strips area left out.
pub fn strip_at(app: &App, x: f32, y: f32) -> Option<u16> {
    if app.top_hit(x, y).and_then(|h| h.win) != Some(Win::Mixer) {
        return None;
    }
    app.hits.iter().rev().find_map(|h| match h.action {
        Action::Mix(MHit::Strip(n)) if h.rect.contains(x, y) => Some(n),
        _ => None,
    })
}

/// The slot row of the shown track under (`x`, `y`).
fn slot_at(app: &App, x: f32, y: f32) -> Option<u8> {
    match app.top_hit(x, y).map(|h| &h.action) {
        Some(Action::Mix(MHit::SlotName(k) | MHit::SlotArrow(k))) => Some(*k),
        _ => None,
    }
}

/// While the slot menu's "Save preset as..." is pressed (`armed`), the mouse leaving its row starts the drag of slot
/// `key`: the menu closes and the hint names the plugin (§2). True when it started (the press is then disarmed).
pub fn arm_move(app: &mut App, armed: Option<Action>) -> bool {
    let Some(Action::PlPopup(i)) = armed else { return false };
    let Some(item) = app.state.playlist.popup.as_ref().and_then(|p| p.items.get(i)) else { return false };
    let crate::playlist::Cmd::Preset(crate::presets::PCmd::SaveAs(key)) = item.cmd else { return false };
    let Some((t, k)) = crate::plugins::fx_of(key) else { return false };
    let (x, y) = app.mouse;
    if app.hits.iter().any(|h| h.action == Action::PlPopup(i) && h.rect.contains(x, y)) {
        return false;
    }
    let Some(p) = track(&app.state.session, t).and_then(|m| m.slots.get(usize::from(k))?.clone()) else { return false };
    app.state.playlist.popup = None;
    app.drag = Some(Drag::SlotCopy { track: t, slot: k });
    app.hint = std::borrow::Cow::Owned(format!("Dragging {}.fst", p.name));
    app.invalidate();
    true
}

/// The plugin list index and the state of slot `k` of track `t`: what a copy is made from.
fn source(s: &State, t: u16, k: u8) -> Option<(usize, Vec<u8>)> {
    let p = track(&s.session, t)?.slots.get(usize::from(k))?.clone()?;
    let index = s.session.plugins.effect_index(&p.name)?;
    let state = s.session.plugins.state(crate::plugins::fx_key(t, k)).unwrap_or(p.state);
    Some((index, state))
}

/// The dragged slot (`t`, `k`) released (§2): on a slot row of the shown track, a copy goes into that slot; on a
/// strip, into its first free slot, or into every selected track's when the strip is part of a multi-selection, the
/// strip then current. One undo step for them all, "Add effect" (a plugin it replaces goes in it too). The message.
pub fn drop_slot(app: &mut App, t: u16, k: u8) -> Option<String> {
    let (x, y) = app.mouse;
    let slot = slot_at(app, x, y);
    let strip = strip_at(app, x, y);
    let s = &mut app.state;
    let Some((index, state)) = source(s, t, k) else {
        return Some("This plugin can't be copied by dragging yet".into());
    };
    let places: Vec<(u16, u8)> = match (slot, strip) {
        (Some(to), _) => vec![(s.selected_mixer, to)],
        (None, Some(n)) => {
            let tracks = if in_group(s, n) { selected(s) } else { vec![n] };
            let free = |s: &State, t: u16| {
                track(&s.session, t).and_then(|m| m.slots.iter().position(Option::is_none)).map(|k| (t, k as u8))
            };
            let places: Vec<_> = tracks.iter().filter_map(|&t| free(s, t)).collect();
            select(s, n, in_group(s, n));
            places
        }
        (None, None) => return None,
    };
    if places.is_empty() {
        return Some("No free effect slot on this track".into());
    }
    let plugin = s.session.plugins.effects.get(index)?.desc.name.clone();
    track_ids(s);
    let mut c = Vec::new();
    for &(to, slot) in &places {
        let id = id_of(s, to)?;
        if track(&s.session, to).and_then(|m| m.slots.get(usize::from(slot))?.as_ref()).is_some() {
            // Its removal takes the slot's name and colour with it, and the undo brings them back.
            c.push(Command::RemoveEffect { track: id, slot });
        } else {
            forget_slot_ui(&mut s.session, to, slot);
        }
        c.push(Command::AddEffect { track: id, slot: Some(slot), plugin: plugin.clone() });
    }
    if let Err(e) = crate::control_ui::command(s, "Add effect", None, c) {
        return Some(e);
    }
    for (to, slot) in places {
        effect_made(s, to, slot, index, Some(state.clone()));
    }
    None
}

/// A browser sample dropped on strip `n` (§3): a new sampler channel at the end of the rack, named after the file,
/// playing into track `n`, which becomes the selected track. One undo step, "Load sample".
pub fn sample_to_strip(app: &mut App, path: String, n: u16) -> Option<String> {
    let s = &mut app.state;
    let stem = std::path::Path::new(&path).file_stem().map(|f| f.to_string_lossy().into_owned());
    let path = crate::data_folder_ui::adopt(s, &path);
    track_ids(s);
    let c = Command::AddChannel {
        name: stem.clone(),
        plugin: None,
        sample: Some(path),
        track: Some(id_of(s, n)?),
        kind: None,
        color: None,
        target: None,
        points: Vec::new(),
        children: Vec::new(),
    };
    let r = match crate::control_ui::command(s, crate::history::LOAD_SAMPLE, None, vec![c]) {
        Ok(r) => r,
        Err(e) => return Some(e),
    };
    let id = r.first().and_then(|v| v["channel"].as_u64()).unwrap_or(0) as u32;
    if let Some(row) = s.session.project.channels.iter().position(|c| c.id.0 == id) {
        s.selected_channel = row;
        s.rack.select_only(row);
    }
    select(s, n, false);
    s.session.dirty = true;
    Some(format!("{} loaded in Lemon Sampler, into {}", stem.unwrap_or_default(), name(&s.session, n)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::browser::Item;

    fn app() -> App {
        let mut a = App::new(None);
        a.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        a.state.session.plugins.effects = crate::plugins::builtin_effects();
        a.state.z = vec![Win::Mixer];
        a
    }

    fn paint(a: &mut App, c: &mut os_ui::Canvas) {
        a.invalidate();
        a.paint(c, None);
    }

    fn rect(a: &App, act: &Action) -> os_ui::Rect {
        a.hits.iter().rev().find(|h| &h.action == act).map(|h| h.rect).unwrap_or_else(|| panic!("no {act:?}"))
    }

    /// An effect in slot 0 of track 2, its saved state `[1, 2, 3]`; its plugin list index.
    fn effect_on_2(a: &mut App) -> usize {
        let name = a.state.session.plugins.effects[0].desc.name.clone();
        track_mut(&mut a.state.session, 2).unwrap().slots[0] =
            Some(os_model::SlotPlugin { id: os_model::Id::NONE, name, state: vec![1, 2, 3] });
        0
    }

    fn created(a: &App) -> Vec<(u16, u8, Option<Vec<u8>>)> {
        a.state
            .session
            .fx_requests
            .iter()
            .filter_map(|r| match r {
                FxReq::Create { track, slot, state, .. } => Some((*track, *slot, state.clone())),
                _ => None,
            })
            .collect()
    }

    /// The slot menu's "Save preset as..." pressed and dragged out of the menu becomes a slot drag; released on another
    /// strip, a copy with the slot's state goes into that track's first free slot, which becomes the current track
    /// (spec: ui/mixer-selection.md §2). One undo step.
    #[test]
    fn save_preset_as_dragged_onto_a_strip_copies_the_slot() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = app();
        let index = effect_on_2(&mut a);
        a.state.selected_mixer = 2;
        a.state.playlist.popup = Some(slot_menu(&a, 0, 300.0, 300.0));
        paint(&mut a, &mut c);
        let i = a
            .state
            .playlist
            .popup
            .as_ref()
            .unwrap()
            .items
            .iter()
            .position(|it| it.label == "Save preset as..." && !it.head)
            .unwrap();
        let row = rect(&a, &Action::PlPopup(i));
        a.mouse_move(row.cx(), row.cy());
        a.mouse_down();
        assert!(a.drag.is_none(), "a press alone does not drag");
        let strip = rect(&a, &Action::Mix(MHit::Strip(5)));
        let steps = a.state.session.history.at();
        a.mouse_move(strip.cx(), strip.cy());
        assert!(matches!(a.drag, Some(Drag::SlotCopy { track: 2, slot: 0 })));
        assert!(a.state.playlist.popup.is_none(), "the menu closes");
        assert!(a.hint.starts_with("Dragging ") && a.hint.ends_with(".fst"), "{}", a.hint);
        paint(&mut a, &mut c);
        a.mouse_up();
        assert!(a.drag.is_none());
        assert_eq!(created(&a), vec![(5, 0, Some(vec![1, 2, 3]))]);
        assert_eq!(
            a.state.session.fx_requests.iter().find_map(|r| match r {
                FxReq::Create { index, .. } => Some(*index),
                _ => None,
            }),
            Some(index)
        );
        assert!(track(&a.state.session, 5).unwrap().slots[0].is_some());
        assert_eq!(a.state.selected_mixer, 5);
        assert_eq!(a.state.session.history.at(), steps + 1);
        crate::history_ui::undo(&mut a.state);
        assert!(track(&a.state.session, 5).unwrap().slots[0].is_none(), "undone");
    }

    /// The plugin's name in slot `k` of track `t`.
    fn slot(a: &App, t: u16, k: usize) -> Option<String> {
        track(&a.state.session, t).unwrap().slots[k].as_ref().map(|p| p.name.clone())
    }

    /// Released on a slot row of the shown track, the copy goes into that slot, replacing what is there, in one undo
    /// step.
    #[test]
    fn slot_dropped_on_a_slot_row_goes_into_that_slot() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = app();
        effect_on_2(&mut a);
        let name = slot(&a, 2, 0);
        a.state.selected_mixer = 2;
        paint(&mut a, &mut c);
        let r = rect(&a, &Action::Mix(MHit::SlotName(4)));
        a.mouse = (r.cx(), r.cy());
        let steps = a.state.session.history.at();
        assert_eq!(drop_slot(&mut a, 2, 0), None);
        assert_eq!(created(&a), vec![(2, 4, Some(vec![1, 2, 3]))]);
        let copy = slot(&a, 2, 4);
        assert!(copy.is_some(), "the model holds the copy");
        assert_eq!(a.state.session.history.at(), steps + 1);
        // Again on the row, now holding another plugin: that one goes first, in the same one step.
        let other = a.state.session.plugins.effects[1].desc.name.clone();
        track_mut(&mut a.state.session, 2).unwrap().slots[4] =
            Some(os_model::SlotPlugin { id: os_model::Id::NONE, name: other.clone(), state: Vec::new() });
        a.state.session.fx_requests.clear();
        paint(&mut a, &mut c);
        a.mouse = (r.cx(), r.cy());
        assert_eq!(drop_slot(&mut a, 2, 0), None);
        assert!(
            a.state.session.fx_requests.contains(&FxReq::Delete { track: 2, slot: 4 }),
            "{:?}",
            a.state.session.fx_requests
        );
        assert_eq!(created(&a), vec![(2, 4, Some(vec![1, 2, 3]))]);
        assert_eq!(slot(&a, 2, 4), copy);
        assert_eq!(a.state.session.history.at(), steps + 2, "one step for the replacement");
        assert_eq!(a.state.session.history.newest_done(), Some("Add effect"));
        crate::history_ui::undo(&mut a.state);
        assert_eq!(slot(&a, 2, 4), Some(other), "one undo: the replaced plugin is back");
        crate::history_ui::undo(&mut a.state);
        assert_eq!(slot(&a, 2, 4), None);
        assert_eq!(slot(&a, 2, 0), name, "the source stays");
    }

    /// A strip takes the drop only inside itself: the mixer's title bar above a strip's column is no strip.
    #[test]
    fn strip_at_needs_the_point_inside_the_strip() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = app();
        paint(&mut a, &mut c);
        let r = rect(&a, &Action::Mix(MHit::Strip(5)));
        assert_eq!(strip_at(&a, r.cx(), r.cy()), Some(5));
        let above = r.y - 4.0;
        assert_eq!(a.top_hit(r.cx(), above).and_then(|h| h.win), Some(Win::Mixer), "still over the mixer");
        assert_eq!(strip_at(&a, r.cx(), above), None);
        // A strip scrolled part way under the master strip takes the pointer only where it shows.
        a.state.mixer_scroll = rect(&a, &Action::Mix(MHit::Strip(1))).w / 2.0;
        paint(&mut a, &mut c);
        let (s1, m) = (rect(&a, &Action::Mix(MHit::Strip(1))), rect(&a, &Action::Mix(MHit::Strip(0))));
        assert!(s1.x >= m.right() - 1.0, "clipped to the strips area: {s1:?} {m:?}");
    }

    /// Released on a strip of a multi-selection, every selected track gets a copy in its first free slot.
    #[test]
    fn slot_dropped_on_a_selection_goes_to_every_selected_track() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut a = app();
        effect_on_2(&mut a);
        track_mut(&mut a.state.session, 6).unwrap().slots[0] = track(&a.state.session, 2).unwrap().slots[0].clone();
        select(&mut a.state, 3, false);
        select(&mut a.state, 6, true);
        paint(&mut a, &mut c);
        let r = rect(&a, &Action::Mix(MHit::Strip(6)));
        a.mouse = (r.cx(), r.cy());
        let steps = a.state.session.history.at();
        assert_eq!(drop_slot(&mut a, 2, 0), None);
        let mut got = created(&a);
        got.sort();
        assert_eq!(got, vec![(3, 0, Some(vec![1, 2, 3])), (6, 1, Some(vec![1, 2, 3]))]);
        assert_eq!(selected(&a.state), vec![6, 3], "the selection stays, the strip dropped on first");
        assert_eq!(a.state.selected_mixer, 6);
        assert_eq!(a.state.session.history.at(), steps + 1, "one step for every copy");
        crate::history_ui::undo(&mut a.state);
        assert_eq!((slot(&a, 3, 0), slot(&a, 6, 1)), (None, None), "one undo takes both");
        assert!(slot(&a, 6, 0).is_some());
        // Off the mixer, nothing happens.
        a.state.session.fx_requests.clear();
        a.mouse = (-50.0, -50.0);
        assert_eq!(drop_slot(&mut a, 2, 0), None);
        assert!(created(&a).is_empty());
    }

    /// A browser sample released on a strip: a new sampler channel named after the file, routed to that track, which
    /// becomes current; one undo step (spec: ui/mixer-selection.md §3).
    #[test]
    fn sample_dropped_on_a_strip_makes_a_sampler_routed_there() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let dir = std::env::temp_dir().join(format!("os_slot_drag_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Grv Clap 03.wav");
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&p).unwrap(), 44100, &[0.0; 8]).unwrap();
        let path = p.to_string_lossy().into_owned();
        let mut a = app();
        paint(&mut a, &mut c);
        let r = rect(&a, &Action::Mix(MHit::Strip(4)));
        let (n, steps) = (a.state.session.project.channels.len(), a.state.session.history.at());
        a.drag = Some(Drag::Browser {
            item: Item::Sample(path.clone()),
            label: "Grv Clap 03".into(),
            m0: (0.0, 0.0),
            started: true,
        });
        a.mouse = (r.cx(), r.bottom() - 20.0);
        a.mouse_up();
        let chans = &a.state.session.project.channels;
        assert_eq!(chans.len(), n + 1, "{:?}", a.state.message);
        let ch = &chans[a.state.selected_channel];
        assert_eq!((ch.name.as_deref(), ch.mixer_track), (Some("Grv Clap 03"), 4));
        assert!(ch.kind.is_sampler());
        assert_eq!(a.state.selected_mixer, 4);
        assert_eq!(a.state.session.history.at(), steps + 1);
        assert!(a.state.message.as_deref().is_some_and(|m| m.contains("Grv Clap 03")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// File ▸ Open mixer track state on a track of a multi-selection loads it onto every selected track, in one undo
    /// step (spec: ui/mixer-selection.md §2); on a track outside it, onto that track only.
    #[test]
    fn track_state_opened_on_a_selection_goes_to_every_selected_track() {
        let dir = std::env::temp_dir().join(format!("os_state_sel_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = app();
        let path = dir.join("Lead.fst");
        {
            let m = track_mut(&mut a.state.session, 2).unwrap();
            m.volume = Some(0.25);
            std::fs::write(&path, crate::mixer_state::TrackState::of(m).to_json()).unwrap();
        }
        select(&mut a.state, 4, false);
        select(&mut a.state, 7, true);
        let steps = a.state.session.history.at();
        run(&mut a, MCmd::OpenState(7, path.clone()));
        let vol = |a: &App, n: u16| track(&a.state.session, n).unwrap().volume;
        assert_eq!((vol(&a, 4), vol(&a, 7)), (Some(0.25), Some(0.25)));
        assert_ne!(vol(&a, 5), Some(0.25));
        assert_eq!(a.state.session.history.at(), steps + 1, "one step");
        crate::history_ui::undo(&mut a.state);
        assert_ne!(vol(&a, 4), Some(0.25));
        assert_ne!(vol(&a, 7), Some(0.25));
        run(&mut a, MCmd::OpenState(9, path));
        assert_eq!((vol(&a, 9), vol(&a, 4)), (Some(0.25), vol(&a, 5)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
