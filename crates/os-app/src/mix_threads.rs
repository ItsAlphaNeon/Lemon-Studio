//! The program's mixing threads (spec: audio/multicore.md): a pool of one fewer workers than the machine has physical
//! cores, which every engine the program builds renders on (playback, exports, renders from the command line), and
//! Audio ▸ CPU's two switches, "Multithreaded mixer processing" and "Multithreaded generator processing", which engines
//! read every tick.

use crate::prefs::{check, MULTITHREADED_GENERATORS, MULTITHREADED_MIXER};
use os_engine::parallel::{self, Pool};
use std::sync::Arc;

/// Starts the pool and applies the switches. `OS_MIX_THREADS=n` sets the number of workers (0: the program renders on
/// one thread).
pub fn start() {
    apply();
    let workers = match std::env::var("OS_MIX_THREADS").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        Some(n) => n,
        None => parallel::default_workers(physical_cores()),
    };
    if workers == 0 {
        return;
    }
    // The workers render for the device as the feeder thread does, so they run at its priority.
    let init: parallel::ThreadInit = Arc::new(crate::transport::raise_priority);
    parallel::set_global(Some(Pool::new(workers, Some(init))));
}

/// Applies the two switches as set (at startup, and when either is clicked).
pub fn apply() {
    parallel::configure(check(&MULTITHREADED_MIXER), check(&MULTITHREADED_GENERATORS));
}

/// The machine's physical cores, when the system says.
#[cfg(windows)]
fn physical_cores() -> Option<usize> {
    use windows_sys::Win32::System::SystemInformation::{GetLogicalProcessorInformation, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION};
    let mut len = 0u32;
    // SAFETY: the first call only reports the length the list needs; the second fills a buffer of that length.
    unsafe {
        GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len);
        let size = std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
        if len == 0 {
            return None;
        }
        let mut list: Vec<SYSTEM_LOGICAL_PROCESSOR_INFORMATION> = Vec::with_capacity(len as usize / size + 1);
        if GetLogicalProcessorInformation(list.as_mut_ptr(), &mut len) == 0 {
            return None;
        }
        list.set_len(len as usize / size);
        let cores = list.iter().filter(|p| p.Relationship == RelationProcessorCore).count();
        (cores > 0).then_some(cores)
    }
}

#[cfg(target_os = "macos")]
fn physical_cores() -> Option<usize> {
    let mut n: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: reads one int into `n`.
    let ok = unsafe { libc::sysctlbyname(c"hw.physicalcpu".as_ptr(), (&raw mut n).cast(), &mut size, std::ptr::null_mut(), 0) } == 0;
    (ok && n > 0).then_some(n as usize)
}

/// Linux: the distinct (package, core) pairs `/proc/cpuinfo` lists.
#[cfg(not(any(windows, target_os = "macos")))]
fn physical_cores() -> Option<usize> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let mut cores = std::collections::BTreeSet::new();
    let mut package = "";
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        match k.trim() {
            "physical id" => package = v.trim(),
            "core id" => {
                cores.insert((package.to_string(), v.trim().to_string()));
            }
            _ => {}
        }
    }
    (!cores.is_empty()).then_some(cores.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefs::set_check;

    /// The machine's physical cores are found, no more than its logical ones.
    #[test]
    fn physical_cores_are_found() {
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        if let Some(n) = physical_cores() {
            assert!((1..=logical).contains(&n), "{n} of {logical}");
        }
    }

    // spec: audio/multicore.md "Settings"
    /// The two switches reach the engines: on by default, each turned off on its own by a click on its check.
    #[test]
    fn the_switches_reach_the_engines() {
        apply();
        assert_eq!((parallel::mixer_on(), parallel::generators_on()), (true, true));
        let mut app = crate::app::App::new(None);
        app.pref_toggle(MULTITHREADED_MIXER.label);
        assert_eq!((parallel::mixer_on(), parallel::generators_on()), (false, true));
        app.pref_toggle(MULTITHREADED_MIXER.label);
        app.pref_toggle(MULTITHREADED_GENERATORS.label);
        assert_eq!((parallel::mixer_on(), parallel::generators_on()), (true, false));
        set_check(&MULTITHREADED_GENERATORS, true);
        apply();
    }
}
