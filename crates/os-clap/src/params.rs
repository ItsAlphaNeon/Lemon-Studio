// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Automation of CLAP parameters: values from the engine (`Instrument::param` / `HostedEffect::param_fx`, by parameter
//! index and normalized value) become `CLAP_EVENT_PARAM_VALUE` events at their frame offsets inside `process`, merged in
//! time order with the host's own edits and the notes. When nothing processes the instance the newest values are
//! flushed on `idle`. Also the plugin's rescan requests (its parameter list changed).
//!
// spec: plugins/plugin-parameters.md §1 (list and rescan), §2 (automation delivery); CLAP ext/params.h, events.h

use std::ptr;

use clap_sys::events::{clap_event_header, clap_event_param_value, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE};
use clap_sys::ext::params::{clap_param_info, clap_plugin_params, CLAP_PARAM_IS_STEPPED, CLAP_PARAM_RESCAN_ALL, CLAP_PARAM_RESCAN_INFO};
use clap_sys::plugin::clap_plugin;

use crate::Plugin;

/// Most automation values waiting for a process call (the audio thread never grows the queue).
pub(crate) const MAX_AUTO: usize = 2048;

/// A parameter in list order: (id, min, max, stepped). A parameter whose info could not be read keeps its place
/// with id `u32::MAX`, so positions stay the automation indexes.
pub(crate) type Range = (u32, f64, f64, bool);

/// Reads the parameter ranges in list order (main thread).
///
/// # Safety
/// `params` must be the plugin's params extension (or null) and `plugin` the instance.
pub(crate) unsafe fn read_ranges(params: *const clap_plugin_params, plugin: *const clap_plugin) -> Vec<Range> {
    let mut out = Vec::new();
    if params.is_null() {
        return out;
    }
    unsafe {
        for i in 0..(*params).count.map_or(0, |c| c(plugin)) {
            let mut info: clap_param_info = std::mem::zeroed();
            if (*params).get_info.is_some_and(|g| g(plugin, i, &mut info)) {
                out.push((info.id, info.min_value, info.max_value, info.flags & CLAP_PARAM_IS_STEPPED != 0));
            } else {
                out.push((u32::MAX, 0.0, 1.0, false));
            }
        }
    }
    out
}

pub(crate) fn value_event(time: u32, id: u32, value: f64) -> clap_event_param_value {
    clap_event_param_value {
        header: clap_event_header {
            size: std::mem::size_of::<clap_event_param_value>() as u32,
            time,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_PARAM_VALUE,
            flags: 0,
        },
        param_id: id,
        cookie: ptr::null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    }
}

impl Plugin {
    /// Automation (audio thread): parameter `index` of the list to normalized `value`, `offset` frames into the next
    /// process call. A stepped parameter gets its nearest step. Unknown indexes are ignored.
    pub(crate) fn automate(&mut self, offset: u32, index: u32, value: f64) {
        let Some(&(id, lo, hi, stepped)) = self.ranges.get(index as usize) else { return };
        if id == u32::MAX || self.auto.len() >= MAX_AUTO {
            return;
        }
        let mut plain = lo + value.clamp(0.0, 1.0) * (hi - lo);
        if stepped {
            plain = plain.round();
        }
        self.auto.push((offset, id, plain));
        // A sleeping instance takes it in the UI thread's flush.
        self.host_data.attention.raise();
    }

    /// Adds this chunk's automation values (time < `n`) to `pvals` after the host edits, in time order, and moves the
    /// later ones on by `n` frames. Audio thread: sorts in place, no allocation.
    pub(crate) fn take_automation(&mut self, n: u32) {
        // Stable insertion sort by time: a newer value for the same frame stays after the older one.
        for i in 1..self.auto.len() {
            let mut j = i;
            while j > 0 && self.auto[j - 1].0 > self.auto[j].0 {
                self.auto.swap(j - 1, j);
                j -= 1;
            }
        }
        let mut keep = 0;
        for i in 0..self.auto.len() {
            let (t, id, v) = self.auto[i];
            if t < n {
                if self.pvals.len() < self.pvals.capacity() {
                    self.pvals.push(value_event(t, id, v));
                }
            } else {
                self.auto[keep] = (t - n, id, v);
                keep += 1;
            }
        }
        self.auto.truncate(keep);
    }

    /// Main thread, every `idle`: re-reads the parameter list after a rescan request, and hands automation values
    /// that no process call took (the instance is bypassed or not playing) to the next flush, newest per parameter.
    pub(crate) fn idle_params(&mut self) {
        let rescan = self.host_data.rescan.swap(0, std::sync::atomic::Ordering::AcqRel);
        if rescan & (CLAP_PARAM_RESCAN_INFO | CLAP_PARAM_RESCAN_ALL) != 0 {
            self.ranges = unsafe { read_ranges(self.params, self.plugin) };
        }
        if !self.processed && !self.auto.is_empty() {
            for (_, id, v) in self.auto.drain(..) {
                self.param_queue.retain(|q| q.0 != id);
                self.param_queue.push((id, v));
            }
        }
        self.processed = false;
        // Edits still waiting: look again on the next tick.
        if !self.auto.is_empty() || !self.param_queue.is_empty() {
            self.host_data.attention.raise();
        }
    }
}
