// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Citrus editor: painting and hit regions (platform independent). Sytrus's layout at its Small size, every control
//! where FL has it, unstyled: flat placeholder colours, no artwork (the Lemon design comes later). Stage 1: the page
//! tabs, the header buttons, the main page, the matrix and the testing keyboard; the other pages show their name.
//!
// spec: plugins/sytrus-editor.md (§1 surface, §2 main page, §6 matrix, §7 header buttons), plugins/citrus.md §6

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
}

/// What a paint shows of the patch.
pub struct Model<'a> {
    pub st: &'a State,
    /// Modules in use (the tabs' marker bars).
    pub used: [bool; 9],
    pub version: &'a str,
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
                other_page(&mut ui);
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

/// The pages of stage 2 and 3: their module's name for now.
fn other_page(ui: &mut Ui) {
    let r = PAGE.inset(4.0);
    panel(ui, r);
    let name = match ui.st.page {
        p @ 1..=6 => format!("Operator {p}"),
        p @ 7..=9 => format!("Filter {}", p - 6),
        _ => "Effects".into(),
    };
    ui.c.text_center(Font::new(14).bold(), Rect::new(r.x, r.y + 20.0, r.w, 20.0), TEXT, &name);
    ui.c.text_center(LABEL, Rect::new(r.x, r.y + 44.0, r.w, 16.0), DIM, "This page's controls come in a later stage.");
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
