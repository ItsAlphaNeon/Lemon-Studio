// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! What a hosted instance costs and plays, published by the audio thread and read by the UI without locking the
//! instance: processing time (the Plugin performance monitor) and voices (the polyphony counter).
// spec: ui/plugin-performance.md §3, audio/voices.md §5

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// A count an instance raises when it has work for the UI thread (callbacks it asked for, parameter events for the
/// app, a flush, a restart, a new rate). The UI locks an instance only when its count moved since it last looked (or
/// its editor is open, or a while has passed), so it rarely holds an instance the audio thread is about to render
/// (spec: audio/implementing.md "Playing live"). Raised from any thread; never blocks.
#[derive(Debug, Default)]
pub struct Attention(AtomicU64);

impl Attention {
    pub fn raise(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }

    /// The count so far: a reader that saw a lower one has work waiting.
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Debug, Default)]
pub struct Meter {
    /// Nanoseconds spent in the instance's processing calls since it was created.
    busy: AtomicU64,
    /// Its voices after the last processed block (instruments).
    voices: AtomicU32,
}

impl Meter {
    /// Adds the time of one processing call.
    pub fn add(&self, d: Duration) {
        self.busy.fetch_add(d.as_nanos().min(u128::from(u64::MAX)) as u64, Ordering::Relaxed);
    }

    pub fn set_voices(&self, n: usize) {
        self.voices.store(n.min(u32::MAX as usize) as u32, Ordering::Relaxed);
    }

    /// Processing time so far, nanoseconds (only ever grows).
    pub fn busy_ns(&self) -> u64 {
        self.busy.load(Ordering::Relaxed)
    }

    pub fn voices(&self) -> usize {
        self.voices.load(Ordering::Relaxed) as usize
    }
}
