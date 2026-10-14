// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Native question dialogs. rfd shows them on Windows and macOS; on Linux rfd runs `zenity`, so when it is missing
//! KDE's `kdialog` is used instead (a stock KDE desktop has only kdialog).

/// The button a question was answered with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Cancel,
}

/// Runs `show`, which opens a blocking dialog (the UI thread waits for its answer). On Linux plugin windows are kept
/// above other windows while Lemon Studio is active, and the app cannot follow the focus while it waits: they step back
/// for the dialog's lifetime, or they could cover it.
pub fn modal<T>(show: impl FnOnce() -> T) -> T {
    #[cfg(target_os = "linux")]
    let _unpinned = crate::plugin_window::unpinned();
    show()
}

/// Shows `dlg` to pick an existing file. `None`: cancelled.
#[cfg(not(test))]
pub fn pick_file(dlg: rfd::FileDialog) -> Option<std::path::PathBuf> {
    modal(|| dlg.pick_file())
}

/// Shows `dlg` to pick a folder. `None`: cancelled.
#[cfg(not(test))]
pub fn pick_folder(dlg: rfd::FileDialog) -> Option<std::path::PathBuf> {
    modal(|| dlg.pick_folder())
}

/// Shows `dlg` to choose where to save. `None`: cancelled.
#[cfg(not(test))]
pub fn save_file(dlg: rfd::FileDialog) -> Option<std::path::PathBuf> {
    modal(|| dlg.save_file())
}

/// Tells the user something they need to read before going on (an information box with an OK button).
#[cfg(not(test))]
pub fn inform(title: &str, text: &str) {
    modal(|| tell(title, text))
}

#[cfg(test)]
pub use fake::{inform, pick_file, pick_folder, save_file};

/// Tests never open a file dialog: each one is counted and gets the answer set with `answer` (default: cancelled).
/// Information boxes are only recorded (`informed`).
#[cfg(test)]
pub mod fake {
    use std::{cell::RefCell, path::PathBuf};

    #[derive(Default)]
    struct Files {
        picks: u32,
        saves: u32,
        answer: Option<PathBuf>,
        told: Vec<String>,
    }

    thread_local! {
        static FILES: RefCell<Files> = RefCell::default();
    }

    /// The path the next file dialog on this thread returns.
    pub fn answer(path: Option<PathBuf>) {
        FILES.with_borrow_mut(|f| f.answer = path);
    }

    /// File dialogs shown on this thread so far: (open, save).
    pub fn shown() -> (u32, u32) {
        FILES.with_borrow(|f| (f.picks, f.saves))
    }

    pub fn pick_file(_: rfd::FileDialog) -> Option<PathBuf> {
        FILES.with_borrow_mut(|f| {
            f.picks += 1;
            f.answer.take()
        })
    }

    pub fn pick_folder(_: rfd::FileDialog) -> Option<PathBuf> {
        pick_file(rfd::FileDialog::new())
    }

    pub fn save_file(_: rfd::FileDialog) -> Option<PathBuf> {
        FILES.with_borrow_mut(|f| {
            f.saves += 1;
            f.answer.take()
        })
    }

    pub fn inform(title: &str, text: &str) {
        FILES.with_borrow_mut(|f| f.told.push(format!("{title}: {text}")));
    }

    /// Information boxes shown on this thread so far (title: text).
    pub fn informed() -> Vec<String> {
        FILES.with_borrow(|f| f.told.clone())
    }
}

#[cfg(not(test))]
fn tell(title: &str, text: &str) {
    #[cfg(all(unix, not(target_os = "macos")))]
    if !on_path("zenity") {
        if on_path("kdialog") {
            let _ = std::process::Command::new("kdialog").args(["--title", title, "--sorry", text]).status();
        }
        return;
    }
    rfd::MessageDialog::new().set_title(title).set_level(rfd::MessageLevel::Info).set_description(text).set_buttons(rfd::MessageButtons::Ok).show();
}

/// Asks a Yes / No / Cancel question (a warning). `None`: no dialog could be shown.
pub fn yes_no_cancel(title: &str, text: &str) -> Option<Answer> {
    modal(|| ask(title, text))
}

fn ask(title: &str, text: &str) -> Option<Answer> {
    #[cfg(all(unix, not(target_os = "macos")))]
    if !on_path("zenity") {
        return on_path("kdialog").then(|| kdialog(title, text)).flatten();
    }
    let answer = rfd::MessageDialog::new()
        .set_title(title)
        .set_level(rfd::MessageLevel::Warning)
        .set_description(text)
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show();
    Some(match answer {
        rfd::MessageDialogResult::Yes => Answer::Yes,
        rfd::MessageDialogResult::No => Answer::No,
        _ => Answer::Cancel,
    })
}

/// Whether `cmd` is an executable file in one of the `PATH` folders.
#[cfg(all(unix, not(target_os = "macos")))]
fn on_path(cmd: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|d| std::fs::metadata(d.join(cmd)).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

/// `kdialog --warningyesnocancel`: exit code 0 = Yes, 1 = No, 2 = Cancel (or closed).
#[cfg(all(unix, not(target_os = "macos")))]
fn kdialog(title: &str, text: &str) -> Option<Answer> {
    let status = std::process::Command::new("kdialog").args(["--title", title, "--warningyesnocancel", text]).status().ok()?;
    Some(match status.code() {
        Some(0) => Answer::Yes,
        Some(1) => Answer::No,
        _ => Answer::Cancel,
    })
}
