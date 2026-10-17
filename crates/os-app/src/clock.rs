// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The toolbar time display's modes: bar:step:tick, bar:beat:tick, minute:second:centisecond and SMPTE timecode.
//! App-wide, kept in `settings.txt` (`settings_file`) across runs.
// spec: ui/time-display.md

use crate::views::toolbar::{bst_ticks, msc_ticks};

/// What the display counts in (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Bar,
    Msc,
    /// SMPTE timecode at `RATES[i]`.
    Smpte(usize),
}

impl Mode {
    /// Its entry in the display's menu, also what `settings.txt` keeps under `clock_mode`.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Bar => "Bar : Beat/Step : Tick",
            Mode::Msc => "Minute : Second : Centisecond",
            Mode::Smpte(i) => RATES[i.min(RATES.len() - 1)].label,
        }
    }

    /// The mode whose menu entry is `text`.
    fn of_label(text: &str) -> Option<Mode> {
        [Mode::Bar, Mode::Msc].into_iter().chain((0..RATES.len()).map(Mode::Smpte)).find(|m| m.label() == text)
    }
}

/// An SMPTE rate: its menu entry, the frame clock it is shown on, whether frames run 1000/1001 slower than that
/// (23.976, 29.97, 59.94) and the frame numbers dropped at each minute but every tenth.
pub struct Rate {
    pub label: &'static str,
    pub nominal: u32,
    pub slow: bool,
    pub drop: u32,
}

const fn rate(label: &'static str, nominal: u32, slow: bool, drop: u32) -> Rate {
    Rate { label, nominal, slow, drop }
}

/// The SMPTE entries, in menu order (§1).
pub const RATES: [Rate; 13] = [
    rate("SMPTE 23.976 fps", 24, true, 0),
    rate("SMPTE 24 fps", 24, false, 0),
    rate("SMPTE 25 fps", 25, false, 0),
    rate("SMPTE 29.97 fps (drop)", 30, true, 2),
    rate("SMPTE 29.97 (non-drop)", 30, true, 0),
    rate("SMPTE 30 fps", 30, false, 0),
    rate("SMPTE 48 fps", 48, false, 0),
    rate("SMPTE 50 fps", 50, false, 0),
    rate("SMPTE 59.94 fps (drop)", 60, true, 4),
    rate("SMPTE 59.94 (non-drop)", 60, true, 0),
    rate("SMPTE 60 fps", 60, false, 0),
    rate("SMPTE 72 fps", 72, false, 0),
    rate("SMPTE 120 fps", 120, false, 0),
];

/// The display's settings: mode and middle unit (Beat when `beat`, else Step). Defaults: Bar, Step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Clock {
    pub mode: Mode,
    pub beat: bool,
}

impl Clock {
    /// The settings kept in `settings.txt`: `clock_mode` (the mode's menu entry) and `clock_middle_unit` (`Step` or
    /// `Beat`); the default for one that is missing or not known.
    pub fn load() -> Clock {
        use crate::settings_file::get;
        let mode = get("clock_mode").and_then(|t| Mode::of_label(&t)).unwrap_or_default();
        Clock { mode, beat: get("clock_middle_unit").as_deref() == Some("Beat") }
    }

    /// Keeps the settings for the next start.
    pub fn save(&self) {
        use crate::settings_file::write;
        write("clock_mode", self.mode.label());
        write("clock_middle_unit", if self.beat { "Beat" } else { "Step" });
    }

    /// A left click: Bar and minute:second:centisecond in turn; SMPTE goes to Bar (§3).
    pub fn toggle(&mut self) {
        self.mode = if self.mode == Mode::Bar { Mode::Msc } else { Mode::Bar };
    }

    /// The small label at the display's top right (§2).
    pub fn label(&self) -> &'static str {
        match self.mode {
            Mode::Bar if self.beat => "B:B:T",
            Mode::Bar => "B:S:T",
            Mode::Msc => "M:S:CS",
            Mode::Smpte(_) => "M:S:FR",
        }
    }

    pub fn hint(&self) -> &'static str {
        match self.mode {
            Mode::Bar if self.beat => "Song position (bar:beat:tick)",
            Mode::Bar => "Song position (bar:step:tick)",
            Mode::Msc => "Song position (minute:second:centisecond)",
            Mode::Smpte(_) => "Song position (minute:second:frame)",
        }
    }

    /// The three numbers shown for a song position given in seconds and in ticks of the display's 96-per-quarter
    /// clock, in a bar of signature `num`/`den`.
    pub fn reading(&self, secs: f64, ticks96: f64, (num, den): (u16, u16)) -> (u32, u32, u32) {
        match self.mode {
            Mode::Bar if self.beat => bbt(ticks96, num, den),
            Mode::Bar => bst_ticks(ticks96),
            Mode::Msc => msc_ticks(secs),
            Mode::Smpte(i) => smpte(secs, &RATES[i.min(RATES.len() - 1)]),
        }
    }
}

/// Bar:beat:tick of a position in 96-per-quarter ticks: bar and beat from 1, the beat a `1/den` note, `num` to the
/// bar, the tick within the beat (§3).
pub fn bbt(ticks96: f64, num: u16, den: u16) -> (u32, u32, u32) {
    let ticks = ticks96.max(0.0) as u32;
    let beat = (96 * 4 / u32::from(den.clamp(1, 64))).max(1);
    let beats = ticks / beat;
    let per_bar = u32::from(num.max(1));
    (beats / per_bar + 1, beats % per_bar + 1, ticks % beat)
}

/// Minutes, seconds and frames of a position in seconds: the whole frames elapsed at the rate's real speed, as
/// timecode (§3).
pub fn smpte(secs: f64, r: &Rate) -> (u32, u32, u32) {
    let fps = if r.slow { f64::from(r.nominal) * 1000.0 / 1001.0 } else { f64::from(r.nominal) };
    timecode((secs.max(0.0) * fps + 1e-9) as u64, r)
}

/// Minutes, seconds and frames of frame `count` on the rate's nominal clock. Drop-frame rates skip frame numbers
/// 0..drop at the start of each minute but every tenth (SMPTE drop-frame timecode).
pub fn timecode(count: u64, r: &Rate) -> (u32, u32, u32) {
    let n = u64::from(r.nominal);
    let mut f = count;
    if r.drop > 0 {
        let drop = u64::from(r.drop);
        let (per_min, per_10) = (n * 60 - drop, n * 600 - drop * 9);
        let (tens, rest) = (f / per_10, f % per_10);
        f += drop * 9 * tens + if rest > drop { drop * ((rest - drop) / per_min) } else { 0 };
    }
    ((f / (n * 60)) as u32, (f / n % 60) as u32, (f % n) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate_of(label: &str) -> &'static Rate {
        RATES.iter().find(|r| r.label == label).unwrap()
    }

    /// Every mode is kept as its menu entry and read back; an unknown entry or unit reads the default.
    #[test]
    fn settings_are_kept() {
        assert_eq!(Clock::load(), Clock::default());
        for m in [Mode::Bar, Mode::Msc].into_iter().chain((0..RATES.len()).map(Mode::Smpte)) {
            Clock { mode: m, beat: true }.save();
            assert_eq!(Clock::load(), Clock { mode: m, beat: true });
        }
        assert_eq!(crate::settings_file::get("clock_mode").as_deref(), Some("SMPTE 120 fps"));
        crate::settings_file::write("clock_mode", "SMPTE 1000 fps");
        crate::settings_file::write("clock_middle_unit", "Quarter");
        assert_eq!(Clock::load(), Clock::default());
    }

    #[test]
    fn smpte_timecode() {
        // The measured case (§2): 1.495 s at 25 fps is 0:01:12.
        assert_eq!(smpte(1.495, rate_of("SMPTE 25 fps")), (0, 1, 12));
        assert_eq!(smpte(61.0, rate_of("SMPTE 24 fps")), (1, 1, 0));
        assert_eq!(smpte(1.0, rate_of("SMPTE 120 fps")), (0, 1, 0));
        // Fractional rates count frames at the real speed: one second of 29.97 is 29 frames, 1.001 s is 30.
        let nd = rate_of("SMPTE 29.97 (non-drop)");
        assert_eq!(smpte(1.0, nd), (0, 0, 29));
        assert_eq!(smpte(1.001, nd), (0, 1, 0));
        assert_eq!(timecode(1800, nd), (1, 0, 0));
        assert_eq!(smpte(1.001, rate_of("SMPTE 23.976 fps")), (0, 1, 0));
        // 29.97 drop: frames 0 and 1 are skipped at each minute, except every tenth.
        let df = rate_of("SMPTE 29.97 fps (drop)");
        assert_eq!(timecode(1799, df), (0, 59, 29));
        assert_eq!(timecode(1800, df), (1, 0, 2));
        assert_eq!(timecode(3597, df), (1, 59, 29));
        assert_eq!(timecode(3598, df), (2, 0, 2));
        assert_eq!(timecode(17981, df), (9, 59, 29));
        assert_eq!(timecode(17982, df), (10, 0, 0));
        assert_eq!(timecode(17982 + 1800, df), (11, 0, 2));
        // An hour of 29.97 drop-frame is 107892 frames and reads 60:00:00 (the timecode keeps to the clock).
        assert_eq!(timecode(107_892, df), (60, 0, 0));
        assert_eq!(smpte(3600.0, df), (60, 0, 0));
        // 59.94 drop skips 0-3.
        let df60 = rate_of("SMPTE 59.94 fps (drop)");
        assert_eq!(timecode(3599, df60), (0, 59, 59));
        assert_eq!(timecode(3600, df60), (1, 0, 4));
        assert_eq!(timecode(35964, df60), (10, 0, 0));
    }

    #[test]
    fn bar_beat_tick() {
        // The measured case (§2): step 13, tick 23 of bar 1 in 4/4 is beat 4, tick 23.
        assert_eq!(bbt(12.0 * 24.0 + 23.0, 4, 4), (1, 4, 23));
        assert_eq!(bbt(0.0, 4, 4), (1, 1, 0));
        assert_eq!(bbt(384.0 + 95.0, 4, 4), (2, 1, 95));
        // 3/8: eighth-note beats of 48 ticks, three to the bar.
        assert_eq!(bbt(144.0, 3, 8), (2, 1, 0));
        assert_eq!(bbt(100.0, 3, 8), (1, 3, 4));
    }

    #[test]
    fn modes_and_clicks() {
        let mut c = Clock::default();
        assert_eq!((c.mode, c.beat, c.label()), (Mode::Bar, false, "B:S:T"));
        c.toggle();
        assert_eq!((c.mode, c.label()), (Mode::Msc, "M:S:CS"));
        c.toggle();
        assert_eq!(c.mode, Mode::Bar);
        c.mode = Mode::Smpte(2);
        assert_eq!(c.label(), "M:S:FR");
        c.toggle();
        assert_eq!(c.mode, Mode::Bar);
        c.beat = true;
        assert_eq!(c.label(), "B:B:T");
        // Middle unit Beat only matters in Bar mode.
        c.mode = Mode::Msc;
        assert_eq!(c.label(), "M:S:CS");
    }
}
