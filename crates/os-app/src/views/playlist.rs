//! Playlist window.
// spec: ui/main-window.md §5–6

use os_model::ClipSource;
use os_ui::{Rect};

use super::{rgb, window_frame, FrameInfo};
use crate::app::{Action, State, Ui};
use crate::icons::{self, Icon};

/// Pixels per bar and per track row at the default zoom.
pub const BAR_W: f32 = 60.0;
pub const ROW_H: f32 = 48.0;

pub fn paint(ui: &mut Ui, s: &State, r: Rect, f: &FrameInfo) {
    let t = ui.t;
    let client = window_frame(ui, r, "Playlist - Arrangement  \u{203a}  (none)  \u{203a}", 281.0);
    // Toolbar icons inside the title bar.
    let tools = [
        Icon::ArrowRight, Icon::Magnet, Icon::Pencil, Icon::Brush, Icon::Delete, Icon::Mute, Icon::Slip, Icon::Slice, Icon::Select,
        Icon::Zoom, Icon::Speaker,
    ];
    let xs = [215.0, 235.0, 268.0, 288.0, 310.0, 331.0, 352.0, 374.0, 394.0, 415.0, 436.0];
    for (icon, x) in tools.into_iter().zip(xs) {
        let b = Rect::new(x, 85.0, 18.0, 18.0);
        let hov = ui.hit(b, Action::Placeholder("playlist tool"), "Playlist tool");
        icons::draw(ui.c, icon, b, if hov { t.glyph } else { t.text });
    }

    // Picker panel with its tab icons (pattern / audio / automation).
    let picker = Rect::new(r.x, client.y, 160.0, client.h);
    ui.c.fill_rect(picker, t.picker);
    for (i, (icon, on)) in [(Icon::PianoRoll, true), (Icon::Wave, false), (Icon::Slide, false)].into_iter().enumerate() {
        let cx = r.x + 23.0 + 47.0 * i as f32;
        icons::draw(ui.c, icon, Rect::new(cx - 10.0, client.y + 15.0, 20.0, 20.0), if on { t.glyph } else { t.text_faint });
    }
    // Track-header strip: add button and the NOTE / CHAN / PAT filters.
    let strip = Rect::new(r.x + 162.0, client.y + 1.0, 108.0, 38.0);
    ui.c.fill_rect_gradient(strip, t.track_header.shade(0.05), t.track_header.shade(-0.03), 0.0);
    icons::draw(ui.c, Icon::Plus, Rect::new(strip.x + 6.0, client.y + 20.0, 16.0, 16.0), t.glyph);
    for (i, (icon, label)) in [(Icon::Wave, "NOTE"), (Icon::Slide, "CHAN"), (Icon::PianoRoll, "PAT")].into_iter().enumerate() {
        let cx = strip.x + 45.0 + 26.0 * i as f32;
        icons::draw(ui.c, icon, Rect::new(cx - 8.0, client.y + 3.0, 16.0, 16.0), if i == 2 { t.glyph } else { t.text_faint });
        ui.c.text_center(os_ui::Font::condensed(8, 4).bold(), Rect::new(cx - 14.0, client.y + 25.0, 28.0, 10.0), if i == 2 { t.glyph } else { t.text_faint }, label);
    }
    let mut y = client.y + 44.0;
    let max_pat = s.project.patterns.iter().map(|p| p.number).max().unwrap_or(1).max(1);
    for n in 1..=max_pat {
        let name = s.project.patterns.iter().find(|p| p.number == n).and_then(|p| p.name.clone()).unwrap_or(format!("Pattern {n}"));
        let row = Rect::new(r.x + 2.0, y, 142.0, 24.0);
        if ui.hit(row, Action::Swallow, "Pattern") || n == s.pattern {
            ui.c.fill_rect(row, t.window_title.shade(0.03));
        }
        if n == s.pattern {
            ui.c.fill_polygon(&[(row.x + 2.0, row.y + 6.0), (row.x + 9.0, row.y + 12.0), (row.x + 2.0, row.y + 18.0)], t.record);
        }
        ui.c.text_clip(os_ui::Font::new(16), row.x + 17.0, row.y + 2.0, row.w - 20.0, t.text, &name);
        y += 24.0;
    }
    icons::draw(ui.c, Icon::Plus, Rect::new(picker.cx() - 8.0, r.bottom() - 18.0, 16.0, 16.0), t.text_dim);

    // Arrangement.
    let arr = s.project.arrangements.first();
    let head_x = r.x + 162.0;
    let head_w = 98.0;
    let grid_x = head_x + head_w + 12.0;
    let grid_w = r.right() - grid_x - 22.0;
    let grid_y = client.y + 42.0; // overview 107–125, ruler 128–144, rows from 148
    let rows = ((r.bottom() - grid_y) / ROW_H).ceil() as usize + 1;
    let (sx, sy) = s.playlist_scroll;
    let first_row = (sy / ROW_H) as usize;

    // Overview + ruler.
    ui.c.fill_rect(Rect::new(grid_x, client.y + 1.0, grid_w, 18.0), t.overview);
    let ruler = Rect::new(grid_x, client.y + 22.0, grid_w, 17.0);
    ui.c.fill_rect(ruler, t.ruler);
    ui.c.push_clip(ruler);
    let bars_visible = (grid_w / BAR_W).ceil() as i32 + 1;
    let first_bar = (sx / BAR_W) as i32;
    for b in first_bar..first_bar + bars_visible {
        let x = grid_x + b as f32 * BAR_W - sx;
        let size = if b % 4 == 0 { 15.0 } else { 13.0 };
        ui.c.text(crate::views::fnt(size), x + 2.0, ruler.y + (16.0 - size), t.ruler_text, &format!("{}", b + 1));
    }
    ui.c.pop_clip();
    ui.c.fill_rect(Rect::new(grid_x + grid_w + 4.0, client.y + 1.0, 18.0, client.h), t.scrollbar);

    // Track headers and rows.
    let area = Rect::new(head_x, grid_y, head_w + 12.0 + grid_w, r.bottom() - grid_y);
    ui.hit(Rect::new(grid_x, grid_y, grid_w, area.h), Action::PlaylistArea, "Playlist");
    ui.c.push_clip(area);
    ui.c.fill_rect(Rect::new(grid_x, grid_y, grid_w, area.h), t.grid_gap);
    let grid = Rect::new(grid_x, grid_y, grid_w, area.h);
    // Row bodies, then bar tints and lines as full-height columns, then the gaps between rows again
    // (far fewer rectangles than per-row lines).
    for row in first_row..first_row + rows {
        let y = grid_y + row as f32 * ROW_H - sy;
        ui.c.fill_rect(Rect::new(grid_x, y, grid_w, 45.0), t.grid);
    }
    let beat = t.grid_gap.mix(t.grid, 0.4);
    ui.c.push_clip(grid);
    for b in first_bar..first_bar + bars_visible {
        let x = grid_x + b as f32 * BAR_W - sx;
        if (b / 4) % 2 == 1 {
            ui.c.fill_rect(Rect::new(x, grid_y, BAR_W, grid.h), t.grid_alt);
        }
        for q in 1..4 {
            ui.c.fill_rect(Rect::new(x + q as f32 * 15.0, grid_y, 1.0, grid.h), beat);
        }
        ui.c.fill_rect(Rect::new(x, grid_y, 2.0, grid.h), t.grid_bar);
    }
    for row in first_row..first_row + rows {
        let y = grid_y + row as f32 * ROW_H - sy;
        ui.c.fill_rect(Rect::new(grid_x, y + 45.0, grid_w, 3.0), t.grid_gap);
    }
    ui.c.pop_clip();
    for row in first_row..first_row + rows {
        let y = grid_y + row as f32 * ROW_H - sy;
        // Header.
        let hr = Rect::new(head_x, y, head_w, 45.0);
        ui.c.fill_rrect(hr, 4.0, t.track_header);
        let name = arr
            .and_then(|a| a.tracks.iter().find(|tr| tr.number as usize == row + 1))
            .and_then(|tr| tr.name.clone())
            .unwrap_or(format!("Track {}", row + 1));
        ui.c.fill_rect_gradient(hr.inset(0.0), t.track_header.shade(0.035), t.track_header.shade(-0.03), 4.0);
        ui.c.text_clip(os_ui::Font::new(16), hr.x + 2.0, y + 1.0, hr.w - 4.0, t.text_faint, &name);
        ui.c.text(os_ui::Font::new(14), hr.x + 14.0, y + 24.0, t.text_faint.shade(-0.2), "...");
        super::led(ui, hr.right() - 12.0, y + 37.0, true);
    }
    // Clips.
    if let Some(a) = arr {
        for clip in &a.clips {
            let ppq = f32::from(s.project.ppq.max(1));
            let x = grid_x + clip.position as f32 / (ppq * 4.0) * BAR_W - sx;
            let w = (clip.length as f32 / (ppq * 4.0) * BAR_W).max(4.0);
            let y = grid_y + f32::from(clip.track) * ROW_H - sy;
            if x > r.right() || x + w < grid_x || y > r.bottom() || y + 45.0 < grid_y {
                continue;
            }
            let color = match clip.source {
                ClipSource::Pattern(n) => s.project.patterns.iter().find(|p| p.number == n).and_then(|p| p.color).map(rgb),
                ClipSource::Channel(c) => s.project.channels.iter().find(|ch| ch.index == c).and_then(|ch| ch.color).map(rgb),
            }
            .unwrap_or(t.clip);
            let cr = Rect::new(x, y + 1.0, w, 43.0);
            ui.c.fill_rrect(cr, 2.0, color.shade(-0.25));
            ui.c.fill_rrect(Rect::new(x, y + 1.0, w, 12.0), 2.0, color);
            let label = match clip.source {
                ClipSource::Pattern(n) => {
                    s.project.patterns.iter().find(|p| p.number == n).and_then(|p| p.name.clone()).unwrap_or(format!("Pattern {n}"))
                }
                ClipSource::Channel(c) => {
                    s.project.channels.iter().find(|ch| ch.index == c).and_then(|ch| ch.name.clone()).unwrap_or_default()
                }
            };
            ui.c.push_clip(cr);
            ui.c.text(os_ui::Font::new(12), x + 3.0, y, t.ruler.shade(0.0), &label);
            ui.c.pop_clip();
        }
    }
    // Playhead.
    if f.playing && s.song_mode {
        let ppq = f64::from(s.project.ppq.max(1));
        let bars = f.seconds * s.tempo / 60.0 / 4.0;
        let _ = ppq;
        let x = grid_x + bars as f32 * BAR_W - sx;
        ui.c.fill_rect(Rect::new(x, grid_y - 20.0, 1.5, area.h + 20.0), t.playhead);
    }
    ui.c.pop_clip();
}
