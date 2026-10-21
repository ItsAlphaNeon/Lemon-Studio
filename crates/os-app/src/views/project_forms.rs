// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project's forms (`crate::project_forms`), painted as dialogs in the message boxes' frame: Rename project (Save as
//! and New project: `views::save_form`).
// spec: ui/data-folder.md §6

use os_ui::{kit, Font, Rect};

use super::dialog::{button, field, frame, hit, panel, place, BTN_H, BTN_W};
use crate::app::{Action, State, Ui};
use crate::dialog::{DHit, Field, Modal};
use crate::project_forms::{FHit, Form, Rename};

/// Paints form `f` of dialog `m`.
pub fn paint(ui: &mut Ui, s: &State, m: &Modal, f: &Form) {
    match f {
        Form::Rename(f) => rename(ui, s, m, f),
        Form::Save(f) => super::save_form::paint(ui, s, m, f),
        Form::Export(f) => super::export_form::paint(ui, s, m, f),
    }
}

/// A label at (x, y).
pub(super) fn label(ui: &mut Ui, x: f32, y: f32, text: &str, enabled: bool) {
    let t = ui.t;
    ui.c.text(Font::new(13), x, y, if enabled { t.text } else { t.text_faint }, text);
}

/// A check at left edge `x`, centred on `cy`: a small box, ticked when `on`, and its label; greyed when not `enabled`.
pub(super) fn check(ui: &mut Ui, x: f32, cy: f32, text: &str, on: bool, enabled: bool, h: FHit, hint: &'static str) {
    let t = ui.t;
    let f = Font::new(13);
    let w = 20.0 + ui.c.text_width(f, text);
    let hov = enabled && hit(ui, Rect::new(x, cy - 10.0, w, 20.0), DHit::Form(h), hint);
    let b = Rect::new(x, (cy - 6.5).round(), 13.0, 13.0);
    ui.c.fill_rrect(b, 2.0, t.s(0x141516));
    ui.c.stroke_rrect(b, 2.0, 1.0, if hov { t.glyph_dim } else { t.text_dim });
    if on {
        kit::tick(ui.c, b, if enabled { t.glyph } else { t.text_faint });
    }
    let lh = ui.c.line_height(f);
    ui.c.text(f, x + 20.0, (cy - lh / 2.0).round(), if !enabled { t.text_faint } else if hov { t.glyph } else { t.text }, text);
}

/// A text field that cannot be typed in now: its text, greyed.
fn field_off(ui: &mut Ui, r: Rect, text: &str) {
    let mut fl = Field::default();
    fl.set(text);
    field(ui, r, &fl, false, DHit::Body, "");
    let bg = ui.t.s(0x282c2d);
    ui.veil(r, bg);
}

/// The bottom right buttons: the main one (`main`), then Cancel.
fn buttons(ui: &mut Ui, r: Rect, main: &str, main_hint: &'static str) {
    let f = Font::new(13);
    let mw = (ui.c.text_width(f, main) + 28.0).max(BTN_W).round();
    let cw = (ui.c.text_width(f, "Cancel") + 28.0).max(BTN_W).round();
    let by = r.bottom() - 12.0 - BTN_H;
    let b = Rect::new(r.right() - 12.0 - cw - 6.0 - mw, by, mw, BTN_H);
    let act = Action::Dlg(DHit::Form(FHit::Main));
    let down = ui.down(b, &act);
    let hov = ui.hit(b, act, main_hint);
    button(ui, b, main, true, hov, down, false);
    let c = Rect::new(b.right() + 6.0, by, cw, BTN_H);
    let act = Action::Dlg(DHit::Cancel);
    let down = ui.down(c, &act);
    let hov = ui.hit(c, act, "Close without renaming (Esc)");
    button(ui, c, "Cancel", false, hov, down, false);
}

/// Rename project, FL's 469 x 246 (spec: ui/data-folder.md §6).
fn rename(ui: &mut Ui, s: &State, m: &Modal, f: &Rename) {
    let r = place(s, m, 469.0, 246.0);
    frame(ui, r, Rename::TITLE);
    panel(ui, Rect::new(r.x + 5.0, r.y + 28.0, r.w - 10.0, r.h - 28.0 - 55.0));
    let (x, w) = (r.x + 16.0, r.w - 32.0);
    label(ui, x, r.y + 38.0, Rename::FIELDS[0], true);
    field(ui, Rect::new(x, r.y + 56.0, w, 24.0), &f.name, f.focus == 0, DHit::Form(FHit::Field(0)), "The project's new name");
    check(ui, x, r.y + 100.0, Rename::CHECKS[0], f.rename_folder, true, FHit::Check(0), "Rename the data folder too");
    let fr = Rect::new(x + 20.0, r.y + 114.0, w - 20.0, 24.0);
    if f.folder_editable() {
        field(ui, fr, &f.folder, f.focus == 1, DHit::Form(FHit::Field(1)), "The data folder's new name");
    } else {
        field_off(ui, fr, if f.rename_folder { f.folder_name() } else { &f.folder.text });
    }
    check(ui, x + 20.0, r.y + 152.0, Rename::CHECKS[1], f.use_name, f.rename_folder, FHit::Check(1), "Name the data folder as the project");
    buttons(ui, r, Rename::MAIN, "Rename the project (Enter)");
}
