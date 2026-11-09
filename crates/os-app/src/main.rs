// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio: the binary. Everything is in the library (`lib.rs`).

// Windows: a GUI program, so started from Explorer, the Start menu or a shortcut it opens no console window beside its
// own. Started from a terminal it still writes there (`attach_console`).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    os_app::run();
}
