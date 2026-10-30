// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The windows that float over the others, in one place: which they are and, for each, its rect, what it paints, what
//! a repaint of it covers and where it is placed. Everything that treats floating windows alike (closing them, keeping
//! them in the workspace, a playlist scroll keeping their pixels, repainting them) asks here, so a new kind of floating
//! window is added once: a window left out of one hand-kept list made the playlist scroll move its pixels.
//!
//! Two kinds float: the app's own windows placed by position (`PLACED`: the channel rack, the channel window and the
//! tool windows), and the plugin windows drawn in the app's window (`Win::Hosted`, Linux). The Playlist, Mixer and
//! Piano roll float too in the classic layout, but as windows with slots (`State::wins`), not here.

use os_ui::Rect;

use crate::app::{State, Win};
use crate::views;

/// One placed window's places (`PLACED`).
pub struct Placed {
    pub win: Win,
    /// Its frame.
    pub rect: fn(&State) -> Rect,
    /// What it paints (the channel rack's graph editor hangs below its frame).
    pub paint: fn(&State) -> Rect,
    /// What a repaint of it covers (its paint area and what its frame casts around it).
    pub bounds: fn(&State) -> Rect,
    /// Its top-left.
    pub pos: fn(&State) -> (f32, f32),
    pub set_pos: fn(&mut State, (f32, f32)),
}

/// The app's own windows placed by position: the channel rack, the channel window and the tool windows.
pub const PLACED: [Placed; 9] = [
    Placed {
        win: Win::ChannelRack,
        rect: views::rack::rect,
        paint: views::rack::paint_area,
        bounds: views::rack::bounds,
        pos: |s| s.rack_pos,
        set_pos: |s, p| s.rack_pos = p,
    },
    Placed {
        win: Win::Plugin,
        rect: views::channel_win::rect,
        paint: views::channel_win::rect,
        bounds: views::channel_win::bounds,
        pos: |s| s.plugin_pos,
        set_pos: |s, p| s.plugin_pos = p,
    },
    Placed { win: Win::Perf, rect: views::perf::rect, paint: views::perf::bounds, bounds: views::perf::bounds, pos: |s| s.perf.pos, set_pos: |s, p| s.perf.pos = p },
    Placed { win: Win::Tapper, rect: views::tapper::rect, paint: views::tapper::bounds, bounds: views::tapper::bounds, pos: |s| s.tapper.pos, set_pos: |s, p| s.tapper.pos = p },
    Placed { win: Win::Touch, rect: views::touch::rect, paint: views::touch::bounds, bounds: views::touch::bounds, pos: |s| s.touch.pos, set_pos: |s, p| s.touch.pos = p },
    Placed { win: Win::Strum, rect: views::strum::rect, paint: views::strum::bounds, bounds: views::strum::bounds, pos: |s| s.strum.pos, set_pos: |s, p| s.strum.pos = p },
    Placed { win: Win::Arp, rect: views::arpeggiate::rect, paint: views::arpeggiate::bounds, bounds: views::arpeggiate::bounds, pos: |s| s.arp.pos, set_pos: |s, p| s.arp.pos = p },
    Placed { win: Win::Events, rect: views::events::rect, paint: views::events::bounds, bounds: views::events::bounds, pos: |s| s.events.pos, set_pos: |s, p| s.events.pos = p },
    Placed {
        win: Win::PianoDlg,
        rect: views::piano_dlg::rect,
        paint: views::piano_dlg::bounds,
        bounds: views::piano_dlg::bounds,
        pos: |s| s.piano_dlg.pos[s.piano_dlg.tool.index()],
        set_pos: |s, p| s.piano_dlg.pos[s.piano_dlg.tool.index()] = p,
    },
];

/// `w`'s entry in `PLACED`.
pub fn placed(w: Win) -> Option<&'static Placed> {
    PLACED.iter().find(|p| p.win == w)
}

/// A tool window (the monitor, the tempo tapper, the touch controller, the piano roll's tool windows): placed, and
/// neither the channel rack nor the channel window.
pub fn is_tool(w: Win) -> bool {
    placed(w).is_some() && !matches!(w, Win::ChannelRack | Win::Plugin)
}

/// `w` floats over the windows with slots: a placed window, or a plugin window drawn in the app's.
pub fn floats(w: Win) -> bool {
    placed(w).is_some() || matches!(w, Win::Hosted(_))
}

/// The frame of floating window `w`, when it is one (a plugin window drawn in the app's: while it is open).
pub fn rect(s: &State, w: Win) -> Option<Rect> {
    match w {
        #[cfg(target_os = "linux")]
        Win::Hosted(row) => s.embeds.get(&row).map(|e| e.rect()),
        w => placed(w).map(|p| (p.rect)(s)),
    }
}

/// What floating window `w` paints.
pub fn paint_area(s: &State, w: Win) -> Option<Rect> {
    match w {
        #[cfg(target_os = "linux")]
        Win::Hosted(_) => rect(s, w),
        w => placed(w).map(|p| (p.paint)(s)),
    }
}

/// What a repaint of floating window `w` covers.
pub fn bounds(s: &State, w: Win) -> Option<Rect> {
    match w {
        #[cfg(target_os = "linux")]
        Win::Hosted(_) => rect(s, w),
        w => placed(w).map(|p| (p.bounds)(s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every placed window is listed once, and its position moves its frame.
    #[test]
    fn placed_windows_move_with_their_position() {
        let mut s = crate::app::App::new(None).state;
        for (i, p) in PLACED.iter().enumerate() {
            assert!(PLACED.iter().skip(i + 1).all(|q| q.win != p.win), "{:?} listed twice", p.win);
            let before = (p.rect)(&s);
            let (x, y) = (p.pos)(&s);
            (p.set_pos)(&mut s, (x + 30.0, y + 20.0));
            let after = (p.rect)(&s);
            assert_eq!((after.x - before.x, after.y - before.y), (30.0, 20.0), "{:?}", p.win);
            assert!(floats(p.win));
        }
        assert!(is_tool(Win::Tapper) && !is_tool(Win::ChannelRack) && !is_tool(Win::Plugin) && !is_tool(Win::Mixer));
        assert!(floats(Win::Hosted(3)) && !floats(Win::Playlist));
    }
}
