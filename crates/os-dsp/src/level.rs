//! Levels: the shared level curve, knob positions, the constant-power pan split.
//!
// spec: audio/conventions.md ("Numbers", "Level curve", "Knob positions are products, not quotients", "Pan split")

/// The single nearest `1 / 12800` (volumes, mix levels, sends).
pub const R12800: f32 = 1.0 / 12800.0;
/// The single nearest `1 / 6400` (pans).
pub const R6400: f32 = 1.0 / 6400.0;
/// The single nearest `1 / 65536` (frequency and width positions; exact).
pub const R65536: f32 = 1.0 / 65536.0;

/// A stored value as a knob position: the value as a single times the single nearest the reciprocal of its full
/// scale (`R12800`, `R6400`, ...), one rounding. For many values this is one unit in the last place away from the
/// quotient.
pub fn knob(value: i32, recip: f32) -> f32 {
    value as f32 * recip
}

/// The level curve `(11^x - 1) / 10` in the host's precision: `x * ln 11` rounded to single, the exponential and
/// the subtraction in doubles, times the single nearest 0.1, rounded once. `curve(1)` is exactly 1; values above 1
/// are allowed (`curve(1.25) = 1.9033...`).
pub fn curve(x: f32) -> f32 {
    if x == 1.0 {
        return 1.0;
    }
    let p = x * (11f64.ln() as f32);
    ((f64::from(p).exp() - 1.0) * f64::from(0.1f32)) as f32
}

/// The constant-power pan split of `level` at `pan` (-1 = left, 1 = right, clamped): the angle `(pan + 1) * pi / 4`
/// in doubles, each side `level * cos` / `level * sin` as a double product rounded to single. Centred, each side is
/// `level * 0.7071...`; a caller that wants unity at centre passes `level * sqrt(2)`.
pub fn pan_split(level: f32, pan: f32) -> (f32, f32) {
    let angle = (f64::from(pan.clamp(-1.0, 1.0)) + 1.0) * std::f64::consts::PI / 4.0;
    let l = f64::from(level);
    ((l * angle.cos()) as f32, (l * angle.sin()) as f32)
}

/// `level * sqrt(2)` as a single (one rounding): the level that comes out of [`pan_split`] as itself at centre.
pub fn unity_at_centre(level: f32) -> f32 {
    (f64::from(level) * std::f64::consts::SQRT_2) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curve_points() {
        assert_eq!(curve(0.0), 0.0);
        assert_eq!(curve(1.0), 1.0);
        assert!((curve(1.25) - 1.90327).abs() < 1e-5, "(11^1.25 - 1) / 10");
        assert!((curve(2.0) - 12.0).abs() < 1e-4);
        // The default channel volume 10000 / 12800.
        assert!((curve(knob(10000, R12800)) - 0.55101).abs() < 1e-5);
    }

    /// Knob positions are products: for 30, 60, 73, 79 and 80 percent the product with the single 0.01 is not the
    /// single quotient.
    #[test]
    fn knob_positions_are_products() {
        let r100 = 0.01f32;
        let differs = [30, 60, 73, 79, 80].iter().filter(|&&v| knob(v, r100) != v as f32 / 100.0).count();
        assert!(differs > 0);
        assert_eq!(knob(6400, R6400), 1.0);
    }

    #[test]
    fn pan_split_is_constant_power() {
        let (l, r) = pan_split(1.0, 0.0);
        assert!((l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-7 && l == r);
        // Unity at centre: exactly 1.
        assert_eq!(pan_split(unity_at_centre(1.0), 0.0), (1.0, 1.0));
        // Hard right leaves the cosine's residue on the left.
        let (l, r) = pan_split(1.0, 1.0);
        assert!(l.abs() < 1e-15 && r == 1.0);
        assert_eq!(pan_split(1.0, 3.0), pan_split(1.0, 1.0), "clamped");
    }
}
