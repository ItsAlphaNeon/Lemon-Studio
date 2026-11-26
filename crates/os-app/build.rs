// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Sets `cfg(plugin_windows)` where plugin windows exist, `cfg(lemon_cards)` where built-in editors are drawn in the app, and `cfg(lemon_updater)` when `LEMON_UPDATER` names the
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
    let has_plugin_windows = std::env::var_os("CARGO_FEATURE_PLUGIN_WINDOWS").is_some();
    if os && has_plugin_windows {
        println!("cargo::rustc-cfg=plugin_windows");
    }
    // `lemon_cards`: the plugins compiled into Lemon Studio draw their editors in its own window, as Lemon cards
    // (`plugin_window::EmbedFrame`), and several of one plugin join as tabs (`plugin_tabs`). Linux and macOS; Windows is
    // next.
    println!("cargo::rustc-check-cfg=cfg(lemon_cards)");
    if matches!(std::env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("linux" | "macos")) && has_plugin_windows {
        println!("cargo::rustc-cfg=lemon_cards");
    }
}
