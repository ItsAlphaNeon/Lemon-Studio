//! Gain, pan and pitch laws.
//!
// spec: audio/gain-laws.md, audio/sampler-playback.md

use std::f64::consts::{FRAC_PI_4, SQRT_2};

/// The shared volume curve: `(11^x − 1) / 10`.
pub fn volume_curve(x: f64) -> f64 {
    (11f64.powf(x.max(0.0)) - 1.0) / 10.0
}

/// Channel volume `v` (0..=1) → amplitude; 1.0 = unity.
pub fn channel_gain(v: f64) -> f64 {
    volume_curve(v)
}

/// Note velocity (0..=128, 100 = unity) → amplitude.
pub fn velocity_gain(v: u8) -> f64 {
    volume_curve(f64::from(v) / 100.0)
}

/// Note pan byte (0..=128, 64 = center) → −1..=1, added to the channel pan.
pub fn note_pan(byte: u8) -> f64 {
    (f64::from(byte) - 64.0) / 64.0
}

/// Mixer fader `f` (0..=1) → amplitude; 0.8 = unity, 1.0 = +5.6 dB.
pub fn fader_gain(f: f64) -> f64 {
    volume_curve(f / 0.8)
}

/// Channel pan (−1..=1), constant power: center = 0.7071 per side.
pub fn channel_pan(p: f64) -> (f64, f64) {
    let a = (1.0 + p.clamp(-1.0, 1.0)) * FRAC_PI_4;
    (a.cos(), a.sin())
}

/// Mixer track pan (−1..=1) as a stereo matrix `[[L←L, L←R], [R←L, R←R]]`.
///
/// Mid follows constant power (unity at center); on the louder side the side signal fades with
/// `cos(qπ/2)`; the quieter side keeps only its own input scaled by `√2·cos(t)`.
pub fn mixer_pan(p: f64) -> [[f64; 2]; 2] {
    let p = p.clamp(-1.0, 1.0);
    let q = p.abs();
    let t = (1.0 + q) * FRAC_PI_4;
    let loud_mid = SQRT_2 * t.sin();
    let side = (q * std::f64::consts::FRAC_PI_2).cos();
    let own = (loud_mid + side) / 2.0;
    let cross = (loud_mid - side) / 2.0;
    let quiet = SQRT_2 * t.cos();
    if p <= 0.0 {
        [[own, cross], [0.0, quiet]]
    } else {
        [[quiet, 0.0], [cross, own]]
    }
}

/// Playback rate for a note key (60 = original speed) plus channel pitch in cents.
pub fn playback_rate(key: u16, cents: i32) -> f64 {
    2f64.powf((f64::from(key) - 60.0) / 12.0 + f64::from(cents) / 1200.0)
}

/// Output frame at which a tick position starts.
pub fn tick_to_frame(ticks: f64, ppq: u16, tempo: f64, rate: u32) -> u64 {
    (ticks / f64::from(ppq) * 60.0 / tempo * f64::from(rate)).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn measured_points() {
        // spec tables: channel volume and fader measurements
        assert!(close(channel_gain(1.0), 1.0, 1e-12));
        assert!(close(channel_gain(0.78125), 0.55101, 1e-4));
        assert!(close(channel_gain(0.5), 0.23166, 1e-4));
        assert!(close(channel_gain(0.02), 0.00491, 1e-4));
        assert!(close(fader_gain(0.8), 1.0, 1e-12));
        assert!(close(fader_gain(1.0), 1.90328, 1e-4));
        assert!(close(fader_gain(0.5), 0.34758, 1e-4));
        let (l, r) = channel_pan(0.5);
        assert!(close(l, 0.3827, 1e-4) && close(r, 0.9239, 1e-4));
        let m = mixer_pan(0.0);
        assert!(close(m[0][0], 1.0, 1e-12) && close(m[1][1], 1.0, 1e-12));
        assert!(close(m[0][1], 0.0, 1e-12) && close(m[1][0], 0.0, 1e-12));
        let m = mixer_pan(-0.5);
        assert!(close(m[0][0], 1.0068, 1e-4) && close(m[0][1], 0.2997, 1e-3) && close(m[1][1], 0.5412, 1e-4));
        let m = mixer_pan(0.25);
        assert!(close(m[1][1], 1.0499, 1e-4) && close(m[1][0], 0.1260, 1e-4) && close(m[0][0], 0.7857, 1e-4));
        assert!(close(playback_rate(72, 0), 2.0, 1e-12));
        assert!(close(playback_rate(60, 100), 1.0595, 1e-4));
        assert_eq!(tick_to_frame(96.0, 96, 130.0, 44100), 20354);
        assert!(close(velocity_gain(50), 0.23166, 1e-4));
        assert!(close(velocity_gain(128), 2.0527, 1e-3));
        assert!(close(note_pan(96), 0.5, 1e-12) && close(note_pan(0), -1.0, 1e-12));
        // Default template chain: channel default × center pan = 0.38961
        assert!(close(channel_gain(0.78125) * channel_pan(0.0).0, 0.38961, 1e-4));
    }
}
