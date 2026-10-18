// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Built-in themes, `.flstheme` files in `themes/` (parsed by `FlTheme::parse`). They colour either skin.
// spec: ui/theme-presets.md
//
// Settings > Theme > Themes lists the Lemon and Citrus themes (the skins' own colours), these, then the user's own
// `.flstheme` files from the `themes` folder in Lemon Studio's user folder (`os_paths::user_dir`).

macro_rules! presets {
    ($($name:literal),* $(,)?) => {
        /// (name, theme text), dark themes first.
        pub const PRESETS: &[(&str, &str)] = &[$(($name, include_str!(concat!("../themes/", $name, ".flstheme"))),)*];
    };
}

presets!(
    "Graphite",
    "Frost",
    "Persimmon",
    "Mango",
    "Spearmint",
    "Moss",
    "Cinnamon",
    "Dragonfruit",
    "Afterlight",
    "Blueshift",
    "Low Profile",
    "Charcoal",
    "Salamander",
    "Velour",
    "Lagoon",
    "Cobalt",
    "Orchid",
    "Jade",
    "Moss Light",
    "Cinnamon Light",
    "Raspberry",
    "Kiwi",
    "Apricot",
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FlTheme;

    /// Every built-in theme parses, none is the stock Dark theme (each is its own colours), and all differ.
    #[test]
    fn presets_parse_and_differ() {
        assert_eq!(PRESETS.len(), 23);
        let themes: Vec<FlTheme> = PRESETS.iter().map(|(_, t)| FlTheme::parse(t)).collect();
        for (th, (name, _)) in themes.iter().zip(PRESETS) {
            assert!(th.selected.is_some() && th.highlight.is_some() && th.text_color.is_some(), "{name}");
            assert!(!th.is_stock(), "{name}");
        }
        for i in 0..themes.len() {
            for j in i + 1..themes.len() {
                assert_ne!(themes[i], themes[j], "{} {}", PRESETS[i].0, PRESETS[j].0);
            }
        }
    }
}
