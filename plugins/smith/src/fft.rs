// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The inverse real FFT that turns a voice's harmonics into one period of its wave, run in passes so a table's work
//! can spread over the blocks before it is needed.
//
// spec: plugins/smith.md ("Rendering")

use std::f64::consts::PI;
use std::sync::Arc;

pub type Cpx = [f32; 2];

#[inline(always)]
fn mul(a: Cpx, b: Cpx) -> Cpx {
    [a[0] * b[0] - a[1] * b[1], a[0] * b[1] + a[1] * b[0]]
}

/// The tables of one size, shared by the voices using it.
#[derive(Debug)]
pub struct Plan {
    /// The real output's length.
    pub len: usize,
    /// `exp(+2 pi i j / m)` for the half-size complex transform of `m = len / 2` points.
    tw: Vec<Cpx>,
    /// `i exp(+2 pi i k / len)`, folding the real spectrum into the half-size one.
    post: Vec<Cpx>,
}

impl Plan {
    /// The plan for `len` real points, a power of two of at least 4.
    pub fn new(len: usize) -> Arc<Plan> {
        assert!(len.is_power_of_two() && len >= 4);
        let m = len / 2;
        let cis = |x: f64| [x.cos() as f32, x.sin() as f32];
        let tw = (0..m).map(|j| cis(2.0 * PI * j as f64 / m as f64)).collect();
        let post = (0..m).map(|k| mul([0.0, 1.0], cis(2.0 * PI * k as f64 / len as f64))).collect();
        Arc::new(Plan { len, tw, post })
    }
}

/// One transform in progress: load the spectrum, run passes until it is done, then take the wave.
#[derive(Debug)]
pub struct Ifft {
    plan: Arc<Plan>,
    a: Vec<Cpx>,
    b: Vec<Cpx>,
    n: usize,
    s: usize,
    in_a: bool,
}

impl Ifft {
    pub fn new(plan: Arc<Plan>) -> Ifft {
        let m = plan.len / 2;
        Ifft { plan, a: vec![[0.0; 2]; m], b: vec![[0.0; 2]; m], n: 1, s: 1, in_a: true }
    }

    pub fn len(&self) -> usize {
        self.plan.len
    }

    /// The passes a transform takes.
    pub fn passes(&self) -> usize {
        ((self.plan.len / 2).trailing_zeros() as usize).div_ceil(2)
    }

    /// Loads the spectrum of `x[t] = sum over k of 2 Re(X[k] exp(+2 pi i k t / len))`, from `spec[k]` for `k` in
    /// `0 .. len / 2` (`spec` may be shorter; the rest is 0, and so is `spec[0]`: no constant part).
    pub fn load(&mut self, spec: &[Cpx]) {
        let m = self.plan.len / 2;
        let at = |k: usize| if k > 0 && k < spec.len() { spec[k] } else { [0.0, 0.0] };
        for k in 0..m {
            let x = at(k);
            let y = at((m - k) % m);
            let e = [x[0] + y[0], x[1] - y[1]];
            let o = mul([x[0] - y[0], x[1] + y[1]], self.plan.post[k]);
            self.a[k] = [e[0] + o[0], e[1] + o[1]];
        }
        self.n = m;
        self.s = 1;
        self.in_a = true;
    }

    /// Whether the transform has finished.
    pub fn done(&self) -> bool {
        self.n <= 1
    }

    /// One pass: a Stockham radix-4 step, a radix-2 one at an odd end. Does nothing when done.
    pub fn pass(&mut self) {
        let (n, s) = (self.n, self.s);
        if n <= 1 {
            return;
        }
        let m = self.plan.len / 2;
        let step = m / n;
        let (x, y): (&[Cpx], &mut [Cpx]) = if self.in_a { (&self.a, &mut self.b) } else { (&self.b, &mut self.a) };
        let tw = &self.plan.tw;
        if n >= 4 {
            let n1 = n / 4;
            for p in 0..n1 {
                let w1 = tw[p * step];
                let w2 = tw[2 * p * step];
                let w3 = tw[3 * p * step];
                let (i0, i1, i2, i3) = (s * p, s * (p + n1), s * (p + 2 * n1), s * (p + 3 * n1));
                let (o0, o1, o2, o3) = (s * 4 * p, s * (4 * p + 1), s * (4 * p + 2), s * (4 * p + 3));
                for q in 0..s {
                    let (a, b, c, d) = (x[q + i0], x[q + i1], x[q + i2], x[q + i3]);
                    let apc = [a[0] + c[0], a[1] + c[1]];
                    let amc = [a[0] - c[0], a[1] - c[1]];
                    let bpd = [b[0] + d[0], b[1] + d[1]];
                    // i (b - d)
                    let jbd = [d[1] - b[1], b[0] - d[0]];
                    y[q + o0] = [apc[0] + bpd[0], apc[1] + bpd[1]];
                    y[q + o1] = mul(w1, [amc[0] + jbd[0], amc[1] + jbd[1]]);
                    y[q + o2] = mul(w2, [apc[0] - bpd[0], apc[1] - bpd[1]]);
                    y[q + o3] = mul(w3, [amc[0] - jbd[0], amc[1] - jbd[1]]);
                }
            }
            self.n = n1;
            self.s = s * 4;
        } else {
            let h = n / 2;
            for p in 0..h {
                let w = tw[p * step];
                let (i0, i1) = (s * p, s * (p + h));
                let (o0, o1) = (s * 2 * p, s * (2 * p + 1));
                for q in 0..s {
                    let (a, b) = (x[q + i0], x[q + i1]);
                    y[q + o0] = [a[0] + b[0], a[1] + b[1]];
                    y[q + o1] = mul(w, [a[0] - b[0], a[1] - b[1]]);
                }
            }
            self.n = h;
            self.s = s * 2;
        }
        self.in_a = !self.in_a;
    }

    /// Runs every pass left.
    pub fn finish(&mut self) {
        while !self.done() {
            self.pass();
        }
    }

    /// Copies the finished wave into `out` (`len` points).
    pub fn take(&self, out: &mut [f32]) {
        let z = if self.in_a { &self.a } else { &self.b };
        for (k, c) in z.iter().enumerate() {
            out[2 * k] = c[0];
            out[2 * k + 1] = c[1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(len: usize, spec: &[Cpx]) -> Vec<f32> {
        let mut f = Ifft::new(Plan::new(len));
        f.load(spec);
        let mut passes = 0;
        while !f.done() {
            f.pass();
            passes += 1;
        }
        assert_eq!(passes, f.passes());
        let mut out = vec![0.0; len];
        f.take(&mut out);
        out
    }

    /// `2 Re(X e^{i w})` per harmonic, for even and odd powers of two.
    #[test]
    fn it_sums_its_harmonics() {
        for len in [16usize, 32, 64, 128, 1024, 2048] {
            let mut spec = vec![[0.0f32; 2]; 8];
            spec[1] = [0.5, 0.0];
            spec[3] = [0.0, 0.25];
            spec[7] = [-0.1, 0.1];
            let w = wave(len, &spec);
            for (m, &v) in w.iter().enumerate() {
                let t = 2.0 * PI * m as f64 / len as f64;
                let want = t.cos() - 0.5 * (3.0 * t).sin() - 0.2 * (7.0 * t).cos() - 0.2 * (7.0 * t).sin();
                assert!((f64::from(v) - want).abs() < 1e-4, "{len} at {m}: {v} vs {want}");
            }
        }
    }
}
