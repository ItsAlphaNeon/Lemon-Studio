// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Studio's own project file (`.lemon`, `docs/native-format.md`): a zip holding
//! - `lemon.json`: what the file is (format, its version, the app that saved it, when);
//! - `project.flp`: the project as it is now, in FL's layout, written as a `.flp` save writes it (so everything that
//!   reads a project reads this, and taking it out of the zip gives FL its project);
//! - `journal.jsonl`: the project's whole history, every session it was edited in (`crate::journal`): what changed,
//!   when and by whom, and every state its plugins had.

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

/// Whether `path` names a Lemon project file.
pub fn is_native(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT))
}

/// Whether `path` names a project file: a `.flp` or a Lemon project.
pub fn is_project_file(path: &Path) -> bool {
    is_native(path) || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flp"))
}

/// What a Lemon project file holds besides the project.
#[derive(Debug, Default)]
pub struct Extra {
    /// The journal's entries, oldest first.
    pub journal: Vec<Value>,
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
    Ok((flp, Some(Extra { journal })))
}

/// Writes project file `path`: the `.flp` bytes as they are, or a `.lemon` holding them and `journal`.
pub fn write(path: &Path, flp: &[u8], journal: &[Value]) -> Result<u64, String> {
    if !is_native(path) {
        std::fs::write(path, flp).map_err(|e| e.to_string())?;
        return Ok(flp.len() as u64);
    }
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
        write(&p, b"FLhd-bytes", &journal).unwrap();
        let (flp, extra) = read(&p).unwrap();
        assert_eq!(flp, b"FLhd-bytes");
        assert_eq!(extra.unwrap().journal, journal);
        // A .flp is its bytes.
        let f = dir.join("x.flp");
        write(&f, b"FLhd", &journal).unwrap();
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
}
