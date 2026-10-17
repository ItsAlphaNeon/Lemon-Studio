// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A minimal stand-in for the FL-internal context object a native plugin requests through host
//! dispatcher ID 71 during `CreatePlugInstance`.
//!
//! The object's real semantics are undocumented (see spec `plugins/fruity-abi.md`, "Host dispatcher
//! ID 71: the context object"). Observation shows a self-consistent object — every method-table
//! slot returns the object itself, every data field points back at the object — is enough for the
//! plugin to finish construction and (for effects) to render: whatever the plugin pulls out of the
//! object and dereferences, as data or as a sub-object, stays on valid, readable memory with a
//! usable method table. This is an experimental interoperability shim, not a reproduction of FL's
//! object; it carries no state and is shared process-wide.
//
// spec: plugins/fruity-abi.md

use std::sync::OnceLock;

/// Method-table slots. Generous: a slot the plugin reads past this would land outside the table.
const VT_SLOTS: usize = 256;
/// Pointer-sized fields in the object body.
const OBJ_QWORDS: usize = 8192;

/// The process-wide object base address, built once.
static OBJECT: OnceLock<usize> = OnceLock::new();

/// Every method-table slot maps here: ignore the arguments, return the object itself, so the
/// plugin can chain method calls and dereference results without faulting. The x64 Windows ABI
/// passes the first integer arguments in registers and ignores an unexpected return value, so this
/// one signature stands in for methods of any small arity.
unsafe extern "system" fn method(_a: isize, _b: isize, _c: isize, _d: isize) -> isize {
    OBJECT.get().copied().unwrap_or(0) as isize
}

fn build() -> usize {
    let m = method as *const () as usize;
    // Method table of identical logging-free stubs.
    let vt: &'static mut [usize] = Box::leak(vec![m; VT_SLOTS].into_boxed_slice());
    let vt_ptr = vt.as_ptr() as usize;
    // Object body: every field points back at the object; field 0 is the method-table pointer.
    let obj: &'static mut [usize] = Box::leak(vec![0usize; OBJ_QWORDS].into_boxed_slice());
    let obj_ptr = obj.as_mut_ptr() as usize;
    for x in obj.iter_mut() {
        *x = obj_ptr;
    }
    obj[0] = vt_ptr;
    obj_ptr
}

/// Pointer to the stand-in context object, as the `intptr` to return from host dispatcher ID 71.
/// Built on first use; thereafter the same object for every plugin in the process (it is stateless,
/// so sharing is safe).
pub fn context_object() -> isize {
    *OBJECT.get_or_init(build) as isize
}
