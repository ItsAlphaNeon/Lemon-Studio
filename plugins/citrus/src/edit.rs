// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the editor's curve view and harmonics view do to a patch: points moved, added and deleted, their modes,
//! tensions and markers, the curve tools, and the harmonics tools. Every curve edit prepares the curve again.
//
// spec: plugins/citrus.md §6 (stage 3), plugins/sytrus-editor.md §3, §4.2, plugins/image-line-curves.md

use crate::curve::{Curve, Point, LFO, MAP};
use crate::state::{OpBlock, ABSOLUTE, GATE, HALF_WAVE};

/// The points' times: the summed distances up to each point.
pub fn times(c: &Curve) -> Vec<f64> {
    let mut t = 0.0;
    c.points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i > 0 {
                t += f64::from(p.dist.max(0.0));
            }
            t
        })
        .collect()
}

/// The markers a curve of this kind keeps (plugins/sytrus.md, "Articulators"): 0 decay, 1 loop start, 2 sustain.
pub fn keeps_marker(c: &Curve, k: usize) -> bool {
    match c.kind {
        MAP => false,
        LFO => k != 0,
        _ => true,
    }
}

/// Moves point `i` to time `t` and `level`, between its neighbours; the first point stays at time 0 and a map's
/// last point at 1. With `slide`, the points after it move along.
pub fn move_point(c: &mut Curve, i: usize, t: f64, level: f32, slide: bool) {
    let ts = times(c);
    let n = ts.len();
    let last_fixed = c.kind == MAP && i + 1 == n;
    let t = if i == 0 || last_fixed {
        ts[i]
    } else {
        let hi = if slide || i + 1 == n { f64::INFINITY } else { ts[i + 1] };
        t.clamp(ts[i - 1], hi)
    };
    let t = if c.kind == MAP { t.min(1.0) } else { t };
    if i > 0 {
        let d = (t - ts[i - 1]) as f32;
        if !slide && i + 1 < n {
            c.points[i + 1].dist = (ts[i + 1] - t).max(0.0) as f32;
        }
        c.points[i].dist = d.max(0.0);
    }
    c.points[i].level = level.clamp(0.0, 1.0);
    c.prepare();
}

/// Adds a point at time `t` and `level` (after the last point before `t`); returns its index.
pub fn insert_point(c: &mut Curve, t: f64, level: f32) -> usize {
    let ts = times(c);
    let i = ts.iter().rposition(|&x| x <= t).map_or(1, |k| k + 1).max(1);
    let prev = ts[i - 1];
    let p = Point { dist: (t - prev).max(0.0) as f32, level: level.clamp(0.0, 1.0), ..Point::default() };
    if i < ts.len() {
        c.points[i].dist = (ts[i] - t).max(0.0) as f32;
    }
    c.points.insert(i, p);
    for m in [&mut c.decay, &mut c.loop_start, &mut c.sustain] {
        if *m >= i as i32 {
            *m += 1;
        }
    }
    c.prepare();
    i
}

/// Deletes point `i` (not the first, nor a curve's only two); the next point keeps its time.
pub fn delete_point(c: &mut Curve, i: usize) -> bool {
    let n = c.points.len();
    if i == 0 || i >= n || n <= 2 {
        return false;
    }
    if i + 1 < n {
        let d = c.points[i].dist + c.points[i + 1].dist;
        c.points[i + 1].dist = d;
    }
    c.points.remove(i);
    for m in [&mut c.decay, &mut c.loop_start, &mut c.sustain] {
        if *m == i as i32 {
            *m = -1;
        } else if *m > i as i32 {
            *m -= 1;
        }
    }
    c.prepare();
    true
}

pub fn set_tension(c: &mut Curve, i: usize, t: f32) {
    c.points[i].tension = t.clamp(-1.0, 1.0);
    c.prepare();
}

pub fn set_mode(c: &mut Curve, i: usize, mode: u8) {
    c.points[i].mode = mode;
    c.prepare();
}

/// The point's arpeggiator break (its step byte): 0 none, 1 previous, 2 same, 3 next.
pub fn set_break(c: &mut Curve, i: usize, step: u8) {
    c.points[i].step = step;
    c.prepare();
}

/// Turns marker `k` (0 decay, 1 loop start, 2 sustain) on at point `i`, or off when it is there.
pub fn toggle_marker(c: &mut Curve, i: usize, k: usize) {
    let m = match k {
        0 => &mut c.decay,
        1 => &mut c.loop_start,
        _ => &mut c.sustain,
    };
    *m = if *m == i as i32 { -1 } else { i as i32 };
    c.prepare();
}

/// Make all loop: the loop from the first point to the last.
pub fn make_all_loop(c: &mut Curve) {
    c.loop_start = 0;
    c.sustain = c.points.len() as i32 - 1;
    c.prepare();
}

pub fn flip(c: &mut Curve) {
    for p in c.points.iter_mut() {
        p.level = 1.0 - p.level;
    }
    c.prepare();
}

/// Stretches the levels to 0 .. 1.
pub fn normalize(c: &mut Curve) {
    let lo = c.points.iter().map(|p| p.level).fold(f32::INFINITY, f32::min);
    let hi = c.points.iter().map(|p| p.level).fold(f32::NEG_INFINITY, f32::max);
    if hi - lo > 1e-6 {
        for p in c.points.iter_mut() {
            p.level = (p.level - lo) / (hi - lo);
        }
        c.prepare();
    }
}

/// Every point in mode 10, smooth.
pub fn all_smooth(c: &mut Curve) {
    for p in c.points.iter_mut() {
        p.mode = 10;
    }
    c.prepare();
}

/// Scale levels (FL's `Level scaling`): about centre `center`, the tension `tension` (-1 .. 1) bends the levels away
/// from it (or towards it), then `mul` stretches them about it and `offset` moves them; clamped to 0 .. 1.
pub fn scale_levels(c: &mut Curve, center: f32, mul: f32, offset: f32, tension: f32) {
    let k = 1000f64.ln() * f64::from(tension);
    let bend = |z: f64| if k.abs() < 1e-9 { z } else { (1.0 - (-k * z).exp()) / (1.0 - (-k).exp()) };
    let cen = f64::from(center);
    for p in c.points.iter_mut() {
        let d = f64::from(p.level) - cen;
        let room = 1.0 - cen;
        let x1 = if room < 1e-9 { f64::from(p.level) } else { cen + d.signum() * room * bend(d.abs() / room) };
        let x2 = cen + (x1 - cen) * f64::from(mul);
        p.level = ((x2 + f64::from(offset)) as f32).clamp(0.0, 1.0);
    }
    c.prepare();
}

/// Smooth up abrupt changes: a zero-length segment (not the first) gets 0.04 time units from the segment before it
/// (less when that one is shorter).
pub fn smooth_abrupt(c: &mut Curve) {
    for i in 2..c.points.len() {
        if c.points[i].dist <= 0.0 && c.points[i - 1].dist > 0.0 {
            let take = c.points[i - 1].dist.min(ABRUPT);
            c.points[i - 1].dist -= take;
            c.points[i].dist = take;
        }
    }
    c.prepare();
}

/// The length Smooth up abrupt changes gives a jump.
const ABRUPT: f32 = 0.04;

/// A point's level as Type in value shows it: the double with 15 significant digits.
pub fn level_text(l: f32) -> String {
    let v = f64::from(l);
    if v == 0.0 {
        return "0".into();
    }
    let decimals = (14 - v.abs().log10().floor() as i32).max(0) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').into() } else { s }
}

/// A typed level: up to 1 the level, above 1 (or with `%`) a percentage; clamped to 0 .. 1. None when it does not
/// read as a number.
pub fn parse_level(text: &str) -> Option<f32> {
    let t = text.trim();
    let (num, pct) = match t.strip_suffix('%') {
        Some(n) => (n.trim(), true),
        None => (t, false),
    };
    let v: f64 = num.parse().ok().filter(|v: &f64| v.is_finite())?;
    let v = if pct || v > 1.0 { v / 100.0 } else { v };
    Some((v as f32).clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------------------------------------------
// Points selection (plugins/sytrus-editor.md §3.1). The first point is never removed or moved in time.

/// The selection's stretches of neighbouring points, (first, last), the first point left out.
fn runs(sel: &[bool]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (i, _) in sel.iter().enumerate().skip(1).filter(|(_, s)| **s) {
        match out.last_mut() {
            Some(r) if r.1 + 1 == i => r.1 = i,
            _ => out.push((i, i)),
        }
    }
    out
}

/// Removes points `a ..= b` with the markers on them; the markers after move back.
fn remove_run(c: &mut Curve, a: usize, b: usize) {
    c.points.drain(a..=b);
    let k = (b - a + 1) as i32;
    for m in [&mut c.decay, &mut c.loop_start, &mut c.sustain] {
        if (a as i32..=b as i32).contains(m) {
            *m = -1;
        } else if *m > b as i32 {
            *m -= k;
        }
    }
}

/// Inserts `pts` at index `i`; the markers from there on move along.
fn insert_run(c: &mut Curve, i: usize, pts: &[Point]) {
    c.points.splice(i..i, pts.iter().copied());
    for m in [&mut c.decay, &mut c.loop_start, &mut c.sustain] {
        if *m >= i as i32 {
            *m += pts.len() as i32;
        }
    }
}

/// A map still ends at 1: the segments from point `from` on are shrunk to fit (Lemon Studio's choice).
fn fit_map(c: &mut Curve, from: usize) {
    if c.kind != MAP {
        return;
    }
    let ts = times(c);
    let (start, end) = (ts[from.min(ts.len() - 1)], ts.last().copied().unwrap_or(0.0));
    if end > 1.0 && end > start {
        let k = ((1.0 - start) / (end - start)) as f32;
        for p in c.points.iter_mut().skip(from + 1) {
            p.dist *= k;
        }
    }
}

/// How many points are selected.
pub fn count(sel: &[bool]) -> usize {
    sel.iter().filter(|s| **s).count()
}

/// The selected points, each with its own segment length.
pub fn copy_points(c: &Curve, sel: &[bool]) -> Vec<Point> {
    c.points.iter().zip(sel).filter(|(_, s)| **s).map(|(p, _)| *p).collect()
}

/// Deletes the selected points; the points after keep their times. Not when fewer than two points would be left.
pub fn delete_points(c: &mut Curve, sel: &[bool]) -> bool {
    let rs = runs(sel);
    let gone: usize = rs.iter().map(|(a, b)| b - a + 1).sum();
    if rs.is_empty() || c.points.len() - gone < 2 {
        return false;
    }
    for &(a, b) in rs.iter().rev() {
        let d: f32 = c.points[a..=b].iter().map(|p| p.dist).sum();
        if let Some(next) = c.points.get_mut(b + 1) {
            next.dist += d;
        }
        remove_run(c, a, b);
    }
    c.prepare();
    true
}

/// Cut's removal: the selected points and the time they spanned go (the point after each stretch lands on the
/// point before it).
pub fn cut_points(c: &mut Curve, sel: &[bool]) -> bool {
    let rs = runs(sel);
    let gone: usize = rs.iter().map(|(a, b)| b - a + 1).sum();
    if rs.is_empty() || c.points.len() - gone < 2 {
        return false;
    }
    for &(a, b) in rs.iter().rev() {
        if let Some(next) = c.points.get_mut(b + 1) {
            next.dist = 0.0;
        }
        remove_run(c, a, b);
    }
    c.prepare();
    true
}

/// Paste (insert): `pts` right after point `after`, the first at zero length; returns the pasted points' indices.
pub fn paste_insert(c: &mut Curve, after: usize, pts: &[Point]) -> std::ops::Range<usize> {
    let mut pts = pts.to_vec();
    let Some(first) = pts.first_mut() else { return 0..0 };
    first.dist = 0.0;
    let at = (after + 1).min(c.points.len());
    insert_run(c, at, &pts);
    fit_map(c, at);
    c.prepare();
    at..at + pts.len()
}

/// Paste (replace): the selected points (from the first to the last selected) give way to `pts`, put where the first
/// of them was; the point after keeps its length. Returns the pasted points' indices.
pub fn paste_replace(c: &mut Curve, sel: &[bool], pts: &[Point]) -> Option<std::ops::Range<usize>> {
    let rs = runs(sel);
    let (a, b) = (rs.first()?.0, rs.last()?.1);
    if pts.is_empty() || c.points.len() - (b - a + 1) + pts.len() < 2 {
        return None;
    }
    let d = c.points[a].dist;
    remove_run(c, a, b);
    let mut pts = pts.to_vec();
    pts[0].dist = d;
    insert_run(c, a, &pts);
    fit_map(c, a);
    c.prepare();
    Some(a..a + pts.len())
}

/// Moves the selected points of `orig` by `dt` in time and `dl` in level: the time offset is held so no stretch
/// passes its neighbours (none with the first point, or a map's last, selected); each level is clamped.
pub fn move_points(c: &mut Curve, orig: &Curve, sel: &[bool], dt: f64, dl: f32) {
    let ts = times(orig);
    let n = ts.len();
    let fixed = sel.first() == Some(&true) || (orig.kind == MAP && sel.get(n - 1) == Some(&true));
    let mut dt = if fixed { 0.0 } else { dt };
    for (a, b) in runs(sel) {
        dt = dt.max(ts[a - 1] - ts[a]);
        if b + 1 < n {
            dt = dt.min(ts[b + 1] - ts[b]);
        } else if orig.kind == MAP {
            dt = dt.min(1.0 - ts[b]);
        }
    }
    let new: Vec<f64> = ts.iter().zip(sel).map(|(&t, &s)| if s { t + dt } else { t }).collect();
    *c = orig.clone();
    for (i, p) in c.points.iter_mut().enumerate() {
        if sel.get(i) == Some(&true) {
            p.level = (p.level + dl).clamp(0.0, 1.0);
        }
        if i > 0 {
            p.dist = (new[i] - new[i - 1]).max(0.0) as f32;
        }
    }
    c.prepare();
}

/// The level on the curve at `x` (0 .. 1) of the segment that ends on point `i`.
pub fn segment_level(c: &Curve, i: usize, x: f64) -> f32 {
    let (y0, y1) = (c.points[i - 1].level, c.points[i].level);
    y0 + c.progress(i, x) * (y1 - y0)
}

// ---------------------------------------------------------------------------------------------------------------
// Harmonics

/// The harmonics' levels (the largest 1) and phases (in cycles) of the operator's table: the shape as sines.
pub fn harmonics_of(op: &OpBlock, smooth: bool) -> (Vec<f32>, Vec<f32>) {
    let t = crate::wave::build(op, smooth).t;
    let n = crate::wave::N;
    let h = op.levels.len();
    let mut mag = vec![0f64; h];
    let mut ph = vec![0f32; h];
    let w = std::f64::consts::TAU / n as f64;
    for k in 0..h {
        let f = (k + 1) as f64 * w;
        // A harmonic L sin(f j + 2 pi phase) gives these sums N/2 L sin(2 pi phase), N/2 L cos(2 pi phase).
        let (mut re, mut im) = (0.0, 0.0);
        let (step_c, step_s) = (f.cos(), f.sin());
        let (mut c, mut s) = (1.0f64, 0.0f64);
        for &x in &t[..n] {
            re += f64::from(x) * c;
            im += f64::from(x) * s;
            (c, s) = (c * step_c - s * step_s, s * step_c + c * step_s);
        }
        mag[k] = 2.0 / n as f64 * re.hypot(im);
        ph[k] = (re.atan2(im) / std::f64::consts::TAU).rem_euclid(1.0) as f32;
    }
    let top = mag.iter().copied().fold(0.0, f64::max);
    // FL keeps the smallest levels too (1e-6 of the largest seen).
    let levels = mag.iter().map(|&m| if top > 0.0 { (m / top) as f32 } else { 0.0 }).collect();
    (levels, ph)
}

/// Convert shape to sine harmonics: the harmonics take the shape, and the shape settings that bend it go to 0.
pub fn convert_to_harmonics(op: &mut OpBlock, smooth: bool) {
    let (levels, phases) = harmonics_of(op, smooth);
    (op.levels, op.phases) = (levels, phases);
    for k in [0, 1, 2, 3] {
        op.shape[k] = 0;
    }
    for f in [HALF_WAVE, GATE, ABSOLUTE] {
        op.flags[f] = 0;
    }
}

/// Transpose one octave up: harmonic `h` moves to `2 h`.
pub fn transpose_up(op: &mut OpBlock) {
    let n = op.levels.len();
    let (mut l, mut p) = (vec![0f32; n], vec![0f32; n]);
    for k in 0..n {
        let to = 2 * (k + 1) - 1;
        if to < n {
            (l[to], p[to]) = (op.levels[k], op.phases[k]);
        }
    }
    (op.levels, op.phases) = (l, p);
}

/// Shift the harmonics one place down (left) or up (right).
pub fn shift(op: &mut OpBlock, right: bool) {
    for v in [&mut op.levels, &mut op.phases] {
        if right {
            v.rotate_right(1);
            v[0] = 0.0;
        } else {
            v.rotate_left(1);
            let n = v.len();
            v[n - 1] = 0.0;
        }
    }
}

/// Attenuate Gibbs phenomenon: the levels times the Lanczos factor up to the highest harmonic in use.
pub fn attenuate_gibbs(op: &mut OpBlock) {
    let Some(top) = op.levels.iter().rposition(|&l| l != 0.0) else { return };
    let m = (top + 2) as f64;
    for (k, l) in op.levels.iter_mut().enumerate().take(top + 1) {
        let x = std::f64::consts::PI * (k + 1) as f64 / m;
        *l *= (x.sin() / x) as f32;
    }
}

pub fn reset_phases(op: &mut OpBlock) {
    op.phases.iter_mut().for_each(|p| *p = 0.0);
}

/// Random phases from `seed` (a small LCG: the editor's own, not FL's).
pub fn randomize_phases(op: &mut OpBlock, seed: u32) {
    let mut s = seed | 1;
    for p in op.phases.iter_mut() {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *p = (s >> 8) as f32 / (1u32 << 24) as f32;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The header's waveform items (plugins/sytrus-editor.md §7)

/// Reset waveform settings: harmonic 1 alone, phases 0, the shape settings 0 and the pre-filter 128.
pub fn reset_waveform(op: &mut OpBlock) {
    op.levels.iter_mut().for_each(|l| *l = 0.0);
    op.levels[0] = 1.0;
    reset_phases(op);
    op.shape = [0, 0, 0, 0, 128];
}

/// Randomize waveform settings: a few harmonics at random levels and phases, a random shape and skew, sometimes a
/// sine shaper and a pre-filter. Lemon Studio's generator (FL's was not studied), from `seed`.
pub fn randomize_waveform(op: &mut OpBlock, seed: u32) {
    let mut s = seed | 1;
    let mut next = |n: u32| {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 8) % n
    };
    reset_waveform(op);
    for _ in 0..2 + next(3) {
        let h = 1 + next(15) as usize;
        op.levels[h] = (32 + next(225)) as f32 / 256.0;
        op.phases[h] = next(256) as f32 / 256.0;
    }
    op.shape[0] = next(513) as i32;
    op.shape[2] = next(129) as i32;
    if next(2) == 0 {
        op.shape[3] = next(129) as i32;
        op.shape[4] = next(129) as i32;
    }
}

/// Morph oscillators 1 to 5: operators 1 .. 5 converted to harmonics, then 2, 3 and 4 mixed between 1 and 5 at
/// 1/4, 2/4 and 3/4: the levels straight, the phases (fractions of a turn) the shorter way round.
pub fn morph_operators(ops: &mut [&mut OpBlock; 5], smooth: bool) {
    for op in ops.iter_mut() {
        convert_to_harmonics(op, smooth);
    }
    let (a, b) = ((ops[0].levels.clone(), ops[0].phases.clone()), (ops[4].levels.clone(), ops[4].phases.clone()));
    for (k, op) in ops.iter_mut().enumerate().take(4).skip(1) {
        let x = k as f32 / 4.0;
        for h in 0..op.levels.len() {
            op.levels[h] = a.0[h] + (b.0[h] - a.0[h]) * x;
            let mut d = b.1[h] - a.1[h];
            d -= d.round();
            op.phases[h] = (a.1[h] + d * x).rem_euclid(1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::ENVELOPE;

    fn env() -> Curve {
        let mut c = Curve::default_of(ENVELOPE);
        c.points = vec![
            Point { level: 0.0, ..Point::default() },
            Point { dist: 1.0, level: 1.0, ..Point::default() },
            Point { dist: 2.0, level: 0.5, ..Point::default() },
            Point { dist: 1.0, level: 0.0, ..Point::default() },
        ];
        c.sustain = 2;
        c.prepare();
        c
    }

    /// A point moves between its neighbours; adding and deleting keep the other points' times and the markers.
    #[test]
    fn points_move_add_and_delete() {
        let mut c = env();
        move_point(&mut c, 1, 2.0, 0.8, false);
        assert_eq!(times(&c), vec![0.0, 2.0, 3.0, 4.0]);
        move_point(&mut c, 1, 9.0, 0.8, false);
        assert_eq!(times(&c)[1], 3.0, "not past the next point");
        move_point(&mut c, 1, 2.0, 0.8, true);
        assert_eq!(times(&c), vec![0.0, 2.0, 2.0, 3.0], "slide: the rest moves along");
        let mut c = env();
        move_point(&mut c, 0, 1.0, 0.25, false);
        assert_eq!((times(&c)[0], c.points[0].level), (0.0, 0.25));
        let i = insert_point(&mut c, 2.5, 0.4);
        assert_eq!((i, c.sustain), (2, 3));
        assert_eq!(times(&c), vec![0.0, 1.0, 2.5, 3.0, 4.0]);
        assert!(delete_point(&mut c, 3));
        assert_eq!((c.sustain, times(&c)), (-1, vec![0.0, 1.0, 2.5, 4.0]));
        assert!(!delete_point(&mut c, 0));
        toggle_marker(&mut c, 2, 2);
        assert_eq!(c.sustain, 2);
        toggle_marker(&mut c, 2, 2);
        assert_eq!(c.sustain, -1);
    }

    /// A map keeps its ends at 0 and 1.
    #[test]
    fn map_ends_stay() {
        let mut c = Curve::default_of(MAP);
        c.points = vec![Point { level: 0.0, ..Point::default() }, Point { dist: 1.0, level: 1.0, ..Point::default() }];
        c.prepare();
        move_point(&mut c, 1, 0.5, 0.3, false);
        assert_eq!((times(&c)[1], c.points[1].level), (1.0, 0.3));
        insert_point(&mut c, 0.25, 0.9);
        move_point(&mut c, 1, 3.0, 0.9, true);
        assert!(times(&c)[1] <= 1.0);
    }

    /// A saw's table converted to harmonics plays the same table again; the tools move the harmonics.
    #[test]
    fn shapes_convert_to_harmonics() {
        let mut op = crate::preset::default_patch().modules[0].op.clone();
        op.shape[0] = 256;
        let before = crate::wave::build(&op, false).t;
        convert_to_harmonics(&mut op, false);
        assert_eq!(op.shape[0], 0);
        assert!((op.levels[1] - 0.5).abs() < 0.02, "a saw's second harmonic is half the first: {}", op.levels[1]);
        let after = crate::wave::build(&op, false).t;
        let n = crate::wave::N;
        let rms = (before[..n].iter().zip(&after[..n]).map(|(a, b)| f64::from(a - b).powi(2)).sum::<f64>() / n as f64).sqrt();
        assert!(rms < 0.02, "rms difference {rms}");
        transpose_up(&mut op);
        assert_eq!((op.levels[0], op.levels[1]), (0.0, 1.0));
        shift(&mut op, false);
        assert_eq!(op.levels[0], 1.0);
        shift(&mut op, true);
        assert_eq!((op.levels[0], op.levels[1]), (0.0, 1.0));
    }

    fn levels(c: &Curve) -> Vec<f32> {
        c.points.iter().map(|p| p.level).collect()
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-5)
    }

    /// Scale levels against FL's saves (scaleG to scaleH: every control off its default, both sides of the centre).
    #[test]
    fn levels_scale_as_fl() {
        let mut c = env();
        c.points.truncate(3);
        for (p, l) in c.points.iter_mut().zip([0.21704, 0.78303, 0.59068]) {
            p.level = l;
        }
        let t = 30.0 / 256.0;
        scale_levels(&mut c, t, 98.0 / 128.0, t, t);
        assert!(close(&levels(&c), &[0.34095, 0.79095, 0.66337]), "{:?}", levels(&c));
        let mut c = env();
        scale_levels(&mut c, 0.0, 1.0, 0.0, 0.0);
        assert_eq!(levels(&c), levels(&env()), "the defaults change nothing");
        scale_levels(&mut c, 0.5, 2.0, 0.25, 0.0);
        assert_eq!(levels(&c), vec![0.0, 1.0, 0.75, 0.0], "clamped");
    }

    /// Smooth up abrupt changes: jumps (but the first segment) get 0.04; a second run changes nothing.
    #[test]
    fn abrupt_changes_smoothed() {
        let mut c = env();
        c.points[2].dist = 0.0;
        c.points[1].dist = 0.0;
        c.points.push(Point { dist: 0.0, level: 0.3, ..Point::default() });
        c.prepare();
        smooth_abrupt(&mut c);
        let ts = times(&c);
        assert_eq!(ts.len(), 5);
        assert!(ts[1] == 0.0 && ts[2] == 0.0, "the first jump and a jump after a jump stay: {ts:?}");
        assert!((ts[3] - 0.96).abs() < 1e-6 && (ts[4] - 1.0).abs() < 1e-6, "{ts:?}");
        let before = times(&c);
        smooth_abrupt(&mut c);
        assert_eq!(times(&c), before);
    }

    /// Type in value: the level's text and what typing gives (FL's examples).
    #[test]
    fn levels_typed_as_fl() {
        assert_eq!(level_text(0.576_388_9), "0.576388895511627");
        assert_eq!(level_text(1.0), "1");
        assert_eq!(level_text(0.0), "0");
        let cases = [("0.25", Some(0.25)), ("50", Some(0.5)), ("1.7", Some(0.017)), ("150", Some(1.0))];
        for (t, v) in cases.into_iter().chain([("0.5%", Some(0.005)), ("-3", Some(0.0)), ("abc", None), ("1,5", None)]) {
            assert_eq!(parse_level(t), v, "{t}");
        }
    }

    /// The selection operations (FL's saves sel*): times kept or closed up, markers along, the first point kept.
    #[test]
    fn selection_operations() {
        let sel = |v: &[usize], n: usize| (0..n).map(|i| v.contains(&i)).collect::<Vec<_>>();
        // Delete: the next point keeps its time.
        let mut c = env();
        assert!(delete_points(&mut c, &sel(&[1, 2], 4)));
        assert_eq!((times(&c), c.sustain), (vec![0.0, 4.0], -1));
        let mut c = env();
        assert!(!delete_points(&mut c, &sel(&[1, 2, 3], 4)), "two points stay");
        // Cut: the time goes too.
        let mut c = env();
        let copied = copy_points(&c, &sel(&[1], 4));
        assert!(cut_points(&mut c, &sel(&[1], 4)));
        assert_eq!((times(&c), c.sustain), (vec![0.0, 0.0, 1.0], 1));
        // Paste (insert) after the last selected, its first point at zero length.
        let r = paste_insert(&mut c, 1, &copied);
        assert_eq!(r, 2..3);
        assert_eq!((times(&c), levels(&c), c.sustain), (vec![0.0, 0.0, 0.0, 1.0], vec![0.0, 0.5, 1.0, 0.0], 1));
        // Duplicate of two points.
        let mut c = env();
        let two = copy_points(&c, &sel(&[1, 2], 4));
        let r = paste_insert(&mut c, 2, &two);
        assert_eq!((r, times(&c)), (3..5, vec![0.0, 1.0, 3.0, 3.0, 5.0, 6.0]));
        // Paste (replace): where the first selected was, the next point keeps its length.
        let mut c = env();
        let r = paste_replace(&mut c, &sel(&[2], 4), &two).unwrap();
        assert_eq!((r, times(&c), levels(&c)), (2..4, vec![0.0, 1.0, 3.0, 5.0, 6.0], vec![0.0, 1.0, 1.0, 0.5, 0.0]));
    }

    /// A group drag moves the selected points together, held between the others.
    #[test]
    fn selected_points_move_together() {
        let orig = env();
        let sel = vec![false, true, true, false];
        let mut c = orig.clone();
        move_points(&mut c, &orig, &sel, 0.5, -0.25);
        assert_eq!((times(&c), levels(&c)), (vec![0.0, 1.5, 3.5, 4.0], vec![0.0, 0.75, 0.25, 0.0]));
        move_points(&mut c, &orig, &sel, 5.0, 0.0);
        assert_eq!(times(&c), vec![0.0, 2.0, 4.0, 4.0], "not past the next point");
        move_points(&mut c, &orig, &[true, true, false, false], 1.0, 0.0);
        assert_eq!(times(&c), times(&orig), "the first point holds the time");
    }

    /// Morph: 2 .. 4 between 1 and 5, the phases the short way round.
    #[test]
    fn operators_morph() {
        let base = crate::preset::default_patch().modules[0].op.clone();
        let mut o: [OpBlock; 5] = std::array::from_fn(|_| base.clone());
        o[0].levels[1] = 0.2;
        o[4].levels[1] = 0.6;
        o[0].phases[1] = 0.9;
        o[4].phases[1] = 0.1;
        let [a, b, c, d, e] = &mut o;
        morph_operators(&mut [a, b, c, d, e], false);
        assert!((o[1].levels[1] - 0.3).abs() < 1e-3 && (o[2].levels[1] - 0.4).abs() < 1e-3, "{}", o[1].levels[1]);
        assert!((o[2].phases[1] - 0.0).abs() < 1e-3 || (o[2].phases[1] - 1.0).abs() < 1e-3, "{}", o[2].phases[1]);
        let mut r = base.clone();
        randomize_waveform(&mut r, 7);
        reset_waveform(&mut r);
        assert_eq!((r.levels[0], r.levels[1], r.phases[1], r.shape), (1.0, 0.0, 0.0, [0, 0, 0, 0, 128]));
    }
}
