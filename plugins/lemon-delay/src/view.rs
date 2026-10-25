// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Delay editor: painting and hit regions (platform independent). The Lemon "Pith" plugin look: the original's
//! sections as outlined cards under ruled headings, INPUT and OUTPUT as columns at the edges, DELAY TIME, DELAY MODEL
//! and FEEDBACK over MODULATION, DIFFUSION and FEEDBACK DISTORTION; the hint well and the wordmark along the foot.
//!
// spec: plugins/lemon-delay.md (§1 window, §2 controls, §3 hints)

use os_engine::effects::delay::{self as dsp, Distortion, PARAMS};
use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{eq as keq, inst, State};
use os_ui::{Canvas, Color, Font, Rect};

use crate::params::{self, DEFS};

/// Editor size in logical pixels.
pub const W: u32 = 784;
pub const H: u32 = 272;

/// The two rows of cards (top, bottom); a heading's rule runs 12 px above its card.
const CARD1: (f32, f32) = (28.0, 116.0);
const CARD2: (f32, f32) = (142.0, 234.0);

/// Cards: (left, right, top, bottom, heading).
const CARDS: [(f32, f32, f32, f32, &str); 8] = [
    (8.0, 66.0, CARD1.0, CARD2.1, "INPUT"),
    (72.0, 266.0, CARD1.0, CARD1.1, "DELAY TIME"),
    (272.0, 400.0, CARD1.0, CARD1.1, "DELAY MODEL"),
    (406.0, 712.0, CARD1.0, CARD1.1, "FEEDBACK"),
    (718.0, 776.0, CARD1.0, CARD2.1, "OUTPUT"),
    (72.0, 266.0, CARD2.0, CARD2.1, "MODULATION"),
    (272.0, 400.0, CARD2.0, CARD2.1, "DIFFUSION"),
    (406.0, 712.0, CARD2.0, CARD2.1, "FEEDBACK DISTORTION"),
];

/// Knob centres of the two rows.
const ROW1: f32 = 64.0;
const ROW2: f32 = 178.0;
/// Knob radii: TIME and the feedback LEVEL are bigger (and 3 px higher, so the row's captions line up).
const BIG: f32 = 22.0;
const SMALL: f32 = 19.0;

/// The input fader's travel (top = the maximum).
pub const FADER_X: f32 = 37.0;
pub const FADER_TOP: f32 = 52.0;
pub const FADER_BOT: f32 = 198.0;

/// The time smoothing slider's travel.
pub const SLIDER_X0: f32 = 140.0;
pub const SLIDER_X1: f32 = 200.0;
const SLIDER_Y: f32 = 92.0;

/// The distortion curve's well.
pub const CURVE: Rect = Rect::new(622.0, 148.0, 80.0, 80.0);

/// The foot's centre line: the hint well at the left and the wordmark at the right are centred on it, 8 px under the
/// cards and 8 px over the edge.
const FOOT: f32 = 253.0;
/// The right edge of the cards (and of the wordmark).
const RIGHT: f32 = 776.0;

const RULE: Color = Color::rgb(0x3c3c3c);
const HEADING: Font = Font::condensed(11, 0).bold();
const LABEL: Font = Font::new(10).bold();
const HINT: Font = Font::new(11);
const MARK_BIG: Font = Font::condensed(22, 0).bold();
const MARK_SMALL: Font = Font::condensed(10, 0).bold();

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
    /// The input fader: vertical drag over its travel.
    Fader(String),
    /// The time smoothing slider: horizontal drag over its travel.
    Slider(String),
    /// An on / off switch (click toggles).
    Switch(String),
    /// One position of a selector (click picks it).
    Choice(String, i32),
    /// The wordmark (the About menu).
    Logo,
}

impl Target {
    /// The setting's id ("" for the wordmark).
    pub fn id(&self) -> &str {
        match self {
            Target::Knob(id) | Target::Fader(id) | Target::Slider(id) | Target::Switch(id) | Target::Choice(id, _) => id,
            Target::Logo => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// The pointer and the pressed control.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
}

struct Ui<'a> {
    c: &'a mut Canvas,
    t: &'a Palette,
    m: &'a dyn Model,
    st: &'a ViewState,
    hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }
    fn state(&self, r: Rect, t: &Target, disabled: bool) -> State {
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed, disabled, ..State::NORMAL }
    }
    fn plain(&self, id: &str) -> i32 {
        self.m.plain(id).round() as i32
    }
}

/// The input fader's y for a normalised value.
pub fn fader_y(norm: f32) -> f32 {
    FADER_BOT - (FADER_BOT - FADER_TOP) * norm.clamp(0.0, 1.0)
}

/// The settings shown.
pub fn values(m: &dyn Model) -> [i32; PARAMS] {
    std::array::from_fn(|k| m.plain(DEFS[k].id).round() as i32)
}

/// Paints the editor; `hint` (empty when the host shows it) goes in the well at the foot.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    for (x0, x1, y0, y1, s) in CARDS {
        lemon::plug_panel(ui.c, Rect::span(x0, x1, y0, y1));
        heading(&mut ui, x0, x1, y0 - 12.0, s);
    }
    let v = values(m);
    let off = v[dsp::TYPE] == dsp::OFF;
    let no_filter = v[dsp::FILTER] == dsp::FILTER_OFF;
    let limit = v[dsp::DIST_TYPE] == 0;
    let (mod_time, mod_cut) = (v[dsp::MOD_TIME] > 0, v[dsp::MOD_CUTOFF] > 0);

    // INPUT
    fader(&mut ui, "in", "WET");
    // DELAY TIME
    knob(&mut ui, "time", 102.0, ROW1 - 3.0, BIG, "TIME", false, off, mod_time);
    switch(&mut ui, "sync", 132.0, 46.0, "TEMPO SYNC", off);
    switch(&mut ui, "keep", 132.0, 66.0, "KEEP PITCH", off);
    slider(&mut ui, "smooth", "SMOOTHING", off);
    let pan = v[dsp::TYPE] == dsp::PING_PONG;
    knob(&mut ui, "offset", 240.0, ROW1, SMALL, if pan { "PAN" } else { "OFFSET" }, true, off, false);
    // DELAY MODEL
    choices(&mut ui, "type", 282.0, 346.0, ROW1 - 24.0, &["MONO", "STEREO", "PING PONG", "OFF"], false);
    knob(&mut ui, "spread", 372.0, ROW1, SMALL, "STEREO", false, off, false);
    // FEEDBACK
    knob(&mut ui, "fb", 438.0, ROW1 - 3.0, BIG, "LEVEL", false, false, false);
    knob(&mut ui, "cutoff", 490.0, ROW1, SMALL, "CUTOFF", false, no_filter, mod_cut);
    knob(&mut ui, "res", 538.0, ROW1, SMALL, "RES", false, no_filter, false);
    choices(&mut ui, "filter", 568.0, 608.0, ROW1 - 24.0, &["LP", "HP", "BP", "OFF"], false);
    knob(&mut ui, "srate", 636.0, ROW1, SMALL, "SMP RATE", false, false, false);
    knob(&mut ui, "bits", 684.0, ROW1, SMALL, "BITS", false, false, false);
    // OUTPUT
    knob(&mut ui, "wet", 747.0, 58.0, SMALL, "WET", false, false, false);
    knob(&mut ui, "tone", 747.0, 126.0, SMALL, "TONE", true, false, false);
    knob(&mut ui, "dry", 747.0, 194.0, SMALL, "DRY", false, false, false);
    // MODULATION: the rate drives the time and the cutoff (the arrow).
    knob(&mut ui, "mrate", 104.0, ROW2, SMALL, "RATE", false, false, false);
    let ax = 134.0;
    ui.c.polyline(&[(ax, ROW2 - 5.0), (ax + 6.0, ROW2), (ax, ROW2 + 5.0)], 1.5, plug::FAINT);
    knob(&mut ui, "mtime", 170.0, ROW2, SMALL, "TIME", false, false, mod_time);
    knob(&mut ui, "mcut", 230.0, ROW2, SMALL, "CUTOFF", false, false, mod_cut);
    // DIFFUSION
    knob(&mut ui, "dlevel", 308.0, ROW2, SMALL, "LEVEL", false, false, false);
    knob(&mut ui, "dspread", 364.0, ROW2, SMALL, "SPREAD", false, false, false);
    // FEEDBACK DISTORTION
    choices(&mut ui, "dtype", 416.0, 460.0, ROW2 - 16.0, &["LIMIT", "SAT"], false);
    knob(&mut ui, "drive", 490.0, ROW2, SMALL, "LEVEL", false, false, false);
    knob(&mut ui, "knee", 538.0, ROW2, SMALL, "KNEE", false, limit, false);
    knob(&mut ui, "sym", 586.0, ROW2, SMALL, "SYMMETRY", false, limit, false);
    curve(&mut ui, &v);
    // The foot.
    let mark_left = logo(&mut ui);
    let well = Rect::span(8.0, mark_left - 16.0, FOOT - 11.0, FOOT + 11.0);
    lemon::plug_well(ui.c, well, 6.0);
    if !hint.is_empty() {
        let lh = ui.c.line_height(HINT);
        ui.c.text_clip(HINT, well.x + 8.0, (well.cy() - lh / 2.0).round(), well.w - 16.0, brand::TEXT, hint);
    }
    ui.hits
}

/// A card's heading (as Lemon Limiter's and Lemon Reverb's): letter-spaced caps centred on a hairline rule at `y`,
/// the rule stopping 7 px short of the text (none where the card is too narrow for it).
fn heading(ui: &mut Ui, x0: f32, x1: f32, y: f32, s: &str) {
    let lh = ui.c.line_height(HEADING);
    let w = lemon::spaced_width(ui.c, HEADING, s, 2.0);
    let tx = ((x0 + x1 + 1.0) / 2.0 - w / 2.0).round();
    lemon::spaced(ui.c, HEADING, tx, (y - lh / 2.0).round(), plug::DIM, s, 2.0);
    if tx - 7.0 > x0 + 8.0 {
        ui.c.fill_rect(Rect::span(x0 + 4.0, tx - 7.0, y, y + 1.0), RULE);
        ui.c.fill_rect(Rect::span((tx + w + 6.0).round(), x1 - 4.0, y, y + 1.0), RULE);
    }
}

/// A caption under a control; boxed when the modulation moves it.
fn caption(ui: &mut Ui, cx: f32, y: f32, s: &str, disabled: bool, boxed: bool) {
    inst::caption(ui.c, ui.t, cx, y, s, disabled);
    if boxed {
        let w = inst::caption_width(ui.c, s, 1.0) + 10.0;
        ui.c.stroke_rrect(Rect::new((cx - w / 2.0).round() + 0.5, y - 2.5, w.round(), 14.0), 4.0, 1.0, plug::FAINT);
    }
}

/// A pith dial of radius `r` (hit area 3 px around it), caption under it.
#[allow(clippy::too_many_arguments)]
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, r: f32, cap: &str, bipolar: bool, disabled: bool, boxed: bool) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - r - 3.0, cy - r - 3.0, 2.0 * r + 6.0, 2.0 * r + 6.0);
    let st = ui.state(hr, &target, disabled);
    lemon::dial(ui.c, cx, cy, r, if r > SMALL { 3.2 } else { 2.8 }, ui.m.norm(id), bipolar, brand::MAIN, st);
    caption(ui, cx, cy + r + 7.0, cap, disabled, boxed);
    ui.hit(hr, target);
}

/// An on / off switch: the Pith toggle (a rounded square, lemon when on) with its label at the right.
fn switch(ui: &mut Ui, id: &str, x: f32, cy: f32, label: &str, disabled: bool) {
    let target = Target::Switch(id.into());
    let w = ui.c.text_width(LABEL, label);
    let hr = Rect::new(x - 2.0, cy - 8.0, w + 24.0, 16.0);
    let mut st = ui.state(hr, &target, disabled);
    let on = ui.plain(id) != 0;
    st.on = on;
    lemon::toggle(ui.c, x + 6.0, cy, on, st);
    let lh = ui.c.line_height(LABEL);
    let ink = if disabled { plug::OFF } else if st.hover { brand::TEXT } else { plug::DIM };
    ui.c.text(LABEL, x + 18.0, (cy - lh / 2.0).round(), ink, label);
    ui.hit(hr, target);
}

/// A selector: one row per position from `top`, the chosen one on a dark pill in lemon.
fn choices(ui: &mut Ui, id: &str, x0: f32, x1: f32, top: f32, names: &[&str], disabled: bool) {
    let chosen = ui.plain(id);
    for (i, name) in names.iter().enumerate() {
        let target = Target::Choice(id.into(), i as i32);
        let r = Rect::span(x0, x1, top + 16.0 * i as f32, top + 16.0 * i as f32 + 15.0);
        let st = ui.state(r, &target, disabled);
        let ink = lemon::choice_button(ui.c, r, chosen == i as i32, brand::MAIN, st);
        ui.c.text_center(LABEL, r, ink, name);
        ui.hit(r, target);
    }
}

/// The input fader: the EQ's slot and handle, the lemon fill up to the handle, the caption under the travel.
fn fader(ui: &mut Ui, id: &str, cap: &str) {
    let target = Target::Fader(id.into());
    let cx = FADER_X;
    let hr = Rect::new(cx - 11.0, FADER_TOP - 8.0, 22.0, FADER_BOT - FADER_TOP + 16.0);
    let st = ui.state(hr, &target, false);
    keq::slot(ui.c, Rect::new(cx - 3.0, FADER_TOP - 4.0, 6.0, FADER_BOT - FADER_TOP + 8.0), st);
    let y = fader_y(ui.m.norm(id));
    if FADER_BOT - y > 1.0 {
        ui.c.fill_rrect(Rect::span(cx - 1.0, cx + 1.0, y, FADER_BOT), 1.0, brand::MAIN.with_alpha(170));
    }
    keq::handle(ui.c, cx, y.round(), brand::MAIN, st);
    caption(ui, cx, FADER_BOT + 14.0, cap, false, false);
    ui.hit(hr, target);
}

/// The time smoothing slider: a horizontal slot, the fill up to the handle (the fader's cap stood upright), the
/// caption under it.
fn slider(ui: &mut Ui, id: &str, cap: &str, disabled: bool) {
    let target = Target::Slider(id.into());
    let hr = Rect::span(SLIDER_X0 - 6.0, SLIDER_X1 + 6.0, SLIDER_Y - 8.0, SLIDER_Y + 8.0);
    let st = ui.state(hr, &target, disabled);
    keq::slot(ui.c, Rect::span(SLIDER_X0 - 4.0, SLIDER_X1 + 4.0, SLIDER_Y - 3.0, SLIDER_Y + 3.0), st);
    let x = SLIDER_X0 + (SLIDER_X1 - SLIDER_X0) * ui.m.norm(id).clamp(0.0, 1.0);
    let line = if disabled { plug::OFF } else { brand::MAIN };
    if x - SLIDER_X0 > 1.0 {
        ui.c.fill_rrect(Rect::span(SLIDER_X0, x, SLIDER_Y - 1.0, SLIDER_Y + 1.0), 1.0, line.with_alpha(170));
    }
    let x = x.round();
    ui.c.fill_rrect(Rect::new(x - 5.0, SLIDER_Y - 6.5, 10.0, 13.0), 3.5, Color::rgb(0x151515));
    let (a, b) = if (st.hover || st.pressed) && !disabled { (0x7d7d7d, 0x575757) } else { (0x6f6f6f, 0x4c4c4c) };
    ui.c.fill_rect_gradient(Rect::new(x - 4.0, SLIDER_Y - 5.5, 8.0, 11.0), Color::rgb(a), Color::rgb(b), 2.5);
    ui.c.fill_rect(Rect::new(x - 0.5, SLIDER_Y - 3.5, 1.2, 7.0), line);
    caption(ui, (SLIDER_X0 + SLIDER_X1) / 2.0, SLIDER_Y + 11.0, cap, disabled, false);
    ui.hit(hr, target);
}

/// The wordmark on the foot's line (as Lemon Limiter's and Lemon Sampler's): the lemon mark and "LEMON" in lemon
/// beside a large grey "DELAY", right edge on the cards'; returns its left edge.
fn logo(ui: &mut Ui) -> f32 {
    let wb = lemon::spaced_width(ui.c, MARK_BIG, "DELAY", 2.0);
    let ws = lemon::spaced_width(ui.c, MARK_SMALL, "LEMON", 2.5);
    let (lh_b, lh_s) = (ui.c.line_height(MARK_BIG), ui.c.line_height(MARK_SMALL));
    let bx = (RIGHT - wb).round();
    lemon::spaced(ui.c, MARK_BIG, bx, (FOOT - lh_b / 2.0).round(), plug::WORDMARK, "DELAY", 2.0);
    let sx = (bx - 8.0 - ws).round();
    lemon::spaced(ui.c, MARK_SMALL, sx, (FOOT - lh_s / 2.0).round() + 1.0, brand::MAIN, "LEMON", 2.5);
    let left = sx - 14.0;
    lemon::lemon_mark(ui.c, left + 5.0, FOOT, 4.0, brand::MAIN);
    ui.hit(Rect::span(left - 2.0, RIGHT, FOOT - 12.0, FOOT + 12.0), Target::Logo);
    left
}

/// The distortion's transfer curve (input -1..1 across, output -1..1 up): the limiter's clip or the saturator.
fn curve(ui: &mut Ui, v: &[i32; PARAMS]) {
    lemon::plug_well(ui.c, CURVE, 6.0);
    let (cx, cy, half) = (CURVE.cx(), CURVE.cy(), CURVE.w / 2.0 - 6.0);
    ui.c.fill_rect(Rect::new(CURVE.x + 6.0, cy.round(), CURVE.w - 12.0, 1.0), plug::GRID);
    ui.c.fill_rect(Rect::new(cx.round(), CURVE.y + 6.0, 1.0, CURVE.h - 12.0), plug::GRID);
    let d = Distortion::of(v);
    let saturate = v[dsp::DIST_TYPE] != 0;
    let pts: Vec<(f32, f32)> = (0..=64)
        .map(|i| {
            let x = i as f32 / 32.0 - 1.0;
            (cx + x * half, cy - d.curve(saturate, x).clamp(-1.0, 1.0) * half)
        })
        .collect();
    ui.c.polyline(&pts, 1.5, brand::MAIN);
}

// ---------------------------------------------------------------------------------------------------------------
// Hints (spec §3)

/// A setting's hint name: the R/L offset reads as a pan in ping pong.
pub fn name(id: &str, m: &dyn Model) -> String {
    match params::index(id) {
        Some(dsp::OFFSET) if m.plain("type").round() as i32 == dsp::PING_PONG => params::PAN_NAME.into(),
        Some(k) => DEFS[k].name.into(),
        None => id.into(),
    }
}

/// The hint line for a target.
pub fn hint(t: &Target, m: &dyn Model) -> String {
    match t {
        Target::Logo => "Lemon Delay".into(),
        t => format!("{}: {}", name(t.id(), m), m.text(t.id())),
    }
}
