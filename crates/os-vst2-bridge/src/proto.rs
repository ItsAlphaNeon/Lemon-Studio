// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The bridge's messages. Both processes read the same bytes, one built for 32 bits and one for 64, so everything
//! here has a fixed width and no pointers: what a plugin call carries by pointer travels as bytes after the header.
//!
// spec: plugins/vst2-bridge.md §2

/// Bytes a channel carries after its header; a larger payload goes through its own mapping (`Header::big`).
pub const DATA: usize = 8 << 20;

/// The three channels between the host and a helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chan {
    /// The host's calls from every thread but the one that processes, served on the helper's main (window) thread.
    Control = 0,
    /// The host's calls from the thread that processes, served on the helper's audio thread.
    Audio = 1,
    /// The plugin's calls to the host that come from no host call in progress (its own threads, its editor).
    Callback = 2,
}

impl Chan {
    pub const ALL: [Chan; 3] = [Chan::Control, Chan::Audio, Chan::Callback];
}

/// What a message is.
pub mod kind {
    pub const REQUEST: u32 = 1;
    pub const REPLY: u32 = 2;
}

/// What a request asks.
pub mod call {
    /// Create the effect (the plugin file's entry point); the reply's `ret` is 1 and `fields` its fields, or 0 and the
    /// payload why.
    pub const CREATE: u32 = 1;
    /// An op-code: `opcode`, `index`, `value`, `opt`, and what the op-code's pointer carries.
    pub const DISPATCH: u32 = 2;
    /// `index`, `opt` the value.
    pub const SET_PARAM: u32 = 3;
    /// `index`; the reply's `ret_f` the value.
    pub const GET_PARAM: u32 = 4;
    /// `index` frames; `value` inputs (bits 0..16) and outputs (bits 16..32); the payload the inputs, channel after
    /// channel; the reply's payload the outputs. Always replacing.
    pub const PROCESS: u32 = 5;
    /// The plugin calls the host: as for `DISPATCH`, with the host op-code.
    pub const CALLBACK: u32 = 6;
    /// `index` parameters from the first: the reply's payload their values (32-bit floats, little-endian), so a host
    /// reading many parameters makes one call.
    pub const GET_PARAMS: u32 = 7;
}

/// The effect's fields, sent with every reply from the helper so the host's copy follows the plugin's.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fields {
    pub magic: i32,
    pub flags: i32,
    pub num_programs: i32,
    pub num_params: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub delay: i32,
    pub unique_id: i32,
    pub version: i32,
    pub io_ratio: f32,
    /// [`has`] bits: which of the effect's functions it has.
    pub has: u32,
    pub _pad: u32,
}

/// `Fields::has` bits.
pub mod has {
    pub const CONTROL: u32 = 1 << 0;
    pub const PROCESS: u32 = 1 << 1;
    pub const PROCESS_FLOAT: u32 = 1 << 2;
    pub const SET_PARAM: u32 = 1 << 3;
    pub const GET_PARAM: u32 = 1 << 4;
}

/// A message's header, at the start of its channel's shared memory.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Header {
    /// [`kind`].
    pub kind: u32,
    /// [`call`].
    pub call: u32,
    pub opcode: i32,
    pub index: i32,
    pub value: i64,
    pub opt: f32,
    pub ret_f: f32,
    pub ret: i64,
    /// Payload bytes.
    pub len: u64,
    /// 1: the payload is in the sender's own mapping (`<channel>.big.<host|helper>`), too large for the channel.
    pub big: u32,
    pub _pad: u32,
    pub fields: Fields,
}

/// The size of the channel's mapping: the header, padded, then [`DATA`].
pub const HEADER_SPACE: usize = 256;
pub const MAPPING: usize = HEADER_SPACE + DATA;

/// The names of a helper's objects: `<base>.<channel>` for a channel's mapping, `.to_host` / `.to_helper` for its two
/// events. `base` is `Local\LemonVst2Bridge.<host pid>.<instance>`.
pub fn base_name(host_pid: u32, instance: u64) -> String {
    format!("Local\\LemonVst2Bridge.{host_pid}.{instance}")
}

pub fn chan_name(base: &str, c: Chan) -> String {
    format!("{base}.{}", c as u32)
}

/// Bytes of a list of MIDI events: each 32 bytes as the plugin interface lays one out (no pointers inside).
pub const MIDI_EVENT_SIZE: usize = 32;

/// Op-codes whose pointer carries text the plugin writes (a buffer of at least 64 bytes; plugins overrun the
/// documented sizes, so the helper's buffer is large).
pub fn out_text(opcode: i32) -> bool {
    // program name, param label / display / name, program name by index, effect / vendor / product name, a shell's
    // next plugin.
    matches!(opcode, 0x05 | 0x06 | 0x07 | 0x08 | 0x1D | 0x2D | 0x2F | 0x30 | 0x46)
}

/// Op-codes whose pointer carries text to the plugin: program name, parameter from text, supports.
pub fn in_text(opcode: i32) -> bool {
    matches!(opcode, 0x04 | 0x1B | 0x33)
}

/// Op-codes whose pointer is a structure the plugin fills, and its size: input / output properties (128 bytes),
/// parameter properties (148).
pub fn out_struct(opcode: i32) -> Option<usize> {
    match opcode {
        0x21 | 0x22 => Some(128),
        0x38 => Some(148),
        _ => None,
    }
}

/// Host op-codes whose pointer carries text the host writes (its vendor and product names).
pub fn host_out_text(opcode: i32) -> bool {
    matches!(opcode, 0x20 | 0x21)
}

/// Host op-codes whose pointer carries text to the host (supports).
pub fn host_in_text(opcode: i32) -> bool {
    opcode == 0x25
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same layout on 32 and 64 bits: only fixed-width fields, 8-byte aligned.
    #[test]
    fn the_header_has_one_layout() {
        assert_eq!(std::mem::size_of::<Fields>(), 48);
        assert_eq!(std::mem::size_of::<Header>(), 104);
        assert!(std::mem::size_of::<Header>() <= HEADER_SPACE);
        assert_eq!(std::mem::offset_of!(Header, value), 16);
        assert_eq!(std::mem::offset_of!(Header, fields), 56);
    }
}
