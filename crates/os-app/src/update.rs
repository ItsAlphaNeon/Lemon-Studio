// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Updates, for the builds the release workflow makes: a nightly build follows the nightlies, a stable one the stable
//! releases (`crate::version`). A build from source never updates itself.
//!
//! The work is done by `lemonstudio-updater` (`tools/updater`), which the release build carries inside itself
//! (`build.rs`, from `LEMON_UPDATER`). It is written to `<user folder>/Updates` and started from there, so it can
//! replace the program it came from:
//!
//! 1. At start (General ▸ Updates ▸ Automatically check for updates), or from HELP ▸ Check for updates, it checks the
//!    channel; with Automatically download program updates on it downloads a newer build right away.
//! 2. Lemon Studio offers it in a message box: Install and restart, Skip this version, Later.
//! 3. Install asks about unsaved changes as closing does, and closes; on the way out (`Updates::on_exit`) the updater
//!    is started to wait for this process to end, put the new build in place of this one (the `.exe`, the AppImage
//!    or the `.app`) and start it.
//! 4. The next start reports how it went (`Updates::startup`).
//!
//! Nothing here blocks: the updater runs on a thread of its own, and its answers are picked up by `App::update_tick`.
//! `OPENSTUDIO_NO_UPDATE_CHECK=1` turns the check at start off; `OPENSTUDIO_UPDATE_BASE` checks another releases URL.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;

use crate::version::{CHANNEL, VERSION};

/// The updater built into this program, from `LEMON_UPDATER` at build time (release builds only).
#[cfg(lemon_updater)]
const UPDATER: Option<&[u8]> = Some(include_bytes!(env!("LEMON_UPDATER_FILE")));
#[cfg(not(lemon_updater))]
const UPDATER: Option<&[u8]> = None;

const UPDATER_NAME: &str = if cfg!(windows) { "lemonstudio-updater.exe" } else { "lemonstudio-updater" };

/// Set to `1`: no check at start (HELP ▸ Check for updates still checks).
const NO_CHECK: &str = "OPENSTUDIO_NO_UPDATE_CHECK";

/// Another releases URL to check (testing an update against a server of one's own; the manifest must still carry the
/// release key's signature).
const BASE: &str = "OPENSTUDIO_UPDATE_BASE";

/// The version the user chose to skip (Skip this version), in the program's settings file.
const SKIPPED_KEY: &str = "update_skipped_version";

/// Whether this build updates itself: a release build carrying its updater.
pub fn enabled() -> bool {
    CHANNEL.is_some() && UPDATER.is_some()
}

/// `<user folder>/Updates/<channel>`: the updater, the download, the outcome of the last update. A folder per channel:
/// a nightly and an alpha on one computer share the user folder, and must not take each other's download or report
/// each other's update.
fn folder() -> Option<PathBuf> {
    Some(os_paths::user_file("Updates")?.join(CHANNEL?))
}

/// What the update replaces: the program file on Windows, the AppImage on Linux, the `.app` on macOS. An error says
/// why this copy cannot be updated (in words for the user).
pub fn target() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Lemon Studio could not find its own program file: {e}"))?;
    #[cfg(target_os = "linux")]
    let target = {
        let _ = exe;
        match std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_absolute()) {
            Some(p) => p,
            None => return Err("This copy of Lemon Studio is not running from its AppImage, so it can't update itself.".into()),
        }
    };
    #[cfg(target_os = "macos")]
    let target = {
        let Some(app) = exe.ancestors().find(|p| p.extension().is_some_and(|x| x == "app")).map(std::path::Path::to_path_buf) else {
            return Err(format!("This copy of Lemon Studio is not inside its {}.app, so it can't update itself.", crate::version::NAME));
        };
        // An app opened from where it was downloaded (still quarantined) runs from a read-only copy macOS made.
        if app.to_string_lossy().contains("/AppTranslocation/") {
            return Err(format!("Move {}.app to your Applications folder and open it from there: macOS runs it from a read-only copy where it is now, so it can't update itself.", crate::version::NAME));
        }
        app
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let target = exe;
    let dir = target.parent().ok_or("Lemon Studio's program file has no folder.")?;
    let probe = dir.join(format!(".lemonstudio-update-check-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(target)
        }
        Err(_) => Err(format!("Lemon Studio can't replace itself in {}: that folder can't be written to. Download the new version from the release page instead.", dir.display())),
    }
}

/// The updater, written out of this program into the updates folder (again whenever it differs).
fn updater() -> Result<PathBuf, String> {
    let bytes = UPDATER.ok_or("This build has no updater.")?;
    let dir = folder().ok_or("Lemon Studio has no user folder to update from.")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(UPDATER_NAME);
    if std::fs::read(&path).is_ok_and(|b| b == bytes) {
        return Ok(path);
    }
    let tmp = dir.join(format!("{UPDATER_NAME}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        // Windows: an updater still running (from a moment ago) holds its file; that one is as good.
        if !path.exists() {
            return Err(format!("{}: {e}", path.display()));
        }
    }
    Ok(path)
}

/// What a check found (`Event::Found`).
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub version: String,
    /// What changed, a line each.
    pub notes: Vec<String>,
    /// The release's page.
    pub page: String,
    /// The download, ready to install (None: not downloaded yet).
    pub staged: Option<PathBuf>,
}

/// The updater's answers, as `App::update_tick` gets them.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Downloading: bytes so far, of.
    Progress(u64, u64),
    Found(Found),
    /// This is the newest build.
    Current,
    /// The channel has no release yet.
    NoRelease,
    /// The channel has no build for this platform.
    Unsupported,
    Failed(String),
}

/// An answer line of the updater (`tools/updater`'s `main`); None for a line it does not know.
pub fn parse(line: &str) -> Option<Event> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Some(match v.get("status")?.as_str()? {
        "progress" => Event::Progress(v.get("done")?.as_u64()?, v.get("total")?.as_u64()?),
        "available" | "staged" => Event::Found(Found {
            version: text("version"),
            notes: v.get("notes").and_then(|n| n.as_array()).map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect()).unwrap_or_default(),
            page: text("page"),
            staged: v.get("path").and_then(|p| p.as_str()).map(PathBuf::from),
        }),
        "current" => Event::Current,
        "none" => Event::NoRelease,
        "unsupported" => Event::Unsupported,
        "error" => Event::Failed(text("message")),
        _ => return None,
    })
}

/// The update waiting to be put in place when Lemon Studio closes.
#[derive(Debug, Clone, PartialEq)]
pub struct Install {
    pub staged: PathBuf,
    pub version: String,
    /// Start Lemon Studio again afterwards (Install and restart; not when the closing was put off and happens later).
    pub relaunch: bool,
}

#[derive(Default)]
pub struct Updates {
    /// The updater's answers while it runs.
    rx: Option<mpsc::Receiver<Event>>,
    /// HELP ▸ Check for updates asked: the outcome is told even when there is nothing new, and a skipped version is
    /// offered again.
    pub asked: bool,
    /// Install was chosen before the download was there: it installs once downloaded.
    pub install_when_staged: bool,
    /// The update found, while its message box waits for a moment without a dialog (and is shown).
    pub offer: Option<Found>,
    pub offer_due: bool,
    pub install: Option<Install>,
}

impl Updates {
    /// Whether the updater is running.
    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Starts the updater on a thread of its own: a check, and the download of a newer build when `download`.
    pub fn start(&mut self, download: bool) -> Result<(), String> {
        let (Some(channel), true) = (CHANNEL, enabled()) else { return Err("This build doesn't update itself.".into()) };
        let exe = updater()?;
        let mut cmd = Command::new(exe);
        cmd.args([if download { "download" } else { "check" }, "--channel", channel, "--current", VERSION]);
        if download {
            cmd.arg("--to").arg(folder().ok_or("no user folder")?.join("download"));
        }
        if let Some(base) = std::env::var_os(BASE).filter(|b| !b.is_empty()) {
            cmd.arg("--base").arg(base);
        }
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("The updater could not start: {e}"))?;
        let out = child.stdout.take().expect("piped");
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        std::thread::Builder::new()
            .name("update".into())
            .spawn(move || {
                let mut answered = false;
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(e) = parse(&line) {
                        answered |= !matches!(e, Event::Progress(..));
                        if tx.send(e).is_err() {
                            break;
                        }
                        crate::wake_ui();
                    }
                }
                let status = child.wait();
                if !answered {
                    let why = status.map_or_else(|e| e.to_string(), |s| s.to_string());
                    let _ = tx.send(Event::Failed(format!("the updater ended without an answer ({why})")));
                    crate::wake_ui();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The updater's answers since the last call. Its last one ends the run.
    pub fn poll(&mut self) -> Vec<Event> {
        let Some(rx) = &self.rx else { return Vec::new() };
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(e) => out.push(e),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    break;
                }
            }
        }
        if out.iter().any(|e| !matches!(e, Event::Progress(..))) {
            self.rx = None;
        }
        out
    }

    /// At start: the outcome of the update the last run started (to tell the user: true when it worked, and what to
    /// say), the program the update replaced removed (Windows), and the check at start begun when it is on.
    pub fn startup(&mut self) -> Option<(bool, String)> {
        #[cfg(windows)]
        if let Ok(exe) = std::env::current_exe() {
            let mut old = exe.into_os_string();
            old.push(".old");
            let _ = std::fs::remove_file(old);
        }
        let outcome = folder().map(|d| d.join("result.json")).and_then(|f| {
            let text = std::fs::read_to_string(&f).ok();
            let _ = std::fs::remove_file(&f);
            text
        });
        let told = outcome.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).map(|v| {
            let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
                (true, format!("Lemon Studio was updated to {version}."))
            } else {
                let why = v.get("message").and_then(|x| x.as_str()).unwrap_or("unknown error");
                (false, format!("The update to Lemon Studio {version} could not be installed: {why}"))
            }
        });
        let off = std::env::var_os(NO_CHECK).is_some_and(|v| v == "1");
        if enabled() && !off && crate::prefs::check(&crate::prefs::AUTO_CHECK_UPDATES) && target().is_ok() {
            if let Err(e) = self.start(crate::prefs::check(&crate::prefs::AUTO_DOWNLOAD_UPDATES)) {
                eprintln!("Lemon Studio: update check: {e}");
            }
        }
        told
    }

    /// The version the user skipped.
    pub fn skipped() -> Option<String> {
        crate::settings_file::get(SKIPPED_KEY).filter(|v| !v.is_empty())
    }

    pub fn skip(version: &str) {
        crate::settings_file::write(SKIPPED_KEY, version);
    }

    /// Lemon Studio is closing: an update waiting to be installed hands over to the updater, which waits for this
    /// process to end.
    pub fn on_exit(&mut self) {
        let Some(install) = self.install.take() else { return };
        if let Err(e) = apply(&install) {
            eprintln!("Lemon Studio: the update could not start: {e}");
            if let Some(f) = folder() {
                let record = serde_json::json!({ "ok": false, "version": install.version, "message": e });
                let _ = std::fs::write(f.join("result.json"), record.to_string());
            }
        }
    }
}

/// Starts the updater's `apply` for `install`, on its own (it outlives this process).
fn apply(install: &Install) -> Result<(), String> {
    let exe = updater()?;
    let target = target()?;
    let result = folder().ok_or("no user folder")?.join("result.json");
    let mut cmd = Command::new(exe);
    cmd.arg("apply").arg("--pid").arg(std::process::id().to_string());
    cmd.arg("--staged").arg(&install.staged).arg("--target").arg(&target);
    cmd.arg("--version").arg(&install.version).arg("--result").arg(&result);
    if install.relaunch {
        cmd.arg("--relaunch");
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(dir) = result.parent() {
        cmd.current_dir(dir);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        // Out of a job that would end it with this process, where the job allows that.
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().map(drop).map_err(|e| e.to_string())
}

/// The message box's text: the version, what changed (at most `MAX_NOTES` lines) and what Install does.
pub fn offer_text(f: &Found) -> Vec<(String, bool)> {
    const MAX_NOTES: usize = 8;
    let mut t = vec![
        ("Lemon Studio ".to_string(), false),
        (f.version.clone(), true),
        (format!(" is available. You have {}.", VERSION), false),
    ];
    if !f.notes.is_empty() {
        let mut s = String::from("\n\nWhat's new:");
        for n in f.notes.iter().take(MAX_NOTES) {
            s.push_str(&format!("\n\u{2022} {n}"));
        }
        if f.notes.len() > MAX_NOTES {
            s.push_str(&format!("\n\u{2026} and {} more", f.notes.len() - MAX_NOTES));
        }
        t.push((s, false));
    }
    t.push(("\n\nInstalling closes Lemon Studio (asking about unsaved changes first) and opens the new version.".into(), false));
    t
}

impl crate::app::App {
    /// At start, once the window's state is restored: how the last update went, and the check at start.
    pub(crate) fn update_startup(&mut self) {
        use crate::dialog::{spans, Answer, Kind, Msg, Then};
        match self.updates.startup() {
            Some((true, m)) => self.state.message = Some(m),
            Some((false, m)) => self.show_msg(Msg::new(Kind::Error, "Update", spans(&[&m]), &[Answer::Ok], Then::Nothing)),
            None => {}
        }
    }

    /// HELP ▸ Check for updates.
    pub(crate) fn check_updates_now(&mut self) {
        use crate::dialog::{spans, Answer, Kind, Msg, Then};
        let info = |app: &mut Self, text: &str| {
            app.show_msg(Msg::new(Kind::Info, "Check for updates", spans(&[text]), &[Answer::Ok], Then::Nothing));
        };
        if !enabled() {
            return info(
                self,
                "This copy of Lemon Studio was built from its source code, so it doesn't update itself: update it with git pull, then cargo run -p os-app --release (see the README).",
            );
        }
        if let Err(e) = target() {
            return info(self, &e);
        }
        if self.updates.busy() {
            self.updates.asked = true;
            self.state.message = Some("Already checking for updates".into());
            return;
        }
        match self.updates.start(crate::prefs::check(&crate::prefs::AUTO_DOWNLOAD_UPDATES)) {
            Ok(()) => {
                self.updates.asked = true;
                self.state.message = Some("Checking for updates".into());
            }
            Err(e) => info(self, &e),
        }
    }

    /// The updater's answers, and the update offered once no dialog is in the way (and nothing is being recorded).
    pub(crate) fn update_tick(&mut self) {
        use crate::dialog::{spans, Answer, Kind, Msg, Then};
        for e in self.updates.poll() {
            let asked = self.updates.asked || self.updates.install_when_staged;
            match e {
                Event::Progress(done, total) => {
                    if asked {
                        let pct = if total > 0 { done * 100 / total } else { 0 };
                        self.state.message = Some(format!("Downloading the update: {pct}%"));
                    }
                }
                Event::Found(f) => {
                    let wanted = std::mem::take(&mut self.updates.install_when_staged);
                    if wanted && f.staged.is_some() {
                        self.updates.asked = false;
                        self.install_update(f);
                        continue;
                    }
                    if !self.updates.asked && Updates::skipped().as_deref() == Some(f.version.as_str()) {
                        continue;
                    }
                    self.updates.asked = false;
                    self.updates.offer = Some(f);
                    self.updates.offer_due = true;
                }
                other => {
                    self.updates.install_when_staged = false;
                    self.updates.asked = false;
                    let channel = crate::version::releases();
                    let text = match other {
                        Event::Current => format!("You have the newest version of Lemon Studio, {}.", crate::version::text()),
                        Event::NoRelease => format!("There is no {channel} release of Lemon Studio yet."),
                        Event::Unsupported => format!("The newest {channel} release of Lemon Studio has no build for this system yet."),
                        Event::Failed(m) => {
                            eprintln!("Lemon Studio: update check: {m}");
                            format!("Lemon Studio could not check for updates: {m}")
                        }
                        Event::Progress(..) | Event::Found(_) => continue,
                    };
                    // The check at start keeps quiet about these.
                    if asked {
                        self.state.message = None;
                        self.show_msg(Msg::new(Kind::Info, "Check for updates", spans(&[&text]), &[Answer::Ok], Then::Nothing));
                    }
                }
            }
        }
        let recording = self.audio_rec.is_some() || self.track_rec.is_some();
        if self.updates.offer_due && !self.modal_open() && !recording {
            self.updates.offer_due = false;
            if let Some(f) = self.updates.offer.clone() {
                let install = if f.staged.is_some() { "Install and restart" } else { "Download and install" };
                let m = Msg::new(Kind::Info, "Update available", offer_text(&f), &[Answer::Yes, Answer::No, Answer::Cancel], Then::Update(f))
                    .labelled(&[install, "Skip this version", "Later"]);
                self.show_msg(m);
            }
        }
    }

    /// The update's message box was answered: Install (Yes), Skip this version (No) or Later (Cancel).
    pub(crate) fn update_answer(&mut self, f: Found, answer: crate::dialog::Answer) {
        use crate::dialog::{spans, Answer, Kind, Msg, Then};
        match answer {
            Answer::Yes if f.staged.is_some() => self.install_update(f),
            Answer::Yes => {
                self.updates.install_when_staged = true;
                match self.updates.start(true) {
                    Ok(()) => self.state.message = Some("Downloading the update".into()),
                    Err(e) => {
                        self.updates.install_when_staged = false;
                        self.show_msg(Msg::new(Kind::Error, "Update", spans(&[&e]), &[Answer::Ok], Then::Nothing));
                    }
                }
            }
            Answer::No => Updates::skip(&f.version),
            _ => {}
        }
    }

    /// Installs the downloaded update `f`: Lemon Studio closes (asking about unsaved changes first) and the updater
    /// takes over (`Updates::on_exit`).
    fn install_update(&mut self, f: Found) {
        let Some(staged) = f.staged else { return };
        self.updates.offer = None;
        self.updates.install = Some(Install { staged, version: f.version, relaunch: true });
        self.ask_discard("updating", crate::dialog::Next::Close);
    }

    /// Closing for the update was called off (Cancel to the unsaved changes): it installs when Lemon Studio closes,
    /// without starting it again.
    pub(crate) fn update_put_off(&mut self) {
        if let Some(i) = self.updates.install.as_mut().filter(|i| i.relaunch) {
            i.relaunch = false;
            self.state.message = Some(format!("Lemon Studio {} installs when Lemon Studio closes", i.version));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_updaters_answers_are_read() {
        assert_eq!(parse(r#"{"status":"progress","done":5,"total":10}"#), Some(Event::Progress(5, 10)));
        assert_eq!(parse(r#"{"status":"current","version":"2026.11.7.1"}"#), Some(Event::Current));
        assert_eq!(parse(r#"{"status":"none"}"#), Some(Event::NoRelease));
        assert_eq!(parse(r#"{"status":"unsupported","version":"1.0.0","platform":"linux-riscv64"}"#), Some(Event::Unsupported));
        assert_eq!(parse(r#"{"message":"offline","status":"error"}"#), Some(Event::Failed("offline".into())));
        let staged = r#"{"status":"staged","version":"2026.11.8.1","path":"/u/Updates/download/a.AppImage","notes":["One","Two"],"page":"https://x"}"#;
        assert_eq!(
            parse(staged),
            Some(Event::Found(Found {
                version: "2026.11.8.1".into(),
                notes: vec!["One".into(), "Two".into()],
                page: "https://x".into(),
                staged: Some("/u/Updates/download/a.AppImage".into()),
            }))
        );
        let available = parse(r#"{"status":"available","version":"1.0.1","notes":[],"page":"","size":3}"#);
        assert!(matches!(available, Some(Event::Found(Found { staged: None, .. }))));
        assert_eq!(parse("not json"), None);
        assert_eq!(parse(r#"{"status":"later"}"#), None);
    }

    #[test]
    fn a_long_list_of_changes_is_cut_short() {
        let f = Found { version: "2026.11.8.1".into(), notes: (1..=12).map(|i| format!("Change {i}")).collect(), page: String::new(), staged: None };
        let text: String = offer_text(&f).into_iter().map(|(s, _)| s).collect();
        assert!(text.contains("\u{2022} Change 8") && !text.contains("Change 9"), "{text}");
        assert!(text.contains("and 4 more"), "{text}");
    }

    #[test]
    fn a_build_from_source_does_not_update() {
        if option_env!("LEMON_CHANNEL").is_none() {
            assert!(!enabled());
            assert!(Updates::default().start(false).is_err());
        }
    }
}
