// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Delay compensation against FL's exports: projects with Fruity Limiter in mixer slots, rendered with Lemon Studio's
//! recreation of its processing as a slot that reports its look-ahead. FL starts the export late by the delay, so the
//! file shows none; the engine's render must line up with FL's from the first sample, at the same length.
//!
//! Runs only when `OPENSTUDIO_PDC_EXPORTS` names a folder of `<name>.flp` projects next to FL's `<name>.wav` exports
//! (32-bit float); prints each case's lag and how far its difference to FL's export lies below FL's peak.
//!
//! `OPENSTUDIO_PDC_EXPORTS=<folder> cargo test -p os-render --test pdc_null -- --nocapture`
// spec: audio/delay-compensation.md, plugins/fruity-limiter.md

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use os_engine::effects::dynamics::{Dynamics, PARAMS};
use os_engine::{Engine, HostedEffect, PlayMode, SampleLibrary};

/// The saved state's flags byte (plugins/fruity-limiter.md).
const FLAGS_BYTE: usize = 168;

/// Lemon Studio's recreation of Fruity Limiter in a slot, reporting its look-ahead as the plugin does.
struct Limiter(Dynamics);

impl HostedEffect for Limiter {
    fn process_fx(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
        self.0.process(left, right, None);
    }
    fn reported_delay_fx(&self) -> u32 {
        self.0.latency() as u32
    }
}

/// The 18 parameters, int32 fields 1..=18 of the saved state, and its flags.
fn limiter_state(state: &[u8]) -> Option<([i32; PARAMS], u8)> {
    let field = |k: usize| state.get(4 * k..4 * k + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()));
    let p: Vec<i32> = (1..=PARAMS).map(field).collect::<Option<_>>()?;
    Some((p.try_into().ok()?, *state.get(FLAGS_BYTE)?))
}

/// The case's render: the project with every Fruity Limiter slot replaced; the slots replaced, and the frames the
/// export skipped at its start.
fn render(flp: &Path, rate: u32) -> (Vec<f32>, usize) {
    let bytes = std::fs::read(flp).unwrap();
    let file = os_flp::FlpFile::parse(&bytes).unwrap();
    let project = os_flp::project::load(&file).unwrap();
    let song = project.arrangements.iter().any(|a| !a.clips.is_empty());
    let mode = if song { PlayMode::Song } else { PlayMode::Pattern(1) };
    let mut library = SampleLibrary::new(SampleLibrary::default_factory_dir());
    let mut e = Engine::new(&project, &mut library, rate, mode);
    let mut limiters = 0;
    for (t, m) in project.mixer.iter().enumerate() {
        for (s, slot) in m.slots.iter().enumerate() {
            let Some((p, flags)) = slot.as_ref().filter(|p| p.name == "Fruity Limiter").and_then(|p| limiter_state(&p.state)) else {
                continue;
            };
            e.set_effect(t, s, Arc::new(Mutex::new(Limiter(Dynamics::new(f64::from(rate), p, flags)))));
            limiters += 1;
        }
    }
    assert!(library.failures.is_empty(), "{}: missing samples {:?}", flp.display(), library.failures);
    (e.render_all(0), limiters)
}

#[test]
fn exports_with_look_ahead_limiters_show_no_delay() {
    let Some(dir) = std::env::var_os("OPENSTUDIO_PDC_EXPORTS").map(PathBuf::from) else {
        eprintln!("OPENSTUDIO_PDC_EXPORTS not set; skipping");
        return;
    };
    let mut flps: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();
    flps.retain(|p| p.extension().is_some_and(|x| x == "flp") && p.with_extension("wav").exists());
    flps.sort();
    let mut cases = 0;
    for flp in flps {
        let name = flp.file_stem().unwrap().to_string_lossy().into_owned();
        let fl = os_engine::sample::load(&flp.with_extension("wav")).unwrap();
        let (ours, limiters) = render(&flp, fl.rate);
        if limiters == 0 {
            continue;
        }
        let frames = ours.len() / 2;
        // The difference sample by sample, and where it would be smallest within 200 frames either way.
        let diff = |lag: i64| {
            let mut d = 0.0f32;
            for f in 0..fl.frames() as i64 {
                let g = f + lag;
                let (l, r) = if (0..frames as i64).contains(&g) { (ours[2 * g as usize], ours[2 * g as usize + 1]) } else { (0.0, 0.0) };
                d = d.max((l - fl.left[f as usize]).abs()).max((r - fl.right[f as usize]).abs());
            }
            d
        };
        let peak = fl.left.iter().chain(&fl.right).fold(0.0f32, |m, s| m.max(s.abs()));
        let db = |d: f32| 20.0 * (f64::from(d) / f64::from(peak)).max(1e-50).log10();
        let at = diff(0);
        let best = (-200..=200).min_by(|&a, &b| diff(a).total_cmp(&diff(b))).unwrap();
        eprintln!(
            "{name}: {frames} frames (FL {}), {limiters} limiter(s); difference {:.1} dB below FL's peak, smallest at lag {best}",
            fl.frames(),
            -db(at)
        );
        assert_eq!(frames, fl.frames(), "{name}: the file's length");
        assert_eq!(best, 0, "{name}: in line with FL's export");
        cases += 1;
    }
    assert!(cases > 0, "no case with a Fruity Limiter slot in {}", dir.display());
}
