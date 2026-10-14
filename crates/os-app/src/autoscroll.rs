// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Auto scrolling (toolbar, OPTIONS > Auto scrolling, the button's right-click menu): while the song or a pattern plays,
//! the view that shows the position follows the playhead, continuously or a quarter page at a time. A program setting,
//! not part of the project: kept in the user folder (`autoscroll.txt`).
// spec: ui/auto-scrolling.md

use std::sync::{Mutex, OnceLock}; // This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub on: bool,
    /// The playhead held at a third of the view; otherwise the view steps a quarter page.
    pub continuous: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { on: false, continuous: true }
    }
}

#[cfg_attr(test, allow(dead_code))]
static SETTINGS: OnceLock<Mutex<Settings>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    /// Tests run in parallel threads: each has its own settings, so one test's switch never scrolls another's views.
    static TEST: std::cell::Cell<Settings> = std::cell::Cell::new(Settings::default());
}

/// Runs `f` on the settings; keeps them when `f` changed them.
fn with<R>(f: impl FnOnce(&mut Settings) -> R) -> R {
    #[cfg(test)]
    return TEST.with(|c| {
        let mut s = c.get();
        let r = f(&mut s);
        c.set(s);
        r
    });
    #[cfg(not(test))]
    {
        let mut s = SETTINGS.get_or_init(|| Mutex::new(load())).lock().unwrap_or_else(|e| e.into_inner());
        let before = *s;
        let r = f(&mut s);
        if *s != before {
            save(&s);
        }
        r
    }
}

/// The current settings: read from `autoscroll.txt` the first time.
pub fn get() -> Settings {
    with(|s| *s)
}

pub fn on() -> bool {
    get().on
}

pub fn continuous() -> bool {
    get().continuous
}

/// Toggles the switch and keeps it; returns the new state.
pub fn toggle() -> bool {
    with(|s| {
        s.on = !s.on;
        s.on
    })
}

/// Toggles continuous scrolling (the switch stays as it is) and keeps it; returns the new state.
pub fn toggle_continuous() -> bool {
    with(|s| {
        s.continuous = !s.continuous;
        s.continuous
    })
}

/// Where a view `w` content pixels wide, scrolled to `left`, scrolls so the playhead at content x `x` stays in view;
/// None to stay (spec §5). Never below 0; the caller clamps to its scroll bar.
pub fn follow(left: f32, w: f32, x: f32, continuous: bool) -> Option<f32> {
    let to = if continuous {
        // Held at a third of the width once it gets there.
        if x >= left && x <= left + w / 3.0 {
            return None;
        }
        x - w / 3.0
    } else if x < left {
        x
    } else if x >= left + w * 7.0 / 8.0 {
        // A quarter page: the playhead goes back to 5/8.
        x - w * 5.0 / 8.0
    } else {
        return None;
    };
    Some(to.max(0.0))
}

/// `key=value` lines: `on` and `continuous` (0 / 1); anything else is ignored.
pub fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "on" => s.on = v.trim() == "1",
            "continuous" => s.continuous = v.trim() == "1",
            _ => {}
        }
    }
    s
}

pub fn text(s: &Settings) -> String {
    format!("on={}\ncontinuous={}\n", u8::from(s.on), u8::from(s.continuous))
}

#[cfg_attr(test, allow(dead_code))]
fn load() -> Settings {
    os_paths::user_file("autoscroll.txt").and_then(|f| std::fs::read_to_string(f).ok()).map(|t| parse(&t)).unwrap_or_default()
}

#[cfg_attr(test, allow(dead_code))]
fn save(s: &Settings) {
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("autoscroll.txt"), text(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_file_round_trips_and_defaults_to_off_continuous() {
        assert_eq!(parse(""), Settings { on: false, continuous: true });
        let s = parse("on=1\ncontinuous=0\njunk\nother=3\n");
        assert_eq!(s, Settings { on: true, continuous: false });
        assert_eq!(text(&s), "on=1\ncontinuous=0\n");
    }

    /// spec §5: continuous holds the playhead at w/3; paged steps a quarter page at 7/8; behind the view, back in view.
    #[test]
    fn follow_rules() {
        let w = 1200.0;
        // Continuous: still until a third, then held there; out of view either side, put at a third.
        assert_eq!(follow(0.0, w, 399.0, true), None);
        assert_eq!(follow(0.0, w, 410.0, true), Some(10.0));
        assert_eq!(follow(5000.0, w, 100.0, true), Some(0.0));
        assert_eq!(follow(5000.0, w, 2000.0, true), Some(1600.0));
        assert_eq!(follow(0.0, w, 9000.0, true), Some(8600.0));
        // Paged: still until 7/8, then the playhead lands at 5/8 (a quarter page).
        assert_eq!(follow(0.0, w, 1049.0, false), None);
        assert_eq!(follow(0.0, w, 1050.0, false), Some(300.0));
        assert_eq!(follow(300.0, w, 1100.0, false), None);
        assert_eq!(follow(0.0, w, 9000.0, false), Some(8250.0));
        // Behind the view: the playhead at the left edge.
        assert_eq!(follow(3000.0, w, 2000.0, false), Some(2000.0));
        assert_eq!(follow(3000.0, w, 0.0, false), Some(0.0));
    }
}
