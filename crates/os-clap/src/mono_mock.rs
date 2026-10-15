// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! A test plugin built into the test binary: one mono input port and one mono output port, as DPF's Kars has its
//! output. Its output is half its input plus 0.25 (so an instrument call is heard too), and it records the channel
//! counts of each process call.

use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, CLAP_PORT_MONO};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::CLAP_INVALID_ID;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::version::CLAP_VERSION;

/// The channel counts of the last process call: input (0 without an input buffer), output.
pub static SEEN_IN: AtomicU32 = AtomicU32::new(u32::MAX);
pub static SEEN_OUT: AtomicU32 = AtomicU32::new(u32::MAX);

pub const ID: &CStr = c"org.lemonstudio.test.mono";

struct Shared<T>(T);
// SAFETY: read-only tables of static strings.
unsafe impl<T> Sync for Shared<T> {}

static FEATURES: Shared<[*const c_char; 2]> = Shared([c"audio-effect".as_ptr(), ptr::null()]);

static DESC: Shared<clap_plugin_descriptor> = Shared(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: ID.as_ptr(),
    name: c"Mono Test".as_ptr(),
    vendor: c"Lemon Studio".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"".as_ptr(),
    features: FEATURES.0.as_ptr(),
});

unsafe extern "C" fn port_count(_: *const clap_plugin, _is_input: bool) -> u32 {
    1
}

unsafe extern "C" fn port_get(_: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool {
    if index != 0 {
        return false;
    }
    let i = unsafe { &mut *info };
    i.id = u32::from(is_input);
    i.flags = CLAP_AUDIO_PORT_IS_MAIN;
    i.channel_count = 1;
    i.port_type = CLAP_PORT_MONO.as_ptr();
    i.in_place_pair = CLAP_INVALID_ID;
    true
}

static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports { count: Some(port_count), get: Some(port_get) };

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

unsafe extern "C" fn process(_: *const clap_plugin, p: *const clap_process) -> clap_process_status {
    let p = unsafe { &*p };
    let n = p.frames_count as usize;
    let input = (p.audio_inputs_count > 0).then(|| unsafe { &*p.audio_inputs });
    let output = unsafe { &*p.audio_outputs };
    SEEN_IN.store(input.map_or(0, |b| b.channel_count), Ordering::Relaxed);
    SEEN_OUT.store(output.channel_count, Ordering::Relaxed);
    // A mono plugin reads and writes its one channel only.
    let out = unsafe { std::slice::from_raw_parts_mut(*output.data32, n) };
    match input {
        Some(b) => {
            let inp = unsafe { std::slice::from_raw_parts(*b.data32, n) };
            out.iter_mut().zip(inp).for_each(|(o, i)| *o = i * 0.5 + 0.25);
        }
        None => out.fill(0.25),
    }
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn get_extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) } == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const clap_plugin_audio_ports as *const c_void
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

unsafe extern "C" fn create(_: *const clap_plugin_factory, _host: *const clap_host, _id: *const c_char) -> *const clap_plugin {
    Box::into_raw(Box::new(clap_plugin {
        desc: &DESC.0,
        plugin_data: ptr::null_mut(),
        init: Some(yes),
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

unsafe extern "C" fn init(_: *const c_char) -> bool {
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

pub static ENTRY: clap_plugin_entry = clap_plugin_entry { clap_version: CLAP_VERSION, init: Some(init), deinit: Some(deinit), get_factory: Some(get_factory) };
