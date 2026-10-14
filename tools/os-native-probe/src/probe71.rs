// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Dynamic observation of the undocumented host object that FL native plugins request through
//! host dispatcher ID 71 during `CreatePlugInstance` (see spec `plugins/fruity-abi.md`). Black-box,
//! inside our own process: we hand the plugin a self-consistent sandbox object and record which of
//! its vtable slots the plugin calls and any memory faults, so the object's required shape can be
//! learned without decompiling FL.
//!
//! The sandbox object `O` is a committed block whose every pointer-sized field holds `&O`, except
//! field 0 which holds `&VT` (a table of logging stubs). Whatever the plugin pulls out of the
//! object and dereferences -- as data or as another object -- lands back on `O`, which is readable
//! and has a valid method table; method calls are logged and return `&O`. The graph has a fixed
//! point, so execution continues past the call instead of crashing and we observe what is needed.
#![cfg(windows)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const STATUS_ACCESS_VIOLATION: u32 = 0xC000_0005;

const VT_SLOTS: usize = 256;
const OBJ_QWORDS: usize = 8192; // 64 KiB object

static OBJ_BASE: AtomicUsize = AtomicUsize::new(0);
static OBJ_END: AtomicUsize = AtomicUsize::new(0);
static VT_BASE: AtomicUsize = AtomicUsize::new(0);
static VT_END: AtomicUsize = AtomicUsize::new(0);

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SLOT_HITS: [AtomicUsize; VT_SLOTS] = {
    const Z: AtomicUsize = AtomicUsize::new(0);
    [Z; VT_SLOTS]
};

fn log(entry: String) {
    // Stream live so a crash still shows what the plugin touched before it died.
    eprintln!("[71] {entry}");
    if let Ok(mut l) = LOG.lock() {
        l.push(entry);
    }
}

/// Drains and returns the observation log.
pub fn take_log() -> Vec<String> {
    LOG.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default()
}

/// Shared logging body. x64 Windows passes the first four integer args in registers; reading only
/// the ones we name is safe whatever the real arity, and a returned value is ignored by callers
/// that expected `void`.
fn slot_call(slot: usize, this: isize, a1: isize, a2: isize, a3: isize) -> isize {
    let n = SLOT_HITS[slot].fetch_add(1, Ordering::Relaxed);
    if n < 3 {
        log(format!("vtable slot {slot:>3} call #{n}: this={this:#x} a1={a1:#x} a2={a2:#x} a3={a3:#x}"));
    } else if n == 3 {
        log(format!("vtable slot {slot:>3}: (further calls suppressed)"));
    }
    OBJ_BASE.load(Ordering::Relaxed) as isize
}

macro_rules! make_stubs {
    ($($i:literal),* $(,)?) => {
        [ $( {
            unsafe extern "system" fn s(this: isize, a1: isize, a2: isize, a3: isize) -> isize {
                slot_call($i, this, a1, a2, a3)
            }
            s as *const () as usize
        } ),* ]
    };
}

fn stub_addrs() -> [usize; VT_SLOTS] {
    make_stubs!(0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246,247,248,249,250,251,252,253,254,255)
}

unsafe extern "system" fn veh(info: *mut EXCEPTION_POINTERS) -> i32 {
    let rec = (*info).ExceptionRecord;
    if rec.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if (*rec).ExceptionCode as u32 == STATUS_ACCESS_VIOLATION {
        let op = (*rec).ExceptionInformation[0];
        let addr = (*rec).ExceptionInformation[1] as usize;
        let at = (*rec).ExceptionAddress as usize;
        let kind = match op { 0 => "read", 1 => "write", 8 => "execute", _ => "access" };
        let (obj, obj_end) = (OBJ_BASE.load(Ordering::Relaxed), OBJ_END.load(Ordering::Relaxed));
        let (vt, vt_end) = (VT_BASE.load(Ordering::Relaxed), VT_END.load(Ordering::Relaxed));
        let where_ = if obj != 0 && addr >= obj && addr < obj_end {
            format!(" (object+{:#x})", addr - obj)
        } else if vt != 0 && addr >= vt && addr < vt_end {
            format!(" (vtable+{:#x}, slot {})", addr - vt, (addr - vt) / 8)
        } else {
            String::new()
        };
        log(format!("FAULT: {kind} at {addr:#x}{where_}, ip {at:#x}"));
    }
    EXCEPTION_CONTINUE_SEARCH
}

/// Allocates and wires the sandbox object and vtable, installs the fault handler, and returns the
/// object base to hand back from host dispatcher ID 71. Idempotent.
pub fn install() -> isize {
    let existing = OBJ_BASE.load(Ordering::Relaxed);
    if existing != 0 {
        return existing as isize;
    }
    unsafe {
        AddVectoredExceptionHandler(1, Some(veh));

        let vt = VirtualAlloc(std::ptr::null(), VT_SLOTS * 8, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) as *mut usize;
        assert!(!vt.is_null(), "VirtualAlloc vtable failed");
        let addrs = stub_addrs();
        for (i, a) in addrs.iter().enumerate() {
            *vt.add(i) = *a;
        }
        VT_BASE.store(vt as usize, Ordering::SeqCst);
        VT_END.store(vt as usize + VT_SLOTS * 8, Ordering::SeqCst);

        let obj = VirtualAlloc(std::ptr::null(), OBJ_QWORDS * 8, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) as *mut usize;
        assert!(!obj.is_null(), "VirtualAlloc object failed");
        let obj_base = obj as usize;
        for i in 0..OBJ_QWORDS {
            *obj.add(i) = obj_base;
        }
        *obj = vt as usize; // field 0 = method table
        OBJ_BASE.store(obj_base, Ordering::SeqCst);
        OBJ_END.store(obj_base + OBJ_QWORDS * 8, Ordering::SeqCst);
        obj_base as isize
    }
}

/// Dispatcher hook: answer ID 71 with the sandbox object; fall through otherwise.
pub fn hook(id: isize, _index: isize, _value: isize) -> Option<isize> {
    (id == 71).then(|| OBJ_BASE.load(Ordering::Relaxed) as isize)
}
