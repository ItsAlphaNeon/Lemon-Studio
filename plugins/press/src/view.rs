// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Press editor: painting and hit regions (platform independent). Maximus's arrangement on a 6 px grid in the Lemon
//! "Pith" plugin look: the header band with the emblem and the A / B buttons, the band's map and the monitor over
//! their tool bars, then the band tabs, the band's gains and saturation, its envelope, the LMH look-ahead and mix and
//! the crossover.
//!
// spec: plugins/lemon-press.md §2, ui/lemon-design.md §11.1

use os_engine::effects::maximus::state::*;
use os_engine::effects::maximus::{crossover_hz, map};
use os_theme::design::lemon as brand;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::State as Look;
use os_ui::{Canvas, Color, Font, Rect};

use crate::ctl;
use crate::edit;

/// Editor size in logical pixels.
pub const W: u32 = 640;
pub const H: u32 = 460;

pub const BG: Color = plug::BG;
const WELL: Color = plug::WELL;
pub const TEXT: Color = brand::TEXT;
const DIM: Color = plug::DIM;
const FAINT: Color = plug::FAINT;
const OFF: Color = plug::OFF;
pub const ACCENT: Color = brand::MAIN;

const LABEL: Font = Font::new(10);
const SMALL: Font = Font::new(9);

/// The bands' colours (FL's, measured from its band tabs): LOW, MID, HIGH, MASTER.
pub const BAND_COLORS: [Color; BANDS] = [Color::rgb(0xbf5b56), Color::rgb(0xc09354), Color::rgb(0xb8c354), Color::rgb(0x80c354)];
pub const BAND_NAMES: [&str; BANDS] = ["LOW", "MID", "HIGH", "MASTER"];

/// The layout's grid: every card 6 px from the editor's edges and from its neighbours.
const GAP: f32 = 6.0;
/// A rect from its left, right, top and bottom edges (right and bottom exclusive), so that cards 6 px apart leave 6
/// clear pixels between them.
const fn edges(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}
const RIGHT: f32 = W as f32 - GAP;
const BOTTOM: f32 = H as f32 - GAP;
/// The header band: the emblem on the left, the header buttons on the right, on one centre line.
const HEAD: (f32, f32) = (GAP, 40.0);
const HEADER_Y: f32 = (HEAD.0 + HEAD.1) / 2.0;
pub const HEADER_X: [f32; 4] = [548.0, 573.0, 598.0, 623.0];
pub const LOGO: Rect = edges(GAP, 200.0, HEAD.0, HEAD.1);
/// The map and the monitor, their tool bars under them.
const TOP: (f32, f32) = (HEAD.1 + GAP, 232.0);
pub const CURVE: Rect = edges(GAP, 192.0, TOP.0, TOP.1);
pub const DISPLAY: Rect = edges(198.0, RIGHT, TOP.0, TOP.1);
const BARS: (f32, f32) = (TOP.1 + GAP, 262.0);
pub const CURVE_BAR: Rect = edges(GAP, 192.0, BARS.0, BARS.1);
pub const DISPLAY_BAR: Rect = edges(198.0, RIGHT, BARS.0, BARS.1);
/// The cards under the bars.
const ROW: (f32, f32) = (BARS.1 + GAP, BOTTOM);
pub const TABS_CARD: Rect = edges(GAP, 78.0, ROW.0, 384.0);
pub const SOLO_CARD: Rect = edges(GAP, 78.0, 384.0 + GAP, ROW.1);
pub const BAND_CARD: Rect = edges(84.0, 258.0, ROW.0, ROW.1);
pub const ENV_CARD: Rect = edges(264.0, 432.0, ROW.0, ROW.1);
pub const LMH_CARD: Rect = edges(438.0, 498.0, ROW.0, ROW.1);
pub const XOVER_CARD: Rect = edges(504.0, RIGHT, ROW.0, ROW.1);
/// A titled card's header band (its title centred, a line under it) and where captions sit: 10 px over a card's foot.
const BAND: f32 = 15.0;
const FOOT: f32 = 10.0;
/// The cards' rows: the upper knobs with their captions, the middle row (boxes and switches), the lower knobs with
/// their captions at the foot.
const UPPER: (f32, f32) = (312.0, 340.0);
const MIDDLE: f32 = 368.0;
const LOWER: (f32, f32) = (ROW.1 - FOOT - 28.0, ROW.1 - FOOT);
const KNOB_R: f32 = 17.5;

/// The band tabs, stacked in their card.
pub fn band_tab(b: usize) -> Rect {
    let r = TABS_CARD;
    Rect::new(r.x + 4.0, r.y + 4.0 + 28.0 * b as f32, r.w - 8.0, 24.0)
}
/// The mode radios: ON, COMP OFF, MUTED, OFF, stacked at the band card's left.
pub const MODE_X: f32 = 97.0;
pub const MODE_Y: [f32; 4] = [292.0, 308.0, 324.0, 340.0];
const MODE_LABELS: [&str; 4] = ["ON", "COMP OFF", "MUTED", "OFF"];
/// The band card's columns: the mode list or the separation, PRE / THRES, POST / CEIL.
const BAND_X: [f32; 3] = [118.0, 180.0, 228.0];
/// The envelope card's columns: ATT and REL 2, REL, SUSTAIN.
const ENV_X: [f32; 3] = [292.0, 348.0, 404.0];
/// The crossover's columns: LOW, LIN, HIGH.
const XOVER_X: [f32; 3] = [540.0, 569.0, 598.0];
/// The curve spinners and the slope boxes.
pub fn spinner(x: f32) -> Rect {
    edges(x - 14.0, x + 14.0, MIDDLE - 9.0, MIDDLE + 9.0)
}
pub fn slope_box(x: f32) -> Rect {
    edges(x - 16.0, x + 16.0, MIDDLE - 8.0, MIDDLE + 8.0)
}
/// The curve tools (freeze, snap) from the curve bar's right margin leftwards.
pub const TOOL_X: [f32; 2] = [148.0, 175.0];
pub const TOOL_NAMES: [&str; 2] = ["Freeze editing / hide points", "Snap to grid"];
const TOOL_LABELS: [&str; 2] = ["FRZ", "SNP"];
/// The display's switches (level, gain envelope, every band's gain envelope) and its MONITOR / BANDS tabs.
pub const SHOW_X: [f32; 3] = [224.0, 268.0, 312.0];
pub const SHOW_NAMES: [&str; 3] = ["Show band level", "Show band gain envelope", "Show all gain envelopes"];
const SHOW_LABELS: [&str; 3] = ["LEVEL", "GAIN", "ALL"];
pub fn display_tab(i: usize) -> Rect {
    let r = DISPLAY_BAR;
    let w = 62.0;
    Rect::new(r.right() - 4.0 - w * (2 - i) as f32, r.y + 3.0, w - 1.0, r.h - 6.0)
}
pub const DISPLAY_NAMES: [&str; 2] = ["MONITOR", "BANDS"];

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A knob of parameter `k`: vertical drag, wheel, menu on right click.
    Knob(usize),
    /// A curve spinner of parameter `k`: 8 px of drag or a wheel notch per step.
    Steps(usize),
    /// A mode radio of the shown band.
    Mode(i32),
    /// A slope box (parameter `k`): a click flips 12 / 24 dB.
    Slope(usize),
    /// The PEAK (false) or RMS (true) radio.
    Rms(bool),
    /// Version 14's linear-phase switch: greyed out, it shows its hint.
    Lin,
    Solo,
    BandTab(usize),
    /// The map view: a point (drag, its menu on right click), a segment's tension handle (drag), empty space (a right
    /// click adds a point).
    Point(usize),
    Tension(usize),
    CurveArea,
    CurveOptions,
    Tool(usize),
    /// The bands display: a crossover edge (0 low, 1 high) drags; a band's region chooses it.
    Edge(usize),
    Region(usize),
    DisplayArea,
    DisplayTab(usize),
    Show(usize),
    /// Store in spare state, flip with spare state, Options, info.
    Header(usize),
    Logo,
    /// The info page: a click returns.
    Cover,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// What the editor shows besides the state.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
    /// The display: 0 monitor, 1 bands.
    pub display: usize,
    /// The monitor's switches: level, gain envelope, every band's gain envelope.
    pub show: [bool; 3],
    /// The curve tools that are on (freeze, snap).
    pub tools: [bool; 2],
    pub info: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState { mouse: None, pressed: None, display: 0, show: [true, true, false], tools: [false; 2], info: false }
    }
}

/// What a paint shows.
pub struct Model<'a> {
    pub st: &'a State,
    /// Each band's detector level and gain, oldest first; the last entry is the sound's latest.
    pub history: &'a [[[f32; 2]; BANDS]],
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
    fn state(&self, r: Rect, t: &Target) -> Look {
        Look { hover: self.hot(r, t), pressed: self.st.pressed.as_ref() == Some(t), on: false, disabled: false }
    }
    fn label(&mut self, f: Font, cx: f32, cy: f32, col: Color, s: &str) {
        self.c.text_center(f, Rect::new(cx - 60.0, cy - 8.0, 120.0, 16.0), col, s);
    }
    fn shown(&self) -> usize {
        self.m.st.shown().min(BANDS - 1)
    }
    fn latest(&self) -> [[f32; 2]; BANDS] {
        self.m.history.last().copied().unwrap_or([[0.0, 1.0]; BANDS])
    }
}

/// Paints the editor. `hint` (not hosted: the host shows it otherwise) is drawn over the display's top.
pub fn paint(c: &mut Canvas, m: &Model, st: &ViewState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(BG);
    if st.info {
        info(&mut ui);
        return ui.hits;
    }
    header(&mut ui);
    curve_view(&mut ui);
    display(&mut ui, hint);
    bars(&mut ui);
    band_tabs(&mut ui);
    band_card(&mut ui);
    env_card(&mut ui);
    lmh_card(&mut ui);
    xover_card(&mut ui);
    ui.hits
}

// ---------------------------------------------------------------------------------------------------------------
// Widgets

fn panel(ui: &mut Ui, r: Rect) {
    lemon::plug_panel(ui.c, r);
}

/// A card with its title centred in a header band and a line under the band.
fn titled(ui: &mut Ui, r: Rect, title: &str, ink: Color) {
    panel(ui, r);
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + BAND, r.w - 2.0, 1.0), plug::PANEL_LINE);
    ui.label(SMALL.bold(), r.cx(), r.y + BAND / 2.0, ink, title);
}

/// The lemon mark and "PRESS" in the wordmark's grey condensed caps (the citrus line's emblem), from `x` or centred
/// on it, on the line `cy`; `s` = the lemon's half length. Returns its rect.
fn emblem(ui: &mut Ui, x: f32, cy: f32, centred: bool, f: Font, s: f32) -> Rect {
    let wn = lemon::spaced_width(ui.c, f, "PRESS", 3.0);
    // The tilted lemon's width with its nubs, and the space before the name.
    let (wl, gap) = (2.2 * s, (s * 1.3).round());
    let w = wl + gap + wn;
    let x = if centred { (x - w / 2.0).round() } else { x };
    let lh = ui.c.line_height(f);
    lemon::lemon_mark(ui.c, x + wl / 2.0, cy, s, brand::MAIN);
    lemon::spaced(ui.c, f, x + wl + gap, (cy - lh / 2.0).round(), plug::WORDMARK, "PRESS", 3.0);
    Rect::new(x, cy - lh / 2.0, w, lh)
}

/// A knob of parameter `k`, radius `r`, its caption's centre at `cap_y`.
fn knob(ui: &mut Ui, k: usize, cx: f32, cy: f32, label: &str, cap_y: f32) {
    let r = KNOB_R;
    let hr = Rect::new(cx - r - 2.0, cy - r - 2.0, 2.0 * r + 4.0, 2.0 * r + 4.0);
    let w = (r * 0.2).clamp(1.8, 3.2);
    let t = Target::Knob(k);
    let look = Look::hover(ui.hot(hr, &t));
    let v = ctl::shown(ui.m.st, k);
    // The low cut's "off" sits at the knob's start, greyed.
    let off = k == P_LOW_CUT && v < 0;
    lemon::dial(ui.c, cx, cy, r, w, ctl::norm(k, v), ctl::bipolar(k), if off { OFF } else { ACCENT }, look);
    ui.label(SMALL, cx, cap_y, DIM, label);
    ui.hit(hr, t);
}

/// A switch at (cx, cy): the kit's rounded-square toggle, its label to the right.
fn lamp(ui: &mut Ui, t: Target, on: bool, cx: f32, cy: f32, label: &str) {
    let w = ui.c.text_width(SMALL, label);
    let hr = Rect::new(cx - 7.0, cy - 7.0, w + 18.0, 14.0);
    let st = ui.state(hr, &t);
    lemon::toggle(ui.c, cx, cy, on, st);
    let lh = ui.c.line_height(SMALL);
    ui.c.text(SMALL, cx + 10.0, (cy - lh / 2.0).round(), if on { TEXT } else { DIM }, label);
    ui.hit(hr, t);
}

/// A box showing a value: a spinner's number or a slope, a small arrow on its right for spinners.
fn value_box(ui: &mut Ui, r: Rect, t: Target, text: &str, arrow: bool) {
    let st = ui.state(r, &t);
    lemon::field(ui.c, r, st);
    let tr = if arrow { Rect::new(r.x, r.y, r.w - 7.0, r.h) } else { r };
    ui.c.text_center(SMALL.bold(), tr, if st.hover { TEXT } else { ACCENT }, text);
    if arrow {
        let (x, y) = (r.right() - 7.0, r.cy());
        ui.c.fill_polygon(&[(x - 2.5, y - 1.5), (x + 2.5, y - 1.5), (x, y + 1.5)], DIM);
    }
    ui.hit(r, t);
}

/// A tab: the chosen one a dark rounded pill with its name in lemon (Pith's sets of choices).
fn tab(ui: &mut Ui, r: Rect, t: Target, label: &str, sel: bool, ink: Option<Color>) {
    let hot = ui.hot(r, &t);
    let pill = r.inset(0.5);
    if sel {
        lemon::outlined(ui.c, pill, 5.0, plug::FIELD_LINE, plug::FIELD);
    } else if hot {
        ui.c.fill_rrect(pill, 5.0, Color::rgb(0x333333));
    }
    let col = if sel { ACCENT } else if hot { TEXT } else { ink.unwrap_or(DIM) };
    ui.c.text_center(SMALL.bold(), r, col, label);
    ui.hit(r, t);
}

/// A small push button of a tool bar (24 x 18), lit when on.
fn tool_button(ui: &mut Ui, cx: f32, cy: f32, w: f32, t: Target, on: bool, label: &str) {
    let r = Rect::new(cx - w / 2.0, cy - 9.0, w, 18.0);
    let mut st = ui.state(r, &t);
    st.on = on;
    let ink = lemon::plug_button(ui.c, r, 4.0, st);
    ui.c.text_center(SMALL, r, ink, label);
    ui.hit(r, t);
}

/// An Options arrow at the left of a bar.
fn options_arrow(ui: &mut Ui, bar: Rect, t: Target) {
    let cy = bar.cy().round();
    let r = Rect::new(bar.x + 5.0, cy - 8.0, 16.0, 16.0);
    let hot = ui.hot(r, &t);
    let x = r.cx();
    ui.c.fill_polygon(&[(x - 4.5, cy - 2.0), (x + 4.5, cy - 2.0), (x, cy + 3.0)], if hot { TEXT } else { DIM });
    ui.hit(r, t);
}

// ---------------------------------------------------------------------------------------------------------------
// The header

fn header(ui: &mut Ui) {
    emblem(ui, LOGO.x + 3.0, HEADER_Y, false, Font::condensed(18, 0).bold(), 6.0);
    ui.hit(LOGO, Target::Logo);
    for (i, x) in HEADER_X.into_iter().enumerate() {
        let (y, rad) = (HEADER_Y, 11.0);
        let t = Target::Header(i);
        let r = Rect::new(x - rad, y - rad, 2.0 * rad, 2.0 * rad);
        let st = ui.state(r, &t);
        let ink = lemon::plug_button(ui.c, r, 11.0, st);
        match i {
            // Store: an arrow down onto a line.
            0 => {
                ui.c.fill_rect(Rect::new(x - 0.75, y - 5.0, 1.5, 6.0), ink);
                ui.c.fill_polygon(&[(x - 3.5, y), (x + 3.5, y), (x, y + 3.5)], ink);
                ui.c.fill_rect(Rect::new(x - 4.5, y + 4.5, 9.0, 1.5), ink);
            }
            1 => ui.c.text_center(Font::new(8).bold(), r, ink, "A/B"),
            2 => ui.c.fill_polygon(&[(x - 4.5, y - 2.0), (x + 4.5, y - 2.0), (x, y + 3.0)], ink),
            _ => ui.c.text_center(Font::new(13).bold(), r, ink, "?"),
        }
        ui.hit(r, t);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The map view

/// Where the map view draws its map: 6 px in from every edge, so a hovered point (5 px with its rim) stays inside the
/// well.
pub fn curve_inner() -> Rect {
    CURVE.inset(6.0)
}

pub fn place_to_x(p: f64) -> f32 {
    let r = curve_inner();
    r.x + r.w * p.clamp(0.0, 1.0) as f32
}

pub fn x_to_place(x: f32) -> f64 {
    let r = curve_inner();
    f64::from((x - r.x) / r.w).clamp(0.0, 1.0)
}

pub fn level_to_y(l: f64) -> f32 {
    let r = curve_inner();
    r.bottom() - r.h * l.clamp(0.0, 1.0) as f32
}

pub fn y_to_level(y: f32) -> f64 {
    let r = curve_inner();
    f64::from((r.bottom() - y) / r.h).clamp(0.0, 1.0)
}

/// The map's scale at `db` decibels.
fn db_place(db: f64) -> f64 {
    edit::level_to_scale(10f64.powf(db / 20.0))
}

/// The grid of both axes, in dB.
const GRID_DB: [f64; 8] = [-18.0, -12.0, -9.0, -6.0, -3.0, 3.0, 6.0, 9.0];

/// Point `i` of the map and the tension handle of the segment ending on it (place, level).
pub fn map_points(m: &Map) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
    let pts = m.places();
    let mids: Vec<f64> = (1..pts.len()).map(|i| (pts[i - 1].0 + pts[i].0) / 2.0).collect();
    let lv = map::levels(m, &mids);
    let handles = std::iter::once((0.0, 0.0)).chain(mids.into_iter().zip(lv)).collect();
    (pts, handles)
}

fn curve_view(ui: &mut Ui) {
    let r = CURVE;
    lemon::plug_well(ui.c, r, 6.0);
    let inner = curve_inner();
    for db in GRID_DB {
        let p = db_place(db);
        let x = place_to_x(p).round();
        let y = level_to_y(p).round();
        ui.c.fill_rect(Rect::new(x, r.y + 1.0, 1.0, r.h - 2.0), plug::GRID);
        ui.c.fill_rect(Rect::new(r.x + 1.0, y, r.w - 2.0, 1.0), plug::GRID);
    }
    let (x0, y0) = (place_to_x(0.5).round(), level_to_y(0.5).round());
    ui.c.fill_rect(Rect::new(x0, r.y + 1.0, 1.0, r.h - 2.0), plug::TICK);
    ui.c.fill_rect(Rect::new(r.x + 1.0, y0, r.w - 2.0, 1.0), plug::TICK);
    ui.c.text(SMALL, r.x + 7.0, r.y + 5.0, FAINT, "OUT");
    ui.c.text_right(SMALL, r.right() - 7.0, r.bottom() - 16.0, FAINT, "IN");
    let b = ui.shown();
    let m = ui.m.st.map(b).clone();
    let ink = BAND_COLORS[b];
    // The map, sampled every pixel.
    let n = inner.w.round().max(1.0) as usize;
    let xs: Vec<f64> = (0..=n).map(|i| i as f64 / n as f64).collect();
    let line: Vec<(f32, f32)> = xs.iter().zip(map::levels(&m, &xs)).map(|(&x, y)| (place_to_x(x), level_to_y(y))).collect();
    let mut area = line.clone();
    area.push((inner.right(), inner.bottom()));
    area.push((inner.x, inner.bottom()));
    ui.c.fill_polygon(&area, ink.with_alpha(36));
    ui.c.polyline(&line, 1.5, ink);
    // The band's level now: where it enters the map and what comes out.
    let [v, g] = ui.latest()[b];
    if v > 1e-4 {
        let (px, py) = (edit::level_to_scale(f64::from(v)), edit::level_to_scale(f64::from(v * g)));
        let (x, y) = (place_to_x(px).round(), level_to_y(py).round());
        ui.c.fill_rect(Rect::new(x, y, 1.0, inner.bottom() - y), brand::HIGHLIGHT.with_alpha(120));
        ui.c.fill_circle(x, y, 2.5, brand::HIGHLIGHT);
    }
    ui.hit(r, Target::CurveArea);
    if ui.st.tools[0] {
        return;
    }
    let (pts, handles) = map_points(&m);
    for (i, &(x, y)) in handles.iter().enumerate().skip(1) {
        if m.points[i].mode == 2 {
            continue;
        }
        let (x, y) = (place_to_x(x), level_to_y(y));
        let t = Target::Tension(i);
        let hr = Rect::new(x - 4.0, y - 4.0, 8.0, 8.0);
        let hot = ui.hot(hr, &t);
        ui.c.stroke_circle(x, y, 3.0, 1.0, if hot { TEXT } else { ink });
        ui.hit(hr, t);
    }
    for (i, &(x, y)) in pts.iter().enumerate() {
        let (x, y) = (place_to_x(x), level_to_y(y));
        let tg = Target::Point(i);
        let hr = Rect::new(x - 5.0, y - 5.0, 10.0, 10.0);
        let hot = ui.hot(hr, &tg);
        let rad = if hot { 4.0 } else { 3.0 };
        ui.c.fill_circle(x, y, rad + 1.0, WELL);
        ui.c.fill_circle(x, y, rad, if hot { brand::HIGHLIGHT } else { ink });
        ui.hit(hr, tg);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The display

/// The display's dB axis: +12 dB at the top, -24 dB at the bottom.
const DISPLAY_DB: (f32, f32) = (12.0, -24.0);
const DISPLAY_GRID: [f32; 8] = [9.0, 6.0, 3.0, 0.0, -3.0, -6.0, -12.0, -18.0];

fn display_inner() -> Rect {
    DISPLAY.inset(6.0)
}

fn db_y(db: f32) -> f32 {
    let r = display_inner();
    let n = ((DISPLAY_DB.0 - db) / (DISPLAY_DB.0 - DISPLAY_DB.1)).clamp(0.0, 1.0);
    r.y + r.h * n
}

fn to_db(v: f32) -> f32 {
    if v > 1e-6 { 20.0 * v.log10() } else { -200.0 }
}

/// The bands display's frequency axis: 20 Hz .. 20 kHz on a log scale.
pub fn hz_to_x(hz: f32) -> f32 {
    let r = display_inner();
    r.x + r.w * ((hz.max(1.0) / 20.0).log10() / 3.0).clamp(0.0, 1.0)
}

pub fn x_to_hz(x: f32) -> f32 {
    let r = display_inner();
    20.0 * 10f32.powf(((x - r.x) / r.w).clamp(0.0, 1.0) * 3.0)
}

/// The crossover's edges as the sound has them: (low, high) in Hz, none for a band not split off; swapped when the
/// low one is above the high one.
pub fn edges_hz(st: &State) -> (Option<f32>, Option<f32>) {
    let lo = (st.params[band(LOW, B_MODE)] != MODE_OFF).then(|| crossover_hz(st.params[P_LOW_FREQ]));
    let hi = (st.params[band(HIGH, B_MODE)] != MODE_OFF).then(|| crossover_hz(st.params[P_HIGH_FREQ]));
    match (lo, hi) {
        (Some(a), Some(b)) if a > b => (Some(b), Some(a)),
        e => e,
    }
}

fn display(ui: &mut Ui, hint: &str) {
    let r = DISPLAY;
    lemon::plug_well(ui.c, r, 6.0);
    let inner = display_inner();
    for db in DISPLAY_GRID {
        let y = db_y(db).round();
        ui.c.fill_rect(Rect::new(r.x + 1.0, y, r.w - 2.0, 1.0), if db == 0.0 { plug::TICK } else { plug::GRID });
        ui.c.text_right(SMALL, r.right() - 5.0, y - 11.0, FAINT, &format!("{db}"));
    }
    ui.c.push_clip(r.inset(1.0));
    if ui.st.display == 0 {
        monitor(ui, inner);
    } else {
        bands_view(ui, inner);
    }
    ui.c.pop_clip();
    if !hint.is_empty() {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        let text = if right.is_empty() { left.to_string() } else { format!("{left}  {right}") };
        let w = ui.c.text_width(LABEL, &text).min(r.w - 16.0);
        ui.c.fill_rrect(Rect::new(r.x + 4.0, r.y + 4.0, w + 10.0, 17.0), 4.0, WELL);
        ui.c.text_clip(LABEL, r.x + 9.0, r.y + 6.0, r.w - 18.0, ACCENT, &text);
    }
}

/// The monitor: the shown band's level (an area) and gain envelope (a line under 0 dB), or every band's gain
/// envelope, scrolling from the right.
fn monitor(ui: &mut Ui, r: Rect) {
    let b = ui.shown();
    let h = ui.m.history;
    let n = h.len();
    let step = 2.0;
    let x_of = |i: usize| r.right() - (n - 1 - i) as f32 * step;
    let first = n.saturating_sub((r.w / step) as usize + 2);
    if ui.st.show[0] && n > 1 {
        let mut area: Vec<(f32, f32)> = (first..n).map(|i| (x_of(i), db_y(to_db(h[i][b][0])))).collect();
        area.push((x_of(n - 1), r.bottom()));
        area.push((x_of(first), r.bottom()));
        ui.c.fill_polygon(&area, BAND_COLORS[b].with_alpha(70));
    }
    let gains: Vec<usize> = if ui.st.show[2] { (0..BANDS).collect() } else if ui.st.show[1] { vec![b] } else { vec![] };
    for g in gains {
        if n < 2 {
            break;
        }
        let line: Vec<(f32, f32)> = (first..n).map(|i| (x_of(i), db_y(to_db(h[i][g][1])))).collect();
        ui.c.polyline(&line, if g == b { 1.5 } else { 1.0 }, BAND_COLORS[g]);
    }
    ui.hit(DISPLAY, Target::DisplayArea);
}

/// The bands: LOW, MID and HIGH over the frequency axis, each with its gain now, the crossover edges to drag.
fn bands_view(ui: &mut Ui, r: Rect) {
    for (f, l) in [(50.0, "50"), (100.0, "100"), (200.0, "200"), (500.0, "500"), (1000.0, "1k"), (2000.0, "2k"), (5000.0, "5k"), (10000.0, "10k")] {
        let x = hz_to_x(f).round();
        ui.c.fill_rect(Rect::new(x, DISPLAY.y + 1.0, 1.0, DISPLAY.h - 2.0), plug::GRID);
        ui.c.text(SMALL, x + 3.0, DISPLAY.bottom() - 16.0, FAINT, l);
    }
    let (lo, hi) = edges_hz(ui.m.st);
    let xs = [r.x, lo.map_or(r.x, hz_to_x), hi.map_or(r.right(), hz_to_x), r.right()];
    let latest = ui.latest();
    let shown = ui.shown();
    for b in 0..3 {
        let (x0, x1) = (xs[b], xs[b + 1]);
        if x1 - x0 < 1.0 {
            continue;
        }
        let band_r = edges(x0, x1, DISPLAY.y + 1.0, DISPLAY.bottom() - 1.0);
        let mode = ui.m.st.params[band(b, B_MODE)];
        let a = if mode == MODE_MUTED { 14 } else if b == shown { 60 } else { 34 };
        ui.c.fill_rect(band_r, BAND_COLORS[b].with_alpha(a));
        // The gain now: from 0 dB down.
        let gy = db_y(to_db(latest[b][1]));
        let y0 = db_y(0.0);
        if gy > y0 + 0.5 {
            ui.c.fill_rect(edges(x0, x1, y0, gy), BAND_COLORS[b].with_alpha(110));
        }
        ui.c.text_center(SMALL.bold(), Rect::new(x0, r.y + 2.0, x1 - x0, 12.0), BAND_COLORS[b], BAND_NAMES[b]);
        ui.hit(band_r, Target::Region(b));
    }
    for (i, e) in [lo, hi].into_iter().enumerate() {
        let Some(hz) = e else { continue };
        let x = hz_to_x(hz).round();
        let t = Target::Edge(i);
        let hr = edges(x - 4.0, x + 5.0, DISPLAY.y, DISPLAY.bottom());
        let hot = ui.hot(hr, &t);
        ui.c.fill_rect(Rect::new(x, DISPLAY.y + 1.0, 1.0, DISPLAY.h - 2.0), if hot { TEXT } else { DIM });
        ui.hit(hr, t);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The tool bars

fn bars(ui: &mut Ui) {
    panel(ui, CURVE_BAR);
    options_arrow(ui, CURVE_BAR, Target::CurveOptions);
    let cy = CURVE_BAR.cy().round();
    for (k, x) in TOOL_X.into_iter().enumerate() {
        tool_button(ui, x, cy, 24.0, Target::Tool(k), ui.st.tools[k], TOOL_LABELS[k]);
    }
    panel(ui, DISPLAY_BAR);
    let cy = DISPLAY_BAR.cy().round();
    for (k, x) in SHOW_X.into_iter().enumerate() {
        tool_button(ui, x, cy, 38.0, Target::Show(k), ui.st.show[k], SHOW_LABELS[k]);
    }
    for (i, l) in DISPLAY_NAMES.into_iter().enumerate() {
        tab(ui, display_tab(i), Target::DisplayTab(i), l, ui.st.display == i, None);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The cards

fn band_tabs(ui: &mut Ui) {
    panel(ui, TABS_CARD);
    let shown = ui.shown();
    for b in 0..BANDS {
        let r = band_tab(b);
        let mode = ui.m.st.params[band(b, B_MODE)];
        let silent = mode == MODE_MUTED || (mode == MODE_OFF && b != MID && b != MASTER);
        tab(ui, r, Target::BandTab(b), BAND_NAMES[b], shown == b, silent.then_some(OFF));
        ui.c.fill_rrect(Rect::new(r.x + 4.0, r.y + 6.0, 3.0, r.h - 12.0), 1.5, BAND_COLORS[b].with_alpha(if silent { 90 } else { 255 }));
    }
    panel(ui, SOLO_CARD);
    let w = 14.0 + 3.0 + ui.c.text_width(SMALL, "SOLO");
    let solo = ui.m.st.solo();
    lamp(ui, Target::Solo, solo, (SOLO_CARD.cx() - w / 2.0 + 7.0).round(), SOLO_CARD.cy().round(), "SOLO");
}

fn band_card(ui: &mut Ui) {
    let b = ui.shown();
    titled(ui, BAND_CARD, BAND_NAMES[b], BAND_COLORS[b]);
    let mode = ui.m.st.params[band(b, B_MODE)];
    for (m, (y, l)) in MODE_Y.into_iter().zip(MODE_LABELS).enumerate() {
        lamp(ui, Target::Mode(m as i32), mode == m as i32, MODE_X, y, l);
    }
    knob(ui, band(b, B_PRE), BAND_X[1], UPPER.0, "PRE", UPPER.1);
    knob(ui, band(b, B_POST), BAND_X[2], UPPER.0, "POST", UPPER.1);
    knob(ui, band(b, B_SEP), BAND_X[0], LOWER.0, "STEREO", LOWER.1);
    knob(ui, band(b, B_THRES), BAND_X[1], LOWER.0, "THRES", LOWER.1);
    knob(ui, band(b, B_CEIL), BAND_X[2], LOWER.0, "CEIL", LOWER.1);
    // The saturation's pair under one bracket, as FL groups them.
    let (x0, x1) = (BAND_X[1] - KNOB_R, BAND_X[2] + KNOB_R);
    let y = LOWER.0 - KNOB_R - 8.0;
    ui.c.fill_rect(edges(x0, x1, y, y + 1.0), plug::PANEL_LINE);
    ui.label(SMALL.bold(), (x0 + x1) / 2.0, y - 7.0, FAINT, "SATURATION");
}

fn env_card(ui: &mut Ui) {
    let b = ui.shown();
    titled(ui, ENV_CARD, "ENVELOPE", DIM);
    knob(ui, band(b, B_ATT), ENV_X[0], UPPER.0, "ATT", UPPER.1);
    knob(ui, band(b, B_REL), ENV_X[1], UPPER.0, "REL", UPPER.1);
    knob(ui, band(b, B_WINDOW), ENV_X[2], UPPER.0, "SUSTAIN", UPPER.1);
    // The stage counts under ATT and REL, the CURVE caption between them.
    for (k, x) in [(B_ATT_STAGES, ENV_X[0]), (B_REL_STAGES, ENV_X[1])] {
        let v = ctl::shown(ui.m.st, band(b, k));
        value_box(ui, spinner(x), Target::Steps(band(b, k)), &v.to_string(), true);
    }
    ui.label(SMALL, (ENV_X[0] + ENV_X[1]) / 2.0, MIDDLE, DIM, "CURVE");
    let rms = ui.m.st.params[band(b, B_RMS)] != 0;
    let x = ENV_X[2] - 12.0;
    lamp(ui, Target::Rms(false), !rms, x, MIDDLE - 8.0, "PEAK");
    lamp(ui, Target::Rms(true), rms, x, MIDDLE + 8.0, "RMS");
    knob(ui, band(b, B_REL2), ENV_X[0], LOWER.0, "REL 2", LOWER.1);
}

fn lmh_card(ui: &mut Ui) {
    titled(ui, LMH_CARD, "LMH", DIM);
    let x = LMH_CARD.cx();
    knob(ui, P_LMH_DELAY, x, UPPER.0, "DELAY", UPPER.1);
    knob(ui, P_LMH_MIX, x, LOWER.0, "MIX", LOWER.1);
}

fn xover_card(ui: &mut Ui) {
    titled(ui, XOVER_CARD, "CROSSOVER", DIM);
    knob(ui, P_LOW_FREQ, XOVER_X[0], UPPER.0, "LOW", UPPER.1);
    knob(ui, P_HIGH_FREQ, XOVER_X[2], UPPER.0, "HIGH", UPPER.1);
    for (k, x) in [(P_LOW_SLOPE, XOVER_X[0]), (P_HIGH_SLOPE, XOVER_X[2])] {
        let t = ctl::text(k, ctl::shown(ui.m.st, k));
        value_box(ui, slope_box(x), Target::Slope(k), &t, false);
    }
    // Version 14's linear-phase mode: shown as stored, greyed out (not played yet).
    let x = XOVER_X[1];
    let hr = edges(x - 9.0, x + 9.0, MIDDLE - 9.0, MIDDLE + 20.0);
    let mut st = ui.state(hr, &Target::Lin);
    st.disabled = true;
    lemon::toggle(ui.c, x, MIDDLE, ui.m.st.linear_phase(), st);
    ui.label(SMALL, x, MIDDLE + 14.0, OFF, "LIN");
    ui.hit(hr, Target::Lin);
    knob(ui, P_LOW_CUT, XOVER_X[1], LOWER.0, "LOW CUT", LOWER.1);
}

fn info(ui: &mut Ui) {
    let (w, h) = ui.c.size();
    let mid = emblem(ui, w / 2.0, h / 2.0 - 45.0, true, Font::condensed(32, 0).bold(), 9.0);
    let v = format!("Version {}", ui.m.version);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 8.0, w, 16.0), DIM, &v);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 26.0, w, 16.0), DIM, "Lemon Studio's multiband maximiser");
    ui.c.text_center(SMALL, Rect::new(0.0, h - 30.0, w, 16.0), FAINT, "Click to return");
    ui.hit(Rect::new(0.0, 0.0, w, h), Target::Cover);
}
