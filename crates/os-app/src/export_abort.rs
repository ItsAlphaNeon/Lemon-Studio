// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Aborting a running export: the progress panel's Abort button and Escape stop it at its next block; what was rendered
//! stays in the file under the name it was given, as FL keeps an aborted render's file (spec: audio/export.md §1d).

use os_ui::{kit, Canvas, Font, Rect};

use crate::app::App;

/// The Abort button, at the top right of the progress panel (`loading_ui::panel_rect`).
pub fn button_rect(screen: (f32, f32)) -> Rect {
    let r = crate::loading_ui::panel_rect(screen);
    Rect::new(r.right() - 16.0 - 72.0, r.y + 8.0, 72.0, 24.0)
}

/// The button, over the export's progress panel.
pub fn paint(c: &mut Canvas, t: &os_theme::Palette, hover: bool) {
    let r = button_rect(c.size());
    kit::button(c, t, r, kit::State { hover, ..kit::State::NORMAL });
    c.text_center(Font::new(14), r, t.text, "Abort");
}

impl App {
    /// A press while the window takes no input (`App::loading`): Escape, or a click on the Abort button, aborts a
    /// running export. True when it did.
    pub fn loading_press(&mut self, escape: bool) -> bool {
        if self.state.session.export.is_none() {
            return false;
        }
        let (x, y) = self.mouse;
        if !escape && !button_rect(self.state.screen).contains(x, y) {
            return false;
        }
        self.state.session.transport.abort_export();
        self.state.message = Some("Aborting the render...".into());
        self.invalidate();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Escape and the Abort button abort only while an export runs; a click elsewhere does nothing.
    #[test]
    fn escape_and_the_button_abort_an_export() {
        let mut app = App::new(None);
        app.state.screen = (1280.0, 800.0);
        assert!(!app.loading_press(true), "nothing runs");
        app.state.session.export = Some(("x.wav".into(), 0.5));
        app.mouse = (5.0, 5.0);
        assert!(!app.loading_press(false), "not on the button");
        let b = button_rect(app.state.screen);
        app.mouse = (b.cx(), b.cy());
        assert!(app.loading_press(false));
        assert!(app.loading_press(true));
        app.state.session.export = None;
    }
}
