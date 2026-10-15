// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Trying a native plugin in a separate process before its first load in this one (spec plugins/fruity-abi.md,
//! "Crash containment").
//!
//! `guard` contains the errors a plugin raises, but a plugin that hangs while it is created, or damages the process in
//! a way no handler sees, would still take the host with it. So before a plugin DLL is first created in the host, a
//! child process (the host's own program, started with [`CHECK_ARG`], the DLL path and a result file) creates it,
//! activates it, renders one block, saves and restores its state and destroys it. On Windows the child runs on a
//! desktop of its own and fails as soon as the plugin shows a message box, so nothing reaches the user's screen. The
//! verdict is remembered per DLL (path, size, modification time) in a small text file, so each version of a plugin is
//! checked once.
//
// spec: plugins/fruity-abi.md

use std::path::{Path, PathBuf}; // This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use os_host::Library as _;

/// The program argument that runs the check in a child process: `<program> --native-check <dll> [<result file>]`.
pub const CHECK_ARG: &str = "--native-check";
/// How long a child may take before the plugin counts as hanging.
const TIMEOUT: Duration = Duration::from_secs(15);
/// The rate the child activates the plugin at.
const RATE: u32 = 48_000;

/// Whether a plugin can be loaded into the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Works,
    /// It does not: why, for the user, short ("plugin error", "hangs", "plugin error while saving its state").
    Fails(String),
}

/// The child's side of the check, given the arguments after [`CHECK_ARG`] (the DLL, then the result file): tries the
/// plugin and returns the process exit code (0 = it works). A failure writes its reason to the result file (to
/// standard output without one) and returns 2. End the process with [`crate::guard::exit`] (the host's guard): a
/// stopped plugin's library can crash while a normal exit unloads it.
pub fn run(args: &[String]) -> i32 {
    let Some(dll) = args.first().map(Path::new) else { return 2 };
    let out = args.get(1).map(PathBuf::from);
    #[cfg(windows)]
    win::quiet(out.clone());
    let fail = |why: String| {
        report(out.as_deref(), &why);
        2
    };
    let lib = match crate::NativeLibrary::load(dll) {
        Ok(l) => l,
        Err(e) => return fail(reason_of(&e)),
    };
    let Some(desc) = lib.plugins().first().cloned() else { return fail("no plugin in the file".into()) };
    let plugin = match lib.create(&desc.id) {
        Ok(p) => p,
        Err(e) => return fail(reason_of(&e)),
    };
    let mut p = plugin.lock().unwrap_or_else(|e| e.into_inner());
    if !p.activate(RATE) {
        return fail(stop_reason(p.stopped(), " while starting"));
    }
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
        *a = (i as f32 * 0.06).sin() * 0.25;
        *b = *a;
    }
    if desc.is_instrument() {
        // A note: started, held, released, then everything stopped.
        p.note_on(0, 60, 100.0 / 128.0);
        for _ in 0..8 {
            p.process(RATE, &mut l, &mut r);
        }
        p.note_off(0, 60);
        for _ in 0..8 {
            p.process(RATE, &mut l, &mut r);
        }
        p.reset();
    } else {
        p.process_fx(RATE, &mut l, &mut r);
    }
    if p.stopped().is_some() {
        return fail(stop_reason(p.stopped(), " while processing sound"));
    }
    let Some(state) = p.save_state() else { return fail(stop_reason(p.stopped(), " while saving its state")) };
    if !p.load_state(&state) {
        return fail(stop_reason(p.stopped(), " while reading its state"));
    }
    drop(p);
    drop(plugin);
    0
}

/// Writes the reason of a failed check where the parent reads it.
fn report(out: Option<&Path>, why: &str) {
    match out {
        Some(f) => {
            let _ = std::fs::write(f, why);
        }
        None => println!("{why}"),
    }
}

/// Why the plugin stopped, from its exception code and the text its runtime gave (this process tries one plugin on
/// one thread, so the guard's last message is its).
fn stop_reason(code: Option<u32>, when: &str) -> String {
    let code = code.unwrap_or(crate::guard::RUNTIME_ERROR);
    format!("{}{when}", crate::guard::describe_with(code, &crate::guard::last_message()))
}

/// A load error as a short reason: without the DLL path, and loading goes without saying ("plugin error").
fn reason_of(e: &str) -> String {
    let e = e.split_once(".dll: ").map_or(e, |(_, rest)| rest);
    e.strip_suffix(" while loading").unwrap_or(e).to_string()
}

/// The verdicts found so far, kept in a text file (one line per DLL).
#[derive(Debug, Default)]
pub struct Checks {
    file: Option<PathBuf>,
    known: Vec<Known>,
    /// Checks running on a thread (`ready`): the DLL and the child's verdict.
    running: Vec<(PathBuf, std::thread::JoinHandle<Option<Verdict>>)>,
}

#[derive(Debug, Clone)]
struct Known {
    dll: PathBuf,
    stamp: (u64, u64),
    verdict: Verdict,
}

impl Checks {
    /// The verdicts remembered in `file` (none when it is missing or `None`).
    pub fn load(file: Option<PathBuf>) -> Checks {
        let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default();
        let known = text.lines().filter_map(parse).collect();
        Checks { file, known, running: Vec::new() }
    }

    /// The remembered verdict for this version of `dll`, if it was checked.
    pub fn known(&self, dll: &Path) -> Option<Verdict> {
        let stamp = stamp(dll)?;
        self.known.iter().find(|k| k.dll == dll && k.stamp == stamp).map(|k| k.verdict.clone())
    }

    /// The verdict for `dll`: remembered, or found now by running `program` (the host's own executable) as a child
    /// with [`CHECK_ARG`]. A child that cannot be started gives no verdict to remember, and the plugin counts as
    /// working (its calls are still guarded).
    pub fn verdict(&mut self, program: &Path, dll: &Path) -> Verdict {
        if let Some(v) = self.known(dll) {
            return v;
        }
        let Some(verdict) = try_in_child(program, dll) else { return Verdict::Works };
        self.remember(dll, &verdict);
        verdict
    }

    /// Whether `verdict` for `dll` answers at once. When it does not, the check is started on a thread (or is
    /// already running) and this answers `true` once it has finished: callers that must not wait (the UI) ask again
    /// later.
    pub fn ready(&mut self, program: &Path, dll: &Path) -> bool {
        if self.known(dll).is_some() {
            return true;
        }
        if let Some(i) = self.running.iter().position(|(d, _)| d == dll) {
            if !self.running[i].1.is_finished() {
                return false;
            }
            let (_, h) = self.running.remove(i);
            if let Ok(Some(v)) = h.join() {
                self.remember(dll, &v);
            }
            return true;
        }
        let (program, path) = (program.to_path_buf(), dll.to_path_buf());
        self.running.push((dll.to_path_buf(), std::thread::spawn(move || try_in_child(&program, &path))));
        false
    }

    fn remember(&mut self, dll: &Path, verdict: &Verdict) {
        if let Some(stamp) = stamp(dll) {
            self.known.retain(|k| k.dll != dll);
            self.known.push(Known { dll: dll.to_path_buf(), stamp, verdict: verdict.clone() });
            self.save();
        }
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let mut text = String::new();
        for k in &self.known {
            let (state, why) = match &k.verdict {
                Verdict::Works => ("works", ""),
                Verdict::Fails(why) => ("fails", why.as_str()),
            };
            text += &format!("{state}\t{}\t{}\t{}\t{why}\n", k.stamp.0, k.stamp.1, k.dll.display());
        }
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, text);
    }
}

fn parse(line: &str) -> Option<Known> {
    let mut f = line.split('\t');
    let verdict = match f.next()? {
        "works" => Verdict::Works,
        "fails" => Verdict::Fails(String::new()),
        _ => return None,
    };
    let stamp = (f.next()?.parse().ok()?, f.next()?.parse().ok()?);
    let dll = PathBuf::from(f.next()?);
    let verdict = match verdict {
        Verdict::Fails(_) => Verdict::Fails(f.next().unwrap_or_default().to_string()),
        v => v,
    };
    Some(Known { dll, stamp, verdict })
}

/// Size and modification time (seconds) of `dll`.
fn stamp(dll: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(dll).ok()?;
    let t = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some((m.len(), t))
}

/// Runs the check for `dll` in a child process of `program`; `None` when the child cannot be started.
fn try_in_child(program: &Path, dll: &Path) -> Option<Verdict> {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let out = std::env::temp_dir().join(format!("lemon-native-check-{}-{n}.txt", std::process::id()));
    let _ = std::fs::remove_file(&out);
    let args = [CHECK_ARG.as_ref(), dll.as_os_str(), out.as_os_str()];
    #[cfg(windows)]
    let code = win::run_hidden(program, &args, TIMEOUT)?;
    #[cfg(not(windows))]
    let code = run_plain(program, &args, TIMEOUT)?;
    let why = std::fs::read_to_string(&out).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let _ = std::fs::remove_file(&out);
    Some(match code {
        Some(0) => Verdict::Works,
        None => Verdict::Fails("hangs".into()),
        // A written reason wins over the exit code (the process may still have crashed while ending).
        Some(code) => Verdict::Fails(why.unwrap_or_else(|| match code {
            2 => "plugin error".into(),
            c => format!("crashes, {}", crate::guard::describe(c)),
        })),
    })
}

/// Runs `program` with `args` and waits up to `timeout`: `None` when it cannot be started, `Some(None)` when it was
/// still running (it is killed), else its exit code.
#[cfg(not(windows))]
fn run_plain(program: &Path, args: &[&std::ffi::OsStr], timeout: Duration) -> Option<Option<u32>> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return Some(Some(s.code().map_or(1, |c| c as u32))),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Some(None);
            }
            Err(_) => return None,
        }
    }
}

/// Windows: the child runs on a desktop of its own, without error dialogs, and fails on a plugin's message box.
#[cfg(windows)]
mod win {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, HWND, LPARAM, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SEM_NOOPENFILEERRORBOX,
    };
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, CreateDesktopW};
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, TerminateProcess, WaitForSingleObject,
        CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain([0]).collect()
    }

    /// Runs `program` with `args` on a new desktop nobody sees and waits up to `timeout`: `None` when it cannot be
    /// started, `Some(None)` when it was still running (it is killed), else its exit code.
    pub fn run_hidden(program: &Path, args: &[&OsStr], timeout: Duration) -> Option<Option<u32>> {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let name = wide(format!("LemonStudioCheck{}-{n}", std::process::id()).as_ref());
        // Paths cannot hold quotes, so quoting every part is enough.
        let mut line = OsStr::new("\"").to_os_string();
        line.push(program);
        line.push("\"");
        for a in args {
            line.push(" \"");
            line.push(a);
            line.push("\"");
        }
        let mut line = wide(&line);
        unsafe {
            let desk = CreateDesktopW(name.as_ptr(), std::ptr::null(), std::ptr::null(), 0, GENERIC_ALL, std::ptr::null());
            if desk.is_null() {
                return None;
            }
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            si.lpDesktop = name.as_ptr() as *mut u16;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let started = CreateProcessW(
                std::ptr::null(),
                line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_NO_WINDOW,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            );
            if started == 0 {
                CloseDesktop(desk);
                return None;
            }
            let code = if WaitForSingleObject(pi.hProcess, timeout.as_millis() as u32) == WAIT_OBJECT_0 {
                let mut c = 0u32;
                GetExitCodeProcess(pi.hProcess, &mut c);
                Some(c)
            } else {
                TerminateProcess(pi.hProcess, 1);
                WaitForSingleObject(pi.hProcess, 5000);
                None
            };
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
            CloseDesktop(desk);
            Some(code)
        }
    }

    /// The child's side: no Windows error dialogs, and a watcher that ends the check when the plugin shows a message
    /// box (it would wait for a click nobody can give), with the box's text as the reason.
    pub fn quiet(out: Option<PathBuf>) {
        unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX) };
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(50));
            if let Some(text) = message_box() {
                super::report(out.as_deref(), &format!("shows \"{text}\""));
                unsafe { TerminateProcess(GetCurrentProcess(), 2) };
            }
        });
    }

    fn class(h: HWND) -> String {
        let mut b = [0u16; 64];
        let n = unsafe { GetClassNameW(h, b.as_mut_ptr(), b.len() as i32) };
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }

    fn text(h: HWND) -> String {
        let mut b = [0u16; 512];
        let n = unsafe { GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32) };
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }

    /// The text of a visible message box of this process (standard dialog or the plugin runtime's own), if one shows.
    fn message_box() -> Option<String> {
        unsafe extern "system" fn top(h: HWND, l: LPARAM) -> BOOL {
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(h, &mut pid) };
            if pid == unsafe { GetCurrentProcessId() }
                && unsafe { IsWindowVisible(h) } != 0
                && matches!(class(h).as_str(), "#32770" | "TMessageForm")
            {
                unsafe { (*(l as *mut Vec<HWND>)).push(h) };
            }
            1
        }
        unsafe extern "system" fn statics(h: HWND, l: LPARAM) -> BOOL {
            let t = text(h);
            if class(h) == "Static" && !t.trim().is_empty() {
                unsafe { (*(l as *mut Vec<String>)).push(t) };
            }
            1
        }
        let mut found: Vec<HWND> = Vec::new();
        unsafe { EnumWindows(Some(top), &mut found as *mut _ as LPARAM) };
        let h = *found.first()?;
        let mut lines: Vec<String> = Vec::new();
        unsafe { EnumChildWindows(h, Some(statics), &mut lines as *mut _ as LPARAM) };
        let t = if lines.is_empty() { text(h) } else { lines.join(" ") };
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(if t.is_empty() { "a message".into() } else { t.chars().take(120).collect() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("os-native-checks-{}", std::process::id()));
        let file = dir.join("checks.txt");
        let dll = std::env::current_exe().unwrap();
        let stamp = stamp(&dll).unwrap();
        let mut c = Checks::load(Some(file.clone()));
        assert_eq!(c.known(&dll), None);
        c.known.push(Known { dll: dll.clone(), stamp, verdict: Verdict::Fails("plugin error while saving its state".into()) });
        c.save();
        let again = Checks::load(Some(file));
        assert_eq!(again.known(&dll), Some(Verdict::Fails("plugin error while saving its state".into())));
        // Another version of the file (a different stamp) has no verdict.
        let mut old = again.known.clone();
        old[0].stamp.0 += 1;
        assert_eq!(Checks { file: None, known: old, running: Vec::new() }.known(&dll), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The whole check in a hidden child for the installed native plugins: `OS_NATIVE_CHECK_PROGRAM` = the app's
    /// program (it runs `--native-check`), `OS_NATIVE_CHECK` = comma-separated plugin folder names, or `all`. Prints one
    /// verdict per plugin; nothing shows on screen. Ignored: it loads the user's installed plugins.
    /// `cargo test -p os-native checks_in_a_hidden_child -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn checks_in_a_hidden_child() {
        let (Some(program), Some(names)) = (std::env::var_os("OS_NATIVE_CHECK_PROGRAM"), std::env::var("OS_NATIVE_CHECK").ok()) else {
            return;
        };
        let dlls: Vec<PathBuf> = if names == "all" {
            crate::dll_files()
        } else {
            names.split(',').filter_map(|n| crate::find_plugin(&crate::default_fl_dir(), n.trim())).collect()
        };
        assert!(!dlls.is_empty(), "no plugin found");
        for dll in dlls {
            let start = std::time::Instant::now();
            let v = try_in_child(Path::new(&program), &dll).expect("the child starts");
            let name = dll.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            println!("{:>5} ms  {name:40} {v:?}", start.elapsed().as_millis());
        }
    }

    #[test]
    fn a_load_error_becomes_a_short_reason() {
        let e = crate::NativeError::Crashed(PathBuf::from(r"C:\x\Y_x64.dll"), "plugin error".into()).to_string();
        assert_eq!(reason_of(&e), "plugin error");
        let e = crate::NativeError::Crashed(PathBuf::from(r"C:\x\Y_x64.dll"), "plugin error: No file: x".into()).to_string();
        assert_eq!(reason_of(&e), "plugin error: No file: x");
        assert_eq!(reason_of(r"C:\x\Y_x64.dll: GetProcAddress failed"), "GetProcAddress failed");
    }
}
