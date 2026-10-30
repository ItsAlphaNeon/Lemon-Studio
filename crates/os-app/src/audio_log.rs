// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The audio logger (TOOLS > Retro audio capture): while it is enabled, what the audio device plays (its first output pair,
//! playback, previews and live notes alike) is kept for the last few minutes; Open capture in audio editor puts it into Tesla
//! on the master track. The device's callback pushes each block into one lock-free ring (`push`); the app's tick
//! moves it into the log (`Log::tick`).
// spec: ui/audio-logger.md

use crate::app::{App, State};
use crate::capture::Ring;
use crate::playlist::{Cmd, Mark, PItem};
use crate::record::RecCmd;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::OnceLock;

/// Settings ▸'s choices: how many minutes the log keeps.
pub const MINUTES: [u32; 4] = [1, 2, 5, 10];
/// Kept in `settings.txt`: the switch (on by default, as in FL Studio) and the length.
const ON: &str = "audio_logger";
const LENGTH: &str = "audio_logger_minutes";
const DEFAULT_MINUTES: u32 = 5;
/// A sample at or below this is silence: a dump leaves it out at both ends.
const SILENT: f32 = 1e-5;

/// What the device's callback hands over: about 20 s of stereo at 48 kHz, drained on every tick of the app.
struct Feed {
    ring: Ring,
    on: AtomicBool,
}

fn feed() -> &'static Feed {
    static F: OnceLock<Feed> = OnceLock::new();
    F.get_or_init(|| Feed { ring: Ring::new(1 << 21), on: AtomicBool::new(false) })
}

/// The device's block as it plays it (`width` channels interleaved; the first pair is kept), while the logger is on.
/// The device's callback calls it; it neither locks nor allocates, and what finds no room is lost.
pub(crate) fn push(sum: &[f32], width: usize) {
    feed().push(sum, width);
}

impl Feed {
    fn push(&self, sum: &[f32], width: usize) {
        if !self.on.load(Ordering::Acquire) || width < 2 {
            return;
        }
        let frames = sum.len() / width;
        self.ring.push_with(2 * frames, |k| sum[k / 2 * width + k % 2]);
    }
}

pub fn enabled() -> bool {
    crate::settings_file::flag(ON, true)
}

/// Bytes the log holds (the menu's header shows them).
static HELD: AtomicUsize = AtomicUsize::new(0);

/// The memory the log takes, in whole MB (the TOOLS menu's "Retro audio capture (memory used: 20MB)").
pub fn memory_mb() -> usize {
    HELD.load(Ordering::Relaxed) / (1024 * 1024)
}

pub fn minutes() -> u32 {
    crate::settings_file::get(LENGTH).and_then(|v| v.parse().ok()).filter(|m| MINUTES.contains(m)).unwrap_or(DEFAULT_MINUTES)
}

/// TOOLS > Retro audio capture > Settings ▸.
pub fn items(_: &State) -> Vec<PItem> {
    let now = minutes();
    let mut items = vec![PItem::head("Keep the last")];
    items.extend(MINUTES.iter().map(|&m| {
        PItem::new(format!("{m} minute{}", if m == 1 { "" } else { "s" }), Cmd::Rec(RecCmd::AudioLog(m))).mark(Mark::Radio(m == now))
    }));
    items
}

/// The log: interleaved stereo at `rate`, oldest first.
#[derive(Debug, Default)]
pub struct Log {
    kept: VecDeque<f32>,
    rate: u32,
    incoming: Vec<f32>,
}

impl Log {
    /// Keeps what the device played since the last tick, at the device's rate; off, the log empties.
    pub fn tick(&mut self) {
        let f = feed();
        let on = enabled();
        f.on.store(on, Ordering::Release);
        if !on {
            f.ring.clear();
            if !self.kept.is_empty() || self.kept.capacity() > 0 {
                *self = Log::default();
            }
            HELD.store(0, Ordering::Relaxed);
            return;
        }
        let mut incoming = std::mem::take(&mut self.incoming);
        incoming.clear();
        f.ring.pop_into(&mut incoming);
        // At the rate of the device that played them, the one last opened (`device::rate` would ask the system for the
        // settings' device on every tick while none is open, which takes milliseconds).
        let rate = crate::device::last().map_or(self.rate, |i| i.rate);
        self.keep(&incoming, rate, minutes());
        self.incoming = incoming;
        HELD.store(self.kept.capacity() * std::mem::size_of::<f32>(), Ordering::Relaxed);
    }

    /// Adds `samples` played at `rate` and drops what is older than `minutes`; a new rate starts the log over.
    fn keep(&mut self, samples: &[f32], rate: u32, minutes: u32) {
        if rate != self.rate {
            self.kept.clear();
            self.rate = rate;
        }
        self.kept.extend(samples);
        let cap = 2 * 60 * minutes as usize * rate as usize;
        if self.kept.len() > cap {
            let over = self.kept.len() - cap;
            self.kept.drain(..over);
        }
    }

    /// The log's left and right channels, without the silence at either end.
    pub fn audio(&self) -> (Vec<f32>, Vec<f32>) {
        let frames = self.kept.len() / 2;
        let loud = |i: usize| self.kept[2 * i].abs() > SILENT || self.kept[2 * i + 1].abs() > SILENT;
        let Some(first) = (0..frames).find(|&i| loud(i)) else { return (Vec::new(), Vec::new()) };
        let last = (0..frames).rev().find(|&i| loud(i)).unwrap_or(first);
        (first..=last).map(|i| (self.kept[2 * i], self.kept[2 * i + 1])).unzip()
    }
}

impl App {
    /// TOOLS > Retro audio capture > Enabled.
    pub(crate) fn toggle_audio_log(&mut self) {
        let on = !enabled();
        crate::settings_file::set_flag(ON, on);
        self.audio_log.tick();
        self.state.message = Some(if on { format!("Retro audio capture on: the last {} minute(s)", minutes()) } else { "Retro audio capture off".into() });
    }

    pub(crate) fn set_audio_log_minutes(&mut self, m: u32) -> String {
        crate::settings_file::write(LENGTH, &m.to_string());
        format!("Retro audio capture: keeps the last {m} minute(s)")
    }

    /// TOOLS > Retro audio capture > Open capture in audio editor: the log (silence at either end left out) into a WAV in the
    /// `Recorded` folder, and into Tesla on the master track as One-click recording's takes go.
    pub(crate) fn dump_audio_log(&mut self) -> String {
        if !enabled() {
            return "Retro audio capture is off: TOOLS > Retro audio capture > Enabled".into();
        }
        let (l, r) = self.audio_log.audio();
        if l.is_empty() {
            return "The audio logger has heard nothing yet".into();
        }
        let sample = tesla::sample::Sample::from_channels(self.audio_log.rate.max(1), vec![l, r], tesla::sample::Format::Float32);
        let dir = crate::track_rec::folder(&self.state);
        let name = format!("Audio log_{}.wav", chrono::Local::now().format("%Y-%m-%d %H-%M-%S"));
        let path = dir.join(name);
        let wrote = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, tesla::wav::write(&sample, tesla::wav::Depth::Own)));
        match wrote {
            Ok(()) => self.take_to_editor(&path),
            Err(e) => format!("{} could not be written: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The log keeps the last minutes at the device's rate, starts over at a new rate, and a dump leaves the silence at
    /// either end out.
    #[test]
    fn keeps_the_last_minutes() {
        let mut log = Log::default();
        // 1 kHz: a minute is 60,000 frames.
        let second = |v: f32| vec![v; 2000];
        log.keep(&second(0.0), 1000, 1);
        for s in 1..=70 {
            log.keep(&second(s as f32 / 100.0), 1000, 1);
        }
        log.keep(&second(0.0), 1000, 1);
        assert_eq!(log.kept.len(), 2 * 60_000);
        let (l, r) = log.audio();
        assert_eq!(l.len(), 59_000, "the silent second at the end is left out");
        assert_eq!((l[0], r[0]), (0.12, 0.12), "the first 12 s are gone");
        // A new rate starts over.
        log.keep(&second(0.5), 2000, 1);
        assert_eq!(log.kept.len(), 2000);
        assert_eq!(Log::default().audio().0.len(), 0);
    }

    /// Open capture in audio editor: off, a message; on, the log into a WAV in `Recorded` and into Tesla on the master track.
    #[test]
    fn a_dump_goes_into_the_audio_editor() {
        use crate::mixer::FxReq;
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app.plugins.effects = crate::plugins::builtin_effects();
        crate::settings_file::set_flag(ON, false);
        assert!(app.dump_audio_log().contains("off"));
        crate::settings_file::set_flag(ON, true);
        assert!(app.dump_audio_log().contains("nothing"));
        let tone: Vec<f32> = (0..9600).map(|i| if i % 2 == 0 { (i / 2) as f32 / 4800.0 } else { 0.5 }).collect();
        app.audio_log.keep(&tone, 48000, 5);
        let msg = app.dump_audio_log();
        assert!(msg.starts_with("Recorded Audio log_") && msg.ends_with(" into the audio editor"), "{msg}");
        let Some(FxReq::Create { track: 0, state: Some(st), .. }) = app.state.mixer.pending.last().cloned() else {
            panic!("{:?}", app.state.mixer.pending)
        };
        let saved = tesla::state::from_plugin_state(&st).unwrap();
        assert_eq!((saved.sample.frames(), saved.sample.rate), (4800, 48000));
        let path = std::path::PathBuf::from(&saved.path);
        assert!(path.exists() && path.parent().unwrap().ends_with("Recorded"), "{path:?}");
        let _ = std::fs::remove_file(path);
    }

    /// The device's block gives its first output pair, only while the logger is on.
    #[test]
    fn the_device_block_gives_its_first_pair() {
        let f = Feed { ring: Ring::new(16), on: AtomicBool::new(false) };
        let mut v = Vec::new();
        // Four channels, two frames.
        let sum = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        f.push(&sum, 4);
        f.on.store(true, Ordering::Release);
        f.push(&sum, 4);
        f.ring.pop_into(&mut v);
        assert_eq!(v, [1.0, 2.0, 5.0, 6.0]);
    }
}
