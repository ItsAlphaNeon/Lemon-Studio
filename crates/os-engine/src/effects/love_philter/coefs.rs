// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A filter's stage coefficients from its cutoff and resonance, for the twelve types and six orders.
//!
// spec: plugins/fruity-love-philter.md ("The cascade", "Coefficients")

use std::f64::consts::{LN_2, PI, TAU};

use os_dsp::num::s;

use super::state::*;

/// One stage's coefficients.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Coef {
    /// Chocolate and Lime: the frequency and damping / feedback (the glides' targets).
    pub f: f32,
    pub q: f32,
    /// The biquads: a1, a2, b0, b1, b2.
    pub b: [f64; 5],
    /// Mango: the coefficient `G'`, the resonance `q`, the output gain `K` and the drive factor `2 D` (0: no drive).
    pub g: f64,
    pub mq: f32,
    pub k: f32,
    pub drive: f64,
}

/// The number of stages an order runs.
pub fn stages(order: i32) -> usize {
    match order {
        1..=3 => order as usize,
        4 => 2,
        5 => 3,
        _ => 0,
    }
}

/// The shared curve `double(single(exp(double(single(v)) * ln n) - 1)) * k`.
pub fn curve(v: f64, n: f64, k: f64) -> f64 {
    f64::from(s((f64::from(s(v)) * n.ln()).exp() - 1.0)) * k
}

/// The plugin's single power `single(exp(double(e) * ln double(single(x))))`, 0 for `x = 0`.
pub fn pow_s(x: f64, e: f64) -> f32 {
    let x = s(x);
    if x <= 0.0 { 0.0 } else { s((e * f64::from(x).ln()).exp()) }
}

fn sinh(z: f64) -> f64 {
    if z.abs() <= 1e-12 { 0.0 } else { (z.exp() - (-z).exp()) / 2.0 }
}

/// What a filter's coefficients are worked out from.
#[derive(Debug, Clone, Copy)]
pub struct Input {
    pub kind: i32,
    pub order: i32,
    pub alt: bool,
    /// The cutoff and resonance with their targets added.
    pub cutoff: f32,
    pub res: f32,
    /// The shelves' and peaking type's gain `A` (control 3, not modulated).
    pub gain: f32,
    /// The Mango type's drive input (control 3 / 128, not modulated).
    pub drive: f32,
    /// The filters' rate factor.
    pub rf: f32,
}

/// The cutoff value `c'` of each stage: the type's for the last, and for the alternative orders each earlier stage
/// the next one's raised to 0.5 (order 4) or 0.75 (order 5).
fn per_stage(cp: f64, order: i32, n: usize) -> [f64; 3] {
    let mut v = [cp; 3];
    let e = match order {
        4 => 0.5,
        5 => 0.75,
        _ => return v,
    };
    for k in (0..n.saturating_sub(1)).rev() {
        v[k] = f64::from(pow_s(v[k + 1], e));
    }
    v
}

/// The coefficients of a filter's stages (`stages(order)` of them are used).
pub fn work_out(p: &Input) -> [Coef; 3] {
    let n = stages(p.order);
    let mut out = [Coef::default(); 3];
    if n == 0 {
        return out;
    }
    let s_inv = f64::from(s(1.0 / n as f64));
    let rf = f64::from(p.rf);
    let c = f64::from(p.cutoff);
    let mut r = f64::from(p.res);
    if p.alt && (1..=10).contains(&p.kind) {
        r = (r * (c + 1.0)) * 0.5;
    }
    let cut = |f: &dyn Fn() -> f64| if c <= 0.0 { 0.0 } else { f() };
    match p.kind {
        CHOCOLATE => {
            let cp = per_stage(cut(&|| curve(c, 51.0, 0.01590000092983246)), p.order, n);
            let mut q = if r <= 0.0 {
                1.0
            } else if r >= 1.0 {
                0.2
            } else {
                1.0 - r * 0.8999999761581421
            };
            if p.alt {
                q *= q;
            }
            for k in 0..n {
                let w = (cp[k] + 0.005) * rf;
                out[k].f = s(w.min(0.625).sin() * 2.0);
                out[k].q = if k == 0 { s(q) } else { 1.0 };
            }
        }
        LIME_LP | LIME_HP => {
            let cp = per_stage(cut(&|| curve(c, 76.0, 0.01324).min(1.6)), p.order, n);
            let mut q = if r <= 0.0 {
                1.0
            } else if r >= 1.0 {
                50.0
            } else {
                curve(r, 1001.0, 0.049) + 1.0
            };
            if n > 1 {
                q = f64::from(pow_s(q, s_inv));
            }
            for k in 0..n {
                let w = (cp[k] + 0.007) * (rf * 1.5707963267948966);
                let kk = w.min(1.2566370614359172).cos();
                out[k].f = s(2.0 - kk * 2.0);
                let m = kk - 1.0;
                let v = s((((-m * m) * m).sqrt() * 1.4142135381698608) / (q * m) + 1.0);
                out[k].q = if v < 0.1f32 { 0.1 } else { v };
            }
        }
        VANILLA_LP | VANILLA_HP | ALL_PASS => {
            let cp = cut(&|| match p.kind {
                VANILLA_LP => curve(c, 1501.0, 0.0006646666442975402),
                VANILLA_HP => curve(c, 1501.0, 0.00039993334212340415).min(0.6001),
                _ => curve(c, 1501.0, 0.00039993334212340415).min(1.0001),
            });
            let cp = per_stage(cp, p.order, n);
            let mut q = if r <= 0.0 {
                0.1
            } else if r >= 1.0 {
                50.0
            } else {
                curve(r, 2001.0, 0.024950001388788223) + 0.1
            };
            if n > 1 {
                q = f64::from(pow_s(q, s_inv));
            }
            let h = 0.5 / q;
            let o = if p.kind == VANILLA_LP { 0.003 } else { 0.0001 };
            for k in 0..n {
                let (sn, cs) = angle(cp[k], o, rf);
                out[k].b = biquad(p.kind, sn * h, cs, 1.0, 0.0);
            }
        }
        BAND_PASS | BAND_STOP => {
            let cp = per_stage(cut(&|| curve(c, 1501.0, 0.00039993334212340415).min(1.0001)), p.order, n);
            let mut b = if r <= 0.0 {
                0.01
            } else if r >= 1.0 {
                12.0
            } else {
                curve(r, 101.0, 0.11989999562501907) + 0.01
            };
            let order = p.order as f32;
            let width = if p.kind == BAND_PASS { order } else { 1.0 / order };
            b *= f64::from(s(f64::from(width) * (LN_2 / 2.0)));
            for k in 0..n {
                let w = omega(cp[k], 0.0001, rf);
                let sn = w.sin();
                let alpha = sn * sinh((b * w) / sn);
                out[k].b = biquad(p.kind, alpha, w.cos(), 1.0, 0.0);
            }
        }
        LOW_SHELF | PEAKING | HIGH_SHELF => {
            let cp = per_stage(cut(&|| curve(c, 1501.0, 0.00039993334212340415).min(1.0001)), p.order, n);
            let u = 1.0 - r;
            let mut a = f64::from(p.gain);
            if n > 1 {
                a = (s_inv * a.ln()).exp();
            }
            if p.kind == PEAKING {
                let q = if u <= 0.0 {
                    0.1
                } else if u >= 1.0 {
                    100.0
                } else {
                    curve(u, 2001.0, 0.049949999898672104) + 0.1
                };
                for k in 0..n {
                    let (sn, cs) = angle(cp[k], 0.0001, rf);
                    out[k].b = biquad(PEAKING, sn / (q * 2.0), cs, a, 0.0);
                }
            } else {
                let sl = if u <= 0.0 {
                    0.1
                } else if u >= 1.0 {
                    1.0
                } else {
                    curve(u, 3.0, 0.44999998807907104) + 0.1
                };
                let beta = a.sqrt() * ((a + 1.0 / a) * (1.0 / sl - 1.0) + 2.0).sqrt();
                for k in 0..n {
                    let (sn, cs) = angle(cp[k], 0.0001, rf);
                    out[k].b = biquad(p.kind, 0.0, cs, a, beta * sn);
                }
            }
        }
        _ => {
            // Mango, without the "r follows" rule.
            let r = f64::from(p.res);
            let cp = per_stage(cut(&|| curve(c, 201.0, 0.014949999749660492).min(3.0)), p.order, n);
            let d = f64::from(s((f64::from(p.drive) * 251f64.ln()).exp() - 1.0)) * 0.12;
            let drive = d >= 0.001;
            let s_inv = f64::from(s(1.0 / ((n as f64 - 1.0) / 4.0 + 1.0)));
            let mut rho = if r <= 0.0 {
                0.0
            } else {
                let m = f64::from(s(d).min(1.0)) * 0.6 + 1.9;
                if r >= 1.0 { m } else { r * m }
            };
            if n > 1 {
                rho = (s_inv * (rho + 1.0).ln()).exp() - 1.0;
            }
            for k in 0..n {
                let w = ((cp[k] + 0.01) * (rf * 0.5)).min(0.65);
                let w2 = w * w;
                let pp = ((((w * 1.873) * w2) + w2 * 0.4955) - w * 0.649) + 0.9988;
                let g = 1.0 - (((-PI) * pp) * w).exp();
                let m = ((w2 * -3.9364) + w * 1.8409) + 0.9968;
                let mut q = s(-rho * m);
                let st = &mut out[k];
                if drive {
                    st.g = g * (1.0 / d);
                    st.drive = 2.0 * d;
                    let tau = s(((1.0 / d) * 4.0).tanh());
                    if p.alt {
                        q = q * tau;
                    }
                    st.k = s((f64::from(1.0 / tau) * 0.5) * f64::from(1.0 - q));
                } else {
                    st.g = g;
                    st.drive = 0.0;
                    st.k = s(f64::from(1.0 - q) * 0.5);
                }
                st.mq = q;
            }
        }
    }
    out
}

fn omega(cp: f64, o: f64, rf: f64) -> f64 {
    ((cp + o) * (rf * TAU)).clamp(0.0, 2.849517146113191)
}

fn angle(cp: f64, o: f64, rf: f64) -> (f64, f64) {
    let w = omega(cp, o, rf);
    (w.sin(), w.cos())
}

/// The biquads' coefficients a1, a2, b0, b1, b2 (spec "The biquads' coefficients"); `gamma` for the shelves.
fn biquad(kind: i32, alpha: f64, k: f64, a: f64, gamma: f64) -> [f64; 5] {
    match kind {
        PEAKING => {
            let d = 1.0 / (alpha / a + 1.0);
            let a1 = (d * k) * -2.0;
            [a1, d * (1.0 - alpha / a), d * (alpha * a + 1.0), a1, d * (1.0 - alpha * a)]
        }
        LOW_SHELF => {
            let t = (a - 1.0) * k;
            let d = 1.0 / (((a + 1.0) + t) + gamma);
            [
                (d * ((a - 1.0) + (a + 1.0) * k)) * -2.0,
                d * (((a + 1.0) + t) - gamma),
                d * (a * (((a + 1.0) - t) + gamma)),
                d * ((a * 2.0) * ((a - 1.0) - (a + 1.0) * k)),
                d * (a * (((a + 1.0) - t) - gamma)),
            ]
        }
        HIGH_SHELF => {
            let t = (a - 1.0) * k;
            let d = 1.0 / (((a + 1.0) - t) + gamma);
            [
                (d * ((a - 1.0) - (a + 1.0) * k)) * 2.0,
                d * (((a + 1.0) - t) - gamma),
                d * (a * (((a + 1.0) + t) + gamma)),
                d * ((a * -2.0) * ((a - 1.0) + (a + 1.0) * k)),
                d * (a * (((a + 1.0) + t) - gamma)),
            ]
        }
        _ => {
            let d = 1.0 / (alpha + 1.0);
            let a1 = (d * k) * -2.0;
            let a2 = d * (1.0 - alpha);
            match kind {
                VANILLA_LP => {
                    let b1 = d * (1.0 - k);
                    let b0 = b1 * 0.5;
                    [a1, a2, b0, b1, b0]
                }
                VANILLA_HP => {
                    let b1 = d * -(k + 1.0);
                    let b0 = -b1 * 0.5;
                    [a1, a2, b0, b1, b0]
                }
                ALL_PASS => [a1, a2, a2, a1, 1.0],
                BAND_PASS => [a1, a2, d * alpha, 0.0, d * -alpha],
                _ => [a1, a2, d, a1, d],
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: i32, order: i32) -> Input {
        Input { kind, order, alt: false, cutoff: 0.5, res: 0.5, gain: 1.0, drive: 0.0, rf: 0.5 }
    }

    /// The gain of a biquad at angular frequency `w`.
    fn response(b: &[f64; 5], w: f64) -> f64 {
        let [a1, a2, b0, b1, b2] = *b;
        let z1 = (w.cos(), -w.sin());
        let z2 = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = (b0 + b1 * z1.0 + b2 * z2.0, b1 * z1.1 + b2 * z2.1);
        let den = (1.0 + a1 * z1.0 + a2 * z2.0, a1 * z1.1 + a2 * z2.1);
        (num.0.hypot(num.1)) / (den.0.hypot(den.1))
    }

    #[test]
    fn biquads_have_their_shapes() {
        let lp = work_out(&input(VANILLA_LP, 1))[0].b;
        assert!((response(&lp, 1e-4) - 1.0).abs() < 1e-6);
        assert!(response(&lp, 3.0) < 0.01);
        let hp = work_out(&input(VANILLA_HP, 1))[0].b;
        assert!(response(&hp, 1e-4) < 1e-3);
        assert!((response(&hp, 3.1) - 1.0).abs() < 0.01);
        let ap = work_out(&input(ALL_PASS, 1))[0].b;
        for w in [0.01, 0.5, 2.0] {
            assert!((response(&ap, w) - 1.0).abs() < 1e-9);
        }
        let mut pk = input(PEAKING, 1);
        pk.gain = 8.0;
        let pk = work_out(&pk)[0].b;
        assert!(response(&pk, 1e-4) < 1.01);
    }

    #[test]
    fn stages_and_orders() {
        assert_eq!((0..6).map(stages).collect::<Vec<_>>(), [0, 1, 2, 3, 2, 3]);
        let c = work_out(&input(CHOCOLATE, 5));
        assert!(c[0].f > c[1].f && c[1].f > c[2].f, "earlier stages take higher cutoffs: {c:?}");
        assert_eq!(c[1].q, 1.0);
        let m = work_out(&Input { drive: 0.5, ..input(MANGO, 1) })[0];
        assert!(m.drive > 0.0 && m.mq < 0.0 && m.k > 0.0);
        assert_eq!(pow_s(0.0, 0.5), 0.0);
        assert_eq!(pow_s(4.0, 0.5), 2.0);
    }
}
