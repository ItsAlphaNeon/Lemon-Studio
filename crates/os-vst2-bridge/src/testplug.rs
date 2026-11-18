// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The helper's built-in test plugin (plugin file `:test:`), so the bridge is tested end to end without a plugin file:
//! "Bridge Test", an instrument with two inputs and two outputs and one parameter (gain). Its output is its input
//! times the gain, plus 0.25 while a note is held, plus the host's tempo / 1000 when the host gives one. It saves its
//! gain as its state, has an editor (a plain child window, 200 × 100), and when its editor opens it reports a gain
//! of 0.75 from a thread of its own (stored first), as an editor's edit.

use std::ffi::c_void;

use crate::abi::{
    self, Effect, HostCallback, MidiEvent, Rect, TimeInfo, category, flags, host_op, op,
};

#[repr(C)]
struct Test {
    effect: Effect,
    callback: Option<HostCallback>,
    gain: f32,
    held: [bool; 128],
    rect: Rect,
    chunk: [u8; 8],
    editor: isize,
}

pub const UNIQUE_ID: i32 = abi::fourcc(*b"LBrT");

fn write(ptr: *mut c_void, s: &str) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), ptr as *mut u8, s.len());
        *(ptr as *mut u8).add(s.len()) = 0;
    }
}

/// The entry point.
pub unsafe extern "C" fn main(callback: HostCallback) -> *mut Effect {
    // A plugin asks the host's version while it starts, before it has an effect.
    unsafe {
        callback(
            std::ptr::null_mut(),
            host_op::VERSION,
            0,
            0,
            std::ptr::null_mut(),
            0.0,
        )
    };
    let mut t: Box<Test> = Box::new(unsafe { std::mem::zeroed() });
    t.callback = Some(callback);
    t.gain = 0.5;
    t.rect = Rect {
        top: 0,
        left: 0,
        bottom: 100,
        right: 200,
    };
    let e = &mut t.effect;
    e.magic = abi::MAGIC;
    e.control = Some(control);
    e.process_float = Some(process);
    e.set_parameter = Some(set);
    e.get_parameter = Some(get);
    e.num_programs = 1;
    e.num_params = 1;
    e.num_inputs = 2;
    e.num_outputs = 2;
    e.flags = flags::EDITOR | flags::SUPPORTS_FLOAT | flags::CHUNKS | flags::INSTRUMENT;
    e.unique_id = UNIQUE_ID;
    e.version = 0x010203;
    e.input_output_ratio = 1.0;
    let raw = Box::into_raw(t);
    unsafe { (*raw).effect.effect_internal = raw as *mut c_void };
    raw as *mut Effect
}

fn this<'a>(e: *mut Effect) -> &'a mut Test {
    unsafe { &mut *(e as *mut Test) }
}

unsafe extern "C" fn control(
    e: *mut Effect,
    opcode: i32,
    index: i32,
    value: isize,
    ptr: *mut c_void,
    opt: f32,
) -> isize {
    let t = this(e);
    let _ = (value, opt);
    match opcode {
        op::CLOSE => {
            drop(unsafe { Box::from_raw(e as *mut Test) });
            1
        }
        op::EFFECT_NAME | op::PRODUCT_NAME => {
            write(ptr, "Bridge Test");
            1
        }
        op::VENDOR_NAME => {
            write(ptr, "Lemon Studio Tests");
            1
        }
        op::VENDOR_VERSION => 0x010203,
        op::CATEGORY => category::INSTRUMENT,
        op::PARAM_NAME if index == 0 => {
            write(ptr, "Gain");
            1
        }
        op::PARAM_LABEL if index == 0 => {
            write(ptr, "x");
            1
        }
        op::PARAM_DISPLAY if index == 0 => {
            write(ptr, &format!("{:.2}", t.gain));
            1
        }
        op::GET_CHUNK => {
            t.chunk[..4].copy_from_slice(&t.gain.to_le_bytes());
            t.chunk[4..].copy_from_slice(b"LBrT");
            unsafe { *(ptr as *mut *mut c_void) = t.chunk.as_mut_ptr().cast() };
            8
        }
        op::SET_CHUNK => {
            if value >= 4 && !ptr.is_null() {
                let b = unsafe { std::slice::from_raw_parts(ptr as *const u8, 4) };
                t.gain = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            1
        }
        op::EVENTS => {
            let slots = ptr as *const isize;
            let count = unsafe { *slots as i32 }.max(0) as usize;
            for i in 0..count {
                let ev = unsafe { &*(*slots.add(2 + i) as *const MidiEvent) };
                let (status, key, vel) = (ev.data[0] & 0xF0, ev.data[1] as usize & 127, ev.data[2]);
                match status {
                    0x90 if vel > 0 => t.held[key] = true,
                    0x80 | 0x90 => t.held[key] = false,
                    _ => {}
                }
            }
            1
        }
        op::EDITOR_GET_RECT => {
            unsafe { *(ptr as *mut *mut Rect) = &mut t.rect };
            1
        }
        op::EDITOR_OPEN => {
            t.editor = open_editor(ptr);
            // An edit in the editor, reported from a thread of the plugin's own.
            let (cb, addr) = (t.callback.expect("callback"), e as usize);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(30));
                // As a plugin does: the value stored, then reported.
                this(addr as *mut Effect).gain = 0.75;
                unsafe {
                    cb(
                        addr as *mut Effect,
                        host_op::AUTOMATE,
                        0,
                        0,
                        std::ptr::null_mut(),
                        0.75,
                    )
                };
            });
            1
        }
        op::EDITOR_CLOSE => {
            close_editor(t.editor);
            t.editor = 0;
            1
        }
        op::SUPPORTS => 1,
        _ => 0,
    }
}

#[cfg(windows)]
fn open_editor(parent: *mut c_void) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_CHILD, WS_VISIBLE};
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let title: Vec<u16> = "Bridge Test\0".encode_utf16().collect();
    unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            200,
            100,
            parent,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        ) as isize
    }
}

#[cfg(windows)]
fn close_editor(w: isize) {
    if w != 0 {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(w as _) };
    }
}

#[cfg(not(windows))]
fn open_editor(_: *mut c_void) -> isize {
    0
}

#[cfg(not(windows))]
fn close_editor(_: isize) {}

unsafe extern "C" fn process(
    e: *mut Effect,
    inputs: *const *const f32,
    outputs: *mut *mut f32,
    frames: i32,
) {
    let t = this(e);
    let time = unsafe {
        (t.callback.expect("callback"))(e, host_op::GET_TIME, 0, 0, std::ptr::null_mut(), 0.0)
    } as *const TimeInfo;
    let tempo = if time.is_null() {
        0.0
    } else {
        (unsafe { (*time).tempo }) as f32
    };
    let note = if t.held.iter().any(|&h| h) { 0.25 } else { 0.0 };
    for c in 0..2 {
        unsafe {
            let i = *inputs.add(c);
            let o = *outputs.add(c);
            for n in 0..frames.max(0) as usize {
                *o.add(n) = *i.add(n) * t.gain + note + tempo / 1000.0;
            }
        }
    }
}

unsafe extern "C" fn set(e: *mut Effect, index: i32, value: f32) {
    if index == 0 {
        this(e).gain = value;
    }
}

unsafe extern "C" fn get(e: *mut Effect, index: i32) -> f32 {
    if index == 0 { this(e).gain } else { 0.0 }
}
