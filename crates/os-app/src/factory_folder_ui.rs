// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::factory_folder` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::path::PathBuf;
use crate::app::App;
use crate::dialog::Then;
#[allow(unused_imports)]
use crate::factory_folder::*;

impl App {
    /// The folder button (or the path beside it): a folder picker starting at the folder in use.
    pub(crate) fn factory_folder_pick(&mut self) {
        use crate::filedlg::{FileDlg, Mode};
        let start = dir().filter(|d| d.is_dir());
        let dlg = FileDlg::new(Mode::Folder, PICK_TITLE, "factory content folder", Then::FactoryFolder).start(start.as_deref());
        self.file_dialog(dlg);
    }

    /// The factory content folder becomes `chosen` (`None`: Reset, the install found on this computer): the engines
    /// take the samples behind the placeholders from there, and so do the hosted Lemon Samplers.
    pub(crate) fn set_factory_folder(&mut self, chosen: Option<PathBuf>) {
        crate::settings_file::write(KEY, &chosen.as_deref().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default());
        self.state.session.transport.set_factory(dir());
        for row in self.state.session.hosted_rows.clone() {
            let Some(ch) = self.state.session.project.channels.get(row).filter(|c| c.kind.is_sampler()) else { continue };
            let Some(path) = ch.sample_path.as_deref().filter(|p| os_engine::SampleLibrary::has_placeholder(p)) else { continue };
            if self.state.session.plugins.descriptor(row).is_some_and(|d| d.name == crate::plugins::LEMON_SAMPLER) {
                self.state.session.plugins.load_sample(row, &crate::plugins::resolve_sample(path));
            }
        }
        self.state.message = Some(format!("FL Studio factory content folder: {}", path_text()));
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::{SAct, FILE};
    use os_ui::Canvas;

    /// The setting: unset it is the install found on this computer; a chosen folder is kept in `settings.txt`, used to
    /// resolve sample paths (their placeholders in any case) and shown on the File page; Reset goes back.
    #[test]
    fn the_chosen_folder_resolves_factory_samples() {
        let root = std::env::temp_dir().join(format!("lemon-factory-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let packs = root.join("Data").join("Patches").join("Packs");
        std::fs::create_dir_all(&packs).unwrap();
        std::fs::write(packs.join("Kick.wav"), b"RIFF").unwrap();
        let mut app = App::new(None);
        assert_eq!(chosen(), None);
        assert_eq!(dir(), os_engine::SampleLibrary::default_factory_dir());
        app.set_factory_folder(Some(root.clone()));
        assert_eq!(crate::settings_file::get(KEY), Some(root.to_string_lossy().into_owned()));
        assert_eq!(dir(), Some(root.clone()));
        let file = crate::plugins::resolve_sample(r"%flstudiodata%\PATCHES\packs\kick.WAV");
        assert!(std::path::Path::new(&file).is_file(), "{file}");
        assert!(path_text().starts_with(&root.to_string_lossy().into_owned()));
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("FL Studio factory content folder: ")));
        // The File page shows it, with Reset and the folder button working; Reset goes back to the install found.
        if let Ok(mut c) = Canvas::new(1920, 1080, 1.0) {
            app.open_settings(FILE);
            app.state.settings.folded.extend(["Backup", "Plugins", "User data folder", "Browser extra search folders"]);
            app.paint(&mut c, None);
            assert!(app.hits.iter().any(|h| h.action == Action::Settings(SAct::Button(SET_FOLDER))), "the folder button works");
            let reset = Action::Settings(SAct::Button(RESET));
            let r = app.hits.iter().rev().find(|h| h.action == reset).map(|h| h.rect).expect("Reset works");
            app.mouse_move(r.cx(), r.cy());
            app.mouse_down();
            app.mouse_up();
            app.paint(&mut c, None);
            assert!(!app.hits.iter().any(|h| h.action == reset), "greyed once nothing is chosen");
        } else {
            app.set_factory_folder(None);
        }
        assert_eq!((chosen(), dir()), (None, os_engine::SampleLibrary::default_factory_dir()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
