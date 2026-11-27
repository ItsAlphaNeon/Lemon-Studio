// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! FILE ▸ Export all playlist track groups...: a folder picker, then one file per playlist track group (a track and
//! the grouped tracks under it) and per ungrouped track that has clips, from the song's start to the end of its clips
//! plus the tail, "<project> - <track name>.wav", "_2", "_3"... for a name taken; tracks without clips are left out
//! (spec: audio/export.md §1f). Files there already are asked about first and one another program holds is an error,
//! as the Export form does (§1e).

use std::collections::HashSet;
use std::path::PathBuf;

use os_model::Clip;

use crate::app::{App, State};
use crate::playlist::Part;

/// The playlist's track groups, in order: each a leading track and the grouped tracks right under it.
pub fn groups(s: &State) -> Vec<Vec<u16>> {
    let Some(arr) = s.session.project.arrangement() else { return Vec::new() };
    let mut v: Vec<Vec<u16>> = Vec::new();
    for (i, t) in arr.tracks.iter().enumerate() {
        match v.last_mut() {
            Some(g) if t.grouped => g.push(i as u16),
            _ => v.push(vec![i as u16]),
        }
    }
    v
}

/// The files to write from the `audible` clips: a group with clips each, named after its leading track, a name taken
/// (any case) by an earlier file getting "_2", "_3"..., each with its tail.
pub fn parts(s: &State, audible: &[Clip]) -> Vec<Part> {
    let project = s
        .session
        .path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map_or_else(|| "Untitled".to_string(), |n| n.to_string_lossy().into_owned());
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for g in groups(s) {
        let Some(end) = audible.iter().filter(|c| g.contains(&c.track)).map(|c| c.end()).max() else { continue };
        let base =
            crate::playlist::file_safe(&format!("{project} - {}", crate::edit::playlist::track_name(&s.session, g[0])));
        let name = std::iter::once(base.clone())
            .chain((2..).map(|k| format!("{base}_{k}")))
            .find(|n| !taken.contains(&n.to_lowercase()))
            .unwrap_or(base);
        taken.insert(name.to_lowercase());
        out.push(Part { tracks: g, name, end, ring: true });
    }
    out
}

/// The menu entry: "Select rendering folder", starting in the Rendered folder.
pub fn ask_folder(app: &mut App) {
    use crate::filedlg::{FileDlg, Mode};
    let start = crate::playlist::render_dir(&app.state);
    let start = start.is_dir().then_some(start);
    let dlg = FileDlg::new(Mode::Folder, "Select rendering folder", "export", crate::dialog::Then::GroupExport)
        .start(start.as_deref());
    app.file_dialog(dlg);
}

/// The folder chosen: the groups render one after another into it, once the files there already are answered for.
pub fn export(app: &mut App, dir: PathBuf) -> String {
    start(app, dir, false)
}

/// "Confirm save as" answered Yes: the groups render over the files there.
pub fn export_over(app: &mut App, dir: PathBuf) -> String {
    start(app, dir, true)
}

fn start(app: &mut App, dir: PathBuf, over: bool) -> String {
    use crate::dialog::{spans, Kind, Then};
    if app.state.session.export.is_some() {
        return "Export track groups: another render is running".into();
    }
    let s = &app.state;
    let audible: Vec<Clip> = crate::edit::playlist::clips(&s.session)
        .iter()
        .filter(|c| !c.muted && crate::edit::playlist::track_on(&s.session, c.track))
        .copied()
        .collect();
    let todo = parts(s, &audible);
    if todo.is_empty() {
        return "Export track groups: the playlist has no clips to play".into();
    }
    let targets: Vec<PathBuf> = todo.iter().map(|p| dir.join(format!("{}.wav", p.name))).collect();
    if let Some(text) = crate::export_safety::in_use_text(&targets) {
        app.tell(Kind::Error, "Error", spans(&[&text]));
        return "Export track groups: a file is in use".into();
    }
    if !over {
        if let Some(m) = crate::export_safety::replace_msg(&targets, Then::GroupReplace(dir.clone())) {
            app.show_msg(m);
            return "Export track groups: some files are there already".into();
        }
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("Export track groups: {e}");
    }
    crate::playlist::start_parts(app, todo, audible, dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_model::ClipSource;

    /// Groups lead by an ungrouped track; a file per group with clips, named after the leading track, "_2" for a name
    /// met before; tracks without clips are left out; each part lasts to the end of its own clips.
    #[test]
    fn a_file_per_group_with_clips() {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.set_song_mode(true);
        for (track, at) in [(0u16, 0u32), (2, 960), (4, 0), (6, 192)] {
            crate::edit::playlist::place_in(&mut app.state.session, ClipSource::Pattern(1), track, at, None);
        }
        {
            let p = app.state.session.project.get_mut();
            let arr = p.arrangement_mut().unwrap();
            for t in arr.tracks.iter_mut() {
                t.grouped = false;
                t.name = None;
            }
            arr.tracks[1].grouped = true;
            arr.tracks[2].grouped = true;
            arr.tracks[0].name = Some("Vox".into());
            arr.tracks[4].name = Some("vox_2".into());
            arr.tracks[6].name = Some("Vox".into());
        }
        let g = groups(&app.state);
        assert_eq!(g[0], [0, 1, 2]);
        assert_eq!(g[1], [3]);
        let audible = crate::edit::playlist::clips(&app.state.session).to_vec();
        let parts = parts(&app.state, &audible);
        let names: Vec<_> = parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Untitled - Vox", "Untitled - vox_2", "Untitled - Vox_3"], "no two files of one name");
        assert!(parts.iter().all(|p| p.ring), "with the tail");
        let job = crate::playlist::render_job(&app.state, std::path::Path::new("."), "x", 96, true);
        assert_eq!(job.options.tail, crate::export::Tail::Ring);
        assert_eq!(parts[0].tracks, [0, 1, 2]);
        let ends: Vec<u32> = parts.iter().map(|p| p.end).collect();
        let len = audible[0].length;
        assert_eq!(ends, [960 + len, len, 192 + len]);
    }

    /// A file there already: "Confirm save as" (No focused) before anything renders; No leaves it, Yes renders.
    #[test]
    fn files_there_are_asked_about() {
        let dir = std::env::temp_dir().join(format!("lemon-group-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.set_song_mode(true);
        crate::edit::playlist::place_in(&mut app.state.session, ClipSource::Pattern(1), 0, 0, None);
        let name = parts(&app.state, crate::edit::playlist::clips(&app.state.session))[0].name.clone();
        let file = dir.join(format!("{name}.wav"));
        std::fs::write(&file, b"old").unwrap();
        export(&mut app, dir.clone());
        let Some(crate::dialog::Body::Msg(m)) = app.state.dialogs.last().map(|d| &d.body) else { panic!("a message") };
        assert_eq!((m.title.as_str(), m.focus), ("Confirm save as", 1));
        assert!(app.state.session.export.is_none(), "nothing rendered yet");
        app.dialog_click(crate::dialog::DHit::Button(1));
        assert!(app.state.dialogs.is_empty() && app.state.session.export.is_none(), "No: nothing");
        export(&mut app, dir.clone());
        app.dialog_click(crate::dialog::DHit::Button(0));
        assert!(app.state.session.export.is_some(), "Yes renders");
        while app.state.session.export.is_some() || app.playlist_render.is_some() {
            app.plugin_tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(std::fs::metadata(&file).unwrap().len() > 44, "replaced");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
