// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Flam tool (Tools ▸ Flam…, Alt+F): its settings and the flam itself, free of any window. The
//! dialog's session is `piano_dlg.rs`, the window `views/piano_dlg.rs`.
// spec: ui/piano-flam.md

use os_model::Note;

/// The Time knob's travel: 24 wheel notches end to end (§1).
pub const TIME_NOTCH: f32 = 1.0 / 24.0;
/// The Velocity knob: 0..128, 4 a notch (§1).
pub const VEL_NOTCH: f32 = 4.0 / 128.0;

/// The dialog's controls (§1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// The Time knob's position, 0..1: the value −1..+1 is `2 × time − 1` (§1).
    pub time: f32,
    pub absolute: bool,
    pub before: bool,
    /// The flam note's share of the velocity, 0..1 (85/128 after Reset).
    pub vel: f32,
    pub group: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { time: 1.0 / 3.0, absolute: true, before: false, vel: 85.0 / 128.0, group: true }
    }
}

/// Options ▸ (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Flam,
    EarlyFlam,
    DoubleStroke,
    EarlyDoubleStroke,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Flam, Preset::EarlyFlam, Preset::DoubleStroke, Preset::EarlyDoubleStroke];

    pub fn name(self) -> &'static str {
        match self {
            Preset::Flam => "Flam",
            Preset::EarlyFlam => "Early flam",
            Preset::DoubleStroke => "Double stroke",
            Preset::EarlyDoubleStroke => "Early double stroke",
        }
    }
}

impl Settings {
    /// The Time value, −1..+1.
    pub fn value(&self) -> f64 {
        2.0 * f64::from(self.time) - 1.0
    }

    /// The signed offset in ticks (§2): milliseconds at the project's tempo, or steps.
    pub fn offset(&self, ppq: u32, bpm: f64) -> i64 {
        let ticks = if self.absolute { self.value() * 100.0 * bpm * f64::from(ppq) / 60000.0 } else { self.value() * f64::from(ppq) / 4.0 };
        ticks.round() as i64
    }

    /// Time and the switches as the preset sets them; Velocity and Group notes stay (§5).
    pub fn preset(&mut self, p: Preset) {
        let (time, absolute, before) = match p {
            Preset::Flam => (1.0 / 3.0, true, false),
            Preset::EarlyFlam => (1.0 / 3.0, true, true),
            Preset::DoubleStroke => (1.0, false, false),
            Preset::EarlyDoubleStroke => (0.0, false, true),
        };
        (self.time, self.absolute, self.before) = (time, absolute, before);
    }

    /// The hint-bar text of the Time knob (§1): whole milliseconds, or `step:tick` at `ppq`.
    pub fn time_text(&self, ppq: u32) -> String {
        if self.absolute {
            return format!("Time offset: {}ms", (self.value() * 100.0).round() as i64);
        }
        let step = i64::from((ppq / 4).max(1));
        let d = (self.value() * step as f64).round() as i64;
        let sign = if d < 0 { "-" } else { "" };
        format!("Time offset: {sign}{}:{:02}", d.abs() / step, d.abs() % step)
    }

    pub fn vel_text(&self) -> String {
        format!("Velocity offset: {}%", (f64::from(self.vel) * 100.0).round() as i64)
    }
}

/// Flam `notes` (the targets, in the pattern's order) by `d` ticks (§3). `used` are note group numbers to leave alone.
pub fn flam(notes: Vec<Note>, set: &Settings, d: i64, used: &[u16]) -> Vec<Note> {
    let t = d.unsigned_abs() as u32;
    let mut next = 1u16;
    let mut out = Vec::with_capacity(notes.len() * 2);
    for n in notes {
        let group = if set.group {
            while used.contains(&next) {
                next += 1;
            }
            next += 1;
            next - 1
        } else {
            0
        };
        let main = Note { unk14: group, ..n };
        let flam = Note { velocity: (f32::from(n.velocity) * set.vel).floor() as u8, unk14: group, ..n };
        let p = n.position;
        match (d.signum(), set.before) {
            (0, _) => out.push(main),
            (-1, false) => out.extend([Note { length: t, ..flam }, Note { position: p + t, ..main }]),
            (-1, true) => {
                let at = p.saturating_sub(t);
                if at < p {
                    out.push(Note { position: at, length: p - at, ..flam });
                }
                out.push(main);
            }
            (_, false) => out.extend([main, Note { position: p + t, ..flam }]),
            (_, true) => out.extend([Note { position: p.saturating_sub(t), ..main }, flam]),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, len: u32, key: u16, vel: u8) -> Note {
        Note { length: len, key, velocity: vel, ..Note::step(0, pos) }
    }

    /// The FL test project (PPQ 96, 130 BPM), in its list order.
    fn proj() -> Vec<Note> {
        vec![note(0, 96, 60, 100), note(96, 48, 64, 80), note(192, 96, 67, 110), note(384, 192, 72, 60), note(384, 96, 64, 127)]
    }

    /// start:length:key:velocity:group, sorted as the spec's tables.
    fn show(ns: &[Note]) -> String {
        let mut v: Vec<_> = ns.iter().map(|n| (n.position, n.key, n.length, n.velocity, n.unk14)).collect();
        v.sort_unstable();
        v.iter().map(|&(p, k, l, vel, g)| format!("{p}:{l}:{k}:{vel}:{g}")).collect::<Vec<_>>().join(" ")
    }

    fn run(f: impl FnOnce(&mut Settings)) -> String {
        let mut s = Settings::default();
        f(&mut s);
        show(&flam(proj(), &s, s.offset(96, 130.0), &[]))
    }

    fn notches(s: &mut Settings, n: i32) {
        s.time = (s.time + n as f32 * TIME_NOTCH).clamp(0.0, 1.0);
    }

    #[test]
    fn offsets_match_fl() {
        let d = |n: i32, absolute: bool| {
            let mut s = Settings { absolute, ..Settings::default() };
            notches(&mut s, n);
            s.offset(96, 130.0)
        };
        assert_eq!([d(0, true), d(-4, true), d(-8, true), d(4, true), d(8, true), d(16, true)], [-7, -14, -21, 0, 7, 21]);
        assert_eq!([d(0, false), d(-8, false), d(8, false), d(6, false), d(16, false)], [-8, -24, 8, 4, 24]);
    }

    #[test]
    fn reset_before_and_after_match_fl() {
        assert_eq!(run(|_| {}), "0:7:60:66:1 7:96:60:100:1 96:7:64:53:2 103:48:64:80:2 192:7:67:73:3 199:96:67:110:3 384:7:64:84:5 384:7:72:39:4 391:96:64:127:5 391:192:72:60:4");
        assert_eq!(run(|s| s.before = true), "0:96:60:100:1 89:7:64:53:2 96:48:64:80:2 185:7:67:73:3 192:96:67:110:3 377:7:64:84:5 377:7:72:39:4 384:96:64:127:5 384:192:72:60:4");
        assert_eq!(run(|s| notches(s, 8)), "0:96:60:100:1 7:96:60:66:1 96:48:64:80:2 103:48:64:53:2 192:96:67:110:3 199:96:67:73:3 384:96:64:127:5 384:192:72:60:4 391:96:64:84:5 391:192:72:39:4");
        assert_eq!(run(|s| (s.before, s.time) = (true, 0.5 + 1.0 / 6.0)), "0:96:60:66:1 0:96:60:100:1 89:48:64:80:2 96:48:64:53:2 185:96:67:110:3 192:96:67:73:3 377:96:64:127:5 377:192:72:60:4 384:96:64:84:5 384:192:72:39:4");
        assert_eq!(run(|s| notches(s, 4)), "0:96:60:100:1 96:48:64:80:2 192:96:67:110:3 384:96:64:127:5 384:192:72:60:4", "time 0: grouped, no flams");
        let plain = flam(proj(), &Settings { group: false, ..Settings::default() }, -7, &[]);
        let grouped = flam(proj(), &Settings::default(), -7, &[]);
        assert_eq!(plain, grouped.into_iter().map(|n| Note { unk14: 0, ..n }).collect::<Vec<_>>(), "Group notes off: as Reset, every group 0");
    }

    #[test]
    fn velocity_matches_fl() {
        let vels = |f: fn(&mut Settings)| {
            let mut s = Settings::default();
            f(&mut s);
            let out = flam(proj(), &s, -7, &[]);
            out.iter().filter(|n| n.length == 7).map(|n| (n.key, n.velocity)).collect::<Vec<_>>()
        };
        assert_eq!(vels(|s| s.vel -= 11.0 * VEL_NOTCH), [(60, 32), (64, 25), (67, 35), (72, 19), (64, 40)]);
        assert_eq!(vels(|s| s.vel = (s.vel + 11.0 * VEL_NOTCH).min(1.0)), [(60, 100), (64, 80), (67, 110), (72, 60), (64, 127)]);
        assert_eq!(vels(|s| s.vel = 0.0).iter().map(|v| v.1).max(), Some(0));
    }

    #[test]
    fn edges_and_presets_match_fl() {
        let edge = vec![note(3, 96, 60, 100), note(96, 48, 64, 80), note(100, 20, 67, 90)];
        let before = Settings { before: true, ..Settings::default() };
        assert_eq!(show(&flam(edge.clone(), &before, -7, &[])), "0:3:60:66:1 3:96:60:100:1 89:7:64:53:2 93:7:67:59:3 96:48:64:80:2 100:20:67:90:3");
        assert_eq!(show(&flam(edge, &before, 7, &[])), "0:96:60:100:1 3:96:60:66:1 89:48:64:80:2 93:20:67:90:3 96:48:64:53:2 100:20:67:59:3");
        let p = |pr: Preset| {
            let mut s = Settings::default();
            s.preset(pr);
            run(|x| *x = s)
        };
        assert_eq!(p(Preset::Flam), run(|_| {}));
        assert_eq!(p(Preset::EarlyFlam), run(|s| s.before = true));
        assert_eq!(p(Preset::DoubleStroke), "0:96:60:100:1 24:96:60:66:1 96:48:64:80:2 120:48:64:53:2 192:96:67:110:3 216:96:67:73:3 384:96:64:127:5 384:192:72:60:4 408:96:64:84:5 408:192:72:39:4");
        assert_eq!(p(Preset::EarlyDoubleStroke), "0:96:60:100:1 72:24:64:53:2 96:48:64:80:2 168:24:67:73:3 192:96:67:110:3 360:24:64:84:5 360:24:72:39:4 384:96:64:127:5 384:192:72:60:4");
    }

    #[test]
    fn groups_skip_used_numbers_and_texts() {
        let out = flam(proj()[..2].to_vec(), &Settings::default(), -7, &[1, 3]);
        assert_eq!(out.iter().map(|n| n.unk14).collect::<Vec<_>>(), [2, 2, 4, 4]);
        let mut s = Settings::default();
        assert_eq!(s.time_text(96), "Time offset: -33ms");
        assert_eq!(s.vel_text(), "Velocity offset: 66%");
        s.absolute = false;
        assert_eq!(s.time_text(96), "Time offset: -0:08");
        s.time = 0.0;
        assert_eq!(s.time_text(96), "Time offset: -1:00");
        s.time = 0.5 + 11.0 / 24.0;
        assert_eq!(s.time_text(96), "Time offset: 0:22");
        s.absolute = true;
        notches(&mut s, -6);
        assert_eq!(s.time_text(96), "Time offset: 42ms");
    }
}
