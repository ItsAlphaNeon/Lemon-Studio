// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project journal: everything that happens to the project, as it happens, written to disk at once, so nothing
//! made between saves is lost and every change can be read back (what, when, by whom). One file per session in the
//! user folder's `Journal`, one JSON object per line (`docs/control-plane.md` "Journal"):
//! - `session`: the app started;
//! - `opened`, `new`, `saved`: the project was opened from a file, made new, or saved (with the path); `copied`: a
//!   copy of it was written (a backup, a zipped project);
//! - `step`, `undo`, `redo`: an undo step was made (its name: who and what) or taken back or given again;
//! - `changes`: what the project's reading (`control::query::project`) gained, lost or changed, by id;
//! - `notes`: a pattern's notes added and removed (every stored field; a moved note is one removed and one added);
//! - `plugin_state`: a hosted plugin's whole state (base64), each time it changes while its window is open, as the
//!   window closes, and as the project is saved: a sound made in a plugin's window survives an unsaved project.
//!
//! - `closed`: the app closed normally (`crate::crash`).
//!
//! The control plane reads it back (`journal`). Journals older than [`KEEP_DAYS`] go at startup.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::app::App;

/// How long journals are kept.
pub const KEEP_DAYS: u64 = 30;
/// How often the plugins being edited are asked for their state.
const PLUGIN_EVERY: Duration = Duration::from_secs(2);
/// How often, at most, the project is read for its changes (a drag changes it every frame).
const PROJECT_EVERY: Duration = Duration::from_millis(250);
/// A larger plugin state (a sampler holding its audio) is noted by size and hash only.
const STATE_MAX: usize = 4 << 20;
/// How many entries the control plane can read back from memory.
const MEMORY: usize = 20_000;

#[derive(Default)]
pub struct Journal {
    file: Option<BufWriter<File>>,
    pub path: Option<PathBuf>,
    /// The journal of the run before this one (`crate::crash`).
    pub previous: Option<PathBuf>,
    /// The newest entries, numbered from 1 (`seq`).
    entries: VecDeque<Value>,
    seq: u64,
    /// The project's last reading, and whether it may have changed since.
    reading: Option<Value>,
    /// Each pattern's notes as last read (by pattern id).
    notes: HashMap<u32, Vec<os_model::Note>>,
    dirty: bool,
    read_at: Option<Instant>,
    /// The history as last seen: (steps, done).
    history: (usize, usize),
    /// Each plugin's state as last written (by owner id), the instances whose windows were open, and each one's
    /// attention count when last read.
    plugin_hash: HashMap<u32, u64>,
    open: Vec<usize>,
    attention: HashMap<usize, u64>,
    /// Instances to read at the next plugin tick: the audio thread had them when they were due.
    retry: Vec<usize>,
    /// The history a Lemon project file brought (`crate::native`), and that file; the number of this session's entry
    /// from which on its entries are the project's (its `opened` or `new`).
    carried: Vec<Value>,
    carried_from: Option<PathBuf>,
    project_from: u64,
    /// The plugin states the project's history holds already (by hash): not written again.
    known: std::collections::HashSet<String>,
    /// This session's `session` entry is in `carried` already (the project was saved into a Lemon project file).
    session_carried: bool,
    plugins_at: Option<Instant>,
}

impl Journal {
    /// A journal for this session: a new file in the user folder's `Journal` (none in tests).
    pub fn start() -> Journal {
        let mut j = Journal::default();
        if !cfg!(test) {
            if let Some(dir) = os_paths::user_dir().map(|d| d.join("Journal")) {
                prune(&dir);
                j.previous = newest(&dir);
                let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let path = dir.join(format!("{stamp}-{}.jsonl", std::process::id()));
                match std::fs::create_dir_all(&dir).and_then(|_| File::create(&path)) {
                    Ok(f) => {
                        j.file = Some(BufWriter::new(f));
                        j.path = Some(path);
                    }
                    Err(e) => eprintln!("lemonstudio: journal: {e}"),
                }
            }
        }
        j.write("session", json!({ "version": env!("CARGO_PKG_VERSION"), "pid": std::process::id() }));
        j
    }

    /// Appends an entry of `kind` with `fields`, and flushes it to disk.
    pub fn write(&mut self, kind: &str, fields: Value) {
        self.seq += 1;
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let mut e = json!({ "seq": self.seq, "t": t, "kind": kind });
        if let (Some(o), Value::Object(f)) = (e.as_object_mut(), fields) {
            o.extend(f);
        }
        if let Some(w) = &mut self.file {
            let ok = serde_json::to_writer(&mut *w, &e).is_ok() && w.write_all(b"\n").is_ok() && w.flush().is_ok();
            if !ok {
                eprintln!("lemonstudio: journal: writing failed; the journal stops");
                self.file = None;
            }
        }
        self.entries.push_back(e);
        if self.entries.len() > MEMORY {
            self.entries.pop_front();
        }
    }

    /// The project's whole history, as a Lemon project file keeps it: what its file brought, then this session's start
    /// and its entries since the project was opened or made (from the journal's file, which holds them all).
    pub fn project_history(&self) -> Vec<Value> {
        let mine = |e: &Value| e["seq"].as_u64().is_some_and(|n| n >= self.project_from);
        let session = self.entries.iter().find(|e| e["kind"] == "session").cloned().filter(|_| !self.session_carried);
        let from_file = self.path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| t.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(mine).collect::<Vec<_>>());
        let here = from_file.unwrap_or_else(|| self.entries.iter().filter(|e| mine(e)).cloned().collect());
        self.carried.iter().cloned().chain(session).chain(here).collect()
    }

    /// The project's file is Lemon project file `path` with history `entries` (an earlier save or a backup of it was
    /// opened as the project): the history goes on from that file's.
    pub fn carry(&mut self, path: &std::path::Path, entries: Vec<Value>) {
        self.known.extend(entries.iter().filter(|e| e["kind"] == "plugin_state").filter_map(|e| e["hash"].as_str().map(str::to_string)));
        self.carried = entries;
        self.carried_from = Some(path.to_path_buf());
    }

    /// The history carried in is now project file `path`'s (a backup of it was opened as the project).
    pub fn carried_by(&mut self, path: &std::path::Path) {
        if self.carried_from.is_some() {
            self.carried_from = Some(path.to_path_buf());
        }
    }

    /// The project was saved into Lemon project file `path`: that file holds the history so far.
    pub fn saved_into(&mut self, path: &std::path::Path) {
        self.carried = self.project_history();
        self.carried_from = Some(path.to_path_buf());
        self.project_from = self.seq + 1;
        self.session_carried = true;
    }

    /// The entries after `since`, oldest first, and the newest number.
    pub fn since(&self, since: u64) -> (Vec<Value>, u64) {
        (self.entries.iter().filter(|e| e["seq"].as_u64().is_some_and(|n| n > since)).cloned().collect(), self.seq)
    }

    /// The project changed (an edit, an undo): its reading is compared at the next tick.
    pub fn touch(&mut self) {
        self.dirty = true;
    }
}

impl App {
    /// The project was opened from `path` (None: made new), with the history its file brought (a Lemon project's): the
    /// journal says so and reads it afresh.
    pub(crate) fn journal_opened(&mut self, path: Option<&std::path::Path>, carried: Vec<Value>) {
        let j = &mut self.journal;
        match path {
            Some(p) => j.write("opened", json!({ "path": p.display().to_string(), "history": carried.len() })),
            None => j.write("new", json!({})),
        }
        j.project_from = j.seq;
        j.carried_from = (!carried.is_empty()).then(|| path.map(std::path::Path::to_path_buf)).flatten();
        j.known = carried.iter().filter(|e| e["kind"] == "plugin_state").filter_map(|e| e["hash"].as_str().map(str::to_string)).collect();
        j.carried = carried;
        j.session_carried = false;
        j.reading = Some(crate::control::query::project(&self.state));
        j.notes = self.state.project.patterns.iter().map(|p| (p.id.0, p.notes.clone())).collect();
        j.dirty = false;
        j.history = (self.state.history.steps(), self.state.history.done());
        j.plugin_hash.clear();
    }

    /// Before a save: the project's last changes and every hosted plugin's state as saved, into the journal.
    pub(crate) fn journal_saving(&mut self) {
        self.journal_tick_project();
        let keys: Vec<usize> = self.plugins.hosted.keys().copied().collect();
        for key in keys {
            if !self.journal_plugin(key) {
                self.journal.retry.push(key);
            }
        }
    }

    /// The project was saved to `path`.
    pub(crate) fn journal_saved(&mut self, path: &std::path::Path) {
        self.journal.write("saved", json!({ "path": path.display().to_string() }));
    }

    /// The app closes normally (`crate::crash` reads a run without this as one that ended otherwise).
    pub(crate) fn journal_closed(&mut self) {
        self.journal.write("closed", json!({}));
    }

    /// A copy of the project was written to `path` (`App::save_copy`: a backup, a zipped project...).
    pub(crate) fn journal_copied(&mut self, path: &std::path::Path) {
        self.journal.write("copied", json!({ "path": path.display().to_string() }));
    }

    /// Each tick: undo steps, the project's changes and the edited plugins' states into the journal.
    pub(crate) fn journal_tick(&mut self) {
        let h = &self.state.history;
        let now = (h.steps(), h.done());
        let was = self.journal.history;
        if now != was {
            let name = h.newest_done().unwrap_or("").to_string();
            let (kind, name) = if now.0 == was.0 && now.1 + 1 == was.1 {
                ("undo", h.name_of(was.1).unwrap_or("").to_string())
            } else if now.0 == was.0 && now.1 == was.1 + 1 {
                ("redo", name)
            } else {
                ("step", name)
            };
            self.journal.write(kind, json!({ "name": name, "done": now.1 }));
            self.journal.history = now;
            self.journal.dirty = true;
        }
        let due = self.journal.read_at.is_none_or(|t| t.elapsed() >= PROJECT_EVERY);
        if self.journal.dirty && due {
            self.journal_tick_project();
        }
        if self.journal.plugins_at.is_none_or(|t| t.elapsed() >= PLUGIN_EVERY) {
            self.journal.plugins_at = Some(Instant::now());
            self.journal_tick_plugins();
        }
    }

    /// The project's changes since its last reading.
    fn journal_tick_project(&mut self) {
        let j = &mut self.journal;
        j.dirty = false;
        j.read_at = Some(Instant::now());
        let now = crate::control::query::project(&self.state);
        if let Some(was) = &j.reading {
            let changes = crate::control::query::changes(was, &now);
            if !changes.is_empty() {
                j.write("changes", json!({ "changes": changes }));
            }
        }
        j.reading = Some(now);
        let p = &self.state.project;
        for q in &p.patterns {
            let was = j.notes.get(&q.id.0);
            if was.is_some_and(|w| *w == q.notes) {
                continue;
            }
            let was = was.cloned().unwrap_or_default();
            let (added, removed) = (minus(&q.notes, &was), minus(&was, &q.notes));
            if !added.is_empty() || !removed.is_empty() {
                let json = |v: Vec<os_model::Note>| v.iter().map(|n| note_json(p, n)).collect::<Vec<_>>();
                j.write("notes", json!({ "pattern": q.id.0, "number": q.number, "added": json(added), "removed": json(removed) }));
            }
            j.notes.insert(q.id.0, q.notes.clone());
        }
    }

    /// The plugins whose windows are open and that said something since they were last read (a parameter moved, a
    /// preset loaded), and those whose windows just closed: their states, when changed. A plugin is locked only then,
    /// and only when free: the audio thread waits for an instance the UI has, so one it has is read next time.
    fn journal_tick_plugins(&mut self) {
        let open = self.editing_plugins();
        let closed: Vec<usize> = self.journal.open.iter().copied().filter(|k| !open.contains(k)).collect();
        let mut read = std::mem::take(&mut self.journal.retry);
        read.extend(closed);
        for &key in &open {
            let count = self.plugins.hosted.get(&key).and_then(|h| h.attention_count());
            // One that never asks for attention is read each time.
            if count.is_none() || self.journal.attention.get(&key) != count.as_ref() {
                read.push(key);
            }
            if let Some(c) = count {
                self.journal.attention.insert(key, c);
            }
        }
        read.sort_unstable();
        read.dedup();
        for key in read {
            if !self.journal_plugin(key) {
                self.journal.retry.push(key);
            }
        }
        self.journal.open = open;
    }

    /// The hosted instances whose windows are open (the plugins a user edits).
    fn editing_plugins(&self) -> Vec<usize> {
        #[cfg(plugin_windows)]
        return self.plugins.hosted.iter().filter(|(_, h)| h.window.is_some()).map(|(&k, _)| k).collect();
        #[cfg(not(plugin_windows))]
        Vec::new()
    }

    /// The state of the instance under `key`, into the journal when it differs from the last written. False when the
    /// audio thread had the instance: not read.
    pub(crate) fn journal_plugin(&mut self, key: usize) -> bool {
        let Some(owner) = crate::control::command::key_owner(&self.state.project, key) else { return true };
        let Ok(state) = self.plugins.state_if_free(key) else { return false };
        let Some(state) = state else { return true };
        let hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            state.hash(&mut h);
            h.finish()
        };
        let was = self.journal.plugin_hash.insert(owner, hash);
        if was == Some(hash) {
            return true;
        }
        // Changed since it was first read after the project opened: an edit in the plugin's own window.
        self.state.plugin_edited |= was.is_some();
        if !self.journal.known.insert(format!("{hash:016x}")) {
            return true;
        }
        let plugin = self.plugins.descriptor(key).map(|d| json!({ "name": d.name, "format": d.format.as_str(), "id": d.id }));
        let place = match crate::plugins::fx_of(key) {
            None => json!({ "channel": self.state.project.channels.get(key).map(|c| c.name.clone()) }),
            Some((t, k)) => json!({ "track": t, "slot": k }),
        };
        let data = (state.len() <= STATE_MAX).then(|| base64(&state));
        self.journal.write("plugin_state", json!({ "owner": owner, "at": place, "plugin": plugin, "size": state.len(), "hash": format!("{hash:016x}"), "state": data }));
        true
    }
}

/// A plugin state found in the journals (`states_of`): where it is (the journal file and the entry's number), when it
/// was written (Unix ms), the channel or slot it played and its hash.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub file: PathBuf,
    pub seq: u64,
    pub t: u64,
    pub at: String,
    pub hash: String,
}

impl Journal {
    /// The states plugin `name` had in the journals, newest first, each distinct state once: this session's, then the
    /// earlier sessions' (their files, newest first). At most `max`.
    pub fn states_of(&self, name: &str, max: usize) -> Vec<Found> {
        let mut out: Vec<Found> = Vec::new();
        let take = |file: &std::path::Path, e: &Value, out: &mut Vec<Found>| {
            if e["kind"] != "plugin_state" || e["plugin"]["name"] != name || !e["state"].is_string() {
                return;
            }
            let hash = e["hash"].as_str().unwrap_or("").to_string();
            if out.iter().any(|f| f.hash == hash) {
                return;
            }
            let at = e["at"]["channel"].as_str().map(str::to_string).or_else(|| e["at"]["track"].as_u64().map(|t| format!("bus {t}"))).unwrap_or_default();
            out.push(Found { file: file.to_path_buf(), seq: e["seq"].as_u64().unwrap_or(0), t: e["t"].as_u64().unwrap_or(0), at, hash });
        };
        let here = self.path.clone().unwrap_or_default();
        for e in self.entries.iter().rev() {
            take(&here, e, &mut out);
        }
        // The project file's own history: an entry is known by its place in it.
        if let Some(from) = &self.carried_from {
            for (i, e) in self.carried.iter().enumerate().rev() {
                let n = out.len();
                take(from, e, &mut out);
                if out.len() > n {
                    out[n].seq = i as u64;
                }
            }
        }
        for file in older_journals(self.path.as_deref()) {
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            let mut found: Vec<Value> = text.lines().filter(|l| l.contains("\"plugin_state\"") && l.contains(name)).filter_map(|l| serde_json::from_str(l).ok()).collect();
            found.reverse();
            for e in &found {
                take(&file, e, &mut out);
            }
            if out.len() >= max {
                break;
            }
        }
        out.truncate(max);
        out
    }

    /// The state written as entry `seq` of journal `file` (this session's when it is this journal's file).
    pub fn state_at(&self, file: &std::path::Path, seq: u64) -> Option<Vec<u8>> {
        let e = if self.carried_from.as_deref() == Some(file) {
            self.carried.get(seq as usize).cloned()?
        } else if self.path.as_deref() == Some(file) || file.as_os_str().is_empty() {
            self.entries.iter().find(|e| e["seq"] == seq).cloned()?
        } else {
            let text = std::fs::read_to_string(file).ok()?;
            text.lines().filter(|l| l.contains("\"plugin_state\"")).filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|e| e["seq"] == seq)?
        };
        unbase64(e["state"].as_str()?)
    }
}

impl App {
    /// Puts back the state written as entry `seq` of journal `file` into the instance under `key` (Recover from
    /// journal). Returns the status message.
    pub(crate) fn recover_state(&mut self, key: usize, file: &std::path::Path, seq: u64) -> String {
        let Some(name) = self.plugins.descriptor(key).map(|d| d.name) else { return "No plugin there".into() };
        let Some(state) = self.journal.state_at(file, seq) else { return format!("That {name} state is no longer in the journal") };
        if !self.plugins.set_state(key, &state) {
            return format!("{name} did not take the state");
        }
        self.plugins.mark_edited(key);
        self.state.params.reread(key);
        if !self.journal_plugin(key) {
            self.journal.retry.push(key);
        }
        self.invalidate();
        format!("{name} recovered from the journal")
    }
}

/// The journal files other than `current`, newest first.
fn older_journals(current: Option<&std::path::Path>) -> Vec<PathBuf> {
    if cfg!(test) {
        return Vec::new();
    }
    let Some(dir) = os_paths::user_dir().map(|d| d.join("Journal")) else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<(SystemTime, PathBuf)> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl") && Some(p.as_path()) != current)
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().map(|(_, p)| p).collect()
}

/// A journal time as a menu shows it: "Today 7:48", "Oct 27 23:57".
pub fn when(t: u64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_millis_opt(t as i64).single() else { return String::new() };
    if at.date_naive() == Local::now().date_naive() { at.format("Today %-H:%M").to_string() } else { at.format("%b %-d %-H:%M").to_string() }
}

/// The notes of `a` that `b` has fewer of (as many times as `a` has them more).
fn minus(a: &[os_model::Note], b: &[os_model::Note]) -> Vec<os_model::Note> {
    let key = |n: &os_model::Note| (n.position, n.key, n.channel, n.length, n.velocity, n.pan, n.fine_pitch, n.release, n.mod_x, n.mod_y, n.flags, (n.group, n.unk14, n.repeat));
    let (mut a, mut b) = (a.to_vec(), b.to_vec());
    a.sort_by_key(key);
    b.sort_by_key(key);
    let (mut out, mut k) = (Vec::new(), 0);
    for n in a {
        while k < b.len() && key(&b[k]) < key(&n) {
            k += 1;
        }
        if k < b.len() && b[k] == n {
            k += 1;
        } else {
            out.push(n);
        }
    }
    out
}

/// A note with every stored field; its channel by id.
fn note_json(p: &os_model::Project, n: &os_model::Note) -> Value {
    let ch = p.channels.iter().find(|c| c.index == n.channel).map(|c| c.id.0);
    json!({
        "channel": ch, "position": n.position, "length": n.length, "key": n.key, "velocity": n.velocity, "pan": n.pan,
        "fine_pitch": n.fine_pitch, "release": n.release, "mod_x": n.mod_x, "mod_y": n.mod_y, "flags": n.flags,
        "group": n.group, "repeat": n.repeat,
    })
}

/// Removes journals older than [`KEEP_DAYS`] from `dir`.
/// The journal in `dir` written last.
fn newest(dir: &std::path::Path) -> Option<PathBuf> {
    let modified = |e: &std::fs::DirEntry| e.metadata().and_then(|m| m.modified()).ok();
    std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "jsonl")).filter_map(|e| Some((modified(&e)?, e.path()))).max().map(|(_, p)| p)
}

fn prune(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let keep = Duration::from_secs(KEEP_DAYS * 86_400);
    for e in rd.flatten() {
        let p = e.path();
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > keep);
        if old && p.extension().is_some_and(|x| x == "jsonl") {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Standard base64 with padding.
pub fn base64(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `base64` read back.
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    let val = |b: u8| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &b in s {
        acc = (acc << 6) | u32::from(val(b)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for n in 0..10 {
            let d: Vec<u8> = (0..n).map(|i| (i * 37 + 5) as u8).collect();
            assert_eq!(unbase64(&base64(&d)).as_deref(), Some(d.as_slice()));
        }
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }

    /// The journal follows the project: an edit's undo step and changes, its undo, and a save; a plugin's state as it
    /// changes while its window is open.
    #[test]
    fn the_journal_follows_the_project() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.journal_opened(None, Vec::new());
        let kinds = |a: &App, from: u64| a.journal.since(from).0.iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let start = a.journal.since(0).1;
        // A tempo change in the interface: a step and its change.
        crate::control::edit(&mut a.state, crate::control::command::TEMPO, crate::control::command::Region::Settings, |s| s.tempo = 140.0);
        a.journal.touch();
        a.journal.read_at = None;
        a.journal_tick();
        let (es, _) = a.journal.since(start);
        assert_eq!(kinds(&a, start), ["step", "changes"]);
        assert_eq!(es[0]["name"], "Tempo");
        assert!(es[1]["changes"].as_array().unwrap().iter().any(|c| c["path"] == "tempo" && c["after"] == 140.0), "{}", es[1]);
        // Undone: an undo and the change back.
        let mark = a.journal.since(0).1;
        crate::history::undo(&mut a.state);
        a.journal.read_at = None;
        a.journal_tick();
        assert_eq!(kinds(&a, mark), ["undo", "changes"]);
        assert_eq!(a.journal.since(mark).0[0]["name"], "Tempo");
        // A note added, then moved: one added; then one removed and one added, with every field.
        let mark = a.journal.since(0).1;
        let n = os_model::Note { position: 0, length: 24, channel: 0, key: 60, velocity: 100, pan: 64, fine_pitch: 120, mod_x: 128, mod_y: 128, release: 64, flags: 0x4000, group: 0, unk14: 0, repeat: 0 };
        crate::patterns::pat_mut(&mut a.state, 1).notes.push(n);
        a.journal.touch();
        a.journal.read_at = None;
        a.journal_tick();
        let notes: Vec<Value> = a.journal.since(mark).0.into_iter().filter(|e| e["kind"] == "notes").collect();
        assert_eq!(notes.len(), 1);
        assert_eq!((notes[0]["added"].as_array().unwrap().len(), notes[0]["removed"].as_array().unwrap().len()), (1, 0));
        assert_eq!(notes[0]["added"][0]["fine_pitch"], 120);
        let mark = a.journal.since(0).1;
        crate::patterns::pat_mut(&mut a.state, 1).notes.iter_mut().find(|m| **m == n).unwrap().position = 48;
        a.journal.touch();
        a.journal.read_at = None;
        a.journal_tick();
        let notes: Vec<Value> = a.journal.since(mark).0.into_iter().filter(|e| e["kind"] == "notes").collect();
        assert_eq!(notes[0]["added"][0]["position"], 48);
        assert_eq!(notes[0]["removed"][0]["position"], 0);
        assert_eq!(notes[0]["removed"].as_array().unwrap().len(), 1, "only the moved note");
        // A plugin's state, written once while it stays the same.
        a.plugins.available = crate::plugins::builtin();
        a.plugins.create_with(0, a.plugins.index_of(crate::citrus::CITRUS).unwrap(), None).unwrap();
        let mark = a.journal.since(0).1;
        a.journal_plugin(0);
        a.journal_plugin(0);
        let (es, _) = a.journal.since(mark);
        assert_eq!(es.len(), 1, "once while unchanged");
        assert_eq!(es[0]["kind"], "plugin_state");
        let st = unbase64(es[0]["state"].as_str().unwrap()).unwrap();
        assert_eq!(Some(st), a.plugins.state(0), "the whole state, readable back");
        // Changed, then recovered from the journal: the state it had.
        let first = a.plugins.state(0).unwrap();
        let p = a.plugins.param_infos(0).into_iter().find(|p| !p.read_only && !p.bypass && p.max > p.min).unwrap();
        let v = if (p.default - p.min).abs() > 1e-6 { p.min } else { p.max };
        assert!(a.plugins.set_param_named(0, &p.name, v));
        a.plugins.hosted[&0].plugin.lock().unwrap().idle();
        assert_ne!(a.plugins.state(0).as_ref(), Some(&first), "changed");
        a.journal_plugin(0);
        let name = a.plugins.descriptor(0).unwrap().name;
        let found = a.journal.states_of(&name, 10);
        assert_eq!(found.len(), 2, "both states, newest first");
        let msg = a.recover_state(0, &found[1].file, found[1].seq);
        assert!(msg.ends_with("recovered from the journal"), "{msg}");
        assert_eq!(a.plugins.state(0), Some(first), "the first state back");
    }
}
