// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The metronome switch and sound (toolbar, OPTIONS > Metronome, Ctrl+M, the button's right-click menu). A program
//! setting, not part of the project: kept in the user folder (`metronome.txt`) and shared with every playing engine,
//! which hears a change from its next beat.
// spec: ui/metronome.md §1-3

use std::sync::{Arc, OnceLock}; // This code is a part of L e m o n S t u d i o.

use os_engine::metronome::{Sound, Switch};

#[cfg_attr(test, allow(dead_code))]
static SWITCH: OnceLock<Arc<Switch>> = OnceLock::new();

/// The switch every playback engine clicks with: read from `metronome.txt` the first time (tests: off, Tick, one per
/// test thread so that tests running side by side don't see each other's clicks).
pub fn switch() -> Arc<Switch> {
    #[cfg(test)]
    {
        thread_local!(static TEST: Arc<Switch> = Arc::new(Switch::default()));
        TEST.with(Arc::clone)
    }
    #[cfg(not(test))]
    SWITCH.get_or_init(|| Arc::new(load())).clone()
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

/// `key=value` lines: `on` (0 / 1) and `sound` (`Sound::key`); anything else is ignored.
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
            _ => {}
        }
    }
    s
}

pub fn text(s: &Switch) -> String {
    format!("on={}\nsound={}\n", u8::from(s.on()), s.sound().key())
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
        assert!(!d.on() && d.sound() == Sound::Tick);
        let s = parse("on=1\nsound=cowbell\njunk\nother=3\n");
        assert!(s.on() && s.sound() == Sound::Cowbell);
        assert_eq!(text(&s), "on=1\nsound=cowbell\n");
        assert_eq!(parse("sound=nope").sound(), Sound::Tick);
    }
}
