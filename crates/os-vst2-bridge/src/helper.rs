// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The helper process (`lemonstudio-bridge32`): it loads one VST 2 plugin file, creates one effect, and serves the
//! host's calls on it until the host closes it or ends. Built for 32-bit Windows to run 32-bit plugins; it works the
//! same built for 64 bits (the tests run it that way).
//!
//! Threads: the main thread serves the control channel and pumps the window messages the plugin's editor needs; a
//! second thread serves the audio channel. A plugin's call to the host goes back on the channel of the host call the
//! calling thread is serving, else on the callback channel.
//!
//! Usage: `lemonstudio-bridge32 <base name> <host pid> <plugin file>` (`:test:` for the built-in test plugin).
//!
// spec: plugins/vst2-bridge.md §3

use std::cell::{Cell, RefCell, UnsafeCell};
use std::ffi::{c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};

use crate::abi::{flags, host_op, op, Effect, Main, MidiEvent, Rect, TimeInfo};
use crate::ipc::{Channel, Pump};
use crate::proto::{self, call, has, Chan, Fields, Header};

struct Helper {
    chans: Vec<Channel>,
    host: HANDLE,
    main: Main,
    effect: AtomicPtr<Effect>,
    /// What the plugin reads through pointers the host gave: the transport (`GET_TIME`) and its folder (`DIRECTORY`).
    time: UnsafeCell<TimeInfo>,
    directory: Mutex<CString>,
    /// The last event list sent, alive until the next (plugins read it during the next process call).
    events: Mutex<(Vec<MidiEvent>, Vec<isize>)>,
    /// One spontaneous call at a time.
    cb_lock: Mutex<()>,
    quit: AtomicBool,
}

// SAFETY: `time` is written by the host's transport answer and read by the plugin, as in a plugin's own host.
unsafe impl Sync for Helper {}
unsafe impl Send for Helper {}

static HELPER: OnceLock<Helper> = OnceLock::new();

thread_local! {
    /// The channel whose request this thread is serving.
    static SERVING: Cell<Option<Chan>> = const { Cell::new(None) };
    /// This thread's process buffers.
    static BUFFERS: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

fn pump_for(c: Chan) -> Pump {
    match c {
        Chan::Audio => Pump::None,
        _ => Pump::Sent,
    }
}

fn fields_of(e: *mut Effect) -> Fields {
    if e.is_null() {
        return Fields::default();
    }
    unsafe {
        let e = &*e;
        let mut h = 0;
        for (b, bit) in [(e.control.is_some(), has::CONTROL), (e.process.is_some(), has::PROCESS), (e.process_float.is_some(), has::PROCESS_FLOAT), (e.set_parameter.is_some(), has::SET_PARAM), (e.get_parameter.is_some(), has::GET_PARAM)] {
            if b {
                h |= bit;
            }
        }
        Fields {
            magic: e.magic,
            flags: e.flags,
            num_programs: e.num_programs,
            num_params: e.num_params,
            num_inputs: e.num_inputs,
            num_outputs: e.num_outputs,
            delay: e.delay,
            unique_id: e.unique_id,
            version: e.version,
            io_ratio: e.input_output_ratio,
            has: h,
            _pad: 0,
        }
    }
}

/// Runs the helper; returns its exit code.
pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: lemonstudio-bridge32 <base name> <host pid> <plugin file>");
        return 2;
    }
    let (base, pid, path) = (&args[1], args[2].parse::<u32>().unwrap_or(0), &args[3]);
    // The plugin's editor sits in the host's window: the same DPI handling as the host.
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let host = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    let mut chans = Vec::new();
    for c in Chan::ALL {
        match Channel::open(&proto::chan_name(base, c)) {
            Ok(ch) => chans.push(ch),
            Err(e) => {
                eprintln!("[bridge] {e}");
                return 3;
            }
        }
    }
    let main = match entry(path) {
        Ok(m) => m,
        Err(e) => {
            // Answer the host's creation with why.
            let ch = &chans[Chan::Control as usize];
            if ch.wait(host, Pump::Sent, None).is_ok() {
                let _ = ch.read();
                ch.reply(Header::default(), e.as_bytes());
            }
            return 4;
        }
    };
    let _ = HELPER.set(Helper {
        chans,
        host,
        main,
        effect: AtomicPtr::new(std::ptr::null_mut()),
        time: UnsafeCell::new(TimeInfo::default()),
        directory: Mutex::new(CString::default()),
        events: Mutex::new((Vec::new(), Vec::new())),
        cb_lock: Mutex::new(()),
        quit: AtomicBool::new(false),
    });
    std::thread::spawn(|| serve_loop(Chan::Audio, Pump::None));
    serve_loop(Chan::Control, Pump::All);
    0
}

/// The plugin file's entry point (kept loaded for the process's life), or the built-in test plugin's.
fn entry(path: &str) -> Result<Main, String> {
    if path == ":test:" {
        return Ok(crate::testplug::main);
    }
    unsafe {
        let lib = libloading::Library::new(path).map_err(|e| format!("{path}: {e}"))?;
        let main = ["VSTPluginMain\0", "main\0"]
            .iter()
            .find_map(|n| lib.get::<Main>(n.as_bytes()).ok().map(|s| *s))
            .ok_or_else(|| format!("{path}: not a VST 2 plugin (no VSTPluginMain)"))?;
        std::mem::forget(lib);
        Ok(main)
    }
}

/// Serves channel `c`'s requests until the host ends (then the process ends) or closes the effect (control).
fn serve_loop(c: Chan, pump: Pump) {
    let h = HELPER.get().expect("helper");
    let ch = &h.chans[c as usize];
    loop {
        if ch.wait(h.host, pump, None).is_err() {
            std::process::exit(0);
        }
        let (req, payload) = ch.read();
        let (r, out) = serve(c, &req, payload);
        ch.reply(r, &out);
        if c == Chan::Control && h.quit.load(Ordering::Acquire) {
            // Closed: the host is done with this process.
            std::process::exit(0);
        }
    }
}

/// Answers one request on channel `c`.
fn serve(c: Chan, req: &Header, payload: Vec<u8>) -> (Header, Vec<u8>) {
    let h = HELPER.get().expect("helper");
    let prev = SERVING.with(|s| s.replace(Some(c)));
    let mut r = Header::default();
    let mut out = Vec::new();
    let e = h.effect.load(Ordering::Acquire);
    unsafe {
        match req.call {
            call::CREATE => {
                let made = (h.main)(callback);
                if made.is_null() {
                    out = b"the plugin did not start".to_vec();
                } else {
                    h.effect.store(made, Ordering::Release);
                    r.ret = 1;
                }
            }
            call::DISPATCH if !e.is_null() => {
                r.ret = dispatch(h, e, req, payload, &mut out) as i64;
                if req.opcode == op::CLOSE {
                    h.effect.store(std::ptr::null_mut(), Ordering::Release);
                    h.quit.store(true, Ordering::Release);
                }
            }
            call::SET_PARAM if !e.is_null() => {
                if let Some(f) = (*e).set_parameter {
                    f(e, req.index, req.opt);
                }
            }
            call::GET_PARAM if !e.is_null() => {
                if let Some(f) = (*e).get_parameter {
                    r.ret_f = f(e, req.index);
                }
            }
            call::PROCESS if !e.is_null() => out = process(e, req, &payload),
            _ => {}
        }
    }
    r.fields = fields_of(h.effect.load(Ordering::Acquire));
    SERVING.with(|s| s.set(prev));
    (r, out)
}

/// Up to the first NUL.
fn until_nul(b: &[u8]) -> Vec<u8> {
    b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())].to_vec()
}

unsafe fn dispatch(h: &Helper, e: *mut Effect, req: &Header, mut payload: Vec<u8>, out: &mut Vec<u8>) -> isize {
    let Some(f) = (unsafe { (*e).control }) else { return 0 };
    let (opc, index, value, opt) = (req.opcode, req.index, req.value as isize, req.opt);
    unsafe {
        if proto::out_text(opc) {
            let mut buf = vec![0u8; 1024];
            let r = f(e, opc, index, value, buf.as_mut_ptr().cast(), opt);
            *out = until_nul(&buf);
            return r;
        }
        if proto::in_text(opc) {
            payload.push(0);
            return f(e, opc, index, value, payload.as_mut_ptr().cast(), opt);
        }
        if let Some(n) = proto::out_struct(opc) {
            let mut buf = vec![0u8; n.max(512)];
            let r = f(e, opc, index, value, buf.as_mut_ptr().cast(), opt);
            *out = buf[..n].to_vec();
            return r;
        }
        match opc {
            op::EDITOR_GET_RECT => {
                let mut rect: *mut Rect = std::ptr::null_mut();
                let r = f(e, opc, index, value, (&mut rect as *mut *mut Rect).cast(), opt);
                if !rect.is_null() {
                    let v = *rect;
                    *out = [v.top, v.left, v.bottom, v.right].iter().flat_map(|x| x.to_le_bytes()).collect();
                }
                r
            }
            // The parent window travels as a number.
            op::EDITOR_OPEN => f(e, opc, index, 0, req.value as isize as *mut c_void, opt),
            op::GET_CHUNK => {
                let mut p: *mut c_void = std::ptr::null_mut();
                let n = f(e, opc, index, value, (&mut p as *mut *mut c_void).cast(), opt);
                if n > 0 && !p.is_null() {
                    *out = std::slice::from_raw_parts(p as *const u8, n as usize).to_vec();
                }
                n
            }
            op::SET_CHUNK => f(e, opc, index, payload.len() as isize, payload.as_mut_ptr().cast(), opt),
            op::EVENTS => {
                let mut ev = h.events.lock().unwrap_or_else(|x| x.into_inner());
                let (midi, list) = &mut *ev;
                midi.clear();
                for c in payload.chunks_exact(proto::MIDI_EVENT_SIZE) {
                    midi.push(std::ptr::read_unaligned(c.as_ptr() as *const MidiEvent));
                }
                // The list: the count, a pointer-sized reserved field, the event pointers (one slot each).
                list.clear();
                list.push(midi.len() as isize);
                list.push(0);
                for m in midi.iter_mut() {
                    list.push(m as *mut MidiEvent as isize);
                }
                f(e, opc, index, value, list.as_mut_ptr().cast(), opt)
            }
            _ => f(e, opc, index, value, std::ptr::null_mut(), opt),
        }
    }
}

/// Runs the plugin on the inputs in `payload`; the outputs, channel after channel.
unsafe fn process(e: *mut Effect, req: &Header, payload: &[u8]) -> Vec<u8> {
    let n = req.index.max(0) as usize;
    let ins = (req.value & 0xFFFF) as usize;
    let outs = ((req.value >> 16) & 0xFFFF) as usize;
    BUFFERS.with(|b| {
        let mut b = b.borrow_mut();
        b.clear();
        b.resize((ins + outs) * n, 0.0);
        for (i, c) in payload.chunks_exact(4).take(ins * n).enumerate() {
            b[i] = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
        }
        let base = b.as_mut_ptr();
        let inp: Vec<*const f32> = (0..ins).map(|c| unsafe { base.add(c * n) as *const f32 }).collect();
        let outp: Vec<*mut f32> = (0..outs).map(|c| unsafe { base.add((ins + c) * n) }).collect();
        unsafe {
            let replacing = (*e).flags & flags::SUPPORTS_FLOAT != 0;
            match ((*e).process_float.filter(|_| replacing), (*e).process) {
                (Some(f), _) | (None, Some(f)) => f(e, inp.as_ptr(), outp.as_ptr() as *mut *mut f32, n as i32),
                _ => {}
            }
        }
        b[ins * n..].iter().flat_map(|v| v.to_le_bytes()).collect()
    })
}

/// The host callback the plugin gets: each call goes to the host and its answer comes back.
pub unsafe extern "C" fn callback(effect: *mut Effect, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize {
    let _ = effect;
    let Some(h) = HELPER.get() else {
        return if opcode == host_op::VERSION { 2400 } else { 0 };
    };
    let payload: Vec<u8> = unsafe {
        if proto::host_in_text(opcode) && !ptr.is_null() {
            CStr::from_ptr(ptr as *const std::ffi::c_char).to_bytes().to_vec()
        } else if opcode == host_op::PROCESS_EVENTS && !ptr.is_null() {
            // An event list as plugins lay it out: the count, a pointer-sized reserved field, the pointers.
            let slots = ptr as *const isize;
            let count = (*slots as i32).max(0) as usize;
            let mut v = Vec::new();
            for i in 0..count.min(4096) {
                let ev = *slots.add(2 + i) as *const MidiEvent;
                if !ev.is_null() && (*ev).kind == crate::abi::EVENT_MIDI {
                    v.extend_from_slice(std::slice::from_raw_parts(ev as *const u8, proto::MIDI_EVENT_SIZE));
                }
            }
            v
        } else {
            Vec::new()
        }
    };
    let req = Header { call: call::CALLBACK, opcode, index, value: value as i64, opt, ..Header::default() };
    let serving = SERVING.with(Cell::get);
    let result = match serving {
        Some(c) => {
            let ch = &h.chans[c as usize];
            ch.call(req, &payload, h.host, pump_for(c), None, &mut |r, p| serve(c, r, p))
        }
        None => {
            let _lock = h.cb_lock.lock().unwrap_or_else(|x| x.into_inner());
            let ch = &h.chans[Chan::Callback as usize];
            ch.call(req, &payload, h.host, Pump::Sent, None, &mut |r, p| serve(Chan::Callback, r, p))
        }
    };
    let Ok((r, data)) = result else { return 0 };
    unsafe {
        match opcode {
            host_op::GET_TIME => {
                if r.ret == 0 || data.len() < std::mem::size_of::<TimeInfo>() {
                    return 0;
                }
                let t = std::ptr::read_unaligned(data.as_ptr() as *const TimeInfo);
                h.time.get().write(t);
                h.time.get() as isize
            }
            host_op::DIRECTORY => {
                if data.is_empty() {
                    return 0;
                }
                let mut d = h.directory.lock().unwrap_or_else(|x| x.into_inner());
                *d = CString::new(until_nul(&data)).unwrap_or_default();
                d.as_ptr() as isize
            }
            o if proto::host_out_text(o) && !ptr.is_null() => {
                let n = data.len().min(63);
                std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, n);
                *(ptr as *mut u8).add(n) = 0;
                r.ret as isize
            }
            _ => r.ret as isize,
        }
    }
}
