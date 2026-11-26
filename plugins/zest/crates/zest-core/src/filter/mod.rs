// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! What the filter models share: the tuning tables, the per-rate constants, the oversampling filters, the VCA and its output
//! stage (`pages/filter-ladder.md` sections 1, 3, 4, 7, 11; `pages/vca.md`).

pub mod bite;
pub mod kernel;
pub mod multimode;
pub mod solver;
pub mod uhbie;

use crate::dsp::{db_gain, hz, one_minus_via_double, parse_tsv, rcpn, tan_d, tclip, F4};
use crate::settings::{Settings, VcaSet};

/// A per-step one-pole pair: steps 30..150.
#[derive(Clone)]
pub struct Tuning {
    pub a: [f32; 121],
    pub b: [f32; 121],
}

impl Tuning {
    /// Lookup(x) of `filter-ladder.md` section 7.
    #[inline(always)]
    pub fn lookup(&self, x: f32) -> (f32, f32) {
        let s = x - 30.0;
        let mut i = s as i32;
        if i < 0 {
            i = 0;
        }
        if i > 119 {
            i = 119;
        }
        let r = s - i as f32;
        let o = 1.0 - r;
        let i = i as usize;
        ((o * self.a[i]) + (r * self.a[i + 1]), (r * self.b[i + 1]) + (o * self.b[i]))
    }
}

/// The per-oversampling constants of `filter-ladder.md` section 3.
#[derive(Clone, Copy, Debug)]
pub struct OsConst {
    pub o: usize,
    /// Os factor
    pub of: f32,
    /// 1 / O
    pub finv: f32,
    pub co: f32,
    pub cb: f32,
    pub cs: f32,
    pub cn: f32,
    pub ch: f32,
    pub kc: f32,
}

impl OsConst {
    fn new(o: usize, fs: f32, ts: f32) -> OsConst {
        let (c_co, c_cb, c_cs) = if o == 4 { (22.0f32, 55.5f32, 9600.0f32) } else { (44.0, 111.0, 19200.0) };
        let t = 1.0 - ts * c_cs;
        let fo = fs * o as f32;
        OsConst {
            o,
            of: if o == 4 { 0.25 } else { 0.5 },
            finv: if o == 4 { 0.25 } else { 0.5 },
            co: one_minus_via_double(ts, c_co),
            cb: one_minus_via_double(ts, c_cb),
            cs: t * t,
            cn: (1.0 / (fo + 5000.0)) * (fo - 5000.0),
            ch: one_minus_via_double(ts, 13.500001),
            kc: if o == 4 { 0.0625 } else { 0.125 },
        }
    }
}

/// Numbers fixed per sample rate, built at construction.
pub struct FilterTables {
    pub fs: f32,
    pub ts: f32,
    /// G = Gain(-12).
    pub g: f32,
    pub t12: f32,
    pub rn1: f32,
    pub rn114: f32,
    pub os2: OsConst,
    pub os4: OsConst,
    /// Ladder: draft, fast, great.
    pub ladder: [Tuning; 3],
    /// Cascade: [clean, rough][draft, fast, great].
    pub cascade: [[Tuning; 3]; 2],
    pub multimode: multimode::Tables,
    pub bite: bite::Tables,
    pub uhbie: uhbie::Tables,
}

/// The column of a tuning TSV for this rate.
fn band(fs: f32) -> &'static str {
    if fs <= 44100.0 {
        "44k"
    } else if fs <= 48000.0 {
        "48k"
    } else {
        "hi"
    }
}

/// The steps 30..150 of column `name` of a tuning file, as float32 pitches.
pub fn tuning_column(text: &str, name: &str) -> [f32; 121] {
    let (head, rows) = parse_tsv(text);
    let col = head.iter().position(|h| h == name).unwrap_or_else(|| panic!("tuning column {name}")) - 1;
    let mut v = [0.0f32; 121];
    for (i, r) in rows.iter().take(121).enumerate() {
        v[i] = r[col] as f32;
    }
    v
}

/// The tuning file's value as float32 from its shortest text (a double parse then rounding is the same for these).
pub fn tuning_name(text_band_prefix: &str, fs: f32) -> String {
    format!("{}_{}", text_band_prefix, band(fs))
}

impl FilterTables {
    pub fn new(fs: f32) -> FilterTables {
        let ts = 1.0 / fs;
        let g = db_gain(-12.0);
        const LADDER: &str = include_str!("../../data/tuning-ladder.tsv");
        const CASCADE: &str = include_str!("../../data/tuning-cascade.tsv");
        let ladder_one = |acc: &str, of: f32| -> Tuning {
            let v = tuning_column(LADDER, &tuning_name(acc, fs));
            let mut t = Tuning { a: [0.0; 121], b: [0.0; 121] };
            for i in 0..121 {
                let h = hz(v[i]);
                let arg = (((h * of) * 0.868226) * ts) * 3.1415927;
                let mut x = tan_d(arg);
                x = x + x;
                t.a[i] = (x - 1.0) / (x + 1.0);
                t.b[i] = x / (x + 1.0);
            }
            t
        };
        let cascade_one = |acc: &str, color: &str, of: f32| -> Tuning {
            let v = tuning_column(CASCADE, &tuning_name(&format!("{acc}_{color}"), fs));
            let mut t = Tuning { a: [0.0; 121], b: [0.0; 121] };
            for i in 0..121 {
                let h = hz(v[i]);
                let arg = ((((h * of) * 4.0) * ts) * 3.1415927) * 0.5;
                let x = tan_d(arg);
                t.a[i] = (x - 1.0) / (x + 1.0);
                t.b[i] = x / (x + 1.0);
            }
            t
        };
        FilterTables {
            fs,
            ts,
            g,
            t12: tclip(0.12),
            rn1: rcpn(1.0),
            rn114: rcpn(1.14),
            os2: OsConst::new(2, fs, ts),
            os4: OsConst::new(4, fs, ts),
            ladder: [ladder_one("draft", 0.5), ladder_one("fast", 0.5), ladder_one("great", 0.25)],
            cascade: [
                [cascade_one("draft", "clean", 0.5), cascade_one("fast", "clean", 0.5), cascade_one("great", "clean", 0.25)],
                [cascade_one("draft", "rough", 0.5), cascade_one("fast", "rough", 0.5), cascade_one("great", "rough", 0.25)],
            ],
            multimode: multimode::Tables::new(fs),
            bite: bite::Tables::new(fs),
            uhbie: uhbie::Tables::new(fs),
        }
    }
}

/// Zest's matrix values of a call (`pages/modulation.md`); `None` of each target when no slot drives it.
#[derive(Clone, Copy)]
pub struct MatrixSums {
    pub any_pitch: bool,
    pub any_shape: bool,
    pub any_cut: bool,
    pub any_vol: bool,
    pub any_pan: bool,
    pub pitch: [f32; 16],
    pub shape: [f32; 16],
    pub cut: [f32; 16],
    pub vol: [f32; 16],
    pub pan: [f32; 16],
    pub res: f32,
}

impl MatrixSums {
    pub const fn zero() -> MatrixSums {
        MatrixSums {
            any_pitch: false,
            any_shape: false,
            any_cut: false,
            any_vol: false,
            any_pan: false,
            pitch: [0.0; 16],
            shape: [0.0; 16],
            cut: [0.0; 16],
            vol: [0.0; 16],
            pan: [0.0; 16],
            res: 0.0,
        }
    }
}

/// One voice's inputs and outputs for a filter call.
pub struct FilterIo<'a> {
    pub rows: &'a [f32; 448],
    pub mx: &'a MatrixSums,
    pub a: &'a [f32],
    pub b: &'a [f32],
    pub l: &'a mut [f32],
    pub r: &'a mut [f32],
    /// The cutoff spread of the voice.
    pub spread: f32,
    pub clear: bool,
}

/// The settings a filter call reads.
#[derive(Clone, Copy)]
pub struct FilterCtx<'a> {
    pub s: &'a Settings,
    pub ft: &'a FilterTables,
    pub acc: i32,
    pub c: usize,
    pub n: usize,
    pub fb_gate: bool,
}

/// The VCA inside a filter kernel (`pages/vca.md`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Vca {
    pub pan: f32,
    pub vol: f32,
    ipan: f32,
    ivol: f32,
    pub g: f32,
    pub pl: f32,
    pub pr: f32,
    ig: f32,
    ipl: f32,
    ipr: f32,
    /// The last pre-pan signal.
    pub gg: f32,
    pub s1: f32,
    pub s2: f32,
    started: bool,
}

impl Vca {
    /// Knob ramps, per call.
    #[inline]
    pub fn begin(&mut self, set: &VcaSet, n: usize) {
        let pan_t = {
            let a = if set.pan < 100.0 { set.pan } else { 100.0 };
            if a > -100.0 {
                a
            } else {
                -100.0
            }
        };
        let vol_t = {
            let v = set.volume * 0.01;
            let a = if v < 4.0 { v } else { 4.0 };
            if a > 0.0 {
                a
            } else {
                0.0
            }
        };
        if !self.started {
            self.started = true;
            self.pan = pan_t;
            self.vol = vol_t;
        }
        let f = 1.0 / (16.0 * n as f32);
        self.ipan = (pan_t - self.pan) * f;
        self.ivol = (vol_t - self.vol) * f;
    }

    /// Per control step: `p` = 4c + j, `j` the step in the call.
    #[inline(always)]
    pub fn control(&mut self, set: &VcaSet, rows: &[f32; 448], mx: &MatrixSums, p: usize, j: usize, kst: f32) {
        self.pan = self.pan + self.ipan;
        self.vol = self.vol + self.ivol;
        let mut m = (set.mod_depth * 0.01) * rows[set.mod_src * 16 + p];
        if mx.any_vol {
            m = m + mx.vol[j];
        }
        let src = if set.env1 { rows[14 * 16 + p] } else { rows[5 * 16 + p] };
        let g_t = (m + 1.0) * src;
        let mut x = set.pan_depth * rows[set.pan_src * 16 + p];
        if mx.any_pan {
            x = x + mx.pan[j];
        }
        let x = x + self.pan;
        let q = {
            let a = if x < 100.0 { x } else { 100.0 };
            if a > -100.0 {
                a
            } else {
                -100.0
            }
        };
        let p_r = q * 0.004999999888241291 + 0.5;
        let p_l = 1.0 - p_r;
        let (g_l, g_r) = (p_l.sqrt(), p_r.sqrt());
        self.ig = (g_t - self.g) * kst;
        self.ipl = (g_l * self.vol - self.pl) * kst;
        self.ipr = (g_r * self.vol - self.pr) * kst;
    }

    /// Per oversampled step: input `o`, returns (L, R) before the Mode shaper; leaves G in `gg`.
    #[inline(always)]
    pub fn tick(&mut self, o: f32) -> (f32, f32) {
        self.g = self.g + self.ig;
        self.pl = self.pl + self.ipl;
        self.pr = self.pr + self.ipr;
        self.gg = self.g * o;
        (self.pl * self.gg, self.pr * self.gg)
    }

    /// The two-band output stage on both channels (`vca.md`, Output stage).
    #[inline(always)]
    pub fn stage(&mut self, cs: f32, rn1: f32, t12: f32, l: f32, r: f32) -> (f32, f32) {
        let lx = l * 0.675;
        let rx = r * 0.675;
        self.s1 = (cs * self.s1) + ((1.0 - cs) * lx);
        self.s2 = (cs * self.s2) + ((1.0 - cs) * rx);
        let v = F4::new((lx - self.s1) - 0.12, (rx - self.s2) - 0.12, (self.s1 * 2.7) - 0.12, (self.s2 * 2.7) - 0.12).tclip().arr();
        let z = t12 + t12;
        (((v[0] * rn1) + (v[2] * rn1) + z) * 0.532, ((v[1] * rn1) + (v[3] * rn1) + z) * 0.532)
    }
}

/// The filter's oversampling chains: up 2 sections, down 6 sections, per lane (`filter-ladder.md` section 4).
pub struct OsCoefs {
    pub up: [F4; 2],
    pub down: [F4; 6],
}

#[inline(always)]
pub fn os_coefs() -> OsCoefs {
    let l = |a: f32, b: f32| F4::new(a, a, b, b);
    OsCoefs {
        up: [l(0.12073212, 0.39036217), l(0.66320205, 0.8907868)],
        down: [
            l(0.036681503, 0.13654763),
            l(0.27463177, 0.42313862),
            l(0.561099, 0.67754006),
            l(0.76974183, 0.83988965),
            l(0.8922608, 0.931542),
            l(0.96209455, 0.9878164),
        ],
    }
}

/// The chains of one kernel.
#[derive(Clone, Copy)]
pub struct Resamplers {
    pub u1: [F4; 3],
    pub u2: [F4; 3],
    pub d1: [F4; 7],
    pub d2: [F4; 7],
}

impl Resamplers {
    pub fn new() -> Resamplers {
        Resamplers { u1: [F4::zero(); 3], u2: [F4::zero(); 3], d1: [F4::zero(); 7], d2: [F4::zero(); 7] }
    }

    /// Input frames from base samples: `fa`, `fb` take `len * o` values.
    #[inline]
    pub fn up(&mut self, co: &OsCoefs, o: usize, a: &[f32], b: &[f32], fa: &mut [f32], fb: &mut [f32]) {
        use crate::dsp::chain_step;
        if o == 4 {
            for i in 0..a.len() {
                let p = chain_step(&mut self.u1, &co.up, F4::new(a[i], b[i], a[i], b[i])).arr();
                let x = chain_step(&mut self.u2, &co.up, F4::new(p[0], p[1], p[0], p[1])).arr();
                let y = chain_step(&mut self.u2, &co.up, F4::new(p[2], p[3], p[2], p[3])).arr();
                let (ra, rb) = (&mut fa[i * 4..i * 4 + 4], &mut fb[i * 4..i * 4 + 4]);
                ra[0] = x[0];
                rb[0] = x[1];
                ra[1] = x[2];
                rb[1] = x[3];
                ra[2] = y[0];
                rb[2] = y[1];
                ra[3] = y[2];
                rb[3] = y[3];
            }
        } else {
            for i in 0..a.len() {
                let p = chain_step(&mut self.u1, &co.up, F4::new(a[i], b[i], a[i], b[i])).arr();
                fa[i * 2] = p[0];
                fb[i * 2] = p[1];
                fa[i * 2 + 1] = p[2];
                fb[i * 2 + 1] = p[3];
            }
        }
    }

    /// Output frames down to base samples.
    #[inline]
    pub fn down(&mut self, co: &OsCoefs, o: usize, fl: &[f32], fr: &[f32], l: &mut [f32], r: &mut [f32]) {
        use crate::dsp::chain_step;
        if o == 4 {
            for i in 0..l.len() {
                let (al, ar) = (&fl[i * 4..i * 4 + 4], &fr[i * 4..i * 4 + 4]);
                let la = self.d1[6].arr();
                let y = chain_step(&mut self.d1, &co.down, F4::new(al[0], ar[0], al[1], ar[1])).arr();
                let g0 = (la[2] + y[0], la[3] + y[1]);
                let y2 = chain_step(&mut self.d1, &co.down, F4::new(al[2], ar[2], al[3], ar[3])).arr();
                let g1 = (y[2] + y2[0], y[3] + y2[1]);
                let lb = self.d2[6].arr();
                let z = chain_step(&mut self.d2, &co.down, F4::new(g0.0, g0.1, g1.0, g1.1)).arr();
                l[i] = (lb[2] + z[0]) * 0.25;
                r[i] = (lb[3] + z[1]) * 0.25;
            }
        } else {
            for i in 0..l.len() {
                let lb = self.d1[6].arr();
                let z = chain_step(&mut self.d1, &co.down, F4::new(fl[i * 2], fr[i * 2], fl[i * 2 + 1], fr[i * 2 + 1])).arr();
                l[i] = (lb[2] + z[0]) * 0.5;
                r[i] = (lb[3] + z[1]) * 0.5;
            }
        }
    }
}
