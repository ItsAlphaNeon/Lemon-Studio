// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! os-render: render an FLP project to a 32-bit float WAV with the Lemon Studio engine.
//!
//! Usage: os-render <project.flp> <out.wav> [--pattern N] [--rate HZ] [--factory DIR]
//!
//! `--factory` is the FL Studio install folder used to resolve `%FLStudioFactoryData%` sample
//! paths (default: `C:\Program Files\Image-Line\FL Studio 2026` if present).

use std::path::PathBuf;
use std::process::ExitCode;

use os_engine::{Engine, PlayMode, SampleLibrary};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("os-render: {e}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let mut mode = PlayMode::Song;
    let mut rate = 44100u32;
    let mut factory = SampleLibrary::default_factory_dir();
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--pattern" => mode = PlayMode::Pattern(value("--pattern")?.parse().map_err(|e| format!("--pattern: {e}"))?),
            "--rate" => rate = value("--rate")?.parse().map_err(|e| format!("--rate: {e}"))?,
            "--factory" => factory = Some(PathBuf::from(value("--factory")?)),
            _ => positional.push(a),
        }
    }
    let [input, output] = positional.as_slice() else {
        return Err("usage: os-render <project.flp> <out.wav> [--pattern N] [--rate HZ] [--factory DIR]".into());
    };

    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let file = os_flp::FlpFile::parse(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let project = os_flp::project::load(&file).map_err(|e| format!("{input}: {e}"))?;

    let mut library = SampleLibrary::new(factory);
    let started = std::time::Instant::now();
    let mut engine = Engine::new(&project, &mut library, rate, mode);
    // A song renders from its start marker and jumps at skips, as the app's export (spec: audio/song-structure.md
    // "Time markers").
    engine.follow_markers();
    let audio = engine.render_all(512);
    let secs = audio.len() as f64 / 2.0 / f64::from(rate);

    let mut out = std::io::BufWriter::new(std::fs::File::create(output).map_err(|e| format!("{output}: {e}"))?);
    os_engine::wav::write_f32_stereo(&mut out, rate, &audio).map_err(|e| format!("{output}: {e}"))?;

    let peak = audio.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    eprintln!(
        "rendered {secs:.2} s in {:.2} s, peak {peak:.4} ({:+.2} dBFS) -> {output}",
        started.elapsed().as_secs_f64(),
        20.0 * peak.max(1e-12).log10()
    );
    let r = &engine.report;
    if r.plugin_channels_silent + r.effects_bypassed + r.automation_clips_ignored > 0 {
        eprintln!(
            "{} effects active; not yet supported: {} plugin channels silent, {} effects bypassed, {} automation clips ignored",
            r.effects_active, r.plugin_channels_silent, r.effects_bypassed, r.automation_clips_ignored
        );
    }
    for (path, why) in &library.failures {
        eprintln!("missing sample: {path} ({why})");
    }
    Ok(())
}
