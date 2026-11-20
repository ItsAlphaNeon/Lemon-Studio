// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Panics are kept in `Logs/panic.log` in the user folder: the time, the version, the thread, the message, where it
//! happened and the backtrace. Started from a desktop launcher, the program has no terminal, so a panic that closes it
//! would otherwise leave nothing to report. The hook before it still runs (it prints the panic as before).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Past this size the log starts again; the one before is kept as `panic.log.1`.
const MAX_BYTES: u64 = 1 << 20;

fn path() -> Option<PathBuf> {
    Some(os_paths::user_dir()?.join("Logs").join("panic.log"))
}

/// Keeps every panic from here on, in this process and its threads.
pub fn install() {
    let before = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(p) = path() {
            let _ = append(&p, &entry(info));
        }
        before(info);
    }));
}

fn entry(info: &std::panic::PanicHookInfo) -> String {
    let thread = std::thread::current();
    let at = info.location().map_or_else(|| "an unknown place".to_string(), |l| format!("{}:{}", l.file(), l.line()));
    format!(
        "{} Lemon Studio {} panicked on thread {} at {}: {}\n{}\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        crate::version::text(),
        thread.name().unwrap_or("(unnamed)"),
        at,
        message(info.payload()),
        std::backtrace::Backtrace::force_capture()
    )
}

/// A panic's message: the text it was given, else a placeholder.
fn message(payload: &dyn std::any::Any) -> &str {
    payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(String::as_str)).unwrap_or("(no message)")
}

/// Adds `text` to the log at `p`, starting it again past `MAX_BYTES`.
fn append(p: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(p).is_ok_and(|m| m.len() > MAX_BYTES) {
        std::fs::rename(p, p.with_extension("log.1"))?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(p)?.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_message_is_its_text() {
        assert_eq!(message(&"static text"), "static text");
        assert_eq!(message(&String::from("formatted")), "formatted");
        assert_eq!(message(&42u8), "(no message)");
    }

    /// Entries are added in turn; past `MAX_BYTES` the log starts again, the one before kept beside it.
    #[test]
    fn the_log_adds_entries_and_starts_again_when_large() {
        let dir = std::env::temp_dir().join(format!("lemon-panic-log-{}", std::process::id()));
        let p = dir.join("Logs").join("panic.log");
        append(&p, "first\n").unwrap();
        append(&p, "second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "first\nsecond\n");
        append(&p, &"x".repeat(MAX_BYTES as usize)).unwrap();
        append(&p, "after\n").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "after\n");
        assert!(std::fs::read_to_string(p.with_extension("log.1")).unwrap().starts_with("first\nsecond\n"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
