// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Lemon Studio's DSP building blocks, shared by the engine and Lemon Studio's own plugins: the arithmetic the engine
//! pages specify, at the precision they give, so that a project plays the way it was made.
//!
//! - [`level`]: the level curve, knob positions as products with a reciprocal, the constant-power pan split.
//! - [`glide`]: the per-block gain glide.
//! - [`mixer`]: a mixer track's fader matrix and its three-band equaliser.
//! - [`resample`]: the sampler's windowed-sinc resampler (table or exact weights).
//! - [`stretch`]: time stretching and pitch shifting (Signalsmith Stretch).
//! - [`articulator`]: a channel's envelopes and LFOs, stepped once per tick for every voice.
//! - [`channel_filter`]: the channel filter a voice runs through.
//!
// spec: audio/conventions.md, audio/mixer.md, audio/sampler.md, audio/time-stretch.md, audio/envelopes-and-lfos.md,
// audio/channel-filter.md

pub mod articulator;
pub mod channel_filter;
pub mod glide;
pub mod level;
pub mod mixer;
pub mod resample;
pub mod stretch;
