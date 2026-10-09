//! Mixer-slot effects: OpenStudio's own recreations, configured from a project's saved
//! plugin state.

mod biquad;
pub mod eq;
pub mod limiter;

pub use biquad::Biquad;

/// An insert effect processing interleaved stereo in place.
pub trait Effect: Send {
    fn process(&mut self, buf: &mut [f32]);
}

/// Creates an OpenStudio effect for a slot plugin, if one exists for that plugin.
/// The FLP plugin name only identifies the saved-state format.
pub fn from_slot(name: &str, state: &[u8], sample_rate: u32) -> Option<Box<dyn Effect>> {
    match name {
        "Fruity Parametric EQ 2" => eq::ParametricEq::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        "Fruity Limiter" => limiter::Limiter::from_state(state, sample_rate).map(|e| Box::new(e) as Box<dyn Effect>),
        _ => None,
    }
}

/// Little-endian int32 field `k` of a saved state.
pub(crate) fn field(state: &[u8], k: usize) -> Option<i32> {
    state.get(4 * k..4 * k + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}
