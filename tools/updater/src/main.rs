// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `lemonstudio-updater`: Lemon Studio's updater, part of the builds the release workflow makes (nightly and stable).
//! A build from source has none: Lemon Studio then never checks for updates (`os-app`'s `update`).
//!
//! Lemon Studio carries this program inside itself, writes it to its user folder and starts it there (so it can
//! replace the program it came from). It has no window; it answers on standard output, one JSON object per line, each
//! with a `status`:
//!
//! - `check --channel <nightly|stable> --current <version>`: is there a newer build?
//!   `none` (the channel has no release yet), `current`, `unsupported` (no build for this platform),
//!   `available` (`version`, `notes`, `page`, `size`), or `error` (`message`).
//! - `download --channel <c> --current <v> --to <folder>`: as `check`, then downloads the newer build into `folder`
//!   (a download already there for the same build is kept), reporting `progress` (`done`, `total`) and then `staged`
//!   (`version`, `path`, `notes`, `page`).
//! - `apply --pid <n> --staged <path> --target <path> --version <v> --result <file> [--relaunch]`: waits for Lemon
//!   Studio (process `n`) to close, puts the staged build in place of `target` (`swap`), writes the outcome to `file`
//!   (`{"ok": bool, "version": "...", "message": "..."}`) for the next start to report, and starts Lemon Studio again
//!   if asked to (the old one, should the update have failed).
//!
//! `--base <url>` replaces the releases URL (for testing against another server). Every manifest must carry a valid
//! minisign signature by the release key, whose public half is built in (`LEMON_UPDATE_KEY` at build time): without
//! it, this program checks nothing.

// No console window when Lemon Studio starts it; its answers go through the pipes Lemon Studio gives it.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod manifest;
mod net;
mod swap;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

/// Where the releases are.
const RELEASES: &str = "https://github.com/ItsAlphaNeon/Lemon-Studio/releases";

/// The release key's public half (minisign), set by the release workflow from the repository's `MINISIGN_PUBLIC_KEY`.
const KEY: Option<&str> = option_env!("LEMON_UPDATE_KEY");

/// How long `apply` waits for Lemon Studio to close (its plugins may take a while to shut down).
const CLOSE_TIMEOUT: Duration = Duration::from_secs(300);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(()) => 0,
        Err(message) => {
            emit(json!({ "status": "error", "message": message }));
            1
        }
    };
    std::process::exit(code);
}

fn run(args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let opts = Opts(&args[args.len().min(1)..]);
    match cmd {
        "check" => check(&opts, None),
        "download" => check(&opts, Some(PathBuf::from(opts.need("--to")?))),
        "apply" => apply(&opts),
        "--version" => {
            println!("lemonstudio-updater {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err(format!("usage: lemonstudio-updater check|download|apply ... (got {cmd:?})")),
    }
}

/// `--name value` options.
struct Opts<'a>(&'a [String]);

impl Opts<'_> {
    fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().position(|a| a == name).and_then(|i| self.0.get(i + 1)).map(String::as_str)
    }

    fn need(&self, name: &str) -> Result<&str, String> {
        self.get(name).ok_or_else(|| format!("{name} is missing"))
    }

    fn flag(&self, name: &str) -> bool {
        self.0.iter().any(|a| a == name)
    }
}

fn emit(v: serde_json::Value) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

/// `check`, and `download` when `to` is given.
fn check(opts: &Opts, to: Option<PathBuf>) -> Result<(), String> {
    let channel = opts.need("--channel")?;
    let current = opts.need("--current")?;
    let base = opts.get("--base").unwrap_or(RELEASES);
    let key = KEY.filter(|k| !k.trim().is_empty()).ok_or("this updater was built without the release key")?;
    let url = manifest::url(base, channel)?;
    let net = net::Net::new()?;
    let Some(body) = net.get(&url)? else {
        emit(json!({ "status": "none" }));
        return Ok(());
    };
    let sig = net.get(&format!("{url}.minisig"))?.ok_or("the manifest has no signature")?;
    let m = manifest::verified(&body, &String::from_utf8_lossy(&sig), key, channel)?;
    if !manifest::newer(channel, &m.version, current)? {
        emit(json!({ "status": "current", "version": m.version }));
        return Ok(());
    }
    let Some(asset) = m.assets.get(&manifest::platform()) else {
        emit(json!({ "status": "unsupported", "version": m.version, "platform": manifest::platform() }));
        return Ok(());
    };
    let Some(to) = to else {
        emit(json!({ "status": "available", "version": m.version, "notes": m.notes, "page": m.page, "size": asset.size }));
        return Ok(());
    };
    let path = stage(&net, &m, asset, &to)?;
    emit(json!({ "status": "staged", "version": m.version, "path": path, "notes": m.notes, "page": m.page }));
    Ok(())
}

/// What a finished download in the staging folder is (`staged.json`).
#[derive(serde::Serialize, serde::Deserialize)]
struct Staged {
    version: String,
    sha256: String,
    path: PathBuf,
}

/// Downloads `asset` into `to` (emptied first, unless it holds this very build already) and unpacks a `.zip` (macOS:
/// the `.app`); the path of the build to install.
fn stage(net: &net::Net, m: &manifest::Manifest, asset: &manifest::Asset, to: &Path) -> Result<PathBuf, String> {
    let record = to.join("staged.json");
    if let Some(s) = std::fs::read(&record).ok().and_then(|b| serde_json::from_slice::<Staged>(&b).ok()) {
        if s.version == m.version && s.sha256.eq_ignore_ascii_case(&asset.sha256) && s.path.exists() {
            return Ok(s.path);
        }
    }
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let name = asset.url.rsplit('/').next().filter(|n| !n.is_empty() && !n.contains(['\\', ':'])).unwrap_or("download");
    let file = to.join(name);
    net.download(&asset.url, &file, asset.size, &asset.sha256, |done, total| {
        emit(json!({ "status": "progress", "done": done, "total": total }));
    })?;
    let path = if name.ends_with(".zip") { unzip_app(&file, &to.join("unpacked"))? } else { file };
    let s = Staged { version: m.version.clone(), sha256: asset.sha256.clone(), path: path.clone() };
    std::fs::write(&record, serde_json::to_vec(&s).expect("json")).map_err(|e| format!("{}: {e}", record.display()))?;
    Ok(path)
}

/// macOS: the `.app` inside `zip`, unpacked into `dir` with `ditto` (as the Finder does: links and signature kept).
fn unzip_app(zip: &Path, dir: &Path) -> Result<PathBuf, String> {
    let ok = std::process::Command::new("ditto").args(["-x", "-k"]).arg(zip).arg(dir).status().is_ok_and(|s| s.success());
    if !ok {
        return Err(format!("could not unpack {}", zip.display()));
    }
    let _ = std::fs::remove_file(zip);
    std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "app"))
        .ok_or_else(|| format!("{} holds no .app", zip.display()))
}

fn apply(opts: &Opts) -> Result<(), String> {
    let pid: u32 = opts.need("--pid")?.parse().map_err(|_| "--pid is not a number")?;
    let staged = PathBuf::from(opts.need("--staged")?);
    let target = PathBuf::from(opts.need("--target")?);
    let version = opts.need("--version")?;
    let result = PathBuf::from(opts.need("--result")?);
    let outcome = if swap::wait_for_exit(pid, CLOSE_TIMEOUT) {
        swap::replace(&staged, &target)
    } else {
        Err("Lemon Studio did not close".into())
    };
    if outcome.is_ok() {
        // The download is not needed any more (`staged` is inside the staging folder; its record too).
        if let Some(dir) = staged.ancestors().find(|d| d.join("staged.json").exists()) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    let message = outcome.as_ref().err().cloned().unwrap_or_default();
    let record = json!({ "ok": outcome.is_ok(), "version": version, "message": message });
    if let Some(dir) = result.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&result, record.to_string());
    if opts.flag("--relaunch") {
        swap::relaunch(&target)?;
    }
    outcome
}
