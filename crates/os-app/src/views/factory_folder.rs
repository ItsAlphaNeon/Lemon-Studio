// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Settings ▸ File ▸ FL Studio factory content folder, drawn as the User data folder group is laid out (Reset, the
//! folder button and the path, spec: ui/settings.md "File"). What it does: `crate::factory_folder`.
// spec: flp/sample-paths.md "For Lemon Studio"

use os_ui::{Font, Rect};

use super::settings::{grey, SAct};
use crate::app::{Action, Ui};
use crate::factory_folder::{self as ff, FOLDER_HINT, RESET, RESET_HINT, RESET_LABEL, SET_FOLDER};

/// The height of the row (`Row::FactoryFolder`).
pub const ROW_H: f32 = super::data_folder::FOLDER_ROW_H;

/// Draws the group's row (`g` the group's body, `x0` where its controls start, `y`..`y + h` the row).
pub fn paint_row(ui: &mut Ui, g: Rect, x0: f32, y: f32, h: f32) {
    let t = ui.t;
    let f = Font::new(13);
    let ty = (y + h / 2.0 - ui.c.line_height(f) / 2.0).round();
    // Reset: greyed while no folder is chosen (the install found on this computer is in use).
    let b = Rect::new(x0, y + 4.0, 76.0, 24.0);
    let on = ff::chosen().is_some();
    let hov = if on {
        ui.hit(b, Action::Settings(SAct::Button(RESET)), RESET_HINT)
    } else {
        // Greyed: the hint shows, a click acts as on the page's body.
        ui.hit(b, Action::Settings(SAct::Body), RESET_HINT);
        false
    };
    super::data_folder::button_face(ui, b, hov);
    ui.c.text_center(f, b, if hov { t.glyph } else { t.text }, RESET_LABEL);
    if !on {
        grey(ui, b);
    }
    // The folder button and the path beside it both pick the folder.
    let fb = Rect::new(b.right() + 8.0, y + 4.0, 26.0, 24.0);
    let path = ff::path_text();
    let pr = Rect::new(fb.right() + 8.0, y + 4.0, (g.right() - 12.0 - (fb.right() + 8.0)).max(0.0), 24.0);
    let hov_f = ui.hit(fb, Action::Settings(SAct::Button(SET_FOLDER)), FOLDER_HINT);
    let pw = ui.c.text_width(f, &path).min(pr.w);
    let hov_p = ui.hit(Rect::new(pr.x, pr.y, pw, pr.h), Action::Settings(SAct::Button(SET_FOLDER)), FOLDER_HINT);
    let lit = hov_f || hov_p;
    super::data_folder::button_face(ui, fb, lit);
    let icon = Rect::new(fb.cx() - 6.5, fb.cy() - 6.5, 13.0, 13.0);
    crate::icons::draw(ui.c, crate::icons::Icon::Folder, icon, if lit { t.glyph } else { t.text });
    ui.c.text_clip(f, pr.x, ty, pr.w, if lit { t.glyph } else { t.text }, &path);
}
