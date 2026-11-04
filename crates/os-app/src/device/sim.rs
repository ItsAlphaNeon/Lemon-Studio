// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A simulated output device: no sound card, a thread that asks the mix for a block on a clock, at a chosen buffer
//! length and rate, as a device's callback does. It opens in place of the audio settings' device while one is set
//! (`set`, or `OS_SIM_DEVICE=<frames>[@<rate>]` for the whole run), so a playback can be checked for dropouts on any
//! machine: on a hidden display, in CI.
//!
//! The device keeps two blocks: while it plays one, the callback makes the next. A callback that asked late or took
//! long, so that it ended after the block before it had played out, is the device's own underrun (`Errors::xrun`), and
//! the clock starts again from then, as a real device's does.
//!
//! While recording (`record`), every block is kept (`Block`) with what a list of counters (`Probe`: the transport's
//! dropouts by kind, each hosted plugin's processing time) moved since the block before, so a dropout can be laid at
//! a plugin's door afterwards (`take`).
// The recording is read by the playback checks (`crate::checks`), which only tests build.
#![cfg_attr(not(test), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{queues, DeviceInfo, Mix, Open, Stream};

/// `DeviceInfo::host` of the simulated device.
pub const HOST: &str = "Simulated";

/// The simulated device's buffer length (frames per block) and rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub block: u32,
    pub rate: u32,
}

/// `None`: not set yet (`OS_SIM_DEVICE` is read on first use).
static CONFIG: Mutex<Option<Option<Config>>> = Mutex::new(None);

/// `OS_SIM_DEVICE`: "256" or "256@48000".
fn parse(v: &str) -> Option<Config> {
    let (block, rate) = v.split_once('@').unwrap_or((v, "48000"));
    let c = Config { block: block.trim().parse().ok()?, rate: rate.trim().parse().ok()? };
    (c.block > 0 && c.rate > 0).then_some(c)
}

/// The simulated device the next open uses, if any.
pub fn config() -> Option<Config> {
    let mut c = CONFIG.lock().ok()?;
    *c.get_or_insert_with(|| std::env::var("OS_SIM_DEVICE").ok().and_then(|v| parse(&v)))
}

/// Opens the simulated device from now on (`None`: the audio settings' device again). An open device keeps playing
/// until it is opened anew (`Transport::reopen`).
pub fn set(c: Option<Config>) {
    if let Ok(mut s) = CONFIG.lock() {
        *s = Some(c);
    }
}

/// A counter the device reads after every block while recording.
pub struct Probe {
    pub name: String,
    read: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl Probe {
    pub fn new(name: &str, read: impl Fn() -> u64 + Send + Sync + 'static) -> Probe {
        Probe { name: name.to_string(), read: Box::new(read) }
    }
}

/// One block the device asked for.
#[derive(Clone, Copy, Debug)]
pub struct Block {
    /// When it was due, since the recording started.
    pub due: Duration,
    /// How late the callback woke for it, and how long the mix took.
    pub late: Duration,
    pub took: Duration,
    /// The mix's peak (both sides).
    pub peak: f32,
    /// The callback ended after the block before had played out: the device's own underrun.
    pub xrun: bool,
    /// The largest step between two frames following each other on one side, from the block before's last frame on
    /// (a click shows as a step far larger than the music's own).
    pub step: f32,
}

/// What a recording kept: the probes' names, the blocks, and per block what each probe moved since the block before
/// (`moved[block * probes.len() + probe]`).
#[derive(Default)]
pub struct Recording {
    pub probes: Vec<String>,
    pub blocks: Vec<Block>,
    pub moved: Vec<u64>,
    pub config: Option<Config>,
    /// When the first block was due.
    pub started: Option<Instant>,
}

impl Recording {
    pub fn moved(&self, block: usize, probe: usize) -> u64 {
        self.moved[block * self.probes.len() + probe]
    }
}

/// Blocks a recording keeps at most (over 20 minutes at 64 frames and 48 kHz); later ones are not kept.
const MAX_BLOCKS: usize = 1 << 20;

struct Rec {
    on: bool,
    probes: Vec<Probe>,
    last: Vec<u64>,
    out: Recording,
}

static REC: Mutex<Rec> = Mutex::new(Rec { on: false, probes: Vec::new(), last: Vec::new(), out: Recording { probes: Vec::new(), blocks: Vec::new(), moved: Vec::new(), config: None, started: None } });

/// Starts recording every block from the next one on, with `probes` read after each (a recording running is replaced).
pub fn record(probes: Vec<Probe>) {
    let Ok(mut r) = REC.lock() else { return };
    let names = probes.iter().map(|p| p.name.clone()).collect();
    r.last = probes.iter().map(|p| (p.read)()).collect();
    r.probes = probes;
    r.out = Recording { probes: names, config: config(), ..Recording::default() };
    // Room for a few minutes, so the device's thread seldom allocates.
    r.out.blocks.reserve(1 << 16);
    let n = r.probes.len();
    r.out.moved.reserve(n << 16);
    r.on = true;
}

/// Ends the recording and hands it over.
pub fn take() -> Recording {
    let Ok(mut r) = REC.lock() else { return Recording::default() };
    r.on = false;
    r.probes.clear();
    std::mem::take(&mut r.out)
}

/// Keeps block `b` (due at `due`) in the recording, if one is running.
fn keep(due: Instant, mut b: Block) {
    let Ok(mut r) = REC.lock() else { return };
    if !r.on || r.out.blocks.len() >= MAX_BLOCKS {
        return;
    }
    let started = *r.out.started.get_or_insert(due);
    b.due = due.saturating_duration_since(started);
    r.out.blocks.push(b);
    let Rec { probes, last, out, .. } = &mut *r;
    for (p, l) in probes.iter().zip(last.iter_mut()) {
        let v = (p.read)();
        out.moved.push(v.saturating_sub(*l));
        *l = v;
    }
}

/// The device's thread: stopped and joined when the stream is dropped.
pub(super) struct SimOut {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SimOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// When block `k` is due, counted from `t0`.
fn due(t0: Instant, k: u64, c: Config) -> Instant {
    t0 + Duration::from_nanos((u128::from(k) * u128::from(c.block) * 1_000_000_000 / u128::from(c.rate)) as u64)
}

/// Opens the simulated device: its thread starts asking for blocks at once.
pub(super) fn open(c: Config, xruns: &Arc<std::sync::atomic::AtomicU64>) -> Result<Open, String> {
    let (inbox, outbox, errors) = queues(xruns);
    let mut mix = Mix::new(inbox.clone(), outbox.clone());
    let stop = Arc::new(AtomicBool::new(false));
    let period = Duration::from_nanos(u64::from(c.block) * 1_000_000_000 / u64::from(c.rate));
    let thread = std::thread::Builder::new()
        .name("lemonstudio-sim-device".into())
        .spawn({
            let (stop, errors) = (stop.clone(), errors.clone());
            move || {
                // At real-time priority, as a device's callback (`crate::rt`).
                #[cfg(target_os = "linux")]
                if let Err(e) = crate::rt::promote(crate::rt::tid()) {
                    eprintln!("Audio: the simulated device runs at normal priority ({e})");
                }
                let (mut t0, mut k) = (Instant::now(), 0u64);
                let mut last = [0.0f32; 2];
                while !stop.load(Ordering::Acquire) {
                    let at = due(t0, k, c);
                    let now = Instant::now();
                    if at > now {
                        std::thread::sleep(at - now);
                    }
                    let woke = Instant::now();
                    let out = mix.block(c.block as usize);
                    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                    let mut step = 0.0f32;
                    for f in out.chunks_exact(2) {
                        step = step.max((f[0] - last[0]).abs()).max((f[1] - last[1]).abs());
                        last = [f[0], f[1]];
                    }
                    let done = Instant::now();
                    let xrun = done > at + period;
                    keep(at, Block { due: Duration::ZERO, late: woke - at, took: done - woke, peak, xrun, step });
                    if xrun {
                        errors.xrun();
                        (t0, k) = (done, 0);
                    }
                    k += 1;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let info = DeviceInfo {
        host: HOST,
        name: format!("Simulated device ({} frames at {} Hz)", c.block, c.rate),
        rate: c.rate,
        fixed: Some(c.block),
        buffers: Some((c.block, c.block)),
        channels: 2,
        inputs: 0,
        out_names: Vec::new(),
        in_names: Vec::new(),
        input_latency: None,
        rates: Vec::new(),
        sizes: Vec::new(),
        latency: Some(c.block),
        instead: None,
    };
    let sim = SimOut { stop, thread: Some(thread) };
    Ok(Open { _stream: Stream::Sim(sim), inbox, outbox, info, note: None, failed: errors.failed, fault: errors.fault, input: None, input_failed: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_reads_frames_and_rate() {
        assert_eq!(parse("256"), Some(Config { block: 256, rate: 48000 }));
        assert_eq!(parse("128@44100"), Some(Config { block: 128, rate: 44100 }));
        assert_eq!(parse("0"), None);
        assert_eq!(parse("x"), None);
    }

    /// Blocks come at the rate's pace, from a clock that does not drift.
    #[test]
    fn blocks_are_due_on_the_clock() {
        let c = Config { block: 441, rate: 44100 };
        let t0 = Instant::now();
        assert_eq!(due(t0, 100, c) - t0, Duration::from_secs(1));
        let c = Config { block: 256, rate: 48000 };
        assert_eq!(due(t0, 187_500, c) - t0, Duration::from_secs(1000));
    }
}
