// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Automation curves: the thirteen segment shapes and the walk that plays a curve one tick at a time, in the
//! precision the host uses (singles where the page says so, doubles elsewhere).
//!
// spec: audio/automation.md ("Walking the curve", "Segment shapes")

use crate::{AutoMode, AutoPoint};

/// `2^-24`: the smallest segment length, slope term and rise the smooth mode works with.
const TINY: f64 = 1.0 / 16_777_216.0;

/// Rounding to the nearest whole number, halves to even (the host's rounding).
fn round(x: f64) -> f64 {
    x.round_ties_even()
}

/// Point `i`'s distance from the point before as a single, in beats (the first point's from the curve's start); a
/// negative distance counts as 0.
fn distance(pts: &[AutoPoint], i: usize) -> f32 {
    let before = if i == 0 { 0.0 } else { pts[i - 1].beat };
    ((pts[i].beat - before) as f32).max(0.0)
}

/// Point `i`'s level as a single.
fn level(pts: &[AutoPoint], i: usize) -> f32 {
    pts[i].value as f32
}

/// The count of steps, pulses or waves of modes 3 to 6: 1000 at tension 0, falling to 0 at full tension.
fn count(t: f32) -> f64 {
    let reach = (1.0 - f64::from(t.abs()) * 0.6) as f32;
    let e = ((f64::from(reach) * 1_000_001f64.ln()).exp() - 1.0) as f32;
    round(f64::from(e) * 0.001)
}

/// The shape of one segment: `progress(x)` maps the position 0..1 along it to 0..1 between its two levels. Its
/// coefficients are worked out once, from the point the segment ends on (and for the smooth mode its neighbours).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    mode: AutoMode,
    /// The tension is below 0.
    neg: bool,
    /// A straight line, whatever the mode.
    line: bool,
    c0: f32,
    c1: f32,
    c2: f32,
}

impl Segment {
    /// The segment that ends on point `i` (`i >= 1`; point 0 shapes nothing and gives a straight line).
    pub fn new(pts: &[AutoPoint], i: usize) -> Segment {
        let line = Segment { mode: AutoMode::Single, neg: false, line: true, c0: 0.0, c1: 0.0, c2: 0.0 };
        let Some(p) = pts.get(i).filter(|_| i > 0) else { return line };
        let t = p.tension;
        let side: f64 = if t < 0.0 { -1.0 } else { 1.0 };
        let mut s = Segment { mode: p.mode, neg: t < 0.0, line: false, c0: 0.0, c1: 0.0, c2: 0.0 };
        use AutoMode::*;
        // Modes 0, 1, 7, 8, 9, 11 and 12 are a straight line at a tension of exactly 0.
        if t == 0.0 && matches!(p.mode, Single | Double | Single2 | Double2 | HalfSine | Single3 | Double3) {
            return line;
        }
        match p.mode {
            Single | Double => {
                let v = ((f64::from(t.abs()) * 1_000_001f64.ln()).exp() - 1.0) as f32;
                s.c1 = (1.0 / f64::from(v)) as f32;
                s.c2 = (f64::from(v) + 1.0).ln() as f32;
            }
            Stairs => {
                let n = count(t) + 1.0 + if t >= 0.0 { 1.0 } else { 0.0 };
                s.c0 = n as f32;
                s.c1 = (1.0 / (n - side)) as f32;
                s.c2 = (side * 0.5) as f32;
            }
            SmoothStairs => {
                s.c0 = (count(t) + 1.0) as f32;
                s.c1 = (1.0 / f64::from(s.c0)) as f32;
            }
            Pulse => {
                s.c0 = ((count(t) + 1.0) * 2.0) as f32;
                s.c2 = (side * 0.5) as f32;
            }
            Wave => {
                let n = count(t);
                if n == 0.0 && t > 0.0 {
                    return line;
                }
                s.c0 = (n + 0.5) as f32;
                s.c1 = std::f64::consts::TAU as f32;
            }
            Single2 | Double2 => s.c0 = t,
            HalfSine => s.c0 = t.abs(),
            Single3 | Double3 => {
                let a = 1.0 - (f64::from(t.abs()) * 0.9).max(1.0 / 65536.0);
                let k = (1.0 - a * a * a) as f32;
                let mut c1 = (1.0 - k) / k;
                let mut c2 = c1;
                if t > 0.0 {
                    c1 = -c1;
                    c2 = c1 - 1.0;
                }
                if p.mode == Double3 {
                    c1 *= 0.5;
                    c2 *= 0.5;
                }
                (s.c0, s.c1, s.c2) = (k, c1, c2);
            }
            Smooth => {
                let (y0, y1) = (level(pts, i - 1), level(pts, i));
                let len = f64::from(distance(pts, i)).max(TINY);
                let rise = f64::from(y1) - f64::from(y0);
                if rise.abs() < TINY {
                    return line;
                }
                // The neighbours count when they are smooth too.
                let (mut before, mut back) = (f64::from(y0), 1.0);
                if pts[i - 1].mode == Smooth {
                    back = f64::from(distance(pts, i - 1)) / len;
                    if i >= 2 {
                        before = f64::from(level(pts, i - 2));
                    }
                }
                let (mut after, mut ahead) = (f64::from(y1), 1.0);
                if let Some(q) = pts.get(i + 1).filter(|q| q.mode == Smooth) {
                    ahead = f64::from(distance(pts, i + 1)) / len;
                    after = f64::from(q.value as f32);
                }
                let back = back.max(TINY) as f32;
                let ahead = ahead.max(TINY) as f32;
                let rise = rise as f32;
                let out = ((((after - f64::from(y0)) / f64::from(rise)) - 1.0) / f64::from(ahead)) as f32;
                let into = (-((before - f64::from(y0)) / f64::from(rise)) / f64::from(back)) as f32;
                s.c0 = if out > 0.0 {
                    let w = 2.0 * ahead + 1.0;
                    out * (w + ahead + 2.0) / (w * out + (ahead + 2.0))
                } else {
                    0.0
                };
                s.c1 = if into > 0.0 {
                    let w = 2.0 * back + 1.0;
                    into * (back + 2.0 + w) / ((back + 2.0) + w * into)
                } else {
                    0.0
                };
                s.c2 = 1.0 - t.abs();
            }
            Hold | Other(_) => {}
        }
        s
    }

    /// The shape at `x` (0..1 along the segment), computed in doubles and rounded to single.
    pub fn progress(&self, x: f64) -> f32 {
        if self.line {
            return x as f32;
        }
        let (c0, c1, c2) = (f64::from(self.c0), f64::from(self.c1), f64::from(self.c2));
        let neg = self.neg;
        let single = |x: f64| if neg { ((x * c2).exp() - 1.0) * c1 } else { 1.0 - (((1.0 - x) * c2).exp() - 1.0) * c1 };
        let curve2 = |a: f64| {
            let u = if neg { a } else { 1.0 - a };
            a - (u.powi(6) - u) * c0
        };
        let curve3 = |a: f64| {
            let u = if neg { a } else { 1.0 - a };
            c1 / (1.0 - c0 * u)
        };
        use AutoMode::*;
        let v = match self.mode {
            Single => single(x),
            Double => {
                if x < 0.5 { single(2.0 * x) / 2.0 } else { 1.0 - single(2.0 * (1.0 - x)) / 2.0 }
            }
            Hold => 0.0,
            Stairs => f64::from(round(x * c0 - c2) as f32) * c1,
            SmoothStairs => {
                let a = x * c0 - 0.5;
                let k = round(a - 0.5);
                let mut d = (a - k) * 2.0 - 1.0;
                if neg {
                    let e = 1.0 - d.abs();
                    d = (1.0 - e * e * e).copysign(d);
                } else {
                    d = d * d * d;
                }
                (k + d / 2.0 + 1.0) * c1
            }
            Pulse => (round(x * c0 - c2) as i64 & 1) as f64,
            Wave => {
                if neg {
                    (((x * c0 + 0.75) * c1).sin() + 1.0) / 2.0
                } else {
                    let a = x * c0 - 0.5;
                    ((a - round(a - 0.5)) * 2.0 - 1.0).abs()
                }
            }
            Single2 => curve2(x),
            Double2 => {
                if x < 0.5 { curve2(2.0 * x) / 2.0 } else { 1.0 - curve2(2.0 * (1.0 - x)) / 2.0 }
            }
            HalfSine => {
                let a = x * f64::from(std::f64::consts::FRAC_PI_2 as f32);
                let h = if neg { 1.0 - a.cos() } else { a.sin() };
                x - (x - h) * c0
            }
            Smooth => {
                let (x2, x3) = (x * x, x * x * x);
                x + ((3.0 * x2 - 2.0 * x3) + x2 * (x - 1.0) * c0 + x * (x - 1.0) * (x - 1.0) * c1 - x) * c2
            }
            Single3 => curve3(x) - c2,
            Double3 => {
                if x < 0.5 { curve3(2.0 * x) - c2 } else { (1.0 - curve3(2.0 * (1.0 - x))) + c2 }
            }
            Other(_) => 0.0,
        };
        v as f32
    }
}

/// The curve's level `x` (0..1) of the way along the segment that ends on point `i`, between the levels of points
/// `i - 1` and `i`: what the walk gives there, for drawing and for values between ticks.
pub fn segment_value(pts: &[AutoPoint], i: usize, x: f64) -> f64 {
    let (from, to) = (level(pts, i - 1), level(pts, i));
    let by = to - from;
    if by == 0.0 {
        return f64::from(from);
    }
    f64::from(Segment::new(pts, i).progress(x) * by + from)
}

/// The step the walk takes per tick: `single(1 / PPQ)`, used as a double (at 96 PPQ about 3e-8 too long, so a stair
/// that falls exactly on a tick has already happened on it).
pub fn tick_step(ppq: u16) -> f64 {
    f64::from((1.0 / f64::from(ppq.max(1))) as f32)
}

/// FL's walk along a curve: it keeps its place inside the current segment and moves on by steps of beats, so that a
/// stair edge that falls exactly on a step lands on the same side as in FL.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walk {
    /// The point the current segment ends on.
    i: usize,
    /// Phase 0..1 along the segment and its rate (1 / the segment's length).
    p: f64,
    r: f64,
    /// The level at the segment's start and the difference across it.
    from: f32,
    by: f32,
    /// The current segment's shape, worked out when first needed.
    seg: Option<Segment>,
}

impl Walk {
    /// The walk at the curve's start (`None` for a curve without points).
    pub fn new(pts: &[AutoPoint]) -> Option<Walk> {
        let first = pts.first()?;
        let d = distance(pts, 0);
        let (p, r) = if d == 0.0 { (1.0, 1.0) } else { (0.0, 1.0 / f64::from(d)) };
        Some(Walk { i: 0, p, r, from: first.value as f32, by: 0.0, seg: None })
    }

    /// One step of `b` beats.
    pub fn step(&mut self, pts: &[AutoPoint], b: f64) {
        self.p += b * self.r;
        while self.p >= 1.0 {
            let left = if self.r == 0.0 { b } else { (self.p - 1.0) / self.r };
            self.from = level(pts, self.i);
            let moved = self.i + 1 < pts.len();
            if moved {
                self.i += 1;
            }
            self.by = level(pts, self.i) - self.from;
            self.seg = None;
            if !moved {
                // Past the last point: the level stays.
                (self.p, self.r) = (0.0, 0.0);
            } else if distance(pts, self.i) > 0.0 {
                self.r = 1.0 / f64::from(distance(pts, self.i));
                self.p = left * self.r;
            } else {
                // A point at the same place as the one before is passed in the same step.
                self.r = 1.0;
                self.p = left + 1.0;
            }
        }
    }

    /// The level where the walk is: `single(single(single(progress(p)) * by) + from)`.
    pub fn value(&mut self, pts: &[AutoPoint]) -> f32 {
        if self.by == 0.0 {
            return self.from;
        }
        let i = self.i;
        let seg = *self.seg.get_or_insert_with(|| Segment::new(pts, i));
        seg.progress(self.p) * self.by + self.from
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(beat: f64, value: f64, mode: AutoMode, tension: f32) -> AutoPoint {
        AutoPoint { beat, value, tension, mode, flag: 2 }
    }

    /// Every shape starts at 0 and arrives at 1, at several tensions (pulse and wave end where their count puts them;
    /// stairs with a negative tension take their first step at once and their last at the point itself).
    #[test]
    fn shapes_run_from_the_start_to_the_end() {
        use AutoMode::*;
        for mode in [Single, Double, Stairs, SmoothStairs, Single2, Double2, HalfSine, Smooth, Single3, Double3] {
            for t in [-1.0f32, -0.5, -0.1, 0.0, 0.1, 0.5, 1.0] {
                let pts = [pt(0.0, 0.0, Single, 0.0), pt(1.0, 1.0, mode, t)];
                let s = Segment::new(&pts, 1);
                let (a, b) = (s.progress(0.0), s.progress(1.0 - 1e-9));
                assert!(a.abs() < 2e-3, "{mode:?} {t}: starts at {a}");
                if !(mode == Stairs && t < 0.0) {
                    assert!((b - 1.0).abs() < 2e-3, "{mode:?} {t}: ends at {b}");
                }
                // In between it stays inside the segment.
                for k in 1..100 {
                    let v = s.progress(f64::from(k) / 100.0);
                    assert!((-1e-3..=1.0 + 1e-3).contains(&v), "{mode:?} {t} at {k}%: {v}");
                }
            }
        }
    }

    /// Spot values from the formulas: a positive single curve starts fast, a negative one slowly; hold stays; the
    /// counts of modes 3 to 6 fall from 1000 at tension 0 to 0 at full tension.
    #[test]
    fn shapes_follow_the_page() {
        use AutoMode::*;
        let seg = |mode, t| Segment::new(&[pt(0.0, 0.0, Single, 0.0), pt(1.0, 1.0, mode, t)], 1);
        assert_eq!(count(0.0), 1000.0);
        assert_eq!(count(1.0), 0.0);
        assert_eq!(count(-1.0), 0.0);
        assert!(seg(Single, 0.5).progress(0.5) > 0.9 && seg(Single, -0.5).progress(0.5) < 0.1);
        assert_eq!(seg(Single, 0.0).progress(0.25), 0.25, "a straight line at tension 0");
        assert_eq!(seg(Hold, 0.3).progress(0.99), 0.0);
        // Stairs at full positive tension: n = 2 steps, c1 = 1: 0 below the middle, 1 above.
        let st = seg(Stairs, 1.0);
        assert_eq!((st.progress(0.2), st.progress(0.8)), (0.0, 1.0));
        // Pulse at full tension: two half-cycles, low then high.
        let pu = seg(Pulse, 1.0);
        assert_eq!((pu.progress(0.2), pu.progress(0.7)), (0.0, 1.0));
        // Wave at full negative tension: half a sine cycle from the bottom to the top.
        let wa = seg(Wave, -1.0);
        assert!(wa.progress(0.0).abs() < 1e-6 && (wa.progress(0.5) - 0.5).abs() < 1e-6 && (wa.progress(1.0) - 1.0).abs() < 1e-6);
        // Single curve 2 at tension 1, halfway: 0.5 + (0.5 - 0.5^6).
        assert!((seg(Single2, 1.0).progress(0.5) - (1.0 - 0.015625)).abs() < 1e-6);
        // Half sine, positive full tension: sin(pi/4) halfway.
        assert!((seg(HalfSine, 1.0).progress(0.5) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        // Stairs at full negative tension: the first step at once, the last at the point.
        let sn = seg(Stairs, -1.0);
        assert_eq!((sn.progress(0.0), sn.progress(0.3), sn.progress(0.9)), (0.0, 0.5, 0.5));
        // Smooth between smooth points rising evenly is a straight line; the first segment leaves flat.
        let pts = [pt(0.0, 0.0, Smooth, 0.0), pt(1.0, 1.0 / 3.0, Smooth, 0.0), pt(2.0, 2.0 / 3.0, Smooth, 0.0), pt(3.0, 1.0, Smooth, 0.0)];
        for x in [0.1, 0.5, 0.8] {
            assert!((Segment::new(&pts, 2).progress(x) - x as f32).abs() < 1e-5);
        }
        assert!(Segment::new(&pts, 1).progress(0.5) < 0.45, "flat start, steeper end");
    }

    /// The walk: a wait before the curve, a ramp, and the last level held; a point at the same place as the one
    /// before is passed in the same step.
    #[test]
    fn the_walk_steps_through_the_points() {
        use AutoMode::*;
        let pts = [pt(1.0, 0.0, Single, 0.0), pt(3.0, 1.0, Single, 0.0), pt(3.0, 0.25, Hold, 0.0), pt(4.0, 0.25, Single, 0.0)];
        let mut w = Walk::new(&pts).unwrap();
        let at = |w: &mut Walk, beats: f64| {
            w.step(&pts, beats);
            w.value(&pts)
        };
        assert_eq!(w.value(&pts), 0.0, "the first level before anything");
        assert_eq!(at(&mut w, 0.5), 0.0, "still waiting");
        assert_eq!(at(&mut w, 1.5), 0.5, "halfway along the ramp from beat 1 to 3");
        // Beat 3: the ramp ends, the next point at the same place is passed, the hold from 0.25 starts.
        assert_eq!(at(&mut w, 1.0), 0.25);
        assert_eq!(at(&mut w, 5.0), 0.25, "past the last point the level stays");
    }

    /// One step per tick of single(1/96): a stair edge that falls exactly on a tick has already happened on it.
    #[test]
    fn a_stair_on_a_tick_has_happened_on_that_tick() {
        use AutoMode::*;
        let pts = [pt(0.0, 0.0, Single, 0.0), pt(1.0, 0.0, Single, 0.0), pt(1.0, 1.0, Hold, 0.0), pt(2.0, 1.0, Single, 0.0)];
        let mut w = Walk::new(&pts).unwrap();
        let b = tick_step(96);
        let mut v = Vec::new();
        for _ in 0..97 {
            w.step(&pts, b);
            v.push(w.value(&pts));
        }
        // After 96 steps (exactly one beat by the ticks) the jump to 1 has happened.
        assert_eq!(v[94], 0.0);
        assert_eq!(v[95], 1.0, "{:?}", &v[90..97]);
    }

    /// Between ticks the drawn curve agrees with the walk.
    #[test]
    fn segment_values_match_the_walk() {
        use AutoMode::*;
        let pts = [pt(0.0, 0.2, Single, 0.0), pt(2.0, 0.9, Single, 0.4), pt(3.0, 0.1, Double2, -0.6)];
        let mut w = Walk::new(&pts).unwrap();
        let b = tick_step(96);
        for k in 1..=288 {
            w.step(&pts, b);
            let beat = f64::from(k) * b;
            let want = if beat < 2.0 { segment_value(&pts, 1, beat / 2.0) } else { segment_value(&pts, 2, beat - 2.0) };
            assert!((f64::from(w.value(&pts)) - want).abs() < 1e-4, "tick {k}");
        }
    }
}
