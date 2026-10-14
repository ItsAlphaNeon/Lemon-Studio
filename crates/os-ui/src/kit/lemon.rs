// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Widget looks of the Lemon design: flat rounded squares on neutral greys, one lemon brand colour; knobs stay
//! round. Every painter takes the same geometry as its Classic twin in `kit.rs`: the design changes the look,
//! never the layout. The views call the `DAW parts` below directly where they draw their own shapes.
// spec: ui/lemon-design.md

use std::f32::consts::PI;

use os_theme::design::lemon as brand;
use os_theme::{Color, Palette};

use super::{chevron, Dir, KnobLook, State};
use crate::{Canvas, Font, Rect};

/// Whether `t` draws the Lemon shapes.
pub fn on(t: &Palette) -> bool {
    t.style.lemon()
}

/// Corner radius of buttons and boxes.
pub fn r_btn(t: &Palette) -> f32 {
    t.style.radius
}

/// Corner radius of small parts.
pub fn r_small(t: &Palette) -> f32 {
    t.style.radius_small
}

/// Corner radius of panels and windows.
pub fn r_panel(t: &Palette) -> f32 {
    t.style.radius_panel
}

/// Hairline over a surface: a step toward the dark highlight.
pub fn hairline(base: Color) -> Color {
    base.mix(brand::DARK_HIGHLIGHT.with_alpha(base.a), 0.45)
}

/// A neutral face for a pointer state.
pub fn face(base: Color, st: State) -> Color {
    if st.pressed {
        base.shade(-0.06)
    } else if st.hover {
        base.mix(brand::DARK_HIGHLIGHT, 0.35)
    } else {
        base
    }
}

/// Text / glyph colour on a lemon face.
pub fn ink_on_lemon() -> Color {
    brand::COLORED_DARK
}

/// Text colour that reads on `col` (dark on light faces, light on dark ones).
pub fn ink_on(col: Color) -> Color {
    if col.hsl().2 > 0.62 { brand::COLORED_DARK } else { Color::rgb(0xf4f4f4) }
}

/// Lit (lemon) face: a soft top-light gradient from the highlight to the main colour.
pub fn lit(c: &mut Canvas, r: Rect, radius: f32, hover: bool) {
    let (top, bottom) = if hover { (brand::HIGHLIGHT, brand::MAIN.mix(brand::HIGHLIGHT, 0.35)) } else { (brand::HIGHLIGHT.mix(brand::MAIN, 0.45), brand::MAIN) };
    c.fill_rect_gradient(r, top, bottom, radius);
}

// ---------------------------------------------------------------- surfaces

/// A flat rounded panel.
pub fn panel(c: &mut Canvas, r: Rect, radius: f32, fill: Color) {
    c.fill_rrect(r, radius, fill);
}

/// A rounded panel with a 1 px outline.
pub fn outlined(c: &mut Canvas, r: Rect, radius: f32, line: Color, fill: Color) {
    c.fill_rrect(r, radius, line);
    c.fill_rrect(r.inset(1.0), (radius - 1.0).max(0.0), fill);
}

/// A display well (hint bar, monitors, time / tempo displays, number fields): darker rounded field.
pub fn well(c: &mut Canvas, t: &Palette, r: Rect, fill: Color) {
    outlined(c, r, r_btn(t).min(r.h / 2.0), fill.shade(-0.22), fill);
}

/// Raised panel: flat rounded fill.
pub fn raised(c: &mut Canvas, r: Rect, base: Color, radius: f32) {
    c.fill_rrect(r, radius.max(3.0), base);
}

// ---------------------------------------------------------------- buttons

/// Toolbar / caption button: rounded square; lemon when on or pressed; only an outline when not available.
pub fn button(c: &mut Canvas, t: &Palette, r: Rect, st: State) {
    let rr = r.inset(1.0);
    let rad = r_btn(t).min(rr.w / 2.0).min(rr.h / 2.0);
    if st.disabled && !(st.on || st.pressed) {
        c.stroke_rrect(rr, rad, 1.0, t.button.mix(t.toolbar, 0.4));
        return;
    }
    if st.on || st.pressed {
        lit(c, rr, rad, st.hover && st.on);
        return;
    }
    c.fill_rrect(rr, rad, face(t.button, st));
}

/// Small square push button (scroll arrows, spin buttons).
pub fn flat_button(c: &mut Canvas, t: &Palette, r: Rect, st: State) {
    let base = t.button.mix(t.scrollbar, 0.5);
    c.fill_rrect(r.inset(0.5), r_small(t), face(base, st));
}

pub fn arrow_button(c: &mut Canvas, t: &Palette, r: Rect, dir: Dir, st: State) {
    flat_button(c, t, r, st);
    let col = if st.hover { t.glyph } else { t.glyph.with_alpha(160) };
    chevron(c, r.cx().round(), r.cy().round(), 4.0, dir, col);
}

/// Dropdown combo: rounded dark field, text, a small down chevron.
pub fn combo(c: &mut Canvas, t: &Palette, r: Rect, text: &str, font: Font, st: State) {
    let base = t.sunken;
    outlined(c, r, r_btn(t).min(r.h / 2.0), base.shade(-0.2), face(base, st));
    let lh = c.line_height(font);
    c.text_clip(font, r.x + 6.0, (r.cy() - lh / 2.0).round(), r.w - 22.0, if st.hover { t.tx(0xffffff) } else { t.text }, text);
    chevron(c, r.right() - 10.0, r.cy(), 3.5, Dir::Down, if st.hover { brand::MAIN } else { t.text_dim });
}

/// Number box with a spin mark.
pub fn value_box(c: &mut Canvas, t: &Palette, r: Rect, text: &str, font: Font, st: State) {
    let base = t.sunken;
    outlined(c, r, r_small(t).min(r.h / 2.0), base.shade(-0.2), face(base, st));
    let lh = c.line_height(font);
    c.text_right(font, r.right() - 9.0, (r.cy() - lh / 2.0).round(), if st.hover { t.glyph } else { t.text }, text);
    super::spin_mark(c, r.right() - 4.5, r.cy(), t.text_dim);
}

/// Segmented choice (PAT / SONG, tabs): a rounded square, lemon when chosen.
pub fn pill(c: &mut Canvas, t: &Palette, r: Rect, label: &str, font: Font, st: State) {
    let rad = r_btn(t).min(r.h / 2.0);
    if st.on {
        lit(c, r, rad, st.hover);
        c.text_center(font, r, t.pill_text, label);
    } else {
        if st.hover {
            c.fill_rrect(r, rad, t.button_hi);
        }
        c.text_center(font, r, if st.hover { t.glyph } else { t.text_dim }, label);
    }
}

// ---------------------------------------------------------------- toggles

/// Enable switch in place of the round LED (same centre and radius): a rounded square, lemon when on.
pub fn led(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, radius: f32, st: State) {
    let h = radius - 0.5;
    let r = Rect::new(cx - h, cy - h, 2.0 * h, 2.0 * h);
    let rad = (radius * 0.45).max(2.0);
    let rim = if st.hover { brand::DARK_HIGHLIGHT.shade(0.15) } else { t.toolbar_line.shade(-0.2) };
    c.fill_rrect(r, rad, rim);
    let inner = r.inset(1.0);
    if st.on {
        c.fill_rect_gradient(inner, brand::HIGHLIGHT, brand::MAIN, rad - 1.0);
    } else {
        c.fill_rrect(inner, rad - 1.0, brand::COLORED_DARK.mix(t.toolbar_line, 0.35));
    }
}

/// Selection frame: a rounded lemon outline.
pub fn focus_frame(c: &mut Canvas, t: &Palette, r: Rect, width: f32) {
    c.stroke_rrect(r, r_small(t), width.min(2.0), t.accent);
}

// ---------------------------------------------------------------- knobs and faders

/// Knobs stay round: a dark disc, a track ring, the value as a lemon arc and a light pointer.
#[allow(clippy::too_many_arguments)]
pub fn knob(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, r: f32, value: f32, bipolar: bool, look: KnobLook, st: State) {
    let v = value.clamp(0.0, 1.0);
    let start = 0.75 * PI;
    let sweep = 1.5 * PI;
    let a = start + sweep * v;
    let (a0, a1) = if bipolar { (start + sweep / 2.0, a) } else { (start, a) };
    let span = (a1 - a0).abs();
    let off = st.disabled;
    let value_col = if off { brand::DARK_HIGHLIGHT } else { brand::MAIN };
    let track = t.toolbar_line.shade(-0.1);
    // Ring radius, ring width, disc radius.
    let (ring, w, disc) = match look {
        KnobLook::Mixer { halo, .. } => {
            c.fill_circle(cx, cy, halo, t.mixer_strip_dark);
            (halo - 2.0, (halo * 0.18).clamp(2.0, 3.0), r)
        }
        KnobLook::Row => (r - 0.8, 1.6, r - 2.6),
        _ => {
            let w = (r * 0.16).clamp(1.6, 2.6);
            (r - w / 2.0, w, r - w - 1.5)
        }
    };
    let show_arc = !matches!(look, KnobLook::Mixer { arc: false, .. });
    c.arc(cx, cy, ring, start, sweep, w, track);
    if show_arc && span > 0.01 {
        c.arc(cx, cy, ring, a0.min(a1), span, w, value_col);
    }
    let base = if st.hover && !off { Color::rgb(0x505050) } else { Color::rgb(0x454545) };
    c.fill_rect_gradient(Rect::new(cx - disc, cy - disc, 2.0 * disc, 2.0 * disc), base.shade(0.06), base.shade(-0.18), disc);
    let pc = if off { t.text_faint } else if st.hover { Color::rgb(0xffffff) } else { t.glyph };
    let (p0, p1) = (disc * 0.25, (disc - 1.5).max(1.0));
    c.line(cx + p0 * a.cos(), cy + p0 * a.sin(), cx + p1 * a.cos(), cy + p1 * a.sin(), (disc * 0.22).clamp(1.4, 2.2), pc);
}

/// Fader cap centred at (cx, cy) (same 14 x 23 footprint as Classic): rounded, with a centre line that turns
/// lemon when the track is selected.
pub fn fader_handle(c: &mut Canvas, _t: &Palette, cx: f32, cy: f32, selected: bool, st: State) {
    let h = Rect::new(cx - 7.0, cy - 11.0, 14.0, 23.0);
    c.fill_rrect(h.outset(1.0), 5.0, Color::rgb(0x161616));
    let base = if st.hover || st.pressed { Color::rgb(0x6a6a6a) } else { Color::rgb(0x5c5c5c) };
    c.fill_rect_gradient(h, base.shade(0.08), base.shade(-0.25), 4.0);
    let line = if selected { brand::MAIN } else { Color::rgb(0xdedede) };
    c.fill_rrect(Rect::new(h.x + 2.0, cy - 1.0, h.w - 4.0, 2.0), 1.0, line);
}

// ---------------------------------------------------------------- scrolling

pub fn scrollbar(c: &mut Canvas, t: &Palette, track: Rect, vertical: bool, pos: f32, view: f32, total: f32, st: State) {
    c.fill_rect(track, t.scrollbar);
    let len = if vertical { track.h } else { track.w };
    let (off, tl) = super::thumb(len, pos, view, total);
    let r = if vertical { Rect::new(track.x + 2.0, track.y + off + 1.0, track.w - 4.0, tl - 2.0) } else { Rect::new(track.x + off + 1.0, track.y + 2.0, tl - 2.0, track.h - 4.0) };
    let col = if st.pressed { brand::DARK_HIGHLIGHT.shade(0.15) } else if st.hover { brand::DARK_HIGHLIGHT } else { t.overview };
    c.fill_rrect(r, (r.w.min(r.h) / 2.0).max(0.0), col);
}

// ---------------------------------------------------------------- windows and popups

/// Outer frame + body of a floating window: rounded, 1 px outline, title-coloured body.
pub fn window(c: &mut Canvas, t: &Palette, r: Rect) {
    outlined(c, r, r_panel(t), t.window_border, t.window_title);
}

/// The client area well inside a floating window frame (rounded at the bottom like the window).
pub fn client(c: &mut Canvas, t: &Palette, r: Rect) {
    c.fill_rrect(r, (r_panel(t) - 3.0).max(2.0), t.window_border.mix(t.window_title, 0.3));
}

/// Popup / dropdown panel body (no shadow: it would paint outside the popup's damage rect).
pub fn popup(c: &mut Canvas, t: &Palette, r: Rect) {
    let rad = 6.0;
    outlined(c, r, rad, t.menu_pop_border, t.menu_pop);
}

/// Hovered row of a list or menu: an inset rounded bar in the coloured dark.
pub fn row_highlight(c: &mut Canvas, t: &Palette, r: Rect) {
    c.fill_rrect(Rect::new(r.x + 3.0, r.y, r.w - 6.0, r.h), 4.0_f32.min(r.h / 2.0), t.menu_pop_hi);
}

/// Window button hover face (minimize / maximize / close).
pub fn window_button_hover(c: &mut Canvas, t: &Palette, b: Rect, close: bool) {
    c.fill_rrect(b, 4.0, if close { t.record.with_alpha(200) } else { t.button_hi });
}

// ---------------------------------------------------------------- DAW parts

/// Step button of the channel rack. `odd` = odd group of four, `dim` = past the pattern end, `playing` = the
/// step playing now.
#[allow(clippy::too_many_arguments)]
pub fn step(c: &mut Canvas, t: &Palette, r: Rect, on: bool, odd: bool, dim: bool, hover: bool, playing: bool) {
    let rad = r_small(t);
    let (a, b) = if dim { (t.step_dim_a, t.step_dim_b) } else { (t.step_a, t.step_b) };
    let off = if odd { b } else { a };
    if on {
        let (top, bottom) = if dim { (brand::MAIN.mix(off, 0.45), brand::MAIN.mix(off, 0.55)) } else { (brand::HIGHLIGHT.mix(brand::MAIN, 0.5), brand::MAIN) };
        c.fill_rect_gradient(r, if hover { brand::HIGHLIGHT } else { top }, bottom, rad);
        if playing {
            c.fill_rrect(Rect::new(r.x + 2.0, r.bottom() - 4.0, r.w - 4.0, 2.0), 1.0, brand::COLORED_DARK);
        }
    } else {
        c.fill_rrect(r, rad, if hover { off.mix(brand::DARK_HIGHLIGHT, 0.4) } else { off });
    }
}

/// Channel name button (rack): `col` = the channel colour. Returns the label colour.
pub fn name_button(c: &mut Canvas, t: &Palette, r: Rect, col: Option<Color>, focused: bool, hover: bool) -> Color {
    let base = t.name_button;
    let face_c = match col {
        Some(col) => col.mix(base, 0.6),
        None => base,
    };
    let face_c = if hover { face_c.mix(brand::DARK_HIGHLIGHT, 0.3) } else { face_c };
    let rad = r_btn(t).min(r.h / 2.0);
    if focused {
        outlined(c, r, rad, brand::MAIN, face_c.mix(brand::DARK_HIGHLIGHT, 0.2));
    } else {
        c.fill_rrect(r, rad, face_c);
    }
    if let Some(col) = col {
        // A colour tab at the left edge.
        c.fill_rrect(Rect::new(r.x + 3.0, r.y + 5.0, 3.0, r.h - 10.0), 1.5, col.mix(Color::rgb(0xffffff), 0.25));
    }
    if focused { Color::rgb(0xffffff) } else { t.name_text }
}

/// A small control box (rack mixer-track and loop boxes, pattern length).
pub fn small_box(c: &mut Canvas, t: &Palette, r: Rect, face_c: Color, hover: bool) {
    outlined(c, r, r_small(t).min(r.h / 2.0), face_c.shade(-0.3), if hover { face_c.mix(brand::DARK_HIGHLIGHT, 0.3) } else { face_c });
}

/// A piano-roll / mini-roll note body.
pub fn note(c: &mut Canvas, r: Rect, fill: Color, rim: Color) {
    let rad = 3.0_f32.min(r.w / 2.0).min(r.h / 2.0);
    if r.w < 3.0 || r.h < 3.0 {
        c.fill_rect(r, fill);
        return;
    }
    c.fill_rrect(r, rad, rim);
    c.fill_rrect(r.inset(1.0), (rad - 1.0).max(0.0), fill);
}

/// Track header of the playlist: a rounded tile with an optional colour tab.
pub fn track_header(c: &mut Canvas, t: &Palette, r: Rect, col: Option<Color>, hover: bool) {
    let rad = r_btn(t).min(r.h / 2.0);
    c.fill_rrect(r, rad, if hover { t.track_header.mix(brand::DARK_HIGHLIGHT, 0.25) } else { t.track_header });
    if let Some(col) = col {
        c.fill_rrect(Rect::new(r.x + 3.0, r.y + 4.0, 3.0, (r.h - 8.0).max(2.0)), 1.5, col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_theme::Design;

    #[test]
    fn lemon_widgets_paint() {
        let mut c = Canvas::new(220, 140, 1.5).unwrap();
        let t = os_theme::design::palette(Design::Lemon, None);
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
            step(&mut c, &t, Rect::new(160.0, 90.0, 13.0, 19.0), st.on, true, false, st.hover, st.on);
            name_button(&mut c, &t, Rect::new(10.0, 100.0, 80.0, 26.0), Some(Color::rgb(0x8040c0)), st.on, st.hover);
        }
        popup(&mut c, &t, Rect::new(120.0, 80.0, 90.0, 50.0));
    }
}
