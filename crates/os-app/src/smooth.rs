// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Eased scrolling and the frame rate of animations: General ▸ Display ▸ Smoothing and Animation refresh rate.
//!
//! A scroll by the mouse wheel or a scroll bar's arrow doesn't jump. `App::eased` runs the handler, puts the view
//! back where it is drawn and leaves an `Ease` for each axis it moved; `App::tick` moves the eases on every frame, two
//! chained stages, in whole pixels (the playlist in the whole steps its drawn pixels can move by, so that it moves
//! them instead of repainting: 1 px at 100 %, 2 at 150 %).
//!
//! The state's scroll is always what is drawn: painting, hit-testing and every handler see the view as it is on
//! screen, and only the target (where the wheel is taking the view) lives in the ease. Zooms, drags and jumps
//! (following playback, a scroll bar drag) set the scroll directly: an ease whose axis moved under it ends. A press
//! of any mouse button stops every ease where it is, but on a scroll bar's arrow, which steps on from the target.
// spec: ui/settings-choices.md "Display: Smoothing, Animation refresh rate, Transparent menus and windows (2026-10-16)"

use std::time::{Duration, Instant};

use crate::app::{App, Scroll};
use crate::prefs;

/// General ▸ Display ▸ Smoothing (`prefs::SMOOTHING`), 0 (scrolls jump) to 512, 128 by default.
pub fn smoothing() -> f32 {
    f32::from(prefs::slider(&prefs::SMOOTHING))
}

/// How much of the way one stage moves in a frame of `dt_ms` at Smoothing `value`: `1 - 2^(-dt / (value x 0.25 ms))`.
/// That is FL's `1 - 2^(-n / value)` with `n` = 40 for a nominal 10 ms frame (its Ultrasmooth), counted in time so
/// that the ease takes as long at any frame rate. All of it at 0.
pub fn share(dt_ms: f64, value: f64) -> f64 {
    if value <= 0.0 { 1.0 } else { 1.0 - (-dt_ms / (value * 0.25)).exp2() }
}

/// Animation refresh rate: the shortest time from one frame to the next while something animates. Ultrasmooth: none
/// (the display's own rate); Smooth: 60 Hz; Less smooth: 40 Hz.
pub fn frame_cap() -> Duration {
    match prefs::index(&prefs::ANIMATION_REFRESH_RATE) {
        0 => Duration::from_millis(25),
        1 => Duration::from_secs_f64(1.0 / 60.0),
        _ => Duration::ZERO,
    }
}

/// What eases: a scroll bar's axis, or the browser's list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Axis {
    Bar(Scroll),
    Browser,
}

const AXES: [Axis; 10] = [
    Axis::Bar(Scroll::PianoX),
    Axis::Bar(Scroll::PianoY),
    Axis::Bar(Scroll::PitchY),
    Axis::Bar(Scroll::PlaylistX),
    Axis::Bar(Scroll::PlaylistY),
    Axis::Bar(Scroll::PlPick),
    Axis::Bar(Scroll::MixerX),
    Axis::Bar(Scroll::RackX),
    Axis::Bar(Scroll::RackY),
    Axis::Browser,
];

/// One axis on its way to `target` (content pixels).
#[derive(Debug, Clone, PartialEq)]
pub struct Ease {
    pub axis: Axis,
    pub target: f64,
    /// The two chained stages: the first follows the target, the second the first; the view shows the second.
    first: f64,
    second: f64,
    /// Where the ease began: the view moves whole pixels from there.
    origin: f64,
    /// What it showed last: anything else moving the axis ends the ease.
    shown: f32,
}

impl Ease {
    pub fn new(axis: Axis, from: f32, target: f64) -> Ease {
        let f = f64::from(from);
        Ease { axis, target, first: f, second: f, origin: f, shown: from }
    }

    /// One frame of `dt_ms` at Smoothing `value`: the first stage moves `share` of the way to the target, the second
    /// as much of the way to the first. Returns where the view is now (whole steps of `q` px from where the ease
    /// began; the target once within half a pixel) and whether it is there.
    pub fn step(&mut self, dt_ms: f64, value: f64, q: f64) -> (f64, bool) {
        let k = share(dt_ms, value);
        self.first += (self.target - self.first) * k;
        self.second += (self.first - self.second) * k;
        if (self.second - self.target).abs() < 0.5 {
            return (self.target, true);
        }
        (self.origin + ((self.second - self.origin) / q).round() * q, false)
    }
}

/// The eases running, and when they last moved (`App::smooth`).
#[derive(Debug, Default)]
pub struct Eases {
    list: Vec<Ease>,
    at: Option<Instant>,
}

impl Eases {
    /// Whether a view is easing: the shell ticks at animation rate.
    pub fn running(&self) -> bool {
        !self.list.is_empty()
    }

    /// A mouse button went down: every view stops where it is drawn.
    pub fn stop(&mut self) {
        *self = Eases::default();
    }

    /// The ease of axis `a`, if it is easing.
    #[cfg(test)]
    pub fn of(&self, a: Axis) -> Option<&Ease> {
        self.list.iter().find(|e| e.axis == a)
    }
}

/// Which zoom (`zooms`) each axis follows: an axis isn't eased when its zoom changed.
fn zoom_of(a: Axis) -> Option<usize> {
    match a {
        Axis::Bar(Scroll::PlaylistX | Scroll::PlaylistY) => Some(0),
        Axis::Bar(Scroll::PianoX | Scroll::PianoY | Scroll::PitchY) => Some(1),
        _ => None,
    }
}

impl App {
    fn axis_get(&self, a: Axis) -> f32 {
        match a {
            Axis::Bar(t) => self.scroll_get(t),
            Axis::Browser => self.state.browser_scroll,
        }
    }

    /// Sets an axis, kept within what it can show.
    fn axis_set(&mut self, a: Axis, v: f32) {
        match a {
            Axis::Bar(t) => self.scroll_set(t, v),
            Axis::Browser => {
                let s = &mut self.state;
                s.browser_scroll = v.clamp(0.0, crate::views::browser::max_scroll(s));
            }
        }
    }

    /// The playlist's zoom and track height, the piano roll's: a change is a zoom.
    fn zooms(&self) -> [(f32, f32); 2] {
        let s = &self.state;
        [(s.playlist_bar_w, s.playlist.row_h), (s.piano.px_per_beat, s.piano.row_h)]
    }

    /// Runs `f`, a scroll by the mouse wheel or a scroll bar's arrow. With Smoothing above 0, every axis `f` moved goes
    /// back to where it is drawn and eases to where `f` took it; a notch during an ease moves its target on as far,
    /// and the ease goes on from where it is. A zoom (`f` changed a zoom or a track height) is not eased: those axes
    /// keep `f`'s values and their eases end.
    pub(crate) fn eased<R>(&mut self, f: impl FnOnce(&mut App) -> R) -> R {
        let before = AXES.map(|a| self.axis_get(a));
        let zooms = self.zooms();
        let r = f(self);
        let zoomed = self.zooms().iter().zip(zooms).map(|(a, b)| *a != b).collect::<Vec<_>>();
        let value = smoothing();
        for (a, v0) in AXES.into_iter().zip(before) {
            let v1 = self.axis_get(a);
            if zoom_of(a).is_some_and(|z| zoomed[z]) || (value <= 0.0 && v1 != v0) {
                self.smooth.list.retain(|e| e.axis != a);
                continue;
            }
            if v1 == v0 {
                continue;
            }
            let target = self.smooth.list.iter().find(|e| e.axis == a).map_or(f64::from(v1), |e| e.target + f64::from(v1 - v0));
            // As far as the axis can go.
            self.axis_set(a, target as f32);
            let target = f64::from(self.axis_get(a));
            self.axis_set(a, v0);
            let shown = self.axis_get(a);
            match self.smooth.list.iter_mut().find(|e| e.axis == a) {
                Some(e) => e.target = target,
                None => self.smooth.list.push(Ease::new(a, shown, target)),
            }
        }
        if self.smooth.running() && self.smooth.at.is_none() {
            self.smooth.at = Some(Instant::now());
        }
        r
    }

    /// The tick: every ease moves on by the time since it last moved.
    pub(crate) fn ease_tick(&mut self, now: Instant) {
        let Some(at) = self.smooth.at else { return };
        self.smooth.at = Some(now);
        self.ease_step(now.saturating_duration_since(at).as_secs_f64() * 1000.0);
    }

    /// One frame of `dt_ms` for every ease, and the damage it leaves: the playlist moves its pixels
    /// (`playlist_scrolled`, or is repainted where that can't be exact), the other windows repaint.
    pub(crate) fn ease_step(&mut self, dt_ms: f64) {
        let value = f64::from(smoothing());
        let pl = self.state.playlist_scroll;
        let blit = f64::from(self.playlist_blit_step().unwrap_or(1.0).max(1.0));
        let mut list = std::mem::take(&mut self.smooth.list);
        let mut moved = Vec::new();
        list.retain_mut(|e| {
            // Something else moved the axis (a drag, a jump, following playback): the ease ends there.
            if (self.axis_get(e.axis) - e.shown).abs() > 0.01 {
                return false;
            }
            let q = if matches!(e.axis, Axis::Bar(Scroll::PlaylistX | Scroll::PlaylistY)) { blit } else { 1.0 };
            let (v, done) = e.step(dt_ms, value, q);
            self.axis_set(e.axis, v as f32);
            let shown = self.axis_get(e.axis);
            if shown != e.shown {
                moved.push(e.axis);
            }
            e.shown = shown;
            !done
        });
        self.smooth.list = list;
        if !self.smooth.running() {
            self.smooth.at = None;
        }
        for a in moved {
            match a {
                Axis::Bar(Scroll::PlaylistX | Scroll::PlaylistY) => {}
                Axis::Bar(t) => self.damage_win(crate::app::scroll_win(t)),
                Axis::Browser => self.invalidate(),
            }
        }
        if self.state.playlist_scroll != pl {
            self.playlist_scrolled(pl);
        }
    }

    /// Tests: every ease at its end.
    #[cfg(test)]
    pub(crate) fn ease_to_end(&mut self) {
        for _ in 0..10_000 {
            if !self.smooth.running() {
                return;
            }
            self.ease_step(1000.0);
        }
    }
}

// spec: ui/settings-choices.md "Display: Smoothing, Animation refresh rate, Transparent menus and windows (2026-10-16)"
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, Win};
    use crate::playlist::PGeom;
    use crate::prefs::{set_slider, SMOOTHING};
    use os_ui::Canvas;

    /// What a 48 px notch at 128 showed, one value per frame (repeats dropped): the spec's measured sequence.
    const NOTCH_48: [f64; 22] =
        [2.0, 5.0, 8.0, 12.0, 16.0, 20.0, 23.0, 26.0, 29.0, 32.0, 34.0, 36.0, 38.0, 39.0, 41.0, 42.0, 43.0, 44.0, 45.0, 46.0, 47.0, 48.0];

    /// The playlist alone, painted, the mouse over its grid.
    fn playlist_app(c: &mut Canvas) -> App {
        let mut app = App::new(None);
        app.state.z = vec![Win::Playlist];
        app.paint(c, None);
        let g = app.pl_grid().expect("playlist shown");
        app.mouse_move(g.x + 300.0, g.y + 100.0);
        app
    }

    /// The values `ease_step(dt_ms)` shows on `axis` until the ease ends, repeats dropped.
    fn shown(app: &mut App, axis: Axis, dt_ms: f64) -> Vec<f64> {
        let mut v: Vec<f64> = Vec::new();
        for _ in 0..1000 {
            if !app.smooth.running() {
                return v;
            }
            app.ease_step(dt_ms);
            let x = f64::from(app.axis_get(axis));
            if v.last() != Some(&x) {
                v.push(x);
            }
        }
        panic!("the ease ends");
    }

    /// The 48 px notch at 128 in 10 ms frames (FL's `n` = 40) shows the spec's sequence, in the stages alone and as
    /// the playlist's scroll; it settles in about the same time at any frame rate.
    #[test]
    fn a_notch_eases_as_measured() {
        let mut e = Ease::new(Axis::Bar(Scroll::PlaylistY), 0.0, 48.0);
        let mut v: Vec<f64> = Vec::new();
        for _ in 0..100 {
            let (x, done) = e.step(10.0, 128.0, 1.0);
            if v.last() != Some(&x) {
                v.push(x);
            }
            if done {
                break;
            }
        }
        assert_eq!(v, NOTCH_48);
        assert!((share(10.0, 128.0) - 0.195).abs() < 0.001, "k at 128: {}", share(10.0, 128.0));

        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 128);
        app.eased(|app| app.scroll_set(Scroll::PlaylistY, 48.0));
        assert_eq!(app.state.playlist_scroll.1, 0.0, "the view starts where it is");
        assert!(app.animating(), "an ease keeps the frames coming");
        assert_eq!(shown(&mut app, Axis::Bar(Scroll::PlaylistY), 10.0), NOTCH_48);
        assert!(!app.animating());

        // Time, not frames: 180 Hz, 60 Hz and 40 Hz all settle in about 300 ms.
        for dt in [1000.0 / 180.0, 1000.0 / 60.0, 25.0] {
            let mut e = Ease::new(Axis::Bar(Scroll::PlaylistY), 0.0, 48.0);
            let frames = (1..1000).find(|_| e.step(dt, 128.0, 1.0).1).unwrap();
            let ms = frames as f64 * dt;
            assert!((270.0..=330.0).contains(&ms), "settled after {ms} ms at {dt} ms frames");
        }
    }

    /// Smoothing 0: the wheel scrolls at once, nothing eases. At 128 the same notch ends in the same place.
    #[test]
    fn zero_is_instant() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 0);
        app.wheel(-1.0, -1.0, false, false);
        let at_once = app.state.playlist_scroll;
        assert!(at_once.0 > 0.0 && at_once.1 > 0.0, "the wheel scrolled: {at_once:?}");
        assert!(!app.smooth.running(), "nothing eases at 0");

        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 128);
        app.wheel(-1.0, -1.0, false, false);
        assert_eq!(app.state.playlist_scroll, (0.0, 0.0), "at 128 the view starts where it is");
        assert!(app.smooth.running());
        app.ease_to_end();
        assert_eq!(app.state.playlist_scroll, at_once, "and eases to where 0 jumps");
    }

    /// A notch during an ease moves its target on by a notch; the view goes on from where it is, without a jump back,
    /// and ends where two notches at once end.
    #[test]
    fn a_notch_during_an_ease_retargets_it() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let y = Axis::Bar(Scroll::PlaylistY);
        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 0);
        app.wheel(0.0, -2.0, false, false);
        let two = app.state.playlist_scroll.1;

        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 128);
        app.wheel(0.0, -1.0, false, false);
        let one = app.smooth.of(y).expect("eases").target;
        for _ in 0..5 {
            app.ease_step(10.0);
        }
        let mid = app.state.playlist_scroll.1;
        assert!(mid > 0.0 && f64::from(mid) < one, "half way: {mid} of {one}");
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(app.state.playlist_scroll.1, mid, "the notch doesn't move the view at once");
        assert_eq!(app.smooth.of(y).expect("still easing").target, f64::from(two), "the target moved on a notch");
        let rest = shown(&mut app, y, 10.0);
        assert!(rest.windows(2).all(|w| w[0] < w[1]) && rest[0] > f64::from(mid), "on from {mid}: {rest:?}");
        assert_eq!(app.state.playlist_scroll.1, two);
    }

    /// A zoom (Ctrl+wheel) changes the zoom and the scroll at once; an ease running on that window ends there.
    #[test]
    fn a_zoom_is_not_eased() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 128);
        app.state.playlist_scroll.0 = 600.0;
        app.paint(&mut c, None);
        app.wheel(-1.0, 0.0, false, false);
        for _ in 0..3 {
            app.ease_step(10.0);
        }
        assert!(app.smooth.of(Axis::Bar(Scroll::PlaylistX)).is_some(), "a sideways notch eases");
        let (bar_w, scroll) = (app.state.playlist_bar_w, app.state.playlist_scroll.0);
        app.wheel(0.0, 1.0, false, true);
        assert!(app.state.playlist_bar_w > bar_w, "Ctrl+wheel zooms in");
        assert_ne!(app.state.playlist_scroll.0, scroll, "the zoom keeps the time under the mouse");
        assert!(!app.smooth.running(), "nothing eases after the zoom");
        let zoomed = app.state.playlist_scroll;
        app.ease_step(10.0);
        assert_eq!(app.state.playlist_scroll, zoomed);
    }

    /// The state holds what is drawn: during an ease the track header under the mouse is the one drawn there, not
    /// the one the ease is taking there. A press stops the ease where it is; a press on a scroll arrow doesn't.
    #[test]
    fn hits_follow_the_drawn_view() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 128);
        app.wheel(0.0, -1.0, false, false);
        for _ in 0..5 {
            app.ease_step(10.0);
        }
        app.paint(&mut c, None);
        let g = app.pl_grid().expect("playlist shown");
        let drawn = PGeom::new(&app.state, g);
        let target = app.smooth.of(Axis::Bar(Scroll::PlaylistY)).expect("eases").target;
        let at = g.y + 5.0;
        let t = drawn.track_at(at);
        let head = app.hits.iter().find(|h| matches!(h.action, Action::PlHeader(_))).expect("track headers").rect;
        let mut later = PGeom::new(&app.state, g);
        later.scroll_y = target as f32;
        assert_ne!(later.track_at(at), t, "the ease is half way");
        let hit = app.top_hit(head.x + 6.0, at).map(|h| h.action.clone());
        assert_eq!(hit, Some(Action::PlHeader(t)), "the header drawn at {at}");
        let r = app.hits.iter().find(|h| h.action == Action::PlHeader(t + 1)).expect("the next header").rect;
        assert_eq!(r.y, drawn.y_of(t + 1), "where it is drawn");

        // A press (on that header): the view stays where it is drawn.
        let y = app.state.playlist_scroll.1;
        app.mouse_move(head.x + 6.0, at);
        app.mouse_down();
        app.mouse_up();
        assert!(!app.smooth.running());
        assert_eq!(app.state.playlist_scroll.1, y);
        app.ease_step(10.0);
        assert_eq!(app.state.playlist_scroll.1, y);
    }

    /// A scroll arrow pressed during its ease steps on from the target: two quick presses go two steps.
    #[test]
    fn scroll_arrows_step_on_from_the_target() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.paint(&mut c, None);
        set_slider(&SMOOTHING, 128);
        let t = Scroll::PianoY;
        let v0 = app.scroll_get(t);
        let r = app.hits.iter().find(|h| h.action == Action::ScrollStep(t, 1)).expect("the down arrow").rect;
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        let one = app.smooth.of(Axis::Bar(t)).expect("eases").target;
        app.ease_step(10.0);
        app.paint(&mut c, None);
        app.mouse_down();
        app.mouse_up();
        let two = app.smooth.of(Axis::Bar(t)).expect("still easing").target;
        assert_eq!(two - one, one - f64::from(v0), "a second step on");
        app.ease_to_end();
        assert_eq!(f64::from(app.scroll_get(t)), two);
    }

    /// The ease goes where the wheel's notch goes at the scrolling speed (General ▸ Input, `crate::scroll_speed`):
    /// at 512 a vertical notch eases two track heights down.
    #[test]
    fn the_ease_goes_as_far_as_the_scrolling_speed() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 128);
        set_slider(&prefs::VERTICAL_SCROLLING_SPEED, 512);
        let row = app.state.playlist.row_h;
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(app.smooth.of(Axis::Bar(Scroll::PlaylistY)).expect("eases").target, f64::from(2.0 * row));
        app.ease_to_end();
        assert_eq!(app.state.playlist_scroll.1, 2.0 * row);
        set_slider(&prefs::VERTICAL_SCROLLING_SPEED, 256);
    }

    /// General ▸ Display ▸ Smoothing is a live slider: the wheel over it moves it by 16 a notch, a click at its left
    /// end sets 0.
    #[test]
    fn the_smoothing_slider_works() {
        use crate::views::settings::{SAct, GENERAL};
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(GENERAL);
        app.paint(&mut c, None);
        let (r, x) = app
            .hits
            .iter()
            .rev()
            .find_map(|h| match h.action {
                Action::Settings(SAct::Slider("Smoothing", x, _)) => Some((h.rect, x)),
                _ => None,
            })
            .expect("Smoothing is a live slider");
        assert_eq!(smoothing(), 128.0, "128 by default");
        app.mouse_move(r.cx(), r.cy());
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(smoothing(), 144.0);
        app.mouse_move(x - 5.0, r.cy());
        app.mouse_down();
        app.mouse_up();
        assert_eq!(smoothing(), 0.0);
    }

    /// An axis moved by something else during its ease (a drag, a jump) ends the ease there.
    #[test]
    fn a_jump_ends_the_ease() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = playlist_app(&mut c);
        set_slider(&SMOOTHING, 128);
        app.wheel(-1.0, 0.0, false, false);
        app.ease_step(10.0);
        app.state.playlist_scroll.0 = 500.0;
        app.ease_step(10.0);
        assert!(!app.smooth.running());
        assert_eq!(app.state.playlist_scroll.0, 500.0);
    }
}
