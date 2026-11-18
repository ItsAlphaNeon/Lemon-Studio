// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Release builds: `LEMON_BRIDGE32` names the 32-bit bridge's helper (`lemonstudio-bridge32.exe`, built for
//! i686-pc-windows-msvc just before), which the program carries (`bridged::helper`, through `LEMON_BRIDGE32_FILE`). A
//! build from source has none and finds the one Cargo built beside it.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(lemon_bridge32)");
    println!("cargo:rerun-if-env-changed=LEMON_BRIDGE32");
    let Some(path) = std::env::var_os("LEMON_BRIDGE32").filter(|p| !p.is_empty()) else { return };
    let path = std::path::absolute(std::path::PathBuf::from(path)).expect("LEMON_BRIDGE32");
    assert!(path.is_file(), "LEMON_BRIDGE32 names no file: {}", path.display());
    println!("cargo:rerun-if-changed={}", path.display());
    println!("cargo::rustc-env=LEMON_BRIDGE32_FILE={}", path.display());
    println!("cargo::rustc-cfg=lemon_bridge32");
}
