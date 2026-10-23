// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Scale levels tool (Tools ▸ Scale levels…, Alt+X): its settings and the scaling of the Control
//! lane's note property, free of any window. The dialog's session is `piano_dlg.rs`, the window `views/piano_dlg.rs`.
// spec: ui/piano-level-scaling.md

use os_model::Note;

use crate::piano::Lane;

/// The tension curve's base (§3): `f(u) = (1 − B^(−t·u)) / (1 − B^(−t))`.
const B: f64 = 1000.0;

/// The dialog's controls as fractions (§1, 100 % = 1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// 0..1.
    pub center: f32,
    /// −2..2.
    pub mult: f32,
    /// −1..1.
    pub offset: f32,
    /// −1..1.
    pub tension: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings::reset(Lane::Velocity)
    }
}

impl Settings {
    /// After Reset with `lane` shown (§1).
    pub fn reset(lane: Lane) -> Self {
        Settings { center: reset_center(lane), mult: 1.0, offset: 0.0, tension: 0.0 }
    }
}

/// Center after Reset: the middle on the bipolar note pan, filter cutoff and resonance lanes, else the bottom (§1).
pub fn reset_center(lane: Lane) -> f32 {
    if matches!(lane, Lane::Pan | Lane::Cutoff | Lane::Resonance) { 0.5 } else { 0.0 }
}

/// The tension curve (§3 step 1), 0..1 for `u` 0..1.
fn bend(u: f64, t: f64) -> f64 {
    if t == 0.0 { u } else { (1.0 - B.powf(-t * u)) / (1.0 - B.powf(-t)) }
}

/// A value `x` (a fraction of the property's full scale) through tension, Multiply and Offset (§3 steps 1-2).
pub fn level(x: f64, set: &Settings) -> f64 {
    let (c, t) = (f64::from(set.center), f64::from(set.tension));
    let bent = if x > c && c < 1.0 {
        c + (1.0 - c) * bend((x - c) / (1.0 - c), t)
    } else if x < c {
        c - c * bend((c - x) / c, t)
    } else {
        x
    };
    c + (bent - c) * f64::from(set.mult) + f64::from(set.offset)
}

/// Scale `lane`'s property of `notes` (the targets) (§2, §3); a channel lane leaves them as they are.
pub fn scale(notes: Vec<Note>, lane: Lane, set: &Settings) -> Vec<Note> {
    if lane.is_channel() {
        return notes;
    }
    let (full, max) = (f64::from(lane.scale()), f64::from(lane.max()));
    notes
        .into_iter()
        .map(|mut n| {
            let y = level(f64::from(lane.get(&n)) / full, set) * full;
            lane.set(&mut n, (y + 0.5).floor().clamp(0.0, max) as u8);
            n
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VELS: [u8; 10] = [0, 16, 32, 48, 64, 80, 96, 100, 112, 128];

    fn out(lane: Lane, ins: &[u8], set: Settings) -> Vec<u8> {
        let notes: Vec<Note> = ins
            .iter()
            .map(|&v| {
                let mut n = Note::step(0, 0);
                lane.set(&mut n, v);
                n
            })
            .collect();
        scale(notes, lane, &set).iter().map(|n| lane.get(n)).collect()
    }

    fn vel(f: impl FnOnce(&mut Settings)) -> Vec<u8> {
        let mut s = Settings::default();
        f(&mut s);
        out(Lane::Velocity, &VELS, s)
    }

    fn vel17(f: impl FnOnce(&mut Settings)) -> Vec<u8> {
        let mut s = Settings::default();
        f(&mut s);
        out(Lane::Velocity, &(0..=16).map(|i| i * 8).collect::<Vec<u8>>(), s)
    }

    /// FL's wheel notches: Center and Offset 3.125 %, Multiply 6.25 %, Tension 3.125 %.
    fn n(k: i32) -> f32 {
        k as f32 / 32.0
    }

    #[test]
    fn multiply_offset_and_center_match_fl() {
        assert_eq!(vel(|_| {}), VELS);
        assert_eq!(vel(|s| s.mult = 1.0 - 2.0 * n(5)), [0, 11, 22, 33, 44, 55, 66, 69, 77, 88]);
        assert_eq!(vel(|s| s.mult = 1.0 - 2.0 * n(10)), [0, 6, 12, 18, 24, 30, 36, 38, 42, 48]);
        assert_eq!(vel(|s| s.mult = 1.0 + 2.0 * n(5)), [0, 21, 42, 63, 84, 105, 126, 128, 128, 128]);
        assert_eq!(vel(|s| s.mult = 0.0), [0; 10]);
        assert_eq!(vel(|s| s.mult = -1.0), [0; 10]);
        assert_eq!(vel(|s| s.offset = n(5)), [20, 36, 52, 68, 84, 100, 116, 120, 128, 128]);
        assert_eq!(vel(|s| s.offset = -n(5)), [0, 0, 12, 28, 44, 60, 76, 80, 92, 108]);
        assert_eq!(vel(|s| (s.center, s.mult) = (n(5), 1.0 - 2.0 * n(5))), [6, 17, 28, 39, 50, 61, 72, 75, 83, 94]);
        assert_eq!(vel(|s| (s.center, s.mult) = (n(16), 1.0 - 2.0 * n(5))), [20, 31, 42, 53, 64, 75, 86, 89, 97, 108]);
        assert_eq!(vel(|s| (s.center, s.mult) = (1.0, 1.0 - 2.0 * n(5))), [40, 51, 62, 73, 84, 95, 106, 109, 117, 128]);
        assert_eq!(vel(|s| (s.center, s.mult) = (n(16), 1.0 + 2.0 * n(5))), [0, 1, 22, 43, 64, 85, 106, 111, 127, 128]);
    }

    #[test]
    fn tension_matches_fl() {
        assert_eq!(vel(|s| s.tension = n(5)), [0, 24, 46, 65, 81, 95, 108, 110, 118, 128]);
        assert_eq!(vel17(|s| s.tension = n(1)), [0, 9, 18, 26, 35, 43, 51, 59, 67, 75, 83, 91, 99, 106, 113, 121, 128]);
        assert_eq!(vel17(|s| s.tension = 0.5), [0, 26, 46, 63, 76, 87, 96, 103, 109, 113, 117, 120, 122, 124, 126, 127, 128]);
        assert_eq!(vel17(|s| s.tension = 1.0), [0, 45, 74, 93, 105, 113, 119, 122, 124, 125, 126, 127, 127, 128, 128, 128, 128]);
        assert_eq!(vel17(|s| s.tension = -0.5), [0, 1, 2, 4, 6, 8, 11, 15, 19, 25, 32, 41, 52, 65, 82, 102, 128]);
        assert_eq!(vel17(|s| s.tension = -1.0), [0, 0, 0, 0, 1, 1, 2, 3, 4, 6, 9, 15, 23, 35, 54, 83, 128]);
        assert_eq!(vel17(|s| (s.tension, s.center, s.mult) = (0.5, 0.5, 1.0 - 2.0 * n(5))), [20, 21, 22, 24, 27, 31, 38, 48, 64, 80, 90, 97, 101, 104, 106, 107, 108]);
        assert_eq!(vel17(|s| (s.tension, s.offset) = (0.5, n(5)))[..9], [20, 46, 66, 83, 96, 107, 116, 123, 128]);
    }

    #[test]
    fn pan_lane_and_other_scales() {
        let pan = Settings::reset(Lane::Pan);
        assert_eq!(pan.center, 0.5);
        assert_eq!(out(Lane::Pan, &VELS, Settings { mult: 1.0 - 2.0 * n(5), ..pan }), [20, 31, 42, 53, 64, 75, 86, 89, 97, 108]);
        assert_eq!(out(Lane::Pan, &VELS, Settings { center: 1.0, mult: 1.0 - 2.0 * n(5), ..pan }), [40, 51, 62, 73, 84, 95, 106, 109, 117, 128]);
        // Each property on its own full scale (§4): cutoff 0..255 of 256, fine pitch of 240, repeat of 14.
        assert_eq!(out(Lane::Cutoff, &[0, 128, 255], Settings { mult: 2.0, ..Settings::reset(Lane::Cutoff) }), [0, 128, 255]);
        assert_eq!(out(Lane::FinePitch, &[0, 120, 240], Settings { mult: 0.5, ..Settings::reset(Lane::FinePitch) }), [0, 60, 120]);
        assert_eq!(out(Lane::Repeat, &[0, 7, 14], Settings { offset: 0.5, ..Settings::reset(Lane::Repeat) }), [7, 14, 14]);
        assert_eq!(Settings::reset(Lane::Release).center, 0.0);
        let chan = vec![Note::step(0, 0)];
        assert_eq!(scale(chan.clone(), Lane::ChanVolume, &Settings { mult: 0.0, ..Settings::default() }), chan);
    }
}
