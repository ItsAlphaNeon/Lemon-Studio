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
//! stored colour mixed with the name button's face (`views::rack::NAME_MIX`). The colour stored is the one the Classic
//! rack in its default palette (the stock Dark theme the values were measured in) draws at the measured values. Those
//! land among the pastels FL stores for audio-clip channels (`ui/playlist.md` "Channel-colour rule": one component
//! about 0x68, one about 0x7e), so the clips' title strips and waveforms look as FL's do.
// spec: ui/settings-choices.md "Light mode and clip note colours (2026-10-16)", ui/flstheme.md §3 (OverrideClips)

use os_model::Rgb;
use os_theme::{FlTheme, Palette};

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

impl ClipColors {
    /// The rule of the applied `.flstheme` `fl`.
    pub fn of(fl: &FlTheme) -> ClipColors {
        ClipColors { note_colors: fl.override_clips, list: fl.note_colors.iter().map(|c| Rgb::new(c.r, c.g, c.b)).collect() }
    }

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

fn clock() -> u64 {
    if cfg!(test) {
        return 0x5eed;
    }
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
}

/// The seed's bits mixed (as `patterns::random_color` does).
fn hash(seed: u64) -> u64 {
    let mut x = seed ^ 0x9e37_79b9_7f4a_7c15;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}

/// Hue (degrees), saturation and lightness as unrounded 0..255 components.
fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
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

/// The colour to store for a button drawn `drawn`: the inverse of the rack's mix with the default name button face.
fn stored(drawn: [f32; 3]) -> Rgb {
    let face = Palette::default().name_button;
    let k = crate::views::rack::NAME_MIX;
    let inv = |d: f32, f: u8| ((d - k * f32::from(f)) / (1.0 - k)).round().clamp(0.0, 255.0) as u8;
    Rgb::new(inv(drawn[0], face.r), inv(drawn[1], face.g), inv(drawn[2], face.b))
}

/// The face the Classic rack draws for stored colour `c` in its default palette.
#[cfg(test)]
pub fn drawn(c: Rgb) -> os_theme::Color {
    crate::views::rgb(c).mix(Palette::default().name_button, crate::views::rack::NAME_MIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use os_model::{ChannelKind, ClipSource};
    use os_theme::Color;

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

    /// The app with the applied theme file's text `fl` (None: none, the stock Dark theme), followed (as on a tick).
    fn app_with(fl: Option<String>) -> App {
        let mut app = App::new(None);
        app.state.fl_text = fl;
        app.tick();
        app
    }

    /// Colours of `n` new audio-clip channels, and of `n` new automation-clip channels.
    fn new_channels(app: &mut App, n: usize) -> (Vec<Rgb>, Vec<Rgb>) {
        let path = wav("new");
        let mut audio = Vec::new();
        let mut auto = Vec::new();
        for k in 0..n {
            crate::playlist::add_audio_clip(&mut app.state, &path, k as u16, 0).expect("audio clip added");
            let ch = app.state.project.channels.last().expect("the clip's channel");
            assert_eq!(ch.kind, ChannelKind::AudioClip);
            audio.push(ch.color.expect("a colour"));
            crate::playlist::create_automation(&mut app.state, 0, "Auto", 0.5);
            let ch = app.state.project.channels.last().expect("the clip's channel");
            assert_eq!(ch.kind, ChannelKind::Automation);
            auto.push(ch.color.expect("a colour"));
        }
        (audio, auto)
    }

    /// "Audio and automation clips use note colors" off (spec: ui/settings-choices.md "Light mode and clip note
    /// colours (2026-10-16)"): new audio and automation clips' channels take muted colours of any hue (drawn at
    /// saturation 0.08, lightness 0.37), and their clips show them. Also without a theme file (the setting reads off).
    #[test]
    fn clip_channels_off_take_muted_colours_of_any_hue() {
        for fl in [None, Some(theme(false, &[255, 65280]))] {
            let mut app = app_with(fl);
            assert!(!app.clip_colors().note_colors);
            let (audio, auto) = new_channels(&mut app, 6);
            for colours in [&audio, &auto] {
                let hues: Vec<f32> = colours.iter().map(|&c| drawn(c).hsl().0).collect();
                for &c in colours.iter() {
                    let (_, s, l) = drawn(c).hsl();
                    assert!((s - OFF.0).abs() < 0.025 && (l - OFF.1).abs() < 0.01, "{c:?} draws at s {s}, l {l}");
                }
                let spread = hues.iter().flat_map(|a| hues.iter().map(move |b| apart(*a, *b))).fold(0.0, f32::max);
                assert!(spread > 90.0, "hues vary: {hues:?}");
            }
            // The clip shows its channel's colour.
            let ch = app.state.project.channels.last().expect("channel").index;
            assert_eq!(crate::playlist::clip_tint(&app.state, ClipSource::Channel(ch)), Some(*auto.last().expect("one")));
        }
    }

    /// The setting on (spec, as above): the hues fall among the note-colour greens, 80°–155°, drawn at saturation
    /// 0.20 and lightness 0.35; audio and automation clips alike.
    #[test]
    fn clip_channels_on_take_note_colour_greens() {
        let mut app = app_with(Some(theme(true, &[])));
        assert!(app.clip_colors().note_colors);
        let (audio, auto) = new_channels(&mut app, 6);
        let mut hues = Vec::new();
        for &c in audio.iter().chain(&auto) {
            let (h, s, l) = drawn(c).hsl();
            assert!((ON_HUES.0 - 2.0..=ON_HUES.1 + 2.0).contains(&h), "{c:?} draws at hue {h}");
            assert!((s - ON.0).abs() < 0.02 && (l - ON.1).abs() < 0.01, "{c:?} draws at s {s}, l {l}");
            hues.push(h);
        }
        let (lo, hi) = hues.iter().fold((360.0f32, 0.0f32), |(a, b), &h| (a.min(h), b.max(h)));
        assert!(hi - lo > 25.0, "hues vary within the greens: {hues:?}");
        // Many seeds: always within the range, and over most of it.
        let rule = app.clip_colors();
        let hues: Vec<f32> = (0..400).map(|k| drawn(rule.pick(k)).hsl().0).collect();
        assert!(hues.iter().all(|h| (78.0..=157.0).contains(h)), "{hues:?}");
        let (lo, hi) = hues.iter().fold((360.0f32, 0.0f32), |(a, b), &h| (a.min(h), b.max(h)));
        assert!(lo < 85.0 && hi > 150.0, "{lo}..{hi}");
    }

    /// The setting on with a theme that gives NoteColor0..15 (spec, as above): a random entry of that list, as given.
    #[test]
    fn clip_channels_on_take_the_themes_note_colours() {
        // COLORREF: red in the low byte (ui/flstheme.md §2).
        let notes = [0x20_40_e0, 0x30_c0_50, 0xa0_60_10, 0x80_80_80];
        let list: Vec<Rgb> = notes.iter().map(|&v| Rgb::new((v & 0xff) as u8, (v >> 8 & 0xff) as u8, (v >> 16) as u8)).collect();
        let mut app = app_with(Some(theme(true, &notes)));
        assert_eq!(app.clip_colors().list, list);
        let (audio, auto) = new_channels(&mut app, 8);
        for c in audio.iter().chain(&auto) {
            assert!(list.contains(c), "{c:?} is a note colour");
        }
        let mut used: Vec<Rgb> = audio.iter().chain(&auto).copied().collect();
        used.sort_by_key(|c| (c.r, c.g, c.b));
        used.dedup();
        assert!(used.len() >= 3, "random entries: {used:?}");
        // With the setting off the list is not used.
        let mut app = app_with(Some(theme(false, &notes)));
        let (audio, _) = new_channels(&mut app, 4);
        assert!(audio.iter().all(|c| !list.contains(c)), "{audio:?}");
    }

    /// Turning the setting on or off recolours no channel or clip (spec, as above); only clips made afterwards follow it.
    #[test]
    fn toggling_the_setting_keeps_existing_colours() {
        let mut app = app_with(Some(theme(false, &[])));
        let (audio, auto) = new_channels(&mut app, 2);
        let before: Vec<Option<Rgb>> = app.state.project.channels.iter().map(|c| c.color).collect();
        let tints: Vec<Option<Rgb>> =
            app.state.project.channels.iter().map(|c| crate::playlist::clip_tint(&app.state, ClipSource::Channel(c.index))).collect();
        for on in [true, false, true] {
            let text = app.state.fl_text.take().expect("theme");
            app.state.fl_text = Some(os_theme::fl::set_key(&text, "OverrideClips", if on { "1" } else { "0" }));
            app.tick();
            assert_eq!(app.state.clip_colors.note_colors, on, "followed");
            assert_eq!(app.state.project.channels.iter().map(|c| c.color).collect::<Vec<_>>(), before, "channels, setting {on}");
            let now: Vec<Option<Rgb>> =
                app.state.project.channels.iter().map(|c| crate::playlist::clip_tint(&app.state, ClipSource::Channel(c.index))).collect();
            assert_eq!(now, tints, "clips, setting {on}");
        }
        // A clip made now follows it: on, a green.
        let (new_audio, new_auto) = new_channels(&mut app, 1);
        for c in new_audio.iter().chain(&new_auto) {
            let h = drawn(*c).hsl().0;
            assert!((ON_HUES.0 - 2.0..=ON_HUES.1 + 2.0).contains(&h), "{c:?}");
        }
        assert!(audio.iter().chain(&auto).all(|c| before.contains(&Some(*c))));
    }

    /// The Lemon design honours the setting too (spec, as above): the Theme page's toggle keeps it in the applied theme
    /// text there as well, and new clips' channels follow it. The colours are stored as for Classic; Lemon's rack
    /// mixes them 60 % toward its own name button face (`os_ui::kit::lemon::name_button`), which keeps them in their
    /// hue family, still tinted, and leaves the name text readable.
    #[test]
    fn the_lemon_design_honours_the_setting() {
        for on in [false, true] {
            let mut app = App::new(None);
            app.set_design(os_theme::Design::Lemon, false);
            if on {
                app.toggle_theme_option(crate::views::settings::NOTE_COLORS_CHECK);
            }
            app.tick();
            assert_eq!(app.clip_colors().note_colors, on);
            let (audio, auto) = new_channels(&mut app, 4);
            let (face, text) = (app.theme.colors.name_button, app.theme.colors.name_text);
            for &c in audio.iter().chain(&auto) {
                let (h, s, _) = drawn(c).hsl();
                if on {
                    assert!((ON_HUES.0 - 2.0..=ON_HUES.1 + 2.0).contains(&h), "{c:?}: hue {h}");
                } else {
                    assert!((s - OFF.0).abs() < 0.025, "{c:?}: saturation {s}");
                }
                let lemon = crate::views::rgb(c).mix(face, 0.6);
                let (lh, ls, ll) = lemon.hsl();
                assert!(apart(lh, h) < 12.0 && ls > 0.03, "Lemon face {lemon:?} of {c:?}: hue {lh} (was {h}), s {ls}");
                assert!(text.hsl().2 - ll > 0.45, "name text on {lemon:?}");
            }
        }
    }

    /// The colour stored is the one the rack draws at the measured values: for the measured buttons it lands among
    /// FL's stored audio-clip pastels (one component near 0x68, one near 0x7e).
    #[test]
    fn stored_colours_draw_as_measured() {
        // Three measured "off" buttons (spec table): #57665d, #655856, #5e5565.
        for hex in [0x57665d, 0x655856, 0x5e5565] {
            let d = Color::rgb(hex);
            let s = stored([f32::from(d.r), f32::from(d.g), f32::from(d.b)]);
            let back = drawn(s);
            assert!((i32::from(back.r) - i32::from(d.r)).abs() <= 1 && (i32::from(back.g) - i32::from(d.g)).abs() <= 1 && (i32::from(back.b) - i32::from(d.b)).abs() <= 1, "{hex:06x}: {back:?}");
            let (lo, hi) = (s.r.min(s.g).min(s.b), s.r.max(s.g).max(s.b));
            assert!((0x60..=0x70).contains(&lo) && (0x78..=0x88).contains(&hi), "{hex:06x} stored as {s:?}");
        }
    }
}
