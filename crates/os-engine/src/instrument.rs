//! Generator plugins as engine instruments: a channel can be played by an external instrument (a hosted CLAP plugin)
//! instead of a sample. The instance outlives engines (Lemon Studio rebuilds the engine on every edit), so it is shared
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
    /// The voices the instrument has sounding, as far as the host knows them (spec: audio/voices.md §2). Valid after
    /// `process` and `reset`. Default: none.
    fn voices(&self) -> usize {
        0
    }
    /// Silences the instrument at once: every voice, tail and queued note is dropped (Stop). Called with no
    /// stream processing the instrument.
    fn reset(&mut self) {}
    /// Sets parameter `index` (its position in the plugin's parameter list) to normalized `value` (0..1) `offset`
    /// frames into the next `process` call (automation; spec: plugins/plugin-parameters.md §2). Default: ignored.
    fn param(&mut self, offset: u32, index: u32, value: f64) {
        let _ = (offset, index, value);
    }
    /// True for an instrument that takes automated parameter values once per tick, at offset 0 of the tick's
    /// `process` call, as the automation clip hands them on (FL native plugins: spec plugins/fruity-abi.md
    /// "Parameters"). Default false: values every few frames inside the tick (spec: plugins/plugin-parameters.md §2).
    fn tick_params(&self) -> bool {
        false
    }
    /// The channel's levels for the next `process` call. An instrument that works the volume and pan into its voices
    /// itself returns true, and the engine then adds its output unchanged (FL native generators get them in their
    /// voices' final levels: spec plugins/fruity-abi.md "Voices"). Default: false, the engine applies the volume and
    /// pan to the output (and the mod X / Y go nowhere).
    fn channel_levels(&mut self, levels: ChannelLevels) -> bool {
        let _ = levels;
        false
    }
    /// Where the song is during the next `process` call (playback only; without a call the transport is stopped).
    /// Default: ignored.
    fn transport(&mut self, info: &TransportInfo) {
        let _ = info;
    }
}

/// A channel's levels as its instrument gets them (spec: audio/voice-levels.md "The channel's levels").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelLevels {
    /// A gain: the level curve of the volume times that of the volume multiplier; 0 when the channel is off.
    pub volume: f32,
    /// The pan with the pan offset added, -1..1 and beyond (not clamped).
    pub pan: f32,
    /// Mod X and mod Y: the offsets of the level adjustments, plus how far automation has moved the filter's cutoff /
    /// resonance knob from its stored position.
    pub mod_x: f32,
    pub mod_y: f32,
}

impl Default for ChannelLevels {
    fn default() -> Self {
        ChannelLevels { volume: 1.0, pan: 0.0, mod_x: 0.0, mod_y: 0.0 }
    }
}

/// The song position and tempo a hosted plugin sees (tempo-synced delays, arpeggiators, LFOs). The engine gives it
/// before each `process` while it plays; it describes the call's first frame.
// spec: plugins/vst3-hosting.md §1 (transport)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportInfo {
    pub playing: bool,
    /// Beats (quarter notes) per minute.
    pub tempo: f64,
    /// Position in frames from the start of the song / pattern.
    pub frame: u64,
    /// The same position in quarter notes.
    pub beats: f64,
    /// Start of the bar the position is in, in quarter notes.
    pub bar_start: f64,
    /// Bar number of that bar, from 0.
    pub bar: i32,
    pub numerator: u16,
    pub denominator: u16,
    /// The loop being played (pattern playback), in quarter notes.
    pub looping: Option<(f64, f64)>,
}

impl TransportInfo {
    /// Frames per quarter note at `rate`.
    pub fn frames_per_beat(&self, rate: u32) -> f64 {
        f64::from(rate) * 60.0 / self.tempo.max(1.0)
    }

    /// The same transport `frames` frames later (a block split into chunks).
    pub fn advanced(&self, frames: u64, rate: u32) -> TransportInfo {
        if frames == 0 {
            return *self;
        }
        let beats = self.beats + frames as f64 / self.frames_per_beat(rate);
        let bar_len = f64::from(self.numerator.max(1)) * 4.0 / f64::from(self.denominator.max(1));
        let bar = (beats / bar_len).floor();
        TransportInfo { frame: self.frame + frames, beats, bar_start: bar * bar_len, bar: bar as i32, ..*self }
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
    /// [`Instrument::tick_params`] for effects.
    fn tick_params_fx(&self) -> bool {
        false
    }
    /// [`Instrument::transport`] for effects: the song position during the next `process_fx` call.
    fn transport_fx(&mut self, info: &TransportInfo) {
        let _ = info;
    }
}

pub type SharedEffect = Arc<Mutex<dyn HostedEffect>>;
