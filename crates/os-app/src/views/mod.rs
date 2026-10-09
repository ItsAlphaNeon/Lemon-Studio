//! Views of the main window, laid out from the measured default layout.
// spec: ui/main-window.md

pub mod browser;
pub mod menus;
pub mod mixer;
pub mod playlist;
pub mod rack;
pub mod toolbar;

use std::borrow::Cow;

use os_ui::{Color, Face, Rect};

use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};

/// Per-frame values that don't live in `State`.
pub struct FrameInfo {
    pub title: String,
    pub hint: Cow<'static, str>,
    pub playing: bool,
    pub seconds: f64,
    pub peaks: (f32, f32),
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
    let wm = ui.t.watermark;
    icons::logo(ui.c, work.cx(), work.cy() + 40.0, 170.0, wm);
    let version = concat!("OpenStudio v", env!("CARGO_PKG_VERSION"), " - pre-alpha");
    let vc = ui.t.version_text;
    let vw = ui.c.text_width(Face::Ui, 16.0, version);
    ui.c.text(Face::Ui, 16.0, work.cx() - vw / 2.0, h - 22.0, vc, version);

    match s.front {
        Some(Win::Playlist) => playlist::paint(ui, s, work, f),
        Some(Win::Mixer) => mixer::paint(ui, s, work, f),
        Some(Win::PianoRoll) => placeholder_window(ui, work, "Piano roll - (not available yet)"),
        _ => {}
    }
    if s.show_browser {
        browser::paint(ui, s, Rect::new(0.0, WORK_Y, BROWSER_W, h - WORK_Y));
    }
    if s.show_rack {
        rack::paint(ui, s);
    }
    toolbar::paint(ui, s, f, w);
    if let Some(m) = s.menu_open {
        menus::paint_dropdown(ui, m);
    }
}

/// Docked window frame with a title bar (§5). Returns the client rect.
pub fn window_frame(ui: &mut Ui, r: Rect, title: &str, title_x: f32) -> Rect {
    let t = ui.t;
    ui.c.fill_rect(r, t.window_title);
    let bar = Rect::new(r.x, r.y, r.w, 23.0);
    ui.c.text(Face::Ui, 13.0, r.x + title_x, r.y + 5.0, t.text, title);
    // Minimize / maximize / close glyphs at the right (1862, 1881, 1901 at full width).
    let right = r.right();
    for (i, (icon, hint)) in [(Icon::Minimize, "Minimize window"), (Icon::Restore, "Restore window"), (Icon::Close, "Close window")]
        .into_iter()
        .enumerate()
    {
        let b = Rect::new(right - 64.0 + i as f32 * 19.5, r.y + 3.0, 17.0, 17.0);
        let hov = ui.hit(b, Action::Placeholder("Window button"), hint);
        let c = if hov { t.glyph } else { t.text_dim.mix(t.glyph, 0.6) };
        icons::draw(ui.c, icon, b.inset(2.0), c);
    }
    let _ = bar;
    Rect::new(r.x, r.y + 23.0, r.w, r.h - 23.0)
}

fn placeholder_window(ui: &mut Ui, r: Rect, title: &str) {
    let client = window_frame(ui, r, title, 30.0);
    ui.c.fill_rect(client, ui.t.grid);
}

/// A 30×31 toolbar button (§1). `on` draws the orange "on" state.
pub fn tool_button(ui: &mut Ui, r: Rect, icon: Icon, on: bool, action: Action, hint: &'static str) {
    let hov = ui.hit(r, action, hint);
    let t = ui.t;
    if on {
        ui.c.fill_rect_gradient(r, t.on_top, t.on_bottom, 3.0);
        icons::draw(ui.c, icon, r.inset(6.0), t.on_glyph);
    } else {
        let face = if hov { t.button_hi } else { t.button };
        ui.c.fill_rrect(r, 3.0, t.toolbar_line);
        ui.c.fill_rect_gradient(r.inset(1.0), face.shade(0.04), face, 2.5);
        icons::draw(ui.c, icon, r.inset(7.0), if hov { Color::rgb(0xffffff) } else { t.glyph });
    }
}

/// A sunken panel (hint bar, monitors).
pub fn sunken(ui: &mut Ui, r: Rect, fill: Color) {
    ui.c.fill_rrect(r, 2.0, fill);
    ui.c.line(r.x + 2.0, r.y + 0.5, r.right() - 2.0, r.y + 0.5, 1.0, fill.shade(-0.3));
}

/// A ring knob: `value` 0…1 (bipolar knobs pass `center = true` to draw from 12 o'clock).
pub fn knob(ui: &mut Ui, cx: f32, cy: f32, r: f32, value: f32, center: bool, c: Color, track: Color) {
    use std::f32::consts::PI;
    let start = 0.75 * PI;
    let sweep = 1.5 * PI;
    ui.c.fill_circle(cx, cy, r, track.shade(-0.35));
    ui.c.arc(cx, cy, r - 1.5, start, sweep, 2.0, track);
    let (a0, a1) = if center { (start + sweep / 2.0, start + sweep * value) } else { (start, start + sweep * value) };
    if (a1 - a0).abs() > 0.01 {
        ui.c.arc(cx, cy, r - 1.5, a0.min(a1), (a1 - a0).abs(), 2.0, c);
    }
    let a = start + sweep * value;
    ui.c.line(cx + (r - 6.0).max(1.0) * a.cos(), cy + (r - 6.0).max(1.0) * a.sin(), cx + (r - 2.5) * a.cos(), cy + (r - 2.5) * a.sin(), 1.6, c);
}

/// A round LED (green accent when lit).
pub fn led(ui: &mut Ui, cx: f32, cy: f32, on: bool) {
    let t = ui.t;
    ui.c.fill_circle(cx, cy, 5.0, t.toolbar_line);
    ui.c.fill_circle(cx, cy, 3.6, if on { t.accent } else { t.accent_dim.shade(-0.4) });
    if on {
        ui.c.fill_circle(cx - 1.0, cy - 1.0, 1.3, t.accent.shade(0.5));
    }
}

/// Channel / clip colors from the model.
pub fn rgb(c: os_model::Rgb) -> Color {
    Color { r: c.r, g: c.g, b: c.b, a: 255 }
}
