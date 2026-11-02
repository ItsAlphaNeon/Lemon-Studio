// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Delay (`pages/fx-delay.md`).

use super::shared::{all_zero, tap, zeros, Down2, Up2};
use crate::dsp::{exp2, pmax, pmin};
use crate::params::id;

const SIZE: usize = 262144;
const MASK: u32 = SIZE as u32 - 1;

pub struct Delay {
    fs: f32,
    dt: f32,
    line: Box<[f32; SIZE]>,
    w: u32,
    ph: f32,
    /// Left, centre, right: float32 on purpose, their rounding is part of the sound.
    d: [f32; 3],
    step: [f32; 3],
    s: f32,
    xh: f32,
    yh: f32,
    up: Up2,
    down: Down2,
    reset: bool,
    clean: bool,
    sv: f32,
    cv: f32,
    fb: f32,
    dry: f32,
    c: f32,
    b0: f32,
    c1: f32,
}

impl Delay {
    pub fn new(fs: f32) -> Delay {
        Delay {
            fs,
            dt: 1.0 / fs,
            line: zeros(),
            w: 0,
            ph: 0.0,
            d: [0.0; 3],
            step: [0.0; 3],
            s: 0.0,
            xh: 0.0,
            yh: 0.0,
            up: Up2::new(),
            down: Down2::new(),
            reset: true,
            clean: true,
            sv: 0.0,
            cv: 0.0,
            fb: 0.0,
            dry: 0.0,
            c: 0.0,
            b0: 0.0,
            c1: 0.0,
        }
    }

    /// Takes the full path from here on, for the test that the silent shortcut is exact.
    #[cfg(test)]
    pub fn force_full(&mut self) {
        self.clean = false;
    }

    pub fn reactivate(&mut self) {
        self.reset = true;
    }

    pub fn setup(&mut self, raw: &[f32], o: usize, bpm: f64) {
        let (fs, dt) = (self.fs, self.dt);
        let left = raw[id::DELAY1_LEFT_DELAY + o];
        let center = raw[id::DELAY1_CENTER_DELAY + o];
        let right = raw[id::DELAY1_RIGHT_DELAY + o];
        self.sv = raw[id::DELAY1_SIDE_VOL + o] * 0.01;
        self.cv = raw[id::DELAY1_CENTER_VOL + o] * 0.01;
        self.fb = raw[id::DELAY1_FEEDBACK + o] * 0.01;
        let hp = raw[id::DELAY1_HP + o] * 0.01;
        let lp = raw[id::DELAY1_LP + o] * 0.01;
        self.dry = raw[id::DELAY1_DRY + o] * 0.01;
        let wow = raw[id::DELAY1_WOW + o] * 0.005;
        let inv = 1.0f32 / 128.0;

        if self.reset && !self.clean {
            self.up = Up2::new();
            self.down = Down2::new();
        }
        let unit = ((fs as f64 * 60.0) / (bpm + bpm)) as f32;

        let p = (128.0 * dt) * (wow * 0.2 + 0.7) + self.ph;
        self.ph = if p >= 1.0 { p - 1.0 } else { p };
        let ph = self.ph;
        let clamp = |x: f32| pmax(pmin(x, 262143.0), 1.0);
        let (a1, a2) = (ph + 0.33, ph + 0.66);
        let tl = clamp(((ph + ph) * 3.1415927f32).sin() * 150.0 * wow + unit * left);
        let tc = clamp(((a1 + a1) * 3.1415927f32).sin() * 115.0 * wow + unit * center);
        let tr = clamp(((a2 + a2) * 3.1415927f32).sin() * 150.0 * wow + unit * right);

        let hz = exp2(((lp * 70.0 + 80.0) - 69.0) / 12.0) * 110.0;
        let w = hz * 3.1415927;
        let w = (w + w) * dt;
        let w = pmin(3.1415927, w);
        let b = 2.0 - w.cos();
        let mut c = b - (b * b - 1.0).sqrt();
        if lp > 0.75 {
            c = c * ((1.0 - lp) * 4.0);
        }
        self.c = c;

        let q = 1.0 - hp;
        let hz = exp2(((1.0 - q * q) * 110.0 - 69.0) / 12.0) * 110.0;
        let hz = pmax(pmin(hz, 8000.0), 10.0);
        let wh = hz * 6.2831855;
        let fs2 = (fs as f64 + fs as f64) as f32;
        let a = (1.0 / (fs2 + wh) as f64) as f32;
        self.b0 = a * fs2;
        self.c1 = (fs2 - wh) * a;

        let t = [tl, tc, tr];
        if self.reset {
            self.reset = false;
            if !self.clean {
                self.line.fill(0.0);
            }
            self.s = 0.0;
            self.xh = 0.0;
            self.yh = 0.0;
            self.d = t;
            self.clean = true;
        }
        for i in 0..3 {
            self.step[i] = (t[i] - self.d[i]) * inv;
        }
    }

    pub fn chunk(&mut self, l: &mut [f32; 16], r: &mut [f32; 16]) {
        if self.clean && all_zero(l, r) {
            for _ in 0..32 {
                for i in 0..3 {
                    self.d[i] = self.step[i] + self.d[i];
                }
            }
            self.w = self.w.wrapping_add(32);
            return;
        }
        self.clean = false;
        let mut xs = [[0.0f32; 2]; 32];
        self.up.run(l, r, &mut xs);

        let [mut dl, mut dc, mut dr] = self.d;
        let [sl, sc, sr] = self.step;
        let (sv, cv, fb, dry, c, b0, c1) = (self.sv, self.cv, self.fb, self.dry, self.c, self.b0, self.c1);
        let (mut s, mut xh, mut yh, mut w) = (self.s, self.xh, self.yh, self.w);
        let line = &mut *self.line;
        for i in 0..32 {
            dl = sl + dl;
            dc = sc + dc;
            dr = sr + dr;
            let tap_c = tap(line, w, dc);
            let tap_l = tap(line, w, dl);
            let tap_r = tap(line, w, dr);
            let e = tap_c * 0.7071 * cv;
            let [x_l, x_r] = xs[i];
            xs[i] = [x_l * dry + (tap_l * sv + e), x_r * dry + (tap_r * sv + e)];
            let x = (x_r + x_l) * 0.7071 + tap_c * fb;
            s = (s - x) * c + x;
            let y = (-b0 * xh + s * b0) + c1 * yh;
            yh = y;
            xh = s;
            line[(w & MASK) as usize] = y;
            w = w.wrapping_add(1);
        }
        self.d = [dl, dc, dr];
        self.s = s;
        self.xh = xh;
        self.yh = yh;
        self.w = w;
        self.down.run(&xs, l, r);
    }
}
