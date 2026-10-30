// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! VIEW ▸ Arrange windows ▸ (spec: ui/menu-submenus.md §2): Realign and the layouts move and resize the Playlist,
//! Mixer, Piano roll and Channel rack, whether they are shown or not (which windows show does not change).
//! - Realign (Shift+Ctrl+H) and Desktop (default): the layout of a new install, the three windows maximized and the
//!   rack where it first opens.
//! - Desktop (alternate) (Lemon Studio's own layout; the reference's rects are unknown): the three float, the
//!   Playlist over the top 55 % of the workspace, the Piano roll under it on the left 60 % and the Mixer on the right.
//! - Save custom layout keeps the current one in the settings (`window_layout`); Custom, greyed until then, brings it
//!   back, moved into the workspace if the screen changed.
//! - Tablet (default) and (alternate) stay greyed: they move the toolbar to the window's bottom, which Lemon Studio
//!   can't do.
//!
//! Window moves are not project edits: no undo step.

use os_ui::Rect;

use crate::app::{App, State, Win, WinState};
use crate::playlist::{Cmd, PItem};
use crate::views;

/// An entry of Arrange windows ▸.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Realign,
    /// Into workspace (also VIEW's own entry: `App::move_into_workspace`).
    Into,
    Default,
    Alternate,
    Custom,
    SaveCustom,
}

/// Settings key of the saved custom layout: per window (Playlist, Mixer, Piano roll) "x y w h maxed mini", then the
/// rack's "x y w h", separated by ";".
const KEY: &str = "window_layout";

/// The windows of a new install: Playlist, Mixer, Piano roll (`State::wins` order), all maximized.
pub fn default_wins() -> [WinState; 3] {
    [
        WinState { rect: Rect::new(693.0, 101.0, 1126.0, 378.0), maxed: true, mini: false },
        WinState { rect: Rect::new(231.0, 565.0, 1552.0, 435.0), maxed: true, mini: false },
        WinState { rect: Rect::new(480.0, 140.0, 1000.0, 640.0), maxed: true, mini: false },
    ]
}

/// Where the Channel rack first opens.
pub const RACK_POS: (f32, f32) = (317.0, 195.0);

/// The submenu's rows, in the reference's order.
pub fn items(_: &State) -> Vec<PItem> {
    let custom = PItem::new("Custom", Cmd::Arrange(Layout::Custom));
    vec![
        PItem::new("Realign", Cmd::Arrange(Layout::Realign)).key("Shift+Ctrl+H"),
        PItem::new(views::menus::INTO_WORKSPACE_LABEL, Cmd::Arrange(Layout::Into)),
        PItem::head("Layouts"),
        PItem::new("Desktop (default)", Cmd::Arrange(Layout::Default)),
        PItem::new("Desktop (alternate)", Cmd::Arrange(Layout::Alternate)),
        PItem::new("Tablet (default)", Cmd::Nothing).off(),
        PItem::new("Tablet (alternate)", Cmd::Nothing).off(),
        if saved().is_some() { custom } else { custom.off() },
        PItem::new("Save custom layout", Cmd::Arrange(Layout::SaveCustom)),
    ]
}

/// The saved custom layout: the three windows and the rack's rect.
fn saved() -> Option<([WinState; 3], Rect)> {
    let text = crate::prefs::text(KEY)?;
    let parts: Vec<Vec<f32>> = text.split(';').map(|p| p.split_whitespace().filter_map(|v| v.parse().ok()).collect()).collect();
    let win = |v: &Vec<f32>| (v.len() == 6).then(|| WinState { rect: Rect::new(v[0], v[1], v[2], v[3]), maxed: v[4] != 0.0, mini: v[5] != 0.0 });
    match parts.as_slice() {
        [a, b, c, r] if r.len() == 4 => Some(([win(a)?, win(b)?, win(c)?], Rect::new(r[0], r[1], r[2], r[3]))),
        _ => None,
    }
}

impl App {
    /// Runs an entry of Arrange windows ▸.
    pub(crate) fn arrange(&mut self, l: Layout) {
        let s = &mut self.state;
        match l {
            Layout::Realign | Layout::Default => {
                s.wins = default_wins();
                s.rack_pos = RACK_POS;
                s.rack_size = views::rack::DEFAULT_SIZE;
            }
            Layout::Into => return self.move_into_workspace(),
            Layout::Alternate => {
                let w = s.work_rect();
                let top = (w.h * 0.55).round();
                let left = (w.w * 0.6).round();
                let float = |x: f32, y: f32, ww: f32, hh: f32| WinState { rect: Rect::new(x, y, ww, hh), maxed: false, mini: false };
                s.wins = [float(w.x, w.y, w.w, top), float(w.x + left, w.y + top, w.w - left, w.h - top), float(w.x, w.y + top, left, w.h - top)];
                s.rack_pos = RACK_POS;
            }
            Layout::Custom => {
                let Some((wins, rack)) = saved() else { return };
                s.wins = wins;
                s.rack_pos = (rack.x, rack.y);
                s.rack_size = (rack.w, rack.h);
                // A layout saved on a larger screen comes back inside this one.
                self.move_into_workspace();
            }
            Layout::SaveCustom => {
                let win = |w: &WinState| format!("{} {} {} {} {} {}", w.rect.x, w.rect.y, w.rect.w, w.rect.h, u8::from(w.maxed), u8::from(w.mini));
                let mut parts: Vec<String> = s.wins.iter().map(win).collect();
                parts.push(format!("{} {} {} {}", s.rack_pos.0, s.rack_pos.1, s.rack_size.0, s.rack_size.1));
                crate::prefs::set_text(KEY, &parts.join(";"));
                s.message = Some("Custom layout saved".into());
            }
        }
        self.invalidate();
    }
}

/// Where the channel window first opens: right of the Channel rack, level with its top.
pub fn channel_home(s: &State) -> (f32, f32) {
    let r = views::rack::rect(s);
    ((r.right() + 8.0).min(1920.0 - 440.0), r.y.min(1080.0 - 210.0))
}

impl App {
    /// VIEW ▸ Align all channel editors (Shift+F12; Lemon Studio's own design, the reference's result is unknown):
    /// the channel window goes back to where it first opens, right of the Channel rack, and the docked plugin windows
    /// of rack channels cascade from there, back to front, each a title bar lower and to the right so every title
    /// shows. Mixer slot windows and detached windows stay where they are.
    pub(crate) fn align_channel_editors(&mut self) {
        let s = &mut self.state;
        let (x, y) = channel_home(s);
        s.plugin_pos = (x, y);
        s.plugin_moved = false;
        #[cfg(all(plugin_windows, windows))]
        {
            let first = usize::from(s.shown(Win::Plugin));
            let k = self.plugins.scale;
            let (ox, oy) = self.canvas_off;
            let rows: Vec<usize> = s.z.iter().filter_map(|w| if let Win::Hosted(r) = *w { Some(r) } else { None }).filter(|&r| r < crate::plugins::FX_KEY).collect();
            for (i, r) in rows.into_iter().enumerate() {
                let d = (first + i) as f32 * crate::app::TITLE_H;
                self.plugins.move_docked(r, ((x + d) * k).round() as i32 + ox, ((y + d) * k).round() as i32 + oy);
            }
        }
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Desktop (alternate) floats the three windows over the workspace without overlap; Save custom layout enables
    /// Custom, which brings that layout back; Realign restores the new-install layout.
    #[test]
    fn arrange_windows_layouts() {
        let mut app = App::new(None);
        let enabled = |label: &str| items(&App::new(None).state).iter().find(|i| i.label == label).unwrap().enabled;
        assert!(!enabled("Custom") && !enabled("Tablet (default)") && enabled("Desktop (alternate)"));
        app.arrange(Layout::Alternate);
        let s = &app.state;
        let work = s.work_rect();
        let [pl, mx, pr] = [Win::Playlist, Win::Mixer, Win::PianoRoll].map(|w| s.win_rect(w));
        assert!(s.wins.iter().all(|w| !w.maxed && !w.mini));
        assert_eq!((pl.x, pl.y, pl.w), (work.x, work.y, work.w));
        assert_eq!((pr.y, pr.right(), mx.x, mx.right()), (pl.bottom(), mx.x, pr.right(), work.right()));
        assert_eq!(mx.bottom(), work.bottom());
        app.state.wins[0].rect.h -= 40.0;
        app.state.rack_pos = (400.0, 300.0);
        let kept = app.state.wins[0].rect;
        app.arrange(Layout::SaveCustom);
        assert!(enabled("Custom"), "saved in the settings");
        app.arrange(Layout::Realign);
        assert!(app.state.wins.iter().all(|w| w.maxed));
        assert_eq!(app.state.wins[0].rect, default_wins()[0].rect);
        assert_eq!(app.state.rack_pos, RACK_POS);
        app.arrange(Layout::Custom);
        assert_eq!(app.state.wins[0].rect, kept);
        assert!(!app.state.wins[0].maxed);
        assert_eq!(app.state.rack_pos, (400.0, 300.0));
    }

    /// Align all channel editors (VIEW and Shift+F12) puts a moved channel window back right of the rack.
    #[test]
    fn align_all_channel_editors() {
        let mut app = App::new(None);
        app.state.raise(Win::Plugin);
        let home = channel_home(&app.state);
        app.state.plugin_pos = (900.0, 700.0);
        app.state.plugin_moved = true;
        let at = crate::views::menus::MENUS[crate::views::menus::VIEW].cols.iter().flat_map(|(_, items)| items.iter()).position(|it| it.label == crate::views::menus::ALIGN_LABEL).unwrap();
        app.menu_item(crate::views::menus::VIEW, at);
        assert_eq!(app.state.plugin_pos, home);
        assert!(!app.state.plugin_moved && app.state.shown(Win::Plugin));
    }
}
