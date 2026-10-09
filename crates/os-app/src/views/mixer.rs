//! Mixer window (wide layout: strips plus the selected track's slot panel).
// spec: ui/main-window.md §7; fader law: audio/gain-laws.md (0 dB at fader 0.8)

use os_ui::{Face, Rect};

use super::{knob, led, window_frame, FrameInfo};
use crate::app::{Action, State, Ui};
use crate::icons::{self, Icon};

pub const STRIP_W: f32 = 44.0;
/// Fader travel: y 605 (top, 0 dB handle) to ~1000; value units per pixel.
pub const FADER_PER_PX: f64 = 1.0 / 460.0;

pub fn fader_db_text(v: f64) -> String {
    let gain = os_engine::laws::fader_gain(v);
    if gain <= 0.0 { "-inf dB".into() } else { format!("{:+.1} dB", 20.0 * gain.log10()) }
}

pub fn paint(ui: &mut Ui, s: &State, r: Rect, f: &FrameInfo) {
    let t = ui.t;
    let client = window_frame(ui, r, "Mixer - Insert", 150.0);
    for (i, icon) in [Icon::ArrowRight, Icon::Mixer, Icon::Mixer].into_iter().enumerate() {
        icons::draw(ui.c, icon, Rect::new(r.x + 8.0 + 22.0 * i as f32, r.y + 4.0, 16.0, 16.0), t.text);
    }
    ui.c.text(Face::Ui, 12.0, r.x + 114.0, r.y + 6.0, t.text, "Wide");

    let panel_w = 220.0;
    let strips = Rect::new(r.x, client.y, r.w - panel_w, client.h);
    let panel = Rect::new(strips.right(), r.y, panel_w, r.h);
    ui.c.fill_rect(strips, t.mixer_bg);
    ui.hit(strips, Action::MixerArea, "Mixer");

    // Level scale (dB marks).
    let scale = Rect::new(r.x + 9.0, client.y + 23.0, 28.0, 400.0);
    for (i, m) in ["3", "0", "3", "6", "9", "12", "15", "18", "21", "24", "27", "30", "33", "36", "39"].iter().enumerate() {
        ui.c.text_center(Face::Ui, 9.0, Rect::new(scale.x, scale.y + 1.0 + i as f32 * 26.0, scale.w, 12.0), t.text_faint, m);
    }

    // Strips: Master (index 0) first, then inserts.
    ui.c.push_clip(Rect::new(r.x + 36.0, client.y, strips.w - 36.0, client.h));
    let first = (s.mixer_scroll / STRIP_W) as usize;
    let count = ((strips.w - 36.0) / STRIP_W).ceil() as usize + 2;
    for n in first..(first + count).min(127) {
        let cx = if n == 0 { r.x + 76.0 } else { r.x + 130.0 + STRIP_W * (n - 1) as f32 } - s.mixer_scroll.min(if n == 0 { 0.0 } else { f32::MAX });
        strip(ui, s, n as u16, cx, client.y, r.bottom(), f);
    }
    ui.c.pop_clip();

    slot_panel(ui, s, panel);
}

fn strip(ui: &mut Ui, s: &State, n: u16, cx: f32, top: f32, bottom: f32, f: &FrameInfo) {
    let t = ui.t;
    let track = s.project.mixer.iter().find(|m| m.index == n);
    let selected = s.selected_mixer == n;
    let body = Rect::new(cx - 15.0, top + 23.0, 30.0, bottom - top - 40.0);
    // Number header.
    let hdr = if n == 0 { "M".to_string() } else { n.to_string() };
    ui.c.text_center(Face::Ui, 10.0, Rect::new(cx - 15.0, top + 2.0, 30.0, 14.0), t.text_faint, &hdr);
    ui.hit(Rect::new(cx - 20.0, top, 40.0, 400.0), Action::MixerSelect(n), "Select mixer track");
    ui.c.fill_rrect(body, 2.0, t.mixer_strip);
    // Peak meter at the strip top.
    let meter = Rect::new(cx - 11.0, body.y + 4.0, 22.0, 322.0);
    ui.c.fill_rect(meter, t.mixer_strip_dark);
    if n == 0 && f.playing {
        let (pl, pr) = f.peaks;
        for (i, p) in [pl, pr].into_iter().enumerate() {
            let h = ((1.0 + 20.0 * p.max(1e-6).log10() / 48.0).clamp(0.0, 1.0)) * meter.h;
            let c = if p >= 1.0 { t.meter_clip } else if p > 0.5 { t.meter_high } else { t.meter_low };
            ui.c.fill_rect(Rect::new(meter.x + 2.0 + i as f32 * 10.0, meter.bottom() - h, 8.0, h), c);
        }
    }
    // Name, rotated.
    let name = track
        .and_then(|m| m.name.clone())
        .unwrap_or_else(|| if n == 0 { "Master".into() } else { format!("Insert {n}") });
    let name_c = track.and_then(|m| m.color).map(super::rgb).unwrap_or(t.text_dim.shade(0.1));
    ui.c.push_clip(Rect::new(cx - 14.0, top + 25.0, 28.0, 330.0));
    ui.c.text_vertical(Face::Ui, 12.0, cx - 24.0, top + 345.0, name_c, &name);
    ui.c.pop_clip();
    let enabled = track.is_none_or(|m| m.enabled);
    led(ui, cx, top + 384.0, enabled);
    // Pan knob and separation.
    let pan = track.and_then(|m| m.pan).unwrap_or(0.0) as f32;
    knob(ui, cx, top + 412.0, 11.0, (pan + 1.0) / 2.0, true, t.glyph, t.button_hi);
    icons::draw(ui.c, Icon::Slip, Rect::new(cx - 7.0, top + 437.0, 14.0, 10.0), t.glyph_dim);
    // Fader.
    let v = track.and_then(|m| m.volume).unwrap_or(0.8);
    let well = Rect::new(cx - 2.0, top + 495.0, 4.0, 400.0);
    ui.c.fill_rect(well, t.mixer_strip_dark);
    let handle_y = top + 499.0 + (0.8 - v as f32) * 460.0;
    let handle = Rect::new(cx - 7.0, handle_y.clamp(top + 440.0, top + 900.0), 14.0, 22.0);
    let hov = ui.hit(Rect::new(cx - 14.0, top + 450.0, 28.0, 470.0), Action::Fader(n), "Volume (drag)");
    let hc = if selected { t.accent } else if hov { t.fader_handle.shade(0.2) } else { t.fader_handle };
    ui.c.fill_rrect(handle, 2.0, hc);
    ui.c.line(handle.x + 2.0, handle.cy(), handle.right() - 2.0, handle.cy(), 1.0, t.mixer_strip_dark);
    // Send arrow.
    if n != 0 {
        ui.c.fill_polygon(&[(cx - 4.0, top + 960.0 + 6.0), (cx, top + 960.0), (cx + 4.0, top + 960.0 + 6.0)], t.glyph_dim);
    }
    if selected {
        ui.c.stroke_rrect(Rect::new(cx - 17.0, top + 21.0, 34.0, 325.0), 2.0, 2.0, t.accent);
    }
}

fn slot_panel(ui: &mut Ui, s: &State, r: Rect) {
    let t = ui.t;
    ui.c.fill_rect(r, t.mixer_panel);
    ui.hit(r, Action::Swallow, "");
    let n = s.selected_mixer;
    let track = s.project.mixer.iter().find(|m| m.index == n);
    let title = match track.and_then(|m| m.name.clone()) {
        Some(nm) => format!("Mixer - {nm}"),
        None if n == 0 => "Mixer - Master".into(),
        None => format!("Mixer - Insert {n}"),
    };
    ui.c.fill_rect(Rect::new(r.x, r.y, r.w, 23.0), t.window_title);
    ui.c.text(Face::Ui, 12.0, r.x + 4.0, r.y + 5.0, t.text, &title);
    // Input selector.
    let inp = Rect::new(r.x + 6.0, r.y + 30.0, 185.0, 19.0);
    ui.c.fill_rrect(inp, 2.0, t.slot);
    ui.c.text(Face::Ui, 12.0, inp.x + 24.0, inp.y + 3.0, t.text, "(none)");
    // Slots.
    for i in 0..10 {
        let y = r.y + 61.0 + 33.0 * i as f32;
        let row = Rect::new(r.x + 4.0, y, 194.0, 28.0);
        let plugin = track.and_then(|m| m.slots[i].as_ref());
        let hov = ui.hit(row, Action::Placeholder("effect slot"), "Effect slot");
        ui.c.fill_rrect(row, 2.0, if hov { t.slot.shade(0.05) } else { t.slot });
        ui.c.fill_polygon(&[(row.x + 6.0, row.y + 9.0), (row.x + 10.0, row.y + 14.0), (row.x + 6.0, row.y + 19.0)], t.slot_text);
        match plugin {
            Some(p) => ui.c.text(Face::Ui, 13.0, row.x + 16.0, row.y + 7.0, t.slot_text_used, &p.name),
            None => ui.c.text(Face::Ui, 13.0, row.x + 16.0, row.y + 7.0, t.slot_text, &format!("Slot {}", i + 1)),
        };
        knob(ui, row.right() - 14.0, row.cy(), 9.0, 1.0, false, t.glyph_dim, t.button_hi);
        led(ui, r.right() - 11.0, row.cy(), plugin.is_some());
    }
    // EQ area and label.
    ui.c.fill_rect(Rect::new(r.x + 4.0, r.y + 403.0, 148.0, 170.0), t.mixer_panel.shade(-0.12));
    ui.c.text(Face::Ui, 13.0, r.x + 4.0, r.y + 595.0, t.text_dim, "Equalizer");
    for k in 0..3 {
        knob(ui, r.right() - 62.0 + k as f32 * 22.0, r.y + 603.0, 7.0, 0.5, true, t.glyph_dim, t.button_hi);
    }
    // Output selectors.
    for (i, label) in ["(none)", "(none)"].into_iter().enumerate() {
        let b = Rect::new(r.x + 6.0, r.bottom() - 58.0 + 33.0 * i as f32, 205.0, 17.0);
        ui.c.fill_rrect(b, 2.0, t.slot);
        ui.c.text(Face::Ui, 12.0, b.x + 22.0, b.y + 2.0, t.text, label);
    }
}
