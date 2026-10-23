// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Citrus editor: painting and hit regions (platform independent). Sytrus's layout at its Small size, every control
//! where FL has it, in the Lemon "Pith" plugin look: outlined grey cards, dark wells, lemon for what is on. The page
//! tabs, the header buttons, the main page, the operator, filter and FX pages' controls with their articulators' tabs
//! and bars with the curve view and the harmonics editor, the matrix and the testing keyboard. Controls that do
//! nothing yet are greyed out.
//!
// spec: plugins/sytrus-editor.md (§1 .. §7), plugins/citrus.md §6, ui/lemon-design.md §11.1

use os_theme::design::lemon as brand;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{eq as keq, State as Look};
use os_ui::{Canvas, Color, Font, Rect};

use crate::ctl::{self, Field};
use crate::edit;
use crate::state::*;

/// Editor size in logical pixels, with the testing keyboard and without it.
pub const W: u32 = 850;
pub const H: u32 = 450;
pub const H_NO_KEYS: u32 = 348;

/// The Pith colours (`ui/lemon-design.md` §11.1).
pub const BG: Color = plug::BG;
pub const PANEL: Color = plug::PANEL;
const WELL: Color = plug::WELL;
/// Raised faces (handles, buttons, marker tags).
const EDGE: Color = plug::FACE;
pub const TEXT: Color = brand::TEXT;
const DIM: Color = plug::DIM;
const FAINT: Color = plug::FAINT;
/// Captions of controls that do nothing yet.
const OFF: Color = plug::OFF;
pub const ACCENT: Color = brand::MAIN;
/// The matrix's accent while the ring-modulation matrix is shown (FL draws it in another colour than FM's).
pub const RING: Color = Color::rgb(0xbcb4e8);

const LABEL: Font = Font::new(10);
const SMALL: Font = Font::new(9);

/// The pages: MAIN, OP 1 .. OP 6, FILTER 1 .. 3, FX.
pub const PAGES: usize = 11;
pub const PAGE_FX: usize = 10;
const TAB_NAMES: [&str; PAGES] = ["MAIN", "OP 1", "OP 2", "OP 3", "OP 4", "OP 5", "OP 6", "FILTER 1", "FILTER 2", "FILTER 3", "FX"];

/// The page area and the matrix panel.
pub const PAGE: Rect = Rect::new(0.0, 26.0, 546.0, 320.0);
pub const MATRIX: Rect = Rect::new(550.0, 31.0, 296.0, 315.0);
/// The matrix's cell pitch (FL's is 31 px; Pith's card padding takes the rest) and its first cell's corner.
const PITCH: f32 = 29.0;
const GRID_X: f32 = 577.0;
const GRID_Y: f32 = 52.0;
pub const KEYS: Rect = Rect::new(4.0, 355.0, 842.0, 95.0);
/// The testing keyboard's keys: C3 .. B7 (FL's names, key 60 = C5).
pub const FIRST_KEY: u8 = 36;
pub const LAST_KEY: u8 = 95;
/// The XY pad.
pub const PAD: Rect = Rect::span(14.0, 142.0, 166.0, 293.0);
pub const PATCH_TEXT: Rect = Rect::new(10.0, 305.0, 341.0, 37.0);

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
    /// The curve view: a point (drag, its menu on right click), a segment's tension handle (drag), empty space (a
    /// right click adds a point).
    Point(usize),
    Tension(usize),
    CurveArea,
    /// The curve tools: freeze, step, snap, slide.
    Tool(usize),
    /// The Options arrows of the curve view and of the harmonics editor.
    CurveOptions,
    HarmOptions,
    /// A harmonic's bar (drag paints levels) and the harmonics' scroll bar.
    Harm(usize),
    HarmScroll,
    /// The Level scaling dialog: its controls (centre, multiplier, offset, tension: vertical drag), its buttons
    /// (Reset, Accept, close) and the rest of the surface, which it blocks.
    ScaleCtl(usize),
    ScaleBtn(usize),
    Dialog,
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
    /// The keys playing, the host's notes too (as [`crate::CitrusParams::playing`]).
    pub playing: [u64; 2],
    /// Per page: the articulator page's target tab and source tab.
    pub target: [usize; PAGES],
    pub part: [usize; PAGES],
    /// The FX page's unit (delays 0 .. 2, 3 the reverb).
    pub fx_unit: usize,
    /// The curve view's time span while a point is dragged (it would follow the point otherwise).
    pub span: Option<f64>,
    /// The curve tools that are on (freeze, step, snap, slide).
    pub tools: [bool; 4],
    /// The first harmonic the harmonics editor shows.
    pub harm_scroll: usize,
    /// The curve view's selected points, by index.
    pub selected: Vec<bool>,
    /// The rubber band a Ctrl + drag on empty space draws.
    pub band: Option<Rect>,
    /// The Level scaling dialog, while it is open: centre, multiplier, offset, tension.
    pub scale: Option<[f32; 4]>,
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
    /// The kit's state for a control: hovered (or held), pressed.
    fn state(&self, r: Rect, t: &Target) -> Look {
        Look { hover: self.hot(r, t), pressed: self.st.pressed.as_ref() == Some(t), on: false, disabled: false }
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
            if let Some(v) = st.scale {
                scale_dialog(&mut ui, v);
            }
        }
    }
    ui.hits
}

// ---------------------------------------------------------------------------------------------------------------
// Widgets

fn panel(ui: &mut Ui, r: Rect) {
    lemon::plug_panel(ui.c, r);
}

/// A fader's groove from `top` to `bot` at `x`: the value filled in lemon from `zero`, the cap at `y`.
fn fader(ui: &mut Ui, x: f32, top: f32, bot: f32, y: f32, zero: f32, st: Look) {
    keq::slot(ui.c, Rect::new(x - 3.0, top - 4.0, 6.0, bot - top + 8.0), st);
    if (y - zero).abs() > 1.0 {
        ui.c.fill_rrect(Rect::span(x - 1.0, x + 1.0, y.min(zero), y.max(zero)), 1.0, ACCENT.with_alpha(170));
    }
    keq::handle(ui.c, x, y.round(), ACCENT, st);
}

/// A vertical fader centred on `x`, its track from `top` to `bot`, the label above it.
fn slider(ui: &mut Ui, f: Field, x: f32, top: f32, bot: f32, label: &str) {
    let t = Target::Slider(f);
    let hr = Rect::span(x - 10.0, x + 10.0, top - 8.0, bot + 8.0);
    let st = ui.state(hr, &t);
    let (lo, hi) = ctl::range(f);
    let y_of = |n: f32| bot - (bot - top) * n.clamp(0.0, 1.0);
    let y = y_of(ui.norm(f));
    let zero = if lo < 0 { y_of(-lo as f32 / (hi - lo) as f32) } else { bot };
    fader(ui, x, top, bot, y, zero, st);
    if !label.is_empty() {
        ui.label(SMALL, x, top - 11.0, DIM, label);
    }
    ui.hit(hr, t);
}

/// A knob of radius `r`: the kit's plugin dial, the value's arc in `accent` (from the top for bipolar ranges).
fn dial(ui: &mut Ui, cx: f32, cy: f32, r: f32, norm: f32, bipolar: bool, accent: Color, hot: bool) {
    lemon::dial(ui.c, cx, cy, r, (r * 0.2).clamp(1.8, 2.8), norm, bipolar, accent, Look::hover(hot));
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
    let st = ui.state(r, &t);
    lemon::field(ui.c, r, st);
    ui.c.text_center(LABEL, r, TEXT, text);
    ui.hit(r, t);
}

/// A switch at (cx, cy): the kit's rounded-square toggle, lemon when on, its label to the right (or under it). A
/// control whose setting was not identified (`Target::Inert`) is greyed out.
fn lamp(ui: &mut Ui, t: Target, on: bool, cx: f32, cy: f32, label: &str, under: bool) {
    let w = ui.c.text_width(SMALL, label);
    let hr = if under { Rect::new(cx - 10.0, cy - 7.0, 20.0, 22.0) } else { Rect::new(cx - 7.0, cy - 7.0, w + 18.0, 14.0) };
    let mut st = ui.state(hr, &t);
    st.disabled = matches!(t, Target::Inert(_));
    lemon::toggle(ui.c, cx, cy, on, st);
    let ink = if st.disabled { OFF } else { DIM };
    if under {
        ui.label(SMALL, cx, cy + 11.0, ink, label);
    } else {
        let lh = ui.c.line_height(SMALL);
        ui.c.text(SMALL, cx + 10.0, (cy - lh / 2.0).round(), ink, label);
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

/// The page tabs on the Pith page strip: the chosen page lit lemon, a lemon dot on the modules in use.
fn tabs(ui: &mut Ui) {
    ui.c.fill_rect(Rect::new(0.0, 0.0, W as f32, 28.0), plug::STRIP);
    ui.c.fill_rect(Rect::new(0.0, 28.0, W as f32, 1.0), plug::STRIP_LINE);
    let fx_used = (0..9).any(|r| ui.m.used[r] && ui.m.st.cell(r, COL_SEND) != 0);
    for (i, name) in TAB_NAMES.iter().enumerate() {
        let r = Rect::new(6.0 + 68.0 * i as f32, 4.0, 66.0, 22.0);
        let t = Target::Tab(i);
        let hot = ui.hot(r, &t);
        let sel = ui.st.page == i;
        let cell = r.inset(1.0);
        if sel {
            lemon::lit(ui.c, cell, 7.0, hot);
        } else if hot {
            ui.c.fill_rrect(cell, 7.0, Color::rgb(0x333333));
        }
        let ink = if sel { brand::COLORED_DARK } else if hot { TEXT } else { DIM };
        ui.c.text_center(LABEL.bold(), r, ink, name);
        let used = match i {
            1..=9 => ui.m.used[i - 1],
            PAGE_FX => fx_used,
            _ => false,
        };
        if used {
            ui.c.fill_circle(r.right() - 7.0, r.cy(), 2.0, if sel { brand::COLORED_DARK } else { ACCENT });
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
        let mut st = ui.state(r, &t);
        st.on = i == 3 && !ui.st.no_keys;
        let ink = lemon::plug_button(ui.c, r, 5.0, st);
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
    knob(ui, Field::Soften, 520.0, 261.0, 10.0, "SOFTEN");
    patch_text(ui, hint);
    // The name where FL has its logo (Citrus has no Lemon wordmark).
    let r = Rect::new(356.0, 298.0, 182.0, 44.0);
    name(ui, r, Font::condensed(24, 0).bold());
    ui.hit(r, Target::Logo);
}

fn modulation(ui: &mut Ui) {
    panel(ui, Rect::new(10.0, 140.0, 166.0, 159.0));
    ui.label(SMALL, 40.0, 151.0, DIM, "X / Y");
    switch(ui, Field::S32(0), 124.0, 150.0, "SMOOTH");
    lemon::plug_well(ui.c, PAD, 6.0);
    ui.c.fill_rect(Rect::span(PAD.cx(), PAD.cx(), PAD.y + 2.0, PAD.bottom() - 3.0), plug::TICK);
    ui.c.fill_rect(Rect::span(PAD.x + 2.0, PAD.right() - 3.0, PAD.cy(), PAD.cy()), plug::TICK);
    let (x, y) = pad_point(ui.get(Field::S(25)), ui.get(Field::S(26)));
    ui.c.fill_circle(x, y, 5.5, ACCENT.with_alpha(60));
    ui.c.fill_circle(x, y, 3.5, ACCENT);
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
        let st = ui.state(r, &t);
        lemon::field(ui.c, r, st);
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
    ui.c.text(SMALL, 364.0, 146.0, DIM, "QUALITY");
}

/// "CITRUS" in the wordmark's grey condensed caps, centred in `r`.
fn name(ui: &mut Ui, r: Rect, f: Font) {
    let wn = lemon::spaced_width(ui.c, f, "CITRUS", 3.0);
    let y = (r.cy() - ui.c.line_height(f) / 2.0).round();
    lemon::spaced(ui.c, f, (r.cx() - wn / 2.0).round(), y, plug::WORDMARK, "CITRUS", 3.0);
}

fn patch_text(ui: &mut Ui, hint: &str) {
    lemon::plug_well(ui.c, PATCH_TEXT, 6.0);
    let lh = ui.c.line_height(LABEL);
    let (x, w) = (PATCH_TEXT.x + 6.0, PATCH_TEXT.w - 12.0);
    if hint.is_empty() {
        let text = plain_text(&ui.m.st.name);
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        let name = lines.next().unwrap_or("(no name)");
        ui.c.text_clip(LABEL, x, PATCH_TEXT.y + 4.0, w, TEXT, name);
        if let Some(more) = lines.next() {
            ui.c.text_clip(SMALL, x, PATCH_TEXT.y + 5.0 + lh, w, DIM, more);
        }
    } else {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        ui.c.text_clip(LABEL, x, PATCH_TEXT.y + 4.0, w, ACCENT, left);
        if !right.is_empty() {
            ui.c.text_right(LABEL, PATCH_TEXT.right() - 6.0, PATCH_TEXT.y + 4.0, DIM, right);
        }
    }
    ui.hit(PATCH_TEXT, Target::PatchText);
}

/// A patch text's plain text: RTF (`{\rtf1 ...}`) without its control words and its font, colour and style tables,
/// `\par` and `\line` as line breaks, `\'hh` as Windows-1252 (Latin-1 here); other text unchanged.
pub fn plain_text(s: &str) -> String {
    if !s.starts_with("{\\rtf") {
        return s.to_string();
    }
    const SKIP: [&str; 6] = ["fonttbl", "colortbl", "stylesheet", "info", "pict", "generator"];
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::new();
    // Per group: whether its text is skipped.
    let mut stack = vec![false];
    let mut i = 0;
    while i < ch.len() {
        let skip = *stack.last().unwrap_or(&false);
        match ch[i] {
            '{' => {
                stack.push(skip);
                i += 1;
            }
            '}' => {
                stack.pop();
                i += 1;
            }
            '\\' if i + 1 < ch.len() => {
                let n = ch[i + 1];
                if n == '\\' || n == '{' || n == '}' {
                    if !skip {
                        out.push(n);
                    }
                    i += 2;
                } else if n == '\'' {
                    let hex: String = ch.iter().skip(i + 2).take(2).collect();
                    if let (false, Ok(v)) = (skip, u8::from_str_radix(&hex, 16)) {
                        out.push(v as char);
                    }
                    i += 4;
                } else if n == '*' {
                    if let Some(top) = stack.last_mut() {
                        *top = true;
                    }
                    i += 2;
                } else if n.is_ascii_alphabetic() {
                    let mut j = i + 1;
                    while j < ch.len() && ch[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    let word: String = ch[i + 1..j].iter().collect();
                    while j < ch.len() && (ch[j] == '-' || ch[j].is_ascii_digit()) {
                        j += 1;
                    }
                    if j < ch.len() && ch[j] == ' ' {
                        j += 1;
                    }
                    if SKIP.contains(&word.as_str()) {
                        if let Some(top) = stack.last_mut() {
                            *top = true;
                        }
                    } else if !skip && (word == "par" || word == "line") {
                        out.push('\n');
                    } else if !skip && word == "tab" {
                        out.push(' ');
                    }
                    i = j;
                } else {
                    i += 2;
                }
            }
            '\r' | '\n' => i += 1,
            c => {
                if !skip {
                    out.push(c);
                }
                i += 1;
            }
        }
    }
    out
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
    let mut st = ui.state(r, &t);
    st.on = ui.get(f) != 0;
    let ink = lemon::plug_button(ui.c, r, 4.0, st);
    ui.c.text_center(SMALL.bold(), r, ink, label);
    ui.hit(r, t);
}

/// A numeric display: the value with `decimals` decimals; each digit is a drag target.
fn digits(ui: &mut Ui, f: Field, r: Rect, decimals: i32, unit: &str) {
    let v = ui.get(f);
    let scale = 10f64.powi(decimals);
    let s = format!("{:.*}", decimals as usize, v as f64 / scale);
    let pressed = matches!(&ui.st.pressed, Some(Target::Digit(p, _)) if *p == f);
    let hover = ui.st.pressed.is_none() && ui.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
    lemon::field(ui.c, r, Look { hover, pressed, on: false, disabled: false });
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
    lemon::plug_well(ui.c, r, 6.0);
    ui.c.fill_rect(Rect::new(r.x + 2.0, r.cy(), r.w - 4.0, 1.0), plug::TICK);
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
    for (k, x, l) in [(3, 106.0, "SH"), (4, 135.0, "TN"), (5, 161.0, "SK"), (6, 187.0, "SN"), (7, 213.0, "PF"), (2, 239.0, "NS")] {
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
    // MODE: FL's "+" button, whose setting was not identified: greyed out.
    let r = Rect::new(380.0, 49.0, 20.0, 20.0);
    let ink = lemon::plug_button(ui.c, r, 5.0, Look { disabled: true, ..Look::NORMAL });
    ui.c.text_center(LABEL, r, ink, "+");
    ui.label(SMALL, 390.0, 77.0, OFF, "MODE");
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
    lamp(ui, Target::Inert("Chorus"), false, 78.0, 113.0, "", false);
    let lh = ui.c.line_height(SMALL);
    ui.c.text_right(SMALL, 68.0, (113.0 - lh / 2.0).round(), OFF, "CHR");
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
        let st = ui.state(r, &t);
        let sel = unit == k;
        let on = if k < 3 { ui.m.st.delays[k].on } else { ui.m.st.reverb.on };
        let ink = lemon::choice_button(ui.c, r, sel, ACCENT, st);
        let s = ["D1", "D2", "D3", "R"][k];
        ui.c.text_center(SMALL.bold(), r, if !sel && on { TEXT } else { ink }, s);
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
            tools(ui);
        }
        Shown::Harmonics => harmonics(ui, m),
    }
}

/// A target or source tab: the chosen one a dark rounded pill with its name in lemon (Pith's sets of choices), a
/// lemon dot when its part is on.
fn tab(ui: &mut Ui, r: Rect, t: Target, label: &str, sel: bool, on: bool) {
    let hot = ui.hot(r, &t);
    let pill = r.inset(1.0);
    if sel {
        lemon::outlined(ui.c, pill, 6.0, plug::FIELD_LINE, plug::FIELD);
    } else if hot {
        ui.c.fill_rrect(pill, 6.0, Color::rgb(0x333333));
    }
    let ink = if sel { ACCENT } else if hot { TEXT } else { DIM };
    ui.c.text_center(SMALL.bold(), r, ink, label);
    if on {
        ui.c.fill_circle(r.x + 7.0, r.cy(), 2.0, ACCENT);
    }
    ui.hit(r, t);
}

/// Where the curve view draws its curve: time 0 half a pixel in, level 1 on the top edge, level 0 3.5 px above the
/// bottom (FL's points, measured).
pub fn curve_inner() -> Rect {
    Rect::new(CURVE.x + 0.5, CURVE.y, CURVE.w - 1.0, CURVE.h - 3.5)
}

/// The time the curve view spans: a map 0 .. 1, an envelope or LFO its length and a tenth more (at least one unit,
/// half a second or a beat).
pub fn curve_span(c: &crate::curve::Curve) -> f64 {
    if c.kind == crate::curve::MAP {
        1.0
    } else {
        (edit::times(c).last().copied().unwrap_or(0.0) * 1.1).max(1.0)
    }
}

pub fn t_to_x(t: f64, span: f64) -> f32 {
    let r = curve_inner();
    r.x + r.w * (t / span) as f32
}

pub fn x_to_t(x: f32, span: f64) -> f64 {
    let r = curve_inner();
    f64::from((x - r.x) / r.w).max(0.0) * span
}

pub fn level_to_y(l: f32) -> f32 {
    let r = curve_inner();
    r.bottom() - r.h * l.clamp(0.0, 1.0)
}

pub fn y_to_level(y: f32) -> f32 {
    let r = curve_inner();
    ((r.bottom() - y) / r.h).clamp(0.0, 1.0)
}

/// The curve view: a grid, the curve with the area below it tinted, its points (filled), its tension handles
/// (hollow, halfway along their segments) and its markers.
fn curve_view(ui: &mut Ui, c: &crate::curve::Curve) {
    let r = CURVE;
    let map = c.kind == crate::curve::MAP;
    lemon::plug_well(ui.c, r, 6.0);
    if map {
        ui.c.fill_rrect(r.inset(1.0), 5.0, Color::rgb(0x1a1a1a));
    }
    for k in 1..8 {
        let x = (r.x + r.w * k as f32 / 8.0).round();
        ui.c.fill_rect(Rect::new(x, r.y + 1.0, 1.0, r.h - 2.0), plug::GRID);
    }
    for k in 1..4 {
        let y = (r.y + r.h * k as f32 / 4.0).round();
        ui.c.fill_rect(Rect::new(r.x + 1.0, y, r.w - 2.0, 1.0), plug::GRID);
    }
    let inner = curve_inner();
    let span = ui.st.span.unwrap_or_else(|| curve_span(c));
    let ts = edit::times(c);
    let n = ts.len();
    if n == 0 {
        return;
    }
    let mut line = vec![(t_to_x(0.0, span), level_to_y(c.points[0].level))];
    for i in 1..n {
        let (x0, x1) = (t_to_x(ts[i - 1], span), t_to_x(ts[i], span));
        let steps = ((x1 - x0) / 2.0).ceil().clamp(1.0, 200.0) as usize;
        for s in 1..=steps {
            let x = s as f64 / steps as f64;
            line.push((x0 + (x1 - x0) * x as f32, level_to_y(edit::segment_level(c, i, x))));
        }
        line.push((x1, level_to_y(c.points[i].level)));
    }
    let ink = if c.enabled || map { ACCENT } else { FAINT };
    let mut area = line.clone();
    area.push((line.last().unwrap().0, inner.bottom()));
    area.push((line[0].0, inner.bottom()));
    ui.c.fill_polygon(&area, ink.with_alpha(40));
    ui.c.polyline(&line, 1.5, ink);
    for (i, tag) in [(c.decay, "D"), (c.sustain, "S"), (c.loop_start, "L")] {
        if let Some(&t) = usize::try_from(i).ok().and_then(|i| ts.get(i)) {
            let x = t_to_x(t, span).round();
            ui.c.fill_rect(Rect::new(x, r.y + 1.0, 1.0, r.h - 2.0), plug::TICK);
            let tag_r = Rect::new(x - 5.0, r.bottom() - 14.0, 11.0, 11.0);
            ui.c.fill_rrect(tag_r, 3.0, EDGE);
            ui.c.text_center(SMALL.bold(), tag_r, TEXT, tag);
        }
    }
    ui.hit(r, Target::CurveArea);
    // Freeze editing / hide points.
    if ui.st.tools[0] {
        return;
    }
    for i in 1..n {
        if c.points[i].mode == 2 {
            continue;
        }
        let (x0, x1) = (t_to_x(ts[i - 1], span), t_to_x(ts[i], span));
        let (x, y) = ((x0 + x1) / 2.0, level_to_y(edit::segment_level(c, i, 0.5)));
        let t = Target::Tension(i);
        let hr = Rect::new(x - 4.0, y - 4.0, 8.0, 8.0);
        let hot = ui.hot(hr, &t);
        ui.c.stroke_circle(x, y, 3.0, 1.0, if hot { TEXT } else { ink });
        ui.hit(hr, t);
    }
    for (i, &t) in ts.iter().enumerate() {
        let (x, y) = (t_to_x(t, span), level_to_y(c.points[i].level));
        let tg = Target::Point(i);
        let hr = Rect::new(x - 5.0, y - 5.0, 10.0, 10.0);
        let hot = ui.hot(hr, &tg);
        let fill = if hot { brand::HIGHLIGHT } else if ui.st.selected.get(i) == Some(&true) { SELECTED } else { ink };
        let rad = if hot { 4.0 } else { 3.0 };
        ui.c.fill_circle(x, y, rad + 1.0, WELL);
        ui.c.fill_circle(x, y, rad, fill);
        ui.hit(hr, tg);
    }
    if let Some(b) = ui.st.band {
        ui.c.fill_rect(b, ACCENT.with_alpha(25));
        ui.c.stroke_rrect(b, 0.0, 1.0, ACCENT);
    }
}

/// Where point `i` of `c` sits in the curve view.
pub fn point_xy(c: &crate::curve::Curve, i: usize, span: f64) -> (f32, f32) {
    (t_to_x(edit::times(c)[i], span), level_to_y(c.points[i].level))
}

/// A selected point's fill (FL's green, measured).
const SELECTED: Color = Color::rgb(0xa8e44a);

/// The Level scaling dialog over the page (FL's at 187, 40, 172 x 277) and its controls' ranges and defaults.
pub const SCALE_DLG: Rect = Rect::new(187.0, 40.0, 172.0, 277.0);
pub const SCALE_RANGE: [(f32, f32); 4] = [(0.0, 1.0), (-2.0, 2.0), (-1.0, 1.0), (-1.0, 1.0)];
pub const SCALE_DEFAULT: [f32; 4] = [0.0, 1.0, 0.0, 0.0];
pub const SCALE_NAMES: [&str; 4] = ["Center", "Multiplier", "Offset", "Tension"];
const SCALE_BTNS: [&str; 3] = ["Reset", "Accept", "Close"];

/// Its controls' centres and its buttons' rects, in surface px.
pub fn scale_ctl(k: usize) -> Rect {
    let (dx, dy) = [(40.0, 135.0), (91.0, 98.0), (130.0, 135.0), (40.0, 190.0)][k];
    let (x, y) = (SCALE_DLG.x + dx, SCALE_DLG.y + dy);
    if k == 3 { Rect::new(x - 14.0, y - 14.0, 28.0, 28.0) } else { Rect::new(x - 10.0, y - 45.0, 20.0, 90.0) }
}

pub fn scale_btn(k: usize) -> Rect {
    let (dx, dy, w) = [(35.0, 255.0, 50.0), (135.0, 255.0, 50.0), (160.0, 12.0, 14.0)][k];
    Rect::new(SCALE_DLG.x + dx - w / 2.0, SCALE_DLG.y + dy - 8.0, w, if k == 2 { 14.0 } else { 16.0 })
}

/// The Level scaling dialog: a panel blocking the surface, three faders, a knob and its buttons.
fn scale_dialog(ui: &mut Ui, v: [f32; 4]) {
    ui.hit(Rect::new(0.0, 0.0, W as f32, H as f32), Target::Dialog);
    ui.c.fill_rect(Rect::new(0.0, 0.0, W as f32, H as f32), BG.with_alpha(110));
    panel(ui, SCALE_DLG);
    ui.c.text(LABEL.bold(), SCALE_DLG.x + 10.0, SCALE_DLG.y + 6.0, TEXT, "Level scaling");
    for k in 0..4 {
        let (lo, hi) = SCALE_RANGE[k];
        let n = (v[k] - lo) / (hi - lo);
        let r = scale_ctl(k);
        let t = Target::ScaleCtl(k);
        let hot = ui.hot(r, &t);
        if k == 3 {
            dial(ui, r.cx(), r.cy(), 12.0, n, true, ACCENT, hot);
            ui.label(SMALL, r.cx(), r.bottom() + 6.0, DIM, SCALE_NAMES[k]);
        } else {
            let (top, bot) = (r.y + 6.0, r.bottom() - 6.0);
            let y_of = |n: f32| bot - (bot - top) * n.clamp(0.0, 1.0);
            let zero = y_of(-lo / (hi - lo));
            let st = Look { hover: hot, pressed: ui.st.pressed.as_ref() == Some(&t), on: false, disabled: false };
            fader(ui, r.cx(), top, bot, y_of(n), zero, st);
            ui.label(SMALL, r.cx(), r.y - 6.0, DIM, SCALE_NAMES[k]);
        }
        ui.hit(r, t);
    }
    for (k, l) in SCALE_BTNS.into_iter().enumerate() {
        let r = scale_btn(k);
        let t = Target::ScaleBtn(k);
        let mut st = ui.state(r, &t);
        st.on = k == 1;
        let ink = lemon::plug_button(ui.c, r, 5.0, st);
        ui.c.text_center(SMALL.bold(), r, ink, if k == 2 { "x" } else { l });
        ui.hit(r, t);
    }
}

/// Harmonics shown at once (FL: 88) and the bars' area.
pub const HARM_VISIBLE: usize = 88;
pub fn harm_bars() -> Rect {
    Rect::span(CURVE.x + 4.0, CURVE.right() - 4.0, CURVE.y + 64.0, CURVE.bottom() - 2.0)
}

/// The harmonic under `x` (scrolled by `scroll`).
pub fn harm_at(x: f32, scroll: usize) -> usize {
    let b = harm_bars();
    let k = ((x - b.x) / (b.w / HARM_VISIBLE as f32)).floor().clamp(0.0, (HARM_VISIBLE - 1) as f32) as usize;
    (scroll + k).min(255)
}

/// The level a harmonic's bar shows at `y`.
pub fn harm_level(y: f32) -> f32 {
    let b = harm_bars();
    ((b.bottom() - y) / b.h).clamp(0.0, 1.0)
}

/// The harmonics' scroll bar.
pub const HARM_SCROLL: Rect = Rect::new(160.0, 312.0, 385.0, 12.0);

/// The scroll a click on the scroll bar at `x` gives.
pub fn harm_scroll_at(x: f32) -> usize {
    let n = 256 - HARM_VISIBLE;
    (((x - HARM_SCROLL.x) / HARM_SCROLL.w).clamp(0.0, 1.0) * n as f32).round() as usize
}

/// The OSC target: the cycle on top, the harmonics' levels below as bars.
fn harmonics(ui: &mut Ui, m: usize) {
    let r = CURVE;
    lemon::plug_well(ui.c, r, 6.0);
    let top = Rect::new(r.x, r.y, r.w, 60.0);
    ui.c.fill_rect(Rect::new(r.x + 1.0, top.bottom(), r.w - 2.0, 1.0), plug::GRID);
    if let Some(w) = ui.m.wave {
        let n = w.len().max(2);
        let pts: Vec<(f32, f32)> = w.iter().enumerate().map(|(i, &v)| (top.x + top.w * i as f32 / (n - 1) as f32, top.cy() - 26.0 * v.clamp(-1.0, 1.0))).collect();
        ui.c.polyline(&pts, 1.2, ACCENT);
    }
    let levels = &ui.m.st.modules[m].op.levels;
    let bars = harm_bars();
    let scroll = ui.st.harm_scroll.min(256 - HARM_VISIBLE);
    let bw = bars.w / HARM_VISIBLE as f32;
    let hot = ui.st.mouse.filter(|&(x, y)| bars.contains(x, y)).map(|(x, _)| harm_at(x, scroll));
    for k in 0..HARM_VISIBLE {
        let h = scroll + k;
        let l = levels.get(h).copied().unwrap_or(0.0);
        let x = bars.x + k as f32 * bw;
        let y = bars.bottom() - bars.h * l.clamp(0.0, 1.0);
        let fill = if hot == Some(h) { brand::HIGHLIGHT } else if l > 0.0 { ACCENT } else { plug::TICK };
        ui.c.fill_rect(Rect::span(x + 0.5, x + bw - 0.5, y.min(bars.bottom() - 1.0), bars.bottom()), fill);
        ui.hit(Rect::new(x, bars.y, bw, bars.h), Target::Harm(h));
    }
    panel(ui, BAR);
    // The scroll bar.
    let hover = ui.st.mouse.is_some_and(|(x, y)| HARM_SCROLL.contains(x, y)) || ui.st.pressed == Some(Target::HarmScroll);
    lemon::field(ui.c, HARM_SCROLL, Look::hover(hover));
    let tw = HARM_SCROLL.w * HARM_VISIBLE as f32 / 256.0;
    let tx = HARM_SCROLL.x + (HARM_SCROLL.w - tw) * scroll as f32 / (256 - HARM_VISIBLE) as f32;
    ui.c.fill_rrect(Rect::new(tx + 1.0, HARM_SCROLL.y + 2.0, tw - 2.0, HARM_SCROLL.h - 4.0), 3.0, if hover { Color::rgb(0x666666) } else { EDGE });
    ui.hit(HARM_SCROLL, Target::HarmScroll);
    options_arrow(ui, Target::HarmOptions);
    lamp(ui, Target::Inert("Smooth"), false, 190.0, 336.0, "SMOOTH", false);
}

/// The Options arrow under the view (167, 335).
fn options_arrow(ui: &mut Ui, t: Target) {
    let r = Rect::new(160.0, 328.0, 14.0, 14.0);
    let hot = ui.hot(r, &t);
    ui.c.fill_polygon(&[(163.0, 333.0), (171.0, 333.0), (167.0, 338.0)], if hot { TEXT } else { DIM });
    ui.hit(r, t);
}

pub const TOOL_X: [f32; 4] = [458.0, 482.0, 505.0, 529.0];
pub const TOOL_NAMES: [&str; 4] = ["Freeze editing / hide points", "Step editing", "Snap to grid", "Slide succeeding points / regions"];
const TOOL_LABELS: [&str; 4] = ["FRZ", "STP", "SNP", "SLD"];

/// The bar under the curve view: the part's switch, the envelope's or LFO's knobs and switches, the Options arrow
/// and the tools.
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
    tools(ui);
}

fn tools(ui: &mut Ui) {
    options_arrow(ui, Target::CurveOptions);
    for (k, x) in TOOL_X.into_iter().enumerate() {
        let r = Rect::new(x - 10.0, 327.0, 20.0, 16.0);
        let t = Target::Tool(k);
        let mut st = ui.state(r, &t);
        st.on = ui.st.tools[k];
        // Step editing does nothing yet.
        st.disabled = k == 1;
        let ink = lemon::plug_button(ui.c, r, 4.0, st);
        ui.c.text_center(SMALL, r, ink, TOOL_LABELS[k]);
        ui.hit(r, t);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Matrix (§6)

/// Cell (r, c)'s centre.
pub fn cell_centre(r: usize, c: usize) -> (f32, f32) {
    (GRID_X + PITCH * c as f32 + PITCH / 2.0, GRID_Y + PITCH * r as f32 + PITCH / 2.0)
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
        let r = Rect::new(x - PITCH / 2.0, GRID_Y - 15.0, PITCH, 14.0);
        let sel = label_page(false, c) == Some(ui.st.page);
        if sel {
            ui.c.fill_rrect(r.inset(1.0), 5.0, accent);
        }
        ui.c.text_center(SMALL.bold(), r, if sel { brand::COLORED_DARK } else { DIM }, s);
        if label_page(false, c).is_some() {
            ui.hit(r, Target::Col(c));
        }
    }
    for (row, s) in rows.into_iter().enumerate() {
        let (_, y) = cell_centre(row, 0);
        let r = Rect::new(GRID_X - 20.0, y - PITCH / 2.0, 18.0, PITCH);
        let sel = label_page(true, row) == Some(ui.st.page);
        if sel {
            ui.c.fill_rrect(r.inset(1.0), 5.0, accent);
        }
        ui.c.text_center(SMALL.bold(), r, if sel { brand::COLORED_DARK } else { DIM }, s);
        ui.hit(r, Target::Row(row));
    }
    for row in 0..9 {
        for c in 0..9 {
            let ring_cell = ring && row < OPERATORS && c < OPERATORS;
            let f = if ring_cell { Field::Ring(row, c) } else { Field::Cell(row, c) };
            let off = ui.m.st.cell_off[if ring_cell { 81 } else { 0 } + row * 9 + c] != 0;
            let (cx, cy) = cell_centre(row, c);
            let hr = Rect::new(cx - PITCH / 2.0, cy - PITCH / 2.0, PITCH, PITCH);
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
    let bar_y = GRID_Y + 9.0 * PITCH + 6.0;
    for (k, (x, s)) in [(GRID_X - 20.0, "FM"), (GRID_X + 28.0, "RM")].into_iter().enumerate() {
        let r = Rect::new(x, bar_y, 44.0, 18.0);
        let t = Target::MatrixTab(k == 1);
        let sel = ring == (k == 1);
        let mut st = ui.state(r, &t);
        st.on = sel;
        let ink = if sel && k == 1 {
            ui.c.fill_rrect(r, 5.0, RING);
            brand::COLORED_DARK
        } else {
            lemon::plug_button(ui.c, r, 5.0, st)
        };
        ui.c.text_center(SMALL.bold(), r, ink, s);
        ui.hit(r, t);
    }
    let f = Font::condensed(11, 0).bold();
    let right = GRID_X + 9.0 * PITCH;
    let y = (bar_y + 9.0 - ui.c.line_height(f) / 2.0).round();
    let wm = lemon::spaced_width(ui.c, f, "MATRIX", 2.0);
    lemon::spaced(ui.c, f, right - wm, y, DIM, "MATRIX", 2.0);
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
            let held = ui.st.key == Some(k) || ui.st.playing[usize::from(k / 64)] >> (k % 64) & 1 != 0;
            let fill = match (black, held) {
                (false, true) => ACCENT,
                (true, true) => brand::DEEP,
                (false, _) => Color::rgb(0xdcdcdc),
                (true, _) => Color::rgb(0x1f1f1f),
            };
            ui.c.fill_rrect(Rect::new(r.x + 0.5, r.y - 3.0, r.w - 1.0, r.h + 3.0), 3.0, fill);
            if k % 12 == 0 {
                ui.label(SMALL, r.cx(), r.bottom() - 9.0, Color::rgb(0x7a7a7a), &key_name(k));
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
    name(ui, mid, Font::condensed(32, 0).bold());
    let v = format!("Version {}", ui.m.version);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 8.0, w, 16.0), DIM, &v);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 26.0, w, 16.0), DIM, "Lemon Studio's six-operator synthesizer");
    ui.c.text_center(SMALL, Rect::new(0.0, h - 30.0, w, 16.0), FAINT, "Click to return");
    ui.hit(Rect::new(0.0, 0.0, w, h), Target::Cover);
}

/// A box of the diagram with its name; returns its rect.
fn block(ui: &mut Ui, x: f32, y: f32, w: f32, s: &str) -> Rect {
    let r = Rect::new(x, y, w, 24.0);
    lemon::outlined(ui.c, r, 6.0, plug::PANEL_LINE, PANEL);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch_text_rtf_shown_plain() {
        let rtf = "{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0\\fswiss Arial;}}{\\colortbl ;\\red255\\green0\\blue0;}\\f0\\fs20 Deep \\'e9t\\'e9\\par\\cf1 by \\{me\\}\\par}";
        assert_eq!(plain_text(rtf), "Deep \u{e9}t\u{e9}\nby {me}\n");
        assert_eq!(plain_text("Init"), "Init");
    }
}
