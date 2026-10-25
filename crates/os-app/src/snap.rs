// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The main toolbar's global snap (the snap panel), the editors' own snap settings that follow it through Main, and
//! the user-folder file that keeps all three.
// spec: ui/global-snap.md

use crate::app::State;
use crate::playlist::{Cmd, Mark, PItem, Popup, Snap};

/// The combo's hint (§6).
pub const HINT: &str = "Main snap / quantization / step edit step (Backspace: snap on / off)";
/// The magnet icon's hint (§6).
pub const MAGNET_HINT: &str = "Snap panel: click turns snapping on / off (Backspace)";

/// The piano roll's and the playlist's snap setting (ui/piano-snap.md §2, ui/playlist.md §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorSnap {
    /// The global snap.
    #[default]
    Main,
    /// Line, Cell, (none) and the fixed units.
    Grid(Snap),
    /// The starts and ends of the pattern's notes (piano roll).
    Events,
    /// The time markers (piano roll).
    Markers,
}

impl EditorSnap {
    pub fn label(self) -> &'static str {
        match self {
            EditorSnap::Main => "Main",
            EditorSnap::Grid(sn) => sn.label(),
            EditorSnap::Events => "Events",
            EditorSnap::Markers => "Markers",
        }
    }

    /// The grid setting with Main resolved to `global`; None for Events and Markers.
    pub fn grid(self, global: Snap) -> Option<Snap> {
        match self {
            EditorSnap::Main => Some(global),
            EditorSnap::Grid(sn) => Some(sn),
            EditorSnap::Events | EditorSnap::Markers => None,
        }
    }

    fn parse(t: &str) -> Option<EditorSnap> {
        match t {
            "Main" => Some(EditorSnap::Main),
            "Events" => Some(EditorSnap::Events),
            "Markers" => Some(EditorSnap::Markers),
            _ => parse_snap(t).map(EditorSnap::Grid),
        }
    }
}

fn parse_snap(t: &str) -> Option<Snap> {
    Snap::MENU.iter().copied().find(|sn| sn.label() == t)
}

/// The global snap (§1): the setting, and the last one other than (none), which the toggle goes back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Global {
    pub now: Snap,
    last: Snap,
}

impl Default for Global {
    fn default() -> Self {
        Global { now: Snap::Line, last: Snap::Line }
    }
}

impl Global {
    pub fn set(&mut self, sn: Snap) {
        self.now = sn;
        if sn != Snap::None {
            self.last = sn;
        }
    }

    /// The magnet icon and Backspace: (none) and back to the last other setting (§1).
    pub fn toggle(&mut self) {
        let to = if self.now == Snap::None { self.last } else { Snap::None };
        self.set(to);
    }

    /// The wheel over the combo: the next menu item (`down`) or the previous one, wrapping (§1).
    pub fn step(&mut self, down: bool) {
        let n = Snap::MENU.len();
        let i = Snap::MENU.iter().position(|&sn| sn == self.now).unwrap_or(0);
        self.set(Snap::MENU[if down { (i + 1) % n } else { (i + n - 1) % n }]);
    }
}

/// The combo's Snap menu (§2), its top-left at (x, y): the editors' list without Main, Events and Markers.
pub fn menu(s: &State, x: f32, y: f32) -> Popup {
    let radio = |sn: Snap| PItem::new(sn.label(), Cmd::GlobalSnap(sn)).mark(Mark::Radio(s.global_snap.now == sn));
    let mut items = vec![radio(Snap::Line), radio(Snap::Cell), PItem::sep()];
    items.extend(Snap::MENU.iter().filter(|sn| !matches!(sn, Snap::Line | Snap::Cell)).map(|&sn| radio(sn)));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

fn changed(s: &State) -> String {
    save(s);
    format!("Main snap: {}", s.global_snap.now.label())
}

/// Pick a Snap menu item.
pub fn set(s: &mut State, sn: Snap) -> String {
    s.global_snap.set(sn);
    changed(s)
}

/// The magnet icon and Backspace.
pub fn toggle(s: &mut State) -> String {
    s.global_snap.toggle();
    changed(s)
}

/// The wheel over the combo: one setting per notch (positive: down, toward Bar).
pub fn wheel(s: &mut State, notches: i32) -> String {
    for _ in 0..notches.unsigned_abs() {
        s.global_snap.step(notches > 0);
    }
    changed(s)
}

/// The three snap settings, as the user folder keeps them (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Saved {
    pub global: Global,
    pub piano: EditorSnap,
    pub playlist: EditorSnap,
}

impl Saved {
    fn of(s: &State) -> Saved {
        Saved { global: s.global_snap, piano: s.piano.snap, playlist: s.playlist.snap }
    }
}

/// `snap.txt`: one `key=label` line per setting; unknown keys and labels keep the defaults. The playlist has no
/// Events or Markers yet (§6).
pub fn parse(text: &str) -> Saved {
    let mut out = Saved::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "global" => {
                if let Some(sn) = parse_snap(v) {
                    out.global.now = sn;
                }
            }
            "last" => {
                if let Some(sn) = parse_snap(v).filter(|&sn| sn != Snap::None) {
                    out.global.last = sn;
                }
            }
            "piano" => out.piano = EditorSnap::parse(v).unwrap_or_default(),
            "playlist" => out.playlist = EditorSnap::parse(v).filter(|p| !matches!(p, EditorSnap::Events | EditorSnap::Markers)).unwrap_or_default(),
            _ => {}
        }
    }
    out
}

pub fn text(s: &Saved) -> String {
    format!("global={}\nlast={}\npiano={}\nplaylist={}\n", s.global.now.label(), s.global.last.label(), s.piano.label(), s.playlist.label())
}

/// The saved settings (tests: the defaults).
pub fn load() -> Saved {
    #[cfg(test)]
    return Saved::default();
    #[cfg(not(test))]
    os_paths::user_file("snap.txt").and_then(|f| std::fs::read_to_string(f).ok()).map(|t| parse(&t)).unwrap_or_default()
}

/// Keeps the three settings for the next run (tests: nothing is written).
pub fn save(s: &State) {
    let t = text(&Saved::of(s));
    #[cfg(not(test))]
    if let Some(dir) = os_paths::user_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("snap.txt"), t);
    }
    #[cfg(test)]
    let _ = t;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_and_wheel() {
        let mut g = Global::default();
        assert_eq!(g.now, Snap::Line);
        g.set(Snap::Beat);
        g.toggle();
        assert_eq!(g.now, Snap::None);
        g.toggle();
        assert_eq!(g.now, Snap::Beat, "back to the last other setting");
        // (none) picked from the menu: the toggle still goes back to Beat.
        g.set(Snap::None);
        g.toggle();
        assert_eq!(g.now, Snap::Beat);
        // The wheel walks the menu's order and wraps both ways.
        let mut g = Global::default();
        g.step(false);
        assert_eq!(g.now, Snap::Bar, "wheeling up from Line gives Bar");
        g.step(true);
        assert_eq!(g.now, Snap::Line);
        g.step(true);
        g.step(true);
        assert_eq!(g.now, Snap::None);
        g.step(true);
        assert_eq!(g.now, Snap::SixthStep);
        g.toggle();
        g.toggle();
        assert_eq!(g.now, Snap::SixthStep);
    }

    #[test]
    fn settings_file_round_trips() {
        assert_eq!(parse(""), Saved::default());
        let mut s = Saved { piano: EditorSnap::Markers, playlist: EditorSnap::Grid(Snap::Cell), ..Saved::default() };
        s.global.set(Snap::HalfBeat);
        s.global.toggle();
        let t = text(&s);
        assert_eq!(t, "global=(none)\nlast=1/2 beat\npiano=Markers\nplaylist=Cell\n");
        assert_eq!(parse(&t), s);
        // Junk keeps the defaults; the playlist has no Events or Markers.
        let p = parse("global=Whole\nlast=(none)\npiano=1/3 step\nplaylist=Events\nother=1\njunk\n");
        assert_eq!(p, Saved { piano: EditorSnap::Grid(Snap::ThirdStep), ..Saved::default() });
    }

    #[test]
    fn menu_matches_the_spec() {
        let mut app = crate::app::App::new(None);
        let s = &mut app.state;
        let labels = |p: &Popup| p.items.iter().map(|i| if i.sep { "-".to_string() } else { i.label.to_string() }).collect::<Vec<_>>().join(",");
        let m = menu(s, 765.0, 72.0);
        assert_eq!(labels(&m), "Line,Cell,-,(none),1/6 step,1/4 step,1/3 step,1/2 step,Step,1/6 beat,1/4 beat,1/3 beat,1/2 beat,Beat,Bar");
        assert_eq!(m.items[0].mark, Mark::Radio(true));
        assert_eq!(set(s, Snap::Bar), "Main snap: Bar");
        let m = menu(s, 0.0, 0.0);
        assert!(m.items.iter().filter(|i| i.mark == Mark::Radio(true)).all(|i| i.label == "Bar"));
        assert_eq!(toggle(s), "Main snap: (none)");
        assert_eq!(wheel(s, 1), "Main snap: 1/6 step");
        assert_eq!(wheel(s, -3), "Main snap: Line");
    }
}
