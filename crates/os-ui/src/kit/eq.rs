//! Widgets of the EQ look (Lemon Parametric EQ): arc knobs, round buttons, fader slots and handles, band tokens.
//! Greys are palette surface roles; band colours are the caller's (signal colours, like meter reds). The Lemon design
//! draws each its own way in the same place (rounded squares for buttons).
// spec: plugins/lemon-parametric-eq-2.md (§2 tokens, §3 faders and knobs, §4 buttons); ui/lemon-design.md §11

use std::f32::consts::PI;

use os_theme::{Color, Palette};

use super::lemon::{self, plug};
use super::State;
use crate::{Canvas, Font, Rect};

/// Arc knob: dark disc and ring, the value arc in `col` from 7:30 o'clock clockwise over 270°. Hover brightens the arc.
pub fn arc_knob(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, r: f32, value: f32, col: Color, st: State) {
    if lemon::on(t) {
        lemon::dial(c, cx, cy, r + 1.0, 2.0, value, false, col, st);
        return;
    }
    let (start, sweep) = (0.75 * PI, 1.5 * PI);
    c.fill_circle(cx, cy, r + 1.0, t.s(0x15212a));
    c.arc(cx, cy, r - 0.8, start, sweep, 2.0, t.s(0x22323d));
    c.fill_circle(cx + 0.3, cy + 0.6, r - 2.8, t.s(0x1c2d37));
    c.arc(cx, cy, r - 3.0, 1.15 * PI, 0.6 * PI, 1.0, t.s(0x2a3c48));
    let v = value.clamp(0.0, 1.0);
    if v > 0.004 {
        let col = if st.hover || st.pressed { col.shade(0.25) } else { col };
        c.arc(cx, cy, r - 0.8, start, sweep * v, 2.4, col);
    }
}

/// Round button: dark disc with a lighter outline ring; `on` fills it light (labels then draw dark, see [`ink`]).
pub fn round_button(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, r: f32, st: State) {
    if lemon::on(t) {
        // A rounded square of the same size.
        lemon::plug_button(c, Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), if r >= 10.0 { 7.0 } else { 4.0 }, st);
        return;
    }
    c.fill_circle(cx, cy, r + 1.0, t.s(0x111c23));
    let ring = if st.hover || st.pressed { t.s(0x3c4f5c) } else { t.s(0x2c3c47) };
    c.stroke_circle(cx, cy, r - 0.5, 1.2, ring);
    let fill = if st.on {
        if st.hover { t.tx(0xdde3e8) } else { t.tx(0xc2cad0) }
    } else if st.pressed {
        t.s(0x101a20)
    } else if st.hover {
        t.s(0x22323d)
    } else {
        t.s(0x1a2730)
    };
    c.fill_circle(cx, cy, r - 2.0, fill);
}

/// Label / glyph colour on a [`round_button`].
pub fn ink(t: &Palette, st: State) -> Color {
    if lemon::on(t) {
        return if st.on { os_theme::design::lemon::COLORED_DARK } else if st.hover { Color::rgb(0xffffff) } else { os_theme::design::lemon::TEXT };
    }
    if st.on {
        t.s(0x23303a)
    } else if st.hover {
        t.tx(0xc8d2da)
    } else {
        t.tx(0x8c9aa6)
    }
}

/// A fader slot (rounded track with a light outline).
pub fn slot(c: &mut Canvas, t: &Palette, r: Rect, st: State) {
    if lemon::on(t) {
        lemon::outlined(c, r, r.w / 2.0, if st.hover { Color::rgb(0x111111) } else { Color::rgb(0x191919) }, plug::WELL);
        return;
    }
    let line = if st.hover { t.s(0x435662) } else { t.s(0x36454f) };
    c.fill_rrect(r, r.w / 2.0, line);
    c.fill_rrect(r.inset(1.5), (r.w / 2.0 - 1.5).max(0.0), t.s(0x1a2933));
    // Faint marks every 6 dB are drawn by the caller; a centre line here.
    c.fill_rect(Rect::new(r.cx() - 0.5, r.y + r.w / 2.0, 1.0, r.h - r.w), t.s(0x1e2e38));
}

/// Fader handle centred at (cx, cy) with a centre line in `line` (Lemon design; Classic draws [`handle`]).
pub fn handle_lined(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, line: Color, st: State) {
    if !lemon::on(t) {
        return handle(c, t, cx, cy, st);
    }
    c.fill_rrect(Rect::new(cx - 6.5, cy - 5.0, 13.0, 10.0), 3.5, Color::rgb(0x151515));
    let (a, b) = if st.hover || st.pressed { (0x7d7d7d, 0x575757) } else { (0x6f6f6f, 0x4c4c4c) };
    c.fill_rect_gradient(Rect::new(cx - 5.5, cy - 4.0, 11.0, 8.0), Color::rgb(a), Color::rgb(b), 2.5);
    c.fill_rect(Rect::new(cx - 3.5, cy - 0.5, 7.0, 1.2), line);
}

/// Fader handle centred at (cx, cy): a 10 x 9 rounded cap.
pub fn handle(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, st: State) {
    let r = Rect::new(cx - 5.0, cy - 4.5, 10.0, 9.0);
    let edge = if st.hover || st.pressed { t.tx(0xc8d2da) } else { t.tx(0x93a1ac) };
    c.fill_rrect(r.outset(1.0), 3.0, t.s(0x070b0d));
    c.fill_rrect(r, 2.5, edge);
    c.fill_rrect(r.inset(1.5), 1.5, t.s(0x0a0f11));
}

/// Band token: dark disc, a ring in `col` broken on the left and right, the band number in the middle. `alpha` dims
/// it (0 dB / muted bands), `big` grows it (hover / drag).
pub fn token(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, n: usize, col: Color, alpha: f32, big: bool) {
    let r = if big { 11.5 } else { 9.0 };
    let a = |x: Color, f: f32| x.with_alpha((f32::from(x.a) * f * alpha.clamp(0.0, 1.0)) as u8);
    if lemon::on(t) {
        // A dark disc, a full ring in the band colour, the number upright.
        c.fill_circle(cx, cy, r, a(Color::rgb(0x181818), 0.88));
        c.stroke_circle(cx, cy, r - 0.8, if big { 2.0 } else { 1.5 }, a(col, 1.0));
        let f = Font::new(12).bold();
        c.text_center(f, Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), a(col, 1.0), &(n + 1).to_string());
        return;
    }
    c.fill_circle(cx, cy, r + 1.5, a(t.s(0x0d1418), 0.55));
    c.fill_circle(cx, cy, r, a(t.s(0x1a2329), 0.88));
    let gap = 0.42;
    for base in [-PI / 2.0, PI / 2.0] {
        c.arc(cx, cy, r - 1.2, base - (PI / 2.0 - gap), PI - 2.0 * gap, 1.6, a(col, 1.0));
    }
    let f = Font::new(if big { 15 } else { 13 }).italic();
    c.text_center(f, Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), a(col.shade(0.1), 1.0), &(n + 1).to_string());
}
