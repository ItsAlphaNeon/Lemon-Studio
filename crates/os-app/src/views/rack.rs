//! Floating channel rack.
// spec: ui/main-window.md §8

use os_ui::{Color, Rect};

use super::{ball_knob, led, rgb};
use crate::app::{Action, State, Ui};
use crate::icons::{self, Icon};

pub const ROW_H: f32 = 30.0;

/// Outer rectangle of the floating rack (with its shadow margin).
pub fn bounds(s: &State) -> Rect {
    let rows = s.project.channels.len().clamp(1, 12);
    Rect::new(s.rack_pos.0 - 6.0, s.rack_pos.1 - 6.0, 510.0, 52.0 + rows as f32 * ROW_H + 41.0 + 12.0)
}

pub fn paint(ui: &mut Ui, s: &State) {
    let t = ui.t;
    let (x, y) = s.rack_pos;
    let rows = s.project.channels.len().clamp(1, 12);
    let h = 52.0 + rows as f32 * ROW_H + 41.0;
    let win = Rect::new(x, y, 498.0, h);
    ui.c.fill_rect(win, t.splitter_dark);
    ui.hit(win, Action::RackArea, "Channel rack");
    let body = win.inset(1.0);
    ui.c.fill_rect(body, t.window_body);
    // Title bar (drag to move).
    let bar = Rect::new(body.x, body.y, body.w, 27.0);
    ui.c.fill_rect_gradient(bar, t.window_title_float.shade(0.05), t.window_title_float.shade(-0.03), 0.0);
    ui.hit(bar, Action::RackDrag, "Drag to move");
    icons::draw(ui.c, Icon::ArrowRight, Rect::new(x + 10.0, y + 6.0, 14.0, 14.0), t.text_dim);
    let grp = Rect::span(x + 108.0, x + 183.0, y + 8.0, y + 20.0);
    ui.c.fill_rrect(grp, 2.0, t.sunken.shade(-0.1));
    ui.c.text(os_ui::Font::new(14), grp.x + 5.0, grp.y - 2.0, t.text, "All");
    ui.c.fill_polygon(&[(grp.right() - 8.0, grp.y + 3.0), (grp.right() - 4.0, grp.cy()), (grp.right() - 8.0, grp.bottom() - 3.0)], t.glyph_dim);
    icons::draw(ui.c, Icon::Speaker, Rect::new(x + 214.0, y + 6.0, 16.0, 16.0), t.text);
    ui.c.text(os_ui::Font::new(16), x + 236.0, y + 3.0, t.text, "Channel rack");
    ball_knob(ui, x + 350.0, y + 14.0, 8.5, 0.5);
    for (i, icon) in [Icon::Back, Icon::Mixer, Icon::PianoRoll].into_iter().enumerate() {
        icons::draw(ui.c, icon, Rect::new(x + 407.0 + 22.0 * i as f32, y + 6.0, 16.0, 16.0), t.text);
    }
    let close = Rect::new(x + 475.0, y + 6.0, 16.0, 16.0);
    let hov = ui.hit(close, Action::CloseRack, "Close");
    icons::draw(ui.c, Icon::Close, close, if hov { t.glyph } else { t.text_dim });

    // Rows live in an inset panel; the footer (add button, scroll bar) sits below it.
    let list = Rect::new(body.x, bar.bottom(), body.w, h - 27.0 - 36.0);
    let inset = Rect::new(body.x + 4.0, bar.bottom() + 2.0, body.w - 8.0, h - 27.0 - 36.0 - 2.0);
    ui.c.fill_rect(inset.outset(1.0), t.splitter_dark.shade(-0.1));
    ui.c.fill_rect_gradient(inset, t.rack_inset.shade(-0.03), t.rack_inset.shade(0.015), 0.0);
    ui.c.push_clip(list);
    let first = (s.rack_scroll / ROW_H) as usize;
    for i in first..(first + rows + 1).min(s.project.channels.len()) {
        let ch = &s.project.channels[i];
        let ry = list.y + 19.0 + i as f32 * ROW_H - s.rack_scroll;
        let selected = i == s.selected_channel;
        led_at(ui, x + 17.0, ry + 11.0, ch.enabled, i);
        ball_knob(ui, x + 35.0, ry + 11.0, 7.5, 0.5 + ch.pan as f32 / 2.0);
        ball_knob(ui, x + 53.0, ry + 11.0, 7.5, (ch.volume as f32).clamp(0.0, 1.0));
        let mb = Rect::span(x + 67.0, x + 98.0, ry + 6.0, ry + 24.0);
        ui.c.fill_rrect(mb, 3.0, t.splitter_dark.shade(-0.1));
        ui.c.fill_rect_gradient(mb.inset(1.0), t.rack_row_box.shade(-0.1), t.rack_row_box, 2.5);
        ui.c.text_center(crate::views::fnt(17.0), mb, t.text_dim.shade(0.1), &format!("{}", ch.mixer_track));
        let nb = Rect::span(x + 109.0, x + 189.0, ry + 4.0, ry + 26.0);
        let nhov = ui.hit(nb, Action::ChannelSelect(i), "Select channel");
        let face = if selected { t.name_button_sel } else { t.name_button };
        ui.c.fill_rrect(nb.outset(1.0), 3.5, t.splitter_dark.shade(-0.1));
        ui.c.fill_rect_gradient(nb, face.shade(0.06), face.shade(-0.06), 3.0);
        let col = ch.color.map(rgb).map_or(t.name_text, |c| c.mix(t.name_text, 0.55));
        let label = ch.name.clone().unwrap_or_else(|| format!("Channel {}", ch.index + 1));
        ui.c.push_clip(nb.inset(1.0));
        ui.c.text_center(os_ui::Font::new(13), nb, if nhov { col.shade(0.3) } else { col }, &label);
        ui.c.pop_clip();
        let sel = Rect::span(x + 200.0, x + 204.0, ry + 5.0, ry + 24.0);
        ui.c.fill_rrect(sel, 1.5, t.splitter_dark.shade(-0.1));
        if selected {
            ui.c.stroke_rrect(sel, 1.5, 1.5, t.accent);
        }
        // Steps (§8: 16 steps, groups of 4).
        let ppq = u32::from(s.project.ppq.max(1));
        let pat = s.project.patterns.iter().find(|p| p.number == s.pattern);
        for k in 0..16u32 {
            let sr = Rect::new(x + 217.0 + 16.0 * k as f32, ry + 5.0, 14.0, 20.0);
            let on = pat.is_some_and(|p| p.notes.iter().any(|n| n.channel == ch.index && n.position == k * ppq / 4));
            let hov = ui.hit(sr, Action::Step(i, k), "Toggle step");
            let base = if (k / 4) % 2 == 0 { t.step_a } else { t.step_b };
            let c = if on { t.step_on } else if hov { base.shade(0.15) } else { base };
            ui.c.fill_rrect(sr.outset(1.0), 3.0, t.splitter_dark.shade(-0.15));
            ui.c.fill_rect_gradient(sr, c.shade(0.1), c.shade(-0.1), 2.5);
            ui.c.fill_rect(Rect::new(sr.x + 3.0, sr.y + 2.0, sr.w - 6.0, 1.0), Color::rgb(0xffffff).with_alpha(if on { 120 } else { 30 }));
            ui.c.fill_rect(Rect::new(sr.x + 3.0, sr.y + 4.0, sr.w - 6.0, 2.0), if on { t.accent } else { Color::rgb(0x000000).with_alpha(60) });
        }
    }
    ui.c.pop_clip();
    // Add channel.
    icons::draw(ui.c, Icon::Plus, Rect::new(x + 459.0 - 250.0, y + h - 24.0, 16.0, 16.0), t.text_dim);
    ui.hit(Rect::new(x + 200.0, y + h - 28.0, 60.0, 24.0), Action::Placeholder("add channel"), "Add a channel");
    ui.c.fill_rect(Rect::new(x + 203.0, y + h - 9.0, 280.0, 3.0), t.sunken);
}

fn led_at(ui: &mut Ui, cx: f32, cy: f32, on: bool, i: usize) {
    ui.hit(Rect::new(cx - 7.0, cy - 7.0, 14.0, 14.0), Action::ChannelEnable(i), "Enable / disable channel");
    led(ui, cx, cy, on);
}
