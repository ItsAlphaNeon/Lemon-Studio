// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Probe: how many note_on events does a hosted instrument receive for ONE live note (typing keyboard)?
use std::sync::{Arc, Mutex};

use os_engine::{Engine, Instrument, PlayMode, SampleLibrary, SharedInstrument};

#[derive(Default)]
struct Count {
    on: Vec<(u64, u8, f32)>,
    off: Vec<(u64, u8)>,
    at: u64,
}

impl Instrument for Count {
    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        self.on.push((self.at + u64::from(offset), key, velocity));
    }
    fn note_off(&mut self, offset: u32, key: u8) {
        self.off.push((self.at + u64::from(offset), key));
    }
    fn process(&mut self, _rate: u32, _l: &mut [f32], _r: &mut [f32]) {
        self.at += 512;
    }
}

fn channel() -> os_model::Channel {
    os_model::Channel {
        index: 0,
        kind: os_model::ChannelKind::Plugin,
        name: None,
        plugin: Some("Lemon Sampler".into()),
        plugin_state: None,
        color: None,
        enabled: true,
        volume: 0.78125,
        pan: 0.0,
        pitch_cents: 0,
        mixer_track: 0,
        sample_path: None,
        layer_children: vec![],
        envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
    }
}

fn project() -> os_model::Project {
    os_model::Project {
        ppq: 96,
        tempo: Some(130.0),
        channels: vec![channel()],
        patterns: vec![os_model::Pattern { number: 1, ..Default::default() }],
        ..Default::default()
    }
}

fn run(with_off: bool) -> (usize, usize) {
    let p = project();
    let mut lib = SampleLibrary::default();
    let mut e = Engine::new(&p, &mut lib, 48000, PlayMode::Pattern(1));
    e.clear_schedule();
    let held = Arc::new(Mutex::new(Count::default()));
    let probe: SharedInstrument = held.clone();
    e.set_instrument(0, probe.clone());
    e.note_on(0, 60, 100);
    let mut out = vec![0.0f32; 512];
    for b in 0..40 {
        if with_off && b == 10 {
            e.note_off(0, 60);
        }
        e.process(&mut out);
    }
    let c = held.lock().unwrap();
    (c.on.len(), c.off.len())
}

#[test]
fn single_live_note_is_sent_once() {
    let (on, _) = run(false);
    assert_eq!(on, 1, "one key press must send one note_on, got {on}");
}

#[test]
fn single_released_live_note_is_sent_once() {
    let (on, off) = run(true);
    assert_eq!(on, 1, "one key press must send one note_on, got {on}");
    assert_eq!(off, 1, "one key release must send one note_off, got {off}");
}
