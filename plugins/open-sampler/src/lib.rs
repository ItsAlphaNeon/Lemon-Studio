//! Open Sampler: OpenStudio's sampler instrument as a VST3 / CLAP plugin (nih-plug).
//!
// spec: plugins/open-sampler.md, plugins/plugin-formats.md
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
use os_engine::Sample;

use params::{FxKey, SamplerParams};

/// The processed sample for one (loaded sample version, effect settings) pair.
pub struct Processed {
    pub version: u64,
    pub key: FxKey,
    pub sample: Option<Arc<Sample>>,
}

/// State shared by the audio thread, the background task and the editor.
#[derive(Default)]
pub struct Shared {
    pub raw: Mutex<Option<Arc<Sample>>>,
    /// Bumped on every load / clear.
    pub raw_version: AtomicU64,
    pub processed: Mutex<Option<Processed>>,
    processed_version: AtomicU64,
    loaded_path: Mutex<String>,
}

impl Shared {
    /// Loads a sample file and makes it the current one (UI / initialize thread).
    pub fn load(&self, path: &str, params: &SamplerParams) -> Result<(), String> {
        let s = os_engine::sample::load(Path::new(path)).map_err(|e| e.to_string())?;
        *self.raw.lock().unwrap() = Some(Arc::new(s));
        *self.loaded_path.lock().unwrap() = path.to_string();
        if let Ok(mut p) = params.sample_path.write() {
            *p = path.to_string();
        }
        self.raw_version.fetch_add(1, Ordering::AcqRel);
        self.precompute(params);
        Ok(())
    }

    pub fn clear(&self, params: &SamplerParams) {
        *self.raw.lock().unwrap() = None;
        self.loaded_path.lock().unwrap().clear();
        if let Ok(mut p) = params.sample_path.write() {
            p.clear();
        }
        self.raw_version.fetch_add(1, Ordering::AcqRel);
        self.precompute(params);
    }

    /// Rebuilds the processed sample from the loaded one and the current effect settings.
    pub fn precompute(&self, params: &SamplerParams) {
        let version = self.raw_version.load(Ordering::Acquire);
        let key = params.fx_key();
        let raw = self.raw.lock().unwrap().clone();
        let sample = raw.map(|r| Arc::new(dsp::precompute(&r, key)));
        *self.processed.lock().unwrap() = Some(Processed { version, key, sample });
        self.processed_version.fetch_add(1, Ordering::AcqRel);
    }

    /// Changes whenever the shown sample changes (editor repaint check).
    pub fn version(&self) -> u64 {
        self.processed_version.load(Ordering::Acquire) ^ (self.raw_version.load(Ordering::Acquire) << 32)
    }
}

pub enum Task {
    Precompute,
}

pub struct OpenSampler {
    params: Arc<SamplerParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    /// (sample version, effect settings) last asked for / currently playing.
    requested: Option<(u64, FxKey)>,
    current: Option<(u64, FxKey)>,
    gains: Vec<(f32, f32)>,
}

impl Default for OpenSampler {
    fn default() -> Self {
        OpenSampler {
            params: Arc::new(SamplerParams::default()),
            shared: Arc::new(Shared::default()),
            engine: dsp::Engine::new(44100.0),
            requested: None,
            current: None,
            gains: Vec::new(),
        }
    }
}

impl OpenSampler {
    fn block(&self) -> dsp::Block {
        let p = &self.params;
        dsp::Block {
            on: true,
            root: p.root.value(),
            cents: f64::from(p.fine.value()),
            vol_env: p.env_vol.envelope(),
            pitch_env: p.env_pitch.envelope(),
            start_offset: p.start_offset.value(),
            cutoff: p.f_cut.value(),
            resonance: p.f_res.value(),
        }
    }

    /// Takes a newer processed sample when the background task has one ready (never blocks).
    fn pick_up_sample(&mut self, ctx: &mut impl ProcessContext<Self>) {
        let want = (self.shared.raw_version.load(Ordering::Acquire), self.params.fx_key());
        if self.requested != Some(want) {
            self.requested = Some(want);
            ctx.execute_background(Task::Precompute);
        }
        if let Ok(p) = self.shared.processed.try_lock() {
            if let Some(p) = p.as_ref() {
                if self.current != Some((p.version, p.key)) {
                    self.current = Some((p.version, p.key));
                    self.engine.sample = p.sample.clone();
                }
            }
        }
    }

    fn render_span(&mut self, out: &mut [&mut [f32]], a: usize, b: usize, blk: &dsp::Block) {
        if a >= b {
            return;
        }
        // Channel volume / pan are the host's: the plugin plays at unity.
        self.gains[..b - a].fill((1.0, 1.0));
        let (left, right) = out.split_at_mut(1);
        self.engine.render(&mut left[0][a..b], &mut right[0][a..b], blk, &self.gains[..b - a]);
    }
}

impl Plugin for OpenSampler {
    const NAME: &'static str = "Open Sampler";
    const VENDOR: &'static str = "OpenStudio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = Task;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let (params, shared) = (self.params.clone(), self.shared.clone());
        Box::new(move |task| match task {
            Task::Precompute => shared.precompute(&params),
        })
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::SamplerEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.engine.set_rate(config.sample_rate);
        self.gains = vec![(0.0, 0.0); config.max_buffer_size as usize];
        // A restored state names a sample: load it now (initialize may block).
        let path = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
        let loaded = self.shared.loaded_path.lock().unwrap().clone();
        if !path.is_empty() && path != loaded {
            if let Err(e) = self.shared.load(&path, &self.params) {
                nih_log!("Open Sampler: {e}");
            }
        } else {
            self.shared.precompute(&self.params);
        }
        if let Some(p) = self.shared.processed.lock().unwrap().as_ref() {
            self.current = Some((p.version, p.key));
            self.requested = self.current;
            self.engine.sample = p.sample.clone();
        }
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        self.pick_up_sample(ctx);
        let blk = self.block();
        let len = buffer.samples();
        if self.gains.len() < len {
            self.gains.resize(len, (0.0, 0.0));
        }
        let out = buffer.as_slice();
        for ch in out.iter_mut() {
            ch.fill(0.0);
        }
        let mut at = 0;
        while let Some(ev) = ctx.next_event() {
            let t = (ev.timing() as usize).min(len);
            self.render_span(out, at, t, &blk);
            at = t;
            match ev {
                NoteEvent::NoteOn { note, velocity, .. } => self.engine.note_on(note, velocity, &blk),
                NoteEvent::NoteOff { note, .. } => self.engine.note_off(note),
                _ => {}
            }
        }
        self.render_span(out, at, len, &blk);
        ProcessStatus::Normal
    }
}

impl ClapPlugin for OpenSampler {
    const CLAP_ID: &'static str = "org.openstudio.open-sampler";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("OpenStudio's sampler instrument");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Sampler, ClapFeature::Stereo];
}

impl Vst3Plugin for OpenSampler {
    const VST3_CLASS_ID: [u8; 16] = *b"OpenStudioSamplr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Sampler];
}

nih_export_clap!(OpenSampler);
nih_export_vst3!(OpenSampler);
