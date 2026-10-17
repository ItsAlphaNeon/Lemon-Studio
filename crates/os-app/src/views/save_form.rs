// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's Save as and New project forms (`crate::save_form`), painted in the dialogs' frame: the Name and
//! location group, Time settings (New project only), the Information fold, then Don't show this in the future and the
//! buttons. A combo's list is drawn in the form, over everything, like Manage shortcuts' lists.
// spec: ui/data-folder.md §7

use os_theme::design::lemon as brand;
use os_ui::kit::{self, lemon as lk};
use os_ui::{Font, Rect};

use super::dialog::{button, field, frame, hit, panel, place, section, tool, BTN_H, BTN_W};
use super::lemon;
use super::project_forms::{check, label};
use crate::app::{Action, State, Ui};
use crate::dialog::{DHit, Modal};
use crate::icons::Icon;
use crate::project_forms::FHit;
use crate::save_form::{self as sf, SaveForm};

/// The forms' sizes with Information folded, as measured (spec §7): Save as 485 x 404, New project 485 x 504.
const W: f32 = 485.0;
const SAVE_H: f32 = 404.0;
const NEW_H: f32 = 504.0;
/// Where the groups sit, from the form's top: the first one's top, the groups' heights and the gap between them.
const TOP: f32 = 34.0;
const PLACE_H: f32 = 206.0;
const TIME_H: f32 = 104.0;
const GAP: f32 = 6.0;
/// The Information group: its header (the fold) and a field's row.
const FOLD_H: f32 = 30.0;
const INFO_ROW: f32 = 30.0;
/// Where the boxes start in a group, from its left edge.
const BOX_X: f32 = 118.0;
/// The bottom band (Don't show this in the future, the buttons) and a list entry's height.
const BAND: f32 = 55.0;
const ITEM_H: f32 = 20.0;

/// The hints the spec does not give (Lemon Studio's).
const INFO_HINTS: [&str; 4] = ["The project's title", "The project's author", "A web link for the project", "The project's genre"];
const TIME_HINTS: [&str; 3] = ["Timebase (PPQ): click to choose", "Numerator: click to choose", "Denominator: click to choose"];

/// The Information group's top (from the form's top) and height.
fn info_place(f: &SaveForm) -> (f32, f32) {
    let y = TOP + PLACE_H + GAP + if f.new { TIME_H + GAP } else { 0.0 };
    (y, if f.info_open { FOLD_H + sf::INFO_FIELDS.len() as f32 * INFO_ROW + 6.0 } else { FOLD_H })
}

/// The form's size: as measured while Information is folded, taller when it is unfolded.
pub fn size(f: &SaveForm) -> (f32, f32) {
    let (y, h) = info_place(f);
    (W, (if f.new { NEW_H } else { SAVE_H }).max(y + h + 10.0 + BAND))
}

/// Paints form `f` of dialog `m`.
pub fn paint(ui: &mut Ui, s: &State, m: &Modal, f: &SaveForm) {
    let (w, h) = size(f);
    let r = place(s, m, w, h);
    frame(ui, r, f.title());
    let t = ui.t;
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    let mut boxes = Vec::new();
    // Name and location.
    let g = Rect::new(r.x + 8.0, r.y + TOP, r.w - 16.0, PLACE_H);
    section(ui, g, sf::GROUP);
    let (lx, bx) = (g.x + 12.0, g.x + BOX_X);
    let name = Rect::new(bx, g.y + 32.0, g.right() - 12.0 - bx, 24.0);
    label(ui, lx, (name.cy() - lh / 2.0).round(), sf::NAME, true);
    field(ui, name, &f.name, f.focus == 0, DHit::Form(FHit::Field(0)), sf::NAME_HINT);
    let folder = Rect::new(g.right() - 12.0 - 26.0, g.y + 66.0, 26.0, 24.0);
    let auto = Rect::new(folder.x - 4.0 - 52.0, folder.y, 52.0, 24.0);
    let ty = (folder.cy() - lh / 2.0).round();
    label(ui, lx, ty, sf::LOCATION, true);
    let shown = fit_tail(ui, font, &f.location(), auto.x - 8.0 - bx);
    ui.c.text(font, bx, ty, t.text_dim, &shown);
    small_button(ui, auto, sf::AUTO, FHit::Auto, sf::AUTO_HINT);
    tool(ui, folder, Icon::Folder, DHit::Form(FHit::Folder), sf::FOLDER_HINT, true);
    check(ui, lx, g.y + 114.0, sf::CHECKS[0], f.new_folder, true, FHit::Check(0), sf::CHECK_HINTS[0]);
    check(ui, lx, g.y + 142.0, sf::CHECKS[1], f.copy, true, FHit::Check(1), sf::CHECK_HINTS[1]);
    let files = Rect::new(lx, g.y + 166.0, 136.0, 22.0);
    combo(ui, f, files, 0, sf::FILES_HINT, &mut boxes);
    let fx = files.right() + 8.0;
    ui.c.text_clip(font, fx, (files.cy() - lh / 2.0).round(), g.right() - 8.0 - fx, t.text, sf::FILES_TEXT);
    // Time settings (New project).
    if f.new {
        let g = Rect::new(r.x + 8.0, r.y + TOP + PLACE_H + GAP, r.w - 16.0, TIME_H);
        section(ui, g, sf::TIME);
        for (k, (text, hint)) in sf::TIME_LABELS.into_iter().zip(TIME_HINTS).enumerate() {
            let b = Rect::new(g.x + BOX_X, g.y + 30.0 + k as f32 * 24.0, 80.0, 20.0);
            label(ui, g.x + 12.0, (b.cy() - lh / 2.0).round(), text, true);
            combo(ui, f, b, k as u8 + 1, hint, &mut boxes);
        }
    }
    info(ui, r, f);
    bottom(ui, r, f);
    if let Some(&(k, b)) = boxes.iter().find(|(k, _)| f.list == Some(*k)) {
        list(ui, s, f, k, b);
    }
}

/// The Information group: its fold (a triangle and the title), and while unfolded Title, Author, Web link and Genre.
fn info(ui: &mut Ui, r: Rect, f: &SaveForm) {
    let t = ui.t;
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    let (y, h) = info_place(f);
    let g = Rect::new(r.x + 8.0, r.y + y, r.w - 16.0, h);
    panel(ui, g);
    let head = Rect::new(g.x, g.y, g.w, FOLD_H);
    let hint = if f.info_open { "Hide the project's information" } else { "Show the project's information" };
    let hov = hit(ui, head, DHit::Form(FHit::Fold), hint);
    let dir = if f.info_open { kit::Dir::Down } else { kit::Dir::Right };
    kit::triangle(ui.c, g.x + 16.0, head.cy(), 4.0, dir, if hov { t.glyph } else { t.text_dim });
    ui.c.text(font.bold(), g.x + 28.0, (head.cy() - lh / 2.0).round(), if hov { t.glyph } else { t.text }, sf::INFO);
    if !f.info_open {
        return;
    }
    for (k, (text, hint)) in sf::INFO_FIELDS.into_iter().zip(INFO_HINTS).enumerate() {
        let b = Rect::new(g.x + BOX_X, g.y + FOLD_H + k as f32 * INFO_ROW, g.right() - 12.0 - (g.x + BOX_X), 24.0);
        label(ui, g.x + 12.0, (b.cy() - lh / 2.0).round(), text, true);
        let n = k as u8 + 1;
        field(ui, b, &f.info[k], f.focus == n, DHit::Form(FHit::Field(n)), hint);
    }
}

/// The bottom band: Don't show this in the future, then Skip (Save as only), Save or Create, and Cancel.
fn bottom(ui: &mut Ui, r: Rect, f: &SaveForm) {
    let font = Font::new(13);
    let by = r.bottom() - 12.0 - BTN_H;
    check(ui, r.x + 16.0, by + BTN_H / 2.0, sf::CHECKS[2], f.dont_show, true, FHit::Check(2), sf::CHECK_HINTS[2]);
    let width = |ui: &mut Ui, s: &str| (ui.c.text_width(font, s) + 28.0).max(BTN_W).round();
    let mut parts = vec![("Cancel", DHit::Cancel, false, "Close without saving (Esc)"), (f.main(), DHit::Form(FHit::Main), true, sf::MAIN_HINT)];
    if !f.new {
        parts.push((sf::SKIP, DHit::Form(FHit::Skip), false, sf::SKIP_HINT));
    }
    let mut x = r.right() - 12.0;
    for (text, h, main, hint) in parts {
        let bw = width(ui, text);
        x -= bw;
        let b = Rect::new(x, by, bw, BTN_H);
        x -= 6.0;
        let act = Action::Dlg(h);
        let down = ui.down(b, &act);
        let hov = ui.hit(b, act, hint);
        button(ui, b, text, main, hov, down, false);
    }
}

/// Combo `k` of the form in `b`, remembered in `boxes` for its list.
fn combo(ui: &mut Ui, f: &SaveForm, b: Rect, k: u8, hint: &'static str, boxes: &mut Vec<(u8, Rect)>) {
    let t = ui.t;
    let hov = hit(ui, b, DHit::Form(FHit::Combo(k)), hint);
    kit::combo(ui.c, t, b, &f.combo_text(k), Font::new(13), kit::State::new(hov, f.list == Some(k), false));
    boxes.push((k, b));
}

/// A button smaller than the bottom ones (Auto).
fn small_button(ui: &mut Ui, b: Rect, text: &str, h: FHit, hint: &'static str) {
    let act = Action::Dlg(DHit::Form(h));
    let down = ui.down(b, &act);
    let hov = ui.hit(b, act, hint);
    button(ui, b, text, false, hov, down, false);
}

/// `text` as it fits in `w`: whole, else its end after "...".
fn fit_tail(ui: &mut Ui, font: Font, text: &str, w: f32) -> String {
    if ui.c.text_width(font, text) <= w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    (1..chars.len()).map(|i| format!("...{}", chars[i..].iter().collect::<String>())).find(|s| ui.c.text_width(font, s) <= w).unwrap_or_default()
}

/// Combo `k`'s open list under its box `b` (above it when the window has no room below), the chosen entry marked. A
/// click anywhere else only closes it.
fn list(ui: &mut Ui, s: &State, f: &SaveForm, k: u8, b: Rect) {
    let t = ui.t;
    hit(ui, Rect::new(0.0, 0.0, s.screen.0, s.screen.1), DHit::Form(FHit::Shut), "");
    let (items, now) = f.list_items(k);
    let h = items.len() as f32 * ITEM_H + 6.0;
    let below = Rect::new(b.x, b.bottom() + 2.0, b.w.max(64.0), h);
    let p = if below.bottom() > s.screen.1 - 4.0 { Rect::new(below.x, (b.y - 2.0 - h).max(0.0), below.w, h) } else { below };
    let lemon = lemon(ui);
    if lemon {
        lk::popup(ui.c, t, p);
    } else {
        ui.c.fill_rect(p, t.menu_pop_border);
        ui.c.fill_rect(p.inset(1.0), t.menu_pop);
    }
    hit(ui, p, DHit::Body, "");
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    for (i, text) in items.iter().enumerate() {
        let row = Rect::new(p.x + 1.0, p.y + 3.0 + i as f32 * ITEM_H, p.w - 2.0, ITEM_H);
        if hit(ui, row, DHit::Form(FHit::Item(i as u8)), "") {
            if lemon {
                lk::row_highlight(ui.c, t, row);
            } else {
                ui.c.fill_rect(row, t.menu_pop_hi);
            }
        }
        let col = if i == now { if lemon { brand::MAIN } else { t.glyph } } else { t.text };
        ui.c.text_clip(font, row.x + 10.0, (row.cy() - lh / 2.0).round(), row.w - 16.0, col, text);
    }
}
