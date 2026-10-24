// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project picker (VIEW > Project picker, Ctrl+F8, the toolbar's project button): the app's own popup listing the
//! project's patterns and channels; a pick makes a pattern current or opens a channel's window.
// spec: ui/project-picker.md

use os_model::ChannelKind;

use crate::app::{App, State};
use crate::playlist::{Cmd, Mark, PItem, Popup};

/// What a project picker entry does (`Cmd::Project`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjCmd {
    /// Pattern `n` becomes the current pattern.
    Pattern(u16),
    /// Rack row `r` becomes the only selected channel and its window opens.
    Channel(usize),
    /// PATTERNS > Find first empty...
    NewPattern,
}

/// The entries (spec §2 Content): Patterns, a column break, Channels.
pub fn items(s: &State) -> Vec<PItem> {
    let mut nums: Vec<u16> = s.project.patterns.iter().map(|p| p.number).collect();
    nums.sort_unstable();
    nums.dedup();
    let mut v = vec![PItem::head("Patterns")];
    v.extend(nums.into_iter().map(|n| {
        PItem::new(crate::patterns::name_of(s, n), Cmd::Project(ProjCmd::Pattern(n))).mark(Mark::Radio(n == s.pattern))
    }));
    v.push(PItem::sep());
    v.push(PItem::new("New pattern", Cmd::Project(ProjCmd::NewPattern)));
    v.push(PItem::col());
    v.push(PItem::head("Channels"));
    // Automation clip channels are left out: they are the playlist's clips, not instruments.
    let rows: Vec<usize> = (0..s.project.channels.len()).filter(|&r| s.project.channels[r].kind != ChannelKind::Automation).collect();
    if rows.is_empty() {
        v.push(PItem::new("(no channels)", Cmd::Nothing).off());
    }
    v.extend(rows.into_iter().map(|r| {
        PItem::new(crate::rack::channel_name(s, r), Cmd::Project(ProjCmd::Channel(r))).mark(Mark::Radio(r == s.selected_channel))
    }));
    v
}

/// Whether the open popup is the project picker (its key closes it).
pub fn is_open(s: &State) -> bool {
    s.playlist.popup.as_ref().is_some_and(|p| p.items.iter().any(|i| i.cmd == Cmd::Project(ProjCmd::NewPattern)))
}

impl App {
    /// Opens the project picker at (x, y).
    pub fn open_project_picker(&mut self, at: (f32, f32)) {
        self.state.menu_open = None;
        self.state.playlist.popup = Some(Popup { x: at.0, y: at.1, items: items(&self.state), sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// Its key (spec §2 Opening): the picker at the mouse, or closes an open one.
    pub fn toggle_project_picker(&mut self) {
        if is_open(&self.state) {
            self.state.playlist.popup = None;
            self.invalidate();
        } else {
            self.open_project_picker(self.mouse);
        }
    }
}

/// Runs a project picker pick (spec §2 Picks). Returns a status message.
pub fn run(app: &mut App, c: ProjCmd) -> Option<String> {
    match c {
        ProjCmd::Pattern(n) => app.state.pattern = n,
        ProjCmd::Channel(r) if r < app.state.project.channels.len() => {
            app.state.selected_channel = r;
            app.state.rack.select_only(r);
            app.open_channel_window(r);
        }
        ProjCmd::Channel(_) => {}
        ProjCmd::NewPattern => {
            let at = crate::patterns::box_at_mouse(app.mouse);
            crate::patterns::run(&mut app.state, crate::patterns::Cmd::FindFirst, at);
        }
    }
    app.invalidate();
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{test_project, Action};
    use crate::views::menus::{self, VIEW};
    use os_model::Pattern;
    use os_ui::{Canvas, Rect};
    use winit::keyboard::{Key, NamedKey};

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = test_project();
        app.state.project.patterns.push(Pattern { number: 3, name: Some("Drop".into()), ..Pattern::default() });
        app.state.pattern = 1;
        app
    }

    fn labels(items: &[PItem]) -> Vec<String> {
        items.iter().filter(|i| !i.sep && !i.col).map(|i| i.label.to_string()).collect()
    }

    /// Patterns in number order with the current one marked, New pattern, a column break, the channels in rack order
    /// with the selected one marked (spec §2 Content).
    #[test]
    fn lists_patterns_then_channels() {
        let mut app = app();
        app.state.selected_channel = 2;
        let v = items(&app.state);
        assert_eq!(labels(&v), ["Patterns", "Pattern 1", "Drop", "New pattern", "Channels", "Kick", "Clap", "Hat", "Snare", "Pluck"]);
        assert!(v[0].head && v.iter().any(|i| i.col));
        let marked: Vec<String> = v.iter().filter(|i| i.mark == Mark::Radio(true)).map(|i| i.label.to_string()).collect();
        assert_eq!(marked, ["Pattern 1", "Hat"]);
        app.state.project.channels[4].kind = ChannelKind::Automation;
        assert!(!labels(&items(&app.state)).contains(&"Pluck".to_string()), "automation clip channels are left out");
        app.state.project.channels.clear();
        let v = items(&app.state);
        assert!(v.iter().any(|i| i.label == "(no channels)" && !i.enabled));
    }

    /// A pattern becomes current; a channel is selected alone and its window opens (spec §2 Picks).
    #[test]
    fn picks() {
        let mut app = app();
        run(&mut app, ProjCmd::Pattern(3));
        assert_eq!(app.state.pattern, 3);
        run(&mut app, ProjCmd::Channel(3));
        assert_eq!(app.state.selected_channel, 3);
        assert_eq!(app.state.rack.sel.iter().copied().collect::<Vec<_>>(), [3]);
        assert_eq!(app.state.pending_open, Some(3), "a sampler channel's window opens once hosted");
    }

    /// Ctrl+F8 opens and closes it; the VIEW entry and the toolbar button open it; the button's right click is the
    /// plugin menu (spec §2 Opening).
    #[test]
    fn ways_in() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        app.paint(&mut c, None);
        let f8 = Key::Named(NamedKey::F8);
        app.key(&f8, true, false);
        assert!(is_open(&app.state), "Ctrl+F8 opens it");
        app.key(&f8, true, false);
        assert!(!is_open(&app.state), "and closes it");

        let i = (0..).map_while(|i| menus::item_at(VIEW, i)).position(|it| it.label == "Project picker").expect("VIEW entry");
        assert!(menus::works(VIEW, menus::item_at(VIEW, i).unwrap()));
        app.menu_item(VIEW, i);
        assert!(is_open(&app.state));
        app.state.playlist.popup = None;

        app.paint(&mut c, None);
        let b = hit_rect(&app, &Action::ProjectPicker);
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down();
        app.mouse_up();
        let p = app.state.playlist.popup.as_ref().expect("open");
        assert!(is_open(&app.state) && (p.x, p.y) == (b.x, b.bottom() + 2.0), "under the button");
        app.state.playlist.popup = None;
        app.paint(&mut c, None);
        app.mouse_move(b.cx(), b.cy());
        app.mouse_down_right();
        app.mouse_up();
        assert!(crate::plugin_menu::is_open(&app.state), "right click: the plugin menu");
    }
}
