// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's widget kit: every control the app and Lemon plugins draw, in one place.
//!
//! Widgets are pure painters: `kit::<widget>(canvas, &palette, rect/centre, value, State)`. Hit-testing and actions
//! stay with the caller (immediate mode). All colours come from the theme palette (named tokens or the role
//! mappers `Palette::s/a/h/tx/led`), all sizes from `Metrics`, so a theme or a metric change restyles everything.
// spec: ui/main-window.md §1 (button faces, LEDs), ui/knobs.md, ui/flstheme.md (roles)

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};

use os_theme::{Color, Palette};

use crate::{Canvas, Font, Glyph, Rect};

pub mod eq;
pub mod inst;
pub mod lemon;
pub mod menu;

/// Interaction state of a control. `on` (latched / active) is separate from the pointer state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct State {
    pub hover: bool,
    pub pressed: bool,
    pub on: bool,
    pub disabled: bool,
}

impl State {
    pub const NORMAL: State = State { hover: false, pressed: false, on: false, disabled: false };
    pub fn hover(hover: bool) -> State {
        State { hover, ..State::NORMAL }
    }
    pub fn new(hover: bool, pressed: bool, on: bool) -> State {
        State { hover, pressed, on, disabled: false }
    }
    pub fn with_on(self, on: bool) -> State {
        State { on, ..self }
    }
    pub fn with_disabled(self, disabled: bool) -> State {
        State { disabled, ..self }
    }
}

/// Shared measurements (logical px). One place to retune the whole UI.
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    /// Corner radius of buttons, boxes and combos.
    pub radius: f32,
    /// Corner radius of small parts (steps, slots).
    pub radius_small: f32,
    /// Lightening of a face on hover (`Color::shade` amount).
    pub hover_lift: f32,
    /// Darkening of a face while pressed.
    pub press_sink: f32,
    /// Window title bar height.
    pub title_h: f32,
    /// Scroll bar thickness.
    pub scrollbar: f32,
    /// Minimum scroll bar thumb length.
    pub thumb_min: f32,
}

pub const METRICS: Metrics =
    Metrics { radius: 3.0, radius_small: 2.5, hover_lift: 0.08, press_sink: -0.06, title_h: 23.0, scrollbar: 10.0, thumb_min: 16.0 };

/// Face colour of a neutral (surface) control for a state.
pub fn face(base: Color, st: State) -> Color {
    let m = METRICS;
    let c = if st.pressed { base.shade(m.press_sink) } else if st.hover { base.shade(m.hover_lift) } else { base };
    if st.disabled { c.mix(base, 0.5).shade(-0.1) } else { c }
}

/// Glyph / label colour for a state on a neutral face.
pub fn ink(t: &Palette, st: State) -> Color {
    if st.disabled {
        t.text_faint
    } else if st.on || st.pressed {
        t.on_glyph
    } else if st.hover {
        t.tx(0xffffff)
    } else {
        t.glyph
    }
}

// ---------------------------------------------------------------- surfaces

/// Raised panel: soft vertical gradient, lighter at the top.
pub fn raised(c: &mut Canvas, r: Rect, base: Color, radius: f32) {
    c.fill_rect_gradient(r, base.shade(0.045), base.shade(-0.035), radius);
}

/// `raised` in the palette's design (Lemon: a flat rounded fill).
pub fn raised_in(c: &mut Canvas, t: &Palette, r: Rect, base: Color, radius: f32) {
    if lemon::on(t) {
        return lemon::raised(c, r, base, radius);
    }
    raised(c, r, base, radius);
}

/// Sunken panel (hint bar, monitors, value displays): flat fill inside a 1 px dark border.
pub fn sunken(c: &mut Canvas, t: &Palette, r: Rect, fill: Color) {
    if lemon::on(t) {
        return lemon::well(c, t, r, fill);
    }
    c.fill_rect(r, t.s(0x1d2022));
    c.fill_rect(r.inset(1.0), fill);
}

/// A 1 px outline with a fill (rounded).
pub fn bordered(c: &mut Canvas, r: Rect, radius: f32, border: Color, fill: Color) {
    c.fill_rrect(r, radius, border);
    c.fill_rrect(r.inset(1.0), (radius - 0.5).max(0.0), fill);
}

/// Dotted vertical separator (four 2x2 dots).
pub fn grip(c: &mut Canvas, t: &Palette, x: f32, y: f32) {
    for k in 0..4 {
        c.fill_rect(Rect::new(x, y + 4.0 * k as f32, 2.0, 2.0), t.s(0x4c4b47));
    }
}

// ---------------------------------------------------------------- buttons

/// The FL-style raised button face (spec §1): 1 px border with softened corners, an elliptical radial face
/// (light centre, darker edges) and a darker 1 px inner rim. `on`/pressed light it orange (Highlight role).
pub fn button(c: &mut Canvas, t: &Palette, r: Rect, st: State) {
    if lemon::on(t) {
        return lemon::button(c, t, r, st);
    }
    let bg = t.toolbar;
    let lit = st.on || st.pressed;
    if st.disabled && !lit {
        // Not available: no raised face, just a faint outline on the background.
        c.fill_rect(r, bg.mix(t.s(0x191d1f), 0.45));
        c.fill_rect(r.inset(1.0), bg.shade(-0.03));
        return;
    }
    let (border, center, edge) = if st.on {
        (t.h(0xbe5507), t.h(0xfffaab), t.h(0xffaf65))
    } else if st.pressed {
        (t.h(0xbe5507), t.h(0xffe598), t.h(0xf99448))
    } else if st.hover {
        (t.s(0x191d1f), t.s(0x3d4244), t.s(0x2c3133))
    } else {
        (t.s(0x191d1f), t.s(0x313537), t.s(0x272b2d))
    };
    c.fill_rect(r, border);
    for (x, y) in [(r.x, r.y), (r.right() - 1.0, r.y), (r.x, r.bottom() - 1.0), (r.right() - 1.0, r.bottom() - 1.0)] {
        c.pixel(x, y, bg.mix(border, 0.25));
    }
    let f = r.inset(1.0);
    c.fill_radial(f, center, edge);
    let rim = Color::rgb(0).with_alpha(if lit { 26 } else { 40 });
    c.fill_rect(Rect::new(f.x, f.y, f.w, 1.0), rim);
    c.fill_rect(Rect::new(f.x, f.bottom() - 1.0, f.w, 1.0), rim);
    c.fill_rect(Rect::new(f.x, f.y + 1.0, 1.0, f.h - 2.0), rim);
    c.fill_rect(Rect::new(f.right() - 1.0, f.y + 1.0, 1.0, f.h - 2.0), rim);
}

/// A button with an icon (glyph drawn by the caller-supplied painter so app and plugin icon sets both work).
pub fn icon_button(c: &mut Canvas, t: &Palette, r: Rect, st: State, icon: &crate::tabler::TablerIcon, pad: f32) {
    button(c, t, r, st);
    c.icon(icon, r.inset(pad), ink(t, st));
}

/// Flat bevelled push button (small square buttons: scroll arrows, spin buttons).
pub fn flat_button(c: &mut Canvas, t: &Palette, r: Rect, st: State) {
    if lemon::on(t) {
        return lemon::flat_button(c, t, r, st);
    }
    c.fill_rect(r, t.s(0x131618));
    let (a, b) = (face(t.s(0x36383a), st), face(t.s(0x2c2e30), st));
    c.fill_rect_gradient(r.inset(1.0), a, b, 0.0);
}

/// Arrow glyph direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Open chevron (stroke) centred at (cx, cy), `size` = half height.
pub fn chevron(c: &mut Canvas, cx: f32, cy: f32, size: f32, dir: Dir, col: Color) {
    let (s, h) = (size, size * 0.55);
    let pts = match dir {
        Dir::Left => [(cx + h, cy - s), (cx - h, cy), (cx + h, cy + s)],
        Dir::Right => [(cx - h, cy - s), (cx + h, cy), (cx - h, cy + s)],
        Dir::Up => [(cx - s, cy + h), (cx, cy - h), (cx + s, cy + h)],
        Dir::Down => [(cx - s, cy - h), (cx, cy + h), (cx + s, cy - h)],
    };
    c.polyline(&pts, 1.2, col);
}

/// Solid triangle (menu arrows, combo arrows) centred at (cx, cy).
pub fn triangle(c: &mut Canvas, cx: f32, cy: f32, size: f32, dir: Dir, col: Color) {
    let (s, h) = (size, size * 0.55);
    let pts = match dir {
        Dir::Left => [(cx + h, cy - s), (cx - h, cy), (cx + h, cy + s)],
        Dir::Right => [(cx - h, cy - s), (cx + h, cy), (cx - h, cy + s)],
        Dir::Up => [(cx - s, cy + h), (cx, cy - h), (cx + s, cy + h)],
        Dir::Down => [(cx - s, cy - h), (cx, cy + h), (cx + s, cy - h)],
    };
    let mut g = Glyph::new();
    g.polygon(&pts);
    c.glyph(col, &g);
}

/// Scroll-bar end / stepper button with an arrow.
pub fn arrow_button(c: &mut Canvas, t: &Palette, r: Rect, dir: Dir, st: State) {
    if lemon::on(t) {
        return lemon::arrow_button(c, t, r, dir, st);
    }
    flat_button(c, t, r, st);
    let col = if st.hover { t.glyph } else { t.glyph.with_alpha(150) };
    chevron(c, r.cx().round(), r.cy().round(), 5.0, dir, col);
}

/// A scroll bar's end button: `arrow_button`, with the light arrow of high visibility while it is on
/// (`scroll_arrow`).
pub fn scroll_arrow_button(c: &mut Canvas, t: &Palette, r: Rect, dir: Dir, st: State) {
    let Some(col) = scroll_arrow(t, st, high_visibility()) else { return arrow_button(c, t, r, dir, st) };
    flat_button(c, t, r, st);
    chevron(c, r.cx().round(), r.cy().round(), if lemon::on(t) { 4.0 } else { 5.0 }, dir, col);
}

/// Small up/down spin mark next to a number.
pub fn spin_mark(c: &mut Canvas, x: f32, cy: f32, col: Color) {
    c.polyline(&[(x - 1.5, cy - 6.0), (x + 1.0, cy - 4.0), (x - 1.5, cy - 2.0)], 1.0, col);
    c.polyline(&[(x + 1.0, cy + 2.0), (x - 1.5, cy + 4.0), (x + 1.0, cy + 6.0)], 1.0, col);
}

/// Dropdown combo: dark box, text, right arrow.
pub fn combo(c: &mut Canvas, t: &Palette, r: Rect, text: &str, font: Font, st: State) {
    if lemon::on(t) {
        return lemon::combo(c, t, r, text, font, st);
    }
    let m = METRICS;
    bordered(c, r, m.radius, t.s(0x161819), face(t.s(0x1e2022), st));
    let lh = c.line_height(font);
    c.text_clip(font, r.x + 5.0, (r.cy() - lh / 2.0).round(), r.w - 22.0, if st.hover { t.tx(0xffffff) } else { t.text }, text);
    triangle(c, r.right() - 10.0, r.cy(), 4.0, Dir::Right, if st.hover { t.glyph } else { t.s(0x686c6e) });
}

/// Number box (step counts, track numbers): rounded, light text, spin mark.
pub fn value_box(c: &mut Canvas, t: &Palette, r: Rect, text: &str, font: Font, st: State) {
    if lemon::on(t) {
        return lemon::value_box(c, t, r, text, font, st);
    }
    bordered(c, r, METRICS.radius, t.s(0x191d1f), face(t.s(0x25282a), st));
    let lh = c.line_height(font);
    c.text_right(font, r.right() - 9.0, (r.cy() - lh / 2.0).round(), if st.hover { t.glyph } else { t.text }, text);
    spin_mark(c, r.right() - 4.5, r.cy(), t.text_dim);
}

/// A pill-shaped segmented choice (PAT / SONG, picker tabs): active segment filled with the pill colour.
pub fn pill(c: &mut Canvas, t: &Palette, r: Rect, label: &str, font: Font, st: State) {
    if lemon::on(t) {
        return lemon::pill(c, t, r, label, font, st);
    }
    if st.on {
        c.fill_rrect(r, r.h / 2.0, t.pill);
        c.text_center(font, r, t.pill_text, label);
    } else {
        if st.hover {
            c.fill_rrect(r, r.h / 2.0, t.button_hi);
        }
        c.text_center(font, r, if st.hover { t.glyph } else { t.text_dim }, label);
    }
}

// ---------------------------------------------------------------- toggles and indicators

/// Round LED (enable switches). `radius` 5 (rack) or 6 (mixer, playlist). Uses the Mute role (green).
pub fn led(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, radius: f32, st: State) {
    if lemon::on(t) {
        return lemon::led(c, t, cx, cy, radius, st);
    }
    let k = radius / 6.0;
    c.fill_circle(cx, cy, radius, if st.hover { t.s(0x6f7173) } else { t.s(0x595b5c) });
    c.fill_circle(cx, cy, 5.0 * k, t.s(0x283028));
    if st.on {
        c.fill_circle(cx, cy, 4.2 * k, t.led(0x3d7803));
        c.fill_circle(cx, cy, 3.4 * k, t.led(0xa0dd48));
        c.fill_circle(cx - 0.6 * k, cy - 0.8 * k, 1.8 * k, t.led(0xe2f5b8));
    } else {
        c.fill_circle(cx, cy, 4.0 * k, t.s(0x1f2a14));
    }
}

/// Square check mark / tick (menus).
pub fn tick(c: &mut Canvas, r: Rect, col: Color) {
    c.polyline(&[(r.x + 3.0, r.cy()), (r.x + r.w * 0.42, r.bottom() - 3.0), (r.right() - 3.0, r.y + 3.0)], 1.8, col);
}

/// Cross (close glyph drawn without an icon).
pub fn cross(c: &mut Canvas, r: Rect, col: Color) {
    c.line(r.x + 4.0, r.y + 4.0, r.right() - 4.0, r.bottom() - 4.0, 1.5, col);
    c.line(r.right() - 4.0, r.y + 4.0, r.x + 4.0, r.bottom() - 4.0, 1.5, col);
}

/// Selection frame (accent outline).
pub fn focus_frame(c: &mut Canvas, t: &Palette, r: Rect, width: f32) {
    if lemon::on(t) {
        return lemon::focus_frame(c, t, r, width);
    }
    c.fill_rect(Rect::new(r.x, r.y, r.w, width), t.accent);
    c.fill_rect(Rect::new(r.x, r.bottom() - width, r.w, width), t.accent);
    c.fill_rect(Rect::new(r.x, r.y, 1.0, r.h), t.accent);
    c.fill_rect(Rect::new(r.right() - 1.0, r.y, 1.0, r.h), t.accent);
}

// ---------------------------------------------------------------- knobs and faders

/// Knob looks. All share the same value mapping: 270° sweep from bottom-left, `bipolar` arcs from the top.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnobLook {
    /// Toolbar ring knob: dark disc, track ring, value arc and pointer in glyph colour.
    Ring,
    /// Dark glossy ball with a pointer (rack rows, channel windows, swing).
    Ball,
    /// Small row knob (channel rack rows, spec ui/channel-rack.md §3): flat dark ball, light pointer.
    Row,
    /// Mixer knob: halo, gradient ball, optional value arc in the record red.
    Mixer { halo: f32, arc: bool },
}

pub fn knob(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, r: f32, value: f32, bipolar: bool, look: KnobLook, st: State) {
    if lemon::on(t) {
        return lemon::knob(c, t, cx, cy, r, value, bipolar, look, st);
    }
    let v = value.clamp(0.0, 1.0);
    let start = 0.75 * PI;
    let sweep = 1.5 * PI;
    let a = start + sweep * v;
    let (a0, a1) = if bipolar { (start + sweep / 2.0, a) } else { (start, a) };
    let lift = |col: Color| if st.hover { col.shade(0.08) } else { col };
    match look {
        KnobLook::Ring => {
            let (col, track) = (t.glyph, t.button_hi);
            c.fill_circle(cx, cy, r, lift(track.shade(-0.5)));
            c.arc(cx, cy, r - 1.5, start, sweep, 2.0, track);
            if (a1 - a0).abs() > 0.01 {
                c.arc(cx, cy, r - 1.5, a0.min(a1), (a1 - a0).abs(), 2.0, col);
            }
            c.line(cx + (r - 6.0).max(1.0) * a.cos(), cy + (r - 6.0).max(1.0) * a.sin(), cx + (r - 2.5) * a.cos(), cy + (r - 2.5) * a.sin(), 1.6, col);
        }
        KnobLook::Ball => {
            c.fill_circle(cx, cy, r, t.toolbar_line.shade(-0.3));
            c.fill_circle(cx, cy, r - 1.0, lift(t.button_hi.shade(0.02)));
            c.fill_circle(cx, cy + 0.6, r - 2.2, lift(t.sunken_dark.shade(-0.1)));
            c.fill_circle(cx - r * 0.25, cy - r * 0.35, r * 0.35, Color::rgb(0xffffff).with_alpha(14));
            let pc = if st.hover { t.glyph } else { t.glyph.shade(-0.15) };
            c.line(cx + r * 0.3 * a.cos(), cy + r * 0.3 * a.sin(), cx + (r - 2.5) * a.cos(), cy + (r - 2.5) * a.sin(), (r * 0.2).clamp(1.4, 2.0), pc);
        }
        KnobLook::Row => {
            c.fill_circle(cx, cy, r, t.s(0x1e2022));
            c.fill_circle(cx, cy - 0.5, r - 1.0, lift(t.s(0x303234)));
            c.fill_circle(cx, cy + 0.3, r - 1.7, lift(t.s(0x232527)));
            let pc = if st.hover { t.tx(0xffffff) } else { t.tx(0xadb2b6) };
            c.line(cx + 1.2 * a.cos(), cy + 1.2 * a.sin(), cx + (r - 1.9) * a.cos(), cy + (r - 1.9) * a.sin(), 2.0, pc);
        }
        KnobLook::Mixer { halo, arc } => {
            c.fill_circle(cx, cy, halo, t.s(0x1b1d20));
            c.stroke_circle(cx, cy, halo - 0.5, 1.0, t.s(0x16181a));
            c.fill_circle(cx, cy, r + 0.6, t.s(0x16181b));
            c.fill_rect_gradient(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), lift(t.s(0x3c3e41)), t.s(0x17191c), r);
            c.arc(cx, cy, r - 0.6, 1.15 * PI, 0.7 * PI, 1.2, t.s(0x55575a));
            if arc && (a1 - a0).abs() > 0.02 {
                c.arc(cx, cy, halo - 2.0, a0.min(a1), (a1 - a0).abs(), (halo * 0.22).max(2.0), t.h(0xce2900));
            }
        }
    }
}

/// Fader handle centred at (cx, cy): grey cap, accent green when selected.
pub fn fader_handle(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, selected: bool, st: State) {
    if lemon::on(t) {
        return lemon::fader_handle(c, t, cx, cy, selected, st);
    }
    let h = Rect::new(cx - 7.0, cy - 11.0, 14.0, 23.0);
    c.fill_rrect(h.outset(1.0), 3.0, t.s(0x121416));
    let (top, mid, low) = if selected {
        (t.a(0xc8f56e), t.a(0x8fd236), t.a(0x3f7a10))
    } else if st.hover || st.pressed {
        (t.s(0x7a7e81), t.s(0x55595c), t.s(0x2a2d30))
    } else {
        (t.s(0x6a6e71), t.s(0x46494c), t.s(0x222528))
    };
    c.fill_rect_gradient(Rect::new(h.x, h.y, h.w, 10.0), top, mid, 2.5);
    c.fill_rect_gradient(Rect::new(h.x, h.y + 10.0, h.w, 13.0), mid.shade(-0.25), low, 2.5);
    c.fill_rect(Rect::new(h.x, h.y + 8.0, h.w, 4.0), mid.shade(-0.1));
    c.fill_rect(Rect::new(h.x + 2.0, h.y + 16.0, h.w - 4.0, 1.0), low.shade(-0.4));
}

// ---------------------------------------------------------------- scrolling

/// Scroll thumb geometry: (offset along the track, thumb length).
pub fn thumb(track_len: f32, pos: f32, view: f32, total: f32) -> (f32, f32) {
    let total = total.max(view).max(1.0);
    let max = (total - view).max(0.0);
    let len = (view / total * track_len).clamp(METRICS.thumb_min.min(track_len), track_len.max(0.0));
    let off = if max > 0.0 { pos.clamp(0.0, max) / max * (track_len - len) } else { 0.0 };
    (off, len)
}

/// High visibility (the program's General ▸ Display setting; spec: ui/settings-choices.md "Display: PPI ruler, Force
/// refreshes, Thick lines, High visibility"): every scroll bar draws a light thumb and light arrows. The app sets it
/// from its setting before it paints; separately painted windows (plugin editors) follow.
static HIGH_VISIBILITY: AtomicBool = AtomicBool::new(false);

/// Turns high visibility on or off (`HIGH_VISIBILITY`).
pub fn set_high_visibility(on: bool) {
    HIGH_VISIBILITY.store(on, Ordering::Relaxed);
}

/// Whether high visibility is on (`HIGH_VISIBILITY`).
pub fn high_visibility() -> bool {
    HIGH_VISIBILITY.load(Ordering::Relaxed)
}

/// A scroll bar's thumb colour: the look's own, or with `high` (high visibility) a light grey, #b9bfc3 as measured
/// (in the Lemon look the same grey through the look's colour map, a neutral light grey); it lifts on hover / drag.
pub fn scroll_thumb(t: &Palette, st: State, high: bool) -> Color {
    if high {
        return t.s(0xb9bfc3).shade(if st.pressed { 0.18 } else if st.hover { 0.1 } else { 0.0 });
    }
    if lemon::on(t) {
        return lemon::scroll_thumb(t, st);
    }
    if st.pressed { t.overview.shade(0.18) } else if st.hover { t.overview.shade(0.12) } else { t.overview }
}

/// A scroll bar's arrow colour with `high` (high visibility): light, #adb3b7 as measured (the look's own light grey
/// in the Lemon look), the bright glyph colour on hover. `None` without it: the look's own arrows.
pub fn scroll_arrow(t: &Palette, st: State, high: bool) -> Option<Color> {
    high.then(|| if st.hover || st.pressed { t.glyph } else { t.s(0xadb3b7) })
}

/// Scroll bar: flat track, rounded thumb that lifts on hover / drag (`scroll_thumb`).
pub fn scrollbar(c: &mut Canvas, t: &Palette, track: Rect, vertical: bool, pos: f32, view: f32, total: f32, st: State) {
    if lemon::on(t) {
        return lemon::scrollbar(c, t, track, vertical, pos, view, total, st);
    }
    c.fill_rect(track, t.scrollbar);
    let len = if vertical { track.h } else { track.w };
    let (off, tl) = thumb(len, pos, view, total);
    let r = if vertical {
        Rect::new(track.x + 2.0, track.y + off, track.w - 4.0, tl)
    } else {
        Rect::new(track.x + off, track.y + 2.0, tl, track.h - 4.0)
    };
    c.fill_rrect(r, 2.0_f32.min(r.w / 2.0).min(r.h / 2.0), scroll_thumb(t, st, high_visibility()));
}

// ---------------------------------------------------------------- windows and popups

/// Title bar of a docked / framed window: gradient bar, separator, title text. Returns the client top y.
pub fn title_bar(c: &mut Canvas, t: &Palette, r: Rect, title: &str, title_x: f32, flat: bool) -> f32 {
    let h = METRICS.title_h;
    let bar = Rect::new(r.x, r.y, r.w, h);
    if !flat {
        c.fill_rect_gradient(bar, t.window_title.shade(0.04), t.window_title.shade(-0.03), 0.0);
        c.fill_rect(Rect::new(r.x, r.y + h - 1.0, r.w, 1.0), t.splitter_dark);
    }
    c.text(Font::new(17), r.x + title_x, r.y + 1.0, t.text, title);
    r.y + h
}

/// Floating window frame: 1 px border, 5 px frame, inner line around the client. Returns the inner rect.
pub fn float_frame(c: &mut Canvas, t: &Palette, r: Rect) -> Rect {
    if lemon::on(t) {
        lemon::window(c, t, r);
        let inner = Rect::new(r.x + 7.0, r.y + 5.0, r.w - 14.0, r.h - 12.0);
        lemon::client(c, t, Rect::new(inner.x - 1.0, inner.y + 22.0, inner.w + 2.0, inner.h - 21.0));
        return inner;
    }
    c.fill_rect(r, t.window_border);
    c.fill_rect(r.inset(1.0), t.window_title);
    c.fill_rect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, 1.0), t.window_title_hi.shade(0.04));
    let inner = Rect::new(r.x + 7.0, r.y + 5.0, r.w - 14.0, r.h - 12.0);
    c.fill_rect(Rect::new(inner.x - 1.0, inner.y + 22.0, inner.w + 2.0, inner.h - 21.0), t.s(0x131718));
    inner
}

/// Dropdown / popup panel body with a soft shadow.
pub fn popup(c: &mut Canvas, t: &Palette, r: Rect) {
    if lemon::on(t) {
        return lemon::popup(c, t, r);
    }
    c.fill_rrect(r.outset(3.0), 6.0, Color::rgb(0).with_alpha(40));
    c.fill_rrect(r.outset(1.5), 4.0, Color::rgb(0).with_alpha(70));
    c.fill_rect(r, t.menu_pop_border);
    c.fill_rect(r.inset(2.0), t.menu_pop);
}

/// Hovered row of a list or menu.
pub fn row_highlight(c: &mut Canvas, t: &Palette, r: Rect) {
    if lemon::on(t) {
        return lemon::row_highlight(c, t, r);
    }
    c.fill_rect(r, t.menu_pop_hi);
}

/// Tooltip bubble: dark, rounded, name line and optional caption line. `at` = top-left; kept inside `bounds`.
pub fn tooltip(c: &mut Canvas, t: &Palette, at: (f32, f32), name: &str, caption: &str, bounds: Rect) -> Rect {
    let (nf, cf) = (Font::new(17), Font::new(14));
    let w = (c.text_width(nf, name).max(if caption.is_empty() { 0.0 } else { c.text_width(cf, caption) }) + 18.0).round();
    let h = if caption.is_empty() { 26.0 } else { 46.0 };
    let x = at.0.min(bounds.right() - w - 4.0).max(bounds.x + 4.0);
    let y = if at.1 + h > bounds.bottom() { at.1 - h - 24.0 } else { at.1 };
    let b = Rect::new(x, y, w, h);
    c.fill_rrect(b.outset(1.0), 7.0, Color::rgb(0).with_alpha(90));
    c.fill_rrect(b, 6.0, t.s(0x090909).with_alpha(245));
    c.text(nf, b.x + 9.0, b.y + 3.0, t.tx(0xffffff), name);
    if !caption.is_empty() {
        c.text(cf, b.x + 9.0, b.y + 24.0, t.text_dim, caption);
    }
    b
}

/// Minimize / maximize / close glyphs (drawn with Tabler icons) in a row from `x0`; `hovered` = index under the mouse.
pub fn window_buttons(c: &mut Canvas, t: &Palette, x0: f32, y: f32, hovered: Option<usize>) -> [Rect; 3] {
    use crate::tabler as tb;
    let mut out = [Rect::default(); 3];
    for (i, icon) in [&tb::MINUS, &tb::SQUARES, &tb::X].into_iter().enumerate() {
        let b = Rect::new(x0 + i as f32 * 19.5, y + 3.0, 17.0, 17.0);
        let hov = hovered == Some(i);
        if hov && lemon::on(t) {
            lemon::window_button_hover(c, t, b, i == 2);
        } else if hov {
            c.fill_rrect(b, 3.0, if i == 2 { t.record.with_alpha(170) } else { t.button_hi });
        }
        c.icon(icon, b.inset(2.0), if hov { t.tx(0xffffff) } else { t.text_dim.mix(t.glyph, 0.6) });
        out[i] = b;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_geometry() {
        assert_eq!(thumb(100.0, 0.0, 50.0, 100.0), (0.0, 50.0));
        assert_eq!(thumb(100.0, 50.0, 50.0, 100.0), (50.0, 50.0));
        let (_, l) = thumb(100.0, 0.0, 1.0, 10_000.0);
        assert_eq!(l, METRICS.thumb_min);
        assert_eq!(thumb(100.0, 10.0, 200.0, 100.0), (0.0, 100.0));
    }

    /// High visibility (spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High
    /// visibility"): the thumb is light grey (#b9bfc3) instead of almost the background, the arrows light (#adb3b7);
    /// the Lemon look uses its own light grey. Off, both looks keep their own.
    #[test]
    fn high_visibility_scroll_bar_colours() {
        let t = Palette::default();
        assert_eq!(scroll_thumb(&t, State::NORMAL, false), t.overview);
        assert_eq!(scroll_thumb(&t, State::NORMAL, true), Color::rgb(0xb9bfc3));
        assert_eq!(scroll_arrow(&t, State::NORMAL, false), None);
        assert_eq!(scroll_arrow(&t, State::NORMAL, true), Some(Color::rgb(0xadb3b7)));
        assert_eq!(scroll_arrow(&t, State::hover(true), true), Some(t.glyph));
        let hover = scroll_thumb(&t, State::hover(true), true);
        assert!(hover.r > 0xb9 && hover.g > 0xbf, "lifts on hover: {hover:?}");

        let l = os_theme::design::palette(os_theme::Design::Lemon, None);
        assert_eq!(scroll_thumb(&l, State::NORMAL, false), l.overview);
        let light = scroll_thumb(&l, State::NORMAL, true);
        assert_eq!((light.r, light.g), (light.g, light.b), "a neutral grey: {light:?}");
        assert!(light.r > 0xb0, "light: {light:?}");
        let arrow = scroll_arrow(&l, State::NORMAL, true).unwrap();
        assert!(arrow.r > 0xa0 && arrow.r == arrow.b, "{arrow:?}");
        assert!(!high_visibility(), "off unless the app turns it on");
    }

    #[test]
    fn widgets_paint_without_panics() {
        let mut c = Canvas::new(200, 120, 1.0).unwrap();
        let t = Palette::default();
        for st in [State::NORMAL, State::hover(true), State::new(false, true, false), State::new(false, false, true)] {
            button(&mut c, &t, Rect::new(2.0, 2.0, 30.0, 33.0), st);
            led(&mut c, &t, 50.0, 20.0, 6.0, st);
            for look in [KnobLook::Ring, KnobLook::Ball, KnobLook::Row, KnobLook::Mixer { halo: 15.0, arc: true }] {
                knob(&mut c, &t, 80.0, 30.0, 10.0, 0.3, true, look, st);
            }
            combo(&mut c, &t, Rect::new(2.0, 60.0, 100.0, 23.0), "All", Font::new(15), st);
            scrollbar(&mut c, &t, Rect::new(110.0, 2.0, 10.0, 100.0), true, 5.0, 10.0, 40.0, st);
            fader_handle(&mut c, &t, 150.0, 50.0, st.on, st);
            arrow_button(&mut c, &t, Rect::new(130.0, 90.0, 18.0, 18.0), Dir::Up, st);
            scroll_arrow_button(&mut c, &t, Rect::new(150.0, 90.0, 18.0, 18.0), Dir::Down, st);
        }
        tooltip(&mut c, &t, (190.0, 110.0), "Name", "Caption", Rect::new(0.0, 0.0, 200.0, 120.0));
    }
}
