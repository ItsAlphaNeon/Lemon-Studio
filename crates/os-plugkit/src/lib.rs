// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

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
use os_ui::Rect;

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

/// An open submenu's parent menu, in screen px (left, top, right, bottom): the parent's panel, and the band across it
/// of the row that opened the submenu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opener {
    pub menu: [i32; 4],
    pub row: [i32; 4],
}

impl Opener {
    /// The parent panel `menu` and its row `row` (menu-local, unscaled, as `MenuLayout::rows` holds it).
    pub fn new(menu: [i32; 4], row: Rect, scale: f32) -> Self {
        let top = menu[1] + (row.y * scale).floor() as i32;
        let bottom = menu[1] + ((row.y + row.h) * scale).ceil() as i32;
        Opener { menu, row: [menu[0], top, menu[2], bottom] }
    }

    /// Whether the pointer at screen `at` leaves the submenu at `rect` for the parent: it is over the parent, off the
    /// submenu and off the row that opened it. The pointer crosses that row on its way into the submenu, so there the
    /// submenu stays open.
    pub fn takes_back(&self, rect: [i32; 4], at: (i32, i32)) -> bool {
        let inside = |r: [i32; 4]| at.0 >= r[0] && at.0 < r[2] && at.1 >= r[1] && at.1 < r[3];
        inside(self.menu) && !inside(rect) && !inside(self.row)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A 200 × 200 menu at (100, 100) whose row at y 40, 20 high, opened a submenu placed the way the popups place it:
    /// at the row's right end less 2 px (`left` true: flipped to the left of the menu).
    fn menus(left: bool) -> (Opener, [i32; 4]) {
        let opener = Opener::new([100, 100, 300, 300], Rect::new(4.0, 40.0, 192.0, 20.0), 1.0);
        let x = if left { 100 - 150 } else { 100 + 196 - 2 };
        (opener, [x, 138, x + 150, 238])
    }

    #[test]
    fn crossing_the_row_reaches_the_submenu() {
        for left in [false, true] {
            let (o, sub) = menus(left);
            assert_eq!(o.row, [100, 140, 300, 160], "the row's band spans the menu");
            // From the middle of the row, one pixel at a time into the submenu (across the menu's padding too).
            let path: Vec<i32> = if left { (60..=200).rev().collect() } else { (200..=320).collect() };
            for x in path {
                assert!(!o.takes_back(sub, (x, 150)), "left {left}: at x {x} the submenu stays");
            }
        }
    }

    #[test]
    fn another_row_of_the_parent_takes_the_pointer_back() {
        let (o, sub) = menus(false);
        assert!(o.takes_back(sub, (200, 170)), "the row below");
        assert!(o.takes_back(sub, (200, 139)), "just above the row");
        assert!(o.takes_back(sub, (200, 101)), "the menu's top padding");
        assert!(!o.takes_back(sub, (296, 200)), "the submenu covers the menu's edge");
        assert!(!o.takes_back(sub, (500, 150)), "off both menus");
        assert!(!o.takes_back(sub, (200, 320)), "below the menu");
    }

    #[test]
    fn the_row_band_rounds_outward_when_scaled() {
        let o = Opener::new([10, 20, 310, 320], Rect::new(4.0, 40.5, 192.0, 20.25), 1.5);
        assert_eq!(o.row, [10, 20 + 60, 310, 20 + 92]);
    }
}
