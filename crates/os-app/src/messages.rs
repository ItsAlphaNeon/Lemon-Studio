// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! General ▸ Hints and warnings. Manage messages: the messages that can be skipped, each an entry that is on (the
//! message shows) or off (it is skipped; a question then acts as if Ok was pressed), kept as a program setting. The
//! button opens their menu; a question's "Remember my choice" check, ticked when Ok is pressed, turns its entry off,
//! Cancel never does. Project data size warning: after a save, a notice when the file is over the chosen limit.
// spec: ui/settings-choices.md "Hints and warnings: Manage messages and Project data size warning (2026-10-16)"

use crate::app::{Action, App};
use crate::playlist::{Cmd, Mark, PItem, Popup};
use crate::prefs::{self, Check};
use crate::views::settings::SAct;

/// What a message is: a question asks before something happens; a notice only tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Question,
    Notice,
}

/// A message that can be skipped: its entry in the Manage messages menu.
#[derive(Debug, PartialEq)]
pub struct Message {
    /// Its setting: its key in `settings.txt`, its label in the menu, on by default. Not among `prefs::CHECKS`: it is
    /// no check on a settings page.
    pub check: Check,
    /// The menu group it is listed under (one of `GROUPS`; FL's group for the same message).
    pub group: &'static str,
    pub kind: Kind,
}

/// The menu's groups in its order (FL's names). A group without entries is left out.
pub const GROUPS: [&str; 5] = ["Audio", "Behavior", "File/Data", "Notifications", "Trial"];

/// The channel rack's delete confirmation: Delete..., Delete selected... and Purge unused audio clips
/// (`rack::ask_delete`).
pub const DELETE_CHANNEL: Message =
    Message { check: Check { key: "msg_delete_channel", label: "Delete channel", default: true }, group: "File/Data", kind: Kind::Question };

/// PATTERNS > Delete's confirmation (`patterns::run`).
pub const DELETE_PATTERN: Message =
    Message { check: Check { key: "msg_delete_pattern", label: "Delete pattern", default: true }, group: "File/Data", kind: Kind::Question };

/// The notice after a save over the Project data size warning (`App::size_notice`).
pub const PROJECT_DATA_SIZE: Message = Message {
    check: Check { key: "msg_project_data_size_warning", label: "Project data size warning", default: true },
    group: "File/Data",
    kind: Kind::Notice,
};

/// Every message that can be skipped.
pub const MESSAGES: &[&Message] = &[&DELETE_CHANNEL, &DELETE_PATTERN, &PROJECT_DATA_SIZE];

/// Whether message `m` shows (its entry is on).
pub fn on(m: &Message) -> bool {
    prefs::check(&m.check)
}

/// Turns message `m`'s entry on or off and keeps it.
pub fn set(m: &Message, on: bool) {
    prefs::set_check(&m.check, on);
}

/// A row of the Manage messages menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// Every entry on.
    ShowAll,
    /// The notices off; the questions stay on (they keep asking until answered with Remember my choice or turned off
    /// one by one).
    ShowNone,
    /// Toggles `MESSAGES[i]`.
    Toggle(usize),
}

/// Carries out a menu row; returns the message.
pub fn pick(p: Pick) -> String {
    match p {
        Pick::ShowAll => {
            MESSAGES.iter().for_each(|m| set(m, true));
            "Messages: all shown".into()
        }
        Pick::ShowNone => {
            MESSAGES.iter().filter(|m| m.kind == Kind::Notice).for_each(|m| set(m, false));
            "Messages: notices skipped, questions still ask".into()
        }
        Pick::Toggle(i) => {
            let Some(m) = MESSAGES.get(i) else { return String::new() };
            let now = prefs::toggle(&m.check);
            format!("{}: {}", m.check.label, if now { "shown" } else { "skipped" })
        }
    }
}

/// The menu: Show all, Show none, then each group that has entries (in `GROUPS` order): its header, which can't be
/// clicked, and its entries in alphabetical order, ticked brightly when on and drawn dim with a dim tick when off. The
/// first group header in the second half of the rows starts the menu's second column (none while one group fits).
pub fn menu() -> Vec<PItem> {
    let mut items = vec![PItem::new("Show all", Cmd::Messages(Pick::ShowAll)), PItem::new("Show none", Cmd::Messages(Pick::ShowNone))];
    for g in GROUPS {
        let mut entries: Vec<(usize, &Message)> = MESSAGES.iter().copied().enumerate().filter(|(_, m)| m.group == g).collect();
        if entries.is_empty() {
            continue;
        }
        entries.sort_by_key(|(_, m)| m.check.label);
        items.push(PItem::head(g));
        for (i, m) in entries {
            let shown = on(m);
            items.push(PItem::new(m.check.label, Cmd::Messages(Pick::Toggle(i))).mark(Mark::Check(shown)).dim(!shown));
        }
    }
    let half = items.len().div_ceil(2);
    if let Some(at) = (half..items.len()).find(|&i| items[i].head) {
        items.insert(at, PItem::col());
    }
    items
}

/// Project data size warning: the limit of each choice of `prefs::PROJECT_DATA_SIZE_WARNING` in MB (1024 × 1024
/// bytes); None: "Don't warn me".
pub const LIMIT_MB: [Option<u64>; 7] = [Some(50), Some(100), Some(250), Some(500), Some(1024), Some(1536), None];

const MB: u64 = 1024 * 1024;

/// A limit as the warning names it: "100 MB", "1 GB", "1.5 GB".
fn size_text(mb: u64) -> String {
    if mb >= 1024 {
        format!("{} GB", mb as f64 / 1024.0)
    } else {
        format!("{mb} MB")
    }
}

/// The notice after a save wrote a project file of `bytes` with the warning's choice `choice`: None at or under the
/// limit, and always with "Don't warn me".
pub fn size_warning(bytes: u64, choice: usize) -> Option<String> {
    let limit = LIMIT_MB.get(choice).copied().flatten()?;
    (bytes > limit * MB).then(|| {
        format!(
            "This project file is {} MB, over the {} warning set in Settings > General > Project data size warning.",
            bytes.div_ceil(MB),
            size_text(limit)
        )
    })
}

impl App {
    /// The Manage messages button was clicked: its menu under the button (at the mouse when the button was not
    /// painted).
    pub(crate) fn messages_menu(&mut self) {
        let button = Action::Settings(SAct::Button("Manage messages"));
        let (x, y) = self.hits.iter().rev().find(|h| h.action == button).map_or(self.mouse, |h| (h.rect.x, h.rect.bottom()));
        self.state.playlist.popup = Some(Popup { x, y: y + 1.0, items: menu(), sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// A save wrote a project file of `bytes`: the Project data size warning when the file is over its limit and its
    /// entry is on. The box's Remember my choice, ticked when Ok is pressed, turns the entry off.
    pub(crate) fn size_notice(&mut self, bytes: u64) {
        use crate::dialog::{spans, Answer, Msg, Then};
        if !on(&PROJECT_DATA_SIZE) {
            return;
        }
        let Some(text) = size_warning(bytes, prefs::index(&prefs::PROJECT_DATA_SIZE_WARNING)) else { return };
        let mut m = Msg::new(crate::dialog::Kind::Info, "Information", spans(&[&text]), &[Answer::Ok], Then::Nothing);
        m.remember = Some((&PROJECT_DATA_SIZE, false));
        self.show_msg(m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialog::DHit;
    use os_ui::{Canvas, Rect};

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn click(app: &mut App, c: &mut Canvas, r: Rect) {
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        app.paint(c, None);
    }

    /// A click on the control drawn for `a`.
    fn press(app: &mut App, c: &mut Canvas, a: &Action) {
        let r = hit_rect(app, a);
        click(app, c, r);
    }

    /// The entries' keys are unique, apart from every declared setting's, and their groups are FL's.
    #[test]
    fn entries_are_kept_under_their_own_keys() {
        let mut keys: Vec<&str> = prefs::CHECKS.iter().map(|c| c.key).collect();
        keys.extend(prefs::COMBOS.iter().map(|c| c.key));
        keys.extend(prefs::NUMBERS.iter().map(|n| n.key));
        keys.extend(prefs::SLIDERS.iter().map(|s| s.key));
        for m in MESSAGES {
            assert!(GROUPS.contains(&m.group), "{}: one of FL's groups", m.check.label);
            assert!(!keys.contains(&m.check.key), "key {} declared twice", m.check.key);
            assert!(m.check.default, "{} is on at first", m.check.label);
            assert!(!m.check.label.contains("FL Studio"));
            keys.push(m.check.key);
        }
        assert_eq!(LIMIT_MB.len(), prefs::PROJECT_DATA_SIZE_WARNING.choices.len());
    }

    /// The menu: Show all, Show none, the File/Data header (not clickable), then its entries in alphabetical order;
    /// an entry that is off has a dim tick and dim text.
    #[test]
    fn the_menu_lists_the_entries_with_their_marks() {
        set(&DELETE_PATTERN, false);
        let items = menu();
        let rows: Vec<(&str, bool, Mark, bool)> = items.iter().map(|it| (&*it.label, it.head, it.mark, it.dim)).collect();
        assert_eq!(
            rows,
            vec![
                ("Show all", false, Mark::None, false),
                ("Show none", false, Mark::None, false),
                ("File/Data", true, Mark::None, false),
                ("Delete channel", false, Mark::Check(true), false),
                ("Delete pattern", false, Mark::Check(false), true),
                ("Project data size warning", false, Mark::Check(true), false),
            ]
        );
        assert_eq!(items[4].cmd, Cmd::Messages(Pick::Toggle(1)));
    }

    /// Show all turns every entry on; Show none turns the notices off and leaves the questions on.
    #[test]
    fn show_all_and_show_none() {
        for m in MESSAGES {
            set(m, false);
        }
        pick(Pick::ShowAll);
        assert!(MESSAGES.iter().all(|m| on(m)));
        pick(Pick::ShowNone);
        assert!(!on(&PROJECT_DATA_SIZE));
        assert!(on(&DELETE_CHANNEL) && on(&DELETE_PATTERN), "questions stay on");
        pick(Pick::ShowAll);
        assert!(on(&PROJECT_DATA_SIZE));
    }

    /// The warning fires over the limit (MB = 1024 × 1024 bytes), stays quiet at or under it, and never fires with
    /// "Don't warn me".
    #[test]
    fn size_warning_over_the_limit_only() {
        let choices = prefs::PROJECT_DATA_SIZE_WARNING.choices;
        assert_eq!(choices, ["50MB", "100MB (default)", "250MB", "500MB", "1GB", "1.5GB", "Don't warn me"]);
        assert_eq!(choices[prefs::PROJECT_DATA_SIZE_WARNING.default], "100MB (default)");
        assert_eq!(
            size_warning(120 * MB, 1).as_deref(),
            Some("This project file is 120 MB, over the 100 MB warning set in Settings > General > Project data size warning.")
        );
        assert_eq!(size_warning(100 * MB, 1), None);
        assert_eq!(size_warning(100 * MB + 1, 1).map(|t| t.contains("101 MB")), Some(true));
        assert_eq!(size_warning(60 * MB, 0).map(|t| t.contains("over the 50 MB warning")), Some(true));
        assert_eq!(size_warning(1100 * MB, 4).map(|t| t.contains("over the 1 GB warning")), Some(true));
        assert_eq!(size_warning(1600 * MB, 5).map(|t| t.contains("over the 1.5 GB warning")), Some(true));
        assert_eq!(size_warning(1500 * MB, 5), None);
        assert_eq!(size_warning(u64::MAX / 2, 6), None, "Don't warn me");
    }

    /// The Manage messages button works: its menu opens under it; a click on an entry toggles it, closes the menu and
    /// is kept in the settings; the next time the entry shows its dim tick and dim text.
    #[test]
    fn a_toggle_closes_the_menu_and_is_kept() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(crate::views::settings::GENERAL);
        app.state.settings.folded.extend(["Display", "Language", "Input", "Undo history"]);
        app.paint(&mut c, None);
        assert!(!app.hits.iter().any(|h| h.action == Action::Placeholder("Manage messages")), "not greyed out");
        let b = hit_rect(&app, &Action::Settings(SAct::Button("Manage messages")));
        click(&mut app, &mut c, b);
        let popup = app.state.playlist.popup.as_ref().expect("its menu");
        assert_eq!((popup.x, popup.y), (b.x, b.bottom() + 1.0), "under the button");
        assert_eq!(popup.items, menu());
        let i = popup.items.iter().position(|it| it.label == "Delete channel").expect("listed");
        press(&mut app, &mut c, &Action::PlPopup(i));
        assert!(app.state.playlist.popup.is_none(), "the menu closes");
        assert!(!on(&DELETE_CHANNEL));
        assert_eq!(prefs::text("msg_delete_channel").as_deref(), Some("0"), "kept in settings.txt");
        click(&mut app, &mut c, b);
        let it = &app.state.playlist.popup.as_ref().expect("its menu").items[i];
        assert_eq!((it.mark, it.dim), (Mark::Check(false), true));
        press(&mut app, &mut c, &Action::PlPopup(i));
        assert!(on(&DELETE_CHANNEL));
    }

    /// After a save over the limit, an information message says how big the file is and names the setting. Its
    /// Remember my choice, ticked with Ok, turns the notice off; Ok alone keeps it; "Don't warn me" never warns; a
    /// small project's save says nothing.
    #[test]
    fn the_size_notice_after_a_save() {
        let Ok(mut c) = Canvas::new(1280, 800, 1.0) else { return };
        let mut app = App::new(None);
        let text = "This project file is 120 MB, over the 100 MB warning set in Settings > General > Project data size warning.";
        app.size_notice(120 * MB);
        assert_eq!(app.top_message().map(|(_, t)| t).as_deref(), Some(text));
        app.paint(&mut c, None);
        press(&mut app, &mut c, &Action::Dlg(DHit::Button(0)));
        assert!(app.top_message().is_none() && on(&PROJECT_DATA_SIZE), "Ok alone keeps it on");

        app.size_notice(120 * MB);
        app.paint(&mut c, None);
        press(&mut app, &mut c, &Action::Dlg(DHit::Remember));
        press(&mut app, &mut c, &Action::Dlg(DHit::Button(0)));
        assert!(app.top_message().is_none());
        assert!(!on(&PROJECT_DATA_SIZE), "Remember my choice + Ok: off");
        app.size_notice(120 * MB);
        assert!(app.top_message().is_none(), "skipped while off");

        set(&PROJECT_DATA_SIZE, true);
        prefs::set_choice(&prefs::PROJECT_DATA_SIZE_WARNING, 6);
        app.size_notice(u64::MAX / 4);
        assert!(app.top_message().is_none(), "Don't warn me");
        prefs::set_choice(&prefs::PROJECT_DATA_SIZE_WARNING, 0);
        app.size_notice(60 * MB);
        assert!(app.top_message().is_some_and(|(_, t)| t.contains("over the 50 MB warning")));
        app.state.dialogs.clear();

        let path = std::env::temp_dir().join(format!("os_size_notice_{}.flp", std::process::id()));
        app.save_to(path.clone());
        assert!(app.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")));
        assert!(app.top_message().is_none(), "a small project is under every limit");
        let _ = std::fs::remove_file(path);
        prefs::set_choice(&prefs::PROJECT_DATA_SIZE_WARNING, prefs::PROJECT_DATA_SIZE_WARNING.default);
    }
}
