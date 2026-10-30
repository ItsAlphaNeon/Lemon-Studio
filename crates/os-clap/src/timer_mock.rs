// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A test plugin built into the test binary (Linux): an effect that registers a 1 ms timer with the host when it is
//! initialised, as an editor does, and counts the timer's calls. While [`FAULT`] is set, its timer reads a bad
//! address instead, as a crashing editor does.

use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::ext::timer_support::{clap_host_timer_support, clap_plugin_timer_support, CLAP_EXT_TIMER_SUPPORT};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::version::CLAP_VERSION;

/// The timer reads a bad address while this is set.
pub static FAULT: AtomicBool = AtomicBool::new(false);
/// How many times the timer ran without faulting.
pub static TICKS: AtomicU32 = AtomicU32::new(0);

pub const ID: &CStr = c"org.lemonstudio.test.timer";

struct Shared<T>(T);
// SAFETY: read-only tables of static strings.
unsafe impl<T> Sync for Shared<T> {}

static FEATURES: Shared<[*const c_char; 2]> = Shared([c"audio-effect".as_ptr(), ptr::null()]);

static DESC: Shared<clap_plugin_descriptor> = Shared(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: ID.as_ptr(),
    name: c"Timer Test".as_ptr(),
    vendor: c"Lemon Studio".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"".as_ptr(),
    features: FEATURES.0.as_ptr(),
});

unsafe extern "C" fn on_timer(_: *const clap_plugin, _: clap_id) {
    if FAULT.load(Ordering::Relaxed) {
        // SAFETY: deliberately not: the read faults and the host's guard catches it.
        unsafe { std::ptr::read_volatile(8 as *const usize) };
    }
    TICKS.fetch_add(1, Ordering::Relaxed);
}

static TIMER: clap_plugin_timer_support = clap_plugin_timer_support { on_timer: Some(on_timer) };

/// Registers the timer through the host the plugin was created with (kept in `plugin_data`).
unsafe extern "C" fn init(p: *const clap_plugin) -> bool {
    unsafe {
        let host = (*p).plugin_data as *const clap_host;
        let Some(get) = (*host).get_extension else { return false };
        let ext = get(host, CLAP_EXT_TIMER_SUPPORT.as_ptr()) as *const clap_host_timer_support;
        let Some(register) = ext.as_ref().and_then(|e| e.register_timer) else { return false };
        let mut id = 0;
        register(host, 1, &mut id)
    }
}

unsafe extern "C" fn yes(_: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn nothing(_: *const clap_plugin) {}

unsafe extern "C" fn destroy(p: *const clap_plugin) {
    drop(unsafe { Box::from_raw(p as *mut clap_plugin) });
}

unsafe extern "C" fn activate(_: *const clap_plugin, _rate: f64, _min: u32, _max: u32) -> bool {
    true
}

unsafe extern "C" fn process(_: *const clap_plugin, _: *const clap_process) -> clap_process_status {
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn get_extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_EXT_TIMER_SUPPORT {
        &TIMER as *const clap_plugin_timer_support as *const c_void
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn count(_: *const clap_plugin_factory) -> u32 {
    1
}

unsafe extern "C" fn descriptor(_: *const clap_plugin_factory, _: u32) -> *const clap_plugin_descriptor {
    &DESC.0
}

unsafe extern "C" fn create(_: *const clap_plugin_factory, host: *const clap_host, _id: *const c_char) -> *const clap_plugin {
    Box::into_raw(Box::new(clap_plugin {
        desc: &DESC.0,
        plugin_data: host as *mut c_void,
        init: Some(init),
        destroy: Some(destroy),
        activate: Some(activate),
        deactivate: Some(nothing),
        start_processing: Some(yes),
        stop_processing: Some(nothing),
        reset: Some(nothing),
        process: Some(process),
        get_extension: Some(get_extension),
        on_main_thread: Some(nothing),
    }))
}

static FACTORY: clap_plugin_factory =
    clap_plugin_factory { get_plugin_count: Some(count), get_plugin_descriptor: Some(descriptor), create_plugin: Some(create) };

unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}

unsafe extern "C" fn deinit() {}

unsafe extern "C" fn get_factory(id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const clap_plugin_factory as *const c_void
    } else {
        ptr::null()
    }
}

pub static ENTRY: clap_plugin_entry =
    clap_plugin_entry { clap_version: CLAP_VERSION, init: Some(entry_init), deinit: Some(deinit), get_factory: Some(get_factory) };
