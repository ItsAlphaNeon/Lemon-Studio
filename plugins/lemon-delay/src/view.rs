// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Delay editor: painting and hit regions (platform independent). A plain first look (not yet the "Pith"
//! plugin look): the original's sections as outlined boxes in its places, INPUT and OUTPUT as columns at the edges,
//! DELAY TIME, DELAY MODEL and FEEDBACK over MODULATION, DIFFUSION and FEEDBACK DISTORTION, and a hint strip at the
//! foot.
//!
// spec: plugins/lemon-delay.md (§1 window, §2 controls, §3 hints)

use os_engine::effects::delay::{self as dsp, Distortion, PARAMS};
use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{eq as keq, inst, State};
use os_ui::{Canvas, Font, Rect};

use crate::params::{self, DEFS};

/// Editor size in logical pixels.
pub const W: u32 = 744;
pub const H: u32 = 236;

/// The hint strip at the foot.
pub const STRIP: Rect = Rect::span(4.0, 740.0, 218.0, 234.0);

/// Sections: (left, right, top, bottom, heading).
const SECTIONS: [(f32, f32, f32, f32, &str); 8] = [
    (4.0, 52.0, 4.0, 214.0, "INPUT"),
    (56.0, 250.0, 4.0, 112.0, "DELAY TIME"),
    (254.0, 374.0, 4.0, 112.0, "DELAY MODEL"),
    (378.0, 686.0, 4.0, 112.0, "FEEDBACK"),
    (690.0, 740.0, 4.0, 214.0, "OUTPUT"),
    (56.0, 250.0, 116.0, 214.0, "MODULATION"),
    (254.0, 374.0, 116.0, 214.0, "DIFFUSION"),
    (378.0, 686.0, 116.0, 214.0, "FEEDBACK DISTORTION"),
];

/// Knob centres of the two rows.
const ROW1: f32 = 58.0;
const ROW2: f32 = 164.0;
const BIG: f32 = 20.0;
const SMALL_R: f32 = 14.0;

/// The input fader's travel (top = the maximum).
pub const FADER_X: f32 = 28.0;
pub const FADER_TOP: f32 = 36.0;
pub const FADER_BOT: f32 = 184.0;

/// The time smoothing slider's travel.
pub const SLIDER_X0: f32 = 138.0;
pub const SLIDER_X1: f32 = 196.0;
const SLIDER_Y: f32 = 88.0;

/// The distortion curve's box.
pub const CURVE: Rect = Rect::new(592.0, 128.0, 80.0, 80.0);

const HEADING: Font = Font::condensed(10, 0).bold();
const LABEL: Font = Font::new(10).bold();
const HINT: Font = Font::new(11);

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
}

impl Target {
    pub fn id(&self) -> &str {
        match self {
            Target::Knob(id) | Target::Fader(id) | Target::Slider(id) | Target::Switch(id) | Target::Choice(id, _) => id,
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

/// Paints the editor; `hint` (empty when the host shows it) goes in the strip at the foot.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    for (x0, x1, y0, y1, s) in SECTIONS {
        lemon::outlined(ui.c, Rect::span(x0, x1, y0, y1), 4.0, plug::PANEL_LINE, plug::PANEL);
        let lh = ui.c.line_height(HEADING);
        ui.c.text_center(HEADING, Rect::span(x0, x1, y0 + 3.0, y0 + 3.0 + lh), plug::DIM, s);
    }
    let v = values(m);
    let off = v[dsp::TYPE] == dsp::OFF;
    let no_filter = v[dsp::FILTER] == dsp::FILTER_OFF;
    let limit = v[dsp::DIST_TYPE] == 0;
    let (mod_time, mod_cut) = (v[dsp::MOD_TIME] > 0, v[dsp::MOD_CUTOFF] > 0);

    // INPUT
    fader(&mut ui, "in", "WET");
    // DELAY TIME
    knob(&mut ui, "time", 90.0, ROW1, BIG, "TIME", false, off, mod_time);
    switch(&mut ui, "sync", 120.0, 34.0, "TEMPO SYNC", off);
    switch(&mut ui, "keep", 120.0, 54.0, "KEEP PITCH", off);
    slider(&mut ui, "smooth", "SMOOTHING", off);
    let pan = v[dsp::TYPE] == dsp::PING_PONG;
    knob(&mut ui, "offset", 226.0, ROW1, SMALL_R, if pan { "PAN" } else { "OFFSET" }, true, off, false);
    // DELAY MODEL
    choices(&mut ui, "type", 262.0, 322.0, 26.0, &["MONO", "STEREO", "PING PONG", "OFF"], false);
    knob(&mut ui, "spread", 348.0, ROW1, SMALL_R, "STEREO", false, off, false);
    // FEEDBACK
    knob(&mut ui, "fb", 414.0, ROW1, BIG, "LEVEL", false, false, false);
    knob(&mut ui, "cutoff", 464.0, ROW1, SMALL_R, "CUTOFF", false, no_filter, mod_cut);
    knob(&mut ui, "res", 506.0, ROW1, SMALL_R, "RES", false, no_filter, false);
    choices(&mut ui, "filter", 530.0, 566.0, 26.0, &["LP", "HP", "BP", "OFF"], false);
    knob(&mut ui, "srate", 606.0, ROW1, SMALL_R, "SMP RATE", false, false, false);
    knob(&mut ui, "bits", 654.0, ROW1, SMALL_R, "BITS", false, false, false);
    // OUTPUT
    knob(&mut ui, "wet", 715.0, 46.0, SMALL_R, "WET", false, false, false);
    knob(&mut ui, "tone", 715.0, 108.0, SMALL_R, "TONE", true, false, false);
    knob(&mut ui, "dry", 715.0, 170.0, SMALL_R, "DRY", false, false, false);
    // MODULATION: the rate drives the time and the cutoff (the arrow).
    knob(&mut ui, "mrate", 88.0, ROW2, SMALL_R, "RATE", false, false, false);
    let ax = 118.0;
    ui.c.polyline(&[(ax, ROW2 - 5.0), (ax + 6.0, ROW2), (ax, ROW2 + 5.0)], 1.5, plug::FAINT);
    knob(&mut ui, "mtime", 152.0, ROW2, SMALL_R, "TIME", false, false, mod_time);
    knob(&mut ui, "mcut", 204.0, ROW2, SMALL_R, "CUTOFF", false, false, mod_cut);
    // DIFFUSION
    knob(&mut ui, "dlevel", 290.0, ROW2, SMALL_R, "LEVEL", false, false, false);
    knob(&mut ui, "dspread", 340.0, ROW2, SMALL_R, "SPREAD", false, false, false);
    // FEEDBACK DISTORTION
    choices(&mut ui, "dtype", 386.0, 428.0, 140.0, &["LIMIT", "SAT"], false);
    knob(&mut ui, "drive", 456.0, ROW2, SMALL_R, "LEVEL", false, false, false);
    knob(&mut ui, "knee", 504.0, ROW2, SMALL_R, "KNEE", false, limit, false);
    knob(&mut ui, "sym", 552.0, ROW2, SMALL_R, "SYMMETRY", true, limit, false);
    curve(&mut ui, &v);
    // The hint strip.
    lemon::outlined(ui.c, STRIP, 3.0, plug::WELL_LINE, plug::WELL);
    if !hint.is_empty() {
        let lh = ui.c.line_height(HINT);
        ui.c.text_clip(HINT, STRIP.x + 6.0, (STRIP.cy() - lh / 2.0).round(), STRIP.w - 12.0, brand::TEXT, hint);
    }
    ui.hits
}

/// A caption under a control; boxed when the modulation moves it.
fn caption(ui: &mut Ui, cx: f32, y: f32, s: &str, disabled: bool, boxed: bool) {
    inst::caption(ui.c, ui.t, cx, y, s, disabled);
    if boxed {
        let w = inst::caption_width(ui.c, s, 1.0) + 8.0;
        ui.c.stroke_rrect(Rect::new((cx - w / 2.0).round() + 0.5, y - 2.5, w.round(), 13.0), 2.0, 1.0, plug::FAINT);
    }
}

/// A knob of radius `r`, caption under it.
#[allow(clippy::too_many_arguments)]
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, r: f32, cap: &str, bipolar: bool, disabled: bool, boxed: bool) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - r - 3.0, cy - r - 3.0, 2.0 * r + 6.0, 2.0 * r + 6.0);
    let st = ui.state(hr, &target, disabled);
    let arc = if disabled { plug::OFF } else { brand::MAIN };
    lemon::dial(ui.c, cx, cy, r, if r > 16.0 { 2.8 } else { 2.2 }, ui.m.norm(id), bipolar, arc, st);
    caption(ui, cx, cy + r + 6.0, cap, disabled, boxed);
    ui.hit(hr, target);
}

/// An on / off switch with its label at the right.
fn switch(ui: &mut Ui, id: &str, x: f32, cy: f32, label: &str, disabled: bool) {
    let target = Target::Switch(id.into());
    let w = ui.c.text_width(LABEL, label);
    let hr = Rect::new(x - 2.0, cy - 8.0, w + 22.0, 16.0);
    let mut st = ui.state(hr, &target, disabled);
    let on = ui.plain(id) != 0;
    st.on = on;
    lemon::toggle(ui.c, x + 6.0, cy, on, st);
    let lh = ui.c.line_height(LABEL);
    ui.c.text(LABEL, x + 16.0, (cy - lh / 2.0).round(), if disabled { plug::OFF } else { plug::DIM }, label);
    ui.hit(hr, target);
}

/// A selector: one row per position from `top`, the chosen one lit.
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

/// The input fader: a slot, the lemon fill up to the handle, the caption under the travel.
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
    caption(ui, cx, FADER_BOT + 12.0, cap, false, false);
    ui.hit(hr, target);
}

/// The time smoothing slider: a horizontal slot, the fill up to the handle, the caption under it.
fn slider(ui: &mut Ui, id: &str, cap: &str, disabled: bool) {
    let target = Target::Slider(id.into());
    let hr = Rect::span(SLIDER_X0 - 6.0, SLIDER_X1 + 6.0, SLIDER_Y - 8.0, SLIDER_Y + 8.0);
    let st = ui.state(hr, &target, disabled);
    keq::slot(ui.c, Rect::span(SLIDER_X0 - 4.0, SLIDER_X1 + 4.0, SLIDER_Y - 3.0, SLIDER_Y + 3.0), st);
    let x = SLIDER_X0 + (SLIDER_X1 - SLIDER_X0) * ui.m.norm(id).clamp(0.0, 1.0);
    let col = if disabled { plug::OFF } else { brand::MAIN };
    if x - SLIDER_X0 > 1.0 {
        ui.c.fill_rrect(Rect::span(SLIDER_X0, x, SLIDER_Y - 1.0, SLIDER_Y + 1.0), 1.0, col.with_alpha(170));
    }
    ui.c.fill_rrect(Rect::new(x.round() - 3.0, SLIDER_Y - 6.0, 6.0, 12.0), 2.0, if st.hover || st.pressed { brand::TEXT } else { col });
    caption(ui, (SLIDER_X0 + SLIDER_X1) / 2.0, SLIDER_Y + 9.0, cap, disabled, false);
    ui.hit(hr, target);
}

/// The distortion's transfer curve (input -1..1 across, output -1..1 up): the limiter's clip or the saturator.
fn curve(ui: &mut Ui, v: &[i32; PARAMS]) {
    lemon::outlined(ui.c, CURVE, 3.0, plug::WELL_LINE, plug::WELL);
    let (cx, cy, half) = (CURVE.cx(), CURVE.cy(), CURVE.w / 2.0 - 4.0);
    ui.c.fill_rect(Rect::new(CURVE.x + 4.0, cy.round(), CURVE.w - 8.0, 1.0), plug::GRID);
    ui.c.fill_rect(Rect::new(cx.round(), CURVE.y + 4.0, 1.0, CURVE.h - 8.0), plug::GRID);
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
    let id = t.id();
    format!("{}: {}", name(id, m), m.text(id))
}
