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

/// Every sampler channel is played and edited by Lemon Sampler (owner, 2026-10-10; spec: plugins/lemon-sampler.md §7):
/// a channel without an instance gets one, started from the channel's sample and envelopes. Without Lemon Sampler
/// (plugin not found) sampler channels keep the engine's own sample playback.
/// With a `deadline` it stops after the instance that passes it; returns whether every sampler row is done.
pub(crate) fn sync_samplers(ses: &mut crate::session::Session, deadline: Option<std::time::Instant>) -> bool {
    let Some(index) = ses.plugins.index_of(crate::plugins::LEMON_SAMPLER) else { return true };
    let mut added = false;
    let mut done = true;
    for row in 0..ses.project.channels.len() {
        let ch = &ses.project.channels[row];
        if !ch.kind.is_sampler() || ses.hosted_rows.contains(&row) || ses.plugins.sampler_failed.contains(&row) {
            continue;
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            done = false;
            break;
        }
        let st = crate::plugins::sampler_state(ch);
        if let Some(l) = &mut ses.loading {
            l.hosted += 1;
            l.label = ch.name.clone().unwrap_or_default();
        }
        match ses.plugins.create_with(row, index, Some(&st)) {
            Ok(_) => {
                ses.hosted_rows.push(row);
                added = true;
                // A channel just added (ADD ▸ Lemon Sampler): its window is about to open (spec:
                // ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup").
                if ses.pending_open == Some(row) {
                    ses.plugins.added(row);
                }
            }
            Err(e) => {
                ses.plugins.sampler_failed.push(row);
                ses.touched.views.push(crate::session::View::Message(format!("Lemon Sampler: {e}")));
            }
        }
    }
    if added {
        ses.transport.set_instruments(ses.plugins.instruments());
        ses.dirty = true;
    }
    done
}

/// Project loading on each tick (spec: ui/project-loading.md): samples decode on worker threads first, then the sampler
/// instances, the plugin channels and the mixer effects start a few per tick (`HOST_BUDGET`) so the progress panel keeps
/// moving. Without a loading project, new sampler rows are hosted at once (`None`). `Some(true)`: the project has just
/// finished loading; `Some(false)`: it is still loading.
pub(crate) fn tick(ses: &mut crate::session::Session, scanning: bool) -> Option<bool> {
    let Some(decoded) = ses.loading.as_mut().map(|l| l.decode_done()) else {
        sync_samplers(ses, None);
        return None;
    };
    let deadline = std::time::Instant::now() + HOST_BUDGET;
    // Then the plugin channels and mixer effects, under the same budget, then the samplers' played samples.
    if decoded && sync_samplers(ses, Some(deadline)) && host_plugins(ses, deadline, scanning) && ses.loading.as_mut().is_some_and(|l| l.builds_done()) {
        ses.loading = None;
        ses.samples.wait = false;
        return Some(true);
    }
    Some(false)
}

/// Starts the opened project's plugin channels and mixer effects until `deadline` (`project_plugins::sync_wrapped`
/// and the other hosts stop after the instance that passes it): true when none is left to start. `scanning`: a plugin
/// scan is running (`project_plugins::host_native`).
fn host_plugins(ses: &mut crate::session::Session, deadline: std::time::Instant, scanning: bool) -> bool {
    ses.plugins.host_deadline = Some(deadline);
    ses.plugins.host_more = false;
    crate::app::sync_effects(ses);
    crate::project_plugins::sync_wrapped(ses);
    crate::osc3::sync_osc3(ses);
    crate::citrus::sync_citrus(ses);
    crate::smith::sync_smith(ses);
    crate::slicer::sync_slicer(ses);
    crate::drums::sync_drums(ses);
    if os_native::enabled() {
        crate::project_plugins::host_native(ses, scanning);
    }
    ses.plugins.host_deadline = None;
    let samplers = ses.hosted_rows.iter().filter(|&&r| ses.project.channels.get(r).is_some_and(|c| c.kind.is_sampler())).count();
    let done = (ses.plugins.hosted.len() + ses.plugins.wrap_failed.len() + ses.plugins.fx_failed.len() + ses.plugins.native_failed.len()).saturating_sub(samplers);
    let more = ses.plugins.host_more;
    if let Some(l) = &mut ses.loading {
        l.label.clear();
        l.plugins_done = done;
        // Plugins no host plays (FL's own effects without a recreation) are not waited for.
        if !more {
            l.plugins = done;
        }
    }
    !more
}
