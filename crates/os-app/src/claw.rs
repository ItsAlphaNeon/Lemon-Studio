// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The piano roll's Claw machine (Tools ▸ Claw machine…, Alt+W): its settings and the change itself, free of any
//! window. It trashes a part of every period, closes the gap or stretches the rest over it, and bends time inside each
//! period. The dialog's session is `piano_dlg.rs`, the window `views/piano_dlg.rs`.
// spec: ui/piano-claw.md

use os_model::Note;

/// The Time dist knob: 32 wheel notches from the centre to either end (§1), on its 0..1 position.
pub const TIME_NOTCH: f32 = 1.0 / 64.0;

/// Period ▸ (§1): beats and name.
pub const PERIODS: [(u32, &str); 5] = [(1, "Beat"), (2, "2 beats"), (4, "4 beats"), (8, "8 beats"), (16, "16 beats")];

/// Trash every ▸ (§1): the k-th of n parts of each period, or nothing; a separator comes before the entries in
/// `TRASH_SEPARATED`.
pub const TRASH: [(Option<(u32, u32)>, &str); 10] = [
    (Some((1, 4)), "1/4"),
    (Some((2, 4)), "2/4"),
    (Some((3, 4)), "3/4"),
    (Some((4, 4)), "4/4"),
    (Some((1, 3)), "1/3"),
    (Some((2, 3)), "2/3"),
    (Some((3, 3)), "3/3"),
    (Some((1, 2)), "1/2"),
    (Some((2, 2)), "2/2"),
    (None, "(none)"),
];
pub const TRASH_SEPARATED: [usize; 3] = [4, 7, 9];

/// The dialog's controls (§1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Index into `PERIODS`.
    pub period: usize,
    /// Index into `TRASH`.
    pub trash: usize,
    /// The Time dist knob's position, 0..1: the value −1..+1 is `2 × time − 1`.
    pub time: f32,
    pub stretch: bool,
    pub short: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { period: 0, trash: 3, time: 0.5, stretch: true, short: true }
    }
}

impl Settings {
    /// Time dist's value, −1..+1.
    pub fn value(&self) -> f64 {
        2.0 * f64::from(self.time) - 1.0
    }

    pub fn period_name(&self) -> &'static str {
        PERIODS[self.period.min(PERIODS.len() - 1)].1
    }

    pub fn trash_name(&self) -> &'static str {
        TRASH[self.trash.min(TRASH.len() - 1)].1
    }

    /// The knob's hint (a Lemon Studio choice, §5).
    pub fn time_text(&self) -> String {
        match (self.value() * 100.0).round() as i64 {
            0 => "Time distribution: 0%".into(),
            p => format!("Time distribution: {p:+}%"),
        }
    }
}

/// Time dist (§3.2): where the fraction `s` (0..1) of a period goes for the value `t` (−1..+1). A positive value runs
/// fast at the period's start and slow at its end, the start `R = (1 + 101^t) / 2` times as fast; a negative one is
/// that mirrored in time.
pub fn curve(s: f64, t: f64) -> f64 {
    if t.abs() < 1e-9 {
        return s;
    }
    let r = (1.0 + 101f64.powf(t.abs())) / 2.0;
    let g = |s: f64| (1.0 - r.powf(-s)) / (1.0 - 1.0 / r);
    if t > 0.0 { g(s) } else { 1.0 - g(1.0 - s) }
}

/// Run the Claw machine over `notes` at `ppq` (§3). Each note keeps its key and other properties; its start and length
/// change, or it is removed.
pub fn claw(notes: Vec<Note>, set: &Settings, ppq: u32) -> Vec<Note> {
    let ppq = ppq.max(1);
    let p = f64::from(PERIODS[set.period.min(PERIODS.len() - 1)].0 * ppq);
    // The trashed part of every period: its start and width (§3.1); the time each period keeps.
    let (a, w) = match TRASH[set.trash.min(TRASH.len() - 1)].0 {
        Some((k, n)) => (p * f64::from(k - 1) / f64::from(n), p / f64::from(n)),
        None => (0.0, 0.0),
    };
    let c = p - w;
    let kept = |x: f64| {
        let q = (x / p).floor();
        let r = x - q * p;
        q * c + if r <= a { r } else if r <= a + w { a } else { r - w }
    };
    let in_trash = |x: f64| {
        let r = x - (x / p).floor() * p;
        w > 0.0 && r >= a && r < a + w
    };
    let t = set.value();
    // Trash and stretch, then Time dist on the new timeline's periods; rounded once, a half to the even tick (§3.3).
    let to = |x: f64| {
        let y = if set.stretch { kept(x) * p / c } else { kept(x) };
        let q = (y / p).floor();
        let z = q * p + p * curve(y / p - q, t);
        z.round_ties_even().max(0.0) as i64
    };
    // A 24th of a beat: 4 ticks at PPQ 96 (§3.4; following the PPQ is a Lemon Studio choice, §5).
    let min = i64::from((ppq / 24).max(1));
    notes
        .into_iter()
        .filter_map(|n| {
            let s = f64::from(n.position);
            // A channel-rack step (no length) stays one: it goes with a trashed part, or its start moves (§5).
            if n.length == 0 {
                return (!in_trash(s)).then(|| Note { position: to(s) as u32, ..n });
            }
            let e = s + f64::from(n.length);
            if kept(e) - kept(s) < 1e-9 {
                return None;
            }
            let (s2, e2) = (to(s), to(e));
            let mut len = e2 - s2;
            if len < min {
                if set.short {
                    return None;
                }
                len = min;
            }
            Some(Note { position: s2 as u32, length: len as u32, ..n })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(pos: u32, len: u32, key: u16) -> Note {
        Note { length: len, key, velocity: 100, ..Note::step(0, pos) }
    }

    /// Sixteen one-step notes on rising keys (§4).
    fn steps() -> Vec<Note> {
        (0..16).map(|i| note(i * 24, 24, 60 + i as u16)).collect()
    }

    fn parse(s: &str) -> Vec<Note> {
        s.split_whitespace()
            .map(|t| {
                let v: Vec<u32> = t.split(':').map(|x| x.parse().unwrap()).collect();
                note(v[0], v[1], v[2] as u16)
            })
            .collect()
    }

    /// start:length:key in start (then key) order, as the spec's tables.
    fn show(ns: &[Note]) -> String {
        let mut v: Vec<_> = ns.iter().map(|n| (n.position, n.key, n.length)).collect();
        v.sort_unstable();
        v.iter().map(|&(p, k, l)| format!("{p}:{l}:{k}")).collect::<Vec<_>>().join(" ")
    }

    fn run(notes: Vec<Note>, f: impl FnOnce(&mut Settings)) -> String {
        let mut s = Settings::default();
        f(&mut s);
        show(&claw(notes, &s, 96))
    }

    fn trash(name: &str) -> usize {
        TRASH.iter().position(|t| t.1 == name).unwrap()
    }

    fn notches(s: &mut Settings, n: i32) {
        s.time = (s.time + n as f32 * TIME_NOTCH).clamp(0.0, 1.0);
    }

    #[test]
    fn trash_every_matches_fl() {
        let each = |beat: &str| (0..4).map(|b| beat.split(' ').map(|t| {
            let v: Vec<u32> = t.split(':').map(|x| x.parse().unwrap()).collect();
            format!("{}:{}:{}", v[0] + 96 * b, v[1], v[2] + 4 * b)
        }).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" ");
        let gone = |keys: &[u16], len: u32| {
            let kept: Vec<u16> = (60..76).filter(|k| !keys.contains(k)).collect();
            kept.iter().enumerate().map(|(i, k)| format!("{}:{len}:{k}", i as u32 * len)).collect::<Vec<_>>().join(" ")
        };
        assert_eq!(run(steps(), |_| {}), gone(&[63, 67, 71, 75], 32));
        assert_eq!(run(steps(), |s| s.trash = trash("2/4")), gone(&[61, 65, 69, 73], 32));
        assert_eq!(run(steps(), |s| s.trash = trash("3/4")), gone(&[62, 66, 70, 74], 32));
        assert_eq!(run(steps(), |s| s.trash = trash("1/3")), each("0:24:61 24:36:62 60:36:63"));
        assert_eq!(run(steps(), |s| s.trash = trash("2/3")), each("0:36:60 36:12:61 48:12:62 60:36:63"));
        assert_eq!(run(steps(), |s| s.trash = trash("3/3")), each("0:36:60 36:36:61 72:24:62"));
        assert_eq!(run(steps(), |s| s.trash = trash("1/2")), each("0:48:62 48:48:63"));
        assert_eq!(run(steps(), |s| s.trash = trash("2/2")), each("0:48:60 48:48:61"));
        assert_eq!(run(steps(), |s| s.trash = trash("(none)")), show(&steps()));
        let keys = |r: &str| r.split(' ').map(|t| t.rsplit(':').next().unwrap().parse::<u16>().unwrap()).collect::<Vec<_>>();
        let missing = |r: &str| (60..76).filter(|k| !keys(r).contains(k)).collect::<Vec<_>>();
        assert_eq!(missing(&run(steps(), |s| s.period = 1)), [66, 67, 74, 75]);
        assert_eq!(missing(&run(steps(), |s| s.period = 2)), [72, 73, 74, 75]);
        assert_eq!(run(steps(), |s| s.stretch = false), gone(&[63, 67, 71, 75], 24));
        assert!(run(steps(), |s| (s.trash, s.stretch) = (trash("2/4"), false)).starts_with("0:24:60 24:24:62 48:24:63 72:24:64 "));
    }

    #[test]
    fn long_and_overlapping_notes_match_fl() {
        let long = || parse("0:96:60 96:96:62 192:96:64 288:96:65 384:384:67");
        assert_eq!(run(long(), |_| {}), show(&long()));
        assert_eq!(run(long(), |s| s.trash = trash("1/2")), show(&long()));
        assert_eq!(run(long(), |s| s.stretch = false), "0:72:60 72:72:62 144:72:64 216:72:65 288:288:67");
        let shorts = || (0..24).map(|i| note(i * 4, 24, 48 + i as u16)).collect::<Vec<_>>();
        assert_eq!(
            run(shorts(), |_| {}),
            "0:32:48 5:32:49 11:32:50 16:32:51 21:32:52 27:32:53 32:32:54 37:32:55 43:32:56 48:32:57 53:32:58 59:32:59 64:32:60 69:27:61 75:21:62 80:16:63 85:11:64 91:5:65 96:5:67 96:11:68 96:16:69 96:21:70 96:27:71"
        );
        assert!(run(shorts(), |s| s.stretch = false).ends_with("48:24:60 52:20:61 56:16:62 60:12:63 64:8:64 68:4:65 72:4:67 72:8:68 72:12:69 72:16:70 72:20:71"));
        let half = run(shorts(), |s| s.trash = trash("1/2"));
        assert!(half.starts_with("0:8:55 0:16:56 0:24:57 0:32:58 0:40:59 0:48:60 8:48:61 ") && half.ends_with(" 88:8:71"), "{half}");
    }

    #[test]
    fn short_notes_match_fl() {
        let tiny = || parse("69:24:60 70:24:61 71:24:62 75:22:63 74:24:64 73:26:65 200:1:66 210:2:67 220:3:68 270:1:69");
        assert_eq!(run(tiny(), |_| {}), "92:4:60 96:4:65 229:4:68");
        assert_eq!(run(tiny(), |s| s.short = false), "92:4:60 93:4:61 95:4:62 96:4:63 96:4:64 96:4:65 203:4:66 216:4:67 229:4:68");
        assert_eq!(run(tiny(), |s| s.stretch = false), "");
        assert_eq!(run(tiny(), |s| (s.stretch, s.short) = (false, false)), "69:4:60 70:4:61 71:4:62 72:4:63 72:4:64 72:4:65 152:4:66 162:4:67 172:4:68");
        let two = "69:24:60 70:24:61 71:24:62 73:26:65 74:24:64 75:22:63";
        assert_eq!(run(tiny(), |s| (s.period, s.stretch) = (1, false)), two);
        assert_eq!(run(tiny(), |s| (s.period, s.stretch, s.short) = (1, false, false)), format!("{two} 152:4:66 162:4:67 172:4:68 222:4:69"));
    }

    #[test]
    fn rounding_matches_fl() {
        let odd = || parse("33:3:60 35:3:61 37:3:62 39:3:63 41:1:64 43:5:65");
        assert_eq!(run(odd(), |s| s.trash = trash("1/3")), "2:4:60 4:5:61 8:4:62 10:5:63 16:8:65");
        assert_eq!(run(odd(), |s| (s.trash, s.time) = (trash("1/3"), 0.5 + 10.0 * TIME_NOTCH)), "2:7:60 7:6:61 11:7:62 16:6:63 24:9:65");
        assert_eq!(
            run(odd(), |s| (s.trash, s.time, s.short) = (trash("1/3"), 0.5 + 10.0 * TIME_NOTCH, false)),
            "2:7:60 7:6:61 11:7:62 16:6:63 20:4:64 24:9:65"
        );
    }

    #[test]
    fn time_dist_matches_fl() {
        let beats = |n: i32| {
            let r = run(steps(), |s| {
                s.trash = trash("(none)");
                notches(s, n);
            });
            r.split(' ').take(4).map(|t| t.rsplit_once(':').unwrap().0.to_string()).collect::<Vec<_>>().join(" ")
        };
        assert_eq!(beats(5), "0:28 28:25 53:23 76:20");
        assert_eq!(beats(-5), "0:20 20:23 43:25 68:28");
        assert_eq!(beats(20), "0:46 46:26 72:15 87:9");
        assert_eq!(beats(-20), "0:9 9:15 24:26 50:46");
        assert!(run(steps(), |s| notches(s, 10)).starts_with("0:43:60 43:31:61 74:22:62 96:43:64 "));
        assert!(run(steps(), |s| notches(s, -10)).starts_with("0:22:60 22:31:61 53:43:62 "));
        assert!(run(steps(), |s| {
            s.stretch = false;
            notches(s, 10);
        })
        .starts_with("0:33:60 33:26:61 59:21:62 80:16:64 96:33:65 129:26:66 155:21:68 176:16:69 192:33:70 "));
        assert!(run(steps(), |s| (s.period, s.trash, s.time) = (1, trash("1/3"), 0.5 - 6.0 * TIME_NOTCH)).starts_with("0:9:62 9:30:63 39:33:64 72:36:65 108:40:66 148:44:67 192:9:70 "));
        assert!(run(steps(), |s| (s.period, s.trash, s.time, s.stretch) = (2, trash("2/4"), 0.5 + 12.0 * TIME_NOTCH, false))
            .starts_with("0:40:60 40:37:61 77:34:62 111:31:63 142:30:68 172:27:69 199:25:70 224:24:71 "));
        // 64 one-step notes over 16 beats.
        let runs = || (0..64).map(|i| note(i * 24, 24, 36 + (i % 48) as u16)).collect::<Vec<_>>();
        let starts = |n: i32| {
            let mut s = Settings { period: 4, trash: trash("(none)"), ..Settings::default() };
            notches(&mut s, n);
            let mut v: Vec<u32> = claw(runs(), &s, 96).iter().map(|n| n.position).collect();
            v.sort_unstable();
            v
        };
        assert_eq!(starts(32)[..10], [0, 93, 181, 264, 341, 414, 483, 548, 608, 665]);
        assert_eq!(starts(32).len(), 54, "ten notes under 4 ticks removed");
        assert_eq!(starts(40), starts(32), "the knob stops at the end");
        assert_eq!(starts(1)[..9], [0, 25, 50, 75, 99, 124, 149, 174, 198]);
    }

    #[test]
    fn curve_and_texts() {
        assert_eq!(curve(0.3, 0.0), 0.3);
        for t in [0.25, -0.5, 1.0] {
            assert!(curve(0.0, t).abs() < 1e-12 && (curve(1.0, t) - 1.0).abs() < 1e-12);
        }
        assert!((curve(0.5, 1.0) - 1.0 / (1.0 + 51f64.sqrt().recip())).abs() < 1e-12, "R = 51 at +100 %");
        let mut s = Settings::default();
        assert_eq!(s.time_text(), "Time distribution: 0%");
        notches(&mut s, 8);
        assert_eq!(s.time_text(), "Time distribution: +25%");
        notches(&mut s, -40);
        assert_eq!(s.time_text(), "Time distribution: -100%");
        assert_eq!((s.period_name(), Settings::default().trash_name()), ("Beat", "4/4"));
    }

    #[test]
    fn steps_without_length() {
        let s = Settings::default();
        let out = claw(vec![note(10, 0, 60), note(80, 0, 61)], &s, 96);
        assert_eq!(out.iter().map(|n| (n.position, n.length)).collect::<Vec<_>>(), [(13, 0)], "the one in the trashed quarter goes");
    }
}
