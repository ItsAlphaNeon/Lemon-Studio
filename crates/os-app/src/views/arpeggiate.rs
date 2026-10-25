// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Arpeggiate dialog ("Piano roll - arpeggiator"): the pattern box with its folder button and ▸ menu,
//! the Options (Pattern, Time mul, Range, Range mode, Sync, Gate), the Levels knobs, Group notes, Reset and Accept,
//! laid out as FL's (303 × 331).
// spec: ui/piano-arpeggiate.md §1

use os_ui::kit::{self, lemon as lk};
use os_ui::{Color, Font, Rect};

use super::{FrameInfo, Look};
use crate::app::{Action, State, Ui, Win};
use crate::arpeggiate::{Act, Knob, Menu, LEVELS};
use crate::icons::{self, Icon};
use crate::knobs::KnobId;

pub const SIZE: (f32, f32) = (303.0, 331.0);

/// The Levels knobs' ring colours (PAN … PITCH), as FL's.
pub(super) const LEVEL_COLOURS: [u32; 6] = [0x3cc8e8, 0x3ed89a, 0x52d84a, 0xd2dc3c, 0xf0a03c, 0xf04a64];

pub fn rect(s: &State) -> Rect {
    Rect::new(s.arp.pos.0, s.arp.pos.1, SIZE.0, SIZE.1)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
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
    let set = &s.arp.set;
    let (bg, line, panel) = (t.s(0x2f3335), t.s(0x16191a), t.s(0x272b2d));
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), line, bg);
    } else {
        ui.c.fill_rect(win, line);
        ui.c.fill_rect(win.inset(1.0), bg);
    }
    ui.hit(win, Action::Swallow, "");
    ui.hit(at(1.0, 1.0, SIZE.0 - 30.0, 26.0), Action::WinDrag(Win::Arp), "");
    label(ui, Font::new(17), x0 + 7.0, y0 + 14.0, -1, t.text, "Piano roll - arpeggiator");
    let close = at(284.0, 10.0, 10.0, 10.0);
    let hov = ui.hit(close.outset(4.0), Action::CloseWin(Win::Arp), "Close (puts the notes back)");
    icons::draw(ui.c, Icon::Close, close.outset(1.0), if hov { t.tx(0xffffff) } else { t.glyph });

    let lf = Font::new(15);
    let small = Font::new(9);
    let boxed = |ui: &mut Ui, r: Rect| {
        if lemon {
            ui.c.stroke_rrect(r, lk::r_small(t), 1.0, line);
        } else {
            ui.c.fill_rect(r, line);
            ui.c.fill_rect(r.inset(1.0), panel);
        }
    };

    // Pattern: its name, the folder button, the ▸ menu.
    boxed(ui, at(6.0, 28.0, 288.0, 28.0));
    label(ui, lf, x0 + 14.0, y0 + 42.0, -1, t.text_dim, "Pattern");
    label(ui, lf, x0 + 68.0, y0 + 42.0, -1, t.text, &s.arp.pattern.name);
    for (r, act, hint) in [(at(240.0, 31.0, 31.0, 22.0), Act::Folder, "Load a pattern file"), (at(273.0, 31.0, 19.0, 22.0), Act::Menu(Menu::Pattern), "Pick a pattern")] {
        let hov = ui.hit(r, Action::Arp(act), hint);
        super::button_face(ui, r, if hov { Look::Hover } else { Look::Off });
        let ink = if hov { t.tx(0xffffff) } else { t.glyph };
        if act == Act::Folder {
            icons::draw(ui.c, Icon::Folder, Rect::new(r.cx() - 8.0, r.cy() - 8.0, 16.0, 16.0), ink);
        } else {
            icons::draw(ui.c, Icon::ArrowRight, Rect::new(r.cx() - 6.0, r.cy() - 6.0, 12.0, 12.0), ink);
        }
    }

    // Options.
    boxed(ui, at(6.0, 63.0, 288.0, 224.0));
    label(ui, lf, x0 + 10.0, y0 + 72.0, -1, t.text, "Options");
    let menu = |ui: &mut Ui, y: f32, text: &str, m: Menu, hint: &'static str, off: bool| {
        let r = at(91.0, y - 9.0, 72.0, 18.0);
        let hov = !off && ui.hit(r, Action::Arp(Act::Menu(m)), hint);
        kit::combo(ui.c, t, r, text, Font::new(13), kit::State::new(hov, false, false).with_disabled(off));
    };
    let side = |ui: &mut Ui, x: f32, y: f32, s: &str, off: bool| label(ui, lf, x0 + x, y0 + y, 1, if off { t.text_faint } else { t.text }, s);
    side(ui, 68.0, 103.0, "Pattern", false);
    menu(ui, 103.0, set.order.name(), Menu::Order, "Pattern direction: Normal, Flip or Alternate", false);
    side(ui, 51.0, 134.0, "Range", false);
    let range = at(57.0, 124.0, 28.0, 20.0);
    let id = KnobId::Arp(Knob::Range);
    let hov = ui.hit(range, Action::Knob(id), s.knob_text(id));
    kit::combo(ui.c, t, range, &set.range_text(), Font::new(13), kit::State::new(hov, false, false));
    menu(ui, 134.0, set.range_order.name(), Menu::RangeOrder, "Range direction: Normal, Flip or Alternate", set.range == 1);
    side(ui, 51.0, 165.0, "Sync", false);
    menu(ui, 165.0, set.sync.name(), Menu::Sync, "Where the pattern restarts: Time, Block or Chord", false);

    let knob = |ui: &mut Ui, cx: f32, cy: f32, r: f32, k: Knob| {
        let id = KnobId::Arp(k);
        let (cx, cy) = (x0 + cx, y0 + cy);
        ui.hit(Rect::new(cx - r - 2.0, cy - r - 2.0, 2.0 * r + 4.0, 2.0 * r + 4.0), Action::Knob(id), s.knob_text(id));
        super::knob(ui, cx, cy, r, s.knob(id), k == Knob::TimeMul, t.text, t.text_dim);
    };
    side(ui, 241.0, 103.0, "Time mul", false);
    knob(ui, 264.0, 104.0, 13.0, Knob::TimeMul);
    side(ui, 241.0, 165.0, "Gate", false);
    knob(ui, 265.0, 166.0, 13.0, Knob::Gate);

    // Levels.
    side(ui, 52.0, 221.0, "Levels", false);
    for (i, (&cx, &(name, ..))) in [74.0, 108.0, 142.0, 182.0, 216.0, 256.0].iter().zip(LEVELS.iter()).enumerate() {
        knob(ui, cx, 221.0, 13.0, Knob::Level(i as u8));
        let ring = t.h(LEVEL_COLOURS[i]);
        ui.c.stroke_circle(x0 + cx, y0 + 221.0, 14.5, 1.5, ring);
        label(ui, small, x0 + cx, y0 + 243.0, 0, t.text_dim, name);
    }

    // Group notes.
    let gr = at(11.0, 263.0, 110.0, 18.0);
    let hov = ui.hit(gr, Action::Arp(Act::Group), "Group arpeggiated notes");
    if lemon {
        lk::led(ui.c, t, x0 + 19.0, y0 + 272.0, 7.0, kit::State::new(hov, false, set.group));
    } else {
        super::led(ui, x0 + 19.0, y0 + 272.0, set.group);
    }
    label(ui, Font::new(13), x0 + 29.0, y0 + 272.0, -1, if hov { t.tx(0xffffff) } else { t.text }, "Group notes");

    // Reset, Accept.
    for (r, act, name, hint) in [(at(6.0, 295.0, 59.0, 28.0), Act::Reset, "Reset", "Reset every control"), (at(235.0, 295.0, 59.0, 28.0), Act::Accept, "Accept", "Accept the arpeggio")] {
        let hov = ui.hit(r, Action::Arp(act), hint);
        super::button_face(ui, r, if hov { Look::Hover } else { Look::Off });
        label(ui, lf, r.cx(), r.cy(), 0, if hov { t.tx(0xffffff) } else { t.text }, name);
    }
}
