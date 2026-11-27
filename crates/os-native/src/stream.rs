// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A minimal in-memory COM `IStream`, for plugin state save/restore.
//!
// spec: plugins/fruity-abi.md (SaveRestoreState takes an IStream)

use std::ffi::c_void;

const S_OK: i32 = 0;
const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;
const E_NOINTERFACE: i32 = 0x8000_4002u32 as i32;
const E_POINTER: i32 = 0x8000_4003u32 as i32;
const E_OUTOFMEMORY: i32 = 0x8007_000Eu32 as i32;
const STG_E_INVALIDFUNCTION: i32 = 0x8003_0001u32 as i32;

/// Upper bound on an in-memory plugin state stream (256 MiB).
const MAX_STREAM_BYTES: usize = 256 * 1024 * 1024;

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
    if s.is_null() {
        return 0;
    }
    (*s).refs = (*s).refs.saturating_add(1);
    (*s).refs
}
unsafe extern "system" fn release(s: *mut MemStream) -> u32 {
    if s.is_null() {
        return 0;
    }
    (*s).refs = (*s).refs.saturating_sub(1);
    (*s).refs
}
unsafe extern "system" fn read(s: *mut MemStream, buf: *mut c_void, cb: u32, read: *mut u32) -> i32 {
    if s.is_null() || (cb > 0 && buf.is_null()) {
        return E_POINTER;
    }
    let s = &mut *s;
    let n = (cb as usize).min(s.data.len().saturating_sub(s.pos));
    if n > 0 {
        std::ptr::copy_nonoverlapping(s.data.as_ptr().add(s.pos), buf as *mut u8, n);
    }
    s.pos += n;
    if !read.is_null() {
        *read = n as u32;
    }
    S_OK
}
unsafe extern "system" fn write(s: *mut MemStream, buf: *const c_void, cb: u32, written: *mut u32) -> i32 {
    if s.is_null() || (cb > 0 && buf.is_null()) {
        return E_POINTER;
    }
    let s = &mut *s;
    let n = cb as usize;
    let Some(end) = s.pos.checked_add(n).filter(|&end| end <= MAX_STREAM_BYTES) else {
        return E_OUTOFMEMORY;
    };
    if s.data.len() < end {
        s.data.resize(end, 0);
    }
    if n > 0 {
        std::ptr::copy_nonoverlapping(buf as *const u8, s.data.as_mut_ptr().add(s.pos), n);
    }
    s.pos = end;
    if !written.is_null() {
        *written = cb;
    }
    S_OK
}
unsafe extern "system" fn seek(s: *mut MemStream, offset: i64, origin: u32, new_pos: *mut u64) -> i32 {
    if s.is_null() {
        return E_POINTER;
    }
    let s = &mut *s;
    let base = match origin {
        0 => Ok(0i64),
        1 => i64::try_from(s.pos),
        2 => i64::try_from(s.data.len()),
        _ => return STG_E_INVALIDFUNCTION,
    };
    let Ok(base) = base else { return STG_E_INVALIDFUNCTION };
    let Some(p) = base.checked_add(offset) else { return STG_E_INVALIDFUNCTION };
    if p < 0 {
        return STG_E_INVALIDFUNCTION;
    }
    let Ok(pos) = usize::try_from(p) else { return STG_E_INVALIDFUNCTION };
    s.pos = pos;
    if !new_pos.is_null() {
        *new_pos = p as u64;
    }
    S_OK
}
unsafe extern "system" fn set_size(s: *mut MemStream, size: u64) -> i32 {
    if s.is_null() {
        return E_POINTER;
    }
    let Ok(n) = usize::try_from(size) else { return E_OUTOFMEMORY };
    if n > MAX_STREAM_BYTES {
        return E_OUTOFMEMORY;
    }
    (*s).data.resize(n, 0);
    S_OK
}
unsafe extern "system" fn not_impl(_s: *mut MemStream) -> i32 {
    E_NOTIMPL
}
unsafe extern "system" fn commit(_s: *mut MemStream, _f: u32) -> i32 {
    S_OK
}
unsafe extern "system" fn stat(s: *mut MemStream, out: *mut c_void, _flag: u32) -> i32 {
    if s.is_null() {
        return E_POINTER;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_stream_rejects_null_overflow_and_huge_sizes() {
        let mut s = MemStream::new(b"hello".to_vec());
        let ptr = &mut *s as *mut MemStream;
        let mut n = 0u32;
        let mut pos = 0u64;
        unsafe {
            assert_eq!(read(std::ptr::null_mut(), std::ptr::null_mut(), 0, std::ptr::null_mut()), E_POINTER);
            assert_eq!(read(ptr, std::ptr::null_mut(), 4, &mut n), E_POINTER);
            assert_eq!(write(ptr, std::ptr::null(), 4, &mut n), E_POINTER);
            // Zero-length read/write with null buffer succeeds without UB.
            assert_eq!(read(ptr, std::ptr::null_mut(), 0, &mut n), S_OK);
            assert_eq!(write(ptr, std::ptr::null(), 0, &mut n), S_OK);
            // Seek overflow and negative seek return STG_E_INVALIDFUNCTION.
            assert_eq!(seek(ptr, -1, 0, &mut pos), STG_E_INVALIDFUNCTION);
            assert_eq!(seek(ptr, 1, 0, &mut pos), S_OK);
            assert_eq!(seek(ptr, i64::MAX, 1, &mut pos), STG_E_INVALIDFUNCTION);
            // Huge set_size and write past MAX_STREAM_BYTES fail cleanly.
            assert_eq!(set_size(ptr, (MAX_STREAM_BYTES as u64) + 1), E_OUTOFMEMORY);
            assert_eq!(seek(ptr, MAX_STREAM_BYTES as i64, 0, &mut pos), S_OK);
            assert_eq!(write(ptr, b"x".as_ptr().cast(), 1, &mut n), E_OUTOFMEMORY);
        }
    }
}
