// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The mixer's routing cables (spec: ui/mixer.md §5: "a green cable is drawn from the current strip's ⬇ to the knob")
//! as ropes plugged in at both ends: a light Verlet simulation, so they sag between their sockets and swing when the
//! mixer window is dragged, resized or scrolled, and when the current track changes. Lemon Studio's own touch; the
//! cables' ends and colours follow the spec.
//!
//! The mixer view connects every cable each time it paints (`begin` / `connect` / `end`); the app steps the ropes on
//! its tick while any of them moves, and they fall asleep once still.

use os_ui::{Color, Rect};

/// How far a cable sags below its sockets at rest, px (it decides each cable's length from its span).
const SAG: f32 = 16.0;
/// Points per cable: about one per `SPACING` px of length, within these bounds.
const SPACING: f32 = 8.0;
const MIN_POINTS: usize = 8;
const MAX_POINTS: usize = 28;
/// Gravity, px/s².
const GRAVITY: f32 = 1800.0;
/// Fixed substep, s.
const STEP: f32 = 1.0 / 240.0;
/// Velocity kept per substep (≈ 30 % per second: a swing settles in a few seconds).
const DAMPING: f32 = 0.995;
const ITERATIONS: usize = 16;
/// A light bending stiffness: points two apart are pushed back toward this share of two segments.
const BEND: f32 = 0.85;
/// A socket jump longer than this (a track change, a fast drag) bends the cable along with it for the excess, so the
/// cable swings rather than snapping across the mixer.
const CARRY: f32 = 64.0;
/// Fastest a point may move per substep, px.
const MAX_SPEED: f32 = 12.0;
/// Below this motion per substep (px) for `SETTLE` substeps in a row, the cables are still.
const REST: f32 = 0.01;
const SETTLE: u32 = 30;
/// Longest frame stepped, s (a stall must not explode the ropes).
const MAX_DT: f32 = 0.05;

type Pt = (f32, f32);

struct Rope {
    /// The track whose send knob the cable plugs into (its other end is the current track's ⬇).
    key: u16,
    /// Plugged into the master strip (clipped with the master, which does not scroll, not with the inserts).
    master: bool,
    color: Color,
    /// Rest length of a segment.
    seg: f32,
    /// Positions and positions one substep ago; the first and last are the sockets.
    p: Vec<Pt>,
    q: Vec<Pt>,
    seen: bool,
}

#[derive(Default)]
pub struct Cables {
    ropes: Vec<Rope>,
    /// Substeps in a row with every rope still; `None` = moving.
    still: Option<u32>,
    acc: f32,
}

/// A cable's length for a span of `d` px: a parabola sagging `SAG` px.
fn length(d: f32) -> f32 {
    d + 8.0 * SAG * SAG / (3.0 * d.max(SAG))
}

/// `pts` resampled to `n` points evenly along its length.
fn resample(pts: &[Pt], n: usize) -> Vec<Pt> {
    let mut acc = vec![0.0];
    for w in pts.windows(2) {
        acc.push(acc[acc.len() - 1] + (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1));
    }
    let total = acc[acc.len() - 1].max(1e-4);
    let mut j = 0;
    (0..n)
        .map(|i| {
            let at = total * i as f32 / (n - 1) as f32;
            while j + 2 < pts.len() && acc[j + 1] < at {
                j += 1;
            }
            let k = ((at - acc[j]) / (acc[j + 1] - acc[j]).max(1e-4)).clamp(0.0, 1.0);
            (pts[j].0 + (pts[j + 1].0 - pts[j].0) * k, pts[j].1 + (pts[j + 1].1 - pts[j].1) * k)
        })
        .collect()
}

impl Cables {
    /// A mixer paint starts: cables not connected again before `end` are unplugged.
    pub fn begin(&mut self) {
        for r in &mut self.ropes {
            r.seen = false;
        }
    }

    /// Plug cable `key` (the send into track `key`) from socket `a` (the current track's ⬇) into socket `b` (the
    /// send knob). `master`: either end is on the master strip.
    pub fn connect(&mut self, key: u16, a: Pt, b: Pt, master: bool, color: Color) {
        let len = length((b.0 - a.0).hypot(b.1 - a.1));
        let n = ((len / SPACING).round() as usize + 1).clamp(MIN_POINTS, MAX_POINTS);
        let seg = len / (n - 1) as f32;
        let Some(r) = self.ropes.iter_mut().find(|r| r.key == key) else {
            // A new cable starts at rest in its sag.
            let p: Vec<Pt> = (0..n)
                .map(|i| {
                    let t = i as f32 / (n - 1) as f32;
                    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t + 4.0 * SAG * t * (1.0 - t))
                })
                .collect();
            self.ropes.push(Rope { key, master, color, seg, q: p.clone(), p, seen: true });
            self.still = None;
            return;
        };
        r.seen = true;
        r.master = master;
        r.color = color;
        let last = r.p.len() - 1;
        let (da, db) = ((a.0 - r.p[0].0, a.1 - r.p[0].1), (b.0 - r.p[last].0, b.1 - r.p[last].1));
        if da == (0.0, 0.0) && db == (0.0, 0.0) && r.p.len() == n {
            return;
        }
        // The excess of a jump over CARRY bends the cable along (fading toward the other end); the rest the cable
        // feels as a tug on its socket.
        let excess = |(dx, dy): Pt| {
            let d = dx.hypot(dy);
            if d > CARRY { (dx * (1.0 - CARRY / d), dy * (1.0 - CARRY / d)) } else { (0.0, 0.0) }
        };
        let (ca, cb) = (excess(da), excess(db));
        for i in 0..=last {
            let t = i as f32 / last as f32;
            let (cx, cy) = (ca.0 * (1.0 - t) + cb.0 * t, ca.1 * (1.0 - t) + cb.1 * t);
            r.p[i] = (r.p[i].0 + cx, r.p[i].1 + cy);
            r.q[i] = (r.q[i].0 + cx, r.q[i].1 + cy);
        }
        if r.p.len() != n {
            r.p = resample(&r.p, n);
            r.q = resample(&r.q, n);
        }
        r.seg = seg;
        let last = n - 1;
        (r.p[0], r.q[0], r.p[last], r.q[last]) = (a, a, b, b);
        self.still = None;
    }

    /// The paint is over: unplug cables it did not connect.
    pub fn end(&mut self) {
        self.ropes.retain(|r| r.seen);
    }

    /// Unplug every cable (the mixer is hidden or collapsed).
    pub fn clear(&mut self) {
        self.ropes.clear();
        self.still = Some(SETTLE);
    }

    /// Whether any cable is still moving (the app keeps ticking fast while so).
    pub fn moving(&self) -> bool {
        !self.ropes.is_empty() && self.still.is_none_or(|n| n < SETTLE)
    }

    /// Every cable: whether it plugs into the master strip, its points and its colour.
    pub fn each(&self) -> impl Iterator<Item = (bool, &[Pt], Color)> {
        self.ropes.iter().map(|r| (r.master, &r.p[..], r.color))
    }

    /// Advance by `dt` seconds; the area the cables covered before and after (to repaint), if they moved.
    pub fn advance(&mut self, dt: f32) -> Option<Rect> {
        if !self.moving() {
            self.acc = 0.0;
            return None;
        }
        let before = self.bounds();
        self.acc = (self.acc + dt.min(MAX_DT)).min(MAX_DT);
        while self.acc >= STEP {
            self.acc -= STEP;
            let motion = self.ropes.iter_mut().map(substep).fold(0.0, f32::max);
            self.still = if motion < REST { Some(self.still.unwrap_or(0) + 1) } else { None };
            if !self.moving() {
                break;
            }
        }
        let after = self.bounds();
        match (before, after) {
            (Some(a), Some(b)) => Some(a.union(&b).outset(2.0)),
            (a, b) => a.or(b).map(|r| r.outset(2.0)),
        }
    }

    fn bounds(&self) -> Option<Rect> {
        let mut pts = self.ropes.iter().flat_map(|r| r.p.iter());
        let &(x, y) = pts.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (x, y, x, y);
        for &(x, y) in pts {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// One Verlet substep of a rope; returns how far its fastest point moved.
fn substep(r: &mut Rope) -> f32 {
    let g = GRAVITY * STEP * STEP;
    let last = r.p.len() - 1;
    for i in 1..last {
        let (p, q) = (r.p[i], r.q[i]);
        let (mut vx, mut vy) = ((p.0 - q.0) * DAMPING, (p.1 - q.1) * DAMPING);
        let v = vx.hypot(vy);
        if v > MAX_SPEED {
            (vx, vy) = (vx * MAX_SPEED / v, vy * MAX_SPEED / v);
        }
        r.q[i] = p;
        r.p[i] = (p.0 + vx, p.1 + vy + g);
    }
    for _ in 0..ITERATIONS {
        for i in 0..last {
            pull(&mut r.p, i, i + 1, r.seg, true);
        }
        for i in 0..last - 1 {
            pull(&mut r.p, i, i + 2, 2.0 * r.seg * BEND, false);
        }
    }
    (1..last).map(|i| (r.p[i].0 - r.q[i].0).hypot(r.p[i].1 - r.q[i].1)).fold(0.0, f32::max)
}

/// Move points `a` and `b` toward distance `len` (the sockets, the first and last points, never move). `exact`: hold
/// the distance both ways (a segment); else only push them apart, softly (bending).
fn pull(p: &mut [Pt], a: usize, b: usize, len: f32, exact: bool) {
    let last = p.len() - 1;
    let (dx, dy) = (p[b].0 - p[a].0, p[b].1 - p[a].1);
    let d = dx.hypot(dy).max(1e-4);
    if !exact && d >= len {
        return;
    }
    let (wa, wb) = (if a == 0 { 0.0 } else { 1.0 }, if b == last { 0.0 } else { 1.0 });
    if wa + wb == 0.0 {
        return;
    }
    let k = (d - len) / d / (wa + wb) * if exact { 1.0 } else { 0.25 };
    let (mx, my) = (dx * k, dy * k);
    p[a] = (p[a].0 + mx * wa, p[a].1 + my * wa);
    p[b] = (p[b].0 - mx * wb, p[b].1 - my * wb);
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: Color = Color::rgb(0x8ec63a);

    fn plug(c: &mut Cables, cables: &[(u16, Pt, Pt)]) {
        c.begin();
        for &(k, a, b) in cables {
            c.connect(k, a, b, false, GREEN);
        }
        c.end();
    }

    fn settle(c: &mut Cables) -> f32 {
        let mut t = 0.0;
        while c.moving() && t < 30.0 {
            c.advance(1.0 / 60.0);
            t += 1.0 / 60.0;
        }
        t
    }

    fn lowest(c: &Cables, i: usize) -> f32 {
        c.ropes[i].p.iter().map(|p| p.1).fold(f32::MIN, f32::max)
    }

    #[test]
    fn a_cable_joins_its_sockets_and_sags_between() {
        let mut c = Cables::default();
        plug(&mut c, &[(5, (100.0, 200.0), (320.0, 203.0))]);
        assert!(settle(&mut c) < 3.0);
        let r = &c.ropes[0];
        assert_eq!((r.p[0], r.p[r.p.len() - 1]), ((100.0, 200.0), (320.0, 203.0)));
        let sag = lowest(&c, 0) - 203.0;
        assert!(sag > SAG * 0.6 && sag < SAG * 2.0, "sags {sag}");
    }

    #[test]
    fn a_moved_window_swings_the_cable_then_it_settles() {
        let mut c = Cables::default();
        plug(&mut c, &[(5, (100.0, 200.0), (180.0, 203.0))]);
        settle(&mut c);
        let mid = |c: &Cables| c.ropes[0].p[c.ropes[0].p.len() / 2].0;
        let rest = mid(&c);
        // Drag the window right: the cable moves along, swings, then rests between its sockets again.
        plug(&mut c, &[(5, (120.0, 200.0), (200.0, 203.0))]);
        assert!(c.moving());
        c.advance(1.0 / 60.0);
        assert!(c.moving());
        let t = settle(&mut c);
        assert!(t > 0.3 && t < 15.0, "settles in {t}s");
        assert!((mid(&c) - rest - 20.0).abs() < 0.5);
    }

    #[test]
    fn a_track_change_moves_the_cables_and_unplugs_the_old_ones() {
        let mut c = Cables::default();
        plug(&mut c, &[(4, (50.0, 200.0), (230.0, 203.0)), (0, (50.0, 200.0), (20.0, 203.0))]);
        settle(&mut c);
        // The current track is now far right: the cable into track 4 is replugged (more points for the longer span).
        plug(&mut c, &[(4, (650.0, 200.0), (230.0, 203.0)), (2, (650.0, 200.0), (140.0, 203.0))]);
        assert_eq!(c.ropes.len(), 2);
        assert!(c.ropes.iter().all(|r| r.p.len() > MIN_POINTS));
        assert!(c.moving());
        assert!(settle(&mut c) < 15.0);
        for i in 0..2 {
            assert!(lowest(&c, i) < 203.0 + SAG * 2.5, "cable {i} hangs {}", lowest(&c, i));
        }
    }
}
