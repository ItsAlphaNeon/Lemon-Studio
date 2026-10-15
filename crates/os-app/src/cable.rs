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
//! its tick while any of them moves, and they fall asleep once still. The simulation's factors are `PARAMS`, tuned
//! live in the cable debug panel (views/cabledebug.rs, Mixer options ▸ Cable physics) and saved to `cables.txt`.
//!
//! With animations off (General settings ▸ Animations), cables hang still in a pre-rendered shape: the one the
//! simulation settles to for their span, worked out once (`settled`) and kept until a factor changes.

use os_ui::{Color, Rect};

/// Points per cable are bounded (the spacing factor sets about one per that many px of length).
const MIN_POINTS: usize = 6;
const MAX_POINTS: usize = 64;
/// Fixed substep, s.
const STEP: f32 = 1.0 / 240.0;
/// Below this motion per substep (px) for `SETTLE` substeps in a row, the cables are still.
const REST: f32 = 0.01;
const SETTLE: u32 = 30;
/// Longest frame stepped, s (a stall must not explode the ropes).
const MAX_DT: f32 = 0.05;
/// Samples of a cable's resting curve.
const CURVE: usize = 128;
/// Longest a pre-rendered shape is simulated for, substeps (10 s).
const SETTLE_MAX: usize = 2400;
/// A mixer paint (simulation included) slower than this while the cables swing is slow...
const SLOW: std::time::Duration = std::time::Duration::from_millis(8);
/// ...and this many slow paints in a row hang the cables still for the session.
const SLOW_PAINTS: u32 = 8;

/// A tunable factor of the simulation.
pub struct Param {
    pub name: &'static str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub decimals: usize,
    pub unit: &'static str,
    pub hint: &'static str,
}

const fn p(name: &'static str, default: f32, min: f32, max: f32, decimals: usize, unit: &'static str, hint: &'static str) -> Param {
    Param { name, default, min, max, decimals, unit, hint }
}

/// Indices into `PARAMS`.
pub mod k {
    pub const SAG: usize = 0;
    pub const GRAVITY: usize = 1;
    pub const KEPT: usize = 2;
    pub const STIFFNESS: usize = 3;
    pub const BEND: usize = 4;
    pub const ITERATIONS: usize = 5;
    pub const SPACING: usize = 6;
    pub const MAX_SPEED: usize = 7;
    pub const TUG: usize = 8;
    pub const KICK: usize = 9;
    pub const LIFT: usize = 10;
}

pub const PARAMS: [Param; 11] = [
    // Defaults tuned by eye in the debug panel (2026-10-15). The sockets sit about 39 px above the mixer's bottom, so
    // the sag dips the low point below the window.
    p("Sag", 58.0, 0.0, 150.0, 0, "px", "How far a cable sags below its sockets at rest (sets each cable's length)"),
    p("Gravity", 986.0, 0.0, 6000.0, 0, "px/s²", "Pull downward"),
    p("Speed kept", 0.01, 0.01, 1.0, 2, "/s", "Share of its speed a cable keeps each second (lower settles sooner)"),
    p("Stiffness", 1.0, 0.0, 1.0, 2, "x", "How straight a cable wants to stay (0 = a limp chain)"),
    p("Bend strength", 0.71, 0.0, 1.0, 2, "x", "How hard the stiffness pushes back"),
    p("Iterations", 16.0, 1.0, 60.0, 0, "", "Constraint passes per substep (more = less stretchy)"),
    p("Point spacing", 3.0, 3.0, 30.0, 0, "px", "Length between a cable's points (smaller = smoother, more work)"),
    p("Max speed", 878.0, 100.0, 10000.0, 0, "px/s", "Fastest a point may move"),
    p("Switch tug", 4.0, 0.0, 600.0, 0, "px", "On a track change or a jump, how much of the move the cable feels as a yank (the rest moves it along)"),
    p("Switch kick", 0.0, 0.0, 3000.0, 0, "px/s", "On a track change, a sideways shove against the move"),
    p("Switch lift", 43.0, 0.0, 3000.0, 0, "px/s", "On a track change, an upward pop"),
];

/// What the debug panel's controls do (views/cabledebug.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugAct {
    /// Drag the slider of factor `i`.
    Param(usize),
    Reset,
    Save,
    Close,
}

/// The simulation's factors (`PARAMS`).
#[derive(Clone, PartialEq)]
pub struct Tuning {
    pub v: [f32; PARAMS.len()],
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { v: PARAMS.map(|p| p.default) }
    }
}

impl Tuning {
    pub fn set(&mut self, i: usize, x: f32) {
        let p = &PARAMS[i];
        // Dividing by a power of ten gives back exactly the literal a value was written as (defaults, cables.txt).
        let scale = 10f32.powi(p.decimals as i32);
        self.v[i] = ((x.clamp(p.min, p.max) * scale).round() / scale).clamp(p.min, p.max);
    }
    pub fn norm(&self, i: usize) -> f32 {
        (self.v[i] - PARAMS[i].min) / (PARAMS[i].max - PARAMS[i].min)
    }
    pub fn text(&self, i: usize) -> String {
        let p = &PARAMS[i];
        format!("{:.*} {}", p.decimals, self.v[i], p.unit).trim_end().to_string()
    }
    /// One `name = value` line per factor (what the debug panel's Save writes).
    pub fn dump(&self) -> String {
        PARAMS.iter().zip(self.v).map(|(p, v)| format!("{} = {v:.*}\n", p.name, p.decimals)).collect()
    }
    pub fn load(&mut self, text: &str) {
        for line in text.lines() {
            let Some((name, val)) = line.split_once('=') else { continue };
            if let (Some(i), Ok(x)) = (PARAMS.iter().position(|p| p.name == name.trim()), val.trim().parse::<f32>()) {
                self.set(i, x);
            }
        }
    }
    /// A cable's shape at rest from socket `a` to socket `b`, finely sampled: a parabola sagging `Sag` px below the
    /// line between them. Its length is the cable's (so a new cable starts neither stretched nor buckled).
    fn curve(&self, a: Pt, b: Pt) -> Vec<Pt> {
        let sag = self.v[k::SAG];
        (0..=CURVE)
            .map(|i| {
                let t = i as f32 / CURVE as f32;
                (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t + 4.0 * sag * t * (1.0 - t))
            })
            .collect()
    }
}

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
    pub tuning: Tuning,
    /// The debug panel is open.
    pub debug: bool,
    /// Animations off: no simulation, every cable hangs still in its pre-rendered shape.
    pub still_cables: bool,
    /// Pre-rendered shapes by span (rounded, px), relative to the first socket.
    shapes: std::collections::HashMap<(i32, i32), Vec<Pt>>,
    ropes: Vec<Rope>,
    /// The current track at the last paint, and which way it moved this paint (-1 left, 1 right, 0 none).
    current: Option<u16>,
    switched: f32,
    /// Substeps in a row with every rope still; `None` = moving.
    still: Option<u32>,
    acc: f32,
    /// Time the last `advance` took, counted into the next paint's cost.
    sim: std::time::Duration,
    /// Slow paints in a row while swinging (`paint_cost`).
    slow: u32,
    /// The cables were made still because the mixer was slow to redraw them; not yet reported (`take_slowed`).
    slowed: bool,
}

fn arc_len(pts: &[Pt]) -> f32 {
    pts.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum()
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
    pub fn with(tuning: Tuning) -> Self {
        Cables { tuning, ..Default::default() }
    }

    /// Set factor `i` (the debug panel): the cables take it at once.
    pub fn set(&mut self, i: usize, x: f32) {
        self.tuning.set(i, x);
        self.shapes.clear();
        self.still = None;
    }

    /// Turn the simulation on or off (General settings ▸ Animations). Off, the cables snap to their pre-rendered
    /// shapes at the next paint; on, they start from there.
    pub fn animate(&mut self, on: bool) {
        self.still_cables = !on;
        self.still = None;
        self.slow = 0;
    }

    /// A mixer paint that ran while the cables swung took `cost`. Once the mixer has been too slow to redraw them
    /// at animation rate (a large project with many sends), they hang still in their pre-rendered shapes, for this
    /// session: the saved Animations setting stands, and choosing "Entertain me" tries again.
    pub fn paint_cost(&mut self, cost: std::time::Duration) {
        self.slow = if cost + std::mem::take(&mut self.sim) > SLOW { self.slow + 1 } else { 0 };
        if self.slow >= SLOW_PAINTS {
            self.animate(false);
            self.slowed = true;
        }
    }

    /// Whether `paint_cost` has just made the cables still (once).
    pub fn take_slowed(&mut self) -> bool {
        std::mem::take(&mut self.slowed)
    }

    /// A mixer paint starts, with track `current` current: cables not connected again before `end` are unplugged.
    pub fn begin(&mut self, current: u16) {
        self.switched = match self.current {
            Some(c) if current > c => 1.0,
            Some(c) if current < c => -1.0,
            _ => 0.0,
        };
        self.current = Some(current);
        for r in &mut self.ropes {
            r.seen = false;
        }
    }

    /// Plug cable `key` (the send into track `key`) from socket `a` (the current track's ⬇) into socket `b` (the
    /// send knob). `master`: either end is on the master strip.
    pub fn connect(&mut self, key: u16, a: Pt, b: Pt, master: bool, color: Color) {
        let tu = &self.tuning;
        let rest = tu.curve(a, b);
        let len = arc_len(&rest);
        let n = ((len / tu.v[k::SPACING]).round() as usize + 1).clamp(MIN_POINTS, MAX_POINTS);
        let seg = len / (n - 1) as f32;
        let tug = tu.v[k::TUG];
        if self.still_cables {
            return self.hang_still(key, a, b, master, color, seg);
        }
        let Some(r) = self.ropes.iter_mut().find(|r| r.key == key) else {
            // A new cable starts at rest in its sag.
            let p = resample(&rest, n);
            self.ropes.push(Rope { key, master, color, seg, q: p.clone(), p, seen: true });
            self.still = None;
            return;
        };
        r.seen = true;
        r.master = master;
        r.color = color;
        let last = r.p.len() - 1;
        let (da, db) = ((a.0 - r.p[0].0, a.1 - r.p[0].1), (b.0 - r.p[last].0, b.1 - r.p[last].1));
        if da == (0.0, 0.0) && db == (0.0, 0.0) && r.p.len() == n && r.seg == seg {
            return;
        }
        // The excess of a jump over the tug moves the cable's whole resting shape to the new sockets, keeping its
        // swing: the cable passes through itself rather than folding into a loop when one socket crosses the other.
        // The rest the cable feels as a yank on its socket.
        let excess = |(dx, dy): Pt| {
            let d = dx.hypot(dy);
            if d > tug { (dx * (1.0 - tug / d), dy * (1.0 - tug / d)) } else { (0.0, 0.0) }
        };
        let (ca, cb) = (excess(da), excess(db));
        if ca != (0.0, 0.0) || cb != (0.0, 0.0) {
            let (oa, ob) = (r.p[0], r.p[last]);
            let from = resample(&tu.curve(oa, ob), last + 1);
            let to = resample(&tu.curve((oa.0 + ca.0, oa.1 + ca.1), (ob.0 + cb.0, ob.1 + cb.1)), last + 1);
            for i in 0..=last {
                let (dx, dy) = (to[i].0 - from[i].0, to[i].1 - from[i].1);
                r.p[i] = (r.p[i].0 + dx, r.p[i].1 + dy);
                r.q[i] = (r.q[i].0 + dx, r.q[i].1 + dy);
            }
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

    /// Animations off: cable `key` hangs still in the pre-rendered shape for its span.
    fn hang_still(&mut self, key: u16, a: Pt, b: Pt, master: bool, color: Color, seg: f32) {
        let span = ((b.0 - a.0).round() as i32, (b.1 - a.1).round() as i32);
        let tu = &self.tuning;
        let shape = self.shapes.entry(span).or_insert_with(|| settled(tu, (span.0 as f32, span.1 as f32)));
        let mut p: Vec<Pt> = shape.iter().map(|&(x, y)| (a.0 + x, a.1 + y)).collect();
        let last = p.len() - 1;
        (p[0], p[last]) = (a, b);
        let rope = Rope { key, master, color, seg, q: p.clone(), p, seen: true };
        match self.ropes.iter_mut().find(|r| r.key == key) {
            Some(r) => *r = rope,
            None => self.ropes.push(rope),
        }
    }

    /// The paint is over: unplug cables it did not connect, and give the rest the track change's kick and lift.
    pub fn end(&mut self) {
        self.ropes.retain(|r| r.seen);
        let (kick, lift) = (self.tuning.v[k::KICK], self.tuning.v[k::LIFT]);
        if self.still_cables || self.switched == 0.0 || (kick == 0.0 && lift == 0.0) {
            return;
        }
        // A velocity change, strongest mid-cable (the sockets hold the ends): against the move, and up.
        let (dvx, dvy) = (-self.switched * kick * STEP, -lift * STEP);
        for r in &mut self.ropes {
            let last = r.p.len() - 1;
            for i in 1..last {
                let w = (std::f32::consts::PI * i as f32 / last as f32).sin();
                r.q[i] = (r.q[i].0 - dvx * w, r.q[i].1 - dvy * w);
            }
        }
        self.still = None;
    }

    /// Unplug every cable (the mixer is hidden or collapsed).
    pub fn clear(&mut self) {
        self.ropes.clear();
        self.current = None;
        self.still = Some(SETTLE);
    }

    /// Whether any cable is still moving (the app keeps ticking fast while so).
    pub fn moving(&self) -> bool {
        !self.still_cables && !self.ropes.is_empty() && self.still.is_none_or(|n| n < SETTLE)
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
        let t0 = std::time::Instant::now();
        let before = self.bounds();
        self.acc = (self.acc + dt.min(MAX_DT)).min(MAX_DT);
        let tu = &self.tuning;
        while self.acc >= STEP {
            self.acc -= STEP;
            let motion = self.ropes.iter_mut().map(|r| substep(r, tu)).fold(0.0, f32::max);
            self.still = if motion < REST { Some(self.still.unwrap_or(0) + 1) } else { None };
            if !self.ropes.is_empty() && self.still.is_some_and(|n| n >= SETTLE) {
                break;
            }
        }
        let after = self.bounds();
        self.sim = t0.elapsed();
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

/// The shape a cable spanning `d` from its first socket settles to (points relative to that socket): its resting curve,
/// simulated until still.
fn settled(tu: &Tuning, d: Pt) -> Vec<Pt> {
    let rest = tu.curve((0.0, 0.0), d);
    let len = arc_len(&rest);
    let n = ((len / tu.v[k::SPACING]).round() as usize + 1).clamp(MIN_POINTS, MAX_POINTS);
    let p = resample(&rest, n);
    let mut r = Rope { key: 0, master: false, color: Color::rgb(0), seg: len / (n - 1) as f32, q: p.clone(), p, seen: true };
    let mut still = 0;
    for _ in 0..SETTLE_MAX {
        still = if substep(&mut r, tu) < REST { still + 1 } else { 0 };
        if still >= SETTLE {
            break;
        }
    }
    r.p
}

/// One Verlet substep of a rope; returns how far its fastest point moved.
fn substep(r: &mut Rope, tu: &Tuning) -> f32 {
    let g = tu.v[k::GRAVITY] * STEP * STEP;
    let damping = tu.v[k::KEPT].powf(STEP);
    let max_speed = tu.v[k::MAX_SPEED] * STEP;
    let (stiff, bend) = (tu.v[k::STIFFNESS], tu.v[k::BEND]);
    let last = r.p.len() - 1;
    for i in 1..last {
        let (p, q) = (r.p[i], r.q[i]);
        let (mut vx, mut vy) = ((p.0 - q.0) * damping, (p.1 - q.1) * damping);
        let v = vx.hypot(vy);
        if v > max_speed {
            (vx, vy) = (vx * max_speed / v, vy * max_speed / v);
        }
        r.q[i] = p;
        r.p[i] = (p.0 + vx, p.1 + vy + g);
    }
    for _ in 0..tu.v[k::ITERATIONS] as usize {
        for i in 0..last {
            pull(&mut r.p, i, i + 1, r.seg, 1.0, true);
        }
        if stiff > 0.0 && bend > 0.0 {
            for i in 0..last - 1 {
                pull(&mut r.p, i, i + 2, 2.0 * r.seg * stiff, bend, false);
            }
        }
    }
    (1..last).map(|i| (r.p[i].0 - r.q[i].0).hypot(r.p[i].1 - r.q[i].1)).fold(0.0, f32::max)
}

/// Move points `a` and `b` toward distance `len` by `share` of the error (the sockets, the first and last points, never
/// move). `exact`: hold the distance both ways (a segment); else only push them apart (bending).
fn pull(p: &mut [Pt], a: usize, b: usize, len: f32, share: f32, exact: bool) {
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
    let k = (d - len) / d / (wa + wb) * share;
    let (mx, my) = (dx * k, dy * k);
    p[a] = (p[a].0 + mx * wa, p[a].1 + my * wa);
    p[b] = (p[b].0 - mx * wb, p[b].1 - my * wb);
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: Color = Color::rgb(0x8ec63a);
    const SAG: f32 = PARAMS[k::SAG].default;

    fn plug(c: &mut Cables, current: u16, cables: &[(u16, Pt, Pt)]) {
        c.begin(current);
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
        plug(&mut c, 1, &[(5, (100.0, 200.0), (320.0, 203.0))]);
        assert!(settle(&mut c) < 3.0);
        let r = &c.ropes[0];
        assert_eq!((r.p[0], r.p[r.p.len() - 1]), ((100.0, 200.0), (320.0, 203.0)));
        let sag = lowest(&c, 0) - 203.0;
        assert!(sag > SAG * 0.6 && sag < SAG * 2.0, "sags {sag}");
    }

    #[test]
    fn a_moved_window_swings_the_cable_then_it_settles() {
        let mut c = Cables::default();
        plug(&mut c, 1, &[(5, (100.0, 200.0), (180.0, 203.0))]);
        settle(&mut c);
        let mid = |c: &Cables| c.ropes[0].p[c.ropes[0].p.len() / 2].0;
        let rest = mid(&c);
        // Drag the window right: the cable moves along, swings, then rests between its sockets again.
        plug(&mut c, 1, &[(5, (120.0, 200.0), (200.0, 203.0))]);
        assert!(c.moving());
        c.advance(1.0 / 60.0);
        assert!(c.moving());
        let t = settle(&mut c);
        assert!(t > 0.3 && t < 15.0, "settles in {t}s");
        let off = mid(&c) - rest - 20.0;
        // A slack cable with some stiffness may rest a few px from its earlier pose.
        assert!(off.abs() < 10.0, "rests {off} px off");
    }

    #[test]
    fn a_track_change_moves_the_cables_and_unplugs_the_old_ones() {
        let mut c = Cables::default();
        plug(&mut c, 1, &[(4, (50.0, 200.0), (230.0, 203.0)), (0, (50.0, 200.0), (20.0, 203.0))]);
        settle(&mut c);
        // The current track is now far right: the cable into track 4 is replugged (more points for the longer span).
        plug(&mut c, 9, &[(4, (650.0, 200.0), (230.0, 203.0)), (2, (650.0, 200.0), (140.0, 203.0))]);
        assert_eq!(c.ropes.len(), 2);
        assert!(c.ropes.iter().all(|r| r.p.len() > MIN_POINTS));
        assert!(c.moving());
        assert!(settle(&mut c) < 15.0);
        for i in 0..2 {
            assert!(lowest(&c, i) < 203.0 + SAG * 2.5, "cable {i} hangs {}", lowest(&c, i));
        }
    }

    #[test]
    fn a_track_change_kicks_and_lifts_the_cables() {
        let mut c = Cables::default();
        c.set(k::KICK, 1500.0);
        c.set(k::LIFT, 1500.0);
        plug(&mut c, 1, &[(4, (50.0, 200.0), (230.0, 203.0))]);
        settle(&mut c);
        let mid = |c: &Cables| c.ropes[0].p[c.ropes[0].p.len() / 2];
        let rest = mid(&c);
        // Same sockets, but the current track moved right: the middle is shoved left and up.
        plug(&mut c, 3, &[(4, (50.0, 200.0), (230.0, 203.0))]);
        c.advance(1.0 / 30.0);
        let now = mid(&c);
        assert!(now.0 < rest.0 - 1.0 && now.1 < rest.1 - 1.0, "{rest:?} -> {now:?}");
        assert!(settle(&mut c) < 15.0);
    }


    fn crossings(pts: &[Pt]) -> usize {
        let cross = |a: Pt, b: Pt, c: Pt, d: Pt| {
            let o = |p: Pt, q: Pt, r: Pt| (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0);
            (o(a, b, c) > 0.0) != (o(a, b, d) > 0.0) && (o(c, d, a) > 0.0) != (o(c, d, b) > 0.0)
        };
        let n = pts.len();
        (0..n - 1).map(|i| (i + 2..n - 1).filter(|&j| cross(pts[i], pts[i + 1], pts[j], pts[j + 1])).count()).sum()
    }

    #[test]
    fn a_socket_passing_the_other_does_not_loop_the_cable() {
        let mut c = Cables::default();
        plug(&mut c, 1, &[(4, (50.0, 200.0), (230.0, 198.0)), (0, (50.0, 200.0), (20.0, 198.0))]);
        for (cur, x) in [(9, 650.0), (1, 50.0), (2, 90.0)] {
            plug(&mut c, cur, &[(4, (x, 200.0), (230.0, 198.0)), (0, (x, 200.0), (20.0, 198.0))]);
            for f in 0..90 {
                c.advance(1.0 / 60.0);
                for r in &c.ropes {
                    assert_eq!(crossings(&r.p), 0, "cable {} crosses itself {f} frames after the switch to {cur}", r.key);
                }
            }
        }
    }

    #[test]
    fn still_cables_hang_in_the_settled_shape() {
        let (a, b) = ((50.0, 200.0), (230.0, 198.0));
        let mut sim = Cables::default();
        plug(&mut sim, 1, &[(4, a, b)]);
        settle(&mut sim);
        let mut still = Cables::default();
        still.animate(false);
        plug(&mut still, 1, &[(4, a, b)]);
        assert!(!still.moving());
        assert_eq!(still.advance(1.0 / 60.0), None);
        // The pre-rendered shape is where the simulation comes to rest (within its rest slack).
        let (s, t) = (&sim.ropes[0].p, &still.ropes[0].p);
        assert_eq!(s.len(), t.len());
        let off = s.iter().zip(t).map(|(p, q)| (p.0 - q.0).hypot(p.1 - q.1)).fold(0.0, f32::max);
        assert!(off < 4.0, "{off} px from the settled cable");
        assert_eq!((t[0], t[t.len() - 1]), (a, b));
        // A track change moves the still cable without a swing, and the kick does not apply.
        still.set(k::KICK, 2000.0);
        plug(&mut still, 3, &[(4, (140.0, 200.0), b)]);
        assert!(!still.moving());
        assert_eq!(still.ropes[0].p[0], (140.0, 200.0));
    }

    #[test]
    fn a_mixer_slow_to_redraw_hangs_the_cables_still() {
        use std::time::Duration;
        let ms = Duration::from_millis;
        let mut c = Cables::default();
        plug(&mut c, 1, &[(4, (50.0, 200.0), (230.0, 198.0))]);
        assert!(c.moving());
        // A few slow paints, or slow ones broken by a quick one, are not enough.
        for _ in 0..SLOW_PAINTS - 1 {
            c.paint_cost(ms(20));
        }
        c.paint_cost(ms(2));
        for _ in 0..SLOW_PAINTS - 1 {
            c.paint_cost(ms(20));
        }
        assert!(c.moving() && !c.take_slowed());
        c.paint_cost(ms(20));
        assert!(c.still_cables && !c.moving(), "slow paints in a row hang the cables still");
        assert!(c.take_slowed() && !c.take_slowed(), "reported once");
        // Turning animations back on tries again.
        c.animate(true);
        c.paint_cost(ms(20));
        assert!(!c.still_cables);
    }

    #[test]
    fn tuning_saves_and_loads() {
        let mut t = Tuning::default();
        t.set(k::GRAVITY, 2500.0);
        t.set(k::KEPT, 0.123);
        let mut u = Tuning::default();
        u.load(&t.dump());
        assert_eq!(u.v[k::GRAVITY], 2500.0);
        assert!((u.v[k::KEPT] - 0.12).abs() < 1e-6);
        assert!(u == t);
    }
}
