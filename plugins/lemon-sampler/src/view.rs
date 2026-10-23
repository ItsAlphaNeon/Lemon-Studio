// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Sampler editor: painting and hit regions (platform independent), built from the `os_ui::kit` widgets.
//!
//! Coordinates are FL window pixels from the spec tables; the editor is the window minus its 24 px title bar, so
//! every y is drawn at `y - OY`. It is drawn in the Lemon design only (`kit::lemon::plug`; the plugins have no Classic
//! look): outlined panels, square toggles, a grey waveform in a dark well.
// spec: plugins/lemon-sampler.md (Look, Layout, Hints, Waveform, Keyboard); ui/lemon-design.md §11.1

use std::f32::consts::{PI, TAU};

use os_dsp::tone::Tones;
use os_engine::Sample;
use os_theme::design::lemon as brand;
use os_theme::waves::Waves;
use os_theme::Palette;
use os_ui::kit::lemon::{self, plug};
use os_ui::kit::{self, inst, State};
use os_ui::tabler as tb;
use os_ui::{Canvas, Color, Font, Rect};

use crate::params::{self, note_name, KEY_MAX};

/// Painted size in logical pixels (FL's window minus its title bar).
pub const W: u32 = 568;
pub const H: u32 = 442;
/// The editor the host shows: the painted area without FL's 4 px frame (the host draws its own frame around it,
/// spec: ui/plugin-window.md), i.e. `W x H` cropped at (`CROP_X`, `CROP_Y`).
pub const CROP_X: u32 = 4;
pub const CROP_Y: u32 = 1;
pub const EDITOR_W: u32 = 560;
pub const EDITOR_H: u32 = 437;
/// Hosted mode (the host's strip row replaces the plugin's own, 47 px): the editor starts below that row.
pub const HOSTED_CROP_Y: u32 = CROP_Y + STRIP_ROWS;
pub const HOSTED_EDITOR_H: u32 = EDITOR_H - STRIP_ROWS;
/// Height of the tab strip row the host draws instead (FL y 25..71).
pub const STRIP_ROWS: u32 = 47;
/// Height of FL's title bar, which the host draws instead.
pub const OY: f32 = 24.0;

/// Sample-page accent (knob rings, page icons).
pub const ORANGE: u32 = 0xffa830;

/// Per envelope target (Panning, Volume, Mod X, Mod Y, Pitch): knob ring, sub-tab text, graph line.
pub const TARGET_COLORS: [(u32, u32, u32); 5] = [
    (0x49e3f6, 0x29b5f9, 0x3ec1d8),
    (0x47f49a, 0x40f08c, 0x3bc97e),
    (0xcfe744, 0xd6fc47, 0xb1c538),
    (0xf29a48, 0xf0792b, 0xce833c),
    (0xf64d70, 0xf13f37, 0xe2415f),
];
pub const TARGET_NAMES: [&str; 5] = ["Panning", "Volume", "Mod X", "Mod Y", "Pitch"];
pub const ENV_PREFIX: [&str; 5] = ["envpan", "envvol", "envmodx", "envmody", "envpitch"];

const LABEL: Font = inst::LABEL;
const CAP: Font = inst::CAPTION;
/// The filter's MOD X / MOD Y captions (larger than knob captions).
const MOD_CAP: Font = Font::new(10).bold();

/// An accent colour of the view: the sample page's orange and the root key's blues are drawn in lemon.
fn accent(t: &Palette, hex: u32) -> Color {
    match hex {
        ORANGE | 0x77a9e6 => brand::MAIN,
        0x235fc3 => brand::DEEP,
        0x5785d2 => brand::DEEP.mix(brand::MAIN, 0.5),
        0x225cc3 => brand::COLORED_DARK,
        _ => t.a(hex),
    }
}

pub fn y(v: f32) -> f32 {
    v - OY
}

/// Inclusive FL-window pixel ranges to an editor rect.
pub fn rr(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::span(x0, x1, y(y0), y(y1))
}

/// Read access to parameter values by id.
pub trait Model {
    /// Normalised value 0..=1.
    fn norm(&self, id: &str) -> f32;
    /// Plain value (bools 0 / 1, ints as numbers).
    fn plain(&self, id: &str) -> f32;
    /// Value text.
    fn text(&self, id: &str) -> String;
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Vertical drag / wheel; menu on right click.
    Knob(String),
    /// Bool toggle on click.
    Toggle(String),
    /// Sets an integer parameter to a plain value on click.
    Set(String, f32),
    /// Choice list: opens the list menu.
    List(String),
    /// Horizontal slider over its track rect (editor coordinates).
    Slider(String, Rect),
    Tab(u8),
    SubTab(u8),
    /// Precomputed-effects page: 0 tools, 1 effects.
    FxPage(u8),
    FileName,
    FileOpen,
    FileBrowse,
    FileClear,
    /// Keyboard key (normal or multi-touch keyboard).
    Key(i32),
    /// The ruler above the keys: drag selects the key zone.
    Ruler,
    /// Keyboard scroll bar (track rect).
    KeyScroll(Rect),
    KeyArrow(i8),
    RootReset,
    Hand,
    /// Waveform (area rect).
    Wave(Rect),
    /// Waveform overview strip (track rect).
    Overview(Rect),
    WaveArrow(i8),
    /// Bit depth / channel readout under the waveform.
    WaveInfo,
    /// Loop marker on the waveform: 0 start, 1 end.
    LoopHandle(u8),
    EnvMenu,
    /// Envelope graph point: 0 attack end, 1 hold end, 2 sustain, 3 release end.
    EnvPoint(u8),
    ReverbAb,
}

impl Target {
    /// The parameter a target edits (middle click resets it).
    pub fn param(&self) -> Option<&str> {
        match self {
            Target::Knob(s) | Target::Toggle(s) | Target::Set(s, _) | Target::List(s) | Target::Slider(s, _) => Some(s),
            Target::ReverbAb => Some("fx_reverb_type"),
            Target::Key(_) => Some("root"),
            Target::LoopHandle(0) => Some("loop_start"),
            Target::LoopHandle(_) => Some("loop_end"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// The name of a control that does nothing yet, `None` for the ones that work: the Content toggles (resample, load
/// regions, load slice markers: nothing reads them) and the envelope presets menu (no presets yet).
/// Greyed-out controls draw their disabled look and the editor only names them.
pub fn off_name(t: &Target) -> Option<&'static str> {
    match t {
        Target::Toggle(id) if matches!(id.as_str(), "resample" | "load_regions" | "load_slices") => {
            Some(hint_text(id).unwrap_or("This option"))
        }
        Target::EnvMenu => Some("Channel envelope presets"),
        _ => None,
    }
}

/// Whether target `t` does anything today (`off_name`).
pub fn works(t: &Target) -> bool {
    off_name(t).is_none()
}

/// Editor-only state (not saved).
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    /// 0 sample, 1 envelope / instrument.
    pub tab: u8,
    /// Envelope target: 0 Panning, 1 Volume, 2 Mod X, 3 Mod Y, 4 Pitch.
    pub sub: u8,
    /// Precomputed-effects page: 0 tools, 1 effects.
    pub fx_page: u8,
    /// Keyboard scroll in px of the normal keyboard (C0 at the keyboard's left edge = 0).
    pub key_scroll: f32,
    /// Multi-touch preview keyboard (big keys).
    pub hand: bool,
    /// Waveform view: first visible frame and visible frames as fractions of the sample (0, 1 = all).
    pub wave_start: f64,
    pub wave_len: f64,
    pub multichannel: bool,
    pub spectrum: bool,
    /// Keys lit orange (held in the editor or sounding).
    pub lit: Vec<i32>,
    /// Ruler drag in progress: the zone being selected.
    pub zone_drag: Option<(i32, i32)>,
    /// Mouse position (hover looks), `None` outside.
    pub mouse: Option<(f32, f32)>,
    /// Control being dragged / pressed.
    pub pressed: Option<Target>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            tab: 0,
            sub: 1,
            fx_page: 0,
            key_scroll: DEFAULT_KEY_SCROLL,
            hand: false,
            wave_start: 0.0,
            wave_len: 1.0,
            multichannel: false,
            spectrum: false,
            lit: Vec::new(),
            zone_drag: None,
            mouse: None,
            pressed: None,
        }
    }
}

/// Frames of a sample the waveform panel draws: `left[i]`, `right[i]` are frame `first + i * step`.
#[derive(Clone, Copy)]
pub struct Frames<'a> {
    pub left: &'a [f32],
    pub right: &'a [f32],
    pub first: usize,
    pub step: usize,
}

impl<'a> Frames<'a> {
    pub fn all(s: &'a Sample) -> Frames<'a> {
        Frames { left: &s.left, right: &s.right, first: 0, step: 1 }
    }

    pub const NONE: Frames<'static> = Frames { left: &[], right: &[], first: 0, step: 1 };

    /// Frame `j` (or the one before it that is kept), both sides.
    fn at(&self, j: usize) -> Option<(f32, f32)> {
        let i = j.checked_sub(self.first)? / self.step.max(1);
        Some((*self.left.get(i)?, *self.right.get(i)?))
    }
}

/// The shown sample: its length and rate, and what the panel draws of it. A sample in memory draws its frames; one
/// that plays from disk (Keep on disk) draws a summary (every few frames) and, zoomed in, the frames themselves.
#[derive(Clone, Copy)]
pub struct Wave<'a> {
    pub frames: usize,
    pub rate: u32,
    /// Whether the sides differ (`None`: compare them).
    pub stereo: Option<bool>,
    pub on_disk: bool,
    /// Frames of the visible range, and of the whole sample (the overview strip).
    pub near: Frames<'a>,
    pub whole: Frames<'a>,
}

impl<'a> Wave<'a> {
    /// A sample in memory.
    pub fn of(s: &'a Sample) -> Wave<'a> {
        Wave { frames: s.frames(), rate: s.rate, stereo: None, on_disk: false, near: Frames::all(s), whole: Frames::all(s) }
    }

    fn stereo(&self) -> bool {
        self.stereo.unwrap_or_else(|| self.whole.left != self.whole.right)
    }
}

/// What the waveform panel shows.
pub struct SampleView<'a> {
    pub sample: Option<Wave<'a>>,
    pub path: &'a str,
    /// Bits of the source file (32 once processed).
    pub bits: u16,
    /// Spectrum picture of the visible range (spectrum view), `w`, `h`, pixels.
    pub spectrum: Option<(usize, usize, &'a [u32])>,
    /// Colourful waveforms (the host's General ▸ Display setting): the sample's tones and how to colour them; `None`
    /// draws the plain grey (spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)").
    pub tones: Option<(&'a Tones, &'a Waves)>,
}

impl SampleView<'_> {
    pub fn file(&self) -> &str {
        self.path.rsplit(['/', '\\']).next().unwrap_or("")
    }
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
    fn on(&self, id: &str) -> bool {
        self.m.plain(id) > 0.5
    }
    /// Hover / pressed look of a control occupying `r`; greyed-out controls (`works`) are disabled and never light up.
    fn state(&self, r: Rect, t: &Target) -> State {
        if !works(t) {
            return State { disabled: true, ..State::NORMAL };
        }
        let pressed = self.st.pressed.as_ref() == Some(t);
        let hover = !pressed && self.st.pressed.is_none() && self.st.mouse.is_some_and(|(x, y)| r.contains(x, y));
        State { hover, pressed, ..State::NORMAL }
    }
}

/// Paints the editor. `hosted` = the host's strip row carries the page tabs and the hint line (host link, spec
/// `plugins/host-link.md`): the plugin leaves that row out. Otherwise it draws its own tabs and `hint` (parameter name and
/// value under the mouse) in the strip's free space.
pub fn paint(c: &mut Canvas, t: &Palette, m: &dyn Model, st: &ViewState, sv: &SampleView, hint: &str, hosted: bool) -> Vec<Hit> {
    let mut ui = Ui { c, t, m, st, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(plug::BG);
    if !hosted {
        tab_strip(&mut ui);
        if !hint.is_empty() {
            let lh = ui.c.line_height(LABEL);
            let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
            ui.c.text_clip(LABEL, 100.0, (y(48.0) - lh / 2.0).round(), 300.0, plug::DIM, left);
            if !right.is_empty() {
                ui.c.text_right(LABEL, 556.0, (y(48.0) - lh / 2.0).round(), plug::DIM, right);
            }
        }
    }
    match st.tab {
        0 => sample_tab(&mut ui, sv),
        _ => envelope_tab(&mut ui),
    }
    ui.hits
}

// ---------------------------------------------------------------------------------------------------------------
// Widgets (thin wrappers over the kit: hit regions + parameter values)

fn panel(ui: &mut Ui, x0: f32, x1: f32, y0: f32, y1: f32, head_w: f32, title: &str) -> Rect {
    let r = rr(x0, x1, y0, y1);
    inst::panel(ui.c, ui.t, r);
    if head_w > 0.0 {
        inst::panel_tab(ui.c, ui.t, r, head_w, title);
    }
    r
}

#[derive(Clone, Copy)]
struct KnobLook {
    r: f32,
    ring: f32,
}

/// Sample-page knobs.
const BIG: KnobLook = KnobLook { r: 13.0, ring: 16.0 };
/// Envelope / LFO knobs.
const MID: KnobLook = KnobLook { r: 11.5, ring: 13.5 };
/// Tension knobs (no dot).
const TINY: KnobLook = KnobLook { r: 8.0, ring: 10.0 };

/// A knob with its value ring and caption (caption text top at `cy + 17.5`).
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, look: KnobLook, accent: u32, bipolar: bool, caption: &str) {
    let cy = y(cy);
    let target = Target::Knob(id.into());
    let hr = Rect::new(cx - look.ring - 2.0, cy - look.ring - 2.0, 2.0 * look.ring + 4.0, 2.0 * look.ring + 4.0);
    let st = ui.state(hr, &target);
    // The loop END knob shows what it cuts off the end: no ring at the sample's end (FL capture t1_default).
    let v = if id == "loop_end" { 1.0 - ui.m.norm(id) } else { ui.m.norm(id) };
    inst::dial(ui.c, ui.t, cx, cy, look.r, look.ring, v, bipolar, self::accent(ui.t, accent), st);
    if !caption.is_empty() {
        // The envelope AMT caption sits lower than the others (measured).
        let dy = if id.starts_with("env") && id.ends_with("_amt") && !id.ends_with("lfo_amt") { 21.5 } else { 17.5 };
        inst::caption(ui.c, ui.t, cx, cy + dy, caption, st.disabled);
    }
    ui.hit(hr, target);
}

/// A round radio / toggle with an optional label to its right (greyed out when it does nothing yet, `works`).
fn radio(ui: &mut Ui, id: &str, cx: f32, cy: f32, label: &str) {
    let on = ui.on(id);
    let cy = y(cy);
    let lw = if label.is_empty() { 0.0 } else { ui.c.text_width(LABEL, label) + 4.0 };
    let r = Rect::new(cx - 8.0, cy - 8.0, 16.0 + lw, 16.0);
    let target = Target::Toggle(id.into());
    let st = ui.state(r, &target);
    inst::radio(ui.c, ui.t, cx, cy, on, st);
    if !label.is_empty() {
        inst::label(ui.c, ui.t, cx + 10.0, cy, label, st.disabled);
    }
    ui.hit(r, target);
}

fn list(ui: &mut Ui, id: &str, x0: f32, x1: f32, y0: f32, y1: f32) {
    let r = rr(x0, x1, y0, y1);
    let target = Target::List(id.into());
    let st = ui.state(r, &target);
    let s = ui.m.text(id);
    inst::list_box(ui.c, ui.t, r, &s, st);
    ui.hit(r, target);
}

fn label(ui: &mut Ui, x: f32, cy: f32, s: &str) -> f32 {
    inst::label(ui.c, ui.t, x, y(cy), s, false)
}

/// A Tabler icon centred on (`cx`, `cy`), `size` px square.
fn icon(c: &mut Canvas, t: &tb::TablerIcon, cx: f32, cy: f32, size: f32, col: Color) {
    c.icon(t, Rect::new(cx - size / 2.0, cy - size / 2.0, size, size), col);
}

/// The page icons the host draws in its strip (same ids as `os_hostlink::PageIcon`).
pub fn page_icon(c: &mut Canvas, page: u8, cx: f32, cy: f32, col: Color) {
    match page {
        0 => icon(c, &tb::WAVE_SINE, cx, cy, 26.0, col),
        _ => icon(c, &tb::TIMELINE, cx, cy, 26.0, col),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tab strip (only when no host draws it)

fn tab_strip(ui: &mut Ui) {
    let hover = (0..2u8).find(|&i| ui.state(rr(5.0 + 43.0 * f32::from(i), 49.0 + 43.0 * f32::from(i), 26.0, 71.0), &Target::Tab(i)).hover);
    lemon::page_strip(ui.c, rr(4.0, 563.0, 25.0, 71.0), 2, ui.st.tab, hover, &|i| 5.0 + 43.0 * f32::from(i), &mut |c, i, x, yy, col| page_icon(c, i, x, yy, col));
    for i in 0..2u8 {
        let x0 = 5.0 + 43.0 * f32::from(i);
        ui.hit(rr(x0, x0 + 44.0, 26.0, 71.0), Target::Tab(i));
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tab 1: sample

fn sample_tab(ui: &mut Ui, sv: &SampleView) {
    let t = ui.t;
    // File panel.
    panel(ui, 11.0, 280.0, 78.0, 114.0, 0.0, "");
    label(ui, 21.0, 96.0, "File");
    let file = sv.file();
    let (stem, ext) = match file.rfind('.') {
        Some(i) if !file.is_empty() => (&file[..i], &file[i..]),
        _ => (file, ""),
    };
    let lh = ui.c.line_height(LABEL);
    let ty = (y(96.0) - lh / 2.0).round();
    let w = ui.c.text_width(LABEL, stem).min(150.0);
    let (name_col, ext_col) = (brand::TEXT, Color::rgb(0x8c8c8c));
    ui.c.text_clip(LABEL, 46.0, ty, 150.0, name_col, stem);
    ui.c.text_clip(LABEL, 46.0 + w, ty, 150.0 - w, ext_col, ext);
    ui.hit(rr(44.0, 200.0, 85.0, 107.0), Target::FileName);
    // One rounded group with thin separators.
    lemon::field(ui.c, rr(204.0, 274.0, 84.0, 108.0), State::NORMAL);
    for (x0, x1, target, ic, size) in [
        (205.0, 234.0, Target::FileOpen, &tb::FOLDER_OPEN, 17.0),
        (236.0, 255.0, Target::FileBrowse, &tb::FOLDER_SEARCH, 16.0),
        (256.0, 273.0, Target::FileClear, &tb::X, 12.0),
    ] {
        let r = rr(x0, x1, 85.0, 107.0);
        let st = ui.state(r, &target);
        if st.hover || st.pressed {
            ui.c.fill_rrect(r.inset(1.0), 4.0, if st.pressed { Color::rgb(0x1f1f1f) } else { Color::rgb(0x353535) });
        }
        if x0 > 205.0 {
            ui.c.fill_rect(rr(x0 - 1.0, x0 - 1.0, 88.0, 104.0), plug::FIELD_LINE);
        }
        icon(ui.c, ic, r.cx(), r.cy(), size, if st.hover { brand::MAIN } else { brand::TEXT });
        ui.hit(r, target);
    }

    panel(ui, 11.0, 280.0, 121.0, 232.0, 69.0, "Content");
    radio(ui, "keep_disk", 25.0, 158.0, "Keep on disk");
    radio(ui, "resample", 25.0, 178.0, "Resample");
    radio(ui, "load_regions", 153.0, 158.0, "Load regions");
    radio(ui, "load_slices", 153.0, 178.0, "Load slice markers");
    label(ui, 21.0, 209.0, "Declicking mode");
    list(ui, "declick", 124.0, 270.0, 199.0, 219.0);

    panel(ui, 11.0, 280.0, 239.0, 319.0, 69.0, "Playback");
    knob(ui, "start_offset", 45.0, 286.0, BIG, ORANGE, false, "START OFFSET");
    knob(ui, "loop_start", 98.0, 286.0, BIG, ORANGE, false, "START");
    knob(ui, "loop_end", 140.0, 286.0, BIG, ORANGE, false, "END");
    icon(ui.c, &tb::ROTATE_CLOCKWISE_2, 119.0, y(286.0) + 21.0, 11.0, brand::MAIN);
    radio(ui, "loop_use", 184.0, 274.0, "Use loop points");
    radio(ui, "loop_pingpong", 184.0, 294.0, "Ping pong loop");

    panel(ui, 287.0, 556.0, 78.0, 155.0, 105.0, "Time stretching");
    knob(ui, "ts_pitch", 312.0, 121.0, BIG, ORANGE, true, "PITCH");
    knob(ui, "ts_mul", 356.0, 121.0, BIG, ORANGE, true, "MUL");
    knob(ui, "ts_time", 392.0, 121.0, BIG, ORANGE, false, "TIME");
    label(ui, 415.0, 112.0, "Mode");
    list(ui, "ts_mode", 456.0, 548.0, 102.0, 122.0);
    if ui.m.plain("ts_mode") as i32 == params::STRETCH_AUTO {
        // Auto picks a method; FL shows the one in use under the box.
        let f = Font::new(11);
        ui.c.text(f, 460.0, y(125.0), plug::FAINT, "e3 generic");
    }

    let fx = panel(ui, 287.0, 556.0, 162.0, 319.0, 0.0, "");
    inst::panel_tab(ui.c, t, fx, 172.0, "");
    for (i, x, ic) in [(0u8, 298.0, &tb::TOOL), (1, 322.0, &tb::FLASK_2)] {
        let r = Rect::new(x - 11.0, y(163.0), 22.0, 19.0);
        let target = Target::FxPage(i);
        let st = ui.state(r, &target);
        // The chosen page is a lit rounded square.
        let on = ui.st.fx_page == i;
        let face = Rect::new(x - 11.0, y(172.0) - 10.0, 22.0, 20.0);
        if on {
            lemon::lit(ui.c, face, 6.0, st.hover);
        } else if st.hover {
            ui.c.fill_rrect(face, 6.0, Color::rgb(0x3d3d3d));
        }
        icon(ui.c, ic, x, y(172.0), 16.0, if on { brand::COLORED_DARK } else if st.hover { brand::TEXT } else { plug::DIM });
        ui.hit(r, target);
    }
    // A panel title like the others.
    let f = LABEL.bold();
    let lh = ui.c.line_height(f);
    ui.c.text(f, 337.0, (y(172.0) - lh / 2.0).round(), brand::TEXT, "Precomputed effects");
    if ui.st.fx_page == 0 {
        radio(ui, "fx_dc", 302.0, 199.0, "Remove DC offset");
        radio(ui, "fx_normalize", 302.0, 221.0, "Normalize");
        radio(ui, "fx_reverse", 302.0, 243.0, "Reverse");
        radio(ui, "fx_polarity", 430.0, 199.0, "Reverse polarity");
        radio(ui, "fx_fade_stereo", 430.0, 221.0, "Fade stereo");
        radio(ui, "fx_swap", 430.0, 243.0, "Swap stereo");
        for (id, x, cap) in [
            ("fx_start", 312.0, "SMP START"),
            ("fx_length", 354.0, "LENGTH"),
            ("fx_in", 408.0, "IN"),
            ("fx_out", 447.0, "OUT"),
            ("fx_crossfade", 494.0, "CROSSFADE"),
            ("fx_trim", 533.0, "TRIM"),
        ] {
            knob(ui, id, x, 283.5, BIG, ORANGE, false, cap);
        }
    } else {
        // BOOST + CLIP share a darker group box.
        lemon::outlined(ui.c, rr(291.0, 334.0, 187.0, 255.0), 8.0, plug::PANEL_LINE, Color::rgb(0x2d2d2d));
        knob(ui, "fx_boost", 312.5, 206.5, BIG, ORANGE, false, "BOOST");
        radio(ui, "fx_clip", 302.0, 246.0, "");
        let lh = ui.c.line_height(CAP);
        inst::caption_at(ui.c, 311.0, (y(246.0) - lh / 2.0).round(), plug::DIM, "CLIP", inst::CAPTION_TRACK);
        ui.hits.last_mut().unwrap().r.w += 30.0;
        knob(ui, "fx_eq", 355.0, 206.5, BIG, ORANGE, true, "EQ");
        knob(ui, "fx_rm_mix", 403.0, 206.5, BIG, ORANGE, false, "RM MIX");
        knob(ui, "fx_rm_freq", 442.5, 206.5, BIG, ORANGE, false, "RM FREQ");
        knob(ui, "fx_cut", 493.5, 206.5, BIG, ORANGE, false, "CUT");
        knob(ui, "fx_res", 533.5, 206.5, BIG, ORANGE, false, "RES");
        let ab = rr(303.0, 315.0, 271.0, 296.0);
        let target = Target::ReverbAb;
        let st = ui.state(Rect::new(ab.x, ab.y, 26.0, ab.h), &target);
        inst::ab_switch(ui.c, ab, ui.m.plain("fx_reverb_type") > 0.5, st);
        ui.hit(Rect::new(ab.x - 2.0, ab.y - 2.0, 28.0, ab.h + 4.0), target);
        knob(ui, "fx_reverb", 355.0, 283.5, BIG, ORANGE, false, "");
        inst::caption(ui.c, t, 342.0, y(283.5) + 17.5, "REVERB", false);
        knob(ui, "fx_sdelay", 403.0, 283.5, BIG, ORANGE, true, "STEREO DELAY");
        knob(ui, "fx_pogo", 533.5, 283.5, BIG, ORANGE, true, "POGO");
    }

    waveform(ui, sv);
}

// ---------------------------------------------------------------------------------------------------------------
// Waveform

/// Waveform area and overview track (editor coordinates).
pub fn wave_area() -> Rect {
    rr(13.0, 555.0, 328.0, 434.0)
}
pub fn overview_track() -> Rect {
    rr(28.0, 539.0, 437.0, 453.0)
}

/// The overview thumb (x from, x to): the visible part of the sample, at least 8 px.
pub fn thumb_span(st: &ViewState, ov: Rect) -> (f32, f32) {
    let a = ov.x + st.wave_start as f32 * ov.w;
    let w = (st.wave_len as f32 * ov.w).max(8.0).min(ov.w);
    let a = a.min(ov.right() - w).round();
    (a, a + w.round())
}

/// Visible range in frames.
pub fn wave_range(st: &ViewState, frames: usize) -> (f64, f64) {
    let n = frames.max(1) as f64;
    (st.wave_start * n, st.wave_len.max(1e-6) * n)
}

/// The waveform panel: a dark well with the wave in light grey, lemon loop markers over a faint lemon tint, the
/// wordmark in its top-right corner and an outlined overview bar.
fn waveform(ui: &mut Ui, sv: &SampleView) {
    let area = wave_area();
    lemon::plug_well(ui.c, area.outset(1.0), 8.0);
    ui.c.push_clip(area.inset(1.0));
    let (v0f, vlf) = (ui.st.wave_start as f32, ui.st.wave_len.max(1e-6) as f32);
    let step = 129.0 / vlf;
    let first = ((v0f / vlf * area.w) / step).floor() as i32 + 1;
    for k in first.. {
        let gx = (12.0 + k as f32 * step - v0f / vlf * area.w).round();
        if gx > area.right() {
            break;
        }
        if gx > area.x {
            ui.c.fill_rect(Rect::new(gx, area.y, 1.0, area.h), plug::GRID);
        }
    }
    let cy = area.y + (area.h / 2.0).floor();
    for gy in [cy - 27.0, cy + 27.0] {
        ui.c.fill_rect(Rect::new(area.x, gy, area.w, 1.0), plug::GRID);
    }
    ui.c.pop_clip();
    // Wordmark: the lemon mark and "LEMON" in lemon, "SAMPLER" in a light grey.
    let (big, small) = (Font::condensed(22, 0).bold(), Font::condensed(10, 0).bold());
    let wb = lemon::spaced_width(ui.c, big, "SAMPLER", 2.0);
    let ws = lemon::spaced_width(ui.c, small, "LEMON", 2.5);
    let lh_b = ui.c.line_height(big);
    let lh_s = ui.c.line_height(small);
    let wy = y(328.0) + 15.0;
    let bx = 547.0 - wb;
    lemon::spaced(ui.c, big, bx, (wy - lh_b / 2.0).round(), Color::rgb(0xffffff).with_alpha(82), "SAMPLER", 2.0);
    let sx = bx - 8.0 - ws;
    lemon::spaced(ui.c, small, sx, (wy - lh_s / 2.0).round() + 1.0, brand::MAIN, "LEMON", 2.5);
    lemon::lemon_mark(ui.c, sx - 8.0, wy, 4.0, brand::MAIN);
    // Overview bar: an outlined strip with arrows at both ends and the visible range as a lighter thumb.
    let ov = overview_track();
    lemon::outlined(ui.c, rr(12.0, 555.0, 437.0, 453.0), 6.0, Color::rgb(0x1a1a1a), Color::rgb(0x232323));
    for (x, dir) in [(19.5, -1i8), (548.5, 1)] {
        let r = Rect::new(x - 7.0, y(437.0), 15.0, 17.0);
        let target = Target::WaveArrow(dir);
        let st = ui.state(r, &target);
        let col = if st.hover { brand::MAIN } else { Color::rgb(0x8a8a8a) };
        kit::chevron(ui.c, x, y(445.5), 3.5, if dir < 0 { kit::Dir::Left } else { kit::Dir::Right }, col);
        ui.hit(r, target);
    }
    {
        let (a, b) = thumb_span(ui.st, ov);
        let st = ui.state(ov, &Target::Overview(ov));
        ui.c.fill_rrect(Rect::new(a, ov.y + 2.0, b - a, ov.h - 4.0), 5.0, if st.hover || st.pressed { Color::rgb(0x474747) } else { Color::rgb(0x3d3d3d) });
    }
    ui.hit(ov, Target::Overview(ov));
    ui.hit(area, Target::Wave(area));
    let Some(s) = sv.sample else {
        ui.c.fill_rect(Rect::new(area.x + 1.0, area.cy().round(), area.w - 2.0, 1.0), Color::rgb(0x3a3a3a));
        return;
    };
    let n = s.frames;
    let (v0, vl) = wave_range(ui.st, n);
    let fx = |f: f64| area.x + ((f - v0) / vl) as f32 * area.w;
    let looped = ui.on("loop_use") && (ui.m.norm("loop_start") > 0.0 || ui.m.norm("loop_end") < 1.0);
    if looped {
        // The loop region behind the wave.
        let (a, b) = (fx(f64::from(ui.m.norm("loop_start")) * n as f64), fx(f64::from(ui.m.norm("loop_end")) * n as f64));
        let r = Rect::new(a.max(area.x), area.y, (b.min(area.right()) - a.max(area.x)).max(0.0), area.h);
        ui.c.fill_rect(r, brand::MAIN.with_alpha(12));
    }
    let wave = Color::rgb(0xc9c9c9);
    if ui.st.spectrum {
        if let Some((w, h, px)) = sv.spectrum {
            ui.c.image(area, w, h, px);
        }
    } else if ui.st.multichannel {
        let top = Rect::new(area.x, area.y, area.w, area.h / 2.0);
        let bot = Rect::new(area.x, area.y + area.h / 2.0, area.w, area.h / 2.0);
        ui.c.fill_rect(Rect::new(area.x, bot.y.round(), area.w, 1.0), plug::WELL_LINE);
        wave_line(ui.c, &s.near, Side::Left, n, top, (v0, vl), top.h * 0.5, wave, 1.4, sv.tones);
        wave_line(ui.c, &s.near, Side::Right, n, bot, (v0, vl), bot.h * 0.5, wave, 1.4, sv.tones);
    } else {
        wave_line(ui.c, &s.near, Side::Mid, n, area, (v0, vl), area.h * 0.5, wave, 1.8, sv.tones);
    }
    let off = f64::from(ui.m.norm("start_offset")) * n as f64;
    if off > 0.0 {
        let x = fx(off).round();
        if x >= area.x && x <= area.right() {
            ui.c.fill_rect(Rect::new(x, area.y, 1.0, area.h), brand::HIGHLIGHT.with_alpha(170));
        }
    }
    if looped {
        for (i, id) in [(0u8, "loop_start"), (1, "loop_end")] {
            let x = fx(f64::from(ui.m.norm(id)) * n as f64).round();
            if x < area.x - 3.0 || x > area.right() + 3.0 {
                continue;
            }
            let target = Target::LoopHandle(i);
            let hr = Rect::new(x - 3.0, area.y, 7.0, area.h);
            let st = ui.state(hr, &target);
            let col = if st.hover || st.pressed { brand::HIGHLIGHT } else { brand::MAIN };
            ui.c.push_clip(area);
            ui.c.fill_rect(Rect::new(x, area.y, 1.0, area.h), col);
            let tri = if i == 0 { [(x + 1.0, area.y), (x + 8.0, area.y), (x + 1.0, area.y + 7.0)] } else { [(x, area.y), (x - 7.0, area.y), (x, area.y + 7.0)] };
            ui.c.fill_polygon(&tri, col);
            ui.c.pop_clip();
            ui.hit(hr, target);
        }
    }
    let inner = Rect::new(ov.x + 1.0, ov.y, ov.w - 2.0, ov.h);
    wave_line(ui.c, &s.whole, Side::Mid, n, inner, (0.0, n as f64), ov.h * 0.42, Color::rgb(0x9a9a9a), 1.3, sv.tones);
    // Bit depth and channel readout on a dark chip.
    let bits = format!("{}", sv.bits & 0xff);
    let zw = ui.c.text_width(LABEL, &bits);
    let info = Rect::new(526.0 - zw, y(418.0), zw + 30.0, 16.0);
    let st = ui.state(info, &Target::WaveInfo);
    ui.c.fill_rrect(info, 4.0, Color::rgb(0x000000).with_alpha(115));
    let col = if st.hover { brand::MAIN } else { Color::rgb(0xd0d0d0) };
    let lh = ui.c.line_height(LABEL);
    ui.c.text(LABEL, 537.0 - zw, (y(426.5) - lh / 2.0).round(), col, &bits);
    ui.c.stroke_rrect(rr(541.0, 551.0, 423.0, 430.0).inset(0.5), 2.0, 1.2, col);
    ui.hit(info, Target::WaveInfo);
}

/// Which side a line draws.
#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
    /// The mid of the two.
    Mid,
}

/// One channel (or the mid of two) of a sample of `n` frames as a line through the visible range `v0 .. v0 + vl`: one
/// point per half pixel when zoomed out, the samples themselves when zoomed in. With `tones` (colourful waveforms) each
/// device-pixel column of the line takes the colour of the frames it shows; silence keeps `col` (spec:
/// ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)").
#[allow(clippy::too_many_arguments)]
fn wave_line(c: &mut Canvas, fr: &Frames, side: Side, n: usize, area: Rect, (v0, vl): (f64, f64), amp: f32, col: Color, width: f32, tones: Option<(&Tones, &Waves)>) {
    if n == 0 || vl <= 0.0 || fr.left.is_empty() {
        return;
    }
    let cy = area.y + (area.h / 2.0).floor() + 0.5;
    let at = |j: usize| {
        fr.at(j).map(|(a, b)| match side {
            Side::Left => a,
            Side::Right => b,
            Side::Mid => 0.5 * (a + b),
        })
    };
    let cols = (area.w.max(1.0) * 2.0) as usize;
    let per = vl / cols as f64;
    let mut pts = Vec::with_capacity(cols + 2);
    if per >= 1.0 {
        for i in 0..=cols {
            let j = (v0 + i as f64 * per) as usize;
            if j >= n {
                break;
            }
            if let Some(y) = at(j) {
                pts.push((area.x + i as f32 * 0.5, cy - y * amp));
            }
        }
    } else {
        let j0 = v0.floor().max(0.0) as usize;
        let j1 = ((v0 + vl).ceil() as usize + 1).min(n);
        for j in j0..j1 {
            if let Some(y) = at(j) {
                let x = area.x + ((j as f64 - v0) / vl) as f32 * area.w;
                pts.push((x, cy - y * amp));
            }
        }
    }
    c.push_clip(area);
    match tones {
        Some((t, w)) => {
            let px = 1.0 / c.scale;
            let frame = |x: f32| v0 + f64::from((x - area.x) / area.w) * vl;
            c.polyline_columns(&pts, width, &mut |x| {
                let tone = t.span(frame(x - px / 2.0), frame(x + px / 2.0));
                w.colour(tone.centroid(), tone.band_powers()).unwrap_or(col)
            });
        }
        None => c.polyline(&pts, width, col),
    }
    c.pop_clip();
}

/// Frames in each column of [`spectrogram`].
pub const SPECTRUM_N: usize = 512;

/// A spectrum picture (`w` x `h`, 0x00RRGGBB) of frames `v0 .. v0 + vl` of the mid signal: Hann-windowed 512-point
/// FFTs, log frequency axis, magnitude in dB coloured dark blue -> orange -> white.
pub fn spectrogram(s: &Sample, v0: f64, vl: f64, w: usize, h: usize) -> Vec<u32> {
    let n = s.frames();
    spectrogram_by(n, s.rate, v0, vl, w, h, |start, mid| {
        for (i, m) in mid.iter_mut().enumerate() {
            let j = start + i as i64;
            *m = if j >= 0 && (j as usize) < n { 0.5 * (s.left[j as usize] + s.right[j as usize]) } else { 0.0 };
        }
    })
}

/// [`spectrogram`] of a sample of `n` frames at `rate` whose frames `mid` gives: the mid of [`SPECTRUM_N`] frames from
/// `start` (silence outside the sample). A sample that plays from disk reads them from its file.
#[allow(clippy::too_many_arguments)]
pub fn spectrogram_by(n: usize, rate: u32, v0: f64, vl: f64, w: usize, h: usize, mut mid: impl FnMut(i64, &mut [f32])) -> Vec<u32> {
    const N: usize = SPECTRUM_N;
    let mut out = vec![0u32; w * h];
    if n == 0 || w == 0 || h == 0 {
        return out;
    }
    let hann: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / N as f32).cos()).collect();
    let mut frames = vec![0f32; N];
    let mut re = vec![0f32; N];
    let mut im = vec![0f32; N];
    let nyq = rate as f32 / 2.0;
    for x in 0..w {
        let centre = v0 + (x as f64 + 0.5) / w as f64 * vl;
        let start = centre as i64 - N as i64 / 2;
        mid(start, &mut frames);
        for i in 0..N {
            re[i] = frames[i] * hann[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for yy in 0..h {
            // Log axis 30 Hz .. Nyquist, top = high.
            let fr = 30.0 * (nyq / 30.0).powf(1.0 - (yy as f32 + 0.5) / h as f32);
            let bin = ((fr / nyq) * (N / 2) as f32).clamp(1.0, (N / 2 - 1) as f32) as usize;
            let mag = (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() / (N as f32 / 4.0);
            let db = 20.0 * mag.max(1e-7).log10();
            let k = ((db + 90.0) / 90.0).clamp(0.0, 1.0);
            out[yy * w + x] = heat(k);
        }
    }
    out
}

fn heat(k: f32) -> u32 {
    let stops = [(0.0, (0x1b, 0x1d, 0x1e)), (0.35, (0x1a, 0x2c, 0x5a)), (0.6, (0x8a, 0x32, 0x6a)), (0.8, (0xf0, 0x8a, 0x30)), (1.0, (0xff, 0xf4, 0xd0))];
    for w in stops.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if k <= b {
            let u = (k - a) / (b - a);
            let l = |p: u8, q: u8| (f32::from(p) + (f32::from(q) - f32::from(p)) * u) as u32;
            return (l(ca.0, cb.0) << 16) | (l(ca.1, cb.1) << 8) | l(ca.2, cb.2);
        }
    }
    0xfff4d0
}

/// In-place radix-2 FFT.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (cr, ci) = (cr * wr - ci * wi, cr * wi + ci * wr);
            }
        }
        len <<= 1;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Tab 2: envelope / instrument

fn envelope_tab(ui: &mut Ui) {
    let t = ui.t;
    let sub = ui.st.sub.min(4) as usize;
    // Measured text spans of the five target tabs (FL capture t2_*); our text is centred in each.
    const SPANS: [(f32, f32); 5] = [(16.0, 55.0), (65.0, 103.0), (113.0, 145.0), (155.0, 187.0), (197.0, 220.0)];
    for (i, n) in TARGET_NAMES.iter().enumerate() {
        let w = ui.c.text_width(LABEL, n);
        let (x0, x1) = SPANS[i];
        let x = ((x0 + x1 + 1.0 - w) / 2.0).round();
        let cell = rr(x0 - 5.0, x1 + 4.0, 78.0, 106.0);
        let target = Target::SubTab(i as u8);
        let st = ui.state(cell, &target);
        lemon::text_tab(ui.c, cell, x, n, LABEL, t.a(TARGET_COLORS[i].1), sub == i, st);
        ui.hit(cell, target);
    }
    let p = ENV_PREFIX[sub];
    let id = |s: &str| format!("{p}_{s}");
    let (arc, _, line) = TARGET_COLORS[sub];

    // Envelope (the Panning target has none: FL shows only its LFO).
    if sub != 0 {
        let pr = panel(ui, 11.0, 297.0, 113.0, 310.0, 0.0, "");
        inst::panel_tab(ui.c, t, pr, 84.0, "");
        radio(ui, &id("on"), 21.0, 123.0, "");
        let lw = label(ui, 32.0, 123.0, "Envelope");
        ui.hits.last_mut().map(|h| h.r.w += lw + 4.0);
        let mr = Rect::new(277.0, y(117.0), 14.0, 16.0);
        let st = ui.state(mr, &Target::EnvMenu);
        let col = if st.disabled { plug::OFF } else if st.hover { brand::MAIN } else { plug::DIM };
        let mut g = os_ui::Glyph::new();
        let (ax, ay) = (284.0, y(126.0));
        g.polygon(&[(ax - 2.5, ay - 4.5), (ax + 3.0, ay), (ax - 2.5, ay + 4.5)]);
        ui.c.glyph(col, &g);
        ui.hit(mr, Target::EnvMenu);
        let g = env_rect();
        inst::graph_bg(ui.c, g);
        env_graph(ui, g, p, t.a(line));
        for (k, x, cap) in [("delay", 33.0, "DELAY"), ("att", 67.0, "ATT"), ("hold", 101.0, "HOLD"), ("dec", 135.0, "DEC"), ("sus", 169.0, "SUS"), ("rel", 203.0, "REL")] {
            knob(ui, &id(k), x, 243.0, MID, arc, false, cap);
        }
        if sub >= 2 {
            knob(ui, &id("amt"), 272.0, 243.0, BIG, arc, true, "AMT");
        }
        for (k, x) in [("att_t", 67.0), ("dec_t", 135.0), ("rel_t", 203.0)] {
            knob(ui, &id(k), x, 283.0, TINY, arc, true, "");
        }
        for x in [101.0, 169.0] {
            inst::caption_spaced(ui.c, t, x, y(283.0) - 4.0, "TENSION", false, 2.3);
        }
        label(ui, 240.0, 292.0, "Tempo");
        radio(ui, &id("tempo"), 284.0, 292.0, "");
    }

    // LFO.
    let lr = panel(ui, 304.0, 450.0, 113.0, 310.0, 0.0, "");
    inst::panel_tab(ui.c, t, lr, 105.0, "");
    let shape = ui.m.plain(&id("lfo_shape")) as usize;
    for (i, x) in [316.0, 341.0, 362.0].into_iter().enumerate() {
        let r = Rect::new(x - 10.0, y(114.0), 21.0, 19.0);
        let target = Target::Set(id("lfo_shape"), i as f32);
        let st = ui.state(r, &target);
        let col = lemon::choice_button(ui.c, r, i == shape, t.a(arc), st);
        lfo_icon(ui.c, i, x, y(123.0), col);
        ui.hit(r, target);
    }
    label(ui, 377.0, 123.0, "LFO");
    let lg = rr(311.0, 442.0, 140.0, 222.0);
    inst::graph_bg(ui.c, lg);
    lfo_graph(ui, lg, p, t.a(line));
    for (k, x, cap) in [("lfo_delay", 326.0, "DELAY"), ("lfo_att", 359.0, "ATT"), ("lfo_amt", 393.0, "AMT"), ("lfo_speed", 427.0, "SPEED")] {
        knob(ui, &id(k), x, 241.0, MID, arc, k == "lfo_amt", cap);
    }
    label(ui, 336.0, 292.0, "Tempo");
    radio(ui, &id("lfo_tempo"), 380.0, 292.0, "");
    label(ui, 394.0, 292.0, "Global");
    radio(ui, &id("lfo_global"), 436.0, 292.0, "");

    // Filter.
    let fr = panel(ui, 457.0, 556.0, 113.0, 310.0, 0.0, "");
    inst::panel_tab(ui.c, t, fr, 55.0, "Filter");
    let cap = plug::DIM;
    ui.c.text(MOD_CAP, 504.0, y(131.0) - 6.0, cap, "MOD X");
    big_knob(ui, "f_cut", 492.5, 168.5, 0x5ea733);
    big_knob(ui, "f_res", 520.0, 233.5, 0xc8a032);
    ui.c.text(MOD_CAP, 464.0, y(264.0) - 7.0, cap, "MOD Y");
    list(ui, "f_type", 463.0, 549.0, 282.0, 302.0);

    root_panel(ui);
}

fn lfo_icon(c: &mut Canvas, shape: usize, cx: f32, cy: f32, col: Color) {
    // One period of each wave (measured sizes: sine and triangle 13 x 11 px, pulse 11 x 9 px).
    let pts: Vec<(f32, f32)> = match shape {
        0 => (0..=24).map(|i| {
            let u = i as f32 / 24.0;
            (cx - 6.0 + 12.0 * u, cy - 4.6 * (TAU * u).sin())
        }).collect(),
        1 => vec![(cx - 8.0, cy), (cx - 5.0, cy - 4.6), (cx + 1.0, cy + 4.6), (cx + 4.0, cy)],
        _ => vec![(cx - 4.5, cy + 4.0), (cx - 4.5, cy - 3.5), (cx, cy - 3.5), (cx, cy + 3.5), (cx + 4.5, cy + 3.5), (cx + 4.5, cy - 4.0)],
    };
    c.polyline(&pts, 1.4, col);
}

fn big_knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, dot: u32) {
    let cy = y(cy);
    let r = Rect::new(cx - 26.0, cy - 26.0, 52.0, 52.0);
    let target = Target::Knob(id.into());
    let st = ui.state(r, &target);
    inst::big_dial(ui.c, cx, cy, ui.m.norm(id), ui.t.a(dot), st);
    ui.hit(r, target);
}

/// Envelope graph geometry: x of each stage end, the sustain level, the drawn level scale (|AMT| on the targets
/// that have one) and the pixels per full knob after squeezing.
pub struct EnvGeom {
    pub x: [f32; 6],
    pub sus: f32,
    pub amp: f32,
    pub px: f32,
}

/// Pixels per full knob in the envelope graph (measured, spec §3).
pub const ENV_PX: f32 = 90.0;

/// The envelope graph rect.
pub fn env_rect() -> Rect {
    rr(18.0, 290.0, 140.0, 222.0)
}

pub fn env_geom(m: &dyn Model, p: &str, g: Rect) -> EnvGeom {
    let n = |s: &str| m.norm(&format!("{p}_{s}"));
    let (dl, at, ho, de, su, re) = (n("delay"), n("att"), n("hold"), n("dec"), n("sus"), n("rel"));
    // Stage lengths in knob units: decay falls (1 - sustain) of the range at full-scale speed, the release is not scaled.
    let units = [dl, at, ho, de * (1.0 - su), re];
    let total: f32 = units.iter().sum();
    let x0 = g.x + 1.0;
    let room = g.w - 4.0;
    // Too long for the graph: squeeze the whole curve (Lemon Studio choice).
    let px = if total * ENV_PX > room && total > 0.0 { room / total } else { ENV_PX };
    let mut x = [x0; 6];
    for i in 0..5 {
        x[i + 1] = x[i] + units[i] * px;
    }
    let has_amt = !matches!(p, "envpan" | "envvol");
    let amp = if has_amt { ((n("amt") - 0.5) * 2.0).abs() } else { 1.0 };
    EnvGeom { x, sus: su, amp, px }
}

/// The envelope of the current target: each stage's width follows its knob (decay scaled by the level it falls),
/// the curves bent by their tensions, hollow nodes at the five stage ends (measured: 2 px line, no fill, drawn in the
/// target colour whether the envelope is on or not, spec §3).
fn env_graph(ui: &mut Ui, g: Rect, p: &str, col: Color) {
    let tension = |s: &str| ui.m.plain(&format!("{p}_{s}"));
    let eg = env_geom(ui.m, p, g);
    let [x0, xd, xa, xh, xs, xe] = eg.x;
    let (top, bot) = env_levels(g);
    let amp = eg.amp;
    let lvl = |l: f32| bot - (bot - top) * l * amp;
    let curve = |a: f32, b: f32, l0: f32, l1: f32, kk: f64, pts: &mut Vec<(f32, f32)>| {
        let steps = ((b - a) / 2.0).ceil().max(1.0) as usize;
        for i in 1..=steps {
            let u = i as f64 / steps as f64;
            let sh = os_engine::envelope::shape(u, kk) as f32;
            pts.push((a + (b - a) * u as f32, lvl(l0 + (l1 - l0) * sh)));
        }
    };
    let tk = |v: f32| os_engine::envelope::tension_k((v * 128.0).round() as i32);
    let su = eg.sus;
    let mut pts = vec![(x0, lvl(0.0)), (xd, lvl(0.0))];
    curve(xd, xa, 0.0, 1.0, tk(tension("att_t")), &mut pts);
    pts.push((xh, lvl(1.0)));
    curve(xh, xs, 1.0, su, -tk(tension("dec_t")), &mut pts);
    curve(xs, xe, su, 0.0, -tk(tension("rel_t")), &mut pts);
    ui.c.push_clip(g);
    ui.c.polyline(&pts, 1.8, col);
    for (i, (x, yy)) in [(xd, lvl(0.0)), (xa, lvl(1.0)), (xh, lvl(1.0)), (xs, lvl(su)), (xe, lvl(0.0))].into_iter().enumerate() {
        let target = Target::EnvPoint(i as u8);
        let hr = Rect::new(x - 5.0, yy - 5.0, 10.0, 10.0);
        let st = ui.state(hr, &target);
        let r = if st.hover || st.pressed { 4.6 } else { 4.0 };
        ui.c.fill_circle(x, yy, r, col);
        ui.c.fill_circle(x, yy, r - 1.8, col.mix(ui.t.s(0x1b1d1e), 0.8));
        ui.hits.push(Hit { r: hr, t: target });
    }
    ui.c.pop_clip();
}

/// Graph y of envelope level 1 and level 0 (measured).
pub fn env_levels(g: Rect) -> (f32, f32) {
    (g.y + 1.8, g.bottom() - 2.2)
}

/// The LFO of the current target: silent for the delay, fading in over the attack, its speed setting the cycles shown.
fn lfo_graph(ui: &mut Ui, g: Rect, p: &str, col: Color) {
    let n = |s: &str| ui.m.norm(&format!("{p}_{s}"));
    let shape = ui.m.plain(&format!("{p}_lfo_shape")) as usize;
    let amt = ui.m.plain(&format!("{p}_lfo_amt"));
    let (dl, at, sp) = (n("lfo_delay"), n("lfo_att"), n("lfo_speed"));
    let cy = g.y + ((g.h - 1.0) / 2.0).floor() + 0.5;
    let a = amt * (g.h / 2.0 - 3.0);
    // Cycles across the graph: 1 .. 16 on an exponential speed knob.
    let cycles = 2f32.powf(sp * 4.0);
    let xd = dl * g.w * 0.35;
    let xa = at * g.w * 0.35;
    let pts: Vec<(f32, f32)> = (0..=g.w as usize)
        .map(|i| {
            let x = i as f32;
            let env = if x < xd { 0.0 } else if xa > 0.0 { ((x - xd) / xa).min(1.0) } else { 1.0 };
            let ph = (x / g.w * cycles).fract();
            let w = match shape {
                0 => (2.0 * PI * ph).sin(),
                1 => 1.0 - 4.0 * ((ph + 0.25).fract() - 0.5).abs(),
                _ => {
                    if ph < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
            };
            (g.x + x, cy - a * w * env)
        })
        .collect();
    ui.c.push_clip(g);
    ui.c.fill_rect(Rect::new(g.x, cy - 0.5, g.w, 1.0), ui.t.s(0x232628));
    ui.c.polyline(&pts, 1.5, col);
    ui.c.pop_clip();
}

// ---------------------------------------------------------------------------------------------------------------
// Root note panel and keyboard (tab 2)

pub const KEYS_X: f32 = 12.0;
pub const KEYS_W: f32 = 544.0;
/// Normal keyboard: octave width, white key width, black key offsets in an octave and width (measured).
pub const OCTAVE_W: f32 = 91.0;
pub const WHITE_W: f32 = 13.0;
const BLACK_OFF: [f32; 5] = [8.0, 23.0, 47.0, 61.0, 75.0];
const BLACK_W: f32 = 8.0;
/// Multi-touch keyboard: white keys 33 px.
pub const HAND_WHITE: f32 = 33.0;
pub const KEY_COUNT: i32 = KEY_MAX + 1;
/// FL opens with C3 at x 96.
pub const DEFAULT_KEY_SCROLL: f32 = 3.0 * OCTAVE_W - (96.0 - KEYS_X);

pub fn is_black(k: i32) -> bool {
    matches!(k.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

const WHITE_INDEX: [usize; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
const BLACK_INDEX: [usize; 12] = [0, 0, 0, 1, 0, 0, 2, 0, 3, 0, 4, 0];

/// Left edge and width of a key in keyboard content px (normal keyboard, or the multi-touch one when `hand`).
pub fn key_span(k: i32, hand: bool) -> (f32, f32) {
    let oct = k.div_euclid(12) as f32;
    let n = k.rem_euclid(12) as usize;
    if hand {
        let ow = 7.0 * HAND_WHITE;
        if is_black(k) {
            // Centred on the boundary between its neighbours, 30 px.
            let edge = oct * ow + (WHITE_INDEX[n] + 1) as f32 * HAND_WHITE;
            (edge - 15.0, 30.0)
        } else {
            (oct * ow + WHITE_INDEX[n] as f32 * HAND_WHITE, HAND_WHITE)
        }
    } else if is_black(k) {
        (oct * OCTAVE_W + BLACK_OFF[BLACK_INDEX[n]], BLACK_W)
    } else {
        (oct * OCTAVE_W + WHITE_INDEX[n] as f32 * WHITE_W, WHITE_W)
    }
}

/// Content width of the keyboard.
pub fn keyboard_width(hand: bool) -> f32 {
    KEY_COUNT as f32 / 12.0 * if hand { 7.0 * HAND_WHITE } else { OCTAVE_W }
}

pub fn keyboard_max_scroll(hand: bool) -> f32 {
    (keyboard_width(hand) - KEYS_W).max(0.0)
}

/// Scroll of the keyboard in its current mode (the multi-touch keyboard follows the normal one, scaled).
pub fn scroll_px(st: &ViewState) -> f32 {
    if st.hand {
        (st.key_scroll * 7.0 * HAND_WHITE / OCTAVE_W).min(keyboard_max_scroll(true))
    } else {
        st.key_scroll
    }
}

/// Rows of the keyboard (editor y): ruler top, keys top, black bottom, keys bottom, scroll top, scroll bottom.
pub fn key_rows() -> (f32, f32, f32, f32, f32, f32) {
    (y(365.0), y(379.0), y(419.0), y(439.0), y(439.0), y(454.0))
}

/// Key under a point of the keyboard (editor coordinates).
pub fn key_at(st: &ViewState, x: f32, yy: f32) -> Option<i32> {
    let (_, kt, bb, kb, _, _) = key_rows();
    let (kt, bb, kb) = if st.hand { (y(365.0), y(400.0), y(454.0)) } else { (kt, bb, kb) };
    if yy < kt || yy >= kb || x < KEYS_X || x >= KEYS_X + KEYS_W {
        return None;
    }
    let cx = x - KEYS_X + scroll_px(st);
    let near = ((cx / if st.hand { 7.0 * HAND_WHITE } else { OCTAVE_W }) as i32) * 12;
    if yy < bb {
        for k in near - 1..near + 13 {
            if (0..KEY_COUNT).contains(&k) && is_black(k) {
                let (l, w) = key_span(k, st.hand);
                if cx >= l && cx < l + w {
                    return Some(k);
                }
            }
        }
    }
    (near - 1..near + 13).find(|&k| {
        (0..KEY_COUNT).contains(&k) && !is_black(k) && {
            let (l, w) = key_span(k, st.hand);
            cx >= l && cx < l + w
        }
    })
}

/// Nearest key to an x of the ruler (zone selection).
pub fn ruler_key(st: &ViewState, x: f32) -> i32 {
    let cx = x - KEYS_X + scroll_px(st);
    let k = (cx / OCTAVE_W * 12.0).floor() as i32;
    k.clamp(0, KEY_MAX)
}

fn root_panel(ui: &mut Ui) {
    let t = ui.t;
    let root = ui.m.plain("root") as i32;
    let (lo, hi) = match ui.st.zone_drag {
        Some((a, b)) => (a.min(b), a.max(b)),
        None => (ui.m.plain("zone_lo") as i32, ui.m.plain("zone_hi") as i32),
    };
    let zone = lo > 0 || hi < KEY_MAX;
    let title = if zone { format!("Root note: {} / zone: {} to {}", note_name(root), note_name(lo), note_name(hi)) } else { format!("Root note: {}", note_name(root)) };
    let pr = panel(ui, 11.0, 556.0, 317.0, 453.0, 0.0, "");
    let tw = ui.c.text_width(LABEL, &title);
    inst::panel_tab(ui.c, t, pr, (tw + 16.0).max(98.0), &title);
    let reset = rr(14.0, 60.0, 340.0, 360.0);
    let st = ui.state(reset, &Target::RootReset);
    inst::text_button(ui.c, reset, "Reset", st);
    ui.hit(reset, Target::RootReset);
    radio(ui, "main_pitch", 80.0, 350.0, "Enable main pitch");
    radio(ui, "add_key", 200.0, 350.0, "Add to key");
    label(ui, 279.0, 350.0, "Fine tune");
    let track = Rect::new(339.0, y(350.0) - 9.0, 90.0, 18.0);
    let target = Target::Slider("fine".into(), track);
    let sr = Rect::new(333.0, y(350.0) - 9.0, 102.0, 18.0);
    let st = ui.state(sr, &target);
    inst::hslider(ui.c, t, 333.0, 435.0, y(350.0), ui.m.norm("fine"), st);
    ui.hit(sr, target);
    // Multi-touch button: lit while on.
    let hr = Rect::new(522.0, y(337.0), 34.0, 26.0);
    let st = ui.state(hr, &Target::Hand);
    let ink = lemon::plug_button(ui.c, hr.inset(1.0), 6.0, st.with_on(ui.st.hand));
    icon(ui.c, &tb::HAND_FINGER, 541.0, y(350.0), 18.0, ink);
    ui.hit(hr, Target::Hand);
    if ui.st.hand {
        hand_keyboard(ui, root);
    } else {
        keyboard(ui, root, lo, hi, zone);
    }
}

/// White key face colour at row `yy` of a key spanning `kt..kb` (lighter towards the bottom).
fn white_face(t: &Palette, top: u32) -> (Color, Color) {
    (t.tx(top), t.tx(0xced3da))
}

/// Colour of a dimmed (out of zone) pixel: 0.3 c + #4e5156 (measured).
fn dim(c: Color) -> Color {
    let f = |v: u8, o: u32| (f32::from(v) * 0.3 + o as f32).min(255.0) as u32;
    Color::rgb((f(c.r, 0x4e) << 16) | (f(c.g, 0x51) << 8) | f(c.b, 0x56))
}

fn keyboard(ui: &mut Ui, root: i32, lo: i32, hi: i32, zone: bool) {
    let t = ui.t;
    let (lt, kt, bb, kb, st_, sb_) = key_rows();
    let area = Rect::new(KEYS_X, lt, KEYS_W, kb - lt);
    let ox = KEYS_X - ui.st.key_scroll;
    let lit = |k: i32| ui.st.lit.contains(&k);
    let out = |k: i32| zone && (k < lo || k > hi);
    let ruler = t.tx(0xbabec5);
    ui.c.push_clip(area);
    ui.c.fill_rect(Rect::new(KEYS_X, lt - 1.0, KEYS_W, 1.0), t.s(0x1c1e21));
    ui.c.fill_rect(Rect::new(KEYS_X, lt, KEYS_W, kt - lt), ruler);
    ui.c.fill_rect(Rect::new(KEYS_X, kt, KEYS_W, kb - kt), ruler);
    let vis = |l: f32, w: f32| ox + l + w >= KEYS_X && ox + l <= KEYS_X + KEYS_W;
    // White keys: C flat, the others a gradient; no gaps between them.
    for k in 0..KEY_COUNT {
        if is_black(k) {
            continue;
        }
        let (l, w) = key_span(k, false);
        if !vis(l, w) {
            continue;
        }
        let x = (ox + l).round();
        let r = Rect::new(x, kt, (ox + l + w).round() - x, kb - kt);
        let (mut a, mut b) = if k.rem_euclid(12) == 0 { (ruler, ruler) } else { white_face(t, 0xbabec5) };
        if k == root {
            a = accent(t, 0x77a9e6);
            b = a;
        } else if lit(k) {
            a = t.h(0xeca449);
            b = t.h(0xfeb65b);
        }
        if out(k) {
            a = dim(a);
            b = dim(b);
        }
        ui.c.fill_rect_gradient(r, a, b, 0.0);
        if k == root {
            // The root's column runs through the ruler.
            ui.c.fill_rect(Rect::new(r.x, lt, r.w, kt - lt), if out(k) { dim(accent(t, 0x77a9e6)) } else { accent(t, 0x77a9e6) });
        }
        ui.hit(Rect::new(r.x, kt, r.w, kb - kt), Target::Key(k));
    }
    // Black keys over them.
    for k in 0..KEY_COUNT {
        if !is_black(k) {
            continue;
        }
        let (l, w) = key_span(k, false);
        if !vis(l, w) {
            continue;
        }
        let x = (ox + l).round();
        let r = Rect::new(x, kt, w, bb - kt);
        let d = |c: Color| if out(k) { dim(c) } else { c };
        let border = if k == root { accent(t, 0x235fc3) } else { t.s(0x2f3031) };
        ui.c.fill_rect(r, d(border));
        let (top, bot) = if k == root {
            (accent(t, 0x5785d2), accent(t, 0x77a9e6))
        } else if lit(k) {
            (t.h(0xc9832f), t.h(0xeca449))
        } else {
            (t.s(0x3a3b3c), t.s(0x555657))
        };
        ui.c.fill_rect_gradient(Rect::new(x + 1.0, kt + 1.0, w - 3.0, 24.0), d(top), d(bot), 0.0);
        ui.c.fill_rect(Rect::new(x + 1.0, kt + 1.0, 1.0, bb - kt - 4.0), d(t.s(0x404142)));
        ui.c.fill_rect(Rect::new(x + 1.0, kt + 36.0, w - 3.0, 1.0), d(t.s(0x575859)));
        ui.c.fill_rect(Rect::new(x, bb, w + 1.0, 1.0), d(t.tx(0xa2a5ab)));
        ui.hit(r, Target::Key(k));
    }
    // Ruler: dim outside the zone, octave labels, zone labels, root label.
    if zone {
        for (a, b) in [(0, lo), (hi + 1, KEY_COUNT)] {
            if a >= b {
                continue;
            }
            let xa = ox + key_span(a, false).0.min(edge_x(a));
            let xb = ox + edge_x(b);
            ui.c.fill_rect(Rect::new(xa, lt, xb - xa, kt - lt), dim(ruler));
        }
    }
    let f = Font::new(12);
    let lh = ui.c.line_height(f);
    let ty = (lt + (kt - lt) / 2.0 - lh / 2.0).round();
    for o in 0..=(KEY_MAX / 12) {
        let k = o * 12;
        if zone && (k == lo || k == hi) {
            continue;
        }
        let x = ox + key_span(k, false).0;
        if x + 20.0 < KEYS_X || x > KEYS_X + KEYS_W {
            continue;
        }
        let col = if k == root { accent(t, 0x225cc3) } else if out(k) { t.s(0x6a6e74) } else { t.s(0x6a6e74) };
        ui.c.text(f, x.round() + 2.0, ty, col, &note_name(k));
    }
    if zone {
        let xa = ox + edge_x(lo);
        ui.c.text(f, xa.round() + 2.0, ty, t.s(0x2a2c30), &note_name(lo));
        let xb = ox + edge_x(hi + 1);
        ui.c.text_right(f, xb.round() - 2.0, ty, t.s(0x2a2c30), &note_name(hi));
    }
    ui.hit(Rect::new(KEYS_X, lt, KEYS_W, kt - lt), Target::Ruler);
    ui.c.pop_clip();
    // Scroll bar.
    let sb = Rect::new(KEYS_X, st_, KEYS_W, sb_ - st_);
    ui.c.fill_rect(sb, t.s(0x3f4042));
    let track = Rect::new(sb.x + 14.0, sb.y, sb.w - 28.0, sb.h);
    let total = keyboard_width(false);
    let tw = (KEYS_W / total * track.w).max(24.0);
    let tx = track.x + (track.w - tw) * (ui.st.key_scroll / keyboard_max_scroll(false).max(1.0));
    let target = Target::KeyScroll(track);
    let st = ui.state(sb, &target);
    ui.c.fill_rrect(Rect::new(tx, sb.cy() - 1.5, tw, 3.0), 1.5, if st.hover || st.pressed { t.s(0x161718) } else { t.s(0x252628) });
    for (x, dir) in [(19.0, -1i8), (549.0, 1)] {
        let r = Rect::new(x - 7.0, sb.y, 14.0, sb.h);
        let st = ui.state(r, &Target::KeyArrow(dir));
        let col = if st.hover { t.s(0x0c0d0e) } else { t.s(0x1e1f21) };
        kit::chevron(ui.c, x, sb.cy(), 3.0, if dir < 0 { kit::Dir::Left } else { kit::Dir::Right }, col);
        ui.hit(r, Target::KeyArrow(dir));
    }
    ui.hit(track, target);
}

/// Left edge (content px) of white key `k` or of the white key after a black one (zone edges).
fn edge_x(k: i32) -> f32 {
    if k >= KEY_COUNT {
        return keyboard_width(false);
    }
    let (l, w) = key_span(k, false);
    if is_black(k) { l + w / 2.0 } else { l }
}

/// Multi-touch keyboard: big keys, note names at the bottom, no ruler or scroll bar (wheel scrolls).
fn hand_keyboard(ui: &mut Ui, root: i32) {
    let t = ui.t;
    let (kt, bb, kb) = (y(365.0), y(400.0), y(454.0));
    let area = Rect::new(KEYS_X, kt, KEYS_W, kb - kt);
    let ox = KEYS_X - scroll_px(ui.st);
    let lit = |k: i32| ui.st.lit.contains(&k);
    ui.c.push_clip(area);
    ui.c.fill_rect(area, t.s(0x9da1a8));
    let f = Font::new(15);
    let lh = ui.c.line_height(f);
    for k in 0..KEY_COUNT {
        if is_black(k) {
            continue;
        }
        let (l, w) = key_span(k, true);
        let x = (ox + l).round();
        if x > KEYS_X + KEYS_W || x + w < KEYS_X {
            continue;
        }
        let r = Rect::new(x + 1.0, kt, w - 1.0, kb - kt);
        let (a, b) = if k == root {
            (accent(t, 0x77a9e6), accent(t, 0x77a9e6))
        } else if lit(k) {
            (t.h(0xeca449), t.h(0xfeb65b))
        } else if k.rem_euclid(12) == 0 {
            (t.tx(0xb0b4bb), t.tx(0xb8bcc3))
        } else {
            white_face(t, 0xc4c8cf)
        };
        ui.c.fill_rect_gradient(r, a, b, 0.0);
        let col = if k == root { accent(t, 0x235fc3) } else { t.s(0x6a6e74) };
        ui.c.text_center(f, Rect::new(r.x, kb - 20.0 - lh / 2.0 + 6.0, r.w, lh), col, &note_name(k));
        ui.hit(r, Target::Key(k));
    }
    for k in 0..KEY_COUNT {
        if !is_black(k) {
            continue;
        }
        let (l, w) = key_span(k, true);
        let x = (ox + l).round();
        if x > KEYS_X + KEYS_W || x + w < KEYS_X {
            continue;
        }
        let r = Rect::new(x, kt, w, bb - kt);
        let (edge, face) = if k == root {
            (accent(t, 0x235fc3), accent(t, 0x5785d2))
        } else if lit(k) {
            (t.h(0xa86a20), t.h(0xeca449))
        } else {
            (t.s(0x1a1b1c), t.s(0x2f3031))
        };
        ui.c.fill_rect(r, edge);
        ui.c.fill_rect_gradient(Rect::new(r.x + 2.0, r.y, r.w - 4.0, r.h - 4.0), face.mix(Color::rgb(0x505152), 0.15), face, 0.0);
        ui.c.fill_rect(Rect::new(r.x + 2.0, r.bottom() - 4.0, r.w - 4.0, 1.0), t.s(0x3c3d3e));
        ui.hit(r, Target::Key(k));
    }
    ui.c.pop_clip();
}

// ---------------------------------------------------------------------------------------------------------------
// Hints

/// FL's hint label of a parameter (the text before ": value").
pub fn hint_label(id: &str) -> Option<&'static str> {
    let suffix = if id.starts_with("env") { id.split_once('_').map_or(id, |(_, s)| s) } else { id };
    Some(match suffix {
        "start_offset" => "Sample start offset",
        "loop_start" => "Loop start position",
        "loop_end" => "Loop end position",
        "ts_pitch" => "Pitch shift",
        "ts_mul" => "Time stretch (multiplicator)",
        "ts_time" => "Time stretch (time)",
        "fx_start" => "Sample start offset",
        "fx_length" => "Sample end offset",
        "fx_in" => "Quick fade in",
        "fx_out" => "Quick fade out",
        "fx_crossfade" => "Crossfade loop",
        "fx_trim" => "Silence trimming threshold",
        "fx_boost" => "Boosts the gain",
        "fx_eq" => "Shifts the frequency balance",
        "fx_rm_mix" => "Ring modulation mix",
        "fx_rm_freq" => "Ring modulation frequency",
        "fx_cut" => "Filter cutoff",
        "fx_res" => "Filter resonance",
        "fx_reverb" => "Reverb amount",
        "fx_sdelay" => "Stereo delay",
        "fx_pogo" => "Pogo effect",
        "f_cut" => "Filter frequency (modulation X)",
        "f_res" => "Filter bandwidth (modulation Y)",
        "fine" => "Fine tuning",
        "delay" | "lfo_delay" => "Predelay time",
        "att" | "lfo_att" => "Attack time",
        "hold" => "Hold time",
        "dec" => "Decay time",
        "sus" => "Sustain level",
        "rel" => "Release time",
        "att_t" => "Attack tension",
        "dec_t" => "Decay tension",
        "rel_t" => "Release tension",
        "amt" => "Modulation amount",
        "lfo_amt" => "Amount",
        "lfo_speed" => "Speed",
        _ => return None,
    })
}

/// Description hints of switches and lists (no value shown).
fn hint_text(id: &str) -> Option<&'static str> {
    let suffix = if id.starts_with("env") { id.split_once('_').map_or(id, |(_, s)| s) } else { id };
    Some(match suffix {
        "keep_disk" => "Stream sample from hard disk when possible",
        "resample" => "Upsample to mixing frequency",
        "load_regions" => "Load sample regions and REX / ZGR beat markers",
        "load_slices" => "Load ACID and Apple Loops beat markers",
        "declick" => "Declicking method",
        "loop_use" => "Use sample loop points",
        "loop_pingpong" => "Loop forward / backward",
        "ts_mode" => "Stretching method",
        "fx_dc" => "Center vertically",
        "fx_normalize" => "Normalize to 0dB",
        "fx_reverse" => "Flip horizontally",
        "fx_polarity" => "Flip vertically",
        "fx_fade_stereo" => "Fade from left channel to right channel",
        "fx_swap" => "Swap left & right channels",
        "fx_clip" => "Add clipping at 0dB",
        "fx_reverb_type" => "Reverb type",
        "on" => "Use envelope",
        "tempo" | "lfo_tempo" => "Tempo-based time",
        "lfo_global" => "Global / retriggered LFO phase",
        "f_type" => "Filter type",
        "main_pitch" => "Affected by the main pitch?",
        "add_key" => "Add root note to triggered note (instead of pitch)",
        _ => return None,
    })
}

/// "Label: value" of a parameter (or its description for switches and lists).
pub fn param_hint(m: &dyn Model, id: &str) -> String {
    if let Some(l) = hint_label(id) {
        return format!("{l}: {}", m.text(id));
    }
    if let Some(d) = hint_text(id) {
        return d.to_string();
    }
    m.text(id)
}

/// `0:421` = seconds : milliseconds.
pub fn time_ms(frames: f64, rate: u32) -> String {
    let ms = (frames / f64::from(rate.max(1)) * 1000.0).round().max(0.0) as u64;
    format!("{}:{:03}", ms / 1000, ms % 1000)
}

/// The hint for the control under the mouse at (`x`, `yy`); a greyed-out control (`off_name`) is named as not
/// available yet.
pub fn hint(t: &Target, m: &dyn Model, st: &ViewState, sv: &SampleView, x: f32, yy: f32) -> String {
    if let Some(name) = off_name(t) {
        return format!("{name}: not available yet");
    }
    match t {
        Target::Knob(id) | Target::Toggle(id) | Target::List(id) | Target::Slider(id, _) => param_hint(m, id),
        Target::Set(id, v) if id.ends_with("lfo_shape") => match *v as i32 {
            0 => "Shape: sine".into(),
            1 => "Shape: triangle".into(),
            _ => "Shape: pulse".into(),
        },
        Target::Set(id, _) => param_hint(m, id),
        Target::ReverbAb => "Reverb type".into(),
        Target::Tab(0) => "Sample settings".into(),
        Target::Tab(_) => "Envelope / instrument settings".into(),
        Target::SubTab(_) => "Envelope / LFO target".into(),
        Target::FxPage(0) => "Tools".into(),
        Target::FxPage(_) => "Effects".into(),
        Target::FileName => sv.path.to_string(),
        Target::FileOpen => "Load sample file... / Sample history".into(),
        Target::FileBrowse => "Locate sample in browser".into(),
        Target::FileClear => "Remove sample".into(),
        Target::Key(_) | Target::Ruler | Target::KeyScroll(_) | Target::KeyArrow(_) => "Select range & offset".into(),
        Target::RootReset => "Default layer properties".into(),
        Target::Hand => "Multi-touch preview keyboard".into(),
        Target::EnvMenu => "Channel envelope presets".into(),
        Target::EnvPoint(_) => "Envelope".into(),
        Target::LoopHandle(i) => param_hint(m, if *i == 0 { "loop_start" } else { "loop_end" }),
        Target::Wave(area) => match sv.sample {
            Some(s) => {
                let n = s.frames;
                let (v0, vl) = wave_range(st, n);
                let f = v0 + f64::from((x - area.x) / area.w) * vl;
                let (cy, amp) = if st.multichannel {
                    let half = area.h / 2.0;
                    if yy < area.y + half { (area.y + half / 2.0, half / 2.0) } else { (area.y + half * 1.5, half / 2.0) }
                } else {
                    (area.cy(), area.h / 2.0)
                };
                let lin = -(yy - cy) / amp;
                let db = 20.0 * lin.abs().max(1e-6).log10();
                format!("{}\t{:.2}dB {:.2}", time_ms(f.max(0.0), s.rate), db, lin)
            }
            None => String::new(),
        },
        // **(Lemon Studio)** Where the played sample is: in memory, or read from its file while it plays (Keep on
        // disk).
        Target::Overview(_) | Target::WaveArrow(_) | Target::WaveInfo => match sv.sample {
            Some(s) => {
                let kind = if sv.bits & 0x100 != 0 { "float" } else { "int" };
                let sides = if s.stereo() { "stereo" } else { "mono" };
                let place = if s.on_disk { "on disk" } else { "in RAM" };
                format!("{}Hz, {}Bit {kind} {sides}, {}, {place}", s.rate, sv.bits & 0xff, time_ms(s.frames as f64, s.rate))
            }
            None => String::new(),
        },
    }
}
