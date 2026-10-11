//! MIDI controller input: every MIDI input device is opened at start, and note messages play the selected channel like
//! the typing keyboard does. Device choice and other messages (controllers, pitch bend) are not handled yet.
//!
// spec: ui/midi-input.md (OpenStudio design)

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
    #[cfg(windows)]
    /// Open devices; dropping one closes it.
    _ports: Vec<win::Port>,
}

impl MidiIn {
    /// Opens every MIDI input device (a device another program holds is skipped). `wake` is called from the driver's
    /// thread after each note, so the UI handles it at once.
    pub fn open(wake: fn()) -> MidiIn {
        let (tx, rx) = channel();
        #[cfg(windows)]
        {
            let (ports, names) = win::open_all(&tx, wake);
            MidiIn { rx, names, _ports: ports }
        }
        #[cfg(not(windows))]
        {
            let _ = (tx, wake);
            MidiIn { rx, names: Vec::new() }
        }
    }

    /// No devices: for tests and screenshots.
    pub fn none() -> MidiIn {
        let (_, rx) = channel();
        MidiIn {
            rx,
            names: Vec::new(),
            #[cfg(windows)]
            _ports: Vec::new(),
        }
    }

    /// Notes received since the last call, oldest first.
    pub fn poll(&self) -> Vec<Msg> {
        self.rx.try_iter().collect()
    }
}

#[cfg(windows)]
mod win {
    use super::{parse, Msg, Sender};
    use windows_sys::Win32::Media::Audio::{
        midiInClose, midiInGetDevCapsW, midiInGetNumDevs, midiInOpen, midiInReset, midiInStart, midiInStop, CALLBACK_FUNCTION, HMIDIIN,
        MIDIINCAPSW,
    };

    /// MM_MIM_DATA: a short message arrived (mmsystem.h).
    const MIM_DATA: u32 = 0x3c3;

    /// What the driver's callback gets: where notes go and how to wake the UI.
    struct Sink {
        tx: Sender<Msg>,
        wake: fn(),
    }

    pub struct Port {
        handle: HMIDIIN,
        sink: *mut Sink,
    }

    // The handle and the sink are only touched again when the port is closed.
    unsafe impl Send for Port {}

    impl Drop for Port {
        fn drop(&mut self) {
            // SAFETY: the handle came from midiInOpen; after midiInClose the driver makes no more callbacks, so the
            // sink can be freed.
            unsafe {
                midiInStop(self.handle);
                midiInReset(self.handle);
                midiInClose(self.handle);
                drop(Box::from_raw(self.sink));
            }
        }
    }

    /// Runs on the driver's thread: only queues the note and posts a wake message (WinMM callbacks must stay that small).
    unsafe extern "system" fn callback(_h: HMIDIIN, msg: u32, instance: usize, p1: usize, _p2: usize) {
        if msg != MIM_DATA || instance == 0 {
            return;
        }
        // SAFETY: `instance` is the `Sink` this port was opened with; it lives until the port is closed.
        let sink = unsafe { &*(instance as *const Sink) };
        if let Some(m) = parse(p1 as u8, (p1 >> 8) as u8, (p1 >> 16) as u8) {
            if sink.tx.send(m).is_ok() {
                (sink.wake)();
            }
        }
    }

    pub fn open_all(tx: &Sender<Msg>, wake: fn()) -> (Vec<Port>, Vec<String>) {
        let (mut ports, mut names) = (Vec::new(), Vec::new());
        // SAFETY: plain WinMM calls; every opened handle is owned by a `Port`, which closes it.
        unsafe {
            for id in 0..midiInGetNumDevs() {
                let mut caps = MIDIINCAPSW::default();
                if midiInGetDevCapsW(id as usize, &mut caps, std::mem::size_of::<MIDIINCAPSW>() as u32) != 0 {
                    continue;
                }
                let raw = caps.szPname;
                let name = String::from_utf16_lossy(&raw[..raw.iter().position(|&c| c == 0).unwrap_or(raw.len())]);
                let sink = Box::into_raw(Box::new(Sink { tx: tx.clone(), wake }));
                let mut handle: HMIDIIN = std::ptr::null_mut();
                if midiInOpen(&mut handle, id, callback as *const () as usize, sink as usize, CALLBACK_FUNCTION) != 0 {
                    drop(Box::from_raw(sink));
                    continue;
                }
                let port = Port { handle, sink };
                if midiInStart(handle) != 0 {
                    continue; // dropped: closed again
                }
                ports.push(port);
                names.push(name);
            }
        }
        (ports, names)
    }
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

    /// Notes sent to a loopback MIDI port (an output that is also an input, such as a loopMIDI port) arrive and wake
    /// the UI. Needs such a port: `cargo test -p os-app midi_loopback -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn midi_loopback() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use windows_sys::Win32::Media::Audio::{midiOutClose, midiOutGetDevCapsW, midiOutGetNumDevs, midiOutOpen, midiOutShortMsg, HMIDIOUT, MIDIOUTCAPSW};
        static WAKES: AtomicUsize = AtomicUsize::new(0);
        fn wake() {
            WAKES.fetch_add(1, Ordering::Relaxed);
        }
        let midi = MidiIn::open(wake);
        // SAFETY: plain WinMM calls on a handle closed below.
        unsafe {
            let out = (0..midiOutGetNumDevs()).find(|&id| {
                let mut caps = MIDIOUTCAPSW::default();
                midiOutGetDevCapsW(id as usize, &mut caps, std::mem::size_of::<MIDIOUTCAPSW>() as u32);
                let raw = caps.szPname;
                let name = String::from_utf16_lossy(&raw[..raw.iter().position(|&c| c == 0).unwrap_or(raw.len())]);
                midi.names.contains(&name)
            });
            let Some(out) = out else { panic!("no loopback port among the inputs {:?}", midi.names) };
            let mut h: HMIDIOUT = std::ptr::null_mut();
            assert_eq!(midiOutOpen(&mut h, out, 0, 0, 0), 0);
            midiOutShortMsg(h, 0x90 | (60 << 8) | (100 << 16));
            midiOutShortMsg(h, 0x80 | (60 << 8));
            std::thread::sleep(std::time::Duration::from_millis(200));
            midiOutClose(h);
        }
        assert_eq!(midi.poll(), vec![Msg::On(60, 100), Msg::Off(60)]);
        assert!(WAKES.load(Ordering::Relaxed) >= 2, "each note wakes the UI");
    }
}
