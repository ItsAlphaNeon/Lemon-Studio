//! OpenStudio audio engine.
//!
//! - [`sample`]: decoding and resolving sample files.
//! - [`laws`]: gain, pan and pitch laws measured from FL Studio.
//! - [`engine`]: block-based playback of patterns and the playlist through the mixer.
//! - [`wav`]: writing renders to disk.

pub mod engine;
pub mod laws;
pub mod sample;
pub mod wav;

pub use engine::{Engine, EngineReport, PlayMode};
pub use sample::{Sample, SampleLibrary};
