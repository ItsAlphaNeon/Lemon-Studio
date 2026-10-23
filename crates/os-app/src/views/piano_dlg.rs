// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's small tool dialogs, laid out as FL's: Flam ("Piano roll - flamer", 198 × 241), Flip ("Piano roll -
//! score flipper", 162 × 160), Scale levels ("Piano roll - level scaling", 172 × 277) and Limit ("Piano roll - limiter /
//! transposer", 432 × 295). One window shows whichever is open (`piano_dlg::Ui::tool`).
// spec: ui/piano-flam.md §1, ui/piano-flip.md §1, ui/piano-level-scaling.md §1, ui/piano-limit.md §1

use os_ui::kit::{self, Dir, lemon as lk};
use os_ui::{Font, Glyph, Rect};

use super::strum::{label, led};
use super::{FrameInfo, Look};
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::knobs::KnobId;
use crate::piano_dlg::{self as dlg, Act, KB_W, KEY_W, Knob, Tool};
use crate::views::piano::{TOP_KEY, is_black, key_name};

/// Each dialog's size, by `Tool::index`; Limit's with both sections open.
const SIZES: [(f32, f32); 4] = [(198.0, 241.0), (162.0, 160.0), (172.0, 277.0), (432.0, 295.0)];

/// The panel heights under Limit's two section headers (piano-limit.md §1): a collapsed section's comes off the window.
const PANELS: [f32; 2] = [67.0, 110.0];

pub fn size(d: &dlg::Ui, tool: Tool) -> (f32, f32) {
    let (w, h) = SIZES[tool.index()];
    match tool {
        Tool::Limit => (w, h - (0..2).filter(|&i| d.collapsed[i]).map(|i| PANELS[i]).sum::<f32>()),
        _ => (w, h),
    }
}

pub fn rect(s: &State) -> Rect {
    let d = &s.piano_dlg;
    let (pos, (w, h)) = (d.pos[d.tool.index()], size(d, d.tool));
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
    let (w, h) = size(d, d.tool);
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

    // Limit draws its own section panels.
    let body = at(7.0, 28.0, w - 14.0, h - 71.0);
    if d.tool != Tool::Limit {
        if lemon {
            ui.c.stroke_rrect(body, lk::r_small(t), 1.0, line);
        } else {
            ui.c.fill_rect(body, line);
            ui.c.fill_rect(body.inset(1.0), panel);
        }
    }
    match d.tool {
        Tool::Flam => flam(ui, s, x0, y0),
        Tool::Flip => flip(ui, s, x0, y0),
        Tool::Levels => levels(ui, s, x0, y0),
        Tool::Limit => limit(ui, s, x0, y0),
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

/// Limit's keyboard (piano-limit.md §1), window-relative: its left edge; below its section's header top, the name
/// strip's top and the white keys' (the black keys start a pixel higher).
const KB_X: f32 = 8.0;
const STRIP_DY: f32 = 49.0;
const KEYS_DY: f32 = 68.0;
const WHITE_H: f32 = 45.0;
const BLACK_H: f32 = 30.0;
/// The scroll bar under the keys: its band's top below the section's header top, its handle (22 × 3).
const BAR_DY: f32 = 113.0;
const HANDLE_W: f32 = 22.0;
/// A black key's left edge right of the white key under it, by pitch class; it is 8 px wide.
const BLACK_DX: [f32; 12] = [0.0, 8.0, 0.0, 10.0, 0.0, 0.0, 8.0, 0.0, 9.0, 0.0, 10.0, 0.0];

/// Window-relative top of Limit's keyboard section, under the first section, open or collapsed.
fn kb_top(d: &dlg::Ui) -> f32 {
    28.0 + 20.0 + if d.collapsed[0] { 0.0 } else { PANELS[0] } + 7.0
}

/// Key `k`'s left edge (px from C0's) and width.
fn key_span(k: i32) -> (f32, f32) {
    if is_black(k) { (dlg::white_x(k) + BLACK_DX[k.rem_euclid(12) as usize], 8.0) } else { (dlg::white_x(k), KEY_W) }
}

/// The white key `wi` white keys up from C0.
fn white_key(wi: i32) -> i32 {
    12 * wi.div_euclid(7) + [0, 2, 4, 5, 7, 9, 11][wi.rem_euclid(7) as usize]
}

/// The key under screen point (`x`, `y`) of Limit's keyboard: a black key where one is drawn, else the white key. On
/// the name strip (`strip`) a black key counts at any height.
pub fn kb_key(s: &State, x: f32, y: f32, strip: bool) -> i32 {
    let (win, d) = (rect(s), &s.piano_dlg);
    let px = x - win.x - KB_X + d.kb_scroll;
    let white = white_key((px / KEY_W).floor() as i32);
    let black_row = strip || y < win.y + kb_top(d) + KEYS_DY - 1.0 + BLACK_H;
    let on = |k: i32| {
        let (l, w) = key_span(k);
        is_black(k) && px >= l && px < l + w
    };
    let k = [white - 1, white + 1].into_iter().find(|&k| black_row && on(k)).unwrap_or(white);
    k.clamp(0, TOP_KEY)
}

/// The scroll bar's handle (screen).
pub fn kb_handle(s: &State) -> Rect {
    let (win, d) = (rect(s), &s.piano_dlg);
    let x = KB_X + d.kb_scroll / dlg::kb_max() * (KB_W - HANDLE_W);
    Rect::new(win.x + x, win.y + kb_top(d) + BAR_DY + 7.0, HANDLE_W, 3.0)
}

/// Keyboard scroll (px) per px the handle moves.
pub fn kb_scroll_per_px() -> f32 {
    dlg::kb_max() / (KB_W - HANDLE_W)
}

/// Limit (piano-limit.md §1): the Key / scale restriction section and the Keyboard range / transpose section.
fn limit(ui: &mut Ui, s: &State, x0: f32, y0: f32) {
    let t = ui.t;
    let lf = Font::new(15);
    let d = &s.piano_dlg;
    let set = &d.limit;
    let y1 = 28.0;
    section(ui, s, x0, y0, y1, 0, "Key / scale restriction");
    if !d.collapsed[0] {
        label(ui, lf, x0 + 71.0, y0 + y1 + 38.0, 1, t.text, "Key / scale");
        label(ui, lf, x0 + 71.0, y0 + y1 + 70.0, 1, t.text, "Snap");
        for (x, y, w, act, text, hint) in [
            (78.0, 27.0, 34.0, Act::KeyMenu, crate::limit::KEYS[usize::from(set.key % 12)], "Key: the scale's first note"),
            (114.0, 27.0, 120.0, Act::ScaleMenu, set.scale_name(), "Scale: the notes move to its keys"),
            (78.0, 59.0, 74.0, Act::SnapMenu, set.snap.name(), "Snap: where a note halfway between two keys goes"),
        ] {
            let r = Rect::new(x0 + x, y0 + y1 + y, w, 22.0);
            let hov = ui.hit(r, Action::PianoDlg(act), hint);
            kit::combo(ui.c, t, r, text, lf, kit::State::hover(hov));
        }
    }
    let y2 = kb_top(d);
    let (lo, hi) = set.shown_range();
    let mut head = format!("Keyboard range ({}..{}) / transpose", key_name(i32::from(lo)), key_name(i32::from(hi)));
    if set.transpose != 0 {
        head += &format!(" ({:+})", set.transpose);
    }
    section(ui, s, x0, y0, y2, 1, &head);
    if d.collapsed[1] {
        return;
    }
    switch(ui, s, x0, y0, 22.0, y2 + 34.0, Act::Wrap, "Wrap to top / bottom", "Wrap: keys above the range go to its bottom, keys below to its top", lf, false);
    keyboard(ui, s, x0 + KB_X, y0 + y2);
    let strip = Rect::new(x0 + KB_X, y0 + y2 + STRIP_DY, KB_W, 18.0);
    ui.hit(strip, Action::PianoDlg(Act::Ruler), "Range: click or drag, the nearer end follows");
    ui.hit(Rect::new(strip.x, strip.bottom(), KB_W, KEYS_DY - STRIP_DY - 18.0 + WHITE_H), Action::PianoDlg(Act::Keys), "Transpose: click or drag, the key becomes C5");
    let bar = Rect::new(strip.x, y0 + y2 + BAR_DY, KB_W, 17.0);
    let hov = ui.hit(bar, Action::PianoDlg(Act::Scroll), "Scroll the keyboard");
    let handle = kb_handle(s);
    let ink = if hov { t.s(0x3c4043) } else { t.s(0x1f2123) };
    if super::lemon(ui) {
        ui.c.fill_rrect(handle.outset(1.0), 2.0, ink);
    } else {
        ui.c.fill_rect(handle, ink);
    }
}

/// One of Limit's sections, its header bar at window-relative `y`: the name on a darker tab, the arrow that collapses it
/// (the name then ends in "…"), and the panel under it while open.
fn section(ui: &mut Ui, s: &State, x0: f32, y0: f32, y: f32, i: u8, name: &str) {
    let t = ui.t;
    let shut = s.piano_dlg.collapsed[usize::from(i)];
    let head = Rect::new(x0 + 7.0, y0 + y, 418.0, 20.0);
    let full = Rect::new(head.x, head.y, head.w, head.h + if shut { 0.0 } else { PANELS[usize::from(i)] });
    if super::lemon(ui) {
        ui.c.fill_rrect(full, lk::r_small(t), t.s(0x272b2d));
        ui.c.fill_rrect(head, lk::r_small(t), t.s(0x232628));
    } else {
        ui.c.fill_rect(full, t.s(0x25272a));
        ui.c.fill_rect(head, t.s(0x232427));
    }
    let text = if shut { format!("{name}…") } else { name.to_string() };
    let f = Font::new(14).bold();
    let right = head.x + ui.c.text_width(f, &text) + 16.0;
    let mut g = Glyph::new();
    g.polygon(&[(head.x, head.y), (right, head.y), (right - 6.0, head.bottom()), (head.x, head.bottom())]);
    ui.c.glyph(t.s(0x1e2023), &g);
    label(ui, f, head.x + 5.0, head.cy(), -1, t.text, &text);
    let arrow = Rect::new(head.right() - 20.0, head.y, 20.0, head.h);
    let hov = ui.hit(arrow, Action::PianoDlg(Act::Collapse(i)), if shut { "Show this section" } else { "Hide this section (it still applies)" });
    kit::chevron(ui.c, arrow.cx(), arrow.cy(), 4.5, if shut { Dir::Down } else { Dir::Up }, if hov { t.tx(0xffffff) } else { t.text_dim });
}

/// Limit's keyboard, its left edge at screen `x`, its section's header top at `y`: the name strip and the keys, light
/// inside the range, and the green key (the one that becomes C5).
fn keyboard(ui: &mut Ui, s: &State, x: f32, y: f32) {
    let t = ui.t;
    let d = &s.piano_dlg;
    let set = &d.limit;
    let (lo, hi) = set.shown_range();
    let (lo, hi) = (i32::from(lo), i32::from(hi));
    let inside = |k: i32| (lo..=hi).contains(&k);
    let green = set.green();
    let green_ink = |k: i32| if inside(k) { t.h(0x5bcc86) } else { t.h(0x688e7d) };
    let c0 = x - d.kb_scroll;
    let (strip_y, keys_y) = (y + STRIP_DY, y + KEYS_DY);
    ui.c.push_clip(Rect::new(x, strip_y, KB_W, KEYS_DY - STRIP_DY + WHITE_H));
    // The name strip: the range's span light, the green key's column through it.
    ui.c.fill_rect(Rect::new(x, strip_y, KB_W, 18.0), t.s(0x868a91));
    let (a, (b, bw)) = (key_span(lo).0, key_span(hi));
    ui.c.fill_rect(Rect::new(c0 + a, strip_y, b + bw - a, 18.0), t.s(0xbabec5));
    let (gx, gw) = key_span(green);
    ui.c.fill_rect(Rect::new(c0 + gx, strip_y, gw, 18.0), green_ink(green));
    // White keys, then the black keys over them.
    let first = (d.kb_scroll / KEY_W).floor() as i32;
    let keys: Vec<i32> = (first..first + (KB_W / KEY_W) as i32 + 2).map(white_key).filter(|&k| (0..=TOP_KEY).contains(&k)).collect();
    for &k in &keys {
        let r = Rect::new(c0 + dlg::white_x(k), keys_y, KEY_W, WHITE_H);
        if k == green {
            ui.c.fill_rect(r, green_ink(k));
        } else if !inside(k) {
            ui.c.fill_rect_gradient(r, t.s(0x868a91), t.s(0x8c9197), 0.0);
        } else if k % 12 == 0 {
            ui.c.fill_rect(r, t.s(0xbabec5));
        } else {
            ui.c.fill_rect_gradient(r, t.s(0xbabfc5), t.s(0xced2da), 0.0);
        }
    }
    for k in keys.iter().map(|&k| k + 1).filter(|&k| is_black(k) && k <= TOP_KEY) {
        let (l, w) = key_span(k);
        let r = Rect::new(c0 + l, keys_y - 1.0, w, BLACK_H);
        let face = Rect::new(r.x, r.y, w - 2.0, BLACK_H - 7.0);
        if k == green {
            ui.c.fill_rect(r, green_ink(k));
        } else if inside(k) {
            ui.c.fill_rect(r, t.s(0x2f3031));
            ui.c.fill_rect_gradient(face, t.s(0x38393a), t.s(0x4f5051), 0.0);
        } else {
            ui.c.fill_rect(r, t.s(0x5b5e63));
            ui.c.fill_rect(face, t.s(0x616469));
        }
    }
    // The names: every C, and the range's two ends darker.
    let f = Font::new(12);
    for &k in keys.iter().filter(|&&k| k % 12 == 0 && k != lo && k != hi) {
        label(ui, f, c0 + dlg::white_x(k) + 2.0, strip_y + 9.0, -1, t.s(0x5a5d62), &key_name(k));
    }
    for k in [lo, hi] {
        label(ui, f.bold(), c0 + key_span(k).0 + 2.0, strip_y + 9.0, -1, t.s(0x15171a), &key_name(k));
    }
    ui.c.pop_clip();
}
