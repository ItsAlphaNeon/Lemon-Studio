// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's LFO tool (Tools ▸ LFO…, Alt+O): its settings and the wave it writes into the Control lane, free of
//! any window. The dialog's session is `piano_dlg.rs`, the window `views/piano_dlg.rs`.
// spec: ui/piano-lfo.md

use os_model::Note;

use crate::piano::Lane;

/// Wheel notches on the knobs' 0..1 positions (§1): Value 1/64 of 0..100 %, Range 1/64 of −100..+100 %, Speed an
/// eighth of an octave over ±5 octaves, Phase 1/64 of a cycle over ±1 cycle.
pub const VALUE_NOTCH: f32 = 1.0 / 64.0;
pub const RANGE_NOTCH: f32 = 1.0 / 64.0;
pub const SPEED_NOTCH: f32 = 1.0 / 80.0;
pub const PHASE_NOTCH: f32 = 1.0 / 128.0;
/// Speed spans ±5 octaves around a one-bar period.
const OCTAVES: f64 = 5.0;

/// Shape ▸ (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Shape {
    #[default]
    Sine,
    Triangle,
    Square,
}

impl Shape {
    pub const ALL: [Shape; 3] = [Shape::Sine, Shape::Triangle, Shape::Square];

    pub fn name(self) -> &'static str {
        match self {
            Shape::Sine => "Sine",
            Shape::Triangle => "Triangle",
            Shape::Square => "Square",
        }
    }

    /// The wave at `f` cycles into it (0..1): 0 at the start, +1 a quarter in (§3).
    pub fn at(self, f: f64) -> f64 {
        match self {
            Shape::Sine => (std::f64::consts::TAU * f).sin(),
            Shape::Triangle if f < 0.25 => 4.0 * f,
            Shape::Triangle if f < 0.75 => 2.0 - 4.0 * f,
            Shape::Triangle => 4.0 * f - 4.0,
            Shape::Square if f < 0.5 => 1.0,
            Shape::Square => -1.0,
        }
    }
}

/// One column of knobs, Start or End (§1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    /// The wave's centre, 0..1 of the property's full scale.
    pub value: f32,
    /// Its amplitude, −1..1 (negative turns the wave over).
    pub range: f32,
    /// Octaves faster than a one-bar period, −5..5.
    pub speed: f32,
}

impl Default for Knobs {
    fn default() -> Self {
        Knobs { value: 0.5, range: 0.25, speed: 0.0 }
    }
}

impl Knobs {
    /// One period in ticks at `ppq`: a bar at speed 0, halved per octave up.
    pub fn period(&self, ppq: u32) -> f64 {
        4.0 * f64::from(ppq.max(1)) * 2f64.powf(-f64::from(self.speed))
    }

    pub fn value_text(&self) -> String {
        format!("{}%", pct(self.value))
    }

    pub fn range_text(&self) -> String {
        format!("{}%", pct(self.range))
    }

    /// The period as steps:ticks, FL's hint; the ticks are the whole part of `P + 1/4` (§1, §7).
    pub fn speed_text(&self, ppq: u32) -> String {
        let step = (ppq.max(4) / 4).max(1);
        let t = (self.period(ppq) + 0.25).floor() as u32;
        format!("{}:{:02}", t / step, t % step)
    }
}

/// Percent with halves to even, as FL's hints show them.
fn pct(v: f32) -> i64 {
    (f64::from(v) * 100.0).round_ties_even() as i64
}

/// The dialog's controls (§1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub start: Knobs,
    pub end: Knobs,
    /// End's switch: off, the Start column holds over the whole pattern.
    pub end_on: bool,
    /// Speed's "log" switch: End's period is reached along an exponential curve instead of a straight line.
    pub log: bool,
    pub shape: Shape,
    /// Cycles, −1..1; only the fractional part counts.
    pub phase: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { start: Knobs::default(), end: Knobs::default(), end_on: false, log: false, shape: Shape::Sine, phase: 0.0 }
    }
}

impl Settings {
    /// The End switch: switched on, End's knobs take Start's values (§1).
    pub fn toggle_end(&mut self) {
        self.end_on = !self.end_on;
        if self.end_on {
            self.end = self.start;
        }
    }

    /// Degrees (a Lemon Studio choice: FL's hint bar shows nothing, §7).
    pub fn phase_text(&self) -> String {
        format!("{}°", (f64::from(self.phase) * 360.0).round() as i64)
    }
}

/// Knob positions (0..1) of the Range, Speed and Phase values, and back.
pub fn range_pos(r: f32) -> f32 {
    (r + 1.0) / 2.0
}

pub fn speed_pos(sp: f32) -> f32 {
    ((f64::from(sp) + OCTAVES) / (2.0 * OCTAVES)) as f32
}

pub fn phase_pos(ph: f32) -> f32 {
    (ph + 1.0) / 2.0
}

pub fn range_of(pos: f32) -> f32 {
    pos * 2.0 - 1.0
}

/// Speed in octaves, in 1/64ths of a wheel notch: whole notches land on exact eighths of an octave, as FL's do (the
/// phase sum shows the difference on a square's edges, §3).
pub fn speed_of(pos: f32) -> f32 {
    let notches = (f64::from(pos) * 2.0 * OCTAVES * 8.0 * 64.0).round() / 64.0;
    (notches / 8.0 - OCTAVES) as f32
}

pub fn phase_of(pos: f32) -> f32 {
    pos * 2.0 - 1.0
}

/// The wave before the lane's scale, `v + r × w(φ)`, tick by tick from the time origin; End's values are reached `len`
/// ticks in (§3). The phase is a running sum of `1 / P` in double precision, as FL's.
fn wave(set: &Settings, ppq: u32, len: u32) -> impl Iterator<Item = f64> {
    let set = *set;
    let len = f64::from(len.max(1));
    let (a, b) = (set.start, if set.end_on { set.end } else { set.start });
    let (p0, p1) = (a.period(ppq), b.period(ppq));
    let mut ph = f64::from(set.phase).rem_euclid(1.0);
    (0u32..).map(move |t| {
        // Past End (a note beyond the pattern's set length) the End values hold.
        let u = if set.end_on { (f64::from(t) / len).min(1.0) } else { 0.0 };
        let lerp = |x: f32, y: f32| f64::from(x) + (f64::from(y) - f64::from(x)) * u;
        let y = lerp(a.value, b.value) + lerp(a.range, b.range) * set.shape.at(ph);
        let p = if set.log { p0 * (p1 / p0).powf(u) } else { p0 + (p1 - p0) * u };
        ph += 1.0 / p;
        if ph >= 1.0 {
            ph -= 1.0;
        }
        y
    })
}

/// Write the LFO into `lane`'s property of `notes` (§2, §3): each note takes the wave at its start, counted from
/// `origin`; `len` is where End's values are reached. Halves round to the even number (§7). A channel lane leaves the
/// notes as they are.
pub fn lfo(notes: Vec<Note>, lane: Lane, set: &Settings, ppq: u32, origin: u32, len: u32) -> Vec<Note> {
    if lane.is_channel() || notes.is_empty() {
        return notes;
    }
    let last = notes.iter().map(|n| n.position.saturating_sub(origin)).max().unwrap_or(0);
    let ys: Vec<f64> = wave(set, ppq, len).take(last as usize + 1).collect();
    let (full, max) = (f64::from(lane.scale()), f64::from(lane.max()));
    notes
        .into_iter()
        .map(|mut n| {
            let y = ys[n.position.saturating_sub(origin) as usize] * full;
            lane.set(&mut n, y.round_ties_even().clamp(0.0, max) as u8);
            n
        })
        .collect()
}

/// The channel lane's control points (§5): a point on every tick from 0 to `len − 1` (the pattern's length), left out
/// where the value is the tick before's. Volume and pan 0..12800, pitch in cents, ±4800.
pub fn points(set: &Settings, lane: Lane, ppq: u32, len: u32) -> Vec<(u32, i32)> {
    let mut out: Vec<(u32, i32)> = Vec::new();
    for (t, y) in (0..len).zip(wave(set, ppq, len)) {
        let y = y.clamp(0.0, 1.0);
        let v = if lane == Lane::ChanPitch { (y - 0.5) * 9600.0 } else { y * 12800.0 }.round_ties_even() as i32;
        if out.last().map(|p| p.1) != Some(v) {
            out.push((t, v));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, key: u16) -> Note {
        Note { length: 24, key, velocity: 100, ..Note::step(0, pos) }
    }

    /// RUN (§6): 64 one-step notes at PPQ 96, a pattern of 1536 ticks.
    fn run() -> Vec<Note> {
        (0..64).map(|i| note(i * 24, 60)).collect()
    }

    fn vels(notes: &[Note], n: usize) -> Vec<u8> {
        notes.iter().take(n).map(|n| n.velocity).collect()
    }

    fn velocities(set: &Settings) -> Vec<u8> {
        vels(&lfo(run(), Lane::Velocity, set, 96, 0, 1536), 24)
    }

    fn knobs(value: i32, range: i32, speed: i32) -> Knobs {
        // The knobs stop at their ends, as `piano_dlg::Ui::set` does.
        let (d, at) = (Knobs::default(), |pos: f32, n: i32, notch: f32| (pos + n as f32 * notch).clamp(0.0, 1.0));
        Knobs { value: at(d.value, value, VALUE_NOTCH), range: range_of(at(range_pos(d.range), range, RANGE_NOTCH)), speed: speed_of(at(speed_pos(d.speed), speed, SPEED_NOTCH)) }
    }

    #[test]
    fn measured_velocities() {
        let d = Settings::default();
        let of = |s: &str| s.split(' ').map(|v| v.parse().unwrap()).collect::<Vec<u8>>();
        assert_eq!(velocities(&d), of("64 76 87 94 96 94 87 76 64 52 41 34 32 34 41 52 64 76 87 94 96 94 87 76"));
        let tri = Settings { shape: Shape::Triangle, ..d };
        assert_eq!(velocities(&tri), of("64 72 80 88 96 88 80 72 64 56 48 40 32 40 48 56 64 72 80 88 96 88 80 72"));
        let sq = velocities(&Settings { shape: Shape::Square, ..d });
        assert_eq!(sq[..17], [[96; 8].as_slice(), &[32; 9]].concat(), "the note at 384 stays low: the phase sum");
        assert_eq!(velocities(&Settings { start: knobs(10, 0, 0), ..d }), of("84 96 107 114 116 114 107 96 84 72 61 54 52 54 61 72 84 96 107 114 116 114 107 96"));
        assert_eq!(velocities(&Settings { start: knobs(0, -16, 0), ..d }), of("64 52 41 34 32 34 41 52 64 76 87 94 96 94 87 76 64 52 41 34 32 34 41 52"));
        assert_eq!(velocities(&Settings { start: knobs(0, 0, 8), ..d })[..8], of("64 87 96 87 64 41 32 41"));
        assert_eq!(velocities(&Settings { start: knobs(0, 0, -3), ..d }), of("64 74 82 89 94 96 95 91 85 77 68 58 49 41 36 32 32 35 40 48 57 66 76 84"));
        assert_eq!(velocities(&Settings { phase: phase_of(phase_pos(0.0) + 16.0 * PHASE_NOTCH), ..d })[..9], of("96 94 87 76 64 52 41 34 32"));
        let end = |e: Knobs, log: bool| Settings { end: e, end_on: true, log, ..d };
        // The note at 192 is an exact 61.5: FL gave 62, halves to even give 61 (§3's open question).
        assert_eq!(velocities(&end(knobs(-10, 0, 0), false)), of("64 76 86 93 95 92 85 74 61 49 38 31 28 30 37 47 59 71 81 88 90 87 80 69"));
        assert_eq!(velocities(&end(knobs(0, 0, 8), false)), of("64 76 87 94 96 93 85 74 61 48 38 33 33 39 50 63 77 89 95 95 89 77 62 48"));
        assert_eq!(velocities(&end(knobs(0, 0, 8), true)), of("64 76 87 94 96 93 85 73 60 47 37 32 34 41 53 68 81 92 96 93 84 69 54 40"));
        // End's knobs turned from Start's values, which switching End on copied.
        let ramp = Settings { start: knobs(32, -8, 0), end: knobs(32 - 32, -8 - 8, 0), end_on: true, ..d };
        assert_eq!(velocities(&ramp), of("128 127 125 124 122 121 120 120 120 121 122 122 122 121 119 116 112 108 104 100 98 97 98 101"));
        let sq16 = velocities(&Settings { shape: Shape::Square, start: knobs(0, 0, 16), ..d });
        assert_eq!(sq16[..12], of("96 96 32 32 32 96 96 32 32 96 96 32"), "the note at 96 low: the phase sum");
    }

    #[test]
    fn other_notes_and_lanes() {
        let d = Settings::default();
        let at = |ps: &[u32]| ps.iter().map(|&p| note(p, 60)).collect::<Vec<_>>();
        // OFFS and SPARSE (two-bar patterns): the time counts from the pattern's start (§3).
        assert_eq!(vels(&lfo(at(&[96, 120, 144, 168, 192]), Lane::Velocity, &d, 96, 0, 768), 5), [96, 94, 87, 76, 64]);
        assert_eq!(vels(&lfo(at(&[0, 10, 50, 100, 101, 250, 383, 700]), Lane::Velocity, &d, 96, 0, 768), 8), [64, 69, 87, 96, 96, 38, 63, 35]);
        let first9 = |lane: Lane, set: &Settings| lfo(run(), lane, set, 96, 0, 1536).iter().take(9).map(|n| lane.get(n)).collect::<Vec<_>>();
        assert_eq!(first9(Lane::Pan, &d), [64, 76, 87, 94, 96, 94, 87, 76, 64]);
        assert_eq!(first9(Lane::FinePitch, &d), [120, 143, 162, 175, 180, 175, 162, 143, 120]);
        assert_eq!(first9(Lane::Cutoff, &d), [128, 152, 173, 187, 192, 187, 173, 152, 128]);
        assert_eq!(first9(Lane::Repeat, &d), [7, 8, 9, 10, 10, 10, 9, 8, 7]);
        assert_eq!(first9(Lane::Release, &Settings { start: knobs(0, 8, 0), ..d }), [64, 88, 109, 123, 128, 123, 109, 88, 64]);
        // A selection (§2): the time from its first note, End at its span less one tick.
        let sel: Vec<Note> = (0..8).map(|i| note(480 + 24 * i, 72)).collect();
        assert_eq!(lfo(sel, Lane::Velocity, &d, 96, 480, 191)[0].velocity, 64, "the start of the wave");
    }

    #[test]
    fn control_points() {
        let d = Settings::default();
        let vol = points(&d, Lane::ChanVolume, 96, 1536);
        assert_eq!(vol.len(), 1520, "repeats left out");
        assert_eq!(vol[..6].iter().map(|p| p.1).collect::<Vec<_>>(), [6400, 6452, 6505, 6557, 6609, 6662]);
        let pitch = points(&d, Lane::ChanPitch, 96, 1536);
        assert_eq!(pitch[..6].iter().map(|p| p.1).collect::<Vec<_>>(), [0, 39, 79, 118, 157, 196]);
        let sq = points(&Settings { shape: Shape::Square, ..d }, Lane::ChanPan, 96, 1536);
        assert_eq!(sq.iter().map(|p| p.0).take(4).collect::<Vec<_>>(), [0, 192, 385, 577], "the phase sum moves the edges");
        assert_eq!(sq.len(), 8);
    }

    #[test]
    fn hints() {
        let speed = |n: i32| knobs(0, 0, n).speed_text(96);
        let got: Vec<String> = [1, 3, 5, 8, 10, 12, 15, 16, 20, 24, 28, 34, 40, 41, -1, -2, -6, -8, -9, -14, -18, -19, -24, -36, -40, -41].map(speed).to_vec();
        let want = "14:16 12:08 10:09 8:00 6:17 5:16 4:08 4:00 2:20 2:00 1:10 0:20 0:12 0:12 17:11 19:00 26:22 32:00 34:21 53:19 76:02 83:00 128:00 362:01 512:00 512:00";
        assert_eq!(got.join(" "), want);
        assert_eq!(knobs(1, 0, 0).value_text(), "52%");
        assert_eq!(knobs(30, 0, 0).value_text(), "97%");
        assert_eq!(knobs(40, 0, 0).value_text(), "100%", "it stops at the end");
        assert_eq!([4, -4, -9, -10].map(|n| knobs(0, n, 0).range_text()), ["38%", "12%", "-3%", "-6%"]);
        let mut set = Settings { start: knobs(3, 2, 1), ..Settings::default() };
        set.toggle_end();
        assert_eq!(set.end, set.start, "End takes Start's values");
        set.phase = phase_of(phase_pos(0.0) - 16.0 * PHASE_NOTCH);
        assert_eq!(set.phase_text(), "-90°");
    }
}
