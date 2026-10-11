//! Playlist window: tools, picker, track headers, ruler, markers, clips, overlays and popup menus.
// spec: ui/main-window.md §5–6; ui/playlist.md

use os_model::{AutoMode, Clip, ClipSource, TimeMarker};
use os_ui::{Color, Font, Rect};

use super::{resize_zones, rgb, window_body, FrameInfo};
use crate::app::{Action, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::playlist::{self as pl, Mark, Overlay, PGeom, PickKind, PlTool, CLIP_H, MARKER_H, ROW_H, TITLE_H, TRACKS};

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
    let title = format!("Playlist - Arrangement  \u{203a}  {}  \u{203a}", pl::source_name(s, pl::source(s)));
    // The grid covers most of the window and is painted all over (rows and gaps): the frame skips it.
    let (fr, fc) = super::frame_rects(s, Win::Playlist, r);
    let cover = grid_rect(s, fr, fc);
    ui.c.set_cover((fr.h >= 2.0 * crate::app::TITLE_H).then_some(cover));
    let (r, client) = super::framed(ui, s, Win::Playlist, r, &title, 281.0, true);
    ui.c.set_cover(None);

    // Title-bar tools: ▸ options, snap magnet, then the nine tools (§1, §3).
    let ty = r.y + 2.0;
    let b = Rect::new(r.x + 13.0 - 9.0, ty, 18.0, 18.0);
    let hov = ui.hit(b, Action::PlOptions, "Playlist options");
    icons::draw(ui.c, Icon::ArrowRight, b, if hov { t.glyph } else { t.text });
    let b = Rect::new(r.x + 33.0 - 9.0, ty, 18.0, 18.0);
    let hov = ui.hit(b, Action::PlSnap, format!("Snap: {}", s.playlist.snap.label()));
    icons::draw(ui.c, Icon::Magnet, b, if hov || s.playlist.snap != pl::Snap::None { t.glyph } else { t.text });
    for ((tool, icon), cx) in PlTool::ALL.into_iter().zip(TOOL_ICONS).zip(TOOL_X) {
        let b = Rect::new(r.x + cx - 9.0, ty, 18.0, 18.0);
        let on = s.playlist.tool == tool;
        let hov = ui.hit(b, Action::PlTool(tool), tool.hint());
        if on {
            ui.c.fill_rrect(b.outset(1.0), 3.0, t.glyph.with_alpha(40));
        }
        icons::draw(ui.c, icon, b, if on { t.glyph } else if hov { t.text.shade(0.3) } else { t.text });
    }
    if r.h < 2.0 * crate::app::TITLE_H {
        return; // collapsed to the title bar
    }

    picker(ui, s, Rect::new(r.x, client.y, 160.0, client.h));

    // Track-header strip: add button and the NOTE / CHAN / PAT filters.
    let strip = Rect::new(r.x + 162.0, client.y + 1.0, 108.0, 38.0);
    ui.c.fill_rect_gradient(strip, t.track_header.shade(0.05), t.track_header.shade(-0.03), 0.0);
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
    let over = Rect::new(grid_x, client.y + 1.0, grid_w, 18.0);
    super::scrollbar(ui, over, false, sx, grid_w, bars * bar_w, crate::app::Scroll::PlaylistX);
    overview(ui, s, over, bars * bar_t);
    let ruler = Rect::new(grid_x, client.y + 22.0, grid_w, 17.0);
    ruler_paint(ui, s, &g, ruler, f);
    ui.c.fill_rect(Rect::new(grid_x + grid_w + 4.0, client.y + 1.0, 18.0, client.h), t.scrollbar);
    let vtrack = Rect::new(grid_x + grid_w + 4.0, grid_y, 18.0, (r.bottom() - grid_y).max(0.0));
    super::scrollbar(ui, vtrack, true, sy, vtrack.h, f32::from(TRACKS) * ROW_H, crate::app::Scroll::PlaylistY);
    if has_markers {
        markers_paint(ui, s, &g, Rect::new(grid_x, lane_y, grid_w, MARKER_H));
    }

    // Grid background and lines.
    ui.hit(grid, Action::PlaylistArea, "Playlist");
    let first_row = (sy / ROW_H) as u16;
    let rows = ((grid.h / ROW_H).ceil() as u16 + 1).min(TRACKS.saturating_sub(first_row));
    ui.c.push_clip(grid);
    // Rows and the gaps under them (painted after the grid lines) tile the grid; past the last track it is a gap.
    let end = g.y_of(first_row + rows);
    if end < grid.bottom() {
        ui.c.fill_rect(Rect::new(grid_x, end, grid_w, grid.bottom() - end), t.grid_gap);
    }
    for row in first_row..first_row + rows {
        let y = g.y_of(row);
        let c = if pl::track_on(s, row) { t.grid } else { t.grid.mix(t.grid_gap, 0.5) };
        ui.c.fill_rect(Rect::new(grid_x, y, grid_w, CLIP_H), c);
    }
    grid_lines(ui, &g, bar_w);
    for row in first_row..first_row + rows {
        ui.c.fill_rect(Rect::new(grid_x, g.y_of(row) + CLIP_H, grid_w, 3.0), t.grid_gap);
    }
    // Time selection (§7).
    if let Some((a, b)) = s.playlist.range {
        let (x0, x1) = (g.x_of(a as f32), g.x_of(b as f32));
        ui.c.fill_rect(Rect::span(x0, x1, grid.y, grid.bottom()), ui.t.tx(0xffffff).with_alpha(18));
        ui.c.fill_rect(Rect::new(x0, grid.y, 1.0, grid.h), t.playhead.with_alpha(90));
        ui.c.fill_rect(Rect::new(x1, grid.y, 1.0, grid.h), t.playhead.with_alpha(90));
    }
    // Clips.
    for (i, c) in pl::clips(s).iter().enumerate() {
        let cr = g.clip_rect(c);
        if cr.x > grid.right() || cr.right() < grid.x || cr.y > grid.bottom() || cr.bottom() < grid.y {
            continue;
        }
        clip_paint(ui, s, &g, i, c);
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
            let (y0, y1) = (g.y_of(t0.min(t1)), g.y_of(t0.max(t1)) + CLIP_H);
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

    // Track headers (§6).
    let heads = Rect::new(head_x, grid_y, head_w, grid.h);
    ui.c.push_clip(heads);
    for row in first_row..first_row + rows {
        header(ui, s, row, Rect::new(head_x, g.y_of(row), head_w, CLIP_H));
    }
    ui.c.pop_clip();
    resize_zones(ui, s, Win::Playlist, outer);
}

/// The arrangement grid in a window laid out at `r` with client rect `client` (see `super::frame_rects`).
fn grid_rect(s: &State, r: Rect, client: Rect) -> Rect {
    let grid_x = r.x + 162.0 + 98.0 + 12.0;
    let grid_w = (r.right() - grid_x - 22.0).max(1.0);
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
        let hov = ui.hit(row, Action::PlPick(src), "Select clip source (right-click a pattern to rename)");
        let filled = pick_used(s, src);
        let k = if hov { 0.05 } else { 0.0 };
        if filled {
            // Used source: grey button with a faint preview of its contents (pl_asher.png).
            ui.c.fill_rect(row, ui.t.s(0x2f3539));
            ui.c.fill_rect(row.inset(1.0), ui.t.s(0x464e53).shade(k));
            preview(ui, s, src, row.inset(3.0));
        } else {
            // Empty pattern: dark face (owner's FL screenshot).
            ui.c.fill_rect_gradient(row, ui.t.s(0x2a2d2e).shade(k), ui.t.s(0x232627).shade(k), 0.0);
        }
        if src == cur && !filled {
            ui.c.stroke_rrect(row.inset(0.5), 0.0, 1.0, ui.t.a(0xa8e44a));
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
            ui.c.fill_polygon(&[(ax, row.cy() - 5.0), (ax + 6.0, row.cy()), (ax, row.cy() + 5.0)], Color::rgb(0xe64363));
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
    ui.c.fill_rect(th.inset(1.0), ui.t.s(0x2c2e30).shade(if hov { 0.06 } else { 0.0 }));
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
    let len = crate::piano::pattern_length(s, &p.notes).max(1) as f32;
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
    let first_bar = (g.scroll_x / bar_w) as i32;
    let n = (ruler.w / bar_w).ceil() as i32 + 1;
    for b in first_bar..first_bar + n {
        if b % every != 0 {
            continue;
        }
        let x = ruler.x + b as f32 * bar_w - g.scroll_x;
        let size = if b % (every * 4) == 0 { 15.0 } else { 13.0 };
        ui.c.text(super::fnt(size), x + 2.0, ruler.y + (16.0 - size), t.ruler_text, &format!("{}", b + 1));
    }
    // Song position: a triangle at the playhead while playing the song, else at the start position.
    let tick = if f.playing && f.song { f.tick as f32 } else { s.playlist.pos as f32 };
    let x = g.x_of(tick);
    ui.c.fill_polygon(&[(x - 6.0, ruler.y + 4.0), (x + 6.0, ruler.y + 4.0), (x, ruler.bottom() - 2.0)], t.playhead);
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
    for m in pl::markers(s) {
        let x = g.x_of(m.position as f32);
        let w = pl::marker_w(m);
        let tab = Rect::new(x, lane.y + 1.0, w, lane.h - 2.0);
        ui.c.fill_rrect(tab, 2.0, t.track_header.shade(0.08));
        ui.c.fill_rect(Rect::new(x, lane.y, 2.0, lane.h), t.ruler_text);
        ui.c.text_clip(Font::new(12), x + 5.0, lane.y + 1.0, w - 7.0, t.text, &marker_label(m));
    }
    ui.c.pop_clip();
}

fn marker_label(m: &TimeMarker) -> String {
    if m.kind == TimeMarker::SIGNATURE {
        let sig = format!("{}/{}", m.numerator, m.denominator);
        if m.name.is_empty() { sig } else { format!("{sig} {}", m.name) }
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
    let mut b = first_bar;
    while b < first_bar + n {
        let x = grid.x + b as f32 * bar_w - g.scroll_x;
        if (b / 4) % 2 == 1 {
            ui.c.fill_rect(Rect::new(x, grid.y, bar_w * bar_step as f32, grid.h), t.grid_alt.with_alpha(160));
        }
        if bar_step == 1 && per_bar > 1 {
            for q in 1..per_bar {
                let c = if unit * q as f32 % (bar_w / 4.0) < 0.5 { beat } else { beat.mix(t.grid, 0.5) };
                ui.c.fill_rect(Rect::new(x + q as f32 * unit, grid.y, 1.0, grid.h), c);
            }
        }
        ui.c.fill_rect(Rect::new(x, grid.y, if bar_step == 1 { 2.0 } else { 1.0 }, grid.h), t.grid_bar);
        b += bar_step;
    }
}

/// One clip (§2).
fn clip_paint(ui: &mut Ui, s: &State, g: &PGeom, i: usize, c: &Clip) {
    let t = ui.t;
    let r = g.clip_rect(c);
    let kind = pl::source_kind(s, c.source);
    let selected = s.playlist.sel.contains(&i);
    let track_on = pl::track_on(s, c.track);
    let dim = |col: Color| if track_on { col } else { col.mix(t.grid, 0.5) };
    // Body colour: the track's colour, else the source's (§2 "Body colour source").
    let color = pl::track_info(s, c.track).and_then(|t| t.color).or_else(|| pl::source_color(s, c.source)).map(rgb);
    let darken = |c: Color| c.mix(ui.t.s(0x000000), 0.55);

    let (title_top, title_bottom, text, body) = if selected {
        (Color::rgb(SEL_TITLE), Color::rgb(SEL_TITLE).shade(-0.1), Color::rgb(SEL_TEXT), Color::rgb(SEL_BODY))
    } else if c.muted {
        (Color::rgb(MUTED_TITLE), Color::rgb(MUTED_TITLE), Color::rgb(MUTED_TEXT), Color::rgb(DEFAULT_BODY))
    } else {
        match kind {
            PickKind::Pattern => {
                let body = color.map_or(Color::rgb(DEFAULT_BODY), darken);
                (Color::rgb(TITLE_TOP), Color::rgb(TITLE_BOTTOM), Color::rgb(TITLE_TEXT), body)
            }
            PickKind::Audio | PickKind::Automation => {
                let col = color.unwrap_or(t.clip);
                let top = Color::rgb(TITLE_TOP).mix(col, 0.3);
                (top, top.shade(-0.1), Color::rgb(TITLE_TEXT), darken(col))
            }
        }
    };
    ui.c.fill_rect(r, dim(Color::rgb(CLIP_BORDER)));
    let inner = Rect::new(r.x + 1.0, r.y + 1.0, (r.w - 2.0).max(1.0), r.h - 2.0);
    let title_r = Rect::new(inner.x, inner.y, inner.w, TITLE_H - 2.0);
    ui.c.fill_rect_gradient(title_r, dim(title_top), dim(title_bottom), 0.0);
    let body_r = g.body(c);
    let body_r = Rect::new(inner.x, body_r.y, inner.w, inner.bottom() - body_r.y);
    ui.c.fill_rect(body_r, dim(body));

    ui.c.push_clip(inner.intersect(&g.grid));
    // Type icon (✕ when muted) and name.
    let (ix, iy) = (inner.x + 3.0, inner.y + 2.0);
    let ic = dim(text);
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
                ui.c.fill_rect(Rect::new(ix, iy + 1.0, 1.0, 8.0), ic);
                ui.c.line(ix + 1.0, iy + 5.0, ix + 8.0, iy + 5.0, 1.0, ic);
                ui.c.line(ix + 5.0, iy + 2.0, ix + 8.0, iy + 5.0, 1.0, ic);
                ui.c.line(ix + 5.0, iy + 8.0, ix + 8.0, iy + 5.0, 1.0, ic);
            }
            PickKind::Automation => ui.c.line(ix, iy + 8.0, ix + 8.0, iy + 1.0, 1.2, ic),
        }
    }
    if r.w > 16.0 {
        ui.c.text_clip(Font::new(12), inner.x + 14.0, inner.y - 1.0, inner.w - 16.0, ic, &pl::source_name(s, c.source));
    }
    // Content.
    let content = if c.muted { dim(text).with_alpha(90) } else { dim(Color::rgb(NOTE)) };
    if !(c.muted && kind == PickKind::Pattern) {
        match (kind, c.source) {
            (PickKind::Pattern, ClipSource::Pattern(n)) => pattern_notes(ui, s, g, c, n, body_r, content),
            (PickKind::Audio, ClipSource::Channel(ch)) => {
                let col = if c.muted { content } else { dim(Color::rgb(WAVE)) };
                waveform(ui, s, g, c, ch, body_r, col);
            }
            (PickKind::Automation, ClipSource::Channel(ch)) => {
                let col = if c.muted { content } else { dim(Color::rgb(CURVE)) };
                curve(ui, s, g, i, c, ch, body_r, dim(body), col);
            }
            _ => {}
        }
    }
    ui.c.pop_clip();
}

fn pattern_notes(ui: &mut Ui, s: &State, g: &PGeom, c: &Clip, n: u16, body: Rect, col: Color) {
    let Some(p) = s.project.pattern(n) else { return };
    if p.notes.is_empty() {
        return;
    }
    let (w0, wl) = pl::window(s, c);
    let (lo, hi) = p.notes.iter().fold((u16::MAX, 0u16), |(lo, hi), n| (lo.min(n.key), hi.max(n.key)));
    let span = f32::from(hi - lo + 1);
    let h = ((body.h - 4.0) / span).clamp(1.0, 3.0);
    let usable = body.h - 4.0 - h;
    let step = (g.ppq / 4).max(1);
    let (x_min, x_max) = (g.grid.x.max(body.x), g.grid.right().min(body.right()));
    for note in &p.notes {
        let pos = f64::from(note.position);
        let len = f64::from(if note.length == 0 { step } else { note.length });
        if pos + len <= w0 || pos >= w0 + wl {
            continue;
        }
        let a = (pos.max(w0) - w0) as f32;
        let b = ((pos + len).min(w0 + wl) - w0) as f32;
        let x0 = body.x + a * g.ppt;
        let x1 = (body.x + b * g.ppt).max(x0 + 1.0);
        if x1 < x_min || x0 > x_max {
            continue;
        }
        let k = if span <= 1.0 { 0.5 } else { f32::from(note.key - lo) / (span - 1.0) };
        let y = body.y + 2.0 + usable * (1.0 - k);
        ui.c.fill_rect(Rect::new(x0, y, (x1 - x0 - 1.0).max(1.0), h), col);
    }
}

fn waveform(ui: &mut Ui, s: &State, g: &PGeom, c: &Clip, ch: u16, body: Rect, col: Color) {
    let Some(path) = s.project.channel(ch).and_then(|chan| chan.sample_path.clone()) else { return };
    let Some(w) = s.playlist.wave(&path) else { return };
    let (w0, _) = pl::window(s, c);
    let ms_per_tick = pl::to_src(s, c.source, 1.0);
    let frames_per_ms = f64::from(w.rate) / 1000.0;
    let mid = body.cy();
    let amp = body.h / 2.0 - 2.0;
    let x0 = body.x.max(g.grid.x).floor();
    let x1 = body.right().min(g.grid.right());
    let mut x = x0;
    while x < x1 {
        let ms_a = w0 + f64::from((x - body.x) / g.ppt) * ms_per_tick;
        let ms_b = w0 + f64::from((x + 1.0 - body.x) / g.ppt) * ms_per_tick;
        let fa = (ms_a * frames_per_ms).max(0.0) as usize / w.per;
        let fb = ((ms_b * frames_per_ms).max(0.0) as usize / w.per).max(fa + 1);
        if fa >= w.peaks.len() {
            break;
        }
        let fb = fb.min(w.peaks.len());
        // Cap the work per column when zoomed far out.
        let stride = ((fb - fa) / 32).max(1);
        let (mut lo, mut hi) = (0.0f32, 0.0f32);
        for &(a, b) in w.peaks[fa..fb].iter().step_by(stride) {
            lo = lo.min(a);
            hi = hi.max(b);
        }
        let m = hi.max(-lo).min(1.0);
        let hpx = (m * amp).max(0.5);
        ui.c.fill_cell(Rect::new(x, mid - hpx, 1.0, hpx * 2.0), col);
        x += 1.0;
    }
}

#[allow(clippy::too_many_arguments)]
fn curve(ui: &mut Ui, s: &State, g: &PGeom, i: usize, c: &Clip, ch: u16, body: Rect, bg: Color, col: Color) {
    let Some(a) = s.project.automation(ch) else { return };
    if a.points.is_empty() {
        return;
    }
    let x0 = body.x.max(g.grid.x - 2.0);
    let x1 = body.right().min(g.grid.right() + 2.0);
    let mut pts = Vec::new();
    let mut x = x0;
    while x <= x1 {
        let v = a.value_at(pl::beat_at(s, g, c, x)).unwrap_or(0.0);
        pts.push((x, pl::value_y(body, v)));
        x += 2.0;
    }
    let v = a.value_at(pl::beat_at(s, g, c, x1)).unwrap_or(0.0);
    pts.push((x1, pl::value_y(body, v)));
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

/// A track header: colour, name, "…", mute LED; selected track lighter, muted dimmer (§6).
fn header(ui: &mut Ui, s: &State, row: u16, hr: Rect) {
    let t = ui.t;
    let info = pl::track_info(s, row);
    let on = pl::track_on(s, row);
    // A coloured track fills its header with the colour (pl_asher.png: #4879a4 header).
    let colored = info.and_then(|i| i.color).map(rgb);
    let base = colored.unwrap_or(t.track_header);
    let base = if s.playlist.track == row { base.shade(0.1) } else { base };
    let base = if on { base } else { base.mix(t.grid_gap, 0.4) };
    ui.c.fill_rect_gradient(hr, base.shade(0.035), base.shade(-0.03), 4.0);
    let led = Rect::new(hr.right() - 19.0, hr.y + 30.0, 14.0, 14.0);
    let name_r = Rect::new(hr.x, hr.y, hr.w, hr.h);
    ui.hit(name_r, Action::PlHeader(row), "Track (double-click: rename, right-click: menu, Shift+wheel: move)");
    ui.hit(led, Action::PlLed(row), "Mute track (right-click / Ctrl+click: solo)");
    let text = if colored.is_some() { ui.t.tx(0xdce8f0) } else { t.text_faint };
    let text = if on { text } else { text.shade(-0.35) };
    let editing = matches!(s.playlist.edit.as_ref().map(|e| e.target), Some(pl::EditTarget::TrackName(r)) if r == row);
    if !editing {
        ui.c.text_clip(Font::new(16), hr.x + 2.0, hr.y + 1.0, hr.w - 4.0, text, &pl::track_name(s, row));
    }
    ui.c.text(Font::new(14), hr.x + 14.0, hr.y + 24.0, text.shade(-0.2), "...");
    super::led(ui, led.cx(), led.cy(), on);
}

/// The open playlist popup menu and its submenu, and the inline text box (drawn above everything).
pub fn paint_overlays(ui: &mut Ui, s: &State) {
    if let Some(p) = &s.playlist.popup {
        let (w, h) = ui.c.size();
        let (_, rows) = popup_list(ui, &p.items, p.x, p.y, (w, h), p.sub, false);
        if let Some(i) = p.sub {
            if let (Some(item), Some(rr)) = (p.items.get(i), rows.get(i)) {
                popup_list(ui, &item.children, rr.right(), rr.y - 2.0, (w, h), None, true);
            }
        }
    }
    if let Some(e) = &s.playlist.edit {
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
    ui.c.fill_rect(r, t.menu_pop);
    ui.c.stroke_rrect(Rect::new(r.x + m.line, r.y + m.line, r.w - 2.0 * m.line, r.h - 2.0 * m.line), 0.0, m.border, t.menu_pop_border);
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
            ui.c.fill_rect(rr, t.menu_pop_head);
            ui.c.text(Font::new(12), rr.x + 4.0, rr.y + 1.0 + (rr.h - os_ui::kit::menu::HEAD) / 2.0, t.menu_pop_head_text, &it.label);
            continue;
        }
        let action = if sub { Action::PlPopupSub(i) } else { Action::PlPopup(i) };
        let hint: String = if it.enabled { it.label.to_string() } else { "Not available yet".into() };
        if ui.hit(rr, action, hint) || open == Some(i) {
            ui.c.fill_rect(rr, t.menu_pop_hi);
        }
        let color = if it.enabled { t.menu_pop_text } else { t.menu_pop_dim };
        let (mx, my) = (rr.x + 10.0, rr.y + m.row / 2.0);
        match it.mark {
            Mark::Check(true) => {
                ui.c.line(mx - 4.0, my, mx - 1.0, my + 3.0, 1.5, t.menu_pop_text);
                ui.c.line(mx - 1.0, my + 3.0, mx + 4.0, my - 3.0, 1.5, t.menu_pop_text);
            }
            // Unticked checks show a faint tick (FL browser options menu, spec ui/browser.md §4).
            Mark::Check(false) => {
                ui.c.line(mx - 4.0, my, mx - 1.0, my + 3.0, 1.5, t.menu_pop_dim.shade(-0.15));
                ui.c.line(mx - 1.0, my + 3.0, mx + 4.0, my - 3.0, 1.5, t.menu_pop_dim.shade(-0.15));
            }
            Mark::Radio(true) => ui.c.fill_circle(mx, my, 3.0, t.menu_pop_text),
            Mark::Radio(false) => ui.c.stroke_circle(mx, my, 3.5, 1.0, t.menu_pop_dim),
            _ => {}
        }
        ui.c.text_clip(font, rr.x + 20.0, rr.y + 2.0 + dy, lw + 4.0, color, &it.label);
        let right = rr.right() - if it.children.is_empty() { 8.0 } else { 16.0 };
        if !it.key.is_empty() {
            ui.c.text_right(font.italic(), right, rr.y + 3.0 + dy, t.menu_pop_shortcut, it.key);
        }
        if !it.children.is_empty() {
            let ax = rr.right() - 11.0;
            ui.c.line(ax, my - 3.5, ax + 3.5, my, 1.2, t.menu_pop_dim);
            ui.c.line(ax + 3.5, my, ax, my + 3.5, 1.2, t.menu_pop_dim);
        }
    }
    (r, rows)
}
