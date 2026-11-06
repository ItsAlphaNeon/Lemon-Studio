// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin palette window: the title bar, the search field, the kind and arrangement pills, the list and the
//! footer (the count, Scan, Manage and the browser's Plugin library).
// spec: ui/plugin-menu.md §6 (Lemon: ui/lemon-design.md §3, §9)

use os_theme::design::lemon as brand;
use os_ui::kit::lemon as lk;
use os_ui::{kit, Font, Rect};

use super::{FrameInfo, Look};
use crate::app::{Action, Scroll, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::motion::{Motion, Slot};
use crate::plugin_palette::{self as pal, PalAct, Row, KINDS, VIEWS};

/// Shortest the window gets in a short workspace.
pub const MIN_H: f32 = 240.0;
const TITLE_H: f32 = 32.0;
/// The search field, the pills row and the list's top, from the window's top.
const SEARCH_Y: f32 = 38.0;
const SEARCH_H: f32 = 28.0;
const PILLS_Y: f32 = 74.0;
const PILL_H: f32 = 22.0;
const LIST_Y: f32 = 104.0;
const FOOT_H: f32 = 40.0;
const PAD: f32 = 10.0;
/// The list's scroll bar.
const BAR_W: f32 = 8.0;

pub fn rect(s: &State) -> Rect {
    pal::rect(s)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

/// The list's well.
fn well(s: &State) -> Rect {
    let w = rect(s);
    Rect::new(w.x + PAD, w.y + LIST_Y, W_IN, w.h - LIST_Y - FOOT_H)
}

const W_IN: f32 = pal::W - 2.0 * PAD;

/// Where the rows show (inside the well).
fn rows_rect(s: &State) -> Rect {
    let w = well(s);
    Rect::new(w.x + 1.0, w.y + 3.0, w.w - 2.0, w.h - 6.0)
}

/// The height the rows show in.
pub fn list_h(s: &State) -> f32 {
    rows_rect(s).h
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let p = &s.palette;
    let lemon = super::lemon(ui);
    let keys = pal::has_keys(s);
    // It appears as a menu does (spec: ui/animations.md §4.3), growing from where it opened: its top left.
    let draw = s.motion.borrow_mut().menu_begin(ui.c, Slot::Palette, 0, win, (win.x, win.y), 1.0);
    // The frame: a rounded tool window, a flat title bar with a hairline under it (Lemon, spec: ui/lemon-design.md §3).
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), t.window_border, t.window_body);
        ui.c.fill_rect(Rect::new(win.x + 1.0, win.y + TITLE_H - 1.0, win.w - 2.0, 1.0), lk::hairline(t.window_body));
    } else {
        ui.c.fill_rect(win, t.window_border);
        ui.c.fill_rect(win.inset(1.0), t.window_body);
        ui.c.fill_rect(Rect::new(win.x + 1.0, win.y + 1.0, win.w - 2.0, 1.0), t.window_title_hi);
    }
    ui.hit(win, Action::Swallow, "");
    ui.hit(Rect::new(win.x + 1.0, win.y + 1.0, win.w - 34.0, TITLE_H - 2.0), Action::WinDrag(Win::PluginPalette), "Drag to move");
    icons::draw(ui.c, Icon::Plug, Rect::new(win.x + 11.0, win.y + 9.0, 15.0, 15.0), if lemon { t.accent } else { t.glyph });
    super::strum::label(ui, Font::new(17), win.x + 33.0, win.y + TITLE_H / 2.0, -1, t.text, pal::title(p));
    let close = Rect::new(win.right() - 25.0, win.y + 8.0, 16.0, 16.0);
    let hov = ui.hit(close, Action::CloseWin(Win::PluginPalette), "Close (Esc)");
    if hov && lemon {
        lk::window_button_hover(ui.c, t, close.outset(2.0), true);
    }
    icons::draw(ui.c, Icon::Close, close.inset(2.5), if hov { t.tx(0xffffff) } else { t.glyph });

    search(ui, s, keys);
    pills(ui, s);
    list(ui, s);
    footer(ui, s);
    Motion::menu_end(ui.c, draw);
}

/// The search field (§6.2): a magnifier, the text or the placeholder, the caret while the palette has the keyboard,
/// the clear button while there is text.
fn search(ui: &mut Ui, s: &State, keys: bool) {
    let t = ui.t;
    let win = rect(s);
    let p = &s.palette;
    let field = Rect::new(win.x + PAD, win.y + SEARCH_Y, W_IN, SEARCH_H);
    ui.hit(field, Action::Palette(PalAct::Search), "Search by name, vendor, category or format (Esc clears)");
    kit::sunken(ui.c, t, field, t.window_body.shade(-0.3));
    if keys {
        ui.c.stroke_rrect(field, if super::lemon(ui) { lk::r_btn(t).min(field.h / 2.0) } else { 0.0 }, 1.0, t.accent);
    }
    icons::draw(ui.c, Icon::Search, Rect::new(field.x + 9.0, field.cy() - 7.0, 14.0, 14.0), if keys { t.glyph } else { t.text_dim });
    let f = Font::new(15);
    let lh = ui.c.line_height(f);
    let ty = (field.cy() - lh / 2.0).round();
    let tx = field.x + 31.0;
    let room = field.right() - 30.0 - tx;
    if p.query.is_empty() {
        ui.c.text_clip(f, tx, ty, room, t.text_faint, "Search plugins");
        if keys {
            ui.c.fill_rect(Rect::new(tx, ty + 2.0, 1.0, lh - 4.0), t.glyph);
        }
        return;
    }
    // A long search shows its end.
    let mut shown: &str = &p.query;
    while ui.c.text_width(f, shown) > room && !shown.is_empty() {
        let cut = shown.char_indices().nth(1).map_or(shown.len(), |(i, _)| i);
        shown = &shown[cut..];
    }
    let w = ui.c.text(f, tx, ty, t.text, shown);
    if keys {
        ui.c.fill_rect(Rect::new((tx + w + 1.0).round(), ty + 2.0, 1.0, lh - 4.0), t.glyph);
    }
    let clear = Rect::new(field.right() - 25.0, field.cy() - 8.0, 16.0, 16.0);
    let hov = ui.hit(clear, Action::Palette(PalAct::Clear), "Clear the search");
    icons::draw(ui.c, Icon::Close, clear.inset(3.0), if hov { t.tx(0xffffff) } else { t.text_dim });
}

/// The kind pills (both kinds listed) and the arrangement pills at the right (§6.2).
fn pills(ui: &mut Ui, s: &State) {
    let t = ui.t;
    let win = rect(s);
    let p = &s.palette;
    let f = Font::new(13);
    let y = win.y + PILLS_Y;
    let width = |ui: &mut Ui, label: &str| (ui.c.text_width(f, label) + 18.0).round();
    if pal::mixed(p) {
        let mut x = win.x + PAD;
        for (k, label) in KINDS {
            let r = Rect::new(x, y, width(ui, label), PILL_H);
            let hint = match k {
                pal::Kind::All => "Instruments and effects (Tab: the next kind)",
                pal::Kind::Instruments => "Instruments only (Tab: the next kind)",
                pal::Kind::Effects => "Effects only (Tab: the next kind)",
            };
            let hov = ui.hit(r, Action::Palette(PalAct::Kind(k)), hint);
            kit::pill(ui.c, t, r, label, f, kit::State::new(hov, false, p.kind == k));
            x = r.right() + 2.0;
        }
    }
    let mut x = win.right() - PAD;
    for (v, label) in VIEWS.iter().rev() {
        let w = width(ui, label);
        x -= w;
        let r = Rect::new(x, y, w, PILL_H);
        let hint = match label {
            &"Type" => "Arrange by type",
            &"A-Z" => "Arrange alphabetically",
            _ => "Arrange by vendor",
        };
        let hov = ui.hit(r, Action::Palette(PalAct::View(*v)), hint);
        kit::pill(ui.c, t, r, label, f, kit::State::new(hov, false, s.plugdb.prefs.menu_view == *v));
        x -= 2.0;
    }
}

/// The list (§6.2-6.4): headings, plugin rows with the highlighted one, notes; the scroll bar when it does not fit.
fn list(ui: &mut Ui, s: &State) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    let p = &s.palette;
    let w = well(s);
    if lemon {
        lk::outlined(ui.c, w, lk::r_small(t) + 2.0, t.rack_inset_border, t.rack_inset);
    } else {
        ui.c.fill_rect(w, t.rack_inset_border);
        ui.c.fill_rect(w.inset(1.0), t.rack_inset);
    }
    ui.hit(w, Action::Palette(PalAct::List), "");
    let rows = pal::rows(s);
    let (tops, total) = pal::tops(&rows);
    let mut area = rows_rect(s);
    let view = area.h;
    let scroll = p.scroll.min((total - view).max(0.0));
    if total > view {
        let track = Rect::new(area.right() - BAR_W - 2.0, area.y, BAR_W, area.h);
        super::scrollbar(ui, track, true, scroll, view, total, Scroll::Palette);
        area.w -= BAR_W + 3.0;
    }
    ui.c.push_clip(area);
    // The rows go to a new arrangement rather than jump (spec: ui/animations.md §4.3).
    s.motion.borrow_mut().palette_list(&rows, &tops, scroll, view);
    let gone = s.motion.borrow().list_gone();
    for (row, y, shows) in gone {
        let r = Rect::new(area.x, area.y + y, area.w, row.height());
        let l = ui.c.begin_layer(r, false);
        paint_row(ui, s, &row, r, None);
        ui.c.end_layer(l, shows);
    }
    // The rows showing, in order, for their fade at Playful (§4.3).
    let mut showing = 0;
    let mut fading = false;
    for (i, row) in rows.iter().enumerate() {
        let ry = area.y + tops[i] - scroll;
        let h = row.height();
        let (off, shows) = s.motion.borrow().list_row(row);
        let there = !(ry + h < area.y || ry > area.bottom());
        if !there && !(ry + off + h >= area.y && ry + off <= area.bottom()) {
            continue;
        }
        let place = Rect::new(area.x, ry, area.w, h);
        let r = Rect::new(area.x, ry + off, area.w, h);
        let opens = s.motion.borrow().palette_row(showing);
        showing += 1;
        // It takes clicks at its place from the first frame.
        let hit = there.then_some((i, place));
        if shows < 1.0 {
            let l = ui.c.begin_layer(r, false);
            paint_row(ui, s, row, r, hit);
            ui.c.end_layer(l, shows);
        } else {
            paint_row(ui, s, row, r, hit);
        }
        if opens < 1.0 {
            ui.c.fill_rect(r, t.rack_inset.with_alpha(((1.0 - opens) * 255.0) as u8));
            fading = true;
        }
    }
    ui.c.pop_clip();
    if fading || s.motion.borrow().list_moving() {
        s.motion.borrow_mut().live(area);
    }
}

/// A row of the list drawn in `r`; `hit`: its index in the list and the place it takes clicks at (a row fading out
/// has none).
fn paint_row(ui: &mut Ui, s: &State, row: &Row, r: Rect, hit: Option<(usize, Rect)>) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    let h = row.height();
    let name_f = Font::new(15);
    let small = Font::new(12);
    let lh = ui.c.line_height(name_f);
    let slh = ui.c.line_height(small);
    match row {
        Row::Head { label, major: true } => {
            let f = Font::new(14).bold();
            let col = if lemon { t.accent } else { t.text };
            super::strum::label(ui, f, r.x + 10.0, r.y + h / 2.0 + 2.0, -1, col, label);
        }
        Row::Head { label, .. } => {
            let cy = r.y + h / 2.0 + 1.0;
            super::strum::label(ui, small, r.x + 10.0, cy, -1, t.text_dim, label);
            let x = r.x + 18.0 + ui.c.text_width(small, label);
            if x < r.right() - 10.0 {
                ui.c.fill_rect(Rect::new(x, cy.round(), r.right() - 10.0 - x, 1.0), t.rack_inset_border);
            }
        }
        Row::Note(text) => super::strum::label(ui, Font::new(14), r.x + 12.0, r.cy(), -1, t.text_faint, text),
        &Row::Plugin { effect, at } => {
            let Some(e) = pal::entry(s, effect, at) else { return };
            let hint = format!("Add {} (Shift+click: keep the palette open)", e.label);
            let hov = hit.is_some_and(|(i, place)| ui.hit(place, Action::Palette(PalAct::Row(i)), hint));
            let sel = hit.is_some_and(|(i, _)| i == s.palette.sel);
            let bar = Rect::new(r.x + 3.0, r.y + 1.0, r.w - 6.0, r.h - 2.0);
            if sel {
                let bg = if lemon { t.brand_sel(brand::COLORED_DARK) } else { t.menu_drop_hi };
                ui.c.fill_rrect(bar, 4.0, if hov { bg.mix(t.button_hi, 0.25) } else { bg });
                ui.c.fill_rrect(Rect::new(bar.x, bar.y + 4.0, 3.0, bar.h - 8.0), 1.5, t.accent);
            } else if hov {
                kit::row_highlight(ui.c, t, Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0));
            }
            if e.favorite {
                icons::draw(ui.c, Icon::Star, Rect::new(r.x + 11.0, r.cy() - 5.5, 11.0, 11.0), t.accent);
            }
            let ink = if sel && lemon { t.brand_sel(brand::HIGHLIGHT) } else if sel || hov { t.glyph } else { t.text };
            let detail = pal::detail(s, e, effect);
            let dw = ui.c.text_width(small, &detail).min(r.w * 0.4);
            let dx = r.right() - 10.0 - dw;
            ui.c.text_clip(small, dx, (r.cy() - slh / 2.0).round(), dw, t.text_dim, &detail);
            let nx = r.x + 27.0;
            ui.c.text_clip(name_f, nx, (r.cy() - lh / 2.0).round(), dx - 10.0 - nx, ink, &e.label);
        }
    }
}

/// The footer (§6.2): the count, then Scan, Manage and the browser's Plugin library at the right.
fn footer(ui: &mut Ui, s: &State) {
    let t = ui.t;
    let win = rect(s);
    let rows = pal::rows(s);
    let cy = win.bottom() - FOOT_H / 2.0 + 1.0;
    let f = Font::new(13);
    super::strum::label(ui, f, win.x + PAD + 4.0, cy, -1, t.text_dim, &pal::count_text(s, &rows));
    let scanning = s.plugdb.progress.is_some();
    let mut x = win.right() - PAD;
    let lib = Rect::new(x - 28.0, cy - 12.0, 28.0, 24.0);
    let hov = ui.hit(lib, Action::Palette(PalAct::Library), "Plugin library (Shift+F8)");
    super::button_face(ui, lib, if hov { Look::Hover } else { Look::Off });
    icons::draw(ui.c, Icon::Browser, lib.inset(6.0), if hov { t.tx(0xffffff) } else { t.glyph });
    x = lib.x - 4.0;
    for (act, label, hint, off) in [
        (PalAct::Manage, "Manage", "Manage plugins... (Settings > Plugins)", false),
        (PalAct::Scan, "Scan", if scanning { "Scanning for new plugins..." } else { "Find new plugins (a background scan)" }, scanning),
    ] {
        let w = (ui.c.text_width(f, label) + 20.0).round();
        let r = Rect::new(x - w, cy - 12.0, w, 24.0);
        let hov = ui.hit(r, Action::Palette(act), hint) && !off;
        kit::button(ui.c, t, r, kit::State::hover(hov).with_disabled(off));
        let ink = if off { t.text_faint } else if hov { t.tx(0xffffff) } else { t.text };
        ui.c.text_center(f, r, ink, label);
        x = r.x - 4.0;
    }
}
