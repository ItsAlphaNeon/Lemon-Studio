// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The mixer on several cores (`crate::parallel`): each tick the tracks run as the jobs of a graph whose edges are
//! what one track needs of another, the result the same as one track after the other in processing order.
//!
//! - What a track adds to other tracks (its sends, a slot's extra output) and to the device's output pairs goes to its
//!   [`Outbox`], not straight into the destination: a track adds its sources' outboxes into its own audio when it
//!   starts, in processing order, so the sum is the one the serial mixer makes whichever source finished first.
//! - Edges: a send to a later track; a sidechain input (a slot listening to another track's output) between listener
//!   and source in processing order, either way: from an earlier track the listener hears this tick's output, from a
//!   later one the last tick's, which the source must not overwrite before the listener has read it.
//! - A send to an earlier track (only inside a routing loop) and the direct outputs reach their destinations after the
//!   run, in processing order, as they would have serially.
//!
//! The generators run the same way before the mixer: each instrument channel is a job (channels sharing one instance
//! run in channel order), and the sampler voices in a few jobs of their own; each puts what it adds to its track in a
//! buffer of its own, added in channel and voice order afterwards.
// spec: audio/multicore.md

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{instrument_job, mix_track, pdc, render_voice, render_voice_into, InstClip, InstCx, InstLane, InstTuning, Lane, MixTrack, Voice};
use crate::parallel::{Graph, Pool};
use crate::TransportInfo;

/// Where something a track puts out goes: another track, or a device output pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Dest {
    Track(usize),
    Pair(usize),
}

/// What one track put out this tick for other places, in the order it made it (`put`), each to be added in.
#[derive(Default)]
pub(super) struct Outbox {
    items: Vec<Dest>,
    bufs: Vec<Vec<f32>>,
    /// A parallel run's tally: hosted plugins of the track that missed the tick, and a slot whose delay changed.
    missed: u64,
    moved: bool,
}

impl Outbox {
    /// Empty for a new tick (the buffers are kept).
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// A new, empty buffer for `dest`, to fill with what is added there.
    pub fn put(&mut self, dest: Dest) -> &mut Vec<f32> {
        let i = self.items.len();
        if self.bufs.len() <= i {
            self.bufs.push(Vec::new());
        }
        self.items.push(dest);
        let b = &mut self.bufs[i];
        b.clear();
        b
    }

    /// Adds what goes to `dest` into `into`, in the order it was put.
    fn deliver(&self, dest: Dest, into: &mut [f32]) {
        for (d, b) in self.items.iter().zip(&self.bufs) {
            if *d == dest {
                for (x, s) in into.iter_mut().zip(b) {
                    *x += *s;
                }
            }
        }
    }

    /// Adds a device output pair's buffer into the device's interleaved channels (`width` of them).
    fn deliver_pair(b: &[f32], p: usize, direct: &mut [f32], width: usize) {
        for (d, s) in direct.chunks_exact_mut(width).zip(b.chunks_exact(2)) {
            d[2 * p] += s[0];
            d[2 * p + 1] += s[1];
        }
    }
}

/// One thread's scratch for running a track: a slot's dry part, a hosted plugin's two sides, a sidechain input and a
/// main input after their pads, what an armed track records from its input.
#[derive(Default)]
pub(super) struct MixScratch {
    pub dry: Vec<f32>,
    pub l: Vec<f32>,
    pub r: Vec<f32>,
    pub key: Vec<f32>,
    pub main: Vec<f32>,
    pub rec: Vec<f32>,
}

/// Every track's output for the slots that listen to it (`Engine::key_out`), shared by the tracks of a tick: the
/// graph's edges keep a track from writing its own while another reads it.
#[derive(Clone, Copy)]
pub(super) struct Keys {
    ptr: *mut Option<Vec<f32>>,
    len: usize,
}

impl Keys {
    pub fn of(v: &mut [Option<Vec<f32>>]) -> Keys {
        Keys { ptr: v.as_mut_ptr(), len: v.len() }
    }

    /// Track `s`'s output, when a slot listens to it.
    ///
    /// # Safety
    /// No one writes track `s`'s output meanwhile.
    pub unsafe fn get<'a>(self, s: usize) -> Option<&'a [f32]> {
        (s < self.len).then(|| unsafe { (*self.ptr.add(s)).as_deref() }).flatten()
    }

    /// Track `t`'s output to write, when a slot listens to it.
    ///
    /// # Safety
    /// Only track `t`'s own turn writes it, and no one reads it meanwhile.
    pub unsafe fn slot<'a>(self, t: usize) -> Option<&'a mut Vec<f32>> {
        (t < self.len).then(|| unsafe { (*self.ptr.add(t)).as_mut() }).flatten()
    }
}

/// What every track's turn of a tick reads.
pub(super) struct MixCx<'a> {
    pub rate: u32,
    pub limit: f32,
    pub song: &'a TransportInfo,
    pub frames: usize,
    /// Whether the master's slots and the insert tracks' run (an export's switches).
    pub master_fx: bool,
    pub insert_fx: bool,
    /// On a device: its inputs this tick (interleaved), how many, and its output pairs.
    pub io: Option<(&'a [f32], usize, usize)>,
    pub keys: Keys,
    pub export: bool,
    pub wait: Duration,
    /// While the song plays: where armed tracks record, and the song tick being mixed (spec: audio/track-io.md §2.5).
    pub record: Option<(&'a dyn super::RecordTap, u64)>,
}

// SAFETY: `keys` is shared under the graph's rule (see `Keys`); the rest is read only (a record tap takes each track's
// pushes from that track's turn alone: `RecordTap`).
unsafe impl Sync for MixCx<'_> {}

/// The engine's parts the mixer changes.
pub(super) struct MixParts<'a> {
    pub order: &'a [usize],
    pub tracks: &'a mut [MixTrack],
    pub bus: &'a mut [Vec<f32>],
    pub capture: Option<&'a mut [Vec<f32>]>,
    pub cap_skip: &'a mut [u64],
    /// The device's output pairs, interleaved.
    pub direct: Option<&'a mut Vec<f32>>,
    pub until: &'a mut Option<Instant>,
    pub missed: &'a mut u64,
}

/// An engine's share of multi-core rendering: the pool, and the mixer's graph and buffers.
pub(super) struct Multicore {
    pool: Option<Arc<Pool>>,
    /// The pool came with the engine (`parallel::global`): it is used while the program's switch is on. A pool given
    /// with `Engine::set_threads` always is.
    follow: bool,
    /// Scratch per lane (the rendering thread, then each worker).
    lanes: Vec<MixScratch>,
    outbox: Vec<Outbox>,
    graph: Graph,
    /// What the graph was built from (processing order, sends, sidechain inputs), each tick's compared with it.
    sig: Vec<u32>,
    sig_now: Vec<u32>,
    /// Each track's place in processing order; per job, where its feeds start in `feeds`, the sources' places.
    pos: Vec<u32>,
    feed_at: Vec<u32>,
    feeds: Vec<u32>,
    pairs: Vec<(u32, u32)>,
    edges: Vec<(u32, u32)>,
    /// Ticks the mixer ran on the pool.
    pub ran: u64,
    /// The generators: per channel what its instrument put out, the channels whose instrument runs this tick, each
    /// voice's buffer and whether it plays on, their graph and its edges (channels sharing an instance).
    inst_out: Vec<InstOut>,
    gen_jobs: Vec<usize>,
    voice_buf: Vec<Vec<f32>>,
    keep: Vec<bool>,
    gen_graph: Graph,
    gen_edges: Vec<(u32, u32)>,
    shared_inst: Vec<(usize, u32)>,
    /// Ticks the generators ran on the pool.
    pub gen_ran: u64,
}

impl Multicore {
    pub fn new(pool: Option<Arc<Pool>>) -> Multicore {
        let mut m = Multicore {
            pool: None,
            follow: true,
            lanes: Vec::new(),
            outbox: Vec::new(),
            graph: Graph::default(),
            sig: Vec::new(),
            sig_now: Vec::new(),
            pos: Vec::new(),
            feed_at: Vec::new(),
            feeds: Vec::new(),
            pairs: Vec::new(),
            edges: Vec::new(),
            ran: 0,
            inst_out: Vec::new(),
            gen_jobs: Vec::new(),
            voice_buf: Vec::new(),
            keep: Vec::new(),
            gen_graph: Graph::default(),
            gen_edges: Vec::new(),
            shared_inst: Vec::new(),
            gen_ran: 0,
        };
        m.set_pool(pool, true);
        m
    }

    pub fn set_pool(&mut self, pool: Option<Arc<Pool>>, follow: bool) {
        let lanes = 1 + pool.as_ref().map_or(0, |p| p.workers());
        self.lanes.resize_with(lanes.max(self.lanes.len()), MixScratch::default);
        self.pool = pool;
        self.follow = follow;
    }

    /// The pool, while the mixer is to use it.
    fn mixer_pool(&self) -> Option<Arc<Pool>> {
        self.pool.clone().filter(|p| p.workers() > 0 && (!self.follow || crate::parallel::mixer_on()))
    }

    /// The pool, while the generators are to use it.
    fn generator_pool(&self) -> Option<Arc<Pool>> {
        self.pool.clone().filter(|p| p.workers() > 0 && (!self.follow || crate::parallel::generators_on()))
    }

    /// The channels whose instrument ran this tick (`generators`) and what each put out, in channel order.
    pub fn instruments(&self) -> impl Iterator<Item = (usize, &InstOut)> + '_ {
        self.gen_jobs.iter().map(|&c| (c, &self.inst_out[c]))
    }

    /// Whether channel `c`'s instrument ran this tick (it was not busy elsewhere).
    pub fn instrument_ran(&self, c: usize) -> bool {
        self.inst_out.get(c).is_some_and(|o| o.ran)
    }

    /// The generators of a tick: every channel's instrument (`instrument_job`) and, with `parts.voices`, the sampler
    /// voices, into their tracks' inputs. On the pool when the generator switch is on and there is more than one job.
    pub fn generators(&mut self, cx: &InstCx, p: GenParts) {
        let nc = cx.channels.len();
        if self.inst_out.len() < nc {
            self.inst_out.resize_with(nc, InstOut::default);
        }
        for o in &mut self.inst_out {
            o.ran = false;
        }
        // A poisoned instance (it panicked while held) stays silent.
        self.gen_jobs.clear();
        self.gen_jobs.extend((0..nc).filter(|&c| cx.channels[c].instrument.0.as_ref().is_some_and(|i| !i.is_poisoned())));
        let voices = p.voices.as_ref().map_or(0, |v| v.0.len());
        let pool = self.generator_pool().filter(|_| self.gen_jobs.len() + voices.min(2) > 1);
        if let Some(pool) = pool {
            // The voices in a few jobs, after the instruments (which take longer).
            let lanes = pool.workers() + 1;
            let chunk = voices.div_ceil(2 * lanes).max(1);
            let k = self.gen_jobs.len();
            let n = k + voices.div_ceil(chunk);
            // Channels sharing one instance run in channel order.
            self.shared_inst.clear();
            for (j, &c) in self.gen_jobs.iter().enumerate() {
                if let Some(i) = cx.channels[c].instrument.0.as_ref() {
                    self.shared_inst.push((Arc::as_ptr(i) as *const () as usize, j as u32));
                }
            }
            self.shared_inst.sort_unstable();
            self.gen_edges.clear();
            self.gen_edges.extend(self.shared_inst.windows(2).filter(|w| w[0].0 == w[1].0).map(|w| (w[0].1, w[1].1)));
            self.gen_graph.set(n, &mut self.gen_edges);
            if let Some((v, _)) = &p.voices {
                while self.voice_buf.len() < v.len() {
                    self.voice_buf.push(Vec::new());
                }
                self.keep.clear();
                self.keep.resize(v.len(), true);
            }
            match self.generators_on(&pool, cx, p, chunk) {
                Ok(()) => {
                    self.gen_ran += 1;
                    return;
                }
                Err(p) => return self.generators_serial(cx, p),
            }
        }
        self.generators_serial(cx, p)
    }

    fn generators_serial(&mut self, cx: &InstCx, p: GenParts) {
        let GenParts { tuning, owed, pads, until, voices, bus } = p;
        if let Some((voices, w)) = voices {
            let (channels, len) = (cx.channels, cx.n);
            voices.retain_mut(|v| render_voice(v, channels, bus, 0, 0, len, w));
        }
        let scratch = &mut self.lanes[0];
        for &c in &self.gen_jobs {
            let lane = InstLane { c, fines: &mut tuning[c], owed: &mut owed[c], pad: pads.get_mut(c), until: &mut *until, out: &mut self.inst_out[c] };
            instrument_job(cx, lane, scratch);
        }
    }

    /// The generators on the pool. `Err` gives the parts back untouched when the pool is busy.
    fn generators_on<'a>(&mut self, pool: &Pool, cx: &InstCx, mut p: GenParts<'a>, chunk: usize) -> Result<(), GenParts<'a>> {
        let deadline = p.until.unwrap_or_else(|| Instant::now() + cx.wait);
        let (voices, w) = match p.voices.as_mut() {
            Some((v, w)) => ((v.as_mut_ptr(), v.len()), *w),
            None => ((std::ptr::null_mut(), 0), 0.0),
        };
        let jobs = GenJobs {
            cx,
            channels: &self.gen_jobs,
            tuning: p.tuning.as_mut_ptr(),
            owed: p.owed.as_mut_ptr(),
            pads: (p.pads.as_mut_ptr(), p.pads.len()),
            out: self.inst_out.as_mut_ptr(),
            lanes: self.lanes.as_mut_ptr(),
            deadline,
            voices,
            w,
            chunk,
            voice_buf: self.voice_buf.as_mut_ptr(),
            keep: self.keep.as_mut_ptr(),
        };
        // SAFETY: each job touches its own channel's state and output, or its own voices and their buffers; channels
        // sharing an instance run one after the other (the graph's edges).
        if !pool.run(&self.gen_graph, &|j, lane| unsafe { jobs.run(j, lane) }) {
            return Err(p);
        }
        // The voices into their tracks, in voice order, then the ones that ended go.
        if let Some((v, _)) = p.voices {
            for (i, voice) in v.iter().enumerate() {
                let into = &mut p.bus[cx.channels[voice.channel].insert];
                for (x, s) in into.iter_mut().zip(&self.voice_buf[i]) {
                    *x += *s;
                }
            }
            let mut i = 0;
            v.retain(|_| {
                i += 1;
                self.keep[i - 1]
            });
        }
        Ok(())
    }

    /// The tick's mixer: every track in processing order, on one thread or on the pool; whether a slot's delay changed.
    pub fn mix(&mut self, parts: MixParts, cx: &MixCx) -> bool {
        if self.outbox.len() < parts.tracks.len() {
            self.outbox.resize_with(parts.tracks.len(), Outbox::default);
        }
        if let Some(pool) = self.mixer_pool() {
            self.plan(parts.order, parts.tracks);
            if self.graph.wide() {
                match self.mix_on(&pool, parts, cx) {
                    Ok(moved) => return moved,
                    // The pool is busy with another engine: this tick renders here.
                    Err(parts) => return self.mix_serial(parts, cx),
                }
            }
        }
        self.mix_serial(parts, cx)
    }

    /// One track after the other; what each puts out reaches its destinations right after it.
    fn mix_serial(&mut self, p: MixParts, cx: &MixCx) -> bool {
        let mut moved = false;
        let MixParts { order, tracks, bus, mut capture, cap_skip, mut direct, until, missed } = p;
        let scratch = &mut self.lanes[0];
        for &t in order {
            let mut buf = std::mem::take(&mut bus[t]);
            let lane = Lane {
                t,
                track: &mut tracks[t],
                buf: &mut buf,
                capture: capture.as_deref_mut().and_then(|c| c.get_mut(t)),
                skip: cap_skip.get_mut(t),
                out: &mut self.outbox[t],
                until: &mut *until,
                missed: &mut *missed,
                moved: &mut moved,
            };
            mix_track(lane, cx, scratch);
            bus[t] = buf;
            let out = &self.outbox[t];
            for (d, b) in out.items.iter().zip(&out.bufs) {
                match *d {
                    // A send to itself adds nothing: the track's audio is out of the bus while it runs.
                    Dest::Track(d) if d != t => {
                        for (x, s) in bus[d].iter_mut().zip(b) {
                            *x += *s;
                        }
                    }
                    Dest::Track(_) => {}
                    Dest::Pair(pr) => {
                        if let (Some(dv), Some((_, _, pairs))) = (direct.as_deref_mut(), cx.io) {
                            Outbox::deliver_pair(b, pr, dv, 2 * pairs);
                        }
                    }
                }
            }
        }
        moved
    }

    /// Works the graph out again when the routing or a sidechain input changed.
    fn plan(&mut self, order: &[usize], tracks: &[MixTrack]) {
        let n = tracks.len();
        let none = u32::MAX;
        let sig = &mut self.sig_now;
        sig.clear();
        sig.push(n as u32);
        sig.extend(order.iter().map(|&t| t as u32));
        for m in tracks {
            sig.push(m.routes.len() as u32);
            sig.extend(m.routes.iter().map(|r| r.dest as u32));
            sig.push(m.effects.len() as u32);
            sig.extend(m.effects.iter().flat_map(|f| [f.key.map_or(none, |k| k as u32), f.main.map_or(none, |k| k as u32)]));
        }
        if *sig == self.sig {
            return;
        }
        std::mem::swap(&mut self.sig, &mut self.sig_now);
        self.pos.clear();
        self.pos.resize(n, 0);
        for (j, &t) in order.iter().enumerate() {
            self.pos[t] = j as u32;
        }
        let pos = &self.pos;
        // Feeds: the sends into each track from tracks before it. Edges: those, and sidechain inputs either way.
        self.pairs.clear();
        self.edges.clear();
        for (s, m) in tracks.iter().enumerate() {
            for r in &m.routes {
                if r.dest != s && r.dest < n && pos[r.dest] > pos[s] {
                    self.pairs.push((pos[r.dest], pos[s]));
                    self.edges.push((pos[s], pos[r.dest]));
                }
            }
            for f in &m.effects {
                for k in [f.key, f.main].into_iter().flatten().filter(|&k| k != s && k < n) {
                    self.edges.push((pos[k].min(pos[s]), pos[k].max(pos[s])));
                }
            }
        }
        self.pairs.sort_unstable();
        self.pairs.dedup();
        self.feed_at.clear();
        self.feed_at.resize(order.len() + 1, 0);
        self.feeds.clear();
        for &(d, s) in &self.pairs {
            self.feed_at[d as usize + 1] += 1;
            self.feeds.push(s);
        }
        for j in 0..order.len() {
            self.feed_at[j + 1] += self.feed_at[j];
        }
        self.graph.set(order.len(), &mut self.edges);
    }

    /// The tracks on the pool. `Err` gives the parts back untouched when the pool is busy.
    fn mix_on<'a>(&mut self, pool: &Pool, mut p: MixParts<'a>, cx: &MixCx) -> Result<bool, MixParts<'a>> {
        let n = p.order.len();
        // One wait for hosted plugins busy elsewhere, for the whole tick, from its first wait or from now.
        let deadline = p.until.unwrap_or_else(|| Instant::now() + cx.wait);
        let jobs = MixJobs {
            cx,
            order: p.order,
            feed_at: &self.feed_at,
            feeds: &self.feeds,
            tracks: p.tracks.as_mut_ptr(),
            bus: p.bus.as_mut_ptr(),
            capture: p.capture.as_deref_mut().map_or((std::ptr::null_mut(), 0), |c| (c.as_mut_ptr(), c.len())),
            skip: (p.cap_skip.as_mut_ptr(), p.cap_skip.len()),
            outbox: self.outbox.as_mut_ptr(),
            lanes: self.lanes.as_mut_ptr(),
            deadline,
        };
        // SAFETY: each job touches its own track's state and outbox, the outboxes of tracks that finished before it
        // (its feeds) and, through `Keys`, outputs the graph orders against it (see the module's notes).
        if !pool.run(&self.graph, &|j, lane| unsafe { jobs.run(j, lane) }) {
            return Err(p);
        }
        self.ran += 1;
        // Sends to earlier tracks, then the direct outputs, in processing order.
        let MixParts { order, bus, mut direct, missed, .. } = p;
        let mut moved = false;
        for (j, &t) in order.iter().enumerate().take(n) {
            let out = &self.outbox[t];
            *missed += out.missed;
            moved |= out.moved;
            for (d, b) in out.items.iter().zip(&out.bufs) {
                match *d {
                    Dest::Track(d) if d != t && self.pos[d] < j as u32 => {
                        for (x, s) in bus[d].iter_mut().zip(b) {
                            *x += *s;
                        }
                    }
                    Dest::Track(_) => {}
                    Dest::Pair(pr) => {
                        if let (Some(dv), Some((_, _, pairs))) = (direct.as_deref_mut(), cx.io) {
                            Outbox::deliver_pair(b, pr, dv, 2 * pairs);
                        }
                    }
                }
            }
        }
        Ok(moved)
    }
}

/// What a channel's instrument put out in a tick (`instrument_job`).
#[derive(Default)]
pub(super) struct InstOut {
    /// What the channel adds to its insert, interleaved.
    pub buf: Vec<f32>,
    /// The instrument ran (else it was busy elsewhere and missed the tick).
    pub ran: bool,
    /// The delay it reported, and whether it said it still sounds.
    pub delay: u32,
    pub active: bool,
}

/// The audio clips with shapes on instrument channels (`Engine::inst_shapes`), shared by the channels of a tick: each
/// channel's turn touches only its own clips.
#[derive(Clone, Copy)]
pub(super) struct Shapes {
    ptr: *mut InstClip,
    len: usize,
}

impl Shapes {
    pub fn of(v: &mut [InstClip]) -> Shapes {
        Shapes { ptr: v.as_mut_ptr(), len: v.len() }
    }

    /// Channel `c`'s clips, in order.
    ///
    /// # Safety
    /// Only channel `c`'s turn touches them, and one iterator at a time.
    pub unsafe fn of_channel<'a>(self, c: usize) -> impl Iterator<Item = &'a mut InstClip> {
        // The channel is read through the pointer: other channels' clips may be in use meanwhile.
        (0..self.len)
            .map(move |i| unsafe { self.ptr.add(i) })
            .filter(move |&p| unsafe { std::ptr::addr_of!((*p).channel).read() } == c)
            .map(|p| unsafe { &mut *p })
    }
}

/// The engine's parts the generators change.
pub(super) struct GenParts<'a> {
    pub tuning: &'a mut [InstTuning],
    pub owed: &'a mut [usize],
    pub pads: &'a mut [pdc::Pad],
    pub until: &'a mut Option<Instant>,
    /// The sampler voices to render (with the interpolation's half width), and the tracks' inputs.
    pub voices: Option<(&'a mut Vec<Voice>, f64)>,
    pub bus: &'a mut [Vec<f32>],
}

/// The generators' jobs of one tick: the instrument channels, then the voices in chunks.
struct GenJobs<'a> {
    cx: &'a InstCx<'a>,
    channels: &'a [usize],
    tuning: *mut InstTuning,
    owed: *mut usize,
    pads: (*mut pdc::Pad, usize),
    out: *mut InstOut,
    lanes: *mut MixScratch,
    deadline: Instant,
    voices: (*mut Voice, usize),
    w: f64,
    chunk: usize,
    voice_buf: *mut Vec<f32>,
    keep: *mut bool,
}

// SAFETY: see `Multicore::generators_on`.
unsafe impl Sync for GenJobs<'_> {}

impl GenJobs<'_> {
    /// Job `j`: an instrument channel, or a chunk of the voices; on lane `lane`.
    unsafe fn run(&self, j: usize, lane: usize) {
        let cx = self.cx;
        unsafe {
            if let Some(&c) = self.channels.get(j) {
                let mut until = Some(self.deadline);
                let l = InstLane {
                    c,
                    fines: &mut *self.tuning.add(c),
                    owed: &mut *self.owed.add(c),
                    pad: (c < self.pads.1).then(|| &mut *self.pads.0.add(c)),
                    until: &mut until,
                    out: &mut *self.out.add(c),
                };
                return instrument_job(cx, l, &mut *self.lanes.add(lane));
            }
            let from = (j - self.channels.len()) * self.chunk;
            for i in from..(from + self.chunk).min(self.voices.1) {
                let buf = &mut *self.voice_buf.add(i);
                buf.clear();
                buf.resize(cx.n * 2, 0.0);
                *self.keep.add(i) = render_voice_into(&mut *self.voices.0.add(i), cx.channels, buf, 0, 0, cx.n, self.w);
            }
        }
    }
}

/// The mixer's jobs of one tick, as raw parts the jobs share (each touching what is its own).
struct MixJobs<'a> {
    cx: &'a MixCx<'a>,
    order: &'a [usize],
    feed_at: &'a [u32],
    feeds: &'a [u32],
    tracks: *mut MixTrack,
    bus: *mut Vec<f32>,
    capture: (*mut Vec<f32>, usize),
    skip: (*mut u64, usize),
    outbox: *mut Outbox,
    lanes: *mut MixScratch,
    deadline: Instant,
}

// SAFETY: see `Multicore::mix_on`.
unsafe impl Sync for MixJobs<'_> {}

impl MixJobs<'_> {
    /// Job `j`: the track `j`-th in processing order, on lane `lane`.
    unsafe fn run(&self, j: usize, lane: usize) {
        let t = self.order[j];
        unsafe {
            let buf = &mut *self.bus.add(t);
            // What the tracks before it send it, in processing order: they have all finished.
            for &s in &self.feeds[self.feed_at[j] as usize..self.feed_at[j + 1] as usize] {
                (*self.outbox.add(self.order[s as usize])).deliver(Dest::Track(t), buf);
            }
            let (mut until, mut missed, mut moved) = (Some(self.deadline), 0, false);
            let lane_scratch = &mut *self.lanes.add(lane);
            let l = Lane {
                t,
                track: &mut *self.tracks.add(t),
                buf,
                capture: (t < self.capture.1).then(|| &mut *self.capture.0.add(t)),
                skip: (t < self.skip.1).then(|| &mut *self.skip.0.add(t)),
                out: &mut *self.outbox.add(t),
                until: &mut until,
                missed: &mut missed,
                moved: &mut moved,
            };
            mix_track(l, self.cx, lane_scratch);
            let out = &mut *self.outbox.add(t);
            (out.missed, out.moved) = (missed, moved);
        }
    }
}

const _: () = {
    const fn send<T: Send>() {}
    send::<MixTrack>();
    send::<Outbox>();
    send::<Voice>();
    send::<InstOut>();
};

#[cfg(test)]
pub(super) mod tests {
    use super::super::*;
    use crate::effects;
    use crate::parallel::Pool;
    use os_model::{Channel, MixerTrack, Note, Pattern, SlotPlugin};

    /// A small pseudo-random generator, so the projects are the same every run.
    pub struct Lcg(pub u64);

    impl Lcg {
        pub fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }

        pub fn below(&mut self, n: u32) -> u32 {
            self.next() % n.max(1)
        }
    }

    pub fn channel(index: u16, track: u16) -> Channel {
        Channel {
            id: os_model::Id::NONE,
            index,
            kind: ChannelKind::Sampler,
            name: None,
            plugin: None,
            plugin_state: None,
            color: None,
            enabled: true,
            volume: 0.6,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: track,
            sample_path: None, // the built-in tone
            layer_children: vec![],
            envelopes: Default::default(),
            rack: Default::default(),
            misc: Default::default(),
            sound: Default::default(),
        }
    }

    fn slot(name: &str, state: Vec<u8>) -> SlotPlugin {
        SlotPlugin { id: os_model::Id::NONE, name: name.into(), state }
    }

    /// A Fruity Limiter (its look-ahead delays the track) keyed from the track's input `key` (0: its own audio).
    fn limiter(key: i32) -> SlotPlugin {
        let p = effects::dynamics::DEFAULTS;
        let mut v = vec![7];
        v.extend_from_slice(&p);
        v.extend_from_slice(&p);
        v.extend_from_slice(&[0, 0, 0, 0, key]);
        let mut state = effects::state_of(&v);
        state.push(0);
        slot("Fruity Limiter", state)
    }

    /// One of the own effects, by number.
    fn effect(k: u32) -> SlotPlugin {
        use effects::*;
        match k % 9 {
            0 => slot("Fruity Reeverb 2", reverb::Settings { knobs: reverb::DEFAULTS, ..Default::default() }.to_state()),
            1 => slot("Fruity Delay 3", delay::Settings { v: delay::DEFAULTS }.to_state()),
            2 => slot("Fruity Chorus", chorus::Settings { v: chorus::DEFAULTS }.to_state()),
            3 => slot("Fruity Filter", filter::Settings { v: filter::DEFAULTS, ..Default::default() }.to_state()),
            4 => slot("Fruity Phaser", phaser::Settings { v: phaser::DEFAULTS }.to_state()),
            5 => slot("Fruity Flanger", flanger::Settings { v: flanger::DEFAULTS }.to_state()),
            6 => slot("Fruity Fast Dist", fast_dist::Settings { v: fast_dist::DEFAULTS }.to_state()),
            7 => slot("Fruity Soft Clipper", soft_clipper::Settings { v: soft_clipper::DEFAULTS }.to_state()),
            // A Stereo Shaper whose difference output goes to the track's second output.
            _ => slot("Fruity Stereo Shaper", stereo_shaper::Settings { v: [0, 0, 0, 0, 0, 0, 2, 1] }.to_state()),
        }
    }

    /// A hosted effect with state: a one-pole low-pass and a gain, its key added in when it has one.
    pub struct Smooth(pub f32, pub [f32; 2]);

    impl crate::instrument::HostedEffect for Smooth {
        fn process_fx(&mut self, _rate: u32, left: &mut [f32], right: &mut [f32]) {
            for (side, buf) in [&mut *left, &mut *right].into_iter().enumerate() {
                for x in buf.iter_mut() {
                    self.1[side] += self.0 * (*x - self.1[side]);
                    *x = self.1[side] * 0.9;
                }
            }
        }

        fn process_fx_keyed(&mut self, rate: u32, left: &mut [f32], right: &mut [f32], key: (&[f32], &[f32])) {
            self.process_fx(rate, left, right);
            for (x, k) in left.iter_mut().zip(key.0).chain(right.iter_mut().zip(key.1)) {
                *x += 0.25 * k;
            }
        }

        fn reported_delay_fx(&self) -> u32 {
            (self.0 * 100.0) as u32
        }
    }

    /// The device's inputs: a slow saw on each.
    struct Saw(u32, usize);

    impl DeviceInput for Saw {
        fn channels(&self) -> usize {
            self.1
        }

        fn read(&mut self, out: &mut [f32]) {
            for f in out.chunks_exact_mut(self.1) {
                for (c, x) in f.iter_mut().enumerate() {
                    *x = ((self.0 + 37 * c as u32) % 400) as f32 / 400.0 - 0.5;
                }
                self.0 += 1;
            }
        }
    }

    /// A dense project: `channels` sampler channels on `inserts` insert tracks with own effects (some keyed, some with
    /// difference outputs, look-aheads for delay compensation), sends at many levels into group buses and a shared
    /// return, a routing loop, monitored device inputs and direct outputs on some tracks.
    pub fn dense(seed: u64, inserts: u16, channels: u16) -> Project {
        let mut rng = Lcg(seed);
        let groups = 4u16;
        let n = inserts + groups + 2; // + the return and the sum of the groups
        let (ret, sum) = (inserts + groups + 1, inserts + groups + 2);
        let mut mixer: Vec<MixerTrack> = (0..=n).map(MixerTrack::new).collect();
        for t in 1..=inserts {
            let m = &mut mixer[usize::from(t)];
            let g = inserts + 1 + rng.below(u32::from(groups)) as u16;
            m.routes = vec![g];
            m.send_levels = vec![(g, 0.5 + f64::from(rng.below(50)) / 100.0)];
            // Some go to the master too, some to the return.
            if rng.below(3) == 0 {
                m.routes.push(0);
            }
            if rng.below(2) == 0 {
                m.routes.push(ret);
                m.send_levels.push((ret, f64::from(rng.below(100)) / 100.0));
            }
            for k in 0..1 + rng.below(3) as usize {
                m.slots[k] = Some(effect(rng.next()));
                m.slot_mix[k] = [1.0, 0.5, 0.0, 0.8][rng.below(4) as usize];
            }
            if rng.below(4) == 0 {
                m.slots[4] = Some(limiter(0));
            }
            m.volume = Some(0.5 + f64::from(rng.below(40)) / 100.0);
            m.pan = Some(f64::from(rng.below(200)) / 100.0 - 1.0);
            if rng.below(5) == 0 {
                m.latency_out = rng.below(20) as f32;
            }
            if rng.below(6) == 0 {
                (m.audio_in, m.monitor) = (rng.below(3) as i32, os_model::MONITOR_ON);
            }
            if rng.below(6) == 0 {
                m.audio_out = 1;
            }
        }
        // Tracks keyed from another insert routed into them at level 0 (a sidechain only).
        for t in [3u16, 7, 11] {
            let src = (t + 5).min(inserts);
            let m = &mut mixer[usize::from(src)];
            m.routes.push(t);
            m.send_levels.push((t, 0.0));
            mixer[usize::from(t)].slots[6] = Some(limiter(1));
        }
        for g in inserts + 1..=inserts + groups {
            let m = &mut mixer[usize::from(g)];
            m.routes = vec![sum];
            m.slots[0] = Some(effect(rng.next()));
            // Each group's limiter is keyed from its last input.
            m.slots[1] = Some(limiter(99));
        }
        mixer[usize::from(ret)].slots[0] = Some(effect(0));
        mixer[usize::from(sum)].slots[0] = Some(limiter(2));
        // A loop: the sum sends back into insert 2, at level 0 (a sidechain from later in the order), and with sound
        // into insert 4 (arriving after insert 4 has run).
        mixer[usize::from(sum)].routes.extend([2, 4]);
        mixer[usize::from(sum)].send_levels = vec![(2, 0.0), (4, 0.3)];
        mixer[2].slots[7] = Some(limiter(9));
        mixer[0].slots[0] = Some(effect(7));
        let chans: Vec<Channel> = (0..channels).map(|c| channel(c, 1 + c % inserts)).collect();
        let mut notes = Vec::new();
        for c in 0..channels {
            for _ in 0..3 {
                let pos = rng.below(2 * 96);
                notes.push(Note { length: 24 + rng.below(96), key: 48 + rng.below(24) as u16, velocity: 60 + rng.below(60) as u8, ..Note::step(c, pos) });
            }
        }
        Project {
            ppq: 96,
            tempo: Some(130.0),
            channels: chans,
            mixer,
            patterns: vec![Pattern { number: 1, notes, ..Pattern::default() }],
            ..Project::default()
        }
    }

    /// The first `TICKS` ticks an engine for `p` renders (and its tracks' captured audio), on `pool` or on one thread;
    /// `hosted` puts hosted effects on some tracks, `device` plays on two output pairs with three inputs. On a pool,
    /// every tick runs on it.
    pub fn render(p: &Project, pool: Option<Arc<Pool>>, hosted: bool, device: bool) -> (Vec<f32>, Vec<Vec<f32>>) {
        const TICKS: usize = 2 * 96 + 40;
        let on = pool.is_some();
        let mut lib = SampleLibrary::new(None);
        let mut e = Engine::new(p, &mut lib, 44100, PlayMode::Pattern(1));
        e.set_threads(pool);
        e.set_track_capture(true);
        if device {
            e.set_device(2, Some(Box::new(Saw(0, 3))));
        }
        if hosted {
            for t in (1..p.mixer.len()).step_by(5) {
                e.set_effect(t, 8, Arc::new(Mutex::new(Smooth(0.05 + t as f32 / 1000.0, [0.0; 2]))));
                // Every other one hears its track's first input on its sidechain bus.
                if t % 2 == 1 {
                    e.set_effect_ports(t, 8, 0, 1);
                }
            }
        }
        let mut out = Vec::new();
        e.render_ticks(TICKS, &mut out);
        assert_eq!(e.multicore.ran, if on { TICKS as u64 } else { 0 }, "mixer ticks on the pool");
        // The generators of a tick with nothing playing stay on the rendering thread.
        assert!(if on { e.multicore.gen_ran > TICKS as u64 / 2 } else { e.multicore.gen_ran == 0 }, "generator ticks on the pool");
        (out, e.take_track_audio())
    }

    /// The first sample where `a` and `b` differ in any bit, if any.
    pub fn first_difference(a: &[f32], b: &[f32]) -> Option<usize> {
        if a.len() != b.len() {
            return Some(a.len().min(b.len()));
        }
        a.iter().zip(b).position(|(x, y)| x.to_bits() != y.to_bits())
    }

    fn check(p: &Project, pool: &Arc<Pool>, hosted: bool, device: bool, what: &str) {
        let (one, one_tracks) = render(p, None, hosted, device);
        assert!(one.iter().any(|s| s.abs() > 1e-3), "{what}: the project sounds");
        for run in 0..2 {
            let (many, many_tracks) = render(p, Some(pool.clone()), hosted, device);
            assert_eq!(first_difference(&one, &many), None, "{what} (run {run}): the master differs");
            assert_eq!(one_tracks.len(), many_tracks.len());
            for (t, (a, b)) in one_tracks.iter().zip(&many_tracks).enumerate() {
                assert_eq!(first_difference(a, b), None, "{what} (run {run}): track {t} differs");
            }
        }
    }

    /// spec: audio/multicore.md: a dense project renders sample for sample the same on several cores as on one.
    #[test]
    fn a_dense_project_renders_the_same_on_several_cores() {
        let pool = Pool::new(3, None);
        let p = dense(1, 24, 16);
        check(&p, &pool, false, false, "own effects");
        check(&p, &pool, true, false, "hosted effects");
        check(&p, &pool, true, true, "on a device");
    }

    /// A hosted instrument with state: a saw per held key, its delay as reported, its clock in its output (so the
    /// order of its process calls shows when channels share it).
    pub struct Synth {
        keys: Vec<(u8, f32, f32)>,
        calls: u32,
        delay: u32,
    }

    impl crate::Instrument for Synth {
        fn note_on(&mut self, _offset: u32, key: u8, velocity: f32) {
            self.keys.push((key, 0.0, velocity));
        }

        fn note_off(&mut self, _offset: u32, key: u8) {
            self.keys.retain(|k| k.0 != key);
        }

        fn process(&mut self, rate: u32, left: &mut [f32], right: &mut [f32]) {
            self.calls += 1;
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let mut x = 0.0;
                for (key, phase, vel) in &mut self.keys {
                    *phase = (*phase + 440.0 * 2f32.powf((f32::from(*key) - 69.0) / 12.0) / rate as f32).fract();
                    x += (*phase - 0.5) * *vel * 0.2;
                }
                (*l, *r) = (x, x * 0.5 + self.calls as f32 * 1e-6);
            }
        }

        fn reported_delay(&self) -> u32 {
            self.delay
        }
    }

    /// spec: audio/multicore.md: hosted instruments render on several cores as on one, channels sharing an instance
    /// in channel order, alongside the sampler voices.
    #[test]
    fn hosted_instruments_render_the_same_on_several_cores() {
        let p = dense(3, 16, 16);
        let pool = Pool::new(3, None);
        let render = |pool: Option<Arc<Pool>>| {
            let mut lib = SampleLibrary::new(None);
            let mut e = Engine::new(&p, &mut lib, 44100, PlayMode::Pattern(1));
            e.set_threads(pool);
            let shared: SharedInstrument = Arc::new(Mutex::new(Synth { keys: vec![], calls: 0, delay: 0 }));
            for c in 0..10 {
                let inst: SharedInstrument = match c {
                    3 | 6 | 9 => shared.clone(),
                    _ => Arc::new(Mutex::new(Synth { keys: vec![], calls: 0, delay: c as u32 * 3 })),
                };
                e.set_instrument(c, inst);
            }
            let mut out = Vec::new();
            e.render_ticks(2 * 96 + 40, &mut out);
            (out, e.multicore.gen_ran)
        };
        let (one, _) = render(None);
        assert!(one.iter().any(|s| s.abs() > 1e-3));
        for run in 0..3 {
            let (many, ran) = render(Some(pool.clone()));
            assert!(ran > 0);
            assert_eq!(first_difference(&one, &many), None, "run {run}");
        }
    }

    /// spec: audio/multicore.md: many tracks and routes in random shapes (sends forward and back, loops, sidechain
    /// inputs), many workers.
    #[test]
    fn random_routings_render_the_same_on_several_cores() {
        let pool = Pool::new(7, None);
        for seed in 0..4u64 {
            let mut p = dense(seed + 10, 48, 12);
            let mut rng = Lcg(seed);
            let n = p.mixer.len() as u32;
            // Extra sends anywhere (loops included) and sidechain inputs by any input number.
            for _ in 0..80 {
                let (s, d) = (1 + rng.below(n - 1), rng.below(n));
                let m = &mut p.mixer[s as usize];
                if s != d && !m.routes.contains(&(d as u16)) {
                    m.routes.push(d as u16);
                    m.send_levels.push((d as u16, f64::from(rng.below(3)) / 2.0));
                }
            }
            for _ in 0..20 {
                let t = 1 + rng.below(n - 1) as usize;
                p.mixer[t].slots[9] = Some(limiter(1 + rng.below(4) as i32));
            }
            check(&p, &pool, seed % 2 == 0, seed % 3 == 0, &format!("seed {seed}"));
        }
    }

    /// The speed-up on this machine: dense projects rendered on one thread, then on pools of several sizes (each
    /// checked against the one-thread render; the best of three runs each). `cargo test -p os-engine --release multicore_bench -- --ignored
    /// --nocapture`; `OS_BENCH_TICKS=n` renders more or fewer ticks (768, about 1.9 s of audio, by default).
    #[test]
    #[ignore]
    fn multicore_bench() {
        let ticks: usize = std::env::var("OS_BENCH_TICKS").ok().and_then(|v| v.parse().ok()).unwrap_or(8 * 96);
        let heavy = |seed: u64, inserts: u16, channels: u16| {
            let mut p = dense(seed, inserts, channels);
            let mut rng = Lcg(seed + 99);
            // Every insert gets three more own effects, a reverb among them.
            for t in 1..=usize::from(inserts) {
                p.mixer[t].slots[1] = Some(effect(0));
                p.mixer[t].slots[2] = Some(effect(rng.next()));
                p.mixer[t].slots[3] = Some(effect(rng.next()));
            }
            p
        };
        let projects = [
            ("mixer-heavy: 96 inserts with 4-5 effects each, 24 channels", heavy(5, 96, 24)),
            ("dense: 48 inserts with 4-5 effects each, 48 channels", heavy(6, 48, 48)),
            ("generator-heavy: 12 inserts, 96 channels", dense(7, 12, 96)),
        ];
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        let mut sizes = vec![1, 3, crate::parallel::default_workers(None)];
        sizes.sort_unstable();
        sizes.dedup();
        println!("{logical} logical cores; {ticks} ticks per render");
        for (what, p) in &projects {
            let run = |pool: Option<Arc<Pool>>| {
                let mut best = (f64::MAX, Vec::new());
                for _ in 0..3 {
                    let mut lib = SampleLibrary::new(None);
                    let mut e = Engine::new(p, &mut lib, 44100, PlayMode::Pattern(1));
                    e.set_threads(pool.clone());
                    let mut out = Vec::new();
                    let t = std::time::Instant::now();
                    e.render_ticks(ticks, &mut out);
                    best = (best.0.min(t.elapsed().as_secs_f64()), out);
                }
                best
            };
            let (one, reference) = run(None);
            let secs = reference.len() as f64 / 2.0 / 44100.0;
            println!("{what}: {secs:.2} s of audio");
            println!("  1 thread: {one:.3} s ({:.1}x real time)", secs / one);
            for &w in &sizes {
                let pool = Pool::new(w, None);
                let (t, out) = run(Some(pool));
                assert_eq!(first_difference(&reference, &out), None, "{what}: {w} workers");
                println!("  {} threads: {t:.3} s ({:.1}x real time), {:.2}x faster", w + 1, secs / t, one / t);
            }
        }
    }
}
