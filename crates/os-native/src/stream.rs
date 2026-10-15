// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! A minimal in-memory COM `IStream`, for plugin state save/restore.
//!
// spec: plugins/fruity-abi.md (SaveRestoreState takes an IStream)

use std::ffi::c_void;

const S_OK: i32 = 0;
const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
const STG_E_INVALIDFUNCTION: i32 = 0x8003_0001u32 as i32;

#[repr(C)]
struct StreamVtbl {
    query_interface: unsafe extern "system" fn(*mut MemStream, *const c_void, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut MemStream) -> u32,
    release: unsafe extern "system" fn(*mut MemStream) -> u32,
    read: unsafe extern "system" fn(*mut MemStream, *mut c_void, u32, *mut u32) -> i32,
    write: unsafe extern "system" fn(*mut MemStream, *const c_void, u32, *mut u32) -> i32,
    seek: unsafe extern "system" fn(*mut MemStream, i64, u32, *mut u64) -> i32,
    set_size: unsafe extern "system" fn(*mut MemStream, u64) -> i32,
    copy_to: unsafe extern "system" fn(*mut MemStream) -> i32,
    commit: unsafe extern "system" fn(*mut MemStream, u32) -> i32,
    revert: unsafe extern "system" fn(*mut MemStream) -> i32,
    lock_region: unsafe extern "system" fn(*mut MemStream) -> i32,
    unlock_region: unsafe extern "system" fn(*mut MemStream) -> i32,
    stat: unsafe extern "system" fn(*mut MemStream, *mut c_void, u32) -> i32,
    clone: unsafe extern "system" fn(*mut MemStream, *mut *mut c_void) -> i32,
}

/// COM-compatible stream over a byte vector. Owned by Rust; refcounting is tracked but the
/// object is freed by its Rust owner.
#[repr(C)]
pub struct MemStream {
    vtbl: *const StreamVtbl,
    refs: u32,
    pub data: Vec<u8>,
    pub pos: usize,
}

impl MemStream {
    pub fn new(data: Vec<u8>) -> Box<MemStream> {
        Box::new(MemStream { vtbl: &VTBL, refs: 1, data, pos: 0 })
    }

    pub fn as_ptr(&mut self) -> *mut c_void {
        self as *mut MemStream as *mut c_void
    }
}

unsafe extern "system" fn query_interface(_s: *mut MemStream, _iid: *const c_void, out: *mut *mut c_void) -> i32 {
    if !out.is_null() {
        *out = std::ptr::null_mut();
    }
    E_NOINTERFACE
}
unsafe extern "system" fn add_ref(s: *mut MemStream) -> u32 {
    (*s).refs += 1;
    (*s).refs
}
unsafe extern "system" fn release(s: *mut MemStream) -> u32 {
    (*s).refs = (*s).refs.saturating_sub(1);
    (*s).refs
}
unsafe extern "system" fn read(s: *mut MemStream, buf: *mut c_void, cb: u32, read: *mut u32) -> i32 {
    let s = &mut *s;
    let n = (cb as usize).min(s.data.len().saturating_sub(s.pos));
    std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buf as *mut u8, n);
    s.pos += n;
    if !read.is_null() {
        *read = n as u32;
    }
    S_OK
}
unsafe extern "system" fn write(s: *mut MemStream, buf: *const c_void, cb: u32, written: *mut u32) -> i32 {
    let s = &mut *s;
    let n = cb as usize;
    if s.data.len() < s.pos + n {
        s.data.resize(s.pos + n, 0);
    }
    std::ptr::copy_nonoverlapping(buf as *const u8, s.data.as_mut_ptr().add(s.pos), n);
    s.pos += n;
    if !written.is_null() {
        *written = cb;
    }
    S_OK
}
unsafe extern "system" fn seek(s: *mut MemStream, offset: i64, origin: u32, new_pos: *mut u64) -> i32 {
    let s = &mut *s;
    let base = match origin {
        0 => 0i64,
        1 => s.pos as i64,
        2 => s.data.len() as i64,
        _ => return STG_E_INVALIDFUNCTION,
    };
    let p = base + offset;
    if p < 0 {
        return STG_E_INVALIDFUNCTION;
    }
    s.pos = p as usize;
    if !new_pos.is_null() {
        *new_pos = p as u64;
    }
    S_OK
}
unsafe extern "system" fn set_size(s: *mut MemStream, size: u64) -> i32 {
    (*s).data.resize(size as usize, 0);
    S_OK
}
unsafe extern "system" fn not_impl(_s: *mut MemStream) -> i32 {
    E_NOTIMPL
}
unsafe extern "system" fn commit(_s: *mut MemStream, _f: u32) -> i32 {
    S_OK
}
unsafe extern "system" fn stat(s: *mut MemStream, out: *mut c_void, _flag: u32) -> i32 {
    // STATSTG: pwcsName (ptr), type (u32), cbSize (u64 at offset 16 on x64) …
    if !out.is_null() {
        std::ptr::write_bytes(out as *mut u8, 0, 80);
        *(out as *mut u8).add(8).cast::<u32>() = 2; // STGTY_STREAM
        *(out as *mut u8).add(16).cast::<u64>() = (*s).data.len() as u64;
    }
    S_OK
}
unsafe extern "system" fn clone(_s: *mut MemStream, out: *mut *mut c_void) -> i32 {
    if !out.is_null() {
        *out = std::ptr::null_mut();
    }
    E_NOTIMPL
}

static VTBL: StreamVtbl = StreamVtbl {
    query_interface,
    add_ref,
    release,
    read,
    write,
    seek,
    set_size,
    copy_to: not_impl,
    commit,
    revert: not_impl,
    lock_region: not_impl,
    unlock_region: not_impl,
    stat,
    clone,
};
