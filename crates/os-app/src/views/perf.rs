// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The Plugin performance monitor window: options row, sortable list of the hosted plugins' processing times.
// spec: ui/plugin-performance.md (Lemon Studio layout)

use os_ui::kit::lemon as lk;
use os_ui::{kit, Font, Rect};

use super::FrameInfo;
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::perf::{fmt_us, Col, PerfAct};

pub const SIZE: (f32, f32) = (660.0, 340.0);
const TITLE_H: f32 = 30.0;
pub const ROW_H: f32 = 20.0;
/// The list's top (header row) below the window's top.
const LIST_Y: f32 = 60.0;

/// Columns: left edge (from the list's left) and width.
const COLS: [(Col, f32, f32); 7] = [
    (Col::Plugin, 0.0, 280.0),
    (Col::Type, 280.0, 42.0),
    (Col::Share, 322.0, 44.0),
    (Col::Time, 366.0, 74.0),
    (Col::Peak, 440.0, 74.0),
    (Col::Total, 514.0, 74.0),
    (Col::Voices, 588.0, 60.0),
];

pub fn rect(s: &State) -> Rect {
    Rect::new(s.perf.pos.0, s.perf.pos.1, SIZE.0, SIZE.1)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

/// The list area (header excluded).
pub fn list_rect(s: &State) -> Rect {
    let w = rect(s);
    Rect::new(w.x + 6.0, w.y + LIST_Y + ROW_H, w.w - 12.0, w.h - LIST_Y - ROW_H - 6.0)
}

/// Rows that fit in the list.
pub fn rows_fit(s: &State) -> usize {
    (list_rect(s).h / ROW_H).floor().max(1.0) as usize
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let p = &s.perf;
    let lemon = super::lemon(ui);
    if lemon {
        // Lemon (spec: ui/lemon-design.md §3): a rounded window.
        lk::outlined(ui.c, win, lk::r_panel(t), t.window_border, t.window_body);
    } else {
        ui.c.fill_rect(win, t.window_border);
        ui.c.fill_rect(win.inset(1.0), t.window_body);
        ui.c.fill_rect(Rect::new(win.x + 1.0, win.y + 1.0, win.w - 2.0, 1.0), t.window_title_hi);
    }
    ui.hit(win, Action::Swallow, "");
    let bar = Rect::new(win.x + 1.0, win.y + 1.0, win.w - 2.0, TITLE_H - 2.0);
    ui.hit(bar, Action::WinDrag(Win::Perf), "Drag to move");
    icons::draw(ui.c, Icon::Plug, Rect::new(win.x + 9.0, win.y + 7.0, 15.0, 15.0), t.glyph);
    ui.c.text_clip(Font::new(17), win.x + 30.0, win.y + 5.0, win.w - 60.0, t.text, "Plugin performance monitor");
    let close = Rect::new(win.right() - 22.0, win.y + 6.0, 16.0, 16.0);
    let hov = ui.hit(close, Action::CloseWin(Win::Perf), "Close (the measurements are reset)");
    icons::draw(ui.c, Icon::Close, close.inset(1.5), if hov { t.tx(0xffffff) } else { t.glyph });

    // Options row (§1).
    let f = Font::new(14);
    let lh = ui.c.line_height(f);
    let cy = win.y + 44.0;
    let ty = (cy - lh / 2.0).round();
    let mut x = win.x + 16.0;
    for (act, label, on, hint) in [
        (PerfAct::ResetOnTransport, "Reset on transport", p.reset_on_transport, "Reset on transport: Play and Stop clear the peaks"),
        (PerfAct::Freeze, "Freeze", p.freeze, "Freeze: stop updating the readings"),
    ] {
        let w = ui.c.text_width(f, label);
        let r = Rect::new(x - 8.0, cy - 9.0, w + 26.0, 18.0);
        let hov = ui.hit(r, Action::Perf(act), hint);
        kit::led(ui.c, t, x, cy, 5.0, kit::State::new(hov, false, on));
        ui.c.text(f, x + 10.0, ty, if hov { t.glyph } else { t.text }, label);
        x += w + 40.0;
    }
    let fx = win.x + 300.0;
    ui.c.text(f, fx, ty, t.text, "Find");
    let fbox = Rect::new(fx + 36.0, cy - 10.0, win.right() - 8.0 - (fx + 36.0), 20.0);
    let hov = ui.hit(fbox, Action::Perf(PerfAct::Find), "Find: show only the plugins whose name, patch or place has this text (Esc clears)");
    let well = if p.finding || hov { t.window_body.shade(-0.25) } else { t.window_body.shade(-0.18) };
    if lemon {
        lk::well(ui.c, t, fbox, well);
    } else {
        kit::sunken(ui.c, t, fbox, well);
    }
    let tw = ui.c.text(f, fbox.x + 6.0, ty, t.text, &p.find);
    if p.finding {
        ui.c.fill_rect(Rect::new(fbox.x + 7.0 + tw, cy - 7.0, 1.0, 14.0), t.text);
    }

    // The list (§2): header, then rows.
    let lx = win.x + 6.0;
    let header = Rect::new(lx, win.y + LIST_Y, win.w - 12.0, ROW_H);
    let list = list_rect(s);
    let well = Rect::new(lx, header.y, header.w, list.bottom() - header.y).outset(1.0);
    if lemon {
        let k = lk::r_small(t);
        lk::outlined(ui.c, well, k, t.rack_inset_border, t.rack_inset);
        ui.c.fill_rrect(header, (k - 1.0).max(0.0), t.window_body.shade(0.06));
        ui.c.fill_rect(Rect::new(header.x, header.cy(), header.w, ROW_H / 2.0), t.window_body.shade(0.06));
    } else {
        ui.c.fill_rect(well, t.rack_inset_border);
        ui.c.fill_rect(Rect::new(lx, list.y, list.w, list.h), t.rack_inset);
        ui.c.fill_rect(header, t.window_body.shade(0.06));
    }
    let hf = Font::new(13);
    let hy = (header.cy() - ui.c.line_height(hf) / 2.0).round();
    for (c, cx, cw) in COLS {
        let r = Rect::new(lx + cx, header.y, cw, ROW_H);
        let hov = ui.hit(r, Action::Perf(PerfAct::Sort(c)), format!("Sort by {} (click again to reverse)", c.title()));
        let sorted = p.sort == c;
        let col = if sorted || hov { t.text } else { t.text_dim };
        let w = ui.c.text_width(hf, c.title());
        let tx = if c == Col::Plugin || c == Col::Type { r.x + 6.0 } else { r.right() - 6.0 - w };
        ui.c.text(hf, tx, hy, col, c.title());
        if sorted {
            let ax = if c == Col::Plugin || c == Col::Type { tx + w + 8.0 } else { tx - 8.0 };
            kit::triangle(ui.c, ax, header.cy(), 3.5, if p.desc { kit::Dir::Down } else { kit::Dir::Up }, t.glyph);
        }
        if cx > 0.0 {
            ui.c.fill_rect(Rect::new(r.x, header.y + 3.0, 1.0, ROW_H - 6.0), t.rack_inset_border);
        }
    }
    ui.hit(list, Action::Perf(PerfAct::List), "");
    let shown = p.shown();
    if shown.is_empty() {
        let msg = if p.rows.is_empty() { "No plugins are hosted" } else { "No plugin matches" };
        ui.c.text_center(f, Rect::new(list.x, list.y, list.w, ROW_H * 2.0), t.text_dim, msg);
        return;
    }
    let fit = rows_fit(s);
    let first = p.scroll.min(shown.len().saturating_sub(fit));
    ui.c.push_clip(list);
    for (i, row) in shown.iter().enumerate().skip(first).take(fit) {
        let ry = list.y + (i - first) as f32 * ROW_H;
        let r = Rect::new(list.x, ry, list.w, ROW_H);
        if (i - first) % 2 == 1 {
            ui.c.fill_rect(r, t.rack_inset.shade(0.03));
        }
        if ui.hit(r, Action::Perf(PerfAct::Row(i)), "Double-click: open the plugin's window") {
            kit::row_highlight(ui.c, t, r);
        }
        let ty = (r.cy() - lh / 2.0).round();
        // Plugin: name - patch, then the place (dimmed), "(stopped)" after a crash.
        let pc = COLS[0];
        let title = row.title();
        let tw = ui.c.text_width(f, &title);
        ui.c.text_clip(f, r.x + 6.0, ty, pc.2 - 12.0, t.text, &title);
        let mut px = r.x + 6.0 + tw + 8.0;
        if row.stopped {
            px += ui.c.text(f, px, ty, t.meter_clip, "(stopped)") + 8.0;
        }
        let room = r.x + pc.2 - 6.0 - px;
        if room > 12.0 {
            ui.c.text_clip(f, px, ty, room, t.text_dim, &row.place);
        }
        let ty_s = if row.synth { "Synth" } else { "FX" };
        ui.c.text(f, r.x + COLS[1].1 + 6.0, ty, t.text, ty_s);
        let nums = [
            format!("{:.1}", row.share()),
            fmt_us(row.time),
            fmt_us(row.peak),
            fmt_us(row.total),
            row.voices.map_or("-".to_string(), |v| v.to_string()),
        ];
        for ((_, cx, cw), v) in COLS[2..].iter().zip(nums) {
            ui.c.text_right(f, r.x + cx + cw - 6.0, ty, t.text, &v);
        }
    }
    ui.c.pop_clip();
    // Where the shown rows sit when they do not all fit: a thin bar at the right.
    if shown.len() > fit {
        let k = list.h / shown.len() as f32;
        let bar = Rect::new(list.right() - 3.0, list.y + first as f32 * k, 2.0, (fit as f32 * k).max(6.0));
        ui.c.fill_rect(bar, t.glyph.with_alpha(110));
    }
}
