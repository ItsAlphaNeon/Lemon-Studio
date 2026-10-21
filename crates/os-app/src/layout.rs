// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Where the windows were when Lemon Studio last closed, put back at the next start: the main window (its restored
//! frame and whether it was maximized) and, inside it, the Playlist, Mixer and Piano roll (their floating rects and
//! whether each was maximized or collapsed to its title bar), the Channel rack (its place and size), the Plugin
//! performance monitor, the Tempo tapper and the Touch controller. App-wide, kept in `settings.txt` (`settings_file`)
//! as the app closes, never in the project. Which windows are open is not kept: a start shows its usual ones.
//!
//! Keys: `window_main` (`x,y,w,h,maximized`: outer top-left and inner size in the units the window system places
//! windows in, points on macOS and pixels elsewhere), `window_playlist`, `window_mixer`, `window_piano_roll`
//! (`x,y,w,h,maximized,collapsed`), `window_channel_rack` (`x,y,w,h`), `window_plugin_performance`,
//! `window_tempo_tapper`, `window_touch_controller` (`x,y`); positions inside the main window are logical px, switches
//! `1` / `0`. A value that cannot be read leaves that window as a first start has it. The first paint keeps every
//! window's part in the workspace, should the main window be smaller now (`win_clamp`).

use os_ui::Rect;

use crate::app::{State, Win, WinState};
use crate::settings_file::{get, write};
use crate::views;

/// The Playlist, Mixer and Piano roll (`State::wins`) and their keys.
const WINS: [(Win, &str); 3] = [(Win::Playlist, "window_playlist"), (Win::Mixer, "window_mixer"), (Win::PianoRoll, "window_piano_roll")];
const RACK: &str = "window_channel_rack";
const PERF: &str = "window_plugin_performance";
const TAPPER: &str = "window_tempo_tapper";
const TOUCH: &str = "window_touch_controller";
const MAIN: &str = "window_main";

/// How tall a strip of the main window's top must be on a screen at the start, in `MainWin` units (`on_screens`).
pub const MAIN_BAR_H: i32 = 32;

/// The main window as it was left: its restored frame (outer top-left and inner size, in the units the window system
/// places windows in: points on macOS, pixels elsewhere) and whether it was maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainWin {
    pub pos: (i32, i32),
    pub size: (u32, u32),
    pub max: bool,
}

/// `key`'s value as `N` comma-separated finite numbers; `None` when it is not set or is anything else.
fn numbers<const N: usize>(key: &str) -> Option<[f32; N]> {
    let v: Vec<f32> = get(key)?.split(',').map(|n| n.trim().parse().ok().filter(|n: &f32| n.is_finite())).collect::<Option<_>>()?;
    v.try_into().ok()
}

fn keep(key: &str, v: &[f32]) {
    write(key, &v.iter().map(f32::to_string).collect::<Vec<_>>().join(","));
}

fn switch(on: bool) -> f32 {
    if on { 1.0 } else { 0.0 }
}

/// The windows inside the main window back where `save` left them; one not kept (or unreadable) stays as it is.
pub fn restore(s: &mut State) {
    for (w, key) in WINS {
        let (Some(i), Some([x, y, rw, rh, max, mini])) = (w.slot(), numbers(key)) else { continue };
        if rw > 0.0 && rh > 0.0 {
            s.wins[i] = WinState { rect: Rect::new(x, y, rw, rh), maxed: max == 1.0, mini: mini == 1.0 };
        }
    }
    if let Some([x, y, w, h]) = numbers(RACK) {
        s.rack_pos = (x, y);
        // Not below the smallest size a resize leaves it (`views::rack::snap_size`).
        s.rack_size = (w.max(views::rack::base_w(s) + 3.0 * views::rack::STEP_W), h.max(views::rack::MIN_H));
    }
    // Placed: they open there, not centred.
    for (key, pos, placed) in [(PERF, &mut s.perf.pos, &mut s.perf.placed), (TAPPER, &mut s.tapper.pos, &mut s.tapper.placed), (TOUCH, &mut s.touch.pos, &mut s.touch.placed)] {
        if let Some([x, y]) = numbers(key) {
            *pos = (x, y);
            *placed = true;
        }
    }
}

/// Keeps where the windows inside the main window are, for the next start. The monitor, tapper and touch controller only
/// once they have a place (opened in this run or an earlier one).
pub fn save(s: &State) {
    for (w, key) in WINS {
        if let Some(i) = w.slot() {
            let ws = s.wins[i];
            keep(key, &[ws.rect.x, ws.rect.y, ws.rect.w, ws.rect.h, switch(ws.maxed), switch(ws.mini)]);
        }
    }
    keep(RACK, &[s.rack_pos.0, s.rack_pos.1, s.rack_size.0, s.rack_size.1]);
    for (key, pos, placed) in [(PERF, s.perf.pos, s.perf.placed), (TAPPER, s.tapper.pos, s.tapper.placed), (TOUCH, s.touch.pos, s.touch.placed)] {
        if placed {
            keep(key, &[pos.0, pos.1]);
        }
    }
}

/// The main window as the last run left it, `None` when not kept.
pub fn main_win() -> Option<MainWin> {
    let [x, y, w, h, max] = numbers(MAIN)?;
    (w >= 1.0 && h >= 1.0).then(|| MainWin { pos: (x.round() as i32, y.round() as i32), size: (w.round() as u32, h.round() as u32), max: max == 1.0 })
}

/// Keeps the main window's frame for the next start.
pub fn save_main(m: MainWin) {
    let (x, y, w, h) = (m.pos.0 as f32, m.pos.1 as f32, m.size.0 as f32, m.size.1 as f32);
    keep(MAIN, &[x, y, w, h, switch(m.max)]);
}

/// `m` on the screens there are now (`screens`: [left, top, right, bottom] in `MainWin` units): where it was while its
/// top strip (`MAIN_BAR_H`) is on them, else moved as little as it takes for that strip to be fully on the screen it
/// overlaps most (the nearest when none: a screen that is gone), and no larger than that screen
/// (`win_clamp::bar_on_screens`).
pub fn on_screens(m: MainWin, screens: &[[i32; 4]]) -> MainWin {
    let w = m.size.0.min(i32::MAX as u32) as i32;
    let bar = [m.pos.0, m.pos.1, m.pos.0.saturating_add(w), m.pos.1.saturating_add(MAIN_BAR_H)];
    let to = crate::win_clamp::bar_on_screens(bar, screens);
    if to == m.pos {
        return m;
    }
    let on = screens.iter().find(|s| (s[0]..s[2]).contains(&to.0) && (s[1]..s[3]).contains(&to.1));
    let size = on.map_or(m.size, |s| (m.size.0.min((s[2] - s[0]).max(1) as u32), m.size.1.min((s[3] - s[1]).max(1) as u32)));
    MainWin { pos: to, size, ..m }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    /// Moved, resized, maximized and collapsed windows come back as they were at the next start; the monitor, tapper and
    /// touch controller open where they were left instead of centred.
    #[test]
    fn windows_come_back_where_they_were() {
        let mut app = App::new(None);
        let s = &mut app.state;
        s.wins[0] = WinState { rect: Rect::new(400.0, 150.5, 900.0, 500.0), maxed: false, mini: false };
        s.wins[1] = WinState { rect: Rect::new(-200.0, 600.0, 1400.0, 420.0), maxed: false, mini: true };
        s.wins[2] = WinState { rect: Rect::new(300.0, 200.0, 1100.0, 700.0), maxed: true, mini: false };
        s.rack_pos = (820.0, 330.0);
        s.rack_size = (900.0, 400.0);
        s.perf.pos = (500.0, 300.0);
        s.perf.placed = true;
        s.tapper.pos = (700.0, 420.0);
        s.tapper.placed = true;
        save(&app.state);

        let mut next = App::new(None);
        assert!(next.state.wins.iter().all(|w| w.maxed), "a new App starts with the defaults");
        restore(&mut next.state);
        let (a, b) = (&app.state, &next.state);
        for i in 0..3 {
            assert_eq!((b.wins[i].rect, b.wins[i].maxed, b.wins[i].mini), (a.wins[i].rect, a.wins[i].maxed, a.wins[i].mini), "window {i}");
        }
        assert_eq!((b.rack_pos, b.rack_size), ((820.0, 330.0), (900.0, 400.0)));
        assert_eq!((b.perf.pos, b.perf.placed, b.tapper.pos, b.tapper.placed), ((500.0, 300.0), true, (700.0, 420.0), true));
        assert!(!b.touch.placed, "never opened: still centred the first time");
    }

    /// A value that cannot be read leaves its window as a first start has it; a rack kept smaller than a resize allows
    /// comes back at the smallest size.
    #[test]
    fn unreadable_values_keep_the_defaults() {
        write("window_playlist", "10,20,abc,40,0,0");
        write("window_mixer", "10,20,30");
        write("window_piano_roll", "10,20,0,400,0,0");
        write("window_tempo_tapper", "NaN,5");
        write("window_channel_rack", "100,120,10,10");
        let mut app = App::new(None);
        let before: Vec<_> = app.state.wins.iter().map(|w| (w.rect, w.maxed, w.mini)).collect();
        restore(&mut app.state);
        let after: Vec<_> = app.state.wins.iter().map(|w| (w.rect, w.maxed, w.mini)).collect();
        assert_eq!(after, before);
        assert!(!app.state.tapper.placed);
        let min_w = views::rack::base_w(&app.state) + 3.0 * views::rack::STEP_W;
        assert_eq!((app.state.rack_pos, app.state.rack_size), ((100.0, 120.0), (min_w, views::rack::MIN_H)));
        assert_eq!(main_win(), None);
    }

    /// The main window's frame and maximized state are kept as given.
    #[test]
    fn the_main_window_is_kept() {
        let m = MainWin { pos: (-1700, 40), size: (1600, 900), max: true };
        save_main(m);
        assert_eq!(get("window_main").as_deref(), Some("-1700,40,1600,900,1"));
        assert_eq!(main_win(), Some(m));
    }

    /// On a screen it stays; on one that is gone it moves to the nearest one there is, as small as it.
    #[test]
    fn the_main_window_stays_on_the_screens() {
        let screens = [[0, 0, 1920, 1080], [1920, 0, 3200, 800]];
        let at = |x, y, w, h| MainWin { pos: (x, y), size: (w, h), max: false };
        assert_eq!(on_screens(at(100, 50, 1200, 800), &screens), at(100, 50, 1200, 800));
        assert_eq!(on_screens(at(1500, 50, 1200, 800), &screens), at(1500, 50, 1200, 800), "across the two");
        assert_eq!(on_screens(at(100, -100, 1200, 800), &screens), at(100, 0, 1200, 800), "above the top");
        // The screen it was on (to the left) is gone: onto the nearest, its top-left at the screen's edge.
        let gone = MainWin { pos: (-2500, 100), size: (2400, 1300), max: true };
        assert_eq!(on_screens(gone, &screens), MainWin { pos: (0, 100), size: (1920, 1080), max: true });
        assert_eq!(on_screens(gone, &[]), gone, "no screens known: as it was");
    }
}
