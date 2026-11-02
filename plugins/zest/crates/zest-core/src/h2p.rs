// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Diva `.h2p` presets: see `pages/presets-h2p.md`. Each `ParamDef` carries its `.h2p` name.

use crate::params::{id, COUNT, PARAMS};

/// A preset as Diva loads it over its startup state: defaults, then the preset, keeping the values presets don't store
/// (Accuracy, OfflineAcc, MultiCore).
pub fn load(text: &str, current: &[f32; COUNT]) -> Result<[f32; COUNT], String> {
    let mut v: [f32; COUNT] = core::array::from_fn(|i| PARAMS[i].default);
    for i in [id::OPT_ACCURACY, id::OPT_OFFLINEACC, id::VCC_MULTICORE] {
        v[i] = current[i];
    }
    apply(text, &mut v)?;
    Ok(v)
}

/// The first parameter of `module` stored under the short name `short`.
fn find(module: &str, short: &str) -> Option<usize> {
    PARAMS.iter().position(|d| {
        !d.h2p.is_empty() && d.h2p == short && d.key.len() > module.len() && d.key.as_bytes()[module.len()] == b'/' && d.key.starts_with(module)
    })
}

/// The text section onto `values` (`pages/presets-h2p.md`).
fn apply(text: &str, values: &mut [f32; COUNT]) -> Result<(), String> {
    let mut module = "";
    let mut seen = false;
    let mut count = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("// Section for ugly compressed binary Data") {
            break;
        }
        if let Some(rest) = line.strip_prefix("#cm=") {
            module = rest;
            seen = true;
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        if k.starts_with('#') {
            let slot = match k {
                "#FX1" => id::MAIN_ACTIVE_FX1,
                "#FX2" => id::MAIN_ACTIVE_FX2,
                _ => continue,
            };
            values[slot] = if v.trim() == "1" { 1.0 } else { 0.0 };
            count += 1;
            continue;
        }
        if !seen {
            continue;
        }
        let v = v.trim();
        if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
            if k == "Module" && (module == "FX1" || module == "FX2") {
                let i = if module == "FX1" { id::FX1_MODULE } else { id::FX2_MODULE };
                if let Some(pos) = PARAMS[i].labels.iter().position(|l| *l == &v[1..v.len() - 1]) {
                    values[i] = PARAMS[i].min + pos as f32;
                    count += 1;
                }
            }
            continue;
        }
        let Ok(x) = v.parse::<f32>() else { continue };
        if let Some(i) = find(module, k) {
            values[i] = x.clamp(PARAMS[i].min, PARAMS[i].max);
            count += 1;
        }
    }
    if count == 0 {
        return Err("no Diva parameters found".into());
    }
    Ok(())
}

/// A Diva preset as a Zest preset file: the values that differ from the defaults, its pots and the header's author and
/// categories. `name` is what the file shows.
pub fn to_zest(text: &str, name: &str) -> Result<zest_preset::PresetFile, String> {
    let cur: [f32; COUNT] = core::array::from_fn(|i| PARAMS[i].default);
    let vals = load(text, &cur)?;
    let params = PARAMS.iter().zip(vals.iter()).filter(|(d, v)| **v != d.default).map(|(d, v)| (d.key.to_string(), *v)).collect();
    let meta = text.strip_prefix("/*@Meta").and_then(|t| t.split("*/").next()).unwrap_or("");
    let field = |k: &str| -> Option<String> {
        let after = meta.split(&format!("{k}:")).nth(1)?;
        let v = after.trim_start().strip_prefix('\'')?.split('\'').next()?.trim();
        (!v.is_empty()).then(|| v.to_string())
    };
    let s = crate::slop::parse_preset(text);
    Ok(zest_preset::PresetFile {
        head: zest_preset::Head { name: Some(name.to_string()), author: field("Author"), categories: field("Categories") },
        spread: s.into(),
        params,
    })
}

/// A Zest preset's values over the defaults, keeping what presets don't store. Unknown keys are skipped.
pub fn zest_values(p: &zest_preset::PresetFile, current: &[f32; COUNT]) -> [f32; COUNT] {
    let mut v: [f32; COUNT] = core::array::from_fn(|i| PARAMS[i].default);
    for i in [id::OPT_ACCURACY, id::OPT_OFFLINEACC, id::VCC_MULTICORE] {
        v[i] = current[i];
    }
    for (k, x) in &p.params {
        if let Some(i) = PARAMS.iter().position(|d| d.key == k) {
            v[i] = x.clamp(PARAMS[i].min, PARAMS[i].max);
        }
    }
    v
}
