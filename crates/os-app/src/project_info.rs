// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Settings > Info (OPTIONS > Project info, F11): the project's title, author, genre, comments and web link, edited in
//! their boxes. A box takes the keyboard when clicked and edits with the file dialog's text field (`dialog::Field`:
//! caret, Backspace / Delete, Left / Right / Home / End, Ctrl+A); Comments has several lines. The project keeps the
//! text (`Project::info`), so an edit is an unsaved change and is saved with the project; the window caption shows the
//! title instead of the file name (`App::title`).
// spec: ui/settings.md "Info", flp/events/project-info.md

use winit::keyboard::{Key, NamedKey};

use crate::app::{App, State};
use crate::dialog::Field;
use crate::views::settings::{self, InfoEdit, InfoField};

impl App {
    /// A click on an Info box: it takes the keyboard with the caret at the end of its text. Another box being edited is
    /// accepted first; a click on the box being edited changes nothing.
    pub(crate) fn info_act(&mut self, field: InfoField) {
        let s = &mut self.state;
        if s.settings.info_edit.as_ref().is_some_and(|e| e.field == field) {
            return;
        }
        accept_info(s);
        let mut text = Field::default();
        text.set(field.get(&s.project.info).as_deref().unwrap_or(""));
        s.settings.info_edit = Some(InfoEdit { field, text });
        self.invalidate();
    }

    /// Keys while an Info box has the keyboard. It takes every key, so none reaches the typing keyboard or a shortcut;
    /// false when no box has it.
    pub(crate) fn info_key(&mut self, key: &Key, ctrl: bool, alt: bool) -> bool {
        let s = &mut self.state;
        if s.settings.info_edit.is_none() {
            return false;
        }
        // The box is on screen only while the Info page is.
        if !(s.settings.open && s.settings.tab == settings::INFO) {
            accept_info(s);
            return false;
        }
        let e = s.settings.info_edit.as_mut().expect("checked");
        match key {
            Key::Named(NamedKey::Escape) => s.settings.info_edit = None,
            Key::Named(NamedKey::Enter) if e.field.multiline() => e.text.insert("\n"),
            Key::Named(NamedKey::Enter) => accept_info(s),
            Key::Named(k @ (NamedKey::Home | NamedKey::End | NamedKey::ArrowUp | NamedKey::ArrowDown)) if e.field.multiline() => line_key(&mut e.text, *k),
            _ => {
                e.text.key(key, ctrl, alt);
            }
        }
        self.invalidate();
        true
    }

    /// Whether an Info box has the keyboard (held keys repeat in it).
    pub fn info_editing(&self) -> bool {
        self.state.settings.info_edit.is_some()
    }
}

/// Writes the Info box being edited into the project. One-line boxes drop leading and trailing blanks; an emptied box
/// clears its field (a file that has the field keeps it, empty; os-flp `apply`). Unchanged text changes nothing.
pub(crate) fn accept_info(s: &mut State) {
    let Some(e) = s.settings.info_edit.take() else { return };
    let text = if e.field.multiline() { e.text.text } else { e.text.text.trim().to_string() };
    let value = e.field.get_mut(&mut s.project.info);
    if value.as_deref().unwrap_or("") != text {
        *value = (!text.is_empty()).then_some(text);
    }
}

/// Home / End / Up / Down in Comments go by its lines (between line breaks): Up and Down keep the caret's column, in
/// characters, and go to the start / end of the text from the first / last line.
fn line_key(f: &mut Field, key: NamedKey) {
    let chars: Vec<char> = f.text.chars().collect();
    let c = f.caret.min(chars.len());
    let start = |i: usize| chars[..i].iter().rposition(|&ch| ch == '\n').map_or(0, |p| p + 1);
    let end = |i: usize| chars[i..].iter().position(|&ch| ch == '\n').map_or(chars.len(), |p| i + p);
    let (s0, e0) = (start(c), end(c));
    f.caret = match key {
        NamedKey::Home => s0,
        NamedKey::End => e0,
        NamedKey::ArrowUp if s0 == 0 => 0,
        NamedKey::ArrowUp => {
            let prev = start(s0 - 1);
            prev + (c - s0).min(s0 - 1 - prev)
        }
        NamedKey::ArrowDown if e0 == chars.len() => chars.len(),
        NamedKey::ArrowDown => {
            let next = e0 + 1;
            next + (c - s0).min(end(next) - next)
        }
        _ => c,
    };
    f.all = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::SAct;

    fn app() -> App {
        let mut app = App::new(None);
        app.open_settings(settings::INFO);
        app
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            let key = if c == ' ' { Key::Named(NamedKey::Space) } else { Key::Character(c.to_string().into()) };
            app.key(&key, false, false);
        }
    }

    fn named(app: &mut App, k: NamedKey) {
        app.key(&Key::Named(k), false, false);
    }

    /// A click on a box edits it; typing, the caret keys and Ctrl+A work; Enter accepts into the project, which then has
    /// unsaved changes, and the caption shows the title (spec: ui/settings.md "Info", flp/events/project-info.md).
    #[test]
    fn title_is_typed_and_accepted() {
        let mut app = app();
        assert!(!app.unsaved());
        app.act(Action::Settings(SAct::Info(InfoField::Title)));
        assert!(app.info_editing());
        typed(&mut app, "My tune");
        named(&mut app, NamedKey::Home);
        typed(&mut app, "X");
        named(&mut app, NamedKey::Delete);
        named(&mut app, NamedKey::End);
        named(&mut app, NamedKey::Backspace);
        named(&mut app, NamedKey::ArrowLeft);
        typed(&mut app, "!");
        assert_eq!(app.state.settings.info_edit.as_ref().unwrap().text.text, "Xy tu!n");
        assert!(app.state.project.info.title.is_none(), "nothing written while typing");
        app.key(&Key::Character("a".into()), true, false);
        typed(&mut app, "  Song  ");
        named(&mut app, NamedKey::Enter);
        assert!(!app.info_editing());
        assert_eq!(app.state.project.info.title.as_deref(), Some("Song"), "trimmed");
        assert!(app.unsaved());
        assert_eq!(app.title(), "[Song]");
    }

    /// Esc drops the edit; an emptied box clears its field; an unchanged one changes nothing.
    #[test]
    fn escape_cancels_and_empty_clears() {
        let mut app = app();
        app.state.project.info.author = Some("Someone".into());
        let before = app.state.project.clone();
        app.act(Action::Settings(SAct::Info(InfoField::Author)));
        typed(&mut app, " else");
        named(&mut app, NamedKey::Escape);
        assert!(!app.info_editing() && app.state.settings.open, "Esc leaves the window open");
        assert_eq!(app.state.project.info.author.as_deref(), Some("Someone"));
        app.act(Action::Settings(SAct::Info(InfoField::Author)));
        named(&mut app, NamedKey::Enter);
        assert_eq!(app.state.project, before, "unchanged");
        app.act(Action::Settings(SAct::Info(InfoField::Author)));
        app.key(&Key::Character("a".into()), true, false);
        named(&mut app, NamedKey::Backspace);
        named(&mut app, NamedKey::Enter);
        assert_eq!(app.state.project.info.author, None);
    }

    /// Comments: Enter starts a new line, Home / End / Up / Down go by lines; clicking another box accepts it.
    #[test]
    fn comments_have_lines() {
        let mut app = app();
        app.act(Action::Settings(SAct::Info(InfoField::Comments)));
        typed(&mut app, "abc");
        named(&mut app, NamedKey::Enter);
        typed(&mut app, "de");
        named(&mut app, NamedKey::ArrowUp);
        typed(&mut app, "X");
        named(&mut app, NamedKey::End);
        typed(&mut app, "Y");
        named(&mut app, NamedKey::ArrowDown);
        named(&mut app, NamedKey::Home);
        typed(&mut app, "Z");
        assert_eq!(app.state.settings.info_edit.as_ref().unwrap().text.text, "abXcY\nZde");
        app.act(Action::Settings(SAct::Info(InfoField::Url)));
        assert_eq!(app.state.project.info.comments.as_deref(), Some("abXcY\nZde"));
        assert_eq!(app.state.settings.info_edit.as_ref().map(|e| e.field), Some(InfoField::Url));
    }

    /// Clicking a painted box edits it; a click on another tab accepts the text; long comments wrap and paint while
    /// edited.
    #[test]
    fn clicks_edit_and_accept() {
        let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = app();
        let click = |app: &mut App, c: &mut os_ui::Canvas, a: Action| {
            app.paint(c, None);
            let r = app.hits.iter().rev().find(|h| h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"));
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down();
            app.mouse_up();
        };
        click(&mut app, &mut c, Action::Settings(SAct::Info(InfoField::Comments)));
        typed(&mut app, &"word ".repeat(400));
        named(&mut app, NamedKey::Enter);
        typed(&mut app, "end");
        app.paint(&mut c, None);
        click(&mut app, &mut c, Action::Settings(SAct::Info(InfoField::Title)));
        assert!(app.state.project.info.comments.as_deref().is_some_and(|t| t.ends_with(" \nend")));
        typed(&mut app, "Clicked");
        click(&mut app, &mut c, Action::Settings(SAct::Tab(settings::DEBUG)));
        assert_eq!(app.state.settings.tab, settings::DEBUG);
        assert!(!app.info_editing());
        assert_eq!(app.state.project.info.title.as_deref(), Some("Clicked"));
    }

    /// Typed credits are saved in a new project's file, and edited again in the opened file; they read back after
    /// reopening (spec: flp/events/project-info.md).
    #[test]
    fn credits_are_saved() {
        let mut app = app();
        let edit = |app: &mut App, f: InfoField, text: &str| {
            app.act(Action::Settings(SAct::Info(f)));
            app.key(&Key::Character("a".into()), true, false);
            named(app, NamedKey::Backspace);
            typed(app, text);
            accept_info(&mut app.state);
        };
        edit(&mut app, InfoField::Title, "Saved song");
        edit(&mut app, InfoField::Comments, "one");
        let path = std::env::temp_dir().join(format!("lemonstudio-info-{}.flp", std::process::id()));
        app.save_to(path.clone());
        assert!(!app.unsaved(), "{:?}", app.state.message);
        app.open(path.clone());
        assert_eq!((app.state.project.info.title.as_deref(), app.state.project.info.comments.as_deref()), (Some("Saved song"), Some("one")));
        // In the opened file: changed, added and cleared.
        edit(&mut app, InfoField::Title, "Renamed");
        edit(&mut app, InfoField::Url, "https://example.org");
        edit(&mut app, InfoField::Comments, "");
        app.save_to(path.clone());
        app.open(path.clone());
        let _ = std::fs::remove_file(&path);
        let i = &app.state.project.info;
        assert_eq!((i.title.as_deref(), i.url.as_deref(), i.comments.as_deref()), (Some("Renamed"), Some("https://example.org"), Some("")));
    }

    /// While a box has the keyboard, keys do not reach shortcuts or the typing keyboard: F5 does not toggle the
    /// playlist and a letter plays no note.
    #[test]
    fn a_box_keeps_the_keyboard() {
        let mut app = app();
        if !app.typing_on() {
            app.toggle_typing();
        }
        assert!(app.typing_key(winit::keyboard::KeyCode::KeyZ, true, false, false), "Z plays a note");
        app.typing_key(winit::keyboard::KeyCode::KeyZ, false, false, false);
        app.act(Action::Settings(SAct::Info(InfoField::Genre)));
        let shown = app.state.shown(crate::app::Win::Playlist);
        named(&mut app, NamedKey::F5);
        assert_eq!(app.state.shown(crate::app::Win::Playlist), shown);
        assert!(!app.typing_key(winit::keyboard::KeyCode::KeyZ, true, false, false));
        typed(&mut app, "z");
        named(&mut app, NamedKey::Enter);
        assert_eq!(app.state.project.info.genre.as_deref(), Some("z"));
    }
}
