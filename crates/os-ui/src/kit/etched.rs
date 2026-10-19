// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Widget looks of the Etched skin: controls are see-through fills over what is behind them (the backdrop on the
//! frame) with a hairline edge, one accent colour for what is on, knobs are thin rings with a round thumb, switches are
//! round, scroll bars are thin lines that widen under the pointer. Every painter takes the same geometry as its Lemon
//! twin in `lemon.rs` (which hands over to it): the skin changes the look, never the layout.
// spec: ui/etched-design.md §8

use std::f32::consts::PI;

use os_theme::{Color, Palette};

use super::{Dir, KnobLook, State};
use crate::{Canvas, Font, Rect};

/// Whether `t` draws the Etched shapes.
pub fn on(t: &Palette) -> bool {
    t.style.etched()
}

/// Whether the colours are the dark ones (light text).
pub fn dark(t: &Palette) -> bool {
    t.text.hsl().2 > 0.5
}

fn white(a: f32) -> Color {
    Color::rgb(0xffffff).with_alpha(a.round().clamp(0.0, 255.0) as u8)
}

fn black(a: f32) -> Color {
    Color::rgb(0).with_alpha(a.round().clamp(0.0, 255.0) as u8)
}

/// The tuned strength of the see-through fills (`os_theme::etched::control_fill`).
fn strength(t: &Palette) -> f32 {
    os_theme::etched::control_fill(dark(t))
}

/// A veil over a surface: white on dark, black on light, at `a` (scaled by the tuned strength).
pub fn veil(t: &Palette, a: u8) -> Color {
    let a = f32::from(a) * strength(t);
    if dark(t) { white(a) } else { black(a) }
}

/// The fill of a neutral control for a state: a faint white on dark; on light a milky white that dims under the
/// pointer.
pub fn control(t: &Palette, st: State) -> Color {
    let k = strength(t);
    if dark(t) {
        let a: u8 = if st.disabled { 0x0b } else if st.pressed { 0x08 } else if st.hover { 0x15 } else { 0x0f };
        white(f32::from(a) * k)
    } else {
        let a: u8 = if st.disabled || st.pressed { 0x4d } else if st.hover { 0x80 } else { 0xb3 };
        white(f32::from(a) * k)
    }
}

/// The hairline of a neutral control or field: all round, a little stronger at the bottom on light.
pub fn edge(c: &mut Canvas, t: &Palette, r: Rect, rad: f32) {
    let (side, bottom) = if dark(t) { (white(18.0), None) } else { (black(15.0), Some(black(26.0))) };
    c.stroke_rrect(r.inset(0.5), (rad - 0.5).max(0.0), 1.0, side);
    if let Some(b) = bottom {
        c.fill_rect(Rect::new(r.x + rad, r.bottom() - 1.0, (r.w - 2.0 * rad).max(0.0), 1.0), b);
    }
}

/// The hairline of an accent face: a light rim, darker at the bottom.
fn accent_edge(c: &mut Canvas, r: Rect, rad: f32) {
    c.stroke_rrect(r.inset(0.5), (rad - 0.5).max(0.0), 1.0, white(20.0));
    c.fill_rect(Rect::new(r.x + rad, r.bottom() - 1.0, (r.w - 2.0 * rad).max(0.0), 1.0), black(46.0));
}

/// The hairline edge of a window or a floating panel.
pub fn panel_edge(t: &Palette) -> Color {
    if dark(t) { white(28.0) } else { black(26.0) }
}

/// An accent face (what is on): the accent, lighter under the pointer, with its rim.
pub fn accent(c: &mut Canvas, t: &Palette, r: Rect, rad: f32, hover: bool) {
    let col = if hover { t.accent.mix(Color::rgb(0xffffff), 0.12) } else { t.accent };
    c.fill_rrect(r, rad, col);
    accent_edge(c, r, rad);
}

/// The face of a thumb (knobs, faders, sliders): solid grey on dark, white on light, with a hairline.
fn thumb_face(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, r: f32) {
    let (face, line) = if dark(t) { (Color::rgb(0x454545), white(24.0)) } else { (Color::rgb(0xffffff), black(36.0)) };
    c.fill_circle(cx, cy, r, face);
    c.stroke_circle(cx, cy, r - 0.5, 1.0, line);
}

// ---------------------------------------------------------------- surfaces

/// A display well (hint bar, time displays, number fields): the field's own colour, a little see-through, with a
/// hairline.
pub fn well(c: &mut Canvas, t: &Palette, r: Rect, fill: Color) {
    let rad = t.style.radius.min(r.h / 2.0);
    let a = (f32::from(fill.a) * if dark(t) { 0.72 } else { 0.8 }).round() as u8;
    c.fill_rrect(r, rad, fill.with_alpha(a));
    edge(c, t, r, rad);
}

// ---------------------------------------------------------------- buttons

/// Toolbar / caption button: a see-through control face with a hairline; the accent when on or pressed; a faint
/// face without an edge when not available.
pub fn button(c: &mut Canvas, t: &Palette, r: Rect, st: State) {
    let rr = r.inset(1.0);
    let rad = t.style.radius.min(rr.w / 2.0).min(rr.h / 2.0);
    if st.on || st.pressed {
        return accent(c, t, rr, rad, st.hover && st.on);
    }
    c.fill_rrect(rr, rad, control(t, st));
    if !st.disabled {
        edge(c, t, rr, rad);
    }
}

/// Small square push button (scroll arrows, spin buttons): no face until the pointer is over it.
pub fn flat_button(c: &mut Canvas, t: &Palette, r: Rect, st: State) {
    if st.hover || st.pressed {
        c.fill_rrect(r.inset(0.5), t.style.radius_small, veil(t, if st.pressed { 0x0c } else { 0x14 }));
    }
}

pub fn arrow_button(c: &mut Canvas, t: &Palette, r: Rect, dir: Dir, st: State) {
    flat_button(c, t, r, st);
    super::triangle(c, r.cx().round(), r.cy().round(), 3.0, dir, if st.hover { t.glyph } else { t.glyph_dim });
}

/// Dropdown combo: a control face, the text, a small down chevron.
pub fn combo(c: &mut Canvas, t: &Palette, r: Rect, text: &str, font: Font, st: State) {
    let rad = t.style.radius.min(r.h / 2.0);
    c.fill_rrect(r, rad, control(t, st));
    edge(c, t, r, rad);
    let lh = c.line_height(font);
    c.text_clip(font, r.x + 7.0, (r.cy() - lh / 2.0).round(), r.w - 22.0, t.text, text);
    super::chevron(c, r.right() - 10.0, r.cy(), 3.0, Dir::Down, if st.hover { t.text } else { t.text_dim });
}

/// Number box: a text field (control face, hairline), the value right-aligned, the spin mark.
pub fn value_box(c: &mut Canvas, t: &Palette, r: Rect, text: &str, font: Font, st: State) {
    let rad = t.style.radius_small.min(r.h / 2.0);
    c.fill_rrect(r, rad, control(t, st));
    edge(c, t, r, rad);
    let lh = c.line_height(font);
    c.text_right(font, r.right() - 9.0, (r.cy() - lh / 2.0).round(), t.text, text);
    super::spin_mark(c, r.right() - 4.5, r.cy(), t.text_dim);
}

/// Segmented choice (PAT / SONG, tabs): the chosen one a control face with a short accent bar under its text.
pub fn pill(c: &mut Canvas, t: &Palette, r: Rect, label: &str, font: Font, st: State) {
    let rad = t.style.radius.min(r.h / 2.0);
    if st.on {
        c.fill_rrect(r, rad, control(t, State::hover(st.hover)));
        let w = 16.0_f32.min(r.w - 8.0).max(4.0);
        c.fill_rrect(Rect::new((r.cx() - w / 2.0).round(), r.bottom() - 3.0, w, 3.0), 1.5, t.accent);
        c.text_center(font, r, t.text, label);
    } else {
        if st.hover {
            c.fill_rrect(r, rad, veil(t, 0x0f));
        }
        c.text_center(font, r, if st.hover { t.text } else { t.text_dim }, label);
    }
}

// ---------------------------------------------------------------- toggles

/// Enable switch in place of the round LED (same centre and radius): a round radio, the accent with a dot when on, a
/// ring when off.
pub fn led(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, radius: f32, st: State) {
    let r = radius - 0.5;
    if st.on {
        c.fill_circle(cx, cy, r, if st.hover { t.accent.mix(Color::rgb(0xffffff), 0.12) } else { t.accent });
        c.fill_circle(cx, cy, (r * if st.hover { 0.48 } else { 0.4 }).max(1.5), t.on_glyph);
    } else {
        c.fill_circle(cx, cy, r, if dark(t) { black(40.0) } else { white(180.0) });
        c.stroke_circle(cx, cy, r - 0.5, 1.0, if st.hover { t.text_dim } else { t.text_faint });
    }
}

// ---------------------------------------------------------------- knobs, faders, sliders

/// Knobs: a thin track ring, the value as an accent arc and a round thumb on the ring at the value.
#[allow(clippy::too_many_arguments)]
pub fn knob(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, r: f32, value: f32, bipolar: bool, look: KnobLook, st: State) {
    let v = value.clamp(0.0, 1.0);
    let (start, sweep) = (0.75 * PI, 1.5 * PI);
    let a = start + sweep * v;
    let (a0, a1) = if bipolar { (start + sweep / 2.0, a) } else { (start, a) };
    let span = (a1 - a0).abs();
    // Ring radius and width.
    let (ring, w) = match look {
        KnobLook::Mixer { halo, .. } => (halo - 2.5, 2.5),
        KnobLook::Row => (r - 1.5, 2.0),
        _ => {
            let w = (r * 0.14).clamp(2.0, 3.0);
            (r - w / 2.0 - 1.0, w)
        }
    };
    let track = if dark(t) { white(40.0) } else { black(36.0) };
    c.fill_circle(cx, cy, (ring - w).max(1.0), veil(t, 0x08));
    c.arc(cx, cy, ring, start, sweep, w, track);
    let show_arc = !matches!(look, KnobLook::Mixer { arc: false, .. });
    let col = if st.disabled { t.text_faint } else { t.accent };
    if show_arc && span > 0.01 {
        c.arc(cx, cy, ring, a0.min(a1), span, w, col);
    }
    let tr = (w * 1.6).clamp(3.0, 5.0);
    let (tx, ty) = (cx + ring * a.cos(), cy + ring * a.sin());
    thumb_face(c, t, tx, ty, tr);
    let dot = tr * if st.hover || st.pressed { 0.62 } else { 0.5 };
    c.fill_circle(tx, ty, dot, col);
}

/// Fader cap centred at (cx, cy) (inside the 14 x 23 footprint): a round thumb, its dot the accent when the track is
/// selected.
pub fn fader_handle(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, selected: bool, st: State) {
    thumb_face(c, t, cx, cy, 7.0);
    let dot = if st.pressed { 3.0 } else if st.hover { 4.2 } else { 3.6 };
    c.fill_circle(cx, cy, dot, if selected { t.accent } else { t.glyph_dim });
}

/// A horizontal slider (the debug panel): a thin track, the accent up to the value, a round thumb.
pub fn slider(c: &mut Canvas, t: &Palette, track: Rect, value: f32, st: State) {
    let v = value.clamp(0.0, 1.0);
    let bar = Rect::new(track.x, track.cy() - 2.0, track.w, 4.0);
    c.fill_rrect(bar, 2.0, if dark(t) { white(60.0) } else { black(70.0) });
    let x = (track.x + track.w * v).round();
    if x > bar.x + 1.0 {
        c.fill_rrect(Rect::new(bar.x, bar.y, x - bar.x, bar.h), 2.0, t.accent);
    }
    thumb_face(c, t, x, track.cy(), 7.0);
    c.fill_circle(x, track.cy(), if st.pressed { 3.0 } else if st.hover { 4.4 } else { 3.6 }, t.accent);
}

// ---------------------------------------------------------------- scrolling

/// Scroll bar: the track's own colour, a thin rounded line for the thumb that widens under the pointer.
#[allow(clippy::too_many_arguments)]
pub fn scrollbar(c: &mut Canvas, t: &Palette, track: Rect, vertical: bool, pos: f32, view: f32, total: f32, st: State) {
    c.fill_rect(track, t.scrollbar);
    let len = if vertical { track.h } else { track.w };
    let (off, tl) = super::thumb(len, pos, view, total);
    let high = super::high_visibility();
    let across = if vertical { track.w } else { track.h };
    let thick = if st.hover || st.pressed || high { (across - 4.0).max(2.0) } else { 2.0_f32.min(across) };
    let r = if vertical {
        Rect::new(track.x + (track.w - thick) / 2.0, track.y + off + 1.0, thick, tl - 2.0)
    } else {
        Rect::new(track.x + off + 1.0, track.y + (track.h - thick) / 2.0, tl - 2.0, thick)
    };
    let col = if high { super::scroll_thumb(t, st, true) } else if st.hover || st.pressed { t.text_dim } else { t.text_faint };
    c.fill_rrect(r, (r.w.min(r.h) / 2.0).max(0.0), col);
}

// ---------------------------------------------------------------- lists and tooltips

/// Hovered row of a list or menu: an inset rounded veil (the panel shows through).
pub fn row_highlight(c: &mut Canvas, t: &Palette, r: Rect) {
    c.fill_rrect(Rect::new(r.x + 4.0, r.y, r.w - 8.0, r.h), 4.0_f32.min(r.h / 2.0), veil(t, 0x14));
}

/// Tooltip bubble body: the menu colour, radius 4, a hairline.
pub fn tooltip_body(c: &mut Canvas, t: &Palette, b: Rect) {
    c.fill_rrect(b.outset(1.0), 5.0, black(if dark(t) { 70.0 } else { 26.0 }));
    c.fill_rrect(b, 4.0, t.menu_pop);
    c.stroke_rrect(b.inset(0.5), 3.5, 1.0, panel_edge(t));
}

// ---------------------------------------------------------------- DAW parts

/// Step button of the channel rack: a face with a hairline, the accent when on.
#[allow(clippy::too_many_arguments)]
pub fn step(c: &mut Canvas, t: &Palette, r: Rect, on: bool, odd: bool, dim: bool, hover: bool, playing: bool) {
    let rad = t.style.radius_small;
    let (a, b) = if dim { (t.step_dim_a, t.step_dim_b) } else { (t.step_a, t.step_b) };
    let off = if odd { b } else { a };
    if on {
        let col = if dim { t.accent.mix(off, 0.5) } else if hover { t.accent.mix(Color::rgb(0xffffff), 0.12) } else { t.accent };
        c.fill_rrect(r, rad, col);
        accent_edge(c, r, rad);
        if playing {
            c.fill_rrect(Rect::new(r.x + 3.0, r.bottom() - 4.0, r.w - 6.0, 2.0), 1.0, t.on_glyph);
        }
    } else {
        c.fill_rrect(r, rad, off);
        if hover {
            c.fill_rrect(r, rad, veil(t, 0x14));
        }
        edge(c, t, r, rad);
    }
}

/// Channel name button (rack): a face with a hairline, the channel colour's tab at the left, an accent rim when
/// focused. Returns the label colour.
pub fn name_button(c: &mut Canvas, t: &Palette, r: Rect, col: Option<Color>, focused: bool, hover: bool) -> Color {
    let base = if focused { t.name_button_sel } else { t.name_button };
    let face = match col {
        Some(col) => col.mix(base, 0.7),
        None => base,
    };
    let rad = t.style.radius.min(r.h / 2.0);
    c.fill_rrect(r, rad, face);
    if hover {
        c.fill_rrect(r, rad, veil(t, 0x12));
    }
    if focused {
        c.stroke_rrect(r.inset(0.5), rad - 0.5, 1.0, t.accent);
    } else {
        edge(c, t, r, rad);
    }
    if let Some(col) = col {
        c.fill_rrect(Rect::new(r.x + 3.0, r.y + 6.0, 3.0, r.h - 12.0), 1.5, col.mix(Color::rgb(0xffffff), 0.2));
    }
    os_theme::fl::readable(t.name_text, face, 0.45)
}

/// A small control box (rack mixer-track and loop boxes, pattern length).
pub fn small_box(c: &mut Canvas, t: &Palette, r: Rect, face: Color, hover: bool) {
    let rad = t.style.radius_small.min(r.h / 2.0);
    c.fill_rrect(r, rad, face);
    if hover {
        c.fill_rrect(r, rad, veil(t, 0x14));
    }
    edge(c, t, r, rad);
}

/// Track header of the playlist: a tile with a hairline and an optional colour tab.
pub fn track_header(c: &mut Canvas, t: &Palette, r: Rect, col: Option<Color>, hover: bool) {
    let rad = t.style.radius.min(r.h / 2.0);
    c.fill_rrect(r, rad, t.track_header);
    if hover {
        c.fill_rrect(r, rad, veil(t, 0x10));
    }
    edge(c, t, r, rad);
    if let Some(col) = col {
        c.fill_rrect(Rect::new(r.x + 3.0, r.y + 4.0, 3.0, (r.h - 8.0).max(2.0)), 1.5, col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_theme::Design;

    #[test]
    fn etched_widgets_paint() {
        let mut c = Canvas::new(220, 140, 1.5).unwrap();
        let t = os_theme::design::palette(Design::Etched, None);
        assert!(on(&t));
        for st in [State::NORMAL, State::hover(true), State::new(false, true, false), State::new(false, false, true), State::NORMAL.with_disabled(true)] {
            super::super::button(&mut c, &t, Rect::new(2.0, 2.0, 30.0, 33.0), st);
            super::super::led(&mut c, &t, 50.0, 20.0, 6.0, st);
            for look in [KnobLook::Ring, KnobLook::Ball, KnobLook::Row, KnobLook::Mixer { halo: 15.0, arc: true }] {
                super::super::knob(&mut c, &t, 80.0, 30.0, 10.0, 0.3, true, look, st);
            }
            super::super::combo(&mut c, &t, Rect::new(2.0, 60.0, 100.0, 23.0), "All", Font::new(15), st);
            super::super::scrollbar(&mut c, &t, Rect::new(110.0, 2.0, 10.0, 100.0), true, 5.0, 10.0, 40.0, st);
            super::super::fader_handle(&mut c, &t, 150.0, 50.0, st.on, st);
            super::super::lemon::step(&mut c, &t, Rect::new(160.0, 90.0, 13.0, 19.0), st.on, true, false, st.hover, st.on);
            super::super::lemon::name_button(&mut c, &t, Rect::new(10.0, 100.0, 80.0, 26.0), Some(Color::rgb(0x8040c0)), st.on, st.hover);
            slider(&mut c, &t, Rect::new(120.0, 110.0, 90.0, 14.0), 0.4, st);
        }
    }
}
