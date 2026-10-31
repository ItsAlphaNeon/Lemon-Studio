// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the controller plugins (Lemon Peak Controller, Lemon Formula Controller) share: the song's ticks as Lemon
//! Studio's engine cuts them, followed block by block, and their outputs sent as MIDI (output k on channel k, a pitch
//! bend with a CC 63 after it adding 7 finer bits: 21 bits in all), which Lemon Studio's links read.
//!
// spec: plugins/lemon-peak-controller.md §1, plugins/lemon-formula-controller.md §1

use nih_plug::prelude::*;
use os_engine::clock::Clock;

/// The tick the controllers work on: 1/96 beat.
pub const TICKS_PER_BEAT: f64 = 96.0;
/// Ticks per beat.
pub const PPQ: u16 = 96;
/// The CC that carries an output's 7 bits below its pitch bend.
pub const FINE_CC: u8 = 63;
/// The largest code of an output: 21 bits.
pub const TOP: u32 = (1 << 21) - 1;

/// An output value (0..1) as its 21-bit code: the pitch bend's 14 bits and the CC's 7.
pub fn code(v: f64) -> (u16, u8) {
    let n = (v.clamp(0.0, 1.0) * f64::from(TOP)).round() as u32;
    ((n >> 7) as u16, (n & 127) as u8)
}

/// Sends the outputs whose code changed since `sent` (`None`: not sent yet), at `timing`.
pub fn send_outputs<P: Plugin>(ctx: &mut impl ProcessContext<P>, sent: &mut [Option<(u16, u8)>], values: &[f64], timing: u32) {
    for (k, (&v, last)) in values.iter().zip(sent.iter_mut()).enumerate() {
        let c = code(v);
        if *last == Some(c) {
            continue;
        }
        *last = Some(c);
        let channel = k as u8;
        ctx.send_event(NoteEvent::MidiPitchBend { timing, channel, value: f32::from(c.0) / 16383.0 });
        ctx.send_event(NoteEvent::MidiCC { timing, channel, cc: FINE_CC, value: f32::from(c.1) / 127.0 });
    }
}

/// The song's ticks as the engine's clock cuts them (`os_engine::clock::Clock`, 96 per beat), followed block by
/// block: where each tick ends in a block. It keeps its own count of frames and goes to the host's position only when
/// that is somewhere else (playback starting, a jump, a loop).
#[derive(Debug, Clone)]
pub struct Ticks {
    clock: Clock,
    /// The song frame the next block starts at.
    pos: u64,
    rate: u32,
    tempo: f64,
}

impl Ticks {
    pub fn new(rate: u32, tempo: f64) -> Ticks {
        Ticks { clock: Clock::at_tick(rate, PPQ, tempo, 1), pos: 0, rate, tempo }
    }

    /// The rate and tempo of the coming ticks (a new rate starts the count again).
    pub fn set(&mut self, rate: u32, tempo: f64) {
        if rate != self.rate {
            *self = Ticks::new(rate, tempo);
        } else if tempo != self.tempo {
            self.tempo = tempo;
            self.clock.set_tempo(rate, PPQ, tempo);
        }
    }

    /// The host's position for the coming block, in ticks: taken when it is not where the count is, or always
    /// (`force`: playback starting).
    pub fn sync(&mut self, ticks: f64, force: bool) {
        let spt = self.clock.samples_per_tick();
        // Where the count is, in ticks: the current tick and the frames into it.
        let here = (self.clock.ticks() - 1) as f64 + (self.pos as f64 - self.clock.start() as f64) / spt;
        if !force && (ticks - here).abs() < 0.05 {
            return;
        }
        // Elsewhere: the clock at a constant tempo up to there.
        let j = (ticks + 1e-6).floor().max(0.0) as u64;
        self.clock = Clock::at_tick(self.rate, PPQ, self.tempo, j + 1);
        self.pos = self.clock.start() + ((ticks - j as f64) * spt).round().max(0.0) as u64;
    }

    /// The tick the next block starts in (from 0).
    pub fn tick(&self) -> u64 {
        self.clock.ticks() - 1
    }

    /// The offset of the next tick end in the coming block of `n` frames, if one is left in it.
    pub fn next_end(&mut self, n: usize) -> Option<usize> {
        let b = self.clock.next_start();
        (b <= self.pos + n as u64).then(|| {
            self.clock.advance();
            (b.max(self.pos + 1) - 1 - self.pos) as usize
        })
    }

    /// The block of `n` frames is done.
    pub fn done(&mut self, n: usize) {
        self.pos += n as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blocks cut on the engine's ticks (130 BPM at 44.1 kHz: 212 or 213 frames) end one tick each, on their last
    /// frame; other blocks end ticks where the song's ticks end.
    #[test]
    fn ticks_end_where_the_engines_do() {
        let ends = |t: &mut Ticks, n: usize| {
            let v: Vec<usize> = std::iter::from_fn(|| t.next_end(n)).collect();
            t.done(n);
            v
        };
        // The engine's blocks, with the host's position before each (tick 286 starts on a half frame).
        let mut engine = Clock::new(44100, PPQ, 130.0);
        let mut t = Ticks::new(44100, 130.0);
        for j in 0..2000u64 {
            let (_, n) = engine.advance();
            t.sync(j as f64, j == 0);
            assert_eq!(t.tick(), j);
            assert_eq!(ends(&mut t, n), vec![n - 1], "tick {j}");
        }
        // 512-frame blocks from the song's start: ticks 0 and 1 end in the first (at 211 and 423).
        let mut t = Ticks::new(44100, 130.0);
        assert_eq!(ends(&mut t, 512), vec![211, 423]);
        assert_eq!(ends(&mut t, 512), vec![123, 335]);
        // A jump: to tick 10 and 100 frames into it.
        let spt = os_engine::clock::samples_per_tick(44100, PPQ, 130.0);
        t.sync(10.0 + 100.0 / spt, false);
        assert_eq!(t.tick(), 10);
        let end10 = Clock::frame_of_tick(44100, PPQ, 130.0, 11) - Clock::frame_of_tick(44100, PPQ, 130.0, 10) - 100 - 1;
        assert_eq!(ends(&mut t, 200), vec![end10 as usize]);
    }

    /// The codes Lemon Studio's host reads back (os-clap's 21-bit value): the bend's 14 bits, then 7 finer ones.
    #[test]
    fn output_codes() {
        assert_eq!(code(0.0), (0, 0));
        assert_eq!(code(1.0), (16383, 127));
        assert_eq!(code(2.0), (16383, 127));
        let (b, f) = code(0.3);
        let back = (f64::from(b) * 128.0 + f64::from(f)) / f64::from(TOP);
        assert!((back - 0.3).abs() < 0.5 / f64::from(TOP));
    }
}
