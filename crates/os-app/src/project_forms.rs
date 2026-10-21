// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project's forms, shown as dialogs (`dialog::Body::Project`): Rename project (Settings ▸ Project ▸ Data folder ▸
//! Rename), and the Save as and New project forms (`crate::save_form`). What Rename does once answered:
//! `crate::data_folder`. `views::project_forms` and `views::save_form` paint them.
// spec: ui/data-folder.md §6, §7

use winit::keyboard::{Key, NamedKey};

use crate::app::App;
use crate::data_folder::{self as df, DThen};
use crate::dialog::{spans, Answer, Body, Field, Kind, Msg, Reply, Then};

/// A form on the dialog stack.
#[derive(Debug, Clone, PartialEq)]
pub enum Form {
    Rename(Rename),
    /// Save as or New project.
    Save(Box<crate::save_form::SaveForm>),
    /// FILE > Export audio (`crate::export_form`).
    Export(Box<crate::export_form::ExportForm>),
}

/// The parts of a form that take clicks (`dialog::DHit::Form`); Cancel and the caption's × are the dialog's own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FHit {
    /// A text field, by its place in the form (`Rename::FIELDS`; Save as: the name, then the Information fields).
    Field(u8),
    /// A check, by its place in the form (`Rename::CHECKS`, `save_form::CHECKS`).
    Check(u8),
    /// The main button: Rename, Save or Create.
    Main,
    /// Save as and New project: Skip, Auto, the folder button, a combo's box (`save_form::SaveForm::list`), an entry of
    /// the open list, the Information group's fold, and a click anywhere but the open list (it closes).
    Skip,
    Auto,
    Folder,
    Combo(u8),
    Item(u8),
    Fold,
    Shut,
    /// Export: a segment (row, segment) of a segmented row, a format's tile.
    Seg(u8, u8),
    Tile(u8),
}

/// Rename project (spec: ui/data-folder.md §6): the project's name; Rename data folder, with the folder's name; Use
/// the project name, which makes the folder's name follow the project's.
#[derive(Debug, Clone, PartialEq)]
pub struct Rename {
    pub name: Field,
    pub folder: Field,
    pub rename_folder: bool,
    pub use_name: bool,
    /// The focused field: 0 the project name, 1 the folder name.
    pub focus: u8,
}

impl Rename {
    pub const TITLE: &'static str = "Rename project";
    /// The fields' labels and the checks', by their places.
    pub const FIELDS: [&'static str; 2] = ["Project name", "Data folder name"];
    pub const CHECKS: [&'static str; 2] = ["Rename data folder", "Use the project name"];
    pub const MAIN: &'static str = "Rename";

    /// The form for a project named `name` whose data folder is named `folder`: the name selected, Rename data folder
    /// ticked, Use the project name not (open question in the report: its default was not seen).
    pub fn new(name: &str, folder: &str) -> Rename {
        let mut f = Rename { name: Field::default(), folder: Field::default(), rename_folder: true, use_name: false, focus: 0 };
        f.name.set(name);
        f.name.select_all();
        f.folder.set(folder);
        f
    }

    /// The folder's new name: the project's while Use the project name is ticked.
    pub fn folder_name(&self) -> &str {
        if self.use_name { &self.name.text } else { &self.folder.text }
    }

    /// Whether the folder's field can be typed in.
    pub fn folder_editable(&self) -> bool {
        self.rename_folder && !self.use_name
    }

    /// What is wrong with the names, if anything.
    fn problem(&self) -> Option<&'static str> {
        if !df::valid_name(&self.name.text) {
            return Some("Type a project name (without \\ / : * ? \" < > |).");
        }
        (self.rename_folder && !df::valid_name(self.folder_name())).then_some("Type a data folder name (without \\ / : * ? \" < > |).")
    }
}

/// The top dialog's form.
fn top(app: &mut App) -> Option<&mut Form> {
    match &mut app.state.dialogs.last_mut()?.body {
        Body::Project(f) => Some(f),
        _ => None,
    }
}

impl App {
    /// Settings ▸ Project ▸ Data folder ▸ Rename: the Rename project form (spec: ui/data-folder.md §6).
    pub(crate) fn open_rename(&mut self) {
        if !df::can_rename(&self.state) {
            return;
        }
        let name = crate::backup::project_name(self.state.path.as_deref());
        let folder = df::folder(&self.state).and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
        self.show_form(Body::Project(Form::Rename(Rename::new(&name, &folder))));
    }

    /// A click on a part of the top form; `Some` closes it with that answer.
    pub(crate) fn form_click(&mut self, h: FHit) -> Option<Reply> {
        if matches!(top(self), Some(Form::Save(_))) {
            return self.save_form_click(h);
        }
        if matches!(top(self), Some(Form::Export(_))) {
            return self.export_form_click(h);
        }
        let Some(Form::Rename(f)) = top(self) else { return None };
        match h {
            FHit::Field(0) => f.focus = 0,
            FHit::Field(_) if f.folder_editable() => f.focus = 1,
            FHit::Field(_) => {}
            FHit::Check(0) => {
                f.rename_folder = !f.rename_folder;
                if !f.folder_editable() {
                    f.focus = 0;
                }
            }
            FHit::Check(_) if f.rename_folder => {
                f.use_name = !f.use_name;
                if !f.folder_editable() {
                    f.focus = 0;
                }
            }
            FHit::Check(_) | FHit::Skip | FHit::Auto | FHit::Folder | FHit::Combo(_) | FHit::Item(_) | FHit::Fold | FHit::Shut | FHit::Seg(..) | FHit::Tile(_) => {}
            FHit::Main => return self.form_accept(),
        }
        self.invalidate();
        None
    }

    /// The form's main button (or Enter): its answer once its fields are right, else a message over it.
    fn form_accept(&mut self) -> Option<Reply> {
        let Some(Form::Rename(f)) = top(self) else { return None };
        match f.problem() {
            None => Some(Reply::Button(Answer::Ok)),
            Some(p) => {
                self.tell(Kind::Warning, Rename::TITLE, spans(&[p]));
                None
            }
        }
    }

    /// A key while a form is on top; `Some` closes it with that answer.
    pub(crate) fn form_key(&mut self, key: &Key, ctrl: bool, alt: bool) -> Option<Reply> {
        if matches!(top(self), Some(Form::Save(_))) {
            return self.save_form_key(key, ctrl, alt);
        }
        if matches!(top(self), Some(Form::Export(_))) {
            return self.export_form_key(key, ctrl, alt);
        }
        match key {
            Key::Named(NamedKey::Escape) => return Some(Reply::Button(Answer::Cancel)),
            Key::Named(NamedKey::Enter) => return self.form_accept(),
            _ => {}
        }
        let Some(Form::Rename(f)) = top(self) else { return None };
        match key {
            Key::Named(NamedKey::Tab) => {
                f.focus = if f.focus == 0 && f.folder_editable() { 1 } else { 0 };
                let fl = if f.focus == 0 { &mut f.name } else { &mut f.folder };
                fl.select_all();
            }
            _ => {
                let fl = if f.focus == 0 { &mut f.name } else { &mut f.folder };
                fl.key(key, ctrl, alt);
            }
        }
        self.invalidate();
        None
    }

    /// The top form closed with `reply`: Rename asks for confirmation (spec: ui/data-folder.md §6); Save as and New
    /// project: `App::save_form_closed`.
    pub(crate) fn form_closed(&mut self, f: Form, reply: Reply) {
        match f {
            Form::Save(f) => self.save_form_closed(*f, reply),
            Form::Export(f) => self.export_form_closed(*f, reply),
            Form::Rename(f) => {
                if reply != Reply::Button(Answer::Ok) {
                    return;
                }
                let folder = f.rename_folder.then(|| f.folder_name().trim().to_string());
                let text = if folder.is_some() {
                    "Are you sure you want to rename this project and its data folder?"
                } else {
                    "Are you sure you want to rename this project?"
                };
                let then = Then::DataFolder(DThen::Rename { name: f.name.text.trim().to_string(), folder });
                self.show_msg(Msg::new(Kind::Question, "Confirmation", spans(&[text]), &[Answer::Yes, Answer::No], then));
            }
        }
        self.invalidate();
    }
}
