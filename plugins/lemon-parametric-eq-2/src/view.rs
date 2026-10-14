// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon Parametric EQ 2 editor: painting and hit regions (platform independent), built from `os_ui::kit` widgets.
//!
//! Coordinates are the editor content's (700 x 350 logical px) from the spec tables. Greys are palette roles so an
//! imported theme recolours the plugin like the rest of Lemon Studio; band colours are signal colours.
// spec: plugins/lemon-parametric-eq-2.md (§1 window, §2 graph, §3 faders and band column, §4 bottom row)

use std::f32::consts::PI;

use os_engine::effects::eq::{self, freq_hz, has_level, Settings, BANDS};
use os_theme::{Color, Palette};
use os_ui::kit::{self, eq as keq, State};
use os_ui::{Canvas, Font, Rect};

use crate::params::{self, byte, flag, Opt};

pub const W: u32 = 700;
pub const H: u32 = 350;

/// The graph's plot area: 20 Hz .. 20 kHz across, +18 .. -18 dB down.
pub const PLOT: Rect = Rect::span(7.0, 506.0, 33.0, 279.0);
pub const PLOT_X0: f32 = 7.0;
pub const PLOT_X1: f32 = 506.0;
pub const ZERO_Y: f32 = 156.0;
/// Pixels per dB on the graph and on every fader.
pub const PX_PER_DB: f32 = 246.0 / 36.0;
/// Columns the curves and the spectrum are computed for.
pub const COLS: usize = 500;

pub const BAND_COLORS: [u32; BANDS] = [0x8855cc, 0xc254a3, 0xbc6353, 0xb4b850, 0x56b24c, 0x42ad94, 0x446ea9];

pub const MAIN_X: f32 = 525.0;
pub const TRACK_TOP: f32 = 25.0;
pub const TRACK_BOTTOM: f32 = 286.0;
pub const FREQ_Y: f32 = 314.0;
pub const BW_Y: f32 = 336.0;
pub const KNOB_R: f32 = 8.0;

/// Left edge of band `k`'s column.
pub fn band_x(k: usize) -> f32 {
    549.0 + 21.0 * k as f32
}

pub fn x_of(n: f64) -> f32 {
    PLOT_X0 + (PLOT_X1 - PLOT_X0) * n.clamp(0.0, 1.0) as f32
}

pub fn n_of(x: f32) -> f64 {
    f64::from(((x - PLOT_X0) / (PLOT_X1 - PLOT_X0)).clamp(0.0, 1.0))
}

pub fn y_of(db: f64) -> f32 {
    ZERO_Y - db as f32 * PX_PER_DB
}

pub fn db_of(y: f32) -> f64 {
    f64::from((ZERO_Y - y) / PX_PER_DB)
}

pub fn band_color(k: usize) -> Color {
    Color::rgb(BAND_COLORS[k % BANDS])
}

/// What a region of the editor does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Graph,
    Token(usize),
    /// Band fader 0..6, 7 = main level.
    Fader(usize),
    TypeTab(usize),
    OrderMark(usize),
    Switch(usize),
    Freq(usize),
    Bw(usize),
    Options,
    Lin,
    Hq,
    Tokens,
    Phones,
    /// 0 off, 1 input, 2 output.
    Monitor(u8),
    Store,
    Flip,
}

pub const MAIN_FADER: usize = BANDS;

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub r: Rect,
    pub t: Target,
}

/// Editor-only state (not saved).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub mouse: Option<(f32, f32)>,
    /// Band whose token is hovered or dragged (its curve and readout show).
    pub active: Option<usize>,
    pub pressed: Option<Target>,
}

/// What the editor shows.
pub struct Look<'a> {
    /// Shown values (pending edits included), with the enable mask.
    pub s: Settings,
    /// The state image (options).
    pub extra: &'a [u8],
    /// Spectrum level per column (dB), when monitoring.
    pub spectrum: Option<&'a [f32]>,
    pub rate: f64,
}

/// The token's centre of band `k` (pass / band filters sit on the 0 dB line).
pub fn token_pos(s: &Settings, k: usize) -> (f32, f32) {
    let b = &s.bands[k];
    let db = if has_level(b.kind) { b.db } else { 0.0 };
    (x_of(b.freq), y_of(db))
}

fn st(vs: &ViewState, r: Rect, t: Target) -> State {
    let hover = vs.pressed.is_none() && vs.mouse.is_some_and(|(x, y)| r.contains(x, y));
    State::new(hover, vs.pressed == Some(t), false)
}

/// Octave boxes start at each C (C1 = 32.7 Hz).
fn c_hz(octave: i32) -> f64 {
    440.0 * 2f64.powf(f64::from(octave * 12 - 57) / 12.0)
}

pub fn paint(c: &mut Canvas, t: &Palette, look: &Look, vs: &ViewState) -> Vec<Hit> {
    let mut hits = Vec::new();
    c.begin_frame(None);
    c.fill_rect_gradient(Rect::new(0.0, 0.0, W as f32, H as f32), t.s(0x233642), t.s(0x0f1a21), 0.0);
    let small = Font::new(9).bold();
    let label = t.tx(0x9aa8b4);

    // Octave row (C1 .. C9) and frequency range row.
    let fx = |hz: f64| x_of(eq::freq_pos(hz));
    let mut edges = vec![PLOT_X0];
    edges.extend((1..=9).map(|o| fx(c_hz(o))));
    edges.push(PLOT_X1 + 1.0);
    for (i, w) in edges.windows(2).enumerate() {
        let r = Rect::new(w[0], 7.0, (w[1] - w[0] - 1.0).max(0.0), 10.0);
        c.fill_rect(r, t.s(0x303d46));
        if (1..=9).contains(&i) {
            c.text(small, r.x + 3.0, r.y - 1.0, label, &format!("C{i}"));
        }
    }
    let ranges = [(20.0, 40.0, ""), (40.0, 80.0, "SUB"), (80.0, 250.0, "BASS"), (250.0, 500.0, "LOW MID"), (500.0, 2000.0, "MID"), (2000.0, 4000.0, "HIGH MID"), (4000.0, 6000.0, "PRS"), (6000.0, 20000.0, "TREBLE")];
    for (a, b, name) in ranges {
        let (x0, x1) = (fx(a), if b >= 20000.0 { PLOT_X1 + 1.0 } else { fx(b) });
        let r = Rect::new(x0, 20.0, (x1 - x0 - 1.0).max(0.0), 10.0);
        c.fill_rect(r, t.s(0x2b3943));
        c.text_center(small, Rect::new(r.x, r.y - 1.0, r.w, r.h), label, name);
    }

    // Plot.
    let mid = Rect::new(PLOT.x, PLOT.y, PLOT.w, ZERO_Y - PLOT.y);
    c.fill_rect_gradient(mid, t.s(0x17262d), t.s(0x1f2c33), 0.0);
    c.fill_rect_gradient(Rect::new(PLOT.x, ZERO_Y, PLOT.w, PLOT.bottom() - ZERO_Y), t.s(0x1f2c33), t.s(0x1c292f), 0.0);
    hits.push(Hit { r: PLOT, t: Target::Graph });
    for i in 0..=24 {
        let db = 18.0 - 1.5 * f64::from(i);
        let y = y_of(db).round();
        let major = i % 4 == 0;
        c.fill_rect(Rect::new(PLOT.x, y, PLOT.w, 1.0), if major { t.s(0x2c3b44) } else { t.s(0x1d2b33) });
    }
    // Every 10 Hz to 100, every 100 Hz to 1k, every 1 kHz to 10k, then 20 kHz; decades brighter.
    let grid = [10.0, 100.0, 1000.0, 10000.0]
        .into_iter()
        .flat_map(|dec| (1..10).map(move |m| (dec * f64::from(m), m == 1)))
        .filter(|(hz, _)| (20.0..=20000.0).contains(hz));
    for (hz, decade) in grid {
        let x = fx(hz).round();
        c.fill_rect(Rect::new(x, PLOT.y, 1.0, PLOT.h), if decade { t.s(0x2e3f48) } else { t.s(0x24333b) });
    }

    // Spectrum (monitor on, histogram enabled).
    let range_db = [60.0, 90.0, 120.0][(params::opt(look.extra, Opt::Range) - 1).clamp(0, 2) as usize];
    if let Some(sp) = look.spectrum {
        let n = sp.len().max(2);
        let mut pts = vec![(PLOT.x, PLOT.bottom())];
        for (i, &db) in sp.iter().enumerate() {
            let x = PLOT.x + PLOT.w * i as f32 / (n - 1) as f32;
            let y = PLOT.y + PLOT.h * (-db / range_db).clamp(0.0, 1.0);
            pts.push((x, y));
        }
        pts.push((PLOT.right(), PLOT.bottom()));
        if flag(look.extra, params::F_HISTOGRAM) {
            c.fill_polygon(&pts, t.s(0x3a5a6a).with_alpha(115));
        }
        // Heatmap strip: the same levels as colour (top, bottom or behind the whole plot).
        if flag(look.extra, params::F_HEATMAP) {
            let pos = params::opt(look.extra, Opt::HeatmapPosition);
            let (y0, h, a) = match pos {
                0 => (PLOT.y, 12.0, 220.0),
                1 => (PLOT.bottom() - 12.0, 12.0, 220.0),
                _ => (PLOT.y, PLOT.h, 40.0),
            };
            let cell_x = |i: usize| PLOT.x + PLOT.w * i as f32 / n as f32;
            for (i, &db) in sp.iter().enumerate() {
                let x = cell_x(i);
                let v = (1.0 + db / range_db).clamp(0.0, 1.0);
                let col = heat(v).with_alpha((a * v) as u8);
                c.fill_cell(Rect::new(x, y0, cell_x(i + 1) - x, h), col);
            }
        }
    }

    // Curves.
    let s = &look.s;
    let curve_of = |f: &dyn Fn(f64) -> f64| -> Vec<(f32, f32)> {
        (0..COLS)
            .map(|i| {
                let n = i as f64 / (COLS - 1) as f64;
                let y = y_of(f(freq_hz(n))).clamp(PLOT.y - 2.0, PLOT.bottom() + 2.0);
                (x_of(n), y)
            })
            .collect()
    };
    let rate = look.rate.max(22050.0);
    if let Some(k) = vs.active.filter(|&k| s.bands[k].kind != eq::DISABLED) {
        let band = s.bands[k];
        let pts = curve_of(&|hz| band.response_db(hz, rate));
        let col = band_color(k);
        let mut fill = pts.clone();
        fill.push((PLOT.right(), PLOT.bottom()));
        fill.push((PLOT.x, PLOT.bottom()));
        c.push_clip(PLOT);
        c.fill_polygon(&fill, col.with_alpha(40));
        c.polyline(&pts, 1.5, col.shade(0.15));
        c.pop_clip();
    }
    let total = curve_of(&|hz| s.response_db(hz, rate));
    let mut fill = total.clone();
    fill.push((PLOT.right(), PLOT.bottom()));
    fill.push((PLOT.x, PLOT.bottom()));
    c.push_clip(PLOT);
    c.fill_polygon(&fill, t.tx(0xe8eef2).with_alpha(14));
    c.polyline(&total, 1.6, t.tx(0xe8eef2));
    c.pop_clip();

    // Frequency labels and the dB scale.
    let marks = [20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0];
    for w in marks.windows(2) {
        let (x0, x1) = (fx(w[0]), if w[1] >= 20000.0 { PLOT_X1 + 1.0 } else { fx(w[1]) });
        let r = Rect::new(x0, 282.0, (x1 - x0 - 1.0).max(0.0), 10.0);
        c.fill_rect(r, t.s(0x263138));
        let txt = if w[0] >= 1000.0 { format!("{}k", w[0] / 1000.0) } else { format!("{}", w[0]) };
        c.text(small, r.x + 3.0, r.y - 1.0, t.tx(0x7c8a96), &txt);
    }
    for db in [18, 12, 6, 0, -6, -12, -18] {
        let s = if db > 0 { format!("+{db}") } else { db.to_string() };
        c.text_right(Font::new(9), 522.0, y_of(f64::from(db)) - 6.0, t.tx(0x7c8a96), &s);
    }

    // Tokens (on top of the curves).
    if byte(look.extra, params::TOKENS_BYTE) != 0 {
        for k in 0..BANDS {
            let b = &s.bands[k];
            if b.kind == eq::DISABLED {
                continue;
            }
            let (x, y) = token_pos(s, k);
            let lit = !has_level(b.kind) || b.db.abs() > 0.005;
            let alpha = if !s.audible(k) { 0.35 } else if lit { 1.0 } else { 0.62 };
            let big = vs.active == Some(k);
            keq::token(c, t, x, y, k, band_color(k), alpha, big);
            hits.push(Hit { r: Rect::new(x - 10.0, y - 10.0, 20.0, 20.0), t: Target::Token(k) });
            if big {
                let f = Font::new(11);
                let col = band_color(k).shade(0.2);
                let ry = (y + 13.0).min(PLOT.bottom() - 26.0);
                c.text_center(f, Rect::new(x - 30.0, ry, 60.0, 13.0), col, &params::hz_text(b.hz()));
                c.text_center(f, Rect::new(x - 30.0, ry + 12.0, 60.0, 13.0), col, &format!("{:.0}%", b.width * 100.0));
            }
        }
    }

    // Main fader.
    let main_r = Rect::new(MAIN_X, TRACK_TOP, 13.0, TRACK_BOTTOM - TRACK_TOP + 1.0);
    let mst = st(vs, main_r, Target::Fader(MAIN_FADER));
    keq::slot(c, t, main_r, mst);
    keq::handle(c, t, main_r.cx(), y_of(s.main_db), mst);
    hits.push(Hit { r: main_r, t: Target::Fader(MAIN_FADER) });

    // Band columns.
    for k in 0..BANDS {
        let b = &s.bands[k];
        let x = band_x(k);
        let col = band_color(k);
        let track = Rect::new(x, 18.0, 13.0, TRACK_BOTTOM - 18.0 + 1.0);
        let fst = st(vs, track, Target::Fader(k));
        keq::slot(c, t, track, fst);
        let tab = Rect::new(x, 4.0, 13.0, 22.0);
        let tab_st = st(vs, tab, Target::TypeTab(k));
        let tab_col = if tab_st.hover || tab_st.pressed { col.shade(0.15) } else { col };
        c.fill_rrect(tab, 3.0, tab_col);
        c.fill_rrect(Rect::new(x + 1.5, 21.0, 10.0, 14.0), 5.0, t.s(0x1a2933));
        let ink = col.shade(-0.78);
        type_icon(c, b.kind, x + 6.5, 10.5, ink);
        order_mark(c, b.order, x + 6.5, 17.5, ink, b.kind);
        hits.push(Hit { r: Rect::new(x, 4.0, 13.0, 12.0), t: Target::TypeTab(k) });
        hits.push(Hit { r: Rect::new(x, 16.0, 13.0, 5.0), t: Target::OrderMark(k) });
        if has_level(b.kind) {
            keq::handle(c, t, track.cx(), y_of(b.db), fst);
            hits.push(Hit { r: Rect::new(x, TRACK_TOP + 2.0, 13.0, TRACK_BOTTOM - TRACK_TOP - 1.0), t: Target::Fader(k) });
        }
        // Mute / solo switch.
        let sw = Rect::new(x, 294.0, 13.0, 5.0);
        let swc = if s.audible(k) { col.mix(Color::rgb(0xffffff), 0.3) } else { t.s(0x3a454e) };
        let sst = st(vs, sw.outset(2.0), Target::Switch(k));
        c.fill_rect(sw, if sst.hover { swc.shade(0.15) } else { swc });
        c.fill_rect(Rect::new(sw.x, sw.bottom(), sw.w, 1.0), swc.shade(-0.5));
        hits.push(Hit { r: sw.outset(2.0), t: Target::Switch(k) });
        // Knobs.
        let cx = x + 6.5;
        for (y, tgt, v) in [(FREQ_Y, Target::Freq(k), b.freq), (BW_Y, Target::Bw(k), b.width)] {
            let r = Rect::new(cx - KNOB_R - 1.0, y - KNOB_R - 1.0, 2.0 * KNOB_R + 2.0, 2.0 * KNOB_R + 2.0);
            keq::arc_knob(c, t, cx, y, KNOB_R, v as f32, col, st(vs, r, tgt));
            hits.push(Hit { r, t: tgt });
        }
    }
    let cap = Font::condensed(12, 0).bold();
    c.text_right(cap, 545.0, FREQ_Y - 8.0, t.s(0x3b4d5a), "FREQ");
    c.text_right(cap, 545.0, BW_Y - 8.0, t.s(0x3b4d5a), "BW");
    let ph = Rect::new(524.0, 289.0, 16.0, 14.0);
    let pst = st(vs, ph, Target::Phones);
    let on = flag(look.extra, params::F_SOLO_SHIFT);
    let pcol = if on { t.tx(0xc8d2da) } else if pst.hover { t.tx(0x6f8290) } else { t.s(0x3b4d5a) };
    headphones(c, 532.0, 296.0, pcol);
    hits.push(Hit { r: ph, t: Target::Phones });

    // Wordmark.
    let wm = t.s(0x2f3d47);
    let x = 9.0 + c.text(Font::light(15, 0), 9.0, 316.0, wm, "LEMON PARAMETRIC");
    let x = x + 5.0 + c.text(Font::light(22, 0), x + 5.0, 310.0, wm, "EQ");
    c.text(Font::light(14, 0), x + 2.0, 322.0, wm, "2");

    // Bottom buttons.
    let mut button = |c: &mut Canvas, cx: f32, cy: f32, r: f32, tgt: Target, lit: bool| -> State {
        let rect = Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r);
        let s = st(vs, rect, tgt).with_on(lit);
        keq::round_button(c, t, cx, cy, r, s);
        hits.push(Hit { r: rect, t: tgt });
        s
    };
    let s1 = button(c, 261.0, 324.0, 11.0, Target::Options, false);
    kit::triangle(c, 262.0, 324.0, 6.0, kit::Dir::Right, keq::ink(t, s1));
    let lin = flag(look.extra, params::F_LIN);
    let s2 = button(c, 298.0, 328.0, 15.0, Target::Lin, lin);
    c.text_center(Font::condensed(13, 0).bold(), Rect::new(283.0, 320.0, 30.0, 16.0), keq::ink(t, s2), "LIN");
    let hq = byte(look.extra, params::HQ_BYTE);
    let s3 = button(c, 337.0, 328.0, 15.0, Target::Hq, hq != 0);
    let sub = if hq & 2 != 0 { "+" } else if flag(look.extra, params::F_HQ_ENHANCED) { "E" } else { "L" };
    let hx = c.text(Font::condensed(13, 0).bold(), 326.0, 319.0, keq::ink(t, s3), "HQ");
    c.text(Font::new(8).bold(), 326.0 + hx, 327.0, keq::ink(t, s3), sub);
    let tok = byte(look.extra, params::TOKENS_BYTE) != 0;
    let s4 = button(c, 377.0, 328.0, 15.0, Target::Tokens, false);
    let tc = if tok { keq::ink(t, s4.with_on(false)).shade(0.3) } else { keq::ink(t, s4).shade(-0.3) };
    kit::tick(c, Rect::new(370.0, 321.0, 14.0, 14.0), tc);
    let mon = byte(look.extra, params::MONITOR_BYTE);
    for (i, mx) in [409.0, 424.0, 440.0].into_iter().enumerate() {
        let s5 = button(c, mx, 322.0, 6.5, Target::Monitor(i as u8), mon == i as u8);
        let g = keq::ink(t, s5);
        match i {
            0 => c.fill_rect(Rect::new(mx - 2.5, 321.5, 5.0, 1.2), g),
            1 => c.fill_rect(Rect::new(mx - 0.6, 319.5, 1.2, 5.0), g),
            _ => c.stroke_circle(mx, 322.0, 2.5, 1.2, g),
        }
    }
    let lab = Font::condensed(11, 0).bold();
    c.text_center(lab, Rect::new(394.0, 333.0, 60.0, 13.0), t.tx(0x6c7c88), "MONITOR");
    let s6 = button(c, 467.0, 321.0, 10.0, Target::Store, false);
    kit::triangle(c, 467.0, 322.0, 6.0, kit::Dir::Down, keq::ink(t, s6));
    let s7 = button(c, 491.0, 321.0, 10.0, Target::Flip, false);
    kit::triangle(c, 491.0, 318.5, 5.0, kit::Dir::Up, keq::ink(t, s7));
    kit::triangle(c, 491.0, 324.0, 5.0, kit::Dir::Down, keq::ink(t, s7));
    c.text_center(lab, Rect::new(449.0, 333.0, 60.0, 13.0), t.tx(0x6c7c88), "COMPARE");
    hits
}

/// Heatmap colour for a level 0..1 (dark blue → teal → warm yellow).
fn heat(v: f32) -> Color {
    let stops = [(0.0, 0x10283a), (0.45, 0x1f6f80), (0.75, 0xc9b24a), (1.0, 0xfff1c0)];
    for w in stops.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if v <= b {
            return Color::rgb(ca).mix(Color::rgb(cb), (v - a) / (b - a));
        }
    }
    Color::rgb(0xfff1c0)
}

/// The type glyph on a band tab (spec §3): curves for pass filters, ≻ / ≺ for shelves, a star for peaking.
fn type_icon(c: &mut Canvas, kind: i32, cx: f32, cy: f32, col: Color) {
    let w = 1.3;
    match kind {
        eq::LOW_PASS => c.polyline(&[(cx - 4.5, cy - 2.0), (cx - 0.5, cy - 2.0), (cx + 2.0, cy - 0.5), (cx + 3.5, cy + 3.5)], w, col),
        eq::HIGH_PASS => c.polyline(&[(cx - 3.5, cy + 3.5), (cx - 2.0, cy - 0.5), (cx + 0.5, cy - 2.0), (cx + 4.5, cy - 2.0)], w, col),
        eq::BAND_PASS => c.polyline(&[(cx - 4.5, cy + 3.5), (cx - 2.5, cy - 1.0), (cx, cy - 2.5), (cx + 2.5, cy - 1.0), (cx + 4.5, cy + 3.5)], w, col),
        eq::BAND_STOP => {
            c.polyline(&[(cx - 4.0, cy - 3.5), (cx, cy + 0.5), (cx + 4.0, cy - 3.5)], w, col);
            c.line(cx, cy + 0.5, cx, cy + 4.0, w, col);
        }
        eq::LOW_SHELF => c.polyline(&[(cx - 4.0, cy - 3.5), (cx + 3.5, cy), (cx - 4.0, cy + 3.5)], w, col),
        eq::HIGH_SHELF => c.polyline(&[(cx + 4.0, cy - 3.5), (cx - 3.5, cy), (cx + 4.0, cy + 3.5)], w, col),
        eq::PEAKING => {
            let pts: Vec<(f32, f32)> = (0..8)
                .map(|i| {
                    let a = i as f32 * PI / 4.0 - PI / 2.0;
                    let r = if i % 2 == 0 { 4.5 } else { 1.3 };
                    (cx + r * a.cos(), cy + r * a.sin())
                })
                .collect();
            c.fill_polygon(&pts, col);
        }
        _ => c.fill_rect(Rect::new(cx - 3.0, cy - 0.6, 6.0, 1.3), col),
    }
}

/// The order mark under the type glyph: "2" one filled dot; Steep hollow, Gentle filled; 4 / 6 / 8 = 2 / 3 / 2x2 dots.
fn order_mark(c: &mut Canvas, order: i32, cx: f32, cy: f32, col: Color, kind: i32) {
    if !matches!(kind, eq::LOW_PASS | eq::HIGH_PASS) {
        // Only pass filters have orders; the original shows the "2" dot on every tab.
        c.fill_rect(Rect::new(cx - 1.0, cy - 1.0, 2.0, 2.0), col);
        return;
    }
    let dots: &[(f32, f32)] = match order.abs() {
        1 => &[(-1.6, 0.0), (1.6, 0.0)],
        2 => &[(-3.0, 0.0), (0.0, 0.0), (3.0, 0.0)],
        3 => &[(-1.6, -1.4), (1.6, -1.4), (-1.6, 1.4), (1.6, 1.4)],
        _ => &[(0.0, 0.0)],
    };
    for &(dx, dy) in dots {
        let (x, y) = (cx + dx, cy + dy);
        if order < 0 {
            c.stroke_rrect(Rect::new(x - 1.0, y - 1.0, 2.0, 2.0), 0.5, 0.8, col);
        } else {
            c.fill_rect(Rect::new(x - 1.0, y - 1.0, 2.0, 2.0), col);
        }
    }
}

/// Headphones glyph (solo on Shift).
fn headphones(c: &mut Canvas, cx: f32, cy: f32, col: Color) {
    c.arc(cx, cy + 1.0, 5.0, PI, PI, 1.4, col);
    c.fill_rrect(Rect::new(cx - 6.0, cy, 3.0, 5.0), 1.0, col);
    c.fill_rrect(Rect::new(cx + 3.0, cy, 3.0, 5.0), 1.0, col);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_follow_the_measurements() {
        assert_eq!(y_of(18.0).round(), 33.0);
        assert_eq!(y_of(-18.0).round(), 279.0);
        assert_eq!(x_of(0.0), 7.0);
        assert_eq!(x_of(1.0), 506.0);
        // 166 px per decade.
        assert!((x_of(eq::freq_pos(200.0)) - x_of(eq::freq_pos(20.0)) - 166.3).abs() < 0.5);
        // C1 box starts near x 42.
        assert!((x_of(eq::freq_pos(c_hz(1))) - 42.5).abs() < 1.0);
    }
}
