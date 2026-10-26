// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the curve view does to a curve: points moved, added and deleted, their modes, tensions and loop markers, and
//! the curve tools. Every edit applies Love Philter's rules for what a curve keeps and prepares it again.
//!
// spec: plugins/lemon-pulp-filter.md §4, plugins/image-line-curves.md

use os_dsp::curve::{Curve, Point, MAP};

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

/// The level on the curve at `x` (0..1) of the segment that ends on point `i`.
pub fn segment_level(c: &Curve, i: usize, x: f64) -> f32 {
    let (y0, y1) = (c.points[i - 1].level, c.points[i].level);
    y0 + c.progress(i, x) * (y1 - y0)
}

fn done(c: &mut Curve) {
    c.keep_love_philter();
}

/// Moves point `i` to time `t` and `level`, between its neighbours; the first point stays at time 0 and a map's last
/// point at 1. With `slide`, the points after it move along.
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
        if !slide && i + 1 < n {
            c.points[i + 1].dist = (ts[i + 1] - t).max(0.0) as f32;
        }
        c.points[i].dist = ((t - ts[i - 1]) as f32).max(0.0);
    }
    c.points[i].level = level.clamp(0.0, 1.0);
    done(c);
}

/// Adds a point at time `t` and `level` (after the last point before `t`); returns its index.
pub fn insert_point(c: &mut Curve, t: f64, level: f32) -> usize {
    let ts = times(c);
    let t = if c.kind == MAP { t.min(1.0) } else { t };
    let i = ts.iter().rposition(|&x| x <= t).map_or(1, |k| k + 1).max(1);
    let prev = ts[i - 1];
    let p = Point { dist: (t - prev).max(0.0) as f32, level: level.clamp(0.0, 1.0), ..Point::default() };
    if i < ts.len() {
        c.points[i].dist = (ts[i] - t).max(0.0) as f32;
    }
    c.points.insert(i, p);
    for m in [&mut c.loop_start, &mut c.sustain] {
        if *m >= i as i32 {
            *m += 1;
        }
    }
    done(c);
    i
}

/// Deletes point `i` (not the first, nor a map's last); the next point keeps its time.
pub fn delete_point(c: &mut Curve, i: usize) -> bool {
    let n = c.points.len();
    if i == 0 || i >= n || (c.kind == MAP && i + 1 == n) {
        return false;
    }
    if i + 1 < n {
        let d = c.points[i].dist + c.points[i + 1].dist;
        c.points[i + 1].dist = d;
    }
    c.points.remove(i);
    for m in [&mut c.loop_start, &mut c.sustain] {
        if *m == i as i32 {
            *m = -1;
        } else if *m > i as i32 {
            *m -= 1;
        }
    }
    done(c);
    true
}

pub fn set_tension(c: &mut Curve, i: usize, t: f32) {
    c.points[i].tension = t.clamp(-1.0, 1.0);
    done(c);
}

pub fn set_mode(c: &mut Curve, i: usize, mode: u8) {
    c.points[i].mode = mode;
    done(c);
}

/// Turns the loop start (`sustain` false) or the sustain point on at point `i`, or off when it is there.
pub fn toggle_marker(c: &mut Curve, i: usize, sustain: bool) {
    let m = if sustain { &mut c.sustain } else { &mut c.loop_start };
    *m = if *m == i as i32 { -1 } else { i as i32 };
    done(c);
}

/// The loop from the first point to the last (FL's patterns loop so).
pub fn make_all_loop(c: &mut Curve) {
    c.loop_start = 0;
    c.sustain = c.points.len() as i32 - 1;
    done(c);
}

pub fn flip(c: &mut Curve) {
    for p in c.points.iter_mut() {
        p.level = 1.0 - p.level;
    }
    done(c);
}

/// A flat curve at `level` over one unit (a map: from 0 to 1).
pub fn reset(c: &mut Curve, level: f32) {
    c.points = vec![Point { level, ..Point::default() }, Point { dist: 1.0, level, ..Point::default() }];
    c.loop_start = -1;
    c.sustain = -1;
    done(c);
}

/// The modes a point's menu offers (the segment that ends on the point).
pub const POINT_MODES: [(&str, u8); 13] = [
    ("Hold", 2),
    ("Smooth", 10),
    ("Single curve", 0),
    ("Single curve 2", 7),
    ("Single curve 3", 11),
    ("Double curve", 1),
    ("Double curve 2", 8),
    ("Double curve 3", 12),
    ("Half sine", 9),
    ("Stairs", 3),
    ("Smooth stairs", 4),
    ("Pulse", 5),
    ("Wave", 6),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> Curve {
        let mut c = Curve::default_of(MAP);
        reset(&mut c, 0.5);
        c
    }

    #[test]
    fn points_move_add_and_go() {
        let mut c = map();
        let i = insert_point(&mut c, 0.25, 1.0);
        assert_eq!(i, 1);
        assert_eq!(times(&c), [0.0, 0.25, 1.0]);
        move_point(&mut c, 1, 0.75, 0.0, false);
        assert_eq!(times(&c), [0.0, 0.75, 1.0]);
        move_point(&mut c, 2, 0.5, 0.2, false);
        assert_eq!(times(&c)[2], 1.0, "a map's last point stays at 1");
        assert!(!delete_point(&mut c, 2));
        assert!(delete_point(&mut c, 1));
        assert_eq!(times(&c), [0.0, 1.0]);
        let mut e = Curve::default_of(os_dsp::curve::ENVELOPE);
        reset(&mut e, 1.0);
        make_all_loop(&mut e);
        assert_eq!((e.loop_start, e.sustain), (0, 1));
        insert_point(&mut e, 0.5, 0.0);
        assert_eq!(e.sustain, 2);
    }
}
