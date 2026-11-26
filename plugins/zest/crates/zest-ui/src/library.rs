// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The preset library: preset files found in the sources (the user's own presets, the banks built in or dropped in
//! the user folder, then the Diva presets folder), categories from folders, authors from name initials, search, filter
//! chips, and favourites (`favorites.txt`).

use std::path::{Path, PathBuf};

/// Zest's user folder: `$XDG_DATA_HOME/zest`, else `~/.local/share/zest`; `%APPDATA%\\Zest` on Windows,
/// `~/Library/Application Support/Zest` on macOS.
pub fn user_dir() -> PathBuf {
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Zest"));
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Zest"));
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("zest"));
    base.unwrap_or_else(|| PathBuf::from("zest"))
}

/// Where the user's own presets are saved (`.zest`).
pub fn user_presets_dir() -> PathBuf {
    user_dir().join("presets")
}

/// The Diva presets folder (u-he's default location).
pub fn diva_presets_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    return std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Documents\\u-he\\Diva.data\\Presets\\Diva"));
    #[cfg(target_os = "macos")]
    {
        let user = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Audio/Presets/u-he/Diva"));
        return user.filter(|d| d.is_dir()).or_else(|| Some(PathBuf::from("/Library/Audio/Presets/u-he/Diva")));
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Documents/u-he/Diva.data/Presets/Diva"))
}

/// Bank folders taken at run time, besides those built in (see `zest_preset`).
pub fn user_banks_dir() -> PathBuf {
    user_dir().join("banks")
}

/// A folder of presets.
#[derive(Clone, Debug, Default)]
pub struct Source {
    pub root: PathBuf,
    /// File extensions taken (lower case, no dot).
    pub exts: Vec<String>,
    /// The category of files directly in `root`; with `single`, of every file.
    pub name: String,
    /// Every preset in one category (`name`); folders become the sub-category.
    pub single: bool,
    /// The user's own presets (saved by Zest).
    pub user: bool,
    /// Each folder directly in `root` is a bank (`<n> <Bank>/<nn> <Preset>.zest`).
    pub banks: bool,
    /// With `banks`: the banks built into the plugin come first.
    pub built_in: bool,
}

/// The category of the user's own presets.
pub const USER: &str = "User";

/// The sources in their order: the user's presets, the banks, then Diva's.
pub fn default_sources() -> Vec<Source> {
    let mut v = vec![
        Source { root: user_presets_dir(), exts: vec!["zest".into()], name: USER.into(), single: true, user: true, ..Default::default() },
        Source { root: user_banks_dir(), exts: vec!["zest".into()], banks: true, built_in: true, ..Default::default() },
    ];
    if let Some(d) = diva_presets_dir() {
        v.push(Source { root: d, exts: vec!["h2p".into()], name: "Diva".into(), ..Default::default() });
    }
    v
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub path: PathBuf,
    /// The file name without extension.
    pub file: String,
    /// The name without the author's initials.
    pub name: String,
    pub category: String,
    /// The folder below the category's, if any.
    pub sub: Option<String>,
    /// The initials a name starts with ("HS Bass Nine" -> "HS").
    pub author: Option<String>,
    /// The author's name from the file's metadata ("Howard Scarr").
    pub author_name: Option<String>,
    /// The kind of sound, from the file's first category (Diva's tags).
    pub kind: Option<Kind>,
    pub user: bool,
    /// A bank's copy of a preset the Diva folder holds too: listed in its bank only.
    pub shadowed: bool,
    /// Lower-case text searched (file name, category, sub-category).
    search: String,
}

impl Preset {
    pub fn new(path: PathBuf, category: String, sub: Option<String>, user: bool) -> Preset {
        let file = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (author, name) = split_initials(&file);
        let (author, name) = (author.map(str::to_string), name.to_string());
        let search = format!("{} {} {}", file, category, sub.as_deref().unwrap_or("")).to_lowercase();
        Preset { path, name, file, category, sub, author, author_name: None, kind: None, user, shadowed: false, search }
    }

    /// Takes the author's name and the kind of sound from a `.h2p` file's metadata block (`/*@Meta ... */`).
    fn read_meta(&mut self) {
        use std::io::Read;
        let Ok(f) = std::fs::File::open(&self.path) else { return };
        let mut head = Vec::with_capacity(2048);
        if f.take(2048).read_to_end(&mut head).is_err() {
            return;
        }
        let text = String::from_utf8_lossy(&head);
        let Some(meta) = text.strip_prefix("/*@Meta").and_then(|t| t.split("*/").next()) else { return };
        let field = |name: &str| -> Option<String> {
            let after = meta.split(&format!("{name}:")).nth(1)?;
            let v = after.trim_start().strip_prefix('\'')?.split('\'').next()?.trim();
            (!v.is_empty()).then(|| v.to_string())
        };
        let kind = field("Categories").and_then(|c| Kind::of_category(c.split(',').next()?.split(':').next()?.trim()));
        self.describe(kind, field("Author"));
    }

    /// The same from a `.zest` file's header; its name, if it has one, replaces the file's.
    fn read_zest_meta(&mut self, text: &str) {
        let Some(h) = zest_preset::parse_head(text) else { return };
        if let Some(n) = &h.name {
            let (initials, name) = split_initials(n);
            self.author = self.author.take().or(initials.map(str::to_string));
            self.name = name.to_string();
            self.file = n.clone();
        }
        self.describe(h.first_category().and_then(Kind::of_category), h.author.clone());
    }

    /// Sets the kind and the author's name, which are searched too.
    fn describe(&mut self, kind: Option<Kind>, author_name: Option<String>) {
        for word in kind.map(|k| k.label().to_string()).into_iter().chain(author_name.clone()) {
            self.search.push(' ');
            self.search.push_str(&word.to_lowercase());
        }
        self.kind = kind;
        self.author_name = author_name;
    }

    /// "Category · sub" for the screen.
    pub fn place(&self) -> String {
        match &self.sub {
            Some(s) => format!("{} · {}", self.category, s),
            None => self.category.clone(),
        }
    }
}

/// A preset name's leading initials (2 to 4 capitals, then a space) and the rest.
pub fn split_initials(file: &str) -> (Option<&str>, &str) {
    if let Some((first, rest)) = file.split_once(' ') {
        let n = first.chars().count();
        if (2..=4).contains(&n) && first.chars().all(|c| c.is_ascii_uppercase()) && !rest.trim().is_empty() {
            return (Some(first), rest.trim_start());
        }
    }
    (None, file)
}

/// A folder name as a category: a leading sort number goes, an all-capitals name gets title case ("1 BASS" -> "Bass",
/// "THIRD PARTY" -> "Third Party"); other names stay ("MIDI Programs").
pub fn category_name(folder: &str) -> String {
    let s = match folder.split_once(' ') {
        Some((n, rest)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => rest,
        _ => folder,
    };
    if s.chars().any(char::is_lowercase) {
        return s.to_string();
    }
    s.split(' ')
        .map(|w| {
            let mut cs = w.chars();
            match cs.next() {
                Some(first) => first.to_uppercase().chain(cs.flat_map(char::to_lowercase)).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The kind of sound a preset is, from the first of its categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Lead,
    Bass,
    Keys,
    Pad,
    Fx,
    Drums,
    Seq,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Lead, Kind::Bass, Kind::Keys, Kind::Pad, Kind::Fx, Kind::Drums, Kind::Seq];

    /// Diva's top category name ("Leads", "Seq+Arp").
    pub fn of_category(c: &str) -> Option<Kind> {
        Some(match c {
            "Leads" => Kind::Lead,
            "Bass" => Kind::Bass,
            "Keys" => Kind::Keys,
            "Pads" => Kind::Pad,
            "FX" => Kind::Fx,
            "Drums" => Kind::Drums,
            "Seq+Arp" => Kind::Seq,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Lead => "Lead",
            Kind::Bass => "Bass",
            Kind::Keys => "Keys",
            Kind::Pad => "Pad",
            Kind::Fx => "FX",
            Kind::Drums => "Drums",
            Kind::Seq => "Sequence",
        }
    }
}

/// A filter chip.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Chip {
    #[default]
    All,
    Favorites,
    Category(String),
    /// A bank (`banks`) or a group of `groups.tsv`, by name.
    Group(String),
    Kind(Kind),
}

#[derive(Clone, Debug, Default)]
pub struct Library {
    pub presets: Vec<Preset>,
    /// Categories in the sources' order (one chip each).
    pub categories: Vec<String>,
    /// Groups read from a `groups.tsv` (`load_groups`): name and presets (indices), in the file's order.
    pub groups: Vec<(String, Vec<usize>)>,
    /// Banks (built in, or folders of the user's): name and presets, in the bank's order. Their chips come first.
    pub banks: Vec<(String, Vec<usize>)>,
}

/// Scanned libraries by their sources (`Library::scan_cached`).
fn cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Library>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Library>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The sources as a cache key: their folders and what they take.
fn cache_key(sources: &[Source]) -> String {
    sources.iter().map(|s| format!("{}|{}|{}|{}\n", s.root.display(), s.exts.join(","), s.name, s.single)).collect()
}

impl Library {
    /// `scan`, kept for the process: an editor opening again takes the copy scanned before at once (scanning ~1,400
    /// presets took ~70 ms of each opening) and scans again on a thread of its own for the opening after, so presets
    /// added meanwhile still come in.
    pub fn scan_cached(sources: &[Source]) -> Library {
        let key = cache_key(sources);
        let kept = cache().lock().ok().and_then(|c| c.get(&key).cloned());
        match kept {
            Some(lib) => {
                Library::warm(sources);
                lib
            }
            None => {
                let lib = Library::scan(sources);
                if let Ok(mut c) = cache().lock() {
                    c.insert(key, lib.clone());
                }
                lib
            }
        }
    }

    /// Scans `sources` on a thread of its own into the cache `scan_cached` takes from (a new instance warms it before
    /// its editor opens).
    pub fn warm(sources: &[Source]) {
        let sources = sources.to_vec();
        std::thread::spawn(move || {
            let lib = Library::scan(&sources);
            if let Ok(mut c) = cache().lock() {
                c.insert(cache_key(&sources), lib);
            }
        });
    }

    /// `warm`, when nothing is kept for `sources` yet.
    pub fn prewarm(sources: &[Source]) {
        let kept = cache().lock().is_ok_and(|c| c.contains_key(&cache_key(sources)));
        if !kept {
            Library::warm(sources);
        }
    }

    pub fn scan(sources: &[Source]) -> Library {
        let mut lib = Library::default();
        for s in sources {
            if s.banks {
                lib.add_banks(s);
            } else {
                lib.add_source(s);
            }
        }
        // A bank's copy of a preset found elsewhere is listed in its bank only.
        let others: std::collections::HashSet<String> =
            lib.presets.iter().enumerate().filter(|(i, _)| !lib.banks.iter().any(|b| b.1.contains(i))).map(|(_, p)| p.file.to_lowercase()).collect();
        let in_banks: Vec<usize> = lib.banks.iter().flat_map(|b| b.1.iter().copied()).collect();
        for i in in_banks {
            let shadowed = others.contains(&lib.presets[i].file.to_lowercase());
            lib.presets[i].shadowed = shadowed;
        }
        lib
    }

    /// The banks: those built in, then the folders in `s.root`. A folder named as a built-in bank adds to it.
    fn add_banks(&mut self, s: &Source) {
        if s.built_in {
            for e in zest_preset::EMBEDDED {
                self.add_bank_preset(zest_preset::unnumbered(e.folder), zest_preset::embedded_path(e), e.text);
            }
        }
        let sorted = |dir: &Path| -> Vec<PathBuf> {
            let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            v.sort();
            v
        };
        for dir in sorted(&s.root).into_iter().filter(|p| p.is_dir()) {
            let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            for f in sorted(&dir) {
                let ext = f.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                if s.exts.iter().any(|x| *x == ext) {
                    if let Ok(text) = zest_preset::read(&f) {
                        self.add_bank_preset(zest_preset::unnumbered(&folder), f, &text);
                    }
                }
            }
        }
    }

    fn add_bank_preset(&mut self, bank: &str, path: PathBuf, text: &str) {
        let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut p = Preset::new(path, bank.to_string(), None, false);
        let (author, name) = split_initials(zest_preset::unnumbered(&stem));
        (p.author, p.name, p.file) = (author.map(str::to_string), name.to_string(), zest_preset::unnumbered(&stem).to_string());
        p.read_zest_meta(text);
        if !self.categories.iter().any(|c| c == bank) {
            self.categories.push(bank.to_string());
        }
        let i = self.presets.len();
        self.presets.push(p);
        match self.banks.iter_mut().find(|b| b.0 == bank) {
            Some(b) => b.1.push(i),
            None => self.banks.push((bank.to_string(), vec![i])),
        }
    }

    fn add_source(&mut self, s: &Source) {
        // (sort key, preset)
        let mut found: Vec<(String, Preset)> = Vec::new();
        let mut stack = vec![(s.root.clone(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    if depth < 8 {
                        stack.push((p, depth + 1));
                    }
                    continue;
                }
                let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                if !s.exts.iter().any(|x| *x == ext) {
                    continue;
                }
                let rel = p.strip_prefix(&s.root).unwrap_or(&p).to_path_buf();
                let folders: Vec<String> = rel.parent().map(|r| r.iter().map(|c| c.to_string_lossy().into_owned()).collect()).unwrap_or_default();
                let (category, sub, key) = if s.single {
                    (s.name.clone(), folders.first().cloned(), rel.to_string_lossy().into_owned())
                } else {
                    match folders.first() {
                        // files directly in the root first, then the folders in their (numbered) order
                        None => (s.name.clone(), None, format!("0/{}", rel.to_string_lossy())),
                        Some(top) => (category_name(top), folders.get(1).cloned(), format!("1/{}", rel.to_string_lossy())),
                    }
                };
                let mut preset = Preset::new(p.clone(), category, sub, s.user);
                if ext == "h2p" {
                    preset.read_meta();
                } else if let Ok(text) = zest_preset::read(&p) {
                    preset.read_zest_meta(&text);
                }
                found.push((key.to_lowercase(), preset));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, p) in found {
            if !self.categories.contains(&p.category) {
                self.categories.push(p.category.clone());
            }
            self.presets.push(p);
        }
        // Copies without metadata (the Diva folder's root holds some of the folders' presets again) take the kind and
        // author of a copy that has it.
        let known: std::collections::HashMap<String, (Option<Kind>, Option<String>)> =
            self.presets.iter().filter(|p| p.kind.is_some()).map(|p| (p.file.to_lowercase(), (p.kind, p.author_name.clone()))).collect();
        for p in self.presets.iter_mut().filter(|p| p.kind.is_none()) {
            if let Some((k, a)) = known.get(&p.file.to_lowercase()) {
                let author = p.author_name.take().or_else(|| a.clone());
                p.describe(*k, author);
            }
        }
    }

    /// The kinds of sound the library has, in `Kind::ALL`'s order.
    pub fn kinds(&self) -> Vec<Kind> {
        Kind::ALL.into_iter().filter(|k| self.presets.iter().any(|p| p.kind == Some(*k))).collect()
    }

    /// Reads groups of presets from `file` (`groups.tsv` beside the favourites, written by tools/explore.py): one
    /// line per preset, `group<TAB>path`, `#` lines skipped. Each group gets a chip and lists its presets in the
    /// file's order; paths the library does not hold are skipped. No file: no groups.
    pub fn load_groups(&mut self, file: &Path) {
        self.groups.clear();
        let Ok(text) = std::fs::read_to_string(file) else { return };
        // One index of the paths, not a search of every preset per line (that took ~40 ms of each editor opening).
        let index = self.index();
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let Some((name, path)) = line.split_once('\t') else { continue };
            let Some(&i) = index.get(Path::new(path.trim())) else { continue };
            match groups.iter_mut().find(|g| g.0 == name) {
                Some(g) => g.1.push(i),
                None => groups.push((name.to_string(), vec![i])),
            }
        }
        self.groups = groups;
    }

    /// Each preset's index by its path (the first, if two share one, as `find`).
    fn index(&self) -> std::collections::HashMap<&Path, usize> {
        let mut m = std::collections::HashMap::with_capacity(self.presets.len());
        for (i, p) in self.presets.iter().enumerate() {
            m.entry(p.path.as_path()).or_insert(i);
        }
        m
    }

    pub fn find(&self, path: &Path) -> Option<usize> {
        self.presets.iter().position(|p| p.path == path)
    }

    /// The presets matching `query` (every word found in the name, category or folder, any case) and the chip, in
    /// library order.
    pub fn filter(&self, query: &str, chip: &Chip, favs: &Favorites) -> Vec<usize> {
        let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
        if let Chip::Group(g) = chip {
            let members = self.banks.iter().chain(&self.groups).find(|x| x.0 == *g).map(|x| x.1.as_slice()).unwrap_or_default();
            return members.iter().copied().filter(|&i| words.iter().all(|w| self.presets[i].search.contains(w.as_str()))).collect();
        }
        self.presets
            .iter()
            .enumerate()
            .filter(|(_, p)| match chip {
                Chip::All => !p.shadowed,
                Chip::Favorites => favs.contains(&p.path),
                Chip::Category(c) => p.category == *c,
                Chip::Kind(k) => p.kind == Some(*k) && !p.shadowed,
                Chip::Group(_) => false,
            })
            .filter(|(_, p)| words.iter().all(|w| p.search.contains(w.as_str())))
            .map(|(i, _)| i)
            .collect()
    }
}

/// Favourite presets: `favorites.txt` in the user folder, one preset path per line (they survive reinstalls).
#[derive(Clone, Debug, Default)]
pub struct Favorites {
    pub file: PathBuf,
    pub paths: Vec<PathBuf>,
}

impl Favorites {
    pub fn default_file() -> PathBuf {
        user_dir().join("favorites.txt")
    }

    pub fn load(file: PathBuf) -> Favorites {
        let paths = std::fs::read_to_string(&file)
            .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect())
            .unwrap_or_default();
        Favorites { file, paths }
    }

    pub fn contains(&self, p: &Path) -> bool {
        self.paths.iter().any(|q| q == p)
    }

    /// Stars or un-stars `p` and writes the file. Returns whether it is a favourite now.
    pub fn toggle(&mut self, p: &Path) -> std::io::Result<bool> {
        let on = if let Some(i) = self.paths.iter().position(|q| q == p) {
            self.paths.remove(i);
            false
        } else {
            self.paths.push(p.to_path_buf());
            true
        };
        self.save()?;
        Ok(on)
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(d) = self.file.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut s = String::new();
        for p in &self.paths {
            s.push_str(&p.to_string_lossy());
            s.push('\n');
        }
        std::fs::write(&self.file, s)
    }
}

/// A file name for a preset name typed in: path separators and control characters go, blank names give `None`.
pub fn preset_file_name(name: &str) -> Option<String> {
    let s: String = name.chars().filter(|c| !matches!(c, '/' | '\\' | '\0') && !c.is_control()).collect();
    let s = s.trim().trim_start_matches('.').trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zest-ui-test-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The kind and the author's name come from a preset's metadata; a copy without it takes them from one with it.
    #[test]
    fn kinds_and_authors_from_metadata() {
        let root = temp("meta");
        let meta = "/*@Meta\n\nAuthor:\n'Howard Scarr'\n\nCategories:\n'Bass:Analogue, Leads:Synth'\n\n*/\n#AM=Diva\n";
        std::fs::create_dir_all(root.join("diva/1 BASS")).unwrap();
        std::fs::write(root.join("diva/1 BASS/HS Bass Nine.h2p"), meta).unwrap();
        // the root's copy: an author, no categories
        std::fs::write(root.join("diva/HS Bass Nine.h2p"), "/*@Meta\n\nAuthor:\n'Howard Scarr'\n\n*/\n").unwrap();
        touch(&root.join("diva/HS Plain.h2p"));
        let lib = Library::scan(&sources(&root));
        let by = |f: &str, folder: &str| lib.presets.iter().find(|p| p.file == f && p.category == folder).unwrap();
        let tagged = by("HS Bass Nine", "Bass");
        assert_eq!((tagged.kind, tagged.author_name.as_deref()), (Some(Kind::Bass), Some("Howard Scarr")), "the first category");
        let copy = by("HS Bass Nine", "Diva");
        assert_eq!((copy.kind, copy.author_name.as_deref()), (Some(Kind::Bass), Some("Howard Scarr")), "the copy in the root");
        assert_eq!(by("HS Plain", "Diva").kind, None);
        assert_eq!(lib.kinds(), [Kind::Bass]);
        let favs = Favorites::default();
        assert_eq!(lib.filter("", &Chip::Kind(Kind::Bass), &favs).len(), 2);
        assert_eq!(lib.filter("scarr", &Chip::All, &favs).len(), 2, "the author's name is searched");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// How long scanning the real libraries takes (`cargo test -p zest-ui --release scan_time -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn scan_time() {
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let lib = Library::scan(&default_sources());
            println!("{} presets in {:?}", lib.presets.len(), t.elapsed());
        }
    }

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    fn sources(root: &Path) -> Vec<Source> {
        vec![
            Source { root: root.join("user"), exts: vec!["zest".into()], name: "User".into(), single: true, user: true, ..Default::default() },
            Source { root: root.join("diva"), exts: vec!["h2p".into()], name: "Diva".into(), ..Default::default() },
        ]
    }

    #[test]
    fn names_categories_and_initials() {
        assert_eq!(split_initials("HS Bass Nine"), (Some("HS"), "Bass Nine"));
        assert_eq!(split_initials("ROY FX Cold December"), (Some("ROY"), "FX Cold December"));
        assert_eq!(split_initials("Bass Nine"), (None, "Bass Nine"));
        assert_eq!(split_initials("ABCDE Too Long"), (None, "ABCDE Too Long"));
        assert_eq!(split_initials("HS"), (None, "HS"));
        assert_eq!(category_name("1 BASS"), "Bass");
        assert_eq!(category_name("3 POLY SYNTH"), "Poly Synth");
        assert_eq!(category_name("THIRD PARTY"), "Third Party");
        assert_eq!(category_name("MIDI Programs"), "MIDI Programs");
        assert_eq!(preset_file_name("  my/pad\\ 2 "), Some("mypad 2".into()));
        assert_eq!(preset_file_name(" .. "), None);
    }

    #[test]
    fn scan_filter_and_search() {
        let root = temp("scan");
        touch(&root.join("user/My Pad.zest"));
        touch(&root.join("user/Keys/Soft EP.zest"));
        touch(&root.join("user/notes.txt"));
        touch(&root.join("diva/HS Bass Nine.h2p"));
        touch(&root.join("diva/2 LEAD/MK Joy.h2p"));
        touch(&root.join("diva/1 BASS/HS Deep Bass.h2p"));
        touch(&root.join("diva/1 BASS/readme.pdf"));
        touch(&root.join("diva/THIRD PARTY/Basari/BAS Warm Pad.h2p"));
        let lib = Library::scan(&sources(&root));
        let files: Vec<&str> = lib.presets.iter().map(|p| p.file.as_str()).collect();
        assert_eq!(files, ["Soft EP", "My Pad", "HS Bass Nine", "HS Deep Bass", "MK Joy", "BAS Warm Pad"]);
        assert_eq!(lib.categories, ["User", "Diva", "Bass", "Lead", "Third Party"]);
        let warm = &lib.presets[5];
        assert_eq!((warm.category.as_str(), warm.sub.as_deref(), warm.author.as_deref(), warm.name.as_str()), ("Third Party", Some("Basari"), Some("BAS"), "Warm Pad"));
        assert_eq!(warm.place(), "Third Party · Basari");
        assert_eq!(lib.presets[0].sub.as_deref(), Some("Keys"));
        assert!(lib.presets[0].user && !lib.presets[2].user);

        let favs = Favorites::default();
        assert_eq!(lib.filter("", &Chip::All, &favs).len(), 6);
        assert_eq!(lib.filter("bass", &Chip::All, &favs), vec![2, 3], "names and categories");
        assert_eq!(lib.filter("", &Chip::Category("Bass".into()), &favs), vec![3]);
        assert_eq!(lib.filter("hs deep", &Chip::All, &favs), vec![3], "every word, initials too");
        assert_eq!(lib.filter("basari", &Chip::All, &favs), vec![5], "folder names");
        assert_eq!(lib.filter("PAD", &Chip::Category("User".into()), &favs), vec![1]);
        assert!(lib.filter("nothing", &Chip::All, &favs).is_empty());
        assert_eq!(lib.find(&root.join("diva/2 LEAD/MK Joy.h2p")), Some(4));

        let mut lib = lib;
        let d = root.join("diva");
        let g = format!("# comment\nNear\t{}\nFar\t{}\nNear\t{}\nNear\t{}\n", d.join("2 LEAD/MK Joy.h2p").display(), d.join("HS Bass Nine.h2p").display(), d.join("1 BASS/HS Deep Bass.h2p").display(), d.join("gone.h2p").display());
        std::fs::write(root.join("groups.tsv"), g).unwrap();
        lib.load_groups(&root.join("groups.tsv"));
        assert_eq!(lib.groups, [("Near".to_string(), vec![4, 3]), ("Far".to_string(), vec![2])]);
        assert_eq!(lib.filter("", &Chip::Group("Near".into()), &favs), vec![4, 3], "the file's order");
        assert_eq!(lib.filter("deep", &Chip::Group("Near".into()), &favs), vec![3]);
        lib.load_groups(&root.join("missing.tsv"));
        assert!(lib.groups.is_empty());
        // A folder of banks: in the order of their numbers, numbers not shown. A copy of a preset found elsewhere is
        // listed in its bank only.
        let banks = root.join("banks");
        std::fs::create_dir_all(banks.join("1 Space")).unwrap();
        std::fs::create_dir_all(banks.join("2 Other")).unwrap();
        std::fs::write(banks.join("1 Space/02 MK Joy.zest"), "#zest 1\nauthor: Mark\ncategories: Leads:Synth\n---\n").unwrap();
        std::fs::write(banks.join("1 Space/01 ZZ Own.zest"), "#zest 1\nname: ZZ Renamed\n---\n").unwrap();
        std::fs::write(banks.join("2 Other/Plain.zest"), "#zest 1\n---\n").unwrap();
        let sources = [
            Source { root: root.join("diva"), exts: vec!["h2p".into()], name: "Diva".into(), ..Default::default() },
            Source { root: banks, exts: vec!["zest".into()], banks: true, ..Default::default() },
        ];
        let lib = Library::scan(&sources);
        let of = |b: usize| lib.banks[b].1.iter().map(|&i| lib.presets[i].name.as_str()).collect::<Vec<_>>();
        assert_eq!(lib.banks.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(), ["Space", "Other"]);
        assert_eq!(of(0), ["Renamed", "Joy"]);
        assert_eq!(of(1), ["Plain"]);
        let joy = lib.banks[0].1[1];
        assert_eq!((lib.presets[joy].kind, lib.presets[joy].author_name.as_deref()), (Some(Kind::Lead), Some("Mark")));
        let favs = Favorites::default();
        assert!(lib.presets[joy].shadowed);
        assert!(!lib.filter("", &Chip::All, &favs).contains(&joy));
        assert!(lib.filter("", &Chip::Group("Space".into()), &favs).contains(&joy));
        assert!(lib.filter("", &Chip::All, &favs).contains(&lib.banks[1].1[0]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn favourites_round_trip() {
        let root = temp("favs");
        let file = root.join("zest/favorites.txt");
        let mut f = Favorites::load(file.clone());
        assert!(f.paths.is_empty(), "no file yet");
        let a = PathBuf::from("/presets/1 BASS/HS Deep Bass.h2p");
        let b = PathBuf::from("/presets/My Pad.zest");
        assert!(f.toggle(&a).unwrap());
        assert!(f.toggle(&b).unwrap());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "/presets/1 BASS/HS Deep Bass.h2p\n/presets/My Pad.zest\n");
        let g = Favorites::load(file.clone());
        assert!(g.contains(&a) && g.contains(&b));
        let mut g = g;
        assert!(!g.toggle(&a).unwrap());
        let h = Favorites::load(file);
        assert_eq!(h.paths, vec![b.clone()]);

        // The favourites chip.
        touch(&root.join("diva/1 BASS/HS Deep Bass.h2p"));
        touch(&root.join("diva/1 BASS/HS Other.h2p"));
        let lib = Library::scan(&sources(&root));
        let mut favs = Favorites::load(root.join("f.txt"));
        favs.toggle(&root.join("diva/1 BASS/HS Other.h2p")).unwrap();
        assert_eq!(lib.filter("", &Chip::Favorites, &favs), vec![1]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
