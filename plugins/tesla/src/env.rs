// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The four envelopes drawn over the sample (the bottom bar's envelope modes): panning, volume, stereo separation and
//! all-purpose, and Tools > Mixdown amp envelopes, which applies the first three to the audio.
//!
//! Edison draws a flat line at mid height with points at the start, the middle and the end. How its envelopes map to
//! gain, pan and width, and what its mixdown does, were not measured: the laws here are **Lemon Studio's design**,
//! built on the Amp dialog's measured laws ([`crate::tools::Amp`]), so an envelope at a height does what the Amp
//! control at the same place does:
//!
//! - **Volume**: a value v (0 bottom, 1 top) is the Amp fader at v: (9^v − 1) / 2, so mid height is ×1, the top ×4
//!   (+12 dB) and the bottom silence.
//! - **Panning**: the top is left, the bottom right (p = 1 − 2v), with the Amp's circular law.
//! - **Stereo separation**: above mid widens up to double width at the top, below narrows to mono at the bottom (the
//!   Amp knob turned left / right: s = 1 − 2v).
//! - **All-purpose**: drawn and saved; the mixdown leaves it alone.
//!
//! Points keep their frame positions when the sample is edited; the envelope always spans the sample (its first
//! point is at the start, its last at the end).
//!
// spec: plugins/tesla/README.md "Envelopes in Tesla", plugins/edison/window.md "Wave area", plugins/edison/menus.md

use serde::{Deserialize, Serialize};

use crate::sample::Sample;
use crate::tools::{self, Amp, Law};

/// The envelopes, by index (the bottom bar's modes 1 to 4).
pub const PAN: usize = 0;
pub const VOLUME: usize = 1;
pub const SEP: usize = 2;
pub const ALL: usize = 3;

pub const NAMES: [&str; 4] = ["Panning", "Volume", "Stereo separation", "All-purpose"];

/// The neutral height (the default flat line).
pub const MID: f32 = 0.5;

/// One envelope point: its frame (fractional) and height, 0 (bottom) to 1 (top).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub pos: f64,
    pub v: f32,
}

/// The four envelopes. An empty list is the default flat line (it follows the sample's length).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Envs {
    pub e: [Vec<Point>; 4],
}

/// The default envelope over `n` frames: flat at mid height, points at the start, the middle and the end.
pub fn default_points(n: usize) -> Vec<Point> {
    let last = n.saturating_sub(1) as f64;
    vec![Point { pos: 0.0, v: MID }, Point { pos: (last / 2.0).round(), v: MID }, Point { pos: last, v: MID }]
}

impl Envs {
    /// Envelope `k` over a sample of `n` frames: its points, the first at 0 and the last at `n − 1`, in order.
    pub fn points(&self, k: usize, n: usize) -> Vec<Point> {
        let e = &self.e[k];
        if e.len() < 2 {
            return default_points(n);
        }
        let last = n.saturating_sub(1) as f64;
        let mut p: Vec<Point> = e.iter().map(|q| Point { pos: q.pos.clamp(0.0, last), v: q.v.clamp(0.0, 1.0) }).collect();
        p[0].pos = 0.0;
        let m = p.len() - 1;
        p[m].pos = last;
        p
    }

    /// Envelope `k` is the flat line at mid height (whatever its points).
    pub fn is_flat(&self, k: usize) -> bool {
        self.e[k].iter().all(|p| p.v == MID)
    }

    /// No envelope that the mixdown applies is edited.
    pub fn amp_flat(&self) -> bool {
        self.is_flat(PAN) && self.is_flat(VOLUME) && self.is_flat(SEP)
    }

    /// Envelope `k` as an editable list (the default points made real).
    fn make(&mut self, k: usize, n: usize) -> &mut Vec<Point> {
        let p = self.points(k, n);
        self.e[k] = p;
        &mut self.e[k]
    }

    /// Adds a point at `pos` (frames) with height `v`; returns its index. A point already at `pos` is moved instead.
    pub fn insert(&mut self, k: usize, n: usize, pos: f64, v: f32) -> usize {
        let last = n.saturating_sub(1) as f64;
        let pos = pos.clamp(0.0, last);
        let e = self.make(k, n);
        if let Some(i) = e.iter().position(|p| p.pos == pos) {
            e[i].v = v.clamp(0.0, 1.0);
            return i;
        }
        let i = e.iter().position(|p| p.pos > pos).unwrap_or(e.len());
        e.insert(i, Point { pos, v: v.clamp(0.0, 1.0) });
        i
    }

    /// Moves point `i` to `pos` and `v`: the first and last points stay at the ends; the others stay between their
    /// neighbours.
    pub fn move_point(&mut self, k: usize, n: usize, i: usize, pos: f64, v: f32) {
        let e = self.make(k, n);
        if i >= e.len() {
            return;
        }
        let m = e.len() - 1;
        if i > 0 && i < m {
            e[i].pos = pos.clamp(e[i - 1].pos, e[i + 1].pos);
        }
        e[i].v = v.clamp(0.0, 1.0);
    }

    /// Removes point `i` (the first and last stay; false for them).
    pub fn remove(&mut self, k: usize, n: usize, i: usize) -> bool {
        let e = self.make(k, n);
        if i == 0 || i + 1 >= e.len() {
            return false;
        }
        e.remove(i);
        true
    }

    /// Tools > Add points at selection / regions: a point at each position, on the line where it already is.
    pub fn add_points(&mut self, k: usize, n: usize, at: &[f64]) {
        let p = self.points(k, n);
        for &pos in at {
            let v = value(&p, pos);
            self.insert(k, n, pos, v);
        }
    }

    /// Tools > Flip vertically: every point mirrored about mid height.
    pub fn flip(&mut self, k: usize, n: usize) {
        for p in self.make(k, n) {
            p.v = 1.0 - p.v;
        }
    }

    /// Tools > Normalize levels: the points stretched about mid height until the farthest reaches the top or the
    /// bottom (Lemon Studio's design). False when the envelope is flat.
    pub fn normalize(&mut self, k: usize, n: usize) -> bool {
        let far = self.points(k, n).iter().map(|p| (p.v - MID).abs()).fold(0.0f32, f32::max);
        if far == 0.0 {
            return false;
        }
        let g = 0.5 / far;
        for p in self.make(k, n) {
            p.v = (MID + (p.v - MID) * g).clamp(0.0, 1.0);
        }
        true
    }
}

/// The envelope's height at `pos` (frames): straight lines between the points, flat beyond the ends.
pub fn value(p: &[Point], pos: f64) -> f32 {
    let Some(first) = p.first() else { return MID };
    if pos <= first.pos {
        return first.v;
    }
    for w in p.windows(2) {
        let (a, b) = (w[0], w[1]);
        if pos <= b.pos {
            if b.pos <= a.pos {
                return b.v;
            }
            let t = ((pos - a.pos) / (b.pos - a.pos)) as f32;
            return a.v + (b.v - a.v) * t;
        }
    }
    p[p.len() - 1].v
}

/// The envelope over frames `0..n`, one height per frame.
pub fn render(p: &[Point], n: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    let mut j = 0;
    for i in 0..n {
        let x = i as f64;
        while j + 1 < p.len() && p[j + 1].pos < x {
            j += 1;
        }
        let v = match (p.get(j), p.get(j + 1)) {
            (Some(a), Some(b)) if x >= a.pos && b.pos > a.pos => a.v + (b.v - a.v) * ((x - a.pos) / (b.pos - a.pos)) as f32,
            (Some(a), Some(b)) => if x < a.pos { a.v } else { b.v },
            (Some(a), None) => a.v,
            _ => MID,
        };
        out.push(v);
    }
    out
}

/// The volume envelope's gain at height `v` (the Amp fader's law).
pub fn gain(v: f32) -> f64 {
    tools::fader_gain(v.clamp(0.0, 1.0))
}

/// The panning envelope's pan at height `v`: −1 (left, the top) to 1 (right, the bottom).
pub fn pan(v: f32) -> f32 {
    1.0 - 2.0 * v.clamp(0.0, 1.0)
}

/// The stereo separation envelope's Amp knob at height `v`: −1 (double width, the top) to 1 (mono, the bottom).
pub fn sep(v: f32) -> f32 {
    1.0 - 2.0 * v.clamp(0.0, 1.0)
}

/// What envelope `k` at height `v` reads, for the hints.
pub fn text(k: usize, v: f32) -> String {
    match k {
        PAN => {
            let p = pan(v);
            let pc = (p.abs() * 100.0).round();
            if pc == 0.0 { "Centred".into() } else { format!("{pc}% {}", if p < 0.0 { "left" } else { "right" }) }
        }
        VOLUME => {
            let g = gain(v);
            if g <= 0.0 { "-inf dB".into() } else { format!("{:+.1} dB", 20.0 * g.log10()) }
        }
        SEP => format!("Width {:.0}%", tools::width(sep(v)) * 100.0),
        _ => format!("{:.0}%", v * 100.0),
    }
}

/// Tools > Mixdown amp envelopes: the panning, volume and stereo separation envelopes applied to every frame, each
/// frame through the Amp matrix of its pan, volume and separation (circular law). A mono sample takes only the
/// volume.
pub fn mixdown(s: &mut Sample, envs: &Envs) {
    let n = s.frames();
    if n == 0 {
        return;
    }
    let vol = render(&envs.points(VOLUME, n), n);
    if s.channels() == 1 {
        for (x, v) in s.ch[0].iter_mut().zip(&vol) {
            *x = (*x as f64 * gain(*v)) as f32;
        }
        return;
    }
    let pn = render(&envs.points(PAN, n), n);
    let sp = render(&envs.points(SEP, n), n);
    let (l, r) = s.ch.split_at_mut(1);
    let mut last: Option<(f32, f32, f32, [[f64; 2]; 2])> = None;
    for i in 0..n {
        let key = (pn[i], vol[i], sp[i]);
        let m = match last {
            Some((a, b, c, m)) if (a, b, c) == key => m,
            _ => {
                let m = Amp { pan: pan(key.0), law: Law::Circular, left: 0.5, volume: key.1, right: 0.5, sep: sep(key.2) }.matrix();
                last = Some((key.0, key.1, key.2, m));
                m
            }
        };
        let (lx, rx) = (l[0][i] as f64, r[0][i] as f64);
        l[0][i] = (m[0][0] * lx + m[0][1] * rx) as f32;
        r[0][i] = (m[1][0] * lx + m[1][1] * rx) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::Format;

    fn st(n: usize, l: f32, r: f32) -> Sample {
        Sample::from_channels(44100, vec![vec![l; n], vec![r; n]], Format::Float32)
    }

    #[test]
    fn default_is_flat_with_three_points() {
        let e = Envs::default();
        let p = e.points(VOLUME, 101);
        assert_eq!(p.iter().map(|p| (p.pos, p.v)).collect::<Vec<_>>(), vec![(0.0, 0.5), (50.0, 0.5), (100.0, 0.5)]);
        assert!(e.amp_flat());
        // A flat mixdown changes nothing.
        let mut s = st(100, 0.3, -0.2);
        mixdown(&mut s, &e);
        assert_eq!(s, st(100, 0.3, -0.2));
    }

    #[test]
    fn values_are_straight_lines_between_points() {
        let mut e = Envs::default();
        e.move_point(VOLUME, 11, 0, 0.0, 0.0);
        e.move_point(VOLUME, 11, 2, 10.0, 1.0);
        let p = e.points(VOLUME, 11);
        assert_eq!(value(&p, 0.0), 0.0);
        assert_eq!(value(&p, 2.5), 0.25);
        assert_eq!(value(&p, 5.0), 0.5);
        assert_eq!(value(&p, 7.5), 0.75);
        assert_eq!(value(&p, 20.0), 1.0);
        let r = render(&p, 11);
        for (i, v) in r.iter().enumerate() {
            assert!((v - value(&p, i as f64)).abs() < 1e-6, "frame {i}");
        }
    }

    #[test]
    fn points_insert_move_and_stay_in_order() {
        let mut e = Envs::default();
        let i = e.insert(PAN, 100, 20.0, 0.9);
        assert_eq!(i, 1);
        // A middle point can't pass its neighbours; the ends stay at the ends.
        e.move_point(PAN, 100, 1, 80.0, 0.1);
        assert_eq!((e.e[PAN][1].pos, e.e[PAN][1].v), (50.0, 0.1));
        e.move_point(PAN, 100, 0, 30.0, 0.7);
        assert_eq!((e.e[PAN][0].pos, e.e[PAN][0].v), (0.0, 0.7));
        assert!(!e.remove(PAN, 100, 0));
        assert!(e.remove(PAN, 100, 1));
        assert_eq!(e.e[PAN].len(), 3);
        // The sample got shorter: the last point follows its end.
        let p = e.points(PAN, 40);
        assert_eq!(p.last().unwrap().pos, 39.0);
        assert!(p.windows(2).all(|w| w[0].pos <= w[1].pos));
    }

    #[test]
    fn tools_flip_normalize_and_add_points() {
        let mut e = Envs::default();
        e.move_point(SEP, 101, 1, 50.0, 0.6);
        e.flip(SEP, 101);
        assert!((e.e[SEP][1].v - 0.4).abs() < 1e-6);
        assert!(e.normalize(SEP, 101));
        assert!(e.e[SEP][1].v.abs() < 1e-6);
        assert_eq!(e.e[SEP][0].v, 0.5);
        assert!(!Envs::default().normalize(SEP, 101));
        e.add_points(SEP, 101, &[25.0, 75.0]);
        let p = e.points(SEP, 101);
        assert_eq!(p.len(), 5);
        assert!((p[1].v - 0.25).abs() < 1e-6, "added on the line");
    }

    #[test]
    fn volume_pan_and_width_laws() {
        assert_eq!(gain(0.5), 1.0);
        assert_eq!(gain(0.0), 0.0);
        assert!((gain(1.0) - 4.0).abs() < 1e-12);
        assert_eq!(pan(1.0), -1.0);
        assert_eq!(sep(0.0), 1.0);
        assert_eq!(text(VOLUME, 0.5), "+0.0 dB");
        assert_eq!(text(PAN, 0.5), "Centred");
        assert_eq!(text(PAN, 0.0), "100% right");
        assert_eq!(text(SEP, 1.0), "Width 200%");
        assert_eq!(text(SEP, 0.0), "Width 0%");
    }

    #[test]
    fn mixdown_applies_volume_pan_and_separation() {
        // Volume: a ramp from silence at the start to ×1 at the end.
        let mut e = Envs::default();
        e.move_point(VOLUME, 11, 0, 0.0, 0.0);
        e.remove(VOLUME, 11, 1);
        let mut s = st(11, 1.0, 1.0);
        mixdown(&mut s, &e);
        assert_eq!(s.ch[0][0], 0.0);
        assert!((s.ch[1][10] - 1.0).abs() < 1e-6);
        assert!(s.ch[0].windows(2).all(|w| w[1] >= w[0]));
        // Panned fully left (the top): the circular law puts M·√2 left and nothing right.
        let mut e = Envs::default();
        for i in 0..3 {
            e.move_point(PAN, 10, i, 0.0, 1.0);
        }
        let mut s = st(10, 0.5, 0.5);
        mixdown(&mut s, &e);
        assert!((s.ch[0][5] as f64 - 0.5 * 2f64.sqrt()).abs() < 1e-6);
        assert!(s.ch[1][5].abs() < 1e-6);
        // Separation at the bottom: mono.
        let mut e = Envs::default();
        for i in 0..3 {
            e.move_point(SEP, 10, i, 0.0, 0.0);
        }
        let mut s = st(10, 1.0, -1.0);
        mixdown(&mut s, &e);
        assert!(s.ch[0][3].abs() < 1e-6 && s.ch[1][3].abs() < 1e-6);
        // Mono samples take the volume only.
        let mut e = Envs::default();
        for i in 0..3 {
            e.move_point(VOLUME, 4, i, 0.0, 1.0);
            e.move_point(PAN, 4, i, 0.0, 1.0);
        }
        let mut m = Sample::from_channels(44100, vec![vec![0.25; 4]], Format::Float32);
        mixdown(&mut m, &e);
        assert!((m.ch[0][2] - 1.0).abs() < 1e-6);
    }
}
