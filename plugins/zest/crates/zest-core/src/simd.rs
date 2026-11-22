// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The SSE intrinsics the engine is written in: the processor's own on x86-64, else (and with the `portable` feature)
//! `soft`, the same operations in plain Rust giving the same bits, so Zest sounds the same on every processor.

#[cfg(zest_sse)]
pub use core::arch::x86_64::*;

#[cfg(not(zest_sse))]
mod soft;
#[cfg(not(zest_sse))]
pub use soft::*;
