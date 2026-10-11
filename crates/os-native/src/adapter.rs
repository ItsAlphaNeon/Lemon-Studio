//! A hosted FL native plugin presented as an os-engine instrument / insert effect and an
//! `os_host::Plugin`, so the app hosts it alongside CLAP and VST3.
//!
//! Effects host and render correctly; generators load and play the correct pitch but their voice
//! level/lifecycle is not yet right (see spec `plugins/fruity-abi.md`, "Status"), so generator
//! output is clamped defensively. The engine works in de-interleaved stereo; FL plugins work in
//! interleaved stereo, so this converts on the boundary.
//
// spec: plugins/fruity-abi.md

use std::ffi::c_void;

use os_engine::instrument::{HostedEffect, Instrument};
use os_host::{Descriptor, ParamEvent, ParamInfo};

use crate::abi::{dispatch, flags, IntPtr};
use crate::Plugin;

/// Largest block handed to the plugin in one render; longer engine buffers are split into chunks of
/// this size (the plugin was told this as its block size at load).
pub const MAX_BLOCK: usize = 8192;

/// A hosted FL native plugin. One type covers both roles: the engine calls `process_fx` when it
/// sits in a mixer slot and `process` / `note_*` when it is a channel generator; the unused role's
/// methods are simply never called.
///
/// The underlying instance is driven from one thread at a time by the engine (the audio thread only
/// ever holds it alone), so moving it between threads is sound even though the plugin is not
/// internally thread-safe — the same contract os-clap uses.
pub struct NativePlugin {
    plugin: Plugin,
    desc: Descriptor,
    rate: u32,
    active: bool,
    /// Live generator voices: (key, voice handle).
    voices: Vec<(u8, IntPtr)>,
    next_tag: IntPtr,
    wants_tick: bool,
    /// Samples per FL tick at the nominal tempo (generators only).
    spt: f64,
    tick_acc: f64,
    inter: Vec<f32>,
}

// SAFETY: the engine serialises all access behind a mutex; never two threads at once.
unsafe impl Send for NativePlugin {}

impl NativePlugin {
    /// Wraps a freshly loaded plugin with its descriptor. Not active yet; call `activate`.
    pub fn new(plugin: Plugin, desc: Descriptor) -> Self {
        let wants_tick = plugin.info.flags & flags::WANT_NEW_TICK != 0;
        NativePlugin {
            plugin,
            desc,
            rate: 0,
            active: false,
            voices: Vec::new(),
            next_tag: 1,
            wants_tick,
            spt: 1.0,
            tick_acc: 0.0,
            inter: vec![0.0; MAX_BLOCK * 2],
        }
    }

    fn set_rate(&mut self, rate: u32) {
        if rate != self.rate {
            self.plugin.dispatch(dispatch::SET_SAMPLE_RATE, 0, rate as IntPtr);
            self.spt = (f64::from(rate) * 60.0 / 130.0 / 96.0).max(1.0);
            self.rate = rate;
        }
    }
}

impl Instrument for NativePlugin {
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
        self.set_rate(rate);
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
            let got = self.plugin.gen_render(&mut self.inter[..len * 2]).min(len);
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

impl HostedEffect for NativePlugin {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        self.set_rate(rate);
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

impl os_host::Plugin for NativePlugin {
    fn desc(&self) -> &Descriptor {
        &self.desc
    }

    fn activate(&mut self, rate: u32) -> bool {
        self.set_rate(rate);
        if !self.active {
            if self.desc.is_instrument() {
                self.plugin.set_tempo(130.0, 96);
            }
            self.plugin.set_enabled(true);
            self.plugin.set_playing(true);
            self.active = true;
        }
        true
    }

    fn idle(&mut self) {
        self.plugin.idle();
    }

    // Editor embedding is not implemented yet for native plugins (experimental); the host shows its
    // own empty frame. The plugin still loads and processes audio.
    fn has_editor(&self) -> bool {
        false
    }
    fn editor_open(&self) -> bool {
        false
    }
    fn open_editor(&mut self, _parent: *mut c_void) -> Result<(u32, u32), String> {
        Err("native plugin editors are not supported yet".into())
    }
    fn editor_size(&self) -> Option<(u32, u32)> {
        None
    }
    fn close_editor(&mut self) {}

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(self.plugin.save_state())
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        self.plugin.restore_state(data);
        true
    }
}

// Native plugins do not surface individual parameters through os_host yet (params/automation use
// the defaults). The types are referenced here so the intent is explicit.
#[allow(dead_code)]
fn _param_api(_p: &ParamInfo, _e: &ParamEvent) {}
