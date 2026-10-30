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
use os_engine::effects::peak_ctrl::{Core, OUTPUTS};

/// The ticks and the MIDI outputs, shared with Lemon Formula Controller.
pub use lemon_small_fx::ticks::{code, Ticks, FINE_CC, PPQ, TICKS_PER_BEAT};

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
        lemon_small_fx::ticks::send_outputs(ctx, &mut self.sent, &self.core.outputs(), timing);
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

    #[test]
    fn a_new_instance_is_the_originals_default() {
        let p = LemonPeakController::default();
        assert_eq!(p.core.s, os_engine::effects::peak_ctrl::Settings::default());
        assert_eq!(<LemonPeakController as Plugin>::NAME, "Lemon Peak Controller");
    }
}
