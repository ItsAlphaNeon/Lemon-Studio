// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Vocoder: Lemon Studio's vocoder (a recreation of FL's Vocodex) as a VST3 / CLAP plugin. Stereo in and out,
//! with a stereo sidechain input bus. The modulator and the carrier are picked from the main input (the track's own
//! audio) and the sidechain by the two input settings, as the original picks them from its track and the tracks
//! sidechained to it: by default the modulator is the main input and the carrier the sidechain.
//!
//! The vocoder is `os_engine::effects::vocoder`, which the engine also plays project slots with; the settings, the
//! parameters and the editor are `lemon_small_fx::vocoder`.
//!
// spec: plugins/lemon-vocoder.md, plugins/vocodex.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

use std::num::NonZeroU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use lemon_small_fx::params::FxParams;
use lemon_small_fx::vocoder::{self as vc, SPEC};
use lemon_small_fx::{editor, Shared, MAX};
use nih_plug::prelude::*;
use os_engine::effects::vocoder::{Settings, Vocoder};

pub struct LemonVocoder {
    params: Arc<FxParams>,
    shared: Arc<Shared>,
    raw: Vec<u8>,
    raw_seen: u64,
    dsp: Vocoder,
    /// What the vocoder plays (kept to see changes without rebuilding it).
    now: Settings,
    /// The main input copied out (the output is written over it), the sidechain, silence, the output.
    own: [Vec<f32>; 2],
    side: [Vec<f32>; 2],
    silent: Vec<f32>,
    out: [Vec<f32>; 2],
}

impl Default for LemonVocoder {
    fn default() -> Self {
        let now = Settings::default();
        LemonVocoder {
            params: Arc::new(FxParams::new(SPEC)),
            shared: Arc::new(Shared::default()),
            raw: lemon_small_fx::params::default_state(SPEC),
            raw_seen: u64::MAX,
            dsp: Vocoder::new(44100, now.params()),
            now,
            own: Default::default(),
            side: Default::default(),
            silent: Vec::new(),
            out: Default::default(),
        }
    }
}

impl LemonVocoder {
    /// What the vocoder plays: the saved state (its curves and the settings no parameter covers) under the parameters
    /// (an editor edit the host has not confirmed yet wins; stored values out of a knob's range play as stored).
    fn settings(&mut self) -> Settings {
        let v = self.params.raw_version.load(Ordering::Acquire);
        if v != self.raw_seen {
            if let Ok(b) = self.params.raw.try_read() {
                self.raw.clone_from(&b);
                self.raw_seen = v;
            }
        }
        let s: [i32; MAX] = self.params.settings(&self.raw);
        vc::settings(&self.raw, &s[..SPEC.settings.len()])
    }

    fn apply(&mut self) {
        let s = self.settings();
        if s != self.now {
            self.now = s;
            self.dsp.set(s.params());
        }
    }

    fn reserve(&mut self, n: usize) {
        for v in self.own.iter_mut().chain(self.side.iter_mut()).chain(self.out.iter_mut()).chain(std::iter::once(&mut self.silent)) {
            v.clear();
            v.resize(n, 0.0);
        }
    }
}

impl Plugin for LemonVocoder {
    const NAME: &'static str = SPEC.name;
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // Stereo with a stereo sidechain input, the carrier's by default (spec plugins/lemon-vocoder.md §1).
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        aux_input_ports: &[new_nonzero_u32(2)],
        names: PortNames { aux_inputs: &["Sidechain"], ..PortNames::const_default() },
        ..AudioIOLayout::const_default()
    }];
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
        let rate = config.sample_rate.round() as u32;
        self.shared.rate.store(rate, Ordering::Relaxed);
        self.raw_seen = u64::MAX;
        self.dsp.set_sample_rate(rate);
        self.apply();
        self.reserve(config.max_buffer_size as usize);
        true
    }

    fn reset(&mut self) {
        self.dsp.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, _ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.apply();
        let n = buffer.samples();
        if self.own[0].len() < n {
            self.reserve(n);
        }
        let main = buffer.as_slice();
        if main.len() < 2 {
            return ProcessStatus::Normal;
        }
        self.own[0][..n].copy_from_slice(&main[0][..n]);
        self.own[1][..n].copy_from_slice(&main[1][..n]);
        let side = aux.inputs.first().map(|b| b.as_slice_immutable()).filter(|s| s.len() >= 2);
        match side {
            Some(s) => {
                self.side[0][..n].copy_from_slice(&s[0][..n]);
                self.side[1][..n].copy_from_slice(&s[1][..n]);
            }
            None => {
                self.side[0][..n].fill(0.0);
                self.side[1][..n].fill(0.0);
            }
        }
        let (m, c) = self.now.routing().pick((&self.own[0][..n], &self.own[1][..n]), (&self.side[0][..n], &self.side[1][..n]), &self.silent[..n]);
        let [ol, or] = &mut self.out;
        self.dsp.process(m, c, (&mut ol[..n], &mut or[..n]));
        main[0][..n].copy_from_slice(&self.out[0][..n]);
        main[1][..n].copy_from_slice(&self.out[1][..n]);
        // The envelopes ring out after the input stops.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for LemonVocoder {
    const CLAP_ID: &'static str = "org.openstudio.lemon-vocoder";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's vocoder");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonVocoder {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioVocdr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonVocoder);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonVocoder);
os_plugkit::linked_clap_entry!(LemonVocoder);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_instance_plays_the_originals_default_with_the_carrier_on_the_sidechain() {
        let mut p = LemonVocoder::default();
        assert_eq!(p.settings(), Settings::default());
        let r = p.settings().routing();
        assert_eq!((r.modulator, r.carrier, r.lr), (0, 1, false));
        assert_eq!(<LemonVocoder as Plugin>::NAME, "Lemon Vocoder");
        // An editor edit reaches what it plays: the band count at its lowest (5).
        let id = p.params.p[vc::BANDS].as_ptr();
        p.params.live.set(id, 0.0);
        assert_eq!(p.settings().v[os_engine::effects::vocoder::BANDS], 5);
    }
}
