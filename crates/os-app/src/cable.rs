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
    /// A cable's length for a span of `d` px: a parabola sagging `Sag` px.
    fn length(&self, d: f32) -> f32 {
        let sag = self.v[k::SAG];
        d + 8.0 * sag * sag / (3.0 * d.max(sag).max(1.0))
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
    ropes: Vec<Rope>,
    /// The current track at the last paint, and which way it moved this paint (-1 left, 1 right, 0 none).
    current: Option<u16>,
    switched: f32,
    /// Substeps in a row with every rope still; `None` = moving.
    still: Option<u32>,
    acc: f32,
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
        self.still = None;
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
        let len = tu.length((b.0 - a.0).hypot(b.1 - a.1));
        let n = ((len / tu.v[k::SPACING]).round() as usize + 1).clamp(MIN_POINTS, MAX_POINTS);
        let seg = len / (n - 1) as f32;
        let (sag, tug) = (tu.v[k::SAG], tu.v[k::TUG]);
        let Some(r) = self.ropes.iter_mut().find(|r| r.key == key) else {
            // A new cable starts at rest in its sag.
            let p: Vec<Pt> = (0..n)
                .map(|i| {
                    let t = i as f32 / (n - 1) as f32;
                    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t + 4.0 * sag * t * (1.0 - t))
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
        if da == (0.0, 0.0) && db == (0.0, 0.0) && r.p.len() == n && r.seg == seg {
            return;
        }
        // The excess of a jump over the tug bends the cable along (fading toward the other end); the rest the cable
        // feels as a yank on its socket.
        let excess = |(dx, dy): Pt| {
            let d = dx.hypot(dy);
            if d > tug { (dx * (1.0 - tug / d), dy * (1.0 - tug / d)) } else { (0.0, 0.0) }
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

    /// The paint is over: unplug cables it did not connect, and give the rest the track change's kick and lift.
    pub fn end(&mut self) {
        self.ropes.retain(|r| r.seen);
        let (kick, lift) = (self.tuning.v[k::KICK], self.tuning.v[k::LIFT]);
        if self.switched == 0.0 || (kick == 0.0 && lift == 0.0) {
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
