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
    /// Starts a note `start` (0..1) of the way into the instrument's sample (a trimmed audio clip; spec:
    /// plugins/open-sampler.md §7). Instruments without a sample start position play it from the top.
    fn note_on_from(&mut self, offset: u32, key: u8, velocity: f32, start: f32) {
        let _ = start;
        self.note_on(offset, key, velocity);
    }
    /// Cuts a note at once, whatever its envelope (the end of an audio clip). Default: a note off.
    fn choke(&mut self, offset: u32, key: u8) {
        self.note_off(offset, key);
    }
    /// Tunes the sounding voices of `key` to `semitones` from the key's own pitch (portamento glides; spec:
    /// audio/channel-voices.md). Default: ignored (no glide).
    fn tune(&mut self, offset: u32, key: u8, semitones: f32) {
        let _ = (offset, key, semitones);
    }
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
    /// Sets parameter `index` (its position in the plugin's parameter list) to normalized `value` (0..1) `offset`
    /// frames into the next `process` call (automation; spec: plugins/plugin-parameters.md §2). Default: ignored.
    fn param(&mut self, offset: u32, index: u32, value: f64) {
        let _ = (offset, index, value);
    }
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

/// An insert effect plugin hosted in a mixer slot (a CLAP effect): stereo in, stereo out. Shared like instruments,
/// because the instance outlives engine rebuilds; the audio thread only `try_lock`s it (a busy instance passes the
/// sound through unchanged).
// spec: ui/mixer.md §7 (effect slots)
pub trait HostedEffect: Send {
    /// Processes `left.len()` frames at `rate` in place.
    fn process_fx(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]);
    /// Drops tails and delay lines (Stop).
    fn reset_fx(&mut self) {}
    /// [`Instrument::param`] for effects: parameter `index` to normalized `value`, `offset` frames into the next
    /// `process_fx` call.
    fn param_fx(&mut self, offset: u32, index: u32, value: f64) {
        let _ = (offset, index, value);
    }
}

pub type SharedEffect = Arc<Mutex<dyn HostedEffect>>;
