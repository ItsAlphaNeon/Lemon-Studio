// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Plugin presets: Lemon Studio's own preset store, used by every hosted plugin through its state save / load. The plugin
//! window's title-bar Presets box lists them (an app popup), its ◀ ▶ arrows step through them, "Save preset as..."
//! stores the current state. Factory presets come compiled into the program; VST 3 and VST 2 preset files import; a
//! preset file dropped on a plugin window, a rack row or a mixer slot loads there.
//!
// spec: plugins/presets.md, ui/plugin-window.md (Presets box), ui/browser.md (Plugin presets)

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
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

/// The factory presets of `plugin`: Lemon Studio's, then (for Citrus and Smith) the `.fst` files of an installed copy.
pub fn factory(st: &crate::settings_file::SettingsStore, plugin: &str) -> Vec<Preset> {
    let mut v = factory_in(FACTORY, plugin);
    v.extend(crate::citrus::more_presets(st, plugin));
    v.extend(crate::smith::more_presets(st, plugin));
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
pub fn read(path: &Path) -> Result<Vec<u8>, String> {
    if is_factory(path) {
        return factory_bytes_in(FACTORY, path).map(<[u8]>::to_vec).ok_or_else(|| format!("{} is not a factory preset", shown_name(path)));
    }
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The presets of `plugin`: the user's, then the factory ones.
pub fn list(st: &crate::settings_file::SettingsStore, plugin: &str) -> Vec<Preset> {
    let mut v = user(plugin);
    v.extend(factory(st, plugin));
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
    #[cfg_attr(not(any(feature = "vst3", feature = "vst2")), allow(unused_variables))]
    let other = || format!("{stem} belongs to another plugin");
    match ext.as_str() {
        #[cfg(feature = "vst3")]
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
        #[cfg(feature = "vst2")]
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
        #[cfg(feature = "vst3")]
        "vstpreset" => Some(format!("vst3:{}", os_vst3::preset::Preset::parse(&bytes)?.class_id)),
        #[cfg(feature = "vst2")]
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
pub fn free_file(dir: &Path, name: &str) -> PathBuf {
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
    /// Copy to B: a copy of the instance's state becomes its spare state (`Hosted::spare`).
    StoreSpare(usize),
    /// Swap A / B: the state in use and the spare state change places; one undo step.
    FlipSpare(usize),
    /// Display preset name in preset selector: tick (true) or untick it.
    ShowName(usize, bool),
}

/// The undo step of Swap A / B.
pub const FLIP_SPARE: &str = "Swap A / B";

/// The state the instance under `key` would go back to on undo (`Taken::Plugin`); `None` when it has none to save.
fn side(ses: &crate::session::Session, key: usize) -> Option<crate::control::command::PluginSide> {
    let name = match crate::plugins::fx_of(key) {
        // A container's effect: no name of its own to put back.
        None if key >= crate::plugins::MEMBER_KEY => None,
        None => ses.project.channels.get(key)?.name.clone(),
        Some((t, k)) => crate::edit::mixer::slot_label(ses, t, k),
    };
    Some(crate::control::command::PluginSide { state: ses.plugins.state(key)?, preset: ses.plugins.presets.get(&key).cloned(), name, fill: 0 })
}

/// `load_into`; `step`: an undo step for it (none for a preset that goes into an instance just made for it, part of
/// the step that made it).
pub fn load_with(ses: &mut crate::session::Session, key: usize, path: &Path, step: bool) -> Option<String> {
    let d = ses.plugins.descriptor(key)?;
    let first = match crate::smith::fst_state(path, &d.name).or_else(|| crate::citrus::fst_state(path)) {
        // An FL preset file (Citrus's and Smith's lists of an FL install's presets) holds the original's state.
        Some(state) => state.map(|s| (shown_name(path), Body::State(s))),
        None => read_any(path, &d).and_then(|v| v.into_iter().next().ok_or_else(|| format!("{} holds no preset", shown_name(path)))),
    };
    let before = if step { side(ses, key) } else { None };
    Some(match first {
        Ok((_, body)) => match apply(ses, key, &body) {
            Some(missing) => {
                ses.plugins.mark_edited(key);
                ses.plugins.set_preset(key, Some(path.to_path_buf()));
                auto_name(ses, key, path);
                if let Some(before) = before.filter(|b| side(ses, key).as_ref() != Some(b)) {
                    crate::control::state_edit(ses, key, crate::control::command::PLUGIN_PRESET, before);
                }
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

/// Swap A / B: the instance under `key` takes its spare state, and the state it had becomes the spare one
/// (an A/B compare). One undo step that puts the state in use back. Lemon Studio's choices: the spare state is not part
/// of undo (undoing a flip leaves the spare state as the flip left it), and the preset shown and the channel's or slot's
/// name stay as they are.
pub fn flip_spare(ses: &mut crate::session::Session, key: usize) -> Option<String> {
    let spare = ses.plugins.hosted.get(&key)?.spare.clone()?;
    let now = ses.plugins.state(key)?;
    let before = side(ses, key);
    if !ses.plugins.set_state(key, &spare) {
        return Some(format!("{} did not take its spare state", ses.plugins.hosted.get(&key)?.name));
    }
    ses.plugins.hosted.get_mut(&key)?.spare = Some(now);
    ses.plugins.mark_edited(key);
    ses.params.reread(key);
    if let Some(before) = before.filter(|b| side(ses, key).as_ref() != Some(b)) {
        crate::control::state_edit(ses, key, FLIP_SPARE, before);
    }
    Some("Swapped A / B".into())
}

/// Puts `body` into the instance under `key`: the names of listed parameters it does not have, `None` when it did
/// not take it.
fn apply(ses: &mut crate::session::Session, key: usize, body: &Body) -> Option<Vec<String>> {
    match body {
        Body::State(s) => ses.plugins.set_state(key, s).then(Vec::new),
        Body::Params(p) => ses.plugins.apply_params(key, p),
    }
}

/// A preset at `path` went into the instance under `key` (the ◀ ▶ arrows or the Presets lists): while "Auto name
/// effect slots" is on, a mixer slot takes the preset's name, kept until the next preset or a rename by hand (the
/// slot's window title shows it too); a channel goes by "Name channels after their sample or plugin" (`rack::auto_name`). Spec:
/// ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup".
fn auto_name(ses: &mut crate::session::Session, key: usize, path: &Path) {
    match crate::plugins::fx_of(key) {
        Some(slot) if ses.settings.check(&crate::prefs::AUTO_NAME_EFFECT_SLOTS) => {
            crate::edit::mixer::set_slot_label(ses, slot.0, slot.1, Some(shown_name(path)));
        }
        Some(_) => {}
        None => {
            if let Some(ch) = ses.project.get_mut().channels.get_mut(key) {
                crate::edit::rack::auto_name(&ses.settings, ch, path);
            }
        }
    }
}

/// `target` with the preset extension.
pub fn with_ext(target: PathBuf) -> PathBuf {
    if is_own(&target) { target } else { target.with_extension(EXT) }
}

// ---------------------------------------------------------------------------------------------------------------
// Drops.

/// How long a preset dropped where a new instance is made waits for that instance (`pending_tick`).
const WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// `pending_tick` on the session: what the loads say goes to the interface (`View::Message`).
pub fn pending_in(ses: &mut crate::session::Session) {
    for (key, path, at) in std::mem::take(&mut ses.plugins.preset_waits) {
        if ses.plugins.descriptor(key).is_some() {
            if let Some(m) = load_with(ses, key, &path, false) {
                ses.touched.views.push(crate::session::View::Message(m));
            }
            ses.touched.repaint = true;
        } else if at.elapsed() < WAIT {
            ses.plugins.preset_waits.push((key, path, at));
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
                let Ok((h, body)) = load(&p.path, &d.uid()) else { panic!("{} does not load for {}", p.name, d.name) };
                assert_eq!(h.plugin, d.name);
                // A state preset (Press, Pulp Filter): the plugin's own tests read its state.
                let Body::Params(list) = body else {
                    checked += 1;
                    continue;
                };
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
    #[cfg(all(feature = "vst3", feature = "vst2"))]
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

}
