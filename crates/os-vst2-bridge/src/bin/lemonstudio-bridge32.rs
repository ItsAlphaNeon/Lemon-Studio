// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The VST 2 bridge's helper process: runs one plugin for Lemon Studio (`os_vst2_bridge::helper`). No console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    std::process::exit(os_vst2_bridge::helper::main());
    #[cfg(not(windows))]
    eprintln!("the VST 2 bridge runs on Windows only");
}
