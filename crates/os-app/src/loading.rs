// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Project loading: an opened project's samples are decoded on worker threads, then its sampler channels get their
//! Lemon Sampler instances a few per frame, while a progress panel shows and the window takes no input. Opening a
//! sampler afterwards never waits for a file.
//!
// spec: ui/project-loading.md

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use os_engine::Sample;
use os_ui::{kit, Canvas, Font, Rect};

/// Time spent starting instances per UI tick, so the window keeps painting the progress.
pub const HOST_BUDGET: std::time::Duration = std::time::Duration::from_millis(12);

pub struct Loading {
    /// Distinct sample files of the project's sampler channels, and how many are decoded.
    files: usize,
    decoded: usize,
    /// Sampler channels, and how many have their instance.
    pub rows: usize,
    pub hosted: usize,
    /// What is happening now (file or channel name).
    pub label: String,
    rx: Receiver<(String, Option<Arc<Sample>>)>,
    /// The threads decoding (`workers`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub workers: usize,
    /// Decodes kept alive until the instances took them (`os_engine::sample::load_shared` hands out live ones).
    held: Vec<Arc<Sample>>,
    queue: Arc<Mutex<Vec<PathBuf>>>,
}

impl Loading {
    /// Starts decoding the sample files of `project`'s sampler channels on worker threads.
    pub fn start(project: &os_model::Project) -> Loading {
        let samplers: Vec<&os_model::Channel> = project.channels.iter().filter(|c| c.kind.is_sampler()).collect();
        // The paths Lemon Sampler opens (`plugins::sampler_state`), each once.
        let files: BTreeSet<String> = samplers
            .iter()
            .map(|c| crate::plugins::resolve_sample(c.sample_path.as_deref().unwrap_or("")))
            .filter(|p| !p.is_empty())
            .collect();
        let queue = Arc::new(Mutex::new(files.iter().rev().map(PathBuf::from).collect::<Vec<_>>()));
        let (tx, rx) = channel();
        let workers = workers(files.len());
        for _ in 0..workers {
            let (queue, tx) = (queue.clone(), tx.clone());
            std::thread::spawn(move || loop {
                let Some(path) = queue.lock().ok().and_then(|mut q| q.pop()) else { break };
                let s = os_engine::sample::load_shared(&path).ok();
                let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                if tx.send((name, s)).is_err() {
                    break;
                }
                os_clap::wake();
            });
        }
        Loading { files: files.len(), decoded: 0, rows: samplers.len(), hosted: 0, label: String::new(), rx, workers, held: Vec::new(), queue }
    }

    /// Takes finished decodes; true when every file is decoded (or failed: the instance reports that file itself).
    pub fn decode_done(&mut self) -> bool {
        while let Ok((name, s)) = self.rx.try_recv() {
            self.decoded += 1;
            self.label = name;
            self.held.extend(s);
        }
        self.decoded >= self.files
    }

    /// Progress 0..=1 over decoding and starting instances.
    pub fn progress(&self) -> f32 {
        let total = self.files + self.rows;
        if total == 0 {
            return 1.0;
        }
        (self.decoded + self.hosted) as f32 / total as f32
    }

    /// The panel's text: the step and the item it is on.
    pub fn status(&self) -> String {
        if self.decoded < self.files {
            format!("Loading samples  {} / {}", self.decoded, self.files)
        } else {
            format!("Starting samplers  {} / {}", self.hosted, self.rows)
        }
    }
}

/// Threads to decode `files` files on: General ▸ Advanced ▸ Threaded project sample loading on, up to 4 (as many as
/// the machine runs at once); off, one, a file at a time. Either way off the UI thread, behind the progress panel.
/// Spec: ui/settings-choices.md "Advanced: Fast sample preview, Auto keep long audio on disk, Threaded project sample
/// loading (2026-10-16)".
pub fn workers(files: usize) -> usize {
    let most = if crate::prefs::check(&crate::prefs::THREADED_SAMPLE_LOADING) {
        std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4)
    } else {
        1
    };
    most.min(files)
}

impl Drop for Loading {
    /// A project opened or closed meanwhile: the workers stop after their current file.
    fn drop(&mut self) {
        if let Ok(mut q) = self.queue.lock() {
            q.clear();
        }
    }
}

/// The progress panel, centred in the window (Lemon Studio design, spec: ui/project-loading.md §3).
pub fn panel_rect(screen: (f32, f32)) -> Rect {
    let (w, h) = (420.0, 92.0);
    Rect::new(((screen.0 - w) / 2.0).round(), ((screen.1 - h) / 2.0).round(), w, h)
}

pub fn paint(c: &mut Canvas, t: &os_theme::Palette, l: &Loading) {
    panel(c, t, "Loading project", &format!("{}   {}", l.status(), l.label), l.progress());
}

/// The progress panel with a title, a status line and a bar at `progress` (0..1); an export shows it too.
pub fn panel(c: &mut Canvas, t: &os_theme::Palette, title: &str, status: &str, progress: f32) {
    let r = panel_rect(c.size());
    kit::popup(c, t, r);
    let x = r.x + 16.0;
    c.text(Font::new(17), x, r.y + 10.0, t.text, title);
    c.text_clip(Font::new(14), x, r.y + 35.0, r.w - 32.0, t.text_dim, status);
    let track = Rect::new(x, r.y + 64.0, r.w - 32.0, 12.0);
    kit::sunken(c, t, track, t.sunken_dark);
    let w = ((track.w - 4.0) * progress.clamp(0.0, 1.0)).round();
    if w > 0.0 {
        c.fill_rect(Rect::new(track.x + 2.0, track.y + 2.0, w, track.h - 4.0), t.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::{check, set_check, THREADED_SAMPLE_LOADING};

    // spec: ui/settings-choices.md "Advanced: Fast sample preview, Auto keep long audio on disk, Threaded project
    // sample loading (2026-10-16)"
    /// Threaded project sample loading off (as found): one decoding thread, a file at a time; on, up to 4 (as many as
    /// the machine runs at once), never more than there are files. A project's loading reads it when it starts.
    #[test]
    fn decoding_threads_follow_threaded_loading() {
        assert!(!check(&THREADED_SAMPLE_LOADING), "off by default");
        assert_eq!((workers(0), workers(1), workers(10)), (0, 1, 1));
        set_check(&THREADED_SAMPLE_LOADING, true);
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
        assert_eq!((workers(0), workers(1), workers(10)), (0, 1, cores));
        // A project of six sampler channels, each with a file of its own (missing: the workers report them failed).
        let mut p = crate::app::default_project();
        for i in 0..6 {
            let r = crate::app::add_channel(&mut p);
            p.channels[r].sample_path = Some(format!("Z:/no such folder/loading-{i}.wav"));
        }
        assert_eq!(Loading::start(&p).workers, cores.min(6), "on");
        set_check(&THREADED_SAMPLE_LOADING, false);
        let mut l = Loading::start(&p);
        assert_eq!(l.workers, 1, "off");
        // Off the UI thread all the same: every file comes back from the one worker.
        let t = std::time::Instant::now();
        while !l.decode_done() {
            assert!(t.elapsed() < std::time::Duration::from_secs(10), "the worker finishes");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!((l.decoded, l.files), (6, 6));
    }
}
