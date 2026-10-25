// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! MIDI devices: the input and output devices the system lists, the enabled inputs opened (a device seen for the
//! first time is enabled), their messages queued for the UI, a watcher that notices devices added or removed, and the
//! external clock of the device chosen for External clock sync, followed on the driver's thread. What the messages do
//! is `crate::midi_play`; what goes out is `crate::midi_out`.
//!
// spec: ui/midi-settings.md §2, §8; ui/midi-input.md §3

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A message from a controller, its MIDI channel 0..15 where it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    /// Key, velocity 1..=127.
    On { ch: u8, key: u8, vel: u8 },
    /// Key, release velocity (64 when the device sends none: a note on with velocity 0).
    Off { ch: u8, key: u8, vel: u8 },
    /// Controller number, value.
    Cc { ch: u8, n: u8, v: u8 },
    /// Pitch bend 0..=16383, 8192 = centre.
    Bend { ch: u8, v: u16 },
    /// Channel pressure.
    Pressure { ch: u8, v: u8 },
    /// Key pressure.
    Poly { ch: u8, key: u8, v: u8 },
    Program { ch: u8, p: u8 },
    Start,
    Continue,
    Stop,
    /// Song Position Pointer, in 16ths.
    Spp(u16),
}

/// The message in `bytes` (one whole short message), if it is one Lemon Studio uses. Note on with velocity 0 is a note
/// off; clock, active sensing, SysEx and MTC are not messages here (the clock is `ExtClock`'s).
pub fn parse(bytes: &[u8]) -> Option<Msg> {
    let (&status, rest) = bytes.split_first()?;
    let d1 = rest.first().map_or(0, |b| b & 0x7f);
    let d2 = rest.get(1).map_or(0, |b| b & 0x7f);
    let ch = status & 0x0f;
    Some(match status {
        0x80..=0x8f => Msg::Off { ch, key: d1, vel: d2 },
        0x90..=0x9f if d2 == 0 => Msg::Off { ch, key: d1, vel: 64 },
        0x90..=0x9f => Msg::On { ch, key: d1, vel: d2 },
        0xa0..=0xaf => Msg::Poly { ch, key: d1, v: d2 },
        0xb0..=0xbf => Msg::Cc { ch, n: d1, v: d2 },
        0xc0..=0xcf => Msg::Program { ch, p: d1 },
        0xd0..=0xdf => Msg::Pressure { ch, v: d1 },
        0xe0..=0xef => Msg::Bend { ch, v: u16::from(d1) | u16::from(d2) << 7 },
        0xf2 => Msg::Spp(u16::from(d1) | u16::from(d2) << 7),
        0xfa => Msg::Start,
        0xfb => Msg::Continue,
        0xfc => Msg::Stop,
        _ => return None,
    })
}

impl Msg {
    /// The message as sent on to an instrument, on MIDI channel 1 (spec §4): not notes, not the clock's.
    pub fn bytes(&self) -> Option<[u8; 3]> {
        Some(match *self {
            Msg::Cc { n, v, .. } => [0xb0, n, v],
            Msg::Bend { v, .. } => [0xe0, (v & 0x7f) as u8, (v >> 7 & 0x7f) as u8],
            Msg::Pressure { v, .. } => [0xd0, v, 0],
            Msg::Poly { key, v, .. } => [0xa0, key, v],
            Msg::Program { p, .. } => [0xc0, p, 0],
            _ => return None,
        })
    }
}

/// A message and the input device it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ev {
    pub dev: Arc<str>,
    pub msg: Msg,
}

/// An input device as Settings > MIDI lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InPort {
    pub name: String,
    /// Open now.
    pub open: bool,
    /// Enabled, but it would not open (another program holds it).
    pub failed: bool,
}

/// The external clock: the chosen device's clock messages as they come (driver thread), read by the UI.
#[derive(Default)]
struct ClockState {
    device: Option<String>,
    /// The last clocks' arrival times.
    times: VecDeque<Instant>,
    /// Clocks since Start (or the last Song Position Pointer).
    count: u64,
    /// Between Start / Continue and Stop.
    running: bool,
}

/// Clocks the tempo is averaged over (one quarter note).
const CLOCKS_PER_BEAT: usize = 24;
/// No clock for this long: the clock is gone.
const CLOCK_GONE: Duration = Duration::from_millis(500);

/// What External clock sync sees (spec §8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtStatus {
    /// A device is chosen.
    pub on: bool,
    /// Its Start runs and its clock comes: the beat, from 1.
    pub beat: Option<u64>,
    /// The tempo of its clock, when it comes.
    pub tempo: Option<f64>,
}

impl ExtStatus {
    /// The status line: "off", "ready", "running [Beat n]".
    pub fn text(&self) -> String {
        match (self.on, self.beat) {
            (false, _) => "Status: off".into(),
            (true, Some(b)) => format!("Status: running [Beat {b}]"),
            (true, None) => "Status: ready".into(),
        }
    }
}

#[derive(Default)]
struct ExtClock(Mutex<ClockState>);

impl ExtClock {
    /// Driver thread: a real-time message from `dev`. Returns whether it is the chosen device's.
    fn feed(&self, dev: &str, status: u8, bytes: &[u8], wake: fn()) -> bool {
        let Ok(mut c) = self.0.lock() else { return false };
        if c.device.as_deref() != Some(dev) {
            return false;
        }
        match status {
            0xf8 => {
                if c.times.len() > CLOCKS_PER_BEAT {
                    c.times.pop_front();
                }
                c.times.push_back(Instant::now());
                if c.running {
                    c.count += 1;
                    // A beat: the status line and the tempo move on.
                    if c.count % CLOCKS_PER_BEAT as u64 == 0 {
                        wake();
                    }
                }
            }
            0xfa => (c.running, c.count) = (true, 0),
            0xfb => c.running = true,
            0xfc => c.running = false,
            0xf2 => {
                let v = u64::from(bytes.get(1).map_or(0, |b| b & 0x7f)) | u64::from(bytes.get(2).map_or(0, |b| b & 0x7f)) << 7;
                c.count = v * 6;
            }
            _ => {}
        }
        true
    }

    fn status(&self) -> ExtStatus {
        let Ok(c) = self.0.lock() else { return ExtStatus { on: false, beat: None, tempo: None } };
        let fresh = c.times.back().is_some_and(|t| t.elapsed() < CLOCK_GONE);
        let tempo = (fresh && c.times.len() > 2).then(|| {
            let span = c.times.back().zip(c.times.front()).map_or(0.0, |(b, a)| b.duration_since(*a).as_secs_f64());
            60.0 / (span / (c.times.len() - 1) as f64 * CLOCKS_PER_BEAT as f64)
        });
        ExtStatus { on: c.device.is_some(), beat: (c.running && fresh).then_some(c.count / CLOCKS_PER_BEAT as u64 + 1), tempo: tempo.filter(|t| t.is_finite()) }
    }

    fn choose(&self, dev: Option<String>) {
        if let Ok(mut c) = self.0.lock() {
            if c.device != dev {
                *c = ClockState { device: dev, ..ClockState::default() };
            }
        }
    }
}

/// The MIDI devices and the messages of the open inputs.
pub struct MidiIn {
    tx: Sender<Ev>,
    rx: Receiver<Ev>,
    wake: fn(),
    /// Every input device the system lists, in its order.
    pub inputs: Vec<InPort>,
    /// Every output device's name.
    pub outputs: Vec<String>,
    /// Open inputs by name; dropping one closes it.
    conns: Vec<(String, midir::MidiInputConnection<()>)>,
    clock: Arc<ExtClock>,
    /// Set by the watcher when the device lists changed.
    changed: Arc<AtomicBool>,
}

/// The name Lemon Studio's connections show in the system's MIDI routing (ALSA / JACK clients).
pub const CLIENT: &str = "Lemon Studio";

/// How often the watcher lists the devices.
const WATCH: Duration = Duration::from_secs(2);

/// Device names, two of the same name told apart by " #2", " #3" in list order (spec §2).
pub fn unique(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for n in names {
        let seen = out.iter().filter(|o| **o == n || o.starts_with(&format!("{n} #"))).count();
        out.push(if seen == 0 { n } else { format!("{n} #{}", seen + 1) });
    }
    out
}

fn input_names() -> Vec<String> {
    let Ok(m) = midir::MidiInput::new(CLIENT) else { return Vec::new() };
    unique(m.ports().iter().map(|p| m.port_name(p).unwrap_or_default()).collect())
}

fn output_names() -> Vec<String> {
    let Ok(m) = midir::MidiOutput::new(CLIENT) else { return Vec::new() };
    unique(m.ports().iter().map(|p| m.port_name(p).unwrap_or_default()).collect())
}

impl MidiIn {
    /// Lists the devices, opens the enabled inputs and starts the watcher. `wake` is called from the driver's thread
    /// after each message the UI should handle, and from the watcher when devices come or go. Devices come through
    /// `midir`: WinMM on Windows, ALSA on Linux, CoreMIDI on macOS.
    pub fn open(wake: fn()) -> MidiIn {
        let mut m = MidiIn::none();
        m.wake = wake;
        m.refresh();
        let changed = m.changed.clone();
        let (ins, outs) = (m.inputs.iter().map(|i| i.name.clone()).collect::<Vec<_>>(), m.outputs.clone());
        let _ = std::thread::Builder::new().name("midi-watch".into()).spawn(move || {
            let mut last = (ins, outs);
            loop {
                std::thread::sleep(WATCH);
                let now = (input_names(), output_names());
                if now != last {
                    last = now;
                    changed.store(true, Ordering::Release);
                    wake();
                }
            }
        });
        m
    }

    /// No devices: for tests and screenshots.
    pub fn none() -> MidiIn {
        fn nothing() {}
        let (tx, rx) = channel();
        MidiIn { tx, rx, wake: nothing, inputs: Vec::new(), outputs: Vec::new(), conns: Vec::new(), clock: Arc::default(), changed: Arc::default() }
    }

    /// Messages received since the last call, oldest first.
    pub fn poll(&self) -> Vec<Ev> {
        self.rx.try_iter().collect()
    }

    /// The watcher saw devices come or go since the last call.
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    /// Lists the devices again and opens / closes inputs as their Enable says (`crate::midi_cfg`). Returns the
    /// inputs that opened now.
    pub fn refresh(&mut self) -> Vec<String> {
        let names = input_names();
        self.outputs = output_names();
        self.conns.retain(|(n, _)| names.contains(n) && crate::midi_cfg::input(n).enabled);
        let mut opened = Vec::new();
        let mut inputs = Vec::with_capacity(names.len());
        for (i, name) in names.iter().enumerate() {
            let want = crate::midi_cfg::input(name).enabled;
            let mut failed = false;
            if want && !self.conns.iter().any(|(n, _)| n == name) {
                match self.connect(i, name) {
                    Some(c) => {
                        self.conns.push((name.clone(), c));
                        opened.push(name.clone());
                    }
                    None => failed = true,
                }
            }
            inputs.push(InPort { name: name.clone(), open: self.conns.iter().any(|(n, _)| n == name), failed });
        }
        self.inputs = inputs;
        opened
    }

    /// Opens the `i`-th input (named `name` in `unique`'s list).
    fn connect(&self, i: usize, name: &str) -> Option<midir::MidiInputConnection<()>> {
        let mut input = midir::MidiInput::new(CLIENT).ok()?;
        // SysEx and active sensing never reach the UI; the clock does (External clock sync).
        input.ignore(midir::Ignore::SysexAndActiveSense);
        let port = input.ports().into_iter().nth(i)?;
        let (tx, wake, clock): (Sender<Ev>, fn(), Arc<ExtClock>) = (self.tx.clone(), self.wake, self.clock.clone());
        let dev: Arc<str> = Arc::from(name);
        // Runs on the driver's thread: only queues the message and wakes the UI.
        let take = move |_stamp: u64, bytes: &[u8], _: &mut ()| {
            let Some(&status) = bytes.first() else { return };
            if status >= 0xf0 && !clock.feed(&dev, status, bytes, wake) {
                // Real-time and system messages of other devices are not used.
                return;
            }
            if let Some(msg) = parse(bytes) {
                if tx.send(Ev { dev: dev.clone(), msg }).is_ok() {
                    wake();
                }
            }
        };
        input.connect(&port, CLIENT, take, ()).ok()
    }

    /// External clock sync follows `device`'s clock (`None`: off).
    pub fn set_ext_sync(&self, device: Option<&str>) {
        self.clock.choose(device.map(String::from));
    }

    pub fn ext_status(&self) -> ExtStatus {
        self.clock.status()
    }

    /// Names of the open inputs (the loopback test).
    #[cfg(test)]
    pub fn open_names(&self) -> Vec<String> {
        self.inputs.iter().filter(|i| i.open).map(|i| i.name.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages() {
        assert_eq!(parse(&[0x90, 60, 100]), Some(Msg::On { ch: 0, key: 60, vel: 100 }));
        assert_eq!(parse(&[0x93, 61, 1]), Some(Msg::On { ch: 3, key: 61, vel: 1 }), "any MIDI channel");
        assert_eq!(parse(&[0x90, 60, 0]), Some(Msg::Off { ch: 0, key: 60, vel: 64 }), "velocity 0 = note off");
        assert_eq!(parse(&[0x85, 60, 30]), Some(Msg::Off { ch: 5, key: 60, vel: 30 }));
        assert_eq!(parse(&[0xb0, 7, 100]), Some(Msg::Cc { ch: 0, n: 7, v: 100 }));
        assert_eq!(parse(&[0xe1, 0, 64]), Some(Msg::Bend { ch: 1, v: 8192 }));
        assert_eq!(parse(&[0xe0, 0x7f, 0x7f]), Some(Msg::Bend { ch: 0, v: 16383 }));
        assert_eq!(parse(&[0xd2, 99]), Some(Msg::Pressure { ch: 2, v: 99 }));
        assert_eq!(parse(&[0xc0, 5]), Some(Msg::Program { ch: 0, p: 5 }));
        assert_eq!(parse(&[0xf2, 0x10, 0x01]), Some(Msg::Spp(144)));
        assert_eq!(parse(&[0xfa]), Some(Msg::Start));
        assert_eq!(parse(&[0xf8]), None, "the clock is not a message here");
        assert_eq!(Msg::Bend { ch: 4, v: 8192 }.bytes(), Some([0xe0, 0, 64]), "sent on channel 1");
        assert_eq!(Msg::On { ch: 0, key: 1, vel: 2 }.bytes(), None);
    }

    #[test]
    fn same_names_are_numbered() {
        let n = unique(vec!["A".into(), "B".into(), "A".into(), "A".into()]);
        assert_eq!(n, vec!["A", "B", "A #2", "A #3"]);
    }

    #[test]
    fn external_clock() {
        fn nothing() {}
        let c = ExtClock::default();
        assert_eq!(c.status().text(), "Status: off");
        assert!(!c.feed("Keys", 0xf8, &[0xf8], nothing), "no device chosen");
        c.choose(Some("Keys".into()));
        assert_eq!(c.status().text(), "Status: ready");
        c.feed("Keys", 0xfa, &[0xfa], nothing);
        for _ in 0..30 {
            c.feed("Keys", 0xf8, &[0xf8], nothing);
            std::thread::sleep(Duration::from_millis(2));
        }
        let s = c.status();
        assert_eq!(s.beat, Some(2), "30 clocks after Start: beat 2");
        assert!(s.tempo.is_some_and(|t| t > 100.0 && t < 1300.0), "{s:?}");
        c.feed("Keys", 0xfc, &[0xfc], nothing);
        assert_eq!(c.status().text(), "Status: ready");
        assert!(!c.feed("Other", 0xfa, &[0xfa], nothing), "another device's clock is not followed");
    }

    /// Messages sent to a loopback MIDI port (an output that is also an input: a loopMIDI port on Windows, ALSA's
    /// "Midi Through" on Linux) arrive and wake the UI. Needs such a port: `cargo test -p os-app midi_loopback --
    /// --ignored`.
    #[test]
    #[ignore]
    fn midi_loopback() { // This code is a part of L e m o n S t u d i o.
        use std::sync::atomic::{AtomicUsize, Ordering};
        static WAKES: AtomicUsize = AtomicUsize::new(0);
        fn wake() {
            WAKES.fetch_add(1, Ordering::Relaxed);
        }
        let midi = MidiIn::open(wake);
        let names = midi.open_names();
        let out = midir::MidiOutput::new("Lemon Studio test").expect("MIDI output");
        let port = out.ports().into_iter().find(|p| out.port_name(p).is_ok_and(|n| names.contains(&n)));
        let Some(port) = port else { panic!("no loopback port among the inputs {names:?}") };
        let mut conn = out.connect(&port, "loopback").expect("open the loopback output");
        conn.send(&[0x90, 60, 100]).unwrap();
        conn.send(&[0xb0, 1, 64]).unwrap();
        conn.send(&[0x80, 60, 0]).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        conn.close();
        let got: Vec<Msg> = midi.poll().into_iter().map(|e| e.msg).collect();
        assert_eq!(got, vec![Msg::On { ch: 0, key: 60, vel: 100 }, Msg::Cc { ch: 0, n: 1, v: 64 }, Msg::Off { ch: 0, key: 60, vel: 0 }]);
        assert!(WAKES.load(Ordering::Relaxed) >= 3, "each message wakes the UI");
    }
}
