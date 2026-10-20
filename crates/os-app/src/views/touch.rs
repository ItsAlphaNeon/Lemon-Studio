// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Touch controller window: the title with the Keys / Pads switch, the octave bar, and the pads or the keyboard.
// spec: ui/touch-controller.md §2 (Lemon: ui/lemon-design.md)

use os_theme::design::lemon as brand;
use os_ui::kit::{self, lemon as lk};
use os_ui::{Color, Font, Rect};

use super::FrameInfo;
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::touch::{is_black, pad_note, TouchAct, OCTAVES, PADS};

pub const SIZE: (f32, f32) = (420.0, 300.0);
/// The pads or keys, from the window's top-left.
const AREA: Rect = Rect::new(8.0, 64.0, 404.0, 228.0);
const GAP: f32 = 6.0;

pub fn rect(s: &State) -> Rect {
    Rect::new(s.touch.pos.0, s.touch.pos.1, SIZE.0, SIZE.1)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

/// Text vertically centred on `cy` from `x` (or centred on `x` when `center`).
fn label(ui: &mut Ui, f: Font, x: f32, cy: f32, center: bool, col: Color, s: &str) {
    let lh = ui.c.line_height(f);
    let y = (cy - lh / 2.0).round();
    let x = if center { (x - ui.c.text_width(f, s) / 2.0).round() } else { x };
    ui.c.text(f, x, y, col, s);
}

/// A button showing `text` or an icon: lit while `on`.
fn button(ui: &mut Ui, r: Rect, a: Action, hint: &'static str, face: Result<&str, Icon>, on: bool) {
    let hov = ui.hit(r, a.clone(), hint);
    let down = ui.down(r, &a);
    let t = ui.t;
    kit::button(ui.c, t, r, kit::State::new(hov, down, on));
    let ink = if on && super::lemon(ui) { lk::ink_on_lit(t) } else if hov { t.tx(0xffffff) } else { t.text };
    match face {
        Ok(text) => label(ui, Font::new(14), r.cx(), r.cy(), true, ink, text),
        Err(icon) => icons::draw(ui.c, icon, Rect::new(r.cx() - 7.0, r.cy() - 7.0, 14.0, 14.0), ink),
    }
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let (x0, y0) = (win.x, win.y);
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::new(x0 + x, y0 + y, w, h);
    let lemon = super::lemon(ui);
    let tc = &s.touch;
    let (bg, line) = (t.s(0x202324), t.s(0x16191a));
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), line, bg);
    } else {
        ui.c.fill_rect(win, line);
        ui.c.fill_rect(win.inset(1.0), bg);
    }
    ui.hit(win, Action::Swallow, "");
    ui.hit(at(1.0, 1.0, SIZE.0 - 2.0, 29.0), Action::WinDrag(Win::Touch), "");
    label(ui, Font::new(17), x0 + 9.0, y0 + 15.0, false, t.text, "Touch controller");
    button(ui, at(250.0, 5.0, 60.0, 21.0), Action::Touch(TouchAct::Mode(false)), "Keyboard", Ok("Keys"), !tc.pads);
    button(ui, at(314.0, 5.0, 60.0, 21.0), Action::Touch(TouchAct::Mode(true)), "Pads", Ok("Pads"), tc.pads);
    let close = at(400.0, 10.0, 10.0, 10.0);
    let hov = ui.hit(close.outset(3.0), Action::CloseWin(Win::Touch), "Close");
    icons::draw(ui.c, Icon::Close, close.outset(1.0), if hov { t.tx(0xffffff) } else { t.glyph });

    // The octave bar: ◀ range ▶.
    button(ui, at(8.0, 34.0, 32.0, 24.0), Action::Touch(TouchAct::Octave(-1)), "Octave down", Err(Icon::ArrowLeft), false);
    button(ui, at(380.0, 34.0, 32.0, 24.0), Action::Touch(TouchAct::Octave(1)), "Octave up", Err(Icon::ArrowRight), false);
    let (low, n) = tc.range();
    let name = |k: u8| crate::note_names::note_name(i32::from(k));
    let range = format!("{} \u{2013} {}", name(low), name(low + n - 1));
    label(ui, Font::new(15), x0 + SIZE.0 / 2.0, y0 + 46.0, true, t.text_dim, &range);

    let held = tc.held.map(|h| h.0);
    let lit = if lemon { t.brand_lit(brand::MAIN) } else { t.s(0xbdbdbd) };
    let area = at(AREA.x, AREA.y, AREA.w, AREA.h);
    if tc.pads {
        let n = f32::from(PADS);
        let (w, h) = ((area.w - GAP * (n - 1.0)) / n, (area.h - GAP * (n - 1.0)) / n);
        for row in 0..PADS {
            for col in 0..PADS {
                let k = pad_note(tc.pad_root, col, row);
                let r = Rect::new(area.x + f32::from(col) * (w + GAP), area.bottom() - h - f32::from(row) * (h + GAP), w, h);
                ui.hit(r, Action::Touch(TouchAct::Note(k)), format!("Touch controller: {}", name(k)));
                let on = held == Some(k);
                // Violet at the bottom to cyan at the top, as FL's pads (Lemon: the brand colour when held).
                let base = Color::rgb(0x55508f).mix(Color::rgb(0x3b8a99), f32::from(row) / (n - 1.0));
                let face = if on { lit } else { base };
                lk::outlined(ui.c, r, lk::r_btn(t), line, face);
                let ink = if on { lk::ink_on_lit(t) } else { t.tx(0xdfe6ea) };
                let text = name(k);
                let tw = ui.c.text_width(Font::new(14), &text);
                label(ui, Font::new(14), r.right() - tw - 8.0, r.bottom() - 12.0, false, ink, &text);
            }
        }
    } else {
        // Two octaves: the white keys, then the black keys over them.
        let whites: Vec<u8> = (tc.keys_low..tc.keys_low + 12 * OCTAVES).filter(|&k| !is_black(k)).collect();
        let w = area.w / whites.len() as f32;
        for (i, &k) in whites.iter().enumerate() {
            let r = Rect::new(area.x + i as f32 * w, area.y, w, area.h);
            ui.hit(r, Action::Touch(TouchAct::Note(k)), format!("Touch controller: {}", name(k)));
            let face = if held == Some(k) { lit } else if k % 12 == 0 { t.tx(0xb9bfc4) } else { t.tx(0xd3d8dc) };
            ui.c.fill_rect(r.inset(0.5), face);
            ui.c.fill_rect(Rect::new(r.right() - 1.0, r.y, 1.0, r.h), line);
            label(ui, Font::new(13), r.cx(), r.bottom() - 12.0, true, t.s(0x4b5155), &name(k));
        }
        let (bw, bh) = (w * 0.6, area.h * 0.6);
        for (i, &k) in whites.iter().enumerate() {
            let b = k + 1;
            if !is_black(b) || b >= tc.keys_low + 12 * OCTAVES {
                continue;
            }
            let r = Rect::new(area.x + (i + 1) as f32 * w - bw / 2.0, area.y, bw, bh);
            ui.hit(r, Action::Touch(TouchAct::Note(b)), format!("Touch controller: {}", name(b)));
            let face = if held == Some(b) { lit } else { t.s(0x1d2022) };
            ui.c.fill_rrect(r, 2.0, face);
        }
        ui.c.stroke_rrect(area, 0.0, 1.0, line);
    }
}
