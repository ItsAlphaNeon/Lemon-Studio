// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

// spec: plugins/fruity-parametric-eq-2.md (Checked against FL)
//
// Parametric EQ 2 against FL's exports. Runs only when OPENSTUDIO_EQ_EXPORTS names a folder of projects
// (`<name>.flp`) with FL's export of each beside it (`<name>.wav`). Every project there with the plugin in a mixer
// slot is rendered with the engine and compared with the export frame by frame; the figures are printed (run with
// `--nocapture`). A sample a project names that is not where it says is looked for by its file name in that folder,
// its parent and their subfolders (the folders may have moved since). The exports are never committed.

use std::path::{Path, PathBuf};

use os_engine::effects::eq::{Mode, Options};
use os_engine::{Engine, PlayMode, SampleLibrary};

const EQ: &str = "Fruity Parametric EQ 2";

/// The largest difference allowed, in dB of full scale: LIN's transforms in doubles come to -127 dB at worst
/// (spec: "Checked against FL"), the other modes to far less. An export in 16-bit is allowed one step of it.
const LIMIT_DB: f64 = -120.0;
const STEP_16: f64 = 1.0 / 32768.0;

/// Where a sample the project names is now: as named, or a file of that name near the exports.
fn find_sample(named: &str, dir: &Path) -> Option<PathBuf> {
    if Path::new(named).is_file() {
        return None;
    }
    let file = Path::new(&named.replace('\\', "/")).file_name()?.to_owned();
    let mut dirs = vec![dir.to_path_buf()];
    dirs.extend(dir.parent().map(Path::to_path_buf));
    for d in dirs.clone() {
        dirs.extend(std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()));
    }
    dirs.into_iter().map(|d| d.join(&file)).find(|p| p.is_file())
}

#[test]
fn parametric_eq_matches_fl_exports() {
    let Some(dir) = std::env::var_os("OPENSTUDIO_EQ_EXPORTS") else {
        eprintln!("OPENSTUDIO_EQ_EXPORTS not set; skipping the EQ null test");
        return;
    };
    let mut projects: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", Path::new(&dir).display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("flp")) && p.with_extension("wav").is_file())
        .collect();
    projects.sort();

    let mut failures = Vec::new();
    let mut compared = 0;
    for path in &projects {
        let name = path.file_stem().unwrap().to_string_lossy();
        let bytes = std::fs::read(path).unwrap();
        let Some(mut project) = os_flp::FlpFile::parse(&bytes).ok().and_then(|f| os_flp::project::load(&f).ok()) else {
            eprintln!("{name}: not read");
            continue;
        };
        let eqs: Vec<Options> = project
            .mixer
            .iter()
            .flat_map(|m| m.slots.iter().flatten())
            .filter(|s| s.name == EQ)
            .map(|s| Options::from_state(&s.state))
            .collect();
        if eqs.is_empty() {
            continue;
        }
        for ch in &mut project.channels {
            if let Some(found) = ch.sample_path.as_deref().and_then(|p| find_sample(p, Path::new(&dir))) {
                ch.sample_path = Some(found.to_string_lossy().into_owned());
            }
        }
        let fl = os_engine::sample::load(&path.with_extension("wav")).unwrap();
        let in_16_bit = fl.left.iter().chain(&fl.right).all(|&v| (f64::from(v) / STEP_16).fract() == 0.0);
        let limit = if in_16_bit { 20.0 * STEP_16.log10() + 1e-9 } else { LIMIT_DB };
        let mut library = SampleLibrary::new(SampleLibrary::default_factory_dir());
        let ours = Engine::new(&project, &mut library, fl.rate, PlayMode::Song).render_all(512);
        // The render skips the slots' delay as FL's export does (spec: audio/delay-compensation.md).
        let frames = (ours.len() / 2).min(fl.frames());
        let mut worst = [0.0f64; 2];
        for i in 0..frames {
            for (c, theirs) in [&fl.left, &fl.right].into_iter().enumerate() {
                let d = (f64::from(ours[2 * i + c]) - f64::from(theirs[i])).abs();
                worst[c] = worst[c].max(d);
            }
        }
        let db = worst.map(|d| if d > 0.0 { 20.0 * d.log10() } else { f64::NEG_INFINITY });
        let modes: Vec<Mode> = eqs.iter().map(|o| o.mode()).collect();
        eprintln!(
            "{name}: {modes:?}, {} Hz{}, {frames} frames (FL {}, ours {}): {:.1} / {:.1} dB",
            fl.rate,
            if in_16_bit { ", 16-bit" } else { "" },
            fl.frames(),
            ours.len() / 2,
            db[0],
            db[1]
        );
        compared += 1;
        if db.iter().any(|&d| d > limit) {
            failures.push(format!("{name}: {:.1} / {:.1} dB", db[0], db[1]));
        }
    }
    eprintln!("{compared} exports compared");
    assert!(failures.is_empty(), "differ from FL's exports:\n{}", failures.join("\n"));
}
