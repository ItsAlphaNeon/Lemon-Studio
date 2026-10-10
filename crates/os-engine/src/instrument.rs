//! Generator plugins as engine instruments: a channel can be played by an external instrument (a hosted CLAP plugin)
//! instead of a sample. The instance outlives engines (OpenStudio rebuilds the engine on every edit), so it is shared
//! behind a mutex; the audio thread only ever `try_lock`s it.
//!
// spec: plugins/plugin-formats.md (hosting: threading)

use std::fmt;
use std::sync::{Arc, Mutex};

/// A note-driven stereo generator.
pub trait Instrument: Send {
    /// Starts a note `offset` frames into the next `process` call. Velocity 0..=1 (FL velocity / 128).
    fn note_on(&mut self, offset: u32, key: u8, velocity: f32);
    /// Ends a note `offset` frames into the next `process` call.
    fn note_off(&mut self, offset: u32, key: u8);
    /// Renders `left.len()` frames at `rate`, overwriting both buffers.
    fn process(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]);
    /// True while the instrument still has sound to deliver (ringing voices, a long sample): the engine keeps
    /// processing it instead of stopping after its idle tail. Valid after `process`.
    fn active(&self) -> bool {
        false
    }
    /// Silences the instrument at once: every voice, tail and queued note is dropped (Stop). Called with no
    /// stream processing the instrument.
    fn reset(&mut self) {}
}

pub type SharedInstrument = Arc<Mutex<dyn Instrument>>;

/// A channel's instrument, if it has one.
#[derive(Clone, Default)]
pub struct Slot(pub Option<SharedInstrument>);

impl fmt::Debug for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "Slot(instrument)" } else { "Slot(none)" })
    }
}
