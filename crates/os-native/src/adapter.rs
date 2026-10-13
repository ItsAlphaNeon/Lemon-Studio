//! A hosted FL native plugin presented as an os-engine instrument / insert effect and an
//! `os_host::Plugin`, so the app hosts it alongside CLAP and VST3.
//!
//! The engine works in de-interleaved stereo; FL plugins work in interleaved stereo, so this converts on the boundary.
//! Generator voices follow spec `plugins/fruity-abi.md`, "Voices": the plugin gets the note's own levels and the final
//! ones (with the channel's volume and pan, and its pitch as each note's tuning), NewTick on tick boundaries at the
//! song's tempo, and its voices end when it asks.
//
// spec: plugins/fruity-abi.md

use std::ffi::c_void;

use os_engine::instrument::{HostedEffect, Instrument, TransportInfo};
use os_host::{Descriptor, ParamEvent, ParamInfo};

use crate::abi::{dispatch, flags, IntPtr, LevelParams};
use crate::{Plugin, Voice};

/// Largest block handed to the plugin in one render; longer engine buffers are split into chunks of
/// this size (the plugin was told this as its block size at load).
pub const MAX_BLOCK: usize = 8192;

/// Ticks per quarter note of the tick clock generators are driven by (spec: "Voices").
const PPQ: u16 = 96;
/// Tempo a generator runs at until the song tells it one.
const DEFAULT_BPM: f64 = 130.0;
/// A released voice the plugin has not asked to kill after this many seconds is killed by the host.
const RELEASE_LIMIT_SECONDS: u64 = 30;
/// Most voices one generator keeps; a new note past it kills the oldest.
const MAX_VOICES: usize = 256;
/// Output beyond this is clipped: a plugin gone wrong must not blow up the mix.
const OUTPUT_LIMIT: f32 = 4.0;

/// A note event for the next `process` call, at its frame offset.
#[derive(Clone, Copy)]
enum Ev {
    On(u8, f32),
    Off(u8),
    Choke(u8),
    Tune(u8, f32),
}

/// A sounding generator voice.
struct Live {
    key: u8,
    voice: Voice,
    /// Frames since its note off (`None` while held).
    released: Option<u64>,
    /// Its tuning from the engine (a glide, the channel's pitch), semitones.
    tune: f32,
}

/// The levels a note's voice starts with (spec: "Voices"; `audio/voice-levels.md` "The note's own levels"): volume =
/// the level curve of the velocity (100 of 128 gives 1), centred, pitch in cents from key 60, no filter offsets.
pub fn note_levels(key: u8, velocity: f32) -> LevelParams {
    let vel = velocity.clamp(0.0, 1.0) * 128.0;
    LevelParams { pan: 0.0, vol: os_dsp::level::curve(vel * 0.01), pitch: (f32::from(key) - 60.0) * 100.0, fcut: 0.0, fres: 0.0 }
}

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
    /// Sounding generator voices, oldest first.
    voices: Vec<Live>,
    /// Note events for the next `process`.
    events: Vec<(u32, Ev)>,
    next_tag: IntPtr,
    wants_tick: bool,
    /// The plugin renders each voice itself (`Voice_Render`) instead of all of them at once (`Gen_Render`).
    voice_render: bool,
    /// The plugin renders audio in its role: an effect that is no generator, a generator, neither with the NoProcess
    /// flag (spec: flags). Otherwise its render methods are never called: an effect passes the sound through.
    renders: bool,
    /// The channel's volume (a gain) and pan the engine gave last (`channel_levels`).
    channel: (f32, f32),
    /// Tempo the plugin was last told.
    bpm: f64,
    /// Frames per tick at that tempo.
    spt: f64,
    /// Frames until the next tick starts (NewTick is due at 0 or below).
    to_tick: f64,
    inter: Vec<f32>,
    /// One voice's output (`Voice_Render`).
    voice_buf: Vec<f32>,
    /// The editor is shown in a host window (ShowEditor with a parent, not yet hidden).
    editor: bool,
}

// SAFETY: the engine serialises all access behind a mutex; never two threads at once.
unsafe impl Send for NativePlugin {}

impl NativePlugin {
    /// Wraps a freshly loaded plugin with its descriptor. Not active yet; call `activate`.
    pub fn new(plugin: Plugin, desc: Descriptor) -> Self {
        let wants_tick = plugin.info.flags & flags::WANT_NEW_TICK != 0;
        let voice_render = plugin.info.uses_voice_render();
        let renders = !plugin.info.no_process() && (desc.is_instrument() || !plugin.info.is_generator());
        NativePlugin {
            plugin,
            desc,
            rate: 0,
            active: false,
            voices: Vec::new(),
            events: Vec::new(),
            next_tag: 1,
            wants_tick,
            voice_render,
            renders,
            channel: (1.0, 0.0),
            bpm: DEFAULT_BPM,
            spt: 1.0,
            to_tick: 0.0,
            inter: vec![0.0; MAX_BLOCK * 2],
            voice_buf: vec![0.0; MAX_BLOCK * 2],
            editor: false,
        }
    }

    fn set_rate(&mut self, rate: u32) {
        if rate != self.rate {
            self.plugin.set_sample_rate(rate);
            self.rate = rate;
            if self.desc.is_instrument() {
                self.set_tempo(self.bpm);
            }
        }
    }

    /// Tells a generator the tempo (event Tempo + SetSamplesPerTick) and keeps its tick clock at it.
    fn set_tempo(&mut self, bpm: f64) {
        self.bpm = bpm;
        self.spt = (f64::from(self.rate.max(1)) * 60.0 / bpm / f64::from(PPQ)).max(1.0);
        self.plugin.set_tempo(bpm as f32, PPQ);
    }

    /// A voice's final levels: its own, times the channel's volume, plus the channel's pan and the voice's tuning
    /// (spec `audio/voice-levels.md` "Final levels").
    fn final_levels(&self, init: LevelParams, tune: f32) -> LevelParams {
        let (volume, pan) = self.channel;
        LevelParams { vol: init.vol * volume, pan: init.pan + pan, pitch: init.pitch + tune * 100.0, ..init }
    }

    fn refresh(&mut self, i: usize) {
        let init = self.voices[i].voice.init();
        let fin = self.final_levels(init, self.voices[i].tune);
        self.voices[i].voice.set_final(fin);
    }

    fn sounding(&self, key: u8) -> impl Iterator<Item = usize> + '_ {
        self.voices.iter().enumerate().filter(move |(_, v)| v.key == key).map(|(i, _)| i)
    }

    fn kill_at(&mut self, i: usize) {
        let live = self.voices.remove(i);
        self.plugin.kill(live.voice);
    }

    fn apply(&mut self, ev: Ev) {
        match ev {
            Ev::On(key, velocity) => {
                if self.voices.len() >= MAX_VOICES {
                    self.kill_at(0);
                }
                let tag = self.next_tag;
                self.next_tag += 1;
                let init = note_levels(key, velocity);
                if let Some(voice) = self.plugin.trigger(init, self.final_levels(init, 0.0), velocity, tag) {
                    self.voices.push(Live { key, voice, released: None, tune: 0.0 });
                }
            }
            Ev::Off(key) => {
                let held = self.sounding(key).find(|&i| self.voices[i].released.is_none());
                if let Some(i) = held {
                    self.voices[i].released = Some(0);
                    self.plugin.release(&self.voices[i].voice);
                }
            }
            Ev::Choke(key) => {
                let first = self.sounding(key).next();
                if let Some(i) = first {
                    self.kill_at(i);
                }
            }
            Ev::Tune(key, semitones) => {
                for i in self.sounding(key).collect::<Vec<_>>() {
                    self.voices[i].tune = semitones;
                    self.refresh(i);
                }
            }
        }
    }

    /// Answers what the plugin asked of its voices during the last call: releases, then kills (spec: "Voices").
    fn answer_requests(&mut self) {
        let (kill, release) = self.plugin.take_voice_requests();
        for tag in release {
            if let Some(i) = self.voices.iter().position(|v| v.voice.tag == tag && v.released.is_none()) {
                self.voices[i].released = Some(0);
                self.plugin.release(&self.voices[i].voice);
            }
        }
        for (tag, free) in kill {
            if let Some(i) = self.voices.iter().position(|v| v.voice.tag == tag) {
                if free {
                    self.kill_at(i);
                } else {
                    let live = self.voices.remove(i);
                    self.plugin.forget(live.voice);
                }
            }
        }
    }

    /// Renders `len` frames of every voice into `inter` (interleaved), zeros where the plugin gave nothing.
    fn render(&mut self, len: usize) {
        self.inter[..len * 2].fill(0.0);
        if self.voice_render {
            let mut done = Vec::new();
            for (i, live) in self.voices.iter().enumerate() {
                self.voice_buf[..len * 2].fill(0.0);
                let (got, more) = self.plugin.voice_render(&live.voice, &mut self.voice_buf[..len * 2]);
                for (o, s) in self.inter[..got.min(len) * 2].iter_mut().zip(&self.voice_buf) {
                    *o += *s;
                }
                if !more {
                    done.push(i);
                }
            }
            for i in done.into_iter().rev() {
                self.kill_at(i);
            }
        } else {
            // "Can render less than length": ask again for the rest.
            let mut filled = 0;
            while filled < len {
                let got = self.plugin.gen_render(&mut self.inter[filled * 2..len * 2]);
                if got == 0 {
                    break;
                }
                filled += got;
            }
        }
        self.answer_requests();
        for live in &mut self.voices {
            if let Some(r) = &mut live.released {
                *r += len as u64;
            }
        }
        let limit = u64::from(self.rate) * RELEASE_LIMIT_SECONDS;
        while let Some(i) = self.voices.iter().position(|v| v.released.is_some_and(|r| r > limit)) {
            self.kill_at(i);
        }
    }
}

impl Instrument for NativePlugin {
    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        self.events.push((offset, Ev::On(key, velocity)));
    }

    fn note_off(&mut self, offset: u32, key: u8) {
        self.events.push((offset, Ev::Off(key)));
    }

    fn choke(&mut self, offset: u32, key: u8) {
        self.events.push((offset, Ev::Choke(key)));
    }

    fn tune(&mut self, offset: u32, key: u8, semitones: f32) {
        self.events.push((offset, Ev::Tune(key, semitones)));
    }

    /// The plugin works the channel's volume and pan into its voices' final levels, which it reads while they sound
    /// (sounding voices follow a change on the next block).
    fn channel_levels(&mut self, volume: f32, pan: f32) -> bool {
        if (volume, pan) != self.channel {
            self.channel = (volume, pan);
            for i in 0..self.voices.len() {
                self.refresh(i);
            }
        }
        true
    }

    fn transport(&mut self, info: &TransportInfo) {
        if info.tempo > 0.0 && (info.tempo - self.bpm).abs() > 1e-6 && self.rate > 0 {
            self.set_tempo(info.tempo);
        }
    }

    /// Renders in spans that end at the next note event or tick boundary: NewTick before each tick, notes on their
    /// frame (spec: "Voices").
    fn process(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        left.fill(0.0);
        right.fill(0.0);
        if !self.renders || self.plugin.stopped().is_some() {
            self.events.clear();
            return;
        }
        self.set_rate(rate);
        let mut events = std::mem::take(&mut self.events);
        events.sort_by_key(|e| e.0);
        let mut next = 0;
        let mut pos = 0;
        while pos < n {
            while let Some(&(off, ev)) = events.get(next).filter(|e| e.0 as usize <= pos) {
                let _ = off;
                self.apply(ev);
                next += 1;
            }
            if self.wants_tick && self.to_tick <= 0.0 {
                self.plugin.new_tick();
                self.to_tick += self.spt;
            }
            let mut end = events.get(next).map_or(n, |e| e.0 as usize).min(n).min(pos + MAX_BLOCK);
            if self.wants_tick {
                end = end.min(pos + self.to_tick.ceil() as usize);
            }
            let end = end.max(pos + 1);
            let len = end - pos;
            self.render(len);
            for i in 0..len {
                let (l, r) = (self.inter[2 * i], self.inter[2 * i + 1]);
                left[pos + i] = if l.is_finite() { l.clamp(-OUTPUT_LIMIT, OUTPUT_LIMIT) } else { 0.0 };
                right[pos + i] = if r.is_finite() { r.clamp(-OUTPUT_LIMIT, OUTPUT_LIMIT) } else { 0.0 };
            }
            if self.wants_tick {
                self.to_tick -= len as f64;
            }
            pos = end;
        }
        for &(_, ev) in &events[next..] {
            self.apply(ev);
        }
        events.clear();
        self.events = events;
        self.plugin.idle();
    }

    fn active(&self) -> bool {
        !self.voices.is_empty()
    }

    /// Held and released voices both count until the plugin kills them or the host does after
    /// `RELEASE_LIMIT_SECONDS` (spec: audio/voices.md §2).
    fn voices(&self) -> usize {
        self.voices.len()
    }

    fn reset(&mut self) {
        self.events.clear();
        while !self.voices.is_empty() {
            self.kill_at(0);
        }
        self.plugin.take_voice_requests();
    }
}

impl Drop for NativePlugin {
    fn drop(&mut self) {
        if self.plugin.stopped().is_some() {
            // A stopped plugin may still hold the addresses of its voices' levels.
            std::mem::forget(std::mem::take(&mut self.voices));
        }
    }
}

impl HostedEffect for NativePlugin {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        // Plugins that process no buffers (controllers, visual plugins) and stopped plugins pass the sound through.
        if !self.renders || self.plugin.stopped().is_some() {
            return;
        }
        self.set_rate(rate);
        let n = left.len().min(right.len());
        let mut off = 0;
        while off < n {
            let len = (n - off).min(MAX_BLOCK);
            for i in 0..len {
                self.inter[2 * i] = left[off + i];
                self.inter[2 * i + 1] = right[off + i];
            }
            if !self.plugin.eff_render(&mut self.inter[..len * 2]) {
                return;
            }
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
                self.set_tempo(self.bpm);
            }
            self.plugin.set_enabled(true);
            self.plugin.set_playing(true);
            self.active = true;
        }
        self.plugin.stopped().is_none()
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
        if let Some(why) = self.plugin.stop_reason() {
            return Err(format!("the plugin stopped ({why})"));
        }
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
        self.plugin.save_state()
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        self.plugin.restore_state(data)
    }

    fn stopped(&self) -> Option<u32> {
        self.plugin.stopped()
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
