// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! General ▸ Display ▸ High visibility: some controls drawn brighter, in both looks, at once (no restart).
//!
//! - The toolbar's record button: a white dot instead of the red one while recording isn't armed (`record_dot`).
//! - The channel rack's rows: brighter channel names and mixer track numbers (`rack_name`, `rack_number`).
//! - The rack's step sequencer / piano roll overview toggle, while off: always the outline it shows on hover
//!   (`views::rack`).
//! - Every scroll bar: a light thumb and light arrows (`os_ui::kit::scroll_thumb`, `scroll_arrow`; `publish` hands the
//!   setting to the widget kit before each paint).
//! - The playlist's horizontal scroll bar: two lighter edges at the top and bottom of its track (`PLAYLIST_EDGES`).
// spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High visibility (2026-10-16)"

use os_theme::{Color, Palette};

use crate::prefs;

/// Whether High visibility is on.
pub fn on() -> bool {
    prefs::check(&prefs::HIGH_VISIBILITY)
}

/// Hands the setting to the widget kit, whose scroll bars follow it (`os_ui::kit::high_visibility`).
pub fn publish() {
    os_ui::kit::set_high_visibility(on());
}

/// The record button's dot while recording isn't armed: red, or with `high` white (#dce7e8, measured).
pub fn record_dot(t: &Palette, high: bool) -> Color {
    if high { t.tx(0xdce7e8) } else { t.record }
}

/// A rack row's channel name text (a channel without a colour, not focused): the look's own, or with `high` brighter,
/// #ededde as measured (the Lemon look: its brightest text colour).
pub fn rack_name(t: &Palette, lemon: bool, high: bool) -> Color {
    match (high, lemon) {
        (false, _) => t.name_text,
        (true, false) => t.tx(0xededde),
        (true, true) => t.text,
    }
}

/// A rack row's mixer track number (not hovered): the look's own, or with `high` brighter, #b2a9b6 as measured (the
/// Lemon look: its brightest text colour).
pub fn rack_number(t: &Palette, lemon: bool, high: bool) -> Color {
    match (high, lemon) {
        (false, _) => t.text_dim.shade(0.08),
        (true, false) => t.tx(0xb2a9b6),
        (true, true) => t.text,
    }
}

/// The two lighter edges of the playlist's horizontal scroll bar track, outer then inner, 1 px each at its top and
/// bottom (measured).
pub const PLAYLIST_EDGES: [u32; 2] = [0x525557, 0x4b4f52];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, App};
    use crate::prefs::{check, find_check, set_check, HIGH_VISIBILITY};
    use crate::views::settings::{self, SAct};
    use os_theme::Design;

    /// The setting (spec: ui/settings-choices.md "Display: PPI ruler, Force refreshes, Thick lines, High
    /// visibility"): off by default, a program setting; its row on the Display page is live and toggles it at once.
    #[test]
    fn the_setting_and_its_row() {
        assert_eq!(find_check("High visibility").map(|c| (c.key, c.default)), Some(("high_visibility", false)));
        assert!(!check(&HIGH_VISIBILITY) && !on(), "off by default");
        let mut app = App::new(None);
        let row = Action::Settings(SAct::Check(HIGH_VISIBILITY.label));
        if let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) {
            app.open_settings(settings::GENERAL);
            app.paint(&mut c, None);
            assert!(app.hits.iter().any(|h| h.action == row), "the row is live");
            assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder("High visibility")));
        }
        app.act(row.clone());
        assert!(on());
        assert_eq!(app.state.message.as_deref(), Some("High visibility: on"));
        app.act(row);
        assert!(!on());
        set_check(&HIGH_VISIBILITY, false);
    }

    /// The colours, off and on, in both looks: the record dot white instead of red, the rack's names and track numbers
    /// brighter (#ededde, #b2a9b6; the Lemon look's brightest text), the scroll bar thumb light grey (#b9bfc3) and its
    /// arrows light (#adb3b7).
    #[test]
    fn the_colours_off_and_on() {
        let fl = Palette::default();
        assert_eq!((record_dot(&fl, false), record_dot(&fl, true)), (fl.record, Color::rgb(0xdce7e8)));
        assert_eq!((rack_name(&fl, false, false), rack_name(&fl, false, true)), (fl.name_text, Color::rgb(0xededde)));
        assert_eq!((rack_number(&fl, false, false), rack_number(&fl, false, true)), (fl.text_dim.shade(0.08), Color::rgb(0xb2a9b6)));
        let normal = os_ui::kit::State::NORMAL;
        assert_eq!(os_ui::kit::scroll_thumb(&fl, normal, true), Color::rgb(0xb9bfc3));
        assert_eq!(os_ui::kit::scroll_arrow(&fl, normal, true), Some(Color::rgb(0xadb3b7)));
        assert_eq!((os_ui::kit::scroll_thumb(&fl, normal, false), os_ui::kit::scroll_arrow(&fl, normal, false)), (fl.overview, None));

        let lemon = os_theme::design::palette(Design::Lemon, None);
        assert_eq!(rack_name(&lemon, true, false), lemon.name_text);
        assert_eq!((rack_name(&lemon, true, true), rack_number(&lemon, true, true)), (lemon.text, lemon.text));
        assert_eq!(lemon.text, os_theme::design::lemon::TEXT, "the look's brightest text colour");
        let dot = record_dot(&lemon, true);
        assert!(dot.r > 0xd0 && dot.g > 0xd0 && dot.b > 0xd0, "white: {dot:?}");
        assert_eq!(record_dot(&lemon, false), lemon.record);
        let thumb = os_ui::kit::scroll_thumb(&lemon, normal, true);
        assert!(thumb.r > 0xb0 && thumb.r == thumb.b, "the look's own light grey: {thumb:?}");
    }
}
