// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Limiter: Lemon Studio's limiter, compressor and noise gate (a recreation of FL's Fruity Limiter) as a VST3 /
//! CLAP plugin (nih-plug). The sound is `os_engine::effects::dynamics`.
//!
// spec: plugins/lemon-limiter.md, plugins/fruity-limiter-dsp.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod editor;
pub mod params;
pub mod view;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use os_engine::effects::dynamics::{Column, Dynamics, DEFAULTS, LIM_ATT, PARAMS};

use params::LimiterParams;

/// Display columns kept (the display is 595 px wide).
pub const HISTORY: usize = 595;

/// What the audio thread and the editor share: the display's history and the editor's holds.
pub struct Shared {
    pub history: Mutex<History>,
    pub rate: AtomicU32,
    /// Set while an editor is open (the audio thread meters only then).
    pub editor_open: AtomicBool,
    /// Samples per display column (the scrolling speed).
    pub column_len: AtomicU32,
    /// The editor is dragging the limiter's attack: the look-ahead keeps its value until the mouse is released
    /// (spec §4.2 "updated on release").
    pub attack_held: AtomicBool,
}

pub struct History {
    pub cols: Vec<Column>,
    pub pos: usize,
    /// Columns written in total.
    pub written: u64,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            history: Mutex::new(History { cols: vec![Column::default(); HISTORY], pos: 0, written: 0 }),
            rate: AtomicU32::new(44100),
            editor_open: AtomicBool::new(false),
            column_len: AtomicU32::new(352),
            attack_held: AtomicBool::new(false),
        }
    }
}

impl History {
    /// The newest `n` columns, oldest first.
    pub fn latest(&self, n: usize) -> Vec<Column> {
        let len = self.cols.len();
        let n = n.min(len).min(self.written as usize);
        (0..n).map(|i| self.cols[(self.pos + len - n + i) % len]).collect()
    }
}

pub struct LemonLimiter {
    params: Arc<LimiterParams>,
    shared: Arc<Shared>,
    dsp: Dynamics,
    raw_seen: u64,
    flags: u8,
    sidechain: u8,
    /// The look-ahead value in use while the editor holds the attack knob.
    attack: i32,
    latency: u32,
}

impl Default for LemonLimiter {
    fn default() -> Self {
        LemonLimiter {
            params: Arc::new(LimiterParams::default()),
            shared: Arc::new(Shared::default()),
            dsp: Dynamics::new(44100.0, DEFAULTS, 0),
            raw_seen: u64::MAX,
            flags: 0,
            sidechain: 0,
            attack: DEFAULTS[LIM_ATT],
            latency: 0,
        }
    }
}

impl LemonLimiter {
    /// Flags and the sidechain number from the state image (never blocks: keeps the last values while the editor writes).
    fn read_raw(&mut self) {
        let v = self.params.raw_version.load(Ordering::Acquire);
        if v == self.raw_seen {
            return;
        }
        if let Ok(b) = self.params.raw.try_read() {
            self.flags = params::byte(&b, params::FLAGS_BYTE);
            self.sidechain = params::byte(&b, params::SIDECHAIN_BYTE);
            self.raw_seen = v;
        }
    }

    /// The parameters for the sound: the held attack keeps its value until the editor lets go.
    fn values(&mut self) -> [i32; PARAMS] {
        let mut p = self.params.values();
        if self.shared.attack_held.load(Ordering::Acquire) {
            p[LIM_ATT] = self.attack;
        } else {
            self.attack = p[LIM_ATT];
        }
        p
    }

    fn publish(&mut self) {
        if self.dsp.columns.is_empty() {
            return;
        }
        if let Ok(mut h) = self.shared.history.try_lock() {
            let n = h.cols.len();
            for c in self.dsp.columns.drain(..) {
                let p = h.pos;
                h.cols[p] = c;
                h.pos = (p + 1) % n;
                h.written += 1;
            }
        }
        self.dsp.columns.clear();
    }
}

impl Plugin for LemonLimiter {
    const NAME: &'static str = "Lemon Limiter";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // Stereo with an optional stereo sidechain input (spec §7).
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            aux_input_ports: &[new_nonzero_u32(2)],
            names: PortNames { aux_inputs: &["Sidechain"], ..PortNames::const_default() },
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout { main_input_channels: NonZeroU32::new(2), main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
    ];
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::LimiterEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, ctx: &mut impl InitContext<Self>) -> bool {
        self.shared.rate.store(config.sample_rate as u32, Ordering::Relaxed);
        self.raw_seen = u64::MAX;
        self.read_raw();
        let p = self.values();
        self.dsp = Dynamics::new(f64::from(config.sample_rate), p, self.flags);
        self.latency = self.dsp.latency() as u32;
        ctx.set_latency_samples(self.latency);
        true
    }

    fn reset(&mut self) {
        self.dsp.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.read_raw();
        let p = self.values();
        self.dsp.set_params(&p);
        self.dsp.set_flags(self.flags);
        let lat = self.dsp.latency() as u32;
        if lat != self.latency {
            self.latency = lat;
            ctx.set_latency_samples(lat);
        }
        let meter = self.shared.editor_open.load(Ordering::Relaxed);
        self.dsp.set_meter(if meter { self.shared.column_len.load(Ordering::Relaxed).max(1) as usize } else { 0 });

        let out = buffer.as_slice();
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let key = (self.sidechain != 0)
            .then(|| aux.inputs.first())
            .flatten()
            .map(|b| b.as_slice_immutable())
            .filter(|s| s.len() >= 2)
            .map(|s| (&*s[0], &*s[1]));
        let (left, right) = out.split_at_mut(1);
        self.dsp.process(left[0], right[0], key);
        if meter {
            self.publish();
        }
        // Asleep: the host may stop calling until sound comes in again (spec §7).
        if self.dsp.asleep() { ProcessStatus::Normal } else { ProcessStatus::KeepAlive }
    }
}

impl ClapPlugin for LemonLimiter {
    const CLAP_ID: &'static str = "org.openstudio.lemon-limiter";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's limiter, compressor and noise gate");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Limiter, ClapFeature::Compressor, ClapFeature::Gate, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonLimiter {
    const VST3_CLASS_ID: [u8; 16] = *b"LemonStudioLimit";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonLimiter);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonLimiter);
os_plugkit::linked_clap_entry!(LemonLimiter);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_returns_the_newest_columns_in_order() {
        let mut h = History { cols: vec![Column::default(); 4], pos: 0, written: 0 };
        for i in 0..6 {
            let p = h.pos;
            h.cols[p] = Column { input: i as f32, ..Column::default() };
            h.pos = (p + 1) % 4;
            h.written += 1;
        }
        let got: Vec<f32> = h.latest(3).iter().map(|c| c.input).collect();
        assert_eq!(got, vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn held_attack_waits_for_the_release() {
        let mut l = LemonLimiter::default();
        l.shared.attack_held.store(true, Ordering::Release);
        l.params.live.set(l.params.p[LIM_ATT].as_ptr(), 0.5);
        assert_eq!(l.values()[LIM_ATT], DEFAULTS[LIM_ATT]);
        l.shared.attack_held.store(false, Ordering::Release);
        assert_eq!(l.values()[LIM_ATT], l.params.v(LIM_ATT));
    }
}
