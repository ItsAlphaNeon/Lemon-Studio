// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Plugin presets: Lemon Studio's own preset store, used by every hosted plugin through its state save / load. The plugin
//! window's title-bar Presets box lists them (an app popup), its ◀ ▶ arrows step through them, "Save preset as..."
//! stores the current state.
//!
// spec: plugins/presets.md, ui/plugin-window.md (Presets box)

use std::path::{Path, PathBuf};

/// File extension of a preset.
pub const EXT: &str = "ospreset";
/// First line of every preset file.
const MAGIC: &[u8] = b"OSPRESET 1\n";

/// What a preset was saved from (the header line after the magic).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    /// Plugin name as the host shows it ("Lemon Sampler").
    pub plugin: String,
    /// The plugin's own id (CLAP id / VST3 class id), empty when unknown.
    pub id: String,
    /// Plugin format the state belongs to ("clap", "vst3").
    pub format: String,
}

/// One preset file in a plugin's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    /// Shown name: the file stem, with its sub-folder ("Bass/Deep") when it sits one level down.
    pub name: String,
    pub path: PathBuf,
}

/// `Presets` in the user folder (`os_paths::user_dir`; `OPENSTUDIO_PRESETS` overrides it, used by tests).
pub fn root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OPENSTUDIO_PRESETS") {
        return Some(PathBuf::from(p));
    }
    os_paths::user_file("Presets")
}

/// A plugin name as a folder name: characters Windows does not allow become `_`, trailing dots / spaces go.
pub fn folder_name(plugin: &str) -> String {
    let s: String = plugin.chars().map(|c| if "\\/:*?\"<>|".contains(c) || (c as u32) < 0x20 { '_' } else { c }).collect();
    let s = s.trim_end_matches(['.', ' ']).trim().to_string();
    if s.is_empty() { "_".into() } else { s }
}

/// The folder of a plugin's presets.
pub fn dir_for(plugin: &str) -> Option<PathBuf> {
    root().map(|r| dir_in(&r, plugin))
}

/// `plugin`'s folder in `root`. A built-in plugin's folder under an earlier name (the newest one there) becomes its
/// folder while it has none yet, so presets saved before a rename stay listed (spec: plugins/presets.md).
fn dir_in(root: &Path, plugin: &str) -> PathBuf {
    let dir = root.join(folder_name(plugin));
    for (old, _) in crate::plugins::OLD_NAMES.iter().filter(|(_, now)| *now == plugin) {
        let old = root.join(folder_name(old));
        if old.is_dir() && !dir.exists() {
            let _ = std::fs::rename(old, &dir);
        }
    }
    dir
}

/// The presets in `dir` and its direct sub-folders, sorted by shown name (case-insensitive, folders' presets after
/// the loose ones).
pub fn list_in(dir: &Path) -> Vec<Preset> {
    let mut out = Vec::new();
    let is_preset = |p: &Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT));
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if is_preset(&p) {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(Preset { name: stem, path: p });
        }
    }
    let mut nested = Vec::new();
    for d in subdirs {
        let folder = d.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() && is_preset(&p) {
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    nested.push(Preset { name: format!("{folder}/{stem}"), path: p });
                }
            }
        }
    }
    let key = |p: &Preset| p.name.to_lowercase();
    out.sort_by_key(key);
    nested.sort_by_key(key);
    out.extend(nested);
    out
}

/// The presets of `plugin`.
pub fn list(plugin: &str) -> Vec<Preset> {
    dir_for(plugin).map(|d| list_in(&d)).unwrap_or_default()
}

/// A preset file's bytes: magic line, one header line (`key=value` pairs separated by tabs), then the state.
pub fn encode(h: &Header, state: &[u8]) -> Vec<u8> {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut b = MAGIC.to_vec();
    b.extend_from_slice(format!("plugin={}\tid={}\tformat={}\n", clean(&h.plugin), clean(&h.id), clean(&h.format)).as_bytes());
    b.extend_from_slice(state);
    b
}

/// Splits a preset file into its header and the plugin state; `None` when it is not a preset file.
pub fn decode(bytes: &[u8]) -> Option<(Header, &[u8])> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let nl = rest.iter().position(|&c| c == b'\n')?;
    let line = std::str::from_utf8(&rest[..nl]).ok()?;
    let mut h = Header::default();
    for kv in line.split('\t') {
        match kv.split_once('=') {
            Some(("plugin", v)) => h.plugin = v.to_string(),
            Some(("id", v)) => h.id = v.to_string(),
            Some(("format", v)) => h.format = v.to_string(),
            _ => {}
        }
    }
    Some((h, &rest[nl + 1..]))
}

/// Writes a preset (creating its folder).
pub fn save(path: &Path, h: &Header, state: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, encode(h, state))
}

/// Reads a preset; an error text when the file is unreadable, not a preset, or made for another plugin (`id` given and
/// different from the file's).
pub fn load(path: &Path, id: &str) -> Result<(Header, Vec<u8>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (h, state) = decode(&bytes).ok_or_else(|| format!("{} is not a Lemon Studio preset", path.display()))?;
    if !id.is_empty() && !h.id.is_empty() && h.id != id {
        return Err(format!("this preset belongs to {}", if h.plugin.is_empty() { &h.id } else { &h.plugin }));
    }
    Ok((h, state.to_vec()))
}

/// The preset `dir` steps away from `current` in `list` (wraps around). Without a current preset (or one no longer in
/// the list) the first step goes to the first preset, a step back to the last.
pub fn step(list: &[Preset], current: Option<&Path>, dir: i32) -> Option<usize> {
    if list.is_empty() {
        return None;
    }
    let n = list.len() as i32;
    match current.and_then(|c| list.iter().position(|p| p.path == c)) {
        Some(i) => Some((i as i32 + dir).rem_euclid(n) as usize),
        None => Some(if dir < 0 { (n - 1) as usize } else { 0 }),
    }
}

/// A file name for a new preset: `name` cleaned like a folder name, with the preset extension.
pub fn file_for(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{}.{EXT}", folder_name(name)))
}

/// The name a preset file is shown with (its stem).
pub fn shown_name(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------------
// The app side: menus and commands. `key` is a `Plugins::hosted` key (rack row or mixer slot).

use crate::app::App;
use crate::playlist::{Cmd, Mark, PItem, Popup};

/// Preset commands (title-bar Presets box, ◀ ▶, window menu).
#[derive(Debug, Clone, PartialEq)]
pub enum PCmd {
    /// Load this preset file into the instance.
    #[cfg_attr(not(windows), allow(dead_code))]
    Load(usize, PathBuf),
    /// Load the next (+1) / previous (-1) preset of the plugin.
    Step(usize, i32),
    /// Store the instance's state in a new preset file (a save dialog in the plugin's folder).
    #[cfg_attr(not(windows), allow(dead_code))]
    SaveAs(usize),
}

/// The preset header of the instance under `key`.
fn header(app: &App, key: usize) -> Option<Header> {
    let d = app.plugins.descriptor(key)?;
    Some(Header { plugin: d.name.clone(), id: d.uid(), format: d.format.as_str().into() })
}

/// The preset list rows for the instance under `key`. `nested`: presets in sub-folders go into one submenu per folder
/// (the title-bar box); otherwise every preset is a row, sub-folder ones as "Folder/name" (the window menu's
/// Presets submenu, which cannot open a further submenu). The plugin window's popups are Windows-only so far.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn items(app: &App, key: usize, nested: bool) -> Vec<PItem> {
    let Some(h) = header(app, key) else { return Vec::new() };
    let cur = app.plugins.presets.get(&key);
    let row = |p: &Preset, label: String| PItem::new(label, Cmd::Preset(PCmd::Load(key, p.path.clone()))).mark(Mark::Check(cur == Some(&p.path)));
    let mut v: Vec<PItem> = Vec::new();
    let mut folders: Vec<(String, Vec<PItem>)> = Vec::new();
    for p in list(&h.plugin) {
        match p.name.split_once('/').filter(|_| nested) {
            Some((folder, stem)) => match folders.iter_mut().find(|(f, _)| f == folder) {
                Some((_, items)) => items.push(row(&p, stem.to_string())),
                None => folders.push((folder.to_string(), vec![row(&p, stem.to_string())])),
            },
            None => v.push(row(&p, p.name.clone())),
        }
    }
    for (folder, children) in folders {
        v.push(PItem { children, ..PItem::new(folder, Cmd::Nothing) });
    }
    if v.is_empty() {
        v.push(PItem::new("(no presets)", Cmd::Nothing).off());
    }
    // Long lists continue in further columns (the popup wraps at the screen's height).
    v
}

/// Fills the window menu's Presets section (spec: ui/plugin-window.md "Presets"): "Presets" becomes the list
/// (a submenu), "Save preset as..." works.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn fill_window_menu(app: &App, key: usize, menu: &mut Popup) {
    for it in menu.items.iter_mut() {
        match &*it.label {
            "Presets" if !it.head => *it = PItem { children: items(app, key, false), ..PItem::new("Presets", Cmd::Nothing) },
            "Save preset as..." => *it = PItem::new("Save preset as...", Cmd::Preset(PCmd::SaveAs(key))),
            _ => {}
        }
    }
}

/// The title-bar Presets box popup: the presets, then "Save preset as...".
#[cfg_attr(not(windows), allow(dead_code))]
pub fn popup(app: &App, key: usize, x: f32, y: f32) -> Popup {
    let mut items = items(app, key, true);
    items.push(PItem::sep());
    items.push(PItem::new("Save preset as...", Cmd::Preset(PCmd::SaveAs(key))));
    Popup { x, y, items, sub: None }
}

/// Runs a preset command; returns the status message.
pub fn run(app: &mut App, c: PCmd) -> Option<String> {
    let msg = match c {
        PCmd::Load(key, path) => load_into(app, key, &path),
        PCmd::Step(key, dir) => {
            let h = header(app, key)?;
            let l = list(&h.plugin);
            match step(&l, app.plugins.presets.get(&key).map(PathBuf::as_path), dir) {
                Some(i) => load_into(app, key, &l[i].path),
                None => Some(format!("No presets for {} yet (Save preset as... stores one)", h.plugin)),
            }
        }
        PCmd::SaveAs(key) => save_as(app, key),
    };
    app.invalidate();
    msg
}

fn load_into(app: &mut App, key: usize, path: &Path) -> Option<String> {
    let h = header(app, key)?;
    Some(match load(path, &h.id) {
        Ok((_, state)) if app.plugins.set_state(key, &state) => {
            app.plugins.mark_edited(key);
            app.plugins.set_preset(key, Some(path.to_path_buf()));
            format!("Preset: {}", shown_name(path))
        }
        Ok(_) => format!("{} did not take the preset {}", h.plugin, shown_name(path)),
        Err(e) => e,
    })
}

/// Save preset as...: the save dialog, in the plugin's preset folder (`save_chosen` writes the file).
fn save_as(app: &mut App, key: usize) -> Option<String> {
    let h = header(app, key)?;
    if app.plugins.state(key).is_none() {
        return Some(format!("{} has no state to save", h.plugin));
    }
    let dir = dir_for(&h.plugin)?;
    let _ = std::fs::create_dir_all(&dir);
    let start = app.plugins.presets.get(&key).map(|p| shown_name(p)).unwrap_or_else(|| "New preset".into());
    let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Save, "Save preset as", "preset", crate::dialog::Then::SavePreset(key))
        .filter("Lemon Studio preset", &[EXT])
        .start(Some(&dir))
        .name(&format!("{start}.{EXT}"));
    app.file_dialog(dlg);
    None
}

/// The preset file chosen in Save preset as...: the plugin's state is written there. Returns the status message.
pub fn save_chosen(app: &mut App, key: usize, target: PathBuf) -> Option<String> {
    let h = header(app, key)?;
    let Some(state) = app.plugins.state(key) else { return Some(format!("{} has no state to save", h.plugin)) };
    let target = if target.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT)) { target } else { target.with_extension(EXT) };
    Some(match save(&target, &h, &state) {
        Ok(()) => {
            app.plugins.set_preset(key, Some(target.clone()));
            format!("Saved preset {}", shown_name(&target))
        }
        Err(e) => format!("Preset not saved: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("os-presets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn preset_files_round_trip() {
        let h = Header { plugin: "Lemon Sampler".into(), id: "org.openstudio.open-sampler".into(), format: "clap".into() };
        let state = b"\x05\0\0\0\0\0\0\0{\"a\"}\n\x00binary";
        let b = encode(&h, state);
        let (h2, s2) = decode(&b).unwrap();
        assert_eq!(h2, h);
        assert_eq!(s2, state);
        assert!(decode(b"not a preset").is_none());
        let dir = tmp("rt");
        let p = file_for(&dir, "Deep: bass?");
        assert_eq!(p.file_name().unwrap(), "Deep_ bass_.ospreset");
        save(&p, &h, state).unwrap();
        assert_eq!(load(&p, "org.openstudio.open-sampler").unwrap().1, state);
        assert!(load(&p, "some.other.plugin").is_err(), "presets of another plugin are refused");
        assert!(load(&p, "").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listing_and_stepping() {
        let dir = tmp("list");
        let h = Header::default();
        for n in ["b", "A", "c"] {
            save(&file_for(&dir, n), &h, b"x").unwrap();
        }
        save(&file_for(&dir.join("Pads"), "warm"), &h, b"x").unwrap();
        std::fs::write(dir.join("readme.txt"), "x").unwrap();
        let l = list_in(&dir);
        let names: Vec<&str> = l.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["A", "b", "c", "Pads/warm"]);
        assert_eq!(step(&l, None, 1), Some(0));
        assert_eq!(step(&l, None, -1), Some(3));
        assert_eq!(step(&l, Some(&l[3].path), 1), Some(0), "wraps");
        assert_eq!(step(&l, Some(&l[0].path), -1), Some(3));
        assert_eq!(step(&[], None, 1), None);
        assert_eq!(folder_name("a/b:c. "), "a_b_c");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Presets saved under a built-in plugin's earlier names move to its folder; another plugin's folder stays.
    #[test]
    fn old_built_in_folders_are_taken_over() {
        let root = tmp("old");
        save(&file_for(&root.join("Open 3x Osc"), "Bass"), &Header::default(), b"x").unwrap();
        save(&file_for(&root.join("Lemon 3x Osc"), "Lead"), &Header::default(), b"x").unwrap();
        let dir = dir_in(&root, crate::osc3::LEMON_TRIO);
        assert_eq!(dir, root.join("Lemon Trio"));
        assert_eq!(list_in(&dir)[0].name, "Lead");
        assert!(root.join("Open 3x Osc").exists());
        save(&file_for(&root.join("Open Sampler"), "Kick"), &Header::default(), b"x").unwrap();
        save(&file_for(&root.join("Open Thing"), "Pad"), &Header::default(), b"x").unwrap();
        let dir = dir_in(&root, crate::plugins::LEMON_SAMPLER);
        assert_eq!(dir, root.join("Lemon Sampler"));
        assert_eq!(list_in(&dir).len(), 1);
        assert!(!root.join("Open Sampler").exists());
        assert_eq!(dir_in(&root, "Thing"), root.join("Thing"));
        assert!(root.join("Open Thing").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Save, list, step and load through a hosted Lemon Sampler: the state goes through the hosting layer and back.
    #[test]
    fn presets_of_a_hosted_plugin() {
        let dir = tmp("hosted");
        // SAFETY: tests in this process that read OPENSTUDIO_PRESETS all point it at a temp folder.
        unsafe { std::env::set_var("OPENSTUDIO_PRESETS", &dir) };
        let mut app = crate::app::App::new(None);
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).expect("built in");
        pl.create(0, index).unwrap();
        app.plugins = pl;
        let h = header(&app, 0).unwrap();
        assert_eq!(h.plugin, "Lemon Sampler");
        assert!(h.id.starts_with("clap:"));
        let state = app.plugins.state(0).unwrap();
        // Two presets: the default state, and one with a sample path.
        save(&file_for(&dir_for(&h.plugin).unwrap(), "Default"), &h, &state).unwrap();
        let other = crate::plugins::with_sample_path(&state, "C:/x/kick.wav").unwrap();
        save(&file_for(&dir_for(&h.plugin).unwrap(), "Kick"), &h, &other).unwrap();
        let names: Vec<String> = items(&app, 0, true).iter().map(|i| i.label.to_string()).collect();
        assert_eq!(names, ["Default", "Kick"]);
        assert!(run(&mut app, PCmd::Step(0, -1)).unwrap().contains("Kick"), "back from no preset = last");
        assert_eq!(app.plugins.sampler_path(0).as_deref(), Some("C:/x/kick.wav"));
        assert_eq!(app.plugins.preset_name(0).as_deref(), Some("Kick"));
        run(&mut app, PCmd::Step(0, 1));
        assert_eq!(app.plugins.sampler_path(0).as_deref(), Some(""), "wrapped to Default");
        assert!(items(&app, 0, true)[0].mark == Mark::Check(true));
        // The window menu's Presets entry lists them.
        let mut m = crate::playlist::plugin_window_menu(0, false, 0.0, 0.0);
        fill_window_menu(&app, 0, &mut m);
        let p = m.items.iter().find(|i| i.label == "Presets" && !i.head).unwrap();
        assert_eq!(p.children.len(), 2);
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Preset(PCmd::SaveAs(0))));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
