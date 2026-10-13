//! What a hosted instance costs and plays, published by the audio thread and read by the UI without locking the
//! instance: processing time (the Plugin performance monitor) and voices (the polyphony counter).
// spec: ui/plugin-performance.md §3, audio/voices.md §5

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

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
