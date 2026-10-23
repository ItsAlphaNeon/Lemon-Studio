// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Citrus editor: painting and hit regions (platform independent). Sytrus's layout at its Small size, every control
//! where FL has it, unstyled: flat placeholder colours, no artwork (the Lemon design comes later). The page tabs,
//! the header buttons, the main page, the operator, filter and FX pages' controls with their articulators' tabs and
//! bars, the matrix and the testing keyboard. The curve view shows its curve (editing it comes in stage 3).
//!
// spec: plugins/sytrus-editor.md (§1 .. §7), plugins/citrus.md §6

use os_ui::{Canvas, Color, Font, Rect};

use crate::ctl::{self, Field};
use crate::state::*;

/// Editor size in logical pixels, with the testing keyboard and without it.
pub const W: u32 = 850;
pub const H: u32 = 450;
pub const H_NO_KEYS: u32 = 348;

/// Placeholder colours (unstyled).
pub const BG: Color = Color::rgb(0x2a2c2e);
pub const PANEL: Color = Color::rgb(0x383b3e);
const WELL: Color = Color::rgb(0x1e2022);
const EDGE: Color = Color::rgb(0x4c5054);
pub const TEXT: Color = Color::rgb(0xd8d8d8);
const DIM: Color = Color::rgb(0x8a8d90);
const FAINT: Color = Color::rgb(0x5a5e62);
pub const ACCENT: Color = Color::rgb(0x6aa8e0);
/// The matrix's accent while the ring-modulation matrix is shown (FL draws it yellow).
pub const RING: Color = Color::rgb(0xe0c040);

const LABEL: Font = Font::new(10);
const SMALL: Font = Font::new(9);

/// The pages: MAIN, OP 1 .. OP 6, FILTER 1 .. 3, FX.
pub const PAGES: usize = 11;
pub const PAGE_FX: usize = 10;
const TAB_NAMES: [&str; PAGES] = ["MAIN", "OP 1", "OP 2", "OP 3", "OP 4", "OP 5", "OP 6", "FILTER 1", "FILTER 2", "FILTER 3", "FX"];

/// The page area and the matrix panel.
pub const PAGE: Rect = Rect::new(0.0, 26.0, 546.0, 320.0);
pub const MATRIX: Rect = Rect::new(547.0, 26.0, 299.0, 320.0);
pub const KEYS: Rect = Rect::new(4.0, 355.0, 842.0, 95.0);
/// The testing keyboard's keys: C3 .. B7 (FL's names, key 60 = C5).
pub const FIRST_KEY: u8 = 36;
pub const LAST_KEY: u8 = 95;
/// The XY pad.
pub const PAD: Rect = Rect::span(14.0, 142.0, 166.0, 293.0);
pub const PATCH_TEXT: Rect = Rect::new(10.0, 302.0, 341.0, 40.0);

/// What covers the whole surface instead of the pages (header buttons 2 and 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cover {
    #[default]
    None,
    Info,
    Diagram,
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Tab(usize),
    /// Options, Show info, Show block diagram, Show / hide testing keyboard.
    Header(usize),
    /// A vertical fader: vertical drag (relative), wheel, menu on right click.
    Slider(Field),
    /// A knob: the same.
    Knob(Field),
    /// A box: 8 px of drag or a wheel notch per step.
    Box(Field),
    /// A lamp switch: a click toggles it.
    Switch(Field),
    /// One position of a selector: a click picks it.
    Choice(Field, i32),
    /// An equaliser type button: a click steps the type.
    Cycle(Field),
    /// The X / Y pad: the dot follows the cursor.
    Pad,
    /// A matrix cell (of the matrix shown): drag and wheel; right click turns it off and on.
    Cell(Field),
    /// A matrix row label (selects the module's page) and a column label (operators, FX).
    Row(usize),
    Col(usize),
    /// The FM / RM tabs.
    MatrixTab(bool),
    Key(u8),
    PatchText,
    Logo,
    /// An articulator page's target tab and source tab.
    ArtTab(usize),
    PartTab(usize),
    /// The FX page's effect unit: delays 1 .. 3, the reverb.
    FxUnit(usize),
    /// A digit of a numeric display: a vertical drag moves the setting by `step` per 10 px.
    Digit(Field, i32),
    /// An operator's waveform: its menu on right click.
    Wave(usize),
    /// A control whose setting was not identified: it shows its hint and does nothing.
    Inert(&'static str),
    /// The info page or the block diagram: a click returns.
    Cover,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// What the editor shows besides the state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
    pub page: usize,
    /// The matrix shows the ring-modulation cells.
    pub ring: bool,
    /// The testing keyboard is hidden.
    pub no_keys: bool,
    pub cover: Cover,
    /// The key the testing keyboard holds.
    pub key: Option<u8>,
    /// Per page: the articulator page's target tab and source tab.
    pub target: [usize; PAGES],
    pub part: [usize; PAGES],
    /// The FX page's unit (delays 0 .. 2, 3 the reverb).
    pub fx_unit: usize,
}

/// What a paint shows of the patch.
pub struct Model<'a> {
    pub st: &'a State,
    /// Modules in use (the tabs' marker bars).
    pub used: [bool; 9],
    pub version: &'a str,
    /// The operator page's wave, one cycle.
    pub wave: Option<&'a [f32]>,
}

struct Ui<'a> {
    c: &'a mut Canvas,
    m: &'a Model<'a>,
    st: &'a ViewState,
    hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }
    fn hot(&self, r: Rect, t: &Target) -> bool {
        self.st.pressed.as_ref() == Some(t) || (self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y)))
    }
    fn get(&self, f: Field) -> i32 {
        ctl::get(self.m.st, f)
    }
    fn norm(&self, f: Field) -> f32 {
        let (lo, hi) = ctl::range(f);
        (self.get(f) - lo) as f32 / (hi - lo) as f32
    }
    /// Text centred on a point.
    fn label(&mut self, f: Font, cx: f32, cy: f32, col: Color, s: &str) {
        self.c.text_center(f, Rect::new(cx - 60.0, cy - 8.0, 120.0, 16.0), col, s);
    }
}

/// The surface's height.
pub fn height(no_keys: bool) -> u32 {
    if no_keys { H_NO_KEYS } else { H }
}

/// Paints the editor. `hint` (not hosted: the host shows it otherwise) goes over the patch text's first line.
pub fn paint(c: &mut Canvas, m: &Model, st: &ViewState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(BG);
    match st.cover {
        Cover::Info => info(&mut ui),
        Cover::Diagram => diagram(&mut ui),
        Cover::None => {
            tabs(&mut ui);
            header(&mut ui);
            if st.page == 0 {
                main_page(&mut ui, hint);
            } else {
                module_page(&mut ui);
            }
            matrix(&mut ui);
            if !st.no_keys {
                keyboard(&mut ui);
            }
        }
    }
    ui.hits
}

// ---------------------------------------------------------------------------------------------------------------
// Widgets

fn panel(ui: &mut Ui, r: Rect) {
    ui.c.fill_rect(r, PANEL);
    ui.c.stroke_rrect(r.inset(0.5), 0.0, 1.0, EDGE);
}

/// A vertical fader centred on `x`, its track from `top` to `bot`, the label above it.
fn slider(ui: &mut Ui, f: Field, x: f32, top: f32, bot: f32, label: &str) {
    let t = Target::Slider(f);
    let hr = Rect::span(x - 10.0, x + 10.0, top - 8.0, bot + 8.0);
    let hot = ui.hot(hr, &t);
    ui.c.fill_rect(Rect::span(x - 1.0, x + 1.0, top, bot), WELL);
    let (lo, hi) = ctl::range(f);
    let y_of = |n: f32| bot - (bot - top) * n.clamp(0.0, 1.0);
    let y = y_of(ui.norm(f));
    let zero = if lo < 0 { y_of(-lo as f32 / (hi - lo) as f32) } else { bot };
    ui.c.fill_rect(Rect::span(x - 1.0, x + 1.0, y.min(zero), y.max(zero)), ACCENT);
    let handle = Rect::new(x - 9.0, (y - 6.0).round(), 18.0, 12.0);
    ui.c.fill_rect(handle, if hot { EDGE.shade(0.15) } else { EDGE });
    ui.c.fill_rect(Rect::new(x - 7.0, y.round(), 14.0, 1.0), TEXT);
    if !label.is_empty() {
        ui.label(SMALL, x, top - 9.0, DIM, label);
    }
    ui.hit(hr, t);
}

/// A knob of radius `r`: a ring, the value's arc in `accent` (from the top for bipolar ranges) and a pointer.
fn dial(ui: &mut Ui, cx: f32, cy: f32, r: f32, norm: f32, bipolar: bool, accent: Color, hot: bool) {
    use std::f32::consts::PI;
    let start = 0.75 * PI;
    let sweep = 1.5 * PI;
    ui.c.fill_circle(cx, cy, r, if hot { WELL.shade(0.1) } else { WELL });
    ui.c.arc(cx, cy, r - 1.5, start, sweep, 2.0, FAINT);
    let a = start + sweep * norm.clamp(0.0, 1.0);
    let from = if bipolar { 1.5 * PI } else { start };
    if (a - from).abs() > 0.01 {
        ui.c.arc(cx, cy, r - 1.5, from.min(a), (a - from).abs(), 2.0, accent);
    }
    ui.c.line(cx + 0.35 * r * a.cos(), cy + 0.35 * r * a.sin(), cx + (r - 3.0) * a.cos(), cy + (r - 3.0) * a.sin(), 1.5, TEXT);
}

fn knob(ui: &mut Ui, f: Field, cx: f32, cy: f32, r: f32, label: &str) {
    let t = Target::Knob(f);
    let hr = Rect::new(cx - r - 2.0, cy - r - 2.0, 2.0 * r + 4.0, 2.0 * r + 4.0);
    let hot = ui.hot(hr, &t);
    let bipolar = ctl::range(f).0 < 0;
    let n = ui.norm(f);
    dial(ui, cx, cy, r, n, bipolar, ACCENT, hot);
    if !label.is_empty() {
        ui.label(SMALL, cx, cy + r + 7.0, DIM, label);
    }
    ui.hit(hr, t);
}

/// A box showing its value's text.
fn value_box(ui: &mut Ui, f: Field, r: Rect, text: &str) {
    let t = Target::Box(f);
    let hot = ui.hot(r, &t);
    ui.c.fill_rect(r, WELL);
    ui.c.stroke_rrect(r.inset(0.5), 0.0, 1.0, if hot { DIM } else { EDGE });
    ui.c.text_center(LABEL, r, TEXT, text);
    ui.hit(r, t);
}

/// A lamp switch at (cx, cy), its label to the right (or under it).
fn lamp(ui: &mut Ui, t: Target, on: bool, cx: f32, cy: f32, label: &str, under: bool) {
    let w = ui.c.text_width(SMALL, label);
    let hr = if under { Rect::new(cx - 10.0, cy - 7.0, 20.0, 22.0) } else { Rect::new(cx - 7.0, cy - 7.0, w + 18.0, 14.0) };
    let hot = ui.hot(hr, &t);
    ui.c.fill_circle(cx, cy, 4.5, if on { ACCENT } else { WELL });
    ui.c.stroke_circle(cx, cy, 4.5, 1.0, if hot { TEXT } else { EDGE });
    if under {
        ui.label(SMALL, cx, cy + 10.0, DIM, label);
    } else {
        let lh = ui.c.line_height(SMALL);
        ui.c.text(SMALL, cx + 8.0, (cy - lh / 2.0).round(), DIM, label);
    }
    ui.hit(hr, t);
}

fn switch(ui: &mut Ui, f: Field, cx: f32, cy: f32, label: &str) {
    let on = ui.get(f) != 0;
    lamp(ui, Target::Switch(f), on, cx, cy, label, false);
}

/// A caption at a panel's foot.
fn caption(ui: &mut Ui, r: Rect, s: &str) {
    ui.label(SMALL, r.cx(), r.bottom() - 9.0, DIM, s);
}

// ---------------------------------------------------------------------------------------------------------------
// Tabs and header (§1, §7)

/// The page a module's row selects: operators 1..6, filters 7..9.
pub fn page_of_row(r: usize) -> usize {
    r + 1
}

fn tabs(ui: &mut Ui) {
    let fx_used = (0..9).any(|r| ui.m.used[r] && ui.m.st.cell(r, COL_SEND) != 0);
    for (i, name) in TAB_NAMES.iter().enumerate() {
        let r = Rect::new(6.0 + 68.0 * i as f32, 4.0, 66.0, 22.0);
        let t = Target::Tab(i);
        let hot = ui.hot(r, &t);
        let sel = ui.st.page == i;
        ui.c.fill_rect(r, if sel { ACCENT } else if hot { EDGE } else { PANEL });
        ui.c.text_center(LABEL, r, if sel { BG } else { TEXT }, name);
        let used = match i {
            1..=9 => ui.m.used[i - 1],
            PAGE_FX => fx_used,
            _ => false,
        };
        if used {
            ui.c.fill_rect(Rect::new(r.right() - 6.0, r.y + 5.0, 2.0, 12.0), if sel { BG } else { ACCENT });
        }
        ui.hit(r, t);
    }
}

pub const HEADER_X: [f32; 4] = [771.0, 793.0, 815.0, 836.0];

fn header(ui: &mut Ui) {
    for (i, x) in HEADER_X.into_iter().enumerate() {
        let (y, rad) = (14.0, 9.0);
        let t = Target::Header(i);
        let r = Rect::new(x - rad, y - rad, 2.0 * rad, 2.0 * rad);
        let hot = ui.hot(r, &t);
        let on = i == 3 && !ui.st.no_keys;
        ui.c.fill_circle(x, y, rad, if on { ACCENT } else if hot { EDGE } else { PANEL });
        let ink = if on { BG } else { TEXT };
        match i {
            // Options: three lines.
            0 => {
                for k in 0..3 {
                    ui.c.fill_rect(Rect::new(x - 4.0, y - 4.0 + 3.0 * k as f32, 8.0, 1.5), ink);
                }
            }
            1 => ui.label(LABEL, x, y, ink, "i"),
            // The block diagram: two boxes joined.
            2 => {
                ui.c.stroke_rrect(Rect::new(x - 5.0, y - 4.0, 4.0, 4.0), 0.0, 1.0, ink);
                ui.c.stroke_rrect(Rect::new(x + 1.0, y, 4.0, 4.0), 0.0, 1.0, ink);
                ui.c.line(x - 1.0, y - 2.0, x + 3.0, y, 1.0, ink);
            }
            // The keyboard: three keys.
            _ => {
                for k in 0..3 {
                    ui.c.fill_rect(Rect::new(x - 4.5 + 3.5 * k as f32, y - 4.0, 2.0, 8.0), ink);
                }
            }
        }
        ui.hit(r, t);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Main page (§2)

fn main_page(ui: &mut Ui, hint: &str) {
    // Master.
    let r = Rect::new(10.0, 35.0, 130.0, 98.0);
    panel(ui, r);
    for (k, x, l) in [(0, 33.0, "VOL"), (1, 76.0, "LFO"), (2, 118.0, "PITCH")] {
        slider(ui, Field::Main(k), x, 58.0, 118.0, l);
    }
    // Envelope scales.
    for (base, rect, xs, cap) in [
        (0, Rect::new(146.0, 35.0, 99.0, 98.0), [158.0, 184.0, 208.0, 235.0], "VOL \u{2303}"),
        (4, Rect::new(251.0, 35.0, 98.0, 98.0), [262.0, 289.0, 313.0, 340.0], "FILTER \u{2303}"),
    ] {
        panel(ui, rect);
        for (j, l) in ["A", "D", "S", "R"].into_iter().enumerate() {
            slider(ui, Field::S(base + j), xs[j], 56.0, 108.0, l);
        }
        caption(ui, rect, cap);
    }
    // Unison.
    let r = Rect::new(356.0, 35.0, 182.0, 98.0);
    panel(ui, r);
    ui.label(SMALL, 375.0, 43.0, DIM, "ORD");
    // A stored order 0 plays as 1.
    let ord = ui.get(Field::S(8)).max(1).to_string();
    value_box(ui, Field::S(8), Rect::new(362.5, 52.5, 25.0, 21.0), &ord);
    let on = ui.get(Field::Switch(0)) != 0;
    lamp(ui, Target::Switch(Field::Switch(0)), on, 375.0, 86.0, "OSC", true);
    for (k, x, l) in [(9, 404.0, "PN"), (10, 430.0, "VL"), (11, 454.0, "PT"), (12, 477.0, "SB"), (13, 500.0, "PH"), (14, 525.0, "EN")] {
        slider(ui, Field::S(k), x, 52.0, 108.0, l);
    }
    caption(ui, r, "UNISON");
    modulation(ui);
    equaliser(ui);
    quality(ui);
    // Switches and SOFTEN.
    panel(ui, Rect::span(356.0, 538.0, 236.0, 298.0));
    switch(ui, Field::S32(1), 367.0, 245.0, "RANDOM");
    lamp(ui, Target::Inert("Limit to 1 voice per key"), false, 367.0, 264.0, "MONO KEY", false);
    switch(ui, Field::S32(2), 367.0, 283.0, "GLOBAL PITCH");
    switch(ui, Field::Switch(1), 448.0, 245.0, "CENTER");
    switch(ui, Field::Switch(2), 448.0, 264.0, "GIBBS OFF");
    switch(ui, Field::Switch(4), 448.0, 283.0, "PORTA");
    knob(ui, Field::Soften, 524.0, 261.0, 10.0, "SOFTEN");
    patch_text(ui, hint);
    // The logo.
    let r = Rect::new(356.0, 298.0, 182.0, 44.0);
    ui.c.text_center(Font::new(20).bold(), r, DIM, "Citrus");
    ui.hit(r, Target::Logo);
}

fn modulation(ui: &mut Ui) {
    panel(ui, Rect::new(10.0, 140.0, 166.0, 159.0));
    ui.label(SMALL, 40.0, 151.0, DIM, "X / Y");
    switch(ui, Field::S32(0), 124.0, 150.0, "SMOOTH");
    ui.c.fill_rect(PAD, WELL);
    ui.c.fill_rect(Rect::span(PAD.cx(), PAD.cx() + 1.0, PAD.y, PAD.bottom()), FAINT);
    ui.c.fill_rect(Rect::span(PAD.x, PAD.right(), PAD.cy(), PAD.cy() + 1.0), FAINT);
    let (x, y) = pad_point(ui.get(Field::S(25)), ui.get(Field::S(26)));
    ui.c.fill_circle(x, y, 4.0, ACCENT);
    ui.hit(PAD, Target::Pad);
    for (k, y, l) in [(25, 192.0, "X"), (26, 222.0, "Y")] {
        knob(ui, Field::S(k), 162.0, y, 10.0, "");
        ui.label(SMALL, 148.0, y, DIM, l);
    }
}

/// The pad's dot for mod X / Y values (y up).
pub fn pad_point(x: i32, y: i32) -> (f32, f32) {
    (PAD.x + PAD.w * x as f32 / 512.0, PAD.bottom() - PAD.h * y as f32 / 512.0)
}

/// Mod X / Y values for a point on the pad.
pub fn pad_values(x: f32, y: f32) -> (i32, i32) {
    let v = |t: f32| (t.clamp(0.0, 1.0) * 512.0).round() as i32;
    (v((x - PAD.x) / PAD.w), v((PAD.bottom() - y) / PAD.h))
}

const EQ_X: [f32; 3] = [266.0, 297.0, 328.0];
pub const EQ_TARGET_Y: [f32; 4] = [173.0, 187.0, 201.0, 215.0];

fn equaliser(ui: &mut Ui) {
    panel(ui, Rect::new(183.0, 140.0, 168.0, 159.0));
    ui.label(SMALL, 205.0, 151.0, DIM, "EQ");
    for (b, x) in EQ_X.into_iter().enumerate() {
        // The type button: an outline of the band's shape.
        let f = Field::Eq(b, 3);
        let r = Rect::new(x - 13.0, 143.0, 26.0, 14.0);
        let t = Target::Cycle(f);
        let hot = ui.hot(r, &t);
        ui.c.fill_rect(r, if hot { EDGE } else { WELL });
        let (l, m, h) = match ui.get(f) {
            0 => (-3.0, 2.0, 2.0),
            1 => (2.0, -3.0, 2.0),
            _ => (2.0, 2.0, -3.0),
        };
        let cy = r.cy();
        ui.c.polyline(&[(x - 9.0, cy + l), (x - 4.0, cy + l), (x, cy + m), (x + 4.0, cy + h), (x + 9.0, cy + h)], 1.2, TEXT);
        ui.hit(r, t);
        slider(ui, Field::Eq(b, 0), x, 176.0, 218.0, "");
        knob(ui, Field::Eq(b, 1), x, 250.0, 10.0, "");
        knob(ui, Field::Eq(b, 2), x, 279.0, 10.0, "");
    }
    ui.label(SMALL, 232.0, 250.0, DIM, "FREQ");
    ui.label(SMALL, 232.0, 279.0, DIM, "BW");
    let mode = ui.get(Field::EqMode);
    for (k, (y, l)) in EQ_TARGET_Y.into_iter().zip(["OFF", "OUT+FX", "OUT", "FX"]).enumerate() {
        lamp(ui, Target::Choice(Field::EqMode, k as i32), mode == k as i32, 197.0, y, l, false);
    }
}

fn quality(ui: &mut Ui) {
    let r = Rect::new(356.0, 140.0, 182.0, 89.0);
    panel(ui, r);
    ui.label(SMALL, 474.0, 160.0, DIM, "DRAFT");
    ui.label(SMALL, 512.0, 160.0, DIM, "RENDER");
    let lh = ui.c.line_height(SMALL);
    ui.c.text(SMALL, 364.0, (181.0 - lh / 2.0).round(), DIM, "HQ ENVELOPES");
    ui.c.text(SMALL, 364.0, (210.0 - lh / 2.0).round(), DIM, "OVERSAMPLING");
    for (k, x) in [(0, 474.0), (1, 512.0)] {
        let f = Field::S(28 + k);
        let on = ui.get(f) != 0;
        lamp(ui, Target::Switch(f), on, x, 181.0, "", false);
        let f = Field::S(30 + k);
        let s = ctl::text(f, ui.get(f));
        value_box(ui, f, Rect::new(x - 15.0, 200.0, 30.0, 20.0), &s);
    }
    caption(ui, r, "QUALITY");
}

fn patch_text(ui: &mut Ui, hint: &str) {
    ui.c.fill_rect(PATCH_TEXT, WELL);
    let lh = ui.c.line_height(LABEL);
    let (x, w) = (PATCH_TEXT.x + 6.0, PATCH_TEXT.w - 12.0);
    if hint.is_empty() {
        let name = if ui.m.st.name.is_empty() { "(no name)" } else { ui.m.st.name.as_str() };
        ui.c.text_clip(LABEL, x, PATCH_TEXT.y + 4.0, w, TEXT, name);
    } else {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        ui.c.text_clip(LABEL, x, PATCH_TEXT.y + 4.0, w, ACCENT, left);
        if !right.is_empty() {
            ui.c.text_right(LABEL, PATCH_TEXT.right() - 6.0, PATCH_TEXT.y + 4.0, DIM, right);
        }
    }
    let _ = lh;
    ui.hit(PATCH_TEXT, Target::PatchText);
}

// ---------------------------------------------------------------------------------------------------------------
// The operator, filter and FX pages (§3 .. §5)

/// The module a page shows (FX: module 10, the effects bus's articulators).
pub fn module_of(page: usize) -> usize {
    if page == PAGE_FX { MODULES - 1 } else { page - 1 }
}

/// What a target tab shows: an articulator, the operator's harmonics or the filter's wave shaper curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Art(usize),
    Harmonics,
    Shaper,
}

/// A page's target tabs: their labels, their hint names and what each shows.
pub fn targets(page: usize) -> &'static [(&'static str, &'static str, Shown)] {
    use Shown::*;
    const OP: [(&str, &str, Shown); 7] = [
        ("PAN", "panning", Art(OP_PAN)),
        ("VOL", "volume", Art(OP_VOLUME)),
        ("MOD", "input modulation", Art(OP_MOD)),
        ("PITCH", "pitch", Art(OP_PITCH)),
        ("PHASE", "phase", Art(OP_PHASE)),
        ("DAMP", "plucked damp", Art(OP_DAMP)),
        ("OSC", "oscillator harmonics", Harmonics),
    ];
    const FILTER: [(&str, &str, Shown); 9] = [
        ("PAN", "panning", Art(F_PAN)),
        ("VOL", "volume", Art(F_VOLUME)),
        ("CUT", "filter cutoff frequency", Art(F_CUTOFF)),
        ("RES", "filter resonance / bw / feedback", Art(F_RES)),
        ("LOW", "filter low / gain / drive", Art(F_LOW)),
        ("BAND", "filter band", Art(F_BAND)),
        ("HIGH", "filter high", Art(F_HIGH)),
        ("WS", "waveshaper", Shaper),
        ("WMIX", "waveshaper mix", Art(F_SHAPER)),
    ];
    const FX: [(&str, &str, Shown); 2] = [("PAN", "panning", Art(0)), ("VOL", "volume", Art(1))];
    match page {
        1..=6 => &OP,
        7..=9 => &FILTER,
        _ => &FX,
    }
}

pub const PART_LABELS: [&str; 8] = ["ENV", "LFO", "KEY M", "VEL M", "MOD X", "MOD Y", "RAND", "UNI"];

/// The articulator editor's parts.
pub const TARGET_TABS: Rect = Rect::new(6.0, 129.0, 540.0, 18.0);
pub const PART_TABS: Rect = Rect::new(6.0, 148.0, 540.0, 18.0);
pub const CURVE: Rect = Rect::new(6.0, 166.0, 540.0, 146.0);
pub const BAR: Rect = Rect::new(6.0, 312.0, 540.0, 34.0);

/// Tab `i` of `n` across `r`.
pub fn tab_rect(r: Rect, i: usize, n: usize) -> Rect {
    let w = r.w / n as f32;
    Rect::new((r.x + w * i as f32).round(), r.y, w.round() - 1.0, r.h)
}

/// The shown target of the page (clamped to its tabs).
pub fn shown(st: &ViewState) -> Shown {
    let t = targets(st.page);
    t[st.target[st.page].min(t.len() - 1)].2
}

fn module_page(ui: &mut Ui) {
    let page = ui.st.page;
    let m = module_of(page);
    panel(ui, Rect::new(6.0, 30.0, 540.0, 96.0));
    match page {
        1..=6 => operator(ui, m),
        7..=9 => filter(ui, m),
        _ => effects(ui),
    }
    articulators(ui, m);
}

/// A small toggle button with its text.
fn toggle(ui: &mut Ui, f: Field, r: Rect, label: &str) {
    let t = Target::Switch(f);
    let hot = ui.hot(r, &t);
    let on = ui.get(f) != 0;
    ui.c.fill_rect(r, if on { ACCENT } else if hot { EDGE } else { WELL });
    ui.c.text_center(SMALL, r, if on { BG } else { DIM }, label);
    ui.hit(r, t);
}

/// A numeric display: the value with `decimals` decimals; each digit is a drag target.
fn digits(ui: &mut Ui, f: Field, r: Rect, decimals: i32, unit: &str) {
    let v = ui.get(f);
    let scale = 10f64.powi(decimals);
    let s = format!("{:.*}", decimals as usize, v as f64 / scale);
    ui.c.fill_rect(r, WELL);
    let pressed = matches!(&ui.st.pressed, Some(Target::Digit(p, _)) if *p == f);
    ui.c.stroke_rrect(r.inset(0.5), 0.0, 1.0, if pressed { DIM } else { EDGE });
    let uw = ui.c.text_width(SMALL, unit);
    let w = ui.c.text_width(LABEL, &s);
    let x0 = (r.right() - 6.0 - uw - 4.0 - w).round();
    let lh = ui.c.line_height(LABEL);
    let y = (r.cy() - lh / 2.0).round();
    ui.c.text(LABEL, x0, y, TEXT, &s);
    let lh2 = ui.c.line_height(SMALL);
    ui.c.text(SMALL, r.right() - 6.0 - uw, (r.cy() - lh2 / 2.0).round(), DIM, unit);
    // The digits' places: ones at the point's left.
    let point = s.find('.').unwrap_or(s.len());
    for (i, ch) in s.char_indices() {
        if !ch.is_ascii_digit() {
            continue;
        }
        let place = if i < point { (point - i - 1) as i32 } else { -((i - point) as i32) };
        let a = x0 + ui.c.text_width(LABEL, &s[..i]);
        let b = x0 + ui.c.text_width(LABEL, &s[..i + ch.len_utf8()]);
        let step = 10f64.powi(decimals + place).round() as i32;
        ui.hit(Rect::span(a, b, r.y, r.bottom()), Target::Digit(f, step.max(1)));
    }
}

/// The operator page's controls (§4).
fn operator(ui: &mut Ui, m: usize) {
    // The waveform: one cycle.
    let r = Rect::new(8.0, 33.0, 80.0, 65.0);
    ui.c.fill_rect(r, WELL);
    ui.c.fill_rect(Rect::new(r.x, r.cy(), r.w, 1.0), FAINT);
    if let Some(w) = ui.m.wave {
        let n = w.len().max(2);
        let pts: Vec<(f32, f32)> =
            w.iter().enumerate().map(|(i, &v)| (r.x + 2.0 + (r.w - 4.0) * i as f32 / (n - 1) as f32, r.cy() - (r.h / 2.0 - 4.0) * v.clamp(-1.0, 1.0))).collect();
        ui.c.polyline(&pts, 1.2, ACCENT);
    }
    ui.hit(r, Target::Wave(m));
    for (k, x, l) in [(0, 20.0, "HALF"), (1, 49.0, "EVEN"), (2, 77.0, "ABS")] {
        toggle(ui, Field::Op(m, 8 + k), Rect::new(x - 12.0, 106.0, 24.0, 14.0), l);
    }
    for (k, x, l) in [(3, 106.0, "SH"), (4, 135.0, "TN"), (5, 161.0, "SK"), (6, 187.0, "SN"), (7, 213.0, "FL"), (2, 239.0, "NS")] {
        slider(ui, Field::Op(m, k), x, 56.0, 111.0, l);
    }
    switch(ui, Field::Op(m, 12), 267.0, 42.0, "CENTER");
    switch(ui, Field::Op(m, 13), 267.0, 57.0, "DECLICK");
    switch(ui, Field::Op(m, 15), 267.0, 73.0, "BAND LIMIT");
    switch(ui, Field::Op(m, 14), 267.0, 114.0, "PLUCK");
    digits(ui, Field::Op(m, 1), Rect::new(335.0, 37.0, 98.0, 20.0), 2, "Hz");
    digits(ui, Field::Op(m, 0), Rect::new(440.0, 37.0, 98.0, 20.0), 4, "x");
    ui.label(SMALL, 384.0, 64.0, DIM, "OFFSET");
    ui.label(SMALL, 489.0, 64.0, DIM, "RATIO");
    knob(ui, Field::Head(m, 0), 355.0, 92.0, 11.0, "PHASE");
    switch(ui, Field::Op(m, 11), 398.0, 90.0, "GLOBAL");
    knob(ui, Field::Head(m, 1), 476.0, 92.0, 11.0, "VOL");
    knob(ui, Field::Head(m, 2), 513.0, 92.0, 11.0, "PE");
}

/// A filter page's controls (§5).
fn filter(ui: &mut Ui, m: usize) {
    let f = Field::Filt(m, 3);
    let kind = ui.get(f).clamp(0, 12) as usize;
    value_box(ui, f, Rect::new(20.0, 36.0, 38.0, 25.0), ctl::FILTER_ABBR[kind]);
    ui.c.fill_polygon(&[(60.0, 45.0), (66.0, 45.0), (63.0, 50.0)], DIM);
    ui.hit(Rect::new(58.0, 36.0, 10.0, 25.0), Target::Box(f));
    let stages = ui.get(Field::Head(m, 0));
    for (k, (y, l)) in [42.0, 57.0, 71.0, 85.0, 99.0, 113.0].into_iter().zip(ctl::STAGE_LABELS).enumerate() {
        lamp(ui, Target::Choice(Field::Head(m, 0), k as i32), stages == k as i32, 86.0, y, l, false);
    }
    switch(ui, Field::Filt(m, 1), 30.0, 96.0, "FLAT");
    switch(ui, Field::Filt(m, 0), 30.0, 112.0, "HQ");
    for (k, x, l) in [(1, 153.0, "ENV"), (2, 198.0, "CUT"), (3, 245.0, "RES"), (7, 424.0, "AMP"), (8, 471.0, "MIX"), (9, 516.0, "NEXT")] {
        knob(ui, Field::Head(m, k), x, 56.0, 14.0, l);
    }
    // MODE: FL's "+" button, whose setting was not identified.
    let r = Rect::new(380.0, 49.0, 20.0, 20.0);
    ui.c.fill_rect(r, WELL);
    ui.c.text_center(LABEL, r, DIM, "+");
    ui.label(SMALL, 390.0, 77.0, DIM, "MODE");
    ui.hit(r, Target::Inert("Filter mode: off"));
    switch(ui, Field::Filt(m, 2), 481.0, 112.0, "WS");
    ui.label(SMALL, 470.0, 95.0, FAINT, "WAVESHAPER");
}

pub const FX_UNIT_Y: [f32; 4] = [48.0, 66.0, 83.0, 101.0];

/// The FX page's controls (§5).
fn effects(ui: &mut Ui) {
    knob(ui, Field::S(15), 21.0, 56.0, 12.0, "PAN");
    ui.label(SMALL, 67.0, 47.0, DIM, "ORD");
    let ord = ui.get(Field::S(16)).to_string();
    value_box(ui, Field::S(16), Rect::new(55.0, 56.0, 24.0, 20.0), &ord);
    lamp(ui, Target::Inert("Chorus"), false, 78.0, 113.0, "CHR", false);
    for (k, x, l) in [(17, 96.0, "DP"), (18, 118.0, "SP"), (19, 139.0, "DL"), (20, 162.0, "SR"), (21, 184.0, "CR"), (22, 206.0, "VL")] {
        slider(ui, Field::S(k), x, 54.0, 113.0, l);
    }
    ui.label(SMALL, 150.0, 121.0, FAINT, "CHORUS");
    // The send.
    let send = ctl::text(Field::S(24), ui.get(Field::S(24)));
    value_box(ui, Field::S(24), Rect::new(227.0, 41.0, 28.0, 23.0), &send);
    knob(ui, Field::S(23), 276.0, 54.0, 11.0, "VOL");
    ui.label(SMALL, 250.0, 90.0, FAINT, "SEND");
    // The unit tabs.
    let unit = ui.st.fx_unit.min(3);
    for (k, y) in FX_UNIT_Y.into_iter().enumerate() {
        let r = Rect::span(300.0, 318.0, y - 8.0, y + 8.0);
        let t = Target::FxUnit(k);
        let hot = ui.hot(r, &t);
        let sel = unit == k;
        ui.c.fill_rect(r, if sel { ACCENT } else if hot { EDGE } else { WELL });
        let on = if k < 3 { ui.m.st.delays[k].on } else { ui.m.st.reverb.on };
        let s = ["D1", "D2", "D3", "R"][k];
        ui.c.text_center(SMALL, r, if sel { BG } else if on { TEXT } else { DIM }, s);
        ui.hit(r, t);
    }
    if unit < 3 {
        let d = unit;
        switch(ui, Field::Delay(d, 6), 345.0, 80.0, "SERIAL");
        switch(ui, Field::Delay(d, 5), 345.0, 95.0, "TEMPO");
        switch(ui, Field::Delay(d, 7), 345.0, 110.0, "ON");
        for (k, x, l) in [(0, 385.0, "FB"), (1, 412.0, "TM"), (2, 438.0, "SO"), (3, 464.0, "VL")] {
            slider(ui, Field::Delay(d, k), x, 54.0, 113.0, l);
        }
        let mode = ui.get(Field::Delay(d, 4));
        for (k, (y, l)) in [47.0, 62.0, 79.0].into_iter().zip(["NORM", "INV", "PONG"]).enumerate() {
            lamp(ui, Target::Choice(Field::Delay(d, 4), k as i32), mode == k as i32, 498.0, y, l, false);
        }
        ui.label(SMALL, 425.0, 121.0, FAINT, &format!("DELAY {}", d + 1));
    } else {
        let f = Field::Reverb(7);
        let c = (ui.get(f) + 1).to_string();
        value_box(ui, f, Rect::new(329.0, 54.0, 24.0, 22.0), &c);
        ui.label(SMALL, 341.0, 45.0, DIM, "COLOR");
        switch(ui, Field::Reverb(10), 333.0, 96.0, "TEMPO");
        switch(ui, Field::Reverb(9), 333.0, 111.0, "ON");
        for (k, x, l) in [(0, 374.0, "LC"), (1, 395.0, "HC"), (2, 415.0, "PD"), (3, 436.0, "RS"), (4, 457.0, "DF"), (5, 478.0, "DE"), (6, 499.0, "HD"), (8, 520.0, "VL")] {
            slider(ui, Field::Reverb(k), x, 54.0, 113.0, l);
        }
        ui.label(SMALL, 447.0, 121.0, FAINT, "REVERB");
    }
}

/// Whether articulator `a` has a part on.
fn art_on(st: &State, m: usize, a: usize) -> bool {
    st.modules[m].arts[a].curves().any(|c| c.enabled)
}

/// The articulator editor: target tabs, source tabs, the curve view and the bar (§3).
fn articulators(ui: &mut Ui, m: usize) {
    let page = ui.st.page;
    let tabs = targets(page);
    let sel = ui.st.target[page].min(tabs.len() - 1);
    for (i, (label, _, what)) in tabs.iter().enumerate() {
        let r = tab_rect(TARGET_TABS, i, tabs.len());
        let on = match what {
            Shown::Art(a) => art_on(ui.m.st, m, *a),
            Shown::Shaper => ui.m.st.modules[m].filter.shaper_on,
            Shown::Harmonics => false,
        };
        tab(ui, r, Target::ArtTab(i), label, i == sel, on);
    }
    match tabs[sel].2 {
        Shown::Art(a) => {
            let p = ui.st.part[page].min(7);
            for (i, label) in PART_LABELS.iter().enumerate() {
                let r = tab_rect(PART_TABS, i, 8);
                let on = ctl::part(ui.m.st, m, a, i).enabled;
                tab(ui, r, Target::PartTab(i), label, i == p, on);
            }
            let c = ctl::part(ui.m.st, m, a, p);
            curve_view(ui, c);
            bar(ui, m, a, p);
        }
        Shown::Shaper => {
            let c = &ui.m.st.modules[m].filter.shaper;
            curve_view(ui, c);
            panel(ui, BAR);
        }
        Shown::Harmonics => {
            harmonics(ui, m);
            panel(ui, BAR);
        }
    }
}

fn tab(ui: &mut Ui, r: Rect, t: Target, label: &str, sel: bool, on: bool) {
    let hot = ui.hot(r, &t);
    ui.c.fill_rect(r, if sel { ACCENT } else if hot { EDGE } else { PANEL });
    ui.c.text_center(SMALL, r, if sel { BG } else { TEXT }, label);
    if on {
        ui.c.fill_rect(Rect::new(r.x + 3.0, r.y + 4.0, 2.0, r.h - 8.0), if sel { BG } else { ACCENT });
    }
    ui.hit(r, t);
}

/// The curve's points over the view: x by the points' times (maps over 0 .. 1), y by their levels.
pub fn curve_points(c: &crate::curve::Curve, r: Rect) -> Vec<(f32, f32)> {
    let mut t = 0f32;
    let times: Vec<f32> = c
        .points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i > 0 {
                t += p.dist.max(0.0);
            }
            t
        })
        .collect();
    let span = if c.kind == crate::curve::MAP { 1.0 } else { t.max(1e-3) * 1.05 };
    c.points.iter().zip(times).map(|(p, t)| (r.x + r.w * (t / span).min(1.0), r.bottom() - r.h * p.level.clamp(0.0, 1.0))).collect()
}

/// The curve view: a grid, the curve with the area below it tinted, its points and markers (read only for now).
fn curve_view(ui: &mut Ui, c: &crate::curve::Curve) {
    let r = CURVE;
    let map = c.kind == crate::curve::MAP;
    ui.c.fill_rect(r, if map { WELL.shade(-0.2) } else { WELL });
    for k in 1..8 {
        let x = (r.x + r.w * k as f32 / 8.0).round();
        ui.c.fill_rect(Rect::new(x, r.y, 1.0, r.h), BG);
    }
    for k in 1..4 {
        let y = (r.y + r.h * k as f32 / 4.0).round();
        ui.c.fill_rect(Rect::new(r.x, y, r.w, 1.0), BG);
    }
    let inner = r.inset(6.0);
    let pts = curve_points(c, inner);
    if pts.is_empty() {
        return;
    }
    let ink = if c.enabled { ACCENT } else { DIM };
    let mut area = pts.clone();
    area.push((pts.last().unwrap().0, inner.bottom()));
    area.push((pts[0].0, inner.bottom()));
    ui.c.fill_polygon(&area, ink.with_alpha(40));
    ui.c.polyline(&pts, 1.5, ink);
    for &(x, y) in &pts {
        ui.c.fill_circle(x, y, 2.5, ink);
    }
    for (i, tag) in [(c.decay, "D"), (c.sustain, "S"), (c.loop_start, "L")] {
        if let Some(&(x, _)) = usize::try_from(i).ok().and_then(|i| pts.get(i)) {
            ui.c.fill_rect(Rect::new(x.round(), r.y, 1.0, r.h), DIM);
            ui.c.text(SMALL, x + 2.0, r.bottom() - 12.0, DIM, tag);
        }
    }
    ui.hit(r, Target::Inert("Curve editing comes in a later stage"));
}

/// The OSC target: the cycle on top, the harmonics' levels below (the first 88).
fn harmonics(ui: &mut Ui, m: usize) {
    let r = CURVE;
    ui.c.fill_rect(r, WELL);
    let top = Rect::new(r.x, r.y, r.w, 60.0);
    ui.c.fill_rect(Rect::new(r.x, top.bottom(), r.w, 1.0), BG);
    if let Some(w) = ui.m.wave {
        let n = w.len().max(2);
        let pts: Vec<(f32, f32)> = w.iter().enumerate().map(|(i, &v)| (top.x + top.w * i as f32 / (n - 1) as f32, top.cy() - 26.0 * v.clamp(-1.0, 1.0))).collect();
        ui.c.polyline(&pts, 1.2, ACCENT);
    }
    let levels = &ui.m.st.modules[m].op.levels;
    let bars = Rect::span(r.x + 4.0, r.right() - 4.0, top.bottom() + 4.0, r.bottom() - 2.0);
    for (h, &l) in levels.iter().take(88).enumerate() {
        let x = bars.x + h as f32 * bars.w / 88.0;
        let y = bars.bottom() - bars.h * l.clamp(0.0, 1.0);
        ui.c.fill_rect(Rect::span(x, x + 5.0, y, bars.bottom()), if l > 0.0 { ACCENT } else { FAINT });
    }
    ui.hit(r, Target::Inert("Harmonics editing comes in a later stage"));
}

/// The bar under the curve view: the part's switch, the envelope's or LFO's knobs and switches, the tools.
fn bar(ui: &mut Ui, m: usize, a: usize, p: usize) {
    panel(ui, BAR);
    switch(ui, Field::Part(m, a, p, 0), 15.0, 320.0, "");
    if p < 2 {
        let labels = if p == 0 { ["ATT", "DEC", "SUS", "REL"] } else { ["SPD", "TNS", "SKEW", "PW"] };
        for (k, (x, l)) in [35.0, 58.0, 81.0, 104.0].into_iter().zip(labels).enumerate() {
            knob(ui, Field::Part(m, a, p, k + 1), x, 323.0, 8.0, l);
        }
        switch(ui, Field::Part(m, a, p, 5), 123.0, 319.0, "TEMPO");
        switch(ui, Field::Part(m, a, p, 6), 123.0, 335.0, "GLOBAL");
    }
    for (x, h) in [(458.0, "Freeze editing / hide points"), (482.0, "Step editing"), (505.0, "Snap to grid"), (529.0, "Slide succeeding points / regions")] {
        let r = Rect::new(x - 10.0, 327.0, 20.0, 16.0);
        ui.c.fill_rect(r, WELL);
        ui.hit(r, Target::Inert(h));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Matrix (§6)

/// Cell (r, c)'s centre.
pub fn cell_centre(r: usize, c: usize) -> (f32, f32) {
    (567.0 + 31.0 * c as f32 + 15.0, 48.0 + 31.0 * r as f32 + 15.0)
}

/// The page a matrix row or column label selects (`None` for PAN and OUT).
pub fn label_page(row: bool, i: usize) -> Option<usize> {
    match (row, i) {
        (_, 0..=5) => Some(i + 1),
        (true, 6..=8) => Some(i + 1),
        (false, COL_SEND) => Some(PAGE_FX),
        _ => None,
    }
}

fn matrix(ui: &mut Ui) {
    panel(ui, MATRIX);
    let ring = ui.st.ring;
    let accent = if ring { RING } else { ACCENT };
    let cols = ["1", "2", "3", "4", "5", "6", "PAN", "FX", "OUT"];
    let rows = ["1", "2", "3", "4", "5", "6", "F1", "F2", "F3"];
    for (c, s) in cols.into_iter().enumerate() {
        let (x, _) = cell_centre(0, c);
        let r = Rect::new(x - 15.0, 30.0, 30.0, 17.0);
        let sel = label_page(false, c) == Some(ui.st.page);
        if sel {
            ui.c.fill_rect(r, accent);
        }
        ui.c.text_center(SMALL, r, if sel { BG } else { DIM }, s);
        if label_page(false, c).is_some() {
            ui.hit(r, Target::Col(c));
        }
    }
    for (row, s) in rows.into_iter().enumerate() {
        let (_, y) = cell_centre(row, 0);
        let r = Rect::new(547.0, y - 15.0, 19.0, 30.0);
        let sel = label_page(true, row) == Some(ui.st.page);
        if sel {
            ui.c.fill_rect(r, accent);
        }
        ui.c.text_center(SMALL, r, if sel { BG } else { DIM }, s);
        ui.hit(r, Target::Row(row));
    }
    for row in 0..9 {
        for c in 0..9 {
            let ring_cell = ring && row < OPERATORS && c < OPERATORS;
            let f = if ring_cell { Field::Ring(row, c) } else { Field::Cell(row, c) };
            let off = ui.m.st.cell_off[if ring_cell { 81 } else { 0 } + row * 9 + c] != 0;
            let (cx, cy) = cell_centre(row, c);
            let hr = Rect::new(cx - 15.0, cy - 15.0, 30.0, 30.0);
            let t = Target::Cell(f);
            let hot = ui.hot(hr, &t);
            if off {
                muted(ui, cx, cy, hot);
            } else {
                let v = ui.get(f);
                let n = (v + 256) as f32 / 512.0;
                dial(ui, cx, cy, 11.0, n, true, if v == 0 { FAINT } else { accent }, hot);
            }
            ui.hit(hr, t);
        }
    }
    for (k, (x, s)) in [(548.0, "FM"), (594.0, "RM")].into_iter().enumerate() {
        let r = Rect::new(x, 327.0, 44.0, 17.0);
        let t = Target::MatrixTab(k == 1);
        let sel = ring == (k == 1);
        let hot = ui.hot(r, &t);
        ui.c.fill_rect(r, if sel { accent } else if hot { EDGE } else { WELL });
        ui.c.text_center(SMALL, r, if sel { BG } else { TEXT }, s);
        ui.hit(r, t);
    }
    ui.c.text_right(LABEL, 840.0, 329.0, DIM, "MATRIX");
}

/// An off cell: a muted speaker.
fn muted(ui: &mut Ui, cx: f32, cy: f32, hot: bool) {
    let ink = if hot { TEXT } else { DIM };
    ui.c.fill_polygon(&[(cx - 7.0, cy - 2.5), (cx - 4.0, cy - 2.5), (cx, cy - 6.0), (cx, cy + 6.0), (cx - 4.0, cy + 2.5), (cx - 7.0, cy + 2.5)], ink);
    ui.c.line(cx + 2.5, cy - 3.0, cx + 7.5, cy + 3.0, 1.2, ink);
    ui.c.line(cx + 2.5, cy + 3.0, cx + 7.5, cy - 3.0, 1.2, ink);
}

// ---------------------------------------------------------------------------------------------------------------
// Testing keyboard

const WHITE: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

/// The white keys' width.
fn white_w() -> f32 {
    KEYS.w / 35.0
}

/// Key `k`'s rect; black keys over the whites.
pub fn key_rect(k: u8) -> Rect {
    let o = (k - FIRST_KEY) / 12;
    let n = (k - FIRST_KEY) % 12;
    let ww = white_w();
    match WHITE.iter().position(|&w| w == n) {
        Some(i) => Rect::new(KEYS.x + ww * (o as f32 * 7.0 + i as f32), KEYS.y, ww, KEYS.h),
        None => {
            let left = WHITE.iter().position(|&w| w == n - 1).unwrap();
            let x = KEYS.x + ww * (o as f32 * 7.0 + left as f32 + 1.0);
            Rect::new(x - ww * 0.3, KEYS.y, ww * 0.6, KEYS.h * 0.6)
        }
    }
}

pub fn is_black(k: u8) -> bool {
    !WHITE.contains(&(k % 12))
}

/// A key's name (FL's: key 60 = C5).
pub fn key_name(k: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[usize::from(k % 12)], k / 12)
}

fn keyboard(ui: &mut Ui) {
    for black in [false, true] {
        for k in (FIRST_KEY..=LAST_KEY).filter(|&k| is_black(k) == black) {
            let r = key_rect(k);
            let held = ui.st.key == Some(k);
            let fill = match (black, held) {
                (_, true) => ACCENT,
                (false, _) => Color::rgb(0xe4e4e4),
                (true, _) => Color::rgb(0x202020),
            };
            ui.c.fill_rect(Rect::new(r.x + 0.5, r.y, r.w - 1.0, r.h), fill);
            if k % 12 == 0 {
                ui.label(SMALL, r.cx(), r.bottom() - 9.0, Color::rgb(0x606060), &key_name(k));
            }
            ui.hit(r, Target::Key(k));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The info page and the block diagram (§7)

fn info(ui: &mut Ui) {
    let (w, h) = ui.c.size();
    let mid = Rect::new(0.0, h / 2.0 - 60.0, w, 30.0);
    ui.c.text_center(Font::new(26).bold(), mid, TEXT, "Citrus");
    let v = format!("Version {}", ui.m.version);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 8.0, w, 16.0), DIM, &v);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 26.0, w, 16.0), DIM, "Lemon Studio's recreation of FL's Sytrus");
    ui.c.text_center(SMALL, Rect::new(0.0, h - 30.0, w, 16.0), FAINT, "Click to return");
    ui.hit(Rect::new(0.0, 0.0, w, h), Target::Cover);
}

/// A box of the diagram with its name; returns its rect.
fn block(ui: &mut Ui, x: f32, y: f32, w: f32, s: &str) -> Rect {
    let r = Rect::new(x, y, w, 24.0);
    ui.c.fill_rect(r, PANEL);
    ui.c.stroke_rrect(r.inset(0.5), 0.0, 1.0, DIM);
    ui.c.text_center(SMALL, r, TEXT, s);
    r
}

fn arrow(ui: &mut Ui, x0: f32, y0: f32, x1: f32, y1: f32) {
    ui.c.line(x0, y0, x1, y1, 1.0, DIM);
    let a = (y1 - y0).atan2(x1 - x0);
    let p = |d: f32| (x1 - 6.0 * (a + d).cos(), y1 - 6.0 * (a + d).sin());
    ui.c.fill_polygon(&[(x1, y1), p(0.4), p(-0.4)], DIM);
}

/// The signal flow: six operators through the matrix (phase and ring modulation, panning), three filters, the
/// unison and output gains, then the effects bus (chorus, delays, reverb) and the equaliser.
fn diagram(ui: &mut Ui) {
    let (w, h) = ui.c.size();
    ui.c.text_center(Font::new(14).bold(), Rect::new(0.0, 16.0, w, 20.0), TEXT, "Citrus: block diagram");
    let top = 70.0;
    let ops: Vec<Rect> = (0..6).map(|k| block(ui, 30.0, top + 38.0 * k as f32, 90.0, &format!("Operator {}", k + 1))).collect();
    let mat = block(ui, 180.0, top + 85.0, 120.0, "Matrix (FM / RM, pan)");
    for r in &ops {
        arrow(ui, r.right(), r.cy(), mat.x, mat.cy());
    }
    let fil: Vec<Rect> = (0..3).map(|k| block(ui, 360.0, top + 40.0 + 50.0 * k as f32, 90.0, &format!("Filter {}", k + 1))).collect();
    for r in &fil {
        arrow(ui, mat.right(), mat.cy(), r.x, r.cy());
    }
    for k in 0..2 {
        arrow(ui, fil[k].cx(), fil[k].bottom(), fil[k + 1].cx(), fil[k + 1].y);
    }
    let out = block(ui, 510.0, top + 40.0, 90.0, "Output");
    let fx = block(ui, 510.0, top + 140.0, 90.0, "Effects send");
    for r in fil.iter().chain([&mat]) {
        arrow(ui, r.right(), r.cy(), out.x, out.cy());
        arrow(ui, r.right(), r.cy(), fx.x, fx.cy());
    }
    let ch = block(ui, 510.0, top + 190.0, 90.0, "Chorus");
    let dl = block(ui, 510.0, top + 240.0, 90.0, "Delays 1 .. 3");
    let rv = block(ui, 640.0, top + 240.0, 90.0, "Reverb");
    arrow(ui, fx.cx(), fx.bottom(), ch.cx(), ch.y);
    arrow(ui, ch.cx(), ch.bottom(), dl.cx(), dl.y);
    arrow(ui, dl.right(), dl.cy(), rv.x, rv.cy());
    let eq = block(ui, 640.0, top + 40.0, 90.0, "Equaliser");
    arrow(ui, out.right(), out.cy(), eq.x, eq.cy());
    arrow(ui, rv.cx(), rv.y, eq.cx(), eq.bottom());
    let main = block(ui, 760.0, top + 40.0, 70.0, "Out");
    arrow(ui, eq.right(), eq.cy(), main.x, main.cy());
    ui.c.text_center(SMALL, Rect::new(0.0, h - 30.0, w, 16.0), FAINT, "Click to return");
    ui.hit(Rect::new(0.0, 0.0, w, h), Target::Cover);
}
