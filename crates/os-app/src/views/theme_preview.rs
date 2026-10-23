// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Settings > Theme previews: a miniature of the main window (menu bar, toolbar, browser, a playlist and a floating
//! channel rack) drawn from a palette's tokens in its skin's shapes, for the Skins and Themes tiles.
// spec: ui/theme-presets.md "Settings"

use os_theme::Palette;
use os_ui::{Canvas, Color, Rect};

/// Size the miniature is laid out at; `paint` scales it to its rect's width.
pub const W: f32 = 182.0;
pub const H: f32 = 96.0;

/// Draw the miniature of `p` scaled to `r`'s width (`r` is `W` x `H` in shape).
pub fn paint(c: &mut Canvas, p: &Palette, r: Rect) {
    let k = r.w / W;
    let old = c.rescale(c.scale * k);
    layout(c, p, Rect::new(r.x / k, r.y / k, W, r.h / k));
    c.unscale(old);
}

fn layout(c: &mut Canvas, p: &Palette, r: Rect) {
    let lemon = p.style.lemon();
    // The skin's corners at preview size.
    let rad = p.style.radius * 0.5;
    let rad_small = p.style.radius_small * 0.4;
    let rad_panel = p.style.radius_panel * 0.4;
    let bar = |c: &mut Canvas, x: f32, y: f32, w: f32, col: Color| c.fill_rect(Rect::new(x, y, w, 1.5), col);
    c.push_clip(r);
    c.fill_rect(r, p.workspace);
    // Watermark: a soft disc in the workspace.
    c.fill_circle(r.right() - 30.0, r.bottom() - 18.0, 11.0, p.watermark);

    // Menu bar with its items.
    let menu = Rect::new(r.x, r.y, r.w, 7.0);
    c.fill_rect(menu, p.menu_panel);
    let mut x = r.x + 4.0;
    for w in [8.0, 7.0, 10.0, 9.0, 8.0, 11.0] {
        bar(c, x, menu.y + 3.0, w, p.menu_text);
        x += w + 4.0;
    }

    // Toolbar: the PAT/SONG pill, transport buttons (play lit), tempo, time display, record and an LED.
    let tb = Rect::new(r.x, menu.bottom(), r.w, 13.0);
    c.fill_rect(tb, p.toolbar);
    c.fill_rect(Rect::new(tb.x, tb.bottom() - 1.0, tb.w, 1.0), p.toolbar_line);
    let by = tb.y + 3.0;
    c.fill_rrect(Rect::new(tb.x + 4.0, by, 13.0, 7.0), rad, p.pill);
    for i in 0..4 {
        let b = Rect::new(tb.x + 21.0 + i as f32 * 9.0, by, 7.0, 7.0);
        if i == 0 {
            c.fill_rect_gradient(b, p.on_top, p.on_bottom, rad);
            c.fill_rect(Rect::new(b.x + 2.5, b.y + 2.0, 2.0, 3.0), p.on_glyph);
        } else {
            c.fill_rrect(b, rad, p.button);
            c.fill_rect(Rect::new(b.x + 2.0, b.y + 3.0, 3.0, 1.0), if i == 2 { p.record } else { p.glyph });
        }
    }
    let tempo = Rect::new(tb.x + 60.0, by, 18.0, 7.0);
    c.fill_rrect(tempo, rad, p.tempo_box);
    bar(c, tempo.x + 3.0, tempo.y + 3.0, 12.0, p.text);
    let time = Rect::new(tb.x + 82.0, by, 26.0, 7.0);
    c.fill_rrect(time, rad, p.time_panel);
    bar(c, time.x + 3.0, time.y + 3.0, 18.0, p.accent);
    c.fill_circle(tb.x + 114.0, tb.cy(), 2.0, p.accent);
    c.fill_circle(tb.x + 120.0, tb.cy(), 2.0, p.accent_dim);
    let cpu = Rect::new(tb.right() - 30.0, by, 26.0, 7.0);
    c.fill_rrect(cpu, rad, p.sunken_dark);
    for i in 0..5 {
        c.fill_rect(Rect::new(cpu.x + 3.0 + i as f32 * 4.0, cpu.y + 2.0 + (i % 3) as f32, 2.0, 5.0 - (i % 3) as f32), p.meter_low);
    }

    // Browser: tab strip, category rows with their colours, folders, one highlighted row.
    let br = Rect::new(r.x, tb.bottom(), 44.0, r.bottom() - tb.bottom());
    c.fill_rect(br, p.browser_list);
    c.fill_rect(Rect::new(br.x, br.y, br.w, 5.0), p.browser_tabs);
    c.fill_rect(Rect::new(br.x + 3.0, br.y + 1.0, 12.0, 4.0), p.browser_title);
    c.fill_rect(Rect::new(br.right() - 1.0, br.y, 1.0, br.h), p.splitter_dark);
    let cats = [p.cat_project, p.cat_recent, p.cat_plugin_db, p.cat_plugin_presets, p.cat_channel_presets, p.cat_mixer_presets];
    for i in 0..9 {
        let y = br.y + 8.0 + i as f32 * 7.0;
        if y + 5.0 > br.bottom() {
            break;
        }
        if i == 7 {
            c.fill_rect(Rect::new(br.x, y - 1.5, br.w - 1.0, 6.0), p.browser_row_hi);
        }
        let (dot, w) = match cats.get(i) {
            Some(col) => (*col, [22.0, 18.0, 25.0, 20.0, 24.0, 19.0][i]),
            None => (p.browser_folder, [16.0, 21.0, 14.0][i - cats.len()]),
        };
        c.fill_rrect(Rect::new(br.x + 3.0, y, 3.0, 3.0), if lemon { 1.0 } else { 0.0 }, dot);
        bar(c, br.x + 9.0, y + 1.0, w, if i < cats.len() { dot } else { p.browser_folder });
    }

    // Playlist window: title, track headers, grid with bar groups, clips, the playhead.
    let pl = Rect::new(br.right() + 4.0, tb.bottom() + 4.0, r.right() - br.right() - 8.0, 50.0);
    window(c, p, pl, rad_panel);
    let grid = Rect::new(pl.x + 23.0, pl.y + 6.0, pl.w - 24.0, pl.h - 7.0);
    let rows = 5;
    let row_h = grid.h / rows as f32;
    let bar_w = grid.w / 8.0;
    for i in 0..rows {
        let y = grid.y + i as f32 * row_h;
        c.fill_rect(Rect::new(pl.x + 1.0, y, 21.0, row_h - 1.0), p.track_header);
        bar(c, pl.x + 4.0, y + row_h / 2.0 - 1.0, 11.0, p.text_faint);
        for b in 0..8 {
            let col = if (b / 4) % 2 == 1 { p.grid_alt } else { p.grid };
            // Under the row line too: a theme's line strength may make it see-through.
            c.fill_rect(Rect::new(grid.x + b as f32 * bar_w, y, bar_w, row_h), col);
        }
        c.fill_rect(Rect::new(grid.x, y + row_h - 1.0, grid.w, 1.0), p.pl_line(p.grid_gap));
    }
    for b in 1..8 {
        c.fill_rect(Rect::new(grid.x + b as f32 * bar_w, grid.y, 1.0, grid.h), p.pl_line(if b % 4 == 0 { p.grid_bar } else { p.grid_gap }));
    }
    for (row, from, len) in [(0, 0.0, 4.0), (1, 2.0, 2.0), (1, 4.0, 3.0), (2, 1.0, 6.0), (3, 4.0, 4.0)] {
        let clip = Rect::new(grid.x + from * bar_w + 0.5, grid.y + row as f32 * row_h + 0.5, len * bar_w - 1.0, row_h - 2.0);
        c.fill_rrect(clip, rad_small, p.clip.shade(-0.25));
        c.fill_rrect(Rect::new(clip.x, clip.y, clip.w, 2.5), rad_small, p.clip.shade(0.3));
        c.fill_rect(Rect::new(clip.x + 1.0, clip.y + 3.5, clip.w - 2.0, clip.h - 4.5), p.clip);
    }
    c.fill_rect(Rect::new(grid.x + bar_w * 3.4, grid.y, 1.0, grid.h), p.playhead);

    // Channel rack: rows of name buttons and steps (groups of four, a few lit), over the playlist's corner.
    let rk = Rect::new(br.right() + 12.0, pl.bottom() - 6.0, 106.0, r.bottom() - pl.bottom() + 4.0);
    window(c, p, rk, rad_panel);
    let lit = [0b1000_1000_1000_1000u16, 0b0010_0000_0010_0000, 0b0000_1000_0000_1001];
    for (i, row) in lit.iter().enumerate() {
        let y = rk.y + 8.0 + i as f32 * 8.0;
        if y + 6.0 > rk.bottom() {
            break;
        }
        c.fill_rrect(Rect::new(rk.x + 4.0, y, 9.0, 6.0), rad_small, p.rack_row_box);
        c.fill_rrect(Rect::new(rk.x + 15.0, y, 20.0, 6.0), rad_small, if i == 0 { p.name_button_sel } else { p.name_button });
        bar(c, rk.x + 17.0, y + 2.5, 12.0, p.name_text);
        for st in 0..16 {
            let on = row & (1 << (15 - st)) != 0;
            let face = if on { p.step_on } else if (st / 4) % 2 == 1 { p.step_b } else { p.step_a };
            c.fill_rrect(Rect::new(rk.x + 38.0 + st as f32 * 4.0, y, 3.2, 6.0), rad_small * 0.6, face);
        }
    }
    c.pop_clip();
}

/// A floating window: border, body and title bar with its caption.
fn window(c: &mut Canvas, p: &Palette, r: Rect, rad: f32) {
    c.fill_rrect(r, rad, p.window_border);
    let inner = r.inset(1.0);
    c.fill_rrect(inner, rad, p.window_body);
    c.fill_rrect(Rect::new(inner.x, inner.y, inner.w, 5.0), rad, p.window_title);
    c.fill_rect(Rect::new(inner.x, inner.y + 3.0, inner.w, 2.0), p.window_title);
    c.fill_rect(Rect::new(inner.x + 4.0, inner.y + 2.0, 18.0, 1.5), p.text);
}
