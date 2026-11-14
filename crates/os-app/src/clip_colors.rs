// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The colour of the channel that a new audio or automation clip makes: the Theme page's "Audio and automation clips
//! use note colors", the current theme's `OverrideClips`. The clip shows its channel's colour
//! (`playlist::clip_tint`); a change of the setting recolours nothing.
//! - Off: a random hue, drawn at saturation 0.08 and lightness 0.37.
//! - On: a random hue of the note-colour greens (80°–155°), drawn at saturation 0.20 and lightness 0.35; when the
//!   theme gives `NoteColor0..15`, a random entry of that list instead, stored as given.
//!
//! Drawn, not stored: the measured values are channel buttons as FL's rack draws them, and Lemon Studio's rack draws a
//! stored colour mixed with the name button's face (`NAME_MIX`). The colour stored is the one the Classic
//! rack in its default palette (the stock Dark theme the values were measured in) draws at the measured values. Those
//! land among the pastels FL stores for audio-clip channels (`ui/playlist.md` "Channel-colour rule": one component
//! about 0x68, one about 0x7e), so the clips' title strips and waveforms look as FL's do.
// spec: ui/settings-choices.md "Light mode and clip note colours (2026-10-16)", ui/flstheme.md §3 (OverrideClips)

use os_model::Rgb;

/// The Classic name button's face: the channel colour mixed this far toward the theme's neutral face. The rack draws
/// with it, and new clip channels' colours are stored through it.
pub const NAME_MIX: f32 = 0.35;

/// Saturation and lightness of a new clip channel's button as drawn, with the setting off (any hue).
pub const OFF: (f32, f32) = (0.08, 0.37);
/// The same with the setting on.
pub const ON: (f32, f32) = (0.20, 0.35);
/// The hues of the note-colour greens, with the setting on (degrees).
pub const ON_HUES: (f32, f32) = (80.0, 155.0);

/// The current theme's rule for new clip channels' colours, as `State::clip_colors` holds it for the code that makes
/// those channels (`App::follow_settings` keeps it equal to `App::clip_colors`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipColors {
    /// "Audio and automation clips use note colors": the theme's `OverrideClips` (off when the file has no such key).
    pub note_colors: bool,
    /// The theme's `NoteColor0..15`; empty when it gives none.
    pub list: Vec<Rgb>,
}

/// The seed of every random colour and the piano roll's randomizer: the time, and a fixed one in tests so a test
/// (and the equivalence harness) gets the same colours on every run.
pub(crate) fn clock() -> u64 {
    if cfg!(test) {
        return 0x5eed;
    }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

/// The seed's bits mixed (as `patterns::random_color` does).
pub(crate) fn hash(seed: u64) -> u64 {
    let mut x = seed ^ 0x9e37_79b9_7f4a_7c15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

/// Hue (degrees), saturation and lightness as unrounded 0..255 components.
pub(crate) fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [(r + m) * 255.0, (g + m) * 255.0, (b + m) * 255.0]
}

impl ClipColors {

    /// The colour for random `seed`.
    pub fn pick(&self, seed: u64) -> Rgb {
        let x = hash(seed);
        if self.note_colors && !self.list.is_empty() {
            return self.list[(x % self.list.len() as u64) as usize];
        }
        let (hue, (s, l)) = if self.note_colors {
            let span = ((ON_HUES.1 - ON_HUES.0) * 100.0) as u64;
            (ON_HUES.0 + (x % (span + 1)) as f32 / 100.0, ON)
        } else {
            ((x % 36_000) as f32 / 100.0, OFF)
        };
        stored(hsl(hue, s, l))
    }

    /// The colour of a new clip channel numbered `index`: a fresh seed (the clock; fixed in tests, so their colours
    /// repeat) mixed with the number.
    pub fn new_channel(&self, index: u16) -> Rgb {
        self.pick(clock() ^ u64::from(index).wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }
}

/// The rack's default name button face (the stock theme's `name_button`): stored colours are mixed back from it
/// (`stored`). The interface's test checks it against the theme.
pub(crate) const NAME_BUTTON: [u8; 3] = [0x2e, 0x31, 0x32];

/// The colour to store for a button drawn `drawn`: the inverse of the rack's mix with the default name button face.
pub(crate) fn stored(drawn: [f32; 3]) -> Rgb {
    let face = Rgb::new(NAME_BUTTON[0], NAME_BUTTON[1], NAME_BUTTON[2]);
    let k = NAME_MIX;
    let inv = |d: f32, f: u8| ((d - k * f32::from(f)) / (1.0 - k)).round().clamp(0.0, 255.0) as u8;
    Rgb::new(inv(drawn[0], face.r), inv(drawn[1], face.g), inv(drawn[2], face.b))
}

#[cfg(test)]
mod tests {
    /// The hue distance between two hues (degrees).
    fn apart(a: f32, b: f32) -> f32 {
        let d = (a - b).rem_euclid(360.0);
        d.min(360.0 - d)
    }

    /// A sample file for new audio clips.
    fn wav(name: &str) -> String {
        // One file per call: tests run in parallel, and one writing the file while another loads it fails the load.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let k = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("os-app-clip-colors-{name}-{}-{k}.wav", std::process::id()));
        os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&p).unwrap(), 48000, &[0.25; 9600]).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// A theme file's text with the setting `on`, and `notes` as its NoteColor list (COLORREF values).
    fn theme(on: bool, notes: &[i64]) -> String {
        let mut text = format!("Lightmode=1\nOverrideClips={}\n", u8::from(on));
        for (i, n) in notes.iter().enumerate() {
            text += &format!("NoteColor{i}={n}\n");
        }
        text
    }

}
