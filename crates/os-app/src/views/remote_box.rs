// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Remote control settings box ("Link to controller..."), drawn: the target's name, the MIDI controller group,
//! the mapping formula and its graph, smoothing, Remove conflicts, Reset and Accept. What it does: `crate::remote`.
// spec: ui/remote-control.md §2, §3

use os_ui::kit::lemon as lk;
use os_ui::kit;
use os_ui::{Color, Font, Rect};

use super::settings::{led, value_red};
use crate::app::{Action, State, Ui};
use crate::remote::{LinkBox, RAct};

/// The box's size (spec §2).
pub const SIZE: (f32, f32) = (270.0, 373.0);

fn hit(ui: &mut Ui, r: Rect, a: RAct, hint: &'static str) -> bool {
    ui.hit(r, Action::Remote(a), hint)
}

pub fn paint(ui: &mut Ui, s: &State, b: &LinkBox) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    let r = Rect::new(b.at.0, b.at.1, SIZE.0, SIZE.1);
    let rad = if lemon { lk::r_panel(t) } else { 0.0 };
    hit(ui, r, RAct::Body, "");
    ui.c.fill_rrect(r.outset(4.0), rad + 4.0, Color::rgb(0).with_alpha(60));
    ui.c.fill_rrect(r, rad, t.s(0x1c1e21));
    ui.c.fill_rrect(r.inset(1.0), (rad - 1.0).max(0.0), t.s(0x2b2d30));
    let f = Font::new(13);
    let dim = t.text_dim;
    // Title bar.
    ui.c.text(f.bold(), r.x + 10.0, r.y + 7.0, t.text, "Remote control settings");
    let x = Rect::new(r.right() - 24.0, r.y + 4.0, 20.0, 20.0);
    let hov = hit(ui, x, RAct::Close, "Close (Esc)");
    ui.c.icon(&os_ui::tabler::X, x.inset(3.0), if hov { t.glyph } else { t.text });
    // The target's name.
    let name = crate::typein::caption(s, b.target);
    let field = Rect::new(r.x + 10.0, r.y + 32.0, r.w - 20.0, 22.0);
    kit::sunken(ui.c, t, field, t.s(0x141516));
    ui.c.text_clip(f, field.x + 6.0, field.y + 3.0, field.w - 12.0, t.text, name.trim_start_matches("Value for "));

    // MIDI controller.
    let gy = r.y + 64.0;
    group(ui, r, gy, 112.0, "MIDI controller");
    let port = b.port.map_or_else(|| "---".into(), |p| p.to_string());
    let ch = (b.ch + 1).to_string();
    let cc = b.cc.map_or_else(|| "---".into(), |c| c.to_string());
    for (i, (label, v, a, hint)) in [
        ("Port", port, RAct::Port, "MIDI input port number (--- = any device)"),
        ("Channel", ch, RAct::Channel, "MIDI channel number"),
        ("Ctrl", cc, RAct::Ctrl, "Continuous Controller number"),
    ]
    .into_iter()
    .enumerate()
    {
        let bx = Rect::new(r.x + 20.0 + 62.0 * i as f32, gy + 38.0, 50.0, 21.0);
        ui.c.text(Font::new(12), bx.x, gy + 22.0, dim, label);
        hit(ui, bx, a, hint);
        value_red(ui, bx, &v);
    }
    check(ui, r.x + 30.0, gy + 76.0, b.auto, RAct::Auto, "Auto detect", "Just move your MIDI controller");
    let omni = crate::remote::omni_kind(b.target);
    check(ui, r.x + 150.0, gy + 76.0, b.omni && omni, RAct::Omni, "Omni", "Input goes to the selected channel");
    if !omni {
        super::settings::grey(ui, Rect::new(r.x + 140.0, gy + 67.0, 70.0, 18.0));
    }
    let mode = Rect::new(r.x + 20.0, gy + 88.0, 90.0, 21.0);
    let hov = hit(ui, mode, RAct::Mode, "How to treat a note or button link (click for the next)");
    combo(ui, mode, b.mode.name(), hov);
    ui.c.text(f, mode.right() + 8.0, mode.y + 3.0, t.text, "Mode");

    // Mapping formula.
    let gy = r.y + 184.0;
    group(ui, r, gy, 74.0, "Mapping formula");
    let graph = Rect::new(r.x + 20.0, gy + 22.0, 56.0, 44.0);
    kit::sunken(ui.c, t, graph, t.s(0x141516));
    let pts: Vec<(f32, f32)> = (0..=24)
        .map(|i| {
            let x = i as f32 / 24.0;
            (graph.x + 3.0 + x * (graph.w - 6.0), graph.bottom() - 3.0 - b.formula.map(x) * (graph.h - 6.0))
        })
        .collect();
    ui.c.polyline(&pts, 1.5, t.glyph);
    let fr = Rect::new(r.x + 90.0, gy + 24.0, 150.0, 21.0);
    let hov = hit(ui, fr, RAct::Formula, "Mapping formula (click for the next)");
    combo(ui, fr, b.formula.name(), hov);
    ui.c.text(Font::new(12), fr.x, gy + 50.0, dim, b.formula.note());

    // Smoothing.
    let gy = r.y + 266.0;
    group(ui, r, gy, 46.0, "Smoothing");
    check(ui, r.x + 30.0, gy + 32.0, b.smooth, RAct::Smooth, "Smoothing", "Enable automation smoothing");
    let tb = Rect::new(r.x + 150.0, gy + 22.0, 50.0, 21.0);
    hit(ui, tb, RAct::Time, "Smoothing envelope time (milliseconds)");
    value_red(ui, tb, &b.time.to_string());
    ui.c.text(f, tb.right() + 6.0, tb.y + 3.0, t.text, "ms");
    if !b.smooth {
        super::settings::grey(ui, Rect::new(tb.x, tb.y, 80.0, tb.h));
    }

    check(ui, r.x + 20.0, r.y + 326.0, b.conflicts, RAct::Conflicts, "Remove conflicts", "Auto-remove other conflicting assignments");
    let reset = Rect::new(r.x + 10.0, r.bottom() - 32.0, 80.0, 24.0);
    let accept = Rect::new(r.right() - 90.0, r.bottom() - 32.0, 80.0, 24.0);
    for (bt, a, label, hint) in [(reset, RAct::Reset, "Reset", "Reset all settings"), (accept, RAct::Accept, "Accept", "Make the link")] {
        let hov = hit(ui, bt, a, hint);
        if lemon {
            lk::outlined(ui.c, bt, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
        } else {
            kit::bordered(ui.c, bt, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
        }
        ui.c.text_center(f, bt, if hov { t.glyph } else { t.text }, label);
    }
}

/// A group's frame and title at `y`, `h` high.
fn group(ui: &mut Ui, r: Rect, y: f32, h: f32, title: &str) {
    let t = ui.t;
    let g = Rect::new(r.x + 8.0, y, r.w - 16.0, h);
    ui.c.stroke_rrect(g, 3.0, 1.0, t.s(0x3a3d40));
    let w = ui.c.text_width(Font::new(12), title);
    ui.c.fill_rect(Rect::new(g.x + 8.0, y - 7.0, w + 8.0, 14.0), t.s(0x2b2d30));
    ui.c.text(Font::new(12), g.x + 12.0, y - 8.0, t.text_dim, title);
}

fn check(ui: &mut Ui, x: f32, cy: f32, on: bool, a: RAct, label: &'static str, hint: &'static str) {
    let t = ui.t;
    let f = Font::new(13);
    let w = ui.c.text_width(f, label);
    let hov = hit(ui, Rect::new(x - 8.0, cy - 9.0, w + 26.0, 18.0), a, hint);
    led(ui, x, cy, on, hov);
    let lh = ui.c.line_height(f);
    ui.c.text(f, x + 11.0, (cy - lh / 2.0).round(), if hov { t.glyph } else { t.text }, label);
}

fn combo(ui: &mut Ui, r: Rect, v: &str, hov: bool) {
    kit::combo(ui.c, ui.t, r, v, Font::new(13), kit::State::hover(hov));
}
