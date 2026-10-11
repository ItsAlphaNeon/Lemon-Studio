//! OpenStudio audio engine.
//!
//! - [`sample`]: decoding and resolving sample files.
//! - [`laws`]: gain, pan and pitch laws measured from FL Studio.
//! - [`effects`]: OpenStudio's mixer-slot effects (EQ, limiter).
//! - [`engine`]: block-based playback of patterns and the playlist through the mixer.
//! - [`wav`]: writing renders to disk.

pub mod effects;
pub mod engine;
pub mod envelope;
pub mod instrument;
pub mod laws;
pub mod sample;
pub mod wav;

pub use engine::{Engine, EngineReport, PlayMode};
pub use instrument::{HostedEffect, Instrument, SharedEffect, SharedInstrument, TransportInfo};
pub use sample::{Sample, SampleLibrary};
