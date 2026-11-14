// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::zipped` (step 9: the session's side stays there).

#![allow(unused_imports)]
#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use crate::app::App;
use crate::dialog::{spans, Kind, Then};
#[allow(unused_imports)]
use crate::zipped::*;

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
    app.state.session.zipping = Some(Zipping { rx, title, name, done, total });
    app.invalidate();
}

/// Opens a zipped project: unzips it (`unzip`), then opens the project file in it.
pub fn import(app: &mut App, zip: PathBuf) {
    if app.state.session.zipping.is_some() {
        return;
    }
    let fallback = crate::user_dir::theme_dir().map(|d| d.join(UNZIP_DIR));
    let z = zip.clone();
    run(app, "Unzipping project", &zip, move |done, total| unzip(&z, fallback.as_deref(), done, total).map(Done::Unzipped));
}

/// FILE > Export zipped project...: a Save dialog for the .zip, in the project's folder, named after it.
pub fn export_dialog(app: &mut App) {
    use crate::filedlg::{FileDlg, Mode};
    let path = app.state.session.path.clone();
    let stem = path.as_deref().and_then(Path::file_stem).map_or_else(|| "Untitled".to_string(), |s| s.to_string_lossy().into_owned());
    let dlg = FileDlg::new(Mode::Save, "Export zipped project", "zipped project", Then::ExportZip)
        .filter("Zip file", &["zip"])
        .start(path.as_deref().and_then(Path::parent))
        .name(&format!("{stem}.zip"));
    app.file_dialog(dlg);
}

/// The project file's bytes as Save writes them (with the samples' names in the zip) and the samples to put beside it.
/// The project itself, its file and its unsaved changes stay as they are.
fn prepare(app: &mut App, zip: &Path) -> Result<(Vec<u8>, Pack), String> {
    // The project goes in as Save writes it: a .lemon file (an FLP's name with .lemon).
    let ext = crate::project_files::EXT;
    let project = app.state.session.path.as_deref().and_then(Path::file_stem).map_or_else(
        || format!("{}.{ext}", zip.file_stem().map_or_else(|| "Untitled".into(), |s| s.to_string_lossy())),
        |n| format!("{}.{ext}", n.to_string_lossy()),
    );
    let mut pack = Pack { project, ..Default::default() };
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let temp = std::env::temp_dir().join(format!("lemon-zip-{}-{nanos}.{ext}", std::process::id()));
    let saved = app.save_copy_with(temp.clone(), &mut |p| pack.gather(p));
    let bytes = std::fs::read(&temp);
    let _ = std::fs::remove_file(&temp);
    match saved {
        None => Err("wait until the project has loaded".into()),
        Some(Err(e)) => Err(e),
        Some(Ok(())) => Ok((bytes.map_err(|e| e.to_string())?, pack)),
    }
}

/// Export zipped project... chose `zip`: writes it on a thread of its own.
pub fn export(app: &mut App, zip: PathBuf) {
    if app.state.session.zipping.is_some() {
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

/// Follows the running job: when it is over, an unzipped project opens, a written zip is reported.
pub fn tick(app: &mut App) {
    let Some(z) = &app.state.session.zipping else { return };
    let result = match z.rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => {
            app.damage_rect(crate::loading_ui::panel_rect(app.state.screen).outset(4.0));
            return;
        }
        Err(TryRecvError::Disconnected) => Err("stopped".into()),
    };
    let z = app.state.session.zipping.take().expect("checked");
    match result {
        Ok(Done::Unzipped(project)) => {
            app.open(project);
        }
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
    crate::loading_ui::panel(c, t, z.title, &format!("{}   {:.0} %", z.name, p * 100.0), p);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(zip: &Path) -> Vec<String> {
        let mut a = zip::ZipArchive::new(File::open(zip).unwrap()).unwrap();
        (0..a.len()).map(|i| a.by_index(i).unwrap().name().unwrap().to_string()).collect()
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("os_zipped_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Export, then open the zip: every sample is in it, the project is unchanged, and the unzipped project finds its
    /// samples beside it, a renamed one too.
    #[test]
    fn export_then_open() {
        let d = temp("roundtrip");
        std::fs::create_dir_all(d.join("one")).unwrap();
        std::fs::create_dir_all(d.join("two")).unwrap();
        let wav = crate::missing_ui::tests::wav(441);
        std::fs::write(d.join("one/Kick.wav"), &wav).unwrap();
        std::fs::write(d.join("two/Kick.wav"), &wav[..wav.len() - 2]).unwrap();
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::default_project());
        let rows: Vec<usize> = (0..4).map(|_| crate::projects::add_channel(app.state.session.project.get_mut())).collect();
        let one = d.join("one/Kick.wav").to_string_lossy().into_owned();
        let two = d.join("two/Kick.wav").to_string_lossy().into_owned();
        app.state.session.project.get_mut().channels[rows[0]].sample_path = Some(one.clone());
        app.state.session.project.get_mut().channels[rows[1]].sample_path = Some(two.clone());
        app.state.session.project.get_mut().channels[rows[2]].sample_path = Some(one.clone());
        app.state.session.project.get_mut().channels[rows[3]].sample_path = Some(d.join("Gone.wav").to_string_lossy().into_owned());
        app.state.session.file = None;
        app.save_to(d.join("Song.flp"));
        let before = app.state.session.project.clone();
        let zip = d.join("out").join("Song.zip");
        std::fs::create_dir_all(d.join("out")).unwrap();
        let (flp, pack) = prepare(&mut app, &zip).unwrap();
        assert_eq!(app.state.session.project, before, "the project is not changed");
        assert_eq!(app.state.session.path.as_deref(), Some(d.join("Song.flp").as_path()));
        assert_eq!(pack.missing, vec!["Gone.wav".to_string()]);
        let (done, total) = (AtomicU64::new(0), AtomicU64::new(0));
        write_zip(&zip, &flp, &pack, &done, &total).unwrap();
        assert_eq!(names(&zip), vec!["Song.lemon", "Kick.wav", "Kick_2.wav"], "the project goes in as a .lemon file");
        assert!(!d.join("out").join("Song.zip.part").exists());
        // On another computer: the original folders are gone.
        std::fs::remove_dir_all(d.join("one")).unwrap();
        std::fs::remove_dir_all(d.join("two")).unwrap();
        let project = unzip(&zip, None, &done, &total).unwrap();
        app.open(project);
        let resolved = |row: usize| crate::plugins::resolve_sample(app.state.session.project.channels[row].sample_path.as_deref().unwrap());
        let unzipped = d.join("out").join("Song");
        assert_eq!(resolved(rows[0]), unzipped.join("Kick.wav").to_string_lossy());
        assert_eq!(resolved(rows[1]), unzipped.join("Kick_2.wav").to_string_lossy());
        assert_eq!(resolved(rows[2]), unzipped.join("Kick.wav").to_string_lossy());
        os_engine::sample::set_search_folders(Vec::new());
        let _ = std::fs::remove_dir_all(&d);
    }
}
