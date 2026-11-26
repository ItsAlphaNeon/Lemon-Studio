// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The host's side of the VST 2 bridge (Windows): a 32-bit plugin runs in its own helper process
//! (`lemonstudio-bridge32`, crate `os-vst2-bridge`), one per instance, and the host gets a stand-in effect whose
//! functions carry every call there and back. The rest of this crate hosts the stand-in as it hosts any effect, so
//! loading, processing, parameters, state, MIDI and the editor work unchanged; a helper that crashes or hangs takes only
//! its plugin with it (silent from then on).
//!
//! Calls from the thread that processes go on the audio channel, all others on the control channel; the plugin's calls
//! back come on the channel of the call they happen in, or on the callback channel, served by a thread of the stand-in's
//! own.
//!
// spec: plugins/vst2-bridge.md §4

use std::ffi::{c_void, CStr, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use os_vst2_bridge::ipc::{wait_any, Channel, Pump};
use os_vst2_bridge::proto::{self, call, has, Chan, Fields, Header};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateProcessW, GetCurrentProcessId, GetCurrentThreadId, SetEvent, TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
};

use crate::abi::{host_op, op, Effect, MidiEvent, Rect, TimeInfo, MAGIC};
use crate::host;

/// How long the helper may take to answer: creating a plugin can load a lot; a processing call is short.
const CREATE_TIMEOUT: Duration = Duration::from_secs(120);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(60);
const AUDIO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long parameter values read from the helper are answered from the host's copy. Each read across the bridge
/// costs a round trip (about 85 µs), and the host reads every parameter of a plugin whose values changed; changes the
/// host makes or the plugin reports clear or update the copy at once, others show within this time.
const PARAMS_FRESH: Duration = Duration::from_millis(100);

/// The helper's file name.
pub const HELPER: &str = "lemonstudio-bridge32.exe";

/// The helper Lemon Studio runs 32-bit plugins with, when it has one: `OS_VST2_BRIDGE`, else the one beside this
/// program, else the one this program carries (written out once), else (a build from source) the one Cargo built
/// beside it for 32-bit Windows. Only a 32-bit program is taken: a build of the helper for this machine (Cargo puts
/// one beside a build of Lemon Studio) cannot load a 32-bit plugin.
pub fn helper() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    let x86 = |p: &Path| crate::files::pe_machine(p) == Some("x86");
    FOUND
        .get_or_init(|| {
            if let Some(p) = std::env::var_os("OS_VST2_BRIDGE").filter(|p| !p.is_empty()) {
                return Some(PathBuf::from(p)).filter(|p| x86(p));
            }
            let exe = std::env::current_exe().ok()?;
            let dir = exe.parent()?;
            let beside = dir.join(HELPER);
            if x86(&beside) {
                return Some(beside);
            }
            if let Some(p) = carried().filter(|p| x86(p)) {
                return Some(p);
            }
            // target/<profile>/[deps/] → target/i686-pc-windows-msvc/<profile>/
            let mut d = dir;
            if d.file_name().is_some_and(|n| n == "deps") {
                d = d.parent()?;
            }
            let profile = d.file_name()?;
            let built = d.parent()?.join("i686-pc-windows-msvc").join(profile).join(HELPER);
            x86(&built).then_some(built)
        })
        .clone()
}

/// The helper this program was built carrying (`LEMON_BRIDGE32` at build time), written to the user's temporary
/// folder under a name that changes with its contents.
fn carried() -> Option<PathBuf> {
    #[cfg(lemon_bridge32)]
    {
        const BYTES: &[u8] = include_bytes!(env!("LEMON_BRIDGE32_FILE"));
        let hash = BYTES.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3));
        let path = std::env::temp_dir().join(format!("LemonStudio-bridge32-{hash:016x}.exe"));
        if std::fs::metadata(&path).is_ok_and(|m| m.len() == BYTES.len() as u64) {
            return Some(path);
        }
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&tmp, BYTES).ok()?;
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        return path.is_file().then_some(path);
    }
    #[allow(unreachable_code)]
    None
}

/// What the stand-in effect and its callback thread share.
struct Link {
    chans: Vec<Channel>,
    process: HANDLE,
    /// The callback thread's stop signal.
    stop: HANDLE,
    dead: AtomicBool,
    /// The stand-in effect (null while it is being created, and after it is closed).
    effect: AtomicPtr<Effect>,
    /// One host call at a time per channel; the thread waiting on each (a call from it while it waits is dropped).
    locks: [Mutex<()>; 2],
    owners: [AtomicU32; 2],
    /// The thread that processes: its calls go on the audio channel.
    audio_thread: AtomicU32,
    name: String,
    /// The plugin's parameter values as last read, all at once (`call::GET_PARAMS`), and until when they stand.
    params: Mutex<Params>,
}

#[derive(Default)]
struct Params {
    values: Vec<f32>,
    until: Option<Instant>,
    /// Moves on with every change: values read across one are not kept as standing.
    generation: u64,
}

impl Params {
    fn fresh(&self, index: i32) -> Option<f32> {
        let i = usize::try_from(index).ok()?;
        self.until.filter(|t| Instant::now() < *t).and(self.values.get(i).copied())
    }
}

// SAFETY: the handles are process-wide; the channels have their own locks.
unsafe impl Send for Link {}
unsafe impl Sync for Link {}

impl Link {
    fn kill(&self) {
        if !self.dead.swap(true, Ordering::AcqRel) {
            unsafe { TerminateProcess(self.process, 1) };
            eprintln!("[os-vst2] {}: the 32-bit plugin's helper stopped answering", self.name);
        }
    }

    /// Sends request `req` on channel `c` and returns the reply, serving the plugin's calls back meanwhile. `None`
    /// when the helper has gone (or this thread is already waiting on `c`).
    fn call(&self, c: Chan, req: Header, payload: &[u8], timeout: Duration, reply_payload: bool) -> Option<(Header, Vec<u8>)> {
        if self.dead.load(Ordering::Acquire) {
            return None;
        }
        let me = unsafe { GetCurrentThreadId() };
        let i = c as usize;
        if self.owners[i].load(Ordering::Acquire) == me {
            return None;
        }
        let _g = self.locks[i].lock().unwrap_or_else(|e| e.into_inner());
        self.owners[i].store(me, Ordering::Release);
        let pump = if c == Chan::Audio { Pump::None } else { Pump::Sent };
        let ch = &self.chans[i];
        let r = ch.call_with(req, payload, self.process, pump, Some(timeout), reply_payload, &mut |h, p| self.serve_callback(h, p));
        self.owners[i].store(0, Ordering::Release);
        match r {
            Ok(x) => Some(x),
            Err(_) => {
                self.kill();
                None
            }
        }
    }

    /// The plugin calls the host: the call made here with what its pointer carries, and the answer sent back.
    fn serve_callback(&self, h: &Header, mut payload: Vec<u8>) -> (Header, Vec<u8>) {
        let mut r = Header::default();
        let mut out = Vec::new();
        if h.call != call::CALLBACK {
            return (r, out);
        }
        let e = self.effect.load(Ordering::Acquire);
        let (opc, index, value, opt) = (h.opcode, h.index, h.value as isize, h.opt);
        let none = std::ptr::null_mut();
        match opc {
            // The plugin's own edit: the copy takes the value as the plugin gave it.
            host_op::AUTOMATE => {
                if let (Ok(mut p), Ok(i)) = (self.params.lock(), usize::try_from(index)) {
                    if let Some(v) = p.values.get_mut(i) {
                        *v = opt;
                    }
                }
            }
            host_op::UPDATE_DISPLAY | host_op::IO_CHANGED => self.params_changed(),
            _ => {}
        }
        unsafe {
            match opc {
                host_op::GET_TIME => {
                    let t = host::callback(e, opc, index, value, none, opt) as *const TimeInfo;
                    if !t.is_null() {
                        out = std::slice::from_raw_parts(t as *const u8, std::mem::size_of::<TimeInfo>()).to_vec();
                        r.ret = 1;
                    }
                }
                host_op::DIRECTORY => {
                    let d = host::callback(e, opc, index, value, none, opt) as *const std::ffi::c_char;
                    if !d.is_null() {
                        out = CStr::from_ptr(d).to_bytes().to_vec();
                        r.ret = 1;
                    }
                }
                host_op::PROCESS_EVENTS => {
                    let mut midi: Vec<MidiEvent> = payload.chunks_exact(proto::MIDI_EVENT_SIZE).map(|c| std::ptr::read_unaligned(c.as_ptr() as *const MidiEvent)).collect();
                    let mut list: Vec<isize> = vec![midi.len() as isize, 0];
                    list.extend(midi.iter_mut().map(|m| m as *mut MidiEvent as isize));
                    r.ret = host::callback(e, opc, index, value, list.as_mut_ptr().cast(), opt) as i64;
                }
                o if proto::host_out_text(o) => {
                    let mut buf = [0u8; 256];
                    r.ret = host::callback(e, opc, index, value, buf.as_mut_ptr().cast(), opt) as i64;
                    out = buf[..buf.iter().position(|&b| b == 0).unwrap_or(buf.len())].to_vec();
                }
                o if proto::host_in_text(o) => {
                    payload.push(0);
                    r.ret = host::callback(e, opc, index, value, payload.as_mut_ptr().cast(), opt) as i64;
                }
                _ => r.ret = host::callback(e, opc, index, value, none, opt) as i64,
            }
        }
        (r, out)
    }

    /// The host's copy of the parameter values is out of date.
    fn params_changed(&self) {
        if let Ok(mut p) = self.params.lock() {
            p.until = None;
            p.generation += 1;
        }
    }

    /// The channel for a call from this thread.
    fn chan(&self) -> Chan {
        if unsafe { GetCurrentThreadId() } == self.audio_thread.load(Ordering::Acquire) { Chan::Audio } else { Chan::Control }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        unsafe {
            if WaitForSingleObject(self.process, 2000) != WAIT_OBJECT_0 {
                TerminateProcess(self.process, 0);
            }
            CloseHandle(self.process);
            CloseHandle(self.stop);
        }
    }
}

/// The stand-in effect: the effect first (the host holds a pointer to it), then what it needs.
#[repr(C)]
struct Proxy {
    effect: Effect,
    link: Arc<Link>,
    callbacks: Option<std::thread::JoinHandle<()>>,
    /// What the host reads through pointers the plugin gave: the editor's rectangle, the last state.
    rect: Rect,
    chunk: Vec<u8>,
}

fn proxy<'a>(e: *mut Effect) -> &'a mut Proxy {
    unsafe { &mut *(e as *mut Proxy) }
}

fn set_fields(e: &mut Effect, f: &Fields) {
    if f.magic != MAGIC {
        return;
    }
    e.flags = f.flags;
    e.num_programs = f.num_programs;
    e.num_params = f.num_params;
    e.num_inputs = f.num_inputs;
    e.num_outputs = f.num_outputs;
    e.delay = f.delay;
    e.unique_id = f.unique_id;
    e.version = f.version;
    e.input_output_ratio = f.io_ratio;
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// Starts a helper for plugin file `plugin` and creates its effect there; the stand-in effect, which the host closes
/// as any effect (`op::CLOSE` frees it). Called inside `host::creating`, so the plugin's calls back while it starts
/// reach the instance being made.
pub fn create(helper: &Path, plugin: &Path) -> Result<*mut Effect, String> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let pid = unsafe { GetCurrentProcessId() };
    let base = proto::base_name(pid, NEXT.fetch_add(1, Ordering::Relaxed));
    let chans = Chan::ALL.iter().map(|&c| Channel::create(&proto::chan_name(&base, c))).collect::<Result<Vec<_>, _>>()?;
    let stop = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    let cmd = format!("\"{}\" {base} {pid} \"{}\"", helper.display(), plugin.display());
    let mut cmd_w: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
    let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let app = wide(helper.as_os_str());
    let ok = unsafe { CreateProcessW(app.as_ptr(), cmd_w.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, CREATE_NO_WINDOW, std::ptr::null(), std::ptr::null(), &si, &mut pi) };
    if ok == 0 {
        unsafe { CloseHandle(stop) };
        return Err(format!("cannot start the 32-bit bridge ({}): error {}", helper.display(), std::io::Error::last_os_error()));
    }
    unsafe { CloseHandle(pi.hThread) };
    let name = plugin.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let link = Arc::new(Link {
        chans,
        process: pi.hProcess,
        stop,
        dead: AtomicBool::new(false),
        effect: AtomicPtr::new(std::ptr::null_mut()),
        locks: [Mutex::new(()), Mutex::new(())],
        owners: [AtomicU32::new(0), AtomicU32::new(0)],
        audio_thread: AtomicU32::new(0),
        name,
        params: Mutex::new(Params::default()),
    });
    let req = Header { call: call::CREATE, ..Header::default() };
    let Some((r, why)) = link.call(Chan::Control, req, &[], CREATE_TIMEOUT, true) else {
        return Err("the 32-bit plugin's helper ended while it started".into());
    };
    if r.ret != 1 {
        link.kill();
        let why = String::from_utf8_lossy(&why).into_owned();
        return Err(if why.is_empty() { "the plugin did not start".into() } else { why });
    }
    let mut effect: Effect = unsafe { std::mem::zeroed() };
    effect.magic = MAGIC;
    effect.control = Some(p_control);
    effect.process = Some(p_process);
    effect.process_float = (r.fields.has & has::PROCESS_FLOAT != 0).then_some(p_process_float as crate::abi::Process);
    effect.set_parameter = Some(p_set);
    effect.get_parameter = Some(p_get);
    set_fields(&mut effect, &r.fields);
    let p = Box::into_raw(Box::new(Proxy { effect, link: link.clone(), callbacks: None, rect: Rect::default(), chunk: Vec::new() }));
    unsafe { (*p).effect.effect_internal = p as *mut c_void };
    link.effect.store(p as *mut Effect, Ordering::Release);
    // The plugin's calls from outside the host's calls: served on a thread of the stand-in's own.
    let l = link.clone();
    let callbacks = std::thread::Builder::new().name("vst2-bridge".into()).spawn(move || {
        let ch = &l.chans[Chan::Callback as usize];
        while let Ok(0) = wait_any(&[ch.mine(), l.stop, l.process], Pump::None, None) {
            let (h, payload) = ch.read();
            let (r, out) = l.serve_callback(&h, payload);
            ch.reply(r, &out);
        }
    });
    unsafe { (*p).callbacks = callbacks.ok() };
    Ok(p as *mut Effect)
}

unsafe extern "C" fn p_control(e: *mut Effect, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize {
    let px = proxy(e);
    let r = unsafe { dispatch(px, opcode, index, value, ptr, opt) };
    if opcode == op::CLOSE {
        let link = px.link.clone();
        link.effect.store(std::ptr::null_mut(), Ordering::Release);
        unsafe { SetEvent(link.stop) };
        if let Some(t) = px.callbacks.take() {
            let _ = t.join();
        }
        drop(unsafe { Box::from_raw(e as *mut Proxy) });
    }
    r
}

unsafe fn dispatch(px: &mut Proxy, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize {
    let mut req = Header { call: call::DISPATCH, opcode, index, value: value as i64, opt, ..Header::default() };
    let mut payload: Vec<u8> = Vec::new();
    unsafe {
        if proto::in_text(opcode) && !ptr.is_null() {
            payload = CStr::from_ptr(ptr as *const std::ffi::c_char).to_bytes().to_vec();
        } else if opcode == op::EDITOR_OPEN {
            req.value = ptr as i64;
        } else if opcode == op::SET_CHUNK && !ptr.is_null() && value > 0 {
            payload = std::slice::from_raw_parts(ptr as *const u8, value as usize).to_vec();
        } else if opcode == op::EVENTS && !ptr.is_null() {
            // The host's list: a 32-bit count, a pointer-sized reserved field, the pointers.
            let count = (*(ptr as *const i32)).max(0) as usize;
            let slots = ptr as *const *const MidiEvent;
            for i in 0..count {
                let ev = *slots.add(2 + i);
                if !ev.is_null() && (*ev).kind == crate::abi::EVENT_MIDI {
                    payload.extend_from_slice(std::slice::from_raw_parts(ev as *const u8, proto::MIDI_EVENT_SIZE));
                }
            }
        }
    }
    let link = px.link.clone();
    if !reads_only(opcode) {
        link.params_changed();
    }
    let c = link.chan();
    let timeout = if c == Chan::Audio { AUDIO_TIMEOUT } else { CONTROL_TIMEOUT };
    let Some((r, out)) = link.call(c, req, &payload, timeout, true) else {
        if opcode == op::EDITOR_GET_RECT && !ptr.is_null() {
            unsafe { *(ptr as *mut *mut Rect) = std::ptr::null_mut() };
        }
        return 0;
    };
    set_fields(&mut px.effect, &r.fields);
    unsafe {
        if proto::out_text(opcode) && !ptr.is_null() {
            proto::copy_out_text(ptr as *mut u8, proto::OUT_TEXT_CAP, &out);
        } else if let Some(n) = proto::out_struct(opcode).filter(|_| !ptr.is_null()) {
            std::ptr::copy_nonoverlapping(out.as_ptr(), ptr as *mut u8, out.len().min(n));
        } else if opcode == op::EDITOR_GET_RECT && !ptr.is_null() {
            if out.len() >= 8 {
                let v = |i: usize| i16::from_le_bytes([out[2 * i], out[2 * i + 1]]);
                px.rect = Rect { top: v(0), left: v(1), bottom: v(2), right: v(3) };
                *(ptr as *mut *mut Rect) = &mut px.rect;
            } else {
                *(ptr as *mut *mut Rect) = std::ptr::null_mut();
            }
        } else if opcode == op::GET_CHUNK && !ptr.is_null() {
            px.chunk = out;
            *(ptr as *mut *mut c_void) = if px.chunk.is_empty() { std::ptr::null_mut() } else { px.chunk.as_mut_ptr().cast() };
            return px.chunk.len() as isize;
        }
    }
    r.ret as isize
}

/// Op-codes that leave the plugin's parameter values as they are (any other may change them: a program, a state, the
/// plugin opening). MIDI events can change them too, but come with every block: their changes show within
/// `PARAMS_FRESH`.
fn reads_only(opcode: i32) -> bool {
    matches!(
        opcode,
        op::GET_PROGRAM
            | op::GET_PROGRAM_NAME
            | op::PARAM_LABEL
            | op::PARAM_DISPLAY
            | op::PARAM_NAME
            | op::EDITOR_GET_RECT
            | op::EDITOR_IDLE
            | op::GET_CHUNK
            | op::EVENTS
            | op::PARAM_AUTOMATABLE
            | op::CATEGORY
            | op::EFFECT_NAME
            | op::VENDOR_NAME
            | op::PRODUCT_NAME
            | op::VENDOR_VERSION
            | op::SUPPORTS
            | op::TAIL_SAMPLES
            | op::OUTPUT_PROPERTIES
            | op::PARAM_PROPERTIES
            | op::VST_VERSION
            | op::PROCESS_BEGIN
            | op::PROCESS_END
    )
}

/// Processing: the inputs go to the helper, its outputs come back (replacing, or added for the accumulating call).
unsafe fn process(e: *mut Effect, inputs: *const *const f32, outputs: *mut *mut f32, frames: i32, replacing: bool) {
    let px = proxy(e);
    let link = px.link.clone();
    link.audio_thread.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    let n = frames.max(0) as usize;
    let ins = px.effect.num_inputs.clamp(0, 64) as usize;
    let outs = px.effect.num_outputs.clamp(0, 64) as usize;
    let mut payload = Vec::with_capacity(ins * n * 4);
    for c in 0..ins {
        let src = unsafe { *inputs.add(c) };
        if src.is_null() {
            payload.resize(payload.len() + n * 4, 0);
        } else {
            payload.extend_from_slice(unsafe { std::slice::from_raw_parts(src as *const u8, n * 4) });
        }
    }
    let req = Header { call: call::PROCESS, index: n as i32, value: (ins | outs << 16) as i64, ..Header::default() };
    let got = link.call(Chan::Audio, req, &payload, AUDIO_TIMEOUT, true);
    for c in 0..outs {
        let dst = unsafe { *outputs.add(c) };
        if dst.is_null() {
            continue;
        }
        let out = unsafe { std::slice::from_raw_parts_mut(dst, n) };
        let from = got.as_ref().and_then(|(_, d)| d.get(c * n * 4..(c + 1) * n * 4));
        for (i, o) in out.iter_mut().enumerate() {
            let v = from.map_or(0.0, |b| f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]));
            if replacing { *o = v } else { *o += v }
        }
    }
    if let Some((r, _)) = got {
        set_fields(&mut px.effect, &r.fields);
    }
}

unsafe extern "C" fn p_process_float(e: *mut Effect, inputs: *const *const f32, outputs: *mut *mut f32, frames: i32) {
    unsafe { process(e, inputs, outputs, frames, true) }
}

/// The accumulating call adds the plugin's output to the outputs.
unsafe extern "C" fn p_process(e: *mut Effect, inputs: *const *const f32, outputs: *mut *mut f32, frames: i32) {
    unsafe { process(e, inputs, outputs, frames, false) }
}

unsafe extern "C" fn p_set(e: *mut Effect, index: i32, value: f32) {
    let link = proxy(e).link.clone();
    let c = link.chan();
    let req = Header { call: call::SET_PARAM, index, opt: value, ..Header::default() };
    let _ = link.call(c, req, &[], if c == Chan::Audio { AUDIO_TIMEOUT } else { CONTROL_TIMEOUT }, false);
    // Read back from the plugin next time: it may store the value otherwise (a stepped parameter).
    link.params_changed();
}

/// A parameter's value: from the host's copy while it stands, else all values read at once.
unsafe extern "C" fn p_get(e: *mut Effect, index: i32) -> f32 {
    let px = proxy(e);
    let link = px.link.clone();
    if let Some(v) = link.params.lock().ok().and_then(|p| p.fresh(index)) {
        return v;
    }
    let c = link.chan();
    let timeout = if c == Chan::Audio { AUDIO_TIMEOUT } else { CONTROL_TIMEOUT };
    let n = px.effect.num_params.max(0);
    let generation = link.params.lock().map_or(0, |p| p.generation);
    let req = Header { call: call::GET_PARAMS, index: n, ..Header::default() };
    match link.call(c, req, &[], timeout, true) {
        Some((r, out)) => {
            set_fields(&mut px.effect, &r.fields);
            let values: Vec<f32> = out.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
            let v = usize::try_from(index).ok().and_then(|i| values.get(i).copied());
            if let Ok(mut p) = link.params.lock() {
                let until = (p.generation == generation).then(|| Instant::now() + PARAMS_FRESH);
                p.values = values;
                p.until = until;
            }
            v.unwrap_or(0.0)
        }
        // The helper has gone, or this thread is inside a call on that channel: the last values read.
        None => link.params.lock().ok().and_then(|p| usize::try_from(index).ok().and_then(|i| p.values.get(i).copied())).unwrap_or(0.0),
    }
}
