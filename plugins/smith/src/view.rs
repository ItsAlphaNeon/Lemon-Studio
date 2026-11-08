// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Smith's editor: painting and hit regions (platform independent). Ogun's form control for control, each at the place
//! the spec's table gives it and of the same kind, drawn in the Lemon "Pith" plugin look the way Lemon Trio redraws
//! 3x Osc. Coordinates are the spec's form px: the editor's own window, so they are used as they stand.
//!
// spec: plugins/smith.md ("The editor"), plugins/ogun.md ("The form", "Controls and ranges"), ui/lemon-design.md §11.1

use citrus::edit;
use os_dsp::curve::{Curve, LfoShape, LFO, MAP};
use os_theme::design::lemon as brand;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::State as Look;
use os_ui::tabler as tb;
use os_ui::{Canvas, Color, Font, Rect};

use crate::ctl::{self, Field, Flag};
use crate::state::*;

/// Editor size in logical pixels; without the keyboard, 85 px shorter.
pub const W: u32 = 560;
pub const H: u32 = 640;
pub const H_NO_KEYS: u32 = 555;

pub const BG: Color = plug::BG;
const WELL: Color = plug::WELL;
pub const TEXT: Color = brand::TEXT;
const DIM: Color = plug::DIM;
const FAINT: Color = plug::FAINT;
pub const ACCENT: Color = brand::MAIN;
/// The grey a fader's groove is lit in where Ogun does not light it in its accent colour.
const LIT_GREY: Color = Color::rgb(0x6a6a6a);
/// A caption's plate, one step up from the card.
const PLATE: Color = Color::rgb(0x3b3b3b);
/// The graph's heavy grid lines, between `plug::GRID` and `plug::TICK`.
const GRID_HEAVY: Color = Color::rgb(0x424242);
const GRID_LIGHT: Color = Color::rgb(0x2e2e2e);

const LABEL: Font = Font::new(11);
const SMALL: Font = Font::new(10);
/// The bar under the graph has less room.
const TINY: Font = Font::new(9);
/// A card's title: tall and narrow, to fit the width Ogun's title takes.
const TITLE: Font = Font::condensed(14, 4).bold();
const TITLE_GAP: f32 = 0.5;

/// A rect from its left, right, top and bottom edges.
const fn edges(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

// Cards.
pub const MASTER: Rect = edges(5.0, 92.0, 2.0, 90.0);
pub const MODS: Rect = edges(96.0, 311.0, 2.0, 90.0);
pub const WORDS: Rect = edges(320.0, 527.0, 7.0, 85.0);
pub const PAD: Rect = edges(99.0, 176.0, 8.0, 84.0);
pub const TIMBRE: Rect = edges(5.0, 234.0, 98.0, 211.0);
pub const UNISON: Rect = edges(240.0, 399.0, 98.0, 211.0);
pub const FILTER: Rect = edges(404.0, 556.0, 98.0, 211.0);
pub const EDITOR: Rect = edges(5.0, 442.0, 216.0, 428.0);
pub const LOGO: Rect = edges(447.0, 555.0, 217.0, 250.0);
pub const EQ: Rect = edges(447.0, 555.0, 252.0, 428.0);
pub const CHORUS: Rect = edges(2.0, 175.0, 433.0, 552.0);
pub const DELAY: Rect = edges(180.0, 338.0, 433.0, 552.0);
pub const REVERB: Rect = edges(343.0, 558.0, 433.0, 552.0);

// The form's lines.
const ROW_TITLE: f32 = 108.0;
const ROW_CAPTION: f32 = 202.5;
const SOUND_TOP: f32 = 122.0;
const SOUND_BOT: f32 = 188.0;
const FX_TITLE: f32 = 443.0;
const FX_CAPTION: f32 = 544.5;
const FX_TOP: f32 = 459.0;
const FX_BOT: f32 = 528.0;

/// The round buttons right of the info box: random preset, options, show / hide keyboard.
const ROUND_X: f32 = 543.5;
pub const ROUND_Y: [f32; 3] = [18.0, 50.0, 75.5];
const ROUND_R: f32 = 9.5;
pub const HEADER_NAMES: [&str; 3] = ["Generate random preset", "Options", "Show / hide testing keyboard"];

/// The articulator editor's targets, in the menu's groups: articulators 0..=12, then the final maps.
pub const TARGETS: usize = ARTS + FINALS;
pub const TARGET_NAMES: [&str; TARGETS] = [
    "Volume",
    "Filter frequency",
    "Filter resonance",
    "Pitch",
    "Unison pitch",
    "Unison phase",
    "Timbre pre-decay",
    "Timbre decay time",
    "Timbre release time",
    "Timbre fullness",
    "Timbre morphing",
    "Timbre rotation",
    "Timbre EQ shifting",
    "Timbre forced harmonics",
    "Timbre impulse EQ",
    "Timbre decay EQ",
    "Timbre release EQ",
];
pub const PART_NAMES: [&str; 6] = ["Envelope", "LFO", "Keyboard mapping", "Velocity mapping", "Modulation X mapping", "Modulation Y mapping"];

/// The name part `p` of target `t` goes by.
pub fn part_name(t: usize, p: usize) -> &'static str {
    if t >= ARTS {
        "Mapping"
    } else if t == A_REL && p == 3 {
        "Release velocity mapping"
    } else {
        PART_NAMES[p]
    }
}

/// The parts target `t` has: articulation targets all six, mapping targets the four maps, the final maps one curve
/// without a choice.
pub fn parts_of(t: usize) -> &'static [usize] {
    match t {
        A_VOLUME | A_CUT | A_RES | A_PITCH | A_U_PITCH => &[0, 1, 2, 3, 4, 5],
        _ if t < ARTS => &[2, 3, 4, 5],
        _ => &[],
    }
}

/// What the curve view edits: an articulator's part, or a final map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CurveId {
    Part(usize, usize),
    Final(usize),
}

/// The curve `id` in `st`.
pub fn curve_of(st: &State, id: CurveId) -> &Curve {
    match id {
        CurveId::Part(a, 0) => &st.arts[a].env,
        CurveId::Part(a, 1) => &st.arts[a].lfo,
        CurveId::Part(a, p) => &st.arts[a].maps[p - 2],
        CurveId::Final(i) => &st.finals[i],
    }
}

pub fn curve_mut(st: &mut State, id: CurveId) -> &mut Curve {
    match id {
        CurveId::Part(a, 0) => &mut st.arts[a].env,
        CurveId::Part(a, 1) => &mut st.arts[a].lfo,
        CurveId::Part(a, p) => &mut st.arts[a].maps[p - 2],
        CurveId::Final(i) => &mut st.finals[i],
    }
}

/// The testing keyboard's keys: four octaves.
pub const FIRST_KEY: u8 = 36;
pub const LAST_KEY: u8 = 83;

/// A curve with more points than this (a resynthesized map) is drawn as a line, without handles.
pub const MANY_POINTS: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A knob or a fader: both drag relative to where they were grabbed.
    Knob(Field),
    Steps(Field),
    Switch(Field),
    /// One choice of a set: the setting and the value it sets.
    Choice(Field, i32),
    Pad,
    /// The articulator editor's target menu and part menu.
    TargetBox,
    PartBox,
    Comb,
    EqType(usize),
    CurveArea,
    Point(usize),
    Tension(usize),
    CurveOptions,
    Tool(usize),
    Key(u8),
    Header(usize),
    Logo,
    Cover,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
    pub info: bool,
    /// The keyboard is hidden.
    pub no_keys: bool,
    /// The articulator editor's target, and the part each target last showed.
    pub target: usize,
    pub part: [usize; TARGETS],
    /// Freeze, step, snap, slide.
    pub tools: [bool; 4],
    /// The time span a point's drag began with (the view stays still during it).
    pub span: Option<f64>,
    /// The key the mouse holds.
    pub key: Option<u8>,
    /// The keys held down.
    pub playing: [u64; 2],
}

/// The painted height.
pub fn height(no_keys: bool) -> u32 {
    if no_keys { H_NO_KEYS } else { H }
}

/// What the curve view shows for the shown target and part.
pub fn shown_curve(st: &ViewState) -> CurveId {
    let t = st.target.min(TARGETS - 1);
    if t >= ARTS {
        return CurveId::Final(t - ARTS);
    }
    let parts = parts_of(t);
    let p = if parts.contains(&st.part[t]) { st.part[t] } else { parts[0] };
    CurveId::Part(t, p)
}

pub struct Model<'a> {
    pub st: &'a State,
    pub version: &'a str,
}

struct Ui<'a, 'c> {
    c: &'c mut Canvas,
    m: &'a Model<'a>,
    st: &'a ViewState,
    hits: Vec<Hit>,
}

impl Ui<'_, '_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }
    fn hot(&self, r: Rect, t: &Target) -> bool {
        self.st.pressed.as_ref() == Some(t) || (self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y)))
    }
    fn state(&self, r: Rect, t: &Target) -> Look {
        Look { hover: self.hot(r, t), pressed: self.st.pressed.as_ref() == Some(t), on: false, disabled: false }
    }
    /// Text centred on a point.
    fn label(&mut self, f: Font, cx: f32, cy: f32, col: Color, s: &str) {
        self.c.text_center(f, Rect::new(cx - 60.0, cy - 8.0, 120.0, 16.0), col, s);
    }
    /// Text from `x` on the line `cy`.
    fn left(&mut self, f: Font, x: f32, cy: f32, col: Color, s: &str) -> f32 {
        let lh = self.c.line_height(f);
        self.c.text(f, x, (cy - lh / 2.0).round(), col, s)
    }
    /// A card's title on the line `cy`; returns where it ends.
    fn title(&mut self, x: f32, cy: f32, s: &str) -> f32 {
        let w = lemon::spaced_width(self.c, TITLE, s, TITLE_GAP);
        let lh = self.c.line_height(TITLE);
        lemon::spaced(self.c, TITLE, x, (cy - lh / 2.0).round(), DIM, s, TITLE_GAP);
        x + w
    }
}

/// Paints the editor. `hint` (when the host does not show it) goes on the info box's bottom line.
pub fn paint(c: &mut Canvas, m: &Model, st: &ViewState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(BG);
    if st.info {
        info(&mut ui);
        return ui.hits;
    }
    master(&mut ui);
    modulation(&mut ui);
    words(&mut ui, hint);
    round_buttons(&mut ui);
    timbre(&mut ui);
    unison(&mut ui);
    filter(&mut ui);
    editor_card(&mut ui);
    logo(&mut ui);
    eq(&mut ui);
    chorus(&mut ui);
    delay(&mut ui);
    reverb(&mut ui);
    if !st.no_keys {
        keyboard(&mut ui);
    }
    ui.hits
}

// ---------------------------------------------------------------------------------------------------------------
// Widgets

/// The lemon mark and "SMITH" in the wordmark's grey condensed caps on the line `cy`: `anchor` 0 starts at `x`, .5
/// centres on it, 1 ends at it.
fn emblem(ui: &mut Ui, x: f32, cy: f32, anchor: f32, f: Font, s: f32) -> Rect {
    let wn = lemon::spaced_width(ui.c, f, "SMITH", 3.0);
    let (wl, gap) = (2.2 * s, (s * 1.3).round());
    let w = wl + gap + wn;
    let x = (x - w * anchor).round();
    let lh = ui.c.line_height(f);
    lemon::lemon_mark(ui.c, x + wl / 2.0, cy, s, brand::MAIN);
    lemon::spaced(ui.c, f, x + wl + gap, (cy - lh / 2.0).round(), plug::WORDMARK, "SMITH", 3.0);
    Rect::new(x, cy - lh / 2.0, w, lh)
}

/// A knob of setting `f`, radius `r`.
fn knob(ui: &mut Ui, f: Field, cx: f32, cy: f32, r: f32) {
    let hr = Rect::new(cx - r - 2.0, cy - r - 2.0, 2.0 * r + 4.0, 2.0 * r + 4.0);
    let w = (r * 0.2).clamp(1.8, 3.2);
    let t = Target::Knob(f);
    let look = Look::hover(ui.hot(hr, &t));
    let v = ctl::shown(ui.m.st, f);
    lemon::dial(ui.c, cx, cy, r, w, ctl::norm(f, v), ctl::is_bipolar(f), ACCENT, look);
    ui.hit(hr, t);
}

/// A fader's look: its ticks' long and short lengths, the colour of its lit groove.
#[derive(Clone, Copy)]
struct FaderLook {
    long: f32,
    short: f32,
    lit: Option<Color>,
}

/// TIMBRE, UNISON: wide ticks, a grey lit groove.
const SOUND_FADER: FaderLook = FaderLook { long: 6.0, short: 4.0, lit: Some(LIT_GREY) };
/// The effects stand closer together.
const FX_FADER: FaderLook = FaderLook { long: 3.0, short: 2.0, lit: Some(ACCENT) };

/// A fader of setting `f` on the groove at `x`: Trio's drawing, its cap narrowed to fit Ogun's spacing. The groove is
/// lit from zero to the cap; it drags relative to where it was grabbed, so a click does not move it.
fn fader(ui: &mut Ui, f: Field, x: f32, top: f32, bot: f32, look: FaderLook) {
    let t = Target::Knob(f);
    let hr = edges(x - 11.0, x + 11.0, top - 6.0, bot + 6.0);
    let st = ui.state(hr, &t);
    for k in 0..9 {
        let yy = (top + (bot - top) * k as f32 / 8.0).round();
        let w = if k % 4 == 0 { look.long } else { look.short };
        ui.c.fill_rect(Rect::new(x - 8.0 - w, yy, w, 1.0), plug::TICK);
        ui.c.fill_rect(Rect::new(x + 8.0, yy, w, 1.0), plug::TICK);
    }
    ui.c.fill_rrect(Rect::new(x - 2.5, top - 2.0, 5.0, bot - top + 4.0), 2.5, Color::rgb(0x1a1a1a));
    let v = ctl::norm(f, ctl::shown(ui.m.st, f));
    let hy = (bot - (bot - top) * v).round();
    if let Some(col) = look.lit {
        let zero = if ctl::is_bipolar(f) { (top + bot) / 2.0 } else { bot };
        if (hy - zero).abs() > 1.0 {
            ui.c.fill_rrect(edges(x - 1.0, x + 1.0, hy.min(zero), hy.max(zero)), 1.0, col);
        }
    }
    ui.c.fill_rrect(Rect::new(x - 7.0, hy - 6.0, 14.0, 12.0), 4.0, Color::rgb(0x151515));
    let (a, b) = if st.hover || st.pressed { (0x7a7a7a, 0x555555) } else { (0x6c6c6c, 0x4b4b4b) };
    ui.c.fill_rect_gradient(Rect::new(x - 6.0, hy - 5.0, 12.0, 10.0), Color::rgb(a), Color::rgb(b), 3.0);
    ui.c.fill_rrect(Rect::new(x - 4.5, hy - 0.75, 9.0, 1.5), 0.75, ACCENT);
    ui.hit(hr, t);
}

/// A caption on a plate.
fn caption(ui: &mut Ui, cx: f32, cy: f32, w: f32, s: &str) {
    ui.c.fill_rrect(Rect::new(cx - w / 2.0, cy - 6.5, w, 13.0), 3.0, PLATE);
    ui.label(SMALL, cx, cy, DIM, s);
}

/// A switch at (cx, cy), its label on one side of it. `w` is the label's width in Ogun's form: the hit area stops
/// there whatever the font does, so it cannot reach a neighbour.
fn lamp(ui: &mut Ui, f: Field, cx: f32, cy: f32, label: &str, on_left: bool, w: f32) {
    lamp_in(ui, SMALL, f, cx, cy, label, on_left, w);
}

#[allow(clippy::too_many_arguments)]
fn lamp_in(ui: &mut Ui, font: Font, f: Field, cx: f32, cy: f32, label: &str, on_left: bool, w: f32) {
    let on = ctl::get(ui.m.st, f) != 0;
    let t = Target::Switch(f);
    let reach = if label.is_empty() { 0.0 } else { w + 2.0 };
    let hr = if on_left { Rect::new(cx - 7.0 - reach, cy - 7.0, 14.0 + reach, 14.0) } else { Rect::new(cx - 7.0, cy - 7.0, 14.0 + reach, 14.0) };
    let st = ui.state(hr, &t);
    lemon::toggle(ui.c, cx, cy, on, st);
    let col = if on { TEXT } else { DIM };
    let lh = ui.c.line_height(font);
    let y = (cy - lh / 2.0).round();
    if on_left {
        ui.c.text_right(font, cx - 8.5, y, col, label);
    } else {
        ui.c.text(font, cx + 8.5, y, col, label);
    }
    ui.hit(hr, t);
}

/// What a box shows: its number, the reverb color by Ogun's letters. The hint has the words.
fn box_text(f: Field, v: i32) -> String {
    match f {
        Field::Int(k) if k == crate::state::CHORUS => v.to_string(),
        Field::Int(k) if k == crate::state::REVERB + 7 => ["B+", "B", "F", "W", "W+"][v.clamp(0, 4) as usize].into(),
        _ => ctl::text(f, v),
    }
}

/// A box showing a value, hit area `hit`.
fn value_box(ui: &mut Ui, r: Rect, hit: Rect, f: Field, right: bool) {
    let t = Target::Steps(f);
    let st = ui.state(hit, &t);
    lemon::field(ui.c, r, st);
    let col = if st.hover { TEXT } else { ACCENT };
    let text = box_text(f, ctl::shown(ui.m.st, f));
    let lh = ui.c.line_height(LABEL);
    if right {
        ui.c.text_right(LABEL, r.right() - 5.0, (r.cy() - lh / 2.0).round(), col, &text);
    } else {
        ui.c.text_center(LABEL, r, col, &text);
    }
    ui.hit(hit, t);
}

/// One cell of the filter's grid.
fn cell(ui: &mut Ui, f: Field, v: i32, cx: f32, cy: f32, label: &str) {
    let r = Rect::new(cx - 11.0, cy - 9.5, 22.0, 19.0);
    let t = Target::Choice(f, v);
    let mut st = ui.state(r, &t);
    st.on = ctl::get(ui.m.st, f) == v;
    let ink = lemon::plug_button(ui.c, r.inset(0.5), 4.0, st);
    ui.c.text_center(SMALL.bold(), r, ink, label);
    ui.hit(r, t);
}

fn triangle_down(ui: &mut Ui, x: f32, y: f32, col: Color) {
    ui.c.fill_polygon(&[(x - 4.5, y - 2.0), (x + 4.5, y - 2.0), (x, y + 3.0)], col);
}

fn triangle_right(ui: &mut Ui, x: f32, y: f32, col: Color) {
    ui.c.fill_polygon(&[(x - 2.0, y - 4.0), (x + 3.0, y), (x - 2.0, y + 4.0)], col);
}

// ---------------------------------------------------------------------------------------------------------------
// The top band

fn master(ui: &mut Ui) {
    lemon::plug_panel(ui.c, MASTER);
    let look = FaderLook { long: 6.0, short: 4.0, lit: None };
    let (top, bot) = (8.0, 66.0);
    fader(ui, Field::Int(LEVEL), 21.0, top, bot, FaderLook { lit: Some(ACCENT), ..look });
    fader(ui, Field::Int(COARSE), 54.0, top, bot, look);
    fader(ui, Field::Int(FINE), 77.0, top, bot, look);
    // Level's ramp, and C (keys) F.
    ui.c.fill_rrect(edges(11.0, 38.0, 73.0, 85.0), 3.0, PLATE);
    ui.c.fill_polygon(&[(19.0, 82.0), (31.0, 82.0), (31.0, 76.0)], ACCENT);
    ui.c.fill_rrect(edges(44.0, 87.0, 73.0, 85.0), 3.0, PLATE);
    ui.label(SMALL, 50.0, 79.0, DIM, "C");
    ui.c.icon(&tb::PIANO, Rect::new(60.0, 73.5, 11.0, 11.0), DIM);
    ui.label(SMALL, 79.0, 79.0, DIM, "F");
}

/// The pad's dot for mod X / Y values (y up); it stays 4 px inside the pad.
pub fn pad_point(x: i32, y: i32) -> (f32, f32) {
    let r = PAD.inset(4.0);
    (r.x + r.w * x as f32 / 512.0, r.bottom() - r.h * y as f32 / 512.0)
}

/// Mod X / Y values for a point on the pad.
pub fn pad_values(x: f32, y: f32) -> (i32, i32) {
    let v = |t: f32| (t.clamp(0.0, 1.0) * 512.0).round() as i32;
    let r = PAD.inset(4.0);
    (v((x - r.x) / r.w), v((r.bottom() - y) / r.h))
}

fn modulation(ui: &mut Ui) {
    lemon::plug_panel(ui.c, MODS);
    lemon::plug_well(ui.c, PAD, 6.0);
    for k in 1..4 {
        let col = if k == 2 { GRID_HEAVY } else { GRID_LIGHT };
        let x = (PAD.x + PAD.w * k as f32 / 4.0).round();
        ui.c.fill_rect(Rect::new(x, PAD.y + 1.0, 1.0, PAD.h - 2.0), col);
        let y = (PAD.y + PAD.h * k as f32 / 4.0).round();
        ui.c.fill_rect(Rect::new(PAD.x + 1.0, y, PAD.w - 2.0, 1.0), col);
    }
    let (x, y) = pad_point(ctl::shown(ui.m.st, Field::Int(MOD_X)), ctl::shown(ui.m.st, Field::Int(MOD_Y)));
    let hot = ui.hot(PAD, &Target::Pad);
    ui.c.fill_circle(x, y, 5.0, WELL);
    ui.c.fill_circle(x, y, 4.0, if hot { brand::HIGHLIGHT } else { ACCENT });
    ui.hit(PAD, Target::Pad);
    ui.title(184.0, 17.0, "MODULATION");
    knob(ui, Field::Int(MOD_X), 196.0, 43.5, 11.0);
    ui.label(SMALL.bold(), 216.0, 43.5, DIM, "X");
    knob(ui, Field::Int(MOD_Y), 196.0, 71.5, 11.0);
    ui.label(SMALL.bold(), 216.0, 71.5, DIM, "Y");
    lamp(ui, Field::Flag(Flag::Smooth), 295.5, 73.5, "SMOOTH", true, 39.0);
}

/// The info box: the patch's lines at Ogun's places, the hint on the bottom line.
fn words(ui: &mut Ui, hint: &str) {
    lemon::plug_well(ui.c, WORDS, 6.0);
    let (x, w) = (WORDS.x + 4.0, WORDS.w - 8.0);
    let lh = ui.c.line_height(LABEL);
    let lines = if hint.is_empty() { 5 } else { 4 };
    for (k, l) in ui.m.st.comment.split('\r').take(lines).enumerate() {
        let y = 15.0 + 13.0 * k as f32;
        let col = if k == 0 { TEXT } else { DIM };
        ui.c.text_clip(LABEL, x, (y - lh / 2.0).round(), w, col, l.trim_start_matches('\n'));
    }
    if !hint.is_empty() {
        let plain = hint.replace('\t', "   ");
        ui.c.text_clip(LABEL, x, (76.0 - lh / 2.0).round(), w, ACCENT, &plain);
    }
}

fn round_buttons(ui: &mut Ui) {
    for (i, y) in ROUND_Y.into_iter().enumerate() {
        let x = ROUND_X;
        let t = Target::Header(i);
        let r = Rect::new(x - ROUND_R, y - ROUND_R, 2.0 * ROUND_R, 2.0 * ROUND_R);
        let st = ui.state(r, &t);
        let ink = lemon::plug_button(ui.c, r, ROUND_R, st);
        match i {
            // A die's face.
            0 => {
                ui.c.stroke_rrect(Rect::new(x - 5.0, y - 5.0, 10.0, 10.0), 2.0, 1.0, ink);
                for (dx, dy) in [(-2.5, -2.5), (2.5, 2.5), (0.0, 0.0)] {
                    ui.c.fill_circle(x + dx, y + dy, 1.0, ink);
                }
            }
            1 => triangle_down(ui, x, y, ink),
            _ => ui.c.icon(&tb::PIANO, Rect::new(x - 6.5, y - 6.5, 13.0, 13.0), ink),
        }
        ui.hit(r, t);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The sound cards

fn timbre(ui: &mut Ui) {
    lemon::plug_panel(ui.c, TIMBRE);
    let end = ui.title(11.0, ROW_TITLE, "TIMBRE");
    // A bell: Ogun's mark for the harmonics' ring.
    let (bx, by) = ((end + 8.0).max(56.0), ROW_TITLE);
    ui.c.polyline(&[(bx - 5.0, by + 4.0), (bx - 3.0, by + 1.0), (bx - 2.0, by - 4.0), (bx + 2.0, by - 4.0), (bx + 3.0, by + 1.0), (bx + 5.0, by + 4.0), (bx - 5.0, by + 4.0)], 1.2, FAINT);
    lamp(ui, Field::Flag(Flag::Declick), 148.0, 106.5, "DECLICK", true, 36.0);
    lamp(ui, Field::Flag(Flag::AltDecay), 169.5, 106.5, "ALT DECAY", false, 46.0);
    for (x, f, l) in [(22.0, RICH, "RICH"), (49.0, PRE, "PRE"), (76.0, DEC, "DEC"), (103.0, REL, "REL"), (130.0, FULL, "FULL")] {
        fader(ui, Field::Int(f), x, SOUND_TOP, SOUND_BOT, SOUND_FADER);
        caption(ui, x, ROW_CAPTION, 25.0, l);
    }
    fader(ui, Field::Int(MORPH), 164.0, SOUND_TOP, SOUND_BOT, SOUND_FADER);
    // 1 > 2.
    let (cx, cy) = (165.0, ROW_CAPTION);
    ui.c.fill_rrect(Rect::new(149.0, cy - 6.5, 11.0, 13.0), 3.0, PLATE);
    ui.label(SMALL, 154.5, cy, DIM, "1");
    triangle_right(ui, cx, cy, ACCENT);
    ui.c.fill_rrect(Rect::new(170.0, cy - 6.5, 11.0, 13.0), 3.0, PLATE);
    ui.label(SMALL, 175.5, cy, DIM, "2");
    for (y0, y1, cy, f, n) in [(135.0, 154.0, 163.0, SEED2, "2"), (174.0, 193.0, ROW_CAPTION, SEED1, "1")] {
        let r = edges(187.0, 227.0, y0, y1);
        value_box(ui, r, r, Field::Int(f), true);
        ui.left(SMALL, 190.0, cy, DIM, "SEED");
        ui.c.fill_rrect(Rect::new(215.0, cy - 6.5, 12.0, 13.0), 3.0, PLATE);
        ui.label(SMALL, 221.0, cy, DIM, n);
    }
}

fn unison(ui: &mut Ui) {
    lemon::plug_panel(ui.c, UNISON);
    let end = ui.title(245.0, ROW_TITLE, "UNISON");
    // Voices around a centre: )|(
    let (ix, iy) = ((end + 8.0).max(293.0), ROW_TITLE);
    let q = std::f32::consts::FRAC_PI_4;
    ui.c.arc(ix - 12.0, iy, 7.5, -q, 2.0 * q, 1.4, FAINT);
    ui.c.fill_rect(Rect::new(ix - 0.5, iy - 5.0, 1.0, 10.0), FAINT);
    ui.c.arc(ix + 12.0, iy, 7.5, 3.0 * q, 2.0 * q, 1.4, FAINT);
    let ord = edges(247.0, 271.0, 174.0, 193.0);
    value_box(ui, ord, ord, Field::Int(U_ORD), false);
    ui.label(SMALL, 259.5, ROW_CAPTION, DIM, "ORD");
    for (x, f, l) in [(295.0, U_PAN, "PAN"), (322.0, U_VOL, "VOL"), (349.0, U_PITCH, "PITCH"), (376.0, U_PHS, "PHS")] {
        fader(ui, Field::Int(f), x, SOUND_TOP, SOUND_BOT, SOUND_FADER);
        caption(ui, x, ROW_CAPTION, 25.0, l);
    }
}

fn filter(ui: &mut Ui) {
    lemon::plug_panel(ui.c, FILTER);
    ui.title(410.0, ROW_TITLE, "FILTER");
    for (k, l) in ["LP", "BP", "BS", "HP"].into_iter().enumerate() {
        cell(ui, Field::Int(F_TYPE), k as i32, 475.0 + 22.0 * k as f32, 109.0, l);
    }
    for (k, l) in ["OF", "x1", "x2", "x3"].into_iter().enumerate() {
        cell(ui, Field::Int(F_MODE), k as i32, 475.0 + 22.0 * k as f32, 129.0, l);
    }
    lamp(ui, Field::Int(FLAT), 429.0, 188.5, "", false, 0.0);
    ui.label(SMALL, 429.0, ROW_CAPTION, DIM, "FLAT");
    knob(ui, Field::Int(CUT), 485.0, 173.0, 22.0);
    caption(ui, 485.0, ROW_CAPTION, 40.0, "CUT");
    knob(ui, Field::Int(RES), 533.0, 176.0, 17.0);
    caption(ui, 533.0, ROW_CAPTION, 30.0, "RES");
}

// ---------------------------------------------------------------------------------------------------------------
// The articulator editor

pub const TARGET_BOX: Rect = edges(10.0, 174.0, 221.0, 240.0);
pub const PART_BOX: Rect = edges(206.0, 370.0, 221.0, 240.0);
pub const COMB_BOX: Rect = edges(400.5, 437.5, 221.0, 240.0);
pub const CURVE: Rect = edges(10.0, 437.5, 245.5, 389.0);
const BAR_Y0: f32 = 391.0;
const TOOLS_Y: f32 = 415.5;

/// A box that opens a menu: its text from the left, an arrow at its right end.
fn menu_box(ui: &mut Ui, r: Rect, t: Target, text: &str) {
    let st = ui.state(r, &t);
    lemon::field(ui.c, r, st);
    ui.left(LABEL, r.x + 5.0, r.cy(), if st.hover { TEXT } else { ACCENT }, text);
    triangle_right(ui, r.right() - 7.0, r.cy(), if st.hover { TEXT } else { DIM });
    ui.hit(r, t);
}

fn editor_card(ui: &mut Ui) {
    lemon::plug_panel(ui.c, EDITOR);
    let t = ui.st.target.min(TARGETS - 1);
    menu_box(ui, TARGET_BOX, Target::TargetBox, TARGET_NAMES[t]);
    // From the target to its part.
    let (ax, ay) = (190.5, 230.0);
    ui.c.fill_rect(Rect::new(ax - 6.0, ay - 1.0, 7.0, 2.0), FAINT);
    ui.c.fill_polygon(&[(ax + 1.0, ay - 4.0), (ax + 6.0, ay), (ax + 1.0, ay + 4.0)], FAINT);
    let id = shown_curve(ui.st);
    let shown = if let CurveId::Part(_, p) = id { p } else { 0 };
    menu_box(ui, PART_BOX, Target::PartBox, part_name(t, shown));
    let (ax, ay) = (385.5, 230.0);
    ui.c.fill_rect(Rect::new(ax - 1.0, ay - 6.0, 2.0, 7.0), FAINT);
    ui.c.fill_polygon(&[(ax - 4.0, ay + 1.0), (ax + 4.0, ay + 1.0), (ax, ay + 6.0)], FAINT);
    if t >= ARTS {
        let f = Field::Comb(t - ARTS);
        let v = ctl::shown(ui.m.st, f);
        let tg = Target::Comb;
        let mut st = ui.state(COMB_BOX, &tg);
        st.on = v != 0;
        let ink = lemon::plug_button(ui.c, COMB_BOX, 5.0, st);
        ui.c.icon(&tb::PIANO, Rect::new(COMB_BOX.x + 3.0, COMB_BOX.cy() - 6.0, 12.0, 12.0), ink);
        ui.c.text_center(SMALL.bold(), Rect::new(COMB_BOX.x + 15.0, COMB_BOX.y, COMB_BOX.w - 16.0, COMB_BOX.h), ink, &ctl::text(f, v));
        ui.hit(COMB_BOX, tg);
    }
    let c = curve_of(ui.m.st, id);
    curve_view(ui, c, id);
    let bar = matches!(id, CurveId::Part(_, 0 | 1));
    let x0 = if bar { 164.0 } else { CURVE.x };
    lemon::plug_well(ui.c, edges(x0, CURVE.right(), 408.0, 423.0), 3.0);
    if let CurveId::Part(a, p @ (0 | 1)) = id {
        lemon::plug_well(ui.c, edges(CURVE.x, 162.0, BAR_Y0, 423.0), 4.0);
        switch_part(ui, a, p);
    }
    scroll_bar(ui, x0);
    tools(ui, x0 + 6.5);
}

/// The bar's envelope or LFO settings, left of the scroll bar. An envelope has no GLOBAL switch.
fn switch_part(ui: &mut Ui, a: usize, p: usize) {
    lamp(ui, Field::Part(a, p, 0), 18.0, 399.5, "", false, 0.0);
    let labels = if p == 0 { ["ATT", "DEC", "SUS", "REL"] } else { ["SPD", "TNS", "SKEW", "PW"] };
    for (k, l) in labels.into_iter().enumerate() {
        let x = 38.0 + 23.0 * k as f32;
        knob(ui, Field::Part(a, p, k + 1), x, 403.0, 9.5);
        ui.label(TINY, x, 417.0, DIM, l);
    }
    lamp_in(ui, TINY, Field::Part(a, p, 5), 131.5, 399.5, "TEMPO", false, 22.0);
    if p == 1 {
        lamp_in(ui, TINY, Field::Part(a, p, 6), 131.5, 415.5, "GLOBAL", false, 22.0);
    }
}

/// The graph has no zoom, so there is nothing to scroll: the thumb fills the track.
fn scroll_bar(ui: &mut Ui, x0: f32) {
    let (x1, y0, y1) = (CURVE.right(), BAR_Y0, BAR_Y0 + 15.0);
    for (l, r, right) in [(x0, x0 + 14.0, false), (x1 - 14.0, x1, true)] {
        ui.c.fill_rrect(edges(l, r, y0, y1), 3.0, plug::FIELD);
        let (cx, cy) = ((l + r) / 2.0, (y0 + y1) / 2.0);
        let d = if right { 2.5 } else { -2.5 };
        ui.c.fill_polygon(&[(cx - d, cy - 3.5), (cx + d, cy), (cx - d, cy + 3.5)], FAINT);
    }
    let track = edges(x0 + 15.0, x1 - 15.0, y0, y1);
    ui.c.fill_rrect(track, 3.0, plug::TRACK);
    ui.c.fill_rrect(track.inset(1.0), 2.5, Color::rgb(0x3c3c3c));
}

pub const TOOL_X: [f32; 4] = [353.5, 378.0, 402.0, 425.5];
pub const TOOL_NAMES: [&str; 4] = ["Freeze editing / hide points", "Step editing", "Snap to grid", "Slide succeeding points / regions"];

fn tools(ui: &mut Ui, options_x: f32) {
    let r = Rect::new(options_x - 7.0, TOOLS_Y - 7.5, 14.0, 15.0);
    let t = Target::CurveOptions;
    let hot = ui.hot(r, &t);
    triangle_down(ui, options_x, TOOLS_Y, if hot { TEXT } else { DIM });
    ui.hit(r, t);
    for (k, x) in TOOL_X.into_iter().enumerate() {
        let r = Rect::new(x - 11.0, TOOLS_Y - 7.5, 22.0, 15.0);
        let t = Target::Tool(k);
        let mut st = ui.state(r, &t);
        // Step editing does nothing yet.
        st.disabled = k == 1;
        let ink = lemon::choice_button(ui.c, r, ui.st.tools[k], ACCENT, st);
        tool_icon(ui, k, x, TOOLS_Y, ink);
        ui.hit(r, t);
    }
}

/// Freeze a snowflake, step a pencil, snap a magnet, slide a double arrow.
fn tool_icon(ui: &mut Ui, k: usize, x: f32, y: f32, ink: Color) {
    match k {
        0 => {
            for a in 0..6 {
                let a = a as f32 * std::f32::consts::FRAC_PI_3 + std::f32::consts::FRAC_PI_6;
                let (dx, dy) = (a.cos(), a.sin());
                ui.c.line(x, y, x + 6.0 * dx, y + 6.0 * dy, 1.0, ink);
                // The arm's two barbs.
                let (mx, my) = (x + 3.5 * dx, y + 3.5 * dy);
                let (b0, b1) = (a + 0.9, a - 0.9);
                ui.c.line(mx, my, mx + 2.2 * b0.cos(), my + 2.2 * b0.sin(), 1.0, ink);
                ui.c.line(mx, my, mx + 2.2 * b1.cos(), my + 2.2 * b1.sin(), 1.0, ink);
            }
        }
        1 => ui.c.icon(&tb::PENCIL, Rect::new(x - 6.0, y - 6.0, 12.0, 12.0), ink),
        2 => ui.c.icon(&tb::MAGNET, Rect::new(x - 6.0, y - 6.0, 12.0, 12.0), ink),
        _ => ui.c.icon(&tb::ARROWS_HORIZONTAL, Rect::new(x - 7.0, y - 7.0, 14.0, 14.0), ink),
    }
}

pub fn curve_inner() -> Rect {
    CURVE.inset(1.0)
}

/// The time the curve view spans: a map 0 .. 1; an envelope or LFO Ogun's default span, or its length and a tenth more
/// when that is longer.
pub fn curve_span(c: &Curve) -> f64 {
    if c.kind == MAP {
        return 1.0;
    }
    let least = if c.kind == LFO { 4.25 } else { 3.5 };
    (edit::times(c).last().copied().unwrap_or(0.0) * 1.1).max(least)
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

/// Where point `i` of `c` sits in the curve view.
pub fn point_xy(c: &Curve, i: usize, span: f64) -> (f32, f32) {
    (t_to_x(edit::times(c)[i], span), level_to_y(c.points[i].level))
}

/// The graph's ground and grid: rows 9 px apart; columns by what is shown (`plugins/ogun.md`, "The form"). On a final
/// map, the part past line n is a lighter zone.
fn graph_ground(ui: &mut Ui, id: CurveId, span: f64) {
    let r = CURVE;
    lemon::plug_well(ui.c, r, 4.0);
    // The upper half a shade lighter.
    let mid = (r.y + r.h / 2.0).round();
    ui.c.fill_rect(edges(r.x + 3.0, r.right() - 3.0, r.y + 1.0, r.y + 3.0), Color::rgb(0x232323));
    ui.c.fill_rect(edges(r.x + 1.0, r.right() - 1.0, r.y + 3.0, mid), Color::rgb(0x232323));
    let inner = curve_inner();
    if let CurveId::Final(_) = id {
        let rich = ui.m.st.ints[RICH];
        let t = if ui.m.st.natural_scale { (rich - 1) as f32 / 15.0 } else { ((1u32 << (rich - 1)) - 1) as f32 / 32767.0 };
        let x = (inner.x + inner.w * t.clamp(0.0, 1.0)).round();
        ui.c.fill_rect(edges(x, r.right() - 1.0, r.y + 1.0, r.bottom() - 1.0), Color::rgb(0xffffff).with_alpha(9));
    }
    let col = |ui: &mut Ui, x: f32, heavy: bool| {
        ui.c.fill_rect(Rect::new(x.round(), r.y + 1.0, 1.0, r.h - 2.0), if heavy { GRID_HEAVY } else { GRID_LIGHT });
    };
    match id {
        CurveId::Final(_) => {
            for k in 1..15 {
                col(ui, inner.x + inner.w * k as f32 / 15.0, false);
            }
        }
        // 120 keys: each C heavy, each black key light.
        CurveId::Part(_, 2) => {
            for key in 1..120 {
                let x = inner.x + inner.w * key as f32 / 120.0;
                if key % 12 == 0 {
                    col(ui, x, true);
                } else if is_black(key) {
                    col(ui, x, false);
                }
            }
        }
        CurveId::Part(_, 3..) => {
            for k in 1..16 {
                col(ui, inner.x + inner.w * k as f32 / 16.0, k % 4 == 0);
            }
        }
        // Time: every .2 unit, each 2 heavy; coarser where the lines would crowd.
        CurveId::Part(..) => {
            let mut step = 0.2;
            while step * f64::from(inner.w) / span < 6.0 {
                step *= 5.0;
            }
            let mut k = 1;
            while k as f64 * step < span {
                col(ui, t_to_x(k as f64 * step, span), k % 10 == 0);
                k += 1;
            }
        }
    }
    let heavy = |k: i32| if let CurveId::Final(_) = id { k == 8 } else { k % 4 == 0 };
    for k in 1..16 {
        let y = (r.y - 0.5 + 9.0 * k as f32).round();
        ui.c.fill_rect(Rect::new(r.x + 1.0, y, r.w - 2.0, 1.0), if heavy(k) { GRID_HEAVY } else { GRID_LIGHT });
    }
}

/// What an LFO plays, in a light line over the whole width: the curve's level about its centre times the wave, as the
/// knobs set it, from the curve's start.
fn lfo_output(ui: &mut Ui, c: &Curve, ts: &[f64], span: f64) {
    let inner = curve_inner();
    let w = inner.w.round() as usize;
    let step = span / w as f64;
    let shape = LfoShape::new(c, step);
    let dphi = if shape.frozen { 0 } else { shape.dphi };
    let mut i = 1;
    let line: Vec<(f32, f32)> = (0..=w)
        .map(|px| {
            let t = step * px as f64;
            while i + 1 < ts.len() && ts[i] < t {
                i += 1;
            }
            let level = if t >= *ts.last().unwrap_or(&0.0) || ts.len() < 2 {
                c.points.last().map_or(0.5, |p| p.level)
            } else {
                let d = ts[i] - ts[i - 1];
                edit::segment_level(c, i, if d > 0.0 { (t - ts[i - 1]) / d } else { 1.0 })
            };
            let phase = (i64::from(shape.start)).wrapping_add(px as i64 * dphi) as u32;
            (inner.x + px as f32 * inner.w / w as f32, level_to_y(0.5 + (level - 0.5) * shape.wave(phase)))
        })
        .collect();
    ui.c.polyline(&line, 1.0, Color::rgb(0xa8a8a8));
}

/// The curve view: the grid, the curve with the area below it tinted, its points (filled), its tension handles (hollow,
/// halfway along their segments) and its markers.
fn curve_view(ui: &mut Ui, c: &Curve, id: CurveId) {
    let r = CURVE;
    let map = c.kind == MAP;
    let span = ui.st.span.unwrap_or_else(|| curve_span(c));
    graph_ground(ui, id, span);
    ui.hit(r, Target::CurveArea);
    let inner = curve_inner();
    let ts = edit::times(c);
    let n = ts.len();
    if n == 0 {
        return;
    }
    let many = n > MANY_POINTS;
    let mut line = vec![(t_to_x(0.0, span), level_to_y(c.points[0].level))];
    if many {
        // One reading per pixel.
        let mut i = 1;
        let w = inner.w as usize;
        for px in 1..=w {
            let t = span * px as f64 / w as f64;
            while i + 1 < n && ts[i] < t {
                i += 1;
            }
            line.push((t_to_x(t, span), level_to_y(c.points[i].level)));
        }
    } else {
        for i in 1..n {
            let (x0, x1) = (t_to_x(ts[i - 1], span), t_to_x(ts[i], span));
            let steps = ((x1 - x0) / 2.0).ceil().clamp(1.0, 200.0) as usize;
            for s in 1..=steps {
                let x = s as f64 / steps as f64;
                line.push((x0 + (x1 - x0) * x as f32, level_to_y(edit::segment_level(c, i, x))));
            }
            line.push((x1, level_to_y(c.points[i].level)));
        }
    }
    let ink = if c.enabled || map { ACCENT } else { FAINT };
    let mut area = line.clone();
    area.push((line.last().unwrap().0, inner.bottom()));
    area.push((line[0].0, inner.bottom()));
    ui.c.fill_polygon(&area, ink.with_alpha(22));
    if c.kind == LFO && !many {
        lfo_output(ui, c, &ts, span);
    }
    ui.c.polyline(&line, 1.5, ink);
    for (i, tag) in [(c.decay, "D"), (c.sustain, "S"), (c.loop_start, "L")] {
        if let Some(&t) = usize::try_from(i).ok().and_then(|i| ts.get(i)) {
            let x = t_to_x(t, span).round();
            ui.c.fill_rect(Rect::new(x, r.y + 1.0, 1.0, r.h - 2.0), plug::TICK);
            let tag_r = Rect::new(x - 5.0, r.bottom() - 15.0, 11.0, 11.0);
            ui.c.fill_rrect(tag_r, 3.0, plug::FACE);
            ui.c.text_center(SMALL.bold(), tag_r, TEXT, tag);
        }
    }
    if many {
        ui.c.text_right(SMALL, r.right() - 8.0, r.y + 4.0, DIM, &format!("{n} points"));
        return;
    }
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
        let fill = if hot { brand::HIGHLIGHT } else { ink };
        let rad = if hot { 4.0 } else { 3.0 };
        ui.c.fill_circle(x, y, rad + 1.0, WELL);
        ui.c.fill_circle(x, y, rad, fill);
        ui.hit(hr, tg);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The logo, the EQ

fn logo(ui: &mut Ui) {
    emblem(ui, 550.0, LOGO.cy(), 1.0, Font::condensed(20, 0).bold(), 6.0);
    ui.hit(LOGO, Target::Logo);
}

/// A band's centre line in the EQ card.
pub const EQ_X: [f32; 3] = [474.0, 506.0, 538.0];
const EQ_ICON_X: [f32; 3] = [474.0, 505.5, 538.0];

pub fn eq_type_box(b: usize) -> Rect {
    Rect::new(EQ_ICON_X[b] - 15.0, 254.0, 30.0, 22.0)
}

fn eq(ui: &mut Ui) {
    lemon::plug_panel(ui.c, EQ);
    lemon::plug_well(ui.c, edges(449.0, 553.0, 254.0, 276.0), 5.0);
    ui.title(452.0, 265.0, "EQ");
    let look = FaderLook { long: 6.0, short: 4.0, lit: None };
    for b in 0..3 {
        let r = eq_type_box(b);
        let t = Target::EqType(b);
        let st = ui.state(r, &t);
        eq_icon(ui, r, ctl::shown(ui.m.st, Field::EqType(b)), if st.hover { TEXT } else { ACCENT });
        ui.hit(r, t);
        let x = EQ_X[b];
        fader(ui, Field::Int(EQ_LEVEL + b), x, 282.0, 354.0, look);
        knob(ui, Field::Int(EQ_FREQ + b), x, 378.0, 14.0);
        knob(ui, Field::Int(EQ_WIDTH + b), x, 412.0, 14.0);
    }
}

/// A band type's shape: low shelf, peak, high shelf.
fn eq_icon(ui: &mut Ui, r: Rect, ty: i32, ink: Color) {
    let (x, y) = (r.cx(), r.cy());
    let pts: Vec<(f32, f32)> = match ty {
        0 => vec![(x - 8.0, y - 3.0), (x - 2.0, y - 3.0), (x + 2.0, y + 3.0), (x + 8.0, y + 3.0)],
        1 => vec![(x - 8.0, y + 3.0), (x - 4.0, y + 3.0), (x, y - 4.0), (x + 4.0, y + 3.0), (x + 8.0, y + 3.0)],
        _ => vec![(x - 8.0, y + 3.0), (x - 2.0, y + 3.0), (x + 2.0, y - 3.0), (x + 8.0, y - 3.0)],
    };
    ui.c.polyline(&pts, 1.5, ink);
}

// ---------------------------------------------------------------------------------------------------------------
// The effects

/// A box with its caption under it on one plate, one hit area: chorus ORD, reverb COL.
fn stacked_box(ui: &mut Ui, x0: f32, x1: f32, f: Field, label: &str) {
    let hit = edges(x0, x1, 458.0, 495.0);
    ui.c.fill_rrect(hit, 4.0, PLATE);
    value_box(ui, edges(x0 + 3.0, x1 - 3.0, 461.0, 481.0), hit, f, false);
    ui.label(SMALL, hit.cx(), 488.0, DIM, label);
}

fn fx_fader(ui: &mut Ui, f: usize, x: f32, label: &str) {
    fader(ui, Field::Int(f), x, FX_TOP, FX_BOT, FX_FADER);
    caption(ui, x, FX_CAPTION, 20.0, label);
}

fn chorus(ui: &mut Ui) {
    lemon::plug_panel(ui.c, CHORUS);
    let base = crate::state::CHORUS;
    ui.title(6.0, FX_TITLE, "CHORUS");
    stacked_box(ui, 5.0, 32.0, Field::Int(base), "ORD");
    for (k, l) in ["DP", "SP", "DL", "SR", "CR", "VL"].into_iter().enumerate() {
        fx_fader(ui, base + 1 + k, 46.0 + 22.0 * k as f32, l);
    }
}

fn delay(ui: &mut Ui) {
    lemon::plug_panel(ui.c, DELAY);
    let base = crate::state::DELAY;
    ui.title(187.0, FX_TITLE, "DELAY");
    lamp(ui, Field::Flag(Flag::DelayOn), 273.0, FX_TITLE, "ON", true, 14.0);
    lamp(ui, Field::Flag(Flag::DelayTempo), 291.0, FX_TITLE, "TEMPO", false, 31.0);
    // The mode: a 3-way slider with a disc on its stop.
    let f = Field::Int(base + 4);
    lemon::plug_well(ui.c, edges(187.0, 202.0, 459.0, 506.0), 7.5);
    let on = ctl::get(ui.m.st, f);
    for (k, l) in ["NORM", "INV", "PONG"].into_iter().enumerate() {
        let y = 467.5 + 16.0 * k as f32;
        let r = edges(187.0, 244.0, y - 8.0, y + 8.0);
        let t = Target::Choice(f, k as i32);
        let st = ui.state(r, &t);
        let sel = on == k as i32;
        if sel {
            lemon::dial_disc(ui.c, 194.5, y, 6.0, st);
            ui.c.fill_circle(194.5, y, 2.0, ACCENT);
        }
        ui.left(SMALL, 206.0, y, if sel { ACCENT } else if st.hover { TEXT } else { DIM }, l);
        ui.hit(r, t);
    }
    for (k, l) in ["FB", "TM", "SO", "VL"].into_iter().enumerate() {
        fx_fader(ui, base + k, 255.0 + 22.0 * k as f32, l);
    }
}

fn reverb(ui: &mut Ui) {
    lemon::plug_panel(ui.c, REVERB);
    let base = crate::state::REVERB;
    ui.title(347.0, FX_TITLE, "REVERB");
    lamp(ui, Field::Flag(Flag::ReverbOn), 495.0, FX_TITLE, "ON", true, 14.0);
    lamp(ui, Field::Flag(Flag::ReverbTempo), 513.0, FX_TITLE, "TEMPO", false, 31.0);
    stacked_box(ui, 346.0, 373.0, Field::Int(base + 7), "COL");
    for (k, (i, l)) in [(0, "LC"), (1, "HC"), (2, "PD"), (3, "RS"), (4, "DF"), (5, "DE"), (6, "HD"), (8, "VL")].into_iter().enumerate() {
        fx_fader(ui, base + i, 389.0 + 22.0 * k as f32, l);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The testing keyboard

const WHITE: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];
/// The black keys' left edges in an octave.
const BLACK_X: [f32; 5] = [11.0, 33.0, 71.0, 92.0, 113.0];
const OCTAVE: f32 = 140.0;
const KEYS_Y0: f32 = 556.0;
const KEYS_Y1: f32 = 638.0;
const BAND_Y1: f32 = 569.0;
const BLACK_Y1: f32 = 616.0;

/// Key `k`'s rect; black keys over the whites.
pub fn key_rect(k: u8) -> Rect {
    let x = OCTAVE * f32::from((k - FIRST_KEY) / 12);
    let n = (k - FIRST_KEY) % 12;
    match WHITE.iter().position(|&w| w == n) {
        Some(i) => edges(x + 20.0 * i as f32, x + 20.0 * (i + 1) as f32, KEYS_Y0, KEYS_Y1),
        None => {
            let j = [1, 3, 6, 8, 10].iter().position(|&b| b == n).unwrap_or(0);
            Rect::new(x + BLACK_X[j], BAND_Y1, 13.0, BLACK_Y1 - BAND_Y1)
        }
    }
}

pub fn is_black(k: u8) -> bool {
    !WHITE.contains(&(k % 12))
}

/// A key's name (key 60 = C5).
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
            // The whites' tops lie under the label band, which is part of each key's hit area.
            let shown = if black { r } else { edges(r.x, r.right(), BAND_Y1, r.bottom()) };
            ui.c.fill_rrect(Rect::new(shown.x + 0.5, shown.y, shown.w - 1.0, shown.h), if black { 2.5 } else { 3.0 }, fill);
            if !black && k % 12 == 0 && !held {
                // The C keys' shaded foot.
                ui.c.fill_rrect(Rect::new(r.x + 0.5, BLACK_Y1, r.w - 1.0, KEYS_Y1 - BLACK_Y1), 3.0, Color::rgb(0xc4c4c4));
            }
            ui.hit(r, Target::Key(k));
        }
    }
    // The label band over the whites: the C keys' names.
    ui.c.fill_rect(edges(0.0, W as f32, KEYS_Y0, BAND_Y1), Color::rgb(0xbababa));
    for k in (FIRST_KEY..=LAST_KEY).step_by(12) {
        ui.left(SMALL.bold(), key_rect(k).x + 1.0, (KEYS_Y0 + BAND_Y1) / 2.0, Color::rgb(0x4a4a4a), &key_name(k));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// About

fn info(ui: &mut Ui) {
    let (w, h) = ui.c.size();
    let mid = emblem(ui, w / 2.0, h / 2.0 - 45.0, 0.5, Font::condensed(32, 0).bold(), 9.0);
    let v = format!("Version {}", ui.m.version);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 8.0, w, 16.0), DIM, &v);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 26.0, w, 16.0), DIM, "Lemon Studio's additive synthesizer");
    ui.c.text_center(SMALL, Rect::new(0.0, h - 30.0, w, 16.0), FAINT, "Click to return");
    ui.hit(Rect::new(0.0, 0.0, w, h), Target::Cover);
}
