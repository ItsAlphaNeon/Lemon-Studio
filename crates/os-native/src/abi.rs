// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Raw object layouts and method tables of the FL native plugin interface.
//!
// spec: plugins/fruity-abi.md

use std::ffi::{c_char, c_void};

pub type IntPtr = isize;

/// `PluginInfo` (4-byte packing).
#[repr(C, packed(4))]
pub struct PluginInfo {
    pub sdk_version: i32,
    pub long_name: *const c_char,
    pub short_name: *const c_char,
    pub flags: i32,
    pub num_params: i32,
    pub def_poly: i32,
    pub num_out_ctrls: i32,
    pub num_out_voices: i32,
    pub reserved: [i32; 30],
}

pub mod flags {
    pub const GENERATOR: i32 = 1;
    pub const USE_SAMPLER: i32 = 1 << 2;
    pub const GET_NOTE_INPUT: i32 = 1 << 4;
    pub const WANT_NEW_TICK: i32 = 1 << 5;
    pub const NO_PROCESS: i32 = 1 << 6;
    pub const NO_WINDOW: i32 = 1 << 10;
    pub const INTERFACELESS: i32 = 1 << 11;
    pub const DEMO_VERSION: i32 = 1 << 15;
    pub const NEW_VOICE_PARAMS: i32 = 1 << 21;
}

pub mod dispatch {
    pub const SHOW_EDITOR: IntPtr = 0;
    pub const PROCESS_MODE: IntPtr = 1;
    pub const FLUSH: IntPtr = 2;
    pub const SET_BLOCK_SIZE: IntPtr = 3;
    pub const SET_SAMPLE_RATE: IntPtr = 4;
    pub const SET_ENABLED: IntPtr = 11;
    pub const SET_PLAYING: IntPtr = 12;
    pub const SET_SAMPLES_PER_TICK: IntPtr = 20;
    pub const GET_PARAM_INFO: IntPtr = 26;
    use super::IntPtr;
}

pub mod event {
    pub const TEMPO: i32 = 0;
    pub const MAX_POLY: i32 = 1;
}

pub mod name {
    pub const PARAM: i32 = 0;
    pub const PARAM_VALUE: i32 = 1;
}

/// ProcessParam flags (spec: "ProcessParam flags").
pub mod rec {
    pub const UPDATE_VALUE: i32 = 1;
    pub const GET_VALUE: i32 = 2;
    pub const UPDATE_CONTROL: i32 = 16;
    pub const FROM_MIDI: i32 = 32;
    /// The full scale of a value "from MIDI". The spec's SDK text gives 0..65536, but every installed plugin tried
    /// under this host (eight effects) maps 0..2^30 linearly onto its control's range and takes 65536 as nearly its
    /// minimum, so OpenStudio uses 2^30 (the scale of the value an automation clip hands on).
    pub const MIDI_FULL: f64 = 1_073_741_824.0;
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct LevelParams {
    pub pan: f32,
    pub vol: f32,
    pub pitch: f32,
    pub fcut: f32,
    pub fres: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct VoiceParams {
    pub init: LevelParams,
    pub fin: LevelParams,
}

/// The plugin object: method-table pointer followed by the documented fields.
#[repr(C)]
pub struct PluginObject { // This code is a part of L e m o n S t u d i o.
    pub vtable: *const PluginVtbl,
    pub host_tag: IntPtr,
    pub info: *const PluginInfo,
    pub editor_handle: *mut c_void,
    pub mono_render: i32,
    pub reserved: [i32; 32],
}

pub type This = *mut PluginObject;

/// Plugin method table, in documented order.
#[repr(C)]
pub struct PluginVtbl {
    pub destroy_object: unsafe extern "system" fn(This),
    pub dispatcher: unsafe extern "system" fn(This, IntPtr, IntPtr, IntPtr) -> IntPtr,
    pub idle: unsafe extern "system" fn(This),
    pub save_restore_state: unsafe extern "system" fn(This, *mut c_void, i32),
    pub get_name: unsafe extern "system" fn(This, i32, i32, i32, *mut c_char),
    pub process_event: unsafe extern "system" fn(This, i32, i32, i32) -> i32,
    pub process_param: unsafe extern "system" fn(This, i32, i32, i32) -> i32,
    pub eff_render: unsafe extern "system" fn(This, *const f32, *mut f32, i32),
    pub gen_render: unsafe extern "system" fn(This, *mut f32, *mut i32),
    pub trigger_voice: unsafe extern "system" fn(This, *const VoiceParams, IntPtr) -> IntPtr,
    pub voice_release: unsafe extern "system" fn(This, IntPtr),
    pub voice_kill: unsafe extern "system" fn(This, IntPtr),
    pub voice_process_event: unsafe extern "system" fn(This, IntPtr, i32, i32, i32) -> i32,
    pub voice_render: unsafe extern "system" fn(This, IntPtr, *mut f32, *mut i32) -> i32,
    pub new_tick: unsafe extern "system" fn(This),
    pub midi_tick: unsafe extern "system" fn(This),
    pub midi_in: unsafe extern "system" fn(This, *mut i32),
    pub msg_in: unsafe extern "system" fn(This, IntPtr),
    pub output_voice_process_event: unsafe extern "system" fn(This, IntPtr, i32, i32, i32) -> i32,
    pub output_voice_kill: unsafe extern "system" fn(This, IntPtr),
}

pub type CreatePlugInstance = unsafe extern "system" fn(*mut c_void, IntPtr) -> *mut PluginObject;

/// Number of documented host methods.
pub const HOST_METHODS: usize = 42;
/// Extra method slots provided after the documented ones, to catch newer, undocumented calls.
pub const HOST_EXTRA_SLOTS: usize = 64;
