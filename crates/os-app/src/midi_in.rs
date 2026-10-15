// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! MIDI controller input: every MIDI input device is opened at start, and note messages play the selected channel like
//! the typing keyboard does. Device choice and other messages (controllers, pitch bend) are not handled yet.
//!
// spec: ui/midi-input.md (Lemon Studio design)

use std::sync::mpsc::{channel, Receiver, Sender};

/// A note message from a controller (any MIDI channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    /// Key and velocity (1..=127).
    On(u8, u8),
    Off(u8),
}

/// The note event in a short MIDI message (status, data 1, data 2), if it is one. Note on with velocity 0 is a note off.
pub fn parse(status: u8, d1: u8, d2: u8) -> Option<Msg> {
    match status & 0xf0 {
        0x90 if d2 & 0x7f > 0 => Some(Msg::On(d1 & 0x7f, d2 & 0x7f)),
        0x80 | 0x90 => Some(Msg::Off(d1 & 0x7f)),
        _ => None,
    }
}

/// The open input devices and the notes they sent.
pub struct MidiIn {
    rx: Receiver<Msg>,
    /// Names of the devices that opened.
    pub names: Vec<String>,
    /// Open devices; dropping one closes it.
    _ports: Vec<midir::MidiInputConnection<()>>,
}

impl MidiIn {
    /// Opens every MIDI input device (a device another program holds is skipped). `wake` is called from the driver's
    /// thread after each note, so the UI handles it at once. Devices come through `midir`: WinMM on Windows, ALSA on
    /// Linux, CoreMIDI on macOS, Web MIDI in a browser.
    pub fn open(wake: fn()) -> MidiIn {
        let (tx, rx) = channel();
        let (ports, names) = open_all(&tx, wake);
        MidiIn { rx, names, _ports: ports }
    }

    /// No devices: for tests and screenshots.
    pub fn none() -> MidiIn {
        let (_, rx) = channel();
        MidiIn { rx, names: Vec::new(), _ports: Vec::new() }
    }

    /// Notes received since the last call, oldest first.
    pub fn poll(&self) -> Vec<Msg> {
        self.rx.try_iter().collect()
    }
}

/// The name Lemon Studio's connections show in the system's MIDI routing (ALSA / JACK clients).
const CLIENT: &str = "Lemon Studio";

/// Connects to every input port. A connection takes its `MidiInput` along, so each port gets its own.
fn open_all(tx: &Sender<Msg>, wake: fn()) -> (Vec<midir::MidiInputConnection<()>>, Vec<String>) {
    let (mut conns, mut names) = (Vec::new(), Vec::new());
    let Ok(list) = midir::MidiInput::new(CLIENT) else { return (conns, names) };
    for port in list.ports() {
        let Ok(name) = list.port_name(&port) else { continue };
        let Ok(mut input) = midir::MidiInput::new(CLIENT) else { break };
        // Clock, active sensing and SysEx never play notes: the driver drops them.
        input.ignore(midir::Ignore::All);
        let Some(port) = input.find_port_by_id(&port.id()) else { continue };
        let tx = tx.clone();
        // Runs on the driver's thread: only queues the note and wakes the UI.
        let note = move |_stamp: u64, bytes: &[u8], _: &mut ()| {
            if let [status, d1, d2, ..] = *bytes {
                if let Some(m) = parse(status, d1, d2) {
                    if tx.send(m).is_ok() {
                        wake();
                    }
                }
            }
        };
        if let Ok(conn) = input.connect(&port, CLIENT, note, ()) {
            conns.push(conn);
            names.push(name);
        }
    }
    (conns, names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_messages() {
        assert_eq!(parse(0x90, 60, 100), Some(Msg::On(60, 100)));
        assert_eq!(parse(0x93, 61, 1), Some(Msg::On(61, 1)), "any MIDI channel");
        assert_eq!(parse(0x90, 60, 0), Some(Msg::Off(60)), "velocity 0 = note off");
        assert_eq!(parse(0x85, 60, 64), Some(Msg::Off(60)));
        assert_eq!(parse(0xb0, 7, 100), None, "controllers are not handled yet");
        assert_eq!(parse(0xf8, 0, 0), None, "clock");
    }

    /// Notes sent to a loopback MIDI port (an output that is also an input: a loopMIDI port on Windows, ALSA's
    /// "Midi Through" on Linux) arrive and wake the UI. Needs such a port: `cargo test -p os-app midi_loopback --
    /// --ignored`.
    #[test]
    #[ignore]
    fn midi_loopback() { // This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!
        use std::sync::atomic::{AtomicUsize, Ordering};
        static WAKES: AtomicUsize = AtomicUsize::new(0);
        fn wake() {
            WAKES.fetch_add(1, Ordering::Relaxed);
        }
        let midi = MidiIn::open(wake);
        let out = midir::MidiOutput::new("Lemon Studio test").expect("MIDI output");
        let port = out.ports().into_iter().find(|p| out.port_name(p).is_ok_and(|n| midi.names.contains(&n)));
        let Some(port) = port else { panic!("no loopback port among the inputs {:?}", midi.names) };
        let mut conn = out.connect(&port, "loopback").expect("open the loopback output");
        conn.send(&[0x90, 60, 100]).unwrap();
        conn.send(&[0x80, 60, 0]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        conn.close();
        assert_eq!(midi.poll(), vec![Msg::On(60, 100), Msg::Off(60)]);
        assert!(WAKES.load(Ordering::Relaxed) >= 2, "each note wakes the UI");
    }
}
