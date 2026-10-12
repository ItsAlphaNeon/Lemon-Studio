//! Shared pieces of Lemon Studio's own plugin editors ("Open" plugins): editor edits the sound hears before the host
//! confirms them ([`live`]), menu definitions shown with the app's own popup look ([`MenuDef`], `os_ui::kit::menu`),
//! and the Win32 parts every editor needs ([`win`]: popups, type-in boxes, painting a canvas into a window).
//!
// spec: plugins/open-sampler.md §4 (edits apply at once, menus), plugins/open-parametric-eq-2.md, plugins/plugin-formats.md

pub mod linked;
pub mod live;
pub mod vals;
#[cfg(windows)]
pub mod win;

use std::path::PathBuf;

use os_theme::Palette;
use os_ui::kit::menu::{MenuItem, MenuStyle};

/// The palette Lemon Studio uses: the remembered `.flstheme` (`%APPDATA%/Lemon Studio/current.flstheme`, spec
/// ui/flstheme.md), else the default one. A plugin runs in its own module, so it reads the theme itself.
pub fn palette() -> Palette {
    if cfg!(test) {
        return Palette::default();
    }
    std::env::var_os("APPDATA")
        .map(|d| PathBuf::from(d).join("Lemon Studio").join("current.flstheme"))
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| os_theme::fl::palette(&os_theme::FlTheme::parse(&String::from_utf8_lossy(&b))))
        .unwrap_or_default()
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

/// Tests that click at random set `OPENSTUDIO_PLUGIN_NO_MODAL` (or Open Sampler's `OPENSAMPLER_NO_MODAL`): menus and
/// type-in boxes then return nothing instead of running a modal loop.
pub fn no_modal() -> bool {
    std::env::var_os("OPENSTUDIO_PLUGIN_NO_MODAL").is_some() || std::env::var_os("OPENSAMPLER_NO_MODAL").is_some()
}
