//! Open Sampler editor: painting and hit regions (platform independent).
//!
//! Coordinates are FL window pixels from the spec tables; the editor is the window minus its 24 px title bar, so
//! every y is drawn at `y - OY`.
// spec: plugins/open-sampler.md (window, tab strip, tabs 1-3)

use std::f32::consts::PI;

use os_engine::Sample;
use os_ui::tabler as tb;
use os_ui::{Canvas, Color, Font, Rect};

use crate::params::{note_name, LFO_SHAPES};

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
const OY: f32 = 24.0;

const FRAME: Color = Color::rgb(0x25292b);
const BODY: Color = Color::rgb(0x202324);
const STRIP: Color = Color::rgb(0x1b1d1e);
const STRIP_LINE: Color = Color::rgb(0x151718);
const PANEL: Color = Color::rgb(0x222525);
const OUTLINE: Color = Color::rgb(0x1a1d1d);
const HEADER: Color = Color::rgb(0x1f2222);
const GRAPH: Color = Color::rgb(0x1b1d1e);
const TEXT: Color = Color::rgb(0xd2d8dc);
const CAPTION: Color = Color::rgb(0xb4bfc7);
const DIM: Color = Color::rgb(0x606364);
const ICON: Color = Color::rgb(0xd5dbdf);
pub const GREEN: Color = Color::rgb(0x47f49a);
pub const ORANGE: Color = Color::rgb(0xffa830);
const DOT_ORANGE: Color = Color::rgb(0xd88a2a);
const WAVE: Color = Color::rgb(0xe6e0e0);

const LABEL: Font = Font::new(13);
const CAP: Font = Font::condensed(9, 4);

fn y(v: f32) -> f32 {
    v - OY
}

/// Inclusive FL-window pixel ranges to an editor rect.
fn rr(x0: f32, x1: f32, y0: f32, y1: f32) -> Rect {
    Rect::span(x0, x1, y(y0), y(y1))
}

/// Read access to parameter values by id.
pub trait Model {
    /// Normalised value 0..=1.
    fn norm(&self, id: &str) -> f32;
    /// Plain value (bools 0 / 1, ints as numbers).
    fn plain(&self, id: &str) -> f32;
    /// Value text for the hint line.
    fn text(&self, id: &str) -> String;
    /// Parameter display name.
    fn name(&self, id: &str) -> String;
}

/// What a region of the editor does when clicked or dragged.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// Vertical drag / wheel.
    Knob(String),
    /// Bool toggle on click.
    Toggle(String),
    /// Sets an integer parameter to a plain value on click.
    Set(String, f32),
    /// Choice list: left click = next, right click = previous.
    Cycle(String),
    /// Integer spin box: vertical drag in steps, wheel.
    Spin(String),
    /// Horizontal slider over its track rect (editor coordinates).
    Slider(String, Rect),
    Tab(u8),
    SubTab(u8),
    FileOpen,
    FileClear,
    /// Keyboard key: sets the root note.
    Key(i32),
    /// Keyboard scroll strip.
    KeyScroll,
    /// Root note "Reset" button.
    RootReset,
}

impl Target {
    pub fn param(&self) -> Option<&str> {
        match self {
            Target::Knob(s) | Target::Toggle(s) | Target::Set(s, _) | Target::Cycle(s) | Target::Spin(s) | Target::Slider(s, _) => Some(s),
            Target::Key(_) | Target::RootReset => Some("root"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// Editor-only state (not saved).
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    /// 0 sample, 1 envelope / instrument.
    pub tab: u8,
    /// Envelope target: 0 Panning, 1 Volume, 2 Mod X, 3 Mod Y, 4 Pitch.
    pub sub: u8,
    /// Keyboard scroll in px (C0 at the keyboard's left edge = 0).
    pub key_scroll: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        // FL shows C3 at x 96 (spec: Root note panel).
        ViewState { tab: 0, sub: 1, key_scroll: 3.0 * OCTAVE_W - (96.0 - KEYS.x) }
    }
}

/// What the waveform panel shows.
pub struct SampleView<'a> {
    pub sample: Option<&'a Sample>,
    pub file: &'a str,
}

pub const ENV_PREFIX: [&str; 5] = ["envpan", "envvol", "envmodx", "envmody", "envpitch"];

pub struct Ui<'a> {
    pub c: &'a mut Canvas,
    pub m: &'a dyn Model,
    pub hits: Vec<Hit>,
}

impl Ui<'_> {
    fn hit(&mut self, r: Rect, t: Target) {
        self.hits.push(Hit { r, t });
    }
    fn on(&self, id: &str) -> bool {
        self.m.plain(id) > 0.5
    }
}

/// Paints the editor. `hosted` = the host's strip row carries the page tabs and the hint line (host link, spec
/// `plugins/host-link.md`): the plugin leaves that row out. Otherwise it draws its own tabs and `hint` (parameter name and
/// value under the mouse) in the strip's free space.
pub fn paint(c: &mut Canvas, m: &dyn Model, st: &ViewState, sv: &SampleView, hint: &str, hosted: bool) -> Vec<Hit> {
    let mut ui = Ui { c, m, hits: Vec::new() };
    ui.c.begin_frame(None);
    ui.c.clear(FRAME);
    ui.c.fill_rect(Rect::span(4.0, 563.0, 1.0, y(461.0)), BODY);
    if !hosted {
        tab_strip(&mut ui, st);
        if !hint.is_empty() {
            let lh = ui.c.line_height(LABEL);
            ui.c.text_clip(LABEL, 100.0, (y(48.0) - lh / 2.0).round(), 230.0, CAPTION, hint);
        }
    }
    match st.tab {
        0 => sample_tab(&mut ui, sv),
        _ => envelope_tab(&mut ui, st),
    }
    ui.hits
}

// ---------------------------------------------------------------------------------------------------------------
// Widgets

/// A panel outline with a header tab (label, slanted right edge). `head_w` = header width at its top.
fn panel(ui: &mut Ui, x0: f32, x1: f32, y0: f32, y1: f32, head_w: f32, title: &str) {
    let r = rr(x0, x1, y0, y1);
    ui.c.fill_rect(r, OUTLINE);
    ui.c.fill_rect(r.inset(1.0), PANEL);
    if head_w > 0.0 {
        header(ui, x0, y0, head_w, title);
    }
}

/// Header tab: darker fill from the panel's top-left, 20 px tall, right edge slanting in by 9 px.
fn header(ui: &mut Ui, x0: f32, y0: f32, head_w: f32, title: &str) {
    let (top, bot) = (y(y0) + 1.0, y(y0) + 20.0);
    let (l, r) = (x0 + 1.0, x0 + head_w);
    ui.c.fill_polygon(&[(l, top), (r, top), (r - 9.0, bot), (l, bot)], HEADER);
    ui.c.line(r, top, r - 9.0, bot, 1.0, OUTLINE);
    ui.c.fill_rect(Rect::new(l, bot, r - 9.0 - l, 1.0), OUTLINE);
    if !title.is_empty() {
        ui.c.text(LABEL, x0 + 4.0, top + 1.0, TEXT, title);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Size {
    /// Top strip, misc and envelope knobs: ball r 11.
    Small,
    /// Sample tab knobs: ball r 13.
    Medium,
    /// Envelope tension knobs.
    Tiny,
}

impl Size {
    fn ball(self) -> f32 {
        match self {
            Size::Small => 11.0,
            Size::Medium => 13.0,
            Size::Tiny => 9.0,
        }
    }
}

/// Knob angle (radians from straight up, clockwise) of a normalised value: an almost full turn from the bottom.
fn knob_angle(v: f32) -> f32 {
    const HALF: f32 = 175.0 * PI / 180.0;
    -HALF + 2.0 * HALF * v.clamp(0.0, 1.0)
}

/// A knob with its value arc (from the start, or from the top for bipolar knobs) and value dot in `accent`.
fn knob(ui: &mut Ui, id: &str, cx: f32, cy: f32, size: Size, accent: Color, bipolar: bool, caption: &str, cap_y: f32, enabled: bool) {
    let cy = y(cy);
    let r = size.ball();
    let v = ui.m.norm(id);
    ui.c.fill_circle(cx, cy, r + 4.0, Color::rgb(0x191c1f));
    ui.c.fill_circle(cx, cy, r + 3.0, Color::rgb(0x23262a));
    ui.c.fill_circle(cx, cy, r + 1.5, Color::rgb(0x191c1f));
    ui.c.fill_rect_gradient(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), Color::rgb(0x4e5154), Color::rgb(0x14171a), r);
    ui.c.fill_circle(cx, cy + 0.8, r - 1.2, Color::rgb(0x1e2124));
    ui.c.fill_rect_gradient(Rect::new(cx - r + 1.6, cy - r + 1.2, 2.0 * r - 3.2, 2.0 * r - 3.2), Color::rgb(0x2a2d30), Color::rgb(0x1a1d20), r - 1.6);
    let a = knob_angle(v);
    if enabled {
        let from = if bipolar { 0.0 } else { knob_angle(0.0) };
        if (a - from).abs() > 0.01 {
            let (s, e) = if a > from { (from, a) } else { (a, from) };
            ui.c.arc(cx, cy, r + 2.5, s - PI / 2.0, e - s, 2.0, accent);
        }
        let d = r - 5.0;
        let dot = if accent == ORANGE { DOT_ORANGE } else { accent };
        ui.c.fill_circle(cx + d * a.sin(), cy - d * a.cos(), 1.4, dot);
    }
    if !caption.is_empty() {
        let w = ui.c.text_width(CAP.bold(), caption);
        ui.c.text(CAP.bold(), (cx - w / 2.0).round(), y(cap_y) - 6.0, if enabled { CAPTION } else { DIM }, caption);
    }
    ui.hit(Rect::new(cx - r - 3.0, cy - r - 3.0, 2.0 * r + 6.0, 2.0 * r + 6.0), Target::Knob(id.into()));
}

/// A round radio / toggle (14 px) with an optional label to its right.
fn radio(ui: &mut Ui, id: &str, cx: f32, cy: f32, label: &str, enabled: bool) {
    let on = ui.on(id);
    radio_at(ui, cx, cy, on, enabled);
    let mut w = 16.0;
    if !label.is_empty() {
        let lh = ui.c.line_height(LABEL);
        w += 4.0 + ui.c.text(LABEL, cx + 10.0, y(cy) - lh / 2.0, if enabled { TEXT } else { DIM }, label);
    }
    ui.hit(Rect::new(cx - 8.0, y(cy) - 8.0, w, 16.0), Target::Toggle(id.into()));
}

fn radio_at(ui: &mut Ui, cx: f32, cy: f32, on: bool, enabled: bool) {
    let cy = y(cy);
    ui.c.fill_circle(cx, cy, 7.5, Color::rgb(0x606262));
    if on && enabled {
        ui.c.fill_circle(cx, cy, 6.5, Color::rgb(0x522f2f));
        ui.c.fill_circle(cx, cy, 5.6, Color::rgb(0x942b15));
        ui.c.fill_circle(cx, cy, 4.8, Color::rgb(0xee8a64));
        ui.c.fill_circle(cx, cy, 3.6, Color::rgb(0xf19672));
    } else if on {
        ui.c.fill_circle(cx, cy, 6.5, Color::rgb(0x3a2e2c));
        ui.c.fill_circle(cx, cy, 5.2, Color::rgb(0x6b4a40));
        ui.c.fill_circle(cx, cy, 3.6, Color::rgb(0x84574a));
    } else {
        ui.c.fill_circle(cx, cy, 6.5, Color::rgb(0x2e2f2f));
        ui.c.fill_circle(cx, cy, 5.6, Color::rgb(0x131516));
        ui.c.fill_circle(cx, cy, 4.8, Color::rgb(0x36393a));
    }
}

/// A dark dropdown box showing a choice parameter's text and a ▸ arrow.
fn dropdown(ui: &mut Ui, id: &str, x0: f32, x1: f32, y0: f32, y1: f32) {
    let r = rr(x0, x1, y0, y1);
    ui.c.fill_rect(r, Color::rgb(0x1b1e1e));
    ui.c.fill_rect(r.inset(1.0), Color::rgb(0x2b2c2d));
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + 2.0, r.w - 2.0, r.h - 3.0), Color::rgb(0x1d1e1f));
    let s = ui.m.text(id);
    let lh = ui.c.line_height(LABEL);
    ui.c.text_clip(LABEL, r.x + 5.0, (r.cy() - lh / 2.0).round(), r.w - 20.0, TEXT, &s);
    arrow_right(ui, r.right() - 9.0, r.cy(), Color::rgb(0x686c6e));
    ui.hit(r, Target::Cycle(id.into()));
}

fn arrow_right(ui: &mut Ui, x: f32, cy: f32, c: Color) {
    icon(ui.c, &tb::CARET_RIGHT, x, cy, 12.0, c);
}

fn label(ui: &mut Ui, x: f32, cy: f32, c: Color, s: &str) -> f32 {
    let lh = ui.c.line_height(LABEL);
    ui.c.text(LABEL, x, (y(cy) - lh / 2.0).round(), c, s)
}

fn button(ui: &mut Ui, r: Rect, s: &str, t: Target) {
    ui.c.fill_rrect(r, 2.0, Color::rgb(0x161819));
    ui.c.fill_rect_gradient(r.inset(1.0), Color::rgb(0x2c2f31), Color::rgb(0x25282a), 1.5);
    ui.c.text_center(LABEL, r, TEXT, s);
    ui.hit(r, t);
}

// ---------------------------------------------------------------------------------------------------------------
// Icons (Tabler Icons, MIT, shared with the rest of the UI through `os_ui::tabler`)

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

fn lfo_icon(c: &mut Canvas, shape: usize, cx: f32, cy: f32, col: Color) {
    // Tabler has no triangle wave: the saw-tool zigzag is the closest.
    let t = match shape {
        0 => &tb::WAVE_SINE,
        1 => &tb::WAVE_SAW_TOOL,
        _ => &tb::WAVE_SQUARE,
    };
    icon(c, t, cx, cy, 18.0, col);
}

// ---------------------------------------------------------------------------------------------------------------
// Tab strip

fn tab_strip(ui: &mut Ui, st: &ViewState) {
    ui.c.fill_rect(rr(4.0, 563.0, 25.0, 71.0), STRIP_LINE);
    ui.c.fill_rect(rr(4.0, 563.0, 26.0, 70.0), STRIP);
    // FL's third tab (miscellaneous functions) belongs to the host's channel window, not the plugin.
    for i in 0..2u8 {
        let x0 = 5.0 + 43.0 * f32::from(i);
        let cell = rr(x0, x0 + 44.0, 26.0, 71.0);
        if st.tab == i {
            ui.c.fill_rect(cell, BODY);
        }
        let col = if st.tab == i { Color::rgb(0x49f1b1) } else { ICON };
        let cx = 27.0 + 43.0 * f32::from(i);
        page_icon(ui.c, i, cx, y(48.0), col);
        ui.hit(cell, Target::Tab(i));
    }
    // The strip's right side (ON / PAN / VOL / PITCH / RANGE / TRACK) is the host's (spec: ui/plugin-window.md).
}

// ---------------------------------------------------------------------------------------------------------------
// Tab 1: sample

fn sample_tab(ui: &mut Ui, sv: &SampleView) {
    // File panel.
    panel(ui, 11.0, 280.0, 78.0, 114.0, 0.0, "");
    label(ui, 21.0, 96.0, TEXT, "File");
    let (stem, ext) = match sv.file.rfind('.') {
        Some(i) if !sv.file.is_empty() => (&sv.file[..i], &sv.file[i..]),
        _ => (sv.file, ""),
    };
    let x = 46.0 + label(ui, 46.0, 96.0, TEXT, stem);
    label(ui, x, 96.0, Color::rgb(0x8c9296), ext);
    let bx = rr(204.0, 274.0, 84.0, 108.0);
    ui.c.fill_rect(bx, Color::rgb(0x1a1c1d));
    for (x0, x1) in [(205.0, 234.0), (236.0, 255.0), (256.0, 273.0)] {
        let r = rr(x0, x1, 85.0, 107.0);
        ui.c.fill_rect(r, Color::rgb(0x242728));
    }
    icon(ui.c, &tb::FOLDER_OPEN, 220.0, y(96.0), 17.0, ICON);
    icon(ui.c, &tb::FOLDER_SEARCH, 245.5, y(96.0), 16.0, ICON);
    icon(ui.c, &tb::X, 264.5, y(96.0), 12.0, ICON);
    ui.hit(rr(205.0, 255.0, 85.0, 107.0), Target::FileOpen);
    ui.hit(rr(256.0, 273.0, 85.0, 107.0), Target::FileClear);

    panel(ui, 11.0, 280.0, 121.0, 232.0, 69.0, "Content");
    radio(ui, "keep_disk", 25.0, 158.0, "Keep on disk", true);
    radio(ui, "resample", 25.0, 178.0, "Resample", false);
    radio(ui, "load_regions", 153.0, 158.0, "Load regions", false);
    radio(ui, "load_slices", 153.0, 178.0, "Load slice markers", false);
    label(ui, 21.0, 209.0, TEXT, "Declicking mode");
    dropdown(ui, "declick", 124.0, 270.0, 199.0, 219.0);

    panel(ui, 11.0, 280.0, 239.0, 319.0, 69.0, "Playback");
    knob(ui, "start_offset", 45.0, 286.0, Size::Medium, ORANGE, false, "START OFFSET", 308.0, true);
    knob(ui, "loop_start", 98.0, 286.0, Size::Medium, ORANGE, false, "START", 308.0, true);
    knob(ui, "loop_end", 140.0, 286.0, Size::Medium, ORANGE, false, "END", 308.0, true);
    icon(ui.c, &tb::ROTATE_CLOCKWISE_2, 120.0, y(308.0), 13.0, Color::rgb(0x9fb4c8));
    radio(ui, "loop_use", 184.0, 274.0, "Use loop points", true);
    radio(ui, "loop_pingpong", 184.0, 294.0, "Ping pong loop", true);

    panel(ui, 287.0, 556.0, 78.0, 155.0, 105.0, "Time stretching");
    knob(ui, "ts_pitch", 312.0, 121.0, Size::Medium, ORANGE, true, "PITCH", 143.0, true);
    knob(ui, "ts_mul", 356.0, 121.0, Size::Medium, ORANGE, true, "MUL", 143.0, true);
    knob(ui, "ts_time", 392.0, 121.0, Size::Medium, ORANGE, false, "TIME", 143.0, true);
    label(ui, 415.0, 112.0, TEXT, "Mode");
    dropdown(ui, "ts_mode", 456.0, 548.0, 102.0, 122.0);

    panel(ui, 287.0, 556.0, 162.0, 319.0, 0.0, "");
    header(ui, 287.0, 162.0, 172.0, "");
    ui.c.fill_rect(rr(288.0, 309.0, 163.0, 181.0), Color::rgb(0x191b1c));
    icon(ui.c, &tb::TOOL, 298.0, y(172.0), 16.0, ORANGE);
    icon(ui.c, &tb::FLASK_2, 322.0, y(172.0), 16.0, ICON);
    label(ui, 337.0, 172.0, TEXT, "Precomputed effects");
    radio(ui, "fx_dc", 302.0, 199.0, "Remove DC offset", true);
    radio(ui, "fx_normalize", 302.0, 221.0, "Normalize", true);
    radio(ui, "fx_reverse", 302.0, 243.0, "Reverse", true);
    radio(ui, "fx_polarity", 430.0, 199.0, "Reverse polarity", true);
    radio(ui, "fx_fade_stereo", 430.0, 221.0, "Fade stereo", true);
    radio(ui, "fx_swap", 430.0, 243.0, "Swap stereo", true);
    for (id, x, cap) in [("fx_start", 312.0, "SMP START"), ("fx_length", 354.0, "LENGTH"), ("fx_in", 408.0, "IN"), ("fx_out", 447.0, "OUT"), ("fx_crossfade", 494.0, "CROSSFADE"), ("fx_trim", 533.0, "TRIM")] {
        knob(ui, id, x, 283.0, Size::Medium, ORANGE, false, cap, 307.0, true);
    }

    waveform(ui, sv);
}

fn waveform(ui: &mut Ui, sv: &SampleView) {
    let area = rr(13.0, 555.0, 328.0, 434.0);
    ui.c.fill_rect(rr(12.0, 556.0, 327.0, 454.0), Color::rgb(0x151718));
    ui.c.fill_rect(area, GRAPH);
    let cy = area.cy().round() + 0.5;
    for i in 1..4 {
        let gx = (area.x + area.w * i as f32 / 4.25).round();
        ui.c.fill_rect(Rect::new(gx, area.y, 1.0, area.h), Color::rgb(0x161819));
    }
    for gy in [area.y + area.h * 0.25, area.y + area.h * 0.75] {
        ui.c.fill_rect(Rect::new(area.x, gy.round(), area.w, 1.0), Color::rgb(0x161819));
    }
    let wm = "Open Sampler";
    let f = Font::light(20, 0);
    let w = ui.c.text_width(f, wm);
    ui.c.text(f, 545.0 - w, y(334.0), Color::rgb(0x3c3f40), wm);
    // Overview strip with arrows.
    let ov = rr(28.0, 540.0, 437.0, 453.0);
    ui.c.fill_rect(rr(13.0, 555.0, 437.0, 453.0), Color::rgb(0x202223));
    ui.c.fill_rect(ov, Color::rgb(0x1d1f20));
    for (x, dir) in [(20.0, -1.0), (548.0, 1.0)] {
        let c = y(445.0);
        ui.c.line(x + 2.0 * dir * -1.0, c - 3.5, x + 2.0 * dir, c, 1.0, Color::rgb(0x7a7e80));
        ui.c.line(x + 2.0 * dir, c, x + 2.0 * dir * -1.0, c + 3.5, 1.0, Color::rgb(0x7a7e80));
    }
    let Some(s) = sv.sample else {
        ui.c.fill_rect(Rect::new(area.x, cy - 0.5, area.w, 1.0), Color::rgb(0x6a6e70));
        return;
    };
    wave_line(ui.c, s, area, cy, area.h * 0.5, WAVE, 1.8);
    wave_line(ui.c, s, Rect::new(ov.x + 3.0, ov.y, ov.w - 6.0, ov.h), ov.cy(), ov.h * 0.42, Color::rgb(0x9c9a9a), 1.0);
    let zoom = "16";
    let zf = LABEL;
    let zw = ui.c.text_width(zf, zoom);
    ui.c.text(zf, 538.0 - zw, y(419.0), TEXT, zoom);
    icon(ui.c, &tb::MAXIMIZE, 545.5, y(426.0), 11.0, TEXT);
}

/// The sample's mid signal as a smooth line, one point per pixel column (FL draws the decimated wave, not min / max).
fn wave_line(c: &mut Canvas, s: &Sample, area: Rect, cy: f32, amp: f32, col: Color, width: f32) {
    let n = s.frames();
    if n == 0 {
        return;
    }
    let cols = (area.w.max(1.0) * 2.0) as usize;
    let pts: Vec<(f32, f32)> = (0..cols)
        .map(|i| {
            let j = (i * n / cols).min(n - 1);
            let v = 0.5 * (s.left[j] + s.right[j]);
            (area.x + i as f32 * 0.5, cy - v * amp)
        })
        .collect();
    c.push_clip(area);
    c.polyline(&pts, width, col);
    c.pop_clip();
}

// ---------------------------------------------------------------------------------------------------------------
// Tab 2: envelope / instrument

fn envelope_tab(ui: &mut Ui, st: &ViewState) {
    let names = ["Panning", "Volume", "Mod X", "Mod Y", "Pitch"];
    let mut x = 15.0;
    for (i, n) in names.iter().enumerate() {
        let w = ui.c.text_width(LABEL, n);
        let cell = rr(x - 4.0, x + w + 3.0, 78.0, 106.0);
        let active = st.sub as usize == i;
        if active {
            ui.c.fill_rect(cell, STRIP);
        }
        label(ui, x, 92.0, if active { Color::rgb(0x46f08e) } else { TEXT }, n);
        ui.hit(cell, Target::SubTab(i as u8));
        x += w + 9.0;
    }
    let p = ENV_PREFIX[st.sub as usize];
    let id = |s: &str| format!("{p}_{s}");

    // Envelope.
    panel(ui, 11.0, 297.0, 113.0, 310.0, 0.0, "");
    header(ui, 11.0, 113.0, 84.0, "");
    radio(ui, &id("on"), 21.0, 123.0, "", true);
    label(ui, 32.0, 123.0, TEXT, "Envelope");
    arrow_right(ui, 284.0, y(127.0), ICON);
    let g = rr(18.0, 291.0, 140.0, 222.0);
    ui.c.fill_rect(g, GRAPH);
    env_graph(ui, g, p);
    let time_caps = [("delay", 33.0, "DELAY"), ("att", 67.0, "ATT"), ("hold", 101.0, "HOLD"), ("dec", 135.0, "DEC"), ("sus", 169.0, "SUS"), ("rel", 203.0, "REL")];
    for (k, x, cap) in time_caps {
        knob(ui, &id(k), x, 243.0, Size::Small, GREEN, false, cap, 265.0, true);
    }
    if st.sub != 1 {
        knob(ui, &id("amt"), 33.0, 283.0, Size::Tiny, GREEN, true, "", 0.0, true);
    }
    for (k, x) in [("att_t", 67.0), ("dec_t", 135.0), ("rel_t", 203.0)] {
        knob(ui, &id(k), x, 283.0, Size::Tiny, GREEN, true, "", 0.0, true);
    }
    for x in [101.0, 169.0] {
        let f = CAP.bold();
        let w = ui.c.text_width(f, "TENSION");
        ui.c.text(f, (x - w / 2.0).round(), y(283.0) - 6.0, CAPTION, "TENSION");
    }
    label(ui, 240.0, 292.0, TEXT, "Tempo");
    radio(ui, &id("tempo"), 284.0, 292.0, "", true);

    // LFO.
    panel(ui, 305.0, 450.0, 113.0, 310.0, 0.0, "");
    header(ui, 305.0, 113.0, 105.0, "");
    let shape = ui.m.plain(&id("lfo_shape")) as usize;
    for (i, x) in [316.0, 341.0, 362.0].into_iter().enumerate() {
        if i == shape {
            ui.c.fill_rect(Rect::new(x - 10.0, y(114.0), 21.0, 19.0), Color::rgb(0x1a1c1d));
        }
        lfo_icon(ui.c, i, x, y(123.0), if i == shape { GREEN } else { ICON });
        ui.hit(Rect::new(x - 10.0, y(114.0), 21.0, 19.0), Target::Set(id("lfo_shape"), i as f32));
    }
    let _ = LFO_SHAPES;
    label(ui, 377.0, 123.0, TEXT, "LFO");
    let lg = rr(311.0, 443.0, 140.0, 222.0);
    ui.c.fill_rect(lg, GRAPH);
    lfo_graph(ui, lg, shape, ui.m.plain(&id("lfo_amt")));
    for (k, x, cap) in [("lfo_delay", 326.0, "DELAY"), ("lfo_att", 359.0, "ATT"), ("lfo_amt", 393.0, "AMT"), ("lfo_speed", 427.0, "SPEED")] {
        knob(ui, &id(k), x, 241.0, Size::Small, GREEN, k == "lfo_amt", cap, 265.0, true);
    }
    label(ui, 336.0, 292.0, TEXT, "Tempo");
    radio(ui, &id("lfo_tempo"), 380.0, 292.0, "", true);
    label(ui, 394.0, 292.0, TEXT, "Global");
    radio(ui, &id("lfo_global"), 436.0, 292.0, "", true);

    // Filter.
    panel(ui, 457.0, 556.0, 113.0, 310.0, 0.0, "");
    header(ui, 457.0, 113.0, 55.0, "Filter");
    let f = CAP.bold();
    ui.c.text(f, 504.0, y(131.0) - 6.0, CAPTION, "MOD X");
    big_knob(ui, "f_cut", 493.0, 167.0);
    big_knob(ui, "f_res", 520.0, 233.0);
    ui.c.text(f, 464.0, y(264.0) - 6.0, CAPTION, "MOD Y");
    dropdown(ui, "f_type", 463.0, 549.0, 282.0, 302.0);

    root_panel(ui, st);
}

/// The filter's big knobs: a metallic cap with a tick ring.
fn big_knob(ui: &mut Ui, id: &str, cx: f32, cy: f32) {
    let cy = y(cy);
    let v = ui.m.norm(id);
    for i in 0..11 {
        let a = knob_angle(i as f32 / 10.0) * 0.86;
        let (s, c) = (a.sin(), a.cos());
        ui.c.line(cx + 26.0 * s, cy - 26.0 * c, cx + 30.0 * s, cy - 30.0 * c, 1.0, Color::rgb(0x34373a));
    }
    ui.c.fill_circle(cx, cy, 24.0, Color::rgb(0x0e0f10));
    ui.c.fill_circle(cx, cy, 22.0, Color::rgb(0x1a1c1e));
    ui.c.fill_rect_gradient(Rect::new(cx - 18.0, cy - 18.0, 36.0, 36.0), Color::rgb(0x5a5d60), Color::rgb(0x202225), 18.0);
    ui.c.fill_radial(Rect::new(cx - 15.0, cy - 15.0, 30.0, 30.0), Color::rgb(0x3e4144), Color::rgb(0x26282b));
    let a = knob_angle(v) * 0.86;
    let d = 20.0;
    let dot = if id == "f_res" { Color::rgb(0xc8a032) } else { Color::rgb(0x3cc83c) };
    ui.c.fill_circle(cx + d * a.sin(), cy - d * a.cos(), 1.5, dot);
    ui.hit(Rect::new(cx - 24.0, cy - 24.0, 48.0, 48.0), Target::Knob(id.into()));
}

/// The envelope of the current target as FL draws it: each stage's width follows its knob (decay scaled by the drop to
/// the sustain), no sustain segment, decay and release curved by their tensions.
fn env_graph(ui: &mut Ui, g: Rect, p: &str) {
    let n = |s: &str| ui.m.norm(&format!("{p}_{s}"));
    let t = |s: &str| ui.m.plain(&format!("{p}_{s}"));
    let (dl, at, ho, de, su, re) = (n("delay"), n("att"), n("hold"), n("dec"), n("sus"), n("rel"));
    let k = 95.0f32;
    let (x0, top, bot) = (g.x, g.y, g.bottom());
    let lvl = |l: f32| bot - (bot - top) * l;
    let xd = x0 + dl * k;
    let xa = xd + at * k;
    let xh = xa + ho * k;
    let xs = xh + de * (1.0 - su) * k;
    let xe = xs + re * k;
    let curve = |a: f32, b: f32, l0: f32, l1: f32, kk: f64, pts: &mut Vec<(f32, f32)>| {
        let steps = ((b - a) / 2.0).ceil().max(1.0) as usize;
        for i in 1..=steps {
            let u = i as f64 / steps as f64;
            let sh = os_engine::envelope::shape(u, kk) as f32;
            pts.push((a + (b - a) * u as f32, lvl(l0 + (l1 - l0) * sh)));
        }
    };
    let tk = |v: f32| os_engine::envelope::tension_k((v * 128.0).round() as i32);
    let mut pts = vec![(x0, bot), (xd, bot)];
    curve(xd, xa, 0.0, 1.0, tk(t("att_t")), &mut pts);
    pts.push((xh, top));
    curve(xh, xs, 1.0, su, -tk(t("dec_t")), &mut pts);
    curve(xs, xe, su, 0.0, -tk(t("rel_t")), &mut pts);
    let col = GREEN.mix(Color::rgb(0x1b1d1e), 0.25);
    ui.c.push_clip(g);
    ui.c.polyline(&pts, 1.4, col);
    for (x, yy) in [(x0, bot), (xa, top), (xh, top), (xs, lvl(su)), (xe, bot)] {
        ui.c.fill_circle(x, yy, 3.5, col);
        ui.c.fill_circle(x, yy, 2.2, GRAPH);
    }
    ui.c.pop_clip();
}

fn lfo_graph(ui: &mut Ui, g: Rect, shape: usize, amt: f32) {
    let cy = g.cy().round() + 0.5;
    let a = amt * (g.h / 2.0 - 4.0);
    let pts: Vec<(f32, f32)> = (0..=g.w as usize)
        .map(|i| {
            let t = i as f32 / g.w * 2.0;
            let w = match shape {
                0 => (2.0 * PI * t).sin(),
                1 => 1.0 - 4.0 * ((t + 0.25).fract() - 0.5).abs(),
                _ => if t.fract() < 0.5 { 1.0 } else { -1.0 },
            };
            (g.x + i as f32, cy - a * w)
        })
        .collect();
    ui.c.push_clip(g);
    ui.c.polyline(&pts, 1.6, GREEN.mix(Color::rgb(0x1b1d1e), 0.25));
    ui.c.pop_clip();
}

// ---------------------------------------------------------------------------------------------------------------
// Root note panel and keyboard (tab 2)

const KEYS: Rect = Rect { x: 12.0, y: 0.0, w: 544.0, h: 0.0 };
const OCTAVE_W: f32 = 92.0;
const WHITE_W: f32 = OCTAVE_W / 7.0;
/// Keys 0..=131 (C0..B10).
const KEY_COUNT: i32 = 132;

fn root_panel(ui: &mut Ui, st: &ViewState) {
    let root = ui.m.plain("root") as i32;
    panel(ui, 11.0, 556.0, 317.0, 453.0, 0.0, "");
    header(ui, 11.0, 317.0, 98.0, &format!("Root note: {}", note_name(root)));
    button(ui, rr(14.0, 60.0, 340.0, 360.0), "Reset", Target::RootReset);
    radio(ui, "main_pitch", 80.0, 350.0, "Enable main pitch", true);
    radio(ui, "add_key", 200.0, 350.0, "Add to key", true);
    label(ui, 279.0, 350.0, TEXT, "Fine tune");
    slider(ui, "fine", 333.0, 435.0, 350.0);
    icon(ui.c, &tb::HAND_FINGER, 541.0, y(350.0), 18.0, ICON);
    keyboard(ui, st, root);
}

/// Horizontal slider with tick marks under the track and a round red-lined thumb.
fn slider(ui: &mut Ui, id: &str, x0: f32, x1: f32, cy: f32) {
    let cy = y(cy);
    ui.c.fill_rrect(Rect::new(x0, cy - 2.5, x1 - x0, 5.0), 2.0, Color::rgb(0x161819));
    for i in 0..=8 {
        let x = (x0 + 6.0 + (x1 - x0 - 12.0) * i as f32 / 8.0).round() + 0.5;
        ui.c.line(x, cy + 5.0, x, cy + if i % 4 == 0 { 10.0 } else { 8.0 }, 1.0, Color::rgb(0x3a3d3f));
    }
    let v = ui.m.norm(id);
    let tx = x0 + 6.0 + (x1 - x0 - 12.0) * v;
    ui.c.fill_circle(tx, cy, 8.0, Color::rgb(0x0f1011));
    ui.c.fill_rect_gradient(Rect::new(tx - 7.0, cy - 7.0, 14.0, 14.0), Color::rgb(0x55585b), Color::rgb(0x1b1d1f), 7.0);
    ui.c.line(tx, cy - 6.0, tx, cy + 6.0, 1.5, Color::rgb(0xe03030));
    let track = Rect::new(x0, cy - 9.0, x1 - x0, 18.0);
    ui.hit(track, Target::Slider(id.into(), Rect::new(x0 + 6.0, track.y, x1 - x0 - 12.0, track.h)));
}

fn is_black(k: i32) -> bool {
    matches!(k.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// Left edge of a white key / centre of a black key, in content px.
fn key_x(k: i32) -> f32 {
    const WHITE_INDEX: [f32; 12] = [0.0, 0.0, 1.0, 1.0, 2.0, 3.0, 3.0, 4.0, 4.0, 5.0, 5.0, 6.0];
    let oct = k.div_euclid(12) as f32;
    let n = k.rem_euclid(12) as usize;
    let base = oct * OCTAVE_W + WHITE_INDEX[n] * WHITE_W;
    if is_black(k as i32) { base + WHITE_W } else { base }
}

pub fn keyboard_max_scroll() -> f32 {
    (KEY_COUNT as f32 / 12.0 * OCTAVE_W - KEYS.w).max(0.0)
}

fn keyboard(ui: &mut Ui, st: &ViewState, root: i32) {
    let (lt, kt, bb, kb) = (y(364.0), y(378.0), y(419.0), y(439.0));
    let area = Rect::new(KEYS.x, lt, KEYS.w, kb - lt + 1.0);
    let ox = KEYS.x - st.key_scroll;
    ui.c.push_clip(area);
    ui.c.fill_rect(Rect::new(KEYS.x, lt, KEYS.w, kt - lt), Color::rgb(0xbabec5));
    ui.c.fill_rect(Rect::new(KEYS.x, kt, KEYS.w, kb - kt + 1.0), Color::rgb(0x2f3031));
    for k in 0..KEY_COUNT {
        if is_black(k) {
            continue;
        }
        let x = ox + key_x(k);
        if x > KEYS.x + KEYS.w || x + WHITE_W < KEYS.x {
            continue;
        }
        let r = Rect::new(x.round() + 1.0, kt, (x + WHITE_W).round() - x.round() - 1.0, kb - kt + 1.0);
        if k == root {
            ui.c.fill_rect(Rect::new(r.x - 1.0, lt, r.w + 1.0, kb - lt + 1.0), Color::rgb(0x77a9e6));
        } else {
            ui.c.fill_rect(Rect::new(r.x, kt, r.w, bb - kt), Color::rgb(0xc1c5cc));
            ui.c.fill_rect_gradient(Rect::new(r.x, bb, r.w, kb - bb + 1.0), Color::rgb(0xc8ccd3), Color::rgb(0xcfd2da), 0.0);
        }
        ui.hit(Rect::new(r.x - 1.0, kt, r.w + 1.0, kb - kt + 1.0), Target::Key(k));
        if k % 12 == 0 {
            let f = Font::new(13);
            ui.c.text(f, x.round() + 2.0, lt, Color::rgb(0x2a2c30), &format!("C{}", k / 12));
        }
    }
    for k in 0..KEY_COUNT {
        if !is_black(k) {
            continue;
        }
        let cx = ox + key_x(k);
        if cx - 6.0 > KEYS.x + KEYS.w || cx + 6.0 < KEYS.x {
            continue;
        }
        let r = Rect::new((cx - 4.5).round(), kt, 9.0, bb - kt);
        if k == root {
            ui.c.fill_rect(r, Color::rgb(0x235fc3));
            ui.c.fill_rect(r.inset(1.0), Color::rgb(0x5785d2));
        } else {
            ui.c.fill_rect(r, Color::rgb(0x2f3031));
            ui.c.fill_rect_gradient(Rect::new(r.x + 1.5, r.y, r.w - 3.0, r.h - 4.0), Color::rgb(0x3a3b3c), Color::rgb(0x59595a), 0.0);
            ui.c.fill_rect(Rect::new(r.x + 1.0, r.y, 1.0, r.h - 3.0), Color::rgb(0x6f7071));
        }
        ui.hit(r, Target::Key(k));
    }
    ui.c.pop_clip();
    // Scroll strip.
    let sb = rr(12.0, 556.0, 440.0, 452.0);
    ui.c.fill_rect(sb, Color::rgb(0x3f4042));
    let total = KEY_COUNT as f32 / 12.0 * OCTAVE_W;
    let tw = (KEYS.w / total * KEYS.w).max(20.0) * 0.5;
    let tx = KEYS.x + 14.0 + (KEYS.w - 28.0 - tw) * (st.key_scroll / keyboard_max_scroll().max(1.0));
    ui.c.fill_rrect(Rect::new(tx, sb.cy() - 1.5, tw, 3.0), 1.5, Color::rgb(0x1e1f21));
    for (x, dir) in [(19.0, -1.0f32), (549.0, 1.0)] {
        let c = sb.cy();
        ui.c.line(x - 1.5 * dir, c - 3.0, x + 1.5 * dir, c, 1.0, Color::rgb(0x1e1f21));
        ui.c.line(x + 1.5 * dir, c, x - 1.5 * dir, c + 3.0, 1.0, Color::rgb(0x1e1f21));
    }
    ui.hit(sb, Target::KeyScroll);
}
