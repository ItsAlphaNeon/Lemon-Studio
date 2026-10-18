// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's Save as and New project forms (a `project_forms::Form`), which Settings ▸ Project ▸ Data folder ▸ Ask
//! for data folder brings up: saving a project that was never saved opens Save as instead of the file dialog, FILE ▸
//! New opens New project over the new project, and the program's start can too. Both name the project and save it into
//! a folder of its own under a root folder (the user's projects folder unless another was chosen), copying the samples
//! it uses there when asked. The form keeps its root folder, its Copy check and its Move choice between uses.
//! `views::save_form` paints them.
// spec: ui/data-folder.md §7

use std::path::{Path, PathBuf};

use os_model::DataFolder;
use winit::keyboard::{Key, NamedKey};

use crate::app::{App, State};
use crate::data_folder as df;
use crate::dialog::{spans, Answer, Body, Field, Kind, Next, Reply, Then};
use crate::prefs;
use crate::project_forms::{FHit, Form};
use crate::views::settings::{DENOMINATORS, NUMERATORS, TIMEBASES};

/// The titles, the group names and the labels (spec §7).
pub const SAVE_TITLE: &str = "Save as";
pub const NEW_TITLE: &str = "New project";
pub const GROUP: &str = "Name and location";
pub const NAME: &str = "Name";
pub const LOCATION: &str = "Location";
pub const AUTO: &str = "Auto";
/// The checks, by their places (`FHit::Check`): Create new project folder, Copy used samples, Don't show.
pub const CHECKS: [&str; 3] = ["Create new project folder", "Copy used samples to the project folder", "Don't show this in the future"];
/// What to do with the files created for the project, and the words after the combo.
pub const FILES: [&str; 3] = ["Move", "Copy", "Don't move or copy"];
pub const FILES_TEXT: &str = "files created for this project to the folder";
pub const INFO: &str = "Information...";
/// The Information fields, by their places after the name (`FHit::Field(1..=4)`): the Info page's.
pub const INFO_FIELDS: [&str; 4] = ["Title", "Author", "Web link", "Genre"];
pub const TIME: &str = "Time settings";
/// The time settings' boxes, by their places after the Move combo (`FHit::Combo(1..=3)`): the Project page's.
pub const TIME_LABELS: [&str; 3] = [crate::views::settings::TIMEBASE, crate::views::settings::NUMERATOR, crate::views::settings::DENOMINATOR];
pub const SKIP: &str = "Skip";
pub const SAVE: &str = "Save";
pub const CREATE: &str = "Create";
/// The folder button's picker.
pub const PICK_TITLE: &str = "Select or create a project folder";

/// The hints (spec §7).
pub const NAME_HINT: &str = "Set the project name";
pub const AUTO_HINT: &str = "Set to default project root folder";
pub const FOLDER_HINT: &str = "Select a project folder...";
pub const CHECK_HINTS: [&str; 3] = ["Select to create a new project folder", "Copy used samples into the project folder", "Don't show this window when starting a new project"];
pub const FILES_HINT: &str = "Choose what to do with created project files";
pub const SKIP_HINT: &str = "Skip to regular file browser";
pub const MAIN_HINT: &str = "Create the new project";

/// Ask for data folder's choice Don't ask (`prefs::ASK_DATA_FOLDER`), which Don't show this in the future sets.
pub const DONT_ASK: usize = 4;

/// What the forms keep between uses (`prefs::text`): the root folder, the Copy check ("1" ticked) and the Move choice.
const ROOT_KEY: &str = "project_form_root";
const COPY_KEY: &str = "project_form_copy_samples";
const FILES_KEY: &str = "project_form_project_files";

/// Ask for data folder (spec §7): saving a project never saved shows Save as (its first, second and fourth choices).
pub fn asks_on_save() -> bool {
    matches!(prefs::index(&prefs::ASK_DATA_FOLDER), 0 | 1 | 3)
}

/// FILE ▸ New shows New project (the first three choices).
pub fn asks_on_new() -> bool {
    matches!(prefs::index(&prefs::ASK_DATA_FOLDER), 0..=2)
}

/// The program's start shows New project (the first choice).
pub fn asks_on_startup() -> bool {
    prefs::index(&prefs::ASK_DATA_FOLDER) == 0
}

/// The user's projects folder, which Auto puts back: `<user folder>/Projects` (`backup::folder` keeps its Backup in
/// it). Tests: the folder their thread set in `TEST_ROOT`, else none.
pub fn default_root() -> Option<PathBuf> {
    #[cfg(test)]
    return TEST_ROOT.with(|d| d.borrow().clone());
    #[cfg(not(test))]
    os_paths::user_dir().map(|d| d.join("Projects"))
}

#[cfg(test)]
thread_local! {
    pub static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// The first `Project_N` (N from 1) that names nothing in `root`, as a folder or as a project file.
pub fn free_name(root: &Path) -> String {
    let taken = |n: &str| !root.as_os_str().is_empty() && (root.join(n).exists() || root.join(format!("{n}.flp")).exists());
    (1u32..).map(|n| format!("Project_{n}")).find(|n| !taken(n)).expect("a free name")
}

/// Save as or New project, as filled in so far.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveForm {
    /// New project (Time settings; Create, no Skip), else Save as.
    pub new: bool,
    pub name: Field,
    /// The folder the project goes into, or into a folder of its name in; empty when none is known.
    pub root: PathBuf,
    pub new_folder: bool,
    pub copy: bool,
    /// The Move combo's choice (`FILES`).
    pub files: u8,
    /// Title, Author, Web link and Genre (`INFO_FIELDS`), and whether their group is unfolded.
    pub info: [Field; 4],
    pub info_open: bool,
    /// New project's time settings: the timebase, and the bar as a time signature.
    pub ppq: u16,
    pub num: u16,
    pub den: u16,
    pub dont_show: bool,
    /// The field that takes the keys: 0 the name, 1-4 the Information fields.
    pub focus: u8,
    /// The combo whose list is open: 0 Move, 1 Timebase, 2 Numerator, 3 Denominator.
    pub list: Option<u8>,
    /// What waits for the save (the save-changes question's work): it goes on once the project is saved.
    pub next: Option<Next>,
}

impl SaveForm {
    /// The form for the open project: the remembered root folder (else the user's projects folder), the next free
    /// `Project_N` selected, Create new project folder ticked, Copy and Move as last left, the project's information and
    /// time settings.
    pub fn new(new: bool, s: &State, next: Option<Next>) -> SaveForm {
        let root = prefs::text(ROOT_KEY).map(PathBuf::from).or_else(default_root).unwrap_or_default();
        let mut name = Field::default();
        name.set(&free_name(&root));
        name.select_all();
        let p = &s.project;
        let info = [&p.info.title, &p.info.author, &p.info.url, &p.info.genre].map(|v| {
            let mut f = Field::default();
            f.set(v.as_deref().unwrap_or(""));
            f
        });
        let (beats, steps) = p.bar();
        let den = 16 / steps.max(1);
        SaveForm {
            new,
            name,
            root,
            new_folder: true,
            copy: prefs::text(COPY_KEY).as_deref() == Some("1"),
            files: prefs::text(FILES_KEY).and_then(|t| FILES.iter().position(|f| *f == t)).unwrap_or(0) as u8,
            info,
            info_open: false,
            ppq: p.ppq,
            num: beats.clamp(1, 16),
            den: if 16 % steps.max(1) == 0 && DENOMINATORS.contains(&den) { den } else { 4 },
            dont_show: false,
            focus: 0,
            list: None,
            next,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.new { NEW_TITLE } else { SAVE_TITLE }
    }

    /// The main button's label: Create or Save.
    pub fn main(&self) -> &'static str {
        if self.new { CREATE } else { SAVE }
    }

    /// The folder the project goes to: the root plus the name with Create new project folder ticked, else the root.
    pub fn folder(&self) -> PathBuf {
        if self.new_folder { self.root.join(self.name.text.trim()) } else { self.root.clone() }
    }

    /// The project's file: `<folder>/<name>.flp`.
    pub fn file(&self) -> PathBuf {
        self.folder().join(format!("{}.flp", self.name.text.trim()))
    }

    /// Location's text: the folder with a trailing separator (nothing without a root).
    pub fn location(&self) -> String {
        if self.root.as_os_str().is_empty() {
            return String::new();
        }
        format!("{}{}", self.folder().display().to_string().trim_end_matches(['\\', '/']), std::path::MAIN_SEPARATOR)
    }

    /// The field that takes the keys.
    fn field_mut(&mut self) -> &mut Field {
        match self.focus {
            1..=4 => &mut self.info[usize::from(self.focus) - 1],
            _ => &mut self.name,
        }
    }

    /// What is wrong, if anything: a name that cannot name a file, no root folder, a project file that is there already
    /// (Lemon Studio rule: nothing is saved over from the form).
    fn problem(&self) -> Option<String> {
        if !df::valid_name(&self.name.text) {
            return Some("Type a project name (without \\ / : * ? \" < > |).".into());
        }
        if self.root.as_os_str().is_empty() {
            return Some("Choose a folder for the project (the folder button).".into());
        }
        let file = self.file();
        file.exists().then(|| format!("{} already exists.", file.display()))
    }

    /// Combo `k`'s list: its entries and the chosen one.
    pub fn list_items(&self, k: u8) -> (Vec<String>, usize) {
        let pos = |v: &[u16], now: u16| v.iter().position(|&x| x == now).unwrap_or(usize::MAX);
        let texts = |v: &[u16]| v.iter().map(u16::to_string).collect();
        match k {
            0 => (FILES.iter().map(|f| f.to_string()).collect(), usize::from(self.files)),
            1 => (texts(TIMEBASES), pos(TIMEBASES, self.ppq)),
            2 => (texts(NUMERATORS), pos(NUMERATORS, self.num)),
            _ => (texts(DENOMINATORS), pos(DENOMINATORS, self.den)),
        }
    }

    /// Combo `k`'s text.
    pub fn combo_text(&self, k: u8) -> String {
        match k {
            0 => FILES[usize::from(self.files).min(FILES.len() - 1)].to_string(),
            1 => self.ppq.to_string(),
            2 => self.num.to_string(),
            _ => self.den.to_string(),
        }
    }

    /// Entry `i` of combo `k`'s list was picked.
    fn pick(&mut self, k: u8, i: usize) {
        match k {
            0 if i < FILES.len() => self.files = i as u8,
            1 => self.ppq = TIMEBASES.get(i).copied().unwrap_or(self.ppq),
            2 => self.num = NUMERATORS.get(i).copied().unwrap_or(self.num),
            3 => self.den = DENOMINATORS.get(i).copied().unwrap_or(self.den),
            _ => {}
        }
    }
}

/// The top dialog's Save as or New project form.
fn top(app: &mut App) -> Option<&mut SaveForm> {
    match &mut app.state.dialogs.last_mut()?.body {
        Body::Project(Form::Save(f)) => Some(f),
        _ => None,
    }
}

impl App {
    /// Save as on a project never saved, while Ask for data folder says so: the Save as form; `next` goes on once the
    /// project is saved (spec §7).
    pub(crate) fn open_save_form(&mut self, next: Option<Next>) {
        let f = SaveForm::new(false, &self.state, next);
        self.show_form(Body::Project(Form::Save(Box::new(f))));
    }

    /// FILE ▸ New made a new project: the New project form over it, while Ask for data folder says so (spec §7).
    pub(crate) fn offer_new_form(&mut self) {
        if asks_on_new() {
            self.open_new_form();
        }
    }

    /// The program has started: with Ask for data folder's first choice, the New project form over the startup project,
    /// when that is a new one (Lemon Studio rule: a project opened from its file is not asked about).
    pub fn startup_form(&mut self) {
        if asks_on_startup() && self.state.path.is_none() {
            self.open_new_form();
        }
    }

    fn open_new_form(&mut self) {
        let f = SaveForm::new(true, &self.state, None);
        self.show_form(Body::Project(Form::Save(Box::new(f))));
    }

    /// A click on a part of the form; `Some` closes it with that answer. With a list open, a click picks from it or
    /// only closes it.
    pub(crate) fn save_form_click(&mut self, h: FHit) -> Option<Reply> {
        let f = top(self)?;
        if let Some(k) = f.list.take() {
            if let FHit::Item(i) = h {
                f.pick(k, usize::from(i));
            }
            self.invalidate();
            return None;
        }
        match h {
            FHit::Field(i) if i == 0 || (f.info_open && i <= 4) => f.focus = i,
            FHit::Field(_) | FHit::Item(_) | FHit::Shut => {}
            FHit::Check(0) => f.new_folder = !f.new_folder,
            FHit::Check(1) => f.copy = !f.copy,
            FHit::Check(_) => f.dont_show = !f.dont_show,
            FHit::Combo(k) if k == 0 || f.new => f.list = Some(k),
            FHit::Combo(_) => {}
            FHit::Fold => {
                f.info_open = !f.info_open;
                if !f.info_open {
                    f.focus = 0;
                }
            }
            FHit::Auto => {
                if let Some(r) = default_root() {
                    f.root = r;
                }
            }
            FHit::Folder => self.form_root_pick(),
            FHit::Skip => return (!f.new).then_some(Reply::Button(Answer::No)),
            FHit::Main => return self.save_form_accept(),
        }
        self.invalidate();
        None
    }

    /// Save or Create (or Enter): the answer once the form is right, else a message over it.
    fn save_form_accept(&mut self) -> Option<Reply> {
        let f = top(self)?;
        let title = f.title();
        match f.problem() {
            None => Some(Reply::Button(Answer::Ok)),
            Some(p) => {
                self.tell(Kind::Warning, title, spans(&[&p]));
                None
            }
        }
    }

    /// Keys while the form is on top; `Some` closes it with that answer. Esc closes an open list first, then the form
    /// (as Cancel); Enter is the main button; Tab goes to the next field (the Information fields while unfolded).
    pub(crate) fn save_form_key(&mut self, key: &Key, ctrl: bool, alt: bool) -> Option<Reply> {
        let f = top(self)?;
        if f.list.is_some() {
            if matches!(key, Key::Named(NamedKey::Escape)) {
                f.list = None;
                self.invalidate();
            }
            return None;
        }
        match key {
            Key::Named(NamedKey::Escape) => return Some(Reply::Button(Answer::Cancel)),
            Key::Named(NamedKey::Enter) => return self.save_form_accept(),
            Key::Named(NamedKey::Tab) => {
                f.focus = if f.info_open { (f.focus + 1) % 5 } else { 0 };
                f.field_mut().select_all();
            }
            _ => {
                f.field_mut().key(key, ctrl, alt);
            }
        }
        self.invalidate();
        None
    }

    /// The folder button: a folder picker for the root folder, starting in it.
    fn form_root_pick(&mut self) {
        use crate::filedlg::{FileDlg, Mode};
        let start = top(self).map(|f| f.root.clone()).filter(|d| d.is_dir());
        let dlg = FileDlg::new(Mode::Folder, PICK_TITLE, "project folder", Then::DataFolder(df::DThen::FormRoot)).start(start.as_deref());
        self.file_dialog(dlg);
    }

    /// The picker chose `dir`: the form's root folder.
    pub(crate) fn form_root_chosen(&mut self, dir: PathBuf) {
        if let Some(f) = top(self) {
            f.root = dir;
        }
        self.invalidate();
    }

    /// The form closed with `reply`. Don't show this in the future, ticked, sets Ask for data folder to Don't ask
    /// whatever the answer. Save / Create saves; Skip opens the file dialog; Cancel saves nothing (the work waiting for
    /// the save is called off; New project's new project stays open, unsaved).
    pub(crate) fn save_form_closed(&mut self, f: SaveForm, reply: Reply) {
        if f.dont_show {
            prefs::set_choice(&prefs::ASK_DATA_FOLDER, DONT_ASK);
        }
        match reply {
            Reply::Button(Answer::Ok) => self.save_form_save(f),
            Reply::Button(Answer::No) => self.save_file_dialog(f.next),
            _ => self.dialog_done(Then::SaveProject(f.next), Reply::File(None)),
        }
    }

    /// Save / Create (spec §7): the folder is made, the form's information (and New project's time settings) go into
    /// the project, the samples it uses are copied to `<folder>/Samples` while Copy is ticked, event 202 names the
    /// folder (the Data folder choice stays as it was), and the project is saved as `<folder>/<name>.flp`. The root
    /// folder, Copy and Move are kept for the next form.
    fn save_form_save(&mut self, f: SaveForm) {
        prefs::set_text(ROOT_KEY, &f.root.to_string_lossy());
        prefs::set_text(COPY_KEY, if f.copy { "1" } else { "0" });
        prefs::set_text(FILES_KEY, FILES[usize::from(f.files).min(FILES.len() - 1)]);
        let dir = f.folder();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.tell(Kind::Error, f.title(), spans(&["", &dir.display().to_string(), &format!(" could not be made: {e}")]));
            return self.dialog_done(Then::SaveProject(f.next), Reply::File(None));
        }
        let info = &mut self.state.project.info;
        for (value, field) in [&mut info.title, &mut info.author, &mut info.url, &mut info.genre].into_iter().zip(&f.info) {
            let text = field.text.trim();
            if value.as_deref().unwrap_or("") != text {
                *value = (!text.is_empty()).then(|| text.to_string());
            }
        }
        if f.new {
            self.set_timebase(f.ppq);
            self.set_bar(f.num, 16 / f.den.max(1));
        }
        if f.copy {
            self.state.message = Some(self.copy_used_samples_to(&dir));
        }
        self.state.project.data_folder.get_or_insert_with(DataFolder::default).folder = df::folder_text(&dir);
        let file = f.file();
        self.dialog_done(Then::SaveProject(f.next), Reply::File(Some(file)));
        df::reread(&self.state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::dialog::DHit;

    /// A fresh scratch folder for one test, set as the user's projects folder (removed again by the caller).
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("os_saveform_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TEST_ROOT.with(|d| *d.borrow_mut() = Some(dir.join("Projects")));
        dir
    }

    fn done(dir: &Path) {
        TEST_ROOT.with(|d| *d.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn form(app: &App) -> Option<&SaveForm> {
        match &app.state.dialogs.last()?.body {
            Body::Project(Form::Save(f)) => Some(f),
            _ => None,
        }
    }

    fn click(app: &mut App, h: FHit) {
        app.dialog_click(DHit::Form(h));
    }

    fn type_text(app: &mut App, text: &str) {
        app.dialog_key(&Key::Character(text.into()), false, false);
    }

    /// Which of Ask for data folder's choices bring which form up (spec §7 table).
    #[test]
    fn the_ask_setting_decides() {
        let table = [(true, true, true), (true, true, false), (false, true, false), (true, false, false), (false, false, false)];
        for (i, want) in table.into_iter().enumerate() {
            prefs::set_choice(&prefs::ASK_DATA_FOLDER, i);
            assert_eq!((asks_on_save(), asks_on_new(), asks_on_startup()), want, "{}", prefs::ASK_DATA_FOLDER.choices[i]);
        }
        prefs::set_choice(&prefs::ASK_DATA_FOLDER, 3);
    }

    /// Save on a project never saved (the default, When saving unsaved projects only): the Save as form, named the next
    /// free Project_N in the user's projects folder, Create new project folder ticked, Location the folder it makes.
    /// Save makes `<root>/Project_1/Project_1.flp`, copies the samples to its Samples folder, 202 names the folder and
    /// 167 stays 0; the next form offers Project_2, the remembered root and Copy as left. A saved project's Save as is
    /// the file dialog.
    #[test]
    fn save_as_form_saves_into_a_new_folder() {
        let dir = scratch("save");
        let root = dir.join("Projects");
        let mut app = App::new(None);
        let kick = dir.join("lib").join("kick.wav");
        std::fs::create_dir_all(kick.parent().unwrap()).unwrap();
        std::fs::write(&kick, [7u8; 12]).unwrap();
        app.state.project.channels[0].sample_path = Some(kick.to_string_lossy().into_owned());
        app.act(Action::MenuItem(0, crate::views::menus::FILE_SAVE));
        let f = form(&app).expect("the Save as form").clone();
        assert_eq!((f.title(), f.name.text.as_str(), f.name.all, f.new_folder, f.copy, f.files), (SAVE_TITLE, "Project_1", true, true, false, 0));
        assert_eq!(f.location(), format!("{}{}", root.join("Project_1").display(), std::path::MAIN_SEPARATOR));
        // Unticked, the root itself; ticked again.
        click(&mut app, FHit::Check(0));
        assert_eq!(form(&app).unwrap().location(), format!("{}{}", root.display(), std::path::MAIN_SEPARATOR));
        click(&mut app, FHit::Check(0));
        click(&mut app, FHit::Check(1));
        click(&mut app, FHit::Main);
        assert!(app.state.dialogs.is_empty(), "saved");
        let folder = root.join("Project_1");
        let file = folder.join("Project_1.flp");
        let copy = folder.join(df::SAMPLES_DIR).join("kick.wav");
        assert_eq!(app.state.path, Some(file.clone()));
        assert!(file.is_file() && copy.is_file());
        let opened = App::new(Some(file.clone()));
        assert_eq!(opened.state.project.channels[0].sample_path.as_deref(), Some(copy.to_string_lossy().as_ref()));
        assert_eq!(opened.state.project.data_folder, Some(DataFolder { choice: DataFolder::NONE, folder: df::folder_text(&folder) }));
        // A new project: the form remembers the root and Copy, and offers the next free name.
        let mut other = App::new(None);
        other.act(Action::MenuItem(0, crate::views::menus::FILE_SAVE_AS));
        let f = form(&other).expect("the form again");
        assert_eq!((f.name.text.as_str(), f.root.clone(), f.copy), ("Project_2", root.clone(), true));
        other.dialog_key(&Key::Named(NamedKey::Escape), false, false);
        assert!(other.state.dialogs.is_empty() && other.state.path.is_none(), "Cancel saves nothing");
        // Save as on a saved project: the file dialog (answered at once in tests: cancelled).
        let saves = crate::dialog::fake::shown().1;
        app.act(Action::MenuItem(0, crate::views::menus::FILE_SAVE_AS));
        assert!(app.state.dialogs.is_empty());
        assert_eq!(crate::dialog::fake::shown().1, saves + 1);
        done(&dir);
    }

    /// Typing names the project; a name that cannot name a file, or a project file that is there already, is refused
    /// with a message over the form. Without Create new project folder the file goes into the root. The Information
    /// fields go into the project.
    #[test]
    fn names_refusals_and_information() {
        let dir = scratch("names");
        let root = dir.join("Projects");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Taken.flp"), b"x").unwrap();
        let mut app = App::new(None);
        app.open_save_form(None);
        type_text(&mut app, "a/b");
        click(&mut app, FHit::Main);
        assert_eq!(app.top_message().map(|m| m.0), Some(SAVE_TITLE.to_string()), "an invalid name");
        app.dialog_click(DHit::Button(0));
        for _ in 0..3 {
            app.dialog_key(&Key::Named(NamedKey::Backspace), false, false);
        }
        type_text(&mut app, "Taken");
        click(&mut app, FHit::Check(0));
        click(&mut app, FHit::Main);
        assert!(app.top_message().is_some_and(|m| m.1.contains("Taken.flp already exists")));
        app.dialog_click(DHit::Button(0));
        // The Information group: folded, its fields do not take the keys; unfolded, Tab goes through them.
        click(&mut app, FHit::Field(1));
        assert_eq!(form(&app).unwrap().focus, 0);
        click(&mut app, FHit::Fold);
        click(&mut app, FHit::Field(1));
        type_text(&mut app, "My song");
        app.dialog_key(&Key::Named(NamedKey::Tab), false, false);
        type_text(&mut app, "Me");
        app.dialog_key(&Key::Named(NamedKey::Tab), false, false);
        app.dialog_key(&Key::Named(NamedKey::Tab), false, false);
        type_text(&mut app, "Lo-fi");
        app.dialog_key(&Key::Named(NamedKey::Tab), false, false);
        app.dialog_key(&Key::Named(NamedKey::Backspace), false, false);
        type_text(&mut app, "Song");
        app.dialog_key(&Key::Named(NamedKey::Enter), false, false);
        assert!(app.state.dialogs.is_empty());
        assert_eq!(app.state.path, Some(root.join("Song.flp")), "into the root");
        let info = &app.state.project.info;
        assert_eq!((info.title.as_deref(), info.author.as_deref(), info.url.as_deref(), info.genre.as_deref()), (Some("My song"), Some("Me"), None, Some("Lo-fi")));
        done(&dir);
    }

    /// Skip opens the file dialog; Don't show this in the future sets Ask for data folder to Don't ask, even when the
    /// form is cancelled, and the next Save goes straight to the file dialog. The folder button's picker sets the
    /// root; Auto puts the user's projects folder back.
    #[test]
    fn skip_dont_show_and_the_root() {
        let dir = scratch("skip");
        let mut app = App::new(None);
        let picked = dir.join("elsewhere");
        std::fs::create_dir_all(&picked).unwrap();
        app.open_save_form(None);
        crate::dialog::fake::answer(Some(picked.clone()));
        click(&mut app, FHit::Folder);
        assert_eq!(form(&app).unwrap().root, picked);
        click(&mut app, FHit::Auto);
        assert_eq!(form(&app).unwrap().root, dir.join("Projects"));
        // Skip: the file dialog (cancelled at once in tests), nothing saved.
        let saves = crate::dialog::fake::shown().1;
        click(&mut app, FHit::Skip);
        assert_eq!(crate::dialog::fake::shown().1, saves + 1);
        assert!(app.state.dialogs.is_empty() && app.state.path.is_none());
        // Don't show, then Cancel.
        app.open_save_form(None);
        click(&mut app, FHit::Check(2));
        app.dialog_click(DHit::Cancel);
        assert_eq!(prefs::index(&prefs::ASK_DATA_FOLDER), DONT_ASK);
        app.act(Action::MenuItem(0, crate::views::menus::FILE_SAVE));
        assert!(app.state.dialogs.is_empty(), "no form");
        assert_eq!(crate::dialog::fake::shown().1, saves + 2, "the file dialog");
        prefs::set_choice(&prefs::ASK_DATA_FOLDER, 3);
        done(&dir);
    }

    /// When starting a new project only: FILE ▸ New opens New project over the new project, with the Project page's
    /// time settings; its lists pick the timebase and the signature. Cancel keeps the new project, unsaved; Create
    /// gives it the form's time settings and saves it as Save does.
    #[test]
    fn new_project_form() {
        let dir = scratch("new");
        prefs::set_choice(&prefs::ASK_DATA_FOLDER, 2);
        let mut app = App::new(None);
        app.act(Action::MenuItem(0, crate::views::menus::FILE_NEW));
        let f = form(&app).expect("the New project form").clone();
        assert_eq!((f.title(), f.main(), f.ppq, f.num, f.den), (NEW_TITLE, CREATE, app.state.project.ppq, 4, 4));
        click(&mut app, FHit::Skip);
        assert!(form(&app).is_some(), "no Skip");
        app.dialog_click(DHit::Cancel);
        assert!(app.state.dialogs.is_empty() && app.state.path.is_none());
        // Saving a project never saved does not ask with this choice.
        app.act(Action::MenuItem(0, crate::views::menus::FILE_SAVE));
        assert!(app.state.dialogs.is_empty());
        app.act(Action::MenuItem(0, crate::views::menus::FILE_NEW));
        // The lists: a click on a box opens it, an entry picks, a click elsewhere only closes it.
        click(&mut app, FHit::Combo(2));
        assert_eq!(form(&app).unwrap().list, Some(2));
        let (items, now) = form(&app).unwrap().list_items(2);
        assert_eq!((items.len(), items[now].as_str()), (16, "4"));
        click(&mut app, FHit::Item(2));
        click(&mut app, FHit::Combo(3));
        click(&mut app, FHit::Item(2));
        click(&mut app, FHit::Combo(1));
        click(&mut app, FHit::Shut);
        click(&mut app, FHit::Combo(1));
        let i = TIMEBASES.iter().position(|&v| v == 192).unwrap();
        click(&mut app, FHit::Item(i as u8));
        let f = form(&app).unwrap();
        assert_eq!((f.list, f.ppq, f.num, f.den), (None, 192, 3, 8));
        click(&mut app, FHit::Main);
        assert!(app.state.dialogs.is_empty());
        assert_eq!((app.state.project.ppq, app.state.project.bar()), (192, (3, 2)));
        let file = dir.join("Projects").join("Project_1").join("Project_1.flp");
        assert_eq!(app.state.path, Some(file.clone()));
        assert!(!app.unsaved());
        done(&dir);
        prefs::set_choice(&prefs::ASK_DATA_FOLDER, 3);
    }

    /// On startup: only the first choice shows New project once the program has started, and only over a new project.
    #[test]
    fn the_form_on_startup() {
        let mut app = App::new(None);
        app.startup_form();
        assert!(app.state.dialogs.is_empty(), "the default does not ask on startup");
        prefs::set_choice(&prefs::ASK_DATA_FOLDER, 0);
        app.startup_form();
        assert_eq!(form(&app).map(SaveForm::title), Some(NEW_TITLE));
        prefs::set_choice(&prefs::ASK_DATA_FOLDER, 3);
    }

    /// The form as painted: its title, its parts' hints as the spec has them, Skip on Save as only; an open list's
    /// entries over everything.
    #[test]
    fn the_forms_are_drawn() {
        let Ok(mut c) = os_ui::Canvas::new(1280, 900, 1.0) else { return };
        let dir = scratch("paint");
        let mut app = App::new(None);
        app.open_save_form(None);
        app.paint(&mut c, None);
        let hint = |app: &App, h: FHit| app.hits.iter().rev().find(|x| x.action == Action::Dlg(DHit::Form(h))).map(|x| x.hint.to_string());
        assert_eq!(hint(&app, FHit::Field(0)).as_deref(), Some(NAME_HINT));
        assert_eq!(hint(&app, FHit::Auto).as_deref(), Some(AUTO_HINT));
        assert_eq!(hint(&app, FHit::Folder).as_deref(), Some(FOLDER_HINT));
        for k in 0..3u8 {
            assert_eq!(hint(&app, FHit::Check(k)).as_deref(), Some(CHECK_HINTS[usize::from(k)]));
        }
        assert_eq!(hint(&app, FHit::Combo(0)).as_deref(), Some(FILES_HINT));
        assert_eq!(hint(&app, FHit::Skip).as_deref(), Some(SKIP_HINT));
        assert_eq!(hint(&app, FHit::Main).as_deref(), Some(MAIN_HINT));
        assert!(hint(&app, FHit::Field(1)).is_none(), "Information is folded");
        let r = app.state.dialogs.last().unwrap().rect.get();
        assert_eq!((r.w, r.h), (485.0, 404.0));
        click(&mut app, FHit::Fold);
        click(&mut app, FHit::Combo(0));
        app.paint(&mut c, None);
        assert!(hint(&app, FHit::Field(4)).is_some(), "unfolded");
        assert!(app.hits.iter().any(|x| x.action == Action::Dlg(DHit::Form(FHit::Item(2)))));
        app.dialog_click(DHit::Cancel);
        app.open_new_form();
        app.paint(&mut c, None);
        let r = app.state.dialogs.last().unwrap().rect.get();
        assert_eq!((r.w, r.h), (485.0, 504.0));
        assert!(hint(&app, FHit::Skip).is_none() && hint(&app, FHit::Combo(3)).is_some());
        done(&dir);
    }
}
