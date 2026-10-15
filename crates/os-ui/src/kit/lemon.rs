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

// ---------------------------------------------------------------- plugin editors

/// Colours of Lemon Studio's own plugin editors in the Lemon design ("Pith": outlined grey panels, dark wells, lemon
/// for what is on). The Lemon palette is fixed, so these are plain colours.
pub mod plug {
    use os_theme::Color;

    /// Editor background.
    pub const BG: Color = Color::rgb(0x2b2b2b);
    /// Panels and cards: outline and fill.
    pub const PANEL_LINE: Color = Color::rgb(0x232323);
    pub const PANEL: Color = Color::rgb(0x333333);
    /// Display wells (graphs, the waveform, fader slots): outline and fill.
    pub const WELL_LINE: Color = Color::rgb(0x181818);
    pub const WELL: Color = Color::rgb(0x1f1f1f);
    /// Fields (lists, button groups): outline and fill.
    pub const FIELD_LINE: Color = Color::rgb(0x1c1c1c);
    pub const FIELD: Color = Color::rgb(0x262626);
    /// Raised button face and its outline.
    pub const FACE: Color = Color::rgb(0x464646);
    pub const FACE_LINE: Color = Color::rgb(0x232323);
    /// Knob track ring.
    pub const TRACK: Color = Color::rgb(0x1d1d1d);
    /// Grid lines in wells, marks and ticks.
    pub const GRID: Color = Color::rgb(0x262626);
    pub const TICK: Color = Color::rgb(0x4a4a4a);
    /// Secondary text (captions, glyphs) and disabled text.
    pub const DIM: Color = Color::rgb(0xa3a3a3);
    pub const FAINT: Color = Color::rgb(0x7f7f7f);
    /// Captions and glyphs of controls that are greyed out (not available yet).
    pub const OFF: Color = Color::rgb(0x5f5f5f);
    /// The large plugin name in wordmarks.
    pub const WORDMARK: Color = Color::rgb(0x6b6b6b);
    /// Page strip (standalone editors): background and its bottom line.
    pub const STRIP: Color = Color::rgb(0x252525);
    pub const STRIP_LINE: Color = Color::rgb(0x1c1c1c);
}

/// Plugin knob: a full track ring, the value as an arc in `arc` over 270° (from the centre when `bipolar`), a dark disc
/// and a light pointer. `r` = outer radius, `w` = ring width. Disabled: no arc, a faint pointer.
#[allow(clippy::too_many_arguments)]
pub fn dial(c: &mut Canvas, cx: f32, cy: f32, r: f32, w: f32, value: f32, bipolar: bool, arc: Color, st: State) {
    let v = value.clamp(0.0, 1.0);
    let (start, sweep) = (0.75 * PI, 1.5 * PI);
    let a = start + sweep * v;
    let ring = r - w / 2.0;
    c.arc(cx, cy, ring, start, sweep, w, plug::TRACK);
    let a0 = if bipolar { start + sweep / 2.0 } else { start };
    if !st.disabled && (a - a0).abs() > 0.01 {
        let col = if st.hover || st.pressed { arc.mix(Color::rgb(0xffffff), 0.2) } else { arc };
        c.arc(cx, cy, ring, a0.min(a), (a - a0).abs(), w, col);
    }
    let disc = r - w - 1.5;
    dial_disc(c, cx, cy, disc, st);
    let pc = if st.disabled { plug::FAINT } else { Color::rgb(0xe6e6e6) };
    let (p0, p1) = (disc * 0.25, (disc - 1.5).max(1.0));
    c.line(cx + p0 * a.cos(), cy + p0 * a.sin(), cx + p1 * a.cos(), cy + p1 * a.sin(), (disc * 0.22).clamp(1.4, 2.2), pc);
}

/// The dark disc of a plugin knob (lighter on hover).
pub fn dial_disc(c: &mut Canvas, cx: f32, cy: f32, disc: f32, st: State) {
    let (top, bottom) = if (st.hover || st.pressed) && !st.disabled { (0x5c5c5c, 0x404040) } else { (0x525252, 0x383838) };
    c.fill_rect_gradient(Rect::new(cx - disc, cy - disc, 2.0 * disc, 2.0 * disc), Color::rgb(top), Color::rgb(bottom), disc);
}

/// Toggle in place of a round radio (same centre): a 12 px rounded square, lemon when on. Disabled options are an
/// outline, with a dim inner square when on (they still toggle).
pub fn toggle(c: &mut Canvas, cx: f32, cy: f32, on: bool, st: State) {
    let r = Rect::new(cx - 6.0, cy - 6.0, 12.0, 12.0);
    if st.disabled {
        c.stroke_rrect(r.inset(0.5), 3.5, 1.0, if st.hover { Color::rgb(0x5f5f5f) } else { Color::rgb(0x4f4f4f) });
        if on {
            c.fill_rrect(r.inset(3.0), 2.0, Color::rgb(0x6e6436));
        }
        return;
    }
    if on {
        c.fill_rrect(r, 4.0, brand::DEEP);
        let (top, bottom) = if st.hover { (Color::rgb(0xffffff).mix(brand::HIGHLIGHT, 0.5), brand::HIGHLIGHT.mix(brand::MAIN, 0.5)) } else { (brand::HIGHLIGHT, brand::MAIN) };
        c.fill_rect_gradient(r.inset(1.0), top, bottom, 3.0);
    } else {
        c.fill_rrect(r, 4.0, if st.hover { Color::rgb(0x111111) } else { plug::FIELD_LINE });
        c.fill_rrect(r.inset(1.0), 3.0, if st.hover { Color::rgb(0x363636) } else { Color::rgb(0x2a2a2a) });
    }
}

/// Panel or card: outlined, radius 10.
pub fn plug_panel(c: &mut Canvas, r: Rect) {
    outlined(c, r, 10.0, plug::PANEL_LINE, plug::PANEL);
}

/// Greys out a panel (`plug_panel`) whose controls do nothing yet: the editor background over it, at the strength of
/// the app's veil over controls that are not available.
pub fn plug_veil(c: &mut Canvas, r: Rect) {
    c.fill_rrect(r, 10.0, plug::BG.with_alpha(155));
}

/// Display well (graphs, waveform), radius `radius`.
pub fn plug_well(c: &mut Canvas, r: Rect, radius: f32) {
    outlined(c, r, radius, plug::WELL_LINE, plug::WELL);
}

/// Field (list box, button group): outlined dark rounded box; hover lifts it, pressed darkens it.
pub fn field(c: &mut Canvas, r: Rect, st: State) {
    let fill = if st.pressed { Color::rgb(0x1f1f1f) } else if st.hover { Color::rgb(0x2f2f2f) } else { plug::FIELD };
    outlined(c, r, 6.0_f32.min(r.h / 2.0), plug::FIELD_LINE, fill);
}

/// Raised button (rounded square, radius `radius`): grey face with a dark outline, lit lemon when `st.on`; disabled,
/// only an outline (olive when on, as a disabled `toggle`). Returns the colour for its label or glyph.
pub fn plug_button(c: &mut Canvas, r: Rect, radius: f32, st: State) -> Color {
    if st.disabled {
        c.stroke_rrect(r.inset(0.5), (radius - 0.5).max(0.0), 1.0, if st.on { Color::rgb(0x6e6436) } else { plug::FACE });
        return plug::OFF;
    }
    if st.on {
        c.fill_rrect(r, radius, brand::DEEP);
        lit(c, r.inset(1.0), (radius - 1.0).max(0.0), st.hover);
        return brand::COLORED_DARK;
    }
    let fill = if st.pressed { Color::rgb(0x3a3a3a) } else if st.hover { Color::rgb(0x525252) } else { plug::FACE };
    outlined(c, r, radius, plug::FACE_LINE, fill);
    if st.hover { Color::rgb(0xffffff) } else { brand::TEXT }
}

/// Text drawn letter by letter with `track` px between letters, left edge at `x`, top at `y`; returns its width.
pub fn spaced(c: &mut Canvas, f: Font, x: f32, y: f32, col: Color, s: &str, track: f32) -> f32 {
    let mut cx = x;
    for (i, ch) in s.chars().enumerate() {
        if i > 0 {
            cx += track;
        }
        let mut b = [0; 4];
        let g = ch.encode_utf8(&mut b);
        c.text(f, cx.round(), y, col, g);
        cx += c.text_width(f, g);
    }
    cx - x
}

/// Width of [`spaced`] text.
pub fn spaced_width(c: &mut Canvas, f: Font, s: &str, track: f32) -> f32 {
    let n = s.chars().count();
    s.chars().map(|ch| c.text_width(f, ch.encode_utf8(&mut [0; 4]))).sum::<f32>() + track * n.saturating_sub(1) as f32
}

/// The small lemon drawn next to plugin wordmarks: a tilted oval with a nub at each end, `s` = half its length.
pub fn lemon_mark(c: &mut Canvas, cx: f32, cy: f32, s: f32, col: Color) {
    let (sin, cos) = (-0.5f32).sin_cos();
    let at = |x: f32, y: f32| (cx + x * cos - y * sin, cy + x * sin + y * cos);
    let oval = |ox: f32, rx: f32, ry: f32| -> Vec<(f32, f32)> {
        (0..24)
            .map(|i| {
                let a = i as f32 / 24.0 * 2.0 * PI;
                at(ox + rx * a.cos(), ry * a.sin())
            })
            .collect()
    };
    c.fill_polygon(&oval(0.0, s, s * 0.72), col);
    c.fill_polygon(&oval(s * 1.02, s * 0.18, s * 0.14), col);
    c.fill_polygon(&oval(-s * 1.02, s * 0.18, s * 0.14), col);
}

/// A plugin wordmark: the lemon mark and "LEMON" small in lemon above `name` large and grey, right-aligned at
/// `right` (or left-aligned at `right` when `left`). `top` = top of the small line.
pub fn wordmark(c: &mut Canvas, x: f32, top: f32, left: bool, name: &str, big: Font, small: Font) {
    let (wn, ws) = (spaced_width(c, big, name, 3.0), spaced_width(c, small, "LEMON", 3.0));
    let lh_s = c.line_height(small);
    let x0 = if left { x } else { x - wn };
    let sx = if left { x0 + 12.0 } else { (x - ws).max(x0 + 12.0) };
    lemon_mark(c, sx - 8.0, top + lh_s / 2.0, 4.5, brand::MAIN);
    spaced(c, small, sx, top, brand::MAIN, "LEMON", 3.0);
    spaced(c, big, x0, top + lh_s - 2.0, plug::WORDMARK, name, 3.0);
}

/// A text tab of a set (envelope targets), `x` = the text's left edge: the chosen one is a dark rounded pill with its
/// name in `col`.
#[allow(clippy::too_many_arguments)]
pub fn text_tab(c: &mut Canvas, cell: Rect, x: f32, name: &str, font: Font, col: Color, active: bool, st: State) {
    let pill = Rect::new(cell.x, cell.y + 4.0, cell.w, cell.h - 8.0);
    if active {
        outlined(c, pill, 7.0, plug::FIELD_LINE, plug::FIELD);
    } else if st.hover {
        c.fill_rrect(pill, 7.0, Color::rgb(0x333333));
    }
    let ink = if active { col } else if st.hover { brand::TEXT } else { plug::DIM };
    let lh = c.line_height(font);
    c.text(font, x, (cell.cy() - lh / 2.0).round(), ink, name);
}

/// A small button of a set (LFO shapes): the chosen one on a dark rounded square. Returns its glyph colour (`col`
/// when chosen, `plug::OFF` when disabled).
pub fn choice_button(c: &mut Canvas, r: Rect, on: bool, col: Color, st: State) -> Color {
    if st.disabled {
        if on {
            outlined(c, r, 5.0, plug::FIELD_LINE, plug::FIELD);
        }
        return plug::OFF;
    }
    if on {
        outlined(c, r, 5.0, plug::FIELD_LINE, plug::FIELD);
    } else if st.hover {
        c.fill_rrect(r, 5.0, Color::rgb(0x3d3d3d));
    }
    if on { col } else if st.hover { brand::TEXT } else { plug::DIM }
}

/// The page strip of a standalone plugin editor (`r` = the strip row): the chosen page's cell lit lemon. `icon`
/// paints page `i`'s icon centred at (x, y) in the given colour.
#[allow(clippy::too_many_arguments)]
pub fn page_strip(c: &mut Canvas, r: Rect, pages: u8, active: u8, hover: Option<u8>, cell_x: &dyn Fn(u8) -> f32, icon: &mut dyn FnMut(&mut Canvas, u8, f32, f32, Color)) {
    c.fill_rect(r, plug::STRIP);
    c.fill_rect(Rect::new(r.x, r.bottom() - 1.0, r.w, 1.0), plug::STRIP_LINE);
    for i in 0..pages {
        let x = cell_x(i);
        let cell = Rect::new(x + 4.0, r.y + 5.0, 37.0, r.h - 11.0);
        let on = i == active;
        if on {
            lit(c, cell, 8.0, false);
        } else if hover == Some(i) {
            c.fill_rrect(cell, 8.0, Color::rgb(0x333333));
        }
        let col = if on { brand::COLORED_DARK } else if hover == Some(i) { brand::TEXT } else { plug::DIM };
        icon(c, i, cell.cx(), cell.cy(), col);
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
