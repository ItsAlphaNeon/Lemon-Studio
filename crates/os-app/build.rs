// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

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

fn main() {
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
