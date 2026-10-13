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
    /// Released voices still ringing until the plugin kills them: (voice handle, seconds left before the host stops
    /// counting one the plugin never kills; spec: audio/voices.md §2).
    ringing: Vec<(IntPtr, f64)>,
    next_tag: IntPtr,
    wants_tick: bool,
    /// Samples per FL tick at the nominal tempo (generators only).
    spt: f64,
    tick_acc: f64,
    inter: Vec<f32>,
    /// The editor is shown in a host window (ShowEditor with a parent, not yet hidden).
    editor: bool,
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
            ringing: Vec::new(),
            next_tag: 1,
            wants_tick,
            spt: 1.0,
            tick_acc: 0.0,
            inter: vec![0.0; MAX_BLOCK * 2],
            editor: false,
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
            self.ringing.push((voice, RINGING_SECONDS));
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
        // The voices the plugin ended itself (Voice_Kill), and released ones past the time limit.
        let secs = n as f64 / f64::from(rate.max(1));
        self.forget_killed();
        for (_, left) in &mut self.ringing {
            *left -= secs;
        }
        self.ringing.retain(|&(_, left)| left > 0.0);
    }

    fn active(&self) -> bool {
        !self.voices.is_empty()
    }

    fn voices(&self) -> usize {
        self.voices.len() + self.ringing.len()
    }

    /// Stop: every voice, held or ringing, is killed.
    fn reset(&mut self) {
        for (_, voice) in self.voices.drain(..) {
            self.plugin.kill(voice);
        }
        for (voice, _) in self.ringing.drain(..) {
            self.plugin.kill(voice);
        }
        self.plugin.killed_voices();
    }
}

/// Longest a released voice is counted while the plugin does not kill it (spec: audio/voices.md §2.5).
const RINGING_SECONDS: f64 = 30.0;

impl NativePlugin {
    /// Drops the voices the plugin reported ended (its Voice_Kill calls to the host).
    fn forget_killed(&mut self) {
        for v in self.plugin.killed_voices() {
            self.voices.retain(|&(_, h)| h != v);
            self.ringing.retain(|&(h, _)| h != v);
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

    // Editors (spec plugins/fruity-abi.md, "Editors"): ShowEditor with the host's window as parent; the
    // plugin creates its editor as a child window of it, sized by itself. ShowEditor(0) hides it again
    // (the plugin keeps the window), so the host's frame can be docked, detached or closed safely.
    fn has_editor(&self) -> bool {
        self.plugin.info.flags & (flags::NO_WINDOW | flags::INTERFACELESS) == 0
    }
    fn editor_open(&self) -> bool {
        self.editor
    }
    fn open_editor(&mut self, parent: *mut c_void, _scale: f32) -> Result<(u32, u32), String> {
        if parent.is_null() {
            return Err("no parent window".into());
        }
        if !self.has_editor() {
            return Err("the plugin has no editor".into());
        }
        self.plugin.dispatch(dispatch::SHOW_EDITOR, 0, parent as IntPtr);
        let h = self.plugin.editor_handle();
        if h == 0 {
            return Err("the plugin did not create an editor".into());
        }
        self.editor = true;
        win::place_at_origin(h);
        win::size(h).ok_or_else(|| "the plugin's editor window is gone".into())
    }
    fn editor_size(&self) -> Option<(u32, u32)> {
        if self.editor { win::size(self.plugin.editor_handle()) } else { None }
    }
    fn close_editor(&mut self) {
        if self.editor {
            self.plugin.dispatch(dispatch::SHOW_EDITOR, 0, 0);
            self.editor = false;
        }
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        Some(self.plugin.save_state())
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        self.plugin.restore_state(data);
        true
    }
}

/// Window helpers for the plugin's editor window (Windows only; elsewhere there is no editor).
mod win {
    #[cfg(windows)]
    pub fn size(h: isize) -> Option<(u32, u32)> {
        use windows_sys::Win32::Foundation::{HWND, RECT};
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsWindow};
        let h = h as HWND;
        if h.is_null() || unsafe { IsWindow(h) } == 0 {
            return None;
        }
        let mut r: RECT = unsafe { std::mem::zeroed() };
        if unsafe { GetWindowRect(h, &mut r) } == 0 {
            return None;
        }
        Some(((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32))
    }

    /// The editor fills the host's area from its top-left corner.
    #[cfg(windows)]
    pub fn place_at_origin(h: isize) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER};
        unsafe { SetWindowPos(h as _, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
    }

    #[cfg(not(windows))]
    pub fn size(_h: isize) -> Option<(u32, u32)> {
        None
    }
    #[cfg(not(windows))]
    pub fn place_at_origin(_h: isize) {}
}

// Native plugins do not surface individual parameters through os_host yet (params/automation use
// the defaults). The types are referenced here so the intent is explicit.
#[allow(dead_code)]
fn _param_api(_p: &ParamInfo, _e: &ParamEvent) {}
