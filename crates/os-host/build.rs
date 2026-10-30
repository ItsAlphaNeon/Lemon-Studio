// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Compiles the crash guard around calls into plugins and sets `cfg(plugin_guard)`: `src/guard.c` (structured
//! exception handling) on Windows MSVC targets, `src/guard_unix.c` (a signal guard) on macOS, Linux and other unix
//! targets. Elsewhere, or without a C compiler, plugin calls are made directly and crashes are not contained.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(plugin_guard)");
    println!("cargo:rerun-if-changed=src/guard.c");
    println!("cargo:rerun-if-changed=src/guard_unix.c");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let unix = std::env::var_os("CARGO_CFG_UNIX").is_some();
    let file = match () {
        _ if os == "windows" && env == "msvc" => "src/guard.c",
        _ if unix => "src/guard_unix.c",
        _ => return,
    };
    match cc::Build::new().file(file).try_compile("os_host_guard") {
        Ok(()) => println!("cargo::rustc-cfg=plugin_guard"),
        Err(e) => println!("cargo:warning=no C compiler for {file} ({e}); plugin crashes will not be contained"),
    }
}
