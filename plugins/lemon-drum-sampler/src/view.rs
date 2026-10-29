// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Drum Sampler editor: painting and hit regions (platform independent), in the Lemon "Pith" plugin look on a
//! 6 px grid: the header band (the lemon mark and "DRUMS" on one line, the bank buttons), the 4 × 4 pad grid (pad 1
//! at the bottom left), and the cards of the chosen pad (its key, cut groups and levels) and of its chosen layer (its
//! sample, velocity range and levels).
//!
// spec: plugins/lemon-drum-sampler.md §3, ui/lemon-design.md §11.1

use os_theme::design::lemon as brand;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{inst, State as Kst};
use os_ui::{Canvas, Color, Font, Rect};

use crate::state::{Pad, State};

/// Editor size in logical pixels.
pub const W: u32 = 618;
pub const H: u32 = 378;

/// Every card 6 px from the editor's edges and from its neighbours.
const GAP: f32 = 6.0;
/// A rect from its left, right, top and bottom edges (right and bottom exclusive).
const fn edges(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}
const RIGHT: f32 = W as f32 - GAP;
const BOTTOM: f32 = H as f32 - GAP;

/// The header band and its centre line.
const HEAD: (f32, f32) = (GAP, 36.0);
const HEAD_Y: f32 = (HEAD.0 + HEAD.1) / 2.0;
pub const LOGO: Rect = edges(GAP, 156.0, HEAD.0, HEAD.1);
/// The bank buttons.
pub const BANK_A: Rect = edges(228.0, 276.0, HEAD_Y - 11.0, HEAD_Y + 11.0);
pub const BANK_B: Rect = edges(282.0, 330.0, HEAD_Y - 11.0, HEAD_Y + 11.0);
/// The pad grid's well; pads 75 px square, 6 px apart and from the well's edges.
pub const GRID: Rect = edges(GAP, 336.0, HEAD.1 + GAP, BOTTOM);
const PAD: f32 = 75.0;
/// The chosen pad's and layer's cards.
const COL: (f32, f32) = (342.0, RIGHT);
pub const PAD_CARD: Rect = edges(COL.0, COL.1, HEAD.1 + GAP, 192.0);
pub const LAYER_CARD: Rect = edges(COL.0, COL.1, 198.0, BOTTOM);
/// A card's title band.
const BAND: f32 = 18.0;
/// The value boxes' line and the knobs' line in each card; captions under them.
const PAD_BOX_Y: f32 = 102.0;
const PAD_KNOB_Y: f32 = 150.0;
const LAYER_BOX_Y: f32 = 258.0;
const LAYER_KNOB_Y: f32 = 318.0;
const BOX_X: [f32; 3] = [384.0, 474.0, 564.0];
const KNOB_X: [f32; 3] = [384.0, 474.0, 564.0];
const KNOB_R: f32 = 17.0;
/// The pad's name and the layer's sample fields; the layer's arrows and its load button.
pub const NAME: Rect = edges(COL.0 + 8.0, COL.1 - 8.0, HEAD.1 + GAP + BAND + 6.0, HEAD.1 + GAP + BAND + 24.0);
pub const PREV: Rect = edges(COL.0 + 6.0, COL.0 + 30.0, 199.0, 198.0 + BAND);
pub const NEXT: Rect = edges(COL.1 - 30.0, COL.1 - 6.0, 199.0, 198.0 + BAND);
pub const SAMPLE: Rect = edges(COL.0 + 8.0, 534.0, 198.0 + BAND + 6.0, 198.0 + BAND + 24.0);
pub const LOAD: Rect = edges(540.0, COL.1 - 8.0, 198.0 + BAND + 5.0, 198.0 + BAND + 25.0);

const SMALL: Font = Font::new(10);
const LABEL: Font = Font::new(11);
const PILL: Font = Font::new(10).bold();

/// The pad and layer settings the editor shows as boxes and knobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Key,
    Cut,
    CutBy,
    PadVolume,
    PadPan,
    PadTune,
    VelLo,
    VelHi,
    LayerVolume,
    LayerPan,
    LayerTune,
}

impl Field {
    /// The value range.
    pub fn range(self) -> (i32, i32) {
        match self {
            Field::Key | Field::VelLo | Field::VelHi => (0, 127),
            Field::Cut | Field::CutBy => (-1, 15),
            Field::PadVolume | Field::LayerVolume => (0, 128),
            Field::PadPan | Field::LayerPan | Field::PadTune | Field::LayerTune => (-128, 128),
        }
    }

    pub fn bipolar(self) -> bool {
        matches!(self, Field::PadPan | Field::LayerPan | Field::PadTune | Field::LayerTune)
    }

    /// The hint's name.
    pub fn name(self) -> &'static str {
        match self {
            Field::Key => "Pad key",
            Field::Cut => "Cut group",
            Field::CutBy => "Cut by group",
            Field::PadVolume => "Pad volume",
            Field::PadPan => "Pad panning",
            Field::PadTune => "Pad tune",
            Field::VelLo => "Layer lowest velocity",
            Field::VelHi => "Layer highest velocity",
            Field::LayerVolume => "Layer volume",
            Field::LayerPan => "Layer panning",
            Field::LayerTune => "Layer tune",
        }
    }

    /// A value's text.
    pub fn text(self, v: i32) -> String {
        match self {
            Field::Key => key_name(v),
            Field::Cut | Field::CutBy => {
                if v < 0 { "--".into() } else { (v + 1).to_string() }
            }
            Field::PadVolume | Field::LayerVolume => format!("{}%", (v as f32 * 100.0 / 128.0).round()),
            Field::PadPan | Field::LayerPan => match v {
                0 => "Centered".into(),
                v if v < 0 => format!("{}% left", (-v as f32 * 100.0 / 128.0).round()),
                v => format!("{}% right", (v as f32 * 100.0 / 128.0).round()),
            },
            Field::PadTune | Field::LayerTune => {
                let c = crate::dsp::tune_cents(v);
                if c == 0.0 { "0 cents".into() } else { format!("{c:+.0} cents") }
            }
            Field::VelLo | Field::VelHi => v.to_string(),
        }
    }
}

/// The name of key `k` (60 = C5).
pub fn key_name(k: i32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[k.rem_euclid(12) as usize], k.div_euclid(12))
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Pad `n` (0..32): a click chooses it and plays it.
    Pad(usize),
    Bank(usize),
    /// A value box or knob: vertical drag / wheel; type-in on right click.
    Value(Field),
    Prev,
    Next,
    Load,
    Logo,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// The pointer, the pressed control, the bank shown and the chosen pad and layer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
    pub bank: usize,
    pub pad: usize,
    pub layer: usize,
}

/// What a paint shows besides the state.
pub struct Look<'a> {
    pub state: &'a State,
    /// Whether each pad has a sample that opened.
    pub loaded: &'a [bool],
    /// Pads lit by a note just now.
    pub lit: &'a [bool],
}

struct Ui<'a> {
    c: &'a mut Canvas,
    t: &'a Palette,
    st: &'a ViewState,
    hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }
    fn state(&self, r: Rect, t: &Target) -> Kst {
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        Kst { hover, pressed, ..Kst::NORMAL }
    }
}

/// Pad `n`'s square: bank `n / 16`, pad 1 of a bank at the bottom left, 4 to a row.
pub fn pad_rect(n: usize) -> Rect {
    let i = n % 16;
    let (col, row) = ((i % 4) as f32, (3 - i / 4) as f32);
    Rect::new(GRID.x + GAP + col * (PAD + GAP), GRID.y + GAP + row * (PAD + GAP), PAD, PAD)
}

/// The value of a field of a pad and layer.
pub fn value(p: &Pad, layer: usize, f: Field) -> Option<i32> {
    let l = p.layers.get(layer);
    Some(match f {
        Field::Key => p.key,
        Field::Cut => p.cut,
        Field::CutBy => p.cut_by,
        Field::PadVolume => p.volume,
        Field::PadPan => p.pan,
        Field::PadTune => p.tune,
        Field::VelLo => l?.vel_lo,
        Field::VelHi => l?.vel_hi,
        Field::LayerVolume => l?.volume,
        Field::LayerPan => l?.pan,
        Field::LayerTune => l?.tune,
    })
}

/// A pad's colour: the stored Windows colour (0x00bbggrr), or the panel's face when none.
fn pad_color(p: &Pad) -> Option<Color> {
    (p.color != 0).then(|| Color::rgb(((p.color & 0xff) << 16) | (p.color & 0xff00) | ((p.color >> 16) & 0xff)))
}

/// The file name of a stored path.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Paints the editor. `hint` (not hosted: the host shows it otherwise) goes along the grid's bottom.
pub fn paint(c: &mut Canvas, t: &Palette, st: &ViewState, look: &Look, hint: &str) -> Vec<Hit> {
    let mut ui = Ui { c, t, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    header(&mut ui);
    grid(&mut ui, look, hint);
    let pads = look.state.pads();
    let pad = pads.get(st.pad);
    pad_card(&mut ui, pad, st.pad);
    layer_card(&mut ui, pad, st.layer);
    ui.hits
}

fn header(ui: &mut Ui) {
    // The emblem: the lemon mark and "DRUMS" on one line.
    let f = Font::condensed(18, 0).bold();
    let s: f32 = 6.0;
    let (wl, gap) = (2.2 * s, (s * 1.3).round());
    let x = LOGO.x + 4.0;
    let lh = ui.c.line_height(f);
    lemon::lemon_mark(ui.c, x + wl / 2.0, HEAD_Y, s, brand::MAIN);
    lemon::spaced(ui.c, f, x + wl + gap, (HEAD_Y - lh / 2.0).round(), plug::WORDMARK, "DRUMS", 3.0);
    ui.hit(LOGO, Target::Logo);
    for (b, r, label) in [(0, BANK_A, "BANK A"), (1, BANK_B, "BANK B")] {
        let t = Target::Bank(b);
        let mut kst = ui.state(r, &t);
        kst.on = ui.st.bank == b;
        let ink = lemon::plug_button(ui.c, r, 6.0, kst);
        ui.c.text_center(PILL, r, ink, label);
        ui.hit(r, t);
    }
}

/// The 16 pads of the bank shown: number and name, key, the pad's colour along its bottom; the chosen pad lit.
fn grid(ui: &mut Ui, look: &Look, hint: &str) {
    lemon::plug_well(ui.c, GRID, 8.0);
    let pads = look.state.pads();
    for i in 0..16 {
        let n = ui.st.bank * 16 + i;
        let Some(p) = pads.get(n) else { continue };
        let r = pad_rect(n);
        let t = Target::Pad(n);
        let mut kst = ui.state(r, &t);
        kst.on = ui.st.pad == n || look.lit.get(n).copied().unwrap_or(false);
        let ink = lemon::plug_button(ui.c, r, 6.0, kst);
        if let Some(col) = pad_color(p) {
            ui.c.fill_rect(Rect::new(r.x + 6.0, r.bottom() - 6.0, r.w - 12.0, 2.0), col);
        }
        let loaded = look.loaded.get(n).copied().unwrap_or(false);
        let has = p.sounding().next().is_some();
        ui.c.text(SMALL.bold(), r.x + 6.0, r.y + 5.0, ink, &(i + 1).to_string());
        let kn = key_name(p.key);
        let kw = ui.c.text_width(SMALL, &kn);
        ui.c.text(SMALL, r.right() - 6.0 - kw, r.y + 5.0, ink, &kn);
        let name = if !p.name.is_empty() {
            p.name.clone()
        } else if let Some((_, l)) = p.sounding().next() {
            file_name(&l.path).to_string()
        } else {
            "empty".into()
        };
        let col = if !has { plug::OFF } else if !loaded { Color::rgb(0xd06050) } else { ink };
        ui.c.text_clip(SMALL, r.x + 6.0, r.cy() - 2.0, r.w - 12.0, col, &name);
        ui.hit(r, t);
    }
    if !hint.is_empty() {
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        let s = if right.is_empty() { left.to_string() } else { format!("{left}  ({right})") };
        let lh = ui.c.line_height(SMALL);
        let y = GRID.bottom() - lh - 3.0;
        let tw = ui.c.text_width(SMALL, &s).min(GRID.w - 12.0);
        ui.c.fill_rrect(Rect::new(GRID.x + 2.0, y - 2.0, tw + 10.0, lh + 4.0), 4.0, Color::rgb(0x1a1a1a).with_alpha(200));
        ui.c.text_clip(SMALL, GRID.x + 7.0, y, GRID.w - 12.0, brand::TEXT, &s);
    }
}

/// A card with its title centred in a band and a line under the band.
fn titled(ui: &mut Ui, r: Rect, title: &str) {
    lemon::plug_panel(ui.c, r);
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + BAND, r.w - 2.0, 1.0), plug::PANEL_LINE);
    ui.c.text_center(SMALL.bold(), Rect::new(r.x, r.y, r.w, BAND), plug::DIM, title);
}

/// A value box (drag, wheel, type-in) with its caption under it.
fn value_box(ui: &mut Ui, cx: f32, y: f32, f: Field, v: Option<i32>, caption: &str) {
    let r = Rect::new(cx - 30.0, y - 10.0, 60.0, 20.0);
    let t = Target::Value(f);
    let mut kst = ui.state(r, &t);
    kst.disabled = v.is_none();
    lemon::field(ui.c, r, kst);
    if let Some(v) = v {
        let col = if kst.hover || kst.pressed { brand::TEXT } else { brand::MAIN };
        let s = if f == Field::Key { key_name(v) } else { f.text(v) };
        ui.c.text_center(SMALL.bold(), r, col, &s);
    }
    inst::caption(ui.c, ui.t, cx, y + 16.0, caption, v.is_none());
    ui.hit(r, t);
}

/// A knob (hit area 40 px across), caption under it.
fn knob(ui: &mut Ui, cx: f32, cy: f32, f: Field, v: Option<i32>, caption: &str) {
    let t = Target::Value(f);
    let hr = Rect::new(cx - KNOB_R - 3.0, cy - KNOB_R - 3.0, 2.0 * KNOB_R + 6.0, 2.0 * KNOB_R + 6.0);
    let mut kst = ui.state(hr, &t);
    kst.disabled = v.is_none();
    let (lo, hi) = f.range();
    let norm = v.map_or(0.0, |v| (v.clamp(lo, hi) - lo) as f32 / (hi - lo) as f32);
    lemon::dial(ui.c, cx, cy, KNOB_R, 2.8, norm, f.bipolar(), brand::MAIN, kst);
    inst::caption(ui.c, ui.t, cx, cy + KNOB_R + 8.0, caption, v.is_none());
    ui.hit(hr, t);
}

/// The chosen pad: its name, key and cut groups, and its volume, pan and tune.
fn pad_card(ui: &mut Ui, pad: Option<&Pad>, n: usize) {
    titled(ui, PAD_CARD, &format!("PAD {}{}", if n >= 16 { "B" } else { "A" }, n % 16 + 1));
    lemon::field(ui.c, NAME, Kst { disabled: pad.is_none(), ..Kst::NORMAL });
    let name = pad.map(|p| p.name.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| "(no name)".into());
    ui.c.text_center(LABEL, NAME, if pad.is_some_and(|p| !p.name.is_empty()) { brand::TEXT } else { plug::FAINT }, &name);
    let v = |f: Field| pad.and_then(|p| value(p, 0, f));
    value_box(ui, BOX_X[0], PAD_BOX_Y, Field::Key, v(Field::Key), "KEY");
    value_box(ui, BOX_X[1], PAD_BOX_Y, Field::Cut, v(Field::Cut), "CUT");
    value_box(ui, BOX_X[2], PAD_BOX_Y, Field::CutBy, v(Field::CutBy), "CUT BY");
    knob(ui, KNOB_X[0], PAD_KNOB_Y, Field::PadVolume, v(Field::PadVolume), "VOLUME");
    knob(ui, KNOB_X[1], PAD_KNOB_Y, Field::PadPan, v(Field::PadPan), "PAN");
    knob(ui, KNOB_X[2], PAD_KNOB_Y, Field::PadTune, v(Field::PadTune), "TUNE");
}

/// The chosen layer: which of the pad's layers, its sample and load button, its velocity range and its levels.
fn layer_card(ui: &mut Ui, pad: Option<&Pad>, layer: usize) {
    let n = pad.map_or(0, |p| p.layers.len());
    let layer = layer.min(n.saturating_sub(1));
    titled(ui, LAYER_CARD, &if n == 0 { "LAYER".to_string() } else { format!("LAYER {} OF {n}", layer + 1) });
    for (r, t, left) in [(PREV, Target::Prev, true), (NEXT, Target::Next, false)] {
        let enabled = if left { layer > 0 } else { layer + 1 < n };
        let kst = ui.state(r, &t);
        let col = if !enabled { plug::OFF } else if kst.hover { brand::TEXT } else { plug::DIM };
        let (x, y) = (r.cx(), r.cy());
        let d = if left { -1.0 } else { 1.0 };
        ui.c.fill_polygon(&[(x - 3.0 * d, y - 4.0), (x - 3.0 * d, y + 4.0), (x + 3.0 * d, y)], col);
        if enabled {
            ui.hit(r, t);
        }
    }
    let l = pad.and_then(|p| p.layers.get(layer));
    lemon::field(ui.c, SAMPLE, Kst { disabled: l.is_none(), ..Kst::NORMAL });
    let file = l.map(|l| file_name(&l.path).to_string()).filter(|s| !s.is_empty());
    ui.c.text_clip(LABEL, SAMPLE.x + 6.0, SAMPLE.cy() - 7.0, SAMPLE.w - 12.0, if file.is_some() { brand::TEXT } else { plug::FAINT }, file.as_deref().unwrap_or("No sample: LOAD opens one"));
    let kst = ui.state(LOAD, &Target::Load);
    let ink = lemon::plug_button(ui.c, LOAD, 6.0, kst);
    ui.c.text_center(PILL, LOAD, ink, "LOAD");
    ui.hit(LOAD, Target::Load);
    let v = |f: Field| pad.and_then(|p| value(p, layer, f));
    value_box(ui, BOX_X[0], LAYER_BOX_Y, Field::VelLo, v(Field::VelLo), "VEL LOW");
    value_box(ui, BOX_X[1], LAYER_BOX_Y, Field::VelHi, v(Field::VelHi), "VEL HIGH");
    knob(ui, KNOB_X[0], LAYER_KNOB_Y, Field::LayerVolume, v(Field::LayerVolume), "VOLUME");
    knob(ui, KNOB_X[1], LAYER_KNOB_Y, Field::LayerPan, v(Field::LayerPan), "PAN");
    knob(ui, KNOB_X[2], LAYER_KNOB_Y, Field::LayerTune, v(Field::LayerTune), "TUNE");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every card and pad sits on the 6 px grid, inside the editor, 6 px from its neighbours.
    #[test]
    fn the_layout_keeps_its_grid() {
        assert_eq!((W % 6, H % 6), (0, 0));
        for r in [LOGO, GRID, PAD_CARD, LAYER_CARD, BANK_A, BANK_B] {
            assert!(r.x >= GAP && r.right() <= RIGHT && r.y >= GAP && r.bottom() <= BOTTOM, "{r:?}");
            assert_eq!(r.x % 6.0, 0.0, "{r:?}");
        }
        assert_eq!(PAD_CARD.x - GRID.right(), GAP);
        assert_eq!(LAYER_CARD.y - PAD_CARD.bottom(), GAP);
        assert_eq!(BANK_B.x - BANK_A.right(), GAP);
        // The pads fill the well with 6 px around and between them; pad 1 at the bottom left, 16 at the top right.
        let (p1, p4, p16) = (pad_rect(0), pad_rect(3), pad_rect(15));
        assert_eq!((p1.x - GRID.x, GRID.bottom() - p1.bottom()), (GAP, GAP));
        assert_eq!((GRID.right() - p4.right(), p16.y - GRID.y), (GAP, GAP));
        assert_eq!(pad_rect(1).x - p1.right(), GAP);
        assert_eq!(pad_rect(16), p1);
        assert_eq!((key_name(60), key_name(36)), ("C5".into(), "C3".into()));
        assert_eq!((Field::Cut.text(-1), Field::Cut.text(0)), ("--".into(), "1".into()));
    }
}
