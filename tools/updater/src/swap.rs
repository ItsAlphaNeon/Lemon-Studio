// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Putting a downloaded build in place of the installed one, once Lemon Studio has closed, and starting it again.
//!
//! - Windows: the `.exe`. A running program's file cannot be overwritten but can be renamed: the old one goes to
//!   `<name>.old` (Lemon Studio deletes it at its next start, should something still hold it now), the new one takes
//!   its name.
//! - Linux: the AppImage, replaced at once by a rename over it.
//! - macOS: the `.app` bundle, a folder: the new one is copied next to it with `ditto` (which keeps the signature), the
//!   old one moved aside, the new one moved in, the old one deleted.
//!
//! The new copy is written next to the target first, so the last step is a rename within one folder. Should the
//! last step fail, the old program is put back.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Waits up to `timeout` for process `pid` to end; true once it has.
pub fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        // SAFETY: a handle opened here is waited on and closed here.
        unsafe {
            let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if h.is_null() {
                return true; // gone already (or never ours to wait for)
            }
            let r = WaitForSingleObject(h, timeout.as_millis().min(u32::MAX as u128 - 1) as u32);
            CloseHandle(h);
            r == WAIT_OBJECT_0
        }
    }
    #[cfg(unix)]
    {
        let start = Instant::now();
        loop {
            // SAFETY: signal 0 only checks that the process exists.
            let gone = unsafe { libc::kill(pid as libc::pid_t, 0) } != 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if gone || zombie(pid) {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = (pid, timeout, Instant::now());
        true
    }
}

/// Linux: a process that has ended but that its parent has not reaped yet still answers signal 0.
#[cfg(unix)]
fn zombie(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        // `/proc/<pid>/stat`: "pid (name) state ...": the state follows the last ')'.
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|s| s.rfind(')').and_then(|i| s[i + 1..].trim_start().chars().next()))
            .is_some_and(|c| c == 'Z' || c == 'X')
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// Puts `staged` (a file, or a `.app` folder) in place of `target`.
pub fn replace(staged: &Path, target: &Path) -> Result<(), String> {
    if staged.is_dir() { replace_bundle(staged, target) } else { replace_file(staged, target) }
}

fn beside(target: &Path, suffix: &str) -> Result<PathBuf, String> {
    let name = target.file_name().ok_or_else(|| format!("{}: no file name", target.display()))?.to_string_lossy();
    let parent = target.parent().ok_or_else(|| format!("{}: no folder", target.display()))?;
    Ok(parent.join(format!(".{name}.{suffix}")))
}

/// A rename, tried for a few seconds: on Windows a virus scanner or the old process's last handles can hold the file
/// for a moment after it ends.
fn rename(from: &Path, to: &Path) -> Result<(), String> {
    let start = Instant::now();
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if start.elapsed() > Duration::from_secs(10) => {
                return Err(format!("{} to {}: {e}", from.display(), to.display()));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

fn replace_file(staged: &Path, target: &Path) -> Result<(), String> {
    let new = beside(target, "new")?;
    let _ = std::fs::remove_file(&new);
    std::fs::copy(staged, &new).map_err(|e| format!("{}: {e}", new.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}: {e}", new.display()))?;
    }
    #[cfg(windows)]
    {
        let old = old_exe(target);
        let _ = std::fs::remove_file(&old);
        if let Err(e) = rename(target, &old) {
            let _ = std::fs::remove_file(&new);
            return Err(e);
        }
        if let Err(e) = rename(&new, target) {
            let _ = std::fs::rename(&old, target);
            let _ = std::fs::remove_file(&new);
            return Err(e);
        }
        let _ = std::fs::remove_file(&old);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        rename(&new, target).inspect_err(|_| {
            let _ = std::fs::remove_file(&new);
        })
    }
}

/// Windows: where the replaced program goes (Lemon Studio deletes it at its next start).
#[cfg(windows)]
pub fn old_exe(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

fn replace_bundle(staged: &Path, target: &Path) -> Result<(), String> {
    let (new, old) = (beside(target, "new")?, beside(target, "old")?);
    let _ = std::fs::remove_dir_all(&new);
    let _ = std::fs::remove_dir_all(&old);
    copy_tree(staged, &new)?;
    if let Err(e) = rename(target, &old) {
        let _ = std::fs::remove_dir_all(&new);
        return Err(e);
    }
    if let Err(e) = rename(&new, target) {
        let _ = std::fs::rename(&old, target);
        let _ = std::fs::remove_dir_all(&new);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// A folder copied with its links, attributes and signature: `ditto` on macOS, `cp -a` elsewhere.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("ditto");
        c.arg(from).arg(to);
        c
    } else {
        let mut c = Command::new("cp");
        c.arg("-a").arg(from).arg(to);
        c
    };
    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("copying {} failed ({s})", from.display())),
        Err(e) => Err(format!("copying {}: {e}", from.display())),
    }
}

/// Starts the program at `target` (the `.exe`, the AppImage or the `.app`), on its own.
pub fn relaunch(target: &Path) -> Result<(), String> {
    let mut cmd = if cfg!(target_os = "macos") && target.is_dir() {
        let mut c = Command::new("open");
        c.arg(target);
        c
    } else {
        Command::new(target)
    };
    if let Some(dir) = target.parent() {
        cmd.current_dir(dir);
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().map(drop).map_err(|e| format!("starting {}: {e}", target.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lemonstudio-updater-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_file_is_replaced_in_place() {
        let d = temp("file");
        let (staged, target) = (d.join("staged.bin"), d.join("Lemon Studio.exe"));
        std::fs::write(&staged, b"new").unwrap();
        std::fs::write(&target, b"old").unwrap();
        replace(&staged, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        // Only the program is left (the staged copy is the caller's to delete).
        let mut left: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["Lemon Studio.exe", "staged.bin"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_bundle_is_replaced_in_place() {
        let d = temp("bundle");
        let (staged, target) = (d.join("staged/Lemon Studio.app"), d.join("Lemon Studio.app"));
        std::fs::create_dir_all(staged.join("Contents/MacOS")).unwrap();
        std::fs::write(staged.join("Contents/MacOS/lemonstudio"), b"new").unwrap();
        std::fs::create_dir_all(target.join("Contents/MacOS")).unwrap();
        std::fs::write(target.join("Contents/MacOS/lemonstudio"), b"old").unwrap();
        std::fs::write(target.join("Contents/stale"), b"gone after").unwrap();
        replace(&staged, &target).unwrap();
        assert_eq!(std::fs::read(target.join("Contents/MacOS/lemonstudio")).unwrap(), b"new");
        assert!(!target.join("Contents/stale").exists());
        assert!(!d.join(".Lemon Studio.app.old").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn an_ended_process_is_not_waited_for() {
        let mut child = if cfg!(windows) {
            Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap()
        } else {
            Command::new("true").spawn().unwrap()
        };
        let pid = child.id();
        child.wait().unwrap();
        assert!(wait_for_exit(pid, Duration::from_secs(5)));
    }
}
