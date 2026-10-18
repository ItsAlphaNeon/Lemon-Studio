// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Colourful waveforms (General ▸ Display ▸ "Colorful waveforms"): the setting with the theme's Waves gradient, for
//! Lemon Studio's own views (playlist audio clips; each sample's tones are analysed once and kept next to its peaks,
//! `PlaylistState::tones`), and shared with the plugin editors running in this process (Lemon Sampler's waveform and
//! overview strip), which read it through `os_theme::waves`. A change takes effect at once.
//!
// spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"

use os_theme::waves::{Mode, Waves};
use os_theme::Palette;

/// How waveforms are coloured now: the setting's mode with the theme's Waves gradient (a Classic design's `.flstheme`
/// with `WaveClr` / `WaveSpc`; the stock gradient otherwise).
pub fn current(t: &Palette) -> Waves {
    Waves::new(Mode::from_index(crate::prefs::index(&crate::prefs::COLORFUL_WAVEFORMS)), &t.waves)
}

/// Shares [`current`] with the plugin editors of this process; they paint again when it changes.
pub fn publish(t: &Palette) {
    os_theme::waves::set(current(t));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::{set_choice, COLORFUL_WAVEFORMS};
    use os_dsp::tone::{Tone, Tones};
    use os_theme::waves::{position, HIGH, LOW};
    use os_theme::Color;
    use std::f32::consts::PI;

    /// The middle of half a second of a sine at `hz` (amplitude 0.5, 44.1 kHz), as a drawn column sees it.
    fn sine_tone(hz: f32) -> Tone {
        let rate = 44100;
        let s: Vec<f32> = (0..rate as usize / 2).map(|i| (2.0 * PI * hz * i as f32 / rate as f32).sin() * 0.5).collect();
        Tones::analyse(&s, rate).span(4096.0, 16384.0)
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// Color map: a 100 Hz sine lands near the start of the (stock) Waves gradient, rose / red; an 8 kHz sine near its
    /// end, on the blue side; a 1 kHz sine in between. Multiband: 100 Hz is mostly the low orange-red, 8 kHz mostly the
    /// high blue. Off colours nothing.
    #[test]
    fn sines_take_their_colours() {
        let palette = Palette::default();
        let (low, mid, high) = (sine_tone(100.0), sine_tone(1000.0), sine_tone(8000.0));
        let colour = |w: &Waves, t: &Tone| w.colour(t.centroid(), t.band_powers()).expect("a colour");
        let pos = |t: &Tone| position(t.centroid().unwrap());

        set_choice(&COLORFUL_WAVEFORMS, 1);
        let map = current(&palette);
        assert_eq!(map.mode, Mode::ColorMap);
        assert!(pos(&low) < 0.2, "100 Hz at {}", pos(&low));
        assert!(pos(&high) > 0.9, "8 kHz at {}", pos(&high));
        assert!((0.4..0.7).contains(&pos(&mid)), "1 kHz at {}", pos(&mid));
        let (l, m, h) = (colour(&map, &low), colour(&map, &mid), colour(&map, &high));
        assert!(l.r > l.g + 40 && l.r > l.b + 40, "100 Hz: rose / red, {l:?}");
        assert!(h.b > h.r + 20 && h.b > h.g, "8 kHz: the blue side, {h:?}");
        assert!(m != l && m != h && m.g > l.g && m.g > h.g, "1 kHz: in between (the pale middle), {m:?}");

        set_choice(&COLORFUL_WAVEFORMS, 2);
        let bands = current(&palette);
        assert_eq!(bands.mode, Mode::Multiband);
        let near = |a: Color, b: Color| a.r.abs_diff(b.r).max(a.g.abs_diff(b.g)).max(a.b.abs_diff(b.b));
        let (l, h) = (colour(&bands, &low), colour(&bands, &high));
        assert!(near(l, LOW) < 16, "100 Hz: orange-red, {l:?}");
        assert!(near(h, HIGH) < 16, "8 kHz: blue, {h:?}");

        set_choice(&COLORFUL_WAVEFORMS, 0);
        let off = current(&palette);
        assert!(!off.on());
        assert_eq!(off.colour(low.centroid(), low.band_powers()), None);
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)", ui/flstheme.md §3 (WaveClr / WaveSpc)
    /// A theme's own Waves gradient replaces the stock one.
    #[test]
    fn a_themes_gradient_is_used() {
        let palette = Palette { waves: vec![(0.0, Color::rgb(0x000000)), (1.0, Color::rgb(0x0000ff))], ..Palette::default() };
        set_choice(&COLORFUL_WAVEFORMS, 1);
        let w = current(&palette);
        assert_eq!(w.colour(Some(50.0), None), Some(Color::rgb(0x000000)));
        assert_eq!(w.colour(Some(10_000.0), None), Some(Color::rgb(0x0000ff)));
        set_choice(&COLORFUL_WAVEFORMS, 0);
    }
}
