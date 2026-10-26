// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the map view does to a band's map: points moved, added and deleted, their modes and tensions. A map runs
//! from place 0 to place 1: its first point stays at 0 and its last at 1, and an edited map is stored with its
//! distances adding up to 1.
//!
// spec: plugins/lemon-press.md §4, plugins/maximus.md ("The state", "The curve table")

use os_engine::effects::maximus::state::{Map, MapPoint};

/// The extra bytes a new point gets: those of the factory map's points.
const NEW_REST: [u8; 3] = [0, 0, 2];

/// Each point's place (0..1, the distances scaled to add up to 1).
pub fn places(m: &Map) -> Vec<f64> {
    m.places().into_iter().map(|(x, _)| x).collect()
}

/// Stores the places `xs` as the points' distances.
fn set_places(m: &mut Map, xs: &[f64]) {
    let mut prev = 0.0;
    for (p, &x) in m.points.iter_mut().zip(xs) {
        p.dist = (x - prev).max(0.0);
        prev = x;
    }
}

/// The scale of the map's axes, 0..1 (0.5 = 0 dB, 1 = about +12 dB), as a level.
pub fn scale_to_level(p: f64) -> f64 {
    ((p * 2.0 * 3f64.ln()).exp() - 1.0) * 0.5
}

/// A level on the map's scale.
pub fn level_to_scale(l: f64) -> f64 {
    (l.max(0.0) * 2.0 + 1.0).ln() * (0.5 / 3f64.ln())
}

/// Moves point `i` to place `x` and `level`, between its neighbours; the first point stays at 0 and the last at 1.
pub fn move_point(m: &mut Map, i: usize, x: f64, level: f64) {
    let mut xs = places(m);
    let n = xs.len();
    if i >= n {
        return;
    }
    xs[i] = if i == 0 {
        0.0
    } else if i + 1 == n {
        1.0
    } else {
        x.clamp(xs[i - 1], xs[i + 1])
    };
    set_places(m, &xs);
    m.points[i].level = level.clamp(0.0, 1.0);
}

/// Adds a point at place `x` and `level` (after the last point before `x`); returns its index.
pub fn insert_point(m: &mut Map, x: f64, level: f64) -> usize {
    let mut xs = places(m);
    let x = x.clamp(0.0, 1.0);
    let i = xs.iter().rposition(|&p| p <= x).map_or(1, |k| k + 1).clamp(1, xs.len().max(1));
    xs.insert(i, x);
    let p = MapPoint { rest: NEW_REST, ..MapPoint::new(0.0, level.clamp(0.0, 1.0)) };
    m.points.insert(i, p);
    set_places(m, &xs);
    i
}

/// Deletes point `i` (neither the first nor the last); the next point keeps its place.
pub fn delete_point(m: &mut Map, i: usize) -> bool {
    let n = m.points.len();
    if i == 0 || i + 1 >= n {
        return false;
    }
    let mut xs = places(m);
    xs.remove(i);
    m.points.remove(i);
    set_places(m, &xs);
    true
}

pub fn set_tension(m: &mut Map, i: usize, t: f32) {
    if let Some(p) = m.points.get_mut(i) {
        p.tension = t.clamp(-1.0, 1.0);
    }
}

pub fn set_mode(m: &mut Map, i: usize, mode: u8) {
    if let Some(p) = m.points.get_mut(i) {
        p.mode = mode;
    }
}

/// The modes a point's menu offers (the segment that ends on the point), as for automation curves.
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

    #[test]
    fn points_move_add_and_go() {
        let mut m = Map::default_map();
        assert_eq!(places(&m), [0.0, 0.5, 1.0]);
        let i = insert_point(&mut m, 0.25, 0.1);
        assert_eq!(i, 1);
        assert_eq!(places(&m), [0.0, 0.25, 0.5, 1.0]);
        let sum: f64 = m.points.iter().map(|p| p.dist).sum();
        assert!((sum - 1.0).abs() < 1e-12);
        move_point(&mut m, 1, 0.75, 0.2);
        assert_eq!(places(&m)[1], 0.5, "kept before its neighbour");
        move_point(&mut m, 3, 0.5, 0.4);
        assert_eq!(places(&m)[3], 1.0, "the last point stays at 1");
        assert_eq!(m.points[3].level, 0.4);
        assert!(!delete_point(&mut m, 3));
        assert!(!delete_point(&mut m, 0));
        assert!(delete_point(&mut m, 1));
        assert_eq!(places(&m), [0.0, 0.5, 1.0]);
        assert!((level_to_scale(1.0) - 0.5).abs() < 1e-12);
        assert!((scale_to_level(0.5) - 1.0).abs() < 1e-12);
    }
}
