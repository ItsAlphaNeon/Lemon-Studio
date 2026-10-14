// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Channel rack window: a floating, resizable child window.
// spec: ui/channel-rack.md

use os_ui::kit::lemon as lk;
use os_ui::{Color, Font, Rect};

use super::{rgb, FrameInfo};
use crate::app::{Action, Live, Scroll, State, Ui, Win};
use crate::icons::{self, Icon};
use crate::knobs::KnobId;
use crate::rack::{self as rk, FillHit, FillType, Tab};

pub const ROW_H: f32 = 30.0;
pub const STEP_W: f32 = 16.0;
/// Default outer size (§1): 16 steps, FL's 499 px plus Lemon Studio's wider name button (§17).
pub const DEFAULT_SIZE: (f32, f32) = (499.0 + rk::NAME_W - rk::FL_NAME_W, 246.0);
/// Smallest height: one row (§5).
pub const MIN_H: f32 = 96.0 + ROW_H;
/// Space above the first row and below the last one inside the rows panel.
const PAD: f32 = 18.0;
/// Graph editor panel height (§14), its tab row included.
pub const GRAPH_H: f32 = 240.0;
const GRAPH_TABS_H: f32 = 20.0;
/// Advanced fill dialog size (§22, Lemon Studio layout).
pub const FILL_W: f32 = 285.0;
pub const FILL_H: f32 = 446.0;

/// Resize edges (bit set) for `Action::RackResize`.
pub const EDGE_RIGHT: u8 = 1;
pub const EDGE_BOTTOM: u8 = 2;

/// Horizontal layout of a row (§3, §17): the name button width and the mixer column move everything right of them.
#[derive(Debug, Clone, Copy)]
pub struct Cols {
    /// Shift of the name button's left edge (the mixer column hidden moves it left).
    pub name_x: f32,
    pub name_w: f32,
    /// Shift of everything right of the name button against the default layout.
    pub dx: f32,
    pub slot_x: f32,
    pub step_x: f32,
    /// Extra width of loop mode (§15).
    pub loop_w: f32,
}

pub fn cols(s: &State) -> Cols {
    let mix_off = if s.rack.show_mixer { 0.0 } else { rk::MIX_W };
    let name_w = (rk::NAME_W * s.rack.name_pct as f32 / 100.0).round();
    let dx = name_w - rk::FL_NAME_W - mix_off;
    Cols { name_x: 108.0 - mix_off, name_w, dx, slot_x: 200.0 + dx, step_x: 217.0 + dx, loop_w: if s.rack.loop_mode { rk::LOOP_W } else { 0.0 } }
}

/// Width of a window showing one step (§5: W = 259 + 16·(n − 1) in the default layout).
pub fn base_w(s: &State) -> f32 {
    let c = cols(s);
    259.0 + c.dx + c.loop_w
}

/// Snap a dragged size so whole steps and rows appear (§5), at least 4 steps and one row.
pub fn snap_size(s: &State, w: f32, h: f32) -> (f32, f32) {
    let b = base_w(s);
    let w = b + STEP_W * ((w - b) / STEP_W).round();
    let h = 96.0 + ROW_H * ((h - 96.0) / ROW_H).round();
    (w.max(b + 3.0 * STEP_W), h.max(MIN_H))
}

/// Whether every note of channel `ch` in the pattern is a plain step (§6: otherwise the row shows
/// the mini piano roll even in step mode).
fn plain_steps(notes: &[os_model::Note], ch: u16) -> bool {
    notes.iter().filter(|n| n.channel == ch).all(|n| n.length == 0 && n.key == 60)
}

/// The window's outer rectangle.
pub fn rect(s: &State) -> Rect {
    Rect::new(s.rack_pos.0, s.rack_pos.1, s.rack_size.0, s.rack_size.1)
}

/// Where the window paints: its rect and the graph editor, which hangs below it (§14).
pub fn paint_area(s: &State) -> Rect {
    match graph_rect(s) {
        Some(g) => rect(s).union(&g.outset(2.0)),
        None => rect(s),
    }
}

/// Everything the window can paint or hit (incl. the resize zones past its edges and the graph editor, which hangs
/// below the window, §14).
pub fn bounds(s: &State) -> Rect {
    let r = rect(s).outset(4.0);
    match graph_rect(s) {
        Some(g) => r.union(&g.outset(2.0)),
        None => r,
    }
}

/// Inside of the rows panel (§1: border at x 4 and W−6, y 29 and H−30).
pub fn panel(win: Rect) -> Rect {
    Rect::span(win.x + 5.0, win.right() - 6.0, win.y + 30.0, win.bottom() - 30.0)
}

/// Steps that fit the window width (§3: 16 at the default width, 36 at 819 px).
pub fn visible_steps(s: &State) -> u32 {
    (((s.rack_size.0 - base_w(s)) / STEP_W).floor() as i32 + 1).max(1) as u32
}

/// A shown row: its channel, the content offset of its name button top, and whether it is zipped (§17).
#[derive(Debug, Clone, Copy)]
pub struct RowPos {
    pub row: usize,
    pub top: f32,
    pub zipped: bool,
}

impl RowPos {
    pub fn pitch(&self) -> f32 {
        if self.zipped { rk::ZIP_H } else { ROW_H }
    }
}

pub fn layout(s: &State) -> Vec<RowPos> {
    let mut y = PAD + 2.0;
    rk::shown(s)
        .into_iter()
        .map(|row| {
            let zipped = s.rack.zipped.contains(&row);
            let p = RowPos { row, top: y, zipped };
            y += p.pitch();
            p
        })
        .collect()
}

/// Height of the shown rows.
pub fn rows_h(s: &State) -> f32 {
    layout(s).iter().map(RowPos::pitch).sum()
}

/// Height of all rows plus the padding (content pixels for the vertical scroll bar).
pub fn content_h(s: &State) -> f32 {
    rows_h(s) + 2.0 * PAD
}

fn scroll(s: &State) -> f32 {
    let inner = panel(rect(s));
    s.rack_scroll.clamp(0.0, (content_h(s) - inner.h).max(0.0))
}

/// Screen y of a row's name button top.
fn row_y(s: &State, p: &RowPos) -> f32 {
    s.rack_pos.1 + 30.0 + p.top - scroll(s) - 2.0
}

/// The channel row at screen y `y` (inside the rows panel), if any.
pub fn row_at(s: &State, y: f32) -> Option<usize> {
    let inner = panel(rect(s));
    if y < inner.y || y >= inner.bottom() {
        return None;
    }
    layout(s).into_iter().find(|p| {
        let band = row_y(s, p) - (ROW_H - 22.0) / 2.0;
        y >= band && y < band + p.pitch()
    }).map(|p| p.row)
}

/// The name button of row `r` when it is shown.
pub fn name_rect(s: &State, r: usize) -> Option<Rect> {
    let p = layout(s).into_iter().find(|p| p.row == r)?;
    let c = cols(s);
    let ry = row_y(s, &p);
    Some(Rect::new(s.rack_pos.0 + c.name_x, ry, c.name_w, 26.0))
}

/// The step column under screen x `x` (any row), if inside the step area.
pub fn step_at(s: &State, x: f32) -> Option<u32> {
    let c = cols(s);
    let f = (x - (s.rack_pos.0 + c.step_x) + sx(s)) / STEP_W;
    (f >= 0.0).then_some(f as u32)
}

/// Horizontal step scroll in pixels.
fn sx(s: &State) -> f32 {
    let shown = visible_steps(s);
    let total = rk::pattern_steps(s).max(shown) + 256;
    s.rack_scroll_x.clamp(0.0, ((total - shown) as f32 * STEP_W).max(0.0))
}

/// Name width that fits the longest channel name (Options > Channel button width > Size to fit).
pub fn fit_name_w(s: &State) -> f32 {
    let longest = s.project.channels.iter().map(|c| c.name.as_deref().unwrap_or("Channel 00").chars().count()).max().unwrap_or(8);
    (longest as f32 * 7.5 + 14.0).max(40.0)
}

/// Graph editor panel (§14) under the focused row: steps area wide, `GRAPH_H` tall.
pub fn graph_rect(s: &State) -> Option<Rect> {
    if !s.rack.graph_open || !s.shown(Win::ChannelRack) {
        return None;
    }
    let p = layout(s).into_iter().find(|p| p.row == s.selected_channel && !p.zipped)?;
    let c = cols(s);
    let x = s.rack_pos.0 + c.step_x - 2.0;
    let w = visible_steps(s) as f32 * STEP_W + 3.0;
    Some(Rect::new(x, row_y(s, &p) + 27.0, w, GRAPH_H))
}

/// The columns of the steps playing at ticks `a` and `b` in pattern mode, when they differ: lit steps down the
/// column and the footer marker (§16) are all that a new step changes.
pub fn step_columns(s: &State, a: f64, b: f64) -> Vec<Rect> {
    let per_step = f64::from(rk::per_step(s).max(1));
    let len = rk::pattern_steps(s).max(1);
    let step = |t: f64| (t / per_step).floor().max(0.0) as u32 % len;
    let (ka, kb) = (step(a), step(b));
    if ka == kb {
        return Vec::new();
    }
    let win = rect(s);
    let inner = panel(win);
    let x0 = win.x + cols(s).step_x - sx(s);
    let bottom = win.bottom() - 22.0;
    [ka, kb].into_iter().map(|k| Rect::new(x0 + STEP_W * k as f32 - 2.0, inner.y, STEP_W + 1.0, bottom - inner.y)).collect()
}

/// The bars area of the graph editor (above its tab row).
pub fn graph_bars(s: &State) -> Option<Rect> {
    graph_rect(s).map(|g| Rect::new(g.x, g.y + 2.0, g.w, g.h - GRAPH_TABS_H - 4.0))
}

/// "808 A..omic": shorten in the middle until it fits (§13).
fn shorten(ui: &mut Ui, f: Font, text: &str, w: f32) -> String {
    if ui.c.text_width(f, text) <= w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 2 {
        keep -= 1;
        let head = keep.div_ceil(2);
        let tail = keep - head;
        let s: String = chars[..head].iter().chain([&'.', &'.']).chain(chars[chars.len() - tail..].iter()).collect();
        if ui.c.text_width(f, &s) <= w {
            return s;
        }
    }
    "..".into()
}

pub fn paint(ui: &mut Ui, s: &State, f: &FrameInfo) {
    let t = ui.t;
    let win = rect(s);
    let (x, y, w, h) = (win.x, win.y, win.w, win.h);
    let c = cols(s);
    let lemon = super::lemon(ui);
    // Frame (§1). Lemon (spec: ui/lemon-design.md §5): one rounded outlined panel.
    if lemon {
        lk::outlined(ui.c, win, lk::r_panel(t), t.window_border, t.window_body);
    } else {
        ui.c.fill_rect(win, t.window_border);
        let body = win.inset(1.0);
        ui.c.fill_rect(body, t.window_body);
        ui.c.fill_rect(Rect::new(body.x, body.y, body.w, 1.0), t.window_title_hi);
    }
    ui.hit(win, Action::RackArea, "Channel rack");
    let bar = Rect::span(x + 1.0, x + w - 1.0, y + 1.0, y + 29.0);
    ui.hit(bar, Action::WinDrag(Win::ChannelRack), "Channel rack - drag to move");
    title_bar(ui, s, win, c, f);

    // Rows panel (§1, §3).
    let inner = panel(win);
    if lemon {
        lk::outlined(ui.c, inner.outset(1.0), lk::r_btn(t), t.rack_inset_border, t.rack_inset);
    } else {
        ui.c.fill_rect(inner.outset(1.0), t.rack_inset_border);
        ui.c.fill_rect(inner, t.rack_inset);
    }
    let view_h = inner.h;
    let total_h = content_h(s);
    let vbar = total_h > view_h + 0.5;
    let rows_clip = if vbar { Rect::span(inner.x, inner.right() - 11.0, inner.y, inner.bottom()) } else { inner };
    let scroll = scroll(s);
    let steps_shown = visible_steps(s);
    let len = rk::pattern_steps(s);
    let sx = sx(s);
    let total_steps = len.max(steps_shown) + 256;
    let first_step = (sx / STEP_W).floor() as u32;
    let per_step = rk::per_step(s);
    let pat = s.project.pattern(s.pattern);
    let notes: &[os_model::Note] = pat.map_or(&[], |p| &p.notes);
    // Playback position (pattern mode): the current step (§16).
    let cur_step = (f.playing && !f.song).then(|| (f.tick / f64::from(per_step)).floor() as u32);
    let step_x0 = x + c.step_x;
    let steps_right = (step_x0 + steps_shown as f32 * STEP_W - 2.0).min(rows_clip.right());

    ui.c.push_clip(rows_clip);
    for p in layout(s) {
        let i = p.row;
        let ry = row_y(s, &p);
        if ry + p.pitch() < rows_clip.y - 4.0 || ry - 4.0 > rows_clip.bottom() {
            continue;
        }
        let ch = &s.project.channels[i];
        let focused = i == s.selected_channel;
        let selected = s.rack.sel.contains(&i);
        let nb = Rect::new(x + c.name_x, ry, c.name_w, 26.0);
        if p.zipped {
            // A zipped row: a 7 px bar in the name column (§17).
            let zr = Rect::new(nb.x, ry + 1.0, nb.w, 7.0);
            ui.hit(Rect::new(zr.x, zr.y - 2.0, zr.w, rk::ZIP_H), Action::ChannelSelect(i), "Zipped channel - Alt+U unzips all");
            let col = ch.color.map(rgb).unwrap_or(t.name_button);
            ui.c.fill_rrect(zr, 2.0, if focused { col.shade(0.2) } else { col });
            if selected {
                ui.c.fill_rect(Rect::new(x + c.slot_x - 1.0, zr.y, 8.0, zr.h), t.accent);
            }
            continue;
        }
        // Mute LED, or the padlock of a locked channel (§10).
        let lr = Rect::new(x + 11.5, ry + 6.5, 12.0, 12.0);
        ui.hit(lr, Action::ChannelEnable(i), "Mute / solo  (shift/ctrl+)0..9");
        if s.rack.locked.contains(&i) {
            padlock(ui, x + 17.5, ry + 12.5, ch.enabled);
        } else {
            rack_led(ui, x + 17.5, ry + 12.5, ch.enabled);
        }
        // Pan and volume knobs (§11).
        ui.hit(Rect::new(x + 27.0, ry + 5.0, 15.0, 16.0), Action::Knob(KnobId::ChanPan(i)), s.knob_text(KnobId::ChanPan(i)));
        row_knob(ui, x + 34.5, ry + 13.0, s.knob(KnobId::ChanPan(i)));
        ui.hit(Rect::new(x + 44.0, ry + 5.0, 15.0, 16.0), Action::Knob(KnobId::ChanVol(i)), s.knob_text(KnobId::ChanVol(i)));
        row_knob(ui, x + 51.5, ry + 13.0, s.knob(KnobId::ChanVol(i)));
        // Mixer track box (§12).
        if s.rack.show_mixer {
            let mb = Rect::span(x + 66.0, x + 99.0, ry + 1.0, ry + 25.0);
            let mhov = ui.hit(mb, Action::RackMixer(i), format!("Target mixer track: {}  (double-click)", insert_name(ch.mixer_track)));
            let face = match (s.rack.colorful, ch.color) {
                (true, Some(col)) => rgb(col).mix(t.rack_row_box, 0.5),
                _ => t.rack_row_box,
            };
            if lemon {
                lk::small_box(ui.c, t, mb, face, mhov);
            } else {
                ui.c.fill_rrect(mb, 3.0, t.rack_control_border);
                ui.c.fill_rect_gradient(mb.inset(1.0), face.shade(-0.04), face, 2.5);
            }
            let num_c = if mhov { t.glyph } else { t.text_dim.shade(0.08) };
            let label = if ch.mixer_track == 0 { "M".to_string() } else { ch.mixer_track.to_string() };
            let nf = super::light(19.0);
            let lh = ui.c.line_height(nf);
            ui.c.text_right(nf, x + 91.0, (mb.cy() - lh / 2.0).round(), num_c, &label);
            spin_mark(ui, x + 94.0, ry + 13.0, t.text_dim.shade(-0.1));
        }
        // Name button (§13): the channel colour fills the face.
        let nhov = ui.hit(nb, Action::ChannelSelect(i), "Click: plugin window, Shift+click: rename, right-click: channel menu");
        let base = ch.color.map_or(t.name_button, |col| rgb(col).mix(t.name_button, 0.35));
        let face = if focused { base.shade(0.18).mix(t.name_button_sel, 0.5) } else { base };
        let face = if nhov { face.shade(0.06) } else { face };
        let col = if lemon {
            lk::name_button(ui.c, t, nb, ch.color.map(rgb), focused, nhov)
        } else {
            ui.c.fill_rrect(nb, 3.0, t.rack_control_border.shade(0.08));
            ui.c.fill_rect(Rect::new(nb.x + 1.0, nb.y + 1.0, nb.w - 2.0, 1.0), face.shade(0.12));
            ui.c.fill_rect_gradient(Rect::new(nb.x + 1.0, nb.y + 2.0, nb.w - 2.0, nb.h - 4.0), face.shade(0.03), face.shade(-0.04), 1.5);
            let col = if ch.color.is_some() { t.name_text.shade(0.2) } else { t.name_text };
            if focused { col.shade(0.25) } else { col }
        };
        let label = ch.name.clone().unwrap_or_else(|| format!("Channel {}", ch.index + 1));
        let nf = Font::new(14);
        let label = shorten(ui, nf, &label, nb.w - 8.0);
        ui.c.push_clip(nb.inset(1.0));
        if s.rack.align_left {
            let lh = ui.c.line_height(nf);
            ui.c.text(nf, nb.x + if lemon { 9.0 } else { 5.0 }, (nb.cy() - lh / 2.0).round(), col, &label);
        } else {
            ui.c.text_center(nf, nb, col, &label);
        }
        ui.c.pop_clip();
        // Activity / select slot (§8, §16).
        let slot = Rect::span(x + c.slot_x, x + c.slot_x + 6.0, ry + 2.0, ry + 24.0);
        let sel_hit = Rect::span(x + c.slot_x - 4.0, x + c.slot_x + 10.0, ry, ry + 26.0);
        ui.hit(sel_hit, Action::RackSelect(i), "Select (Up/Down)");
        ui.live(Live::Sounding, slot);
        let sounding = f.playing && !f.paused && f.sounding.iter().any(|&(n, _)| n == ch.index) && ch.enabled;
        if lemon {
            let fill = if sounding { t.accent } else { ui.t.s(0x2b2d2f) };
            lk::outlined(ui.c, slot, 3.0, ui.t.s(0x1f2224), fill);
            if selected {
                let fr = Rect::span(x + c.slot_x - 1.5, x + c.slot_x + 7.5, ry - 0.5, ry + 26.5);
                ui.c.stroke_rrect(fr, 4.0, 1.5, t.accent);
            }
        } else {
            ui.c.fill_rect(slot, ui.t.s(0x1f2224));
            let slot_fill = if sounding { ui.t.tx(0xd8dcde) } else { ui.t.s(0x2b2d2f) };
            ui.c.fill_rect(slot.inset(1.0), slot_fill);
        }
        if selected && !lemon {
            let fr = Rect::span(x + c.slot_x - 1.0, x + c.slot_x + 7.0, ry, ry + 26.0);
            ui.c.fill_rect(Rect::new(fr.x, fr.y, fr.w, 2.0), t.accent);
            ui.c.fill_rect(Rect::new(fr.x, fr.bottom() - 2.0, fr.w, 2.0), t.accent);
            ui.c.fill_rect(Rect::new(fr.x, fr.y, 1.0, fr.h), t.accent);
            ui.c.fill_rect(Rect::new(fr.right() - 1.0, fr.y, 1.0, fr.h), t.accent);
        }
        // Steps (§3, §9), clipped to the step area.
        let step_area = Rect::span(step_x0 - 1.0, steps_right, rows_clip.y, rows_clip.bottom());
        if s.rack_roll_view || !plain_steps(notes, ch.index) {
            let block = Rect::span(step_x0 - 2.0, steps_right + 2.0, ry - 2.0, ry + 28.0);
            mini_roll(ui, block, notes, ch.index, per_step, sx, focused);
            ui.hit(block.intersect(&rows_clip), Action::RackRoll(i), "Piano roll preview - click to open the Piano roll");
        } else {
            let row_len = rk::row_len(s, i);
            ui.c.push_clip(step_area);
            for k in first_step..(first_step + steps_shown + 1) {
                let sr = Rect::new(step_x0 + STEP_W * k as f32 - sx, ry + 4.0, 13.0, 19.0);
                let on = rk::step_on(notes, ch.index, k, per_step);
                let hov = ui.hit(sr.intersect(&step_area), Action::Step(i, k), "Step - click or drag to paint (left / right)");
                step_button(ui, sr, k, on, k >= row_len, hov, on && cur_step.is_some_and(|c| c % len.max(1) == k));
            }
            ui.c.pop_clip();
        }
        // Loop length box (§15).
        if s.rack.loop_mode {
            let lb = Rect::new(steps_right + 10.0, ry + 2.0, 24.0, 22.0);
            let has = notes.iter().any(|n| n.channel == ch.index);
            let lhov = ui.hit(lb, Action::RackLoopBox(i), "Set loop point");
            if lemon {
                lk::small_box(ui.c, t, lb, t.rack_row_box, lhov);
            } else {
                ui.c.fill_rrect(lb, 3.0, t.rack_control_border);
                ui.c.fill_rect_gradient(lb.inset(1.0), t.rack_row_box.shade(-0.04), t.rack_row_box, 2.5);
            }
            let tc = if !has { t.text_dim.shade(-0.3) } else if lhov { t.glyph } else { t.text_dim.shade(0.08) };
            let txt = rk::row_len(s, i).to_string();
            ui.c.text_center(Font::new(13), lb, tc, &txt);
        }
    }
    ui.c.pop_clip();
    if vbar {
        let track = Rect::span(inner.right() - 10.0, inner.right(), inner.y, inner.bottom());
        super::scrollbar(ui, track, true, scroll, view_h, total_h, Scroll::RackY);
    }

    // Footer (§4, §16).
    add_button(ui, x + c.name_x, c.name_w, y + h);
    if let Some(k) = cur_step {
        let k = k % len.max(1);
        let px = step_x0 + STEP_W * k as f32 - sx;
        if px >= step_x0 - 1.0 && px + 13.0 <= steps_right + 2.0 {
            if lemon {
                ui.c.fill_rrect(Rect::new(px, y + h - 30.0, 13.0, 7.0), 2.5, t.accent);
            } else {
                ui.c.fill_rect(Rect::new(px, y + h - 30.0, 13.0, 7.0), ui.t.s(0x5a6064));
            }
        }
    }
    let track = Rect::span(x + c.slot_x + 8.0, x + w - 2.0, y + h - 13.0, y + h - 6.0);
    let view = steps_shown as f32 * STEP_W;
    super::scrollbar(ui, track, false, sx, view, total_steps as f32 * STEP_W, Scroll::RackX);
    // Redraw the bar in the rack's own look: flat track, thin dark thumb, end chevrons (Classic only).
    if !lemon {
    ui.c.fill_rect(track, t.scrollbar);
    ui.c.fill_rect(Rect::new(track.x, track.y, 1.0, track.h), ui.t.s(0x181c1e));
    ui.c.fill_rect(Rect::new(track.right() - 1.0, track.y, 1.0, track.h), ui.t.s(0x181c1e));
    let total_px = total_steps as f32 * STEP_W;
    let tlen = (view / total_px * track.w).clamp(16.0f32.min(track.w), track.w.max(0.0));
    let max = (total_px - view).max(0.0);
    let off = if max > 0.0 { sx / max * (track.w - tlen) } else { 0.0 };
    ui.c.fill_rect(Rect::new(track.x + off + 6.0, track.y + 2.0, (tlen - 12.0).max(8.0), 3.0), ui.t.s(0x14181a));
    let chev = ui.t.s(0x3c4042);
    let cy = track.cy();
    ui.c.polyline(&[(track.x, cy - 3.0), (track.x - 3.0, cy), (track.x, cy + 3.0)], 1.3, chev);
    ui.c.polyline(&[(x + w - 23.0, cy - 3.0), (x + w - 20.0, cy), (x + w - 23.0, cy + 3.0)], 1.3, chev);
    }

    // Resize zones: right edge, bottom edge, corner (§5).
    let (r, b) = (win.right(), win.bottom());
    ui.hit(Rect::span(r - 3.0, r + 4.0, y + 29.0, b - 8.0), Action::RackResize(EDGE_RIGHT), "Drag to resize");
    ui.hit(Rect::span(x, r - 8.0, b - 3.0, b + 4.0), Action::RackResize(EDGE_BOTTOM), "Drag to resize");
    ui.hit(Rect::span(r - 10.0, r + 4.0, b - 10.0, b + 4.0), Action::RackResize(EDGE_RIGHT | EDGE_BOTTOM), "Drag to resize");

    graph_editor(ui, s, sx, first_step, steps_shown);
}

/// Title bar contents (§2, §7). Left items follow the name column; the right group follows the right edge.
fn title_bar(ui: &mut Ui, s: &State, win: Rect, c: Cols, f: &FrameInfo) {
    let t = ui.t;
    let lemon = super::lemon(ui);
    let (x, y, r) = (win.x, win.y, win.right());
    let ohov = ui.hit(Rect::new(x + 8.0, y + 6.0, 16.0, 17.0), Action::RackOptions, "Channel options");
    ui.c.fill_polygon(&[(x + 14.0, y + 10.5), (x + 18.5, y + 14.0), (x + 14.0, y + 17.5)], if ohov { ui.t.tx(0xffffff) } else { ui.t.tx(0xc2c8cc) });
    // Loop starter: not available yet, greyed out.
    let sw = Rect::new(x + 34.0, y + 6.0, 16.0, 16.0);
    ui.off(sw, "Loop starter");
    icons::draw(ui.c, Icon::Countdown, sw, os_ui::kit::ink(t, os_ui::kit::State::NORMAL.with_disabled(true)));
    // Display filter combo (§7, §21).
    let combo = Rect::span(x + c.name_x - 1.0, x + c.name_x + c.name_w, y + 4.0, y + 27.0);
    let chov = ui.hit(combo, Action::RackFilter, "Channel display filter (PgUp/PgDn)");
    if lemon {
        lk::well(ui.c, t, combo, if chov { t.sunken.mix(t.button_hi, 0.5) } else { t.sunken });
    } else {
        ui.c.fill_rect(combo, ui.t.s(0x192227));
        let cf = combo.inset(1.0);
        ui.c.fill_rect(Rect::new(cf.x, cf.y, cf.w, 1.0), ui.t.s(0x303132));
        ui.c.fill_rect(Rect::new(cf.x, cf.y + 1.0, cf.w, cf.h / 2.0), ui.t.s(0x202122));
        ui.c.fill_rect(Rect::span(cf.x, cf.right(), cf.y + 1.0 + cf.h / 2.0, cf.bottom()), ui.t.s(0x1b1c1d));
    }
    let label = match s.rack.filter {
        rk::Filter::All => "All".to_string(),
        rk::Filter::Group(g) => s.rack.groups.get(g).map_or("All".to_string(), |g| g.name.clone()),
        rk::Filter::Unsorted => "Unsorted".to_string(),
    };
    let lf = Font::new(17);
    let label = shorten(ui, lf, &label, combo.w - 18.0);
    ui.c.text(lf, combo.x + 6.0, y + 5.0, if chov { t.glyph.shade(0.1) } else { t.glyph }, &label);
    let ax = combo.right() - 9.0;
    ui.c.fill_polygon(&[(ax, y + 11.5), (ax + 4.0, y + 15.0), (ax, y + 18.5)], ui.t.s(0x686c6e));
    // Dots: drag to resize the channel buttons (§7).
    let dots = Rect::new(x + 200.0 + c.dx, y + 5.0, 10.0, 20.0);
    let dhov = ui.hit(dots, Action::RackDots, "Drag to resize channel buttons");
    for k in 0..4 {
        ui.c.fill_rect(Rect::new(x + 204.0 + c.dx, y + 8.0 + 4.0 * k as f32, 2.0, 2.0), if dhov { ui.t.tx(0xa0a4a6) } else { ui.t.s(0x4c4b47) });
    }
    // Speaker: play / pause the pattern; orange while the pattern plays (§7).
    let spk = Rect::new(x + 213.0 + c.dx, y + 5.0, 19.0, 19.0);
    let shov = ui.hit(spk, Action::RackSpeaker, "Play / pause pattern");
    let pat_playing = f.playing && !f.paused && !f.song;
    let sc = if pat_playing { ui.t.h(0xffa53a) } else if shov { ui.t.tx(0xffffff) } else { t.glyph };
    icons::draw(ui.c, Icon::Speaker, Rect::new(x + 215.0 + c.dx, y + 7.0, 15.0, 15.0), sc);
    ui.c.push_clip(Rect::span(x + 236.0 + c.dx, r - 165.0, y, y + 29.0));
    ui.c.text(Font::new(17), x + 236.0 + c.dx, y + 5.0, t.text, "Channel rack");
    ui.c.pop_clip();
    // Swing knob with its halo (§7); a non-zero swing draws an orange arc.
    let kx = r - 149.5;
    let swing = s.knob(KnobId::Swing);
    ui.hit(Rect::new(kx - 12.0, y + 3.0, 24.0, 24.0), Action::Knob(KnobId::Swing), s.knob_text(KnobId::Swing));
    ui.c.fill_circle(kx, y + 14.5, 12.0, ui.t.s(0x212225));
    if swing > 0.0 {
        let start = 0.75 * std::f32::consts::PI;
        ui.c.arc(kx, y + 14.5, 10.5, start, 1.5 * std::f32::consts::PI * swing, 2.0, ui.t.h(0xffa53a));
    } else {
        ui.c.fill_circle(kx, y + 14.5, 10.5, ui.t.s(0x2b2d2f));
    }
    super::ball_knob(ui, kx, y + 14.5, 9.0, swing);
    // Pattern length box (§7): "···" = Auto, else the length in steps.
    let bx = Rect::span(r - 132.0, r - 100.0, y + 4.0, y + 26.0);
    let bhov = ui.hit(bx, Action::RackLen, "Pattern length");
    if lemon {
        lk::small_box(ui.c, t, bx, ui.t.s(0x35393b), bhov);
    } else {
        ui.c.fill_rrect(bx, 3.5, ui.t.s(0x191d1f));
        ui.c.fill_rect_gradient(bx.inset(1.0), ui.t.s(0x35393b), ui.t.s(0x2f3335), 3.0);
    }
    let lc = if bhov { ui.t.tx(0xffffff) } else { ui.t.tx(0xa7adb0) };
    match rk::explicit_len(s) {
        None => {
            for k in 0..3 {
                ui.c.fill_rect(Rect::new(r - 116.0 + 3.0 * k as f32, y + 14.0, 2.0, 1.0), lc);
            }
        }
        Some(n) => ui.c.text_right(Font::new(14), r - 106.0, y + 6.0, lc, &n.to_string()),
    }
    spin_mark(ui, r - 104.0, y + 15.0, ui.t.s(0x6f7476));
    // Loop mode (§7, §15): green outline when on.
    let lr = Rect::new(r - 90.5, y + 5.0, 19.0, 19.0);
    let lhov = ui.hit(lr, Action::RackLoop, "Loop mode");
    let green = ui.t.a(0x8fd14a);
    if lemon {
        // Lemon: toggles are lit rounded squares with dark glyphs.
        if s.rack.loop_mode {
            lk::lit(ui.c, lr, lk::r_btn(t), lhov);
        } else if lhov {
            ui.c.fill_rrect(lr, lk::r_btn(t), t.button_hi);
        }
        ui.c.icon(&os_ui::tabler::REPEAT, lr.inset(2.5), if s.rack.loop_mode { lk::ink_on_lemon() } else { t.glyph });
    } else {
        if s.rack.loop_mode || lhov {
            ui.c.stroke_rrect(lr, 3.0, 1.0, if s.rack.loop_mode { green } else { t.glyph.shade(-0.3) });
        }
        ui.c.icon(&os_ui::tabler::REPEAT, lr.inset(2.5), if s.rack.loop_mode { green } else if lhov { ui.t.tx(0xffffff) } else { t.glyph });
    }
    // Graph editor (§7, §14): blue outline when on.
    let gx = r - 58.0;
    let gr = Rect::new(gx - 9.5, y + 5.0, 19.0, 19.0);
    let ghov = ui.hit(gr, Action::RackGraph, "Graph editor (Ctrl+K)");
    let blue = ui.t.a(0x40a8e0);
    let gc = if lemon {
        if s.rack.graph_open {
            lk::lit(ui.c, gr, lk::r_btn(t), ghov);
        } else if ghov {
            ui.c.fill_rrect(gr, lk::r_btn(t), t.button_hi);
        }
        if s.rack.graph_open { lk::ink_on_lemon() } else { t.glyph }
    } else {
        if s.rack.graph_open || ghov {
            ui.c.stroke_rrect(gr, 3.0, 1.0, if s.rack.graph_open { blue } else { t.glyph.shade(-0.3) });
        }
        if s.rack.graph_open { blue } else if ghov { ui.t.tx(0xffffff) } else { t.glyph }
    };
    for (i, hgt) in [5.0, 9.0, 6.0, 11.0, 4.0].into_iter().enumerate() {
        ui.c.fill_rect(Rect::new(gx - 7.0 + 3.0 * i as f32, y + 20.0 - hgt, 2.0, hgt), gc);
    }
    ui.c.fill_rect(Rect::new(gx - 8.0, y + 20.0, 16.0, 1.0), gc.shade(-0.3));
    // Step sequencer / piano roll overview (§6).
    let roll_view = s.rack_roll_view;
    let sxc = r - 35.0;
    let br = Rect::new(sxc - 10.0, y + 5.0, 20.0, 20.0);
    let hov = ui.hit(br, Action::RackRollView, "Interactive step sequencer / piano roll overview");
    // On (§6): orange outline with a dark orange fill; hover draws the outline.
    if lemon {
        if roll_view {
            lk::lit(ui.c, br, lk::r_btn(t), hov);
        } else if hov {
            ui.c.fill_rrect(br, lk::r_btn(t), t.button_hi);
        }
        let oc = if roll_view { lk::ink_on_lemon() } else { t.accent };
        for i in 0..3 {
            ui.c.fill_rrect(Rect::new(sxc - 7.5 + 5.5 * i as f32, y + 9.0, 4.0, 11.0), 1.0, oc);
        }
    } else {
        if roll_view {
            ui.c.fill_rrect(br, 3.0, ui.t.h(0xc1923a));
            ui.c.fill_rrect(br.inset(1.0), 2.5, ui.t.h(0x574b30));
        } else if hov {
            ui.c.stroke_rrect(br, 3.0, 1.0, ui.t.h(0xc1923a));
        }
        let oc = if hov { ui.t.h(0xffd77a) } else { ui.t.h(0xffc642) };
        for i in 0..3 {
            let b = Rect::new(sxc - 8.0 + 5.5 * i as f32, y + 9.0, 4.5, 11.0);
            ui.c.fill_rect(b, oc);
            ui.c.fill_rect(Rect::new(b.x + 0.5, b.y + 2.0, b.w - 1.0, 1.0), Color::rgb(0x71292b));
        }
    }
    let close = Rect::new(r - 22.0, y + 6.0, 16.0, 16.0);
    let hov = ui.hit(close, Action::CloseWin(Win::ChannelRack), "Close");
    icons::draw(ui.c, Icon::Close, close.inset(1.5), if hov { ui.t.tx(0xffffff) } else { t.glyph });
}

/// One step button (§3): bordered, lighter at the top, a dark dash below the top edge (red at the play position).
fn step_button(ui: &mut Ui, r: Rect, k: u32, on: bool, past_len: bool, hov: bool, playing: bool) {
    let t = ui.t;
    let odd = (k / 4) % 2 == 1;
    if super::lemon(ui) {
        return lk::step(ui.c, t, r, on, odd, past_len, hov, playing);
    }
    let (border, face) = match (past_len, odd) {
        (false, false) => (t.rack_control_border.shade(-0.1), t.step_a),
        (false, true) => (t.rack_control_border.shade(-0.1), t.step_b),
        (true, false) => (t.step_dim_border, t.step_dim_a),
        (true, true) => (t.step_dim_border, t.step_dim_b),
    };
    let face = if on { t.step_on } else { face };
    let face = if hov { face.shade(0.1) } else { face };
    ui.c.fill_rrect(r, 2.5, border);
    let f = r.inset(1.0);
    if past_len && !on {
        ui.c.fill_rect(f, face);
        ui.c.fill_rect(Rect::new(f.x + 2.0, f.y + 2.0, f.w - 4.0, 2.0), face.shade(-0.18));
        return;
    }
    ui.c.fill_rect_gradient(f, face.shade(0.14), face.shade(-0.12), 1.5);
    // Cap highlight and the dash.
    ui.c.fill_rect(Rect::new(f.x, f.y, f.w, 1.0), face.shade(0.2));
    let dash = if playing { Color::rgb(0xe03030) } else { face.shade(-0.55) };
    ui.c.fill_rect(Rect::new(f.x + 2.0, f.y + 2.0, f.w - 4.0, 2.0), dash);
    ui.c.fill_rect(Rect::new(f.x, f.bottom() - 2.0, f.w, 2.0), face.shade(-0.35));
}

/// Graph editor (§14): one column per step, the focused channel's step values as bars, the tab row below.
fn graph_editor(ui: &mut Ui, s: &State, sx: f32, first: u32, shown: u32) {
    let (Some(g), Some(bars)) = (graph_rect(s), graph_bars(s)) else { return };
    let t = ui.t;
    let tab = s.rack.graph_tab;
    let ps = rk::per_step(s);
    ui.c.fill_rect(g, ui.t.s(0x15181a));
    ui.c.stroke_rrect(g, 0.0, 1.0, ui.t.s(0x0c0e0f));
    ui.hit(g, Action::Swallow, "");
    let ch = s.project.channels.get(s.selected_channel).map(|c| c.index);
    let notes = rk::row_notes(s, s.selected_channel);
    ui.c.push_clip(bars);
    let x0 = s.rack_pos.0 + cols(s).step_x;
    for k in first..first + shown + 1 {
        let cx = x0 + STEP_W * k as f32 - sx;
        let col = Rect::new(cx - 1.0, bars.y, STEP_W, bars.h);
        ui.c.fill_rect(col, if (k / 4) % 2 == 0 { ui.t.s(0x1b1e20) } else { ui.t.s(0x202326) });
        ui.c.fill_rect(Rect::new(col.right() - 1.0, col.y, 1.0, col.h), ui.t.s(0x111315));
        let Some(n) = notes.iter().find(|n| n.position / ps == k) else { continue };
        let v = tab.get(n, ps);
        let c = ui.t.a(tab.color());
        if tab.centred() {
            let mid = bars.y + bars.h / 2.0;
            let yv = bars.bottom() - v * bars.h;
            ui.c.fill_rect(Rect::span(cx + 3.0, cx + 10.0, yv.min(mid), yv.max(mid) + 1.0), c.shade(-0.35));
            ui.c.fill_rect(Rect::new(cx + 2.0, yv - 1.0, 9.0, 2.0), c);
        } else if tab == Tab::Note {
            let yv = bars.bottom() - v * (bars.h - 14.0) - 7.0;
            ui.c.fill_rect(Rect::new(cx + 1.0, yv - 2.0, 11.0, 4.0), c);
            ui.c.text_center(Font::new(10), Rect::new(cx - 4.0, yv - 16.0, 22.0, 12.0), c, &rk::key_name(n.key));
        } else {
            let yv = bars.bottom() - v * bars.h;
            ui.c.fill_rect(Rect::span(cx + 3.0, cx + 10.0, yv, bars.bottom()), c.shade(-0.4));
            ui.c.fill_rect(Rect::new(cx + 3.0, yv, 7.0, 2.0), c);
        }
    }
    ui.c.pop_clip();
    let hint = match ch {
        Some(_) => format!("Graph editor: {} - click / drag to set (left or right button)", tab.label()),
        None => "Graph editor".into(),
    };
    ui.hit(bars, Action::RackGraphArea, hint);
    // Tab row.
    let tr = Rect::new(g.x, g.bottom() - GRAPH_TABS_H, g.w, GRAPH_TABS_H);
    ui.c.fill_rect(tr, ui.t.s(0x23272a));
    let tw = (tr.w / Tab::ALL.len() as f32).floor();
    for (i, tb) in Tab::ALL.into_iter().enumerate() {
        let r = Rect::new(tr.x + i as f32 * tw, tr.y, tw, tr.h);
        let hov = ui.hit(r, Action::RackGraphTab(i), tb.label());
        let c = if tb == tab { ui.t.a(tb.color()) } else if hov { t.glyph } else { t.text_dim };
        ui.c.push_clip(r);
        ui.c.text_center(Font::new(12), r, c, tb.label());
        ui.c.pop_clip();
    }
}

/// Mini piano roll of one channel (§6) in `block` (one 30 px row); `sx` is the step scroll in pixels.
fn mini_roll(ui: &mut Ui, block: Rect, notes: &[os_model::Note], ch: u16, per_step: u32, sx: f32, focused: bool) {
    // (separator, fill) for odd and even bars.
    let shades = if focused {
        [(ui.t.s(0x040709), ui.t.s(0x111416)), (ui.t.s(0x000002), ui.t.s(0x0a0d0f))]
    } else {
        [(ui.t.s(0x0a0d0f), ui.t.s(0x171a1c)), (ui.t.s(0x030608), ui.t.s(0x101315))]
    };
    let bar = 16.0 * STEP_W;
    let mut k = (sx / bar).floor();
    loop {
        let x0 = block.x + 2.0 + k * bar - sx;
        if x0 >= block.right() {
            break;
        }
        let span = Rect::span(x0.max(block.x), (x0 + bar).min(block.right()), block.y, block.bottom());
        let (sep, fill) = shades[(k as i64 % 2) as usize];
        ui.c.fill_rect(span, sep);
        ui.c.fill_rect(Rect::span(span.x, span.right(), span.y + 1.0, span.bottom() - 1.0), fill);
        k += 1.0;
    }
    let mine: Vec<&os_model::Note> = notes.iter().filter(|n| n.channel == ch).collect();
    let lo = mine.iter().map(|n| n.key).min().unwrap_or(60);
    let hi = mine.iter().map(|n| n.key).max().unwrap_or(60);
    let range = f32::from((hi - lo).max(1));
    ui.c.push_clip(block);
    for n in mine {
        let nx = block.x + 2.0 + n.position as f32 / per_step as f32 * STEP_W - sx;
        let len = if n.length == 0 { per_step } else { n.length };
        let nw = (len as f32 / per_step as f32 * STEP_W - 1.0).max(2.0);
        let t = if hi == lo { 0.5 } else { f32::from(n.key - lo) / range };
        let ny = (block.bottom() - 7.0 - t * (block.h - 10.0)).round();
        ui.c.fill_rect(Rect::new(nx, ny, nw, 1.0), ui.t.a(0x698a6f));
        ui.c.fill_rect(Rect::new(nx, ny + 1.0, nw, 3.0), ui.t.a(0x9ed1a5));
    }
    ui.c.pop_clip();
}

/// Row knob (§3): dark ball, r 7, light pointer from the centre to the rim.
fn row_knob(ui: &mut Ui, cx: f32, cy: f32, value: f32) {
    let hov = ui.hovering(Rect::new(cx - 7.5, cy - 7.5, 15.0, 15.0));
    os_ui::kit::knob(ui.c, ui.t, cx, cy, 7.5, value, false, os_ui::kit::KnobLook::Row, os_ui::kit::State::hover(hov));
}

/// Enable LED (§3: ring r 5, green ball).
fn rack_led(ui: &mut Ui, cx: f32, cy: f32, on: bool) {
    let hov = ui.hovering(Rect::new(cx - 6.0, cy - 6.0, 12.0, 12.0));
    os_ui::kit::led(ui.c, ui.t, cx, cy, 5.0, os_ui::kit::State::hover(hov).with_on(on));
}

/// The small green padlock of a locked channel (§10); dimmed when the channel is muted.
fn padlock(ui: &mut Ui, cx: f32, cy: f32, on: bool) {
    let c = if on { ui.t.a(0x97da3a) } else { ui.t.s(0x67696b) };
    ui.c.stroke_rrect(Rect::new(cx - 2.5, cy - 6.0, 5.0, 6.0), 2.5, 1.3, c);
    ui.c.fill_rrect(Rect::new(cx - 4.5, cy - 1.5, 9.0, 7.0), 1.0, c);
}

/// The small up/down spin mark beside a number (§3).
fn spin_mark(ui: &mut Ui, x: f32, cy: f32, c: Color) {
    os_ui::kit::spin_mark(ui.c, x, cy, c);
}

/// The add-channel "+" (§4) under the name column; `bottom` is the window's bottom edge.
fn add_button(ui: &mut Ui, x: f32, w: f32, bottom: f32) {
    let t = ui.t;
    let area = Rect::span(x + 2.0, x + w, bottom - 26.0, bottom - 5.0);
    let hov = ui.hit(area, Action::RackAdd, "Add a channel");
    let (cx, cy) = ((x + w / 2.0 + 1.5).round() + 0.5, bottom - 15.5);
    let c = if hov { ui.t.h(0xffc63b) } else { ui.t.tx(0xd4dade) };
    let lemon = super::lemon(ui);
    if lemon && hov {
        ui.c.fill_rrect(area.inset(1.0), lk::r_btn(t), t.button_hi);
    }
    ui.c.fill_rect(Rect::new(cx - 4.5, cy - 1.0, 9.0, 2.0), c);
    ui.c.fill_rect(Rect::new(cx - 1.0, cy - 4.5, 2.0, 9.0), c);
    if hov && !lemon {
        let (l, r) = (x + 3.0, x + w - 1.0);
        let pts = [(l, bottom - 24.0), (r, bottom - 24.0), (r - 8.0, bottom - 6.0), (l + 7.0, bottom - 6.0), (l, bottom - 24.0)];
        ui.c.polyline(&pts, 2.0, t.add_hover);
    }
}

pub fn insert_name(track: u16) -> String {
    if track == 0 { "Master".into() } else { format!("Insert {track}") }
}

/// The Advanced fill dialog (§22), drawn above everything while open.
pub fn paint_fill(ui: &mut Ui, s: &State) {
    let Some(fd) = s.rack.fill.as_ref() else { return };
    let t = ui.t;
    let r = Rect::new(fd.at.0, fd.at.1, FILL_W, FILL_H);
    ui.hit(Rect::new(0.0, 0.0, s.screen.0, s.screen.1), Action::Swallow, "");
    ui.c.fill_rect(r, t.window_border);
    ui.c.fill_rect(r.inset(1.0), t.window_body);
    ui.hit(r, Action::RackFill(FillHit::Inside), "");
    let font = Font::new(14);
    // Title bar.
    ui.c.fill_rect(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, 26.0), t.window_title_hi.shade(-0.05));
    ui.c.text(Font::new(15), r.x + 10.0, r.y + 5.0, t.text, "Advanced fill");
    let close = Rect::new(r.right() - 22.0, r.y + 6.0, 16.0, 16.0);
    let hov = ui.hit(close, Action::RackFill(FillHit::Close), "Close");
    icons::draw(ui.c, Icon::Close, close.inset(1.5), if hov { ui.t.tx(0xffffff) } else { t.glyph });
    // Preview row: the channel.
    let name = s.project.channels.get(fd.row).map(|c| c.name.clone().unwrap_or_else(|| format!("Channel {}", c.index + 1))).unwrap_or_default();
    ui.c.text(font, r.x + 12.0, r.y + 34.0, t.text_dim, "Preview");
    ui.c.fill_polygon(&[(r.x + 70.0, r.y + 38.0), (r.x + 75.0, r.y + 42.0), (r.x + 70.0, r.y + 46.0)], t.glyph);
    ui.c.text_clip(font, r.x + 82.0, r.y + 34.0, r.w - 94.0, t.text, &name);
    // Ring of steps.
    let (cx, cy, rad) = (r.x + r.w / 2.0, r.y + 172.0, 105.0);
    let n = fd.steps.len().max(1);
    let pos = |k: usize| {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * k as f32 / n as f32;
        (cx + rad * a.cos(), cy + rad * a.sin())
    };
    ui.c.stroke_circle(cx, cy, rad, 1.0, ui.t.s(0x3a3e41));
    let on: Vec<(f32, f32)> = (0..n).filter(|&k| fd.steps.get(k).copied().unwrap_or(false)).map(pos).collect();
    let brown = ui.t.h(0xc07a3a);
    if on.len() >= 3 {
        ui.c.fill_polygon(&on, brown.with_alpha(50));
    }
    if on.len() >= 2 {
        let mut ring = on.clone();
        ring.push(on[0]);
        ui.c.polyline(&ring, 1.5, brown.shade(-0.2));
    }
    for k in 0..n {
        let (px, py) = pos(k);
        let active = fd.steps.get(k).copied().unwrap_or(false);
        let hr = Rect::new(px - 7.0, py - 7.0, 14.0, 14.0);
        let hov = ui.hit(hr, Action::RackFill(FillHit::Dot(k)), format!("Step {} - click to toggle", k + 1));
        if active {
            ui.c.fill_circle(px, py, 6.0, if hov { brown.shade(0.2) } else { brown });
        } else {
            ui.c.fill_circle(px, py, 3.0, if hov { t.glyph } else { ui.t.s(0x6a6e71) });
        }
    }
    // Centre: rotation.
    for (d, label, dx) in [(-1, "Rotate left", -22.0), (1, "Rotate right", 6.0)] {
        let br = Rect::new(cx + dx, cy - 8.0, 16.0, 16.0);
        let hov = ui.hit(br, Action::RackFill(FillHit::Rotate(d)), label);
        let dir = if d < 0 { os_ui::kit::Dir::Left } else { os_ui::kit::Dir::Right };
        os_ui::kit::arrow_button(ui.c, t, br, dir, os_ui::kit::State::hover(hov));
    }
    ui.c.text_center(Font::new(11), Rect::new(cx - 20.0, cy + 10.0, 40.0, 14.0), t.text_dim, &format!("{}", fd.rotate));
    // Buttons: random, reset.
    let by = r.y + 296.0;
    for (i, (label, hit)) in [("Random", FillHit::Random), ("Reset", FillHit::Reset)].into_iter().enumerate() {
        let br = Rect::new(r.x + 12.0 + i as f32 * 84.0, by, 76.0, 22.0);
        let hov = ui.hit(br, Action::RackFill(hit), label);
        os_ui::kit::button(ui.c, t, br, os_ui::kit::State::hover(hov));
        ui.c.text_center(font, br, t.text, label);
    }
    // Sequence group.
    let gy = r.y + 330.0;
    ui.c.fill_rect(Rect::new(r.x + 8.0, gy, r.w - 16.0, 18.0), t.menu_pop_head);
    ui.c.text(Font::new(12), r.x + 12.0, gy + 2.0, t.menu_pop_head_text, "Sequence");
    let rows: [(&str, String, Option<FillHit>, Option<(FillHit, FillHit)>); 3] = [
        ("Fill type", fd.kind.label().to_string(), Some(FillHit::Type), None),
        ("Sequence length", fd.len.to_string(), None, Some((FillHit::Len(-1), FillHit::Len(1)))),
        (
            if fd.kind == FillType::Euclid { "Step count" } else { "Interval" },
            if fd.kind == FillType::Manual { "---".into() } else { fd.amount.to_string() },
            None,
            (fd.kind != FillType::Manual).then_some((FillHit::Amount(-1), FillHit::Amount(1))),
        ),
    ];
    for (i, (label, value, click, arrows)) in rows.into_iter().enumerate() {
        let ry = gy + 24.0 + i as f32 * 24.0;
        ui.c.text(font, r.x + 14.0, ry + 2.0, t.text_dim, label);
        let vb = Rect::new(r.x + 140.0, ry, 90.0, 21.0);
        let hov = match click {
            Some(h) => ui.hit(vb, Action::RackFill(h), "Click to change"),
            None => false,
        };
        os_ui::kit::sunken(ui.c, t, vb, if hov { t.tempo_box.shade(0.08) } else { t.tempo_box });
        ui.c.text_center(font, vb, t.text, &value);
        if let Some((dn, up)) = arrows {
            for (j, h) in [dn, up].into_iter().enumerate() {
                let ar = Rect::new(vb.right() + 4.0 + j as f32 * 18.0, ry + 2.0, 16.0, 17.0);
                let hov = ui.hit(ar, Action::RackFill(h), if j == 0 { "Less" } else { "More" });
                let dir = if j == 0 { os_ui::kit::Dir::Left } else { os_ui::kit::Dir::Right };
                os_ui::kit::arrow_button(ui.c, t, ar, dir, os_ui::kit::State::hover(hov));
            }
        }
    }
    // Accept.
    let ar = Rect::new(r.right() - 92.0, r.bottom() - 32.0, 80.0, 24.0);
    let hov = ui.hit(ar, Action::RackFill(FillHit::Accept), "Write the sequence to the channel's steps (Enter)");
    os_ui::kit::button(ui.c, t, ar, os_ui::kit::State::hover(hov));
    ui.c.text_center(font, ar, t.text, "Accept");
}
