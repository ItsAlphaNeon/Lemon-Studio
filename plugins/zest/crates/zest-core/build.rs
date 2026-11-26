// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `zest_sse`: the engine uses the processor's SSE (and AVX when it has it): on x86-64 unless the `portable` feature asks
//! for the plain-Rust lanes (`src/simd/soft.rs`), as every other processor gets.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(zest_sse)");
    let x86 = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64");
    if x86 && std::env::var_os("CARGO_FEATURE_PORTABLE").is_none() {
        println!("cargo::rustc-cfg=zest_sse");
    }
}
