// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::loading` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use os_engine::Sample;
use os_ui::{kit, Canvas, Font, Rect};
#[allow(unused_imports)]
use crate::loading::*;

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

/// The progress panel, centred in the window (Lemon Studio design, spec: ui/project-loading.md §3).
pub fn panel_rect(screen: (f32, f32)) -> Rect {
    let (w, h) = (420.0, 92.0);
    Rect::new(((screen.0 - w) / 2.0).round(), ((screen.1 - h) / 2.0).round(), w, h)
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
        assert_eq!((workers(&crate::settings_file::store(), 0), workers(&crate::settings_file::store(), 1), workers(&crate::settings_file::store(), 10)), (0, 1, 1));
        set_check(&THREADED_SAMPLE_LOADING, true);
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
        assert_eq!((workers(&crate::settings_file::store(), 0), workers(&crate::settings_file::store(), 1), workers(&crate::settings_file::store(), 10)), (0, 1, cores));
        // A project of six sampler channels, each with a file of its own (missing: the workers report them failed).
        let mut p = crate::projects::default_project(&crate::settings_file::store());
        for i in 0..6 {
            let r = crate::projects::add_channel(&mut p);
            p.channels[r].sample_path = Some(format!("Z:/no such folder/loading-{i}.wav"));
        }
        assert_eq!(Loading::start(&crate::settings_file::store(), &p).workers, cores.min(6), "on");
        set_check(&THREADED_SAMPLE_LOADING, false);
        let mut l = Loading::start(&crate::settings_file::store(), &p);
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
