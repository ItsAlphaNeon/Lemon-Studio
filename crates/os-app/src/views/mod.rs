//! Views of the main window, laid out from the measured default layout.
// spec: ui/main-window.md

pub mod browser;
pub mod channel_win;
pub mod menus;
pub mod picker;
pub mod mixer;
pub mod piano;
pub mod playlist;
pub mod rack;
pub mod toolbar;

use std::borrow::Cow;

use os_ui::{Color, Rect};

use crate::app::{Action, Bar, Scroll, State, Ui, Win};
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

pub fn paint(ui: &mut Ui, s: &State, f: &FrameInfo) {
    let (w, h) = ui.c.size();
    ui.c.clear(ui.t.workspace);

    // Workspace (§9): watermark, then docked windows on top.
    let left = if s.show_browser { BROWSER_W } else { 0.0 };
    let work = Rect::new(left, WORK_Y, w - left, h - WORK_Y);
    let docked = s.docked();
    if docked.is_none() {
        let wm = ui.t.watermark;
        icons::logo(ui.c, work.cx(), work.cy() + 40.0, 170.0, wm);
        let version = concat!("OpenStudio v", env!("CARGO_PKG_VERSION"), " - pre-alpha");
        let vc = ui.t.version_text;
        let vw = ui.c.text_width(crate::views::fnt(16.0), version);
        ui.c.text(crate::views::fnt(16.0), work.cx() - vw / 2.0, h - 22.0, vc, version);
    }

    // One stacking order for all child windows (spec: ui/channel-rack.md §5). A maximized window
    // fills the workspace, so windows below it are covered and not painted.
    let docked_at = s.z.iter().rposition(|&w| s.is_max(w));
    if s.show_browser {
        browser::paint(ui, s, Rect::new(0.0, WORK_Y, BROWSER_W, h - WORK_Y));
    }
    let _ = work;
    for (i, &win) in s.z.iter().enumerate() {
        if docked_at.is_some_and(|d| i < d) {
            continue;
        }
        ui.win = Some(win);
        let r = s.win_rect(win);
        match win {
            Win::Playlist => playlist::paint(ui, s, r, f),
            Win::Mixer => mixer::paint(ui, s, r, f),
            Win::PianoRoll => piano::paint(ui, s, r, f),
            Win::ChannelRack => rack::paint(ui, s, f),
            Win::Plugin => channel_win::paint(ui, s, f),
            _ => {}
        }
        ui.win = None;
    }
    toolbar::paint(ui, s, f, w);
    if let Some(m) = s.menu_open {
        menus::paint_dropdown(ui, s, m);
    }
    if s.picker_open {
        picker::paint(ui, s);
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
pub fn window_buttons(ui: &mut Ui, win: Win, x0: f32, y: f32, normal: Color, hover: Color) {
    for (i, (icon, hint, action)) in [
        (Icon::Minimize, "Minimize window", Action::MinWin(win)),
        (Icon::Restore, "Maximize / restore window", Action::MaxWin(win)),
        (Icon::Close, "Close window", Action::CloseWin(win)),
    ]
    .into_iter()
    .enumerate()
    {
        let b = Rect::new(x0 + i as f32 * 19.5, y + 3.0, 17.0, 17.0);
        let hov = ui.hit(b, action, hint);
        icons::draw(ui.c, icon, b.inset(2.0), if hov { hover } else { normal });
    }
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
    if !flat {
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
    ui.c.fill_rect(r, Color::rgb(0x191d1f));
    ui.c.fill_rect(r.inset(1.0), t.window_title);
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, 1.0), Color::rgb(0x303436));
    let inner = Rect::new(r.x + 7.0, r.y + 5.0, r.w - 14.0, r.h - 12.0);
    ui.c.fill_rect(Rect::new(inner.x - 1.0, inner.y + 22.0, inner.w + 2.0, inner.h - 21.0), Color::rgb(0x131718));
    ui.hit(Rect::new(r.x, r.y, r.w, 28.0), Action::WinDrag(win), "Drag to move");
    Some(inner)
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
    let t = ui.t;
    let len = if vertical { track.h } else { track.w };
    let total = total.max(view).max(1.0);
    let max = (total - view).max(0.0);
    let thumb = (view / total * len).clamp(16.0_f32.min(len), len);
    let off = if max > 0.0 { pos.clamp(0.0, max) / max * (len - thumb) } else { 0.0 };
    let zooms = matches!(target, Scroll::PianoX | Scroll::PlaylistX);
    let hint = if zooms { "Drag to scroll - right-drag: left/right scrolls, up/down zooms" } else { "Drag to scroll" };
    let hov = ui.hit(track, Action::Scrollbar(target), hint);
    ui.c.fill_rect(track, t.scrollbar);
    let r = if vertical {
        Rect::new(track.x + 2.0, track.y + off, track.w - 4.0, thumb)
    } else {
        Rect::new(track.x + off, track.y + 2.0, thumb, track.h - 4.0)
    };
    let c = if hov { t.overview.shade(0.12) } else { t.overview };
    ui.c.fill_rrect(r, 2.0_f32.min(r.w / 2.0).min(r.h / 2.0), c);
    ui.bars.push(Bar { target, track, vertical, max, view, thumb });
}

/// Raised surface: soft vertical gradient (lighter at the top) so panels don't look flat.
pub fn raised(ui: &mut Ui, r: Rect, base: Color, radius: f32) {
    ui.c.fill_rect_gradient(r, base.shade(0.045), base.shade(-0.035), radius);
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

/// FL-style raised button face (spec §1): 1 px border with softened corners, an elliptical radial
/// face (light centre, darker edges) and a darker 1 px inner rim.
pub fn button_face(ui: &mut Ui, r: Rect, look: Look) {
    let bg = ui.t.toolbar;
    let (border, center, edge) = match look {
        Look::Off => (Color::rgb(0x191d1f), Color::rgb(0x313537), Color::rgb(0x272b2d)),
        Look::Hover => (Color::rgb(0x191d1f), Color::rgb(0x3d4244), Color::rgb(0x2c3133)),
        Look::On => (Color::rgb(0xbe5507), Color::rgb(0xfffaab), Color::rgb(0xffaf65)),
        Look::Hot => (Color::rgb(0xbe5507), Color::rgb(0xffe598), Color::rgb(0xf99448)),
    };
    ui.c.fill_rect(r, border);
    for (x, y) in [(r.x, r.y), (r.right() - 1.0, r.y), (r.x, r.bottom() - 1.0), (r.right() - 1.0, r.bottom() - 1.0)] {
        ui.c.pixel(x, y, bg.mix(border, 0.25));
    }
    let face = r.inset(1.0);
    ui.c.fill_radial(face, center, edge);
    // Inner rim.
    let rim = Color::rgb(0x000000).with_alpha(if matches!(look, Look::On | Look::Hot) { 26 } else { 40 });
    ui.c.fill_rect(Rect::new(face.x, face.y, face.w, 1.0), rim);
    ui.c.fill_rect(Rect::new(face.x, face.bottom() - 1.0, face.w, 1.0), rim);
    ui.c.fill_rect(Rect::new(face.x, face.y + 1.0, 1.0, face.h - 2.0), rim);
    ui.c.fill_rect(Rect::new(face.right() - 1.0, face.y + 1.0, 1.0, face.h - 2.0), rim);
}

/// A 30×33 toolbar button (§1). `on` draws the orange "on" state.
pub fn tool_button(ui: &mut Ui, r: Rect, icon: Icon, on: bool, action: Action, hint: &'static str) {
    let hov = ui.hit(r, action.clone(), hint);
    let down = ui.down(r, &action);
    let look = if on { Look::On } else if down { Look::Hot } else if hov { Look::Hover } else { Look::Off };
    button_face(ui, r, look);
    let c = match look {
        Look::On | Look::Hot => ui.t.on_glyph,
        Look::Hover => Color::rgb(0xffffff),
        Look::Off => ui.t.glyph,
    };
    icons::draw(ui.c, icon, Rect::new(r.x + 5.0, r.y + 6.5, 20.0, 20.0), c);
}

/// A sunken panel (hint bar, monitors): flat fill inside a 1 px dark border.
pub fn sunken(ui: &mut Ui, r: Rect, fill: Color) {
    ui.c.fill_rect(r, Color::rgb(0x1d2022));
    ui.c.fill_rect(r.inset(1.0), fill);
}

/// A ring knob (toolbar style): arc from the minimum; bipolar knobs pass `center = true`.
pub fn knob(ui: &mut Ui, cx: f32, cy: f32, r: f32, value: f32, center: bool, c: Color, track: Color) {
    use std::f32::consts::PI;
    let start = 0.75 * PI;
    let sweep = 1.5 * PI;
    ui.c.fill_circle(cx, cy, r, track.shade(-0.5));
    ui.c.arc(cx, cy, r - 1.5, start, sweep, 2.0, track);
    let (a0, a1) = if center { (start + sweep / 2.0, start + sweep * value) } else { (start, start + sweep * value) };
    if (a1 - a0).abs() > 0.01 {
        ui.c.arc(cx, cy, r - 1.5, a0.min(a1), (a1 - a0).abs(), 2.0, c);
    }
    let a = start + sweep * value;
    ui.c.line(cx + (r - 6.0).max(1.0) * a.cos(), cy + (r - 6.0).max(1.0) * a.sin(), cx + (r - 2.5) * a.cos(), cy + (r - 2.5) * a.sin(), 1.6, c);
}

/// A dark glossy ball knob (mixer, rack, slots) with a pointer.
pub fn ball_knob(ui: &mut Ui, cx: f32, cy: f32, r: f32, value: f32) {
    use std::f32::consts::PI;
    let t = ui.t;
    ui.c.fill_circle(cx, cy, r, t.toolbar_line.shade(-0.3));
    ui.c.fill_circle(cx, cy, r - 1.0, t.button_hi.shade(0.02));
    ui.c.fill_circle(cx, cy + 0.6, r - 2.2, t.sunken_dark.shade(-0.1));
    ui.c.fill_circle(cx - r * 0.25, cy - r * 0.35, r * 0.35, Color::rgb(0xffffff).with_alpha(14));
    let a = 0.75 * PI + 1.5 * PI * value.clamp(0.0, 1.0);
    let (ix, iy) = (r * 0.3, r * 0.3);
    let _ = (ix, iy);
    ui.c.line(cx + (r * 0.35) * a.cos(), cy + (r * 0.35) * a.sin(), cx + (r - 2.8) * a.cos(), cy + (r - 2.8) * a.sin(), 1.4, t.glyph.shade(-0.15));
}

/// A round LED (spec §7: grey ring, green ball with a bright core when lit).
pub fn led(ui: &mut Ui, cx: f32, cy: f32, on: bool) {
    ui.c.fill_circle(cx, cy, 6.0, Color::rgb(0x595b5c));
    ui.c.fill_circle(cx, cy, 5.0, Color::rgb(0x283028));
    if on {
        ui.c.fill_circle(cx, cy, 4.2, Color::rgb(0x3d7803));
        ui.c.fill_circle(cx, cy, 3.4, Color::rgb(0xa0dd48));
        ui.c.fill_circle(cx - 0.6, cy - 0.8, 1.8, Color::rgb(0xe2f5b8));
    } else {
        ui.c.fill_circle(cx, cy, 4.0, Color::rgb(0x1f2a14));
    }
}

/// Channel / clip colors from the model.
pub fn rgb(c: os_model::Rgb) -> Color {
    Color { r: c.r, g: c.g, b: c.b, a: 255 }
}
