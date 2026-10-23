// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Stereo Enhancer: Lemon Studio's stereo enhancer as a VST3 / CLAP plugin module. The plugin, its parameters and its editor are
//! `lemon_small_fx::stereo_enhancer` (shared with the other small effects); this crate gives it its own module file.
//!
// spec: plugins/lemon-stereo-enhancer.md, plugins/fruity-stereo-enhancer.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

#[cfg(not(feature = "linked"))]
use nih_plug::prelude::*;

pub use lemon_small_fx::stereo_enhancer::LemonStereoEnhancer;

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonStereoEnhancer);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonStereoEnhancer);
os_plugkit::linked_clap_entry!(LemonStereoEnhancer);
