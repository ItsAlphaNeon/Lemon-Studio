// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon 3x Osc editor: painting and hit regions (platform independent), built from the `os_ui::kit` widgets and
//! Lemon Studio's own drawings (no artwork of the original).
//!
//! Coordinates are FL window pixels from the spec tables; the editor is the window minus its 24 px title bar, so
//! every y is drawn at `y - OY`. Colours are palette roles (`t.s` surface, `t.a` accent, `t.h` highlight, `t.tx`
//! text) so an imported theme recolours the plugin like the rest of Lemon Studio.
// spec: plugins/lemon-3x-osc.md (§1 window, §2 oscillator page, §3 envelope page, §4 hints)

use std::f32::consts::{PI, TAU};

use os_theme::Palette;
use os_ui::kit::{inst, State};
use os_ui::tabler as tb;
use os_ui::{Canvas, Color, Font, Rect};

use crate::params::{AM_BYTE, LFO_SHAPES};

/// Painted size in logical pixels (the original's window minus its title bar; same frame as Lemon Sampler).
pub const W: u32 = 568;
pub const H: u32 = 442;
/// The editor the host shows: the painted area cropped at (`CROP_X`, `CROP_Y`) without the original's 4 px frame.
pub const CROP_X: u32 = 4;
pub const CROP_Y: u32 = 1;
pub const EDITOR_W: u32 = 560;
pub const EDITOR_H: u32 = 437;
/// Hosted mode: the host draws the strip row (47 px), the editor starts below it.
pub const STRIP_ROWS: u32 = 47;
pub const HOSTED_CROP_Y: u32 = CROP_Y + STRIP_ROWS;
pub const HOSTED_EDITOR_H: u32 = EDITOR_H - STRIP_ROWS;
/// Height of the original's title bar (the host draws its own).
pub const OY: f32 = 24.0;

/// Selected shape glyphs, pan and phase-randomness pointers.
pub const ORANGE: u32 = 0xf2a13a;
/// Coarse / fine value markers.
pub const LIME: u32 = 0xb8e04a;
/// Fader handle centre line.
pub const RED: u32 = 0xff4040;

/// Envelope targets: knob ring, sub-tab text, graph line (as Lemon Sampler).
pub const TARGET_COLORS: [(u32, u32, u32); 5] = [
    (0x49e3f6, 0x29b5f9, 0x3ec1d8),
    (0x47f49a, 0x40f08c, 0x3bc97e),
    (0xcfe744, 0xd6fc47, 0xb1c538),
    (0xf29a48, 0xf0792b, 0xce833c),
    (0xf64d70, 0xf13f37, 0xe2415f),
];
pub const TARGET_NAMES: [&str; 5] = ["Panning", "Volume", "Mod X", "Mod Y", "Pitch"];
/// Parameter id prefixes of the five records (state order).
pub const ENV_PREFIX: [&str; 5] = ["pan", "vol", "cut", "res", "pitch"];

const LABEL: Font = inst::LABEL;
const CAP: Font = Font::new(12);
const MOD_CAP: Font = Font::new(10).bold();

pub fn y(v: f32) -> f32 {
    v - OY
}

/// Inclusive FL-window pixel ranges to an editor rect.
pub fn rr(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::span(x0, x1, y(y0), y(y1))
}

/// Centre y of oscillator row `n` (FL px).
pub fn row_y(n: usize) -> f32 {
    120.0 + 108.0 * n as f32
}

/// Shape button centres: (shape or `INVERT`, x, row offset): top row sine, square, rounded saw, custom; bottom row
/// triangle, saw, noise, invert.
pub const INVERT: i32 = -1;
pub const SHAPE_BUTTONS: [(i32, f32, f32); 8] =
    [(0, 60.0, -19.0), (2, 97.0, -19.0), (4, 134.0, -19.0), (6, 173.0, -19.0), (1, 60.0, 18.0), (3, 97.0, 18.0), (5, 134.0, 18.0), (INVERT, 173.0, 18.0)];

/// Read access to parameter values by id.
pub trait Model {
    fn norm(&self, id: &str) -> f32;
    fn plain(&self, id: &str) -> f32;
    fn text(&self, id: &str) -> String;
}

impl Model for os_plugkit::vals::Vals {
    fn norm(&self, id: &str) -> f32 {
        os_plugkit::vals::Vals::norm(self, id)
    }
    fn plain(&self, id: &str) -> f32 {
        os_plugkit::vals::Vals::plain(self, id)
    }
    fn text(&self, id: &str) -> String {
        os_plugkit::vals::Vals::text(self, id)
    }
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Vertical drag / wheel; menu on right click (knobs and faders).
    Knob(String),
    /// A 0 / 1 parameter toggled on click.
    Toggle(String),
    /// Sets an integer parameter to a value on click.
    Set(String, f32),
    /// Flag byte of the switches parameter (0..2 invert osc 1..3, 3 = amplitude modulation).
    Flag(usize),
    /// Choice list: opens the list menu.
    List(String),
    Tab(u8),
    SubTab(u8),
    /// Envelope graph node: 0 delay end, 1 attack end, 2 hold end, 3 decay end (sustain), 4 release end.
    EnvPoint(u8),
}

impl Target {
    /// The parameter a target edits.
    pub fn param(&self) -> Option<&str> {
        match self {
            Target::Knob(s) | Target::Toggle(s) | Target::Set(s, _) | Target::List(s) => Some(s),
            Target::Flag(_) => Some("flags"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// Editor-only state (not saved).
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    /// 0 oscillators, 1 envelopes.
    pub tab: u8,
    /// Envelope target: 0 Panning, 1 Volume, 2 Mod X, 3 Mod Y, 4 Pitch.
    pub sub: u8,
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState { tab: 0, sub: 1, mouse: None, pressed: None }
    }
}

pub struct Ui<'a> {
    pub c: &'a mut Canvas,
    pub t: &'a Palette,
    pub m: &'a dyn Model,
    pub st: &'a ViewState,
    pub hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }
    fn on(&self, id: &str) -> bool {
        self.m.plain(id) > 0.5
    }
    fn flag(&self, b: usize) -> bool {
        self.m.plain("flags").round() as i32 & (1 << b) != 0
    }
    /// Hover / pressed look of a control occupying `r`.
    fn state(&self, r: Rect, t: &Target) -> State {
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed, ..State::NORMAL }
    }
}

/// Paints the editor. `hosted` = the host's strip row carries the page tabs and the hint line (host link); otherwise
/// the plugin draws its own two tabs and `hint` in the strip row.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, hint: &str, hosted: bool) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(t.s(0x25292b));
    ui.c.fill_rect(Rect::span(4.0, 563.0, 1.0, y(461.0)), t.s(0x202324));
    for r in [Rect::span(0.0, 0.0, 0.0, y(465.0)), Rect::span(567.0, 567.0, 0.0, y(465.0)), Rect::span(0.0, 567.0, y(465.0), y(465.0))] {
        ui.c.fill_rect(r, t.s(0x191d1f));
    }
    if !hosted {
        tab_strip(&mut ui);
        if !hint.is_empty() {
            let lh = ui.c.line_height(LABEL);
            let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
            ui.c.text_clip(LABEL, 100.0, (y(48.0) - lh / 2.0).round(), 300.0, t.tx(0xb4bfc7), left);
            if !right.is_empty() {
                ui.c.text_right(LABEL, 556.0, (y(48.0) - lh / 2.0).round(), t.tx(0xb4bfc7), right);
            }
        }
    }
    match st.tab {
        0 => osc_page(&mut ui),
        _ => envelope_page(&mut ui),
    }
    ui.hits
}

fn icon(c: &mut Canvas, t: &tb::TablerIcon, cx: f32, cy: f32, size: f32, col: Color) {
    c.icon(t, Rect::new(cx - size / 2.0, cy - size / 2.0, size, size), col);
}

/// The page icons the host draws in its strip (same ids as `os_hostlink::PageIcon`: plugin, envelope).
pub fn page_icon(c: &mut Canvas, page: u8, cx: f32, cy: f32, col: Color) {
    match page {
        0 => icon(c, &tb::PLUG, cx, cy, 26.0, col),
        _ => icon(c, &tb::TIMELINE, cx, cy, 26.0, col),
    }
}

fn tab_strip(ui: &mut Ui) {
    let t = ui.t;
    ui.c.fill_rect(rr(4.0, 563.0, 25.0, 71.0), t.s(0x151718));
    ui.c.fill_rect(rr(4.0, 563.0, 26.0, 70.0), t.s(0x1b1d1e));
    for i in 0..2u8 {
        let x0 = 5.0 + 43.0 * f32::from(i);
        let cell = rr(x0, x0 + 44.0, 26.0, 71.0);
        let active = ui.st.tab == i;
        let st = ui.state(cell, &Target::Tab(i));
        if active {
            ui.c.fill_rect(cell, t.s(0x202324));
        }
        let col = if active {
            t.a(0x49f1b1)
        } else if st.hover {
            t.tx(0xffffff)
        } else {
            t.tx(0xd5dbdf)
        };
        page_icon(ui.c, i, 27.0 + 43.0 * f32::from(i), y(48.0), col);
        ui.hit(cell, Target::Tab(i));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Oscillator page (spec §2)

/// Clock angle (radians from 12 o'clock, clockwise) of a knob value: 7 o'clock .. 5 o'clock (300°).
pub fn knob_angle(v: f32) -> f32 {
    (v.clamp(0.0, 1.0) - 0.5) * (300.0f32).to_radians()
}

fn polar(cx: f32, cy: f32, a: f32, d: f32) -> (f32, f32) {
    (cx + d * a.sin(), cy - d * a.cos())
}

/// A brushed-metal knob in a dark socket: `ticks` marks around it, a coloured marker on the rim at the value.
#[allow(clippy::too_many_arguments)]
fn metal_knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, r: f32, ticks: usize, accent: u32, value_text: bool) {
    let t = ui.t;
    let cy = y(cy);
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - r - 7.0, cy - r - 7.0, 2.0 * r + 14.0, 2.0 * r + 14.0);
    let st = ui.state(hr, &target);
    let v = ui.m.norm(id);
    // Tick marks around the socket.
    let ring = r + 7.0;
    for i in 0..ticks {
        let a = knob_angle(i as f32 / (ticks - 1).max(1) as f32);
        let (x0, y0) = polar(cx, cy, a, ring - 1.5);
        let (x1, y1) = polar(cx, cy, a, ring + 1.5);
        ui.c.line(x0, y0, x1, y1, 1.0, t.s(0x3e484d));
    }
    // Socket, drop shadow and the metal face (light rim on top, darker centre for a dished look).
    ui.c.fill_circle(cx, cy + 1.5, r + 4.0, t.s(0x000000).with_alpha(60));
    ui.c.fill_circle(cx, cy, r + 3.5, t.s(0x1e2427));
    ui.c.fill_circle(cx, cy, r + 2.2, t.s(0x2c3438));
    let lift = if st.hover || st.pressed { -0.8 } else { 0.0 };
    let face = Rect::new(cx - r, cy - r + lift, 2.0 * r, 2.0 * r);
    // Dark brushed face: a light rim lit from above around a dished dark centre.
    ui.c.fill_rect_gradient(face, t.s(0xc4cace), t.s(0x343a3e), r);
    ui.c.fill_rect_gradient(face.inset(r * 0.14), t.s(0x353c40), t.s(0x575f64), r * 0.86);
    ui.c.stroke_circle(cx, cy + lift, r * 0.86, 0.8, t.s(0x1c2124).with_alpha(160));
    // Value marker on the rim.
    let a = knob_angle(v);
    let (x0, y0) = polar(cx, cy, a, r + 2.5);
    let (x1, y1) = polar(cx, cy, a, r + 7.5);
    ui.c.line(x0, y0, x1, y1, 2.4, t.a(accent));
    if value_text {
        let s = format!("{}", ui.m.plain(id).round() as i32);
        let f = Font::new(13);
        let lh = ui.c.line_height(f);
        let w = ui.c.text_width(f, &s);
        ui.c.text(f, (cx - w / 2.0).round(), (cy + lift - lh / 2.0).round(), t.tx(0xdfe4e7), &s);
    }
    ui.hit(hr, target);
}

/// A vertical fader: dark slot with ticks, handle with a red centre line at the value (centre = 0).
fn fader(ui: &mut Ui, id: &str, x: f32, cy: f32) {
    let t = ui.t;
    let (top, bot) = (y(cy - 40.0), y(cy + 40.0));
    let target = Target::Knob(id.into());
    let hr = Rect::new(x - 13.0, top - 6.0, 26.0, bot - top + 12.0);
    let st = ui.state(hr, &target);
    for k in 0..9 {
        let yy = top + (bot - top) * k as f32 / 8.0;
        let w = if k % 4 == 0 { 6.0 } else { 4.0 };
        ui.c.fill_rect(Rect::new(x - 8.0 - w, yy.round(), w, 1.0), t.s(0x4d585e));
        ui.c.fill_rect(Rect::new(x + 9.0, yy.round(), w, 1.0), t.s(0x4d585e));
    }
    ui.c.fill_rrect(Rect::new(x - 3.0, top - 2.0, 6.0, bot - top + 4.0), 2.0, t.s(0x1f2629));
    ui.c.fill_rect(Rect::new(x - 1.0, top, 2.0, bot - top), t.s(0x101415));
    let v = ui.m.norm(id);
    let hy = (bot - (bot - top) * v).round();
    let h = Rect::new(x - 12.0, hy - 6.0, 24.0, 12.0);
    ui.c.fill_rrect(h.outset(1.0), 2.0, t.s(0x000000).with_alpha(90));
    let (a, b) = if st.hover || st.pressed { (0x4a5257, 0x262c2f) } else { (0x3d4448, 0x1e2326) };
    ui.c.fill_rect_gradient(h, t.s(a), t.s(b), 2.0);
    ui.c.fill_rect(Rect::new(h.x + 2.0, hy - 0.5, h.w - 4.0, 1.5), t.a(RED));
    ui.hit(hr, target);
}

/// The bow-tie mark used as the stereo captions (Lemon Studio drawing).
fn bowtie(c: &mut Canvas, cx: f32, cy: f32, col: Color) {
    c.fill_polygon(&[(cx - 5.0, cy - 4.0), (cx, cy), (cx - 5.0, cy + 4.0)], col);
    c.fill_polygon(&[(cx + 5.0, cy - 4.0), (cx, cy), (cx + 5.0, cy + 4.0)], col);
}

/// Shape glyph `shape` (0..6) centred on (`cx`, `cy`), about 24 x 18 px.
fn shape_glyph(c: &mut Canvas, shape: i32, cx: f32, cy: f32, col: Color) {
    let (w, h) = (11.0, 8.0);
    let pts: Vec<(f32, f32)> = match shape {
        0 => (0..=32)
            .map(|i| {
                let u = i as f32 / 32.0;
                (cx - w + 2.0 * w * u, cy - h * (TAU * u).sin())
            })
            .collect(),
        1 => vec![(cx - w, cy + h * 0.2), (cx - w * 0.5, cy - h), (cx + w * 0.5, cy + h), (cx + w, cy - h * 0.2)],
        2 => vec![(cx - w, cy + h), (cx - w, cy - h), (cx, cy - h), (cx, cy + h), (cx + w, cy + h), (cx + w, cy - h)],
        3 => vec![(cx - w, cy + h), (cx - w * 0.15, cy - h), (cx - w * 0.15, cy + h), (cx + w * 0.75, cy - h), (cx + w * 0.75, cy + h * 0.2)],
        4 => {
            let mut p = vec![(cx - w, cy + h)];
            for i in 0..=12 {
                let u = i as f32 / 12.0;
                p.push((cx - w + 1.2 * w * u, cy + h - 2.0 * h * (1.0 - (-u * 5.0).exp()) / (1.0 - (-5.0f32).exp())));
            }
            p.extend([(cx + 0.3 * w, cy - h), (cx + 0.3 * w, cy + h), (cx + w, cy + h), (cx + w, cy - h * 0.2)]);
            p
        }
        5 => {
            // A die: noise.
            let r = Rect::new(cx - 9.0, cy - 9.0, 18.0, 18.0);
            c.fill_rrect(r, 3.5, col);
            for (dx, dy) in [(-4.0, -4.0), (4.0, 4.0), (0.0, 0.0)] {
                c.fill_circle(cx + dx, cy + dy, 1.7, Color::rgb(0xffffff).with_alpha(150));
            }
            return;
        }
        _ => {
            // Custom: a jagged burst.
            let ys = [0.0, -0.3, 0.4, -0.9, 1.0, -1.0, 0.8, -0.5, 0.3, 0.0];
            let n = ys.len() - 1;
            let pts: Vec<(f32, f32)> = ys.iter().enumerate().map(|(i, v)| (cx - w + 2.0 * w * i as f32 / n as f32, cy - h * v)).collect();
            let mut poly = pts.clone();
            poly.extend(pts.iter().rev().map(|&(x, yy)| (x, 2.0 * cy - yy)));
            c.fill_polygon(&poly, col);
            return;
        }
    };
    c.polyline(&pts, 2.2, col);
}

/// Invert switch: two stacked triangles.
fn invert_glyph(c: &mut Canvas, cx: f32, cy: f32, col: Color) {
    c.fill_polygon(&[(cx - 8.0, cy - 2.0), (cx, cy - 9.0), (cx + 8.0, cy - 2.0)], col);
    c.fill_polygon(&[(cx - 8.0, cy + 2.0), (cx, cy + 9.0), (cx + 8.0, cy + 2.0)], col);
}

fn osc_page(ui: &mut Ui) {
    let t = ui.t;
    // Background: a soft radial grey lightest at the top middle (measured colours), then the dark bottom band.
    let area = rr(4.0, 563.0, 72.0, 385.0);
    ui.c.push_clip(area);
    ui.c.fill_radial(Rect::new(40.0, y(72.0) - 346.0, 520.0, 692.0), t.s(0x78878e), t.s(0x515b61));
    ui.c.pop_clip();
    for (x, col) in [(4.0, 0x2c3337), (5.0, 0x3d474c), (562.0, 0x3d474c), (563.0, 0x2c3337)] {
        ui.c.fill_rect(rr(x, x, 72.0, 385.0), t.s(col));
    }
    let band = rr(4.0, 563.0, 386.0, 461.0);
    ui.c.push_clip(band);
    ui.c.fill_radial(Rect::new(4.0 - 100.0, y(386.0) - 40.0, 760.0, 230.0), t.s(0x2a363b), t.s(0x212a2e));
    ui.c.pop_clip();
    ui.c.fill_rect(rr(4.0, 563.0, 386.0, 386.0), t.s(0x1a2023));

    let ink = t.tx(0x2d3438);
    for n in 0..3 {
        let o = n + 1;
        let cy = row_y(n);
        let id = |s: &str| format!("o{o}_{s}");
        // Number tab.
        let (top, bot) = (y(cy - 27.0), y(cy + 28.0));
        ui.c.fill_polygon(&[(8.0, top), (30.0, top), (40.0, (top + bot) / 2.0), (30.0, bot), (8.0, bot)], t.s(0x2f3a40));
        let f = Font::light(34, 0).italic();
        let s = o.to_string();
        let lh = ui.c.line_height(f);
        let w = ui.c.text_width(f, &s);
        ui.c.text(f, (24.0 - w / 2.0).round(), ((top + bot) / 2.0 - lh / 2.0).round(), t.tx(0xc4ccd0), &s);
        // Shapes and invert.
        let shape = ui.m.plain(&id("shape")).round() as i32;
        for (s, x, dy) in SHAPE_BUTTONS {
            let cyy = y(cy + dy);
            let target = if s == INVERT { Target::Flag(n) } else { Target::Set(id("shape"), s as f32) };
            let hr = Rect::new(x - 17.0, cyy - 17.0, 34.0, 34.0);
            let st = ui.state(hr, &target);
            if s == INVERT {
                let on = ui.flag(n);
                let col = if on { t.a(ORANGE) } else if st.hover { t.tx(0xe4e8ea) } else { t.tx(0xc9ced1) };
                invert_glyph(ui.c, x, cyy + 2.0, col);
            } else {
                let sel = shape == s;
                if sel {
                    ui.c.fill_circle(x, cyy, 19.0, t.s(0x2a2f32));
                    ui.c.fill_circle(x, cyy, 18.0, t.s(0x3a4044));
                }
                let col = if sel {
                    t.a(ORANGE)
                } else if st.hover {
                    t.s(0x4a5459)
                } else {
                    t.s(0x2d3236)
                };
                shape_glyph(ui.c, s, x, cyy, col);
            }
            ui.hit(hr, target);
        }
        // Stereo faders, pitch, pan, mix.
        fader(ui, &id("phase"), 236.0, cy);
        fader(ui, &id("detune"), 266.0, cy);
        metal_knob(ui, &id("coarse"), 342.0, cy, 22.0, 25, LIME, true);
        metal_knob(ui, &id("fine"), 405.0, cy, 16.0, 5, LIME, false);
        metal_knob(ui, &id("pan"), 467.0, cy, 16.0, 5, ORANGE, false);
        if n > 0 {
            metal_knob(ui, &format!("o{o}_mix"), 527.0, cy, 21.0, 5, LIME, false);
        }
        // Captions under rows 1 and 2 (the bottom band covers row 3's, as in the original).
        if n == 2 {
            continue;
        }
        let cap_y = y(cy + 53.0);
        let lh = ui.c.line_height(CAP);
        let wl = ui.c.text_width(CAP, "PHASE OFS");
        ui.c.text(CAP, (243.0 - wl).round(), (cap_y - lh / 2.0).round(), ink, "PHASE OFS");
        bowtie(ui.c, 251.0, cap_y, ink);
        ui.c.text(CAP, 259.0, (cap_y - lh / 2.0).round(), ink, "DETUNE");
        let w = ui.c.text_width(CAP, "COARSE");
        ui.c.text(CAP, (342.0 - w / 2.0).round(), (cap_y - lh / 2.0).round(), ink, "COARSE");
        ui.c.text(CAP, 393.0, (cap_y - lh / 2.0).round(), ink, "FINE");
        // Keyboard mark between COARSE and FINE (decorative).
        icon(ui.c, &tb::PIANO, 377.0, y(cy + 55.0), 14.0, ink);
        bowtie(ui.c, 467.0, cap_y, ink);
        if n > 0 {
            // Ramp mark: the mix level.
            ui.c.fill_polygon(&[(521.0, cap_y + 4.0), (533.0, cap_y + 4.0), (533.0, cap_y - 4.0)], ink);
        }
    }

    // Bottom band: AM, HQ, phase randomness, wordmark.
    let am = ui.flag(AM_BYTE);
    let r = Rect::new(14.0, y(423.0) - 8.0, 70.0, 16.0);
    let st = ui.state(r, &Target::Flag(AM_BYTE));
    inst::radio(ui.c, t, 22.0, y(423.0), am, st);
    inst::label(ui.c, t, 32.0, y(423.0), "AM OSC 3", false);
    ui.hit(r, Target::Flag(AM_BYTE));
    let r = Rect::new(93.0, y(423.0) - 8.0, 40.0, 16.0);
    let st = ui.state(r, &Target::Toggle("hq".into()));
    inst::radio(ui.c, t, 101.0, y(423.0), ui.on("hq"), st);
    inst::label(ui.c, t, 111.0, y(423.0), "HQ", false);
    ui.hit(r, Target::Toggle("hq".into()));
    dark_knob(ui, "rand", 168.0, 423.0, 17.0);
    bowtie(ui.c, 206.0, y(423.0), t.tx(0xb4bfc7));
    inst::label(ui.c, t, 214.0, y(423.0), "PHASE RAND", false);
    let f = Font::light(36, 0);
    let lh = ui.c.line_height(f);
    ui.c.text_right(f, 548.0, (y(414.0) - lh / 2.0).round(), t.tx(0x6d7a80), "Lemon 3x Osc");
}

/// The phase-randomness knob: dark, ticks around it, an orange pointer turning over 300°.
fn dark_knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, r: f32) {
    let t = ui.t;
    let cy = y(cy);
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - r - 6.0, cy - r - 6.0, 2.0 * r + 12.0, 2.0 * r + 12.0);
    let st = ui.state(hr, &target);
    for i in 0..5 {
        let a = knob_angle(i as f32 / 4.0);
        let (x0, y0) = polar(cx, cy, a, r + 4.0);
        let (x1, y1) = polar(cx, cy, a, r + 8.0);
        ui.c.line(x0, y0, x1, y1, 1.0, t.s(0x56636a));
    }
    ui.c.fill_circle(cx, cy, r + 3.0, t.s(0x141a1d));
    let lift = if st.hover || st.pressed { -0.8 } else { 0.0 };
    ui.c.fill_rect_gradient(Rect::new(cx - r, cy - r + lift, 2.0 * r, 2.0 * r), t.s(0x4b555a), t.s(0x15191b), r);
    ui.c.fill_rect_gradient(Rect::new(cx - r + 2.0, cy - r + 2.0 + lift, 2.0 * r - 4.0, 2.0 * r - 4.0), t.s(0x2f373b), t.s(0x1e2427), r - 2.0);
    let a = knob_angle(ui.m.norm(id));
    let (x0, y0) = polar(cx, cy + lift, a, r * 0.45);
    let (x1, y1) = polar(cx, cy + lift, a, r - 2.5);
    ui.c.line(x0, y0, x1, y1, 2.6, t.a(ORANGE));
    ui.hit(hr, target);
}

// ---------------------------------------------------------------------------------------------------------------
// Envelope page (spec §3: Lemon Sampler's envelope tab without the keyboard)

fn panel(ui: &mut Ui, x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    let r = rr(x0, x1, y0, y1);
    inst::panel(ui.c, ui.t, r);
    r
}

#[derive(Clone, Copy)]
struct KnobLook {
    r: f32,
    ring: f32,
}

const BIG: KnobLook = KnobLook { r: 13.0, ring: 16.0 };
const MID: KnobLook = KnobLook { r: 11.5, ring: 13.5 };
const TINY: KnobLook = KnobLook { r: 8.0, ring: 10.0 };

fn dial(ui: &mut Ui, id: &str, cx: f32, cy: f32, look: KnobLook, accent: u32, bipolar: bool, caption: &str) {
    let cy = y(cy);
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - look.ring - 2.0, cy - look.ring - 2.0, 2.0 * look.ring + 4.0, 2.0 * look.ring + 4.0);
    let st = ui.state(hr, &target);
    inst::dial(ui.c, ui.t, cx, cy, look.r, look.ring, ui.m.norm(id), bipolar, ui.t.a(accent), st);
    if !caption.is_empty() {
        let dy = if id.ends_with("_amt") && !id.ends_with("lfo_amt") { 21.5 } else { 17.5 };
        inst::caption(ui.c, ui.t, cx, cy + dy, caption, false);
    }
    ui.hit(hr, target);
}

fn radio(ui: &mut Ui, id: &str, cx: f32, cy: f32) {
    let on = ui.on(id);
    let cy = y(cy);
    let r = Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0);
    let target = Target::Toggle(id.into());
    let st = ui.state(r, &target);
    inst::radio(ui.c, ui.t, cx, cy, on, st);
    ui.hit(r, target);
}

fn label(ui: &mut Ui, x: f32, cy: f32, s: &str) -> f32 {
    inst::label(ui.c, ui.t, x, y(cy), s, false)
}

fn envelope_page(ui: &mut Ui) {
    let t = ui.t;
    ui.c.fill_rect(rr(4.0, 563.0, 72.0, 461.0), t.s(0x202324));
    let sub = ui.st.sub.min(4) as usize;
    const SPANS: [(f32, f32); 5] = [(16.0, 55.0), (65.0, 103.0), (113.0, 145.0), (155.0, 187.0), (197.0, 220.0)];
    for (i, n) in TARGET_NAMES.iter().enumerate() {
        let w = ui.c.text_width(LABEL, n);
        let (x0, x1) = SPANS[i];
        let x = ((x0 + x1 + 1.0 - w) / 2.0).round();
        let cell = rr(x0 - 5.0, x1 + 4.0, 78.0, 106.0);
        let target = Target::SubTab(i as u8);
        let st = ui.state(cell, &target);
        let active = sub == i;
        if active {
            ui.c.fill_rect(cell, t.s(0x1b1d1e));
        }
        let col = if active {
            t.a(TARGET_COLORS[i].1)
        } else if st.hover {
            t.tx(0xffffff)
        } else {
            t.tx(0xd2d8dc)
        };
        let lh = ui.c.line_height(LABEL);
        ui.c.text(LABEL, x, (y(92.0) - lh / 2.0).round(), col, n);
        ui.hit(cell, target);
    }
    let p = ENV_PREFIX[sub];
    let id = |s: &str| format!("{p}_{s}");
    let (arc, _, line) = TARGET_COLORS[sub];

    // Envelope (the Panning target has none).
    if sub != 0 {
        let pr = panel(ui, 11.0, 297.0, 113.0, 310.0);
        inst::panel_tab(ui.c, t, pr, 84.0, "");
        radio(ui, &id("env"), 21.0, 123.0);
        let lw = label(ui, 32.0, 123.0, "Envelope");
        if let Some(h) = ui.hits.last_mut() {
            h.r.w += lw + 4.0;
        }
        let g = env_rect();
        inst::graph_bg(ui.c, t, g);
        env_graph(ui, g, p, t.a(line));
        for (k, x, cap) in [("delay", 33.0, "DELAY"), ("att", 67.0, "ATT"), ("hold", 101.0, "HOLD"), ("dec", 135.0, "DEC"), ("sus", 169.0, "SUS"), ("rel", 203.0, "REL")] {
            dial(ui, &id(k), x, 243.0, MID, arc, false, cap);
        }
        if sub >= 2 {
            dial(ui, &id("amt"), 272.0, 243.0, BIG, arc, true, "AMT");
        }
        for (k, x) in [("att_t", 67.0), ("dec_t", 135.0), ("rel_t", 203.0)] {
            dial(ui, &id(k), x, 283.0, TINY, arc, true, "");
        }
        for x in [101.0, 169.0] {
            inst::caption_spaced(ui.c, t, x, y(283.0) - 4.0, "TENSION", false, 2.3);
        }
    }

    // LFO.
    let lr = panel(ui, 304.0, 450.0, 113.0, 310.0);
    inst::panel_tab(ui.c, t, lr, 105.0, "");
    let shape = ui.m.plain(&id("lfo_shape")).round() as usize;
    for (i, x) in [316.0, 341.0, 362.0].into_iter().enumerate() {
        let r = Rect::new(x - 10.0, y(114.0), 21.0, 19.0);
        let target = Target::Set(id("lfo_shape"), i as f32);
        let st = ui.state(r, &target);
        if i == shape {
            ui.c.fill_rect(r, t.s(0x1a1c1d));
        }
        let col = if i == shape {
            t.a(arc)
        } else if st.hover {
            t.tx(0xffffff)
        } else {
            t.tx(0xd5dbdf)
        };
        lfo_icon(ui.c, i, x, y(123.0), col);
        ui.hit(r, target);
    }
    label(ui, 377.0, 123.0, "LFO");
    let lg = rr(311.0, 442.0, 140.0, 222.0);
    inst::graph_bg(ui.c, t, lg);
    lfo_graph(ui, lg, p, t.a(line));
    for (k, x, cap) in [("lfo_delay", 326.0, "DELAY"), ("lfo_att", 359.0, "ATT"), ("lfo_amt", 393.0, "AMT"), ("lfo_speed", 427.0, "SPEED")] {
        dial(ui, &id(k), x, 241.0, MID, arc, k == "lfo_amt", cap);
    }

    // Filter.
    let fr = panel(ui, 457.0, 556.0, 113.0, 310.0);
    inst::panel_tab(ui.c, t, fr, 55.0, "Filter");
    ui.c.text(MOD_CAP, 504.0, y(131.0) - 6.0, t.tx(0xb4bfc7), "MOD X");
    big_knob(ui, "cutoff", 492.5, 168.5, 0x5ea733);
    big_knob(ui, "resonance", 520.0, 233.5, 0xc8a032);
    ui.c.text(MOD_CAP, 464.0, y(264.0) - 7.0, t.tx(0xb4bfc7), "MOD Y");
    let r = rr(463.0, 549.0, 282.0, 302.0);
    let target = Target::List("filter".into());
    let st = ui.state(r, &target);
    let s = ui.m.text("filter");
    inst::list_box(ui.c, t, r, &s, st);
    ui.hit(r, target);
}

fn lfo_icon(c: &mut Canvas, shape: usize, cx: f32, cy: f32, col: Color) {
    let pts: Vec<(f32, f32)> = match shape {
        0 => (0..=24)
            .map(|i| {
                let u = i as f32 / 24.0;
                (cx - 6.0 + 12.0 * u, cy - 4.6 * (TAU * u).sin())
            })
            .collect(),
        1 => vec![(cx - 8.0, cy), (cx - 5.0, cy - 4.6), (cx + 1.0, cy + 4.6), (cx + 4.0, cy)],
        _ => vec![(cx - 4.5, cy + 4.0), (cx - 4.5, cy - 3.5), (cx, cy - 3.5), (cx, cy + 3.5), (cx + 4.5, cy + 3.5), (cx + 4.5, cy - 4.0)],
    };
    c.polyline(&pts, 1.4, col);
}

fn big_knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, dot: u32) {
    let cy = y(cy);
    let r = Rect::new(cx - 26.0, cy - 26.0, 52.0, 52.0);
    let target = Target::Knob(id.into());
    let st = ui.state(r, &target);
    inst::big_dial(ui.c, ui.t, cx, cy, ui.m.norm(id), ui.t.a(dot), st);
    ui.hit(r, target);
}

/// Envelope graph geometry: x of each stage end, the sustain level, the drawn level scale and the pixels per knob.
pub struct EnvGeom {
    pub x: [f32; 6],
    pub sus: f32,
    pub amp: f32,
    pub px: f32,
}

/// Pixels per full knob in the envelope graph (as Lemon Sampler).
pub const ENV_PX: f32 = 90.0;

pub fn env_rect() -> Rect {
    rr(18.0, 290.0, 140.0, 222.0)
}

pub fn env_geom(m: &dyn Model, p: &str, g: Rect) -> EnvGeom {
    let n = |s: &str| m.norm(&format!("{p}_{s}"));
    let (dl, at, ho, de, su, re) = (n("delay"), n("att"), n("hold"), n("dec"), n("sus"), n("rel"));
    let units = [dl, at, ho, de * (1.0 - su), re];
    let total: f32 = units.iter().sum();
    let x0 = g.x + 1.0;
    let room = g.w - 4.0;
    let px = if total * ENV_PX > room && total > 0.0 { room / total } else { ENV_PX };
    let mut x = [x0; 6];
    for i in 0..5 {
        x[i + 1] = x[i] + units[i] * px;
    }
    let has_amt = !matches!(p, "pan" | "vol");
    let amp = if has_amt { ((n("amt") - 0.5) * 2.0).abs() } else { 1.0 };
    EnvGeom { x, sus: su, amp, px }
}

pub fn env_levels(g: Rect) -> (f32, f32) {
    (g.y + 1.8, g.bottom() - 2.2)
}

fn env_graph(ui: &mut Ui, g: Rect, p: &str, col: Color) {
    let tension = |s: &str| ui.m.plain(&format!("{p}_{s}")).round() as i32;
    let eg = env_geom(ui.m, p, g);
    let [x0, xd, xa, xh, xs, xe] = eg.x;
    let (top, bot) = env_levels(g);
    let amp = eg.amp;
    let lvl = |l: f32| bot - (bot - top) * l * amp;
    let curve = |a: f32, b: f32, l0: f32, l1: f32, kk: f64, pts: &mut Vec<(f32, f32)>| {
        let steps = ((b - a) / 2.0).ceil().max(1.0) as usize;
        for i in 1..=steps {
            let u = i as f64 / steps as f64;
            let sh = os_engine::envelope::shape(u, kk) as f32;
            pts.push((a + (b - a) * u as f32, lvl(l0 + (l1 - l0) * sh)));
        }
    };
    let tk = os_engine::envelope::tension_k;
    let su = eg.sus;
    let mut pts = vec![(x0, lvl(0.0)), (xd, lvl(0.0))];
    curve(xd, xa, 0.0, 1.0, tk(tension("att_t")), &mut pts);
    pts.push((xh, lvl(1.0)));
    curve(xh, xs, 1.0, su, -tk(tension("dec_t")), &mut pts);
    curve(xs, xe, su, 0.0, -tk(tension("rel_t")), &mut pts);
    ui.c.push_clip(g);
    ui.c.polyline(&pts, 1.8, col);
    for (i, (x, yy)) in [(xd, lvl(0.0)), (xa, lvl(1.0)), (xh, lvl(1.0)), (xs, lvl(su)), (xe, lvl(0.0))].into_iter().enumerate() {
        let target = Target::EnvPoint(i as u8);
        let hr = Rect::new(x - 5.0, yy - 5.0, 10.0, 10.0);
        let st = ui.state(hr, &target);
        let r = if st.hover || st.pressed { 4.6 } else { 4.0 };
        ui.c.fill_circle(x, yy, r, col);
        ui.c.fill_circle(x, yy, r - 1.8, col.mix(ui.t.s(0x1b1d1e), 0.8));
        ui.hits.push(Hit { r: hr, t: target });
    }
    ui.c.pop_clip();
}

fn lfo_graph(ui: &mut Ui, g: Rect, p: &str, col: Color) {
    let n = |s: &str| ui.m.norm(&format!("{p}_{s}"));
    let shape = ui.m.plain(&format!("{p}_lfo_shape")).round() as usize;
    let amt = (n("lfo_amt") - 0.5) * 2.0;
    let (dl, at, sp) = (n("lfo_delay"), n("lfo_att"), n("lfo_speed"));
    let cy = g.y + ((g.h - 1.0) / 2.0).floor() + 0.5;
    let a = amt * (g.h / 2.0 - 3.0);
    let cycles = 2f32.powf(sp * 4.0);
    let xd = dl * g.w * 0.35;
    let xa = at * g.w * 0.35;
    let pts: Vec<(f32, f32)> = (0..=g.w as usize)
        .map(|i| {
            let x = i as f32;
            let env = if x < xd {
                0.0
            } else if xa > 0.0 {
                ((x - xd) / xa).min(1.0)
            } else {
                1.0
            };
            let ph = (x / g.w * cycles).fract();
            let w = match shape {
                0 => (2.0 * PI * ph).sin(),
                1 => 1.0 - 4.0 * ((ph + 0.25).fract() - 0.5).abs(),
                _ => {
                    if ph < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };
            (g.x + x, cy - a * w * env)
        })
        .collect();
    ui.c.push_clip(g);
    ui.c.fill_rect(Rect::new(g.x, cy - 0.5, g.w, 1.0), ui.t.s(0x232628));
    ui.c.polyline(&pts, 1.5, col);
    ui.c.pop_clip();
}

// ---------------------------------------------------------------------------------------------------------------
// Hints (spec §4)

/// The hint line for a target: the parameter's name and value as the original words them.
pub fn hint(t: &Target, m: &dyn Model, names: &dyn Fn(&str) -> String) -> String {
    match t {
        Target::Knob(id) | Target::Toggle(id) | Target::List(id) => {
            if id == "hq" {
                return "High quality (anti aliased oscillators)".into();
            }
            format!("{}: {}", names(id), m.text(id))
        }
        // The original names the button only ("Osc 1 shape").
        Target::Set(id, _) if id.ends_with("_shape") && id.starts_with('o') => format!("Osc {} shape", &id[1..2]),
        Target::Set(id, v) => format!("{}: {}", names(id), LFO_SHAPES.get(*v as usize).copied().unwrap_or("")),
        Target::Flag(AM_BYTE) => "Oscillator 3 = amplitude modulation".into(),
        Target::Flag(n) => format!("Invert osc {}", n + 1),
        Target::Tab(0) => "Oscillators".into(),
        Target::Tab(_) => "Envelopes, LFOs and filter".into(),
        Target::SubTab(i) => format!("{} envelope / LFO", TARGET_NAMES[usize::from(*i).min(4)]),
        Target::EnvPoint(_) => String::new(),
    }
}
