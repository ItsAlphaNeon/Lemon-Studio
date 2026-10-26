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

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use crate::app::App;
use crate::dialog::{spans, Kind, Then};

/// Where a zip is unzipped when its own folder cannot be written to (a read-only drive, say): this folder in the user
/// folder.
const UNZIP_DIR: &str = "Unzipped projects";
/// Files already compressed are stored as they are.
const STORED: &[&str] = &["mp3", "ogg", "flac", "m4a", "aac", "opus", "zip"];

/// A zip being unzipped or written: the panel's title and file, and the bytes done of the total.
pub struct Zipping {
    rx: Receiver<Result<Done, String>>,
    title: &'static str,
    name: String,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
}

enum Done {
    /// The project file unzipped.
    Unzipped(PathBuf),
    /// The zip written, how many samples it holds, and the samples not found (not in it).
    Zipped { zip: PathBuf, samples: usize, missing: Vec<String> },
}

/// The file name of a stored sample path (either slash).
fn file_name(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

fn is_project(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("flp"))
}

/// Starts `job` on a thread of its own, behind the panel.
fn run(app: &mut App, title: &'static str, file: &Path, job: impl FnOnce(&AtomicU64, &AtomicU64) -> Result<Done, String> + Send + 'static) {
    let (tx, rx) = channel();
    let (done, total) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
    let (d, t) = (done.clone(), total.clone());
    let spawned = std::thread::Builder::new().name("zipped-project".into()).spawn(move || {
        let _ = tx.send(job(&d, &t));
    });
    if let Err(e) = spawned {
        app.state.message = Some(format!("{title} failed: {e}"));
        return;
    }
    let name = file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    app.zipping = Some(Zipping { rx, title, name, done, total });
    app.invalidate();
}

// --- Opening a zip ---

/// Opens a zipped project: unzips it (`unzip`), then opens the project file in it.
pub fn import(app: &mut App, zip: PathBuf) {
    if app.zipping.is_some() {
        return;
    }
    let fallback = crate::app::theme_dir().map(|d| d.join(UNZIP_DIR));
    let z = zip.clone();
    run(app, "Unzipping project", &zip, move |done, total| unzip(&z, fallback.as_deref(), done, total).map(Done::Unzipped));
}

/// A file in the zip: its index, its path inside the zip, its size.
type Entry = (usize, PathBuf, u64);

/// Unzips `zip` into a folder beside it named after it (`unzip_into`), else under `fallback`; returns the project file:
/// the one nearest the top of the zip (then by name). Names that would leave the folder are skipped.
pub fn unzip(zip: &Path, fallback: Option<&Path>, done: &AtomicU64, total: &AtomicU64) -> Result<PathBuf, String> {
    let mut a = zip::ZipArchive::new(io::BufReader::new(File::open(zip).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    let mut entries: Vec<Entry> = Vec::new();
    for i in 0..a.len() {
        let f = a.by_index_raw(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        if let Some(rel) = f.enclosed_name() {
            entries.push((i, rel, f.size()));
        }
    }
    let project = entries
        .iter()
        .filter(|e| is_project(&e.1))
        .min_by_key(|e| (e.1.components().count(), e.1.to_string_lossy().to_lowercase()))
        .map(|e| e.1.clone())
        .ok_or_else(|| "it holds no FLP project".to_string())?;
    total.store(entries.iter().map(|e| e.2).sum(), Ordering::Relaxed);
    let stem = zip.file_stem().map_or_else(|| "Project".to_string(), |s| s.to_string_lossy().into_owned());
    let beside = zip.parent().map(|d| if d.as_os_str().is_empty() { Path::new(".") } else { d });
    let mut first_error = None;
    for base in beside.into_iter().chain(fallback) {
        let Some(dest) = folder_for(base, &stem, &entries, &project) else { continue };
        done.store(0, Ordering::Relaxed);
        match unzip_into(&mut a, &entries, &dest, done) {
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
fn unzip_into<R: Read + io::Seek>(a: &mut zip::ZipArchive<R>, entries: &[Entry], dest: &Path, done: &AtomicU64) -> Result<(), String> {
    let err = |p: &Path, e: &dyn std::fmt::Display| format!("{}: {e}", p.display());
    std::fs::create_dir_all(dest).map_err(|e| err(dest, &e))?;
    for (i, rel, size) in entries {
        let out = dest.join(rel);
        if std::fs::metadata(&out).is_ok_and(|m| m.is_file() && m.len() == *size) {
            done.fetch_add(*size, Ordering::Relaxed);
            continue;
        }
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).map_err(|e| err(d, &e))?;
        }
        let mut part = out.clone().into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        let mut f = a.by_index(*i).map_err(|e| err(rel, &e))?;
        let written = File::create(&part).and_then(|w| copy(&mut f, io::BufWriter::new(w), done)).and_then(|()| std::fs::rename(&part, &out));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&part);
            return Err(err(&out, &e));
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

// --- Writing a zip ---

/// FILE > Export zipped project...: a Save dialog for the .zip, in the project's folder, named after it.
pub fn export_dialog(app: &mut App) {
    use crate::filedlg::{FileDlg, Mode};
    let path = app.state.path.clone();
    let stem = path.as_deref().and_then(Path::file_stem).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
    let dlg = FileDlg::new(Mode::Save, "Export zipped project", "zipped project", Then::ExportZip)
        .filter("Zip file", &["zip"])
        .start(path.as_deref().and_then(Path::parent))
        .name(&format!("{stem}.zip"));
    app.file_dialog(dlg);
}

/// What goes into the zip besides the project file: each sample file once, under the name it has in the zip.
#[derive(Default)]
struct Pack {
    /// The project file's name in the zip (no sample takes it).
    project: String,
    files: Vec<(PathBuf, String)>,
    /// Samples the project names that were not found (file names).
    missing: Vec<String>,
}

impl Pack {
    /// Takes the samples of the project being written; a sample renamed in the zip (two files of one name) gets that
    /// name in the project file too, so it is found by name once unzipped.
    fn gather(&mut self, p: &mut os_model::Project) {
        for ch in &mut p.channels {
            let Some(stored) = ch.sample_path.clone().filter(|s| !s.trim().is_empty()) else { continue };
            let real = PathBuf::from(crate::plugins::resolve_sample(&stored));
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

/// The project file's bytes as Save writes them (with the samples' names in the zip) and the samples to put beside it.
/// The project itself, its file and its unsaved changes stay as they are.
fn prepare(app: &mut App, zip: &Path) -> Result<(Vec<u8>, Pack), String> {
    let project = app.state.path.as_deref().and_then(Path::file_name).map_or_else(
        || format!("{}.flp", zip.file_stem().map_or_else(|| "Untitled".into(), |s| s.to_string_lossy())),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut pack = Pack { project, ..Default::default() };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let temp = std::env::temp_dir().join(format!("lemon-zip-{}-{nanos}.flp", std::process::id()));
    let saved = app.save_copy_with(temp.clone(), &mut |p| pack.gather(p));
    let bytes = std::fs::read(&temp);
    let _ = std::fs::remove_file(&temp);
    match saved {
        None => Err("wait until the project has loaded".into()),
        Some(Err(e)) => Err(e),
        Some(Ok(())) => Ok((bytes.map_err(|e| e.to_string())?, pack)),
    }
}

/// Writes the zip: the project file, then the samples, side by side at its top. Written beside `zip`, then moved in.
fn write_zip(zip: &Path, flp: &[u8], pack: &Pack, done: &AtomicU64, total: &AtomicU64) -> Result<(), String> {
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

/// Export zipped project... chose `zip`: writes it on a thread of its own.
pub fn export(app: &mut App, zip: PathBuf) {
    if app.zipping.is_some() {
        return;
    }
    let zip = if zip.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) {
        zip
    } else {
        let mut z = zip.into_os_string();
        z.push(".zip");
        PathBuf::from(z)
    };
    let (flp, pack) = match prepare(app, &zip) {
        Ok(p) => p,
        Err(e) => {
            app.state.message = Some(format!("Export zipped project failed: {e}"));
            return app.invalidate();
        }
    };
    let z = zip.clone();
    run(app, "Zipping project", &zip, move |done, total| {
        write_zip(&z, &flp, &pack, done, total)?;
        Ok(Done::Zipped { zip: z, samples: pack.files.len(), missing: pack.missing })
    });
}

// --- Both ---

/// Follows the running job: when it is over, an unzipped project opens, a written zip is reported.
pub fn tick(app: &mut App) {
    let Some(z) = &app.zipping else { return };
    let result = match z.rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => {
            app.damage_rect(crate::loading::panel_rect(app.state.screen).outset(4.0));
            return;
        }
        Err(TryRecvError::Disconnected) => Err("stopped".into()),
    };
    let z = app.zipping.take().expect("checked");
    match result {
        Ok(Done::Unzipped(project)) => app.open(project),
        Ok(Done::Zipped { zip, samples, missing }) => {
            app.state.message = Some(format!("Exported {} with {samples} samples", zip.display()));
            if !missing.is_empty() {
                let mut names = missing;
                names.sort_by_key(|n| n.to_lowercase());
                names.dedup();
                let text = format!(" was written without these samples, which were not found: {}.", names.join(", "));
                app.tell(Kind::Warning, "Samples not zipped", spans(&[&z.name, &text]));
            }
        }
        Err(e) => {
            let what = if z.title.starts_with("Unzip") { "Could not open " } else { "Could not write " };
            app.tell(Kind::Warning, z.title, spans(&[what, &z.name, &format!(": {e}")]));
        }
    }
    app.invalidate();
}

/// The progress panel: the zip and how much of it is done.
pub fn paint(c: &mut os_ui::Canvas, t: &os_theme::Palette, z: &Zipping) {
    let total = z.total.load(Ordering::Relaxed);
    let p = if total == 0 { 0.0 } else { (z.done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32 };
    crate::loading::panel(c, t, z.title, &format!("{}   {:.0} %", z.name, p * 100.0), p);
}

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

    /// Export, then open the zip: every sample is in it, the project is unchanged, and the unzipped project finds its
    /// samples beside it, a renamed one too.
    #[test]
    fn export_then_open() {
        let d = temp("roundtrip");
        std::fs::create_dir_all(d.join("one")).unwrap();
        std::fs::create_dir_all(d.join("two")).unwrap();
        let wav = crate::missing::tests::wav(441);
        std::fs::write(d.join("one/Kick.wav"), &wav).unwrap();
        std::fs::write(d.join("two/Kick.wav"), &wav[..wav.len() - 2]).unwrap();
        let mut app = App::new(None);
        app.state.project = crate::app::default_project();
        let rows: Vec<usize> = (0..4).map(|_| crate::app::add_channel(&mut app.state.project)).collect();
        let one = d.join("one/Kick.wav").to_string_lossy().into_owned();
        let two = d.join("two/Kick.wav").to_string_lossy().into_owned();
        app.state.project.channels[rows[0]].sample_path = Some(one.clone());
        app.state.project.channels[rows[1]].sample_path = Some(two.clone());
        app.state.project.channels[rows[2]].sample_path = Some(one.clone());
        app.state.project.channels[rows[3]].sample_path = Some(d.join("Gone.wav").to_string_lossy().into_owned());
        app.state.file = None;
        app.save_to(d.join("Song.flp"));
        let before = app.state.project.clone();
        let zip = d.join("out").join("Song.zip");
        std::fs::create_dir_all(d.join("out")).unwrap();
        let (flp, pack) = prepare(&mut app, &zip).unwrap();
        assert_eq!(app.state.project, before, "the project is not changed");
        assert_eq!(app.state.path.as_deref(), Some(d.join("Song.flp").as_path()));
        assert_eq!(pack.missing, vec!["Gone.wav".to_string()]);
        let (done, total) = (AtomicU64::new(0), AtomicU64::new(0));
        write_zip(&zip, &flp, &pack, &done, &total).unwrap();
        assert_eq!(names(&zip), vec!["Song.flp", "Kick.wav", "Kick_2.wav"]);
        assert!(!d.join("out").join("Song.zip.part").exists());
        // On another computer: the original folders are gone.
        std::fs::remove_dir_all(d.join("one")).unwrap();
        std::fs::remove_dir_all(d.join("two")).unwrap();
        let project = unzip(&zip, None, &done, &total).unwrap();
        app.open(project);
        let resolved = |row: usize| crate::plugins::resolve_sample(app.state.project.channels[row].sample_path.as_deref().unwrap());
        let unzipped = d.join("out").join("Song");
        assert_eq!(resolved(rows[0]), unzipped.join("Kick.wav").to_string_lossy());
        assert_eq!(resolved(rows[1]), unzipped.join("Kick_2.wav").to_string_lossy());
        assert_eq!(resolved(rows[2]), unzipped.join("Kick.wav").to_string_lossy());
        os_engine::sample::set_search_folders(Vec::new());
        let _ = std::fs::remove_dir_all(&d);
    }
}
