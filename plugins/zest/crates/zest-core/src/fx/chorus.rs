// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Chorus (`pages/fx-chorus.md`).

use super::shared::{all_zero, lfo_hz, load2, lowpass_coefficients, store2, tap, tri, zeros, Down4, Frame, Up4, WetLp};
use crate::dsp::F4;
use crate::params::id;

const SIZE: usize = 8192;

pub struct Chorus {
    fs: f32,
    dt: f32,
    line_l: Box<[f32; SIZE]>,
    line_r: Box<[f32; SIZE]>,
    w: u32,
    ph: [f32; 3],
    /// L1, R1, L2, R2, L3, R3, in 4x samples.
    d: [f32; 6],
    wcur: f32,
    wstep: f32,
    reset: bool,
    /// Only zero input since the last clearing: lines, resamplers and filter states all zero.
    clean: bool,
    lp: WetLp,
    up: Up4,
    down: Down4,
    ty: u8,
    d01: f32,
    inc: [f32; 3],
    fs4: f32,
}

#[inline(always)]
fn tri_pair(ph: f32) -> (f32, f32) {
    let wrap = |v: f32| {
        let v = if v >= 1.0 { v - 1.0 } else { v };
        if v < 0.0 {
            v + 1.0
        } else {
            v
        }
    };
    (tri(wrap(ph - 0.25)), tri(wrap(ph + 0.25)))
}

impl Chorus {
    pub fn new(fs: f32) -> Chorus {
        Chorus {
            fs,
            dt: 1.0 / fs,
            line_l: zeros(),
            line_r: zeros(),
            w: 0,
            ph: [0.0; 3],
            d: [0.0; 6],
            wcur: 0.0,
            wstep: 0.0,
            reset: true,
            clean: true,
            lp: WetLp::new(),
            up: Up4::new(),
            down: Down4::new(),
            ty: 0,
            d01: 0.0,
            inc: [0.0; 3],
            fs4: 0.0,
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

    pub fn setup(&mut self, raw: &[f32], o: usize) {
        self.ty = raw[id::CHRS1_TYPE + o].round().clamp(0.0, 2.0) as u8;
        let r = raw[id::CHRS1_RATE + o] * 0.005 + 0.5;
        self.d01 = raw[id::CHRS1_DEPTH + o] * 0.01;
        let wet = raw[id::CHRS1_WET + o] * 0.01;
        let inv = 1.0f32 / 256.0;
        if self.ty < 2 {
            let cut = if self.ty == 0 { 122.0 } else { 140.0 };
            self.lp.set(lowpass_coefficients(cut, self.dt), self.reset);
        }
        if self.reset {
            self.reset = false;
            if !self.clean {
                self.line_l.fill(0.0);
                self.line_r.fill(0.0);
                self.up = Up4::new();
                self.down = Down4::new();
            }
            self.wcur = wet;
            self.clean = self.lp.states_zero();
        }
        self.wstep = (wet - self.wcur) * inv;
        let a = lfo_hz(r);
        let dt4 = self.dt * 0.25;
        if self.ty < 2 {
            self.inc[0] = a * ((dt4 * 16.0) * 4.0);
        } else {
            let i1 = (a * dt4) * 16.0;
            self.inc = [i1, i1 * 5.1, i1 * 3.33];
        }
        self.fs4 = (self.fs * 4.0) / 1000.0;
    }

    /// The target delays of this chunk, in the order of `d`, after the LFO step.
    #[inline]
    fn targets(&mut self) -> [f32; 6] {
        let n = if self.ty == 2 { 3 } else { 1 };
        for j in 0..n {
            let p = self.inc[j] + self.ph[j];
            self.ph[j] = if p > 1.0 { p - 1.0 } else { p };
        }
        let (d01, fs4) = (self.d01, self.fs4);
        let mut t = [0.0f32; 6];
        match self.ty {
            0 => {
                let dk = (d01 * 4.0) + (d01 * 4.0);
                let (a, b) = tri_pair(self.ph[0]);
                t[0] = (((a - 1.0) * 0.9 + 1.0) * a * dk + 1.7) * fs4;
                t[1] = (((b - 1.0) * 0.9 + 1.0) * b * dk + 1.7) * fs4;
            }
            1 => {
                let dk = d01 * 15.0;
                let ph = self.ph[0];
                let (u, v) = (ph - 0.125, ph + 0.125);
                let s_l = ((u + u) * 3.1415927f32).sin();
                let s_r = ((v + v) * 3.1415927f32).sin();
                t[0] = ((s_l * 0.5 + 0.5) * dk + 5.0) * fs4;
                t[1] = ((s_r * 0.5 + 0.5) * dk + 5.0) * fs4;
            }
            _ => {
                let d6 = d01 * 6.0;
                for j in 0..2 {
                    let (a, b) = tri_pair(self.ph[j]);
                    t[2 * j] = (a * d6 + d01) * fs4;
                    t[2 * j + 1] = (b * d6 + d01) * fs4;
                }
                let (a, b) = tri_pair(self.ph[2]);
                let (d2, base) = (d01 + d01, d6 + d01);
                t[4] = (a * d2 + base) * fs4;
                t[5] = (b * d2 + base) * fs4;
            }
        }
        t
    }

    pub fn chunk(&mut self, l: &mut [f32; 16], r: &mut [f32; 16]) {
        let t = self.targets();
        let taps = if self.ty == 2 { 6 } else { 2 };
        let mut st = [0.0f32; 6];
        for i in 0..taps {
            st[i] = (t[i] - self.d[i]) / 64.0;
        }
        if self.clean && all_zero(l, r) {
            for _ in 0..64 {
                for i in 0..taps {
                    self.d[i] = st[i] + self.d[i];
                }
                self.wcur = self.wstep + self.wcur;
                if self.ty < 2 && self.lp.ramping() {
                    self.lp.advance();
                }
            }
            self.w = self.w.wrapping_add(64);
            return;
        }
        self.clean = false;
        let mut x = [[0.0f32; 2]; 64];
        self.up.run(l, r, &mut x);
        match self.ty {
            2 => self.frames_ensemble(&st, &mut x),
            ty => self.frames_single(ty, &st, &mut x),
        }
        self.down.run(&x, l, r);
    }

    #[inline]
    fn frames_single(&mut self, ty: u8, st: &[f32; 6], x: &mut [Frame; 64]) {
        let g = if ty == 0 { 1.007f32 } else { 0.707f32 };
        let dry = F4::splat(0.707);
        let (mut dl, mut dr) = (self.d[0], self.d[1]);
        let (sl, sr) = (st[0], st[1]);
        let mut wcur = self.wcur;
        let wstep = self.wstep;
        let mut w = self.w;
        let (line_l, line_r) = (&mut *self.line_l, &mut *self.line_r);
        let mut lp = self.lp;
        for i in 0..64 {
            dl = sl + dl;
            dr = sr + dr;
            wcur = wstep + wcur;
            let [x_l, x_r] = x[i];
            line_l[w as usize & (SIZE - 1)] = x_l;
            line_r[w as usize & (SIZE - 1)] = x_r;
            w = w.wrapping_add(1);
            let tl = tap(line_l, w, dl);
            let tr = tap(line_r, w, dr);
            let y = lp.step(F4::new(tl, tr, 0.0, 0.0));
            let out = F4::splat(wcur * g) * y + load2(&x[i]) * dry;
            store2(out, &mut x[i]);
        }
        self.lp = lp;
        self.d[0] = dl;
        self.d[1] = dr;
        self.wcur = wcur;
        self.w = w;
    }

    #[inline]
    fn frames_ensemble(&mut self, st: &[f32; 6], x: &mut [Frame; 64]) {
        let mut d = self.d;
        let mut wcur = self.wcur;
        let wstep = self.wstep;
        let mut w = self.w;
        let (line_l, line_r) = (&mut *self.line_l, &mut *self.line_r);
        for i in 0..64 {
            for k in 0..6 {
                d[k] = st[k] + d[k];
            }
            wcur = wstep + wcur;
            let [x_l, x_r] = x[i];
            line_l[w as usize & (SIZE - 1)] = x_l;
            line_r[w as usize & (SIZE - 1)] = x_r;
            w = w.wrapping_add(1);
            let sum_l = (tap(line_l, w, d[2]) + tap(line_l, w, d[0])) + tap(line_l, w, d[4]);
            let sum_r = (tap(line_r, w, d[3]) + tap(line_r, w, d[1])) + tap(line_r, w, d[5]);
            let (dry, gw) = (1.0 - wcur * 0.4, wcur * 0.7);
            x[i] = [dry * x_l + gw * sum_l, dry * x_r + gw * sum_r];
        }
        self.d = d;
        self.wcur = wcur;
        self.w = w;
    }
}
