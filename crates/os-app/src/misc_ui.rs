// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::misc` (step 9: the session's side stays there).

#![allow(unused_imports)]
use crate::misc_page::{self as mp};
use os_model::{Channel, ChannelMisc};
#[allow(unused_imports)]
use crate::misc::*;

/// A Misc tab edit of rack row `row`: the channel model, its hosted Lemon Sampler (root / fine), then playback.
/// An undo step; a slider's drag or wheel notches one step (`control::run_edit`).
pub fn set(app: &mut crate::app::App, row: usize, m: M, v: f32) {
    let s = &mut app.state;
    crate::mixer::track_ids(s);
    let Some(channel) = s.session.project.channels.get(row).map(|c| c.id.0) else { return };
    let c = crate::control::command::Command::SetMisc { channel, values: [(m.key(), v)].into() };
    let run = Some(crate::history::Run::Channel(row, crate::history::ChannelControl::Misc(m)));
    let done = if mp::draggable(m) { crate::control_ui::knob_command(s, "Channel settings", run, vec![c]) } else { crate::control_ui::command(s, "Channel settings", run, vec![c]) };
    if let Err(e) = done {
        s.message = Some(e);
    }
    app.invalidate();
}

/// Rack row `row`'s Lemon Sampler takes the Misc values it plays again (after an undo put them back).
pub fn follow(app: &mut crate::app::App, row: usize) {
    app.host(|s| follow_in(s, row));
}

/// The list menu of a Misc value (chord list, number boxes) at canvas point (x, y); the current value is ticked. A knob or
/// slider's menu is "Reset" (spec: ui/knobs.md "Owner decisions").
#[cfg(plugin_windows)]
pub fn menu(app: &crate::app::App, row: usize, m: M, x: f32, y: f32) -> crate::playlist::Popup {
    use crate::playlist::{Cmd, Mark, PItem, Popup};
    if mp::draggable(m) && !mp::list_on_right(m) {
        return Popup { x, y, items: vec![PItem::new("Reset", Cmd::Misc(row, m, mp::default_of(m)))], sub: None, scroll: [0; 2] };
    }
    let cur = app.state.session.project.channels.get(row).map_or(mp::default_of(m), |ch| get(&ch.misc, m));
    let items = mp::list_entries(m)
        .into_iter()
        .map(|(label, v)| PItem::new(label, Cmd::Misc(row, m, v)).mark(Mark::Radio((v - cur).abs() < 0.5)))
        .collect();
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Misc knob's or slider's right-click menu is Reset, to the page's default; a list keeps its values.
    #[cfg(plugin_windows)]
    #[test]
    fn knob_and_slider_menus_reset() {
        use crate::playlist::Cmd;
        let mut app = crate::app::App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        for m in [M::LevelPan, M::TimeGate, M::VolMid, M::Fine] {
            let menu = menu(&app, 0, m, 0.0, 0.0);
            assert_eq!(menu.items.len(), 1, "{m:?}");
            assert_eq!((menu.items[0].label.as_ref(), &menu.items[0].cmd), ("Reset", &Cmd::Misc(0, m, mp::default_of(m))), "{m:?}");
        }
        assert!(menu(&app, 0, M::PolyMax, 0.0, 0.0).items.len() > 1, "a number box's list");
        assert!(menu(&app, 0, M::ArpChord, 0.0, 0.0).items.len() > 1, "the chord list");
    }

}
