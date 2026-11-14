// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The system's MIDI ports (`midir`), for `midi_in` and `midi_out`. Without the `midi-io` feature there are none: the
//! lists are empty and nothing opens.

use crate::midi_in::CLIENT;
#[cfg(feature = "midi-io")]
use crate::midi_in::unique;

#[cfg(feature = "midi-io")]
pub use midir::{MidiInputConnection as InConn, MidiOutputConnection as OutConn};

/// No connection exists without `midi-io`.
#[cfg(not(feature = "midi-io"))]
pub enum Never {}
#[cfg(not(feature = "midi-io"))]
pub struct InConn<T>(Never, std::marker::PhantomData<T>);
#[cfg(not(feature = "midi-io"))]
pub enum OutConn {}

#[cfg(not(feature = "midi-io"))]
impl OutConn {
    pub fn send(&mut self, _bytes: &[u8]) -> Result<(), ()> {
        match *self {}
    }
}

/// Every input port's name, as the system lists them.
pub fn input_names() -> Vec<String> {
    #[cfg(feature = "midi-io")]
    {
        let Ok(m) = midir::MidiInput::new(CLIENT) else { return Vec::new() };
        m.ports().iter().map(|p| m.port_name(p).unwrap_or_default()).collect()
    }
    #[cfg(not(feature = "midi-io"))]
    Vec::new()
}

/// Every output port's name, as the system lists them.
pub fn output_names() -> Vec<String> {
    #[cfg(feature = "midi-io")]
    {
        let Ok(m) = midir::MidiOutput::new(CLIENT) else { return Vec::new() };
        m.ports().iter().map(|p| m.port_name(p).unwrap_or_default()).collect()
    }
    #[cfg(not(feature = "midi-io"))]
    Vec::new()
}

/// Opens the `i`-th input port; `take` gets each message (time stamp, bytes) on the driver's thread. SysEx and active
/// sensing are left out.
#[cfg_attr(not(feature = "midi-io"), allow(unused_variables))]
pub fn open_input(i: usize, take: impl FnMut(u64, &[u8], &mut ()) + Send + 'static) -> Option<InConn<()>> {
    #[cfg(feature = "midi-io")]
    {
        let mut input = midir::MidiInput::new(CLIENT).ok()?;
        input.ignore(midir::Ignore::SysexAndActiveSense);
        let port = input.ports().into_iter().nth(i)?;
        input.connect(&port, CLIENT, take, ()).ok()
    }
    #[cfg(not(feature = "midi-io"))]
    None
}

/// Opens output `name` (as `midi_in::unique` lists them).
#[cfg_attr(not(feature = "midi-io"), allow(unused_variables))]
pub fn open_output(name: &str) -> Option<OutConn> {
    #[cfg(feature = "midi-io")]
    {
        let out = midir::MidiOutput::new(CLIENT).ok()?;
        let ports = out.ports();
        let names = unique(ports.iter().map(|p| out.port_name(p).unwrap_or_default()).collect());
        let i = names.iter().position(|n| n == name)?;
        out.connect(&ports[i], CLIENT).ok()
    }
    #[cfg(not(feature = "midi-io"))]
    None
}
