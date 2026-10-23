// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's small tool dialogs, laid out as FL's: Flam ("Piano roll - flamer", 198 × 241), Flip ("Piano roll -
//! score flipper", 162 × 160) and Scale levels ("Piano roll - level scaling", 172 × 277). One window shows whichever
//! is open (`piano_dlg::Ui::tool`).
// spec: ui/piano-flam.md §1, ui/piano-flip.md §1, ui/piano-level-scaling.md §1

use os_ui::kit::{self, lemon as lk};
use os_ui::{Font, Glyph, Rect};

use super::strum::{label, led};
use super::{FrameInfo, Look};
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::knobs::KnobId;
use crate::piano_dlg::{Act, Knob, Tool};

/// Each dialog's size, by `Tool::index`.
const SIZES: [(f32, f32); 3] = [(198.0, 241.0), (162.0, 160.0), (172.0, 277.0)];

pub fn size(tool: Tool) -> (f32, f32) {
    SIZES[tool.index()]
}

pub fn rect(s: &State) -> Rect {
    let d = &s.piano_dlg;
    let (pos, (w, h)) = (d.pos[d.tool.index()], size(d.tool));
    Rect::new(pos.0, pos.1, w, h)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

/// The level faders' zero line (window-relative): Center's travel ends there, Multiply's and Offset's centre on it.
const ZERO_Y: f32 = 135.0;

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let (x0, y0) = (win.x, win.y);
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::new(x0 + x, y0 + y, w, h);
    let lemon = super::lemon(ui);
    let d = &s.piano_dlg;
    let (w, h) = size(d.tool);
    let (bg, line, panel) = (t.s(0x2f3335), t.s(0x16191a), t.s(0x272b2d));
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), line, bg);
    } else {
        ui.c.fill_rect(win, line);
        ui.c.fill_rect(win.inset(1.0), bg);
    }
    ui.hit(win, Action::Swallow, "");
    ui.hit(at(1.0, 1.0, w - 30.0, 26.0), Action::WinDrag(Win::PianoDlg), "");
    // FL's narrow dialogs: the title condensed to end before ×.
    let title = ui.c.fit_font(Font::new(17), d.tool.title(), w - 31.0);
    label(ui, title, x0 + 7.0, y0 + 15.0, -1, t.text, d.tool.title());
    let close = at(w - 20.0, 10.0, 10.0, 10.0);
    let hov = ui.hit(close.outset(4.0), Action::CloseWin(Win::PianoDlg), "Close (puts the notes back)");
    icons::draw(ui.c, Icon::Close, close.outset(1.0), if hov { t.tx(0xffffff) } else { t.glyph });

    let body = at(7.0, 28.0, w - 14.0, h - 71.0);
    if lemon {
        ui.c.stroke_rrect(body, lk::r_small(t), 1.0, line);
    } else {
        ui.c.fill_rect(body, line);
        ui.c.fill_rect(body.inset(1.0), panel);
    }
    match d.tool {
        Tool::Flam => flam(ui, s, x0, y0),
        Tool::Flip => flip(ui, s, x0, y0),
        Tool::Levels => levels(ui, s, x0, y0),
    }

    // Reset, Accept.
    let lf = Font::new(15);
    for (r, act, name, hint) in [(at(7.0, h - 35.0, 60.0, 28.0), Act::Reset, "Reset", "Reset every control"), (at(w - 67.0, h - 35.0, 60.0, 28.0), Act::Accept, "Accept", "Accept the change")] {
        let hov = ui.hit(r, Action::PianoDlg(act), hint);
        super::button_face(ui, r, if hov { Look::Hover } else { Look::Off });
        label(ui, lf, r.cx(), r.cy(), 0, if hov { t.tx(0xffffff) } else { t.text }, name);
    }
}

/// A round switch with its label right of it (`x`, `cy`: the switch's centre, window-relative).
#[allow(clippy::too_many_arguments)]
fn switch(ui: &mut Ui, s: &State, x0: f32, y0: f32, x: f32, cy: f32, act: Act, name: &str, hint: &'static str, f: Font, off: bool) {
    let t = ui.t;
    let w = ui.c.text_width(f, name) + 22.0;
    let hov = !off && ui.hit(Rect::new(x0 + x - 9.0, y0 + cy - 9.0, w, 18.0), Action::PianoDlg(act), hint);
    led(ui, x0 + x, y0 + cy, s.piano_dlg.on(act), hov, off);
    label(ui, f, x0 + x + 11.0, y0 + cy, -1, if off { t.text_faint } else if hov { t.tx(0xffffff) } else { t.text }, name);
}

/// A knob with its hint (`cx`, `cy` window-relative).
fn knob(ui: &mut Ui, s: &State, x0: f32, y0: f32, cx: f32, cy: f32, k: Knob, bipolar: bool) {
    let t = ui.t;
    let id = KnobId::PianoDlg(k);
    let (cx, cy) = (x0 + cx, y0 + cy);
    ui.hit(Rect::new(cx - 17.0, cy - 17.0, 34.0, 34.0), Action::Knob(id), s.knob_text(id));
    super::knob(ui, cx, cy, 15.0, s.knob(id), bipolar, t.text, t.text_dim);
}

/// Flam (piano-flam.md §1): Options ▸, Time, Absolute, Before, Velocity, Group notes.
fn flam(ui: &mut Ui, s: &State, x0: f32, y0: f32) {
    let t = ui.t;
    let lf = Font::new(15);
    // The Options tab: a slanted right edge, ▸ and its name.
    let r = Rect::new(x0 + 8.0, y0 + 29.0, 68.0, 18.0);
    let hov = ui.hit(r, Action::PianoDlg(Act::Options), "Presets");
    let mut g = Glyph::new();
    g.polygon(&[(r.x, r.y), (r.right(), r.y), (r.right() - 6.0, r.bottom()), (r.x, r.bottom())]);
    ui.c.glyph(if hov { t.s(0x3a3e41) } else { t.s(0x33373a) }, &g);
    let ink = if hov { t.tx(0xffffff) } else { t.text };
    let mut g = Glyph::new();
    g.polygon(&[(r.x + 5.0, r.cy() - 3.5), (r.x + 9.0, r.cy()), (r.x + 5.0, r.cy() + 3.5)]);
    ui.c.glyph(ink, &g);
    label(ui, lf, r.x + 14.0, r.cy(), -1, ink, "Options");

    label(ui, lf, x0 + 60.0, y0 + 73.0, 1, t.text, "Time");
    knob(ui, s, x0, y0, 82.0, 73.0, Knob::FlamTime, true);
    label(ui, lf, x0 + 60.0, y0 + 127.0, 1, t.text, "Velocity");
    knob(ui, s, x0, y0, 82.0, 127.0, Knob::FlamVel, false);
    switch(ui, s, x0, y0, 122.0, 64.0, Act::Absolute, "Absolute", "Absolute / relative time", lf, false);
    switch(ui, s, x0, y0, 122.0, 84.0, Act::Before, "Before", "Insert flam before original drum", lf, false);
    switch(ui, s, x0, y0, 20.0, 183.0, Act::Group, "Group notes", "Group notes", Font::new(13), false);
}

/// Flip (piano-flip.md §1): Flip horizontally, Preserve start times (greyed while that is off), Flip vertically.
fn flip(ui: &mut Ui, s: &State, x0: f32, y0: f32) {
    let t = ui.t;
    let lf = Font::new(15);
    let set = &s.piano_dlg.flip;
    switch(ui, s, x0, y0, 21.0, 45.0, Act::Horizontal, "Flip horizontally", "Mirror the notes in time", lf, false);
    switch(ui, s, x0, y0, 21.0, 100.0, Act::Vertical, "Flip vertically", "Mirror the notes in pitch", lf, false);
    // Preserve start times: right-aligned, its switch after the label.
    let (f, off) = (Font::new(13), !set.horizontal);
    let w = ui.c.text_width(f, "Preserve start times");
    let hov = !off && ui.hit(Rect::new(x0 + 117.0 - w, y0 + 52.0, w + 19.0, 18.0), Action::PianoDlg(Act::Preserve), "Keep the spacing of the note starts");
    label(ui, f, x0 + 117.0, y0 + 61.0, 1, if off { t.text_faint } else if hov { t.tx(0xffffff) } else { t.text }, "Preserve start times");
    led(ui, x0 + 126.0, y0 + 61.0, set.preserve, hov, off);
}

/// Scale levels (piano-level-scaling.md §1): the Center, Multiply and Offset faders and the Tension knob.
fn levels(ui: &mut Ui, s: &State, x0: f32, y0: f32) {
    let t = ui.t;
    let lf = Font::new(15);
    let lemon = super::lemon(ui);
    let orange = if lemon { t.accent } else { t.h(0xf0a030) };
    // (centre x, track top and bottom, its value, its name)
    for (cx, top, bottom, k, name) in [(38.0, 57.0, 157.0, Knob::Center, "Center"), (87.0, 57.0, 213.0, Knob::Mult, "Multiply"), (134.0, 57.0, 213.0, Knob::Offset, "Offset")] {
        let (cx, top, bottom) = (x0 + cx, y0 + top, y0 + bottom);
        label(ui, lf, cx, y0 + 40.0, 0, t.text, name);
        let travel = k.travel().unwrap_or(72.0);
        let zero = y0 + ZERO_Y;
        // Marks every 9 px of the travel, on both sides of the track; the zero mark longer.
        let (lo, hi) = if k == Knob::Center { (zero - travel, zero) } else { (zero - travel / 2.0, zero + travel / 2.0) };
        let mut y = lo;
        while y <= hi + 0.5 {
            let len = if (y - zero).abs() < 0.5 { 18.0 } else { 14.0 };
            ui.c.fill_rect(Rect::new(cx - 6.0 - len, y.round(), len, 1.0), t.s(0x202325));
            ui.c.fill_rect(Rect::new(cx + 6.0, y.round(), len, 1.0), t.s(0x202325));
            y += 9.0;
        }
        if lemon {
            ui.c.fill_rrect(Rect::new(cx - 2.0, top, 4.0, bottom - top), 2.0, t.s(0x141618));
        } else {
            ui.c.fill_rect(Rect::new(cx - 1.5, top, 3.0, bottom - top), t.s(0x141618));
        }
        let id = KnobId::PianoDlg(k);
        let v = s.knob(id);
        let hy = if k == Knob::Center { zero - v * travel } else { zero - (v - 0.5) * travel };
        // Multiply and Offset: an orange bar from zero to the handle and a line through it (FL's look).
        if k != Knob::Center {
            let (a, b) = if hy < zero { (hy, zero) } else { (zero, hy) };
            ui.c.fill_rect(Rect::new(cx - 1.0, a, 2.0, b - a), orange);
        }
        let hov = ui.hit(Rect::new(cx - 11.0, top - 5.0, 22.0, bottom - top + 10.0), Action::Knob(id), s.knob_text(id));
        kit::fader_handle(ui.c, t, cx, hy, false, kit::State::hover(hov));
        if k != Knob::Center {
            ui.c.fill_rect(Rect::new(cx - 20.0, hy.round(), 40.0, 1.0), orange);
        }
    }
    knob(ui, s, x0, y0, 38.0, 190.0, Knob::Tension, true);
    label(ui, lf, x0 + 38.0, y0 + 221.0, 0, t.text, "Tension");
}
