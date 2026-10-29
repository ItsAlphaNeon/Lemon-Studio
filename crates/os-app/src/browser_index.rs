// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The browser's index of its disk folders (spec: ui/browser.md §11): every entry below the dropped folders and the
//! project's data folder, read on a worker thread, so that a search finds files in folders nobody opened.
// spec: ui/browser.md §5, §11

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Receiver;

/// Entries one index holds at most; deeper or larger trees are cut off there (spec §11).
pub const MAX_ENTRIES: usize = 200_000;

/// One entry below an indexed folder: its path, its label as the tree shows it (lower case), whether it is a folder,
/// and which root it is under.
#[derive(Debug, Clone)]
pub struct IEntry {
    pub path: PathBuf,
    pub label: String,
    pub dir: bool,
    pub root: usize,
}

/// A finished index: the roots it was read from and what is below them.
#[derive(Debug, Default)]
pub struct Built {
    pub roots: Vec<PathBuf>,
    pub entries: Vec<IEntry>,
    /// Whether `MAX_ENTRIES` cut the walk short.
    pub cut: bool,
}

/// The query and "Search for folders" a set of folders holding a match was found for (`Index::match_dirs`).
type Found = (String, bool, Rc<HashSet<PathBuf>>);

/// The finished index, a build running on a worker thread, and the folders that hold a match of the last query (for
/// the finished index: dropped whenever it changes).
#[derive(Default)]
pub struct Index {
    done: Option<Built>,
    job: Option<(Vec<PathBuf>, Receiver<Built>)>,
    cache: RefCell<Option<Found>>,
}

impl Index {
    /// Starts reading `roots` on a worker thread (tests: at once); a build already running is dropped.
    pub fn start(&mut self, roots: Vec<PathBuf>) {
        if cfg!(test) {
            self.done = Some(build(&roots));
            self.job = None;
            *self.cache.borrow_mut() = None;
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let r = roots.clone();
        let spawned = std::thread::Builder::new().name("browser-index".into()).spawn(move || {
            let _ = tx.send(build(&r));
        });
        self.job = spawned.ok().map(|_| (roots, rx));
    }

    /// Takes a finished build; true when one finished now.
    pub fn poll(&mut self) -> bool {
        let Some((_, rx)) = &self.job else { return false };
        match rx.try_recv() {
            Ok(b) => {
                self.done = Some(b);
                self.job = None;
                *self.cache.borrow_mut() = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                false
            }
        }
    }

    pub fn building(&self) -> bool {
        self.job.is_some()
    }

    /// Whether the index (finished or being built) covers exactly `roots`.
    pub fn covers(&self, roots: &[PathBuf]) -> bool {
        match (&self.job, &self.done) {
            (Some((r, _)), _) => r == roots,
            (None, Some(b)) => b.roots == roots,
            (None, None) => false,
        }
    }

    /// Forgets the index (Reread structure): the next build reads the folders again.
    pub fn clear(&mut self) {
        self.done = None;
        self.job = None;
        *self.cache.borrow_mut() = None;
    }

    /// Number of entries in the finished index.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.done.as_ref().map_or(0, |b| b.entries.len())
    }

    /// Whether the finished index stopped at `MAX_ENTRIES`.
    pub fn cut(&self) -> bool {
        self.done.as_ref().is_some_and(|b| b.cut)
    }

    /// The folders that hold, somewhere below them, an entry whose label contains `q` (lower case): the folders a
    /// search opens. `folders`: folders match too ("Search for folders"). `None` without a finished index.
    pub fn match_dirs(&self, q: &str, folders: bool) -> Option<Rc<HashSet<PathBuf>>> {
        let b = self.done.as_ref()?;
        if let Some((_, _, set)) = self.cache.borrow().as_ref().filter(|(cq, cf, _)| cq == q && *cf == folders) {
            return Some(set.clone());
        }
        let mut set = HashSet::new();
        for e in b.entries.iter().filter(|e| (folders || !e.dir) && e.label.contains(q)) {
            let root = &b.roots[e.root];
            let mut p = e.path.parent();
            while let Some(d) = p {
                if !set.insert(d.to_path_buf()) || d == root {
                    break;
                }
                p = d.parent();
            }
        }
        let set = Rc::new(set);
        *self.cache.borrow_mut() = Some((q.to_string(), folders, set.clone()));
        Some(set)
    }
}

/// How a disk entry is labelled in the tree (spec §6): audio files and presets without their extension, other files
/// and folders by their whole name.
pub fn disk_label(name: &str, path: &Path, dir: bool) -> String {
    let stem = || name.rsplit_once('.').map_or(name, |(a, _)| a).to_string();
    if !dir && (crate::views::browser::file_icon(name) == crate::icons::Icon::FileAudio || crate::presets::is_preset_file(path)) {
        stem()
    } else {
        name.to_string()
    }
}

/// Whether a directory entry is listed in the browser: hidden and system entries, dot files, `desktop.ini` and
/// `Thumbs.db` are not (spec §6).
pub fn listed(name: &str, md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM
        if md.file_attributes() & 0x6 != 0 {
            return false;
        }
    }
    #[cfg(not(windows))]
    let _ = md;
    let low = name.to_ascii_lowercase();
    !(name.starts_with('.') || low == "desktop.ini" || low == "thumbs.db")
}

/// Reads everything below `roots`, breadth first, skipping what the browser does not list and links to folders (no
/// loops); stops at `MAX_ENTRIES`.
pub fn build(roots: &[PathBuf]) -> Built {
    let mut entries = Vec::new();
    let mut cut = false;
    'roots: for (root, r) in roots.iter().enumerate() {
        let mut queue = std::collections::VecDeque::from([r.clone()]);
        while let Some(dir) = queue.pop_front() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Ok(md) = e.metadata() else { continue };
                if !listed(&name, &md) {
                    continue;
                }
                let link = e.file_type().is_ok_and(|t| t.is_symlink());
                let path = e.path();
                let is_dir = md.is_dir();
                if is_dir && !link {
                    queue.push_back(path.clone());
                }
                entries.push(IEntry { label: disk_label(&name, &path, is_dir).to_lowercase(), path, dir: is_dir, root });
                if entries.len() >= MAX_ENTRIES {
                    cut = true;
                    break 'roots;
                }
            }
        }
    }
    Built { roots: roots.to_vec(), entries, cut }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index reads every level, labels audio files without their extension, and finds the folders that hold a
    /// match (spec §11).
    #[test]
    fn finds_the_folders_holding_a_match() {
        let root = std::env::temp_dir().join(format!("os_index_{}", std::process::id()));
        let deep = root.join("Kits").join("808");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(root.join("Loops")).unwrap();
        std::fs::write(deep.join("Big Kick.wav"), b"").unwrap();
        std::fs::write(root.join("Loops").join("notes.txt"), b"").unwrap();
        std::fs::write(root.join(".hidden.wav"), b"").unwrap();
        let mut ix = Index::default();
        ix.start(vec![root.clone()]);
        assert!(ix.covers(std::slice::from_ref(&root)) && !ix.building());
        assert_eq!(ix.len(), 5, "two folders, a folder inside, two files; the dot file left out");
        let set = ix.match_dirs("kick", true).unwrap();
        assert_eq!(*set, HashSet::from([root.clone(), root.join("Kits"), deep.clone()]));
        assert!(ix.match_dirs("big kick.wav", true).unwrap().is_empty(), "audio labels have no extension");
        assert!(ix.match_dirs("notes.txt", true).unwrap().contains(&root.join("Loops")));
        // A folder's name matches only with "Search for folders".
        assert!(ix.match_dirs("808", true).unwrap().contains(&root.join("Kits")));
        assert!(ix.match_dirs("808", false).unwrap().is_empty());
        ix.clear();
        assert!(ix.match_dirs("kick", true).is_none() && !ix.covers(std::slice::from_ref(&root)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
