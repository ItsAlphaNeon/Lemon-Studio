// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Settings > General ▸ Miscellaneous ▸ Default template and Startup project.
//!
//! The Default template is what FILE ▸ New gives (the FILE menu's first item names it, "New (<name>)"): the built-in
//! "Empty" (`app::default_project`), or a template found in the `Templates` folder of Lemon Studio's user folder, laid
//! out as FL's: a folder per category, holding a folder per template with `<name>.flp` in it. A template is opened as a
//! project and the result is untitled; a picked template whose file has gone gives Empty. The pick is a program setting
//! kept as free text (`prefs::text`): "Empty" or "<category>/<template>".
//!
//! The Startup project (`prefs::STARTUP_PROJECT`) is the project Lemon Studio starts with when no project file is
//! passed to it: the built-in project, the Default template, or the project file last opened or saved (kept as a
//! program setting too; one that is missing or does not load gives the Default template).
// spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup project", "General"

use std::path::{Path, PathBuf};

use crate::app::App;
use crate::prefs;

/// The built-in template: Lemon Studio's own new project (one Lemon Sampler channel, 130 BPM).
pub const EMPTY: &str = "Empty";
/// The settings file's keys: the picked template and the project file last opened or saved.
const TEMPLATE_KEY: &str = "default_template";
const LAST_KEY: &str = "last_project";

/// The Templates folder: in Lemon Studio's user folder; in tests, the one set in `TEST_DIR` (else none).
pub fn root() -> Option<PathBuf> {
    #[cfg(test)]
    return TEST_DIR.with(|d| d.borrow().clone());
    #[cfg(not(test))]
    os_paths::user_dir().map(|d| d.join("Templates"))
}

#[cfg(test)]
thread_local! {
    pub static TEST_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// A category folder of templates and the templates in it, by name.
#[derive(Debug, Clone, PartialEq)]
pub struct Category {
    pub name: String,
    pub templates: Vec<String>,
}

/// Template `name`'s file in category `cat`.
fn file_in(root: &Path, cat: &str, name: &str) -> PathBuf {
    root.join(cat).join(name).join(format!("{name}.flp"))
}

/// The folders in `dir`, by name, sorted without regard to case.
fn folders(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|r| r.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    v.sort_by_key(|n| n.to_lowercase());
    v
}

/// The templates in `root`: its category folders with the template folders that hold their `<name>.flp`. Categories
/// without one are left out.
pub fn scan(root: &Path) -> Vec<Category> {
    folders(root)
        .into_iter()
        .filter_map(|cat| {
            let templates: Vec<String> = folders(&root.join(&cat)).into_iter().filter(|t| file_in(root, &cat, t).is_file()).collect();
            (!templates.is_empty()).then_some(Category { name: cat, templates })
        })
        .collect()
}

/// The pick as kept: "Empty" or "<category>/<template>".
fn picked() -> String {
    prefs::text(TEMPLATE_KEY).unwrap_or_else(|| EMPTY.into())
}

/// The picked template's file; `None` for Empty, and for a template whose file has gone (FILE ▸ New gives Empty then).
pub fn file() -> Option<PathBuf> {
    let pick = picked();
    let (cat, name) = pick.split_once('/')?;
    Some(file_in(&root()?, cat, name)).filter(|f| f.is_file())
}

/// The Default template as the FILE menu ("New (<name>)") and the settings page show it: the template's name, or
/// "Empty".
pub fn name() -> String {
    match file() {
        Some(_) => picked().split_once('/').map_or_else(|| EMPTY.into(), |(_, n)| n.to_string()),
        None => EMPTY.into(),
    }
}

/// The project file last opened or saved (`None` when there is none yet).
pub fn last_project() -> Option<PathBuf> {
    prefs::text(LAST_KEY).map(PathBuf::from)
}

/// `path` was opened or saved: it is the last used project.
pub(crate) fn remember(path: &Path) {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    prefs::set_text(LAST_KEY, &path.to_string_lossy());
}

/// The last used project as kept, to put back with `restore_last` after writing a copy or opening a template.
pub(crate) fn last_kept() -> Option<String> {
    prefs::text(LAST_KEY)
}

pub(crate) fn restore_last(kept: Option<String>) {
    prefs::set_text(LAST_KEY, kept.as_deref().unwrap_or(""));
}

impl App {
    /// The Default template combo was clicked: its tree under the box (`x`, `y`: the box's bottom left), "Empty" then a
    /// submenu per category, the current pick marked.
    pub(crate) fn template_menu(&mut self, x: f32, y: f32) {
        use crate::playlist::{Cmd, Mark, PItem, Popup};
        let now = if file().is_some() { picked() } else { EMPTY.into() };
        let mut items = vec![PItem::new(EMPTY, Cmd::Template(EMPTY.into())).mark(Mark::Radio(now == EMPTY))];
        for c in root().map(|r| scan(&r)).unwrap_or_default() {
            let children = c
                .templates
                .iter()
                .map(|t| {
                    let pick = format!("{}/{t}", c.name);
                    let on = now == pick;
                    PItem::new(t.clone(), Cmd::Template(pick)).mark(Mark::Radio(on))
                })
                .collect();
            items.push(PItem { children, ..PItem::new(c.name, Cmd::Nothing) });
        }
        self.state.playlist.popup = Some(Popup { x, y: y + 1.0, items, sub: None });
        self.invalidate();
    }

    /// A row of the Default template tree was picked (`Cmd::Template`): kept. Returns the message.
    pub(crate) fn set_template(&mut self, pick: &str) -> Option<String> {
        prefs::set_text(TEMPLATE_KEY, pick);
        self.invalidate();
        Some(format!("Default template: {}", name()))
    }

    /// FILE ▸ New with a template picked: the template opened as an untitled project (Save asks where), started now.
    /// False for Empty and when the template does not load: the caller makes the built-in project then.
    pub(crate) fn new_from_template(&mut self) -> bool {
        let Some(path) = file() else { return false };
        let kept = last_kept();
        self.state.path = None;
        self.open(path.clone());
        restore_last(kept);
        if self.state.path.as_deref() != Some(path.as_path()) {
            return false;
        }
        let s = &mut self.state;
        s.path = None;
        crate::project_info::created(s);
        s.message = Some(format!("New project from the template {}", name()));
        self.mark_saved();
        true
    }

    /// At startup, when no project file was passed to Lemon Studio: the Startup project's choice. Empty project keeps
    /// the built-in project `App::new` made; Last used project opens the project file last opened or saved, else (and
    /// for Default template) FILE ▸ New's project.
    pub fn startup_project(&mut self) {
        let choice = prefs::index(&prefs::STARTUP_PROJECT);
        if choice == 0 {
            return;
        }
        if choice == 2 {
            if let Some(p) = last_project().filter(|p| p.is_file()) {
                self.open(p.clone());
                if self.state.path.as_deref() == Some(p.as_path()) {
                    return;
                }
            }
        }
        self.new_from_template();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::playlist::{Cmd, Mark};
    use crate::views::settings::{self, SAct};

    /// A fresh Templates folder for this test (this thread's `root`), with "Minimal/Basic with limiter" (a 140 BPM
    /// project with six channels: the test project's five and a copy), "Utility/Scoring", and folders that are no
    /// templates.
    fn templates(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("os_templates_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (cat, name) in [("Minimal", "Basic with limiter"), ("Utility", "Scoring")] {
            let mut a = App::new(None);
            a.state.tempo = 140.0;
            let ch = a.state.project.channels[0].clone();
            a.state.project.channels.push(ch);
            let at = dir.join(cat).join(name);
            std::fs::create_dir_all(&at).unwrap();
            a.save_to(at.join(format!("{name}.flp")));
            assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        }
        std::fs::create_dir_all(dir.join("Other").join("No file")).unwrap();
        std::fs::create_dir_all(dir.join("Minimal").join("Wrong name")).unwrap();
        std::fs::write(dir.join("Minimal").join("Wrong name").join("Other.flp"), b"x").unwrap();
        TEST_DIR.with(|d| *d.borrow_mut() = Some(dir.clone()));
        // Saving those made them the last used project: start this test without one.
        restore_last(None);
        dir
    }

    /// The FILE menu's first item as painted.
    fn file_new_label() -> String {
        crate::views::menus::label_of(0, crate::views::menus::item_at(0, crate::views::menus::FILE_NEW).unwrap()).into_owned()
    }

    /// The tree: "Empty" first, then a submenu per category holding its templates; folders that hold no `<name>.flp`
    /// are left out (spec: ui/settings-choices.md "Miscellaneous: seven more checks, Default template and Startup
    /// project").
    #[test]
    fn the_template_tree() {
        let dir = templates("tree");
        assert_eq!(
            scan(&dir),
            vec![
                Category { name: "Minimal".into(), templates: vec!["Basic with limiter".into()] },
                Category { name: "Utility".into(), templates: vec!["Scoring".into()] },
            ]
        );
        let mut app = App::new(None);
        assert_eq!((name(), file_new_label()), ("Empty".to_string(), "New (Empty)".to_string()));
        app.open_settings(settings::GENERAL);
        app.act(Action::Settings(SAct::Combo(settings::DEFAULT_TEMPLATE, 10.0, 20.0)));
        let p = app.state.playlist.popup.as_ref().expect("the tree");
        let labels: Vec<&str> = p.items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Empty", "Minimal", "Utility"]);
        assert_eq!((p.items[0].mark, p.items[1].children.len()), (Mark::Radio(true), 1));
        let pick = p.items[1].children[0].clone();
        assert_eq!((pick.label.as_ref(), &pick.cmd), ("Basic with limiter", &Cmd::Template("Minimal/Basic with limiter".into())));
        let msg = crate::playlist::run(&mut app, pick.cmd);
        assert_eq!(msg.as_deref(), Some("Default template: Basic with limiter"));
        assert_eq!(name(), "Basic with limiter");
        assert_eq!(file_new_label(), "New (Basic with limiter)");
        app.act(Action::Settings(SAct::Combo(settings::DEFAULT_TEMPLATE, 10.0, 20.0)));
        let p = app.state.playlist.popup.as_ref().unwrap();
        assert_eq!((p.items[0].mark, p.items[1].children[0].mark), (Mark::Radio(false), Mark::Radio(true)));
        // The settings page's combo shows the pick and opens the tree.
        if let Ok(mut c) = os_ui::Canvas::new(1920, 1080, 1.0) {
            app.state.playlist.popup = None;
            app.state.settings.folded.extend(["Display", "Language", "Input", "Undo history", "Hints and warnings"]);
            app.paint(&mut c, None);
            let h = app.hits.iter().rev().find(|h| matches!(&h.action, Action::Settings(SAct::Combo(l, ..)) if *l == settings::DEFAULT_TEMPLATE));
            assert!(h.is_some(), "the combo works");
            let page = settings::page(settings::GENERAL, &app.state);
            let shown = page.iter().flat_map(|g| &g.rows).find_map(|r| match r {
                settings::Row::Combo(v, _, l) if *l == settings::DEFAULT_TEMPLATE => Some(v.clone()),
                _ => None,
            });
            assert_eq!(shown.as_deref(), Some("Basic with limiter"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// FILE ▸ New opens the picked template as an untitled project, which is not the last used project; Empty, or a
    /// template whose file has gone, gives the built-in project (spec: as above).
    #[test]
    fn file_new_uses_the_default_template() {
        let dir = templates("new");
        let mut app = App::new(None);
        app.set_template("Minimal/Basic with limiter");
        app.menu_item(0, crate::views::menus::FILE_NEW);
        assert!(app.state.dialogs.is_empty(), "nothing to discard");
        assert_eq!(app.state.tempo, 140.0);
        assert_eq!(app.state.project.channels.len(), 6);
        assert_eq!(app.state.path, None, "untitled: Save asks where");
        assert!(app.state.file.is_some(), "the template's events are kept for the save");
        assert!(!app.unsaved());
        assert_eq!(app.title(), "[untitled]");
        assert_eq!(app.state.message.as_deref(), Some("New project from the template Basic with limiter"));
        assert_eq!(last_project(), None);
        let w = app.state.project.work_time.expect("started");
        assert_eq!(w.spent, 0.0);
        // Saved where asked, it becomes that file; the template stays as it was.
        let saved = dir.join("Song.flp");
        let before = std::fs::read(file().unwrap()).unwrap();
        app.save_to(saved.clone());
        assert_eq!(app.state.path.as_deref(), Some(saved.as_path()));
        assert_eq!(std::fs::read(file().unwrap()).unwrap(), before);
        assert_eq!(last_project(), Some(std::path::absolute(&saved).unwrap()));
        // The template's file gone: Empty.
        std::fs::remove_dir_all(dir.join("Minimal")).unwrap();
        assert_eq!(name(), "Empty");
        app.menu_item(0, crate::views::menus::FILE_NEW);
        assert_eq!((app.state.tempo, app.state.project.channels.len(), app.state.file.is_none()), (130.0, 1, true));
        assert_eq!(app.state.message.as_deref(), Some("New project"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Startup project: exactly FL's three choices, Default template by default; the last used project is the file
    /// last opened or saved (not a backup copy); a missing one gives the default template (spec: as above).
    #[test]
    fn startup_project_choices() {
        assert_eq!(prefs::STARTUP_PROJECT.choices, ["Empty project", "Default template", "Last used project"]);
        assert_eq!(prefs::choice(&prefs::STARTUP_PROJECT), "Default template");
        let dir = templates("startup");
        let mut app = App::new(None);
        app.set_template("Utility/Scoring");
        // Default template.
        let mut a = App::new(None);
        a.startup_project();
        assert_eq!((a.state.tempo, a.state.path.clone()), (140.0, None));
        // Empty project.
        prefs::set_choice(&prefs::STARTUP_PROJECT, 0);
        let mut a = App::new(None);
        a.startup_project();
        assert_eq!((a.state.tempo, a.state.project.channels.len(), a.state.file.is_none()), (130.0, 5, true), "App::new's project, untouched");
        // Last used project: the file last saved; a backup copy does not count.
        prefs::set_choice(&prefs::STARTUP_PROJECT, 2);
        let song = dir.join("Last.flp");
        app.state.tempo = 111.0;
        app.save_to(song.clone());
        assert_eq!(last_project(), Some(std::path::absolute(&song).unwrap()));
        assert!(app.save_copy(dir.join("copy.flp")).is_some_and(|r| r.is_ok()));
        assert_eq!(last_project(), Some(std::path::absolute(&song).unwrap()), "a copy is not the last used project");
        let mut a = App::new(None);
        a.startup_project();
        assert_eq!((a.state.tempo, a.state.path.as_deref()), (111.0, Some(std::path::absolute(&song).unwrap().as_path())));
        // Opening another file makes it the last used project.
        let other = dir.join("Other.flp");
        std::fs::copy(&song, &other).unwrap();
        a.open(other.clone());
        assert_eq!(last_project(), Some(std::path::absolute(&other).unwrap()));
        // Gone, or not a project: the default template.
        std::fs::remove_file(&other).unwrap();
        let mut a = App::new(None);
        a.startup_project();
        assert_eq!((a.state.tempo, a.state.path.clone()), (140.0, None));
        std::fs::write(&other, b"not a project").unwrap();
        let mut a = App::new(None);
        a.startup_project();
        assert_eq!((a.state.tempo, a.state.path.clone()), (140.0, None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
