// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Test plugins built into the test binary, written against [`crate::abi`] the way a VST 2 plugin is: a gain effect
//! (an editor, a chunk state, a switch parameter), a synth (notes, programs, a parameter list as its state) and a shell
//! offering both. They cover what no installed plugin can be counted on for: the host callback's answers, note and
//! automation timing, the transport, editor requests, input / output changes and the teardown order.

use std::cell::RefCell;
use std::ffi::{c_void, CStr};

use crate::abi::*;

pub const GAIN_ID: i32 = fourcc(*b"MkGn");
pub const SYNTH_ID: i32 = fourcc(*b"MkSy");
/// The editor's size.
pub const EDITOR: (i16, i16) = (300, 200);

/// What the test plugins saw. Per thread: each test drives its plugins on its own thread.
#[derive(Default)]
pub struct Probe {
    /// Calls in order ("open", "rate 48000", "resume 1", ...).
    pub log: Vec<String>,
    /// Plugin objects alive.
    pub live: i32,
    /// Each MIDI event received: (offset, status, data 1, data 2).
    pub midi: Vec<(i32, u8, u8, u8)>,
    /// Each process call's length.
    pub blocks: Vec<i32>,
    /// Each process call's transport: (flags, tempo, quarter notes, frames).
    pub times: Vec<(i32, f64, f64, f64)>,
    /// Editor idle calls.
    pub idles: usize,
}

thread_local! {
    static PROBE: RefCell<Probe> = RefCell::default();
}

/// Reads or changes this thread's probe (never across a call into a plugin).
pub fn probe<R>(f: impl FnOnce(&mut Probe) -> R) -> R {
    PROBE.with(|p| f(&mut p.borrow_mut()))
}

fn log(s: impl Into<String>) {
    probe(|p| p.log.push(s.into()));
}

/// A test plugin: the effect first, so the effect pointer is the plugin's.
#[repr(C)]
struct Mock {
    effect: Effect,
    host: HostCallback,
    /// `GAIN_ID`, `SYNTH_ID`, or 0 for the shell.
    id: i32,
    params: [f32; 2],
    program: isize,
    rect: Rect,
    chunk: Vec<u8>,
    shell_next: usize,
    /// The synth's level and the note events of the current block (offset, status, key, velocity).
    level: f32,
    events: Vec<(i32, u8, u8, u8)>,
}

unsafe fn mock<'a>(e: *mut Effect) -> &'a mut Mock {
    unsafe { &mut *(e as *mut Mock) }
}

unsafe fn write(ptr: *mut c_void, s: &str) {
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), ptr as *mut u8, s.len());
        *(ptr as *mut u8).add(s.len()) = 0;
    }
}

fn create(host: HostCallback, id: i32) -> *mut Effect {
    let (gain, synth) = (id == GAIN_ID, id == SYNTH_ID);
    let effect = Effect {
        magic: MAGIC,
        control: Some(dispatch),
        process: None,
        set_parameter: Some(set_parameter),
        get_parameter: Some(get_parameter),
        num_programs: if synth { 4 } else { 1 },
        num_params: if gain { 2 } else if synth { 1 } else { 0 },
        num_inputs: if gain { 2 } else { 0 },
        num_outputs: 2,
        flags: if gain {
            flags::EDITOR | flags::SUPPORTS_FLOAT | flags::CHUNKS
        } else if synth {
            flags::SUPPORTS_FLOAT | flags::INSTRUMENT
        } else {
            0
        },
        host_reserved_1: std::ptr::null_mut(),
        host_reserved_2: std::ptr::null_mut(),
        delay: 0,
        _unknown_02: 0,
        _unknown_03: 0,
        input_output_ratio: 1.0,
        effect_internal: std::ptr::null_mut(),
        host_internal: std::ptr::null_mut(),
        unique_id: id,
        version: 1,
        process_float: Some(process),
        process_double: None,
    };
    let m = Box::new(Mock {
        effect,
        host,
        id,
        params: [0.5, 0.0],
        program: 0,
        rect: Rect { top: 0, left: 0, bottom: EDITOR.1, right: EDITOR.0 },
        chunk: Vec::new(),
        shell_next: 0,
        level: 0.0,
        events: Vec::new(),
    });
    probe(|p| p.live += 1);
    let e = Box::into_raw(m) as *mut Effect;
    // Plugins call back before they hand out their effect: the host answers from the instance being created.
    let rate = unsafe { host(e, host_op::GET_SAMPLE_RATE, 0, 0, std::ptr::null_mut(), 0.0) };
    log(format!("main rate {rate}"));
    e
}

/// The gain effect's file.
pub unsafe extern "C" fn gain_main(host: HostCallback) -> *mut Effect {
    create(host, GAIN_ID)
}

/// The synth's file.
pub unsafe extern "C" fn synth_main(host: HostCallback) -> *mut Effect {
    create(host, SYNTH_ID)
}

/// A shell: the plugin the host asks for, else the shell itself.
pub unsafe extern "C" fn shell_main(host: HostCallback) -> *mut Effect {
    let version = unsafe { host(std::ptr::null_mut(), host_op::VERSION, 0, 0, std::ptr::null_mut(), 0.0) };
    let id = unsafe { host(std::ptr::null_mut(), host_op::CURRENT_ID, 0, 0, std::ptr::null_mut(), 0.0) } as i32;
    log(format!("shell main {version} {id:08X}"));
    create(host, id)
}

/// A file whose entry point gives nothing.
pub unsafe extern "C" fn broken_main(_host: HostCallback) -> *mut Effect {
    std::ptr::null_mut()
}

unsafe extern "C" fn dispatch(e: *mut Effect, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize {
    let m = unsafe { mock(e) };
    let (gain, synth) = (m.id == GAIN_ID, m.id == SYNTH_ID);
    match opcode {
        op::OPEN => log("open"),
        op::CLOSE => {
            log("close");
            probe(|p| p.live -= 1);
            drop(unsafe { Box::from_raw(e as *mut Mock) });
        }
        op::CATEGORY => {
            return if gain {
                category::EFFECT
            } else if synth {
                category::INSTRUMENT
            } else {
                category::CONTAINER
            };
        }
        op::EFFECT_NAME => unsafe { write(ptr, if gain { "Mock Gain" } else if synth { "Mock Synth" } else { "Mock Shell" }) },
        op::VENDOR_NAME => unsafe { write(ptr, "Mock Audio") },
        op::VENDOR_VERSION => return 0x01_02_03,
        op::CONTAINER_NEXT_EFFECT_ID => {
            let list = [(GAIN_ID, "Shell Gain"), (SYNTH_ID, "Shell Synth")];
            let Some(&(id, name)) = list.get(m.shell_next) else { return 0 };
            m.shell_next += 1;
            unsafe { write(ptr, name) };
            return id as isize;
        }
        op::SET_SAMPLE_RATE => log(format!("rate {opt}")),
        op::SET_BLOCK_SIZE => log(format!("block {value}")),
        op::SUSPEND_RESUME => {
            log(format!("resume {value}"));
            if value == 1 && gain && m.effect.delay == 0 {
                m.effect.delay = 64;
            }
        }
        op::PROCESS_BEGIN => log("begin"),
        op::PROCESS_END => log("end"),
        op::GET_PROGRAM => return m.program,
        op::SET_PROGRAM => {
            m.program = value;
            log(format!("program {value}"));
        }
        op::PARAM_NAME => unsafe { write(ptr, ["Gain", "Mode"].get(index as usize).copied().unwrap_or("")) },
        op::PARAM_LABEL if gain && index == 0 => unsafe { write(ptr, "dB") },
        op::PARAM_DISPLAY => unsafe { write(ptr, &format!("{:.2}", m.params[index as usize & 1] * 2.0)) },
        op::PARAM_PROPERTIES if gain && index == 1 => {
            let p = unsafe { &mut *(ptr as *mut ParameterProperties) };
            p.flags = param_flags::SWITCH | param_flags::CATEGORY;
            p.category_label[..4].copy_from_slice(b"Main");
            return 1;
        }
        op::GET_CHUNK => {
            m.chunk = b"MOCK".iter().copied().chain(m.params.iter().flat_map(|v| v.to_le_bytes())).collect();
            unsafe { *(ptr as *mut *mut c_void) = m.chunk.as_mut_ptr().cast() };
            return m.chunk.len() as isize;
        }
        op::SET_CHUNK => {
            let data = unsafe { std::slice::from_raw_parts(ptr as *const u8, value as usize) };
            log(format!("set chunk {}", data.len()));
            if let Some(rest) = data.strip_prefix(b"MOCK") {
                for (p, b) in m.params.iter_mut().zip(rest.chunks_exact(4)) {
                    *p = f32::from_le_bytes(b.try_into().unwrap());
                }
            }
            // Some plugins report their restored values as edits.
            unsafe { (m.host)(e, host_op::AUTOMATE, 0, 0, std::ptr::null_mut(), m.params[0]) };
        }
        op::EVENTS => {
            // The list holds as many pointers as it counts, whatever its declared length.
            let list = ptr as *const Events<1>;
            let first = unsafe { std::ptr::addr_of!((*list).events) } as *const *mut MidiEvent;
            let events = unsafe { std::slice::from_raw_parts(first, (*list).count as usize) };
            for &ev in events {
                let ev = unsafe { &*ev };
                assert_eq!((ev.kind, ev.size), (EVENT_MIDI, 24));
                let item = (ev.offset, ev.data[0], ev.data[1], ev.data[2]);
                probe(|p| p.midi.push(item));
                m.events.push(item);
            }
        }
        op::SUPPORTS => {
            let name = unsafe { CStr::from_ptr(ptr as *const std::ffi::c_char) };
            return if synth && name == supports::RECEIVE_MIDI_EVENT { 1 } else { -1 };
        }
        op::TAIL_SAMPLES => return if synth { 4800 } else { 1 },
        op::EDITOR_GET_RECT => {
            unsafe { *(ptr as *mut *mut Rect) = &mut m.rect };
            return 1;
        }
        op::EDITOR_OPEN => log(format!("editor open {}", ptr as usize)),
        op::EDITOR_CLOSE => log("editor close"),
        op::EDITOR_IDLE => probe(|p| p.idles += 1),
        op::CUSTOM if index == fourcc(*b"PreS") && value == fourcc(*b"AeCs") as isize => log(format!("scale {opt}")),
        _ => {}
    }
    0
}

unsafe extern "C" fn set_parameter(e: *mut Effect, index: i32, value: f32) {
    let m = unsafe { mock(e) };
    if let Some(p) = m.params.get_mut(index as usize) {
        *p = value;
    }
    // Some plugins report every change, the host's own too.
    unsafe { (m.host)(e, host_op::AUTOMATE, index, 0, std::ptr::null_mut(), value) };
    log(format!("set {index} {value}"));
}

unsafe extern "C" fn get_parameter(e: *mut Effect, index: i32) -> f32 {
    unsafe { mock(e) }.params.get(index as usize).copied().unwrap_or(0.0)
}

unsafe extern "C" fn process(e: *mut Effect, inputs: *const *const f32, outputs: *mut *mut f32, frames: i32) {
    let m = unsafe { mock(e) };
    let n = frames as usize;
    let t = unsafe { (m.host)(e, host_op::GET_TIME, 0, 0, std::ptr::null_mut(), 0.0) } as *const TimeInfo;
    let time = unsafe { t.as_ref() }.map(|t| (t.flags, t.tempo, t.ppq_pos, t.sample_pos)).unwrap_or_default();
    probe(|p| {
        p.blocks.push(frames);
        p.times.push(time);
    });
    let outs = m.effect.num_outputs as usize;
    let out: Vec<&mut [f32]> = (0..outs).map(|c| unsafe { std::slice::from_raw_parts_mut(*outputs.add(c), n) }).collect();
    if m.id == GAIN_ID {
        let gain = m.params[0] * 2.0;
        for (c, o) in out.into_iter().enumerate() {
            let input = unsafe { std::slice::from_raw_parts(*inputs.add(c.min(m.effect.num_inputs as usize - 1)), n) };
            o.iter_mut().zip(input).for_each(|(o, i)| *o = i * gain);
        }
        return;
    }
    // The synth: a held note sounds at the level parameter times its velocity, from its event on.
    let mut events = std::mem::take(&mut m.events);
    events.sort_by_key(|e| e.0);
    let mut level = m.level;
    let mut next = events.iter().peekable();
    let mut buf = vec![0.0f32; n];
    for (i, v) in buf.iter_mut().enumerate() {
        while let Some(&&(at, status, key, vel)) = next.peek() {
            if at as usize > i {
                break;
            }
            match status & 0xF0 {
                0x90 => level = m.params[0] * f32::from(vel) / 127.0,
                0x80 => level = 0.0,
                0xB0 if key == 120 => level = 0.0,
                _ => {}
            }
            next.next();
        }
        *v = level;
    }
    m.level = level;
    for o in out {
        o.copy_from_slice(&buf);
    }
}

/// Simulates a gesture in the plugin's editor: parameter `index` dragged to `value`.
pub unsafe fn edit(e: *mut Effect, index: i32, value: f32) {
    let m = unsafe { mock(e) };
    let none = std::ptr::null_mut();
    unsafe {
        (m.host)(e, host_op::BEGIN_EDIT, index, 0, none, 0.0);
        m.params[index as usize] = value;
        (m.host)(e, host_op::AUTOMATE, index, 0, none, value);
        (m.host)(e, host_op::END_EDIT, index, 0, none, 0.0);
    }
}

/// Simulates the editor asking for a new size.
pub unsafe fn ask_size(e: *mut Effect, w: i32, h: i32) {
    let m = unsafe { mock(e) };
    unsafe { (m.host)(e, host_op::SIZE_WINDOW, w, h as isize, std::ptr::null_mut(), 0.0) };
}

/// Simulates the plugin switching to one output with `delay` frames of latency.
pub unsafe fn go_mono(e: *mut Effect, delay: i32) {
    let m = unsafe { mock(e) };
    m.effect.num_outputs = 1;
    m.effect.delay = delay;
    unsafe { (m.host)(e, host_op::IO_CHANGED, 0, 0, std::ptr::null_mut(), 0.0) };
}

/// Asks the host something the way a plugin does.
pub unsafe fn ask(e: *mut Effect, opcode: i32, ptr: *mut c_void) -> isize {
    let m = unsafe { mock(e) };
    unsafe { (m.host)(e, opcode, 0, 0, ptr, 0.0) }
}
