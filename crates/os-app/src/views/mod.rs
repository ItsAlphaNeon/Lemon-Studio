//! Views of the main window, laid out from the measured default layout.
// spec: ui/main-window.md

pub mod browser;
pub mod menus;
pub mod mixer;
pub mod playlist;
pub mod rack;
pub mod toolbar;

use std::borrow::Cow;

use os_ui::{Color, Rect};

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
    if s.front.is_none() {
        let wm = ui.t.watermark;
        icons::logo(ui.c, work.cx(), work.cy() + 40.0, 170.0, wm);
        let version = concat!("OpenStudio v", env!("CARGO_PKG_VERSION"), " - pre-alpha");
        let vc = ui.t.version_text;
        let vw = ui.c.text_width(crate::views::fnt(16.0), version);
        ui.c.text(crate::views::fnt(16.0), work.cx() - vw / 2.0, h - 22.0, vc, version);
    }

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
    ui.c.fill_rect_gradient(bar, t.window_title.shade(0.04), t.window_title.shade(-0.03), 0.0);
    ui.c.fill_rect(Rect::new(r.x, r.y + 22.0, r.w, 1.0), t.splitter_dark);
    ui.c.text(os_ui::Font::new(17), r.x + title_x, r.y + 1.0, t.text, title);
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
}

/// FL-style raised button face (spec §1): 1 px border with softened corners, an elliptical radial
/// face (light centre, darker edges) and a darker 1 px inner rim.
pub fn button_face(ui: &mut Ui, r: Rect, look: Look) {
    let bg = ui.t.toolbar;
    let (border, center, edge) = match look {
        Look::Off => (Color::rgb(0x191d1f), Color::rgb(0x313537), Color::rgb(0x272b2d)),
        Look::Hover => (Color::rgb(0x191d1f), Color::rgb(0x3d4244), Color::rgb(0x2c3133)),
        Look::On => (Color::rgb(0xbe5507), Color::rgb(0xfffaab), Color::rgb(0xffaf65)),
    };
    ui.c.fill_rect(r, border);
    for (x, y) in [(r.x, r.y), (r.right() - 1.0, r.y), (r.x, r.bottom() - 1.0), (r.right() - 1.0, r.bottom() - 1.0)] {
        ui.c.pixel(x, y, bg.mix(border, 0.25));
    }
    let face = r.inset(1.0);
    ui.c.fill_radial(face, center, edge);
    // Inner rim.
    let rim = Color::rgb(0x000000).with_alpha(if look == Look::On { 26 } else { 40 });
    ui.c.fill_rect(Rect::new(face.x, face.y, face.w, 1.0), rim);
    ui.c.fill_rect(Rect::new(face.x, face.bottom() - 1.0, face.w, 1.0), rim);
    ui.c.fill_rect(Rect::new(face.x, face.y + 1.0, 1.0, face.h - 2.0), rim);
    ui.c.fill_rect(Rect::new(face.right() - 1.0, face.y + 1.0, 1.0, face.h - 2.0), rim);
}

/// A 30×33 toolbar button (§1). `on` draws the orange "on" state.
pub fn tool_button(ui: &mut Ui, r: Rect, icon: Icon, on: bool, action: Action, hint: &'static str) {
    let hov = ui.hit(r, action, hint);
    let look = if on { Look::On } else if hov { Look::Hover } else { Look::Off };
    button_face(ui, r, look);
    let c = match look {
        Look::On => ui.t.on_glyph,
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
