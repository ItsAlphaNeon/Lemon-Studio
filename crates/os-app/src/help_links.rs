// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! HELP ▸ Help index, Tutorials ▸ and What's new (Lemon Studio's design): Lemon Studio has no help files or tutorials
//! of its own yet, so they open the project's pages in the default web browser: its README (what works and how to
//! start), the README's how-to sections one by one, and its merged pull requests (the changes, newest first).
//! Diagnostics writes a report (`crate::diagnostics`).

pub const HELP_INDEX_URL: &str = "https://github.com/ItsAlphaNeon/Lemon-Studio#readme";
pub const WHATS_NEW_URL: &str = "https://github.com/ItsAlphaNeon/Lemon-Studio/pulls?q=is%3Apr+is%3Amerged";

/// HELP ▸ Tutorials ▸: the README's how-to sections, as (entry, page).
pub const TUTORIALS: [(&str, &str); 4] = [
    ("Getting started", "https://github.com/ItsAlphaNeon/Lemon-Studio#getting-started"),
    ("Using Lemon Studio", "https://github.com/ItsAlphaNeon/Lemon-Studio#using-lemon-studio"),
    ("Scripting and assistants", "https://github.com/ItsAlphaNeon/Lemon-Studio#scripting-and-assistants"),
    ("Command-line tools", "https://github.com/ItsAlphaNeon/Lemon-Studio#command-line-tools"),
];

/// HELP ▸ Tutorials ▸'s entries.
pub fn tutorial_items() -> Vec<crate::playlist::PItem> {
    TUTORIALS.iter().map(|&(label, url)| crate::playlist::PItem::new(label, crate::playlist::Cmd::Web(url))).collect()
}

/// Opens `url` in the default web browser (tests: nothing).
pub fn open(url: &str) {
    if cfg!(test) {
        return;
    }
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[cfg(test)]
mod tests {
    use crate::views::menus::{item_at, works, HELP_MENU};

    /// Help index and What's new work and open their pages.
    #[test]
    fn help_index_and_whats_new_open_the_project_pages() {
        let mut app = crate::app::App::new(None);
        let at = |label: &str| (0..20).find(|&i| item_at(HELP_MENU, i).is_some_and(|it| it.label == label)).unwrap();
        for (label, url) in [("Help index", super::HELP_INDEX_URL), ("What's new", super::WHATS_NEW_URL)] {
            assert!(works(HELP_MENU, item_at(HELP_MENU, at(label)).unwrap()));
            app.menu_item(HELP_MENU, at(label));
            assert_eq!(app.state.message.as_deref(), Some(format!("Opening {url}").as_str()));
        }
    }

    /// Tutorials ▸ works: its submenu lists the README's how-to sections, each opening its page.
    #[test]
    fn tutorials_open_the_readme_sections() {
        use crate::views::menus::submenu;
        let mut app = crate::app::App::new(None);
        let at = (0..20).find(|&i| item_at(HELP_MENU, i).is_some_and(|it| it.label == "Tutorials")).unwrap();
        assert!(works(HELP_MENU, item_at(HELP_MENU, at).unwrap()));
        let items = submenu(HELP_MENU, "Tutorials").unwrap()(&app.state);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Getting started", "Using Lemon Studio", "Scripting and assistants", "Command-line tools"]);
        crate::playlist::run(&mut app, items[0].cmd.clone());
        assert_eq!(app.state.message.as_deref(), Some(format!("Opening {}", super::TUTORIALS[0].1).as_str()));
    }
}
