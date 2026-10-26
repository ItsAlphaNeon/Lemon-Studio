// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Pulp Filter editor: painting and hit regions (platform independent). Love Philter's layout, every knob, slider and
//! switch where FL has it and at FL's size, in the Lemon "Pith" plugin look: outlined grey cards, dark wells, lemon for
//! what is on. The filter row, the filter's IN / FILTER / WS / OUT panels, the target and part tabs with the curve
//! view, the modulation card, the master sliders, the envelope follower card and the description. Controls that do
//! nothing yet are greyed out.
//!
// spec: plugins/lemon-pulp-filter.md §2, ui/lemon-design.md §11.1

use os_engine::effects::love_philter::state::*;
use os_dsp::curve::{Curve, MAP, SYNCED};
use os_theme::design::lemon as brand;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{eq as keq, State as Look};
use os_ui::{Canvas, Color, Font, Rect};

use crate::ctl;
use crate::edit;

/// Editor size in logical pixels (FL's editor without its title bar).
pub const W: u32 = 684;
pub const H: u32 = 409;

pub const BG: Color = plug::BG;
const WELL: Color = plug::WELL;
const EDGE: Color = plug::FACE;
pub const TEXT: Color = brand::TEXT;
const DIM: Color = plug::DIM;
const FAINT: Color = plug::FAINT;
const OFF: Color = plug::OFF;
pub const ACCENT: Color = brand::MAIN;

const LABEL: Font = Font::new(10);
const SMALL: Font = Font::new(9);

/// The filter row: the enable switches' and the tabs' pitch.
pub const FILTER_PITCH: f32 = 29.7;
pub fn enable_x(f: usize) -> f32 {
    23.5 + FILTER_PITCH * f as f32
}
pub fn filter_tab(f: usize) -> Rect {
    Rect::new(12.0 + 30.0 * f as f32, 28.0, 25.0, 27.0)
}

/// The filter's panels: IN, FILTER, WS, OUT (header band 61 .. 76).
pub const PANELS: [(f32, f32, &str); 4] = [(11.0, 56.0, "IN"), (62.0, 297.0, "FILTER"), (301.0, 372.0, "WS"), (376.0, 445.0, "OUT")];
const PANEL_TOP: f32 = 61.0;
const PANEL_BOTTOM: f32 = 196.0;

/// The type box and its labels by type.
pub const TYPE_BOX: Rect = Rect::span(79.0, 106.0, 97.0, 118.0);
pub const TYPE_LABELS: [&str; TYPES as usize] = ["SVF", "LP", "HP", "LP2", "BP", "BS", "HP2", "AP", "LS", "PK", "HS", "LP3"];
pub const TYPE_NAMES: [&str; TYPES as usize] = [
    "Chocolate (state variable)",
    "Lime low pass",
    "Lime high pass",
    "Vanilla low pass",
    "Band pass",
    "Band stop",
    "Vanilla high pass",
    "All pass",
    "Low shelf",
    "Peaking",
    "High shelf",
    "Mango low pass",
];
/// The order radios: (order, x, y, label).
pub const ORDERS: [(i32, f32, f32, &str); 6] = [
    (0, 118.0, 90.0, "OFF"),
    (1, 118.0, 106.0, "SINGLE"),
    (2, 118.0, 122.0, "DUAL"),
    (3, 170.0, 90.0, "TRIPLE"),
    (4, 170.0, 106.0, "ALT DUAL"),
    (5, 170.0, 122.0, "ALT TRIPLE"),
];

/// The target tabs (the curve view's): PAN .. HIGH, the waveshaper's curve, WMIX.
pub const TARGET_TABS: Rect = Rect::span(11.0, 445.0, 201.0, 216.0);
pub const TARGET_LABELS: [&str; 9] = ["PAN", "VOL", "CUT", "RES", "LOW", "BAND", "HIGH", "WS", "WMIX"];
/// The waveshaper curve's tab.
pub const TAB_WS: usize = 7;
/// The part tabs: the pattern spinner, then PAT, LFO, MOD X, MOD Y, IEF.
pub const PATTERN_BOX: Rect = Rect::span(11.0, 52.0, 219.0, 234.0);
pub const PART_X: [f32; 6] = [54.0, 132.0, 211.0, 289.0, 368.0, 445.0];
pub const PART_LABELS: [&str; 5] = ["PAT", "LFO", "MOD X", "MOD Y", "IEF"];
pub const PART_TOP: f32 = 219.0;
pub const PART_BOTTOM: f32 = 234.0;
/// The curve view (the waveshaper's curve also takes the part row's place).
pub const CURVE: Rect = Rect::span(11.0, 445.0, 235.0, 362.0);
pub const CURVE_WS: Rect = Rect::span(11.0, 445.0, 219.0, 362.0);
pub const BAR: Rect = Rect::span(11.0, 164.0, 364.0, 397.0);
pub const TOOLBAR: Rect = Rect::span(166.0, 445.0, 364.0, 397.0);
pub const TOOL_X: [f32; 4] = [360.0, 385.0, 409.0, 432.0];
pub const TOOL_NAMES: [&str; 4] = ["Freeze editing / hide points", "Step editing", "Snap to grid", "Slide succeeding points / regions"];
const TOOL_LABELS: [&str; 4] = ["FRZ", "STP", "SNP", "SLD"];
pub const LFO_KNOB_X: [f32; 4] = [40.0, 63.0, 86.0, 109.0];

/// The right panel.
pub const MOD_CARD: Rect = Rect::span(455.0, 607.0, 57.0, 298.0);
pub const PAD: Rect = Rect::span(462.0, 600.0, 64.0, 203.0);
pub const IEF_CARD: Rect = Rect::span(612.0, 673.0, 155.0, 298.0);
pub const MASTER_X: [f32; 3] = [617.0, 641.5, 665.0];
pub const HEADER_X: [f32; 3] = [612.0, 637.5, 661.5];
pub const OVERSAMPLING: Rect = Rect::span(641.0, 672.0, 324.0, 342.0);
pub const OVERSAMPLING_LABELS: [&str; 5] = ["1x", "2x", "4x", "8x", "16x"];
pub const DESC: Rect = Rect::span(454.0, 673.0, 348.0, 398.0);
pub const LOGO: Rect = Rect::span(455.0, 560.0, 5.0, 53.0);

/// A switch of the state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sw {
    /// The filter's alternate resonance, its waveshaper, the waveshaper's bipolar mode.
    Alt,
    Ws,
    WsBipolar,
    Smooth,
    Center,
    HqEnvelopes,
    /// The envelope follower listens to filter 1.
    Use1,
    /// The shown pattern's or LFO's own switch and its tempo switch.
    PartOn,
    Tempo,
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A filter's enable switch (right click: solo) and its tab.
    Enable(usize),
    FilterTab(usize),
    /// A knob or a vertical slider of setting `k` (automation index): vertical drag, wheel, menu on right click.
    Knob(usize),
    Slider(usize),
    /// The pattern number box: 8 px of drag or a wheel notch per step.
    Pattern,
    Switch(Sw),
    /// An order radio.
    Order(i32),
    /// The type box: a click opens the types, the wheel steps them.
    Type,
    TargetTab(usize),
    PartTab(usize),
    /// The X / Y pad: the dot follows the cursor.
    Pad,
    /// The curve view: a point (drag, its menu on right click), a segment's tension handle (drag), empty space (a
    /// right click adds a point).
    Point(usize),
    Tension(usize),
    CurveArea,
    Tool(usize),
    CurveOptions,
    /// Compact view, Options, info.
    Header(usize),
    Oversampling,
    Desc,
    Logo,
    /// A control that does nothing yet: it shows its hint.
    Inert(&'static str),
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
    /// The filter shown (0 .. 7), the target tab and the part tab.
    pub filter: usize,
    pub target: usize,
    pub part: usize,
    /// The curve view's time span while a point is dragged.
    pub span: Option<f64>,
    /// The curve tools that are on (freeze, step, snap, slide).
    pub tools: [bool; 4],
    pub info: bool,
}

impl Default for ViewState {
    /// FL opens on filter 1's VOL target, its pattern.
    fn default() -> Self {
        ViewState { mouse: None, pressed: None, filter: 0, target: 1, part: 0, span: None, tools: [false; 4], info: false }
    }
}

/// What a paint shows.
pub struct Model<'a> {
    pub st: &'a State,
    /// Mod X, mod Y and the envelope follower as the sound has them (0 .. 1024).
    pub positions: [u32; 3],
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
    fn filter(&self) -> &Filter {
        &self.m.st.filters[self.st.filter.min(FILTERS - 1)]
    }
}

/// Paints the editor. `hint` (not hosted: the host shows it otherwise) takes the description's place.
pub fn paint(c: &mut Canvas, m: &Model, st: &ViewState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(BG);
    if st.info {
        info(&mut ui);
        return ui.hits;
    }
    filter_row(&mut ui);
    panels(&mut ui);
    curve_section(&mut ui);
    right_panel(&mut ui, hint);
    ui.hits
}

// ---------------------------------------------------------------------------------------------------------------
// Widgets

fn panel(ui: &mut Ui, r: Rect) {
    lemon::plug_panel(ui.c, r);
}

/// A knob of setting `k` (none: a filter's control it does not have, greyed out), radius `r`, its caption's centre at
/// `cap_y`.
fn knob(ui: &mut Ui, k: Option<usize>, cx: f32, cy: f32, r: f32, label: &str, cap_y: f32) {
    let hr = Rect::new(cx - r - 2.0, cy - r - 2.0, 2.0 * r + 4.0, 2.0 * r + 4.0);
    let w = (r * 0.2).clamp(1.8, 3.2);
    let Some(k) = k else {
        lemon::dial(ui.c, cx, cy, r, w, 0.0, false, ACCENT, Look { disabled: true, ..Look::default() });
        ui.label(SMALL, cx, cap_y, OFF, label);
        return;
    };
    let t = Target::Knob(k);
    let look = Look::hover(ui.hot(hr, &t));
    let v = ctl::shown(ui.m.st, k);
    lemon::dial(ui.c, cx, cy, r, w, ctl::norm(k, v), ctl::bipolar(k), ACCENT, look);
    if !label.is_empty() {
        ui.label(SMALL, cx, cap_y, DIM, label);
    }
    ui.hit(hr, t);
}

/// A vertical slider of setting `k` at `x`, its track from `top` to `bot` (the value filled in lemon from its zero).
fn slider(ui: &mut Ui, k: Option<usize>, x: f32, top: f32, bot: f32) {
    let hr = Rect::span(x - 8.0, x + 8.0, top - 6.0, bot + 6.0);
    let Some(k) = k else {
        keq::slot(ui.c, Rect::new(x - 3.0, top - 4.0, 6.0, bot - top + 8.0), Look::default());
        return;
    };
    let t = Target::Slider(k);
    let st = ui.state(hr, &t);
    let y_of = |n: f32| bot - (bot - top) * n.clamp(0.0, 1.0);
    let y = y_of(ctl::norm(k, ctl::shown(ui.m.st, k)));
    let (lo, _) = range(k);
    let zero = if lo < 0 { y_of(ctl::norm(k, 0)) } else { bot };
    keq::slot(ui.c, Rect::new(x - 3.0, top - 4.0, 6.0, bot - top + 8.0), st);
    if (y - zero).abs() > 1.0 {
        ui.c.fill_rrect(Rect::span(x - 1.0, x + 1.0, y.min(zero), y.max(zero)), 1.0, ACCENT.with_alpha(170));
    }
    keq::handle(ui.c, x, y.round(), ACCENT, st);
    ui.hit(hr, t);
}

/// A switch at (cx, cy): the kit's rounded-square toggle, its label to the right, or centred at `under` below it.
fn lamp(ui: &mut Ui, t: Target, on: bool, cx: f32, cy: f32, label: &str, under: Option<f32>) {
    let w = ui.c.text_width(SMALL, label);
    let hr = match under {
        Some(ly) => Rect::span(cx - 12.0, cx + 12.0, cy - 7.0, ly + 6.0),
        None => Rect::new(cx - 7.0, cy - 7.0, w + 18.0, 14.0),
    };
    let mut st = ui.state(hr, &t);
    st.disabled = matches!(t, Target::Inert(_));
    lemon::toggle(ui.c, cx, cy, on, st);
    let ink = if st.disabled { OFF } else { DIM };
    match under {
        Some(ly) => ui.label(SMALL, cx, ly, ink, label),
        None => {
            let lh = ui.c.line_height(SMALL);
            ui.c.text(SMALL, cx + 10.0, (cy - lh / 2.0).round(), ink, label);
        }
    }
    ui.hit(hr, t);
}

/// A tab of a row: the chosen one a dark rounded pill with its name in lemon (Pith's sets of choices), a lemon dot when
/// what it shows is in use.
fn tab(ui: &mut Ui, r: Rect, t: Target, label: &str, sel: bool, used: bool) {
    let hot = ui.hot(r, &t);
    let pill = r.inset(0.5);
    if sel {
        lemon::outlined(ui.c, pill, 5.0, plug::FIELD_LINE, plug::FIELD);
    } else if hot {
        ui.c.fill_rrect(pill, 5.0, Color::rgb(0x333333));
    }
    let ink = if sel { ACCENT } else if hot { TEXT } else { DIM };
    ui.c.text_center(SMALL.bold(), r, ink, label);
    if used {
        ui.c.fill_circle(r.x + 6.0, r.cy(), 1.8, ACCENT);
    }
    ui.hit(r, t);
}

// ---------------------------------------------------------------------------------------------------------------
// The filter row and the filter's panels

fn filter_row(ui: &mut Ui) {
    let solo = ui.m.st.solo();
    for f in 0..FILTERS {
        let fl = &ui.m.st.filters[f];
        let (on, next, enabled) = (fl.enabled && solo.is_none_or(|s| s == f), fl.controls[C_NEXT] != 0 && f + 1 < FILTERS, fl.enabled);
        let x = enable_x(f);
        let t = Target::Enable(f);
        let hr = Rect::new(x - 7.0, 10.0, 14.0, 14.0);
        let st = ui.state(hr, &t);
        lemon::toggle(ui.c, x, 17.0, on, st);
        if solo == Some(f) {
            ui.c.stroke_rrect(Rect::new(x - 8.0, 9.0, 16.0, 16.0), 5.0, 1.0, ACCENT);
        }
        ui.hit(hr, t);
        // To the next filter.
        if next {
            let ax = x + 15.0;
            ui.c.fill_polygon(&[(ax - 2.5, 14.0), (ax + 2.5, 17.0), (ax - 2.5, 20.0)], ACCENT);
        }
        let r = filter_tab(f);
        let t = Target::FilterTab(f);
        let hot = ui.hot(r, &t);
        let sel = ui.st.filter == f;
        if sel {
            lemon::lit(ui.c, r, 6.0, hot);
        } else {
            let st = Look::hover(hot);
            lemon::field(ui.c, r, st);
        }
        let ink = if sel { brand::COLORED_DARK } else if !enabled { OFF } else if hot { TEXT } else { DIM };
        ui.c.text_center(Font::new(15).bold(), r, ink, &(f + 1).to_string());
        ui.hit(r, t);
    }
    ui.c.fill_rect(Rect::new(4.0, 58.0, 441.0, 1.0), plug::STRIP_LINE);
}

fn panels(ui: &mut Ui) {
    for (x0, x1, title) in PANELS {
        let r = Rect::span(x0, x1, PANEL_TOP, PANEL_BOTTOM);
        panel(ui, r);
        ui.c.fill_rect(Rect::new(x0 + 1.0, 76.0, x1 - x0 - 2.0, 1.0), plug::PANEL_LINE);
        let cx = if title == "FILTER" { (x0 + x1) / 2.0 } else { (x0 + x1) / 2.0 };
        ui.label(SMALL.bold(), cx, 68.5, DIM, title);
    }
    let f = ui.st.filter.min(FILTERS - 1);
    let ctl = |n: usize| control_index(f, n);
    // IN.
    knob(ui, ctl(C_IN), 34.0, 163.0, 17.5, "VOL", 186.0);
    // FILTER: the alternate switch in the header, the type, the orders, the knobs.
    let fl = ui.filter().clone();
    let alt_t = Target::Switch(Sw::Alt);
    let lh = ui.c.line_height(SMALL);
    ui.c.text(SMALL, 255.0, (68.5 - lh / 2.0).round(), DIM, "ALT");
    let hr = Rect::new(251.0, 61.0, 40.0, 15.0);
    let st = ui.state(hr, &alt_t);
    lemon::toggle(ui.c, 283.0, 68.5, fl.alt, st);
    ui.hit(hr, alt_t);
    lemon::plug_well(ui.c, Rect::span(72.0, 222.0, 81.0, 132.0), 6.0);
    let t = Target::Type;
    let st = ui.state(TYPE_BOX, &t);
    lemon::field(ui.c, TYPE_BOX, st);
    let kind = fl.kind.clamp(0, TYPES - 1) as usize;
    ui.c.text_center(LABEL.bold(), TYPE_BOX, if st.hover { TEXT } else { ACCENT }, TYPE_LABELS[kind]);
    ui.hit(TYPE_BOX, t);
    for (o, x, y, label) in ORDERS {
        lamp(ui, Target::Order(o), fl.order == o, x, y, label, None);
    }
    knob(ui, ctl(C_ENV), 105.0, 159.0, 17.5, "ENV", 186.0);
    knob(ui, ctl(C_CUT), 148.5, 159.0, 20.5, "CUT", 186.0);
    knob(ui, ctl(C_RES), 191.5, 159.0, 17.5, "RES", 186.0);
    match fl.kind {
        CHOCOLATE => {
            for (n, x, l) in [(C_LOW, 238.0, "L"), (C_BAND, 259.5, "B"), (C_HIGH, 281.5, "H")] {
                slider(ui, ctl(n), x, 88.0, 175.0);
                ui.label(SMALL, x, 186.0, DIM, l);
            }
        }
        LOW_SHELF | PEAKING | HIGH_SHELF => knob(ui, ctl(C_LOW), 259.0, 159.0, 17.5, "GAIN", 186.0),
        MANGO => knob(ui, ctl(C_LOW), 259.0, 159.0, 17.5, "DRIVE", 186.0),
        _ => {}
    }
    // WS.
    lamp(ui, Target::Switch(Sw::Ws), fl.ws, 317.5, 109.0, "ON", Some(125.0));
    knob(ui, ctl(C_PRE), 351.5, 100.5, 17.5, "AMP", 125.0);
    let bip = fl.ws_bipolar != 0;
    let t = Target::Switch(Sw::WsBipolar);
    let hr = Rect::span(305.0, 330.0, 158.0, 192.0);
    let hot = ui.hot(hr, &t);
    ui.c.text_center(Font::new(15).bold(), Rect::new(307.5, 160.0, 20.0, 16.0), if hot { TEXT } else if bip { ACCENT } else { DIM }, if bip { "\u{b1}" } else { "+" });
    ui.label(SMALL, 317.5, 186.0, DIM, "MODE");
    ui.hit(hr, t);
    knob(ui, ctl(C_WSMIX), 351.5, 159.0, 17.5, "MIX", 186.0);
    // OUT.
    knob(ui, ctl(C_PAN), 398.5, 100.5, 17.5, "PAN", 125.0);
    knob(ui, ctl(C_VOL), 399.5, 163.0, 17.5, "VOL", 186.0);
    slider(ui, ctl(C_NEXT), 431.0, 88.0, 176.0);
    ui.label(SMALL, 431.0, 186.0, if ctl(C_NEXT).is_some() { DIM } else { OFF }, "NEXT");
}

// ---------------------------------------------------------------------------------------------------------------
// The curve view and its tabs

/// What the curve view shows: target `t`'s part `p` (0 pattern, 1 LFO, 2 .. 4 the maps), or the waveshaper's curve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Part(usize, usize),
    Ws,
}

pub fn shown(st: &ViewState) -> Shown {
    match st.target {
        TAB_WS => Shown::Ws,
        8 => Shown::Part(T_WSMIX, st.part.min(4)),
        t => Shown::Part(t.min(6), st.part.min(4)),
    }
}

/// The target a target tab shows (none for the waveshaper's curve).
pub fn tab_target(i: usize) -> Option<usize> {
    match i {
        TAB_WS => None,
        8 => Some(T_WSMIX),
        i => Some(i),
    }
}

/// The pattern the curve view shows: the pattern number's.
pub fn pattern_index(st: &State) -> usize {
    (st.globals[PATTERN].clamp(1, PATTERNS as i32) - 1) as usize
}

/// A part's curve.
pub fn part<'a>(st: &'a State, f: usize, t: usize, p: usize) -> &'a Curve {
    let a = &st.filters[f].arts[t];
    match p {
        0 => &a.patterns[pattern_index(st)],
        1 => &a.lfo,
        p => &a.maps[(p - 2).min(2)],
    }
}

/// Whether a part changes its target: a pattern or LFO that is on, a map that is not neutral.
pub fn part_used(st: &State, f: usize, t: usize, p: usize) -> bool {
    let a = &st.filters[f].arts[t];
    match p {
        0 => a.patterns.iter().any(|c| c.enabled),
        1 => a.lfo.enabled,
        p => !os_engine::effects::love_philter::neutral(&a.maps[(p - 2).min(2)], t),
    }
}

pub fn target_used(st: &State, f: usize, i: usize) -> bool {
    match tab_target(i) {
        Some(t) => (0..5).any(|p| part_used(st, f, t, p)),
        None => st.filters[f].ws,
    }
}

pub fn part_tab(i: usize) -> Rect {
    Rect::span(PART_X[i], PART_X[i + 1] - 1.0, PART_TOP, PART_BOTTOM)
}

pub fn target_tab(i: usize) -> Rect {
    let w = TARGET_TABS.w / 9.0;
    Rect::new((TARGET_TABS.x + w * i as f32).round(), TARGET_TABS.y, w.round() - 1.0, TARGET_TABS.h)
}

fn curve_section(ui: &mut Ui) {
    let f = ui.st.filter.min(FILTERS - 1);
    for (i, label) in TARGET_LABELS.into_iter().enumerate() {
        let used = target_used(ui.m.st, f, i);
        tab(ui, target_tab(i), Target::TargetTab(i), label, ui.st.target == i, used);
    }
    match shown(ui.st) {
        Shown::Part(t, p) => {
            // The pattern number.
            let r = PATTERN_BOX;
            let tg = Target::Pattern;
            let st = ui.state(r, &tg);
            lemon::field(ui.c, r, st);
            let n = ui.m.st.globals[PATTERN].clamp(1, PATTERNS as i32);
            let lh = ui.c.line_height(LABEL);
            ui.c.text_right(LABEL.bold(), r.right() - 13.0, (r.cy() - lh / 2.0).round(), TEXT, &n.to_string());
            ui.c.fill_polygon(&[(r.right() - 10.0, r.cy() - 1.5), (r.right() - 4.0, r.cy() - 1.5), (r.right() - 7.0, r.cy() + 2.0)], DIM);
            ui.hit(r, tg);
            for (i, label) in PART_LABELS.into_iter().enumerate() {
                let used = part_used(ui.m.st, f, t, i);
                tab(ui, part_tab(i), Target::PartTab(i), label, p == i, used);
            }
            let c = part(ui.m.st, f, t, p).clone();
            let pos = match p {
                2..=4 => Some(ui.m.positions[p - 2] as f32 / 1024.0),
                _ => None,
            };
            let label = (p == 0).then(|| n.to_string());
            curve_view(ui, CURVE, &c, additive(t), pos, label.as_deref());
            bar(ui, f, t, p, &c);
        }
        Shown::Ws => {
            let fl = ui.filter();
            let c = fl.ws_curve.clone();
            let bip = fl.ws_bipolar != 0;
            curve_view(ui, CURVE_WS, &c, bip, None, None);
            panel(ui, BAR);
        }
    }
    tools(ui);
}

/// Where the curve view draws its curve in `r`: time 0 half a pixel in, level 1 2 px below the top, level 0 3 px above
/// the bottom.
pub fn curve_inner(r: Rect) -> Rect {
    Rect::new(r.x + 3.5, r.y + 3.0, r.w - 7.0, r.h - 6.0)
}

/// The time the curve view spans: a map 0 .. 1, a pattern or LFO its length and a tenth more (at least one unit).
pub fn curve_span(c: &Curve) -> f64 {
    if c.kind == MAP { 1.0 } else { (edit::times(c).last().copied().unwrap_or(0.0) * 1.1).max(1.0) }
}

pub fn t_to_x(r: Rect, t: f64, span: f64) -> f32 {
    let r = curve_inner(r);
    r.x + r.w * (t / span) as f32
}

pub fn x_to_t(r: Rect, x: f32, span: f64) -> f64 {
    let r = curve_inner(r);
    f64::from((x - r.x) / r.w).max(0.0) * span
}

pub fn level_to_y(r: Rect, l: f32) -> f32 {
    let r = curve_inner(r);
    r.bottom() - r.h * l.clamp(0.0, 1.0)
}

pub fn y_to_level(r: Rect, y: f32) -> f32 {
    let r = curve_inner(r);
    ((r.bottom() - y) / r.h).clamp(0.0, 1.0)
}

/// Where point `i` of `c` sits in the view `r`.
pub fn point_xy(r: Rect, c: &Curve, i: usize, span: f64) -> (f32, f32) {
    (t_to_x(r, edit::times(c)[i], span), level_to_y(r, c.points[i].level))
}

/// The curve view in `r`: a grid (a centre line for an additive target), the curve with the area from its neutral
/// level tinted, its points, its tension handles, the loop markers, a map's input position.
fn curve_view(ui: &mut Ui, r: Rect, c: &Curve, centred: bool, pos: Option<f32>, label: Option<&str>) {
    lemon::plug_well(ui.c, r, 6.0);
    let map = c.kind == MAP;
    for k in 1..16 {
        let x = (r.x + r.w * k as f32 / 16.0).round();
        ui.c.fill_rect(Rect::new(x, r.y + 1.0, 1.0, r.h - 2.0), if k % 4 == 0 { plug::TICK.with_alpha(110) } else { plug::GRID });
    }
    for k in 1..4 {
        let y = (r.y + r.h * k as f32 / 4.0).round();
        let mid = k == 2 && centred;
        ui.c.fill_rect(Rect::new(r.x + 1.0, y, r.w - 2.0, 1.0), if mid { plug::TICK } else { plug::GRID });
    }
    if let Some(l) = label {
        ui.c.text(SMALL, r.x + 9.0, r.y + 6.0, FAINT, l);
    }
    let span = ui.st.span.unwrap_or_else(|| curve_span(c));
    let ts = edit::times(c);
    let n = ts.len();
    if n == 0 {
        return;
    }
    let mut line = vec![(t_to_x(r, 0.0, span), level_to_y(r, c.points[0].level))];
    for i in 1..n {
        let (x0, x1) = (t_to_x(r, ts[i - 1], span), t_to_x(r, ts[i], span));
        let steps = ((x1 - x0) / 2.0).ceil().clamp(1.0, 200.0) as usize;
        for s in 1..=steps {
            let x = s as f64 / steps as f64;
            line.push((x0 + (x1 - x0) * x as f32, level_to_y(r, edit::segment_level(c, i, x))));
        }
        line.push((x1, level_to_y(r, c.points[i].level)));
    }
    // A pattern holds its last level; the view draws it on to the edge.
    if !map {
        let last = *line.last().unwrap();
        line.push((curve_inner(r).right(), last.1));
    }
    let ink = if c.enabled || map { ACCENT } else { FAINT };
    let base = level_to_y(r, if centred { 0.5 } else { 0.0 });
    let mut area = line.clone();
    area.push((line.last().unwrap().0, base));
    area.push((line[0].0, base));
    ui.c.fill_polygon(&area, ink.with_alpha(36));
    ui.c.polyline(&line, 1.5, ink);
    for (i, tag) in [(c.sustain, "E"), (c.loop_start, "L")] {
        if let Some(&t) = usize::try_from(i).ok().and_then(|i| ts.get(i)) {
            let x = t_to_x(r, t, span).round();
            ui.c.fill_rect(Rect::new(x, r.y + 1.0, 1.0, r.h - 2.0), plug::TICK);
            let tag_r = Rect::new(x - 5.0, r.bottom() - 14.0, 11.0, 11.0);
            ui.c.fill_rrect(tag_r, 3.0, EDGE);
            ui.c.text_center(SMALL.bold(), tag_r, TEXT, tag);
        }
    }
    if let Some(p) = pos {
        let x = (curve_inner(r).x + curve_inner(r).w * p.clamp(0.0, 1.0)).round();
        ui.c.fill_rect(Rect::new(x, r.y + 1.0, 1.0, r.h - 2.0), brand::HIGHLIGHT.with_alpha(120));
    }
    ui.hit(r, Target::CurveArea);
    if ui.st.tools[0] {
        return;
    }
    for i in 1..n {
        if c.points[i].mode == 2 {
            continue;
        }
        let (x0, x1) = (t_to_x(r, ts[i - 1], span), t_to_x(r, ts[i], span));
        let (x, y) = ((x0 + x1) / 2.0, level_to_y(r, edit::segment_level(c, i, 0.5)));
        let t = Target::Tension(i);
        let hr = Rect::new(x - 4.0, y - 4.0, 8.0, 8.0);
        let hot = ui.hot(hr, &t);
        ui.c.stroke_circle(x, y, 3.0, 1.0, if hot { TEXT } else { ink });
        ui.hit(hr, t);
    }
    for (i, &t) in ts.iter().enumerate() {
        let (x, y) = (t_to_x(r, t, span), level_to_y(r, c.points[i].level));
        let tg = Target::Point(i);
        let hr = Rect::new(x - 5.0, y - 5.0, 10.0, 10.0);
        let hot = ui.hot(hr, &tg);
        let rad = if hot { 4.0 } else { 3.0 };
        ui.c.fill_circle(x, y, rad + 1.0, WELL);
        ui.c.fill_circle(x, y, rad, if hot { brand::HIGHLIGHT } else { ink });
        ui.hit(hr, tg);
    }
}

/// The bar under the curve view: the pattern's or LFO's switch, the LFO's knobs, the tempo switch.
fn bar(ui: &mut Ui, f: usize, t: usize, p: usize, c: &Curve) {
    panel(ui, BAR);
    if p > 1 {
        return;
    }
    lamp(ui, Target::Switch(Sw::PartOn), c.enabled, 20.0, 373.0, "", None);
    if p == 1 {
        for (i, (x, l)) in LFO_KNOB_X.into_iter().zip(["SPD", "TNS", "SKW", "PW"]).enumerate() {
            knob(ui, Some(lfo_index(f, t, i)), x, 375.0, 8.0, l, 391.0);
        }
    }
    let synced = c.flags & SYNCED != 0;
    // FL's small lamp with TEMPO to its right, in the bar's last 40 px.
    lamp(ui, Target::Switch(Sw::Tempo), synced, 131.0, 372.0, "", None);
    let f = Font::new(8).bold();
    let lh = ui.c.line_height(f);
    ui.c.text_center(f, Rect::new(118.0, (388.0 - lh / 2.0).round(), 26.0, lh), DIM, "TEMPO");
}

fn tools(ui: &mut Ui) {
    panel(ui, TOOLBAR);
    let r = Rect::new(165.0, 382.0, 14.0, 14.0);
    let t = Target::CurveOptions;
    let hot = ui.hot(r, &t);
    ui.c.fill_polygon(&[(168.0, 387.0), (176.0, 387.0), (172.0, 392.0)], if hot { TEXT } else { DIM });
    ui.hit(r, t);
    for (k, x) in TOOL_X.into_iter().enumerate() {
        let r = Rect::new(x - 11.0, 381.0, 22.0, 15.0);
        let t = Target::Tool(k);
        let mut st = ui.state(r, &t);
        st.on = ui.st.tools[k];
        st.disabled = k == 1;
        let ink = lemon::plug_button(ui.c, r, 4.0, st);
        ui.c.text_center(SMALL, r, ink, TOOL_LABELS[k]);
        ui.hit(r, t);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The right panel

/// The pad's dot for mod X and mod Y (0 .. 1024).
pub fn pad_point(x: i32, y: i32) -> (f32, f32) {
    let r = PAD.inset(6.0);
    (r.x + r.w * x.clamp(0, 1024) as f32 / 1024.0, r.bottom() - r.h * y.clamp(0, 1024) as f32 / 1024.0)
}

/// Mod X and mod Y at a point of the pad.
pub fn pad_values(x: f32, y: f32) -> (i32, i32) {
    let r = PAD.inset(6.0);
    let v = |n: f32| (n.clamp(0.0, 1.0) * 1024.0).round() as i32;
    (v((x - r.x) / r.w), v((r.bottom() - y) / r.h))
}

fn right_panel(ui: &mut Ui, hint: &str) {
    // The wordmark where FL has its logo, the lemon where it has its heart.
    let big = Font::condensed(22, 0).bold().italic();
    lemon::spaced(ui.c, big, 457.0, 4.0, plug::WORDMARK, "PULP", 2.0);
    lemon::spaced(ui.c, big, 457.0, 28.0, plug::WORDMARK, "FILTER", 2.0);
    lemon::lemon_mark(ui.c, 538.0, 21.0, 13.0, plug::WORDMARK);
    ui.hit(LOGO, Target::Logo);
    for (i, x) in HEADER_X.into_iter().enumerate() {
        let (y, rad) = (17.0, 11.0);
        let t = if i == 0 { Target::Inert("Show / hide left controls panel (not done yet)") } else { Target::Header(i) };
        let r = Rect::new(x - rad, y - rad, 2.0 * rad, 2.0 * rad);
        let mut st = ui.state(r, &t);
        st.disabled = i == 0;
        let ink = lemon::plug_button(ui.c, r, 11.0, st);
        match i {
            // A gear: a ring with teeth.
            0 => {
                for k in 0..8 {
                    let a = k as f32 * std::f32::consts::PI / 4.0;
                    ui.c.line(x + 4.0 * a.cos(), y + 4.0 * a.sin(), x + 7.0 * a.cos(), y + 7.0 * a.sin(), 2.0, ink);
                }
                ui.c.stroke_circle(x, y, 4.0, 1.6, ink);
            }
            1 => ui.c.fill_polygon(&[(x - 4.5, y - 2.0), (x + 4.5, y - 2.0), (x, y + 3.0)], ink),
            _ => ui.c.text_center(Font::new(13).bold(), r, ink, "?"),
        }
        ui.hit(r, t);
    }
    modulation(ui);
    // The master, master LFO and envelope follower levels.
    for (k, x) in [MASTER, LFO_MASTER, FOLLOW_LEVEL].into_iter().zip(MASTER_X) {
        slider(ui, Some(k), x, 62.0, 133.0);
    }
    ui.c.fill_polygon(&[(612.0, 151.0), (621.0, 151.0), (621.0, 144.0)], DIM);
    ui.label(SMALL, MASTER_X[1], 147.5, DIM, "LFO");
    ui.label(SMALL, MASTER_X[2], 147.5, DIM, "IEF");
    // The envelope follower.
    panel(ui, IEF_CARD);
    ui.label(SMALL.bold(), IEF_CARD.cx(), 164.0, DIM, "IEF");
    ui.c.fill_rect(Rect::new(IEF_CARD.x + 1.0, 172.0, IEF_CARD.w - 2.0, 1.0), plug::PANEL_LINE);
    lamp(ui, Target::Switch(Sw::Use1), ui.m.st.follow_filter1, 630.0, 184.0, "USE1", None);
    knob(ui, Some(FOLLOW_ATTACK), 643.0, 212.0, 13.5, "ATT", 237.0);
    knob(ui, Some(FOLLOW_RELEASE), 643.0, 265.5, 13.5, "REL", 289.0);
    // Switches, oversampling, description.
    let s = ui.m.st;
    let (dc, hq) = (s.dc(), s.hq_envelopes());
    lamp(ui, Target::Switch(Sw::Center), dc, 460.0, 312.0, "CENTER", None);
    lamp(ui, Target::Switch(Sw::HqEnvelopes), hq, 523.5, 312.0, "HQ ENVELOPES", None);
    let lh = ui.c.line_height(SMALL);
    ui.c.text_right(SMALL, 634.0, (333.0 - lh / 2.0).round(), DIM, "OVERSAMPLING");
    let t = Target::Oversampling;
    let st = ui.state(OVERSAMPLING, &t);
    lemon::field(ui.c, OVERSAMPLING, st);
    let o = s.oversampling() as usize;
    let r = OVERSAMPLING;
    ui.c.text_center(SMALL.bold(), Rect::new(r.x, r.y, r.w - 8.0, r.h), TEXT, OVERSAMPLING_LABELS[o]);
    ui.c.fill_polygon(&[(r.right() - 9.0, r.cy() - 1.5), (r.right() - 4.0, r.cy() - 1.5), (r.right() - 6.5, r.cy() + 1.5)], DIM);
    ui.hit(r, t);
    description(ui, hint);
}

fn modulation(ui: &mut Ui) {
    panel(ui, MOD_CARD);
    lemon::plug_well(ui.c, PAD, 6.0);
    ui.c.fill_rect(Rect::span(PAD.cx(), PAD.cx() + 1.0, PAD.y + 2.0, PAD.bottom() - 2.0), plug::GRID);
    ui.c.fill_rect(Rect::span(PAD.x + 2.0, PAD.right() - 2.0, PAD.cy(), PAD.cy() + 1.0), plug::GRID);
    let (mx, my) = (ui.m.st.mod_x(), ui.m.st.mod_y());
    let (x, y) = pad_point(mx, my);
    let hot = ui.hot(PAD, &Target::Pad);
    ui.c.fill_circle(x, y, 7.0, ACCENT.with_alpha(if hot { 80 } else { 50 }));
    ui.c.stroke_circle(x, y, 4.5, 1.5, ACCENT);
    ui.hit(PAD, Target::Pad);
    let on = ui.m.st.smooth();
    lamp(ui, Target::Switch(Sw::Smooth), on, 467.5, 215.5, "SMOOTH", None);
    knob(ui, Some(6), 535.0, 258.0, 17.5, "X", 286.0);
    knob(ui, Some(7), 581.0, 258.0, 17.5, "Y", 286.0);
    let lh = ui.c.line_height(LABEL);
    ui.c.text(LABEL.italic(), 461.0, (286.0 - lh / 2.0).round(), FAINT, "Modulation");
}

fn description(ui: &mut Ui, hint: &str) {
    lemon::plug_well(ui.c, DESC, 6.0);
    let lh = ui.c.line_height(LABEL);
    let (x, w) = (DESC.x + 7.0, DESC.w - 14.0);
    if hint.is_empty() {
        let text = plain_text(&ui.m.st.description_text());
        for (k, line) in text.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).enumerate() {
            ui.c.text_clip(LABEL, x, DESC.y + 5.0 + k as f32 * (lh + 1.0), w, if k == 0 { TEXT } else { DIM }, line);
        }
    } else {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        ui.c.text_clip(LABEL, x, DESC.y + 5.0, w, ACCENT, left);
        if !right.is_empty() {
            ui.c.text_clip(LABEL, x, DESC.y + 6.0 + lh, w, TEXT, right);
        }
    }
    ui.hit(DESC, Target::Desc);
}

/// A description's plain text: RTF (`{\rtf1 ...}`) without its control words and its font, colour and style tables,
/// `\par` and `\line` as line breaks, `\'hh` as Windows-1252 (Latin-1 here); other text unchanged.
pub fn plain_text(s: &str) -> String {
    if !s.starts_with("{\\rtf") {
        return s.to_string();
    }
    const SKIP: [&str; 6] = ["fonttbl", "colortbl", "stylesheet", "info", "pict", "generator"];
    let ch: Vec<char> = s.chars().collect();
    let mut out = String::new();
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

fn info(ui: &mut Ui) {
    let (w, h) = ui.c.size();
    let big = Font::condensed(32, 0).bold().italic();
    let wn = lemon::spaced_width(ui.c, big, "PULP FILTER", 3.0);
    let y = h / 2.0 - 60.0;
    lemon::spaced(ui.c, big, ((w - wn) / 2.0).round(), y, plug::WORDMARK, "PULP FILTER", 3.0);
    lemon::lemon_mark(ui.c, (w + wn) / 2.0 + 22.0, y + 18.0, 12.0, ACCENT);
    let v = format!("Version {}", ui.m.version);
    ui.c.text_center(LABEL, Rect::new(0.0, y + 46.0, w, 16.0), DIM, &v);
    ui.c.text_center(LABEL, Rect::new(0.0, y + 64.0, w, 16.0), DIM, "Lemon Studio's pattern-controlled multi-filter");
    ui.c.text_center(SMALL, Rect::new(0.0, h - 30.0, w, 16.0), FAINT, "Click to return");
    ui.hit(Rect::new(0.0, 0.0, w, h), Target::Cover);
}
