// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! General ▸ Input's scrolling speeds (`prefs::VERTICAL_SCROLLING_SPEED`, `prefs::HORIZONTAL_SCROLLING_SPEED`): the
//! editors (the playlist, the piano roll, the channel rack, the mixer and their scroll bars) scroll by the vertical
//! wheel's steps times the vertical speed / 256 and by the sideways wheel's (a tilt wheel, a touchpad's sideways swipe)
//! times the horizontal speed / 256 (`App::wheel_act`). At the default 256 the steps are as they were; at 0 that wheel
//! doesn't scroll. Shift+wheel keeps its own step; zooms, value boxes and lists take the wheel's steps as they come.
// spec: ui/settings-choices.md "Input: Vertical and Horizontal scrolling speed"

use crate::prefs::{self, Slider, HORIZONTAL_SCROLLING_SPEED, VERTICAL_SCROLLING_SPEED};

/// The wheel's steps (`dx` sideways, `dy` vertical) at the scrolling speeds.
pub fn scaled(dx: f32, dy: f32) -> (f32, f32) {
    let k = |s: &Slider| f32::from(prefs::slider(s)) / 256.0;
    (dx * k(&HORIZONTAL_SCROLLING_SPEED), dy * k(&VERTICAL_SCROLLING_SPEED))
}

#[cfg(test)]
mod tests {
    use crate::app::{Action, App, Win};
    use crate::prefs::{set_slider, HORIZONTAL_SCROLLING_SPEED, SMOOTHING, VERTICAL_SCROLLING_SPEED};
    use os_ui::Canvas;

    fn speeds(v: u16, h: u16) {
        set_slider(&VERTICAL_SCROLLING_SPEED, v);
        set_slider(&HORIZONTAL_SCROLLING_SPEED, h);
    }

    /// The playlist in front, the mouse over an empty spot of its grid.
    fn playlist(c: &mut Canvas) -> App {
        // Notches at once (General ▸ Display ▸ Smoothing 0): these tests measure the steps, `crate::smooth` the ease.
        set_slider(&SMOOTHING, 0);
        let mut app = App::new(None);
        app.state.z = vec![Win::Playlist];
        app.paint(c, None);
        let g = app.pl_grid().expect("the playlist grid");
        let at = (0..40)
            .flat_map(|i| (0..10).map(move |j| (g.x + 40.0 + i as f32 * 30.0, g.y + 40.0 + j as f32 * 40.0)))
            .find(|&(x, y)| app.top_hit(x, y).is_some_and(|h| h.action == Action::PlaylistArea))
            .expect("an empty spot of the grid");
        app.mouse_move(at.0, at.1);
        app
    }

    /// One notch from the top-left of the playlist: how far it scrolled (sideways, down).
    fn pl_notch(app: &mut App, dx: f32, dy: f32, shift: bool) -> (f32, f32) {
        app.state.playlist_scroll = (0.0, 0.0);
        app.wheel(dx, dy, shift, false);
        app.state.playlist_scroll
    }

    // spec: ui/settings-choices.md "Input: Vertical and Horizontal scrolling speed (2026-10-16)"
    /// The playlist scrolls by the wheel's steps times speed / 256: a track height a vertical notch at the default,
    /// twice that at 512, nothing at 0; the sideways wheel the same with the horizontal speed.
    #[test]
    fn the_playlist_scrolls_at_the_speeds() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = playlist(&mut c);
        let row = app.state.playlist.row_h;
        let step = pl_notch(&mut app, -1.0, 0.0, false).0;
        assert!(step > 0.0, "the sideways wheel scrolls");
        for (v, k) in [(0, 0.0), (256, 1.0), (512, 2.0), (128, 0.5)] {
            speeds(v, 256);
            assert_eq!(pl_notch(&mut app, 0.0, -1.0, false), (0.0, row * k), "vertical at {v}");
            assert_eq!(pl_notch(&mut app, -1.0, 0.0, false), (step, 0.0), "the horizontal speed stays 256");
            speeds(256, v);
            assert_eq!(pl_notch(&mut app, -1.0, 0.0, false), (step * k, 0.0), "horizontal at {v}");
            assert_eq!(pl_notch(&mut app, 0.0, -1.0, false), (0.0, row), "the vertical speed stays 256");
        }
        speeds(256, 256);
    }

    /// Shift+wheel (sideways) keeps its own step whatever the speeds; Ctrl+wheel zooms as at the default.
    #[test]
    fn shift_wheel_and_zoom_ignore_the_speeds() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = playlist(&mut c);
        speeds(256, 256);
        let shifted = pl_notch(&mut app, 0.0, -1.0, true);
        assert!(shifted.0 > 0.0 && shifted.1 == 0.0, "Shift+wheel scrolls sideways");
        let zoom = |app: &mut App| {
            let w0 = app.state.playlist_bar_w;
            app.wheel(0.0, 1.0, false, true);
            let w = app.state.playlist_bar_w;
            app.state.playlist_bar_w = w0;
            w
        };
        let zoomed = zoom(&mut app);
        assert!(zoomed > app.state.playlist_bar_w, "Ctrl+wheel zooms in");
        let mut piano = piano(&mut c);
        let (pz, pshift) = piano_zoom_and_shift(&mut piano);
        assert!(pz != piano.state.piano.px_per_beat && pshift > 0.0, "the piano roll zooms and scrolls: {pz} {pshift}");
        for (v, h) in [(0, 0), (512, 512), (0, 512)] {
            speeds(v, h);
            assert_eq!(pl_notch(&mut app, 0.0, -1.0, true), shifted, "Shift+wheel at {v} / {h}");
            assert_eq!(zoom(&mut app), zoomed, "Ctrl+wheel at {v} / {h}");
            assert_eq!(piano_zoom_and_shift(&mut piano), (pz, pshift), "the piano roll at {v} / {h}");
        }
        speeds(256, 256);
    }

    /// The piano roll in front, the mouse over its grid, scrolled to the middle of the keys.
    fn piano(c: &mut Canvas) -> App {
        set_slider(&SMOOTHING, 0);
        let mut app = App::new(None);
        app.state.z = vec![Win::PianoRoll];
        app.paint(c, None);
        let g = app.piano_geom().expect("the piano roll").grid;
        let (x, y) = (g.x + g.w / 2.0, g.y + g.h / 2.0);
        assert!(app.top_hit(x, y).is_some_and(|h| matches!(h.action, Action::PianoGrid | Action::PianoNote(_))));
        app.mouse_move(x, y);
        app
    }

    /// One notch in the piano roll from the left, in the middle of the keys: how far it scrolled (ticks, px).
    fn piano_notch(app: &mut App, dx: f32, dy: f32, shift: bool) -> (f32, f32) {
        app.state.piano.scroll_x = 0.0;
        app.state.piano.scroll_y = 1200.0;
        app.wheel(dx, dy, shift, false);
        (app.state.piano.scroll_x, app.state.piano.scroll_y - 1200.0)
    }

    /// Ctrl+wheel's new time zoom and Shift+wheel's sideways scroll in the piano roll.
    fn piano_zoom_and_shift(app: &mut App) -> (f32, f32) {
        let z0 = app.state.piano.px_per_beat;
        app.wheel(0.0, 1.0, false, true);
        let z = app.state.piano.px_per_beat;
        app.state.piano.px_per_beat = z0;
        (z, piano_notch(app, 0.0, -1.0, true).0)
    }

    /// The piano roll scrolls by the wheel's steps times speed / 256: three key rows a vertical notch at the default,
    /// six at 512, none at 0; the sideways wheel the same with the horizontal speed.
    #[test]
    fn the_piano_roll_scrolls_at_the_speeds() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = piano(&mut c);
        speeds(256, 256);
        let row = app.state.piano.row_h;
        let step = piano_notch(&mut app, -1.0, 0.0, false).0;
        assert!(step > 0.0, "the sideways wheel scrolls");
        assert_eq!(piano_notch(&mut app, 0.0, -1.0, false), (0.0, 3.0 * row), "three rows a notch at the default");
        for (v, k) in [(0, 0.0), (256, 1.0), (512, 2.0)] {
            speeds(v, 256);
            assert_eq!(piano_notch(&mut app, 0.0, -1.0, false), (0.0, 3.0 * row * k), "vertical at {v}");
            assert_eq!(piano_notch(&mut app, 0.0, 1.0, false), (0.0, -3.0 * row * k), "vertical up at {v}");
            speeds(256, v);
            assert_eq!(piano_notch(&mut app, -1.0, 0.0, false), (step * k, 0.0), "horizontal at {v}");
        }
        speeds(256, 256);
    }
}
