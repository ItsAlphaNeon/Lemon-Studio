// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Debug panel for the Etched look (Ctrl+Shift+E on the Etched skin): one slider per `os_theme::etched::PARAMS`
//! entry, for the mode shown (dark or light), plus Reset / Save / Close. The panel is itself a frosted Etched panel,
//! so it shows what it tunes. A tuning aid, as views/cabledebug.rs is for the cables.
// spec: ui/etched-design.md §9

use super::Ui;
use crate::app::{Action, State};
use crate::cable::DebugAct;
use os_theme::etched::{appearance, tuning, Tuning, PARAMS};
use os_ui::kit;
use os_ui::Rect;

/// The groups of `PARAMS`: (heading, first index).
const GROUPS: [(&str, usize); 3] = [("Backdrop", 0), ("Menus and popups", 6), ("Surfaces", 12)];
const ROW: f32 = 22.0;
const TOP: f32 = 38.0;

/// Whole panel (below the toolbar, at the right).
pub const PANEL: Rect = Rect::new(850.0, 44.0, 380.0, TOP + ROW * (PARAMS.len() + GROUPS.len()) as f32 + 10.0);

/// Group of parameter `i`.
fn group(i: usize) -> usize {
    GROUPS.iter().rposition(|g| i >= g.1).unwrap_or(0)
}

/// Top of parameter `i`'s row.
fn row_y(i: usize) -> f32 {
    PANEL.y + TOP + ROW * (i + group(i) + 1) as f32
}

/// Slider track of parameter `i` (the drag maps x across it to the range).
pub fn slider(i: usize) -> Rect {
    Rect::new(PANEL.x + 136.0, row_y(i) + 4.0, 160.0, 14.0)
}

/// Header buttons: Reset, Save, Close.
fn button(n: usize) -> Rect {
    Rect::new(PANEL.right() - 186.0 + 60.0 * n as f32, PANEL.y + 8.0, 56.0, 24.0)
}

pub fn paint(ui: &mut Ui, _s: &State) {
    let t = ui.t;
    let dark = appearance().dark;
    let tu = tuning();
    kit::lemon::frosted(ui.c, t, PANEL, t.menu_pop);
    ui.hit(PANEL, Action::Swallow, "Etched look: drag a slider, right-click resets it");
    ui.c.text(super::fnt(15.0).bold(), PANEL.x + 14.0, PANEL.y + 11.0, t.menu_pop_text, "Etched look");
    let mode = if dark { "dark" } else { "light" };
    let tw = ui.c.text_width(super::fnt(15.0).bold(), "Etched look");
    ui.c.text(super::fnt(13.0), PANEL.x + 22.0 + tw, PANEL.y + 13.0, t.menu_pop_shortcut, mode);
    for (n, label) in ["Reset", "Save", "Close"].into_iter().enumerate() {
        let r = button(n);
        let (a, hint) = match n {
            0 => (DebugAct::Reset, "Reset every value of this mode to its default"),
            1 => (DebugAct::Save, "Save the values to etched.txt in the user data folder (loaded at start)"),
            _ => (DebugAct::Close, "Close the panel (Ctrl+Shift+E)"),
        };
        let hov = ui.hit(r, Action::EtchedDebug(a), hint);
        let down = ui.down(r, &Action::EtchedDebug(a));
        kit::button(ui.c, t, r, kit::State::new(hov, down, false));
        ui.c.text_center(super::fnt(13.0), r, t.menu_pop_text, label);
    }
    for (g, (heading, first)) in GROUPS.iter().enumerate() {
        let y = PANEL.y + TOP + ROW * (first + g) as f32;
        ui.c.text(super::fnt(13.0).bold(), PANEL.x + 14.0, y + 4.0, t.menu_pop_shortcut, heading);
    }
    for (i, p) in PARAMS.iter().enumerate() {
        let y = row_y(i);
        let changed = tu.get(dark, i) != Tuning::default_of(dark, i);
        // The heading says which; the row says what.
        let name = if i >= GROUPS[2].1 { p.name } else { p.name.split_once(' ').map_or(p.name, |(_, rest)| rest) };
        let name = format!("{}{}", name[..1].to_uppercase(), &name[1..]);
        ui.c.text(super::fnt(13.0), PANEL.x + 24.0, y + 4.0, if changed { t.accent } else { t.menu_pop_text }, &name);
        let track = slider(i);
        let hit = Rect::new(track.x - 8.0, y, track.w + 16.0, ROW);
        let hov = ui.hit(hit, Action::EtchedDebug(DebugAct::Param(i)), p.hint);
        let down = ui.down(hit, &Action::EtchedDebug(DebugAct::Param(i)));
        kit::etched::slider(ui.c, t, track, tu.norm(dark, i), kit::State::new(hov, down, false));
        ui.c.text(super::fnt(13.0), track.right() + 14.0, y + 4.0, if changed { t.accent } else { t.menu_pop_shortcut }, &tu.text(dark, i));
    }
}
