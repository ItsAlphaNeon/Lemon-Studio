// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Context menus: the component handler answers `IComponentHandler3`, whose `createContextMenu` gives the plug-in an
//! `IContextMenu` holding the host's entries for the parameter right-clicked in its editor ("Create automation clip").
//! The plug-in shows them in a menu of its own, or adds its entries and asks the host to pop the whole menu up: the
//! host keeps a copy (with its own references to the targets) for the app, which shows it as one of Lemon Studio's
//! popups (`os_host::Plugin::take_menu`); a pick runs later on the UI thread (`os_host::Plugin::menu_done`).
// spec: plugins/vst3-hosting.md §9, plugins/plugin-parameters.md §6 (plugin knob menus)
// The binding's COM macros end statements with `;` inside expressions.
#![allow(semicolon_in_expressions_from_non_local_macros)]

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use vst3_sys as vst3_com;
use vst3_sys::base::{kInvalidArgument, kResultFalse, kResultOk, tresult, IUnknown};
use vst3_sys::vst::{IContextMenu, IContextMenuItem, IContextMenuTarget, String128};
use vst3_sys::{c_void, com_interface, VstPtr, VST3};

use crate::host::{from_utf16, write_str128, Com, Handler, Shared};

/// `IComponentHandler3` as documented: `createContextMenu` takes a pointer to the parameter id, null when the click
/// was not on a parameter (the binding has the id by value).
#[com_interface("69F11617-D26B-400D-A4B6-B9647B6EBBAB")]
pub trait IComponentHandler3: IUnknown {
    unsafe fn create_context_menu(&self, plug_view: *mut c_void, param_id: *const u32) -> *mut c_void;
}

// `IContextMenuItem::flags`: a submenu starts with an entry flagged 8 | disabled (its name is the submenu's title) and
// ends with one flagged 16 | separator.
const IS_SEPARATOR: i32 = 1;
const IS_DISABLED: i32 = 2;
const IS_CHECKED: i32 = 4;
const GROUP_START: i32 = 8;
const GROUP_END: i32 = 16;

/// Tag of the host's "Create automation clip" entry (its separator has 0).
const CREATE_AUTOMATION: i32 = 1;
const CREATE_AUTOMATION_LABEL: &str = "Create automation clip";

/// Menu ids, unique across instances (`os_host::Menu::id`).
static NEXT_MENU: AtomicU64 = AtomicU64::new(1);

/// A menu entry and its target. The host holds a reference to the target (`addItem` takes one).
#[derive(Clone)]
pub struct Entry {
    name: String128,
    tag: i32,
    flags: i32,
    target: Option<VstPtr<dyn IContextMenuTarget>>,
    /// One of the host's own entries.
    host: bool,
}

impl Entry {
    fn target_ptr(&self) -> *mut c_void {
        self.target.as_ref().map_or(std::ptr::null_mut(), |t| t.as_ptr() as *mut c_void)
    }

    /// A row the user can pick: not a separator, a submenu's start or end, nor disabled.
    fn pickable(&self) -> bool {
        self.flags & (IS_SEPARATOR | IS_DISABLED | GROUP_START | GROUP_END) == 0
    }
}

/// A menu the plug-in asked the host to pop up: its entries in the order shown, each holding the host's own reference
/// to its target until the user picks one or closes the menu, a newer popup replaces it, or the editor closes.
pub struct Popup {
    id: u64,
    /// Where, in the view's coordinates.
    at: (i32, i32),
    entries: Vec<Entry>,
    /// Handed to the app.
    shown: bool,
}

/// The menu `createContextMenu` returns. Its first entries are the host's (for an automatable parameter a separator
/// and "Create automation clip"); the plug-in adds its own after them.
#[VST3(implements(IContextMenu))]
pub struct ContextMenu {
    shared: Arc<Shared>,
    entries: RefCell<Vec<Entry>>,
}

/// The host's entries' target: "Create automation clip" for one parameter. It lives as long as the menu (and the
/// plug-in's or a popup's references to it).
#[VST3(implements(IContextMenuTarget))]
pub struct HostTarget {
    shared: Arc<Shared>,
    param: u32,
}

impl IContextMenuTarget for HostTarget {
    /// The pick reaches the app as an automation request for the parameter (`os_host::Plugin::take_automation_requests`),
    /// as the CLAP entry's does.
    unsafe fn execute_menu_item(&self, tag: i32) -> tresult {
        if tag != CREATE_AUTOMATION {
            return kResultFalse;
        }
        if let Ok(mut q) = self.shared.automate.lock() {
            if !q.contains(&self.param) {
                q.push(self.param);
            }
        }
        self.shared.attention.raise();
        os_host::wake();
        kResultOk
    }
}

impl IComponentHandler3 for Handler {
    /// A new menu (reference count 1, the plug-in releases it) with the host's entries for parameter `*param_id`: a
    /// separator and "Create automation clip" when the parameter can be automated, else none.
    unsafe fn create_context_menu(&self, _plug_view: *mut c_void, param_id: *const u32) -> *mut c_void {
        let shared = &self.shared;
        let param = (!param_id.is_null()).then(|| unsafe { *param_id });
        let automatable = param.filter(|id| shared.automatable.lock().is_ok_and(|a| a.contains(id)));
        let mut entries = Vec::new();
        if let Some(param) = automatable {
            let own = Com::new(HostTarget::allocate(shared.clone(), param));
            // SAFETY: our own object, as its one interface; the pointer takes a reference of its own.
            let target = unsafe { VstPtr::<dyn IContextMenuTarget>::shared(own.raw() as *mut *mut _) };
            let sep = Entry { name: [0; 128], tag: 0, flags: IS_SEPARATOR, target: target.clone(), host: true };
            let mut create = Entry { name: [0; 128], tag: CREATE_AUTOMATION, flags: 0, target, host: true };
            unsafe { write_str128(create.name.as_mut_ptr() as *mut u16, CREATE_AUTOMATION_LABEL) };
            entries.extend([sep, create]);
        }
        Box::into_raw(ContextMenu::allocate(shared.clone(), RefCell::new(entries))) as *mut c_void
    }
}

impl IContextMenu for ContextMenu {
    unsafe fn get_item_count(&self) -> i32 {
        self.entries.borrow().len() as i32
    }

    /// The target is handed out without a new reference.
    unsafe fn get_item(&self, index: i32, item: *mut IContextMenuItem, target: *mut *mut c_void) -> tresult {
        let entries = self.entries.borrow();
        let Some(e) = usize::try_from(index).ok().and_then(|i| entries.get(i)) else { return kInvalidArgument };
        unsafe {
            if !item.is_null() {
                (*item).name = e.name;
                (*item).tag = e.tag;
                (*item).flags = e.flags;
            }
            if !target.is_null() {
                *target = e.target_ptr();
            }
        }
        kResultOk
    }

    /// Takes a reference to the target, released with the menu or by `removeItem`.
    unsafe fn add_item(&self, item: *mut IContextMenuItem, target: *mut c_void) -> tresult {
        if item.is_null() {
            return kInvalidArgument;
        }
        let (name, tag, flags) = unsafe { ((*item).name, (*item).tag, (*item).flags) };
        let target = unsafe { VstPtr::<dyn IContextMenuTarget>::shared(target as *mut *mut _) };
        self.entries.borrow_mut().push(Entry { name, tag, flags, target, host: false });
        kResultOk
    }

    /// Removes the first entry with the item's tag and this target.
    unsafe fn remove_item(&self, item: *mut IContextMenuItem, target: *mut c_void) -> tresult {
        if item.is_null() {
            return kInvalidArgument;
        }
        let tag = unsafe { (*item).tag };
        let gone = {
            let mut entries = self.entries.borrow_mut();
            let Some(i) = entries.iter().position(|e| e.tag == tag && e.target_ptr() == target) else { return kResultFalse };
            entries.remove(i)
        };
        // Released outside the borrow: the target's release is the plug-in's code.
        drop(gone);
        kResultOk
    }

    /// Hands the menu to the app and returns at once: the plug-in's entries first, then the host's. A pick calls the
    /// entry's target later, on the UI thread; a newer popup replaces this one.
    unsafe fn popup(&self, x: i32, y: i32) -> tresult {
        let entries: Vec<Entry> = {
            let all = self.entries.borrow();
            all.iter().filter(|e| !e.host).chain(all.iter().filter(|e| e.host)).cloned().collect()
        };
        let popup = Popup { id: NEXT_MENU.fetch_add(1, Ordering::Relaxed), at: (x, y), entries, shown: false };
        let old = self.shared.menu.lock().ok().and_then(|mut m| m.replace(popup));
        drop(old);
        self.shared.attention.raise();
        os_host::wake();
        kResultOk
    }
}

/// The popup waiting for the app, as format-neutral rows; `scale` turns view coordinates into editor pixels.
pub fn take(shared: &Shared, scale: f32) -> Option<os_host::Menu> {
    let mut m = shared.menu.lock().ok()?;
    let p = m.as_mut().filter(|p| !p.shown)?;
    p.shown = true;
    let px = |v: i32| (v as f32 * scale).round() as i32;
    Some(os_host::Menu { id: p.id, at: (px(p.at.0), px(p.at.1)), items: rows(&p.entries) })
}

/// Menu `id` closed: a pick calls its entry's target (UI thread), then the host's references go. Another id (a menu
/// a newer one replaced) does nothing.
pub fn done(shared: &Shared, id: u64, pick: Option<usize>) {
    // Out of the lock first: the plug-in may open another menu from its target.
    let Some(p) = shared.menu.lock().ok().and_then(|mut m| m.take_if(|p| p.id == id)) else { return };
    if let Some(e) = pick.and_then(|i| p.entries.get(i)).filter(|e| e.pickable()) {
        if let Some(t) = &e.target {
            unsafe { t.execute_menu_item(e.tag) };
        }
    }
}

/// Drops a waiting or shown popup with the host's references to its targets (the editor closes, the instance goes).
pub fn drop_popup(shared: &Shared) {
    let p = shared.menu.lock().ok().and_then(|mut m| m.take());
    drop(p);
}

/// Entries (in the order shown) as rows: a group start opens a submenu titled by its name, a group end closes it (an
/// unclosed one closes at the end); an entry's pick is its position in `entries`.
fn rows(entries: &[Entry]) -> Vec<os_host::MenuItem> {
    use os_host::MenuItem;
    let mut open: Vec<(String, Vec<MenuItem>)> = vec![(String::new(), Vec::new())];
    fn close(open: &mut Vec<(String, Vec<MenuItem>)>) {
        if open.len() > 1 {
            let (label, items) = open.pop().unwrap();
            open.last_mut().unwrap().1.push(MenuItem::Sub { label, items: tidy(items) });
        }
    }
    for (i, e) in entries.iter().enumerate() {
        if e.flags & GROUP_START != 0 {
            open.push((from_utf16(&e.name), Vec::new()));
        } else if e.flags & GROUP_END != 0 {
            close(&mut open);
        } else {
            let row = if e.flags & IS_SEPARATOR != 0 {
                MenuItem::Separator
            } else {
                MenuItem::Entry { label: from_utf16(&e.name), pick: i, enabled: e.flags & IS_DISABLED == 0, checked: e.flags & IS_CHECKED != 0 }
            };
            open.last_mut().unwrap().1.push(row);
        }
    }
    while open.len() > 1 {
        close(&mut open);
    }
    tidy(open.pop().unwrap().1)
}

/// Separators at either end of a list, or next to another, are left out.
fn tidy(items: Vec<os_host::MenuItem>) -> Vec<os_host::MenuItem> {
    let mut out: Vec<os_host::MenuItem> = Vec::with_capacity(items.len());
    for it in items {
        let sep = it == os_host::MenuItem::Separator;
        if !(sep && out.last().is_none_or(|l| *l == os_host::MenuItem::Separator)) {
            out.push(it);
        }
    }
    if out.last() == Some(&os_host::MenuItem::Separator) {
        out.pop();
    }
    out
}
