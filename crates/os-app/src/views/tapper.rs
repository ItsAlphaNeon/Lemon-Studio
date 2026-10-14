// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The Tempo tapper window: the pad (taps and the tempo), the nudge pair, Sync, Mute, now, Apply tempo and the beat bar.
// spec: ui/tempo-tapper.md §1, §3-5, §8 (Classic: the measured colours; Lemon: ui/lemon-design.md)

use os_theme::design::lemon as brand;
use os_ui::kit::lemon as lk;
use os_ui::{Color, Font, Rect};

use super::FrameInfo;
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::tapper::{TapAct, BAR_X0, BAR_X1};

pub const SIZE: (f32, f32) = (156.0, 294.0);
/// The pad, from the window's top-left (§1: x 12-143, y 31-162), and its cut corners (Classic).
const PAD: Rect = Rect::new(12.0, 31.0, 132.0, 132.0);
const CUT: f32 = 16.0;
/// The beat bar's track (§1: y 276-288).
const BAR_Y: f32 = 276.0;
const BAR_H: f32 = 13.0;

pub fn rect(s: &State) -> Rect {
    Rect::new(s.tapper.pos.0, s.tapper.pos.1, SIZE.0, SIZE.1)
}

pub fn bounds(s: &State) -> Rect {
    rect(s).outset(1.0)
}

/// The pad on screen (what a tap's flash repaints).
pub fn pad_rect(s: &State) -> Rect {
    let w = rect(s);
    Rect::new(w.x + PAD.x, w.y + PAD.y, PAD.w, PAD.h)
}

/// A round LED (Classic: dark grey, red-orange when on; Lemon: the rounded-square switch, an outline when greyed out).
fn led(ui: &mut Ui, cx: f32, cy: f32, on: bool, hover: bool, off: bool) {
    let t = ui.t;
    if super::lemon(ui) {
        if off {
            let h = 7.0;
            ui.c.stroke_rrect(Rect::new(cx - h, cy - h, 2.0 * h, 2.0 * h), 3.0, 1.0, t.s(0x34383a));
        } else {
            lk::led(ui.c, t, cx, cy, 7.5, os_ui::kit::State::new(hover, false, on));
        }
        return;
    }
    ui.c.fill_circle(cx, cy, 7.5, t.s(0x16191a));
    if on {
        ui.c.fill_circle(cx, cy, 6.5, t.h(0xe8461c));
        ui.c.fill_circle(cx, cy, 4.5, t.h(0xff6a35));
        ui.c.fill_circle(cx - 1.5, cy - 1.8, 2.0, t.h(0xffc2a0));
    } else {
        ui.c.fill_circle(cx, cy, 6.5, if hover { t.s(0x464a4d) } else { t.s(0x33373a) });
        ui.c.fill_circle(cx, cy, 4.5, t.s(0x2a2d30));
    }
}

/// Text vertically centred on `cy` from `x` (or centred on `x` when `center`).
fn label(ui: &mut Ui, f: Font, x: f32, cy: f32, center: bool, col: Color, s: &str) {
    let lh = ui.c.line_height(f);
    let y = (cy - lh / 2.0).round();
    let x = if center { (x - ui.c.text_width(f, s) / 2.0).round() } else { x };
    ui.c.text(f, x, y, col, s);
}

pub fn paint(ui: &mut Ui, s: &State, _f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let (x0, y0) = (win.x, win.y);
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::new(x0 + x, y0 + y, w, h);
    let lemon = super::lemon(ui);
    let set = crate::tapper::get();
    let tap = &s.tapper;
    // §1: background #202324, 1 px border #16191a. Lemon (spec: ui/lemon-design.md §3): a rounded window.
    let (bg, line) = (t.s(0x202324), t.s(0x16191a));
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), line, bg);
    } else {
        ui.c.fill_rect(win, line);
        ui.c.fill_rect(win.inset(1.0), bg);
    }
    ui.hit(win, Action::Swallow, "");
    // Title: the bar moves the window; neither has a hint (§1).
    ui.hit(at(1.0, 1.0, SIZE.0 - 2.0, 29.0), Action::WinDrag(Win::Tapper), "");
    label(ui, Font::new(17), x0 + 9.0, y0 + 13.0, false, t.text, "Tempo tapper");
    let close = at(137.0, 8.0, 10.0, 10.0);
    let hov = ui.hit(close.outset(3.0), Action::CloseWin(Win::Tapper), "Close");
    icons::draw(ui.c, Icon::Close, close.outset(1.0), if hov { t.tx(0xffffff) } else { t.glyph });

    // The pad (§1, §4): lit by a tap, fading back.
    let pad = at(PAD.x, PAD.y, PAD.w, PAD.h);
    ui.hit(pad, Action::Tapper(TapAct::Pad), "Tap me, or press any key on your MIDI keyboard");
    let k = tap.flash.clamp(0.0, 1.0);
    let (lit, ink) = if lemon { (brand::MAIN, lk::ink_on_lemon()) } else { (t.s(0xbdbdbd), t.s(0xbdbdbd).shade(-0.35)) };
    if lemon {
        // Flat rounded square, lemon while lit.
        let face = t.s(0x282b2c).mix(lit, k);
        lk::outlined(ui.c, pad, lk::r_panel(t), line, face);
    } else {
        // Octagon in a 1 px border, its fill shading from #232627 at the edge to #2c2f30 in the middle: a radial fill,
        // the border, then the corners cut (a 1 px diagonal of border over the window's background).
        let inner = pad.inset(1.0);
        ui.c.fill_radial(inner, t.s(0x2c2f30), t.s(0x232627));
        if k > 0.0 {
            ui.c.fill_rect(inner, lit.with_alpha((k * 255.0).round() as u8));
        }
        ui.c.stroke_rrect(pad, 0.0, 1.0, line);
        let (l, tp, r, b) = (pad.x, pad.y, pad.right(), pad.bottom());
        for (cx, cy, sx, sy) in [(l, tp, 1.0, 1.0), (r, tp, -1.0, 1.0), (l, b, 1.0, -1.0), (r, b, -1.0, -1.0)] {
            let tri = |n: f32| [(cx, cy), (cx + sx * n, cy), (cx, cy + sy * n)];
            ui.c.fill_polygon(&tri(CUT), line);
            ui.c.fill_polygon(&tri(CUT - std::f32::consts::SQRT_2), bg);
        }
    }
    let cx = pad.cx();
    match tap.count.tempo() {
        // §1: before a tempo is known, "TAP" low in the pad; then the tempo in large digits in the middle.
        None => {
            let col = if lemon { t.text_dim } else { t.tx(0xb6bbbe) };
            label(ui, Font::new(24), cx, y0 + 137.0, true, col.mix(ink, k * 0.8), "TAP");
        }
        Some(bpm) => {
            let col = if lemon { t.pill } else { t.tx(0xe2e6e8) };
            let text = format!("{bpm:.0}");
            let f = ui.c.fit_font(Font::light(51, 0), &text, pad.w - 14.0);
            label(ui, f, cx, y0 + 90.0, true, col.mix(ink, k * 0.8), &text);
        }
    }

    // − nudge / nudge + (§1, §6): not available yet, drawn greyed out.
    let dim = if lemon { t.text_faint } else { t.tx(0x9a9fa2) };
    let (left, right) = (at(20.0, 167.0, 57.0, 38.0), at(78.0, 167.0, 58.0, 38.0));
    ui.off(left, "Nudge tempo to align time (Left / pitch bend)");
    ui.off(right, "Nudge tempo to align time (Right / pitch bend)");
    if lemon {
        // Lemon's greyed-out rule: an outline only, no fill.
        for r in [left, right] {
            ui.c.stroke_rrect(r, lk::r_btn(t), 1.0, t.s(0x34383a));
        }
    } else {
        let (top, bot) = (left.y, left.bottom());
        let face = t.s(0x1e2122);
        ui.c.fill_polygon(&[(left.x, top), (left.right(), top), (left.right(), bot), (left.x + 7.0, bot)], face);
        ui.c.fill_polygon(&[(right.x, top), (right.right(), top), (right.right() - 7.0, bot), (right.x, bot)], face);
        ui.c.fill_rect(Rect::new(left.right(), top, 1.0, bot - top), line);
    }
    let nf = Font::new(15);
    label(ui, nf, left.cx() + 2.0, left.cy(), true, dim, "- nudge");
    label(ui, nf, right.cx() - 2.0, right.cy(), true, dim, "nudge +");

    // Sync, Mute (switches) and now (a button): not available yet (§6, §8); with Apply tempo off FL greys them too.
    let lf = Font::new(14);
    let cy = y0 + 231.0;
    ui.off(at(5.0, 222.0, 43.0, 18.0), "Resynchronize to taps");
    led(ui, x0 + 14.0, cy, set.sync, false, true);
    label(ui, lf, x0 + 26.0, cy, false, dim, "Sync");
    ui.off(at(59.0, 222.0, 48.0, 18.0), "Mute all audio during first taps (during playback)");
    led(ui, x0 + 68.0, cy, set.mute, false, true);
    label(ui, lf, x0 + 79.0, cy, false, dim, "Mute");
    let now = at(109.0, 219.0, 42.0, 25.0);
    ui.off(now, "Mute now, for a few bars (during playback)");
    if lemon {
        ui.c.stroke_rrect(now, lk::r_btn(t), 1.0, t.s(0x34383a));
    } else {
        ui.c.fill_polygon(&[(now.x, now.y), (now.right(), now.y), (now.right(), now.bottom()), (now.x + 6.0, now.bottom())], t.s(0x1e2122));
    }
    label(ui, lf, now.cx() + 2.0, now.cy(), true, dim, "now");
    if !lemon {
        // Classic greys out by veiling the faces with the background.
        for r in [Rect::new(left.x, left.y, right.right() - left.x, left.h), at(5.0, 222.0, 102.0, 18.0), now] {
            ui.veil(r, bg);
        }
    }

    // Apply tempo (§5).
    let ay = y0 + 261.0;
    let hov = ui.hit(at(30.0, 252.0, 88.0, 18.0), Action::Tapper(TapAct::Apply), "Apply tempo: tapped tempos set the project tempo");
    led(ui, x0 + 40.0, ay, set.apply, hov, false);
    label(ui, lf, x0 + 50.0, ay, false, if hov { t.tx(0xffffff) } else { t.text }, "Apply tempo");

    // The beat bar (§3): the taps of the count, accented on the first of each four.
    let track = at(BAR_X0, BAR_Y, BAR_X1 - BAR_X0, BAR_H);
    let radius = if lemon { lk::r_small(t) } else { BAR_H / 2.0 };
    ui.c.fill_rrect(track, radius, t.s(0x1e1f21));
    if !lemon {
        // Classic: only the right end is round.
        ui.c.fill_rect(Rect::new(track.x, track.y, radius, track.h), t.s(0x1e1f21));
    }
    if let Some((end, accent)) = tap.count.bar() {
        let col = if accent { t.h(0xe27a2c) } else { t.tx(0xabb1b5) };
        let fill = Rect::new(track.x, track.y, end - BAR_X0, track.h);
        if lemon {
            ui.c.fill_rrect(fill, radius.min(fill.w / 2.0), col);
        } else if end >= BAR_X1 {
            ui.c.fill_rrect(fill, radius, col);
            ui.c.fill_rect(Rect::new(fill.x, fill.y, radius, fill.h), col);
        } else {
            ui.c.fill_rect(fill, col);
        }
    }
}
