// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A small effect's editor: painting and hit regions (platform independent). The Lemon "Pith" plugin look, as Lemon
//! Delay's: one row of outlined cards under ruled headings holding the dials and selectors, a well that pictures the
//! settings, and the hint well and the wordmark on one line along the foot.
//!
// spec: plugins/lemon-small-effects.md (§2 the frame, §3 hints), the plugin pages (plugins/lemon-soft-clipper.md,
// lemon-fast-distortion.md, lemon-balance.md, lemon-stereo-enhancer.md, lemon-transient-processor.md; each §2
// controls); ui/lemon-design.md §11.1

use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{inst, State};
use os_ui::{Canvas, Color, Font, Rect};

use crate::{Control, Spec, MAX};

/// Dials bigger than this get the thicker arc.
const SMALL: f32 = 19.0;

const RULE: Color = Color::rgb(0x3c3c3c);
const HEADING: Font = Font::condensed(11, 0).bold();
const LABEL: Font = Font::new(10).bold();
const HINT: Font = Font::new(11);
const MARK_BIG: Font = Font::condensed(22, 0).bold();
const MARK_SMALL: Font = Font::condensed(10, 0).bold();

/// The foot's centre line: the hint well at the left and the wordmark at the right are centred on it, 8 px under the
/// cards and 8 px over the edge.
pub fn foot(spec: &Spec) -> f32 {
    spec.size.1 as f32 - 19.0
}

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
    /// One position of a selector (click picks it).
    Choice(String, i32),
    /// The wordmark (the About menu).
    Logo,
}

impl Target {
    /// The setting's id ("" for the wordmark).
    pub fn id(&self) -> &str {
        match self {
            Target::Knob(id) | Target::Choice(id, _) => id,
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
    fn state(&self, r: Rect, t: &Target) -> State {
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed, ..State::NORMAL }
    }
}

/// The settings shown (the first `n` of `MAX`).
pub fn values(spec: &Spec, m: &dyn Model) -> [i32; MAX] {
    std::array::from_fn(|k| spec.settings.get(k).map_or(0, |s| m.plain(s.id).round() as i32))
}

/// Paints the editor; `hint` (empty when the host shows it) goes in the well at the foot.
pub fn paint(spec: &Spec, c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    for card in spec.cards {
        lemon::plug_panel(ui.c, Rect::span(card.x0, card.x1, card.y0, card.y1));
        heading(&mut ui, card.x0, card.x1, card.y0 - 12.0, card.heading);
    }
    for control in spec.controls {
        match *control {
            Control::Knob { id, cx, cy, r, caption, bipolar } => knob(&mut ui, id, cx, cy, r, caption, bipolar),
            Control::Choice { id, x0, x1, top, names } => {
                let min = spec.index(id).map_or(0, |k| spec.settings[k].range.0);
                choices(&mut ui, id, x0, x1, top, min, names);
            }
        }
    }
    if let Some(d) = &spec.display {
        lemon::plug_well(ui.c, d.r, 6.0);
        let v = values(spec, m);
        (d.draw)(ui.c, d.r, &v[..spec.settings.len()]);
    }
    // The foot.
    let y = foot(spec);
    let mark_left = logo(&mut ui, spec);
    let well = Rect::span(8.0, mark_left - 16.0, y - 11.0, y + 11.0);
    lemon::plug_well(ui.c, well, 6.0);
    if !hint.is_empty() {
        let lh = ui.c.line_height(HINT);
        ui.c.text_clip(HINT, well.x + 8.0, (well.cy() - lh / 2.0).round(), well.w - 16.0, brand::TEXT, hint);
    }
    ui.hits
}

/// A card's heading (as Lemon Delay's): letter-spaced caps centred on a hairline rule at `y`, the rule stopping 7 px
/// short of the text (none where the card is too narrow for it).
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

/// A pith dial of radius `r` (hit area 3 px around it), caption under it.
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, r: f32, cap: &str, bipolar: bool) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - r - 3.0, cy - r - 3.0, 2.0 * r + 6.0, 2.0 * r + 6.0);
    let st = ui.state(hr, &target);
    lemon::dial(ui.c, cx, cy, r, if r > SMALL { 3.2 } else { 2.8 }, ui.m.norm(id), bipolar, brand::MAIN, st);
    inst::caption(ui.c, ui.t, cx, cy + r + 7.0, cap, false);
    ui.hit(hr, target);
}

/// A selector: one row per position from `top` (the first is the range's minimum), the chosen one on a dark pill in
/// lemon.
fn choices(ui: &mut Ui, id: &str, x0: f32, x1: f32, top: f32, min: i32, names: &[&str]) {
    let chosen = ui.m.plain(id).round() as i32;
    for (i, name) in names.iter().enumerate() {
        let v = min + i as i32;
        let target = Target::Choice(id.into(), v);
        let r = Rect::span(x0, x1, top + 16.0 * i as f32, top + 16.0 * i as f32 + 15.0);
        let st = ui.state(r, &target);
        let ink = lemon::choice_button(ui.c, r, chosen == v, brand::MAIN, st);
        ui.c.text_center(LABEL, r, ink, name);
        ui.hit(r, target);
    }
}

/// The wordmark on the foot's line (as Lemon Delay's): the lemon mark and "LEMON" in lemon beside the effect's name
/// in large grey capitals, right edge on the cards'; returns its left edge.
fn logo(ui: &mut Ui, spec: &Spec) -> f32 {
    let (y, right) = (foot(spec), spec.size.0 as f32 - 8.0);
    let wb = lemon::spaced_width(ui.c, MARK_BIG, spec.word, 2.0);
    let ws = lemon::spaced_width(ui.c, MARK_SMALL, "LEMON", 2.5);
    let (lh_b, lh_s) = (ui.c.line_height(MARK_BIG), ui.c.line_height(MARK_SMALL));
    let bx = (right - wb).round();
    lemon::spaced(ui.c, MARK_BIG, bx, (y - lh_b / 2.0).round(), plug::WORDMARK, spec.word, 2.0);
    let sx = (bx - 8.0 - ws).round();
    lemon::spaced(ui.c, MARK_SMALL, sx, (y - lh_s / 2.0).round() + 1.0, brand::MAIN, "LEMON", 2.5);
    let left = sx - 14.0;
    lemon::lemon_mark(ui.c, left + 5.0, y, 4.0, brand::MAIN);
    ui.hit(Rect::span(left - 2.0, right, y - 12.0, y + 12.0), Target::Logo);
    left
}

// ---------------------------------------------------------------------------------------------------------------
// Pieces the displays share

/// A transfer curve in a well: `f` maps input to output; the input runs over `-x_max..x_max` across, the output over
/// `-y_max..y_max` up (clipped to the well). Axes through the centre, and dim lines at full scale (±1 out).
pub fn transfer(c: &mut Canvas, r: Rect, x_max: f32, y_max: f32, f: impl Fn(f32) -> f32) {
    let (cx, cy) = (r.cx(), r.cy());
    let (hw, hh) = (r.w / 2.0 - 6.0, r.h / 2.0 - 6.0);
    c.fill_rect(Rect::new(r.x + 6.0, cy.round(), r.w - 12.0, 1.0), plug::GRID);
    c.fill_rect(Rect::new(cx.round(), r.y + 6.0, 1.0, r.h - 12.0), plug::GRID);
    for s in [-1.0f32, 1.0] {
        let y = (cy - s / y_max * hh).round();
        if (y - cy).abs() < hh {
            c.fill_rect(Rect::new(r.x + 6.0, y, r.w - 12.0, 1.0), plug::GRID);
        }
        let x = (cx + s / x_max * hw).round();
        if (x - cx).abs() < hw {
            c.fill_rect(Rect::new(x, r.y + 6.0, 1.0, r.h - 12.0), plug::GRID);
        }
    }
    let n = (r.w - 12.0).max(2.0) as usize;
    let pts: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let x = (i as f32 / n as f32 * 2.0 - 1.0) * x_max;
            let y = f(x);
            let y = if y.is_finite() { y.clamp(-y_max, y_max) } else { 0.0 };
            (cx + x / x_max * hw, cy - y / y_max * hh)
        })
        .collect();
    c.polyline(&pts, 1.5, brand::MAIN);
}

/// Two output meters side by side (L and R) for `left` and `right` as levels: a slim bar from the bottom of its slot
/// for each, full height at `top` (a level above it fills the bar; a negative one, an inverted output, is grey), a
/// tick at unity across both.
pub fn level_bars(c: &mut Canvas, r: Rect, left: f32, right: f32, top: f32) {
    const BAR: f32 = 14.0;
    const GAP: f32 = 26.0;
    let (y0, y1) = (r.y + 9.0, r.y + r.h - 22.0);
    let x0 = (r.cx() - BAR - GAP / 2.0).round();
    let lh = c.line_height(LABEL);
    let unity = (y1 - (y1 - y0) / top).round();
    c.fill_rect(Rect::span(x0 - 8.0, x0 + 2.0 * BAR + GAP + 8.0, unity, unity + 1.0), plug::TICK);
    for (i, (v, name)) in [(left, "L"), (right, "R")].into_iter().enumerate() {
        let x = x0 + i as f32 * (BAR + GAP);
        c.fill_rrect(Rect::span(x, x + BAR, y0, y1), 3.0, plug::TRACK);
        let h = ((y1 - y0) * (v.abs() / top).clamp(0.0, 1.0)).round();
        if h > 0.0 {
            let col = if v < 0.0 { plug::FAINT } else { brand::MAIN };
            c.fill_rrect(Rect::span(x, x + BAR, y1 - h, y1), 3.0, col);
        }
        c.text_center(LABEL, Rect::new(x - 8.0, y1 + 4.0, BAR + 16.0, lh), plug::DIM, name);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Hints (spec §3)

/// A setting's hint name.
pub fn name(spec: &Spec, id: &str) -> String {
    spec.index(id).map_or_else(|| id.to_string(), |k| spec.settings[k].name.to_string())
}

/// The hint line for a target.
pub fn hint(spec: &Spec, t: &Target, m: &dyn Model) -> String {
    match t {
        Target::Logo => spec.name.into(),
        t => format!("{}: {}", name(spec, t.id()), m.text(t.id())),
    }
}
