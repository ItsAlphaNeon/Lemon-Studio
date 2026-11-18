// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The VST 2 bridge: 32-bit plugins run in a helper process (`lemonstudio-bridge32`, built for 32-bit Windows) and
//! the host talks to them through shared memory. This crate holds what both sides share (the messages, the channels)
//! and the helper itself; the host's side is `os_vst2::bridged`.
//!
// spec: plugins/vst2-bridge.md

/// The plugin interface, the same file `os-vst2` uses (it compiles to the 32-bit layouts in a 32-bit build).
#[path = "../../os-vst2/src/abi.rs"]
pub mod abi;
#[cfg(windows)]
pub mod helper;
#[cfg(windows)]
pub mod ipc;
pub mod proto;
pub mod testplug;
