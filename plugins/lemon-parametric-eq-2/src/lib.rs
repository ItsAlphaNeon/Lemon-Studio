// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Parametric EQ: Lemon Studio's 7-band parametric EQ as a VST3 / CLAP plugin (nih-plug). The sound is the
//! original's in every mode (`os_engine::effects::eq::Equalizer`); the plugin reports LIN's and HQ Enhanced's delay.
//!
// spec: plugins/lemon-parametric-eq-2.md, plugins/fruity-parametric-eq-2.md, plugins/parametric-eq-2.md,
// plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod analyzer;
pub mod editor;
pub mod params;
pub mod view;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use os_engine::effects::eq::{Equalizer, Options, PARAMS};

use params::EqParams;

/// Samples kept for the analyzer.
pub const SCOPE_LEN: usize = 1 << 15;

/// What the audio thread hands the editor: the monitored signal for the analyzer.
pub struct Shared {
    pub scope: Mutex<Scope>,
    pub rate: AtomicU32,
    /// Set while an editor is open (the audio thread feeds the analyzer only then).
    pub editor_open: AtomicBool,
}

pub struct Scope {
    pub buf: Vec<f32>,
    pub pos: usize,
    /// Samples written in total.
    pub written: u64,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            scope: Mutex::new(Scope { buf: vec![0.0; SCOPE_LEN], pos: 0, written: 0 }),
            rate: AtomicU32::new(44100),
            editor_open: AtomicBool::new(false),
        }
    }
}

impl Scope {
    /// The newest `n` samples, oldest first.
    pub fn latest(&self, n: usize) -> Vec<f32> {
        let n = n.min(self.buf.len());
        (0..n).map(|i| self.buf[(self.pos + self.buf.len() - n + i) % self.buf.len()]).collect()
    }
}

pub struct LemonEq {
    params: Arc<EqParams>,
    shared: Arc<Shared>,
    sr: f64,
    /// The original's processing (every mode), and the parameters it was last given as stored values.
    eq: Equalizer,
    applied: [i32; PARAMS],
    /// The options read from the state image, and the delay last reported to the host.
    opts: Options,
    latency: u32,
    /// Monitor (0 off, 1 input, 2 output) and input (1 Mid, 2 Side, 3 Left, 4 Right), read with the options.
    monitor: (u8, i32),
    extra_seen: u64,
    feed: Vec<f32>,
}

impl Default for LemonEq {
    fn default() -> Self {
        let params = Arc::new(EqParams::default());
        let opts = Options::from_image(&params::default_state());
        let applied = params.settings(opts.mask).params();
        LemonEq {
            params,
            shared: Arc::new(Shared::default()),
            sr: 44100.0,
            eq: Equalizer::new(44100.0, &applied, opts),
            applied,
            opts,
            latency: 0,
            monitor: (2, 1),
            extra_seen: u64::MAX,
            feed: Vec::new(),
        }
    }
}

impl LemonEq {
    /// Options from the state image (never blocks: keeps the last values while the editor writes).
    fn read_extra(&mut self) {
        let v = self.params.extra_version.load(Ordering::Acquire);
        if v == self.extra_seen {
            return;
        }
        if let Ok(b) = self.params.extra.try_read() {
            self.opts = Options::from_image(&b);
            self.monitor = (params::byte(&b, params::MONITOR_BYTE), params::opt(&b, params::Opt::MonitorInput));
            self.extra_seen = v;
        }
    }

    /// The parameters and options as they stand, handed to the processing: a changed value counts as an automation
    /// value (coefficients anew from this block on, no smoothing, spec: "State and timing").
    fn apply(&mut self) {
        self.read_extra();
        self.eq.set_options(self.opts);
        let p = self.params.settings(self.opts.mask).params();
        for (i, (&new, old)) in p.iter().zip(self.applied.iter_mut()).enumerate() {
            if new != *old {
                *old = new;
                self.eq.set_param(i, new);
            }
        }
    }

    fn feed_scope(&mut self, l: &[f32], r: &[f32]) {
        self.feed.clear();
        let input = self.monitor.1;
        self.feed.extend(l.iter().zip(r).map(|(&l, &r)| match input {
            2 => (l - r) * 0.5,
            3 => l,
            4 => r,
            _ => (l + r) * 0.5,
        }));
        if let Ok(mut s) = self.shared.scope.try_lock() {
            let n = s.buf.len();
            for &x in &self.feed {
                let p = s.pos;
                s.buf[p] = x;
                s.pos = (p + 1) % n;
            }
            s.written += self.feed.len() as u64;
        }
    }
}

impl Plugin for LemonEq {
    const NAME: &'static str = "Lemon Parametric EQ";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::EqEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, ctx: &mut impl InitContext<Self>) -> bool {
        self.sr = f64::from(config.sample_rate);
        self.shared.rate.store(config.sample_rate as u32, Ordering::Relaxed);
        self.extra_seen = u64::MAX;
        self.read_extra();
        self.applied = self.params.settings(self.opts.mask).params();
        self.eq = Equalizer::new(self.sr, &self.applied, self.opts);
        self.latency = self.eq.latency() as u32;
        ctx.set_latency_samples(self.latency);
        self.feed = Vec::with_capacity(config.max_buffer_size as usize);
        true
    }

    /// A new instance's state, with FL's silent buffers before a render already run (LIN starts at full level).
    fn reset(&mut self) {
        self.eq.reset();
        self.eq.preroll();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.apply();
        // The delay is looked at every block and reported when it has changed (LIN on or off, its size, HQ Enhanced).
        let latency = self.eq.latency() as u32;
        if latency != self.latency {
            self.latency = latency;
            ctx.set_latency_samples(latency);
        }
        let len = buffer.samples();
        let out = buffer.as_slice();
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let watch = self.shared.editor_open.load(Ordering::Relaxed) && self.monitor.0 != 0;
        if watch && self.monitor.0 == 1 {
            self.feed_scope(&out[0][..len], &out[1][..len]);
        }
        let (left, right) = out.split_at_mut(1);
        self.eq.process(&mut left[0][..len], &mut right[0][..len]);
        if watch && self.monitor.0 == 2 {
            self.feed_scope(&out[0][..len], &out[1][..len]);
        }
        ProcessStatus::Normal
    }
}

// CLAP id and VST3 class id keep the spelling of the plugin's earlier "Open" name: hosts find saved sessions'
// plugins by them (spec: plugins/plugin-formats.md, identity).
impl ClapPlugin for LemonEq {
    const CLAP_ID: &'static str = "org.openstudio.open-parametric-eq-2";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's 7-band parametric EQ");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Equalizer, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonEq {
    const VST3_CLASS_ID: [u8; 16] = *b"OpenStudioParEQ2";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Eq];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonEq);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonEq);
os_plugkit::linked_clap_entry!(LemonEq);
