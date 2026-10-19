// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Reverb editor: painting and hit regions (platform independent), in the Lemon "Pith" plugin look. The room
//! visual fills a dark bezel at the left (the original's place); the controls keep the original's four columns as
//! outlined cards under ruled headings: INPUT (mid/side, the cuts), ROOM (pre-delay and its tempo switch, size,
//! diffusion, modulation), DECAY (bass, decay, crossover, damping, the wordmark where the original has its logo) and
//! MIX (dry, early and wet faders over the separation knob).
//!
// spec: plugins/lemon-reverb.md (§1 window, §2 room visual, §3 hints, "Look"), ui/lemon-design.md §11.1

use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{eq as keq, inst, State};
use os_ui::{Canvas, Color, Font, Rect};

use crate::room;

/// Editor size in logical pixels.
pub const W: u32 = 664;
pub const H: u32 = 248;

/// The room visual's bezel and its picture (the original's 160 px panel shown at 224 px).
pub const BEZEL: Rect = Rect::span(8.0, 240.0, 8.0, 240.0);
pub const PICTURE: Rect = Rect::new(12.0, 12.0, 224.0, 224.0);

/// Knob rows (centres; the cards keep 13 px above the first dial and below the last caption) and the cards' span.
const ROWS: [f32; 3] = [62.0, 126.0, 190.0];
const CARD_TOP: f32 = 28.0;
const CARD_BOT: f32 = 240.0;
const HEAD_Y: f32 = 16.0;
/// A caption's top under its control's centre (every caption of a row on one line).
const CAPTION_DY: f32 = 26.0;

/// Columns: (left, right, heading).
const COLUMNS: [(f32, f32, &str); 4] = [(248.0, 322.0, "INPUT"), (328.0, 446.0, "ROOM"), (452.0, 570.0, "DECAY"), (576.0, 656.0, "MIX")];
/// Two controls side by side sit this far either side of their card's centre.
const PAIR: f32 = 26.0;

/// The level faders' travel (top = 160, the knob's top; bottom = 0), between the first knob row and the SEP knob.
pub const FADER_TOP: f32 = 44.0;
pub const FADER_BOT: f32 = 136.0;
pub const FADER_X: [f32; 3] = [mid(3) - 25.0, mid(3), mid(3) + 25.0];

/// A card's centre.
const fn mid(i: usize) -> f32 {
    (COLUMNS[i].0 + COLUMNS[i].1) / 2.0
}

const RULE: Color = Color::rgb(0x3c3c3c);
const HEADING: Font = Font::condensed(11, 0).bold();
const SMALL: Font = Font::new(11);
const PILL: Font = Font::new(10).bold();

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
    /// A level fader: drag / wheel / click to jump; menu on right click.
    Fader(String),
    /// A switch (click toggles; menu on right click).
    Switch(String),
    /// The room visual (its hint names the setting it shows).
    Room,
    /// The wordmark (the About menu).
    Logo,
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

/// What a paint shows besides the parameters: the room visual's look and turn.
pub struct Look {
    pub room: room::Look,
    pub angle: f32,
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
}

/// A fader's y for a normalised value.
pub fn fader_y(norm: f32) -> f32 {
    FADER_BOT - (FADER_BOT - FADER_TOP) * norm.clamp(0.0, 1.0)
}

/// Paints the editor. `hint` (not hosted: the host shows it otherwise) goes in the picture's top-left corner.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, look: &Look, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    picture(&mut ui, look, hint);
    headings(&mut ui);
    for (x0, x1, _) in COLUMNS {
        lemon::plug_panel(ui.c, Rect::span(x0, x1, CARD_TOP, CARD_BOT));
    }
    let (l, r) = (|i: usize| mid(i) - PAIR, |i: usize| mid(i) + PAIR);
    // INPUT
    switch(&mut ui, "midside", mid(0), ROWS[0], "M/S", "MID/SIDE");
    knob(&mut ui, "hcut", mid(0), ROWS[1], "H.CUT");
    knob(&mut ui, "lcut", mid(0), ROWS[2], "L.CUT");
    // ROOM: pre-delay and its tempo switch; size and diffusion with the modulation beside them.
    knob(&mut ui, "delay", l(1), ROWS[0], "DELAY");
    switch(&mut ui, "tempo", r(1), ROWS[0], "BPM", "TEMPO");
    knob(&mut ui, "size", l(1), ROWS[1], "SIZE");
    knob(&mut ui, "mod", r(1), ROWS[1], "MOD");
    knob(&mut ui, "diff", l(1), ROWS[2], "DIFF");
    knob(&mut ui, "speed", r(1), ROWS[2], "SPEED");
    // DECAY
    knob(&mut ui, "bass", l(2), ROWS[0], "BASS");
    knob(&mut ui, "dec", r(2), ROWS[0], "DEC");
    knob(&mut ui, "cross", l(2), ROWS[1], "CROSS");
    knob(&mut ui, "damp", r(2), ROWS[1], "DAMP");
    logo(&mut ui);
    // MIX
    for (id, x, cap) in [("dry", FADER_X[0], "DRY"), ("er", FADER_X[1], "ER"), ("wet", FADER_X[2], "WET")] {
        fader(&mut ui, id, x, cap);
    }
    knob(&mut ui, "sep", mid(3), ROWS[2], "SEP");
    ui.hits
}

/// The column headings over their cards (as Lemon Limiter's): letter-spaced captions on a hairline rule.
fn headings(ui: &mut Ui) {
    let lh = ui.c.line_height(HEADING);
    for (x0, x1, s) in COLUMNS {
        let w = lemon::spaced_width(ui.c, HEADING, s, 2.0);
        let tx = ((x0 + x1 + 1.0) / 2.0 - w / 2.0).round();
        lemon::spaced(ui.c, HEADING, tx, (HEAD_Y - lh / 2.0).round(), plug::DIM, s, 2.0);
        ui.c.fill_rect(Rect::span(x0 + 4.0, tx - 7.0, HEAD_Y, HEAD_Y), RULE);
        ui.c.fill_rect(Rect::span((tx + w + 6.0).round(), x1 - 4.0, HEAD_Y, HEAD_Y), RULE);
    }
}

/// A knob (hit area 44 px across), caption under its centre.
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, caption: &str) {
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - 22.0, cy - 22.0, 44.0, 44.0);
    let st = ui.state(hr, &target);
    lemon::dial(ui.c, cx, cy, 19.0, 2.8, ui.m.norm(id), id == "sep", brand::MAIN, st);
    inst::caption(ui.c, ui.t, cx, cy + CAPTION_DY, caption, false);
    ui.hit(hr, target);
}

/// A switch: a raised button lit lemon when on, its short label inside, the caption under it.
fn switch(ui: &mut Ui, id: &str, cx: f32, cy: f32, label: &str, caption: &str) {
    let target = Target::Switch(id.into());
    let r = Rect::new(cx - 20.0, cy - 10.0, 40.0, 20.0);
    let hr = Rect::new(cx - 24.0, cy - 22.0, 48.0, 44.0);
    let mut st = ui.state(hr, &target);
    st.on = ui.m.norm(id) > 0.5;
    let ink = lemon::plug_button(ui.c, r, 6.0, st);
    ui.c.text_center(PILL, r, ink, label);
    inst::caption(ui.c, ui.t, cx, cy + CAPTION_DY, caption, false);
    ui.hit(hr, target);
}

/// A level fader: a slot, a tick at 100 %, a lemon fill up to the handle, the caption under the travel.
fn fader(ui: &mut Ui, id: &str, cx: f32, caption: &str) {
    let target = Target::Fader(id.into());
    let hr = Rect::new(cx - 11.0, FADER_TOP - 8.0, 22.0, FADER_BOT - FADER_TOP + 16.0);
    let st = ui.state(hr, &target);
    let slot = Rect::new(cx - 3.0, FADER_TOP - 4.0, 6.0, FADER_BOT - FADER_TOP + 8.0);
    keq::slot(ui.c, slot, st);
    // 100 % (128 of 160).
    let y100 = fader_y(128.0 / 160.0).round();
    for side in [-1.0, 1.0] {
        ui.c.fill_rect(Rect::new(cx + side * 6.0 - if side < 0.0 { 3.0 } else { 0.0 }, y100, 3.0, 1.0), plug::FAINT);
    }
    let y = fader_y(ui.m.norm(id));
    if FADER_BOT - y > 1.0 {
        ui.c.fill_rrect(Rect::span(cx - 1.0, cx + 1.0, y, FADER_BOT), 1.0, brand::MAIN.with_alpha(170));
    }
    keq::handle(ui.c, cx, y.round(), brand::MAIN, st);
    inst::caption(ui.c, ui.t, cx, FADER_BOT + 14.0, caption, false);
    ui.hit(hr, target);
}

/// The wordmark in the decay card's third row (where the original has its logo): the lemon and "LEMON" over a large
/// "REVERB", centred between the second row's captions and the card's foot.
fn logo(ui: &mut Ui) {
    let (big, small) = (Font::condensed(20, 0).bold(), Font::condensed(10, 0).bold());
    let wn = lemon::spaced_width(ui.c, big, "REVERB", 3.0);
    let x = (mid(2) + wn / 2.0).round();
    let top = ROWS[2] - 8.0;
    lemon::wordmark(ui.c, x, top, false, "REVERB", big, small);
    ui.hit(Rect::span(COLUMNS[2].0 + 6.0, COLUMNS[2].1 - 6.0, top - 6.0, top + 40.0), Target::Logo);
}

/// The room visual in its bezel; the hint over its top-left corner.
fn picture(ui: &mut Ui, look: &Look, hint: &str) {
    lemon::plug_well(ui.c, BEZEL, 8.0);
    let (_, _, w, h) = ui.c.device_rect(PICTURE);
    let side = w.min(h).max(1);
    let px = room::render(&look.room, look.angle, side);
    ui.c.image(PICTURE, side, side, &px);
    if !hint.is_empty() {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        let s = if right.is_empty() { left.to_string() } else { format!("{left}  ({right})") };
        let tw = ui.c.text_width(SMALL, &s).min(PICTURE.w - 12.0);
        let lh = ui.c.line_height(SMALL);
        ui.c.fill_rrect(Rect::new(PICTURE.x + 3.0, PICTURE.y + 3.0, tw + 10.0, lh + 4.0), 4.0, Color::rgb(0x1a1a1a).with_alpha(150));
        ui.c.text_clip(SMALL, PICTURE.x + 8.0, PICTURE.y + 5.0, PICTURE.w - 12.0, brand::TEXT, &s);
    }
    ui.hit(PICTURE, Target::Room);
}

// ---------------------------------------------------------------------------------------------------------------
// Hints (spec §3)

/// The hint line for a target (a tab before the right-aligned part).
pub fn hint(t: &Target, m: &dyn Model, names: &dyn Fn(&str) -> String) -> String {
    match t {
        Target::Knob(id) | Target::Fader(id) => format!("{}: {}", names(id), m.text(id)),
        Target::Switch(id) => {
            let on = if m.norm(id) > 0.5 { "on" } else { "off" };
            format!("{}\t{on}", names(id))
        }
        Target::Room => "Room".into(),
        Target::Logo => "Lemon Reverb".into(),
    }
}
