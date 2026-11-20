// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The program's choices that outlive a run: one file in the user folder, `settings.txt`, never the project. Every
//! app-wide choice the user makes and should find again at the next start is kept here under a stable key; a new
//! feature adds keys, not a file of its own.
//!
//! The file is `key=value` lines. A key's value is its last line's; setting one replaces its first line (dropping any
//! later one) or adds a line at the end; lines of keys this version does not know are written back as they were, so an
//! older or newer Lemon Studio keeps what the other wrote. A missing or unreadable value is the reader's default. The
//! file is read when its store is made (`SettingsStore::at`) and written whole whenever a value changes. A session has its
//! store (`Session::settings`); tests keep theirs in memory and never read or write the user folder.
//!
//! Keys so far: `toolbar_separators`, `toolbar_flat_buttons`, `toolbar_hint_bar` (`views::toolbar::Opts`),
//! `clock_mode`, `clock_middle_unit` (`clock::Clock`), `step_edit` (`crate::step_edit`), the `window_*` positions
//! (`crate::layout`), `factory_folder` (`crate::factory_folder`), the settings window's declared controls
//! (`crate::prefs`), and `recent_project_1` .. `recent_project_50` (`crate::recent`).

use std::sync::Mutex; // This code is a part of L e m o n S t u d i o.

/// The file's lines as read: lines of keys this version does not know are written back as they were.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Store {
    lines: Vec<String>,
}

impl Store {
    pub fn parse(text: &str) -> Store {
        Store { lines: text.lines().map(String::from).collect() }
    }

    fn text(&self) -> String {
        self.lines.iter().map(|l| format!("{l}\n")).collect()
    }

    /// The value of `key`: its last line's.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().rev().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim())
    }

    /// Sets `key` to `value`: its first line is replaced (and any later one dropped), or a line is added at the end.
    fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}={value}");
        let mut found = false;
        self.lines.retain_mut(|l| {
            if !l.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
                return true;
            }
            if found {
                return false;
            }
            found = true;
            l.clone_from(&line);
            true
        });
        if !found {
            self.lines.push(line);
        }
    }
}

/// The settings a session keeps: `settings.txt` of a folder (the user folder for Lemon Studio), or memory only (tests,
/// a frontend that keeps none). Clones share one store.
#[derive(Clone, Debug, Default)]
pub struct SettingsStore {
    store: std::sync::Arc<Mutex<Store>>,
    file: Option<std::path::PathBuf>,
}

impl SettingsStore {
    /// `settings.txt` in `dir`: read now, written whole whenever a value changes.
    pub fn at(dir: impl Into<std::path::PathBuf>) -> SettingsStore {
        let file = dir.into().join("settings.txt");
        let store = std::fs::read_to_string(&file).map(|t| Store::parse(&t)).unwrap_or_default();
        SettingsStore { store: std::sync::Arc::new(Mutex::new(store)), file: Some(file) }
    }

    /// Kept in memory only, empty at the start.
    pub fn memory() -> SettingsStore {
        SettingsStore::default()
    }

    /// The user folder's (`os_paths::user_dir`), in memory when there is none.
    pub fn user() -> SettingsStore {
        os_paths::user_dir().map_or_else(SettingsStore::memory, SettingsStore::at)
    }

    /// Runs `f` on the settings.
    pub fn read<R>(&self, f: impl FnOnce(&Store) -> R) -> R {
        f(&self.store.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Sets `key` to `value` (one line: line breaks become spaces) and keeps the file when that changed it.
    pub fn write(&self, key: &str, value: &str) {
        let value = value.replace(['\r', '\n'], " ");
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        if s.get(key) != Some(value.as_str()) {
            s.set(key, &value);
            self.keep(&s);
        }
    }

    /// Sets each key to its value as `write` does, keeping the file once. A key without a line stays without one when
    /// its value is empty.
    pub fn write_all(&self, pairs: &[(String, String)]) {
        let mut s = self.store.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = false;
        for (key, value) in pairs {
            let value = value.replace(['\r', '\n'], " ");
            if s.get(key).unwrap_or("") != value {
                s.set(key, &value);
                changed = true;
            }
        }
        if changed {
            self.keep(&s);
        }
    }

    /// `key`'s value as text, `None` when it is not set or empty.
    pub fn get(&self, key: &str) -> Option<String> {
        self.read(|s| s.get(key).filter(|v| !v.is_empty()).map(String::from))
    }

    /// A switch kept as `1` / `0` under `key`; `default` when it is not set or is something else.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        self.read(|s| match s.get(key) {
            Some("1") => true,
            Some("0") => false,
            _ => default,
        })
    }

    /// Keeps switch `key` as `1` / `0`.
    pub fn set_flag(&self, key: &str, on: bool) {
        self.write(key, if on { "1" } else { "0" });
    }

    /// Writes a whole new file beside the old one and then puts it in its place, so a crash part-way leaves the old
    /// file.
    fn keep(&self, s: &Store) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = file.with_extension("txt.tmp");
        if std::fs::write(&tmp, s.text()).is_ok() && std::fs::rename(&tmp, file).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file's rules: the last line of a key wins on reading; setting replaces its first line and drops the later
    /// ones, or adds a line; unknown lines and lines without `=` are written back as they were.
    #[test]
    fn lines_are_kept_and_the_last_wins() {
        let mut s = Store::parse("theme=dark\nnot a setting\ntoolbar_flat_buttons=0\nfuture_key = x \ntoolbar_flat_buttons=1\n");
        assert_eq!((s.get("toolbar_flat_buttons"), s.get("future_key"), s.get("missing")), (Some("1"), Some("x"), None));
        s.set("toolbar_flat_buttons", "0");
        s.set("clock_mode", "SMPTE 25 fps");
        assert_eq!(s.text(), "theme=dark\nnot a setting\ntoolbar_flat_buttons=0\nfuture_key = x \nclock_mode=SMPTE 25 fps\n");
        assert_eq!(Store::parse(&s.text()), s);
    }

    /// Switches read their default when unset or unreadable; values are one line; a store in memory starts empty.
    #[test]
    fn flags_and_text() {
        let st = SettingsStore::memory();
        assert!(st.flag("toolbar_separators", true) && !st.flag("toolbar_hint_bar", false));
        st.write("toolbar_hint_bar", "yes");
        assert!(!st.flag("toolbar_hint_bar", false), "not 1 or 0: the default");
        st.set_flag("toolbar_hint_bar", true);
        st.set_flag("toolbar_separators", false);
        assert!(st.flag("toolbar_hint_bar", false) && !st.flag("toolbar_separators", true));
        st.write("clock_mode", "two\nlines");
        assert_eq!(st.get("clock_mode").as_deref(), Some("two lines"));
        st.write("clock_mode", "");
        assert_eq!(st.get("clock_mode"), None);
    }

    /// A store at a folder reads its file when made and writes it whole on a change; a second store at the same folder
    /// reads what the first wrote.
    #[test]
    fn a_store_at_a_folder_keeps_its_file() {
        let dir = std::env::temp_dir().join(format!("lemon-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.txt"), "future_key=x\n").unwrap();
        let st = SettingsStore::at(&dir);
        assert_eq!(st.get("future_key").as_deref(), Some("x"));
        st.set_flag("toolbar_hint_bar", true);
        assert_eq!(std::fs::read_to_string(dir.join("settings.txt")).unwrap(), "future_key=x\ntoolbar_hint_bar=1\n");
        assert!(SettingsStore::at(&dir).flag("toolbar_hint_bar", false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
