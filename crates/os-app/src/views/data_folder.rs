// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Settings ▸ Project ▸ Data folder, drawn: the Data folder combo; Rename, the folder button and the path; Ask for data
//! folder; Copy used samples to the project folder. What they do: `crate::data_folder`.
// spec: ui/data-folder.md §1

use os_ui::kit::lemon as lk;
use os_ui::kit;
use os_ui::{Font, Rect};

use super::settings::{grey, led, Row, SAct};
use crate::app::{Action, State, Ui};
use crate::data_folder::{self as df, ASK_HINT, COMBO, COMBO_HINT, COPY_HINT, RENAME, RENAME_HINT, SET_FOLDER};
use crate::prefs::COPY_USED_SAMPLES;

/// The group's title: its rows are drawn here.
pub const GROUP: &str = "Data folder";

/// The height of the Rename row (`Row::DataFolder`).
pub const FOLDER_ROW_H: f32 = 32.0;

/// Whether the settings page's row in group `group` is drawn here.
pub fn shows(group: &str) -> bool {
    group == GROUP
}

/// Draws row `row` of the group (`g` the group's body, `x0` where its controls start, `y`..`y + h` the row).
#[allow(clippy::too_many_arguments)]
pub fn paint_row(ui: &mut Ui, s: &State, row: &Row, g: Rect, x0: f32, y: f32, h: f32) {
    let t = ui.t;
    let f = Font::new(13);
    let cy = y + h / 2.0;
    let ty = (cy - ui.c.line_height(f) / 2.0).round();
    match row {
        // The Data folder combo: the project's choice, no label beside it.
        Row::Combo(_, w, label) if *label == COMBO => {
            let b = Rect::new(x0, y + 3.0, *w, 21.0);
            let hov = ui.hit(b, Action::Settings(SAct::Combo(COMBO, b.x, b.bottom())), COMBO_HINT);
            kit::combo(ui.c, t, b, df::choice_text(&s.project), f, kit::State::hover(hov));
        }
        // Ask for data folder: the program setting's choice, its label on the right.
        Row::Combo(_, w, label) => {
            let b = Rect::new(x0, y + 3.0, *w, 21.0);
            let hov = ui.hit(b, Action::Settings(SAct::Combo(label, b.x, b.bottom())), ASK_HINT);
            let v = crate::prefs::find_combo(label).map_or("", crate::prefs::choice);
            kit::combo(ui.c, t, b, v, f, kit::State::hover(hov));
            ui.c.text(f, b.right() + 8.0, ty, if hov { t.glyph } else { t.text }, label);
        }
        Row::DataFolder => {
            // Rename: greyed without a data folder or for a project never saved.
            let b = Rect::new(x0, y + 4.0, 76.0, 24.0);
            let on = df::can_rename(s);
            let hov = if on {
                ui.hit(b, Action::Settings(SAct::Button(RENAME)), RENAME_HINT)
            } else {
                // Greyed: the hint shows, a click acts as on the page's body.
                ui.hit(b, Action::Settings(SAct::Body), RENAME_HINT);
                false
            };
            button_face(ui, b, hov);
            ui.c.text_center(f, b, if hov { t.glyph } else { t.text }, RENAME);
            if !on {
                grey(ui, b);
            }
            // The folder button and the path beside it both pick the folder.
            let fb = Rect::new(b.right() + 8.0, y + 4.0, 26.0, 24.0);
            let path = df::path_text(s);
            let pw = (g.right() - 12.0 - (fb.right() + 8.0)).max(0.0);
            let pr = Rect::new(fb.right() + 8.0, y + 4.0, pw, 24.0);
            let hov_f = ui.hit(fb, Action::Settings(SAct::Button(SET_FOLDER)), SET_FOLDER);
            let pw = ui.c.text_width(f, &path).min(pr.w);
            let hov_p = ui.hit(Rect::new(pr.x, pr.y, pw, pr.h), Action::Settings(SAct::Button(SET_FOLDER)), SET_FOLDER);
            button_face(ui, fb, hov_f || hov_p);
            let icon = Rect::new(fb.cx() - 6.5, fb.cy() - 6.5, 13.0, 13.0);
            crate::icons::draw(ui.c, crate::icons::Icon::Folder, icon, if hov_f || hov_p { t.glyph } else { t.text });
            ui.c.text_clip(f, pr.x, ty, pr.w, if hov_f || hov_p { t.glyph } else { t.text }, &path);
        }
        // Copy used samples to the project folder: greyed without a data folder.
        Row::Check(k, _) => {
            let x = x0 + 10.0;
            let tw = ui.c.text_width(f, k);
            let hr = Rect::new(x - 8.0, cy - 9.0, tw + 26.0, 18.0);
            let on = df::folder(s).is_some();
            let hov = if on {
                ui.hit(hr, Action::Settings(SAct::Check(COPY_USED_SAMPLES.label)), COPY_HINT)
            } else {
                ui.hit(hr, Action::Settings(SAct::Body), COPY_HINT);
                false
            };
            led(ui, x, cy, crate::prefs::check(&COPY_USED_SAMPLES), hov);
            ui.c.text(f, x + 11.0, ty, if hov { t.glyph } else { t.text }, k);
            if !on {
                grey(ui, hr);
            }
        }
        _ => {}
    }
}

/// A push button's face in `b` (the settings page's buttons).
fn button_face(ui: &mut Ui, b: Rect, hov: bool) {
    let t = ui.t;
    if super::lemon(ui) {
        lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
    } else {
        kit::bordered(ui.c, b, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
    }
}
