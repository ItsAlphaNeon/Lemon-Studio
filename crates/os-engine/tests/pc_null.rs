// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Null test of Lemon Studio's Fruity Peak Controller against FL Studio's own exports, tick by tick. Skipped unless
//! `OS_PC_NULL` names a directory of cases; nothing in it is needed to build or to run the other tests.
//!
//! Each export is of a project whose master holds a Fruity Peak Controller in slot 0, linked to the master's volume.
//! The probe is a stereo file: the sound the controller follows on the left, a small constant (the carrier) on the
//! right. The master's fader is the only thing that changes the carrier, so the export's right side divided by the
//! carrier and the chain's gain is the fader's gain, sample by sample; at every tick's start the fader starts its ramp
//! to the new value, and the ramp's first step gives that value (a step the fader's speed limit cut short says nothing
//! and is skipped). The test runs the recreation on the same probe, hands its output on as a link does (one tick late,
//! as the master's volume `Round(16000 v)`) and compares the two values, as the link's 0..1 value.
//!
//! A case is a `*.case` text file of `key = value` lines (`#` starts a comment):
//!
//! | Key | Meaning |
//! |---|---|
//! | `state` | The slot's saved state, in hex |
//! | `output` | The link's output: 0 Peak+LFO, 1 Peak, 2 LFO |
//! | `smooth` | 1 when the link has Smoothing on (default 0) |
//! | `formula` | The link's formula (default none) |
//! | `dry` | The probe (a WAV) |
//! | `gain` | What the channel does to the probe before the master (the slot hears `dry * gain`) |
//! | `fl` | FL's export |
//! | `carrier` | The right side's constant in `dry` (default 1e-4) |
//! | `bpm`, `ppq` | The project's tempo and timebase (default 130, 96) |
//! | `limit` | The largest difference allowed, in link value (default 2e-4) |
//! | `judge` | `no`: only report (the random shape) |
//!
//! Paths are relative to the case file. The cases (FL's exports) are not part of the repository.
//!
// spec: plugins/lemon-peak-controller.md "Null test", flp/events/automation.md "Controller links"

use std::path::{Path, PathBuf};

use os_engine::clock::Clock;
use os_engine::effects;
use os_engine::formula::Formula;

struct Case {
    state: Vec<u8>,
    output: usize,
    smooth: bool,
    formula: Option<Formula>,
    dry: PathBuf,
    gain: f64,
    fl: PathBuf,
    carrier: f64,
    bpm: f64,
    ppq: u16,
    limit: f64,
    judge: bool,
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
        state: Vec::new(),
        output: 1,
        smooth: false,
        formula: None,
        dry: PathBuf::new(),
        gain: 1.0,
        fl: PathBuf::new(),
        carrier: 1e-4,
        bpm: 130.0,
        ppq: 96,
        limit: 2e-4,
        judge: true,
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
            "state" => case.state = hex(value)?,
            "output" => case.output = num(value)? as usize,
            "smooth" => case.smooth = num(value)? != 0.0,
            "formula" => case.formula = Some(Formula::parse(value).ok_or_else(|| format!("formula: cannot read {value}"))?),
            "dry" => case.dry = dir.join(value),
            "gain" => case.gain = num(value)?,
            "fl" => case.fl = dir.join(value),
            "carrier" => case.carrier = num(value)?,
            "bpm" => case.bpm = num(value)?,
            "ppq" => case.ppq = num(value)? as u16,
            "limit" => case.limit = num(value)?,
            "judge" => case.judge = value != "no",
            other => return Err(format!("unknown key {other}")),
        }
    }
    if case.state.is_empty() || case.dry.as_os_str().is_empty() || case.fl.as_os_str().is_empty() {
        return Err("state, dry and fl are needed".into());
    }
    Ok(case)
}

/// The link value the master's volume holds for `v`: the link's `Round(v * 2^30)`, the volume's `Round(16000 x)`.
fn as_volume(v: f64) -> f64 {
    let n = (v.clamp(0.0, 1.0) * 1_073_741_824.0).round() / 1_073_741_824.0;
    (n * 16000.0).round() / 16000.0
}

/// The link value whose volume gives `gain` (the fader's curve, `(11^(1.25 v) - 1) / 10`, turned over).
fn of_gain(gain: f64) -> f64 {
    (gain * 10.0 + 1.0).max(1e-12).ln() / 11f64.ln() / 1.25
}

/// Runs one case: the largest difference and the number of ticks compared.
fn run(case: &Case) -> Result<(f64, usize), String> {
    let dry = os_engine::sample::load(&case.dry).map_err(|e| e.to_string())?;
    let fl = os_engine::sample::load(&case.fl).map_err(|e| e.to_string())?;
    if dry.rate != fl.rate {
        return Err(format!("rates differ: {} and {}", dry.rate, fl.rate));
    }
    let mut fx = effects::from_slot("Fruity Peak Controller", &case.state, dry.rate).ok_or("state too short")?;
    let g = |i: usize| f64::from(fl.right[i]) / case.carrier / case.gain;
    let n = dry.left.len().min(fl.right.len()).saturating_sub(2);
    let mut clock = Clock::new(dry.rate, case.ppq, case.bpm);
    let (mut at, mut worst, mut compared) = (0usize, 0.0f64, 0usize);
    let mut smooth: Option<[f64; 2]> = None;
    let mut buf = Vec::new();
    loop {
        // The value the link hands on at this tick's start: what the controller's last tick left.
        let mut v = f64::from(fx.control(case.output).ok_or("no controller output")?).clamp(0.0, 1.0);
        if case.smooth {
            let s = smooth.get_or_insert([v, v]);
            s[0] += (v - s[0]) * 0.075;
            s[1] += (s[0] - s[1]) * 0.075;
            v = s[1];
        }
        if let Some(f) = &case.formula {
            v = f.map(v);
        }
        let ours = as_volume(v);
        let len = clock.advance().1;
        if at + len > n {
            break;
        }
        // The fader's step at the tick's start (its ramp lasts the tick); the first tick sets the value in place.
        let step = g(at + 1) - g(at);
        if at > 0 && step.abs() < 0.001 - 1e-7 {
            let theirs = of_gain(g(at) + len as f64 * step);
            worst = worst.max((ours - theirs).abs());
            compared += 1;
        }
        buf.clear();
        buf.extend((at..at + len).flat_map(|i| [(f64::from(dry.left[i]) * case.gain) as f32, (f64::from(dry.right[i]) * case.gain) as f32]));
        fx.process(&mut buf);
        at += len;
    }
    Ok((worst, compared))
}

#[test]
fn peak_controller_nulls_against_fl_exports() {
    let Some(dir) = std::env::var_os("OS_PC_NULL") else {
        eprintln!("OS_PC_NULL not set: the null test against FL's exports is skipped");
        return;
    };
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("OS_PC_NULL is a directory")
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
            Ok((d, k)) if !case.judge => println!("{name}: {d:.2e} over {k} ticks (not judged)"),
            Ok((d, k)) if d <= case.limit && k > 0 => println!("{name}: {d:.2e} over {k} ticks"),
            Ok((d, k)) => failed.push(format!("{name}: {d:.2e} over {k} ticks, limit {:.0e}", case.limit)),
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    assert!(failed.is_empty(), "{} of {} cases failed:\n{}", failed.len(), cases.len(), failed.join("\n"));
}
