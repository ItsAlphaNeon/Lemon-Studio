// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Export form (`crate::export_form`), painted in the dialogs' frame: File across the top, then two columns,
//! Source over Formats and Quality over Extras, and the bottom band. A combo's list is drawn over everything, as Save
//! as does.
// spec: audio/export.md §3

use os_theme::design::lemon as brand;
use os_ui::kit::{self, lemon as lk};
use os_ui::{Font, Rect};

use super::dialog::{button, field, frame, hit, place, section, tool, BTN_H, BTN_W};
use super::lemon;
use super::project_forms::{check, label};
use crate::app::{Action, State, Ui};
use crate::dialog::{DHit, Modal};
use crate::export::FORMATS;
use crate::export_form::{self as ef, ExportForm};
use crate::icons::Icon;
use crate::project_forms::FHit;

pub const W: f32 = 660.0;
pub const H: f32 = 480.0;
/// Groups: File's top and height, the columns' groups' heights, the gap between groups.
const TOP: f32 = 34.0;
const FILE_H: f32 = 84.0;
const UPPER_H: f32 = 132.0;
const LOWER_H: f32 = 152.0;
const GAP: f32 = 6.0;
const ITEM_H: f32 = 20.0;

/// Paints form `f` of dialog `m`.
pub fn paint(ui: &mut Ui, s: &State, m: &Modal, f: &ExportForm) {
    let r = place(s, m, W, H);
    frame(ui, r, if f.to_clip { crate::pattern_render::TITLE } else { ef::TITLE });
    let t = ui.t;
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    let mut boxes = Vec::new();
    // File: the name and the folder.
    let g = Rect::new(r.x + 8.0, r.y + TOP, r.w - 16.0, FILE_H);
    section(ui, g, "File");
    let bx = g.x + 80.0;
    let name = Rect::new(bx, g.y + 26.0, 320.0, 24.0);
    label(ui, g.x + 12.0, (name.cy() - lh / 2.0).round(), "Name", true);
    field(ui, name, &f.name, true, DHit::Form(FHit::Field(0)), "The files' name (each format adds its extension)");
    let ext: Vec<&str> = f.o.ticked().map(|i| crate::export::EXTENSIONS[i]).collect();
    ui.c.text(font, name.right() + 8.0, (name.cy() - lh / 2.0).round(), t.text_dim, &format!(".{}", ext.join(" .")));
    let folder = Rect::new(g.right() - 12.0 - 26.0, g.y + 54.0, 26.0, 24.0);
    let fy = (folder.cy() - lh / 2.0).round();
    label(ui, g.x + 12.0, fy, "Folder", true);
    ui.c.text_clip(font, bx, fy, folder.x - 8.0 - bx, t.text_dim, &f.folder.display().to_string());
    tool(ui, folder, Icon::Folder, DHit::Form(FHit::Folder), "Choose the folder the files go to", true);
    // The columns.
    let cw = ((r.w - 16.0 - GAP) / 2.0).floor();
    let (lx, rx) = (r.x + 8.0, r.x + 8.0 + cw + GAP);
    let y1 = g.bottom() + GAP;
    let y2 = y1 + UPPER_H + GAP;
    source(ui, f, Rect::new(lx, y1, cw, UPPER_H));
    quality(ui, f, Rect::new(rx, y1, cw, UPPER_H), &mut boxes);
    formats(ui, f, Rect::new(lx, y2, cw, LOWER_H), &mut boxes);
    extras(ui, f, Rect::new(rx, y2, cw, LOWER_H));
    bottom(ui, r, f);
    if let Some(&(k, b)) = boxes.iter().find(|(k, _)| f.list == Some(*k)) {
        let (items, now) = f.list_items(k);
        list(ui, s, &items, now, b);
    }
}

/// Source: Play, Range and Tail as segmented rows, then the length and size line.
fn source(ui: &mut Ui, f: &ExportForm, g: Rect) {
    section(ui, g, "Source");
    let t = ui.t;
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    for (row, (text, segs)) in ef::SEGS.iter().enumerate() {
        let y = g.y + 28.0 + row as f32 * 28.0;
        label(ui, g.x + 12.0, (y + 11.0 - lh / 2.0).round(), text, true);
        let strip = Rect::new(g.x + 72.0, y, g.right() - 12.0 - (g.x + 72.0), 22.0);
        ui.c.fill_rrect(strip, if lemon(ui) { lk::r_btn(t).min(11.0) } else { 11.0 }, t.sunken);
        let sw = strip.w / segs.len() as f32;
        let chosen = f.seg(row as u8);
        for (i, seg) in segs.iter().enumerate() {
            let b = Rect::new((strip.x + i as f32 * sw).round(), strip.y, sw.round(), strip.h);
            let (enabled, off) = match (row, i) {
                (0, 0) => (f.song_ok, "The playlist is empty"),
                (0, _) => (f.pat_ok, "The pattern has no notes"),
                (1, 1) => (f.has_selection(), "There is no time selection"),
                _ => (true, ""),
            };
            let hint = if enabled { ef::SEG_HINTS[row] } else { off };
            let hov = enabled && hit(ui, b, DHit::Form(FHit::Seg(row as u8, i as u8)), hint);
            kit::pill(ui.c, t, b, seg, font, kit::State::new(hov, false, i == chosen));
            if !enabled {
                ui.veil(b, t.sunken);
            }
        }
    }
    ui.c.text(font, g.x + 12.0, g.y + 110.0, t.text_dim, &f.summary_text());
}

/// Quality: sample rate, channels and resampling, then Dither and HQ for plugins.
fn quality(ui: &mut Ui, f: &ExportForm, g: Rect, boxes: &mut Vec<(u8, Rect)>) {
    section(ui, g, "Quality");
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    for (i, (text, k)) in [("Sample rate", 5u8), ("Channels", 6), ("Resampling", 7)].into_iter().enumerate() {
        let b = Rect::new(g.x + 110.0, g.y + 28.0 + i as f32 * 26.0, g.right() - 12.0 - (g.x + 110.0), 22.0);
        label(ui, g.x + 12.0, (b.cy() - lh / 2.0).round(), text, true);
        combo(ui, f, b, k, boxes);
    }
    let cy = g.y + 116.0;
    check_k(ui, f, g.x + 12.0, cy, 0);
    check_k(ui, f, g.x + 150.0, cy, 1);
}

/// Formats: a row per format, its tile (a toggle) and its settings, greyed while it is off.
fn formats(ui: &mut Ui, f: &ExportForm, g: Rect, boxes: &mut Vec<(u8, Rect)>) {
    section(ui, g, "Formats");
    let t = ui.t;
    let font = Font::new(13).bold();
    let combos: [&[(u8, f32)]; 4] = [&[(0, 140.0)], &[(1, 90.0), (2, 90.0)], &[(3, 110.0)], &[(4, 110.0)]];
    for (i, name) in FORMATS.iter().enumerate() {
        let y = g.y + 28.0 + i as f32 * 30.0;
        let tile = Rect::new(g.x + 12.0, y, 64.0, 24.0);
        let on = f.o.formats[i];
        let hov = hit(ui, tile, DHit::Form(FHit::Tile(i as u8)), ef::TILE_HINTS[i]);
        if !on {
            // An outline while off, so the tile still reads as a button.
            let rad = if lemon(ui) { lk::r_btn(t).min(12.0) } else { 12.0 };
            ui.c.stroke_rrect(tile.inset(0.5), rad, 1.0, if hov { t.glyph_dim } else { t.text_faint });
        }
        kit::pill(ui.c, t, tile, name, font, kit::State::new(hov, false, on));
        let mut x = tile.right() + 10.0;
        for &(k, w) in combos[i] {
            combo(ui, f, Rect::new(x, y + 1.0, w, 22.0), k, boxes);
            x += w + 6.0;
        }
        if i == crate::export::MP3 {
            check_k(ui, f, x + 4.0, y + 12.0, 11);
        }
    }
}

/// Extras: the stems, the effects and what WAV files carry.
fn extras(ui: &mut Ui, f: &ExportForm, g: Rect) {
    section(ui, g, "Extras");
    let (c1, c2) = (g.x + 12.0, g.x + 12.0 + (g.w / 2.0).round());
    let row = |i: usize| g.y + 38.0 + i as f32 * 26.0;
    check_k(ui, f, c1, row(0), 2);
    check_k(ui, f, c1 + 18.0, row(1), 3);
    check_k(ui, f, c1, row(2), 4);
    check_k(ui, f, c1, row(3), 5);
    for (i, k) in [6u8, 7, 8, 9].into_iter().enumerate() {
        check_k(ui, f, c2, row(i), k);
    }
}

/// The bottom band: Open the folder when done, then Export and Cancel.
fn bottom(ui: &mut Ui, r: Rect, f: &ExportForm) {
    let font = Font::new(13);
    let by = r.bottom() - 12.0 - BTN_H;
    check_k(ui, f, r.x + 16.0, by + BTN_H / 2.0, 10);
    let width = |ui: &mut Ui, s: &str| (ui.c.text_width(font, s) + 28.0).max(BTN_W).round();
    let mut x = r.right() - 12.0;
    for (text, h, main, hint) in [("Cancel", DHit::Cancel, false, "Close without exporting (Esc)"), (ef::MAIN, DHit::Form(FHit::Main), true, "Render and write the files (Enter)")] {
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

/// Check `k` of the form at left edge `x`, centred on `cy`.
fn check_k(ui: &mut Ui, f: &ExportForm, x: f32, cy: f32, k: u8) {
    let (on, enabled) = f.check(k);
    check(ui, x, cy, ef::CHECKS[usize::from(k)], on, enabled, FHit::Check(k), ef::CHECK_HINTS[usize::from(k)]);
}

/// Combo `k` in `b`, greyed while its format is off; remembered in `boxes` for its list.
fn combo(ui: &mut Ui, f: &ExportForm, b: Rect, k: u8, boxes: &mut Vec<(u8, Rect)>) {
    let t = ui.t;
    let enabled = ExportForm::combo_format(k).is_none_or(|i| f.o.formats[i]);
    let hov = enabled && hit(ui, b, DHit::Form(FHit::Combo(k)), ef::COMBO_HINTS[usize::from(k)]);
    kit::combo(ui.c, t, b, &f.combo_text(k), Font::new(13), kit::State::new(hov, f.list == Some(k), false));
    if !enabled {
        let bg = t.s(0x282c2d);
        ui.veil(b, bg);
    }
    boxes.push((k, b));
}

/// A combo's open list of `items` under its box `b` (above it when the window has no room below), entry `now` marked.
/// A click anywhere else only closes it. The MIDI import form's combos use it too.
pub(super) fn list<S: AsRef<str>>(ui: &mut Ui, s: &State, items: &[S], now: usize, b: Rect) {
    let t = ui.t;
    hit(ui, Rect::new(0.0, 0.0, s.screen.0, s.screen.1), DHit::Form(FHit::Shut), "");
    let h = items.len() as f32 * ITEM_H + 6.0;
    let below = Rect::new(b.x, b.bottom() + 2.0, b.w.max(120.0), h);
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
        let col = if i == now { if lemon { t.brand_sel(brand::MAIN) } else { t.glyph } } else { t.text };
        ui.c.text_clip(font, row.x + 10.0, (row.cy() - lh / 2.0).round(), row.w - 16.0, col, text.as_ref());
    }
}
