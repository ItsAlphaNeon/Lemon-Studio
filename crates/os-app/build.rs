// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Embeds `assets/lemonstudio.ico` as icon resource 1 (Windows). Uses the Windows SDK's `rc.exe`
//! (or `$RC`); without it the build goes on and the exe just has no icon. Also sets `cfg(plugin_windows)` where plugin
//! windows exist.

use std::path::PathBuf;
use std::process::Command;

fn find_rc() -> Option<PathBuf> {
    if let Ok(rc) = std::env::var("RC") {
        return Some(rc.into());
    }
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "x64",
    };
    let pf = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let bin = PathBuf::from(pf).join(r"Windows Kits\10\bin");
    let mut vers: Vec<PathBuf> = std::fs::read_dir(&bin).ok()?.flatten().map(|e| e.path()).collect();
    vers.sort();
    vers.into_iter().rev().map(|v| v.join(arch).join("rc.exe")).find(|p| p.exists())
}

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
    // `plugin_windows`: plugin editors open in Lemon Studio's frame windows (Win32 on Windows, X11 on Linux, AppKit on
    // macOS).
    println!("cargo::rustc-check-cfg=cfg(plugin_windows)");
    if matches!(std::env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("windows" | "linux" | "macos")) {
        println!("cargo::rustc-cfg=plugin_windows");
    }
    println!("cargo:rerun-if-changed=assets/lemonstudio.rc");
    println!("cargo:rerun-if-changed=assets/lemonstudio.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found (Windows SDK); building without the app icon");
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("lemonstudio.res");
    let ok = Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&out)
        .arg("assets/lemonstudio.rc")
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("cargo:rustc-link-arg-bins={}", out.display());
    } else {
        println!("cargo:warning=rc.exe failed; building without the app icon");
    }
}
