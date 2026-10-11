//! The sequencer's tick clock: song positions in ticks become sample positions through a running sum of the
//! samples per tick, so every tick lasts a whole number of samples that varies by one from tick to tick.
//!
// spec: audio/timing.md ("Samples per tick", "The tick clock", "Tempo that changes")

/// Samples per tick at `bpm`: the tempo is held as a single, the division is done in doubles.
pub fn samples_per_tick(rate: u32, ppq: u16, bpm: f64) -> f64 {
    f64::from(rate) * 60.0 / (f64::from(bpm as f32) * f64::from(ppq.max(1)))
}

/// The running tick clock of one pass through a song or pattern.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    spt: f64,
    /// The running sum `T`: where the current tick ends, before rounding.
    sum: f64,
    /// First sample of the current tick and the first sample after it.
    start: u64,
    end: u64,
    /// Ticks started so far; the current tick is `ticks - 1`.
    ticks: u64,
}

impl Clock {
    /// A clock before its first tick.
    pub fn new(rate: u32, ppq: u16, bpm: f64) -> Self {
        Clock { spt: samples_per_tick(rate, ppq, bpm), sum: 0.0, start: 0, end: 0, ticks: 0 }
    }

    /// Starts the next tick and returns its first sample and its length (at least 1). Its length is settled with the
    /// tempo in force; a tempo set afterwards sizes the ticks after it.
    pub fn advance(&mut self) -> (u64, usize) {
        self.start = self.end;
        self.sum += self.spt;
        self.end = (self.sum.round_ties_even().max(0.0) as u64).max(self.start + 1);
        self.ticks += 1;
        (self.start, (self.end - self.start) as usize)
    }

    /// The tempo for the ticks that start after this call.
    pub fn set_tempo(&mut self, rate: u32, ppq: u16, bpm: f64) {
        self.spt = samples_per_tick(rate, ppq, bpm);
    }

    /// Back to before the first tick (a loop starting again).
    pub fn restart(&mut self) {
        (self.sum, self.start, self.end, self.ticks) = (0.0, 0, 0, 0);
    }

    pub fn samples_per_tick(&self) -> f64 {
        self.spt
    }

    /// The tick's nominal length: `Round(samples per tick)`, at least 1 (gain ramps divide by it).
    pub fn nominal(&self) -> usize {
        (self.spt.round_ties_even() as usize).max(1)
    }

    /// Ticks started so far (the index of the next tick).
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// First sample of the next tick.
    pub fn next_start(&self) -> u64 {
        self.end
    }

    /// First sample of the current tick.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// A clock that has started `n` ticks at a constant tempo (the state after playing up to tick `n`).
    pub fn at_tick(rate: u32, ppq: u16, bpm: f64, n: u64) -> Self {
        let mut c = Clock::new(rate, ppq, bpm);
        for _ in 0..n {
            c.advance();
        }
        c
    }

    /// The sample at which tick `n` starts at a constant tempo.
    pub fn frame_of_tick(rate: u32, ppq: u16, bpm: f64, n: u64) -> u64 {
        Clock::at_tick(rate, ppq, bpm, n).end
    }

    /// The tick in which sample `frame` lies at a constant tempo, and that tick's first sample.
    pub fn tick_of_frame(rate: u32, ppq: u16, bpm: f64, frame: u64) -> (u64, u64) {
        let mut c = Clock::new(rate, ppq, bpm);
        loop {
            let (start, len) = c.advance();
            if frame < start + len as u64 {
                return (c.ticks - 1, start);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// spec: audio/timing.md: at 185 BPM, 96 PPQ and 44100 Hz a tick is 148.986... samples, so ticks are 148 or 149
    /// samples long in the pattern the running sum gives.
    #[test]
    fn ticks_are_whole_samples_from_a_running_sum() {
        let mut c = Clock::new(44100, 96, 185.0);
        assert!((c.samples_per_tick() - 148.986).abs() < 1e-3);
        let mut sum = 0.0f64;
        let mut start = 0u64;
        for _ in 0..10_000 {
            let (s, len) = c.advance();
            sum += c.samples_per_tick();
            assert_eq!(s, start);
            assert!(len == 148 || len == 149, "{len}");
            start = sum.round_ties_even() as u64;
            assert_eq!(s + len as u64, start);
        }
    }

    /// The running sum, not `tick * samples per tick`: at 130 BPM, 96 PPQ and 44100 Hz tick 78 starts at
    /// 16537.4999... (sample 16537), where the product gives exactly 16537.5 (rounded to 16538).
    #[test]
    fn the_sum_decides_half_samples() {
        assert_eq!(Clock::frame_of_tick(44100, 96, 130.0, 78), 16537);
        assert_eq!(Clock::tick_of_frame(44100, 96, 130.0, 16537), (78, 16537));
        assert_eq!(Clock::tick_of_frame(44100, 96, 130.0, 16536).0, 77);
        assert_eq!(Clock::frame_of_tick(44100, 96, 130.0, 0), 0);
    }

    /// The tempo is a single: 123.456 BPM and its nearest single give the same clock.
    #[test]
    fn the_tempo_is_a_single() {
        let a = samples_per_tick(48000, 96, 123.456);
        let b = 48000.0 * 60.0 / (f64::from(123.456f32) * 96.0);
        assert_eq!(a, b);
        assert_ne!(a, 48000.0 * 60.0 / (123.456 * 96.0));
    }

    /// A new tempo sizes the ticks after the one in progress.
    #[test]
    fn tempo_changes_apply_from_the_next_tick() {
        let mut c = Clock::new(44100, 96, 120.0);
        let (_, a) = c.advance();
        c.set_tempo(44100, 96, 60.0);
        let (_, b) = c.advance();
        assert_eq!((a, b), (230, 459));
    }
}
