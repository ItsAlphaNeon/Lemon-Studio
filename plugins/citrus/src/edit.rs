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
    let levels = mag.iter().map(|&m| if top > 0.0 && m / top > 1e-4 { (m / top) as f32 } else { 0.0 }).collect();
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
}
