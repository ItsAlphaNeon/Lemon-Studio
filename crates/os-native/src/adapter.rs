//! Adapters that let a hosted FL native plugin act as an os-engine insert effect or instrument.
//!
//! Effects host and render correctly; generators load and play the correct pitch but their voice
//! level/lifecycle is not yet right (see spec `plugins/fruity-abi.md`, "Status"). Both are
//! experimental. The engine works in de-interleaved stereo (separate left/right slices); FL
//! plugins work in interleaved stereo, so these adapters convert on the boundary.
//
// spec: plugins/fruity-abi.md

use os_engine::instrument::{HostedEffect, Instrument};

use crate::abi::{dispatch, flags, IntPtr};
use crate::Plugin;

/// Largest block handed to the plugin in one `Eff_Render` / `Gen_Render`; longer engine buffers are
/// split into chunks of this size (the plugin was told this as its block size at load).
pub const MAX_BLOCK: usize = 8192;

/// A hosted FL native effect as an engine insert effect.
///
/// The underlying plugin instance is driven from one thread at a time by the engine (the audio
/// thread only ever holds it alone), so moving it between threads is sound even though the plugin
/// itself is not internally thread-safe — the same contract os-clap uses.
pub struct NativeEffect {
    plugin: Plugin,
    rate: u32,
    inter: Vec<f32>,
}

// SAFETY: the engine serialises all access behind a mutex; never two threads at once.
unsafe impl Send for NativeEffect {}

impl NativeEffect {
    /// Wraps a loaded effect plugin. `rate` is the sample rate it was loaded at.
    pub fn new(mut plugin: Plugin, rate: u32) -> Self {
        plugin.set_enabled(true);
        plugin.set_playing(true);
        NativeEffect { plugin, rate, inter: vec![0.0; MAX_BLOCK * 2] }
    }
}

impl HostedEffect for NativeEffect {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        if rate != self.rate {
            self.plugin.dispatch(dispatch::SET_SAMPLE_RATE, 0, rate as IntPtr);
            self.rate = rate;
        }
        let n = left.len().min(right.len());
        let mut off = 0;
        while off < n {
            let len = (n - off).min(MAX_BLOCK);
            for i in 0..len {
                self.inter[2 * i] = left[off + i];
                self.inter[2 * i + 1] = right[off + i];
            }
            self.plugin.eff_render(&mut self.inter[..len * 2]);
            for i in 0..len {
                left[off + i] = self.inter[2 * i];
                right[off + i] = self.inter[2 * i + 1];
            }
            off += len;
        }
    }

    fn reset_fx(&mut self) {
        self.plugin.dispatch(dispatch::FLUSH, 0, 0);
    }
}

/// A hosted FL native generator as an engine instrument. Experimental: pitch is correct, but the
/// output level is not yet right (the plugin's voice-level pipeline is not fully modelled), so the
/// adapter applies a defensive full-scale clamp to protect the output. A WantNewTick generator is
/// given `NewTick` on tick boundaries so its voices advance.
pub struct NativeInstrument {
    plugin: Plugin,
    rate: u32,
    /// Live voices: (key, voice handle, tag).
    voices: Vec<(u8, IntPtr)>,
    next_tag: IntPtr,
    wants_tick: bool,
    /// Samples per FL tick, from the nominal tempo.
    spt: f64,
    tick_acc: f64,
    inter: Vec<f32>,
}

// SAFETY: see NativeEffect.
unsafe impl Send for NativeInstrument {}

impl NativeInstrument {
    /// Wraps a loaded generator plugin at `rate`, using a nominal tempo for its tick clock.
    pub fn new(mut plugin: Plugin, rate: u32) -> Self {
        let wants_tick = plugin.info.flags & flags::WANT_NEW_TICK != 0;
        plugin.set_tempo(130.0, 96);
        plugin.set_enabled(true);
        plugin.set_playing(true);
        let spt = f64::from(rate) * 60.0 / 130.0 / 96.0;
        NativeInstrument {
            plugin,
            rate,
            voices: Vec::new(),
            next_tag: 1,
            wants_tick,
            spt: spt.max(1.0),
            tick_acc: 0.0,
            inter: vec![0.0; MAX_BLOCK * 2],
        }
    }
}

impl Instrument for NativeInstrument {
    fn note_on(&mut self, _offset: u32, key: u8, velocity: f32) {
        let tag = self.next_tag;
        self.next_tag += 1;
        let voice = self.plugin.trigger(f32::from(key), velocity, 0.0, tag);
        self.voices.push((key, voice));
    }

    fn note_off(&mut self, _offset: u32, key: u8) {
        if let Some(pos) = self.voices.iter().position(|&(k, _)| k == key) {
            let (_, voice) = self.voices.remove(pos);
            self.plugin.release(voice);
        }
    }

    fn process(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        if rate != self.rate {
            self.plugin.dispatch(dispatch::SET_SAMPLE_RATE, 0, rate as IntPtr);
            self.spt = (f64::from(rate) * 60.0 / 130.0 / 96.0).max(1.0);
            self.rate = rate;
        }
        let n = left.len().min(right.len());
        let mut off = 0;
        while off < n {
            let len = (n - off).min(MAX_BLOCK);
            if self.wants_tick {
                self.tick_acc += len as f64;
                while self.tick_acc >= self.spt {
                    self.plugin.new_tick();
                    self.tick_acc -= self.spt;
                }
            }
            for s in self.inter[..len * 2].iter_mut() {
                *s = 0.0;
            }
            let got = self.plugin.gen_render(&mut self.inter[..len * 2]);
            let got = got.min(len);
            for i in 0..got {
                // Defensive clamp: the generator level is not yet correct and can blow up.
                left[off + i] = self.inter[2 * i].clamp(-1.0, 1.0);
                right[off + i] = self.inter[2 * i + 1].clamp(-1.0, 1.0);
            }
            self.plugin.idle();
            off += len;
        }
    }

    fn active(&self) -> bool {
        !self.voices.is_empty()
    }

    fn reset(&mut self) {
        for (_, voice) in self.voices.drain(..) {
            self.plugin.kill(voice);
        }
    }
}
