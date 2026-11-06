// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Smith's editor: painting and hit regions (platform independent). Ogun's controls on a 6 px grid in the Lemon "Pith"
//! plugin look: the header with the emblem and the patch's words, the master, timbre, unison and filter cards beside
//! the X / Y pad, the articulator editor beside the EQ, the effects, and the testing keyboard.
//!
// spec: plugins/smith.md, plugins/ogun.md ("Controls and ranges"), ui/lemon-design.md §11.1

use citrus::edit;
use os_dsp::curve::{Curve, MAP};
use os_theme::design::lemon as brand;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::State as Look;
use os_ui::{Canvas, Color, Font, Rect};

use crate::ctl::{self, Field, Flag};
use crate::state::*;

/// Editor size in logical pixels.
pub const W: u32 = 852;
pub const H: u32 = 564;

pub const BG: Color = plug::BG;
const WELL: Color = plug::WELL;
pub const TEXT: Color = brand::TEXT;
const DIM: Color = plug::DIM;
const FAINT: Color = plug::FAINT;
pub const ACCENT: Color = brand::MAIN;

const LABEL: Font = Font::new(10);
const SMALL: Font = Font::new(9);

/// The cards' title band.
const BAND: f32 = 15.0;

/// A rect from its left, right, top and bottom edges: every card is 6 px from the editor's edges and its neighbours.
const fn edges(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// The header: the emblem, the patch's words, the round buttons on one centre line.
const HEADER_Y: f32 = 21.0;
pub const LOGO: Rect = edges(6.0, 138.0, 6.0, 36.0);
pub const WORDS: Rect = edges(144.0, 768.0, 8.0, 34.0);
pub const HEADER_X: [f32; 3] = [785.0, 810.0, 835.0];
pub const HEADER_NAMES: [&str; 3] = ["Generate random preset", "Options", "About"];

pub const MASTER: Rect = edges(6.0, 156.0, 42.0, 114.0);
pub const TIMBRE: Rect = edges(162.0, 648.0, 42.0, 114.0);
pub const UNISON: Rect = edges(6.0, 252.0, 120.0, 192.0);
pub const FILTER: Rect = edges(258.0, 510.0, 120.0, 192.0);
pub const TONE: Rect = edges(516.0, 648.0, 120.0, 192.0);
pub const MODS: Rect = edges(654.0, 846.0, 42.0, 192.0);
pub const PAD: Rect = edges(660.0, 786.0, 60.0, 186.0);
pub const EDITOR_BAR: Rect = edges(6.0, 594.0, 198.0, 222.0);
pub const CURVE: Rect = edges(6.0, 594.0, 226.0, 350.0);
pub const BAR: Rect = edges(6.0, 594.0, 354.0, 390.0);
pub const EQ: Rect = edges(600.0, 846.0, 198.0, 390.0);
pub const CHORUS: Rect = edges(6.0, 270.0, 396.0, 468.0);
pub const DELAY: Rect = edges(276.0, 534.0, 396.0, 468.0);
pub const REVERB: Rect = edges(540.0, 846.0, 396.0, 468.0);
pub const KEYS: Rect = edges(6.0, 846.0, 474.0, 558.0);

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
    "EQ shifting",
    "Forced harmonics",
    "Impulse EQ",
    "Decay EQ",
    "Release EQ",
];
pub const PART_LABELS: [&str; 6] = ["ENV", "LFO", "KEY", "VEL", "MOD X", "MOD Y"];
pub const PART_NAMES: [&str; 6] = ["Envelope", "LFO", "Keyboard", "Velocity", "Mod X", "Mod Y"];

/// The parts target `t` has: articulation targets all six (Volume stops at Velocity), mapping targets the four maps,
/// the final maps none.
pub fn parts_of(t: usize) -> &'static [usize] {
    match t {
        A_VOLUME => &[0, 1, 2, 3],
        A_CUT | A_RES | A_PITCH | A_U_PITCH => &[0, 1, 2, 3, 4, 5],
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

/// The testing keyboard's keys.
pub const FIRST_KEY: u8 = 36;
pub const LAST_KEY: u8 = 95;

/// A curve with more points than this (a resynthesized map) is drawn as a line, without handles.
pub const MANY_POINTS: usize = 512;

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Knob(Field),
    Steps(Field),
    Switch(Field),
    /// One choice of a set: the setting and the value it sets.
    Choice(Field, i32),
    Pad,
    /// The articulator editor's target menu.
    TargetBox,
    PartTab(usize),
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
}

/// Paints the editor. `hint` (when the host does not show it) goes on the patch words' second line.
pub fn paint(c: &mut Canvas, m: &Model, st: &ViewState, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(BG);
    if st.info {
        info(&mut ui);
        return ui.hits;
    }
    header(&mut ui, hint);
    master(&mut ui);
    timbre(&mut ui);
    unison(&mut ui);
    filter(&mut ui);
    tone(&mut ui);
    modulation(&mut ui);
    editor_card(&mut ui);
    eq(&mut ui);
    chorus(&mut ui);
    delay(&mut ui);
    reverb(&mut ui);
    keyboard(&mut ui);
    ui.hits
}

// ---------------------------------------------------------------------------------------------------------------
// Widgets

/// A card with its title centred in a header band and a line under the band.
fn titled(ui: &mut Ui, r: Rect, title: &str) {
    lemon::plug_panel(ui.c, r);
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + BAND, r.w - 2.0, 1.0), plug::PANEL_LINE);
    ui.label(SMALL.bold(), r.cx(), r.y + BAND / 2.0, DIM, title);
}

/// The lemon mark and "SMITH" in the wordmark's grey condensed caps, from `x` or centred on it, on the line `cy`.
fn emblem(ui: &mut Ui, x: f32, cy: f32, centred: bool, f: Font, s: f32) -> Rect {
    let wn = lemon::spaced_width(ui.c, f, "SMITH", 3.0);
    let (wl, gap) = (2.2 * s, (s * 1.3).round());
    let w = wl + gap + wn;
    let x = if centred { (x - w / 2.0).round() } else { x };
    let lh = ui.c.line_height(f);
    lemon::lemon_mark(ui.c, x + wl / 2.0, cy, s, brand::MAIN);
    lemon::spaced(ui.c, f, x + wl + gap, (cy - lh / 2.0).round(), plug::WORDMARK, "SMITH", 3.0);
    Rect::new(x, cy - lh / 2.0, w, lh)
}

/// A knob of setting `f`, radius `r`, its caption under it.
fn knob(ui: &mut Ui, f: Field, cx: f32, cy: f32, r: f32, label: &str) {
    let hr = Rect::new(cx - r - 2.0, cy - r - 2.0, 2.0 * r + 4.0, 2.0 * r + 4.0);
    let w = (r * 0.2).clamp(1.8, 3.2);
    let t = Target::Knob(f);
    let look = Look::hover(ui.hot(hr, &t));
    let v = ctl::shown(ui.m.st, f);
    lemon::dial(ui.c, cx, cy, r, w, ctl::norm(f, v), ctl::is_bipolar(f), ACCENT, look);
    if !label.is_empty() {
        ui.label(SMALL, cx, cy + r + 8.0, DIM, label);
    }
    ui.hit(hr, t);
}

/// A switch at (cx, cy): the kit's rounded-square toggle, its label to the right.
fn lamp(ui: &mut Ui, f: Field, cx: f32, cy: f32, label: &str) {
    let on = ctl::get(ui.m.st, f) != 0;
    let t = Target::Switch(f);
    let w = ui.c.text_width(SMALL, label);
    let hr = Rect::new(cx - 7.0, cy - 7.0, w + 18.0, 14.0);
    let st = ui.state(hr, &t);
    lemon::toggle(ui.c, cx, cy, on, st);
    let lh = ui.c.line_height(SMALL);
    ui.c.text(SMALL, cx + 10.0, (cy - lh / 2.0).round(), if on { TEXT } else { DIM }, label);
    ui.hit(hr, t);
}

/// A box showing a value.
fn value_box(ui: &mut Ui, r: Rect, t: Target, text: &str) {
    let st = ui.state(r, &t);
    lemon::field(ui.c, r, st);
    let col = if st.hover { TEXT } else { ACCENT };
    let (w, lh) = (lemon::spaced_width(ui.c, SMALL, text, 1.0), ui.c.line_height(SMALL));
    lemon::spaced(ui.c, SMALL, (r.cx() - w / 2.0).round(), (r.cy() - lh / 2.0).round(), col, text, 1.0);
    ui.hit(r, t);
}

/// A tab: the chosen one a dark rounded pill with its name in lemon (Pith's sets of choices), a lemon dot on a part
/// that is on.
fn tab(ui: &mut Ui, r: Rect, t: Target, label: &str, sel: bool, dot: bool) {
    let hot = ui.hot(r, &t);
    let pill = r.inset(0.5);
    if sel {
        lemon::outlined(ui.c, pill, 5.0, plug::FIELD_LINE, plug::FIELD);
    } else if hot {
        ui.c.fill_rrect(pill, 5.0, Color::rgb(0x3a3a3a));
    }
    let col = if sel { ACCENT } else if hot { TEXT } else { DIM };
    ui.c.text_center(SMALL.bold(), r, col, label);
    if dot {
        ui.c.fill_circle(r.x + 6.0, r.cy(), 2.0, ACCENT);
    }
    ui.hit(r, t);
}

/// A choice of setting `f` as a tab.
fn choice(ui: &mut Ui, f: Field, v: i32, r: Rect, label: &str) {
    let sel = ctl::get(ui.m.st, f) == v;
    tab(ui, r, Target::Choice(f, v), label, sel, false);
}

// ---------------------------------------------------------------------------------------------------------------
// The header

fn header(ui: &mut Ui, hint: &str) {
    emblem(ui, LOGO.x + 3.0, HEADER_Y, false, Font::condensed(18, 0).bold(), 6.0);
    ui.hit(LOGO, Target::Logo);
    // The patch's words, the hint over the second line.
    lemon::plug_well(ui.c, WORDS, 6.0);
    let [title, author, more] = ui.m.st.comment_lines();
    let top = [title.as_str(), author.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("  -  ");
    let (x, w) = (WORDS.x + 8.0, WORDS.w - 16.0);
    ui.c.text_clip(LABEL, x, WORDS.y + 1.0, w, TEXT, &top);
    let (second, col) = if hint.is_empty() { (more.as_str(), DIM) } else { (hint, ACCENT) };
    let plain = second.replace('\t', "   ");
    ui.c.text_clip(SMALL, x, WORDS.y + 14.0, w, col, &plain);
    for (i, x) in HEADER_X.into_iter().enumerate() {
        let (y, rad) = (HEADER_Y, 11.0);
        let t = Target::Header(i);
        let r = Rect::new(x - rad, y - rad, 2.0 * rad, 2.0 * rad);
        let st = ui.state(r, &t);
        let ink = lemon::plug_button(ui.c, r, 11.0, st);
        match i {
            // A die's face.
            0 => {
                ui.c.stroke_rrect(Rect::new(x - 5.0, y - 5.0, 10.0, 10.0), 2.0, 1.0, ink);
                for (dx, dy) in [(-2.5, -2.5), (2.5, 2.5), (0.0, 0.0)] {
                    ui.c.fill_circle(x + dx, y + dy, 1.0, ink);
                }
            }
            1 => ui.c.fill_polygon(&[(x - 4.5, y - 2.0), (x + 4.5, y - 2.0), (x, y + 3.0)], ink),
            _ => ui.c.text_center(Font::new(13).bold(), r, ink, "?"),
        }
        ui.hit(r, t);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The top cards

/// The first row's and the effects' knob line.
const KNOB_LINE: f32 = 36.0;

fn master(ui: &mut Ui) {
    titled(ui, MASTER, "MASTER");
    let y = MASTER.y + KNOB_LINE;
    for (k, (f, l)) in [(LEVEL, "LEVEL"), (COARSE, "COARSE"), (FINE, "FINE")].into_iter().enumerate() {
        knob(ui, Field::Int(f), 33.0 + 48.0 * k as f32, y, 16.0, l);
    }
}

fn timbre(ui: &mut Ui) {
    titled(ui, TIMBRE, "TIMBRE");
    let r = TIMBRE;
    let y = r.y + KNOB_LINE;
    for (k, (f, l)) in [(RICH, "RICH"), (PRE, "PRE"), (DEC, "DEC"), (REL, "REL"), (FULL, "FULL"), (MORPH, "")].into_iter().enumerate() {
        knob(ui, Field::Int(f), r.x + 30.0 + 48.0 * k as f32, y, 16.0, l);
    }
    // 1 > 2: a small arrow between the digits.
    let (cx, cy) = (r.x + 30.0 + 48.0 * 5.0, y + 24.0);
    ui.label(SMALL, cx - 6.0, cy, DIM, "1");
    ui.c.fill_polygon(&[(cx - 2.0, cy - 3.0), (cx + 2.0, cy), (cx - 2.0, cy + 3.0)], DIM);
    ui.label(SMALL, cx + 6.0, cy, DIM, "2");
    for (x, f, l) in [(468.0, SEED2, "SEED 2"), (522.0, SEED1, "SEED 1")] {
        let f = Field::Int(f);
        ui.label(SMALL, x + 24.0, r.y + 28.0, DIM, l);
        let text = ctl::text(f, ctl::shown(ui.m.st, f));
        value_box(ui, Rect::new(x, r.y + 37.0, 48.0, 18.0), Target::Steps(f), &text);
    }
    lamp(ui, Field::Flag(Flag::Declick), 584.0, r.y + 29.0, "DECLICK");
    lamp(ui, Field::Flag(Flag::AltDecay), 584.0, r.y + 47.0, "ALT DEC");
}

fn unison(ui: &mut Ui) {
    titled(ui, UNISON, "UNISON");
    let y = UNISON.y + KNOB_LINE;
    for (k, (f, l)) in [(U_ORD, "ORD"), (U_PAN, "PAN"), (U_VOL, "VOL"), (U_PITCH, "PITCH"), (U_PHS, "PHS")].into_iter().enumerate() {
        knob(ui, Field::Int(f), 33.0 + 48.0 * k as f32, y, 16.0, l);
    }
}

fn filter(ui: &mut Ui) {
    titled(ui, FILTER, "FILTER");
    let r = FILTER;
    for (k, l) in ["LP", "BP", "BS", "HP"].into_iter().enumerate() {
        choice(ui, Field::Int(F_TYPE), k as i32, Rect::new(r.x + 8.0 + 26.0 * k as f32, r.y + 22.0, 26.0, 18.0), l);
    }
    for (k, l) in ["OF", "x1", "x2", "x3"].into_iter().enumerate() {
        choice(ui, Field::Int(F_MODE), k as i32, Rect::new(r.x + 8.0 + 26.0 * k as f32, r.y + 44.0, 26.0, 18.0), l);
    }
    lamp(ui, Field::Int(FLAT), r.x + 122.0, r.y + 53.0, "FLAT");
    let y = r.y + KNOB_LINE;
    knob(ui, Field::Int(CUT), 436.0, y, 16.0, "CUT");
    knob(ui, Field::Int(RES), 484.0, y, 16.0, "RES");
}

/// The tone switches the Options menu also holds.
fn tone(ui: &mut Ui) {
    titled(ui, TONE, "TONE");
    let x = TONE.x + 14.0;
    for (k, (g, l)) in [(Flag::Porta, "PORTAMENTO"), (Flag::AutoLoudness, "AUTO LOUDNESS"), (Flag::LinkRelease, "LINK RELEASE")].into_iter().enumerate() {
        lamp(ui, Field::Flag(g), x, TONE.y + 27.0 + 17.0 * k as f32, l);
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

fn modulation(ui: &mut Ui) {
    titled(ui, MODS, "MODULATION");
    lemon::plug_well(ui.c, PAD, 6.0);
    ui.c.fill_rect(Rect::new(PAD.cx().round(), PAD.y + 1.0, 1.0, PAD.h - 2.0), plug::GRID);
    ui.c.fill_rect(Rect::new(PAD.x + 1.0, PAD.cy().round(), PAD.w - 2.0, 1.0), plug::GRID);
    let (x, y) = pad_point(ctl::shown(ui.m.st, Field::Int(MOD_X)), ctl::shown(ui.m.st, Field::Int(MOD_Y)));
    let (x, y) = (x.clamp(PAD.x + 5.0, PAD.right() - 5.0), y.clamp(PAD.y + 5.0, PAD.bottom() - 5.0));
    let hot = ui.hot(PAD, &Target::Pad);
    ui.c.fill_circle(x, y, 5.0, WELL);
    ui.c.fill_circle(x, y, 4.0, if hot { brand::HIGHLIGHT } else { ACCENT });
    ui.hit(PAD, Target::Pad);
    knob(ui, Field::Int(MOD_X), 816.0, 82.0, 14.0, "X");
    knob(ui, Field::Int(MOD_Y), 816.0, 132.0, 14.0, "Y");
    lamp(ui, Field::Flag(Flag::Smooth), 797.0, 174.0, "SMOOTH");
}

// ---------------------------------------------------------------------------------------------------------------
// The articulator editor

const PART_X: f32 = 204.0;
const PART_W: f32 = 48.0;

pub fn part_tab(i: usize) -> Rect {
    Rect::new(PART_X + PART_W * i as f32, EDITOR_BAR.y + 3.0, PART_W, 18.0)
}

pub const TARGET_BOX: Rect = Rect::new(12.0, 201.0, 186.0, 18.0);
pub const COMB_BOX: Rect = Rect::new(204.0, 201.0, 96.0, 18.0);

fn editor_card(ui: &mut Ui) {
    lemon::plug_panel(ui.c, EDITOR_BAR);
    let t = ui.st.target.min(TARGETS - 1);
    // The target's menu.
    let st = ui.state(TARGET_BOX, &Target::TargetBox);
    lemon::field(ui.c, TARGET_BOX, st);
    let lh = ui.c.line_height(SMALL);
    ui.c.text(SMALL, TARGET_BOX.x + 8.0, (TARGET_BOX.cy() - lh / 2.0).round(), if st.hover { TEXT } else { ACCENT }, TARGET_NAMES[t]);
    let (ax, ay) = (TARGET_BOX.right() - 10.0, TARGET_BOX.cy());
    ui.c.fill_polygon(&[(ax - 4.0, ay - 2.0), (ax + 4.0, ay - 2.0), (ax, ay + 3.0)], if st.hover { TEXT } else { DIM });
    ui.hit(TARGET_BOX, Target::TargetBox);
    // Its parts, or a final map's comb.
    let parts = parts_of(t);
    let id = shown_curve(ui.st);
    let shown = if let CurveId::Part(_, p) = id { p } else { 0 };
    for (i, &p) in parts.iter().enumerate() {
        let on = t < ARTS && p < 2 && ctl::part(ui.m.st, t, p).enabled;
        tab(ui, part_tab(i), Target::PartTab(p), PART_LABELS[p], p == shown, on);
    }
    if t >= ARTS {
        let f = Field::Comb(t - ARTS);
        let v = ctl::shown(ui.m.st, f);
        value_box(ui, COMB_BOX, Target::Comb, &format!("COMB {}", ctl::text(f, v).to_uppercase()));
    }
    let c = curve_of(ui.m.st, id);
    curve_view(ui, c);
    lemon::plug_panel(ui.c, BAR);
    if let CurveId::Part(a, p @ (0 | 1)) = id {
        switch_part(ui, a, p);
    }
    tools(ui);
}

const BAR_Y: f32 = 372.0;

/// The bar's envelope or LFO settings.
fn switch_part(ui: &mut Ui, a: usize, p: usize) {
    lamp(ui, Field::Part(a, p, 0), 20.0, BAR_Y, "");
    let labels = if p == 0 { ["ATT", "DEC", "SUS", "REL"] } else { ["SPD", "TNS", "SKEW", "PW"] };
    for (k, l) in labels.into_iter().enumerate() {
        knob(ui, Field::Part(a, p, k + 1), 44.0 + 24.0 * k as f32, BAR_Y - 7.0, 8.0, "");
        ui.label(SMALL, 44.0 + 24.0 * k as f32, BAR_Y + 10.0, DIM, l);
    }
    lamp(ui, Field::Part(a, p, 5), 142.0, BAR_Y - 8.0, "TEMPO");
    lamp(ui, Field::Part(a, p, 6), 142.0, BAR_Y + 8.0, "SHARED");
}

pub const OPTIONS_R: Rect = Rect::new(474.0, BAR_Y - 8.0, 16.0, 16.0);
pub const TOOL_X: [f32; 4] = [510.0, 534.0, 558.0, 582.0];
pub const TOOL_NAMES: [&str; 4] = ["Freeze editing / hide points", "Step editing", "Snap to grid", "Slide succeeding points / regions"];
const TOOL_LABELS: [&str; 4] = ["FRZ", "STP", "SNP", "SLD"];

fn tools(ui: &mut Ui) {
    let t = Target::CurveOptions;
    let hot = ui.hot(OPTIONS_R, &t);
    let (x, y) = (OPTIONS_R.cx(), OPTIONS_R.cy());
    ui.c.fill_polygon(&[(x - 4.5, y - 2.0), (x + 4.5, y - 2.0), (x, y + 3.0)], if hot { TEXT } else { DIM });
    ui.hit(OPTIONS_R, t);
    for (k, x) in TOOL_X.into_iter().enumerate() {
        let r = Rect::new(x - 10.0, BAR_Y - 8.0, 20.0, 16.0);
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

pub fn curve_inner() -> Rect {
    Rect::span(CURVE.x + 8.0, CURVE.right() - 8.0, CURVE.y + 8.0, CURVE.bottom() - 20.0)
}

/// The time the curve view spans: a map 0 .. 1, an envelope or LFO its length and a tenth more (at least one unit).
pub fn curve_span(c: &Curve) -> f64 {
    if c.kind == MAP { 1.0 } else { (edit::times(c).last().copied().unwrap_or(0.0) * 1.1).max(1.0) }
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

/// The curve view: a grid, the curve with the area below it tinted, its points (filled), its tension handles (hollow,
/// halfway along their segments) and its markers.
fn curve_view(ui: &mut Ui, c: &Curve) {
    let r = CURVE;
    let map = c.kind == MAP;
    lemon::plug_well(ui.c, r, 6.0);
    for k in 1..8 {
        let x = (r.x + r.w * k as f32 / 8.0).round();
        ui.c.fill_rect(Rect::new(x, r.y + 1.0, 1.0, r.h - 2.0), plug::GRID);
    }
    for k in 1..4 {
        let y = (r.y + r.h * k as f32 / 4.0).round();
        ui.c.fill_rect(Rect::new(r.x + 1.0, y, r.w - 2.0, 1.0), plug::GRID);
    }
    ui.hit(r, Target::CurveArea);
    let inner = curve_inner();
    let span = ui.st.span.unwrap_or_else(|| curve_span(c));
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
    ui.c.fill_polygon(&area, ink.with_alpha(40));
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
// EQ and effects

/// A band's centre line in the EQ card.
pub fn eq_x(b: usize) -> f32 {
    645.0 + 78.0 * b as f32
}

pub fn eq_type_box(b: usize) -> Rect {
    Rect::new(eq_x(b) - 30.0, EQ.y + 21.0, 60.0, 18.0)
}

fn eq(ui: &mut Ui) {
    titled(ui, EQ, "EQ");
    for b in 0..3 {
        let x = eq_x(b);
        let r = eq_type_box(b);
        let t = Target::EqType(b);
        let st = ui.state(r, &t);
        lemon::field(ui.c, r, st);
        eq_icon(ui, r, ctl::shown(ui.m.st, Field::EqType(b)), if st.hover { TEXT } else { ACCENT });
        ui.hit(r, t);
        for (k, (f, l)) in [(EQ_LEVEL + b, "LEVEL"), (EQ_FREQ + b, "FREQ"), (EQ_WIDTH + b, "WIDTH")].into_iter().enumerate() {
            let y = EQ.y + 60.0 + 48.0 * k as f32;
            knob(ui, Field::Int(f), x, y, 15.0, l);
        }
    }
}

/// A band type's shape: low shelf, peak, high shelf.
fn eq_icon(ui: &mut Ui, r: Rect, ty: i32, ink: Color) {
    let (x, y) = (r.cx(), r.cy());
    let pts: Vec<(f32, f32)> = match ty {
        0 => vec![(x - 14.0, y - 3.0), (x - 4.0, y - 3.0), (x + 4.0, y + 3.0), (x + 14.0, y + 3.0)],
        1 => vec![(x - 14.0, y + 3.0), (x - 5.0, y + 3.0), (x, y - 4.0), (x + 5.0, y + 3.0), (x + 14.0, y + 3.0)],
        _ => vec![(x - 14.0, y + 3.0), (x - 4.0, y + 3.0), (x + 4.0, y - 3.0), (x + 14.0, y - 3.0)],
    };
    ui.c.polyline(&pts, 1.5, ink);
}

fn chorus(ui: &mut Ui) {
    titled(ui, CHORUS, "CHORUS");
    let y = CHORUS.y + KNOB_LINE;
    for (k, l) in ["ORD", "DP", "SP", "DL", "SR", "CR", "VL"].into_iter().enumerate() {
        knob(ui, Field::Int(crate::state::CHORUS + k), 30.0 + 36.0 * k as f32, y, 12.0, l);
    }
}

fn delay(ui: &mut Ui) {
    titled(ui, DELAY, "DELAY");
    let r = DELAY;
    lamp(ui, Field::Flag(Flag::DelayOn), r.x + 12.0, r.y + 30.0, "ON");
    lamp(ui, Field::Flag(Flag::DelayTempo), r.x + 12.0, r.y + 50.0, "TEMPO");
    for (k, l) in ["NORM", "INV", "PONG"].into_iter().enumerate() {
        choice(ui, Field::Int(crate::state::DELAY + 4), k as i32, Rect::new(r.x + 60.0, r.y + 18.0 + 17.0 * k as f32, 42.0, 17.0), l);
    }
    let y = r.y + KNOB_LINE;
    for (k, l) in ["FB", "TM", "SO", "VL"].into_iter().enumerate() {
        knob(ui, Field::Int(crate::state::DELAY + k), 396.0 + 36.0 * k as f32, y, 12.0, l);
    }
}

fn reverb(ui: &mut Ui) {
    titled(ui, REVERB, "REVERB");
    let r = REVERB;
    lamp(ui, Field::Flag(Flag::ReverbOn), r.x + 12.0, r.y + 25.0, "ON");
    lamp(ui, Field::Flag(Flag::ReverbTempo), r.x + 12.0, r.y + 40.0, "TEMPO");
    let col = Field::Int(crate::state::REVERB + 7);
    let text = ctl::text(col, ctl::shown(ui.m.st, col));
    value_box(ui, Rect::new(r.x + 4.0, r.y + 50.0, 48.0, 16.0), Target::Steps(col), &text);
    let y = r.y + KNOB_LINE;
    for (k, (i, l)) in [(0, "LC"), (1, "HC"), (2, "PD"), (3, "RS"), (4, "DF"), (5, "DE"), (6, "HD"), (8, "VL")].into_iter().enumerate() {
        knob(ui, Field::Int(crate::state::REVERB + i), 604.0 + 30.0 * k as f32, y, 12.0, l);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The testing keyboard

const WHITE: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

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
            let left = WHITE.iter().position(|&w| w == n - 1).unwrap_or(0);
            let x = KEYS.x + ww * (o as f32 * 7.0 + left as f32 + 1.0);
            Rect::new(x - ww * 0.3, KEYS.y, ww * 0.6, KEYS.h * 0.6)
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
            ui.c.fill_rrect(Rect::new(r.x + 0.5, r.y, r.w - 1.0, r.h), 3.0, fill);
            if k % 12 == 0 {
                ui.label(SMALL, r.cx(), r.bottom() - 9.0, Color::rgb(0x7a7a7a), &key_name(k));
            }
            ui.hit(r, Target::Key(k));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// About

fn info(ui: &mut Ui) {
    let (w, h) = ui.c.size();
    let mid = emblem(ui, w / 2.0, h / 2.0 - 45.0, true, Font::condensed(32, 0).bold(), 9.0);
    let v = format!("Version {}", ui.m.version);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 8.0, w, 16.0), DIM, &v);
    ui.c.text_center(LABEL, Rect::new(0.0, mid.bottom() + 26.0, w, 16.0), DIM, "Lemon Studio's additive synthesizer");
    ui.c.text_center(SMALL, Rect::new(0.0, h - 30.0, w, 16.0), FAINT, "Click to return");
    ui.hit(Rect::new(0.0, 0.0, w, h), Target::Cover);
}
