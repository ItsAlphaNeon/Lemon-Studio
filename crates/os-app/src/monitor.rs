// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The toolbar's output monitor: an oscilloscope / spectrum analyzer panel (click switches) and the stereo peak meter.
//!
//! Fed from the audio thread's tap (the last samples that left the output, mono) and the per-channel peaks since the
//! last read. Everything here runs on the UI thread: `update` is called from `App::tick` as often as it likes, but the
//! picture only advances at the configured refresh rate. All look-and-feel numbers live in `Settings` (one table,
//! `PARAMS`) so the F12 debug panel can tune them against FL.
//!
// spec: ui/main-window.md (monitor group), ui/monitor.md

use std::f32::consts::PI;
use std::time::{Duration, Instant};

/// Columns of the scope / analyzer panel (its inner width in logical px).
pub const COLS: usize = 84;
/// Largest analyzer FFT (the tap keeps at least this many samples).
pub const MAX_FFT: usize = 1 << 14;

/// One tunable: label, default, min, max, decimals shown, unit.
pub struct Param {
    pub name: &'static str,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub decimals: usize,
    pub unit: &'static str,
}

const fn p(name: &'static str, default: f32, min: f32, max: f32, decimals: usize, unit: &'static str) -> Param {
    Param { name, default, min, max, decimals, unit }
}

/// Indices into `Settings::v`.
pub mod k {
    pub const REFRESH: usize = 0;
    pub const FFT_BITS: usize = 1;
    pub const F_LO: usize = 2;
    pub const F_HI: usize = 3;
    pub const DB_FLOOR: usize = 4;
    pub const DB_TOP: usize = 5;
    pub const TILT: usize = 6;
    pub const ATTACK: usize = 7;
    pub const FALL: usize = 8;
    pub const SMOOTH: usize = 9;
    pub const PEAK_MODE: usize = 10;
    pub const SCOPE_MS: usize = 11;
    pub const SCOPE_GAIN: usize = 12;
    pub const TRIGGER: usize = 13;
    pub const METER_RANGE: usize = 14;
    pub const METER_FALL: usize = 15;
    pub const METER_HOLD: usize = 16;
    pub const SILENCE_MS: usize = 17;
}

pub const PARAMS: [Param; 18] = [
    p("Refresh rate", 120.0, 5.0, 240.0, 0, "Hz"),
    p("FFT size (2^n)", 14.0, 9.0, 14.0, 0, ""),
    p("Low frequency", 10.0, 5.0, 200.0, 0, "Hz"),
    p("High frequency", 24000.0, 4000.0, 24000.0, 0, "Hz"),
    p("Analyzer floor", -78.0, -140.0, -30.0, 0, "dB"),
    p("Analyzer top", -30.0, -60.0, 12.0, 0, "dB"),
    p("Tilt", 1.4, 0.0, 9.0, 1, "dB/oct"),
    p("Rise (1 = instant)", 1.0, 0.05, 1.0, 2, ""),
    p("Bar fall", 600.0, 5.0, 2000.0, 0, "dB/s"),
    p("Column blur", 1.0, 0.0, 4.0, 0, "cols"),
    p("Bin: max(0) / avg(1)", 0.0, 0.0, 1.0, 0, ""),
    p("Scope window", 25.0, 2.0, 170.0, 0, "ms"),
    p("Scope gain", 2.64, 0.25, 4.0, 2, "x"),
    p("Scope trigger", 1.0, 0.0, 1.0, 0, "on/off"),
    p("Meter range", 96.0, 20.0, 120.0, 0, "dB"),
    p("Meter fall", 241.0, 3.0, 300.0, 0, "dB/s"),
    p("Meter peak hold", 2018.0, 0.0, 3000.0, 0, "ms"),
    p("Silence after", 499.0, 50.0, 1000.0, 0, "ms"),
];

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub v: [f32; PARAMS.len()],
}

impl Default for Settings {
    fn default() -> Self {
        // Through `set`, so defaults sit on the same step grid as loaded values.
        let mut s = Settings { v: [0.0; PARAMS.len()] };
        (0..PARAMS.len()).for_each(|i| s.set(i, PARAMS[i].default));
        s
    }
}

impl Settings {
    pub fn set(&mut self, i: usize, x: f32) {
        let p = &PARAMS[i];
        let step = 10f32.powi(-(p.decimals as i32));
        self.v[i] = ((x.clamp(p.min, p.max) / step).round() * step).clamp(p.min, p.max);
    }
    /// Position of parameter `i` within its range, 0..=1.
    pub fn norm(&self, i: usize) -> f32 {
        (self.v[i] - PARAMS[i].min) / (PARAMS[i].max - PARAMS[i].min)
    }
    pub fn text(&self, i: usize) -> String {
        let p = &PARAMS[i];
        let n = if i == k::FFT_BITS { format!(" = {}", 1u32 << self.v[i] as u32) } else { String::new() };
        format!("{:.*} {}{n}", p.decimals, self.v[i], p.unit)
    }
    /// One `name = value` line per parameter (what the debug panel's Save writes).
    pub fn dump(&self) -> String {
        PARAMS.iter().zip(self.v).map(|(p, v)| format!("{} = {v:.*}\n", p.name, p.decimals)).collect()
    }
    pub fn load(&mut self, text: &str) {
        for line in text.lines() {
            let Some((name, val)) = line.split_once('=') else { continue };
            if let (Some(i), Ok(x)) = (PARAMS.iter().position(|p| p.name == name.trim()), val.trim().parse::<f32>()) {
                self.set(i, x);
            }
        }
    }
    fn fft(&self) -> usize {
        1 << (self.v[k::FFT_BITS] as usize).clamp(9, 14)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Scope,
    Spectrum,
}

pub struct Monitor {
    pub mode: Mode,
    pub cfg: Settings,
    /// The F12 debug panel is open.
    pub debug: bool,
    /// Scope: per column the lowest and highest sample (-1..=1).
    pub wave: [(f32, f32); COLS],
    /// Analyzer: per column the bar level in dB (`floor` or below = nothing).
    pub bars_db: [f32; COLS],
    /// Peak meter: shown level per channel (dB) and the held peak (dB, time it was set).
    pub meter_db: [f32; 2],
    pub hold: [(f32, Option<Instant>); 2],
    last_total: u64,
    /// When new audio last arrived and when the picture last advanced.
    last_audio: Option<Instant>,
    last_step: Option<Instant>,
    rate: u32,
    /// What `ranges` / `window` were built for: (rate, fft, lo, hi, tilt).
    built: (u32, usize, f32, f32, f32),
    window: Vec<f32>,
    /// FFT bin range (inclusive start, exclusive end) per column and the tilt (dB) applied to it.
    ranges: Vec<(usize, usize, f32)>,
    re: Vec<f32>,
    im: Vec<f32>,
    input: Vec<f32>,
}

/// Below this many dB a level counts as silence.
pub const SILENT_DB: f32 = -200.0;

impl Default for Monitor {
    fn default() -> Self {
        Monitor {
            mode: Mode::Spectrum,
            cfg: Settings::default(),
            debug: false,
            wave: [(0.0, 0.0); COLS],
            bars_db: [SILENT_DB; COLS],
            meter_db: [SILENT_DB; 2],
            hold: [(SILENT_DB, None); 2],
            last_total: 0,
            last_audio: None,
            last_step: None,
            rate: 0,
            built: (0, 0, 0.0, 0.0, 0.0),
            window: Vec::new(),
            ranges: Vec::new(),
            re: Vec::new(),
            im: Vec::new(),
            input: vec![0.0; MAX_FFT],
        }
    }
}

fn db(x: f32) -> f32 {
    if x > 1e-10 { 20.0 * x.log10() } else { SILENT_DB }
}

impl Monitor {
    pub fn with_cfg(cfg: Settings) -> Self {
        Monitor { cfg, ..Default::default() }
    }

    pub fn toggle(&mut self) {
        self.mode = if self.mode == Mode::Scope { Mode::Spectrum } else { Mode::Scope };
        self.last_step = None;
    }

    /// Something is still moving (a decay or a signal): the app should keep ticking quickly.
    pub fn active(&self) -> bool {
        self.meter_db.iter().any(|&d| d > SILENT_DB)
            || self.hold.iter().any(|h| h.1.is_some())
            || self.bars_db.iter().any(|&b| b > SILENT_DB)
            || self.wave.iter().any(|w| *w != (0.0, 0.0))
    }

    /// Analyzer bar height of column `c`, 0..=1 of the panel.
    pub fn bar(&self, c: usize) -> f32 {
        let (lo, top) = (self.cfg.v[k::DB_FLOOR], self.cfg.v[k::DB_TOP]);
        ((self.bars_db[c] - lo) / (top - lo).max(1.0)).clamp(0.0, 1.0)
    }

    /// Meter length of a level in dB, 0..=1.
    pub fn meter_pos(&self, level_db: f32) -> f32 {
        let r = self.cfg.v[k::METER_RANGE];
        (1.0 + level_db / r).clamp(0.0, 1.0)
    }

    /// Frequency (Hz) at the centre of column `c`.
    pub fn column_freq(&self, c: usize) -> f32 {
        self.edge(c as f32 + 0.5)
    }

    fn edge(&self, c: f32) -> f32 {
        let (lo, hi) = (self.cfg.v[k::F_LO], self.cfg.v[k::F_HI].max(self.cfg.v[k::F_LO] * 2.0));
        lo * (hi / lo).powf(c / COLS as f32)
    }

    /// Hint-bar text for the panel under column `c` (the analyzer reads out frequency and level).
    pub fn hint(&self, c: usize) -> String {
        match self.mode {
            Mode::Spectrum => {
                let c = c.min(COLS - 1);
                let f = self.column_freq(c);
                let fs = if f >= 1000.0 { format!("{:.2} kHz", f / 1000.0) } else { format!("{f:.0} Hz") };
                if self.bar(c) <= 0.0 {
                    format!("{fs}  (click: oscilloscope)")
                } else {
                    format!("{fs}  {:.0} dB  (click: oscilloscope)", self.bars_db[c])
                }
            }
            Mode::Scope => "Oscilloscope (click: spectrum analyzer)".into(),
        }
    }

    /// Advances the picture if a refresh period has passed. `peaks` returns (and resets) the per-channel output
    /// peaks since the last call; `tap` fills the slice with the last samples (older first) and returns the running
    /// sample count (unchanged = no new audio). Returns true when the picture changed.
    pub fn update(
        &mut self,
        now: Instant,
        rate: u32,
        peaks: impl FnOnce() -> (f32, f32),
        tap: impl FnOnce(&mut [f32]) -> u64,
    ) -> bool {
        let period = Duration::from_secs_f32(1.0 / self.cfg.v[k::REFRESH].max(1.0));
        let dt = match self.last_step {
            Some(t) if now.duration_since(t) < period => return false,
            // Long gaps (window hidden, first frame) count as one period so decays don't jump.
            Some(t) => now.duration_since(t).min(period * 4).as_secs_f32(),
            None => period.as_secs_f32(),
        };
        self.last_step = Some(now);
        let before = (self.wave, self.bars_db, self.meter_db, self.hold);
        if rate > 0 {
            self.rate = rate;
        }

        let total = tap(&mut self.input);
        if total != self.last_total {
            self.last_total = total;
            self.last_audio = Some(now);
        }
        // No new samples for a short while (a tick between two audio blocks is normal): keep the last picture.
        // Only after `Silence after` with nothing new does the display decay to rest.
        let silence = Duration::from_secs_f32(self.cfg.v[k::SILENCE_MS] / 1000.0);
        let live = self.last_audio.is_some_and(|t| now.duration_since(t) < silence);

        let (pl, pr) = peaks();
        self.meter(if live { [db(pl), db(pr)] } else { [SILENT_DB; 2] }, now, dt);
        match self.mode {
            Mode::Scope => self.scope(live),
            Mode::Spectrum => self.spectrum(live, dt),
        }
        before != (self.wave, self.bars_db, self.meter_db, self.hold)
    }

    fn meter(&mut self, new: [f32; 2], now: Instant, dt: f32) {
        let range = self.cfg.v[k::METER_RANGE];
        let fall = self.cfg.v[k::METER_FALL] * dt;
        let hold = Duration::from_secs_f32(self.cfg.v[k::METER_HOLD] / 1000.0);
        for ch in 0..2 {
            let shown = (self.meter_db[ch] - fall).max(new[ch]);
            self.meter_db[ch] = if shown < -range { SILENT_DB } else { shown };
            let (h, t) = self.hold[ch];
            if hold.is_zero() || self.meter_db[ch] <= SILENT_DB && t.is_some_and(|t| now.duration_since(t) >= hold) {
                self.hold[ch] = (SILENT_DB, None);
            } else if self.meter_db[ch] >= h || t.is_some_and(|t| now.duration_since(t) >= hold) {
                self.hold[ch] = (self.meter_db[ch], (self.meter_db[ch] > SILENT_DB).then_some(now));
            }
        }
    }

    fn scope(&mut self, live: bool) {
        if !live {
            self.wave = [(0.0, 0.0); COLS];
            return;
        }
        let rate = self.rate.max(8000) as f32;
        let n = ((self.cfg.v[k::SCOPE_MS] / 1000.0 * rate) as usize).clamp(COLS, MAX_FFT / 2);
        let gain = self.cfg.v[k::SCOPE_GAIN];
        // Trigger: the newest rising zero crossing that still leaves `n` samples after it, searched in the older
        // half of the tap, so the trace stands still on periodic sounds instead of crawling every refresh.
        let len = self.input.len();
        let mut start = len - n;
        if self.cfg.v[k::TRIGGER] >= 0.5 {
            let s = &self.input;
            let lo = len.saturating_sub(n + n.max(rate as usize / 20));
            if let Some(i) = (lo + 1..=len - n).rev().find(|&i| s[i - 1] < 0.0 && s[i] >= 0.0) {
                start = i;
            }
        }
        let s = &self.input[start..start + n];
        for c in 0..COLS {
            let (a, b) = (c * n / COLS, ((c + 1) * n / COLS).max(c * n / COLS + 1));
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &v in &s[a..b] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            self.wave[c] = ((lo * gain).clamp(-1.0, 1.0), (hi * gain).clamp(-1.0, 1.0));
        }
        if self.wave.iter().all(|w| w.0.abs() < 1e-5 && w.1.abs() < 1e-5) {
            self.wave = [(0.0, 0.0); COLS];
        }
    }

    fn build(&mut self) {
        let fft = self.cfg.fft();
        let key = (self.rate, fft, self.cfg.v[k::F_LO], self.cfg.v[k::F_HI], self.cfg.v[k::TILT]);
        if key == self.built || self.rate == 0 {
            return;
        }
        self.built = key;
        self.window = (0..fft).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / fft as f32).cos()).collect();
        self.re = vec![0.0; fft];
        self.im = vec![0.0; fft];
        let bin_hz = self.rate as f32 / fft as f32;
        let tilt = self.cfg.v[k::TILT];
        self.ranges = (0..COLS)
            .map(|c| {
                let (f0, f1) = (self.edge(c as f32), self.edge(c as f32 + 1.0));
                let a = ((f0 / bin_hz).round() as usize).min(fft / 2 - 1);
                let b = ((f1 / bin_hz).round() as usize).max(a + 1).min(fft / 2);
                // A level tilt per octave around 1 kHz makes music look level, as analyzers usually do.
                (a, b, tilt * (self.column_freq(c) / 1000.0).log2())
            })
            .collect();
    }

    fn spectrum(&mut self, live: bool, dt: f32) {
        self.build();
        let floor = self.cfg.v[k::DB_FLOOR];
        let fall = self.cfg.v[k::FALL] * dt;
        let decay = |b: &mut f32| {
            *b -= fall;
            if *b < floor {
                *b = SILENT_DB;
            }
        };
        if !live || self.ranges.is_empty() {
            self.bars_db.iter_mut().for_each(decay);
            return;
        }
        let fft = self.re.len();
        let s = &self.input[self.input.len() - fft..];
        for i in 0..fft {
            self.re[i] = s[i] * self.window[i];
            self.im[i] = 0.0;
        }
        fft_in_place(&mut self.re, &mut self.im);
        // A full-scale sine reads 0 dB: amplitude = 2 |X| / sum(window) = 2 |X| / (N / 2).
        let norm = 4.0 / fft as f32;
        let avg = self.cfg.v[k::PEAK_MODE] >= 0.5;
        let mut raw = [SILENT_DB; COLS];
        for (c, &(a, b, tilt)) in self.ranges.iter().enumerate() {
            let mut m = 0.0f32;
            for i in a..b {
                let p = self.re[i] * self.re[i] + self.im[i] * self.im[i];
                m = if avg { m + p } else { m.max(p) };
            }
            if avg {
                m /= (b - a) as f32;
            }
            raw[c] = db(m.sqrt() * norm) + tilt;
        }
        let blur = self.cfg.v[k::SMOOTH] as usize;
        let rise = self.cfg.v[k::ATTACK];
        for c in 0..COLS {
            let target = if blur == 0 {
                raw[c]
            } else {
                let (a, b) = (c.saturating_sub(blur), (c + blur + 1).min(COLS));
                // Blur in power, then back to dB.
                db((raw[a..b].iter().map(|d| 10f32.powf(d / 10.0)).sum::<f32>() / (b - a) as f32).sqrt())
            };
            let cur = self.bars_db[c];
            let next = if target > cur {
                if cur <= SILENT_DB { target.max(floor) * rise + floor * (1.0 - rise) } else { cur + (target - cur) * rise }
            } else {
                (cur - fall).max(target)
            };
            self.bars_db[c] = if next < floor { SILENT_DB } else { next };
        }
    }
}

/// In-place radix-2 FFT (`re`, `im` length a power of two).
fn fft_in_place(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr0, wi0) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                (wr, wi) = (wr * wr0 - wi * wi0, wr * wi0 + wi * wr0);
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, rate: u32, amp: f32, out: &mut [f32]) {
        for (i, v) in out.iter_mut().enumerate() {
            *v = amp * (2.0 * PI * hz * i as f32 / rate as f32).sin();
        }
    }

    #[test]
    fn analyzer_peaks_at_the_tone() {
        let mut m = Monitor::default();
        m.cfg.set(k::TILT, 0.0);
        m.cfg.set(k::SMOOTH, 0.0);
        let changed = m.update(Instant::now(), 48000, || (0.5, 0.5), |b| {
            sine(1000.0, 48000, 0.5, b);
            1
        });
        assert!(changed);
        let top = (0..COLS).max_by(|&a, &b| m.bars_db[a].total_cmp(&m.bars_db[b])).unwrap();
        let f = m.column_freq(top);
        assert!((f / 1000.0).log2().abs() < 0.2, "peak column is at {f} Hz");
        // 0.5 amplitude = -6 dB (tilt ~0 at 1 kHz).
        assert!((m.bars_db[top] + 6.0).abs() < 1.5, "level {}", m.bars_db[top]);
    }

    #[test]
    fn no_flicker_between_audio_blocks() {
        let mut m = Monitor::default();
        let t0 = Instant::now();
        m.update(t0, 48000, || (0.5, 0.5), |b| {
            sine(440.0, 48000, 0.5, b);
            1
        });
        let bars = m.bars_db;
        // Next refresh: the audio thread hasn't delivered a new block yet (same count). The picture must hold.
        m.update(t0 + Duration::from_millis(40), 48000, || (0.0, 0.0), |b| {
            sine(440.0, 48000, 0.5, b);
            1
        });
        let top = (0..COLS).max_by(|&a, &b| bars[a].total_cmp(&bars[b])).unwrap();
        assert!((m.bars_db[top] - bars[top]).abs() < 0.5, "held, not blanked");
    }

    #[test]
    fn refresh_rate_limits_updates() {
        let mut m = Monitor::default();
        let t0 = Instant::now();
        assert!(m.update(t0, 48000, || (0.5, 0.5), |b| {
            sine(440.0, 48000, 0.5, b);
            1
        }));
        assert!(!m.update(t0 + Duration::from_millis(5), 48000, || (0.9, 0.9), |_| 2), "too soon");
    }

    #[test]
    fn silence_decays_to_nothing() {
        let mut m = Monitor::default();
        let t0 = Instant::now();
        m.update(t0, 48000, || (0.5, 0.5), |b| {
            sine(440.0, 48000, 0.5, b);
            1
        });
        assert!(m.active());
        for i in 1..200 {
            m.update(t0 + Duration::from_millis(34 * i), 48000, || (0.0, 0.0), |_| 1);
        }
        assert!(!m.active(), "decayed to rest");
    }

    #[test]
    fn meter_falls_at_its_rate() {
        let mut m = Monitor::default();
        m.cfg.set(k::METER_FALL, 30.0);
        m.cfg.set(k::REFRESH, 30.0);
        let t0 = Instant::now();
        m.update(t0, 48000, || (1.0, 0.5), |_| 1);
        assert!(m.meter_db[0].abs() < 0.01 && (m.meter_db[1] + 6.02).abs() < 0.05);
        m.update(t0 + Duration::from_millis(100), 48000, || (0.0, 0.0), |_| 2);
        // 30 dB/s for 0.1 s.
        assert!((m.meter_db[0] + 3.0).abs() < 0.2, "{}", m.meter_db[0]);
    }

    #[test]
    fn scope_follows_the_wave() {
        let mut m = Monitor::default();
        m.mode = Mode::Scope;
        m.update(Instant::now(), 48000, || (0.8, 0.8), |b| {
            sine(100.0, 48000, 0.8, b);
            1
        });
        let hi = m.wave.iter().map(|w| w.1).fold(0.0, f32::max);
        let lo = m.wave.iter().map(|w| w.0).fold(0.0, f32::min);
        assert!(hi > 0.7 && lo < -0.7);
        // Triggered: the trace starts at a rising zero crossing.
        assert!(m.wave[0].0.abs() < 0.1, "{:?}", m.wave[0]);
    }

    #[test]
    fn settings_round_trip() {
        let mut s = Settings::default();
        s.set(k::FALL, 123.4);
        let mut t = Settings::default();
        t.load(&s.dump());
        assert!(t == s);
    }
}
