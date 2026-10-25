// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Tesla's editor: painting and hit regions (platform independent), in the Lemon "Pith" plugin look. Edison's five
//! bands, top to bottom: the toolbar (transport, recording, nine menu buttons, the tool grid, the wordmark and the input
//! mute), the info row, the wave area (both channels overlaid in one lane, markers and regions as flags along the top,
//! the loop along the bottom), the scroll bar with an overview, and the bottom bar. The window is resizable: the wave
//! area takes the room; the tool grid stays at the right.
//!
// spec: plugins/tesla/README.md "The window", plugins/edison/window.md

use os_theme::design::lemon as brand;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::State;
use os_ui::{tabler, Canvas, Color, Font, Rect};

use crate::doc::Doc;
use crate::peaks::Peaks;
use crate::state::{self, Rec};
use crate::tools::{Amp, Law};

/// The default editor size (Edison's) and the smallest it may be resized to, in logical pixels.
pub const W: u32 = 744;
pub const H: u32 = 487;
pub const MIN_W: u32 = 680;
pub const MIN_H: u32 = 360;

pub const TOOLBAR_H: f32 = 92.0;
pub const INFO_H: f32 = 28.0;
pub const SCROLL_H: f32 = 29.0;
pub const BOTTOM_H: f32 = 21.0;
/// The flag strip along the top of the wave area and the loop strip along its bottom.
pub const FLAG_H: f32 = 14.0;
pub const LOOP_H: f32 = 10.0;

const LEFT_WAVE: Color = Color::rgb(0xe4e4e4);
const RIGHT_WAVE: Color = Color::rgb(0xd4b43c);
const FLAG: Color = Color::rgb(0xf0a030);
const LOOP: Color = Color::rgb(0xe0505a);
const RECORD: Color = Color::rgb(0xe8503c);
const SEL: Color = Color::rgb(0xffe066);

const CAPTION: Font = Font::condensed(9, 0).bold();
const VALUE: Font = Font::new(11).bold();
const SMALL: Font = Font::new(10);
const LABEL: Font = Font::condensed(10, 0).bold();
const HINT: Font = Font::new(11);
const MARK: Font = Font::condensed(15, 0).bold();
const TITLE: Font = Font::condensed(13, 0).bold();

/// Where the bands are for an editor `w` × `h`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub w: f32,
    pub h: f32,
    pub info: Rect,
    pub wave: Rect,
    pub scroll: Rect,
    pub bottom: Rect,
    /// The tool grid's left edge.
    pub grid_x: f32,
}

impl Layout {
    pub fn new(w: u32, h: u32) -> Layout {
        let (w, h) = (w.max(MIN_W) as f32, h.max(MIN_H) as f32);
        let info = Rect::new(0.0, TOOLBAR_H, w, INFO_H);
        let wave_top = TOOLBAR_H + INFO_H + 1.0;
        let wave_bot = h - BOTTOM_H - SCROLL_H;
        Layout {
            w,
            h,
            info,
            wave: Rect::new(4.0, wave_top, w - 8.0, wave_bot - wave_top),
            scroll: Rect::new(4.0, wave_bot + 1.0, w - 8.0, SCROLL_H - 2.0),
            bottom: Rect::new(0.0, h - BOTTOM_H, w, BOTTOM_H),
            grid_x: w - 380.0,
        }
    }

    /// The lane the waveform fills (between the flag strip and the loop strip).
    pub fn lane(&self) -> Rect {
        Rect::new(self.wave.x, self.wave.y + FLAG_H, self.wave.w, self.wave.h - FLAG_H - LOOP_H)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tr {
    Slave,
    Loop,
    Play,
    Scrub,
    Stop,
    Record,
}

/// The nine menus (the bottom bar's Options menu holds them all).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuId {
    File,
    Format,
    Edit,
    Tools,
    Regions,
    View,
    Snap,
    Select,
    Zoom,
    Options,
}

pub const MENUS: [(MenuId, &str); 9] = [
    (MenuId::File, "File"),
    (MenuId::Format, "Format"),
    (MenuId::Edit, "Edit"),
    (MenuId::Tools, "Tools"),
    (MenuId::Regions, "Regions"),
    (MenuId::View, "View"),
    (MenuId::Snap, "Snap"),
    (MenuId::Select, "Select"),
    (MenuId::Zoom, "Zoom"),
];

/// The bottom bar's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bt {
    Undo,
    /// Edit modes: 0 waveform, 1 pan, 2 volume, 3 stereo separation, 4 all-purpose envelope (only the waveform in
    /// Tesla 1).
    Mode(u8),
    Follow,
    Spectrum,
    Threshold,
    Regions,
    Freeze,
    ClickFree,
    Snap,
    Slide,
}

/// The Amp panel's controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmpCtl {
    Pan,
    Law,
    Left,
    Volume,
    Right,
    Sep,
    Reset,
    Preview,
    Accept,
    Close,
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Transport(Tr),
    When,
    Max,
    Append,
    Menu(MenuId),
    /// The tool grid, 0..18 (row 1 then row 2).
    Tool(usize),
    Wordmark,
    Mute,
    Format,
    Wave,
    Flag(u32),
    LoopFlag,
    Scroll,
    Bottom(Bt),
    Grip,
    Amp(AmpCtl),
    /// The panel behind the Amp controls (eats clicks).
    AmpPanel,
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// One tool button: its glyph, hint (left action / right action) and whether each action exists in Tesla 1.
pub struct ToolDef {
    pub glyph: Glyph,
    pub hint: &'static str,
    pub left: bool,
    pub right: bool,
}

/// The tool grid. Edison's layout; the RUN button's place (scripting, not planned) holds Amp / Center; tools of later
/// versions are shown greyed.
pub const TOOLS: [ToolDef; 18] = [
    ToolDef { glyph: Glyph::Undo, hint: "Undo / undo history", left: true, right: true },
    ToolDef { glyph: Glyph::Normalize, hint: "Normalize / normalize regions", left: true, right: true },
    ToolDef { glyph: Glyph::FadeIn, hint: "Fade in / declick in", left: true, right: true },
    ToolDef { glyph: Glyph::Amp, hint: "Amp... / center", left: true, right: true },
    ToolDef { glyph: Glyph::Reverb, hint: "Reverb... (later version)", left: false, right: false },
    ToolDef { glyph: Glyph::Eq, hint: "Equalize... (later version)", left: false, right: false },
    ToolDef { glyph: Glyph::Flag, hint: "Add marker or region / remove marker or region", left: true, right: true },
    ToolDef { glyph: Glyph::Loop, hint: "Set loop / remove loop", left: true, right: true },
    ToolDef { glyph: Glyph::Drag, hint: "Drag / copy sample or selection", left: true, right: true },
    ToolDef { glyph: Glyph::Reverse, hint: "Reverse / swap channels", left: true, right: true },
    ToolDef { glyph: Glyph::Gate, hint: "Trim side noise / gate noise (later version)", left: false, right: false },
    ToolDef { glyph: Glyph::FadeOut, hint: "Fade out / declick out", left: true, right: true },
    ToolDef { glyph: Glyph::Stretch, hint: "Time stretch / pitch shift... (later version)", left: false, right: false },
    ToolDef { glyph: Glyph::Blur, hint: "Blur... (later version)", left: false, right: false },
    ToolDef { glyph: Glyph::Clean, hint: "Clean up (denoise)... (later version)", left: false, right: false },
    ToolDef { glyph: Glyph::Slice, hint: "Auto-slicing (later version)", left: false, right: false },
    ToolDef { glyph: Glyph::Save, hint: "Save as... / save as 24Bit .wav...", left: true, right: true },
    ToolDef { glyph: Glyph::Send, hint: "Send to playlist as audio clip / to selected channel", left: true, right: true },
];

/// Tesla's own glyphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Undo,
    Normalize,
    FadeIn,
    FadeOut,
    Amp,
    Reverb,
    Eq,
    Flag,
    Loop,
    Drag,
    Reverse,
    Gate,
    Stretch,
    Blur,
    Clean,
    Slice,
    Save,
    Send,
    Eye,
    Pages,
    Scrub,
    Snowflake,
    SCurve,
    Arrow,
    Bars,
    Threshold,
    Slide,
    Wave,
    Pan,
    Ramp,
    BowTie,
    Chain,
    Down,
}

/// The pointer, the pressed control and the open Amp panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    pub pressed: Option<Target>,
    pub amp: Option<AmpUi>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AmpUi {
    pub amp: Amp,
    pub previewing: bool,
}

// ---------------------------------------------------------------------------------------------------------------
// What is shown: the document's sample, and a recording in progress after it

/// One stretch of audio in the wave area: its channels, their peaks, the frame it starts at.
pub struct Part<'a> {
    pub ch: Vec<&'a [f32]>,
    pub peaks: &'a Peaks,
    pub at: usize,
}

pub struct Shown<'a> {
    pub parts: Vec<Part<'a>>,
    pub rate: u32,
    pub channels: usize,
}

impl Shown<'_> {
    pub fn frames(&self) -> usize {
        self.parts.last().map_or(0, |p| p.at + p.ch.first().map_or(0, |c| c.len()))
    }

    /// Frame `i` of channel `c` (0 outside).
    pub fn at(&self, c: usize, i: usize) -> f32 {
        for p in &self.parts {
            let n = p.ch.first().map_or(0, |c| c.len());
            if i >= p.at && i < p.at + n {
                return p.ch[c.min(p.ch.len() - 1)][i - p.at];
            }
        }
        0.0
    }

    /// (min, max) of channel `c` over frames `a..b`.
    pub fn minmax(&self, c: usize, a: usize, b: usize) -> Option<(f32, f32)> {
        let mut r: Option<(f32, f32)> = None;
        for p in &self.parts {
            let n = p.ch.first().map_or(0, |c| c.len());
            let (pa, pb) = (a.max(p.at), b.min(p.at + n));
            if pa < pb {
                let src = p.ch[c.min(p.ch.len() - 1)];
                let (lo, hi) = p.peaks.minmax(src, c.min(p.ch.len() - 1), pa - p.at, pb - p.at);
                r = Some(r.map_or((lo, hi), |(l, h)| (l.min(lo), h.max(hi))));
            }
        }
        r
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Time formats

/// `frames` as View > Time format shows it. `per_px` (frames per pixel) picks the Auto format.
pub fn time_text(fmt: u8, frames: f64, rate: u32, tempo: f32, per_px: f64) -> String {
    let rate = rate.max(1) as f64;
    match fmt {
        state::TIME_SAMPLES => format!("{}", frames.round() as i64),
        state::TIME_BBT => {
            let bpm = if tempo > 0.0 { tempo as f64 } else { 120.0 };
            let beats = frames / rate * bpm / 60.0;
            let ticks = (beats * 96.0).floor() as i64;
            format!("{}:{}:{:02}", ticks / 384 + 1, (ticks / 96) % 4 + 1, ticks % 96)
        }
        state::TIME_AUTO if per_px < 0.05 * rate / 1000.0 && per_px > 0.0 => format!("{}", frames.round() as i64),
        _ => {
            let ms = (frames / rate * 1000.0).round() as i64;
            let (m, s, r) = (ms / 60000, (ms / 1000) % 60, ms % 1000);
            if m > 0 { format!("{m}:{s:02}:{r:03}") } else { format!("{s}:{r:03}") }
        }
    }
}

/// The caption of the length for a time format.
pub fn time_caption(fmt: u8) -> &'static str {
    match fmt {
        state::TIME_SAMPLES => "LENGTH / SAMPLES",
        state::TIME_BBT => "LENGTH / BAR:BEAT:TICK",
        _ => "LENGTH / MIN:SEC:MS",
    }
}

/// A level in dB (as the hover readout shows it).
pub fn db_text(v: f32) -> String {
    if v == 0.0 { "-inf dB".into() } else { format!("{:.2}dB", 20.0 * v.abs().log10()) }
}

// ---------------------------------------------------------------------------------------------------------------
// Painting

/// What a paint needs besides the canvas.
pub struct Look<'a> {
    pub doc: &'a Doc,
    pub shown: &'a Shown<'a>,
    pub st: &'a ViewState,
    pub hint: &'a str,
    pub playing: bool,
    pub pos: f64,
    pub armed: bool,
    pub capturing: bool,
    pub level: f32,
    pub rec: &'a Rec,
    pub looping: bool,
    pub can_undo: bool,
    /// The spectrogram of the lane (w, h, pixels) when the spectrum view is on.
    pub spectrum: Option<(usize, usize, &'a [u32])>,
}

struct Ui<'a> {
    c: &'a mut Canvas,
    st: &'a ViewState,
    hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }

    fn state(&self, r: Rect, t: Target, on: bool) -> State {
        let hover = self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed: self.st.pressed == Some(t) && hover, on, disabled: false }
    }

    /// A raised Pith button; returns the glyph colour.
    fn button(&mut self, r: Rect, t: Target, on: bool, enabled: bool) -> Color {
        let st = self.state(r, t, on).with_disabled(!enabled);
        self.hit(r, t);
        lemon::plug_button(self.c, r, 5.0, st)
    }

    /// A flat icon button of the bottom bar; returns the glyph colour.
    fn flat(&mut self, r: Rect, t: Target, on: bool, enabled: bool) -> Color {
        let st = self.state(r, t, on).with_disabled(!enabled);
        self.hit(r, t);
        lemon::choice_button(self.c, r, on, brand::MAIN, st)
    }
}

/// The view range: the first frame shown and frames per pixel of the lane.
pub fn span(doc: &Doc, frames: usize, lane_w: f32) -> (f64, f64) {
    let total = frames.max(1) as f64;
    let len = if doc.view.len > 0.0 { doc.view.len.min(total) } else { total };
    (doc.view.start.clamp(0.0, (total - len).max(0.0)), len / lane_w.max(1.0) as f64)
}

/// The x of frame `f` in the lane.
pub fn frame_x(lane: Rect, start: f64, per_px: f64, f: f64) -> f32 {
    lane.x + ((f - start) / per_px) as f32
}

pub fn paint(c: &mut Canvas, l: &Look, w: u32, h: u32) -> Vec<Hit> {
    let lay = Layout::new(w, h);
    c.clear(plug::BG);
    let mut ui = Ui { c, st: l.st, hits: Vec::new() };
    toolbar(&mut ui, l, &lay);
    info_row(&mut ui, l, &lay);
    wave(&mut ui, l, &lay);
    scroll(&mut ui, l, &lay);
    bottom(&mut ui, l, &lay);
    if let Some(a) = &l.st.amp {
        amp_panel(&mut ui, a, &lay);
    }
    ui.hits
}

fn toolbar(ui: &mut Ui, l: &Look, lay: &Layout) {
    let c = &mut *ui.c;
    c.fill_rect(Rect::new(0.0, 0.0, lay.w, TOOLBAR_H), plug::BG);
    // Transport.
    let y = 8.0;
    let tr = [(Tr::Slave, l.rec.slave), (Tr::Loop, l.looping), (Tr::Play, l.playing), (Tr::Scrub, false), (Tr::Stop, false)];
    for (i, (t, on)) in tr.into_iter().enumerate() {
        let r = Rect::new(8.0 + i as f32 * 28.0, y, 26.0, 26.0);
        let col = ui.button(r, Target::Transport(t), on, true);
        let g = r.inset(5.0);
        match t {
            Tr::Slave => ui.c.icon_stroke(&tabler::LINK, g, col, 2.0),
            Tr::Loop => ui.c.icon_stroke(&tabler::REPEAT, g, col, 2.0),
            Tr::Play => {
                if l.playing {
                    ui.c.fill_rect(Rect::new(g.x + 3.0, g.y + 2.0, 4.0, g.h - 4.0), col);
                    ui.c.fill_rect(Rect::new(g.right() - 7.0, g.y + 2.0, 4.0, g.h - 4.0), col);
                } else {
                    ui.c.icon(&tabler::PLAYER_PLAY, g, col);
                }
            }
            Tr::Scrub => glyph(ui.c, Glyph::Scrub, g, col),
            Tr::Stop => ui.c.icon(&tabler::PLAYER_STOP, g.inset(1.0), col),
            Tr::Record => {}
        }
    }
    // Record: a large disc, lit while armed.
    let rr = Rect::new(150.0, 5.0, 32.0, 32.0);
    let st = ui.state(rr, Target::Transport(Tr::Record), l.armed);
    ui.hit(rr, Target::Transport(Tr::Record));
    let ring = if st.hover { Color::rgb(0x5a5a5a) } else { plug::FACE };
    ui.c.fill_circle(rr.cx(), rr.cy(), 15.0, plug::FACE_LINE);
    ui.c.fill_circle(rr.cx(), rr.cy(), 14.0, ring);
    let disc = if l.capturing { RECORD } else if l.armed { Color::rgb(0xf08a30) } else { Color::rgb(0x8a3a32) };
    ui.c.fill_circle(rr.cx(), rr.cy(), 8.0, disc);
    // Append, under the record button.
    let ar = Rect::new(150.0, 44.0, 64.0, 14.0);
    let st = ui.state(ar, Target::Append, l.rec.append);
    ui.hit(ar, Target::Append);
    lemon::toggle(ui.c, ar.x + 6.0, ar.cy(), l.rec.append, st);
    ui.c.text(CAPTION, ar.x + 16.0, ar.y + 1.0, if st.hover { brand::TEXT } else { plug::DIM }, "APPEND");
    // When and For.
    let wr = Rect::new(192.0, 9.0, 86.0, 22.0);
    let st = ui.state(wr, Target::When, false);
    ui.hit(wr, Target::When);
    lemon::field(ui.c, wr, st);
    let when = state::WHEN_NAMES[l.rec.when.min(3) as usize].to_uppercase();
    ui.c.text_clip(LABEL, wr.x + 7.0, wr.y + 5.0, wr.w - 18.0, brand::TEXT, &when);
    caret(ui.c, wr, plug::DIM);
    ui.c.text(CAPTION, 284.0, 15.0, plug::DIM, "FOR");
    let mr = Rect::new(304.0, 9.0, 52.0, 22.0);
    let st = ui.state(mr, Target::Max, false);
    ui.hit(mr, Target::Max);
    lemon::field(ui.c, mr, st);
    ui.c.text_clip(LABEL, mr.x + 7.0, mr.y + 5.0, mr.w - 18.0, brand::TEXT, state::MAX_SHORT[l.rec.max.min(11) as usize]);
    caret(ui.c, mr, plug::DIM);
    // The input level, under the selectors.
    let lv = Rect::new(222.0, 40.0, 134.0, 5.0);
    lemon::plug_well(ui.c, lv, 2.0);
    let db = if l.level > 0.0 { 20.0 * l.level.log10() } else { -100.0 };
    let f = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    if f > 0.0 {
        let col = if l.level >= 1.0 { RECORD } else { brand::MAIN };
        ui.c.fill_rrect(Rect::new(lv.x + 1.0, lv.y + 1.0, (lv.w - 2.0) * f, lv.h - 2.0), 1.5, col);
    }
    // −60 dB, where On input and Input start.
    ui.c.fill_rect(Rect::new(lv.x + 1.0, lv.y - 2.0, 1.0, lv.h + 4.0), plug::TICK);

    // The nine menu buttons, in three groups.
    let mut x = 8.0;
    for (i, (id, _)) in MENUS.iter().enumerate() {
        if i == 2 || i == 5 {
            ui.c.fill_rect(Rect::new(x + 2.0, 64.0, 1.0, 18.0), plug::TICK);
            x += 8.0;
        }
        let r = Rect::new(x, 60.0, 28.0, 26.0);
        let col = ui.button(r, Target::Menu(*id), false, true);
        let g = r.inset(6.0);
        match id {
            MenuId::File => ui.c.icon_stroke(&tabler::DEVICE_FLOPPY, g, col, 1.8),
            MenuId::Format => glyph(ui.c, Glyph::Pages, g, col),
            MenuId::Edit => ui.c.icon_stroke(&tabler::CUT, g, col, 1.8),
            MenuId::Tools => ui.c.icon_stroke(&tabler::TOOL, g, col, 1.8),
            MenuId::Regions => glyph(ui.c, Glyph::Flag, g, col),
            MenuId::View => glyph(ui.c, Glyph::Eye, g, col),
            MenuId::Snap => ui.c.icon_stroke(&tabler::MAGNET, g, col, 1.8),
            MenuId::Select => ui.c.icon_stroke(&tabler::MARQUEE_2, g, col, 1.8),
            MenuId::Zoom => ui.c.icon_stroke(&tabler::ZOOM_IN, g, col, 1.8),
            MenuId::Options => {}
        }
        x += 30.0;
    }

    // The tool grid.
    for (i, t) in TOOLS.iter().enumerate() {
        let r = Rect::new(lay.grid_x + (i % 9) as f32 * 40.0, 25.0 + (i / 9) as f32 * 32.0, 38.0, 30.0);
        let col = ui.button(r, Target::Tool(i), false, t.left || t.right);
        glyph(ui.c, t.glyph, Rect::new(r.cx() - 9.0, r.cy() - 9.0, 18.0, 18.0), col);
    }

    // The wordmark and the input mute at the top right.
    let mr = Rect::new(lay.w - 26.0, 3.0, 20.0, 19.0);
    let col = ui.flat(mr, Target::Mute, l.rec.mute, true);
    ui.c.icon_stroke(if l.rec.mute { &tabler::VOLUME_OFF } else { &tabler::VOLUME }, mr.inset(2.5), col, 1.8);
    let name_w = lemon::spaced_width(ui.c, MARK, "TESLA", 3.0);
    let nx = lay.w - 34.0 - name_w;
    let wr = Rect::new(nx - 16.0, 3.0, name_w + 18.0, 19.0);
    ui.hit(wr, Target::Wordmark);
    lemon::lemon_mark(ui.c, nx - 9.0, 12.5, 4.5, brand::MAIN);
    lemon::spaced(ui.c, MARK, nx, 3.0, plug::WORDMARK, "TESLA", 3.0);
}

/// The small arrow at a selector field's right end.
fn caret(c: &mut Canvas, r: Rect, col: Color) {
    let (x, y) = (r.right() - 11.0, r.cy());
    c.fill_polygon(&[(x, y - 4.0), (x + 4.0, y), (x, y + 4.0)], col);
}

fn info_row(ui: &mut Ui, l: &Look, lay: &Layout) {
    let r = lay.info;
    lemon::outlined(ui.c, Rect::new(4.0, r.y, lay.w - 8.0, r.h - 1.0), 6.0, plug::PANEL_LINE, plug::PANEL);
    let s = &l.doc.sample;
    let rate = l.shown.rate;
    let field = |ui: &mut Ui, x: f32, cap: &str, val: &str, col: Color| {
        ui.c.text(CAPTION, x, r.y + 2.0, plug::FAINT, cap);
        ui.c.text(VALUE, x, r.y + 12.0, col, val);
    };
    field(ui, 12.0, "SAMPLERATE", &format!("{rate}Hz"), brand::TEXT);
    // Format: bit depth and a channel glyph; clicking converts.
    let fr = Rect::new(82.0, r.y + 1.0, 52.0, r.h - 3.0);
    let st = ui.state(fr, Target::Format, false);
    ui.hit(fr, Target::Format);
    if st.hover {
        ui.c.fill_rrect(fr, 4.0, Color::rgb(0x3d3d3d));
    }
    field(ui, 86.0, "FORMAT", &format!("{}", s.format.bits()), brand::TEXT);
    let gx = 106.0;
    let gy = r.y + 15.0;
    ui.c.stroke_circle(gx + 5.0, gy + 4.0, 4.0, 1.4, plug::DIM);
    if l.shown.channels == 2 {
        ui.c.stroke_circle(gx + 12.0, gy + 4.0, 4.0, 1.4, plug::DIM);
    }
    let tempo = if s.tempo > 0.0 { format!("{:.1}", s.tempo) } else { "-".into() };
    field(ui, 140.0, "TEMPO", &tempo, brand::TEXT);
    // The hint well.
    let hx = 190.0;
    let title_x = lay.w - 270.0;
    let hr = Rect::new(hx, r.y + 4.0, title_x - hx - 10.0, r.h - 9.0);
    lemon::plug_well(ui.c, hr, 5.0);
    ui.c.text_clip(HINT, hr.x + 7.0, hr.y + 3.0, hr.w - 14.0, plug::DIM, l.hint);
    // Title ("RECORDING" in red while armed) and length.
    let (title, col) = if l.armed { ("RECORDING".to_string(), RECORD) } else { (l.doc.title(), brand::TEXT) };
    ui.c.text(CAPTION, title_x, r.y + 2.0, plug::FAINT, "TITLE");
    ui.c.text_clip(TITLE, title_x, r.y + 11.0, 150.0, col, &title);
    let lw = lay.w - 12.0;
    let fmt = l.doc.view.time;
    ui.c.text_right(CAPTION, lw, r.y + 2.0, plug::FAINT, time_caption(fmt));
    let len = time_text(if fmt == state::TIME_AUTO { state::TIME_MSM } else { fmt }, l.shown.frames() as f64, rate, s.tempo, 1.0);
    ui.c.text_right(VALUE, lw, r.y + 12.0, brand::TEXT, &len);
}

/// A good grid step (frames) for about `px` pixels between lines.
pub fn grid_step(fmt: u8, rate: u32, tempo: f32, per_px: f64, px: f64) -> f64 {
    let want = per_px * px;
    let rate = rate.max(1) as f64;
    let units: Vec<f64> = if fmt == state::TIME_BBT {
        let beat = rate * 60.0 / if tempo > 0.0 { tempo as f64 } else { 120.0 };
        [1.0 / 16.0, 1.0 / 8.0, 0.25, 0.5, 1.0, 4.0, 16.0, 64.0, 256.0].iter().map(|b| b * beat).collect()
    } else if fmt == state::TIME_SAMPLES {
        (0..12).flat_map(|e| [1.0, 2.0, 5.0].map(|m| m * 10f64.powi(e))).collect()
    } else {
        [0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0].iter().map(|s| s * rate).collect()
    };
    units.iter().copied().find(|&u| u >= want).unwrap_or(*units.last().unwrap())
}

fn wave(ui: &mut Ui, l: &Look, lay: &Layout) {
    let wr = lay.wave;
    lemon::plug_well(ui.c, wr, 6.0);
    ui.hit(wr, Target::Wave);
    let lane = lay.lane();
    let n = l.shown.frames();
    let doc = l.doc;
    if n == 0 && !l.armed {
        ui.c.text_center(SMALL, Rect::new(wr.x, wr.bottom() - 40.0, wr.w, 20.0), plug::FAINT, "Click to load a sample");
        return;
    }
    let (start, per_px) = span(doc, n, lane.w);
    let fx = |f: f64| frame_x(lane, start, per_px, f);
    ui.c.push_clip(wr.inset(1.0));
    if let Some((sw, sh, px)) = l.spectrum {
        ui.c.image(lane, sw, sh, px);
    }
    // Selection.
    if let Some((a, b)) = doc.sel {
        let (x0, x1) = (fx(a as f64).max(lane.x), fx(b as f64 + 1.0).min(lane.right()));
        if x1 > x0 {
            ui.c.fill_rect(Rect::new(x0, wr.y + 1.0, (x1 - x0).max(1.0), wr.h - 2.0), SEL.with_alpha(34));
            ui.c.fill_rect(Rect::new(x0, wr.y + 1.0, 1.0, wr.h - 2.0), SEL.with_alpha(120));
            ui.c.fill_rect(Rect::new(x1 - 1.0, wr.y + 1.0, 1.0, wr.h - 2.0), SEL.with_alpha(120));
        }
    }
    // Grid: quarter heights, and time lines.
    let mid = lane.cy();
    for k in 1..4 {
        let y = (lane.y + lane.h * k as f32 / 4.0).round();
        ui.c.fill_rect(Rect::new(lane.x, y, lane.w, 1.0), if k == 2 { plug::TICK.with_alpha(160) } else { plug::GRID });
    }
    let fmt = doc.view.time;
    let step = grid_step(fmt, l.shown.rate, doc.sample.tempo, per_px, 90.0);
    let mut g = (start / step).floor() * step;
    let end = start + per_px * lane.w as f64;
    while g <= end {
        let x = fx(g).round();
        if x >= lane.x {
            ui.c.fill_rect(Rect::new(x, lane.y, 1.0, lane.h), plug::GRID);
            let t = time_text(fmt, g, l.shown.rate, doc.sample.tempo, per_px);
            ui.c.text(SMALL, x + 3.0, lane.bottom() - 13.0, plug::OFF, &t);
        }
        g += step;
    }
    // The waveform (the spectrogram is drawn under the selection and the grid).
    if l.spectrum.is_none() {
        let amp = lane.h / 2.0;
        let y_of = |v: f32| mid - v.clamp(-1.0, 1.0) * amp;
        let chans = l.shown.channels;
        let colors = [LEFT_WAVE, RIGHT_WAVE];
        // The right channel first, the left over it.
        for c in (0..chans).rev() {
            let col = if chans == 1 { LEFT_WAVE } else { colors[c] };
            if per_px >= 1.0 {
                let mut x = lane.x;
                while x < lane.right() {
                    let a = start + (x - lane.x) as f64 * per_px;
                    let b = a + per_px;
                    if b > 0.0 && (a as usize) < n {
                        if let Some((lo, hi)) = l.shown.minmax(c, a.max(0.0) as usize, (b.ceil() as usize).max(a as usize + 1)) {
                            let (y0, y1) = (y_of(hi), y_of(lo));
                            ui.c.fill_rect(Rect::new(x, y0, 1.0, (y1 - y0).max(1.0)), col.with_alpha(if c == 1 { 200 } else { 230 }));
                        }
                    }
                    x += 1.0;
                }
            } else {
                // Zoomed in past one frame per pixel: the frames joined, each a dot.
                let i0 = start.floor().max(0.0) as usize;
                let i1 = (end.ceil() as usize + 1).min(n);
                let pts: Vec<(f32, f32)> = (i0..i1).map(|i| (fx(i as f64), y_of(l.shown.at(c, i)))).collect();
                ui.c.polyline(&pts, 1.2, col);
                if per_px < 0.2 {
                    for &(x, y) in &pts {
                        ui.c.fill_circle(x, y, 1.8, col);
                    }
                }
            }
        }
    }
    // Markers and regions along the top, the loop along the bottom.
    if doc.view.regions {
        for q in doc.sample.cues_by_pos() {
            let x = fx(q.pos as f64).round();
            if let Some(len) = q.len {
                let x1 = fx((q.pos + len) as f64).round();
                if x1 >= lane.x && x <= lane.right() {
                    ui.c.fill_rect(Rect::new(x, wr.y + 1.0, (x1 - x).max(1.0), FLAG_H - 2.0), FLAG.with_alpha(40));
                    ui.c.fill_rect(Rect::new(x1 - 1.0, wr.y + 1.0, 1.0, wr.h - 2.0), Color::rgb(0x8a8a8a).with_alpha(110));
                    ui.c.fill_rect(Rect::new(x1 - 5.0, wr.y + 1.0, 5.0, 5.0), Color::rgb(0x9a9a9a));
                }
            }
            if x < lane.x - 120.0 || x > lane.right() {
                continue;
            }
            ui.c.fill_rect(Rect::new(x, wr.y + 1.0, 1.0, wr.h - 2.0), FLAG.with_alpha(170));
            let tw = ui.c.text_width(SMALL, &q.name).max(6.0);
            let fr = Rect::new(x, wr.y + 1.0, tw + 8.0, FLAG_H - 1.0);
            ui.c.fill_rect(fr, FLAG);
            ui.c.text(SMALL, x + 4.0, wr.y + 1.0, Color::rgb(0x2a1a00), &q.name);
            ui.hit(fr, Target::Flag(q.id));
        }
    }
    if doc.view.looped {
        if let Some((a, b)) = doc.sample.looped {
            let (x0, x1) = (fx(a as f64).round(), fx(b as f64 + 1.0).round());
            let y = wr.bottom() - LOOP_H;
            if x1 >= lane.x && x0 <= lane.right() {
                ui.c.fill_rect(Rect::new(x0.max(lane.x), y + 3.0, (x1.min(lane.right()) - x0.max(lane.x)).max(1.0), 4.0), LOOP.with_alpha(200));
                let fr = Rect::new(x0, y - 2.0, 30.0, LOOP_H + 1.0);
                ui.c.fill_rect(Rect::new(x0, lane.y, 1.0, wr.bottom() - lane.y - 1.0), LOOP.with_alpha(120));
                ui.c.fill_rect(Rect::new(x1 - 1.0, lane.y, 1.0, wr.bottom() - lane.y - 1.0), LOOP.with_alpha(120));
                ui.c.fill_rect(fr, LOOP);
                ui.c.text(SMALL, x0 + 4.0, y - 2.0, Color::rgb(0xffffff), "Loop");
                ui.hit(fr, Target::LoopFlag);
            }
        }
    }
    // The cursor and the playhead.
    if doc.sel.is_none() {
        let x = fx(doc.cursor as f64).round();
        ui.c.fill_rect(Rect::new(x, lane.y, 1.0, lane.h), SEL.with_alpha(110));
    }
    if l.playing {
        let x = fx(l.pos).round();
        ui.c.fill_rect(Rect::new(x, wr.y + 1.0, 1.0, wr.h - 2.0), Color::rgb(0xffffff));
    }
    ui.c.pop_clip();
}

fn scroll(ui: &mut Ui, l: &Look, lay: &Layout) {
    let r = lay.scroll;
    lemon::plug_well(ui.c, r, 5.0);
    ui.hit(r, Target::Scroll);
    let n = l.shown.frames();
    if n == 0 {
        return;
    }
    let inner = r.inset(2.0);
    let per = n as f64 / inner.w as f64;
    let mid = inner.cy();
    let amp = inner.h / 2.0 - 1.0;
    for c in (0..l.shown.channels).rev() {
        let col = if c == 1 { RIGHT_WAVE.with_alpha(120) } else { LEFT_WAVE.with_alpha(130) };
        let mut x = inner.x;
        while x < inner.right() {
            let a = ((x - inner.x) as f64 * per) as usize;
            let b = (((x - inner.x + 1.0) as f64 * per) as usize).max(a + 1);
            if let Some((lo, hi)) = l.shown.minmax(c, a, b) {
                let (y0, y1) = (mid - hi.clamp(-1.0, 1.0) * amp, mid - lo.clamp(-1.0, 1.0) * amp);
                ui.c.fill_rect(Rect::new(x, y0, 1.0, (y1 - y0).max(1.0)), col);
            }
            x += 1.0;
        }
    }
    // The visible part.
    let lane = lay.lane();
    let (start, per_px) = span(l.doc, n, lane.w);
    let x0 = inner.x + (start / per) as f32;
    let x1 = inner.x + ((start + per_px * lane.w as f64) / per) as f32;
    let tr = Rect::new(x0.max(inner.x), r.y + 1.0, (x1.min(inner.right()) - x0.max(inner.x)).max(6.0), r.h - 2.0);
    let st = ui.state(r, Target::Scroll, false);
    ui.c.fill_rrect(tr, 4.0, brand::MAIN.with_alpha(if st.hover { 46 } else { 30 }));
    ui.c.stroke_rrect(tr.inset(0.5), 4.0, 1.0, brand::MAIN.with_alpha(150));
    if l.playing {
        let px = inner.x + (l.pos / per) as f32;
        ui.c.fill_rect(Rect::new(px.round(), r.y + 2.0, 1.0, r.h - 4.0), Color::rgb(0xffffff));
    }
}

fn bottom(ui: &mut Ui, l: &Look, lay: &Layout) {
    let r = lay.bottom;
    let y = r.y + 1.0;
    let mut x = 6.0;
    let btn = |ui: &mut Ui, x: f32, t: Target, g: Glyph, on: bool, enabled: bool| {
        let b = Rect::new(x, y, 22.0, 18.0);
        let col = ui.flat(b, t, on, enabled);
        glyph(ui.c, g, Rect::new(b.cx() - 7.0, b.cy() - 7.0, 14.0, 14.0), col);
    };
    btn(ui, x, Target::Menu(MenuId::Options), Glyph::Down, false, true);
    x += 24.0;
    btn(ui, x, Target::Bottom(Bt::Undo), Glyph::Undo, false, l.can_undo);
    x += 30.0;
    for (i, g) in [Glyph::Wave, Glyph::Pan, Glyph::Ramp, Glyph::BowTie, Glyph::Chain].into_iter().enumerate() {
        // Envelope modes come in a later version.
        btn(ui, x, Target::Bottom(Bt::Mode(i as u8)), g, i == 0, i == 0);
        x += 24.0;
    }
    let v = &l.doc.view;
    let right = [
        (Bt::Follow, Glyph::Arrow, v.follow, true),
        (Bt::Spectrum, Glyph::Bars, v.spectrum, true),
        (Bt::Threshold, Glyph::Threshold, false, false),
        (Bt::Regions, Glyph::Flag, v.regions, true),
        (Bt::Freeze, Glyph::Snowflake, v.freeze, true),
        (Bt::ClickFree, Glyph::SCurve, v.clickfree, true),
        (Bt::Snap, Glyph::Snowflake, v.snap_on, true),
        (Bt::Slide, Glyph::Slide, v.slide, true),
    ];
    let mut x = lay.w - 18.0 - right.len() as f32 * 24.0;
    for (b, g, on, en) in right {
        let g = if b == Bt::Snap { Glyph::Eq } else { g };
        let rr = Rect::new(x, y, 22.0, 18.0);
        let col = ui.flat(rr, Target::Bottom(b), on, en);
        let gr = Rect::new(rr.cx() - 7.0, rr.cy() - 7.0, 14.0, 14.0);
        if b == Bt::Snap {
            ui.c.icon_stroke(&tabler::MAGNET, gr, col, 1.8);
        } else {
            glyph(ui.c, g, gr, col);
        }
        x += 24.0;
    }
    // The resize grip in the corner.
    let gr = Rect::new(lay.w - 14.0, lay.h - 14.0, 14.0, 14.0);
    ui.hit(gr, Target::Grip);
    for k in 0..3 {
        let o = 3.0 + k as f32 * 4.0;
        ui.c.line(lay.w - 2.0 - o, lay.h - 2.0, lay.w - 2.0, lay.h - 2.0 - o, 1.0, plug::TICK);
    }
}

/// The Amp panel's geometry, centred on the wave area.
pub fn amp_rect(lay: &Layout) -> Rect {
    let (w, h) = (372.0, 196.0);
    Rect::new((lay.wave.cx() - w / 2.0).round(), (lay.wave.cy() - h / 2.0).round().max(lay.wave.y), w, h)
}

/// The Amp panel's knobs' centres and the faders' slots.
pub fn amp_parts(lay: &Layout) -> (Rect, [(AmpCtl, Rect); 6]) {
    let p = amp_rect(lay);
    let knob = |cx: f32| Rect::new(p.x + cx - 22.0, p.y + 48.0, 44.0, 44.0);
    let fader = |cx: f32| Rect::new(p.x + cx - 9.0, p.y + 36.0, 18.0, 108.0);
    (p, [
        (AmpCtl::Pan, knob(52.0)),
        (AmpCtl::Law, Rect::new(p.x + 14.0, p.y + 112.0, 76.0, 20.0)),
        (AmpCtl::Left, fader(140.0)),
        (AmpCtl::Volume, fader(186.0)),
        (AmpCtl::Right, fader(232.0)),
        (AmpCtl::Sep, knob(316.0)),
    ])
}

/// A fader's travel inside its slot (the top = +1).
pub fn fader_travel(slot: Rect) -> (f32, f32) {
    (slot.y + 8.0, slot.bottom() - 8.0)
}

fn amp_panel(ui: &mut Ui, a: &AmpUi, lay: &Layout) {
    ui.c.fill_rect(lay.wave, plug::BG.with_alpha(140));
    let (p, parts) = amp_parts(lay);
    ui.hit(p, Target::AmpPanel);
    lemon::outlined(ui.c, p.outset(1.0), 11.0, Color::rgb(0x111111), plug::PANEL_LINE);
    lemon::plug_panel(ui.c, p);
    ui.c.text(TITLE, p.x + 14.0, p.y + 8.0, brand::TEXT, "Amp");
    for (ctl, r) in parts {
        let t = Target::Amp(ctl);
        ui.hit(r, t);
        let st = ui.state(r, t, false);
        match ctl {
            AmpCtl::Pan | AmpCtl::Sep => {
                let v = if ctl == AmpCtl::Pan { a.amp.pan } else { a.amp.sep };
                lemon::dial(ui.c, r.cx(), r.cy(), 20.0, 4.0, (v + 1.0) / 2.0, true, brand::MAIN, st);
                let cap = if ctl == AmpCtl::Pan { "PANNING" } else { "STEREO SEP" };
                ui.c.text_center(CAPTION, Rect::new(r.x - 20.0, r.y - 16.0, r.w + 40.0, 12.0), plug::DIM, cap);
            }
            AmpCtl::Law => {
                lemon::field(ui.c, r, st);
                let s = if a.amp.law == Law::Circular { "CIRCULAR" } else { "TRIANGULAR" };
                ui.c.text_center(LABEL, r, brand::TEXT, s);
            }
            AmpCtl::Left | AmpCtl::Volume | AmpCtl::Right => {
                let v = match ctl {
                    AmpCtl::Left => a.amp.left,
                    AmpCtl::Volume => a.amp.volume,
                    _ => a.amp.right,
                };
                lemon::plug_well(ui.c, Rect::new(r.cx() - 2.0, r.y + 4.0, 4.0, r.h - 8.0), 2.0);
                let (top, bot) = fader_travel(r);
                ui.c.fill_rect(Rect::new(r.x + 2.0, (top + bot) / 2.0, r.w - 4.0, 1.0), plug::TICK);
                let y = bot - (v + 1.0) / 2.0 * (bot - top);
                let col = match ctl {
                    AmpCtl::Left => Color::rgb(0xf0a030),
                    AmpCtl::Right => Color::rgb(0xe0505a),
                    _ => Color::rgb(0xbdbdbd),
                };
                ui.c.fill_rrect(Rect::new(r.x, y - 5.0, r.w, 10.0), 3.0, if st.hover { col.shade(0.2) } else { col });
                let cap = match ctl {
                    AmpCtl::Left => "L",
                    AmpCtl::Right => "R",
                    _ => "VOL",
                };
                ui.c.text_center(CAPTION, Rect::new(r.x - 10.0, r.bottom() + 2.0, r.w + 20.0, 12.0), plug::DIM, cap);
            }
            _ => {}
        }
    }
    let by = p.bottom() - 32.0;
    let buttons = [(AmpCtl::Reset, "RESET", 150.0), (AmpCtl::Preview, "PREVIEW", 220.0), (AmpCtl::Accept, "ACCEPT", 292.0)];
    for (ctl, label, x) in buttons {
        let r = Rect::new(p.x + x, by, 66.0, 22.0);
        let col = ui.button(r, Target::Amp(ctl), ctl == AmpCtl::Preview && a.previewing, true);
        ui.c.text_center(LABEL, r, col, label);
    }
    let cr = Rect::new(p.right() - 26.0, p.y + 6.0, 20.0, 20.0);
    let col = ui.flat(cr, Target::Amp(AmpCtl::Close), false, true);
    ui.c.icon_stroke(&tabler::X, cr.inset(4.0), col, 2.0);
}

/// Draws a glyph into `r`.
pub fn glyph(c: &mut Canvas, g: Glyph, r: Rect, col: Color) {
    let (x, y, w, h) = (r.x, r.y, r.w, r.h);
    let p = |fx: f32, fy: f32| (x + fx * w, y + fy * h);
    let lw = (w / 10.0).clamp(1.3, 2.0);
    match g {
        Glyph::Undo => c.icon_stroke(&tabler::ARROW_BACK_UP, r, col, 2.0),
        Glyph::Normalize => {
            for (i, hh) in [0.35f32, 0.7, 0.5, 0.9, 0.6].iter().enumerate() {
                let bx = x + (0.1 + i as f32 * 0.17) * w;
                c.fill_rect(Rect::new(bx, y + h * (1.0 - hh * 0.85), w * 0.11, h * hh * 0.85), col);
            }
            c.fill_rect(Rect::new(x, y + h * 0.05, w, 1.5), col);
        }
        Glyph::FadeIn => {
            let pts: Vec<(f32, f32)> = (0..=12).map(|i| {
                let t = i as f32 / 12.0;
                p(t, 1.0 - (1.0 - (1.0 - t).powi(2)) * 0.9)
            }).collect();
            c.polyline(&pts, lw, col);
            c.line(x, y + h, x + w, y + h, 1.0, col.with_alpha(120));
        }
        Glyph::FadeOut => {
            let pts: Vec<(f32, f32)> = (0..=12).map(|i| {
                let t = i as f32 / 12.0;
                p(t, 1.0 - (1.0 - t).powi(2) * 0.9)
            }).collect();
            c.polyline(&pts, lw, col);
            c.line(x, y + h, x + w, y + h, 1.0, col.with_alpha(120));
        }
        Glyph::Amp => {
            c.stroke_circle(r.cx(), r.cy(), w * 0.42, lw, col);
            c.line(r.cx(), r.cy(), r.cx() + w * 0.25, r.cy() - h * 0.25, lw, col);
        }
        Glyph::Reverb => {
            for k in 0..4 {
                let xx = x + w * (0.12 + k as f32 * 0.24);
                let hh = h * (0.9 - k as f32 * 0.2);
                c.fill_rect(Rect::new(xx, y + h - hh, w * 0.12, hh), col);
            }
        }
        Glyph::Eq => c.icon_stroke(&tabler::ADJUSTMENTS_HORIZONTAL, r, col, 1.8),
        Glyph::Flag => {
            c.line(x + w * 0.2, y + h * 0.05, x + w * 0.2, y + h, lw, col);
            c.fill_polygon(&[p(0.2, 0.05), p(0.9, 0.25), p(0.2, 0.5)], col);
        }
        Glyph::Loop => c.icon_stroke(&tabler::REPEAT, r, col, 1.8),
        Glyph::Drag => c.icon_stroke(&tabler::HAND_FINGER, r, col, 1.8),
        Glyph::Reverse => {
            c.line(x + w * 0.1, r.cy(), x + w * 0.9, r.cy(), lw, col);
            c.polyline(&[p(0.4, 0.25), p(0.1, 0.5), p(0.4, 0.75)], lw, col);
        }
        Glyph::Gate => {
            c.polyline(&[p(0.0, 0.8), p(0.3, 0.8), p(0.3, 0.2), p(0.7, 0.2), p(0.7, 0.8), p(1.0, 0.8)], lw, col);
        }
        Glyph::Stretch => c.icon_stroke(&tabler::ARROWS_HORIZONTAL, r, col, 1.8),
        Glyph::Blur => c.icon_stroke(&tabler::BLEND_MODE, r, col, 1.8),
        Glyph::Clean => c.icon_stroke(&tabler::BRUSH, r, col, 1.8),
        Glyph::Slice => c.icon_stroke(&tabler::SLICE, r, col, 1.8),
        Glyph::Save => c.icon_stroke(&tabler::DEVICE_FLOPPY, r, col, 1.8),
        Glyph::Send => c.icon_stroke(&tabler::PLAYLIST, r, col, 1.8),
        Glyph::Eye => {
            let top: Vec<(f32, f32)> = (0..=10).map(|i| {
                let t = i as f32 / 10.0;
                p(t, 0.5 - (std::f32::consts::PI * t).sin() * 0.32)
            }).collect();
            let bot: Vec<(f32, f32)> = (0..=10).map(|i| {
                let t = i as f32 / 10.0;
                p(t, 0.5 + (std::f32::consts::PI * t).sin() * 0.32)
            }).collect();
            c.polyline(&top, lw, col);
            c.polyline(&bot, lw, col);
            c.fill_circle(r.cx(), r.cy(), w * 0.16, col);
        }
        Glyph::Pages => {
            c.stroke_rrect(Rect::new(x + w * 0.28, y, w * 0.62, h * 0.8), 2.0, lw, col);
            c.stroke_rrect(Rect::new(x + w * 0.1, y + h * 0.2, w * 0.62, h * 0.8), 2.0, lw, col);
        }
        Glyph::Scrub => {
            c.stroke_circle(r.cx(), r.cy(), w * 0.4, lw, col);
            c.line(r.cx(), y, r.cx(), y + h, lw, col);
            c.line(x, r.cy(), x + w, r.cy(), lw, col);
        }
        Glyph::Snowflake => {
            for k in 0..3 {
                let a = k as f32 * std::f32::consts::PI / 3.0;
                let (dx, dy) = (a.cos() * w * 0.45, a.sin() * h * 0.45);
                c.line(r.cx() - dx, r.cy() - dy, r.cx() + dx, r.cy() + dy, lw, col);
            }
        }
        Glyph::SCurve => {
            let pts: Vec<(f32, f32)> = (0..=12).map(|i| {
                let t = i as f32 / 12.0;
                p(t, 0.9 - (0.5 - 0.5 * (std::f32::consts::PI * t).cos()) * 0.8)
            }).collect();
            c.polyline(&pts, lw, col);
        }
        Glyph::Arrow => {
            c.line(x + w * 0.1, r.cy(), x + w * 0.85, r.cy(), lw, col);
            c.polyline(&[p(0.55, 0.25), p(0.88, 0.5), p(0.55, 0.75)], lw, col);
        }
        Glyph::Bars => {
            for (i, hh) in [0.4f32, 0.9, 0.6, 0.75].iter().enumerate() {
                c.fill_rect(Rect::new(x + w * (0.08 + i as f32 * 0.24), y + h * (1.0 - hh), w * 0.14, h * hh), col);
            }
        }
        Glyph::Threshold => {
            c.line(x, y + h * 0.35, x + w, y + h * 0.35, 1.0, col);
            c.line(x, y + h * 0.65, x + w, y + h * 0.65, 1.0, col);
            c.polyline(&[p(0.0, 0.5), p(0.25, 0.2), p(0.5, 0.8), p(0.75, 0.3), p(1.0, 0.5)], lw, col);
        }
        Glyph::Slide => {
            c.line(x, r.cy(), x + w, r.cy(), lw, col);
            c.polyline(&[p(0.25, 0.25), p(0.0, 0.5), p(0.25, 0.75)], lw, col);
            c.polyline(&[p(0.75, 0.25), p(1.0, 0.5), p(0.75, 0.75)], lw, col);
        }
        Glyph::Wave => {
            let pts: Vec<(f32, f32)> = (0..=16).map(|i| {
                let t = i as f32 / 16.0;
                p(t, 0.5 - (t * 4.0 * std::f32::consts::PI).sin() * 0.35 * (1.0 - t * 0.5))
            }).collect();
            c.polyline(&pts, lw, col);
        }
        Glyph::Pan => {
            c.line(x, r.cy(), x + w, r.cy(), 1.0, col);
            c.fill_circle(x + w * 0.65, r.cy(), w * 0.2, col);
        }
        Glyph::Ramp => c.polyline(&[p(0.0, 0.9), p(1.0, 0.1)], lw, col),
        Glyph::BowTie => c.fill_polygon(&[p(0.0, 0.15), p(0.5, 0.5), p(0.0, 0.85), p(1.0, 0.15), p(0.5, 0.5), p(1.0, 0.85)], col),
        Glyph::Chain => c.icon_stroke(&tabler::LINK, r, col, 1.8),
        Glyph::Down => c.fill_polygon(&[p(0.15, 0.3), p(0.85, 0.3), p(0.5, 0.75)], col),
    }
}

/// The hint for a control (Edison's texts in Lemon's words).
pub fn hint(t: &Target, l: &HintCtx) -> String {
    match t {
        Target::Transport(Tr::Slave) => "Slave playback to host".into(),
        Target::Transport(Tr::Loop) => "Loop".into(),
        Target::Transport(Tr::Play) => "Play / pause \u{00b7} Space".into(),
        Target::Transport(Tr::Scrub) => "Scrub (drag)".into(),
        Target::Transport(Tr::Stop) => "Stop \u{00b7} Space".into(),
        Target::Transport(Tr::Record) => "Record".into(),
        Target::When => format!("When / what to record: {}", state::WHEN_NAMES[l.when.min(3) as usize]),
        Target::Max => format!("Maximum recording time: {}", state::MAX_NAMES[l.max.min(11) as usize]),
        Target::Append => "Append recording to existing sample".into(),
        Target::Menu(MenuId::Options) => "Options".into(),
        Target::Menu(m) => MENUS.iter().find(|(id, _)| id == m).map_or("", |(_, n)| n).to_string(),
        Target::Tool(i) => TOOLS[*i].hint.to_string(),
        Target::Wordmark => "Tesla: Lemon Studio's audio editor and recorder".into(),
        Target::Mute => "Mute input".into(),
        Target::Format => "Channels / press to convert".into(),
        Target::Wave => String::new(),
        Target::Flag(_) => "Marker / region: drag to move, right-click for options".into(),
        Target::LoopFlag => "Loop: right-click for options".into(),
        Target::Scroll => "Drag to scroll \u{00b7} wheel to zoom".into(),
        Target::Bottom(Bt::Undo) => "Undo / undo history".into(),
        Target::Bottom(Bt::Mode(0)) => "Edit waveform".into(),
        Target::Bottom(Bt::Mode(1)) => "Edit panning envelope (later version)".into(),
        Target::Bottom(Bt::Mode(2)) => "Edit volume envelope (later version)".into(),
        Target::Bottom(Bt::Mode(3)) => "Edit stereo separation envelope (later version)".into(),
        Target::Bottom(Bt::Mode(_)) => "Edit all-purpose envelope (later version)".into(),
        Target::Bottom(Bt::Follow) => "Scroll to follow playback".into(),
        Target::Bottom(Bt::Spectrum) => "Display waveform / spectrum".into(),
        Target::Bottom(Bt::Threshold) => "View noise threshold mark (later version)".into(),
        Target::Bottom(Bt::Regions) => "View regions / markers / loop".into(),
        Target::Bottom(Bt::Freeze) => "Freeze editing (markers and regions stay put)".into(),
        Target::Bottom(Bt::ClickFree) => "Click-free (smooth) editing".into(),
        Target::Bottom(Bt::Snap) => "Snap".into(),
        Target::Bottom(Bt::Slide) => "Slide succeeding markers and regions".into(),
        Target::Grip => "Drag to resize".into(),
        Target::Amp(AmpCtl::Pan) => "Panning".into(),
        Target::Amp(AmpCtl::Law) => "Panning law".into(),
        Target::Amp(AmpCtl::Left) => "Left channel volume".into(),
        Target::Amp(AmpCtl::Volume) => "Volume".into(),
        Target::Amp(AmpCtl::Right) => "Right channel volume".into(),
        Target::Amp(AmpCtl::Sep) => "Stereo separation".into(),
        Target::Amp(AmpCtl::Reset) => "Reset".into(),
        Target::Amp(AmpCtl::Preview) => "Preview".into(),
        Target::Amp(AmpCtl::Accept) => "Accept".into(),
        Target::Amp(AmpCtl::Close) => "Close (cancel)".into(),
        Target::AmpPanel => String::new(),
    }
}

/// What the hints read besides the target.
pub struct HintCtx {
    pub when: u8,
    pub max: u8,
}
