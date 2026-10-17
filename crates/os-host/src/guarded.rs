// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A hosted instance whose every call runs under the crash guard ([`crate::guard`]).
//
// spec: plugins/plugin-formats.md ("Crash containment")

use std::cell::RefCell;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::guard::{self, Module};
use crate::{Attention, Descriptor, HostedEffect, Instrument, Meter, ParamEvent, ParamInfo, Plugin, SharedPlugin};
use os_engine::{ChannelLevels, ChannelVoicing, TransportInfo};

/// A plugin instance of any format, called through the crash guard. Once its code crashed (this instance, or another
/// one of the same plugin file) it is stopped: it is never called again, nor destroyed (its memory is left as the crash
/// left it). A stopped instrument plays silence, a stopped effect passes the sound through, and its saved state is the
/// last one it gave or was given.
///
/// It also keeps the instance's [`Meter`]: the time every processing call takes and the voices after it (spec:
/// ui/plugin-performance.md §3, audio/voices.md §5).
pub struct Guarded<P: Plugin> {
    inner: ManuallyDrop<P>,
    module: Arc<Module>,
    /// The state the instance last saved or loaded: what a stopped instance saves.
    state: RefCell<Option<Vec<u8>>>,
    meter: Arc<Meter>,
    /// The instance's attention count, raised here too when it crashes (the UI then reports the stop).
    attention: Option<Arc<Attention>>,
}

/// Shares `plugin`, code of `module`, as a guarded instance (format crates' `Library::create`).
pub fn share<P: Plugin + 'static>(plugin: P, module: Arc<Module>) -> SharedPlugin {
    let attention = plugin.attention();
    Arc::new(Mutex::new(Guarded { inner: ManuallyDrop::new(plugin), module, state: RefCell::new(None), meter: Arc::default(), attention }))
}

impl<P: Plugin> Guarded<P> {
    /// Runs `f` on the instance under the guard; `stopped` when its code crashed (now or before).
    fn run<R>(&mut self, stopped: R, f: impl FnOnce(&mut P) -> R) -> R {
        let inner = &mut *self.inner;
        guard::call(&self.module, || f(inner)).unwrap_or_else(|_| {
            if let Some(a) = &self.attention {
                a.raise();
            }
            stopped
        })
    }

    /// `run` for the methods that only look.
    fn look<R>(&self, stopped: R, f: impl FnOnce(&P) -> R) -> R {
        let inner = &*self.inner;
        guard::call(&self.module, || f(inner)).unwrap_or(stopped)
    }
}

impl<P: Plugin> Drop for Guarded<P> {
    fn drop(&mut self) {
        if self.module.crashed().is_none() {
            // SAFETY: dropped once, here; never used again.
            let inner = &mut self.inner;
            let _ = guard::call(&self.module, || unsafe { ManuallyDrop::drop(inner) });
        }
    }
}

impl<P: Plugin> Instrument for Guarded<P> {
    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        self.run((), |p| p.note_on(offset, key, velocity))
    }
    fn note_off(&mut self, offset: u32, key: u8) {
        self.run((), |p| p.note_off(offset, key))
    }
    fn note_on_from(&mut self, offset: u32, key: u8, velocity: f32, start: f32) {
        self.run((), |p| p.note_on_from(offset, key, velocity, start))
    }
    fn note_start(&mut self, offset: u32, key: u8, velocity: f32, start: f32, levels: os_engine::NoteLevels) {
        self.run((), |p| p.note_start(offset, key, velocity, start, levels))
    }
    fn choke(&mut self, offset: u32, key: u8) {
        self.run((), |p| p.choke(offset, key))
    }
    fn voice_start(&mut self, offset: u32, key: u8, velocity: f32, start: f32, levels: os_engine::NoteLevels, voice: u32) {
        self.run((), |p| p.voice_start(offset, key, velocity, start, levels, voice))
    }
    fn voice_gains(&self) -> bool {
        self.look(false, |p| p.voice_gains())
    }
    fn voice_gain(&mut self, offset: u32, voice: u32, gain: f32) {
        self.run((), |p| p.voice_gain(offset, voice, gain))
    }
    fn voice_choke(&mut self, offset: u32, key: u8, voice: u32) {
        self.run((), |p| p.voice_choke(offset, key, voice))
    }
    fn tune(&mut self, offset: u32, key: u8, semitones: f32) {
        self.run((), |p| p.tune(offset, key, semitones))
    }
    fn process(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        let t0 = Instant::now();
        let voices = self.run(None, |p| {
            p.process(rate, left, right);
            Some(p.voices())
        });
        self.meter.add(t0.elapsed());
        match voices {
            Some(n) => self.meter.set_voices(n),
            None => {
                // Silence, also over whatever the crash left half written.
                left.fill(0.0);
                right.fill(0.0);
                self.meter.set_voices(0);
            }
        }
    }
    fn active(&self) -> bool {
        self.look(false, |p| p.active())
    }
    fn voices(&self) -> usize {
        self.look(0, |p| p.voices())
    }
    fn reset(&mut self) {
        self.run((), |p| p.reset());
        let n = self.look(0, |p| p.voices());
        self.meter.set_voices(n);
    }
    fn param(&mut self, offset: u32, index: u32, value: f64) {
        self.run((), |p| p.param(offset, index, value))
    }
    fn tick_params(&self) -> bool {
        self.look(false, |p| p.tick_params())
    }
    fn transport(&mut self, info: &TransportInfo) {
        self.run((), |p| p.transport(info))
    }
    fn channel_levels(&mut self, levels: ChannelLevels) -> bool {
        self.run(false, |p| p.channel_levels(levels))
    }
    fn channel_voicing(&mut self, voicing: &ChannelVoicing) {
        self.run((), |p| p.channel_voicing(voicing))
    }
}

impl<P: Plugin> HostedEffect for Guarded<P> {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        // A crash midway leaves the block as far as the plugin got; later blocks pass through untouched.
        let t0 = Instant::now();
        self.run((), |p| p.process_fx(rate, left, right));
        self.meter.add(t0.elapsed());
    }
    fn reset_fx(&mut self) {
        self.run((), |p| p.reset_fx())
    }
    fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
        self.run((), |p| p.param_fx(offset, index, value))
    }
    fn tick_params_fx(&self) -> bool {
        self.look(false, |p| p.tick_params_fx())
    }
    fn transport_fx(&mut self, info: &TransportInfo) {
        self.run((), |p| p.transport_fx(info))
    }
}

impl<P: Plugin> Plugin for Guarded<P> {
    /// The descriptor is the host's own record, not plugin code.
    fn desc(&self) -> &Descriptor {
        self.inner.desc()
    }
    fn activate(&mut self, rate: u32) -> bool {
        self.run(false, |p| p.activate(rate))
    }
    fn idle(&mut self) {
        self.run((), |p| p.idle())
    }
    fn has_editor(&self) -> bool {
        self.look(false, |p| p.has_editor())
    }
    fn editor_open(&self) -> bool {
        self.look(false, |p| p.editor_open())
    }
    fn open_editor(&mut self, parent: *mut c_void, scale: f32) -> Result<(u32, u32), String> {
        let stopped = Err(format!("{} has stopped", self.inner.desc().name));
        let r = self.run(stopped, |p| p.open_editor(parent, scale));
        // The editor's windows go under the guard at once.
        guard::watch_windows(true);
        r
    }
    fn editor_size(&self) -> Option<(u32, u32)> {
        self.look(None, |p| p.editor_size())
    }
    fn close_editor(&mut self) {
        self.run((), |p| p.close_editor())
    }
    fn save_state(&self) -> Option<Vec<u8>> {
        let saved = self.look(None, |p| p.save_state());
        match &saved {
            Some(s) => *self.state.borrow_mut() = Some(s.clone()),
            None if self.stopped().is_some() => return self.state.borrow().clone(),
            None => {}
        }
        saved
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        let ok = self.run(false, |p| p.load_state(data));
        if ok {
            *self.state.borrow_mut() = Some(data.to_vec());
        }
        ok
    }
    fn latency(&self) -> u32 {
        self.look(0, |p| p.latency())
    }
    fn use_start_param(&mut self, name: &str) -> bool {
        self.run(false, |p| p.use_start_param(name))
    }
    fn use_note_levels(&mut self) -> bool {
        self.run(false, |p| p.use_note_levels())
    }
    fn use_voice_gains(&mut self) -> bool {
        self.run(false, |p| p.use_voice_gains())
    }
    fn set_offline(&mut self, offline: bool) {
        self.run((), |p| p.set_offline(offline))
    }
    fn param_count(&self) -> usize {
        self.look(0, |p| p.param_count())
    }
    fn param_info(&self, index: usize) -> Option<ParamInfo> {
        self.look(None, |p| p.param_info(index))
    }
    fn param_value(&self, id: u32) -> Option<f64> {
        self.look(None, |p| p.param_value(id))
    }
    fn param_text(&self, id: u32, value: f64) -> Option<String> {
        self.look(None, |p| p.param_text(id, value))
    }
    fn set_param(&mut self, id: u32, value: f64) -> bool {
        self.run(false, |p| p.set_param(id, value))
    }
    fn take_param_events(&mut self) -> Vec<ParamEvent> {
        self.run(Vec::new(), |p| p.take_param_events())
    }
    fn take_automation_requests(&mut self) -> Vec<u32> {
        self.run(Vec::new(), |p| p.take_automation_requests())
    }
    /// Its module's crash, else a stop the instance reports itself (a native plugin stops one instance when it raises
    /// an error it does not handle: spec plugins/fruity-abi.md, "Crash containment").
    fn stopped(&self) -> Option<u32> {
        self.module.crashed().or_else(|| self.look(None, |p| p.stopped()))
    }
    fn meter(&self) -> Option<Arc<Meter>> {
        Some(self.meter.clone())
    }
    fn attention(&self) -> Option<Arc<Attention>> {
        self.attention.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A plugin that faults in its processing once `crash` is set, as one with a bad pointer does.
    struct Crashy {
        desc: Descriptor,
        crash: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
        attention: Arc<Attention>,
    }

    impl Crashy {
        fn maybe_crash(&self) {
            if self.crash.load(Ordering::Relaxed) {
                // SAFETY: deliberately not: the read faults and the guard catches it.
                unsafe { std::ptr::read_volatile(8 as *const usize) };
            }
        }
    }

    impl Drop for Crashy {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Relaxed);
        }
    }

    impl Instrument for Crashy {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, l: &mut [f32], r: &mut [f32]) {
            l.fill(0.5);
            self.maybe_crash();
            r.fill(0.5);
        }
    }

    impl HostedEffect for Crashy {
        fn process_fx(&mut self, _: u32, l: &mut [f32], r: &mut [f32]) {
            self.maybe_crash();
            l.iter_mut().chain(r.iter_mut()).for_each(|v| *v *= 2.0);
        }
    }

    impl Plugin for Crashy {
        fn desc(&self) -> &Descriptor {
            &self.desc
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {}
        fn has_editor(&self) -> bool {
            true
        }
        fn editor_open(&self) -> bool {
            false
        }
        fn open_editor(&mut self, _: *mut c_void, _: f32) -> Result<(u32, u32), String> {
            Ok((10, 10))
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {}
        fn save_state(&self) -> Option<Vec<u8>> {
            self.maybe_crash();
            Some(vec![1, 2, 3])
        }
        fn load_state(&mut self, _: &[u8]) -> bool {
            true
        }
        fn attention(&self) -> Option<Arc<Attention>> {
            Some(self.attention.clone())
        }
    }

    fn crashy() -> (SharedPlugin, Arc<AtomicBool>, Arc<AtomicBool>) {
        let desc = Descriptor {
            format: crate::Format::Vst3,
            id: "0".into(),
            name: "Crashy".into(),
            vendor: String::new(),
            version: String::new(),
            features: vec![],
            kind: crate::Kind::Effect,
            path: Default::default(),
        };
        let (crash, dropped) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let p = share(Crashy { desc, crash: crash.clone(), dropped: dropped.clone(), attention: Arc::default() }, Arc::new(Module::default()));
        (p, crash, dropped)
    }

    #[test]
    fn a_running_instance_is_called_and_destroyed() {
        let (p, _, dropped) = crashy();
        let (mut l, mut r) = ([0.25f32; 4], [0.25f32; 4]);
        p.lock().unwrap().process_fx(48000, &mut l, &mut r);
        assert_eq!((l, r), ([0.5; 4], [0.5; 4]));
        assert_eq!(p.lock().unwrap().save_state(), Some(vec![1, 2, 3]));
        assert_eq!(p.lock().unwrap().stopped(), None);
        drop(p);
        assert!(dropped.load(Ordering::Relaxed));
    }

    /// A crash stops the instance: effects pass the sound through, instruments are silent, the last saved state is kept,
    /// and the instance is never destroyed.
    #[cfg(plugin_guard)]
    #[test]
    fn a_crashed_instance_stops() {
        let (p, crash, dropped) = crashy();
        assert_eq!(p.lock().unwrap().save_state(), Some(vec![1, 2, 3]));
        let attention = p.lock().unwrap().attention().expect("the instance's count, through the guard");
        let before = attention.count();
        crash.store(true, Ordering::Relaxed);
        let (mut l, mut r) = ([0.25f32; 4], [0.25f32; 4]);
        p.lock().unwrap().process_fx(48000, &mut l, &mut r);
        assert_eq!(p.lock().unwrap().stopped(), Some(0xC000_0005));
        assert!(attention.count() > before, "the UI hears of the stop without polling");
        crash.store(false, Ordering::Relaxed);
        p.lock().unwrap().process_fx(48000, &mut l, &mut r);
        assert_eq!((l, r), ([0.25; 4], [0.25; 4]), "passed through");
        p.lock().unwrap().process(48000, &mut l, &mut r);
        assert_eq!((l, r), ([0.0; 4], [0.0; 4]), "silent");
        assert_eq!(p.lock().unwrap().save_state(), Some(vec![1, 2, 3]), "the last state it saved");
        assert!(p.lock().unwrap().open_editor(std::ptr::null_mut(), 1.0).is_err());
        assert!(!p.lock().unwrap().has_editor());
        drop(p);
        assert!(!dropped.load(Ordering::Relaxed), "a crashed instance is left as it is");
    }

    /// An instrument that stops itself (it contained an error of its own) and takes the channel's levels into its voices.
    struct SelfStopping {
        desc: Descriptor,
        stop: Option<u32>,
        levels: Option<(f32, f32)>,
    }

    impl Instrument for SelfStopping {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
        fn channel_levels(&mut self, levels: ChannelLevels) -> bool {
            self.levels = Some((levels.volume, levels.pan));
            true
        }
    }

    impl HostedEffect for SelfStopping {
        fn process_fx(&mut self, _: u32, _: &mut [f32], _: &mut [f32]) {}
    }

    impl Plugin for SelfStopping {
        fn desc(&self) -> &Descriptor {
            &self.desc
        }
        fn activate(&mut self, _: u32) -> bool {
            true
        }
        fn idle(&mut self) {
            self.stop = Some(0x0EED_FADE);
        }
        fn has_editor(&self) -> bool {
            false
        }
        fn editor_open(&self) -> bool {
            false
        }
        fn open_editor(&mut self, _: *mut c_void, _: f32) -> Result<(u32, u32), String> {
            Err("none".into())
        }
        fn editor_size(&self) -> Option<(u32, u32)> {
            None
        }
        fn close_editor(&mut self) {}
        fn save_state(&self) -> Option<Vec<u8>> {
            self.levels.map(|(v, p)| vec![(v * 10.0).round() as u8, ((p + 1.0) * 10.0).round() as u8])
        }
        fn load_state(&mut self, _: &[u8]) -> bool {
            true
        }
        fn stopped(&self) -> Option<u32> {
            self.stop
        }
    }

    /// The channel's levels reach an instrument that takes them, and a stop the instance reports itself counts as
    /// stopped (spec: plugins/fruity-abi.md, "Voices" and "Crash containment").
    #[test]
    fn channel_levels_and_own_stops_reach_through() {
        let desc = Descriptor {
            format: crate::Format::Native,
            id: "0".into(),
            name: "Self".into(),
            vendor: String::new(),
            version: String::new(),
            features: vec![],
            kind: crate::Kind::Instrument,
            path: Default::default(),
        };
        let p = share(SelfStopping { desc, stop: None, levels: None }, Arc::new(Module::default()));
        let mut g = p.lock().unwrap();
        assert!(g.channel_levels(ChannelLevels { volume: 0.5, pan: -0.3, ..ChannelLevels::default() }));
        assert_eq!(g.save_state(), Some(vec![5, 7]));
        assert_eq!(g.stopped(), None);
        g.idle();
        assert_eq!(g.stopped(), Some(0x0EED_FADE));
    }
}
