// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Message boxes, the file dialog (`dialog`, `filedlg`) and Manage shortcuts (`shortcuts`), painted over everything
//! with a veil under them. Classic draws FL's message form as measured; Lemon draws the same layout in the Lemon
//! design's rounded parts.
// spec: ui/dialogs.md §3, §4; ui/settings-choices.md "Input: Manage shortcuts (2026-10-16)"

use os_theme::design::lemon as brand;
use os_ui::kit::{self, lemon as lk};
use os_ui::{tabler, Color, Font, Rect};

use super::lemon;
use crate::app::{Action, State, Ui};
use crate::dialog::{Body, DHit, Field, Kind, Modal, Msg};
use crate::filedlg::{self as fd, FileDlg, Focus, Mode, PlaceKind, SortBy, HEAD_H, ROW_H};
use crate::icons::{self, Icon};
use crate::timesig::SigForm;

/// The frame's outer border, its colour (caption and button band), the body panel and its edge lines (spec §1a).
const BORDER: u32 = 0x16191a;
const FRAME: u32 = 0x202324;
const PANEL: u32 = 0x282c2d;
const PANEL_LINE: u32 = 0x1a1d1e;
/// Fields and lists, their outline.
const WELL: u32 = 0x1b1e1f;
const WELL_LINE: u32 = 0x141617;
/// The focus border of a field (FL's Save as form, spec §2) and a text selection.
const FOCUS_BLUE: Color = Color::rgb(0x1367b8);
const SEL_BLUE: Color = Color::rgb(0x0080ff);

/// Caption height (below it the body starts at 28).
const CAP_H: f32 = 26.0;
/// Message box: where the text starts, the icon's centre, the line pitch, width limits, button size.
const TEXT_X: f32 = 96.0;
const TEXT_R: f32 = 17.0;
const ICON_X: f32 = 47.0;
const LINE: f32 = 22.0;
const MIN_W: f32 = 260.0;
const MAX_W: f32 = 560.0;
pub(super) const BTN_W: f32 = 64.0;
pub(super) const BTN_H: f32 = 31.0;

/// Every open dialog, bottom to top, each over a veil that covers the window (and what is under it).
pub fn paint(ui: &mut Ui, s: &State) {
    let full = Rect::new(0.0, 0.0, s.screen.0, s.screen.1);
    for m in &s.dialogs {
        ui.c.fill_rect(full, Color::rgb(0).with_alpha(80));
        ui.hit(full, Action::Swallow, "");
        match &m.body {
            Body::Msg(msg) => msg_box(ui, s, m, msg),
            Body::File(f) => file_box(ui, s, m, f),
            Body::Shortcuts(f) => shortcuts_box(ui, s, m, f),
            Body::TimeSig(f) => sig_box(ui, s, m, f),
            Body::Project(f) => super::project_forms::paint(ui, s, m, f),
        }
    }
}

pub(super) fn hit(ui: &mut Ui, r: Rect, h: DHit, hint: &'static str) -> bool {
    ui.hit(r, Action::Dlg(h), hint)
}

/// Where a dialog of `w` x `h` sits: where its caption was dragged, else centred; inside the window either way.
pub(super) fn place(s: &State, m: &Modal, w: f32, h: f32) -> Rect {
    let (sw, sh) = s.screen;
    let (x, y) = m.at.unwrap_or(((sw - w) / 2.0, (sh - h) / 2.0));
    let r = Rect::new(x.min(sw - w).max(0.0).round(), y.min(sh - h).max(0.0).round(), w, h);
    m.rect.set(r);
    r
}

/// Border, caption (title, ×) and the frame colour under everything else.
pub(super) fn frame(ui: &mut Ui, r: Rect, title: &str) {
    let t = ui.t;
    let lemon = lemon(ui);
    hit(ui, r, DHit::Body, "");
    if lemon {
        ui.c.fill_rrect(r.outset(4.0), lk::r_panel(t) + 4.0, Color::rgb(0).with_alpha(60));
        lk::outlined(ui.c, r, lk::r_panel(t), t.s(BORDER), t.s(FRAME));
        ui.c.fill_rect(Rect::new(r.x + lk::r_panel(t), r.y + CAP_H, r.w - 2.0 * lk::r_panel(t), 1.0), lk::hairline(t.s(FRAME)));
    } else {
        ui.c.fill_rect(r, t.s(BORDER));
        ui.c.fill_rect(r.inset(1.0), t.s(FRAME));
    }
    let f = Font::new(13);
    let lh = ui.c.line_height(f);
    ui.c.text_clip(f, r.x + if lemon { 11.0 } else { 7.0 }, (r.y + 1.0 + (CAP_H - lh) / 2.0).round(), r.w - 40.0, t.text, title);
    hit(ui, Rect::new(r.x, r.y, r.w - 30.0, CAP_H), DHit::Caption, "Drag to move");
    let x = Rect::new(r.right() - 26.0, r.y + 3.0, 21.0, 21.0);
    let hov = hit(ui, x, DHit::Close, "Close (Esc)");
    if hov && lemon {
        lk::window_button_hover(ui.c, t, x, true);
    }
    ui.c.icon(&tabler::X, x.inset(5.0), if hov { t.glyph } else { t.text_dim });
}

/// A body panel: square with darker lines along its top and bottom (Classic), an outlined rounded panel (Lemon).
pub(super) fn panel(ui: &mut Ui, r: Rect) {
    let t = ui.t;
    if lemon(ui) {
        lk::outlined(ui.c, r, lk::r_panel(t) - 2.0, t.s(PANEL_LINE), t.s(PANEL));
    } else {
        ui.c.fill_rect(r, t.s(PANEL));
        ui.c.fill_rect(Rect::new(r.x, r.y, r.w, 1.0), t.s(PANEL_LINE));
        ui.c.fill_rect(Rect::new(r.x, r.bottom() - 1.0, r.w, 1.0), t.s(PANEL_LINE));
    }
}

// ---------------------------------------------------------------- message box

fn msg_box(ui: &mut Ui, s: &State, m: &Modal, msg: &Msg) {
    let t = ui.t;
    let lemon = lemon(ui);
    let font = Font::new(14);
    let lines = wrap(ui, font, &msg.text, MAX_W - TEXT_X - TEXT_R);
    let text_w = lines.iter().map(|l| line_width(ui, font, l)).fold(0.0, f32::max);
    let bf = Font::new(13);
    let widths: Vec<f32> = (0..msg.buttons.len()).map(|i| (ui.c.text_width(bf, msg.label(i)) + 28.0).max(BTN_W).round()).collect();
    let buttons_w = widths.iter().sum::<f32>() + 6.0 * widths.len().saturating_sub(1) as f32;
    // Remember my choice sits left of the buttons (spec: ui/settings-choices.md "Hints and warnings: Manage messages
    // and Project data size warning (2026-10-16)").
    let remember_w = if msg.remember.is_some() { super::remember_width(ui) + 24.0 } else { 0.0 };
    let w = (text_w + TEXT_X + TEXT_R).max(buttons_w + 24.0 + remember_w).max(MIN_W).ceil();
    let body_h = (27.0 + lines.len() as f32 * LINE).max(91.0);
    let r = place(s, m, w, 28.0 + body_h + 63.0);
    frame(ui, r, &msg.title);
    let body = Rect::new(r.x + 5.0, r.y + 28.0, r.w - 10.0, body_h);
    panel(ui, body);
    icon(ui, msg.kind, r.x + ICON_X, body.y + 45.0);
    for (k, line) in lines.iter().enumerate() {
        let (mut x, y) = (r.x + TEXT_X, body.y + 13.0 + k as f32 * LINE);
        for (run, hi) in line {
            let col = match (*hi, lemon) {
                (true, true) => t.brand_lit(brand::MAIN),
                (true, false) => t.h(0xff9e00),
                _ => t.text,
            };
            x += ui.c.text(font, x, y, col, run);
        }
    }
    let by = body.bottom() + 20.0;
    if let Some((_, ticked)) = msg.remember {
        super::remember_check(ui, r.x + 14.0, by + BTN_H / 2.0, ticked, Action::Dlg(DHit::Remember));
    }
    let mut bx = r.right() - 12.0 - buttons_w;
    for (i, bw) in widths.into_iter().enumerate() {
        let b = Rect::new(bx, by, bw, BTN_H);
        let act = Action::Dlg(DHit::Button(i));
        let down = ui.down(b, &act);
        let hov = ui.hit(b, act, msg.label(i));
        button(ui, b, msg.label(i), i == msg.focus, hov, down, true);
        bx += bw + 6.0;
    }
}

/// Breaks the text runs into lines at most `max_w` wide (at spaces; a longer word anywhere) and at "\n".
fn wrap(ui: &mut Ui, font: Font, text: &[(String, bool)], max_w: f32) -> Vec<Vec<(String, bool)>> {
    fn push(lines: &mut [Vec<(String, bool)>], s: &str, hi: bool) {
        let line = lines.last_mut().expect("a line");
        match line.last_mut() {
            Some((t, h)) if *h == hi => t.push_str(s),
            _ => line.push((s.to_string(), hi)),
        }
    }
    let mut lines = vec![Vec::new()];
    let mut w = 0.0;
    for (run, hi) in text {
        for (k, para) in run.split('\n').enumerate() {
            if k > 0 {
                lines.push(Vec::new());
                w = 0.0;
            }
            for word in para.split_inclusive(' ') {
                let ww = ui.c.text_width(font, word.trim_end());
                if w > 0.0 && w + ww > max_w {
                    lines.push(Vec::new());
                    w = 0.0;
                }
                let word = if w == 0.0 { word.trim_start() } else { word };
                if ww > max_w {
                    for ch in word.chars() {
                        let s = ch.to_string();
                        let cw = ui.c.text_width(font, &s);
                        if w > 0.0 && w + cw > max_w {
                            lines.push(Vec::new());
                            w = 0.0;
                        }
                        push(&mut lines, &s, *hi);
                        w += cw;
                    }
                } else if !word.is_empty() {
                    push(&mut lines, word, *hi);
                    w += ui.c.text_width(font, word);
                }
            }
        }
    }
    lines
}

fn line_width(ui: &mut Ui, font: Font, line: &[(String, bool)]) -> f32 {
    let n = line.len();
    line.iter().enumerate().map(|(i, (s, _))| ui.c.text_width(font, if i + 1 == n { s.trim_end() } else { s })).sum()
}

/// The icon in the body's left part: question, exclamation and the discs with a knocked-out "!" or "i". Classic draws
/// it light grey on the panel; Lemon on a rounded tile (spec §1b, §3).
fn icon(ui: &mut Ui, kind: Kind, cx: f32, cy: f32) {
    let t = ui.t;
    let (glyph, back, k) = if lemon(ui) {
        let tile_c = if kind == Kind::Error { t.record.mix(Color::rgb(0x161616), 0.78) } else { t.brand_lit(brand::COLORED_DARK) };
        ui.c.fill_rrect(Rect::new(cx - 28.0, cy - 28.0, 56.0, 56.0), lk::r_btn(t) + 4.0, tile_c);
        (if kind == Kind::Error { t.record } else { t.brand_lit(brand::MAIN) }, tile_c, 0.72)
    } else {
        (t.tx(0x98a2a8), t.s(PANEL), 1.0)
    };
    let c = &mut *ui.c;
    match kind {
        Kind::Question => {
            // A hook from the left over the top, down to the stem, and the dot.
            let (w, r, oy) = (7.0 * k, 10.0 * k, cy - 14.0 * k);
            c.arc(cx, oy, r, std::f32::consts::PI, 1.25 * std::f32::consts::PI, w, glyph);
            let end = (cx + r * 0.7071, oy + r * 0.7071);
            c.fill_circle(end.0, end.1, w / 2.0, glyph);
            c.line(end.0, end.1, cx, cy + 0.5 * k, w, glyph);
            c.fill_rrect(Rect::new(cx - w / 2.0, cy - 1.5 * k, w, 9.0 * k), w / 2.0, glyph);
            c.fill_circle(cx, cy + 19.0 * k, 4.5 * k, glyph);
        }
        Kind::Warning => {
            c.fill_rrect(Rect::new(cx - 4.0 * k, cy - 27.0 * k, 8.0 * k, 35.0 * k), 4.0 * k, glyph);
            c.fill_circle(cx, cy + 19.0 * k, 5.0 * k, glyph);
        }
        Kind::Error => {
            c.fill_circle(cx, cy, 27.0 * k, glyph);
            c.fill_rrect(Rect::new(cx - 3.5 * k, cy - 17.0 * k, 7.0 * k, 22.0 * k), 3.5 * k, back);
            c.fill_circle(cx, cy + 12.0 * k, 4.0 * k, back);
        }
        Kind::Info => {
            c.fill_circle(cx, cy, 27.0 * k, glyph);
            c.fill_circle(cx, cy - 13.0 * k, 4.0 * k, back);
            c.fill_rrect(Rect::new(cx - 3.5 * k, cy - 5.0 * k, 7.0 * k, 22.0 * k), 3.5 * k, back);
        }
    }
}

/// A dialog button (spec §1d, §3). `main`: the focused (message box) or default (file dialog) button. `accel`: the first
/// letter gets the dotted underline.
pub(super) fn button(ui: &mut Ui, b: Rect, label: &str, main: bool, hov: bool, down: bool, accel: bool) {
    let t = ui.t;
    let ink = if lemon(ui) {
        let rad = lk::r_btn(t);
        if main {
            lk::lit_in(ui.c, ui.t, b, rad, hov);
            lk::ink_on_lit(ui.t)
        } else {
            lk::outlined(ui.c, b, rad, t.s(0x161819), lk::face(t.s(0x2a2d2f), kit::State::new(hov, down, false)));
            if hov { t.glyph } else { t.text }
        }
    } else {
        ui.c.fill_rrect(b, 10.0, t.s(0x181b1c));
        let inner = b.inset(1.0);
        if main {
            let (top, bottom) = if down { (t.s(0x202425), t.s(0x232728)) } else if hov { (t.s(0x2f3334), t.s(0x26292a)) } else { (t.s(0x2a2e2f), t.s(0x232728)) };
            ui.c.fill_rect_gradient(inner, top, bottom, 9.0);
            ui.c.fill_rect(Rect::new(inner.x + 8.0, inner.y, inner.w - 16.0, 1.0), t.s(0x343839));
        } else {
            ui.c.fill_rrect(inner, 9.0, t.s(if down { 0x1a1d1e } else if hov { 0x232627 } else { 0x1d2021 }));
        }
        if hov { t.glyph } else { t.text }
    };
    let f = Font::new(13);
    let tw = ui.c.text_width(f, label);
    let lh = ui.c.line_height(f);
    let (x, y) = ((b.x + (b.w - tw) / 2.0).round(), (b.cy() - lh / 2.0).round());
    ui.c.text(f, x, y, ink, label);
    if accel {
        let first: String = label.chars().take(1).collect();
        let w = ui.c.text_width(f, &first);
        let base = (y + ui.c.ascent(f) + 2.0).round();
        let mut dx = 0.0;
        while dx < w - 0.5 {
            ui.c.fill_rect(Rect::new(x + dx, base, 1.0, 1.0), ink);
            dx += 2.0;
        }
    }
}

// ---------------------------------------------------------------- time signature dialog

/// PATTERNS > Set time signature... (spec: ui/pattern-time-signature.md §1): Numerator and Denominator value boxes, whose
/// lists are the app's popups (`App::sig_list`), and Accept, in FL's 156 x 143 (wider when the labels need it).
fn sig_box(ui: &mut Ui, s: &State, m: &Modal, f: &SigForm) {
    let t = ui.t;
    let font = Font::new(13);
    let labels = ["Numerator", "Denominator"];
    let label_w = labels.iter().map(|l| ui.c.text_width(font, l)).fold(0.0, f32::max).ceil();
    let box_w = 48.0;
    let r = place(s, m, (12.0 + label_w + 10.0 + box_w + 12.0).max(156.0), 143.0);
    frame(ui, r, "Time signature change");
    panel(ui, Rect::new(r.x + 5.0, r.y + 28.0, r.w - 10.0, 72.0));
    let lh = ui.c.line_height(font);
    let open = crate::timesig::list_open(s);
    for (i, (label, value)) in labels.into_iter().zip([f.num, f.den]).enumerate() {
        let den = i == 1;
        let cy = r.y + if den { 75.0 } else { 49.0 };
        ui.c.text(font, r.x + 12.0, (cy - lh / 2.0).round(), t.text, label);
        let b = Rect::new(r.right() - 12.0 - box_w, (cy - 10.0).round(), box_w, 20.0);
        let hov = hit(ui, b, DHit::SigValue(den), if den { "Denominator: click to choose" } else { "Numerator: click to choose" });
        kit::combo(ui.c, t, b, &value.to_string(), font, kit::State::new(hov, open == Some(den), false));
    }
    let bw = (ui.c.text_width(font, "Accept") + 28.0).max(BTN_W).round();
    let b = Rect::new(r.right() - 12.0 - bw, r.y + 105.0, bw, BTN_H);
    let act = Action::Dlg(DHit::Accept);
    let down = ui.down(b, &act);
    let hov = ui.hit(b, act, "Enter");
    button(ui, b, "Accept", true, hov, down, false);
}

// ---------------------------------------------------------------- file dialog

/// Where the file dialog's parts sit (spec §4a).
struct Layout {
    panel: Rect,
    back: Rect,
    up: Rect,
    new_folder: Option<Rect>,
    path: Rect,
    places: Rect,
    list: Rect,
    name_y: f32,
    name: Rect,
    combo: Option<Rect>,
}

fn layout(r: Rect, f: &FileDlg) -> Layout {
    let panel = Rect::new(r.x + 8.0, r.y + 32.0, r.w - 16.0, r.h - 32.0 - 44.0);
    let ty = panel.y + 30.0;
    let back = Rect::new(panel.x + 8.0, ty, 26.0, 26.0);
    let up = Rect::new(back.right() + 4.0, ty, 26.0, 26.0);
    let new_folder = (f.mode != Mode::Open).then(|| Rect::new(panel.right() - 34.0, ty, 26.0, 26.0));
    let path = Rect::span(up.right() + 6.0, new_folder.map_or(panel.right() - 8.0, |n| n.x - 4.0), ty, ty + 26.0);
    let name_y = panel.bottom() - 34.0;
    let combo = (f.mode != Mode::Folder && !f.filters.is_empty()).then(|| Rect::new(panel.right() - 238.0, name_y, 230.0, 26.0));
    let name = Rect::span(panel.x + 84.0, combo.map_or(panel.right() - 8.0, |c| c.x - 8.0), name_y, name_y + 26.0);
    let (y0, y1) = (ty + 34.0, name_y - 8.0);
    let places = Rect::span(panel.x + 8.0, panel.x + 158.0, y0, y1);
    let list = Rect::span(places.right() + 8.0, panel.right() - 8.0, y0, y1);
    Layout { panel, back, up, new_folder, path, places, list, name_y, name, combo }
}

fn file_box(ui: &mut Ui, s: &State, m: &Modal, f: &FileDlg) {
    let t = ui.t;
    let (sw, sh) = s.screen;
    let w = 760.0_f32.min(sw - 80.0).max(560.0_f32.min(sw));
    let h = 500.0_f32.min(sh - 80.0).max(360.0_f32.min(sh));
    let r = place(s, m, w, h);
    frame(ui, r, &f.title);
    let l = layout(r, f);
    section(ui, l.panel, "Location");
    tool(ui, l.back, Icon::Back, DHit::Back, "Back (Alt+Left)", f.can_go_back());
    tool(ui, l.up, Icon::ArrowUp, DHit::Up, "Up to the folder above (Alt+Up)", f.can_go_up());
    if let Some(n) = l.new_folder {
        tool(ui, n, Icon::FolderPlus, DHit::NewFolder, "New folder", true);
    }
    field(ui, l.path, &f.path, f.focus == Focus::Path, DHit::PathField, "The folder shown: type a path and press Enter to go there");
    places(ui, f, l.places);
    list(ui, f, l.list);
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    let label = if f.mode == Mode::Folder { "Folder" } else { "File name" };
    ui.c.text(font, l.panel.x + 10.0, (l.name_y + 13.0 - lh / 2.0).round(), t.text, label);
    field(ui, l.name, &f.name, f.focus == Focus::Name, DHit::NameField, "");
    if let Some(c) = l.combo {
        let hov = hit(ui, c, DHit::Filter, "The type of file listed");
        let label = f.chosen().map(fd::Filter::label).unwrap_or_default();
        kit::combo(ui.c, t, c, &label, font, kit::State::new(hov, f.filter_open, false));
    }
    // The band: the status, then the main button and Cancel.
    let main = match f.mode {
        Mode::Open => "Open",
        Mode::Save => "Save",
        Mode::Folder => "Select folder",
    };
    let bw = |ui: &mut Ui, s: &str| (ui.c.text_width(font, s) + 28.0).max(84.0).round();
    let (mw, cw) = (bw(ui, main), bw(ui, "Cancel"));
    let cancel = Rect::new(r.right() - 12.0 - cw, r.bottom() - 37.0, cw, 30.0);
    let ok = Rect::new(cancel.x - 6.0 - mw, cancel.y, mw, 30.0);
    let col = if f.status.is_some() { t.h(0xff9e00) } else { t.text_dim };
    ui.c.text_clip(font, r.x + 14.0, (cancel.cy() - lh / 2.0).round(), ok.x - r.x - 26.0, col, &f.status_text());
    for (b, label, h, is_main) in [(ok, main, DHit::Accept, true), (cancel, "Cancel", DHit::Cancel, false)] {
        let act = Action::Dlg(h);
        let down = ui.down(b, &act);
        let hov = ui.hit(b, act, if is_main { "Enter" } else { "Esc" });
        button(ui, b, label, is_main, hov, down, false);
    }
    if let (Some(c), true) = (l.combo, f.filter_open) {
        filter_list(ui, f, c, r);
    }
}

// ---------------------------------------------------------------- Manage shortcuts

/// Manage shortcuts: the window's width, a row's pitch, a box's size, a list entry's height.
const SC_W: f32 = 380.0;
const SC_ROW: f32 = 27.0;
const SC_BOX: (f32, f32) = (116.0, 21.0);
const SC_ITEM_H: f32 = 22.0;

/// The height of a Manage shortcuts group of `n` rows: its header, its rows.
fn sc_group_h(n: usize) -> f32 {
    30.0 + n as f32 * SC_ROW + 4.0
}

/// Manage shortcuts: a group per section, a combo per row with the row's label beside it, then Reset all and Apply; the
/// open list over everything.
fn shortcuts_box(ui: &mut Ui, s: &State, m: &Modal, f: &crate::shortcuts::Form) {
    use crate::shortcuts::{GROUPS, ROWS, TITLE};
    let t = ui.t;
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    let groups_h: f32 = GROUPS.iter().map(|g| sc_group_h(g.1) + 6.0).sum();
    let r = place(s, m, SC_W, 28.0 + 4.0 + groups_h + 47.0);
    frame(ui, r, TITLE);
    let mut y = r.y + 32.0;
    let mut row = 0;
    let mut open_box = None;
    for (title, n) in GROUPS {
        let p = Rect::new(r.x + 8.0, y, r.w - 16.0, sc_group_h(n));
        section(ui, p, title);
        for k in 0..n {
            let b = Rect::new(p.x + 10.0, p.y + 30.0 + k as f32 * SC_ROW, SC_BOX.0, SC_BOX.1);
            let open = f.open == Some(row);
            let hov = hit(ui, b, DHit::Shortcut(row), ROWS[row].label);
            kit::combo(ui.c, t, b, f.text(row), font, kit::State::new(hov, open, false));
            ui.c.text(font, b.right() + 10.0, (b.cy() - lh / 2.0).round(), t.text, ROWS[row].label);
            if open {
                open_box = Some((row, b));
            }
            row += 1;
        }
        y = p.bottom() + 6.0;
    }
    let bw = |ui: &mut Ui, s: &str| (ui.c.text_width(font, s) + 28.0).max(BTN_W).round();
    let (rw, aw) = (bw(ui, "Reset all"), bw(ui, "Apply"));
    let by = r.bottom() - 8.0 - BTN_H;
    let reset = Rect::new(r.x + 12.0, by, rw, BTN_H);
    let apply = Rect::new(r.right() - 12.0 - aw, by, aw, BTN_H);
    for (b, label, h, main, hint) in [
        (reset, "Reset all", DHit::ResetAll, false, "Put the default shortcuts in the form (Apply keeps them)"),
        (apply, "Apply", DHit::Apply, true, "Apply the shortcuts and close"),
    ] {
        let act = Action::Dlg(h);
        let down = ui.down(b, &act);
        let hov = ui.hit(b, act, hint);
        button(ui, b, label, main, hov, down, false);
    }
    if let Some((row, b)) = open_box {
        shortcut_list(ui, f, row, b, r);
    }
}

/// The open list of a Manage shortcuts box: its choices under the box (above it when there is no room), the chosen one
/// marked.
fn shortcut_list(ui: &mut Ui, f: &crate::shortcuts::Form, row: usize, b: Rect, dlg: Rect) {
    let t = ui.t;
    let choices = crate::shortcuts::ROWS[row].choices;
    let h = choices.len() as f32 * SC_ITEM_H + 6.0;
    let below = Rect::new(b.x, b.bottom() + 2.0, b.w, h);
    let p = if below.bottom() > dlg.bottom() - 4.0 { Rect::new(b.x, b.y - 2.0 - h, b.w, h) } else { below };
    if lemon(ui) {
        lk::popup(ui.c, t, p);
    } else {
        ui.c.fill_rect(p, t.menu_pop_border);
        ui.c.fill_rect(p.inset(1.0), t.menu_pop);
    }
    hit(ui, p, DHit::Body, "");
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    for (i, ch) in choices.iter().enumerate() {
        let rr = Rect::new(p.x + 1.0, p.y + 3.0 + i as f32 * SC_ITEM_H, p.w - 2.0, SC_ITEM_H);
        let hov = hit(ui, rr, DHit::ShortcutChoice(i), "");
        if hov {
            if lemon(ui) {
                lk::row_highlight(ui.c, t, rr);
            } else {
                ui.c.fill_rect(rr, t.menu_pop_hi);
            }
        }
        let col = if i == f.vals[row] { if lemon(ui) { t.brand_sel(brand::MAIN) } else { t.glyph } } else { t.text };
        ui.c.text_clip(font, rr.x + 10.0, (rr.cy() - lh / 2.0).round(), rr.w - 16.0, col, ch);
    }
}

/// A section panel with its header tab (as the Settings window's groups and FL's Save as form).
pub(super) fn section(ui: &mut Ui, p: Rect, title: &str) {
    let t = ui.t;
    let f = Font::new(13).bold();
    if lemon(ui) {
        lk::outlined(ui.c, p, lk::r_panel(t) - 2.0, t.s(PANEL_LINE), t.s(PANEL));
        ui.c.text(f, p.x + 10.0, p.y + 6.0, t.accent, title);
    } else {
        ui.c.fill_rect(p, t.s(PANEL_LINE));
        ui.c.fill_rect(p.inset(1.0), t.s(0x25292b));
        let tw = ui.c.text_width(f, title);
        let hdr = Rect::new(p.x + 1.0, p.y + 1.0, tw + 14.0, 22.0);
        ui.c.fill_rect(hdr, t.s(0x1f2324));
        for k in 0..6 {
            let k = k as f32;
            ui.c.fill_rect(Rect::new(hdr.right() + k, hdr.y, 1.0, hdr.h - k * 4.0), t.s(0x1f2324));
        }
        ui.c.text(f, p.x + 6.0, p.y + 4.0, t.text, title);
    }
}

/// A square tool button with an icon; `on` = it can be used now.
pub(super) fn tool(ui: &mut Ui, r: Rect, icon: Icon, h: DHit, hint: &'static str, on: bool) {
    let t = ui.t;
    let act = Action::Dlg(h);
    let down = on && ui.down(r, &act);
    let hov = on && ui.hit(r, act, hint);
    if lemon(ui) {
        let st = kit::State { hover: hov, pressed: down, on: false, disabled: !on };
        lk::outlined(ui.c, r, lk::r_btn(t), t.s(0x161819), lk::face(t.s(0x2a2d2f), st));
    } else {
        ui.c.fill_rect(r, t.s(WELL_LINE));
        ui.c.fill_rect(r.inset(1.0), t.s(if down { 0x1a1d1e } else if hov { 0x2c3133 } else { 0x23272a }));
    }
    let col = if !on { t.text_faint.with_alpha(110) } else if hov { t.glyph } else { t.text };
    icons::draw(ui.c, icon, r.inset(6.0), col);
}

/// A dark field or list body.
fn well(ui: &mut Ui, r: Rect, line: Option<Color>) {
    let t = ui.t;
    if lemon(ui) {
        lk::outlined(ui.c, r, lk::r_btn(t).min(r.h / 2.0), line.unwrap_or(t.s(WELL_LINE)), t.s(WELL));
    } else {
        let (c, w) = match line {
            Some(c) => (c, 2.0),
            None => (t.s(WELL_LINE), 1.0),
        };
        ui.c.fill_rect(r, c);
        ui.c.fill_rect(r.inset(w), t.s(WELL));
    }
}

/// A one-line text field: the text (scrolled so the caret shows), the selection or the caret while focused.
pub(super) fn field(ui: &mut Ui, r: Rect, fl: &Field, focused: bool, h: DHit, hint: &'static str) {
    let t = ui.t;
    let lemon = lemon(ui);
    hit(ui, r, h, hint);
    well(ui, r, focused.then_some(if lemon { t.brand_sel(brand::MAIN) } else { FOCUS_BLUE }));
    let f = Font::new(13);
    let lh = ui.c.line_height(f);
    let inner = Rect::new(r.x + 7.0, r.y + 2.0, r.w - 14.0, r.h - 4.0);
    let caret_x = ui.c.text_width(f, fl.before_caret());
    let shift = (caret_x - (inner.w - 2.0)).max(0.0);
    let (x, y) = (inner.x - shift, (r.cy() - lh / 2.0).round());
    ui.c.push_clip(inner);
    let mut ink = t.text;
    if focused && fl.all {
        let w = ui.c.text_width(f, &fl.text);
        ui.c.fill_rect(Rect::new(x - 1.0, y, w + 2.0, lh), if lemon { t.brand_sel(brand::COLORED_DARK) } else { SEL_BLUE });
        ink = if lemon { t.brand_sel(brand::HIGHLIGHT) } else { Color::rgb(0xffffff) };
    }
    ui.c.text(f, x, y, ink, &fl.text);
    if focused && !fl.all {
        ui.c.fill_rect(Rect::new((x + caret_x).round(), y, 1.0, lh), t.glyph);
    }
    ui.c.pop_clip();
}

/// A row's background: selected or hovered.
fn row_bg(ui: &mut Ui, row: Rect, sel: bool, hov: bool) {
    let t = ui.t;
    if lemon(ui) {
        if sel {
            ui.c.fill_rrect(Rect::new(row.x + 3.0, row.y + 1.0, row.w - 6.0, row.h - 2.0), 4.0, t.brand_sel(brand::COLORED_DARK));
        } else if hov {
            ui.c.fill_rrect(Rect::new(row.x + 3.0, row.y + 1.0, row.w - 6.0, row.h - 2.0), 4.0, t.s(0x262a2c));
        }
    } else if sel {
        ui.c.fill_rect(row, t.s(0x2f4a63));
    } else if hov {
        ui.c.fill_rect(row, t.s(0x24292b));
    }
}

/// Row text colour.
fn row_ink(ui: &Ui, sel: bool, hov: bool) -> Color {
    let t = ui.t;
    if sel && lemon(ui) {
        t.brand_sel(brand::HIGHLIGHT)
    } else if sel || hov {
        t.glyph
    } else {
        t.text
    }
}

fn places(ui: &mut Ui, f: &FileDlg, r: Rect) {
    let t = ui.t;
    well(ui, r, None);
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    let now = f.place_now();
    let clip = r.inset(1.0);
    ui.c.push_clip(clip);
    let mut y = r.y + 4.0;
    for (i, p) in f.places.iter().enumerate() {
        if i > 0 && p.kind == PlaceKind::Drive && f.places[i - 1].kind != PlaceKind::Drive {
            ui.c.fill_rect(Rect::new(r.x + 8.0, y + 3.0, r.w - 16.0, 1.0), t.s(0x2a2e30));
            y += 7.0;
        }
        if y > clip.bottom() {
            break;
        }
        let row = Rect::new(clip.x, y, clip.w, 22.0);
        let hov = ui.hit(row.intersect(&clip), Action::Dlg(DHit::Place(i)), p.path.display().to_string());
        let sel = now == Some(i);
        row_bg(ui, row, sel, hov);
        let shape = match p.kind {
            PlaceKind::Home => &tabler::USER,
            PlaceKind::Desktop => &tabler::LAYOUT_ROWS,
            PlaceKind::Documents => &tabler::FILE_TEXT,
            PlaceKind::Downloads => &tabler::DOWNLOAD,
            PlaceKind::Music => &tabler::MUSIC,
            PlaceKind::Start => &tabler::FOLDER_OPEN,
            PlaceKind::Drive => &tabler::BOX,
        };
        let ink = row_ink(ui, sel, hov);
        let ic = if sel && lemon(ui) { t.brand_sel(brand::MAIN) } else if sel || hov { t.glyph } else { t.text_dim };
        ui.c.icon(shape, Rect::new(row.x + 8.0, row.y + 3.0, 16.0, 16.0), ic);
        ui.c.text_clip(font, row.x + 30.0, (row.cy() - lh / 2.0).round(), row.w - 36.0, ink, &p.label);
        y += 22.0;
    }
    ui.c.pop_clip();
}

/// The icon a file row shows.
fn file_icon(name: &str) -> Icon {
    let ext = std::path::Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "flp" => Icon::FileProject,
        "wav" | "mp3" | "flac" | "ogg" | "aif" | "aiff" => Icon::FileAudio,
        "zip" => Icon::FileZip,
        "txt" | "flstheme" => Icon::FileText,
        e if e == crate::presets::EXT => Icon::FilePreset,
        _ => Icon::FileBlank,
    }
}

fn list(ui: &mut Ui, f: &FileDlg, r: Rect) {
    let t = ui.t;
    let lemon = lemon(ui);
    well(ui, r, None);
    hit(ui, r, DHit::List, "");
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    let head = Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, HEAD_H);
    let rows = Rect::span(r.x + 1.0, r.right() - 1.0, head.bottom(), r.bottom() - 1.0);
    f.view.set(rows);
    let total = f.entries.len() as f32 * ROW_H;
    let scroll = f.scroll.min((total - rows.h).max(0.0)).max(0.0);
    let bar = total > rows.h;
    let body = if bar { Rect::new(rows.x, rows.y, rows.w - 11.0, rows.h) } else { rows };
    let x_mod = body.right() - 128.0;
    let x_size = x_mod - 76.0;
    // Header: the sort column shows its direction.
    if lemon {
        ui.c.fill_rrect(head, lk::r_btn(t) - 1.0, t.s(0x222527));
        ui.c.fill_rect(Rect::new(head.x, head.bottom() - 4.0, head.w, 4.0), t.s(0x222527));
    } else {
        ui.c.fill_rect(head, t.s(0x23272a));
    }
    ui.c.fill_rect(Rect::new(head.x, head.bottom() - 1.0, head.w, 1.0), t.s(WELL_LINE));
    for (by, label, x0, x1, right) in [(SortBy::Name, "Name", head.x, x_size, false), (SortBy::Size, "Size", x_size, x_mod, true), (SortBy::Modified, "Modified", x_mod, head.right(), false)] {
        let hr = Rect::span(x0, x1, head.y, head.bottom() - 1.0);
        let hov = hit(ui, hr, DHit::Header(by), "Sort by this column (again: the other way round)");
        let col = if hov { t.glyph } else { t.text_dim };
        let tw = ui.c.text_width(font, label);
        let tx = if right { hr.right() - 8.0 - tw } else { hr.x + if by == SortBy::Name { 30.0 } else { 6.0 } };
        let ty = (hr.cy() - lh / 2.0).round();
        ui.c.text(font, tx, ty, col, label);
        if f.sort == by {
            let cx = if right { tx - 8.0 } else { tx + tw + 8.0 };
            kit::chevron(ui.c, cx, hr.cy(), 3.0, if f.reversed { kit::Dir::Up } else { kit::Dir::Down }, if lemon { t.brand_lit(brand::MAIN) } else { t.text_dim });
        }
        if by != SortBy::Name {
            ui.c.fill_rect(Rect::new(x0, hr.y + 4.0, 1.0, hr.h - 8.0), t.s(WELL_LINE));
        }
    }
    // Rows.
    ui.c.push_clip(body);
    let first = (scroll / ROW_H).floor() as usize;
    let shown = (body.h / ROW_H).ceil() as usize + 1;
    for i in first..(first + shown).min(f.entries.len()) {
        let e = &f.entries[i];
        let row = Rect::new(body.x, (body.y + i as f32 * ROW_H - scroll).round(), body.w, ROW_H);
        let hov = ui.hit(row.intersect(&body), Action::Dlg(DHit::Row(i)), "");
        let sel = f.sel == Some(i);
        row_bg(ui, row, sel, hov);
        let ink = row_ink(ui, sel, hov);
        let (ic, icol) = if e.dir { (Icon::Folder, if lemon { t.brand_lit(brand::MAIN) } else { t.browser_folder }) } else { (file_icon(&e.name), if sel || hov { t.glyph } else { t.text_dim }) };
        icons::draw(ui.c, ic, Rect::new(row.x + 8.0, row.y + 2.0, 16.0, 16.0), icol);
        let ty = (row.cy() - lh / 2.0).round();
        ui.c.text_clip(font, row.x + 30.0, ty, x_size - row.x - 36.0, ink, &e.name);
        if !e.dir {
            let size = fd::size_text(e.size);
            let sw = ui.c.text_width(font, &size);
            ui.c.text(font, x_mod - 8.0 - sw, ty, if sel { ink } else { t.text_dim }, &size);
        }
        if let Some(m) = e.modified {
            ui.c.text_clip(font, x_mod + 6.0, ty, body.right() - x_mod - 8.0, if sel { ink } else { t.text_dim }, &fd::time_text(m));
        }
    }
    if f.entries.is_empty() {
        let note = match &f.error {
            Some(e) => format!("This folder cannot be read: {e}"),
            None if f.mode == Mode::Folder => "No folders here".to_string(),
            None => "Nothing to list here".to_string(),
        };
        ui.c.text_clip(font, body.x + 12.0, body.y + 10.0, body.w - 24.0, if f.error.is_some() { t.h(0xff9e00) } else { t.text_dim }, &note);
    }
    ui.c.pop_clip();
    if bar {
        let track = Rect::new(body.right() + 1.0, rows.y + 1.0, 9.0, rows.h - 2.0);
        let act = Action::Dlg(DHit::Bar);
        let down = ui.down(track, &act);
        let hov = ui.hit(track, act, "");
        kit::scrollbar(ui.c, t, track, true, scroll, rows.h, total, kit::State::new(hov, down, false));
    }
}

/// The type combo's list, under the combo (above it when there is no room).
fn filter_list(ui: &mut Ui, f: &FileDlg, c: Rect, dlg: Rect) {
    let t = ui.t;
    let h = f.filters.len() as f32 * 22.0 + 6.0;
    let below = Rect::new(c.x, c.bottom() + 2.0, c.w, h);
    let p = if below.bottom() > dlg.bottom() - 4.0 { Rect::new(c.x, c.y - 2.0 - h, c.w, h) } else { below };
    if lemon(ui) {
        lk::popup(ui.c, t, p);
    } else {
        ui.c.fill_rect(p, t.menu_pop_border);
        ui.c.fill_rect(p.inset(1.0), t.menu_pop);
    }
    hit(ui, p, DHit::Body, "");
    let font = Font::new(13);
    let lh = ui.c.line_height(font);
    for (i, flt) in f.filters.iter().enumerate() {
        let row = Rect::new(p.x + 1.0, p.y + 3.0 + i as f32 * 22.0, p.w - 2.0, 22.0);
        let hov = hit(ui, row, DHit::FilterItem(i), "");
        if hov {
            if lemon(ui) {
                lk::row_highlight(ui.c, t, row);
            } else {
                ui.c.fill_rect(row, t.menu_pop_hi);
            }
        }
        let col = if i == f.filter { if lemon(ui) { t.brand_sel(brand::MAIN) } else { t.glyph } } else { t.text };
        ui.c.text_clip(font, row.x + 10.0, (row.cy() - lh / 2.0).round(), row.w - 16.0, col, &flt.label());
    }
}
