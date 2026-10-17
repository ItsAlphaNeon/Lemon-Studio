// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin crash containment.
//!
//! Every call Lemon Studio makes into a plugin's code goes through [`call`]. On Windows MSVC builds it runs under
//! structured exception handling (`guard.c`): an access violation or another exception the plugin raises and does not
//! handle itself comes back as `Err(code)` instead of ending the process. The plugin's [`Module`] is then marked
//! crashed: every instance of it stops ([`crate::Guarded`]) and its windows no longer reach its code
//! ([`watch_windows`]). Elsewhere calls are made directly and nothing is contained.
//!
//! What cannot be contained: errors the system ends a process for at once (heap corruption, stack buffer overruns, a
//! stack overflow) and crashes on threads a plugin started itself.
//
// spec: plugins/plugin-formats.md ("Crash containment")

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// The code of a plugin: one loaded plugin file, or one instance of a plugin compiled into Lemon Studio.
#[derive(Debug, Default)]
pub struct Module {
    /// Load address of the plugin file (its module handle); 0 for this program's own code.
    base: usize,
    /// Exception code of its first contained crash, 0 while it runs.
    crash: AtomicU32,
}

impl Module {
    /// The exception code of the crash that stopped this module's code, if one did.
    pub fn crashed(&self) -> Option<u32> {
        match self.crash.load(Ordering::Acquire) {
            0 => None,
            c => Some(c),
        }
    }

    /// Marks the module crashed with `code` (the first crash is kept) and wakes the UI thread to report it.
    fn trip(&self, code: u32) {
        let _ = self.crash.compare_exchange(0, code.max(1), Ordering::AcqRel, Ordering::Acquire);
        TRIPPED.store(true, Ordering::Release);
        crate::wake();
    }
}

/// Guarded plugin files by load address, kept for the whole process (a crashed file is never unloaded).
static MODULES: Mutex<Vec<Arc<Module>>> = Mutex::new(Vec::new());
/// A plugin crashed in this process and the crash was contained.
static TRIPPED: AtomicBool = AtomicBool::new(false);

/// The exception code of an error raised by the Object Pascal runtime native plugins are built in, which the plugin
/// did not handle (spec: plugins/fruity-abi.md, "Crash containment").
pub const RUNTIME_ERROR: u32 = 0x0EED_FADE;

thread_local! {
    /// The text of the last contained exception on this thread ([`last_message`]).
    static LAST_MESSAGE: RefCell<String> = const { RefCell::new(String::new()) };
}

/// What a contained exception was (the layout `guard.c` fills).
#[cfg(plugin_guard)]
#[repr(C)]
struct Fault {
    code: u32,
    /// The text of the runtime's error object (UTF-16, 0-terminated), empty when there is none.
    message: [u16; 256],
}

/// The module whose code is at `addr` (a plugin file's entry point, or a table it exports). All instances of one plugin
/// file share their `Module`, so a crash stops them all; a plugin compiled into this program gets a new `Module` on
/// every call (one per instance).
pub fn module_of(addr: *const c_void) -> Arc<Module> {
    let base = base_of(addr);
    if base == 0 || base == own_base() {
        return Arc::new(Module::default());
    }
    let mut list = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = list.iter().find(|m| m.base == base) {
        return m.clone();
    }
    let m = Arc::new(Module { base, crash: AtomicU32::new(0) });
    list.push(m.clone());
    m
}

/// The module of the loaded plugin file `path` (a file loaded by path, such as a native plugin DLL); see [`module_of`].
pub fn module_named(path: &std::path::Path) -> Arc<Module> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: only looks the loaded file up.
        let m = unsafe { GetModuleHandleW(wide.as_ptr()) };
        if !m.is_null() {
            return module_of(m as *const c_void);
        }
    }
    let _ = path;
    Arc::new(Module::default())
}

/// Calls `f`, code of `module`, under the guard. A crash inside marks the module crashed and comes back as its exception
/// code; a module that crashed before is not called at all.
pub fn call<R, F: FnOnce() -> R>(module: &Module, f: F) -> Result<R, u32> {
    if let Some(c) = module.crashed() {
        return Err(c);
    }
    catch(f).inspect_err(|&c| module.trip(c))
}

/// Calls `f` under the guard: a crash inside comes back as its exception code. For plugin code no [`Module`] stands for
/// yet (a plugin file being loaded); [`call`] otherwise.
pub fn catch<R, F: FnOnce() -> R>(f: F) -> Result<R, u32> {
    #[cfg(plugin_guard)]
    {
        let mut slot: (Option<F>, Option<R>) = (Some(f), None);
        let mut fault = Fault { code: 0, message: [0; 256] };
        // SAFETY: the trampoline gets the slot it was instantiated for, alive for the whole call.
        let done = unsafe { os_host_guarded_call(trampoline::<R, F>, (&raw mut slot).cast(), &mut fault) } != 0;
        match slot.1.take() {
            Some(r) if done => Ok(r),
            _ => {
                TRIPPED.store(true, Ordering::Release);
                let len = fault.message.iter().position(|&c| c == 0).unwrap_or(fault.message.len());
                LAST_MESSAGE.with(|m| *m.borrow_mut() = String::from_utf16_lossy(&fault.message[..len]));
                Err(fault.code.max(1))
            }
        }
    }
    #[cfg(not(plugin_guard))]
    Ok(f())
}

#[cfg(plugin_guard)]
unsafe extern "C-unwind" {
    /// `guard.c`: calls `f(ctx)`; 0 and what was raised in `fault` when it raised an exception the guard contains.
    fn os_host_guarded_call(f: extern "C-unwind" fn(*mut c_void), ctx: *mut c_void, fault: *mut Fault) -> i32;
}

/// Runs the closure in `catch`'s slot. Rust code between the guard and the plugin unwinds like a panic would.
#[cfg(plugin_guard)]
extern "C-unwind" fn trampoline<R, F: FnOnce() -> R>(ctx: *mut c_void) {
    // SAFETY: `ctx` is the `(Option<F>, Option<R>)` slot of the `catch` call running this.
    let slot = unsafe { &mut *ctx.cast::<(Option<F>, Option<R>)>() };
    if let Some(f) = slot.0.take() {
        slot.1 = Some(f());
    }
}

/// The text the plugin's runtime gave the last exception contained on this thread ("" when it gave none): what the
/// plugin would have shown in its own error box.
pub fn last_message() -> String {
    LAST_MESSAGE.with(|m| m.borrow().clone())
}

/// Whether plugin crashes are contained in this build.
pub const fn contained() -> bool {
    cfg!(plugin_guard)
}

/// Whether a plugin crashed in this process (and the crash was contained). Its code stays loaded in whatever state the
/// crash left it, and unloading it as a normal process exit does can crash: see [`exit`].
pub fn tripped() -> bool {
    TRIPPED.load(Ordering::Acquire)
}

/// Ends the process with `code`. After a contained crash ([`tripped`]) the process ends at once, without unloading
/// plugin files (a stopped plugin's code never runs again); otherwise it exits normally.
pub fn exit(code: i32) -> ! {
    #[cfg(windows)]
    if tripped() {
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        // SAFETY: ends this process.
        unsafe { TerminateProcess(GetCurrentProcess(), code as u32) };
    }
    std::process::exit(code)
}

/// What exception `code` means, for messages ("a memory error").
pub fn describe(code: u32) -> String {
    match code {
        0xC000_0005 => "a memory error".into(),
        0xC000_0006 => "a paging error".into(),
        0xC000_001D | 0xC000_0096 => "an invalid instruction".into(),
        0xC000_0094 => "a division by zero".into(),
        0xE06D_7363 => "an error it did not handle".into(),
        RUNTIME_ERROR => "a plugin error".into(),
        c => format!("error {c:#010X}"),
    }
}

/// What the user is told when plugin `name` stopped after exception `code` ("Reverb stopped after a memory error").
pub fn stopped_text(name: &str, code: u32) -> String {
    format!("{name} stopped after {}", describe(code))
}

/// Load address of the module whose code or data is at `addr` (0 when none).
fn base_of(addr: *const c_void) -> usize {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::LibraryLoader::{
            GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        };
        let mut m = std::ptr::null_mut();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        // SAFETY: only looks the address up.
        if unsafe { GetModuleHandleExW(flags, addr.cast(), &mut m) } != 0 {
            return m as usize;
        }
    }
    let _ = addr;
    0
}

/// Load address of this program.
fn own_base() -> usize {
    static BASE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *BASE.get_or_init(|| base_of(own_base as *const c_void))
}

/// The guarded plugin file whose code is at `addr`, if any.
#[cfg(plugin_guard)]
fn guarded_at(addr: *const c_void) -> Option<Arc<Module>> {
    let base = base_of(addr);
    if base == 0 {
        return None;
    }
    MODULES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|m| m.base == base).cloned()
}

/// Puts the windows of guarded plugin files under the guard (UI thread). Every window of this process whose window
/// procedure is a guarded plugin file's gets its messages through [`call`]; once that file crashed, its windows only
/// get the system's default handling. Looks at most every 250 ms unless `now` (after an editor opened).
pub fn watch_windows(now: bool) {
    #[cfg(plugin_guard)]
    windows::watch(now);
    #[cfg(not(plugin_guard))]
    let _ = now;
}

#[cfg(plugin_guard)]
mod windows {
    use super::*;
    use std::cell::Cell;
    use std::time::{Duration, Instant};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    /// Window properties of a guarded window: its own window procedure, and the `Module` it belongs to.
    const PROC: *const u16 = windows_sys::w!("LemonStudio.GuardedProc");
    const MODULE: *const u16 = windows_sys::w!("LemonStudio.GuardedModule");

    thread_local! {
        static LAST: Cell<Option<Instant>> = const { Cell::new(None) };
    }

    pub fn watch(now: bool) {
        if !now && LAST.get().is_some_and(|t| t.elapsed() < Duration::from_millis(250)) {
            return;
        }
        LAST.set(Some(Instant::now()));
        if MODULES.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            return;
        }
        for h in windows() {
            // SAFETY: a window of this process; a destroyed one fails the calls harmlessly.
            unsafe { guard(h) };
        }
    }

    /// The windows of this process: top-level ones, their descendants, and message-only ones.
    fn windows() -> Vec<HWND> {
        unsafe extern "system" fn push(h: HWND, list: LPARAM) -> BOOL {
            // SAFETY: `list` is the vector `windows` passed.
            unsafe { (*(list as *mut Vec<HWND>)).push(h) };
            1
        }
        let mut top: Vec<HWND> = Vec::new();
        // SAFETY: the callback only pushes to `top`, alive for the call.
        unsafe { EnumWindows(Some(push), &raw mut top as LPARAM) };
        let mut h: HWND = std::ptr::null_mut();
        loop {
            // SAFETY: walks the message-only windows.
            h = unsafe { FindWindowExW(HWND_MESSAGE, h, std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                break;
            }
            top.push(h);
        }
        // SAFETY: plain query.
        let pid = unsafe { GetCurrentProcessId() };
        let ours = |h: HWND| {
            let mut p = 0u32;
            // SAFETY: plain query.
            unsafe { GetWindowThreadProcessId(h, &mut p) };
            p == pid
        };
        let mut all = Vec::new();
        for h in top.into_iter().filter(|&h| ours(h)) {
            all.push(h);
            // SAFETY: the callback only pushes to `all`, alive for the call.
            unsafe { EnumChildWindows(h, Some(push), &raw mut all as LPARAM) };
        }
        all
    }

    /// Routes window `h` through the guard when its window procedure is a guarded plugin file's.
    unsafe fn guard(h: HWND) {
        unsafe {
            // Already guarded (or subclassed again by the plugin on top of the guard: left as it is).
            if !GetPropW(h, PROC).is_null() {
                return;
            }
            let wide = IsWindowUnicode(h) != 0;
            let proc = if wide { GetWindowLongPtrW(h, GWLP_WNDPROC) } else { GetWindowLongPtrA(h, GWLP_WNDPROC) };
            if proc == 0 || proc == addr(guarded_w) || proc == addr(guarded_a) {
                return;
            }
            let Some(m) = guarded_at(proc as *const c_void) else { return };
            // A window class of this program that a plugin subclassed is left alone: the program's own window procedure
            // is further down its chain, and must keep running after a crash.
            if GetClassLongPtrW(h, GCLP_HMODULE) == own_base() {
                return;
            }
            if crate::debug() {
                eprintln!("[guard] window {h:?} of the plugin file at {:#x}", m.base);
            }
            subclass(h, &m, proc, wide);
        }
    }

    /// Puts window `h`, whose window procedure `proc` is code of `m`, under the guard.
    pub(super) unsafe fn subclass(h: HWND, m: &Arc<Module>, proc: isize, wide: bool) {
        unsafe {
            SetPropW(h, PROC, proc as *mut c_void);
            SetPropW(h, MODULE, Arc::as_ptr(m) as *mut c_void);
            if wide {
                SetWindowLongPtrW(h, GWLP_WNDPROC, addr(guarded_w));
            } else {
                SetWindowLongPtrA(h, GWLP_WNDPROC, addr(guarded_a));
            }
        }
    }

    /// A window procedure's address.
    fn addr(f: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) -> isize {
        f as isize
    }

    unsafe extern "system" fn guarded_w(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe { guarded(h, msg, w, l, true) }
    }

    unsafe extern "system" fn guarded_a(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe { guarded(h, msg, w, l, false) }
    }

    /// The guard's window procedure: the plugin's own under [`call`], or the default handling once its file crashed.
    unsafe fn guarded(h: HWND, msg: u32, w: WPARAM, l: LPARAM, wide: bool) -> LRESULT {
        unsafe {
            let default = || if wide { DefWindowProcW(h, msg, w, l) } else { DefWindowProcA(h, msg, w, l) };
            let proc = GetPropW(h, PROC) as usize;
            let module = GetPropW(h, MODULE) as *const Module;
            if proc == 0 || module.is_null() {
                return default();
            }
            if msg == WM_NCDESTROY {
                RemovePropW(h, PROC);
                RemovePropW(h, MODULE);
            }
            // SAFETY: modules are kept in `MODULES` for the whole process.
            let module = &*module;
            // SAFETY: `proc` is the window procedure the window had.
            let proc: WNDPROC = std::mem::transmute::<usize, WNDPROC>(proc);
            let run = || if wide { CallWindowProcW(proc, h, msg, w, l) } else { CallWindowProcA(proc, h, msg, w, l) };
            call(module, run).unwrap_or_else(|_| default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calls_return_their_result() {
        let m = Module::default();
        assert_eq!(call(&m, || 2 + 3), Ok(5));
        let mut v = vec![1];
        assert_eq!(call(&m, || v.push(2)), Ok(()));
        assert_eq!(v, [1, 2]);
        assert_eq!(m.crashed(), None);
    }

    /// Reads memory at `addr`, as a plugin with a bad pointer does.
    #[cfg(plugin_guard)]
    #[inline(never)]
    extern "C-unwind" fn read_at(addr: usize) -> usize {
        // SAFETY: deliberately not: the read faults and the guard catches it.
        unsafe { std::ptr::read_volatile(addr as *const usize) }
    }

    #[cfg(plugin_guard)]
    #[test]
    fn a_memory_error_stops_the_module_and_comes_back_as_its_code() {
        let m = Module::default();
        assert_eq!(call(&m, || read_at(8)), Err(0xC000_0005));
        assert_eq!(m.crashed(), Some(0xC000_0005));
        assert!(tripped());
        // Never called again.
        let mut ran = false;
        assert_eq!(call(&m, || ran = true), Err(0xC000_0005));
        assert!(!ran);
        assert_eq!(describe(0xC000_0005), "a memory error");
    }

    /// A runtime error's object: class pointer, then the message as a counted UTF-16 string.
    #[cfg(plugin_guard)]
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
        static TEXT: Counted = Counted { len: 9, text: [66, 97, 100, 32, 105, 110, 100, 101, 120] };
        let obj: [usize; 2] = [0, TEXT.text.as_ptr() as usize];
        let args = [0usize, obj.as_ptr() as usize];
        let r = catch(|| unsafe { RaiseException(RUNTIME_ERROR, 1, 2, args.as_ptr()) });
        assert_eq!(r, Err(RUNTIME_ERROR));
        assert_eq!(last_message(), "Bad index");
        assert_eq!(describe(RUNTIME_ERROR), "a plugin error");
    }

    /// A panic in Lemon Studio's own code under the guard is still a panic.
    #[cfg(plugin_guard)]
    #[test]
    fn panics_pass_through_the_guard() {
        let m = Module::default();
        let r = std::panic::catch_unwind(|| call(&m, || panic!("ours")));
        assert!(r.is_err());
        assert_eq!(m.crashed(), None);
    }

    #[test]
    fn this_program_gets_a_module_per_instance() {
        let a = module_of(this_program_gets_a_module_per_instance as *const c_void);
        let b = module_of(this_program_gets_a_module_per_instance as *const c_void);
        assert!(!Arc::ptr_eq(&a, &b));
    }

    /// A crash in a guarded window procedure stops its module; the window then gets the default handling.
    #[cfg(plugin_guard)]
    #[test]
    fn a_crash_in_a_window_procedure_stops_its_module() {
        use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe extern "system" fn faulty(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            match msg {
                // SAFETY: deliberately not: the read faults and the guard catches it.
                WM_USER => unsafe { std::ptr::read_volatile(8 as *const usize) as LRESULT },
                m if m == WM_USER + 1 => 7,
                _ => unsafe { DefWindowProcW(h, msg, w, l) },
            }
        }
        let class = windows_sys::w!("LemonStudioGuardTest");
        unsafe {
            let wc = WNDCLASSW { lpfnWndProc: Some(faulty), lpszClassName: class, ..std::mem::zeroed() };
            RegisterClassW(&wc);
            let h = CreateWindowExW(0, class, class, WS_OVERLAPPED, 0, 0, 10, 10, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            assert!(!h.is_null());
            let m: &'static Arc<Module> = Box::leak(Box::new(Arc::new(Module::default())));
            let proc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = faulty;
            windows::subclass(h, m, proc as isize, true);
            assert_eq!(SendMessageW(h, WM_USER + 1, 0, 0), 7, "the plugin's own handling");
            assert_eq!(SendMessageW(h, WM_USER, 0, 0), 0);
            assert_eq!(m.crashed(), Some(0xC000_0005));
            assert_eq!(SendMessageW(h, WM_USER + 1, 0, 0), 0, "default handling after the crash");
            assert!(DestroyWindow(h) != 0);
        }
    }
}
