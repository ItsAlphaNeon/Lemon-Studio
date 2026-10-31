// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The event editor's window: the title, the toolbar (menu and tools), the ruler and the level graph of one control's
//! events in the current pattern.
// spec: ui/event-editor.md §2, §3 (Lemon: ui/lemon-design.md)

use os_theme::design::lemon as brand;
use os_ui::kit::{self, lemon as lk};
use os_ui::{Color, Font, Rect};

use super::FrameInfo;
use crate::app::{Action, State, Ui, Win};
use crate::events::{self, EvAct, Tool};
use crate::icons::{self, Icon};

pub const SIZE: (f32, f32) = (720.0, 320.0);
/// The ruler and the graph, from the window's top-left; the graph's value labels sit left of it.
const RULER: Rect = Rect::new(52.0, 64.0, 660.0, 18.0);
const GRAPH: Rect = Rect::new(52.0, 82.0, 660.0, 230.0);

pub fn rect(s: &State) -> Rect {
    Rect::new(s.events.pos.0, s.events.pos.1, SIZE.0, SIZE.1)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

/// The graph on screen.
pub fn graph(s: &State) -> Rect {
    let w = rect(s);
    Rect::new(w.x + GRAPH.x, w.y + GRAPH.y, GRAPH.w, GRAPH.h)
}

fn ruler(s: &State) -> Rect {
    let w = rect(s);
    Rect::new(w.x + RULER.x, w.y + RULER.y, RULER.w, RULER.h)
}

/// The window's title: the control's name.
pub fn title(s: &State) -> String {
    format!("Events - {}", s.events.name)
}

fn ppq(s: &State) -> f64 {
    f64::from(s.project.ppq.max(4))
}

/// The tick under x (unclamped).
pub fn tick_at(s: &State, x: f32) -> f64 {
    s.events.scroll + f64::from(x - graph(s).x) * ppq(s) / f64::from(s.events.px_per_beat)
}

pub fn x_of(s: &State, tick: f64) -> f32 {
    graph(s).x + ((tick - s.events.scroll) * f64::from(s.events.px_per_beat) / ppq(s)) as f32
}

/// The level (0..1) under y.
pub fn level_at(s: &State, y: f32) -> f64 {
    let g = graph(s);
    f64::from((g.bottom() - y) / g.h).clamp(0.0, 1.0)
}

fn y_of(g: Rect, level: f64) -> f32 {
    g.bottom() - (level as f32) * g.h
}

/// Text vertically centred on `cy` from `x` (or right-aligned to `x` when `right`).
fn label(ui: &mut Ui, f: Font, x: f32, cy: f32, right: bool, col: Color, s: &str) {
    let lh = ui.c.line_height(f);
    let y = (cy - lh / 2.0).round();
    let x = if right { (x - ui.c.text_width(f, s)).round() } else { x };
    ui.c.text(f, x, y, col, s);
}

fn button(ui: &mut Ui, r: Rect, a: Action, hint: &'static str, face: Result<&str, Icon>, on: bool) {
    let hov = ui.hit(r, a.clone(), hint);
    let down = ui.down(r, &a);
    let t = ui.t;
    kit::button(ui.c, t, r, kit::State::new(hov, down, on));
    let ink = if on && super::lemon(ui) { lk::ink_on_lit(t) } else if hov { t.tx(0xffffff) } else { t.text };
    match face {
        Ok(text) => {
            let f = Font::new(14);
            let w = ui.c.text_width(f, text);
            label(ui, f, (r.cx() - w / 2.0).round(), r.cy(), false, ink, text);
        }
        Err(icon) => icons::draw(ui.c, icon, Rect::new(r.cx() - 7.0, r.cy() - 7.0, 14.0, 14.0), ink),
    }
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let (x0, y0) = (win.x, win.y);
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::new(x0 + x, y0 + y, w, h);
    let lemon = super::lemon(ui);
    let e = &s.events;
    let (bg, line) = (t.s(0x202324), t.s(0x16191a));
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), line, bg);
    } else {
        ui.c.fill_rect(win, line);
        ui.c.fill_rect(win.inset(1.0), bg);
    }
    ui.hit(win, Action::Swallow, "");
    ui.hit(at(1.0, 1.0, SIZE.0 - 2.0, 29.0), Action::WinDrag(Win::Events), "");
    label(ui, Font::new(17), x0 + 9.0, y0 + 15.0, false, t.text, &title(s));
    let close = at(SIZE.0 - 20.0, 10.0, 10.0, 10.0);
    let hov = ui.hit(close.outset(3.0), Action::CloseWin(Win::Events), "Close");
    icons::draw(ui.c, Icon::Close, close.outset(1.0), if hov { t.tx(0xffffff) } else { t.glyph });

    // The toolbar: the menu, then the tools.
    button(ui, at(8.0, 34.0, 60.0, 24.0), Action::Events(EvAct::Menu), "Event editor menu", Ok("Edit"), false);
    let tools = [(Tool::Draw, "Draw", "Draw (left: freehand, right: line)"), (Tool::Erase, "Erase", "Erase events"), (Tool::Select, "Select", "Select a stretch of time")];
    for (i, (tool, text, hint)) in tools.into_iter().enumerate() {
        button(ui, at(80.0 + 64.0 * i as f32, 34.0, 60.0, 24.0), Action::Events(EvAct::Tool(tool)), hint, Ok(text), e.tool == tool);
    }

    let g = graph(s);
    let r = ruler(s);
    let ppq = ppq(s);
    let bar = ppq * 4.0;
    let len = f64::from(s.project.pattern_length(s.pattern));
    let accent = if lemon { t.brand_lit(brand::MAIN) } else { Color::rgb(0x8fb35c) };

    // The ruler: bar numbers.
    ui.hit(r, Action::Events(EvAct::Ruler), "Event editor: drag to select a stretch of time");
    ui.c.fill_rect(r, t.s(0x2a2e30));
    ui.c.push_clip(r);
    let first_bar = (e.scroll / bar).floor().max(0.0) as u32;
    let mut b = first_bar;
    loop {
        let x = x_of(s, f64::from(b) * bar);
        if x > r.right() {
            break;
        }
        ui.c.fill_rect(Rect::new(x.floor(), r.y + 9.0, 1.0, 9.0), t.text_dim);
        label(ui, Font::new(12), x.floor() + 3.0, r.y + 6.0, false, t.text_dim, &(b + 1).to_string());
        b += 1;
    }
    ui.c.pop_clip();

    // The graph: background, past the pattern's end darker, beat and bar lines.
    ui.hit(g, Action::Events(EvAct::Graph), "Event editor: left drag draws, right drag draws a line");
    ui.c.fill_rect(g, t.s(0x34393b));
    ui.c.push_clip(g);
    let end_x = x_of(s, len);
    if end_x < g.right() {
        ui.c.fill_rect(Rect::new(end_x.max(g.x), g.y, g.right() - end_x.max(g.x), g.h), t.s(0x2b2f31));
    }
    let px_beat = f64::from(e.px_per_beat);
    let beat_step = if px_beat >= 12.0 { ppq } else { bar };
    let mut k = (e.scroll / beat_step).floor().max(0.0);
    loop {
        let tick = k * beat_step;
        let x = x_of(s, tick).floor();
        if x > g.right() {
            break;
        }
        let col = if (tick / bar).fract() == 0.0 { t.s(0x4a5053) } else { t.s(0x3c4144) };
        ui.c.fill_rect(Rect::new(x, g.y, 1.0, g.h), col);
        k += 1.0;
    }
    // A bipolar control's centre.
    let bipolar = os_model::target::bipolar(e.target);
    let base = if bipolar { y_of(g, 0.5) } else { g.bottom() };
    if bipolar {
        ui.c.fill_rect(Rect::new(g.x, base.round(), g.w, 1.0), t.s(0x4a5053));
    }
    if let Some((a, b)) = e.sel {
        let (xa, xb) = (x_of(s, f64::from(a)), x_of(s, f64::from(b)));
        ui.c.fill_rect(Rect::new(xa, g.y, (xb - xa).max(1.0), g.h), t.s(0x46525a));
        ui.c.fill_rect(Rect::new(xa, r.y, (xb - xa).max(1.0), r.h), accent.mix(t.s(0x2a2e30), 0.5));
    }

    // The events: each one's level held until the next.
    let points = events::points(s);
    let fill = accent.mix(t.s(0x34393b), 0.55);
    for (i, &(tick, v)) in points.iter().enumerate() {
        let xa = x_of(s, f64::from(tick)).floor();
        let xb = points.get(i + 1).map_or(g.right(), |n| x_of(s, f64::from(n.0)).floor());
        if xb < g.x || xa > g.right() {
            continue;
        }
        let y = y_of(g, events::level(e.range, v)).round();
        let top = y.min(base);
        ui.c.fill_rect(Rect::new(xa, top, (xb - xa).max(1.0), (base - y).abs().max(1.0)), fill);
        ui.c.fill_rect(Rect::new(xa, y - 1.0, (xb - xa).max(1.0), 2.0), accent);
        ui.c.fill_rect(Rect::new(xa, top, 1.0, (base - y).abs().max(1.0)), accent);
    }
    if let Some(((ta, la), (tb, lb))) = e.line {
        let (xa, xb) = (x_of(s, f64::from(ta)), x_of(s, f64::from(tb)));
        ui.c.line(xa, y_of(g, la), xb, y_of(g, lb), 2.0, t.tx(0xffffff));
    }
    ui.c.pop_clip();
    ui.c.stroke_rrect(g, 0.0, 1.0, line);

    // The levels at the top, the middle and the bottom, in the setting's own stored unit.
    for lv in [1.0, 0.5, 0.0] {
        let text = events::value(e.range, lv).to_string();
        label(ui, Font::new(12), g.x - 6.0, y_of(g, lv).clamp(g.y + 6.0, g.bottom() - 6.0), true, t.text_dim, &text);
    }
    if points.is_empty() {
        let msg = "No events for this control in this pattern: draw some";
        let w = ui.c.text_width(Font::new(14), msg);
        label(ui, Font::new(14), (g.cx() - w / 2.0).round(), g.cy(), false, t.text_dim, msg);
    }
}
