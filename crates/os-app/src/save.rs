// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Saving, the session's half: the project as playback and the hosted plugins have it, written to a file. The
//! interface adds what is its own (the playlist's view, Lemon Studio's state, the time spent, `App::write_project`).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)

use std::path::Path;

use crate::session::Session;

/// Sampler channels keep their sample and settings in Lemon Sampler: copied to the channel so the FLP stores them.
pub(crate) fn store_samplers(ses: &mut Session) {
    for row in ses.hosted_rows.clone() {
        if ses.project.channels.get(row).is_some_and(|c| c.kind.is_sampler()) {
            let lemon = ses.plugins.descriptor(row).is_some_and(|d| d.name == crate::plugins::LEMON_SAMPLER);
            if let Some(st) = ses.plugins.state(row).filter(|_| lemon) {
                crate::plugins::store_sampler(&mut ses.project.get_mut().channels[row], &st);
            }
            if let Some(p) = ses.plugins.sampler_path(row) {
                // A placeholder path that still names the same file is kept as the project wrote it.
                let ch = &mut ses.project.get_mut().channels[row];
                if ch.sample_path.as_deref().is_some_and(|old| crate::plugins::resolve_sample(old) == p) {
                    continue;
                }
                // An empty path: the sample was taken out (spec: flp/events/channels.md "Loop points (event 251)").
                if !p.is_empty() {
                    ch.sample_path = Some(p);
                } else if ch.sample_path.is_some() {
                    ch.clear_sample();
                }
            }
        }
    }
}

/// The built-in recreations write their settings into the model as the file's own plugin state: Lemon Parametric
/// EQ, Tesla (as Edison), Pulp Filter (as Love Philter) and Press (as Maximus) into their slots, Lemon Trio into its
/// channels.
pub(crate) fn store_builtin_states(ses: &mut Session) {
    use crate::plugins::{EDISON_FILE_NAME, EQ_FILE_NAME, LOVE_PHILTER_FILE_NAME, MAXIMUS_FILE_NAME};
    for key in ses.plugins.hosted.keys().copied().filter(|&k| k >= crate::plugins::FX_KEY).collect::<Vec<_>>() {
        let Some((t, k)) = crate::plugins::fx_of(key) else { continue };
        let Some(slot) = ses.project.get_mut().mixer.iter_mut().find(|m| m.index == t).and_then(|m| m.slots[usize::from(k)].as_mut()) else { continue };
        let st = match slot.name.as_str() {
            EQ_FILE_NAME => ses.plugins.eq_file_state(key),
            EDISON_FILE_NAME => ses.plugins.edison_file_state(key),
            LOVE_PHILTER_FILE_NAME => ses.plugins.love_philter_file_state(key),
            MAXIMUS_FILE_NAME => ses.plugins.maximus_file_state(key),
            file => ses.plugins.recreation_file_state(key, file),
        };
        if let Some(st) = st {
            slot.state = st;
        }
    }
    crate::osc3::store(ses.project.get_mut(), &ses.plugins);
    crate::citrus::store(ses.project.get_mut(), &ses.plugins);
    crate::smith::store(ses.project.get_mut(), &ses.plugins);
    crate::slicer::store(ses.project.get_mut(), &ses.plugins);
    crate::drums::store(ses.project.get_mut(), &ses.plugins);
}

/// Writes the project to `target`; `before` is the project before the interface stored its part, `extra` Lemon
/// Studio's own events (`lemon_state::events`) and `edit` the interface's change to the copy written. Returns the size
/// written, or why not, and what the status bar says.
pub(crate) fn write_in(ses: &mut Session, target: &Path, before: os_model::Project, extra: Vec<os_flp::Event>, edit: &mut dyn FnMut(&mut os_model::Project)) -> (Result<u64, String>, String) {
    store_builtin_states(ses);
    let tempo = ses.tempo();
    ses.project.write_if(|p| p.tempo.replace(tempo) != Some(tempo));
    // Kept as the project says, or pattern mode, as a new project's (an unsaved change of none).
    ses.project.write_if(|p| {
        let unset = p.song_mode.is_none();
        p.song_mode.get_or_insert(false);
        unset
    });
    if ses.project != before {
        ses.dirty = true;
    }
    // A file Lemon Studio made before the project had a playlist has no arrangement block (FL's files always have
    // one): build it again from the model, which holds everything that file stored.
    let lacks_playlist = ses.file.as_ref().and_then(|f| os_flp::project::load(f).ok()).is_some_and(|p| p.arrangements.is_empty()) && !ses.project.arrangements.is_empty();
    // A project that was not opened from a file gets a new FLP built from the model.
    let mut file = if ses.file.is_none() || lacks_playlist {
        // The new file's blocks are the model's channels and mixer tracks in order.
        crate::app::settle_origins(ses.project.get_mut());
        // FL's files always have a playlist, and the mixer is stored after it (spec: flp/events/mixer.md).
        if ses.project.arrangements.is_empty() {
            ses.project.get_mut().arrangements.push(os_flp::project::default_arrangement());
        }
        let mut first = ses.project.clone();
        crate::rack::as_saved(&mut first);
        crate::project_plugins::store_hosted(&mut first, &ses.plugins);
        match os_flp::project::create(&first) {
            Ok(f) => f,
            Err(e) => return (Err(e.to_string()), format!("Save failed: {e}")),
        }
    } else {
        ses.file.clone().expect("checked")
    };
    // Deleted, cloned and moved channels are written by `apply` (os-flp `restructure`); it edits a copy so a failed
    // save leaves the loaded file as it was.
    let lost = os_flp::project::unsaved_parts(&ses.project, &file);
    // Hosted CLAP / VST3 plugins are written as wrapped plugins with their current state (spec: flp/events/plugins.md).
    let mut proj = ses.project.clone();
    crate::rack::as_saved(&mut proj);
    crate::project_plugins::store_hosted(&mut proj, &ses.plugins);
    edit(&mut proj);
    // Added mixer tracks are written before the hidden last track (spec: flp/events/mixer.md "Adding tracks").
    let result = os_flp::project::apply(&proj, &mut file)
        .map_err(|e| e.to_string())
        .and_then(|()| crate::app::lemon_or_flp(&mut file, target, extra))
        .and_then(|()| file.to_bytes().map_err(|e| e.to_string()))
        .and_then(|b| crate::replace_file::replace(target, &b).map(|()| b.len() as u64).map_err(|e| e.to_string()));
    let message = match &result {
        Ok(_) if lost.is_empty() => format!("Saved {}", target.display()),
        Ok(_) => format!("Saved {} (not stored yet: {})", target.display(), lost.join(", ")),
        Err(e) => format!("Save failed: {e}"),
    };
    if result.is_ok() {
        // The saved file now holds the channels and mixer tracks in model order, and the plugins as written.
        crate::app::settle_origins(ses.project.get_mut());
        crate::project_plugins::adopt_saved(ses.project.get_mut(), &proj);
        ses.file = Some(file);
        ses.path = Some(target.to_path_buf());
    }
    (result, message)
}
