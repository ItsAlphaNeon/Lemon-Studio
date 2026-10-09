//! Piano roll window: toolbar, keyboard, note grid, ruler, overview and the velocity panel.
// spec: ui/piano-roll.md

use os_model::Note;
use os_ui::{Color, Font, Rect};

use super::{resize_zones, window_body, FrameInfo};
use crate::app::{Action, Scroll, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::piano::{self, Geom, Overlay, Tool};

/// Grid row pitch: 17 px of fill plus a 1 px separator (§3).
pub const ROW: f32 = 18.0;
/// Highest key shown (B10); the lowest is 0 (C0) (§3).
pub const TOP_KEY: i32 = 131;

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Key name in FL's naming (key 60 = C5) (§3).
pub fn key_name(key: i32) -> String {
    format!("{}{}", NAMES[key.rem_euclid(12) as usize], key.div_euclid(12))
}

pub fn is_black(key: i32) -> bool {
    matches!(key.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

/// `c` minus `d` on every channel (the grid's line colors are the row fill minus a constant, §3).
fn sub(c: Color, d: u8) -> Color {
    Color { r: c.r.saturating_sub(d), g: c.g.saturating_sub(d), b: c.b.saturating_sub(d), a: c.a }
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

pub fn layout(r: Rect) -> Layout {
    let bottom = r.bottom();
    let gx = r.x + 73.0;
    let gr = r.right() - 23.0;
    let gy = r.y + 64.0;
    let split = bottom - 185.0;
    Layout {
        tab: Rect::new(r.x, r.y + 23.0, 71.0, 41.0),
        overview: Rect::new(r.x + 72.0, r.y + 23.0, gr - r.x - 72.0 + 2.0, 22.0),
        ruler: Rect::new(r.x + 72.0, r.y + 45.0, gr - r.x - 72.0 + 2.0, 19.0),
        keys: Rect::new(r.x, gy, 71.0, split - gy),
        grid: Rect::new(gx, gy, gr - gx, split - gy),
        vscroll: Rect::new(gr + 1.0, r.y + 23.0, r.right() - gr - 1.0, split - r.y - 23.0),
        splitter: Rect::new(r.x, split, r.w, 8.0),
        label: Rect::new(r.x, split + 8.0, 71.0, bottom - split - 8.0),
        panel: Rect::new(gx, split + 8.0, gr - gx, bottom - split - 8.0),
    }
}

pub fn paint(ui: &mut Ui, s: &State, r: Rect, f: &FrameInfo) {
    let t = ui.t;
    let channel = s.project.channels.get(s.selected_channel);
    let name = channel.and_then(|c| c.name.clone()).unwrap_or_else(|| "(none)".into());
    let title = format!("Piano roll - {name}  \u{203a}");
    window_body(ui, r);
    let outer = r;
    let floating = !s.wins[Win::PianoRoll.slot().unwrap_or(0)].maxed && r.h >= 2.0 * crate::app::TITLE_H;
    let (r, _) = super::framed(ui, s, Win::PianoRoll, r, &title, 365.0, true);
    let shift = if floating { -4.0 } else { 0.0 };
    toolbar(ui, r, shift, s.piano.tool);
    if r.h < 2.0 * crate::app::TITLE_H {
        return; // collapsed to the title bar
    }
    let l = layout(r);
    let g = Geom::new(s, l.grid);
    let ch = channel.map(|c| c.index);
    let pat = s.project.patterns.iter().find(|p| p.number == s.pattern);
    let notes: &[Note] = pat.map_or(&[], |p| &p.notes);
    let sel = s.piano.selection(s.pattern, ch);
    let selecting = !sel.is_empty();

    // Note-colour tab (§6).
    let tab = l.tab;
    ui.c.fill_rect(tab, t.window_title);
    let green = t.note_top.mix(t.note_bottom, 0.6);
    ui.c.fill_polygon(&[(tab.x, tab.bottom()), (tab.x + 31.0, tab.y), (tab.right(), tab.y), (tab.right(), tab.bottom())], green);
    ui.c.line(tab.x + 0.5, tab.bottom() - 0.5, tab.x + 31.0, tab.y + 0.5, 1.0, t.note_top.shade(0.15));
    let ink = green.shade(-0.3);
    ui.c.polyline(&[(tab.right() - 15.0, tab.y + 16.0), (tab.right() - 10.0, tab.y + 16.0), (tab.right() - 7.0, tab.y + 7.0), (tab.right() - 3.0, tab.y + 7.0)], 1.6, ink);
    ui.c.fill_polygon(&[(tab.right() - 4.0, tab.bottom() - 18.0), (tab.right() - 4.0, tab.bottom() - 5.0), (tab.right() - 17.0, tab.bottom() - 5.0)], ink);

    overview(ui, s, &l, &g, notes, ch);
    ruler(ui, &l, &g);
    // Lit keys: the one held with the mouse and this channel's notes playing now (§4).
    let mut lit: Vec<i32> = s.piano.held.into_iter().collect();
    lit.extend(f.sounding.iter().filter(|&&(c, _)| Some(c) == ch).map(|&(_, k)| i32::from(k)));
    keyboard(ui, &l, &g, &lit);
    grid(ui, &l, &g, selecting);

    // Notes: ghosts of the other channels first, then this channel's (§5).
    ui.hit(l.grid, Action::PianoGrid, "Piano roll: click to add a note, drag a note to move it or its end to resize, right-click to delete");
    ui.c.push_clip(l.grid);
    let ghost = t.text_faint.with_alpha(110);
    for n in notes.iter().filter(|n| Some(n.channel) != ch) {
        if let Some(nr) = g.note_rect(n).filter(|nr| nr.intersect(&l.grid).w > 0.0) {
            ui.c.stroke_rrect(nr.inset(1.5), 2.0, 1.0, ghost);
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
            (Color::rgb(0x6d7276), Color::rgb(0x63686c), Color::rgb(0x474a4d), Color::rgb(0xc4c8cc))
        } else {
            (t.note_top, t.note_bottom, t.note_rim, t.note_text)
        };
        ui.c.fill_rrect(nr, 2.0, rim);
        let body = Rect::new(nr.x + 1.0, nr.y + 1.0, (nr.w - 2.0).max(1.0), nr.h - 2.0);
        ui.c.fill_rect_gradient(body, top, bottom, 1.0);
        if nr.w > 6.0 {
            let hi = top.mix(bottom, 0.45);
            ui.c.fill_rect(Rect::new(body.x, body.y + 1.0, 1.0, body.h - 2.0), hi);
            ui.c.fill_rect(Rect::new(body.right() - 1.0, body.y + 1.0, 1.0, body.h - 2.0), hi);
        }
        if nr.w > 12.0 {
            let label = key_name(i32::from(n.key));
            let w = ui.c.text_width(label_font, &label);
            let shown = if w <= nr.w - 5.0 { label } else { format!("{}..", &label[..1]) };
            ui.c.text_clip(label_font, nr.x + 3.0, nr.y + 2.0, nr.w - 4.0, text, &shown);
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
            ui.c.fill_rect(Rect::new(x, l.grid.y, 1.0, l.grid.h), Color::rgb(0xffffff).with_alpha(200));
        }
        None => {}
    }
    // Playhead (pattern mode).
    if f.playing && !s.song_mode {
        let len = piano::pattern_length(s, notes);
        let ppq = f64::from(s.project.ppq.max(1));
        let tick = (f.seconds * s.tempo / 60.0 * ppq) % f64::from(len.max(1));
        let x = g.x_of(tick as f32);
        ui.c.fill_rect(Rect::new(x, l.grid.y, 1.5, l.grid.h), t.playhead);
    }
    ui.c.pop_clip();

    // Vertical scroll bar.
    ui.c.fill_rect(Rect::new(l.vscroll.x, l.ruler.y, l.vscroll.w, l.grid.bottom() - l.ruler.y), t.scrollbar);
    let up = Rect::new(l.vscroll.x, l.ruler.y, l.vscroll.w, 19.0);
    let down = Rect::new(l.vscroll.x, l.grid.bottom() - 19.0, l.vscroll.w, 19.0);
    arrow_button(ui, up, 2);
    arrow_button(ui, down, 3);
    let track = Rect::new(l.vscroll.x + 1.0, up.bottom(), l.vscroll.w - 2.0, (down.y - up.bottom()).max(0.0));
    super::scrollbar(ui, track, true, s.piano.scroll_y, l.grid.h, (TOP_KEY + 1) as f32 * ROW, Scroll::PianoY);

    velocity_panel(ui, s, &l, &g, notes, ch, &sel);
    resize_zones(ui, s, Win::PianoRoll, outer);
}

/// Scroll arrow button (§1a): raised face with a 1 px dark rim and a chevron pointing `dir`
/// (0 left, 1 right, 2 up, 3 down; 4 = a dot).
fn arrow_button(ui: &mut Ui, b: Rect, dir: u8) {
    let t = ui.t;
    ui.c.fill_rect(b, Color::rgb(0x131618));
    ui.c.fill_rect_gradient(b.inset(1.0), Color::rgb(0x36383a), Color::rgb(0x2c2e30), 0.0);
    let (cx, cy) = ((b.x + b.w / 2.0).round(), (b.y + b.h / 2.0).round());
    let c = t.glyph.with_alpha(150);
    let pts = match dir {
        0 => [(cx + 2.0, cy - 5.0), (cx - 3.0, cy), (cx + 2.0, cy + 5.0)],
        1 => [(cx - 2.0, cy - 5.0), (cx + 3.0, cy), (cx - 2.0, cy + 5.0)],
        2 => [(cx - 5.0, cy + 2.0), (cx, cy - 3.0), (cx + 5.0, cy + 2.0)],
        3 => [(cx - 5.0, cy - 2.0), (cx, cy + 3.0), (cx + 5.0, cy - 2.0)],
        _ => {
            ui.c.fill_rect(Rect::new(cx - 2.0, cy - 2.0, 3.0, 3.0), Color::rgb(0x000000));
            return;
        }
    };
    ui.c.polyline(&pts, 1.2, c);
}

/// Toolbar icons in the title bar (§2). Only the draw tool exists so far; it is always active.
/// `shift` moves the icons (the floating window draws them 4 px further left, §1a).
fn toolbar(ui: &mut Ui, r: Rect, shift: f32, active: Tool) {
    let t = ui.t;
    let ox = r.x - 209.0 + shift;
    let items: [(f32, Icon, &'static str, Option<Tool>); 15] = [
        (222.0, Icon::ArrowRight, "Piano roll options", None),
        (242.0, Icon::MidiControl, "Tools", None),
        (263.0, Icon::Magnet, "Snap to grid: step", None),
        (283.0, Icon::Note, "Note properties", None),
        (305.0, Icon::Blend, "Scale and chord helpers", None),
        (327.0, Icon::Undo, "Undo (Ctrl+Z)", None),
        (360.0, Icon::Pencil, "Draw (P)", Some(Tool::Draw)),
        (380.0, Icon::Brush, "Paint (B): drag to paint notes along the row", Some(Tool::Paint)),
        (402.0, Icon::Plus, "Paint and add", None),
        (422.0, Icon::Delete, "Delete (D): click or swipe to delete notes", Some(Tool::Delete)),
        (442.0, Icon::Mute, "Mute (T): click or swipe to mute / unmute notes", Some(Tool::Mute)),
        (463.0, Icon::Slice, "Slice (C): drag a line to cut notes", Some(Tool::Slice)),
        (485.0, Icon::Select, "Select (E): drag a box to select notes, Ctrl adds", Some(Tool::Select)),
        (507.0, Icon::Zoom, "Zoom (Z): drag a box to zoom, right-click zooms out / back", Some(Tool::Zoom)),
        (527.0, Icon::Speaker, "Playback (Y): drag to hear the notes under the line", Some(Tool::Playback)),
    ];
    for (x, icon, hint, tool) in items {
        let b = Rect::new(ox + x - 9.0, r.y + 2.0, 18.0, 18.0);
        let on = tool.is_some() && tool == Some(active);
        let action = match tool {
            Some(t) => Action::PianoTool(t),
            None if icon == Icon::Undo => Action::PianoUndo,
            None => Action::Placeholder("piano roll tool"),
        };
        let hov = ui.hit(b, action, hint);
        let c = if on { t.pill } else if hov { Color::rgb(0xffffff) } else { t.glyph };
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
    let len = piano::pattern_length(s, notes) as f32;
    let span = len.max(g.scroll_x + view) + view / 2.0;
    // Scroll arrows at both ends (§1a); the bar runs between them.
    arrow_button(ui, Rect::new(o.x, o.y, 19.0, o.h), 0);
    arrow_button(ui, Rect::new(l.grid.right() - 19.0, o.y, 19.0, o.h), 1);
    arrow_button(ui, Rect::new(l.vscroll.x, o.y, l.vscroll.w, o.h), 4);
    let track = Rect::new(o.x + 19.0, o.y, (l.grid.right() - o.x - 38.0).max(0.0), o.h);
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

fn ruler(ui: &mut Ui, l: &Layout, g: &Geom) {
    let t = ui.t;
    let rr = l.ruler;
    ui.c.fill_rect(Rect::new(rr.x, rr.y, rr.w, 17.0), t.ruler);
    ui.c.fill_rect(Rect::new(rr.x, rr.y + 17.0, rr.w, 1.0), Color::rgb(0x16191c));
    ui.c.fill_rect(Rect::new(rr.x, rr.y + 18.0, rr.w, 1.0), Color::rgb(0x090c0f));
    ui.c.push_clip(Rect::new(l.grid.x, rr.y, l.grid.w, rr.h));
    let bar_px = g.ppt * g.bar as f32;
    let step = if bar_px < 30.0 { 4 } else if bar_px < 60.0 { 2 } else { 1 };
    let first = (g.scroll_x / g.bar as f32).floor() as i64;
    let last = ((g.scroll_x + l.grid.w / g.ppt) / g.bar as f32).ceil() as i64;
    for b in first..=last {
        if b % step != 0 {
            continue;
        }
        let x = g.x_of((b * i64::from(g.bar)) as f32);
        ui.c.text(Font::new(13), x + 2.0, rr.y + 1.0, t.text_dim, &format!("{}", b + 1));
    }
    ui.c.pop_clip();
}

fn keyboard(ui: &mut Ui, l: &Layout, g: &Geom, lit: &[i32]) {
    let t = ui.t;
    let k = l.keys;
    ui.hit(k, Action::PianoKeys, "Keyboard");
    ui.c.push_clip(k);
    // White keys: a stepped horizontal gradient (§4), with a dark right edge.
    let bands = 6;
    for i in 0..bands {
        let x = k.x + (k.w - 1.0) * i as f32 / bands as f32;
        let w = (k.w - 1.0) / bands as f32 + 1.0;
        let c = t.key_white_left.mix(t.key_white, i as f32 / (bands - 1) as f32);
        ui.c.fill_rect(Rect::new(x, k.y, w, k.h), c);
    }
    ui.c.fill_rect(Rect::new(k.right() - 1.0, k.y, 1.0, k.h), Color::rgb(0x7e8289));
    let (lo, hi) = g.visible_keys();
    // Lit white keys: the row plus half of each neighbouring black row (the white-key shape).
    for &key in lit.iter().filter(|&&key| !is_black(key) && (lo..=hi).contains(&key)) {
        let y = g.y_of(key);
        let top = if is_black(key + 1) { y - 9.0 } else { y };
        let bottom = if is_black(key - 1) { y + 26.0 } else { y + 17.0 };
        for i in 0..bands {
            let x = k.x + (k.w - 1.0) * i as f32 / bands as f32;
            let w = (k.w - 1.0) / bands as f32 + 1.0;
            let c = t.key_lit_left.mix(t.key_lit, i as f32 / (bands - 1) as f32);
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
                if !lit.contains(&key) {
                    let left = t.key_white_left.shade(-0.16);
                    ui.c.fill_rect_gradient(Rect::new(k.x, y, 45.0, 17.0), left, left, 0.0);
                    ui.c.fill_rect(Rect::new(k.x + 45.0, y - 9.0, k.w - 46.0, 26.0), t.key_c);
                }
                ui.c.text_right(Font::new(12), k.right() - 4.0, y + 1.0, t.key_label, &key_name(key));
                ui.c.fill_rect(Rect::new(k.x, y + 17.0, k.w - 1.0, 1.0), sep);
            }
            5 => ui.c.fill_rect(Rect::new(k.x, y + 17.0, k.w - 1.0, 1.0), sep),
            _ => {}
        }
    }
    for key in lo..=hi {
        if is_black(key) {
            let y = g.y_of(key);
            let (left, right) = if lit.contains(&key) { (t.key_black_lit_left, t.key_black_lit) } else { (t.key_black_left, t.key_black) };
            for i in 0..6 {
                let c = left.mix(right, i as f32 / 5.0);
                let x = k.x + if i == 0 { 0.0 } else { 10.0 + 8.0 * (i - 1) as f32 };
                let w = if i == 0 { 10.0 } else if i == 5 { 3.0 } else { 8.0 };
                ui.c.fill_rect(Rect::new(x, y, w, 18.0), c);
            }
        }
    }
    ui.c.pop_clip();
    // Divider between keyboard and grid.
    ui.c.fill_rect(Rect::new(k.right(), l.grid.y, 1.0, k.h), Color::rgb(0x131718));
    ui.c.fill_rect(Rect::new(k.right() + 1.0, l.grid.y, 1.0, k.h), Color::rgb(0x000000));
}

/// Vertical step / beat / bar lines over `area`, in `line(fill)` colors per horizontal band.
fn columns(ui: &mut Ui, g: &Geom, area: Rect, bands: &[(f32, f32, Color)], bar: Color) {
    let step = g.step as f32 * g.ppt;
    let show_steps = step >= 5.0;
    let beat_px = g.beat as f32 * g.ppt;
    let first = (g.scroll_x / g.step as f32).floor() as i64;
    let last = ((g.scroll_x + area.w / g.ppt) / g.step as f32).ceil() as i64;
    let per_beat = i64::from((g.beat / g.step).max(1));
    let per_bar = i64::from((g.bar / g.step).max(1));
    for i in first..=last {
        let x = g.x_of((i * i64::from(g.step)) as f32).floor();
        if x < area.x || x >= area.right() {
            continue;
        }
        if i % per_bar == 0 {
            ui.c.fill_rect(Rect::new(x, area.y, 1.0, area.h), bar);
        } else if i % per_beat == 0 && beat_px >= 4.0 {
            for &(y, h, c) in bands {
                ui.c.fill_rect(Rect::new(x, y, 1.0, h), sub(c, 0x12));
            }
        } else if show_steps {
            for &(y, h, c) in bands {
                ui.c.fill_rect(Rect::new(x, y, 1.0, h), sub(c, 0x0a));
            }
        }
    }
}

fn grid(ui: &mut Ui, l: &Layout, g: &Geom, selecting: bool) {
    let t = ui.t;
    let gr = l.grid;
    let (white, black) = if selecting { (t.pr_row_sel, t.pr_row_black_sel) } else { (t.pr_row, t.pr_row_black) };
    ui.c.push_clip(gr);
    let (lo, hi) = g.visible_keys();
    let mut bands = Vec::with_capacity((hi - lo + 1).max(0) as usize);
    for key in lo..=hi {
        let y = g.y_of(key);
        let fill = if is_black(key) { black } else { white };
        ui.c.fill_rect(Rect::new(gr.x, y, gr.w, 17.0), fill);
        ui.c.fill_rect(Rect::new(gr.x, y + 17.0, gr.w, 1.0), sub(white, 0x0a));
        bands.push((y, 17.0, fill));
    }
    let bar = if selecting { Color::rgb(0x0e080c) } else { t.pr_bar };
    columns(ui, g, gr, &bands, bar);
    ui.c.pop_clip();
}

fn velocity_panel(ui: &mut Ui, s: &State, l: &Layout, g: &Geom, notes: &[Note], ch: Option<u16>, sel: &[usize]) {
    let t = ui.t;
    // Splitter with a grip (§1).
    ui.c.fill_rect(l.splitter, t.window_title);
    let gx = l.grid.x + l.grid.w / 2.0 - 8.0;
    for i in 0..5 {
        ui.c.fill_rect(Rect::new(gx + i as f32 * 3.0, l.splitter.y + 4.0, 1.0, 1.0), t.text_dim);
    }
    // Label column.
    ui.c.fill_rect(l.label, t.window_title);
    let lw = ui.c.text(Font::new(16), l.label.x + 12.0, l.label.y + 2.0, t.text, "Control");
    let _ = lw;
    ui.c.fill_polygon(&[(l.label.x + 61.0, l.label.y + 9.0), (l.label.x + 65.0, l.label.y + 12.5), (l.label.x + 61.0, l.label.y + 16.0)], t.text_dim);
    ui.c.fill_rect(Rect::new(l.panel.x - 2.0, l.panel.y, 1.0, l.panel.h), Color::rgb(0x131718));
    ui.c.fill_rect(Rect::new(l.panel.x - 1.0, l.panel.y, 1.0, l.panel.h), Color::rgb(0x000205));

    // Panel grid: horizontal lines every 22 px, then the time columns (§7).
    let p = l.panel;
    ui.hit(p, Action::PianoVelocity, "Velocity: drag to set the velocity of notes");
    ui.c.push_clip(p);
    ui.c.fill_rect(p, t.pr_row);
    let mut bands = Vec::new();
    let mut y = p.y;
    let mut i = 0;
    while y < p.bottom() {
        let h = 21.0f32.min(p.bottom() - y);
        bands.push((y, h, t.pr_row));
        if y + 21.0 < p.bottom() {
            ui.c.fill_rect(Rect::new(p.x, y + 21.0, p.w, 1.0), sub(t.pr_row, if i % 2 == 1 { 0x12 } else { 0x0a }));
        }
        y += 22.0;
        i += 1;
    }
    columns(ui, g, p, &bands, t.pr_bar);
    ui.c.text(Font::new(16), p.x + 7.0, p.y + 2.0, t.text_faint.shade(-0.35), "Velocity");

    // Lollipops.
    for (idx, n) in notes.iter().enumerate().filter(|(_, n)| Some(n.channel) == ch) {
        let x = g.x_of(n.position as f32).floor();
        if x > p.right() + 4.0 {
            continue;
        }
        let end = g.x_of((n.position + piano::shown_len(s, n)) as f32);
        if end < p.x - 4.0 {
            continue;
        }
        let top = piano::velocity_y(p, n.velocity).round();
        let (stem, dot) = if sel.contains(&idx) {
            (t.note_sel_top.mix(t.note_sel_bottom, 0.4), t.note_sel_rim)
        } else if n.muted() {
            (Color::rgb(0x80858a), Color::rgb(0x474a4d))
        } else {
            (t.velocity, t.velocity_dot)
        };
        ui.c.fill_rect(Rect::new(x, top, 2.0, p.bottom() - top), stem);
        ui.c.fill_rect(Rect::new(x + 4.0, top, (end - x - 4.0).max(0.0), 1.0), stem);
        ui.c.fill_circle(x + 1.0, top + 0.5, 3.5, stem);
        ui.c.fill_circle(x + 1.0, top + 0.5, 2.5, dot);
    }
    ui.c.pop_clip();
}
