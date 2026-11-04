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
//! `space_with_an_instrument_open` plays and stops quickly with an instrument's window open. Run under `ulimit -r 0`
//! it takes rtkit's path, as for a user outside the `realtime` group; `OS_CHECK_PLAY_MS` (1000) is how long each Play
//! lasts.
//!
//! Both end with what every Play and Stop cost: the render time of the first blocks after Play and how soon it was
//! heard, the largest step between two samples after each Stop against the largest while playing (a click), the
//! real-time promotions (refused by rtkit, taken back by the safety cap) and the device's opens.

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
    let _ = write!(out, "{}", costs(rec, presses, render));
    let late = rec.blocks.iter().map(|b| b.late).max().unwrap_or_default();
    let took = rec.blocks.iter().map(|b| b.took).max().unwrap_or_default();
    let _ = writeln!(out, "\nthe device's callback: woke at most {:.2} ms late, mixed in at most {:.2} ms", late.as_secs_f64() * 1e3, took.as_secs_f64() * 1e3);
    out
}

/// What each Play and Stop cost (see the module's docs), from a recording.
fn costs(rec: &Recording, presses: &[Press], render: usize) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let (Some(c), Some(started)) = (rec.config, rec.started) else { return out };
    let period = Duration::from_nanos(u64::from(c.block) * 1_000_000_000 / u64::from(c.rate));
    let at = |b: usize| started + rec.blocks[b].due;
    // The blocks from `from` for `d` (the first after a press: due at or after it).
    let span = |from: Instant, d: Duration| (0..rec.blocks.len()).filter(move |&b| at(b) >= from && at(b) < from + d);
    let ms = |d: f64| d * 1e3;
    // The first 25 ms of blocks after each Play: their render time, and the first block with sound.
    let first = Duration::from_millis(25);
    let (mut sums, mut worst, mut heard) = (Vec::new(), Vec::new(), Vec::new());
    for p in presses.iter().filter(|p| p.play) {
        let blocks: Vec<usize> = span(p.at, first).collect();
        sums.push(blocks.iter().map(|&b| rec.moved(b, render)).sum::<u64>() as f64 * 1e-9);
        worst.push(blocks.iter().map(|&b| rec.moved(b, render)).max().unwrap_or(0) as f64 * 1e-9);
        if let Some(b) = span(p.at, Duration::from_secs(1)).find(|&b| rec.blocks[b].peak > SILENT) {
            heard.push((at(b) - p.at).as_secs_f64());
        }
    }
    let stats = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        if v.is_empty() {
            return "-".to_string();
        }
        format!("median {:.2} ms, worst {:.2} ms", ms(v[v.len() / 2]), ms(v[v.len() - 1]))
    };
    let _ = writeln!(out, "\nafter Play ({} of them):", sums.len());
    let _ = writeln!(out, "  render time of the first {:.0} ms of blocks: {}", ms(first.as_secs_f64()), stats(&mut sums));
    let _ = writeln!(out, "  the longest of those blocks: {} ({:.2} ms a block)", stats(&mut worst), ms(period.as_secs_f64()));
    let _ = writeln!(out, "  first sound: {} after the press ({} of them sounded)", stats(&mut heard.clone()), heard.len());
    // Clicks: the largest step in the 200 ms after each Stop, and while playing (from 200 ms after Play to the Stop).
    let mut play_step = 0.0f32;
    let mut stops = Vec::new();
    for (i, p) in presses.iter().enumerate() {
        let next = presses.get(i + 1).map_or(Duration::from_secs(5), |n| n.at - p.at);
        if p.play {
            let from = p.at + Duration::from_millis(200);
            play_step = span(from, next.saturating_sub(Duration::from_millis(200))).map(|b| rec.blocks[b].step).fold(play_step, f32::max);
        } else {
            stops.push(span(p.at, Duration::from_millis(200).min(next)).map(|b| rec.blocks[b].step).fold(0.0f32, f32::max));
        }
    }
    let louder = stops.iter().filter(|&&s| s > play_step).count();
    stops.sort_by(f32::total_cmp);
    if let (Some(&med), Some(&top)) = (stops.get(stops.len() / 2), stops.last()) {
        let _ = writeln!(out, "\nsample steps (clicks): while playing at most {play_step:.4}; in the 200 ms after Stop median {med:.4}, worst {top:.4}; {louder} of {} Stops above the playing maximum", stops.len());
    }
    out
}

/// The counters a check reads before and after: real-time promotions, rtkit's refusals, the safety cap's demotions,
/// the device's opens.
struct Counters {
    at: Instant,
    promoted: usize,
    refused: usize,
    demoted: usize,
    opens: u64,
}

impl Counters {
    fn now() -> Counters {
        #[cfg(target_os = "linux")]
        let ((promoted, demoted), refused) = (crate::rt::counts(), crate::rt::refused());
        #[cfg(not(target_os = "linux"))]
        let ((promoted, demoted), refused) = ((0, 0), 0);
        Counters { at: Instant::now(), promoted, refused, demoted, opens: crate::device::opens() }
    }

    /// What moved since `self`.
    fn since(&self) -> String {
        let n = Counters::now();
        let minutes = self.at.elapsed().as_secs_f64() / 60.0;
        let opens = n.opens - self.opens;
        format!(
            "real-time: {} promotions, {} refused by rtkit, {} taken back by the safety cap; the device opened {opens} times ({:.1} a minute)",
            n.promoted - self.promoted,
            n.refused - self.refused,
            n.demoted - self.demoted,
            opens as f64 / minutes.max(1e-9)
        )
    }
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
    let c0 = Counters::now();
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
    eprintln!("{}", c0.since());
    sim::set(None);
}

/// Play and Stop by Space in quick turns (a second of play, half a second stopped), 30 times, with an instrument's
/// window open, so that the instruments are processed while stopped too. Reports as `space_play_stop` does.
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
    let c0 = Counters::now();
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
    eprintln!("the transport counted: {} feeder late, {} plugin late, {} device; {} heard", d.feeder - d0.feeder, d.plugin - d0.plugin, d.device - d0.device, d.heard - d0.heard);
    eprintln!("{}", c0.since());
    app.transport.listen(false);
    sim::set(None);
}
