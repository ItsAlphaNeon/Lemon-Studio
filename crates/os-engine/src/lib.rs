//! Lemon Studio audio engine.
//!
//! - [`sample`]: decoding and resolving sample files.
//! - [`laws`]: gain, pan and pitch laws measured from FL Studio.
//! - [`effects`]: Lemon Studio's mixer-slot effects (EQ, limiter).
//! - [`clock`]: the sequencer's tick clock (ticks to samples).
//! - [`engine`]: tick-based playback of patterns and the playlist through the mixer.
//! - [`voices`]: a hosted instrument's voices as the host counts them.
//! - [`wav`]: writing renders to disk.

pub mod clock;
pub mod effects;
pub mod engine;
pub mod envelope;
pub mod instrument;
pub mod laws;
pub mod sample;
pub mod voices;
pub mod wav;

pub use clock::Clock;
pub use engine::{Engine, EngineReport, PlayMode};
pub use instrument::{HostedEffect, Instrument, SharedEffect, SharedInstrument, TransportInfo};
pub use sample::{Sample, SampleLibrary};
