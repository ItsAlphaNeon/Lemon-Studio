// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Renders a project file the way FILE > Export as WAV does, from a test, for comparing Lemon Studio's sound with
//! other renders of the same file: the project opens as in the app (Lemon Sampler on sampler channels, the built-in
//! effects and, when asked, the installed CLAP / VST3 plugins), the hosted plugins run offline at the render rate,
//! and the song is written as a 32-bit float WAV. It also prints what plays each channel and mixer slot.
//!
//! `OS_RENDER_FLP=<project.flp> OS_RENDER_OUT=<out.wav> cargo test -p os-app --bin lemonstudio render_project -- --ignored --nocapture`
//!
//! - `OS_RENDER_RATE` (44100): the render rate; every plugin starts at it.
//! - `OS_RENDER_SCAN` (0): 1 = also host the installed CLAP / VST3 plugins the project names.
//! - `OS_RENDER_SECS`: stop after this many seconds of song.
//!
//! With the scan on it loads plugin files in-process: run it alone, in its own process.
// spec: audio/export.md

use std::time::{Duration, Instant};

use os_engine::{Engine, PlayMode, SampleLibrary};
use os_model::ChannelKind;

#[test]
#[ignore]
fn render_project() {
    let (Some(flp), Some(out)) = (std::env::var_os("OS_RENDER_FLP"), std::env::var_os("OS_RENDER_OUT")) else {
        eprintln!("set OS_RENDER_FLP and OS_RENDER_OUT");
        return;
    };
    let env = |k: &str| std::env::var(k).ok();
    let rate: u32 = env("OS_RENDER_RATE").and_then(|s| s.parse().ok()).unwrap_or(44100);
    let scan = env("OS_RENDER_SCAN").is_some_and(|s| s == "1");
    let limit: f64 = env("OS_RENDER_SECS").and_then(|s| s.parse().ok()).unwrap_or(f64::MAX);

    let mut app = crate::app::App::new(None);
    app.plugins = if scan { crate::plugins::Plugins::scan() } else { crate::plugins::Plugins::builtins() };
    // Instances start at the render rate (`Plugins::create_with` activates at the current rate).
    app.plugins.set_rate(rate);
    app.open(flp.into());
    eprintln!("OPEN {}", app.state.message.clone().unwrap_or_default());
    let t0 = Instant::now();
    while app.loading() && t0.elapsed() < Duration::from_secs(300) {
        app.plugin_tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    // Effects, wrapped plugins and 3x Osc are hosted on the ticks after loading.
    let mut last = usize::MAX;
    for _ in 0..400 {
        app.plugin_tick();
        let n = app.plugins.hosted.len();
        if n == last && t0.elapsed() > Duration::from_secs(2) {
            break;
        }
        last = n;
        std::thread::sleep(Duration::from_millis(10));
    }
    eprintln!("HOSTED {} in {:.1} s: {}", app.plugins.hosted.len(), t0.elapsed().as_secs_f64(), app.state.message.clone().unwrap_or_default());
    report(&app);

    app.plugins.set_offline(true);
    let project = crate::rack::play_project(&app.state);
    let mut library = SampleLibrary::new(SampleLibrary::default_factory_dir());
    library.resample_stretches = true;
    let mut e = Engine::new(&project, &mut library, rate, PlayMode::Song);
    for (row, i) in app.plugins.instruments() {
        e.set_instrument(row, i);
    }
    for (t, s, f) in app.plugins.effects() {
        e.set_effect(t, s, f);
    }
    let len = e.length().max(1);
    let cap = (len + u64::from(rate) * 30).min((limit * f64::from(rate)) as u64);
    let mut audio: Vec<f32> = Vec::new();
    let t1 = Instant::now();
    loop {
        let done = e.render_ticks(32, &mut audio);
        if done || audio.len() as u64 / 2 >= cap {
            break;
        }
    }
    drop(e);
    let file = std::fs::File::create(&out).expect("create the output file");
    let mut w = std::io::BufWriter::new(file);
    os_engine::wav::write_f32_stereo(&mut w, rate, &audio).expect("write");
    std::io::Write::flush(&mut w).expect("flush");
    let peak = audio.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    eprintln!("RENDERED {:.2} s (song {:.2} s) at {rate} Hz in {:.1} s, peak {peak:.3}", audio.len() as f64 / 2.0 / f64::from(rate), len as f64 / f64::from(rate), t1.elapsed().as_secs_f64());
    app.reset_hosted();
}

/// What plays each channel and mixer slot: the hosted plugin (name, format) or the engine.
fn report(app: &crate::app::App) { // This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!
    let p = &app.state.project;
    for (row, c) in p.channels.iter().enumerate() {
        if matches!(c.kind, ChannelKind::Automation | ChannelKind::Layer) {
            continue;
        }
        let who = match app.plugins.descriptor(row) {
            Some(d) => format!("{} ({})", d.name, crate::plugins::format_name(d.format)),
            None => "engine".into(),
        };
        let what = c.plugin.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("{:?}", c.kind));
        eprintln!("CH {row:>3} {:<28} {:<26} -> {who}", c.name.clone().unwrap_or_default(), what);
    }
    for m in &p.mixer {
        for (k, s) in m.slots.iter().enumerate() {
            let Some(s) = s else { continue };
            let who = match app.plugins.descriptor(crate::plugins::fx_key(m.index, k as u8)) {
                Some(d) => format!("{} ({})", d.name, crate::plugins::format_name(d.format)),
                None => "nothing (sound passes)".into(),
            };
            eprintln!("FX {:>3}.{k} {:<26} -> {who}", m.index, s.name);
        }
    }
}
