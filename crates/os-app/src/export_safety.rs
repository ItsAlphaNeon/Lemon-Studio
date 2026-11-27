// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The Export form's Start, before the render: a file it would write that another program holds open is an error and
//! back to the form, nothing written; one that is there already is asked about ("Confirm save as", Yes replaces it, No
//! goes back to the form), as FL asks (spec: audio/export.md §1e).

use std::path::{Path, PathBuf};

use crate::app::App;
use crate::dialog::{spans, Answer, Body, Kind, Msg, Then};
use crate::export_form::ExportForm;
use crate::project_forms::Form;

/// A file there that can't be opened for writing: another program holds it, or it is read-only.
pub fn in_use(p: &Path) -> bool {
    p.is_file() && std::fs::OpenOptions::new().append(true).open(p).is_err()
}

/// The first of `targets` another program holds: the "Error" message, as FL's.
pub fn in_use_text(targets: &[PathBuf]) -> Option<String> {
    let p = targets.iter().find(|p| in_use(p))?;
    let name = p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    Some(format!("Couldn't create the file \"{name}\".\nPlease check that no other application is currently using it."))
}

/// "Confirm save as" for the `targets` that are there already (None: none is), Yes (`then`) replacing them, No the
/// focused answer, as a file dialog asks it.
pub fn replace_msg(targets: &[PathBuf], then: Then) -> Option<Msg> {
    let there: Vec<String> = targets
        .iter()
        .filter(|p| p.is_file())
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    let text = match &there[..] {
        [] => return None,
        [one] => format!("{one} already exists.\nDo you want to replace it?"),
        more => format!("{} and {} more already exist.\nDo you want to replace them?", more[0], more.len() - 1),
    };
    let mut m = Msg::new(Kind::Warning, "Confirm save as", spans(&[&text]), &[Answer::Yes, Answer::No], then);
    // Replacing loses the files: Enter alone does not.
    m.focus = 1;
    Some(m)
}

impl App {
    /// The form's Start: true when the render may go ahead now. Otherwise the form is back and a message is over it.
    pub fn export_cleared(&mut self, f: &ExportForm) -> bool {
        let targets: Vec<PathBuf> = self.export_job(f).targets();
        if let Some(text) = in_use_text(&targets) {
            self.show_form(Body::Project(Form::Export(Box::new(f.clone()))));
            self.tell(Kind::Error, "Error", spans(&[&text]));
            return false;
        }
        let Some(m) = replace_msg(&targets, Then::ExportReplace) else { return true };
        self.show_form(Body::Project(Form::Export(Box::new(f.clone()))));
        self.show_msg(m);
        false
    }

    /// The answer to "Confirm save as": Yes closes the form below and renders over the files; No leaves the form.
    pub fn export_replace(&mut self, yes: bool) {
        if !yes {
            return;
        }
        let Some(crate::dialog::Modal { body: Body::Project(Form::Export(_)), .. }) = self.state.dialogs.last() else {
            return;
        };
        let Some(crate::dialog::Modal { body: Body::Project(Form::Export(f)), .. }) = self.state.dialogs.pop() else {
            return;
        };
        self.modal_changed();
        self.export_go(*f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::DHit;
    use crate::project_forms::FHit;

    fn app(dir: &Path) -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.session.project.get_mut().patterns[0].notes.push(os_model::Note::step(0, 0));
        app.state.set_song_mode(false);
        app.open_track_render(vec![1]);
        let Some(crate::dialog::Modal { body: Body::Project(Form::Export(f)), .. }) = app.state.dialogs.last_mut()
        else {
            panic!("the form")
        };
        f.folder = dir.to_path_buf();
        f.name.set("guard");
        f.armed = true; // the test folder is not kept as the export folder
        app
    }

    fn top_title(app: &App) -> Option<String> {
        match &app.state.dialogs.last()?.body {
            Body::Msg(m) => Some(m.title.clone()),
            Body::Project(Form::Export(_)) => Some("form".into()),
            _ => None,
        }
    }

    /// A file there: "Confirm save as" over the form; No goes back to the form, Yes renders over it.
    #[test]
    fn a_file_there_is_asked_about() {
        let dir = std::env::temp_dir().join(format!("lemon-export-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("guard.wav");
        std::fs::write(&file, b"old").unwrap();
        let mut app = app(&dir);
        app.dialog_click(DHit::Form(FHit::Main));
        assert_eq!(top_title(&app).as_deref(), Some("Confirm save as"));
        let Some(Body::Msg(m)) = app.state.dialogs.last().map(|d| &d.body) else { panic!("a message") };
        assert_eq!(m.focus, 1, "No is focused: Enter alone does not replace");
        assert!(app.state.session.export.is_none(), "nothing rendered yet");
        app.dialog_click(DHit::Button(1));
        assert_eq!(top_title(&app).as_deref(), Some("form"), "No: back to the form");
        app.dialog_click(DHit::Form(FHit::Main));
        app.dialog_click(DHit::Button(0));
        assert!(app.state.dialogs.is_empty() && app.state.session.export.is_some(), "Yes renders");
        while app.state.session.export.is_some() {
            app.plugin_tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(std::fs::metadata(&file).unwrap().len() > 44, "replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file another program holds open: the Error message over the form, nothing written.
    #[cfg(windows)]
    #[test]
    fn a_file_in_use_is_an_error() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("lemon-export-inuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("guard.wav");
        std::fs::write(&file, b"old").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();
        let mut app = app(&dir);
        app.dialog_click(DHit::Form(FHit::Main));
        let Some(Body::Msg(m)) = app.state.dialogs.last().map(|d| &d.body) else { panic!("a message") };
        assert_eq!(m.title, "Error");
        assert!(m.plain().contains("Couldn't create the file \"guard.wav\"."), "{}", m.plain());
        assert!(app.state.session.export.is_none());
        app.dialog_click(DHit::Button(0));
        assert_eq!(top_title(&app).as_deref(), Some("form"), "back to the form");
        drop(held);
        assert_eq!(std::fs::read(&file).unwrap(), b"old", "nothing written");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
