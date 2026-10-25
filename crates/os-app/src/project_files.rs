// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! FILE's project file rows (also TOOLS ▸ Delete unused files and save project): Revert to last backup, Save as
//! template..., New from template ▸, Save to new project folder... and Delete unused files and save project.
//!
//! Revert opens the newest backup of the project (autosaved or overwritten, by the time in its name) from the
//! project's backup folder and the global one, under the project's own name: the project file is not touched until the
//! next save. Lemon Studio has no factory templates: Save as template... writes a copy of the project into the
//! Templates folder of the user folder (`crate::templates`, category `User` unless a category folder was picked), and
//! New from template ▸ lists the templates there by category, then Empty. Save to new project folder... saves into a
//! new folder named after the project, in the folder picked, which becomes its data folder. Delete unused files looks
//! only in the data folder's Samples, Recorded and Rendered folders, and only when no other project file lies in the
//! data folder.
// spec: ui/project-templates.md, ui/menu-submenus.md §1, §4, ui/data-folder.md

use std::path::{Path, PathBuf};

use os_model::DataFolder;

use crate::app::{App, State};
use crate::dialog::{spans, Answer, Kind, Msg, Next, Reply, Then};
use crate::playlist::{Cmd, PItem};

/// The category Save as template... writes to when no category folder was picked.
pub const USER_CATEGORY: &str = "User";
/// The data folder's folders Delete unused files looks in: the copied samples, the recordings and the renders.
pub const SCANNED: [&str; 3] = [crate::data_folder::SAMPLES_DIR, "Recorded", "Rendered"];
/// The names the delete question lists before "and N more".
const LISTED: usize = 10;

/// The newest backup of project `name` in folders `dirs`: an autosave or an "overwritten on" copy, by the time in its
/// name (an autosave wins a tie: it is the later state).
pub fn newest_backup(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    let mut best: Option<((u32, u32, u32, u32, u32), bool, PathBuf)> = None;
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for e in entries.flatten() {
            let Some(file) = e.file_name().to_str().map(str::to_string) else { continue };
            let found = crate::backup::saved_at(&file, name, "autosaved").map(|t| (t, true)).or_else(|| crate::backup::saved_at(&file, name, "overwritten").map(|t| (t, false)));
            let Some((t, auto)) = found else { continue };
            if best.as_ref().is_none_or(|(bt, ba, _)| (t, auto) > (*bt, *ba)) {
                best = Some((t, auto, e.path()));
            }
        }
    }
    best.map(|(_, _, p)| p)
}

/// Where Save as template... writes the template picked in the save dialog as `chosen`:
/// `<root>/<category>/<name>/<name>.flp`, the category being the folder picked when it is one directly in `root` (or the template's own folder in one),
/// else `User`.
pub fn template_target(root: &Path, chosen: &Path) -> PathBuf {
    let name = chosen.file_stem().map_or_else(|| "Template".to_string(), |n| n.to_string_lossy().into_owned());
    let parent = chosen.parent();
    let cat = match (parent, parent.and_then(Path::parent), parent.and_then(Path::parent).and_then(Path::parent)) {
        (Some(p), Some(pp), _) if pp == root => p.file_name(),
        (Some(p), Some(pp), Some(ppp)) if ppp == root && p.file_name().is_some_and(|n| n.to_string_lossy() == name) => pp.file_name(),
        _ => None,
    };
    let cat = cat.map_or_else(|| USER_CATEGORY.to_string(), |c| c.to_string_lossy().into_owned());
    crate::templates::file_in(root, &cat, &name)
}

/// FILE ▸ New from template ▸: a row per category of the Templates folder with its templates (each starts a new
/// project from that template), then Empty (the built-in project).
pub fn template_items(_: &State) -> Vec<PItem> {
    let mut items: Vec<PItem> = crate::templates::root()
        .map(|r| crate::templates::scan(&r))
        .unwrap_or_default()
        .into_iter()
        .map(|c| {
            let children = c.templates.iter().map(|t| PItem::new(t.clone(), Cmd::NewTemplate(format!("{}/{t}", c.name)))).collect();
            PItem { children, ..PItem::new(c.name, Cmd::Nothing) }
        })
        .collect();
    items.push(PItem::new(crate::templates::EMPTY, Cmd::NewTemplate(crate::templates::EMPTY.into())));
    items
}

/// Two paths name the same file (Windows: whatever the case).
fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().replace('\\', "/");
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// The files under `dir`, every folder down.
fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => files_under(&p, out),
            Ok(t) if t.is_file() => out.push(p),
            _ => {}
        }
    }
}

/// The files of data folder `dir` the project does not use (`used`: the files it refers to): those in its Samples,
/// Recorded and Rendered folders, project files never. Sorted by path.
pub fn unused_files(dir: &Path, used: &[PathBuf]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for sub in SCANNED {
        files_under(&dir.join(sub), &mut found);
    }
    found.retain(|f| !f.extension().is_some_and(|e| e.eq_ignore_ascii_case("flp")) && !used.iter().any(|u| same_file(f, u)));
    found.sort();
    found
}

/// Whether data folder `dir` holds a project file other than `own`: then it is not this project's alone, and Delete
/// unused files leaves it alone.
pub fn shared_folder(dir: &Path, own: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|r| {
        r.flatten().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("flp")) && e.path().is_file() && !same_file(&e.path(), own))
    })
}

impl App {
    /// FILE ▸ Revert to last backup: the newest backup of the project, after the unsaved changes question as Open asks
    /// it; without one, a box says so.
    pub(crate) fn revert_to_backup(&mut self) {
        let Some(path) = self.state.path.clone() else { return };
        let name = crate::backup::project_name(Some(&path));
        let mut dirs: Vec<PathBuf> = crate::data_folder::backup_folder(&self.state, Some(&path)).into_iter().collect();
        dirs.extend(crate::backup::folder().filter(|d| !dirs.contains(d)));
        match newest_backup(&dirs, &name) {
            Some(b) => self.ask_discard("reverting to the last backup", Next::Revert(b)),
            None => {
                let file = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.tell(Kind::Info, "Revert to last backup", spans(&["There is no backup of ", &file, "."]));
            }
        }
    }

    /// The revert goes on: backup `b` is opened as the project, keeping the project's file and name (Save writes it
    /// over the project file); the project counts as changed. It is not a recent or last used project.
    pub(crate) fn revert_from(&mut self, b: PathBuf) {
        let Some(own) = self.state.path.clone() else { return };
        let (kept, recent) = (crate::templates::last_kept(), crate::recent::list());
        self.open(b.clone());
        crate::templates::restore_last(kept);
        crate::recent::restore(&recent);
        // Not opened: `open` said why.
        if self.state.path.as_deref() != Some(b.as_path()) {
            return;
        }
        self.state.path = Some(own);
        // The project file still holds the last save: unsaved until saved again.
        self.saved.1 = f64::NAN;
        let n = b.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.state.message = Some(format!("Reverted to {n}"));
    }

    /// FILE ▸ Save as template...: the save dialog, in the Templates folder's User category, for the template's name.
    pub(crate) fn save_template_dialog(&mut self) {
        use crate::filedlg::{FileDlg, Mode};
        let Some(root) = crate::templates::root() else {
            self.state.message = Some("Save as template: no user folder".into());
            return;
        };
        let start = root.join(USER_CATEGORY);
        let _ = std::fs::create_dir_all(&start);
        let name = format!("{}.flp", crate::backup::project_name(self.state.path.as_deref()));
        let dlg = FileDlg::new(Mode::Save, "Save as template", "template", Then::SaveTemplate).filter("FLP project", &["flp"]).start(Some(&start)).name(&name);
        self.file_dialog(dlg);
    }

    /// The template's name was given (`chosen`, from the save dialog): a copy of the project is written as that
    /// template (`template_target`); the project keeps its file and name.
    pub(crate) fn template_chosen(&mut self, chosen: PathBuf) {
        let Some(root) = crate::templates::root() else { return };
        let target = template_target(&root, &chosen);
        if let Some(Err(e)) = target.parent().map(std::fs::create_dir_all) {
            self.state.message = Some(format!("Save as template failed: {e}"));
            return;
        }
        let name = target.file_stem().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.state.message = Some(match self.save_copy(target) {
            Some(Ok(())) => format!("Saved the template {name}"),
            Some(Err(e)) => format!("Save as template failed: {e}"),
            None => "Save as template: wait for the project to load or the export to end".into(),
        });
    }

    /// A row of New from template ▸ (`Cmd::NewTemplate`: "Empty" or "<category>/<template>"): after the unsaved changes
    /// question as New asks it, a new untitled project from it.
    pub(crate) fn pick_new_template(&mut self, pick: &str) {
        let file = pick.split_once('/').and_then(|(cat, name)| Some(crate::templates::file_in(&crate::templates::root()?, cat, name)));
        self.ask_discard("starting a new project", Next::Template(file));
    }

    /// The new project from template `file` (None: Empty, or a template that does not load: the built-in project); the
    /// New project form follows as after New.
    pub(crate) fn new_from_pick(&mut self, file: Option<PathBuf>) {
        let name = file.as_deref().and_then(Path::file_stem).map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        if !file.is_some_and(|f| self.new_from_file(&f, &name)) {
            self.empty_project();
        }
        self.offer_new_form();
    }

    /// FILE ▸ Save to new project folder...: a folder picker, starting in the project's folder, for where the new
    /// folder goes.
    pub(crate) fn new_folder_dialog(&mut self) {
        use crate::filedlg::{FileDlg, Mode};
        let Some(path) = self.state.path.clone() else { return };
        let dlg = FileDlg::new(Mode::Folder, "Save to new project folder", "project folder", Then::NewProjectFolder).start(path.parent());
        self.file_dialog(dlg);
    }

    /// The place was picked (`root`): the project is saved as `<root>/<name>/<name>.flp`, named after the project, that
    /// new folder its data folder (the used samples copied there while Copy used samples to the project folder is on). A
    /// project file already there is refused.
    pub(crate) fn new_folder_chosen(&mut self, root: PathBuf) {
        const TITLE: &str = "Save to new project folder";
        let name = crate::backup::project_name(self.state.path.as_deref());
        let dir = root.join(&name);
        let file = dir.join(format!("{name}.flp"));
        if file.exists() {
            return self.tell(Kind::Warning, TITLE, spans(&["", &file.display().to_string(), " already exists. Nothing was saved."]));
        }
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return self.tell(Kind::Error, TITLE, spans(&["", &dir.display().to_string(), &format!(" could not be made: {e}")]));
        }
        if crate::prefs::check(&crate::prefs::COPY_USED_SAMPLES) {
            self.copy_used_samples_to(&dir);
        }
        let df = self.state.project.data_folder.get_or_insert_with(DataFolder::default);
        if df.choice == DataFolder::NONE {
            df.choice = DataFolder::RELATIVE;
        }
        df.folder = crate::data_folder::folder_text(&dir);
        self.save_to(file);
        crate::data_folder::reread(&self.state);
    }

    /// The files the project refers to: its channels' samples (Lemon Sampler's own when hosted).
    fn used_files(&self) -> Vec<PathBuf> {
        let mut used = Vec::new();
        for (row, ch) in self.state.project.channels.iter().enumerate() {
            used.extend(self.plugins.sampler_path(row).filter(|p| !p.is_empty()).map(PathBuf::from));
            used.extend(ch.sample_path.as_deref().filter(|p| !p.is_empty()).map(|p| PathBuf::from(crate::plugins::resolve_sample(p))));
        }
        used
    }

    /// FILE / TOOLS ▸ Delete unused files and save project: the data folder's files the project does not use
    /// (`unused_files`) are listed in a Yes / No question; Yes deletes them, then saves. With none, it only saves. A
    /// data folder that holds other project files is not looked in.
    pub(crate) fn delete_unused_ask(&mut self) {
        const TITLE: &str = "Delete unused files";
        let Some(path) = self.state.path.clone() else { return };
        let dir = crate::data_folder::folder(&self.state).filter(|d| d.is_dir());
        if let Some(d) = dir.as_ref().filter(|d| shared_folder(d, &path)) {
            let text = spans(&["The data folder ", &d.display().to_string(), " holds other projects too: no files were deleted."]);
            return self.tell(Kind::Warning, TITLE, text);
        }
        let files = dir.as_deref().map(|d| unused_files(d, &self.used_files())).unwrap_or_default();
        if files.is_empty() {
            self.save_to(path);
            self.prefix_message("No unused files");
            return;
        }
        let d = dir.expect("files were found in it");
        let mut text = vec![(format!("Delete these {} files the project does not use from its data folder, then save it?\n", files.len()), false)];
        for f in files.iter().take(LISTED) {
            text.push(("\n".into(), false));
            text.push((f.strip_prefix(&d).unwrap_or(f).display().to_string(), true));
        }
        if files.len() > LISTED {
            text.push((format!("\nand {} more", files.len() - LISTED), false));
        }
        self.show_msg(Msg::new(Kind::Question, TITLE, text, &[Answer::Yes, Answer::No], Then::DeleteUnused(files)));
    }

    /// The delete question was answered: Yes deletes `files` and saves the project; No changes nothing.
    pub(crate) fn delete_unused_done(&mut self, files: Vec<PathBuf>, reply: &Reply) {
        if *reply != Reply::Button(Answer::Yes) {
            return;
        }
        let Some(path) = self.state.path.clone() else { return };
        let failed = files.iter().filter(|f| std::fs::remove_file(f).is_err()).count();
        self.save_to(path);
        let done = files.len() - failed;
        match failed {
            0 => self.prefix_message(&format!("Deleted {done} unused files")),
            n => self.prefix_message(&format!("Deleted {done} unused files ({n} could not be deleted)")),
        }
    }

    /// `what` goes before the message the save left: "<what>. Saved ...".
    fn prefix_message(&mut self, what: &str) {
        let m = self.state.message.take().unwrap_or_default();
        self.state.message = Some(if m.is_empty() { what.to_string() } else { format!("{what}. {m}") });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("os_projfiles_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(m: u32, d: u32, h: u32, min: u32) -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, m, d).unwrap().and_hms_opt(h, min, 0).unwrap()
    }

    /// The newest backup of the name wins, across folders and both kinds; other names never count.
    #[test]
    fn backup_picking() {
        use crate::backup::file_name;
        let (a, b) = (scratch("bk_a"), scratch("bk_b"));
        assert_eq!(newest_backup(&[a.clone(), b.clone()], "song"), None, "no backup");
        for (dir, f) in [
            (&a, file_name("song", "autosaved", at(10, 1, 9, 5))),
            (&a, file_name("song", "autosaved", at(9, 30, 23, 59))),
            (&b, file_name("song", "overwritten", at(10, 1, 10, 0))),
            (&b, file_name("song2", "autosaved", at(12, 1, 0, 0))),
            (&a, file_name("son", "autosaved", at(12, 1, 0, 0))),
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        assert_eq!(newest_backup(&[a.clone(), b.clone()], "song"), Some(b.join(file_name("song", "overwritten", at(10, 1, 10, 0)))));
        assert_eq!(newest_backup(std::slice::from_ref(&a), "song"), Some(a.join(file_name("song", "autosaved", at(10, 1, 9, 5)))));
        // A tie: the autosave.
        std::fs::write(a.join(file_name("song", "autosaved", at(10, 1, 10, 0))), b"x").unwrap();
        assert_eq!(newest_backup(&[b.clone(), a.clone()], "song"), Some(a.join(file_name("song", "autosaved", at(10, 1, 10, 0)))));
        for d in [a, b] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    /// Revert opens the newest backup under the project's own path, as a change; with no backup, a box and nothing else.
    #[test]
    fn revert_opens_the_newest_backup() {
        let dir = scratch("revert");
        let bk = dir.join("bk");
        std::fs::create_dir_all(&bk).unwrap();
        crate::backup::TEST_DIR.with(|d| *d.borrow_mut() = Some(bk.clone()));
        let song = dir.join("song.flp");
        let mut app = App::new(None);
        app.state.tempo = 111.0;
        app.save_to(song.clone());
        app.revert_to_backup();
        assert!(app.modal_open(), "no backup: a box says so");
        app.state.dialogs.clear();
        assert_eq!(app.state.tempo, 111.0);
        app.state.tempo = 123.0;
        // Later than the "overwritten on" copy the next save makes now.
        assert_eq!(app.write_backup(at(12, 31, 23, 59)).map(|r| r.is_ok()), Some(true));
        app.state.tempo = 150.0;
        app.save_to(song.clone());
        app.revert_to_backup();
        assert_eq!(app.state.tempo, 123.0, "the backup is open");
        assert_eq!(app.state.path.as_deref(), Some(song.as_path()), "under the project's own name");
        assert!(app.unsaved(), "the project file still holds the last save");
        crate::backup::TEST_DIR.with(|d| *d.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Templates go to User, or to the category folder picked; New from template ▸ lists them by category, then Empty;
    /// a pick starts an untitled project from it.
    #[test]
    fn template_listing() {
        let root = scratch("tpl");
        assert_eq!(template_target(&root, &root.join("User").join("Mine.flp")), root.join("User").join("Mine").join("Mine.flp"));
        assert_eq!(template_target(&root, &root.join("Beats").join("Trap.flp")), root.join("Beats").join("Trap").join("Trap.flp"));
        assert_eq!(template_target(&root, &root.join("Beats").join("Trap").join("Trap.flp")), root.join("Beats").join("Trap").join("Trap.flp"));
        assert_eq!(template_target(&root, &std::env::temp_dir().join("Elsewhere.flp")), root.join("User").join("Elsewhere").join("Elsewhere.flp"));
        crate::templates::TEST_DIR.with(|d| *d.borrow_mut() = Some(root.clone()));
        let rows = template_items(&App::new(None).state);
        assert_eq!(rows.iter().map(|r| r.label.to_string()).collect::<Vec<_>>(), ["Empty"], "no templates: Empty alone");
        let mut app = App::new(None);
        app.state.tempo = 97.0;
        let work = scratch("tpl_work");
        let song = work.join("song.flp");
        app.save_to(song.clone());
        app.template_chosen(root.join("User").join("Slow.flp"));
        assert_eq!(app.state.path.as_deref(), Some(song.as_path()), "the project keeps its file");
        assert!(root.join("User").join("Slow").join("Slow.flp").is_file(), "{:?}", app.state.message);
        let rows = template_items(&app.state);
        assert_eq!(rows.iter().map(|r| r.label.to_string()).collect::<Vec<_>>(), ["User", "Empty"]);
        assert_eq!(rows[0].children.len(), 1);
        assert_eq!((rows[0].children[0].label.as_ref(), &rows[0].children[0].cmd), ("Slow", &Cmd::NewTemplate("User/Slow".into())));
        app.state.tempo = 140.0;
        app.save_to(song.clone());
        app.pick_new_template("User/Slow");
        assert_eq!((app.state.tempo, app.state.path.clone()), (97.0, None), "an untitled project from the template");
        assert!(!app.unsaved());
        app.pick_new_template("Empty");
        assert_eq!((app.state.tempo, app.state.path.clone()), (130.0, None), "the built-in project");
        crate::templates::TEST_DIR.with(|d| *d.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&work);
    }

    /// Save to new project folder: `<name>/<name>.flp` in the folder picked, the data folder there; one already there
    /// is refused.
    #[test]
    fn new_project_folder() {
        let dir = scratch("newdir");
        let mut app = App::new(None);
        app.save_to(dir.join("Tune.flp"));
        app.new_folder_chosen(dir.clone());
        let file = dir.join("Tune").join("Tune.flp");
        assert_eq!(app.state.path.as_deref(), Some(file.as_path()), "{:?}", app.state.message);
        assert!(file.is_file());
        assert_eq!(crate::data_folder::folder(&app.state), Some(dir.join("Tune")));
        app.new_folder_chosen(dir.clone());
        assert!(app.modal_open(), "already there: refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unused files: only the data folder's Samples, Recorded and Rendered, never what the project uses or project files;
    /// a folder with other projects is left alone. Yes deletes and saves.
    #[test]
    fn unused_file_finding() {
        let dir = scratch("unused");
        for f in ["Samples/kick.wav", "Samples/old.wav", "Samples/sub/deep.wav", "Recorded/take 1.wav", "Rendered/x.flp", "Backup/song (autosaved on 1-1-2026 at 0h00).flp", "loose.wav"] {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let used = [dir.join("samples").join("KICK.wav"), dir.join("Recorded").join("other.wav")];
        let used = if cfg!(windows) { used.to_vec() } else { vec![dir.join("Samples").join("kick.wav")] };
        let found = unused_files(&dir, &used);
        assert_eq!(found, [dir.join("Recorded").join("take 1.wav"), dir.join("Samples").join("old.wav"), dir.join("Samples").join("sub").join("deep.wav")]);
        // The project in the folder, with a channel on old.wav.
        let mut app = App::new(None);
        let song = dir.join("song.flp");
        app.state.project.data_folder = Some(DataFolder { choice: DataFolder::RELATIVE, folder: String::new() });
        app.save_to(song.clone());
        if let Some(ch) = app.state.project.channels.first_mut() {
            ch.sample_path = Some(dir.join("Samples").join("old.wav").to_string_lossy().into_owned());
        }
        let left = unused_files(&dir, &app.used_files());
        assert!(!left.contains(&dir.join("Samples").join("old.wav")) && left.contains(&dir.join("Samples").join("kick.wav")), "{left:?}");
        app.delete_unused_ask();
        assert!(app.modal_open(), "asked first");
        assert!(dir.join("Samples").join("kick.wav").is_file(), "nothing deleted before the answer");
        app.state.dialogs.clear();
        app.delete_unused_done(left.clone(), &Reply::Button(Answer::No));
        assert!(left.iter().all(|f| f.is_file()), "No: nothing deleted");
        app.delete_unused_done(left.clone(), &Reply::Button(Answer::Yes));
        assert!(left.iter().all(|f| !f.exists()));
        assert!(dir.join("Samples").join("old.wav").is_file() && dir.join("loose.wav").is_file() && song.is_file() && dir.join("Rendered").join("x.flp").is_file());
        assert!(!app.unsaved() && app.state.message.as_deref().is_some_and(|m| m.starts_with("Deleted")), "{:?}", app.state.message);
        // Another project in the folder: refused, nothing deleted.
        std::fs::write(dir.join("Samples").join("again.wav"), b"x").unwrap();
        std::fs::write(dir.join("other.flp"), b"x").unwrap();
        assert!(shared_folder(&dir, &song) && !shared_folder(&dir.join("Samples"), &song));
        app.delete_unused_ask();
        assert!(app.modal_open() && dir.join("Samples").join("again.wav").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rows that need a saved project are greyed out until it is saved; Save as template... and New from
    /// template ▸ are not.
    #[test]
    fn rows_need_a_saved_project() {
        use crate::views::menus::{active, item_at, DELETE_UNUSED_LABEL, TOOLS_MENU};
        let mut app = App::new(None);
        let row = |menu: usize, label: &str| (0..200).find_map(|i| item_at(menu, i).filter(|it| it.label == label)).unwrap();
        let rows = [(0, "Revert to last backup"), (0, "Save to new project folder..."), (0, DELETE_UNUSED_LABEL), (TOOLS_MENU, DELETE_UNUSED_LABEL)];
        for (m, l) in rows {
            assert!(!active(&app.state, m, row(m, l)), "{l}: never saved");
        }
        assert!(active(&app.state, 0, row(0, "Save as template...")) && active(&app.state, 0, row(0, "New from template")));
        app.state.path = Some(std::env::temp_dir().join("song.flp"));
        for (m, l) in rows {
            assert!(active(&app.state, m, row(m, l)), "{l}: saved");
        }
    }
}
