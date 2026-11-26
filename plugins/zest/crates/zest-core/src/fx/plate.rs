// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Plate (`pages/fx-plate.md`).
//!
//! Every fixed ring starts at index 0 and moves by one per frame, so they share one write counter. Each ring masks it by its size.

use super::shared::{all_zero, flush, trunc_i32, zeros, Down2, Up2};
use crate::params::id;

const LINE_S: usize = 8192;
const LINE_L: usize = 65536;
const PRE: usize = 262144;
const MOD: usize = 65536;
const INV: f32 = 1.0 / 128.0;

const TAPS: [f32; 14] = [266.0, 2974.0, 1913.0, 1996.0, 1990.0, 187.0, 1066.0, 353.0, 3627.0, 1228.0, 2673.0, 2111.0, 335.0, 121.0];
const MODS: [f32; 2] = [672.0, 908.0];
const LINES: [f32; 10] = [142.0, 107.0, 379.0, 277.0, 4453.0, 4217.0, 1800.0, 2656.0, 3720.0, 3163.0];

/// Modulated allpass with limiter (section 6).
struct ModAp {
    buf: Box<[f32; MOD]>,
    r: u32,
    r2: u32,
    w: u32,
    x1: f32,
    y1: f32,
    c: f32,
    x2: f32,
    y2: f32,
    c2: f32,
    dcx: f32,
    dcy: f32,
    g_old: f32,
    g_new: f32,
    rms: f32,
    last: f32,
    xf: f32,
    sumsq: f32,
}

impl ModAp {
    fn new() -> ModAp {
        ModAp {
            buf: zeros(),
            r: 0,
            r2: 0,
            w: 0,
            x1: 0.0,
            y1: 0.0,
            c: 0.0,
            x2: 0.0,
            y2: 0.0,
            c2: 0.0,
            dcx: 0.0,
            dcy: 0.0,
            g_old: 0.0,
            g_new: 0.0,
            rms: 0.0,
            last: 0.0,
            xf: 0.0,
            sumsq: 0.0,
        }
    }

    fn prepare(&mut self, d: f32, fs: f32, reset: bool) {
        const M: u32 = MOD as u32 - 1;
        let mut i = 0;
        if d == self.last && !reset {
            self.x2 = self.x1;
            self.y2 = self.y1;
            self.r2 = self.r;
            self.c2 = self.c;
        } else {
            self.last = d;
            let v = (d * fs + 0.0).clamp(2.0, 65533.0);
            let v = v - 0.3;
            i = trunc_i32(v);
            self.r2 = self.w.wrapping_sub(i as u32) & M;
            self.x2 = 0.0;
            self.y2 = 0.0;
            let f = (v - (i - 1) as f32) - 0.7;
            self.c2 = (1.0 - f) / (f + 1.0);
            if !reset {
                let at = |k: u32| self.buf[(self.r2.wrapping_sub(k) & M) as usize];
                let mut y = at(10);
                let mut xx = at(9);
                for k in (1..=8).rev() {
                    let x = at(k);
                    y = (x - y) * self.c2 + xx;
                    xx = x;
                }
                self.x2 = xx;
                self.y2 = y;
            }
        }
        self.xf = 0.0;
        self.sumsq = 0.0;
        if reset {
            self.w = i as u32;
            self.r = 0;
            self.r2 = 0;
            self.c = self.c2;
            self.dcx = 0.0;
            self.dcy = 0.0;
            self.x1 = 0.0;
            self.y1 = 0.0;
            self.rms = 1.0;
            self.g_old = 1.0;
        }
        self.g_new = if self.rms > 1.0 { 1.0 / self.rms } else { 1.0 };
    }

    /// One frame; returns (y, n).
    #[inline(always)]
    fn frame(&mut self, cross: f32, input: f32, decay: f32, g6: f32, dcc: f32) -> (f32, f32) {
        const M: usize = MOD - 1;
        let x = self.buf[self.r as usize & M];
        let y1 = (x - self.y1) * self.c + self.x1;
        self.x1 = x;
        self.y1 = y1;
        let x = self.buf[self.r2 as usize & M];
        let y2 = (x - self.y2) * self.c2 + self.x2;
        self.x2 = x;
        self.y2 = y2;
        self.r = self.r.wrapping_add(1);
        self.r2 = self.r2.wrapping_add(1);
        let y = y1 + (y2 - y1) * self.xf;
        self.xf = self.xf + INV;
        let n = (cross * decay + input) + y * g6;
        let g = (self.g_new - self.g_old) * self.xf + self.g_old;
        let u = (n - self.dcx) + dcc * self.dcy;
        self.dcx = n;
        self.dcy = u;
        self.buf[self.w as usize & M] = g * u;
        self.w = self.w.wrapping_add(1);
        self.sumsq = self.sumsq + u * u;
        (y, n)
    }

    fn wrap_up(&mut self) {
        self.flush();
        self.g_old = self.g_new;
        self.r = self.r2;
        self.x1 = self.x2;
        self.y1 = self.y2;
        self.c = self.c2;
        let m = self.sumsq * INV;
        let e = (if m > 0.0 { m } else { 0.0 }).sqrt();
        self.rms = ((e - self.rms) * 0.01 + self.rms) * 0.999;
        self.flush();
    }

    fn flush(&mut self) {
        self.x1 = flush(self.x1);
        self.y1 = flush(self.y1);
        self.dcx = flush(self.dcx);
        self.dcy = flush(self.dcy);
        self.c = flush(self.c);
        self.g_old = flush(self.g_old);
        self.rms = flush(self.rms);
    }
}

pub struct Plate {
    fs: f32,
    dt: f32,
    pre: Box<[f32; PRE]>,
    short: [Box<[f32; LINE_S]>; 4],
    long: [Box<[f32; LINE_L]>; 6],
    ap: [ModAp; 2],
    w: u32,
    ph: f32,
    bw: f32,
    d1: f32,
    d2: f32,
    up: Up2,
    down: Down2,
    reset: bool,
    clean: bool,
    a: f32,
    fd: f32,
    g625: f32,
    g707: f32,
    g6: f32,
    d_line: [u32; 10],
    d_pre: u32,
    taps: [u32; 14],
    dcc: f32,
    dry: f32,
    wet: f32,
    decay: f32,
}

impl Plate {
    pub fn new(fs: f32) -> Plate {
        Plate {
            fs,
            dt: 1.0 / fs,
            pre: zeros(),
            short: [zeros(), zeros(), zeros(), zeros()],
            long: [zeros(), zeros(), zeros(), zeros(), zeros(), zeros()],
            ap: [ModAp::new(), ModAp::new()],
            w: 0,
            ph: 0.0,
            bw: 0.0,
            d1: 0.0,
            d2: 0.0,
            up: Up2::new(),
            down: Down2::new(),
            reset: true,
            clean: true,
            a: 0.0,
            fd: 0.0,
            g625: 0.0,
            g707: 0.0,
            g6: 0.0,
            d_line: [0; 10],
            d_pre: 0,
            taps: [0; 14],
            dcc: 0.0,
            dry: 0.0,
            wet: 0.0,
            decay: 0.0,
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
        let (fs, dt) = (self.fs, self.dt);
        let p = (dt * 0.5) * 128.0 + self.ph;
        self.ph = if p > 1.0 { p - 1.0 } else { p };
        let v = self.ph;
        let v = if v >= 1.0 { v - 1.0 } else { v };
        let v = if v < 0.0 { v + 1.0 } else { v };
        let lfo = super::shared::tri(v);

        let damp = raw[id::PLATE1_DAMP + o];
        self.a = 1.0 - damp * 0.00998;
        self.fd = damp * 0.00999;
        let diff = raw[id::PLATE1_DIFFUSION + o] * 0.01;
        self.g625 = diff * 0.625;
        self.g707 = diff * 0.707;
        self.g6 = diff * 0.6;
        let size = raw[id::PLATE1_SIZE + o] * 0.01;
        let s = size + size;
        let scaled = |l: f32| trunc_i32(((l * s) * fs) / 29761.0);

        let md = ((fs * s) / 29761.0) * 16.0;
        let reset = self.reset;
        for (ap, l) in self.ap.iter_mut().zip(MODS) {
            let lp = scaled(l) as f32;
            let mut d = ((lfo * md + lp) * 1000.0) * dt * 0.001;
            if d <= 0.0001 {
                d = 0.0001;
            }
            ap.prepare(d, fs, reset);
        }

        for (i, l) in LINES.into_iter().enumerate() {
            let size = if i < 4 { LINE_S } else { LINE_L };
            self.d_line[i] = scaled(l).clamp(0, size as i32 - 1) as u32;
        }
        let p = raw[id::PLATE1_PREDELAY + o] * 0.001;
        self.d_pre = trunc_i32((p + p) * fs).clamp(0, PRE as i32 - 1) as u32;

        if reset {
            self.reset = false;
            if !self.clean {
                self.pre.fill(0.0);
                for b in &mut self.short {
                    b.fill(0.0);
                }
                for b in &mut self.long {
                    b.fill(0.0);
                }
                for ap in &mut self.ap {
                    ap.buf.fill(0.0);
                }
                self.up = Up2::new();
                self.down = Down2::new();
            }
            self.bw = 0.0;
            self.d1 = 0.0;
            self.d2 = 0.0;
            self.clean = true;
        }

        for (t, l) in self.taps.iter_mut().zip(TAPS) {
            *t = scaled(l) as u32;
        }
        self.dcc = 1.0 - dt * 126.0;
        self.dry = raw[id::PLATE1_DRY + o] * 0.01;
        self.wet = raw[id::PLATE1_WET + o] * 0.006;
        self.decay = raw[id::PLATE1_DECAY + o] * 0.01;
    }

    pub fn chunk(&mut self, l: &mut [f32; 16], r: &mut [f32; 16]) {
        if self.clean && all_zero(l, r) {
            self.w = self.w.wrapping_add(32);
            for ap in &mut self.ap {
                ap.r = ap.r.wrapping_add(32);
                ap.r2 = ap.r2.wrapping_add(32);
                ap.w = ap.w.wrapping_add(32);
                ap.xf = ap.xf + 0.25;
            }
            return;
        }
        self.clean = false;
        let mut x = [[0.0f32; 2]; 32];
        self.up.run(l, r, &mut x);

        let (g625, g707, g6, decay, dcc, dry, wet) = (self.g625, self.g707, self.g6, self.decay, self.dcc, self.dry, self.wet);
        let (one_a, a, fd, one_fd) = (1.0 - self.a, self.a, self.fd, 1.0 - self.fd);
        let (mut bw, mut d1, mut d2) = (self.bw, self.d1, self.d2);
        let mut w = self.w;
        let t = self.taps;
        let dl = self.d_line;
        let dp = self.d_pre;
        let [s0, s1, s2, s3] = &mut self.short;
        let [l4, l5, l6, l7, l8, l9] = &mut self.long;
        let [ap_a, ap_b] = &mut self.ap;
        let pre = &mut *self.pre;
        const MS: usize = LINE_S - 1;
        const ML: usize = LINE_L - 1;
        const MP: usize = PRE - 1;

        for i in 0..32 {
            let [x_l, x_r] = x[i];
            let wi = w as usize;
            // a read at delay d, before this frame's write
            let rd = |d: u32| w.wrapping_sub(1).wrapping_sub(d) as usize;
            let p = pre[rd(dp) & MP];
            pre[wi & MP] = (x_r + x_l) * 0.5;
            bw = one_a * bw + p * a;

            let mut v = bw;
            let rr = s0[rd(dl[0]) & MS];
            v = v - rr * g625;
            s0[wi & MS] = v;
            v = v * g625 + rr;
            let rr = s1[rd(dl[1]) & MS];
            v = v - rr * g625;
            s1[wi & MS] = v;
            v = v * g625 + rr;
            let rr = s2[rd(dl[2]) & MS];
            v = v - rr * g707;
            s2[wi & MS] = v;
            v = v * g707 + rr;
            let rr = s3[rd(dl[3]) & MS];
            v = v - rr * g707;
            s3[wi & MS] = v;
            v = v * g707 + rr;
            let input = v;

            let c_a = l9[rd(dl[9]) & ML];
            let c_b = l8[rd(dl[8]) & ML];
            let (y_a, n_a) = ap_a.frame(c_a, input, decay, g6, dcc);
            let (y_b, n_b) = ap_b.frame(c_b, input, decay, g6, dcc);
            let t_a = y_a - n_a * g6;
            let t_b = y_b - n_b * g6;

            let r4 = l4[rd(dl[4]) & ML];
            l4[wi & ML] = t_a;
            let r5 = l5[rd(dl[5]) & ML];
            l5[wi & ML] = t_b;
            d1 = fd * d1 + one_fd * r4;
            d2 = fd * d2 + one_fd * r5;

            let mut u = d1 * decay;
            let r6 = l6[rd(dl[6]) & ML];
            u = u - r6 * g707;
            l6[wi & ML] = u;
            u = u * g707 + r6;
            let mut e = d2 * decay;
            let r7 = l7[rd(dl[7]) & ML];
            e = e - r7 * g707;
            l7[wi & ML] = e;
            e = e * g707 + r7;
            l8[wi & ML] = u;
            l9[wi & ML] = e;

            // taps, after the writes: `t` samples before the newest, which sits at w
            let tp = |t: u32| w.wrapping_sub(t) as usize & ML;
            let sl = (((((l5[tp(t[1])] + l5[tp(t[0])]) - l7[tp(t[2])]) + l9[tp(t[3])]) - l4[tp(t[4])]) - l6[tp(t[5])]) - l8[tp(t[6])];
            let sr = (((((l4[tp(t[7])] + l4[tp(t[8])]) - l6[tp(t[9])]) + l8[tp(t[10])]) - l5[tp(t[11])]) - l7[tp(t[12])]) - l9[tp(t[13])];
            x[i] = [sl * wet + x_l * dry, sr * wet + x_r * dry];
            w = w.wrapping_add(1);
        }
        self.bw = bw;
        self.d1 = d1;
        self.d2 = d2;
        self.w = w;
        self.down.run(&x, l, r);
    }

    pub fn wrap_up(&mut self) {
        self.ap[0].wrap_up();
        self.ap[1].wrap_up();
    }
}
