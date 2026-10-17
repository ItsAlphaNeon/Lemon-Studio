// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Debug panel for the toolbar output monitor (F12, or right-click the monitor): one slider per
//! `monitor::PARAMS` entry, plus Reset / Save / Close. A tuning aid for matching FL by eye, not an FL feature.
//!
// spec: ui/monitor.md (debug panel)

use super::Ui;
use crate::app::{Action, State};
use crate::monitor::PARAMS;
use os_ui::{Color, Rect};

/// Whole panel (below the toolbar, right of the monitor's left edge).
pub const PANEL: Rect = Rect::new(880.0, 40.0, 330.0, 28.0 + 18.0 * PARAMS.len() as f32);
const ROW_Y: f32 = 62.0;

/// Slider track of parameter `i` (the drag maps x across it to the range).
pub fn slider(i: usize) -> Rect {
    Rect::new(PANEL.x + 128.0, ROW_Y + 18.0 * i as f32 + 5.0, 110.0, 8.0)
}

/// Header buttons: Reset, Save, Close.
pub fn button(n: usize) -> Rect {
    Rect::new(PANEL.x + PANEL.w - 150.0 + 50.0 * n as f32, PANEL.y + 4.0, 46.0, 16.0)
}

pub fn paint(ui: &mut Ui, s: &State) {
    let m = &s.monitor;
    let (bg, edge, ink, dim, fill) =
        (Color::rgb(0x2b2f32), Color::rgb(0x15181a), Color::rgb(0xd6dbde), Color::rgb(0x8a949b), Color::rgb(0xe0a040));
    ui.c.fill_rect(PANEL, edge);
    ui.c.fill_rect(Rect::new(PANEL.x + 1.0, PANEL.y + 1.0, PANEL.w - 2.0, PANEL.h - 2.0), bg);
    ui.hit(PANEL, Action::Swallow, "Monitor debug panel: drag a slider, right-click resets it");
    ui.c.text(super::fnt(13.0).bold(), PANEL.x + 8.0, PANEL.y + 4.0, ink, "Monitor debug");
    for (n, label) in ["Reset", "Save", "Close"].into_iter().enumerate() {
        let r = button(n);
        ui.c.fill_rect(r, Color::rgb(0x3d4347));
        let w = ui.c.text_width(super::fnt(12.0), label);
        ui.c.text(super::fnt(12.0), r.x + (r.w - w) / 2.0, r.y + 1.0, ink, label);
        let (a, hint) = match n {
            0 => (Action::MonReset, "Reset every monitor setting to its default"),
            1 => (Action::MonSave, "Save the settings to monitor.txt in the user data folder (loaded at start)"),
            _ => (Action::MonClose, "Close the debug panel (F12)"),
        };
        ui.hit(r, a, hint);
    }
    for (i, p) in PARAMS.iter().enumerate() {
        let y = ROW_Y + 18.0 * i as f32;
        let changed = m.cfg.v[i] != p.default;
        ui.c.text(super::fnt(12.0), PANEL.x + 8.0, y, if changed { fill } else { ink }, p.name);
        let t = slider(i);
        ui.c.fill_rect(t, Color::rgb(0x1d2022));
        let k = m.cfg.norm(i);
        ui.c.fill_rect(Rect::new(t.x, t.y, (t.w * k).round(), t.h), if changed { fill } else { dim });
        ui.c.text(super::fnt(12.0), t.x + t.w + 8.0, y, ink, &m.cfg.text(i));
        ui.hit(Rect::new(t.x - 4.0, y, t.w + 8.0, 18.0), Action::MonParam(i), p.name);
    }
}
