// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Multi-core rendering: a pool of worker threads that runs a tick's jobs (mixer tracks, generator channels) in the
//! order a dependency graph allows, the rendering thread taking part and waiting for the last one.
//!
//! - [`Graph`]: jobs `0..n` and their edges, built once (when what it was built from changes) and run every tick.
//!   Every edge goes from a lower job to a higher one, so job order is a valid serial order.
//! - [`Pool`]: the workers. [`Pool::run`] hands them a graph and the job function; a job whose last predecessor
//!   finished is queued, any thread takes it. Nothing allocates, locks or spawns while it runs. Between runs a worker
//!   spins a short while (the next tick usually follows at once while a stream fills its buffer), then parks.
//! - The program's switches (Audio settings ▸ CPU: Multithreaded mixer / generator processing) and its pool:
//!   [`configure`], [`global`]. An engine takes the pool when it is built; the switches count from its next tick.
//! - [`on_audio_thread`]: true on the threads that render (workers always, a thread inside an engine's tick), for plugin
//!   formats that ask (CLAP's `thread-check`).
//!
//! The engine keeps its results identical to rendering on one thread: what tracks add into one bus is summed in a
//! fixed order once its producers are done, never in the order they happen to finish.
// spec: audio/multicore.md

use std::any::Any;
use std::cell::{Cell, UnsafeCell};
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

thread_local! {
    static AUDIO: Cell<bool> = const { Cell::new(false) };
}

/// Whether the calling thread renders audio: a pool's worker, or a thread inside an engine's tick.
pub fn on_audio_thread() -> bool {
    AUDIO.with(Cell::get)
}

/// Marks the calling thread as rendering until the value is dropped (then as it was before).
pub struct AudioScope(bool);

impl AudioScope {
    pub fn enter() -> AudioScope {
        AudioScope(AUDIO.with(|a| a.replace(true)))
    }
}

impl Drop for AudioScope {
    fn drop(&mut self) {
        AUDIO.with(|a| a.set(self.0));
    }
}

static MIXER: AtomicBool = AtomicBool::new(true);
static GENERATORS: AtomicBool = AtomicBool::new(true);
static GLOBAL: Mutex<Option<Arc<Pool>>> = Mutex::new(None);

/// The program's switches: the mixer's tracks and the generator channels on several cores. Engines built with the
/// program's pool read them every tick.
pub fn configure(mixer: bool, generators: bool) {
    MIXER.store(mixer, Ordering::Relaxed);
    GENERATORS.store(generators, Ordering::Relaxed);
}

/// Whether the mixer runs on several cores (the program's switch).
pub fn mixer_on() -> bool {
    MIXER.load(Ordering::Relaxed)
}

/// Whether the generator channels run on several cores (the program's switch).
pub fn generators_on() -> bool {
    GENERATORS.load(Ordering::Relaxed)
}

/// Sets the program's pool, which engines built from now on use (`None`: they render on one thread).
pub fn set_global(pool: Option<Arc<Pool>>) {
    if let Ok(mut g) = GLOBAL.lock() {
        *g = pool;
    }
}

/// The program's pool. Without one set, `OS_MIX_THREADS=n` in the environment makes one of `n` workers (the tools and
/// the tests use it to render on several cores).
pub fn global() -> Option<Arc<Pool>> {
    let mut g = GLOBAL.lock().ok()?;
    if g.is_none() {
        static FROM_ENV: OnceLock<Option<Arc<Pool>>> = OnceLock::new();
        let env = FROM_ENV.get_or_init(|| {
            let n: usize = std::env::var("OS_MIX_THREADS").ok()?.trim().parse().ok()?;
            (n > 0).then(|| Pool::new(n, None))
        });
        *g = env.clone();
    }
    g.clone()
}

/// Workers for a pool on this machine: one fewer than its physical cores (the rendering thread is the last), at least
/// one. `physical` is what the caller found out (`None`: the logical count halved when it is even and above 4, a
/// guess at hyper-threading).
pub fn default_workers(physical: Option<usize>) -> usize {
    let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
    let cores = physical.unwrap_or(if logical > 4 && logical % 2 == 0 { logical / 2 } else { logical });
    cores.clamp(1, logical).saturating_sub(1).max(1)
}

/// Jobs `0..n` and their edges (each from a lower job to a higher one), with the state of a run.
#[derive(Default)]
pub struct Graph {
    n: usize,
    /// Per job: its number of predecessors; where its successors start in `next` (`n + 1` entries).
    deps: Vec<u32>,
    first: Vec<u32>,
    next: Vec<u32>,
    /// Some two jobs can run at once (else running the jobs in order on one thread is as fast).
    wide: bool,
    /// A run: predecessors still to finish per job, the queue of jobs ready (job + 1; 0 = being written), its read and
    /// write ends, and the jobs finished.
    pending: Vec<AtomicU32>,
    ready: Vec<AtomicU32>,
    head: AtomicUsize,
    tail: AtomicUsize,
    done: AtomicUsize,
    /// Scratch for `set`: each job's level, and the jobs per level.
    level: Vec<u32>,
    per: Vec<u32>,
}

impl Graph {
    /// Builds the graph of `n` jobs and `edges` (sorted and deduplicated here). Allocates only to grow.
    pub fn set(&mut self, n: usize, edges: &mut Vec<(u32, u32)>) {
        edges.sort_unstable();
        edges.dedup();
        debug_assert!(edges.iter().all(|&(a, b)| a < b && (b as usize) < n), "edges run forward");
        self.n = n;
        self.deps.clear();
        self.deps.resize(n, 0);
        self.first.clear();
        self.first.resize(n + 1, 0);
        self.next.clear();
        for &(a, b) in edges.iter() {
            self.deps[b as usize] += 1;
            self.first[a as usize + 1] += 1;
            self.next.push(b);
        }
        for j in 0..n {
            self.first[j + 1] += self.first[j];
        }
        // Each job's level: one past its latest predecessor's. Two jobs on one level can run at once.
        self.level.clear();
        self.level.resize(n, 0);
        self.per.clear();
        self.per.resize(n, 0);
        for &(a, b) in edges.iter() {
            self.level[b as usize] = self.level[b as usize].max(self.level[a as usize] + 1);
        }
        for &l in &self.level {
            self.per[l as usize] += 1;
        }
        self.wide = self.per.iter().any(|&c| c > 1);
        while self.pending.len() < n {
            self.pending.push(AtomicU32::new(0));
            self.ready.push(AtomicU32::new(0));
        }
    }

    pub fn len(&self) -> usize {
        self.n
    }

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Some two jobs can run at the same time.
    pub fn wide(&self) -> bool {
        self.wide
    }

    /// Job `j`'s successors.
    pub fn successors(&self, j: usize) -> &[u32] {
        &self.next[self.first[j] as usize..self.first[j + 1] as usize]
    }

    /// Ready for a run: every count back at its start, the jobs without predecessors queued.
    fn reset(&self) {
        self.head.store(0, Ordering::Relaxed);
        self.tail.store(0, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        for j in 0..self.n {
            self.pending[j].store(self.deps[j], Ordering::Relaxed);
            self.ready[j].store(0, Ordering::Relaxed);
        }
        for j in 0..self.n {
            if self.deps[j] == 0 {
                self.push(j);
            }
        }
    }

    fn push(&self, j: usize) {
        let i = self.tail.fetch_add(1, Ordering::AcqRel);
        self.ready[i].store(j as u32 + 1, Ordering::Release);
    }
}

/// What the workers run: a graph, and the job function behind a pointer.
#[derive(Clone, Copy)]
struct Batch {
    graph: *const Graph,
    ctx: *const (),
    call: unsafe fn(*const (), usize, usize),
}

/// One worker's place: asleep (parked) or not, and its thread to wake.
struct Sleeper {
    asleep: AtomicBool,
    thread: OnceLock<Thread>,
}

struct Shared {
    /// Odd while a run is on, even between runs; each run adds 2.
    generation: AtomicU64,
    /// Workers between seeing a run and leaving it.
    inside: AtomicUsize,
    /// A rendering thread is using the pool (another one renders on its own meanwhile).
    taken: AtomicBool,
    stop: AtomicBool,
    batch: UnsafeCell<Batch>,
    sleepers: Box<[Sleeper]>,
    /// The first panic of a run's jobs, raised again on the rendering thread.
    panic: Mutex<Option<Box<dyn Any + Send>>>,
    /// How long a worker spins after a run before it parks.
    spin: Duration,
}

// SAFETY: `batch` is written only by the thread that holds `taken`, between runs (no worker is inside then); workers
// read it during the run it belongs to (see `Pool::run`).
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

/// Worker threads for rendering on several cores. Shared by engines: one runs on it at a time, another meanwhile renders
/// on its own thread (the results are the same either way).
pub struct Pool {
    shared: Arc<Shared>,
    handles: Vec<JoinHandle<()>>,
}

/// A worker's start: the program's own setup for rendering threads (priority, platform registration).
pub type ThreadInit = Arc<dyn Fn() + Send + Sync>;

/// How long workers spin after a run before parking: the next tick of a stream filling its buffer comes within it.
const SPIN: Duration = Duration::from_micros(200);

impl Pool {
    /// A pool of `workers` threads, each started with `init`.
    pub fn new(workers: usize, init: Option<ThreadInit>) -> Arc<Pool> {
        let shared = Arc::new(Shared {
            generation: AtomicU64::new(0),
            inside: AtomicUsize::new(0),
            taken: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            batch: UnsafeCell::new(Batch { graph: std::ptr::null(), ctx: std::ptr::null(), call: nothing }),
            sleepers: (0..workers).map(|_| Sleeper { asleep: AtomicBool::new(false), thread: OnceLock::new() }).collect(),
            panic: Mutex::new(None),
            spin: SPIN,
        });
        let handles = (0..workers)
            .filter_map(|w| {
                let (shared, init) = (shared.clone(), init.clone());
                std::thread::Builder::new()
                    .name(format!("lemonstudio-mix-{}", w + 1))
                    .spawn(move || {
                        AUDIO.with(|a| a.set(true));
                        if let Some(init) = init {
                            init();
                        }
                        let _ = shared.sleepers[w].thread.set(std::thread::current());
                        worker(&shared, w);
                    })
                    .ok()
            })
            .collect();
        Arc::new(Pool { shared, handles })
    }

    /// The pool's worker threads.
    pub fn workers(&self) -> usize {
        self.handles.len()
    }

    /// Runs every job of `graph` as `job(j, lane)` (lane 0: the calling thread, 1.. the workers), each once its
    /// predecessors have finished, the calling thread taking part; returns when all have. `false` (nothing run) when
    /// another thread is using the pool. A panic in a job is raised here once every job has run.
    pub fn run<F: Fn(usize, usize) + Sync>(&self, graph: &Graph, job: &F) -> bool {
        let sh = &*self.shared;
        if graph.n == 0 {
            return true;
        }
        if sh.taken.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return false;
        }
        unsafe fn call<F: Fn(usize, usize)>(ctx: *const (), j: usize, lane: usize) {
            unsafe { (*(ctx as *const F))(j, lane) }
        }
        graph.reset();
        let batch = Batch { graph, ctx: job as *const F as *const (), call: call::<F> };
        // SAFETY: no worker is inside a run (the last run waited for them), so none reads the batch now.
        unsafe { *sh.batch.get() = batch };
        sh.generation.fetch_add(1, Ordering::SeqCst);
        for s in sh.sleepers.iter() {
            if s.asleep.load(Ordering::SeqCst) {
                if let Some(t) = s.thread.get() {
                    t.unpark();
                }
            }
        }
        work(sh, graph, batch, 0);
        sh.generation.fetch_add(1, Ordering::SeqCst);
        while sh.inside.load(Ordering::SeqCst) != 0 {
            std::hint::spin_loop();
        }
        sh.taken.store(false, Ordering::Release);
        if let Some(p) = sh.panic.lock().ok().and_then(|mut p| p.take()) {
            resume_unwind(p);
        }
        true
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for s in self.shared.sleepers.iter() {
            if let Some(t) = s.thread.get() {
                t.unpark();
            }
        }
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

unsafe fn nothing(_: *const (), _: usize, _: usize) {}

/// Takes ready jobs of the run and runs them until every job has finished.
fn work(sh: &Shared, graph: &Graph, batch: Batch, lane: usize) {
    let n = graph.n;
    let mut idle = 0u32;
    while graph.done.load(Ordering::Acquire) < n {
        let h = graph.head.load(Ordering::Relaxed);
        if h >= graph.tail.load(Ordering::Acquire) {
            // Nothing ready: the jobs running now free the next ones.
            idle += 1;
            if idle < 4096 {
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
            continue;
        }
        if graph.head.compare_exchange_weak(h, h + 1, Ordering::AcqRel, Ordering::Relaxed).is_err() {
            continue;
        }
        // The slot was claimed by its writer before it wrote it.
        let j = loop {
            match graph.ready[h].load(Ordering::Acquire) {
                0 => std::hint::spin_loop(),
                v => break v as usize - 1,
            }
        };
        // SAFETY: the job function and its context live until the run is over (`Pool::run` waits for every job).
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| unsafe { (batch.call)(batch.ctx, j, lane) })) {
            if let Ok(mut slot) = sh.panic.lock() {
                slot.get_or_insert(p);
            }
        }
        for &s in graph.successors(j) {
            if graph.pending[s as usize].fetch_sub(1, Ordering::AcqRel) == 1 {
                graph.push(s as usize);
            }
        }
        graph.done.fetch_add(1, Ordering::AcqRel);
        idle = 0;
    }
}

fn worker(sh: &Shared, w: usize) {
    let me = &sh.sleepers[w];
    let mut seen = 0u64;
    loop {
        // Wait for a run other than the last one: spin a while, then park.
        let started = Instant::now();
        let mut spins = 0u32;
        let g = loop {
            if sh.stop.load(Ordering::Acquire) {
                return;
            }
            let g = sh.generation.load(Ordering::SeqCst);
            if g & 1 == 1 && g != seen {
                break g;
            }
            spins += 1;
            if spins % 64 != 0 || started.elapsed() < sh.spin {
                std::hint::spin_loop();
                continue;
            }
            me.asleep.store(true, Ordering::SeqCst);
            let g = sh.generation.load(Ordering::SeqCst);
            if !(g & 1 == 1 && g != seen) && !sh.stop.load(Ordering::SeqCst) {
                std::thread::park();
            }
            me.asleep.store(false, Ordering::SeqCst);
        };
        seen = g;
        sh.inside.fetch_add(1, Ordering::SeqCst);
        if sh.generation.load(Ordering::SeqCst) == g {
            // SAFETY: the run `g` is on and cannot end while this worker is inside it.
            let batch = unsafe { *sh.batch.get() };
            work(sh, unsafe { &*batch.graph }, batch, w + 1);
        }
        sh.inside.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every job runs once, after all its predecessors.
    #[test]
    fn jobs_run_once_after_their_predecessors() {
        let pool = Pool::new(3, None);
        let n = 200;
        let mut edges: Vec<(u32, u32)> = (1..n as u32).flat_map(|b| [(b / 2, b), (b / 3, b)]).filter(|e| e.0 < e.1).collect();
        let mut g = Graph::default();
        g.set(n, &mut edges.clone());
        assert!(g.wide());
        for _ in 0..50 {
            let stamp: Vec<AtomicUsize> = (0..n).map(|_| AtomicUsize::new(0)).collect();
            let clock = AtomicUsize::new(1);
            let lanes = AtomicUsize::new(0);
            assert!(pool.run(&g, &|j, lane| {
                lanes.fetch_or(1 << lane, Ordering::Relaxed);
                assert_eq!(stamp[j].swap(clock.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst), 0, "job {j} ran twice");
            }));
            for &(a, b) in &edges {
                let (a, b) = (stamp[a as usize].load(Ordering::SeqCst), stamp[b as usize].load(Ordering::SeqCst));
                assert!(a != 0 && b != 0 && a < b, "{a} before {b}");
            }
        }
        edges.clear();
        g.set(3, &mut edges);
        assert!(g.wide());
        g.set(3, &mut vec![(0, 1), (1, 2)]);
        assert!(!g.wide(), "a chain");
    }

    /// A pool in use by one thread is refused to another, which renders on its own.
    #[test]
    fn a_busy_pool_is_refused() {
        let pool = Pool::new(1, None);
        let mut g = Graph::default();
        g.set(2, &mut vec![]);
        let inner = AtomicBool::new(true);
        assert!(pool.run(&g, &|_, _| {
            inner.fetch_and(!pool.run(&g, &|_, _| {}), Ordering::SeqCst);
        }));
        assert!(inner.load(Ordering::SeqCst), "refused while running");
    }

    /// A panic in a job comes back on the rendering thread once the run is over; the pool works on.
    #[test]
    fn a_panicking_job_is_raised_after_the_run() {
        let pool = Pool::new(2, None);
        let mut g = Graph::default();
        g.set(8, &mut vec![(0, 7)]);
        let ran = AtomicUsize::new(0);
        let r = catch_unwind(AssertUnwindSafe(|| {
            pool.run(&g, &|j, _| {
                ran.fetch_add(1, Ordering::SeqCst);
                assert!(j != 3, "job 3");
            })
        }));
        assert!(r.is_err());
        assert_eq!(ran.load(Ordering::SeqCst), 8);
        assert!(pool.run(&g, &|_, _| {}));
    }

    /// Workers say they render; other threads only inside a scope.
    #[test]
    fn workers_are_audio_threads() {
        let pool = Pool::new(2, None);
        let mut g = Graph::default();
        g.set(64, &mut vec![]);
        let all = AtomicBool::new(true);
        let _scope = AudioScope::enter();
        pool.run(&g, &|_, _| {
            all.fetch_and(on_audio_thread(), Ordering::SeqCst);
        });
        assert!(all.load(Ordering::SeqCst));
        drop(_scope);
        assert!(!on_audio_thread());
    }
}
