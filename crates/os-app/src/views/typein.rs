// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The value box of "Type in value...": field, mode buttons, ✓ / ✗ and caption.
// spec: ui/type-in-value.md §2-3 (the measured layout and shades)

use os_ui::{Font, Rect};

use crate::app::{Action, State, Ui};
use crate::playlist::TextEdit;
use crate::typein::{self, Target, TiAct, H};

pub fn paint(ui: &mut Ui, s: &State, e: &TextEdit, t: Target, norm: bool) {
    let th = ui.t;
    let r = Rect::new(e.at.0, e.at.1, typein::width(t), H);
    ui.hit(r, Action::TypeIn(TiAct::Field), "Type, Enter accepts, Esc cancels");
    ui.c.fill_rect(r, th.s(0x1b1d1e));
    ui.c.stroke_rrect(r, 0.0, 1.0, th.s(0x607f8f));

    // The field: the text, its selection or the caret.
    let font = Font::new(17);
    let max_w = r.w - 52.0;
    let tw = ui.c.text_width(font, &e.text).min(max_w);
    if e.sel && !e.text.is_empty() {
        ui.c.fill_rect(Rect::new(r.x + 5.0, r.y + 4.0, tw + 2.0, 24.0), th.s(0x315d8a));
    }
    ui.c.text_clip(font, r.x + 6.0, r.y + 6.0, max_w, th.text, &e.text);
    if !e.sel {
        ui.c.fill_rect(Rect::new(r.x + 7.0 + tw, r.y + 6.0, 1.0, 19.0), th.text);
    }

    // Mode buttons: the knob's unit, then "A" (normalized).
    if let Some((label, name)) = typein::mode_button(t) {
        let buttons = [(label, format!("Type in value as: {name}"), false), ("A", "Type in normalized value (0-1) or %".to_string(), true)];
        for (i, (label, hint, is_norm)) in buttons.into_iter().enumerate() {
            let b = Rect::new(r.x + 202.0 + 19.0 * i as f32, r.y + 3.0, 18.0, 17.0);
            let hov = ui.hit(b, Action::TypeIn(TiAct::Mode(is_norm)), hint);
            let on = norm == is_norm;
            ui.c.fill_rect(b, th.s(if hov { 0x2a2d2e } else { 0x1b1d1e }));
            ui.c.stroke_rrect(b, 0.0, 1.0, th.s(if on { 0x4a4d4f } else { 0x2a2c2d }));
            ui.c.text_center(Font::new(11).bold(), b, if on { th.text } else { th.text.shade(0.3) }, label);
        }
    }

    // ✓, ✗ and the caption.
    let dim = th.tx(0x6a6d6e);
    let ok = Rect::new(r.x + 3.0, r.y + 30.0, 16.0, 18.0);
    let ok_hov = ui.hit(ok, Action::TypeIn(TiAct::Accept), "Accept");
    let (cx, cy) = (ok.cx(), ok.cy());
    ui.c.polyline(&[(cx - 4.5, cy), (cx - 1.5, cy + 3.5), (cx + 4.5, cy - 4.0)], 1.5, if ok_hov { th.text } else { dim });
    let close = Rect::new(r.x + 23.0, r.y + 30.0, 16.0, 18.0);
    let close_hov = ui.hit(close, Action::TypeIn(TiAct::Close), "Close (Esc)");
    let (cx, cy, c) = (close.cx(), close.cy(), if close_hov { th.text } else { dim });
    ui.c.line(cx - 4.0, cy - 4.0, cx + 4.0, cy + 4.0, 1.5, c);
    ui.c.line(cx - 4.0, cy + 4.0, cx + 4.0, cy - 4.0, 1.5, c);
    let cap = typein::caption(s, t);
    ui.c.text_clip(Font::new(13), r.x + 44.0, r.y + 31.0, r.w - 50.0, dim, &cap);
}
