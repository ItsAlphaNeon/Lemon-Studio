// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Views of the main window, laid out from the measured default layout.
// spec: ui/main-window.md

pub mod browser;
pub mod cabledebug;
pub mod channel_win;
pub mod data_folder;
pub mod factory_folder;
pub mod dialog;
pub mod menus;
pub mod midi_form;
pub mod midi_page;
pub mod mondebug;
pub mod settings;
pub mod mixer;
pub mod piano;
pub mod patterns;
pub mod perf;
pub mod playlist;
pub mod project_forms;
pub mod rack;
pub mod remote_box;
pub mod export_form;
pub mod save_form;
pub mod arpeggiate;
pub mod strum;
pub mod piano_dlg;
pub mod tapper;
pub mod touch;
pub mod theme_preview;
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
    /// The tempo the toolbar shows: the one playback plays at (tempo automation moves it), else the project's.
    pub tempo: f64,
    pub peaks: (f32, f32),
    /// Notes playing now, as (channel index, key).
    pub sounding: Vec<(u16, u16)>,
    /// The song position slider's handle is being dragged (it shows the dragged position, not the playhead).
    pub song_pos_drag: bool,
    /// What the toolbar editor's drag carries (spec: ui/toolbar-menu.md §8).
    pub bar_drag: Option<crate::toolbar::BarDrag>,
}

/// UI text at a nominal size (Cuprum; spec §10).
pub fn fnt(size: f32) -> os_ui::Font {
    os_ui::Font::new(size.round() as u16)
}

/// The light display face (song position).
pub fn light(size: f32) -> os_ui::Font {
    os_ui::Font::light(size.round() as u16, 0)
}

/// Display scale of popup menus and combo lists for a window at scale `main`: General ▸ Display ▸ Pop-ups scaling
/// (Main = `main`). Their row heights follow it (`os_ui::kit::menu::metrics`). Spec: ui/ui-scaling.md §1–2.
pub fn popup_scale(main: f32) -> f32 {
    let pct = crate::prefs::choice(&crate::prefs::POPUPS_SCALING).strip_suffix('%').and_then(|p| p.parse::<f32>().ok());
    pct.map_or(main, |p| p / 100.0)
}

/// Top of the workspace (§1) below the toolbar of two rows (the classic layout).
pub const WORK_Y: f32 = 83.0;

/// Top of the workspace and the browser now: the window's top while the toolbar is hidden (spec: ui/toolbar-menu.md
/// §5), else the top band's bottom (`band_h`).
pub fn work_y(s: &State) -> f32 {
    if s.toolbar.hidden { 0.0 } else { band_h(s) }
}

/// Display scale of the main window's top band (the toolbar rows, `WORK_Y` tall in its own px for two rows) for a
/// window at scale `main`: General ▸ Display ▸ Toolbars scaling (Main = `main`). Spec: ui/ui-scaling.md §1.
pub fn toolbar_scale(main: f32) -> f32 {
    let pct = crate::prefs::choice(&crate::prefs::TOOLBARS_SCALING).strip_suffix('%').and_then(|p| p.parse::<f32>().ok());
    pct.map_or(main, |p| p / 100.0)
}

/// The window's px per px of the top band at the last paint's scale: 1 at Toolbars scaling Main.
pub fn band_k(s: &State) -> f32 {
    let main = s.ui_scale.max(0.01);
    toolbar_scale(main) / main
}

/// Height of the top band in the window's px (shown or not): its rows' (`toolbar::Layout::band_h`, `WORK_Y` for two)
/// at its scale.
pub fn band_h(s: &State) -> f32 {
    s.bar.layout.band_h() * band_k(s)
}

/// `r` in px `k` times as big.
pub fn scale_rect(r: Rect, k: f32) -> Rect {
    Rect::new(r.x * k, r.y * k, r.w * k, r.h * k)
}

/// The top band's toolbar panels as laid out now, in the band's own px.
fn placed(s: &State) -> Vec<crate::toolbar::Placed> {
    crate::toolbar::place(&s.bar.layout, s.screen.0 / band_k(s), s.bar.editing)
}

/// How far toolbar panel `p` is from its home (`toolbar::Panel::home`); `None` when it is not on the toolbar.
pub fn panel_at(s: &State, p: crate::toolbar::Panel) -> Option<crate::toolbar::At> {
    placed(s).into_iter().find(|q| q.panel == p).map(|q| q.at())
}

/// Rect `r` of the top band, given in the home px of the panel whose slot holds its top-left (`toolbar::Panel`,
/// `app::HINT_RECT`, `patterns::PLUS_BOX_AT`...), in the window's px where that panel is drawn now. A panel that is not
/// on the toolbar leaves `r` where it is (scaled).
pub fn band_rect(s: &State, r: Rect) -> Rect {
    let a = crate::toolbar::Panel::at_home(r.x, r.y).and_then(|p| panel_at(s, p)).unwrap_or_default();
    scale_rect(a.r(r), band_k(s))
}

/// The parts of rect `r` of the top band, given in its panels' home px (`band_rect`), in the window's px where each of
/// those panels is drawn now: to repaint them (`App::damage_band`). Panels that are not on the toolbar give none.
pub fn band_rects(s: &State, r: Rect) -> Vec<Rect> {
    let k = band_k(s);
    placed(s)
        .into_iter()
        .filter(|p| !p.panel.repeats())
        .filter_map(|p| {
            let (row, x0, x1) = p.panel.home();
            let slot = Rect::new(x0, crate::toolbar::ROW_H * row as f32, x1 - x0, crate::toolbar::ROW_H + 1.0);
            let part = slot.intersect(&r);
            (!part.is_empty()).then(|| scale_rect(p.at().r(part), k))
        })
        .collect()
}

/// The toolbar editor's palette in the window's px, while it is open (spec: ui/toolbar-menu.md §8).
pub fn palette_rect(s: &State) -> Option<Rect> {
    let k = band_k(s);
    (s.bar.editing && !s.toolbar.hidden).then(|| scale_rect(toolbar::palette(s, s.screen.0 / k).rect, k))
}
/// The Lemon layout's status bar along the window's bottom (General settings ▸ Layout): its height.
pub const STATUS_H: f32 = 32.0;
/// The Lemon layout's gap around the workspace card, and the card's corner radius.
pub const GUTTER: f32 = 6.0;
pub const CARD_R: f32 = 8.0;

/// Height of the status bar now: none in the classic layout.
pub fn status_h(s: &State) -> f32 {
    if s.bar.lemon { STATUS_H } else { 0.0 }
}

/// The status bar (the Lemon layout).
pub fn status_rect(s: &State) -> Rect {
    Rect::new(0.0, s.screen.1 - STATUS_H, s.screen.0, STATUS_H)
}

/// The docked browser, its splitter included. The Lemon layout makes it a card like the workspace: a gap left, above
/// and below it, and the splitter is the gap between the two cards (`GUTTER` wide, no bars).
pub fn browser_rect(s: &State) -> Rect {
    let (top, h) = (work_y(s), s.screen.1 - status_h(s));
    if browser_right(s) {
        Rect::new(s.screen.0 - browser_w(s), top + GUTTER, browser_w(s) - GUTTER, (h - top - 2.0 * GUTTER).max(0.0))
    } else if s.bar.lemon {
        Rect::new(GUTTER, top + GUTTER, browser_w(s) - GUTTER, (h - top - 2.0 * GUTTER).max(0.0))
    } else {
        Rect::new(0.0, top, browser_w(s), (h - top).max(0.0))
    }
}

/// The browser sits right of the workspace (the Lemon layout, its title bar dragged there); else left of it.
pub fn browser_right(s: &State) -> bool {
    s.bar.lemon && s.browser_right
}

/// The browser's inner edge (where its splitter meets the workspace): its width from the window's left or right.
pub fn browser_edge(s: &State) -> f32 {
    if browser_right(s) { s.screen.0 - browser_w(s) } else { browser_w(s) }
}

/// The browser panel within `browser_rect` (the rest is its splitter).
pub fn browser_panel(s: &State, r: Rect) -> Rect {
    if browser_right(s) {
        Rect::new(r.x + GUTTER, r.y, r.w - GUTTER, r.h)
    } else if s.bar.lemon {
        Rect::new(r.x, r.y, r.w - GUTTER, r.h)
    } else {
        Rect::new(r.x, r.y, r.w - browser::SPLITTER, r.h)
    }
}

/// Width of the docked browser incl. the splitter (§4): the default, and the least its splitter drags it to.
pub const BROWSER_W: f32 = 209.0;

/// Width of the docked browser incl. the splitter, as dragged (`State::browser_w`), at most half the window (spec:
/// ui/browser.md §9).
pub fn browser_w(s: &State) -> f32 {
    s.browser_w.min(s.screen.0 / 2.0).max(BROWSER_W)
}

/// Paints the frame; returns the number of hits registered before the overlays (menus, popups, picker...), which cover
/// docked plugin windows.
pub fn paint(ui: &mut Ui, s: &State, f: &FrameInfo) -> usize {
    let (w, h) = ui.c.size();
    // A maximized Playlist or Piano roll paints all of its rect (`framed`): the workspace under it needs no clear.
    let cover = s.docked().filter(|w| matches!(w, Win::Playlist | Win::PianoRoll)).map(|w| s.win_rect(w));
    ui.c.set_cover(cover);
    ui.c.clear(ui.t.workspace);
    ui.c.set_cover(None);

    // Workspace (§9): the theme's picture or the watermark, then docked windows on top. The Lemon layout puts it on a
    // card over a darker backdrop, the status bar below.
    let top = work_y(s);
    let work = s.work_rect();
    let docked = s.docked();
    // Snapped windows (the Lemon layout's split views) are cards of their own, the backdrop between them.
    let tiles = crate::tiles::plan(s);
    let anim = s.tile_anim.as_ref();
    let split = (docked.is_none() && !tiles.tiles.is_empty()) || anim.is_some();
    if s.bar.lemon {
        ui.c.fill_rect(Rect::new(0.0, top, w, h - top), backdrop(ui));
        if !split {
            ui.c.fill_rrect(work, CARD_R, ui.t.workspace);
        }
    }
    // spec: ui/flstheme.md §3 (BackMode, BackPicFilename); no watermark over a picture (`wallpaper`).
    // The cutting-edge branch marks itself: the watermark and the version string lean red.
    if docked.is_none() && !split && !crate::wallpaper::paint(ui, work) {
        let wm = ui.t.watermark.mix(CUTTING_EDGE_RED, 0.12);
        icons::logo(ui.c, work.cx(), work.cy() + 40.0, 170.0, wm);
        let version = concat!("Lemon Studio v", env!("CARGO_PKG_VERSION"), " - cutting-edge pre-alpha");
        let vc = ui.t.version_text.mix(CUTTING_EDGE_RED, 0.65);
        let vw = ui.c.text_width(crate::views::fnt(16.0), version);
        ui.c.text(crate::views::fnt(16.0), work.cx() - vw / 2.0, work.bottom() - 22.0, vc, version);
    }

    // One stacking order for all child windows (spec: ui/channel-rack.md §5). A maximized window
    // fills the workspace, so windows below it are covered and not painted.
    let order = s.paint_order();
    let docked_at = order.iter().rposition(|&w| s.is_max(w));
    // Each part is skipped by a repaint that misses it (`Ui::part`).
    if s.show_browser {
        let r = browser_rect(s);
        ui.part(Seg::Browser, r, |ui| {
            browser::paint(ui, s, r);
            if s.bar.lemon {
                card_corners(ui, browser_panel(s, r));
            }
        });
    }
    let _ = work;
    // The Lemon layout's animation: under the moving windows, a closing one's last look and the opening ones at their
    // places.
    if let Some(m) = anim {
        // Where the windows are this frame: the gaps between neighbours run on through a closing window's picture.
        let now: Vec<Rect> = order.iter().filter(|w| w.slot().is_some()).map(|&w| m.at(w).unwrap_or_else(|| s.win_rect(w))).collect();
        for (r, snap) in &m.closing {
            match snap {
                Some(snap) => ui.c.paint_snapshot(snap),
                None => os_ui::kit::lemon::outlined(ui.c, *r, CARD_R, ui.t.window_border, ui.t.window_body),
            }
            card_corners(ui, *r);
            for gap in gaps(&now) {
                let g = if gap.w <= 2.0 * GUTTER { Rect::new(gap.x, r.y, gap.w, r.h) } else { Rect::new(r.x, gap.y, r.w, gap.h) };
                ui.c.fill_rect(g.intersect(r), backdrop(ui));
            }
        }
        for &(win, r) in m.opening() {
            ui.win = Some(win);
            ui.c.push_clip(r);
            paint_window(ui, s, win, r, r, f);
            ui.c.pop_clip();
            ui.win = None;
            card_corners(ui, r);
        }
    }
    for (i, &win) in order.iter().enumerate() {
        // While windows move (the Lemon layout's animation) they all show, the covered ones too.
        if (docked_at.is_some_and(|d| i < d) && anim.is_none()) || anim.is_some_and(|m| m.opening().iter().any(|o| o.0 == win)) {
            continue;
        }
        ui.win = Some(win);
        let moved = anim.and_then(|m| m.at(win));
        let r = moved.unwrap_or_else(|| s.win_rect(win));
        // A window paints inside its paint area (also what it marks as drawn, e.g. text's generous overhang).
        let a = if moved.is_some() { r } else { paint_area(s, win) };
        let h0 = ui.hits_len();
        ui.c.push_clip(a);
        paint_window(ui, s, win, r, a, f);
        ui.c.pop_clip();
        // A split or maximized window's clicks stay in its card (a track row cut by the card's edge would take the
        // window's below it).
        if s.tiled() && win.slot().is_some() {
            ui.clip_hits(h0, a);
        }
        ui.win = None;
        // A split window without the keyboard is muted (`State::veil`); a maximized one never.
        if let Some(v) = win.slot().map(|i| s.veil[i]).filter(|&v| v > 0.0 && s.bar.lemon && s.snapped(win) && docked.is_none()) {
            let bg = backdrop(ui);
            ui.c.fill_rect(r, Color { a: (v * VEIL * 255.0).round() as u8, ..bg });
        }
        // A maximized or snapped window fills its card square: its corners are rounded before the floating windows
        // above it, which go over them.
        if let Some(mr) = moved {
            card_corners(ui, mr);
        } else if s.bar.lemon && docked_at == Some(i) {
            card_corners(ui, work);
        } else if let Some(&(_, tr)) = tiles.tiles.iter().find(|t| t.0 == win).filter(|_| docked_at.is_none() || anim.is_some()) {
            card_corners(ui, tr);
        }
    }
    // The gaps between snapped windows drag their splits.
    if split && anim.is_none() {
        for d in &tiles.dividers {
            ui.hit(d.gap, Action::TileSplit(d.node), "Drag to resize the split");
        }
    }
    let _ = docked;
    if s.bar.lemon {
        toolbar::paint_status_bar(ui, s, f);
    }
    let n = paint_rest(ui, s, f, w);
    // The menus and boxes of plugin editors drawn in the app's window (Linux), above everything.
    #[cfg(target_os = "linux")]
    for (&row, e) in &s.embeds {
        e.paint_overlays(ui.c);
        if let Some(r) = e.overlays_rect() {
            ui.hit(r, Action::Embed(row), "");
        }
    }
    n
}

/// Paints window `win` at `r` inside its paint area `a`.
fn paint_window(ui: &mut Ui, s: &State, win: Win, r: Rect, a: Rect, f: &FrameInfo) {
        match win {
            Win::Playlist => ui.part(Seg::Win(win), a, |ui| playlist::paint(ui, s, r, f)),
            Win::Mixer => ui.part(Seg::Win(win), a, |ui| {
                // Timed while the cables swing: a mixer too slow to redraw them hangs them still. Not in tests: they
                // paint while the rest of the suite runs, and on a busy machine the clock would hang the cables (and
                // post their message) at random between two paints a test compares. `cable`'s tests give the costs.
                let swinging = (!cfg!(test) && s.cables.borrow().moving()).then(std::time::Instant::now);
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
            Win::Touch => ui.part(Seg::Win(win), a, |ui| touch::paint(ui, s, f)),
            Win::Strum => ui.part(Seg::Win(win), a, |ui| strum::paint(ui, s, f)),
            Win::Arp => ui.part(Seg::Win(win), a, |ui| arpeggiate::paint(ui, s, f)),
            Win::PianoDlg => ui.part(Seg::Win(win), a, |ui| piano_dlg::paint(ui, s, f)),
            // A plugin window drawn in the app's (Linux): its frame and editor; every click on it goes to it.
            #[cfg(target_os = "linux")]
            Win::Hosted(row) => {
                if let Some(e) = s.embeds.get(&row) {
                    ui.part(Seg::Win(win), a, |ui| {
                        e.paint(ui.c);
                        ui.hit(e.rect(), Action::Embed(row), "");
                    });
                }
            }
            _ => {}
        }
}

/// The frame above the windows: the toolbar, the hint bar, menus, popups, the settings window and dialogs. Returns the
/// number of hits registered before the overlays.
fn paint_rest(ui: &mut Ui, s: &State, f: &FrameInfo, w: f32) -> usize {
    if !s.toolbar.hidden {
        // At Toolbars scaling (spec: ui/ui-scaling.md §1): the band works in its own px, `k` of the window's each, so
        // it is `w / k` of them wide; what doesn't fit is left out (`toolbar::room`).
        let scale = toolbar_scale(ui.c.scale);
        ui.part(Seg::Toolbar, Rect::new(0.0, 0.0, w, band_h(s)), |ui| ui.scaled(scale, |ui, k| toolbar::paint(ui, s, f, w / k)));
    }
    // The floating hint bar, over every window (spec: ui/toolbar-menu.md §4).
    if s.toolbar.hint_bar {
        toolbar::paint_hint_bar(ui, s, f);
    }
    let overlays = ui.hit_count();
    // The toolbar editor's palette goes over the windows, which docked plugin windows leave free like any overlay (spec:
    // ui/toolbar-menu.md §8).
    if s.bar.editing && !s.toolbar.hidden {
        let scale = toolbar_scale(ui.c.scale);
        ui.scaled(scale, |ui, k| toolbar::paint_palette(ui, s, f, w / k));
    }
    if let Some(m) = s.menu_open {
        ui.scaled(popup_scale(ui.c.scale), |ui, k| menus::paint_dropdown(ui, s, m, k));
    }
    // A menu opened from the settings window (Audio page combos) goes over it; a dropdown's submenu over the dropdown;
    // the time signature dialog's value lists over the dialog.
    let over_dialog = crate::timesig::form_open(s);
    if !s.settings.open && !over_dialog {
        playlist::paint_overlays(ui, s);
    }
    rack::paint_fill(ui, s);
    patterns::paint(ui, s);
    if s.settings.open {
        settings::paint(ui, s);
        if !over_dialog {
            playlist::paint_overlays(ui, s);
        }
    }
    browser::paint_ghost(ui, s);
    if s.monitor.debug {
        mondebug::paint(ui, s);
    }
    if s.cables.borrow().debug {
        cabledebug::paint(ui, s);
    }
    // The Song info box of a project opened with "Show info on opening" (spec: flp/events/project-info.md).
    crate::project_info::paint_song_info(ui, s);
    // The Remote control settings box (spec: ui/remote-control.md §2).
    if let Some(b) = &s.link_box {
        remote_box::paint(ui, s, b);
    }
    // Message boxes and file dialogs go over everything (spec: ui/dialogs.md).
    dialog::paint(ui, s);
    if over_dialog {
        playlist::paint_overlays(ui, s);
    }
    overlays
}

/// The cutting-edge branch's mark colour (the workspace watermark, the version string, the status bar's mark).
pub(crate) const CUTTING_EDGE_RED: Color = Color::rgb(0xe5483b);

/// The Lemon layout's backdrop around the workspace card: a shade darker than the workspace.
pub(crate) fn backdrop(ui: &Ui) -> Color {
    ui.t.workspace.shade(-0.35)
}

/// The gaps between windows side by side or one above the other (at most two gutters apart, along what they share).
fn gaps(rects: &[Rect]) -> Vec<Rect> {
    let mut out = Vec::new();
    for a in rects {
        for b in rects {
            let (dx, dy) = (b.x - a.right(), b.y - a.bottom());
            if dx > 0.0 && dx <= 2.0 * GUTTER && a.y < b.bottom() && b.y < a.bottom() {
                out.push(Rect::new(a.right(), a.y.max(b.y), dx, a.bottom().min(b.bottom()) - a.y.max(b.y)));
            }
            if dy > 0.0 && dy <= 2.0 * GUTTER && a.x < b.right() && b.x < a.right() {
                out.push(Rect::new(a.x.max(b.x), a.bottom(), a.right().min(b.right()) - a.x.max(b.x), dy));
            }
        }
    }
    out
}

/// Rounds the workspace card's corners over whatever a docked window painted there: each corner outside the arc is
/// filled with the backdrop.
fn card_corners(ui: &mut Ui, r: Rect) {
    let bg = backdrop(ui);
    let n = 8;
    // past the card's edge by a device px or so: an edge between device px would leave a sliver of the square corner
    let e = 1.5 / ui.c.scale;
    for (cx, cy, sx, sy) in [(r.x, r.y, 1.0, 1.0), (r.right(), r.y, -1.0, 1.0), (r.x, r.bottom(), 1.0, -1.0), (r.right(), r.bottom(), -1.0, -1.0)] {
        // the corner point (out past the card), then the arc from one edge to the other, curving towards the card
        let mut pts = vec![(cx - sx * e, cy - sy * e), (cx + sx * CARD_R, cy - sy * e), (cx + sx * CARD_R, cy)];
        for i in 0..=n {
            let a = std::f32::consts::FRAC_PI_2 * i as f32 / n as f32;
            pts.push((cx + sx * CARD_R * (1.0 - a.sin()), cy + sy * CARD_R * (1.0 - a.cos())));
        }
        pts.push((cx, cy + sy * CARD_R));
        pts.push((cx - sx * e, cy + sy * CARD_R));
        ui.c.fill_polygon(&pts, bg);
    }
}

/// Where window `w` paints: its rect (and the channel rack's graph editor hanging below it). The resize zones past
/// its edges are hits only. A repaint that misses this skips the window; a playlist scroll paints around it.
pub fn paint_area(s: &State, w: Win) -> Rect {
    match w {
        Win::ChannelRack => rack::paint_area(s),
        Win::Plugin => channel_win::rect(s),
        Win::Perf => perf::bounds(s),
        Win::Tapper => tapper::bounds(s),
        Win::Touch => touch::bounds(s),
        Win::Strum => strum::bounds(s),
        Win::Arp => arpeggiate::bounds(s),
        Win::PianoDlg => piano_dlg::bounds(s),
        Win::Mixer => s.mixer_hang().map_or(s.win_rect(Win::Mixer), |h| s.win_rect(Win::Mixer).union(&h)),
        #[cfg(target_os = "linux")]
        Win::Hosted(row) => s.embeds.get(&row).map_or(Rect::new(0.0, 0.0, 0.0, 0.0), |e| e.rect()),
        w => s.win_rect(w),
    }
}

/// How much of the backdrop a muted split window takes on (`State::veil` at 1).
const VEIL: f32 = 0.3;

/// Window `w`'s own rect: the paint area without what hangs out of it (the channel rack's graph editor).
pub fn frame_rect(s: &State, w: Win) -> Rect {
    match w {
        Win::ChannelRack => rack::rect(s),
        Win::Plugin => channel_win::rect(s),
        w => paint_area(s, w),
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
    if s.wins[i].maxed || s.wins[i].mini || s.tiled() {
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
/// The Lemon layout's split views have no minimize: the other two keep their places.
pub fn window_buttons(ui: &mut Ui, win: Win, x0: f32, y: f32, _normal: Color, _hover: Color) {
    let mut hovered = None;
    let maxed = win.slot().is_some_and(|i| ui.maxed[i]);
    // A window alone in the Lemon layout has only its close button.
    let skip = if ui.alone { 2 } else { usize::from(ui.tiled) };
    let max_hint = match (ui.tiled, maxed) {
        (false, _) => "Maximize / restore window",
        (true, false) => "Maximize (F)",
        (true, true) => "Back to the split (F)",
    };
    for (i, (hint, action)) in [("Minimize window", Action::MinWin(win)), (max_hint, Action::MaxWin(win)), ("Close window", Action::CloseWin(win))]
        .into_iter()
        .enumerate()
        .skip(skip)
    {
        if ui.hit(Rect::new(x0 + i as f32 * 19.5, y + 3.0, 17.0, 17.0), action, hint) {
            hovered = Some(i);
        }
    }
    let glyph = match (ui.tiled, maxed) {
        _ if ui.alone => kit::MaxGlyph::None,
        (false, _) => kit::MaxGlyph::Restore,
        (true, false) => kit::MaxGlyph::Full,
        (true, true) => kit::MaxGlyph::Split,
    };
    kit::window_buttons(ui.c, ui.t, x0, y, hovered, glyph);
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
    // The title stops short of the window buttons (a narrow window cuts it).
    ui.c.text_clip(os_ui::Font::new(17), r.x + title_x, r.y + 1.0, (title_room(r, buttons) - title_x).max(0.0), t.text, title);
    ui.hit(bar, Action::WinDrag(win), drag_hint(ui, win));
    // Minimize / maximize / close glyphs at the right (1862, 1881, 1901 at full width).
    if buttons {
        window_buttons(ui, win, r.right() - 64.0, r.y, t.text_dim.mix(t.glyph, 0.6), t.glyph);
    }
    Rect::new(r.x, r.y + 23.0, r.w, r.h - 23.0)
}

/// A title bar's hint: a split window is dragged onto another to change places.
fn drag_hint(ui: &Ui, win: Win) -> &'static str {
    match win.slot() {
        Some(i) if ui.tiled && !ui.maxed[i] && !ui.alone => "Drag onto another window to change places (Ctrl+Alt+Shift+arrows)",
        _ => "Drag to move",
    }
}

/// How far from a title bar's left (at `r`) its text may reach: short of the window buttons when there are any.
pub fn title_room(r: Rect, buttons: bool) -> f32 {
    if buttons { r.w - 68.0 } else { r.w }
}

/// Floating window frame (spec ui/piano-roll.md §1a, same for Playlist / Mixer / Piano roll):
/// 1 px dark border, 5 px frame, 1 px inner line around the client, flat 28 px title bar.
/// Returns the inner rect (laid out like the docked window) when `win` floats, else `None`.
pub fn float_frame(ui: &mut Ui, s: &State, win: Win, r: Rect) -> Option<Rect> {
    let ws = &s.wins[win.slot()?];
    if ws.maxed || ws.mini || s.tiled() || r.h < 2.0 * crate::app::TITLE_H {
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
    let floats = win.slot().is_some_and(|i| !s.wins[i].maxed && !s.wins[i].mini) && !s.tiled() && r.h >= 2.0 * crate::app::TITLE_H;
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

/// "Small scrollbars in editors" is on (General ▸ Miscellaneous; spec: ui/settings-choices.md "Miscellaneous"): the
/// piano roll's and the playlist's scroll bars are about half as thick, without arrow buttons; the room goes to the
/// time ruler and the grid.
pub fn small_scrollbars() -> bool {
    crate::prefs::check(&crate::prefs::SMALL_SCROLLBARS)
}

/// The Playlist picker's scroll bar hint (its right-drag resizes the previews, spec: ui/scroll-bars.md §3).
pub const PICK_BAR_HINT: &str = "Drag to scroll - right-drag resizes the previews";

/// A scroll bar over `track`: thumb sized `view / total`, positioned by `pos` (content pixels).
/// Registers the bar for dragging (see `App::scrollbar_press`).
pub fn scrollbar(ui: &mut Ui, track: Rect, vertical: bool, pos: f32, view: f32, total: f32, target: Scroll) {
    let len = if vertical { track.h } else { track.w };
    let total = total.max(view).max(1.0);
    let max = (total - view).max(0.0);
    let (_, thumb) = os_ui::kit::thumb(len, pos, view, total);
    let hint = match target {
        Scroll::PianoX | Scroll::PlaylistX => "Drag to scroll - right-drag: left/right scrolls, up/down zooms",
        Scroll::PlPick => PICK_BAR_HINT,
        _ => "Drag to scroll",
    };
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

/// A square toolbar button's face for `look` and the colour of its icon. With the Toolbars menu's Flat buttons on
/// (spec: ui/toolbar-menu.md §3) there is no face: a lit button is a slightly darker flat square with a blue icon
/// (Lemon: its button shade, the icon in the brand colour), the others just their icon.
pub fn tool_face(ui: &mut Ui, r: Rect, look: Look) -> Color {
    let lit = matches!(look, Look::On | Look::Hot);
    if !ui.flat {
        button_face(ui, r, look);
    } else if lit && lemon(ui) {
        ui.c.fill_rrect(r.inset(1.0), ui.t.style.radius, ui.t.button);
        return ui.t.brand_lit(os_theme::design::lemon::MAIN);
    } else if lit {
        ui.c.fill_rect(Rect::new(r.x, r.y + 1.0, r.w, r.h - 2.0), ui.t.s(0x282c2e));
        return ui.t.a(0x4589c7);
    }
    match look {
        Look::On | Look::Hot => ui.t.on_glyph,
        Look::Hover => ui.t.tx(0xffffff),
        Look::Off => ui.t.glyph,
    }
}

/// A 30×33 toolbar button (§1). `on` draws the orange "on" state.
/// An `Action::Placeholder` button is not available yet: drawn greyed out, never lit (`Ui::off`).
pub fn tool_button(ui: &mut Ui, r: Rect, icon: Icon, on: bool, action: Action, hint: &'static str) {
    if matches!(action, Action::Placeholder(_)) {
        ui.off(r, hint);
        let st = kit::State::NORMAL.with_disabled(true);
        if !ui.flat {
            kit::button(ui.c, ui.t, r, st);
        }
        icons::draw(ui.c, icon, Rect::new(r.x + 5.0, r.y + 6.5, 20.0, 20.0), kit::ink(ui.t, st));
        return;
    }
    let hov = ui.hit(r, action.clone(), hint);
    let down = ui.down(r, &action);
    let look = if on { Look::On } else if down { Look::Hot } else if hov { Look::Hover } else { Look::Off };
    let c = tool_face(ui, r, look);
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

/// The label of a question's check that turns its Manage messages entry off when Ok is pressed.
const REMEMBER: &str = "Remember my choice";

/// How wide `remember_check` draws.
pub fn remember_width(ui: &mut Ui) -> f32 {
    20.0 + ui.c.text_width(os_ui::Font::new(13), REMEMBER)
}

/// A question's "Remember my choice" check at the bottom left of its box, left edge `x`, centred on `cy`: a small box,
/// ticked when `on`, and its label; a click on either is `action`. Spec: ui/settings-choices.md "Hints and warnings:
/// Manage messages and Project data size warning (2026-10-16)".
pub fn remember_check(ui: &mut Ui, x: f32, cy: f32, on: bool, action: Action) {
    let t = ui.t;
    let f = os_ui::Font::new(13);
    let w = remember_width(ui);
    let hov = ui.hit(Rect::new(x, cy - 10.0, w, 20.0), action, "Ok with this ticked turns this message off (Manage messages)");
    let b = Rect::new(x, (cy - 6.5).round(), 13.0, 13.0);
    ui.c.fill_rrect(b, 2.0, t.s(0x141516));
    ui.c.stroke_rrect(b, 2.0, 1.0, if hov { t.glyph_dim } else { t.text_dim });
    if on {
        kit::tick(ui.c, b, t.glyph);
    }
    let lh = ui.c.line_height(f);
    ui.c.text(f, x + 20.0, (cy - lh / 2.0).round(), if hov { t.glyph } else { t.text }, REMEMBER);
}

/// Channel / clip colors from the model.
pub fn rgb(c: os_model::Rgb) -> Color {
    Color { r: c.r, g: c.g, b: c.b, a: 255 }
}
