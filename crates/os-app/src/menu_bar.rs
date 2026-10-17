// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The macOS menu bar: Lemon Studio's application menu, then FILE ... HELP as native menus. Their entries are not written
//! here: each menu is filled when it opens from `views::menus::bar_model` (`update` keeps the latest), built from the
//! same `MENUS` table and project state as the dropdowns of the window's own menu bar, so a change to those menus shows
//! here too. Choosing an entry does what the dropdown's does (`take_picks`).
//!
//! The shortcuts are shown (Command for Ctrl, Option for Alt), but the keys are not the menus': the menus answer that
//! they have no key equivalent, so key presses reach the app as before and do what its own key handling does.
// spec: ui/menus.md

use std::cell::RefCell;
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSMenu, NSMenuDelegate, NSMenuItem};
use objc2_foundation::{ns_string, NSString};

use crate::views::menus::{self, BarMenu, Kind, Mark};

thread_local! {
    /// The menus as they should read now (`update`).
    static MODEL: RefCell<Vec<BarMenu>> = const { RefCell::new(Vec::new()) };
    /// Entries chosen and not handled yet: (menu, flat item index), see `menus::pick`.
    static PICKS: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
    /// The menus of the bar, by `MENUS` index, and the object that fills them and takes their choices.
    static BAR: RefCell<Option<(Vec<Retained<NSMenu>>, Retained<Target>)>> = const { RefCell::new(None) };
}

/// A menu item's tag: the menu and flat item index it does (`menus::pick`).
const PER_MENU: isize = 1 << 20;

define_class!(
    // SAFETY: NSObject has no subclassing requirements; `Target` has no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "LemonStudioMenuBar"]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSMenuDelegate for Target {
        /// A menu opens: its entries as `MODEL` has them.
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            fill(self, menu);
        }
    }

    impl Target {
        #[unsafe(method(pick:))]
        fn pick(&self, item: &NSMenuItem) {
            let tag = item.tag();
            PICKS.with(|p| p.borrow_mut().push(((tag / PER_MENU) as usize, (tag % PER_MENU) as usize)));
            crate::wake_ui();
        }

        /// The keys stay the app's (see the module notes).
        #[unsafe(method(menuHasKeyEquivalent:forEvent:target:action:))]
        fn has_key_equivalent(&self, _menu: &NSMenu, _event: &NSEvent, _target: *mut c_void, _action: *mut c_void) -> bool {
            false
        }
    }
);

/// Puts Lemon Studio's menu bar in place of winit's (UI thread, once the app runs).
pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let app = NSApplication::sharedApplication(mtm);
    let target: Retained<Target> = unsafe { msg_send![super(mtm.alloc::<Target>().set_ivars(())), init] };
    let bar = NSMenu::new(mtm);
    bar.addItem(&with_submenu(mtm, app_menu(mtm, &target)));
    let mut menus = Vec::new();
    for (i, m) in menus::MENUS.iter().enumerate() {
        let title = NSString::from_str(&menus::bar_title(m.title));
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &title);
        menu.setAutoenablesItems(false);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        bar.addItem(&with_submenu(mtm, menu.clone()));
        if i == menus::HELP_MENU {
            // The Help menu gets the system's search field.
            app.setHelpMenu(Some(&menu));
        }
        menus.push(menu);
    }
    app.setMainMenu(Some(&bar));
    BAR.with(|b| *b.borrow_mut() = Some((menus, target)));
}

/// The menus as they should read now (UI tick); a menu opened later shows it.
pub fn update(model: Vec<BarMenu>) {
    MODEL.with(|m| {
        let mut m = m.borrow_mut();
        if *m != model {
            *m = model;
        }
    });
}

/// Entries chosen since the last call: (menu, flat item index) for `menus::pick`.
pub fn take_picks() -> Vec<(usize, usize)> {
    PICKS.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// An item that opens `menu`.
fn with_submenu(mtm: MainThreadMarker, menu: Retained<NSMenu>) -> Retained<NSMenuItem> {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&menu.title());
    item.setSubmenu(Some(&menu));
    item
}

/// The application menu: About, Settings (OPTIONS > General settings), the system's Services and Hide entries, Quit
/// (which asks about unsaved changes, see `macos::hook_app`). These keep their usual key equivalents.
fn app_menu(mtm: MainThreadMarker, target: &Target) -> Retained<NSMenu> {
    let app = NSApplication::sharedApplication(mtm);
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Lemon Studio"));
    menu.setAutoenablesItems(false);
    let add = |title: &NSString, action: Option<Sel>, key: &NSString, mods: NSEventModifierFlags| {
        let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), title, action, key) };
        item.setKeyEquivalentModifierMask(mods);
        menu.addItem(&item);
        item
    };
    let cmd = NSEventModifierFlags::Command;
    // About and Settings are the menus' own entries.
    let own = |item: Retained<NSMenuItem>, at: Option<(usize, usize)>| {
        match at.filter(|&(m, i)| menus::item_at(m, i).is_some_and(|it| menus::works(m, it))) {
            Some((m, i)) => {
                unsafe { item.setTarget(Some(target)) };
                item.setTag(m as isize * PER_MENU + i as isize);
            }
            None => item.setEnabled(false),
        }
    };
    own(add(ns_string!("About Lemon Studio"), Some(sel!(pick:)), ns_string!(""), cmd), Some((menus::HELP_MENU, menus::HELP_ABOUT)));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let settings = menus::MENUS[menus::OPTIONS_MENU].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == "General settings");
    own(add(ns_string!("Settings…"), Some(sel!(pick:)), ns_string!(","), cmd), settings.map(|i| (menus::OPTIONS_MENU, i)));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let services = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Services"));
    let item = add(ns_string!("Services"), None, ns_string!(""), cmd);
    item.setSubmenu(Some(&services));
    app.setServicesMenu(Some(&services));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add(ns_string!("Hide Lemon Studio"), Some(sel!(hide:)), ns_string!("h"), cmd);
    add(ns_string!("Hide Others"), Some(sel!(hideOtherApplications:)), ns_string!("h"), cmd | NSEventModifierFlags::Option);
    add(ns_string!("Show All"), Some(sel!(unhideAllApplications:)), ns_string!(""), cmd);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add(ns_string!("Quit Lemon Studio"), Some(sel!(terminate:)), ns_string!("q"), cmd);
    menu
}

/// Fills `menu` (one of the bar's) with its entries from `MODEL`.
fn fill(target: &Target, menu: &NSMenu) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some(index) = BAR.with(|b| b.borrow().as_ref().and_then(|(menus, _)| menus.iter().position(|m| std::ptr::eq(&**m, menu)))) else { return };
    let Some(model) = MODEL.with(|m| m.borrow().get(index).cloned()) else { return };
    menu.removeAllItems();
    // Section headers (macOS 14 and later) are system ones; earlier ones are greyed-out entries.
    let headers = AnyClass::get(c"NSMenuItem").is_some_and(|c| c.class_method(sel!(sectionHeaderWithTitle:)).is_some());
    for row in &model.rows {
        let title = NSString::from_str(&row.label.replace("...", "…"));
        let item = match row.kind {
            Kind::Sep => NSMenuItem::separatorItem(mtm),
            Kind::Head if headers => NSMenuItem::sectionHeaderWithTitle(&title, mtm),
            Kind::Head => {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&title);
                item.setEnabled(false);
                item
            }
            Kind::Item => {
                let item = NSMenuItem::new(mtm);
                item.setTitle(&title);
                if let Some((key, mods)) = key_equivalent(row.shortcut) {
                    item.setKeyEquivalent(&NSString::from_str(&key));
                    item.setKeyEquivalentModifierMask(mods);
                }
                item.setEnabled(row.enabled);
                if let Some((m, i)) = row.pick {
                    unsafe {
                        item.setTarget(Some(target));
                        item.setAction(Some(sel!(pick:)));
                    }
                    item.setTag(m as isize * PER_MENU + i as isize);
                }
                // NSControlStateValueOn / Off.
                item.setState(isize::from(matches!(row.mark, Mark::Check(true) | Mark::Radio(true))));
                item
            }
        };
        menu.addItem(&item);
    }
}

/// Keys the application menu keeps (`app_menu`), which macOS users expect there: Hide (Command+H), Hide Others, Quit,
/// Settings. A shortcut of the window's menus on one of them is not shown (Ctrl+H, TOOLS > Stop sound, would be
/// Command+H: the key hides Lemon Studio).
const RESERVED: &[(&str, NSEventModifierFlags)] = &[
    ("h", NSEventModifierFlags::Command),
    ("h", NSEventModifierFlags::Command.union(NSEventModifierFlags::Option)),
    ("q", NSEventModifierFlags::Command),
    (",", NSEventModifierFlags::Command),
];

/// A shortcut as `MENUS` writes it ("Shift+Ctrl+S", "Alt+F8", "F5") as a menu key equivalent: the key, and Command for
/// Ctrl, Option for Alt. `None` for keys a menu cannot show, and for the application menu's own (`RESERVED`).
fn key_equivalent(shortcut: &str) -> Option<(String, NSEventModifierFlags)> {
    let mut parts: Vec<&str> = shortcut.split('+').collect();
    let key = parts.pop().filter(|k| !k.is_empty())?;
    let mut mods = NSEventModifierFlags::empty();
    for p in parts {
        mods |= match p {
            "Ctrl" => NSEventModifierFlags::Command,
            "Shift" => NSEventModifierFlags::Shift,
            "Alt" => NSEventModifierFlags::Option,
            _ => return None,
        };
    }
    // AppKit's function key characters (`NSUpArrowFunctionKey` ...).
    let function = |code: u32| char::from_u32(code).map(String::from);
    let key = match key {
        "Up" => function(0xF700)?,
        "Down" => function(0xF701)?,
        "Left" => function(0xF702)?,
        "Right" => function(0xF703)?,
        "Ins" => function(0xF727)?,
        "Del" => function(0xF728)?,
        f if f.len() > 1 && f.starts_with('F') => function(0xF703 + f[1..].parse::<u32>().ok().filter(|n| (1..=24).contains(n))?)?,
        k if k.chars().count() == 1 => k.to_lowercase(),
        _ => return None,
    };
    (!RESERVED.iter().any(|&(k, m)| k == key && m == mods)).then_some((key, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_become_key_equivalents() {
        let k = |s| key_equivalent(s).map(|(k, m)| (k.chars().map(|c| c as u32).collect::<Vec<_>>(), m));
        assert_eq!(k("Ctrl+O"), Some((vec!['o' as u32], NSEventModifierFlags::Command)));
        assert_eq!(k("Shift+Ctrl+S"), Some((vec!['s' as u32], NSEventModifierFlags::Shift | NSEventModifierFlags::Command)));
        assert_eq!(k("F5"), Some((vec![0xF708], NSEventModifierFlags::empty())));
        assert_eq!(k("Alt+F8"), Some((vec![0xF70B], NSEventModifierFlags::Option)));
        assert_eq!(k("Shift+Ctrl+Up"), Some((vec![0xF700], NSEventModifierFlags::Shift | NSEventModifierFlags::Command)));
        assert_eq!(k(""), None);
        // The application menu's keys stay its own: TOOLS > Stop sound shows no shortcut.
        assert_eq!(k("Ctrl+H"), None);
        assert!(k("Shift+Ctrl+H").is_some());
    }
}
