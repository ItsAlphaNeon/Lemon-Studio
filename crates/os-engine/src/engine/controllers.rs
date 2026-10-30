// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Controller links: a controller plugin in a mixer slot (Fruity Peak Controller, Lemon Peak Controller) drives targets
//! through the project's links (event 227). At every tick's start each link reads its output as the controller's last
//! tick left it, puts it through the link's smoothing and formula, and hands it to the target as an automation value
//! would be: so a value the controller works out in one tick acts from the next.
//!
// spec: plugins/lemon-peak-controller.md "Links", flp/events/automation.md "Controller links"

use os_model::target::Target;

use super::param_auto::{self, Dest, ParamEvent};
use super::{Engine, Fx, MixTrack};
use crate::formula::Formula;

/// The smoothing's step per tick: two one-pole stages in a row (an approximation of the original's Smoothing at its
/// default time; spec: plugins/lemon-peak-controller.md "Links").
const SMOOTH: f64 = 0.075;

/// One link the engine plays.
#[derive(Debug, Clone)]
pub(super) struct CtlLink {
    track: usize,
    slot: usize,
    output: usize,
    target: Target,
    formula: Option<Formula>,
    smooth: Option<Option<[f64; 2]>>,
    /// The parameter value last handed on (plugin parameters only get a value when it changes).
    last: Option<i64>,
}

/// The project's links from controllers in mixer slots to targets the engine plays. A link from elsewhere (a
/// generator's controller) or to a target the engine does not know is left out.
pub(super) fn links(project: &os_model::Project) -> Vec<CtlLink> {
    project
        .controller_links
        .iter()
        .filter_map(|c| {
            let (track, slot) = c.source_slot()?;
            let target = os_model::target::decode(c.target());
            if matches!(target, Target::Other(_)) {
                return None;
            }
            // An unreadable formula hands the value on as it is.
            let formula = c.formula.as_deref().and_then(Formula::parse).filter(|f| !f.is_identity());
            Some(CtlLink {
                track: usize::from(track),
                slot: usize::from(slot),
                output: usize::from(c.output()),
                target,
                formula,
                smooth: c.smoothing().then_some(None),
                last: None,
            })
        })
        .collect()
}

/// Output `k` of the controller in `track`'s slot `slot`, as its last tick left it.
fn read(tracks: &[MixTrack], track: usize, slot: usize, k: usize) -> Option<f32> {
    let fx = tracks.get(track)?.effects.iter().find(|f| f.slot == slot)?;
    match &fx.fx {
        Fx::Own(e) => e.control(k),
        Fx::Hosted(h) => h.try_lock().ok()?.control_fx(k),
    }
}

impl CtlLink {
    /// The value for the target (`Round(v * 2^30)`) from the controller's output `v`.
    fn value(&mut self, v: f64) -> i64 {
        let mut v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        if let Some(s) = self.smooth.as_mut() {
            let st = s.get_or_insert([v, v]);
            st[0] += (v - st[0]) * SMOOTH;
            st[1] += (st[0] - st[1]) * SMOOTH;
            v = st[1];
        }
        if let Some(f) = &self.formula {
            v = f.map(v);
        }
        (v.clamp(0.0, 1.0) * 1_073_741_824.0).round() as i64
    }
}

impl Engine {
    /// Hands every link's value to its target for the tick starting now (after the automation clips' values, so a link
    /// wins over a clip on the same target). On playback's first tick volumes and pans are set in place.
    pub(super) fn run_controllers(&mut self, len: usize) {
        if self.ctl_links.is_empty() {
            return;
        }
        let len = if self.tick == self.auto_from { 0 } else { len.max(1) as u32 };
        let mut links = std::mem::take(&mut self.ctl_links);
        for link in &mut links {
            let Some(v) = read(&self.tracks, link.track, link.slot, link.output) else { continue };
            let n = link.value(f64::from(v));
            if param_auto::is_param(link.target) {
                // Per-tick plugins take it as a clip's value; the others as an event at the tick's start.
                self.params.tick_value(link.target, n);
                if link.last != Some(n) {
                    link.last = Some(n);
                    let (dest, index) = match link.target {
                        Target::ChannelParam(c, k) => (Dest::Channel(usize::from(c)), k),
                        Target::SlotParam(t, s, k) => (Dest::Slot(usize::from(t), usize::from(s)), k),
                        _ => continue,
                    };
                    self.params.events.push(ParamEvent { dest, offset: 0, index: u32::from(index), value: n as f64 / 1_073_741_824.0 });
                }
            } else {
                self.auto_value(link.target, n, len);
            }
        }
        self.ctl_links = links;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use os_model::target::{mixer, slot_param, MIXER_VOLUME};
    use os_model::{ControllerLink, Id, MixerTrack, Project, SlotPlugin};

    use crate::effects::peak_ctrl::{amount_factor, decay_per_tick, Settings};
    use crate::{Engine, HostedEffect, PlayMode, SampleLibrary};

    /// A project at 130 BPM, PPQ 96 (212.02 frames per tick at 44.1 kHz), with a Peak Controller in track 1's slot 0 and
    /// the links given; nothing plays.
    fn project(settings: Settings, links: Vec<ControllerLink>) -> Project {
        let mut mixer: Vec<MixerTrack> = (0..4).map(MixerTrack::new).collect();
        mixer[1].slots[0] = Some(SlotPlugin { id: Id::NONE, name: "Fruity Peak Controller".into(), state: settings.to_state() });
        Project { ppq: 96, tempo: Some(130.0), mixer, controller_links: links, ..Project::default() }
    }

    fn pc_link(output: u16, target: u32) -> ControllerLink {
        ControllerLink::new(0x7000 + 64, output, target)
    }

    fn vol(e: &Engine, t: usize) -> i32 {
        e.tracks[t].vol
    }

    /// The Peak output drives a mixer track's volume: Round(16000 v), the controller's base level from the first tick;
    /// an inverted formula turns it over; a link from a slot without a controller or to an unknown target does nothing.
    #[test]
    fn links_drive_mixer_volumes() {
        let s = Settings { base: 32768, amount: 0, ..Settings::default() };
        let mut inverted = pc_link(1, mixer(3, MIXER_VOLUME));
        inverted.formula = Some("1-Input*0.5".into());
        let links = vec![pc_link(1, mixer(2, MIXER_VOLUME)), inverted, ControllerLink::new(0x7000 + 64 + 5, 1, mixer(0, MIXER_VOLUME)), pc_link(1, 0x7000_1FF0)];
        let p = project(s, links);
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 44100, PlayMode::Song);
        e.set_looping(true);
        assert_eq!(e.ctl_links.len(), 3, "the unknown target is left out");
        let master = vol(&e, 0);
        let mut out = vec![0.0f32; 2 * 212];
        e.process(&mut out);
        assert_eq!(vol(&e, 2), 8000);
        assert_eq!(vol(&e, 3), 12000);
        assert_eq!(vol(&e, 0), master, "slot 5 holds no controller");
    }

    /// A stand-in hosted controller (0.25 more per tick, up to 0.75 on its output 2), and a stand-in effect that records
    /// the parameter values it gets.
    #[derive(Default)]
    struct Stub {
        v: f32,
        ticks: u32,
        seen: Vec<(u32, u32, f64)>,
    }

    impl HostedEffect for Stub {
        fn process_fx(&mut self, _rate: u32, _l: &mut [f32], _r: &mut [f32]) {
            self.ticks += 1;
            self.v = (self.ticks.min(3) as f32) * 0.25;
        }
        fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
            self.seen.push((self.ticks, offset, value));
            assert_eq!(index, 7);
        }
        fn control_fx(&self, k: usize) -> Option<f32> {
            (k == 2).then_some(self.v)
        }
    }

    #[test]
    fn hosted_controllers_drive_plugin_parameters() {
        let mut p = project(Settings::default(), vec![pc_link(2, slot_param(1, 1, 7))]);
        p.mixer[1].slots[1] = Some(SlotPlugin { id: Id::NONE, name: "Something".into(), state: vec![] });
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 44100, PlayMode::Song);
        e.set_looping(true);
        let stub = Arc::new(Mutex::new(Stub::default()));
        e.set_effect(1, 0, stub.clone());
        let target = Arc::new(Mutex::new(Stub::default()));
        e.set_effect(1, 1, target.clone());
        let mut out = vec![0.0f32; 2 * 212 * 6];
        e.process(&mut out);
        let seen = target.lock().unwrap().seen.clone();
        // The value one tick late, every value once, at offset 0.
        let values: Vec<f64> = seen.iter().map(|s| s.2).collect();
        assert_eq!(values, vec![0.0, 0.25, 0.5, 0.75]);
        assert!(seen.iter().all(|s| s.1 == 0));
    }

    /// The engine's own Fruity Peak Controller: the input's peak acts on the target one tick later and falls by the
    /// decay per tick once the input is quiet.
    #[test]
    fn the_peak_follower_pumps_a_track() {
        let p = project(Settings::default(), vec![pc_link(1, mixer(2, MIXER_VOLUME))]);
        let mut e = Engine::new(&p, &mut SampleLibrary::new(None), 44100, PlayMode::Song);
        // Drive the controller directly: a tick with a peak of 0.5, then quiet ticks.
        let mut vols = Vec::new();
        for k in 0..6 {
            if let super::Fx::Own(fx) = &mut e.tracks[1].effects[0].fx {
                let mut b = vec![if k == 0 { 0.5 } else { 0.0 }; 2 * 212];
                fx.process(&mut b);
            }
            e.run_controllers(212);
            vols.push(vol(&e, 2));
        }
        let want = |ticks: f64| (16000.0 * (0.5 - decay_per_tick(66) * ticks).max(0.0) * amount_factor(179)).round() as i32;
        assert_eq!(vols, (0..6).map(|k| want(k as f64)).collect::<Vec<_>>());
    }

    /// A smoothing link starts at its first value and moves to a new one gradually.
    #[test]
    fn smoothing_glides() {
        let mut l = super::links(&project(Settings::default(), vec![{
            let mut c = pc_link(1, mixer(2, MIXER_VOLUME));
            c.bytes[12] |= 2;
            c
        }]))
        .remove(0);
        let first = l.value(0.0);
        assert_eq!(first, 0);
        let a = l.value(1.0);
        let b = l.value(1.0);
        assert!(a > 0 && b > a && b < 1 << 29, "{a} {b}");
    }
}
