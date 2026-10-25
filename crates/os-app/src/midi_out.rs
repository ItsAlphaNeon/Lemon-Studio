// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! MIDI output: master sync (MIDI clock or MTC) to the output devices with Send master sync, and all notes off. One
//! thread owns the open outputs; it watches the playback (`transport::Watch`) and times the clock itself, from the
//! playback's position and tempo at each start.
//!
// spec: ui/midi-settings.md §2, §7

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use crate::transport::{State, Watch};

/// Synchronization type (spec §7): the choices of `prefs::MIDI_SYNC_TYPE`, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncKind {
    Clock,
    /// Frames per second and the MTC rate code (0 = 24, 1 = 25, 2 = 30 drop, 3 = 30).
    Mtc(f64, u8),
}

impl SyncKind {
    /// The kind for entry `i` of the Synchronization type combo.
    pub fn of(i: usize) -> SyncKind {
        match i {
            1 => SyncKind::Mtc(24.0, 0),
            2 => SyncKind::Mtc(25.0, 1),
            3 => SyncKind::Mtc(30.0, 2),
            4 => SyncKind::Mtc(30.0, 3),
            5 => SyncKind::Mtc(30000.0 / 1001.0, 2),
            6 => SyncKind::Mtc(30000.0 / 1001.0, 3),
            _ => SyncKind::Clock,
        }
    }
}

/// What the output thread does, as the settings say.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Output devices that get the master sync (open), with their Send all notes off.
    pub sync: Vec<(String, bool)>,
    pub kind: SyncKind,
    /// Master sync offset, milliseconds.
    pub offset_ms: i32,
    /// The project's ticks per quarter note and tempo (until playback reports its own).
    pub ppq: u16,
    pub tempo: f64,
}

impl Default for Config {
    fn default() -> Config {
        Config { sync: Vec::new(), kind: SyncKind::Clock, offset_ms: 0, ppq: 96, tempo: 120.0 }
    }
}

enum Cmd {
    Config(Config),
    NotesOff,
}

/// The output thread's handle.
pub struct MidiOut {
    tx: Option<Sender<Cmd>>,
    last: Config,
}

impl MidiOut {
    /// Starts the output thread, which follows `watch`.
    pub fn start(watch: Watch) -> MidiOut {
        let (tx, rx) = channel();
        let ok = std::thread::Builder::new().name("midi-out".into()).spawn(move || run(rx, watch)).is_ok();
        MidiOut { tx: ok.then_some(tx), last: Config::default() }
    }

    /// No thread: tests and screenshots.
    pub fn none() -> MidiOut {
        MidiOut { tx: None, last: Config::default() }
    }

    /// The settings changed (sent only when they differ from the last).
    pub fn configure(&mut self, c: Config) {
        if c != self.last {
            self.last = c.clone();
            if let Some(tx) = &self.tx {
                let _ = tx.send(Cmd::Config(c));
            }
        }
    }

    /// All notes off to the open outputs that want it (after an export).
    pub fn notes_off(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::NotesOff);
        }
    }
}

/// Controller 123 on all 16 channels.
fn all_notes_off(c: &mut midir::MidiOutputConnection) {
    for ch in 0..16u8 {
        let _ = c.send(&[0xb0 | ch, 123, 0]);
    }
}

/// Opens output `name` (as `midi_in::unique` lists it).
fn connect(name: &str) -> Option<midir::MidiOutputConnection> {
    let out = midir::MidiOutput::new(crate::midi_in::CLIENT).ok()?;
    let ports = out.ports();
    let names = crate::midi_in::unique(ports.iter().map(|p| out.port_name(p).unwrap_or_default()).collect());
    let i = names.iter().position(|n| n == name)?;
    out.connect(&ports[i], crate::midi_in::CLIENT).ok()
}

/// The clock's timeline from the last start: the position and time it started from, at a tempo.
#[derive(Clone, Copy)]
struct Line {
    at: Instant,
    tick: f64,
    tempo: f64,
    ppq: f64,
}

impl Line {
    /// When the song reaches `tick`.
    fn time(&self, tick: f64) -> Instant {
        let secs = (tick - self.tick) / self.ppq * 60.0 / self.tempo.max(1.0);
        if secs >= 0.0 { self.at + Duration::from_secs_f64(secs) } else { self.at }
    }

    fn tick_now(&self, now: Instant) -> f64 {
        self.tick + now.saturating_duration_since(self.at).as_secs_f64() * self.tempo / 60.0 * self.ppq
    }
}

/// The running sync: its line and the next clock (MIDI clock: its number at 24 a quarter; MTC: the quarter frame).
struct Running {
    line: Line,
    next: u64,
    /// MTC: the time in seconds at the line's start, and the quarter-frame piece next.
    secs: f64,
    piece: u8,
}

fn run(rx: Receiver<Cmd>, watch: Watch) {
    let mut cfg = Config::default();
    let mut open: Vec<(String, bool, midir::MidiOutputConnection)> = Vec::new();
    let (mut was, mut generation) = (State::Stopped, watch.generation());
    let mut running: Option<Running> = None;
    // Messages due later (a positive offset delays them all).
    let mut due: Vec<(Instant, Vec<u8>)> = Vec::new();
    loop {
        let wait = if open.is_empty() { Duration::from_millis(250) } else { Duration::from_millis(1) };
        match rx.recv_timeout(wait) {
            Ok(Cmd::Config(c)) => {
                open.retain(|(n, _, _)| c.sync.iter().any(|(m, _)| m == n));
                for (name, off) in &c.sync {
                    match open.iter_mut().find(|o| &o.0 == name) {
                        Some(o) => o.1 = *off,
                        None => {
                            if let Some(mut conn) = connect(name) {
                                if *off {
                                    all_notes_off(&mut conn);
                                }
                                open.push((name.clone(), *off, conn));
                            }
                        }
                    }
                }
                if c.kind != cfg.kind || c.ppq != cfg.ppq {
                    running = None;
                    was = State::Stopped;
                }
                cfg = c;
                continue;
            }
            Ok(Cmd::NotesOff) => {
                for o in open.iter_mut().filter(|o| o.1) {
                    all_notes_off(&mut o.2);
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if open.is_empty() {
            (was, generation, running) = (watch.state(), watch.generation(), None);
            due.clear();
            continue;
        }
        let now = Instant::now();
        let shift = Duration::from_millis(u64::from(cfg.offset_ms.unsigned_abs()));
        let later = |t: Instant| if cfg.offset_ms > 0 { t + shift } else { t.checked_sub(shift).unwrap_or(t) };
        let (state, g) = (watch.state(), watch.generation());
        let ppq = f64::from(cfg.ppq.max(1));
        let tempo = watch.tempo().unwrap_or(cfg.tempo);
        let mut send = |bytes: Vec<u8>, at: Instant| due.push((at, bytes));
        let jumped = g != generation && state == State::Playing && was == State::Playing;
        if (state == State::Playing && was != State::Playing) || jumped {
            let tick = watch.tick().max(0.0);
            let at = if cfg.offset_ms > 0 { now + shift } else { now };
            if jumped {
                send(vec![0xfc], at);
            }
            let line = Line { at: now, tick, tempo, ppq };
            match cfg.kind {
                SyncKind::Clock => {
                    let sixteenth = (tick / (ppq / 4.0)).floor() as u64;
                    let spp = sixteenth.min(0x3fff) as u16;
                    send(vec![0xf2, (spp & 0x7f) as u8, (spp >> 7) as u8], at);
                    send(vec![if tick < 1.0 && was == State::Stopped { 0xfa } else { 0xfb }], at);
                    let next = (tick * 24.0 / ppq).ceil() as u64;
                    running = Some(Running { line, next, secs: 0.0, piece: 0 });
                }
                SyncKind::Mtc(fps, code) => {
                    let secs = tick / ppq * 60.0 / tempo.max(1.0);
                    send(full_frame(secs, fps, code), at);
                    running = Some(Running { line, next: 0, secs, piece: 0 });
                }
            }
        } else if state != State::Playing && was == State::Playing {
            let at = if cfg.offset_ms > 0 { now + shift } else { now };
            if cfg.kind == SyncKind::Clock {
                send(vec![0xfc], at);
                if state == State::Stopped {
                    send(vec![0xf2, 0, 0], at);
                }
            }
            running = None;
        }
        (was, generation) = (state, g);
        // A change of tempo: the line goes on from where it is.
        if let Some(r) = running.as_mut() {
            if (r.line.tempo - tempo).abs() > 1e-6 {
                let tick = r.line.tick_now(now);
                if let SyncKind::Mtc(..) = cfg.kind {
                    r.secs += now.saturating_duration_since(r.line.at).as_secs_f64();
                    r.next = 0;
                }
                r.line = Line { at: now, tick, tempo, ppq };
            }
        }
        if let Some(r) = running.as_mut() {
            match cfg.kind {
                SyncKind::Clock => {
                    while later(r.line.time(r.next as f64 * ppq / 24.0)) <= now + Duration::from_millis(1) {
                        let at = later(r.line.time(r.next as f64 * ppq / 24.0));
                        due.push((at, vec![0xf8]));
                        r.next += 1;
                    }
                }
                SyncKind::Mtc(fps, code) => {
                    let quarter = 1.0 / (fps * 4.0);
                    while later(r.line.at + Duration::from_secs_f64(r.next as f64 * quarter)) <= now + Duration::from_millis(1) {
                        let at = later(r.line.at + Duration::from_secs_f64(r.next as f64 * quarter));
                        // The 8 pieces carry the time at the first piece's frame.
                        let secs = r.secs + (r.next - u64::from(r.piece)) as f64 * quarter;
                        due.push((at, vec![0xf1, quarter_frame(secs, fps, code, r.piece)]));
                        r.piece = (r.piece + 1) % 8;
                        r.next += 1;
                    }
                }
            }
        }
        // Send what is due, in time order.
        due.sort_by_key(|d| d.0);
        let ready = due.partition_point(|d| d.0 <= now + Duration::from_millis(1));
        for (_, bytes) in due.drain(..ready) {
            for o in open.iter_mut() {
                let _ = o.2.send(&bytes);
            }
        }
    }
}

/// Hours, minutes, seconds, frames of `secs`.
fn timecode(secs: f64, fps: f64) -> (u8, u8, u8, u8) {
    let whole = secs.max(0.0);
    let frames_per_s = fps.round().max(1.0);
    let s = whole.floor() as u64;
    let f = ((whole - whole.floor()) * fps).floor().min(frames_per_s - 1.0) as u8;
    ((s / 3600 % 24) as u8, (s / 60 % 60) as u8, (s % 60) as u8, f)
}

/// The full-frame SysEx at `secs`.
fn full_frame(secs: f64, fps: f64, code: u8) -> Vec<u8> {
    let (h, m, s, f) = timecode(secs, fps);
    vec![0xf0, 0x7f, 0x7f, 0x01, 0x01, code << 5 | h, m, s, f, 0xf7]
}

/// Quarter-frame piece `piece` (0..8) of the time `secs`.
fn quarter_frame(secs: f64, fps: f64, code: u8, piece: u8) -> u8 {
    let (h, m, s, f) = timecode(secs, fps);
    let v = match piece {
        0 => f & 0x0f,
        1 => f >> 4,
        2 => s & 0x0f,
        3 => s >> 4,
        4 => m & 0x0f,
        5 => m >> 4,
        6 => h & 0x0f,
        _ => (h >> 4) | code << 1,
    };
    piece << 4 | (v & 0x0f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_messages() {
        assert_eq!(full_frame(3723.5, 25.0, 1), vec![0xf0, 0x7f, 0x7f, 0x01, 0x01, 0x20 | 1, 2, 3, 12, 0xf7]);
        // 1 h 2 min 3 s frame 12: the pieces.
        let p: Vec<u8> = (0..8).map(|i| quarter_frame(3723.5, 25.0, 1, i)).collect();
        assert_eq!(p, vec![0x0c, 0x10, 0x23, 0x30, 0x42, 0x50, 0x61, 0x72]);
        assert_eq!(SyncKind::of(0), SyncKind::Clock);
        assert_eq!(SyncKind::of(2), SyncKind::Mtc(25.0, 1));
    }

    #[test]
    fn clock_line() {
        let at = Instant::now();
        let l = Line { at, tick: 0.0, tempo: 120.0, ppq: 96.0 };
        // A quarter note at 120 BPM is half a second; a clock is a 24th of it.
        assert_eq!(l.time(96.0).duration_since(at), Duration::from_millis(500));
        assert!((l.time(4.0).duration_since(at).as_secs_f64() - 0.5 / 24.0).abs() < 1e-6);
        assert!((l.tick_now(at + Duration::from_millis(250)) - 48.0).abs() < 1e-6);
    }
}
