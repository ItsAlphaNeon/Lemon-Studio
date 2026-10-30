// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Real-time priority for the audio threads (Linux): the feeder that renders and the device callback that hands its
//! blocks over (`transport`, `device`). At normal priority they share the CPU with everything else, and a busy machine
//! (a build, a browser) makes them late at small buffers: dropouts. Windows and macOS raise theirs in `transport`.
//!
//! A thread asks the kernel for `SCHED_FIFO` itself when its limits allow (`RLIMIT_RTPRIO`), else rtkit (the desktop's
//! RealtimeKit on the system bus, which PipeWire uses too) for it. Before rtkit agrees the process caps the CPU time a
//! real-time thread may take without blocking (`RLIMIT_RTTIME`): past the soft cap (a project too heavy to render in
//! time spins its feeder) the kernel sends SIGXCPU, and the handler puts every promoted thread back to normal priority
//! instead of letting the hard cap kill the program.

use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::Once;

/// The priority asked for (rtkit allows up to its `MaxRealtimePriority`, 20 by default).
const PRIORITY: i32 = 20;
/// RLIMIT_RTTIME, µs: the soft cap demotes (SIGXCPU), the hard one is rtkit's own maximum (it refuses more).
const RTTIME_SOFT: u64 = 100_000;
const RTTIME_HARD: u64 = 200_000;

/// The promoted threads (kernel thread ids), for the SIGXCPU handler; 0 = free. A slot whose thread ended is taken
/// again (`remember`).
static PROMOTED: [AtomicI32; 64] = [const { AtomicI32::new(0) }; 64];
static COUNT: AtomicUsize = AtomicUsize::new(0);
/// Threads put back to normal priority by the SIGXCPU handler.
static DEMOTED: AtomicUsize = AtomicUsize::new(0);

/// How many threads were promoted, and how many of them demoted since (`demote_all`).
pub fn counts() -> (usize, usize) {
    (COUNT.load(Ordering::Acquire), DEMOTED.load(Ordering::Acquire))
}

#[cfg(test)]
static TEST_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Tests: promotions happen (for the tests of this module and the audio threads').
#[cfg(test)]
pub fn enable_in_tests() {
    TEST_ON.store(true, Ordering::Release);
}

/// The audio threads by what they do, for the audio log (`crate::dropout_log`).
#[derive(Clone, Copy)]
pub enum Role {
    Feeder,
    Callback,
}

static ROLES: [AtomicI32; 2] = [const { AtomicI32::new(0) }; 2];

/// Thread `tid` is now the one doing `role`.
pub fn set_role(role: Role, tid: i32) {
    ROLES[role as usize].store(tid, Ordering::Release);
}

/// Whether the feeder and the device callback run at real-time priority (`None`: none running).
pub fn roles() -> (Option<bool>, Option<bool>) {
    let state = |r: Role| match ROLES[r as usize].load(Ordering::Acquire) {
        0 => None,
        t if ended(t) => None,
        t => Some(is_realtime(t)),
    };
    (state(Role::Feeder), state(Role::Callback))
}

/// The calling thread's kernel id.
pub fn tid() -> i32 {
    // SAFETY: gettid has no preconditions.
    unsafe { libc::gettid() }
}

/// Puts thread `tid` of this process at real-time priority: by itself, else through rtkit. Blocks for rtkit's answer
/// (a few ms): not to be called from the audio callback itself.
pub fn promote(tid: i32) -> Result<(), String> {
    // Tests play on the null device, which never makes a thread wait: dozens of them spinning at real-time priority at
    // once is no test of anything. Only the tests that ask for it promote (`enable_in_tests`).
    #[cfg(test)]
    if !TEST_ON.load(Ordering::Acquire) {
        return Err("not in tests".into());
    }
    static HANDLER: Once = Once::new();
    HANDLER.call_once(install_handler);
    // Again at every promotion: the kernel raises the soft cap by a second at each SIGXCPU, which would leave the hard
    // one to kill the program next time.
    cap_rttime();
    // Known before it is real-time: a SIGXCPU while rtkit answers must find it.
    remember(tid);
    let param = libc::sched_param { sched_priority: PRIORITY };
    // SAFETY: sets the scheduling policy of a thread of this process.
    let direct = unsafe { libc::sched_setscheduler(tid, libc::SCHED_FIFO | libc::SCHED_RESET_ON_FORK, &param) } == 0;
    if !direct {
        rtkit(tid)?;
    }
    COUNT.fetch_add(1, Ordering::AcqRel);
    Ok(())
}

/// `tid` into a free slot of `PROMOTED`, or one whose thread has ended. Not one whose thread is only not real-time: it
/// may be waiting for rtkit, listed before it is promoted, and would be lost.
fn remember(tid: i32) {
    if PROMOTED.iter().any(|s| s.load(Ordering::Acquire) == tid) {
        return;
    }
    for slot in &PROMOTED {
        let t = slot.load(Ordering::Acquire);
        if (t == 0 || ended(t)) && slot.compare_exchange(t, tid, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            return;
        }
    }
}

/// Thread `tid` of this process has ended.
fn ended(tid: i32) -> bool {
    // SAFETY: reads a thread's scheduling policy; fails for a thread that is gone.
    unsafe { libc::sched_getscheduler(tid) < 0 }
}

/// Whether thread `tid` runs at real-time priority now.
pub fn is_realtime(tid: i32) -> bool {
    // SAFETY: reads a thread's scheduling policy.
    let p = unsafe { libc::sched_getscheduler(tid) };
    // rtkit gives SCHED_RR, the kernel's own path SCHED_FIFO.
    p >= 0 && matches!(p & !libc::SCHED_RESET_ON_FORK, libc::SCHED_FIFO | libc::SCHED_RR)
}

fn cap_rttime() {
    let mut cur = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: reads and sets this process's own limit.
    unsafe {
        libc::getrlimit(libc::RLIMIT_RTTIME, &mut cur);
        let hard = cur.rlim_max.min(RTTIME_HARD);
        let lim = libc::rlimit { rlim_cur: RTTIME_SOFT.min(hard), rlim_max: hard };
        libc::setrlimit(libc::RLIMIT_RTTIME, &lim);
    }
}

fn rtkit(tid: i32) -> Result<(), String> {
    let conn = zbus::blocking::Connection::system().map_err(|e| format!("no system bus: {e}"))?;
    let p = zbus::blocking::Proxy::new(&conn, "org.freedesktop.RealtimeKit1", "/org/freedesktop/RealtimeKit1", "org.freedesktop.RealtimeKit1")
        .map_err(|e| e.to_string())?;
    let max: i32 = p.get_property("MaxRealtimePriority").unwrap_or(PRIORITY);
    let prio = PRIORITY.min(max).max(1) as u32;
    p.call::<_, _, ()>("MakeThreadRealtime", &(tid as u64, prio)).map_err(|e| format!("rtkit: {e}"))
}

/// SIGXCPU: a real-time thread passed the soft cap: every promoted thread back to normal priority, and the soft cap
/// back where it was (the kernel raised it by a second), so a thread this missed is warned again soon, not killed.
extern "C" fn demote_all(_: libc::c_int) {
    // SAFETY: a system call, safe in a signal handler.
    unsafe {
        let lim = libc::rlimit { rlim_cur: RTTIME_SOFT, rlim_max: RTTIME_HARD };
        libc::setrlimit(libc::RLIMIT_RTTIME, &lim);
    }
    let normal = libc::sched_param { sched_priority: 0 };
    // The threads stay listed: one rtkit is still promoting is demoted at the next SIGXCPU (the re-armed soft cap
    // sends one soon); `remember` reuses a slot once its thread has ended.
    for slot in &PROMOTED {
        let t = slot.load(Ordering::Acquire);
        if t != 0 && is_realtime(t) {
            // SAFETY: a system call, safe in a signal handler.
            // With the reset-on-fork flag rtkit set: an unprivileged process may not clear it (EPERM).
            unsafe { libc::sched_setscheduler(t, libc::SCHED_OTHER | libc::SCHED_RESET_ON_FORK, &normal) };
            DEMOTED.fetch_add(1, Ordering::AcqRel);
        }
    }
}

fn install_handler() {
    // SAFETY: installs a handler that only makes system calls and touches atomics.
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = demote_all as extern "C" fn(libc::c_int) as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGXCPU, &sa, std::ptr::null_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Several real-time threads spinning at once, promoted at about the same time (the feeder and the device callback
    /// of a stream, a second stream): each is put back to normal priority, none is killed. Three rounds; rtkit refuses
    /// more than about 25 promotions in 20 s ("busy"), and a thread it refused just spins at normal priority.
    #[test]
    #[ignore]
    fn spinning_threads_together_are_demoted_not_killed() {
        enable_in_tests();
        for _ in 0..3 {
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    std::thread::spawn(|| {
                        let _ = promote(tid());
                        let t0 = Instant::now();
                        while t0.elapsed() < Duration::from_millis(600) {
                            std::hint::spin_loop();
                        }
                        is_realtime(tid())
                    })
                })
                .collect();
            for t in threads {
                assert!(!t.join().unwrap(), "demoted");
            }
        }
    }

    /// A real-time thread that spins past the soft cap is put back to normal priority, not killed at the hard one.
    #[test]
    #[ignore]
    fn a_spinning_thread_is_demoted_not_killed() {
        enable_in_tests();
        let h = std::thread::spawn(|| {
            promote(tid()).expect("real-time priority");
            let mut lim = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
            unsafe { libc::getrlimit(libc::RLIMIT_RTTIME, &mut lim) };
            println!("RLIMIT_RTTIME soft {} hard {}", lim.rlim_cur, lim.rlim_max);
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_millis(1000) {
                std::hint::spin_loop();
            }
            is_realtime(tid())
        });
        assert!(!h.join().unwrap(), "demoted");
    }

    /// Deadlines met by a thread working like the audio feeder at 256 frames (48 kHz: a 5.33 ms period, 1.5 ms of work
    /// each) while every core is kept busy at normal priority, at normal priority and at real-time priority. Prints the
    /// misses; asserts the real-time thread misses fewer. Busies the machine for about 20 s:
    /// `cargo test -p os-app --bin lemonstudio rt::tests::deadlines_under_load -- --ignored --exact --nocapture`
    #[test]
    #[ignore]
    fn deadlines_under_load() {
        enable_in_tests();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let env = |k: &str, d: u64| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        // OS_RT_LOAD busy threads per core, OS_RT_WORK µs of work per 5333 µs block.
        let (per_core, work) = (env("OS_RT_LOAD", 2) as usize, env("OS_RT_WORK", 1500));
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        let load: Vec<_> = (0..cores * per_core)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut x = 1u64;
                    while !stop.load(Ordering::Relaxed) {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                        std::hint::black_box(x);
                    }
                })
            })
            .collect();
        let run = |rt: bool| {
            std::thread::spawn(move || {
                let work = Duration::from_micros(work);
                if rt {
                    promote(tid()).expect("real-time priority");
                    assert!(is_realtime(tid()));
                }
                let period = Duration::from_micros(5333);
                let (periods, mut missed, mut worst) = (1500u32, 0u32, Duration::ZERO);
                let start = Instant::now();
                for i in 1..=periods {
                    let t0 = Instant::now();
                    while t0.elapsed() < work {
                        std::hint::spin_loop();
                    }
                    let due = start + period * i;
                    let now = Instant::now();
                    if now > due {
                        missed += 1;
                        worst = worst.max(now - due);
                    } else {
                        std::thread::sleep(due - now);
                    }
                }
                (missed, periods, worst)
            })
            .join()
            .unwrap()
        };
        let normal = run(false);
        let realtime = run(true);
        stop.store(true, Ordering::Relaxed);
        load.into_iter().for_each(|t| t.join().unwrap());
        for (name, (m, n, w)) in [("normal", normal), ("real-time", realtime)] {
            println!("{name:>9}: {m} of {n} blocks late ({:.1} %), worst {:.1} ms late", 100.0 * f64::from(m) / f64::from(n), w.as_secs_f64() * 1000.0);
        }
        assert!(realtime.0 <= normal.0, "real-time misses no more: {realtime:?} vs {normal:?}");
    }
}
