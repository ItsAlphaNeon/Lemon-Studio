// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The metronome switch, sound and volume (toolbar, OPTIONS > Metronome, Ctrl+M, the button's right-click menu). A
//! program
//! setting, not part of the project: kept in the user folder (`metronome.txt`) and shared with every playing engine,
//! which hears a change from its next beat. The switch also carries Settings > Audio ▸ Metronome track
//! (`prefs::METRONOME_TRACK`, kept with the other settings).
// spec: ui/metronome.md §1-3

use std::sync::{Arc, OnceLock}; // This code is a part of L e m o n S t u d i o.

use os_engine::metronome::{Sound, Switch};

#[cfg_attr(test, allow(dead_code))]
static SWITCH: OnceLock<Arc<Switch>> = OnceLock::new();

/// The switch every playback engine clicks with: read from `metronome.txt` and the Metronome track setting the first
/// time (tests: off, Tick, one per test thread so that tests running side by side don't see each other's clicks).
pub fn switch() -> Arc<Switch> {
    #[cfg(test)]
    {
        thread_local!(static TEST: Arc<Switch> = Arc::new(with_track(Switch::default())));
        TEST.with(Arc::clone)
    }
    #[cfg(not(test))]
    SWITCH.get_or_init(|| Arc::new(with_track(load()))).clone()
}

fn with_track(s: Switch) -> Switch {
    s.set_track(crate::prefs::number(&crate::prefs::METRONOME_TRACK));
    s
}

pub fn on() -> bool {
    switch().on()
}

pub fn sound() -> Sound {
    switch().sound()
}

/// Toggles the switch and keeps it; returns the new state.
pub fn toggle() -> bool {
    let s = switch();
    s.set_on(!s.on());
    save(&s);
    s.on()
}

/// Picks the sound (the switch stays as it is) and keeps it.
pub fn set_sound(sound: Sound) {
    let s = switch();
    s.set_sound(sound);
    save(&s);
}

/// The volume in whole dB (spec §2 "Metronome volume").
pub fn volume() -> i8 {
    switch().volume()
}

/// Picks the volume (clamped to -24..=12 dB; the switch and the sound stay as they are) and keeps it.
pub fn set_volume(db: i8) {
    let s = switch();
    s.set_volume(db);
    save(&s);
}

/// `key=value` lines: `on` (0 / 1), `sound` (`Sound::key`) and `volume` (whole dB); anything else is ignored.
pub fn parse(text: &str) -> Switch {
    let s = Switch::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match (k.trim(), v.trim()) {
            ("on", v) => s.set_on(v == "1"),
            ("sound", v) => {
                if let Some(sound) = Sound::from_key(v) {
                    s.set_sound(sound);
                }
            }
            ("volume", v) => {
                if let Ok(db) = v.parse::<i32>() {
                    s.set_volume(db.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8);
                }
            }
            _ => {}
        }
    }
    s
}

pub fn text(s: &Switch) -> String {
    format!("on={}\nsound={}\nvolume={}\n", u8::from(s.on()), s.sound().key(), s.volume())
}

#[cfg_attr(test, allow(dead_code))]
fn load() -> Switch {
    os_paths::user_file("metronome.txt").and_then(|f| std::fs::read_to_string(f).ok()).map(|t| parse(&t)).unwrap_or_default()
}

fn save(s: &Switch) {
    if cfg!(test) {
        return;
    }
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("metronome.txt"), text(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_file_round_trips_and_defaults_to_off_tick() {
        let d = parse("");
        assert!(!d.on() && d.sound() == Sound::Tick && d.volume() == 0);
        let s = parse("on=1\nsound=cowbell\njunk\nother=3\nvolume=+6\n");
        assert!(s.on() && s.sound() == Sound::Cowbell && s.volume() == 6);
        assert_eq!(text(&s), "on=1\nsound=cowbell\nvolume=6\n");
        assert_eq!(parse("sound=nope").sound(), Sound::Tick);
        // Volumes are clamped to -24..=12 dB; one that is not a whole number is ignored.
        let v = ["volume=99", "volume=-400", "volume=-18", "volume=3.5"].map(|t| parse(t).volume());
        assert_eq!(v, [12, -24, -18, 0]);
    }
}
