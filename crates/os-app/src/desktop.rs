// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The desktop Lemon Studio runs on. A tiling window manager places, sizes and closes windows with its own keys, so
//! the tiling layout's toolbar leaves out the caption buttons there (`views::toolbar::shown`), unless the window floats.
//!
//! Wayland has a standard for this (xdg-shell's `wm_capabilities` and the `tiled_*` states of a configure), which
//! winit 0.30 does not pass on; until it does, the desktop's name (`XDG_CURRENT_DESKTOP`) says whether it tiles, and on
//! Hyprland its socket says whether the window floats.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// Desktops whose window manager tiles (names as `XDG_CURRENT_DESKTOP` gives them, compared without case).
const TILING: &[&str] = &["hyprland", "sway", "i3", "niri", "river", "dwl", "qtile", "bspwm", "awesome", "herbstluftwm", "xmonad", "dwm", "leftwm", "wayfire-tiling", "mango", "miracle-wm"];

/// The window manager tiles windows (any of the `:`-separated names in `XDG_CURRENT_DESKTOP` is a tiling one).
/// Tests set it themselves (`set_tiling`): none tiles otherwise, whatever desktop runs them.
pub fn tiling() -> bool {
    if cfg!(test) {
        return TEST_TILING.with(|t| t.get().0);
    }
    static T: OnceLock<bool> = OnceLock::new();
    *T.get_or_init(|| std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| tiles(&d)))
}

thread_local! {
    static TEST_TILING: std::cell::Cell<(bool, bool)> = const { std::cell::Cell::new((false, false)) };
}

/// Tests: whether the desktop tiles, and whether the window floats on it (for the calling thread).
#[cfg(test)]
pub fn set_tiling(tiling: bool, floating: bool) {
    TEST_TILING.with(|t| t.set((tiling, floating)));
}

fn tiles(desktop: &str) -> bool {
    desktop.split(':').any(|n| TILING.iter().any(|t| n.trim().eq_ignore_ascii_case(t)))
}

/// The main window floats on a tiling desktop (as last asked, `window_changed`).
static FLOATING: AtomicBool = AtomicBool::new(false);

/// Whether the window manager leaves the window's caption buttons to it: not tiling, or the window floats.
pub fn captions() -> bool {
    let floating = if cfg!(test) { TEST_TILING.with(|t| t.get().1) } else { FLOATING.load(Ordering::Relaxed) };
    !tiling() || floating
}

/// The main window was resized or moved (made floating or tiled again comes with one): asks Hyprland whether it floats.
/// Whether that changed.
pub fn window_changed() -> bool {
    #[cfg(all(target_os = "linux", not(test)))]
    if crate::plugin_window::hyprland::running() {
        if let Some(f) = crate::plugin_window::hyprland::main_floats() {
            return FLOATING.swap(f, Ordering::Relaxed) != f;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    #[test]
    fn tiling_desktops_by_name() {
        assert!(super::tiles("Hyprland"));
        assert!(super::tiles("sway"));
        assert!(super::tiles("niri:GNOME"));
        assert!(!super::tiles("GNOME"));
        assert!(!super::tiles("KDE"));
        assert!(!super::tiles(""));
    }
}
