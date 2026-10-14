// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The host side of CLAP's context-menu extension: when a plugin builds the right-click menu of one of its parameters it
//! asks the host for entries, and Lemon Studio adds "Create automation clip". A pick comes back through `perform` and is
//! handed to the app by `take_automation_requests` (parameter ids). The host never pops up menus for the plugin.
//!
// spec: plugins/plugin-parameters.md §6 (plugin knob menus); CLAP ext/context-menu.h

use std::ffi::{c_void, CStr};
use std::sync::atomic::Ordering;

use clap_sys::ext::context_menu::{
    clap_context_menu_builder, clap_context_menu_entry, clap_context_menu_target, clap_host_context_menu, CLAP_CONTEXT_MENU_ITEM_ENTRY,
    CLAP_CONTEXT_MENU_ITEM_SEPARATOR, CLAP_CONTEXT_MENU_TARGET_KIND_PARAM,
};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;

use crate::{HostData, Plugin};

/// Action id of "Create automation clip".
const CREATE_AUTOMATION: clap_id = 1;
static CREATE_AUTOMATION_LABEL: &CStr = c"Create automation clip";

pub(crate) static HOST_CONTEXT_MENU: clap_host_context_menu =
    clap_host_context_menu { populate: Some(populate), perform: Some(perform), can_popup: Some(can_popup), popup: Some(popup) };

unsafe fn param_target(target: *const clap_context_menu_target) -> Option<clap_id> {
    if target.is_null() {
        return None;
    }
    let t = unsafe { *target };
    (t.kind == CLAP_CONTEXT_MENU_TARGET_KIND_PARAM).then_some(t.id)
}

unsafe extern "C" fn populate(_host: *const clap_host, target: *const clap_context_menu_target, builder: *const clap_context_menu_builder) -> bool {
    if builder.is_null() || unsafe { param_target(target) }.is_none() {
        return true;
    }
    let Some(add) = (unsafe { (*builder).add_item }) else { return false };
    let entry = clap_context_menu_entry { label: CREATE_AUTOMATION_LABEL.as_ptr(), is_enabled: true, action_id: CREATE_AUTOMATION };
    unsafe {
        add(builder, CLAP_CONTEXT_MENU_ITEM_SEPARATOR, std::ptr::null());
        add(builder, CLAP_CONTEXT_MENU_ITEM_ENTRY, &entry as *const clap_context_menu_entry as *const c_void)
    }
}

unsafe extern "C" fn perform(host: *const clap_host, target: *const clap_context_menu_target, action_id: clap_id) -> bool {
    let Some(id) = (unsafe { param_target(target) }).filter(|_| action_id == CREATE_AUTOMATION && !host.is_null()) else { return false };
    let data = unsafe { &*((*host).host_data as *const HostData) };
    if let Ok(mut q) = data.automate.lock() {
        q.push(id);
    }
    data.callback.store(true, Ordering::Release);
    unsafe { crate::attend(host) };
    true
}

unsafe extern "C" fn can_popup(_host: *const clap_host) -> bool {
    false
}

unsafe extern "C" fn popup(_host: *const clap_host, _target: *const clap_context_menu_target, _screen: i32, _x: i32, _y: i32) -> bool {
    false
}

impl Plugin {
    /// Parameter ids the plugin's menus asked to automate since the last call (main thread).
    pub(crate) fn automation_requests(&mut self) -> Vec<u32> {
        self.host_data.automate.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use clap_sys::ext::context_menu::CLAP_EXT_CONTEXT_MENU; // This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!

    /// Collects the labels a builder was given.
    unsafe extern "C" fn add_item(b: *const clap_context_menu_builder, kind: u32, data: *const c_void) -> bool {
        let out = unsafe { &mut *((*b).ctx as *mut Vec<String>) };
        out.push(match kind {
            CLAP_CONTEXT_MENU_ITEM_ENTRY => unsafe { CStr::from_ptr((*(data as *const clap_context_menu_entry)).label) }.to_string_lossy().into_owned(),
            _ => "-".into(),
        });
        true
    }

    /// A plugin's parameter menu gets "Create automation clip" from the host; picking it reaches the app as that
    /// parameter's id. Global (non-parameter) menus get nothing.
    #[test]
    fn parameter_menus_offer_create_automation_clip() {
        let entry = &lemon_parametric_eq_2::LINKED_CLAP_ENTRY as *const _ as *const c_void;
        let lib = unsafe { crate::Library::from_entry(entry, std::path::Path::new("Lemon Parametric EQ 2.clap")) }.expect("linked EQ");
        let mut p = Plugin::new(&lib, &lib.plugins[0].id).unwrap();
        let host: *const clap_host = &*p._host;
        let ext = unsafe { (*host).get_extension.unwrap()(host, CLAP_EXT_CONTEXT_MENU.as_ptr()) } as *const clap_host_context_menu;
        assert!(!ext.is_null());
        let mut labels: Vec<String> = Vec::new();
        let builder = clap_context_menu_builder { ctx: &mut labels as *mut Vec<String> as *mut c_void, add_item: Some(add_item), supports: None };
        let param = clap_context_menu_target { kind: CLAP_CONTEXT_MENU_TARGET_KIND_PARAM, id: 77 };
        let global = clap_context_menu_target { kind: 0, id: 0 };
        let attention = os_host::Plugin::attention(&p).expect("a CLAP instance has a count");
        let before = attention.count();
        unsafe {
            assert!((*ext).populate.unwrap()(host, &global, &builder));
            assert!(labels.is_empty());
            assert!((*ext).populate.unwrap()(host, &param, &builder));
            assert_eq!(labels, ["-", "Create automation clip"]);
            assert!(!(*ext).perform.unwrap()(host, &param, 99));
            assert!((*ext).perform.unwrap()(host, &param, CREATE_AUTOMATION));
            assert!(!(*ext).can_popup.unwrap()(host));
        }
        assert!(attention.count() > before, "the pick reaches the UI without it polling");
        assert_eq!(os_host::Plugin::take_automation_requests(&mut p), vec![77]);
        assert!(os_host::Plugin::take_automation_requests(&mut p).is_empty());
    }
}
