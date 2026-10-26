// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The spectrum view: a spectrogram of the shown part, time left to right, frequency linear from 0 at the bottom to
//! Nyquist at the top, coloured by one of Tesla's own gradients (not Edison's images).
//!
// spec: plugins/edison/window.md "Wave area" (spectrum view), plugins/edison/menus.md "View" (Display settings),
// plugins/tesla/README.md "Version 1" (views)

/// The gradients: name and colour stops (position, 0xRRGGBB) from silence to full level.
pub const GRADIENTS: [(&str, &[(f32, u32)]); 4] = [
    ("Ember", &[(0.0, 0x000000), (0.3, 0x5a0a06), (0.55, 0xc1400c), (0.8, 0xf5b324), (1.0, 0xfffbe6)]),
    ("Lemon", &[(0.0, 0x000000), (0.35, 0x34320a), (0.65, 0xa8a020), (0.85, 0xffe24a), (1.0, 0xffffff)]),
    ("Ice", &[(0.0, 0x000000), (0.35, 0x0a1a4a), (0.65, 0x1b78b0), (0.85, 0x7fe0f0), (1.0, 0xffffff)]),
    ("Graphite", &[(0.0, 0x000000), (1.0, 0xffffff)]),
];

/// The level range shown: −100 dB (black) to 0 dB (the gradient's end).
const FLOOR_DB: f32 = -100.0;

/// The colour of level `t` (0..1) in gradient `g`.
pub fn color(g: usize, t: f32) -> u32 {
    let stops = GRADIENTS[g.min(GRADIENTS.len() - 1)].1;
    let t = t.clamp(0.0, 1.0);
    let k = stops.iter().position(|&(p, _)| p >= t).unwrap_or(stops.len() - 1).max(1);
    let ((p0, c0), (p1, c1)) = (stops[k - 1], stops[k]);
    let f = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1.0 };
    let ch = |s: u32| -> u32 {
        let (a, b) = (((c0 >> s) & 255) as f32, ((c1 >> s) & 255) as f32);
        ((a + (b - a) * f).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// An in-place radix-2 complex FFT of a fixed power-of-two size.
pub struct Fft {
    n: usize,
    cos: Vec<f32>,
    sin: Vec<f32>,
}

impl Fft {
    pub fn new(n: usize) -> Fft {
        assert!(n.is_power_of_two());
        let w = |k: usize| -2.0 * std::f64::consts::PI * k as f64 / n as f64;
        Fft { n, cos: (0..n / 2).map(|k| w(k).cos() as f32).collect(), sin: (0..n / 2).map(|k| w(k).sin() as f32).collect() }
    }

    pub fn run(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        let bits = n.trailing_zeros();
        for i in 0..n {
            let j = i.reverse_bits() >> (usize::BITS - bits);
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let step = n / len;
            for s in (0..n).step_by(len) {
                for k in 0..len / 2 {
                    let (c, si) = (self.cos[k * step], self.sin[k * step]);
                    let (u, v) = (s + k, s + k + len / 2);
                    let tr = re[v] * c - im[v] * si;
                    let ti = re[v] * si + im[v] * c;
                    re[v] = re[u] - tr;
                    im[v] = im[u] - ti;
                    re[u] += tr;
                    im[u] += ti;
                }
            }
            len *= 2;
        }
    }
}

/// A spectrogram picture `w` × `h` px (0xRRGGBB rows, top first) of frames `start .. start + w · per_px` of a mono
/// signal (`at(i)` = frame `i`, 0 outside), with `bands` frequency bands (an FFT of 2 · `bands`).
pub fn render(at: &dyn Fn(isize) -> f32, frames: usize, start: f64, per_px: f64, w: usize, h: usize, bands: usize, gradient: usize) -> Vec<u32> {
    let n = (bands * 2).next_power_of_two().max(64);
    let fft = Fft::new(n);
    let win: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect();
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    let mut out = vec![0u32; w * h];
    // A Hann window's coherent gain is ½: a full-scale sine reads 0 dB.
    let norm = 4.0 / n as f32;
    let mut col = vec![0f32; n / 2];
    for x in 0..w {
        let centre = start + (x as f64 + 0.5) * per_px;
        if centre < 0.0 || centre >= frames as f64 {
            continue;
        }
        let c = centre as isize - n as isize / 2;
        for i in 0..n {
            re[i] = at(c + i as isize) * win[i];
            im[i] = 0.0;
        }
        fft.run(&mut re, &mut im);
        for (k, v) in col.iter_mut().enumerate() {
            let m = (re[k] * re[k] + im[k] * im[k]).sqrt() * norm;
            let db = 20.0 * m.max(1e-9).log10();
            *v = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
        }
        for y in 0..h {
            // Linear frequency: the top row is Nyquist.
            let f0 = (h - 1 - y) as f32 / h as f32 * (n / 2) as f32;
            let f1 = (h - y) as f32 / h as f32 * (n / 2) as f32;
            let (k0, k1) = (f0 as usize, (f1.ceil() as usize).clamp(f0 as usize + 1, n / 2));
            let v = col[k0..k1].iter().fold(0f32, |m, &v| m.max(v));
            out[y * w + x] = color(gradient, v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sine_lands_in_its_row() {
        let rate = 8000.0;
        let sine = |i: isize| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate).sin();
        let (w, h) = (4, 100);
        let px = render(&sine, 100000, 10000.0, 100.0, w, h, 512, 3);
        // 1 kHz of 4 kHz Nyquist: a quarter up from the bottom, row 75; the brightest there.
        let lum = |y: usize| px[y * w + 1] & 255;
        let best = (0..h).max_by_key(|&y| lum(y)).unwrap();
        assert!((74..=76).contains(&best), "{best}");
        assert!(lum(best) > 240 && lum(10) < 60);
    }

    #[test]
    fn gradient_ends() {
        assert_eq!(color(3, 0.0), 0x000000);
        assert_eq!(color(3, 1.0), 0xffffff);
        assert_eq!(color(0, 1.0), 0xfffbe6);
    }
}
