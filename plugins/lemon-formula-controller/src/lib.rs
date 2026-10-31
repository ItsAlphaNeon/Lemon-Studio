// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Formula Controller: Lemon Studio's formula controller (a recreation of FL's Fruity Formula Controller) as a
//! VST3 / CLAP plugin. Once per tick of the song (1/96 beat) it works out its formula of the knobs a, b and c, the song
//! position, the tempo and the clock, and puts out the result as one controller value; the sound passes through. The
//! value leaves as MIDI on channel 0, a pitch bend with the CC 63 after it adding 7 finer bits (21 bits in all), as
//! Lemon Peak Controller's do; Lemon Studio's links read it (`HostedEffect::control_fx`), other hosts can map it. A
//! formula that does not compile puts out nothing.
//!
//! The formula is `os_engine::effects::formula_ctrl::Core`, which the engine also plays project slots with; the
//! settings, the parameters and the editor are `lemon_small_fx::formula_controller`, the ticks and the MIDI out
//! `lemon_small_fx::ticks`.
//!
// spec: plugins/lemon-formula-controller.md, plugins/fruity-formula-controller.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

use std::num::NonZeroU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use lemon_small_fx::formula_controller::{self as fc, SPEC};
use lemon_small_fx::params::FxParams;
use lemon_small_fx::ticks::{send_outputs, Ticks, TICKS_PER_BEAT};
use lemon_small_fx::{editor, Shared};
use nih_plug::prelude::*;
use os_engine::effects::formula_ctrl::Core;

pub struct LemonFormulaController {
    params: Arc<FxParams>,
    shared: Arc<Shared>,
    raw: Vec<u8>,
    raw_seen: u64,
    core: Core,
    /// The song's ticks (they run on while the song is stopped).
    ticks: Ticks,
    tempo: f64,
    playing: bool,
    /// The song position, in beats, while the song is stopped (where it stopped, or where the host says it is).
    stopped_at: f64,
    /// The code last sent (`None`: send at the next tick).
    sent: [Option<(u16, u8)>; 1],
}

impl Default for LemonFormulaController {
    fn default() -> Self {
        let raw = lemon_small_fx::params::default_state(SPEC);
        LemonFormulaController {
            params: Arc::new(FxParams::new(SPEC)),
            shared: Arc::new(Shared::default()),
            core: Core::new(fc::settings(&raw, &SPEC.start())),
            raw,
            raw_seen: u64::MAX,
            ticks: Ticks::new(44100, 120.0),
            tempo: 120.0,
            playing: false,
            stopped_at: 0.0,
            sent: [None],
        }
    }
}

impl LemonFormulaController {
    /// Takes up the saved state's formula when the editor or the host changed it (never blocks), and the knobs (an
    /// editor edit the host has not confirmed yet wins).
    fn update(&mut self) {
        let v = self.params.raw_version.load(Ordering::Acquire);
        if v != self.raw_seen {
            if let Ok(b) = self.params.raw.try_read() {
                self.raw.clone_from(&b);
                self.raw_seen = v;
                let s = self.params.settings(&self.raw);
                self.core.set(fc::settings(&self.raw, &s[..3]));
            }
        }
        let s = self.params.settings(&self.raw);
        self.core.set_knobs([s[0], s[1], s[2]]);
    }

    /// Sends the output if it changed since it was last sent, at `timing` (nothing without one).
    fn send(&mut self, ctx: &mut impl ProcessContext<Self>, timing: u32) {
        if let Some(v) = self.core.output() {
            send_outputs(ctx, &mut self.sent, &[v], timing);
        }
    }
}

impl Plugin for LemonFormulaController {
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
        self.update();
        self.sent = [None];
        true
    }

    fn reset(&mut self) {
        self.sent = [None];
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.update();
        let rate = self.shared.rate.load(Ordering::Relaxed).max(1);
        let t = ctx.transport();
        let playing = t.playing;
        self.tempo = t.tempo.filter(|t| *t > 0.0).unwrap_or(self.tempo);
        self.ticks.set(rate, self.tempo);
        let at = t.pos_beats();
        let starting = playing && !self.playing;
        self.playing = playing;
        match at {
            Some(b) if playing => self.ticks.sync(b * TICKS_PER_BEAT, starting),
            Some(b) => self.stopped_at = b,
            None => {}
        }
        if self.sent[0].is_none() {
            self.send(ctx, 0);
        }
        let n = buffer.samples();
        while let Some(e) = self.ticks.next_end(n) {
            // The next tick's start: its value acts on the targets from there (spec "Ticks").
            let beats = if playing { self.ticks.tick() as f64 / TICKS_PER_BEAT } else { self.stopped_at };
            if playing {
                self.stopped_at = beats;
            }
            self.core.advance(beats, self.tempo);
            self.send(ctx, e as u32);
        }
        self.ticks.done(n);
        // The formula runs on in silence.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for LemonFormulaController {
    const CLAP_ID: &'static str = "org.openstudio.lemon-formula-controller";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's formula controller");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Utility, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonFormulaController {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioFormC";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Tools];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonFormulaController);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonFormulaController);
os_plugkit::linked_clap_entry!(LemonFormulaController);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_instance_is_the_originals_default() {
        let p = LemonFormulaController::default();
        assert_eq!(p.core.s, os_engine::effects::formula_ctrl::Settings::default());
        assert_eq!(p.core.output(), Some(0.0), "no formula: 0");
        assert_eq!(<LemonFormulaController as Plugin>::NAME, "Lemon Formula Controller");
        assert_ne!(<LemonFormulaController as Vst3Plugin>::VST3_CLASS_ID, *b"LemonStudioPeakC");
    }
}
