// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The toolbar rows, their panels wherever the layout puts them (`crate::toolbar`), and the editor's palette.
// spec: ui/main-window.md §1–3, ui/toolbar-menu.md §8

use std::ops::Range;

use os_ui::{Font, Rect};

use super::{menus, sunken, tool_button, FrameInfo};
use crate::app::{Action, Live, State, Ui, Util, Win};
use crate::icons::{self, Icon};
use crate::toolbar::{place, At, BarHit, Moving, Panel, Placed, Target, Tile, PANELS, ROW_H};

/// The ▶ arrow's width at the band's right end.
pub const MORE_W: f32 = 18.0;
/// The arrow's hint, short enough for the hint panel. A click on it does nothing (spec: ui/ui-scaling.md, Lemon Studio).
pub const MORE_HINT: &str = "Widen the window or lower Toolbars scaling";

/// Where the panels of a band `width` px wide must end to show, when all of them need `full` (`toolbar::natural_width`;
/// 1440 for the classic layout, whose last utility button ends at x 1437): anywhere when all of them fit, else before
/// the arrow. A narrower band leaves out the panels that don't fit, from the right of each row, and draws the ▶ arrow
/// at its right end. Spec: ui/ui-scaling.md §1 "Toolbars scaling".
pub fn room(width: f32, full: f32) -> f32 {
    if width >= full { f32::INFINITY } else { width - MORE_W }
}

/// `room` for the band of `s`, `width` px wide.
pub fn fit(s: &State, width: f32) -> f32 {
    room(width, crate::toolbar::natural_width(&shown(s), s.bar.editing))
}

/// The Toolbars menu's switches (spec: ui/toolbar-menu.md §8): app-wide, kept in `settings.txt` across runs but for
/// Hide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opts {
    /// Group and row separator lines (§2).
    pub separators: bool,
    /// Square buttons without face and edges (§3).
    pub flat: bool,
    /// The floating hint bar at the window's bottom-left (§4).
    pub hint_bar: bool,
    /// Toolbar and menu bar hidden, the browser and workspace up to the window's top (§5).
    pub hidden: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { separators: true, flat: false, hint_bar: false, hidden: false }
    }
}

impl Opts {
    /// The switches kept in `settings.txt` (`toolbar_separators`, `toolbar_flat_buttons`, `toolbar_hint_bar`); the
    /// default for one that is missing. Hide is not kept: a start shows the toolbar.
    pub fn load() -> Opts {
        use crate::settings_file::flag;
        let d = Opts::default();
        Opts {
            separators: flag("toolbar_separators", d.separators),
            flat: flag("toolbar_flat_buttons", d.flat),
            hint_bar: flag("toolbar_hint_bar", d.hint_bar),
            hidden: false,
        }
    }

    /// Keeps the switches but Hide for the next start.
    pub fn save(&self) {
        use crate::settings_file::set_flag;
        set_flag("toolbar_separators", self.separators);
        set_flag("toolbar_flat_buttons", self.flat);
        set_flag("toolbar_hint_bar", self.hint_bar);
    }
}

/// The Toolbars menu entries that work (§8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// The toolbar editor on / off.
    Edit,
    Separators,
    Flat,
    HintBar,
    Hide,
}

/// The toolbar as shown: the tiling layout's leaves out the caption buttons where the window manager tiles the window
/// (`crate::desktop`: it places, sizes and closes windows with its own keys), its flexible spaces taking their room;
/// the editor shows them still.
pub fn shown(s: &State) -> std::borrow::Cow<'_, crate::toolbar::Layout> {
    let l = &s.bar.layout;
    if !s.bar.tiling || s.bar.editing || crate::desktop::captions() || !l.has(Panel::Caption) {
        return std::borrow::Cow::Borrowed(l);
    }
    let mut l = l.clone();
    for row in &mut l.rows {
        row.retain(|&p| p != Panel::Caption);
    }
    std::borrow::Cow::Owned(l)
}

/// The toolbar's rows, the bottom edge excepted, in a band `width` px wide: one band, repainted whole while the
/// separators are off (their contents are moved by a pixel after painting, `paint`).
pub fn band(s: &State, width: f32) -> Rect {
    Rect::new(0.0, 0.0, width, s.bar.layout.height())
}

/// The floating hint bar: 400 x 82 at the window's left edge, its bottom 2 px above the window's (§4).
pub fn hint_bar_rect(s: &State) -> Rect {
    Rect::new(0.0, s.screen.1 - 84.0, 400.0, 82.0)
}

/// The floating hint bar (§4): a near-black panel with the hint at its top left and a round grip at its right, over
/// every window. Clicks on it go nowhere. A hint too long for a line wraps, as many lines as the panel holds; the
/// panel is tall enough for several lines.
pub fn paint_hint_bar(ui: &mut Ui, s: &State, f: &FrameInfo) {
    let t = ui.t;
    let r = hint_bar_rect(s);
    ui.hit(r, Action::Swallow, "");
    let (edge, mid) = (t.s(0x141618), t.s(0x111213));
    let half = (r.h / 2.0).round();
    ui.c.fill_rect_gradient(Rect::new(r.x, r.y, r.w, half), edge, mid, 0.0);
    ui.c.fill_rect_gradient(Rect::new(r.x, r.y + half, r.w, r.h - half), mid, edge, 0.0);
    let hint: &str = if !f.hint.is_empty() { &f.hint } else { s.message.as_deref().unwrap_or("") };
    let font = Font::new(14);
    let (w, lh) = (r.w - 50.0, ui.c.line_height(font));
    let chars: Vec<char> = hint.chars().collect();
    let mut y = r.y + 5.0;
    for (start, n) in super::settings::wrap(ui, font, hint, w) {
        if y + lh > r.bottom() - 4.0 {
            break;
        }
        let line: String = chars[start..start + n].iter().collect();
        ui.c.text_clip(font, r.x + 7.0, y, w, t.text, line.trim_end());
        y += lh;
    }
    let (cx, cy) = (r.right() - 22.0, r.cy());
    ui.c.fill_circle(cx, cy, 11.0, t.s(0x24272a));
    for (dx, dy) in [(-3.0, -3.0), (3.0, -3.0), (-3.0, 3.0), (3.0, 3.0)] {
        ui.c.fill_circle(cx + dx, cy + dy, 1.3, t.s(0x5c6164));
    }
}

fn btn(x: f32, y: f32) -> Rect {
    Rect::new(x, y, 30.0, 33.0)
}

/// What a panel paints with: where it is, where the row's room ends (`room`, band px) and whether it is a palette
/// tile's picture (`preview`: nothing it paints may change the state, such as the tempo digits' places).
struct Ctx<'a> {
    s: &'a State,
    f: &'a FrameInfo,
    a: At,
    fit: f32,
    preview: bool,
}

/// The band in its own px, `width` of them wide (`views::paint` scales it by Toolbars scaling). In each row the panels
/// that would end past `room` are left out, from the first one that does not fit.
pub fn paint(ui: &mut Ui, s: &State, f: &FrameInfo, width: f32) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    let shown = shown(s);
    let l = &*shown;
    let editing = s.bar.editing;
    let fit = fit(s, width);
    let (rows, h) = (l.rows.len(), l.height());
    // Separator lines: off from the Toolbars menu (spec: ui/toolbar-menu.md §2).
    let seps = s.toolbar.separators;
    // Bands.
    if lemon {
        // Lemon (spec: ui/lemon-design.md §4): flat bands, short rounded separators.
        ui.c.fill_rect(Rect::new(0.0, 0.0, width, h), t.toolbar);
        for r in (1..rows).filter(|_| seps) {
            ui.c.fill_rect(Rect::new(0.0, ROW_H * r as f32, width, 1.0), t.toolbar_line);
        }
    } else {
        for r in 0..rows {
            let (top, bottom) = l.row_span(r);
            let (a, b) = if r == 0 { (0.025, -0.015) } else { (0.02, -0.02) };
            ui.c.fill_rect_gradient(Rect::new(0.0, top, width, bottom - top), t.toolbar.shade(a), t.toolbar.shade(b), 0.0);
            if r > 0 {
                let line = if seps { t.toolbar_line } else { t.toolbar.shade(-0.015) };
                ui.c.fill_rect(Rect::new(0.0, ROW_H * r as f32, width, 1.0), line);
            }
        }
    }
    ui.c.fill_rect(Rect::new(0.0, h, width, 2.0), t.toolbar_line);
    // Empty toolbar areas drag the window (registered first, so controls win).
    ui.hit(Rect::new(0.0, 0.0, width, h), Action::DragWindow, "");

    let placed = place(l, width, editing);
    for row in 0..rows {
        for p in placed.iter().filter(|p| p.row == row) {
            let a = p.at();
            let shows = if p.panel.repeats() { p.x + p.w <= fit } else { a.x(p.panel.first_end()) <= fit };
            if !shows {
                break;
            }
            // Group separators (2 px gaps in row 1 and row 2), each at the left edge of the panel it comes
            // with, shown with that panel; none at a row's start.
            if seps && p.panel.separator() && p.idx > 0 {
                separator(ui, p.x, p.row, lemon);
            }
            let mark = ui.mark();
            let all = panel(ui, &Ctx { s, f, a, fit, preview: false }, p.panel);
            if editing {
                ui.forget(mark);
                item_frame(ui, l, p);
            }
            if !all {
                break;
            }
        }
    }
    // Without separators row 1's contents move 1 px down and row 2's 1 px up, over the row line (§2). The pixels are
    // moved after painting; the band is always repainted whole then (`App::take_damage`).
    if !seps && rows == 2 {
        let d = ui.c.scale.round().max(1.0) as i32;
        ui.c.shift(Rect::new(0.0, 0.0, width, 42.0), 0, d, &[]);
        ui.c.shift(Rect::new(0.0, 41.0, width, 40.0), 0, -d, &[]);
    }
    if editing {
        grips(ui, s);
    }

    // Panels left out (spec: ui/ui-scaling.md §1 "Toolbars scaling"): the ▶ arrow at the band's right end, in the logo's
    // place. Hovering it says why; a click does nothing.
    let mid = (h / 2.0).floor();
    if fit.is_finite() {
        let r = Rect::new(width - MORE_W, 0.0, MORE_W, h);
        ui.hit(r, Action::Swallow, MORE_HINT);
        os_ui::kit::triangle(ui.c, r.cx(), mid, 5.0, os_ui::kit::Dir::Right, t.glyph);
        return;
    }
    if !l.logo {
        return;
    }
    // Logo spot (spans the rows): the Lemon Studio mark, just right of the last panel (x 1487 in the classic layout). It
    // keeps to the right edge until the window is too narrow, then the edge crops it (spec ui/main-window.md §12); the
    // toolbar never moves.
    let end = placed.iter().map(|p| p.x + p.w).fold(0.0, f32::max);
    let lx = (width - 45.0).max(end + 47.0);
    let r = Rect::new(lx - 35.0, 0.0, 78.0, h - 1.0);
    if editing {
        ui.hit(r, Action::Bar(BarHit::Logo), "Logo - drag onto the palette to take it off");
        frame(ui, r.inset(2.0));
    } else {
        ui.hit(r, Action::MenuItem(menus::HELP_MENU, menus::HELP_ABOUT), "About Lemon Studio");
    }
    icons::logo(ui.c, lx, mid, 26.0 * h / 81.0, if lemon { t.toolbar_edge } else { t.toolbar_line });
}

/// A group separator at band x `x` of row `row`.
fn separator(ui: &mut Ui, x: f32, row: usize, lemon: bool) {
    let t = ui.t;
    let top = ROW_H * row as f32;
    if lemon {
        ui.c.fill_rrect(Rect::new(x, top + 9.0, 2.0, 23.0), 1.0, t.toolbar_line);
        return;
    }
    let (y, h) = if row == 0 { (2.0, 37.0) } else { (top + 3.0, 35.0) };
    ui.c.fill_rect(Rect::new(x, y, 1.0, h), t.toolbar_line);
    ui.c.fill_rect(Rect::new(x + 1.0, y, 1.0, h), t.toolbar_edge.with_alpha(110));
}

/// Paints panel `p`; false when one of its buttons did not fit (its row ends there).
fn panel(ui: &mut Ui, c: &Ctx, p: Panel) -> bool {
    match p {
        Panel::Caption => caption(ui, c),
        Panel::Hint => hint_panel(ui, c),
        Panel::Transport => transport(ui, c),
        Panel::Recording => return recording(ui, c),
        Panel::Time => time(ui, c),
        Panel::Monitor => monitor(ui, c),
        Panel::Cpu => cpu(ui, c),
        Panel::Utilities => return utilities(ui, c),
        Panel::Menu => menu_bar(ui, c),
        Panel::Volume => volume(ui, c),
        Panel::Pitch => pitch(ui, c),
        Panel::SongPos => song_pos(ui, c),
        Panel::Modes => return modes(ui, c),
        Panel::Snap => snap(ui, c),
        Panel::Pattern => pattern(ui, c),
        Panel::Windows => return window_buttons(ui, c, 0..5),
        Panel::Tools => return window_buttons(ui, c, 5..9),
        Panel::Space | Panel::Flex => {}
    }
    true
}

/// Caption buttons.
fn caption(ui: &mut Ui, c: &Ctx) {
    let (t, a) = (ui.t, c.a);
    for (x, icon, action, hint) in [
        (4.0, Icon::Minimize, Action::Minimize, "Minimize"),
        (24.0, Icon::Restore, Action::Maximize, "Restore / maximize"),
        (44.0, Icon::Close, Action::Close, "Exit Lemon Studio"),
    ] {
        let r = a.r(Rect::new(x, 11.0, 19.0, 19.0));
        let hov = ui.hit(r, action, hint);
        super::button_face(ui, r, if hov { super::Look::Hover } else { super::Look::Off });
        icons::draw(ui.c, icon, r.inset(3.5), if hov { ui.t.tx(0xffffff) } else { t.glyph });
    }
}

/// Hint panel: project title + hint / message.
fn hint_panel(ui: &mut Ui, c: &Ctx) {
    let (t, a, s, f) = (ui.t, c.a, c.s, c.f);
    let hp = a.span(72.0, 376.0, 4.0, 36.0);
    sunken(ui, hp, t.sunken);
    ui.hit(hp, Action::DragWindow, "Status bar");
    ui.c.text_clip(Font::new(13), a.x(76.0), a.y(6.0), 280.0, t.text_dim.shade(-0.05), &f.title);
    let hint: &str = if !f.hint.is_empty() { &f.hint } else { s.message.as_deref().unwrap_or("") };
    ui.c.text_clip(Font::new(14), a.x(77.0), a.y(19.0), 280.0, t.text, hint);
    icons::draw(ui.c, Icon::MidiControl, a.span(360.0, 374.0, 17.0, 32.0), ui.t.s(0x515457));
}

/// Transport group: PAT / SONG, play, stop, record and the tempo (to the tempo box's right edge, x 570).
fn transport(ui: &mut Ui, c: &Ctx) {
    let (t, a, s, f) = (ui.t, c.a, c.s, c.f);
    let lemon = super::lemon(ui);
    let capsule = a.span(383.0, 483.0, 4.0, 36.0);
    if lemon {
        // Lemon: one rounded-square tray, each section a rounded square inside it.
        os_ui::kit::lemon::outlined(ui.c, capsule, 7.0, t.toolbar_line.shade(-0.15), t.sunken);
    } else {
        ui.c.fill_rrect(capsule, 15.0, t.toolbar_line.shade(-0.2));
        ui.c.fill_rect_gradient(capsule.inset(1.0), t.sunken.shade(0.03), t.sunken.shade(-0.05), 14.0);
        ui.c.fill_rect(a.r(Rect::new(418.0, 6.0, 1.0, 29.0)), t.toolbar_line);
        ui.c.fill_rect(a.r(Rect::new(449.0, 6.0, 1.0, 29.0)), t.toolbar_line);
    }
    // PAT / SONG fill the whole top / bottom half of the capsule's left section.
    let pat = a.span(384.0, 418.0, 5.0, 20.0);
    let song = a.span(384.0, 418.0, 20.0, 35.0);
    ui.hit(pat, Action::PatSong(false), "Pattern: play the current pattern");
    ui.hit(song, Action::PatSong(true), "Song: play the arrangement");
    for (r, label, active) in [(pat, "PAT", !s.song_mode), (song, "SONG", s.song_mode)] {
        if active && lemon {
            os_ui::kit::lemon::lit_in(ui.c, ui.t, Rect::new(r.x + 2.0, r.y + 1.0, r.w - 3.0, r.h - 1.5), 4.0, false);
        } else if active {
            ui.c.push_clip(r);
            ui.c.fill_rrect(capsule.inset(1.0), 14.0, t.pill);
            ui.c.pop_clip();
        }
        ui.c.text_center(Font::condensed(10, 5), Rect::new(r.x + 1.0, r.y, r.w, r.h), if active { t.pill_text } else { t.glyph_dim }, label);
    }
    // Lit while playing, with a pause glyph (Play pauses); unlit with the play glyph when paused.
    let play_on = f.playing && !f.paused;
    for (r, icon, on, action, hint) in [
        (a.span(419.0, 449.0, 5.0, 35.0), Icon::Play, play_on, Action::Play, "Play / pause (Space plays / stops)"),
        (a.span(450.0, 482.0, 5.0, 35.0), Icon::Stop, false, Action::Stop, "Stop (Space)"),
    ] {
        let hov = ui.hit(r, action.clone(), hint);
        let on = on || ui.down(r, &action);
        // Lit / hovered sections fill their whole slot of the capsule (clipped to its round end).
        if (on || hov) && lemon {
            let seg = Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 2.0);
            if on {
                os_ui::kit::lemon::lit_in(ui.c, ui.t, seg, 5.0, hov);
            } else {
                ui.c.fill_rrect(seg, 5.0, t.button_hi);
            }
        } else if on || hov {
            ui.c.push_clip(r);
            if on {
                ui.c.fill_rect_gradient(capsule.inset(1.0), t.on_top, t.on_bottom, 14.0);
            } else {
                ui.c.fill_rrect(capsule.inset(1.0), 14.0, t.button_hi.with_alpha(110));
            }
            ui.c.pop_clip();
        }
        let g = Rect::new(r.x + 8.0, r.y + 8.0, r.w - 16.0, r.h - 16.0);
        let c = if on { t.on_glyph } else { t.glyph };
        if icon == Icon::Play && s.rec.waiting {
            // Waiting for input (spec: ui/recording.md §3): three dots.
            for k in [-1.0, 0.0, 1.0] {
                ui.c.fill_circle(g.cx() + k * g.w * 0.32, g.cy(), (g.w * 0.1).max(1.5), c);
            }
        } else if icon == Icon::Play && on {
            // Pause: two bars.
            let (w, h) = (g.w * 0.22, g.h * 0.7);
            let y = g.cy() - h / 2.0;
            ui.c.fill_rrect(Rect::new(g.cx() - w * 1.6, y, w, h), 1.0, c);
            ui.c.fill_rrect(Rect::new(g.cx() + w * 0.6, y, w, h), 1.0, c);
        } else {
            icons::draw(ui.c, icon, g, c);
        }
    }
    // Record (spec: ui/recording.md §2): armed, the whole round face fills with the record colour, a lighter rim and a
    // dark dot in the middle; disarmed, a small dimmed dot. The record colour is white under High visibility
    // (`crate::high_visibility`).
    let rec = a.span(486.0, 513.0, 7.0, 33.0);
    let hov = ui.hit(rec, Action::Record, crate::record::hint());
    let armed = s.rec.armed;
    let down = ui.down(rec, &Action::Record);
    let dot = crate::high_visibility::record_dot(t, crate::high_visibility::on());
    let face = Rect::new(rec.cx() - 14.0, rec.cy() - 14.0, 28.0, 28.0);
    if lemon {
        os_ui::kit::button(ui.c, t, face, os_ui::kit::State::new(hov, down, false));
    } else {
        ui.c.fill_circle(rec.cx(), rec.cy(), 14.0, t.toolbar_line.shade(-0.25));
        let bg = if hov || down { t.button_hi } else { t.button.shade(-0.12) };
        ui.c.fill_circle(rec.cx(), rec.cy(), 12.6, bg);
    }
    if armed {
        let fill = if hov || down { dot.shade(0.12) } else { dot };
        ui.c.fill_circle(rec.cx(), rec.cy(), 12.0, fill);
        ui.c.stroke_circle(rec.cx(), rec.cy(), 11.4, 1.6, dot.shade(0.45));
        ui.c.fill_circle(rec.cx(), rec.cy(), 4.6, dot.shade(-0.6));
    } else {
        let bg = if lemon { t.toolbar } else { t.button.shade(-0.12) };
        ui.c.fill_circle(rec.cx(), rec.cy(), 5.6, if hov { dot } else { dot.mix(bg, 0.35) });
    }
    // Tempo.
    let tb = a.span(517.0, 570.0, 6.0, 34.0);
    ui.hit(tb, Action::Tempo, "Tempo (drag up/down: the whole number, or the decimal place the drag starts on)");
    if lemon {
        os_ui::kit::lemon::well(ui.c, t, tb, t.time_panel);
    } else {
        ui.c.fill_rrect(tb, 5.0, ui.t.s(0x5b5f61));
        ui.c.fill_rrect(tb.inset(1.0), 4.0, ui.t.s(0x5f6466));
    }
    // Lemon displays show their digits in the brand colour.
    let digits = if lemon { t.pill } else { t.glyph };
    // While playing, the tempo playback is at: tempo automation moves it (spec: audio/timing.md "Tempo that changes").
    let whole = format!("{}", f.tempo.trunc() as i64);
    let frac = format!(".{:03}", ((f.tempo.fract()) * 1000.0).round() as i64 % 1000);
    let ww = ui.c.text_width(Font::condensed(19, 6), &whole);
    let fw = ui.c.text_width(Font::condensed(13, 5), &frac);
    let x0 = (tb.right() - 11.0 - ww - fw).max(tb.x + 2.0);
    // Where each decimal digit starts (band px), for the drag's step (the point counts as the whole number).
    if !c.preview {
        let mut small = |n: usize| x0 + ww + ui.c.text_width(Font::condensed(13, 5), &frac[..n]);
        s.tempo_digits.set([small(1), small(2), small(3)]);
    }
    ui.c.push_clip(tb.inset(1.0));
    ui.c.text(Font::condensed(19, 6), x0, a.y(9.0), digits, &whole);
    ui.c.text(Font::condensed(13, 5), x0 + ww, a.y(16.0), t.glyph_dim.shade(0.3), &frac);
    ui.c.pop_clip();
    let (r, y) = (tb.right(), |v: f32| a.y(v));
    ui.c.fill_polygon(&[(r - 9.0, y(17.0)), (r - 6.0, y(13.0)), (r - 3.0, y(17.0))], t.glyph_dim);
    ui.c.fill_polygon(&[(r - 9.0, y(22.0)), (r - 6.0, y(26.0)), (r - 3.0, y(22.0))], t.glyph_dim);
}

/// Recording options. The metronome works (spec: ui/metronome.md); the others wait for recording.
fn recording(ui: &mut Ui, c: &Ctx) -> bool {
    let a = c.a;
    let m = a.r(btn(580.0, 4.0));
    if m.right() > c.fit {
        return false;
    }
    tool_button(ui, m, Icon::Metronome, crate::metronome::on(), Action::Metronome, "Metronome (Ctrl+M) - right-click for the sound and volume");
    // Start on input, Recording precount, Blend, Loop record (spec: ui/recording.md §3-4).
    use crate::record::Opt;
    let opts = [(Icon::WaitInput, Opt::Wait), (Icon::Countdown, Opt::Precount), (Icon::Blend, Opt::Blend), (Icon::LoopRecord, Opt::Loop)];
    for (i, (icon, opt)) in opts.into_iter().enumerate() {
        let r = a.r(btn(613.0 + 33.0 * i as f32, 4.0));
        if r.right() > c.fit {
            return false;
        }
        tool_button(ui, r, icon, opt.on(), Action::RecOpt(opt), crate::record::option_hint(opt));
    }
    true
}

/// Song position (B:S:T, or M:S:CS: a click toggles, right-click opens the mode menu).
fn time(ui: &mut Ui, c: &Ctx) {
    let (t, a, s, f) = (ui.t, c.a, c.s, c.f);
    let lemon = super::lemon(ui);
    let tp = a.span(745.0, 873.0, 4.0, 36.0);
    if lemon {
        os_ui::kit::lemon::well(ui.c, t, tp, t.time_panel);
    } else {
        ui.c.fill_rect(tp, ui.t.s(0x212829));
        ui.c.fill_rect(tp.inset(1.0), t.time_panel);
    }
    // spec: ui/time-display.md §2-3 (the bar's signature is the project's).
    ui.hit(tp, Action::Clock, s.clock.hint());
    ui.live(Live::Clock, tp);
    let ppq = f64::from(s.project.ppq.max(1));
    // A bar that makes no signature (3 steps per beat) counts its beats as quarters.
    let sig = s.project.signature().unwrap_or((s.project.bar().0, 4));
    let secs = f.tick / ppq * 60.0 / s.project.tick_tempo(s.tempo.max(1.0));
    let (bar, step, tick) = s.clock.reading(secs, f.tick * 96.0 / ppq, sig);
    // Light digits drawn piece by piece (big 36 px condensed to 14, small 24 to 9),
    // right-aligned so long bar numbers grow to the left and never leave the panel.
    let (big, small) = (Font::light(32, 13), Font::light(22, 8));
    let bar_s = bar.to_string();
    let step_s = format!("{step:02}");
    let tick_s = format!("{tick:02}");
    let (bw, sw, kw) = (ui.c.text_width(big, &bar_s), ui.c.text_width(big, &step_s), ui.c.text_width(small, &tick_s));
    let right = a.x(863.0);
    let tick_x = right - kw;
    let step_x = tick_x - 8.0 - sw;
    let bar_x = (step_x - 8.0 - bw).max(tp.x + 2.0);
    ui.c.push_clip(tp.inset(1.0));
    let g = if lemon { t.pill } else { t.glyph };
    if let Some(c) = s.rec.count.filter(|c| c.left() > 0) {
        // A recording count-in (spec: ui/recording.md §3): the beats left.
        ui.c.text_center(Font::light(22, 8), Rect::new(tp.x, tp.y + 2.0, tp.w, tp.h - 2.0), g, &format!("REC IN {}", c.left()));
    } else if lemon {
        // The Lemon skin's own reading: the bar in lemon, the rest in the text colour, a round dot between the parts.
        // While the song plays, the first dot lights on each beat and fades through it.
        use os_theme::design::lemon as brand;
        let lit = t.brand_sel(brand::HIGHLIGHT);
        let (ink, dim) = (t.text, t.text.mix(t.time_panel, 0.5));
        let pulse = if f.playing && !f.paused { 1.0 - (f.tick / ppq).fract() as f32 } else { 0.0 };
        // Each dot sits on its digits' baseline, centred in the gap between them, as a full stop would.
        let (base, small_base) = (a.y(-3.0) + ui.c.ascent(big), a.y(9.0) + ui.c.ascent(small));
        let (r1, r2) = (1.7, 1.4);
        ui.c.text(big, bar_x, a.y(-3.0), lit, &bar_s);
        ui.c.fill_circle((bar_x + bw + step_x) / 2.0, base - r1 - 0.5, r1 + 0.5 * pulse, dim.mix(lit, pulse));
        ui.c.text(big, step_x, a.y(-3.0), ink, &step_s);
        ui.c.fill_circle((step_x + sw + tick_x) / 2.0, small_base - r2 - 0.5, r2, dim);
        ui.c.text(small, tick_x, a.y(9.0), dim, &tick_s);
    } else {
        ui.c.text(big, bar_x, a.y(-3.0), g, &bar_s);
        ui.c.fill_rect(Rect::new(step_x - 5.5, a.y(20.0), 2.0, 2.5), g);
        ui.c.fill_rect(Rect::new(step_x - 5.5, a.y(30.0), 2.0, 2.5), g);
        ui.c.text(big, step_x, a.y(-3.0), g, &step_s);
        ui.c.fill_rect(Rect::new(tick_x - 5.0, a.y(21.0), 2.0, 2.0), g);
        ui.c.fill_rect(Rect::new(tick_x - 5.0, a.y(29.5), 2.0, 2.0), g);
        ui.c.text(small, tick_x, a.y(9.0), g, &tick_s);
    }
    ui.c.pop_clip();
    let label = s.clock.label();
    ui.c.text_right(Font::condensed(7, 3).bold(), a.x(871.0), a.y(5.0), if lemon { t.text_dim } else { t.glyph.shade(-0.1) }, label);
}

/// The output monitor (spec ui/monitor.md): a scope / analyzer panel at x 882..968 and two peak bars at x 970..1047.
fn monitor(ui: &mut Ui, c: &Ctx) {
    use crate::monitor::{Mode, COLS};
    let (a, s) = (c.a, c.s);
    let (edge, fill, ink) = (ui.t.s(0x1d2022), ui.t.s(0x25282a), ui.t.tx(0xb9bfc3));
    let m = &s.monitor;
    // Left panel: border, then 84 x 31 px of picture, bottom edge at y 36. Pictures are drawn as cells, which tile at
    // any scale (spec ui/ui-scaling.md §4).
    let lemon = super::lemon(ui);
    let (fill, ink) = if lemon { (ui.t.time_panel, ui.t.pill.mix(ui.t.time_panel, 0.15)) } else { (fill, ink) };
    let scope = a.r(Rect::new(880.0, 4.0, 86.0, 33.0));
    if lemon {
        os_ui::kit::lemon::well(ui.c, ui.t, scope, fill);
    } else {
        ui.c.fill_rect(scope, edge);
        ui.c.fill_rect(a.r(Rect::new(881.0, 5.0, 84.0, 31.0)), fill);
    }
    let (x0, bottom) = (a.x(881.0), a.y(36.0));
    match m.mode {
        Mode::Spectrum => {
            for col in 0..COLS {
                let h = m.bar(col) * 31.0;
                let whole = h.floor();
                let x = x0 + col as f32;
                if whole > 0.0 {
                    ui.c.fill_cell(Rect::new(x, bottom - whole, 1.0, whole), ink);
                }
                let part = h - whole;
                if part > 0.05 && whole < 31.0 {
                    ui.c.fill_cell(Rect::new(x, bottom - whole - 1.0, 1.0, 1.0), ink.with_alpha((part * 255.0) as u8));
                }
            }
        }
        Mode::Scope => {
            for col in 0..COLS {
                let (lo, hi) = m.wave[col];
                let (y0, y1) = (a.y(20.0) - hi * 15.0, a.y(20.0) - lo * 15.0);
                let (top, bottom) = (y0.min(y1).round(), y0.max(y1).round());
                ui.c.fill_cell(Rect::new(x0 + col as f32, top, 1.0, (bottom - top).max(1.0)), ink);
            }
        }
    }
    ui.hit(scope, Action::Monitor, "Oscilloscope / spectrum analyzer (click to switch)");

    // Right panel: two peak bars (meter range .. 0 dB), light grey fading to cream at the tip, plus the held peak.
    let peaks = a.r(Rect::new(968.0, 4.0, 77.0, 23.0));
    if lemon {
        os_ui::kit::lemon::well(ui.c, ui.t, peaks, fill);
    } else {
        ui.c.fill_rect(peaks, edge);
        ui.c.fill_rect(a.r(Rect::new(969.0, 5.0, 75.0, 21.0)), fill);
    }
    // Classic: both ends follow the theme's highlight key. Lemon: the displays' digit colour (like the scope's ink),
    // dimmed toward the panel at the left and full at the tip. The held peak takes the tip colour.
    let (lo, hi) = if lemon {
        (ui.t.pill.mix(ui.t.time_panel, 0.45), ui.t.pill)
    } else {
        (ui.t.h(0xc8ced1), ui.t.h(0xcdc7a8))
    };
    let x0 = a.x(969.0);
    for i in 0..2 {
        let y = a.y(5.0 + 11.0 * i as f32);
        let hold = (m.meter_pos(m.hold[i].0) * 75.0).round();
        if m.hold[i].1.is_some() && hold >= 1.0 {
            ui.c.fill_rect(Rect::new(x0 + hold - 1.0, y, 1.0, 10.0), hi);
        }
        let len = (m.meter_pos(m.meter_db[i]) * 75.0).round();
        if len < 1.0 {
            continue;
        }
        let mix = |k: f32, shade: f32| {
            let l = |u: u8, v: u8| (((u as f32 + (v as f32 - u as f32) * k) * shade).round() as u32).min(255);
            os_ui::Color::rgb((l(lo.r, hi.r) << 16) | (l(lo.g, hi.g) << 8) | l(lo.b, hi.b))
        };
        for x in 0..len as i32 {
            let k = x as f32 / len.max(1.0);
            // Tip column darker, left column slightly dark; top and bottom rows shaded like a raised bar.
            let side = if x == len as i32 - 1 { 0.58 } else if x == 0 { 0.82 } else { 1.0 };
            let px = x0 + x as f32;
            ui.c.fill_cell(Rect::new(px, y, 1.0, 1.0), mix(k, 0.82 * side));
            ui.c.fill_cell(Rect::new(px, y + 1.0, 1.0, 8.0), mix(k, side));
            ui.c.fill_cell(Rect::new(px, y + 9.0, 1.0, 1.0), mix(k, 0.75 * side));
        }
    }
    ui.hit(peaks, Action::Swallow, "Output level (peak L / R)");
    ui.c.stroke_circle(a.x(1037.0), a.y(33.5), 4.0, 2.0, ui.t.s(0x66737d));
}

/// CPU panel (spec: ui/main-window.md §13): audio load (bar, percent, recent history or the Misc counts), memory,
/// voices (or frames per second), dropouts. The hint is the reading in words (`App::tick`).
fn cpu(ui: &mut Ui, c: &Ctx) {
    let (t, a, s) = (ui.t, c.a, c.s);
    let cpu = a.r(crate::app::CPU_RECT);
    sunken(ui, cpu, t.sunken_dark);
    ui.hit(cpu, Action::CpuPanel, "");
    let r = &s.cpu.now;
    let level = |load: u32| crate::cpu::load_color(t, t.glyph, load);
    let k = (r.load as f32 / 100.0).min(1.0);
    ui.c.fill_rect(a.r(Rect::new(1055.0, 6.0, 1.5, 12.0)), t.glyph.with_alpha(60));
    if k > 0.0 {
        ui.c.fill_cell(a.r(Rect::new(1055.0, 6.0 + 12.0 * (1.0 - k), 1.5, 12.0 * k)), level(r.load));
    }
    ui.c.text_right(Font::new(17), a.x(1136.0), a.y(3.0), level(r.load), &format!("{}%", r.load));
    if let Some(m) = r.misc {
        // Misc: handles, threads, fonts in the history's place.
        ui.c.text(Font::new(13), a.x(1058.0), a.y(20.0), t.glyph, &format!("{} H  {} T  {} F", m.handles, m.threads, m.fonts));
    } else {
        // One 2 px column per reading, newest on the right, full height at 100 %.
        for (i, &h) in s.cpu.history.iter().rev().enumerate() {
            let hh = (11.0 * h as f32 / 100.0).min(11.0).round();
            if hh > 0.0 {
                ui.c.fill_cell(a.r(Rect::new(1128.0 - 2.0 * i as f32, 32.0 - hh, 2.0, hh)), level(h).with_alpha(170));
            }
        }
        ui.c.line(a.x(1055.0), a.y(32.0), a.x(1130.0), a.y(32.0), 1.0, t.glyph.with_alpha(160));
    }
    let mem = r.mem_mb.map_or("- MB".to_string(), |m| format!("{m} MB"));
    ui.c.text_right(Font::condensed(18, 6), a.x(1199.0), a.y(3.0), t.glyph, &mem);
    if s.cpu.fps_view {
        let unit = Font::new(13);
        let uw = ui.c.text_width(unit, "fps");
        ui.c.text_right(unit, a.x(1166.0), a.y(21.0), t.glyph, "fps");
        ui.c.text_right(Font::condensed(18, 6), a.x(1166.0) - uw - 2.0, a.y(17.0), t.glyph, &r.fps.to_string());
    } else {
        ui.c.text_right(Font::condensed(18, 6), a.x(1166.0), a.y(17.0), t.glyph, &r.voices.to_string());
    }
    if r.dropouts > 0 {
        ui.c.text_right(Font::condensed(18, 6), a.x(1199.0), a.y(17.0), t.meter_clip, &format!("!{}", r.dropouts.min(9999)));
    }
}

/// Utility buttons: "left / right" hints, right clicks in `App::toolbar_right` (spec: ui/toolbar-buttons.md §2).
/// The content library's download button is left out (spec: ui/main-window.md §2).
fn utilities(ui: &mut Ui, c: &Ctx) -> bool {
    let (a, s) = (c.a, c.s);
    let utils = [
        (Icon::Undo, "Undo / Undo history (Ctrl+Z)"),
        (Icon::Cut, "Open audio editor / New audio editor"),
        (Icon::Mic, "Quick record"),
        (Icon::Help, "Help index / About..."),
        (Icon::Save, "Save as... / Save new version (Shift+Ctrl+S)"),
        (Icon::SaveRender, "Export audio... (Ctrl+R)"),
        (Icon::Info, "Project info (F11)"),
    ];
    for (i, (icon, hint)) in utils.into_iter().enumerate() {
        let r = a.r(btn(1209.0 + 33.0 * i as f32, 4.0));
        if r.right() > c.fit {
            return false;
        }
        let action = match icon {
            Icon::Undo => Action::Util(Util::Undo),
            Icon::Help => Action::Util(Util::Help),
            Icon::Save => Action::Util(Util::Save),
            // spec: plugins/tesla/requests.md #5: Tesla on the master track.
            Icon::Cut => Action::Util(Util::AudioEditor),
            Icon::SaveRender => Action::MenuItem(0, menus::FILE_EXPORT),
            Icon::Info => Action::MenuItem(menus::OPTIONS_MENU, menus::OPT_INFO),
            // spec: ui/one-click-recording.md §2: lit while it records; a click then stops.
            Icon::Mic => Action::OneClick,
            _ => Action::Placeholder(hint),
        };
        let lit = icon == Icon::Mic && s.rec.audio;
        tool_button(ui, r, icon, lit, action, hint);
    }
    true
}

/// Menu bar.
fn menu_bar(ui: &mut Ui, c: &Ctx) {
    let (t, a, s) = (ui.t, c.a, c.s);
    let mb = a.span(4.0, 296.0, 45.0, 77.0);
    let lemon = super::lemon(ui);
    if lemon {
        os_ui::kit::lemon::panel(ui.c, mb, 6.0, t.menu_panel);
    } else {
        ui.c.fill_rect_gradient(mb, t.menu_panel.shade(-0.04), t.menu_panel.shade(0.02), 2.5);
        ui.c.stroke_rrect(mb, 2.5, 1.0, t.toolbar_line.shade(-0.1));
    }
    let layout = menus::bar_layout(ui.c);
    for (i, (m, (x0, x1, width))) in menus::MENUS.iter().zip(layout).enumerate() {
        let title = m.title;
        let r = a.r(Rect::new(x0 - 3.0, 47.0, x1 - x0 + 6.0, 29.0));
        let hov = ui.hit(r, Action::Menu(i), title);
        let open = s.menu_open == Some(i) && !c.preview;
        if lemon && open {
            os_ui::kit::lemon::lit_in(ui.c, ui.t, r.inset(1.0), 4.0, false);
        } else if hov || open {
            ui.c.fill_rrect(if lemon { r.inset(1.0) } else { r }, if lemon { 4.0 } else { 2.0 }, t.menu_drop_hi);
        }
        ui.c.text(Font::condensed(menus::BAR_FONT.size, width as u16), a.x(x0), a.y(51.0), if lemon && open { t.pill_text } else { t.menu_text }, title);
    }
}

/// Main volume (project value; spec: audio/mixer.md "Main volume").
fn volume(ui: &mut Ui, c: &Ctx) {
    use crate::knobs::KnobId;
    let (t, a, s) = (ui.t, c.a, c.s);
    ui.hit(a.span(303.0, 337.0, 45.0, 77.0), Action::Knob(KnobId::MasterVol), s.knob_text(KnobId::MasterVol));
    super::knob(ui, a.x(319.0), a.y(61.0), 14.0, s.knob(KnobId::MasterVol), false, t.glyph, t.button_hi);
}

/// Main pitch (project value; spec: audio/voice-levels.md).
fn pitch(ui: &mut Ui, c: &Ctx) {
    use crate::knobs::KnobId;
    let (t, a, s) = (ui.t, c.a, c.s);
    ui.hit(a.span(340.0, 376.0, 45.0, 77.0), Action::Knob(KnobId::MasterPitch), s.knob_text(KnobId::MasterPitch));
    let (cx, cy) = (a.x(358.0), a.y(61.0));
    super::knob(ui, cx, cy, 14.0, s.knob(KnobId::MasterPitch), true, t.glyph, t.button_hi);
    for k in 0..20 {
        let ang = 0.75 * std::f32::consts::PI + 1.5 * std::f32::consts::PI * k as f32 / 19.0;
        ui.c.fill_circle(cx + 17.5 * ang.cos(), cy + 17.5 * ang.sin(), 0.7, t.glyph_dim);
    }
}

/// Song position slider (spec: ui/song-position-slider.md): the handle follows the playhead while playing.
fn song_pos(ui: &mut Ui, c: &Ctx) {
    let (t, a, s, f) = (ui.t, c.a, c.s, c.f);
    let lemon = super::lemon(ui);
    let sp = a.r(crate::song_pos::PANEL);
    let range = crate::song_pos::range(s);
    let pos = crate::song_pos::shown(s, f.playing, f.song, f.tick, f.song_pos_drag);
    ui.live(Live::Clock, sp);
    match range {
        Some(_) => {
            ui.hit(sp, Action::SongPos, crate::song_pos::hint(s, pos));
        }
        // An empty playlist in SONG mode: nothing to move through.
        None => {
            ui.hit(sp, Action::Swallow, "Song position: the arranger is empty");
        }
    }
    let hx = range.map(|len| a.x(crate::song_pos::handle_x(pos, len).round()));
    let y = |v: f32| a.y(v);
    if lemon {
        let x0 = a.x(390.0);
        ui.c.fill_rrect(Rect::new(x0, y(60.0), 180.0, 4.0), 2.0, t.sunken_dark);
        match hx {
            Some(hx) => {
                use os_theme::design::lemon as brand;
                ui.c.fill_rrect(Rect::new(x0, y(60.0), hx - x0, 4.0), 2.0, t.brand_lit(brand::MAIN));
                ui.c.fill_rrect(Rect::new(hx - 6.0, y(55.0), 12.0, 14.0), 3.0, t.button);
                ui.c.fill_rect(Rect::new(hx - 1.0, y(58.0), 2.0, 8.0), t.brand_lit(brand::MAIN));
            }
            None => ui.c.fill_rect(a.r(Rect::new(394.0, 58.0, 2.0, 8.0)), t.button),
        }
    } else {
        // Classic: the colours of §1.
        let x0 = a.x(394.0);
        ui.c.fill_rect(Rect::new(x0, y(60.0), 172.0, 4.0), ui.t.s(0x343739));
        match hx {
            Some(hx) => {
                let green = ui.t.a(0xa8e44a);
                ui.c.fill_rect(Rect::new(x0, y(60.0), hx - x0, 4.0), green);
                ui.c.fill_circle(hx + 0.5, y(62.0), 6.5, ui.t.s(0x1c1f21));
                ui.c.fill_circle(hx + 0.5, y(62.0), 5.5, ui.t.s(0x26292b));
                ui.c.fill_rect(Rect::new(hx, y(56.0), 1.0, 2.0), ui.t.s(0x9a9d9f));
                ui.c.fill_rect(Rect::new(hx, y(58.0), 1.0, 7.0), green);
                ui.c.fill_rect(Rect::new(hx, y(65.0), 1.0, 3.0), ui.t.s(0x686b6d));
            }
            None => ui.c.fill_rect(a.r(Rect::new(395.0, 56.0, 1.0, 12.0)), ui.t.s(0x282b2d)),
        }
    }
}

/// Mode buttons (spec: ui/main-window.md §3). Typing keyboard, auto scrolling (spec: ui/auto-scrolling.md), step edit
/// (spec: ui/step-edit.md), enable groups (spec: ui/enable-groups.md) and multilink (spec: ui/remote-control.md §4)
/// work.
/// The mode buttons: icon, whether on, what a click does, hint.
fn mode_buttons(s: &State) -> [(Icon, bool, Action, &'static str); 5] {
    [
        (Icon::TypingKeyboard, s.toggles.contains(&"typing"), Action::Typing, "Computer MIDI keyboard (Ctrl+T)"),
        (Icon::AutoScroll, crate::autoscroll::on(), Action::AutoScroll, "Auto scrolling - right-click for continuous scrolling"),
        (Icon::StepEdit, crate::step_edit::on(), Action::StepEdit, "Step editing (Ctrl+E)"),
        (Icon::Link, crate::enable_groups::on(), Action::EnableGroups, "Grouped clips move together (Shift+Alt+G)"),
        (Icon::MidiControl, s.multilink.on, Action::Multilink, "Multi MIDI learn (Ctrl+J)"),
    ]
}

fn modes(ui: &mut Ui, c: &Ctx) -> bool {
    for (i, (icon, on, action, hint)) in mode_buttons(c.s).into_iter().enumerate() {
        let r = c.a.r(btn(580.0 + 33.0 * i as f32, 45.0));
        if r.right() > c.fit {
            return false;
        }
        tool_button(ui, r, icon, on, action, hint);
    }
    true
}

/// Snap panel (spec: ui/global-snap.md §1): the magnet turns snapping on / off, the combo holds the global snap.
fn snap(ui: &mut Ui, c: &Ctx) {
    let (t, a, s) = (ui.t, c.a, c.s);
    let lemon = super::lemon(ui);
    let global = s.global_snap.now;
    let on = global != crate::playlist::Snap::None;
    ui.hit(a.span(746.0, 767.0, 50.0, 72.0), Action::GlobalSnapToggle, crate::snap::MAGNET_HINT);
    let magnet = if lemon {
        if on { t.glyph_dim } else { t.glyph_dim.mix(t.toolbar, 0.55) }
    } else {
        t.tx(if on { 0x8b959c } else { 0x52585c })
    };
    icons::draw(ui.c, Icon::Magnet, a.span(748.0, 766.0, 52.0, 70.0), magnet);
    let snap = a.span(768.0, 830.0, 50.0, 72.0);
    let hov = ui.hit(snap, Action::GlobalSnap, crate::snap::HINT);
    if lemon {
        os_ui::kit::lemon::well(ui.c, t, snap, if hov { t.sunken.mix(t.button_hi, 0.5) } else { t.sunken });
    } else {
        ui.c.fill_rrect(snap, 2.0, t.sunken_dark);
    }
    // The longest labels ("1/6 step") are narrowed to fit before the arrow.
    let label = global.label();
    let room = snap.w - 5.0 - 14.0;
    let font = if ui.c.text_width(Font::new(16), label) <= room { Font::new(16) } else { Font::condensed(16, 6) };
    ui.c.text(font, snap.x + 5.0, a.y(51.0), t.glyph, label);
    // The arrow points down while the mouse is over the combo (§1).
    let dir = if hov { os_ui::kit::Dir::Down } else { os_ui::kit::Dir::Right };
    os_ui::kit::triangle(ui.c, snap.right() - 8.0, a.y(61.0), 3.5, dir, t.glyph_dim);
}

/// Pattern selector (to the "+", x 962).
fn pattern(ui: &mut Ui, c: &Ctx) {
    let (t, a, s) = (ui.t, c.a, c.s);
    let lemon = super::lemon(ui);
    let prev = a.span(838.0, 851.0, 47.0, 76.0);
    ui.hit(prev, Action::PatternPrev, "Pattern options"); // spec: ui/pattern-menu.md §1
    ui.c.fill_rrect(prev, if lemon { 4.0 } else { 2.0 }, t.button);
    icons::draw(ui.c, Icon::ArrowRight, prev.inset(2.0), t.glyph);
    let pb = a.span(855.0, 948.0, 47.0, 76.0);
    let hov = ui.hit(pb, Action::PatternNext, crate::patterns::name_of(s, s.pattern.max(1)));
    if lemon {
        os_ui::kit::lemon::well(ui.c, t, pb, if hov { t.sunken.mix(t.button_hi, 0.5) } else { t.sunken });
    } else {
        ui.c.fill_rrect(pb, 2.0, t.tempo_box.shade(if hov { 0.16 } else { 0.08 }));
    }
    let name = s
        .project
        .patterns
        .iter()
        .find(|p| p.number == s.pattern)
        .and_then(|p| p.name.clone())
        .unwrap_or_else(|| format!("Pattern {}", s.pattern));
    // Right-aligned before the spin arrow; a name too long for that keeps its start and is cut at the arrow.
    let pf = Font::condensed(21, 7);
    let end = pb.right() - 11.0;
    if ui.c.text_width(pf, &name) <= end - (pb.x + 2.0) {
        ui.c.push_clip(pb.inset(2.0));
        ui.c.text_right(pf, end, a.y(49.0), t.glyph, &name);
        ui.c.pop_clip();
    } else {
        ui.c.text_clip(pf, pb.x + 4.0, a.y(49.0), end - (pb.x + 4.0), t.glyph, &name);
    }
    let add = a.span(951.0, 962.0, 50.0, 72.0);
    ui.hit(add, Action::PatternAdd, "New pattern (find first / next empty)");
    icons::draw(ui.c, Icon::Plus, add, t.glyph);
}

/// Window buttons `which` of the row of nine (spec: ui/toolbar-buttons.md §2; right clicks in `App::toolbar_right`):
/// the window buttons are the first five, the pickers and tools the rest.
fn window_buttons(ui: &mut Ui, c: &Ctx, which: Range<usize>) -> bool {
    let a = c.a;
    for (i, (icon, action, hint)) in WINDOW_BUTTONS.into_iter().enumerate().filter(|(i, _)| which.contains(i)) {
        let r = a.r(btn(973.0 + 33.0 * i as f32, 45.0));
        if r.right() > c.fit {
            return false;
        }
        let (hov, down) = (ui.hit(r, action.clone(), hint), ui.down(r, &action));
        // The tiling layout: a window's button is lit while it is open, and a hover only glows (as the metronome's).
        let look = if c.s.bar.tiling {
            let open = match action {
                Action::ToggleWin(Win::Browser) => c.s.show_browser,
                Action::ToggleWin(w) => c.s.shown(w),
                _ => false,
            };
            if down { super::Look::Hot } else if open { super::Look::On } else if hov { super::Look::Hover } else { super::Look::Off }
        } else if hov || down {
            super::Look::Hot
        } else {
            super::Look::Off
        };
        let ink = super::tool_face(ui, r, look);
        icons::draw(ui.c, icon, Rect::new(r.x + 5.0, r.y + 6.5, 20.0, 20.0), ink);
    }
    // The news panel (x 1304..1470, with its separator) is left out: it suggested an online service Lemon Studio
    // does not have. A tips panel may take its place (spec: ui/main-window.md §3).
    true
}

/// The row of nine: the window buttons (the first five), then the pickers and tools.
const WINDOW_BUTTONS: [(Icon, Action, &str); 9] = [
        (Icon::Playlist, Action::ToggleWin(Win::Playlist), "Arranger / Maximize arranger (F5)"),
        (Icon::PianoRoll, Action::ToggleWin(Win::PianoRoll), "Piano roll / Maximize Piano roll (F7)"),
        (Icon::ChannelRack, Action::ToggleWin(Win::ChannelRack), "Instrument rack (F6)"),
        (Icon::Mixer, Action::ToggleWin(Win::Mixer), "Mixer (F9)"),
        (Icon::Browser, Action::ToggleWin(Win::Browser), "Browser / Plugin palette (Alt+F8)"),
        (Icon::Project, Action::ProjectPicker, "Clip palette / Plugin palette (Ctrl+F8)"),
        (Icon::Plug, Action::PickerOpen, "Plugin palette (F8)"),
        (Icon::Kick, Action::ToggleWin(Win::Tapper), "Tap tempo (Ctrl+F5)"),
    (Icon::Touch, Action::ToggleWin(Win::Touch), "Touch controller (Alt+F7)"),
    // The store button is hidden: its stand-in, a repository of free and open-source plugins (spec:
    // ui/main-window.md §3), had the plugin picker's icon. It is to come back with a shop icon.
];

/// The tiling layout's status bar along the window's bottom (General settings ▸ Layout; `super::status_rect`): Lemon
/// Studio's mark, the project's title and the hint at the left; the global snap, the mode buttons, the pickers and
/// tools, and the CPU load and memory at the right. These leave the toolbar, which the tiling layout keeps to one row.
pub fn paint_status_bar(ui: &mut Ui, s: &State, f: &FrameInfo) {
    let t = ui.t;
    let r = super::status_rect(s);
    ui.hit(r, Action::Swallow, "");
    ui.c.fill_rect(r, t.toolbar);
    ui.c.fill_rect(Rect::new(r.x, r.y, r.w, 1.0), t.toolbar_edge);
    let cy = r.cy() + 0.5;
    icons::logo(ui.c, r.x + 15.0, cy, 8.0, super::CUTTING_EDGE_RED);
    // the right side, from the right edge leftwards
    let mut x = r.right() - 8.0;
    let small = Font::new(13);
    let cpu = &s.cpu.now;
    let level = crate::cpu::load_color(t, t.glyph, cpu.load);
    let mem = cpu.mem_mb.map_or("- MB".to_string(), |m| format!("{m} MB"));
    let load = format!("{}%", cpu.load);
    // Slots as wide as the widest reading: the bar to their left stays put while the numbers change length.
    let mw = ui.c.text_width(small, &mem).max(ui.c.text_width(small, "88888 MB"));
    let lw = ui.c.text_width(small, &load).max(ui.c.text_width(small, "100%"));
    let ty = (cy - ui.c.line_height(small) / 2.0).round();
    ui.c.text_right(small, x, ty, t.text_dim, &mem);
    ui.c.text_right(small, x - mw - 10.0, ty, level, &load);
    // The dropouts since the program started (as the classic CPU panel counts them), in red, in a slot of their own
    // left of the load (empty while there are none).
    let drops = cpu.dropouts;
    let dl = format!("!{}", drops.min(9999));
    let dw = ui.c.text_width(small, &dl).max(ui.c.text_width(small, "!888"));
    let dx = x - mw - 10.0 - lw - 8.0;
    if drops > 0 {
        // A red pill: it should catch the eye.
        let tw = ui.c.text_width(small, &dl);
        let pill = Rect::new((dx - tw - 6.0).round(), (cy - 9.0).round(), (tw + 12.0).round(), 18.0);
        ui.c.fill_rrect(pill, 9.0, t.meter_clip);
        ui.c.text_right(small, dx, ty, t.s(0xffffff), &dl);
    }
    let cpu_r = Rect::new(dx - dw - 4.0, r.y, mw + lw + dw + 26.0, r.h);
    let hint = match drops {
        0 => "CPU load and memory, no dropouts - double-click for the performance monitor".to_string(),
        1 => "CPU load and memory, 1 dropout since the start - double-click for the performance monitor".to_string(),
        n => format!("CPU load and memory, {n} dropouts since the start - double-click for the performance monitor"),
    };
    ui.hit(cpu_r, Action::CpuPanel, hint);
    x = cpu_r.x - 10.0;
    let mut button = |ui: &mut Ui, x: &mut f32, icon: Icon, on: bool, action: Action, hint: &'static str| {
        let b = Rect::new(*x - 24.0, (cy - 11.0).round(), 24.0, 22.0);
        let hov = ui.hit(b, action.clone(), hint);
        let look = if on { super::Look::On } else if ui.down(b, &action) { super::Look::Hot } else if hov { super::Look::Hover } else { super::Look::Off };
        let ink = super::tool_face(ui, b, look);
        icons::draw(ui.c, icon, b.inset(4.0), ink);
        *x = b.x - 2.0;
    };
    for (icon, action, hint) in WINDOW_BUTTONS[5..].iter().rev().cloned() {
        let on = matches!(action, Action::ToggleWin(w) if s.z.contains(&w));
        button(ui, &mut x, icon, on, action, hint);
    }
    x -= 8.0;
    split_button(ui, s, &mut x, cy);
    x -= 8.0;
    for (icon, on, action, hint) in mode_buttons(s).into_iter().rev() {
        button(ui, &mut x, icon, on, action, hint);
    }
    x -= 8.0;
    // the global snap: its value (a click opens its menu), and the magnet before it
    let global = s.global_snap.now;
    let label = global.label();
    let sw = ui.c.text_width(small, label) + 18.0;
    let sr = Rect::new(x - sw, (cy - 10.0).round(), sw, 20.0);
    let hov = ui.hit(sr, Action::GlobalSnap, crate::snap::HINT);
    os_ui::kit::lemon::well(ui.c, t, sr, if hov { t.sunken.mix(t.button_hi, 0.5) } else { t.sunken });
    ui.c.text(small, sr.x + 5.0, ty, t.glyph, label);
    os_ui::kit::triangle(ui.c, sr.right() - 7.0, cy, 3.0, os_ui::kit::Dir::Down, t.glyph_dim);
    let mr = Rect::new(sr.x - 26.0, (cy - 11.0).round(), 24.0, 22.0);
    let on = global != crate::playlist::Snap::None;
    ui.hit(mr, Action::GlobalSnapToggle, crate::snap::MAGNET_HINT);
    icons::draw(ui.c, Icon::Magnet, mr.inset(4.0), if on { t.glyph_dim } else { t.glyph_dim.mix(t.toolbar, 0.55) });
    // the left side: the title, then the hint, up to what the right side took
    let left = r.x + 32.0;
    let room = (mr.x - 16.0 - left).max(0.0);
    let hint: &str = if !f.hint.is_empty() { &f.hint } else { s.message.as_deref().unwrap_or("") };
    let title_w = ui.c.text_width(small, &f.title).min(room * 0.4);
    ui.c.text_clip(small, left, ty, title_w, t.text_dim.shade(-0.05), &f.title);
    let hx = left + title_w + if f.title.is_empty() { 0.0 } else { 14.0 };
    ui.c.text_clip(small, hx, ty, (left + room - hx).max(0.0), t.text, hint);
}

/// The status bar's split button (the tiling layout's split views, `crate::tiles`): its glyph is the preset in use,
/// a click takes the next.
fn split_button(ui: &mut Ui, s: &State, x: &mut f32, cy: f32) {
    let b = Rect::new(*x - 24.0, (cy - 11.0).round(), 24.0, 22.0);
    let p = &crate::tiles::PRESETS[s.tiles.preset];
    let hint = format!("Split: {} - click for the next", p.name);
    let hov = ui.hit(b, Action::TilePreset, hint);
    let look = if ui.down(b, &Action::TilePreset) { super::Look::Hot } else if hov { super::Look::Hover } else { super::Look::Off };
    let ink = super::tool_face(ui, b, look);
    // three panes, a px apart: the single one across, the pair split the other way
    let g = Rect::new(b.x + 5.0, b.y + 5.0, 14.0, 12.0);
    let panes = if p.outer_rows {
        [Rect::new(g.x, g.y, 14.0, 5.0), Rect::new(g.x, g.y + 6.0, 6.5, 6.0), Rect::new(g.x + 7.5, g.y + 6.0, 6.5, 6.0)]
    } else {
        [Rect::new(g.x, g.y, 8.0, 5.5), Rect::new(g.x, g.y + 6.5, 8.0, 5.5), Rect::new(g.x + 9.0, g.y, 5.0, 12.0)]
    };
    for pane in panes {
        ui.c.fill_rrect(pane, 1.0, ink);
    }
    *x = b.x - 2.0;
}

/// The editor's outline around something that can be dragged.
fn frame(ui: &mut Ui, r: Rect) {
    let c = ui.t.glyph_dim.with_alpha(130);
    ui.c.stroke_rrect(r, 3.0, 1.0, c);
}

/// While editing, a panel on the toolbar is one thing to drag: its controls' hits are dropped (`Ui::forget`) for one of
/// its own, and it is framed. The spaces show as hatched boxes.
fn item_frame(ui: &mut Ui, l: &crate::toolbar::Layout, p: &Placed) {
    let r = p.rect(l);
    let name = p.panel.name();
    ui.hit(r, Action::Bar(BarHit::Item(p.row, p.idx)), format!("{name} - drag to move it, onto the palette to take it off"));
    if p.panel.repeats() {
        space_picture(ui, r.inset(3.0), p.panel);
    }
    frame(ui, r.inset(1.0));
}

/// A space: a hatched box, a flexible one with arrows to both sides.
fn space_picture(ui: &mut Ui, r: Rect, p: Panel) {
    let t = ui.t;
    ui.c.push_clip(r);
    let mut x = r.x - r.h;
    while x < r.right() {
        ui.c.line(x, r.bottom(), x + r.h, r.y, 1.0, t.glyph_dim.with_alpha(60));
        x += 6.0;
    }
    ui.c.pop_clip();
    if p == Panel::Flex && r.w >= 24.0 {
        use os_ui::kit::{triangle, Dir};
        triangle(ui.c, r.x + 6.0, r.cy(), 3.5, Dir::Left, t.glyph_dim);
        triangle(ui.c, r.right() - 6.0, r.cy(), 3.5, Dir::Right, t.glyph_dim);
    }
}

/// While editing, each row starts with a grip (a column of short dashes) that drags the row.
fn grips(ui: &mut Ui, s: &State) {
    let t = ui.t;
    let l = &s.bar.layout;
    for row in 0..l.rows.len() {
        let (top, bottom) = l.row_span(row);
        let r = Rect::new(0.0, top, crate::toolbar::GRIP_W, bottom - top);
        let hov = ui.hit(r, Action::Bar(BarHit::Grip(row)), "Row - drag onto the other row to swap them");
        let c = if hov { t.glyph } else { t.glyph_dim };
        let mut y = top + 6.0;
        while y + 2.0 <= bottom - 5.0 {
            ui.c.fill_rect(Rect::new(3.0, y, 4.0, 1.0), c);
            y += 4.0;
        }
    }
}

/// Palette spacing (band px): padding, the gap between tiles, the label under a tile, a line of tiles, the bottom bar.
const PAD: f32 = 10.0;
const GAP: f32 = 14.0;
const LABEL_H: f32 = 16.0;
const LINE_H: f32 = ROW_H + LABEL_H + 8.0;
const BAR_H: f32 = 30.0;

/// The editor's palette under the band (band px): where it is, its tiles (each the picture's rect, its name under it)
/// and its bottom bar.
pub struct PaletteLayout {
    pub rect: Rect,
    pub tiles: Vec<(Tile, Rect)>,
    pub bar: Rect,
}

/// A tile's picture width.
fn tile_w(t: Tile) -> f32 {
    match t {
        Tile::Panel(p) => p.width().max(64.0),
        Tile::Logo => 92.0,
    }
}

/// The palette of a band `width` px wide: every panel and the logo as tiles, line after line, the bottom bar below.
pub fn palette(s: &State, width: f32) -> PaletteLayout {
    let top = s.bar.layout.band_h();
    let (mut x, mut y) = (PAD, top + PAD);
    let mut tiles = Vec::new();
    for tile in PANELS.iter().map(|&p| Tile::Panel(p)).chain([Tile::Logo]) {
        let w = tile_w(tile);
        if x > PAD && x + w > width - PAD {
            x = PAD;
            y += LINE_H;
        }
        tiles.push((tile, Rect::new(x, y, w, ROW_H)));
        x += w + GAP;
    }
    let bar = Rect::new(0.0, y + ROW_H + LABEL_H + PAD, width, BAR_H);
    PaletteLayout { rect: Rect::new(0.0, top, width, bar.bottom() - top), tiles, bar }
}

/// Whether tile `tile` is on the toolbar already (dimmed: it cannot be added twice).
fn placed_already(s: &State, tile: Tile) -> bool {
    match tile {
        Tile::Panel(p) => !p.repeats() && s.bar.layout.has(p),
        Tile::Logo => s.bar.layout.logo,
    }
}

/// The toolbar editor's palette (spec: ui/toolbar-menu.md §8), right under the band, the window's width: a tile for
/// every panel (a picture of it, its name under it), dimmed when it is on the toolbar already; at the bottom, what to
/// do, the layout's preset with its menu, the rows and Done. Then what a drag carries, under the mouse.
pub fn paint_palette(ui: &mut Ui, s: &State, f: &FrameInfo, width: f32) {
    let t = ui.t;
    let p = palette(s, width);
    let bg = t.toolbar.shade(-0.06);
    ui.c.fill_rect(p.rect, bg);
    ui.c.fill_rect(Rect::new(0.0, p.rect.bottom() - 1.0, width, 1.0), t.toolbar_line);
    ui.hit(p.rect, Action::Bar(BarHit::Palette), "");
    for &(tile, r) in &p.tiles {
        let name = match tile {
            Tile::Panel(p) => p.name(),
            Tile::Logo => "Logo",
        };
        ui.c.fill_rrect(r, 3.0, t.toolbar);
        ui.c.push_clip(r);
        match tile {
            Tile::Panel(pn) if pn.repeats() => space_picture(ui, r.inset(4.0), pn),
            Tile::Panel(pn) => {
                let (row, x0, x1) = pn.home();
                // Centred in a tile wider than the panel.
                let dx = ((r.w - (x1 - x0)) / 2.0).floor();
                let a = At { dx: r.x + dx - x0, dy: r.y - ROW_H * row as f32 };
                let mark = ui.mark();
                panel(ui, &Ctx { s, f, a, fit: f32::INFINITY, preview: true }, pn);
                ui.forget(mark);
            }
            Tile::Logo => icons::logo(ui.c, r.cx(), r.cy(), 13.0, if super::lemon(ui) { t.toolbar_edge } else { t.toolbar_line }),
        }
        ui.c.pop_clip();
        let label = Rect::new(r.x - GAP / 2.0, r.bottom() + 2.0, r.w + GAP, LABEL_H - 2.0);
        if placed_already(s, tile) {
            ui.veil(r.outset(1.0), bg);
            ui.hit(r, Action::Bar(BarHit::Palette), format!("{name} is on the toolbar"));
            ui.c.text_center(Font::new(12), label, t.text_dim.mix(bg, 0.5), name);
        } else {
            let hov = ui.hit(r, Action::Bar(BarHit::Tile(tile)), format!("{name} - drag onto the toolbar"));
            frame(ui, r.outset(1.0));
            ui.c.text_center(Font::new(12), label, if hov { t.text } else { t.text_dim }, name);
        }
    }

    // The bottom bar.
    let b = p.bar;
    ui.c.fill_rect(b, t.toolbar.shade(-0.1));
    ui.c.fill_rect(Rect::new(0.0, b.y, width, 1.0), t.toolbar_line);
    let font = Font::new(13);
    let text = "Drag panels to and from the toolbar";
    ui.c.text(font, PAD, b.y + 7.0, t.text_dim, text);
    let x = PAD + ui.c.text_width(font, text) + 24.0;
    let name = format!("Preset: {}", s.bar.name());
    let pr = Rect::new(x, b.y + 4.0, ui.c.text_width(font, &name) + 30.0, BAR_H - 8.0);
    let hov = ui.hit(pr, Action::Bar(BarHit::Presets), "The toolbar presets, Save preset as... and Delete preset");
    os_ui::kit::combo(ui.c, t, pr, &name, font, os_ui::kit::State::hover(hov));
    let done = Rect::new(width - PAD - 70.0, b.y + 4.0, 70.0, BAR_H - 8.0);
    let hov = ui.hit(done, Action::Bar(BarHit::Done), "Done: the toolbar keeps this layout");
    os_ui::kit::button(ui.c, t, done, os_ui::kit::State::hover(hov));
    ui.c.text_center(font, done, t.text, "Done");
    let rows = s.bar.layout.rows.len();
    for (i, label) in ["1 row", "2 rows"].into_iter().enumerate() {
        let r = Rect::new(done.x - 24.0 - 64.0 * (2 - i) as f32, b.y + 5.0, 60.0, BAR_H - 10.0);
        let hov = ui.hit(r, Action::Bar(BarHit::Rows(i + 1)), format!("The toolbar in {label}"));
        os_ui::kit::pill(ui.c, t, r, label, Font::new(12), os_ui::kit::State::new(hov, false, rows == i + 1));
    }

    if let Some(d) = f.bar_drag {
        ghost(ui, s, &d, &p, width);
    }
}

/// What a drag carries, under the mouse, and where it would land: a caret between the panels of a row.
fn ghost(ui: &mut Ui, s: &State, d: &crate::toolbar::BarDrag, p: &PaletteLayout, width: f32) {
    let t = ui.t;
    let l = &s.bar.layout;
    let (name, w, h) = match d.moving {
        Moving::Item(r, i) => match l.rows.get(r).and_then(|row| row.get(i)) {
            Some(&pn) => (pn.name(), pn.width().max(40.0), ROW_H),
            None => return,
        },
        Moving::Tile(tile @ Tile::Panel(pn)) => (pn.name(), tile_w(tile), ROW_H),
        Moving::Tile(Tile::Logo) | Moving::Logo => ("Logo", tile_w(Tile::Logo), ROW_H),
        Moving::Row(_) => ("Row", width, ROW_H),
    };
    let (mx, my) = ui.mouse;
    let r = Rect::new(mx - d.grab.0, my - d.grab.1, w, h);
    ui.c.fill_rrect(r, 4.0, t.pill.with_alpha(50));
    ui.c.stroke_rrect(r, 4.0, 1.5, t.pill);
    ui.c.text_center(Font::new(13), r, t.text, name);
    let to = crate::toolbar::drop_at(&s.bar, width, d, (mx, my), (p.rect.y, p.rect.bottom()));
    let Target::Insert { row, idx } = to else { return };
    if matches!(d.moving, Moving::Row(_) | Moving::Logo | Moving::Tile(Tile::Logo)) {
        return;
    }
    let skip = match d.moving {
        Moving::Item(r, i) => Some((r, i)),
        _ => None,
    };
    let others: Vec<Placed> = place(l, width, true).into_iter().filter(|q| q.row == row && Some((q.row, q.idx)) != skip).collect();
    let x = others.get(idx).map_or_else(|| others.last().map_or(crate::toolbar::GRIP_W, |q| q.x + q.w), |q| q.x);
    let (top, bottom) = l.row_span(row);
    ui.c.fill_rect(Rect::new(x - 1.0, top + 2.0, 2.0, bottom - top - 4.0), t.pill);
}

/// Minutes, seconds and centiseconds of a position in seconds.
pub fn msc_ticks(secs: f64) -> (u32, u32, u32) {
    let cs = (secs.max(0.0) * 100.0) as u32;
    (cs / 6000, cs / 100 % 60, cs % 100)
}

/// Bar:step:tick (1-based bar and step, 16 steps per bar, `ppq / 4` ticks per step).
#[cfg(test)]
pub fn bst(seconds: f64, tempo: f64, ppq: u32) -> (u32, u32, u32) {
    bst_ticks(seconds * tempo / 60.0 * f64::from(ppq))
}

/// Bar:step:tick of a position given in ticks of a 96-per-quarter clock (the display's own resolution).
pub fn bst_ticks(ticks: f64) -> (u32, u32, u32) {
    let ticks = ticks.max(0.0) as u32;
    let per_step = 96 / 4;
    let steps = ticks / per_step;
    (steps / 16 + 1, steps % 16 + 1, ticks % per_step)
}

#[cfg(test)]
mod tests {
    use super::bst;

    #[test]
    fn bar_step_tick() {
        assert_eq!(bst(0.0, 120.0, 96), (1, 1, 0));
        // 120 BPM: one bar (4 beats) = 2 s.
        assert_eq!(bst(2.0, 120.0, 96), (2, 1, 0));
        assert_eq!(bst(0.125, 120.0, 96), (1, 2, 0));
    }

    use super::{room, MORE_HINT, MORE_W};
    use crate::app::{Action, App, Win};
    use crate::prefs::{choice, set_choice, TOOLBARS_SCALING};
    use os_ui::{Canvas, Rect};

    /// The classic layout's width.
    const FULL_W: f32 = 1440.0;

    fn hit(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn has(app: &App, a: &Action) -> bool {
        app.hits.iter().any(|h| &h.action == a)
    }

    fn click(app: &mut App, c: &mut Canvas, x: f32, y: f32) {
        app.mouse_move(x, y);
        app.mouse_down();
        app.mouse_up();
        app.paint(c, None);
    }

    /// On a tiling desktop the tiling layout's toolbar has no caption buttons, its flexible spaces taking their room;
    /// a floating window, the classic layout and the toolbar editor keep them.
    #[test]
    fn a_tiling_desktop_takes_the_caption_buttons() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        let captions = |app: &mut App, c: &mut Canvas| {
            app.paint(c, None);
            [Action::Minimize, Action::Maximize, Action::Close].iter().filter(|a| has(app, a)).count()
        };
        app.state.bar.tiling = true;
        app.state.bar.layout = crate::toolbar::PRESETS[crate::toolbar::TILING_PRESET].layout();
        assert_eq!(captions(&mut app, &mut c), 3, "not a tiling desktop");
        let flex = |app: &App| crate::views::toolbar_placed(&app.state).iter().filter(|p| p.panel == crate::toolbar::Panel::Flex).map(|p| p.w).sum::<f32>();
        let wide = flex(&app);
        crate::desktop::set_tiling(true, false);
        assert_eq!(captions(&mut app, &mut c), 0);
        assert_eq!(flex(&app), wide + crate::toolbar::Panel::Caption.width(), "the spaces take their room");
        assert!(app.state.bar.layout.has(crate::toolbar::Panel::Caption), "kept in the layout");
        app.state.bar.editing = true;
        assert!(super::shown(&app.state).has(crate::toolbar::Panel::Caption), "the editor shows them");
        app.state.bar.editing = false;
        crate::desktop::set_tiling(true, true);
        assert_eq!(captions(&mut app, &mut c), 3, "a floating window");
        crate::desktop::set_tiling(true, false);
        app.state.bar.tiling = false;
        app.state.bar.layout = crate::toolbar::Layout::classic();
        assert_eq!(captions(&mut app, &mut c), 3, "the classic layout");
        crate::desktop::set_tiling(false, false);
    }

    fn twice(r: Rect) -> Rect {
        Rect::new(r.x * 2.0, r.y * 2.0, r.w * 2.0, r.h * 2.0)
    }

    /// Toolbars scaling (spec: ui/ui-scaling.md §1), chosen on the settings page: at 200 % on a 100 % window the top band
    /// is twice as tall at once and its controls twice as big; the browser, the workspace and the windows in it keep
    /// their places below the band.
    #[test]
    fn toolbars_scaling_scales_the_top_band() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.show_browser = true;
        app.paint(&mut c, None);
        assert_eq!(choice(&TOOLBARS_SCALING), "Main");
        assert_eq!((crate::views::band_h(&app.state), app.state.work_rect().y), (83.0, 83.0));
        let (clock, tab) = (hit(&app, &Action::Clock), hit(&app, &Action::BrowserTab(1)));
        assert_eq!(app.state.win_rect(Win::Playlist).y, 83.0, "docked under the band");

        app.open_settings(crate::views::settings::GENERAL);
        app.paint(&mut c, None);
        let combo = app
            .hits
            .iter()
            .rev()
            .find(|h| matches!(&h.action, Action::Settings(crate::views::settings::SAct::Combo(l, ..)) if *l == TOOLBARS_SCALING.label))
            .map(|h| h.rect)
            .expect("the combo works");
        click(&mut app, &mut c, combo.cx(), combo.cy());
        let labels: Vec<String> = app.state.playlist.popup.as_ref().expect("its menu").items.iter().map(|it| it.label.to_string()).collect();
        assert_eq!(labels, TOOLBARS_SCALING.choices);
        let item = hit(&app, &Action::PlPopup(5));
        click(&mut app, &mut c, item.cx(), item.cy());
        assert_eq!((choice(&TOOLBARS_SCALING), app.state.message.as_deref()), ("200%", Some("Toolbars scaling: 200%")));
        app.state.settings.open = false;
        app.paint(&mut c, None);

        // The band: 166 px instead of 83, its controls doubled; the rest keeps its place under it.
        assert_eq!((crate::views::band_h(&app.state), app.state.work_rect().y), (166.0, 166.0));
        assert_eq!(hit(&app, &Action::Clock), twice(clock));
        let drag = app.hits.iter().find(|h| h.action == Action::DragWindow).map(|h| h.rect);
        assert_eq!(drag, Some(Rect::new(0.0, 0.0, 1920.0, 162.0)));
        assert_eq!(hit(&app, &Action::BrowserTab(1)), Rect { y: tab.y + 83.0, ..tab });
        assert_eq!(app.state.win_rect(Win::Playlist).y, 166.0, "docked under the band");
        // A floating window keeps its distance to the band when the band shrinks back.
        app.state.wins[0].maxed = false;
        app.state.wins[0].rect = Rect::new(400.0, 300.0, 900.0, 500.0);
        app.state.raise(Win::Playlist);
        app.paint(&mut c, None);
        assert_eq!(app.pref_choose(&TOOLBARS_SCALING, 0).as_deref(), Some("Toolbars scaling: Main"));
        app.paint(&mut c, None);
        assert_eq!((crate::views::band_h(&app.state), app.state.win_rect(Win::Playlist).y), (83.0, 217.0));
        assert_eq!(hit(&app, &Action::Clock), clock);
    }

    /// At 200 % the band's controls answer where they are drawn: the metronome, the menu bar (whose FILE menu keeps
    /// Pop-ups scaling, rows 19 px at Main on a 100 % window) and the tempo's digits; repaints cover the scaled panels.
    #[test]
    fn band_controls_are_hit_where_drawn() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        set_choice(&TOOLBARS_SCALING, 5);
        app.paint(&mut c, None);
        // The metronome (580, 4, 30 x 33 in the band's px), pressed near its bottom-right corner, where the Main layout
        // has another button.
        let m = hit(&app, &Action::Metronome);
        assert_eq!(m, Rect::new(1160.0, 8.0, 60.0, 66.0));
        let was = crate::metronome::on();
        click(&mut app, &mut c, m.right() - 4.0, m.bottom() - 4.0);
        assert_eq!(crate::metronome::on(), !was);
        // FILE opens its menu under its title, at Pop-ups scaling.
        let file = hit(&app, &Action::Menu(0));
        assert!(file.y >= 90.0 && file.h > 50.0, "{file:?}");
        click(&mut app, &mut c, file.cx(), file.cy());
        assert_eq!(app.state.menu_open, Some(0));
        let save = hit(&app, &crate::views::menus::pick(0, crate::views::menus::FILE_SAVE));
        app.state.menu_open = None;
        // Under its title, which moved: at Main the same menu opens 78 px higher (the bottom row ends at 78, not 156)
        // and 4 px further left (its left edge, 4 px, doubled); same rows, same width.
        set_choice(&TOOLBARS_SCALING, 0);
        app.paint(&mut c, None);
        let main = hit(&app, &Action::Menu(0));
        click(&mut app, &mut c, main.cx(), main.cy());
        let save0 = hit(&app, &crate::views::menus::pick(0, crate::views::menus::FILE_SAVE));
        assert_eq!((save.h, save0.h, save.w, save.x - save0.x, save.y - save0.y), (19.0, 19.0, save0.w, 4.0, 78.0), "{save:?} {save0:?}");
        app.state.menu_open = None;
        set_choice(&TOOLBARS_SCALING, 5);
        app.paint(&mut c, None);
        // The tempo: the first decimal steps by 0.1.
        let tempo = hit(&app, &Action::Tempo);
        let digits = app.state.tempo_digits.get();
        assert!(digits[0].is_finite() && tempo.contains(2.0 * digits[0] + 2.0, tempo.cy()));
        app.mouse_move(2.0 * digits[0] + 2.0, tempo.cy());
        app.mouse_down();
        assert!(matches!(app.drag, Some(crate::app::Drag::Tempo { step, .. }) if step == 0.1));
        app.mouse_up();
        // A change in the hint panel repaints the panel where it is drawn.
        app.paint(&mut c, None);
        let _ = app.take_damage();
        app.damage_band(crate::app::HINT_RECT);
        let panel = crate::views::band_rect(&app.state, crate::app::HINT_RECT);
        let damage = app.take_damage().flatten().expect("rects");
        assert!(damage.iter().any(|r| r.contains(panel.x, panel.y) && r.contains(panel.right() - 1.0, panel.bottom() - 1.0)), "{damage:?}");
        set_choice(&TOOLBARS_SCALING, 0);
    }

    /// Panels that don't fit are left out from the right; the ▶ arrow at the band's right end says why when hovered and
    /// does nothing when clicked (spec: ui/ui-scaling.md §1). Everything fits at Main on 1920 px.
    #[test]
    fn panels_that_do_not_fit_leave_an_arrow() {
        assert_eq!((room(FULL_W, FULL_W), room(FULL_W - 1.0, FULL_W)), (f32::INFINITY, FULL_W - 1.0 - MORE_W));
        let more = |app: &App| app.hits.iter().rev().find(|h| h.hint == MORE_HINT).map(|h| (h.rect, h.action.clone()));
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.paint(&mut c, None);
        assert_eq!(more(&app), None);
        assert!(has(&app, &Action::Monitor) && has(&app, &Action::CpuPanel) && has(&app, &Action::PatternPrev));
        assert!(app.hits.iter().any(|h| h.hint == "About Lemon Studio"));
        // No news panel: Lemon Studio has no online service.
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder("News")));

        // 200 %: 960 band px. Row 1 ends after the time display, row 2 after the snap combo.
        set_choice(&TOOLBARS_SCALING, 5);
        app.invalidate();
        app.paint(&mut c, None);
        let (r, action) = more(&app).expect("the arrow");
        assert_eq!((r, action), (Rect::new(1920.0 - 2.0 * MORE_W, 0.0, 2.0 * MORE_W, 162.0), Action::Swallow));
        assert!(has(&app, &Action::Clock) && has(&app, &Action::GlobalSnap));
        assert!(!has(&app, &Action::Monitor) && !has(&app, &Action::CpuPanel) && !has(&app, &Action::PatternPrev));
        assert!(!app.hits.iter().any(|h| h.hint == "About Lemon Studio"));
        app.mouse_move(r.cx(), r.cy());
        assert_eq!(app.hint, MORE_HINT);
        let before = (app.state.message.clone(), app.state.menu_open, app.state.settings.open);
        app.mouse_down();
        app.mouse_up();
        assert_eq!((app.state.message.clone(), app.state.menu_open, app.state.settings.open), before, "a click does nothing");

        // Main on a narrower window: the same rule (the CPU panel fits, the last buttons don't).
        set_choice(&TOOLBARS_SCALING, 0);
        let Ok(mut narrow) = Canvas::new(1300, 800, 1.0) else { return };
        app.invalidate();
        app.paint(&mut narrow, None);
        let (r, _) = more(&app).expect("the arrow");
        assert_eq!(r, Rect::new(1300.0 - MORE_W, 0.0, MORE_W, 81.0));
        assert!(has(&app, &Action::CpuPanel) && has(&app, &Action::PatternPrev));
        assert!(!app.hits.iter().any(|h| h.hint == "About Lemon Studio" || h.hint == "Project info (F11)"));
    }
}
