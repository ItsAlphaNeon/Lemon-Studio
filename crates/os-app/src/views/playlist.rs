// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Playlist window: tools, picker, track headers, ruler, markers, clips, overlays and popup menus.
// spec: ui/main-window.md §5–6; ui/playlist.md

use os_model::{AutoMode, Clip, ClipSource, TimeMarker};
use os_ui::kit::lemon as lk;
use os_theme::waves::Waves;
use os_ui::{Color, Font, Rect};

use super::{resize_zones, rgb, window_body, FrameInfo};
use crate::app::{Action, Seg, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::playlist::{self as pl, Mark, Overlay, PGeom, PickKind, PlTool, MARKER_H, TITLE_H, TRACKS};

/// Default pixels per bar (the zoom is `State::playlist_bar_w`).
pub const BAR_W: f32 = 60.0;

// Clip colours (spec: ui/playlist.md §2).
const CLIP_BORDER: u32 = 0x353c41;
const TITLE_TOP: u32 = 0x50595e;
const TITLE_BOTTOM: u32 = 0x3f494d;
const TITLE_TEXT: u32 = 0xd6dfe4;
const SEL_TITLE: u32 = 0x75201f;
const SEL_TEXT: u32 = 0xc29d8f;
const SEL_BODY: u32 = 0x111517;
const MUTED_TITLE: u32 = 0x1e2326;
const MUTED_TEXT: u32 = 0x686e71;
const DEFAULT_BODY: u32 = 0x0d1214;
const NOTE: u32 = 0xa5aeb3;
const WAVE: u32 = 0xb1c7bd;
const CURVE: u32 = 0xd8e0b0;

/// A clip colour in the palette's design: Classic draws the measured value, Lemon its own neutral clip shades with
/// the lemon selection (spec: ui/lemon-design.md §7).
fn cc(t: &os_theme::Palette, v: u32) -> Color {
    if !lk::on(t) {
        return Color::rgb(v);
    }
    use os_theme::design::lemon as brand;
    match v {
        CLIP_BORDER => Color::rgb(0x1e1e1e),
        TITLE_TOP | TITLE_BOTTOM => Color::rgb(0x5c5c5c),
        TITLE_TEXT => Color::rgb(0xf2f2f2),
        SEL_TITLE => brand::MAIN,
        SEL_TEXT => brand::COLORED_DARK,
        SEL_BODY => Color::rgb(0x2e2810),
        MUTED_TITLE => Color::rgb(0x2b2b2b),
        MUTED_TEXT => Color::rgb(0x767676),
        DEFAULT_BODY => Color::rgb(0x1c1c1c),
        NOTE => Color::rgb(0xd6d6d6),
        WAVE => Color::rgb(0xe6e6e6),
        CURVE => brand::HIGHLIGHT,
        _ => t.s(v),
    }
}
/// Tool button centres in the title bar, relative to the window's left edge (§1: 275..442 at x 209).
const TOOL_X: [f32; 9] = [66.0, 86.0, 108.0, 129.0, 150.0, 171.0, 192.0, 213.0, 233.0];
const TOOL_ICONS: [Icon; 9] =
    [Icon::Pencil, Icon::Brush, Icon::Delete, Icon::Mute, Icon::Slip, Icon::Slice, Icon::Select, Icon::Zoom, Icon::Speaker];
/// Picker list row pitch (§1).
const PICK_ROW: f32 = 31.0; // pitch of the measured 30 px rows

pub fn paint(ui: &mut Ui, s: &State, r: Rect, f: &FrameInfo) {
    let t = ui.t;
    window_body(ui, r);
    let outer = r;
    let arr = pl::arrangement_name(s);
    let title = format!("Playlist - {arr}  \u{203a}  {}  \u{203a}", pl::source_name(s, pl::source(s)));
    // The grid covers most of the window and is painted all over (rows and gaps): the frame skips it.
    let (fr, fc) = super::frame_rects(s, Win::Playlist, r);
    let cover = grid_rect(s, fr, fc);
    ui.c.set_cover((fr.h >= 2.0 * crate::app::TITLE_H).then_some(cover));
    let (r, client) = super::framed(ui, s, Win::Playlist, r, &title, 281.0, true);
    ui.c.set_cover(None);
    // The arrangement name opens the arrangement menu; hovered it turns orange with a down arrow (§12, as FL).
    {
        let f = Font::new(17);
        let x0 = r.x + 281.0 + ui.c.text_width(f, "Playlist - ");
        let w = ui.c.text_width(f, &arr);
        let arrow_w = ui.c.text_width(f, "  \u{203a}  ");
        let hit = Rect::new(x0 - 2.0, r.y, w + arrow_w * 0.7, 22.0);
        if ui.hit(hit, Action::PlArrangement, "Arrangements (switch, add, clone, rename, delete)") {
            let bg = ui.t.window_title;
            ui.c.fill_rect(Rect::new(x0 - 1.0, r.y + 1.0, w + arrow_w * 0.7, 20.0), bg);
            let hot = ui.t.h(0xf7a94c);
            ui.c.text(f, x0, r.y + 1.0, hot, &arr);
            let ax = x0 + w + arrow_w * 0.35;
            ui.c.fill_polygon(&[(ax - 4.0, r.y + 9.0), (ax + 4.0, r.y + 9.0), (ax, r.y + 14.0)], hot);
        }
    }

    // Title-bar tools: ▸ options, snap magnet, then the nine tools (§1, §3).
    let ty = r.y + 2.0;
    let b = Rect::new(r.x + 13.0 - 9.0, ty, 18.0, 18.0);
    let hov = ui.hit(b, Action::PlOptions, "Playlist options");
    icons::draw(ui.c, Icon::ArrowRight, b, if hov { t.glyph } else { t.text });
    let b = Rect::new(r.x + 33.0 - 9.0, ty, 18.0, 18.0);
    let hov = ui.hit(b, Action::PlSnap, format!("Snap: {}", s.playlist.snap.label()));
    icons::draw(ui.c, Icon::Magnet, b, if hov || pl::grid_snap(s) != pl::Snap::None { t.glyph } else { t.text });
    for ((tool, icon), cx) in PlTool::ALL.into_iter().zip(TOOL_ICONS).zip(TOOL_X) {
        let b = Rect::new(r.x + cx - 9.0, ty, 18.0, 18.0);
        let on = s.playlist.tool == tool;
        let hov = ui.hit(b, Action::PlTool(tool), tool.hint());
        let ink = tool_face(ui, b, on, hov);
        icons::draw(ui.c, icon, b, ink);
    }
    // Stretch mode (Shift+M, §9b): |<->| after the tools, lit while on.
    {
        let b = Rect::new(r.x + 254.0 - 9.0, ty, 18.0, 18.0);
        let hov = ui.hit(b, Action::PlStretch, if s.playlist.stretch { "Stretch mode: on (Shift+M)" } else { "Stretch mode: off (Shift+M)" });
        let on = s.playlist.stretch;
        let col = tool_face(ui, b, on, hov);
        let (cx, cy) = (b.cx(), b.cy());
        ui.c.fill_rect(Rect::new(cx - 7.0, cy - 5.0, 1.2, 10.0), col);
        ui.c.fill_rect(Rect::new(cx + 5.8, cy - 5.0, 1.2, 10.0), col);
        ui.c.line(cx - 5.0, cy, cx + 4.0, cy, 1.2, col);
        ui.c.fill_polygon(&[(cx - 5.5, cy), (cx - 2.5, cy - 2.5), (cx - 2.5, cy + 2.5)], col);
        ui.c.fill_polygon(&[(cx + 4.5, cy), (cx + 1.5, cy - 2.5), (cx + 1.5, cy + 2.5)], col);
    }
    if r.h < 2.0 * crate::app::TITLE_H {
        return; // collapsed to the title bar
    }

    // A part of its own: a repaint of the grid alone (a scroll) skips it.
    let pick = Rect::new(r.x, client.y, 160.0, client.h);
    ui.part(Seg::PlPicker, pick, |ui| picker(ui, s, pick));

    // Track-header strip: add button and the NOTE / CHAN / PAT filters.
    let strip = Rect::new(r.x + 162.0, client.y + 1.0, 108.0, 38.0);
    if super::lemon(ui) {
        ui.c.fill_rrect(strip, lk::r_btn(t), t.track_header);
    } else {
        ui.c.fill_rect_gradient(strip, t.track_header.shade(0.05), t.track_header.shade(-0.03), 0.0);
    }
    icons::draw(ui.c, Icon::Plus, Rect::new(strip.x + 6.0, client.y + 20.0, 16.0, 16.0), t.glyph);
    for (i, (icon, label)) in [(Icon::Wave, "NOTE"), (Icon::Slide, "CHAN"), (Icon::PianoRoll, "PAT")].into_iter().enumerate() {
        let cx = strip.x + 45.0 + 26.0 * i as f32;
        icons::draw(ui.c, icon, Rect::new(cx - 8.0, client.y + 3.0, 16.0, 16.0), if i == 2 { t.glyph } else { t.text_faint });
        ui.c.text_center(Font::condensed(8, 4).bold(), Rect::new(cx - 14.0, client.y + 25.0, 28.0, 10.0), if i == 2 { t.glyph } else { t.text_faint }, label);
    }

    // Arrangement geometry.
    let head_x = r.x + 162.0;
    let head_w = 98.0;
    let grid = grid_rect(s, r, client);
    debug_assert_eq!(grid, cover);
    let (grid_x, grid_y, grid_w) = (grid.x, grid.y, grid.w);
    let has_markers = !pl::markers(s).is_empty();
    let lane_y = client.y + 42.0;
    let g = PGeom::new(s, grid);
    let (sx, sy) = s.playlist_scroll;
    let bar_w = s.playlist_bar_w;
    let bar_t = (g.ppq * 4) as f32;

    // Overview (horizontal scroll bar with a miniature of the clips) + ruler. The song extends 16 bars past
    // its last clip, at least 100 bars; 500 tracks.
    let last = pl::clips(s).iter().map(|c| c.end() as f32 / bar_t).fold(0.0, f32::max);
    let bars = (last.ceil() + 16.0).max(100.0);
    let (hbar, vbar) = bar_sizes();
    let over = Rect::new(grid_x, client.y + 1.0, grid_w, hbar);
    debug_assert_eq!(over, overview_rect(s, grid));
    super::scrollbar(ui, over, false, sx, grid_w, bars * bar_w, crate::app::Scroll::PlaylistX);
    // High visibility: two lighter edges at the top and bottom of the track, around the thumb (spec:
    // ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility").
    if crate::high_visibility::on() && over.h >= 6.0 {
        for (i, hex) in crate::high_visibility::PLAYLIST_EDGES.into_iter().enumerate() {
            let k = i as f32;
            ui.c.fill_rect(Rect::new(over.x, over.y + k, over.w, 1.0), t.s(hex));
            ui.c.fill_rect(Rect::new(over.x, over.bottom() - 1.0 - k, over.w, 1.0), t.s(hex));
        }
    }
    overview(ui, s, over, bars * bar_t);
    let ruler = ruler_rect(s, grid);
    ruler_paint(ui, s, &g, ruler, f);
    ui.c.fill_rect(Rect::new(grid_x + grid_w + 4.0, client.y + 1.0, vbar, client.h), t.scrollbar);
    let vtrack = Rect::new(grid_x + grid_w + 4.0, grid_y, vbar, (r.bottom() - grid_y).max(0.0));
    super::scrollbar(ui, vtrack, true, sy, vtrack.h, g.rows.total(), crate::app::Scroll::PlaylistY);
    if has_markers {
        markers_paint(ui, s, &g, Rect::new(grid_x, lane_y, grid_w, MARKER_H));
    }

    // Grid background and lines; rows follow the track layout (collapsed groups' members are strips, §6a).
    ui.hit(grid, Action::PlaylistArea, "Playlist");
    let mut visible = Vec::new();
    let mut row = g.track_at(grid.y);
    while row < TRACKS && g.y_of(row) < grid.bottom() {
        visible.push(row);
        row += 1;
    }
    let gap_of = |row: u16| if g.rows.strip(row) { 1.0 } else { 3.0 };
    ui.c.push_clip(grid);
    // Rows and the gaps under them (painted after the grid lines) tile the grid; past the last track it is a gap (the
    // frame skips the grid: `set_cover`).
    let end = visible.last().map_or(grid.y, |&r| g.y_of(r) + g.rows.height(r));
    if end < grid.bottom() {
        ui.c.fill_rect(Rect::new(grid_x, end, grid_w, grid.bottom() - end), t.grid_gap);
    }
    for &row in &visible {
        let (y, h) = (g.y_of(row), g.rows.height(row));
        let c = if pl::track_on(s, row) { t.grid } else { t.grid.mix(t.grid_gap, 0.5) };
        let c = if g.rows.strip(row) { c.mix(t.grid_gap, 0.5) } else { c };
        ui.c.fill_rect(Rect::new(grid_x, y, grid_w, h - gap_of(row)), c);
    }
    grid_lines(ui, &g, bar_w);
    for &row in &visible {
        let (y, h) = (g.y_of(row), g.rows.height(row));
        ui.c.fill_rect(Rect::new(grid_x, y + h - gap_of(row), grid_w, gap_of(row)), t.grid_gap);
    }
    // Time selection (§7).
    if let Some((a, b)) = s.playlist.range {
        let (x0, x1) = (g.x_of(a as f32), g.x_of(b as f32));
        ui.c.fill_rect(Rect::span(x0, x1, grid.y, grid.bottom()), ui.t.tx(0xffffff).with_alpha(18));
        ui.c.fill_rect(Rect::new(x0, grid.y, 1.0, grid.h), t.playhead.with_alpha(90));
        ui.c.fill_rect(Rect::new(x1, grid.y, 1.0, grid.h), t.playhead.with_alpha(90));
    }
    // Clips reaching the painted part of the grid (a scrolled-in strip is a small part, see `App::playlist_scrolled`);
    // a clip draws inside its own rect.
    for (i, c) in pl::clips(s).iter().enumerate() {
        if ui.c.visible(g.clip_rect(c).outset(2.0)) {
            clip_paint(ui, s, &g, i, c);
        }
    }
    // Overlays.
    match s.playlist.overlay {
        Some(Overlay::Marquee(m)) => {
            ui.c.fill_rect(m, Color::rgb(0xff5050).with_alpha(30));
            ui.c.stroke_rrect(m, 0.0, 1.0, Color::rgb(0xff6060));
        }
        Some(Overlay::ZoomBox(m)) => {
            ui.c.fill_rect(m, ui.t.tx(0xffffff).with_alpha(20));
            ui.c.stroke_rrect(m, 0.0, 1.0, ui.t.tx(0xd0d8dc));
        }
        Some(Overlay::Slice { tick, t0, t1 }) => {
            let x = g.x_of(tick as f32);
            let (y0, y1) = (g.y_of(t0.min(t1)), g.y_of(t0.max(t1)) + g.rows.height(t0.max(t1)) - 3.0);
            ui.c.fill_rect(Rect::new(x - 0.5, y0, 1.5, y1 - y0), Color::rgb(0xff7a5a));
        }
        None => {}
    }
    // Playhead.
    if f.playing && f.song {
        let x = g.x_of(f.tick as f32);
        ui.c.fill_rect(Rect::new(x, grid.y, 1.5, grid.h), t.playhead);
    }
    ui.c.pop_clip();

    // Track headers (§6, §6a).
    let heads = Rect::new(head_x, grid_y, head_w, grid.h);
    debug_assert!({
        let p = scroll_parts(s, grid);
        p.ruler == ruler && p.heads == heads && p.lane == has_markers.then(|| Rect::new(grid_x, lane_y, grid_w, MARKER_H))
    });
    ui.c.push_clip(heads);
    for &row in &visible {
        header(ui, s, &g, row, head_x, head_w);
    }
    ui.c.pop_clip();
    resize_zones(ui, s, Win::Playlist, outer);
}

/// The overview bar above `grid` (a miniature of the clips that is also the horizontal scroll bar, §1).
pub fn overview_rect(s: &State, grid: Rect) -> Rect {
    let lane_y = grid.y - if pl::markers(s).is_empty() { 0.0 } else { MARKER_H };
    Rect::new(grid.x, lane_y - 41.0, grid.w, bar_sizes().0)
}

/// The time ruler above `grid`, 3 px under the overview bar: 17 px tall, taller by what thin scroll bars free.
pub fn ruler_rect(s: &State, grid: Rect) -> Rect {
    let o = overview_rect(s, grid);
    Rect::new(grid.x, o.bottom() + 3.0, grid.w, 17.0 + 18.0 - o.h)
}

/// Thickness of the overview bar (the horizontal scroll bar) and of the vertical scroll bar: 18 px, or 9 and 10 px
/// with "Small scrollbars in editors" (FL measured 17 → 9 and about 19 → 10 px; spec: ui/settings-choices.md
/// "Miscellaneous").
fn bar_sizes() -> (f32, f32) {
    if super::small_scrollbars() { (9.0, 10.0) } else { (18.0, 18.0) }
}

/// The grid the playlist would paint now: None while it is collapsed to its title bar.
pub fn grid_of(s: &State) -> Option<Rect> {
    let (fr, fc) = super::frame_rects(s, Win::Playlist, s.win_rect(Win::Playlist));
    (fr.h >= 2.0 * crate::app::TITLE_H).then(|| grid_rect(s, fr, fc))
}

/// The parts of a playlist painted with its grid at `grid` that follow the view (`App::playlist_scrolled`): the
/// grid, the ruler and marker lane above it (sideways), the track headers (up and down); and the scroll bars.
pub struct ScrollParts {
    pub grid: Rect,
    pub ruler: Rect,
    pub lane: Option<Rect>,
    pub heads: Rect,
    pub overview: Rect,
    /// The vertical scroll bar's column.
    pub vbar: Rect,
}

pub fn scroll_parts(s: &State, grid: Rect) -> ScrollParts {
    let overview = overview_rect(s, grid);
    ScrollParts {
        grid,
        ruler: ruler_rect(s, grid),
        lane: (!pl::markers(s).is_empty()).then(|| Rect::new(grid.x, overview.y + 41.0, grid.w, MARKER_H)),
        heads: Rect::new(grid.x - 110.0, grid.y, 98.0, grid.h),
        vbar: Rect::new(grid.right(), overview.y, 4.0 + bar_sizes().1, grid.bottom() - overview.y),
        overview,
    }
}

/// The arrangement grid in a window laid out at `r` with client rect `client` (see `super::frame_rects`).
fn grid_rect(s: &State, r: Rect, client: Rect) -> Rect {
    let grid_x = r.x + 162.0 + 98.0 + 12.0;
    let grid_w = (r.right() - grid_x - 4.0 - bar_sizes().1).max(1.0);
    let grid_y = client.y + 42.0 + if pl::markers(s).is_empty() { 0.0 } else { MARKER_H };
    Rect::new(grid_x, grid_y, grid_w, (r.bottom() - grid_y).max(0.0))
}

/// Picker panel: kind tabs, then the list of clip sources of that kind (§1).
fn picker(ui: &mut Ui, s: &State, p: Rect) {
    let t = ui.t;
    ui.c.fill_rect(p, t.picker);
    for (i, (icon, kind, hint)) in [
        (Icon::PianoRoll, PickKind::Pattern, "Patterns"),
        (Icon::Wave, PickKind::Audio, "Audio clips"),
        (Icon::Slide, PickKind::Automation, "Automation clips"),
    ]
    .into_iter()
    .enumerate()
    {
        let cx = p.x + 23.0 + 47.0 * i as f32;
        let b = Rect::new(cx - 14.0, p.y + 11.0, 28.0, 28.0);
        let hov = ui.hit(b, Action::PlPickTab(kind), hint);
        let on = s.playlist.picker == kind;
        icons::draw(ui.c, icon, Rect::new(cx - 10.0, p.y + 15.0, 20.0, 20.0), if on { t.glyph } else if hov { t.text } else { t.text_faint });
    }
    let items: Vec<ClipSource> = pick_items(s);
    let cur = pl::source(s);
    // spec: ui/playlist.md §1a (picker panel, measured from FL): framed list + 15 px scroll bar, 19 px footer with "+".
    let bottom = p.bottom() - FOOT_H;
    let bx = Rect::span(p.x, p.right() - 6.0, p.y + 44.0, bottom);
    if bx.h < 8.0 {
        return;
    }
    ui.c.fill_rect(bx, ui.t.s(0x131718));
    let list = Rect::span(bx.x + 1.0, bx.right() - 16.0, bx.y + 1.0, bx.bottom() - 1.0);
    ui.c.fill_rect(list, ui.t.s(0x202527));
    ui.hit(list, Action::PlPickList, "");
    let total = items.len() as f32 * PICK_ROW + 4.0;
    let scroll = s.playlist.pick_scroll.min((total - list.h).max(0.0)).max(0.0);
    ui.c.push_clip(list);
    let mut y = list.y + 3.0 - scroll;
    for src in items {
        if y > list.bottom() {
            break;
        }
        if y + PICK_ROW < list.y {
            y += PICK_ROW;
            continue;
        }
        let row = Rect::new(list.x + 2.0, y, list.w - 4.0, PICK_ROW - 1.0);
        let hov = ui.hit(row, Action::PlPick(src), "Select clip source (right-click a pattern for its menu)");
        let filled = pick_used(s, src);
        let k = if hov { 0.05 } else { 0.0 };
        let lemon = super::lemon(ui);
        if filled && lemon {
            lk::outlined(ui.c, row, lk::r_btn(ui.t), ui.t.s(0x2f3539), ui.t.s(0x464e53).shade(k));
            preview(ui, s, src, row.inset(3.0));
        } else if filled {
            // Used source: grey button with a faint preview of its contents (pl_asher.png).
            ui.c.fill_rect(row, ui.t.s(0x2f3539));
            ui.c.fill_rect(row.inset(1.0), ui.t.s(0x464e53).shade(k));
            preview(ui, s, src, row.inset(3.0));
        } else if lemon {
            ui.c.fill_rrect(row, lk::r_btn(ui.t), ui.t.s(0x2a2d2e).shade(k));
        } else {
            // Empty pattern: dark face (owner's FL screenshot).
            ui.c.fill_rect_gradient(row, ui.t.s(0x2a2d2e).shade(k), ui.t.s(0x232627).shade(k), 0.0);
        }
        if src == cur && !filled {
            ui.c.stroke_rrect(row.inset(0.5), if lemon { lk::r_btn(ui.t) } else { 0.0 }, 1.0, ui.t.a(0xa8e44a));
        }
        let ic = Color::rgb(if filled { TITLE_TEXT } else { 0xa6aaab });
        if matches!(src, ClipSource::Pattern(_)) && !filled {
            ui.c.fill_circle(row.x + 7.5, row.cy(), 1.6, ui.t.tx(0xb9bcbd));
        } else {
            for k in 0..3 {
                ui.c.fill_rect(Rect::new(row.x + 4.0, row.cy() - 4.0 + 3.0 * k as f32, 6.0, 1.0), ic);
            }
        }
        ui.c.text_clip(Font::new(14), row.x + 15.0, row.cy() - 8.0, row.w - 18.0, ic, &pl::source_name(s, src));
        if src == cur {
            let ax = list.x - 1.0;
            let mark = if lemon { ui.t.accent } else { Color::rgb(0xe64363) };
            ui.c.fill_polygon(&[(ax, row.cy() - 5.0), (ax + 6.0, row.cy()), (ax, row.cy() + 5.0)], mark);
        }
        y += PICK_ROW;
    }
    ui.c.pop_clip();
    // Scroll bar: bordered dithered track, raised thumb with a dark grip.
    let track = Rect::span(list.right(), bx.right() - 1.0, list.y, list.bottom());
    ui.c.fill_rect(Rect::new(track.x, track.y, 1.0, track.h), ui.t.s(0x131718));
    let tr = Rect::span(track.x + 1.0, track.right(), track.y, track.bottom());
    ui.c.fill_rect(tr, ui.t.s(0x212426));
    let mut yy = tr.y;
    while yy < tr.bottom() {
        ui.c.fill_rect(Rect::new(tr.x, yy, tr.w, 1.0), ui.t.s(0x23272a));
        yy += 2.0;
    }
    let max = (total - list.h).max(0.0);
    let thumb = if max > 0.0 { (list.h / total * tr.h).max(14.0) } else { 14.0 };
    let off = if max > 0.0 { scroll / max * (tr.h - thumb) } else { 0.0 };
    let hov = ui.hit(tr, Action::Scrollbar(crate::app::Scroll::PlPick), "Drag to scroll");
    let th = Rect::new(tr.x, tr.y + off, tr.w, thumb);
    ui.c.fill_rect(th, ui.t.s(0x36383a));
    // A light thumb under High visibility (`os_ui::kit::scroll_thumb`).
    let face = if crate::high_visibility::on() {
        os_ui::kit::scroll_thumb(ui.t, os_ui::kit::State::hover(hov), true)
    } else {
        ui.t.s(0x2c2e30).shade(if hov { 0.06 } else { 0.0 })
    };
    ui.c.fill_rect(th.inset(1.0), face);
    ui.c.fill_rect(Rect::new(th.cx() - 1.0, th.cy() - 2.5, 2.0, 5.0), ui.t.s(0x000000));
    ui.bars.push(crate::app::Bar { target: crate::app::Scroll::PlPick, track: tr, vertical: true, max, view: list.h, thumb });
    // Footer "+": new pattern (find first empty).
    let foot = Rect::new(p.x, bottom, bx.w, FOOT_H);
    let hov = ui.hit(foot, Action::PlPickAdd, "New pattern (find first empty)");
    let pc = if hov { ui.t.tx(0xa0a4a6) } else { ui.t.s(0x545657) };
    let (cx, cy) = ((list.x + list.right()) / 2.0, foot.y + FOOT_H / 2.0);
    ui.c.fill_rect(Rect::new(cx - 3.5, cy - 0.5, 8.0, 2.0), pc);
    ui.c.fill_rect(Rect::new(cx - 0.5, cy - 3.5, 2.0, 8.0), pc);
}

const FOOT_H: f32 = 19.0;

/// Sources listed in the picker: used patterns only (scrolling the selector must not add rows).
pub fn pick_items(s: &State) -> Vec<ClipSource> {
    match s.playlist.picker {
        PickKind::Pattern => {
            let v = crate::patterns::listed(s);
            if v.is_empty() { vec![ClipSource::Pattern(1)] } else { v.into_iter().map(ClipSource::Pattern).collect() }
        }
        PickKind::Audio => s
            .project
            .channels
            .iter()
            .filter(|c| c.kind == os_model::ChannelKind::AudioClip)
            .map(|c| ClipSource::Channel(c.index))
            .collect(),
        PickKind::Automation => s.project.automations.iter().map(|a| ClipSource::Channel(a.channel)).collect(),
    }
}

fn pick_used(s: &State, src: ClipSource) -> bool {
    match src {
        ClipSource::Pattern(n) => s.project.pattern(n).is_some_and(|p| !p.notes.is_empty()),
        _ => true,
    }
}

/// Faint note preview inside a picker button.
fn preview(ui: &mut Ui, s: &State, src: ClipSource, r: Rect) {
    let ClipSource::Pattern(n) = src else { return };
    let Some(p) = s.project.pattern(n) else { return };
    let len = crate::piano::pattern_length(s, n).max(1) as f32;
    let lo = p.notes.iter().map(|n| n.key).min().unwrap_or(0);
    let hi = p.notes.iter().map(|n| n.key).max().unwrap_or(0);
    let span = f32::from(hi - lo + 1).max(6.0);
    let c = ui.t.s(0x5a6268);
    for note in &p.notes {
        let x = r.x + note.position as f32 / len * r.w;
        let w = (note.length.max(s.project.ppq as u32 / 4) as f32 / len * r.w).max(2.0);
        let y = r.bottom() - 2.0 - f32::from(note.key - lo + 1) / span * (r.h - 2.0);
        ui.c.fill_rect(Rect::new(x, y, w.min(r.right() - x).max(0.0), 2.0), c);
    }
}

/// Miniature of the arrangement inside the overview bar (§1).
fn overview(ui: &mut Ui, s: &State, over: Rect, total_ticks: f32) {
    let cs = pl::clips(s);
    if cs.is_empty() {
        return;
    }
    let top = cs.iter().map(|c| c.track).min().unwrap_or(0);
    let bottom = cs.iter().map(|c| c.track).max().unwrap_or(0);
    let span = f32::from(bottom - top + 1).max(1.0);
    let k = over.w / total_ticks.max(1.0);
    let row_h = ((over.h - 6.0) / span).clamp(1.0, 3.0);
    for c in cs {
        let col = pl::source_color(s, c.source).map(rgb).unwrap_or(ui.t.clip).shade(0.2);
        let y = over.y + 3.0 + f32::from(c.track - top) / span * (over.h - 6.0);
        ui.c.fill_rect(Rect::new(over.x + c.position as f32 * k, y, (c.length as f32 * k).max(1.0), row_h), col.with_alpha(170));
    }
}

/// Bar numbers, time selection, song position (§7).
fn ruler_paint(ui: &mut Ui, s: &State, g: &PGeom, ruler: Rect, f: &FrameInfo) {
    let t = ui.t;
    ui.c.fill_rect(ruler, t.ruler);
    ui.hit(ruler, Action::PlRuler, "Song position (right-drag: time selection)");
    ui.c.push_clip(ruler);
    if let Some((a, b)) = s.playlist.range {
        ui.c.fill_rect(Rect::span(g.x_of(a as f32), g.x_of(b as f32), ruler.y, ruler.bottom()), t.playhead.with_alpha(60));
    }
    let bar_w = g.bar_w;
    let every = label_step(bar_w);
    // From the bars whose numbers still reach into view (a number is up to ~40 px wide): which labels are drawn must
    // not depend on the scroll position, or a scrolled ruler would differ from a repainted one.
    let first_bar = ((g.scroll_x - 48.0) / bar_w).floor().max(0.0) as i32;
    let last_bar = ((g.scroll_x + ruler.w) / bar_w).ceil() as i32 + 1;
    for b in first_bar..last_bar {
        if b % every != 0 {
            continue;
        }
        let x = g.xs(f64::from(b) * f64::from(bar_w));
        let size = if b % (every * 4) == 0 { 15.0 } else { 13.0 };
        // Along the ruler's bottom 17 px (all of it unless thin scroll bars made it taller).
        ui.c.text(super::fnt(size), x + 2.0, ruler.bottom() - 17.0 + (16.0 - size), t.ruler_text, &format!("{}", b + 1));
    }
    // Song position: a flag ◤ on the playhead while playing the song, else on the start position (§7: its left edge
    // on the position, its top along the ruler's top).
    let tick = if f.playing && f.song { f.tick as f32 } else { s.playlist.pos as f32 };
    let x = g.x_of(tick);
    let (y0, k) = (ruler.y + 0.5, 9.0);
    ui.c.fill_polygon(&[(x - 0.5, y0 - 0.5), (x + k + 1.0, y0 - 0.5), (x - 0.5, y0 + k + 1.0)], t.s(0x0b1114));
    ui.c.fill_polygon(&[(x, y0), (x + k, y0), (x, y0 + k)], t.playhead);
    ui.c.pop_clip();
}

/// Label every n bars so numbers don't overlap.
fn label_step(bar_w: f32) -> i32 {
    let mut every = 1;
    while bar_w * (every as f32) < 24.0 {
        every *= 2;
    }
    every
}

fn markers_paint(ui: &mut Ui, s: &State, g: &PGeom, lane: Rect) {
    let t = ui.t;
    ui.c.fill_rect(lane, t.ruler.shade(0.03));
    ui.hit(lane, Action::PlMarkers, "Time marker (drag moves, right-click: menu)");
    ui.c.push_clip(lane);
    // A tab per marker (§7): flat left edge on its position, rounded top-right corner, slanted right edge.
    for m in pl::markers(s) {
        let x64 = g.x64(f64::from(m.position));
        let x = x64 as f32;
        let w = pl::marker_w(m);
        if x > lane.right() || x + w < lane.x {
            continue;
        }
        let (top, bottom) = (lane.y + 1.0, lane.bottom());
        let slant = (bottom - top) * 0.45;
        // Offsets from the marker's grid point (`PGeom::xs64`).
        let at = |d: f32| pl::snap(x64 + f64::from(d));
        let right = at(w - slant);
        let tab = [
            (x, bottom),
            (x, top),
            (right - 3.0, top),
            (at(w - slant - 1.2), top + 0.4),
            (at(w - slant - 0.3), top + 1.5),
            (at(w), bottom),
        ];
        ui.c.fill_polygon(&tab, t.ruler.mix(t.track_header, 0.55));
        ui.c.text_clip(Font::new(12), x + 3.0, lane.y + 1.0, right - x - 3.0, t.text, &marker_label(m));
    }
    ui.c.pop_clip();
}

/// A marker's label: its name; a time signature first shows the signature (spec: ui/pattern-time-signature.md §2).
pub(crate) fn marker_label(m: &TimeMarker) -> String {
    if m.kind == TimeMarker::SIGNATURE {
        // FL names a new signature marker after its signature ("3/8", spec: ui/pattern-time-signature.md §4): shown once.
        let sig = format!("{}/{}", m.numerator, m.denominator);
        if m.name.is_empty() || m.name == sig { sig } else { format!("{sig} {}", m.name) }
    } else {
        m.name.clone()
    }
}

/// Vertical lines: grid units (dim), beats, bars (bright); every fourth bar group tinted.
fn grid_lines(ui: &mut Ui, g: &PGeom, bar_w: f32) {
    let t = ui.t;
    let grid = g.grid;
    let mut bar_step = 1;
    while bar_w * (bar_step as f32) < 8.0 {
        bar_step *= 2;
    }
    let first_bar = ((g.scroll_x / bar_w) as i32 / bar_step) * bar_step;
    let n = (grid.w / bar_w).ceil() as i32 + bar_step + 1;
    let beat = t.grid_gap.mix(t.grid, 0.4);
    let unit = pl::grid_ticks(g.ppq, bar_w) as f32 * g.ppt;
    let per_bar = ((bar_w / unit).round() as i32).max(1);
    // Bar b's x on the drawing grid (`PGeom::xs`), plus `d` content pixels.
    let bar_x = |b: i32, d: f32| g.xs(f64::from(b) * f64::from(bar_w) + f64::from(d));
    let mut b = first_bar;
    while b < first_bar + n {
        let x = bar_x(b, 0.0);
        if (b / 4) % 2 == 1 {
            ui.c.fill_rect(Rect::new(x, grid.y, bar_x(b + bar_step, 0.0) - x, grid.h), t.grid_alt.with_alpha(160));
        }
        if bar_step == 1 && per_bar > 1 {
            for q in 1..per_bar {
                let c = if unit * q as f32 % (bar_w / 4.0) < 0.5 { beat } else { beat.mix(t.grid, 0.5) };
                ui.c.fill_rect(Rect::new(bar_x(b, q as f32 * unit), grid.y, 1.0, grid.h), c);
            }
        }
        ui.c.fill_rect(Rect::new(x, grid.y, if bar_step == 1 { 2.0 } else { 1.0 }, grid.h), t.grid_bar);
        b += bar_step;
    }
}

/// `c` with `d` added to each component, clamped (spec: ui/playlist.md §2, "Channel-colour rule").
fn offset(c: Color, d: i16) -> Color {
    let f = |v: u8| (i16::from(v) + d).clamp(0, 255) as u8;
    Color { r: f(c.r), g: f(c.g), b: f(c.b), a: c.a }
}

/// One clip (§2): a near-black outline with rounded top corners, the title strip (pattern clips neutral, audio and
/// automation clips in their channel's colour), the body in the track's colour, then the content.
fn clip_paint(ui: &mut Ui, s: &State, g: &PGeom, i: usize, c: &Clip) {
    let t = ui.t;
    let (r, clip_x) = g.clip_draw_rect(c);
    let full_w = g.clip_rect(c).w;
    // On a collapsed group's strip a clip is a bar in its colour (§6a).
    if g.rows.strip(c.track) {
        let col = if s.playlist.sel.contains(&i) {
            cc(t, SEL_TITLE)
        } else {
            pl::clip_tint(s, c.source)
                .or_else(|| pl::source_color(s, c.source))
                .or_else(|| pl::track_info(s, c.track).and_then(|t| t.color))
                .map_or(cc(t, TITLE_TOP), rgb)
        };
        let col = if c.muted || !pl::track_on(s, c.track) { col.mix(t.grid, 0.6) } else { col };
        ui.c.fill_rrect(r, 1.5, col);
        return;
    }
    let kind = pl::source_kind(s, c.source);
    let selected = s.playlist.sel.contains(&i);
    let track_on = pl::track_on(s, c.track);
    let dim = |col: Color| if track_on { col } else { col.mix(t.grid, 0.5) };
    // Body colour: the track's colour, else the source's (§2 "Body colour source").
    let color = pl::track_info(s, c.track).and_then(|t| t.color).or_else(|| pl::source_color(s, c.source)).map(rgb);
    let darken = |c: Color| c.mix(ui.t.s(0x000000), 0.55);
    let tint = pl::clip_tint(s, c.source).map(rgb);

    // Title strip: (edge rows, highlight row, gradient top, gradient bottom, text).
    let (edge, high, top, bottom, text, body) = if selected {
        let sel = cc(t, SEL_TITLE);
        (sel.shade(-0.25), sel.shade(0.08), sel, sel.shade(-0.1), cc(t, SEL_TEXT), cc(t, SEL_BODY))
    } else if c.muted {
        let m = cc(t, MUTED_TITLE);
        (m.shade(-0.2), m, m, m, cc(t, MUTED_TEXT), cc(t, DEFAULT_BODY))
    } else {
        let body = color.map_or(cc(t, DEFAULT_BODY), darken);
        match (kind, tint) {
            (PickKind::Audio | PickKind::Automation, Some(c)) => {
                (offset(c, -42), offset(c, -8), offset(c, -17), offset(c, -26), cc(t, TITLE_TEXT), body)
            }
            _ => (cc(t, CLIP_BORDER), cc(t, TITLE_TOP), cc(t, TITLE_TOP), cc(t, TITLE_BOTTOM), cc(t, TITLE_TEXT), body),
        }
    };
    // Outline (rounded at the top only); a clip a few pixels wide keeps its colours to its edges.
    let outline = dim(t.s(0x03090d));
    let narrow = r.w < 8.0;
    let lemon = super::lemon(ui);
    // Rect fills with the top corners knocked out (cheaper than rounded paths for hundreds of clips).
    if lemon {
        ui.c.fill_rrect(r, lk::r_btn(t).min(r.w / 2.0).min(r.h / 2.0), outline);
    } else {
        ui.c.fill_rect(r, outline);
    }
    let row = if track_on { t.grid } else { t.grid.mix(t.grid_gap, 0.5) };
    if !narrow && !lemon {
        ui.c.fill_rect(Rect::new(r.x, r.y, 1.0, 1.0), row);
        ui.c.fill_rect(Rect::new(r.right() - 1.0, r.y, 1.0, 1.0), row);
    }
    let side = if narrow { 0.0 } else { 1.0 };
    let inner = Rect::new(r.x + side, r.y + 1.0, (r.w - 2.0 * side).max(1.0), r.h - 2.0);
    let title_h = TITLE_H - 2.0;
    // Edge colour behind the strip (its top corners in the outline's colour), then the highlight row and the gradient.
    // Lemon (spec: ui/lemon-design.md §7): a rounded tile with a flat title strip.
    let ir = (lk::r_btn(t) - 1.0).min(inner.w / 2.0).min(inner.h / 2.0);
    if lemon {
        ui.c.fill_rrect(inner, ir, dim(top));
    } else {
        ui.c.fill_rect(Rect::new(inner.x, inner.y, inner.w, title_h + 1.0), dim(edge));
    }
    if !narrow && inner.w > 4.0 && !lemon {
        ui.c.fill_rect(Rect::new(inner.x, inner.y, 1.0, 1.0), outline);
        ui.c.fill_rect(Rect::new(inner.right() - 1.0, inner.y, 1.0, 1.0), outline);
    }
    if inner.w > 3.0 && !lemon {
        ui.c.fill_rect(Rect::new(inner.x + 1.0, inner.y + 1.0, inner.w - 2.0, 1.0), dim(high));
        ui.c.fill_rect_gradient(Rect::new(inner.x + 1.0, inner.y + 2.0, inner.w - 2.0, title_h - 3.0), dim(top), dim(bottom), 0.0);
    }
    // Body: a slightly darker first row and a darker last row.
    let body_r = g.body(c);
    let body_r = Rect::new(inner.x, body_r.y, inner.w, inner.bottom() - body_r.y);
    // The content is measured from the body's true edges (the drawn ones may be pulled in, `clip_draw_rect`).
    let body_at = Body { r: body_r, x: clip_x + f64::from(side), w: f64::from((full_w - 2.0 * side).max(1.0)) };
    if lemon {
        ui.c.fill_rrect(body_r, ir, dim(body));
        ui.c.fill_rect(Rect::new(body_r.x, body_r.y, body_r.w, ir.min(body_r.h)), dim(body));
    } else {
        ui.c.fill_rect(body_r, dim(body));
        ui.c.fill_rect(Rect::new(body_r.x, body_r.y, body_r.w, 1.0), dim(body.shade(-0.08)));
        ui.c.fill_rect(Rect::new(body_r.x, body_r.bottom() - 1.0, body_r.w, 1.0), dim(body.shade(-0.3)));
    }

    ui.c.push_clip(inner.intersect(&g.grid));
    // Type icon (✕ when muted) and name; a narrow clip shows only its strip.
    let (ix, iy) = (inner.x + 3.0, inner.y + 2.0);
    let ic = dim(text);
    if r.w >= 12.0 {
        if c.muted {
            ui.c.line(ix, iy + 1.0, ix + 7.0, iy + 8.0, 1.2, ic);
            ui.c.line(ix + 7.0, iy + 1.0, ix, iy + 8.0, 1.2, ic);
        } else {
            match kind {
                PickKind::Pattern => {
                    for k in 0..3 {
                        ui.c.fill_rect(Rect::new(ix, iy + 1.0 + 3.0 * k as f32, 8.0, 1.0), ic);
                    }
                }
                PickKind::Audio => {
                    // A bar, a solid arrowhead against it, a shaft and a second arrowhead (FL's audio clip mark).
                    ui.c.fill_rect(Rect::new(ix, iy + 1.0, 1.0, 8.0), ic);
                    ui.c.fill_polygon(&[(ix + 1.0, iy + 2.5), (ix + 4.0, iy + 5.0), (ix + 1.0, iy + 7.5)], ic);
                    ui.c.line(ix + 3.0, iy + 5.0, ix + 8.5, iy + 5.0, 1.0, ic);
                    ui.c.fill_polygon(&[(ix + 6.0, iy + 3.0), (ix + 9.0, iy + 5.0), (ix + 6.0, iy + 7.0)], ic);
                }
                PickKind::Automation => ui.c.line(ix, iy + 8.0, ix + 8.0, iy + 1.0, 1.2, ic),
            }
        }
    }
    if r.w > 16.0 {
        ui.c.text_clip(Font::new(12), inner.x + 14.0, inner.y - 1.0, inner.w - 16.0, ic, &pl::source_name(s, c.source));
    }
    // Content.
    let content = if c.muted { dim(text).with_alpha(90) } else { dim(cc(t, NOTE)) };
    if !(c.muted && kind == PickKind::Pattern) {
        match (kind, c.source) {
            (PickKind::Pattern, ClipSource::Pattern(n)) => pattern_notes(ui, s, g, c, n, body_at, content),
            (PickKind::Audio, ClipSource::Channel(ch)) => {
                // The channel colour lifted by 80 (§2); without one the measured default.
                let wave = tint.map_or(cc(t, WAVE), |c| offset(c, 80));
                let col = if c.muted { content } else { dim(wave) };
                // Colorful waveforms colour a clip that plays; a muted one keeps its grey.
                let waves = (!c.muted).then(|| crate::waves::current(t));
                waveform(ui, s, g, c, ch, body_at, col, waves.as_ref().map(|w| (w, &dim as &dyn Fn(Color) -> Color)));
                if !c.muted {
                    fades(ui, s, g, i, c, body_at, dim(wave));
                }
            }
            (PickKind::Automation, ClipSource::Channel(ch)) => {
                let col = if c.muted { content } else { dim(cc(t, CURVE)) };
                curve(ui, s, g, i, c, ch, body_at, dim(body), col);
            }
            _ => {}
        }
    }
    ui.c.pop_clip();
}

/// A clip's body as drawn (`r`, its edges possibly pulled in by `PGeom::clip_draw_rect`) and its true left edge and
/// width, where the content is measured from: content positions are `x` plus a fixed offset, on the drawing grid
/// (`PGeom::xs64`).
#[derive(Clone, Copy)]
struct Body {
    r: Rect,
    x: f64,
    w: f64,
}

fn pattern_notes(ui: &mut Ui, s: &State, g: &PGeom, c: &Clip, n: u16, body: Body, col: Color) {
    let Some(p) = s.project.pattern(n) else { return };
    if p.notes.is_empty() {
        return;
    }
    let br = body.r;
    let (w0, wl) = pl::window(s, c);
    let (lo, hi) = p.notes.iter().fold((u16::MAX, 0u16), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
    let span = f32::from(hi - lo + 1);
    let h = ((br.h - 4.0) / span).clamp(1.0, 3.0);
    let usable = br.h - 4.0 - h;
    let step = (g.ppq / 4).max(1);
    // Only the notes reaching the painted part of the body.
    let Some(area) = ui.c.paint_bounds() else { return };
    let (x_min, x_max) = (area.x.max(br.x), area.right().min(br.right()));
    let ppt = f64::from(g.ppt);
    for note in &p.notes {
        let pos = f64::from(note.position);
        let len = f64::from(if note.length == 0 { step } else { note.length });
        if pos + len <= w0 || pos >= w0 + wl {
            continue;
        }
        let a = pos.max(w0) - w0;
        let b = (pos + len).min(w0 + wl) - w0;
        let x0 = pl::snap(body.x + a * ppt);
        let x1 = pl::snap(body.x + b * ppt).max(x0 + 1.0);
        if x1 < x_min || x0 > x_max {
            continue;
        }
        let k = if span <= 1.0 { 0.5 } else { f32::from(note.key - lo) / (span - 1.0) };
        let y = pl::snap(f64::from(br.y) + 2.0 + f64::from(usable * (1.0 - k)));
        let y1 = pl::snap(f64::from(y) + f64::from(h));
        ui.c.fill_rect(Rect::new(x0, y, (x1 - x0 - 1.0).max(1.0), y1 - y), col);
    }
}

/// The clip's part of the sample: every device-pixel column spans the minimum to the maximum of its samples, times
/// the clip's fades and gain, filled as one anti-aliased shape (§2, §9a). With `colours` (the colourful waveforms
/// setting, and how the clip dims its colours) each column takes the colour of the frames behind it.
#[allow(clippy::too_many_arguments)]
fn waveform(ui: &mut Ui, s: &State, g: &PGeom, c: &Clip, ch: u16, body: Body, col: Color, colours: Option<(&Waves, &dyn Fn(Color) -> Color)>) {
    let Some(path) = s.project.channel(ch).and_then(|chan| chan.sample_path.clone()) else { return };
    let Some(w) = s.playlist.wave(&path) else { return };
    let Some(area) = ui.c.paint_bounds() else { return };
    let (w0, _) = pl::window(s, c);
    let ms_per_tick = pl::to_src(s, c.source, 1.0);
    let frames_per_ms = f64::from(w.rate) / 1000.0;
    let br = body.r;
    let mid = f64::from(br.y) + f64::from(br.h) / 2.0;
    let amp = f64::from(br.h / 2.0 - 2.0);
    let ppt = f64::from(g.ppt);
    // Columns one device pixel wide, counted from the body's left edge (so a repaint of a strip draws the columns a
    // whole repaint draws), over the painted part and a few columns beyond it (the shape's ends change the pixels
    // next to them).
    let px = 1.0 / f64::from(ui.c.scale.max(1.0));
    let (lo, hi) = (f64::from(area.x) - 3.0 * px, f64::from(area.right()) + 3.0 * px);
    let j0 = ((lo - body.x) / px).floor().max(0.0);
    let j1 = ((hi - body.x) / px).ceil();
    let x1 = (body.x + body.w).min(body.x + j1 * px);
    if body.x + j0 * px >= x1 {
        return;
    }
    // From the clip's start (the body starts a pixel in).
    let side = body.x - g.x64(f64::from(c.position));
    // The fades count the song's milliseconds (`pl::ms_px`), the window the sample's.
    let fade_ms = pl::fade_ms_per_tick(s);
    let total_ms = f64::from(c.length) * fade_ms;
    let (fin, fout, gain) = (c.stored_fade_in(), c.stored_fade_out(), f64::from(c.gain()));
    // Gain of the fades `d` px into the body (the engine's law, os_model::fade_curve).
    let shape = |d: f64| -> f64 {
        let ms = (side + d) / ppt * fade_ms;
        let mut k = gain;
        if let Some((l, t)) = fin {
            k *= os_model::fade_curve(ms / f64::from(l), f64::from(t));
        }
        if let Some((l, t)) = fout {
            k *= os_model::fade_curve((total_ms - ms) / f64::from(l), f64::from(t));
        }
        k
    };
    // Peaks from `da` to `db` px into the body.
    let range = |da: f64, db: f64| -> Option<(f32, f32)> {
        let ms_a = w0 + da / ppt * ms_per_tick;
        let ms_b = w0 + db / ppt * ms_per_tick;
        let fa = (ms_a * frames_per_ms).max(0.0) as usize / w.per;
        let fb = ((ms_b * frames_per_ms).max(0.0) as usize / w.per).max(fa + 1);
        if fa >= w.peaks.len() {
            return None;
        }
        let fb = fb.min(w.peaks.len());
        // Cap the work per column when zoomed far out.
        let stride = ((fb - fa) / 32).max(1);
        Some(w.peaks[fa..fb].iter().step_by(stride).fold((f32::MAX, f32::MIN), |(lo, hi), &(a, b)| (lo.min(a), hi.max(b))))
    };
    let n = ((x1 - body.x) / px - j0).ceil().max(0.0) as usize;
    let mut top: Vec<(f32, f32)> = Vec::with_capacity(n + 2);
    let mut bottom: Vec<(f32, f32)> = Vec::with_capacity(n + 2);
    let mut j = j0;
    let mut x = body.x + j * px;
    while x < x1 {
        let d = j * px;
        let Some((lo, hi)) = range(d, d + px) else { break };
        let k = shape(d + px * 0.5) * amp;
        let (mut yt, mut yb) = (mid - f64::from(hi.clamp(-1.0, 1.0)) * k, mid - f64::from(lo.clamp(-1.0, 1.0)) * k);
        // Keep a hairline where the column is flat (a DC sample, silence).
        if yb - yt < 0.7 {
            let m = (yt + yb) / 2.0;
            (yt, yb) = (m - 0.35, m + 0.35);
        }
        let (yt, yb) = (pl::snap(yt), pl::snap(yb));
        let xm = pl::snap((x + px * 0.5).min(x1));
        if top.is_empty() {
            top.push((pl::snap(x), yt));
            bottom.push((pl::snap(x), yb));
        }
        top.push((xm, yt));
        bottom.push((xm, yb));
        j += 1.0;
        x = body.x + j * px;
    }
    let (Some(&(_, yt)), Some(&(_, yb))) = (top.last(), bottom.last()) else { return };
    let x = pl::snap(x.min(x1));
    top.push((x, yt));
    bottom.push((x, yb));
    let mut outline = top;
    outline.extend(bottom.into_iter().rev());
    // Colorful waveforms (spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"): the same shape,
    // each device-pixel column in the colour of the frames it shows; silence keeps the plain colour.
    let tones = colours.filter(|(w, _)| w.on()).and_then(|_| s.playlist.tones(&path));
    match (colours, tones) {
        (Some((waves, dim)), Some(tones)) => {
            let frames = |d: f64| (w0 + d / ppt * ms_per_tick) * frames_per_ms;
            ui.c.fill_polygon_columns(&outline, &mut |x| {
                let d = ((f64::from(x) - body.x) / px).floor() * px;
                let t = tones.span(frames(d), frames(d + px));
                waves.colour(t.centroid(), t.band_powers()).map_or(col, dim)
            });
        }
        _ => ui.c.fill_polygon(&outline, col),
    }
}

/// An audio clip's fade curves, and its handles while the mouse is over it (§9a): fade triangles at the top
/// corners (at the fades' ends), tension rings in the middle of each curve, the gain half-circle at the bottom. A
/// crossfade is drawn by its later clip, over both bodies: an X of the two fades across the overlap, with its handle
/// in the middle.
fn fades(ui: &mut Ui, s: &State, g: &PGeom, i: usize, c: &Clip, edges: Body, wave: Color) {
    let body = edges.r;
    if body.w < 12.0 || body.h < 10.0 {
        return;
    }
    let white = ui.t.tx(0xffffff);
    let hovered = s.playlist.hover_clip == Some(i);
    let lit = |h: pl::Handle| hovered && s.playlist.hover_handle == Some(h);
    let (fx_in, fx_out) = pl::fade_xs(s, g, c);
    // Points on the drawing grid, from the body's true edges (`Body`).
    let (bottom, h) = (f64::from(body.bottom()), f64::from(body.h));
    let curve = |ui: &mut Ui, xa: f64, xb: f64, t: f32, out: bool| {
        let n = ((xb - xa) / 2.0).ceil().max(2.0) as usize;
        let pts: Vec<(f32, f32)> = (0..=n)
            .map(|k| {
                let u = k as f64 / n as f64;
                let x = if out { 1.0 - u } else { u };
                (pl::snap(xa + (xb - xa) * u), pl::snap(bottom - os_model::fade_curve(x, f64::from(t)) * h))
            })
            .collect();
        ui.c.polyline(&pts, 1.4, white.with_alpha(230));
    };
    // This clip's crossfade with an earlier clip, and whether a later clip draws this clip's fade-out in its own.
    let xf = pl::crossfade(s, g, i);
    let later = os_model::crossfade::partner_after(&s.project, pl::clips(s), i).filter(|&b| pl::crossfade(s, g, b).is_some());
    if let Some((_, t)) = c.stored_fade_in() {
        if xf.is_none() {
            curve(ui, edges.x, f64::from(fx_in), t, false);
        }
        ui.c.fill_rect(Rect::new(fx_in, body.y, 1.0, body.h), white.with_alpha(50));
    }
    if let Some((_, t)) = c.stored_fade_out() {
        if later.is_none() {
            curve(ui, f64::from(fx_out), edges.x + edges.w, t, true);
        }
        ui.c.fill_rect(Rect::new(fx_out - 1.0, body.y, 1.0, body.h), white.with_alpha(50));
    }
    if let Some(xf) = xf {
        curve(ui, edges.x, f64::from(xf.x1), xf.t_in, false);
        curve(ui, edges.x, f64::from(xf.x1), xf.t_out, true);
    }
    // Handles only while the mouse is over the clip (as FL): white, brighter on the one under the mouse.
    if !hovered {
        return;
    }
    let _ = wave;
    let col = |h: pl::Handle| if lit(h) { white } else { white.with_alpha(215) };
    let k = 6.0;
    ui.c.fill_polygon(&[(fx_in, body.y), (fx_in + k, body.y), (fx_in, body.y + k)], col(pl::Handle::FadeIn));
    ui.c.fill_polygon(&[(fx_out, body.y), (fx_out - k, body.y), (fx_out, body.y + k)], col(pl::Handle::FadeOut));
    // The gain handle: a half disc on the body's bottom edge (a shape, not a clipped circle: clips are costly).
    let (gx, gy) = (body.cx(), body.bottom());
    let r = if lit(pl::Handle::Gain) { 5.0 } else { 4.0 };
    let half: Vec<(f32, f32)> = (0..=8).map(|k| {
        let a = std::f32::consts::PI * k as f32 / 8.0;
        (gx - r * a.cos(), gy - r * a.sin())
    }).collect();
    ui.c.fill_polygon(&half, col(pl::Handle::Gain));
    if hovered {
        // The crossfade's handle: a big filled circle at the later clip's gain in the middle, a small ring at the
        // earlier clip's (§9a); where they meet the ring shows inside the circle.
        if let Some(xf) = xf.filter(|xf| xf.x1 - xf.x0 > 10.0) {
            let (hx, hy, ry) = pl::xfade_handle(g, c, &xf);
            let on = lit(pl::Handle::Crossfade);
            ui.c.fill_circle(hx, hy, if on { pl::XFADE_R + 1.0 } else { pl::XFADE_R }, col(pl::Handle::Crossfade));
            ui.c.fill_circle(hx, ry, 2.5, ui.t.s(0x1c2328).with_alpha(200));
            ui.c.stroke_circle(hx, ry, 2.5, 1.1, white);
        }
        for (h, on, xa, xb, out) in [
            (pl::Handle::TensionIn, c.stored_fade_in().filter(|_| !pl::managed(s, c, false)), body.x, fx_in, false),
            (pl::Handle::TensionOut, c.stored_fade_out().filter(|_| !pl::managed(s, c, true)), fx_out, body.right(), true),
        ] {
            let Some((_, t)) = on else { continue };
            if xb - xa <= 10.0 {
                continue;
            }
            let (mx, my) = pl::fade_mid(body, xa, xb, t, out);
            if lit(h) {
                ui.c.fill_circle(mx, my, 5.5, white);
            } else {
                ui.c.fill_circle(mx, my, 4.0, ui.t.s(0x1c2328).with_alpha(200));
                ui.c.stroke_circle(mx, my, 4.0, 1.3, white);
                ui.c.line(mx - 2.2, my + 2.2, mx + 2.2, my - 2.2, 1.1, white);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn curve(ui: &mut Ui, s: &State, g: &PGeom, i: usize, c: &Clip, ch: u16, edges: Body, bg: Color, col: Color) {
    let Some(a) = s.project.automation(ch) else { return };
    if a.points.is_empty() {
        return;
    }
    let Some(paint) = ui.c.paint_bounds() else { return };
    let body = edges.r;
    // A point every 2 px from the body's left edge (so a repaint of a strip draws the points a whole repaint draws),
    // over the painted part and a few points beyond it (the line's ends change the pixels next to them).
    let (lo, hi) = (f64::from(paint.x) - 8.0, f64::from(paint.right()) + 8.0);
    let k0 = ((lo - edges.x) / 2.0).floor().max(0.0);
    let x1 = (edges.x + edges.w).min(edges.x + 2.0 * ((hi - edges.x) / 2.0).ceil());
    let x0 = edges.x + 2.0 * k0;
    if x0 > x1 {
        return;
    }
    // The beat under content x (measured from the clip's start, as `pl::beat_at`).
    let (w0, _) = pl::window(s, c);
    let clip_x = g.x64(f64::from(c.position));
    let beat = |x: f64| w0 + (x - clip_x) / f64::from(g.ppt) / f64::from(g.ppq);
    let point = |x: f64| (pl::snap(x), pl::value_y(body, a.value_at(beat(x)).unwrap_or(0.0)));
    let mut pts = Vec::new();
    let mut k = k0;
    while edges.x + 2.0 * k <= x1 {
        pts.push(point(edges.x + 2.0 * k));
        k += 1.0;
    }
    pts.push(point(x1));
    let (x0, x1) = (pl::snap(x0), pl::snap(x1));
    let mut area = pts.clone();
    area.push((x1, body.bottom()));
    area.push((x0, body.bottom()));
    ui.c.fill_polygon(&area, bg.shade(0.07));
    ui.c.polyline(&pts, 1.5, col);
    // Points and tension handles while the mouse is over the clip (§8).
    if s.playlist.hover_clip == Some(i) {
        for (k, p) in a.points.iter().enumerate() {
            let px = pl::point_x(s, g, c, p.beat);
            if px < body.x - 4.0 || px > body.right() + 4.0 {
                continue;
            }
            let py = pl::value_y(body, p.value);
            ui.c.fill_circle(px, py, 4.0, bg);
            ui.c.stroke_circle(px, py, 4.0, 1.5, col);
            if k > 0 {
                let q = a.points[k - 1];
                let x_a = pl::point_x(s, g, c, q.beat);
                if p.mode.has_tension() && p.mode != AutoMode::Hold && px - x_a > 16.0 {
                    let mid = (q.beat + p.beat) / 2.0;
                    let hx = pl::point_x(s, g, c, mid);
                    let hy = pl::value_y(body, a.value_at(mid).unwrap_or(0.0));
                    ui.c.stroke_circle(hx, hy, 2.5, 1.0, col.with_alpha(200));
                }
            }
        }
    }
}

/// A track header (§6, §6a): a box in the track's colour with the name, "…" and the mute LED. A group is one block
/// in its group track's colour: the group track spans the width with a collapse arrow, its members are indented;
/// a collapsed group's members are bare strips.
fn header(ui: &mut Ui, s: &State, g: &PGeom, row: u16, x: f32, w: f32) {
    const INDENT: f32 = 6.0;
    const RADIUS: f32 = 4.0;
    let t = ui.t;
    let (y, h) = (g.y_of(row), g.rows.height(row));
    let strip = g.rows.strip(row);
    let leader = pl::group_track(s, row);
    let lead = pl::is_group_track(s, row);
    let member = leader.is_some();
    let next_member = pl::track_info(s, row + 1).is_some_and(|x| x.grouped);
    let joins_below = (lead || member) && next_member;
    let on = pl::track_on(s, row);
    // A member takes its group track's colour, so the block reads as one.
    let colored = pl::track_info(s, leader.unwrap_or(row)).and_then(|i| i.color).map(rgb);
    let base = colored.unwrap_or(t.track_header);
    let base = if s.playlist.track == row { base.shade(0.1) } else { base };
    let base = if on { base } else { base.mix(t.grid_gap, 0.4) };
    let (bx, bw) = if member { (x + INDENT, w - INDENT) } else { (x, w) };
    let gap = if strip { 1.0 } else { 3.0 };
    let bh = if joins_below { h } else { h - gap };
    let hr = Rect::new(bx, y, bw, bh);
    if member {
        // The indent beside the members: a dark strip (FL dithers it).
        ui.c.fill_rect(Rect::new(x, y, INDENT, bh), t.s(0x2b3439));
    }
    let (top, bottom) = if super::lemon(ui) { (base, base) } else { (base.shade(0.035), base.shade(-0.03)) };
    let k = if super::lemon(ui) { lk::r_btn(t) + 1.0 } else { RADIUS }.min(bh / 2.0);
    ui.c.fill_rect_gradient(hr, top, bottom, k);
    // Square the corners that continue into the rows above / below.
    if member {
        ui.c.fill_rect(Rect::new(hr.x, hr.y, hr.w, k), top);
    }
    if joins_below {
        let left = if lead { hr.x + k } else { hr.x };
        ui.c.fill_rect(Rect::new(left, hr.bottom() - k, hr.right() - left, k), bottom);
    }
    ui.hit(hr, Action::PlHeader(row), "Track (double-click: rename, right-click: menu, Shift+wheel: move)");
    if strip {
        return;
    }
    // The LED and "…" keep their distance from the row's bottom when the track height changes (Alt+wheel); short
    // rows drop the "…" and keep the name clear of the LED.
    let short = g.rows.row_h < pl::ROW_H - 8.0;
    let led = Rect::new(hr.right() - 19.0, (h - gap - 15.0).max(1.0) + hr.y, 14.0, 14.0);
    ui.hit(led, Action::PlLed(row), "Mute track (right-click / Ctrl+click: solo)");
    let text = if colored.is_some() { ui.t.tx(0xdce8f0) } else { t.text_faint };
    let text = if on { text } else { text.shade(-0.35) };
    let editing = matches!(s.playlist.edit.as_ref().map(|e| e.target), Some(pl::EditTarget::TrackName(r)) if r == row);
    if !editing {
        let name_w = if short { hr.w - 26.0 } else { hr.w - 4.0 };
        ui.c.text_clip(Font::new(16), hr.x + 2.0, hr.y + 1.0, name_w, text, &pl::track_name(s, row));
    }
    if !short {
        ui.c.text(Font::new(14), hr.x + 14.0, led.y - 6.0, text.shade(-0.2), "...");
    }
    let info = pl::track_info(s, row);
    if lead {
        // Collapse arrow left of the LED: up while open, down while collapsed.
        let a = Rect::new(led.x - 18.0, led.y, 14.0, 14.0);
        let hov = ui.hit(a, Action::PlGroupArrow(row), "Collapse / expand the group");
        let col = if hov { ui.t.tx(0xffffff) } else { text };
        let (cx, cy) = (a.cx(), a.cy());
        if info.is_some_and(|i| i.collapsed) {
            ui.c.fill_polygon(&[(cx - 4.5, cy - 2.5), (cx + 4.5, cy - 2.5), (cx, cy + 2.5)], col);
        } else {
            ui.c.fill_polygon(&[(cx - 4.5, cy + 2.5), (cx + 4.5, cy + 2.5), (cx, cy - 2.5)], col);
        }
    }
    if lead && info.is_some_and(|i| i.locked) {
        // FL draws a lock where the LED is on such a group (§6a).
        let (cx, cy) = (led.cx(), led.cy() + 1.0);
        let green = t.led(0xa0dd48);
        ui.c.fill_rrect(Rect::new(cx - 5.0, cy - 2.0, 10.0, 8.0), 1.5, green);
        ui.c.polyline(
            &[(cx - 3.0, cy - 2.0), (cx - 3.0, cy - 5.0), (cx - 1.5, cy - 7.0), (cx + 1.5, cy - 7.0), (cx + 3.0, cy - 5.0), (cx + 3.0, cy - 2.0)],
            1.6,
            green,
        );
    } else {
        super::led(ui, led.cx(), led.cy(), on);
    }
}

/// The face behind a title-bar tool icon; returns the icon colour. Lemon: the active tool is a lit rounded square.
fn tool_face(ui: &mut Ui, b: Rect, on: bool, hov: bool) -> Color {
    let t = ui.t;
    if super::lemon(ui) {
        if on {
            lk::lit(ui.c, b.outset(1.0), lk::r_btn(t), hov);
        } else if hov {
            ui.c.fill_rrect(b.outset(1.0), lk::r_btn(t), t.button_hi);
        }
        return if on { lk::ink_on_lemon() } else { t.glyph };
    }
    if on {
        ui.c.fill_rrect(b.outset(1.0), 3.0, t.glyph.with_alpha(40));
    }
    if on { t.glyph } else if hov { t.text.shade(0.3) } else { t.text }
}

/// The open playlist popup menu and its submenu, and the inline text box (drawn above everything).
pub fn paint_overlays(ui: &mut Ui, s: &State) {
    if let Some(p) = &s.playlist.popup {
        // At the Pop-ups scaling (spec: ui/ui-scaling.md §1).
        ui.scaled(super::popup_scale(ui.c.scale), |ui, k| {
            let (w, h) = ui.c.size();
            let (_, rows) = popup_list(ui, &p.items, p.x / k, p.y / k, (w, h), p.sub, false);
            if let Some(i) = p.sub {
                if let (Some(item), Some(rr)) = (p.items.get(i), rows.get(i)) {
                    popup_list(ui, &item.children, rr.right(), rr.y - 2.0, (w, h), None, true);
                }
            }
        });
    }
    if let Some(e) = &s.playlist.edit
        && let pl::EditTarget::TypeIn(t, norm) = e.target
    {
        super::typein::paint(ui, s, e, t, norm);
    } else if let Some(e) = &s.playlist.edit {
        let t = ui.t;
        let r = Rect::new(e.at.0, e.at.1, 150.0, 20.0);
        ui.hit(r, Action::Swallow, "Type, Enter accepts, Esc cancels");
        ui.c.fill_rect(r, ui.t.s(0x0e1214));
        ui.c.stroke_rrect(r, 0.0, 1.0, t.glyph);
        ui.c.text_clip(Font::new(14), r.x + 4.0, r.y + 2.0, r.w - 8.0, t.text, &e.text);
        let cw = ui.c.text_width(Font::new(14), &e.text).min(r.w - 10.0);
        ui.c.fill_rect(Rect::new(r.x + 5.0 + cw, r.y + 3.0, 1.0, 14.0), t.text);
    }
}

// Row heights follow the display scale like FL's popup menus (spec: ui/ui-scaling.md §2).
fn item_h(m: &os_ui::kit::menu::Metrics, it: &pl::PItem) -> f32 {
    if it.col { 0.0 } else if it.sep { m.sep } else if it.head { m.head } else { m.row }
}

/// Column and y offset of each item: a new column starts at a column break or when the next item would pass
/// `max_h` (long menus such as the channel rack's options wrap, spec: ui/channel-rack.md §17-18).
fn popup_layout(m: &os_ui::kit::menu::Metrics, items: &[pl::PItem], max_h: f32) -> (Vec<(usize, f32)>, usize, f32) {
    let (mut col, mut y, mut h) = (0usize, 0.0f32, 0.0f32);
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let ih = item_h(m, it);
        if it.col || (y > 0.0 && y + ih > max_h) {
            col += 1;
            y = 0.0;
        }
        out.push((col, y));
        y += ih;
        h = h.max(y);
    }
    (out, col + 1, h)
}

/// Whether any entry of a submenu (or of its own submenus) does something.
fn any_enabled(items: &[pl::PItem]) -> bool {
    items.iter().any(|c| !c.head && !c.sep && !c.col && c.enabled && (c.children.is_empty() || any_enabled(&c.children)))
}

/// Draw a popup list at (x, y) kept on screen; returns its rect and each item's row rect.
fn popup_list(ui: &mut Ui, items: &[pl::PItem], x: f32, y: f32, screen: (f32, f32), open: Option<usize>, sub: bool) -> (Rect, Vec<Rect>) {
    let t = ui.t;
    let font = Font::new(14);
    let mut lw: f32 = 80.0;
    let mut kw: f32 = 0.0;
    for it in items {
        lw = lw.max(ui.c.text_width(font, &it.label));
        if !it.key.is_empty() {
            kw = kw.max(ui.c.text_width(font.italic(), it.key));
        }
    }
    let cw = 20.0 + lw + if kw > 0.0 { kw + 24.0 } else { 12.0 } + 14.0;
    let m = os_ui::kit::menu::metrics(ui.c.scale);
    // Text sits where it does at 100 %, moved by half of what the row lost.
    let dy = (m.row - os_ui::kit::menu::ROW) / 2.0;
    let (layout, cols, body_h) = popup_layout(&m, items, (screen.1 - 8.0).max(m.row));
    let (w, h) = (cw * cols as f32, body_h + 2.0 * m.border);
    let x = x.min(screen.0 - w - 2.0).max(0.0);
    let y = y.min(screen.1 - h - 2.0).max(0.0);
    let r = Rect::new(x, y, w, h);
    let mut rows = Vec::with_capacity(items.len());
    let lemon = super::lemon(ui);
    if lemon {
        os_ui::kit::popup(ui.c, t, r);
    } else {
        ui.c.fill_rect(r, t.menu_pop);
        ui.c.stroke_rrect(Rect::new(r.x + m.line, r.y + m.line, r.w - 2.0 * m.line, r.h - 2.0 * m.line), 0.0, m.border, t.menu_pop_border);
    }
    ui.hit(r, Action::Swallow, "");
    for (i, it) in items.iter().enumerate() {
        let (col, off) = layout[i];
        let rr = Rect::new(r.x + m.border + col as f32 * cw, r.y + m.border + off, cw - 2.0 * m.border, item_h(&m, it));
        rows.push(rr);
        if col > 0 && (i == 0 || layout[i - 1].0 != col) {
            ui.c.fill_rect(Rect::new(rr.x - m.border, r.y + m.border, 1.0, r.h - 2.0 * m.border), t.menu_pop_head);
        }
        if it.col {
            continue;
        }
        if it.sep {
            ui.c.fill_rect(Rect::new(rr.x, rr.y + m.sep_line, rr.w, m.line), t.menu_pop_head);
            continue;
        }
        if it.head {
            if lemon {
                ui.c.fill_rrect(Rect::new(rr.x + 3.0, rr.y + 1.0, rr.w - 6.0, rr.h - 2.0), 4.0f32.min(rr.h / 2.0 - 1.0).max(0.0), t.menu_pop_head);
            } else {
                ui.c.fill_rect(rr, t.menu_pop_head);
            }
            ui.c.text(Font::new(12), rr.x + 4.0, rr.y + 1.0 + (rr.h - os_ui::kit::menu::HEAD) / 2.0, t.menu_pop_head_text, &it.label);
            continue;
        }
        let action = if sub { Action::PlPopupSub(i) } else { Action::PlPopup(i) };
        // Greyed out: a disabled entry, or a submenu with nothing available in it (it still opens, to show what is
        // planned). Disabled entries never light up.
        let live = it.enabled && (it.children.is_empty() || any_enabled(&it.children));
        if it.enabled {
            let lit = ui.hit(rr, action, it.label.to_string()) || open == Some(i);
            if lit && lemon {
                os_ui::kit::row_highlight(ui.c, t, rr);
            } else if lit {
                ui.c.fill_rect(rr, t.menu_pop_hi);
            }
        } else if matches!(it.cmd, pl::Cmd::NotYet(_)) {
            ui.hit(rr, action, crate::app::off_text(&it.label));
        } else {
            // Disabled here and now (nothing to merge, the last arrangement), not missing.
            ui.hit(rr, action, it.label.to_string());
        }
        // A working entry drawn dim: a Manage messages entry that is off (spec: ui/settings-choices.md "Hints and
        // warnings: Manage messages and Project data size warning (2026-10-16)").
        let color = if live && !it.dim { t.menu_pop_text } else { t.menu_pop_dim };
        let (mx, my) = (rr.x + 10.0, rr.y + m.row / 2.0);
        match it.mark {
            Mark::Check(true) => {
                ui.c.line(mx - 4.0, my, mx - 1.0, my + 3.0, 1.5, color);
                ui.c.line(mx - 1.0, my + 3.0, mx + 4.0, my - 3.0, 1.5, color);
            }
            // Unticked checks show a faint tick (FL browser options menu, spec ui/browser.md §4).
            Mark::Check(false) => {
                ui.c.line(mx - 4.0, my, mx - 1.0, my + 3.0, 1.5, t.menu_pop_dim.shade(-0.15));
                ui.c.line(mx - 1.0, my + 3.0, mx + 4.0, my - 3.0, 1.5, t.menu_pop_dim.shade(-0.15));
            }
            Mark::Radio(true) => ui.c.fill_circle(mx, my, 3.0, color),
            Mark::Radio(false) => ui.c.stroke_circle(mx, my, 3.5, 1.0, t.menu_pop_dim),
            _ => {}
        }
        ui.c.text_clip(font, rr.x + 20.0, rr.y + 2.0 + dy, lw + 4.0, color, &it.label);
        let right = rr.right() - if it.children.is_empty() { 8.0 } else { 16.0 };
        if !it.key.is_empty() {
            let sc = if live { t.menu_pop_shortcut } else { t.menu_pop_dim.mix(t.menu_pop, 0.35) };
            ui.c.text_right(font.italic(), right, rr.y + 3.0 + dy, sc, it.key);
        }
        if !it.children.is_empty() {
            let ax = rr.right() - 11.0;
            ui.c.line(ax, my - 3.5, ax + 3.5, my, 1.2, t.menu_pop_dim);
            ui.c.line(ax + 3.5, my, ax, my + 3.5, 1.2, t.menu_pop_dim);
        }
    }
    (r, rows)
}
