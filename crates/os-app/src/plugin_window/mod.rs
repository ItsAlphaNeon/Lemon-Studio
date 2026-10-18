// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The window around a hosted plugin's editor: Lemon Studio's title bar (menu arrow, gear, channel name, "(mixer target)",
//! Presets box, minimize, close) and the gear-toggled strip, one row: the plugin's page tabs (a plain plugin has the plug
//! tab, a Lemon plugin joined through the host link brings its own pages), the Misc wrench, then
//! ON / PAN / VOL / PITCH / RANGE / TRACK. The plugin parents its editor into a child area below the strip.
//!
//! This module is the frame itself, the same everywhere: its state, drawing (os-ui) and what input does to it. The
//! native window around it is the platform's:
//! - `win32`: docked (a child of the main window, clipped to the workspace and stacked with the app's own windows) until
//!   the menu arrow's "Detached" makes it a top-level window owned by the main window.
//! - `x11` (Linux): docked as on Windows when the main window is an X11 one; always a top-level window under Wayland.
//!   Plugin editors on Linux embed only in X11 windows, so under Wayland the frame is an XWayland window, and an X11
//!   window cannot be a child of the Wayland main window.
//! - `cocoa` (macOS): docked as on Windows (a view in the main window's view, masked to the workspace) until "Detached"
//!   moves it into a panel of its own, kept above the main window while Lemon Studio is active.
//!
// spec: ui/plugin-window.md, plugins/host-link.md, plugins/lemon-sampler.md (strip geometry), ui/knobs.md (drag 1/1024 per px)

use std::f32::consts::PI;

use os_hostlink::PageIcon;
use os_ui::tabler as tb;
use os_ui::{Canvas, Color, Font, Rect};

#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use win32::{default_position, first_position, lift_hide, lift_show, stack, take_exposed, take_keys, take_link, FrameWindow};

#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
pub use x11::{
    busy, default_position, exposed, focus_main, lift_hide, lift_show, main_focused, paint_dirty, pointer, pump, screen_to_main, stack, take_exposed, take_lift_input, unpinned,
    FrameWindow, Unpinned,
};

#[cfg(target_os = "macos")]
mod cocoa;
#[cfg(target_os = "macos")]
pub use cocoa::{default_position, lift_hide, lift_show, pointer, row_window_at, screen_to_main, stack, take_exposed, take_keys, take_lift_input, FrameWindow};

/// Where a detached window opening for the first time goes: where a new window opens. Only Windows places it by
/// monitor (Settings "Open detached plugins with the main window"; spec: ui/settings-choices.md "Miscellaneous:
/// naming, detached plugins and startup").
#[cfg(not(windows))]
pub fn first_position(owner: isize, scale: f32, _with_main: bool) -> (i32, i32) {
    default_position(owner, scale)
}

/// Point `p` on a monitor whose work area is `from` ([left, top, right, bottom], screen px) moved to the same place in
/// work area `to`, kept inside it with at least `keep` px of the window left on screen.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn moved_to(p: (i32, i32), from: [i32; 4], to: [i32; 4], keep: (i32, i32)) -> (i32, i32) {
    let (x, y) = (to[0] + p.0 - from[0], to[1] + p.1 - from[1]);
    (x.min(to[2] - keep.0).max(to[0]), y.min(to[3] - keep.1).max(to[1]))
}

/// A message a Lemon plugin's editor sent to its frame (host link, plugin -> host).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkMsg {
    /// Hint or page changed: poll the link now.
    Changed,
    /// Preview note from the plugin's keyboard: key, velocity, on.
    Note(u8, u8, bool),
    /// "Create automation clip" on the plugin parameter at this list position.
    Automate(usize),
}

/// Mouse input on the lifted popup (see `lift_show`); positions in the main window (physical px).
#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiftInput {
    Move(i32, i32),
    /// X button: 1 left, 2 middle, 3 right.
    Down(u8),
    Up,
}

/// Host link messages from Lemon plugins' editors since the last call: (editor area window, message). The editors queue
/// them in `os_hostlink` (on Linux from threads of their own; UI thread).
#[cfg(not(windows))]
pub fn take_link() -> Vec<(isize, LinkMsg)> {
    os_hostlink::take_host_messages()
        .into_iter()
        .map(|(area, cmd, arg)| {
            let m = match cmd {
                os_hostlink::Cmd::Note => {
                    let (k, v, on) = os_hostlink::unpack_note(arg);
                    LinkMsg::Note(k, v, on)
                }
                os_hostlink::Cmd::Automate => LinkMsg::Automate(arg.max(0) as usize),
                _ => LinkMsg::Changed,
            };
            (area as isize, m)
        })
        .collect()
}

/// Title bar height (outline row included); the strip, or the editor's outline, starts here.
pub const TITLE_H: f32 = 27.0;
/// Strip height, its outline included (y 27..72).
pub const STRIP_H: f32 = 46.0;
/// Left / right / bottom inset of the editor: 1 px outline, 5 px frame, 1 px editor outline.
pub const BORDER: f32 = 7.0;
/// Width of the Presets box in the title bar (its right edge is 49 px from the window's).
const PRESETS_W: f32 = 80.0;

fn c_frame() -> Color {
    os_theme::current().s(0x25292b)
}
fn c_outline() -> Color {
    os_theme::current().s(0x191d1f)
}
fn c_top_light() -> Color {
    os_theme::current().s(0x2b2f31)
}
fn c_editor_line() -> Color {
    os_theme::current().s(0x13191d)
}
fn c_presets() -> Color {
    os_theme::current().s(0x1d2123)
}
fn c_arrow() -> Color {
    os_theme::current().tx(0xc2c8cc)
}
fn c_gear_on() -> Color {
    os_theme::current().a(0x8eff7e)
}
fn c_strip() -> Color {
    os_theme::current().s(0x1b1d1e)
}
fn c_strip_line() -> Color {
    os_theme::current().s(0x151718)
}
fn c_body() -> Color {
    os_theme::current().s(0x202324)
}
fn c_text() -> Color {
    os_theme::current().tx(0xd4dade)
}
fn c_grey() -> Color {
    os_theme::current().tx(0x8c9296)
}
fn c_icon() -> Color {
    os_theme::current().tx(0xd4dade)
}
fn c_green() -> Color {
    os_theme::current().a(0x47f49a)
}
fn c_caption() -> Color {
    os_theme::current().tx(0x9fb4c8)
}

/// Channel values the strip shows (pushed by the app).
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelInfo {
    pub name: String,
    /// "Insert 3" / "Master".
    pub target: String,
    pub enabled: bool,
    /// 0..1, -1..1, cents.
    pub volume: f64,
    pub pan: f64,
    pub pitch_cents: i32,
    /// Semitones (pitch knob range).
    pub range: i32,
    /// Insert number, 0 = master.
    pub track: u16,
    /// The channel's Misc tab values (`None`: no channel behind the window, e.g. mixer effects: no Misc tab, and the
    /// strip's ON / PAN / VOL do nothing).
    pub misc: Option<crate::misc_page::MiscVals>,
}

/// What the user did in the frame, for the app to apply to the channel.
#[derive(Clone, Debug, PartialEq)]
pub enum FrameEvent {
    Close,
    /// The user picked page `n` of the plugin (a tab left of the Misc tab).
    Page(usize),
    ToggleOn,
    Volume(f64),
    Pan(f64),
    PitchCents(i32),
    /// The window was clicked: it comes to the front of the app's window stack (docked windows, Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Raise,
    /// The menu arrow was clicked: the app opens the window menu at this screen point.
    Menu(i32, i32),
    /// The Presets box was clicked: the app opens the preset list at this screen point (spec: plugins/presets.md).
    Presets(i32, i32),
    /// The Presets box arrows (or the wheel over the box): next (+1) / previous (-1) preset.
    PresetStep(i32),
    /// A Misc tab value changed (spec: ui/plugin-window.md "Misc tab").
    Misc(crate::misc_page::M, f32),
    /// A Misc tab list (chord list, number box values): the app opens it at this screen point.
    MiscMenu(crate::misc_page::M, i32, i32),
    /// A Misc tab keyboard key: key, velocity, pressed.
    Note(u8, u8, bool),
    /// A detached window moved to a monitor with another DPI: the app redraws it at this scale (spec: ui/ui-scaling.md §3).
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    Dpi(f32),
    /// A greyed-out control was clicked: the app says it is not available yet.
    NotAvailable(&'static str),
}

#[derive(Clone, Copy, PartialEq)]
enum Knob {
    Pan,
    Vol,
    Pitch,
}

struct State {
    info: ChannelInfo,
    strip: bool,
    /// The plugin's page tabs, left to right (a plain plugin: the plug).
    pages: Vec<PageIcon>,
    /// The plugin's active page.
    page: usize,
    /// The Misc tab (right of the page tabs) is showing instead of the plugin.
    misc: bool,
    /// The plugin's hint line (parameter under the mouse), shown in the title bar.
    hint: String,
    /// A greyed-out frame control under the mouse: its "not available yet" hint replaces the plugin's.
    off_hint: Option<&'static str>,
    /// The loaded preset's name, shown in the Presets box ("Presets" without one).
    preset: Option<String>,
    /// Misc tab page state (tracking page, keyboard scroll, drag, hover).
    misc_ui: crate::misc_page::MiscUi,
    /// Plugin editor size (logical px).
    editor: (f32, f32),
    events: Vec<FrameEvent>,
    drag: Option<(Knob, f32, f32)>,
    /// Title-bar move in progress: cursor and window position (screen px) at the press. Done by hand: the system's
    /// move loop (HTCAPTION) blocks the UI thread, which would freeze all drawing while a window moves.
    moving: Option<(i32, i32, i32, i32)>,
    /// A title-bar button (close, minimize) pressed and not released yet: it acts on the release over it (owner,
    /// 2026-10-15: acting on the press made accidental closes easy).
    armed: Option<Part>,
    /// A top-level window (menu arrow > Detached) instead of a child of the main window (always when the main window is
    /// a native Wayland one).
    detached: bool,
    /// Docked: the workspace and the rects covering it (the app's windows and popups above this one), in the main
    /// window's client area (physical px). Kept so a move re-clips at once instead of at the app's next tick.
    clip: Option<([i32; 4], Vec<[i32; 4]>)>,
    /// The clip last applied (window rect, workspace, covering rects), so unchanged clips are not reset.
    clip_key: Option<Vec<i32>>,
}

impl State {
    /// A new frame's state: strip shown, the plug tab, no hint or preset.
    fn new(info: ChannelInfo, editor: (f32, f32), detached: bool) -> State {
        State {
            info,
            strip: true,
            pages: vec![PageIcon::Plugin],
            page: 0,
            misc: false,
            hint: String::new(),
            off_hint: None,
            preset: None,
            misc_ui: Default::default(),
            editor,
            events: Vec::new(),
            drag: None,
            moving: None,
            armed: None,
            detached,
            clip: None,
            clip_key: None,
        }
    }


    fn knob_value(&self, k: Knob) -> f32 {
        let i = &self.info;
        match k {
            Knob::Pan => (i.pan as f32 + 1.0) / 2.0,
            Knob::Vol => i.volume as f32,
            Knob::Pitch => 0.5 + i.pitch_cents as f32 / (i.range.max(1) as f32 * 100.0) / 2.0,
        }
    }
    fn set_knob(&mut self, k: Knob, v: f32) {
        let v = v.clamp(0.0, 1.0);
        let e = match k {
            Knob::Pan => FrameEvent::Pan(f64::from(v) * 2.0 - 1.0),
            Knob::Vol => FrameEvent::Volume(f64::from(v)),
            Knob::Pitch => FrameEvent::PitchCents(((v - 0.5) * 2.0 * self.info.range.max(1) as f32 * 100.0).round() as i32),
        };
        match e {
            FrameEvent::Pan(p) => self.info.pan = p,
            FrameEvent::Volume(x) => self.info.volume = x,
            FrameEvent::PitchCents(c) => self.info.pitch_cents = c,
            _ => {}
        }
        self.events.push(e);
    }
    fn size(&self) -> (f32, f32) {
        (self.editor.0 + 2.0 * BORDER, self.area_y() + self.editor.1 + BORDER)
    }
    /// Top of the editor (below the strip, or the title bar, and the editor's 1 px outline).
    fn area_y(&self) -> f32 {
        TITLE_H + if self.strip { STRIP_H } else { 0.0 } + 1.0
    }
}

/// Hit regions of the frame (window coordinates, logical px).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Part {
    Close,
    Minimize,
    Gear,
    /// The menu arrow (the window menu, an app popup).
    Menu,
    /// The Presets box and its previous / next arrows (spec: plugins/presets.md).
    Presets,
    PresetPrev,
    PresetNext,
    /// Page tab `i`; `i == pages.len()` is the Misc tab.
    Tab(usize),
    On,
    Knob(u8),
    /// A strip control that is not available yet (drawn greyed out): its name for the hint.
    Off(&'static str),
    Caption,
}

impl State {
    /// The window belongs to a channel (not a mixer effect): ON, PAN and VOL act on it.
    fn channel(&self) -> bool {
        self.info.misc.is_some()
    }
}

/// Strip controls that do nothing yet, with their rects: the pitch range and mixer track boxes (no editing); ON, PAN, VOL
/// and PITCH too in an effect's window (there is no channel).
fn strip_off(st: &State, w: f32) -> Vec<(Rect, &'static str)> {
    let dx = w - STRIP_RIGHT;
    let mut v = Vec::new();
    if !st.channel() {
        v.push((Rect::new(340.0 + dx, 38.0, 18.0, 18.0), "Effect on / off"));
        v.push((Rect::new(383.0 + dx - 14.0, 33.0, 28.0, 28.0), "Panning"));
        v.push((Rect::new(417.0 + dx - 14.0, 33.0, 28.0, 28.0), "Volume"));
        v.push((Rect::new(455.0 + dx - 14.0, 33.0, 28.0, 28.0), "Channel pitch"));
    }
    v.push((Rect::new(470.0 + dx, 35.0, 28.0, 23.0), "Pitch range"));
    v.push((Rect::new(517.0 + dx, 35.0, 41.0, 23.0), "Mixer track number"));
    v
}

fn parts(st: &State) -> Vec<(Rect, Part)> {
    let (w, _) = st.size();
    let p = presets_rect(w);
    let mut v = vec![
        (Rect::new(w - 23.0, 7.0, 17.0, 17.0), Part::Close),
        (Rect::new(w - 42.0, 7.0, 17.0, 17.0), Part::Minimize),
        (MENU_RECT, Part::Menu),
        (GEAR_RECT, Part::Gear),
        (Rect::new(p.right() - 33.0, p.y, 17.0, p.h), Part::PresetPrev),
        (Rect::new(p.right() - 16.0, p.y, 16.0, p.h), Part::PresetNext),
        (p, Part::Presets),
    ];
    if st.strip {
        let dx = w - STRIP_RIGHT;
        for i in 0..=st.pages.len() {
            v.push((tab_rect(i), Part::Tab(i)));
        }
        v.extend(strip_off(st, w).into_iter().map(|(r, name)| (r, Part::Off(name))));
        if st.channel() {
            v.push((Rect::new(340.0 + dx, 38.0, 18.0, 18.0), Part::On));
            for (i, x) in [383.0, 417.0, 455.0].into_iter().enumerate() {
                v.push((Rect::new(x + dx - 14.0, 33.0, 28.0, 28.0), Part::Knob(i as u8)));
            }
        }
    }
    v.push((Rect::new(0.0, 0.0, w, TITLE_H), Part::Caption));
    v
}

fn part_at(st: &State, x: f32, y: f32) -> Option<Part> {
    parts(st).into_iter().find(|(r, _)| r.contains(x, y)).map(|(_, p)| p)
}

const KNOBS: [Knob; 3] = [Knob::Pan, Knob::Vol, Knob::Pitch];

/// Cell of strip tab `i` (45 px wide from x 7, inside the strip's outline; the Misc tab is the one after the pages).
fn tab_rect(i: usize) -> Rect {
    Rect::new(7.0 + 45.0 * i as f32, TITLE_H + 1.0, 45.0, STRIP_H - 1.0)
}

/// The strip's right-hand controls (ON ... TRACK) are placed for a window this wide and move with its right edge.
const STRIP_RIGHT: f32 = 570.0;

/// The title bar's menu arrow and gear (always in the title bar, whether the strip is shown or not).
const MENU_RECT: Rect = Rect { x: 8.0, y: 5.0, w: 14.0, h: 21.0 };
const GEAR_RECT: Rect = Rect { x: 24.0, y: 5.0, w: 21.0, h: 21.0 };

/// The Presets box: right side of the title bar, left of minimize.
fn presets_rect(w: f32) -> Rect {
    Rect::new(w - 49.0 - PRESETS_W, 6.0, PRESETS_W, 19.0)
}

fn page_tabler(p: PageIcon) -> &'static tb::TablerIcon {
    match p {
        PageIcon::Plugin => &tb::PLUG,
        PageIcon::Sample => &tb::WAVE_SINE,
        PageIcon::Envelope => &tb::TIMELINE,
    }
}

fn icon_at(c: &mut Canvas, t: &tb::TablerIcon, cx: f32, cy: f32, size: f32, col: Color) {
    c.icon(t, Rect::new(cx - size / 2.0, cy - size / 2.0, size, size), col);
}

/// The title bar's text font (FL: 8 px cap height, baseline under row 18).
fn title_font() -> Font {
    Font::new(13)
}

fn paint(c: &mut Canvas, st: &State) {
    let (w, h) = st.size();
    c.begin_frame(None);
    c.clear(c_frame());
    // 1 px outline around the window, a lighter line under its top edge.
    for r in [Rect::new(0.0, 0.0, w, 1.0), Rect::new(0.0, h - 1.0, w, 1.0), Rect::new(0.0, 0.0, 1.0, h), Rect::new(w - 1.0, 0.0, 1.0, h)] {
        c.fill_rect(r, c_outline());
    }
    c.fill_rect(Rect::new(1.0, 1.0, w - 2.0, 1.0), c_top_light());
    title_bar(c, st, w);
    if st.strip {
        strip(c, st, w);
    }
    // The editor's 1 px outline; the plugin's own window covers the inside.
    let ay = st.area_y();
    c.fill_rect(Rect::new(BORDER - 1.0, ay - 1.0, w - 2.0 * BORDER + 2.0, h - ay - BORDER + 2.0), c_editor_line());
    let a = Rect::new(BORDER, ay, w - 2.0 * BORDER, h - ay - BORDER);
    c.fill_rect(a, c_body());
    if st.misc {
        match &st.info.misc {
            Some(v) => crate::misc_page::paint(c, &os_theme::current(), a.x, a.y, v, &st.misc_ui),
            None => c.text_center(Font::new(16), a, c_grey(), "No channel settings here"),
        }
    }
}

/// Title bar, left to right: menu arrow, gear (boxed while the strip is shown), channel name, "(mixer target)", the
/// plugin's hint; right side: Presets box with previous / next arrows, minimize, close.
fn title_bar(c: &mut Canvas, st: &State, w: f32) {
    icon_at(c, &tb::CARET_RIGHT, 14.4, 15.5, 15.4, c_arrow());
    let g = GEAR_RECT;
    let lemon = os_ui::kit::lemon::on(&os_theme::current());
    if st.strip && lemon {
        // Lemon (spec: ui/lemon-design.md §11): the gear is a lit rounded square while the strip shows.
        os_ui::kit::lemon::lit(c, g, 6.0, false);
    } else if st.strip {
        c.stroke_rrect(Rect::new(g.x + 1.0, g.y + 1.0, g.w - 2.0, g.h - 2.0), 3.5, 2.0, c_gear_on());
    }
    let gear = if st.strip && lemon { os_ui::kit::lemon::ink_on_lemon() } else if st.strip { c_gear_on() } else { c_icon() };
    icon_at(c, &tb::SETTINGS_FILLED, g.cx(), g.cy(), 18.4, gear);
    let p = presets_rect(w);
    let f = title_font();
    let ty = (19.0 - c.ascent(f)).round();
    let nx = 48.0;
    let room = (p.x - 8.0 - nx).max(0.0);
    let nw = c.text_width(f, &st.info.name).min(room);
    c.text_clip(f, nx, ty, room, c_text(), &st.info.name);
    let target = format!("({})", st.info.target);
    let tx = nx + nw + c.text_width(f, " ");
    let troom = (p.x - 8.0 - tx).max(0.0);
    c.text_clip(f, tx, ty, troom, c_grey(), &target);
    let tw = c.text_width(f, &target).min(troom);
    let off = st.off_hint.map(crate::app::off_text);
    let hint = if let Some(o) = &off { o } else if st.misc { &st.misc_ui.hint } else { &st.hint };
    if !hint.is_empty() {
        let hx = tx + tw + 14.0;
        let hf = Font::new(12);
        let hy = (19.0 - c.ascent(hf)).round();
        // A tab splits the hint: the part after it sits right-aligned before the Presets box (spec: plugins/host-link.md).
        let (left, right) = hint.split_once('\t').unwrap_or((hint, ""));
        let end = p.x - 8.0;
        let rw = if right.is_empty() { 0.0 } else { c.text_width(hf, right).min((end - hx).max(0.0)) };
        if rw > 0.0 {
            c.text_clip(hf, end - rw, hy, rw, c_caption(), right);
        }
        c.text_clip(hf, hx, hy, (end - rw - 10.0 - hx).max(0.0), c_caption(), left);
    }
    // Presets box: the loaded preset's name, or "Presets" (spec: plugins/presets.md).
    c.fill_rrect(p, if lemon { 6.0 } else { 3.0 }, c_presets());
    c.text_clip(f, p.x + 9.0, ty, p.w - 9.0 - 31.0, c_text(), st.preset.as_deref().unwrap_or("Presets"));
    icon_at(c, &tb::CARET_LEFT, p.right() - 22.2, 15.5, 18.9, c_icon());
    icon_at(c, &tb::CARET_RIGHT, p.right() - 7.8, 15.5, 18.9, c_icon());
    c.icon_stroke(&tb::MINUS, Rect::new(w - 39.5, 7.0, 12.0, 12.0), c_icon(), 2.0);
    c.icon_stroke(&tb::X, Rect::new(w - 22.25, 7.75, 15.5, 15.5), c_icon(), 1.6);
}

fn strip(c: &mut Canvas, st: &State, w: f32) {
    // Strip box with a 1 px outline (its bottom edge is the editor's outline).
    let (dx, dy) = (w - STRIP_RIGHT, TITLE_H - 24.0);
    c.fill_rect(Rect::new(BORDER - 1.0, TITLE_H, w - 2.0 * BORDER + 2.0, STRIP_H), c_strip_line());
    c.fill_rect(Rect::new(BORDER, TITLE_H + 1.0, w - 2.0 * BORDER, STRIP_H - 1.0), c_strip());
    // Page tabs, then the Misc wrench (always the last tab).
    let n = st.pages.len();
    for i in 0..=n {
        let cell = tab_rect(i);
        let active = if i == n { st.misc } else { !st.misc && st.page == i };
        if active && os_ui::kit::lemon::on(&os_theme::current()) {
            c.fill_rrect(cell.inset(2.0), 6.0, c_body());
        } else if active {
            c.fill_rect(cell, c_body());
        }
        let col = if active { os_theme::current().a(0x49f1b1) } else { c_icon() };
        let t = if i == n { &tb::TOOL } else { page_tabler(st.pages[i]) };
        icon_at(c, t, cell.cx(), cell.cy(), 26.0, col);
    }
    // ON LED.
    let (lx, ly) = (349.0 + dx, 43.0 + dy);
    os_ui::kit::led(c, &os_theme::current(), lx, ly, 7.5, os_ui::kit::State::NORMAL.with_on(st.info.enabled));
    caption(c, lx, "ON");
    value_box(c, Rect::new(455.0 + dx, 32.0 + dy, 43.0, 23.0), &st.info.range.to_string());
    for (k, x, cap) in [(Knob::Pan, 383.0, "PAN"), (Knob::Vol, 417.0, "VOL"), (Knob::Pitch, 455.0, "PITCH")] {
        // Indicator dots: pan teal, volume green, pitch red.
        let dot = os_theme::current().a(match k { Knob::Pan => 0x43aebd, Knob::Vol => 0x47f49a, Knob::Pitch => 0xbb465f });
        knob(c, x + dx, 43.0 + dy, st.knob_value(k), k != Knob::Vol, dot);
        caption(c, if k == Knob::Pitch { x + dx - 3.0 } else { x + dx }, cap);
    }
    caption(c, 484.0 + dx, "RANGE");
    let track = if st.info.track == 0 { "---".to_string() } else { st.info.track.to_string() };
    value_box(c, Rect::new(517.0 + dx, 32.0 + dy, 41.0, 23.0), &track);
    caption(c, 537.0 + dx, "TRACK");
    // Greyed out (`strip_off`): RANGE and TRACK; from ON on in an effect's window.
    let x0 = if st.channel() { 470.0 } else { 334.0 };
    c.fill_rect(Rect::span(x0 + dx, 561.0 + dx, TITLE_H + 2.0, TITLE_H + STRIP_H - 1.0), c_strip().with_alpha(crate::app::OFF_VEIL));
}

fn caption(c: &mut Canvas, cx: f32, s: &str) {
    let f = Font::condensed(9, 4).bold();
    let w = c.text_width(f, s);
    c.text(f, (cx - w / 2.0).round(), TITLE_H + 33.0, c_caption(), s);
}

fn knob(c: &mut Canvas, cx: f32, cy: f32, v: f32, bipolar: bool, dot: Color) {
    let r = 11.0;
    let p = os_theme::current();
    if os_ui::kit::lemon::on(&p) {
        let look = os_ui::kit::KnobLook::Mixer { halo: r + 4.0, arc: true };
        return os_ui::kit::knob(c, &p, cx, cy, r, v, bipolar, look, os_ui::kit::State::NORMAL);
    }
    c.fill_circle(cx, cy, r + 4.0, os_theme::current().s(0x191c1f));
    c.fill_circle(cx, cy, r + 3.0, os_theme::current().s(0x23262a));
    c.fill_circle(cx, cy, r + 1.5, os_theme::current().s(0x191c1f));
    c.fill_rect_gradient(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), os_theme::current().s(0x4e5154), os_theme::current().s(0x14171a), r);
    c.fill_rect_gradient(Rect::new(cx - r + 1.6, cy - r + 1.2, 2.0 * r - 3.2, 2.0 * r - 3.2), os_theme::current().s(0x2a2d30), os_theme::current().s(0x1a1d20), r - 1.6);
    let half = 175.0 * PI / 180.0;
    let ang = |v: f32| -half + 2.0 * half * v.clamp(0.0, 1.0);
    let (a, from) = (ang(v), if bipolar { 0.0 } else { ang(0.0) });
    if (a - from).abs() > 0.01 {
        let (s, e) = if a > from { (from, a) } else { (a, from) };
        c.arc(cx, cy, r + 2.5, s - PI / 2.0, e - s, 2.0, c_green());
    }
    c.fill_circle(cx + (r - 5.0) * a.sin(), cy - (r - 5.0) * a.cos(), 1.4, dot);
}

fn value_box(c: &mut Canvas, r: Rect, s: &str) {
    let p = os_theme::current();
    let lemon = os_ui::kit::lemon::on(&p);
    if lemon {
        use os_theme::design::lemon as brand;
        let well = p.brand_lit(brand::COLORED_DARK);
        os_ui::kit::lemon::outlined(c, r, 4.0, well.shade(-0.25), well);
    } else {
        c.fill_rrect(r, 4.0, Color::rgb(0x631d25));
        c.fill_rect_gradient(r.inset(1.0), Color::rgb(0x8d434b), Color::rgb(0x833941), 3.0);
    }
    let f = Font::light(22, 0);
    let w = c.text_width(f, s);
    let lh = c.line_height(f);
    let ink = if lemon { p.brand_lit(os_theme::design::lemon::HIGHLIGHT) } else { Color::rgb(0xe8dfe2) };
    c.text(f, r.right() - 11.0 - w, (r.cy() - lh / 2.0).round(), ink, s);
}

/// `rect` minus `cut`: up to four rects (left, right, top, bottom of the hole), each [left, top, right, bottom].
#[cfg(not(windows))]
fn subtract(rect: [i32; 4], cut: [i32; 4]) -> Vec<[i32; 4]> {
    let [l, t, r, b] = rect;
    let [cl, ct, cr, cb] = [cut[0].max(l), cut[1].max(t), cut[2].min(r), cut[3].min(b)];
    if cl >= cr || ct >= cb {
        return vec![rect];
    }
    [[l, t, cl, b], [cr, t, r, b], [cl, t, cr, ct], [cl, cb, cr, b]].into_iter().filter(|q| q[0] < q[2] && q[1] < q[3]).collect()
}

/// The parts of window rect `me` inside `work` and outside every cover (a docked frame's clip, off Windows).
#[cfg(not(windows))]
fn visible(me: [i32; 4], work: [i32; 4], covers: &[[i32; 4]]) -> Vec<[i32; 4]> {
    let start = [me[0].max(work[0]), me[1].max(work[1]), me[2].min(work[2]), me[3].min(work[3])];
    if start[0] >= start[2] || start[1] >= start[3] {
        return Vec::new();
    }
    covers.iter().fold(vec![start], |parts, &c| parts.into_iter().flat_map(|p| subtract(p, c)).collect())
}

// ---------------------------------------------------------------------------------------------------------------
// Input: what a press, a move, a release or the wheel does to the frame. The native window turns its events into these
// calls (logical px, window coordinates) and does what the `Reply` asks.

/// What the native window does after an input event.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Reply {
    /// Nothing.
    None,
    /// Repaint the frame.
    Repaint,
    /// Lay the frame out again (its size, the editor area shown or hidden), then repaint.
    Relayout,
    /// Repaint, and keep the mouse until the button is released (a knob or Misc drag, a held Misc key).
    Capture,
    /// Start moving the window by its title bar.
    Move,
    /// Minimize the window (a docked one hides).
    Minimize,
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Left,
    Right,
    Middle,
}

/// Window point (logical px) to screen point (physical px), for the app's popups.
type ToScreen<'a> = &'a dyn Fn(f32, f32) -> (i32, i32);

impl State {
    /// Left button down at (x, y); `double` for the second press of a double click.
    fn left_down(&mut self, x: f32, y: f32, double: bool, screen: ToScreen) -> Reply {
        let part = part_at(self, x, y);
        match part {
            Some(Part::Menu) => {
                // The app draws the menu (its own popup, like every other Lemon Studio menu), below the arrow.
                let m = MENU_RECT;
                let (sx, sy) = screen(m.x, m.bottom());
                self.events.push(FrameEvent::Menu(sx, sy));
                os_clap::wake();
                return Reply::None;
            }
            Some(Part::Presets) => {
                // The preset list is an app popup too, under the box's left edge.
                let b = presets_rect(self.size().0);
                let (sx, sy) = screen(b.x, b.bottom());
                self.events.push(FrameEvent::Presets(sx, sy));
                os_clap::wake();
                return Reply::None;
            }
            Some(Part::PresetPrev | Part::PresetNext) => {
                self.events.push(FrameEvent::PresetStep(if part == Some(Part::PresetPrev) { -1 } else { 1 }));
                os_clap::wake();
                return Reply::None;
            }
            None => {
                if let Some(r) = self.misc_button(x, y, Btn::Left, screen) {
                    return r;
                }
            }
            _ => {}
        }
        match part {
            // Close and minimize only arm on the press; the release over the same button acts (`left_up`).
            Some(Part::Close | Part::Minimize) => {
                self.armed = part;
                Reply::Capture
            }
            Some(Part::Gear) => {
                self.strip = !self.strip;
                if !self.strip {
                    self.misc = false;
                }
                Reply::Relayout
            }
            Some(Part::Tab(t)) if t == self.pages.len() => {
                self.misc = true;
                Reply::Relayout
            }
            Some(Part::Tab(t)) => {
                self.misc = false;
                self.page = t;
                self.events.push(FrameEvent::Page(t));
                Reply::Relayout
            }
            Some(Part::On) => {
                self.info.enabled = !self.info.enabled;
                self.events.push(FrameEvent::ToggleOn);
                Reply::Repaint
            }
            Some(Part::Knob(i)) => {
                let k = KNOBS[usize::from(i)];
                self.drag = Some((k, y, self.knob_value(k)));
                Reply::Capture
            }
            Some(Part::Off(name)) => {
                self.events.push(FrameEvent::NotAvailable(name));
                os_clap::wake();
                Reply::Repaint
            }
            // Drag the window by the title bar.
            Some(Part::Caption) if !double => Reply::Move,
            _ => Reply::Repaint,
        }
    }

    /// Right button down: the Misc page's lists and root note.
    fn right_down(&mut self, x: f32, y: f32, screen: ToScreen) -> Reply {
        self.misc_button(x, y, Btn::Right, screen).unwrap_or(Reply::None)
    }

    /// Middle button down resets a knob (spec: ui/knobs.md, owner decision).
    fn middle_down(&mut self, x: f32, y: f32, screen: ToScreen) -> Reply {
        if let Some(r) = self.misc_button(x, y, Btn::Middle, screen) {
            return r;
        }
        if let Some(Part::Knob(i)) = part_at(self, x, y) {
            let k = KNOBS[usize::from(i)];
            let d = if k == Knob::Vol { 0.78125 } else { 0.5 };
            self.set_knob(k, d);
            return Reply::Repaint;
        }
        Reply::None
    }

    /// The mouse moved to (x, y) (a window move by the title bar is the native window's). `fine`: Ctrl is held.
    fn motion(&mut self, x: f32, y: f32, fine: bool) -> Reply {
        let mut hover = false;
        if self.misc {
            if self.misc_ui.drag.is_some() {
                let ay = self.area_y();
                if let Some((m, v)) = crate::misc_page::drag_value(&self.misc_ui, x - BORDER, y - ay, fine) {
                    if self.info.misc.is_some_and(|vals| vals.get(m) != v) {
                        misc_set(self, m, v);
                        os_clap::wake();
                        return Reply::Repaint;
                    }
                }
                return Reply::None;
            }
            if let Some(old) = self.misc_ui.held {
                // Dragging along the keyboard lets go of the held key and sounds each key entered.
                match misc_hit(self, x, y) {
                    Some(crate::misc_page::Hit::Key(k)) if k != old => {
                        self.misc_ui.held = Some(k);
                        self.events.push(FrameEvent::Note(old, 0, false));
                        self.events.push(FrameEvent::Note(k, 100, true));
                        os_clap::wake();
                        return Reply::Repaint;
                    }
                    _ => return Reply::None,
                }
            }
            if self.drag.is_none() && self.moving.is_none() {
                let h = misc_hit(self, x, y);
                if h != self.misc_ui.hover {
                    self.misc_ui.hover = h;
                    misc_hint(self, h);
                    hover = true;
                }
            }
        }
        if let Some((k, y0, v0)) = self.drag {
            self.set_knob(k, v0 + (y0 - y) / 1024.0);
            return Reply::Repaint;
        }
        // Over a greyed-out control the title bar says it is not available yet.
        let off = match part_at(self, x, y) {
            Some(Part::Off(name)) => Some(name),
            _ => None,
        };
        if off != self.off_hint {
            self.off_hint = off;
            return Reply::Repaint;
        }
        if hover { Reply::Repaint } else { Reply::None }
    }

    /// The left button was released at (x, y): an armed title-bar button acts if the mouse is still over it, then
    /// everything a release ends ends (`release`).
    fn left_up(&mut self, x: f32, y: f32) -> Reply {
        let armed = self.armed.take();
        let over = armed.is_some() && part_at(self, x, y) == armed;
        self.release();
        match armed {
            Some(Part::Close) if over => {
                self.events.push(FrameEvent::Close);
                Reply::Repaint
            }
            Some(Part::Minimize) if over => Reply::Minimize,
            Some(_) => Reply::Repaint,
            None => Reply::None,
        }
    }

    /// The button was released (or the mouse lost otherwise): drags end, a held Misc key is let go, an armed title-bar
    /// button is let go without acting.
    fn release(&mut self) {
        self.armed = None;
        self.drag = None;
        self.moving = None;
        self.misc_ui.drag = None;
        if let Some(k) = self.misc_ui.held.take() {
            self.events.push(FrameEvent::Note(k, 0, false));
            os_clap::wake();
        }
    }

    /// The wheel turned `notches` (up positive) over (x, y).
    fn wheel(&mut self, x: f32, y: f32, notches: f32) -> Reply {
        let part = part_at(self, x, y);
        if part.is_none() {
            if let Some(r) = self.misc_wheel(x, y, notches) {
                return r;
            }
        }
        if matches!(part, Some(Part::Presets | Part::PresetPrev | Part::PresetNext)) && notches != 0.0 {
            // Wheel down = next preset.
            self.events.push(FrameEvent::PresetStep(if notches < 0.0 { 1 } else { -1 }));
            os_clap::wake();
            return Reply::None;
        }
        if let Some(Part::Knob(i)) = part {
            let k = KNOBS[usize::from(i)];
            let v = self.knob_value(k) + notches / 64.0;
            self.set_knob(k, v);
            return Reply::Repaint;
        }
        Reply::None
    }

    /// A button press over the Misc page; `None` when the page does not take it.
    fn misc_button(&mut self, x: f32, y: f32, b: Btn, screen: ToScreen) -> Option<Reply> {
        use crate::misc_page::{self as mp, Hit, M};
        let h = misc_hit(self, x, y)?;
        let vals = self.info.misc?;
        // Greyed-out controls only say so.
        if let Some(name) = mp::off_name(h, &vals) {
            if b != Btn::Middle {
                self.events.push(FrameEvent::NotAvailable(name));
                os_clap::wake();
            }
            return Some(Reply::None);
        }
        let mut reply = Reply::Repaint;
        match (b, h) {
            (Btn::Middle, Hit::Value(m)) => misc_set(self, m, mp::default_of(m)),
            (Btn::Right, Hit::Value(m) | Hit::Step(m, _)) if mp::list_on_right(m) => {
                let (sx, sy) = screen(x, y);
                self.events.push(FrameEvent::MiscMenu(m, sx, sy));
            }
            (Btn::Right, Hit::Key(k)) => misc_set(self, M::Root, f32::from(k)),
            (Btn::Left, Hit::Value(m)) if mp::list_on_click(m) => {
                let (sx, sy) = screen(x, y);
                self.events.push(FrameEvent::MiscMenu(m, sx, sy));
            }
            (Btn::Left, Hit::Value(m)) if mp::draggable(m) => {
                self.misc_ui.drag = Some((m, mp::drag_start(m, x - BORDER, y - self.area_y()), vals.get(m)));
                reply = Reply::Capture;
            }
            (Btn::Left, Hit::Value(m)) => {
                if let Some(v) = mp::click_value(m, vals.get(m)) {
                    misc_set(self, m, v);
                }
            }
            (Btn::Left, Hit::Step(m, d)) => {
                let (lo, hi) = mp::range_of(m);
                misc_set(self, m, (vals.get(m) + d as f32).clamp(lo, hi));
            }
            (Btn::Left, Hit::ArpDir(i)) => misc_set(self, M::ArpDir, f32::from(i)),
            (Btn::Left, Hit::TrackTab(k)) => self.misc_ui.track_key = k,
            (Btn::Left, Hit::CutSelf) => {
                // The channel cuts itself: cut and cut-by the same group (spec: ui/plugin-window.md "Misc tab").
                let g = if vals.get(M::Cut) >= 1.0 { vals.get(M::Cut) } else { 1.0 };
                misc_set(self, M::Cut, g);
                misc_set(self, M::CutBy, g);
            }
            (Btn::Left, Hit::Reset) => {
                misc_set(self, M::Root, 60.0);
                misc_set(self, M::Fine, 0.0);
            }
            (Btn::Left, Hit::Key(k)) => {
                self.misc_ui.held = Some(k);
                self.events.push(FrameEvent::Note(k, 100, true));
                reply = Reply::Capture;
            }
            _ => {}
        }
        os_clap::wake();
        Some(reply)
    }

    /// The wheel over the Misc page; `None` when the page does not take it.
    fn misc_wheel(&mut self, x: f32, y: f32, notches: f32) -> Option<Reply> {
        use crate::misc_page::{self as mp, Hit};
        let h = misc_hit(self, x, y)?;
        if self.info.misc.is_some_and(|v| mp::off_name(h, &v).is_some()) {
            return Some(Reply::None);
        }
        match h {
            Hit::Value(m) | Hit::Step(m, _) => {
                let v = self.info.misc.map(|vals| vals.get(m))?;
                if let Some(nv) = mp::wheel_value(m, v, notches) {
                    misc_set(self, m, nv);
                    os_clap::wake();
                }
            }
            Hit::Key(_) | Hit::Keyboard => mp::scroll_keyboard(&mut self.misc_ui, notches),
            _ => {}
        }
        Some(Reply::Repaint)
    }
}

/// The Misc page hit under frame point (x, y), when the Misc tab shows.
fn misc_hit(st: &State, x: f32, y: f32) -> Option<crate::misc_page::Hit> {
    if !st.misc || st.info.misc.is_none() {
        return None;
    }
    let (ax, ay) = (BORDER, st.area_y());
    crate::misc_page::hit(&st.misc_ui, x - ax, y - ay)
}

/// Sets a Misc value on the page at once and tells the app.
fn misc_set(st: &mut State, m: crate::misc_page::M, v: f32) {
    if let Some(vals) = st.info.misc.as_mut() {
        vals.set(m, v);
    }
    st.events.push(FrameEvent::Misc(m, v));
    misc_hint(st, Some(crate::misc_page::Hit::Value(m)));
}

fn misc_hint(st: &mut State, h: Option<crate::misc_page::Hit>) {
    use crate::misc_page::{note_name, off_name, value_text, Hit, M};
    if let Some(name) = h.zip(st.info.misc).and_then(|(h, v)| off_name(h, &v)) {
        st.misc_ui.hint = crate::app::off_text(name);
        return;
    }
    st.misc_ui.hint = match (h, &st.info.misc) {
        (Some(Hit::Value(m) | Hit::Step(m, _)), Some(v)) => format!("{}: {}", m.label(), value_text(m, v.get(m))),
        (Some(Hit::ArpDir(i)), _) => format!("Arpeggio direction: {}", value_text(M::ArpDir, f32::from(i))),
        (Some(Hit::Key(k)), _) => format!("{} (right click sets the root note)", note_name(i32::from(k))),
        (Some(Hit::CutSelf), _) => "Cut itself: a new note cuts the channel's playing notes".into(),
        (Some(Hit::Reset), _) => "Reset the root note and fine tuning".into(),
        (Some(Hit::TrackTab(k)), _) => (if k { "Key tracking" } else { "Velocity tracking" }).into(),
        _ => String::new(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(strip: bool, target: &str) -> State {
        let info = ChannelInfo { name: "Keys".into(), target: target.into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 0, misc: Some(Default::default()) };
        State { strip, ..State::new(info, (480.0, 270.0), false) }
    }

    /// "Open detached plugins with the main window": a detached window opening for the first time keeps its place
    /// relative to the monitor it would open on, moved to the chosen monitor and kept on it (spec:
    /// ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup").
    #[test]
    fn first_detached_window_moves_between_monitors() {
        let primary = [0, 0, 1920, 1040];
        let second = [1920, -200, 4480, 1240];
        assert_eq!(moved_to((2340, -40), second, second, (200, 100)), (2340, -40), "the same monitor: unchanged");
        assert_eq!(moved_to((2340, -40), second, primary, (200, 100)), (420, 160));
        assert_eq!(moved_to((4400, 1200), second, primary, (200, 100)), (1720, 940), "kept on the smaller monitor");
    }

    #[test]
    fn frame_geometry() {
        // spec: ui/plugin-window.md "Frame": 7 px around the editor, 27 px title bar, 46 px strip.
        let st = state(true, "Master");
        assert_eq!(st.size(), (494.0, 74.0 + 270.0 + 7.0));
        assert_eq!(state(false, "Master").size().1, 28.0 + 270.0 + 7.0);
        let at = |x, y| part_at(&st, x, y);
        assert_eq!(at(15.0, 15.0), Some(Part::Menu));
        assert_eq!(at(34.0, 15.0), Some(Part::Gear));
        assert_eq!(at(390.0, 15.0), Some(Part::Presets));
        assert_eq!(at(422.0, 15.0), Some(Part::PresetPrev));
        assert_eq!(at(438.0, 15.0), Some(Part::PresetNext));
        assert_eq!(at(460.0, 15.0), Some(Part::Minimize));
        assert_eq!(at(479.0, 15.0), Some(Part::Close));

        // Close and minimize act on the release over them, not on the press; sliding off, or losing the mouse,
        // cancels.
        let mut st = state(true, "Master");
        let screen = |_: f32, _: f32| (0, 0);
        let closes = |st: &State| st.events.iter().filter(|e| matches!(e, FrameEvent::Close)).count();
        assert_eq!(st.left_down(479.0, 15.0, false, &screen), Reply::Capture);
        assert_eq!(closes(&st), 0, "the press does not close");
        assert_eq!(st.left_up(200.0, 15.0), Reply::Repaint);
        assert_eq!(closes(&st), 0, "released off the button");
        st.left_down(479.0, 15.0, false, &screen);
        st.release();
        assert_eq!(st.left_up(479.0, 15.0), Reply::None, "the mouse was lost first");
        st.left_down(479.0, 15.0, false, &screen);
        assert_eq!(st.left_up(479.0, 15.0), Reply::Repaint);
        assert_eq!(closes(&st), 1);
        st.left_down(460.0, 15.0, false, &screen);
        assert_eq!(st.left_up(479.0, 15.0), Reply::Repaint, "pressed on minimize, released on close: nothing");
        assert_eq!(closes(&st), 1);
        st.left_down(460.0, 15.0, false, &screen);
        assert_eq!(st.left_up(460.0, 15.0), Reply::Minimize);
        assert_eq!(at(200.0, 15.0), Some(Part::Caption));
        assert_eq!(at(29.0, 50.0), Some(Part::Tab(0)));
        assert_eq!(at(74.0, 50.0), Some(Part::Tab(1)));
    }

    /// Renders the frame offscreen (`OS_SHOT_DIR=<dir> cargo test -p os-app frame_shots -- --ignored`).
    #[test]
    #[ignore]
    fn frame_shots() {
        let Some(dir) = std::env::var_os("OS_SHOT_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        let misc = |sampler: bool| {
            let mut st = state(true, "Insert 1");
            st.misc = true;
            st.editor = (crate::misc_page::PAGE_W, crate::misc_page::PAGE_H);
            st.info.misc = Some(crate::misc_page::MiscVals { sampler, ..Default::default() });
            st
        };
        let effect = || {
            let mut st = state(true, "Insert 1");
            st.info.misc = None;
            st
        };
        for (name, st) in [
            ("frame_strip.png", state(true, "Master")),
            ("frame_bare.png", state(false, "Master")),
            ("frame_effect.png", effect()),
            ("frame_misc.png", misc(true)),
            ("frame_misc_plugin.png", misc(false)),
        ] {
            let (w, h) = st.size();
            let mut c = Canvas::new(w as u32, h as u32, 1.0).unwrap();
            paint(&mut c, &st);
            c.pixmap.save_png(dir.join(name)).unwrap();
        }
        // The Misc page again in the Lemon design.
        os_theme::set_current(&os_theme::design::palette(os_theme::Design::Lemon, None));
        for (name, st) in [("frame_misc_lemon.png", misc(true)), ("frame_misc_plugin_lemon.png", misc(false))] {
            let (w, h) = st.size();
            let mut c = Canvas::new(w as u32, h as u32, 1.0).unwrap();
            paint(&mut c, &st);
            c.pixmap.save_png(dir.join(name)).unwrap();
        }
        os_theme::set_current(&os_theme::Palette::default());
    }

    /// Strip controls that do nothing yet are greyed out: RANGE / TRACK, and ON / PAN / VOL / PITCH of an effect.
    #[test]
    fn dead_strip_controls_are_off() {
        let st = state(true, "Master");
        let (w, _) = st.size();
        let dx = w - STRIP_RIGHT;
        assert_eq!(part_at(&st, 383.0 + dx, 47.0), Some(Part::Knob(0)), "PAN works");
        assert_eq!(part_at(&st, 417.0 + dx, 47.0), Some(Part::Knob(1)), "VOL works");
        assert_eq!(part_at(&st, 349.0 + dx, 47.0), Some(Part::On));
        assert_eq!(part_at(&st, 455.0 + dx, 47.0), Some(Part::Knob(2)), "PITCH works");
        assert_eq!(part_at(&st, 485.0 + dx, 47.0), Some(Part::Off("Pitch range")));
        assert_eq!(part_at(&st, 537.0 + dx, 47.0), Some(Part::Off("Mixer track number")));
        let mut fx = state(true, "Master");
        fx.info.misc = None;
        assert_eq!(part_at(&fx, 383.0 + dx, 47.0), Some(Part::Off("Panning")));
        assert_eq!(part_at(&fx, 349.0 + dx, 47.0), Some(Part::Off("Effect on / off")));
    }

    /// A Misc keyboard key sounds while the button is held: sliding to the next key lets go of it and sounds that one,
    /// the release lets go of the last.
    #[test]
    fn misc_keys_sound_while_held() {
        use crate::misc_page::Hit;
        let mut st = state(true, "Insert 1");
        st.misc = true;
        st.editor = (crate::misc_page::PAGE_W, crate::misc_page::PAGE_H);
        let (w, h) = st.size();
        let find = |st: &State, k: u8| {
            (0..w as i32).flat_map(|x| (0..h as i32).map(move |y| (x as f32, y as f32))).find(|&(x, y)| misc_hit(st, x, y) == Some(Hit::Key(k))).unwrap()
        };
        let (c, cs) = (find(&st, 60), find(&st, 61));
        let screen = |_: f32, _: f32| (0, 0);
        assert_eq!(st.left_down(c.0, c.1, false, &screen), Reply::Capture);
        assert_eq!(st.motion(c.0, c.1, false), Reply::None, "the held key keeps sounding");
        assert_eq!(st.motion(cs.0, cs.1, false), Reply::Repaint);
        st.release();
        let notes: Vec<(u8, bool)> = st.events.iter().filter_map(|e| if let FrameEvent::Note(k, _, on) = e { Some((*k, *on)) } else { None }).collect();
        assert_eq!(notes, [(60, true), (60, false), (61, true), (61, false)]);
        assert_eq!(st.misc_ui.held, None);
    }
}
