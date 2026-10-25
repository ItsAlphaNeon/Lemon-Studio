// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The metronome: while its switch is on, a click on every beat of the playing position (the bar's beat: a quarter
//! in x/4, an eighth in 3/8), accented on every `numerator`-th, added to the master track's input (so the master track and the main volume apply to it). Live
//! playback only: an export never clicks. The five sounds are Lemon Studio's own, synthesized here.
// spec: ui/metronome.md

use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU8, Ordering};
use std::sync::Arc;

/// The sound of the clicks (spec §2, §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    HiHat,
    Tick,
    Beep,
    Cowbell,
    Pulse,
}

impl Sound {
    /// In the order of the sound menu.
    pub const ALL: [Sound; 5] = [Sound::HiHat, Sound::Tick, Sound::Beep, Sound::Cowbell, Sound::Pulse];

    /// The menu row.
    pub fn label(self) -> &'static str {
        match self {
            Sound::HiHat => "Hi-hat",
            Sound::Tick => "Tick",
            Sound::Beep => "Beep",
            Sound::Cowbell => "Cowbell",
            Sound::Pulse => "Pulse",
        }
    }

    /// The name kept in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Sound::HiHat => "hihat",
            Sound::Tick => "tick",
            Sound::Beep => "beep",
            Sound::Cowbell => "cowbell",
            Sound::Pulse => "pulse",
        }
    }

    pub fn from_key(k: &str) -> Option<Sound> {
        Sound::ALL.into_iter().find(|s| s.key() == k)
    }
}

/// The volume rows of the sound menu, in dB (spec §2, Lemon Studio's own).
pub const VOLUMES: [i8; 7] = [-12, -6, 0, 3, 6, 9, 12];
/// The volumes the settings file may hold (spec §3).
pub const VOLUME_MIN: i8 = -24;
pub const VOLUME_MAX: i8 = 12;

/// A volume's menu row: "-6 dB", "0 dB", "+3 dB".
pub fn volume_label(db: i8) -> String {
    if db > 0 { format!("+{db} dB") } else { format!("{db} dB") }
}

/// The switch, the sound and the volume, shared by the interface and every playing engine: a change is heard from the
/// next beat.
#[derive(Debug)]
pub struct Switch {
    on: AtomicBool,
    sound: AtomicU8,
    volume: AtomicI8,
}

impl Default for Switch {
    /// Off, Tick, 0 dB (spec §3).
    fn default() -> Switch {
        Switch { on: AtomicBool::new(false), sound: AtomicU8::new(1), volume: AtomicI8::new(0) }
    }
}

impl Switch {
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    pub fn sound(&self) -> Sound {
        Sound::ALL.get(self.sound.load(Ordering::Relaxed) as usize).copied().unwrap_or(Sound::Tick)
    }

    pub fn set_sound(&self, s: Sound) {
        let i = Sound::ALL.iter().position(|&x| x == s).unwrap_or(1);
        self.sound.store(i as u8, Ordering::Relaxed);
    }

    /// The volume in whole dB, 0 the level of spec §5.
    pub fn volume(&self) -> i8 {
        self.volume.load(Ordering::Relaxed)
    }

    pub fn set_volume(&self, db: i8) {
        self.volume.store(db.clamp(VOLUME_MIN, VOLUME_MAX), Ordering::Relaxed);
    }
}

/// Peak of a plain click and of an accented one, before the master track (spec §5: -3.5 dB, the accent 3.5 dB louder).
const PLAIN: f32 = 0.67;
const ACCENT: f32 = 1.0;

/// One click, mono, at `rate`: the accented click is the plain one two semitones higher (spec §5).
pub fn render(sound: Sound, accent: bool, rate: u32) -> Vec<f32> {
    let p = if accent { 2f64.powf(2.0 / 12.0) } else { 1.0 };
    let sr = f64::from(rate.max(1));
    let secs = match sound {
        Sound::HiHat => 0.100,
        Sound::Tick => 0.015,
        Sound::Beep => 0.060,
        Sound::Cowbell => 0.200,
        Sound::Pulse => 0.050,
    } / p;
    let n = (secs * sr).round().max(1.0) as usize;
    let decay = |t: f64, tau: f64| (-t * p / tau).exp();
    let mut out: Vec<f64> = Vec::with_capacity(n);
    match sound {
        Sound::HiHat => {
            // White noise from a fixed-seed generator through a first-order high-pass at 7 kHz.
            let mut seed: u32 = 0x9e37_79b9;
            let k = (-TAU * 7000.0 * p / sr).exp();
            let (mut last_in, mut last_out) = (0.0, 0.0);
            for i in 0..n {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let x = f64::from(seed) / f64::from(u32::MAX) * 2.0 - 1.0;
                let y = k * (last_out + x - last_in);
                (last_in, last_out) = (x, y);
                out.push(y * decay(i as f64 / sr, 0.018));
            }
        }
        Sound::Tick => {
            for i in 0..n {
                let t = i as f64 / sr;
                let w = TAU * 1250.0 * p * t;
                out.push((w.sin() + (2.0 * w).sin() / 3.0) * decay(t, 0.0025));
            }
        }
        Sound::Beep => {
            let fade = 0.002 / p * sr;
            for i in 0..n {
                let t = i as f64 / sr;
                let edge = (i as f64 / fade).min((n - i) as f64 / fade).min(1.0);
                out.push((TAU * 1000.0 * p * t).sin() * edge);
            }
        }
        Sound::Cowbell => {
            let tri = |ph: f64| 4.0 * (ph - (ph + 0.5).floor()).abs() - 1.0;
            for i in 0..n {
                let t = i as f64 / sr;
                out.push((tri(540.0 * p * t) + tri(800.0 * p * t)) * decay(t, 0.050));
            }
        }
        Sound::Pulse => {
            // The frequency falls exponentially from 160 to 50 Hz over the click; the phase is its running sum.
            let (f0, f1) = (160.0 * p, 50.0 * p);
            let mut phase = 0.0f64;
            for i in 0..n {
                let t = i as f64 / sr;
                let f = f0 * (f1 / f0).powf(i as f64 / n as f64);
                out.push(phase.sin() * decay(t, 0.012));
                phase += TAU * f / sr;
            }
        }
    }
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-9);
    let level = if accent { ACCENT } else { PLAIN };
    out.iter().map(|v| (v / peak) as f32 * level).collect()
}

/// `render` at a volume of `db` (spec §5 "Volume"): every sample times `10^(db / 20)`; above 0 dB then soft-limited,
/// so that a louder click keeps more of its decay near full scale instead of passing it.
pub fn render_at(sound: Sound, accent: bool, rate: u32, db: i8) -> Vec<f32> {
    let click = render(sound, accent, rate);
    if db == 0 {
        return click;
    }
    let g = 10f32.powf(f32::from(db) / 20.0);
    click.into_iter().map(|v| if db > 0 { limit(v * g) } else { v * g }).collect()
}

/// Linear up to 0.8, then bending towards 1.0 (spec §5 "Volume").
fn limit(v: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = v.abs();
    if a <= KNEE {
        v
    } else {
        v.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
    }
}

/// One engine's metronome: the shared switch, the clicks rendered at the engine's rate, and the click sounding.
pub struct Metronome {
    switch: Arc<Switch>,
    rate: u32,
    /// The sound and volume the clicks were rendered for: plain, accented.
    clicks: Option<((Sound, i8), [Vec<f32>; 2])>,
    /// The click sounding: accented, next sample.
    playing: Option<(bool, usize)>,
}

impl Metronome {
    pub fn new(switch: Arc<Switch>, rate: u32) -> Metronome {
        Metronome { switch, rate, clicks: None, playing: None }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn switch(&self) -> &Arc<Switch> {
        &self.switch
    }

    /// The tick at position `tick` starts: with `start` (the song is not over) and the switch on, a beat (every `beat`
    /// ticks: `Project::beat_ticks`) starts a click, accented on every `numerator`-th beat counted from position 0.
    /// Then whatever click is sounding is added to `out` (interleaved stereo, the tick's frames); a new click cuts the
    /// one before.
    pub fn tick(&mut self, tick: u64, beat: u32, numerator: u16, start: bool, out: &mut [f32]) {
        let on = start && self.switch.on();
        self.click(tick, beat, numerator, on, out);
    }

    /// A tick of a recording count-in (spec: ui/recording.md §3), `tick` counted from the count-in's start: clicks on
    /// every beat, accented on each bar's first, with the switch on or off.
    pub fn count_in(&mut self, tick: u64, beat: u32, numerator: u16, out: &mut [f32]) {
        self.click(tick, beat, numerator, true, out);
    }

    fn click(&mut self, tick: u64, beat: u32, numerator: u16, on: bool, out: &mut [f32]) {
        let ppq = u64::from(beat.max(1));
        if on && tick % ppq == 0 {
            let (sound, db) = (self.switch.sound(), self.switch.volume());
            if self.clicks.as_ref().is_none_or(|(k, _)| *k != (sound, db)) {
                self.clicks = Some(((sound, db), [render_at(sound, false, self.rate, db), render_at(sound, true, self.rate, db)]));
            }
            let accent = (tick / ppq) % u64::from(numerator.max(1)) == 0;
            self.playing = Some((accent, 0));
        }
        let (Some((accent, pos)), Some((_, clicks))) = (self.playing.as_mut(), self.clicks.as_ref()) else { return };
        if add(&clicks[usize::from(*accent)], pos, out) {
            self.playing = None;
        }
    }
}

/// Adds `buf` from `pos` to `out` (interleaved stereo, the same on both sides) and moves `pos` on. Returns whether the
/// click is over.
fn add(buf: &[f32], pos: &mut usize, out: &mut [f32]) -> bool {
    for f in out.chunks_exact_mut(2) {
        let Some(&v) = buf.get(*pos) else { break };
        f[0] += v;
        f[1] += v;
        *pos += 1;
    }
    *pos >= buf.len()
}

/// Clicks started on demand rather than on the beat: the Tempo tapper's taps (spec: ui/tempo-tapper.md §8). Always
/// Tick, the metronome's default sound, whatever the switch is set to, at the metronome's volume; a new click cuts the
/// one before.
pub struct TapClicks {
    rate: u32,
    /// The volume, then plain, accented: rendered at the first click and when the volume changes.
    clicks: Option<(i8, [Vec<f32>; 2])>,
    /// The click sounding: accented, next sample.
    playing: Option<(bool, usize)>,
}

impl TapClicks {
    pub fn new(rate: u32) -> TapClicks {
        TapClicks { rate, clicks: None, playing: None }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// A click at `db` from the next sample `mix` adds.
    pub fn start(&mut self, accent: bool, db: i8) {
        if self.clicks.as_ref().is_none_or(|(v, _)| *v != db) {
            self.clicks = Some((db, [render_at(Sound::Tick, false, self.rate, db), render_at(Sound::Tick, true, self.rate, db)]));
        }
        self.playing = Some((accent, 0));
    }

    pub fn sounding(&self) -> bool {
        self.playing.is_some()
    }

    /// Adds the click sounding to `out` (interleaved stereo).
    pub fn mix(&mut self, out: &mut [f32]) {
        let (Some((accent, pos)), Some((_, clicks))) = (self.playing.as_mut(), self.clicks.as_ref()) else { return };
        if add(&clicks[usize::from(*accent)], pos, out) {
            self.playing = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (frame, peak of the next 5 ms) where sound starts after at least 100 silent frames.
    fn onsets(out: &[f32]) -> Vec<(usize, f32)> {
        let frames: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        let mut v = Vec::new();
        let mut last: Option<usize> = None;
        for (i, &s) in frames.iter().enumerate() {
            if s == 0.0 {
                continue;
            }
            if last.is_none_or(|l| i - l > 100) {
                let pk = frames[i..(i + 240).min(frames.len())].iter().fold(0.0f32, |m, x| m.max(x.abs()));
                v.push((i, pk));
            }
            last = Some(i);
        }
        v
    }

    /// Runs `ticks` ticks of `len` frames from position 0.
    fn run(m: &mut Metronome, ticks: u64, len: usize, numerator: u16) -> Vec<f32> {
        let mut all = Vec::new();
        for t in 0..ticks {
            let mut buf = vec![0.0; len * 2];
            m.tick(t, 4, numerator, true, &mut buf);
            all.extend(buf);
        }
        all
    }

    #[test]
    fn every_sound_has_its_level_and_the_accent_is_higher_and_louder() {
        for s in Sound::ALL {
            let (plain, acc) = (render(s, false, 48000), render(s, true, 48000));
            let pk = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!((pk(&plain) - PLAIN).abs() < 1e-4 && (pk(&acc) - ACCENT).abs() < 1e-4, "{s:?}");
            // Two semitones up: 0.891 times as long.
            let ratio = acc.len() as f64 / plain.len() as f64;
            assert!((ratio - 2f64.powf(-2.0 / 12.0)).abs() < 0.01, "{s:?} {ratio}");
            assert_eq!(Sound::from_key(s.key()), Some(s));
        }
    }

    #[test]
    fn clicks_every_beat_with_the_accent_every_numerator_beats() {
        let sw = Arc::new(Switch::default());
        sw.set_on(true);
        sw.set_sound(Sound::Beep);
        // 4 ticks per beat, 1200 frames per tick: a beat every 4800 frames; 3/4 accents every 3rd.
        let out = run(&mut Metronome::new(sw, 48000), 24, 1200, 3);
        let o = onsets(&out);
        // Onsets within a frame of each beat (a sine's first sample is 0).
        assert_eq!(o.iter().map(|x| x.0 / 2 * 2).collect::<Vec<_>>(), (0..6).map(|b| b * 4800).collect::<Vec<_>>());
        let accented: Vec<bool> = o.iter().map(|x| x.1 > (PLAIN + ACCENT) / 2.0).collect();
        assert_eq!(accented, [true, false, false, true, false, false]);
    }

    #[test]
    fn off_or_after_the_end_nothing_starts() {
        let sw = Arc::new(Switch::default());
        let mut m = Metronome::new(sw.clone(), 48000);
        assert!(run(&mut m, 8, 1200, 4).iter().all(|&s| s == 0.0));
        sw.set_on(true);
        let mut buf = vec![0.0; 2400];
        m.tick(0, 4, 4, false, &mut buf);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn a_tap_click_is_the_tick_sound_whatever_the_switch_says() {
        let mut c = TapClicks::new(48000);
        let mut buf = vec![0.0; 2000];
        c.mix(&mut buf);
        assert!(buf.iter().all(|&s| s == 0.0) && !c.sounding(), "nothing before a tap");
        for accent in [false, true] {
            c.start(accent, 0);
            let tick = render(Sound::Tick, accent, 48000);
            let mut out = vec![0.0; tick.len() * 2 + 200];
            // Over two blocks, the second one ending past the click.
            let (a, b) = out.split_at_mut(300);
            c.mix(a);
            assert!(c.sounding());
            c.mix(b);
            assert!(!c.sounding());
            let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
            assert_eq!(&left[..tick.len()], &tick[..], "accent {accent}");
            assert!(out.chunks_exact(2).all(|f| f[0] == f[1]) && left[tick.len()..].iter().all(|&s| s == 0.0));
        }
    }

    /// Spec §5 "Volume": below 0 dB the click is scaled; above it gets louder (more energy) without passing full scale.
    #[test]
    fn the_volume_scales_the_click_and_above_0_db_stays_below_full_scale() {
        let pk = |v: &[f32]| v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let energy = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>();
        for s in Sound::ALL {
            for accent in [false, true] {
                let base = render(s, accent, 48000);
                assert_eq!(render_at(s, accent, 48000, 0), base);
                let quiet = render_at(s, accent, 48000, -6);
                assert!((pk(&quiet) / pk(&base) - 0.501).abs() < 1e-3, "{s:?}");
                let mut last = energy(&base);
                for db in [3, 6, 9, 12] {
                    let loud = render_at(s, accent, 48000, db);
                    assert!(pk(&loud) <= 1.0 && energy(&loud) > last, "{s:?} {accent} {db} dB");
                    last = energy(&loud);
                }
            }
        }
        assert_eq!(VOLUMES.map(volume_label), ["-12 dB", "-6 dB", "0 dB", "+3 dB", "+6 dB", "+9 dB", "+12 dB"]);
        let sw = Switch::default();
        assert_eq!(sw.volume(), 0);
        sw.set_volume(40);
        assert_eq!(sw.volume(), VOLUME_MAX);
        sw.set_volume(-100);
        assert_eq!(sw.volume(), VOLUME_MIN);
    }

    /// The switch's volume reaches the beat clicks from the next click, and a tap click takes its own.
    #[test]
    fn beat_and_tap_clicks_follow_the_volume() {
        let sw = Arc::new(Switch::default());
        sw.set_on(true);
        let mut m = Metronome::new(sw.clone(), 48000);
        let first = onsets(&run(&mut m, 4, 1200, 4))[0].1;
        sw.set_volume(-12);
        let quiet = onsets(&run(&mut m, 4, 1200, 4))[0].1;
        assert!((quiet / first - 0.251).abs() < 0.01, "{first} {quiet}");
        let mut c = TapClicks::new(48000);
        for db in [0, -6] {
            c.start(false, db);
            let mut out = vec![0.0; 2000];
            c.mix(&mut out);
            assert_eq!(out.iter().step_by(2).copied().collect::<Vec<_>>()[..600], render_at(Sound::Tick, false, 48000, db)[..600]);
        }
    }

    #[test]
    fn a_click_longer_than_a_tick_carries_on_into_the_next() {
        let sw = Arc::new(Switch::default());
        sw.set_on(true);
        sw.set_sound(Sound::Cowbell);
        let mut m = Metronome::new(sw, 48000);
        // 200 ms = 9600 frames over ticks of 1000 frames: the click spans ten ticks, the 2nd tick still sounds.
        let mut a = vec![0.0; 2000];
        m.tick(0, 96, 4, true, &mut a);
        let mut b = vec![0.0; 2000];
        m.tick(1, 96, 4, true, &mut b);
        assert!(b.iter().any(|&s| s != 0.0));
    }
}
