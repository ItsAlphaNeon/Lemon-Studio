// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A hosted FL native plugin presented as an os-engine instrument / insert effect and an
//! `os_host::Plugin`, so the app hosts it alongside CLAP and VST3.
//!
//! The engine works in de-interleaved stereo; FL plugins work in interleaved stereo, so this converts on the boundary.
//! Generator voices follow spec `plugins/fruity-abi.md`, "Voices": the plugin gets the note's own levels and the final
//! ones (with the channel's volume and pan, and its pitch as each note's tuning), NewTick on tick boundaries at the
//! song's tempo, and its voices end when it asks. Given the channel's envelopes, LFOs and filter, the host steps them per
//! tick into each voice's final levels and runs the voices of a plugin that renders voice by voice through the filter
//! (`voicing`).
//
// spec: plugins/fruity-abi.md

use std::ffi::c_void;

use os_engine::instrument::{ChannelLevels, ChannelVoicing, HostedEffect, Instrument, NoteLevels, TransportInfo};
use os_host::{Descriptor, ParamInfo};

use crate::abi::{dispatch, flags, rec, IntPtr, LevelParams};
use crate::voicing::{Shape, Shaping};
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
    On(u8, f32, NoteLevels),
    Off(u8),
    Choke(u8),
    Tune(u8, f32),
    /// Parameter index, MIDI value 0..2^30.
    Param(u32, i32),
}

/// A normalized parameter value (0..1) as the MIDI value a native plugin takes: `Round(v * MIDI_FULL)` (spec:
/// "Parameters"; the scale, see `rec::MIDI_FULL`).
fn midi_value(v: f64) -> i32 {
    (v.clamp(0.0, 1.0) * rec::MIDI_FULL).round_ties_even() as i32
}

/// A sounding generator voice.
struct Live {
    key: u8,
    voice: Voice,
    /// Frames since its note off (`None` while held).
    released: Option<u64>,
    /// Its tuning from the engine (a glide, the channel's pitch), semitones.
    tune: f32,
    /// The channel's envelopes, LFOs and filter on it (once the engine gave them).
    shape: Option<Shape>,
    /// Its note is off and its volume envelope is releasing it: the plugin's own release waits for the envelope's end
    /// (an OpenStudio choice: the plugin's release would otherwise cut the envelope's release short).
    ending: bool,
    /// Its volume envelope ran out on the last tick (the plugin was told to release it then): the host kills it.
    ran_out: bool,
}

/// The levels a note's voice starts with (spec: "Voices"; `audio/voice-levels.md` "The note's own levels"): volume =
/// the level curve of the velocity (100 of 128 gives 1), the note's pan, pitch in cents from key 60, the note's mod X /
/// mod Y as the filter offsets.
pub fn note_levels(key: u8, velocity: f32, own: NoteLevels) -> LevelParams {
    let vel = velocity.clamp(0.0, 1.0) * 128.0;
    LevelParams { pan: own.pan, vol: os_dsp::level::curve(vel * 0.01), pitch: (f32::from(key) - 60.0) * 100.0, fcut: own.mod_x, fres: own.mod_y }
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
    /// The channel's levels the engine gave last (`channel_levels`).
    channel: ChannelLevels,
    /// Tempo the plugin was last told.
    bpm: f64,
    /// Frames per tick at that tempo.
    spt: f64,
    /// Frames until the next tick starts (NewTick is due at 0 or below).
    to_tick: f64,
    inter: Vec<f32>,
    /// One voice's output (`Voice_Render`).
    voice_buf: Vec<f32>,
    /// The channel's envelopes, LFOs and filter (`channel_voicing`); `None` until the engine gives them.
    shaping: Option<Shaping>,
    /// Frames until the next tick of the envelopes and LFOs (due at 0 or below).
    art_to_tick: f64,
    /// The engine starts a tick at the next `process` call's first frame.
    tick_now: bool,
    /// The editor is shown in a host window (ShowEditor with a parent, not yet hidden).
    editor: bool,
    /// The MIDI value the host last gave each parameter it moved (index, value).
    sent: Vec<(u32, i32)>,
    /// Values the open editor's controls still have to show (interface thread, `idle`).
    follow: Vec<(u32, i32)>,
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
            channel: ChannelLevels::default(),
            bpm: DEFAULT_BPM,
            spt: 1.0,
            to_tick: 0.0,
            inter: vec![0.0; MAX_BLOCK * 2],
            voice_buf: vec![0.0; MAX_BLOCK * 2],
            shaping: None,
            art_to_tick: 0.0,
            tick_now: false,
            editor: false,
            sent: Vec::new(),
            follow: Vec::new(),
        }
    }

    /// Moves parameter `index` to MIDI value `midi` (the render lock is taken: audio thread), and has the editor's
    /// control follow on the interface thread while the editor is open.
    fn send_param(&mut self, index: u32, midi: i32) {
        if index as usize >= self.plugin.param_count() {
            return;
        }
        self.plugin.set_param_midi(index as i32, midi);
        set_entry(&mut self.sent, index, midi);
        if self.editor {
            set_entry(&mut self.follow, index, midi);
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
        if let Some(s) = &mut self.shaping {
            s.set_tempo(bpm);
        }
    }

    /// Frames per tick of the channel's envelopes and LFOs (the project's ticks).
    fn art_spt(&self) -> f64 {
        self.shaping.as_ref().map_or(self.spt, |s| s.frames_per_tick(self.rate.max(1)))
    }

    /// A voice's final levels: its own, times the channel's volume, plus the channel's pan, mod X / Y and the voice's
    /// tuning (spec `audio/voice-levels.md` "Final levels").
    fn final_levels(&self, init: LevelParams, tune: f32) -> LevelParams {
        let c = self.channel;
        LevelParams {
            vol: init.vol * c.volume,
            pan: init.pan + c.pan,
            pitch: init.pitch + tune * 100.0,
            fcut: init.fcut + c.mod_x,
            fres: init.fres + c.mod_y,
        }
    }

    /// Works voice `i`'s final levels out again: the channel's, its tuning, and its envelopes' and LFOs' parts.
    fn refresh(&mut self, i: usize) {
        let init = self.voices[i].voice.init();
        let mut fin = self.final_levels(init, self.voices[i].tune);
        if let Some(s) = &self.voices[i].shape {
            fin = s.levels(fin);
        }
        self.voices[i].voice.set_final(fin);
    }

    /// A tick of the channel's envelopes and LFOs starts: every voice steps and gets its new final levels. A released
    /// voice whose volume envelope has run out gets the plugin's release on this tick and is killed on the next (spec:
    /// "Voices", the channel's envelopes, LFOs and filter; an OpenStudio choice: the plugin hears its levels at 0 for a
    /// tick before the voice goes).
    fn art_tick(&mut self) {
        let Some(mut sh) = self.shaping.take() else { return };
        let nominal = sh.frames_per_tick(self.rate.max(1));
        let channel = self.channel;
        sh.tick();
        let mut i = 0;
        while i < self.voices.len() {
            if self.voices[i].ran_out {
                self.kill_at(i);
                continue;
            }
            let init = self.voices[i].voice.init();
            let live = &mut self.voices[i];
            if let Some(s) = &mut live.shape {
                sh.step(s, &init, &channel, nominal);
                if live.ending && sh.volume_ended(s) {
                    live.ending = false;
                    live.ran_out = true;
                    live.released = Some(0);
                    self.plugin.release(&live.voice);
                }
            }
            self.refresh(i);
            i += 1;
        }
        self.shaping = Some(sh);
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
            Ev::On(key, velocity, own) => {
                if self.voices.len() >= MAX_VOICES {
                    self.kill_at(0);
                }
                let tag = self.next_tag;
                self.next_tag += 1;
                let init = note_levels(key, velocity, own);
                // The channel's envelopes and LFOs take the voice's first tick at once; a plugin that renders voice by
                // voice gets the channel filter (spec: "Voices").
                let (channel, nominal, filter) = (self.channel, self.art_spt(), self.voice_render.then_some(self.rate.max(1)));
                let shape = self.shaping.as_mut().map(|s| s.start(&init, &channel, filter, nominal));
                let mut fin = self.final_levels(init, 0.0);
                if let Some(s) = &shape {
                    fin = s.levels(fin);
                }
                if let Some(voice) = self.plugin.trigger(init, fin, velocity, tag) {
                    self.voices.push(Live { key, voice, released: None, tune: 0.0, shape, ending: false, ran_out: false });
                }
            }
            Ev::Off(key) => {
                let held = self.sounding(key).find(|&i| self.voices[i].released.is_none() && !self.voices[i].ending);
                if let Some(i) = held {
                    // The envelopes go into their release; with the volume envelope on the plugin's own release waits
                    // for its end.
                    let live = &mut self.voices[i];
                    let mut wait = false;
                    if let (Some(sh), Some(s)) = (&self.shaping, &mut live.shape) {
                        sh.release(s);
                        wait = sh.volume_on();
                    }
                    if wait {
                        live.ending = true;
                    } else {
                        live.released = Some(0);
                        self.plugin.release(&live.voice);
                    }
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
            Ev::Param(index, midi) => self.send_param(index, midi),
        }
    }

    /// Answers what the plugin asked of its voices during the last call: releases, then kills (spec: "Voices").
    fn answer_requests(&mut self) {
        let (kill, release) = self.plugin.take_voice_requests();
        for tag in release {
            if let Some(i) = self.voices.iter().position(|v| v.voice.tag == tag && v.released.is_none()) {
                let live = &mut self.voices[i];
                if let (Some(sh), Some(s), false) = (&self.shaping, &mut live.shape, live.ending) {
                    sh.release(s);
                }
                live.ending = false;
                live.released = Some(0);
                self.plugin.release(&live.voice);
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
            for (i, live) in self.voices.iter_mut().enumerate() {
                self.voice_buf[..len * 2].fill(0.0);
                let (got, more) = self.plugin.voice_render(&live.voice, &mut self.voice_buf[..len * 2]);
                // The channel filter, on the host's side (spec: "Voices").
                if let Some(s) = &mut live.shape {
                    s.filter(&mut self.voice_buf[..got.min(len) * 2]);
                }
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
        self.events.push((offset, Ev::On(key, velocity, NoteLevels::default())));
    }

    /// The note's own pan and mod X / Y become its voice's `Init` levels (and so part of its `Final` ones).
    fn note_start(&mut self, offset: u32, key: u8, velocity: f32, _start: f32, levels: NoteLevels) {
        self.events.push((offset, Ev::On(key, velocity, levels)));
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

    /// A tuning moves its voice's pitch (`Ev::Tune`), which the generator reads while the voice sounds.
    fn takes_tuning(&self) -> bool {
        true
    }

    /// Automated values arrive once per tick, at its start (spec: "Parameters").
    fn param(&mut self, offset: u32, index: u32, value: f64) {
        self.events.push((offset, Ev::Param(index, midi_value(value))));
    }

    fn tick_params(&self) -> bool {
        true
    }

    /// The plugin works the channel's levels into its voices' final levels, which it reads while they sound (sounding
    /// voices follow a change on the next block).
    fn channel_levels(&mut self, levels: ChannelLevels) -> bool {
        if levels != self.channel {
            self.channel = levels;
            for i in 0..self.voices.len() {
                self.refresh(i);
            }
        }
        true
    }

    /// The host runs the channel's envelopes, LFOs and filter on a generator's voices (spec: "Voices"); the engine's
    /// tick starts at the next `process` call's first frame.
    fn channel_voicing(&mut self, voicing: &ChannelVoicing) {
        if !self.desc.is_instrument() {
            return;
        }
        self.tick_now = true;
        match &mut self.shaping {
            Some(s) => s.set(voicing),
            None => self.shaping = Some(Shaping::new(*voicing, self.bpm)),
        }
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
        // A tick of the envelopes and LFOs: at the first frame when the engine starts one there (and then none more in
        // this call), else on the plugin's own clock of the project's ticks.
        let engine_tick = std::mem::take(&mut self.tick_now);
        let shaped = self.shaping.is_some();
        if engine_tick {
            self.art_to_tick = 0.0;
        }
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
            if shaped && self.art_to_tick <= 0.0 && (pos == 0 || !engine_tick) {
                self.art_tick();
                self.art_to_tick += self.art_spt();
            }
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
            if shaped && !engine_tick {
                end = end.min(pos + self.art_to_tick.ceil().max(1.0) as usize);
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
            if shaped {
                self.art_to_tick -= len as f64;
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
        // The global LFOs start again with the next first note (an OpenStudio choice: Stop ends a render).
        if let Some(s) = &mut self.shaping {
            s.reset();
        }
        self.art_to_tick = 0.0;
    }

    fn reported_delay(&self) -> u32 {
        self.plugin.latency()
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

    /// The engine hands an effect its tick's values before the tick is processed: they apply at once.
    fn param_fx(&mut self, _offset: u32, index: u32, value: f64) {
        self.send_param(index, midi_value(value));
    }

    fn tick_params_fx(&self) -> bool {
        true
    }

    fn reported_delay_fx(&self) -> u32 {
        self.plugin.latency()
    }
}

impl os_host::Plugin for NativePlugin {
    fn desc(&self) -> &Descriptor {
        &self.desc
    }

    fn latency(&self) -> u32 {
        self.plugin.latency()
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
        // The editor's controls follow the values the host gave (spec: "Parameters").
        for (index, midi) in std::mem::take(&mut self.follow) {
            if self.editor {
                self.plugin.show_param_midi(index as i32, midi);
            }
        }
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

    // Parameters (spec: "Parameters"): the index is the id; names and value texts from GetName; values cross as
    // normalized 0..1 and reach the plugin as MIDI values. The plugin's own range is not known to the host.
    fn param_count(&self) -> usize {
        self.plugin.param_count()
    }

    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        (index < self.plugin.param_count()).then(|| ParamInfo {
            id: index as u32,
            name: shown_name(&self.plugin.param_name(index as i32)),
            unit: String::new(),
            group: String::new(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            steps: 0,
            automatable: true,
            read_only: false,
            hidden: false,
            bypass: false,
            enumerated: false,
        })
    }

    /// The value the host last gave the parameter; `None` before it gave one (a native plugin's own value comes in its
    /// own units, not normalized).
    fn param_value(&self, id: u32) -> Option<f64> {
        self.sent.iter().find(|e| e.0 == id).map(|e| f64::from(e.1) / rec::MIDI_FULL)
    }

    /// The plugin's text for the parameter's current value; it gives no text for other values, so only the value the
    /// host last gave it has one.
    fn param_text(&self, id: u32, value: f64) -> Option<String> {
        let current = self.sent.iter().find(|e| e.0 == id)?.1;
        (current == midi_value(value) && (id as usize) < self.plugin.param_count())
            .then(|| self.plugin.param_text(id as i32))
            .filter(|t| !t.is_empty())
    }

    fn set_param(&mut self, id: u32, value: f64) -> bool {
        if id as usize >= self.plugin.param_count() {
            return false;
        }
        self.send_param(id, midi_value(value));
        true
    }
}

/// A parameter name as shown: the installed plugins start their names with caret codes ("^b^aCutoff"), which are
/// left out (an OpenStudio choice; what the codes mean is not known).
fn shown_name(raw: &str) -> String {
    let mut s = raw;
    while let Some(rest) = s.strip_prefix('^') {
        let mut chars = rest.chars();
        if chars.next().is_none() {
            break;
        }
        s = chars.as_str();
    }
    s.trim().to_string()
}

/// Sets `index`'s value in a small (index, value) list.
fn set_entry(list: &mut Vec<(u32, i32)>, index: u32, value: i32) {
    match list.iter_mut().find(|e| e.0 == index) {
        Some(e) => e.1 = value,
        None => list.push((index, value)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_values_and_names() {
        assert_eq!(midi_value(0.0), 0);
        assert_eq!(midi_value(1.0), 1 << 30);
        assert_eq!(midi_value(0.5), 1 << 29);
        assert_eq!(midi_value(2.0), 1 << 30);
        assert_eq!(midi_value(-1.0), 0);
        // The value an automation clip hands on goes through unchanged.
        let n = 123_456_789i64;
        assert_eq!(i64::from(midi_value(n as f64 / 1_073_741_824.0)), n);
        assert_eq!(shown_name("^b^aCutoff"), "Cutoff");
        // A note's own pan and mod X / Y are its voice's own levels (spec: "Voices").
        let own = NoteLevels { pan: -0.5, mod_x: 0.25, mod_y: -1.0 };
        let l = note_levels(64, 100.0 / 128.0, own);
        assert_eq!((l.pan, l.fcut, l.fres, l.pitch), (-0.5, 0.25, -1.0, 400.0));
        assert!((l.vol - 1.0).abs() < 1e-6, "velocity 100 gives 1: {}", l.vol);
        assert_eq!(shown_name("Plain"), "Plain");
        assert_eq!(shown_name("^"), "^");
    }
}
