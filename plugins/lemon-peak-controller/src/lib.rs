// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Peak Controller: Lemon Studio's peak follower and LFO controller (a recreation of FL's Fruity Peak Controller)
//! as a VST3 / CLAP plugin. It follows the peak of the sound it is given and runs an LFO, once per tick of the song
//! (1/96 beat), and puts out three controller values, Peak + LFO, Peak and LFO; the sound passes through, or is
//! muted. The values leave as MIDI: output k on channel k, as a pitch bend with the CC 63 after it adding 7 finer bits
//! (21 bits in all); Lemon Studio's links read them (`HostedEffect::control_fx`), other hosts can map them.
//!
//! The follower and the LFO are `os_engine::effects::peak_ctrl::Core`, which the engine also plays project slots
//! with; the settings, the parameters and the editor are `lemon_small_fx::peak_controller`.
//!
// spec: plugins/lemon-peak-controller.md, plugins/fruity-peak-controller.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

use std::num::NonZeroU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use lemon_small_fx::params::FxParams;
use lemon_small_fx::peak_controller::{self as pc, SPEC};
use lemon_small_fx::{editor, Shared, MAX};
use nih_plug::prelude::*;
use os_engine::clock::Clock;
use os_engine::effects::peak_ctrl::{Core, OUTPUTS};

/// The tick the controller works on: 1/96 beat.
pub const TICKS_PER_BEAT: f64 = 96.0;
/// The CC that carries an output's 7 bits below its pitch bend.
pub const FINE_CC: u8 = 63;
/// The largest code of an output: 21 bits.
const TOP: u32 = (1 << 21) - 1;

/// An output value (0..1) as its 21-bit code: the pitch bend's 14 bits and the CC's 7.
pub fn code(v: f64) -> (u16, u8) {
    let n = (v.clamp(0.0, 1.0) * f64::from(TOP)).round() as u32;
    ((n >> 7) as u16, (n & 127) as u8)
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
    /// (orce: playback starting).
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

/// Ticks per beat.
const PPQ: u16 = 96;

pub struct LemonPeakController {
    params: Arc<FxParams>,
    shared: Arc<Shared>,
    raw: Vec<u8>,
    raw_seen: u64,
    core: Core,
    /// The largest sample magnitude since the current tick began.
    peak: f64,
    /// The song's ticks (they run on while the song is stopped).
    ticks: Ticks,
    tempo: f64,
    playing: bool,
    /// The codes last sent per output (`None`: send at the next tick).
    sent: [Option<(u16, u8)>; OUTPUTS],
}

impl Default for LemonPeakController {
    fn default() -> Self {
        LemonPeakController {
            params: Arc::new(FxParams::new(SPEC)),
            shared: Arc::new(Shared::default()),
            raw: lemon_small_fx::params::default_state(SPEC),
            raw_seen: u64::MAX,
            core: Core::new(pc::settings(&SPEC.start())),
            peak: 0.0,
            ticks: Ticks::new(44100, 120.0),
            tempo: 120.0,
            playing: false,
            sent: [None; OUTPUTS],
        }
    }
}

impl LemonPeakController {
    /// The settings it plays (an editor edit the host has not confirmed yet wins; stored values out of a knob's range
    /// play as stored).
    fn settings(&mut self) -> [i32; MAX] {
        let v = self.params.raw_version.load(Ordering::Acquire);
        if v != self.raw_seen {
            if let Ok(b) = self.params.raw.try_read() {
                self.raw.clone_from(&b);
                self.raw_seen = v;
            }
        }
        self.params.settings(&self.raw)
    }

    /// Sends the outputs that changed since they were last sent, at `timing`.
    fn send(&mut self, ctx: &mut impl ProcessContext<Self>, timing: u32) {
        for (k, v) in self.core.outputs().into_iter().enumerate() {
            let c = code(v);
            if self.sent[k] == Some(c) {
                continue;
            }
            self.sent[k] = Some(c);
            let channel = k as u8;
            ctx.send_event(NoteEvent::MidiPitchBend { timing, channel, value: f32::from(c.0) / 16383.0 });
            ctx.send_event(NoteEvent::MidiCC { timing, channel, cc: FINE_CC, value: f32::from(c.1) / 127.0 });
        }
    }
}

impl Plugin for LemonPeakController {
    const NAME: &'static str = SPEC.name;
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] =
        &[AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() }];
    const MIDI_OUTPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::FxEditor::new(SPEC, self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.shared.rate.store(config.sample_rate.round() as u32, Ordering::Relaxed);
        self.raw_seen = u64::MAX;
        let s = self.settings();
        self.core = Core::new(pc::settings(&s));
        self.sent = [None; OUTPUTS];
        true
    }

    /// The song's start: the peak value 0, the LFO at its phase.
    fn reset(&mut self) {
        self.core.reset();
        self.peak = 0.0;
        self.sent = [None; OUTPUTS];
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let s = self.settings();
        let settings = pc::settings(&s);
        self.core.set(settings);
        let rate = self.shared.rate.load(Ordering::Relaxed).max(1);
        let t = ctx.transport();
        let playing = t.playing;
        self.tempo = t.tempo.filter(|t| *t > 0.0).unwrap_or(self.tempo);
        self.ticks.set(rate, self.tempo);
        let at = if playing { t.pos_beats().map(|b| b * TICKS_PER_BEAT) } else { None };
        // Playback starting: the LFO from its phase, the follower empty, as at the start of a song.
        let starting = playing && !self.playing;
        if starting {
            self.core.reset();
            self.peak = 0.0;
        }
        self.playing = playing;
        if let Some(at) = at {
            self.ticks.sync(at, starting);
        }
        let n = buffer.samples();
        if self.sent.iter().any(Option::is_none) {
            self.send(ctx, 0);
        }
        let out = buffer.as_slice();
        let level = |i: usize| out.iter().map(|ch| ch.get(i).map_or(0.0, |s| f64::from(s.abs()))).fold(0.0, f64::max);
        let mut from = 0;
        while let Some(e) = self.ticks.next_end(n) {
            let peak = (from..=e).map(level).fold(self.peak, f64::max);
            self.core.advance(peak, 1.0);
            self.peak = 0.0;
            from = e + 1;
            self.send(ctx, e as u32);
        }
        self.peak = (from..n).map(level).fold(self.peak, f64::max);
        self.ticks.done(n);
        if settings.mute {
            for ch in buffer.as_slice() {
                ch.fill(0.0);
            }
        }
        // The LFO runs on in silence.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for LemonPeakController {
    const CLAP_ID: &'static str = "org.openstudio.lemon-peak-controller";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's peak controller");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Analyzer, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonPeakController {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioPeakC";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Analyzer];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonPeakController);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonPeakController);
os_plugkit::linked_clap_entry!(LemonPeakController);

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
            assert_eq!(ends(&mut t, n), vec![n - 1], "tick {j}");
        }
        // 512-frame blocks from the song's start: ticks 0 and 1 end in the first (at 211 and 423).
        let mut t = Ticks::new(44100, 130.0);
        assert_eq!(ends(&mut t, 512), vec![211, 423]);
        assert_eq!(ends(&mut t, 512), vec![123, 335]);
        // A jump: to tick 10 and 100 frames into it.
        let spt = os_engine::clock::samples_per_tick(44100, PPQ, 130.0);
        t.sync(10.0 + 100.0 / spt, false);
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

    #[test]
    fn a_new_instance_is_the_originals_default() {
        let p = LemonPeakController::default();
        assert_eq!(p.core.s, os_engine::effects::peak_ctrl::Settings::default());
        assert_eq!(<LemonPeakController as Plugin>::NAME, "Lemon Peak Controller");
    }
}
