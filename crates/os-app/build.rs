// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Sets `cfg(plugin_windows)` where plugin windows exist, and `cfg(lemon_updater)` when `LEMON_UPDATER` names the
//! updater to carry (release builds, `crate::update`). The program's icon and version resource are `lemonstudio`'s.

use std::path::PathBuf;

/// Release builds: `LEMON_UPDATER` names the `lemonstudio-updater` built just before, which the program carries
/// (`update::UPDATER`, through `LEMON_UPDATER_FILE`). A build from source has none.
fn updater() {
    println!("cargo::rustc-check-cfg=cfg(lemon_updater)");
    println!("cargo:rerun-if-env-changed=LEMON_UPDATER");
    let Some(path) = std::env::var_os("LEMON_UPDATER").filter(|p| !p.is_empty()) else { return };
    let path = std::path::absolute(PathBuf::from(path)).expect("LEMON_UPDATER");
    assert!(path.is_file(), "LEMON_UPDATER names no file: {}", path.display());
    println!("cargo:rerun-if-changed={}", path.display());
    println!("cargo::rustc-env=LEMON_UPDATER_FILE={}", path.display());
    println!("cargo::rustc-cfg=lemon_updater");
}

fn main() {
    updater();
    // `plugin_windows`: plugin editors open in Lemon Studio's frame windows (Win32 on Windows, X11 on Linux, AppKit on
    // macOS), with the `plugin-windows` feature.
    println!("cargo::rustc-check-cfg=cfg(plugin_windows)");
    let os = matches!(std::env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("windows" | "linux" | "macos"));
    if os && std::env::var_os("CARGO_FEATURE_PLUGIN_WINDOWS").is_some() {
        println!("cargo::rustc-cfg=plugin_windows");
    }
}
