// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Dialogs drawn in the main window: message boxes (as FL's message form) and the file dialog (`filedlg`). They replace
//! the system's own dialogs, which some Linux desktops do not have. They do not block: the caller says what happens
//! with the answer (`Then`), which `App::dialog_done` carries out once the dialog closes. While one is open it takes
//! every click, key, wheel turn and drop (`App::modal_open`); `views::dialog` paints them.
// spec: ui/dialogs.md

use std::cell::Cell;
use std::path::PathBuf;

use os_ui::Rect;
use winit::keyboard::{Key, NamedKey};

use crate::app::{App, Drag};
use crate::filedlg::{FileDlg, Focus, Outcome, SortBy, ROW_H};

/// The icon of a message box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Question,
    Warning,
    Error,
    Info,
}

/// A message box button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
    Ok,
}

impl Answer {
    pub fn label(self) -> &'static str {
        match self {
            Answer::Yes => "Yes",
            Answer::No => "No",
            Answer::Cancel => "Cancel",
            Answer::Ok => "Ok",
        }
    }
}

/// A button's accelerator: its label's first letter, pressed without Alt (spec §1e).
fn accel(label: &str) -> char {
    label.chars().next().map_or(' ', |c| c.to_ascii_lowercase())
}

/// The work that waits for a dialog's answer (`App::dialog_done`).
#[derive(Debug, Clone, PartialEq)]
pub enum Then {
    /// Nothing (an information box).
    Nothing,
    /// The save-changes question before `Next`: Yes saves first, No goes on, Cancel stays.
    Discard(Next),
    /// Where to save the project (Save as, or Save of a project without a file), then `Next` once it is saved.
    SaveProject(Option<Next>),
    /// The Export form's folder button (`App::export_folder_chosen`).
    ExportFolder,
    OpenProject,
    LoadTheme,
    /// The Arpeggiate dialog's folder button (spec: ui/piano-arpeggiate.md §1).
    ArpPattern,
    PluginFolder,
    /// Save preset as... for the plugin with this key (`plugins::Plugins::hosted`).
    SavePreset(usize),
    /// Load sample... for this rack row.
    RackSample(usize),
    /// The replace question of the Save dialog under it: Yes answers that dialog with this path.
    Replace(PathBuf),
    /// File ▸ Browser extra search folders: the folder for this row (`App::search_folder_chosen`).
    BrowserFolder(usize),
    /// Switch all / selected audio clip channels to ▸: Ok sets these rack rows to this stretch method (spec:
    /// ui/channel-switches.md §1).
    StretchClips(Vec<usize>, i32),
    /// The time signature conflict question for this pattern: Yes removes its start marker and opens the dialog
    /// (spec: ui/pattern-time-signature.md §3).
    ReplaceSignature(u16),
    /// The time signature dialog: Accept (Ok) puts numerator / denominator at the start of this pattern (§2).
    Signature(u16, u8, u8),
    /// The project's data folder: its questions and folder pickers (`crate::data_folder`).
    DataFolder(crate::data_folder::DThen),
}

/// What waits for the save-changes question.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Close the window (and the program).
    Close,
    /// macOS: Quit from the menu bar or the Dock (AppKit waits for the answer).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Quit,
    NewProject,
    /// FILE > Open: the open dialog.
    OpenDialog,
    /// A project dropped on the window.
    Open(PathBuf),
}

/// A dialog's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Button(Answer),
    /// The file or folder chosen; None: cancelled.
    File(Option<PathBuf>),
}

/// A message box (spec §3).
pub struct Msg {
    pub kind: Kind,
    pub title: String,
    /// Runs of text, the highlighted ones (names) marked; "\n" breaks the line.
    pub text: Vec<(String, bool)>,
    pub buttons: Vec<Answer>,
    /// The focused button (Enter / Space press it).
    pub focus: usize,
    pub then: Then,
    /// The "Remember my choice" check at the bottom left: the Manage messages entry it turns off when the box is
    /// answered Ok (or Yes) while it is ticked, and whether it is ticked. None: the box has no such check. Spec:
    /// ui/settings-choices.md "Hints and warnings: Manage messages and Project data size warning (2026-10-16)".
    pub remember: Option<(&'static crate::messages::Message, bool)>,
    /// The buttons' own labels, in the order of `buttons` (empty: the answers' labels, "Yes", "No", ...).
    pub labels: Vec<&'static str>,
}

impl Msg {
    pub fn new(kind: Kind, title: &str, text: Vec<(String, bool)>, buttons: &[Answer], then: Then) -> Msg {
        Msg { kind, title: title.into(), text, buttons: buttons.to_vec(), focus: 0, then, remember: None, labels: Vec::new() }
    }

    /// The same box with buttons labelled `labels` (in the order of its buttons) instead of the answers' names.
    pub fn labelled(mut self, labels: &[&'static str]) -> Msg {
        self.labels = labels.to_vec();
        self
    }

    /// Button `i`'s label.
    pub fn label(&self, i: usize) -> &'static str {
        self.labels.get(i).copied().or_else(|| self.buttons.get(i).map(|b| b.label())).unwrap_or("")
    }

    /// The button Esc and the caption's × press: Cancel, else No, else the last one.
    pub fn escape(&self) -> Answer {
        [Answer::Cancel, Answer::No].into_iter().find(|a| self.buttons.contains(a)).or(self.buttons.last().copied()).unwrap_or(Answer::Cancel)
    }

    /// The text without its marks.
    #[cfg(test)]
    pub fn plain(&self) -> String {
        self.text.iter().map(|(t, _)| t.as_str()).collect()
    }
}

/// Text runs for `Msg::text`: `parts` alternate plain and highlighted, starting plain.
pub fn spans(parts: &[&str]) -> Vec<(String, bool)> {
    parts.iter().enumerate().filter(|(_, p)| !p.is_empty()).map(|(i, p)| (p.to_string(), i % 2 == 1)).collect()
}

pub enum Body {
    Msg(Msg),
    File(Box<FileDlg>),
    /// General ▸ Input ▸ Manage shortcuts (`crate::shortcuts`).
    Shortcuts(crate::shortcuts::Form),
    /// PATTERNS > Set time signature... (`timesig`).
    TimeSig(crate::timesig::SigForm),
    /// Rename project, Save as and New project (`crate::project_forms`).
    Project(crate::project_forms::Form),
}

/// A dialog on the stack (`State::dialogs`, bottom to top; only the top one takes input).
pub struct Modal {
    pub body: Body,
    /// Top-left once dragged by its caption (None: centred in the window).
    pub at: Option<(f32, f32)>,
    /// Where it was painted last.
    pub rect: Cell<Rect>,
}

impl Modal {
    pub(crate) fn new(body: Body) -> Modal {
        Modal { body, at: None, rect: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)) }
    }
}

/// The parts of a dialog that take clicks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DHit {
    /// The dialog itself (clicks on nothing in particular), its caption (drag), the caption's ×.
    Body,
    Caption,
    Close,
    /// A message box button (index).
    Button(usize),
    /// A message box's Remember my choice check.
    Remember,
    /// File dialog: Back, Up, New folder, the path and name fields, a place, a row, a column header, the list's empty
    /// part, its scroll bar, the type combo and its entries, the main button and Cancel.
    Back,
    Up,
    NewFolder,
    PathField,
    NameField,
    Place(usize),
    Row(usize),
    Header(SortBy),
    List,
    Bar,
    Filter,
    FilterItem(usize),
    Accept,
    Cancel,
    /// Manage shortcuts: a row's box (opens its list), an entry of the open list, Reset all and Apply.
    Shortcut(usize),
    ShortcutChoice(usize),
    ResetAll,
    Apply,
    /// Time signature dialog: the denominator's value box (true) or the numerator's; Accept is `Accept`.
    SigValue(bool),
    /// Rename project, Save as and New project: their parts (`crate::project_forms`).
    Form(crate::project_forms::FHit),
}

/// A one-line text field: the text, the caret (in characters) and whether all of it is selected (typing replaces it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Field {
    pub text: String,
    pub caret: usize,
    pub all: bool,
}

impl Field {
    pub fn set(&mut self, s: &str) {
        self.text = s.into();
        self.caret = self.len();
        self.all = false;
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn select_all(&mut self) {
        self.all = !self.text.is_empty();
        self.caret = self.len();
    }

    /// Byte offset of character `i`.
    fn at(&self, i: usize) -> usize {
        self.text.char_indices().nth(i).map_or(self.text.len(), |(b, _)| b)
    }

    /// The text up to the caret (where it is drawn).
    pub fn before_caret(&self) -> &str {
        &self.text[..self.at(self.caret)]
    }

    /// Removes a selection; true when there was one.
    fn take_all(&mut self) -> bool {
        if !self.all {
            return false;
        }
        self.text.clear();
        self.caret = 0;
        self.all = false;
        true
    }

    pub fn insert(&mut self, s: &str) {
        self.take_all();
        let b = self.at(self.caret);
        self.text.insert_str(b, s);
        self.caret += s.chars().count();
    }

    /// The editing keys. True when the key was taken.
    pub fn key(&mut self, key: &Key, ctrl: bool, alt: bool) -> bool {
        match key {
            Key::Named(NamedKey::Backspace) => {
                if !self.take_all() && self.caret > 0 {
                    self.caret -= 1;
                    let b = self.at(self.caret);
                    self.text.remove(b);
                }
            }
            Key::Named(NamedKey::Delete) => {
                if !self.take_all() && self.caret < self.len() {
                    let b = self.at(self.caret);
                    self.text.remove(b);
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                self.caret = if self.all { 0 } else { self.caret.saturating_sub(1) };
                self.all = false;
            }
            Key::Named(NamedKey::ArrowRight) => {
                self.caret = if self.all { self.len() } else { (self.caret + 1).min(self.len()) };
                self.all = false;
            }
            Key::Named(NamedKey::Home) => {
                self.caret = 0;
                self.all = false;
            }
            Key::Named(NamedKey::End) => {
                self.caret = self.len();
                self.all = false;
            }
            Key::Named(NamedKey::Space) => self.insert(" "),
            Key::Character(c) if ctrl && !alt && c.eq_ignore_ascii_case("a") => self.select_all(),
            // Ctrl+Alt is AltGr on Windows: it types characters.
            Key::Character(c) if (!ctrl || alt) && !c.chars().any(char::is_control) => self.insert(c),
            _ => return false,
        }
        true
    }
}

/// What a key did in the file dialog.
enum FileKey {
    Nothing,
    Outcome(Outcome),
    Cancel,
}

fn file_key(f: &mut FileDlg, key: &Key, ctrl: bool, alt: bool) -> FileKey {
    use NamedKey as N;
    if f.filter_open {
        match key {
            Key::Named(N::Escape | N::Enter) => f.filter_open = false,
            Key::Named(N::ArrowUp) => f.set_filter(f.filter.saturating_sub(1)),
            Key::Named(N::ArrowDown) => f.set_filter(f.filter + 1),
            _ => {}
        }
        // `set_filter` closes the list; arrows keep it open.
        if matches!(key, Key::Named(N::ArrowUp | N::ArrowDown)) {
            f.filter_open = true;
        }
        return FileKey::Nothing;
    }
    match key {
        Key::Named(N::Escape) => return FileKey::Cancel,
        Key::Named(N::Enter) if f.focus == Focus::Path => f.path_enter(),
        Key::Named(N::Enter) => return FileKey::Outcome(f.accept()),
        Key::Named(N::Tab) => {
            f.focus = if f.focus == Focus::Name { Focus::Path } else { Focus::Name };
            if f.focus == Focus::Path {
                f.path.select_all();
            }
        }
        Key::Named(N::ArrowUp) if alt => f.up(),
        Key::Named(N::ArrowLeft) if alt => f.go_back(),
        Key::Named(N::ArrowUp) => f.step(-1),
        Key::Named(N::ArrowDown) => f.step(1),
        Key::Named(N::PageUp) => f.step(-f.page()),
        Key::Named(N::PageDown) => f.step(f.page()),
        Key::Named(N::Home) if f.list_focus => f.step(-(1 << 40)),
        Key::Named(N::End) if f.list_focus => f.step(1 << 40),
        Key::Named(N::F5) => f.reread(),
        // Backspace in an empty name field: the parent folder.
        Key::Named(N::Backspace) if f.focus == Focus::Name && f.name.text.is_empty() => f.up(),
        _ => {
            let name = f.focus == Focus::Name;
            let field = if name { &mut f.name } else { &mut f.path };
            if field.key(key, ctrl, alt) {
                f.status = None;
                if name {
                    f.list_focus = false;
                }
            }
        }
    }
    FileKey::Nothing
}

impl App {
    /// A dialog is open: it takes every click, key, wheel turn and drop.
    pub fn modal_open(&self) -> bool {
        !self.state.dialogs.is_empty()
    }

    /// Shows a message box over everything.
    pub(crate) fn show_msg(&mut self, m: Msg) {
        self.state.dialogs.push(Modal::new(Body::Msg(m)));
        self.modal_changed();
    }

    /// Shows a form of the app's own (the time signature dialog) over everything.
    pub(crate) fn show_form(&mut self, body: Body) {
        self.state.dialogs.push(Modal::new(body));
        self.modal_changed();
    }

    /// A message box with Ok only.
    pub(crate) fn tell(&mut self, kind: Kind, title: &str, text: Vec<(String, bool)>) {
        self.show_msg(Msg::new(kind, title, text, &[Answer::Ok], Then::Nothing));
    }

    /// Shows a file dialog; its answer goes to its `then`.
    pub(crate) fn file_dialog(&mut self, dlg: FileDlg) {
        #[cfg(test)]
        if !fake::is_live() {
            let answer = fake::take(dlg.mode == crate::filedlg::Mode::Save);
            return self.dialog_done(dlg.then, Reply::File(answer));
        }
        self.state.dialogs.push(Modal::new(Body::File(Box::new(dlg.opened()))));
        self.modal_changed();
    }

    /// Closes the top dialog with `reply` and carries out what waited for it.
    fn answer(&mut self, reply: Reply) {
        let Some(m) = self.state.dialogs.pop() else { return };
        if matches!(self.drag, Some(Drag::Dialog { .. } | Drag::DialogBar { .. })) {
            self.drag = None;
        }
        self.modal_changed();
        let then = match m.body {
            Body::Msg(m) => {
                // Ok with Remember my choice ticked turns the message off; Cancel never does.
                if let Some((entry, true)) = m.remember {
                    if matches!(reply, Reply::Button(Answer::Ok | Answer::Yes)) {
                        crate::messages::set(entry, false);
                    }
                }
                m.then
            }
            Body::File(f) => {
                if let Reply::File(Some(p)) = &reply {
                    f.remember(p);
                }
                f.then
            }
            Body::Shortcuts(f) => return self.shortcuts_closed(&f, reply == Reply::Button(Answer::Ok)),
            Body::Project(f) => return self.form_closed(f, reply),
            Body::TimeSig(f) => {
                // Its value list goes with it.
                self.state.playlist.popup = None;
                Then::Signature(f.pattern, f.num, f.den)
            }
        };
        // The replace question answers the Save dialog under it.
        if let Then::Replace(p) = then {
            if reply == Reply::Button(Answer::Yes) {
                self.answer(Reply::File(Some(p)));
            }
            return;
        }
        self.dialog_done(then, reply);
    }

    /// What Esc and the caption's × answer.
    fn escape_reply(&self) -> Option<Reply> {
        Some(match &self.state.dialogs.last()?.body {
            Body::Msg(m) => Reply::Button(m.escape()),
            Body::File(_) => Reply::File(None),
            Body::Shortcuts(_) => Reply::Button(Answer::Cancel),
            Body::TimeSig(_) => Reply::Button(Answer::Cancel),
            Body::Project(_) => Reply::Button(Answer::Cancel),
        })
    }

    fn top_file(&mut self) -> Option<&mut FileDlg> {
        match &mut self.state.dialogs.last_mut()?.body {
            Body::File(f) => Some(f),
            Body::Msg(_) | Body::Shortcuts(_) | Body::TimeSig(_) | Body::Project(_) => None,
        }
    }

    fn file_outcome(&mut self, o: Outcome) {
        match o {
            Outcome::Stay => {}
            Outcome::Done(p) => self.answer(Reply::File(Some(p))),
            Outcome::Replace(p) => {
                let name = p.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                let mut m = Msg::new(Kind::Warning, "Confirm save as", spans(&["", &name, " already exists.\nDo you want to replace it?"]), &[Answer::Yes, Answer::No], Then::Replace(p));
                // Replacing loses the file: Enter alone does not.
                m.focus = 1;
                self.show_msg(m);
            }
        }
    }

    /// A click on a part of the top dialog.
    pub(crate) fn dialog_click(&mut self, h: DHit) {
        self.invalidate();
        let (x, y) = self.mouse;
        let Some(m) = self.state.dialogs.last_mut() else { return };
        let r = m.rect.get();
        // The type list closes on a click anywhere else.
        if let Body::File(f) = &mut m.body {
            if f.filter_open && !matches!(h, DHit::FilterItem(_) | DHit::Filter) {
                f.filter_open = false;
                return;
            }
        }
        // So does a Manage shortcuts list.
        if let Body::Shortcuts(f) = &mut m.body {
            if f.open.is_some() && !matches!(h, DHit::ShortcutChoice(_) | DHit::Shortcut(_)) {
                f.open = None;
                return;
            }
        }
        match h {
            DHit::Body | DHit::List if !matches!(m.body, Body::File(_)) => {}
            DHit::Caption => self.drag = Some(Drag::Dialog { dx: x - r.x, dy: y - r.y }),
            DHit::Close | DHit::Cancel => {
                if let Some(reply) = self.escape_reply() {
                    self.answer(reply);
                }
            }
            DHit::Remember => {
                if let Body::Msg(Msg { remember: Some((_, ticked)), .. }) = &mut m.body {
                    *ticked = !*ticked;
                }
            }
            DHit::Button(i) => {
                let a = match &m.body {
                    Body::Msg(msg) => msg.buttons.get(i).copied(),
                    Body::File(_) | Body::Shortcuts(_) | Body::TimeSig(_) | Body::Project(_) => None,
                };
                if let Some(a) = a {
                    self.answer(Reply::Button(a));
                }
            }
            // Manage shortcuts: Apply keeps the form and closes; a box opens or closes its list; an entry is picked
            // (a swap within its group, `Form::pick`); Reset all fills in the defaults.
            DHit::Apply => self.answer(Reply::Button(Answer::Ok)),
            DHit::Shortcut(_) | DHit::ShortcutChoice(_) | DHit::ResetAll => {
                let Body::Shortcuts(f) = &mut m.body else { return };
                match h {
                    DHit::Shortcut(row) => f.open = if f.open == Some(row) { None } else { Some(row) },
                    DHit::ShortcutChoice(v) => {
                        if let Some(row) = f.open.take() {
                            f.pick(row, v);
                        }
                    }
                    _ => f.reset(),
                }
            }
            // The time signature dialog (spec: ui/pattern-time-signature.md §1-2).
            DHit::SigValue(den) => self.sig_list(den),
            DHit::Accept if matches!(m.body, Body::TimeSig(_)) => self.answer(Reply::Button(Answer::Ok)),
            // Rename project, Save as and New project: a part that closes the form answers it.
            DHit::Form(h) => {
                if let Some(reply) = self.form_click(h) {
                    self.answer(reply);
                }
            }
            _ => {
                let Some(f) = self.top_file() else { return };
                let out = match h {
                    DHit::Back => {
                        f.go_back();
                        Outcome::Stay
                    }
                    DHit::Up => {
                        f.up();
                        Outcome::Stay
                    }
                    DHit::NewFolder => {
                        f.new_folder();
                        Outcome::Stay
                    }
                    DHit::PathField => {
                        if f.focus != Focus::Path {
                            f.focus = Focus::Path;
                            f.path.select_all();
                        }
                        Outcome::Stay
                    }
                    DHit::NameField => {
                        f.focus = Focus::Name;
                        f.list_focus = false;
                        Outcome::Stay
                    }
                    DHit::Place(i) => {
                        if let Some(p) = f.places.get(i).map(|p| p.path.clone()) {
                            f.go(p);
                        }
                        Outcome::Stay
                    }
                    DHit::Row(i) => f.click_row(i),
                    DHit::Header(by) => {
                        f.sort_by(by);
                        Outcome::Stay
                    }
                    DHit::List => {
                        f.sel = None;
                        f.list_focus = false;
                        Outcome::Stay
                    }
                    DHit::Bar => {
                        let v = f.view.get();
                        let total = f.entries.len() as f32 * ROW_H;
                        let ratio = if v.h > 0.0 { total / v.h } else { 0.0 };
                        let s0 = f.scroll;
                        self.drag = Some(Drag::DialogBar { y0: y, s0, ratio });
                        Outcome::Stay
                    }
                    DHit::Filter => {
                        f.filter_open = !f.filter_open;
                        Outcome::Stay
                    }
                    DHit::FilterItem(i) => {
                        f.set_filter(i);
                        Outcome::Stay
                    }
                    DHit::Accept if f.focus == Focus::Path => {
                        f.path_enter();
                        Outcome::Stay
                    }
                    DHit::Accept => f.accept(),
                    _ => Outcome::Stay,
                };
                self.file_outcome(out);
            }
        }
    }

    /// A key press while a dialog is open (repeats included).
    pub(crate) fn dialog_key(&mut self, key: &Key, ctrl: bool, alt: bool) {
        self.invalidate();
        let shift = self.shift;
        let Some(m) = self.state.dialogs.last_mut() else { return };
        match &mut m.body {
            Body::Msg(msg) => {
                let n = msg.buttons.len().max(1);
                let press = match key {
                    Key::Named(NamedKey::Tab) => {
                        msg.focus = if shift { (msg.focus + n - 1) % n } else { (msg.focus + 1) % n };
                        None
                    }
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => {
                        msg.focus = msg.focus.saturating_sub(1);
                        None
                    }
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => {
                        msg.focus = (msg.focus + 1).min(n - 1);
                        None
                    }
                    Key::Named(NamedKey::Enter | NamedKey::Space) => msg.buttons.get(msg.focus).copied(),
                    Key::Named(NamedKey::Escape) => Some(msg.escape()),
                    Key::Character(c) if !ctrl => {
                        let c = c.chars().next().map(|c| c.to_ascii_lowercase());
                        (0..msg.buttons.len()).find(|&i| Some(accel(msg.label(i))) == c).map(|i| msg.buttons[i])
                    }
                    _ => None,
                };
                if let Some(a) = press {
                    self.answer(Reply::Button(a));
                }
            }
            Body::File(f) => match file_key(f, key, ctrl, alt) {
                FileKey::Nothing => {}
                FileKey::Outcome(o) => self.file_outcome(o),
                FileKey::Cancel => self.answer(Reply::File(None)),
            },
            // Manage shortcuts: Esc closes an open list, else the window (as its ×).
            Body::Shortcuts(f) => {
                if matches!(key, Key::Named(NamedKey::Escape)) && f.open.take().is_none() {
                    self.answer(Reply::Button(Answer::Cancel));
                }
            }
            // Esc closes an open value list first, then the dialog (as its ×); Enter presses Accept, its only button.
            Body::TimeSig(_) => {
                let list = self.state.playlist.popup.is_some();
                match key {
                    Key::Named(NamedKey::Escape) if list => self.state.playlist.popup = None,
                    Key::Named(NamedKey::Escape) => self.answer(Reply::Button(Answer::Cancel)),
                    Key::Named(NamedKey::Enter) if !list => self.answer(Reply::Button(Answer::Ok)),
                    _ => {}
                }
            }
            Body::Project(_) => {
                if let Some(reply) = self.form_key(key, ctrl, alt) {
                    self.answer(reply);
                }
            }
        }
    }

    /// The wheel over a dialog: scrolls the file list.
    pub(crate) fn dialog_wheel(&mut self, dy: f32) {
        if let Some(f) = self.top_file() {
            f.scroll -= dy * 3.0 * ROW_H;
            f.clamp_scroll();
            self.invalidate();
        }
    }

    /// A file dropped on the window while a dialog is open: the file dialog goes there.
    pub(crate) fn dialog_drop(&mut self, p: PathBuf) {
        if let Some(f) = self.top_file() {
            f.dropped(&p);
            self.invalidate();
        }
    }

    /// Drags the top dialog by its caption to top-left (x, y), kept inside the window.
    pub(crate) fn dialog_drag(&mut self, x: f32, y: f32) {
        let (sw, sh) = self.state.screen;
        if let Some(m) = self.state.dialogs.last_mut() {
            let r = m.rect.get();
            m.at = Some((x.min(sw - r.w).max(0.0).round(), y.min(sh - r.h).max(0.0).round()));
            self.invalidate();
        }
    }

    /// Drags the file list's scroll bar.
    pub(crate) fn dialog_scroll(&mut self, scroll: f32) {
        if let Some(f) = self.top_file() {
            f.scroll = scroll;
            f.clamp_scroll();
            self.invalidate();
        }
    }

    /// The top message box's title and text.
    #[cfg(test)]
    pub fn top_message(&self) -> Option<(String, String)> {
        match &self.state.dialogs.last()?.body {
            Body::Msg(m) => Some((m.title.clone(), m.plain())),
            Body::File(_) | Body::Shortcuts(_) | Body::TimeSig(_) | Body::Project(_) => None,
        }
    }
}

/// Tests answer file dialogs without showing them: each one is counted and gets the answer set with `answer` (default:
/// cancelled), and unsaved changes are not asked about. `live(true)` shows them as the user sees them instead.
#[cfg(test)]
pub mod fake {
    use std::{cell::RefCell, path::PathBuf};

    #[derive(Default)]
    struct Files {
        picks: u32,
        saves: u32,
        answer: Option<PathBuf>,
        live: bool,
    }

    thread_local! {
        static FILES: RefCell<Files> = RefCell::default();
    }

    /// The path the next file dialog on this thread returns.
    pub fn answer(path: Option<PathBuf>) {
        FILES.with_borrow_mut(|f| f.answer = path);
    }

    /// File dialogs answered on this thread so far: (open and folder, save).
    pub fn shown() -> (u32, u32) {
        FILES.with_borrow(|f| (f.picks, f.saves))
    }

    /// Dialogs on this thread are shown and answered through the UI; unsaved changes are asked about.
    pub fn live(on: bool) {
        FILES.with_borrow_mut(|f| f.live = on);
    }

    pub fn is_live() -> bool {
        FILES.with_borrow(|f| f.live)
    }

    pub(super) fn take(save: bool) -> Option<PathBuf> {
        FILES.with_borrow_mut(|f| {
            if save {
                f.saves += 1;
            } else {
                f.picks += 1;
            }
            f.answer.take()
        })
    }
}
