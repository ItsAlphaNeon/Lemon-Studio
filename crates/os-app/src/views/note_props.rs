// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Note properties window: the note's name in the title, the Levels section (seven knobs, the slide
//! and portamento switches, the colour tab and its grid), the Time section (start time and duration), Reset and
//! Accept. With several notes it is "Step properties": the Level offsets section and the two buttons only.
// spec: ui/piano-note-properties.md §2–§4

use os_ui::{Color, Font, Rect};

use super::strum::{label, led};
use super::{FrameInfo, Look};
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::knobs::KnobId;
use crate::note_props::{Act, LEVELS};

/// The window's size: Note properties, and Step properties (§2, §4).
pub const SIZE: (f32, f32) = (297.0, 295.0);
pub const SIZE_MANY: (f32, f32) = (297.0, 152.0);

pub fn size(s: &State) -> (f32, f32) {
    if s.note_props.many() { SIZE_MANY } else { SIZE }
}

pub fn rect(s: &State) -> Rect {
    let (w, h) = size(s);
    Rect::new(s.note_props.pos.0, s.note_props.pos.1, w, h)
}

/// The window, with the colour grid when it hangs below the window's edge.
pub fn bounds(s: &State) -> Rect {
    let r = rect(s);
    let r = if s.note_props.grid() { r.union(&grid_rect(r)) } else { r };
    r.outset(1.0)
}

/// The knobs' labels and centres (x), in `LEVELS` order (§2).
const NAMES: [&str; 7] = ["PAN", "VEL", "REL", "MOD X", "MOD Y", "PITCH", "REPEAT"];
const KNOB_X: [f32; 7] = [37.0, 71.0, 105.0, 142.0, 176.0, 218.0, 260.0];

/// The 16 colour groups' swatches, group 0 first (§3: numbered down the columns).
pub const SWATCHES: [u32; 16] = [
    0x9ed1a5, 0x9fd3ba, 0xa1d6d0, 0xa3cad8, 0xa5b8db, 0xa8a7de, 0xbca7de, 0xd1a7de, 0xdda7d6, 0xdba5c0, 0xd9a3a9,
    0xd6afa2, 0xd4c1a0, 0xd1d29e, 0xbdd19e, 0xa9d19d,
];

/// The colour tab (§2).
fn tab_rect(win: Rect) -> Rect {
    Rect::new(win.x + 255.0, win.y + 108.0, 42.0, 32.0)
}

/// The colour grid (§3): 164 x 164 under the colour tab, its right edge on the window's.
fn grid_rect(win: Rect) -> Rect {
    Rect::new(win.x + win.w - 164.0, win.y + 140.0, 164.0, 164.0)
}

fn section(ui: &mut Ui, win: Rect, y: f32, h: f32, name: &str) {
    let t = ui.t;
    ui.c.fill_rect(Rect::new(win.x + 4.0, win.y + y, win.w - 8.0, h), t.s(0x3a3f42));
    ui.c.fill_rect(Rect::new(win.x + 4.0, win.y + y, win.w - 8.0, 17.0), t.s(0x464c50));
    label(ui, Font::new(13), win.x + 10.0, win.y + y + 8.5, -1, t.text, name);
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let (x0, y0) = (win.x, win.y);
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::new(x0 + x, y0 + y, w, h);
    let many = s.note_props.many();
    let (bg, line) = (t.s(0x2f3335), t.s(0x16191a));
    if super::lemon(ui) {
        os_ui::kit::lemon::outlined(ui.c, win, os_ui::kit::lemon::r_panel(t), line, bg);
    } else {
        ui.c.fill_rect(win, line);
        ui.c.fill_rect(win.inset(1.0), bg);
    }
    ui.hit(win, Action::Swallow, "");
    ui.hit(at(1.0, 1.0, win.w - 30.0, 26.0), Action::WinDrag(Win::NoteProps), "");
    let Some(n) = crate::note_props::shown(s) else { return };

    // Title: "Note properties - C5" (the name orange), "slide to C5" for a slide note; "Step properties -" (§1).
    let tf = Font::new(17);
    if many {
        label(ui, tf, x0 + 7.0, y0 + 15.0, -1, t.text, "Step properties -");
    } else {
        let name = super::piano::key_name(i32::from(n.key));
        let head = if n.slide() {
            "slide to "
        } else if n.porta() {
            "porta to "
        } else {
            "Note properties - "
        };
        let w = ui.c.text_width(tf, head);
        label(ui, tf, x0 + 7.0, y0 + 15.0, -1, t.text, head);
        label(ui, tf, x0 + 7.0 + w, y0 + 15.0, -1, t.h(0xfb9531), &name);
    }
    let close = at(win.w - 20.0, 10.0, 10.0, 10.0);
    let hov = ui.hit(close.outset(4.0), Action::CloseWin(Win::NoteProps), "Close");
    icons::draw(ui.c, Icon::Close, close.outset(1.0), if hov { t.tx(0xffffff) } else { t.glyph });

    // Levels / Level offsets.
    section(ui, win, 30.0, if many { 80.0 } else { 116.0 }, if many { "Level offsets" } else { "Levels" });
    let small = Font::new(9);
    for (k, x) in KNOB_X.iter().enumerate() {
        let id = KnobId::NoteProp(k as u8);
        let (cx, cy) = (x0 + x, y0 + 70.0);
        let bipolar = LEVELS[k].center().is_some() || many;
        ui.hit(Rect::new(cx - 15.0, cy - 15.0, 30.0, 30.0), Action::Knob(id), s.knob_text(id));
        super::knob(ui, cx, cy, 13.0, s.knob_at(id, cx, cy, 14.0), bipolar, t.text, t.text_dim);
        label(ui, small, cx, y0 + 95.0, 0, t.text_dim, NAMES[k]);
    }

    if !many {
        // Slide and portamento (§2): an LED and a glyph each.
        for (x, on, act, hint) in [
            (33.0, n.slide(), Act::Slide, "Slide: the note glides the notes it overlaps to its key"),
            (76.0, n.porta(), Act::Porta, "Portamento: the note glides from the note before"),
        ] {
            let hov = ui.hit(at(x - 9.0, 115.0, 36.0, 18.0), Action::NoteProps(act), hint);
            led(ui, x0 + x, y0 + 124.0, on, hov, false);
            let ink = if on { t.h(0xfb9531) } else { t.text };
            let (gx, gy) = (x0 + x + 10.0, y0 + 124.0);
            if act == Act::Slide {
                ui.c.fill_polygon(&[(gx + 12.0, gy - 6.0), (gx + 12.0, gy + 6.0), (gx, gy + 6.0)], ink);
            } else {
                ui.c.polyline(
                    &[(gx, gy + 5.0), (gx + 4.0, gy + 5.0), (gx + 8.0, gy - 5.0), (gx + 12.0, gy - 5.0)],
                    1.6,
                    ink,
                );
            }
        }
        // Colour tab (§2, §3).
        label(ui, Font::new(15), x0 + 246.0, y0 + 124.0, 1, t.text, "Color");
        let tab = tab_rect(win);
        let c = n.colour();
        let hov = ui.hit(tab, Action::NoteProps(Act::ColourGrid), "Color group / MIDI channel");
        ui.c.fill_rect(tab, t.s(SWATCHES[usize::from(c)]));
        if hov {
            ui.c.stroke_rrect(tab.inset(0.5), 0.0, 1.0, t.tx(0xffffff));
        }
        label(ui, Font::new(13), tab.cx(), tab.cy(), 0, t.tx(0x2f3335), &(c + 1).to_string());

        // Time (§2): start time and duration as bar:step:tick.
        section(ui, win, 150.0, 102.0, "Time");
        let ppq = f64::from(s.session.project.ppq.max(4));
        for (y, name, ticks, dur) in [(178.0, "Start time", n.position, false), (212.0, "Duration", n.length, true)] {
            label(ui, Font::new(15), x0 + 14.0, y0 + y + 12.0, -1, t.text, name);
            let r = at(75.0, y, 103.0, 24.0);
            ui.c.fill_rect(r, t.s(0x8a2a2c));
            ui.c.fill_rect(r.inset(1.0), t.s(0x4a1517));
            let (b, st, tk) = super::toolbar::bst_ticks(f64::from(ticks) * 96.0 / ppq);
            let (b, st) = if dur { (b - 1, st - 1) } else { (b, st) };
            let text = format!("{b}:{st:02}:{tk:02}");
            label(ui, Font::new(17), r.x + r.w - 8.0, r.cy(), 1, t.tx(0xffd9d9), &text);
            ui.hit(r, Action::Swallow, if dur { "Duration (bar:step:tick)" } else { "Start time (bar:step:tick)" });
        }
    }

    // Reset and Accept.
    let by = win.h - 37.0;
    for (r, act, name, hint) in [
        (at(6.0, by, 60.0, 30.0), Act::Reset, "Reset", "Reset: the values the window opened with"),
        (at(win.w - 66.0, by, 60.0, 30.0), Act::Accept, "Accept", "Accept the changes"),
    ] {
        let hov = ui.hit(r, Action::NoteProps(act), hint);
        super::button_face(ui, r, if hov { Look::Hover } else { Look::Off });
        label(ui, Font::new(15), r.cx(), r.cy(), 0, if hov { t.tx(0xffffff) } else { t.text }, name);
    }

    if s.note_props.grid() {
        paint_grid(ui, s, win);
    }
}

/// The colour grid (§3): 4 x 4 swatches numbered down the columns, the note's own outlined.
fn paint_grid(ui: &mut Ui, s: &State, win: Rect) {
    let t = ui.t;
    let g = grid_rect(win);
    ui.c.fill_rect(g, t.s(0x16191a));
    ui.hit(g, Action::Swallow, "Color group / MIDI channel");
    let cur = crate::note_props::shown(s).map_or(0, |n| n.colour());
    for c in 0..16u8 {
        let (col, row) = (f32::from(c / 4), f32::from(c % 4));
        let r = Rect::new(g.x + 1.0 + col * 40.75, g.y + 1.0 + row * 40.75, 39.75, 39.75);
        let hov = ui.hit(r, Action::NoteProps(Act::Colour(c)), format!("Color group / MIDI channel {}", c + 1));
        ui.c.fill_rect(r, t.s(SWATCHES[usize::from(c)]));
        if c == cur || hov {
            ui.c.stroke_rrect(r.inset(1.0), 0.0, 2.0, Color::rgb(0xffffff));
        }
        label(ui, Font::new(13), r.cx(), r.cy(), 0, t.tx(0x2f3335), &(c + 1).to_string());
    }
}
