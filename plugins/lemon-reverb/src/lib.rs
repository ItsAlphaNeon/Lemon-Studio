// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Reverb: Lemon Studio's plate reverb (a recreation of FL's Fruity Reeverb 2) as a VST3 / CLAP plugin
//! (nih-plug). The sound is `os_engine::effects::reverb`.
//!
// spec: plugins/lemon-reverb.md, plugins/fruity-reeverb-2.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod editor;
pub mod params;
pub mod room;
pub mod view;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use nih_plug::prelude::*;
use os_engine::effects::reverb::{Reverb, Settings};

use params::ReverbParams;

/// What the audio thread and the editor share.
pub struct Shared {
    pub rate: AtomicU32,
    /// Set while an editor is open.
    pub editor_open: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Shared { rate: AtomicU32::new(44100), editor_open: AtomicBool::new(false) }
    }
}

pub struct LemonReverb {
    params: Arc<ReverbParams>,
    shared: Arc<Shared>,
    dsp: Reverb,
    raw: Vec<u8>,
    raw_seen: u64,
}

impl Default for LemonReverb {
    fn default() -> Self {
        LemonReverb {
            params: Arc::new(ReverbParams::default()),
            shared: Arc::new(Shared::default()),
            dsp: Reverb::new(44100, Settings::default(), 120.0),
            raw: params::default_state(),
            raw_seen: u64::MAX,
        }
    }
}

impl LemonReverb {
    /// The saved state's copy (never blocks: keeps the last one while the editor writes).
    fn read_raw(&mut self) {
        let v = self.params.raw_version.load(Ordering::Acquire);
        if v == self.raw_seen {
            return;
        }
        if let Ok(b) = self.params.raw.try_read() {
            self.raw.clone_from(&b);
            self.raw_seen = v;
        }
    }

    fn settings(&mut self) -> Settings {
        self.read_raw();
        self.params.settings(&self.raw)
    }
}

impl Plugin for LemonReverb {
    const NAME: &'static str = "Lemon Reverb";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] =
        &[AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::ReverbEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.shared.rate.store(config.sample_rate as u32, Ordering::Relaxed);
        self.raw_seen = u64::MAX;
        let s = self.settings();
        self.dsp = Reverb::new(config.sample_rate.round() as u32, s, 120.0);
        true
    }

    fn reset(&mut self) {
        self.dsp.clear();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let s = self.settings();
        self.dsp.set(s);
        if let Some(bpm) = ctx.transport().tempo {
            self.dsp.set_tempo(bpm);
        }
        let out = buffer.as_slice();
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (left, right) = out.split_at_mut(1);
        self.dsp.process(left[0], right[0]);
        // The tail rings on after the input stops.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for LemonReverb {
    const CLAP_ID: &'static str = "org.openstudio.lemon-reverb";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's plate reverb");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Reverb, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonReverb {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioRevrb";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonReverb);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonReverb);
os_plugkit::linked_clap_entry!(LemonReverb);

#[cfg(test)]
mod tests {
    use super::*;
    use os_engine::effects::reverb::{DEFAULTS, WET};

    #[test]
    fn editor_edits_reach_the_sound_and_stored_bytes_play_as_stored() {
        let mut r = LemonReverb::default();
        assert_eq!(r.settings(), Settings::default());
        r.params.live.set(r.params.p[WET].as_ptr(), 0.0);
        assert_eq!(r.settings().knobs[WET], 0);
        // A state whose decay is below the knob's range keeps it while the knob is not moved.
        let mut fl = params::default_state();
        params::set_field(&mut fl, 6, 0);
        let p = ReverbParams::default();
        p.set_raw(fl.clone());
        let mut r = LemonReverb { params: Arc::new(p), ..LemonReverb::default() };
        // The parameter is at its default, the stored field differs: the parameter wins (it was edited).
        assert_eq!(r.settings().knobs[5], DEFAULTS[5]);
    }
}
