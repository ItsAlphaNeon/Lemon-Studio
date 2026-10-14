// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! A hosted instrument's voices as the host can know them: from the notes it sends and what the instrument reports
//! back. The plugin hosts keep one list per instance and give its count as `Instrument::voices`.
// spec: audio/voices.md §2

/// Most voices a list keeps (§2.7).
pub const MAX_VOICES: usize = 512;
/// How long a released voice lasts when the instrument does not report voice ends and states no tail (§2.5).
const DEFAULT_TAIL_SECONDS: f64 = 1.0;
/// The longest a released voice lasts without a report (§2.5), also for instruments that do report.
const MAX_TAIL_SECONDS: f64 = 30.0;

#[derive(Debug, Clone, Copy)]
struct Entry {
    key: u8,
    /// Note id the host gave (-1: none).
    id: i32,
    /// Seconds a released voice still lasts; `None` while its note is held.
    left: Option<f64>,
}

/// The voices of one instance, oldest first.
#[derive(Debug, Clone)]
pub struct VoiceLedger {
    voices: Vec<Entry>,
    /// The instrument has reported a voice end: its released voices last until reported (§2.4).
    reports: bool,
    /// The plugin's voice capacity, when it states one.
    capacity: Option<usize>,
    /// Seconds a released voice lasts while the instrument does not report ends.
    tail: f64,
}

impl Default for VoiceLedger {
    fn default() -> Self {
        VoiceLedger { voices: Vec::with_capacity(64), reports: false, capacity: None, tail: DEFAULT_TAIL_SECONDS }
    }
}

impl VoiceLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// The plugin's voice capacity (CLAP voice-info), if it states one.
    pub fn set_capacity(&mut self, capacity: Option<usize>) {
        self.capacity = capacity.filter(|&c| c > 0);
    }

    /// The tail the plugin states, in seconds (VST 3 tail samples / rate); none or 0 = the default second.
    pub fn set_tail(&mut self, seconds: Option<f64>) {
        self.tail = match seconds {
            Some(s) if s.is_finite() && s > 0.0 => s.min(MAX_TAIL_SECONDS),
            _ => DEFAULT_TAIL_SECONDS,
        };
    }

    /// The instrument reports its voice ends (a Lemon plugin, or a CLAP plugin after its first NOTE_END).
    pub fn reports(&self) -> bool {
        self.reports
    }

    /// A note on: a new voice.
    pub fn start(&mut self, key: u8, id: i32) {
        self.voices.push(Entry { key, id, left: None });
        let limit = self.capacity.unwrap_or(MAX_VOICES).min(MAX_VOICES);
        if self.voices.len() > limit {
            let extra = self.voices.len() - limit;
            self.voices.drain(..extra);
        }
    }

    fn release_left(&self) -> f64 {
        if self.reports {
            MAX_TAIL_SECONDS
        } else {
            self.tail
        }
    }

    /// A note off: the held voices of `key` ring out.
    pub fn release(&mut self, key: u8) {
        let left = self.release_left();
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.left.is_none()) {
            v.left = Some(left);
        }
    }

    /// A choke: the voices of `key` close quickly. An instrument that reports ends keeps them until it does.
    pub fn choke(&mut self, key: u8) {
        if self.reports {
            self.release(key);
        } else {
            self.voices.retain(|v| v.key != key);
        }
    }

    /// The instrument reports that a voice ended: by note id when the host gave one, else the oldest voice of the key,
    /// released before held (§2.4). A report that matches nothing is ignored.
    pub fn ended(&mut self, key: i16, id: i32) {
        if !self.reports {
            // From the first report on, released voices wait for theirs.
            self.reports = true;
            for v in self.voices.iter_mut().filter(|v| v.left.is_some()) {
                v.left = Some(MAX_TAIL_SECONDS);
            }
        }
        let by_id = (id >= 0).then(|| self.voices.iter().position(|v| v.id == id)).flatten();
        let at = by_id.or_else(|| {
            let key = u8::try_from(key).ok()?;
            self.voices.iter().position(|v| v.key == key && v.left.is_some()).or_else(|| self.voices.iter().position(|v| v.key == key))
        });
        if let Some(i) = at {
            self.voices.remove(i);
        }
    }

    /// `seconds` of audio went by: released voices that have outlasted their time end.
    pub fn advance(&mut self, seconds: f64) {
        if self.voices.iter().all(|v| v.left.is_none()) {
            return;
        }
        for v in &mut self.voices {
            if let Some(l) = &mut v.left {
                *l -= seconds;
            }
        }
        self.voices.retain(|v| v.left.is_none_or(|l| l > 0.0));
    }

    /// The instrument says it has nothing more to play: no voices left (`all`), or only the held ones (§2.6).
    pub fn quiet(&mut self, all: bool) {
        if all {
            self.voices.clear();
        } else {
            self.voices.retain(|v| v.left.is_none());
        }
    }

    /// Reset (Stop): no voices.
    pub fn clear(&mut self) {
        self.voices.clear();
    }

    /// Keys of the voices whose notes are still held, oldest first (one per voice).
    pub fn held(&self) -> impl Iterator<Item = u8> + '_ {
        self.voices.iter().filter(|v| v.left.is_none()).map(|v| v.key)
    }

    /// Voices now, within the plugin's capacity.
    pub fn count(&self) -> usize {
        self.voices.len().min(self.capacity.unwrap_or(MAX_VOICES))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_voices_ring_for_the_tail_without_reports() {
        let mut l = VoiceLedger::new();
        l.start(60, -1);
        l.start(64, -1);
        assert_eq!(l.count(), 2);
        l.release(60);
        assert_eq!(l.count(), 2, "a released voice rings");
        l.advance(0.6);
        assert_eq!(l.count(), 2);
        l.advance(0.5);
        assert_eq!(l.count(), 1, "gone after the default second");
        assert_eq!(l.held().collect::<Vec<_>>(), [64]);
        l.set_tail(Some(0.25));
        l.release(64);
        l.advance(0.3);
        assert_eq!(l.count(), 0, "the stated tail");
        l.set_tail(Some(f64::INFINITY));
        l.start(1, -1);
        l.release(1);
        l.advance(1.5);
        assert_eq!(l.count(), 0, "an infinite tail falls back to the default");
    }

    #[test]
    fn reported_ends_are_exact() {
        let mut l = VoiceLedger::new();
        l.start(60, -1);
        l.start(60, -1);
        l.start(62, 7);
        l.release(60);
        l.ended(60, -1);
        assert!(l.reports());
        assert_eq!(l.count(), 2);
        // Released voices now wait for their reports, well past the default second.
        l.start(65, -1);
        l.release(65);
        l.advance(5.0);
        assert_eq!(l.count(), 3);
        l.ended(99, 7);
        assert_eq!(l.count(), 2, "matched by note id");
        l.ended(70, -1);
        assert_eq!(l.count(), 2, "nothing matches: ignored");
        l.ended(60, -1);
        l.ended(65, -1);
        assert_eq!(l.count(), 0);
    }

    #[test]
    fn chokes_quiet_limits_and_reset() {
        let mut l = VoiceLedger::new();
        l.start(60, -1);
        l.start(61, -1);
        l.choke(60);
        assert_eq!(l.count(), 1, "a choke ends at once without reports");
        l.release(61);
        l.start(62, -1);
        l.quiet(false);
        assert_eq!(l.held().collect::<Vec<_>>(), [62], "quiet output ends the released voices only");
        l.quiet(true);
        assert_eq!(l.count(), 0);
        l.set_capacity(Some(4));
        for k in 0..10 {
            l.start(k, -1);
        }
        assert_eq!(l.count(), 4);
        assert_eq!(l.held().collect::<Vec<_>>(), [6, 7, 8, 9], "the oldest leave first");
        l.set_capacity(None);
        for k in 0..600u16 {
            l.start((k % 128) as u8, -1);
        }
        assert_eq!(l.count(), MAX_VOICES);
        l.clear();
        assert_eq!(l.count(), 0);
    }
}
