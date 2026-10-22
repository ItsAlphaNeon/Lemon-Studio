// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Strum dialog ("Piano roll - strumizer"): the Start and End sections, Chop chords, Alternate
//! direction, Reset and Accept, laid out as FL's (312 × 219).
// spec: ui/piano-strum.md §1

use os_ui::kit::lemon as lk;
use os_ui::{Color, Font, Rect};

use super::{FrameInfo, Look};
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::knobs::KnobId;
use crate::strum::{Act, Knob};

pub const SIZE: (f32, f32) = (312.0, 219.0);

pub fn rect(s: &State) -> Rect {
    Rect::new(s.strum.pos.0, s.strum.pos.1, SIZE.0, SIZE.1)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

/// A round switch (FL: dark, red-orange when on; Lemon: the kit's LED, an outline when greyed out).
fn led(ui: &mut Ui, cx: f32, cy: f32, on: bool, hover: bool, off: bool) {
    let t = ui.t;
    if super::lemon(ui) {
        if off {
            let h = 6.5;
            ui.c.stroke_rrect(Rect::new(cx - h, cy - h, 2.0 * h, 2.0 * h), 3.0, 1.0, t.s(0x34383a));
        } else {
            lk::led(ui.c, t, cx, cy, 7.0, os_ui::kit::State::new(hover, false, on));
        }
        return;
    }
    ui.c.fill_circle(cx, cy, 7.0, t.s(0x16191a));
    if on && !off {
        ui.c.fill_circle(cx, cy, 6.0, t.h(0xe8461c));
        ui.c.fill_circle(cx, cy, 4.0, t.h(0xff6a35));
        ui.c.fill_circle(cx - 1.4, cy - 1.6, 1.8, t.h(0xffc2a0));
    } else {
        ui.c.fill_circle(cx, cy, 6.0, if hover && !off { t.s(0x464a4d) } else { t.s(0x33373a) });
        ui.c.fill_circle(cx, cy, 4.0, t.s(0x2a2d30));
    }
}

/// Text vertically centred on `cy`, from `x` (left), centred on it or ending at it (`align` -1, 0, 1).
fn label(ui: &mut Ui, f: Font, x: f32, cy: f32, align: i8, col: Color, s: &str) {
    let lh = ui.c.line_height(f);
    let w = ui.c.text_width(f, s);
    let x = match align {
        0 => x - w / 2.0,
        1 => x - w,
        _ => x,
    };
    ui.c.text(f, x.round(), (cy - lh / 2.0).round(), col, s);
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let (x0, y0) = (win.x, win.y);
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::new(x0 + x, y0 + y, w, h);
    let lemon = super::lemon(ui);
    let set = &s.strum.set;
    let (bg, line, panel) = (t.s(0x2f3335), t.s(0x16191a), t.s(0x272b2d));
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), line, bg);
    } else {
        ui.c.fill_rect(win, line);
        ui.c.fill_rect(win.inset(1.0), bg);
    }
    ui.hit(win, Action::Swallow, "");
    ui.hit(at(1.0, 1.0, SIZE.0 - 30.0, 26.0), Action::WinDrag(Win::Strum), "");
    label(ui, Font::new(17), x0 + 7.0, y0 + 15.0, -1, t.text, "Piano roll - strumizer");
    let close = at(292.0, 10.0, 10.0, 10.0);
    let hov = ui.hit(close.outset(4.0), Action::CloseWin(Win::Strum), "Close (puts the notes back)");
    icons::draw(ui.c, Icon::Close, close.outset(1.0), if hov { t.tx(0xffffff) } else { t.glyph });

    let small = Font::new(9);
    let lf = Font::new(15);
    // A section: its panel and the LED that turns it on.
    let section = |ui: &mut Ui, r: Rect, on: bool, act: Act, name: &str, hint: &'static str| {
        if lemon {
            ui.c.stroke_rrect(r, lk::r_small(t), 1.0, line);
        } else {
            ui.c.fill_rect(r, line);
            ui.c.fill_rect(r.inset(1.0), panel);
        }
        let hov = ui.hit(Rect::new(r.x + 2.0, r.y + 1.0, 60.0, 16.0), Action::Strum(act), hint);
        led(ui, r.x + 10.0, r.y + 8.0, on, hov, false);
        label(ui, lf, r.x + 20.0, r.y + 8.0, -1, if hov { t.tx(0xffffff) } else { t.text }, name);
    };
    section(ui, at(8.0, 30.0, 146.0, 184.0), set.start, Act::Start, "Start", "Start: strum the note starts and velocities");
    section(ui, at(160.0, 30.0, 146.0, 92.0), set.end, Act::End, "End", "End: strum the note ends");

    // Knobs (§1): strengths orange, tensions green on FL; greyed with their section.
    let knob = |ui: &mut Ui, cx: f32, cy: f32, k: Knob, off: bool| {
        let id = KnobId::Strum(k);
        let (cx, cy) = (x0 + cx, y0 + cy);
        if !off {
            ui.hit(Rect::new(cx - 14.0, cy - 14.0, 28.0, 28.0), Action::Knob(id), s.knob_text(id));
        }
        super::knob(ui, cx, cy, 12.0, s.knob(id), true, t.text, t.text_dim);
        if off {
            ui.veil(Rect::new(cx - 14.0, cy - 14.0, 28.0, 28.0), panel);
        }
    };
    let (soff, eoff) = (!set.start, !set.end);
    let head = |ui: &mut Ui, x: f32, txt: &str, off: bool| label(ui, small, x0 + x, y0 + 57.0, 0, if off { t.text_faint } else { t.text_dim }, txt);
    head(ui, 81.0, "STRENGTH", soff);
    head(ui, 121.0, "TENSION", soff);
    head(ui, 233.0, "STRENGTH", eoff);
    head(ui, 274.0, "TENSION", eoff);
    let side = |off: bool| if off { t.text_faint } else { t.text };
    label(ui, lf, x0 + 59.0, y0 + 87.0, 1, side(soff), "Time");
    label(ui, lf, x0 + 59.0, y0 + 132.0, 1, side(soff), "Velocity");
    label(ui, lf, x0 + 211.0, y0 + 87.0, 1, side(eoff), "Time");
    knob(ui, 81.0, 87.0, Knob::Time, soff);
    knob(ui, 121.0, 87.0, Knob::TimeTension, soff);
    knob(ui, 81.0, 132.0, Knob::Vel, soff);
    knob(ui, 121.0, 132.0, Knob::VelTension, soff);
    knob(ui, 233.0, 87.0, Knob::EndTime, eoff);
    knob(ui, 274.0, 87.0, Knob::EndTension, eoff);

    // Checks.
    let check = |ui: &mut Ui, x: f32, cy: f32, on: bool, act: Act, name: &str, hint: &'static str, off: bool| {
        let hov = !off && ui.hit(at(x - 9.0, cy - 9.0, 130.0, 18.0), Action::Strum(act), hint);
        led(ui, x0 + x, y0 + cy, on, hov, off);
        label(ui, lf, x0 + x + 11.0, y0 + cy, -1, if off { t.text_faint } else if hov { t.tx(0xffffff) } else { t.text }, name);
    };
    check(ui, 21.0, 174.0, set.preserve_end, Act::PreserveEnd, "Preserve end", "Preserve end: keep the note ends where they are", soff);
    check(ui, 21.0, 193.0, set.ahead, Act::Ahead, "Trigger ahead", "Trigger ahead: the strum ends on the chord's start", soff);
    check(ui, 173.0, 142.0, set.chop, Act::Chop, "Chop chords", "Chop chords: notes still held where others start are cut there", false);
    check(ui, 173.0, 162.0, set.alternate, Act::Alternate, "Alternate direction", "Alternate direction: every second chord strums the other way", false);

    // Reset, Accept.
    for (r, act, name, hint) in [(at(160.0, 180.0, 58.0, 33.0), Act::Reset, "Reset", "Reset every control"), (at(247.0, 180.0, 58.0, 33.0), Act::Accept, "Accept", "Accept the strum")] {
        let hov = ui.hit(r, Action::Strum(act), hint);
        super::button_face(ui, r, if hov { Look::Hover } else { Look::Off });
        label(ui, lf, r.cx(), r.cy(), 0, if hov { t.tx(0xffffff) } else { t.text }, name);
    }
}
