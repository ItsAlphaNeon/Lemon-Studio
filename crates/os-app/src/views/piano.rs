// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Piano roll window: toolbar, keyboard, note grid, ruler, overview and the velocity panel.
// spec: ui/piano-roll.md

use os_model::Note;
use os_ui::kit::lemon as lk;
use os_ui::{Color, Font, Rect};

use super::{resize_zones, window_body, FrameInfo};
use crate::app::{Action, Live, Scroll, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::piano::{self, Geom, Lane, Overlay, Tool};

/// Default grid row pitch: 17 px of fill plus a 1 px separator (§3); the vertical zoom changes it (§3a).
pub const ROW: f32 = 18.0;
/// Highest key shown (B10); the lowest is 0 (C0) (§3).
pub const TOP_KEY: i32 = 131;

/// Key name in FL's naming (key 60 = C5) (§3), as the settings choose it (`crate::note_names`).
pub fn key_name(key: i32) -> String {
    crate::note_names::note_name(key)
}

pub fn is_black(key: i32) -> bool {
    matches!(key.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// `c` minus `d` on every channel (the grid's line colors are the row fill minus a constant, §3), the constant scaled by
/// the theme's grid line strength (`PRGridContrast`, spec: ui/flstheme.md §3).
fn sub(t: &os_theme::Palette, c: Color, d: u8) -> Color {
    let d = (f32::from(d) * t.pr_lines).round().clamp(0.0, 255.0) as u8;
    Color { r: c.r.saturating_sub(d), g: c.g.saturating_sub(d), b: c.b.saturating_sub(d), a: c.a }
}

/// The bar line colour at the theme's grid line strength (spec: ui/flstheme.md §3, PRGridContrast).
fn bar_line(t: &os_theme::Palette) -> Color {
    t.pr_line(t.pr_bar)
}

/// The regions of the window for a given window rect `r` (§1; offsets from the client corner at
/// x 209, y 83).
pub struct Layout {
    pub tab: Rect,
    pub overview: Rect,
    pub ruler: Rect,
    pub keys: Rect,
    pub grid: Rect,
    pub vscroll: Rect,
    pub splitter: Rect,
    pub label: Rect,
    pub panel: Rect,
}

/// `split`: how the height under the ruler is shared by the grid and the event panel (spec:
/// ui/event-panel-splitter.md; the default puts the splitter at bottom − 185, §1).
///
/// The overview (the horizontal scroll bar) is 22 px tall and the vertical scroll bar's column 22 px wide; with
/// "Small scrollbars in editors" 11 and 12 px (about half, as FL's playlist measured; spec: ui/settings-choices.md
/// "Miscellaneous"), the room going to the ruler and the grid.
pub fn layout(r: Rect, split: crate::piano_split::Split) -> Layout {
    use crate::piano_split::BAND;
    let (hbar, vcol) = if super::small_scrollbars() { (11.0, 12.0) } else { (22.0, 22.0) };
    let bottom = r.bottom();
    let gx = r.x + 73.0;
    let gr = r.right() - vcol - 1.0;
    let gy = r.y + 64.0;
    let split = gy + split.grid_h(bottom - gy - BAND);
    Layout {
        tab: Rect::new(r.x, r.y + 23.0, 71.0, 41.0),
        overview: Rect::new(r.x + 72.0, r.y + 23.0, gr - r.x - 72.0 + 2.0, hbar),
        ruler: Rect::new(r.x + 72.0, r.y + 23.0 + hbar, gr - r.x - 72.0 + 2.0, 41.0 - hbar),
        keys: Rect::new(r.x, gy, 71.0, split - gy),
        grid: Rect::new(gx, gy, gr - gx, split - gy),
        vscroll: Rect::new(gr + 1.0, r.y + 23.0, r.right() - gr - 1.0, split - r.y - 23.0),
        splitter: Rect::new(r.x, split, r.w, BAND),
        label: Rect::new(r.x, split + BAND, 71.0, bottom - split - BAND),
        panel: Rect::new(gx, split + BAND, gr - gx, bottom - split - BAND),
    }
}

/// The note grid the piano roll would paint now (where it opens, before it is painted).
pub fn grid_rect(s: &State) -> Rect {
    let (r, _) = super::frame_rects(s, Win::PianoRoll, s.win_rect(Win::PianoRoll));
    layout(r, s.piano.split).grid
}

pub fn paint(ui: &mut Ui, s: &State, r: Rect, f: &FrameInfo) {
    let t = ui.t;
    let channel = s.project.channels.get(s.selected_channel);
    let name = channel.and_then(|c| c.name.clone()).unwrap_or_else(|| "(none)".into());
    let title = "Piano roll - ";
    window_body(ui, r);
    let outer = r;
    let floating = !s.wins[Win::PianoRoll.slot().unwrap_or(0)].maxed && r.h >= 2.0 * crate::app::TITLE_H;
    let (r, _) = super::framed(ui, s, Win::PianoRoll, r, title, 365.0, true);
    // The target channel: name and arrow, orange with a ▾ while hovered (spec: ui/piano-target-channel.md §1).
    let font = Font::new(17);
    let nx = r.x + 365.0 + ui.c.text_width(font, title);
    let nw = ui.c.text_width(font, &name);
    let hov = ui.hit(
        Rect::new(nx - 2.0, r.y + 2.0, nw + 22.0, 19.0),
        Action::PianoTarget,
        "Target channel (wheel: channels with notes; right-click: settings)",
    );
    let orange = t.h(0xfb9531);
    ui.c.text(font, nx, r.y + 1.0, if hov { orange } else { t.text }, &name);
    let (ax, ay) = (nx + nw + 11.0, r.y + 12.0);
    if hov {
        os_ui::kit::triangle(ui.c, ax, ay, 4.0, os_ui::kit::Dir::Down, orange);
    } else {
        os_ui::kit::triangle(ui.c, ax, ay, 4.0, os_ui::kit::Dir::Right, t.text_dim);
    }
    let shift = if floating { -4.0 } else { 0.0 };
    toolbar(ui, s, r, shift);
    if r.h < 2.0 * crate::app::TITLE_H {
        return; // collapsed to the title bar
    }
    let l = layout(r, s.piano.split);
    let g = Geom::new(s, l.grid);
    let ch = channel.map(|c| c.index);
    let pat = s.project.patterns.iter().find(|p| p.number == s.pattern);
    let notes: &[Note] = pat.map_or(&[], |p| &p.notes);
    let sel = s.piano.selection(s.pattern, ch);
    // Only a ruler time selection shades the grid, and only its columns (§10).
    let range = s.piano.range.map(|(a, b)| (g.x_of(a as f32), g.x_of(b as f32)));

    // Note-colour tab (§6).
    let tab = l.tab;
    let lemon = super::lemon(ui);
    ui.c.fill_rect(tab, t.window_title);
    let green = t.note_top.mix(t.note_bottom, 0.6);
    if lemon {
        // Lemon (spec: ui/lemon-design.md §6): a rounded colour tile instead of the slanted tab.
        ui.c.fill_rrect(Rect::new(tab.x + 3.0, tab.y + 3.0, tab.w - 6.0, tab.h - 6.0), lk::r_btn(t), green);
    } else {
        ui.c.fill_polygon(&[(tab.x, tab.bottom()), (tab.x + 31.0, tab.y), (tab.right(), tab.y), (tab.right(), tab.bottom())], green);
        ui.c.line(tab.x + 0.5, tab.bottom() - 0.5, tab.x + 31.0, tab.y + 0.5, 1.0, t.note_top.shade(0.15));
    }
    let ink = green.shade(-0.3);
    ui.c.polyline(&[(tab.right() - 15.0, tab.y + 16.0), (tab.right() - 10.0, tab.y + 16.0), (tab.right() - 7.0, tab.y + 7.0), (tab.right() - 3.0, tab.y + 7.0)], 1.6, ink);
    ui.c.fill_polygon(&[(tab.right() - 4.0, tab.bottom() - 18.0), (tab.right() - 4.0, tab.bottom() - 5.0), (tab.right() - 17.0, tab.bottom() - 5.0)], ink);
    // Note colours: not available yet, greyed out.
    ui.off(tab, "Note color");
    ui.veil(tab, t.window_title);

    overview(ui, s, &l, &g, notes, ch);
    // A time-signature marker shows its signature (spec: ui/pattern-time-signature.md §2).
    let markers: Vec<(u32, String)> =
        s.project.pattern(s.pattern).map_or(&[][..], |p| &p.markers).iter().map(|m| (m.position, super::playlist::marker_label(m))).collect();
    ruler(ui, &l, &g, range, s.piano.marker, &markers);
    // Lit keys: the one held with the mouse and this channel's notes playing now (§4).
    let mut lit: Vec<i32> = s.piano.held.into_iter().collect();
    lit.extend(s.piano.typed.iter().filter(|&&(c, _)| Some(c) == ch).map(|&(_, k)| k));
    lit.extend(f.sounding.iter().filter(|&&(c, _)| Some(c) == ch).map(|&(_, k)| i32::from(k)));
    ui.live(Live::Sounding, l.keys);
    keyboard(ui, &l, &g, &lit);
    grid(ui, &l, &g, range);

    // Notes: ghosts of the other channels first (when shown, Alt+V), then this channel's (§5).
    ui.hit(l.grid, Action::PianoGrid, "Piano roll: click to add a note, drag a note to move it or its end to resize, right-click to delete");
    ui.c.push_clip(l.grid);
    let ghost = t.text_faint.with_alpha(110);
    for n in notes.iter().filter(|n| s.piano.ghosts && Some(n.channel) != ch) {
        if let Some(nr) = g.note_rect(n).filter(|nr| nr.intersect(&l.grid).w > 0.0) {
            ui.c.stroke_rrect(nr.inset(1.5), if lemon { 3.0 } else { 2.0 }, 1.0, ghost);
        }
    }
    let label_font = Font::new(11);
    for (i, n) in notes.iter().enumerate().filter(|(_, n)| Some(n.channel) == ch) {
        let Some(nr) = g.note_rect(n) else { continue };
        let vis = nr.intersect(&l.grid);
        if vis.w <= 0.0 || vis.h <= 0.0 {
            continue;
        }
        let selected = sel.contains(&i);
        let hint = format!("{}  -  velocity {}%", key_name(i32::from(n.key)), (u32::from(n.velocity) * 100 + 64) / 128);
        ui.hit(vis, Action::PianoNote(i), hint);
        let (top, bottom, rim, text) = if selected {
            (t.note_sel_top, t.note_sel_bottom, t.note_sel_rim, t.note_sel_text)
        } else if n.muted() {
            // Muted (§9): grey body, measured from FL.
            (ui.t.s(0x6d7276), ui.t.s(0x63686c), ui.t.s(0x474a4d), ui.t.tx(0xc4c8cc))
        } else {
            (t.note_top, t.note_bottom, t.note_rim, t.note_text)
        };
        let body = Rect::new(nr.x + 1.0, nr.y + 1.0, (nr.w - 2.0).max(1.0), nr.h - 2.0);
        if lemon {
            // Rounded-square notes: rim, then the body as a soft top-light gradient.
            let rad = 3.5f32.min(nr.w / 2.0).min(nr.h / 2.0);
            ui.c.fill_rrect(nr, rad, rim);
            ui.c.fill_rect_gradient(body, top, bottom, (rad - 1.0).max(0.0));
        } else {
            ui.c.fill_rrect(nr, 2.0, rim);
            ui.c.fill_rect_gradient(body, top, bottom, 1.0);
        }
        if nr.w > 6.0 && !lemon {
            let hi = top.mix(bottom, 0.45);
            ui.c.fill_rect(Rect::new(body.x, body.y + 1.0, 1.0, body.h - 2.0), hi);
            ui.c.fill_rect(Rect::new(body.right() - 1.0, body.y + 1.0, 1.0, body.h - 2.0), hi);
        }
        // A repeating note shows a tick at every repeat interval inside its body (spec §7a).
        if let Some(iv) = n.repeat_ticks(s.project.ppq) {
            let end = n.position + piano::shown_len(s, n);
            let mut at = n.position + iv;
            let px = g.x_of((n.position + iv) as f32) - g.x_of(n.position as f32);
            while at < end && px >= 2.0 {
                let x = g.x_of(at as f32).floor();
                if x >= l.grid.x && x < l.grid.right() {
                    ui.c.fill_rect(Rect::new(x, body.y, 1.0, body.h), rim);
                }
                at += iv;
            }
        }
        // A slide note shows a small filled triangle at its right end, a note with its own portamento switch a small
        // curved stroke (spec: ui/settings-choices.md "Input: Click-and-hold and both mouse buttons (2026-10-16)";
        // flp/events/patterns.md "Note flags"; audio/note-rules.md). The key name keeps clear of them.
        let mut room = nr.w;
        if (n.slide() || n.porta()) && body.w >= 8.0 && body.h >= 4.0 {
            let h = (body.h - 4.0).clamp(3.0, 9.0);
            let w = (h * 0.7).min(body.w / 2.0 - 1.0);
            let (mut x1, cy) = (body.right() - 2.0, body.y + body.h / 2.0);
            if n.slide() {
                ui.c.fill_polygon(&[(x1 - w, cy - h / 2.0), (x1, cy), (x1 - w, cy + h / 2.0)], text);
                room = x1 - w - 2.0 - nr.x;
                x1 -= w + 2.0;
            }
            if n.porta() && x1 - w > body.x {
                // A quarter of an ellipse rising from the lower left to the upper right.
                let pts: Vec<(f32, f32)> = (0..=8)
                    .map(|k| {
                        let a = k as f32 / 8.0 * std::f32::consts::FRAC_PI_2;
                        (x1 - w * a.cos(), cy + h / 2.0 - h * a.sin())
                    })
                    .collect();
                ui.c.polyline(&pts, 1.5, text);
                room = x1 - w - 2.0 - nr.x;
            }
        }
        if room > 12.0 && g.row >= 13.0 {
            let label = key_name(i32::from(n.key));
            let w = ui.c.text_width(label_font, &label);
            let shown = if w <= room - 5.0 { label } else { format!("{}..", &label[..1]) };
            ui.c.text_clip(label_font, nr.x + if lemon { 4.0 } else { 3.0 }, nr.y + 2.0, room - 4.0, text, &shown);
        }
    }
    // Tool overlays (§9): marquee and zoom box (orange, rounded), slice and scrub lines.
    match s.piano.overlay {
        Some(Overlay::Marquee(m) | Overlay::ZoomBox(m)) => {
            let orange = t.pill;
            ui.c.fill_rrect(m, 4.0, orange.with_alpha(48));
            ui.c.stroke_rrect(m.inset(0.5), 4.0, 1.5, orange);
        }
        Some(Overlay::Slice(tick)) => {
            let x = g.x_of(tick as f32).round();
            ui.c.fill_rect(Rect::new(x - 1.0, l.grid.y, 2.0, l.grid.h), t.pill);
        }
        Some(Overlay::Scrub(tick)) => {
            let x = g.x_of(tick).round();
            ui.c.fill_rect(Rect::new(x, l.grid.y, 1.0, l.grid.h), ui.t.tx(0xffffff).with_alpha(200));
        }
        None => {}
    }
    // Playhead: the pattern's own in pattern mode; in song mode where the song is in this pattern, while a clip of
    // it plays there.
    let head = if !f.playing { None } else if f.song { piano::pattern_tick_in_song(s, f.tick) } else { Some(f.tick) };
    if let Some(tick) = head {
        let x = g.x_of(tick as f32);
        ui.c.fill_rect(Rect::new(x, l.grid.y, 1.5, l.grid.h), t.playhead);
    } else if let Some(tick) = (!f.playing && s.song_mode).then(|| piano::pattern_tick_in_song(s, f64::from(s.playlist.pos))).flatten() {
        // Stopped in song mode on a place where this pattern plays: a grey line there.
        let x = g.x_of(tick as f32).round();
        ui.c.fill_rect(Rect::new(x - 1.0, l.grid.y, 3.0, l.grid.h), ui.t.s(0x38373b));
        ui.c.fill_rect(Rect::new(x, l.grid.y, 1.0, l.grid.h), ui.t.s(0x53585c));
    } else if !f.playing {
        // Stopped: a grey line at the play-start marker (§10).
        let x = g.x_of(s.piano.marker as f32).round();
        ui.c.fill_rect(Rect::new(x - 1.0, l.grid.y, 3.0, l.grid.h), ui.t.s(0x38373b));
        ui.c.fill_rect(Rect::new(x, l.grid.y, 1.0, l.grid.h), ui.t.s(0x53585c));
    }
    ui.c.pop_clip();

    // Vertical scroll bar: arrow buttons at its ends; small scroll bars have none and run along the grid.
    ui.c.fill_rect(Rect::new(l.vscroll.x, l.ruler.y, l.vscroll.w, l.grid.bottom() - l.ruler.y), t.scrollbar);
    let track = if super::small_scrollbars() {
        Rect::new(l.vscroll.x + 1.0, l.grid.y, l.vscroll.w - 2.0, l.grid.h)
    } else {
        let up = Rect::new(l.vscroll.x, l.grid.y - 19.0, l.vscroll.w, 19.0);
        let down = Rect::new(l.vscroll.x, l.grid.bottom() - 19.0, l.vscroll.w, 19.0);
        arrow_button(ui, up, 2, Scroll::PianoY);
        arrow_button(ui, down, 3, Scroll::PianoY);
        Rect::new(l.vscroll.x + 1.0, up.bottom(), l.vscroll.w - 2.0, (down.y - up.bottom()).max(0.0))
    };
    super::scrollbar(ui, track, true, s.piano.scroll_y, l.grid.h, (TOP_KEY + 1) as f32 * g.row, Scroll::PianoY);

    velocity_panel(ui, s, &l, &g, notes, ch, &sel);
    if s.lane_menu {
        ui.scaled(super::popup_scale(ui.c.scale), |ui, k| lane_menu(ui, s, &l, k));
    }
    resize_zones(ui, s, Win::PianoRoll, outer);
}

/// Scroll arrow button (§1a): raised face with a 1 px dark rim and a chevron pointing `dir`
/// (0 left, 1 right, 2 up, 3 down); a click steps `target` that way.
fn arrow_button(ui: &mut Ui, b: Rect, dir: u8, target: Scroll) {
    use os_ui::kit::{self, Dir};
    let (d, step) = match dir {
        0 => (Dir::Left, -1),
        1 => (Dir::Right, 1),
        2 => (Dir::Up, -1),
        _ => (Dir::Down, 1),
    };
    let act = Action::ScrollStep(target, step);
    let hov = ui.hit(b, act.clone(), "Scroll");
    let st = kit::State::new(hov, ui.down(b, &act), false);
    kit::scroll_arrow_button(ui.c, ui.t, b, d, st);
}

/// The square button with a dot above the vertical scroll bar (§1a): not available yet, greyed out.
fn corner_button(ui: &mut Ui, b: Rect) {
    use os_ui::kit;
    ui.off(b, "Piano roll view options");
    kit::flat_button(ui.c, ui.t, b, kit::State::NORMAL.with_disabled(true));
    let (cx, cy) = ((b.x + b.w / 2.0).round(), (b.y + b.h / 2.0).round());
    ui.c.fill_rect(Rect::new(cx - 2.0, cy - 2.0, 3.0, 3.0), ui.t.s(0x000000));
    ui.veil(b, ui.t.scrollbar);
}

/// Toolbar icons in the title bar (§2): the editing tools and undo; the menus and helpers that are not available yet
/// are greyed out. `shift` moves the icons (the floating window draws them 4 px further left, §1a).
fn toolbar(ui: &mut Ui, s: &State, r: Rect, shift: f32) {
    let t = ui.t;
    let active = s.piano.tool;
    let ox = r.x - 209.0 + shift;
    let items: [(f32, Icon, &'static str, Option<Action>); 15] = [
        (222.0, Icon::ArrowRight, "Piano roll options", None),
        (242.0, Icon::MidiControl, "Piano roll tools: quantize, chop, glue, strum, arpeggiate, randomize...", Some(Action::PianoToolsMenu)),
        (263.0, Icon::Magnet, "", Some(Action::PianoSnapMenu)),
        (283.0, Icon::Note, "Note properties", None),
        (305.0, Icon::Blend, "Scale and chord helpers", None),
        (327.0, Icon::Undo, "Undo (Ctrl+Z)", Some(Action::PianoUndo)),
        (360.0, Icon::Pencil, "Draw (P)", Some(Action::PianoTool(Tool::Draw))),
        (380.0, Icon::Brush, "Paint (B): drag to paint notes along the row", Some(Action::PianoTool(Tool::Paint))),
        (402.0, Icon::Plus, "Paint and add", None),
        (422.0, Icon::Delete, "Delete (D): click or swipe to delete notes", Some(Action::PianoTool(Tool::Delete))),
        (442.0, Icon::Mute, "Mute (T): click or swipe to mute / unmute notes", Some(Action::PianoTool(Tool::Mute))),
        (463.0, Icon::Slice, "Slice (C): drag a line to cut notes", Some(Action::PianoTool(Tool::Slice))),
        (485.0, Icon::Select, "Select (E): drag a box to select notes, Ctrl adds", Some(Action::PianoTool(Tool::Select))),
        (507.0, Icon::Zoom, "Zoom (Z): drag a box to zoom, right-click zooms out / back", Some(Action::PianoTool(Tool::Zoom))),
        (527.0, Icon::Speaker, "Playback (Y): drag to hear the notes under the line", Some(Action::PianoTool(Tool::Playback))),
    ];
    for (x, icon, hint, action) in items {
        let b = Rect::new(ox + x - 9.0, r.y + 2.0, 18.0, 18.0);
        let Some(action) = action else {
            ui.off(b, hint);
            icons::draw(ui.c, icon, b, t.text_faint);
            continue;
        };
        let on = action == Action::PianoTool(active);
        // The magnet's hint names the current snap (spec: ui/piano-snap.md §8).
        let hint: std::borrow::Cow<'static, str> = if action == Action::PianoSnapMenu { crate::piano_snap::hint(s).into() } else { hint.into() };
        let hov = ui.hit(b, action, hint);
        let c = if super::lemon(ui) {
            // Lemon: the active tool is a lit rounded square with a dark glyph.
            let bg = b.outset(1.0);
            if on {
                lk::lit_in(ui.c, ui.t, bg, lk::r_btn(t), hov);
            } else if hov {
                ui.c.fill_rrect(bg, lk::r_btn(t), t.button_hi);
            }
            if on { lk::ink_on_lit(ui.t) } else { t.glyph }
        } else if on {
            t.pill
        } else if hov {
            ui.t.tx(0xffffff)
        } else {
            t.glyph
        };
        icons::draw(ui.c, icon, b, c);
    }
    icons::draw(ui.c, Icon::Speaker, Rect::new(ox + 547.0, r.y + 4.0, 15.0, 15.0), t.text);
}

fn overview(ui: &mut Ui, s: &State, l: &Layout, g: &Geom, notes: &[Note], ch: Option<u16>) {
    let t = ui.t;
    let o = l.overview;
    // Horizontal scroll bar over the pattern plus room to scroll on (half a screen past the
    // pattern or the current view).
    let view = l.grid.w / g.ppt;
    let len = piano::pattern_length(s, s.pattern) as f32;
    let span = len.max(g.scroll_x + view) + view / 2.0;
    // Scroll arrows at both ends (§1a); the bar runs between them (small scroll bars: no arrows).
    let arrow = if super::small_scrollbars() { 0.0 } else { 19.0 };
    if arrow > 0.0 {
        arrow_button(ui, Rect::new(o.x, o.y, arrow, o.h), 0, Scroll::PianoX);
        arrow_button(ui, Rect::new(l.grid.right() - arrow, o.y, arrow, o.h), 1, Scroll::PianoX);
    }
    corner_button(ui, Rect::new(l.vscroll.x, o.y, l.vscroll.w, o.h));
    let track = Rect::new(o.x + arrow, o.y, (l.grid.right() - o.x - 2.0 * arrow).max(0.0), o.h);
    super::scrollbar(ui, track, false, g.scroll_x * g.ppt, l.grid.w, span * g.ppt, Scroll::PianoX);
    // Mini-map: one dash per note of the channel.
    let dash = t.note_top.mix(t.note_bottom, 0.5);
    for n in notes.iter().filter(|n| Some(n.channel) == ch) {
        let x = o.x + n.position as f32 / span * o.w;
        let w = (piano::shown_len(s, n) as f32 / span * o.w).max(1.0);
        let y = o.y + 3.0 + (1.0 - f32::from(n.key) / TOP_KEY as f32) * (o.h - 7.0);
        ui.c.fill_rect(Rect::new(x, y.round(), w, 1.0), dash);
    }
}

fn ruler(ui: &mut Ui, l: &Layout, g: &Geom, range: Option<(f32, f32)>, marker: u32, markers: &[(u32, String)]) {
    let t = ui.t;
    ui.hit(Rect::new(l.grid.x, l.ruler.y, l.grid.w, l.ruler.h), Action::PianoRuler, "Drag: move the play start - right-drag: select a time range");
    // The content keeps to the bottom 19 px; small scroll bars make the ruler taller, the extra band plain.
    ui.c.fill_rect(Rect::new(l.ruler.x, l.ruler.y, l.ruler.w, l.ruler.h - 19.0), t.ruler);
    let rr = Rect::new(l.ruler.x, l.ruler.bottom() - 19.0, l.ruler.w, 19.0);
    ui.c.fill_rect(Rect::new(rr.x, rr.y, rr.w, 17.0), t.ruler);
    ui.c.fill_rect(Rect::new(rr.x, rr.y + 17.0, rr.w, 1.0), ui.t.s(0x16191c));
    ui.c.fill_rect(Rect::new(rr.x, rr.y + 18.0, rr.w, 1.0), ui.t.s(0x090c0f));
    ui.c.push_clip(Rect::new(l.grid.x, rr.y, l.grid.w, rr.h));
    // Time selection: red band (§10, colours sampled from FL).
    if let Some((x0, x1)) = range {
        let b = Rect::span(x0, x1, rr.y + 1.0, rr.y + 19.0);
        ui.c.fill_rect_gradient(b, Color::rgb(0x9c5353), Color::rgb(0x944a4b), 0.0);
        ui.c.fill_rect(Rect::new(b.x, b.y, b.w, 1.0), Color::rgb(0xbf7576));
        ui.c.fill_rect(Rect::new(b.x, b.bottom() - 2.0, b.w, 1.0), Color::rgb(0x9f5556));
        ui.c.fill_rect(Rect::new(b.x, b.bottom() - 1.0, b.w, 1.0), Color::rgb(0xb06667));
    }
    // Bar numbers on the pattern's own bars (its time-signature markers, spec: ui/pattern-time-signature.md §2).
    let bar_px = g.ppt * g.bar as f32;
    let step = if bar_px < 30.0 { 4 } else if bar_px < 60.0 { 2 } else { 1 };
    for (tick, n) in g.bar_lines(g.scroll_x + l.grid.w / g.ppt) {
        let x = g.x_of(tick as f32);
        if (n - 1) % step != 0 || x < l.grid.x - 40.0 {
            continue;
        }
        ui.c.text(Font::new(13), x + 2.0, rr.y + 1.0, t.text_dim, &format!("{n}"));
    }
    // Time markers (§10a): a flag line and the name, drawn like the playlist's marker tabs.
    for (tick, name) in markers {
        let (tick, name) = (*tick, name.as_str());
        let x = g.x_of(tick as f32).round();
        if x < l.grid.x - 200.0 || x > l.grid.right() {
            continue;
        }
        let w = ui.c.text_width(Font::new(12), name) + 8.0;
        let tab = Rect::new(x, rr.y + 1.0, w, 15.0);
        ui.c.fill_rect(tab, t.window_title);
        ui.c.fill_rect(Rect::new(x, rr.y + 1.0, 1.0, rr.h - 1.0), t.text);
        ui.c.text(Font::new(12), x + 4.0, rr.y + 2.0, t.text, name);
    }
    // Play-start marker: an orange downward triangle (§10, colours sampled from FL).
    let mx = g.x_of(marker as f32).round() + 0.5;
    let tri = [(mx - 7.5, rr.y + 2.0), (mx + 7.5, rr.y + 2.0), (mx, rr.y + 11.0)];
    ui.c.fill_polygon(&tri, ui.t.s(0x2f1818));
    let inner = [(mx - 5.5, rr.y + 3.0), (mx + 5.5, rr.y + 3.0), (mx, rr.y + 9.5)];
    ui.c.fill_polygon(&inner, ui.t.h(0xdd9561));
    ui.c.fill_rect(Rect::new(mx - 5.0, rr.y + 3.0, 10.0, 1.0), ui.t.h(0xffc08b));
    ui.c.pop_clip();
}

fn keyboard(ui: &mut Ui, l: &Layout, g: &Geom, lit: &[i32]) {
    let t = ui.t;
    let k = l.keys;
    ui.hit(k, Action::PianoKeys, "Keyboard");
    ui.c.push_clip(k);
    // White keys: a stepped horizontal gradient (§4), with a dark right edge. Lemon: flat keys.
    let lemon = super::lemon(ui);
    let bands = if lemon { 1 } else { 6 };
    for i in 0..bands {
        let x = k.x + (k.w - 1.0) * i as f32 / bands as f32;
        let w = (k.w - 1.0) / bands as f32 + 1.0;
        let c = if lemon { t.key_white } else { t.key_white_left.mix(t.key_white, i as f32 / (bands - 1) as f32) };
        ui.c.fill_rect(Rect::new(x, k.y, w, k.h), c);
    }
    ui.c.fill_rect(Rect::new(k.right() - 1.0, k.y, 1.0, k.h), ui.t.s(0x7e8289));
    let (lo, hi) = g.visible_keys();
    // Lit white keys: the row plus half of each neighbouring black row (the white-key shape).
    let (row, half) = (g.row, (g.row / 2.0).floor());
    for &key in lit.iter().filter(|&&key| !is_black(key) && (lo..=hi).contains(&key)) {
        let y = g.y_of(key);
        let top = if is_black(key + 1) { y - half } else { y };
        let bottom = if is_black(key - 1) { y + row - 1.0 + half } else { y + row - 1.0 };
        for i in 0..bands {
            let x = k.x + (k.w - 1.0) * i as f32 / bands as f32;
            let w = (k.w - 1.0) / bands as f32 + 1.0;
            let c = if lemon { t.key_lit } else { t.key_lit_left.mix(t.key_lit, i as f32 / (bands - 1) as f32) };
            ui.c.fill_rect(Rect::new(x, top, w, bottom - top), c);
        }
        ui.c.fill_rect(Rect::new(k.right() - 1.0, top, 1.0, bottom - top), t.key_lit_edge);
    }
    let sep = t.key_white_left.shade(-0.12);
    for key in lo..=hi {
        let y = g.y_of(key);
        match key.rem_euclid(12) {
            0 => {
                // C: darker key, extended half a row up on the right; octave label.
                if !lit.contains(&key) && lemon {
                    ui.c.fill_rect(Rect::new(k.x, y - half, k.w - 1.0, row - 1.0 + half), t.key_c);
                } else if !lit.contains(&key) {
                    let left = t.key_white_left.shade(-0.16);
                    ui.c.fill_rect_gradient(Rect::new(k.x, y, 45.0, row - 1.0), left, left, 0.0);
                    ui.c.fill_rect(Rect::new(k.x + 45.0, y - half, k.w - 46.0, row - 1.0 + half), t.key_c);
                }
                if row >= 10.0 {
                    ui.c.text_right(Font::new(12), k.right() - 4.0, y + 1.0 + (row - ROW) / 2.0, t.key_label, &key_name(key));
                }
                ui.c.fill_rect(Rect::new(k.x, y + row - 1.0, k.w - 1.0, 1.0), sep);
            }
            5 => ui.c.fill_rect(Rect::new(k.x, y + row - 1.0, k.w - 1.0, 1.0), sep),
            _ => {}
        }
    }
    for key in lo..=hi {
        if is_black(key) {
            let y = g.y_of(key);
            let (left, right) = if lit.contains(&key) { (t.key_black_lit_left, t.key_black_lit) } else { (t.key_black_left, t.key_black) };
            if lemon {
                // Rounded at the playing end; the left end runs under the clip.
                ui.c.fill_rrect(Rect::new(k.x - 6.0, y, 51.0, row), 3.0f32.min(row / 2.0), right);
                continue;
            }
            for i in 0..6 {
                let c = left.mix(right, i as f32 / 5.0);
                let x = k.x + if i == 0 { 0.0 } else { 10.0 + 8.0 * (i - 1) as f32 };
                let w = if i == 0 { 10.0 } else if i == 5 { 3.0 } else { 8.0 };
                ui.c.fill_rect(Rect::new(x, y, w, row), c);
            }
        }
    }
    ui.c.pop_clip();
    // Divider between keyboard and grid.
    ui.c.fill_rect(Rect::new(k.right(), l.grid.y, 1.0, k.h), ui.t.s(0x131718));
    ui.c.fill_rect(Rect::new(k.right() + 1.0, l.grid.y, 1.0, k.h), ui.t.s(0x000000));
}

/// Vertical step / beat / bar lines over `area`, in `line(fill)` colors per horizontal band.
fn columns(ui: &mut Ui, g: &Geom, area: Rect, bands: &[(f32, f32, Color)], bar: Color) {
    let step = g.step as f32 * g.ppt;
    let show_steps = step >= 5.0;
    let beat_px = g.beat as f32 * g.ppt;
    let first = (g.scroll_x / g.step as f32).floor() as i64;
    let last = ((g.scroll_x + area.w / g.ppt) / g.step as f32).ceil() as i64;
    for i in first.max(0)..=last {
        let tick = (i * i64::from(g.step)) as u32;
        let x = g.x_of(tick as f32).floor();
        if x < area.x || x >= area.right() {
            continue;
        }
        // The pattern's own bars and beats (spec: ui/pattern-time-signature.md §2: 3/8, a bar every 6 steps).
        let (on_bar, on_beat) = g.line_at(tick);
        if on_bar {
            ui.c.fill_rect(Rect::new(x, area.y, 1.0, area.h), bar);
        } else if on_beat && beat_px >= 4.0 {
            for &(y, h, c) in bands {
                ui.c.fill_rect(Rect::new(x, y, 1.0, h), sub(ui.t, c, 0x12));
            }
        } else if show_steps {
            for &(y, h, c) in bands {
                ui.c.fill_rect(Rect::new(x, y, 1.0, h), sub(ui.t, c, 0x0a));
            }
        }
    }
}

fn grid(ui: &mut Ui, l: &Layout, g: &Geom, range: Option<(f32, f32)>) {
    grid_rows(ui, l, g, l.grid, false);
    if let Some((x0, x1)) = range {
        let r = Rect::span(x0, x1, l.grid.y, l.grid.bottom()).intersect(&l.grid);
        if r.w > 0.0 {
            grid_rows(ui, l, g, r, true);
        }
    }
}

/// The note-grid rows and columns, clipped to `clip` (`selecting` = the red time-selection tint).
fn grid_rows(ui: &mut Ui, l: &Layout, g: &Geom, clip: Rect, selecting: bool) {
    let t = ui.t;
    let gr = l.grid;
    let (white, black) = if selecting { (t.pr_row_sel, t.pr_row_black_sel) } else { (t.pr_row, t.pr_row_black) };
    ui.c.push_clip(clip);
    let (lo, hi) = g.visible_keys();
    let mut bands = Vec::with_capacity((hi - lo + 1).max(0) as usize);
    for key in lo..=hi {
        let y = g.y_of(key);
        let fill = if is_black(key) { black } else { white };
        ui.c.fill_rect(Rect::new(gr.x, y, gr.w, g.row - 1.0), fill);
        ui.c.fill_rect(Rect::new(gr.x, y + g.row - 1.0, gr.w, 1.0), sub(t, white, 0x0a));
        bands.push((y, g.row - 1.0, fill));
    }
    let bar = if selecting { ui.t.s(0x0e080c) } else { bar_line(t) };
    columns(ui, g, gr, &bands, bar);
    ui.c.pop_clip();
}

fn velocity_panel(ui: &mut Ui, s: &State, l: &Layout, g: &Geom, notes: &[Note], ch: Option<u16>, sel: &[usize]) {
    let t = ui.t;
    // Splitter with a grip (§1): drag to share the height, double-click to hide / show the event panel (spec:
    // ui/event-panel-splitter.md). The grip stays on a collapsed band so that it can be found.
    ui.c.fill_rect(l.splitter, t.window_title);
    ui.hit(l.splitter, Action::PianoSplitter, "Event panel: drag to resize, double-click to hide or show");
    let gx = l.grid.x + l.grid.w / 2.0 - 8.0;
    for i in 0..5 {
        ui.c.fill_rect(Rect::new(gx + i as f32 * 3.0, l.splitter.y + 4.0, 1.0, 1.0), t.text_faint);
    }
    if l.panel.h < 1.0 {
        // Collapsed: only its (empty) place, which the app reads for the window's extent.
        ui.hit(l.panel, Action::PianoVelocity, "");
        return;
    }
    // Label column.
    ui.c.fill_rect(l.label, t.window_title);
    let btn = Rect::new(l.label.x, l.label.y, l.label.w, 22.0);
    let hov = ui.hit(btn, Action::PianoLaneMenu, "Event panel: choose which note property to edit");
    ui.c.text(Font::new(16), l.label.x + 12.0, l.label.y + 2.0, if hov || s.lane_menu { ui.t.tx(0xffffff) } else { t.text }, "Control");
    ui.c.fill_polygon(&[(l.label.x + 61.0, l.label.y + 9.0), (l.label.x + 65.0, l.label.y + 12.5), (l.label.x + 61.0, l.label.y + 16.0)], t.text_dim);
    ui.c.fill_rect(Rect::new(l.panel.x - 2.0, l.panel.y, 1.0, l.panel.h), ui.t.s(0x131718));
    ui.c.fill_rect(Rect::new(l.panel.x - 1.0, l.panel.y, 1.0, l.panel.h), ui.t.s(0x000205));

    // Panel grid: horizontal lines every 22 px, then the time columns (§7).
    let p = l.panel;
    ui.hit(p, Action::PianoVelocity, "Event panel: drag to set this property of notes");
    let lane = s.piano.lane;
    ui.c.push_clip(p);
    ui.c.fill_rect(p, t.pr_row);
    if let Some((a, b)) = s.piano.range {
        let r = Rect::span(g.x_of(a as f32), g.x_of(b as f32), p.y, p.bottom());
        ui.c.fill_rect(r, t.pr_row_sel);
    }
    let mut bands = Vec::new();
    let mut y = p.y;
    let mut i = 0;
    while y < p.bottom() {
        let h = 21.0f32.min(p.bottom() - y);
        bands.push((y, h, t.pr_row));
        if y + 21.0 < p.bottom() {
            ui.c.fill_rect(Rect::new(p.x, y + 21.0, p.w, 1.0), sub(t, t.pr_row, if i % 2 == 1 { 0x12 } else { 0x0a }));
        }
        y += 22.0;
        i += 1;
    }
    columns(ui, g, p, &bands, bar_line(t));
    ui.c.text(Font::new(16), p.x + 7.0, p.y + 2.0, t.text_faint.shade(-0.35), lane.name());

    if lane.is_channel() {
        control_bars(ui, s, g, p, lane, ch);
    }
    ui.c.pop_clip();
    if lane == Lane::ChanPitch {
        pitch_scroll(ui, s, l);
    }
    ui.c.push_clip(p);
    // Lollipops.
    for (idx, n) in notes.iter().enumerate().filter(|(_, n)| !lane.is_channel() && Some(n.channel) == ch) {
        let x = g.x_of(n.position as f32).floor();
        if x > p.right() + 4.0 {
            continue;
        }
        let end = g.x_of((n.position + piano::shown_len(s, n)) as f32);
        if end < p.x - 4.0 {
            continue;
        }
        let top = lane.y_of(p, lane.get(n)).round();
        // Bipolar lanes hang the stem from the centre line.
        let base = lane.center().map_or(p.bottom(), |c| lane.y_of(p, c).round());
        let (stem, dot) = if sel.contains(&idx) {
            (t.note_sel_top.mix(t.note_sel_bottom, 0.4), t.note_sel_rim)
        } else if n.muted() {
            (ui.t.s(0x80858a), ui.t.s(0x474a4d))
        } else {
            (t.velocity, t.velocity_dot)
        };
        ui.c.fill_rect(Rect::new(x, top.min(base), 2.0, (base - top).abs()), stem);
        ui.c.fill_rect(Rect::new(x + 4.0, top, (end - x - 4.0).max(0.0), 1.0), stem);
        ui.c.fill_circle(x + 1.0, top + 0.5, 3.5, stem);
        ui.c.fill_circle(x + 1.0, top + 0.5, 2.5, dot);
    }
    ui.c.pop_clip();
}

/// The pitch lane's own vertical scroll bar with arrow buttons, right of the panel (spec §7b): the window
/// is 482 cents tall over ±4800 cents and each arrow click moves it 455 cents. In the vertical scroll bar's column.
fn pitch_scroll(ui: &mut Ui, s: &State, l: &Layout) {
    let t = ui.t;
    let col = Rect::new(l.vscroll.x + 2.0, l.panel.y, l.vscroll.w - 3.0, l.panel.h);
    // Small scroll bars: no arrow buttons.
    let btn = if super::small_scrollbars() { 0.0 } else { 20.0_f32.min(col.h / 3.0) };
    let (up, down) = (Rect::new(col.x, col.y, col.w, btn), Rect::new(col.x, col.bottom() - btn, col.w, btn));
    let track = Rect::new(col.x, up.bottom(), col.w, (down.y - up.bottom()).max(1.0));
    // Content in cents: the top edge of the window is `4800 - pos`.
    let total = (2 * piano::PITCH_RANGE) as f32;
    let view = 482.0;
    let top_edge = (s.piano.pitch_center as f32 + view / 2.0).min(piano::PITCH_RANGE as f32);
    super::scrollbar(ui, track, true, piano::PITCH_RANGE as f32 - top_edge, view, total, Scroll::PitchY);
    let arrows = if btn > 0.0 { vec![(up, 1, "Scroll the lane up"), (down, -1, "Scroll the lane down")] } else { Vec::new() };
    for (r, dir, name) in arrows {
        let hov = ui.hit(r, Action::PitchStep(dir), name);
        ui.c.fill_rect(r, if hov { t.scrollbar.shade(0.2) } else { t.scrollbar.shade(0.1) });
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let d = if dir > 0 { -2.0 } else { 2.0 };
        // Light under High visibility (`os_ui::kit::scroll_arrow`).
        let col = os_ui::kit::scroll_arrow(t, os_ui::kit::State::hover(hov), crate::high_visibility::on()).unwrap_or(t.text_dim);
        ui.c.fill_polygon(&[(cx - 4.0, cy - d), (cx, cy + d), (cx + 4.0, cy - d)], col);
    }
}

/// Channel control lanes (spec: ui/piano-roll.md §7b): a step graph of the lane's points; each bar runs from its
/// point to the next (the last to the panel's end) and is filled from the lane's baseline to the value.
fn control_bars(ui: &mut Ui, s: &State, g: &Geom, p: Rect, lane: Lane, ch: Option<u16>) {
    let (Some(ch), Some(param)) = (ch, lane.param()) else { return };
    // Fills measured from FL; the edge of the volume bars was measured, the others are derived from their fill.
    let (fill, edge) = match lane {
        Lane::ChanVolume => (Color::rgb(0x6e4a4c), Color::rgb(0x895556)),
        Lane::ChanPan => (ui.t.a(0x57643f), ui.t.a(0x6f7d51)),
        _ => (Color::rgb(0x6b4669), Color::rgb(0x865c83)),
    };
    let kind = os_model::ControlPoint::new(0, ch, param, 0).kind;
    let points: Vec<_> = piano::controls(s).iter().filter(|c| c.kind == kind).collect();
    let center = s.piano.pitch_center;
    let base = lane.control_base(p, center).round();
    for (i, c) in points.iter().enumerate() {
        let x0 = g.x_of(c.position as f32).floor();
        let x1 = points.get(i + 1).map_or(p.right(), |n| g.x_of(n.position as f32).floor());
        if x1 < p.x || x0 > p.right() {
            continue;
        }
        let y = lane.control_y(p, c.value, center).round();
        let r = Rect::new(x0, y.min(base), (x1 - x0).max(1.0), (base - y).abs().max(1.0));
        ui.c.fill_rect(r, fill);
        ui.c.fill_rect(Rect::new(x0, r.y, 1.0, r.h), edge);
    }
}

/// The event panel's Control menu (spec: ui/piano-roll.md §7a): same look as the menu bar dropdowns. `k`: the canvas is
/// at the Pop-ups scaling, a window position `x` is `x / k` (see `Ui::scaled`).
fn lane_menu(ui: &mut Ui, s: &State, l: &Layout, k: f32) {
    let t = ui.t;
    const ROW: f32 = 19.0;
    const HEAD: f32 = 16.0;
    let (w0, w1) = (170.0, 124.0);
    let h = HEAD + 7.0 * ROW;
    let win_h = ui.c.size().1;
    let r = Rect::new((l.label.x + 9.0) / k, ((l.label.y + 24.0) / k).min(win_h - h - 8.0), w0 + w1 + 6.0, h + 4.0);
    ui.c.fill_rect(r, t.menu_pop);
    ui.c.stroke_rrect(r.inset(1.0), 0.0, 2.0, t.menu_pop_border);
    ui.hit(r, Action::Swallow, "");
    let body = r.inset(2.0);
    let font = Font::new(14);
    let head = |ui: &mut Ui, x: f32, w: f32, text: &str| {
        ui.c.fill_rect(Rect::new(x, body.y, w, HEAD), t.menu_pop_head);
        ui.c.text(Font::new(12), x + 4.0, body.y + 1.0, t.menu_pop_head_text, text);
    };
    head(ui, body.x, w0, "Note properties");
    head(ui, body.x + w0 + 2.0, w1, "Channel controls");
    ui.c.fill_rect(Rect::new(body.x + w0, body.y, 2.0, body.h), t.menu_pop_border);
    for (i, lane) in Lane::ALL.iter().enumerate() {
        // Note properties fill the left column, channel controls the right one.
        let rr = if i < Lane::NOTE_LANES {
            Rect::new(body.x, body.y + HEAD + i as f32 * ROW, w0, ROW)
        } else {
            Rect::new(body.x + w0 + 2.0, body.y + HEAD + (i - Lane::NOTE_LANES) as f32 * ROW, w1, ROW)
        };
        if ui.hit(rr, Action::PianoLanePick(i), lane.menu_name()) || *lane == s.piano.lane {
            ui.c.fill_rect(rr, t.menu_pop_hi);
        }
        ui.c.text(font, rr.x + 20.0, rr.y + 2.0, t.menu_pop_text, lane.menu_name());
    }
}
