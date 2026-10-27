// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! UI-thread load of opening a real project: opens it as the app does, then ticks and paints until it has loaded and
//! settled, and prints every tick or paint that held the UI thread for long, with how long the open itself took.
//!
//! `OS_OPEN_BENCH=<project.flp or .zip> cargo test -p os-app --bin lemonstudio open_load -- --ignored --nocapture`

use std::time::{Duration, Instant};

use os_ui::Canvas;

/// A tick or paint at least this long is listed.
const SLOW: Duration = Duration::from_millis(30);

#[test]
#[ignore]
fn open_load() {
    let Some(path) = std::env::var_os("OS_OPEN_BENCH") else {
        eprintln!("set OS_OPEN_BENCH to a project file");
        return;
    };
    let mut app = crate::app::App::new(None);
    app.plugins = crate::plugins::Plugins::default();
    app.plugins.available = crate::plugins::builtin();
    app.plugins.effects = crate::plugins::builtin_effects();
    let mut canvas = Canvas::new(1920, 1080, 1.0).unwrap();
    let t0 = Instant::now();
    app.open(path.into());
    eprintln!("open: {:.0} ms", t0.elapsed().as_secs_f64() * 1000.0);
    let (mut worst, mut slow, mut ticks) = (Duration::ZERO, 0, 0);
    let mut quiet = 0;
    while t0.elapsed() < Duration::from_secs(300) && quiet < 40 {
        let loading = app.loading();
        let t = Instant::now();
        app.tick();
        let tick = t.elapsed();
        let t = Instant::now();
        app.paint(&mut canvas, None);
        let paint = t.elapsed();
        ticks += 1;
        worst = worst.max(tick).max(paint);
        if tick >= SLOW || paint >= SLOW {
            slow += 1;
            eprintln!(
                "{:7.0} ms  tick {:5.0} ms  paint {:5.0} ms  {}  {}",
                t0.elapsed().as_secs_f64() * 1000.0,
                tick.as_secs_f64() * 1000.0,
                paint.as_secs_f64() * 1000.0,
                if loading { "loading" } else { "loaded" },
                app.project_loading().map(|(_, s)| s).unwrap_or_default()
            );
        }
        quiet = if app.loading() { 0 } else { quiet + 1 };
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!("settled in {:.1} s: {ticks} ticks, {slow} slow, worst {:.0} ms", t0.elapsed().as_secs_f64(), worst.as_secs_f64() * 1000.0);
    eprintln!("{}", app.state.message.clone().unwrap_or_default());
    for d in &app.state.dialogs {
        if let crate::dialog::Body::Msg(m) = &d.body {
            eprintln!("box: {}", m.plain());
        }
    }
    // OS_OPEN_BENCH_ZIP=<out.zip>: then Export zipped project... to it, timed the same way.
    let Some(out) = std::env::var_os("OS_OPEN_BENCH_ZIP") else { return };
    app.state.dialogs.clear();
    let t0 = Instant::now();
    crate::zipped::export(&mut app, out.into());
    let mut worst = t0.elapsed();
    while app.loading() && t0.elapsed() < Duration::from_secs(300) {
        let t = Instant::now();
        app.tick();
        app.paint(&mut canvas, None);
        worst = worst.max(t.elapsed());
        std::thread::sleep(Duration::from_millis(5));
    }
    eprintln!("zipped in {:.1} s, worst {:.0} ms: {}", t0.elapsed().as_secs_f64(), worst.as_secs_f64() * 1000.0, app.state.message.clone().unwrap_or_default());
}
