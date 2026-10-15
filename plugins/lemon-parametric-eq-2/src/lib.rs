// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon Parametric EQ: Lemon Studio's 7-band parametric EQ as a VST3 / CLAP plugin (nih-plug).
//!
// spec: plugins/lemon-parametric-eq-2.md, plugins/parametric-eq-2.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod analyzer;
pub mod editor;
pub mod params;
pub mod view;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use os_engine::effects::eq::{Band, Sections, Settings, BANDS};

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

/// Parameter changes glide over this many seconds (no zipper noise while dragging).
const GLIDE: f64 = 0.008;
/// Filters are redesigned at most every this many samples while gliding.
const CHUNK: usize = 32;

/// One band's filter sections and the settings they were designed for.
#[derive(Clone, Copy, Default)]
struct BandDsp {
    secs: Sections,
    designed: Option<Band>,
}

impl BandDsp {
    fn design(&mut self, b: &Band, sr: f64) {
        if self.designed.as_ref() == Some(b) {
            return;
        }
        let new = b.sections(sr);
        // Keep the filters' memory for sections that stay (a glide, not a restart).
        for i in 0..new.n {
            if i < self.secs.n {
                self.secs.s[i].set_coefs(&new.s[i]);
            } else {
                self.secs.s[i] = new.s[i];
                self.secs.s[i].reset();
            }
        }
        self.secs.n = new.n;
        self.designed = Some(*b);
    }
}

pub struct LemonEq {
    params: Arc<EqParams>,
    shared: Arc<Shared>,
    sr: f64,
    bands: [BandDsp; BANDS],
    /// Settings the filters glide toward / are at.
    cur: Option<Settings>,
    gain: f64,
    mask: u8,
    /// Monitor (0 off, 1 input, 2 output) and input (1 Mid, 2 Side, 3 Left, 4 Right), read with the mask.
    monitor: (u8, i32),
    extra_seen: u64,
    feed: Vec<f32>,
}

impl Default for LemonEq {
    fn default() -> Self {
        LemonEq {
            params: Arc::new(EqParams::default()),
            shared: Arc::new(Shared::default()),
            sr: 44100.0,
            bands: [BandDsp::default(); BANDS],
            cur: None,
            gain: 1.0,
            mask: 0x7f,
            monitor: (2, 1),
            extra_seen: u64::MAX,
            feed: Vec::new(),
        }
    }
}

fn glide(cur: f64, to: f64, a: f64) -> f64 {
    let v = cur + (to - cur) * a;
    if (to - v).abs() < 1e-5 { to } else { v }
}

impl LemonEq {
    /// Options from the state image (never blocks: keeps the last values while the editor writes).
    fn read_extra(&mut self) {
        let v = self.params.extra_version.load(Ordering::Acquire);
        if v == self.extra_seen {
            return;
        }
        if let Ok(b) = self.params.extra.try_read() {
            self.mask = params::mask(&b);
            self.monitor = (params::byte(&b, params::MONITOR_BYTE), params::opt(&b, params::Opt::MonitorInput));
            self.extra_seen = v;
        }
    }

    /// Moves the current settings a step toward `t` and redesigns the bands that changed.
    fn step(&mut self, t: &Settings, a: f64) {
        let cur = self.cur.get_or_insert(*t);
        for (c, t) in cur.bands.iter_mut().zip(&t.bands) {
            c.kind = t.kind;
            c.order = t.order;
            c.db = glide(c.db, t.db, a);
            c.freq = glide(c.freq, t.freq, a);
            c.width = glide(c.width, t.width, a);
        }
        cur.main_db = glide(cur.main_db, t.main_db, a);
        cur.mask = t.mask;
        let cur = *cur;
        for (d, b) in self.bands.iter_mut().zip(&cur.bands) {
            d.design(b, self.sr);
        }
        self.gain = 10f64.powf(cur.main_db / 20.0);
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

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.sr = f64::from(config.sample_rate);
        self.shared.rate.store(config.sample_rate as u32, Ordering::Relaxed);
        self.bands = [BandDsp::default(); BANDS];
        self.cur = None;
        self.extra_seen = u64::MAX;
        self.feed = Vec::with_capacity(config.max_buffer_size as usize);
        true
    }

    fn reset(&mut self) {
        for b in &mut self.bands {
            for s in &mut b.secs.s {
                s.reset();
            }
        }
        self.cur = None;
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, _ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.read_extra();
        let target = self.params.settings(self.mask);
        let a = 1.0 - (-(CHUNK as f64) / (GLIDE * self.sr)).exp();
        let len = buffer.samples();
        let out = buffer.as_slice();
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let watch = self.shared.editor_open.load(Ordering::Relaxed) && self.monitor.0 != 0;
        if watch && self.monitor.0 == 1 {
            self.feed_scope(&out[0][..len], &out[1][..len]);
        }
        let mut at = 0;
        while at < len {
            let end = (at + CHUNK).min(len);
            self.step(&target, a);
            let mask = self.mask;
            let (left, right) = out.split_at_mut(1);
            for i in at..end {
                let (mut l, mut r) = (f64::from(left[0][i]), f64::from(right[0][i]));
                for (k, b) in self.bands.iter_mut().enumerate() {
                    if mask & (1 << k) == 0 {
                        continue;
                    }
                    for s in &mut b.secs.s[..b.secs.n] {
                        l = s.tick(0, l);
                        r = s.tick(1, r);
                    }
                }
                left[0][i] = (l * self.gain) as f32;
                right[0][i] = (r * self.gain) as f32;
            }
            at = end;
        }
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
