// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The toolbar's CPU panel: how hard the audio engine works (time spent rendering per second of audio, and its recent
//! history), the voices playing (or the interface's frames per second), the memory the program uses and the dropouts
//! (device blocks the engine could not fill in time, heard as gaps). Right-click: its menu; double-click: the Plugin
//! performance monitor.
// spec: ui/main-window.md §13 (CPU panel), audio/voices.md §5

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Readings kept for the load graph (one 2 px column each, newest on the right).
pub const HISTORY: usize = 37;

/// How often a reading is taken (the panel's Refresh menu).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Refresh {
    Slow,
    #[default]
    Medium,
    Fast,
}

impl Refresh {
    pub const ALL: [Refresh; 3] = [Refresh::Slow, Refresh::Medium, Refresh::Fast];

    pub fn every(self) -> Duration {
        Duration::from_millis(match self {
            Refresh::Slow => 1000,
            Refresh::Medium => 250,
            Refresh::Fast => 100,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Refresh::Slow => "Slow",
            Refresh::Medium => "Medium",
            Refresh::Fast => "Fast",
        }
    }
}

/// The panel menu's commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuCmd {
    Refresh(Refresh),
    /// View FPS (true) or the voice count (false).
    Fps(bool),
    Misc,
    Monitor,
    AudioSettings,
}

/// The troubleshooting readings shown with Misc: handles, threads, fonts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Misc {
    pub handles: u32,
    pub threads: u32,
    pub fonts: u32,
}

/// What the panel shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// Audio load, percent (0 while nothing plays; past 100 the engine cannot keep up).
    pub load: u32,
    /// Voices being mixed now: the engine's and every hosted instrument's.
    pub voices: usize,
    /// Frames the interface painted per second over the reading.
    pub fps: u32,
    /// Memory in use, MB (None where the system does not say).
    pub mem_mb: Option<u64>,
    pub dropouts: u64,
    /// Handles / threads / fonts, while Misc is ticked.
    pub misc: Option<Misc>,
}

#[derive(Debug, Default)]
pub struct Cpu {
    pub now: Reading,
    /// Load readings, percent, oldest first.
    pub history: VecDeque<u32>,
    pub refresh: Refresh,
    /// View FPS instead of the voice count.
    pub fps_view: bool,
    /// Misc ticked: handles, threads and fonts in the history's place.
    pub misc: bool,
    at: Option<Instant>,
    /// Frames painted since the last reading.
    frames: u32,
}

impl Cpu {
    /// The panel with the settings kept in the user folder (`cpu_panel.txt`).
    pub fn saved() -> Cpu {
        let mut c = Cpu::default();
        if !cfg!(test) {
            if let Some(t) = os_paths::user_file(SETTINGS).and_then(|f| std::fs::read_to_string(f).ok()) {
                c.parse(&t);
            }
        }
        c
    }

    fn parse(&mut self, t: &str) {
        for line in t.lines() {
            match line.trim().split_once('=') {
                Some(("refresh", v)) => self.refresh = Refresh::ALL.into_iter().find(|r| r.label().eq_ignore_ascii_case(v.trim())).unwrap_or_default(),
                Some(("view", v)) => self.fps_view = v.trim() == "fps",
                Some(("misc", v)) => self.misc = v.trim() == "1",
                _ => {}
            }
        }
    }

    fn text(&self) -> String {
        format!("refresh={}\nview={}\nmisc={}\n", self.refresh.label(), if self.fps_view { "fps" } else { "voices" }, u8::from(self.misc))
    }

    /// Keeps the panel's settings for the next start.
    pub fn save(&self) {
        if cfg!(test) {
            return;
        }
        if let Some(dir) = os_paths::user_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(SETTINGS), self.text());
        }
    }

    /// A menu command that changes the panel (the others are the app's). The next reading comes at once.
    pub fn set(&mut self, c: CpuCmd) {
        match c {
            CpuCmd::Refresh(r) => self.refresh = r,
            CpuCmd::Fps(f) => self.fps_view = f,
            CpuCmd::Misc => self.misc = !self.misc,
            CpuCmd::Monitor | CpuCmd::AudioSettings => return,
        }
        self.at = None;
        self.frames = 0;
        self.save();
    }

    /// The interface painted a frame.
    pub fn frame(&mut self) {
        self.frames = self.frames.saturating_add(1);
    }

    /// A reading is due.
    pub fn due(&self, now: Instant) -> bool {
        self.at.is_none_or(|t| now >= t + self.refresh.every())
    }

    /// Takes a reading when one is due: the engine's load (None: nothing plays), the voices playing and the dropouts
    /// so far. Returns whether the panel changed.
    pub fn update(&mut self, now: Instant, load: Option<f32>, voices: usize, dropouts: u64) -> bool {
        if !self.due(now) {
            return false;
        }
        let secs = self.at.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
        let fps = if secs > 0.0 { (f64::from(self.frames) / secs).round() as u32 } else { 0 };
        self.at = Some(now);
        self.frames = 0;
        let misc = self.misc.then(misc_now);
        self.read(load, voices, fps, memory_mb(), dropouts, misc)
    }

    fn read(&mut self, load: Option<f32>, voices: usize, fps: u32, mem_mb: Option<u64>, dropouts: u64, misc: Option<Misc>) -> bool {
        let load = load.map_or(0, |l| (l * 100.0).round().clamp(0.0, 999.0) as u32);
        // An idle graph stays the same when another zero comes in.
        let moved = load != 0 || self.history.iter().any(|&h| h != 0);
        self.history.push_back(load);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        // Frames per second only matter while they are shown: else a repaint would cause the next one.
        let fps = if self.fps_view { fps } else { 0 };
        let r = Reading { load, voices, fps, mem_mb, dropouts, misc };
        let changed = moved || r != self.now;
        self.now = r;
        changed
    }

    /// The hint over the panel.
    pub fn hint(&self) -> String {
        let r = &self.now;
        let shown = if self.fps_view {
            format!("{} frames per second", r.fps)
        } else {
            match r.voices {
                1 => "1 voice".to_string(),
                n => format!("{n} voices"),
            }
        };
        let mem = r.mem_mb.map_or(String::new(), |m| format!(", {m} MB of memory in use"));
        let misc = r.misc.map_or(String::new(), |m| format!(", {} handles, {} threads, {} fonts", m.handles, m.threads, m.fonts));
        let drop = match r.dropouts {
            0 => "no dropouts".to_string(),
            1 => "1 dropout".to_string(),
            n => format!("{n} dropouts"),
        };
        format!("Audio load {}%, {shown}{mem}{misc}, {drop} (double-click: plugin performance; right-click: options)", r.load)
    }
}

/// The panel's settings file in the user folder.
const SETTINGS: &str = "cpu_panel.txt";

/// The handles, threads and fonts the program holds now.
fn misc_now() -> Misc {
    let (handles, threads) = handles_threads();
    Misc { handles, threads, fonts: os_ui::font_count() as u32 }
}

/// Kernel handles the program has open, and its threads.
#[cfg(windows)]
fn handles_threads() -> (u32, u32) {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessHandleCount};
    // SAFETY: queries on our own process; the snapshot handle is closed before returning.
    unsafe {
        let mut handles = 0u32;
        GetProcessHandleCount(GetCurrentProcess(), &mut handles);
        let mut threads = 0;
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap != INVALID_HANDLE_VALUE {
            let me = GetCurrentProcessId();
            let mut e: PROCESSENTRY32W = std::mem::zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e) != 0;
            while ok {
                if e.th32ProcessID == me {
                    threads = e.cntThreads;
                    break;
                }
                ok = Process32NextW(snap, &mut e) != 0;
            }
            CloseHandle(snap);
        }
        (handles, threads)
    }
}

/// Open file descriptors and threads.
#[cfg(target_os = "linux")]
fn handles_threads() -> (u32, u32) {
    let fds = std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count() as u32);
    let threads = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Threads:").and_then(|v| v.trim().parse().ok())))
        .unwrap_or(0);
    (fds, threads)
}

/// Open file descriptors and threads.
#[cfg(target_os = "macos")]
fn handles_threads() -> (u32, u32) {
    let pid = std::process::id() as libc::c_int;
    // SAFETY: with a null buffer `proc_pidinfo` only reports the bytes the descriptor list needs.
    let fd_bytes = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    let fds = (fd_bytes.max(0) as usize / std::mem::size_of::<libc::proc_fdinfo>()) as u32;
    (fds, task_info().map_or(0, |t| t.pti_threadnum as u32))
}

/// The task's counters (threads, resident memory).
#[cfg(target_os = "macos")]
fn task_info() -> Option<libc::proc_taskinfo> {
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: a plain query on our own process into a struct of the size we pass.
    unsafe {
        let mut t: libc::proc_taskinfo = std::mem::zeroed();
        let got = libc::proc_pidinfo(std::process::id() as libc::c_int, libc::PROC_PIDTASKINFO, 0, (&raw mut t).cast(), size);
        (got == size).then_some(t)
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn handles_threads() -> (u32, u32) {
    (0, 0)
}

/// Memory the program uses (its working set), MB.
#[cfg(windows)]
pub fn memory_mb() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: a plain query on our own process into a counters struct of the size we pass.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        (K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0).then(|| c.WorkingSetSize as u64 >> 20)
    }
}

/// Memory the program uses (resident pages), MB.
#[cfg(target_os = "linux")]
pub fn memory_mb() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let pages: u64 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(pages * 4096 >> 20)
}

/// Memory the program uses (resident size), MB.
#[cfg(target_os = "macos")]
pub fn memory_mb() -> Option<u64> {
    task_info().map(|t| t.pti_resident_size >> 20)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn memory_mb() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_fill_the_graph_and_idle_stays_quiet() {
        let mut c = Cpu::default();
        assert!(c.read(None, 0, 0, Some(100), 0, None), "the first reading shows the memory");
        assert!(!c.read(None, 0, 0, Some(100), 0, None), "nothing changed");
        assert!(c.read(Some(0.123), 4, 0, Some(100), 0, None));
        assert_eq!((c.now.load, c.now.voices), (12, 4));
        // After playback the graph scrolls the old load out, then goes quiet.
        let mut changes = 0;
        for _ in 0..HISTORY + 5 {
            changes += usize::from(c.read(None, 0, 0, Some(100), 0, None));
        }
        assert_eq!(changes, HISTORY, "every reading moves the graph until the load has left it");
        assert_eq!(c.history.len(), HISTORY);
        assert!(c.read(None, 0, 0, Some(100), 1, None), "a dropout shows");
        assert!(c.hint().contains("1 dropout") && c.hint().contains("100 MB") && c.hint().contains("0 voices"));
        assert!(memory_mb().is_some_and(|m| m > 0) || cfg!(not(any(windows, target_os = "linux", target_os = "macos"))));
    }

    /// Refresh rates, the FPS view and Misc (spec: ui/main-window.md §13).
    #[test]
    fn refresh_view_and_misc() {
        let t0 = Instant::now();
        let mut c = Cpu::default();
        assert!(c.update(t0, None, 3, 0), "the first reading at once");
        assert!(!c.due(t0 + Duration::from_millis(200)), "Medium = 250 ms");
        assert!(c.due(t0 + Duration::from_millis(250)));
        c.set(CpuCmd::Refresh(Refresh::Slow));
        assert!(c.due(t0), "a new rate reads at once");
        c.update(t0, None, 3, 0);
        assert!(!c.due(t0 + Duration::from_millis(900)) && c.due(t0 + Duration::from_secs(1)));
        c.set(CpuCmd::Fps(true));
        c.update(t0, None, 3, 0);
        for _ in 0..30 {
            c.frame();
        }
        assert!(c.update(t0 + Duration::from_millis(1000), None, 3, 0));
        assert_eq!(c.now.fps, 30);
        assert!(c.hint().contains("30 frames per second"), "{}", c.hint());
        c.set(CpuCmd::Misc);
        c.update(t0 + Duration::from_millis(1000), None, 3, 0);
        let m = c.now.misc.expect("Misc readings");
        if cfg!(any(windows, target_os = "linux", target_os = "macos")) {
            assert!(m.handles > 0 && m.threads > 0, "{m:?}");
        }
        assert!(c.hint().contains("threads"));
        // Kept as text.
        let mut d = Cpu::default();
        d.parse(&c.text());
        assert_eq!((d.refresh, d.fps_view, d.misc), (Refresh::Slow, true, true));
    }
}
