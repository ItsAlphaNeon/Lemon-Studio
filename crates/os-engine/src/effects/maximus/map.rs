// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A band's map as a table of gains: the map walked like an automation clip's curve, with every value in doubles
//! (the stairs' progress excepted), 8192 entries per unit of input level up to level 4.
//!
// spec: plugins/maximus.md ("The curve table"), audio/automation.md ("Walking the curve", "Segment shapes")

use super::state::Map;

/// The table's entries: levels 0..4 at 8192 per unit.
pub const ENTRIES: usize = 32768;
const TINY: f64 = 1.0 / 16_777_216.0;
const LN3: f64 = 1.098_612_288_668_109_8;

fn round(x: f64) -> f64 {
    x.round_ties_even()
}

/// The count of steps, pulses or waves of modes 3 to 6.
fn count(t: f64) -> f64 {
    let reach = 1.0 - t.abs() * 0.6;
    round(((reach * 1_000_001f64.ln()).exp() - 1.0) * 0.001)
}

/// One segment's shape, worked out from the point it ends on (and for the smooth mode its neighbours).
#[derive(Debug, Clone, Copy)]
struct Segment {
    mode: u8,
    neg: bool,
    line: bool,
    c0: f64,
    c1: f64,
    c2: f64,
}

impl Segment {
    /// The segment ending on point `i` of `pts` (place, level), with the points' tensions and modes in `map`.
    fn new(map: &Map, pts: &[(f64, f64)], i: usize) -> Segment {
        let line = Segment { mode: 0, neg: false, line: true, c0: 0.0, c1: 0.0, c2: 0.0 };
        if i == 0 || i >= pts.len() {
            return line;
        }
        let p = &map.points[i];
        let t = f64::from(p.tension);
        let side = if t < 0.0 { -1.0 } else { 1.0 };
        let mut s = Segment { mode: p.mode, neg: t < 0.0, line: false, c0: 0.0, c1: 0.0, c2: 0.0 };
        if t == 0.0 && matches!(p.mode, 0 | 1 | 7 | 8 | 9 | 11 | 12) {
            return line;
        }
        let dist = |k: usize| pts[k].0 - if k == 0 { 0.0 } else { pts[k - 1].0 };
        match p.mode {
            0 | 1 => {
                let v = (t.abs() * 1_000_001f64.ln()).exp() - 1.0;
                s.c1 = 1.0 / v;
                s.c2 = (v + 1.0).ln();
            }
            3 => {
                let n = count(t) + 1.0 + if t >= 0.0 { 1.0 } else { 0.0 };
                s.c0 = n;
                // The stairs' progress is a product of two singles.
                s.c1 = f64::from((1.0 / (n - side)) as f32);
                s.c2 = side * 0.5;
            }
            4 => {
                s.c0 = count(t) + 1.0;
                s.c1 = 1.0 / s.c0;
            }
            5 => {
                s.c0 = (count(t) + 1.0) * 2.0;
                s.c2 = side * 0.5;
            }
            6 => {
                let n = count(t);
                if n == 0.0 && t > 0.0 {
                    return line;
                }
                s.c0 = n + 0.5;
                s.c1 = std::f64::consts::TAU;
            }
            7 | 8 => s.c0 = t,
            9 => s.c0 = t.abs(),
            11 | 12 => {
                let a = 1.0 - (t.abs() * 0.9).max(1.0 / 65536.0);
                let k = 1.0 - a * a * a;
                let mut c1 = (1.0 - k) / k;
                let mut c2 = c1;
                if t > 0.0 {
                    c1 = -c1;
                    c2 = c1 - 1.0;
                }
                if p.mode == 12 {
                    c1 *= 0.5;
                    c2 *= 0.5;
                }
                (s.c0, s.c1, s.c2) = (k, c1, c2);
            }
            10 => {
                let (y0, y1) = (pts[i - 1].1, pts[i].1);
                let len = dist(i).max(TINY);
                let rise = y1 - y0;
                if rise.abs() < TINY {
                    return line;
                }
                let (mut before, mut back) = (y0, 1.0);
                if map.points[i - 1].mode == 10 {
                    back = dist(i - 1) / len;
                    if i >= 2 {
                        before = pts[i - 2].1;
                    }
                }
                let (mut after, mut ahead) = (y1, 1.0);
                if i + 1 < pts.len() && map.points[i + 1].mode == 10 {
                    ahead = dist(i + 1) / len;
                    after = pts[i + 1].1;
                }
                let back = back.max(TINY);
                let ahead = ahead.max(TINY);
                let out = ((after - y0) / rise - 1.0) / ahead;
                let into = -((before - y0) / rise) / back;
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
            _ => {}
        }
        s
    }

    fn progress(&self, x: f64) -> f64 {
        if self.line {
            return x;
        }
        let (c0, c1, c2, neg) = (self.c0, self.c1, self.c2, self.neg);
        let single = |x: f64| if neg { ((x * c2).exp() - 1.0) * c1 } else { 1.0 - (((1.0 - x) * c2).exp() - 1.0) * c1 };
        let curve2 = |a: f64| {
            let u = if neg { a } else { 1.0 - a };
            a - (u.powi(6) - u) * c0
        };
        let curve3 = |a: f64| {
            let u = if neg { a } else { 1.0 - a };
            c1 / (1.0 - c0 * u)
        };
        match self.mode {
            0 => single(x),
            1 => {
                if x < 0.5 { single(2.0 * x) / 2.0 } else { 1.0 - single(2.0 * (1.0 - x)) / 2.0 }
            }
            2 => 0.0,
            3 => f64::from((round(x * c0 - c2) as f32) * c1 as f32),
            4 => {
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
            5 => (round(x * c0 - c2) as i64 & 1) as f64,
            6 => {
                if neg {
                    (((x * c0 + 0.75) * c1).sin() + 1.0) / 2.0
                } else {
                    let a = x * c0 - 0.5;
                    ((a - round(a - 0.5)) * 2.0 - 1.0).abs()
                }
            }
            7 => curve2(x),
            8 => {
                if x < 0.5 { curve2(2.0 * x) / 2.0 } else { 1.0 - curve2(2.0 * (1.0 - x)) / 2.0 }
            }
            9 => {
                let a = x * std::f64::consts::FRAC_PI_2;
                let h = if neg { 1.0 - a.cos() } else { a.sin() };
                x - (x - h) * c0
            }
            10 => {
                let (x2, x3) = (x * x, x * x * x);
                x + ((3.0 * x2 - 2.0 * x3) + x2 * (x - 1.0) * c0 + x * (x - 1.0) * (x - 1.0) * c1 - x) * c2
            }
            11 => curve3(x) - c2,
            12 => {
                if x < 0.5 { curve3(2.0 * x) - c2 } else { (1.0 - curve3(2.0 * (1.0 - x))) + c2 }
            }
            _ => 0.0,
        }
    }
}

/// The walk along a map, stepped along the input axis.
struct Walk<'a> {
    map: &'a Map,
    pts: Vec<(f64, f64)>,
    i: usize,
    p: f64,
    r: f64,
    from: f64,
    by: f64,
    seg: Option<Segment>,
}

impl<'a> Walk<'a> {
    fn new(map: &'a Map) -> Option<Walk<'a>> {
        let pts = map.places();
        let first = *pts.first()?;
        let (p, r) = if first.0 == 0.0 { (1.0, 1.0) } else { (0.0, 1.0 / first.0) };
        Some(Walk { map, pts, i: 0, p, r, from: first.1, by: 0.0, seg: None })
    }

    fn dist(&self, k: usize) -> f64 {
        self.pts[k].0 - if k == 0 { 0.0 } else { self.pts[k - 1].0 }
    }

    fn step(&mut self, b: f64) {
        self.p += b * self.r;
        while self.p >= 1.0 {
            let left = if self.r == 0.0 { b } else { (self.p - 1.0) / self.r };
            self.from = self.pts[self.i].1;
            let moved = self.i + 1 < self.pts.len();
            if moved {
                self.i += 1;
            }
            self.by = self.pts[self.i].1 - self.from;
            self.seg = None;
            if !moved {
                (self.p, self.r) = (0.0, 0.0);
            } else if self.dist(self.i) > 0.0 {
                self.r = 1.0 / self.dist(self.i);
                self.p = left * self.r;
            } else {
                self.r = 1.0;
                self.p = left + 1.0;
            }
        }
    }

    fn value(&mut self) -> f64 {
        if self.by == 0.0 {
            return self.from;
        }
        let (map, i) = (self.map, self.i);
        let pts = &self.pts;
        let seg = *self.seg.get_or_insert_with(|| Segment::new(map, pts, i));
        seg.progress(self.p) * self.by + self.from
    }
}

/// The map's level at each of `xs` (places 0..1 in rising order), walked as the table walks it: for drawing the map.
pub fn levels(map: &Map, xs: &[f64]) -> Vec<f64> {
    let Some(mut w) = Walk::new(map) else { return vec![0.0; xs.len()] };
    let mut prev = 0.0;
    xs.iter()
        .map(|&x| {
            w.step((x - prev).max(0.0));
            prev = x.max(prev);
            w.value()
        })
        .collect()
}

/// A band's gain table and what the gain law and the saturation read from it.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// `c[i]`: the gain at level `max(i, 1) / 8192`, `ENTRIES + 2` entries.
    pub c: Vec<f32>,
    /// The last entry below the ceiling; past it the output is held at `top`.
    pub limit: usize,
    pub top: f64,
    /// The largest output, over the ceiling.
    pub reach: f64,
}

impl Table {
    /// The table of `map` with the ceiling `double(single(0.975))`, or 1 (`full`).
    pub fn new(map: &Map, full: bool) -> Table {
        let ceiling = if full { 1.0 } else { f64::from(0.975f32) };
        let half = ceiling * 0.5;
        let mut c = vec![0.0f32; ENTRIES + 2];
        let (mut limit, mut top, mut most) = (0, 0.0, f64::MIN);
        let mut walk = Walk::new(map);
        let mut prev = 0.0;
        for (i, ci) in c.iter_mut().enumerate().take(ENTRIES + 1) {
            let level = i.max(1) as f64 / 8192.0;
            let x = (level * 2.0 + 1.0).ln() * (0.5 / LN3);
            let y = match walk.as_mut() {
                Some(w) => {
                    w.step(x - prev);
                    w.value()
                }
                None => 0.0,
            };
            prev = x;
            let mut out = ((y * 2.0 * LN3).exp() - 1.0) * half;
            if (out - ceiling).abs() < TINY {
                out = ceiling;
            } else {
                limit = i;
            }
            *ci = (out / level) as f32;
            top = out;
            most = most.max(out);
        }
        c[limit + 1] = c[limit];
        Table { c, limit, top, reach: most / ceiling }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The factory map is a limiter at the ceiling: unity gain below it, the ceiling held above.
    #[test]
    fn the_default_map_limits_at_the_ceiling() {
        let t = Table::new(&Map::default_map(), false);
        let ceiling = f64::from(0.975f32);
        // Level 0.5 (-6 dB) is on the rising line: out = level * ceiling.
        assert!((f64::from(t.c[4096]) - ceiling).abs() < 1e-6, "{}", t.c[4096]);
        assert!(t.limit < 8192 && t.limit > 7000, "{}", t.limit);
        assert_eq!(t.top, ceiling);
        assert!((t.reach - 1.0).abs() < 1e-12);
        let full = Table::new(&Map::default_map(), true);
        assert!((f64::from(full.c[4096]) - 1.0).abs() < 1e-6);
    }
}
