// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Trio: Lemon Studio's three-oscillator synthesizer as a VST3 / CLAP plugin (nih-plug).
//!
// spec: plugins/lemon-3x-osc.md, plugins/3x-osc.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod editor;
pub mod params;
pub mod view;

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use os_engine::envelope;

use params::*;

/// State shared by the audio thread and the editor.
#[derive(Default)]
pub struct Shared {
    /// Notes the editor plays itself (key, velocity, on) when the host does not play them.
    pub previews: Mutex<Vec<(u8, f32, bool)>>,
    /// Keys sounding (bit per key).
    pub sounding: [AtomicU64; 2],
    /// The custom shape's wave (spec: plugins/lemon-3x-osc.md §6); `custom_version` moves on every change.
    pub custom: Mutex<Option<Arc<dsp::Wave>>>,
    pub custom_version: AtomicU64,
    /// The file `custom` came from (also kept when it could not be read).
    loaded_path: Mutex<String>,
}

impl Shared {
    /// Loads sample file `path` as the custom shape's wave (UI or initialize thread); the state remembers it.
    pub fn load(&self, path: &str, params: &Osc3Params) -> Result<(), String> {
        let s = os_engine::sample::load_shared(Path::new(path)).map_err(|e| e.to_string())?;
        let w = dsp::Wave::from_sample(&s.left, &s.right).ok_or_else(|| format!("{path}: no sound in it"))?;
        self.set_custom(Some(Arc::new(w)), path, params);
        Ok(())
    }

    /// Replaces the custom shape's wave; `path` is what the state remembers ("" = no sample).
    pub fn set_custom(&self, w: Option<Arc<dsp::Wave>>, path: &str, params: &Osc3Params) {
        if let Ok(mut c) = self.custom.lock() {
            *c = w;
        }
        if let Ok(mut p) = self.loaded_path.lock() {
            *p = path.to_string();
        }
        if let Ok(mut p) = params.sample_path.write() {
            *p = path.to_string();
        }
        self.custom_version.fetch_add(1, Ordering::AcqRel);
    }

    /// The custom shape's sample file ("" = none).
    pub fn loaded_path(&self) -> String {
        self.loaded_path.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// The custom shape's wave.
    pub fn wave(&self) -> Option<Arc<dsp::Wave>> {
        self.custom.lock().ok().and_then(|c| c.clone())
    }

    pub fn sounding_keys(&self) -> Vec<i32> {
        let mut out = Vec::new();
        for (w, a) in self.sounding.iter().enumerate() {
            let bits = a.load(Ordering::Relaxed);
            for b in 0..64 {
                if bits & (1 << b) != 0 {
                    out.push((w * 64 + b) as i32);
                }
            }
        }
        out
    }
}

/// The per-block settings of the voices from the parameters (spec: plugins/3x-osc.md §1, §2).
pub fn block(p: &Osc3Params, bend: f64) -> dsp::Block {
    let v = |k: usize| p.v(k);
    let f = |k: usize| f64::from(p.v(k));
    let osc = |n: usize| {
        let b = osc(n);
        dsp::Osc {
            shape: v(b + SHAPE).clamp(0, 6) as u8,
            invert: p.flag(n),
            pan: f(b + PAN) / 64.0,
            semis: f(b + COARSE) + f(b + FINE) / 100.0,
            phase: f(b + PHASE) / 64.0 * 0.5,
            detune: f(b + DETUNE),
        }
    };
    let target = |t: usize| {
        let r = rec(t);
        // The panning record has no audible effect (spec §2.6).
        if t == T_PAN {
            return dsp::Target::default();
        }
        let e = os_model::Envelope {
            enabled: v(r + R_ON) != 0,
            delay: v(r + R_DELAY),
            attack: v(r + R_ATT),
            hold: v(r + R_HOLD),
            decay: v(r + R_DEC),
            sustain: v(r + R_SUS),
            release: v(r + R_REL),
            amount: v(r + R_AMT),
            attack_tension: v(r + R_ATT_T),
            decay_tension: v(r + R_DEC_T),
            release_tension: v(r + R_REL_T),
            ..Default::default()
        };
        dsp::Target {
            env: envelope::Params::new(&e),
            lfo: dsp::Lfo {
                shape: v(r + R_LFO_SHAPE).clamp(0, 2) as u8,
                delay: envelope::knob_seconds(v(r + R_LFO_DELAY)),
                attack: envelope::knob_seconds(v(r + R_LFO_ATT)),
                amount: f(r + R_LFO_AMT),
                hz: dsp::lfo_hz(v(r + R_LFO_SPEED)),
            },
        }
    };
    dsp::Block {
        osc: [osc(0), osc(1), osc(2)],
        m2: f(MIX2) / 128.0,
        m3: f(MIX3) / 128.0,
        am: p.flag(AM_BYTE),
        hq: v(HQ) != 0,
        rand: f(RAND) / 64.0,
        targets: std::array::from_fn(target),
        filter: v(FILTER).clamp(0, 7) as u8,
        cutoff: f(CUTOFF),
        resonance: f(RESONANCE),
        bend,
    }
}

pub struct LemonTrio {
    params: Arc<Osc3Params>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    /// Pitch bend in cents.
    bend: f64,
    /// `Shared::custom_version` the engine's custom wave is from.
    custom_seen: u64,
}

impl Default for LemonTrio {
    fn default() -> Self {
        LemonTrio { params: Arc::new(Osc3Params::default()), shared: Arc::new(Shared::default()), engine: dsp::Engine::new(44100.0), bend: 0.0, custom_seen: 0 }
    }
}

impl LemonTrio {
    /// The engine takes the custom wave last loaded (never waits: a busy lock is tried again next block).
    fn take_custom(&mut self) {
        let v = self.shared.custom_version.load(Ordering::Acquire);
        if v != self.custom_seen
            && let Ok(w) = self.shared.custom.try_lock()
        {
            self.engine.custom = w.clone();
            self.custom_seen = v;
        }
    }

    fn publish_sounding(&self) {
        let mut bits = [0u64; 2];
        for k in self.engine.sounding() {
            let k = usize::from(k).min(127);
            bits[k / 64] |= 1 << (k % 64);
        }
        for (a, b) in self.shared.sounding.iter().zip(bits) {
            a.store(b, Ordering::Relaxed);
        }
    }
}

impl Plugin for LemonTrio {
    const NAME: &'static str = "Lemon Trio";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::Osc3Editor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.engine.set_rate(config.sample_rate);
        // A restored state names the custom shape's sample: load it now (initialize may block). One that cannot be read
        // stays named, so the state keeps it, and plays silence.
        let path = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
        if path != self.shared.loaded_path() {
            if path.is_empty() {
                self.shared.set_custom(None, "", &self.params);
            } else if let Err(e) = self.shared.load(&path, &self.params) {
                nih_log!("Lemon Trio: {e}");
                self.shared.set_custom(None, &path, &self.params);
            }
        }
        self.take_custom();
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.publish_sounding();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.take_custom();
        let mut blk = block(&self.params, self.bend);
        if let Ok(mut q) = self.shared.previews.try_lock() {
            for (key, vel, on) in q.drain(..) {
                if on {
                    self.engine.note_on(key, vel, None, &blk);
                } else {
                    self.engine.note_off(key);
                }
            }
        }
        let len = buffer.samples();
        let out = buffer.as_slice();
        for ch in out.iter_mut() {
            ch.fill(0.0);
        }
        if out.len() < 2 {
            return ProcessStatus::Normal;
        }
        let mut at = 0;
        while let Some(ev) = ctx.next_event() {
            let t = (ev.timing() as usize).min(len);
            if t > at {
                let (left, right) = out.split_at_mut(1);
                self.engine.render(&mut left[0][at..t], &mut right[0][at..t], &blk);
                at = t;
            }
            match ev {
                NoteEvent::NoteOn { note, velocity, voice_id, .. } => self.engine.host_note_on(note, velocity, voice_id, &blk),
                NoteEvent::NoteOff { note, voice_id, .. } => self.engine.note_off_id(note, voice_id),
                NoteEvent::Choke { note, .. } => self.engine.choke(note),
                // Portamento and slide notes (spec: audio/channel-voices.md): the host's tuning of a key's voices.
                NoteEvent::PolyTuning { note, voice_id, tuning, .. } => self.engine.tune(note, voice_id, f64::from(tuning)),
                NoteEvent::MidiPitchBend { value, .. } => {
                    // ±2 semitones, as Lemon Sampler.
                    self.bend = f64::from(value * 2.0 - 1.0) * 200.0;
                    blk.bend = self.bend;
                }
                NoteEvent::MidiCC { cc: 120 | 123, .. } => self.engine.all_off(),
                _ => {}
            }
        }
        if at < len {
            let (left, right) = out.split_at_mut(1);
            self.engine.render(&mut left[0][at..len], &mut right[0][at..len], &blk);
        }
        // Voice ends go back to the host, at the end of the block (spec: audio/voices.md §4).
        let last = (len as u32).saturating_sub(1);
        for (note, voice_id) in self.engine.take_ended() {
            ctx.send_event(NoteEvent::VoiceTerminated { timing: last, voice_id, channel: 0, note });
        }
        self.publish_sounding();
        if self.engine.has_voices() { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

// CLAP id and VST3 class id keep the spelling of the plugin's earlier "Open" name: hosts find saved sessions'
// plugins by them (spec: plugins/plugin-formats.md, identity).
impl ClapPlugin for LemonTrio {
    const CLAP_ID: &'static str = "org.openstudio.open-3x-osc";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's three-oscillator synthesizer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Synthesizer, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonTrio {
    const VST3_CLASS_ID: [u8; 16] = *b"OpenStudio3xOsc!";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonTrio);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonTrio);
os_plugkit::linked_clap_entry!(LemonTrio);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_block_is_the_default_patch() {
        let p = Osc3Params::default();
        let b = block(&p, 0.0);
        let d = dsp::Block::default();
        assert_eq!((b.osc, b.m2, b.m3, b.am, b.hq, b.rand), (d.osc, d.m2, d.m3, d.am, d.hq, d.rand));
        assert_eq!((b.filter, b.cutoff, b.resonance), (0, 256.0, 0.0));
        assert!(b.filter_off());
        assert!(b.targets.iter().all(|t| t.env.is_none() && t.lfo.amount == 0.0));
        assert!((b.targets[1].lfo.hz - 0.505).abs() < 1e-9);
    }
}
