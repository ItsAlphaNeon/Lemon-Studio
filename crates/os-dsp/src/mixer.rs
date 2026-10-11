//! A mixer track's fader matrix and its three-band equaliser.
//!
// spec: audio/mixer.md ("Fader matrix", "Equaliser", "Sends")

use crate::level::{R12800, R6400, R65536, curve, knob, pan_split, unity_at_centre};

/// Stored defaults of the three equaliser bands: gain (0.01 dB), frequency and width positions (0..65536).
pub const EQ_DEFAULT: [[i32; 3]; 3] = [[0, 5777, 17500], [0, 33145, 17500], [0, 55825, 17500]];

/// The fader matrix of a track as four singles `[LL, RL, RR, LR]`: `left' = LL * left + RL * right`,
/// `right' = RR * right + LR * left`. `volume` 0..16000 (12800 = unity), `pan` -6400..6400, `separation` -64..64
/// (positive merges towards mono, negative widens). A track that is switched off, or at volume 0, has all four at 0.
/// Centred without separation `LL = RR = vol`; a panned track attenuates one side by the constant-power law and
/// feeds half of what that side lost across, so a hard-panned track still carries both inputs.
pub fn fader_matrix(volume: i32, pan: i32, separation: i32, on: bool) -> [f32; 4] {
    if !on || volume <= 0 {
        return [0.0; 4];
    }
    let vol = curve(knob(volume, R12800));
    let p = knob(pan, R6400);
    let (g0, g1) = pan_split(unity_at_centre(vol), p);
    let (mut xl, mut xr) = (0.0f64, 0.0f64);
    if p > 0.0 {
        xr = (1.0 - f64::from(g0) / f64::from(vol)) * 0.5;
    } else if p < 0.0 {
        xl = (1.0 - f64::from(g1) / f64::from(vol)) * 0.5;
    }
    if separation != 0 {
        let sep = f64::from(separation) / 64.0;
        let add = if separation > 0 { (5f64.powf(sep) - 1.0) * 0.25 } else { sep };
        xl = (2.0 * xl + add).min(1.0) * 0.5;
        xr = (2.0 * xr + add).min(1.0) * 0.5;
    }
    let (g0, g1) = (f64::from(g0), f64::from(g1));
    [(g0 * (1.0 - xl)) as f32, (g0 * xl) as f32, (g1 * (1.0 - xr)) as f32, (g1 * xr) as f32]
}

/// A send's gain from its stored level (0..16000, 12800 = unity): the level curve.
pub fn send_gain(level: i32) -> f32 {
    curve(knob(level, R12800))
}

/// One biquad band with its last two inputs and outputs per side kept as doubles.
#[derive(Debug, Clone, Copy)]
struct Band {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    /// Per side: x1, x2, y1, y2.
    state: [[f64; 4]; 2],
}

impl Band {
    #[inline]
    fn run(&mut self, side: usize, x: f32) -> f32 {
        let s = &mut self.state[side];
        let x = f64::from(x);
        let y = self.b0 * x + self.b1 * s[0] + self.b2 * s[1] - self.a1 * s[2] - self.a2 * s[3];
        *s = [x, s[0], y, s[2]];
        y as f32
    }
}

/// The track's equaliser: band 1 a low shelf, band 2 a peaking filter, band 3 a high shelf, in series. A band whose
/// gain is 0 is skipped. Filter states start at zero.
#[derive(Debug, Clone, Default)]
pub struct TrackEq {
    bands: [Option<Band>; 3],
}

impl TrackEq {
    /// The equaliser for stored band settings `[gain, frequency, width]` (gain -1800..1800 in 0.01 dB, frequency and
    /// width 0..65536) at `rate`.
    pub fn new(settings: [[i32; 3]; 3], rate: u32) -> Self {
        let mut bands = [None; 3];
        for (k, &[gain, freq, width]) in settings.iter().enumerate() {
            if gain != 0 {
                bands[k] = Some(design(k, gain, freq, width, rate));
            }
        }
        TrackEq { bands }
    }

    /// True when no band is active (the equaliser passes its input unchanged).
    pub fn is_flat(&self) -> bool {
        self.bands.iter().all(Option::is_none)
    }

    /// Processes interleaved stereo in place; each band hands the next its output rounded to single.
    pub fn process(&mut self, buf: &mut [f32]) {
        for band in self.bands.iter_mut().flatten() {
            for f in buf.chunks_exact_mut(2) {
                f[0] = band.run(0, f[0]);
                f[1] = band.run(1, f[1]);
            }
        }
    }
}

/// Coefficients of band `k` (0 = low shelf, 1 = peaking, 2 = high shelf).
fn design(k: usize, gain: i32, freq: i32, width: i32, rate: u32) -> Band {
    // Parameters scaled in single precision.
    let db = gain as f32 * 0.01f32;
    let x = knob(freq, R65536);
    let shaped = (((f64::from(x) * 101f64.ln()).exp() - 1.0) as f32 as f64 * 159.9 + 10.0) as f32;
    let f = shaped.min((f64::from(rate) * 0.4999) as f32);
    let q = knob(width, R65536);
    // Then in doubles.
    let w = f64::from(f) * 2.0 * std::f64::consts::PI / f64::from(rate);
    let (sn, cs) = (w.sin(), w.cos());
    let a = 10f64.powf(f64::from(db) / 40.0);
    let (b0, b1, b2, a1, a2) = if k == 1 {
        let qq = f64::from((((f64::from(q) * 1001f64.ln()).exp()) - 1.0) as f32) * 0.0999 + 0.1;
        let alpha = sn / (2.0 * qq);
        let n = 1.0 / (alpha / a + 1.0);
        (n * (1.0 + alpha * a), -2.0 * n * cs, n * (1.0 - alpha * a), -2.0 * n * cs, n * (1.0 - alpha / a))
    } else {
        let s = f64::from(q) * 1.9 + 0.1;
        let beta = ((a * a + 1.0) / s - (a - 1.0) * (a - 1.0)).sqrt() * sn;
        let d = (a - 1.0) * cs;
        if k == 0 {
            let n = 1.0 / (a + 1.0 + d + beta);
            (
                n * a * ((a + 1.0) - d + beta),
                2.0 * n * a * ((a - 1.0) - (a + 1.0) * cs),
                n * a * ((a + 1.0) - d - beta),
                -2.0 * n * ((a - 1.0) + (a + 1.0) * cs),
                n * ((a + 1.0 + d) - beta),
            )
        } else {
            let n = 1.0 / ((a + 1.0) - d + beta);
            (
                n * a * (a + 1.0 + d + beta),
                -2.0 * n * a * ((a - 1.0) + (a + 1.0) * cs),
                n * a * ((a + 1.0 + d) - beta),
                2.0 * n * ((a - 1.0) - (a + 1.0) * cs),
                n * (((a + 1.0) - d) - beta),
            )
        }
    };
    Band { b0, b1, b2, a1, a2, state: [[0.0; 4]; 2] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Centred at unity the matrix is the identity; the pan law matches the measured mixer pan (at -0.5:
    /// 1.0068 / 0.2997 / 0.5412, spec: audio/gain-laws.md).
    #[test]
    fn fader_matrix_points() {
        assert_eq!(fader_matrix(12800, 0, 0, true), [1.0, 0.0, 1.0, 0.0]);
        let [ll, rl, rr, lr] = fader_matrix(12800, -3200, 0, true);
        assert!((ll - 1.0068).abs() < 1e-4 && (rl - 0.2997).abs() < 1e-4 && (rr - 0.5412).abs() < 1e-4 && lr == 0.0);
        assert_eq!(fader_matrix(12800, 0, 0, false), [0.0; 4]);
        assert_eq!(fader_matrix(0, 0, 0, true), [0.0; 4]);
        // 16000 is curve(1.25).
        assert!((fader_matrix(16000, 0, 0, true)[0] - 1.90327).abs() < 1e-5);
    }

    /// Positive separation merges (mono at 64), negative widens (the side doubled at -64).
    #[test]
    fn separation_merges_and_widens() {
        let [ll, rl, rr, lr] = fader_matrix(12800, 0, 64, true);
        assert!((ll - 0.5).abs() < 1e-6 && (rl - 0.5).abs() < 1e-6 && (rr - 0.5).abs() < 1e-6 && (lr - 0.5).abs() < 1e-6);
        let [ll, rl, ..] = fader_matrix(12800, 0, -64, true);
        assert!((ll - 1.5).abs() < 1e-6 && (rl + 0.5).abs() < 1e-6);
        // Halfway merged follows 5^sep.
        let [_, rl, ..] = fader_matrix(12800, 0, 32, true);
        assert!((f64::from(rl) - (5f64.sqrt() - 1.0) * 0.25 * 0.5).abs() < 1e-6);
    }

    #[test]
    fn sends_use_the_level_curve() {
        assert_eq!(send_gain(12800), 1.0);
        assert_eq!(send_gain(0), 0.0);
    }

    /// A flat equaliser passes everything; a boosted peaking band at its centre frequency gains about its boost.
    #[test]
    fn equaliser_bands() {
        let mut flat = TrackEq::new(EQ_DEFAULT, 44100);
        assert!(flat.is_flat());
        let mut x = vec![0.25f32; 64];
        flat.process(&mut x);
        assert!(x.iter().all(|&v| v == 0.25));
        // +12 dB on band 2 at its default frequency: measure the gain on a sine there.
        let mut s = EQ_DEFAULT;
        s[1][0] = 1200;
        let mut eq = TrackEq::new(s, 44100);
        assert!(!eq.is_flat());
        let f = f64::from(((((33145.0f32 / 65536.0) as f64 * 101f64.ln()).exp() - 1.0) as f32 as f64 * 159.9 + 10.0) as f32);
        let n = 44100;
        let mut buf: Vec<f32> = (0..n).flat_map(|i| {
            let v = (2.0 * std::f64::consts::PI * f * i as f64 / 44100.0).sin() as f32 * 0.1;
            [v, v]
        }).collect();
        eq.process(&mut buf);
        let peak = buf[n..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((20.0 * (peak / 0.1).log10() - 12.0).abs() < 0.2, "{} dB at {f} Hz", 20.0 * (peak / 0.1).log10());
    }
}
