// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The audio log: what went wrong with playback, and what was true when it did, so a dropout heard once can be
//! explained afterwards. `Logs/audio.log` in the user folder, one line per event:
//! - a playback starting (the audio settings it plays with);
//! - an audio thread's real-time priority: given, refused (and why), or taken back by the safety cap (`crate::rt`);
//! - dropouts, at most one line a second: how many of each kind (an empty block: the feeder was late; a plugin late
//!   for its tick; the device's own underruns) and whether the feeder and the device callback were real-time then.
//!
//! Small by design: events only, never per block; at `CAP` bytes the file becomes `audio.log.1` (the one before is
//! dropped), so the log never takes more than twice that.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

/// The largest the log grows before it is rotated.
const CAP: u64 = 256 * 1024;

static LOCK: Mutex<()> = Mutex::new(());

fn path() -> Option<PathBuf> {
    #[cfg(test)]
    return TEST_DIR.with(|d| d.borrow().clone()).map(|d| d.join("audio.log"));
    #[cfg(not(test))]
    Some(os_paths::user_dir()?.join("Logs").join("audio.log"))
}

#[cfg(test)]
thread_local! {
    static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub fn set_test_dir(d: Option<PathBuf>) {
    TEST_DIR.with(|t| *t.borrow_mut() = d);
}

/// Appends `line` with the local time. Not from the audio callback (it touches the disk).
pub fn write(line: &str) {
    let Some(p) = path() else { return };
    let _g = LOCK.lock();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::metadata(&p).is_ok_and(|m| m.len() >= CAP) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {line}", now());
    }
}

/// The local time, `YYYY-MM-DD HH:MM:SS`.
fn now() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r fills the struct given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// Dropouts by kind, as the transport counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drops {
    pub empty: u64,
    pub plugin: u64,
    pub device: u64,
}

impl Drops {
    pub fn total(&self) -> u64 {
        self.empty + self.plugin + self.device
    }
}

/// The dropouts since the last line, and what the threads were: written when there are any, at most once a second.
#[derive(Default)]
pub struct Watch {
    seen: Drops,
    demoted: usize,
    last: Option<std::time::Instant>,
}

impl Watch {
    /// The transport's counts now (`now`) and the real-time state of the audio threads (`rt`: feeder, callback; `None`
    /// when unknown): a line when something new happened and a second has passed since the last.
    pub fn check(&mut self, now: Drops, demoted: usize, rt: (Option<bool>, Option<bool>), context: &str) -> Option<String> {
        let new = Drops { empty: now.empty.saturating_sub(self.seen.empty), plugin: now.plugin.saturating_sub(self.seen.plugin), device: now.device.saturating_sub(self.seen.device) };
        let demotions = demoted.saturating_sub(self.demoted);
        if new.total() == 0 && demotions == 0 {
            return None;
        }
        if self.last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1)) {
            return None;
        }
        self.seen = now;
        self.demoted = demoted;
        self.last = Some(std::time::Instant::now());
        let state = |s: Option<bool>| match s {
            Some(true) => "real-time",
            Some(false) => "normal priority",
            None => "not running",
        };
        let mut line = String::new();
        if new.total() > 0 {
            line = format!("dropouts +{} ({} empty blocks: the feeder was late; {} plugins late; {} device underruns)", new.total(), new.empty, new.plugin, new.device);
        }
        if demotions > 0 {
            if !line.is_empty() {
                line.push_str("; ");
            }
            line.push_str(&format!("{demotions} audio threads taken back to normal priority by the safety cap"));
        }
        Some(format!("{line}; feeder {}, device callback {}; {context}", state(rt.0), state(rt.1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_per_burst_with_its_kinds() {
        let mut w = Watch::default();
        assert_eq!(w.check(Drops::default(), 0, (Some(true), Some(true)), "x"), None, "nothing new");
        let l = w.check(Drops { empty: 3, plugin: 1, device: 0 }, 0, (Some(true), Some(false)), "buffer 256").unwrap();
        assert!(l.starts_with("dropouts +4 (3 empty blocks") && l.contains("feeder real-time, device callback normal priority; buffer 256"), "{l}");
        assert_eq!(w.check(Drops { empty: 5, plugin: 1, device: 0 }, 0, (Some(true), Some(true)), "x"), None, "within the second");
        w.last = None;
        let l = w.check(Drops { empty: 5, plugin: 1, device: 2 }, 1, (Some(false), None), "x").unwrap();
        assert!(l.starts_with("dropouts +4 (2 empty") && l.contains("1 audio threads taken back") && l.contains("device callback not running"), "{l}");
    }

    #[test]
    fn the_log_rotates_at_its_cap() {
        let dir = std::env::temp_dir().join(format!("lemon-audio-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        set_test_dir(Some(dir.clone()));
        let long = "x".repeat(1000);
        for _ in 0..(CAP / 1000 + 5) {
            write(&long);
        }
        let main = std::fs::metadata(dir.join("audio.log")).unwrap().len();
        let old = std::fs::metadata(dir.join("audio.log.1")).unwrap().len();
        assert!(main < CAP && old >= CAP && old < CAP + 2000, "{main} {old}");
        set_test_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
