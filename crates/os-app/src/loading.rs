// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Project loading: an opened project's samples are decoded on worker threads, then its sampler channels get their
//! Lemon Sampler instances a few per frame (their played samples are built on build threads), then its plugin
//! channels and mixer effects start a few per frame, while a progress panel shows and the window takes no input.
//! Opening a sampler afterwards never waits for a file.
//!
// spec: ui/project-loading.md

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use os_engine::Sample;
use os_ui::Rect;

/// Time spent starting instances per UI tick, so the window keeps painting the progress.
pub const HOST_BUDGET: std::time::Duration = std::time::Duration::from_millis(12);

pub struct Loading {
    /// Distinct sample files of the project's sampler channels, and how many are decoded.
    pub(crate) files: usize,
    pub(crate) decoded: usize,
    /// Sampler channels, and how many have their instance.
    pub rows: usize,
    pub hosted: usize,
    /// Plugin channels and filled mixer slots, and how many have started or failed (`App::loading_tick`).
    pub plugins: usize,
    pub plugins_done: usize,
    /// The samplers are all started: their played samples are being built (`lemon_sampler::pending_builds`), and the
    /// most that were waiting at once.
    pub builds: Option<usize>,
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
        let plugins = project.channels.iter().filter(|c| c.kind == os_model::ChannelKind::Plugin).count()
            + project.mixer.iter().map(|m| m.slots.iter().flatten().count()).sum::<usize>();
        lemon_sampler::set_deferred_builds(true);
        Loading {
            files: files.len(),
            decoded: 0,
            rows: samplers.len(),
            hosted: 0,
            plugins,
            plugins_done: 0,
            builds: None,
            label: String::new(),
            rx,
            workers,
            held: Vec::new(),
            queue,
        }
    }

    /// The played samples still being built, once every instance has started: true when there are none left.
    pub fn builds_done(&mut self) -> bool {
        let left = lemon_sampler::pending_builds();
        let most = self.builds.unwrap_or(0).max(left);
        self.builds = Some(most);
        left == 0
    }

    fn builds_left(&self) -> usize {
        self.builds.map_or(0, |_| lemon_sampler::pending_builds())
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

    /// Progress 0..=1 over decoding, starting instances, starting plugins and building the played samples.
    pub fn progress(&self) -> f32 {
        let built = self.builds.map_or(0, |most| most - self.builds_left());
        let total = self.files + self.rows + self.plugins + self.builds.unwrap_or(0);
        if total == 0 {
            return 1.0;
        }
        (self.decoded + self.hosted + self.plugins_done.min(self.plugins) + built) as f32 / total as f32
    }

    /// The panel's text: the step and the item it is on.
    pub fn status(&self) -> String {
        if self.decoded < self.files {
            format!("Loading samples  {} / {}", self.decoded, self.files)
        } else if self.hosted < self.rows {
            format!("Starting samplers  {} / {}", self.hosted, self.rows)
        } else if self.builds.is_none() {
            format!("Starting plugins  {}", self.plugins_done)
        } else {
            format!("Preparing samples  {} left", self.builds_left())
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
    /// A project opened or closed meanwhile: the workers stop after their current file. Samplers started from now on
    /// build their played sample as they start.
    fn drop(&mut self) {
        if let Ok(mut q) = self.queue.lock() {
            q.clear();
        }
        lemon_sampler::set_deferred_builds(false);
    }
}

/// The progress panel, centred in the window (Lemon Studio design, spec: ui/project-loading.md §3).
pub fn panel_rect(screen: (f32, f32)) -> Rect {
    let (w, h) = (420.0, 92.0);
    Rect::new(((screen.0 - w) / 2.0).round(), ((screen.1 - h) / 2.0).round(), w, h)
}
