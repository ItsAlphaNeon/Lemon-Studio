// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Widgets of the EQ look (Lemon Parametric EQ): arc knobs, rounded buttons, fader slots and handles, band tokens.
//! Lemon design only (the plugins have no Classic look); band colours are the caller's.
// spec: plugins/lemon-parametric-eq-2.md (§2 tokens, §3 faders and knobs, §4 buttons); ui/lemon-design.md §11.1

use os_theme::design::lemon as brand;
use os_theme::Color;

use super::lemon::{self, plug};
use super::State;
use crate::{Canvas, Font, Rect};

/// Arc knob of radius `r`: the value arc in `col` over 270°. Hover brightens the arc.
pub fn arc_knob(c: &mut Canvas, cx: f32, cy: f32, r: f32, value: f32, col: Color, st: State) {
    lemon::dial(c, cx, cy, r + 1.0, 2.0, value, false, col, st);
}

/// Button of radius `r`: a rounded square of the same size, lit when `st.on` (labels then draw dark, see [`ink`]).
pub fn round_button(c: &mut Canvas, cx: f32, cy: f32, r: f32, st: State) {
    lemon::plug_button(c, Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), if r >= 10.0 { 7.0 } else { 4.0 }, st);
}

/// Label / glyph colour on a [`round_button`].
pub fn ink(st: State) -> Color {
    if st.disabled {
        plug::OFF
    } else if st.on {
        brand::COLORED_DARK
    } else if st.hover {
        Color::rgb(0xffffff)
    } else {
        brand::TEXT
    }
}

/// A fader slot: an outlined rounded well.
pub fn slot(c: &mut Canvas, r: Rect, st: State) {
    lemon::outlined(c, r, r.w / 2.0, if st.hover { Color::rgb(0x111111) } else { Color::rgb(0x191919) }, plug::WELL);
}

/// Fader handle centred at (cx, cy): a grey cap with a centre line in `line`.
pub fn handle(c: &mut Canvas, cx: f32, cy: f32, line: Color, st: State) {
    c.fill_rrect(Rect::new(cx - 6.5, cy - 5.0, 13.0, 10.0), 3.5, Color::rgb(0x151515));
    let (a, b) = if st.hover || st.pressed { (0x7d7d7d, 0x575757) } else { (0x6f6f6f, 0x4c4c4c) };
    c.fill_rect_gradient(Rect::new(cx - 5.5, cy - 4.0, 11.0, 8.0), Color::rgb(a), Color::rgb(b), 2.5);
    c.fill_rect(Rect::new(cx - 3.5, cy - 0.5, 7.0, 1.2), line);
}

/// Band token: a dark disc, a full ring in `col`, the band number upright in the middle. `alpha` dims it (0 dB /
/// muted bands), `big` grows it (hover / drag).
pub fn token(c: &mut Canvas, cx: f32, cy: f32, n: usize, col: Color, alpha: f32, big: bool) {
    let r = if big { 11.5 } else { 9.0 };
    let a = |x: Color, f: f32| x.with_alpha((f32::from(x.a) * f * alpha.clamp(0.0, 1.0)) as u8);
    c.fill_circle(cx, cy, r, a(Color::rgb(0x181818), 0.88));
    c.stroke_circle(cx, cy, r - 0.8, if big { 2.0 } else { 1.5 }, a(col, 1.0));
    let f = Font::new(12).bold();
    c.text_center(f, Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), a(col, 1.0), &(n + 1).to_string());
}
