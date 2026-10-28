// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's own project file (`.lemon`, `docs/native-format.md`): a zip holding
//! - `lemon.json`: what the file is (format, its version, the app that saved it, when);
//! - `project.flp`: the project as it is now, in FL's layout, written as a `.flp` save writes it (so everything that
//!   reads a project reads this, and taking it out of the zip gives FL its project);
//! - `journal.jsonl`: the project's whole history, every session it was edited in (`crate::journal`): what changed,
//!   when and by whom, and every state its plugins had;
//! - `ids.json`: its things' ids (`control::ids::stored`), so they keep them from session to session;
//! - `history.json`: its undo history (`history::History::stored`), so Ctrl+Z goes on past opening it;
//! - `versions.json` and `versions/<saved>.flp`: each earlier save's snapshot (the browser's Current project ▸
//!   Versions opens one).

use std::io::{Cursor, Read, Write};
use std::path::Path;

use serde_json::{json, Value};

/// The extension.
pub const EXT: &str = "lemon";
/// The format's version: a file of a newer one is not opened.
pub const VERSION: u64 = 1;

const MANIFEST: &str = "lemon.json";
const SNAPSHOT: &str = "project.flp";
const JOURNAL: &str = "journal.jsonl";
const IDS: &str = "ids.json";
const HISTORY: &str = "history.json";
const VERSIONS: &str = "versions.json";

/// An earlier save of a Lemon project, kept in the file: its snapshot's name in the zip and when it was saved (Unix ms).
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub file: String,
    pub saved: u64,
}

/// The versions Lemon project file `path` keeps, newest first (none for another file).
pub fn versions(path: &Path) -> Vec<Version> {
    let Ok(bytes) = std::fs::read(path).map_err(|_| ()).and_then(|b| if is_native(path) { Ok(b) } else { Err(()) }) else { return Vec::new() };
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else { return Vec::new() };
    let mut v = read_versions(&mut zip);
    v.reverse();
    v
}

/// Version `file`'s snapshot (an FLP) from Lemon project file `path`.
pub fn version(path: &Path, file: &str) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    zip.by_name(file).map_err(|e| format!("{file}: {e}"))?.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The versions list of an open Lemon project file, oldest first.
fn read_versions(zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>) -> Vec<Version> {
    let mut s = String::new();
    if zip.by_name(VERSIONS).ok().and_then(|mut f| f.read_to_string(&mut s).ok()).is_none() {
        return Vec::new();
    }
    let v: Value = serde_json::from_str(&s).unwrap_or_default();
    v.as_array().into_iter().flatten().filter_map(|e| Some(Version { file: e["file"].as_str()?.to_string(), saved: e["saved"].as_u64()? })).collect()
}

/// What the file at `path` keeps of its earlier saves, for the next one (a `.lemon` only): each version's snapshot,
/// oldest first, then the snapshot it holds now as the newest version, unless `flp` (what is saved now) is the same.
fn carried_versions(path: &Path, flp: &[u8]) -> Vec<(Version, Vec<u8>)> {
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let Ok(mut zip) = zip::ZipArchive::new(Cursor::new(bytes)) else { return Vec::new() };
    let list = read_versions(&mut zip);
    let mut out = Vec::new();
    for v in list {
        let mut b = Vec::new();
        if zip.by_name(&v.file).ok().and_then(|mut f| f.read_to_end(&mut b).ok()).is_some() {
            out.push((v, b));
        }
    }
    let mut now = Vec::new();
    let saved = {
        let mut s = String::new();
        zip.by_name(MANIFEST).ok().and_then(|mut f| f.read_to_string(&mut s).ok());
        serde_json::from_str::<Value>(&s).ok().and_then(|m| m["saved"].as_u64()).unwrap_or(0)
    };
    if zip.by_name(SNAPSHOT).ok().and_then(|mut f| f.read_to_end(&mut now).ok()).is_some() && now != flp && out.last().is_none_or(|(_, b)| *b != now) {
        out.push((Version { file: format!("versions/{saved}.flp"), saved }, now));
    }
    out
}

/// Whether `path` names a Lemon project file.
pub fn is_native(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT))
}

/// Whether `path` names a project file: a `.flp` or a Lemon project.
pub fn is_project_file(path: &Path) -> bool {
    is_native(path) || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flp"))
}

/// The extension a project saves with: its file's, when it has one; a Lemon project for a new one.
pub fn ext_for(path: Option<&Path>) -> String {
    path.filter(|p| is_project_file(p)).and_then(|p| p.extension()).map_or_else(|| EXT.to_string(), |e| e.to_string_lossy().into_owned())
}

/// What a Lemon project file holds besides the project.
#[derive(Debug, Default)]
pub struct Extra {
    /// The journal's entries, oldest first.
    pub journal: Vec<Value>,
    /// The project's ids (`control::ids::stored`).
    pub ids: Option<Value>,
    /// The undo history (`history::History::stored`).
    pub history: Option<Value>,
}

/// Reads a project file: a `.flp`'s bytes; a `.lemon`'s snapshot, and its history.
pub fn read(path: &Path) -> Result<(Vec<u8>, Option<Extra>), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if !is_native(path) {
        return Ok((bytes, None));
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a Lemon project: {e}"))?;
    let text = |zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str| -> Result<String, String> {
        let mut s = String::new();
        zip.by_name(name).map_err(|e| format!("{name}: {e}"))?.read_to_string(&mut s).map_err(|e| e.to_string())?;
        Ok(s)
    };
    let manifest: Value = serde_json::from_str(&text(&mut zip, MANIFEST)?).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let v = manifest["version"].as_u64().unwrap_or(0);
    if manifest["format"] != "lemon" || v == 0 {
        return Err("not a Lemon project".into());
    }
    if v > VERSION {
        return Err(format!("saved by a newer Lemon Studio (format {v}; this one reads up to {VERSION})"));
    }
    let mut flp = Vec::new();
    zip.by_name(SNAPSHOT).map_err(|e| format!("{SNAPSHOT}: {e}"))?.read_to_end(&mut flp).map_err(|e| e.to_string())?;
    // A file without a journal (or with a damaged line) still opens.
    let journal = text(&mut zip, JOURNAL).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let ids = text(&mut zip, IDS).ok().and_then(|t| serde_json::from_str(&t).ok());
    let history = text(&mut zip, HISTORY).ok().and_then(|t| serde_json::from_str(&t).ok());
    Ok((flp, Some(Extra { journal, ids, history })))
}

/// Writes project file `path`: the `.flp` bytes as they are, or a `.lemon` holding them, `journal`, `ids` and the undo
/// `history` (when there is one to keep).
pub fn write(path: &Path, flp: &[u8], journal: &[Value], ids: &Value, history: Option<&Value>) -> Result<u64, String> {
    if !is_native(path) {
        std::fs::write(path, flp).map_err(|e| e.to_string())?;
        return Ok(flp.len() as u64);
    }
    // The earlier saves go along: what the file holds now becomes the newest version.
    let versions = carried_versions(path, flp);
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let manifest = json!({ "format": "lemon", "version": VERSION, "app": env!("CARGO_PKG_VERSION"), "saved": t, "snapshot": SNAPSHOT, "journal": JOURNAL, "entries": journal.len() });
        let e = |e: zip::result::ZipError| e.to_string();
        zip.start_file(MANIFEST, opts).map_err(e)?;
        zip.write_all(serde_json::to_string_pretty(&manifest).unwrap_or_default().as_bytes()).map_err(|e| e.to_string())?;
        zip.start_file(SNAPSHOT, opts).map_err(e)?;
        zip.write_all(flp).map_err(|e| e.to_string())?;
        zip.start_file(JOURNAL, opts).map_err(e)?;
        for entry in journal {
            zip.write_all(entry.to_string().as_bytes()).map_err(|e| e.to_string())?;
            zip.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        zip.start_file(IDS, opts).map_err(e)?;
        zip.write_all(ids.to_string().as_bytes()).map_err(|e| e.to_string())?;
        if let Some(h) = history {
            zip.start_file(HISTORY, opts).map_err(e)?;
            zip.write_all(h.to_string().as_bytes()).map_err(|e| e.to_string())?;
        }
        if !versions.is_empty() {
            let list: Vec<Value> = versions.iter().map(|(v, _)| json!({ "file": v.file, "saved": v.saved })).collect();
            zip.start_file(VERSIONS, opts).map_err(e)?;
            zip.write_all(Value::Array(list).to_string().as_bytes()).map_err(|e| e.to_string())?;
            for (v, b) in &versions {
                zip.start_file(&v.file, opts).map_err(e)?;
                zip.write_all(b).map_err(|e| e.to_string())?;
            }
        }
        zip.finish().map_err(e)?;
    }
    // Through a temporary file, so a failed write keeps the old project.
    let bytes = buf.into_inner();
    let tmp = path.with_extension("lemon.part");
    std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        e.to_string()
    })?;
    Ok(bytes.len() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lemon_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("lemon-native-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.lemon");
        let journal = vec![json!({ "seq": 1, "kind": "session" }), json!({ "seq": 2, "kind": "step", "name": "Tempo" })];
        write(&p, b"FLhd-bytes", &journal, &json!({}), None).unwrap();
        let (flp, extra) = read(&p).unwrap();
        assert_eq!(flp, b"FLhd-bytes");
        assert_eq!(extra.unwrap().journal, journal);
        // A .flp is its bytes.
        let f = dir.join("x.flp");
        write(&f, b"FLhd", &journal, &json!({}), None).unwrap();
        let (b, x) = read(&f).unwrap();
        assert!(b == b"FLhd" && x.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A project saved as a Lemon project file opens again with its history: a plugin state from before is offered in
    /// Recover from journal and goes back in; saved again, the file keeps the earlier history and adds the new.
    #[test]
    fn a_lemon_project_keeps_its_history() {
        use crate::app::App;
        let dir = std::env::temp_dir().join(format!("lemon-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.lemon");
        let mut a = App::new(None);
        a.plugins.available = crate::plugins::builtin();
        a.state.project = crate::app::test_project();
        a.journal_opened(None, Vec::new());
        a.state.project.channels[0].kind = os_model::ChannelKind::Plugin;
        a.plugins.create_with(0, a.plugins.index_of(crate::citrus::CITRUS).unwrap(), None).unwrap();
        let first = a.plugins.state(0).unwrap();
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{:?}", a.state.message);
        let (_, extra) = read(&path).unwrap();
        let kinds: Vec<String> = extra.unwrap().journal.iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect();
        assert!(kinds.contains(&"new".to_string()) && kinds.contains(&"plugin_state".to_string()), "{kinds:?}");

        let mut b = App::new(None);
        b.plugins.available = crate::plugins::builtin();
        assert!(b.open(path.clone()), "{:?}", b.state.message);
        b.plugins.create_with(0, b.plugins.index_of(crate::citrus::CITRUS).unwrap(), None).unwrap();
        // Changed in this session, then the earlier session's state recovered.
        let p = b.plugins.param_infos(0).into_iter().find(|p| !p.read_only && !p.bypass && p.max > p.min).unwrap();
        let v = if (p.default - p.min).abs() > 1e-6 { p.min } else { p.max };
        assert!(b.plugins.set_param_named(0, &p.name, v));
        b.plugins.hosted[&0].plugin.lock().unwrap().idle();
        assert_ne!(b.plugins.state(0).as_ref(), Some(&first));
        let found = b.journal.states_of(&b.plugins.descriptor(0).unwrap().name, 10);
        let old = found.iter().find(|f| f.file == path).expect("the file's own history offers it");
        b.recover_state(0, &old.file.clone(), old.seq);
        assert_eq!(b.plugins.state(0), Some(first), "the earlier session's state back");
        // Saved again: the earlier history, then this session's.
        b.save_to(path.clone());
        let (_, extra) = read(&path).unwrap();
        let opened = extra.unwrap().journal.iter().filter(|e| e["kind"] == "new" || e["kind"] == "opened").count();
        assert_eq!(opened, 2, "both sessions");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Lemon project's things keep their ids from session to session, and new ones never reuse an id.
    #[test]
    fn ids_survive_saving() {
        use crate::app::App;
        let dir = std::env::temp_dir().join(format!("lemon-ids-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ids.lemon");
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.project.arrangements = vec![os_flp::project::default_arrangement()];
        crate::playlist::arr_mut(&mut a.state).clips.push(os_model::Clip::new(0, 384, 2, os_model::ClipSource::Pattern(1)));
        // A named pattern is stored even without notes.
        a.state.project.patterns[0].name = Some("Verse".into());
        // Ids given out before, so the project's are not 1, 2, 3...
        for _ in 0..3 {
            a.state.control.settle(&mut a.state.project);
            let c = a.state.project.channels[0].clone();
            a.state.project.channels.push(c);
            a.state.control.settle(&mut a.state.project);
            a.state.project.channels.pop();
        }
        a.state.control.settle(&mut a.state.project);
        let read = |p: &os_model::Project| {
            (p.channels.iter().map(|c| c.id.0).collect::<Vec<_>>(), p.patterns.iter().map(|c| c.id.0).collect::<Vec<_>>(), p.mixer.iter().map(|c| c.id.0).collect::<Vec<_>>(), p.arrangements[0].clips.iter().map(|c| c.id.0).collect::<Vec<_>>())
        };
        let before = read(&a.state.project);
        a.save_to(path.clone());
        let mut b = App::new(None);
        // This session has given out ids of its own already.
        b.state.control.settle(&mut b.state.project);
        assert!(b.open(path.clone()));
        b.state.control.settle(&mut b.state.project);
        assert_eq!(read(&b.state.project), before, "the same ids");
        let most = before.0.iter().chain(&before.1).chain(&before.2).chain(&before.3).max().copied().unwrap();
        let c = b.state.project.channels[0].clone();
        b.state.project.channels.push(c);
        b.state.control.settle(&mut b.state.project);
        assert!(b.state.project.channels.last().unwrap().id.0 > most, "a new thing gets a new id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Lemon project's undo history survives closing it: opened again, Ctrl+Z takes back each step made before
    /// the save, in order, and redo gives them again.
    #[test]
    fn undo_survives_closing() {
        use crate::app::App;
        use crate::knobs::KnobId;
        let dir = std::env::temp_dir().join(format!("lemon-undo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("undo.lemon");
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.project.arrangements = vec![os_flp::project::default_arrangement()];
        a.state.project.patterns[0].name = Some("Verse".into());
        crate::playlist::arr_mut(&mut a.state).clips.push(os_model::Clip::new(0, 384, 2, os_model::ClipSource::Pattern(1)));
        a.state.control.settle(&mut a.state.project);
        // What the project is at each point: the model and the tempo.
        let at = |a: &App| (a.state.project.clone(), a.state.tempo);
        let mut points = vec![at(&a)];
        // A setting (a command's Put), a knob, notes, the playlist, a mute (the channels' on states).
        crate::control::edit(&mut a.state, crate::control::command::TEMPO, crate::control::command::Region::Settings, |s| s.tempo = 141.0);
        points.push(at(&a));
        crate::history::knob(&mut a.state, KnobId::ChanVol(0), false);
        a.state.set_knob(KnobId::ChanVol(0), 0.3);
        points.push(at(&a));
        let snap = crate::history::Snap::pattern(&a.state, 1);
        crate::history::record(&mut a.state, "Add note", snap);
        let n = os_model::Note { position: 96, length: 24, channel: 0, key: 64, velocity: 100, pan: 64, fine_pitch: 120, release: 64, mod_x: 128, mod_y: 128, flags: 0x4000, group: 0, unk14: 0, repeat: 0 };
        crate::patterns::pat_mut(&mut a.state, 1).notes.push(n);
        points.push(at(&a));
        crate::playlist::snapshot(&mut a.state);
        crate::playlist::arr_mut(&mut a.state).clips.clear();
        points.push(at(&a));
        crate::rack::toggle_mute(&mut a.state, 1);
        points.push(at(&a));
        assert_eq!(a.state.history.done(), 5);
        a.save_to(path.clone());

        let mut b = App::new(None);
        assert!(b.open(path.clone()), "{:?}", b.state.message);
        assert_eq!(b.state.history.done(), 5, "the steps came along");
        assert_eq!(b.state.history.newest_done(), Some(crate::rack::MUTE_SOLO));
        // Each save stamps the work time: not what undo is about.
        // A clip's record bytes are written as it is saved: not what undo is about either.
        // The model's tempo follows the live one (compared beside it) only as the project is saved.
        let plain = |mut p: os_model::Project| {
            p.work_time = None;
            p.tempo = None;
            // An unset main volume is saved as its default.
            p.main_volume = Some(p.main_volume.unwrap_or(os_model::Project::MAIN_VOLUME_DEFAULT));
            p.arrangements.iter_mut().flat_map(|a| a.clips.iter_mut()).for_each(|c| c.raw = os_model::Clip::TEMPLATE);
            p
        };
        let model = |b: &App| (plain(b.state.project.clone()), b.state.tempo);
        // The file's model is what saving and reading give: compare each undo with the saved-and-read point.
        let read_back = |p: &(os_model::Project, f64)| {
            let mut t = App::new(None);
            t.state.project = p.0.clone();
            t.state.tempo = p.1;
            let f = dir.join("point.flp");
            t.save_to(f.clone());
            let mut r = App::new(None);
            r.open(f);
            (plain(r.state.project.clone()), r.state.tempo)
        };
        assert_eq!(model(&b), read_back(&points[5]));
        for k in (0..5).rev() {
            crate::history::undo(&mut b.state).unwrap();
            assert_eq!(model(&b), read_back(&points[k]), "undone to point {k}");
        }
        for k in 1..=5 {
            crate::history::redo(&mut b.state).unwrap();
            assert_eq!(model(&b), read_back(&points[k]), "redone to point {k}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each save keeps the one before it as a version; opening an earlier version makes it the project, in the same
    /// file, with the file's history going on; saving then keeps the newest as a version too.
    #[test]
    fn versions_of_a_lemon_project() {
        use crate::app::App;
        let dir = std::env::temp_dir().join(format!("lemon-versions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v.lemon");
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.journal_opened(None, Vec::new());
        for t in [120.0, 150.0, 170.0] {
            a.state.tempo = t;
            a.save_to(path.clone());
        }
        let v = versions(&path);
        assert_eq!(v.len(), 2, "the two earlier saves: {v:?}");
        assert!(v[0].saved >= v[1].saved, "newest first");
        assert_eq!(a.state.versions, v);
        // The oldest opened as the project: its tempo, the same file, unsaved.
        let oldest = v[1].file.clone();
        a.open_version(&oldest);
        assert_eq!(a.state.tempo, 120.0);
        assert_eq!(a.state.path.as_deref(), Some(path.as_path()));
        assert!(a.unsaved());
        a.save_to(path.clone());
        let v = versions(&path);
        assert_eq!(v.len(), 3, "170 is kept as a version too");
        let (_, extra) = read(&path).unwrap();
        let kinds: Vec<String> = extra.unwrap().journal.iter().map(|e| e["kind"].as_str().unwrap_or("").to_string()).collect();
        assert!(kinds.contains(&"new".to_string()) && kinds.contains(&"version".to_string()), "the history went on: {kinds:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
