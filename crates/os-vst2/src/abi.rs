// Copyright (C) 2026 ItsAlphaNeon (Lemon Studio's own parts). Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later AND BSD-3-Clause
//
// Lemon Studio's own parts of this file are NOT to be used as part of the ArtCraft suite, nor is any other part of
// Lemon Studio. The parts that come from others' work keep their authors' copyright and licence, named below.

//! The VST 2 binary interface: the structures, op-codes and flags Lemon Studio's host uses.
//!
//! Written from clean-room sources only, never from Steinberg's VST 2 SDK, whose files (`aeffect.h`, `aeffectx.h`) may not
//! be redistributed and must not be consulted for this crate:
//! - vst2sdk 0.4.0 by Xaymar (<https://github.com/Xaymar/vst2sdk>, BSD-3-Clause, notice below), reverse engineered from
//!   released plugins and hosts: the effect structure, effect op-codes and flags, categories, the event header, the
//!   editor rectangle, parameter properties and the host op-codes it documents.
//! - FST 0.177 by IOhannes m zmölnig (<https://git.iem.at/zmoelnig/FST>, GPL-3.0-or-later): the host op-codes vst2sdk
//!   leaves undocumented (time info, tempo, edit begin / end) and the time info structure.
//! - Where the two differ, the layout DPF ships in its VST 2 plugins (<https://github.com/DISTRHO/DPF>, ISC) decides:
//!   an event list holds its event pointers inline after a pointer-sized reserved field (as FST has it).
//!
//! Only 64-bit layouts are relied on (Lemon Studio is a 64-bit program); the calling convention is the platform's C one.
//
// vst2sdk notice (BSD-3-Clause), for the parts of this file derived from it:
//
// Copyright (c) 2020 Xaymar Dirks <info@xaymar.com> (previously known as Michael Fabian Dirks)
//
// Redistribution and use in source and binary forms, with or without modification, are permitted provided that the
// following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following
//    disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the
//    following disclaimer in the documentation and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
//    products derived from this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES,
// INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
// DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
// SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY,
// WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

#![allow(dead_code)]

use std::ffi::c_void;

/// `'VstP'`: the first field of every effect.
pub const MAGIC: i32 = fourcc(*b"VstP");

/// A four-character code as VST 2 numbers it (the first character in the high byte).
pub const fn fourcc(c: [u8; 4]) -> i32 {
    i32::from_be_bytes(c)
}

/// The host's side of every call a plugin makes (vst2sdk `vst_host_callback_t`).
pub type HostCallback = unsafe extern "C" fn(effect: *mut Effect, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize;
/// The plugin's op-code entry (vst2sdk `vst_effect_control_t`).
pub type Control = unsafe extern "C" fn(effect: *mut Effect, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize;
/// Processing: `inputs` / `outputs` are `num_inputs` / `num_outputs` channel pointers of `frames` samples each.
pub type Process = unsafe extern "C" fn(effect: *mut Effect, inputs: *const *const f32, outputs: *mut *mut f32, frames: i32);
pub type ProcessDouble = unsafe extern "C" fn(effect: *mut Effect, inputs: *const *const f64, outputs: *mut *mut f64, frames: i32);
pub type SetParameter = unsafe extern "C" fn(effect: *mut Effect, index: i32, value: f32);
pub type GetParameter = unsafe extern "C" fn(effect: *mut Effect, index: i32) -> f32;
/// The plugin file's entry point (`VSTPluginMain`): a new effect, or null.
pub type Main = unsafe extern "C" fn(callback: HostCallback) -> *mut Effect;

/// A plugin instance as the plugin hands it out (vst2sdk `vst_effect_t`).
#[repr(C)]
pub struct Effect {
    /// [`MAGIC`].
    pub magic: i32,
    pub control: Option<Control>,
    /// Accumulating processing (deprecated): adds to the outputs.
    pub process: Option<Process>,
    pub set_parameter: Option<SetParameter>,
    pub get_parameter: Option<GetParameter>,
    pub num_programs: i32,
    pub num_params: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    /// [`flags`].
    pub flags: i32,
    /// Zero when created; for the host (vst2sdk: "reserved for host?"). Lemon Studio keeps nothing here.
    pub host_reserved_1: *mut c_void,
    /// Zero when created; for the host. Lemon Studio keeps its instance's host side here ([`crate::host`]).
    pub host_reserved_2: *mut c_void,
    /// Latency in frames; may change when the plugin resumes.
    pub delay: i32,
    pub _unknown_02: i32,
    pub _unknown_03: i32,
    pub input_output_ratio: f32,
    /// The plugin's own pointer.
    pub effect_internal: *mut c_void,
    /// Not touched by Lemon Studio (some plugin frameworks use it too).
    pub host_internal: *mut c_void,
    pub unique_id: i32,
    pub version: i32,
    /// Replacing processing: overwrites the outputs.
    pub process_float: Option<Process>,
    pub process_double: Option<ProcessDouble>,
}

/// Effect flags (vst2sdk `VST_EFFECT_FLAG`).
pub mod flags {
    /// The plugin has its own editor.
    pub const EDITOR: i32 = 1 << 0;
    /// `process_float` is there (required in VST 2.4).
    pub const SUPPORTS_FLOAT: i32 = 1 << 4;
    /// The state is saved and restored as an opaque chunk.
    pub const CHUNKS: i32 = 1 << 5;
    /// An instrument (along with the instrument category).
    pub const INSTRUMENT: i32 = 1 << 8;
    pub const SILENT_TAIL: i32 = 1 << 9;
    pub const SUPPORTS_DOUBLE: i32 = 1 << 12;
}

/// Plugin categories (vst2sdk `VST_EFFECT_CATEGORY`).
pub mod category {
    pub const UNKNOWN: isize = 0;
    pub const EFFECT: isize = 1;
    pub const INSTRUMENT: isize = 2;
    pub const METERING: isize = 3;
    pub const MASTERING: isize = 4;
    pub const SPATIAL: isize = 5;
    pub const DELAY_OR_ECHO: isize = 6;
    pub const EXTERNAL: isize = 7;
    pub const RESTORATION: isize = 8;
    pub const OFFLINE: isize = 9;
    /// A shell: one file offering several plugins (vst2sdk "container").
    pub const CONTAINER: isize = 10;
    pub const WAVE_GENERATOR: isize = 11;
}

/// Op-codes the host sends the plugin (vst2sdk `VST_EFFECT_OPCODE`).
pub mod op {
    pub const OPEN: i32 = 0x00;
    pub const CLOSE: i32 = 0x01;
    /// `value`: the program.
    pub const SET_PROGRAM: i32 = 0x02;
    pub const GET_PROGRAM: i32 = 0x03;
    /// `ptr`: a text buffer.
    pub const GET_PROGRAM_NAME: i32 = 0x05;
    /// Parameter `index`'s unit label into `ptr`.
    pub const PARAM_LABEL: i32 = 0x06;
    /// Parameter `index`'s current value as text into `ptr`.
    pub const PARAM_DISPLAY: i32 = 0x07;
    pub const PARAM_NAME: i32 = 0x08;
    /// `opt`: the sample rate.
    pub const SET_SAMPLE_RATE: i32 = 0x0A;
    /// `value`: the largest block the host processes.
    pub const SET_BLOCK_SIZE: i32 = 0x0B;
    /// `value` 1: resume (ready to process), 0: suspend.
    pub const SUSPEND_RESUME: i32 = 0x0C;
    /// `ptr`: a `*mut *mut Rect` the plugin points at its editor's rectangle.
    pub const EDITOR_GET_RECT: i32 = 0x0D;
    /// `ptr`: the parent window (HWND, NSView, X11 window id).
    pub const EDITOR_OPEN: i32 = 0x0E;
    pub const EDITOR_CLOSE: i32 = 0x0F;
    /// Called regularly while the editor is open.
    pub const EDITOR_IDLE: i32 = 0x13;
    /// `index` 0: the whole bank, 1: the current program; `ptr`: a `*mut *mut c_void` the plugin points at the chunk;
    /// returns its size.
    pub const GET_CHUNK: i32 = 0x17;
    /// `index` as for `GET_CHUNK`, `value`: the size, `ptr`: the chunk.
    pub const SET_CHUNK: i32 = 0x18;
    /// `ptr`: an [`Events`] list for the next process call.
    pub const EVENTS: i32 = 0x19;
    pub const PARAM_AUTOMATABLE: i32 = 0x1A;
    /// Returns a [`category`].
    pub const CATEGORY: i32 = 0x23;
    pub const EFFECT_NAME: i32 = 0x2D;
    pub const VENDOR_NAME: i32 = 0x2F;
    pub const PRODUCT_NAME: i32 = 0x30;
    pub const VENDOR_VERSION: i32 = 0x31;
    /// Vendor extensions: `index` and `value` say which.
    pub const CUSTOM: i32 = 0x32;
    /// `ptr`: a feature name ([`crate::abi::supports`]); returns 1 yes, -1 no, 0 unknown.
    pub const SUPPORTS: i32 = 0x33;
    /// Returns 0 when unknown, 1 for no tail, else the tail in frames.
    pub const TAIL_SAMPLES: i32 = 0x34;
    /// Parameter `index`'s properties into `ptr` (a [`ParameterProperties`]); returns 1 when it has them.
    pub const PARAM_PROPERTIES: i32 = 0x38;
    pub const VST_VERSION: i32 = 0x3A;
    /// A shell's next plugin: its name into `ptr`, its id returned (0 after the last).
    pub const CONTAINER_NEXT_EFFECT_ID: i32 = 0x46;
    /// Processing starts (after resume) / stops (before suspend).
    pub const PROCESS_BEGIN: i32 = 0x47;
    pub const PROCESS_END: i32 = 0x48;
}

/// Op-codes a plugin sends the host. vst2sdk documents those without a note; FST the ones marked "(FST)".
pub mod host_op {
    /// The plugin's editor changed parameter `index` to `opt`.
    pub const AUTOMATE: i32 = 0x00;
    /// Returns the host's VST version (2400).
    pub const VERSION: i32 = 0x01;
    /// The shell plugin to create, while a shell's entry point runs.
    pub const CURRENT_ID: i32 = 0x02;
    pub const IDLE: i32 = 0x03;
    /// (FST)
    pub const WANT_MIDI: i32 = 0x06;
    /// (FST) Returns a pointer to the host's [`TimeInfo`]; `value`: the [`time`] fields the plugin wants.
    pub const GET_TIME: i32 = 0x07;
    /// (FST) `ptr`: events the plugin sends out.
    pub const PROCESS_EVENTS: i32 = 0x08;
    /// (FST) Returns the tempo times 10000.
    pub const TEMPO_AT: i32 = 0x0A;
    /// The plugin's inputs, outputs, parameter count or latency changed.
    pub const IO_CHANGED: i32 = 0x0D;
    /// (FST)
    pub const NEED_IDLE: i32 = 0x0E;
    /// The plugin asks for an editor of `index` x `value` pixels.
    pub const SIZE_WINDOW: i32 = 0x0F;
    pub const GET_SAMPLE_RATE: i32 = 0x10;
    pub const GET_BLOCK_SIZE: i32 = 0x11;
    pub const INPUT_LATENCY: i32 = 0x12;
    pub const OUTPUT_LATENCY: i32 = 0x13;
    /// (FST: the process level, 4 = offline rendering; vst2sdk reads the same op-code as the calling thread's role.)
    pub const PROCESS_LEVEL: i32 = 0x17;
    /// The host's vendor name into `ptr` (64 bytes).
    pub const VENDOR_NAME: i32 = 0x20;
    pub const PRODUCT_NAME: i32 = 0x21;
    pub const VENDOR_VERSION: i32 = 0x22;
    /// `ptr`: a feature name ([`crate::abi::host_supports`]); 1 when the host has it.
    pub const SUPPORTS: i32 = 0x25;
    /// 1 = English.
    pub const LANGUAGE: i32 = 0x26;
    /// The plugin's folder (a C string; not on macOS).
    pub const DIRECTORY: i32 = 0x29;
    /// Parameter names, programs or the display changed.
    pub const UPDATE_DISPLAY: i32 = 0x2A;
    /// A gesture on parameter `index` starts / ends.
    pub const BEGIN_EDIT: i32 = 0x2B;
    pub const END_EDIT: i32 = 0x2C;
}

/// Feature names the host asks a plugin about (`op::SUPPORTS`; vst2sdk `vst_effect_supports_t`).
pub mod supports {
    pub const RECEIVE_EVENTS: &std::ffi::CStr = c"receiveVstEvents";
    pub const RECEIVE_MIDI_EVENT: &std::ffi::CStr = c"receiveVstMidiEvent";
}

/// Feature names a plugin asks the host about (`host_op::SUPPORTS`; vst2sdk `vst_host_supports_t`) and Lemon Studio has.
pub const HOST_SUPPORTS: &[&[u8]] = &[b"sendVstEvents", b"sendVstMidiEvent", b"sendVstTimeInfo", b"sizeWindow", b"startStopProcess", b"shellCategory", b"acceptIOChanges"];

/// An editor's rectangle (vst2sdk `vst_rect_t`).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Rect {
    pub top: i16,
    pub left: i16,
    pub bottom: i16,
    pub right: i16,
}

/// Event types (vst2sdk `VST_EVENT_TYPE`).
pub const EVENT_MIDI: i32 = 1;

/// A MIDI event (vst2sdk `vst_event_midi_t`; FST and DPF agree on the header and the data's place).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct MidiEvent {
    /// [`EVENT_MIDI`].
    pub kind: i32,
    /// The bytes after `kind` and `size` (24).
    pub size: i32,
    /// Frames into the next process call.
    pub offset: i32,
    /// 1 = played live.
    pub flags: i32,
    pub note_length: i32,
    pub note_offset: i32,
    /// Up to three MIDI bytes; the fourth is 0.
    pub data: [u8; 4],
    /// Cents.
    pub tune: i8,
    pub velocity: i8,
    pub _pad: [u8; 2],
}

impl MidiEvent {
    pub const SIZE: i32 = (std::mem::size_of::<MidiEvent>() - 8) as i32;

    pub fn new(offset: u32, data: [u8; 3]) -> MidiEvent {
        MidiEvent { kind: EVENT_MIDI, size: Self::SIZE, offset: offset as i32, data: [data[0], data[1], data[2], 0], ..MidiEvent::default() }
    }
}

/// An event list for `op::EVENTS` (FST `t_fstEvents`, DPF `HostVstEvents`): the count, a pointer-sized reserved field,
/// then the event pointers inline.
#[repr(C)]
pub struct Events<const N: usize> {
    pub count: i32,
    pub reserved: isize,
    pub events: [*mut MidiEvent; N],
}

/// The host's transport, which plugins read through `host_op::GET_TIME` (FST `t_fstTimeInfo`; DPF reads the same
/// places). The four 32-bit fields marked unknown are read differently by the two (SMPTE offset and rate in DPF); Lemon
/// Studio leaves them 0 and never flags SMPTE time valid.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct TimeInfo {
    /// Frames from the song start.
    pub sample_pos: f64,
    pub sample_rate: f64,
    pub nanoseconds: f64,
    /// Quarter notes from the song start.
    pub ppq_pos: f64,
    pub tempo: f64,
    /// The start of the current bar, in quarter notes.
    pub bar_start_pos: f64,
    pub cycle_start_pos: f64,
    pub cycle_end_pos: f64,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
    pub _unknown: [i32; 2],
    pub samples_to_next_clock: i32,
    /// [`time`] flags.
    pub flags: i32,
    pub _unknown_2: [i32; 2],
}

/// `TimeInfo::flags` (FST).
pub mod time {
    pub const TRANSPORT_CHANGED: i32 = 1 << 0;
    pub const PLAYING: i32 = 1 << 1;
    pub const CYCLE_ACTIVE: i32 = 1 << 2;
    pub const RECORDING: i32 = 1 << 3;
    pub const NANOS_VALID: i32 = 1 << 8;
    pub const PPQ_POS_VALID: i32 = 1 << 9;
    pub const TEMPO_VALID: i32 = 1 << 10;
    pub const BARS_VALID: i32 = 1 << 11;
    pub const CYCLE_POS_VALID: i32 = 1 << 12;
    pub const TIME_SIG_VALID: i32 = 1 << 13;
}

/// A parameter's properties (vst2sdk `vst_parameter_properties_t`); few plugins give them.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ParameterProperties {
    pub step_f32: f32,
    pub step_small_f32: f32,
    pub step_large_f32: f32,
    pub name: [u8; 64],
    /// [`param_flags`].
    pub flags: u32,
    pub min_value_i32: i32,
    pub max_value_i32: i32,
    pub step_i32: i32,
    pub label: [u8; 8],
    pub index: u16,
    pub category: u16,
    pub num_parameters_in_category: u16,
    pub _unknown_00: u16,
    pub category_label: [u8; 24],
    pub _reserved: [u8; 16],
}

/// `ParameterProperties::flags` (vst2sdk `VST_PARAMETER_FLAG`).
pub mod param_flags {
    pub const SWITCH: u32 = 1 << 0;
    pub const INTEGER_LIMITS: u32 = 1 << 1;
    pub const CATEGORY: u32 = 1 << 5;
}

#[cfg(all(test, target_pointer_width = "64"))]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The 64-bit layouts both clean-room sources give.
    #[test]
    fn layouts() {
        assert_eq!(offset_of!(Effect, num_programs), 40);
        assert_eq!(offset_of!(Effect, flags), 56);
        assert_eq!(offset_of!(Effect, host_reserved_1), 64);
        assert_eq!(offset_of!(Effect, delay), 80);
        assert_eq!(offset_of!(Effect, input_output_ratio), 92);
        assert_eq!(offset_of!(Effect, host_internal), 104);
        assert_eq!(offset_of!(Effect, unique_id), 112);
        assert_eq!(offset_of!(Effect, process_float), 120);
        assert_eq!(size_of::<Effect>(), 136);
        assert_eq!(size_of::<MidiEvent>(), 32);
        assert_eq!(offset_of!(MidiEvent, data), 24);
        assert_eq!(MidiEvent::SIZE, 24);
        assert_eq!(offset_of!(Events<4>, events), 16);
        assert_eq!(offset_of!(TimeInfo, time_sig_numerator), 64);
        assert_eq!(offset_of!(TimeInfo, samples_to_next_clock), 80);
        assert_eq!(offset_of!(TimeInfo, flags), 84);
        assert_eq!(size_of::<TimeInfo>(), 96);
        assert_eq!(size_of::<ParameterProperties>(), 148);
        assert_eq!(MAGIC, 0x5673_7450);
    }
}
