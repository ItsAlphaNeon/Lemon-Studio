//! Plugin scanning (spec: ui/plugin-manager.md §4): each plugin file is loaded in a separate scanner process (this
//! program started with `--scan-plugin`), so a plugin that crashes, hangs or opens a window while it loads cannot harm
//! the app; the sweep runs on a background thread and hands its results to the UI thread.
// spec: ui/plugin-manager.md

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use os_host::Format;

use crate::app::App;
use crate::plugin_db::{self as db, Cache, Mode, Record, Scanned, Status};

/// The scanner's command-line switch: `lemonstudio --scan-plugin <format> <path>`.
pub const ARG: &str = "--scan-plugin";
/// The scanner's one line on standard output starts with this.
const TAG: &str = "OSSCAN ";
/// A scanner still running after this is ended (status `timed out`).
const TIMEOUT: Duration = Duration::from_secs(30);
/// Scanners running at the same time.
pub const WORKERS: usize = 4;

/// The scanner process: loads one plugin file, prints its plugin list as one `OSSCAN <json>` line and ends at once,
/// without unloading the plugin (its clean-up code never runs).
pub fn child_main(args: &[String]) -> ! {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows_sys::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX};
        // No error boxes (a missing DLL, a crash): the app reads the outcome from the exit code.
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);
        // The same thread model as the app's UI thread, where plugins are loaded for use.
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }
    let format = args.first().and_then(|f| Format::parse(f));
    let path = args.get(1).map(PathBuf::from);
    let out = match (format, path) {
        (Some(format), Some(path)) => match crate::plugins::load_file(&path, format) {
            Ok(lib) => serde_json::json!({ "ok": true, "plugins": lib.plugins().iter().map(Record::of).collect::<Vec<_>>() }),
            Err(e) => serde_json::json!({ "ok": false, "error": e, "crashed": os_host::guard::tripped() }),
        },
        _ => serde_json::json!({ "ok": false, "error": format!("usage: {ARG} clap|vst3|vst2|native <path>") }),
    };
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{TAG}{out}");
    let _ = stdout.flush();
    end_now(0)
}

/// Ends this process without running anything else (no plugin clean-up code).
fn end_now(code: u32) -> ! {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), code);
    }
    std::process::exit(code as i32)
}

/// The outcome of a scanner's line.
fn parse(line: &str) -> Scanned {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return Scanned::failed(Status::Error, "the scanner's answer was unreadable") };
    if v["ok"].as_bool() == Some(true) {
        let plugins = serde_json::from_value::<Vec<Record>>(v["plugins"].clone()).unwrap_or_default();
        return Scanned::ok(plugins);
    }
    let error = v["error"].as_str().unwrap_or("the plugin did not load").to_string();
    Scanned::failed(if v["crashed"].as_bool() == Some(true) { Status::Crashed } else { Status::Error }, error)
}

/// Scans file `path` in a scanner process (§4).
pub fn scan_in_child(path: &Path, format: Format) -> Scanned {
    let Ok(exe) = std::env::current_exe() else { return Scanned::failed(Status::Error, "no scanner program") };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(ARG).arg(format.as_str()).arg(path).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Scanned::failed(Status::Error, format!("the scanner did not start: {e}")),
    };
    let (tx, rx) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        // Plugins may print anything while they load: only the tagged line counts.
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(rest) = line.strip_prefix(TAG) {
                    let _ = tx.send(rest.to_string());
                    break;
                }
            }
        });
    }
    let line = match rx.recv_timeout(TIMEOUT) {
        Ok(l) => Some(l),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            let _ = child.kill();
            let _ = child.wait();
            return Scanned::failed(Status::Timeout, format!("still loading after {} s", TIMEOUT.as_secs()));
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => None,
    };
    // The scanner ends itself after its line; one that does not is ended.
    let end = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                break child.wait().ok();
            }
        }
    };
    match line {
        Some(l) => parse(&l),
        None => {
            let code = status.and_then(|s| s.code()).map_or(0, |c| c as u32);
            Scanned::failed(Status::Crashed, format!("the scanner ended with {} while loading it", os_host::guard::describe(code)))
        }
    }
}

/// Scans file `path` in this process (native files, whose libraries load nothing until used; tests).
pub fn scan_here(path: &Path, format: Format) -> Scanned {
    match crate::plugins::load_file(path, format) {
        Ok(lib) => Scanned::ok(lib.plugins().iter().map(Record::of).collect()),
        Err(e) => Scanned::failed(Status::Error, e),
    }
}

/// How the sweep scans one file: native files in this process (nothing is loaded), the others in a scanner process.
fn scan_file(path: &Path, format: Format) -> Scanned {
    // Tests scan in this process (a test program is no scanner).
    if format == Format::Native || cfg!(test) { scan_here(path, format) } else { scan_in_child(path, format) }
}

/// What the sweep thread tells the UI thread.
pub enum Msg {
    Progress { done: usize, total: usize, file: String },
    Done(Cache),
}

/// A running sweep.
pub struct Sweep {
    rx: mpsc::Receiver<Msg>,
}

impl Sweep {
    /// A sweep that never reports (tests: "a scan is running").
    #[cfg(test)]
    pub fn idle() -> Sweep {
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        Sweep { rx }
    }
}

/// Starts a sweep on a background thread: finds the plugin files of `prefs`' folders, scans what `mode` asks for
/// (keeping `old`'s results for the rest) and sends the new cache.
pub fn start(prefs: &db::Prefs, old: Cache, mode: Mode) -> Sweep {
    let (tx, rx) = mpsc::channel();
    let mut dirs = db::search_dirs(prefs);
    let rescan_errors = prefs.rescan_errors;
    let mut native = crate::plugins::native_files();
    // Tests never search the machine's own plugin folders, only the folders a test added.
    if cfg!(test) {
        dirs.retain(|d| d.kind == db::DirKind::Added);
        native.clear();
    }
    std::thread::Builder::new()
        .name("plugin scan".into())
        .spawn(move || {
            let found = db::discover(&dirs, native);
            let _ = tx.send(Msg::Progress { done: 0, total: 0, file: String::new() });
            let progress = |done: usize, total: usize, p: &Path| {
                let file = p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned());
                let _ = tx.send(Msg::Progress { done, total, file });
                os_host::wake();
            };
            let cache = db::sweep(&found, &old, mode, rescan_errors, WORKERS, &scan_file, &progress);
            let _ = tx.send(Msg::Done(cache));
            os_host::wake();
        })
        .expect("plugin scan thread");
    Sweep { rx }
}

impl App {
    /// Startup (spec: ui/plugin-manager.md §4 "When"): the built-in plugins and the cached plugins are listed at once
    /// (no plugin file is loaded), then a background scan starts when "Find new plugins at startup" is on (always
    /// when there is no cache yet).
    pub fn start_plugins(&mut self) {
        let first = !self.load_plugins();
        if first || self.state.plugdb.prefs.startup_scan {
            self.start_scan(Mode::Changed);
        }
    }

    /// The built-in plugins plus the last scan's, and the user's choices. False when there is no scan cache yet.
    pub fn load_plugins(&mut self) -> bool {
        let (owner, scale) = (self.plugins.owner, self.plugins.scale);
        self.plugins = crate::plugins::Plugins::builtins();
        (self.plugins.owner, self.plugins.scale) = (owner, scale);
        let db = &mut self.state.plugdb;
        db.prefs = db::Prefs::load();
        let cached = db::Cache::load();
        let found = cached.is_some();
        db.cache = cached.unwrap_or_default();
        self.plugins.merge(&self.state.plugdb.cache);
        self.state.plugdb.rebuild(&self.plugins);
        found
    }

    /// Starts a scan, or queues it after the running one (one at a time).
    pub fn start_scan(&mut self, mode: Mode) {
        if self.sweep.is_some() {
            let q = &mut self.scan_queued;
            *q = Some(if *q == Some(Mode::All) { Mode::All } else { mode });
            return;
        }
        let db = &mut self.state.plugdb;
        let first = db.cache.files.is_empty();
        db.progress = Some(db::Progress { first, ..db::Progress::default() });
        if first {
            self.state.message = Some("Looking for plugins...".into());
        }
        self.sweep = Some(start(&self.state.plugdb.prefs, self.state.plugdb.cache.clone(), mode));
        self.invalidate();
    }

    /// UI tick: the running scan's progress and results.
    pub(crate) fn scan_tick(&mut self) {
        let Some(sw) = &self.sweep else { return };
        let mut done = None;
        let mut moved = false;
        while let Ok(m) = sw.rx.try_recv() {
            match m {
                Msg::Progress { done, total, file } => {
                    if let Some(p) = &mut self.state.plugdb.progress {
                        (p.done, p.total, p.file) = (done, total, file);
                    }
                    moved = true;
                }
                Msg::Done(c) => done = Some(c),
            }
        }
        let Some(cache) = done else {
            if moved && self.state.settings.open {
                self.invalidate();
            }
            return;
        };
        self.sweep = None;
        self.apply_scan(cache);
        if let Some(mode) = self.scan_queued.take() {
            self.start_scan(mode);
        }
    }

    /// A scan ended: its results are kept, the lists updated in place, the hint bar says what changed.
    fn apply_scan(&mut self, cache: Cache) {
        let report = db::compare(&self.state.plugdb.cache, &cache);
        cache.save();
        self.plugins.merge(&cache);
        let db = &mut self.state.plugdb;
        db.cache = cache;
        db.progress = None;
        db.rebuild(&self.plugins);
        self.state.message = Some(report.message());
        self.invalidate();
    }

    /// The user changed a plugin choice: saved, and the lists follow.
    pub(crate) fn prefs_changed(&mut self) {
        self.state.plugdb.prefs.save();
        self.state.plugdb.rebuild(&self.plugins);
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_lines_parse() {
        let ok = parse(r#"{"ok":true,"plugins":[{"id":"a","name":"A","vendor":"V","version":"1","features":["instrument"],"kind":"instrument"}]}"#);
        assert_eq!(ok.status, Status::Ok);
        assert_eq!(ok.plugins[0].name, "A");
        let bad = parse(r#"{"ok":false,"error":"no clap_entry","crashed":false}"#);
        assert_eq!((bad.status, bad.error.as_str()), (Status::Error, "no clap_entry"));
        assert_eq!(parse(r#"{"ok":false,"error":"x","crashed":true}"#).status, Status::Crashed);
        assert_eq!(parse("garbage").status, Status::Error);
    }

    /// A file that is not a plugin fails in this process without harm.
    #[test]
    fn a_broken_file_scans_as_an_error() {
        let p = std::env::temp_dir().join(format!("os_scan_broken_{}.clap", std::process::id()));
        std::fs::write(&p, b"not a plugin").unwrap();
        let r = scan_here(&p, Format::Clap);
        assert_eq!(r.status, Status::Error);
        let _ = std::fs::remove_file(p);
    }
}
