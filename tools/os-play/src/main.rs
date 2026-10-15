// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! os-play: play an FLP project through the default audio output (WASAPI on Windows).
//!
//! Usage: os-play <project.flp> [--pattern N] [--factory DIR]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use os_engine::{Engine, PlayMode, SampleLibrary};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("os-play: {e}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut input = None;
    let mut mode = PlayMode::Song;
    let mut factory = SampleLibrary::default_factory_dir();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pattern" => {
                let v = args.next().ok_or("--pattern needs a value")?;
                mode = PlayMode::Pattern(v.parse().map_err(|e| format!("--pattern: {e}"))?);
            }
            "--factory" => factory = Some(PathBuf::from(args.next().ok_or("--factory needs a value")?)),
            _ => input = Some(a),
        }
    }
    let input = input.ok_or("usage: os-play <project.flp> [--pattern N] [--factory DIR]")?;

    let bytes = std::fs::read(&input).map_err(|e| format!("{input}: {e}"))?;
    let file = os_flp::FlpFile::parse(&bytes).map_err(|e| format!("{input}: {e}"))?;
    let project = os_flp::project::load(&file).map_err(|e| format!("{input}: {e}"))?;

    let host = cpal::default_host();
    let device = host.default_output_device().ok_or("no audio output device")?;
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let config: StreamConfig = supported.into();
    let rate = config.sample_rate;
    eprintln!(
        "output: {} ({} Hz, {} ch, {format:?})",
        device.description().map(|d| d.to_string()).unwrap_or_else(|_| "default device".into()),
        rate,
        config.channels
    );

    let mut library = SampleLibrary::new(factory);
    let engine = Engine::new(&project, &mut library, rate, mode);
    for (path, why) in &library.failures {
        eprintln!("missing sample: {path} ({why})");
    }
    let r = &engine.report;
    if r.plugin_channels_silent + r.effects_bypassed + r.automation_clips_ignored > 0 {
        eprintln!(
            "{} effects active; not yet supported: {} plugin channels silent, {} effects bypassed, {} automation clips ignored",
            r.effects_active, r.plugin_channels_silent, r.effects_bypassed, r.automation_clips_ignored
        );
    }
    let length = engine.length();

    let done = Arc::new(AtomicBool::new(false));
    let played = Arc::new(AtomicU64::new(0));
    let engine = Arc::new(Mutex::new(engine));
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config, engine, done.clone(), played.clone()),
        SampleFormat::I16 => build::<i16>(&device, &config, engine, done.clone(), played.clone()),
        SampleFormat::I32 => build::<i32>(&device, &config, engine, done.clone(), played.clone()),
        SampleFormat::U16 => build::<u16>(&device, &config, engine, done.clone(), played.clone()),
        other => return Err(format!("unsupported output sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;

    let secs = |f: u64| f as f64 / f64::from(rate);
    while !done.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(250));
        eprint!("\r{:7.2} / {:.2} s ", secs(played.load(Ordering::Relaxed)), secs(length));
    }
    std::thread::sleep(Duration::from_millis(300)); // let the device drain
    eprintln!("\ndone");
    Ok(())
}

fn build<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    engine: Arc<Mutex<Engine>>,
    done: Arc<AtomicBool>,
    played: Arc<AtomicU64>,
) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let mut stereo: Vec<f32> = Vec::with_capacity(8192);
    device
        .build_output_stream::<T, _, _>(
            config.clone(),
            move |out: &mut [T], _| {
                let frames = out.len() / channels;
                stereo.resize(frames * 2, 0.0);
                // The engine is only shared with this callback; try_lock never waits in practice.
                match engine.try_lock() {
                    Ok(mut e) if !e.finished() => {
                        e.process(&mut stereo);
                        played.store(e.position(), Ordering::Relaxed);
                    }
                    Ok(_) => {
                        stereo.fill(0.0);
                        done.store(true, Ordering::Relaxed);
                    }
                    Err(_) => stereo.fill(0.0),
                }
                for (frame, s) in out.chunks_mut(channels).zip(stereo.chunks_exact(2)) {
                    for (c, slot) in frame.iter_mut().enumerate() {
                        let v = match c {
                            0 => s[0],
                            1 => s[1],
                            _ => 0.0,
                        };
                        *slot = T::from_sample(v.clamp(-1.0, 1.0));
                    }
                }
            },
            |e| eprintln!("\naudio stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}
