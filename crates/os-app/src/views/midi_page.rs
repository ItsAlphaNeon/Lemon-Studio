// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Settings > MIDI's own rows, drawn: the output and input device lists and the selected device's options, the sync
//! offsets, the link combos with their curve buttons, External clock sync and its status line, and the mapping curve
//! window. The page's plain checks, combos and channel boxes are `crate::prefs` declarations drawn by
//! `super::settings`; what the rows do is `App::midi_page_act`.
// spec: ui/midi-settings.md §1

use os_ui::kit::lemon as lk;
use os_ui::kit;
use os_ui::{Color, Font, Rect};

use super::settings::{grey, led, value_red, SAct};
use crate::app::{Action, State, Ui};
use crate::midi_cfg::{self as cfg, Curve, CurveOf, Offset};

/// The MIDI page's rows of its own (`super::settings::Row::Midi`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MRow {
    Outputs,
    /// The selected output's Send master sync, Send all notes off and Port.
    OutOptions,
    /// An offset box: on the left (master sync) or at the right (external sync).
    Offset(Offset),
    Inputs,
    /// The selected input's Enable and Port.
    InOptions,
    /// A link combo with its curve button.
    Link(CurveOf),
    ExtSync,
    Status,
}

impl MRow {
    /// The declared combo the row shows, if any (`crate::prefs` declaration tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn combo(&self) -> Option<&'static str> {
        match self {
            MRow::Link(CurveOf::Velocity) => Some(crate::prefs::VELOCITY_LINK.label),
            MRow::Link(CurveOf::Release) => Some(crate::prefs::RELEASE_LINK.label),
            _ => None,
        }
    }

    pub fn height(&self) -> f32 {
        match self {
            MRow::Outputs => LIST_HEAD + ROW_H * OUT_ROWS as f32 + 10.0,
            MRow::Inputs => LIST_HEAD + ROW_H * IN_ROWS as f32 + 10.0,
            MRow::OutOptions | MRow::InOptions | MRow::Offset(_) | MRow::Link(_) | MRow::ExtSync => 27.0,
            MRow::Status => 20.0,
        }
    }
}

/// What a click on one of the rows does (`SAct::Midi`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MAct {
    /// Selects list row i.
    Output(usize),
    Input(usize),
    /// An input row's power icon: switches it on or off.
    Power(usize),
    OutSync,
    OutNotesOff,
    /// The selected device's Port box (the wheel and a drag step it; right-click resets).
    OutPort,
    InPort,
    Enable,
    Offset(Offset),
    /// A curve button: opens its window.
    Curve(CurveOf),
    /// The External clock sync combo, with its bottom left.
    ExtSync(f32, f32),
    /// The curve window's parts: panel, close, Reset, Accept, a handle (0 start, 1 middle, 2 end).
    CurveBody,
    CurveClose,
    CurveReset,
    CurveAccept,
    CurveHandle(u8),
}

impl MAct {
    /// A value box (the wheel steps it).
    pub fn is_value(self) -> bool {
        matches!(self, MAct::OutPort | MAct::InPort | MAct::Offset(_))
    }
}

/// The curve window while open: which curve, the curve as edited.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurveEdit {
    pub of: CurveOf,
    pub curve: Curve,
}

const LIST_HEAD: f32 = 18.0;
const ROW_H: f32 = 17.0;
const OUT_ROWS: usize = 5;
const IN_ROWS: usize = 7;

fn act(m: MAct) -> Action {
    Action::Settings(SAct::Midi(m))
}

fn port_text(p: Option<u8>) -> String {
    p.map_or_else(|| "---".into(), |p| p.to_string())
}

/// Draws row `m` (`g` the group's body, `x0` where its controls start, `y`..`y + h` the row).
#[allow(clippy::too_many_arguments)]
pub fn paint_row(ui: &mut Ui, s: &State, m: MRow, g: Rect, x0: f32, y: f32, h: f32) {
    let t = ui.t;
    let f = Font::new(13);
    let cy = y + h / 2.0;
    let ty = (cy - ui.c.line_height(f) / 2.0).round();
    let p = &s.midi;
    match m {
        MRow::Outputs => {
            let rows: Vec<(String, String, bool)> = p
                .outputs
                .iter()
                .map(|n| {
                    let c = cfg::output(n);
                    let mut info = format!("port {}", port_text(c.port));
                    if c.sync {
                        info.push_str(", sync");
                    }
                    (n.clone(), info, false)
                })
                .collect();
            list(ui, Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, h - 6.0), "Installed MIDI output devices", &rows, p.output, OUT_ROWS, false);
        }
        MRow::Inputs => {
            let rows: Vec<(String, String, bool)> = p
                .inputs
                .iter()
                .map(|i| {
                    let c = cfg::input(&i.name);
                    let kind = if i.failed { "(in use)".to_string() } else { crate::prefs::choice(&crate::prefs::CONTROLLER_TYPE).to_string() };
                    (i.name.clone(), format!("{kind}   port {}", port_text(c.port)), c.enabled)
                })
                .collect();
            list(ui, Rect::new(x0 - 4.0, y + 2.0, g.w - 16.0, h - 6.0), "Installed MIDI input devices", &rows, p.input, IN_ROWS, true);
        }
        MRow::OutOptions => {
            let out = p.selected_output().map(|n| cfg::output(n));
            let c = out.unwrap_or_default();
            check(ui, g.x + 22.0, cy, c.sync, MAct::OutSync, "Send master sync", "Send master synchronization to this device", out.is_some());
            check(ui, g.x + 324.0, cy, c.notes_off, MAct::OutNotesOff, "Send all notes off", "Send all notes off to this device on init and after render", out.is_some());
            port_box(ui, g, y, &port_text(c.port), MAct::OutPort, "Access port number (right-click: reset)", out.is_some());
        }
        MRow::InOptions => {
            let inp = p.selected_input().map(|i| cfg::input(&i.name));
            let c = inp.unwrap_or_default();
            check(ui, g.x + 22.0, cy, c.enabled, MAct::Enable, "Enable", "Enable input from this device", inp.is_some());
            port_box(ui, g, y, &port_text(c.port), MAct::InPort, "Unique / feedback port number (right-click: reset)", inp.is_some());
        }
        MRow::Offset(o) => {
            let v = cfg::offset(o).to_string();
            match o {
                Offset::MasterSync => {
                    let b = Rect::new(x0, y + 3.0, 46.0, 21.0);
                    let hov = ui.hit(b, act(MAct::Offset(o)), "Midi master clock sync offset (milliseconds; wheel or drag, right-click: reset)");
                    value_red(ui, b, &v);
                    ui.c.text(f, b.right() + 8.0, ty, if hov { t.glyph } else { t.text }, "Master sync offset (milliseconds)");
                }
                Offset::External => {
                    let b = Rect::new(g.right() - 58.0, y + 3.0, 48.0, 21.0);
                    let hov = ui.hit(b, act(MAct::Offset(o)), "Midi external clock sync offset (milliseconds; wheel or drag, right-click: reset)");
                    value_red(ui, b, &v);
                    ui.c.text_right(f, b.x - 8.0, ty, if hov { t.glyph } else { t.text }, "External sync offset (milliseconds)");
                }
            }
        }
        MRow::Link(of) => {
            let (c, hint) = match of {
                CurveOf::Velocity => (&crate::prefs::VELOCITY_LINK, "Note on velocity link target"),
                CurveOf::Release => (&crate::prefs::RELEASE_LINK, "Release velocity link target"),
            };
            let b = Rect::new(x0, y + 3.0, 100.0, 21.0);
            let hov = ui.hit(b, Action::Settings(SAct::Combo(c.label, b.x, b.bottom())), hint);
            kit::combo(ui.c, t, b, crate::prefs::choice(c), f, kit::State::hover(hov));
            // The curve button: a small curve icon.
            let cb = Rect::new(b.right() + 4.0, y + 3.0, 24.0, 21.0);
            let lit = !cfg::curve(of).is_line();
            let hov_c = ui.hit(cb, act(MAct::Curve(of)), "Edit mapping curve");
            button_face(ui, cb, hov_c);
            let col = if lit || hov_c { t.glyph } else { t.text };
            let pts: Vec<(f32, f32)> = (0..=10)
                .map(|i| {
                    let x = i as f32 / 10.0;
                    let v = cfg::curve(of).map(x);
                    (cb.x + 5.0 + x * (cb.w - 10.0), cb.bottom() - 5.0 - v * (cb.h - 10.0))
                })
                .collect();
            ui.c.polyline(&pts, 1.3, col);
            ui.c.text(f, cb.right() + 8.0, ty, if hov { t.glyph } else { t.text }, c.label);
        }
        MRow::ExtSync => {
            let b = Rect::new(x0, y + 3.0, 168.0, 21.0);
            let hov = ui.hit(b, act(MAct::ExtSync(b.x, b.bottom())), "Device whose MIDI clock drives playback");
            let v = cfg::ext_sync().unwrap_or_else(|| "Off".into());
            kit::combo(ui.c, t, b, &v, f, kit::State::hover(hov));
            ui.c.text(f, b.right() + 8.0, ty, if hov { t.glyph } else { t.text }, "External clock sync");
        }
        MRow::Status => {
            let text = if p.status.is_empty() { "Status: off" } else { &p.status };
            ui.c.text(f, x0, ty, t.text_dim, text);
        }
    }
}

/// A device list: title, rows (name, second column, power lit), the selected row, rows shown, whether rows have a
/// power icon.
#[allow(clippy::too_many_arguments)]
fn list(ui: &mut Ui, b: Rect, title: &'static str, rows: &[(String, String, bool)], sel: usize, n: usize, power: bool) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    if lemon {
        lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), t.s(0x222527));
    } else {
        ui.c.fill_rect(b, t.s(0x161819));
        ui.c.fill_rect(b.inset(1.0), t.s(0x222527));
    }
    let f = Font::new(13);
    ui.c.text(f, b.x + 6.0, b.y + 2.0, t.text_dim, title);
    let top = b.y + 2.0 + LIST_HEAD;
    if rows.is_empty() {
        ui.c.text(f, b.x + 8.0, top + 1.0, t.text_dim, "(no devices)");
    }
    for (i, (name, info, on)) in rows.iter().take(n).enumerate() {
        let rr = Rect::new(b.x + 2.0, top + i as f32 * ROW_H, b.w - 4.0, ROW_H);
        let act_row = if power { MAct::Input(i) } else { MAct::Output(i) };
        let hov = ui.hit(rr, act(act_row), title);
        if i == sel {
            let c = if lemon { t.brand_sel(os_theme::design::lemon::COLORED_DARK) } else { t.a(0x2d5a8c) };
            ui.c.fill_rect(rr, c);
        } else if hov {
            ui.c.fill_rect(rr, t.s(0x2c2f31));
        } else if i % 2 == 1 {
            ui.c.fill_rect(rr, t.s(0x26292b));
        }
        let col = if hov { t.glyph } else { t.text };
        ui.c.text_clip(f, rr.x + 4.0, rr.y + 1.0, rr.w * 0.55 - 8.0, col, name);
        ui.c.text_clip(f, rr.x + rr.w * 0.55, rr.y + 1.0, rr.w * 0.45 - 30.0, t.text_dim, info);
        if power {
            let pb = Rect::new(rr.right() - 20.0, rr.y + 1.0, 15.0, 15.0);
            ui.hit(pb, act(MAct::Power(i)), if *on { "Enabled: click to switch this device off" } else { "Off: click to switch this device on" });
            let c = if *on { Color::rgb(0x5fd35a) } else { t.s(0x55585a) };
            // A power lamp: lit green while enabled.
            ui.c.fill_circle(pb.cx(), pb.cy(), 5.5, t.s(0x141516));
            ui.c.fill_circle(pb.cx(), pb.cy(), 4.0, c);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check(ui: &mut Ui, x: f32, cy: f32, on: bool, a: MAct, label: &'static str, hint: &'static str, enabled: bool) {
    let t = ui.t;
    let f = Font::new(13);
    let tw = ui.c.text_width(f, label);
    let hr = Rect::new(x - 8.0, cy - 9.0, tw + 26.0, 18.0);
    let hov = if enabled {
        ui.hit(hr, act(a), hint)
    } else {
        ui.hit(hr, Action::Settings(SAct::Body), "No device selected");
        false
    };
    led(ui, x, cy, on, hov);
    let lh = ui.c.line_height(f);
    ui.c.text(f, x + 11.0, (cy - lh / 2.0).round(), if hov { t.glyph } else { t.text }, label);
    if !enabled {
        grey(ui, hr);
    }
}

fn port_box(ui: &mut Ui, g: Rect, y: f32, v: &str, a: MAct, hint: &'static str, enabled: bool) {
    let t = ui.t;
    let f = Font::new(13);
    let b = Rect::new(g.right() - 52.0, y + 3.0, 42.0, 21.0);
    if enabled {
        ui.hit(b, act(a), hint);
    }
    value_red(ui, b, v);
    let ty = (b.cy() - ui.c.line_height(f) / 2.0).round();
    ui.c.text_right(f, b.x - 8.0, ty, t.text, "Port");
    if !enabled {
        let lw = ui.c.text_width(f, "Port");
        grey(ui, Rect::span(b.x - 8.0 - lw, b.right(), b.y, b.bottom()));
    }
}

fn button_face(ui: &mut Ui, b: Rect, hov: bool) {
    let t = ui.t;
    if super::lemon(ui) {
        lk::outlined(ui.c, b, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::hover(hov)));
    } else {
        kit::bordered(ui.c, b, 2.0, t.s(0x161819), kit::face(t.s(0x212425), kit::State::hover(hov)));
    }
}

/// The curve window's rects in the settings window `win`: the panel (240 x 320, spec §3), the graph, Reset, Accept and
/// the close button.
pub fn curve_rects(win: Rect) -> (Rect, Rect, Rect, Rect, Rect) {
    let panel = Rect::new((win.cx() - 120.0).round(), win.y + 160.0, 240.0, 320.0);
    let graph = Rect::new(panel.x + 14.0, panel.y + 36.0, panel.w - 28.0, panel.w - 28.0);
    let reset = Rect::new(panel.x + 14.0, panel.bottom() - 38.0, 90.0, 26.0);
    let accept = Rect::new(panel.right() - 104.0, panel.bottom() - 38.0, 90.0, 26.0);
    let close = Rect::new(panel.right() - 26.0, panel.y + 5.0, 20.0, 20.0);
    (panel, graph, reset, accept, close)
}

/// The three handles' centres in `graph` for curve `c`: start, middle, end.
pub fn handles(graph: Rect, c: &Curve) -> [(f32, f32); 3] {
    let at = |x: f32, v: f32| (graph.x + x * graph.w, graph.bottom() - v * graph.h);
    [at(0.0, c.lo), at(0.5, c.map(0.5)), at(1.0, c.hi)]
}

/// The curve with handle `which` dragged to `(x, y)` in `graph`.
pub fn drag_handle(graph: Rect, c: Curve, which: u8, y: f32) -> Curve {
    let v = ((graph.bottom() - y) / graph.h).clamp(0.0, 1.0);
    let mut c = c;
    match which {
        0 => c.lo = v,
        2 => c.hi = v,
        _ => {
            let span = c.hi - c.lo;
            if span.abs() > 1e-3 {
                c.mid = ((v - c.lo) / span).clamp(0.02, 0.98);
            }
        }
    }
    c
}

pub fn paint_curve(ui: &mut Ui, e: &CurveEdit, win: Rect) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    let (panel, graph, reset, accept, close) = curve_rects(win);
    let rad = if lemon { lk::r_panel(t) } else { 0.0 };
    ui.hit(panel, act(MAct::CurveBody), "");
    ui.c.fill_rrect(panel.outset(3.0), rad + 3.0, Color::rgb(0).with_alpha(70));
    ui.c.fill_rrect(panel, rad, t.s(0x1c1e21));
    ui.c.fill_rrect(panel.inset(1.0), (rad - 1.0).max(0.0), t.s(0x2b2d30));
    ui.c.text(Font::new(13).bold(), panel.x + 10.0, panel.y + 8.0, t.text, e.of.title());
    let hov = ui.hit(close, act(MAct::CurveClose), "Close (drops the changes)");
    ui.c.icon(&os_ui::tabler::X, close.inset(3.0), if hov { t.glyph } else { t.text });
    kit::sunken(ui.c, t, graph, t.s(0x141516));
    for k in 1..4 {
        let k = k as f32 / 4.0;
        ui.c.fill_rect(Rect::new(graph.x + k * graph.w, graph.y, 1.0, graph.h), t.s(0x202224));
        ui.c.fill_rect(Rect::new(graph.x, graph.y + k * graph.h, graph.w, 1.0), t.s(0x202224));
    }
    let pts: Vec<(f32, f32)> = (0..=64)
        .map(|i| {
            let x = i as f32 / 64.0;
            (graph.x + x * graph.w, graph.bottom() - e.curve.map(x) * graph.h)
        })
        .collect();
    ui.c.polyline(&pts, 1.8, t.glyph);
    for (i, (hx, hy)) in handles(graph, &e.curve).into_iter().enumerate() {
        let r = Rect::new(hx - 7.0, hy - 7.0, 14.0, 14.0);
        let hint = ["Start level: drag up or down", "Tension: drag up or down", "End level: drag up or down"][i];
        let hov = ui.hit(r, act(MAct::CurveHandle(i as u8)), hint);
        ui.c.fill_circle(hx, hy, 5.5, if hov { t.glyph } else { t.text });
        ui.c.fill_circle(hx, hy, 3.5, t.s(0x141516));
    }
    for (b, a, label, hint) in [(reset, MAct::CurveReset, "Reset", "Straight line"), (accept, MAct::CurveAccept, "Accept", "Keep this curve")] {
        let hov = ui.hit(b, act(a), hint);
        button_face(ui, b, hov);
        ui.c.text_center(Font::new(13), b, if hov { t.glyph } else { t.text }, label);
    }
}
