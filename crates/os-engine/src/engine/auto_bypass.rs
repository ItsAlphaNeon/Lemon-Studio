// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Auto-bypass when silent, the original's "Smart disable" (spec: audio/auto-bypass.md): a mixer slot whose effect has
//! heard and put out silence for its hold time stops running the effect, and runs it again as soon as sound reaches it.
//! The switch is per slot (`os_model::MixerTrack::slot_cpu`, `PluginCpu::smart_disable`) and, over all of them, the program's Audio ▸ CPU setting.
//! How the original tells silence was not measured: the level and the hold times are Lemon Studio's.

/// The program-wide switch (Audio settings ▸ CPU ▸ Auto-bypass when silent), on by default.
#[cfg(not(test))]
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

#[cfg(test)]
thread_local! {
    /// The switch, one per test thread: tests run side by side, and one turning it off turned it off for the engines
    /// of the others.
    static ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Turns auto-bypass on or off for every slot whose own switch is on (the program's setting).
#[cfg(not(test))]
pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The program's setting.
#[cfg(not(test))]
pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Turns auto-bypass on or off for this test thread.
#[cfg(test)]
pub fn set_enabled(on: bool) {
    ENABLED.with(|e| e.set(on));
}

/// This test thread's setting.
#[cfg(test)]
pub fn enabled() -> bool {
    ENABLED.with(|e| e.get())
}

/// A block whose samples all stay at or below this level counts as silent: -100 dBFS.
pub const SILENT: f32 = 1e-5;

/// The hold of a hosted plugin's slot, in seconds: hosted plugins are not asked for their tails, so they get a long one.
pub const HOSTED_HOLD: f32 = 4.0;

/// Whether every sample of `buf` is silent.
pub fn silent(buf: &[f32]) -> bool {
    buf.iter().all(|x| x.abs() <= SILENT)
}

/// How long a slot's input and output have been silent, in frames.
#[derive(Debug, Clone, Copy, Default)]
pub struct Quiet {
    frames: u64,
}

impl Quiet {
    /// Whether the effect may be skipped this block: its input is silent (`quiet_in`), and its input and output have
    /// been for `hold` frames (`None`: never skipped).
    pub fn skip(&self, quiet_in: bool, hold: Option<u64>) -> bool {
        quiet_in && hold.is_some_and(|h| self.frames >= h)
    }

    /// A block of `n` frames ran: silent in and out (`quiet`), or not, which starts the count again.
    pub fn note(&mut self, quiet: bool, n: usize) {
        self.frames = if quiet { self.frames.saturating_add(n as u64) } else { 0 };
    }

    /// Frames counted so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }
}

/// A hold of `seconds` (plus the effect's own delay `delay`, in frames) at `rate`, in frames.
pub fn hold_frames(seconds: f32, rate: u32, delay: u32) -> u64 {
    (f64::from(seconds.max(0.0)) * f64::from(rate)).round() as u64 + u64::from(delay)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Silence is counted until the hold; sound in or out starts it again, and sound in always runs the effect.
    #[test]
    fn the_hold_counts_silent_blocks() {
        let mut q = Quiet::default();
        let hold = Some(hold_frames(1.0, 1000, 24));
        assert_eq!(hold, Some(1024));
        for _ in 0..7 {
            assert!(!q.skip(true, hold));
            q.note(true, 128);
        }
        assert!(!q.skip(true, hold), "896 frames");
        q.note(true, 128);
        assert!(q.skip(true, hold), "1024 frames");
        assert!(!q.skip(false, hold), "sound in");
        assert!(!q.skip(true, None), "never for an effect without a hold");
        q.note(false, 128);
        assert_eq!(q.frames(), 0);
        assert!(silent(&[0.0, 1e-5, -1e-5]) && !silent(&[0.0, 2e-5]));
    }
}
