// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Colourful waveforms (General ▸ Display ▸ "Colorful waveforms"): how a stretch of a sample waveform is coloured by
//! what it is made of.
//! - Color map: the stretch's spectral centroid on a log-frequency scale, 50 Hz (position 0) to 10 kHz (1), picks a
//!   colour from the Waves gradient: the theme's (`.flstheme` `WaveClr0..5` at `WaveSpc0..5`), else [`STOCK`].
//! - Multiband: the energy-weighted mix of [`LOW`], [`MID`] and [`HIGH`] by the stretch's power below 250 Hz, from
//!   250 Hz to 4 kHz, and above 4 kHz.
//! - Off: no colour; a waveform keeps its plain colour.
//!
//! The app also keeps the setting here, process-wide ([`set`], [`current`]), for the waveforms painted by plugin
//! editors running in its process (Lemon Sampler), as it does the palette (`crate::current`).
//!
// spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)", ui/flstheme.md §3 (WaveClr / WaveSpc)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use crate::Color;

/// The setting's choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Off,
    ColorMap,
    Multiband,
}

impl Mode {
    /// The mode of the setting's choice `i` (0 Off, 1 Color map, 2 Multiband).
    pub fn from_index(i: usize) -> Mode {
        match i {
            1 => Mode::ColorMap,
            2 => Mode::Multiband,
            _ => Mode::Off,
        }
    }
}

/// The stock Waves gradient (position, colour), as measured: `#7f2047` rose at the low end through peach and pale
/// yellow to `#696091` violet-blue at the high end.
pub const STOCK: [(f32, Color); 6] = [
    (0.0, Color::rgb(0x7f2047)),
    (0.15, Color::rgb(0xc97467)),
    (0.3, Color::rgb(0xe08b8b)),
    (0.475, Color::rgb(0xf1f1ca)),
    (0.9, Color::rgb(0x81b5a0)),
    (1.0, Color::rgb(0x696091)),
];

/// Multiband's low colour (below 250 Hz): orange-red.
pub const LOW: Color = Color::rgb(0xb03800);
/// Multiband's mid colour (250 Hz to 4 kHz): olive green.
pub const MID: Color = Color::rgb(0x607010);
/// Multiband's high colour (above 4 kHz): blue.
pub const HIGH: Color = Color::rgb(0x0018d0);

/// The ends of Color map's log-frequency scale (Hz): gradient positions 0 and 1.
pub const SCALE_HZ: (f32, f32) = (50.0, 10_000.0);

/// The gradient position of frequency `hz`: log scale, 50 Hz = 0, 10 kHz = 1, clamped.
pub fn position(hz: f32) -> f32 {
    let (lo, hi) = SCALE_HZ;
    ((hz.max(1e-3) / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0)
}

/// The colour at position `t` of a gradient (stops sorted by position): linear between the stops around it, the end
/// stops' colours beyond them. An empty gradient is the stock one.
pub fn gradient_at(stops: &[(f32, Color)], t: f32) -> Color {
    let stops = if stops.is_empty() { &STOCK[..] } else { stops };
    let (first, last) = (stops[0], stops[stops.len() - 1]);
    if t <= first.0 {
        return first.1;
    }
    for w in stops.windows(2) {
        let ((p0, c0), (p1, c1)) = (w[0], w[1]);
        if t <= p1 {
            return if p1 > p0 { c0.mix(c1, (t - p0) / (p1 - p0)) } else { c1 };
        }
    }
    last.1
}

/// Multiband's colour for band powers `bands` (low, mid, high): the three colours mixed in proportion to the powers.
/// `None` without power.
pub fn multiband(bands: [f32; 3]) -> Option<Color> {
    let total: f32 = bands.iter().map(|b| b.max(0.0)).sum();
    if total.is_nan() || total <= 0.0 {
        return None;
    }
    let mix = |f: fn(Color) -> u8| -> u8 {
        let v: f32 = [LOW, MID, HIGH].iter().zip(bands).map(|(c, b)| f32::from(f(*c)) * b.max(0.0)).sum();
        (v / total).round().clamp(0.0, 255.0) as u8
    };
    Some(Color { r: mix(|c| c.r), g: mix(|c| c.g), b: mix(|c| c.b), a: 255 })
}

/// How waveforms are coloured: the setting's mode and the Waves gradient to use.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Waves {
    pub mode: Mode,
    /// Gradient stops (position 0..1, colour), sorted; empty = [`STOCK`].
    pub stops: Vec<(f32, Color)>,
}

impl Waves {
    /// `mode` with the theme's gradient `theme` (empty = the stock gradient).
    pub fn new(mode: Mode, theme: &[(f32, Color)]) -> Waves {
        Waves { mode, stops: theme.to_vec() }
    }

    /// Whether waveforms are coloured at all.
    pub fn on(&self) -> bool {
        self.mode != Mode::Off
    }

    /// The colour of a stretch with spectral centroid `centroid` (Hz) and band powers `bands`, each `None` for
    /// silence. `None`: no colour (Off, or silence), the waveform's plain colour applies.
    pub fn colour(&self, centroid: Option<f32>, bands: Option<[f32; 3]>) -> Option<Color> {
        match self.mode {
            Mode::Off => None,
            Mode::ColorMap => centroid.map(|f| gradient_at(&self.stops, position(f))),
            Mode::Multiband => bands.and_then(multiband),
        }
    }
}

static CURRENT: RwLock<Option<Arc<Waves>>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// The waveform colouring of this process (set by the app; Off until then).
pub fn current() -> Arc<Waves> {
    CURRENT.read().ok().and_then(|g| g.clone()).unwrap_or_default()
}

/// Sets the waveform colouring of this process. A change counts up [`generation`], so painters that cache a picture
/// (plugin editors) know to paint again.
pub fn set(w: Waves) {
    if *current() == w {
        return;
    }
    if let Ok(mut g) = CURRENT.write() {
        *g = Some(Arc::new(w));
        GENERATION.fetch_add(1, Ordering::AcqRel);
    }
}

/// Counts the changes made by [`set`].
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// The stock gradient: its stops exactly, linear between them, its ends beyond them.
    #[test]
    fn stock_gradient_interpolates_between_stops() {
        for (p, c) in STOCK {
            assert_eq!(gradient_at(&[], p), c);
        }
        // 40 % of the way from 0.15 #c97467 to 0.3 #e08b8b, 30 % from 0.475 #f1f1ca to 0.9 #81b5a0.
        assert_eq!(gradient_at(&STOCK, 0.21), Color::rgb(0xd27d75));
        assert_eq!(gradient_at(&STOCK, 0.6025), Color::rgb(0xcfdfbd));
        assert_eq!(gradient_at(&STOCK, -1.0), Color::rgb(0x7f2047));
        assert_eq!(gradient_at(&STOCK, 2.0), Color::rgb(0x696091));
        // A theme's gradient of its own.
        let theme = [(0.2, Color::rgb(0x000000)), (0.6, Color::rgb(0xffffff))];
        assert_eq!(gradient_at(&theme, 0.0), Color::rgb(0x000000));
        assert_eq!(gradient_at(&theme, 0.3), Color::rgb(0x404040));
    }

    // spec: ui/settings-choices.md "Display: Colorful waveforms (2026-10-16)"
    /// Color map's scale: 50 Hz at 0, 10 kHz at 1, log in between, clamped; Multiband mixes by power; Off and
    /// silence have no colour.
    #[test]
    fn positions_and_band_mixes() {
        assert_eq!(position(50.0), 0.0);
        assert!((position(10_000.0) - 1.0).abs() < 1e-6);
        assert!((position(1000.0) - (20f32).ln() / (200f32).ln()).abs() < 1e-6);
        assert_eq!((position(10.0), position(20_000.0)), (0.0, 1.0));
        assert_eq!(multiband([1.0, 0.0, 0.0]), Some(LOW));
        assert_eq!(multiband([0.0, 0.0, 2.0]), Some(HIGH));
        assert_eq!(multiband([1.0, 1.0, 0.0]), Some(Color::rgb(0x885408)));
        assert_eq!(multiband([0.0; 3]), None);
        let w = Waves::new(Mode::ColorMap, &[]);
        assert_eq!(w.colour(Some(50.0), None), Some(STOCK[0].1));
        assert_eq!(w.colour(None, Some([1.0, 0.0, 0.0])), None, "silence");
        assert_eq!(Waves::new(Mode::Multiband, &[]).colour(Some(50.0), Some([0.0, 1.0, 0.0])), Some(MID));
        assert_eq!(Waves::default().colour(Some(50.0), Some([1.0, 0.0, 0.0])), None, "Off");
        assert_eq!((Mode::from_index(0), Mode::from_index(1), Mode::from_index(2)), (Mode::Off, Mode::ColorMap, Mode::Multiband));
    }

    /// The process-wide setting starts Off; a change counts up the generation, setting the same again does not.
    #[test]
    fn the_process_setting_counts_its_changes() {
        assert!(!current().on());
        let g = generation();
        set(Waves::new(Mode::Multiband, &[]));
        assert_eq!((current().mode, generation()), (Mode::Multiband, g + 1));
        set(Waves::new(Mode::Multiband, &[]));
        assert_eq!(generation(), g + 1);
        set(Waves::default());
        assert_eq!((current().mode, generation()), (Mode::Off, g + 2));
    }
}
