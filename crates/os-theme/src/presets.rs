// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Built-in theme presets, in `.flstheme` text form (parsed by `FlTheme::parse`).
// spec: ui/theme-presets.md
//
// Lemon Studio ships no third-party theme data. Settings > Theme lists these, then the user's own
// `.flstheme` files from the `themes` folder in Lemon Studio's user folder (`os_paths::user_dir`).

/// (name, theme text).
pub const PRESETS: &[(&str, &str)] = &[];
