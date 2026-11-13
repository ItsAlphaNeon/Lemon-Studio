// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! An opened project's missing files. Samples whose stored path is not found are looked for by name in the project's
//! own folders (a project unzipped or moved from another computer keeps its samples beside it). Once the project has
//! loaded, a box lists the samples and plugins still missing; Find samples... looks for the samples by name in a folder
//! the user picks and its subfolders, on a thread of its own, behind a progress panel.
//!
// spec: flp/sample-paths.md "Missing samples", ui/project-loading.md §5-6

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use crate::dialog::{spans, Answer, Kind, Msg, Then};

/// The most names the box lists per kind before "and N more".
const LISTED: usize = 8;
/// A search stops after this long, or after this many folders, with what it found by then.
pub(crate) const SEARCH_TIME: Duration = Duration::from_secs(60);
const SEARCH_FOLDERS: usize = 100_000;
/// Folders deeper than this under the picked one are not searched.
const SEARCH_DEPTH: usize = 16;

/// What the missing files box knows about the open project.
#[derive(Default)]
pub struct Missing {
    /// Folders picked with Find samples... (where missing samples were found), searched after the project's own.
    pub picked: Vec<PathBuf>,
    /// The project was opened and the box has not been considered yet: it is, once the project has loaded.
    pub pending: bool,
    /// A Find samples... search running.
    pub search: Option<Search>,
}

/// A running Find samples... search: the folders it found missing samples in come back once it is over.
pub struct Search {
    pub(crate) rx: Receiver<Vec<PathBuf>>,
    pub folder: PathBuf,
    pub started: Instant,
}

/// The file name of a stored sample path (either slash).
pub(crate) fn file_name(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

/// `names` sorted, each once, at most `LISTED` of them, then "and N more".
fn listed(mut names: Vec<String>) -> String {
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    let more = names.len().saturating_sub(LISTED);
    let mut s = names.into_iter().take(LISTED).collect::<Vec<_>>().join(", ");
    if more > 0 {
        s.push_str(&format!(" and {more} more"));
    }
    s
}

/// The box: what is missing, that the project keeps it, and Find samples... when samples are missing.
pub(crate) fn missing_box(samples: &[String], plugins: &[String]) -> Msg {
    let count = |v: &[String]| v.iter().collect::<HashSet<_>>().len();
    let mut parts: Vec<String> = vec!["Some files this project uses were not found. The project keeps them: saving it does not remove them.\n".into()];
    if !samples.is_empty() {
        parts.push(format!("\nSamples ({}): ", count(samples)));
        parts.push(listed(samples.to_vec()));
    }
    if !plugins.is_empty() {
        parts.push(format!("\nPlugins ({}): ", count(plugins)));
        parts.push(listed(plugins.to_vec()));
    }
    if !samples.is_empty() {
        parts.push("\n\nFind samples... looks for the missing samples by name in a folder you choose and its subfolders.".into());
    }
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let text = spans(&parts);
    if samples.is_empty() {
        return Msg::new(Kind::Warning, "Missing project files", text, &[Answer::Ok], Then::MissingFiles);
    }
    Msg::new(Kind::Warning, "Missing project files", text, &[Answer::Yes, Answer::Ok], Then::MissingFiles).labelled(&["Find samples...", "OK"])
}

/// The folders under `root` (itself included) that hold a file named as one of `wanted` (lower case), in the order
/// found, until every name is found, `deadline` passes or `SEARCH_FOLDERS` folders were listed.
pub fn find_in(root: &Path, mut wanted: HashSet<String>, deadline: Instant) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if !os_engine::sample::reachable(root) {
        return found;
    }
    let mut stack = vec![(root.to_path_buf(), 0)];
    let mut listed = 0;
    while let Some((dir, depth)) = stack.pop() {
        if wanted.is_empty() || Instant::now() >= deadline || listed >= SEARCH_FOLDERS {
            break;
        }
        listed += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut here = false;
        for e in entries.flatten() {
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if depth < SEARCH_DEPTH {
                    stack.push((e.path(), depth + 1));
                }
            } else if wanted.remove(&e.file_name().to_string_lossy().to_lowercase()) {
                here = true;
            }
        }
        if here {
            found.push(dir);
        }
    }
    found
}
