// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The display's pixel density: General ▸ Display's PPI ruler.
//!
//! A program setting from 72 to 400 PPI. Until the user sets it, it follows the display: 96 times the display's own
//! scale. The ruler (`views::settings`) is a bar whose filled part is N device pixels long: hold a ruler against the
//! screen and drag until it is one inch long. A drag moves its end at half the mouse's speed, the wheel steps 1 PPI.
//! Nothing else uses the value yet; it is meant for anything drawn at a real-world size later.
// spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility (2026-10-16)"

use crate::app::{App, Drag};

/// Where the user's value is kept (`crate::prefs` free text); unset while it follows the display.
pub const KEY: &str = "display_ppi";
/// The lowest PPI.
pub const MIN: u16 = 72;
/// The highest PPI: the ruler's track is as long as this many device pixels.
pub const MAX: u16 = 400;
/// The PPI of a display at 100 % scale.
const BASE: f32 = 96.0;

/// `v` rounded and brought into `MIN..=MAX` (not a number: `MIN`).
fn clamp(v: f32) -> u16 {
    v.round().max(f32::from(MIN)).min(f32::from(MAX)) as u16
}

/// The PPI of a display at `scale` (its device pixels per logical one) while the user hasn't set one: 96 times it,
/// rounded, in range.
pub fn default_for(scale: f32) -> u16 {
    clamp(BASE * scale)
}

/// The value the user set (in range), or `None` while it follows the display (also when what is kept can't be read).
pub fn stored() -> Option<u16> {
    let v = crate::prefs::text(KEY)?.trim().parse::<i64>().ok()?;
    Some(v.clamp(i64::from(MIN), i64::from(MAX)) as u16)
}

/// The PPI on a display at `display_scale`: the user's, else the display's own.
pub fn ppi(display_scale: f32) -> u16 {
    stored().unwrap_or_else(|| default_for(display_scale))
}

/// Keeps `v` (brought into range) as the user's PPI: from now on it no longer follows the display.
pub fn set(v: u16) {
    crate::prefs::set_text(KEY, &v.clamp(MIN, MAX).to_string());
}

/// The ruler's second line: "(N PPI/advised scaling: S%)", S being the display's own scale.
pub fn label(n: u16, display_scale: f32) -> String {
    format!("({n} PPI/advised scaling: {}%)", (display_scale * 100.0).round() as i64)
}

/// The PPI after the mouse moved `dx` logical pixels from where the ruler was pressed (at `n0`) on a canvas at
/// `ui_scale`: the bar's end moves at half the mouse's speed, so N changes by half the device pixels moved.
pub fn dragged(n0: u16, dx: f32, ui_scale: f32) -> u16 {
    clamp(f32::from(n0) + dx * ui_scale / 2.0)
}

/// `n` after `notches` wheel notches (up for more), 1 PPI each, in range.
pub fn stepped(n: u16, notches: i32) -> u16 {
    (i64::from(n) + i64::from(notches)).clamp(i64::from(MIN), i64::from(MAX)) as u16
}

/// The settings window's side of the ruler.
impl App {
    /// The ruler was pressed (`SAct::Ppi`): a drag from here moves its end (`Drag::Ppi`); the press alone changes
    /// nothing.
    pub(crate) fn ppi_press(&mut self) {
        self.drag = Some(Drag::Ppi { x0: self.mouse.0, n0: ppi(self.state.display_scale) });
        self.invalidate();
    }

    /// Shows and keeps `n` as the PPI, if it is not what the ruler shows already (so a drag back to where it began
    /// leaves a PPI that follows the display as it was).
    pub(crate) fn ppi_set(&mut self, n: u16) {
        if n != ppi(self.state.display_scale) {
            set(n);
            self.invalidate();
        }
    }

    /// Wheel notches over the ruler: 1 PPI each, up for more.
    pub(crate) fn ppi_wheel(&mut self, notches: i32) {
        self.ppi_set(stepped(ppi(self.state.display_scale), notches));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::{self, Row, SAct};
    use os_ui::{Canvas, Rect};

    // spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility (2026-10-16)"
    /// Until the user sets it, the PPI follows the display: 96 times its scale (96 at 100 %, 144 at 150 %), in
    /// 72..=400. Once set, it is kept, read back in range, and no longer follows.
    #[test]
    fn the_ppi_follows_the_display_until_set() {
        assert_eq!((default_for(1.0), default_for(1.5), default_for(1.25), default_for(2.0)), (96, 144, 120, 192));
        assert_eq!((default_for(0.5), default_for(5.0), default_for(f32::NAN)), (72, 400, 72), "in range");
        assert_eq!((stored(), ppi(1.0), ppi(1.5)), (None, 96, 144), "unset: the display's");
        set(150);
        assert_eq!((stored(), ppi(1.0), ppi(2.0)), (Some(150), 150, 150), "set: kept, whatever the display");
        set(20);
        assert_eq!(stored(), Some(72));
        set(9000);
        assert_eq!(stored(), Some(400));
        crate::prefs::set_text(KEY, "lots");
        assert_eq!((stored(), ppi(1.5)), (None, 144), "unreadable: the display's");
        crate::prefs::set_text(KEY, "1000");
        assert_eq!(stored(), Some(400), "kept out of range: its end");
        crate::prefs::set_text(KEY, "");
        assert_eq!(stored(), None);
    }

    /// The second line reads "(N PPI/advised scaling: S%)", S the display's own scale whatever N is.
    #[test]
    fn the_ppi_label() {
        assert_eq!(label(96, 1.0), "(96 PPI/advised scaling: 100%)");
        assert_eq!(label(144, 1.5), "(144 PPI/advised scaling: 150%)");
        assert_eq!(label(300, 1.0), "(300 PPI/advised scaling: 100%)");
        assert_eq!(label(72, 1.25), "(72 PPI/advised scaling: 125%)");
    }

    /// A drag moves the bar's end at half the mouse's speed, in device pixels; it stops at 72 and 400. The wheel steps
    /// 1 PPI a notch, in range.
    #[test]
    fn the_ppi_drag_and_wheel_maths() {
        assert_eq!((dragged(96, 20.0, 1.0), dragged(96, -20.0, 1.0), dragged(96, 0.0, 1.0)), (106, 86, 96));
        assert_eq!(dragged(96, 20.0, 2.0), 116, "at 200 %, 20 logical px are 40 device px");
        assert_eq!(dragged(96, 3.0, 1.0), 98, "half pixels round");
        assert_eq!((dragged(96, -100.0, 1.0), dragged(96, 1000.0, 1.0)), (72, 400));
        assert_eq!((stepped(96, 1), stepped(96, -1), stepped(96, 3)), (97, 95, 99));
        assert_eq!((stepped(400, 1), stepped(72, -1)), (400, 72));
    }

    fn ruler(app: &App) -> Rect {
        app.hits.iter().rev().find(|h| h.action == Action::Settings(SAct::Ppi)).map(|h| h.rect).expect("the ruler works")
    }

    /// The second line as the page has it.
    fn shown(app: &App) -> Option<String> {
        let rows = settings::page(settings::GENERAL, &app.state).into_iter().flat_map(|g| g.rows);
        rows.into_iter().find_map(|r| match r {
            Row::Text(v, true) if v.contains(" PPI/advised scaling: ") => Some(v),
            _ => None,
        })
    }

    fn pixel(c: &Canvas, x: f32, y: f32) -> u32 {
        let mut v = vec![0u32; 1920 * 1080];
        c.copy_to_xrgb(&mut v);
        v[y as usize * 1920 + x as usize] & 0xff_ffff
    }

    /// On the General page the ruler is live: its lines show N and the display's scale; a press changes nothing, a
    /// drag sets N at half the mouse's speed and keeps it, the wheel steps it; the filled part is N px long (+1) and
    /// grey, orange while dragged.
    #[test]
    fn the_ppi_ruler_on_the_settings_page() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.state.display_scale = 1.5;
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder("Display PPI")), "not greyed out");
        assert_eq!(shown(&app).as_deref(), Some("(144 PPI/advised scaling: 150%)"));
        app.state.display_scale = 1.0;
        assert_eq!(shown(&app).as_deref(), Some("(96 PPI/advised scaling: 100%)"));
        app.invalidate();
        app.paint(&mut c, None);
        let r = ruler(&app);
        // The track starts at the page's left margin (the 96 px filled part from there), 400 px long.
        let x0 = r.x + 4.0;
        let cy = r.y + 7.0;
        assert_eq!(r.w, 408.0, "400 px and a little room");
        assert_eq!((pixel(&c, x0 + 48.0, cy), pixel(&c, x0 + 95.5, cy)), (0xb9bfc3, 0xb9bfc3), "filled, grey at rest");
        assert_ne!(pixel(&c, x0 + 99.5, cy), 0xb9bfc3, "97 px with its rounded end");
        // A press alone keeps the PPI following the display; dragging sets it, the mouse off the bar too.
        app.mouse_move(x0 + 200.0, cy);
        app.mouse_down();
        assert_eq!(stored(), None);
        app.mouse_move(x0 + 220.0, cy + 40.0);
        assert_eq!((stored(), ppi(1.0)), (Some(106), 106));
        app.paint(&mut c, None);
        assert_eq!(pixel(&c, x0 + 48.0, cy), 0xffbc40, "orange while dragged");
        assert_eq!(shown(&app).as_deref(), Some("(106 PPI/advised scaling: 100%)"));
        app.mouse_move(x0 - 600.0, cy);
        assert_eq!(ppi(1.0), 72);
        app.mouse_move(x0 + 1600.0, cy);
        assert_eq!(ppi(1.0), 400);
        app.mouse_move(x0 + 300.0, cy);
        app.mouse_up();
        assert_eq!(ppi(1.0), 146);
        app.mouse_move(x0 + 1000.0, cy + 200.0);
        assert_eq!(ppi(1.0), 146, "released: the mouse no longer moves it");
        app.paint(&mut c, None);
        app.paint(&mut c, None);
        assert_eq!(pixel(&c, x0 + 140.0, cy), 0xb9bfc3, "grey again");
        assert_ne!(pixel(&c, x0 + 150.0, cy), 0xb9bfc3, "146 PPI: 147 px");
        // The wheel over the bar steps 1 PPI, and the page doesn't scroll.
        let scroll = app.state.settings.scroll;
        app.mouse_move(x0 + 50.0, cy);
        app.wheel(0.0, 1.0, false, false);
        assert_eq!(ppi(1.0), 147);
        app.wheel(0.0, -3.0, false, false);
        app.wheel(0.0, -1.0, false, false);
        assert_eq!(ppi(1.0), 145, "a notch a turn");
        assert_eq!(app.state.settings.scroll, scroll);
        crate::prefs::set_text(KEY, "");
    }

    /// At 200 % the bar is as long in device pixels: 400 / 2 logical px, and a drag moves N by the device pixels.
    #[test]
    fn the_ppi_ruler_at_200_percent() {
        let Ok(mut c) = Canvas::new(3840, 2160, 2.0) else { return };
        let mut app = App::new(None);
        app.state.display_scale = 2.0;
        app.open_settings(settings::GENERAL);
        app.paint(&mut c, None);
        let r = ruler(&app);
        assert_eq!(r.w, 208.0);
        assert_eq!(ppi(2.0), 192);
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_move(r.cx() + 10.0, r.cy());
        app.mouse_up();
        assert_eq!(stored(), Some(202));
        crate::prefs::set_text(KEY, "");
    }
}
