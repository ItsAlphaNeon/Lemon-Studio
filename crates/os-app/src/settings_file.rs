// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's settings: the store of its session (`os_session::settings_file`), one for the program, read through
//! these functions where no session is in reach (drawing, scaling, key names). The app gives its session this store
//! (`App::new`); each test thread has one of its own, in memory, so tests never read or write the user folder.

pub use os_session::settings_file::*;

#[cfg(not(test))]
static STORE: std::sync::OnceLock<SettingsStore> = std::sync::OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST: SettingsStore = SettingsStore::memory();
}

/// The program's store: the user folder's, read at the first use.
pub fn store() -> SettingsStore {
    #[cfg(test)]
    return TEST.with(SettingsStore::clone);
    #[cfg(not(test))]
    STORE.get_or_init(SettingsStore::user).clone()
}

/// Runs `f` on the settings.
pub fn read<R>(f: impl FnOnce(&Store) -> R) -> R {
    store().read(f)
}

/// Sets `key` to `value` (one line) and keeps the file when that changed it.
pub fn write(key: &str, value: &str) {
    store().write(key, value);
}

/// Sets each key to its value, keeping the file once.
pub fn write_all(pairs: &[(String, String)]) {
    store().write_all(pairs);
}

/// `key`'s value as text, `None` when it is not set or empty.
pub fn get(key: &str) -> Option<String> {
    store().get(key)
}

/// A switch kept as `1` / `0` under `key`; `default` when it is not set or is something else.
pub fn flag(key: &str, default: bool) -> bool {
    store().flag(key, default)
}

/// Keeps switch `key` as `1` / `0`.
pub fn set_flag(key: &str, on: bool) {
    store().set_flag(key, on);
}
