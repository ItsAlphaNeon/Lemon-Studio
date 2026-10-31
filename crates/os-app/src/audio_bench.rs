// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio-thread load on a real project: opens it as the app does (Lemon Sampler instances, hosted effects), renders the
//! song tick by tick and prints how long each second of audio took to render, as a share of one core. Ticks render the
//! same way during playback, so a second near 100 % here is a second that drops out live.
//!
//! `OS_AUDIO_BENCH=<project.flp> cargo test -p os-app --bin lemonstudio audio_load -- --ignored --nocapture`
//! (`OS_AUDIO_BENCH_SECS=n` stops after n seconds of song; `OS_MIX_THREADS=n` renders on n mixing workers beside the
//! rendering thread, spec: audio/multicore.md, where the time is still the rendering thread's wall clock).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use os_engine::{Engine, HostedEffect, Instrument, PlayMode, SampleLibrary, SharedEffect, SharedInstrument, TransportInfo};

/// A hosted instrument that adds the time its `process` calls take to a counter.
struct TimedInst(SharedInstrument, Arc<AtomicU64>);

impl Instrument for TimedInst {
    fn note_on(&mut self, offset: u32, key: u8, velocity: f32) {
        self.0.lock().unwrap().note_on(offset, key, velocity)
    }
    fn note_off(&mut self, offset: u32, key: u8) {
        self.0.lock().unwrap().note_off(offset, key)
    }
    fn note_on_from(&mut self, offset: u32, key: u8, velocity: f32, start: f32) {
        self.0.lock().unwrap().note_on_from(offset, key, velocity, start)
    }
    fn note_start(&mut self, offset: u32, key: u8, velocity: f32, start: f32, levels: os_engine::NoteLevels) {
        self.0.lock().unwrap().note_start(offset, key, velocity, start, levels)
    }
    fn channel_levels(&mut self, levels: os_engine::ChannelLevels) -> bool {
        self.0.lock().unwrap().channel_levels(levels)
    }
    fn tick_params(&self) -> bool {
        self.0.lock().unwrap().tick_params()
    }
    fn channel_voicing(&mut self, voicing: &os_engine::ChannelVoicing) {
        self.0.lock().unwrap().channel_voicing(voicing)
    }
    fn choke(&mut self, offset: u32, key: u8) {
        self.0.lock().unwrap().choke(offset, key)
    }
    fn voice_start(&mut self, offset: u32, key: u8, velocity: f32, start: f32, levels: os_engine::NoteLevels, voice: u32) {
        self.0.lock().unwrap().voice_start(offset, key, velocity, start, levels, voice)
    }
    fn voice_gains(&self) -> bool {
        self.0.lock().unwrap().voice_gains()
    }
    fn voice_gain(&mut self, offset: u32, voice: u32, gain: f32) {
        self.0.lock().unwrap().voice_gain(offset, voice, gain)
    }
    fn voice_choke(&mut self, offset: u32, key: u8, voice: u32) {
        self.0.lock().unwrap().voice_choke(offset, key, voice)
    }
    fn tune(&mut self, offset: u32, key: u8, semitones: f32) {
        self.0.lock().unwrap().tune(offset, key, semitones)
    }
    fn process(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        let t = Instant::now();
        self.0.lock().unwrap().process(rate, left, right);
        self.1.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    fn active(&self) -> bool {
        self.0.lock().unwrap().active()
    }
    fn param(&mut self, offset: u32, index: u32, value: f64) {
        self.0.lock().unwrap().param(offset, index, value)
    }
    fn transport(&mut self, info: &TransportInfo) {
        self.0.lock().unwrap().transport(info)
    }
    fn reported_delay(&self) -> u32 {
        self.0.lock().unwrap().reported_delay()
    }
}

/// [`TimedInst`] for effects.
struct TimedFx(SharedEffect, Arc<AtomicU64>);

impl HostedEffect for TimedFx {
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
        let t = Instant::now();
        self.0.lock().unwrap().process_fx(rate, left, right);
        self.1.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
    fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
        self.0.lock().unwrap().param_fx(offset, index, value)
    }
    fn transport_fx(&mut self, info: &TransportInfo) {
        self.0.lock().unwrap().transport_fx(info)
    }
    fn reported_delay_fx(&self) -> u32 {
        self.0.lock().unwrap().reported_delay_fx()
    }
}

#[test]
#[ignore]
fn audio_load() {
    let Some(path) = std::env::var_os("OS_AUDIO_BENCH") else {
        eprintln!("set OS_AUDIO_BENCH to a project file");
        return;
    };
    let limit: f64 = std::env::var("OS_AUDIO_BENCH_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(f64::MAX);
    let mut app = crate::app::App::new(None);
    app.plugins = crate::plugins::Plugins::default();
    app.plugins.available = crate::plugins::builtin();
    app.plugins.effects = crate::plugins::builtin_effects();
    app.open(path.into());
    eprintln!("{}", app.state.message.clone().unwrap_or_default());
    let t0 = Instant::now();
    while app.loading() && t0.elapsed() < Duration::from_secs(120) {
        app.plugin_tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    // Mixer effects are hosted a few ticks after loading.
    for _ in 0..20 {
        app.plugin_tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!("loaded in {:.1} s", t0.elapsed().as_secs_f64());
    let rate = crate::transport::Transport::device_rate().unwrap_or(48000);
    let project = crate::rack::play_project(&app.state);
    let mut library = SampleLibrary::new(crate::factory_folder::dir());
    library.resample_stretches = true;
    let mut e = Engine::new(&project, &mut library, rate, PlayMode::Song);
    let inst = app.plugins.instruments();
    let fx = app.plugins.effects();
    eprintln!("{} hosted instruments, {} hosted effects, {} Hz", inst.len(), fx.len(), rate);
    // OS_AUDIO_BENCH_SKIP=inst|fx leaves those out, to see what each costs.
    let skip = std::env::var("OS_AUDIO_BENCH_SKIP").unwrap_or_default();
    let mut timers: Vec<(String, Arc<AtomicU64>)> = Vec::new();
    for (row, i) in inst.into_iter().filter(|_| skip != "inst") {
        let ns = Arc::new(AtomicU64::new(0));
        timers.push((format!("inst {row} {}", project.channels.get(row).and_then(|c| c.name.as_deref()).unwrap_or("")), ns.clone()));
        e.set_instrument(row, Arc::new(Mutex::new(TimedInst(i, ns))));
    }
    for (t, s, f) in fx.into_iter().filter(|_| skip != "fx") {
        let ns = Arc::new(AtomicU64::new(0));
        timers.push((format!("fx {t}.{s}"), ns.clone()));
        e.set_effect(t, s, Arc::new(Mutex::new(TimedFx(f, ns))));
    }
    let mut out: Vec<f32> = Vec::new();
    let (mut spent, mut total, mut worst) = (Duration::ZERO, Duration::ZERO, 0.0f64);
    let mut second = 1u64;
    let mut peak = 0.0f32;
    loop {
        let t = Instant::now();
        let done = e.render_ticks(1, &mut out);
        let d = t.elapsed();
        spent += d;
        total += d;
        let frames = out.len() as u64 / 2;
        while frames >= second * u64::from(rate) || (done && spent > Duration::ZERO) {
            let load = spent.as_secs_f64() * 100.0;
            worst = worst.max(load);
            let from = ((second - 1) * u64::from(rate) * 2) as usize;
            peak = out[from.min(out.len())..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            eprintln!("{:>4} s  {:>5.1} %  {}  peak {:.2}", second, load, "#".repeat((load / 2.0) as usize), peak);
            spent = Duration::ZERO;
            second += 1;
            if done {
                break;
            }
        }
        if done || frames as f64 / f64::from(rate) >= limit {
            break;
        }
    }
    let secs = out.len() as f64 / 2.0 / f64::from(rate);
    let (mut ti, mut tf) = (0u64, 0u64);
    for (name, ns) in &timers {
        let v = ns.load(Ordering::Relaxed);
        if name.starts_with("inst") { ti += v } else { tf += v }
    }
    eprintln!("instruments {:.2} s, effects {:.2} s, engine {:.2} s", ti as f64 * 1e-9, tf as f64 * 1e-9, total.as_secs_f64() - (ti + tf) as f64 * 1e-9);
    timers.sort_by_key(|(_, ns)| std::cmp::Reverse(ns.load(Ordering::Relaxed)));
    for (name, ns) in timers.iter().take(25) {
        eprintln!("  {:>7.3} s  {name}", ns.load(Ordering::Relaxed) as f64 * 1e-9);
    }
    eprintln!("{secs:.1} s of song in {:.2} s: average {:.1} %, worst second {:.1} % (last peak {peak:.2})", total.as_secs_f64(), total.as_secs_f64() / secs * 100.0, worst);
}
