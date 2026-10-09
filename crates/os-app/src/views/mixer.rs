//! Mixer window (wide layout: strips plus the selected track's slot panel).
// spec: ui/main-window.md §7; fader law: audio/gain-laws.md (0 dB at fader 0.8)

use os_ui::{Color, Face, Rect};

use super::{ball_knob, led, raised, window_frame, FrameInfo};
use crate::app::{Action, State, Ui};
use crate::icons::{self, Icon};

pub const STRIP_W: f32 = 44.0;
/// Fader travel: value units per pixel (0 dB handle at value 0.8).
pub const FADER_PER_PX: f64 = 1.0 / 460.0;

// Vertical layout, absolute y at the default 1080p window (spec §7).
const NUM_Y: f32 = 116.0; // header numbers (centre)
const METER_Y0: f32 = 127.0;
const METER_Y1: f32 = 453.0;
const LED_Y: f32 = 467.0;
const PAN_Y: f32 = 497.0;
const SEP1_Y: f32 = 525.0;
const SEP2_Y: f32 = 542.0;
const RING_Y: f32 = 562.0;
const HANDLE_Y0DB: f32 = 605.0; // top of the handle at 0 dB
const FADER_Y0: f32 = 520.0;
const FADER_Y1: f32 = 981.0;

/// dB marks of the level scale and their measured y positions.
const SCALE: [(&str, f32); 17] = [
    ("3", 181.0), ("0", 243.0), ("3", 303.0), ("6", 362.0), ("9", 421.0), ("12", 481.0), ("15", 537.0), ("18", 590.0),
    ("21", 641.0), ("24", 688.0), ("27", 730.0), ("30", 768.0), ("33", 801.0), ("36", 828.0), ("39", 850.0), ("42", 868.0),
    ("45", 882.0),
];

pub fn fader_db_text(v: f64) -> String {
    let gain = os_engine::laws::fader_gain(v);
    if gain <= 0.0 { "-inf dB".into() } else { format!("{:+.1} dB", 20.0 * gain.log10()) }
}

pub fn paint(ui: &mut Ui, s: &State, r: Rect, f: &FrameInfo) {
    let t = ui.t;
    let client = window_frame(ui, r, "", 0.0);
    for (i, icon) in [Icon::ArrowRight, Icon::Mixer, Icon::Mixer].into_iter().enumerate() {
        icons::draw(ui.c, icon, Rect::new(r.x + 8.0 + 22.0 * i as f32, r.y + 4.0, 16.0, 16.0), t.text);
    }
    ui.c.text(Face::Ui, 13.0, r.x + 134.0, r.y + 5.0, t.text, "Wide");

    let panel_w = 220.0;
    let strips = Rect::new(r.x, client.y, r.w - panel_w, client.h);
    let panel = Rect::new(strips.right(), r.y, panel_w, r.h);
    ui.c.fill_rect(strips, t.mixer_bg);
    ui.hit(strips, Action::MixerArea, "Mixer");

    // Level scale on the left (C header above it).
    ui.c.text_center(Face::Ui, 10.0, Rect::new(r.x + 12.0, NUM_Y - 7.0, 20.0, 14.0), t.text_faint, "C");
    ui.c.fill_rect(Rect::new(r.x + 9.0, METER_Y0, 24.0, 900.0 - METER_Y0), t.mixer_strip_dark.shade(0.05));
    for (m, y) in SCALE {
        ui.c.text_center(Face::Ui, 9.5, Rect::new(r.x + 9.0, y - 7.0, 24.0, 14.0), t.text_faint.shade(0.1), m);
    }

    // Strips: Master (index 0) is fixed; inserts scroll.
    let area = Rect::new(r.x + 54.0, client.y, strips.w - 54.0, client.h);
    ui.c.push_clip(area);
    let first = ((s.mixer_scroll / STRIP_W) as usize).saturating_sub(1);
    let count = (area.w / STRIP_W).ceil() as usize + 3;
    for n in (first + 1..first + 1 + count).filter(|n| *n < 127) {
        let cx = r.x + 130.0 + STRIP_W * (n - 1) as f32 - s.mixer_scroll;
        strip(ui, s, n as u16, cx, r.bottom(), f);
    }
    ui.c.pop_clip();
    ui.c.fill_rect(Rect::new(r.x + 52.0, client.y, 2.0, client.h), t.mixer_bg.shade(-0.35));
    strip(ui, s, 0, r.x + 76.0, r.bottom(), f);

    slot_panel(ui, s, panel);
}

fn strip(ui: &mut Ui, s: &State, n: u16, cx: f32, bottom: f32, f: &FrameInfo) {
    let t = ui.t;
    let track = s.project.mixer.iter().find(|m| m.index == n);
    let selected = s.selected_mixer == n;
    let hdr = if n == 0 { "M".to_string() } else { n.to_string() };
    ui.c.text_center(Face::Ui, 10.0, Rect::new(cx - 20.0, NUM_Y - 7.0, 20.0, 14.0), if selected { t.text } else { t.text_faint }, &hdr);
    ui.hit(Rect::new(cx - 22.0, 107.0, 44.0, 400.0), Action::MixerSelect(n), "Select mixer track");

    // Strip body: faint panel with a divider on its right edge.
    ui.c.fill_rect_gradient(Rect::new(cx - 22.0, 107.0, 44.0, bottom - 107.0), t.mixer_bg.shade(0.015), t.mixer_bg.shade(-0.02), 0.0);
    ui.c.fill_rect(Rect::new(cx + 21.0, 107.0, 1.0, bottom - 107.0), t.mixer_bg.shade(-0.3));

    // Meter slot (14 px wide), with a faint centre line; the selected strip gets the green frame.
    let meter = Rect::new(cx - 7.0, METER_Y0, 14.0, METER_Y1 - METER_Y0);
    ui.c.fill_rect_gradient(meter, t.mixer_strip_dark.shade(-0.08), t.mixer_strip_dark.shade(0.03), 0.0);
    ui.c.fill_rect(Rect::new(meter.x, meter.y, 1.0, meter.h), Color::rgb(0x000000).with_alpha(60));
    ui.c.fill_rect(Rect::new(cx - 0.5, meter.y, 1.0, meter.h), Color::rgb(0xffffff).with_alpha(7));
    if n == 0 && f.playing {
        let (pl, pr) = f.peaks;
        for (i, p) in [pl, pr].into_iter().enumerate() {
            let h = ((1.0 + 20.0 * p.max(1e-6).log10() / 48.0).clamp(0.0, 1.0)) * meter.h;
            let c = if p >= 1.0 { t.meter_clip } else if p > 0.5 { t.meter_high } else { t.meter_low };
            ui.c.fill_rect(Rect::new(meter.x + 1.0 + i as f32 * 6.0, meter.bottom() - h, 5.0, h), c);
        }
    }
    if selected {
        ui.c.stroke_rrect(meter.inset(-1.0), 2.0, 2.0, t.accent);
    }

    // Name, rotated, to the left of the meter.
    let name = track
        .and_then(|m| m.name.clone())
        .unwrap_or_else(|| if n == 0 { "Master".into() } else { format!("Insert {n}") });
    let name_c = track
        .and_then(|m| m.color)
        .map(super::rgb)
        .map_or(if selected { t.text } else { t.text_dim.shade(0.1) }, |c| c.mix(t.text, 0.4));
    ui.c.push_clip(Rect::new(cx - 22.0, 380.0, 15.0, 74.0));
    ui.c.text_vertical(Face::Ui, 12.0, cx - 19.0, 452.0, name_c, &name);
    ui.c.pop_clip();

    let enabled = track.is_none_or(|m| m.enabled);
    led(ui, cx, LED_Y, enabled);
    let pan = track.and_then(|m| m.pan).unwrap_or(0.0) as f32;
    ui.hit(Rect::new(cx - 14.0, PAN_Y - 14.0, 28.0, 28.0), Action::Placeholder("pan"), "Pan");
    ball_knob(ui, cx, PAN_Y, 13.5, (pan + 1.0) / 2.0);
    // Stereo separation and swap glyphs, then the route ring.
    let g = t.glyph.shade(-0.1);
    ui.c.fill_polygon(&[(cx - 3.5, SEP1_Y - 1.5), (cx, SEP1_Y - 5.5), (cx + 3.5, SEP1_Y - 1.5)], g);
    ui.c.fill_polygon(&[(cx - 3.5, SEP1_Y + 1.5), (cx, SEP1_Y + 5.5), (cx + 3.5, SEP1_Y + 1.5)], g);
    ui.c.fill_polygon(&[(cx - 1.5, SEP2_Y - 3.5), (cx - 5.5, SEP2_Y), (cx - 1.5, SEP2_Y + 3.5)], g);
    ui.c.fill_polygon(&[(cx + 1.5, SEP2_Y - 3.5), (cx + 5.5, SEP2_Y), (cx + 1.5, SEP2_Y + 3.5)], g);
    ui.c.fill_circle(cx, RING_Y, 6.0, t.sunken_dark.shade(-0.2));
    ui.c.stroke_circle(cx, RING_Y, 6.0, 1.0, t.button_hi);

    // Fader: slot, travel line, handle.
    let v = track.and_then(|m| m.volume).unwrap_or(0.8) as f32;
    ui.c.fill_rect(Rect::new(cx - 2.0, FADER_Y0, 4.0, FADER_Y1 - FADER_Y0), t.mixer_strip_dark.shade(-0.12));
    let hy = (HANDLE_Y0DB + (0.8 - v) * 460.0).clamp(FADER_Y0 - 20.0, FADER_Y1 - 12.0);
    if selected {
        ui.c.fill_rect(Rect::new(cx - 1.0, hy + 12.0, 2.0, FADER_Y1 - hy - 12.0), t.accent.shade(-0.1));
    }
    let hov = ui.hit(Rect::new(cx - 14.0, FADER_Y0 - 20.0, 28.0, FADER_Y1 - FADER_Y0 + 30.0), Action::Fader(n), "Volume (drag)");
    let handle = Rect::new(cx - 7.0, hy, 14.0, 24.0);
    let base = if selected { t.accent } else if hov { t.fader_handle.shade(0.2) } else { t.fader_handle };
    ui.c.fill_rect(handle.inset(-1.0), Color::rgb(0x000000).with_alpha(110));
    ui.c.fill_rect_gradient(handle, base.shade(0.18), base.shade(-0.18), 2.5);
    ui.c.fill_rect(Rect::new(handle.x + 2.0, handle.cy() - 0.5, handle.w - 4.0, 1.0), Color::rgb(0x000000).with_alpha(110));
    ui.c.fill_rect(Rect::new(handle.x + 2.0, handle.cy() + 0.5, handle.w - 4.0, 1.0), Color::rgb(0xffffff).with_alpha(40));

    // Routing (pin, ring, dot) and the send arrow.
    let by = bottom - 138.0;
    ui.c.fill_polygon(&[(cx - 3.5, by + 2.0), (cx + 3.5, by + 2.0), (cx, by + 8.0)], t.glyph_dim);
    ui.c.stroke_circle(cx, by + 21.0, 5.5, 1.3, t.glyph_dim);
    ui.c.fill_circle(cx, by + 40.0, 2.0, t.glyph_dim.shade(-0.2));
    if n != 0 {
        let ay = bottom - 42.0;
        let col = if selected { t.accent } else { t.glyph_dim.shade(0.15) };
        ui.c.fill_polygon(&[(cx - 4.5, ay + 6.0), (cx, ay), (cx + 4.5, ay + 6.0)], col);
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
    ui.c.fill_rect_gradient(Rect::new(r.x, r.y, r.w, 23.0), t.window_title.shade(0.04), t.window_title.shade(-0.03), 0.0);
    ui.c.text(Face::Ui, 12.0, r.x + 4.0, r.y + 5.0, t.text, &title);
    for (i, icon) in [Icon::Minimize, Icon::Restore, Icon::Close].into_iter().enumerate() {
        icons::draw(ui.c, icon, Rect::new(r.right() - 58.0 + 19.0 * i as f32, r.y + 3.0, 17.0, 17.0), t.text_dim.shade(0.2));
    }
    // Input selector.
    let inp = Rect::new(r.x + 6.0, r.y + 30.0, 185.0, 19.0);
    ui.c.fill_rrect(inp, 2.0, t.slot.shade(-0.1));
    ui.c.fill_rect(Rect::new(inp.x + 2.0, inp.y, inp.w - 4.0, 1.0), Color::rgb(0x000000).with_alpha(60));
    ui.c.text(Face::Ui, 12.5, inp.x + 22.0, inp.y + 3.0, t.text, "(none)");
    icons::draw(ui.c, Icon::ArrowRight, Rect::new(inp.right() - 14.0, inp.y + 4.0, 11.0, 11.0), t.glyph_dim);
    // Slots.
    for i in 0..10 {
        let y = r.y + 61.0 + 33.0 * i as f32;
        let row = Rect::new(r.x + 4.0, y, 194.0, 28.0);
        let plugin = track.and_then(|m| m.slots[i].as_ref());
        let hov = ui.hit(row, Action::Placeholder("effect slot"), "Effect slot");
        raised(ui, row, if hov { t.slot.shade(0.06) } else { t.slot }, 2.5);
        ui.c.fill_polygon(&[(row.x + 6.0, row.y + 9.0), (row.x + 10.0, row.y + 14.0), (row.x + 6.0, row.y + 19.0)], t.slot_text);
        match plugin {
            Some(p) => ui.c.text(Face::Ui, 13.5, row.x + 16.0, row.y + 6.0, t.slot_text_used, &p.name),
            None => ui.c.text(Face::Ui, 13.5, row.x + 16.0, row.y + 6.0, t.slot_text, &format!("Slot {}", i + 1)),
        };
        ball_knob(ui, row.right() - 14.0, row.cy(), 10.5, 1.0);
        led(ui, r.right() - 11.0, row.cy(), plugin.is_some());
    }
    // EQ area and label.
    let eq = Rect::new(r.x + 4.0, r.y + 403.0, 148.0, 170.0);
    ui.c.fill_rect_gradient(eq, t.mixer_panel.shade(-0.2), t.mixer_panel.shade(-0.1), 0.0);
    ui.c.fill_rect(Rect::new(eq.x, eq.y + 85.0, eq.w, 1.0), Color::rgb(0xffffff).with_alpha(60));
    ui.c.text(Face::Ui, 13.0, r.x + 4.0, r.y + 595.0, t.text_dim, "Equalizer");
    for k in 0..3 {
        ball_knob(ui, r.right() - 62.0 + k as f32 * 22.0, r.y + 603.0, 7.5, 0.5);
    }
    // Output selectors.
    for (i, label) in ["(none)", "(none)"].into_iter().enumerate() {
        let b = Rect::new(r.x + 6.0, r.bottom() - 58.0 + 33.0 * i as f32, 205.0, 17.0);
        ui.c.fill_rrect(b, 2.0, t.slot.shade(-0.1));
        ui.c.text(Face::Ui, 12.5, b.x + 22.0, b.y + 2.0, t.text, label);
    }
}
