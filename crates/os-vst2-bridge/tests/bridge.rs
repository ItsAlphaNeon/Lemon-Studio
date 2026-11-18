// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The bridge end to end: `os_vst2` hosting the helper's built-in test plugin (`:test:`) through the helper this
//! package builds, as it hosts any plugin. `OS_VST2_BRIDGE32=<helper built for i686> OS_VST2_PLUGIN32=<a 32-bit
//! plugin file>` adds a real 32-bit plugin (`a_32_bit_plugin_runs`, ignored by default).
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use os_engine::Instrument as _;
use os_host::{Library as _, ParamEvent, Plugin as _};
use os_vst2::{id_text, Module, Plugin};

fn helper() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lemonstudio-bridge32"))
}

fn module() -> Arc<Module> {
    Module::load_bridged(Path::new(":test:"), &helper()).expect("the test plugin through the bridge")
}

fn plugin(m: &Arc<Module>) -> Plugin {
    Plugin::new(m, &id_text(os_vst2_bridge::testplug::UNIQUE_ID)).expect("create")
}

#[test]
fn the_bridged_plugin_is_listed_as_itself() {
    let m = module();
    let d = &m.plugins()[0];
    assert_eq!((d.name.as_str(), d.vendor.as_str(), d.version.as_str()), ("Bridge Test", "Lemon Studio Tests", "1.2.3"));
    assert_eq!(d.kind, os_host::Kind::Instrument);
    assert_eq!(d.format, os_host::Format::Vst2);
    // Its instances are made through the module like any plugin's.
    let p = m.clone().create(&d.id).expect("create");
    drop(p);
}

#[test]
fn notes_parameters_and_the_transport_cross_the_bridge() {
    let m = module();
    let mut p = plugin(&m);
    assert!(p.activate(48000));
    assert_eq!(p.param_count(), 1);
    assert_eq!(p.param_value(0), Some(0.5));
    let (mut l, mut r) = (vec![0.0f32; 256], vec![0.0f32; 256]);
    p.process(48000, &mut l, &mut r);
    // The transport the host gives (120 BPM) reaches the plugin: its output carries tempo / 1000.
    assert!((l[100] - 0.12).abs() < 1e-6, "{}", l[100]);
    p.note_on(0, 60, 1.0);
    p.process(48000, &mut l, &mut r);
    assert!((l[100] - 0.37).abs() < 1e-6 && (r[100] - 0.37).abs() < 1e-6, "{} {}", l[100], r[100]);
    p.note_off(0, 60);
    p.process(48000, &mut l, &mut r);
    assert!((l[100] - 0.12).abs() < 1e-6, "the note ended: {}", l[100]);
    assert!(p.set_param(0, 0.25));
    assert_eq!(p.param_value(0), Some(0.25));
}

#[test]
fn the_state_crosses_the_bridge_both_ways() {
    let m = module();
    let mut a = plugin(&m);
    a.set_param(0, 0.3);
    let st = a.save_state().expect("state");
    let mut b = plugin(&m);
    assert_eq!(b.param_value(0), Some(0.5));
    assert!(b.load_state(&st));
    assert!((b.param_value(0).unwrap() - 0.3).abs() < 1e-6);
}

/// The editor opens in a window of this process from the helper's, and an edit the plugin makes on a thread of its
/// own (not inside any host call) reaches the host.
#[test]
fn the_editor_opens_in_the_hosts_window() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, GetWindow, GW_CHILD, WS_POPUP};
    let m = module();
    let mut p = plugin(&m);
    p.activate(48000);
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let parent = unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 300, 200, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
    assert!(!parent.is_null());
    let size = p.open_editor(parent.cast(), 1.0).expect("editor");
    assert_eq!(size, (200, 100));
    assert!(!unsafe { GetWindow(parent, GW_CHILD) }.is_null(), "the plugin's window is the host window's child");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while Instant::now() < deadline && events.is_empty() {
        os_vst2_bridge::ipc::pump_messages(os_vst2_bridge::ipc::Pump::All);
        events = p.take_param_events();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(events.iter().any(|e| matches!(e, ParamEvent::Value(0, v) if (*v - 0.75).abs() < 1e-6)), "{events:?}");
    assert_eq!(p.param_value(0), Some(0.75), "the plugin's own edit is the value read");
    p.close_editor();
    unsafe { DestroyWindow(parent) };
}

/// Reading every parameter of a plugin again and again costs one trip to the helper per change, not one per read:
/// the host keeps the values (spec: plugins/vst2-bridge.md §4). Edits read back at once.
#[test]
fn parameter_reads_are_answered_from_the_hosts_copy() {
    let m = module();
    let mut p = plugin(&m);
    p.activate(48000);
    assert_eq!(p.param_value(0), Some(0.5));
    let t = Instant::now();
    for _ in 0..10_000 {
        assert_eq!(p.param_value(0), Some(0.5));
    }
    // A trip each would take most of a second; from the copy it takes well under 50 ms.
    assert!(t.elapsed() < Duration::from_millis(50), "{:?}", t.elapsed());
    for v in [0.1, 0.9, 0.4] {
        assert!(p.set_param(0, v));
        assert!((p.param_value(0).unwrap() - v).abs() < 1e-6);
    }
    // A new state is read back from the plugin.
    let q = plugin(&m);
    let st = q.save_state().expect("state");
    assert!(p.load_state(&st));
    assert_eq!(p.param_value(0), Some(0.5));
}

#[test]
fn a_helper_that_cannot_load_its_plugin_says_why() {
    let e = Module::load_bridged(Path::new("C:/nowhere/None.dll"), &helper()).err().expect("an error");
    assert!(e.contains("None.dll"), "{e}");
}

/// A real 32-bit plugin through the 32-bit helper: listed, activated, a note processed (its output finite).
#[test]
#[ignore]
fn a_32_bit_plugin_runs() {
    let (Some(helper), Some(file)) = (std::env::var_os("OS_VST2_BRIDGE32"), std::env::var_os("OS_VST2_PLUGIN32")) else { return };
    let m = Module::load_bridged(Path::new(&file), Path::new(&helper)).expect("load");
    let d = m.plugins()[0].clone();
    eprintln!("{} by {} ({:?}, version {})", d.name, d.vendor, d.kind, d.version);
    let mut p = Plugin::new(&m, &d.id).expect("create");
    assert!(p.activate(44100));
    let (mut l, mut r) = (vec![0.0f32; 4410], vec![0.0f32; 4410]);
    p.note_on(0, 60, 0.8);
    let mut peak = 0.0f32;
    for _ in 0..10 {
        p.process(44100, &mut l, &mut r);
        peak = l.iter().chain(&r).fold(peak, |a, v| a.max(v.abs()));
        assert!(l.iter().chain(&r).all(|v| v.is_finite()));
    }
    eprintln!("peak {peak}, {} parameters, state {} bytes", p.param_count(), p.save_state().map_or(0, |s| s.len()));
    assert!(peak > 0.0, "the note sounded");
}

