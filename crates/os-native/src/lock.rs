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
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn reentrant_and_blocks_other_threads() {
        let l = Arc::new(PluginLock::new());
        l.enter();
        l.enter();
        l.leave();
        let l2 = l.clone();
        let t = std::thread::spawn(move || {
            let start = Instant::now();
            let _g = l2.guard();
            start.elapsed()
        });
        std::thread::sleep(Duration::from_millis(50));
        l.leave();
        let waited = t.join().unwrap();
        assert!(cfg!(not(windows)) || waited >= Duration::from_millis(30), "the other thread waited for the unlock");
        // An unlock without holding the lock is ignored.
        l.leave();
        let _g = l.guard();
    }
}
