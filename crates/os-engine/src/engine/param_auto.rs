// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Automation clips that move plugin parameters (song mode): each clip's curve is sampled every [`PARAM_STEP`] frames
//! inside a block and every value that changed becomes a parameter event at its frame offset, for the channel's
//! instrument or the mixer slot's hosted effect. The plugins smooth their parameters themselves.
//!
// spec: plugins/plugin-parameters.md §2 (automation playback), §3 (targets)

use os_model::target::Target;

use super::{Engine, Fx, FxSlot};

/// Frames between two samples of a parameter curve (a Lemon Studio choice).
pub const PARAM_STEP: usize = 64;

/// Where a parameter event goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dest {
    /// Rack position of the channel whose instrument plays it.
    Channel(usize),
    /// Mixer track, slot.
    Slot(usize, usize),
}

/// One parameter change inside the current block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ParamEvent {
    pub dest: Dest,
    /// Frames from the block start.
    pub offset: u32,
    /// Index in the plugin's parameter list.
    pub index: u32,
    /// Normalized 0..1.
    pub value: f64,
}

/// Engine state for parameter automation.
#[derive(Debug, Default)]
pub(super) struct ParamAuto {
    /// This block's events, in frame order.
    pub events: Vec<ParamEvent>,
    /// Last value sent per target (a value is only sent again when it changes).
    last: Vec<(Target, f64)>,
    /// Scratch: targets already handled at the current step.
    done: Vec<Target>,
    /// The playlist has clips on plugin parameters at all.
    pub any: bool,
    /// This tick's values for plugins that take one value per tick (`Instrument::tick_params`): the walked value the
    /// clip hands on, at offset 0, when it differs from the last one handed on (spec: plugins/fruity-abi.md
    /// "Parameters", audio/automation.md "When a clip acts").
    pub ticks: Vec<ParamEvent>,
    /// Last per-tick value handed on per target (`Round(level * 2^30)`).
    last_tick: Vec<(Target, i64)>,
}

impl ParamAuto {
    pub fn new(any: bool) -> Self {
        ParamAuto {
            events: Vec::with_capacity(256),
            last: Vec::with_capacity(16),
            done: Vec::with_capacity(16),
            any,
            ticks: Vec::with_capacity(16),
            last_tick: Vec::with_capacity(16),
        }
    }

    /// Values are sent again from scratch (after a seek: the plugin gets the value at the new position).
    pub fn forget(&mut self) {
        self.last.clear();
        self.last_tick.clear();
    }

    /// The value `n` (`Round(level * 2^30)`) an automation clip hands on to plugin parameter `target` for the tick
    /// starting now, kept for per-tick plugins when it changed.
    pub fn tick_value(&mut self, target: Target, n: i64) {
        let (dest, index) = match target {
            Target::ChannelParam(c, k) => (Dest::Channel(usize::from(c)), k),
            Target::SlotParam(t, s, k) => (Dest::Slot(usize::from(t), usize::from(s)), k),
            _ => return,
        };
        match self.last_tick.iter_mut().find(|(t, _)| *t == target) {
            Some((_, last)) if *last == n => return,
            Some((_, last)) => *last = n,
            None => self.last_tick.push((target, n)),
        }
        self.ticks.push(ParamEvent { dest, offset: 0, index: u32::from(index), value: n as f64 / 1_073_741_824.0 });
    }

    /// Channel `c`'s per-tick values for this tick.
    pub fn channel_ticks(&self, c: usize) -> impl Iterator<Item = (u32, f64)> + '_ {
        self.ticks.iter().filter(move |e| e.dest == Dest::Channel(c)).map(|e| (e.index, e.value))
    }

    /// Hands mixer track `t`'s slot events of this block to their hosted effects (before the track is processed).
    pub fn deliver_slots(&mut self, t: usize, effects: &[FxSlot]) {
        if self.events.is_empty() && self.ticks.is_empty() {
            return;
        }
        for fx in effects {
            let Fx::Hosted(h) = &fx.fx else { continue };
            let dest = Dest::Slot(t, fx.slot);
            if !self.events.iter().chain(&self.ticks).any(|e| e.dest == dest) {
                continue;
            }
            // A busy instance misses these values: they are sent again at the next step.
            let Ok(mut h) = h.try_lock() else {
                let mine = |tg: &Target| matches!(*tg, Target::SlotParam(a, b, _) if usize::from(a) == t && usize::from(b) == fx.slot);
                self.last.retain(|(tg, _)| !mine(tg));
                self.last_tick.retain(|(tg, _)| !mine(tg));
                continue;
            };
            let list = if h.tick_params_fx() { &self.ticks } else { &self.events };
            for e in list.iter().filter(|e| e.dest == dest) {
                h.param_fx(e.offset, e.index, e.value);
            }
        }
    }

    /// Channel `c`'s events for the span `a..b` of the block (offsets from `a`), oldest first.
    pub fn channel_span(&self, c: usize, a: usize, b: usize) -> impl Iterator<Item = (u32, u32, f64)> + '_ {
        let (a, b) = (a as u32, b as u32);
        self.events
            .iter()
            .filter(move |e| e.dest == Dest::Channel(c) && e.offset >= a && e.offset < b)
            .map(move |e| (e.offset - a, e.index, e.value))
    }

    /// A busy instrument missed channel `c`'s values: send them again at the next step.
    pub fn resend_channel(&mut self, c: usize) {
        let mine = |tg: &Target| matches!(*tg, Target::ChannelParam(ch, _) if usize::from(ch) == c);
        self.last.retain(|(tg, _)| !mine(tg));
        self.last_tick.retain(|(tg, _)| !mine(tg));
    }
}

pub(super) fn is_param(t: Target) -> bool {
    matches!(t, Target::ChannelParam(..) | Target::SlotParam(..))
}

impl Engine {
    /// Samples every plugin-parameter clip over the `frames` frames of the tick starting now into `self.params.events`.
    pub(super) fn run_param_automation(&mut self, frames: usize) {
        self.params.events.clear();
        if !self.params.any {
            return;
        }
        let (now, len, ppq) = (self.tick as f64, frames.max(1) as f64, f64::from(self.ppq));
        let pa = &mut self.params;
        for step in (0..frames).step_by(PARAM_STEP) {
            // The position in ticks: the tick plus the share of it the step has reached.
            let f = now + step as f64 / len;
            pa.done.clear();
            // Later clips win where clips on the same target overlap (as for the other targets).
            for a in self.autos.iter().rev() {
                if f < a.start as f64 || f >= a.end as f64 {
                    continue;
                }
                // One tick ahead, as every automation target (spec: audio/automation.md "When a clip acts").
                let beat = a.from_beat + (f + 1.0 - a.start as f64) / ppq;
                let mut value = None;
                for &target in &a.targets {
                    if !is_param(target) || pa.done.contains(&target) {
                        continue;
                    }
                    pa.done.push(target);
                    if value.is_none() {
                        value = self.curves[a.curve].value_at(beat).map(|v| v.clamp(0.0, 1.0));
                    }
                    let Some(v) = value else { break };
                    match pa.last.iter_mut().find(|(t, _)| *t == target) {
                        Some((_, last)) if (*last - v).abs() < 1e-7 => continue,
                        Some((_, last)) => *last = v,
                        None => pa.last.push((target, v)),
                    }
                    let (dest, index) = match target {
                        Target::ChannelParam(c, k) => (Dest::Channel(usize::from(c)), k),
                        Target::SlotParam(t, s, k) => (Dest::Slot(usize::from(t), usize::from(s)), k),
                        _ => continue,
                    };
                    pa.events.push(ParamEvent { dest, offset: step as u32, index: u32::from(index), value: v });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use os_model::{AutoPoint, Automation, Channel, ChannelKind, Clip, ClipSource, Project};

    use crate::{Engine, HostedEffect, Instrument, PlayMode, SampleLibrary};

    /// Records parameter events as (absolute frame, index, value).
    #[derive(Default)]
    struct Probe {
        frame: u64,
        seen: Vec<(u64, u32, f64)>,
        pending: Vec<(u32, u32, f64)>,
        /// Takes one value per tick (`tick_params`).
        ticks: bool,
    }

    impl Probe {
        fn take(&mut self, n: usize) {
            for (o, i, v) in self.pending.drain(..) {
                self.seen.push((self.frame + u64::from(o), i, v));
            }
            self.frame += n as u64;
        }
    }

    impl Instrument for Probe {
        fn note_on(&mut self, _: u32, _: u8, _: f32) {}
        fn note_off(&mut self, _: u32, _: u8) {}
        fn process(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
            l.fill(0.0);
            r.fill(0.0);
            self.take(l.len());
        }
        fn param(&mut self, offset: u32, index: u32, value: f64) {
            self.pending.push((offset, index, value));
        }
        fn tick_params(&self) -> bool {
            self.ticks
        }
    }

    impl HostedEffect for Probe {
        fn process_fx(&mut self, _rate: u32, l: &mut [f32], _r: &mut [f32]) {
            self.take(l.len());
        }
        fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
            self.pending.push((offset, index, value));
        }
        fn tick_params_fx(&self) -> bool {
            self.ticks
        }
    }

    fn channel(index: u16, kind: ChannelKind) -> Channel {
        Channel {
            index,
            kind,
            name: None,
            plugin: None,
            color: None,
            enabled: true,
            volume: 0.78125,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: 0,
            sample_path: None,
            layer_children: vec![],
            envelopes: Default::default(),
            rack: Default::default(),
            plugin_state: None,
            misc: Default::default(), sound: Default::default(),
        }
    }

    /// A ramp 0 -> 1 over 4 beats on `target`, its clip at tick 0 for 4 beats (PPQ 96, 120 BPM at 48 kHz = 24000
    /// frames per beat).
    fn project(target: u32) -> Project {
        let auto = Automation {
            channel: 1,
            targets: vec![target],
            points: vec![AutoPoint::new(0.0, 0.0), AutoPoint::new(4.0, 1.0)],
            header: vec![0; 21],
            tail: vec![],
        };
        Project {
            ppq: 96,
            tempo: Some(120.0),
            channels: vec![channel(0, ChannelKind::Plugin), channel(1, ChannelKind::Automation)],
            automations: vec![auto],
            arrangements: vec![os_model::Arrangement { clips: vec![Clip::new(0, 4 * 96, 0, ClipSource::Channel(1))], ..Default::default() }],
            mixer: (0..3).map(os_model::MixerTrack::new).collect(),
            ..Project::default()
        }
    }

    fn run(e: &mut Engine, frames: usize, block: usize) {
        let mut out = vec![0.0f32; block * 2];
        for _ in 0..frames / block {
            e.process(&mut out);
        }
    }

    /// A clip on a channel plugin parameter reaches the instrument as frame-accurate events every 64 frames, with the
    /// curve's value one tick (250 frames) after that frame, and stops at the clip end.
    #[test]
    fn channel_parameter_clip_drives_the_instrument() {
        let p = project(os_model::target::channel_param(0, 7));
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        assert_eq!(e.report.automation_clips_ignored, 0);
        let probe = Arc::new(Mutex::new(Probe::default()));
        e.set_instrument(0, probe.clone());
        run(&mut e, 48000 * 3, 1000);
        let seen = probe.lock().unwrap().seen.clone();
        assert!(seen.len() > 1000, "{} events", seen.len());
        assert!(seen.iter().all(|&(_, i, _)| i == 7));
        // Every event carries the ramp's value one tick ahead of its own frame.
        for &(f, _, v) in &seen {
            let want = ((f + 250) as f64 / (4.0 * 24000.0)).min(1.0);
            assert!((v - want).abs() < 1e-6, "frame {f}: {v} vs {want}");
        }
        // The clip ends at 4 beats = 96000 frames: nothing after it.
        assert!(seen.iter().all(|&(f, _, _)| f < 96000));
        assert_eq!(seen[0].0, 0);
        assert!(seen.windows(2).all(|w| w[1].0 - w[0].0 <= 64), "a value at least every 64 frames");
    }

    /// A slot plugin parameter target drives the hosted effect of that slot; a seek resends the value there.
    #[test]
    fn slot_parameter_clip_drives_the_effect() {
        let p = project(os_model::target::slot_param(1, 3, 12));
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let probe = Arc::new(Mutex::new(Probe::default()));
        e.set_effect(1, 3, probe.clone());
        run(&mut e, 24000, 1000);
        let seen = probe.lock().unwrap().seen.clone();
        assert!(seen.len() > 300, "{}", seen.len());
        assert!(seen.iter().all(|&(_, i, _)| i == 12));
        let &(f, _, v) = seen.last().unwrap();
        assert!((v - (f + 250) as f64 / 96000.0).abs() < 1e-6);
        // Constant curve after a seek: one event, then nothing until it changes.
        let mut p = p;
        p.automations[0].points = vec![AutoPoint::new(0.0, 0.25), AutoPoint::new(4.0, 0.25)];
        let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
        let probe = Arc::new(Mutex::new(Probe::default()));
        e.set_effect(1, 3, probe.clone());
        e.seek(48000);
        run(&mut e, 10000, 1000);
        let seen = probe.lock().unwrap().seen.clone();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!((seen[0].2 - 0.25).abs() < 1e-9);
    }

    /// A plugin that takes one value per tick gets the value the clip hands on for that tick, at the tick start, only
    /// when it changed (spec: plugins/fruity-abi.md "Parameters"); instruments and slot effects alike.
    #[test]
    fn per_tick_plugins_get_the_handed_on_value_at_each_tick_start() {
        let mut lib = SampleLibrary::new(None);
        for slot in [false, true] {
            let target = if slot { os_model::target::slot_param(1, 3, 12) } else { os_model::target::channel_param(0, 7) };
            let p = project(target);
            let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
            let probe = Arc::new(Mutex::new(Probe { ticks: true, ..Probe::default() }));
            if slot {
                e.set_effect(1, 3, probe.clone());
            } else {
                e.set_instrument(0, probe.clone());
            }
            run(&mut e, 48000 * 3, 1000);
            let seen = probe.lock().unwrap().seen.clone();
            // 4 beats of 96 ticks of 250 frames: a new value at every tick of the ramp, none after the clip.
            assert_eq!(seen.len(), 384, "slot {slot}");
            for (t, &(f, i, v)) in seen.iter().enumerate() {
                assert_eq!(f, 250 * t as u64, "slot {slot}");
                assert_eq!(i, if slot { 12 } else { 7 });
                // The walk is single precision; the value handed on is a whole multiple of 2^-30.
                let want = (t + 1) as f64 / 384.0;
                assert!((v - want).abs() < 1e-6, "tick {t}: {v} vs {want}");
                assert_eq!((v * 1_073_741_824.0).fract(), 0.0);
            }
            // A constant curve: one value, at the first tick.
            let mut p = p;
            p.automations[0].points = vec![AutoPoint::new(0.0, 0.25), AutoPoint::new(4.0, 0.25)];
            let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Song);
            let probe = Arc::new(Mutex::new(Probe { ticks: true, ..Probe::default() }));
            if slot {
                e.set_effect(1, 3, probe.clone());
            } else {
                e.set_instrument(0, probe.clone());
            }
            run(&mut e, 48000, 1000);
            // After a seek the value is handed on again.
            e.seek(48000);
            run(&mut e, 10000, 1000);
            let seen = probe.lock().unwrap().seen.clone();
            assert_eq!(seen.len(), 2, "slot {slot}: {seen:?}");
            assert!(seen.iter().all(|s| (s.2 - 0.25).abs() < 1e-9));
        }
    }
}
