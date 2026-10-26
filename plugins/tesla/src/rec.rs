// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Recording: which input frames to keep, by the When mode and the For limit (Tesla's own rule until Edison's is
//! measured), and the lock-free ring that carries them from the audio thread to the thread that builds the take.
//!
// spec: plugins/tesla/README.md "Version 1" (recording), plugins/tesla/requests.md #4, plugins/edison/state.md

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::state::{self, Limit, WHEN_INPUT, WHEN_NOW, WHEN_ON_INPUT, WHEN_ON_PLAY};

/// The input "is present" while either channel is above −60 dBFS.
pub const THRESHOLD: f32 = 0.001;
/// Input mode keeps recording this long after the input falls below the threshold.
pub const HOLD_SECONDS: f64 = 0.5;

/// What the recorder decided for a block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Block {
    /// Frames kept from this block.
    pub kept: usize,
    /// The For limit was reached: recording stops and disarms.
    pub done: bool,
}

/// The state of one armed recording.
#[derive(Clone, Debug, Default)]
pub struct Recorder {
    /// On input: the input has been present once (recording is latched on).
    started: bool,
    /// Input: frames of hold left.
    hold: usize,
    /// Frames kept so far in this take.
    pub kept: u64,
}

impl Recorder {
    /// Starts over (a new arming).
    pub fn reset(&mut self) {
        *self = Recorder::default();
    }

    /// The most frames a take may hold at `rate` with maximum time index `max` (`u64::MAX` = forever). Size limits
    /// count the recorded audio as 32-bit float stereo.
    pub fn limit_frames(max: u8, rate: u32) -> u64 {
        match state::limit(max) {
            Limit::Forever => u64::MAX,
            Limit::Seconds(s) => (s * rate as f64) as u64,
            Limit::Bytes(b) => b / 8,
        }
    }

    /// Goes through a block of input (`l`, `r`), calling `keep(i)` for every frame to record, in order.
    pub fn block(&mut self, when: u8, playing: bool, rate: u32, max: u8, l: &[f32], r: &[f32], mut keep: impl FnMut(usize)) -> Block {
        let limit = Self::limit_frames(max, rate);
        let hold = (HOLD_SECONDS * rate as f64) as usize;
        let mut out = Block::default();
        for i in 0..l.len().min(r.len()) {
            let present = l[i].abs() > THRESHOLD || r[i].abs() > THRESHOLD;
            let take = match when {
                WHEN_NOW => true,
                WHEN_ON_INPUT => {
                    self.started |= present;
                    self.started
                }
                WHEN_INPUT => {
                    if present {
                        self.hold = hold;
                        true
                    } else if self.hold > 0 {
                        self.hold -= 1;
                        true
                    } else {
                        false
                    }
                }
                WHEN_ON_PLAY => playing,
                _ => false,
            };
            if !take {
                continue;
            }
            if self.kept >= limit {
                out.done = true;
                break;
            }
            keep(i);
            self.kept += 1;
            out.kept += 1;
        }
        if self.kept >= limit {
            out.done = true;
        }
        out
    }
}

/// A single-producer single-consumer ring of stereo frames (the audio thread writes, one other thread reads).
pub struct Ring {
    buf: Box<[std::cell::UnsafeCell<[f32; 2]>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY: one producer writes slots between tail and head, one consumer reads them; the atomics order the handover.
unsafe impl Sync for Ring {}
unsafe impl Send for Ring {}

impl Ring {
    pub fn new(frames: usize) -> Ring {
        let n = frames.next_power_of_two().max(2);
        Ring { buf: (0..n).map(|_| std::cell::UnsafeCell::new([0.0; 2])).collect(), head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len() - 1
    }

    /// Writes a frame; false when the ring is full (the frame is lost).
    pub fn push(&self, f: [f32; 2]) -> bool {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        let next = (h + 1) & (self.buf.len() - 1);
        if next == t {
            return false;
        }
        // SAFETY: only the producer writes slot `h`, which the consumer doesn't read until `head` moves past it.
        unsafe { *self.buf[h].get() = f };
        self.head.store(next, Ordering::Release);
        true
    }

    /// Reads every frame written so far into `l` and `r`; returns how many.
    pub fn drain(&self, l: &mut Vec<f32>, r: &mut Vec<f32>) -> usize {
        let h = self.head.load(Ordering::Acquire);
        let mut t = self.tail.load(Ordering::Relaxed);
        let mut n = 0;
        while t != h {
            // SAFETY: slots between tail and head were published by the producer.
            let f = unsafe { *self.buf[t].get() };
            l.push(f[0]);
            r.push(f[1]);
            t = (t + 1) & (self.buf.len() - 1);
            n += 1;
        }
        self.tail.store(t, Ordering::Release);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(r: &mut Recorder, when: u8, playing: bool, max: u8, l: &[f32]) -> (Vec<usize>, Block) {
        let mut v = Vec::new();
        let b = r.block(when, playing, 1000, max, l, &vec![0.0; l.len()], |i| v.push(i));
        (v, b)
    }

    #[test]
    fn when_modes() {
        let quiet_loud: Vec<f32> = (0..2000).map(|i| if (500..600).contains(&i) { 0.5 } else { 0.0 }).collect();
        // Now: everything.
        assert_eq!(run(&mut Recorder::default(), WHEN_NOW, false, 0, &quiet_loud).0.len(), 2000);
        // On input: from the first loud frame on, continuously.
        let (v, _) = run(&mut Recorder::default(), WHEN_ON_INPUT, false, 0, &quiet_loud);
        assert_eq!((v[0], v.len()), (500, 1500));
        // Input: the loud stretch plus a 0.5 s hold (500 frames at 1 kHz).
        let (v, _) = run(&mut Recorder::default(), WHEN_INPUT, false, 0, &quiet_loud);
        assert_eq!((v[0], *v.last().unwrap(), v.len()), (500, 1099, 600));
        // On play: while the host plays.
        assert!(run(&mut Recorder::default(), WHEN_ON_PLAY, false, 0, &quiet_loud).0.is_empty());
        assert_eq!(run(&mut Recorder::default(), WHEN_ON_PLAY, true, 0, &quiet_loud).0.len(), 2000);
        // −60 dBFS is the line.
        assert!(run(&mut Recorder::default(), WHEN_ON_INPUT, false, 0, &[0.001; 10]).0.is_empty());
    }

    #[test]
    fn the_limit_stops_recording() {
        // 1 minute at 1 kHz = 60,000 frames.
        let mut r = Recorder::default();
        let block = vec![0.0; 1000];
        for _ in 0..59 {
            assert!(!run(&mut r, WHEN_NOW, false, 1, &block).1.done);
        }
        let (v, b) = run(&mut r, WHEN_NOW, false, 1, &block);
        assert_eq!((v.len(), b.done), (1000, true));
        assert_eq!(run(&mut r, WHEN_NOW, false, 1, &block).0.len(), 0);
        // 50 MB of 32-bit float stereo.
        assert_eq!(Recorder::limit_frames(7, 44100), 50 * 1024 * 1024 / 8);
        assert_eq!(Recorder::limit_frames(0, 44100), u64::MAX);
    }

    #[test]
    fn ring_hands_frames_over() {
        let ring = Ring::new(4);
        assert_eq!(ring.capacity(), 3);
        assert!(ring.push([1.0, 2.0]) && ring.push([3.0, 4.0]) && ring.push([5.0, 6.0]));
        assert!(!ring.push([7.0, 8.0]));
        let (mut l, mut r) = (Vec::new(), Vec::new());
        assert_eq!(ring.drain(&mut l, &mut r), 3);
        assert_eq!((l, r), (vec![1.0, 3.0, 5.0], vec![2.0, 4.0, 6.0]));
        assert!(ring.push([9.0, 9.0]));
    }
}
