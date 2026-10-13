//! Lemon Studio's DSP building blocks, shared by the engine and Lemon Studio's own plugins: the arithmetic the engine
//! pages specify, at the precision they give, so that a project plays the way it was made.
//!
//! - [`level`]: the level curve, knob positions as products with a reciprocal, the constant-power pan split.
//! - [`glide`]: the per-block gain glide.
//! - [`mixer`]: a mixer track's fader matrix and its three-band equaliser.
//! - [`resample`]: the sampler's windowed-sinc resampler (table or exact weights).
//! - [`stretch`]: time stretching and pitch shifting (Signalsmith Stretch).
//!
// spec: audio/conventions.md, audio/mixer.md, audio/sampler.md, audio/time-stretch.md

pub mod glide;
pub mod level;
pub mod mixer;
pub mod resample;
pub mod stretch;
