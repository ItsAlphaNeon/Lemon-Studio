// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `cargo run -p os-app --release` keeps Lemon Studio's own plugins current: before the window opens, a release build
//! that Cargo started from its checkout runs `bundle-plugins --install`, which rebuilds the plugins and installs the
//! ones that changed where the plugin scan (and any DAW) finds them. A plugin added to the repository then shows up at
//! the next run, without a separate step. An installed `lemonstudio`, or one started outside Cargo, builds nothing.

use std::path::Path;
use std::process::Command;

/// Set to `1`: skip the plugin build (the plugins installed already are used as they are).
const SKIP: &str = "OPENSTUDIO_NO_PLUGIN_BUILD";

/// Runs `bundle-plugins --install` when this release build was started by `cargo run` in the checkout it was built
/// from. A failed build is reported and Lemon Studio starts anyway, with the plugins installed before.
pub fn run() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    // Cargo gives the programs it runs their package's variables; started any other way, the program has none.
    let from_cargo = std::env::var_os("CARGO_MANIFEST_DIR").is_some_and(|d| d == manifest);
    if cfg!(debug_assertions) || !from_cargo || std::env::var_os(SKIP).is_some_and(|v| v == "1") {
        return;
    }
    eprintln!("Rebuilding Lemon Studio's plugins ({SKIP}=1 skips this)");
    let ok = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .args(["run", "--release", "-p", "bundle-plugins", "--", "--install"])
        .current_dir(Path::new(manifest).join("../.."))
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("The plugins could not be rebuilt (see above); starting Lemon Studio with the plugins installed before");
    }
}
