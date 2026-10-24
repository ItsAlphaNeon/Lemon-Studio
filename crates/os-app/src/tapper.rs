// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Tempo tapper (VIEW > Tempo tapper, Ctrl+F5): taps on its pad give a tempo, the average of the time between the
//! taps as a whole number of BPM, written to the project tempo while Apply tempo is on. The counting is here, free of
//! any clock (taps are times in seconds); the window is `views/tapper.rs`. Apply tempo, Sync and Mute are program
//! settings, not part of the project: kept in the user folder (`tapper.txt`).
// spec: ui/tempo-tapper.md

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// After the first tap, a second one this long after it or later starts a new count (§2).
pub const FIRST_GAP: f64 = 2.0;
/// Later, a tap this many average intervals after the previous one or later starts a new count (§2).
pub const GAP_FACTOR: f64 = 1.5;
/// The beat bar is full from this tap of a count on (§3).
pub const BAR_TAPS: usize = 16;
/// The beat bar's track, x from the window's left: left edge and right end (exclusive), spec §1.
pub const BAR_X0: f32 = 5.0;
pub const BAR_X1: f32 = 151.0;
/// How long a tap's flash of the pad takes to fade (spec §8).
pub const FLASH_SECS: f32 = 0.25;
/// MIDI keys pressed this close together (a chord) are one tap: no one taps faster (1000 BPM).
pub const MIDI_CHORD: f64 = 0.06;

/// One count of taps: when it started, its latest tap and how many taps it has (§2).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tapper {
    first: f64,
    last: f64,
    taps: usize,
}

impl Tapper {
    /// A tap at `t` seconds: it continues the count, or starts a new one when it comes too long after the previous
    /// tap (§2). Returns the new tempo, from the second tap of a count on.
    pub fn tap(&mut self, t: f64) -> Option<f64> {
        let gap = t - self.last;
        let restart = match self.taps {
            0 => true,
            1 => gap >= FIRST_GAP,
            n => gap >= GAP_FACTOR * (self.last - self.first) / (n - 1) as f64,
        };
        if restart {
            *self = Tapper { first: t, last: t, taps: 1 };
        } else {
            self.last = t;
            self.taps += 1;
        }
        self.tempo()
    }

    /// Taps in the current count (0 before the first).
    #[cfg(test)]
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// When the latest tap was.
    pub fn last(&self) -> Option<f64> {
        (self.taps > 0).then_some(self.last)
    }

    /// The tempo of the count: 60 / the average of every interval since the count started, rounded to a whole BPM;
    /// None before its second tap (§2).
    pub fn tempo(&self) -> Option<f64> {
        let span = self.last - self.first;
        if self.taps < 2 || span <= 0.0 {
            return None;
        }
        Some((60.0 * (self.taps - 1) as f64 / span).round())
    }

    /// The beat bar (§3): where its fill ends (x from the window's left; right end at about 8.4 n + 8.6 after tap n,
    /// the whole track from the 16th tap on) and whether it is the accent colour (taps 1, 5, 9, 13, ...: the first
    /// beat of each group of four). None before the first tap.
    pub fn bar(&self) -> Option<(f32, bool)> {
        let n = self.taps;
        if n == 0 {
            return None;
        }
        let end = if n >= BAR_TAPS { BAR_X1 } else { (8.4 * n as f32 + 8.6).min(BAR_X1) };
        Some((end, n % 4 == 1))
    }
}

/// What the window's working controls do (Sync, Mute and now are not available yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapAct {
    /// The pad: a tap on press.
    Pad,
    /// The Apply tempo switch.
    Apply,
    /// − nudge (-1) / nudge + (+1), held: playback runs slower / faster until the button is let go (§6).
    Nudge(i8),
}

/// The nudge being held: -1, 0 or +1 (spec §6). Every playback engine reads it (`Engine::set_nudge`); a press on a
/// nudge button sets it and any release of the mouse button clears it. One per test thread under tests.
pub fn nudge_flag() -> std::sync::Arc<std::sync::atomic::AtomicI8> {
    use std::sync::{atomic::AtomicI8, Arc};
    #[cfg(test)]
    {
        thread_local!(static TEST: Arc<AtomicI8> = Arc::new(AtomicI8::new(0)));
        TEST.with(Arc::clone)
    }
    #[cfg(not(test))]
    {
        static FLAG: std::sync::OnceLock<Arc<AtomicI8>> = std::sync::OnceLock::new();
        FLAG.get_or_init(|| Arc::new(AtomicI8::new(0))).clone()
    }
}

/// The nudge held now (`nudge_flag`).
pub fn nudging() -> i8 {
    nudge_flag().load(std::sync::atomic::Ordering::Relaxed)
}

/// The window's state: where it is (centred over the main window the first time, then where it was left, also in later
/// runs: `layout`), the count and the pad's flash.
pub struct TapperUi {
    /// Top-left corner (logical px).
    pub pos: (f32, f32),
    pub placed: bool,
    pub count: Tapper,
    /// When the last tap lit the pad, while its flash fades, and how lit it is (1 at the tap, 0 when gone).
    pub flash_at: Option<Instant>,
    pub flash: f32,
    /// The tapper's clock: taps are seconds since this.
    epoch: Instant,
    /// Tests tap at times of their own.
    #[cfg(test)]
    pub test_time: Option<f64>,
    /// Tests record the taps' clicks (accented or not) instead of opening an audio device.
    #[cfg(test)]
    pub test_clicks: Vec<bool>,
}

impl Default for TapperUi {
    fn default() -> Self {
        TapperUi {
            pos: (0.0, 0.0),
            placed: false,
            count: Tapper::default(),
            flash_at: None,
            flash: 0.0,
            epoch: Instant::now(),
            #[cfg(test)]
            test_time: None,
            #[cfg(test)]
            test_clicks: Vec::new(),
        }
    }
}

impl TapperUi {
    /// Seconds on the tapper's clock now.
    pub fn clock(&self) -> f64 {
        #[cfg(test)]
        if let Some(t) = self.test_time {
            return t;
        }
        self.epoch.elapsed().as_secs_f64()
    }

    /// Fades the pad's flash (spec §4, §8: over 250 ms). Returns whether it is still fading or just went out (the pad
    /// needs painting).
    pub fn fade(&mut self, now: Instant) -> bool {
        let Some(at) = self.flash_at else { return false };
        let k = 1.0 - now.saturating_duration_since(at).as_secs_f32() / FLASH_SECS;
        if k <= 0.0 {
            self.flash_at = None;
            self.flash = 0.0;
        } else {
            self.flash = k;
        }
        true
    }
}

// ---------------------------------------------------------------- settings (spec §7, §8)

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// New tempos set the project tempo.
    pub apply: bool,
    /// Kept for when Sync and Mute work.
    pub sync: bool,
    pub mute: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { apply: true, sync: false, mute: false }
    }
}

#[cfg_attr(test, allow(dead_code))]
static SETTINGS: OnceLock<Mutex<Settings>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// Tests run in parallel threads: each has its own settings, so one test's switch never changes another's tempo.
    static TEST: std::cell::Cell<Settings> = std::cell::Cell::new(Settings::default());
}

/// Runs `f` on the settings; keeps them when `f` changed them.
fn with<R>(f: impl FnOnce(&mut Settings) -> R) -> R {
    #[cfg(test)]
    return TEST.with(|c| {
        let mut s = c.get();
        let r = f(&mut s);
        c.set(s);
        r
    });
    #[cfg(not(test))]
    {
        let mut s = SETTINGS.get_or_init(|| Mutex::new(load())).lock().unwrap_or_else(|e| e.into_inner());
        let before = *s;
        let r = f(&mut s);
        if *s != before {
            save(&s);
        }
        r
    }
}

/// The current settings: read from `tapper.txt` the first time.
pub fn get() -> Settings {
    with(|s| *s)
}

/// Toggles Apply tempo and keeps it; returns the new state.
pub fn toggle_apply() -> bool {
    with(|s| {
        s.apply = !s.apply;
        s.apply
    })
}

/// `key=value` lines: `apply` (missing: 1), `sync` and `mute` (missing: 0), each 0 / 1; anything else is ignored.
pub fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let on = v.trim() == "1";
        match k.trim() {
            "apply" => s.apply = on,
            "sync" => s.sync = on,
            "mute" => s.mute = on,
            _ => {}
        }
    }
    s
}

pub fn text(s: &Settings) -> String {
    format!("apply={}\nsync={}\nmute={}\n", u8::from(s.apply), u8::from(s.sync), u8::from(s.mute))
}

#[cfg_attr(test, allow(dead_code))]
fn load() -> Settings {
    os_paths::user_file("tapper.txt").and_then(|f| std::fs::read_to_string(f).ok()).map(|t| parse(&t)).unwrap_or_default()
}

#[cfg_attr(test, allow(dead_code))]
fn save(s: &Settings) {
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("tapper.txt"), text(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Taps at the given gaps (seconds) after a first tap at 0; the tempo after each tap.
    fn run(gaps: &[f64]) -> (Tapper, Vec<Option<f64>>) {
        let mut tp = Tapper::default();
        let mut t = 0.0;
        let mut out = vec![tp.tap(t)];
        for g in gaps {
            t += g;
            out.push(tp.tap(t));
        }
        (tp, out)
    }

    /// spec §2: one tap shows nothing; then 60 / the average interval, rounded to a whole BPM.
    #[test]
    fn tempo_is_the_rounded_average() {
        let (tp, out) = run(&[0.5]);
        assert_eq!(out, [None, Some(120.0)]);
        assert_eq!(tp.taps(), 2);
        // 127.66 BPM shows and applies as 128.
        assert_eq!(run(&[60.0 / 127.66]).1[1], Some(128.0));
        assert_eq!(run(&[60.0 / 127.4]).1[1], Some(127.0));
        assert_eq!(Tapper::default().tempo(), None);
    }

    /// spec §2: every interval since the count started is averaged (no sliding window).
    #[test]
    fn every_interval_of_the_count_is_averaged() {
        let gaps: Vec<f64> = std::iter::repeat_n(0.5, 19).chain(std::iter::repeat_n(0.4, 8)).collect();
        let (tp, out) = run(&gaps);
        assert_eq!(tp.taps(), 28);
        // Taps 24, 26 and 28 (the measured 124, 126, 128).
        assert_eq!((out[23], out[25], out[27]), (Some(124.0), Some(126.0), Some(128.0)));
        let gaps: Vec<f64> = std::iter::repeat_n(0.5, 7).chain(std::iter::repeat_n(0.4, 8)).collect();
        assert_eq!(run(&gaps).1[15], Some(134.0));
    }

    /// spec §2: after the first tap a second one 2 s later or more starts a new count; later 1.5 x the average
    /// interval does, with no absolute limit.
    #[test]
    fn a_long_gap_starts_a_new_count() {
        let (tp, out) = run(&[1.98]);
        assert_eq!((tp.taps(), out[1]), (2, Some(30.0)));
        let (tp, out) = run(&[2.0]);
        assert_eq!((tp.taps(), out[1]), (1, None), "2 s restarts: the display goes back to TAP");
        // Average 0.5 s: 1.47 x counts, 1.5 x restarts.
        let (tp, out) = run(&[0.5, 0.5, 0.735]);
        assert_eq!((tp.taps(), out[3]), (4, Some(104.0)));
        let (tp, out) = run(&[0.5, 0.5, 0.75]);
        assert_eq!((tp.taps(), out[3]), (1, None));
        // Taps 1.5 s apart: 2.1 s still counts.
        let (tp, _) = run(&[1.5, 1.5, 2.1]);
        assert_eq!(tp.taps(), 4);
        // A restarted count is a new one: its first tap is tap 1, its own 2 s rule applies.
        let (tp, out) = run(&[0.5, 0.5, 5.0, 0.6]);
        assert_eq!((tp.taps(), out[4]), (2, Some(100.0)));
        let mut tp = Tapper::default();
        tp.tap(10.0);
        assert_eq!((tp.tap(10.5), tp.tap(11.0)), (Some(120.0), Some(120.0)));
    }

    /// spec §3: the fill ends at about 8.4 n + 8.6 (a stub after the first tap), full from the 16th tap; the accent
    /// colour on taps 1, 5, 9, 13, 17.
    #[test]
    fn beat_bar_fill_and_accent() {
        let mut tp = Tapper::default();
        assert_eq!(tp.bar(), None);
        let mut bars = Vec::new();
        for i in 0..20 {
            tp.tap(f64::from(i) * 0.5);
            bars.push(tp.bar().unwrap());
        }
        assert!((bars[0].0 - 17.0).abs() < 1e-4 && bars[0].1);
        assert!((bars[1].0 - 25.4).abs() < 1e-4 && !bars[1].1);
        assert!((bars[14].0 - 134.6).abs() < 1e-3);
        assert_eq!(bars[15].0, BAR_X1);
        assert_eq!(bars[19].0, BAR_X1);
        let accents: Vec<usize> = (0..20).filter(|&i| bars[i].1).map(|i| i + 1).collect();
        assert_eq!(accents, [1, 5, 9, 13, 17]);
        // A new count starts the bar again.
        tp.tap(100.0);
        assert_eq!(tp.bar(), Some((17.0, true)));
    }

    /// spec §8: the flash fades over 250 ms, then goes out.
    #[test]
    fn flash_fades() {
        let mut ui = TapperUi::default();
        assert!(!ui.fade(Instant::now()));
        let t0 = Instant::now();
        ui.flash_at = Some(t0);
        ui.flash = 1.0;
        assert!(ui.fade(t0 + std::time::Duration::from_millis(125)));
        assert!((ui.flash - 0.5).abs() < 0.01);
        assert!(ui.fade(t0 + std::time::Duration::from_millis(260)));
        assert_eq!((ui.flash, ui.flash_at), (0.0, None));
        assert!(!ui.fade(t0 + std::time::Duration::from_millis(300)));
    }

    #[test]
    fn settings_file_round_trips_and_defaults_to_apply() {
        assert_eq!(parse(""), Settings { apply: true, sync: false, mute: false });
        assert_eq!(parse("junk\nother=1\n"), Settings::default());
        let s = parse("apply=0\nsync=1\nmute=1\nx=2\n");
        assert_eq!(s, Settings { apply: false, sync: true, mute: true });
        assert_eq!(text(&s), "apply=0\nsync=1\nmute=1\n");
        assert_eq!(parse(&text(&Settings::default())), Settings::default());
    }
}
