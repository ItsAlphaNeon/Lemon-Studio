// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The host side of a VST 2 instance: the callback plugins call ([`callback`]) and what those calls share with the
//! instance ([`Shared`]).
//!
//! Plugins call back from any thread, inside the host's calls into them or on their own threads, so everything here is
//! reached through the effect (`Effect::host_reserved_2` points at its `Shared`) and never through the instance.

use std::cell::{Cell, UnsafeCell};
use std::ffi::{c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use os_host::{Attention, ParamEvent};

use crate::abi::{host_op, Effect, TimeInfo, HOST_SUPPORTS};

/// What the host reports as its VST version.
const HOST_VERSION: isize = 2400;
/// The host's vendor and product name.
const HOST_NAME: &str = "Lemon Studio";

/// What a plugin told the host, and what it reads from it (shared with the instance).
pub struct Shared {
    /// Edits of the plugin's editor, for the app (`os_host::Plugin::take_param_events`).
    pub events: Mutex<Vec<ParamEvent>>,
    /// The size the editor asked for (`SIZE_WINDOW`).
    pub size: Mutex<Option<(i32, i32)>>,
    /// The plugin's inputs, outputs, parameters or latency changed (`IO_CHANGED`).
    pub io_changed: AtomicBool,
    /// The sample rate and largest block it was set up for, for `GET_SAMPLE_RATE` / `GET_BLOCK_SIZE`.
    pub rate: AtomicU32,
    pub block: AtomicU32,
    /// Rendering offline (an export).
    pub offline: AtomicBool,
    /// The plugin a shell creates (its id), 0 for a plain plugin file (`CURRENT_ID`).
    pub shell_id: i32,
    /// The plugin's folder (`DIRECTORY`).
    pub directory: CString,
    /// Raised with everything above.
    pub attention: Arc<Attention>,
    /// The transport plugins read (`GET_TIME`): written by the instance's own calls only (processing, activation, a
    /// stop), one at a time under the instance's mutex; plugins read it during processing.
    time: UnsafeCell<TimeInfo>,
}

// SAFETY: `time` is only written by the instance's own calls (see the field) and read by the plugin.
unsafe impl Sync for Shared {}

/// Most editor edits kept until the app takes them.
const MAX_EVENTS: usize = 4096;

impl Shared {
    pub fn new(shell_id: i32, directory: CString) -> Shared {
        Shared {
            events: Mutex::default(),
            size: Mutex::default(),
            io_changed: AtomicBool::new(false),
            // The values plugins assume before a host sets them up (vst2sdk `VST_DEFAULT_SAMPLE_RATE` / `_BLOCK_SIZE`).
            rate: AtomicU32::new(44100),
            block: AtomicU32::new(1024),
            offline: AtomicBool::new(false),
            shell_id,
            directory,
            attention: Arc::default(),
            time: UnsafeCell::new(TimeInfo::default()),
        }
    }

    pub fn time(&self) -> TimeInfo {
        // SAFETY: see `time`.
        unsafe { self.time.get().read() }
    }

    /// Sets the transport the plugin reads next (the instance's own calls only, see `time`).
    pub fn set_time(&self, t: TimeInfo) {
        // SAFETY: see `time`.
        unsafe { self.time.get().write(t) }
    }

    fn event(&self, e: ParamEvent) {
        if let Ok(mut v) = self.events.lock() {
            if v.len() < MAX_EVENTS {
                v.push(e);
            }
        }
        self.attention.raise();
    }
}

thread_local! {
    /// The host side of the instance a plugin's entry point is creating on this thread: the plugin calls back before it
    /// has handed out its effect (or with an effect not pointed at its host side yet).
    static CREATING: Cell<*const Shared> = const { Cell::new(std::ptr::null()) };
    /// The host itself is setting a parameter on this thread: the plugin's `AUTOMATE` echo of it is not an edit.
    static HOST_EDIT: Cell<bool> = const { Cell::new(false) };
}

/// Sets `cell` to `v` for the length of `f`.
fn with_set<T: Copy + 'static, R>(cell: &'static std::thread::LocalKey<Cell<T>>, v: T, f: impl FnOnce() -> R) -> R {
    struct Restore<T: Copy + 'static>(&'static std::thread::LocalKey<Cell<T>>, T);
    impl<T: Copy + 'static> Drop for Restore<T> {
        fn drop(&mut self) {
            self.0.with(|c| c.set(self.1));
        }
    }
    let _restore = Restore(cell, cell.with(|c| c.replace(v)));
    f()
}

/// Runs `f` (the host setting a parameter or a state) so that the plugin's echo of it is not reported as an edit.
pub fn host_edit<R>(f: impl FnOnce() -> R) -> R {
    with_set(&HOST_EDIT, true, f)
}

/// Runs `f` (a call to a plugin file's entry point) with `shared` as the host side of the effect it creates.
pub fn creating<R>(shared: &Arc<Shared>, f: impl FnOnce() -> R) -> R {
    with_set(&CREATING, Arc::as_ptr(shared), f)
}

/// Points `effect` at its host side. The instance keeps `shared` alive as long as the effect.
///
/// # Safety
/// `effect` is a live effect.
pub unsafe fn attach(effect: *mut Effect, shared: &Arc<Shared>) {
    unsafe { (*effect).host_reserved_2 = Arc::as_ptr(shared) as *mut c_void };
}

/// The host side of `effect` (or of the effect being created on this thread).
unsafe fn shared_of<'a>(effect: *mut Effect) -> Option<&'a Shared> {
    let own = if effect.is_null() { std::ptr::null() } else { unsafe { (*effect).host_reserved_2 as *const Shared } };
    let p = if own.is_null() { CREATING.with(Cell::get) } else { own };
    unsafe { p.as_ref() }
}

/// Writes `s` into a plugin's text buffer of `cap` bytes, NUL-terminated.
unsafe fn write_text(ptr: *mut c_void, s: &str, cap: usize) {
    if ptr.is_null() || cap == 0 {
        return;
    }
    let n = s.len().min(cap - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(s.as_ptr(), ptr as *mut u8, n);
        *(ptr as *mut u8).add(n) = 0;
    }
}

/// Everything a plugin asks of the host (vst2sdk `vst_host_callback_t`). Unknown op-codes answer 0 ("not handled").
///
/// # Safety
/// Called by plugins only, with the arguments each op-code documents.
pub unsafe extern "C" fn callback(effect: *mut Effect, opcode: i32, index: i32, value: isize, ptr: *mut c_void, opt: f32) -> isize {
    let shared = unsafe { shared_of(effect) };
    let Some(s) = shared else {
        // Nothing to answer from yet: only what does not depend on an instance.
        return match opcode {
            host_op::VERSION => HOST_VERSION,
            _ => 0,
        };
    };
    match opcode {
        host_op::VERSION => HOST_VERSION,
        host_op::CURRENT_ID => s.shell_id as isize,
        host_op::AUTOMATE => {
            if !HOST_EDIT.with(Cell::get) {
                s.event(ParamEvent::Value(index.max(0) as u32, f64::from(opt).clamp(0.0, 1.0)));
            }
            1
        }
        host_op::BEGIN_EDIT => {
            s.event(ParamEvent::Begin(index.max(0) as u32));
            1
        }
        host_op::END_EDIT => {
            s.event(ParamEvent::End(index.max(0) as u32));
            1
        }
        host_op::GET_TIME => s.time.get() as isize,
        host_op::TEMPO_AT => (s.time().tempo * 10000.0) as isize,
        host_op::IO_CHANGED => {
            s.io_changed.store(true, Ordering::Release);
            s.attention.raise();
            os_host::wake();
            1
        }
        host_op::SIZE_WINDOW => {
            if let Ok(mut size) = s.size.lock() {
                *size = Some((index, value as i32));
            }
            s.attention.raise();
            os_host::wake();
            1
        }
        host_op::GET_SAMPLE_RATE => s.rate.load(Ordering::Relaxed) as isize,
        host_op::GET_BLOCK_SIZE => s.block.load(Ordering::Relaxed) as isize,
        host_op::PROCESS_LEVEL => {
            if s.offline.load(Ordering::Relaxed) {
                4
            } else {
                0
            }
        }
        host_op::VENDOR_NAME | host_op::PRODUCT_NAME => {
            unsafe { write_text(ptr, HOST_NAME, 64) };
            1
        }
        host_op::VENDOR_VERSION => 1,
        host_op::SUPPORTS => {
            if ptr.is_null() {
                return 0;
            }
            let name = unsafe { CStr::from_ptr(ptr as *const std::ffi::c_char) }.to_bytes();
            isize::from(HOST_SUPPORTS.contains(&name))
        }
        host_op::LANGUAGE => 1,
        // On macOS the op-code returns a structure from the classic Mac OS file system: not given.
        host_op::DIRECTORY if !cfg!(target_os = "macos") => s.directory.as_ptr() as isize,
        host_op::UPDATE_DISPLAY => {
            s.attention.raise();
            1
        }
        _ => 0,
    }
}
