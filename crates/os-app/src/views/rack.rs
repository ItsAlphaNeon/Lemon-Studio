//! Channel rack window: a floating, resizable child window.
// spec: ui/channel-rack.md

use os_ui::{Color, Font, Rect};

use super::{ball_knob, rgb, FrameInfo};
use crate::app::{Action, Scroll, State, Ui, Win};
use crate::icons::{self, Icon};

pub const ROW_H: f32 = 30.0;
pub const STEP_W: f32 = 16.0;
/// Default outer size (§1).
pub const DEFAULT_SIZE: (f32, f32) = (499.0, 246.0);
/// Smallest size: the default width, one row.
pub const MIN_SIZE: (f32, f32) = (307.0, 96.0 + ROW_H);

/// Snap a dragged size so whole steps and rows appear (§5: W = 259 + 16·(n−1), H = 96 + 30·rows).
pub fn snap_size(w: f32, h: f32) -> (f32, f32) {
    let w = 259.0 + STEP_W * ((w - 259.0) / STEP_W).round();
    let h = 96.0 + ROW_H * ((h - 96.0) / ROW_H).round();
    (w.max(MIN_SIZE.0), h.max(MIN_SIZE.1))
}

/// Whether every note of channel `ch` in the pattern is a plain step (§6: otherwise the row shows
/// the mini piano roll even in step mode).
fn plain_steps(notes: &[os_model::Note], ch: u16) -> bool {
    notes.iter().filter(|n| n.channel == ch).all(|n| n.length == 0 && n.key == 60)
}
/// x of the first step (§3).
const STEP_X: f32 = 217.0;
/// Space above the first row and below the last one inside the rows panel.
const PAD: f32 = 18.0;

/// Resize edges (bit set) for `Action::RackResize`.
pub const EDGE_RIGHT: u8 = 1;
pub const EDGE_BOTTOM: u8 = 2;

/// The window's outer rectangle.
pub fn rect(s: &State) -> Rect {
    Rect::new(s.rack_pos.0, s.rack_pos.1, s.rack_size.0, s.rack_size.1)
}

/// Everything the window can paint or hit (incl. the resize zones past its edges).
pub fn bounds(s: &State) -> Rect {
    rect(s).outset(4.0)
}

/// Inside of the rows panel (§1: border at x 4 and W−6, y 29 and H−30).
pub fn panel(win: Rect) -> Rect {
    Rect::span(win.x + 5.0, win.right() - 6.0, win.y + 30.0, win.bottom() - 30.0)
}

/// Steps that fit the window width (§3: 16 at the default width, 36 at 819 px).
pub fn visible_steps(win: Rect) -> u32 {
    (((win.w - 259.0) / STEP_W).floor() as i32 + 1).max(1) as u32
}

/// Pattern length in steps: whole bars (16 steps) past the last used step, at least one bar.
pub fn pattern_steps(s: &State) -> u32 {
    let per_step = (u32::from(s.project.ppq.max(4)) / 4).max(1);
    let last = s
        .project
        .patterns
        .iter()
        .find(|p| p.number == s.pattern)
        .and_then(|p| p.notes.iter().map(|n| n.position / per_step).max());
    last.map_or(16, |l| (l / 16 + 1) * 16)
}

/// Height of all rows plus the padding (content pixels for the vertical scroll bar).
pub fn content_h(s: &State) -> f32 {
    s.project.channels.len() as f32 * ROW_H + 2.0 * PAD
}

pub fn paint(ui: &mut Ui, s: &State, f: &FrameInfo) {
    let roll_view = s.rack_roll_view;
    let t = ui.t;
    let win = rect(s);
    let (x, y, w, h) = (win.x, win.y, win.w, win.h);
    // Frame (§1).
    ui.c.fill_rect(win, t.window_border);
    let body = win.inset(1.0);
    ui.c.fill_rect(body, t.window_body);
    ui.hit(win, Action::RackArea, "Channel rack");
    ui.c.fill_rect(Rect::new(body.x, body.y, body.w, 1.0), t.window_title_hi);
    let bar = Rect::span(x + 1.0, x + w - 1.0, y + 1.0, y + 29.0);
    ui.hit(bar, Action::WinDrag(Win::ChannelRack), "Channel rack - drag to move");
    title_bar(ui, win, roll_view);

    // Rows panel (§1, §3).
    let inner = panel(win);
    ui.c.fill_rect(inner.outset(1.0), t.rack_inset_border);
    ui.c.fill_rect(inner, t.rack_inset);
    let view_h = inner.h;
    let total_h = content_h(s);
    let vbar = total_h > view_h + 0.5;
    let rows_clip = if vbar { Rect::span(inner.x, inner.right() - 11.0, inner.y, inner.bottom()) } else { inner };
    let scroll = s.rack_scroll.clamp(0.0, (total_h - view_h).max(0.0));
    let steps_shown = visible_steps(win);
    let len = pattern_steps(s);
    let total_steps = len.max(steps_shown);
    // Steps continue past the pattern (§3, §4: the thumb is small even for a one-bar pattern).
    let scroll_steps = total_steps + 256;
    let sx = s.rack_scroll_x.clamp(0.0, (scroll_steps - steps_shown) as f32 * STEP_W);
    let first_step = (sx / STEP_W).floor() as u32;
    let ppq = u32::from(s.project.ppq.max(4));
    let per_step = ppq / 4;
    let pat = s.project.patterns.iter().find(|p| p.number == s.pattern);
    // Playback position (pattern mode) for the activity slots.
    let tick = (f.playing && !s.song_mode).then(|| {
        let loop_len = f64::from(len * per_step);
        (f.seconds * s.tempo / 60.0 * f64::from(ppq)) % loop_len
    });

    ui.c.push_clip(rows_clip);
    let first = (scroll / ROW_H) as usize;
    let last = (((scroll + view_h) / ROW_H) as usize + 1).min(s.project.channels.len());
    for i in first..last {
        let ch = &s.project.channels[i];
        // Row 0's name button top is at y 50 (§3).
        let ry = y + 50.0 + i as f32 * ROW_H - scroll;
        let focused = i == s.selected_channel;
        // Enable LED.
        let lr = Rect::new(x + 11.5, ry + 6.5, 12.0, 12.0);
        ui.hit(lr, Action::ChannelEnable(i), "Channel enabled - click to mute / unmute");
        rack_led(ui, x + 17.5, ry + 12.5, ch.enabled);
        // Pan and volume knobs.
        ui.hit(Rect::new(x + 27.0, ry + 5.0, 15.0, 16.0), Action::Placeholder("channel pan"), format!("Channel panning: {}", pan_text(ch.pan)));
        row_knob(ui, x + 34.5, ry + 13.0, 0.5 + ch.pan as f32 / 2.0);
        ui.hit(Rect::new(x + 44.0, ry + 5.0, 15.0, 16.0), Action::Placeholder("channel volume"), format!("Channel volume: {:.0}%", ch.volume * 100.0));
        row_knob(ui, x + 51.5, ry + 13.0, (ch.volume as f32).clamp(0.0, 1.0));
        // Mixer track box (drag to change).
        let mb = Rect::span(x + 66.0, x + 99.0, ry + 1.0, ry + 25.0);
        let mhov = ui.hit(mb, Action::RackMixer(i), format!("Mixer track: {} - drag to change", insert_name(ch.mixer_track)));
        ui.c.fill_rrect(mb, 3.0, t.rack_control_border);
        ui.c.fill_rect_gradient(mb.inset(1.0), t.rack_row_box.shade(-0.04), t.rack_row_box, 2.5);
        let num_c = if mhov { t.glyph } else { t.text_dim.shade(0.08) };
        let label = if ch.mixer_track == 0 { "M".to_string() } else { ch.mixer_track.to_string() };
        let nf = super::light(19.0);
        let lh = ui.c.line_height(nf);
        ui.c.text_right(nf, x + 91.0, (mb.cy() - lh / 2.0).round(), num_c, &label);
        spin_mark(ui, x + 94.0, ry + 13.0, t.text_dim.shade(-0.1));
        // Name button: opens the channel's plugin window.
        let nb = Rect::span(x + 108.0, x + 190.0, ry, ry + 26.0);
        let nhov = ui.hit(nb, Action::ChannelSelect(i), "Click to show the channel's plugin window");
        let face = if focused { t.name_button_sel } else { t.name_button };
        let face = if nhov { face.shade(0.06) } else { face };
        ui.c.fill_rrect(nb, 3.0, t.rack_control_border.shade(0.08));
        ui.c.fill_rect(Rect::new(nb.x + 1.0, nb.y + 1.0, nb.w - 2.0, 1.0), face.shade(0.12));
        ui.c.fill_rect_gradient(Rect::new(nb.x + 1.0, nb.y + 2.0, nb.w - 2.0, nb.h - 4.0), face.shade(0.03), face.shade(-0.04), 1.5);
        let col = ch.color.map(rgb).map_or(t.name_text, |c| c.mix(t.name_text, 0.55));
        let col = if focused { col.shade(0.25) } else { col };
        let label = ch.name.clone().unwrap_or_else(|| format!("Channel {}", ch.index + 1));
        let nf = ui.c.fit_font(Font::new(14), &label, nb.w - 6.0);
        ui.c.push_clip(nb.inset(1.0));
        ui.c.text_center(nf, nb, col, &label);
        ui.c.pop_clip();
        // Activity / select slot.
        let slot = Rect::span(x + 200.0, x + 206.0, ry + 2.0, ry + 24.0);
        let sel_hit = Rect::span(x + 196.0, x + 210.0, ry, ry + 26.0);
        ui.hit(sel_hit, Action::RackSelect(i), "Select channel");
        let playing_now = tick.is_some_and(|tk| {
            pat.is_some_and(|p| {
                p.notes.iter().any(|n| {
                    let l = if n.length == 0 { per_step } else { n.length };
                    n.channel == ch.index && f64::from(n.position) <= tk && tk < f64::from(n.position + l)
                })
            })
        });
        ui.c.fill_rect(slot, Color::rgb(0x1f2224));
        let slot_fill = if playing_now && ch.enabled { t.accent.shade(-0.1) } else { Color::rgb(0x2b2d2f) };
        ui.c.fill_rect(slot.inset(1.0), slot_fill);
        if s.rack_selected == Some(i) {
            let fr = Rect::span(x + 199.0, x + 207.0, ry, ry + 26.0);
            ui.c.fill_rect(Rect::new(fr.x, fr.y, fr.w, 2.0), t.accent);
            ui.c.fill_rect(Rect::new(fr.x, fr.bottom() - 2.0, fr.w, 2.0), t.accent);
            ui.c.fill_rect(Rect::new(fr.x, fr.y, 1.0, fr.h), t.accent);
            ui.c.fill_rect(Rect::new(fr.right() - 1.0, fr.y, 1.0, fr.h), t.accent);
        }
        // Steps (§3), clipped to the step area.
        let steps_right = (x + STEP_X + steps_shown as f32 * STEP_W - 2.0).min(rows_clip.right());
        let step_area = Rect::span(x + STEP_X - 1.0, steps_right, rows_clip.y, rows_clip.bottom());
        let notes: &[os_model::Note] = pat.map_or(&[], |p| &p.notes);
        if s.rack_roll_view || !plain_steps(notes, ch.index) {
            let block = Rect::span(x + STEP_X - 2.0, steps_right + 2.0, ry - 2.0, ry + 28.0);
            mini_roll(ui, block, notes, ch.index, per_step, sx, focused);
            ui.hit(block.intersect(&rows_clip), Action::RackRoll(i), "Piano roll preview - click to open the Piano roll");
            continue;
        }
        ui.c.push_clip(step_area);
        for k in first_step..(first_step + steps_shown + 1) {
            let sr = Rect::new(x + STEP_X + STEP_W * k as f32 - sx, ry + 4.0, 13.0, 19.0);
            let on = pat.is_some_and(|p| p.notes.iter().any(|n| n.channel == ch.index && n.position == k * per_step));
            let hov = ui.hit(sr.intersect(&step_area), Action::Step(i, k), "Step - click to toggle");
            step_button(ui, sr, k, on, k >= len, hov);
        }
        ui.c.pop_clip();
    }
    ui.c.pop_clip();
    if vbar {
        let track = Rect::span(inner.right() - 10.0, inner.right(), inner.y, inner.bottom());
        super::scrollbar(ui, track, true, scroll, view_h, total_h, Scroll::RackY);
    }

    // Footer (§4).
    add_button(ui, x, y + h);
    let track = Rect::span(x + 208.0, x + w - 2.0, y + h - 13.0, y + h - 6.0);
    let view = steps_shown as f32 * STEP_W;
    super::scrollbar(ui, track, false, sx, view, scroll_steps as f32 * STEP_W, Scroll::RackX);
    // Redraw the bar in the rack's own look: flat track, thin dark thumb, end chevrons.
    ui.c.fill_rect(track, t.scrollbar);
    ui.c.fill_rect(Rect::new(track.x, track.y, 1.0, track.h), Color::rgb(0x181c1e));
    ui.c.fill_rect(Rect::new(track.right() - 1.0, track.y, 1.0, track.h), Color::rgb(0x181c1e));
    let total_px = scroll_steps as f32 * STEP_W;
    let tlen = (view / total_px * track.w).clamp(16.0, track.w);
    let max = (total_px - view).max(0.0);
    let off = if max > 0.0 { sx / max * (track.w - tlen) } else { 0.0 };
    ui.c.fill_rect(Rect::new(track.x + off + 6.0, track.y + 2.0, (tlen - 12.0).max(8.0), 3.0), Color::rgb(0x14181a));
    let chev = Color::rgb(0x3c4042);
    let cy = track.cy();
    ui.c.polyline(&[(x + 208.0, cy - 3.0), (x + 205.0, cy), (x + 208.0, cy + 3.0)], 1.3, chev);
    ui.c.polyline(&[(x + w - 23.0, cy - 3.0), (x + w - 20.0, cy), (x + w - 23.0, cy + 3.0)], 1.3, chev);

    // Resize zones: right edge, bottom edge, corner (§5).
    let (r, b) = (win.right(), win.bottom());
    ui.hit(Rect::span(r - 3.0, r + 4.0, y + 29.0, b - 8.0), Action::RackResize(EDGE_RIGHT), "Drag to resize");
    ui.hit(Rect::span(x, r - 8.0, b - 3.0, b + 4.0), Action::RackResize(EDGE_BOTTOM), "Drag to resize");
    ui.hit(Rect::span(r - 10.0, r + 4.0, b - 10.0, b + 4.0), Action::RackResize(EDGE_RIGHT | EDGE_BOTTOM), "Drag to resize");
}

/// Title bar contents (§2). Left items keep their x; the right group follows the right edge.
fn title_bar(ui: &mut Ui, win: Rect, roll_view: bool) {
    let t = ui.t;
    let (x, y, r) = (win.x, win.y, win.right());
    ui.hit(Rect::new(x + 8.0, y + 6.0, 16.0, 17.0), Action::Placeholder("channel rack options"), "Channel rack options");
    ui.c.fill_polygon(&[(x + 14.0, y + 10.5), (x + 18.5, y + 14.0), (x + 14.0, y + 17.5)], Color::rgb(0xc2c8cc));
    let sw = Rect::new(x + 34.0, y + 6.0, 16.0, 16.0);
    ui.hit(sw, Action::Placeholder("graph editor"), "Graph editor");
    icons::draw(ui.c, Icon::Countdown, sw, t.glyph);
    // Group combo "All".
    let combo = Rect::span(x + 107.0, x + 190.0, y + 4.0, y + 27.0);
    let chov = ui.hit(combo, Action::Placeholder("channel group"), "Channel group filter");
    ui.c.fill_rect(combo, Color::rgb(0x192227));
    let cf = combo.inset(1.0);
    ui.c.fill_rect(Rect::new(cf.x, cf.y, cf.w, 1.0), Color::rgb(0x303132));
    ui.c.fill_rect(Rect::new(cf.x, cf.y + 1.0, cf.w, cf.h / 2.0), Color::rgb(0x202122));
    ui.c.fill_rect(Rect::span(cf.x, cf.right(), cf.y + 1.0 + cf.h / 2.0, cf.bottom()), Color::rgb(0x1b1c1d));
    ui.c.text(Font::new(17), x + 113.0, y + 5.0, if chov { t.glyph.shade(0.1) } else { t.glyph }, "All");
    ui.c.fill_polygon(&[(x + 181.0, y + 11.5), (x + 185.0, y + 15.0), (x + 181.0, y + 18.5)], Color::rgb(0x686c6e));
    // Dotted separator.
    for k in 0..4 {
        ui.c.fill_rect(Rect::new(x + 204.0, y + 8.0 + 4.0 * k as f32, 2.0, 2.0), Color::rgb(0x4c4b47));
    }
    icons::draw(ui.c, Icon::Speaker, Rect::new(x + 215.0, y + 7.0, 15.0, 15.0), t.glyph);
    ui.c.text(Font::new(17), x + 236.0, y + 5.0, t.text, "Channel rack");
    // Swing knob with its halo.
    let kx = r - 149.5;
    ui.hit(Rect::new(kx - 12.0, y + 3.0, 24.0, 24.0), Action::Placeholder("swing"), "Swing");
    ui.c.fill_circle(kx, y + 14.5, 12.0, Color::rgb(0x212225));
    ui.c.fill_circle(kx, y + 14.5, 10.5, Color::rgb(0x2b2d2f));
    ball_knob(ui, kx, y + 14.5, 9.0, 0.0);
    // Step-count box.
    let bx = Rect::span(r - 132.0, r - 100.0, y + 4.0, y + 26.0);
    ui.hit(bx, Action::Placeholder("steps per bar"), "Pattern length");
    ui.c.fill_rrect(bx, 3.5, Color::rgb(0x191d1f));
    ui.c.fill_rect_gradient(bx.inset(1.0), Color::rgb(0x35393b), Color::rgb(0x2f3335), 3.0);
    for k in 0..3 {
        ui.c.fill_rect(Rect::new(r - 116.0 + 3.0 * k as f32, y + 14.0, 2.0, 1.0), Color::rgb(0xa7adb0));
    }
    spin_mark(ui, r - 104.0, y + 15.0, Color::rgb(0x6f7476));
    // Tool icons and close.
    let ur = Rect::new(r - 89.5, y + 6.0, 17.0, 17.0);
    let hov = ui.hit(ur, Action::Placeholder("channel rack undo"), "Undo step edits");
    icons::draw(ui.c, Icon::Undo, ur, if hov { Color::rgb(0xffffff) } else { t.glyph });
    // Graph bars and the step-sequencer blocks (§2), drawn as plain shapes.
    let gx = r - 58.0;
    let hov = ui.hit(Rect::new(gx - 9.0, y + 6.0, 18.0, 17.0), Action::Placeholder("graph editor"), "Graph editor");
    let gc = if hov { Color::rgb(0xffffff) } else { t.glyph };
    for (i, hgt) in [5.0, 9.0, 6.0, 11.0, 4.0].into_iter().enumerate() {
        ui.c.fill_rect(Rect::new(gx - 7.0 + 3.0 * i as f32, y + 20.0 - hgt, 2.0, hgt), gc);
    }
    ui.c.fill_rect(Rect::new(gx - 8.0, y + 20.0, 16.0, 1.0), gc.shade(-0.3));
    let sxc = r - 35.0;
    let br = Rect::new(sxc - 10.0, y + 5.0, 20.0, 20.0);
    let hov = ui.hit(br, Action::RackRollView, if roll_view { "Show steps" } else { "Show piano roll previews" });
    // On (§6): orange outline with a dark orange fill.
    if roll_view {
        ui.c.fill_rrect(br, 3.0, Color::rgb(0xc1923a));
        ui.c.fill_rrect(br.inset(1.0), 2.5, Color::rgb(0x574b30));
    }
    let oc = if hov { Color::rgb(0xffd77a) } else { Color::rgb(0xffc642) };
    for i in 0..3 {
        let b = Rect::new(sxc - 8.0 + 5.5 * i as f32, y + 9.0, 4.5, 11.0);
        ui.c.fill_rect(b, oc);
        ui.c.fill_rect(Rect::new(b.x + 0.5, b.y + 2.0, b.w - 1.0, 1.0), Color::rgb(0x71292b));
    }
    let close = Rect::new(r - 22.0, y + 6.0, 16.0, 16.0);
    let hov = ui.hit(close, Action::CloseWin(Win::ChannelRack), "Close");
    icons::draw(ui.c, Icon::Close, close.inset(1.5), if hov { Color::rgb(0xffffff) } else { t.glyph });
}

/// One step button (§3): bordered, lighter at the top, a dark dash below the top edge.
fn step_button(ui: &mut Ui, r: Rect, k: u32, on: bool, past_len: bool, hov: bool) {
    let t = ui.t;
    let odd = (k / 4) % 2 == 1;
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
    ui.c.fill_rect(Rect::new(f.x + 2.0, f.y + 2.0, f.w - 4.0, 2.0), face.shade(-0.55));
    ui.c.fill_rect(Rect::new(f.x, f.bottom() - 2.0, f.w, 2.0), face.shade(-0.35));
}

/// Mini piano roll of one channel (§6) in `block` (one 30 px row); `sx` is the step scroll in pixels.
fn mini_roll(ui: &mut Ui, block: Rect, notes: &[os_model::Note], ch: u16, per_step: u32, sx: f32, focused: bool) {
    // (separator, fill) for odd and even bars.
    let shades = if focused {
        [(Color::rgb(0x040709), Color::rgb(0x111416)), (Color::rgb(0x000002), Color::rgb(0x0a0d0f))]
    } else {
        [(Color::rgb(0x0a0d0f), Color::rgb(0x171a1c)), (Color::rgb(0x030608), Color::rgb(0x101315))]
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
        ui.c.fill_rect(Rect::new(nx, ny, nw, 1.0), Color::rgb(0x698a6f));
        ui.c.fill_rect(Rect::new(nx, ny + 1.0, nw, 3.0), Color::rgb(0x9ed1a5));
    }
    ui.c.pop_clip();
}

/// Row knob (§3): dark ball, r 7, light pointer from the centre to the rim.
fn row_knob(ui: &mut Ui, cx: f32, cy: f32, value: f32) {
    use std::f32::consts::PI;
    ui.c.fill_circle(cx, cy, 7.5, Color::rgb(0x1e2022));
    ui.c.fill_circle(cx, cy - 0.5, 6.5, Color::rgb(0x303234));
    ui.c.fill_circle(cx, cy + 0.3, 5.8, Color::rgb(0x232527));
    let a = 0.75 * PI + 1.5 * PI * value.clamp(0.0, 1.0);
    ui.c.line(cx + 1.2 * a.cos(), cy + 1.2 * a.sin(), cx + 5.6 * a.cos(), cy + 5.6 * a.sin(), 2.0, Color::rgb(0xadb2b6));
}

/// Enable LED (§3: ring r 5, green ball).
fn rack_led(ui: &mut Ui, cx: f32, cy: f32, on: bool) {
    ui.c.fill_circle(cx, cy, 5.0, Color::rgb(0x67696b));
    ui.c.fill_circle(cx, cy, 4.0, Color::rgb(0x384b39));
    if on {
        ui.c.fill_circle(cx, cy, 3.3, Color::rgb(0x3c7605));
        ui.c.fill_circle(cx, cy, 2.6, Color::rgb(0x97da3a));
        ui.c.fill_circle(cx, cy + 0.5, 1.6, Color::rgb(0xb7e66e));
        ui.c.fill_circle(cx - 0.8, cy - 1.0, 0.9, Color::rgb(0xedf7c4));
    } else {
        ui.c.fill_circle(cx, cy, 3.3, Color::rgb(0x1f2a14));
    }
}

/// The small up/down spin mark beside a number (§3).
fn spin_mark(ui: &mut Ui, x: f32, cy: f32, c: Color) {
    ui.c.polyline(&[(x - 1.5, cy - 6.0), (x + 1.0, cy - 4.0), (x - 1.5, cy - 2.0)], 1.0, c);
    ui.c.polyline(&[(x + 1.0, cy + 2.0), (x - 1.5, cy + 4.0), (x + 1.0, cy + 6.0)], 1.0, c);
}

/// The add-channel "+" (§4); `bottom` is the window's bottom edge.
fn add_button(ui: &mut Ui, x: f32, bottom: f32) {
    let t = ui.t;
    let area = Rect::span(x + 110.0, x + 190.0, bottom - 26.0, bottom - 5.0);
    let hov = ui.hit(area, Action::RackAdd, "Add a channel");
    let (cx, cy) = (x + 150.5, bottom - 15.5);
    let c = if hov { Color::rgb(0xffc63b) } else { Color::rgb(0xd4dade) };
    ui.c.fill_rect(Rect::new(cx - 4.5, cy - 1.0, 9.0, 2.0), c);
    ui.c.fill_rect(Rect::new(cx - 1.0, cy - 4.5, 2.0, 9.0), c);
    if hov {
        let pts = [(x + 111.0, bottom - 24.0), (x + 189.0, bottom - 24.0), (x + 181.0, bottom - 6.0), (x + 118.0, bottom - 6.0), (x + 111.0, bottom - 24.0)];
        ui.c.polyline(&pts, 2.0, t.add_hover);
    }
}

pub fn insert_name(track: u16) -> String {
    if track == 0 { "Master".into() } else { format!("Insert {track}") }
}

fn pan_text(p: f64) -> String {
    let v = (p * 100.0).round() as i32;
    match v {
        0 => "centered".into(),
        v if v < 0 => format!("{}% left", -v),
        v => format!("{v}% right"),
    }
}
