// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Enable groups (toolbar, OPTIONS > Enable groups, Shift+Alt+G): while on, grouped playlist clips select, move and
//! group as one; while off, each clip is on its own (the groups are kept). A program setting, not part of the project:
//! kept in the user folder (`groups.txt`).
// spec: ui/enable-groups.md

#[cfg_attr(test, allow(unused_imports))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

#[cfg_attr(test, allow(dead_code))]
static ON: OnceLock<AtomicBool> = OnceLock::new();

#[cfg(test)]
thread_local! { // This code is a part of L e m o n S t u d i o.
    /// Tests run in parallel threads: each has its own switch, so one test turning it off never changes another's clicks.
    static TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// The switch: read from `groups.txt` the first time (missing: on).
pub fn on() -> bool {
    #[cfg(test)]
    return TEST.with(std::cell::Cell::get);
    #[cfg(not(test))]
    ON.get_or_init(|| AtomicBool::new(load())).load(Ordering::Relaxed)
}

/// Toggles the switch and keeps it; returns the new state.
pub fn toggle() -> bool {
    let on = !on();
    #[cfg(test)]
    TEST.with(|c| c.set(on));
    #[cfg(not(test))]
    {
        ON.get_or_init(|| AtomicBool::new(on)).store(on, Ordering::Relaxed);
        save(on);
    }
    on
}

/// `on=0|1`; anything else is ignored (missing: on).
pub fn parse(text: &str) -> bool {
    text.lines().filter_map(|l| l.split_once('=')).filter(|(k, _)| k.trim() == "on").last().is_none_or(|(_, v)| v.trim() != "0")
}

pub fn text(on: bool) -> String {
    format!("on={}\n", u8::from(on))
}

#[cfg_attr(test, allow(dead_code))]
fn load() -> bool {
    os_paths::user_file("groups.txt").and_then(|f| std::fs::read_to_string(f).ok()).is_none_or(|t| parse(&t))
}

#[cfg_attr(test, allow(dead_code))]
fn save(on: bool) {
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("groups.txt"), text(on));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_file_round_trips_and_defaults_to_on() {
        assert!(parse(""));
        assert!(parse("junk\nother=0\n"));
        assert!(!parse("on=0\n"));
        assert!(parse(&text(true)) && !parse(&text(false)));
        assert_eq!(text(false), "on=0\n");
    }
}
