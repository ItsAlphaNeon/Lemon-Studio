// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The container's window (Lemon Studio's design, the plain list window of the Patcher's slice 1): the effects (and
//! a channel's container's instruments) in order, each with its name (a click opens its window), its wet and dry knobs
//! and its menu button; Add effect (Add plugin on a channel) under them.
// spec: plugins/patcher/README.md §4 slice 1, §4.3, §4.4

use os_ui::kit::lemon as lk;
use os_ui::{Font, Rect};

use super::{FrameInfo, Look};
use crate::app::{Action, State, Ui, Win};
use crate::container::{level_text, member_count, members, CAct};
use crate::icons::{self, Icon};
use crate::knobs::KnobId;

pub const W: f32 = 330.0;
const TITLE_H: f32 = 30.0;
/// The knobs' captions, then the rows from here.
const TOP: f32 = 48.0;
pub const ROW_H: f32 = 34.0;
/// Under the rows: Add effect and the margin.
const FOOT: f32 = 40.0;
/// The knobs' centres and the menu button, from the window's right edge.
const WET_X: f32 = 108.0;
const DRY_X: f32 = 68.0;
const MENU_X: f32 = 34.0;

/// The window's height for `n` members (an empty list keeps one row for "(empty)").
fn height(n: usize) -> f32 {
    TOP + ROW_H * n.max(1) as f32 + FOOT
}

pub fn rect(s: &State) -> Rect {
    let (t, k) = s.container.at;
    let n = member_count(s, t, k).unwrap_or(0);
    Rect::new(s.container.pos.0, s.container.pos.1, W, height(n))
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let (x0, y0) = (win.x, win.y);
    let lemon = super::lemon(ui);
    let (bg, line) = (t.s(0x2f3335), t.s(0x16191a));
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), line, bg);
    } else {
        ui.c.fill_rect(win, line);
        ui.c.fill_rect(win.inset(1.0), bg);
    }
    ui.hit(win, Action::Swallow, "");
    let (track, slot) = s.container.at;
    let title = crate::container::title(s, track, slot).unwrap_or_else(|| "Container".into());
    ui.hit(Rect::new(x0 + 1.0, y0 + 1.0, W - 30.0, TITLE_H - 4.0), Action::WinDrag(Win::Container), "");
    super::strum::label(ui, Font::new(17), x0 + 9.0, y0 + 15.0, -1, t.text, &title);
    let close = Rect::new(x0 + W - 20.0, y0 + 10.0, 10.0, 10.0);
    let hov = ui.hit(close.outset(4.0), Action::CloseWin(Win::Container), "Close");
    icons::draw(ui.c, Icon::Close, close.outset(1.0), if hov { t.tx(0xffffff) } else { t.glyph });

    let small = Font::new(9);
    let lf = Font::new(15);
    let (wx, dx, mx) = (x0 + W - WET_X, x0 + W - DRY_X, x0 + W - MENU_X);
    super::strum::label(ui, small, wx, y0 + TOP - 8.0, 0, t.text_dim, "WET");
    super::strum::label(ui, small, dx, y0 + TOP - 8.0, 0, t.text_dim, "DRY");
    let list = members(s, track, slot).unwrap_or_default();
    if list.is_empty() {
        super::strum::label(ui, lf, x0 + 12.0, y0 + TOP + ROW_H / 2.0, -1, t.text_faint, "(empty)");
    }
    for (i, m) in list.iter().enumerate() {
        let row = Rect::new(x0 + 4.0, y0 + TOP + ROW_H * i as f32, W - 8.0, ROW_H);
        if i > 0 {
            ui.c.fill_rect(Rect::new(row.x, row.y, row.w, 1.0), t.s(0x272a2c));
        }
        // The name: a click opens the effect's window; a member whose plugin was not found is greyed.
        let found = crate::container::member_key(s, crate::container::place_key(track, slot), m.id).is_some();
        let name_r = Rect::new(row.x, row.y, row.w - WET_X - 18.0, row.h);
        let hint = if found { format!("Open {}'s window (right-click: its menu)", m.name) } else { format!("{} was not found (right-click: its menu)", m.name) };
        let hov = ui.hit(name_r, Action::Container(CAct::Open(i)), hint);
        let ink = if !found { t.text_faint } else if hov { t.tx(0xffffff) } else { t.text };
        super::strum::label(ui, lf, row.x + 6.0, row.cy(), -1, t.text_dim, &format!("{}.", i + 1));
        let ty = (row.cy() - ui.c.line_height(lf) / 2.0).round();
        ui.c.text_clip(lf, row.x + 26.0, ty, name_r.w - 28.0, ink, &m.name);
        // The wet and dry knobs, at the levels the chain plays.
        for (cx, wet) in [(wx, true), (dx, false)] {
            let id = KnobId::Member(track, slot, m.id, wet);
            let v = s.knob(id);
            ui.hit(Rect::new(cx - 15.0, row.cy() - 15.0, 30.0, 30.0), Action::Knob(id), level_text(&m.name, wet, v));
            super::knob(ui, cx, row.cy(), 11.0, v, false, t.text, t.text_dim);
        }
        // Its menu: Open editor, the levels' parameter menus, Move up, Move down, Remove.
        let b = Rect::new(mx - 12.0, row.cy() - 11.0, 24.0, 22.0);
        let hov = ui.hit(b, Action::Container(CAct::Menu(i)), "Menu: open, levels, move, remove");
        super::button_face(ui, b, if hov { Look::Hover } else { Look::Off });
        icons::draw(ui.c, Icon::ArrowRight, b.inset(5.0), if hov { t.tx(0xffffff) } else { t.glyph });
    }
    // Add effect: the effect list of an empty slot.
    let add = Rect::new(x0 + 10.0, win.bottom() - FOOT + 8.0, 112.0, 24.0);
    let full = list.len() >= crate::container::MAX_MEMBERS;
    let channel = slot == crate::container::CHANNEL;
    let hint = match (full, channel) {
        (true, _) => "The container is full (15 plugins)",
        (false, true) => "Add an instrument or an effect at the end of the chain",
        (false, false) => "Add an effect at the end of the chain",
    };
    let hov = ui.hit(add, Action::Container(CAct::Add), hint);
    super::button_face(ui, add, if hov { Look::Hover } else { Look::Off });
    let ink = if full { t.text_faint } else if hov { t.tx(0xffffff) } else { t.text };
    icons::draw(ui.c, Icon::Plus, Rect::new(add.x + 6.0, add.cy() - 6.0, 12.0, 12.0), ink);
    super::strum::label(ui, lf, add.x + 22.0, add.cy(), -1, ink, if channel { "Add plugin" } else { "Add effect" });
}
