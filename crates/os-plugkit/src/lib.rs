// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Shared pieces of Lemon Studio's own plugin editors ("Open" plugins): editor edits the sound hears before the host
//! confirms them ([`live`]), menu definitions shown with the app's own popup look ([`MenuDef`], `os_ui::kit::menu`),
//! and the native parts every editor needs: popups, type-in boxes, painting a canvas into a window ([`win`] on Windows,
//! `x11` on Linux, `mac` on macOS). Linux and macOS share one API and input type ([`gui`]), so a plugin's editor loop
//! is the same on both.
//!
// spec: plugins/lemon-sampler.md §4 (edits apply at once, menus), plugins/lemon-parametric-eq-2.md, plugins/plugin-formats.md

pub mod linked;
pub mod live;
pub mod vals;
#[cfg(windows)]
pub mod win;
#[cfg(target_os = "linux")]
pub mod x11;
#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod input;

/// The editor window of this platform where it runs the plugins' shared editor loop: `x11` on Linux, `mac` on macOS.
#[cfg(target_os = "linux")]
pub use x11 as gui;
#[cfg(target_os = "macos")]
pub use mac as gui;

use os_theme::design::Design;
use os_theme::Palette;
use os_ui::kit::menu::{MenuItem, MenuStyle};

/// The palette Lemon Studio's plugin editors draw with: always the Lemon design's, whatever design the app or a saved
/// setting names (the plugins' Classic look is retired, spec ui/lemon-design.md §11.1). Their menus use it too.
pub fn palette() -> Palette {
    os_theme::design::palette(Design::Lemon, None)
}

/// A menu to show: rows, what each does, submenus by row.
#[derive(Clone, Debug)]
pub struct MenuDef<C> {
    pub items: Vec<MenuItem>,
    pub cmds: Vec<C>,
    pub subs: Vec<(usize, MenuDef<C>)>,
    pub style: MenuStyle,
    pub current: Option<usize>,
}

impl<C: Clone> MenuDef<C> {
    pub fn new(style: MenuStyle) -> Self {
        MenuDef { items: Vec::new(), cmds: Vec::new(), subs: Vec::new(), style, current: None }
    }

    pub fn add(&mut self, it: MenuItem, c: C) -> usize {
        self.items.push(it);
        self.cmds.push(c);
        self.items.len() - 1
    }

    pub fn head(&mut self, s: &str, none: C) {
        self.add(MenuItem::head(s), none);
    }

    pub fn sep(&mut self, none: C) {
        self.add(MenuItem::Sep, none);
    }

    /// A row that opens `sub`.
    pub fn submenu(&mut self, label: &str, sub: MenuDef<C>, none: C) -> usize {
        let i = self.add(MenuItem::item(label).sub(), none);
        self.subs.push((i, sub));
        i
    }

    pub fn sub(&self, i: usize) -> Option<&MenuDef<C>> {
        self.subs.iter().find(|(j, _)| *j == i).map(|(_, m)| m)
    }

    /// The command of the first row (searching submenus too) labelled `label` (tests).
    pub fn find(&self, label: &str) -> Option<C> {
        for (i, it) in self.items.iter().enumerate() {
            if matches!(it, MenuItem::Item { label: l, .. } if l == label) {
                return Some(self.cmds[i].clone());
            }
        }
        self.subs.iter().find_map(|(_, s)| s.find(label))
    }
}

/// True when `OS_DEBUG` is set (editor logging).
pub fn debug() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OS_DEBUG").is_some())
}

/// Tests that click at random set `OPENSTUDIO_PLUGIN_NO_MODAL` (or Lemon Sampler's `LEMONSAMPLER_NO_MODAL`): menus and
/// type-in boxes then return nothing instead of running a modal loop.
pub fn no_modal() -> bool {
    std::env::var_os("OPENSTUDIO_PLUGIN_NO_MODAL").is_some() || std::env::var_os("LEMONSAMPLER_NO_MODAL").is_some()
}
