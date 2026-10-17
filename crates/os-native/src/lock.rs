// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The per-plugin render lock behind the host's `LockMix` / `UnlockMix`, `LockPlugin` / `UnlockPlugin` and
//! `SuspendOutput` / `ResumeOutput` methods (spec plugins/fruity-abi.md, "Editors": an editor runs on the GUI
//! thread while the audio thread renders; a plugin brackets edits that must not race its rendering with these
//! calls). The host takes the lock around every render call, so a plugin holding it from its GUI keeps its own
//! rendering out until it unlocks. Re-entrant per thread; an unlock from a thread that does not hold it is ignored.
//
// spec: plugins/fruity-abi.md

#[cfg(windows)]
mod imp {
    use std::cell::UnsafeCell;
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows_sys::Win32::System::Threading::{
        DeleteCriticalSection, EnterCriticalSection, GetCurrentThreadId, InitializeCriticalSection, LeaveCriticalSection, CRITICAL_SECTION,
    };

    pub struct PluginLock {
        cs: Box<UnsafeCell<CRITICAL_SECTION>>,
        /// Thread id holding the lock (0 = none) and its depth; written only by the holder.
        owner: AtomicU32,
        depth: AtomicU32,
    }

    // SAFETY: a critical section is made to be shared between threads; it never moves (boxed).
    unsafe impl Send for PluginLock {}
    unsafe impl Sync for PluginLock {}

    impl PluginLock {
        pub fn new() -> Self {
            let cs: Box<UnsafeCell<CRITICAL_SECTION>> = Box::new(UnsafeCell::new(unsafe { std::mem::zeroed() }));
            unsafe { InitializeCriticalSection(cs.get()) };
            PluginLock { cs, owner: AtomicU32::new(0), depth: AtomicU32::new(0) }
        }

        pub fn enter(&self) {
            unsafe { EnterCriticalSection(self.cs.get()) };
            self.owner.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
            self.depth.fetch_add(1, Ordering::Relaxed);
        }

        pub fn leave(&self) {
            let me = unsafe { GetCurrentThreadId() };
            if self.owner.load(Ordering::Relaxed) != me || self.depth.load(Ordering::Relaxed) == 0 {
                return;
            }
            if self.depth.fetch_sub(1, Ordering::Relaxed) == 1 {
                self.owner.store(0, Ordering::Relaxed);
            }
            unsafe { LeaveCriticalSection(self.cs.get()) };
        }
    }

    impl Drop for PluginLock {
        fn drop(&mut self) {
            unsafe { DeleteCriticalSection(self.cs.get()) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    /// Elsewhere native plugins do not load; the lock is a no-op.
    pub struct PluginLock;
    impl PluginLock {
        pub fn new() -> Self {
            PluginLock
        }
        pub fn enter(&self) {}
        pub fn leave(&self) {}
    }
}

pub use imp::PluginLock;

impl Default for PluginLock {
    fn default() -> Self {
        Self::new()
    }
}

/// Holds the lock for a scope.
pub struct Guard<'a>(&'a PluginLock);

impl PluginLock {
    pub fn guard(&self) -> Guard<'_> {
        self.enter();
        Guard(self)
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    #[test]
    fn reentrant_and_blocks_other_threads() {
        let l = Arc::new(PluginLock::new());
        l.enter();
        l.enter();
        l.leave();
        // Still held once: another thread must not get it until the last unlock (checked by what it did, not by how
        // long it waited, which depends on when the system lets it run).
        let (l2, got) = (l.clone(), Arc::new(AtomicBool::new(false)));
        let got2 = got.clone();
        let (started, wait_start) = mpsc::channel();
        let t = std::thread::spawn(move || {
            started.send(()).unwrap();
            let _g = l2.guard();
            got2.store(true, Ordering::SeqCst);
        });
        wait_start.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(cfg!(not(windows)) || !got.load(Ordering::SeqCst), "the other thread waited for the unlock");
        l.leave();
        t.join().unwrap();
        assert!(got.load(Ordering::SeqCst));
        // An unlock without holding the lock is ignored.
        l.leave();
        let _g = l.guard();
    }
}
