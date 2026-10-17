// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A CLAP entry table without exported symbols, for plugins compiled into Lemon Studio itself.
//!
//! nih-plug's `nih_export_clap!` / `nih_export_vst3!` define the C entry points (`clap_entry`, `GetPluginFactory`,
//! `InitDll`, ...). Two plugins linked into one program would define them twice, so a plugin crate built with its
//! `linked` feature skips those exports and offers [`linked_clap_entry!`]'s table instead; the host passes its
//! address to `os_clap::Library::from_entry`. The table works like the exported one (same descriptors, same
//! wrapper), only without the global symbol.
//!
// spec: plugins/plugin-formats.md (built-in plugins)

/// Defines `pub static LINKED_CLAP_ENTRY: clap_plugin_entry` for one plugin type.
#[macro_export]
macro_rules! linked_clap_entry {
    ($plugin_ty:ty) => {
        #[doc(hidden)]
        mod linked_clap {
            use ::nih_plug::wrapper::clap::{
                clap_host, clap_plugin, clap_plugin_descriptor, clap_plugin_factory, PluginDescriptor, Wrapper, CLAP_PLUGIN_FACTORY_ID,
            };
            use ::std::ffi::{c_void, CStr};
            use ::std::os::raw::c_char;
            use ::std::sync::{Arc, OnceLock};

            use super::*;

            const FACTORY: clap_plugin_factory = clap_plugin_factory {
                get_plugin_count: Some(get_plugin_count),
                get_plugin_descriptor: Some(get_plugin_descriptor),
                create_plugin: Some(create_plugin),
            };

            static DESCRIPTOR: OnceLock<PluginDescriptor> = OnceLock::new();

            fn descriptor() -> &'static PluginDescriptor {
                DESCRIPTOR.get_or_init(PluginDescriptor::for_plugin::<$plugin_ty>)
            }

            unsafe extern "C" fn get_plugin_count(_factory: *const clap_plugin_factory) -> u32 {
                1
            }

            unsafe extern "C" fn get_plugin_descriptor(_factory: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor {
                if index == 0 { descriptor().clap_plugin_descriptor() } else { ::std::ptr::null() }
            }

            unsafe extern "C" fn create_plugin(_factory: *const clap_plugin_factory, host: *const clap_host, plugin_id: *const c_char) -> *const clap_plugin {
                // SAFETY: the host passes a valid C string (CLAP factory contract).
                if plugin_id.is_null() || unsafe { CStr::from_ptr(plugin_id) } != descriptor().clap_id() {
                    return ::std::ptr::null();
                }
                // The wrapper's `destroy` takes this reference back (as in nih-plug's own factory).
                // SAFETY: `Wrapper::new` returns a live Arc; its plugin struct outlives the returned pointer.
                unsafe { (*Arc::into_raw(Wrapper::<$plugin_ty>::new(host))).clap_plugin.as_ptr() }
            }

            pub extern "C" fn init(_plugin_path: *const c_char) -> bool {
                true
            }

            pub extern "C" fn deinit() {}

            pub extern "C" fn get_factory(factory_id: *const c_char) -> *const c_void {
                // SAFETY: the host passes a valid C string (CLAP entry contract).
                if !factory_id.is_null() && unsafe { CStr::from_ptr(factory_id) } == CLAP_PLUGIN_FACTORY_ID {
                    &FACTORY as *const _ as *const c_void
                } else {
                    ::std::ptr::null()
                }
            }
        }

        /// The plugin's CLAP entry table for hosting it inside the same program (no exported symbol).
        pub static LINKED_CLAP_ENTRY: ::nih_plug::wrapper::clap::clap_plugin_entry = ::nih_plug::wrapper::clap::clap_plugin_entry {
            clap_version: ::nih_plug::wrapper::clap::CLAP_VERSION,
            init: Some(self::linked_clap::init),
            deinit: Some(self::linked_clap::deinit),
            get_factory: Some(self::linked_clap::get_factory),
        };
    };
}
