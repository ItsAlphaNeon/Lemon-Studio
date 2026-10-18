// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mixer window (wide layout: strips plus the selected track's slot panel).
// spec: ui/main-window.md §7 (layout), ui/mixer.md (controls, hints, routing look); fader law: audio/gain-laws.md

use std::f32::consts::PI;

use os_ui::kit::lemon as lk;
use os_ui::{Color, Font, Glyph, Rect};

use super::{led, resize_zones, window_body, window_buttons, FrameInfo};
use crate::knobs::KnobId;
use crate::app::{Action, Live, State, Ui, Win, TITLE_H};
use crate::meters;
use crate::mixer::{self as mx, MHit};
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

/// The "C" column's marks as (label, reference y), top down: the default scale's `SCALE`, its first mark the "+3"; with
/// `alt` ("Alternate meter scale") 0, 3 … 30 at the heights the default law puts the levels `meters::ALTERNATE` maps
/// them to, with no +3 (spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup
/// project").
pub fn scale_marks(alt: bool, floor: f32) -> Vec<(&'static str, f32)> {
    if !alt {
        return SCALE.to_vec();
    }
    SCALE[1..].iter().zip(meters::ALTERNATE).map(|(&(m, _), (_, d))| (m, meters::law_y(d, floor, false))).collect()
}

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


pub fn paint(ui: &mut Ui, s: &State, r: Rect, f: &FrameInfo) {
    let panel_w = 221.0;
    window_body(ui, r);
    let outer = r;
    let inner = super::float_frame(ui, s, Win::Mixer, r);
    let r = inner.unwrap_or(r);
    let client = super::window_frame_ex(ui, Win::Mixer, Rect::new(r.x, r.y, r.w - panel_w, r.h), "", 0.0, false, inner.is_some());
    title_tools(ui, s, r);
    if r.h < 2.0 * TITLE_H {
        // Collapsed to the title bar: only the slot panel's title with the window buttons.
        s.cables.borrow_mut().clear();
        slot_panel(ui, s, Rect::new(r.right() - panel_w, r.y, panel_w, r.h));
        return;
    }
    let ly = Ly::new(r);

    let strips = Rect::new(r.x, client.y, r.w - panel_w - 7.0, client.h);
    ui.c.fill_rect(Rect::new(r.x, client.y, r.w - panel_w, client.h), ui.t.s(0x232527));
    ui.hit(strips, Action::MixerArea, "Mixer");
    let inserts = mx::inserts(s);
    let bottom = r.bottom();

    // Header row: "C" (scale) and "M" (master) on a lighter band.
    ui.c.fill_rect(Rect::new(r.x + 1.0, ly.y(HEAD_Y0), 92.0, HEAD_Y1 - HEAD_Y0 + 1.0), ui.t.s(0x2b2e30));
    ui.c.text_center(Font::new(17), Rect::new(r.x + 12.0, ly.y(HEAD_Y0), 20.0, 21.0), ui.t.s(0x6d7275), "C");
    ui.c.text_center(Font::new(17), Rect::new(r.x + 50.0, ly.y(HEAD_Y0), 22.0, 21.0), ui.t.s(0x6d7275), "M");

    // "C" column: the level scale with the current track's meter (spec: ui/mixer.md §9).
    let col = Rect::new(r.x + 9.0, ly.y(METER_Y0), 28.0, bottom - 156.0 - ly.y(METER_Y0));
    let cfg = &s.mixer.meter_cfg;
    let floor = cfg.v[meters::k::FLOOR_DB];
    // The column is the law's whole range (130-924 at the measured size) squeezed evenly into its height, so marks and
    // bars keep their proportions at any mixer height (the strips' fixed-size controls do not apply here).
    let law_h = meters::LAW_BOTTOM - meters::LAW_TOP;
    let cy = |abs: f32| col.y + (abs - meters::LAW_TOP) / law_h * col.h;
    let alt = crate::prefs::check(&crate::prefs::ALTERNATE_METER_SCALE);
    let hint = if col.contains(ui.mouse.0, ui.mouse.1) {
        // The level at the mouse (the scale is not linear in y: invert it through the law).
        let db = meters::db_at((col.bottom() - ui.mouse.1) / col.h.max(1.0), floor, alt);
        format!("Current  {db:.2}dB  {:.2}", 10f32.powf(db / 20.0))
    } else {
        "Level scale".to_string()
    };
    ui.hit(col, Action::Mix(MHit::Scale), hint);
    let lemon = super::lemon(ui);
    if lemon {
        lk::outlined(ui.c, col, lk::r_btn(ui.t), ui.t.s(0x191d1e), ui.t.s(0x222527));
    } else {
        ui.c.fill_rrect(col, 3.0, ui.t.s(0x191d1e));
        ui.c.fill_rrect(col.inset(1.0), 2.5, ui.t.s(0x222527));
    }
    // Marks closer than a label height are skipped when the window is squished: "0" always shows, "+3" only with room
    // above it, the rest downward from "0". The alternate scale has no "+3".
    let scale = scale_marks(alt, floor);
    let (plus, below) = if alt { (None, &scale[..]) } else { (Some(scale[0]), &scale[1..]) };
    let zero = cy(below[0].1);
    let mut marks: Vec<(&str, f32)> = Vec::new();
    if let Some((m, y)) = plus.map(|(m, y)| (m, cy(y))).filter(|&(_, y)| zero - y >= 14.0) {
        marks.push((m, y));
    }
    marks.push((below[0].0, zero));
    let mut last = zero;
    for &(m, y) in &below[1..] {
        let y = cy(y);
        if y - last >= 14.0 {
            marks.push((m, y));
            last = y;
        }
    }
    for (m, y) in marks {
        ui.c.text_center(Font::new(12), Rect::new(col.x, y - 8.0, col.w, 15.0), ui.t.s(0x7f8a90), m);
        ui.c.fill_rect(Rect::new(col.x + 4.0, y + 7.0, col.w - 8.0, 1.0), ui.t.s(0x111315));
    }
    // Four unlabelled ticks below 45 (the alternate scale has none below its 30).
    for k in (0..4).filter(|_| !alt) {
        let y = cy(893.0 + 7.5 * k as f32);
        if y - last >= 10.0 {
            ui.c.fill_rect(Rect::new(col.x + 4.0, y, col.w - 8.0, 1.0), ui.t.s(0x111315));
        }
    }
    // The bars cover the labels (measured: two 13 px bars filling the column inside its 1 px border).
    ui.live(Live::Meter, col);
    if let Some(m) = s.mixer.meters.get(usize::from(s.selected_mixer)) {
        let ys = |db: f32| cy(meters::law_y(db, floor, alt)).min(col.bottom() - 1.0);
        for i in 0..2 {
            meter_bar(ui, col.x + 1.0 + 13.0 * i as f32, 13.0, col.bottom() - 1.0, m.db[i], m.hold[i], cfg, &ys);
        }
    }

    // Inserts scroll; the master strip and the scale stay put.
    let area = Rect::new(r.x + 97.0, client.y, strips.w - 97.0, client.h);
    ui.c.fill_rect(Rect::new(r.x + 88.0, client.y, 9.0, client.h), ui.t.s(0x1a1e20));
    for k in 0..3 {
        ui.c.fill_rect(Rect::new(r.x + 92.0, ly.y(497.0 + 3.0 * k as f32), 1.0, 1.0), ui.t.s(0x3a3e41));
    }
    ui.c.push_clip(area);
    let first = (s.mixer_scroll / STRIP_W) as usize;
    let count = (area.w / STRIP_W).ceil() as usize + 2;
    for n in (first + 1..first + 1 + count).filter(|n| *n <= usize::from(inserts)) {
        let left = area.x + STRIP_W * (n - 1) as f32 - s.mixer_scroll;
        strip(ui, s, n as u16, left, bottom, f, &ly);
    }
    // "+" after the last strip, at the fader handle's height (spec: ui/mixer.md §1).
    let px = area.x + STRIP_W * f32::from(inserts) - s.mixer_scroll + 14.0;
    let pb = Rect::new(px - 9.0, ly.y(HANDLE_0DB) - 9.0, 18.0, 18.0);
    if px < area.right() + 10.0 {
        let hov = ui.hit(pb, Action::Mix(MHit::Add), "Add more tracks");
        if hov && lemon {
            ui.c.fill_rrect(pb, lk::r_btn(ui.t), ui.t.button_hi);
        } else if hov {
            ui.c.stroke_rrect(pb, 3.0, 1.0, ui.t.h(0xf09a28));
        }
        icons::draw(ui.c, Icon::Plus, pb.inset(3.0), if hov { ui.t.tx(0xffffff) } else { ui.t.tx(0x9aa1a5) });
    }
    ui.c.pop_clip();
    strip(ui, s, 0, r.x + 44.0, bottom, f, &ly);

    // Horizontal scroll bar under the strips.
    let sb = Rect::new(area.x + 13.0, bottom - 14.0, area.w - 20.0, 9.0);
    super::scrollbar(ui, sb, false, s.mixer_scroll, area.w, (f32::from(inserts) + 1.0) * STRIP_W, crate::app::Scroll::MixerX);
    chevron(ui, area.x + 5.0, bottom - 9.5, false, ui.t.s(0x5d6366));
    cables(ui, s, strips, area, bottom);

    slot_panel(ui, s, Rect::new(r.right() - panel_w, r.y, panel_w, r.h));
    resize_zones(ui, s, Win::Mixer, outer);
}

/// The routing cables (spec: ui/mixer.md §5): one from the current track's ⬇ into each of its send knobs, sagging
/// and swinging (crate::cable). Sockets on strips scrolled out of view still hold their cable's end.
fn cables(ui: &mut Ui, s: &State, strips: Rect, area: Rect, bottom: f32) {
    let socket_x = |n: u16| match n {
        0 => strips.x + 44.0 + 22.0,
        n => area.x + STRIP_W * f32::from(n - 1) - s.mixer_scroll + 22.0,
    };
    // Each cable is plugged in under the middle of its ⬇ and its knob, which are drawn again over the cables' ends.
    let (ay, ky) = (bottom - 38.0, bottom - 40.0);
    let cur = s.selected_mixer;
    let from = (socket_x(cur), ay);
    let sends: Vec<(u16, f64)> = (0..=mx::inserts(s)).filter(|&n| n != cur).filter_map(|n| Some((n, mx::send(s, cur, n)?))).collect();
    let mut cables = s.cables.borrow_mut();
    cables.begin(cur);
    for &(n, level) in &sends {
        // A send at zero is dimmed, as its knob.
        let col = if level <= 0.0 { ui.t.accent.mix(ui.t.s(0x26292b), 0.65) } else { ui.t.accent };
        cables.connect(n, from, (socket_x(n), ky), cur == 0 || n == 0, col);
    }
    cables.end();
    // Cables between inserts are clipped where the inserts scroll; those into the master are not.
    for (master, pts, col) in cables.each() {
        ui.c.push_clip(if master { strips } else { area });
        ui.c.polyline(pts, 2.0, col);
        ui.c.pop_clip();
    }
    drop(cables);
    if sends.is_empty() {
        return;
    }
    let clip = |n: u16| if n == 0 { strips } else { area };
    ui.c.push_clip(clip(cur));
    down_arrow(ui, from.0, ay);
    ui.c.pop_clip();
    for (n, level) in sends {
        ui.c.push_clip(clip(n));
        send_knob(ui, s, n, socket_x(n), ky, level);
        ui.c.pop_clip();
    }
}

fn title_tools(ui: &mut Ui, s: &State, r: Rect) {
    let y = r.y + 11.5;
    let ob = Rect::new(r.x + 3.0, y - 9.0, 16.0, 18.0);
    let hov = ui.hit(ob, Action::Mix(MHit::Options), "Mixer options");
    chevron(ui, r.x + 11.0, y, true, if hov { ui.t.tx(0xffffff) } else { ui.t.tx(0xc4ccd0) });
    // View buttons (multi-touch, waveforms, extra properties, track inspector): not available yet, greyed out (drawn
    // in their colours faded into the title bar, like `Ui::veil`; a veil would show over the bar's gradient).
    let bg = ui.t.window_title;
    let dim = |c: Color| c.mix(bg, f32::from(crate::app::OFF_VEIL) / 255.0);
    for (icon, dx, name) in [(Icon::Touch, 37.0, "Multi-touch control"), (Icon::Plug, 61.0, "View waveforms")] {
        let b = Rect::new(r.x + dx - 9.0, y - 9.0, 18.0, 18.0);
        ui.off(b, name);
        icons::draw(ui.c, icon, b.inset(1.0), dim(ui.t.tx(0xc4ccd0)));
    }
    ui.off(Rect::new(r.x + 76.0, y - 9.0, 16.0, 18.0), "Extra volume / stereo properties");
    ui.off(Rect::new(r.x + 92.0, y - 9.0, 15.0, 18.0), "Track inspector");
    // Bow-tie (stereo) in blue, layout square in orange, then "+" and the view combo.
    let mut g = Glyph::new();
    let cx = r.x + 85.0;
    g.polygon(&[(cx - 8.0, y - 6.0), (cx - 1.0, y), (cx - 8.0, y + 6.0)]).polygon(&[(cx + 8.0, y - 6.0), (cx + 1.0, y), (cx + 8.0, y + 6.0)]);
    g.rect(cx - 8.5, y - 6.0, 1.5, 12.0);
    ui.c.glyph(dim(Color::rgb(0x5fb6f0)), &g);
    let mut g = Glyph::new();
    g.frame(Rect::new(r.x + 93.0, y - 6.0, 12.0, 12.0), 1.6);
    ui.c.glyph(dim(ui.t.h(0xf3b234)), &g);
    ui.c.fill_rect(Rect::new(r.x + 101.5, y - 4.5, 2.0, 9.0), dim(Color::rgb(0xd03a2a)));
    let pb = Rect::new(r.x + 109.0, y - 8.0, 16.0, 16.0);
    let hov = ui.hit(pb, Action::Mix(MHit::AddAfter), "Add a new track after the selected track");
    icons::draw(ui.c, Icon::Plus, Rect::new(r.x + 110.0, y - 7.0, 14.0, 14.0), if hov { ui.t.tx(0xffffff) } else { ui.t.tx(0xc4ccd0) });
    let lb = Rect::new(r.x + 130.0, r.y + 2.0, 50.0, 20.0);
    let hov = ui.hit(lb, Action::Mix(MHit::Layout), "Mixer layout");
    let _ = s;
    ui.c.text(Font::new(17), r.x + 134.0, r.y + 2.0, if hov { ui.t.tx(0xffffff) } else { ui.t.tx(0xb8c4cb) }, "Wide");
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u32;
    Color::rgb(ch(a.r, b.r) << 16 | ch(a.g, b.g) << 8 | ch(a.b, b.b))
}

fn shade(c: Color, k: f32) -> Color {
    mix(Color::rgb(0), c, k)
}

/// One meter bar (spec: ui/mixer.md §9, measured from FL): the colour depends on the height, not on the level: green up
/// to "Yellow from", yellow-orange up to "Red from", red above; each zone is a slight vertical gradient. The outer
/// columns are darker, the next ones a touch lighter; a 1 px dark cap sits on the top and the peak-hold line is a
/// 1 px orange line. `ys` maps a level to y; `bottom` is the bar's foot.
#[allow(clippy::too_many_arguments)]
fn meter_bar(ui: &mut Ui, x: f32, w: f32, bottom: f32, db: f32, hold: f32, cfg: &meters::Settings, ys: &dyn Fn(f32) -> f32) {
    use meters::k;
    let t = ui.t;
    let full_top = ys(12.0);
    let grad = cfg.v[k::GRADIENT];
    let edge = cfg.v[k::EDGE];
    // Zone colours at the zone's top and bottom (measured 0xa0db40 / 0xb6db4d, 0xeec757 / 0xfac050).
    let green = (ui.t.a(0xa0db40), ui.t.a(0xb6db4d));
    let yellow = (ui.t.h(0xeec757), ui.t.h(0xfac050));
    let red = (t.meter_clip, t.meter_clip);
    let y_yel = ys(cfg.v[k::YELLOW_DB]);
    let y_red = ys(cfg.v[k::RED_DB].max(cfg.v[k::YELLOW_DB]));
    if db > meters::SILENT {
        let top = ys(db).min(bottom);
        for (z0, z1, (ct, cb)) in [(y_yel, bottom, green), (y_red, y_yel, yellow), (full_top, y_red, red)] {
            let (a, b) = (top.max(z0), z1.min(bottom));
            if b - a < 0.5 {
                continue;
            }
            // Colours at a and b along the zone's gradient (scaled by the gradient setting).
            let at = |y: f32| mix(ct, mix(ct, cb, grad), (y - z0) / (z1 - z0).max(1.0));
            let (ca, cb2) = (at(a), at(b));
            let r = Rect::new(x, a, w, b - a);
            ui.c.fill_rect_gradient(r, ca, cb2, 0.0);
            if w >= 4.0 {
                let hl = |c: Color| mix(c, Color::rgb(0xffffff), 0.04);
                ui.c.fill_rect_gradient(Rect::new(x + 1.0, a, 1.0, b - a), hl(ca), hl(cb2), 0.0);
                ui.c.fill_rect_gradient(Rect::new(x + w - 2.0, a, 1.0, b - a), hl(ca), hl(cb2), 0.0);
                ui.c.fill_rect_gradient(Rect::new(x, a, 1.0, b - a), shade(ca, edge), shade(cb2, edge), 0.0);
                ui.c.fill_rect_gradient(Rect::new(x + w - 1.0, a, 1.0, b - a), shade(ca, edge), shade(cb2, edge), 0.0);
            }
        }
        if cfg.v[k::CAP] >= 0.5 && top - 1.0 > full_top {
            ui.c.fill_rect(Rect::new(x, top - 1.0, w, 1.0), ui.t.s(0x181c1e));
        }
    }
    if cfg.v[k::HOLD_ON] >= 0.5 && hold > meters::SILENT {
        let y = ys(hold).min(bottom - 1.0);
        let c = ui.t.h(0xe16e2f);
        ui.c.fill_rect(Rect::new(x, y, w, 1.0), c);
        if w >= 4.0 {
            ui.c.fill_rect(Rect::new(x, y, 1.0, 1.0), shade(c, edge));
            ui.c.fill_rect(Rect::new(x + w - 1.0, y, 1.0, 1.0), shade(c, edge));
        }
    }
}

/// One strip; `left` is its left edge (44 px pitch). Columns: knobs at +22, meter / fader at +32,
/// name and small controls at +12 (spec §7).
fn strip(ui: &mut Ui, s: &State, n: u16, left: f32, bottom: f32, f: &FrameInfo, ly: &Ly) {
    let t = ui.t;
    let track = s.project.mixer.iter().find(|m| m.index == n);
    let selected = s.selected_mixer == n;
    let also = !selected && mx::is_selected(s, n);
    let (kx, mx, sx) = (left + 22.0, left + 32.0, left + 12.0);
    let _ = f;

    // Body and the strip divider.
    // The current strip's body is a shade lighter (measured: 0x26292b vs 0x232527).
    let body = if selected { ui.t.s(0x26292b) } else { ui.t.s(0x232527) };
    let lemon = super::lemon(ui);
    if lemon {
        // Lemon (spec: ui/lemon-design.md §8): each strip is a rounded card on the dark gap colour.
        let card = Rect::new(left, ly.y(HEAD_Y1) + 1.0, STRIP_W, bottom - ly.y(HEAD_Y1) - 1.0);
        ui.c.fill_rect(card, ui.t.s(0x131517));
        ui.c.fill_rrect(Rect::new(card.x + 1.5, card.y + 1.0, card.w - 3.0, card.h - 4.0), lk::r_panel(t), body);
    } else {
        ui.c.fill_rect(Rect::new(left, ly.y(HEAD_Y1) + 1.0, STRIP_W, bottom - ly.y(HEAD_Y1) - 1.0), body);
        ui.c.fill_rect(Rect::new(left + STRIP_W - 1.0, ly.y(HEAD_Y1) + 1.0, 1.0, bottom - ly.y(HEAD_Y1) - 1.0), ui.t.s(0x1c1f21));
    }
    // The whole strip selects (Ctrl adds, Shift renames); controls register on top of it (spec: ui/mixer.md §2).
    ui.hit(Rect::new(left, ly.y(HEAD_Y0), STRIP_W, bottom - ly.y(HEAD_Y0)), Action::Mix(MHit::Strip(n)), mx::name(s, n));
    let hov = ui.hovering(Rect::new(left, ly.y(HEAD_Y0), STRIP_W, ly.y(PANEL_Y1) - ly.y(HEAD_Y0)));

    // Header number (insert strips only; the master's "M" sits in the fixed header band).
    if n != 0 {
        let hr = Rect::new(left, ly.y(HEAD_Y0), STRIP_W, ly.y(HEAD_Y1) - ly.y(HEAD_Y0) + 1.0);
        ui.c.fill_rect(hr, ui.t.s(0x2b2e30));
        if selected && lemon {
            lk::lit_in(ui.c, ui.t, Rect::new(hr.x + 3.0, hr.y + 2.0, hr.w - 6.0, hr.h - 4.0), lk::r_btn(t), false);
        } else if selected {
            // Hatched header for the selected strip.
            ui.c.push_clip(hr);
            for k in 0..14 {
                let x = hr.x - 20.0 + 5.0 * k as f32;
                ui.c.line(x, hr.bottom(), x + 21.0, hr.y, 1.0, ui.t.s(0x34383a));
            }
            ui.c.pop_clip();
        }
        let c = if selected && lemon { lk::ink_on_lit(ui.t) } else if selected || hov { ui.t.tx(0x9aa1a5) } else { ui.t.s(0x6d7275) };
        ui.c.text_center(Font::new(17), Rect::new(left, ly.y(HEAD_Y0), STRIP_W, 21.0), c, &n.to_string());
    }

    // Dark head panel (name, LED, pan) with a rounded bottom.
    ui.c.fill_rrect(Rect::new(left + 1.0, ly.y(PANEL_Y0), STRIP_W - 2.0, ly.y(PANEL_Y1) - ly.y(PANEL_Y0)), 9.0, ui.t.s(0x1d1f21));
    ui.c.fill_rect(Rect::new(left + 1.0, ly.y(PANEL_Y0), STRIP_W - 2.0, 12.0), ui.t.s(0x1d1f21));

    // Meter slot with its tick marks; the selected strip gets a green frame.
    let meter = Rect::new(mx - 7.5, ly.y(METER_Y0), 16.0, ly.y(METER_Y1) - ly.y(METER_Y0));
    if lemon {
        ui.c.fill_rrect(meter, lk::r_small(t), ui.t.s(0x191d1e));
        ui.c.fill_rrect(Rect::new(meter.x + 2.0, meter.y + 2.0, 12.0, meter.h - 4.0), 2.5, ui.t.s(0x232729));
    } else {
        ui.c.fill_rrect(meter, 2.0, ui.t.s(0x191d1e));
        ui.c.fill_rect(Rect::new(meter.x + 2.0, meter.y + 2.0, 11.0, meter.h - 4.0), ui.t.s(0x232729));
    }
    for y in [181.0, 230.0, 278.0, 327.0, 375.0, 423.0] {
        ui.c.fill_rect(Rect::new(meter.x + 3.0, ly.y(y), 9.0, 1.0), ui.t.s(0x111315));
    }
    // Per-track peak meter (spec: ui/mixer.md §9): two 6 px bars on the same scale as the "C" column.
    ui.live(Live::Meter, meter);
    if let Some(m) = s.mixer.meters.get(usize::from(n)) {
        let cfg = &s.mixer.meter_cfg;
        let (top, bot) = (meter.y + 2.0, meter.bottom() - 2.0);
        let floor = cfg.v[meters::k::FLOOR_DB];
        let alt = crate::prefs::check(&crate::prefs::ALTERNATE_METER_SCALE);
        let ys = |db: f32| bot - meters::frac(db, floor, alt) * (bot - top);
        for i in 0..2 {
            meter_bar(ui, meter.x + 2.0 + 6.0 * i as f32, 6.0, bot, m.db[i], m.hold[i], cfg, &ys);
        }
    }
    if selected {
        ui.c.stroke_rrect(Rect::new(meter.x - 1.0, meter.y - 2.0, meter.w + 2.0, meter.h + 4.0), if lemon { lk::r_btn(t) } else { 1.0 }, 2.0, t.accent);
    }

    // Name, rotated, reading upward from the bottom of the meter.
    let name = track.and_then(|m| m.name.clone()).unwrap_or_else(|| if n == 0 { "Master".into() } else { format!("Insert {n}") });
    let name_c = track
        .and_then(|m| m.color)
        .map(super::rgb)
        .map_or(if selected { ui.t.tx(0xaab2b7) } else if also { ui.t.tx(0xe4e8ea) } else { ui.t.s(0x7f868a) }, |c| {
            if also { c.mix(ui.t.tx(0xffffff), 0.5) } else { c.mix(ui.t.s(0x7f868a), 0.4) }
        });
    ui.c.push_clip(Rect::new(left + 2.0, ly.y(PANEL_Y0), 20.0, ly.y(METER_Y1) - ly.y(PANEL_Y0)));
    ui.c.text_vertical(Font::new(15), sx - 7.0, meter.bottom() - 1.0, name_c, &name);
    ui.c.pop_clip();

    let enabled = track.is_none_or(|m| m.enabled);
    ui.hit(Rect::new(kx - 7.0, ly.y(LED_Y) - 7.0, 14.0, 14.0), Action::Mix(MHit::Led(n)), "Mute / solo");
    led(ui, kx, ly.y(LED_Y), enabled);
    let pan = KnobId::MixPan(n);
    ui.hit(Rect::new(kx - 14.0, ly.y(PAN_Y) - 14.0, 28.0, 28.0), Action::Knob(pan), s.knob_text(pan));
    mixer_knob(ui, kx, ly.y(PAN_Y), 10.5, 15.0, s.knob(pan), Some(true));

    // Reverse polarity (⇕) and swap left / right (⇔) buttons, lit when on, then the separation knob (§4).
    // They keep their size and spacing below the fader's top at any mixer height (FL's short mixer stacks them the
    // same way), so they are placed from the fader's top, not scaled with it.
    let (pol, swp) = (track.is_some_and(|m| m.polarity), track.is_some_and(|m| m.swap));
    let top = ly.y(TRACK_Y0);
    let (sep_y, swap_y, ring_y) = (top + SEP_Y - TRACK_Y0, top + SWAP_Y - TRACK_Y0, top + RING_Y - TRACK_Y0);
    let pr = Rect::new(sx - 8.0, sep_y - 8.0, 16.0, 16.0);
    let hp = ui.hit(pr, Action::Mix(MHit::Polarity(n)), "Reverse polarity");
    let mut g = Glyph::new();
    g.polygon(&[(sx - 5.0, sep_y - 1.5), (sx, sep_y - 6.0), (sx + 5.0, sep_y - 1.5)]);
    g.polygon(&[(sx - 5.0, sep_y + 1.5), (sx, sep_y + 6.0), (sx + 5.0, sep_y + 1.5)]);
    ui.c.glyph(if pol { ui.t.h(0xf3b234) } else if hp { ui.t.tx(0xffffff) } else { ui.t.tx(0xb7c3ca) }, &g);
    let wr = Rect::new(sx - 8.0, swap_y - 8.0, 16.0, 16.0);
    let hw = ui.hit(wr, Action::Mix(MHit::Swap(n)), "Swap left & right channels");
    let mut g = Glyph::new();
    g.polygon(&[(sx - 1.0, swap_y - 6.0), (sx - 7.0, swap_y), (sx - 1.0, swap_y + 6.0)]);
    g.polygon(&[(sx + 1.5, swap_y - 6.0), (sx + 7.5, swap_y), (sx + 1.5, swap_y + 6.0)]);
    ui.c.glyph(if swp { Color::rgb(0x5fb6f0) } else if hw { ui.t.tx(0xffffff) } else { ui.t.tx(0xb7c3ca) }, &g);
    ui.c.fill_rect(Rect::new(sx - 0.5, swap_y - 6.0, 1.0, 12.0), ui.t.s(0x5a6266));
    let sep = KnobId::MixSep(n);
    ui.hit(Rect::new(sx - 9.0, ring_y - 9.0, 18.0, 18.0), Action::Knob(sep), s.knob_text(sep));
    mixer_knob(ui, sx, ring_y, 6.0, 9.5, s.knob(sep), Some(true));

    // Fader: track, gain marks, handle (green on the selected strip, with the track lit below it).
    let track_y1 = bottom - 99.0;
    if lemon {
        ui.c.fill_rrect(Rect::new(mx - 2.0, ly.y(TRACK_Y0), 4.0, track_y1 - ly.y(TRACK_Y0)), 2.0, ui.t.s(0x141618));
    } else {
        ui.c.fill_rect(Rect::new(mx - 1.5, ly.y(TRACK_Y0), 3.0, track_y1 - ly.y(TRACK_Y0)), ui.t.s(0x141618));
    }
    fader_marks(ui, mx, track_y1, ly);
    let v = track.and_then(|m| m.volume).unwrap_or(0.8) as f32;
    // max/min, not clamp: a very short window gives track_y1 < ly.y(TRACK_Y0) (clamp would panic).
    let hy = (ly.y(HANDLE_0DB + (0.8 - v) * 440.0)).min(track_y1 - 5.0).max(ly.y(TRACK_Y0) + 5.0);
    if selected {
        if lemon {
            ui.c.fill_rrect(Rect::new(mx - 1.5, hy, 3.0, track_y1 - hy), 1.5, t.accent);
        } else {
            ui.c.fill_rect(Rect::new(mx - 1.0, hy, 2.0, track_y1 - hy), ui.t.a(0x8ec63a));
        }
    }
    let hov = ui.hit(Rect::new(mx - 9.0, ly.y(TRACK_Y0) - 5.0, 18.0, track_y1 - ly.y(TRACK_Y0) + 10.0), Action::Fader(n), mx::level_text("Volume", f64::from(v)));
    fader_handle(ui, mx, hy, selected, hov);

    // Input pin, latency clock and disk-recording dot: not available yet, greyed out.
    let master = n == 0;
    let by = bottom;
    let pin_c = if master { ui.t.h(0xf09a28) } else { ui.t.s(0x4d5357) };
    let mut g = Glyph::new();
    g.rrect(Rect::new(sx - 4.0, by - 146.0, 8.0, 9.0), 3.0);
    g.polygon(&[(sx - 2.5, by - 138.0), (sx + 2.5, by - 138.0), (sx, by - 133.0)]);
    ui.c.glyph(pin_c, &g);
    ui.c.fill_rect(Rect::new(sx - 1.0, by - 145.0, 2.0, 9.0), if master { ui.t.h(0x7a3c08) } else { ui.t.s(0x23272a) });
    ui.off(Rect::new(sx - 8.0, by - 148.0, 16.0, 18.0), "Track audio input");
    let clk = if master { ui.t.h(0xf09a28) } else { ui.t.tx(0xc4d4e0) };
    let mut g = Glyph::new();
    g.ring(sx, by - 117.0, 6.0, 1.6).rect(sx - 0.7, by - 121.0, 1.4, 4.5).rect(sx - 0.7, by - 117.7, 3.5, 1.4);
    ui.c.glyph(clk, &g);
    ui.off(Rect::new(sx - 8.0, by - 125.0, 16.0, 16.0), "Track latency");
    ui.off(Rect::new(sx - 6.0, by - 106.0, 12.0, 12.0), "Disk recording");
    ui.c.fill_circle(sx, by - 100.0, 2.6, ui.t.tx(0xd8e0e6));
    ui.veil(Rect::new(sx - 9.0, by - 149.0, 18.0, 55.0), body);

    // Routing (spec: ui/mixer.md §5): the current track's green ⬇ (its cables are drawn over all strips: `cables`),
    // a send knob where the current track sends, else the ▲ "route to this track" button; an orange "^" marks tracks that receive a route.
    let ay = by - 38.0;
    let cur = s.selected_mixer;
    let cur_name = mx::name(s, cur);
    if selected {
        down_arrow(ui, kx, ay);
    } else if let Some(level) = mx::send(s, cur, n) {
        // Send knob (fader law) with its arc, the green "^" that removes the route; its cable is drawn with the others.
        let ky = by - 40.0;
        let id = KnobId::Send(cur, n);
        ui.hit(Rect::new(kx - 13.0, ky - 13.0, 26.0, 26.0), Action::Knob(id), s.knob_text(id));
        send_knob(ui, s, n, kx, ky, level);
        let ur = Rect::new(kx - 6.0, by - 61.0, 12.0, 8.0);
        let hov = ui.hit(ur, Action::Mix(MHit::Unroute(n)), format!("Remove the send from {cur_name} to {}", mx::name(s, n)));
        ui.c.fill_polygon(&[(kx - 4.0, by - 55.0), (kx, by - 59.0), (kx + 4.0, by - 55.0)], if hov { ui.t.tx(0xffffff) } else { t.accent });
    } else if !(master && cur == 0) {
        let hint = format!("Enable send from {cur_name} to {}  Ctrl/Shift", mx::name(s, n));
        let hov = ui.hit(Rect::new(kx - 8.0, ay - 8.0, 16.0, 14.0), Action::Mix(MHit::Route(n)), hint);
        let mut g = Glyph::new();
        g.polygon(&[(kx - 6.5, ay + 3.0), (kx, ay - 3.5), (kx + 6.5, ay + 3.0)]);
        ui.c.glyph(if hov { t.accent } else { ui.t.tx(0xd8e0e6) }, &g);
        if !master && mx::receives(s, n) {
            ui.c.fill_polygon(&[(kx - 3.5, ay - 9.0), (kx, ay - 12.5), (kx + 3.5, ay - 9.0)], ui.t.h(0xf09a28));
        }
    }
}

/// The current track's green ⬇ (centred on `ay`): where its cables leave.
fn down_arrow(ui: &mut Ui, kx: f32, ay: f32) {
    let a = [(kx - 3.0, ay - 6.0), (kx + 3.0, ay - 6.0), (kx + 3.0, ay - 1.0), (kx + 7.0, ay - 1.0), (kx, ay + 6.0), (kx - 7.0, ay - 1.0), (kx - 3.0, ay - 1.0)];
    ui.c.fill_polygon(&a, ui.t.accent);
}

/// The send knob from the current track into track `n`, with its level arc (none at zero).
fn send_knob(ui: &mut Ui, s: &State, n: u16, kx: f32, ky: f32, level: f64) {
    let id = KnobId::Send(s.selected_mixer, n);
    mixer_knob(ui, kx, ky, 10.0, 13.0, s.knob(id), None);
    if level > 0.0 {
        // From the knob's zero to its pointer (the arc takes a sweep, not an end angle).
        let sweep = (level as f32).min(1.0) * 1.5 * PI;
        ui.c.arc(kx, ky, 12.5, 0.75 * PI, sweep, 2.0, ui.t.accent);
    }
}

/// The dark round mixer knob: a shaded ball inside a darker halo.
/// `arc`: Some(true) draws the red value arc from the top (pan style), Some(false) from the start.
fn mixer_knob(ui: &mut Ui, cx: f32, cy: f32, r: f32, halo: f32, value: f32, arc: Option<bool>) {
    let hov = ui.hovering(Rect::new(cx - halo, cy - halo, 2.0 * halo, 2.0 * halo));
    let look = os_ui::kit::KnobLook::Mixer { halo, arc: arc.is_some() };
    os_ui::kit::knob(ui.c, ui.t, cx, cy, r, value, arc.unwrap_or(false), look, os_ui::kit::State::hover(hov));
}

/// Gain marks beside a fader track (spec: ui/mixer.md §5, measured from FL): 6 px ticks on both sides at 0, −6, −12 and
/// −18 dB (2 px clear of the track), and faint 4 px rails between consecutive ticks (1 px clear of each tick). They
/// belong to the track: the handle covers them as it moves.
fn fader_marks(ui: &mut Ui, mx: f32, track_y1: f32, ly: &Ly) {
    let ys: Vec<f32> = [0.0, -6.0, -12.0, -18.0]
        .iter()
        .map(|&db| ly.y(HANDLE_0DB + (0.8 - fader_value(db)) * 440.0).floor())
        .filter(|&y| y < track_y1 - 1.0)
        .collect();
    let (tick, rail) = (ui.t.s(0x181a1c), ui.t.s(0x202324));
    for w in ys.windows(2) {
        let (a, b) = (w[0] + 2.0, w[1] - 1.0);
        if b > a {
            ui.c.fill_rect(Rect::new(mx - 7.5, a, 4.0, b - a), rail);
            ui.c.fill_rect(Rect::new(mx + 3.5, a, 4.0, b - a), rail);
        }
    }
    for y in ys {
        ui.c.fill_rect(Rect::new(mx - 9.5, y, 6.0, 1.0), tick);
        ui.c.fill_rect(Rect::new(mx + 3.5, y, 6.0, 1.0), tick);
    }
}

/// Fader value (0..1, 0.8 = 0 dB) of a gain in dB: the inverse of `laws::fader_gain`.
fn fader_value(db: f32) -> f32 {
    0.8 * (10.0 * 10f32.powf(db / 20.0) + 1.0).ln() / 11f32.ln()
}

/// Fader handle centred on (cx, cy): 14 × 23.
fn fader_handle(ui: &mut Ui, cx: f32, cy: f32, selected: bool, hov: bool) {
    os_ui::kit::fader_handle(ui.c, ui.t, cx, cy, selected, os_ui::kit::State::hover(hov));
}

fn slot_panel(ui: &mut Ui, s: &State, r: Rect) {
    let t = ui.t;
    ui.c.fill_rect(r, ui.t.s(0x2e3133));
    ui.c.fill_rect(Rect::new(r.x - 1.0, r.y, 1.0, r.h), ui.t.s(0x1b1e20));
    ui.hit(r, Action::Swallow, "");
    let n = s.selected_mixer;
    let track = s.project.mixer.iter().find(|m| m.index == n);
    let title = match track.and_then(|m| m.name.clone()) {
        Some(nm) => format!("Mixer - {nm}"),
        None if n == 0 => "Mixer - Master".into(),
        None => format!("Mixer - Insert {n}"),
    };
    ui.c.fill_rect(Rect::new(r.x, r.y, r.w, 23.0), t.window_title);
    ui.c.text_clip(Font::new(17), r.x + 1.0, r.y + 2.0, r.w - 70.0, ui.t.tx(0xd4dade), &title);
    ui.hit(Rect::new(r.x, r.y, r.w, 23.0), Action::WinDrag(Win::Mixer), "Drag to move");
    window_buttons(ui, Win::Mixer, r.right() - 58.0, r.y, ui.t.tx(0xd4dade), ui.t.tx(0xffffff));
    if r.h < 2.0 * TITLE_H {
        return;
    }
    // Squish: the slot rows and the equalizer graph shrink with the height (the knob rows under
    // the graph are dropped when they no longer fit); the top and bottom rows keep their size.
    let room = r.h - 125.0;
    let kp = ((room - 15.0) / 503.0).clamp(0.3, 1.0);
    let knobs = room >= 579.0;
    let q = kp.max(0.55);
    // Input row: EQ glyph, "(none)" combo, latency clock. Audio inputs are not available yet: greyed out.
    let input_bg = ui.t.s(0x2b2e30);
    ui.c.fill_rect(Rect::new(r.x, r.y + 23.0, r.w, 36.0), input_bg);
    let mut g = Glyph::new();
    g.polygon(&[(r.x + 7.0, r.y + 32.0), (r.x + 13.0, r.y + 32.0), (r.x + 18.0, r.y + 37.5), (r.x + 13.0, r.y + 43.0), (r.x + 7.0, r.y + 43.0)]);
    ui.c.glyph(ui.t.s(0x40464a), &g);
    ui.c.text(Font::new(9).bold(), r.x + 6.0, r.y + 43.0, ui.t.s(0x40464a), "EQ");
    off_combo(ui, Rect::new(r.x + 27.0, r.y + 28.0, 159.0, 23.0), "Audio input source", input_bg);
    let mut g = Glyph::new();
    g.ring(r.x + 202.0, r.y + 39.5, 8.5, 1.8).rect(r.x + 201.2, r.y + 34.0, 1.6, 6.5).rect(r.x + 201.2, r.y + 38.9, 5.0, 1.6);
    ui.c.glyph(ui.t.tx(0xd4dade), &g);
    let clock = Rect::new(r.x + 192.0, r.y + 29.0, 21.0, 21.0);
    ui.off(clock, "Input latency");
    ui.veil(clock, input_bg);

    // Ten effect slots.
    for i in 0..10 {
        let row = Rect::new(r.x + 1.0, r.y + 61.0 + 33.0 * kp * i as f32, r.w - 2.0, 28.0 * kp);
        let k = i as u8;
        let plugin = mx::slot_name(s, n, k);
        let fs = (18.0 * kp.max(0.62)).round() as u16;
        let hint = format!("FX slot {} options", i + 1);
        let ha = ui.hit(Rect::new(row.x, row.y, 19.0, row.h), Action::Mix(MHit::SlotArrow(k)), hint.clone());
        let hn = ui.hit(Rect::new(row.x + 19.0, row.y, row.w - 64.0, row.h), Action::Mix(MHit::SlotName(k)), hint);
        let hov = ha || hn;
        if super::lemon(ui) {
            let face = if hov { ui.t.s(0x2f3335) } else { ui.t.s(0x2a2d2f) };
            lk::outlined(ui.c, Rect::new(row.x + 2.0, row.y, row.w - 4.0, row.h), lk::r_btn(t).min(row.h / 2.0), ui.t.s(0x1f2224), face);
        } else {
            ui.c.fill_rect(row, ui.t.s(0x1f2224));
            ui.c.fill_rect(row.inset(1.0), if hov { ui.t.s(0x2f3335) } else { ui.t.s(0x2a2d2f) });
        }
        chevron(ui, row.x + 10.0, row.cy(), true, if ha { ui.t.tx(0xffffff) } else { ui.t.tx(0xc4ccd0) });
        match &plugin {
            Some(p) => ui.c.text_clip(Font::new(fs), row.x + 19.0, row.y + 4.0 * kp, 140.0, ui.t.tx(0xc9d0d4), p),
            None => ui.c.text_clip(Font::new(fs).italic(), row.x + 19.0, row.y + 4.0 * kp, 140.0, ui.t.s(0x50575b), &format!("Slot {}", i + 1)),
        }
        let mix = KnobId::SlotMix(n, k);
        ui.hit(Rect::new(row.right() - 46.0, row.y, 26.0, row.h), Action::Knob(mix), s.knob_text(mix));
        mixer_knob(ui, row.right() - 33.0, row.cy(), 10.5 * q, 12.0 * q, s.knob(mix), Some(false));
        ui.c.stroke_circle(row.right() - 33.0, row.cy(), 11.8 * q, 1.5, ui.t.s(0x55595c));
        ui.hit(Rect::new(row.right() - 18.0, row.y, 18.0, row.h), Action::Mix(MHit::SlotLed(k)), "Mute / solo  (ctrl)0..9");
        led(ui, row.right() - 10.0, row.cy(), track.is_none_or(|m| m.slot_on[i]));
    }
    let sep = r.y + 61.0 + 330.0 * kp;
    ui.c.fill_rect(Rect::new(r.x, sep, r.w, 1.0), ui.t.s(0x1f2224));

    // Equalizer: response graph, three band gain faders (±18 dB), band frequency and width knobs (spec: ui/mixer.md §8).
    let eq = Rect::new(r.x + 4.0, sep + 11.0, 146.0, 173.0 * kp);
    let settings = track.map_or(os_model::EQ_DEFAULT, |m| m.eq);
    // Its hint follows the cursor (App::mouse_move).
    let hint = if eq.contains(ui.mouse.0, ui.mouse.1) { mx::eq_graph_hint(s, eq, ui.mouse.0) } else { String::new() };
    ui.hit(eq, Action::Mix(MHit::EqGraph), hint);
    if super::lemon(ui) {
        lk::outlined(ui.c, eq, lk::r_btn(t), ui.t.s(0x1f2224), ui.t.s(0x272a2c));
    } else {
        ui.c.fill_rect(eq, ui.t.s(0x1f2224));
        ui.c.fill_rect(eq.inset(1.0), ui.t.s(0x272a2c));
    }
    for k in 1..5 {
        ui.c.fill_rect(Rect::new(eq.x + 14.0 + 31.0 * k as f32, eq.y + 1.0, 1.0, eq.h - 2.0), ui.t.s(0x2b2e30));
    }
    // The response curve: the three bands' summed gain over the graph's width, which spans the frequency knob's
    // 10 Hz .. 16 kHz law; ±18 dB fills the graph's half-height.
    let half = eq.h / 2.0 - 4.0;
    let steps = (eq.w - 2.0) as usize;
    let pts: Vec<(f32, f32)> = (0..=steps)
        .map(|i| {
            let p = i as f32 / steps as f32;
            let hz = os_dsp::mixer::eq_frequency((p * 65536.0).round() as i32);
            let db = os_dsp::mixer::eq_response_db(settings, 44100, f64::from(hz)) as f32;
            (eq.x + 1.0 + p * (eq.w - 2.0), eq.cy() - (db / 18.0).clamp(-1.0, 1.0) * half) // as mx::eq_graph_db
        })
        .collect();
    ui.c.push_clip(eq.inset(1.0));
    ui.c.polyline(&pts, 2.0, ui.t.tx(0xc9cfd3));
    ui.c.pop_clip();
    for k in 0..3 {
        let fx = r.x + 162.0 + 22.0 * k as f32;
        ui.c.fill_rect(Rect::new(fx - 2.0, eq.y + 1.0, 4.0, eq.h - 2.0), ui.t.s(0x17191b));
        ui.c.fill_rect(Rect::new(fx - 9.0, eq.y + 36.0 * kp, 18.0, 1.0), ui.t.s(0x3a3e41));
        ui.c.fill_rect(Rect::new(fx - 9.0, eq.y + 136.0 * kp, 18.0, 1.0), ui.t.s(0x3a3e41));
        ui.c.fill_rect(Rect::new(fx - 9.0, eq.cy(), 18.0, 1.0), ui.t.s(0x111315));
        let id = KnobId::Eq(n, k as u8, 0);
        let hov = ui.hit(Rect::new(fx - 9.0, eq.y, 18.0, eq.h), Action::Knob(id), s.knob_text(id));
        // The handle travels ±(half-height − 8) for ±18 dB.
        let hy = eq.cy() - (s.knob(id) * 2.0 - 1.0) * (eq.h / 2.0 - 8.0);
        let h = Rect::new(fx - 6.0, hy - 6.0, 12.0, 13.0);
        let face = if hov { 0x585c5f } else { 0x4a4e51 };
        if super::lemon(ui) {
            ui.c.fill_rrect(h.outset(1.0), lk::r_small(t), ui.t.s(0x121416));
            ui.c.fill_rrect(h, lk::r_small(t) - 1.0, ui.t.s(face));
        } else {
            ui.c.fill_rrect(h.outset(1.0), 2.0, ui.t.s(0x121416));
            ui.c.fill_rect_gradient(h, ui.t.s(if hov { 0x6c7073 } else { 0x5e6265 }), ui.t.s(0x2a2d30), 2.0);
        }
        ui.c.fill_rect(Rect::new(fx - 5.0, hy, 10.0, 1.0), ui.t.s(0x111315));
    }
    let ly = eq.bottom() + 8.0;
    if knobs {
        ui.c.text(Font::new(17), r.x + 4.0, ly, ui.t.tx(0xa9b2b7), "Equalizer");
    }
    for row in 0..2 {
        if !knobs {
            break;
        }
        for k in 0..3 {
            let (kx, ky) = (r.x + 162.0 + 22.0 * k as f32, ly + 20.0 + 20.0 * row as f32);
            let id = KnobId::Eq(n, k as u8, 1 + row as u8);
            ui.hit(Rect::new(kx - 9.0, ky - 9.0, 18.0, 18.0), Action::Knob(id), s.knob_text(id));
            mixer_knob(ui, kx, ky, 7.0, 8.0, s.knob(id), Some(false));
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
        ui.c.glyph(ui.t.tx(0x8c969c), &g);
    }
    let b = r.bottom();

    // Output rows at the bottom: audio outputs are not available yet, greyed out.
    ui.c.fill_rect(Rect::new(r.x, b - 66.0, r.w, 1.0), ui.t.s(0x1f2224));
    ui.c.fill_rect(Rect::new(r.x, b - 33.0, r.w, 1.0), ui.t.s(0x1f2224));
    let mut g = Glyph::new();
    g.ring(r.x + 14.0, b - 48.5, 8.0, 1.5).rect(r.x + 13.3, b - 53.0, 1.4, 5.0).rect(r.x + 13.3, b - 49.2, 4.0, 1.4);
    ui.c.glyph(ui.t.s(0x40464a), &g);
    ui.off(Rect::new(r.x + 25.0, b - 60.0, r.w - 28.0, 22.0), "Audio output target");
    ui.c.text(Font::new(17), r.x + 30.0, b - 59.0, ui.t.tx(0xd4dade), "(none)");
    chevron(ui, r.right() - 11.0, b - 48.5, true, ui.t.tx(0x8c969c));
    ui.veil(Rect::span(r.x, r.right(), b - 65.0, b - 34.0), ui.t.s(0x2e3133));
    let mut g = Glyph::new();
    g.ring(r.x + 13.0, b - 16.0, 8.0, 1.6);
    g.erase(true).rect(r.x + 13.0, b - 26.0, 10.0, 20.0);
    g.erase(false).polygon(&[(r.x + 11.0, b - 20.5), (r.x + 16.0, b - 20.5), (r.x + 21.0, b - 16.0), (r.x + 16.0, b - 11.5), (r.x + 11.0, b - 11.5)]);
    ui.c.glyph(ui.t.tx(0xc4ccd0), &g);
    ui.veil(Rect::new(r.x + 3.0, b - 26.0, 20.0, 20.0), ui.t.s(0x2e3133));
    off_combo(ui, Rect::new(r.x + 26.0, b - 28.0, 189.0, 24.0), "Audio output target", ui.t.s(0x2e3133));
}

/// An input / output combo that is not available yet: drawn greyed out, its hit only says so.
fn off_combo(ui: &mut Ui, r: Rect, name: &'static str, bg: Color) {
    ui.off(r, name);
    let st = os_ui::kit::State::NORMAL.with_disabled(true);
    os_ui::kit::combo(ui.c, ui.t, r, "(none)", Font::new(17), st);
    ui.veil(r, bg);
}

/// Small solid chevron centred on (x, y), pointing right or left.
pub fn chevron(ui: &mut Ui, x: f32, y: f32, right: bool, c: Color) {
    let d = if right { 1.0 } else { -1.0 };
    let mut g = Glyph::new();
    g.polygon(&[(x - 2.5 * d, y - 4.5), (x + 2.5 * d, y), (x - 2.5 * d, y + 4.5)]);
    ui.c.glyph(c, &g);
}
