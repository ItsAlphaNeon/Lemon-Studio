// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The host object handed to FL native plugins.
//!
//! Implements the documented host methods that matter for loading and rendering; every other
//! slot (including extra slots past the documented 42) is a stub that records its index the first
//! time it is called, so undocumented calls can be discovered by observation.
//!
// spec: plugins/fruity-abi.md

use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CString};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::abi::{IntPtr, VoiceParams, HOST_EXTRA_SLOTS, HOST_METHODS};
use crate::lock::PluginLock;

/// A hook the host consults before its own handling of a `Dispatcher` call (plugin -> host),
/// so experimental code can answer undocumented dispatcher IDs the published SDK does not cover
/// (see "Unknown host slots" in the spec). Returns `Some(result)` to answer, `None` to fall
/// through to the built-in handling. `(id, index, value)`.
pub type DispatchHook = fn(IntPtr, IntPtr, IntPtr) -> Option<IntPtr>;

/// Function pointer stored as a `usize` (0 = none). A plain pointer, so no allocation or lock
/// on the hot dispatcher path.
static DISPATCH_HOOK: AtomicUsize = AtomicUsize::new(0);

/// Installs (or clears, with `None`) the global dispatcher hook. Process-wide.
pub fn set_dispatch_hook(hook: Option<DispatchHook>) {
    DISPATCH_HOOK.store(hook.map_or(0, |f| f as usize), Ordering::SeqCst);
}

fn dispatch_hook() -> Option<DispatchHook> {
    let p = DISPATCH_HOOK.load(Ordering::Relaxed);
    (p != 0).then(|| unsafe { std::mem::transmute::<usize, DispatchHook>(p) })
}

const SLOTS: usize = HOST_METHODS + HOST_EXTRA_SLOTS;
const WAVETABLE_LEN: usize = 16384;

/// The C++-compatible host object. Plugins only touch the documented leading fields;
/// `ctx` is ours.
#[repr(C)]
pub struct HostObject {
    vtable: *const VTable,
    pub host_version: i32,
    pub flags: i32,
    pub app_handle: *mut c_void,
    pub wave_tables: [*mut f32; 10],
    pub temp_buffers: [*mut f32; 4],
    pub reserved: [i32; 30],
    pub ctx: *mut HostContext,
}

/// What a generator asked of its voices during a call, answered by the host once the call has returned (spec:
/// "Voices"), and what the host tells it about them.
#[derive(Default)]
pub struct VoiceBook {
    /// Voices the plugin has finished (host `Voice_Kill`): the tag, and whether the host frees the voice with the
    /// plugin's own `Voice_Kill` (the call's KillHandle; false = the plugin freed it already).
    pub kill: Vec<(IntPtr, bool)>,
    /// Tags of voices the plugin wants released (host `Voice_Release`).
    pub release: Vec<IntPtr>,
    /// Note-on velocity (0..1) of each live voice, by tag (`FPV_GetVelocity`).
    pub velocity: Vec<(IntPtr, f32)>,
}

/// Host-side state shared with the callbacks.
pub struct HostContext {
    pub prog_path: CString,
    pub sample_rate: u32,
    /// Voice requests and voice facts. A lock, because a plugin may call from its own threads.
    pub voices: Mutex<VoiceBook>,
    /// Calls the host couldn't serve meaningfully: (slot or dispatcher id) → count.
    pub unhandled: BTreeMap<String, u32>,
    /// Render lock behind LockMix / LockPlugin / SuspendOutput (see `lock`).
    pub lock: PluginLock,
    /// The parameter count the plugin set with host dispatcher 27 SetNumParams; -1 until it does.
    pub num_params: AtomicIsize,
    _wavetables: Vec<Vec<f32>>,
    _temp: Vec<f32>,
}

/// Global log of unknown/unhandled calls (callbacks can't always reach a context).
pub static CALL_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Set `OS_NATIVE_TRACE=1` to print every logged host call immediately.
pub fn trace_enabled() -> bool {
    std::env::var_os("OS_NATIVE_TRACE").is_some()
}

fn log_once(entry: String) {
    if trace_enabled() {
        eprintln!("[host] {entry}");
    }
    if let Ok(mut l) = CALL_LOG.lock() {
        if !l.contains(&entry) {
            l.push(entry);
        }
    }
}

fn ctx<'a>(this: *mut HostObject) -> Option<&'a mut HostContext> {
    unsafe { this.as_ref().and_then(|h| h.ctx.as_mut()) }
}

impl HostObject {
    /// Builds a host. `prog_path` is the FL install folder (returned for "GetProgPath").
    pub fn new(prog_path: &str, sample_rate: u32, max_block: usize) -> Box<HostObject> {
        let mut tables: Vec<Vec<f32>> = (0..6).map(|_| vec![0.0; WAVETABLE_LEN]).collect();
        let mut seed = 0x1234_5678u32;
        for i in 0..WAVETABLE_LEN {
            let ph = i as f32 / WAVETABLE_LEN as f32;
            tables[0][i] = (ph * std::f32::consts::TAU).sin();
            tables[1][i] = 1.0 - 4.0 * (ph - 0.5).abs(); // triangle
            tables[2][i] = if ph < 0.5 { 1.0 } else { -1.0 };
            tables[3][i] = 2.0 * ph - 1.0;
            tables[4][i] = 2.0 * ph - 1.0;
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            tables[5][i] = seed as f32 / u32::MAX as f32 * 2.0 - 1.0;
        }
        let mut temp = vec![0.0f32; max_block * 2 * 4];
        let mut wave_tables = [std::ptr::null_mut(); 10];
        for (slot, t) in wave_tables.iter_mut().zip(tables.iter_mut()) {
            *slot = t.as_mut_ptr();
        }
        let base = temp.as_mut_ptr();
        let temp_buffers = [0, 1, 2, 3].map(|k| unsafe { base.add(k * max_block * 2) });
        let ctx = Box::into_raw(Box::new(HostContext {
            prog_path: CString::new(prog_path).unwrap_or_default(),
            sample_rate,
            voices: Mutex::new(VoiceBook::default()),
            unhandled: BTreeMap::new(),
            lock: PluginLock::new(),
            num_params: AtomicIsize::new(-1),
            _wavetables: tables,
            _temp: temp,
        }));
        Box::new(HostObject {
            vtable: &VTABLE,
            host_version: 26_01_07_00,
            flags: 0,
            app_handle: std::ptr::null_mut(),
            wave_tables,
            temp_buffers,
            reserved: [0; 30],
            ctx,
        })
    }

    pub fn context(&mut self) -> &mut HostContext {
        unsafe { &mut *self.ctx }
    }

    /// The plugin's render lock (lives as long as the host object).
    pub fn lock(&self) -> &PluginLock {
        unsafe { &(*self.ctx).lock }
    }

    /// The parameter count the plugin set later (host dispatcher 27), if it did.
    pub fn num_params(&self) -> Option<i32> {
        let n = unsafe { &(*self.ctx).num_params }.load(Ordering::Relaxed);
        (n >= 0).then(|| n.min(i32::MAX as isize) as i32)
    }

    /// The voice requests and facts (lives as long as the host object).
    pub fn voices(&self) -> std::sync::MutexGuard<'_, VoiceBook> {
        let book = unsafe { &(*self.ctx).voices };
        book.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for HostObject {
    fn drop(&mut self) {
        if !self.ctx.is_null() {
            unsafe { drop(Box::from_raw(self.ctx)) };
        }
    }
}

// ---------------------------------------------------------------------------------------
// Implemented methods

unsafe extern "system" fn dispatcher(this: *mut HostObject, _sender: IntPtr, id: IntPtr, index: IntPtr, value: IntPtr) -> IntPtr {
    if let Some(hook) = dispatch_hook() {
        if let Some(r) = hook(id, index, value) {
            return r;
        }
    }
    match id {
        // GetProgPath / GetProjDataPath: data path
        29 | 47 => ctx(this).map_or(0, |c| c.prog_path.as_ptr() as IntPtr),
        // LocateDataFile / PackDataFile: give the name back unchanged
        14 | 28 => value,
        // GetMixingTime / GetPlaybackTime / GetSelTime: zero time (no selection)
        36 | 37 | 38 => {
            if let Some(t) = (value as *mut [f64; 2]).as_mut() {
                *t = [0.0, 0.0];
            }
            0
        }
        // The undocumented context object a plugin requests during construction: hand back the
        // stand-in (spec plugins/fruity-abi.md, "Host dispatcher ID 71"). Without this the plugin
        // crashes; with it, effects host and render.
        71 => crate::stub::context_object(),
        // SetNumParams: the plugin's parameter count from now on (spec: "Parameters"); answered 0 as before.
        27 => {
            if let Some(c) = ctx(this) {
                c.num_params.store(value.max(0), Ordering::Relaxed);
            }
            0
        }
        // Harmless notifications observed from FL 26 plugins (answered 0 with no ill effect).
        // 2 EditorResized: the host polls the editor's size on its UI tick instead.
        // 22 OnHint_Direct, 24 GetInstance (none).
        2 | 3 | 5 | 10 | 13 | 22 | 23 | 24 | 30 | 33 | 35 | 46 | 48 | 50 | 72 | 86 => 0,
        _ => {
            log_once(format!("host dispatcher id {id} (index {index}, value {value})"));
            if let Some(c) = ctx(this) {
                *c.unhandled.entry(format!("dispatcher {id}")).or_default() += 1;
            }
            0
        }
    }
}

fn book<'a>(this: *mut HostObject) -> Option<std::sync::MutexGuard<'a, VoiceBook>> {
    ctx(this).map(|c| c.voices.lock().unwrap_or_else(|e| e.into_inner()))
}

// Voice_Release / Voice_Kill (Sender = the voice's tag): noted here, answered by the host after the plugin's call
// returns, so the plugin's voice list never changes under its own feet (spec: "Voices").
unsafe extern "system" fn voice_release(this: *mut HostObject, sender: IntPtr) {
    if trace_enabled() {
        eprintln!("[host] Voice_Release tag {sender}");
    }
    if let Some(mut b) = book(this) {
        if !b.release.contains(&sender) {
            b.release.push(sender);
        }
    }
}

unsafe extern "system" fn voice_kill(this: *mut HostObject, sender: IntPtr, kill_handle: i32) {
    if trace_enabled() {
        eprintln!("[host] Voice_Kill tag {sender} kill_handle {kill_handle:#x}");
    }
    // KillHandle is a 32-bit boolean from some plugins (1 or -1) and a byte from others: only its low byte counts.
    let free = kill_handle & 0xFF != 0;
    if let Some(mut b) = book(this) {
        if !b.kill.iter().any(|k| k.0 == sender) {
            b.kill.push((sender, free));
        }
    }
}

/// Voice_ProcessEvent (public SDK ids): 1 GetLength, 2 GetColor, 3 GetVelocity, 4 GetRelVelocity, 5 GetRelTime (floats
/// as their bits), 6 SetLinkVelocity.
unsafe extern "system" fn voice_process_event(this: *mut HostObject, sender: IntPtr, id: IntPtr, _v: IntPtr, _f: IntPtr) -> IntPtr {
    let bits = |x: f32| x.to_bits() as IntPtr;
    match id {
        1 => -1, // length in ticks: undefined (notes are released by the host)
        2 => 0,  // colour 0
        3 => {
            let vel = book(this).and_then(|b| b.velocity.iter().find(|v| v.0 == sender).map(|v| v.1));
            bits(vel.unwrap_or(DEFAULT_VELOCITY))
        }
        4 => bits(DEFAULT_RELEASE_VELOCITY),
        5 => bits(1.0),
        _ => 0,
    }
}

/// A note's default velocity (100 of 128) and release velocity (64 of 128), as the plugin sees them.
pub const DEFAULT_VELOCITY: f32 = 100.0 / 128.0;
const DEFAULT_RELEASE_VELOCITY: f32 = 0.5;

/// Adds a mono or stereo source into a stereo destination, ramping from the last levels.
unsafe fn add_wave(stereo_src: bool, src: *const f32, dst: *mut f32, len: i32, lvol: f32, rvol: f32, last_l: *mut f32, last_r: *mut f32) {
    if src.is_null() || dst.is_null() || len <= 0 {
        return;
    }
    let n = len as usize;
    let (l0, r0) = (*last_l, *last_r);
    for i in 0..n {
        let t = (i + 1) as f32 / n as f32;
        let gl = l0 + (lvol - l0) * t;
        let gr = r0 + (rvol - r0) * t;
        let (sl, sr) = if stereo_src { (*src.add(2 * i), *src.add(2 * i + 1)) } else { (*src.add(i), *src.add(i)) };
        *dst.add(2 * i) += sl * gl;
        *dst.add(2 * i + 1) += sr * gr;
    }
    *last_l = lvol;
    *last_r = rvol;
}

unsafe extern "system" fn add_wave_mono(_this: *mut HostObject, src: *const f32, dst: *mut f32, len: i32, l: f32, r: f32, ll: *mut f32, lr: *mut f32) {
    add_wave(false, src, dst, len, l, r, ll, lr);
}
unsafe extern "system" fn add_wave_stereo(_this: *mut HostObject, src: *const f32, dst: *mut f32, len: i32, l: f32, r: f32, ll: *mut f32, lr: *mut f32) {
    add_wave(true, src, dst, len, l, r, ll, lr);
}

// LockMix / UnlockMix (no sender), SuspendOutput / ResumeOutput, LockPlugin / UnlockPlugin (sender): this host
// keeps one lock per plugin instance and holds it around every render call, so a plugin locking from its editor
// keeps its rendering out until it unlocks.
unsafe extern "system" fn lock_enter(this: *mut HostObject) {
    if let Some(c) = ctx(this) {
        c.lock.enter();
    }
}
unsafe extern "system" fn lock_leave(this: *mut HostObject) {
    if let Some(c) = ctx(this) {
        c.lock.leave();
    }
}

unsafe extern "system" fn time_f64(_this: *mut HostObject) -> f64 {
    0.0
}

/// ComputeLRVol: the host's pan split of `vol` at `pan` (spec: "Voices"; `audio/conventions.md` "Pan split"): centred,
/// each side is `vol * 0.7071`.
unsafe extern "system" fn compute_lr_vol(_this: *mut HostObject, l: *mut f32, r: *mut f32, pan: f32, vol: f32) {
    if let (Some(l), Some(r)) = (l.as_mut(), r.as_mut()) {
        (*l, *r) = os_dsp::level::pan_split(vol, pan);
    }
}

unsafe extern "system" fn compute_lr_vol_old(this: *mut HostObject, l: *mut f32, r: *mut f32, pan: i32, vol: f32) {
    compute_lr_vol(this, l, r, pan as f32 / 64.0, vol);
}

unsafe extern "system" fn trigger_output_voice(_this: *mut HostObject, _p: *mut VoiceParams, _i: IntPtr, _t: IntPtr) -> IntPtr {
    log_once("host TriggerOutputVoice (output voices not supported)".into());
    -1
}

#[repr(C)]
struct IoBuffer {
    buffer: *mut c_void,
    flags: u32,
}

unsafe extern "system" fn get_io_buffer(_this: *mut HostObject, _sender: IntPtr, _index: IntPtr, b: *mut IoBuffer) {
    if let Some(b) = b.as_mut() {
        b.buffer = std::ptr::null_mut();
        b.flags = 0;
    }
}

// ---------------------------------------------------------------------------------------
// Stubs: one distinct function per slot so the slot index can be logged.

macro_rules! stubs {
    ($($n:literal),*) => {
        const STUBS: [*const c_void; SLOTS] = [$({
            unsafe extern "system" fn s(_this: *mut HostObject) -> IntPtr {
                log_once(format!("host slot {}", $n));
                0
            }
            s as *const c_void
        }),*];
    };
}
stubs!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29,
       30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59,
       60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89,
       90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105);

/// No-op for documented methods that need no action (locks, MIDI out, hints, …).
unsafe extern "system" fn noop(_this: *mut HostObject) -> IntPtr {
    0
}

const fn build_vtable() -> [*const c_void; SLOTS] {
    let mut v = STUBS;
    v[0] = dispatcher as *const c_void;
    v[1] = noop as *const c_void; // OnParamChanged
    v[2] = noop as *const c_void; // OnHint
    v[3] = compute_lr_vol_old as *const c_void;
    v[4] = voice_release as *const c_void;
    v[5] = voice_kill as *const c_void;
    v[6] = voice_process_event as *const c_void;
    v[7] = lock_enter as *const c_void; // LockMix
    v[8] = lock_leave as *const c_void; // UnlockMix
    v[9] = noop as *const c_void; // MIDIOut_Delayed
    v[10] = noop as *const c_void; // MIDIOut
    v[11] = add_wave_mono as *const c_void;
    v[12] = add_wave_stereo as *const c_void;
    v[16] = noop as *const c_void; // GetSongMixingTime (int 0)
    v[17] = time_f64 as *const c_void;
    v[18] = time_f64 as *const c_void;
    v[19] = noop as *const c_void; // OnControllerChanged
    v[21] = noop as *const c_void; // PlugMsg_Delayed
    v[22] = noop as *const c_void; // PlugMsg_Kill
    v[28] = lock_enter as *const c_void; // SuspendOutput
    v[29] = lock_leave as *const c_void; // ResumeOutput
    v[31] = compute_lr_vol as *const c_void;
    v[32] = lock_enter as *const c_void; // LockPlugin (sender ignored: one host object per plugin)
    v[33] = lock_leave as *const c_void; // UnlockPlugin
    v[34] = noop as *const c_void; // LockMix_Shared_Old
    v[35] = noop as *const c_void; // UnlockMix_Shared_Old
    v[36] = get_io_buffer as *const c_void;
    v[37] = get_io_buffer as *const c_void;
    v[38] = trigger_output_voice as *const c_void;
    v
}

#[repr(transparent)]
pub struct VTable([*const c_void; SLOTS]);
// Function pointers only; safe to share between threads.
unsafe impl Sync for VTable {}
static VTABLE: VTable = VTable(build_vtable());

/// Returns the logged calls and clears the log.
pub fn take_call_log() -> Vec<String> {
    CALL_LOG.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default()
}

/// Exposed for diagnostics: the C string for a char pointer.
pub fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}
