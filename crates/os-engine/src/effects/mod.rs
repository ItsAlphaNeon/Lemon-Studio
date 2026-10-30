// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Mixer-slot effects: Lemon Studio's own recreations, configured from a project's saved
//! plugin state.

pub mod balance;
pub mod chorus;
pub mod delay;
pub mod dynamics;
pub mod eq;
pub mod fast_dist;
pub mod filter;
pub mod flanger;
pub mod limiter;
pub mod love_philter;
pub mod maximus;
pub mod peak_ctrl;
pub mod phaser;
pub mod reverb;
pub mod soft_clipper;
pub mod stereo_enhancer;
pub mod stereo_shaper;
pub mod transient_processor;

/// An insert effect processing interleaved stereo in place.
pub trait Effect: Send {
    fn process(&mut self, buf: &mut [f32]);
    /// Empties delay lines and filter states (a slot switched on during the song, spec: audio/mixer.md "Changes while
    /// the song plays").
    fn reset(&mut self) {}
    /// The song's tempo in BPM (effects with tempo-based times).
    fn set_tempo(&mut self, _bpm: f64) {}
    /// The delay the effect puts its output out late by, in samples (a look-ahead; spec: audio/delay-compensation.md).
    /// The engine reads it after every `process` and delays the other paths to match. Default: none.
    fn latency(&self) -> usize {
        0
    }
    /// An extra output (Fruity Stereo Shaper's difference output): the number of the track's output it goes to,
    /// counted from 1, and what the block processed last put out on it, interleaved like the block (spec:
    /// audio/sidechain.md "Extra output").
    fn extra(&self) -> Option<(usize, &[f32])> {
        None
    }
    /// A sidechain input (Fruity Limiter's): the number of the track's input it listens to, counted from 1, 0 = none
    /// (spec: audio/sidechain.md "Sidechain input").
    fn key_input(&self) -> usize {
        0
    }
    /// Processes one block with its sidechain input `key`, interleaved like the block. Called instead of `process`
    /// when `key_input` names an input and the track has one.
    fn process_keyed(&mut self, buf: &mut [f32], _key: &[f32]) {
        self.process(buf);
    }
    /// A controller's output `k` (0..1) as the block processed last left it, for the links that read it (Fruity Peak
    /// Controller's three; spec: plugins/lemon-peak-controller.md "Links"). `None`: no such output.
    fn control(&self, _k: usize) -> Option<f32> {
        None
    }
}

/// Creates a Lemon Studio effect for a slot plugin, if one exists for that plugin.
/// The FLP plugin name only identifies the saved-state format.
pub fn from_slot(name: &str, state: &[u8], sample_rate: u32) -> Option<Box<dyn Effect>> {
    match name {
        "Fruity Parametric EQ 2" => eq::ParametricEq::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Limiter" => limiter::Limiter::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Reeverb 2" => reverb::Reverb::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Delay 3" => delay::Delay::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Soft Clipper" => soft_clipper::SoftClipper::from_state(state).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Fast Dist" => fast_dist::FastDist::from_state(state).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Balance" => balance::Balance::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Stereo Enhancer" => stereo_enhancer::StereoEnhancer::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Chorus" => chorus::Chorus::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Flanger" => flanger::Flanger::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Phaser" => phaser::Phaser::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Stereo Shaper" => stereo_shaper::StereoShaper::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Love Philter" => love_philter::LovePhilter::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Maximus" => maximus::Maximus::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Transient Processor" => {
            transient_processor::TransientProcessor::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>)
        }
        "Fruity Filter" => filter::Filter::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Peak Controller" => peak_ctrl::PeakController::from_state(state).map(|e| Box::new(e) as Box<dyn Effect>),
        _ => None,
    }
}

/// Little-endian int32 field `k` of a saved state.
pub(crate) fn field(state: &[u8], k: usize) -> Option<i32> {
    state.get(4 * k..4 * k + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}

/// `N` little-endian int32 fields of a saved state from field `first` on; `None` when the state is shorter.
pub(crate) fn ints<const N: usize>(state: &[u8], first: usize) -> Option<[i32; N]> {
    let mut v = [0; N];
    for (k, x) in v.iter_mut().enumerate() {
        *x = field(state, first + k)?;
    }
    Some(v)
}

/// Values as a saved state of little-endian int32 fields.
pub(crate) fn state_of(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

