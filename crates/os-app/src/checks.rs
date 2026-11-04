// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Playback checks in simulated real time: a real project plays on the simulated device (`crate::device::sim`) while a
//! script drives the app as a user would, and a short report says which dropouts happened, of what kind, whether they
//! cut into sound, how soon after a key press, and which plugin took the most time in the worst blocks.
//!
//! `OS_CHECK_PROJECT=<project.flp> cargo test -p os-app --bin lemonstudio space_play_stop -- --ignored --nocapture`
//! (`OS_CHECK_BLOCK=256`, `OS_CHECK_RATE=48000`: the device; `OS_CHECK_ROUNDS=20`; `OS_CHECK_RT=0` leaves the audio
//! threads at normal priority; `OS_CHECK_UI_IDLE=1` stops the app's loop while the tails ring out after each Stop, to
//! tell what the UI thread causes). Run it on an idle machine, on a hidden display: the timing is the machine's.
//!
//! `space_with_an_instrument_open` plays and stops quickly with an instrument's window open, which opens the live
//! stream at every Stop. Run under `ulimit -r 0` it takes rtkit's path, as for a user outside the `realtime` group;
//! `OS_CHECK_PLAY_MS` (1000) is how long each Play lasts.

use std::time::{Duration, Instant};

use crate::app::App;
use crate::device::sim::{self, Recording};

fn var<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Below this peak a block is silence (the transport's own line, -60 dBFS).
const SILENT: f32 = 0.001;

/// What a script did, and when.
struct Press {
    at: Instant,
    phase: &'static str,
    /// Play (else Stop).
    play: bool,
}

/// The app with `path` open and its plugins hosted, on the simulated device.
fn open(path: &std::path::Path) -> App {
    let c = sim::Config { block: var("OS_CHECK_BLOCK", 256), rate: var("OS_CHECK_RATE", 48000) };
    sim::set(Some(c));
    // Linux: the audio threads at real-time priority, as the app has them (`crate::rt`).
    #[cfg(target_os = "linux")]
    if std::env::var("OS_CHECK_RT").as_deref() != Ok("0") {
        crate::rt::enable_in_tests();
    }
    let mut app = App::new(None);
    app.plugins = crate::plugins::Plugins::default();
    app.plugins.available = crate::plugins::builtin();
    app.plugins.effects = crate::plugins::builtin_effects();
    let t0 = Instant::now();
    app.open(path.into());
    while app.loading() && t0.elapsed() < Duration::from_secs(180) {
        app.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    // Mixer effects are hosted a few ticks after loading.
    run(&mut app, Duration::from_millis(500));
    eprintln!("{}: loaded in {:.1} s, {} plugins hosted", path.display(), t0.elapsed().as_secs_f64(), app.plugins.hosted.len());
    app
}

/// The app's loop for `d`: a tick about every 4 ms, as its window's frames.
fn run(app: &mut App, d: Duration) {
    let t0 = Instant::now();
    while t0.elapsed() < d {
        app.tick();
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn space(app: &mut App, presses: &mut Vec<Press>, phase: &'static str) {
    let play = !app.transport.playing();
    presses.push(Press { at: Instant::now(), phase, play });
    app.key(&winit::keyboard::Key::Named(winit::keyboard::NamedKey::Space), false, false);
}

/// Starts recording blocks with the transport's counters and every hosted plugin's processing time. The probes'
/// names: the transport's (`Transport::probes`), then one per plugin, "<name> (<place>)".
fn record(app: &App) -> usize {
    let mut probes = app.transport.probes();
    let fixed = probes.len();
    for s in app.perf_sources() {
        if let Some(m) = s.meter {
            probes.push(sim::Probe::new(&format!("{} ({})", s.name, s.place), move || m.busy_ns()));
        }
    }
    sim::record(probes);
    fixed
}

/// One dropout or worst block, as the report lists it.
struct Seen {
    block: usize,
    kinds: Vec<&'static str>,
    sound: bool,
}

/// The report of a recording: per phase, the dropouts by kind (heard or over silence) and how soon after a press;
/// then the worst blocks by render time, with the plugins that took the most time in them.
fn report(rec: &Recording, presses: &[Press], fixed: usize) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let Some(c) = rec.config else { return "no simulated device".into() };
    let Some(started) = rec.started else { return "no blocks".into() };
    let period = Duration::from_nanos(u64::from(c.block) * 1_000_000_000 / u64::from(c.rate));
    let probe = |name: &str| rec.probes.iter().position(|p| p == name).expect("a transport probe");
    let (feeder, plugin, heard, render) = (probe("feeder"), probe("plugin"), probe("heard"), probe("render"));
    let n = rec.blocks.len();
    let at = |b: usize| started + rec.blocks[b].due;
    // Heard as the transport tells (`Transport::heard_dropouts`); a device underrun, by the blocks around it.
    let sound = |b: usize| rec.moved(b, heard) > 0 || (rec.blocks[b].xrun && (rec.blocks[b].peak > SILENT || (b > 0 && rec.blocks[b - 1].peak > SILENT)));
    let _ = writeln!(out, "device: {} frames at {} Hz ({:.2} ms a block), {} blocks ({:.1} s)", c.block, c.rate, period.as_secs_f64() * 1e3, n, n as f64 * period.as_secs_f64());
    // Every block with a dropout, by kind: an empty block, a plugin's missed tick, the device's own underrun.
    let mut seen = Vec::new();
    for b in 0..n {
        let mut kinds = Vec::new();
        if rec.moved(b, feeder) > 0 {
            kinds.push("feeder late");
        }
        if rec.moved(b, plugin) > 0 {
            kinds.push("plugin late");
        }
        if rec.blocks[b].xrun {
            kinds.push("device underrun");
        }
        if !kinds.is_empty() {
            seen.push(Seen { block: b, kinds, sound: sound(b) });
        }
    }
    let mut phases: Vec<&str> = presses.iter().map(|p| p.phase).collect();
    phases.dedup();
    // The press a block came after.
    let after = |b: usize| presses.iter().rev().find(|p| p.at <= at(b));
    let _ = writeln!(out, "\n{:<10} {:>24} {:>24} {:>24} {:>9} {:>9}", "phase", "feeder late (heard/all)", "plugin late (heard/all)", "device (heard/all)", "<100 ms", "<100 ms");
    let _ = writeln!(out, "{:<10} {:>24} {:>24} {:>24} {:>9} {:>9}", "", "", "", "", "of Play", "of Stop");
    for ph in &phases {
        let mine: Vec<&Seen> = seen.iter().filter(|s| after(s.block).is_some_and(|p| p.phase == *ph)).collect();
        let count = |k: &str| {
            let all: Vec<&&Seen> = mine.iter().filter(|s| s.kinds.contains(&k)).collect();
            format!("{}/{}", all.iter().filter(|s| s.sound).count(), all.len())
        };
        let soon = |play: bool| {
            mine.iter().filter(|s| after(s.block).is_some_and(|p| p.play == play && at(s.block) - p.at < Duration::from_millis(100))).count()
        };
        let _ = writeln!(out, "{:<10} {:>24} {:>24} {:>24} {:>9} {:>9}", ph, count("feeder late"), count("plugin late"), count("device underrun"), soon(true), soon(false));
    }
    // The plugins that took the most time in block `b`'s interval (what the feeder rendered for it).
    let top = |b: usize, k: usize| {
        let mut v: Vec<(u64, &str)> = (fixed..rec.probes.len()).map(|p| (rec.moved(b, p), rec.probes[p].as_str())).filter(|(ns, _)| *ns > 0).collect();
        v.sort_by(|a, b| b.0.cmp(&a.0));
        v.truncate(k);
        v.iter().map(|(ns, name)| format!("{name} {:.2} ms", *ns as f64 * 1e-6)).collect::<Vec<_>>().join(", ")
    };
    let ms = |ns: u64| ns as f64 * 1e-6;
    let line = |out: &mut String, b: usize, what: &str| {
        let p = after(b).map_or(String::new(), |p| format!("{:>6.0} ms after {}", (at(b) - p.at).as_secs_f64() * 1e3, if p.play { "Play" } else { "Stop" }));
        let blk = &rec.blocks[b];
        let _ = writeln!(
            out,
            "  {:>7.3} s  {what:<28} render {:>6.2} ms  late {:>5.2} ms  peak {:.3}  {p}  {}",
            blk.due.as_secs_f64(),
            ms(rec.moved(b, render)),
            blk.late.as_secs_f64() * 1e3,
            blk.peak,
            top(b, 3)
        );
    };
    if !seen.is_empty() {
        let _ = writeln!(out, "\ndropouts (the heard ones first, at most 30):");
        let mut order: Vec<&Seen> = seen.iter().filter(|s| s.sound).chain(seen.iter().filter(|s| !s.sound)).collect();
        order.truncate(30);
        for s in order {
            let what = format!("{}{}", s.kinds.join("+"), if s.sound { "" } else { ", silent" });
            line(&mut out, s.block, &what);
        }
    }
    let mut worst: Vec<usize> = (0..n).collect();
    worst.sort_by_key(|&b| std::cmp::Reverse(rec.moved(b, render)));
    let _ = writeln!(out, "\nworst blocks by render time ({:.2} ms a block):", period.as_secs_f64() * 1e3);
    for &b in worst.iter().take(10) {
        let share = format!("{:.0} % of a block", ms(rec.moved(b, render)) / (period.as_secs_f64() * 1e3) * 100.0);
        line(&mut out, b, &share);
    }
    let late = rec.blocks.iter().map(|b| b.late).max().unwrap_or_default();
    let took = rec.blocks.iter().map(|b| b.took).max().unwrap_or_default();
    let _ = writeln!(out, "\nthe device's callback: woke at most {:.2} ms late, mixed in at most {:.2} ms", late.as_secs_f64() * 1e3, took.as_secs_f64() * 1e3);
    out
}

/// Play and Stop by Space, as the Discord report did: 20 times with the tails left to ring out after each Stop, then
/// 20 times with Play pressed while the tails still ring. Reports the dropouts of each and the worst blocks.
#[test]
#[ignore]
fn space_play_stop() {
    let Some(path) = std::env::var_os("OS_CHECK_PROJECT") else {
        eprintln!("set OS_CHECK_PROJECT to a project file");
        return;
    };
    let rounds: usize = var("OS_CHECK_ROUNDS", 20);
    let mut app = open(std::path::Path::new(&path));
    let mut presses = Vec::new();
    let fixed = record(&app);
    let d0 = app.transport.drops();
    for _ in 0..rounds {
        space(&mut app, &mut presses, "space");
        run(&mut app, Duration::from_secs(2));
        space(&mut app, &mut presses, "space");
        // The tails ring out (3 s under -96 dB) and the effects are reset.
        if std::env::var_os("OS_CHECK_UI_IDLE").is_some() {
            std::thread::sleep(Duration::from_millis(4500));
        } else {
            run(&mut app, Duration::from_millis(4500));
        }
    }
    for _ in 0..rounds {
        space(&mut app, &mut presses, "tails");
        run(&mut app, Duration::from_secs(2));
        space(&mut app, &mut presses, "tails");
        // Play again while the tails ring.
        run(&mut app, Duration::from_millis(300));
    }
    run(&mut app, Duration::from_millis(4500));
    let rec = sim::take();
    let d = app.transport.drops();
    eprintln!("\n{}", report(&rec, &presses, fixed));
    eprintln!(
        "the transport counted: {} feeder late, {} plugin late, {} device; {} heard (since the app started: {} dropouts)",
        d.feeder - d0.feeder,
        d.plugin - d0.plugin,
        d.device - d0.device,
        d.heard - d0.heard,
        app.transport.underruns()
    );
    sim::set(None);
}

/// Play and Stop by Space in quick turns (a second of play, half a second stopped), 30 times, with an instrument's
/// window open: each Stop opens the live stream and each Play the playback, every one with new audio threads asking for
/// real-time priority. Reports the dropouts, and how many promotions rtkit refused and how many audio threads the
/// safety cap took back.
#[test]
#[ignore]
fn space_with_an_instrument_open() {
    let Some(path) = std::env::var_os("OS_CHECK_PROJECT") else {
        eprintln!("set OS_CHECK_PROJECT to a project file");
        return;
    };
    let rounds: usize = var("OS_CHECK_ROUNDS", 30);
    let mut app = open(std::path::Path::new(&path));
    app.transport.listen(true);
    run(&mut app, Duration::from_millis(500));
    let mut presses = Vec::new();
    let fixed = record(&app);
    let d0 = app.transport.drops();
    #[cfg(target_os = "linux")]
    let (r0, (p0, x0)) = (crate::rt::refused(), crate::rt::counts());
    for _ in 0..rounds {
        space(&mut app, &mut presses, "quick");
        run(&mut app, Duration::from_millis(var("OS_CHECK_PLAY_MS", 1000)));
        space(&mut app, &mut presses, "quick");
        run(&mut app, Duration::from_millis(500));
    }
    run(&mut app, Duration::from_millis(1000));
    let rec = sim::take();
    let d = app.transport.drops();
    eprintln!("\n{}", report(&rec, &presses, fixed));
    eprintln!(
        "the transport counted: {} feeder late, {} plugin late, {} device; {} heard",
        d.feeder - d0.feeder,
        d.plugin - d0.plugin,
        d.device - d0.device,
        d.heard - d0.heard,
    );
    #[cfg(target_os = "linux")]
    {
        let (p, x) = crate::rt::counts();
        eprintln!("real-time: {} promotions, {} refused by rtkit, {} taken back by the safety cap", p - p0, crate::rt::refused() - r0, x - x0);
    }
    app.transport.listen(false);
    sim::set(None);
}
