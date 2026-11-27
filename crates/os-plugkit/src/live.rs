// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Editor edits the host has not handed back yet (spec: plugins/lemon-sampler.md §4 "Edits apply at once").
//!
//! The editor tells the host about every edit; nih-plug then changes the parameter only when the host passes the edit
//! back (VST3 while processing) or flushes (CLAP), and some hosts do neither. So the sound reads parameters through
//! [`LiveEdits::value`]: the editor's newest value wins until the host's own value moves to it (confirmed) or away from
//! the value it had at the edit (automation / undo in the host).
//!
//! Each plugin instance's params object owns one [`LiveEdits`] and hands it to its editor's [`crate::vals::Vals`], so
//! instances never see or wait on each other's edits. Entries are keyed by the parameter's address, which stays put
//! while the params object lives (behind the plugin's `Arc`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use nih_plug::prelude::*;

#[derive(Clone, Copy)]
struct LiveEdit {
    addr: usize,
    /// The editor's value (normalised).
    norm: f32,
    /// The host's value when the edit was made.
    base: f32,
}

/// One plugin instance's editor edits the host has not handed back yet.
#[derive(Default)]
pub struct LiveEdits {
    list: Mutex<Vec<LiveEdit>>,
    /// Fast path for the audio thread: true only while `list` is non-empty (updated under `list`'s lock).
    any: AtomicBool,
}

impl LiveEdits {
    /// Records an editor edit: `p` sounds at `norm` (normalised) until its host value moves from what it is now.
    pub fn set(&self, p: ParamPtr, norm: f32) {
        // SAFETY: callers hold pointers from a live params object.
        let host = unsafe { p.unmodulated_normalized_value() };
        self.set_at(param_addr(p), norm, host);
    }

    fn set_at(&self, addr: usize, norm: f32, host: f32) {
        if let Ok(mut l) = self.list.lock() {
            l.retain(|e| e.addr != addr);
            if (norm - host).abs() > 1e-7 {
                l.push(LiveEdit { addr, norm, base: host });
            }
            self.any.store(!l.is_empty(), Ordering::Release);
        }
    }

    /// The normalised value the sound should use for the parameter at `addr` (`host` = its host value), or `None` to
    /// use the host's. Never blocks: while this instance's editor holds the list, the host value is used for this block.
    #[inline]
    fn norm_at(&self, addr: usize, host: f32) -> Option<f32> {
        if !self.any.load(Ordering::Acquire) {
            return None;
        }
        self.norm_at_slow(addr, host)
    }

    #[cold]
    fn norm_at_slow(&self, addr: usize, host: f32) -> Option<f32> {
        let mut l = self.list.try_lock().ok()?;
        let i = l.iter().position(|e| e.addr == addr)?;
        let e = l[i];
        if (host - e.base).abs() <= 1e-7 && (host - e.norm).abs() > 1e-7 {
            Some(e.norm)
        } else {
            // Confirmed, or the host changed it since: the host's value from now on.
            l.swap_remove(i);
            self.any.store(!l.is_empty(), Ordering::Release);
            None
        }
    }

    /// The normalised value the sound uses for `p` (its pending editor edit, else the host's value).
    #[inline]
    pub fn norm(&self, p: ParamPtr) -> f32 {
        // SAFETY: callers hold pointers from a live params object.
        let host = unsafe { p.unmodulated_normalized_value() };
        self.norm_at(param_addr(p), host).unwrap_or(host)
    }

    /// The plain value the sound uses for `p` (its pending editor edit, else the host's value).
    #[inline]
    pub fn value<P: Param>(&self, p: &P) -> P::Plain {
        if !self.any.load(Ordering::Acquire) {
            return p.modulated_plain_value();
        }
        match self.norm_at_slow(p as *const P as usize, p.unmodulated_normalized_value()) {
            Some(n) => p.preview_plain(n),
            None => p.modulated_plain_value(),
        }
    }
}

/// The address a parameter's edits are keyed by.
fn param_addr(p: ParamPtr) -> usize {
    match p {
        ParamPtr::FloatParam(p) => p as usize,
        ParamPtr::IntParam(p) => p as usize,
        ParamPtr::BoolParam(p) => p as usize,
        ParamPtr::EnumParam(p) => p as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param() -> IntParam {
        IntParam::new("p", 0, IntRange::Linear { min: 0, max: 4 })
    }

    #[test]
    fn edits_sound_until_the_host_confirms_or_moves() {
        let live = LiveEdits::default();
        let p = param();
        live.set(p.as_ptr(), 0.5);
        assert_eq!(live.value(&p), 2);
        assert_eq!(live.norm(p.as_ptr()), 0.5);
        // The host confirms: its value from then on.
        let a = param_addr(p.as_ptr());
        assert_eq!(live.norm_at(a, 0.5), None);
        assert_eq!(live.norm_at(a, 0.0), None);
        // The host moves elsewhere (automation): the edit is dropped for good, also when the host comes back.
        live.set_at(a, 0.75, 0.0);
        assert_eq!(live.norm_at(a, 0.25), None);
        assert_eq!(live.norm_at(a, 0.0), None);
        // An edit back to the host value is no edit.
        live.set_at(a, 0.0, 0.0);
        assert_eq!(live.norm_at(a, 0.0), None);
    }

    #[test]
    fn instances_keep_their_own_edits() {
        let (a, b) = (LiveEdits::default(), LiveEdits::default());
        let p = param();
        a.set(p.as_ptr(), 0.5);
        assert_eq!(b.value(&p), 0);
        // Another instance holding its list (its editor, or its audio thread) never hides this instance's edit.
        b.set(p.as_ptr(), 1.0);
        let _held = a.list.lock().unwrap();
        assert_eq!(b.value(&p), 4);
    }

    #[test]
    fn empty_fast_path_tracks_pending_edits() {
        let live = LiveEdits::default();
        let p = param();
        let a = param_addr(p.as_ptr());
        assert!(!live.any.load(Ordering::Acquire));
        // Even if the mutex is held while empty, `norm_at` / `value` skip locking via the atomic fast path.
        {
            let _held = live.list.lock().unwrap();
            assert_eq!(live.norm_at(a, 0.0), None);
            assert_eq!(live.value(&p), 0);
        }
        live.set(p.as_ptr(), 0.5);
        assert!(live.any.load(Ordering::Acquire));
        assert_eq!(live.value(&p), 2);
        // Confirming the last edit clears the atomic flag back to false.
        assert_eq!(live.norm_at(a, 0.5), None);
        assert!(!live.any.load(Ordering::Acquire));
        // Editing back to host also leaves the flag false.
        live.set(p.as_ptr(), 0.75);
        assert!(live.any.load(Ordering::Acquire));
        live.set_at(a, 0.0, 0.0);
        assert!(!live.any.load(Ordering::Acquire));
    }
}
