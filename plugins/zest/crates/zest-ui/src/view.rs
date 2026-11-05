// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Painting and hit regions (platform independent), in the Lemon "Pith" plugin look (specs `ui/lemon-design.md`
//! §11.1): cards under ruled headings on `#2b2b2b`, round dials with lemon arcs, pills and toggles, the hint well and
//! the wordmark along the foot (hosted, the wordmark alone: a host's title row has the hint). Everything is laid out for the size the window has now (logical px, never less
//! than [`BASE_W`] x [`base_h`]; smaller windows are drawn zoomed out, see [`fit`]).

use os_theme::design::lemon as brand;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::State;
use os_ui::{Canvas, Color, Font, Rect};

use crate::library::{Chip, Kind, Library};
use crate::macros::Macro;
use crate::pages::{self, Card, Ctl};
use crate::params::{Model, ParamTable};
use crate::rules;

/// The editor's size in logical px (the standalone editor, its tab strip included).
pub const BASE_W: f32 = 960.0;
pub const BASE_H: f32 = 560.0;
/// The smallest window the host may make.
pub const MIN_W: f32 = 760.0;
pub const MIN_H: f32 = 460.0;
/// The editor's own page tabs (left out when the host shows them in its strip).
pub const STRIP: f32 = 30.0;
/// The foot (hint well, wordmark).
pub const FOOT: f32 = 34.0;
/// What is left under the pages when hosted: the wordmark alone, the host's title row has the hint line.
pub const HOSTED_FOOT: f32 = 30.0;
const M: f32 = 8.0;
/// Width of the Main page's side columns.
const SIDE: f32 = 300.0;
/// From a card's top to the next card's: the heading sits in the gap.
const GAP: f32 = 24.0;
/// The Advanced page's rows.
pub const ROW_H: f32 = 17.0;
/// The preset list's rows.
pub const LIST_ROW: f32 = 18.0;
/// How many kinds of sound get a chip of their own in the row (the rest are in the ⋯ menu).
pub const ROW_KINDS: usize = 5;

const RULE: Color = Color::rgb(0x3c3c3c);
const HEADING: Font = Font::condensed(11, 0).bold();
const CAPTION: Font = Font::new(9).bold();
const LABEL: Font = Font::new(10).bold();
const SMALL: Font = Font::new(11);
const NAME: Font = Font::new(21).bold();
const HINT: Font = Font::new(11);
const MARK_BIG: Font = Font::condensed(22, 0).bold();
const MARK_SMALL: Font = Font::condensed(10, 0).bold();

/// The base height: the standalone editor with its tab strip and foot, or hosted without the strip and the hint well:
/// the host's title row has the page tabs and the hint line.
pub fn base_h(hosted: bool) -> f32 {
    if hosted {
        BASE_H - STRIP - FOOT + HOSTED_FOOT
    } else {
        BASE_H
    }
}

/// For a window of `w` x `h` logical px: the zoom it is drawn at (1, or less for windows smaller than the base size)
/// and the layout size (logical px at that zoom, never below the base size).
pub fn fit(w: f32, h: f32, hosted: bool) -> (f32, f32, f32) {
    let z = (w / BASE_W).min(h / base_h(hosted)).min(1.0).max(0.25);
    (z, w / z, h / z)
}

/// What a region does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Vertical drag, wheel, double click resets, right click types in.
    Knob(String),
    Macro(Macro),
    /// The Advanced page's value bars: drag either way.
    Value(String),
    /// On / off.
    Toggle(String),
    /// One position of a choice.
    Choice(String, i32),
    /// A box that opens the choice's menu.
    Menu(String),
    Tab(usize),
    Chip(Chip),
    /// A preset in the list (library index).
    Preset(usize),
    Star,
    Prev,
    Next,
    /// The ⋯ chip: the menu of the other chips.
    More,
    Save,
    Search,
    ListBar,
    AdvBar,
}

impl Target {
    /// The parameter it edits, if one.
    pub fn key(&self) -> Option<&str> {
        match self {
            Target::Knob(k) | Target::Value(k) | Target::Toggle(k) | Target::Choice(k, _) | Target::Menu(k) => Some(k),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// The current preset as the screen shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Current {
    pub path: Option<std::path::PathBuf>,
    pub name: String,
    pub place: String,
    pub author: Option<String>,
    pub favourite: bool,
    pub modified: bool,
}

/// Everything painting reads besides the parameter values.
pub struct ViewIn<'a> {
    pub table: &'a ParamTable,
    pub lib: &'a Library,
    /// The presets listed (library indices).
    pub shown: &'a [usize],
    pub favs: &'a crate::library::Favorites,
    pub current: &'a Current,
    pub page: usize,
    pub hosted: bool,
    pub size: (f32, f32),
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<&'a Target>,
    pub query: &'a str,
    pub search_focus: bool,
    pub chip: &'a Chip,
    /// First list row shown (fractional).
    pub list_scroll: f32,
    pub adv_scroll: f32,
    pub hint: &'a str,
    /// Blinking caret on (the search box).
    pub caret: bool,
    pub viz: &'a crate::viz::Viz,
}

/// What painting found out: hit regions and the scrollable areas.
#[derive(Clone, Debug, Default)]
pub struct ViewOut {
    pub hits: Vec<Hit>,
    /// The screen (the current preset): the wheel over it steps through the presets.
    pub screen: Rect,
    /// The preset list's well and how many rows fit.
    pub list: Rect,
    pub list_rows: f32,
    /// The Advanced page's area and its content height.
    pub adv: Rect,
    pub adv_height: f32,
}

struct P<'a, 'b> {
    c: &'a mut Canvas,
    v: &'a ViewIn<'b>,
    m: &'a dyn Model,
    out: ViewOut,
}

impl P<'_, '_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.out.hits.push(Hit { r, t });
    }

    fn state(&self, r: Rect, t: &Target, disabled: bool) -> State {
        let pressed = self.v.pressed == Some(t);
        let hover = !pressed && self.v.pressed.is_none() && self.v.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed, disabled, ..State::NORMAL }
    }

    fn norm(&self, key: &str) -> f32 {
        self.v.table.get(key).map_or(0.0, |p| p.norm(self.m.get(key)))
    }

    fn veiled(&self, key: &str) -> bool {
        rules::veiled(self.m, key)
    }

    fn label_of(&self, key: &str) -> String {
        let v = self.m.get(key);
        self.v.table.get(key).map_or_else(String::new, |p| p.format(v))
    }
}

/// Paints the editor and returns its hit regions.
pub fn paint(c: &mut Canvas, v: &ViewIn, m: &dyn Model) -> ViewOut {
    let mut p = P { c, v, m, out: ViewOut::default() };
    p.c.begin_frame(None);
    p.c.clear(plug::BG);
    let (w, h) = v.size;
    let top = if v.hosted { 0.0 } else { STRIP };
    if !v.hosted {
        strip(&mut p, w);
    }
    let area = Rect::span(M, w - M, top, h - if v.hosted { HOSTED_FOOT } else { FOOT });
    match v.page {
        pages::EFFECTS => effects_page(&mut p, area),
        pages::ADVANCED => advanced_page(&mut p, area),
        _ => main_page(&mut p, area),
    }
    foot(&mut p, w, h);
    p.out
}

// -------------------------------------------------------------------------------------------------------------------
// Frame

/// The editor's own page tabs: text tabs, the chosen one a lit cell.
fn strip(p: &mut P, w: f32) {
    p.c.fill_rect(Rect::new(0.0, 0.0, w, STRIP - 1.0), plug::STRIP);
    p.c.fill_rect(Rect::new(0.0, STRIP - 1.0, w, 1.0), plug::STRIP_LINE);
    let mut x = M;
    for (i, name) in pages::PAGES.iter().enumerate() {
        let tw = lemon::spaced_width(p.c, LABEL, name, 1.5);
        let r = Rect::new(x, 4.0, (tw + 28.0).round(), STRIP - 9.0);
        let t = Target::Tab(i);
        let st = p.state(r, &t, false);
        let on = p.v.page == i;
        let ink = if on {
            p.c.fill_rrect(r, 8.0, brand::DEEP);
            lemon::lit(p.c, r.inset(1.0), 7.0, st.hover);
            brand::COLORED_DARK
        } else if st.hover {
            p.c.fill_rrect(r, 8.0, Color::rgb(0x333333));
            brand::TEXT
        } else {
            plug::DIM
        };
        let lh = p.c.line_height(LABEL);
        lemon::spaced(p.c, LABEL, (r.cx() - tw / 2.0).round(), (r.cy() - lh / 2.0).round(), ink, name, 1.5);
        p.hit(r, t);
        x = r.right() + 4.0;
    }
}

/// The hint well and the wordmark on one line along the foot.
fn foot(p: &mut P, w: f32, h: f32) {
    let y = h - if p.v.hosted { HOSTED_FOOT } else { FOOT } / 2.0;
    let right = w - M;
    let wb = lemon::spaced_width(p.c, MARK_BIG, "ZEST", 2.0);
    let ws = lemon::spaced_width(p.c, MARK_SMALL, "LEMON", 2.5);
    let (lh_b, lh_s) = (p.c.line_height(MARK_BIG), p.c.line_height(MARK_SMALL));
    let bx = (right - wb).round();
    lemon::spaced(p.c, MARK_BIG, bx, (y - lh_b / 2.0).round(), plug::WORDMARK, "ZEST", 2.0);
    let sx = (bx - 8.0 - ws).round();
    lemon::spaced(p.c, MARK_SMALL, sx, (y - lh_s / 2.0).round() + 1.0, brand::MAIN, "LEMON", 2.5);
    let left = sx - 14.0;
    lemon::lemon_mark(p.c, left + 5.0, y, 4.0, brand::MAIN);
    if p.v.hosted {
        return;
    }
    let well = Rect::span(M, left - 16.0, y - 11.0, y + 11.0);
    lemon::plug_well(p.c, well, 6.0);
    if !p.v.hint.is_empty() {
        let lh = p.c.line_height(HINT);
        p.c.text_clip(HINT, well.x + 8.0, (well.cy() - lh / 2.0).round(), well.w - 16.0, brand::TEXT, p.v.hint);
    }
}

/// A card's heading: letter-spaced capitals centred on a hairline rule at `y`.
fn heading(p: &mut P, x0: f32, x1: f32, y: f32, s: &str, dim: bool) {
    let lh = p.c.line_height(HEADING);
    let w = lemon::spaced_width(p.c, HEADING, s, 2.0);
    let tx = ((x0 + x1) / 2.0 - w / 2.0).round();
    lemon::spaced(p.c, HEADING, tx, (y - lh / 2.0).round(), if dim { plug::OFF } else { plug::DIM }, s, 2.0);
    if tx - 7.0 > x0 + 8.0 {
        p.c.fill_rect(Rect::span(x0 + 4.0, tx - 7.0, y, y + 1.0), RULE);
        p.c.fill_rect(Rect::span((tx + w + 6.0).round(), x1 - 4.0, y, y + 1.0), RULE);
    }
}

/// A caption centred at `cx`, top at `y`.
fn caption(p: &mut P, cx: f32, y: f32, s: &str, disabled: bool) {
    let f = CAPTION;
    let w = lemon::spaced_width(p.c, f, s, 1.0);
    lemon::spaced(p.c, f, (cx - w / 2.0).round(), y.round(), if disabled { plug::OFF } else { plug::DIM }, s, 1.0);
}

// -------------------------------------------------------------------------------------------------------------------
// Controls

/// A dial of radius `r` with its caption under it; the hit area is the dial and its caption.
#[allow(clippy::too_many_arguments)]
fn dial(p: &mut P, t: Target, cx: f32, cy: f32, r: f32, norm: f32, bipolar: bool, cap: &str, disabled: bool) {
    let hr = Rect::new(cx - r - 4.0, cy - r - 4.0, 2.0 * r + 8.0, 2.0 * r + 20.0);
    let st = p.state(hr, &t, disabled);
    let ring = if r > 20.0 { 3.4 } else { 2.8 };
    lemon::dial(p.c, cx, cy, r, ring, norm, bipolar, brand::MAIN, st);
    caption(p, cx, cy + r + 5.0, cap, disabled);
    p.hit(hr, t);
}

/// The knob radius for a cell.
fn knob_r(cell: Rect) -> f32 {
    (cell.w * 0.3).min((cell.h - 30.0) / 2.0).clamp(12.0, 30.0).round()
}

/// A dial centred in `cell` (dial over caption).
#[allow(clippy::too_many_arguments)]
fn cell_dial(p: &mut P, cell: Rect, t: Target, norm: f32, bipolar: bool, cap: &str, disabled: bool) {
    let r = knob_r(cell);
    let block = 2.0 * r + 16.0;
    let cy = (cell.y + (cell.h - block) / 2.0 + r).round();
    dial(p, t, cell.cx().round(), cy, r, norm, bipolar, cap, disabled);
}

/// A choice's pills stacked in `cell` (caption under them when given).
fn pills_v(p: &mut P, cell: Rect, key: &str, labels: &[&str], cap: Option<&str>, disabled: bool) {
    let n = labels.len() as f32;
    let cap_h = if cap.is_some() { 14.0 } else { 0.0 };
    let ph = ((cell.h - cap_h - 8.0) / n - 2.0).clamp(11.0, 17.0).floor();
    let total = n * (ph + 2.0) - 2.0 + cap_h;
    let mut y = (cell.y + (cell.h - total) / 2.0).round();
    let min = p.v.table.get(key).map_or(0.0, |i| i.min);
    let chosen = (p.m.get(key) - min).round() as i32;
    let x0 = cell.x + 4.0;
    let x1 = cell.right() - 4.0;
    for (i, l) in labels.iter().enumerate() {
        let r = Rect::span(x0, x1, y, y + ph);
        pill(p, r, key, i as i32, chosen == i as i32, l, disabled);
        y += ph + 2.0;
    }
    if let Some(c) = cap {
        caption(p, cell.cx(), y + 3.0, c, disabled);
    }
}

/// One pill of a choice.
#[allow(clippy::too_many_arguments)]
fn pill(p: &mut P, r: Rect, key: &str, i: i32, on: bool, label: &str, disabled: bool) {
    let t = Target::Choice(key.into(), i);
    let st = p.state(r, &t, disabled);
    let ink = lemon::choice_button(p.c, r, on, brand::MAIN, st);
    let f = p.c.fit_font(LABEL, label, r.w - 4.0);
    p.c.text_center(f, r, ink, label);
    p.hit(r, t);
}

/// A choice's pills side by side across `r`, widths following their labels.
fn pills_h(p: &mut P, r: Rect, key: &str, labels: &[&str], disabled: bool) {
    let min = p.v.table.get(key).map_or(0.0, |i| i.min);
    let chosen = (p.m.get(key) - min).round() as i32;
    let widths: Vec<f32> = labels.iter().map(|l| p.c.text_width(LABEL, l) + 14.0).collect();
    let sum: f32 = widths.iter().sum();
    let k = (r.w - 2.0 * (labels.len() as f32 - 1.0)) / sum;
    let mut x = r.x;
    for (i, (l, w)) in labels.iter().zip(&widths).enumerate() {
        let pw = if i + 1 == labels.len() { r.right() - x } else { (w * k).round() };
        pill(p, Rect::new(x, r.y, pw, r.h), key, i as i32, chosen == i as i32, l, disabled);
        x += pw + 2.0;
    }
}

/// On / off switches stacked in `cell`: toggle and label.
fn switches(p: &mut P, cell: Rect, list: &[(&str, &str)], card_veiled: bool) {
    let n = list.len() as f32;
    let rh = 17.0;
    let mut y = (cell.y + (cell.h - n * rh) / 2.0).round();
    let lw = list.iter().map(|(_, l)| p.c.text_width(LABEL, l)).fold(0.0, f32::max);
    let x = (cell.cx() - (lw + 18.0) / 2.0).round().max(cell.x + 2.0);
    for (key, label) in list {
        let disabled = card_veiled || p.veiled(key);
        let t = Target::Toggle(key.to_string());
        let hr = Rect::new(x - 2.0, y, lw + 22.0, rh);
        let mut st = p.state(hr, &t, disabled);
        let on = p.m.get(key) >= 0.5;
        st.on = on;
        let cy = y + rh / 2.0;
        lemon::toggle(p.c, x + 6.0, cy, on, st);
        let lh = p.c.line_height(LABEL);
        let ink = if disabled { plug::OFF } else if st.hover { brand::TEXT } else { plug::DIM };
        p.c.text(LABEL, x + 17.0, (cy - lh / 2.0).round(), ink, label);
        p.hit(hr, t);
        y += rh;
    }
}

/// A box showing a choice; a click opens its menu.
fn menu_box(p: &mut P, r: Rect, key: &str, disabled: bool) {
    let t = Target::Menu(key.into());
    let st = p.state(r, &t, disabled);
    lemon::field(p.c, r, st);
    let text = p.label_of(key);
    let ink = if disabled { plug::OFF } else { brand::TEXT };
    let f = p.c.fit_font(LABEL, &text, r.w - 16.0);
    let lh = p.c.line_height(f);
    let tw = p.c.text_width(f, &text);
    p.c.text(f, (r.x + (r.w - 10.0 - tw) / 2.0).round().max(r.x + 4.0), (r.cy() - lh / 2.0).round(), ink, &text);
    let (ax, ay) = (r.right() - 8.0, r.cy());
    p.c.fill_polygon(&[(ax - 3.0, ay - 1.5), (ax + 3.0, ay - 1.5), (ax, ay + 2.0)], if disabled { plug::OFF } else { plug::DIM });
    p.hit(r, t);
}

/// A cell holding a menu box over a caption.
fn cell_menu(p: &mut P, cell: Rect, key: &str, cap: &str, disabled: bool) {
    let bh = 20.0;
    let y = (cell.cy() - (bh + 18.0) / 2.0).round();
    let r = Rect::new(cell.x + 3.0, y, cell.w - 6.0, bh);
    menu_box(p, r, key, disabled);
    caption(p, cell.cx(), r.bottom() + 6.0, cap, disabled);
}

/// The value a Main card's knob shows and whether it is bipolar.
fn macro_dial(p: &mut P, cell: Rect, mc: Macro, disabled: bool) {
    let norm = mc.norm(p.m);
    cell_dial(p, cell, Target::Macro(mc), norm, mc.bipolar(), &mc.name().to_uppercase(), disabled);
}

fn cell_ctl(p: &mut P, cell: Rect, ctl: &Ctl, card_veiled: bool) {
    match ctl {
        Ctl::Knob { key, cap, bipolar } => {
            let d = card_veiled || p.veiled(key);
            let n = p.norm(key);
            cell_dial(p, cell, Target::Knob(key.to_string()), n, *bipolar, cap, d);
        }
        Ctl::Macro(mc) => {
            // Detune spreads the Triple VCO's oscillators 2 and 3; veiled in the other models.
            let d = card_veiled || (*mc == Macro::Detune && (p.m.int("OSC/Model") != rules::TRIPLE || (p.m.get("OSC/Volume2") <= 0.0 && p.m.get("OSC/Volume3") <= 0.0)));
            macro_dial(p, cell, *mc, d);
        }
        Ctl::Switches(list) => switches(p, cell, list, card_veiled),
        Ctl::Pills { key, labels, cap } => {
            let d = card_veiled || p.veiled(key);
            pills_v(p, cell, key, labels, *cap, d);
        }
        Ctl::Menu { key, cap } => {
            let d = card_veiled || p.veiled(key);
            cell_menu(p, cell, key, cap, d);
        }
        Ctl::MenuPair { keys, cap } => {
            let bh = 17.0;
            let y = (cell.cy() - (2.0 * bh + 3.0 + 16.0) / 2.0).round();
            for (i, k) in keys.iter().enumerate() {
                let d = card_veiled || p.veiled(k);
                menu_box(p, Rect::new(cell.x + 3.0, y + i as f32 * (bh + 3.0), cell.w - 6.0, bh), k, d);
            }
            caption(p, cell.cx(), y + 2.0 * bh + 3.0 + 5.0, cap, card_veiled);
        }
        Ctl::EnvAmount => {
            let key = "VCF1/FreqModDepth";
            let s = p.m.int("VCF1/FreqModSrc");
            let d = card_veiled || p.veiled(key);
            let cap = if s == rules::src::NONE { "ENV AMT".to_string() } else { rules::source_caption(p.m, s) };
            let n = p.norm(key);
            cell_dial(p, cell, Target::Knob(key.into()), n, true, &cap, d);
        }
        Ctl::LfoDepth(i) => {
            let routes = rules::lfo1_routes(p.m);
            match routes.get(*i) {
                Some(r) => {
                    let key = r.depth.unwrap_or(r.src);
                    let n = p.norm(key);
                    cell_dial(p, cell, Target::Knob(key.to_string()), n, true, r.target, card_veiled);
                }
                // No route: an empty, veiled depth (the layout does not move).
                None => {
                    let r = knob_r(cell);
                    let block = 2.0 * r + 16.0;
                    let cy = (cell.y + (cell.h - block) / 2.0 + r).round();
                    lemon::dial(p.c, cell.cx().round(), cy, r, 2.8, 0.5, true, brand::MAIN, State { disabled: true, ..State::NORMAL });
                    caption(p, cell.cx(), cy + r + 5.0, if *i == 0 { "DEPTH" } else { "DEPTH 2" }, true);
                }
            }
        }
    }
}

// -------------------------------------------------------------------------------------------------------------------
// Main page

/// A card's fixed height besides its rows.
fn card_fixed(c: &Card) -> f32 {
    12.0 + if c.top.is_some() { 24.0 } else { 0.0 }
}

/// Stacks cards from `top` to `bottom` in column `x0..x1`, rows sharing the height left.
fn column(p: &mut P, x0: f32, x1: f32, top: f32, bottom: f32, cards: &[Card]) {
    let rows: f32 = cards.iter().map(|c| c.rows as f32).sum();
    let fixed: f32 = cards.iter().map(card_fixed).sum::<f32>() + GAP * (cards.len() as f32 - 1.0);
    let row_h = ((bottom - top - fixed) / rows).max(56.0).floor();
    let mut y = top;
    for c in cards {
        let h = card_fixed(c) + row_h * c.rows as f32;
        card(p, Rect::new(x0, y, x1 - x0, h), c);
        y += h + GAP;
    }
}

fn card(p: &mut P, r: Rect, c: &Card) {
    let row_h = (r.h - card_fixed(c)) / c.rows as f32;
    lemon::plug_panel(p.c, r);
    heading(p, r.x, r.right(), r.y - 11.0, &c.title, c.veiled);
    let mut y = r.y + 6.0;
    if let Some(top) = &c.top {
        let mut x1 = r.right() - 8.0;
        if let Some(Ctl::MenuPair { keys, cap }) = &c.top_extra {
            // The pitch bend range: two small boxes and a caption at the row's right end.
            let bw = 34.0;
            let bx = x1 - 2.0 * bw - 3.0;
            for (i, k) in keys.iter().enumerate() {
                menu_box(p, Rect::new(bx + i as f32 * (bw + 3.0), y + 1.0, bw, 18.0), k, c.veiled);
            }
            let cw = lemon::spaced_width(p.c, CAPTION, cap, 1.0);
            caption(p, bx - 6.0 - cw / 2.0, y + 5.0, cap, c.veiled);
            x1 = bx - 12.0 - cw;
        }
        if let Ctl::Pills { key, labels, .. } = top {
            let d = c.veiled || p.veiled(key);
            pills_h(p, Rect::span(r.x + 8.0, x1, y + 1.0, y + 19.0), key, labels, d);
        }
        y += 24.0;
        p.c.fill_rect(Rect::span(r.x + 10.0, r.right() - 10.0, y - 2.0, y - 1.0), plug::PANEL_LINE);
    }
    let cw = (r.w - 8.0) / c.cols as f32;
    // A model using fewer rows than the card has: its rows a little taller, centred in the card.
    let used = c.cells.iter().map(|cl| cl.row + 1).max().unwrap_or(1).max(1);
    let area_h = row_h * c.rows as f32;
    let row_h = (area_h / used as f32).min(row_h * 1.25);
    let y = (y + (area_h - row_h * used as f32) / 2.0).round();
    for cell in &c.cells {
        let cr = Rect::new(r.x + 4.0 + cw * cell.col as f32, y + row_h * cell.row as f32, cw * cell.span as f32, row_h);
        if cell.span > 1 {
            if let Ctl::Pills { key, labels, cap } = &cell.ctl {
                let d = c.veiled || p.veiled(key);
                let ph = 17.0;
                let py = (cr.cy() - (ph + 16.0) / 2.0).round();
                pills_h(p, Rect::new(cr.x + 4.0, py, cr.w - 8.0, ph), key, labels, d);
                if let Some(cap) = cap {
                    caption(p, cr.cx(), py + ph + 6.0, cap, d);
                }
                continue;
            }
        }
        cell_ctl(p, cr, &cell.ctl, c.veiled);
    }
}

fn main_page(p: &mut P, area: Rect) {
    let (left, right) = pages::main_cards(p.m);
    let top = area.y + 22.0;
    let bottom = area.bottom() - 2.0;
    let side = (area.w * 0.315).max(SIDE).min((area.w - 2.0 * M) * 0.36).floor();
    column(p, area.x, area.x + side, top, bottom, &left);
    column(p, area.right() - side, area.right(), top, bottom, &right);
    browser(p, Rect::span(area.x + side + M, area.right() - side - M, top, bottom));
}

// -------------------------------------------------------------------------------------------------------------------
// Preset browser

/// A five-pointed star centred at (cx, cy), outer radius `r`.
fn star(c: &mut Canvas, cx: f32, cy: f32, r: f32, fill: Option<Color>, line: Color) {
    let pts: Vec<(f32, f32)> = (0..10)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
            let rr = if i % 2 == 0 { r } else { r * 0.45 };
            (cx + rr * a.cos(), cy + rr * a.sin())
        })
        .collect();
    match fill {
        Some(f) => c.fill_polygon(&pts, f),
        None => {
            let mut closed = pts.clone();
            closed.push(pts[0]);
            c.polyline(&closed, 1.3, line);
        }
    }
}

/// A chip's box (lit when `on`); returns the ink for what is drawn on it.
fn chip_box(p: &mut P, cr: Rect, t: &Target, on: bool) -> Color {
    let st = p.state(cr, t, false);
    let ink = lemon::choice_button(p.c, cr, on, brand::MAIN, st);
    if !on {
        p.c.stroke_rrect(cr.inset(0.5), 4.5, 1.0, Color::rgb(0x3e3e3e));
    }
    ink
}

/// A kind of sound's colour (its chip's label, its presets' dots).
pub fn kind_color(k: Kind) -> Color {
    match k {
        Kind::Lead => brand::MAIN,
        Kind::Bass => Color::rgb(0xe08a4a),
        Kind::Keys => Color::rgb(0x5cc0b0),
        Kind::Pad => Color::rgb(0x9d93e0),
        Kind::Fx => Color::rgb(0xd77aa8),
        Kind::Drums => Color::rgb(0xd4625a),
        Kind::Seq => Color::rgb(0x6aa6dc),
    }
}

/// A previous / next arrow in `r`.
fn arrow(p: &mut P, r: Rect, t: Target, left: bool) {
    let st = p.state(r, &t, false);
    if st.hover || st.pressed {
        p.c.fill_rrect(r, 6.0, Color::rgb(0x2a2a2a));
    }
    let (cx, cy) = (r.cx(), r.cy());
    let d = if left { -1.0 } else { 1.0 };
    let col = if st.hover { brand::MAIN } else { plug::DIM };
    p.c.polyline(&[(cx - 3.0 * d, cy - 7.0), (cx + 3.0 * d, cy), (cx - 3.0 * d, cy + 7.0)], 2.2, col);
    p.hit(r, t);
}

fn browser(p: &mut P, r: Rect) {
    lemon::plug_panel(p.c, r);
    heading(p, r.x, r.right(), r.y - 11.0, "PRESETS", false);
    // The screen: the visualizer, the current preset's name and its author.
    let screen = Rect::new(r.x + 8.0, r.y + 8.0, r.w - 16.0, 84.0);
    lemon::plug_well(p.c, screen, 8.0);
    p.out.screen = screen;
    let cur = p.v.current;
    arrow(p, Rect::new(screen.x + 4.0, screen.cy() - 16.0, 26.0, 32.0), Target::Prev, true);
    arrow(p, Rect::new(screen.right() - 30.0, screen.cy() - 16.0, 26.0, 32.0), Target::Next, false);
    let grid = p.v.viz.paint(p.c, screen.x + 34.0, screen.cy());
    let inner = Rect::span(grid.right() + 10.0, screen.right() - 34.0, screen.y, screen.bottom());
    let (name, name_col) = if cur.name.is_empty() { ("No preset loaded".to_string(), plug::FAINT) } else { (cur.name.clone(), brand::TEXT) };
    let f = p.c.fit_font(NAME, &name, inner.w - 8.0);
    let lh = p.c.line_height(f);
    let sub = match (&cur.author, cur.modified) {
        (Some(a), true) => format!("{}  ·  EDITED", a.to_uppercase()),
        (Some(a), false) => a.to_uppercase(),
        (None, true) => "EDITED".into(),
        (None, false) => String::new(),
    };
    let clh = p.c.line_height(CAPTION);
    let block = if sub.is_empty() { lh } else { lh + 6.0 + clh };
    let ny = (screen.cy() - block / 2.0).round();
    let nw = p.c.text_width(f, &name);
    p.c.text(f, (inner.cx() - nw / 2.0).round(), ny, name_col, &name);
    if !sub.is_empty() {
        let sw = lemon::spaced_width(p.c, CAPTION, &sub, 1.2);
        lemon::spaced(p.c, CAPTION, (inner.cx() - sw / 2.0).round(), (ny + lh + 6.0).round(), plug::DIM, &sub, 1.2);
    }
    // the star, top right of the screen
    let sr = Rect::new(screen.right() - 30.0, screen.y + 4.0, 24.0, 22.0);
    let st = p.state(sr, &Target::Star, cur.path.is_none());
    let col = if cur.favourite { Some(brand::MAIN) } else { None };
    star(p.c, sr.cx(), sr.cy(), 7.5, col, if st.hover { brand::TEXT } else { plug::FAINT });
    if cur.path.is_some() {
        p.hit(sr, Target::Star);
    }

    // Search box and Save.
    let y = screen.bottom() + 8.0;
    let save = Rect::new(r.right() - 8.0 - 58.0, y, 58.0, 22.0);
    let stv = p.state(save, &Target::Save, false);
    let ink = lemon::plug_button(p.c, save, 6.0, stv);
    p.c.text_center(LABEL, save, ink, "SAVE");
    p.hit(save, Target::Save);
    let sb = Rect::span(r.x + 8.0, save.x - 6.0, y, y + 22.0);
    let sst = p.state(sb, &Target::Search, false);
    lemon::field(p.c, sb, sst);
    if p.v.search_focus {
        p.c.stroke_rrect(sb.inset(0.5), 5.5, 1.0, brand::DEEP);
    }
    // magnifier
    let (mx, my) = (sb.x + 12.0, sb.cy() - 1.0);
    p.c.stroke_circle(mx, my, 4.0, 1.4, plug::FAINT);
    p.c.line(mx + 3.0, my + 3.0, mx + 6.5, my + 6.5, 1.6, plug::FAINT);
    let lh = p.c.line_height(SMALL);
    let ty = (sb.cy() - lh / 2.0).round();
    if p.v.query.is_empty() && !p.v.search_focus {
        p.c.text(SMALL, sb.x + 24.0, ty, plug::FAINT, "Search presets");
    } else {
        p.c.text_clip(SMALL, sb.x + 24.0, ty, sb.w - 30.0, brand::TEXT, p.v.query);
        if p.v.search_focus && p.v.caret {
            let cx = (sb.x + 24.0 + p.c.text_width(SMALL, p.v.query) + 1.0).min(sb.right() - 6.0);
            p.c.fill_rect(Rect::new(cx.round(), ty + 1.0, 1.0, lh - 2.0), brand::MAIN);
        }
    }
    p.hit(sb, Target::Search);

    // The chips, one row: the banks, favourites and every preset, then the kinds of sound (each in its colour), then
    // ⋯ for the rest (lit while one of its chips is chosen).
    let lib = p.v.lib;
    let mut chips: Vec<(Chip, String)> = lib.banks.iter().map(|g| (Chip::Group(g.0.clone()), g.0.clone())).collect();
    chips.extend([(Chip::Favorites, "★".into()), (Chip::All, "All".into())]);
    let collections = chips.len();
    chips.extend(lib.kinds().into_iter().take(ROW_KINDS).map(|k| (Chip::Kind(k), k.label().to_string())));
    let more = crate::ui::Ui::more_chips(lib);
    let (mut x, cy) = (r.x + 8.0, sb.bottom() + 8.0);
    let ch = 18.0;
    for (i, (chip, label)) in chips.into_iter().enumerate() {
        if i == collections {
            // a quiet divider between the collections and the kinds
            p.c.fill_rect(Rect::new(x + 1.0, cy + 4.0, 1.0, ch - 8.0), Color::rgb(0x3e3e3e));
            x += 7.0;
        }
        let star_chip = chip == Chip::Favorites;
        let w = if star_chip { 24.0 } else { (p.c.text_width(LABEL, &label) + 12.0).round() };
        let cr = Rect::new(x, cy, w, ch);
        let t = Target::Chip(chip.clone());
        let on = *p.v.chip == chip;
        let ink = chip_box(p, cr, &t, on);
        let ink = match chip {
            Chip::Kind(k) if !on => kind_color(k),
            _ => ink,
        };
        if star_chip {
            star(p.c, cr.cx(), cr.cy(), 5.0, Some(ink), ink);
        } else {
            p.c.text_center(LABEL, cr, ink, &label);
        }
        p.hit(cr, t);
        x += w + 4.0;
    }
    if !more.is_empty() {
        let cr = Rect::new(r.right() - 8.0 - 24.0, cy, 24.0, ch);
        let on = more.iter().any(|(c, _)| c == p.v.chip);
        let ink = chip_box(p, cr, &Target::More, on);
        for d in [-4.0, 0.0, 4.0] {
            p.c.fill_circle(cr.cx() + d, cr.cy(), 1.3, ink);
        }
        p.hit(cr, Target::More);
    }

    // The list.
    let list = Rect::span(r.x + 8.0, r.right() - 8.0, cy + ch + 8.0, r.bottom() - 8.0);
    lemon::plug_well(p.c, list, 6.0);
    let rows = ((list.h - 8.0) / LIST_ROW).floor().max(1.0);
    p.out.list = list;
    p.out.list_rows = rows;
    let n = p.v.shown.len();
    let first = p.v.list_scroll.clamp(0.0, (n as f32 - rows).max(0.0)).floor() as usize;
    let inner = Rect::span(list.x + 4.0, list.right() - 12.0, list.y + 4.0, list.bottom() - 4.0);
    p.c.push_clip(inner);
    let lh = p.c.line_height(SMALL);
    let stars = match p.v.chip {
        Chip::Favorites => false,
        Chip::Group(g) => !lib.banks.iter().any(|b| b.0 == *g),
        _ => true,
    };
    for (k, &i) in p.v.shown.iter().enumerate().skip(first).take(rows as usize + 1) {
        let pr = &p.v.lib.presets[i];
        let row = Rect::new(inner.x, inner.y + (k - first) as f32 * LIST_ROW, inner.w, LIST_ROW);
        let t = Target::Preset(i);
        let st = p.state(row, &t, false);
        let current = cur.path.as_deref() == Some(pr.path.as_path());
        if current {
            p.c.fill_rrect(row, 4.0, Color::rgb(0x3a3418));
        } else if st.hover {
            p.c.fill_rrect(row, 4.0, Color::rgb(0x2c2c2c));
        }
        let ty = (row.cy() - lh / 2.0).round();
        // the kind's dot; a star at the end where it says something (not in a list of favourites)
        if let Some(k) = pr.kind {
            p.c.fill_circle(row.x + 9.0, row.cy(), 3.0, kind_color(k));
        }
        let starred = stars && p.v.favs.contains(&pr.path);
        if starred {
            star(p.c, row.right() - 9.0, row.cy(), 4.5, Some(if current { brand::MAIN } else { brand::DEEP }), brand::DEEP);
        }
        let rw = if starred { 18.0 } else { 4.0 };
        p.c.text_clip(SMALL, row.x + 18.0, ty, row.w - 18.0 - rw, if current { brand::MAIN } else { brand::TEXT }, &pr.name);
        p.hit(row, t);
    }
    p.c.pop_clip();
    if n == 0 {
        let msg = if p.v.lib.presets.is_empty() { "No presets found" } else { "Nothing matches" };
        p.c.text_center(SMALL, list, plug::FAINT, msg);
    }
    // Scroll bar.
    if n as f32 > rows {
        let track = Rect::new(list.right() - 9.0, list.y + 4.0, 5.0, list.h - 8.0);
        p.c.fill_rrect(track, 2.5, Color::rgb(0x2a2a2a));
        let th = (track.h * rows / n as f32).max(18.0);
        let ty = track.y + (track.h - th) * (first as f32 / (n as f32 - rows));
        let tr = Rect::new(track.x, ty, track.w, th);
        let st = p.state(track.outset(3.0), &Target::ListBar, false);
        p.c.fill_rrect(tr, 2.5, if st.hover || st.pressed { Color::rgb(0x6a6a6a) } else { Color::rgb(0x4a4a4a) });
        p.hit(track.outset(3.0), Target::ListBar);
    }
}

// -------------------------------------------------------------------------------------------------------------------
// Effects page

fn effects_page(p: &mut P, area: Rect) {
    let top = area.y + 22.0;
    let bottom = area.bottom() - 2.0;
    let half = (area.w - M) / 2.0;
    for slot in 1..=2 {
        let x0 = area.x + (slot - 1) as f32 * (half + M);
        effect_card(p, Rect::new(x0, top, half, bottom - top), slot);
    }
}

fn effect_card(p: &mut P, r: Rect, slot: usize) {
    let s = pages::effect_slot(p.v.table, p.m, slot);
    let on = p.m.on(&s.on);
    lemon::plug_panel(p.c, r);
    heading(p, r.x, r.right(), r.y - 11.0, &format!("EFFECT {slot}"), !on);
    // On switch and the module pills.
    let y = r.y + 8.0;
    switches(p, Rect::new(r.x + 6.0, y, 48.0, 22.0), &[(s.on.as_str(), "ON")], false);
    let labels: Vec<&str> = pages::MODULES.iter().map(|(l, _)| *l).collect();
    pills_h(p, Rect::span(r.x + 62.0, r.right() - 8.0, y + 2.0, y + 21.0), &s.module, &labels, false);
    let gy = y + 30.0;
    p.c.fill_rect(Rect::span(r.x + 10.0, r.right() - 10.0, gy - 2.0, gy - 1.0), plug::PANEL_LINE);
    // The module's controls: up to ten, two rows of five.
    let n = s.params.len();
    let cols = 5usize;
    let rows = n.div_ceil(cols).max(1);
    let cw = (r.w - 8.0) / cols as f32;
    let rh = ((r.bottom() - gy - 8.0) / rows as f32).min(150.0);
    let top = gy + ((r.bottom() - gy - 8.0) - rh * rows as f32) / 2.0;
    for (k, key) in s.params.iter().enumerate() {
        let (row, col) = (k / cols, k % cols);
        let in_row = if row + 1 == rows { n - row * cols } else { cols };
        let x0 = r.x + 4.0 + (r.w - 8.0 - cw * in_row as f32) / 2.0;
        let cell = Rect::new(x0 + cw * col as f32, top + rh * row as f32, cw, rh);
        effect_control(p, cell, key, !on);
    }
}

/// An effect's control: a big dial with caption and value, pills for a choice, a toggle for a switch.
fn effect_control(p: &mut P, cell: Rect, key: &str, slot_off: bool) {
    let Some(info) = p.v.table.get(key).cloned() else { return };
    let cap = info.name.to_uppercase();
    if info.is_switch() {
        switches(p, cell, &[(key, &cap)], slot_off);
        return;
    }
    if info.is_choice() {
        let labels: Vec<String> = info.labels.iter().map(|l| l.to_uppercase()).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let c2 = Rect::new(cell.x + 8.0, cell.y + 10.0, cell.w - 16.0, cell.h - 20.0);
        pills_v(p, c2, key, &refs, Some(&cap), slot_off);
        return;
    }
    let r = (cell.w * 0.33).min((cell.h - 50.0) / 2.0).clamp(14.0, 34.0).round();
    let block = 2.0 * r + 30.0;
    let cy = (cell.y + (cell.h - block) / 2.0 + r).round();
    let bipolar = info.min < 0.0 && info.max > 0.0;
    let n = info.norm(p.m.get(key));
    dial(p, Target::Knob(key.into()), cell.cx().round(), cy, r, n, bipolar, &cap, slot_off);
    let text = p.m.text(key, p.m.get(key)).unwrap_or_else(|| info.format(p.m.get(key)));
    let f = SMALL;
    let tw = p.c.text_width(f, &text);
    p.c.text(f, (cell.cx() - tw / 2.0).round(), cy + r + 18.0, if slot_off { plug::OFF } else { brand::TEXT }, &text);
}

// -------------------------------------------------------------------------------------------------------------------
// Advanced page

/// Column width of the Advanced page (at least).
const ADV_COL: f32 = 214.0;

/// Places the sections in columns (each into the shortest so far): (column, y) per section, column count and width,
/// and the content height.
pub fn advanced_layout(sections: &[(String, Vec<String>)], width: f32) -> (Vec<(usize, f32)>, usize, f32, f32) {
    let cols = (((width + M) / (ADV_COL + M)).floor() as usize).max(1);
    let cw = ((width - M * (cols as f32 - 1.0)) / cols as f32).floor();
    let mut heights = vec![0.0f32; cols];
    let mut at = Vec::new();
    for (_, keys) in sections {
        let (col, _) = heights.iter().enumerate().fold((0, f32::MAX), |b, (i, &h)| if h < b.1 { (i, h) } else { b });
        at.push((col, heights[col]));
        heights[col] += section_h(keys.len());
    }
    let total = heights.iter().copied().fold(0.0, f32::max);
    (at, cols, cw, total)
}

/// A section's height: its heading, the card and the gap after it.
fn section_h(rows: usize) -> f32 {
    22.0 + rows as f32 * ROW_H + 10.0 + 6.0
}

fn advanced_page(p: &mut P, area: Rect) {
    let view = Rect::span(area.x, area.right(), area.y + 6.0, area.bottom());
    let sections = pages::advanced_sections(p.v.table);
    let mut width = view.w;
    let (_, _, _, total) = advanced_layout(&sections, width);
    let scrolls = total > view.h;
    if scrolls {
        width -= 12.0;
    }
    let (at, _, cw, total) = advanced_layout(&sections, width);
    p.out.adv = view;
    p.out.adv_height = total;
    let scroll = p.v.adv_scroll.clamp(0.0, (total - view.h).max(0.0));
    p.c.push_clip(view);
    for ((title, keys), (col, y0)) in sections.iter().zip(&at) {
        let x0 = view.x + *col as f32 * (cw + M);
        let y = view.y + y0 - scroll;
        if y > view.bottom() || y + section_h(keys.len()) < view.y {
            continue;
        }
        let card = Rect::new(x0, y + 16.0, cw, keys.len() as f32 * ROW_H + 10.0);
        lemon::plug_panel(p.c, card);
        heading(p, card.x, card.right(), card.y - 9.0, &title.to_uppercase(), false);
        for (k, key) in keys.iter().enumerate() {
            let row = Rect::new(card.x + 4.0, card.y + 5.0 + k as f32 * ROW_H, card.w - 8.0, ROW_H);
            adv_row(p, row, key, view);
        }
    }
    p.c.pop_clip();
    if scrolls {
        let track = Rect::new(view.right() - 6.0, view.y, 5.0, view.h);
        p.c.fill_rrect(track, 2.5, Color::rgb(0x262626));
        let th = (track.h * view.h / total).max(24.0);
        let ty = track.y + (track.h - th) * (scroll / (total - view.h));
        let st = p.state(track.outset(3.0), &Target::AdvBar, false);
        p.c.fill_rrect(Rect::new(track.x, ty, track.w, th), 2.5, if st.hover || st.pressed { Color::rgb(0x6a6a6a) } else { Color::rgb(0x4a4a4a) });
        p.hit(track.outset(3.0), Target::AdvBar);
    }
}

/// One parameter on the Advanced page: its name and a value bar, a toggle or a choice box.
fn adv_row(p: &mut P, row: Rect, key: &str, view: Rect) {
    let Some(info) = p.v.table.get(key).cloned() else { return };
    let disabled = p.veiled(key);
    let visible = row.y >= view.y - 1.0 && row.bottom() <= view.bottom() + 1.0;
    let hover = p.v.mouse.is_some_and(|(x, y)| row.contains(x, y) && view.contains(x, y));
    if hover {
        p.c.fill_rrect(row, 4.0, Color::rgb(0x3a3a3a));
    }
    let lh = p.c.line_height(SMALL);
    let vw = (row.w * 0.42).clamp(70.0, 96.0);
    let name_w = row.w - vw - 10.0;
    p.c.text_clip(SMALL, row.x + 4.0, (row.cy() - lh / 2.0).round(), name_w, if disabled { plug::OFF } else { plug::DIM }, &info.name);
    let vr = Rect::new(row.right() - vw, row.y + 2.0, vw, row.h - 4.0);
    if info.is_switch() {
        let t = Target::Toggle(key.into());
        let hr = Rect::new(vr.x, row.y, vr.w, row.h);
        let mut st = p.state(hr, &t, disabled);
        st.on = p.m.on(key);
        lemon::toggle(p.c, vr.right() - 8.0, row.cy(), st.on, st);
        if visible {
            p.hit(hr, t);
        }
    } else if info.is_choice() {
        if visible {
            menu_box(p, vr, key, disabled);
        } else {
            lemon::field(p.c, vr, State::NORMAL);
        }
    } else {
        let t = Target::Value(key.into());
        let st = p.state(vr, &t, disabled);
        lemon::field(p.c, vr, st);
        let n = info.norm(p.m.get(key));
        let bipolar = info.min < 0.0 && info.max > 0.0;
        let inner = vr.inset(2.0);
        let (a, b) = if bipolar { (inner.x + inner.w / 2.0, inner.x + inner.w * n) } else { (inner.x, inner.x + inner.w * n) };
        let col = if disabled { Color::rgb(0x4a4a4a) } else { brand::MAIN.with_alpha(if st.hover || st.pressed { 110 } else { 80 }) };
        if (b - a).abs() > 0.5 {
            p.c.fill_rrect(Rect::span(a.min(b), a.max(b), inner.y, inner.bottom()), 3.0, col);
        }
        let text = p.m.text(key, p.m.get(key)).unwrap_or_else(|| info.format(p.m.get(key)));
        let f = p.c.fit_font(LABEL, &text, vr.w - 6.0);
        p.c.text_center(f, vr, if disabled { plug::OFF } else { brand::TEXT }, &text);
        if visible {
            p.hit(vr, t);
        }
    }
}
