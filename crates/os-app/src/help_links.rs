// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! HELP ▸ Help index and What's new (Lemon Studio's design): Lemon Studio has no help files of its own yet, so they
//! open the project's pages in the default web browser, its README (what works and how to start) and its merged pull
//! requests (the changes, newest first). Tutorials ▸ and Diagnostics stay greyed.

pub const HELP_INDEX_URL: &str = "https://github.com/ItsAlphaNeon/Lemon-Studio#readme";
pub const WHATS_NEW_URL: &str = "https://github.com/ItsAlphaNeon/Lemon-Studio/pulls?q=is%3Apr+is%3Amerged";

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

    /// Help index and What's new work and open their pages; Tutorials and Diagnostics stay greyed.
    #[test]
    fn help_index_and_whats_new_open_the_project_pages() {
        let mut app = crate::app::App::new(None);
        let at = |label: &str| (0..20).find(|&i| item_at(HELP_MENU, i).is_some_and(|it| it.label == label)).unwrap();
        for (label, url) in [("Help index", super::HELP_INDEX_URL), ("What's new", super::WHATS_NEW_URL)] {
            assert!(works(HELP_MENU, item_at(HELP_MENU, at(label)).unwrap()));
            app.menu_item(HELP_MENU, at(label));
            assert_eq!(app.state.message.as_deref(), Some(format!("Opening {url}").as_str()));
        }
        for label in ["Tutorials", "Diagnostics"] {
            assert!(!works(HELP_MENU, item_at(HELP_MENU, at(label)).unwrap()), "{label}");
        }
    }
}
