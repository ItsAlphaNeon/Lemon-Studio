// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Views of the main window, laid out from the measured default layout.
// spec: ui/main-window.md

pub mod browser;
pub mod cabledebug;
pub mod channel_win;
pub mod dialog;
pub mod menus;
pub mod mondebug;
pub mod settings;
pub mod mixer;
pub mod piano;
pub mod patterns;
pub mod perf;
pub mod playlist;
pub mod rack;
pub mod tapper;
pub mod toolbar;
pub mod typein;

use std::borrow::Cow;

use os_ui::{kit, Color, Rect};

use crate::app::{Action, Bar, Scroll, Seg, State, Ui, Win};
use crate::icons::{self, Icon};

/// Per-frame values that don't live in `State`.
pub struct FrameInfo {
    pub title: String,
    pub hint: Cow<'static, str>,
    pub playing: bool,
    /// Playing but held by Play (pause).
    pub paused: bool,
    /// Playback position in ticks of the running mode (a pattern wraps at its end), and whether that mode is
    /// the playlist. Every playhead and the time display read these, never the UI's own tempo or mode.
    pub tick: f64,
    pub song: bool,
    pub peaks: (f32, f32),
    /// Notes playing now, as (channel index, key).
    pub sounding: Vec<(u16, u16)>,
    /// The song position slider's handle is being dragged (it shows the dragged position, not the playhead).
    pub song_pos_drag: bool,
}

/// UI text at a nominal size (Cuprum; spec §10).
pub fn fnt(size: f32) -> os_ui::Font {
    os_ui::Font::new(size.round() as u16)
}

/// The light display face (song position).
pub fn light(size: f32) -> os_ui::Font {
    os_ui::Font::light(size.round() as u16, 0)
}

/// Top of the workspace (§1).
pub const WORK_Y: f32 = 83.0;
/// Width of the docked browser incl. the splitter (§4).
pub const BROWSER_W: f32 = 209.0;

/// Paints the frame; returns the number of hits registered before the overlays (menus, popups, picker...), which cover
/// docked plugin windows.
pub fn paint(ui: &mut Ui, s: &State, f: &FrameInfo) -> usize {
    let (w, h) = ui.c.size();
    // A maximized Playlist or Piano roll paints all of its rect (`framed`): the workspace under it needs no clear.
    let cover = s.docked().filter(|w| matches!(w, Win::Playlist | Win::PianoRoll)).map(|w| s.win_rect(w));
    ui.c.set_cover(cover);
    ui.c.clear(ui.t.workspace);
    ui.c.set_cover(None);

    // Workspace (§9): watermark, then docked windows on top.
    let left = if s.show_browser { BROWSER_W } else { 0.0 };
    let work = Rect::new(left, WORK_Y, w - left, h - WORK_Y);
    let docked = s.docked();
    if docked.is_none() {
        let wm = ui.t.watermark;
        icons::logo(ui.c, work.cx(), work.cy() + 40.0, 170.0, wm);
        let version = concat!("Lemon Studio v", env!("CARGO_PKG_VERSION"), " - pre-alpha");
        let vc = ui.t.version_text;
        let vw = ui.c.text_width(crate::views::fnt(16.0), version);
        ui.c.text(crate::views::fnt(16.0), work.cx() - vw / 2.0, h - 22.0, vc, version);
    }

    // One stacking order for all child windows (spec: ui/channel-rack.md §5). A maximized window
    // fills the workspace, so windows below it are covered and not painted.
    let docked_at = s.z.iter().rposition(|&w| s.is_max(w));
    // Each part is skipped by a repaint that misses it (`Ui::part`).
    if s.show_browser {
        let r = Rect::new(0.0, WORK_Y, BROWSER_W, h - WORK_Y);
        ui.part(Seg::Browser, r, |ui| browser::paint(ui, s, r));
    }
    let _ = work;
    for (i, &win) in s.z.iter().enumerate() {
        if docked_at.is_some_and(|d| i < d) {
            continue;
        }
        ui.win = Some(win);
        let r = s.win_rect(win);
        // A window paints inside its paint area (also what it marks as drawn, e.g. text's generous overhang).
        let a = paint_area(s, win);
        ui.c.push_clip(a);
        match win {
            Win::Playlist => ui.part(Seg::Win(win), a, |ui| playlist::paint(ui, s, r, f)),
            Win::Mixer => ui.part(Seg::Win(win), a, |ui| {
                // Timed while the cables swing: a mixer too slow to redraw them hangs them still.
                let swinging = s.cables.borrow().moving().then(std::time::Instant::now);
                mixer::paint(ui, s, r, f);
                if let Some(t) = swinging {
                    s.cables.borrow_mut().paint_cost(t.elapsed());
                }
            }),
            Win::PianoRoll => ui.part(Seg::Win(win), a, |ui| piano::paint(ui, s, r, f)),
            Win::ChannelRack => ui.part(Seg::Win(win), a, |ui| rack::paint(ui, s, f)),
            Win::Plugin => ui.part(Seg::Win(win), a, |ui| channel_win::paint(ui, s, f)),
            Win::Perf => ui.part(Seg::Win(win), a, |ui| perf::paint(ui, s, f)),
            Win::Tapper => ui.part(Seg::Win(win), a, |ui| tapper::paint(ui, s, f)),
            _ => {}
        }
        ui.c.pop_clip();
        ui.win = None;
    }
    ui.part(Seg::Toolbar, Rect::new(0.0, 0.0, w, WORK_Y), |ui| toolbar::paint(ui, s, f, w));
    let overlays = ui.hit_count();
    // A menu opened from the settings window (Audio page combos) goes over it.
    if !s.settings.open {
        playlist::paint_overlays(ui, s);
    }
    if let Some(m) = s.menu_open {
        menus::paint_dropdown(ui, s, m);
    }
    rack::paint_fill(ui, s);
    patterns::paint(ui, s);
    if s.settings.open {
        settings::paint(ui, s);
        playlist::paint_overlays(ui, s);
    }
    browser::paint_ghost(ui, s);
    if s.monitor.debug {
        mondebug::paint(ui, s);
    }
    if s.cables.borrow().debug {
        cabledebug::paint(ui, s);
    }
    // Message boxes and file dialogs go over everything (spec: ui/dialogs.md).
    dialog::paint(ui, s);
    overlays
}

/// Where window `w` paints: its rect (and the channel rack's graph editor hanging below it). The resize zones past
/// its edges are hits only. A repaint that misses this skips the window; a playlist scroll paints around it.
pub fn paint_area(s: &State, w: Win) -> Rect {
    match w {
        Win::ChannelRack => rack::paint_area(s),
        Win::Plugin => channel_win::rect(s),
        Win::Perf => perf::bounds(s),
        Win::Tapper => tapper::bounds(s),
        w => s.win_rect(w),
    }
}

/// Start of a Playlist / Mixer / Piano roll window: blocks clicks through it, and (once the
/// content is painted, so the zones sit on top) `resize_zones` adds its resize edges.
pub fn window_body(ui: &mut Ui, r: Rect) {
    ui.hit(r, Action::Swallow, "");
}

/// Resize edges of a floating window: thin strips along the sides and small corner squares,
/// reaching 3 px outside the window. Not for maximized or collapsed windows.
pub fn resize_zones(ui: &mut Ui, s: &State, win: Win, r: Rect) {
    let Some(i) = win.slot() else { return };
    if s.wins[i].maxed || s.wins[i].mini {
        return;
    }
    use crate::app::{EDGE_B, EDGE_L, EDGE_R, EDGE_T};
    let (o, n, c) = (3.0, 2.0, 8.0);
    let hint = "Drag to resize";
    let (x0, x1, y0, y1) = (r.x, r.right(), r.y, r.bottom());
    for (z, e) in [
        (Rect::span(x0 - o, x0 + n, y0 + c, y1 - c), EDGE_L),
        (Rect::span(x1 - n, x1 + o, y0 + c, y1 - c), EDGE_R),
        (Rect::span(x0 + c, x1 - c, y0 - o, y0 + n), EDGE_T),
        (Rect::span(x0 + c, x1 - c, y1 - n, y1 + o), EDGE_B),
        (Rect::span(x0 - o, x0 + c, y0 - o, y0 + c), EDGE_L | EDGE_T),
        (Rect::span(x1 - c, x1 + o, y0 - o, y0 + c), EDGE_R | EDGE_T),
        (Rect::span(x0 - o, x0 + c, y1 - c, y1 + o), EDGE_L | EDGE_B),
        (Rect::span(x1 - c, x1 + o, y1 - c, y1 + o), EDGE_R | EDGE_B),
    ] {
        ui.hit(z, Action::WinResize(win, e), hint);
    }
}

/// Minimize / maximize / close buttons from `x0` at the title bar of a window at top `y`.
pub fn window_buttons(ui: &mut Ui, win: Win, x0: f32, y: f32, _normal: Color, _hover: Color) {
    let mut hovered = None;
    for (i, (hint, action)) in [
        ("Minimize window", Action::MinWin(win)),
        ("Maximize / restore window", Action::MaxWin(win)),
        ("Close window", Action::CloseWin(win)),
    ]
    .into_iter()
    .enumerate()
    {
        if ui.hit(Rect::new(x0 + i as f32 * 19.5, y + 3.0, 17.0, 17.0), action, hint) {
            hovered = Some(i);
        }
    }
    kit::window_buttons(ui.c, ui.t, x0, y, hovered);
}

/// Window frame with a title bar (§5); the bar drags the window. `buttons` = draw the window
/// buttons at the right (the Mixer draws its own over the slot panel). Returns the client rect.
pub fn window_frame(ui: &mut Ui, win: Win, r: Rect, title: &str, title_x: f32, buttons: bool) -> Rect {
    window_frame_ex(ui, win, r, title, title_x, buttons, false)
}

/// `window_frame` with `flat`: a flat title bar without the separator (inside a floating frame).
pub fn window_frame_ex(ui: &mut Ui, win: Win, r: Rect, title: &str, title_x: f32, buttons: bool, flat: bool) -> Rect {
    let t = ui.t;
    ui.c.fill_rect(r, t.window_title);
    let bar = Rect::new(r.x, r.y, r.w, 23.0);
    if kit::lemon::on(t) {
        // Lemon (spec: ui/lemon-design.md §3): a flat bar, a hairline under it.
        if !flat {
            ui.c.fill_rect(Rect::new(r.x, r.y + 22.0, r.w, 1.0), t.splitter_dark);
        }
    } else if !flat {
        ui.c.fill_rect_gradient(bar, t.window_title.shade(0.04), t.window_title.shade(-0.03), 0.0);
        ui.c.fill_rect(Rect::new(r.x, r.y + 22.0, r.w, 1.0), t.splitter_dark);
    }
    ui.c.text(os_ui::Font::new(17), r.x + title_x, r.y + 1.0, t.text, title);
    ui.hit(bar, Action::WinDrag(win), "Drag to move");
    // Minimize / maximize / close glyphs at the right (1862, 1881, 1901 at full width).
    if buttons {
        window_buttons(ui, win, r.right() - 64.0, r.y, t.text_dim.mix(t.glyph, 0.6), t.glyph);
    }
    Rect::new(r.x, r.y + 23.0, r.w, r.h - 23.0)
}

/// Floating window frame (spec ui/piano-roll.md §1a, same for Playlist / Mixer / Piano roll):
/// 1 px dark border, 5 px frame, 1 px inner line around the client, flat 28 px title bar.
/// Returns the inner rect (laid out like the docked window) when `win` floats, else `None`.
pub fn float_frame(ui: &mut Ui, s: &State, win: Win, r: Rect) -> Option<Rect> {
    let ws = &s.wins[win.slot()?];
    if ws.maxed || ws.mini || r.h < 2.0 * crate::app::TITLE_H {
        return None;
    }
    let t = ui.t;
    let inner = Rect::new(r.x + 7.0, r.y + 5.0, r.w - 14.0, r.h - 12.0);
    if kit::lemon::on(t) {
        kit::float_frame(ui.c, t, r);
    } else {
        ui.c.fill_rect(r, ui.t.s(0x191d1f));
        ui.c.fill_rect(r.inset(1.0), t.window_title);
        ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, 1.0), ui.t.s(0x303436));
        ui.c.fill_rect(Rect::new(inner.x - 1.0, inner.y + 22.0, inner.w + 2.0, inner.h - 21.0), ui.t.s(0x131718));
    }
    ui.hit(Rect::new(r.x, r.y, r.w, 28.0), Action::WinDrag(win), "Drag to move");
    Some(inner)
}

/// The (layout rect, client rect) that `framed` returns for `win` at `r`, without painting.
pub fn frame_rects(s: &State, win: Win, r: Rect) -> (Rect, Rect) {
    let floats = win.slot().is_some_and(|i| !s.wins[i].maxed && !s.wins[i].mini) && r.h >= 2.0 * crate::app::TITLE_H;
    let r = if floats { Rect::new(r.x + 7.0, r.y + 5.0, r.w - 14.0, r.h - 12.0) } else { r };
    (r, Rect::new(r.x, r.y + 23.0, r.w, r.h - 23.0))
}

/// Window frame for `win`: the floating frame when it floats, else the docked title bar.
/// Returns (layout rect, client rect).
pub fn framed(ui: &mut Ui, s: &State, win: Win, r: Rect, title: &str, title_x: f32, buttons: bool) -> (Rect, Rect) {
    match float_frame(ui, s, win, r) {
        Some(inner) => (inner, window_frame_ex(ui, win, inner, title, title_x, buttons, true)),
        None => (r, window_frame(ui, win, r, title, title_x, buttons)),
    }
}

/// A scroll bar over `track`: thumb sized `view / total`, positioned by `pos` (content pixels).
/// Registers the bar for dragging (see `App::scrollbar_press`).
pub fn scrollbar(ui: &mut Ui, track: Rect, vertical: bool, pos: f32, view: f32, total: f32, target: Scroll) {
    let len = if vertical { track.h } else { track.w };
    let total = total.max(view).max(1.0);
    let max = (total - view).max(0.0);
    let (_, thumb) = os_ui::kit::thumb(len, pos, view, total);
    let zooms = matches!(target, Scroll::PianoX | Scroll::PlaylistX);
    let hint = if zooms { "Drag to scroll - right-drag: left/right scrolls, up/down zooms" } else { "Drag to scroll" };
    let act = Action::Scrollbar(target);
    let hov = ui.hit(track, act.clone(), hint);
    let st = kit::State::new(hov, ui.down(track, &act), false);
    kit::scrollbar(ui.c, ui.t, track, vertical, pos, view, total, st);
    ui.bars.push(Bar { target, track, vertical, max, view, thumb });
}

/// Raised surface: soft vertical gradient (lighter at the top) so panels don't look flat.
pub fn raised(ui: &mut Ui, r: Rect, base: Color, radius: f32) {
    kit::raised_in(ui.c, ui.t, r, base, radius);
}

/// Whether the Lemon design draws (spec: ui/lemon-design.md); views keep their Classic drawing otherwise.
pub fn lemon(ui: &Ui) -> bool {
    kit::lemon::on(ui.t)
}

/// Button face states.
#[derive(Clone, Copy, PartialEq)]
pub enum Look {
    Off,
    Hover,
    On,
    /// Hovered or held window-toggle buttons light up orange (spec: ui/main-window.md §1a).
    Hot,
}

impl Look {
    /// The kit state for this look.
    pub fn state(self) -> kit::State {
        match self {
            Look::Off => kit::State::NORMAL,
            Look::Hover => kit::State::hover(true),
            Look::On => kit::State::new(false, false, true),
            Look::Hot => kit::State::new(true, true, false),
        }
    }
}

/// FL-style raised button face (spec §1): 1 px border with softened corners, an elliptical radial
/// face (light centre, darker edges) and a darker 1 px inner rim.
pub fn button_face(ui: &mut Ui, r: Rect, look: Look) {
    kit::button(ui.c, ui.t, r, look.state());
}

/// A 30×33 toolbar button (§1). `on` draws the orange "on" state.
/// An `Action::Placeholder` button is not available yet: drawn greyed out, never lit (`Ui::off`).
pub fn tool_button(ui: &mut Ui, r: Rect, icon: Icon, on: bool, action: Action, hint: &'static str) {
    if matches!(action, Action::Placeholder(_)) {
        ui.off(r, hint);
        let st = kit::State::NORMAL.with_disabled(true);
        kit::button(ui.c, ui.t, r, st);
        icons::draw(ui.c, icon, Rect::new(r.x + 5.0, r.y + 6.5, 20.0, 20.0), kit::ink(ui.t, st));
        return;
    }
    let hov = ui.hit(r, action.clone(), hint);
    let down = ui.down(r, &action);
    let look = if on { Look::On } else if down { Look::Hot } else if hov { Look::Hover } else { Look::Off };
    button_face(ui, r, look);
    let c = match look {
        Look::On | Look::Hot => ui.t.on_glyph,
        Look::Hover => ui.t.tx(0xffffff),
        Look::Off => ui.t.glyph,
    };
    icons::draw(ui.c, icon, Rect::new(r.x + 5.0, r.y + 6.5, 20.0, 20.0), c);
}

/// A sunken panel (hint bar, monitors): flat fill inside a 1 px dark border.
pub fn sunken(ui: &mut Ui, r: Rect, fill: Color) {
    kit::sunken(ui.c, ui.t, r, fill);
}

/// A ring knob (toolbar style): arc from the minimum; bipolar knobs pass `center = true`.
pub fn knob(ui: &mut Ui, cx: f32, cy: f32, r: f32, value: f32, center: bool, _c: Color, _track: Color) {
    let hov = ui.hovering(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r));
    kit::knob(ui.c, ui.t, cx, cy, r, value, center, kit::KnobLook::Ring, kit::State::hover(hov));
}

/// A dark glossy ball knob (mixer, rack, slots) with a pointer.
pub fn ball_knob(ui: &mut Ui, cx: f32, cy: f32, r: f32, value: f32) {
    let hov = ui.hovering(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r));
    kit::knob(ui.c, ui.t, cx, cy, r, value, false, kit::KnobLook::Ball, kit::State::hover(hov));
}

/// A round LED (spec §7: grey ring, green ball with a bright core when lit).
pub fn led(ui: &mut Ui, cx: f32, cy: f32, on: bool) {
    let hov = ui.hovering(Rect::new(cx - 6.0, cy - 6.0, 12.0, 12.0));
    kit::led(ui.c, ui.t, cx, cy, 6.0, kit::State::hover(hov).with_on(on));
}

/// Channel / clip colors from the model.
pub fn rgb(c: os_model::Rgb) -> Color {
    Color { r: c.r, g: c.g, b: c.b, a: 255 }
}
