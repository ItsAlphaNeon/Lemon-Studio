//! Mixer window (wide layout: strips plus the selected track's slot panel).
// spec: ui/main-window.md §7; fader law: audio/gain-laws.md (0 dB at fader 0.8)

use std::f32::consts::PI;

use os_ui::{Color, Font, Glyph, Rect};

use super::{led, resize_zones, window_body, window_buttons, FrameInfo};
use crate::app::{Action, State, Ui, Win, TITLE_H};
use crate::icons::{self, Icon};

pub const STRIP_W: f32 = 44.0;
/// Fader travel: value units per pixel (handle centre 618 at 0 dB = value 0.8, 970 at value 0).
pub const FADER_PER_PX: f64 = 1.0 / 440.0;

// Vertical layout at the default 1080p window (spec §7): absolute y for a window top of 83;
// bottom-anchored values are offsets from the window bottom.
const HEAD_Y0: f32 = 107.0; // header numbers row
const HEAD_Y1: f32 = 127.0;
const METER_Y0: f32 = 130.0;
const METER_Y1: f32 = 453.0;
const PANEL_Y0: f32 = 128.0; // dark head panel behind meter, name, LED, pan
const PANEL_Y1: f32 = 508.0;
const LED_Y: f32 = 467.0;
const PAN_Y: f32 = 496.5;
const SEP_Y: f32 = 527.0;
const SWAP_Y: f32 = 545.0;
const RING_Y: f32 = 563.0;
const TRACK_Y0: f32 = 525.0;
const HANDLE_0DB: f32 = 618.0; // handle centre at 0 dB (value 0.8)

/// dB marks of the level scale and their measured y positions (number centres).
const SCALE: [(&str, f32); 17] = [
    ("3", 181.0), ("0", 241.0), ("3", 301.0), ("6", 360.0), ("9", 419.0), ("12", 479.0), ("15", 535.0), ("18", 588.0),
    ("21", 639.0), ("24", 686.0), ("27", 728.0), ("30", 766.0), ("33", 799.0), ("36", 826.0), ("39", 848.0), ("42", 866.0),
    ("45", 880.0),
];

/// Height of the mixer window the absolute y values above were measured at (1080p workspace).
const REF_H: f32 = 997.0;
/// Reference y of the window top.
const REF_TOP: f32 = 83.0;
/// Height of the parts that do not scale: header, head-panel controls and the bottom controls.
const FIXED_H: f32 = 218.0;

/// How much the mixer's stretchy parts (meter, level scale, fader) are scaled at window `r`:
/// 1 at the measured height; the rest of the strip keeps its size.
pub fn scale(r: Rect) -> f32 {
    ((r.h - FIXED_H) / (REF_H - FIXED_H)).max(0.2)
}

/// Maps the measured (absolute, window top 83) y values to the window `r`: the meter (130-453) and
/// the fader (525-981) scale, the controls between them and below them keep their size.
struct Ly {
    top: f32,
    k: f32,
}

impl Ly {
    fn new(r: Rect) -> Ly {
        Ly { top: r.y, k: scale(r) }
    }

    fn y(&self, abs: f32) -> f32 {
        let (m0, m1, f0, f1) = (130.0 - REF_TOP, 453.0 - REF_TOP, 525.0 - REF_TOP, 981.0 - REF_TOP);
        let ry = abs - REF_TOP;
        let k = self.k;
        let y = if ry < m0 {
            ry
        } else if ry < m1 {
            m0 + (ry - m0) * k
        } else if ry < f0 {
            m0 + (m1 - m0) * k + (ry - m1)
        } else if ry < f1 {
            m0 + (m1 - m0) * k + (f0 - m1) + (ry - f0) * k
        } else {
            m0 + (m1 - m0) * k + (f0 - m1) + (f1 - f0) * k + (ry - f1)
        };
        self.top + y
    }
}

pub fn fader_db_text(v: f64) -> String {
    let gain = os_engine::laws::fader_gain(v);
    if gain <= 0.0 { "-inf dB".into() } else { format!("{:+.1} dB", 20.0 * gain.log10()) }
}

const BG: Color = Color::rgb(0x232527);
const STRIP_BG: Color = Color::rgb(0x26292b);
const HEAD_PANEL: Color = Color::rgb(0x1d1f21);
const SLOT_DARK: Color = Color::rgb(0x191d1e);
const METER_FILL: Color = Color::rgb(0x232729);
const LABEL: Color = Color::rgb(0x6d7275);
const NAME: Color = Color::rgb(0x7f868a);
const ICON: Color = Color::rgb(0xc4ccd0);

pub fn paint(ui: &mut Ui, s: &State, r: Rect, f: &FrameInfo) {
    let panel_w = 221.0;
    window_body(ui, r);
    let outer = r;
    let inner = super::float_frame(ui, s, Win::Mixer, r);
    let r = inner.unwrap_or(r);
    let client = super::window_frame_ex(ui, Win::Mixer, Rect::new(r.x, r.y, r.w - panel_w, r.h), "", 0.0, false, inner.is_some());
    title_tools(ui, r);
    if r.h < 2.0 * TITLE_H {
        // Collapsed to the title bar: only the slot panel's title with the window buttons.
        slot_panel(ui, s, Rect::new(r.right() - panel_w, r.y, panel_w, r.h));
        return;
    }
    let ly = Ly::new(r);

    let strips = Rect::new(r.x, client.y, r.w - panel_w - 7.0, client.h);
    ui.c.fill_rect(Rect::new(r.x, client.y, r.w - panel_w, client.h), BG);
    ui.hit(strips, Action::MixerArea, "Mixer");
    let bottom = r.bottom();

    // Header row: "C" (scale) and "M" (master) on a lighter band.
    ui.c.fill_rect(Rect::new(r.x + 1.0, ly.y(HEAD_Y0), 92.0, HEAD_Y1 - HEAD_Y0 + 1.0), Color::rgb(0x2b2e30));
    ui.c.text_center(Font::new(17), Rect::new(r.x + 12.0, ly.y(HEAD_Y0), 20.0, 21.0), LABEL, "C");
    ui.c.text_center(Font::new(17), Rect::new(r.x + 50.0, ly.y(HEAD_Y0), 22.0, 21.0), LABEL, "M");

    // Level scale column.
    let col = Rect::new(r.x + 9.0, ly.y(METER_Y0), 28.0, bottom - 156.0 - ly.y(METER_Y0));
    ui.c.fill_rrect(col, 3.0, SLOT_DARK);
    ui.c.fill_rrect(col.inset(1.0), 2.5, Color::rgb(0x222527));
    // Marks closer than a label height are skipped when the window is squished.
    let mut last = f32::MIN;
    for (m, y) in SCALE {
        let y = ly.y(y);
        if y - last < 14.0 {
            continue;
        }
        last = y;
        ui.c.text_center(Font::new(12), Rect::new(col.x, y - 8.0, col.w, 15.0), Color::rgb(0x7f8a90), m);
        ui.c.fill_rect(Rect::new(col.x + 4.0, y + 7.0, col.w - 8.0, 1.0), Color::rgb(0x111315));
    }
    for k in 0..4 {
        ui.c.fill_rect(Rect::new(col.x + 4.0, ly.y(893.0 + 7.5 * k as f32), col.w - 8.0, 1.0), Color::rgb(0x111315));
    }

    // Inserts scroll; the master strip and the scale stay put.
    let area = Rect::new(r.x + 97.0, client.y, strips.w - 97.0, client.h);
    ui.c.fill_rect(Rect::new(r.x + 88.0, client.y, 9.0, client.h), Color::rgb(0x1a1e20));
    for k in 0..3 {
        ui.c.fill_rect(Rect::new(r.x + 92.0, ly.y(497.0 + 3.0 * k as f32), 1.0, 1.0), Color::rgb(0x3a3e41));
    }
    ui.c.push_clip(area);
    let first = (s.mixer_scroll / STRIP_W) as usize;
    let count = (area.w / STRIP_W).ceil() as usize + 2;
    for n in (first + 1..first + 1 + count).filter(|n| *n < 127) {
        let left = area.x + STRIP_W * (n - 1) as f32 - s.mixer_scroll;
        strip(ui, s, n as u16, left, bottom, f, &ly);
    }
    ui.c.pop_clip();
    strip(ui, s, 0, r.x + 44.0, bottom, f, &ly);

    // Horizontal scroll bar under the strips.
    let sb = Rect::new(area.x + 13.0, bottom - 14.0, area.w - 20.0, 9.0);
    super::scrollbar(ui, sb, false, s.mixer_scroll, area.w, 126.0 * STRIP_W, crate::app::Scroll::MixerX);
    chevron(ui, area.x + 5.0, bottom - 9.5, false, Color::rgb(0x5d6366));

    slot_panel(ui, s, Rect::new(r.right() - panel_w, r.y, panel_w, r.h));
    resize_zones(ui, s, Win::Mixer, outer);
}

fn title_tools(ui: &mut Ui, r: Rect) {
    let y = r.y + 11.5;
    chevron(ui, r.x + 11.0, y, true, ICON);
    for (icon, dx) in [(Icon::Touch, 37.0), (Icon::Plug, 61.0)] {
        let b = Rect::new(r.x + dx - 9.0, y - 9.0, 18.0, 18.0);
        let hov = ui.hit(b, Action::Placeholder("mixer tool"), "Mixer tool");
        icons::draw(ui.c, icon, b.inset(1.0), if hov { Color::rgb(0xffffff) } else { ICON });
    }
    // Bow-tie (stereo) in blue, layout square in orange, then "+" and the view combo.
    let mut g = Glyph::new();
    let cx = r.x + 85.0;
    g.polygon(&[(cx - 8.0, y - 6.0), (cx - 1.0, y), (cx - 8.0, y + 6.0)]).polygon(&[(cx + 8.0, y - 6.0), (cx + 1.0, y), (cx + 8.0, y + 6.0)]);
    g.rect(cx - 8.5, y - 6.0, 1.5, 12.0);
    ui.c.glyph(Color::rgb(0x5fb6f0), &g);
    let mut g = Glyph::new();
    g.frame(Rect::new(r.x + 93.0, y - 6.0, 12.0, 12.0), 1.6);
    ui.c.glyph(Color::rgb(0xf3b234), &g);
    ui.c.fill_rect(Rect::new(r.x + 101.5, y - 4.5, 2.0, 9.0), Color::rgb(0xd03a2a));
    icons::draw(ui.c, Icon::Plus, Rect::new(r.x + 110.0, y - 7.0, 14.0, 14.0), ICON);
    ui.hit(Rect::new(r.x + 130.0, r.y + 2.0, 50.0, 20.0), Action::Placeholder("mixer layout"), "Mixer layout");
    ui.c.text(Font::new(17), r.x + 134.0, r.y + 2.0, Color::rgb(0xb8c4cb), "Wide");
}

/// One strip; `left` is its left edge (44 px pitch). Columns: knobs at +22, meter / fader at +32,
/// name and small controls at +12 (spec §7).
fn strip(ui: &mut Ui, s: &State, n: u16, left: f32, bottom: f32, f: &FrameInfo, ly: &Ly) {
    let t = ui.t;
    let track = s.project.mixer.iter().find(|m| m.index == n);
    let selected = s.selected_mixer == n;
    let (kx, mx, sx) = (left + 22.0, left + 32.0, left + 12.0);

    // Body and the strip divider.
    ui.c.fill_rect(Rect::new(left, ly.y(HEAD_Y1) + 1.0, STRIP_W, bottom - ly.y(HEAD_Y1) - 1.0), STRIP_BG);
    ui.c.fill_rect(Rect::new(left + STRIP_W - 1.0, ly.y(HEAD_Y1) + 1.0, 1.0, bottom - ly.y(HEAD_Y1) - 1.0), Color::rgb(0x1c1f21));
    let hov = ui.hit(Rect::new(left, ly.y(HEAD_Y0), STRIP_W, ly.y(PANEL_Y1) - ly.y(HEAD_Y0)), Action::MixerSelect(n), "Select mixer track");

    // Header number (insert strips only; the master's "M" sits in the fixed header band).
    if n != 0 {
        let hr = Rect::new(left, ly.y(HEAD_Y0), STRIP_W, ly.y(HEAD_Y1) - ly.y(HEAD_Y0) + 1.0);
        ui.c.fill_rect(hr, Color::rgb(0x2b2e30));
        if selected {
            // Hatched header for the selected strip.
            ui.c.push_clip(hr);
            for k in 0..14 {
                let x = hr.x - 20.0 + 5.0 * k as f32;
                ui.c.line(x, hr.bottom(), x + 21.0, hr.y, 1.0, Color::rgb(0x34383a));
            }
            ui.c.pop_clip();
        }
        let c = if selected || hov { Color::rgb(0x9aa1a5) } else { LABEL };
        ui.c.text_center(Font::new(17), Rect::new(left, ly.y(HEAD_Y0), STRIP_W, 21.0), c, &n.to_string());
    }

    // Dark head panel (name, LED, pan) with a rounded bottom.
    ui.c.fill_rrect(Rect::new(left + 1.0, ly.y(PANEL_Y0), STRIP_W - 2.0, ly.y(PANEL_Y1) - ly.y(PANEL_Y0)), 9.0, HEAD_PANEL);
    ui.c.fill_rect(Rect::new(left + 1.0, ly.y(PANEL_Y0), STRIP_W - 2.0, 12.0), HEAD_PANEL);

    // Meter slot with its tick marks; the selected strip gets a green frame.
    let meter = Rect::new(mx - 7.5, ly.y(METER_Y0), 16.0, ly.y(METER_Y1) - ly.y(METER_Y0));
    ui.c.fill_rrect(meter, 2.0, SLOT_DARK);
    ui.c.fill_rect(Rect::new(meter.x + 2.0, meter.y + 2.0, 11.0, meter.h - 4.0), METER_FILL);
    for y in [181.0, 230.0, 278.0, 327.0, 375.0, 423.0] {
        ui.c.fill_rect(Rect::new(meter.x + 3.0, ly.y(y), 9.0, 1.0), Color::rgb(0x111315));
    }
    if n == 0 && f.playing {
        let (pl, pr) = f.peaks;
        for (i, p) in [pl, pr].into_iter().enumerate() {
            let h = ((1.0 + 20.0 * p.max(1e-6).log10() / 48.0).clamp(0.0, 1.0)) * (meter.h - 4.0);
            let c = if p >= 1.0 { t.meter_clip } else if p > 0.5 { t.meter_high } else { t.meter_low };
            ui.c.fill_rect(Rect::new(meter.x + 2.0 + i as f32 * 6.0, meter.bottom() - 2.0 - h, 5.0, h), c);
        }
    }
    if selected {
        ui.c.stroke_rrect(Rect::new(meter.x - 1.0, meter.y - 2.0, meter.w + 2.0, meter.h + 4.0), 1.0, 2.0, t.accent);
    }

    // Name, rotated, reading upward from the bottom of the meter.
    let name = track.and_then(|m| m.name.clone()).unwrap_or_else(|| if n == 0 { "Master".into() } else { format!("Insert {n}") });
    let name_c = track
        .and_then(|m| m.color)
        .map(super::rgb)
        .map_or(if selected { Color::rgb(0xaab2b7) } else { NAME }, |c| c.mix(NAME, 0.4));
    ui.c.push_clip(Rect::new(left + 2.0, ly.y(PANEL_Y0), 20.0, ly.y(METER_Y1) - ly.y(PANEL_Y0)));
    ui.c.text_vertical(Font::new(15), sx - 7.0, meter.bottom() - 1.0, name_c, &name);
    ui.c.pop_clip();

    let enabled = track.is_none_or(|m| m.enabled);
    ui.hit(Rect::new(kx - 7.0, ly.y(LED_Y) - 7.0, 14.0, 14.0), Action::Placeholder("mute"), "Mute / unmute track");
    led(ui, kx, ly.y(LED_Y), enabled);
    ui.hit(Rect::new(kx - 14.0, ly.y(PAN_Y) - 14.0, 28.0, 28.0), Action::Placeholder("pan"), "Panning");
    mixer_knob(ui, kx, ly.y(PAN_Y), 10.5, 15.0);

    // Stereo separation, swap, and the small separation knob.
    let mut g = Glyph::new();
    g.polygon(&[(sx - 5.0, ly.y(SEP_Y) - 1.5), (sx, ly.y(SEP_Y) - 6.0), (sx + 5.0, ly.y(SEP_Y) - 1.5)]);
    g.polygon(&[(sx - 5.0, ly.y(SEP_Y) + 1.5), (sx, ly.y(SEP_Y) + 6.0), (sx + 5.0, ly.y(SEP_Y) + 1.5)]);
    g.polygon(&[(sx - 1.0, ly.y(SWAP_Y) - 6.0), (sx - 7.0, ly.y(SWAP_Y)), (sx - 1.0, ly.y(SWAP_Y) + 6.0)]);
    g.polygon(&[(sx + 1.5, ly.y(SWAP_Y) - 6.0), (sx + 7.5, ly.y(SWAP_Y)), (sx + 1.5, ly.y(SWAP_Y) + 6.0)]);
    ui.c.glyph(Color::rgb(0xb7c3ca), &g);
    ui.c.fill_rect(Rect::new(sx - 0.5, ly.y(SWAP_Y) - 6.0, 1.0, 12.0), Color::rgb(0x5a6266));
    ui.hit(Rect::new(sx - 9.0, ly.y(RING_Y) - 9.0, 18.0, 18.0), Action::Placeholder("stereo separation"), "Stereo separation");
    mixer_knob(ui, sx, ly.y(RING_Y), 6.0, 9.5);

    // Fader: track, handle (green on the selected strip, with the track lit below it).
    let track_y1 = bottom - 99.0;
    ui.c.fill_rect(Rect::new(mx - 1.5, ly.y(TRACK_Y0), 3.0, track_y1 - ly.y(TRACK_Y0)), Color::rgb(0x141618));
    let v = track.and_then(|m| m.volume).unwrap_or(0.8) as f32;
    // max/min, not clamp: a very short window gives track_y1 < ly.y(TRACK_Y0) (clamp would panic).
    let hy = (ly.y(HANDLE_0DB + (0.8 - v) * 440.0)).min(track_y1 - 5.0).max(ly.y(TRACK_Y0) + 5.0);
    if selected {
        ui.c.fill_rect(Rect::new(mx - 1.0, hy, 2.0, track_y1 - hy), Color::rgb(0x8ec63a));
    }
    let hov = ui.hit(Rect::new(mx - 9.0, ly.y(TRACK_Y0) - 5.0, 18.0, track_y1 - ly.y(TRACK_Y0) + 10.0), Action::Fader(n), "Volume (drag)");
    fader_handle(ui, mx, hy, selected, hov);

    // Routing controls near the bottom: input pin, latency clock, polarity dot.
    let master = n == 0;
    let by = bottom;
    let pin_c = if master { Color::rgb(0xf09a28) } else { Color::rgb(0x4d5357) };
    let mut g = Glyph::new();
    g.rrect(Rect::new(sx - 4.0, by - 146.0, 8.0, 9.0), 3.0);
    g.polygon(&[(sx - 2.5, by - 138.0), (sx + 2.5, by - 138.0), (sx, by - 133.0)]);
    ui.c.glyph(pin_c, &g);
    ui.c.fill_rect(Rect::new(sx - 1.0, by - 145.0, 2.0, 9.0), if master { Color::rgb(0x7a3c08) } else { Color::rgb(0x23272a) });
    ui.hit(Rect::new(sx - 8.0, by - 148.0, 16.0, 18.0), Action::Placeholder("input"), "Audio input");
    let clk = if master { Color::rgb(0xf09a28) } else { Color::rgb(0xc4d4e0) };
    let mut g = Glyph::new();
    g.ring(sx, by - 117.0, 6.0, 1.6).rect(sx - 0.7, by - 121.0, 1.4, 4.5).rect(sx - 0.7, by - 117.7, 3.5, 1.4);
    ui.c.glyph(clk, &g);
    ui.hit(Rect::new(sx - 8.0, by - 125.0, 16.0, 16.0), Action::Placeholder("latency"), "Latency compensation");
    ui.c.fill_circle(sx, by - 100.0, 2.6, Color::rgb(0xd8e0e6));

    // Send / route indicator.
    let ay = by - 38.0;
    if master {
        // The master's output knob with its green cable.
        mixer_knob(ui, kx, by - 40.0, 10.0, 13.0);
        ui.c.arc(kx, by - 40.0, 12.5, 0.75 * PI, 1.5 * PI, 2.0, t.accent);
        ui.c.fill_polygon(&[(kx - 4.0, by - 55.0), (kx, by - 59.0), (kx + 4.0, by - 55.0)], t.accent);
        ui.c.polyline(&[(kx - 3.0, by - 28.0), (kx - 1.0, by - 18.0), (kx + 6.0, by - 9.0), (kx + 10.0, by)], 2.0, t.accent);
    } else if selected {
        let a = [(kx - 3.0, ay - 6.0), (kx + 3.0, ay - 6.0), (kx + 3.0, ay - 1.0), (kx + 7.0, ay - 1.0), (kx, ay + 6.0), (kx - 7.0, ay - 1.0), (kx - 3.0, ay - 1.0)];
        ui.c.fill_polygon(&a, t.accent);
        ui.c.polyline(&[(kx, ay + 6.0), (kx - 1.0, ay + 18.0), (kx - 6.0, ay + 28.0), (kx - 7.0, ay + 38.0)], 2.0, t.accent);
    } else {
        let hov = ui.hit(Rect::new(kx - 8.0, ay - 8.0, 16.0, 14.0), Action::Placeholder("route"), "Route to this track");
        let mut g = Glyph::new();
        g.polygon(&[(kx - 6.5, ay + 3.0), (kx, ay - 3.5), (kx + 6.5, ay + 3.0)]);
        ui.c.glyph(if hov { t.accent } else { Color::rgb(0xd8e0e6) }, &g);
    }
}

/// The dark round mixer knob: a shaded ball inside a darker halo.
fn mixer_knob(ui: &mut Ui, cx: f32, cy: f32, r: f32, halo: f32) {
    ui.c.fill_circle(cx, cy, halo, Color::rgb(0x1b1d20));
    ui.c.stroke_circle(cx, cy, halo - 0.5, 1.0, Color::rgb(0x16181a));
    ui.c.fill_circle(cx, cy, r + 0.6, Color::rgb(0x16181b));
    ui.c.fill_rect_gradient(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), Color::rgb(0x3c3e41), Color::rgb(0x17191c), r);
    ui.c.arc(cx, cy, r - 0.6, 1.15 * PI, 0.7 * PI, 1.2, Color::rgb(0x55575a));
}

/// Fader handle centred on (cx, cy): 14 × 23, with the long 0 dB line through its middle.
fn fader_handle(ui: &mut Ui, cx: f32, cy: f32, selected: bool, hov: bool) {
    let h = Rect::new(cx - 7.0, cy - 11.0, 14.0, 23.0);
    ui.c.fill_rrect(h.outset(1.0), 3.0, Color::rgb(0x121416));
    let (top, mid, low) = if selected {
        (Color::rgb(0xc8f56e), Color::rgb(0x8fd236), Color::rgb(0x3f7a10))
    } else if hov {
        (Color::rgb(0x7a7e81), Color::rgb(0x55595c), Color::rgb(0x2a2d30))
    } else {
        (Color::rgb(0x6a6e71), Color::rgb(0x46494c), Color::rgb(0x222528))
    };
    ui.c.fill_rect_gradient(Rect::new(h.x, h.y, h.w, 10.0), top, mid, 2.5);
    ui.c.fill_rect_gradient(Rect::new(h.x, h.y + 10.0, h.w, 13.0), mid.shade(-0.25), low, 2.5);
    ui.c.fill_rect(Rect::new(h.x, h.y + 8.0, h.w, 4.0), mid.shade(-0.1));
    ui.c.fill_rect(Rect::new(cx - 15.0, cy - 0.5, 30.0, 1.0), Color::rgb(0x111315));
    ui.c.fill_rect(Rect::new(h.x + 2.0, h.y + 16.0, h.w - 4.0, 1.0), low.shade(-0.4));
}

fn slot_panel(ui: &mut Ui, s: &State, r: Rect) {
    let t = ui.t;
    ui.c.fill_rect(r, Color::rgb(0x2e3133));
    ui.c.fill_rect(Rect::new(r.x - 1.0, r.y, 1.0, r.h), Color::rgb(0x1b1e20));
    ui.hit(r, Action::Swallow, "");
    let n = s.selected_mixer;
    let track = s.project.mixer.iter().find(|m| m.index == n);
    let title = match track.and_then(|m| m.name.clone()) {
        Some(nm) => format!("Mixer - {nm}"),
        None if n == 0 => "Mixer - Master".into(),
        None => format!("Mixer - Insert {n}"),
    };
    ui.c.fill_rect(Rect::new(r.x, r.y, r.w, 23.0), t.window_title);
    ui.c.text_clip(Font::new(17), r.x + 1.0, r.y + 2.0, r.w - 70.0, Color::rgb(0xd4dade), &title);
    ui.hit(Rect::new(r.x, r.y, r.w, 23.0), Action::WinDrag(Win::Mixer), "Drag to move");
    window_buttons(ui, Win::Mixer, r.right() - 58.0, r.y, Color::rgb(0xd4dade), Color::rgb(0xffffff));
    if r.h < 2.0 * TITLE_H {
        return;
    }
    // Squish: the slot rows and the equalizer graph shrink with the height (the knob rows under
    // the graph are dropped when they no longer fit); the top and bottom rows keep their size.
    let room = r.h - 125.0;
    let kp = ((room - 15.0) / 503.0).clamp(0.3, 1.0);
    let knobs = room >= 579.0;
    let q = kp.max(0.55);
    // Input row: EQ glyph, "(none)" combo, latency clock.
    ui.c.fill_rect(Rect::new(r.x, r.y + 23.0, r.w, 36.0), Color::rgb(0x2b2e30));
    let mut g = Glyph::new();
    g.polygon(&[(r.x + 7.0, r.y + 32.0), (r.x + 13.0, r.y + 32.0), (r.x + 18.0, r.y + 37.5), (r.x + 13.0, r.y + 43.0), (r.x + 7.0, r.y + 43.0)]);
    ui.c.glyph(Color::rgb(0x40464a), &g);
    ui.c.text(Font::new(9).bold(), r.x + 6.0, r.y + 43.0, Color::rgb(0x40464a), "EQ");
    combo(ui, Rect::new(r.x + 27.0, r.y + 28.0, 159.0, 23.0), "(none)", Action::Placeholder("input"), "Audio input");
    let mut g = Glyph::new();
    g.ring(r.x + 202.0, r.y + 39.5, 8.5, 1.8).rect(r.x + 201.2, r.y + 34.0, 1.6, 6.5).rect(r.x + 201.2, r.y + 38.9, 5.0, 1.6);
    ui.c.glyph(Color::rgb(0xd4dade), &g);

    // Ten effect slots.
    for i in 0..10 {
        let row = Rect::new(r.x + 1.0, r.y + 61.0 + 33.0 * kp * i as f32, r.w - 2.0, 28.0 * kp);
        let plugin = track.and_then(|m| m.slots[i].as_ref());
        let fs = (18.0 * kp.max(0.62)).round() as u16;
        let hov = ui.hit(Rect::new(row.x, row.y, row.w - 45.0, row.h), Action::Placeholder("effect slot"), "Effect slot: click to add a plugin");
        ui.c.fill_rect(row, Color::rgb(0x1f2224));
        ui.c.fill_rect(row.inset(1.0), if hov { Color::rgb(0x2f3335) } else { Color::rgb(0x2a2d2f) });
        chevron(ui, row.x + 10.0, row.cy(), true, Color::rgb(0xc4ccd0));
        match plugin {
            Some(p) => ui.c.text_clip(Font::new(fs), row.x + 19.0, row.y + 4.0 * kp, 140.0, Color::rgb(0xc9d0d4), &p.name),
            None => ui.c.text_clip(Font::new(fs).italic(), row.x + 19.0, row.y + 4.0 * kp, 140.0, Color::rgb(0x50575b), &format!("Slot {}", i + 1)),
        }
        ui.hit(Rect::new(row.right() - 46.0, row.y, 26.0, row.h), Action::Placeholder("slot mix"), "Mix level");
        mixer_knob(ui, row.right() - 33.0, row.cy(), 10.5 * q, 12.0 * q);
        ui.c.stroke_circle(row.right() - 33.0, row.cy(), 11.8 * q, 1.5, Color::rgb(0x55595c));
        ui.hit(Rect::new(row.right() - 18.0, row.y, 18.0, row.h), Action::Placeholder("slot enable"), "Enable / disable slot");
        led(ui, row.right() - 10.0, row.cy(), true);
    }
    let sep = r.y + 61.0 + 330.0 * kp;
    ui.c.fill_rect(Rect::new(r.x, sep, r.w, 1.0), Color::rgb(0x1f2224));

    // Equalizer: response graph, three band gain faders, band knobs.
    let eq = Rect::new(r.x + 4.0, sep + 11.0, 146.0, 173.0 * kp);
    ui.c.fill_rect(eq, Color::rgb(0x1f2224));
    ui.c.fill_rect(eq.inset(1.0), Color::rgb(0x272a2c));
    for k in 1..5 {
        ui.c.fill_rect(Rect::new(eq.x + 14.0 + 31.0 * k as f32, eq.y + 1.0, 1.0, eq.h - 2.0), Color::rgb(0x2b2e30));
    }
    ui.c.fill_rect(Rect::new(eq.x + 1.0, eq.cy() - 1.0, eq.w - 2.0, 2.0), Color::rgb(0xc9cfd3));
    for k in 0..3 {
        let fx = r.x + 162.0 + 22.0 * k as f32;
        ui.c.fill_rect(Rect::new(fx - 2.0, eq.y + 1.0, 4.0, eq.h - 2.0), Color::rgb(0x17191b));
        ui.c.fill_rect(Rect::new(fx - 9.0, eq.y + 36.0 * kp, 18.0, 1.0), Color::rgb(0x3a3e41));
        ui.c.fill_rect(Rect::new(fx - 9.0, eq.y + 136.0 * kp, 18.0, 1.0), Color::rgb(0x3a3e41));
        let h = Rect::new(fx - 6.0, eq.cy() - 6.0, 12.0, 13.0);
        ui.c.fill_rrect(h.outset(1.0), 2.0, Color::rgb(0x121416));
        ui.c.fill_rect_gradient(h, Color::rgb(0x5e6265), Color::rgb(0x2a2d30), 2.0);
        ui.c.fill_rect(Rect::new(fx - 9.0, eq.cy(), 18.0, 1.0), Color::rgb(0x111315));
        ui.hit(Rect::new(fx - 9.0, eq.y, 18.0, eq.h), Action::Placeholder("eq band"), "Band gain");
    }
    let ly = eq.bottom() + 8.0;
    if knobs {
        ui.c.text(Font::new(17), r.x + 4.0, ly, Color::rgb(0xa9b2b7), "Equalizer");
    }
    for row in 0..2 {
        if !knobs {
            break;
        }
        for k in 0..3 {
            let (kx, ky) = (r.x + 162.0 + 22.0 * k as f32, ly + 20.0 + 20.0 * row as f32);
            ui.hit(Rect::new(kx - 9.0, ky - 9.0, 18.0, 18.0), Action::Placeholder("eq knob"), if row == 0 { "Band frequency" } else { "Band width" });
            mixer_knob(ui, kx, ky, 7.0, 8.0);
            let a = [1.1, 1.5, 1.9][k] * PI + if row == 1 { 0.3 } else { 0.0 };
            ui.c.line(kx + 1.5 * a.cos(), ky + 1.5 * a.sin(), kx + 5.5 * a.cos(), ky + 5.5 * a.sin(), 1.5, Color::rgb(0xd4dade));
        }
    }
    let mut g = Glyph::new();
    for k in 0..3 {
        g.rect(r.x + 141.0 + 3.0 * k as f32, ly + 16.0, 1.4, 6.0);
    }
    g.rect(r.x + 141.0, ly + 21.0, 8.0, 1.4);
    let ay = ly + 40.0;
    g.line(r.x + 137.0, ay, r.x + 149.0, ay, 1.2);
    g.polygon(&[(r.x + 135.0, ay), (r.x + 139.0, ay - 3.0), (r.x + 139.0, ay + 3.0)]).polygon(&[(r.x + 151.0, ay), (r.x + 147.0, ay - 3.0), (r.x + 147.0, ay + 3.0)]);
    if knobs {
        ui.c.glyph(Color::rgb(0x8c969c), &g);
    }

    // Output rows at the bottom.
    let b = r.bottom();
    ui.c.fill_rect(Rect::new(r.x, b - 66.0, r.w, 1.0), Color::rgb(0x1f2224));
    ui.c.fill_rect(Rect::new(r.x, b - 33.0, r.w, 1.0), Color::rgb(0x1f2224));
    let mut g = Glyph::new();
    g.ring(r.x + 14.0, b - 48.5, 8.0, 1.5).rect(r.x + 13.3, b - 53.0, 1.4, 5.0).rect(r.x + 13.3, b - 49.2, 4.0, 1.4);
    ui.c.glyph(Color::rgb(0x40464a), &g);
    ui.hit(Rect::new(r.x + 25.0, b - 60.0, r.w - 28.0, 22.0), Action::Placeholder("output"), "Output");
    ui.c.text(Font::new(17), r.x + 30.0, b - 59.0, Color::rgb(0xd4dade), "(none)");
    chevron(ui, r.right() - 11.0, b - 48.5, true, Color::rgb(0x8c969c));
    let mut g = Glyph::new();
    g.ring(r.x + 13.0, b - 16.0, 8.0, 1.6);
    g.erase(true).rect(r.x + 13.0, b - 26.0, 10.0, 20.0);
    g.erase(false).polygon(&[(r.x + 11.0, b - 20.5), (r.x + 16.0, b - 20.5), (r.x + 21.0, b - 16.0), (r.x + 16.0, b - 11.5), (r.x + 11.0, b - 11.5)]);
    ui.c.glyph(Color::rgb(0xc4ccd0), &g);
    combo(ui, Rect::new(r.x + 26.0, b - 28.0, 189.0, 24.0), "(none)", Action::Placeholder("output"), "Output");
}

fn combo(ui: &mut Ui, r: Rect, text: &str, action: Action, hint: &'static str) {
    let hov = ui.hit(r, action, hint);
    ui.c.fill_rrect(r, 3.0, Color::rgb(0x161819));
    ui.c.fill_rrect(r.inset(1.0), 2.5, if hov { Color::rgb(0x25282a) } else { Color::rgb(0x1e2022) });
    ui.c.text_clip(Font::new(17), r.x + 5.0, r.y + (r.h - 21.0) / 2.0, r.w - 24.0, Color::rgb(0xd4dade), text);
    chevron(ui, r.right() - 10.0, r.cy(), true, Color::rgb(0x5d6366));
}

/// Small solid chevron centred on (x, y), pointing right or left.
pub fn chevron(ui: &mut Ui, x: f32, y: f32, right: bool, c: Color) {
    let d = if right { 1.0 } else { -1.0 };
    let mut g = Glyph::new();
    g.polygon(&[(x - 2.5 * d, y - 4.5), (x + 2.5 * d, y), (x - 2.5 * d, y + 4.5)]);
    ui.c.glyph(c, &g);
}
