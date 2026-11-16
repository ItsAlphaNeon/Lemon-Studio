// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The dropout log: what went wrong with playback, and what was true when it did, so a dropout heard once can be
//! explained afterwards. `Logs/audio.log` in the user folder, one line per event:
//! - a playback starting (the audio settings it plays with);
//! - an audio thread's real-time priority: given, refused (and why), or taken back by the safety cap (`crate::rt`);
//! - dropouts, at most one line a second: how many of each kind (an empty block: the feeder was late; a plugin late
//!   for its tick; the device's own underruns) and whether the feeder and the device callback were real-time then;
//! - a hosted plugin held by the UI thread long enough to make the audio thread wait for it (`held`), at most one line
//!   a second, with how much of the hold the UI thread was running: the rest it waited for a CPU, preempted while it
//!   held the lock.
//!
//! Small by design: events only, never per block; at `CAP` bytes the file becomes `audio.log.1` (the one before is
//! dropped), so the log never takes more than twice that.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The largest the log grows before it is rotated.
const CAP: u64 = 256 * 1024;
/// The shortest hold of a plugin by the UI thread that is written: a block is 5.3 ms at 256 frames, and the feeder works
/// a few ms ahead.
const HELD_MIN: Duration = Duration::from_millis(2);

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

/// When the UI thread took a hosted plugin's lock (`held`): the time, and the thread's own running time then.
#[derive(Clone, Copy)]
pub struct Hold {
    at: Instant,
    cpu: Option<Duration>,
}

impl Hold {
    pub fn start() -> Hold {
        Hold { at: Instant::now(), cpu: thread_cpu() }
    }

    pub fn elapsed(&self) -> Duration {
        self.at.elapsed()
    }

    /// How long the thread ran since it began.
    fn ran(&self) -> Option<Duration> {
        Some(thread_cpu()?.saturating_sub(self.cpu?))
    }
}

/// The calling thread's own running time.
#[cfg(unix)]
fn thread_cpu() -> Option<Duration> {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime fills the struct given.
    (unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) } == 0).then(|| Duration::new(t.tv_sec as u64, t.tv_nsec as u32))
}

#[cfg(not(unix))]
fn thread_cpu() -> Option<Duration> {
    None
}

/// The UI thread held hosted plugin `plugin` since `hold` doing `what`. The audio thread waits for that lock and misses
/// the plugin's tick past its wait: a long hold is written.
pub fn held(plugin: &str, what: &str, hold: Hold) {
    let held = hold.elapsed();
    if held < HELD_MIN {
        return;
    }
    let ran = hold.ran();
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let line = match LAST.lock() {
        Ok(mut last) => held_line(&mut last, Instant::now(), plugin, what, held, ran),
        Err(_) => None,
    };
    if let Some(l) = line {
        write(&l);
    }
}

/// `held`'s line, unless a hold under `HELD_MIN`, or one was written less than a second before `now`.
fn held_line(last: &mut Option<Instant>, now: Instant, plugin: &str, what: &str, held: Duration, ran: Option<Duration>) -> Option<String> {
    if held < HELD_MIN || last.is_some_and(|t| now.duration_since(t) < Duration::from_secs(1)) {
        return None;
    }
    *last = Some(now);
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    Some(match ran {
        Some(r) => format!("{plugin}: held {:.1} ms by {what}, running {:.1} ms of it", ms(held), ms(r.min(held))),
        None => format!("{plugin}: held {:.1} ms by {what}", ms(held)),
    })
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

impl From<crate::transport::Drops> for Drops {
    fn from(d: crate::transport::Drops) -> Drops {
        Drops { empty: d.feeder, plugin: d.plugin, device: d.device }
    }
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
    fn a_long_hold_is_written_at_most_once_a_second() {
        let (mut last, t0) = (None, Instant::now());
        let ran = Some(Duration::from_micros(300));
        assert_eq!(held_line(&mut last, t0, "Zest", "the UI's tick", Duration::from_micros(1500), ran), None, "short");
        assert_eq!(
            held_line(&mut last, t0, "Zest", "the UI's tick", Duration::from_micros(4250), ran).as_deref(),
            Some("Zest: held 4.2 ms by the UI's tick, running 0.3 ms of it")
        );
        assert_eq!(held_line(&mut last, t0 + Duration::from_millis(500), "Zest", "x", Duration::from_millis(9), ran), None, "within the second");
        let l = held_line(&mut last, t0 + Duration::from_secs(1), "Zest", "x", Duration::from_millis(9), None);
        assert_eq!(l.as_deref(), Some("Zest: held 9.0 ms by x"), "no running time where the system gives none");
    }

    /// A thread that sleeps holds without running; one that works runs the whole time.
    #[cfg(unix)]
    #[test]
    fn a_hold_tells_running_from_waiting() {
        let h = Hold::start();
        std::thread::sleep(Duration::from_millis(20));
        assert!(h.ran().unwrap() < Duration::from_millis(5), "asleep: {:?}", h.ran());
        let h = Hold::start();
        while h.elapsed() < Duration::from_millis(20) {
            std::hint::black_box(0u64);
        }
        assert!(h.ran().unwrap() >= Duration::from_millis(10), "working: {:?}", h.ran());
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
