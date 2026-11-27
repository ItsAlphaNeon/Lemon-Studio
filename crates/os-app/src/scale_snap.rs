// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Snap to scale: with "Snap PR notes to scale" on (the piano roll toolbar's note button, or the Scale highlighting
//! popup), notes drawn or moved in the piano roll land on the keys of the highlighted scale; with "Snap incoming MIDI"
//! ▸ "Typing to piano keyboard" on, so do the typing keyboard's notes. The switches are not saved in the project.
// spec: ui/piano-scale-snap.md

/// The key of the scale `keys` (bit `pc`, C = bit 0) nearest to `key`; `up` decides a key halfway between two. A key
/// already in the scale, or an empty scale, stays.
pub fn snap_key(key: i32, keys: u16, up: bool) -> i32 {
    snap_key_in(key, keys, up, i32::MIN, i32::MAX)
}

/// [`snap_key`] among the keys `lo..=hi` only: a scale key past either end is passed over for the nearest one the
/// other way.
pub fn snap_key_in(key: i32, keys: u16, up: bool, lo: i32, hi: i32) -> i32 {
    let keys = keys & 0xFFF;
    let has = |k: i32| (lo..=hi).contains(&k) && keys & (1 << k.rem_euclid(12)) != 0;
    if keys == 0 || has(key) {
        return key;
    }
    for d in 1..=12 {
        let (a, b) = if up { (key + d, key - d) } else { (key - d, key + d) };
        if has(a) {
            return a;
        }
        if has(b) {
            return b;
        }
    }
    key
}

/// The scale key nearest to the continuous pitch `key + frac - 0.5`, where `frac` is the mouse's place in the row
/// (0 bottom edge, 1 top edge; §2): the upper half of an out-of-scale row goes up, the lower half down, the middle up.
pub fn snap_pitch(key: i32, keys: u16, frac: f32) -> i32 {
    snap_pitch_in(key, keys, frac, i32::MIN, i32::MAX)
}

/// [`snap_pitch`] among the keys `lo..=hi` only.
pub fn snap_pitch_in(key: i32, keys: u16, frac: f32, lo: i32, hi: i32) -> i32 {
    let keys = keys & 0xFFF;
    let has = |k: i32| (lo..=hi).contains(&k) && keys & (1 << k.rem_euclid(12)) != 0;
    if keys == 0 || has(key) {
        return key;
    }
    let p = key as f32 + frac.clamp(0.0, 1.0) - 0.5;
    let mut best = key;
    let mut dist = f32::MAX;
    for k in key - 12..=key + 12 {
        if !has(k) {
            continue;
        }
        let d = (k as f32 - p).abs();
        if d < dist - 1e-4 || ((d - dist).abs() <= 1e-4 && k > best) {
            (best, dist) = (k, d);
        }
    }
    best
}

/// The Snap to scale switches (§1). Session state only: not in the project (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Settings {
    /// "Snap PR notes to scale": notes drawn or moved land on the scale.
    pub notes: bool,
    /// "Snap incoming MIDI" ▸ "Typing to piano keyboard" ("Snap all devices" is not there yet).
    pub midi_typing: bool,
}

/// The key a note drawn at `tick` on row `key` gets, the mouse at `frac` of the row's height (§2): snapped while
/// "Snap PR notes to scale" is on.
pub fn drawn(s: &crate::app::State, tick: u32, key: i32, frac: f32) -> i32 {
    if !s.piano.scale_snap.notes {
        return key;
    }
    snap_pitch_in(key, keys_at(s, tick), frac, 0, crate::piano::TOP_KEY).clamp(0, crate::piano::TOP_KEY)
}

/// [`drawn`] without the key range: a chord stamp drops the keys that land past it rather than moving them
/// (spec: ui/piano-chord-stamp.md §3).
pub fn drawn_free(s: &crate::app::State, tick: u32, key: i32, frac: f32) -> i32 {
    if !s.piano.scale_snap.notes {
        return key;
    }
    snap_pitch(key, keys_at(s, tick), frac)
}

/// The key a note moved to `tick` on `key` gets (§3): snapped while "Snap PR notes to scale" is on, even without a key
/// change; halfway goes down.
pub fn moved(s: &crate::app::State, tick: u32, key: i32) -> i32 {
    if !s.piano.scale_snap.notes {
        return key;
    }
    snap_key_in(key, keys_at(s, tick), false, 0, crate::piano::TOP_KEY).clamp(0, crate::piano::TOP_KEY)
}

/// A typing keyboard note (§4): snapped to the scale at the song position while "Typing to piano keyboard" is on
/// (halfway goes up).
pub fn typed(s: &crate::app::State, key: i32) -> i32 {
    if !s.piano.scale_snap.midi_typing {
        return key;
    }
    snap_key_in(key, keys_at(s, s.session.pat_pos as u32), true, 0, 127)
}

/// The keys of the scale highlighted at `tick` (a key marker from its tick on, else the Scale highlighting setting).
pub fn keys_at(s: &crate::app::State, tick: u32) -> u16 {
    crate::piano::scale_spans(s).iter().take_while(|&&(at, _)| at <= tick).last().map_or(0xFFF, |&(_, k)| k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_scale_keys_go_to_the_nearest_scale_key() {
        let c_major = os_model::scale_keys(0, 0);
        assert_eq!(snap_key(60, c_major, true), 60, "C stays");
        assert_eq!(snap_key(61, c_major, true), 62, "C# halfway: up to D");
        assert_eq!(snap_key(61, c_major, false), 60, "C# halfway: down to C (a moved note, §3)");
        let c_pent = os_model::scale_keys(4, 0); // C D E G A
        assert_eq!(snap_key(65, c_pent, true), 64, "F is nearer E than G");
        assert_eq!(snap_key(0, 0, true), 0, "no scale: unchanged");
    }

    #[test]
    fn a_drawn_note_follows_the_mouse_in_its_row() {
        let c_major = os_model::scale_keys(0, 0);
        assert_eq!(snap_pitch(61, c_major, 0.5), 62, "middle of C#5: D5, as FL");
        assert_eq!(snap_pitch(61, c_major, 0.95), 62, "top edge: D5");
        assert_eq!(snap_pitch(61, c_major, 0.05), 60, "bottom edge: C5");
        assert_eq!(snap_pitch(54, c_major, 0.5), 55, "F#4: G4, as FL");
        assert_eq!(snap_pitch(60, c_major, 0.0), 60, "a scale row stays");
    }

    /// A state with Scale highlighting `mode` on `root`, Snap PR notes to scale and Typing to piano keyboard on.
    fn snapping(mode: i32, root: i32) -> crate::app::App {
        let mut app = crate::app::App::new(None);
        crate::edit::notes::put_scale_highlight(&mut app.state.session, os_model::ScaleHighlight { mode, root });
        app.state.piano.scale_snap.notes = true;
        app.state.piano.scale_snap.midi_typing = true;
        app
    }

    #[test]
    fn snapping_keeps_inside_the_key_range() {
        let app = snapping(4, 0); // C pentatonic: C D E G A
        assert_eq!(moved(&app.state, 0, 131), 129, "B: the C above is past the top, so A");
        assert_eq!(drawn(&app.state, 0, 131, 0.9), 129);
        let app = snapping(0, 11); // B major: B C# D# E F# G# A#
        assert_eq!(moved(&app.state, 0, 0), 1, "C: the B below is past the bottom, so C#");
        assert_eq!(drawn(&app.state, 0, 0, 0.1), 1);
    }

    #[test]
    fn typed_keys_snap_inside_the_midi_range() {
        let app = snapping(0, 1); // C# major: C# D# F F# G# A# C
        assert_eq!(typed(&app.state, 62), 63, "D halfway: up to D#");
        assert_eq!(typed(&app.state, 127), 126, "G: G# is past 127, so F#");
        let mut app = snapping(0, 1);
        app.state.piano.scale_snap.midi_typing = false;
        assert_eq!(typed(&app.state, 62), 62, "off: unchanged");
    }

    /// Snapping follows the scale whether or not the grid shows it (Note grid highlights, Alt+B).
    #[test]
    fn snap_does_not_need_the_grid_highlights() {
        let mut app = snapping(0, 0);
        app.state.piano.highlights = false;
        assert_eq!(moved(&app.state, 0, 61), 60);
        assert_eq!(typed(&app.state, 61), 62);
    }

    /// The switches outlive a project load, as the grid highlights do.
    #[test]
    fn the_switches_outlive_a_project() {
        let mut app = snapping(0, 0);
        app.state.piano.chord_name = false;
        app.state.piano.reset();
        assert!(app.state.piano.scale_snap.notes && app.state.piano.scale_snap.midi_typing);
        assert!(!app.state.piano.chord_name);
    }
}
