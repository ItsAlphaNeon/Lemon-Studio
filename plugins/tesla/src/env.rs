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
//! - **All-purpose**: drawn and saved; the mixdown leaves it alone. Tools > Scratch (envelope) and Edit > Paste mix
//!   (envelope) follow it.
//!
//! A point's **curve** shapes the line leaving it: straight, or smooth (an S-curve, flat at both ends: Tools > Turn
//! all points smooth, Alt+click on a point). While Tesla plays, the panning, volume and stereo separation envelopes are
//! heard as the mixdown would make them ([`Playback`]); the sample itself changes only with the mixdown.
//!
//! Points keep their frame positions when the sample is edited; the envelope always spans the sample (its first
//! point is at the start, its last at the end).
//!
// spec: plugins/tesla/README.md "Envelopes in Tesla", plugins/edison/window.md "Wave area", plugins/edison/menus.md,
// plugins/tesla/README.md "Downbeat, score, smooth points, envelopes heard and scratching in Tesla 1.7"

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

/// One envelope point: its frame (fractional) and height, 0 (bottom) to 1 (top), and the curve of the line leaving it
/// (smooth, or straight). Files saved before curves load with straight lines.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub pos: f64,
    pub v: f32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub smooth: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
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
    [0.0, (last / 2.0).round(), last].into_iter().map(|pos| Point { pos, v: MID, smooth: false }).collect()
}

impl Envs {
    /// Envelope `k` over a sample of `n` frames: its points, the first at 0 and the last at `n − 1`, in order.
    pub fn points(&self, k: usize, n: usize) -> Vec<Point> {
        let e = &self.e[k];
        if e.len() < 2 {
            return default_points(n);
        }
        let last = n.saturating_sub(1) as f64;
        let mut p: Vec<Point> = e.iter().map(|q| Point { pos: q.pos.clamp(0.0, last), v: q.v.clamp(0.0, 1.0), smooth: q.smooth }).collect();
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
        // A point added on a smooth line keeps the line smooth.
        let smooth = i > 0 && e[i - 1].smooth;
        e.insert(i, Point { pos, v: v.clamp(0.0, 1.0), smooth });
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

    /// Tools > Turn all points smooth: every point's line becomes an S-curve. False when they all are already.
    pub fn smooth_all(&mut self, k: usize, n: usize) -> bool {
        if self.points(k, n).iter().all(|p| p.smooth) {
            return false;
        }
        for p in self.make(k, n) {
            p.smooth = true;
        }
        true
    }

    /// Alt+click on point `i`: its line becomes smooth, or straight again. The new curve (`None` past the end).
    pub fn toggle_smooth(&mut self, k: usize, n: usize, i: usize) -> Option<bool> {
        let p = self.make(k, n).get_mut(i)?;
        p.smooth = !p.smooth;
        Some(p.smooth)
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

/// The envelope tools with a dialog (Tools > Envelope; Lemon Studio's design, the original's dialogs were not opened).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvTool {
    Scale,
    Decimate,
    Filter,
    SmoothUp,
    Sequence,
}

impl EnvTool {
    pub const ALL: [EnvTool; 5] = [EnvTool::Scale, EnvTool::Decimate, EnvTool::Filter, EnvTool::SmoothUp, EnvTool::Sequence];

    /// The undo step's and the panel's name.
    pub fn name(self) -> &'static str {
        match self {
            EnvTool::Scale => "Scale levels",
            EnvTool::Decimate => "Decimate points",
            EnvTool::Filter => "Filter",
            EnvTool::SmoothUp => "Smooth up",
            EnvTool::Sequence => "Create sequence",
        }
    }

    /// The knobs' captions (the second `None` when the tool has one knob).
    pub fn captions(self) -> (&'static str, Option<&'static str>) {
        match self {
            EnvTool::Scale => ("SCALE", Some("OFFSET")),
            EnvTool::Decimate => ("TOLERANCE", None),
            EnvTool::Filter => ("AMOUNT", None),
            EnvTool::SmoothUp => ("POINTS", Some("SMOOTH")),
            EnvTool::Sequence => ("STEPS", Some("DEPTH")),
        }
    }
}

/// An envelope tool's dialog: the tool and its two knobs, each −1 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvDlg {
    pub tool: EnvTool,
    pub a: f32,
    pub b: f32,
}

impl EnvDlg {
    /// The tool's dialog at its defaults: Scale 100 % and no offset; tolerance 5 %; ±3 points; 45 points smoothed
    /// over ±2; 8 steps at half depth.
    pub fn new(tool: EnvTool) -> EnvDlg {
        let (a, b) = match tool {
            EnvTool::Scale => (0.0, 0.0),
            EnvTool::Decimate => (-0.6, 0.0),
            EnvTool::Filter => (-0.5, 0.0),
            EnvTool::SmoothUp => (0.0, 0.0),
            EnvTool::Sequence => (-0.2, 0.0),
        };
        EnvDlg { tool, a, b }
    }

    fn u(v: f32) -> f32 {
        (v.clamp(-1.0, 1.0) + 1.0) / 2.0
    }

    /// Scale levels: the factor about mid height (0 to 2) and the offset (± half the height).
    pub fn scale(&self) -> (f32, f32) {
        (1.0 + self.a.clamp(-1.0, 1.0), self.b.clamp(-1.0, 1.0) * 0.5)
    }

    /// Decimate points: the largest height error a removed point may leave (0 to ¼ of the height).
    pub fn tolerance(&self) -> f32 {
        Self::u(self.a) * 0.25
    }

    /// Filter: the points on each side averaged (1 to 8).
    pub fn reach(&self) -> usize {
        1 + (Self::u(self.a) * 7.0).round() as usize
    }

    /// Smooth up: the points made (8 to 256) and the points on each side averaged (0 to 4).
    pub fn smooth_up(&self) -> (usize, usize) {
        ((8.0 * 2f32.powf(Self::u(self.a) * 5.0)).round() as usize, (Self::u(self.b) * 4.0).round() as usize)
    }

    /// Create sequence: the steps (2 to 64) and the depth (0 to 1 of the height).
    pub fn sequence(&self) -> (usize, f32) {
        (2f32.powf(1.0 + Self::u(self.a) * 5.0).round() as usize, Self::u(self.b))
    }

    /// The knobs' readings.
    pub fn texts(&self) -> (String, String) {
        match self.tool {
            EnvTool::Scale => {
                let (g, o) = self.scale();
                (format!("{:.0}%", g * 100.0), format!("{:+.0}%", o * 100.0))
            }
            EnvTool::Decimate => (format!("{:.1}%", self.tolerance() * 100.0), String::new()),
            EnvTool::Filter => (format!("±{} points", self.reach()), String::new()),
            EnvTool::SmoothUp => {
                let (n, r) = self.smooth_up();
                (format!("{n} points"), format!("±{r}"))
            }
            EnvTool::Sequence => {
                let (n, d) = self.sequence();
                (format!("{n} steps"), format!("{:.0}%", d * 100.0))
            }
        }
    }
}

/// The heights of `v` averaged over ±`r` neighbours (fewer at the ends).
fn average(v: &[f32], r: usize) -> Vec<f32> {
    (0..v.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(r), (i + r + 1).min(v.len()));
            v[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect()
}

/// Decimation: the points between `a` and `b` kept where one is farther than `tol` (in height) from the straight line
/// between the kept ones (Douglas-Peucker on heights).
fn keep(p: &[Point], a: usize, b: usize, tol: f32, out: &mut Vec<bool>) {
    if b <= a + 1 {
        return;
    }
    let line = |i: usize| {
        let t = if p[b].pos > p[a].pos { ((p[i].pos - p[a].pos) / (p[b].pos - p[a].pos)) as f32 } else { 0.0 };
        p[a].v + (p[b].v - p[a].v) * t
    };
    let (far, err) = (a + 1..b).map(|i| (i, (p[i].v - line(i)).abs())).fold((a, 0.0f32), |m, x| if x.1 > m.1 { x } else { m });
    if err > tol {
        out[far] = true;
        keep(p, a, far, tol, out);
        keep(p, far, b, tol, out);
    }
}

impl Envs {
    /// Runs an envelope tool's dialog on envelope `k` over `n` frames; Create sequence fills `range` (frames, first and
    /// last). False when nothing changed.
    pub fn apply_tool(&mut self, k: usize, n: usize, dlg: &EnvDlg, range: (usize, usize)) -> bool {
        let before = self.points(k, n);
        let last = n.saturating_sub(1) as f64;
        let after: Vec<Point> = match dlg.tool {
            EnvTool::Scale => {
                let (g, o) = dlg.scale();
                before.iter().map(|p| Point { v: (MID + (p.v - MID) * g + o).clamp(0.0, 1.0), ..*p }).collect()
            }
            EnvTool::Decimate => {
                let mut kept = vec![false; before.len()];
                kept[0] = true;
                kept[before.len() - 1] = true;
                keep(&before, 0, before.len() - 1, dlg.tolerance(), &mut kept);
                before.iter().zip(kept).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
            }
            EnvTool::Filter => {
                let v = average(&before.iter().map(|p| p.v).collect::<Vec<_>>(), dlg.reach());
                before.iter().zip(v).map(|(p, v)| Point { v, ..*p }).collect()
            }
            EnvTool::SmoothUp => {
                let (m, r) = dlg.smooth_up();
                let m = m.max(2);
                let pos: Vec<f64> = (0..m).map(|i| (last * i as f64 / (m - 1) as f64).round()).collect();
                let v = average(&pos.iter().map(|&x| value(&before, x)).collect::<Vec<_>>(), r);
                let mut p: Vec<Point> = pos.into_iter().zip(v).map(|(pos, v)| Point { pos, v, smooth: false }).collect();
                p.dedup_by(|b, a| b.pos == a.pos);
                p
            }
            EnvTool::Sequence => {
                let (steps, depth) = dlg.sequence();
                let (a, b) = (range.0.min(range.1) as f64, (range.1.max(range.0) as f64).min(last));
                let (hi, lo) = (MID + depth / 2.0, MID - depth / 2.0);
                let mut p: Vec<Point> = before.iter().copied().filter(|q| q.pos < a || q.pos > b).collect();
                let len = b + 1.0 - a;
                for i in 0..steps {
                    let s0 = (a + len * i as f64 / steps as f64).round();
                    let s1 = ((a + len * (i + 1) as f64 / steps as f64).round() - 1.0).max(s0);
                    let v = if i % 2 == 0 { hi } else { lo };
                    p.push(Point { pos: s0, v, smooth: false });
                    if s1 > s0 {
                        p.push(Point { pos: s1, v, smooth: false });
                    }
                }
                // Past the range the envelope goes on where it was.
                if b < last && !before.iter().any(|q| q.pos == b + 1.0) {
                    p.push(Point { pos: b + 1.0, v: value(&before, b + 1.0), smooth: false });
                }
                if a > 0.0 && !before.iter().any(|q| q.pos == a - 1.0) {
                    p.push(Point { pos: a - 1.0, v: value(&before, a - 1.0), smooth: false });
                }
                p.sort_by(|x, y| x.pos.total_cmp(&y.pos));
                p.dedup_by(|b, a| b.pos == a.pos);
                p
            }
        };
        if after == before {
            return false;
        }
        self.e[k] = after;
        true
    }
}

/// The line from `a` to `b` at `t` (0 to 1 of the way): straight, or an S-curve (smoothstep) when `a` is smooth.
fn between(a: &Point, b: &Point, t: f32) -> f32 {
    let t = if a.smooth { t * t * (3.0 - 2.0 * t) } else { t };
    a.v + (b.v - a.v) * t
}

/// The envelope's height at `pos` (frames): each point's line (straight or smooth) to the next, flat beyond the ends.
pub fn value(p: &[Point], pos: f64) -> f32 {
    let Some(first) = p.first() else { return MID };
    if pos <= first.pos {
        return first.v;
    }
    // The first point at or after `pos`.
    let j = p.partition_point(|q| q.pos < pos);
    if j >= p.len() {
        return p[p.len() - 1].v;
    }
    let (a, b) = (&p[j - 1], &p[j]);
    between(a, b, ((pos - a.pos) / (b.pos - a.pos)) as f32)
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
            (Some(a), Some(b)) if x >= a.pos && b.pos > a.pos => between(a, b, ((x - a.pos) / (b.pos - a.pos)) as f32),
            (Some(a), Some(b)) => if x < a.pos { a.v } else { b.v },
            (Some(a), None) => a.v,
            _ => MID,
        };
        out.push(v);
    }
    out
}

/// The volume envelope's gain at height `v`: 2v, from silence at the bottom through ×1 at mid height to ×2 (+6 dB)
/// at the top (the original's manual: 0% to 200%).
pub fn gain(v: f32) -> f64 {
    2.0 * f64::from(v.clamp(0.0, 1.0))
}

/// The panning envelope's pan at height `v`: −1 (left, the top) to 1 (right, the bottom).
pub fn pan(v: f32) -> f32 {
    1.0 - 2.0 * v.clamp(0.0, 1.0)
}

/// The stereo separation envelope's Amp knob at height `v`: 1 (mono, the top) to −1 (double width, the bottom), mid
/// height unchanged (the original's manual: top mono, bottom maximum separation).
pub fn sep(v: f32) -> f32 {
    2.0 * v.clamp(0.0, 1.0) - 1.0
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

/// The panning, volume and stereo separation envelopes as playback hears them (Lemon Studio's design: Edison's
/// envelopes are heard before a mixdown; the original's playback was not measured). Made off the audio thread
/// whenever the envelopes or the sample change.
#[derive(Clone, Debug, Default)]
pub struct Playback {
    /// Nothing to apply: the three are flat (or the sample is empty).
    pub flat: bool,
    mono: bool,
    pan: Vec<Point>,
    vol: Vec<Point>,
    sep: Vec<Point>,
}

impl Playback {
    pub fn new(e: &Envs, n: usize, channels: usize) -> Playback {
        if n == 0 || e.amp_flat() {
            return Playback { flat: true, ..Playback::default() };
        }
        Playback { flat: false, mono: channels < 2, pan: e.points(PAN, n), vol: e.points(VOLUME, n), sep: e.points(SEP, n) }
    }

    /// The three heights at frame `pos`.
    pub fn at(&self, pos: f64) -> (f32, f32, f32) {
        (value(&self.pan, pos), value(&self.vol, pos), value(&self.sep, pos))
    }

    /// The Amp matrix for heights `k` (as the mixdown uses them; a mono sample takes only the volume).
    pub fn matrix(&self, k: (f32, f32, f32)) -> [[f64; 2]; 2] {
        if self.mono {
            let g = gain(k.1);
            return [[g, 0.0], [0.0, g]];
        }
        Amp { pan: pan(k.0), law: Law::Circular, left: 0.5, volume: k.1, right: 0.5, sep: sep(k.2) }.matrix()
    }
}

/// Tools > Scratch (envelope): the all-purpose envelope `p` (over the whole sample) is the playing **position** within
/// frames `a..=b` (the original's manual: the vertical axis is the position, the bottom the start, the top the end).
/// Frame i of the range plays the frame at a + (b − a) · v(i), read with linear interpolation: a line rising from
/// the bottom to the top plays the range as it is, a falling one backwards, a level one holds a frame. The range keeps
/// its length. How the original reads between frames was not measured: Lemon Studio's design.
pub fn scratch(s: &mut Sample, a: usize, b: usize, p: &[Point]) {
    let n = s.frames();
    if n == 0 || a > b {
        return;
    }
    let b = b.min(n - 1);
    let src = s.clone();
    let span = (b - a) as f64;
    for i in a..=b {
        let x = a as f64 + span * f64::from(value(p, i as f64).clamp(0.0, 1.0));
        let (j, t) = (x.floor() as usize, (x - x.floor()) as f32);
        for (c, out) in s.ch.iter_mut().enumerate() {
            let (u, w) = (src.at(c, j), src.at(c, (j + 1).min(b)));
            out[i] = u + (w - u) * t;
        }
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
    fn smooth_points_old_files_playback_and_scratch() {
        // A smooth point's line is an S-curve: the same ends and middle, flat at the ends.
        let mut e = Envs::default();
        e.move_point(VOLUME, 11, 0, 0.0, 0.0);
        e.move_point(VOLUME, 11, 2, 10.0, 1.0);
        e.remove(VOLUME, 11, 1);
        assert!(e.smooth_all(VOLUME, 11));
        assert!(!e.smooth_all(VOLUME, 11), "already smooth");
        let p = e.points(VOLUME, 11);
        assert_eq!((value(&p, 0.0), value(&p, 5.0), value(&p, 10.0)), (0.0, 0.5, 1.0));
        assert!((value(&p, 1.0) - 0.028).abs() < 1e-3 && (value(&p, 9.0) - 0.972).abs() < 1e-3);
        assert_eq!(render(&p, 11)[1], value(&p, 1.0));
        assert_eq!(e.toggle_smooth(VOLUME, 11, 0), Some(false));
        assert_eq!(value(&e.points(VOLUME, 11), 1.0), 0.1);
        // Saved points: straight ones as before; files without curves load straight.
        let json = serde_json::to_string(&e).unwrap();
        assert!(json.contains("\"smooth\":true") && json.matches("smooth").count() == 1, "{json}");
        let old: Envs = serde_json::from_str(r#"{"e":[[],[{"pos":0.0,"v":0.2},{"pos":10.0,"v":0.8}],[],[]]}"#).unwrap();
        assert!(old.e[VOLUME].iter().all(|p| !p.smooth));
        // Playback: flat envelopes do nothing; otherwise each frame hears what the mixdown makes of it.
        assert!(Playback::new(&Envs::default(), 100, 2).flat);
        let mut v = Envs::default();
        v.move_point(VOLUME, 101, 2, 100.0, 1.0);
        v.move_point(PAN, 101, 0, 0.0, 0.8);
        let pb = Playback::new(&v, 101, 2);
        let mut s = st(101, 0.25, -0.1);
        mixdown(&mut s, &v);
        for i in [0usize, 30, 100] {
            let m = pb.matrix(pb.at(i as f64));
            let l = m[0][0] * 0.25 + m[0][1] * -0.1;
            let r = m[1][0] * 0.25 + m[1][1] * -0.1;
            assert!((s.ch[0][i] as f64 - l).abs() < 1e-6 && (s.ch[1][i] as f64 - r).abs() < 1e-6, "{i}");
        }
        assert!(s.ch[0][100] > 0.5, "louder at the top");
        // Scratch: the height is the position. A line from the bottom to the top plays the range as it is.
        let ramp = Sample::from_channels(44100, vec![(0..100).map(|i| i as f32).collect()], Format::Float32);
        let line = |v0: f32, v1: f32| {
            let mut e = Envs::default();
            for i in 0..3 {
                let x = [0.0, 50.0, 99.0][i];
                e.move_point(ALL, 100, i, x, v0 + (v1 - v0) * (x / 99.0) as f32);
            }
            e.points(ALL, 100)
        };
        let mut r = ramp.clone();
        scratch(&mut r, 0, 99, &line(0.0, 1.0));
        assert!(r.ch[0].iter().zip(&ramp.ch[0]).all(|(a, b)| (a - b).abs() < 1e-3));
        // Falling: backwards.
        let mut back = ramp.clone();
        scratch(&mut back, 0, 99, &line(1.0, 0.0));
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(near(back.ch[0][0], 99.0) && near(back.ch[0][99], 0.0) && near(back.ch[0][40], 59.0));
        // Level a quarter of the way up: holds the frame a quarter into the range; outside the range nothing moves.
        let mut h = ramp.clone();
        scratch(&mut h, 10, 50, &line(0.25, 0.25));
        assert!(h.ch[0][10..=50].iter().all(|&x| x == 20.0) && h.ch[0][9] == 9.0 && h.ch[0][51] == 51.0);
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
    fn envelope_tools_with_a_dialog() {
        let n = 1001;
        let mut e = Envs::default();
        e.move_point(VOLUME, n, 1, 500.0, 0.9);
        // Scale levels: x50 % about mid height, then +10 % up.
        let dlg = EnvDlg { tool: EnvTool::Scale, a: -0.5, b: 0.2 };
        assert_eq!(dlg.texts(), ("50%".to_string(), "+10%".to_string()));
        let mut s = e.clone();
        assert!(s.apply_tool(VOLUME, n, &dlg, (0, n - 1)));
        assert!((s.e[VOLUME][1].v - 0.8).abs() < 1e-6 && (s.e[VOLUME][0].v - 0.6).abs() < 1e-6);
        assert!(!s.clone().apply_tool(VOLUME, n, &EnvDlg::new(EnvTool::Scale), (0, n - 1)), "the defaults change nothing");
        // Smooth up: evenly spaced points on the old line, averaged.
        let mut u = e.clone();
        assert!(u.apply_tool(VOLUME, n, &EnvDlg { tool: EnvTool::SmoothUp, a: -1.0, b: -1.0 }, (0, n - 1)));
        assert_eq!(u.e[VOLUME].len(), 8);
        assert!(u.e[VOLUME].iter().all(|p| (p.v - value(&e.points(VOLUME, n), p.pos)).abs() < 1e-6), "no smoothing: on the line");
        let mut v = e.clone();
        assert!(v.apply_tool(VOLUME, n, &EnvDlg::new(EnvTool::SmoothUp), (0, n - 1)));
        assert_eq!(v.e[VOLUME].len(), 45);
        let peak = v.e[VOLUME].iter().map(|p| p.v).fold(0f32, f32::max);
        assert!(peak < 0.9 && peak > 0.8, "the corner rounded off: {peak}");
        // Decimate points: the evenly spaced points on straight lines go back to the corner.
        assert!(u.apply_tool(VOLUME, n, &EnvDlg::new(EnvTool::Decimate), (0, n - 1)));
        assert!(u.e[VOLUME].len() <= 4, "{:?}", u.e[VOLUME]);
        // Filter: the corner averaged with its neighbours.
        let mut f = e.clone();
        assert!(f.apply_tool(VOLUME, n, &EnvDlg { tool: EnvTool::Filter, a: -1.0, b: 0.0 }, (0, n - 1)));
        assert!((f.e[VOLUME][1].v - (0.5 + 0.9 + 0.5) / 3.0).abs() < 1e-6);
        // Create sequence: 4 steps over frames 200 to 599, alternately up and down, the rest as it was.
        let mut q = Envs::default();
        let dlg = EnvDlg { tool: EnvTool::Sequence, a: -0.6, b: 1.0 };
        assert_eq!(dlg.sequence(), (4, 1.0));
        assert!(q.apply_tool(PAN, n, &dlg, (200, 599)));
        let p = q.points(PAN, n);
        let at = |x: f64| value(&p, x);
        assert_eq!((at(200.0), at(299.0), at(300.0), at(399.0), at(400.0), at(599.0)), (1.0, 1.0, 0.0, 0.0, 1.0, 0.0));
        assert_eq!((at(100.0), at(199.0), at(600.0), at(1000.0)), (0.5, 0.5, 0.5, 0.5));
    }

    #[test]
    fn volume_pan_and_width_laws() {
        assert_eq!(gain(0.5), 1.0);
        assert_eq!(gain(0.0), 0.0);
        assert_eq!(gain(1.0), 2.0);
        assert_eq!(pan(1.0), -1.0);
        assert_eq!((sep(1.0), sep(0.5), sep(0.0)), (1.0, 0.0, -1.0));
        assert_eq!(text(VOLUME, 0.5), "+0.0 dB");
        assert_eq!(text(VOLUME, 1.0), "+6.0 dB");
        assert_eq!(text(PAN, 0.5), "Centred");
        assert_eq!(text(PAN, 0.0), "100% right");
        assert_eq!(text(SEP, 0.0), "Width 200%");
        assert_eq!(text(SEP, 1.0), "Width 0%");
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
        // Separation at the top: mono.
        let mut e = Envs::default();
        for i in 0..3 {
            e.move_point(SEP, 10, i, 0.0, 1.0);
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
        assert!((m.ch[0][2] - 0.5).abs() < 1e-6);
    }
}
