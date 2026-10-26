// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! An opened project's missing files. Samples whose stored path is not found are looked for by name in the project's
//! own folders (a project unzipped or moved from another computer keeps its samples beside it). Once the project has
//! loaded, a box lists the samples and plugins still missing; Find samples... looks for the samples by name in a folder
//! the user picks and its subfolders, on a thread of its own, behind a progress panel.
//!
// spec: flp/sample-paths.md "Missing samples", ui/project-loading.md §5-6

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use crate::app::{App, State};
use crate::dialog::{spans, Answer, Kind, Msg, Then};

/// The most names the box lists per kind before "and N more".
const LISTED: usize = 8;
/// A search stops after this long, or after this many folders, with what it found by then.
const SEARCH_TIME: Duration = Duration::from_secs(60);
const SEARCH_FOLDERS: usize = 100_000;
/// Folders deeper than this under the picked one are not searched.
const SEARCH_DEPTH: usize = 16;

/// What the missing files box knows about the open project.
#[derive(Default)]
pub struct Missing {
    /// Folders picked with Find samples... (where missing samples were found), searched after the project's own.
    pub picked: Vec<PathBuf>,
    /// Plugins the project names that are not installed or did not start (`project_plugins::sync_wrapped`,
    /// `host_native`), each once.
    pub plugins: Vec<String>,
    /// The project was opened and the box has not been considered yet: it is, once the project has loaded.
    pub pending: bool,
    /// A Find samples... search running.
    pub search: Option<Search>,
}

impl Missing {
    /// Adds plugin names (each once; "Serum (error text)" counts as its own entry).
    pub fn add_plugins(&mut self, names: &[String]) {
        for n in names {
            if !self.plugins.contains(n) {
                self.plugins.push(n.clone());
            }
        }
    }
}

/// A running Find samples... search: the folders it found missing samples in come back once it is over.
pub struct Search {
    rx: Receiver<Vec<PathBuf>>,
    pub folder: PathBuf,
    pub started: Instant,
}

/// The folders a missing sample of the open project is looked for in by its name, in order: the project file's folder,
/// its data folder and that folder's Samples folder, then the folders picked in the missing files box (`picked`).
pub fn search_folders(s: &State, picked: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let project = s.path.as_deref().and_then(|p| p.parent()).filter(|d| !d.as_os_str().is_empty()).map(PathBuf::from);
    let data = crate::data_folder::folder(s);
    let samples = data.as_ref().map(|d| d.join(crate::data_folder::SAMPLES_DIR));
    for d in project.into_iter().chain(data).chain(samples).chain(picked.iter().cloned()) {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs
}

/// A project was opened: its samples are looked for in its own folders, and what the last project picked or missed is
/// forgotten.
pub fn opened(app: &mut App) {
    app.missing.picked.clear();
    app.missing.plugins.clear();
    app.missing.search = None;
    app.missing.pending = true;
    apply(app);
}

/// The search folders take the open project's folders and the picked ones.
pub fn apply(app: &App) {
    os_engine::sample::set_search_folders(search_folders(&app.state, &app.missing.picked));
}

/// The channels whose sample is not found (sampler and audio clip channels, and a 3x Osc's custom shape): row and the
/// path the project stores.
pub fn missing_samples(s: &State) -> Vec<(usize, String)> {
    s.project
        .channels
        .iter()
        .enumerate()
        .filter_map(|(row, ch)| Some((row, ch.sample_path.clone().filter(|p| !p.trim().is_empty())?)))
        .filter(|(_, p)| !Path::new(&crate::plugins::resolve_sample(p)).is_file())
        .collect()
}

/// The file name of a stored sample path (either slash).
fn file_name(p: &str) -> &str {
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

/// The project has loaded: the box lists its missing samples and plugins, once per opened project (none: no box).
pub fn loaded(app: &mut App) {
    if !std::mem::take(&mut app.missing.pending) {
        return;
    }
    let samples: Vec<String> = missing_samples(&app.state).iter().map(|(_, p)| file_name(p).to_string()).collect();
    let plugins = app.missing.plugins.clone();
    if samples.is_empty() && plugins.is_empty() {
        return;
    }
    app.show_msg(missing_box(&samples, &plugins));
}

/// The box: what is missing, that the project keeps it, and Find samples... when samples are missing.
fn missing_box(samples: &[String], plugins: &[String]) -> Msg {
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

/// Find samples...: a folder picker, starting in the project's folder.
pub fn pick_folder(app: &mut App) {
    use crate::filedlg::{FileDlg, Mode};
    let start = app.state.path.as_deref().and_then(Path::parent).filter(|d| d.is_dir()).map(PathBuf::from);
    let dlg = FileDlg::new(Mode::Folder, "Find missing samples", "missing samples", Then::SampleFolder).start(start.as_deref());
    app.file_dialog(dlg);
}

/// Looks for the missing samples by name in `folder` and its subfolders, on a thread of its own (`tick` takes the
/// result).
pub fn search(app: &mut App, folder: PathBuf) {
    let wanted: HashSet<String> = missing_samples(&app.state).iter().map(|(_, p)| file_name(p).to_lowercase()).collect();
    if wanted.is_empty() {
        return;
    }
    let (tx, rx) = channel();
    let root = folder.clone();
    std::thread::spawn(move || {
        let _ = tx.send(find_in(&root, wanted, Instant::now() + SEARCH_TIME));
    });
    app.missing.search = Some(Search { rx, folder, started: Instant::now() });
    app.invalidate();
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

/// Follows a running search: when it is over, the folders it found samples in are searched from now on, and the
/// channels whose sample is found there take it.
pub fn tick(app: &mut App) {
    let Some(s) = &app.missing.search else { return };
    let dirs = match s.rx.try_recv() {
        Ok(d) => d,
        Err(TryRecvError::Empty) => {
            app.damage_rect(crate::loading::panel_rect(app.state.screen).outset(4.0));
            return;
        }
        Err(TryRecvError::Disconnected) => Vec::new(),
    };
    let folder = app.missing.search.take().map(|s| s.folder).unwrap_or_default();
    let before = missing_samples(&app.state);
    for d in dirs {
        if !app.missing.picked.contains(&d) {
            app.missing.picked.push(d);
        }
    }
    apply(app);
    let found = relink(app, &before);
    let left = before.len() - found;
    app.state.message = Some(match (found, left) {
        (0, _) => format!("No missing samples found in {}", folder.display()),
        (n, 0) => format!("Found all {n} missing samples in {}", folder.display()),
        (n, l) => format!("Found {n} missing samples in {} ({l} still missing)", folder.display()),
    });
    app.invalidate();
}

/// The channels of `before` (missing samples) whose sample is found now take the file found: its path goes into the
/// project (a change to save), and a hosted Lemon Sampler loads it. Returns how many were found.
fn relink(app: &mut App, before: &[(usize, String)]) -> usize {
    let mut found = 0;
    for (row, old) in before {
        let now = crate::plugins::resolve_sample(old);
        if !Path::new(&now).is_file() {
            continue;
        }
        found += 1;
        let Some(ch) = app.state.project.channels.get_mut(*row) else { continue };
        ch.sample_path = Some(now.clone());
        if ch.kind.is_sampler() {
            app.state.sample_loads.push((*row, now));
        }
    }
    found
}

/// The search's progress panel: the folder searched and the time so far.
pub fn paint(c: &mut os_ui::Canvas, t: &os_theme::Palette, s: &Search) {
    let secs = s.started.elapsed().as_secs();
    crate::loading::panel(c, t, "Looking for missing samples", &format!("{}   {secs} s", s.folder.display()), (secs as f32 / SEARCH_TIME.as_secs_f32()).min(1.0));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 16-bit mono 44.1 kHz WAV file of `frames` silent frames.
    pub(crate) fn wav(frames: u32) -> Vec<u8> {
        let data = frames * 2;
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + data).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(44100u32.to_le_bytes());
        b.extend((44100u32 * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(data.to_le_bytes());
        b.resize(b.len() + data as usize, 0);
        b
    }

    /// A search finds the folders, at any depth, that hold the wanted names (case does not count), and stops looking
    /// once every name is found.
    #[test]
    fn search_finds_folders_by_name() {
        let root = std::env::temp_dir().join(format!("os_missing_search_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("a/b/c/Kick.WAV"), b"").unwrap();
        std::fs::write(root.join("d/snare.wav"), b"").unwrap();
        std::fs::write(root.join("d/other.wav"), b"").unwrap();
        let wanted: HashSet<String> = ["kick.wav", "snare.wav", "gone.wav"].iter().map(|s| s.to_string()).collect();
        let mut found = find_in(&root, wanted, Instant::now() + Duration::from_secs(10));
        found.sort();
        assert_eq!(found, vec![root.join("a/b/c"), root.join("d")]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The box lists each missing name once, sorted, and offers Find samples... only when samples are missing.
    #[test]
    fn the_box_lists_what_is_missing() {
        let s = vec!["b.wav".to_string(), "a.wav".into(), "b.wav".into()];
        let p = vec!["Serum".to_string()];
        let m = missing_box(&s, &p);
        assert!(m.plain().contains("Samples (2): a.wav, b.wav"), "{}", m.plain());
        assert!(m.plain().contains("Plugins (1): Serum"));
        assert_eq!(m.label(0), "Find samples...");
        let m = missing_box(&[], &p);
        assert_eq!(m.buttons, vec![Answer::Ok]);
        let many: Vec<String> = (0..12).map(|i| format!("s{i:02}.wav")).collect();
        assert!(missing_box(&many, &[]).plain().contains("s07.wav and 4 more"));
    }

    /// Opening a project from another computer whose samples lie beside it: the samples are found by name in the
    /// project's folder, nothing is changed in the project, and only what is truly missing is listed.
    #[test]
    fn samples_beside_the_project_are_found() {
        let dir = std::env::temp_dir().join(format!("os_missing_beside_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = wav(4410);
        std::fs::write(dir.join("Here.wav"), &wav).unwrap();
        let mut app = App::new(None);
        app.state.project = crate::app::default_project();
        let a = crate::app::add_channel(&mut app.state.project);
        let b = crate::app::add_channel(&mut app.state.project);
        app.state.project.channels[a].sample_path = Some(r"C:\Users\someone\AppData\Local\Temp\Image-Line\x\Here.wav".into());
        app.state.project.channels[b].sample_path = Some(r"C:\Users\someone\Downloads\Gone.wav".into());
        app.state.file = None;
        app.save_to(dir.join("Song.flp"));
        app.plugins = crate::plugins::Plugins::default();
        app.plugins.available = crate::plugins::builtin();
        app.open(dir.join("Song.flp"));
        let missing = missing_samples(&app.state);
        assert_eq!(missing.iter().map(|(r, _)| *r).collect::<Vec<_>>(), vec![b], "only the sample that is nowhere");
        assert_eq!(crate::plugins::resolve_sample(app.state.project.channels[a].sample_path.as_deref().unwrap()), dir.join("Here.wav").to_string_lossy());
        let t0 = Instant::now();
        while app.loading() && t0.elapsed() < Duration::from_secs(20) {
            app.plugin_tick();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!app.unsaved(), "finding a sample by name changes nothing");
        // The box lists Gone.wav; Find samples... in a folder that has it relinks the channel.
        assert!(app.state.dialogs.last().is_some_and(|d| matches!(&d.body, crate::dialog::Body::Msg(m) if m.plain().contains("Gone.wav"))));
        app.state.dialogs.clear();
        std::fs::create_dir_all(dir.join("elsewhere/deep")).unwrap();
        std::fs::write(dir.join("elsewhere/deep/Gone.wav"), &wav).unwrap();
        search(&mut app, dir.join("elsewhere"));
        let t0 = Instant::now();
        while app.missing.search.is_some() && t0.elapsed() < Duration::from_secs(20) {
            tick(&mut app);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(missing_samples(&app.state).is_empty(), "{:?}", app.state.message);
        assert_eq!(app.state.project.channels[b].sample_path.as_deref(), Some(dir.join("elsewhere").join("deep").join("Gone.wav").to_string_lossy().as_ref()));
        assert!(app.unsaved(), "a relinked sample is a change to save");
        os_engine::sample::set_search_folders(Vec::new());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
