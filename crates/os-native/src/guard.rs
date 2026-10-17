// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Calls into a native plugin that cannot take the process down (spec plugins/fruity-abi.md, "Crash containment").
//!
//! Every call the host makes into a native plugin goes through `call`, which runs it under the host's crash guard
//! (`os_host::guard`): an error the plugin raises and does not handle itself (its runtime's exceptions, access
//! violations) comes back as `Err(code)` instead of ending the process, with the runtime's error text in
//! [`last_message`]. Where the guard is not built in, the call is made directly.
//
// spec: plugins/fruity-abi.md

use crate::abi::IntPtr;

pub use os_host::guard::{contained, exit, last_message, tripped, RUNTIME_ERROR};

/// An access violation inside the plugin.
pub const ACCESS_VIOLATION: u32 = 0xC000_0005;

/// A plugin method: up to six integer or pointer arguments (unused trailing ones are ignored by the method). It may
/// unwind: its runtime raises its errors as exceptions.
type Method = unsafe extern "system-unwind" fn(IntPtr, IntPtr, IntPtr, IntPtr, IntPtr, IntPtr) -> IntPtr;

/// Calls plugin method `f` (a function pointer read from the plugin's method table, or its entry point) with up to six
/// integer / pointer arguments. `Err` carries the exception code when the plugin raised one it did not handle (its text
/// in [`last_message`]). A method returning a 32-bit value has it in the low half.
///
/// # Safety
/// `f` must be a plugin function taking at most six integer or pointer arguments, and the arguments must be what it
/// expects.
pub unsafe fn call(f: *const (), args: [IntPtr; 6]) -> Result<IntPtr, u32> {
    // SAFETY: the caller passes a plugin method of this shape.
    let method = unsafe { std::mem::transmute::<*const (), Method>(f) };
    let [a, b, c, d, e, g] = args;
    let r = os_host::guard::catch(|| unsafe { method(a, b, c, d, e, g) });
    if let (Err(code), true) = (&r, std::env::var_os("OS_NATIVE_TRACE").is_some()) {
        eprintln!("[guard] {code:#010X} {:?}", last_message());
    }
    r
}

/// A short description of exception `code` for messages ("plugin error").
pub fn describe(code: u32) -> String {
    match code {
        RUNTIME_ERROR => "plugin error".into(),
        ACCESS_VIOLATION => "memory error".into(),
        c => format!("exception {c:#010X}"),
    }
}

/// `describe` with the plugin's own text when it gave a short one: `plugin error: List index out of bounds (0)`.
pub fn describe_with(code: u32, message: &str) -> String {
    let text = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim_end_matches('.');
    if text.is_empty() || text.chars().count() > 100 {
        describe(code)
    } else {
        format!("{}: {text}", describe(code))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "system" fn add(a: IntPtr, b: IntPtr, c: IntPtr) -> IntPtr {
        a + b + c
    }

    #[test]
    fn calls_with_fewer_arguments_than_six() {
        assert_eq!(unsafe { call(add as *const (), [1, 2, 3, 99, 99, 99]) }, Ok(6));
    }

    #[test]
    fn an_access_violation_comes_back_as_an_error() {
        if !contained() {
            return;
        }
        unsafe extern "system" fn bad(p: IntPtr) -> IntPtr {
            unsafe { std::ptr::read_volatile(p as *const IntPtr) }
        }
        assert_eq!(unsafe { call(bad as *const (), [8, 0, 0, 0, 0, 0]) }, Err(ACCESS_VIOLATION));
    }

    #[cfg(windows)]
    #[test]
    fn a_raised_exception_comes_back_as_an_error() {
        if !contained() {
            return;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn RaiseException(code: u32, flags: u32, n: u32, args: *const usize);
        }
        unsafe extern "system" fn raise() -> IntPtr {
            unsafe { RaiseException(RUNTIME_ERROR, 1, 0, std::ptr::null()) };
            0
        }
        assert_eq!(unsafe { call(raise as *const (), [0; 6]) }, Err(RUNTIME_ERROR));
    }

    /// A runtime error's object: class pointer, then the message as a counted UTF-16 string.
    #[cfg(windows)]
    #[test]
    fn a_runtime_error_gives_its_message() {
        if !contained() {
            return;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn RaiseException(code: u32, flags: u32, n: u32, args: *const usize);
        }
        #[repr(C)]
        struct Counted {
            len: i32,
            text: [u16; 9],
        }
        static TEXT: Counted = Counted { len: 9, text: [b'B' as u16, b'a' as u16, b'd' as u16, b' ' as u16, b'i' as u16, b'n' as u16, b'd' as u16, b'e' as u16, b'x' as u16] };
        static mut OBJ: [usize; 2] = [0, 0];
        unsafe extern "system" fn raise() -> IntPtr {
            unsafe {
                OBJ[1] = TEXT.text.as_ptr() as usize;
                let args = [0usize, std::ptr::addr_of!(OBJ) as usize];
                RaiseException(RUNTIME_ERROR, 1, 2, args.as_ptr());
            }
            0
        }
        assert_eq!(unsafe { call(raise as *const (), [0; 6]) }, Err(RUNTIME_ERROR));
        assert_eq!(last_message(), "Bad index");
        assert_eq!(describe_with(RUNTIME_ERROR, &last_message()), "plugin error: Bad index");
    }
}
