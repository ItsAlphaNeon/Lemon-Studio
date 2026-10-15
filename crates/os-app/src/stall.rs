// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Keeps the app ticking and painting while a modal message loop holds up winit's.
//!
//! Plugin menus, type-in boxes and file dialogs (Lemon plugins' and other plugins' alike) run a message loop of their
//! own on the UI thread until they close. winit's loop waits meanwhile, and `about_to_wait` and its wake-up time with
//! it, so nothing ticked or painted while such a menu was open: the playhead, meters and clock stood still. Those
//! loops still dispatch posted messages (a user event among them) and paint messages.
//!
//! Each tick tells the watchdog when the next one is due. When one is missed by `LATE`, the watchdog wakes the app
//! with a user event; `Shell::user_event` then ticks and asks for its frames itself, and the watchdog wakes it on time
//! (without `LATE`) until winit's loop runs again.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::Thread;
use std::time::{Duration, Instant};

/// How long past a tick's due time winit's loop counts as held up.
pub const LATE: Duration = Duration::from_millis(25);

/// Longest the watchdog waits before it looks at the due time again.
const POLL: Duration = Duration::from_millis(50);

/// No tick asked for.
const NONE: u64 = u64::MAX;

pub struct Watchdog {
    base: Instant,
    late: Duration,
    /// When the next tick is due, in nanoseconds after `base`.
    due: AtomicU64,
    /// The watchdog found a missed tick and woke the app.
    held: AtomicBool,
    /// The watchdog thread, woken when a due time is set after a wake (`next`).
    thread: OnceLock<Thread>,
}

impl Watchdog {
    pub fn new(late: Duration) -> Arc<Watchdog> {
        Arc::new(Watchdog { base: Instant::now(), late, due: AtomicU64::new(NONE), held: AtomicBool::new(false), thread: OnceLock::new() })
    }

    /// Starts the watchdog thread; `wake` wakes the event loop from any thread.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn start(self: &Arc<Self>, wake: fn()) {
        let dog = self.clone();
        let run = move || loop {
            let due = dog.due.load(Ordering::Acquire);
            let now = dog.nanos(Instant::now());
            if due == NONE {
                // Until the woken tick asks for the next one (`next` unparks). Parking is coarse (the system timer),
                // sleeping is not: due times are slept to.
                std::thread::park_timeout(POLL);
            } else if now < due {
                std::thread::sleep(Duration::from_nanos(due - now).min(POLL));
            } else if dog.due.compare_exchange(due, NONE, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                // Nobody moved the due time since: the tick was missed.
                dog.held.store(true, Ordering::Release);
                wake();
            }
        };
        if let Ok(t) = std::thread::Builder::new().name("ui watchdog".into()).spawn(run) {
            let _ = self.thread.set(t.thread().clone());
        }
    }

    /// A tick ran and asks for the next at `at`. A tick from winit's loop (`held` false) is missed only `LATE` after
    /// that; one from a wake of the watchdog's own (`held`) is woken exactly at `at`, since winit's loop may still be
    /// held up.
    pub fn next(&self, at: Instant, held: bool) {
        let at = if held { at } else { at + self.late };
        if self.due.swap(self.nanos(at), Ordering::AcqRel) == NONE {
            if let Some(t) = self.thread.get() {
                t.unpark();
            }
        }
    }

    /// Whether the watchdog found a missed tick and woke the app (true once per wake).
    pub fn take(&self) -> bool {
        self.held.swap(false, Ordering::AcqRel)
    }

    fn nanos(&self, t: Instant) -> u64 {
        t.saturating_duration_since(self.base).as_nanos().min(u128::from(NONE - 1)) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static WAKES: AtomicUsize = AtomicUsize::new(0);

    fn wake() {
        WAKES.fetch_add(1, Ordering::AcqRel);
    }

    #[test]
    fn wakes_only_when_a_tick_is_missed() {
        // A generous `late`, so a busy test machine does not miss a tick by accident.
        let dog = Watchdog::new(Duration::from_millis(400));
        dog.start(wake);
        // Ticks on time, every 5 ms (due 10 ms later): no wake.
        let end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < end {
            dog.next(Instant::now() + Duration::from_millis(10), false);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(WAKES.load(Ordering::Acquire), 0, "ticks on time never wake the app");
        assert!(!dog.take());
        // The ticks stop (a modal loop holds up winit's): one wake, `late` after the due time.
        let stopped = Instant::now();
        dog.next(stopped + Duration::from_millis(10), false);
        let deadline = stopped + Duration::from_secs(10);
        while WAKES.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(WAKES.load(Ordering::Acquire), 1, "a missed tick wakes the app");
        assert!(stopped.elapsed() >= Duration::from_millis(410), "not before the tick was late: {:?}", stopped.elapsed());
        assert!(dog.take(), "the wake says the loop is held up");
        assert!(!dog.take(), "once per wake");
        // Held up: the next tick is woken at its due time, without `late`.
        let held = Instant::now();
        dog.next(held + Duration::from_millis(10), true);
        let deadline = held + Duration::from_secs(10);
        while WAKES.load(Ordering::Acquire) == 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(WAKES.load(Ordering::Acquire), 2);
        assert!(held.elapsed() >= Duration::from_millis(10));
        assert!(dog.take());
        // No tick asked for since: no more wakes.
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(WAKES.load(Ordering::Acquire), 2, "one wake per missed tick");
    }
}
