// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The song position slider in the toolbar's main panel (spec: ui/song-position-slider.md): where the song position
//! is within the song (SONG mode) or the pattern (PAT mode), and a way to move it.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use os_ui::Rect;

use crate::app::{App, Drag, State, HINT_RECT};

/// The panel (input area), main-window coordinates (spec: ui/main-window.md §3).
pub const PANEL: Rect = Rect::new(381.0, 45.0, 194.0, 32.0);
/// The handle's centre runs from `X0` (position 0) to `X0 + W` (the end of the range) (§1).
pub const X0: f32 = 395.0;
pub const W: f32 = 170.0;
/// Half the handle's width: a press this close to its centre grabs it.
pub const GRAB: f32 = 6.5;
/// Holding the button on the track: the first repeat, then the repeat interval (§4).
const FIRST_REPEAT: Duration = Duration::from_millis(250);
const REPEAT: Duration = Duration::from_millis(60);

/// The range in ticks: the song's length (the end of the last clip) in SONG mode, `None` while the playlist is empty
/// (the slider is disabled); the current pattern's length in PAT mode (§2).
pub fn range(s: &State) -> Option<u32> {
    if s.song_mode {
        crate::playlist::clips(s).iter().map(|c| c.end()).max().filter(|&e| e > 0)
    } else {
        Some(crate::piano::pattern_length(s, s.pattern).max(1))
    }
}

/// The position the slider moves: the song position, or the piano roll's play-start marker in PAT mode.
pub fn stored(s: &State) -> u32 {
    if s.song_mode { s.playlist.pos } else { s.piano.marker }
}

/// Where the handle is: the playhead while this mode plays (and the handle is not being dragged), else the stored
/// position.
pub fn shown(s: &State, playing: bool, song: bool, tick: f64, dragging: bool) -> u32 {
    if playing && song == s.song_mode && !dragging { tick.max(0.0) as u32 } else { stored(s) }
}

pub fn handle_x(pos: u32, range: u32) -> f32 {
    X0 + W * pos.min(range) as f32 / range.max(1) as f32
}

/// The position under `x`, clamped to 0 … range − 1 tick.
pub fn tick_at(x: f32, range: u32) -> u32 {
    let t = ((x - X0) / W * range as f32).round().max(0.0) as u32;
    t.min(range.saturating_sub(1))
}

/// A click beside the handle: range / 88 in whole half steps (PPQ / 8 ticks), at least one (§4).
pub fn click_step(range: u32, ppq: u16) -> u32 {
    let half = (u32::from(ppq) / 8).max(1);
    ((f64::from(range) / 88.0 / f64::from(half)).round() as u32).max(1) * half
}

/// One wheel notch: 3 / 64 of the range, rounded (§3).
pub fn wheel_step(range: u32) -> u32 {
    ((u64::from(range) * 3 + 32) / 64).max(1) as u32
}

/// "Song position: B:S:T" in the time panel's format.
pub fn hint(s: &State, pos: u32) -> String {
    let (bar, step, tick) = crate::views::toolbar::bst_ticks(f64::from(pos) * 96.0 / f64::from(s.project.ppq.max(1)));
    format!("Song position: {bar}:{step:02}:{tick:02}")
}

impl App {
    /// The position the slider's input starts from (the playhead while this mode plays).
    fn song_pos_now(&self) -> u32 {
        let t = &self.transport;
        let dragging = matches!(self.drag, Some(Drag::SongPos { .. }));
        shown(&self.state, t.playing(), t.song(), t.tick(), dragging)
    }

    /// Move the position to `t` (clamped to the range); `restart` = playback jumps there now (not during a drag).
    pub(crate) fn set_song_pos(&mut self, t: u32, restart: bool) {
        let Some(range) = range(&self.state) else { return };
        let t = t.min(range - 1);
        let s = &mut self.state;
        if s.song_mode {
            s.playlist.pos = t;
            // The initial values follow a song position move (spec: ui/settings-choices.md "Advanced: Initialize
            // controls automatically").
            crate::init::apply(s);
        } else {
            s.piano.marker = t;
        }
        self.hint = Cow::Owned(hint(&self.state, t));
        self.damage_rect(HINT_RECT);
        if restart {
            self.song_pos_restart();
        }
        self.invalidate();
    }

    /// Playback follows a moved position, as after a click in the playlist ruler (spec: ui/playlist.md §7): the song
    /// plays on from there (a paused pattern stops); a playing pattern starts again from its new position.
    fn song_pos_restart(&mut self) {
        let tp = &self.transport;
        if !tp.playing() {
            return;
        }
        if self.state.song_mode {
            if tp.song() || !tp.paused() {
                let p = self.state.playlist.pos;
                self.play_song_from(p);
            } else {
                tp.stop();
            }
        } else if !tp.song() && !tp.paused() {
            self.transport.stop();
            self.start_playback();
        }
    }

    /// Left button down on the slider: on the handle it drags, beside it one step toward the pointer (repeated while
    /// held, see `song_pos_tick`).
    pub(crate) fn song_pos_press(&mut self) {
        let Some(range) = range(&self.state) else { return };
        // The slider is laid out in the top band's px (Toolbars scaling).
        let x = self.band_mouse().0;
        let hx = handle_x(self.song_pos_now(), range);
        if (x - hx).abs() <= GRAB {
            self.drag = Some(Drag::SongPos { dx: x - hx });
        } else {
            self.song_pos_step();
            self.drag = Some(Drag::SongPosHold { next: Instant::now() + FIRST_REPEAT });
        }
    }

    /// One step toward the pointer, stopping under it.
    fn song_pos_step(&mut self) {
        let Some(range) = range(&self.state) else { return };
        let now = self.song_pos_now();
        let target = tick_at(self.band_mouse().0, range);
        let step = click_step(range, self.state.project.ppq);
        let next = if target > now { (now + step).min(target) } else { now.saturating_sub(step).max(target) };
        if next != now {
            self.set_song_pos(next, true);
        }
    }

    /// Dragging the handle: the position follows the pointer (minus the grab offset).
    pub(crate) fn song_pos_drag(&mut self, dx: f32) {
        if let Some(range) = range(&self.state) {
            self.set_song_pos(tick_at(self.band_mouse().0 - dx, range), false);
        }
    }

    /// The handle let go: playback follows.
    pub(crate) fn song_pos_release(&mut self) {
        self.song_pos_restart();
    }

    /// Right click on the slider: straight to the pointer (§3).
    pub(crate) fn song_pos_jump(&mut self) {
        if let Some(range) = range(&self.state) {
            self.set_song_pos(tick_at(self.band_mouse().0, range), true);
        }
    }

    /// The wheel over the slider: down = later, 3 / 64 of the range per notch (§3).
    pub(crate) fn song_pos_wheel(&mut self, dy: f32) {
        let Some(range) = range(&self.state) else { return };
        if dy == 0.0 {
            return;
        }
        let notches = (-dy).round() as i64;
        let notches = if notches == 0 { -(dy.signum() as i64) } else { notches };
        let now = i64::from(self.song_pos_now());
        let t = (now + notches * i64::from(wheel_step(range))).max(0);
        self.set_song_pos(t as u32, true);
    }

    /// While the button is held on the track: the next step when it is due.
    pub(crate) fn song_pos_tick(&mut self, now: Instant) {
        let Some(Drag::SongPosHold { next }) = self.drag else { return };
        if now >= next {
            self.song_pos_step();
            self.drag = Some(Drag::SongPosHold { next: now + REPEAT });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handle's place and the steps (§1, §3, §4): measured on FL at PPQ 96.
    #[test]
    fn handle_and_steps() {
        let song = 3168; // 8.25 bars
        assert_eq!(handle_x(0, song), 395.0);
        assert_eq!(handle_x(song, song), 565.0);
        assert_eq!(handle_x(4 * 384, song).round(), 477.0, "bar 5");
        assert_eq!(click_step(3168, 96), 36);
        assert_eq!(click_step(6240, 96), 72);
        assert_eq!(click_step(384, 96), 12, "at least one half step");
        assert_eq!(wheel_step(3168), 149);
        assert_eq!(wheel_step(6240), 293);
        assert_eq!(tick_at(480.0, 6240), 3120);
        assert_eq!(tick_at(300.0, 6240), 0);
        assert_eq!(tick_at(700.0, 6240), 6239, "the last tick");
    }
}
