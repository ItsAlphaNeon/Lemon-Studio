// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Limiter editor: painting and hit regions (platform independent), in the Lemon "Pith" plugin look: every
//! control in the original's place with its hit area, drawn as outlined cards under ruled section headings, a dark
//! display well with a dB scale and the wordmark, round knobs with lemon value arcs. Coordinates are the spec's
//! editor-content pixels.
//!
// spec: plugins/lemon-limiter.md (§1 window, §2 controls, §3 display, §4 hints, "Look"), ui/lemon-design.md §11.1

use os_engine::effects::dynamics::{self as dsp, Column};
use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{self, eq as keq, inst, State};
use os_ui::{Canvas, Color, Font, Rect};

/// Editor size in logical pixels (the original's editor content).
pub const W: u32 = 626;
pub const H: u32 = 300;

/// The display (spec §1): level 0 at the bottom edge, the top edge = amplitude 4 (+12 dB).
pub const DISPLAY: Rect = Rect::span(30.0, 624.0, 1.0, 158.0);
/// The view strip's scrolling-speed fader track.
pub const SPEED_TOP: f32 = 87.0;
pub const SPEED_BOT: f32 = 152.0;

/// Display layers: input peaks, output peaks, analysis & gain envelopes, level markers.
pub const LAYERS: usize = 4;
pub const LAYER_Y: [f32; LAYERS] = [32.0, 47.0, 60.0, 73.0];
pub const LAYER_HINTS: [&str; LAYERS] = ["Show input peaks", "Show output peaks", "Show analysis & gain envelopes", "Show level markers"];

/// The spec's mode panel: the page's own controls.
const MODE_CARD: Rect = Rect::span(113.0, 460.0, 178.0, 297.0);
const LOUDNESS_CARD: Rect = Rect::span(3.0, 109.0, 178.0, 297.0);
const GATE_CARD: Rect = Rect::span(464.0, 623.0, 178.0, 297.0);

/// Display layer colours: soft pastels next to the lemon (as Lemon Parametric EQ's band colours) for the input peaks,
/// the envelopes and the compressor's threshold; the output peaks are lemon.
const LAVENDER: Color = Color::rgb(0xbcb4e8);
const SKY: Color = Color::rgb(0xa9cbe6);
/// The value arc of a compressor knob while the compressor is off (the Lemon design's disabled arc).
const IDLE_ARC: Color = Color::rgb(0x666666);
/// Section heading rules.
const RULE: Color = Color::rgb(0x3c3c3c);
/// The COMPARE label (as Lemon Parametric EQ's).
const LABEL_INK: Color = Color::rgb(0x8f8f8f);

const HEAD: Font = Font::new(11).bold();
const SMALL: Font = Font::new(11);
const HEADING: Font = Font::condensed(11, 0).bold();
const SCALE: Font = Font::new(9);

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
    /// Vertical drag / wheel; menu on right click.
    Knob(String),
    /// A stepped box (the curves): drag, wheel or its arrows.
    Spin(String),
    /// The sidechain number box.
    Sidechain,
    /// LIMIT / COMP.
    Page(u8),
    Layer(usize),
    Speed,
    /// The display menu (▼).
    Menu,
    Store,
    Flip,
    Display,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// Editor settings (kept in the state image's editor bytes) and the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    /// 0 LIMIT, 1 COMP.
    pub page: u8,
    pub layers: [bool; LAYERS],
    /// Scrolling speed 0..1.
    pub speed: f32,
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState { page: 0, layers: [true; LAYERS], speed: 0.5, mouse: None, pressed: None }
    }
}

/// What a paint shows besides the parameters.
pub struct Look<'a> {
    pub sidechain: u8,
    /// Display columns, oldest first.
    pub history: &'a [Column],
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
    fn state(&self, r: Rect, t: &Target) -> State {
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed, ..State::NORMAL }
    }
    fn centred(&mut self, f: Font, cx: f32, cy: f32, col: Color, s: &str) {
        let w = self.c.text_width(f, s);
        let lh = self.c.line_height(f);
        self.c.text(f, (cx - w / 2.0).round(), (cy - lh / 2.0).round(), col, s);
    }
}

/// Whether the compressor works (threshold below 0 dB).
pub fn comp_on(m: &dyn Model) -> bool {
    m.plain("c_thres") < 999.5
}

/// Height in the display (0 bottom, 1 top) of amplitude `a`: the inverse of `a = (3^(2h) − 1) / 2` (spec §3).
pub fn level_h(a: f32) -> f32 {
    ((2.0 * a.max(0.0) + 1.0).ln() / (2.0 * 3f32.ln())).clamp(0.0, 1.0)
}

/// Amplitude at display y.
pub fn display_amp(y: f32) -> f32 {
    let h = ((DISPLAY.bottom() - 1.0 - y) / (DISPLAY.h - 1.0)).clamp(0.0, 1.0);
    (9f32.powf(h) - 1.0) / 2.0
}

fn level_y(a: f32) -> f32 {
    DISPLAY.bottom() - 1.0 - level_h(a) * (DISPLAY.h - 1.0)
}

/// Paints the editor. `hint` (not hosted: the host shows it otherwise) goes in the display's top-left corner.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, look: &Look, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    view_strip(&mut ui);
    display(&mut ui, look);
    if !hint.is_empty() {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        let s = if right.is_empty() { left.to_string() } else { format!("{left}  ({right})") };
        ui.c.text_clip(SMALL, DISPLAY.x + 8.0, DISPLAY.y + 5.0, DISPLAY.w - 210.0, plug::DIM, &s);
    }
    headings(&mut ui);
    for r in [LOUDNESS_CARD, MODE_CARD, GATE_CARD] {
        lemon::plug_panel(ui.c, r);
    }
    // The mode card's loudness controls (CEIL; THRES, KNEE, RATIO) sit under LOUDNESS, its envelope under ENVELOPE.
    ui.c.fill_rect(Rect::span(283.0, 283.0, 190.0, 264.0), plug::PANEL_LINE);
    // Always shown (spec §2.1).
    knob(&mut ui, "gain", 30.0, 208.0, brand::MAIN, "GAIN");
    knob(&mut ui, "sat", 84.0, 208.0, brand::MAIN, "SAT");
    reduction_chip(&mut ui, look);
    knob(&mut ui, "g_rel", 487.0, 208.0, brand::MAIN, "REL");
    knob(&mut ui, "g_gain", 542.0, 208.0, brand::MAIN, "GAIN");
    knob(&mut ui, "g_thres", 597.0, 208.0, brand::MAIN, "THRES");
    compare_buttons(&mut ui);
    if st.page == 0 { limit_page(&mut ui) } else { comp_page(&mut ui, look) }
    page_tabs(&mut ui);
    ui.hits
}

/// LOUDNESS, ENVELOPE and NOISE GATE over their spans (spec §1): letter-spaced captions on a hairline rule.
fn headings(ui: &mut Ui) {
    let lh = ui.c.line_height(HEADING);
    for (x0, x1, s) in [(0.0, 283.0, "LOUDNESS"), (284.0, 461.0, "ENVELOPE"), (462.0, 625.0, "NOISE GATE")] {
        let w = lemon::spaced_width(ui.c, HEADING, s, 2.0);
        let tx = ((x0 + x1 + 1.0) / 2.0 - w / 2.0).round();
        lemon::spaced(ui.c, HEADING, tx, (168.0 - lh / 2.0).round(), plug::DIM, s, 2.0);
        ui.c.fill_rect(Rect::span(x0 + 8.0, tx - 9.0, 168.0, 168.0), RULE);
        ui.c.fill_rect(Rect::span((tx + w + 8.0).round(), x1 - 8.0, 168.0, 168.0), RULE);
    }
}

/// Display columns the reduction readout holds its largest value over (about a quarter second at the default speed).
const REDUCTION_HOLD: usize = 32;

/// The gain reduction of the last few display columns as a dark chip in the loudness card (Lemon Studio choice: a
/// readout, nothing to click). `--` while the display has no history.
fn reduction_chip(ui: &mut Ui, look: &Look) {
    let r = Rect::span(11.0, 101.0, 273.0, 291.0);
    lemon::outlined(ui.c, r, 6.0, plug::FIELD_LINE, plug::FIELD);
    let lh = ui.c.line_height(HEAD);
    let ty = (r.cy() - lh / 2.0).round();
    ui.c.text(HEAD, r.x + 8.0, ty, plug::DIM, "GR");
    let recent = &look.history[look.history.len().saturating_sub(REDUCTION_HOLD)..];
    let (s, col) = match recent.iter().map(|c| c.gain).reduce(f32::min) {
        None => ("--".to_string(), plug::FAINT),
        Some(g) if g >= 0.9988 || !g.is_finite() => ("0.0 dB".to_string(), plug::DIM),
        Some(g) if g <= 0.0 => ("-INF dB".to_string(), brand::HIGHLIGHT),
        Some(g) => (format!("{:.1} dB", 20.0 * g.log10()), brand::HIGHLIGHT),
    };
    ui.c.text_right(SMALL, r.right() - 8.0, ty, col, &s);
}

/// A knob (hit area 44 px across), caption under its centre; `arc` = the value arc's colour.
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, arc: Color, caption: &str) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - 22.0, cy - 22.0, 44.0, 44.0);
    let st = ui.state(hr, &target);
    let bipolar = matches!(id, "c_ratio" | "c_knee");
    lemon::dial(ui.c, cx, cy, 19.0, 2.8, ui.m.norm(id), bipolar, arc, st);
    inst::caption(ui.c, ui.t, cx, cy + 26.0, caption, false);
    ui.hit(hr, target);
}

/// A stepped box: the number and small up / down arrows at its right.
fn spinner(ui: &mut Ui, cx: f32, cy: f32, w: f32, text: &str, target: Target) {
    let r = Rect::new(cx - w / 2.0, cy - 9.0, w, 18.0);
    let st = ui.state(r, &target);
    lemon::field(ui.c, r, st);
    let ink = if st.hover || st.pressed { Color::rgb(0xffffff) } else { brand::TEXT };
    ui.centred(SMALL, cx - 3.0, cy, ink, text);
    let ax = r.right() - 5.5;
    let arrows = if st.hover || st.pressed { brand::MAIN } else { plug::DIM };
    ui.c.fill_polygon(&[(ax - 2.5, cy - 1.5), (ax, cy - 4.5), (ax + 2.5, cy - 1.5)], arrows);
    ui.c.fill_polygon(&[(ax - 2.5, cy + 1.5), (ax, cy + 4.5), (ax + 2.5, cy + 1.5)], arrows);
    ui.hit(r, target);
}

fn curve_spinner(ui: &mut Ui, id: &str, cx: f32) {
    let s = format!("{}", ui.m.plain(id).round() as i32);
    spinner(ui, cx, 282.0, 22.0, &s, Target::Spin(id.into()));
}

/// ATT / REL captions joined to the CURVE caption at `x0` by short lines (a bracket when they share one).
fn curve_caption(ui: &mut Ui, x0: f32, x1: f32) {
    let col = plug::TICK;
    let low = if x0 == x1 { 255.0 } else { 250.0 };
    for x in [x0, x1] {
        ui.c.fill_rect(Rect::span(x, x, 245.0, low), col);
    }
    if x0 != x1 {
        ui.c.fill_rect(Rect::span(x0, x1, low, low), col);
        ui.c.fill_rect(Rect::span(x0, x0, low, 255.0), col);
    }
    inst::caption(ui.c, ui.t, x0, 258.0, "CURVE", false);
}

fn limit_page(ui: &mut Ui) {
    knob(ui, "ceil", 144.0, 208.0, brand::MAIN, "CEIL");
    knob(ui, "att", 313.0, 206.0, brand::MAIN, "ATT");
    knob(ui, "rel", 369.0, 206.0, brand::MAIN, "REL");
    knob(ui, "sustain", 430.0, 206.0, brand::MAIN, "SUSTAIN");
    curve_caption(ui, 313.0, 313.0);
    curve_caption(ui, 369.0, 369.0);
    curve_spinner(ui, "att_curve", 313.0);
    curve_spinner(ui, "rel_curve", 369.0);
}

fn comp_page(ui: &mut Ui, look: &Look) {
    // The compressor's own knobs show their arcs in lemon while it works, grey otherwise (spec §2.3).
    let col = if comp_on(ui.m) { brand::MAIN } else { IDLE_ARC };
    knob(ui, "c_thres", 144.0, 208.0, col, "THRES");
    knob(ui, "c_knee", 199.0, 208.0, col, "KNEE");
    knob(ui, "c_ratio", 254.0, 208.0, col, "RATIO");
    knob(ui, "c_att", 313.0, 206.0, brand::MAIN, "ATT");
    knob(ui, "c_rel", 369.0, 206.0, brand::MAIN, "REL");
    knob(ui, "c_rms", 430.0, 206.0, brand::MAIN, "SUSTAIN");
    curve_caption(ui, 313.0, 369.0);
    curve_spinner(ui, "c_curve", 313.0);
    inst::caption(ui.c, ui.t, 369.0, 258.0, "SIDECHAIN", false);
    let s = if look.sidechain == 0 { "--".to_string() } else { look.sidechain.to_string() };
    spinner(ui, 369.0, 282.0, 34.0, &s, Target::Sidechain);
}

/// LIMIT / COMP: a button group in the mode card's bottom-left corner, the shown page lit.
fn page_tabs(ui: &mut Ui) {
    lemon::outlined(ui.c, Rect::span(118.0, 196.0, 274.0, 292.0), 6.0, plug::FIELD_LINE, plug::FIELD);
    let cells = [Rect::span(120.0, 155.0, 276.0, 290.0), Rect::span(157.0, 194.0, 276.0, 290.0)];
    for (i, (x0, x1, s)) in [(113.0, 152.0, "LIMIT"), (153.0, 196.0, "COMP")].into_iter().enumerate() {
        let r = Rect::span(x0, x1, 276.0, 295.0);
        let cell = cells[i];
        let target = Target::Page(i as u8);
        let st = ui.state(r, &target);
        let on = ui.st.page == i as u8;
        if on {
            lemon::lit(ui.c, cell, 4.5, st.hover);
        } else if st.hover {
            ui.c.fill_rrect(cell, 4.5, Color::rgb(0x363636));
        }
        let ink = if on { brand::COLORED_DARK } else if st.hover { brand::TEXT } else { plug::DIM };
        ui.centred(HEAD, cell.cx(), cell.cy(), ink, s);
        ui.hit(r, target);
    }
}

/// Store and Flip (A/B compare): round buttons as Lemon Parametric EQ's, labelled COMPARE at their left.
fn compare_buttons(ui: &mut Ui) {
    let lab = Font::condensed(11, 0).bold();
    let lh = ui.c.line_height(lab);
    ui.c.text_right(lab, 567.0, (282.0 - lh / 2.0).round(), LABEL_INK, "COMPARE");
    for (cx, target) in [(584.0, Target::Store), (612.0, Target::Flip)] {
        let cy = 282.0;
        let r = Rect::new(cx - 11.0, cy - 11.0, 22.0, 22.0);
        let st = ui.state(r, &target);
        keq::round_button(ui.c, cx, cy, 11.0, st);
        let ink = keq::ink(st);
        if target == Target::Store {
            kit::triangle(ui.c, cx, cy + 1.0, 6.0, kit::Dir::Down, ink);
        } else {
            kit::triangle(ui.c, cx, cy - 2.5, 5.0, kit::Dir::Up, ink);
            kit::triangle(ui.c, cx, cy + 3.0, 5.0, kit::Dir::Down, ink);
        }
        ui.hit(r, target);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// View strip and display (spec §3)

/// The display menu, the four layer switches and the scrolling-speed fader on an outlined column.
fn view_strip(ui: &mut Ui) {
    lemon::outlined(ui.c, Rect::span(2.0, 26.0, 1.0, 158.0), 6.0, plug::PANEL_LINE, plug::PANEL);
    let cx = 14.0;
    let r = Rect::new(cx - 9.0, 8.0, 18.0, 16.0);
    let st = ui.state(r, &Target::Menu);
    if st.hover || st.pressed {
        ui.c.fill_rrect(r.inset(1.0), 4.0, Color::rgb(0x3d3d3d));
    }
    let ink = if st.hover || st.pressed { brand::MAIN } else { plug::DIM };
    kit::triangle(ui.c, cx, 16.0, 5.0, kit::Dir::Down, ink);
    ui.hit(r, Target::Menu);
    for (k, &y) in LAYER_Y.iter().enumerate() {
        let r = Rect::new(cx - 9.0, y - 6.5, 18.0, 13.0);
        let target = Target::Layer(k);
        let st = ui.state(r, &target);
        let cell = Rect::new(cx - 8.0, y - 5.5, 16.0, 11.0);
        let col = lemon::choice_button(ui.c, cell, ui.st.layers[k], layer_colour(k), st);
        layer_icon(ui.c, k, cx, y, col);
        ui.hit(r, target);
    }
    // Scrolling speed: ticks, a dark groove and a rounded grey cap with a lemon line.
    let r = Rect::new(cx - 9.0, SPEED_TOP - 6.0, 18.0, SPEED_BOT - SPEED_TOP + 12.0);
    let st = ui.state(r, &Target::Speed);
    for k in 0..5 {
        let yy = (SPEED_TOP + (SPEED_BOT - SPEED_TOP) * k as f32 / 4.0).round();
        ui.c.fill_rect(Rect::new(cx - 9.0, yy, 3.0, 1.0), plug::TICK);
        ui.c.fill_rect(Rect::new(cx + 6.0, yy, 3.0, 1.0), plug::TICK);
    }
    ui.c.fill_rrect(Rect::new(cx - 2.0, SPEED_TOP - 1.0, 4.0, SPEED_BOT - SPEED_TOP + 2.0), 2.0, Color::rgb(0x1a1a1a));
    let hy = (SPEED_BOT - (SPEED_BOT - SPEED_TOP) * ui.st.speed).round();
    ui.c.fill_rrect(Rect::new(cx - 8.0, hy - 5.0, 16.0, 10.0), 3.5, Color::rgb(0x151515));
    let (a, b) = if st.hover || st.pressed { (0x7a7a7a, 0x555555) } else { (0x6c6c6c, 0x4b4b4b) };
    ui.c.fill_rect_gradient(Rect::new(cx - 7.0, hy - 4.0, 14.0, 8.0), Color::rgb(a), Color::rgb(b), 2.5);
    ui.c.fill_rrect(Rect::new(cx - 5.0, hy - 0.75, 10.0, 1.5), 0.75, brand::MAIN);
    ui.hit(r, Target::Speed);
}

/// Layer `k`'s colour in the display and on its switch.
fn layer_colour(k: usize) -> Color {
    match k {
        0 => LAVENDER,
        1 => brand::MAIN,
        2 => SKY,
        _ => brand::TEXT,
    }
}

/// The switches' marks: a bar (input peaks), a ring (output peaks), a slash (envelopes), two lines (level markers).
fn layer_icon(c: &mut Canvas, k: usize, cx: f32, y: f32, col: Color) {
    match k {
        0 => c.fill_rrect(Rect::new(cx - 1.25, y - 4.5, 2.5, 9.0), 1.0, col),
        1 => c.stroke_circle(cx, y, 3.6, 1.6, col),
        2 => c.line(cx - 3.5, y + 3.5, cx + 3.5, y - 3.5, 1.6, col),
        _ => {
            c.fill_rrect(Rect::new(cx - 4.5, y - 2.5, 9.0, 1.6), 0.8, col);
            c.fill_rrect(Rect::new(cx - 4.5, y + 1.0, 9.0, 1.6), 0.8, col);
        }
    }
}

/// The history in a dark well: a dB scale, the wordmark behind the levels (where the original has its watermark),
/// the input peaks in lavender with the output peaks in lemon over them (the lavender left above the lemon is what
/// the plugin takes off), the envelopes and the level markers.
fn display(ui: &mut Ui, look: &Look) {
    let d = DISPLAY;
    lemon::plug_well(ui.c, d, 6.0);
    ui.c.push_clip(d.inset(1.0));
    for db in [6.0f32, 0.0, -6.0, -12.0] {
        let y = level_y(10f32.powf(db / 20.0)).round();
        ui.c.fill_rect(Rect::new(d.x + 1.0, y, d.w - 2.0, 1.0), if db == 0.0 { Color::rgb(0x2e2e2e) } else { plug::GRID });
    }
    wordmark(ui.c, d.right() - 12.0, d.y + 17.0);
    let [show_in, show_out, show_env, show_marks] = ui.st.layers;
    let cols = look.history;
    let n = cols.len().min(d.w as usize);
    let x_of = |i: usize| d.right() - (n - i) as f32;
    let bot = d.bottom();
    for (i, col) in cols[cols.len() - n..].iter().enumerate() {
        let x = x_of(i);
        if show_in {
            let y = level_y(col.input);
            ui.c.fill_rect(Rect::new(x, y, 1.0, bot - y), LAVENDER.with_alpha(150));
        }
        if show_out {
            let y = level_y(col.output);
            ui.c.fill_rect(Rect::new(x, y, 1.0, bot - y), brand::MAIN.with_alpha(220));
        }
    }
    if show_env && n > 1 {
        let start = cols.len() - n;
        let level: Vec<(f32, f32)> = (0..n).map(|i| (x_of(i), level_y(cols[start + i].level))).collect();
        let gain: Vec<(f32, f32)> = (0..n).map(|i| (x_of(i), level_y(cols[start + i].gain.min(4.0)))).collect();
        ui.c.polyline(&level, 1.2, SKY);
        ui.c.polyline(&gain, 1.2, brand::HIGHLIGHT.with_alpha(210));
    }
    if show_marks {
        let ceil = dsp::level(ui.m.plain("ceil").round() as i32, 3.0);
        ui.c.fill_rect(Rect::new(d.x, level_y(ceil).round() - 1.0, d.w, 2.0), brand::TEXT.with_alpha(190));
        if comp_on(ui.m) {
            let thr = dsp::level(ui.m.plain("c_thres").round() as i32, 7.0);
            ui.c.fill_rect(Rect::new(d.x, level_y(thr).round(), d.w, 1.0), SKY);
        }
    }
    // The scale's labels, over the levels so they stay readable.
    for (db, s) in [(6.0f32, "+6"), (0.0, "0"), (-6.0, "-6"), (-12.0, "-12")] {
        let y = level_y(10f32.powf(db / 20.0)).round();
        ui.c.text(SCALE, d.x + 6.0, y - 11.0, plug::FAINT, s);
    }
    ui.c.pop_clip();
    ui.hit(d, Target::Display);
}

/// The lemon mark and "LEMON" in lemon beside a large faint "LIMITER", right-aligned at `right` and centred on `cy`
/// (as Lemon Sampler's in its waveform well).
fn wordmark(c: &mut Canvas, right: f32, cy: f32) {
    let (big, small) = (Font::condensed(22, 0).bold(), Font::condensed(10, 0).bold());
    let wb = lemon::spaced_width(c, big, "LIMITER", 2.0);
    let ws = lemon::spaced_width(c, small, "LEMON", 2.5);
    let (lh_b, lh_s) = (c.line_height(big), c.line_height(small));
    let bx = right - wb;
    lemon::spaced(c, big, bx, (cy - lh_b / 2.0).round(), Color::rgb(0xffffff).with_alpha(82), "LIMITER", 2.0);
    let sx = bx - 8.0 - ws;
    lemon::spaced(c, small, sx, (cy - lh_s / 2.0).round() + 1.0, brand::MAIN, "LEMON", 2.5);
    lemon::lemon_mark(c, sx - 8.0, cy, 4.0, brand::MAIN);
}

// ---------------------------------------------------------------------------------------------------------------
// Hints (spec §4)

/// The hint line for a target (a tab before the right-aligned part).
pub fn hint(t: &Target, m: &dyn Model, names: &dyn Fn(&str) -> String, mouse: Option<(f32, f32)>) -> String {
    match t {
        Target::Knob(id) | Target::Spin(id) => {
            let s = format!("{}: {}", names(id), m.text(id));
            if id == "att" { format!("{s}\tupdated on release") } else { s }
        }
        Target::Sidechain => "Sidechain input number".into(),
        Target::Page(_) => String::new(),
        Target::Layer(k) => LAYER_HINTS[(*k).min(LAYERS - 1)].into(),
        Target::Speed => "Scrolling speed".into(),
        Target::Menu => "Display options".into(),
        Target::Store => "Store in spare state".into(),
        Target::Flip => "Flip with spare state (A/B compare)".into(),
        Target::Display => {
            let a = mouse.map_or(0.0, |(_, y)| display_amp(y));
            if a <= 0.0 { "-INFdB 0.00".into() } else { format!("{:.1}dB {:.2}", 20.0 * a.log10(), a) }
        }
    }
}
