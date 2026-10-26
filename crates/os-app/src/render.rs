// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Renders a project file without a window, the way FILE > Export audio does: the project opens as in the app
//! (Lemon Sampler on sampler channels, the built-in effects and the plugins of the last plugin scan), the hosted
//! plugins run offline at the render rate, and the song is written as a 32-bit float WAV. It also prints what plays
//! each channel and mixer slot. No audio device is opened.
//!
//! `lemonstudio --render <project.flp> <out.wav> [--rate HZ] [--seconds S] [--from TICK] [--builtin-only]`
//!
//! - `--rate` (44100): the render rate; every plugin starts at it.
//! - `--seconds`: stop after this many seconds of song.
//! - `--from` (0): start at this tick, as playback started there does.
//! - `--builtin-only`: host only Lemon Studio's own plugins, not the installed ones the last scan found (a render
//!   that is the same on every machine).
//!
//! The ignored test `render_project` does the same from environment variables (`OS_RENDER_FLP`, `OS_RENDER_OUT`,
//! `OS_RENDER_RATE`, `OS_RENDER_SECS`, `OS_RENDER_FROM`, `OS_RENDER_SCAN`=1 to scan the installed plugins in-process).
// spec: audio/export.md

use std::path::PathBuf;
use std::time::{Duration, Instant};

use os_engine::{Engine, PlayMode};
use os_model::ChannelKind;

/// The plugins a render hosts besides Lemon Studio's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hosted {
    /// Only the built-in plugins.
    Builtin,
    /// The built-in plugins and those of the last plugin scan, as the app starts with.
    LastScan,
    /// The built-in plugins and a fresh scan of the plugin folders, in this process (tests).
    #[cfg(test)]
    ScanNow,
}

/// What to render and how.
#[derive(Debug, Clone)]
pub struct Opts {
    pub project: PathBuf,
    pub out: PathBuf,
    pub rate: u32,
    pub hosted: Hosted,
    /// Stop after this many seconds of song.
    pub seconds: Option<f64>,
    /// Start at this tick.
    pub from: u64,
}

impl Opts {
    pub fn new(project: PathBuf, out: PathBuf) -> Opts {
        Opts { project, out, rate: 44100, hosted: Hosted::LastScan, seconds: None, from: 0 }
    }
}

/// `lemonstudio --render ...`: renders and returns the process's exit code (0 done, 1 failed, 2 wrong arguments).
pub fn cli(args: &[String]) -> i32 {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("lemonstudio --render: {e}");
            eprintln!("usage: lemonstudio --render <project.flp> <out.wav> [--rate HZ] [--seconds S] [--from TICK] [--builtin-only]");
            return 2;
        }
    };
    match render(&o) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lemonstudio --render: {e}");
            1
        }
    }
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut paths = Vec::new();
    let (mut rate, mut seconds, mut from, mut hosted) = (44100u32, None, 0u64, Hosted::LastScan);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--rate" => rate = value("--rate")?.parse().map_err(|e| format!("--rate: {e}"))?,
            "--seconds" => seconds = Some(value("--seconds")?.parse().map_err(|e| format!("--seconds: {e}"))?),
            "--from" => from = value("--from")?.parse().map_err(|e| format!("--from: {e}"))?,
            "--builtin-only" => hosted = Hosted::Builtin,
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            s => paths.push(PathBuf::from(s)),
        }
    }
    let [project, out]: [PathBuf; 2] = paths.try_into().map_err(|_| "needs a project file and an output file".to_string())?;
    if !(8000..=384000).contains(&rate) {
        return Err(format!("--rate {rate}: outside 8000..384000"));
    }
    Ok(Opts { rate, seconds, from, hosted, ..Opts::new(project, out) })
}

/// Renders `o.project` into `o.out`. Progress and what plays each channel go to standard error.
pub fn render(o: &Opts) -> Result<(), String> {
    // The audio thread must not open the user's audio device (an ASIO driver opens at start).
    crate::transport::headless();
    let mut app = crate::app::App::new(None);
    match o.hosted {
        Hosted::Builtin => app.plugins = crate::plugins::Plugins::builtins(),
        Hosted::LastScan => {
            app.load_plugins();
        }
        #[cfg(test)]
        Hosted::ScanNow => app.plugins = crate::plugins::Plugins::scan(),
    }
    // Instances start at the render rate (`Plugins::create_with` activates at the current rate).
    app.plugins.set_rate(o.rate);
    app.open(o.project.clone());
    let opened = app.state.path.is_some();
    eprintln!("OPEN {}", app.state.message.clone().unwrap_or_default());
    if !opened {
        app.reset_hosted();
        return Err(format!("could not open {}", o.project.display()));
    }
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
    let mut library = crate::transport::host_library(crate::factory_folder::dir());
    let mut e = Engine::with_instruments(&project, &mut library, o.rate, PlayMode::Song, &app.plugins.instruments());
    crate::project_plugins::attach_effects(&mut e, &project, app.plugins.effects());
    if o.from > 0 {
        e.seek_tick(o.from);
    } else {
        // As the export: from the start marker, jumping at skips (spec: audio/song-structure.md "Time markers").
        e.follow_markers();
    }
    let rate = u64::from(o.rate);
    let len = e.length().max(1);
    let limit = o.seconds.map_or(u64::MAX, |s| (s.max(0.0) * rate as f64) as u64);
    let cap = (len + rate * 30).min(limit);
    let mut audio: Vec<f32> = Vec::new();
    let t1 = Instant::now();
    loop {
        let done = e.render_ticks(32, &mut audio);
        if done || audio.len() as u64 / 2 >= cap {
            break;
        }
    }
    drop(e);
    let written = std::fs::File::create(&o.out).and_then(|file| {
        let mut w = std::io::BufWriter::new(file);
        os_engine::wav::write_f32_stereo(&mut w, o.rate, &audio)?;
        std::io::Write::flush(&mut w)
    });
    let peak = audio.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    eprintln!("RENDERED {:.2} s (song {:.2} s) at {} Hz in {:.1} s, peak {peak:.3}", audio.len() as f64 / 2.0 / rate as f64, len as f64 / rate as f64, o.rate, t1.elapsed().as_secs_f64());
    app.reset_hosted();
    written.map_err(|e| format!("{}: {e}", o.out.display()))
}

/// What plays each channel and mixer slot: the hosted plugin (name, format) or the engine.
fn report(app: &crate::app::App) { // This code is a part of L e m o n S t u d i o.
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
                None if os_engine::effects::from_slot(&s.name, &s.state, 44100).is_some() => "engine".into(),
                None => "nothing (sound passes)".into(),
            };
            eprintln!("FX {:>3}.{k} {:<26} -> {who}", m.index, s.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders `OS_RENDER_FLP` into `OS_RENDER_OUT`:
    /// `OS_RENDER_FLP=<project.flp> OS_RENDER_OUT=<out.wav> cargo test -p os-app --bin lemonstudio render_project -- --ignored --nocapture`.
    /// With `OS_RENDER_SCAN=1` it scans and loads plugin files in-process: run it alone, in its own process.
    #[test]
    #[ignore]
    fn render_project() {
        let (Some(flp), Some(out)) = (std::env::var_os("OS_RENDER_FLP"), std::env::var_os("OS_RENDER_OUT")) else {
            eprintln!("set OS_RENDER_FLP and OS_RENDER_OUT");
            return;
        };
        let env = |k: &str| std::env::var(k).ok();
        let o = Opts {
            rate: env("OS_RENDER_RATE").and_then(|s| s.parse().ok()).unwrap_or(44100),
            hosted: if env("OS_RENDER_SCAN").is_some_and(|s| s == "1") { Hosted::ScanNow } else { Hosted::Builtin },
            seconds: env("OS_RENDER_SECS").and_then(|s| s.parse().ok()),
            from: env("OS_RENDER_FROM").and_then(|s| s.parse().ok()).unwrap_or(0),
            ..Opts::new(flp.into(), out.into())
        };
        render(&o).expect("render");
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_are_read() {
        let o = parse(&args(&["song.flp", "out.wav", "--rate", "48000", "--seconds", "12.5", "--from", "96", "--builtin-only"])).unwrap();
        assert_eq!((o.project, o.out), (PathBuf::from("song.flp"), PathBuf::from("out.wav")));
        assert_eq!((o.rate, o.seconds, o.from, o.hosted), (48000, Some(12.5), 96, Hosted::Builtin));
        let d = parse(&args(&["a.flp", "b.wav"])).unwrap();
        assert_eq!((d.rate, d.seconds, d.from, d.hosted), (44100, None, 0, Hosted::LastScan));
    }

    #[test]
    fn bad_options_are_refused() {
        assert!(parse(&args(&["a.flp"])).is_err());
        assert!(parse(&args(&["a.flp", "b.wav", "c.wav"])).is_err());
        assert!(parse(&args(&["a.flp", "b.wav", "--rate"])).is_err());
        assert!(parse(&args(&["a.flp", "b.wav", "--rate", "fast"])).is_err());
        assert!(parse(&args(&["a.flp", "b.wav", "--rate", "100"])).is_err());
        assert!(parse(&args(&["a.flp", "b.wav", "--loud"])).is_err());
    }

    /// A project written from the model renders to a WAV of the song's length, with no window and no device.
    #[test]
    fn renders_a_project_file() {
        let dir = std::env::temp_dir().join(format!("lemon-render-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (flp, out) = (dir.join("song.flp"), dir.join("song.wav"));
        let file = os_flp::project::create(&crate::app::test_project()).expect("create");
        std::fs::write(&flp, file.to_bytes().expect("bytes")).unwrap();
        let o = Opts { hosted: Hosted::Builtin, seconds: Some(2.0), ..Opts::new(flp, out.clone()) };
        render(&o).expect("render");
        let wav = std::fs::read(&out).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert!(wav.len() > 44, "the file holds audio");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Lemon Sampler channel on a short sample with a sustain loop (a single-cycle wave, as MicrO_5.wav) and no
    /// volume envelope plays the loop for the note's length and goes quiet once the note ends.
    // spec: audio/sampler.md "The end of a voice"
    #[test]
    fn a_looping_sample_stops_at_its_notes_end() {
        use os_model::{Arrangement, Clip, ClipSource, Note, Pattern};
        let dir = std::env::temp_dir().join(format!("lemon-render-loop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (flp, out, sample) = (dir.join("loop.flp"), dir.join("loop.wav"), dir.join("cycle.wav"));
        // 550 frames of 16-bit mono at 44.1 kHz with a `smpl` loop over the whole file.
        let frames = 550u32;
        let data: Vec<u8> = (0..frames).flat_map(|i| (if i < frames / 2 { 12000i16 } else { -12000 }).to_le_bytes()).collect();
        let mut smpl = vec![0u8; 36];
        smpl[28..32].copy_from_slice(&1u32.to_le_bytes());
        for v in [0u32, 0, 0, frames - 1, 0, 0] {
            smpl.extend(v.to_le_bytes());
        }
        let mut b = Vec::new();
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        for v in [1u16, 1] {
            b.extend(v.to_le_bytes());
        }
        b.extend(44100u32.to_le_bytes());
        b.extend((44100u32 * 2).to_le_bytes());
        for v in [2u16, 16] {
            b.extend(v.to_le_bytes());
        }
        for (id, body) in [(b"data", &data), (b"smpl", &smpl)] {
            b.extend(id);
            b.extend((body.len() as u32).to_le_bytes());
            b.extend(body);
        }
        let mut wav = b"RIFF".to_vec();
        wav.extend((b.len() as u32).to_le_bytes());
        wav.extend(b);
        std::fs::write(&sample, wav).unwrap();

        let mut p = crate::app::test_project();
        p.channels.truncate(1);
        p.channels[0].sample_path = Some(sample.to_string_lossy().into_owned());
        // One beat (96 ticks at 130 BPM: about 0.46 s).
        p.patterns = vec![Pattern { number: 1, notes: vec![Note { length: 96, key: 60, ..Note::step(0, 0) }], ..Pattern::default() }];
        p.arrangements = vec![Arrangement { number: 1, clips: vec![Clip::new(0, 384, 0, ClipSource::Pattern(1))], ..Default::default() }];
        let file = os_flp::project::create(&p).expect("create");
        std::fs::write(&flp, file.to_bytes().expect("bytes")).unwrap();
        render(&Opts { hosted: Hosted::Builtin, seconds: Some(1.5), ..Opts::new(flp, out.clone()) }).expect("render");

        let wav = std::fs::read(&out).unwrap();
        let at = wav.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
        let audio: Vec<f32> = wav[at..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        let peak = |from: f64, to: f64| audio[(from * 44100.0) as usize * 2..(to * 44100.0) as usize * 2].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        // The sample alone lasts 12 ms: still sounding at 0.3 s is the loop.
        assert!(peak(0.25, 0.4) > 0.05, "the loop plays for the note: {}", peak(0.25, 0.4));
        assert!(peak(0.7, 1.4) < 1e-4, "quiet after the note's end: {}", peak(0.7, 1.4));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
