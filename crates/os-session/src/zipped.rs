// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Zipped projects: a project file and its samples in one .zip, as FL's zipped loop packages are (the project file and
//! the samples side by side at the top of the zip). Opening a .zip unzips it into a folder beside it, then opens its
//! project file: the samples are found by name in the project's folder (`crate::missing`). FILE > Export zipped
//! project... writes the project and every sample it uses into a .zip. Both run on a thread of their own behind a
//! progress panel.
//!
// spec: ui/zipped-projects.md

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;


/// Where a zip is unzipped when its own folder cannot be written to (a read-only drive, say): this folder in the user
/// folder.
pub const UNZIP_DIR: &str = "Unzipped projects";
/// Files already compressed are stored as they are.
const STORED: &[&str] = &["mp3", "ogg", "flac", "m4a", "aac", "opus", "zip"];

/// A zip being unzipped or written: the panel's title and file, and the bytes done of the total.
pub struct Zipping {
    pub rx: Receiver<Result<Done, String>>,
    pub title: &'static str,
    pub name: String,
    pub done: Arc<AtomicU64>,
    pub total: Arc<AtomicU64>,
}

pub enum Done {
    /// The project file unzipped.
    Unzipped(PathBuf),
    /// The zip written, how many samples it holds, and the samples not found (not in it).
    Zipped { zip: PathBuf, samples: usize, missing: Vec<String> },
}

/// The file name of a stored sample path (either slash).
pub fn file_name(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

fn is_project(p: &Path) -> bool {
    crate::project_files::is_project(p)
}

// --- Opening a zip ---

/// A file in the zip: its index, its path inside the zip, its size.
type Entry = (usize, PathBuf, u64);

/// Upper bounds on an untrusted zip project archive: entry count and total extracted bytes (2 GiB).
const MAX_ENTRIES: usize = 10_000;
const MAX_UNZIP_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Unzips `zip` into a folder beside it named after it (`unzip_into`), else under `fallback`; returns the project file:
/// the one nearest the top of the zip (then by name). Names that would leave the folder are skipped.
pub fn unzip(zip: &Path, fallback: Option<&Path>, done: &AtomicU64, total: &AtomicU64) -> Result<PathBuf, String> {
    unzip_with_limits(zip, fallback, done, total, MAX_ENTRIES, MAX_UNZIP_BYTES)
}

fn unzip_with_limits(
    zip: &Path,
    fallback: Option<&Path>,
    done: &AtomicU64,
    total: &AtomicU64,
    max_entries: usize,
    max_bytes: u64,
) -> Result<PathBuf, String> {
    let mut a = zip::ZipArchive::new(io::BufReader::new(File::open(zip).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    if a.len() > max_entries {
        return Err(format!("the zip holds too many files (more than {max_entries})"));
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut declared_total: u64 = 0;
    for i in 0..a.len() {
        let f = a.by_index_raw(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        if let Some(rel) = f.enclosed_name() {
            let size = f.size();
            declared_total = declared_total.checked_add(size).ok_or_else(|| "the zip is too large to unzip".to_string())?;
            if declared_total > max_bytes {
                return Err("the zip is too large to unzip".to_string());
            }
            entries.push((i, rel, size));
        }
    }
    let project = entries
        .iter()
        .filter(|e| is_project(&e.1))
        .min_by_key(|e| (e.1.components().count(), e.1.to_string_lossy().to_lowercase()))
        .map(|e| e.1.clone())
        .ok_or_else(|| "it holds no FLP project".to_string())?;
    total.store(declared_total, Ordering::Relaxed);
    let stem = zip.file_stem().map_or_else(|| "Project".to_string(), |s| s.to_string_lossy().into_owned());
    let beside = zip.parent().map(|d| if d.as_os_str().is_empty() { Path::new(".") } else { d });
    let mut first_error = None;
    for base in beside.into_iter().chain(fallback) {
        let Some(dest) = folder_for(base, &stem, &entries, &project) else { continue };
        done.store(0, Ordering::Relaxed);
        match unzip_into(&mut a, &entries, &dest, done, max_bytes) {
            Ok(()) => return Ok(dest.join(&project)),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "no folder to unzip it into".into()))
}

/// The folder under `base` the zip goes into: `<stem>`, else `<stem> (2)` and on. One that exists is used again when
/// it is this zip unzipped before (it has the project file, and no file of the zip's with another size).
fn folder_for(base: &Path, stem: &str, entries: &[Entry], project: &Path) -> Option<PathBuf> {
    (1..100).map(|n| if n == 1 { base.join(stem) } else { base.join(format!("{stem} ({n})")) }).find(|d| {
        if !d.exists() {
            return true;
        }
        let same = |rel: &Path, size: u64| std::fs::metadata(d.join(rel)).is_ok_and(|m| m.is_file() && m.len() == size);
        let unzipped = entries.iter().any(|e| e.1 == project && same(&e.1, e.2));
        unzipped && entries.iter().all(|(_, rel, size)| !d.join(rel).exists() || same(rel, *size))
    })
}

/// Writes the zip's files into `dest`; a file already there with the same size is kept. Each file is written beside
/// its place, then moved in, so a stopped unzip leaves no part file under a real name.
fn unzip_into<R: Read + io::Seek>(
    a: &mut zip::ZipArchive<R>,
    entries: &[Entry],
    dest: &Path,
    done: &AtomicU64,
    max_bytes: u64,
) -> Result<(), String> {
    let err = |p: &Path, e: &dyn std::fmt::Display| format!("{}: {e}", p.display());
    std::fs::create_dir_all(dest).map_err(|e| err(dest, &e))?;
    let mut extracted: u64 = 0;
    for (i, rel, size) in entries {
        let out = dest.join(rel);
        if std::fs::metadata(&out).is_ok_and(|m| m.is_file() && m.len() == *size) {
            extracted = extracted.checked_add(*size).ok_or_else(|| err(&out, &"unzipped data exceeds size limit"))?;
            if extracted > max_bytes {
                return Err(err(&out, &"unzipped data exceeds size limit"));
            }
            done.fetch_add(*size, Ordering::Relaxed);
            continue;
        }
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| err(d, &e))?;
        }
        let mut part = out.clone().into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        let remaining = max_bytes.saturating_sub(extracted);
        let mut f = a.by_index(*i).map_err(|e| err(rel, &e))?;
        let written = File::create(&part)
            .and_then(|w| copy_limited(&mut f, io::BufWriter::new(w), done, remaining))
            .and_then(|n| std::fs::rename(&part, &out).map(|()| n));
        match written {
            Ok(n) => extracted = extracted.saturating_add(n),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(err(&out, &e));
            }
        }
    }
    Ok(())
}

/// Copies `r` into `w`, counting the bytes into `done`.
fn copy(r: &mut impl Read, mut w: impl Write, done: &AtomicU64) -> io::Result<()> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        w.write_all(&buf[..n])?;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    w.flush()
}

/// Copies up to `limit` bytes from `r` into `w` using `Read::take(limit + 1)`, failing if the stream yields more bytes
/// than `limit` (protects against zip bombs and spoofed uncompressed-size headers).
fn copy_limited(r: &mut impl Read, mut w: impl Write, done: &AtomicU64, limit: u64) -> io::Result<u64> {
    let mut limited = r.take(limit.saturating_add(1));
    let mut buf = vec![0u8; 256 * 1024];
    let mut copied: u64 = 0;
    loop {
        let n = match limited.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        copied = copied.saturating_add(n as u64);
        if copied > limit {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "unzipped data exceeds size limit"));
        }
        w.write_all(&buf[..n])?;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    w.flush()?;
    Ok(copied)
}

// --- Writing a zip ---

/// What goes into the zip besides the project file: each sample file once, under the name it has in the zip.
#[derive(Default)]
pub struct Pack {
    /// The project file's name in the zip (no sample takes it).
    pub project: String,
    pub files: Vec<(PathBuf, String)>,
    /// Samples the project names that were not found (file names).
    pub missing: Vec<String>,
}

impl Pack {
    /// Takes the samples of the project being written; a sample renamed in the zip (two files of one name) gets that
    /// name in the project file too, so it is found by name once unzipped.
    pub fn gather(&mut self, st: &crate::settings_file::SettingsStore, p: &mut os_model::Project) {
        for ch in &mut p.channels {
            let Some(stored) = ch.sample_path.clone().filter(|s| !s.trim().is_empty()) else { continue };
            let real = PathBuf::from(crate::plugins::resolve_sample(st, &stored));
            if !real.is_file() {
                self.missing.push(file_name(&stored).to_string());
                continue;
            }
            let name = match self.files.iter().find(|(f, _)| *f == real) {
                Some((_, n)) => n.clone(),
                None => {
                    let n = self.free_name(&real.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()));
                    self.files.push((real, n.clone()));
                    n
                }
            };
            let old = file_name(&stored);
            if old != name {
                ch.sample_path = Some(format!("{}{name}", &stored[..stored.len() - old.len()]));
            }
        }
    }

    /// `name`, else `<stem>_2.<ext>` and on, so no two files in the zip share a name (case does not count).
    fn free_name(&self, name: &str) -> String {
        let taken = |n: &str| n.eq_ignore_ascii_case(&self.project) || self.files.iter().any(|(_, f)| f.eq_ignore_ascii_case(n));
        if !taken(name) {
            return name.to_string();
        }
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
            _ => (name, String::new()),
        };
        (2..).map(|n| format!("{stem}_{n}{ext}")).find(|n| !taken(n)).expect("a free name")
    }
}

/// Writes the zip: the project file, then the samples, side by side at its top. Written beside `zip`, then moved in.
pub fn write_zip(zip: &Path, flp: &[u8], pack: &Pack, done: &AtomicU64, total: &AtomicU64) -> Result<(), String> {
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;
    let sizes: Vec<u64> = pack.files.iter().map(|(f, _)| std::fs::metadata(f).map_or(0, |m| m.len())).collect();
    total.store(flp.len() as u64 + sizes.iter().sum::<u64>(), Ordering::Relaxed);
    let options = |name: &str, size: u64| {
        let stored = Path::new(name).extension().is_some_and(|e| STORED.iter().any(|s| e.eq_ignore_ascii_case(s)));
        SimpleFileOptions::default()
            .compression_method(if stored { CompressionMethod::Stored } else { CompressionMethod::Deflated })
            .large_file(size >= u64::from(u32::MAX))
    };
    let mut part = zip.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let write = || -> Result<(), String> {
        let mut w = zip::ZipWriter::new(io::BufWriter::new(File::create(&part).map_err(|e| e.to_string())?));
        w.start_file(pack.project.as_str(), options(&pack.project, flp.len() as u64)).map_err(|e| e.to_string())?;
        w.write_all(flp).map_err(|e| e.to_string())?;
        done.fetch_add(flp.len() as u64, Ordering::Relaxed);
        for ((file, name), size) in pack.files.iter().zip(&sizes) {
            let mut r = File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
            w.start_file(name.as_str(), options(name, *size)).map_err(|e| e.to_string())?;
            copy(&mut r, &mut w, done).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        w.finish().map_err(|e| e.to_string())?.flush().map_err(|e| e.to_string())?;
        std::fs::rename(&part, zip).map_err(|e| e.to_string())
    };
    let r = write();
    if r.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    r
}

// --- Both ---

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("os_zipped_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_zip(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(File::create(path).unwrap());
        for (n, b) in files {
            w.start_file(*n, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b).unwrap();
        }
        w.finish().unwrap();
    }

    fn names(zip: &Path) -> Vec<String> {
        let mut a = zip::ZipArchive::new(File::open(zip).unwrap()).unwrap();
        (0..a.len()).map(|i| a.by_index(i).unwrap().name().unwrap().to_string()).collect()
    }

    /// A zip unzips beside itself and the project nearest its top is the one opened; names leaving the folder are
    /// skipped; unzipping it again uses the same folder, and a folder holding other files of those names is not used.
    #[test]
    fn unzips_beside_the_zip() {
        let d = temp("unzip");
        let zip = d.join("Song pack.zip");
        make_zip(&zip, &[("old/Backup.flp", b"b"), ("Song.flp", b"song"), ("Kick.wav", b"kick"), ("../evil.txt", b"x")]);
        let (done, total) = (AtomicU64::new(0), AtomicU64::new(0));
        let project = unzip(&zip, None, &done, &total).unwrap();
        assert_eq!(project, d.join("Song pack").join("Song.flp"));
        assert_eq!(std::fs::read(d.join("Song pack/Kick.wav")).unwrap(), b"kick");
        assert!(!d.join("evil.txt").exists());
        assert_eq!(done.load(Ordering::Relaxed), total.load(Ordering::Relaxed));
        assert_eq!(unzip(&zip, None, &done, &total).unwrap(), project, "unzipped before: the same folder");
        std::fs::write(d.join("Song pack/Kick.wav"), b"another kick").unwrap();
        assert_eq!(unzip(&zip, None, &done, &total).unwrap(), d.join("Song pack (2)").join("Song.flp"));
        make_zip(&d.join("empty.zip"), &[("a.wav", b"a")]);
        assert!(unzip(&d.join("empty.zip"), None, &done, &total).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Two samples of one name: the second is renamed, in the zip and in its project file.
    #[test]
    fn names_in_the_zip_are_unique() {
        let mut p = Pack { project: "Song.flp".into(), ..Default::default() };
        p.files.push((PathBuf::from("a/Kick.wav"), "Kick.wav".into()));
        assert_eq!(p.free_name("kick.WAV"), "kick_2.WAV");
        assert_eq!(p.free_name("Song.flp"), "Song_2.flp");
        assert_eq!(p.free_name("Snare.wav"), "Snare.wav");
    }

    /// Rejects archives with too many entries, huge declared sizes, or spoofed headers whose actual decompressed
    /// payload exceeds the extraction budget.
    #[test]
    fn rejects_zip_bombs_and_spoofed_headers() {
        let d = temp("bomb");
        let (done, total) = (AtomicU64::new(0), AtomicU64::new(0));

        // 1. Entry count cap.
        let many = d.join("many.zip");
        make_zip(&many, &[("Song.flp", b"s"), ("a.wav", b"a"), ("b.wav", b"b"), ("c.wav", b"c")]);
        assert!(unzip_with_limits(&many, None, &done, &total, 3, 1024).is_err());

        // 2. Declared size cap.
        let big = d.join("big.zip");
        let payload = vec![0u8; 4096];
        make_zip(&big, &[("Song.flp", b"s"), ("Big.wav", &payload)]);
        assert!(unzip_with_limits(&big, None, &done, &total, 10, 1024).is_err());

        // 3. Spoofed uncompressed-size header (header claims 10 bytes, actual deflate stream yields 4096 bytes).
        let spoofed = d.join("spoofed.zip");
        let mut raw = std::fs::read(&big).unwrap();
        let needle = 4096u32.to_le_bytes();
        let fake = 10u32.to_le_bytes();
        let mut patched = 0usize;
        for w in 0..raw.len().saturating_sub(4) {
            if raw[w..w + 4] == needle {
                raw[w..w + 4].copy_from_slice(&fake);
                patched += 1;
            }
        }
        assert!(patched >= 2, "patched local and central header uncompressed sizes");
        std::fs::write(&spoofed, &raw).unwrap();
        assert!(unzip_with_limits(&spoofed, None, &done, &total, 10, 1024).is_err());
        assert!(!d.join("spoofed/Big.wav").exists(), "oversized file was not finalized");
        assert!(!d.join("spoofed/Big.wav.part").exists(), "partial bomb file was cleaned up");

        // 4. Direct stream enforcement via Read::take(limit + 1).
        let mut out = Vec::new();
        let err = copy_limited(&mut io::Cursor::new(&payload), &mut out, &done, 1024).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(out.len() <= 1024);
        let _ = std::fs::remove_dir_all(&d);
    }
}
