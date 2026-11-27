// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Opening and saving, the session's half: a project file read into the session, and the project as playback and the
//! hosted plugins have it written to a file. The interface adds what is its own (the views, Lemon Studio's state, the
//! time spent: `App::open`, `App::write_project`).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)

use os_model::Project;
use std::path::Path;

use crate::session::Session;

/// Sampler channels keep their sample and settings in Lemon Sampler: copied to the channel so the FLP stores them.
pub fn store_samplers(ses: &mut Session) {
    for row in ses.hosted_rows.clone() {
        if ses.project.channels.get(row).is_some_and(|c| c.kind.is_sampler()) {
            let lemon = ses.plugins.descriptor(row).is_some_and(|d| d.name == crate::plugins::LEMON_SAMPLER);
            if let Some(st) = ses.plugins.state(row).filter(|_| lemon) {
                crate::plugins::store_sampler(&ses.settings, &mut ses.project.get_mut().channels[row], &st);
            }
            if let Some(p) = ses.plugins.sampler_path(row) {
                // A placeholder path that still names the same file is kept as the project wrote it.
                let ch = &mut ses.project.get_mut().channels[row];
                if ch.sample_path.as_deref().is_some_and(|old| crate::plugins::resolve_sample(&ses.settings, old) == p) {
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
pub fn store_builtin_states(ses: &mut Session) {
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
    crate::osc3::store(&ses.settings, ses.project.get_mut(), &ses.plugins);
    crate::citrus::store(ses.project.get_mut(), &ses.plugins);
    crate::smith::store(ses.project.get_mut(), &ses.plugins);
    crate::slicer::store(ses.project.get_mut(), &ses.plugins);
    crate::drums::store(ses.project.get_mut(), &ses.plugins);
    crate::soundfont::store(ses.project.get_mut(), &ses.plugins);
}

/// Writes the project to `target`; `before` is the project before the interface stored its part, `extra` Lemon
/// Studio's own events (`lemon_state::events`) and `edit` the interface's change to the copy written. Returns the size
/// written, or why not, and what the status bar says.
pub fn write_in(ses: &mut Session, target: &Path, before: os_model::Project, extra: Vec<os_flp::Event>, edit: &mut dyn FnMut(&mut os_model::Project)) -> (Result<u64, String>, String) {
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
        crate::project_io::settle_origins(ses.project.get_mut());
        // FL's files always have a playlist, and the mixer is stored after it (spec: flp/events/mixer.md).
        if ses.project.arrangements.is_empty() {
            ses.project.get_mut().arrangements.push(os_flp::project::default_arrangement());
        }
        let mut first = ses.project.clone();
        crate::edit::rack::as_saved(&mut first);
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
    let mut lost = os_flp::project::unsaved_parts(&ses.project, &file);
    // An FLP file has no place for an audio clip's own pitch, pan and reverse (a `.lemon` file's event 204 does).
    if crate::project_files::is_flp(target) && crate::clip_reverse::used(&ses.project) {
        lost.push("audio clip pitch, pan and reverse");
    }
    // Hosted CLAP / VST3 plugins are written as wrapped plugins with their current state (spec: flp/events/plugins.md).
    let mut proj = ses.project.clone();
    crate::edit::rack::as_saved(&mut proj);
    crate::project_plugins::store_hosted(&mut proj, &ses.plugins);
    edit(&mut proj);
    // Added mixer tracks are written before the hidden last track (spec: flp/events/mixer.md "Adding tracks").
    let result = os_flp::project::apply(&proj, &mut file)
        .map_err(|e| e.to_string())
        .and_then(|()| crate::project_io::lemon_or_flp(&mut file, target, extra))
        .and_then(|()| file.to_bytes().map_err(|e| e.to_string()))
        .and_then(|b| crate::replace_file::replace(target, &b).map(|()| b.len() as u64).map_err(|e| e.to_string()));
    let message = match &result {
        Ok(_) if lost.is_empty() => format!("Saved {}", target.display()),
        Ok(_) => format!("Saved {} (not stored yet: {})", target.display(), lost.join(", ")),
        Err(e) => format!("Save failed: {e}"),
    };
    if result.is_ok() {
        // The saved file now holds the channels and mixer tracks in model order, and the plugins as written.
        crate::project_io::settle_origins(ses.project.get_mut());
        crate::project_plugins::adopt_saved(ses.project.get_mut(), &proj);
        ses.file = Some(file);
        ses.path = Some(target.to_path_buf());
    }
    (result, message)
}

/// Every hosted instance goes and nothing is loading (a project opens, the program ends).
pub fn reset_hosted(ses: &mut Session) {
    ses.plugins.clear();
    ses.loading = None;
    ses.samples.wait = false;
    ses.hosted_rows.clear();
    ses.plugins.sampler_failed.clear();
    ses.plugins.fx_failed.clear();
    ses.plugins.wrap_failed.clear();
    ses.plugins.native_failed.clear();
    ses.transport.set_instruments(Vec::new());
    ses.transport.set_effects(Vec::new());
}

/// Another project replaces the session's (open, New, a MIDI file opened as a project): playback and the hosted
/// plugins stop, and what the session remembers of the old project goes: a recording take (unwritten), what
/// performance mode launched, the playback positions, the history and the control plane.
pub fn leave_in(ses: &mut Session) {
    ses.transport.silence();
    reset_hosted(ses);
    ses.rec.forget();
    ses.perform = crate::perform::Perform::default();
    ses.pat_pos = 0;
    ses.song_pos = 0;
    ses.history.reset();
    ses.control.reset();
}

/// `project`, read from `file` at `path`, becomes the session's, after `leave_in`. The interface then sets its views
/// (`App::open`) and starts the loading (`start_loading`).
pub fn open_in(ses: &mut Session, file: os_flp::FlpFile, mut project: os_model::Project, path: std::path::PathBuf) {
    leave_in(ses);
    // As saved (event 9); a file without it opens in song mode, as in FL 26 (spec: flp/engine-events.md "Project").
    project.song_mode = Some(project.song_mode.unwrap_or(true));
    project.current_pattern = Some(project.current_pattern.unwrap_or(1).max(1));
    project.tempo = Some(project.tempo.unwrap_or(130.0));
    ses.project = crate::locked::Locked::new(project);
    // Opening in song mode sets the initial values, as FL 26 does (spec: flp/engine-events.md "Event 216"); the project
    // is opened with them, not changed.
    crate::init::apply(ses);
    // One whose file has no start is started now, as a writer creating a project does (spec:
    // flp/events/project-info.md "Writer notes").
    ses.project.get_mut().work_time.get_or_insert_with(crate::project_info::work_time_now);
    ses.file = Some(file);
    ses.path = Some(path);
    ses.control.event(&crate::control::By::User, "opened", ses.path.as_ref().map_or(String::new(), |p| p.display().to_string()), Vec::new());
    // Live notes use the new project.
    ses.dirty = true;
    // Ids for what the file holds, before Lemon Studio's own state (which names channels by position) is applied.
    ses.control.settle(ses.project.get_mut());
}

/// The opened project's samples start decoding and its plugins starting (`loading::tick`).
pub fn start_loading(ses: &mut Session) {
    ses.loading = Some(crate::loading::Loading::start(&ses.settings, &ses.project));
    ses.samples.wait = true;
}

/// Reads the project file at `path`: its file and project, and whether it was converted. A project in the layout used
/// before version 24.2 is converted to the current one (spec: flp/layouts.md "Converting to the newer layout"); one the
/// conversion cannot handle is refused with the version it was saved by (the second part of the error).
pub fn read(path: &Path) -> Result<(os_flp::FlpFile, os_model::Project, bool), (String, Option<String>)> {
    let mut f = std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| os_flp::FlpFile::parse(&b).map_err(|e| e.to_string())).map_err(|e| (e, None))?;
    match os_flp::project::upgrade(&mut f).and_then(|converted| Ok((os_flp::project::load(&f)?, converted))) {
        Ok((p, converted)) => Ok((f, p, converted)),
        Err(os_flp::project::ProjectError::LegacyLayout(v)) => Err((os_flp::project::ProjectError::LegacyLayout(v.clone()).to_string(), Some(v))),
        Err(e) => Err((e.to_string(), None)),
    }
}

/// The plugin host's work on one tick without an interface (`--render`, `--serve`): the opened project's loading, the
/// stock plugins kept in step with the project, the mixer's effects and containers, the commands' plugins, samples into
/// the samplers, and the instances' housekeeping. What it would show is dropped.
pub fn host_tick(ses: &mut Session) {
    // Instances follow their channels and slots by id (a client's move, an undo), as the window's tick does first.
    let plan = crate::control::host::follow_plan(ses, true, true);
    crate::control::host::follow_apply(ses, plan);
    crate::loading::tick(ses, false);
    crate::lemon_midi_out::sync_midi_outs(ses);
    let _ = crate::edit::mixer::fx_requests_in(ses);
    let _ = crate::container::sync_containers_in(ses);
    if ses.loading.is_none() {
        crate::project_plugins::sync_effects(ses);
        crate::project_plugins::sync_wrapped(ses);
        crate::osc3::sync_osc3(ses);
        crate::citrus::sync_citrus(ses);
        crate::smith::sync_smith(ses);
        crate::slicer::sync_slicer(ses);
        crate::drums::sync_drums(ses);
        crate::soundfont::sync_soundfont(ses);
        if crate::native::enabled() {
            crate::project_plugins::host_native(ses, false);
        }
    }
    crate::control::host::host_commands(ses);
    crate::control::host::sample_loads(ses);
    let _ = ses.plugins.idle_headless();
    ses.touched = Default::default();
}

/// Saves the project to `target` without an interface (a client of `--serve`): what a `.lemon` file keeps of the
/// interface's choices is what a new interface starts with (`lemon_state::Looks::none`), and the time spent is the
/// project's. On success the project counts as saved. Returns the size written, or why not, and what was said.
pub fn save(ses: &mut Session, target: &Path) -> (Result<u64, String>, String) {
    store_samplers(ses);
    let before = ses.project.clone();
    let looks = crate::lemon_state::Looks::none();
    let extra = crate::lemon_state::events_in(ses, &ses.plugins, &looks);
    let (result, message) = write_in(ses, target, before, extra, &mut |_| {});
    if result.is_ok() {
        mark_saved(ses, &looks);
    }
    (result, message)
}

/// The project as it is now counts as saved (or opened, or made), with the interface's choices `looks`: what unsaved
/// changes are measured against (`App::unsaved`).
pub fn mark_saved(ses: &mut Session, looks: &crate::lemon_state::Looks) {
    // New things get their ids before the copy, so ids alone are no unsaved change.
    let control = &mut ses.control;
    ses.project.write_if(|p| control.settle(p));
    ses.saved = (ses.project.clone(), ses.tempo(), crate::lemon_state::saved_form_in(ses, &ses.plugins, looks));
    ses.plugin_edited = false;
}

/// Opens the project file at `path` without an interface (`--render`, `--serve`): read, made the session's with what a
/// `.lemon` file keeps of the project, Patcher slots as containers, counted as saved, and loading. Returns what an
/// interface would say, or why it could not open.
pub fn open_headless(ses: &mut Session, path: &Path) -> Result<String, String> {
    let (file, project, _) = read(path).map_err(|(e, _)| format!("Could not open {}: {e}", path.display()))?;
    open_in(ses, file, project, path.to_path_buf());
    if let Some(f) = ses.file.take() {
        crate::lemon_state::apply_session(ses, &f);
        ses.file = Some(f);
    }
    let effects = ses.plugins.effects.clone();
    let patchers = crate::container::open_patchers(ses, &effects);
    mark_saved(ses, &crate::lemon_state::Looks::none());
    start_loading(ses);
    Ok(format!("Opened {}{}", path.display(), patchers.map_or(String::new(), |m| format!(". {m}"))))
}

/// The plugins the program offers without an interface: the built-in ones and those the last scan found.
pub fn last_scan() -> crate::plugins::Plugins {
    let mut p = crate::plugins::Plugins::builtins();
    if let Some(cache) = crate::plugin_db::Cache::load() {
        p.merge(&cache);
    }
    p
}

/// What a copy of the project (`App::save_copy`, a backup) leaves as it was: the project, its file and name, its saved
/// point and its unsaved plugin edits.
pub struct Kept {
    project: os_model::Project,
    written: Option<&'static std::panic::Location<'static>>,
    path: Option<std::path::PathBuf>,
    file: Option<os_flp::FlpFile>,
    saved: (os_model::Project, f64, Vec<u8>),
    plugin_edited: bool,
}

impl Kept {
    pub fn of(ses: &Session) -> Kept {
        Kept {
            project: ses.project.clone(),
            written: ses.project.written_at(),
            path: ses.path.clone(),
            file: ses.file.clone(),
            saved: ses.saved.clone(),
            plugin_edited: ses.plugin_edited,
        }
    }

    pub fn put(self, ses: &mut Session) {
        (ses.path, ses.file, ses.saved, ses.plugin_edited) = (self.path, self.file, self.saved, self.plugin_edited);
        ses.project = crate::locked::Locked::restored(self.project, self.written);
    }
}

/// Writes a copy of the project to `target` without an interface (a backup of a served session): as `save`, but the
/// project keeps its file, name and saved point.
pub fn copy(ses: &mut Session, target: &Path) -> Result<(), String> {
    let kept = Kept::of(ses);
    store_samplers(ses);
    let before = ses.project.clone();
    let extra = crate::lemon_state::events_in(ses, &ses.plugins, &crate::lemon_state::Looks::none());
    let (result, message) = write_in(ses, target, before, extra, &mut |_| {});
    kept.put(ses);
    result.map(|_| ()).map_err(|_| message.trim_start_matches("Save failed: ").to_string())
}

/// Whether the project changed since it was last opened, saved or made, with the interface's choices `looks`: the
/// session's half of `App::unsaved`.
pub fn unsaved(ses: &Session, looks: &crate::lemon_state::Looks) -> bool {
    !crate::projects::same_as_saved(&ses.saved.0, &ses.project)
        || ses.saved.1 != ses.tempo()
        || ses.saved.2 != crate::lemon_state::saved_form_in(ses, &ses.plugins, looks)
        || ses.plugin_edited
}

/// The file just written holds the model's channels and mixer tracks in order: each one's file block is its position
/// (os-flp `apply` follows moved channels and the effect slots of moved mixer tracks by these).
/// Makes the file written to `target` a `.lemon` file (Lemon Studio's chunks around the FLP's, saved by this version,
/// with `extra`: Lemon Studio's own state, `crate::lemon_state`), or a plain FLP for a target named `.flp`, which has no
/// place for `extra` (spec: flp/lemon-format.md).
pub fn lemon_or_flp(file: &mut os_flp::FlpFile, target: &std::path::Path, mut extra: Vec<os_flp::Event>) -> Result<(), String> {
    if crate::project_files::is_flp(target) {
        os_flp::lemon::to_flp(file);
        return Ok(());
    }
    extra.insert(0, os_flp::lemon::saved_by(&format!("Lemon Studio {}", crate::version::VERSION)));
    os_flp::lemon::to_lemon(file, &extra).map_err(|e| e.to_string())
}

pub fn settle_origins(p: &mut Project) {
    for (i, c) in p.channels.iter_mut().enumerate() {
        c.rack.origin = Some(i as u16);
    }
    for (i, m) in p.mixer.iter_mut().enumerate() {
        m.origin = Some(i as u16);
    }
}

