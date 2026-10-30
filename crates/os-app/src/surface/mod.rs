// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Control surfaces: keyboards whose buttons, pads, knobs, lights and screen Lemon Studio drives itself, as FL Studio's
//! device scripts do. A recognised keyboard's control port (its "DAW" port) is opened both ways here and is not a
//! plain MIDI input (`crate::midi_in` leaves it alone); its keys' port stays one. What the controls do is `drive`.
//!
//! - `launchkey`: the Novation Launchkey MK4 and Launchkey Mini MK4.
// spec: controllers/launchkey-mk4.md

pub mod drive;
pub mod launchkey;

use std::ops::Range;
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use launchkey::Model;

/// Whether `name` (a system MIDI port) is a control port this module opens.
pub fn claims(name: &str) -> bool {
    Model::of_port(name).is_some()
}

/// An open control port.
pub struct Port {
    pub model: Model,
    /// The input's name, as `midi_in::unique` lists it.
    pub name: String,
    rx: Receiver<Vec<u8>>,
    _input: midir::MidiInputConnection<()>,
    output: midir::MidiOutputConnection,
}

impl Port {
    /// Opens the first recognised control port, both ways. `wake` is called from the driver's thread after each
    /// message.
    pub fn open(wake: fn()) -> Option<Port> {
        let mut input = midir::MidiInput::new(crate::midi_in::CLIENT).ok()?;
        input.ignore(midir::Ignore::All);
        let ins = input.ports();
        let in_names = crate::midi_in::unique(ins.iter().map(|p| input.port_name(p).unwrap_or_default()).collect());
        let (i, model) = in_names.iter().enumerate().find_map(|(i, n)| Some((i, Model::of_port(n)?)))?;
        let output = midir::MidiOutput::new(crate::midi_in::CLIENT).ok()?;
        let outs = output.ports();
        let out_names: Vec<String> = outs.iter().map(|p| output.port_name(p).unwrap_or_default()).collect();
        // The same name both ways (ALSA), or the same keyboard's other half ("MIDIOUT2 (...)" on Windows).
        let o = out_names.iter().position(|n| *n == in_names[i]).or_else(|| out_names.iter().position(|n| Model::of_port(n) == Some(model)))?;
        let output = output.connect(&outs[o], crate::midi_in::CLIENT).ok()?;
        let (tx, rx) = channel();
        let take = move |_stamp: u64, bytes: &[u8], _: &mut ()| {
            if tx.send(bytes.to_vec()).is_ok() {
                wake();
            }
        };
        let input = input.connect(&ins[i], crate::midi_in::CLIENT, take, ()).ok()?;
        Some(Port { model, name: in_names[i].clone(), rx, _input: input, output })
    }

    /// Messages received since the last call, oldest first.
    pub fn poll(&self) -> Vec<Vec<u8>> {
        self.rx.try_iter().collect()
    }

    pub fn send(&mut self, m: &[u8]) {
        let _ = self.output.send(m);
    }

    /// Whether the port is still listed (the keyboard was not unplugged).
    pub fn listed(&self, inputs: &[crate::midi_in::InPort]) -> bool {
        inputs.iter().any(|i| i.name == self.name)
    }
}

/// The mixer tracks and rack rows a surface's faders and pads take, outlined in the mixer and the channel rack (FL's
/// controller rectangle): until a time, or with no end while the surface's Shift is held.
#[derive(Debug, Default, Clone)]
pub struct Marks {
    pub mixer: Option<(Range<usize>, Option<Instant>)>,
    pub rack: Option<(Vec<usize>, Option<Instant>)>,
}

impl Marks {
    fn live<T>(m: &Option<(T, Option<Instant>)>) -> Option<&T> {
        m.as_ref().filter(|(_, until)| until.is_none_or(|u| u > Instant::now())).map(|(t, _)| t)
    }

    /// The mixer tracks outlined now.
    pub fn mixer(&self) -> Option<Range<usize>> {
        Marks::live(&self.mixer).cloned()
    }

    /// The rack rows outlined now.
    pub fn rack(&self) -> Option<&[usize]> {
        Marks::live(&self.rack).map(|v| v.as_slice())
    }

    /// An outline is up that goes by itself: the shell keeps ticking so that it is taken down on time.
    pub fn fading(&self) -> bool {
        let timed = |u: &Option<Instant>| u.is_some_and(|u| u > Instant::now());
        self.mixer.as_ref().is_some_and(|m| timed(&m.1)) || self.rack.as_ref().is_some_and(|m| timed(&m.1))
    }
}

#[cfg(test)]
mod tests {
    use super::launchkey::{self as lk, display, Colour, In, Led, Text};
    use super::*;
    use std::time::Duration;

    /// A Launchkey MK4 plugged in: DAW mode is confirmed and the layouts reported, the pads light in the rack's
    /// colours, the screen shows text, and the keyboard goes back to its own mode. Needs the keyboard:
    /// `cargo test -p os-app launchkey_live -- --ignored`.
    #[test]
    #[ignore]
    fn launchkey_live() {
        fn nothing() {}
        let mut p = Port::open(nothing).expect("a Launchkey MK4's DAW port");
        let m = p.model;
        p.send(&lk::DAW_ON);
        p.send(&lk::DRUMS_OWNED);
        for msg in lk::START_LAYOUTS {
            p.send(&msg);
        }
        std::thread::sleep(Duration::from_millis(400));
        let got: Vec<In> = p.poll().iter().filter_map(|b| lk::parse(b)).collect();
        assert!(got.contains(&In::Daw(true)), "{got:?}");
        assert!(got.contains(&In::PadLayout(lk::Pads::Daw)), "{got:?}");
        assert!(got.contains(&In::EncoderLayout(lk::Encoders::Plugin)), "{got:?}");
        let mut lights = lk::Lights::default();
        for pad in 0..16u8 {
            let h = f32::from(pad) / 16.0;
            let rgb = os_model::Rgb::new((255.0 * (1.0 - h)) as u8, (255.0 * h) as u8, 128);
            lights.set(Led::Pad(pad), Colour::of(rgb), false);
        }
        lights.set(Led::Button(lk::button::PLAY), lk::WHITE, false);
        p.send(&lights.flush(m).expect("lights"));
        let mut screen = lk::Screen::default();
        for msg in screen.write(m, display::STATIONARY, Text::three("", "Lemon Studio", "Launchkey test"), std::time::Instant::now()) {
            p.send(&msg);
        }
        std::thread::sleep(Duration::from_millis(2500));
        lights.all_off();
        if let Some(msg) = lights.flush(m) {
            p.send(&msg);
        }
        p.send(&lk::DRUMS_RELEASED);
        p.send(&lk::DAW_OFF);
        std::thread::sleep(Duration::from_millis(300));
        let back: Vec<In> = p.poll().iter().filter_map(|b| lk::parse(b)).collect();
        assert!(back.contains(&In::Daw(false)), "{back:?}");
    }
}
