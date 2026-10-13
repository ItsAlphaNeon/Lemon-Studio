//! The toolbar's CPU panel: how hard the audio engine works (time spent rendering per second of audio, and its recent
//! history), the notes playing, the memory the program uses and the dropouts (device blocks the engine could not fill
//! in time, heard as gaps). A click opens Settings > Audio.
// spec: ui/main-window.md "CPU panel"

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A reading is taken this often.
pub const EVERY: Duration = Duration::from_millis(250);
/// Readings kept for the load graph (one 2 px column each, newest on the right).
pub const HISTORY: usize = 37;

/// What the panel shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    /// Audio load, percent (0 while nothing plays; past 100 the engine cannot keep up).
    pub load: u32,
    pub notes: usize,
    /// Memory in use, MB (None where the system does not say).
    pub mem_mb: Option<u64>,
    pub dropouts: u64,
}

#[derive(Debug, Default)]
pub struct Cpu {
    pub now: Reading,
    /// Load readings, percent, oldest first.
    pub history: VecDeque<u32>,
    at: Option<Instant>,
}

impl Cpu {
    /// Takes a reading when one is due: the engine's load (None: nothing plays), the notes playing and the dropouts so
    /// far. Returns whether the panel changed.
    pub fn update(&mut self, now: Instant, load: Option<f32>, notes: usize, dropouts: u64) -> bool {
        if self.at.is_some_and(|t| now < t + EVERY) {
            return false;
        }
        self.at = Some(now);
        self.read(load, notes, memory_mb(), dropouts)
    }

    fn read(&mut self, load: Option<f32>, notes: usize, mem_mb: Option<u64>, dropouts: u64) -> bool {
        let load = load.map_or(0, |l| (l * 100.0).round().clamp(0.0, 999.0) as u32);
        // An idle graph stays the same when another zero comes in.
        let moved = load != 0 || self.history.iter().any(|&h| h != 0);
        self.history.push_back(load);
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        let r = Reading { load, notes, mem_mb, dropouts };
        let changed = moved || r != self.now;
        self.now = r;
        changed
    }

    /// The hint over the panel.
    pub fn hint(&self) -> String {
        let r = &self.now;
        let mem = r.mem_mb.map_or(String::new(), |m| format!(", {m} MB of memory in use"));
        let drop = match r.dropouts {
            0 => "no dropouts".to_string(),
            1 => "1 dropout".to_string(),
            n => format!("{n} dropouts"),
        };
        format!("Audio load {}%, {} notes playing{mem}, {drop} (click: audio settings)", r.load, r.notes)
    }
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

#[cfg(not(any(windows, target_os = "linux")))]
pub fn memory_mb() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_fill_the_graph_and_idle_stays_quiet() {
        let mut c = Cpu::default();
        assert!(c.read(None, 0, Some(100), 0), "the first reading shows the memory");
        assert!(!c.read(None, 0, Some(100), 0), "nothing changed");
        assert!(c.read(Some(0.123), 4, Some(100), 0));
        assert_eq!((c.now.load, c.now.notes), (12, 4));
        // After playback the graph scrolls the old load out, then goes quiet.
        let mut changes = 0;
        for _ in 0..HISTORY + 5 {
            changes += usize::from(c.read(None, 0, Some(100), 0));
        }
        assert_eq!(changes, HISTORY, "every reading moves the graph until the load has left it");
        assert_eq!(c.history.len(), HISTORY);
        assert!(c.read(None, 0, Some(100), 1), "a dropout shows");
        assert!(c.hint().contains("1 dropout") && c.hint().contains("100 MB"));
        assert!(memory_mb().is_some_and(|m| m > 0) || cfg!(not(any(windows, target_os = "linux"))));
    }
}
