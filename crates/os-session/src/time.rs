// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The clock the program reads (control-plane step 9: the session's; `crate::anim` re-exports it for the motions).
//! Tests run one of their own, or none.

use std::time::Instant;
#[cfg(any(test, feature = "test-util"))]
use std::time::Duration;

// Tests see motion only on a clock they run themselves (`set_clock`, `advance`); without one, every motion is at its
// end at once, so a frame painted twice is the same frame.
#[cfg(any(test, feature = "test-util"))]
thread_local! {
    static CLOCK: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// The time the program reads (the motions, the session's work time).
pub fn now() -> Instant {
    #[cfg(any(test, feature = "test-util"))]
    if let Some(t) = CLOCK.with(|c| c.get()) {
        return t;
    }
    Instant::now()
}

/// Tests that run no clock: nothing moves and nothing flashes, so painting the same state twice paints the same frame.
pub fn still() -> bool {
    #[cfg(any(test, feature = "test-util"))]
    return CLOCK.with(|c| c.get()).is_none();
    #[cfg(not(any(test, feature = "test-util")))]
    false
}

/// Tests: starts this thread's clock at the present (`true`) or stops it (`false`: motions end at once again).
#[cfg(any(test, feature = "test-util"))]
pub fn set_clock(on: bool) {
    CLOCK.with(|c| c.set(on.then(Instant::now)));
}

/// Tests: runs this thread's clock from `t` (back on after `set_clock(false)`, where it was).
#[cfg(any(test, feature = "test-util"))]
pub fn set_clock_at(t: Instant) {
    CLOCK.with(|c| c.set(Some(t)));
}

/// Tests: moves this thread's clock on by `ms`.
#[cfg(any(test, feature = "test-util"))]
pub fn advance(ms: u64) {
    CLOCK.with(|c| c.set(Some(c.get().unwrap_or_else(Instant::now) + Duration::from_millis(ms))));
}

