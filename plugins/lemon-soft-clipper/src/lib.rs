// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Soft Clipper: Lemon Studio's soft clipper as a VST3 / CLAP plugin module. The plugin, its parameters and its editor are
//! `lemon_small_fx::soft_clipper` (shared with the other small effects); this crate gives it its own module file.
//!
// spec: plugins/lemon-soft-clipper.md, plugins/fruity-soft-clipper.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

#[cfg(not(feature = "linked"))]
use nih_plug::prelude::*;

pub use lemon_small_fx::soft_clipper::LemonSoftClipper;

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonSoftClipper);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonSoftClipper);
os_plugkit::linked_clap_entry!(LemonSoftClipper);
