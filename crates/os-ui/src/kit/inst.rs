// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Instrument-window controls (channel / plugin windows: Lemon Sampler and later Lemon plugins): the FL-style dial with
//! a 360° value ring, the big brushed filter knob, round radios, outlined panels with a header tab, the A/B switch,
//! the fine-tune slider and the dark list box. Measured looks: spec `plugins/lemon-sampler.md` §Look. The Lemon
//! design draws every one of them its own way in the same place (spec `ui/lemon-design.md` §11.1). The controls the
//! app's own windows share (the Misc page) keep both looks; those only the plugins draw (big dial, Reset button, A/B
//! switch, graph background) have the Lemon look only, as the plugins have no Classic look.
// spec: plugins/lemon-sampler.md (Look: dial, big dial, radio, panel, A/B switch, slider, list box)

use std::f32::consts::{PI, TAU};

use os_theme::{Color, Palette};

use super::lemon::{self, plug};
use super::{Dir, State};
use crate::{Canvas, Font, Glyph, Rect};

/// Caption font of instrument knobs (bold condensed capitals).
pub const CAPTION: Font = Font::new(8);
/// Label font of instrument windows (radio labels, panel titles, list boxes).
pub const LABEL: Font = Font::new(13);

/// Screen angle (radians, 0 = 3 o'clock, clockwise) of a dial's pointer. Unipolar dials start at the bottom and turn
/// a full circle clockwise; bipolar dials have their centre value at the top, half a turn each way.
pub fn dial_angle(value: f32, bipolar: bool) -> f32 {
    let v = value.clamp(0.0, 1.0);
    if bipolar { -PI / 2.0 + (v - 0.5) * TAU } else { PI / 2.0 + v * TAU }
}

/// Dial: socket ring, dark ball lit from the top, value ring in the socket's channel in `accent` and a pointer dot.
/// `r` = ball radius, `ring` = radius of the value ring (sample-page knobs 13 / 16, envelope knobs 11.5 / 13.5,
/// tension knobs 8 / 10: no dot). Hover lifts the ball 1 px and thickens the ring; disabled draws no ring and a dim dot.
pub fn dial(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, r: f32, ring: f32, value: f32, bipolar: bool, accent: Color, st: State) {
    if lemon::on(t) {
        // A 270° knob as big as the value ring.
        let w = (ring * 0.15).clamp(1.8, 2.6);
        lemon::dial(c, cx, cy, ring + 0.5, w, value, bipolar, accent, st);
        return;
    }
    c.fill_circle(cx, cy, ring + 1.5, t.s(0x191c1f));
    c.fill_circle(cx, cy, ring + 0.6, t.s(0x24272a));
    c.fill_circle(cx, cy, (ring - 1.8).max(r + 0.6), t.s(0x1a1d20));
    let by = if st.hover || st.pressed { cy - 1.0 } else { cy };
    let ball = Rect::new(cx - r, by - r, 2.0 * r, 2.0 * r);
    c.fill_rect_gradient(ball, t.s(0x3f4245), t.s(0x121518), r);
    c.fill_rect_gradient(ball.inset(1.2), t.s(0x2f3235), t.s(0x1a1d20), r - 1.2);
    c.arc(cx, by, r - 0.6, 1.15 * PI, 0.7 * PI, 1.0, t.s(0x505356));
    let v = value.clamp(0.0, 1.0);
    if !st.disabled {
        let (start, sweep) = if bipolar { (-PI / 2.0, (v - 0.5) * TAU) } else { (PI / 2.0, v * TAU) };
        let w = if st.hover || st.pressed { 2.6 } else { 2.0 };
        if sweep.abs() > 0.02 {
            if sweep.abs() >= TAU - 0.01 {
                c.stroke_circle(cx, cy, ring, w, accent);
            } else {
                c.arc(cx, cy, ring, start, sweep, w, accent);
            }
        }
    }
    if r >= 9.0 {
        let a = dial_angle(v, bipolar);
        let d = r - 3.5;
        let dot = if st.disabled { t.s(0x4a4d50) } else { accent.mix(Color::rgb(0x101010).with_alpha(accent.a), 0.3) };
        let (x, y) = (cx + d * a.cos(), by + d * a.sin());
        c.fill_rect(Rect::new((x - 1.0).round(), (y - 1.0).round(), 2.0, 2.0), dot);
        c.fill_rect(Rect::new((x - 1.0).round(), (y - 2.0).round(), 2.0, 1.0), dot.mix(t.s(0x26292c), 0.55));
        c.fill_rect(Rect::new((x - 1.0).round(), (y + 1.0).round(), 2.0, 1.0), dot.mix(t.s(0x26292c), 0.55));
    }
}

/// Clock angle (radians, 0 = 12 o'clock, clockwise) of the big dial's pointer: 7 o'clock to 5 o'clock (300°).
pub fn big_dial_angle(value: f32) -> f32 {
    (210.0 + 300.0 * value.clamp(0.0, 1.0)).to_radians()
}

/// Big filter knob (MOD X / MOD Y): seven ticks 45° apart (none at the bottom) around a large knob turning over 300°
/// with the value arc in `dot`. Hover brightens it.
pub fn big_dial(c: &mut Canvas, cx: f32, cy: f32, value: f32, dot: Color, st: State) {
    let hot = st.hover || st.pressed;
    for k in [0, 1, 2, 3, 5, 6, 7] {
        let a = (k as f32 * 45.0).to_radians();
        let (s, co) = (a.sin(), -a.cos());
        c.line(cx + 26.5 * s, cy + 26.5 * co, cx + 30.5 * s, cy + 30.5 * co, 1.2, plug::TICK);
    }
    let (start, sweep) = (big_dial_angle(0.0) - PI / 2.0, 300f32.to_radians());
    c.arc(cx, cy, 22.5, start, sweep, 3.0, plug::TRACK);
    let v = value.clamp(0.0, 1.0);
    if v > 0.004 {
        c.arc(cx, cy, 22.5, start, sweep * v, 3.0, if hot { dot.mix(Color::rgb(0xffffff), 0.2) } else { dot });
    }
    let (top, bottom) = if hot { (0x5c5c5c, 0x404040) } else { (0x525252, 0x383838) };
    c.fill_rect_gradient(Rect::new(cx - 19.0, cy - 19.0, 38.0, 38.0), Color::rgb(top), Color::rgb(bottom), 19.0);
    let a = big_dial_angle(value);
    let (s, co) = (a.sin(), -a.cos());
    c.line(cx + 6.0 * s, cy + 6.0 * co, cx + 16.0 * s, cy + 16.0 * co, 2.4, Color::rgb(0xe6e6e6));
}

/// Round radio / toggle, 15 px: dark with a grey ring when off, glowing red-orange when on; a disabled option that is
/// on shows a dim brown dot (it still toggles).
pub fn radio(c: &mut Canvas, t: &Palette, cx: f32, cy: f32, on: bool, st: State) {
    if lemon::on(t) {
        lemon::toggle(c, cx, cy, on, st);
        return;
    }
    c.fill_circle(cx, cy, 7.5, if st.hover && !st.disabled { t.s(0x6c6e6e) } else { t.s(0x606262) });
    if on && !st.disabled {
        c.fill_circle(cx, cy, 6.5, t.h(0x522f2f));
        c.fill_circle(cx, cy, 5.6, t.h(0x942b15));
        c.fill_circle(cx, cy, 4.8, t.h(0xee8a64));
        c.fill_circle(cx, cy, 3.6, t.h(0xf19672));
        c.fill_circle(cx - 1.0, cy - 1.4, 1.6, t.h(0xffc4a8).with_alpha(160));
    } else if on {
        c.fill_circle(cx, cy, 6.5, t.s(0x3a2e2c));
        c.fill_circle(cx, cy, 5.2, t.h(0x6b4a40));
        c.fill_circle(cx, cy, 3.6, t.h(0x84574a));
    } else {
        c.fill_circle(cx, cy, 6.5, t.s(0x2e2f2f));
        c.fill_circle(cx, cy, 5.6, t.s(0x131516));
        c.fill_circle(cx, cy, 4.8, t.s(0x36393a));
    }
}

/// Outlined panel body.
pub fn panel(c: &mut Canvas, t: &Palette, r: Rect) {
    if lemon::on(t) {
        lemon::plug_panel(c, r);
        return;
    }
    c.fill_rect(r, t.s(0x1a1d1d));
    c.fill_rect(r.inset(1.0), t.s(0x222525));
}

/// Panel header tab at the panel's top-left: darker fill 20 px tall whose right edge slants in by 9 px; `title` is
/// drawn 4 px in. `w` = width at the top.
pub fn panel_tab(c: &mut Canvas, t: &Palette, panel: Rect, w: f32, title: &str) {
    if lemon::on(t) {
        // No tab: a plain title.
        if !title.is_empty() {
            let f = LABEL.bold();
            let lh = c.line_height(f);
            c.text(f, panel.x + 10.0, (panel.y + 11.5 - lh / 2.0).round(), os_theme::design::lemon::TEXT, title);
        }
        return;
    }
    let (top, bot) = (panel.y + 1.0, panel.y + 20.0);
    let (l, r) = (panel.x + 1.0, panel.x + w);
    c.fill_polygon(&[(l, top), (r, top), (r - 9.0, bot), (l, bot)], t.s(0x1f2222));
    c.line(r, top, r - 9.0, bot, 1.0, t.s(0x1a1d1d));
    c.fill_rect(Rect::new(l, bot, r - 9.0 - l, 1.0), t.s(0x1a1d1d));
    if !title.is_empty() {
        c.text(LABEL, panel.x + 4.0, top + 1.0, t.tx(0xd2d8dc), title);
    }
}

/// Extra space between caption letters (px).
pub const CAPTION_TRACK: f32 = 1.0;

/// Width of a letter-spaced caption.
pub fn caption_width(c: &mut Canvas, s: &str, track: f32) -> f32 {
    let n = s.chars().count();
    s.chars().map(|ch| c.text_width(CAPTION, ch.encode_utf8(&mut [0; 4]))).sum::<f32>() + track * n.saturating_sub(1) as f32
}

/// Letter-spaced caption with its left edge at `x`, top at `y`.
pub fn caption_at(c: &mut Canvas, x: f32, y: f32, col: Color, s: &str, track: f32) {
    let mut x = x;
    for ch in s.chars() {
        let mut b = [0; 4];
        let g = ch.encode_utf8(&mut b);
        c.text(CAPTION, x.round(), y, col, g);
        x += c.text_width(CAPTION, g) + track;
    }
}

/// Knob caption centred on `cx`, top at `y`.
pub fn caption(c: &mut Canvas, t: &Palette, cx: f32, y: f32, s: &str, disabled: bool) {
    caption_spaced(c, t, cx, y, s, disabled, CAPTION_TRACK);
}

/// Caption with its own letter spacing (the envelope page's TENSION labels are spread wider).
pub fn caption_spaced(c: &mut Canvas, t: &Palette, cx: f32, y: f32, s: &str, disabled: bool, track: f32) {
    let w = caption_width(c, s, track);
    let col = if lemon::on(t) {
        if disabled { plug::OFF } else { plug::DIM }
    } else if disabled {
        t.s(0x606364)
    } else {
        t.tx(0xb4bfc7)
    };
    caption_at(c, (cx - w / 2.0).round(), y, col, s, track);
}

/// Label left-aligned at `x`, vertically centred on `cy`; returns its width.
pub fn label(c: &mut Canvas, t: &Palette, x: f32, cy: f32, s: &str, disabled: bool) -> f32 {
    let lh = c.line_height(LABEL);
    let col = if lemon::on(t) {
        if disabled { plug::FAINT } else { os_theme::design::lemon::TEXT }
    } else if disabled {
        t.s(0x606364)
    } else {
        t.tx(0xd2d8dc)
    };
    c.text(LABEL, x, (cy - lh / 2.0).round(), col, s)
}

/// Dark list box with the chosen entry and a ▸ arrow (declicking mode, stretch mode, filter type).
pub fn list_box(c: &mut Canvas, t: &Palette, r: Rect, text: &str, st: State) {
    if lemon::on(t) {
        lemon::field(c, r, st);
        let lh = c.line_height(LABEL);
        let col = if st.disabled { plug::FAINT } else if st.hover { Color::rgb(0xffffff) } else { os_theme::design::lemon::TEXT };
        c.text_clip(LABEL, r.x + 7.0, (r.cy() - lh / 2.0).round(), r.w - 24.0, col, text);
        let ac = if st.hover { os_theme::design::lemon::MAIN } else { plug::DIM };
        super::chevron(c, r.right() - 10.0, r.cy(), 3.5, Dir::Down, ac);
        return;
    }
    c.fill_rect(r, t.s(0x1b1e1e));
    c.fill_rect(r.inset(1.0), if st.hover { t.s(0x34363a) } else { t.s(0x2b2c2d) });
    c.fill_rect(Rect::new(r.x + 1.0, r.y + 2.0, r.w - 2.0, r.h - 3.0), if st.pressed { t.s(0x17181a) } else { t.s(0x1d1e1f) });
    let lh = c.line_height(LABEL);
    let col = if st.disabled { t.s(0x606364) } else if st.hover { t.tx(0xffffff) } else { t.tx(0xd2d8dc) };
    c.text_clip(LABEL, r.x + 5.0, (r.cy() - lh / 2.0).round(), r.w - 20.0, col, text);
    let ac = if st.hover { t.tx(0xc8ccce) } else { t.s(0x686c6e) };
    let (x, y) = (r.right() - 9.0, r.cy());
    let mut g = Glyph::new();
    g.polygon(&[(x - 2.0, y - 4.0), (x + 2.5, y), (x - 2.0, y + 4.0)]);
    c.glyph(ac, &g);
}

/// Small push button with a text label ("Reset"), lit when `st.on`.
pub fn text_button(c: &mut Canvas, r: Rect, s: &str, st: State) {
    let ink = lemon::plug_button(c, r, 6.0, st);
    c.text_center(LABEL, r, ink, s);
}

/// Vertical A / B switch (reverb type): a rounded slot with a lit square on the chosen side, letters to the right.
pub fn ab_switch(c: &mut Canvas, r: Rect, b: bool, st: State) {
    let slot = Rect::new(r.x, r.y, 12.0, r.h);
    lemon::field(c, slot, State { pressed: false, ..st });
    let by = if b { slot.bottom() - 6.5 } else { slot.y + 6.5 };
    lemon::lit(c, Rect::new(slot.x + 1.5, by - 4.5, 9.0, 9.0), 3.0, st.hover);
    let f = Font::new(12).bold();
    let lh = c.line_height(f);
    let (on, off) = (os_theme::design::lemon::TEXT, plug::FAINT);
    c.text(f, slot.right() + 4.0, (slot.y + 6.5 - lh / 2.0).round(), if b { off } else { on }, "A");
    c.text(f, slot.right() + 4.0, (slot.bottom() - 6.5 - lh / 2.0).round(), if b { on } else { off }, "B");
}

/// Horizontal slider with ticks under the track and a round thumb with a red line; returns the track rect the value
/// maps onto (`value` 0..1 = left..right).
pub fn hslider(c: &mut Canvas, t: &Palette, x0: f32, x1: f32, cy: f32, value: f32, st: State) -> Rect {
    if lemon::on(t) {
        // A thin groove, ticks under it and a rounded cap with a lemon line (the faders' look).
        c.fill_rrect(Rect::new(x0 + 2.0, cy - 2.5, x1 - x0 - 4.0, 5.0), 2.5, Color::rgb(0x1a1a1a));
        for i in 0..=8 {
            let x = (x0 + 6.0 + (x1 - x0 - 12.0) * i as f32 / 8.0).round() + 0.5;
            c.line(x, cy + 6.0, x, cy + if i % 4 == 0 { 10.0 } else { 8.5 }, 1.0, plug::TICK);
        }
        let track = Rect::new(x0 + 6.0, cy - 9.0, x1 - x0 - 12.0, 18.0);
        let tx = track.x + track.w * value.clamp(0.0, 1.0);
        let cap = Rect::new(tx - 6.0, cy - 7.0, 12.0, 14.0);
        c.fill_rrect(cap.outset(1.0), 5.0, Color::rgb(0x151515));
        let (top, bot) = if st.hover || st.pressed { (0x7a7a7a, 0x555555) } else { (0x6c6c6c, 0x4b4b4b) };
        c.fill_rect_gradient(cap, Color::rgb(top), Color::rgb(bot), 4.0);
        c.fill_rrect(Rect::new(tx - 0.75, cy - 4.5, 1.5, 9.0), 0.75, os_theme::design::lemon::MAIN);
        return track;
    }
    c.fill_rrect(Rect::new(x0, cy - 2.5, x1 - x0, 5.0), 2.0, t.s(0x161819));
    for i in 0..=8 {
        let x = (x0 + 6.0 + (x1 - x0 - 12.0) * i as f32 / 8.0).round() + 0.5;
        c.line(x, cy + 5.0, x, cy + if i % 4 == 0 { 10.0 } else { 8.0 }, 1.0, t.s(0x3a3d3f));
    }
    let track = Rect::new(x0 + 6.0, cy - 9.0, x1 - x0 - 12.0, 18.0);
    let tx = track.x + track.w * value.clamp(0.0, 1.0);
    c.fill_circle(tx, cy, 8.0, t.s(0x0f1011));
    let (top, bot) = if st.hover || st.pressed { (t.s(0x66696c), t.s(0x24272a)) } else { (t.s(0x55585b), t.s(0x1b1d1f)) };
    c.fill_rect_gradient(Rect::new(tx - 7.0, cy - 7.0, 14.0, 14.0), top, bot, 7.0);
    c.line(tx, cy - 6.0, tx, cy + 6.0, 1.5, t.h(0xe03030));
    track
}

/// Envelope / LFO graph background: a dark well.
pub fn graph_bg(c: &mut Canvas, r: Rect) {
    lemon::plug_well(c, r, 6.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dial_angles() {
        // Unipolar: 0 and 1 at the bottom, 0.25 at 9 o'clock.
        assert!((dial_angle(0.0, false) - PI / 2.0).abs() < 1e-6);
        assert!((dial_angle(0.25, false) - PI).abs() < 1e-6);
        // Bipolar centre at the top.
        assert!((dial_angle(0.5, true) + PI / 2.0).abs() < 1e-6);
        // Big dial: 7 o'clock .. 5 o'clock.
        assert!((big_dial_angle(0.0).to_degrees() - 210.0).abs() < 1e-3);
        assert!((big_dial_angle(1.0).to_degrees() - 510.0).abs() < 1e-3);
    }

    #[test]
    fn paints() {
        let mut c = Canvas::new(200, 120, 1.0).unwrap();
        for t in [Palette::default(), os_theme::design::palette(os_theme::Design::Lemon, None)] {
            for st in [State::NORMAL, State::hover(true), State::NORMAL.with_disabled(true)] {
            dial(&mut c, &t, 20.0, 20.0, 11.5, 13.5, 0.31, false, t.a(0x47f49a), st);
            dial(&mut c, &t, 50.0, 20.0, 8.0, 10.0, 0.1, true, t.a(0x47f49a), st);
            big_dial(&mut c, 100.0, 40.0, 1.0, t.a(0x5ea733), st);
            radio(&mut c, &t, 150.0, 20.0, true, st);
            list_box(&mut c, &t, Rect::new(2.0, 60.0, 100.0, 20.0), "Fast LP", st);
            ab_switch(&mut c, Rect::new(150.0, 40.0, 24.0, 26.0), true, st);
            hslider(&mut c, &t, 10.0, 110.0, 100.0, 0.5, st);
            panel(&mut c, &t, Rect::new(2.0, 2.0, 60.0, 40.0));
            panel_tab(&mut c, &t, Rect::new(2.0, 2.0, 60.0, 40.0), 40.0, "Filter");
            text_button(&mut c, Rect::new(2.0, 90.0, 40.0, 20.0), "Reset", st);
            graph_bg(&mut c, Rect::new(60.0, 60.0, 50.0, 30.0));
            caption(&mut c, &t, 30.0, 40.0, "AMT", st.disabled);
            label(&mut c, &t, 120.0, 100.0, "Tempo", st.disabled);
            }
        }
    }
}
