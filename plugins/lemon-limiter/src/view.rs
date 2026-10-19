// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Limiter editor: painting and hit regions (platform independent). Placeholder look for now: plain dials,
//! boxes and buttons of the `os_ui` Lemon kit in the original's places (no artwork of the original); it moves to the
//! Lemon "Pith" plugin look once that is final. Coordinates are the spec's editor-content pixels.
//!
// spec: plugins/lemon-limiter.md (§1 window, §2 controls, §3 display, §4 hints)

use os_engine::effects::dynamics::{self as dsp, Column};
use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{inst, State};
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

const VIOLET: u32 = 0xa58cf0;
const RED: u32 = 0xe8615a;
const GREEN: u32 = 0x6fd27a;
const GATE: u32 = 0xd0914a;
const CYAN: u32 = 0x4fd0e6;
const TEAL: u32 = 0x5fc8b8;
const GREY: Color = Color::rgb(0x8a8a8a);

const HEAD: Font = Font::new(11).bold();
const SMALL: Font = Font::new(11);

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
        ui.c.text_clip(SMALL, DISPLAY.x + 6.0, DISPLAY.y + 4.0, DISPLAY.w - 12.0, plug::DIM, &s);
    }
    sections(&mut ui);
    // Always shown (spec §2.1).
    knob(&mut ui, "gain", 30.0, 208.0, VIOLET, "GAIN");
    knob(&mut ui, "sat", 84.0, 208.0, RED, "SAT");
    knob(&mut ui, "g_rel", 487.0, 208.0, GATE, "REL");
    knob(&mut ui, "g_gain", 542.0, 208.0, GATE, "GAIN");
    knob(&mut ui, "g_thres", 597.0, 208.0, GATE, "THRES");
    compare_buttons(&mut ui);
    let panel = Rect::span(113.0, 460.0, 178.0, 297.0);
    lemon::plug_panel(ui.c, panel);
    if st.page == 0 { limit_page(&mut ui) } else { comp_page(&mut ui, look) }
    page_tabs(&mut ui);
    ui.hits
}

fn sections(ui: &mut Ui) {
    let line = plug::STRIP_LINE;
    ui.c.fill_rect(Rect::span(0.0, 625.0, 159.0, 159.0), line);
    ui.c.fill_rect(Rect::span(0.0, 625.0, 160.0, 175.0), plug::STRIP);
    for (x0, x1, s) in [(0.0, 283.0, "LOUDNESS"), (284.0, 461.0, "ENVELOPE"), (462.0, 625.0, "NOISE GATE")] {
        ui.centred(HEAD, (x0 + x1 + 1.0) / 2.0, 168.0, plug::DIM, s);
    }
    for x in [283.0, 461.0] {
        ui.c.fill_rect(Rect::span(x, x, 160.0, 175.0), line);
    }
    ui.c.fill_rect(Rect::span(0.0, 625.0, 176.0, 176.0), line);
}

/// A knob ~44 px across, caption 31 px under its centre.
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, accent: u32, caption: &str) {
    knob_col(ui, id, cx, cy, Color::rgb(accent), caption);
}

fn knob_col(ui: &mut Ui, id: &str, cx: f32, cy: f32, accent: Color, caption: &str) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - 22.0, cy - 22.0, 44.0, 44.0);
    let st = ui.state(hr, &target);
    let bipolar = matches!(id, "c_ratio" | "c_knee");
    inst::dial(ui.c, ui.t, cx, cy, 16.0, 19.0, ui.m.norm(id), bipolar, accent, st);
    inst::caption(ui.c, ui.t, cx, cy + 26.0, caption, false);
    ui.hit(hr, target);
}

/// A stepped box: the number and small up / down arrows at its right.
fn spinner(ui: &mut Ui, cx: f32, cy: f32, w: f32, text: &str, target: Target) {
    let r = Rect::new(cx - w / 2.0, cy - 9.0, w, 18.0);
    let st = ui.state(r, &target);
    lemon::field(ui.c, r, st);
    let ink = if st.hover || st.pressed { brand::TEXT } else { plug::DIM };
    ui.centred(SMALL, cx - 3.0, cy, ink, text);
    let ax = r.right() - 5.0;
    ui.c.fill_polygon(&[(ax - 2.5, cy - 2.0), (ax, cy - 5.0), (ax + 2.5, cy - 2.0)], ink);
    ui.c.fill_polygon(&[(ax - 2.5, cy + 2.0), (ax, cy + 5.0), (ax + 2.5, cy + 2.0)], ink);
    ui.hit(r, target);
}

fn curve_spinner(ui: &mut Ui, id: &str, cx: f32) {
    let s = format!("{}", ui.m.plain(id).round() as i32);
    spinner(ui, cx, 282.0, 22.0, &s, Target::Spin(id.into()));
}

/// ATT / REL captions joined to the CURVE caption at `x0` by short lines (a bracket when they share one).
fn curve_caption(ui: &mut Ui, x0: f32, x1: f32) {
    let col = plug::FAINT;
    let low = if x0 == x1 { 256.0 } else { 249.0 };
    for x in [x0, x1] {
        ui.c.fill_rect(Rect::span(x, x, 241.0, low), col);
    }
    if x0 != x1 {
        ui.c.fill_rect(Rect::span(x0, x1, low, low), col);
        ui.c.fill_rect(Rect::span(x0, x0, low, 256.0), col);
    }
    inst::caption(ui.c, ui.t, x0, 260.0, "CURVE", false);
}

fn limit_page(ui: &mut Ui) {
    knob(ui, "ceil", 144.0, 208.0, GREEN, "CEIL");
    knob(ui, "att", 313.0, 206.0, VIOLET, "ATT");
    knob(ui, "rel", 369.0, 206.0, VIOLET, "REL");
    knob(ui, "sustain", 430.0, 206.0, VIOLET, "SUSTAIN");
    curve_caption(ui, 313.0, 313.0);
    curve_caption(ui, 369.0, 369.0);
    curve_spinner(ui, "att_curve", 313.0);
    curve_spinner(ui, "rel_curve", 369.0);
}

fn comp_page(ui: &mut Ui, look: &Look) {
    // The compressor's own knobs are cyan while it works, grey otherwise (spec §2.3).
    let col = if comp_on(ui.m) { Color::rgb(CYAN) } else { GREY };
    knob_col(ui, "c_thres", 144.0, 208.0, col, "THRES");
    knob_col(ui, "c_knee", 199.0, 208.0, col, "KNEE");
    knob_col(ui, "c_ratio", 254.0, 208.0, col, "RATIO");
    knob(ui, "c_att", 313.0, 206.0, CYAN, "ATT");
    knob(ui, "c_rel", 369.0, 206.0, CYAN, "REL");
    knob(ui, "c_rms", 430.0, 206.0, CYAN, "SUSTAIN");
    curve_caption(ui, 313.0, 369.0);
    curve_spinner(ui, "c_curve", 313.0);
    inst::caption(ui.c, ui.t, 369.0, 260.0, "SIDECHAIN", false);
    let s = if look.sidechain == 0 { "--".to_string() } else { look.sidechain.to_string() };
    spinner(ui, 369.0, 282.0, 34.0, &s, Target::Sidechain);
}

fn page_tabs(ui: &mut Ui) {
    for (i, (x0, x1, s)) in [(113.0, 152.0, "LIMIT"), (153.0, 196.0, "COMP")].into_iter().enumerate() {
        let r = Rect::span(x0, x1, 276.0, 295.0);
        let target = Target::Page(i as u8);
        let st = ui.state(r, &target);
        let on = ui.st.page == i as u8;
        if on {
            ui.c.fill_rrect(r, 5.0, Color::rgb(0x1c1c1c));
        } else if st.hover {
            ui.c.fill_rrect(r, 5.0, Color::rgb(0x3d3d3d));
        }
        let ink = if on { brand::TEXT } else { plug::DIM };
        ui.centred(HEAD, r.cx(), r.cy(), ink, s);
        ui.hit(r, target);
    }
}

fn compare_buttons(ui: &mut Ui) {
    for (cx, target) in [(584.0, Target::Store), (612.0, Target::Flip)] {
        let cy = 282.0;
        let r = Rect::new(cx - 11.0, cy - 11.0, 22.0, 22.0);
        let st = ui.state(r, &target);
        let face = if st.pressed { Color::rgb(0x3a3a3a) } else if st.hover { Color::rgb(0x525252) } else { plug::FACE };
        ui.c.fill_circle(cx, cy, 11.0, plug::FACE_LINE);
        ui.c.fill_circle(cx, cy, 10.0, face);
        let ink = brand::TEXT;
        if target == Target::Store {
            ui.c.fill_polygon(&[(cx - 4.0, cy - 2.0), (cx + 4.0, cy - 2.0), (cx, cy + 3.0)], ink);
        } else {
            ui.c.fill_polygon(&[(cx - 3.5, cy - 1.5), (cx + 3.5, cy - 1.5), (cx, cy - 6.0)], ink);
            ui.c.fill_polygon(&[(cx - 3.5, cy + 1.5), (cx + 3.5, cy + 1.5), (cx, cy + 6.0)], ink);
        }
        ui.hit(r, target);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// View strip and display (spec §3)

fn view_strip(ui: &mut Ui) {
    ui.c.fill_rect(Rect::span(0.0, 29.0, 0.0, 158.0), plug::STRIP);
    let cx = 13.0;
    // Display menu.
    let r = Rect::new(cx - 9.0, 8.0, 18.0, 16.0);
    let st = ui.state(r, &Target::Menu);
    let ink = if st.hover { brand::TEXT } else { plug::DIM };
    ui.c.fill_polygon(&[(cx - 4.5, 13.5), (cx + 4.5, 13.5), (cx, 19.0)], ink);
    ui.hit(r, Target::Menu);
    for (k, &y) in LAYER_Y.iter().enumerate() {
        let r = Rect::new(cx - 9.0, y - 6.5, 18.0, 13.0);
        let target = Target::Layer(k);
        let st = ui.state(r, &target);
        let on = ui.st.layers[k];
        let col = |c: Color| if on { c } else if st.hover { plug::DIM } else { plug::OFF };
        match k {
            0 => ui.c.fill_rect(Rect::new(cx - 1.0, y - 5.0, 2.5, 10.0), col(Color::rgb(VIOLET))),
            1 => ui.c.stroke_circle(cx, y, 4.0, 1.6, col(Color::rgb(GREEN))),
            2 => ui.c.line(cx - 4.0, y + 4.0, cx + 4.0, y - 4.0, 1.6, col(brand::TEXT)),
            _ => {
                ui.c.fill_rect(Rect::new(cx - 4.5, y - 2.5, 9.0, 1.6), col(Color::rgb(VIOLET)));
                ui.c.fill_rect(Rect::new(cx - 4.5, y + 1.0, 9.0, 1.6), col(Color::rgb(VIOLET)));
            }
        }
        ui.hit(r, target);
    }
    // Scrolling speed fader.
    let r = Rect::new(cx - 9.0, SPEED_TOP - 6.0, 18.0, SPEED_BOT - SPEED_TOP + 12.0);
    let st = ui.state(r, &Target::Speed);
    ui.c.fill_rrect(Rect::new(cx - 1.5, SPEED_TOP, 3.0, SPEED_BOT - SPEED_TOP), 1.5, plug::TRACK);
    let hy = (SPEED_BOT - (SPEED_BOT - SPEED_TOP) * ui.st.speed).round();
    let face = if st.hover || st.pressed { Color::rgb(0x6a6a6a) } else { Color::rgb(0x555555) };
    ui.c.fill_rrect(Rect::new(cx - 7.0, hy - 4.0, 14.0, 8.0), 3.0, face);
    ui.c.fill_rect(Rect::new(cx - 4.0, hy - 0.5, 8.0, 1.0), brand::MAIN);
    ui.hit(r, Target::Speed);
}

fn display(ui: &mut Ui, look: &Look) {
    let d = DISPLAY;
    lemon::plug_well(ui.c, d, 4.0);
    ui.c.push_clip(d.inset(1.0));
    // 0 dB line.
    ui.c.fill_rect(Rect::new(d.x, level_y(1.0).round(), d.w, 1.0), Color::rgb(0x2a2a2a));
    let [show_in, show_out, show_env, show_marks] = ui.st.layers;
    let cols = look.history;
    let n = cols.len().min(d.w as usize);
    let x_of = |i: usize| d.right() - (n - i) as f32;
    let bot = d.bottom();
    let (vio, grn, mint) = (Color::rgb(VIOLET), Color::rgb(GREEN), Color::rgb(0xd8f0dc));
    for (i, col) in cols[cols.len() - n..].iter().enumerate() {
        let x = x_of(i);
        let (yi, yo) = (level_y(col.input), level_y(col.output));
        if show_in {
            ui.c.fill_rect(Rect::new(x, yi, 1.0, bot - yi), vio.with_alpha(200));
        }
        if show_out {
            ui.c.fill_rect(Rect::new(x, yo, 1.0, bot - yo), grn.with_alpha(200));
        }
        if show_in && show_out {
            let y = yi.max(yo);
            ui.c.fill_rect(Rect::new(x, y, 1.0, bot - y), mint.with_alpha(210));
        }
    }
    if show_env && n > 1 {
        let teal = Color::rgb(TEAL);
        let start = cols.len() - n;
        let level: Vec<(f32, f32)> = (0..n).map(|i| (x_of(i), level_y(cols[start + i].level))).collect();
        let gain: Vec<(f32, f32)> = (0..n).map(|i| (x_of(i), level_y(cols[start + i].gain.min(4.0)))).collect();
        ui.c.polyline(&level, 1.2, teal);
        ui.c.polyline(&gain, 1.2, teal.mix(brand::TEXT, 0.4));
    }
    if show_marks {
        let ceil = dsp::level(ui.m.plain("ceil").round() as i32, 3.0);
        ui.c.fill_rect(Rect::new(d.x, level_y(ceil).round() - 1.0, d.w, 2.0), Color::rgb(0xc8c8c8));
        if comp_on(ui.m) {
            let thr = dsp::level(ui.m.plain("c_thres").round() as i32, 7.0);
            ui.c.fill_rect(Rect::new(d.x, level_y(thr).round(), d.w, 1.0), Color::rgb(CYAN));
        }
    }
    ui.c.pop_clip();
    ui.hit(d, Target::Display);
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

