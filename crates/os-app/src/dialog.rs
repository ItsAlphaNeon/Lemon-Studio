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
