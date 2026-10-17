// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Debug panel for the mixer's cable physics (Mixer options ▸ Cable physics): one slider per `cable::PARAMS` entry,
//! plus Reset / Save / Close. A tuning aid, not an FL feature (as views/mondebug.rs for the monitor).

use super::Ui;
use crate::app::{Action, State};
use crate::cable::{DebugAct, PARAMS};
use os_ui::{Color, Rect};

/// Whole panel (below the toolbar, at the right).
pub const PANEL: Rect = Rect::new(860.0, 40.0, 350.0, 28.0 + 18.0 * PARAMS.len() as f32 + 6.0);
const ROW_Y: f32 = 62.0;

/// Slider track of factor `i` (the drag maps x across it to the range).
pub fn slider(i: usize) -> Rect {
    Rect::new(PANEL.x + 108.0, ROW_Y + 18.0 * i as f32 + 5.0, 140.0, 8.0)
}

/// Header buttons: Reset, Save, Close.
fn button(n: usize) -> Rect {
    Rect::new(PANEL.x + PANEL.w - 150.0 + 50.0 * n as f32, PANEL.y + 4.0, 46.0, 16.0)
}

pub fn paint(ui: &mut Ui, s: &State) {
    let cables = s.cables.borrow();
    let tu = &cables.tuning;
    let (bg, edge, ink, dim, fill) =
        (Color::rgb(0x2b2f32), Color::rgb(0x15181a), Color::rgb(0xd6dbde), Color::rgb(0x8a949b), Color::rgb(0x8ec63a));
    ui.c.fill_rect(PANEL, edge);
    ui.c.fill_rect(PANEL.inset(1.0), bg);
    ui.hit(PANEL, Action::Swallow, "Cable physics debug panel: drag a slider, right-click resets it");
    ui.c.text(super::fnt(13.0).bold(), PANEL.x + 8.0, PANEL.y + 4.0, ink, "Cable physics");
    for (n, label) in ["Reset", "Save", "Close"].into_iter().enumerate() {
        let r = button(n);
        ui.c.fill_rect(r, Color::rgb(0x3d4347));
        let w = ui.c.text_width(super::fnt(12.0), label);
        ui.c.text(super::fnt(12.0), r.x + (r.w - w) / 2.0, r.y + 1.0, ink, label);
        let (a, hint) = match n {
            0 => (DebugAct::Reset, "Reset every cable factor to its default"),
            1 => (DebugAct::Save, "Save the factors to cables.txt in the user data folder (loaded at start)"),
            _ => (DebugAct::Close, "Close the cable physics panel"),
        };
        ui.hit(r, Action::CableDebug(a), hint);
    }
    for (i, p) in PARAMS.iter().enumerate() {
        let y = ROW_Y + 18.0 * i as f32;
        let changed = tu.v[i] != p.default;
        ui.c.text(super::fnt(12.0), PANEL.x + 8.0, y, if changed { fill } else { ink }, p.name);
        let t = slider(i);
        ui.c.fill_rect(t, Color::rgb(0x1d2022));
        ui.c.fill_rect(Rect::new(t.x, t.y, (t.w * tu.norm(i)).round(), t.h), if changed { fill } else { dim });
        ui.c.text(super::fnt(12.0), t.right() + 8.0, y, ink, &tu.text(i));
        ui.hit(Rect::new(t.x - 4.0, y, t.w + 8.0, 18.0), Action::CableDebug(DebugAct::Param(i)), p.hint);
    }
}
