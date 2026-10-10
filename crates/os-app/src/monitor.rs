//! The toolbar's output monitor: an oscilloscope / spectrum analyzer panel (click switches) and the stereo peak bars.
//!
//! Fed from the audio thread's tap (the last samples that left the output, mono). Everything here runs on the UI
//! thread: `update` is called from `App::tick`, the views only read the columns.
//!
// spec: ui/main-window.md (monitor group), ui/monitor.md

use std::f32::consts::PI;

/// Columns of the scope / analyzer panel (its inner width in logical px).
pub const COLS: usize = 84;
/// FFT length of the analyzer and samples shown by the scope.
const FFT: usize = 2048;
const SCOPE_SAMPLES: usize = 1536;
/// Analyzer range: lowest and highest frequency shown (log scale) and the dB range mapped to the panel height.
const F_LO: f32 = 25.0;
const F_HI: f32 = 20000.0;
const DB_FLOOR: f32 = -90.0;
/// Fraction of its value a column keeps per update (falling bars), and the same for the peak bars.
const FALL: f32 = 0.82;
const PEAK_FALL: f32 = 0.9;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Scope,
    Spectrum,
}

pub struct Monitor {
    pub mode: Mode,
    /// Scope: per column the lowest and highest sample (-1..=1).
    pub wave: [(f32, f32); COLS],
    /// Analyzer: per column the bar height, 0..=1.
    pub bars: [f32; COLS],
    /// Smoothed stereo peaks, linear.
    pub peaks: (f32, f32),
    last_total: u64,
    rate: u32,
    window: Vec<f32>,
    /// FFT bin range (inclusive start, exclusive end) per column, and the tilt (dB) applied to it; rebuilt on a
    /// sample-rate change.
    ranges: Vec<(usize, usize, f32)>,
    re: Vec<f32>,
    im: Vec<f32>,
    cos: Vec<f32>,
    sin: Vec<f32>,
    input: Vec<f32>,
}

impl Default for Monitor {
    fn default() -> Self {
        let window = (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / FFT as f32).cos()).collect();
        let cos = (0..FFT / 2).map(|i| (2.0 * PI * i as f32 / FFT as f32).cos()).collect();
        let sin = (0..FFT / 2).map(|i| -(2.0 * PI * i as f32 / FFT as f32).sin()).collect();
        Monitor {
            mode: Mode::Spectrum,
            wave: [(0.0, 0.0); COLS],
            bars: [0.0; COLS],
            peaks: (0.0, 0.0),
            last_total: 0,
            rate: 0,
            window,
            ranges: Vec::new(),
            re: vec![0.0; FFT],
            im: vec![0.0; FFT],
            cos,
            sin,
            input: vec![0.0; FFT.max(SCOPE_SAMPLES)],
        }
    }
}

impl Monitor {
    pub fn toggle(&mut self) {
        self.mode = if self.mode == Mode::Scope { Mode::Spectrum } else { Mode::Scope };
    }

    /// Something is still moving (a decay or a signal): the app should keep ticking quickly.
    pub fn active(&self) -> bool {
        self.peaks.0 > 1e-4
            || self.peaks.1 > 1e-4
            || self.bars.iter().any(|&b| b > 0.0)
            || self.wave.iter().any(|w| *w != (0.0, 0.0))
    }

    /// Frequency (Hz) at the centre of column `c`.
    pub fn column_freq(c: usize) -> f32 {
        F_LO * (F_HI / F_LO).powf((c as f32 + 0.5) / COLS as f32)
    }

    /// Hint-bar text for the panel under column `c` (the analyzer reads out frequency and level).
    pub fn hint(&self, c: usize) -> String {
        match self.mode {
            Mode::Spectrum => {
                let c = c.min(COLS - 1);
                let f = Self::column_freq(c);
                let db = DB_FLOOR + self.bars[c] * -DB_FLOOR;
                let fs = if f >= 1000.0 { format!("{:.2} kHz", f / 1000.0) } else { format!("{f:.0} Hz") };
                if self.bars[c] <= 0.0 { format!("{fs}  (click: oscilloscope)") } else { format!("{fs}  {db:.0} dB  (click: oscilloscope)") }
            }
            Mode::Scope => "Oscilloscope (click: spectrum analyzer)".into(),
        }
    }

    /// Takes the newest tap samples and the output peaks. Returns true when the picture changed.
    /// `tap` fills the slice with the last samples (older first) and returns the running sample count;
    /// an unchanged count means no audio arrived (stopped, paused): the picture decays to silence.
    pub fn update(&mut self, rate: u32, peaks: (f32, f32), tap: impl FnOnce(&mut [f32]) -> u64) -> bool {
        let before = (self.wave, self.bars, self.peaks);
        if rate != self.rate && rate > 0 {
            self.rate = rate;
            self.build_ranges();
        }
        let n = FFT.max(SCOPE_SAMPLES);
        let total = tap(&mut self.input[..n]);
        let fresh = total != self.last_total;
        self.last_total = total;
        if !fresh {
            self.input[..n].fill(0.0);
        }
        self.peaks = (peaks.0.max(self.peaks.0 * PEAK_FALL), peaks.1.max(self.peaks.1 * PEAK_FALL));
        if self.peaks.0 < 1e-4 {
            self.peaks.0 = 0.0;
        }
        if self.peaks.1 < 1e-4 {
            self.peaks.1 = 0.0;
        }
        match self.mode {
            Mode::Scope => self.scope(),
            Mode::Spectrum => self.spectrum(),
        }
        before != (self.wave, self.bars, self.peaks)
    }

    fn scope(&mut self) {
        let s = &self.input[self.input.len() - SCOPE_SAMPLES..];
        for c in 0..COLS {
            let (a, b) = (c * SCOPE_SAMPLES / COLS, (c + 1) * SCOPE_SAMPLES / COLS);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &v in &s[a..b] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            self.wave[c] = (lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0));
        }
        if self.wave.iter().all(|w| w.0.abs() < 1e-5 && w.1.abs() < 1e-5) {
            self.wave = [(0.0, 0.0); COLS];
        }
    }

    fn build_ranges(&mut self) {
        let bin_hz = self.rate as f32 / FFT as f32;
        let edge = |c: f32| F_LO * (F_HI / F_LO).powf(c / COLS as f32);
        self.ranges = (0..COLS)
            .map(|c| {
                let (f0, f1) = (edge(c as f32), edge(c as f32 + 1.0));
                let a = (f0 / bin_hz).floor() as usize;
                let b = ((f1 / bin_hz).ceil() as usize).max(a + 1).min(FFT / 2);
                // A level tilt of 3 dB per octave around 1 kHz makes music look level, as analyzers usually do.
                (a.min(FFT / 2 - 1), b, 3.0 * (Self::column_freq(c) / 1000.0).log2())
            })
            .collect();
    }

    fn spectrum(&mut self) {
        if self.ranges.is_empty() {
            return;
        }
        let s = &self.input[self.input.len() - FFT..];
        if s.iter().all(|&v| v == 0.0) {
            for b in &mut self.bars {
                *b *= FALL;
                if *b < 0.004 {
                    *b = 0.0;
                }
            }
            return;
        }
        for i in 0..FFT {
            self.re[i] = s[i] * self.window[i];
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im, &self.cos, &self.sin);
        // A full-scale sine reads 0 dB: amplitude = 2 |X| / sum(window) = 2 |X| / (N / 2).
        let norm = 4.0 / FFT as f32;
        for (c, &(a, b, tilt)) in self.ranges.iter().enumerate() {
            let mut m = 0.0f32;
            for k in a..b {
                m = m.max(self.re[k] * self.re[k] + self.im[k] * self.im[k]);
            }
            let db = 20.0 * (m.sqrt() * norm).max(1e-9).log10() + tilt;
            let h = ((db - DB_FLOOR) / -DB_FLOOR).clamp(0.0, 1.0);
            self.bars[c] = h.max(self.bars[c] * FALL);
        }
    }
}

/// In-place radix-2 FFT (`re`, `im` length a power of two); `cos` / `sin` are the twiddles for that length.
fn fft(re: &mut [f32], im: &mut [f32], cos: &[f32], sin: &[f32]) {
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
        let step = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = (cos[k * step], sin[k * step]);
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
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
        let changed = m.update(48000, (0.5, 0.5), |b| {
            sine(1000.0, 48000, 0.5, b);
            1
        });
        assert!(changed);
        let top = (0..COLS).max_by(|&a, &b| m.bars[a].total_cmp(&m.bars[b])).unwrap();
        let f = Monitor::column_freq(top);
        assert!((f / 1000.0).log2().abs() < 0.2, "peak column is at {f} Hz");
        // 0.5 amplitude = -6 dB, 1 kHz tilt 0: bar at (90 - 6) / 90.
        assert!((m.bars[top] - 84.0 / 90.0).abs() < 0.06, "height {}", m.bars[top]);
    }

    #[test]
    fn silence_decays_to_nothing() {
        let mut m = Monitor::default();
        m.update(48000, (0.5, 0.5), |b| {
            sine(440.0, 48000, 0.5, b);
            1
        });
        assert!(m.active());
        for _ in 0..120 {
            m.update(48000, (0.0, 0.0), |_| 1); // same sample count: no new audio
        }
        assert!(!m.active(), "decayed to rest");
    }

    #[test]
    fn scope_follows_the_wave() {
        let mut m = Monitor::default();
        m.mode = Mode::Scope;
        m.update(48000, (0.8, 0.8), |b| {
            sine(100.0, 48000, 0.8, b);
            1
        });
        let hi = m.wave.iter().map(|w| w.1).fold(0.0, f32::max);
        let lo = m.wave.iter().map(|w| w.0).fold(0.0, f32::min);
        assert!(hi > 0.7 && lo < -0.7);
    }
}
