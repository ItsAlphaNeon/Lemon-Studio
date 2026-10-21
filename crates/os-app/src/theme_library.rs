// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Settings ▸ Theme extras of Lemon Studio's own: the applied theme can be saved under a name into the user `themes`
//! folder (Save preset...), and the user's themes renamed, duplicated, deleted or shown in their folder from a tile's
//! right-click menu; theme changes can be undone and redone (Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y while the page is open,
//! or its Undo / Redo buttons); and a theme whose text is hard to read on its surfaces gets a warning with a one-click
//! fix.
// spec: ui/theme-presets.md §D

use crate::app::{App, State};
use crate::dialog::Field;
use crate::views::settings::{SAct, THEME};
use os_theme::{fl, Color, FlTheme, Palette};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The user's themes folder (`themes` in the user folder; in tests, under the test's theme folder).
pub fn user_dir() -> Option<PathBuf> {
    crate::app::theme_dir().map(|d| d.join("themes"))
}

/// The user's themes: (name, text) of each `.flstheme` in `user_dir`, by name.
pub fn user_themes() -> Vec<(String, String)> {
    let Some(d) = user_dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&d) else { return Vec::new() };
    let mut v: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("flstheme")))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), String::from_utf8_lossy(&std::fs::read(&p).ok()?).into_owned())))
        .collect();
    v.sort_by_key(|(n, _)| n.to_lowercase());
    v
}

/// A typed name made a file name: characters Windows does not allow in one dropped, spaces and dots trimmed at the
/// ends; `None` when nothing is left.
pub fn clean_name(name: &str) -> Option<String> {
    let s: String = name.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control()).collect();
    let s = s.trim().trim_end_matches('.').trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// What the name panel does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NameTarget {
    /// Save preset...: the applied theme under a new name.
    Save,
    /// Rename the user theme at Themes tile `.0`.
    Rename(usize),
    /// Delete the user theme at Themes tile `.0` (no text box: a question).
    Delete(usize),
}

/// The name panel over the Theme page: what it does, the name being typed, and where it opens (its top left).
#[derive(Debug, Clone, PartialEq)]
pub struct NameEdit {
    pub target: NameTarget,
    pub text: Field,
    pub at: (f32, f32),
}

/// The panel's size.
pub const NAME_W: f32 = 280.0;
pub const NAME_H: f32 = 104.0;

/// The name panel's rects in the window `win`: the panel (kept inside the window), the text box, and the OK and Cancel
/// buttons.
pub fn name_rects(e: &NameEdit, win: os_ui::Rect) -> (os_ui::Rect, os_ui::Rect, os_ui::Rect, os_ui::Rect) {
    use os_ui::Rect;
    let x = e.at.0.clamp(win.x + 8.0, (win.right() - NAME_W - 8.0).max(win.x + 8.0));
    let y = e.at.1.clamp(win.y + 8.0, (win.bottom() - NAME_H - 8.0).max(win.y + 8.0));
    let panel = Rect::new(x, y, NAME_W, NAME_H);
    let field = Rect::new(x + 10.0, y + 32.0, NAME_W - 20.0, 24.0);
    let cancel = Rect::new(panel.right() - 10.0 - 70.0, panel.bottom() - 10.0 - 26.0, 70.0, 26.0);
    let ok = Rect::new(cancel.x - 8.0 - 80.0, cancel.y, 80.0, 26.0);
    (panel, field, ok, cancel)
}

impl NameTarget {
    pub fn title(self, s: &State) -> String {
        let name = |i: usize| s.settings.themes.get(i).map_or("", |(n, _)| n.as_str()).to_string();
        match self {
            NameTarget::Save => "Save the theme as".into(),
            NameTarget::Rename(i) => format!("Rename \"{}\" to", name(i)),
            NameTarget::Delete(i) => format!("Delete \"{}\"?", name(i)),
        }
    }

    pub fn ok(self) -> &'static str {
        match self {
            NameTarget::Save => "Save",
            NameTarget::Rename(_) => "Rename",
            NameTarget::Delete(_) => "Delete",
        }
    }

    pub fn typed(self) -> bool {
        !matches!(self, NameTarget::Delete(_))
    }
}

/// A theme state undo goes back to: the applied theme's text and the theme as chosen (`State::fl_text`,
/// `State::theme_base`; `None`: the skin's own colours).
#[derive(Debug, Clone, PartialEq)]
pub struct Snap {
    pub text: Option<String>,
    pub base: Option<String>,
}

/// What a change was, so a run of wheel notches on one knob is one undo step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Wheel(usize),
    Other,
}

/// Theme undo / redo (kept while the program runs).
#[derive(Debug, Default)]
pub struct History {
    pub undo: Vec<Snap>,
    pub redo: Vec<Snap>,
    last: Option<(Step, Instant)>,
}

/// Steps kept.
pub const HISTORY_MAX: usize = 100;
/// Wheel notches on one knob closer together than this are one undo step.
const WHEEL_RUN: Duration = Duration::from_millis(800);

/// The text / surface pairs that must stay readable.
fn text_pairs(p: &Palette) -> [(Color, Color); 5] {
    [(p.text, p.window_title), (p.glyph, p.toolbar), (p.menu_text, p.menu_panel), (p.name_text, p.name_button), (p.browser_folder, p.browser_list)]
}

/// A colour's relative luminance (sRGB, 0..1).
fn luminance(c: Color) -> f32 {
    let f = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
}

/// The contrast ratio of two colours (1..21).
fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// How readable the palette's text is: the smallest contrast ratio between a text colour and the surface it is drawn on.
pub fn readability(p: &Palette) -> f32 {
    text_pairs(p).iter().map(|(a, b)| contrast(*a, *b)).fold(21.0, f32::min)
}

/// Below this ratio the Theme page warns (every built-in theme is above it on both skins).
pub const READABLE: f32 = 2.5;
/// The ratio the fix aims for.
const FIX_TARGET: f32 = 4.5;

/// The theme text `text` with its text colour (TextColor) changed so the text reads on skin `d` with it: the colour's
/// hue kept and its lightness moved away from the surfaces, else white or black; `None` when nothing reaches
/// `READABLE`.
pub fn fix_text(text: &str, d: os_theme::Design) -> Option<String> {
    let th = FlTheme::parse(text);
    let p = os_theme::design::palette(d, Some(&th));
    let now = th.text_color.unwrap_or(p.text);
    let (h, s, _) = now.hsl();
    let dark_bg = text_pairs(&p).iter().map(|(_, b)| b.hsl().2).sum::<f32>() / 5.0 < 0.5;
    let steps: Vec<f32> = if dark_bg { (0..=20).map(|k| 0.5 + k as f32 * 0.025).collect() } else { (0..=20).map(|k| 0.5 - k as f32 * 0.025).collect() };
    let mut candidates: Vec<Color> = steps.into_iter().map(|l| Color::from_hsl_pub(h, s, l, 255)).collect();
    candidates.extend([Color::rgb(0xffffff), Color::rgb(0x000000)]);
    let try_one = |c: Color| {
        let t = fl::set_key(text, "TextColor", &fl::colour_value("TextColor", c).to_string());
        let r = readability(&os_theme::design::palette(d, Some(&FlTheme::parse(&t))));
        (t, r)
    };
    let tried: Vec<(String, f32)> = candidates.into_iter().map(try_one).collect();
    tried
        .iter()
        .find(|(_, r)| *r >= FIX_TARGET)
        .or_else(|| tried.iter().filter(|(_, r)| *r >= READABLE).max_by(|a, b| a.1.total_cmp(&b.1)))
        .map(|(t, _)| t.clone())
}

impl App {
    // ---- Undo / redo ----

    fn snap(&self) -> Snap {
        Snap { text: self.state.fl_text.clone(), base: self.state.theme_base.clone() }
    }

    /// Before a theme change: the state now goes on the undo list (one step for a run of wheel notches on one knob) and
    /// the redo list empties.
    pub(crate) fn theme_mark(&mut self, step: Step) {
        let now = Instant::now();
        let snap = self.snap();
        let h = &mut self.state.settings.history;
        let run = matches!((h.last, step), (Some((Step::Wheel(a), t)), Step::Wheel(b)) if a == b && now.duration_since(t) < WHEEL_RUN);
        h.last = Some((step, now));
        if run || h.undo.last() == Some(&snap) {
            return;
        }
        h.undo.push(snap);
        if h.undo.len() > HISTORY_MAX {
            h.undo.remove(0);
        }
        h.redo.clear();
    }

    /// Theme undo (`redo` = redo): goes back to the last state that differs from now. Returns the message.
    pub(crate) fn theme_undo(&mut self, redo: bool) -> String {
        self.picker_close();
        let now = self.snap();
        let h = &mut self.state.settings.history;
        h.last = None;
        let (from, to) = if redo { (&mut h.redo, &mut h.undo) } else { (&mut h.undo, &mut h.redo) };
        let Some(snap) = std::iter::from_fn(|| from.pop()).find(|s| *s != now) else {
            return if redo { "Nothing to redo".into() } else { "Nothing to undo".into() };
        };
        to.push(now);
        self.theme_restore(snap);
        let what = self.theme_name();
        if redo { format!("Redo: {what}") } else { format!("Undo: {what}") }
    }

    /// The name of the applied theme for messages: its tile's, else "your theme", else the skin's own colours.
    fn theme_name(&self) -> String {
        let st = &self.state.settings;
        let tile = st.themes.get(st.preset).map(|(n, _)| n.clone());
        match (&self.state.fl_text, &self.state.theme_base) {
            (None, _) => format!("{} (the skin's own colours)", self.design().name()),
            (Some(t), Some(b)) if t == b => tile.unwrap_or_else(|| "the loaded theme".into()),
            _ => match tile {
                Some(n) => format!("{n} (adjusted)"),
                None => "your theme".into(),
            },
        }
    }

    /// Applies a theme state: the window, the skin and theme tiles, and `current.flstheme` (removed for none).
    pub(crate) fn theme_restore(&mut self, s: Snap) {
        self.theme = crate::app::skin_theme(self.design(), &self.theme.name.clone(), s.text.as_deref());
        os_theme::set_current(&self.theme.colors);
        if let Some(d) = crate::app::theme_dir() {
            match &s.text {
                Some(t) => {
                    let _ = std::fs::create_dir_all(&d);
                    let _ = std::fs::write(d.join("current.flstheme"), t);
                }
                None => {
                    let _ = std::fs::remove_file(d.join("current.flstheme"));
                }
            }
        }
        self.state.fl_text = s.text;
        self.state.theme_base = s.base;
        self.sync_preset();
        self.invalidate();
    }

    /// Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y while the Theme page is open: theme undo / redo. Returns whether the key was taken.
    pub(crate) fn theme_undo_key(&mut self, key: &winit::keyboard::Key, ctrl: bool) -> bool {
        let st = &self.state.settings;
        if !(ctrl && st.open && st.tab == THEME && st.name_edit.is_none()) {
            return false;
        }
        let winit::keyboard::Key::Character(c) = key else { return false };
        let redo = if c.eq_ignore_ascii_case("z") {
            self.shift
        } else if c.eq_ignore_ascii_case("y") {
            true
        } else {
            return false;
        };
        let m = self.theme_undo(redo);
        self.state.message = Some(m);
        true
    }

    // ---- Readability ----

    /// Updates the Theme page's readability warning from the applied colours.
    pub(crate) fn check_readable(&mut self) {
        let r = readability(&self.theme.colors);
        self.state.settings.unreadable = (r < READABLE).then_some(r);
    }

    /// Fix text: the applied theme's text colour moved until its text reads (`fix_text`).
    pub(crate) fn theme_fix_text(&mut self) {
        let text = self.theme_text();
        match fix_text(&text, self.design()) {
            Some(t) => {
                self.theme_mark(Step::Other);
                self.theme_edit(t);
                self.theme_store();
                self.state.message = Some("Text colour changed so the text reads (Ctrl+Z undoes)".into());
            }
            None => self.state.message = Some("No text colour reads on these surfaces: try the Brightness or Contrast knob".into()),
        }
    }

    // ---- Saved themes ----

    /// Save preset...: the name panel above the button, with a free name.
    pub(crate) fn theme_save_open(&mut self) {
        let st = &self.state.settings;
        let base = st.themes.get(st.preset).map_or("My theme".to_string(), |(n, _)| format!("{n} custom"));
        let taken: Vec<String> = st.themes.iter().map(|(n, _)| n.to_lowercase()).collect();
        let name = std::iter::once(base.clone()).chain((2..).map(|k| format!("{base} {k}"))).find(|n| !taken.contains(&n.to_lowercase())).unwrap_or(base);
        let at = self.button_at("Save preset...").map_or((self.mouse.0, self.mouse.1 - NAME_H), |r| (r.x, r.y - NAME_H - 6.0));
        self.name_open(NameTarget::Save, &name, at);
    }

    /// The right-click menu of Themes tile `i`: Duplicate for any theme; Rename..., Delete... and Show in folder for the
    /// user's own.
    pub(crate) fn theme_menu(&mut self, i: usize, x: f32, y: f32) {
        use crate::playlist::{Cmd, PItem, Popup, ThemeCmd};
        if i >= self.state.settings.themes.len() {
            return;
        }
        let mut items = vec![PItem::new("Duplicate as my theme", Cmd::Theme(ThemeCmd::Duplicate(i)))];
        if i >= self.state.settings.user_from {
            items.push(PItem::new("Rename...", Cmd::Theme(ThemeCmd::Rename(i))));
            items.push(PItem::new("Delete...", Cmd::Theme(ThemeCmd::Delete(i))));
            items.push(PItem::new("Show in folder", Cmd::Theme(ThemeCmd::Show(i))));
        }
        self.state.playlist.popup = Some(Popup { x, y, items, sub: None });
        self.invalidate();
    }

    /// A pick in a tile's menu. Returns the message.
    pub(crate) fn theme_cmd(&mut self, c: crate::playlist::ThemeCmd) -> Option<String> {
        use crate::playlist::ThemeCmd;
        let at = |app: &App, i: usize| {
            app.hits.iter().rev().find(|h| h.action == crate::app::Action::Settings(SAct::Preset(i))).map_or(app.mouse, |h| (h.rect.x, h.rect.bottom() + 4.0))
        };
        match c {
            ThemeCmd::Duplicate(i) => {
                let (name, text) = self.state.settings.themes.get(i).cloned()?;
                let taken: Vec<String> = self.state.settings.themes.iter().map(|(n, _)| n.to_lowercase()).collect();
                let base = format!("{name} copy");
                let new = std::iter::once(base.clone()).chain((2..).map(|k| format!("{base} {k}"))).find(|n| !taken.contains(&n.to_lowercase()))?;
                // The Lemon and Citrus themes stay the skins' own colours as copies too.
                Some(match self.write_user_theme(&new, &text) {
                    Ok(()) => {
                        self.reload_themes();
                        format!("Theme duplicated: {new}")
                    }
                    Err(e) => format!("Could not save the theme: {e}"),
                })
            }
            ThemeCmd::Rename(i) => {
                let name = self.state.settings.themes.get(i)?.0.clone();
                let p = at(self, i);
                self.name_open(NameTarget::Rename(i), &name, p);
                None
            }
            ThemeCmd::Delete(i) => {
                let p = at(self, i);
                self.name_open(NameTarget::Delete(i), "", p);
                None
            }
            ThemeCmd::Show(i) => {
                let name = self.state.settings.themes.get(i)?.0.clone();
                let path = user_dir()?.join(format!("{name}.flstheme"));
                show_in_folder(&path);
                None
            }
        }
    }

    fn name_open(&mut self, target: NameTarget, name: &str, at: (f32, f32)) {
        self.picker_close();
        let mut text = Field::default();
        text.set(name);
        text.select_all();
        self.state.settings.name_edit = Some(NameEdit { target, text, at });
        self.invalidate();
    }

    /// The panel's OK: save, rename or delete. Keeps the panel open with a message when the name will not do.
    pub(crate) fn name_ok(&mut self) {
        let Some(e) = self.state.settings.name_edit.clone() else { return };
        let themes = self.state.settings.themes.clone();
        let user_from = self.state.settings.user_from;
        let msg = match e.target {
            NameTarget::Delete(i) => {
                let Some((name, _)) = themes.get(i).filter(|_| i >= user_from) else { return self.name_close() };
                let r = user_dir().map(|d| std::fs::remove_file(d.join(format!("{name}.flstheme"))));
                self.state.settings.name_edit = None;
                self.reload_themes();
                match r {
                    Some(Ok(())) => format!("Theme deleted: {name}"),
                    Some(Err(e)) => format!("Could not delete the theme: {e}"),
                    None => "No user folder".into(),
                }
            }
            NameTarget::Save | NameTarget::Rename(_) => {
                let Some(name) = clean_name(&e.text.text) else {
                    self.state.message = Some("Type a name for the theme".into());
                    return self.invalidate();
                };
                let builtin = themes[..user_from.min(themes.len())].iter().any(|(n, _)| n.eq_ignore_ascii_case(&name));
                if builtin {
                    self.state.message = Some(format!("\"{name}\" is a built-in theme's name: choose another"));
                    return self.invalidate();
                }
                match e.target {
                    NameTarget::Rename(i) => {
                        let Some((old, _)) = themes.get(i).filter(|_| i >= user_from) else { return self.name_close() };
                        let clash = !old.eq_ignore_ascii_case(&name) && themes[user_from..].iter().any(|(n, _)| n.eq_ignore_ascii_case(&name));
                        if clash {
                            self.state.message = Some(format!("You have a theme called \"{name}\" already"));
                            return self.invalidate();
                        }
                        let r = user_dir().map(|d| std::fs::rename(d.join(format!("{old}.flstheme")), d.join(format!("{name}.flstheme"))));
                        self.state.settings.name_edit = None;
                        self.reload_themes();
                        match r {
                            Some(Ok(())) => format!("Theme renamed: {name}"),
                            Some(Err(e)) => format!("Could not rename the theme: {e}"),
                            None => "No user folder".into(),
                        }
                    }
                    _ => {
                        let text = crate::theme_adjust::applied_text(&self.state, self.design());
                        let replaced = themes[user_from.min(themes.len())..].iter().any(|(n, _)| n.eq_ignore_ascii_case(&name));
                        self.state.settings.name_edit = None;
                        match self.write_user_theme(&name, &text) {
                            Ok(()) => {
                                // The saved theme is now the chosen one, so its tile is framed.
                                self.state.theme_base = Some(text);
                                self.reload_themes();
                                if replaced { format!("Theme saved: {name} (replaced)") } else { format!("Theme saved: {name}") }
                            }
                            Err(e) => format!("Could not save the theme: {e}"),
                        }
                    }
                }
            }
        };
        self.state.message = Some(msg);
        self.invalidate();
    }

    pub(crate) fn name_close(&mut self) {
        self.state.settings.name_edit = None;
        self.invalidate();
    }

    /// Keys while the name panel is open: it takes every key (Enter = OK, Esc = Cancel). False when it is not open.
    pub(crate) fn name_key(&mut self, key: &winit::keyboard::Key, ctrl: bool, alt: bool) -> bool {
        use winit::keyboard::{Key, NamedKey};
        let st = &mut self.state.settings;
        let Some(e) = st.name_edit.as_mut() else { return false };
        if !(st.open && st.tab == THEME) {
            st.name_edit = None;
            return false;
        }
        match key {
            Key::Named(NamedKey::Escape) => self.name_close(),
            Key::Named(NamedKey::Enter) => self.name_ok(),
            _ if e.target.typed() => {
                e.text.key(key, ctrl, alt);
                self.invalidate();
            }
            _ => {}
        }
        true
    }

    /// Whether the name panel has the keyboard (held keys repeat in it).
    pub fn name_editing(&self) -> bool {
        self.state.settings.name_edit.as_ref().is_some_and(|e| e.target.typed())
    }

    fn write_user_theme(&self, name: &str, text: &str) -> std::io::Result<()> {
        let d = user_dir().ok_or_else(|| std::io::Error::other("no user folder"))?;
        std::fs::create_dir_all(&d)?;
        std::fs::write(d.join(format!("{name}.flstheme")), text)
    }

    /// The Themes list again (the user's themes may have changed), with the chosen tile and previews.
    pub(crate) fn reload_themes(&mut self) {
        self.theme_list();
        self.sync_preset();
        self.invalidate();
    }

    /// The Themes tiles: the Lemon and Citrus themes, the built-in themes, then the user's (`user_dir`).
    pub(crate) fn theme_list(&mut self) {
        let st = &mut self.state.settings;
        st.themes = os_theme::Design::ALL
            .iter()
            .map(|d| (d.name().to_string(), os_theme::design::skin_text(*d)))
            .chain(os_theme::presets::PRESETS.iter().map(|(n, t)| (n.to_string(), t.to_string())))
            .collect();
        st.user_from = st.themes.len();
        st.themes.extend(user_themes());
    }

    /// The rect of the Settings button `label` drawn last frame.
    fn button_at(&self, label: &'static str) -> Option<os_ui::Rect> {
        self.hits.iter().rev().find(|h| h.action == crate::app::Action::Settings(SAct::Button(label))).map(|h| h.rect)
    }
}

/// Opens the system file browser at `path`, selected.
pub(crate) fn show_in_folder(path: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = path.parent().map(|d| std::process::Command::new("xdg-open").arg(d).spawn());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned_for_files() {
        assert_eq!(clean_name("  My: theme?. ").as_deref(), Some("My theme"));
        assert_eq!(clean_name(" .. "), None);
        assert_eq!(clean_name("a/b\\c").as_deref(), Some("abc"));
    }

    /// The fix makes a theme with text the colour of its surfaces readable, keeping the rest of the file.
    #[test]
    fn fix_text_makes_text_readable() {
        let d = os_theme::Design::Citrus;
        let dark = fl::DARK_TEXT;
        assert!(readability(&os_theme::design::palette(d, Some(&FlTheme::parse(dark)))) >= READABLE);
        let bad = fl::set_key(dark, "TextColor", &fl::colour_value("TextColor", Color::rgb(0x2a2e30)).to_string());
        let p = os_theme::design::palette(d, Some(&FlTheme::parse(&bad)));
        assert!(readability(&p) < READABLE, "{}", readability(&p));
        let fixed = fix_text(&bad, d).expect("a fix");
        assert!(readability(&os_theme::design::palette(d, Some(&FlTheme::parse(&fixed)))) >= READABLE);
        assert_eq!(fl::remove_key(&fixed, "TextColor"), fl::remove_key(&bad, "TextColor"));
    }

    /// Every built-in theme reads on both skins (no warning on a shipped theme).
    #[test]
    fn built_in_themes_read() {
        for d in os_theme::Design::ALL {
            for (name, text) in os_theme::presets::PRESETS {
                let r = readability(&os_theme::design::palette(d, Some(&FlTheme::parse(text))));
                assert!(r >= READABLE, "{name} on {}: {r}", d.name());
            }
        }
    }
}
