// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio audio engine.
//!
//! - [`sample`]: decoding and resolving sample files.
//! - [`laws`]: gain, pan and pitch laws measured from FL Studio.
//! - [`effects`]: Lemon Studio's mixer-slot effects (EQ, limiter, reverb, delay, soft clipper, fast distortion,
//!   balance, stereo enhancer, transient processor, chorus, stereo shaper, flanger, phaser, peak controller).
//! - [`formula`]: links' mapping formulas.
//! - [`clock`]: the sequencer's tick clock (ticks to samples).
//! - [`engine`]: tick-based playback of patterns and the playlist through the mixer.
//! - [`metronome`]: the click on every beat while playing live.
//! - [`parallel`]: rendering on several cores (a pool of worker threads and the jobs' graph).
//! - [`voices`]: a hosted instrument's voices as the host counts them.
//! - [`wav`]: writing renders to disk.

pub mod clock;
pub mod effects;
pub mod engine;
pub mod envelope;
pub mod formula;
pub mod instrument;
pub mod laws;
pub mod metronome;
pub mod parallel;
pub mod sample;
pub mod voices;
pub mod wav;

pub use clock::Clock;
pub use engine::{input_channels, DeviceInput, Engine, EngineReport, PlayMode, RecordPoint, RecordTap};
pub use instrument::{ChannelLevels, ChannelVoicing, HostedEffect, Instrument, NoteLevels, SharedEffect, SharedInstrument, TransportInfo, TuningBend};
pub use sample::{Sample, SampleLibrary};
