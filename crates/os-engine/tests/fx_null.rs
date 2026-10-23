// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Null test of Lemon Studio's mixer-slot effects against FL Studio's own exports. Skipped unless `OS_FX_NULL` names a
//! directory of cases; nothing in it is needed to build or to run the other tests.
//!
//! A case is a `*.case` text file of `key = value` lines (`#` starts a comment):
//!
//! | Key | Meaning |
//! |---|---|
//! | `plugin` | The slot's plugin name as a project stores it (`Fruity Chorus`, `Fruity Stereo Shaper`, ...) |
//! | `state` | The slot's saved state, in hex |
//! | `dry` | A WAV of what the slot is fed: FL's export of the same project with the slot switched off |
//! | `fl` | FL's export with the slot on |
//! | `bpm`, `ppq` | The project's tempo and timebase: the effect runs on FL's ticks, as in a render (default 140, 96) |
//! | `preroll` | Frames of silence FL ran through the effect before the song, in blocks of 512 (default 0) |
//! | `phases` | Fruity Chorus only: the three LFO start phases of that export, `a,b,c` (FL draws them from the clock; a case without them is run with Lemon Studio's fixed seed and only reported) |
//! | `limit_db` | The largest difference allowed, in dB of full scale (default -120) |
//!
//! Paths are relative to the case file. The test prints every case's peak difference and fails when one is over its
//! limit. Lemon Studio's slot is the recreation run here (`effects::from_slot`); the engine's own handling of the slot
//! (switches, mix level, the difference output) is not part of this comparison.
//!
// spec: plugins/fruity-chorus.md "FL 2026 null test (not run yet)", plugins/fruity-stereo-shaper.md "FL 2026 null test (not run yet)"

use std::path::{Path, PathBuf};

use os_engine::clock::Clock;
use os_engine::effects::{self, chorus, Effect};

struct Case {
    plugin: String,
    state: Vec<u8>,
    dry: PathBuf,
    fl: PathBuf,
    bpm: f64,
    ppq: u16,
    preroll: usize,
    phases: Option<[u32; 3]>,
    limit_db: f64,
}

fn hex(s: &str) -> Result<Vec<u8>, String> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.len() % 2 != 0 {
        return Err("odd number of hex digits".into());
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string())).collect()
}

fn parse(path: &Path) -> Result<Case, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut case = Case {
        plugin: String::new(),
        state: Vec::new(),
        dry: PathBuf::new(),
        fl: PathBuf::new(),
        bpm: 140.0,
        ppq: 96,
        preroll: 0,
        phases: None,
        limit_db: -120.0,
    };
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=').ok_or_else(|| format!("not `key = value`: {line}"))?;
        let value = value.trim();
        let num = |v: &str| v.parse::<f64>().map_err(|e| format!("{key}: {e}"));
        match key.trim() {
            "plugin" => case.plugin = value.to_owned(),
            "state" => case.state = hex(value)?,
            "dry" => case.dry = dir.join(value),
            "fl" => case.fl = dir.join(value),
            "bpm" => case.bpm = num(value)?,
            "ppq" => case.ppq = num(value)? as u16,
            "preroll" => case.preroll = num(value)? as usize,
            "limit_db" => case.limit_db = num(value)?,
            "phases" => {
                let p: Vec<u32> = value.split(',').map(|v| v.trim().parse::<u32>().map_err(|e| format!("phases: {e}"))).collect::<Result<_, _>>()?;
                case.phases = Some(p.try_into().map_err(|_| "phases: three values".to_owned())?);
            }
            other => return Err(format!("unknown key {other}")),
        }
    }
    if case.plugin.is_empty() || case.dry.as_os_str().is_empty() || case.fl.as_os_str().is_empty() {
        return Err("plugin, dry and fl are needed".into());
    }
    Ok(case)
}

/// The effect as a slot plays it; a chorus started on the export's phases when the case names them.
fn effect(case: &Case, rate: u32) -> Result<Box<dyn Effect>, String> {
    if case.plugin == "Fruity Chorus" {
        let mut c = chorus::Chorus::from_state(&case.state, rate).ok_or("state too short")?;
        if let Some(p) = case.phases {
            c.set_phases(p);
        }
        return Ok(Box::new(c));
    }
    effects::from_slot(&case.plugin, &case.state, rate).ok_or_else(|| format!("no recreation of {} (or the state is too short)", case.plugin))
}

/// Runs one case; the peak difference in dB of full scale.
fn run(case: &Case) -> Result<f64, String> {
    let dry = os_engine::sample::load(&case.dry).map_err(|e| e.to_string())?;
    let fl = os_engine::sample::load(&case.fl).map_err(|e| e.to_string())?;
    if dry.rate != fl.rate {
        return Err(format!("rates differ: {} and {}", dry.rate, fl.rate));
    }
    let mut fx = effect(case, dry.rate)?;
    let mut silence = vec![0.0f32; 1024];
    for _ in 0..case.preroll / 512 {
        silence.fill(0.0);
        fx.process(&mut silence);
    }
    let n = dry.left.len().min(fl.left.len());
    let mut clock = Clock::new(dry.rate, case.ppq, case.bpm);
    let (mut at, mut peak) = (0, 0.0f64);
    let mut buf = Vec::new();
    while at < n {
        let len = clock.advance().1.min(n - at);
        buf.clear();
        buf.extend((at..at + len).flat_map(|i| [dry.left[i], dry.right[i]]));
        fx.process(&mut buf);
        for (k, f) in buf.chunks_exact(2).enumerate() {
            let i = at + k;
            peak = peak.max(f64::from(f[0] - fl.left[i]).abs()).max(f64::from(f[1] - fl.right[i]).abs());
        }
        at += len;
    }
    Ok(if peak == 0.0 { f64::NEG_INFINITY } else { 20.0 * peak.log10() })
}

#[test]
fn slot_effects_null_against_fl_exports() {
    let Some(dir) = std::env::var_os("OS_FX_NULL") else {
        eprintln!("OS_FX_NULL not set: the null test against FL's exports is skipped");
        return;
    };
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("OS_FX_NULL is a directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "case"))
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "no *.case files in {}", PathBuf::from(&dir).display());
    let mut failed = Vec::new();
    for path in &cases {
        let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let case = match parse(path) {
            Ok(c) => c,
            Err(e) => {
                failed.push(format!("{name}: {e}"));
                continue;
            }
        };
        match run(&case) {
            // A chorus without the export's phases is only reported: its LFOs start elsewhere.
            Ok(db) if case.plugin == "Fruity Chorus" && case.phases.is_none() => println!("{name}: {db:.1} dB (phases unknown, not judged)"),
            Ok(db) if db <= case.limit_db => println!("{name}: {db:.1} dB"),
            Ok(db) => failed.push(format!("{name}: {db:.1} dB, over {:.1}", case.limit_db)),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    assert!(failed.is_empty(), "{} of {} cases failed:\n{}", failed.len(), cases.len(), failed.join("\n"));
}
