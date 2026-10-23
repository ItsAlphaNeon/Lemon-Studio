// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The EQ's processing, shared by the engine's own slot effect and the Lemon Parametric EQ plugin: the main level, then
//! LIN, or the bands as plain processing, HQ Legacy, HQ+ or HQ Enhanced; the silence gate, the bands' countdowns and the
//! clean-up at block ends. A block is one call of [`Equalizer::process`].
//!
// spec: plugins/fruity-parametric-eq-2.md ("Which processing a state selects", "Behaviour", "State and timing")

use super::design::{self, BandCoefs, Coefs, Design, Stored};
use super::hq::{self, Decimator, HalfBand, TAPS, TINY};
use super::lin::{self, Lin, TableBand};
use super::{BANDS, BAND_PASS, BAND_STOP, MASK_BYTE, PEAKING};

/// The 36 parameters: `group * 7 + band` (level, frequency, width, type, order), then the main level.
pub const PARAMS: usize = 36;
pub const MAIN: usize = 35;
/// A block is silent when no sample's magnitude is above this (1e-6).
const SILENT: u32 = 0x358637BD;
/// Pre-roll: the buffers of silence FL runs before a render.
const PREROLL_BLOCK: usize = 2048;

/// Each parameter's range.
pub fn range(i: usize) -> (i32, i32) {
    match (i / BANDS, i) {
        (_, MAIN) | (0, _) => (-1800, 1800),
        (1 | 2, _) => (0, 65536),
        (3, _) => (0, 7),
        _ => (-3, 3),
    }
}

/// The processing a state selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Plain,
    HqLegacy,
    HqPlus,
    HqEnhanced,
    Lin,
}

/// The settings of the state that decide the processing (besides the parameters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// The int at 292: bit 0 = HQ on, bits 1..8 = the HQ mode.
    pub hq: i32,
    /// LIN's size setting (byte 309) and crossfade setting (byte 313).
    pub lin_size: i32,
    pub lin_fade: i32,
    /// Byte 317: 0x01 LIN, 0x02 HQ Enhanced (the other bits do nothing to the sound and are dropped).
    pub flags: u8,
    /// Bit `k` = band `k + 1` enabled.
    pub mask: u8,
}

impl Default for Options {
    /// A state of version 3 or lower with HQ off.
    fn default() -> Self {
        Options { hq: 0, lin_size: 2, lin_fade: 0, flags: 0, mask: 0x7f }
    }
}

fn int(state: &[u8], at: usize) -> Option<i32> {
    state.get(at..at + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
}

impl Options {
    /// From a saved state: the options of versions 4 and up, the defaults before.
    pub fn from_state(state: &[u8]) -> Options {
        Options::read(state, int(state, 0).unwrap_or(0) >= 4)
    }

    /// From a full-size state image whatever its version (the plugin's image, which the editor writes the options
    /// into; an older state is completed with the defaults there).
    pub fn from_image(state: &[u8]) -> Options {
        Options::read(state, true)
    }

    fn read(state: &[u8], with_options: bool) -> Options {
        let mut o = Options { hq: int(state, 292).unwrap_or(0), ..Options::default() };
        if with_options {
            o.lin_size = int(state, 309).unwrap_or(2);
            o.lin_fade = int(state, 313).unwrap_or(0);
            o.flags = state.get(317).copied().unwrap_or(0) & 0x03;
            o.mask = state.get(MASK_BYTE).copied().unwrap_or(0x7f) & 0x7f;
        }
        o
    }

    pub fn mode(&self) -> Mode {
        if self.flags & 0x01 != 0 { Mode::Lin } else { self.band_mode() }
    }

    /// The processing of the rows below LIN (which also decides LIN's coefficients).
    fn band_mode(&self) -> Mode {
        let on = self.hq & 1 != 0;
        match (on, (self.hq >> 1) & 0xff) {
            (true, 0) if self.flags & 0x02 != 0 => Mode::HqEnhanced,
            (true, 0) => Mode::HqLegacy,
            (true, 1) => Mode::HqPlus,
            _ => Mode::Plain,
        }
    }

    /// The bands' coefficients at the project's `rate`: the ordinary ones at twice it with HQ on in mode 0, the matched
    /// design's in HQ+, the ordinary ones at it otherwise.
    pub fn design(&self, rate: f64) -> Design {
        match self.band_mode() {
            Mode::HqLegacy | Mode::HqEnhanced => Design::Ordinary(2.0 * rate),
            Mode::HqPlus => Design::Matched(rate),
            _ => Design::Ordinary(rate),
        }
    }

    /// LIN's transform length.
    pub fn lin_n(&self, rate: f64) -> usize {
        lin::transform_len(self.lin_size, rate)
    }

    /// The delay the processing reports, in frames.
    pub fn latency(&self, rate: f64) -> usize {
        match self.mode() {
            Mode::Lin => lin::delay(self.lin_n(rate)),
            Mode::HqEnhanced => 44,
            _ => 0,
        }
    }
}

/// One section's state, in doubles.
#[derive(Clone, Copy, Debug, Default)]
struct Section {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Section {
    #[inline]
    fn run(&mut self, c: &Coefs, x: f32) -> f32 {
        let x = f64::from(x);
        let y = x * c.b0 + (((self.x1 * c.b1 - self.y2 * c.a2) + self.x2 * c.b2) - self.y1 * c.a1);
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y as f32
    }

    fn tidy(&mut self) {
        for v in [&mut self.x1, &mut self.x2, &mut self.y1, &mut self.y2] {
            if v.abs() < TINY {
                *v = 0.0;
            }
        }
    }
}

/// A band: its stored values, coefficients, the sections' state per channel and the countdown of a band at 0 dB.
#[derive(Clone, Copy, Debug, Default)]
struct Band {
    stored: Stored,
    coefs: BandCoefs,
    state: [[Section; 4]; 2],
    count: i64,
    off: bool,
}

impl Band {
    fn clear(&mut self) {
        self.state = Default::default();
    }

    /// A shelf or peaking band at a level of exactly 0.
    fn at_zero(&self) -> bool {
        self.stored.has_gain() && self.stored.level == 0
    }
}

pub struct Equalizer {
    /// The project's rate.
    rate: f64,
    params: [i32; PARAMS],
    opts: Options,
    design: Design,
    bands: [Band; BANDS],
    gain: f32,
    gate: i64,
    closed: bool,
    up: [HalfBand; 2],
    down: [HalfBand; 2],
    dec: [Decimator; 2],
    taps: [f32; TAPS],
    lin: Option<Lin>,
    /// LIN's "settings changed" mark.
    changed: bool,
    hi: [Vec<f32>; 2],
}

/// `single(exp(double(single(main) * 0.01 * dB)))`, the product in singles.
fn main_gain(main: i32) -> f32 {
    (f64::from(main as f32 * 0.01 * design::db_unit())).exp() as f32
}

impl Equalizer {
    /// A new instance at the project's `rate` with these parameters (clamped to their ranges) and options.
    pub fn new(rate: f64, params: &[i32; PARAMS], opts: Options) -> Equalizer {
        let params: [i32; PARAMS] = std::array::from_fn(|i| {
            let (lo, hi) = range(i);
            params[i].clamp(lo, hi)
        });
        let mut eq = Equalizer {
            rate,
            params,
            opts,
            design: opts.design(rate),
            bands: [Band::default(); BANDS],
            gain: main_gain(params[MAIN]),
            gate: 0,
            closed: false,
            up: Default::default(),
            down: Default::default(),
            dec: Default::default(),
            taps: hq::decimator_taps(),
            lin: None,
            changed: true,
            hi: [Vec::new(), Vec::new()],
        };
        for k in 0..BANDS {
            eq.set_band(k);
        }
        eq
    }

    /// From a saved state (fields 1..36 = the parameters, the options by the state's version).
    pub fn from_state(state: &[u8], rate: f64) -> Option<Equalizer> {
        let mut params = [0; PARAMS];
        for (i, p) in params.iter_mut().enumerate() {
            *p = int(state, 4 * (i + 1))?;
        }
        Some(Equalizer::new(rate, &params, Options::from_state(state)))
    }

    pub fn params(&self) -> &[i32; PARAMS] {
        &self.params
    }

    pub fn options(&self) -> Options {
        self.opts
    }

    pub fn mode(&self) -> Mode {
        self.opts.mode()
    }

    /// The delay the processing reports, in frames.
    pub fn latency(&self) -> usize {
        self.opts.latency(self.rate)
    }

    /// The rate the bands run at.
    fn band_rate(&self) -> f64 {
        self.design.rate()
    }

    /// Band `k`'s settings (re)set: its coefficients anew and its countdown from the start.
    fn set_band(&mut self, k: usize) {
        let p = &self.params;
        let stored = Stored { level: p[k], freq: p[BANDS + k], width: p[2 * BANDS + k], kind: p[3 * BANDS + k], order: p[4 * BANDS + k] };
        let count = (self.band_rate() as i64) / 5;
        let b = &mut self.bands[k];
        b.stored = stored;
        b.coefs = design::band(&stored, self.design);
        b.count = count;
        b.off = false;
    }

    /// An automation value for parameter `i`: from the next block on, without smoothing. A value for a band's type or
    /// order also clears its sections; any value for a band starts its countdown anew.
    pub fn set_param(&mut self, i: usize, v: i32) {
        if i >= PARAMS {
            return;
        }
        let (lo, hi) = range(i);
        self.params[i] = v.clamp(lo, hi);
        if i == MAIN {
            self.gain = main_gain(self.params[MAIN]);
        } else {
            let k = i % BANDS;
            self.set_band(k);
            if i / BANDS >= 3 {
                self.bands[k].clear();
            }
        }
        self.changed = true;
    }

    /// New options (an editor change, or a state loaded into a running instance). A new way of designing the bands
    /// recomputes them all and starts their countdowns anew; LIN switched on starts from silence.
    pub fn set_options(&mut self, o: Options) {
        if o == self.opts {
            return;
        }
        let was_lin = self.opts.mode() == Mode::Lin;
        self.opts = o;
        let design = o.design(self.rate);
        if design != self.design {
            self.design = design;
            for k in 0..BANDS {
                self.set_band(k);
            }
        }
        if o.mode() != Mode::Lin || !was_lin {
            self.lin = None;
        }
        self.changed = true;
    }

    /// Back to a new instance's state (the settings stay).
    pub fn reset(&mut self) {
        for k in 0..BANDS {
            self.set_band(k);
            self.bands[k].clear();
        }
        self.up = Default::default();
        self.down = Default::default();
        self.dec = Default::default();
        self.gate = 0;
        self.closed = false;
        self.lin = None;
        self.changed = true;
    }

    /// The buffers of silence FL runs before a render: four of 2048 frames, five when LIN's `N` is 8192 or more, so
    /// that LIN's first fade is over when the song starts and its blocks fall where FL's do. In the other modes
    /// silence does nothing (the gate closes on the first buffer).
    pub fn preroll(&mut self) {
        let buffers = if self.mode() == Mode::Lin && self.opts.lin_n(self.rate) >= 8192 { 5 } else { 4 };
        let (mut l, mut r) = (vec![0f32; PREROLL_BLOCK], vec![0f32; PREROLL_BLOCK]);
        for _ in 0..buffers {
            l.fill(0.0);
            r.fill(0.0);
            self.process(&mut l, &mut r);
        }
    }

    /// The table for LIN from the bands as they stand.
    fn lin_table(&self, n: usize) -> Vec<f32> {
        let bands: Vec<TableBand> = self
            .bands
            .iter()
            .enumerate()
            .map(|(k, b)| TableBand {
                freq: design::freq_hz(b.stored.freq.clamp(0, 65536)),
                narrow: b.stored.width < 8192 && matches!(b.stored.kind, BAND_PASS | BAND_STOP | PEAKING),
                sections: (self.opts.mask & (1 << k) != 0 && b.stored.kind != 0).then_some(b.coefs),
            })
            .collect();
        lin::table(n, self.rate, self.band_rate(), &bands)
    }

    fn runs(&self, k: usize) -> bool {
        let b = &self.bands[k];
        self.opts.mask & (1 << k) != 0 && b.stored.kind != 0 && !(b.at_zero() && b.off)
    }

    /// One block of both channels, in place.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        if n == 0 {
            return;
        }
        let (l, r) = (&mut l[..n], &mut r[..n]);
        let mode = self.mode();
        if mode == Mode::Lin {
            let len = self.opts.lin_n(self.rate);
            if self.lin.as_ref().is_none_or(|e| e.n() != len) {
                self.lin = Some(Lin::new(len, self.rate));
                self.changed = true;
            }
            if self.changed && !self.lin.as_ref().is_some_and(Lin::fading) {
                self.changed = false;
                let t = self.lin_table(len);
                let time = lin::fade_time(self.opts.lin_fade);
                if let Some(e) = self.lin.as_mut() {
                    e.hand_over(&t, time);
                }
            }
        }

        // The silence gate (in LIN refilled on every block).
        let silent = f32::from_bits(SILENT);
        let open = (self.rate as i64) / 5;
        if mode == Mode::Lin || l.iter().chain(r.iter()).any(|x| x.abs() > silent) {
            self.gate = open;
            self.closed = false;
        } else {
            self.gate = self.gate.saturating_sub(n as i64);
            if self.gate <= -(n as i64) {
                if !self.closed {
                    self.closed = true;
                    self.up = Default::default();
                    self.down = Default::default();
                    for k in 0..BANDS {
                        if self.runs(k) && self.bands[k].at_zero() {
                            self.bands[k].clear();
                        }
                    }
                }
                return;
            }
        }

        if self.gain != 1.0 {
            for x in l.iter_mut().chain(r.iter_mut()) {
                *x *= self.gain;
            }
        }

        match mode {
            Mode::Lin => {
                if let Some(e) = self.lin.as_mut() {
                    e.process(l, r);
                }
            }
            Mode::Plain | Mode::HqPlus => {
                self.run_bands(0, l);
                self.run_bands(1, r);
                self.band_ends(n);
            }
            Mode::HqLegacy | Mode::HqEnhanced => {
                for (ch, x) in [l, r].into_iter().enumerate() {
                    let mut hi = std::mem::take(&mut self.hi[ch]);
                    hi.resize(2 * n, 0.0);
                    if mode == Mode::HqLegacy {
                        for (i, &v) in x.iter().enumerate() {
                            [hi[2 * i], hi[2 * i + 1]] = self.up[ch].up(v);
                        }
                    } else {
                        for (i, &v) in x.iter().enumerate() {
                            hi[2 * i] = v + v;
                            hi[2 * i + 1] = 0.0;
                        }
                    }
                    self.run_bands(ch, &mut hi);
                    if mode == Mode::HqLegacy {
                        for (i, y) in x.iter_mut().enumerate() {
                            *y = self.down[ch].down(hi[2 * i], hi[2 * i + 1]);
                        }
                        self.up[ch].tidy();
                        self.down[ch].tidy();
                    } else {
                        for (i, y) in x.iter_mut().enumerate() {
                            *y = self.dec[ch].down(&self.taps, hi[2 * i], hi[2 * i + 1]);
                        }
                    }
                    self.hi[ch] = hi;
                }
                self.band_ends(2 * n);
            }
        }
    }

    /// The enabled bands in order on channel `ch`.
    fn run_bands(&mut self, ch: usize, x: &mut [f32]) {
        for k in 0..BANDS {
            if !self.runs(k) {
                continue;
            }
            let b = &mut self.bands[k];
            for (c, s) in b.coefs.as_slice().iter().zip(b.state[ch].iter_mut()) {
                for v in x.iter_mut() {
                    *v = s.run(c, *v);
                }
            }
        }
    }

    /// Block end for the bands that ran `samples` samples: the clean-up, and the countdown of those at 0 dB.
    fn band_ends(&mut self, samples: usize) {
        for k in 0..BANDS {
            if !self.runs(k) {
                continue;
            }
            let b = &mut self.bands[k];
            for s in b.state.iter_mut().flatten() {
                s.tidy();
            }
            if b.at_zero() {
                b.count -= samples as i64;
                if b.count <= 0 {
                    b.off = true;
                    b.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::eq::{HIGH_PASS, HIGH_SHELF, LOW_SHELF};

    /// FL's factory preset "Default" with band 4 at `level`, in the given options.
    fn preset(level: i32) -> [i32; PARAMS] {
        let mut p = [0; PARAMS];
        let freqs = [10923, 18204, 25486, 32768, 40050, 47332, 54613];
        for k in 0..BANDS {
            p[BANDS + k] = freqs[k];
            p[2 * BANDS + k] = if k == 0 || k == 6 { 40124 } else { 25412 };
            p[3 * BANDS + k] = match k {
                0 => LOW_SHELF,
                6 => HIGH_SHELF,
                _ => PEAKING,
            };
        }
        p[3] = level;
        p
    }

    fn opts(hq: i32, flags: u8) -> Options {
        Options { hq, flags, ..Options::default() }
    }

    fn noise(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    }

    fn run(eq: &mut Equalizer, x: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (x.to_vec(), x.iter().map(|v| -0.5 * v).collect::<Vec<_>>());
        for (a, b) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            eq.process(a, b);
        }
        (l, r)
    }

    // spec: plugins/fruity-parametric-eq-2.md "Which processing a state selects"
    #[test]
    fn the_state_selects_the_processing() {
        let mode = |hq, flags| opts(hq, flags).mode();
        assert_eq!(mode(0, 0), Mode::Plain);
        assert_eq!(mode(1, 0), Mode::HqLegacy);
        assert_eq!(mode(1, 0x02), Mode::HqEnhanced);
        assert_eq!(mode(3, 0), Mode::HqPlus);
        assert_eq!(mode(3, 0x02), Mode::HqPlus);
        assert_eq!(mode(1, 0x03), Mode::Lin);
        assert_eq!(mode(0, 0x01), Mode::Lin);
        // None of the modes: HQ on in mode 2 or 3 (and above), mode 1 with HQ off, 0x02 with HQ off.
        for (hq, flags) in [(5, 0), (7, 0), (9, 0x02), (2, 0), (0, 0x02), (2, 0x02)] {
            assert_eq!(mode(hq, flags), Mode::Plain, "{hq} {flags:#x}");
        }
        // LIN's coefficients are what the rows below would give.
        let rate = 44100.0;
        assert_eq!(opts(1, 0x01).design(rate), Design::Ordinary(88200.0));
        assert_eq!(opts(3, 0x01).design(rate), Design::Matched(rate));
        assert_eq!(opts(0, 0x01).design(rate), Design::Ordinary(rate));
        // Reported delays: 3 N / 4 in LIN, 44 in HQ Enhanced, 0 otherwise.
        assert_eq!(opts(1, 0x01).latency(rate), 1536);
        assert_eq!(Options { lin_size: 4, ..opts(0, 0x01) }.latency(rate), 6144);
        assert_eq!(opts(1, 0x02).latency(rate), 44);
        assert_eq!(opts(1, 0x02).latency(48000.0), 44);
        assert_eq!(opts(1, 0).latency(rate), 0);
        assert_eq!(opts(3, 0).latency(rate), 0);
        // Old states: no options before version 4.
        let mut st = vec![0u8; 296];
        st[0] = 3;
        st[292] = 1;
        assert_eq!(Options::from_state(&st), opts(1, 0));
        let mut st = vec![0u8; 354];
        st[0] = 8;
        st[292] = 3;
        st[309] = 1;
        st[313] = 2;
        st[317] = 0x63;
        st[318] = 0x7b;
        assert_eq!(Options::from_state(&st), Options { hq: 3, lin_size: 1, lin_fade: 2, flags: 0x03, mask: 0x7b });
    }

    // spec: plugins/fruity-parametric-eq-2.md "Which processing a state selects", "Checked against FL" (states that
    // select none of the three modes play as plain processing)
    #[test]
    fn states_that_select_no_mode_play_as_plain() {
        let x = noise(6000, 1);
        let mut plain = Equalizer::new(44100.0, &preset(900), opts(0, 0));
        let want = run(&mut plain, &x, 512);
        for o in [opts(5, 0), opts(7, 0), opts(2, 0), opts(0, 0x02), opts(0, 0x42)] {
            let mut eq = Equalizer::new(44100.0, &preset(900), o);
            assert_eq!(eq.latency(), 0);
            assert_eq!(run(&mut eq, &x, 512), want, "{o:?}");
        }
        // And the plain processing changes the sound (band 4 at +9 dB).
        assert_ne!(want.0, x);
    }

    // spec: plugins/fruity-parametric-eq-2.md "HQ Enhanced"
    #[test]
    fn hq_enhanced_without_bands_is_the_input_44_frames_late() {
        let mut p = preset(0);
        for k in 0..BANDS {
            p[3 * BANDS + k] = 0;
        }
        let mut eq = Equalizer::new(44100.0, &p, opts(1, 0x02));
        assert_eq!(eq.latency(), 44);
        let x: Vec<f32> = (0..3000).map(|i| (i as f32 * 0.02).sin() * 0.5).collect();
        let (l, r) = run(&mut eq, &x, 256);
        let g = 2.0 * hq::decimator_taps()[89];
        for i in 44..3000 {
            assert!((l[i] - x[i - 44] * g).abs() < 1e-3, "{i}");
            assert!((r[i] + 0.5 * x[i - 44] * g).abs() < 1e-3, "{i}");
        }
        assert!((g - 0.99984).abs() < 1e-5, "{g}");
    }

    // spec: plugins/fruity-parametric-eq-2.md "LIN"
    #[test]
    fn lin_without_bands_is_the_input_3n_4_late() {
        let mut p = preset(0);
        for k in 0..BANDS {
            p[3 * BANDS + k] = 0;
        }
        for (size, n) in [(0, 512), (2, 2048)] {
            let mut eq = Equalizer::new(44100.0, &p, Options { lin_size: size, ..opts(1, 0x01) });
            eq.preroll();
            let d = 3 * n / 4;
            assert_eq!(eq.latency(), d);
            let x = noise(3 * n, 7);
            let (l, _) = run(&mut eq, &x, 300);
            // The table is flat (gain 1): the kernel is the window's centre value, 1 to within 2e-5.
            for i in d..3 * n {
                assert!((l[i] - x[i - d]).abs() < 2e-5, "{size} {i}: {} {}", l[i], x[i - d]);
            }
        }
    }

    /// LIN plays the bands' magnitude: a sine at a peaking band's centre comes out with its gain (through the table's
    /// 2^(dB / 6)).
    #[test]
    fn lin_follows_the_bands() {
        let mut eq = Equalizer::new(44100.0, &preset(1200), opts(0, 0x01));
        eq.preroll();
        let hz = f64::from(design::freq_hz(32768));
        let x: Vec<f32> = (0..20000).map(|i| (std::f64::consts::TAU * hz * i as f64 / 44100.0).sin() as f32 * 0.1).collect();
        let (l, _) = run(&mut eq, &x, 1000);
        let peak = l[10000..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((peak / 0.1 - 4.0 * 0.999862).abs() < 0.02, "{peak}");
    }

    // spec: plugins/fruity-parametric-eq-2.md "Order of processing", "HQ+"
    #[test]
    fn modes_match_their_designs_at_low_frequencies() {
        // A sine well below the Nyquist frequency: plain, HQ Legacy, HQ+ and HQ Enhanced all give the band's gain.
        let hz = 1000.0;
        let x: Vec<f32> = (0..20000).map(|i| (std::f64::consts::TAU * hz * i as f64 / 44100.0).sin() as f32 * 0.1).collect();
        let mut p = preset(0);
        p[3] = 600;
        p[BANDS + 3] = ((hz / 20.0f64).ln() / 1000f64.ln() * 65536.0).round() as i32;
        for o in [opts(0, 0), opts(1, 0), opts(3, 0), opts(1, 0x02)] {
            let mut eq = Equalizer::new(44100.0, &p, o);
            let (l, _) = run(&mut eq, &x, 512);
            let peak = l[10000..].iter().fold(0f32, |m, v| m.max(v.abs()));
            assert!((20.0 * (peak / 0.1).log10() - 6.0).abs() < 0.05, "{o:?}: {}", 20.0 * (peak / 0.1).log10());
        }
    }

    // spec: plugins/fruity-parametric-eq-2.md "Order of processing" (the main level), "A section"
    #[test]
    fn plain_processing_is_the_main_level_then_the_sections() {
        let mut p = preset(0);
        for k in [0, 1, 3, 4, 5] {
            p[3 * BANDS + k] = 0;
        }
        p[MAIN] = -650;
        p[3 * BANDS + 6] = HIGH_PASS;
        p[4 * BANDS + 6] = -1;
        p[2] = 300;
        let x = noise(1500, 3);
        let mut eq = Equalizer::new(48000.0, &p, opts(0, 0));
        let (l, _) = run(&mut eq, &x, 1500);
        // By hand: the main level in singles, then band 3 (+3 dB) and band 7's two sections, each rounded to single.
        let gain = ((-650.0f32 * 0.01 * design::db_unit()) as f64).exp() as f32;
        let stored = |k: usize| Stored { level: p[k], freq: p[BANDS + k], width: p[2 * BANDS + k], kind: p[3 * BANDS + k], order: p[4 * BANDS + k] };
        let mut secs: Vec<(Coefs, Section)> = Vec::new();
        for k in [2, 6] {
            for c in design::band(&stored(k), Design::Ordinary(48000.0)).as_slice() {
                secs.push((*c, Section::default()));
            }
        }
        assert_eq!(secs.len(), 3);
        for (i, &v) in x.iter().enumerate() {
            let mut y = v * gain;
            for (c, s) in secs.iter_mut() {
                y = s.run(c, y);
            }
            assert_eq!(l[i].to_bits(), y.to_bits(), "{i}");
        }
    }

    // spec: plugins/fruity-parametric-eq-2.md "Which bands run", "State and timing" (silence)
    #[test]
    fn zero_gain_bands_count_down_and_silence_passes() {
        let mut eq = Equalizer::new(44100.0, &preset(0), opts(0, 0));
        let count = 44100 / 5;
        assert!((0..BANDS).all(|k| eq.runs(k)));
        let x = noise(512, 9);
        let mut blocks = 0;
        while (0..BANDS).any(|k| eq.runs(k)) {
            let (mut l, mut r) = (x.clone(), x.clone());
            eq.process(&mut l, &mut r);
            blocks += 1;
        }
        assert_eq!(blocks, (count + 511) / 512);
        // Now nothing runs: the block passes unchanged. An automation value starts band 4's count again.
        let (l, _) = run(&mut eq, &x, 512);
        assert_eq!(l, x);
        eq.set_param(3, 0);
        assert!(eq.runs(3) && !eq.runs(2));
        // Silence: once the gate has closed, blocks pass unchanged without the main level.
        let mut eq = Equalizer::new(44100.0, &preset(600), opts(1, 0));
        eq.set_param(MAIN, 600);
        let quiet = vec![1e-7f32; 512];
        let (l, _) = run(&mut eq, &quiet, 512);
        assert_eq!(l, quiet, "the gate starts closed");
        let (l, _) = run(&mut eq, &x, 512);
        assert_ne!(l, x);
        // 0.2 s of quiet still runs (the tail), then it passes unchanged.
        let tail: Vec<f32> = vec![1e-7f32; 44100 / 5 + 1024];
        let (l, _) = run(&mut eq, &tail, 512);
        assert_ne!(l[..512], tail[..512]);
        assert_eq!(l[l.len() - 512..], tail[tail.len() - 512..]);
    }

    /// Automation of a type or order clears the band's sections; other values keep them.
    #[test]
    fn automation_recomputes_at_once() {
        let mut eq = Equalizer::new(44100.0, &preset(1200), opts(0, 0));
        let x = noise(512, 4);
        run(&mut eq, &x, 512);
        let before = eq.bands[3].state;
        eq.set_param(BANDS + 3, 40000);
        assert_eq!(eq.bands[3].state[0][0].y1, before[0][0].y1);
        assert_ne!(eq.bands[3].coefs, design::band(&Stored { level: 1200, freq: 32768, width: 25412, kind: PEAKING, order: 0 }, Design::Ordinary(44100.0)));
        eq.set_param(4 * BANDS + 3, 2);
        assert_eq!(eq.bands[3].state[0][0].y1, 0.0);
        assert_eq!(eq.bands[3].coefs.n, 3);
        // Out-of-range values are held to the range.
        eq.set_param(4 * BANDS + 3, 9);
        assert_eq!(eq.params()[4 * BANDS + 3], 3);
    }
}
