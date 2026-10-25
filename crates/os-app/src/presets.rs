// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin presets: Lemon Studio's own preset store, used by every hosted plugin through its state save / load. The plugin
//! window's title-bar Presets box lists them (an app popup), its ◀ ▶ arrows step through them, "Save preset as..."
//! stores the current state. Factory presets come compiled into the program; VST 3 and VST 2 preset files import; a
//! preset file dropped on a plugin window, a rack row or a mixer slot loads there.
//!
// spec: plugins/presets.md, ui/plugin-window.md (Presets box), ui/browser.md (Plugin presets)

use std::path::{Path, PathBuf};

/// File extension of a preset.
pub const EXT: &str = "ospreset";
/// The preset files Import preset... and drops take: Lemon Studio's own, VST 3's and VST 2's (program and bank).
pub const IMPORT_EXTS: &[&str] = &[EXT, "vstpreset", "fxp", "fxb"];
/// First line of every preset file.
const MAGIC: &[u8] = b"OSPRESET 1\n";
/// The `body` header value of a preset that lists parameter values instead of holding the plugin's state.
pub const PARAMS_BODY: &str = "params";

/// What a preset was saved from (the header line after the magic).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    /// Plugin name as the host shows it ("Lemon Sampler").
    pub plugin: String,
    /// The plugin's own id (CLAP id / VST3 class id), empty when unknown.
    pub id: String,
    /// Plugin format the state belongs to ("clap", "vst3").
    pub format: String,
    /// What follows the header: empty for the plugin's state, [`PARAMS_BODY`] for a parameter list.
    pub body: String,
}

/// What a preset holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// The plugin's saved state, exactly what `save_state` gave.
    State(Vec<u8>),
    /// Parameter values by name, in the plugin's own (plain) range; every parameter not listed goes to its default.
    Params(Vec<(String, f64)>),
}

/// One preset in a plugin's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    /// Shown name: the file stem, with its sub-folder ("Bass/Deep") when it sits one level down.
    pub name: String,
    pub path: PathBuf,
    /// One of the presets compiled into the program (read-only; its path is under [`FACTORY_ROOT`]).
    pub factory: bool,
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

/// Whether `p` names a preset file (`.ospreset`).
fn is_own(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT))
}

/// Whether `p` is a file Import preset... and drops take ([`IMPORT_EXTS`]).
pub fn is_preset_file(p: &Path) -> bool {
    p.extension().is_some_and(|e| IMPORT_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Sorts presets by shown name (case-insensitive), folders' presets after the loose ones.
fn sorted(mut loose: Vec<Preset>, mut nested: Vec<Preset>) -> Vec<Preset> {
    let key = |p: &Preset| p.name.to_lowercase();
    loose.sort_by_key(key);
    nested.sort_by_key(key);
    loose.extend(nested);
    loose
}

/// The presets in `dir` and its direct sub-folders, sorted by shown name (case-insensitive, folders' presets after
/// the loose ones).
pub fn list_in(dir: &Path) -> Vec<Preset> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            subdirs.push(p);
        } else if is_own(&p) {
            out.push(Preset { name: shown_name(&p), path: p, factory: false });
        }
    }
    let mut nested = Vec::new();
    for d in subdirs {
        let folder = d.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() && is_own(&p) {
                    nested.push(Preset { name: format!("{folder}/{}", shown_name(&p)), path: p, factory: false });
                }
            }
        }
    }
    sorted(out, nested)
}

// ---------------------------------------------------------------------------------------------------------------
// Factory presets: `factory-presets/<plugin>/[<folder>/]<name>.ospreset` in this crate, compiled in by `build.rs`.

/// (path under the factory folder, `/`-separated; file bytes).
static FACTORY: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/factory_presets.rs"));

/// The root of factory preset paths. `<` is not allowed in Windows file names (nor used in folder names elsewhere), so
/// no file on disk is ever taken for one.
pub const FACTORY_ROOT: &str = "<factory>";

/// Whether `p` is a factory preset's path.
pub fn is_factory(p: &Path) -> bool {
    p.starts_with(FACTORY_ROOT)
}

/// The factory presets of `plugin` in `files`, sorted as [`list_in`] sorts.
fn factory_in(files: &[(&str, &[u8])], plugin: &str) -> Vec<Preset> {
    let folder = folder_name(plugin);
    let (mut loose, mut nested) = (Vec::new(), Vec::new());
    for (rel, _) in files {
        let parts: Vec<&str> = rel.split('/').collect();
        if parts[0] != folder {
            continue;
        }
        let path = parts.iter().fold(PathBuf::from(FACTORY_ROOT), |p, s| p.join(s));
        let stem = shown_name(&path);
        match parts.len() {
            2 => loose.push(Preset { name: stem, path, factory: true }),
            3 => nested.push(Preset { name: format!("{}/{stem}", parts[1]), path, factory: true }),
            _ => {}
        }
    }
    sorted(loose, nested)
}

/// The factory presets of `plugin`: Lemon Studio's, then (for Citrus) the `.fst` files of an installed copy.
pub fn factory(plugin: &str) -> Vec<Preset> {
    let mut v = factory_in(FACTORY, plugin);
    v.extend(crate::citrus::more_presets(plugin));
    v
}

/// The plugin folders that have factory presets.
pub fn factory_plugins() -> Vec<&'static str> {
    let mut v: Vec<&str> = FACTORY.iter().filter_map(|(rel, _)| rel.split('/').next()).collect();
    v.dedup();
    v
}

/// A factory preset's bytes, by its path.
fn factory_bytes_in(files: &[(&str, &'static [u8])], path: &Path) -> Option<&'static [u8]> {
    let rel = path.strip_prefix(FACTORY_ROOT).ok()?;
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    files.iter().find(|(r, _)| r.split('/').eq(parts.iter().map(String::as_str))).map(|(_, b)| *b)
}

/// A preset file's bytes, from the disk or from the factory presets.
fn read(path: &Path) -> Result<Vec<u8>, String> {
    if is_factory(path) {
        return factory_bytes_in(FACTORY, path).map(<[u8]>::to_vec).ok_or_else(|| format!("{} is not a factory preset", shown_name(path)));
    }
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The presets of `plugin`: the user's, then the factory ones.
pub fn list(plugin: &str) -> Vec<Preset> {
    let mut v = user(plugin);
    v.extend(factory(plugin));
    v
}

/// The user's presets of `plugin` (its folder under [`root`]).
pub fn user(plugin: &str) -> Vec<Preset> {
    dir_for(plugin).map(|d| list_in(&d)).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------------
// The file format.

fn header_line(h: &Header) -> Vec<u8> {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut line = format!("plugin={}\tid={}\tformat={}", clean(&h.plugin), clean(&h.id), clean(&h.format));
    if !h.body.is_empty() {
        line.push_str(&format!("\tbody={}", clean(&h.body)));
    }
    let mut b = MAGIC.to_vec();
    b.extend_from_slice(line.as_bytes());
    b.push(b'\n');
    b
}

/// A preset file's bytes: magic line, one header line (`key=value` pairs separated by tabs), then the state.
pub fn encode(h: &Header, state: &[u8]) -> Vec<u8> {
    let mut b = header_line(&Header { body: String::new(), ..h.clone() });
    b.extend_from_slice(state);
    b
}

/// A parameter-list preset's bytes: the header with `body=params`, then one `name = value` line per parameter.
pub fn encode_params(h: &Header, params: &[(String, f64)]) -> Vec<u8> {
    let mut b = header_line(&Header { body: PARAMS_BODY.into(), ..h.clone() });
    for (name, v) in params {
        b.extend_from_slice(format!("{} = {v}\n", name.replace(['\n', '\r'], " ")).as_bytes());
    }
    b
}

/// Splits a preset file into its header and what follows; `None` when it is not a preset file.
pub fn decode(bytes: &[u8]) -> Option<(Header, &[u8])> {
    // A hand-edited file may have Windows line ends: the first two lines may end in "\r\n".
    let rest = bytes.strip_prefix(MAGIC).or_else(|| bytes.strip_prefix(b"OSPRESET 1\r\n"))?;
    let nl = rest.iter().position(|&c| c == b'\n')?;
    let line = std::str::from_utf8(&rest[..nl]).ok()?.trim_end_matches('\r');
    let mut h = Header::default();
    for kv in line.split('\t') {
        match kv.split_once('=') {
            Some(("plugin", v)) => h.plugin = v.to_string(),
            Some(("id", v)) => h.id = v.to_string(),
            Some(("format", v)) => h.format = v.to_string(),
            Some(("body", v)) => h.body = v.to_string(),
            _ => {}
        }
    }
    Some((h, &rest[nl + 1..]))
}

/// A parameter list's lines: `name = value` (the last `=` splits them), blank lines and `#` comments skipped.
pub fn parse_params(text: &str) -> Result<Vec<(String, f64)>, String> {
    let mut v = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parsed = line.rsplit_once('=').and_then(|(k, x)| Some((k.trim(), x.trim().parse::<f64>().ok()?)));
        match parsed {
            Some((k, x)) if !k.is_empty() && x.is_finite() => v.push((k.to_string(), x)),
            _ => return Err(format!("line {} is not \"parameter name = value\": {line}", n + 1)),
        }
    }
    Ok(v)
}

/// Writes a preset holding `state` (creating its folder).
pub fn save(path: &Path, h: &Header, state: &[u8]) -> std::io::Result<()> {
    save_body(path, h, &Body::State(state.to_vec()))
}

/// Writes a preset holding `body` (creating its folder).
pub fn save_body(path: &Path, h: &Header, body: &Body) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, match body {
        Body::State(s) => encode(h, s),
        Body::Params(p) => encode_params(h, p),
    })
}

/// Reads a preset (from the disk or the factory presets); an error text when the file is unreadable, not a preset,
/// or made for another plugin (`id` given and different from the file's).
pub fn load(path: &Path, id: &str) -> Result<(Header, Body), String> {
    let bytes = read(path)?;
    let (h, rest) = decode(&bytes).ok_or_else(|| format!("{} is not a Lemon Studio preset", shown_name(path)))?;
    if !id.is_empty() && !h.id.is_empty() && !h.id.eq_ignore_ascii_case(id) {
        return Err(format!("{} belongs to {}", shown_name(path), if h.plugin.is_empty() { &h.id } else { &h.plugin }));
    }
    let body = if h.body == PARAMS_BODY {
        let text = std::str::from_utf8(rest).map_err(|_| format!("{} is not text", shown_name(path)))?;
        Body::Params(parse_params(text).map_err(|e| format!("{}: {e}", shown_name(path)))?)
    } else {
        Body::State(rest.to_vec())
    };
    Ok((h, body))
}

/// The presets a file of any [`IMPORT_EXTS`] type holds for plugin `d`: (name, body), a bank's programs in order (a
/// program without a name is named after the file and its number). An error text when the file is not one, or is for
/// another plugin or format.
pub fn read_any(path: &Path, d: &os_host::Descriptor) -> Result<Vec<(String, Body)>, String> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let stem = shown_name(path);
    let other = || format!("{stem} belongs to another plugin");
    match ext.as_str() {
        "vstpreset" => {
            if d.format != os_host::Format::Vst3 {
                return Err(format!("{stem} is a VST 3 preset; {} is not a VST 3 plugin", d.name));
            }
            let bytes = read(path)?;
            let p = os_vst3::preset::Preset::parse(&bytes).ok_or_else(|| format!("{stem} is not a VST 3 preset"))?;
            if !p.class_id.eq_ignore_ascii_case(&d.id) {
                return Err(other());
            }
            Ok(vec![(stem, Body::State(bytes))])
        }
        "fxp" | "fxb" => {
            if d.format != os_host::Format::Vst2 {
                return Err(format!("{stem} is a VST 2 preset; {} is not a VST 2 plugin", d.name));
            }
            let f = os_vst2::fxp::parse(&read(path)?).ok_or_else(|| format!("{stem} is not a VST 2 preset"))?;
            if !os_vst2::id_text(f.unique_id).eq_ignore_ascii_case(&d.id) {
                return Err(other());
            }
            let bank = f.programs.len() > 1;
            Ok(f.programs
                .into_iter()
                .enumerate()
                .map(|(i, (name, s))| {
                    let name = match (name.is_empty(), bank) {
                        (false, _) => name,
                        (true, true) => format!("{stem} {}", i + 1),
                        (true, false) => stem.clone(),
                    };
                    (name, Body::State(s.to_bytes()))
                })
                .collect())
        }
        _ => load(path, &d.uid()).map(|(_, b)| vec![(stem, b)]),
    }
}

/// The identity (`Descriptor::uid`) of the plugin a preset file of any [`IMPORT_EXTS`] type is for, when it says.
pub fn uid_of(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let bytes = read(path).ok()?;
    match ext.as_str() {
        "vstpreset" => Some(format!("vst3:{}", os_vst3::preset::Preset::parse(&bytes)?.class_id)),
        "fxp" | "fxb" => Some(format!("vst2:{}", os_vst2::id_text(os_vst2::fxp::parse(&bytes)?.unique_id))),
        _ => decode(&bytes).map(|(h, _)| h.id).filter(|id| !id.is_empty()),
    }
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

/// [`file_for`], numbered ("Name 2", "Name 3", ...) while that file exists.
fn free_file(dir: &Path, name: &str) -> PathBuf {
    let mut p = file_for(dir, name);
    let mut n = 2;
    while p.exists() {
        p = file_for(dir, &format!("{name} {n}"));
        n += 1;
    }
    p
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
    /// Import preset...: an open dialog for a preset file of any [`IMPORT_EXTS`] type.
    #[cfg_attr(not(windows), allow(dead_code))]
    Import(usize),
    /// Rename preset...: the current preset under a new name (a save dialog in its folder).
    #[cfg_attr(not(windows), allow(dead_code))]
    Rename(usize),
    /// Delete preset...: the current preset's file goes, after a question.
    #[cfg_attr(not(windows), allow(dead_code))]
    Delete(usize),
    /// Browse presets: the browser's Plugin presets, opened at the plugin.
    #[cfg_attr(not(windows), allow(dead_code))]
    Browse(usize),
}

/// The preset header of the instance under `key`.
fn header(app: &App, key: usize) -> Option<Header> {
    let d = app.plugins.descriptor(key)?;
    Some(Header { plugin: d.name.clone(), id: d.uid(), format: d.format.as_str().into(), body: String::new() })
}

/// The current preset of the instance under `key` when it is a file of the user's presets (one that can be renamed and
/// deleted; factory presets and files dropped from elsewhere cannot).
fn own_current(app: &App, key: usize) -> Option<PathBuf> {
    let p = app.plugins.presets.get(&key)?;
    (!is_factory(p) && p.is_file() && root().is_some_and(|r| p.starts_with(r))).then(|| p.clone())
}

/// Rows of the presets in `list`: sub-folder presets in one submenu per folder when `nested`, else "Folder/name" rows.
fn rows(app: &App, key: usize, list: &[Preset], nested: bool) -> Vec<PItem> {
    let cur = app.plugins.presets.get(&key);
    let row = |p: &Preset, label: String| PItem::new(label, Cmd::Preset(PCmd::Load(key, p.path.clone()))).mark(Mark::Check(cur == Some(&p.path)));
    let mut v: Vec<PItem> = Vec::new();
    let mut folders: Vec<(String, Vec<PItem>)> = Vec::new();
    for p in list {
        match p.name.split_once('/').filter(|_| nested) {
            Some((folder, stem)) => match folders.iter_mut().find(|(f, _)| f == folder) {
                Some((_, items)) => items.push(row(p, stem.to_string())),
                None => folders.push((folder.to_string(), vec![row(p, stem.to_string())])),
            },
            None => v.push(row(p, p.name.clone())),
        }
    }
    for (folder, children) in folders {
        v.push(PItem { children, ..PItem::new(folder, Cmd::Nothing) });
    }
    v
}

/// The preset list rows for the instance under `key`: the user's presets, then the factory ones (under "User" and
/// "Factory" heads while there are both). `nested`: presets in sub-folders go into one submenu per folder (the
/// title-bar box); otherwise every preset is a row, sub-folder ones as "Folder/name" (the window menu's Presets
/// submenu, which cannot open a further submenu). The plugin window's popups are Windows-only so far.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn items(app: &App, key: usize, nested: bool) -> Vec<PItem> {
    let Some(h) = header(app, key) else { return Vec::new() };
    let (mine, fac) = (user(&h.plugin), factory(&h.plugin));
    let both = !mine.is_empty() && !fac.is_empty();
    let mut v: Vec<PItem> = Vec::new();
    if both {
        v.push(PItem::head("User"));
    }
    v.extend(rows(app, key, &mine, nested));
    if both {
        v.push(PItem::head("Factory"));
    }
    v.extend(rows(app, key, &fac, nested));
    if v.is_empty() {
        v.push(PItem::new("(no presets)", Cmd::Nothing).off());
    }
    // Long lists continue in further columns (the popup wraps at the screen's height).
    v
}

/// Fills the window menu's Presets section (spec: ui/plugin-window.md "Presets"): "Presets" becomes the list
/// (a submenu), "Browse presets" and "Save preset as..." work.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn fill_window_menu(app: &App, key: usize, menu: &mut Popup) {
    for it in menu.items.iter_mut() {
        match &*it.label {
            "Presets" if !it.head => *it = PItem { children: items(app, key, false), ..PItem::new("Presets", Cmd::Nothing) },
            "Browse presets" => *it = PItem::new("Browse presets", Cmd::Preset(PCmd::Browse(key))),
            "Save preset as..." => *it = PItem::new("Save preset as...", Cmd::Preset(PCmd::SaveAs(key))),
            _ => {}
        }
    }
}

/// The title-bar Presets box popup: the presets, then the commands on them.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn popup(app: &App, key: usize, x: f32, y: f32) -> Popup {
    let mut items = items(app, key, true);
    let own = own_current(app, key).is_some();
    let on = |it: PItem, on: bool| if on { it } else { it.off() };
    items.push(PItem::sep());
    items.push(PItem::new("Save preset as...", Cmd::Preset(PCmd::SaveAs(key))));
    items.push(PItem::new("Import preset...", Cmd::Preset(PCmd::Import(key))));
    items.push(on(PItem::new("Rename preset...", Cmd::Preset(PCmd::Rename(key))), own));
    items.push(on(PItem::new("Delete preset...", Cmd::Preset(PCmd::Delete(key))), own));
    items.push(PItem::sep());
    items.push(PItem::new("Browse presets", Cmd::Preset(PCmd::Browse(key))));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
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
        PCmd::Import(key) => {
            header(app, key)?;
            let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Open, "Import preset", "preset import", crate::dialog::Then::ImportPreset(key))
                .filter("Presets", IMPORT_EXTS)
                .filter("All files", &[]);
            app.file_dialog(dlg);
            None
        }
        PCmd::Rename(key) => {
            let cur = own_current(app, key)?;
            let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Save, "Rename preset", "preset", crate::dialog::Then::RenamePreset(key, cur.clone()))
                .filter("Lemon Studio preset", &[EXT])
                .start(cur.parent())
                .name(&format!("{}.{EXT}", shown_name(&cur)));
            app.file_dialog(dlg);
            None
        }
        PCmd::Delete(key) => {
            let cur = own_current(app, key)?;
            use crate::dialog::{Answer, Kind, Msg, Then};
            let text = vec![("Delete the preset ".to_string(), false), (shown_name(&cur), true), ("?".to_string(), false)];
            app.show_msg(Msg::new(Kind::Question, "Delete preset", text, &[Answer::Yes, Answer::No], Then::DeletePreset(key, cur)));
            None
        }
        PCmd::Browse(key) => browse(app, key),
    };
    app.invalidate();
    msg
}

/// Loads preset `path` (any [`IMPORT_EXTS`] type, or a factory preset) into the instance under `key`; returns the
/// status message.
pub fn load_into(app: &mut App, key: usize, path: &Path) -> Option<String> {
    let d = app.plugins.descriptor(key)?;
    let first = match crate::citrus::fst_state(path) {
        // An FL preset file (Citrus's list of an FL install's presets) holds the original's state.
        Some(state) => state.map(|s| (shown_name(path), Body::State(s))),
        None => read_any(path, &d).and_then(|v| v.into_iter().next().ok_or_else(|| format!("{} holds no preset", shown_name(path)))),
    };
    Some(match first {
        Ok((_, body)) => match apply(app, key, &body) {
            Some(missing) => {
                app.plugins.mark_edited(key);
                app.plugins.set_preset(key, Some(path.to_path_buf()));
                auto_name(app, key, path);
                match missing.is_empty() {
                    true => format!("Preset: {}", shown_name(path)),
                    false => format!("Preset: {} ({} has no {})", shown_name(path), d.name, missing.join(", ")),
                }
            }
            None => format!("{} did not take the preset {}", d.name, shown_name(path)),
        },
        Err(e) => e,
    })
}

/// Puts `body` into the instance under `key`: the names of listed parameters it does not have, `None` when it did
/// not take it.
fn apply(app: &mut App, key: usize, body: &Body) -> Option<Vec<String>> {
    match body {
        Body::State(s) => app.plugins.set_state(key, s).then(Vec::new),
        Body::Params(p) => app.plugins.apply_params(key, p),
    }
}

/// A preset at `path` went into the instance under `key` (the ◀ ▶ arrows or the Presets lists): while "Auto name
/// effect slots" is on, a mixer slot takes the preset's name, kept until the next preset or a rename by hand (the
/// slot's window title shows it too); a channel goes by "Auto name channels" (`rack::auto_name`). Spec:
/// ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup".
fn auto_name(app: &mut App, key: usize, path: &Path) {
    let s = &mut app.state;
    match crate::plugins::fx_of(key) {
        Some(slot) if crate::prefs::check(&crate::prefs::AUTO_NAME_EFFECT_SLOTS) => {
            s.mixer.slot_names.insert(slot, shown_name(path));
        }
        Some(_) => {}
        None => {
            if let Some(ch) = s.project.channels.get_mut(key) {
                crate::rack::auto_name(ch, path);
            }
        }
    }
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

/// `target` with the preset extension.
fn with_ext(target: PathBuf) -> PathBuf {
    if is_own(&target) { target } else { target.with_extension(EXT) }
}

/// The preset file chosen in Save preset as...: the plugin's state is written there. Returns the status message.
pub fn save_chosen(app: &mut App, key: usize, target: PathBuf) -> Option<String> {
    let h = header(app, key)?;
    let Some(state) = app.plugins.state(key) else { return Some(format!("{} has no state to save", h.plugin)) };
    let target = with_ext(target);
    Some(match save(&target, &h, &state) {
        Ok(()) => {
            app.plugins.set_preset(key, Some(target.clone()));
            format!("Saved preset {}", shown_name(&target))
        }
        Err(e) => format!("Preset not saved: {e}"),
    })
}

/// The file chosen in Import preset...: its presets are copied into the plugin's folder (a bank's into a sub-folder
/// named after it), as Lemon Studio presets, and the first one loads. Returns the status message.
pub fn import_chosen(app: &mut App, key: usize, file: PathBuf) -> Option<String> {
    let h = header(app, key)?;
    let d = app.plugins.descriptor(key)?;
    let found = match read_any(&file, &d) {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => return Some(format!("{} holds no preset", shown_name(&file))),
        Err(e) => return Some(e),
    };
    let mut dir = dir_for(&h.plugin)?;
    if found.len() > 1 {
        dir = dir.join(folder_name(&shown_name(&file)));
    }
    let mut first = None;
    for (name, body) in &found {
        let target = free_file(&dir, name);
        if let Err(e) = save_body(&target, &h, body) {
            return Some(format!("Preset not imported: {e}"));
        }
        first.get_or_insert(target);
    }
    let loaded = load_into(app, key, first.as_deref()?);
    Some(match found.len() {
        1 => loaded.unwrap_or_default().replacen("Preset:", "Imported preset", 1),
        n => format!("Imported {n} presets from {}", shown_name(&file)),
    })
}

/// The new name chosen in Rename preset... for preset `old`. Returns the status message.
pub fn rename_chosen(app: &mut App, key: usize, old: PathBuf, target: PathBuf) -> Option<String> {
    let target = with_ext(target);
    if target == old {
        return None;
    }
    Some(match std::fs::rename(&old, &target) {
        Ok(()) => {
            if app.plugins.presets.get(&key) == Some(&old) {
                app.plugins.set_preset(key, Some(target.clone()));
            }
            format!("Renamed preset {} to {}", shown_name(&old), shown_name(&target))
        }
        Err(e) => format!("Preset not renamed: {e}"),
    })
}

/// Delete preset... answered Yes for preset `path`. Returns the status message.
pub fn delete_chosen(app: &mut App, key: usize, path: PathBuf) -> Option<String> {
    Some(match std::fs::remove_file(&path) {
        Ok(()) => {
            if app.plugins.presets.get(&key) == Some(&path) {
                app.plugins.set_preset(key, None);
            }
            format!("Deleted preset {}", shown_name(&path))
        }
        Err(e) => format!("Preset not deleted: {e}"),
    })
}

/// Browse presets: the browser shows, on its All tab, Plugin presets opened at the plugin's folder.
fn browse(app: &mut App, key: usize) -> Option<String> {
    let h = header(app, key)?;
    let s = &mut app.state;
    s.show_browser = true;
    s.browser_tab = 0;
    s.browser_search.clear();
    let root = crate::views::browser::presets_key();
    let k = format!("{root}/{}", folder_name(&h.plugin));
    s.browser_open.insert(root);
    s.browser_open.insert(k.clone());
    crate::browser::select(s, &k);
    crate::browser::reveal(s);
    None
}

// ---------------------------------------------------------------------------------------------------------------
// Drops.

/// How long a preset dropped where a new instance is made waits for that instance (`pending_tick`).
const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// A preset file dropped at the mouse (from the desktop or the browser): into the plugin window under it, the plugin of
/// the rack row or mixer slot under it, else into a new channel (a generator) or effect (in the slot under the mouse,
/// else the current track's first free one) of the plugin the preset is for. Returns the status message.
pub fn dropped(app: &mut App, path: &Path) -> Option<String> {
    use crate::app::{Action, Win};
    #[cfg(any(windows, target_os = "macos"))]
    if let Some(row) = app.plugin_window_under_cursor() {
        return load_into(app, row, path);
    }
    let (x, y) = app.mouse;
    let (win, mix) = match app.top_hit(x, y) {
        Some(h) => (h.win, match h.action {
            Action::Mix(crate::mixer::MHit::SlotName(k) | crate::mixer::MHit::SlotArrow(k)) => Some(k),
            _ => None,
        }),
        None => (None, None),
    };
    let mut slot = None;
    match win {
        Some(Win::ChannelRack) => {
            if let Some(row) = crate::views::rack::row_at(&app.state, y).filter(|&r| app.plugins.descriptor(r).is_some()) {
                return load_into(app, row, path);
            }
        }
        Some(Win::Mixer) => {
            if let Some(k) = mix {
                let key = crate::plugins::fx_key(app.state.selected_mixer, k);
                if app.plugins.descriptor(key).is_some() {
                    return load_into(app, key, path);
                }
                slot = Some(k);
            }
        }
        _ => {}
    }
    let name = shown_name(path);
    let Some(uid) = uid_of(path) else { return Some(format!("{name}: drop it on a plugin window, a channel or a mixer slot")) };
    let same = |d: &os_host::Descriptor| d.uid().eq_ignore_ascii_case(&uid);
    if let Some(i) = app.plugins.available.iter().position(|a| same(&a.desc)) {
        crate::plugin_menu::add_generator(app, i, None);
        let row = app.state.project.channels.len() - 1;
        app.plugins.preset_waits.push((row, path.to_path_buf(), std::time::Instant::now()));
        return Some(format!("Loading {name} in a new channel"));
    }
    if let Some(i) = app.plugins.effects.iter().position(|a| same(&a.desc)) {
        let t = app.state.selected_mixer;
        let slot = slot.or_else(|| app.state.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots.iter().position(Option::is_none)).map(|k| k as u8));
        let Some(k) = slot else { return Some("Every slot of this track is in use".into()) };
        crate::plugin_menu::add_effect(app, i, Some(k));
        app.plugins.preset_waits.push((crate::plugins::fx_key(t, k), path.to_path_buf(), std::time::Instant::now()));
        return Some(format!("Loading {name} in a new effect"));
    }
    let plugin = std::fs::read(path).ok().and_then(|b| decode(&b).map(|(h, _)| h.plugin)).filter(|p| !p.is_empty());
    Some(format!("{name} is for {}, which is not installed", plugin.unwrap_or_else(|| "a plugin".into())))
}

/// Presets dropped where a new instance was made load once it is hosted (or are given up after [`WAIT`]).
pub fn pending_tick(app: &mut App) {
    if app.plugins.preset_waits.is_empty() {
        return;
    }
    for (key, path, at) in std::mem::take(&mut app.plugins.preset_waits) {
        if app.plugins.descriptor(key).is_some() {
            app.state.message = load_into(app, key, &path);
            app.invalidate();
        } else if at.elapsed() < WAIT {
            app.plugins.preset_waits.push((key, path, at));
        }
    }
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

    fn state_of(r: Result<(Header, Body), String>) -> Vec<u8> {
        match r.unwrap().1 {
            Body::State(s) => s,
            b => panic!("not a state: {b:?}"),
        }
    }

    #[test]
    fn preset_files_round_trip() {
        let h = Header { plugin: "Lemon Sampler".into(), id: "org.openstudio.open-sampler".into(), format: "clap".into(), body: String::new() };
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
        assert_eq!(state_of(load(&p, "org.openstudio.open-sampler")), state);
        assert!(load(&p, "some.other.plugin").is_err(), "presets of another plugin are refused");
        assert!(load(&p, "").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: plugins/presets.md "File format": a parameter list instead of the state.
    #[test]
    fn parameter_list_presets() {
        let h = Header { plugin: "Lemon Reverb".into(), id: "clap:x".into(), format: "clap".into(), body: String::new() };
        let params = vec![("Decay time".to_string(), 25.0), ("Wet = level".to_string(), -0.5)];
        let b = encode_params(&h, &params);
        assert!(std::str::from_utf8(&b).unwrap().ends_with("body=params\nDecay time = 25\nWet = level = -0.5\n"));
        let dir = tmp("params");
        let p = dir.join("Hall.ospreset");
        std::fs::write(&p, &b).unwrap();
        assert_eq!(load(&p, "clap:x").unwrap().1, Body::Params(params.clone()));
        assert_eq!(parse_params("# a comment\n\n  Mix = 1e2 \r\n").unwrap(), [("Mix".to_string(), 100.0)]);
        // Windows line ends, as a text editor (or a Git checkout) may leave them.
        let crlf = String::from_utf8(b.clone()).unwrap().replace('\n', "\r\n");
        let (h2, rest) = decode(crlf.as_bytes()).unwrap();
        assert_eq!((h2.plugin.as_str(), h2.body.as_str()), ("Lemon Reverb", PARAMS_BODY));
        assert_eq!(parse_params(std::str::from_utf8(rest).unwrap()).unwrap(), params);
        assert!(parse_params("Mix: 1").is_err());
        assert!(parse_params("= 1").is_err());
        assert!(parse_params("Mix = loud").is_err());
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
        assert_eq!(free_file(&dir, "b"), dir.join("b 2.ospreset"));
        assert_eq!(free_file(&dir, "d"), dir.join("d.ospreset"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // spec: plugins/presets.md "Factory presets"
    #[test]
    fn factory_presets_are_listed_and_read_by_path() {
        static FILES: &[(&str, &[u8])] = &[("Lemon Reverb/Room.ospreset", b"r"), ("Lemon Reverb/Halls/Big.ospreset", b"b"), ("Lemon Reverb/Halls/Air.ospreset", b"a"), ("Lemon Trio/Bass.ospreset", b"t"), ("Lemon Reverb/a/b/c.ospreset", b"deep")];
        let l = factory_in(FILES, "Lemon Reverb");
        let names: Vec<&str> = l.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Room", "Halls/Air", "Halls/Big"], "deeper folders are left out");
        assert!(l.iter().all(|p| p.factory && is_factory(&p.path)));
        assert_eq!(factory_bytes_in(FILES, &l[2].path), Some(&b"b"[..]));
        assert_eq!(factory_bytes_in(FILES, Path::new("<factory>/Lemon Trio/Lead.ospreset")), None);
        assert!(!is_factory(Path::new("C:/Presets/Lemon Trio/Bass.ospreset")));
        // The compiled-in presets all read as presets of the plugin they are filed under.
        for (rel, bytes) in FACTORY {
            let (h, rest) = decode(bytes).unwrap_or_else(|| panic!("{rel} is not a preset"));
            assert_eq!(folder_name(&h.plugin), rel.split('/').next().unwrap(), "{rel}");
            if h.body == PARAMS_BODY {
                parse_params(std::str::from_utf8(rest).unwrap()).unwrap_or_else(|e| panic!("{rel}: {e}"));
            }
        }
    }

    /// Every parameter a factory preset of a built-in plugin lists is one that plugin has, and it is in range.
    #[test]
    fn factory_presets_of_built_in_plugins_name_their_parameters() {
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        pl.effects = crate::plugins::builtin_effects();
        let mut keys = Vec::new();
        for i in 0..pl.available.len() {
            pl.create(i, i).unwrap();
            keys.push(i);
        }
        for i in 0..pl.effects.len() {
            pl.create_effect(1, i as u8, i, None).unwrap();
            keys.push(crate::plugins::fx_key(1, i as u8));
        }
        let mut checked = 0;
        for key in keys {
            let d = pl.descriptor(key).unwrap();
            let infos = pl.param_infos(key);
            for p in factory_in(FACTORY, &d.name) {
                let Ok((h, Body::Params(list))) = load(&p.path, &d.uid()) else { panic!("{} does not load for {}", p.name, d.name) };
                assert_eq!(h.plugin, d.name);
                for (name, v) in list {
                    let i = infos.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("{}/{}: no parameter {name}", d.name, p.name));
                    let (lo, hi) = (i.min.min(i.max), i.max.max(i.min));
                    assert!(v >= lo - 1e-9 && v <= hi + 1e-9, "{}/{}: {name} = {v} outside {lo}..{hi}", d.name, p.name);
                }
                checked += 1;
            }
        }
        assert!(checked > 0 || FACTORY.is_empty());
    }

    // spec: plugins/presets.md "Import": VST 3 and VST 2 files are taken by a plugin of their format and id only.
    #[test]
    fn foreign_preset_files() {
        let dir = tmp("foreign");
        let desc = |name: &str, id: &str, format| os_host::Descriptor {
            format,
            id: id.into(),
            name: name.into(),
            vendor: String::new(),
            version: String::new(),
            features: Vec::new(),
            kind: os_host::Kind::Instrument,
            path: PathBuf::new(),
        };
        let vst3 = desc("Vee", "0123456789ABCDEF0123456789ABCDEF", os_host::Format::Vst3);
        let bytes = os_vst3::preset::Preset { class_id: vst3.id.clone(), component: b"comp".to_vec(), controller: vec![] }.to_bytes();
        let p = dir.join("Shiny.vstpreset");
        std::fs::write(&p, &bytes).unwrap();
        assert_eq!(read_any(&p, &vst3).unwrap(), [("Shiny".to_string(), Body::State(bytes.clone()))]);
        assert_eq!(uid_of(&p).unwrap(), vst3.uid());
        let other = os_host::Descriptor { id: "F".repeat(32), ..vst3.clone() };
        assert!(read_any(&p, &other).unwrap_err().contains("another plugin"));
        let vst2 = desc("Old", "41424344", os_host::Format::Vst2);
        assert!(read_any(&p, &vst2).unwrap_err().contains("not a VST 3 plugin"));
        // A bank of two programs, the second without a name.
        let program = |name: &str, v: f32| {
            let mut b = b"CcnK\0\0\0\0FxCk".to_vec();
            for x in [1, 0x41424344, 1, 1] {
                b.extend(i32::to_be_bytes(x));
            }
            let mut n = [0u8; 28];
            n[..name.len()].copy_from_slice(name.as_bytes());
            b.extend(n);
            b.extend(v.to_be_bytes());
            b
        };
        let mut bank = b"CcnK\0\0\0\0FxBk".to_vec();
        for x in [1, 0x41424344, 1, 2] {
            bank.extend(i32::to_be_bytes(x));
        }
        bank.extend([0u8; 128]);
        bank.extend(program("Pluck", 0.25));
        bank.extend(program("", 0.5));
        let f = dir.join("Set.fxb");
        std::fs::write(&f, &bank).unwrap();
        let got = read_any(&f, &vst2).unwrap();
        assert_eq!(got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Pluck", "Set 2"]);
        assert_eq!(got[0].1, Body::State(os_vst2::state::Saved::Params { program: None, values: vec![0.25] }.to_bytes()));
        assert_eq!(uid_of(&f).as_deref(), Some("vst2:41424344"));
        assert!(read_any(&f, &vst3).is_err());
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

    /// Held by the tests that point `OPENSTUDIO_PRESETS` (process-wide) at their own folder.
    static PRESETS_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A hosted Lemon Sampler on rack row 0, presets in a temp folder (`OPENSTUDIO_PRESETS`, kept while the guard is).
    fn hosted_sampler(name: &str) -> (App, PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = PRESETS_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp(name);
        // SAFETY: tests in this process that read OPENSTUDIO_PRESETS all point it at a temp folder.
        unsafe { std::env::set_var("OPENSTUDIO_PRESETS", &dir) };
        let mut app = crate::app::App::new(None);
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).expect("built in");
        pl.create(0, index).unwrap();
        app.plugins = pl;
        (app, dir, guard)
    }

    /// Save, list, step and load through a hosted Lemon Sampler: the state goes through the hosting layer and back.
    #[test]
    fn presets_of_a_hosted_plugin() {
        let (mut app, dir, _env) = hosted_sampler("hosted");
        let h = header(&app, 0).unwrap();
        assert_eq!(h.plugin, "Lemon Sampler");
        assert!(h.id.starts_with("clap:"));
        let state = app.plugins.state(0).unwrap();
        // Two presets: the default state, and one with a sample path.
        save(&file_for(&dir_for(&h.plugin).unwrap(), "Default"), &h, &state).unwrap();
        let other = crate::plugins::with_sample_path(&state, "C:/x/rim.wav").unwrap();
        save(&file_for(&dir_for(&h.plugin).unwrap(), "Rim"), &h, &other).unwrap();
        let names: Vec<String> = user(&h.plugin).iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["Default", "Rim"]);
        assert!(run(&mut app, PCmd::Step(0, -1)).unwrap().contains("Rim") || !factory(&h.plugin).is_empty(), "back from no preset = last");
        run(&mut app, PCmd::Load(0, file_for(&dir_for(&h.plugin).unwrap(), "Rim")));
        assert_eq!(app.plugins.sampler_path(0).as_deref(), Some("C:/x/rim.wav"));
        assert_eq!(app.plugins.preset_name(0).as_deref(), Some("Rim"));
        // spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup": "Auto name channels" is
        // on by default.
        assert_eq!(app.state.project.channels[0].name.as_deref(), Some("Rim"), "the ◀ ▶ arrows rename the channel");
        run(&mut app, PCmd::Step(0, -1));
        assert_eq!(app.plugins.sampler_path(0).as_deref(), Some(""), "back to Default");
        let it = items(&app, 0, true);
        assert!(it.iter().any(|i| i.label == "Default" && i.mark == Mark::Check(true)));
        // The window menu's Presets entry lists them; Browse presets works.
        let mut m = crate::playlist::plugin_window_menu(0, false, 0.0, 0.0);
        fill_window_menu(&app, 0, &mut m);
        let p = m.items.iter().find(|i| i.label == "Presets" && !i.head).unwrap();
        assert!(p.children.iter().filter(|i| !i.head).count() >= 2);
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Preset(PCmd::SaveAs(0))));
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Preset(PCmd::Browse(0))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Presets box popup's commands: Rename and Delete only for one of the user's presets; rename, delete, import
    /// and browse do what they say.
    #[test]
    fn presets_box_commands() {
        let (mut app, dir, _env) = hosted_sampler("box");
        let h = header(&app, 0).unwrap();
        let enabled = |app: &App, label: &str| popup(app, 0, 0.0, 0.0).items.iter().find(|i| i.label == label).unwrap().enabled;
        assert!(enabled(&app, "Save preset as...") && enabled(&app, "Import preset...") && enabled(&app, "Browse presets"));
        assert!(!enabled(&app, "Rename preset...") && !enabled(&app, "Delete preset..."), "no current preset");
        let state = app.plugins.state(0).unwrap();
        let mine = file_for(&dir_for(&h.plugin).unwrap(), "Mine");
        save(&mine, &h, &state).unwrap();
        run(&mut app, PCmd::Load(0, mine.clone()));
        assert!(enabled(&app, "Rename preset...") && enabled(&app, "Delete preset..."));
        let renamed = mine.with_file_name("Ours.ospreset");
        rename_chosen(&mut app, 0, mine.clone(), renamed.with_extension(""));
        assert!(!mine.exists() && renamed.exists());
        assert_eq!(app.plugins.preset_name(0).as_deref(), Some("Ours"));
        assert_eq!(delete_chosen(&mut app, 0, renamed.clone()).as_deref(), Some("Deleted preset Ours"));
        assert!(!renamed.exists());
        assert_eq!(app.plugins.preset_name(0), None);
        // Import: a preset from elsewhere is copied into the plugin's folder and loads.
        let elsewhere = tmp("box-elsewhere").join("Snare.ospreset");
        save(&elsewhere, &h, &crate::plugins::with_sample_path(&state, "C:/x/snare.wav").unwrap()).unwrap();
        assert_eq!(import_chosen(&mut app, 0, elsewhere.clone()).as_deref(), Some("Imported preset Snare"));
        let copy = file_for(&dir_for(&h.plugin).unwrap(), "Snare");
        assert!(copy.exists());
        assert_eq!(app.plugins.presets.get(&0), Some(&copy));
        assert_eq!(app.plugins.sampler_path(0).as_deref(), Some("C:/x/snare.wav"));
        import_chosen(&mut app, 0, elsewhere.clone());
        assert!(copy.with_file_name("Snare 2.ospreset").exists(), "a second import does not replace the first");
        // Browse presets: the browser shows its Plugin presets at the plugin.
        app.state.show_browser = false;
        run(&mut app, PCmd::Browse(0));
        assert!(app.state.show_browser);
        assert_eq!(app.state.browser_sel.as_deref(), Some(format!("{}/Lemon Sampler", crate::views::browser::presets_key()).as_str()));
        let rows = crate::views::browser::rows(&app.state);
        assert!(rows.iter().any(|r| r.label == "Snare" && matches!(&r.item, crate::views::browser::Item::Preset(p) if Path::new(p) == copy)));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(elsewhere.parent().unwrap());
    }

    // spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup"
    /// A preset loaded from the Presets lists names what holds it: a channel while "Auto name channels" is on (the
    /// default), a mixer slot while "Auto name effect slots" is on (off by default). A slot keeps a preset's name until
    /// the next preset or a rename by hand, whatever the check; the slot's window title shows the slot's name.
    #[test]
    fn presets_auto_name_channels_and_effect_slots() {
        use crate::app::App;
        use crate::playlist::EditTarget;
        use crate::prefs::{self, AUTO_NAME_CHANNELS, AUTO_NAME_EFFECT_SLOTS};
        let dir = tmp("auto-name");
        let mut app = App::new(None);
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).expect("built in");
        // Rack row 0's instance, and one in Insert 1's first slot, which holds Fruity Delay 3.
        let fx = crate::plugins::fx_key(1, 0);
        pl.create(0, index).unwrap();
        pl.create(fx, index).unwrap();
        app.plugins = pl;
        app.state.project.channels[0].name = Some("808 Snare".into());
        crate::mixer::track_mut(&mut app.state, 1).unwrap().slots[0] = Some(crate::plugins::new_slot("Fruity Delay 3"));
        let (h, state) = (header(&app, 0).unwrap(), app.plugins.state(0).unwrap());
        let preset = |name: &str| {
            let p = file_for(&dir, name);
            save(&p, &h, &state).unwrap();
            p
        };
        let (chorus, bandpass, bass) = (preset("Chorus"), preset("Bandpass"), preset("Deep Bass"));
        let channel = |app: &App| app.state.project.channels[0].name.clone().unwrap_or_default();
        let slot = |app: &App| crate::mixer::slot_name(&app.state, 1, 0).unwrap_or_default();
        // Channels: on by default.
        assert_eq!(run(&mut app, PCmd::Load(0, bass)).as_deref(), Some("Preset: Deep Bass"));
        assert_eq!(channel(&app), "Deep Bass");
        prefs::set_check(&AUTO_NAME_CHANNELS, false);
        app.state.project.channels[0].name = Some("808 HiHat".into());
        run(&mut app, PCmd::Load(0, chorus.clone()));
        assert_eq!(channel(&app), "808 HiHat", "off: the channel keeps its name");
        // Effect slots: off by default.
        run(&mut app, PCmd::Load(fx, bandpass.clone()));
        assert_eq!(slot(&app), "Fruity Delay 3", "off: the slot keeps its name");
        prefs::set_check(&AUTO_NAME_EFFECT_SLOTS, true);
        run(&mut app, PCmd::Load(fx, chorus));
        assert_eq!(slot(&app), "Chorus");
        #[cfg(plugin_windows)]
        assert_eq!(crate::mixer::fx_info(&app.state.project, &app.state.mixer, fx).unwrap().name, "Chorus", "window title");
        // The name stays with the check off, until a rename by hand or the next preset with it on.
        prefs::set_check(&AUTO_NAME_EFFECT_SLOTS, false);
        run(&mut app, PCmd::Load(fx, bandpass.clone()));
        assert_eq!(slot(&app), "Chorus", "kept");
        crate::playlist::start_edit(&mut app.state, EditTarget::SlotName(1, 0), (0.0, 0.0));
        app.state.playlist.edit.as_mut().unwrap().text = "My echo".into();
        crate::playlist::commit_edit(&mut app.state);
        assert_eq!(slot(&app), "My echo", "renamed by hand");
        prefs::set_check(&AUTO_NAME_EFFECT_SLOTS, true);
        run(&mut app, PCmd::Load(fx, bandpass));
        assert_eq!(slot(&app), "Bandpass", "the next preset");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A parameter-list preset sets the listed parameters and puts the others back to their defaults; names the
    /// plugin does not have are reported.
    #[test]
    fn parameter_presets_load_through_the_parameters() {
        let (mut app, dir, _env) = hosted_sampler("param-load");
        let h = header(&app, 0).unwrap();
        let infos = app.plugins.param_infos(0);
        let knob = infos.iter().find(|i| !i.read_only && !i.hidden && !i.bypass && i.steps == 0 && i.max > i.min).expect("a knob").clone();
        let mid = knob.min + (knob.max - knob.min) * 0.25;
        let p = dir.join("Quarter.ospreset");
        std::fs::write(&p, encode_params(&h, &[(knob.name.clone(), mid), ("No such knob".into(), 1.0)])).unwrap();
        let msg = run(&mut app, PCmd::Load(0, p)).unwrap();
        assert!(msg.starts_with("Preset: Quarter") && msg.contains("No such knob"), "{msg}");
        assert!((app.plugins.param_value(0, knob.id).unwrap() - 0.25).abs() < 1e-3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
