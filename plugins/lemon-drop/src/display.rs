// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The display: the glass box (background, punches, border, bevel or glossy reflection, the mirror image under it),
//! the title, and the mode's picture (oscilloscope, spectrum, meter, vectorscope) in the state's 28 colours.
//! Platform independent: it paints on an os-ui canvas in logical pixels.
//!
// spec: plugins/lemon-drop.md "Display", plugins/wave-candy.md "Display"

use os_ui::{Canvas, Color, Font, Rect};

use crate::analysis::{self, Analyzer, F_HIGH, F_LOW, FLOOR_DB};
use crate::state::*;

/// A target's colour as drawn: its colour with its opacities folded in, times `fade`.
pub fn color(st: &State, k: usize, fade: f32) -> Color {
    let p = st.paints[k];
    let [r, g, b] = p.rgb();
    let q = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    Color { r: q(r), g: q(g), b: q(b), a: q(p.alpha() * fade) }
}

/// A gradient colour (spectrum 0 .. 4, meter 0 .. 4) with its alpha, premultiplied word as stored.
fn gradient(st: &State, first: usize, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0) * 4.0;
    let i = (t.floor() as usize).min(3);
    let f = t - i as f32;
    let a = color(st, first + i, 1.0);
    let b = color(st, first + i + 1, 1.0);
    a.mix(b, f)
}

/// What the display needs besides the state and the analysis.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointer {
    /// The mouse over the display (logical px), when it is.
    pub at: Option<(f32, f32)>,
    /// The display is held (frozen).
    pub held: bool,
}

/// The stored display window's width over its height (the default's when the stored one makes no sense).
pub fn window_aspect(st: &State) -> f32 {
    let [l, t, r, b] = st.window;
    let (w, h) = ((r - l) as f32, (b - t) as f32);
    if w >= 8.0 && h >= 8.0 { w / h } else { 301.0 / 125.0 }
}

/// The box inside `area`: the stored window's shape, as large as fits over the room the mirror image takes under it,
/// centred; never narrower than 72 px.
pub fn box_rect(st: &State, area: Rect) -> Rect {
    let mirror = if st.bevel > 0 { 0.2 } else { 0.0 };
    let room_h = (area.h / (1.0 + mirror)).floor();
    let aspect = window_aspect(st);
    let (mut w, mut h) = (area.w, area.w / aspect);
    if h > room_h {
        h = room_h;
        w = (h * aspect).max(72.0).min(area.w);
    }
    Rect::new((area.x + (area.w - w) / 2.0).round(), area.y, w.round(), h.round())
}

/// The mode shown: the state's, or with auto visual mode, between the oscilloscope and the meter by the stored
/// window's shape: tall and narrow (1.5 times as tall as wide), the meter; short and wide (1.5 times as wide), the
/// oscilloscope. A spectrum or a vectorscope stays (the original's tall spectrum and square vectorscope presets do).
pub fn shown_mode(st: &State, _b: Rect) -> Mode {
    let a = window_aspect(st);
    let auto = st.auto_mode && matches!(st.mode, Mode::Oscilloscope | Mode::Meter);
    if auto && a < 1.0 / 1.5 {
        Mode::Meter
    } else if auto && a > 1.5 {
        Mode::Oscilloscope
    } else {
        st.mode
    }
}

/// A glowing light: a soft round spot of colour `c`, brightest in the middle, gone at radius `r`. The target's
/// opacity sets the spot's strength (its object opacity is low: the light is built of many faint discs).
fn light(cv: &mut Canvas, x: f32, y: f32, r: f32, c: Color) {
    if c.a == 0 {
        return;
    }
    const RINGS: usize = 10;
    let a = ((c.a as f32 * 6.0).min(230.0) / RINGS as f32).max(1.0) as u8;
    for i in 0..RINGS {
        let k = 1.0 - i as f32 / RINGS as f32;
        cv.fill_circle(x, y, r * k * k.sqrt(), c.with_alpha(a));
    }
}

fn radius(st: &State) -> f32 {
    st.corners.clamp(0, 512) as f32 / 512.0 * 12.0 * 1.5
}

/// The frame's width: the bevel when the knob is on the bevel side, a thin border otherwise.
fn frame(st: &State) -> f32 {
    if st.bevel < 0 { 3.0 + (-st.bevel).min(256) as f32 / 256.0 * 9.0 } else { 6.0 }
}

/// The content rect (inside the frame).
pub fn inner(st: &State, b: Rect) -> Rect {
    b.inset(frame(st))
}

pub fn paint(c: &mut Canvas, st: &State, a: &Analyzer, area: Rect, ptr: Pointer) {
    let b = box_rect(st, area);
    let fade = a.fade(st);
    let rad = radius(st);
    let fr = frame(st);
    let inn = b.inset(fr);
    // The mirror image under the box.
    if st.bevel > 0 && st.visible {
        mirror(c, st, b, area, rad);
    }
    if !st.visible {
        return;
    }
    if st.background {
        c.stroke_rrect(b.inset(fr / 2.0), (rad - fr / 2.0).max(0.0), fr, color(st, BORDER, 1.0));
        c.fill_rrect(inn, (rad - fr).max(0.0), color(st, BACKGROUND, 1.0));
        punches(c, st, inn);
        if st.bevel < 0 {
            bevel(c, st, b, fr, rad);
        }
    }
    c.push_clip(inn);
    let mode = shown_mode(st, b);
    match mode {
        Mode::Oscilloscope => scope(c, st, a, inn, fade),
        Mode::Spectrum => spectrum(c, st, a, inn, fade),
        Mode::Meter => meter(c, st, a, inn, fade),
        Mode::Vectorscope => vector(c, st, a, inn, fade),
    }
    if st.background && st.bevel >= 0 {
        gloss(c, st, inn);
    }
    title(c, st, inn);
    if let Some((x, y)) = ptr.at.filter(|&(x, y)| inn.contains(x, y)) {
        hint(c, st, a, inn, mode, x, y);
    }
    if ptr.held {
        c.text_right(Font::new(9), inn.right() - 8.0, inn.bottom() - 16.0, color(st, LABELS, 1.0), "HOLD");
    }
    c.pop_clip();
    if st.glow && st.background {
        let g = color(st, BORDER, 1.0).with_alpha(255);
        c.stroke_rrect(b.inset(0.5), rad, 1.0, g.mix(color(st, GLOW, 1.0).with_alpha(255), 0.6).with_alpha(160));
    }
}

/// The glass's gloss: the upper part of the box lit from the reflection's top colour fading toward its bottom
/// colour, over 62 % of the height; the glass effect colour along its lower edge.
fn gloss(c: &mut Canvas, st: &State, r: Rect) {
    let k = st.bevel.clamp(0, 256) as f32 / 64.0;
    let top = color(st, REFLECTION_TOP, k.min(1.0));
    let bottom = color(st, REFLECTION_BOTTOM, 0.0);
    let g = Rect::new(r.x, r.y, r.w, (r.h * 0.62).round());
    c.fill_rect_gradient(g, top, top.with_alpha(0).mix(bottom, 0.0), radius(st).min(8.0));
    let glass = color(st, GLASS, 1.0);
    c.fill_rect(Rect::new(r.x, g.bottom() - 1.0, r.w, 1.0), glass);
}

/// The bevel: the top and left edges in the bevel top colour, the bottom and right in the bevel bottom colour.
fn bevel(c: &mut Canvas, st: &State, b: Rect, fr: f32, rad: f32) {
    let top = color(st, BEVEL_TOP, 1.0);
    let bottom = color(st, BEVEL_BOTTOM, 1.0);
    c.fill_rect_gradient(Rect::new(b.x, b.y, b.w, fr), bottom, top, rad.min(fr));
    c.fill_rect_gradient(Rect::new(b.x, b.bottom() - fr, b.w, fr), bottom, top, rad.min(fr));
}

/// Decorations punched in the background: a grid of squares (positive size) or circles (negative).
fn punches(c: &mut Canvas, st: &State, r: Rect) {
    if st.punches == 0 {
        return;
    }
    let s = st.punches.unsigned_abs().min(24) as f32;
    let col = color(st, PUNCHES, 1.0);
    let step = s * 2.0;
    let mut y = r.y + s / 2.0;
    while y + s <= r.bottom() {
        let mut x = r.x + s / 2.0;
        while x + s <= r.right() {
            if st.punches > 0 {
                c.fill_rect(Rect::new(x, y, s, s), col);
            } else {
                c.fill_circle(x + s / 2.0, y + s / 2.0, s / 2.0, col);
            }
            x += step;
        }
        y += step;
    }
}

/// The mirror image: the box's shape again under it, upside down and fading out.
fn mirror(c: &mut Canvas, st: &State, b: Rect, area: Rect, rad: f32) {
    let h = (area.bottom() - b.bottom()).max(0.0);
    if h < 2.0 {
        return;
    }
    let k = st.bevel.clamp(0, 256) as f32 / 256.0;
    let r = Rect::new(b.x, b.bottom() + 2.0, b.w, h - 2.0);
    let border = color(st, BORDER, k);
    c.push_clip(r);
    c.fill_rect_gradient(Rect::new(b.x, r.y, b.w, r.h), border, border.with_alpha(0), rad);
    c.pop_clip();
}

fn title(c: &mut Canvas, st: &State, r: Rect) {
    if st.title.is_empty() {
        return;
    }
    let size = (st.font_height.unsigned_abs() as f32 * 0.9).clamp(8.0, 40.0) as u16;
    let f = Font::new(size);
    c.text_right(f, r.right() - 8.0, r.y + 6.0, color(st, TITLE, 1.0), &st.title_text());
}

// ---- Oscilloscope

/// The pinch: the trace's ends drawn in to the centre line over the outer tenth of the width.
fn pinch(x: f32) -> f32 {
    let e = x.min(1.0 - x) / 0.1;
    if e >= 1.0 { 1.0 } else { 0.5 - 0.5 * (std::f32::consts::PI * e.max(0.0)).cos() }
}

fn scope(c: &mut Canvas, st: &State, a: &Analyzer, r: Rect, fade: f32) {
    let ruler = color(st, SCOPE_RULER, 1.0);
    let (cx, cy) = (r.cx(), r.cy());
    c.fill_rect(Rect::new(r.x, cy.round(), r.w, 1.0), ruler);
    c.fill_rect(Rect::new(cx.round(), r.y + r.h * 0.05, 1.0, r.h * 0.9), ruler);
    let tick = r.h / 30.0;
    let mut k = 1.0;
    while k * tick < r.h * 0.45 {
        for s in [-1.0, 1.0] {
            c.fill_rect(Rect::new(cx - 6.0, (cy + s * k * tick).round(), 13.0, 1.0), ruler.with_alpha(ruler.a / 2));
        }
        k += 1.0;
    }
    for s in [-1.0, 1.0] {
        c.fill_rect(Rect::new(cx - r.w * 0.1, (cy + s * r.h / 4.0).round(), r.w * 0.2, 1.0), ruler);
    }
    let (l, rr) = a.trace(st);
    let sep = if st.separation > 0 { st.separation.min(1000) as f32 / 1000.0 } else { 0.0 };
    let wave = color(st, WAVEFORM, fade);
    let glow = color(st, GLOW, fade);
    let half = r.h / 2.0;
    let mut draw = |pts: &[f32], y0: f32, amp: f32| {
        let n = pts.len().max(2);
        let cols = (r.w * 2.0) as usize;
        let mut line = Vec::with_capacity(cols.min(n) + 2);
        if n > cols {
            // Several samples per column: their extremes.
            for i in 0..cols {
                let a0 = i * n / cols;
                let a1 = ((i + 1) * n / cols).max(a0 + 1).min(n);
                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                for &v in &pts[a0..a1] {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                let fx = i as f32 / cols as f32;
                let p = if st.pinch { pinch(fx) } else { 1.0 };
                let x = r.x + fx * r.w;
                line.push((x, y0 - hi * amp * p));
                if hi - lo > 1e-4 {
                    line.push((x, y0 - lo * amp * p));
                }
            }
        } else {
            let steps = if st.interpolate { 4 } else { 1 };
            for i in 0..n - 1 {
                for s in 0..steps {
                    let t = s as f32 / steps as f32;
                    let v = if st.interpolate { cubic(pts, i, t) } else { pts[i] };
                    let fx = (i as f32 + t) / (n - 1) as f32;
                    let p = if st.pinch { pinch(fx) } else { 1.0 };
                    line.push((r.x + fx * r.w, y0 - v * amp * p));
                    if !st.interpolate {
                        let fx2 = (i + 1) as f32 / (n - 1) as f32;
                        line.push((r.x + fx2 * r.w, y0 - v * amp * p));
                    }
                }
            }
            if let Some(&v) = pts.last() {
                let p = if st.pinch { pinch(1.0) } else { 1.0 };
                line.push((r.right(), y0 - v * amp * p));
            }
        }
        c.polyline(&line, 1.5, wave);
        // The glowing lights where the trace meets the frame.
        if glow.a > 0 {
            for x in [r.x, r.right()] {
                light(c, x, y0, 16.0, glow);
            }
        }
    };
    if sep > 0.0 {
        let amp = half * (1.0 - sep / 2.0);
        draw(&l, cy - sep * r.h / 4.0, amp);
        draw(&rr, cy + sep * r.h / 4.0, amp);
        let lab = color(st, LABELS, 1.0);
        c.text(Font::new(9), r.x + 8.0, cy - sep * r.h / 4.0 - amp * 0.9, lab, "L");
        c.text(Font::new(9), r.x + 8.0, cy + sep * r.h / 4.0 + amp * 0.6, lab, "R");
    } else {
        let m: Vec<f32> = l.iter().zip(&rr).map(|(a, b)| 0.5 * (a + b)).collect();
        draw(&m, cy, half);
    }
}

/// Catmull-Rom between `p[i]` and `p[i + 1]`.
fn cubic(p: &[f32], i: usize, t: f32) -> f32 {
    let g = |k: isize| p[k.clamp(0, p.len() as isize - 1) as usize];
    let i = i as isize;
    let (p0, p1, p2, p3) = (g(i - 1), g(i), g(i + 1), g(i + 2));
    0.5 * (2.0 * p1 + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
}

// ---- Spectrum

/// The spectrum's label column width.
pub const SPECTRUM_LABELS: f32 = 52.0;

fn spectrum(c: &mut Canvas, st: &State, a: &Analyzer, r: Rect, fade: f32) {
    let lab = color(st, LABELS, 1.0);
    let ruler = color(st, SCOPE_RULER, 1.0);
    let plot = Rect::new(r.x + SPECTRUM_LABELS, r.y + 6.0, r.w - SPECTRUM_LABELS - 4.0, r.h - 12.0);
    // Frequency labels: 20 Hz and round frequencies, kept 14 px apart; the octaves' C notes further left.
    let y_of = |f: f32| plot.bottom() - analysis::freq_pos(f, st.spectrum_curve) * plot.h;
    c.fill_rect(Rect::new(plot.x - 4.0, plot.y, 1.0, plot.h), ruler);
    let mut last = f32::MAX;
    for &f in &[20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 15000.0, 20000.0] {
        let y = y_of(f);
        if last - y < 14.0 && f != 20.0 {
            continue;
        }
        last = y;
        let (n, unit) = if f >= 1000.0 { (format!("{}k", f / 1000.0), "Hz") } else { (format!("{f}"), "Hz") };
        let w = c.text_width(Font::new(9), unit);
        c.text_right(Font::new(9), plot.x - 6.0 - w, y - 6.0, lab, &n);
        c.text(Font::new(9), plot.x - 6.0 - w, y - 6.0, lab.with_alpha(lab.a / 2), unit);
        c.fill_rect(Rect::new(plot.x - 6.0, y.round(), 3.0, 1.0), ruler);
    }
    let mut last = f32::MAX;
    for oct in 1..=10 {
        let f = 32.703 * 2f32.powi(oct - 1);
        if !(F_LOW..=F_HIGH).contains(&f) {
            continue;
        }
        let y = y_of(f);
        if last - y < 12.0 {
            continue;
        }
        last = y;
        c.text(Font::new(8), r.x + 4.0, y - 5.0, lab, &format!("C{oct}"));
    }
    // The columns, newest at the right, one per logical px.
    let cols = plot.w.max(1.0) as usize;
    let rows = plot.h.max(1.0) as usize;
    let range = st.spectrum_range.clamp(48, 144) as f32;
    let have = a.columns.len().min(cols);
    if have == 0 {
        keyboard(c, st, plot, lab);
        return;
    }
    let mut px = vec![0u32; have * rows];
    // Each row's frequency span, as bins (filled per column).
    let bins: Vec<(f32, f32)> = (0..rows)
        .map(|y| {
            let p0 = 1.0 - (y as f32 + 1.0) / rows as f32;
            let p1 = 1.0 - y as f32 / rows as f32;
            (analysis::pos_freq(p0, st.spectrum_curve), analysis::pos_freq(p1, st.spectrum_curve))
        })
        .collect();
    let bg = color(st, BACKGROUND, 1.0);
    let back = Color::rgb(0x2b2b2b).mix(Color::rgb(0x000000), bg.a as f32 / 255.0);
    for (x, col) in a.columns.iter().skip(a.columns.len() - have).enumerate() {
        for (y, &(f0, f1)) in bins.iter().enumerate() {
            let k0 = (f0 / col.hz_per_bin) as usize;
            let k1 = ((f1 / col.hz_per_bin).ceil() as usize).max(k0 + 1).min(col.db.len());
            let mut d = FLOOR_DB;
            for k in k0.min(col.db.len().saturating_sub(1))..k1 {
                d = d.max(col.db[k]);
            }
            let g = gradient(st, SPECTRUM, 1.0 + d / range);
            let g = g.with_alpha((g.a as f32 * fade) as u8);
            let o = back.mix(g.with_alpha(255), g.a as f32 / 255.0);
            px[y * have + x] = (u32::from(o.r) << 16) | (u32::from(o.g) << 8) | u32::from(o.b);
        }
    }
    let w = have as f32;
    c.image(Rect::new(plot.right() - w, plot.y, w, plot.h), have, rows, &px);
    keyboard(c, st, plot, lab);
}

/// Spectrum scale fully right: a piano keyboard along the axis at the plot's left edge, keys at their notes.
fn keyboard(c: &mut Canvas, st: &State, plot: Rect, lab: Color) {
    if st.spectrum_curve < 256 {
        return;
    }
    let y_of = |f: f32| plot.bottom() - analysis::freq_pos(f, st.spectrum_curve) * plot.h;
    // Scale fully right: a piano keyboard along the axis, its keys at their notes' frequencies.
    let kb = Rect::new(plot.x, plot.y, 8.0, plot.h);
    c.fill_rect(kb, Color::rgb(0xe8e8e8).with_alpha(lab.a.max(90)));
    for n in 12..=132 {
        let f = 440.0 * 2f32.powf((n as f32 - 69.0) / 12.0);
        if !(F_LOW..=F_HIGH).contains(&f) {
            continue;
        }
        let y = y_of(f);
        let h = (y_of(f / 2f32.powf(1.0 / 24.0)) - y_of(f * 2f32.powf(1.0 / 24.0))).max(1.0);
        if matches!(n % 12, 1 | 3 | 6 | 8 | 10) {
            c.fill_rect(Rect::new(kb.x, y - h / 2.0, kb.w * 0.6, h), Color::rgb(0x202020));
        } else if h > 2.5 {
            c.fill_rect(Rect::new(kb.x, (y + h / 2.0).round(), kb.w, 1.0), Color::rgb(0x8a8a8a));
        }
    }
}

// ---- Meter

/// Where a level sits on the meter, 0 (top) .. 1 (bottom), for scale 0 .. 256: tables measured on the original at
/// scale 0, 128 and 256 (dB, place), mixed between.
const SCALE_0: [(f32, f32); 17] = [
    (1.0, 0.132),
    (0.0, 0.222),
    (-1.0, 0.302),
    (-2.0, 0.373),
    (-3.0, 0.437),
    (-4.0, 0.493),
    (-5.0, 0.545),
    (-6.0, 0.59),
    (-7.0, 0.632),
    (-8.0, 0.668),
    (-9.0, 0.7),
    (-11.0, 0.752),
    (-13.0, 0.795),
    (-15.0, 0.828),
    (-18.0, 0.868),
    (-22.0, 0.903),
    (-29.0, 0.937),
];
const SCALE_128: [(f32, f32); 21] = [
    (6.0, 0.037),
    (5.0, 0.068),
    (4.0, 0.1),
    (2.0, 0.16),
    (0.0, 0.222),
    (-2.0, 0.282),
    (-4.0, 0.34),
    (-6.0, 0.397),
    (-8.0, 0.452),
    (-10.0, 0.505),
    (-12.0, 0.555),
    (-14.0, 0.605),
    (-16.0, 0.65),
    (-18.0, 0.69),
    (-20.0, 0.728),
    (-22.0, 0.763),
    (-25.0, 0.805),
    (-28.0, 0.845),
    (-32.0, 0.882),
    (-37.0, 0.913),
    (-47.0, 0.945),
];
const SCALE_256: [(f32, f32); 26] = [
    (15.0, 0.037),
    (12.0, 0.073),
    (9.0, 0.112),
    (6.0, 0.148),
    (3.0, 0.185),
    (0.0, 0.222),
    (-3.0, 0.258),
    (-6.0, 0.295),
    (-9.0, 0.332),
    (-12.0, 0.368),
    (-15.0, 0.405),
    (-18.0, 0.442),
    (-21.0, 0.478),
    (-24.0, 0.515),
    (-27.0, 0.552),
    (-30.0, 0.588),
    (-33.0, 0.623),
    (-36.0, 0.657),
    (-39.0, 0.69),
    (-42.0, 0.723),
    (-45.0, 0.758),
    (-49.0, 0.797),
    (-53.0, 0.835),
    (-57.0, 0.868),
    (-63.0, 0.905),
    (-72.0, 0.937),
];
/// Where silence sits.
const METER_FLOOR: f32 = 0.963;

fn table_place(t: &[(f32, f32)], db: f32) -> f32 {
    if db >= t[0].0 {
        // Above the table: on with the first step's slope.
        let s = (t[1].1 - t[0].1) / (t[1].0 - t[0].0);
        return t[0].1 + (db - t[0].0) * s;
    }
    for w in t.windows(2) {
        let ((d0, p0), (d1, p1)) = (w[0], w[1]);
        if db >= d1 {
            return p0 + (p1 - p0) * (db - d0) / (d1 - d0);
        }
    }
    // Below: toward the floor as the amplitude goes to nothing.
    let (dl, pl) = t[t.len() - 1];
    let a = 10f32.powf((db - dl) / 20.0);
    METER_FLOOR - (METER_FLOOR - pl) * a
}

pub fn meter_place(db: f32, scale: i32) -> f32 {
    let s = scale.clamp(0, 256) as f32 / 128.0;
    let (a, b, f) = if s <= 1.0 { (&SCALE_0[..], &SCALE_128[..], s) } else { (&SCALE_128[..], &SCALE_256[..], s - 1.0) };
    let p = table_place(a, db) * (1.0 - f) + table_place(b, db) * f;
    p.clamp(-0.5, METER_FLOOR)
}

/// The meter's colour at level `db`: the gradient's -INF .. -6 dB .. -0 dB, then +0 dB .. +6 dB.
fn meter_color(st: &State, db: f32) -> Color {
    let pick = |k: usize| color(st, METER + k, 1.0);
    if db >= 0.0 {
        pick(3).mix(pick(4), (db / 6.0).clamp(0.0, 1.0))
    } else if db >= -6.0 {
        pick(1).mix(pick(2), (db + 6.0) / 6.0)
    } else {
        pick(0).mix(pick(1), ((db + 60.0) / 54.0).clamp(0.0, 1.0))
    }
}

fn meter(c: &mut Canvas, st: &State, a: &Analyzer, r: Rect, fade: f32) {
    let lab = color(st, LABELS, 1.0);
    let ruler = color(st, SCOPE_RULER, 1.0);
    // The labels take at most 40 % of a narrow box (their text shrinks with it).
    let labels_w = (r.w * 0.4).min(44.0);
    let plot = Rect::new(r.x + labels_w, r.y, r.w - labels_w - 6.0, r.h - 4.0);
    let y_of = |db: f32| plot.y + meter_place(db, st.meter_scale) * plot.h;
    // dB labels: 0 dB always, then every dB outward from it (up, then down), kept 12 px from those already placed.
    let mut placed: Vec<f32> = Vec::new();
    let order = std::iter::once(0.0).chain((1..=15).map(|d| d as f32)).chain((1..=96).map(|d| -(d as f32)));
    for d in order {
        let y = y_of(d);
        if y < r.y + 2.0 || y >= plot.bottom() - 6.0 || placed.iter().any(|&p| (p - y).abs() < 12.0) {
            continue;
        }
        placed.push(y);
        let s = if d > 0.0 { format!("+{d}") } else { format!("{d}") };
        let unit = "dB";
        let uw = c.text_width(Font::new(8), unit);
        c.text_right(Font::new(9), plot.x - 6.0 - uw - 2.0, y - 6.0, lab, &s);
        c.text(Font::new(8), plot.x - 6.0 - uw, y - 5.0, lab.with_alpha(lab.a / 2), unit);
        c.fill_rect(Rect::new(plot.x - 4.0, y.round(), 3.0, 1.0), ruler);
    }
    c.fill_rect(Rect::new(plot.x - 2.0, r.y + 4.0, 1.0, r.h - 8.0), ruler);
    let gap = 4.0;
    let bw = (plot.w - gap) / 2.0;
    for ch in 0..2 {
        let bar = Rect::new(plot.x + ch as f32 * (bw + gap), plot.y, bw, plot.h);
        c.fill_rect(bar, color(st, METER_BACKGROUND, 1.0));
        let level = a.meter[ch];
        let top = y_of(level).max(bar.y);
        // The bar, coloured by level along its height.
        let mut y = bar.bottom();
        while y > top {
            let y1 = (y - 2.0).max(top);
            let db = level_at(st, plot, (y + y1) / 2.0);
            let col = meter_color(st, db);
            c.fill_rect(Rect::new(bar.x, y1, bar.w, y - y1), col.with_alpha((col.a as f32 * fade) as u8));
            y = y1;
        }
        let h = a.hold[ch].db;
        if h > FLOOR_DB + 1.0 && st.meter_hold > 0 {
            let hy = y_of(h);
            let col = meter_color(st, h);
            c.fill_rect(Rect::new(bar.x, (hy - 1.0).round(), bar.w, 2.0), col.with_alpha((col.a as f32 * fade) as u8));
        }
    }
    // The glowing lights at 0 dB.
    let glow = color(st, GLOW, fade);
    if glow.a > 0 {
        let y0 = y_of(0.0);
        for x in [r.x, r.right()] {
            light(c, x, y0, 16.0, glow);
        }
    }
}

/// The level at height `y` on the meter (by bisection of `meter_place`).
fn level_at(st: &State, plot: Rect, y: f32) -> f32 {
    let p = (y - plot.y) / plot.h;
    let (mut lo, mut hi) = (-120.0f32, 24.0f32);
    for _ in 0..24 {
        let m = 0.5 * (lo + hi);
        if meter_place(m, st.meter_scale) > p {
            lo = m;
        } else {
            hi = m;
        }
    }
    0.5 * (lo + hi)
}

// ---- Vectorscope

fn vector(c: &mut Canvas, st: &State, a: &Analyzer, r: Rect, fade: f32) {
    let ruler = color(st, VECTOR_RULER, 1.0);
    let lab = color(st, LABELS, 1.0);
    let (cx, cy) = (r.cx(), r.cy());
    // Dashed spokes every 22.5°.
    for k in 0..16 {
        let ang = k as f32 * std::f32::consts::PI / 8.0;
        let (dx, dy) = (ang.cos() * r.w / 2.0, ang.sin() * r.h / 2.0);
        let mut t: f32 = 0.12;
        while t < 0.92 {
            let t1 = (t + 0.06).min(0.92);
            c.line(cx + dx * t, cy + dy * t, cx + dx * t1, cy + dy * t1, 2.0, ruler.with_alpha(ruler.a / 2));
            t += 0.1;
        }
    }
    let f = Font::new(9);
    c.text(f, r.x + r.w * 0.12, r.y + 8.0, lab, "+ L");
    c.text_right(f, r.right() - r.w * 0.12, r.y + 8.0, lab, "+ R");
    c.text(f, r.x + r.w * 0.12, r.bottom() - 20.0, lab, "- R");
    c.text_right(f, r.right() - r.w * 0.12, r.bottom() - 20.0, lab, "- L");
    let pts: Vec<(f32, f32)> = a.lissajous(st).iter().map(|&(s, m)| (cx + s * r.w / 2.0, cy - m * r.h / 2.0)).collect();
    if pts.len() < 2 {
        return;
    }
    let thick = st.vector_thickness.clamp(100, 600) as f32 / 100.0;
    let wave = color(st, WAVEFORM, fade);
    let liss = color(st, LISSAJOU, fade);
    let glow = color(st, GLOW, fade);
    match st.vector_mode {
        // String: a hairline.
        0 => c.polyline(&pts, 1.0, wave),
        // Spaghetti: the line at its thickness.
        1 => c.polyline(&pts, thick, wave),
        // Glowstick: a wide soft glow under a bright core.
        3 => {
            c.polyline(&pts, thick * 3.0, wave.with_alpha(wave.a / 6));
            c.polyline(&pts, thick * 1.6, glow.with_alpha(glow.a.saturating_mul(3)));
            c.polyline(&pts, thick * 0.6, wave);
        }
        // Tube: a dark edge, the body, a light core.
        _ => {
            c.polyline(&pts, thick + 1.5, Color::rgb(0x000000).with_alpha(wave.a / 2));
            c.polyline(&pts, thick, wave.mix(Color::rgb(0x000000).with_alpha(wave.a), 0.25));
            c.polyline(&pts, (thick * 0.4).max(1.0), wave);
        }
    }
    if liss.a > 0 {
        c.polyline(&pts, thick * 2.0, liss);
    }
}

// ---- Hint

/// The reading under the pointer: a cross in the hint location colour and the value in the hint colour.
fn hint(c: &mut Canvas, st: &State, _a: &Analyzer, r: Rect, mode: Mode, x: f32, y: f32) {
    let loc = color(st, HINT_LOCATION, 1.0);
    let text = match mode {
        Mode::Oscilloscope => {
            let t = (x - r.x) / r.w * analysis::scope_span(st) * 1000.0;
            let v = (r.cy() - y) / (r.h / 2.0);
            format!("{t:.1} ms  {v:+.2}")
        }
        Mode::Spectrum => {
            let plot_y0 = r.y + 6.0;
            let p = 1.0 - (y - plot_y0) / (r.h - 12.0);
            let f = analysis::pos_freq(p.clamp(0.0, 1.0), st.spectrum_curve);
            format!("{:.0} Hz  {}", f, note_name(f))
        }
        Mode::Meter => {
            let p = (y - r.y) / (r.h - 4.0);
            let plot = Rect::new(r.x, r.y, r.w, r.h - 4.0);
            let _ = p;
            format!("{:.1} dB", level_at(st, plot, y))
        }
        Mode::Vectorscope => {
            let s = (x - r.cx()) / (r.w / 2.0);
            let m = (r.cy() - y) / (r.h / 2.0);
            let pan = if (s.abs() + m.abs()) > 1e-3 { s / (s.abs() + m.abs()) * 100.0 } else { 0.0 };
            format!("Pan {pan:+.0}%")
        }
    };
    c.fill_rect(Rect::new(x.round(), r.y, 1.0, r.h), loc);
    c.fill_rect(Rect::new(r.x, y.round(), r.w, 1.0), loc);
    let f = Font::new(9);
    let tx = if x + 8.0 + c.text_width(f, &text) > r.right() { x - 8.0 - c.text_width(f, &text) } else { x + 8.0 };
    c.text(f, tx, y - 16.0, color(st, HINT, 1.0), &text);
}

/// The note nearest a frequency, e.g. "A4".
pub fn note_name(f: f32) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let n = (12.0 * (f / 440.0).log2() + 69.0).round() as i32;
    format!("{}{}", N[n.rem_euclid(12) as usize], n.div_euclid(12) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meter_scale_matches_the_original_at_its_three_settings() {
        assert!((meter_place(0.0, 0) - 0.222).abs() < 1e-3);
        assert!((meter_place(-6.0, 128) - 0.397).abs() < 1e-3);
        assert!((meter_place(-30.0, 256) - 0.588).abs() < 1e-3);
        // Lower levels sit lower at every scale.
        for s in [0, 64, 128, 200, 256] {
            let mut last = -1.0;
            for d in (-100..=12).rev() {
                let p = meter_place(d as f32, s);
                assert!(p >= last - 1e-4, "scale {s} at {d} dB");
                last = p;
            }
        }
    }

    #[test]
    fn notes_are_named() {
        assert_eq!(note_name(440.0), "A4");
        assert_eq!(note_name(261.63), "C4");
    }

    #[test]
    fn auto_mode_follows_the_stored_window() {
        let area = Rect::new(0.0, 0.0, 600.0, 200.0);
        let st = |window, mode| State { auto_mode: true, window, mode, ..State::new_default() };
        assert_eq!(shown_mode(&st([0, 0, 40, 700], Mode::Oscilloscope), area), Mode::Meter);
        assert_eq!(shown_mode(&st([0, 0, 300, 120], Mode::Meter), area), Mode::Oscilloscope);
        assert_eq!(shown_mode(&st([0, 0, 40, 700], Mode::Spectrum), area), Mode::Spectrum);
        assert_eq!(shown_mode(&st([0, 0, 195, 195], Mode::Vectorscope), area), Mode::Vectorscope);
        // The box keeps the window's shape inside the area, centred.
        let b = box_rect(&st([0, 0, 200, 400], Mode::Meter), area);
        assert!((b.w / b.h - 0.5).abs() < 0.01 && (b.cx() - 300.0).abs() < 1.0, "{b:?}");
        // Never narrower than 72 px.
        assert_eq!(box_rect(&st([0, 0, 20, 800], Mode::Meter), area).w, 72.0);
    }
}
