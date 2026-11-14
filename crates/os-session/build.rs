// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Compiles the factory presets in (`crate::presets`) and rebuilds when the release workflow's version or channel
//! changes (`crate::version`; `cfg(lemon_nightly)` when `LEMON_CHANNEL` is `nightly`).

use std::path::PathBuf;

/// The factory presets in `factory-presets/<plugin>/[<folder>/]<name>.ospreset`, as a Rust slice expression of (path
/// under that folder, `/`-separated; bytes) for `presets.rs` to include (spec: plugins/presets.md "Factory presets").
fn factory_presets() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("factory-presets");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    let files = |dir: &std::path::Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        v.sort();
        v
    };
    let name = |p: &std::path::Path| p.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let preset = |p: &std::path::Path| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ospreset"));
    for plugin in files(&root).into_iter().filter(|p| p.is_dir()) {
        for p in files(&plugin) {
            if preset(&p) {
                found.push((format!("{}/{}", name(&plugin), name(&p)), p));
            } else if p.is_dir() {
                for q in files(&p).into_iter().filter(|q| preset(q)) {
                    found.push((format!("{}/{}/{}", name(&plugin), name(&p), name(&q)), q));
                }
            }
        }
    }
    let mut out = String::from("&[\n");
    for (rel, path) in found {
        out.push_str(&format!("    ({rel:?}, include_bytes!({:?}) as &[u8]),\n", path.display().to_string()));
    }
    out.push(']');
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("factory_presets.rs"), out).unwrap();
}

fn main() {
    factory_presets();
    println!("cargo:rerun-if-env-changed=LEMON_VERSION");
    println!("cargo::rustc-check-cfg=cfg(lemon_nightly)");
    println!("cargo:rerun-if-env-changed=LEMON_CHANNEL");
    if std::env::var("LEMON_CHANNEL").is_ok_and(|c| c == "nightly") {
        println!("cargo::rustc-cfg=lemon_nightly");
    }
}
