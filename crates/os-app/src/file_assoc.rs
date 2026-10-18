// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! General ▸ System ▸ "Associate project files with application" on Windows: the per-user registration that makes
//! `.flp` files open with this copy of Lemon Studio. Spec: ui/settings-choices.md "System: Associate project files with
//! application (2026-10-16)".
//!
//! The decisions are pure: `look` gathers what the registry holds through a reader it is given, `ticked` and `blocked`
//! say what the check shows, `tick` and `untick` say what to write. Only `reg` touches the real registry, and it only
//! writes from the settings page's click (`App::assoc_click`); nothing is written at startup. Under `cfg(test)` `reg`
//! is a made-up registry of the test thread's own, so tests never read or write the real one.

use crate::app::App;

/// Lemon Studio's own file type for `.flp` files (spec).
pub const PROG_ID: &str = "LemonStudio.Project";
/// The extension it registers.
pub const EXT: &str = ".flp";
/// The type's description and the open verb's name (spec).
const DESCRIPTION: &str = "Lemon Studio project";
const FRIENDLY_NAME: &str = "Lemon Studio";
/// The per-user classes, where every write goes (`Root::Classes`).
const CLASSES: &str = r"Software\Classes";
/// Windows' "Open with ▸ Always" choice for `.flp` (`Root::User`), value `ProgId`.
const USER_CHOICE: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.flp\UserChoice";
/// The hint bar's text when a `UserChoice` keeps another program after a tick (spec).
pub const BLOCKED_HINT: &str = "Windows opens .flp files with another program: choose Lemon Studio under Open with ▸ Choose another app";

/// Where a read looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Root {
    /// `HKEY_CURRENT_USER\Software\Classes`: the per-user classes (all writes go here).
    Classes,
    /// `HKEY_CURRENT_USER`.
    User,
    /// `HKEY_CLASSES_ROOT`: the per-user classes over the machine-wide ones.
    Merged,
}

/// One change under `HKEY_CURRENT_USER\Software\Classes` (`key` is relative to it; `name` "" is the default value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Sets a string value, creating the key.
    Set { key: String, name: &'static str, data: String },
    /// Removes a value (gone already is fine).
    DeleteValue { key: String, name: &'static str },
    /// Removes a key with everything under it (gone already is fine).
    DeleteTree { key: String },
}

/// What the registry holds for `.flp` (`look`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// The per-user `.flp` default value: the name of its file type.
    pub ext_type: Option<String>,
    /// That type's per-user open command.
    pub open_command: Option<String>,
    /// `LemonStudio.Project`'s `Previous`: the `.flp` default the tick replaced.
    pub previous: Option<String>,
    /// The "Open with ▸ Always" choice: its ProgId and that type's open command.
    pub user_choice: Option<(String, Option<String>)>,
}

/// Reads what the check depends on through `get(root, key, value name)` ("" = the key's default value; a value that is
/// missing, empty or not a string is `None`).
pub fn look(get: impl Fn(Root, &str, &str) -> Option<String>) -> Seen {
    let get = |root, key: &str, name| get(root, key, name).filter(|v| !v.is_empty());
    let ext_type = get(Root::Classes, EXT, "");
    let open_command = ext_type.as_deref().and_then(|t| get(Root::Classes, &format!(r"{t}\shell\open\command"), ""));
    let previous = get(Root::Classes, PROG_ID, "Previous");
    let user_choice = get(Root::User, USER_CHOICE, "ProgId").map(|id| {
        let command = get(Root::Merged, &format!(r"{id}\shell\open\command"), "");
        (id, command)
    });
    Seen { ext_type, open_command, previous, user_choice }
}

/// The path as Windows writes it in commands: without the `\\?\` prefix.
fn plain(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

/// A path compared as Windows does: case-insensitive, either slash.
fn norm(path: &str) -> String {
    plain(path).replace('/', "\\").to_lowercase()
}

/// Whether open command `command` runs `program`: its first word, quoted or not, is that program's path.
pub fn runs(command: &str, program: &str) -> bool {
    let command = command.trim_start();
    let program = norm(program);
    if let Some(rest) = command.strip_prefix('"') {
        return rest.split('"').next().is_some_and(|exe| norm(exe) == program);
    }
    // Unquoted: the path itself (it may hold spaces), then a space or the end.
    let command = norm(command);
    command.strip_prefix(&program).is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// Whether a `UserChoice` names a program other than `program`: then Windows keeps opening `.flp` files with it.
pub fn blocked(program: &str, seen: &Seen) -> bool {
    seen.user_choice.as_ref().is_some_and(|(id, command)| !id.eq_ignore_ascii_case(PROG_ID) && !command.as_deref().is_some_and(|c| runs(c, program)))
}

/// What the check shows (spec: "The check shows the truth"): the per-user `.flp` type's open command runs `program`
/// and no `UserChoice` names another program.
pub fn ticked(program: &str, seen: &Seen) -> bool {
    seen.open_command.as_deref().is_some_and(|c| runs(c, program)) && !blocked(program, seen)
}

/// Whether the per-user `.flp` default names Lemon Studio's type.
fn ours(seen: &Seen) -> bool {
    seen.ext_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(PROG_ID))
}

/// The writes of a tick for `program` (spec): the type first, then `.flp` pointed at it. `Previous` keeps the `.flp`
/// default it replaces (removed when there was none); a tick over Lemon Studio's own registration (another copy's, or
/// one a `UserChoice` overrides) keeps the `Previous` it has.
pub fn tick(program: &str, seen: &Seen) -> Vec<Op> {
    let exe = plain(program);
    let set = |key: String, name, data: String| Op::Set { key, name, data };
    let mut ops = Vec::new();
    if !ours(seen) {
        ops.push(match &seen.ext_type {
            Some(t) => set(PROG_ID.into(), "Previous", t.clone()),
            None => Op::DeleteValue { key: PROG_ID.into(), name: "Previous" },
        });
    }
    ops.extend([
        set(PROG_ID.into(), "", DESCRIPTION.into()),
        set(format!(r"{PROG_ID}\DefaultIcon"), "", format!("\"{exe}\",0")),
        set(format!(r"{PROG_ID}\shell\open"), "FriendlyAppName", FRIENDLY_NAME.into()),
        set(format!(r"{PROG_ID}\shell\open\command"), "", format!("\"{exe}\" \"%1\"")),
        set(EXT.into(), "", PROG_ID.into()),
        Op::Set { key: format!(r"{EXT}\OpenWithProgids"), name: PROG_ID, data: String::new() },
    ]);
    ops
}

/// The writes of an untick (spec): when `.flp` still names Lemon Studio's type its default goes back to `Previous`
/// (or is removed when there was none); `.flp`'s `OpenWithProgids` entry and the type itself are removed. Another
/// program's type is never touched.
pub fn untick(seen: &Seen) -> Vec<Op> {
    let mut ops = Vec::new();
    if ours(seen) {
        ops.push(match &seen.previous {
            Some(p) => Op::Set { key: EXT.into(), name: "", data: p.clone() },
            None => Op::DeleteValue { key: EXT.into(), name: "" },
        });
    }
    ops.push(Op::DeleteValue { key: format!(r"{EXT}\OpenWithProgids"), name: PROG_ID });
    ops.push(Op::DeleteTree { key: PROG_ID.into() });
    ops
}

/// This program's path (tests: a made-up one).
fn program() -> Option<String> {
    #[cfg(test)]
    return Some(tests::PROGRAM.into());
    #[cfg(not(test))]
    std::env::current_exe().ok().map(|p| plain(&p.to_string_lossy()).to_string())
}

/// The settings page's side.
impl App {
    /// Reads the check's state for General ▸ System (each time the page opens; spec: "It is read again whenever the
    /// page opens").
    pub(crate) fn assoc_read(&mut self) {
        self.state.settings.assoc = program().is_some_and(|p| ticked(&p, &look(reg::get)));
    }

    /// The check was clicked: ticked, the registration is removed; else it is written. Then the registry is read
    /// again: the check shows what Windows now says, and the hint bar why it stayed unticked when a `UserChoice` keeps
    /// another program.
    pub(crate) fn assoc_click(&mut self) {
        let label = crate::views::settings::ASSOCIATE_CHECK;
        let Some(program) = program() else {
            self.state.message = Some(format!("{label}: the program's own path is not known"));
            return;
        };
        let seen = look(reg::get);
        let was = ticked(&program, &seen);
        let ops = if was { untick(&seen) } else { tick(&program, &seen) };
        let written = reg::apply(&ops);
        let seen = look(reg::get);
        let now = ticked(&program, &seen);
        self.state.settings.assoc = now;
        self.state.message = Some(if !written {
            format!("{label}: Windows did not allow the change")
        } else if !was && blocked(&program, &seen) {
            BLOCKED_HINT.to_string()
        } else {
            format!("{label}: {}", if now { "on" } else { "off" })
        });
        self.invalidate();
    }
}

/// The real registry: reads anywhere, writes under `HKEY_CURRENT_USER\Software\Classes` only, then tells the shell.
#[cfg(not(test))]
mod reg {
    use super::{Op, Root, CLASSES};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegSetKeyValueW, HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn at(root: Root, key: &str) -> (HKEY, String) {
        match root {
            Root::Classes => (HKEY_CURRENT_USER, format!(r"{CLASSES}\{key}")),
            Root::User => (HKEY_CURRENT_USER, key.to_string()),
            Root::Merged => (HKEY_CLASSES_ROOT, key.to_string()),
        }
    }

    /// A string value (REG_SZ, or REG_EXPAND_SZ expanded); `None` when missing or of another type.
    pub fn get(root: Root, key: &str, name: &str) -> Option<String> {
        let (hkey, path) = at(root, key);
        let (k, n) = (wide(&path), wide(name));
        let mut bytes = 0u32;
        // SAFETY: `k` and `n` are NUL-terminated; a null data pointer asks only for the size.
        if unsafe { RegGetValueW(hkey, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), &mut bytes) } != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; bytes as usize / 2 + 1];
        let mut bytes = (buf.len() * 2) as u32;
        // SAFETY: `buf` holds `bytes` bytes.
        if unsafe { RegGetValueW(hkey, k.as_ptr(), n.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut bytes) } != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Applies `ops` in order and tells the shell that associations changed; false when one failed.
    pub fn apply(ops: &[Op]) -> bool {
        let gone = |r: u32| r == ERROR_SUCCESS || r == ERROR_FILE_NOT_FOUND;
        let mut ok = true;
        for op in ops {
            ok &= match op {
                Op::Set { key, name, data } => {
                    let (k, n, d) = (wide(&format!(r"{CLASSES}\{key}")), wide(name), wide(data));
                    // SAFETY: NUL-terminated strings; `d` is `d.len() * 2` bytes, its NUL included as REG_SZ wants.
                    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr(), REG_SZ, d.as_ptr().cast(), (d.len() * 2) as u32) == ERROR_SUCCESS }
                }
                Op::DeleteValue { key, name } => {
                    let (k, n) = (wide(&format!(r"{CLASSES}\{key}")), wide(name));
                    // SAFETY: NUL-terminated strings.
                    gone(unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, k.as_ptr(), n.as_ptr()) })
                }
                Op::DeleteTree { key } => {
                    let k = wide(&format!(r"{CLASSES}\{key}"));
                    // SAFETY: NUL-terminated string. The tree's contents, then the key itself.
                    gone(unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) }) && gone(unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, k.as_ptr()) })
                }
            };
        }
        // SAFETY: SHCNE_ASSOCCHANGED takes no items.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED as i32, SHCNF_IDLIST, null(), null()) };
        ok
    }
}

/// A made-up registry per test thread: `HKCU` and `HKLM` keys by path, case-insensitive. Tests never touch the real one.
#[cfg(test)]
mod reg {
    use super::{Op, Root, CLASSES};
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    thread_local! {
        /// (full key path, value name) → data, both lower-cased; and how often the shell was told.
        static REG: RefCell<(BTreeMap<(String, String), String>, usize)> = RefCell::new(Default::default());
    }

    fn full(root: &str, key: &str) -> String {
        format!(r"{root}\{key}").to_lowercase()
    }

    /// Sets a value: `root` is "HKCU" or "HKLM", `key` relative to it.
    pub fn put(root: &str, key: &str, name: &str, data: &str) {
        REG.with(|r| r.borrow_mut().0.insert((full(root, key), name.to_lowercase()), data.into()));
    }

    /// Removes a value.
    pub fn remove(root: &str, key: &str, name: &str) {
        REG.with(|r| r.borrow_mut().0.remove(&(full(root, key), name.to_lowercase())));
    }

    /// Every value as (key, name, data), sorted, and how often the shell was told.
    pub fn dump() -> (Vec<(String, String, String)>, usize) {
        REG.with(|r| {
            let r = r.borrow();
            (r.0.iter().map(|((k, n), d)| (k.clone(), n.clone(), d.clone())).collect(), r.1)
        })
    }

    fn value(key: &str, name: &str) -> Option<String> {
        REG.with(|r| r.borrow().0.get(&(key.to_lowercase(), name.to_lowercase())).cloned())
    }

    pub fn get(root: Root, key: &str, name: &str) -> Option<String> {
        match root {
            Root::Classes => value(&full("HKCU", &format!(r"{CLASSES}\{key}")), name),
            Root::User => value(&full("HKCU", key), name),
            Root::Merged => value(&full("HKCU", &format!(r"{CLASSES}\{key}")), name).or_else(|| value(&full("HKLM", &format!(r"{CLASSES}\{key}")), name)),
        }
    }

    pub fn apply(ops: &[Op]) -> bool {
        REG.with(|r| {
            let mut r = r.borrow_mut();
            for op in ops {
                match op {
                    Op::Set { key, name, data } => {
                        r.0.insert((full("HKCU", &format!(r"{CLASSES}\{key}")), name.to_lowercase()), data.clone());
                    }
                    Op::DeleteValue { key, name } => {
                        r.0.remove(&(full("HKCU", &format!(r"{CLASSES}\{key}")), name.to_lowercase()));
                    }
                    Op::DeleteTree { key } => {
                        let k = full("HKCU", &format!(r"{CLASSES}\{key}"));
                        let under = format!(r"{k}\");
                        r.0.retain(|(path, _), _| *path != k && !path.starts_with(&under));
                    }
                }
            }
            r.1 += 1;
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Action;
    use crate::views::settings::{self, SAct, ASSOCIATE_CHECK};
    use os_ui::{Canvas, Rect};

    /// The made-up program path `program` gives in tests.
    pub const PROGRAM: &str = r"C:\Programs\Lemon Studio\lemonstudio.exe";
    const OTHER: &str = r"C:\Program Files\Other DAW\other.exe";
    const FL_TYPE: &str = "Other.flp.1";

    fn seen() -> Seen {
        look(reg::get)
    }

    /// The other program's per-user registration, as found on the measured machine (spec).
    fn other_registration() {
        reg::put("HKCU", r"Software\Classes\.flp", "", FL_TYPE);
        reg::put("HKCU", r"Software\Classes\.flp\OpenWithProgids", FL_TYPE, "");
        reg::put("HKCU", &format!(r"Software\Classes\{FL_TYPE}\shell\open\command"), "", &format!("\"{OTHER}\" \"%1\""));
    }

    #[test]
    fn open_commands_name_their_program() {
        assert!(runs(&format!("\"{PROGRAM}\" \"%1\""), PROGRAM));
        assert!(runs(&format!("  \"{}\" \"%1\"", PROGRAM.to_uppercase()), PROGRAM), "case-insensitive");
        assert!(runs(&format!("\"{}\" \"%1\"", PROGRAM.replace('\\', "/")), PROGRAM), "either slash");
        assert!(runs(&format!("\"{PROGRAM}\" \"%1\""), &format!(r"\\?\{PROGRAM}")), "the \\\\?\\ prefix");
        assert!(runs(&format!("{PROGRAM} %1"), PROGRAM), "unquoted, with spaces in the path");
        assert!(runs(PROGRAM, PROGRAM));
        assert!(!runs(&format!("{PROGRAM}2 %1"), PROGRAM), "a longer name");
        assert!(!runs(&format!("\"{OTHER}\" \"%1\""), PROGRAM));
        assert!(!runs("", PROGRAM));
    }

    /// A clean registry: the tick writes the spec's keys, the type before `.flp`; then the check is ticked. The untick
    /// removes everything again.
    #[test]
    fn tick_and_untick_on_a_clean_registry() {
        assert_eq!(seen(), Seen::default());
        assert!(!ticked(PROGRAM, &seen()));
        let ops = tick(PROGRAM, &seen());
        let set = |key: &str, name, data: &str| Op::Set { key: key.into(), name, data: data.into() };
        assert_eq!(
            ops,
            [
                Op::DeleteValue { key: PROG_ID.into(), name: "Previous" },
                set(PROG_ID, "", "Lemon Studio project"),
                set(r"LemonStudio.Project\DefaultIcon", "", &format!("\"{PROGRAM}\",0")),
                set(r"LemonStudio.Project\shell\open", "FriendlyAppName", "Lemon Studio"),
                set(r"LemonStudio.Project\shell\open\command", "", &format!("\"{PROGRAM}\" \"%1\"")),
                set(".flp", "", PROG_ID),
                set(r".flp\OpenWithProgids", PROG_ID, ""),
            ]
        );
        assert!(reg::apply(&ops));
        let s = seen();
        assert_eq!((s.ext_type.as_deref(), s.previous.as_deref()), (Some(PROG_ID), None));
        assert!(ticked(PROGRAM, &s) && !ticked(OTHER, &s));
        assert_eq!(
            untick(&s),
            [
                Op::DeleteValue { key: ".flp".into(), name: "" },
                Op::DeleteValue { key: r".flp\OpenWithProgids".into(), name: PROG_ID },
                Op::DeleteTree { key: PROG_ID.into() },
            ]
        );
        reg::apply(&untick(&s));
        // What is left: the empty `.flp` and `OpenWithProgids` keys hold no values.
        assert_eq!(reg::dump(), (vec![], 2));
        assert!(!ticked(PROGRAM, &seen()));
    }

    /// Over another program's registration: the tick keeps its type name in `Previous`, the untick puts it back, and the
    /// other program's own keys are never touched.
    #[test]
    fn another_programs_registration_comes_back() {
        other_registration();
        let before = reg::dump().0;
        assert!(!ticked(PROGRAM, &seen()) && ticked(OTHER, &seen()));
        let ops = tick(PROGRAM, &seen());
        assert_eq!(ops[0], Op::Set { key: PROG_ID.into(), name: "Previous", data: FL_TYPE.into() });
        reg::apply(&ops);
        assert!(ticked(PROGRAM, &seen()) && !ticked(OTHER, &seen()));
        // A second tick (say from another copy of Lemon Studio) keeps `Previous`.
        let again = tick(r"D:\Lemon\lemonstudio.exe", &seen());
        assert!(!again.iter().any(|op| matches!(op, Op::Set { name: "Previous", .. } | Op::DeleteValue { name: "Previous", .. })));
        reg::apply(&again);
        assert!(!ticked(PROGRAM, &seen()), "now the other copy's");
        assert_eq!(seen().previous.as_deref(), Some(FL_TYPE));
        reg::apply(&tick(PROGRAM, &seen()));
        // Unticked: `.flp` names the other program's type again; its values are as they were.
        let s = seen();
        assert_eq!(untick(&s)[0], Op::Set { key: ".flp".into(), name: "", data: FL_TYPE.into() });
        reg::apply(&untick(&s));
        assert_eq!(reg::dump().0, before);
        assert!(ticked(OTHER, &seen()));
        // An untick while `.flp` names another type leaves `.flp` alone.
        assert!(!untick(&seen()).iter().any(|op| matches!(op, Op::Set { key, .. } | Op::DeleteValue { key, name: "" } if key == ".flp")));
    }

    /// "Open with ▸ Always" (`UserChoice`): naming another program it keeps the check unticked over a full registration
    /// (`blocked`); naming Lemon Studio's type, or a type whose command runs this program, it doesn't.
    #[test]
    fn a_user_choice_can_keep_another_program() {
        reg::apply(&tick(PROGRAM, &seen()));
        assert!(ticked(PROGRAM, &seen()) && !blocked(PROGRAM, &seen()));
        let choice = r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.flp\UserChoice";
        reg::put("HKCU", choice, "ProgId", FL_TYPE);
        reg::put("HKLM", &format!(r"Software\Classes\{FL_TYPE}\shell\open\command"), "", &format!("\"{OTHER}\" \"%1\""));
        assert_eq!(seen().user_choice, Some((FL_TYPE.into(), Some(format!("\"{OTHER}\" \"%1\"")))));
        assert!(blocked(PROGRAM, &seen()) && !ticked(PROGRAM, &seen()));
        reg::put("HKCU", choice, "ProgId", "lemonstudio.project");
        assert!(ticked(PROGRAM, &seen()), "Lemon Studio's own type, any case");
        reg::put("HKCU", choice, "ProgId", r"Applications\lemonstudio.exe");
        reg::put("HKCU", r"Software\Classes\Applications\lemonstudio.exe\shell\open\command", "", &format!("\"{PROGRAM}\" \"%1\""));
        assert!(ticked(PROGRAM, &seen()), "a type that runs this program");
        reg::put("HKCU", choice, "ProgId", "Unknown.Type");
        assert!(blocked(PROGRAM, &seen()), "a type without a command is another program's");
    }

    fn hit_rect(app: &App, a: &Action) -> Rect {
        app.hits.iter().rev().find(|h| &h.action == a).map(|h| h.rect).unwrap_or_else(|| panic!("no hit for {a:?}"))
    }

    fn click(app: &mut App, c: &mut Canvas, r: Rect) {
        app.mouse_move(r.cx(), r.cy());
        app.mouse_down();
        app.mouse_up();
        app.paint(c, None);
    }

    /// The settings page (made-up registry): the check shows what the registry says when the page opens, a click
    /// ticks or unticks and reads it again, and a `UserChoice` for another program keeps it unticked with the spec's
    /// hint.
    #[test]
    fn the_check_on_the_settings_page() {
        let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
        let mut app = App::new(None);
        app.open_settings(settings::GENERAL);
        app.state.settings.folded.extend(["Display", "Language", "Input", "Undo history"]);
        app.paint(&mut c, None);
        assert!(!app.state.settings.assoc);
        let key = Action::Settings(SAct::Check(ASSOCIATE_CHECK));
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(app.state.settings.assoc && ticked(PROGRAM, &seen()));
        assert_eq!(app.state.message.as_deref(), Some("Associate project files with application: on"));
        assert_eq!(reg::dump().1, 1, "the shell was told");
        click(&mut app, &mut c, r);
        assert!(!app.state.settings.assoc && seen() == Seen::default());
        assert_eq!(app.state.message.as_deref(), Some("Associate project files with application: off"));
        // Registered elsewhere meanwhile: read again when the page opens (and when its tab is chosen).
        reg::apply(&tick(PROGRAM, &seen()));
        app.state.settings.open = false;
        app.open_settings(settings::GENERAL);
        assert!(app.state.settings.assoc);
        reg::apply(&untick(&seen()));
        app.paint(&mut c, None);
        let audio = hit_rect(&app, &Action::Settings(SAct::Tab(settings::AUDIO)));
        click(&mut app, &mut c, audio);
        assert!(app.state.settings.assoc, "read on the General page only");
        let general = hit_rect(&app, &Action::Settings(SAct::Tab(settings::GENERAL)));
        click(&mut app, &mut c, general);
        assert!(!app.state.settings.assoc);
        // Another program's "Always": the tick is written, the check stays unticked and the hint bar says why.
        reg::put("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.flp\UserChoice", "ProgId", FL_TYPE);
        app.paint(&mut c, None);
        let r = hit_rect(&app, &key);
        click(&mut app, &mut c, r);
        assert!(!app.state.settings.assoc);
        assert_eq!(seen().ext_type.as_deref(), Some(PROG_ID));
        assert_eq!(app.state.message.as_deref(), Some(BLOCKED_HINT));
        // Once the choice is gone the check is ticked.
        reg::remove("HKCU", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.flp\UserChoice", "ProgId");
        app.open_settings(settings::GENERAL);
        assert!(app.state.settings.assoc);
    }
}
