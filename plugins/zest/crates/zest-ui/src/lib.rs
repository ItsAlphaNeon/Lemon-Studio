// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Zest's editor, everything but the window: the pages (Main, Effects, Advanced) and their layout, painting with
//! Lemon Studio's UI kit in the "Pith" plugin look, hit testing and input, the preset library and favourites, the
//! rules that veil controls a preset makes pointless, and the Main page's two macros.
//!
//! It knows parameters only through [`params::ParamTable`] (metadata handed over by the plugin) and [`params::Model`]
//! (values), and presets only as files: loading and saving are actions the plugin carries out ([`ui::Act`]). So the
//! engine behind it can be replaced without touching this crate.
//!
//! Spec: `zest/editor.md` (specs repo, cutting-edge branch).

pub mod library;
pub mod macros;
pub mod pages;
pub mod params;
pub mod rules;
pub mod ui;
pub mod view;
pub mod viz;

pub use params::{MemModel, Model, ParamInfo, ParamTable};
pub use ui::{Act, Btn, MenuReq, Mods, TypeTarget, Ui};

/// The Lemon design's palette (the plugin editors always draw it).
pub fn palette() -> os_theme::Palette {
    os_theme::design::palette(os_theme::design::Design::Lemon, None)
}
