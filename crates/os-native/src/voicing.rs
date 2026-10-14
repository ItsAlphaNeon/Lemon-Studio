// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The channel's envelopes, LFOs and filter on a native generator's voices (spec: plugins/fruity-abi.md "Voices", the
//! channel's envelopes, LFOs and filter, an OpenStudio design): the host steps the five envelopes and LFOs once per
//! tick into each voice's final levels; a plugin that renders voice by voice (UseSampler) also has each voice run
//! through the channel filter on the host's side.
// spec: plugins/fruity-abi.md, audio/envelopes-and-lfos.md, audio/channel-filter.md

use os_dsp::articulator::{Parts, Record, Shared, Timing, Voice as Articulators};
use os_dsp::channel_filter::{self, Filter};
use os_engine::instrument::{ChannelLevels, ChannelVoicing};

use crate::abi::LevelParams;

/// A channel's envelopes, LFOs and filter at the generator's tempo, and the shared phases of its global LFOs.
pub struct Shaping {
    voicing: ChannelVoicing,
    timing: [Timing; 5],
    shared: Shared,
    tempo: f64,
}

/// What the host keeps of one voice: its envelopes and LFOs, their parts, and its filter.
pub struct Shape {
    art: Articulators,
    parts: Parts,
    filter: Option<Filter>,
}

impl Shaping {
    pub fn new(voicing: ChannelVoicing, tempo: f64) -> Shaping {
        let timing = Timing::all(&voicing.records, voicing.ppq.max(1), tempo);
        Shaping { voicing, timing, shared: Shared::default(), tempo }
    }

    /// The settings the engine gives with every tick; the times are worked out again when they changed.
    pub fn set(&mut self, voicing: &ChannelVoicing) {
        if *voicing != self.voicing {
            self.voicing = *voicing;
            self.retime();
        }
    }

    /// A new tempo: times that do not follow the tempo are worked out again; voices carry on from where they are.
    pub fn set_tempo(&mut self, tempo: f64) {
        if tempo > 0.0 && tempo != self.tempo {
            self.tempo = tempo;
            self.retime();
        }
    }

    fn retime(&mut self) {
        self.timing = Timing::all(&self.voicing.records, self.voicing.ppq.max(1), self.tempo);
    }

    /// Frames per tick at `rate`.
    pub fn frames_per_tick(&self, rate: u32) -> f64 {
        (f64::from(rate) * 60.0 / self.tempo.max(1.0) / f64::from(self.voicing.ppq.max(1))).max(1.0)
    }

    /// A tick starts, before any voice steps: the global LFOs' phase moves on.
    pub fn tick(&mut self) {
        self.shared.tick(&self.timing);
    }

    /// A note starts a voice with own levels `init` (the plugin's first note starts the global LFOs' phase; an
    /// OpenStudio choice: a note the plugin makes no voice for counts too). `filter`: the rate when the host runs the
    /// voice through the channel filter. The voice takes its first tick at once.
    pub fn start(&mut self, init: &LevelParams, channel: &ChannelLevels, filter: Option<u32>, nominal: f64) -> Shape {
        self.shared.note();
        let art = Articulators::start(&self.voicing.records, &self.timing);
        let mut shape = Shape { art, parts: Parts::NONE, filter: filter.map(|rate| Filter::new(self.voicing.filter_type, rate)) };
        self.step(&mut shape, init, channel, nominal);
        shape
    }

    /// One tick of a voice: its envelopes and LFOs step, and its filter takes the tick's knob positions (spec:
    /// audio/channel-filter.md "Knob positions": the stored knob and the channel's offset as one single, the note's own
    /// mod X / Y, then the envelope's and LFO's part). `nominal`: the tick's length in frames.
    pub fn step(&self, s: &mut Shape, init: &LevelParams, channel: &ChannelLevels, nominal: f64) {
        let records = &self.voicing.records;
        s.art.step(records, &self.timing, &self.shared.phases(records));
        s.parts = s.art.parts(records);
        if let Some(f) = &mut s.filter {
            let v = &self.voicing;
            let knob = |stored: i32, offset: f32| stored as f32 / 256.0 + offset;
            let x = channel_filter::position(init.fcut, knob(v.cutoff, channel.mod_x), s.parts.mod_x);
            let y = channel_filter::position(init.fres, knob(v.resonance, channel.mod_y), s.parts.mod_y);
            f.tick(v.filter_type, x, y, nominal);
        }
    }

    /// The voice's note is released: its envelopes that are on go into their release.
    pub fn release(&self, s: &mut Shape) {
        s.art.release(&self.voicing.records, &self.timing);
    }

    /// The volume envelope is on: a released note's voice plays on until its release has ended.
    pub fn volume_on(&self) -> bool {
        self.voicing.records[Record::VOLUME].on
    }

    /// The voice's volume envelope is on and has run out.
    pub fn volume_ended(&self, s: &Shape) -> bool {
        s.art.volume_ended(&self.voicing.records)
    }

    /// A new render or playback (Stop): the global LFOs wait for the next first note.
    pub fn reset(&mut self) {
        self.shared.reset();
    }
}

impl Shape {
    /// Final levels `fin` with the parts worked in (spec: audio/envelopes-and-lfos.md "What a voice gets from them"):
    /// the volume times the volume part (and type 5's correction, a Lemon Studio choice: it goes on the voice's level,
    /// which the plugin ramps), pan, pitch and mod X / Y plus theirs.
    pub fn levels(&self, fin: LevelParams) -> LevelParams {
        let p = self.parts;
        let gain = self.filter.as_ref().map_or(1.0, Filter::gain);
        LevelParams {
            vol: fin.vol * p.volume * gain,
            pan: fin.pan + p.pan,
            pitch: fin.pitch + p.pitch,
            fcut: fin.fcut + p.mod_x,
            fres: fin.fres + p.mod_y,
        }
    }

    /// Runs the voice's rendered frames (interleaved stereo) through its filter, when it has one.
    pub fn filter(&mut self, buf: &mut [f32]) {
        if let Some(f) = &mut self.filter {
            f.run(buf);
        }
    }

    pub fn parts(&self) -> Parts {
        self.parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init() -> LevelParams {
        LevelParams { pan: 0.0, vol: 1.0, pitch: 0.0, fcut: 0.0, fres: 0.0 }
    }

    /// A new channel's settings leave a voice's levels and sound as they are.
    #[test]
    fn a_new_channel_changes_nothing() {
        let mut sh = Shaping::new(ChannelVoicing::default(), 130.0);
        let (i, c) = (init(), ChannelLevels::default());
        let mut s = sh.start(&i, &c, Some(44100), 100.0);
        let fin = LevelParams { pan: 0.25, vol: 0.5, pitch: 300.0, fcut: 0.1, fres: -0.2 };
        let mut buf: Vec<f32> = (0..200).map(|k| (k as f32 * 0.37).sin()).collect();
        let before = buf.clone();
        for _ in 0..30 {
            sh.tick();
            sh.step(&mut s, &i, &c, 100.0);
            let l = s.levels(fin);
            assert_eq!((l.pan, l.vol, l.pitch, l.fcut, l.fres), (fin.pan, fin.vol, fin.pitch, fin.fcut, fin.fres));
            s.filter(&mut buf);
        }
        assert_eq!(buf, before, "the open filter passes the voice unfiltered");
    }

    /// With the volume envelope on, the volume follows it, and after the release it runs out; a pitch envelope adds
    /// its amount in cents; a mod X LFO moves the cutoff.
    #[test]
    fn envelopes_reach_the_levels() {
        let mut v = ChannelVoicing::default();
        let vol = &mut v.records[Record::VOLUME];
        (vol.on, vol.attack, vol.release) = (true, 6000, 6000);
        let pitch = &mut v.records[Record::PITCH];
        (pitch.on, pitch.attack, pitch.amount) = (true, 100, 700);
        let mut sh = Shaping::new(v, 120.0);
        let (i, c) = (init(), ChannelLevels::default());
        let mut s = sh.start(&i, &c, None, 100.0);
        let first = s.levels(i);
        assert!(first.vol > 0.0 && first.vol < 0.5, "the attack starts low: {}", first.vol);
        assert_eq!(first.pitch, 700.0);
        for _ in 0..200 {
            sh.tick();
            sh.step(&mut s, &i, &c, 100.0);
        }
        assert!(s.levels(i).vol > 0.3, "the decay is under way: {}", s.levels(i).vol);
        assert!(!sh.volume_ended(&s));
        sh.release(&mut s);
        let mut n = 0;
        while !sh.volume_ended(&s) {
            sh.tick();
            sh.step(&mut s, &i, &c, 100.0);
            n += 1;
            assert!(n < 10_000);
        }
        assert!(n > 5, "the release takes its time: {n}");
        assert_eq!(s.levels(i).vol, 0.0);
    }

    /// The filter's knob positions: the stored cutoff with the channel's offset, the note's mod X and the part. A
    /// closed cutoff takes the highs out of a voice.
    #[test]
    fn the_filter_follows_the_knobs_and_the_note() {
        let v = ChannelVoicing { cutoff: 64, ..ChannelVoicing::default() };
        let (i, c) = (init(), ChannelLevels::default());
        let mut sh = Shaping::new(v, 120.0);
        let mut s = sh.start(&i, &c, Some(44100), 100.0);
        let mut buf: Vec<f32> = (0..400).flat_map(|k| { let x = if k % 2 == 0 { 0.5 } else { -0.5 }; [x, x] }).collect();
        s.filter(&mut buf);
        assert!(buf[700].abs() < 0.05, "Nyquist is gone: {}", buf[700]);
        // The note's own mod X and the channel's offset open it again (0.25 + (64 / 256 + 0.5) = 1).
        let open = ChannelLevels { mod_x: 0.5, ..c };
        let note = LevelParams { fcut: 0.25, ..i };
        let mut s = sh.start(&note, &open, Some(44100), 100.0);
        let mut buf: Vec<f32> = (0..400).flat_map(|k| { let x = if k % 2 == 0 { 0.5 } else { -0.5 }; [x, x] }).collect();
        let before = buf.clone();
        s.filter(&mut buf);
        assert_eq!(buf, before);
    }
}
