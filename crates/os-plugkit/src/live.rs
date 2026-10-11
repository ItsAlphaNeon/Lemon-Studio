//! Editor edits the host has not handed back yet (spec: plugins/open-sampler.md §4 "Edits apply at once").
//!
//! The editor tells the host about every edit; nih-plug then changes the parameter only when the host passes the edit
//! back (VST3 while processing) or flushes (CLAP), and some hosts do neither. So the sound reads parameters through
//! [`Live::live`]: the editor's newest value wins until the host's own value moves to it (confirmed) or away from the
//! value it had at the edit (automation / undo in the host). Entries are keyed by the parameter's address, unique per
//! plugin instance.

use nih_plug::prelude::*;

static LIVE: std::sync::Mutex<Vec<LiveEdit>> = std::sync::Mutex::new(Vec::new());

#[derive(Clone, Copy)]
struct LiveEdit {
    addr: usize,
    /// The editor's value (normalised).
    norm: f32,
    /// The host's value when the edit was made.
    base: f32,
}

/// Records an editor edit of the parameter at `addr` whose host value is `host` now.
pub fn live_set(addr: usize, norm: f32, host: f32) {
    if let Ok(mut l) = LIVE.lock() {
        l.retain(|e| e.addr != addr);
        if (norm - host).abs() > 1e-7 {
            l.push(LiveEdit { addr, norm, base: host });
        }
    }
}

/// The normalised value the sound should use for the parameter at `addr` (`host` = its host value), or `None` to use
/// the host's. Never blocks: when the editor holds the list, the host value is used for this block.
pub fn live_norm(addr: usize, host: f32) -> Option<f32> {
    let mut l = LIVE.try_lock().ok()?;
    let i = l.iter().position(|e| e.addr == addr)?;
    let e = l[i];
    if (host - e.base).abs() <= 1e-7 && (host - e.norm).abs() > 1e-7 {
        Some(e.norm)
    } else {
        // Confirmed, or the host changed it since: the host's value from now on.
        l.swap_remove(i);
        None
    }
}

/// Forgets the edits of parameters inside `from..to` (a dropped instance).
pub fn live_forget(from: usize, to: usize) {
    if let Ok(mut l) = LIVE.lock() {
        l.retain(|e| e.addr < from || e.addr >= to);
    }
}

/// The normalised value the sound uses for `p` (its pending editor edit, else the host's value).
pub fn live_of(p: ParamPtr) -> f32 {
    // SAFETY: callers hold pointers from a live params object.
    let host = unsafe { p.unmodulated_normalized_value() };
    live_norm(param_addr(p), host).unwrap_or(host)
}

/// The address `live_set` keys a parameter by.
pub fn param_addr(p: ParamPtr) -> usize {
    match p {
        ParamPtr::FloatParam(p) => p as usize,
        ParamPtr::IntParam(p) => p as usize,
        ParamPtr::BoolParam(p) => p as usize,
        ParamPtr::EnumParam(p) => p as usize,
    }
}

/// Parameter values for the sound (see the module docs).
pub trait Live {
    type Plain;
    fn live(&self) -> Self::Plain;
}

impl Live for FloatParam {
    type Plain = f32;
    fn live(&self) -> f32 {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}

impl Live for IntParam {
    type Plain = i32;
    fn live(&self) -> i32 {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}

impl Live for BoolParam {
    type Plain = bool;
    fn live(&self) -> bool {
        match live_norm(self as *const Self as usize, self.unmodulated_normalized_value()) {
            Some(n) => self.preview_plain(n),
            None => self.value(),
        }
    }
}
