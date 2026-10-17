// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Compiles `src/guard.c` (structured exception handling around calls into plugins) on Windows MSVC targets and sets
//! `cfg(plugin_guard)`. Elsewhere, or without a C compiler, plugin calls are made directly and crashes are not contained.

fn main() {
    println!("cargo::rustc-check-cfg=cfg(plugin_guard)");
    println!("cargo:rerun-if-changed=src/guard.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    match cc::Build::new().file("src/guard.c").try_compile("os_host_guard") {
        Ok(()) => println!("cargo::rustc-cfg=plugin_guard"),
        Err(e) => println!("cargo:warning=no C compiler for src/guard.c ({e}); plugin crashes will not be contained"),
    }
}
