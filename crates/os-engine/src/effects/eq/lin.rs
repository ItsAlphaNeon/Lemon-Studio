// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! LIN: a linear-phase filter whose magnitude is the bands'. A table of gains from the bands' coefficients becomes a
//! kernel of `N / 2` taps, convolved by FFT in blocks of `N / 2`; a new kernel fades in over the crossfade time while
//! the old one fades out. The transforms are in doubles, rounded to single wherever the page holds singles.
//!
// spec: plugins/fruity-parametric-eq-2.md ("LIN")

use std::f64::consts::{LN_2, TAU};

use super::design::BandCoefs;
use super::fft::Fft;

/// The crossfade time in seconds for the crossfade settings 0..3 (other values: the first).
pub fn fade_time(setting: i32) -> f32 {
    match setting {
        1 => 0.25,
        2 => 0.01,
        3 => 0.02,
        _ => 0.1,
    }
}

/// The transform length `N` for a size setting at the project's `rate`.
pub fn transform_len(size: i32, rate: f64) -> usize {
    let n0 = 512usize << size.clamp(0, 7);
    if rate < 44100.0 {
        return n0 / 2;
    }
    let up = (((rate * (1.0 / 44100.0)).ln() / LN_2 + 0.5) as f32).trunc() as u32;
    (n0 << up.min(8)).min(65536)
}

/// The delay LIN reports: `3 N / 4`.
pub fn delay(n: usize) -> usize {
    3 * n / 4
}

/// What the table needs of one band.
pub struct TableBand {
    /// The band's frequency in Hz (a single, as the design has it).
    pub freq: f32,
    /// Width value below 8192 and band pass, notch or peaking.
    pub narrow: bool,
    /// Its sections, when it is enabled and its type is not 0.
    pub sections: Option<BandCoefs>,
}

/// `p2(x)` for a single `x > 0`: a cubic for `2^x` that reads 0.999862 at whole powers of two.
fn p2(x: f32) -> f32 {
    let k = x.trunc() as i32;
    let fr = f64::from(x - k as f32);
    (2f64.powi(k) * (fr * (fr * (fr * 0.079064658211 + 0.224328091473) + 0.69659850257) + 0.999862219597)) as f32
}

/// A table entry from the bands' squared magnitude: `2^(dB / 6)`.
fn gain_of(mag: f32) -> f32 {
    let v = if mag == 0.0 { -1000.0 } else { ((f64::from(mag).log10() * 0.5555555555555556) as f32).clamp(-1000.0, 1000.0) };
    if v.is_nan() {
        return 1.0;
    }
    let t = v * 18.0;
    let x = t * (1.0f64 / 6.0) as f32;
    if t < -100.0 {
        0.0
    } else if t > 100.0 {
        2f64.powf(f64::from(x)) as f32
    } else if x == 0.0 {
        1.0
    } else if x > 0.0 {
        p2(x)
    } else {
        1.0 / p2(-x)
    }
}

/// The table of `N / 2 + 1` gains: entry `i` for the frequency `i * rate / N` (`rate` the project's), the bands'
/// coefficients evaluated at `band_rate`.
pub fn table(n: usize, rate: f64, band_rate: f64, bands: &[TableBand]) -> Vec<f32> {
    let step = (rate / n as f64) as f32;
    let angle = (1.0 / band_rate) * TAU;
    let mut out = vec![0f32; n / 2 + 1];
    let mut prev = 0f32;
    for (i, e) in out.iter_mut().enumerate() {
        let mut f = i as f32 * step;
        if f < 0.1 {
            f = 0.1;
        }
        if let Some(b) = bands.iter().find(|b| b.narrow && prev < b.freq && b.freq <= f) {
            f = b.freq;
        }
        prev = f;
        let w = (f64::from(f) * angle) as f32;
        let (s1, c1) = f64::from(w).sin_cos();
        let (s2, c2) = f64::from(w * 2.0).sin_cos();
        let mut mag = 1f32;
        for secs in bands.iter().filter_map(|b| b.sections.as_ref()) {
            let mut gb = 1f32;
            for c in secs.as_slice() {
                let re = (c.b0 * c2 + c.b1 * c1) + c.b2;
                let im = c.b1 * s1 + c.b0 * s2;
                let rd = (c2 + c.a1 * c1) + c.a2;
                let id = c.a1 * s1 + s2;
                gb = ((f64::from(gb) * (re * re + im * im)) / (rd * rd + id * id)) as f32;
            }
            mag *= gb;
        }
        *e = gain_of(mag);
    }
    out[n / 2] = out[n / 2 - 1];
    out
}

/// A kernel's transform, `N` bins, held as singles.
pub struct Kernel {
    re: Vec<f32>,
    im: Vec<f32>,
}

/// The zero-phase response of a table, `N` singles with its centre at `N / 2`.
fn response(t: &[f32], fft: &Fft, n: usize) -> Vec<f32> {
    let (mut re, mut im) = (vec![0f64; n], vec![0f64; n]);
    for k in 0..=n / 2 {
        let v = f64::from(if k % 2 == 1 { -t[k] } else { t[k] });
        re[k] = v;
        if k > 0 && k < n / 2 {
            re[n - k] = v;
        }
    }
    fft.run(&mut re, &mut im, true);
    let scale = 1.0f32 / n as f32;
    re.iter().map(|&v| (v as f32) * scale).collect()
}

/// The `N / 2` taps: the response's values around its centre (from `N / 4`) under a sine window.
fn taps(t: &[f32], fft: &Fft, n: usize) -> Vec<f32> {
    let z = response(t, fft, n);
    let h = n / 2;
    let step = 1.0f32 / (h - 1) as f32;
    (0..h).map(|i| z[n / 4 + i] * (f64::from((i as f32 * std::f32::consts::PI) * step).sin() as f32)).collect()
}

/// The kernel of a table: its taps transformed with `N / 2` zeros after them.
pub fn kernel(t: &[f32], fft: &Fft, n: usize) -> Kernel {
    let (mut re, mut im) = (vec![0f64; n], vec![0f64; n]);
    for (r, &v) in re.iter_mut().zip(&taps(t, fft, n)) {
        *r = f64::from(v);
    }
    fft.run(&mut re, &mut im, false);
    Kernel { re: re.iter().map(|&v| v as f32).collect(), im: im.iter().map(|&v| v as f32).collect() }
}

/// One convolver: a kernel (none = silence), per channel an input block of `H` and a result of `N`, and the position.
struct Convolver {
    kernel: Option<Kernel>,
    input: [Vec<f32>; 2],
    result: [Vec<f32>; 2],
    pos: usize,
}

impl Convolver {
    fn new(n: usize) -> Convolver {
        Convolver { kernel: None, input: [vec![0.0; n / 2], vec![0.0; n / 2]], result: [vec![0.0; n], vec![0.0; n]], pos: 0 }
    }

    fn empty(&mut self) {
        for c in 0..2 {
            self.input[c].fill(0.0);
            self.result[c].fill(0.0);
        }
        self.pos = 0;
    }

    fn frame(&mut self, x: [f32; 2], fft: &Fft, re: &mut [f64], im: &mut [f64]) -> [f32; 2] {
        let Some(k) = &self.kernel else { return [0.0; 2] };
        let out = [self.result[0][self.pos], self.result[1][self.pos]];
        self.input[0][self.pos] = x[0];
        self.input[1][self.pos] = x[1];
        self.pos += 1;
        let h = self.input[0].len();
        if self.pos == h {
            self.pos = 0;
            let n = 2 * h;
            let scale = 1.0f32 / n as f32;
            for c in 0..2 {
                for (r, &x) in re.iter_mut().zip(&self.input[c]) {
                    *r = f64::from(x);
                }
                re[h..].fill(0.0);
                im.fill(0.0);
                fft.run(re, im, false);
                for i in 0..n {
                    let (xr, xi) = (re[i] as f32, im[i] as f32);
                    let (kr, ki) = (k.re[i], k.im[i]);
                    re[i] = f64::from(xr * kr - xi * ki);
                    im[i] = f64::from(xr * ki + xi * kr);
                }
                fft.run(re, im, true);
                let res = &mut self.result[c];
                res.copy_within(h.., 0);
                res[h..].fill(0.0);
                for (r, &y) in res.iter_mut().zip(re.iter()) {
                    *r = scale * (y as f32) + *r;
                }
            }
        }
        out
    }
}

/// The two convolvers, the last `N` input frames and the crossfade between them.
pub struct Lin {
    n: usize,
    rate: f64,
    fft: Fft,
    conv: [Convolver; 2],
    history: Vec<[f32; 2]>,
    /// Where the next frame goes in `history` (the oldest kept one).
    hpos: usize,
    cnt: i64,
    len: i64,
    /// The convolver whose turn it is next, and the one that got the last table.
    next: usize,
    newest: usize,
    pending: Option<(Kernel, f32)>,
    re: Vec<f64>,
    im: Vec<f64>,
}

impl Lin {
    /// Nothing handed over yet: silence until the first table.
    pub fn new(n: usize, rate: f64) -> Lin {
        Lin {
            n,
            rate,
            fft: Fft::new(n),
            conv: [Convolver::new(n), Convolver::new(n)],
            history: vec![[0.0; 2]; n],
            hpos: 0,
            cnt: 0,
            len: 0,
            next: 0,
            newest: 0,
            pending: None,
            re: vec![0.0; n],
            im: vec![0.0; n],
        }
    }

    pub fn n(&self) -> usize {
        self.n
    }

    /// True while a crossfade is under way.
    pub fn fading(&self) -> bool {
        self.cnt < self.len
    }

    /// Hands over a new table (`N / 2 + 1` gains) with its crossfade time in seconds.
    pub fn hand_over(&mut self, table: &[f32], time: f32) {
        self.pending = Some((kernel(table, &self.fft, self.n), time));
    }

    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (l, r) in l.iter_mut().zip(r.iter_mut()) {
            [*l, *r] = self.frame([*l, *r]);
        }
    }

    fn frame(&mut self, x: [f32; 2]) -> [f32; 2] {
        let (fft, re, im) = (&self.fft, &mut self.re, &mut self.im);
        if self.cnt >= self.len
            && let Some((k, time)) = self.pending.take()
        {
            self.cnt = 0;
            self.len = ((self.rate as f32) * time).trunc() as i64;
            let c = self.next;
            self.next ^= 1;
            self.newest = c;
            let conv = &mut self.conv[c];
            conv.empty();
            conv.kernel = Some(k);
            for i in 0..self.n {
                conv.frame(self.history[(self.hpos + i) % self.n], fft, re, im);
            }
        }
        self.history[self.hpos] = x;
        self.hpos = (self.hpos + 1) % self.n;
        if self.cnt >= self.len {
            return self.conv[self.newest].frame(x, fft, re, im);
        }
        let a = self.conv[0].frame(x, fft, re, im);
        let b = self.conv[1].frame(x, fft, re, im);
        let mut f = self.cnt as f32 / self.len as f32;
        if self.newest == 1 {
            f = 1.0 - f;
        }
        self.cnt += 1;
        [b[0] * (1.0 - f) + a[0] * f, b[1] * (1.0 - f) + a[1] * f]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: plugins/fruity-parametric-eq-2.md "LIN": "The transform length"
    #[test]
    fn transform_lengths_and_delays() {
        assert_eq!((0..5).map(|s| transform_len(s, 44100.0)).collect::<Vec<_>>(), vec![512, 1024, 2048, 4096, 8192]);
        assert_eq!(transform_len(2, 48000.0), 2048);
        assert_eq!(transform_len(2, 32000.0), 1024);
        assert_eq!(transform_len(2, 62366.0), 2048);
        assert_eq!(transform_len(2, 62367.0), 4096);
        assert_eq!(transform_len(2, 96000.0), 4096);
        assert_eq!(transform_len(7, 192000.0), 65536);
        assert_eq!([2048, 512, 1024, 4096, 8192].map(delay), [1536, 384, 768, 3072, 6144]);
        assert_eq!([0, 1, 2, 3, 9].map(fade_time), [0.1, 0.25, 0.01, 0.02, 0.1]);
    }

    // spec: plugins/fruity-parametric-eq-2.md "LIN": "The table"
    #[test]
    fn gains_double_every_6_db() {
        assert_eq!(gain_of(1.0), 1.0);
        // 18 dB (mag = 10^1.8) gives about 8 * 0.999862, not 7.943.
        let g = gain_of(10f32.powf(1.8));
        assert!((g - 8.0 * 0.999862).abs() < 1e-3, "{g}");
        let g = gain_of(10f32.powf(-1.2));
        assert!((g - 1.0 / (4.0 * 0.999862)).abs() < 1e-3, "{g}");
        assert_eq!(gain_of(0.0), 0.0);
        assert_eq!(p2(2.0), (4.0 * 0.999862219597) as f32);
    }

    /// A table with one narrow peaking band and a shelf: the narrow band's centre is put into the table.
    #[test]
    fn narrow_bands_enter_the_table() {
        use super::super::design::{band, Design, Stored};
        let rate = 44100.0;
        let peak = Stored { level: 1200, freq: 30000, width: 4000, kind: super::super::PEAKING, order: 0 };
        let coefs = band(&peak, Design::Ordinary(rate));
        let freq = super::super::design::freq_hz(peak.freq);
        let t = table(2048, rate, rate, &[TableBand { freq, narrow: true, sections: Some(coefs) }]);
        // The entry whose frequency first reaches the centre holds the full +12 dB: 2^2 through the cubic, which reads
        // 4 * 0.999862 at x = 2 and 2 * 1.999854 just below it.
        let i = (f64::from(freq) / (rate / 2048.0)).ceil() as usize;
        assert!((t[i] - 4.0).abs() < 1e-3, "{} {}", t[i], p2(2.0));
        assert!(t[i - 1] < t[i] && t[i + 1] < t[i]);
        assert_eq!(t[1024], t[1023]);
    }

    // spec: plugins/fruity-parametric-eq-2.md "LIN": "From a table to a kernel"
    #[test]
    fn kernels_are_symmetric_about_n_4() {
        use super::super::design::{band, freq_hz, Design, Stored};
        let (n, rate) = (2048, 44100.0);
        let fft = Fft::new(n);
        let bands: Vec<TableBand> = [(900, 20000, 30000, 5), (-1200, 40000, 20000, 6), (0, 60000, 36068, 3)]
            .iter()
            .map(|&(level, freq, width, kind)| {
                let s = Stored { level, freq, width, kind, order: 0 };
                TableBand { freq: freq_hz(freq), narrow: false, sections: Some(band(&s, Design::Ordinary(rate))) }
            })
            .collect();
        let t = table(n, rate, rate, &bands);
        // The zero-phase response is even about N / 2.
        let z = response(&t, &fft, n);
        let big = z.iter().fold(0f32, |m, v| m.max(v.abs()));
        for d in 1..n / 2 {
            assert!((z[n / 2 - d] - z[n / 2 + d]).abs() <= big * 1e-6, "{d}: {} {}", z[n / 2 - d], z[n / 2 + d]);
        }
        // The taps keep it about tap N / 4, up to the window's half-sample offset ((H - 1) / 2 is its centre).
        let k = taps(&t, &fft, n);
        let h = n / 2;
        assert_eq!(k.len(), h);
        assert_eq!(k[0], 0.0);
        for d in 1..h / 2 {
            assert!((k[h / 2 - d] - k[h / 2 + d]).abs() <= big * 3.2e-3, "{d}");
        }
        // A flat table gives a single tap at N / 4 (the window's value there).
        let flat = taps(&vec![1f32; n / 2 + 1], &fft, n);
        assert!((flat[n / 4] - 1.0).abs() < 1e-4, "{}", flat[n / 4]);
        assert!(flat.iter().enumerate().all(|(i, v)| i == n / 4 || v.abs() < 1e-6));
    }

    // spec: plugins/fruity-parametric-eq-2.md "LIN": "A convolver", "Frames, and the change of kernel"
    #[test]
    fn a_flat_table_delays_by_3n_4() {
        let n = 512;
        let flat = vec![1f32; n / 2 + 1];
        // An impulse comes out 3 N / 4 frames later once the first fade (from silence) is over.
        let mut lin = Lin::new(n, 44100.0);
        lin.hand_over(&flat, 0.001);
        let len = (44100.0f32 * 0.001) as usize;
        let total = len + 2 * n;
        let mut l = vec![0f32; total];
        let mut r = vec![0f32; total];
        l[len] = 1.0;
        r[len] = -0.5;
        lin.process(&mut l, &mut r);
        let at = len + delay(n);
        let peak = (0..total).max_by(|&a, &b| l[a].abs().total_cmp(&l[b].abs())).unwrap();
        assert_eq!(peak, at);
        assert!((l[at] - 1.0).abs() < 1e-4 && (r[at] + 0.5).abs() < 1e-4, "{} {}", l[at], r[at]);
        assert!(l.iter().enumerate().all(|(i, v)| i == at || v.abs() < 1e-6));
    }

    /// A new table fades in over `len` frames while the old kernel fades out.
    #[test]
    fn a_new_table_crossfades() {
        let n = 512;
        let rate = 44100.0;
        let mut lin = Lin::new(n, rate);
        lin.hand_over(&vec![1f32; n / 2 + 1], 0.0);
        let mut l = vec![0.5f32; 4 * n];
        let mut r = l.clone();
        lin.process(&mut l, &mut r);
        // A constant through the flat kernel: about the input after the delay.
        assert!((l[4 * n - 1] - 0.5).abs() < 1e-3, "{}", l[4 * n - 1]);
        // Half the gain from now on, over 0.01 s = 441 frames.
        lin.hand_over(&vec![0.5f32; n / 2 + 1], 0.01);
        let mut l = vec![0.5f32; 600];
        let mut r = l.clone();
        lin.process(&mut l, &mut r);
        assert!(!lin.fading());
        assert!((l[0] - 0.5).abs() < 2e-3 && (l[220] - 0.375).abs() < 2e-3 && (l[500] - 0.25).abs() < 2e-3, "{} {} {}", l[0], l[220], l[500]);
        for w in l[..441].windows(2) {
            assert!(w[1] <= w[0] + 1e-6);
        }
    }
}
