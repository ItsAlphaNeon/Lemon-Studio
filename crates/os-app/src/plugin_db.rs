// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The plugin database: the user's plugin choices (search folders, favourites, hidden plugins, combining formats), the
//! scan cache, finding plugin files, the sweep that scans the files that changed, and the lists the plugin menus and
//! the plugin manager show.
// spec: ui/plugin-manager.md, ui/plugin-menu.md

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use os_host::{Descriptor, Format, Kind};
use serde::{Deserialize, Serialize};

use crate::plugins::Available;

/// The format listed when a plugin is found in several (spec: ui/plugin-manager.md §3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Prefer {
    #[default]
    Vst3,
    Clap,
    /// Not hosted yet: offered greyed out.
    Vst2,
}

impl Prefer {
    pub fn label(self) -> &'static str {
        match self {
            Prefer::Vst3 => "VST3",
            Prefer::Clap => "CLAP",
            Prefer::Vst2 => "VST2",
        }
    }
}

/// How the plugin menus arrange their plugins (spec: ui/plugin-menu.md §3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MenuView {
    #[default]
    Categories,
    Simple,
    Vendor,
}

/// A search folder the user added (§2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub path: String,
    #[serde(default = "yes")]
    pub on: bool,
}

fn yes() -> bool {
    true
}

/// The user's choices: `plugins.json` (§5). Missing keys take their defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub version: u32,
    pub startup_scan: bool,
    pub rescan_errors: bool,
    pub combine: bool,
    pub prefer: Prefer,
    pub menu_view: MenuView,
    pub folders: Vec<Folder>,
    /// Standard folders switched off (paths as listed).
    pub standard_off: Vec<String>,
    /// Plugin uids (`format:id`).
    pub favorites: Vec<String>,
    pub hidden: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs {
            version: 1,
            startup_scan: true,
            rescan_errors: false,
            combine: true,
            prefer: Prefer::Vst3,
            menu_view: MenuView::Categories,
            folders: Vec::new(),
            standard_off: Vec::new(),
            favorites: Vec::new(),
            hidden: Vec::new(),
        }
    }
}

const PREFS_FILE: &str = "plugins.json";
const CACHE_FILE: &str = "plugin_cache.json";

/// Reads a JSON file from the user folder (nothing under test: tests never see the user's files).
fn read_json<T: for<'de> Deserialize<'de>>(name: &str) -> Option<T> {
    if cfg!(test) {
        return None;
    }
    let bytes = std::fs::read(os_paths::user_file(name)?).ok()?;
    serde_json::from_slice(&bytes).map_err(|e| eprintln!("lemonstudio: {name}: {e}")).ok()
}

/// Writes a JSON file into the user folder through a temporary file, so a failed write keeps the old one.
fn write_json<T: Serialize>(name: &str, v: &T) {
    if cfg!(test) {
        return;
    }
    let Some(p) = os_paths::user_file(name) else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("json.tmp");
    match serde_json::to_vec_pretty(v) {
        Ok(text) if std::fs::write(&tmp, &text).is_ok() => {
            if let Err(e) = std::fs::rename(&tmp, &p) {
                eprintln!("lemonstudio: {name}: {e}");
            }
        }
        _ => eprintln!("lemonstudio: could not write {name}"),
    }
}

impl Prefs {
    pub fn load() -> Prefs {
        read_json(PREFS_FILE).unwrap_or_default()
    }

    pub fn save(&self) {
        write_json(PREFS_FILE, self);
    }

    pub fn is_favorite(&self, uid: &str) -> bool {
        self.favorites.iter().any(|u| u == uid)
    }

    pub fn is_hidden(&self, uid: &str) -> bool {
        self.hidden.iter().any(|u| u == uid)
    }

    /// Stars or unstars plugin `uid`.
    pub fn toggle_favorite(&mut self, uid: &str) {
        toggle(&mut self.favorites, uid);
    }

    /// Shows or hides plugin `uid` in the plugin menus.
    pub fn toggle_hidden(&mut self, uid: &str) {
        toggle(&mut self.hidden, uid);
    }

    /// Adds search folder `path` (on); false when it is listed already (added or standard).
    pub fn add_folder(&mut self, path: &Path) -> bool {
        if search_dirs(self).iter().any(|d| same_path(&d.path, path)) {
            return false;
        }
        self.folders.push(Folder { path: path.to_string_lossy().into_owned(), on: true });
        true
    }

    /// Switches search folder `path` on or off (added or standard).
    pub fn toggle_folder(&mut self, path: &Path) {
        if let Some(f) = self.folders.iter_mut().find(|f| same_path(Path::new(&f.path), path)) {
            f.on = !f.on;
        } else if let Some(i) = self.standard_off.iter().position(|o| same_path(Path::new(o), path)) {
            self.standard_off.remove(i);
        } else {
            self.standard_off.push(path.to_string_lossy().into_owned());
        }
    }

    /// Removes added folder `path` (standard folders stay).
    pub fn remove_folder(&mut self, path: &Path) {
        self.folders.retain(|f| !same_path(Path::new(&f.path), path));
    }
}

fn toggle(list: &mut Vec<String>, uid: &str) {
    match list.iter().position(|u| u == uid) {
        Some(i) => {
            list.remove(i);
        }
        None => list.push(uid.to_string()),
    }
}

/// What scanning a file gave (§4, §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Error,
    Crashed,
    Timeout,
    /// Loaded, but no instrument or effect in it.
    Empty,
}

impl Status {
    /// Scanned again only with "Rescan plugins with errors" (or Rescan all, or when the file changes).
    pub fn failed(self) -> bool {
        matches!(self, Status::Error | Status::Crashed | Status::Timeout)
    }

    /// The plugin table's Status column.
    pub fn text(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Error => "error",
            Status::Crashed => "crashed",
            Status::Timeout => "timed out",
            Status::Empty => "no plugins",
        }
    }
}

/// A plugin a scanned file offers (a `Descriptor` without its format and path).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub kind: String,
}

impl Record {
    pub fn of(d: &Descriptor) -> Record {
        Record {
            id: d.id.clone(),
            name: d.name.clone(),
            vendor: d.vendor.clone(),
            version: d.version.clone(),
            features: d.features.clone(),
            kind: kind_str(d.kind).into(),
        }
    }

    pub fn descriptor(&self, format: Format, path: &Path) -> Descriptor {
        Descriptor {
            format,
            id: self.id.clone(),
            name: self.name.clone(),
            vendor: self.vendor.clone(),
            version: self.version.clone(),
            features: self.features.clone(),
            kind: kind_of(&self.kind),
            path: path.to_path_buf(),
        }
    }

    /// An instrument or an effect (offered in the menus).
    pub fn usable(&self) -> bool {
        kind_of(&self.kind) != Kind::Other
    }
}

fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::Instrument => "instrument",
        Kind::Effect => "effect",
        Kind::Other => "other",
    }
}

fn kind_of(s: &str) -> Kind {
    match s {
        "instrument" => Kind::Instrument,
        "effect" => Kind::Effect,
        _ => Kind::Other,
    }
}

/// The table's Type column.
fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Instrument => "Instrument",
        Kind::Effect => "Effect",
        Kind::Other => "Other",
    }
}

/// One scanned file (§5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileScan {
    pub path: String,
    pub format: String,
    #[serde(default)]
    pub size: u64,
    /// Seconds since 1970.
    #[serde(default)]
    pub modified: u64,
    pub status: Status,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub plugins: Vec<Record>,
}

/// The scan results: `plugin_cache.json` (§5).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cache {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub files: Vec<FileScan>,
}

impl Cache {
    pub fn load() -> Option<Cache> {
        read_json(CACHE_FILE)
    }

    pub fn save(&self) {
        write_json(CACHE_FILE, self);
    }

    /// Unique instruments and effects found.
    fn uids(&self) -> HashSet<String> {
        self.files
            .iter()
            .filter(|f| f.status == Status::Ok)
            .flat_map(|f| f.plugins.iter().filter(|r| r.usable()).map(move |r| format!("{}:{}", f.format, r.id)))
            .collect()
    }

    fn errors(&self) -> usize {
        self.files.iter().filter(|f| f.status.failed()).count()
    }
}

/// Where a search folder comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirKind {
    /// A format's documented install folder (or `CLAP_PATH`): searched for that format, can be switched off.
    Standard(Format),
    /// Added by the user: searched for every format.
    Added,
}

/// A folder the scan searches (§2).
#[derive(Clone, Debug, PartialEq)]
pub struct SearchDir {
    pub path: PathBuf,
    pub kind: DirKind,
    pub on: bool,
}

impl SearchDir {
    /// The tag at the row's right end.
    pub fn tag(&self) -> &'static str {
        match self.kind {
            DirKind::Standard(Format::Clap) => "CLAP",
            DirKind::Standard(Format::Vst3) => "VST3",
            DirKind::Standard(Format::Native) => "native",
            DirKind::Added => "All formats",
        }
    }
}

/// Every search folder: the VST 3 and CLAP standard folders, then the added ones.
pub fn search_dirs(p: &Prefs) -> Vec<SearchDir> {
    let standard = os_vst3::standard_dirs().into_iter().map(|d| (d, Format::Vst3)).chain(os_clap::standard_dirs().into_iter().map(|d| (d, Format::Clap)));
    let mut v: Vec<SearchDir> = Vec::new();
    for (path, f) in standard {
        if !v.iter().any(|d| d.kind == DirKind::Standard(f) && same_path(&d.path, &path)) {
            let on = !p.standard_off.iter().any(|o| same_path(Path::new(o), &path));
            v.push(SearchDir { path, kind: DirKind::Standard(f), on });
        }
    }
    v.extend(p.folders.iter().map(|f| SearchDir { path: PathBuf::from(&f.path), kind: DirKind::Added, on: f.on }));
    v
}

/// A path compared without case and trailing separators on Windows.
fn path_key(p: &Path) -> String {
    let mut s = p.to_string_lossy().into_owned();
    if cfg!(windows) {
        s = s.replace('/', "\\").to_lowercase();
    }
    while s.len() > 1 && (s.ends_with('\\') || s.ends_with('/')) && !s.ends_with(":\\") {
        s.pop();
    }
    s
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b)
}

/// A plugin file the search found.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub path: PathBuf,
    pub format: Format,
}

/// How deep below a search folder plugin files are looked for.
const MAX_DEPTH: u32 = 8;

/// The plugin files in the switched-on folders (§4): `.clap` files (and bundles on macOS) and `.vst3` bundles or files (a
/// bundle is not searched inside), then the `native` files; each file once, files of the built-in Lemon plugins left out.
pub fn discover(dirs: &[SearchDir], native: Vec<PathBuf>) -> Vec<Found> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut walked = HashSet::new();
    for d in dirs.iter().filter(|d| d.on) {
        let (clap, vst3) = match d.kind {
            DirKind::Standard(Format::Clap) => (true, false),
            DirKind::Standard(Format::Vst3) => (false, true),
            DirKind::Standard(Format::Native) => (false, false),
            DirKind::Added => (true, true),
        };
        walk(&d.path, 0, (clap, vst3), &mut walked, &mut seen, &mut out);
    }
    for p in native {
        if seen.insert(path_key(&p)) {
            out.push(Found { path: p, format: Format::Native });
        }
    }
    out.retain(|f| !builtin_file(&f.path));
    out
}

fn walk(dir: &Path, depth: u32, (clap, vst3): (bool, bool), walked: &mut HashSet<(PathBuf, bool, bool)>, seen: &mut HashSet<String>, out: &mut Vec<Found>) {
    // A folder reached twice (links, nested search folders) is searched once.
    let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if depth > MAX_DEPTH || !walked.insert((canon, clap, vst3)) {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort_by_key(|p| path_key(p));
    for p in entries {
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase());
        let format = match ext.as_deref() {
            Some("clap") if clap && os_clap::is_plugin(&p) => Some(Format::Clap),
            Some("vst3") if vst3 && (p.is_dir() || p.is_file()) => Some(Format::Vst3),
            _ => None,
        };
        match format {
            Some(format) => {
                if seen.insert(path_key(&p)) {
                    out.push(Found { path: p, format });
                }
            }
            None if p.is_dir() => walk(&p, depth + 1, (clap, vst3), walked, seen, out),
            None => {}
        }
    }
}

/// A file named like a built-in Lemon plugin, or like one by its earlier "Open" name (same plugin id): the built-in copy
/// is used (a second copy in the process would share the built-in one's editor window class).
fn builtin_file(p: &Path) -> bool {
    let builtin = [crate::plugins::LEMON_SAMPLER, crate::plugins::LEMON_EQ, crate::osc3::LEMON_3X_OSC];
    let old = crate::plugins::OLD_NAMES.map(|(old, _)| old);
    p.file_stem().is_some_and(|s| builtin.iter().chain(&old).any(|b| s.eq_ignore_ascii_case(b)))
}

/// A file's size and modification time (§4): a bundle's module inside it (VST 3; CLAP on macOS), else the file itself.
pub fn stamp(path: &Path, format: Format) -> (u64, u64) {
    let module = match format {
        Format::Vst3 => os_vst3::module_file(path),
        Format::Clap => os_clap::module_file(path),
        Format::Native => path.to_path_buf(),
    };
    let file = if module.exists() { module } else { path.to_path_buf() };
    let Ok(md) = std::fs::metadata(&file) else { return (0, 0) };
    let modified = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    (if md.is_dir() { 0 } else { md.len() }, modified)
}

/// Which files a sweep scans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// New and changed files (Find new plugins).
    Changed,
    /// Every file (Rescan all).
    All,
}

/// What scanning one file gave.
#[derive(Clone, Debug, PartialEq)]
pub struct Scanned {
    pub status: Status,
    pub error: String,
    pub plugins: Vec<Record>,
}

impl Scanned {
    pub fn ok(plugins: Vec<Record>) -> Scanned {
        Scanned { status: Status::Ok, error: String::new(), plugins }
    }

    pub fn failed(status: Status, error: impl Into<String>) -> Scanned {
        Scanned { status, error: error.into(), plugins: Vec::new() }
    }
}

/// Scans the files of `found` that need it (§4 "Cache") with up to `workers` scans at a time; the others keep their
/// result from `old`. `progress(n, total, file)` is called as each scan starts. Files no longer found drop out.
pub fn sweep(
    found: &[Found],
    old: &Cache,
    mode: Mode,
    rescan_errors: bool,
    workers: usize,
    scan: &(dyn Fn(&Path, Format) -> Scanned + Sync),
    progress: &(dyn Fn(usize, usize, &Path) + Sync),
) -> Cache {
    let old_by: HashMap<(String, &str), &FileScan> = old.files.iter().map(|f| ((path_key(Path::new(&f.path)), f.format.as_str()), f)).collect();
    let mut files: Vec<Option<FileScan>> = vec![None; found.len()];
    let mut todo = Vec::new();
    for (i, f) in found.iter().enumerate() {
        let (size, modified) = stamp(&f.path, f.format);
        let prev = old_by.get(&(path_key(&f.path), f.format.as_str()));
        match prev {
            Some(p) if mode == Mode::Changed && p.size == size && p.modified == modified && !(p.status.failed() && rescan_errors) => {
                files[i] = Some((*p).clone());
            }
            _ => todo.push((i, size, modified)),
        }
    }
    let total = todo.len();
    let next = AtomicUsize::new(0);
    let results = Mutex::new(files);
    std::thread::scope(|sc| {
        for _ in 0..workers.max(1).min(total) {
            sc.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(i, size, modified)) = todo.get(k) else { break };
                    let f = &found[i];
                    progress(k + 1, total, &f.path);
                    let r = scan(&f.path, f.format);
                    let status = if r.status == Status::Ok && !r.plugins.iter().any(Record::usable) { Status::Empty } else { r.status };
                    let fs = FileScan {
                        path: f.path.to_string_lossy().into_owned(),
                        format: f.format.as_str().into(),
                        size,
                        modified,
                        status,
                        error: r.error,
                        plugins: r.plugins,
                    };
                    if let Ok(mut v) = results.lock() {
                        v[i] = Some(fs);
                    }
                }
            });
        }
    });
    Cache { version: 1, files: results.into_inner().unwrap_or_default().into_iter().flatten().collect() }
}

/// What changed between two scans (§4 "Results").
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Report {
    pub added: usize,
    pub gone: usize,
    pub total: usize,
    pub errors: usize,
    pub old_errors: usize,
}

pub fn compare(old: &Cache, new: &Cache) -> Report {
    let (a, b) = (old.uids(), new.uids());
    Report { added: b.difference(&a).count(), gone: a.difference(&b).count(), total: b.len(), errors: new.errors(), old_errors: old.errors() }
}

impl Report {
    /// The hint bar's message.
    pub fn message(&self) -> String {
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("{} new plugin{}", self.added, if self.added == 1 { "" } else { "s" }));
        }
        if self.gone > 0 {
            parts.push(format!("{} gone", self.gone));
        }
        if parts.is_empty() && self.errors == self.old_errors {
            return "Plugin scan: no changes".into();
        }
        let mut m = if parts.is_empty() { format!("Plugin scan: {} plugins", self.total) } else { format!("Plugin scan: {} ({} in all)", parts.join(", "), self.total) };
        if self.errors > 0 {
            m.push_str(&format!(", {} file{} with errors", self.errors, if self.errors == 1 { "" } else { "s" }));
        }
        m
    }
}

/// Trailing format markers left out when names are compared (§3), longest first.
const MARKERS: [&str; 10] = [" (clap)", " (vst3)", " (vst2)", " (vst)", " (x64)", " clap", " vst3", " vst2", " vst", " x64"];

/// `name` without trailing format markers (its own case).
pub fn strip_marker(name: &str) -> &str {
    let mut s = name.trim();
    'again: loop {
        for m in MARKERS {
            if s.len() > m.len() && s.is_char_boundary(s.len() - m.len()) && s[s.len() - m.len()..].eq_ignore_ascii_case(m) {
                s = s[..s.len() - m.len()].trim_end();
                continue 'again;
            }
        }
        return s;
    }
}

/// The name plugins are matched by (§3): without markers, lower case.
fn base_name(name: &str) -> String {
    strip_marker(name).to_lowercase()
}

/// Vendors are compatible when one is empty or, letters and digits only, one starts with the other (§3).
fn vendors_match(a: &str, b: &str) -> bool {
    let k = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let (a, b) = (k(a), k(b));
    a.is_empty() || b.is_empty() || a.starts_with(&b) || b.starts_with(&a)
}

/// Lower ranks are listed first: the built-in plugins, the preferred format, then VST3, CLAP, (VST2), native.
fn rank(d: &Descriptor, prefer: Prefer) -> usize {
    if crate::project_plugins::is_builtin(d) {
        return 0;
    }
    let preferred = match prefer {
        Prefer::Vst3 => Some(Format::Vst3),
        Prefer::Clap => Some(Format::Clap),
        Prefer::Vst2 => None,
    };
    if Some(d.format) == preferred {
        return 1;
    }
    match d.format {
        Format::Vst3 => 2,
        Format::Clap => 3,
        Format::Native => 5,
    }
}

/// For each plugin of `list` (one kind): `Some(j)` when combining lists `list[j]` instead, a more preferred format of
/// the same plugin among the copies that are neither hidden nor missing (§3). Hidden and missing plugins get `None`.
pub fn combined_into(list: &[Available], prefs: &Prefs, missing: &HashSet<String>) -> Vec<Option<usize>> {
    let mut out = vec![None; list.len()];
    if !prefs.combine {
        return out;
    }
    let uid: Vec<String> = list.iter().map(|a| a.desc.uid()).collect();
    let live: Vec<bool> = uid.iter().map(|u| !prefs.is_hidden(u) && !missing.contains(u)).collect();
    let base: Vec<String> = list.iter().map(|a| base_name(&a.desc.name)).collect();
    let ranks: Vec<usize> = list.iter().map(|a| rank(&a.desc, prefs.prefer)).collect();
    for i in (0..list.len()).filter(|&i| live[i]) {
        let better = (0..list.len())
            .filter(|&j| j != i && live[j] && base[j] == base[i] && list[j].desc.format != list[i].desc.format)
            .filter(|&j| ranks[j] < ranks[i] && vendors_match(&list[j].desc.vendor, &list[i].desc.vendor))
            .min_by_key(|&j| (ranks[j], j));
        out[i] = better;
    }
    out
}

/// A format's name as the lists show it.
pub fn format_label(f: Format) -> &'static str {
    crate::plugins::format_name(f)
}

/// Generator categories, then effect categories, in table order (spec: ui/plugin-menu.md §4); "Other" last.
const GEN_CATS: &[(&str, &[&str])] = &[("Drum", &["drum", "drum-machine"]), ("Sampler", &["sampler", "granular"]), ("Keys", &["piano"]), ("Synth", &["synthesizer", "synth"])];
const FX_CATS: &[(&str, &[&str])] = &[
    ("Analyzer", &["analyzer"]),
    ("Dynamics", &["dynamics", "compressor", "limiter", "expander", "gate", "transient-shaper", "de-esser"]),
    ("EQ", &["eq", "equalizer"]),
    ("Filter", &["filter"]),
    ("Distortion", &["distortion"]),
    ("Modulation", &["modulation", "chorus", "flanger", "phaser", "tremolo", "frequency-shifter"]),
    ("Delay", &["delay"]),
    ("Reverb", &["reverb"]),
    ("Pitch", &["pitch shift", "pitch-shifter", "pitch-correction", "phase-vocoder"]),
    ("Spatial", &["spatial", "up-downmix"]),
    ("Utility", &["tools", "utility", "mixing", "mastering", "restoration", "multi-effects", "glitch", "generator", "network"]),
];
pub const OTHER: &str = "Other";

/// The categories of the generator (`effects` false) or effect lists, in menu order.
pub fn categories(effects: bool) -> impl Iterator<Item = &'static str> {
    (if effects { FX_CATS } else { GEN_CATS }).iter().map(|(c, _)| *c).chain(std::iter::once(OTHER))
}

/// A plugin's category (§4): the first category in table order with a matching feature string.
pub fn category(d: &Descriptor) -> &'static str {
    match d.name.as_str() {
        n if n == crate::plugins::LEMON_SAMPLER => return "Sampler",
        n if n == crate::osc3::LEMON_3X_OSC => return "Synth",
        n if n == crate::plugins::LEMON_EQ => return "EQ",
        _ => {}
    }
    let feats: Vec<String> = d.features.iter().map(|f| f.trim().to_lowercase()).collect();
    let table = if d.is_effect() { FX_CATS } else { GEN_CATS };
    table.iter().find(|(_, keys)| keys.iter().any(|k| feats.iter().any(|f| f == k))).map_or(OTHER, |(c, _)| c)
}

/// The built-in plugins' vendor in the menus.
pub const BUILTIN_VENDOR: &str = "Lemon Studio";
pub const NO_VENDOR: &str = "(unknown vendor)";

/// A plugin as the menus list it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Index into `Plugins::available` (generators) or `Plugins::effects`.
    pub index: usize,
    pub name: String,
    /// The name, plus its format when another format of it is listed too.
    pub label: String,
    pub uid: String,
    /// The vendor as "By vendor" groups it.
    pub vendor: String,
    pub format: Format,
    pub category: &'static str,
    pub favorite: bool,
}

/// What the plugin menus list: shown plugins only, alphabetical by label.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lists {
    pub gens: Vec<Entry>,
    pub fx: Vec<Entry>,
}

pub fn lists(available: &[Available], effects: &[Available], prefs: &Prefs, missing: &HashSet<String>) -> Lists {
    Lists { gens: entries(available, prefs, missing), fx: entries(effects, prefs, missing) }
}

fn entries(list: &[Available], prefs: &Prefs, missing: &HashSet<String>) -> Vec<Entry> {
    let into = combined_into(list, prefs, missing);
    let shown: Vec<usize> = (0..list.len()).filter(|&i| into[i].is_none()).filter(|&i| { let u = list[i].desc.uid(); !prefs.is_hidden(&u) && !missing.contains(&u) }).collect();
    let mut v: Vec<Entry> = shown
        .iter()
        .map(|&i| {
            let d = &list[i].desc;
            let twin = shown.iter().any(|&k| k != i && list[k].desc.format != d.format && base_name(&list[k].desc.name) == base_name(&d.name));
            let label = if twin { format!("{} ({})", strip_marker(&d.name), format_label(d.format)) } else { d.name.clone() };
            let vendor = if crate::project_plugins::is_builtin(d) {
                BUILTIN_VENDOR.to_string()
            } else if d.vendor.trim().is_empty() {
                NO_VENDOR.to_string()
            } else {
                d.vendor.trim().to_string()
            };
            let uid = d.uid();
            Entry { index: i, name: d.name.clone(), label, favorite: prefs.is_favorite(&uid), uid, vendor, format: d.format, category: category(d) }
        })
        .collect();
    v.sort_by_key(|e| e.label.to_lowercase());
    v
}

/// Plugin table columns (§2 group 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Col {
    Star,
    Show,
    Name,
    Format,
    Type,
    Vendor,
    Status,
    File,
}

impl Col {
    pub const ALL: [Col; 8] = [Col::Star, Col::Show, Col::Name, Col::Format, Col::Type, Col::Vendor, Col::Status, Col::File];

    pub fn title(self) -> &'static str {
        match self {
            Col::Star => "★",
            Col::Show => "Show",
            Col::Name => "Name",
            Col::Format => "Format",
            Col::Type => "Type",
            Col::Vendor => "Vendor",
            Col::Status => "Status",
            Col::File => "File",
        }
    }
}

/// One row of the plugin table.
#[derive(Clone, Debug, PartialEq)]
pub struct TableRow {
    /// Empty for a file that failed (no ★ / Show).
    pub uid: String,
    pub name: String,
    pub format: &'static str,
    pub kind: &'static str,
    pub vendor: String,
    pub status: String,
    /// File name; `path` is the whole path (the hint).
    pub file: String,
    pub path: String,
    /// Why a file failed, or which format is listed instead.
    pub note: String,
    pub favorite: bool,
    pub shown: bool,
    /// Lost to a preferred format (drawn dim).
    pub dim: bool,
    /// Has ★ and Show (instruments and effects).
    pub toggles: bool,
}

fn file_name(path: &str) -> String {
    Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Every row of the plugin table: the plugins of `available` and `effects` (built-in and found), then the files that
/// gave no plugin and the plugins of other kinds from the cache.
pub fn table(available: &[Available], effects: &[Available], cache: &Cache, prefs: &Prefs, missing: &HashSet<String>) -> Vec<TableRow> {
    let mut rows = Vec::new();
    for (list, kind) in [(available, Kind::Instrument), (effects, Kind::Effect)] {
        let into = combined_into(list, prefs, missing);
        for (i, a) in list.iter().enumerate() {
            let d = &a.desc;
            let uid = d.uid();
            if missing.contains(&uid) {
                continue;
            }
            let builtin = crate::project_plugins::is_builtin(d);
            let path = d.path.to_string_lossy().into_owned();
            let note = into[i].map(|j| format!("Listed as {} ({})", list[j].desc.name, format_label(list[j].desc.format))).unwrap_or_default();
            rows.push(TableRow {
                name: d.name.clone(),
                format: format_label(d.format),
                kind: kind_label(kind),
                vendor: if builtin { BUILTIN_VENDOR.into() } else { d.vendor.clone() },
                status: if builtin { "built in".into() } else { "ok".into() },
                file: if builtin { "(built in)".into() } else { file_name(&path) },
                path: if builtin { String::new() } else { path },
                note,
                favorite: prefs.is_favorite(&uid),
                shown: !prefs.is_hidden(&uid),
                dim: into[i].is_some(),
                toggles: true,
                uid,
            });
        }
    }
    for f in &cache.files {
        let format = Format::parse(&f.format).map_or("?", format_label);
        if f.status == Status::Ok {
            for r in f.plugins.iter().filter(|r| !r.usable()) {
                rows.push(TableRow {
                    uid: String::new(),
                    name: r.name.clone(),
                    format,
                    kind: kind_label(Kind::Other),
                    vendor: r.vendor.clone(),
                    status: "ok".into(),
                    file: file_name(&f.path),
                    path: f.path.clone(),
                    note: "Not an instrument or an effect: not listed".into(),
                    favorite: false,
                    shown: false,
                    dim: true,
                    toggles: false,
                });
            }
            continue;
        }
        rows.push(TableRow {
            uid: String::new(),
            name: Path::new(&f.path).file_stem().map_or_else(|| f.path.clone(), |s| s.to_string_lossy().into_owned()),
            format,
            kind: "-",
            vendor: String::new(),
            status: f.status.text().into(),
            file: file_name(&f.path),
            path: f.path.clone(),
            note: f.error.clone(),
            favorite: false,
            shown: false,
            dim: false,
            toggles: false,
        });
    }
    rows
}

/// The rows whose name, vendor or file contain `find` (any case), sorted by `col` (`rev`: reversed), then by name.
pub fn sort_filter(rows: &[TableRow], col: Col, rev: bool, find: &str) -> Vec<TableRow> {
    let find = find.trim().to_lowercase();
    let mut v: Vec<TableRow> = rows
        .iter()
        .filter(|r| find.is_empty() || [&r.name, &r.vendor, &r.file].iter().any(|s| s.to_lowercase().contains(&find)))
        .cloned()
        .collect();
    let text = |r: &TableRow| -> String {
        match col {
            Col::Star => (if r.favorite { "0" } else { "1" }).into(),
            Col::Show => (if r.shown { "0" } else { "1" }).into(),
            Col::Name => String::new(),
            Col::Format => r.format.to_lowercase(),
            Col::Type => r.kind.to_lowercase(),
            Col::Vendor => r.vendor.to_lowercase(),
            Col::Status => r.status.to_lowercase(),
            Col::File => r.file.to_lowercase(),
        }
    };
    v.sort_by_cached_key(|r| (text(r), r.name.to_lowercase(), r.format));
    if rev {
        v.reverse();
    }
    v
}

/// Scan progress, for the status line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub file: String,
    /// No cache yet: the hint bar says "Looking for plugins...".
    pub first: bool,
}

/// What the views read (`State::plugdb`): the choices, the last scan, the lists, the table and its view.
#[derive(Debug)]
pub struct Db {
    pub prefs: Prefs,
    pub cache: Cache,
    pub lists: Lists,
    /// Every table row (`table`); `rows` = these sorted and filtered.
    pub all_rows: Vec<TableRow>,
    pub rows: Vec<TableRow>,
    pub sort: (Col, bool),
    pub find: String,
    /// Typing goes into the Find box.
    pub finding: bool,
    /// First table row shown.
    pub top: usize,
    /// While a scan runs.
    pub progress: Option<Progress>,
}

impl Default for Db {
    fn default() -> Db {
        Db {
            prefs: Prefs::default(),
            cache: Cache::default(),
            lists: Lists::default(),
            all_rows: Vec::new(),
            rows: Vec::new(),
            sort: (Col::Name, false),
            find: String::new(),
            finding: false,
            top: 0,
            progress: None,
        }
    }
}

impl Db {
    /// Rebuilds the lists and the table from the plugins the host knows.
    pub fn rebuild(&mut self, plugins: &crate::plugins::Plugins) {
        self.lists = lists(&plugins.available, &plugins.effects, &self.prefs, &plugins.missing);
        self.all_rows = table(&plugins.available, &plugins.effects, &self.cache, &self.prefs, &plugins.missing);
        self.refilter();
    }

    /// Re-sorts and re-filters the table (sort column, Find text).
    pub fn refilter(&mut self) {
        self.rows = sort_filter(&self.all_rows, self.sort.0, self.sort.1, &self.find);
        self.top = self.top.min(self.rows.len().saturating_sub(1));
    }

    /// The Scanning group's status line (§2).
    pub fn status_line(&self) -> String {
        if let Some(p) = &self.progress {
            return if p.total == 0 { "Looking for plugin files...".into() } else { format!("Scanning {} of {}: {}", p.done, p.total, p.file) };
        }
        let files = self.cache.files.len();
        let plugins = self.cache.uids().len();
        let errors = self.cache.errors();
        let mut s = format!("{plugins} plugin{} in {files} file{}", if plugins == 1 { "" } else { "s" }, if files == 1 { "" } else { "s" });
        if errors > 0 {
            s.push_str(&format!(", {errors} with errors"));
        }
        s
    }
}

/// A library that only lists descriptors (nothing is loaded): plugin lists for tests.
#[cfg(test)]
pub(crate) struct Listed(PathBuf, Vec<Descriptor>);

#[cfg(test)]
impl os_host::Library for Listed {
    fn path(&self) -> &Path {
        &self.0
    }
    fn plugins(&self) -> &[Descriptor] {
        &self.1
    }
    fn create(self: std::sync::Arc<Self>, _: &str) -> Result<os_host::SharedPlugin, String> {
        Err("not loadable".into())
    }
}

/// A CLAP plugin `name` that can be listed but not loaded (tests).
#[cfg(test)]
pub(crate) fn fake(name: &str, kind: Kind) -> Available {
    let d = Descriptor {
        format: Format::Clap,
        id: format!("test.{}", name.to_lowercase().replace(' ', "-")),
        name: name.into(),
        vendor: "Test".into(),
        version: "1.0".into(),
        features: Vec::new(),
        kind,
        path: PathBuf::from(format!("C:/Plugins/{name}.clap")),
    };
    Available { lib: std::sync::Arc::new(Listed(d.path.clone(), vec![d.clone()])), desc: d }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn desc(format: Format, id: &str, name: &str, vendor: &str, kind: Kind, features: &[&str]) -> Descriptor {
        Descriptor {
            format,
            id: id.into(),
            name: name.into(),
            vendor: vendor.into(),
            version: "1.0".into(),
            features: features.iter().map(|s| s.to_string()).collect(),
            kind,
            path: PathBuf::from(format!("C:/Plugins/{name}.{}", format.as_str())),
        }
    }

    pub(crate) fn avail(d: Descriptor) -> Available {
        Available { lib: Arc::new(Listed(d.path.clone(), vec![d.clone()])), desc: d }
    }

    fn fx(format: Format, id: &str, name: &str, vendor: &str) -> Available {
        avail(desc(format, id, name, vendor, Kind::Effect, &["audio-effect", "reverb"]))
    }

    fn labels(l: &[Entry]) -> Vec<&str> {
        l.iter().map(|e| e.label.as_str()).collect()
    }

    #[test]
    fn markers_and_vendors() {
        assert_eq!(strip_marker("Serum (x64) VST3"), "Serum");
        assert_eq!(strip_marker("Vital (CLAP)"), "Vital");
        assert_eq!(strip_marker("VST"), "VST");
        assert_eq!(strip_marker("Surge XT"), "Surge XT");
        assert!(vendors_match("FabFilter", "Fabfilter Software Instruments"));
        assert!(vendors_match("", "Anyone"));
        assert!(!vendors_match("Xfer Records", "Vital Audio"));
    }

    /// Combining (spec: ui/plugin-manager.md §3): one entry per plugin in the preferred format, the next format when the
    /// preferred copy is hidden, format labels when combining is off.
    #[test]
    fn formats_combine_by_preference() {
        let list = vec![
            fx(Format::Clap, "com.u.verb", "Verb", "U Audio"),
            fx(Format::Vst3, "AAAA", "Verb", "U Audio Inc."),
            fx(Format::Clap, "com.x.other", "Other Verb", "X"),
            fx(Format::Vst3, "BBBB", "Verb", "Somebody Else"),
        ];
        let mut prefs = Prefs::default();
        let none = HashSet::new();
        let l = lists(&[], &list, &prefs, &none);
        // VST3 first: the U Audio CLAP copy goes; another vendor's "Verb" stays (same format: no format labels).
        assert_eq!(l.fx.iter().map(|e| e.index).collect::<Vec<_>>(), vec![2, 1, 3]);
        assert_eq!(labels(&l.fx), vec!["Other Verb", "Verb", "Verb"]);
        assert_eq!(combined_into(&list, &prefs, &none), vec![Some(1), None, None, None]);
        prefs.prefer = Prefer::Clap;
        let l = lists(&[], &list, &prefs, &none);
        assert!(l.fx.iter().any(|e| e.index == 0) && !l.fx.iter().any(|e| e.index == 1));
        // Hiding the preferred copy lists the next one.
        prefs.toggle_hidden("clap:com.u.verb");
        let l = lists(&[], &list, &prefs, &none);
        assert!(l.fx.iter().any(|e| e.index == 1) && !l.fx.iter().any(|e| e.index == 0));
        // Off: every format, labelled.
        prefs = Prefs { combine: false, ..Prefs::default() };
        let l = lists(&[], &list, &prefs, &none);
        assert_eq!(labels(&l.fx), vec!["Other Verb", "Verb (CLAP)", "Verb (VST3)", "Verb (VST3)"]);
        // Missing plugins are not listed and do not win.
        let gone: HashSet<String> = ["vst3:AAAA".to_string()].into();
        let l = lists(&[], &list, &Prefs::default(), &gone);
        assert!(l.fx.iter().any(|e| e.index == 0) && !l.fx.iter().any(|e| e.index == 1));
    }

    #[test]
    fn categories_follow_the_feature_tables() {
        let c = |kind, f: &[&str]| category(&desc(Format::Clap, "x", "X", "", kind, f));
        assert_eq!(c(Kind::Instrument, &["instrument", "synthesizer", "stereo"]), "Synth");
        assert_eq!(c(Kind::Instrument, &["Instrument", "Drum"]), "Drum");
        assert_eq!(c(Kind::Instrument, &["instrument"]), "Other");
        assert_eq!(c(Kind::Effect, &["Fx", "EQ"]), "EQ");
        assert_eq!(c(Kind::Effect, &["audio-effect", "compressor", "limiter"]), "Dynamics");
        assert_eq!(c(Kind::Effect, &["Fx", "Pitch Shift"]), "Pitch");
        assert_eq!(c(Kind::Effect, &["audio-effect", "mono"]), "Other");
        assert_eq!(categories(true).last(), Some("Other"));
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("os_plugdb_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Discovery (§4): .clap files (bundles on macOS) and .vst3 bundles, recursively, not inside bundles; standard folders only for their
    /// format; switched-off folders and built-in Lemon plugin files (also by their earlier "Open" names) are skipped.
    #[test]
    fn discovery_finds_plugin_files() {
        let d = temp("discover");
        let w = |p: &str| {
            let p = d.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        };
        w("added/Vendor/Synth.clap");
        w("added/Vendor/Deep/Er/Fx.vst3");
        w("added/Bundle.vst3/Contents/x86_64-win/Bundle.vst3");
        w("added/Bundle.vst3/Contents/x86_64-win/Inner.clap");
        w("added/Lemon Sampler.clap");
        w("added/Open Parametric EQ 2.vst3/Contents/x86_64-win/Open Parametric EQ 2.vst3");
        w("added/readme.txt");
        // A macOS CLAP bundle: a plugin on macOS, a plain folder elsewhere.
        w("added/Mac/Keys.clap/Contents/MacOS/Keys");
        w("clap/OnlyClap.clap");
        w("clap/NotHere.vst3");
        w("off/Hidden.clap");
        let dirs = vec![
            SearchDir { path: d.join("clap"), kind: DirKind::Standard(Format::Clap), on: true },
            SearchDir { path: d.join("added"), kind: DirKind::Added, on: true },
            SearchDir { path: d.join("off"), kind: DirKind::Added, on: false },
            SearchDir { path: d.join("added").join("Vendor"), kind: DirKind::Added, on: true },
        ];
        let found = discover(&dirs, Vec::new());
        let names: Vec<(String, Format)> = found.iter().map(|f| (f.path.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/"), f.format)).collect();
        let mut want = vec![("clap/OnlyClap.clap".to_string(), Format::Clap), ("added/Bundle.vst3".to_string(), Format::Vst3)];
        if cfg!(target_os = "macos") {
            want.push(("added/Mac/Keys.clap".to_string(), Format::Clap));
        }
        want.extend([("added/Vendor/Deep/Er/Fx.vst3".to_string(), Format::Vst3), ("added/Vendor/Synth.clap".to_string(), Format::Clap)]);
        assert_eq!(names, want);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The sweep (§4 "Cache"): unchanged files keep their result, changed and new ones are scanned, failed ones only
    /// with the option, Rescan all scans everything, gone files drop out; the report counts the changes.
    #[test]
    fn sweep_reuses_unchanged_files() {
        let d = temp("sweep");
        for f in ["A.clap", "B.clap", "Bad.clap"] {
            std::fs::write(d.join(f), b"plugin").unwrap();
        }
        let found = |names: &[&str]| names.iter().map(|n| Found { path: d.join(n), format: Format::Clap }).collect::<Vec<_>>();
        let scanned = Mutex::new(Vec::<String>::new());
        let scan = |p: &Path, _: Format| -> Scanned {
            let n = p.file_stem().unwrap().to_string_lossy().into_owned();
            scanned.lock().unwrap().push(n.clone());
            if n == "Bad" {
                return Scanned::failed(Status::Crashed, "ended with error 0xC0000005");
            }
            Scanned::ok(vec![Record { id: format!("id.{n}"), name: n, vendor: String::new(), version: String::new(), features: vec![], kind: "effect".into() }])
        };
        let progress = |_: usize, _: usize, _: &Path| {};
        let take = || std::mem::take(&mut *scanned.lock().unwrap());
        let first = sweep(&found(&["A.clap", "B.clap", "Bad.clap"]), &Cache::default(), Mode::Changed, false, 4, &scan, &progress);
        let mut s = take();
        s.sort();
        assert_eq!(s, vec!["A", "B", "Bad"]);
        assert_eq!(first.files.iter().map(|f| f.status).collect::<Vec<_>>(), vec![Status::Ok, Status::Ok, Status::Crashed]);
        let r = compare(&Cache::default(), &first);
        assert_eq!((r.added, r.total, r.errors), (2, 2, 1));
        assert_eq!(r.message(), "Plugin scan: 2 new plugins (2 in all), 1 file with errors");
        // Nothing changed: nothing scanned.
        let again = sweep(&found(&["A.clap", "B.clap", "Bad.clap"]), &first, Mode::Changed, false, 4, &scan, &progress);
        assert!(take().is_empty());
        assert_eq!(again, first);
        assert_eq!(compare(&first, &again).message(), "Plugin scan: no changes");
        // With the option the failed file is tried again; a changed file is scanned; a gone file drops out.
        std::fs::write(d.join("B.clap"), b"plugin, a newer version").unwrap();
        let next = sweep(&found(&["B.clap", "Bad.clap"]), &first, Mode::Changed, true, 2, &scan, &progress);
        let mut s = take();
        s.sort();
        assert_eq!(s, vec!["B", "Bad"]);
        assert_eq!(next.files.len(), 2);
        assert_eq!(compare(&first, &next).gone, 1);
        // Rescan all.
        sweep(&found(&["B.clap", "Bad.clap"]), &next, Mode::All, false, 1, &scan, &progress);
        assert_eq!(take().len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn files_round_trip_and_take_defaults() {
        let p: Prefs = serde_json::from_str(r#"{"prefer":"clap","folders":[{"path":"D:\\Plugins"}]}"#).unwrap();
        assert_eq!(p.prefer, Prefer::Clap);
        assert!(p.combine && p.startup_scan && p.folders[0].on);
        let text = serde_json::to_string(&p).unwrap();
        assert!(text.contains("\"menu_view\":\"categories\""));
        assert_eq!(serde_json::from_str::<Prefs>(&text).unwrap(), p);
        let c = Cache {
            version: 1,
            files: vec![FileScan { path: "a.clap".into(), format: "clap".into(), size: 3, modified: 4, status: Status::Timeout, error: "x".into(), plugins: vec![] }],
        };
        let text = serde_json::to_string(&c).unwrap();
        assert!(text.contains("\"status\":\"timeout\""));
        assert_eq!(serde_json::from_str::<Cache>(&text).unwrap(), c);
    }

    #[test]
    fn folders_toggle_and_table_sorts() {
        let mut p = Prefs::default();
        let d = PathBuf::from("D:/My Plugins");
        assert!(p.add_folder(&d));
        // The same folder spelled another way: case and separators count only on Windows.
        let again = if cfg!(windows) { "d:\\my plugins\\" } else { "D:/My Plugins/" };
        assert!(!p.add_folder(Path::new(again)), "the same folder once");
        p.toggle_folder(&d);
        assert!(!p.folders[0].on);
        if let Some(std) = search_dirs(&p).into_iter().find(|s| s.kind != DirKind::Added) {
            p.toggle_folder(&std.path);
            assert!(!search_dirs(&p).iter().find(|s| s.path == std.path).unwrap().on);
            p.toggle_folder(&std.path);
            assert!(p.standard_off.is_empty());
        }
        p.remove_folder(&d);
        assert!(p.folders.is_empty());

        let gens = vec![avail(desc(Format::Vst3, "S1", "Zeta", "B", Kind::Instrument, &["Instrument", "Synth"])), avail(desc(Format::Clap, "s2", "alpha", "A", Kind::Instrument, &[]))];
        let mut prefs = Prefs::default();
        prefs.toggle_favorite("clap:s2");
        let cache = Cache { version: 1, files: vec![FileScan { path: "C:/x/Broken.vst3".into(), format: "vst3".into(), size: 0, modified: 0, status: Status::Error, error: "no factory".into(), plugins: vec![] }] };
        let rows = table(&gens, &[], &cache, &prefs, &HashSet::new());
        assert_eq!(rows.len(), 3);
        let by_name = sort_filter(&rows, Col::Name, false, "");
        assert_eq!(by_name.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["alpha", "Broken", "Zeta"]);
        let starred = sort_filter(&rows, Col::Star, false, "");
        assert_eq!(starred[0].name, "alpha");
        let found = sort_filter(&rows, Col::Name, true, "BROK");
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].status.as_str(), found[0].note.as_str(), found[0].toggles), ("error", "no factory", false));
    }
}
