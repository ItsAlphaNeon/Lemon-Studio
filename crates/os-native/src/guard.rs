//! Calls into a plugin that cannot take the process down (spec plugins/fruity-abi.md, "Crash containment").
//!
//! Every call the host makes into a plugin goes through `call`. On Windows MSVC builds it runs under structured
//! exception handling (`guard.c`): an error the plugin raises and does not handle itself (its runtime's exceptions,
//! access violations) comes back as `Err(code)` instead of ending the process. Elsewhere the call is made directly.
//
// spec: plugins/fruity-abi.md

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::abi::IntPtr;

/// The exception code of an error raised by the plugin's own runtime (and not handled by it). Seen when a data file is
/// missing or an unimplemented method is called.
pub const RUNTIME_ERROR: u32 = 0x0EED_FADE;
/// An access violation inside the plugin.
pub const ACCESS_VIOLATION: u32 = 0xC000_0005;

/// A plugin of this process raised an error that was contained.
static TRIPPED: AtomicBool = AtomicBool::new(false);

/// What a contained error was (the layout `guard.c` fills).
#[repr(C)]
struct Fault {
    code: u32,
    address: usize,
    params: [usize; 2],
    /// The text of the plugin runtime's error object (UTF-16, 0-terminated), empty when there is none.
    message: [u16; 256],
}

thread_local! {
    /// The text of the last contained error on this thread (`last_message`).
    static LAST_MESSAGE: RefCell<String> = const { RefCell::new(String::new()) };
}

#[cfg(native_guard)]
unsafe extern "C" {
    fn os_native_guarded_call(
        f: *const (),
        a: IntPtr,
        b: IntPtr,
        c: IntPtr,
        d: IntPtr,
        e: IntPtr,
        g: IntPtr,
        ret: *mut IntPtr,
        fault: *mut Fault,
    ) -> i32;
}

/// Calls plugin method `f` (a function pointer read from the plugin's method table, or its entry point) with up to six
/// integer / pointer arguments; unused trailing arguments are ignored by the method. `Err` carries the exception code
/// when the plugin raised one it did not handle. A method returning a 32-bit value has it in the low half.
///
/// # Safety
/// `f` must be a plugin function taking at most six integer or pointer arguments, and the arguments must be what it
/// expects.
pub unsafe fn call(f: *const (), args: [IntPtr; 6]) -> Result<IntPtr, u32> {
    #[cfg(native_guard)]
    {
        let mut ret = 0;
        let mut fault = Fault { code: 0, address: 0, params: [0; 2], message: [0; 256] };
        let [a, b, c, d, e, g] = args;
        if unsafe { os_native_guarded_call(f, a, b, c, d, e, g, &mut ret, &mut fault) } != 0 {
            Ok(ret)
        } else {
            TRIPPED.store(true, Ordering::Relaxed);
            let len = fault.message.iter().position(|&c| c == 0).unwrap_or(fault.message.len());
            let text = String::from_utf16_lossy(&fault.message[..len]);
            if trace() {
                eprintln!(
                    "[guard] {:#010X} at {:#x} (params {:#x}, {:#x}) {text:?}",
                    fault.code, fault.address, fault.params[0], fault.params[1]
                );
            }
            LAST_MESSAGE.with(|m| *m.borrow_mut() = text);
            Err(fault.code)
        }
    }
    #[cfg(not(native_guard))]
    {
        type Method = unsafe extern "system" fn(IntPtr, IntPtr, IntPtr, IntPtr, IntPtr, IntPtr) -> IntPtr;
        let [a, b, c, d, e, g] = args;
        Ok(unsafe { std::mem::transmute::<*const (), Method>(f)(a, b, c, d, e, g) })
    }
}

/// The text the plugin's runtime gave the last error contained on this thread ("" when it gave none): what the
/// plugin would have shown in its own error box.
pub fn last_message() -> String {
    LAST_MESSAGE.with(|m| m.borrow().clone())
}

fn trace() -> bool {
    std::env::var_os("OS_NATIVE_TRACE").is_some()
}

/// Whether plugin errors are contained in this build.
pub const fn contained() -> bool {
    cfg!(native_guard)
}

/// Whether a plugin of this process raised an error that was contained. That plugin's library stays loaded, in
/// whatever state the error left it, and unloading it as a normal process exit does can crash: see [`exit`].
pub fn tripped() -> bool {
    TRIPPED.load(Ordering::Relaxed)
}

/// Ends the process with `code`. After a contained plugin error ([`tripped`]) the process ends at once, without
/// unloading its libraries (a stopped plugin's code never runs again); otherwise it exits normally.
pub fn exit(code: i32) -> ! {
    #[cfg(windows)]
    if tripped() {
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        unsafe { TerminateProcess(GetCurrentProcess(), code as u32) };
    }
    std::process::exit(code)
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

    #[cfg(native_guard)]
    #[test]
    fn an_access_violation_comes_back_as_an_error() {
        unsafe extern "system" fn bad(p: IntPtr) -> IntPtr {
            unsafe { std::ptr::read_volatile(p as *const IntPtr) }
        }
        assert_eq!(unsafe { call(bad as *const (), [8, 0, 0, 0, 0, 0]) }, Err(ACCESS_VIOLATION));
    }

    #[cfg(native_guard)]
    #[test]
    fn a_raised_exception_comes_back_as_an_error() {
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
    #[cfg(native_guard)]
    #[test]
    fn a_runtime_error_gives_its_message() {
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
