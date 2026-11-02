// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Zest as a VST3 / CLAP instrument (nih-plug).
//!
//! Every parameter is automatable and saved by the host. The editor is `zest-ui` (Lemon Studio's plugin look: Main,
//! Effects and Advanced pages, the preset browser); `editor` is its window glue. Presets: the banks built in and
//! dropped in (`zest_preset`), the user's own (`.zest`, in `~/.local/share/zest/presets/`) and Diva's (`.h2p`) from the
//! user's Diva preset folder.

use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use zest_core::params::COUNT;
use zest_core::slop::PresetSlop;
use zest_core::{Engine, Event, Transport};

pub mod editor;
mod params_gen;
mod scope;
use params_gen::ZestParams;

/// The largest slice handed to the engine at once; longer host blocks are split.
const MAX_BLOCK: usize = 1024;
const MAX_EVENTS: usize = 1024;

/// The preset's pots and Rnd flags, handed from the editor to the audio thread.
pub type PendingSlop = Arc<Mutex<Option<PresetSlop>>>;

pub struct Zest {
    params: Arc<ZestParams>,
    engine: Option<Engine>,
    pending: PendingSlop,
    /// The last sound played, for the editor's visualizer.
    scope: Arc<scope::Scope>,
    events: Vec<Event>,
    chunk: Vec<Event>,
    values: [f32; COUNT],
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Default for Zest {
    fn default() -> Self {
        Zest {
            params: Arc::new(ZestParams::new()),
            engine: None,
            pending: Arc::new(Mutex::new(None)),
            scope: Arc::new(scope::Scope::default()),
            events: Vec::with_capacity(MAX_EVENTS),
            chunk: Vec::with_capacity(MAX_EVENTS),
            values: [0.0; COUNT],
            left: vec![0.0; MAX_BLOCK],
            right: vec![0.0; MAX_BLOCK],
        }
    }
}

fn event_time(e: &Event) -> u32 {
    match *e {
        Event::NoteOn { time, .. }
        | Event::NoteOff { time, .. }
        | Event::PitchBend { time, .. }
        | Event::Controller { time, .. }
        | Event::Pressure { time, .. }
        | Event::Panic { time }
        | Event::Tuning { time, .. } => time,
    }
}

fn with_time(e: &Event, t: u32) -> Event {
    let mut e = *e;
    match &mut e {
        Event::NoteOn { time, .. }
        | Event::NoteOff { time, .. }
        | Event::PitchBend { time, .. }
        | Event::Controller { time, .. }
        | Event::Pressure { time, .. }
        | Event::Panic { time }
        | Event::Tuning { time, .. } => *time = t,
    }
    e
}

impl Plugin for Zest {
    const NAME: &'static str = "Zest";
    const VENDOR: &'static str = "Lemon (local build)";
    const URL: &'static str = "https://example.invalid/zest";
    const EMAIL: &'static str = "none@example.invalid";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] =
        &[AudioIOLayout { main_input_channels: None, main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::ZestEditor::new(self.params.clone(), self.pending.clone(), self.scope.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        // The preset library is read before the editor first opens (`Library::scan_cached`).
        zest_ui::library::Library::prewarm(&zest_ui::library::default_sources());
        let mut e = Engine::new(config.sample_rate, MAX_BLOCK);
        e.set_offline(config.process_mode == ProcessMode::Offline);
        self.scope.set_rate(config.sample_rate);
        // Diva's 16-sample event grid (see zest-core): the host compensates it
        context.set_latency_samples(e.latency());
        self.engine = Some(e);
        // The analog spread of the preset the state was saved with (the engine is new here).
        let kept = self.params.editor.read().ok().and_then(|m| m.spread.clone());
        if let Some(f) = kept.and_then(|t| zest_preset::parse(&t).ok()) {
            if let Ok(mut p) = self.pending.lock() {
                *p = Some(f.spread.into());
            }
        }
        true
    }

    fn reset(&mut self) {
        if let Some(e) = &mut self.engine {
            e.reset();
        }
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let n = buffer.samples();
        self.events.clear();
        while let Some(ev) = context.next_event() {
            let time = (ev.timing() as usize).min(n.saturating_sub(1)) as u32;
            let e = match ev {
                NoteEvent::NoteOn { note, velocity, .. } => Event::NoteOn { time, key: note, velocity },
                NoteEvent::NoteOff { note, .. } => Event::NoteOff { time, key: note },
                NoteEvent::Choke { note, .. } => Event::NoteOff { time, key: note },
                NoteEvent::MidiPitchBend { value, .. } => Event::PitchBend { time, value: value * 2.0 - 1.0 },
                NoteEvent::MidiCC { cc, value, .. } => {
                    if cc == 123 || cc == 120 {
                        Event::Panic { time }
                    } else {
                        Event::Controller { time, cc, value }
                    }
                }
                NoteEvent::MidiChannelPressure { pressure, .. } => Event::Pressure { time, value: pressure },
                // Per-note tuning (CLAP note expressions): Lemon Studio's slide notes and glides.
                NoteEvent::PolyTuning { note, tuning, .. } => Event::Tuning { time, key: note, semitones: tuning },
                _ => continue,
            };
            if self.events.len() < MAX_EVENTS {
                self.events.push(e);
            }
        }
        let tr = context.transport();
        let mut transport = Transport { tempo: tr.tempo.unwrap_or(120.0), playing: tr.playing, beats: tr.pos_beats().unwrap_or(0.0) };
        let out = buffer.as_slice();
        let Some(engine) = &mut self.engine else {
            for ch in out.iter_mut() {
                ch.fill(0.0);
            }
            return ProcessStatus::Normal;
        };
        // a preset loaded in the editor: its pots and Rnd flags first, as a preset's knobs apply over them (never blocks: a
        // busy lock waits for the next call)
        if let Ok(mut p) = self.pending.try_lock() {
            if let Some(slop) = p.take() {
                engine.apply_preset_slop(&slop);
            }
        }
        self.params.values(&mut self.values);
        engine.set_params(&self.values);
        let mut start = 0usize;
        let mut ev = 0usize;
        while start < n {
            let len = (n - start).min(MAX_BLOCK);
            self.chunk.clear();
            while ev < self.events.len() && (event_time(&self.events[ev]) as usize) < start + len {
                let t = event_time(&self.events[ev]) as usize - start;
                self.chunk.push(with_time(&self.events[ev], t as u32));
                ev += 1;
            }
            let (l, r) = (&mut self.left[..len], &mut self.right[..len]);
            engine.process(&self.chunk, &transport, l, r);
            out[0][start..start + len].copy_from_slice(l);
            out[1][start..start + len].copy_from_slice(r);
            self.scope.push(l, r);
            transport.beats += len as f64 / 60.0 * transport.tempo / context.transport().sample_rate as f64;
            start += len;
        }
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Zest {
    const CLAP_ID: &'static str = "org.lemon.zest-local";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Analog synthesizer (local test build)");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Synthesizer, ClapFeature::Stereo];
}

impl Vst3Plugin for Zest {
    const VST3_CLASS_ID: [u8; 16] = *b"ZestLocalSynth01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth];
}

// Not when compiled into Lemon Studio (`linked`): see Cargo.toml.
#[cfg(not(feature = "linked"))]
nih_export_clap!(Zest);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(Zest);

#[cfg(feature = "linked")]
os_plugkit::linked_clap_entry!(Zest);
