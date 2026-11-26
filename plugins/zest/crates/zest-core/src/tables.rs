// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The oscillator tables of `pages/tables.md`, generated once and shared by all instances. Nothing here is stored as data.
//!
//! The correction tables are kept per phase, so an insertion reads two contiguous columns instead of 255 scattered cache
//! lines: `col[j][k]` is the page's `S[128 k + j]`, for j = 0 .. 129.

use std::sync::OnceLock;

pub const TAPS: usize = 255;
pub const WAVE_LEN: usize = 2048;
pub const WAVE_LEVELS: usize = 10;
pub const BL_LEN: usize = 2052;
pub const BL_LEVELS: usize = 30;

/// A step or ramp correction table, indexed by phase then tap.
pub struct Blep {
    pub col: Box<[[f32; TAPS]; 130]>,
}

impl Blep {
    fn from_flat(t: &[f32]) -> Blep {
        let mut col: Box<[[f32; TAPS]; 130]> = vec![[0.0f32; TAPS]; 130].into_boxed_slice().try_into().unwrap();
        for j in 0..130 {
            for k in 0..TAPS {
                col[j][k] = t[128 * k + j];
            }
        }
        Blep { col }
    }

    /// The page's `S[n]`, for checks.
    pub fn at(&self, n: usize) -> f32 {
        let (k, j) = (n / 128, n % 128);
        if k >= TAPS {
            // n = 32640, 32641
            self.col[128 + j][TAPS - 1]
        } else {
            self.col[j][k]
        }
    }
}

/// The wavetables (9 shapes x 10 levels x 2048). Level l starts at offset l * 2048.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    Triangle = 0,
    SoftTriangle = 1,
    SawTriangle = 2,
    SoftClipTriangle = 3,
    Square = 4,
    Pulse2 = 5,
    Pulse4 = 6,
    Pulse8 = 7,
    PairedPulse = 8,
}

pub struct Tables {
    pub step1: Blep,
    pub step2: Blep,
    pub step4: Blep,
    pub ramp1: Blep,
    pub ramp4: Blep,
    /// 9 shapes, each 10 x 2048 values.
    pub waves: Vec<Vec<f32>>,
    /// Digital oscillator: 30 x 2052 values.
    pub bl_saw: Vec<f32>,
    pub bl_tri: Vec<f32>,
}

impl Tables {
    pub fn get() -> &'static Tables {
        static T: OnceLock<Tables> = OnceLock::new();
        T.get_or_init(Tables::build)
    }

    #[inline]
    pub fn wave(&self, w: Wave) -> &[f32] {
        &self.waves[w as usize]
    }

    fn build() -> Tables {
        let fit = BlepFit::parse();
        let (s1, p1) = step_flat(fit.row("blep_step"), -1e-07);
        let (s2, _) = step_flat(fit.row("blep2_step"), 2e-07);
        let (s4, p4) = step_flat(fit.row("blep4_step"), 1e-07);
        let r1 = ramp_flat(fit.row("blep_ramp"), &p1, 0.0);
        let r4 = ramp_flat(fit.row("blep4_ramp"), &p4, -3e-07);
        let waves = (0..9).map(build_wave).collect();
        Tables {
            step1: Blep::from_flat(&s1),
            step2: Blep::from_flat(&s2),
            step4: Blep::from_flat(&s4),
            ramp1: Blep::from_flat(&r1),
            ramp4: Blep::from_flat(&r4),
            waves,
            bl_saw: build_bl(true),
            bl_tri: build_bl(false),
        }
    }
}

const POINTS: usize = 16256;

/// The rows of `data/blep-fit.tsv`: table name, c, then the correction's coefficients.
struct BlepFit(Vec<(String, Vec<f64>)>);

impl BlepFit {
    fn parse() -> BlepFit {
        let text = include_str!("../data/blep-fit.tsv");
        BlepFit(
            text.lines()
                .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
                .map(|l| {
                    let mut w = l.split('\t');
                    let name = w.next().unwrap_or("").to_string();
                    (name, w.map(|x| x.trim().parse().unwrap_or(0.0)).collect())
                })
                .collect(),
        )
    }

    fn row(&self, name: &str) -> &[f64] {
        &self.0.iter().find(|r| r.0 == name).unwrap_or_else(|| panic!("blep-fit row {name}")).1
    }
}

/// v = f32(round(x * 1e7) / 1e7), as Diva's tables hold their values.
#[inline]
fn seven_decimals(x: f64) -> f32 {
    ((x * 1e7).round() / 1e7) as f32
}

/// The sum over k of T_k(u) * (a[k] + b[k]...): the Chebyshev polynomials at u.
#[inline]
fn chebyshev(u: f64, t: &mut [f64; 32]) {
    t[0] = 1.0;
    t[1] = u;
    for k in 1..31 {
        t[k + 1] = 2.0 * u * t[k] - t[k - 1];
    }
}

/// A step table, section 3.2, and its unrounded p(m) for the ramp of the same variant.
fn step_flat(row: &[f64], tail: f32) -> (Vec<f32>, Vec<f64>) {
    use std::f64::consts::PI;
    let c = row[0];
    let (cc, cd, ce) = (&row[1..33], &row[33..65], &row[65..97]);
    let h = |t: f64| -> f64 {
        let x = PI * t / 128.0;
        let w = 0.42 + 0.5 * x.cos() + 0.08 * (2.0 * x).cos();
        2.0 * c * sinc(c * t) * w
    };
    let mut p = vec![0.0f64; POINTS];
    let mut s = vec![0.0f32; 32642];
    let mut sum = 0.0f64;
    let mut t = [0.0f64; 32];
    for m in 0..POINTS {
        for i in (8 * m).saturating_sub(if m == 0 { 0 } else { 4 })..8 * m + 4 {
            sum += h((i as f64 + 0.5) / 1024.0);
        }
        let tm = (m as f64 + 0.5) / 128.0;
        chebyshev(2.0 * tm / 128.0 - 1.0, &mut t);
        let (sn, cs) = (PI * c * tm).sin_cos();
        let mut corr = 0.0;
        for k in 0..32 {
            corr += t[k] * (cc[k] * cs + cd[k] * sn + ce[k]);
        }
        let pm = sum / 1024.0 - 1.0 + corr;
        p[m] = pm;
        let v = seven_decimals(pm);
        s[16383 - m] = -v;
        s[16384 + m] = v;
    }
    s[32640] = tail;
    s[32641] = tail;
    (s, p)
}

/// A ramp table, section 4, from the same variant's p(m).
fn ramp_flat(row: &[f64], p: &[f64], tail: f32) -> Vec<f32> {
    use std::f64::consts::PI;
    let c = row[0];
    let (cc, cd, ce, cf) = (&row[1..33], &row[33..65], &row[65..97], &row[97..129]);
    let mut r = vec![0.0f32; 32642];
    let mut t = [0.0f64; 32];
    for m in 0..POINTS {
        let tm = (m as f64 + 0.5) / 128.0;
        chebyshev(2.0 * tm / 128.0 - 1.0, &mut t);
        let (sn, cs) = (PI * c * tm).sin_cos();
        let mut v = 0.0;
        for k in 0..32 {
            v += t[k] * (cc[k] * cs + cd[k] * sn + ce[k] + cf[k] * p[m]);
        }
        let v = seven_decimals(v);
        r[16383 - m] = v;
        r[16384 + m] = v;
    }
    r[32640] = tail;
    r[32641] = tail;
    r
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let p = std::f64::consts::PI * x;
        p.sin() / p
    }
}

/// In-place radix-2 FFT (`inverse` without the 1/n scale).
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        for k in 0..len / 2 {
            let w = sign * 2.0 * std::f64::consts::PI * k as f64 / len as f64;
            let (wr, wi) = (w.cos(), w.sin());
            let mut i = 0;
            while i < n {
                let (a, b) = (i + k, i + k + len / 2);
                let (xr, xi) = (re[b] * wr - im[b] * wi, re[b] * wi + im[b] * wr);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
                i += len;
            }
        }
        len <<= 1;
    }
}

/// The base shape cut to its first `h` harmonics: a brick wall on the DFT, bin h stays, bin 0 stays.
fn band_limit(spec_re: &[f64], spec_im: &[f64], h: usize) -> Vec<f64> {
    let n = spec_re.len();
    let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
    for b in 0..n {
        let bin = b.min(n - b);
        if bin <= h {
            re[b] = spec_re[b];
            im[b] = spec_im[b];
        }
    }
    fft(&mut re, &mut im, true);
    re.iter().map(|x| x / n as f64).collect()
}

fn tri(x: usize) -> f64 {
    if x < 1024 {
        -1.0 + x as f64 / 512.0
    } else {
        3.0 - x as f64 / 512.0
    }
}

fn base_shape(shape: usize, x: usize) -> f64 {
    let in_range = |list: &[(usize, usize)]| list.iter().any(|&(a, b)| a <= x && x < b);
    match shape {
        0 => tri(x),
        1 => {
            let t = tri(x);
            let k = 181.0 / 256.0;
            if t > k {
                t - (t - k) * (t - k) / (2.0 * (1.0 - k))
            } else {
                t
            }
        }
        2 => {
            if x < 1024 {
                -1.0 + x as f64 / 512.0
            } else {
                0.8 - 1.8 * (x as f64 - 1024.0) / 1024.0
            }
        }
        3 => (0.8 * tri(x)).tanh() / 0.8f64.tanh(),
        4 => (x >= 1024) as u32 as f64,
        5 => in_range(&[(512, 1024), (1536, 2048)]) as u32 as f64,
        6 => (0..4).any(|i| {
            let a = i * 512;
            a + 256 <= x && x < a + 512
        }) as u32 as f64,
        7 => (0..8).any(|i| {
            let a = i * 256;
            a + 128 <= x && x < a + 256
        }) as u32 as f64,
        _ => in_range(&[(640, 768), (896, 1024), (1664, 1792), (1920, 2048)]) as u32 as f64,
    }
}

fn build_wave(shape: usize) -> Vec<f32> {
    let mut re: Vec<f64> = (0..WAVE_LEN).map(|x| base_shape(shape, x)).collect();
    let mut im = vec![0.0; WAVE_LEN];
    fft(&mut re, &mut im, false);
    let mut out = Vec::with_capacity(WAVE_LEN * WAVE_LEVELS);
    for l in 0..WAVE_LEVELS {
        let h = 1024 >> l;
        out.extend(band_limit(&re, &im, h).iter().map(|x| *x as f32));
    }
    out
}

const BL_HARMONICS: [usize; 30] =
    [1, 1, 1, 2, 2, 3, 4, 5, 6, 8, 10, 13, 17, 22, 28, 36, 45, 58, 73, 93, 119, 151, 192, 244, 309, 393, 499, 634, 806, 1023];

fn build_bl(saw: bool) -> Vec<f32> {
    let base: Vec<f64> = (0..WAVE_LEN).map(|x| if saw { -1.0 + x as f64 / 1024.0 } else { tri(x) }).collect();
    let mean: f64 = base.iter().sum::<f64>() / WAVE_LEN as f64;
    let mut re = base.clone();
    let mut im = vec![0.0; WAVE_LEN];
    fft(&mut re, &mut im, false);
    let mut out = Vec::with_capacity(BL_LEN * BL_LEVELS);
    for &h in BL_HARMONICS.iter() {
        let lim = band_limit(&re, &im, h);
        let lvl: Vec<f32> = lim.iter().map(|x| (x - mean * 0.5) as f32).collect();
        out.extend_from_slice(&lvl);
        out.extend_from_slice(&lvl[..4]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f64, e: f64) -> bool {
        (a as f64 - b).abs() < e
    }

    #[test]
    fn page_check_values() {
        let t = Tables::get();
        let check: [(usize, [f64; 5]); 5] = [
            (129, [0.0000001, -0.0000002, -0.0000001, 0.0, -0.0000003]),
            (16384, [-0.9929686, -0.9964842, -0.9982421, 0.1300494, 0.5270553]),
            (16512, [0.1690655, -0.1916181, -0.5605388, -0.0265163, 0.0707069]),
            (17000, [-0.0256840, -0.0837127, 0.1713908, -0.0074803, -0.0230247]),
            (30000, [-0.0000406, -0.0001079, -0.0002297, 0.0000089, 0.0000441]),
        ];
        for (n, v) in check {
            for (tab, x) in [&t.step1, &t.step2, &t.step4, &t.ramp1, &t.ramp4].iter().zip(v) {
                assert!(close(tab.at(n), x, 1.5e-7), "n {n}: {} against {x}", tab.at(n));
            }
        }
        assert_eq!(t.step1.at(32641), -1e-07);
        assert_eq!(t.ramp4.at(32640), -3e-07);
    }

    #[test]
    fn generation_time() {
        let t0 = std::time::Instant::now();
        let _ = Tables::build();
        let all = t0.elapsed().as_secs_f64() * 1e3;
        let t0 = std::time::Instant::now();
        let fit = BlepFit::parse();
        let (_, p1) = step_flat(fit.row("blep_step"), 0.0);
        let _ = step_flat(fit.row("blep2_step"), 0.0);
        let (_, p4) = step_flat(fit.row("blep4_step"), 0.0);
        let _ = ramp_flat(fit.row("blep_ramp"), &p1, 0.0);
        let _ = ramp_flat(fit.row("blep4_ramp"), &p4, 0.0);
        eprintln!("all tables {all:.1} ms, of which the five step and ramp tables {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    }

    #[test]
    fn wave_shapes() {
        let t = Tables::get();
        let tr = t.wave(Wave::Triangle);
        assert!((tr[0] + 1.0).abs() < 1e-5);
        assert!((tr[1024] - 1.0).abs() < 1e-5);
        let st = t.wave(Wave::SoftTriangle);
        assert!((st[1024] - 0.853515625).abs() < 1e-4);
        let sq = t.wave(Wave::Square);
        assert!((sq[9 * 2048..].iter().sum::<f32>() / 2048.0 - 0.5).abs() < 1e-5);
        // saw levels keep half the mean
        let m: f64 = t.bl_saw[29 * BL_LEN..29 * BL_LEN + 2048].iter().map(|x| *x as f64).sum::<f64>() / 2048.0;
        assert!((m + 1.0 / 4096.0).abs() < 1e-6);
        assert_eq!(t.bl_saw[0], t.bl_saw[2048]);
    }
}
