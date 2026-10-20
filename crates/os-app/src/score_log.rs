// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The score logger (TOOLS > Dump score log to selected pattern, Clear log): every note played on the typing keyboard,
//! a MIDI keyboard or the Touch controller is kept for 30 minutes, playing or not, armed or not; a dump writes the last
//! few minutes into the current pattern at their real time offsets, unquantized.
// spec: ui/recording.md §6

use crate::app::State;
use crate::playlist::{Cmd, PItem};
use crate::record::RecCmd;
use crate::step_edit::Src;
use os_model::Note;
use std::time::{Duration, Instant};

/// How long the log keeps a note.
const KEEP: Duration = Duration::from_secs(30 * 60);

/// The dump submenu's choices, in minutes.
pub const MINUTES: [u32; 5] = [2, 5, 10, 20, 30];

/// The undo step of a dump.
pub const DUMPED: &str = "Dump score log";

/// A logged note: what was played, on which channel (`Note::channel`), when it went down and up.
#[derive(Debug, Clone, Copy)]
struct Logged {
    src: Src,
    key: u16,
    velocity: u8,
    channel: u16,
    down: Instant,
    up: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct Log {
    notes: Vec<Logged>,
}

impl Log {
    pub fn down(&mut self, src: Src, key: u16, velocity: u8, channel: u16, now: Instant) {
        self.notes.retain(|n| now.saturating_duration_since(n.down) < KEEP);
        self.up(src, now);
        self.notes.push(Logged { src, key, velocity, channel, down: now, up: None });
    }

    pub fn up(&mut self, src: Src, now: Instant) {
        for n in self.notes.iter_mut().filter(|n| n.src == src && n.up.is_none()) {
            n.up = Some(now);
        }
    }

    pub fn clear(&mut self) {
        self.notes.clear();
    }

    /// The notes that went down in the last `minutes` before `now`, as pattern notes: the first at tick 0, the rest at
    /// their time after it at `ticks_per_second`, as long as they were held (one still held: until now).
    pub fn notes(&self, minutes: u32, now: Instant, ticks_per_second: f64) -> Vec<Note> {
        let since = Duration::from_secs(u64::from(minutes) * 60);
        let recent: Vec<&Logged> = self.notes.iter().filter(|n| now.saturating_duration_since(n.down) <= since).collect();
        let Some(first) = recent.iter().map(|n| n.down).min() else { return Vec::new() };
        let ticks = |d: Duration| (d.as_secs_f64() * ticks_per_second).round() as u32;
        recent
            .iter()
            .map(|n| {
                let at = ticks(n.down - first);
                let length = ticks(n.up.unwrap_or(now).saturating_duration_since(n.down)).max(1);
                Note { length, key: n.key, velocity: n.velocity, ..Note::step(n.channel, at) }
            })
            .collect()
    }
}

/// TOOLS > Dump score log to selected pattern ▸.
pub fn items(_: &State) -> Vec<PItem> {
    MINUTES.iter().map(|&m| PItem::new(format!("Last {m} minutes"), Cmd::Rec(RecCmd::Dump(m)))).collect()
}

/// Ticks a second at the project's tempo.
pub fn ticks_per_second(s: &State) -> f64 {
    let p = &s.project;
    f64::from(p.ppq.max(1)) * p.tick_tempo(p.tempo.unwrap_or(140.0).max(1.0)) / 60.0
}
