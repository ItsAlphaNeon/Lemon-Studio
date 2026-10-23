// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Keep on disk (**Lemon Studio**): the played sample read from its file while it plays instead of held whole in
//! memory. Only a file whose frames are the played sample as they are streams (no precomputed step changes them), and
//! only uncompressed PCM, whose frames [`Layout`] finds in the file.
//!
//! - [`Played`] holds the frames around every point a voice can start at (the sample's start, START OFFSET's frame, the
//!   loop start, and points learned from notes that started further in) in memory, so a voice starts at once.
//! - [`Disk`] keeps the file open, one ring of frames per voice, and a reader thread that keeps each ring filled ahead
//!   of its voice. The audio thread only reads what is there: no file access, no allocation, no lock.
//! - An offline render waits: a voice whose frames are not in yet reads them itself. Either way a voice reads the
//!   same frames in the same order as from the sample in memory, so what it plays is the same to the bit.
//!
// spec: plugins/lemon-sampler.md "Keep on disk (Lemon Studio)"

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::Thread;

use os_dsp::resample::{self, Quality, N_MAX};
use os_dsp::tone::Tones;

use crate::dsp::MAX_VOICES;

/// Frames held in memory from each point a voice starts at (0.68 s at 48 kHz): the reader has that long to start the
/// voice's ring.
pub const HEAD: usize = 1 << 15;
/// Frames held before each point: the most a read reaches back.
pub const BEFORE: usize = N_MAX;
/// Frames in a voice's ring (a power of two).
pub const RING: usize = 1 << 16;
/// Frames the reader reads at a time.
const CHUNK: usize = 1 << 13;
/// A voice's ring starts this far before the end of the held frames it plays, so a read that leaves them finds all
/// its frames in the ring.
const OVERLAP: usize = 2 * N_MAX + 2;
/// Frames a voice keeps behind its reads: a rising pitch reaches further back.
const KEEP: usize = 2 * N_MAX;

static EXPORTING: AtomicBool = AtomicBool::new(false);

/// The host renders an export (Lemon Studio sets it for every export, high quality or not): voices wait for their
/// frames as in an offline render.
pub fn set_exporting(on: bool) {
    EXPORTING.store(on, Ordering::Release);
}

pub fn exporting() -> bool {
    EXPORTING.load(Ordering::Acquire)
}

static OPEN: AtomicUsize = AtomicUsize::new(0);

/// Files open for playing from disk, in every instance of the process.
pub fn open_files() -> usize {
    OPEN.load(Ordering::Acquire)
}

// ---------------------------------------------------------------------------------------------------------------
// Where the frames lie

/// How one value is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    U8,
    I8,
    I16,
    I24,
    I32,
    F32,
    F64,
}

impl Kind {
    fn bytes(self) -> usize {
        match self {
            Kind::U8 | Kind::I8 => 1,
            Kind::I16 => 2,
            Kind::I24 => 3,
            Kind::I32 | Kind::F32 => 4,
            Kind::F64 => 8,
        }
    }
}

/// Where an uncompressed file's frames lie and how they are stored: WAV integer PCM of 8, 16, 24 or 32 bits and
/// floating point of 32 or 64 (also as WAVE_FORMAT_EXTENSIBLE when every bit is used), AIFF and AIFF-C (`NONE`,
/// `sowt`) integer PCM of 8, 16, 24 or 32 bits. Compressed files and anything else are decoded into memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub rate: u32,
    pub channels: u16,
    pub frames: usize,
    /// Byte offset of the first frame.
    data: u64,
    kind: Kind,
    big: bool,
}

impl Layout {
    /// The layout of the file at `path`, when it is one that streams and holds all the frames its headers name.
    pub fn of(path: &Path) -> Option<Layout> {
        let mut f = File::open(path).ok()?;
        let len = f.metadata().ok()?.len();
        let mut head = [0u8; 12];
        f.read_exact(&mut head).ok()?;
        let l = match (&head[0..4], &head[8..12]) {
            (b"RIFF", b"WAVE") => wav(&mut f)?,
            (b"FORM", b"AIFF") => aiff(&mut f, false)?,
            (b"FORM", b"AIFC") => aiff(&mut f, true)?,
            _ => return None,
        };
        let ok = l.frames > 0 && l.rate > 0 && l.channels > 0 && l.data + (l.frames * l.block()) as u64 <= len;
        ok.then_some(l)
    }

    /// Bytes per frame.
    fn block(&self) -> usize {
        self.kind.bytes() * usize::from(self.channels)
    }

    /// One value as the decoder makes it (os-engine's: integers over 2^(bits - 1), unsigned 8-bit about 128).
    fn value(&self, b: &[u8]) -> f32 {
        let big = self.big;
        match self.kind {
            Kind::U8 => (f32::from(b[0]) / 128.0) - 1.0,
            Kind::I8 => f32::from(b[0] as i8) / 128.0,
            Kind::I16 => f32::from(if big { i16::from_be_bytes([b[0], b[1]]) } else { i16::from_le_bytes([b[0], b[1]]) }) / 32768.0,
            Kind::I24 => {
                let v = if big { i32::from_be_bytes([b[0], b[1], b[2], 0]) } else { i32::from_le_bytes([0, b[0], b[1], b[2]]) };
                (v >> 8) as f32 / 8_388_608.0
            }
            Kind::I32 => {
                let v = if big { i32::from_be_bytes([b[0], b[1], b[2], b[3]]) } else { i32::from_le_bytes([b[0], b[1], b[2], b[3]]) };
                (f64::from(v) / 2_147_483_648.0) as f32
            }
            Kind::F32 => f32::from_bits(if big { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) }),
            Kind::F64 => {
                let a: [u8; 8] = b[..8].try_into().unwrap_or([0; 8]);
                (if big { f64::from_be_bytes(a) } else { f64::from_le_bytes(a) }) as f32
            }
        }
    }

    /// Frames `at .. at + left.len()` (all inside the file) from `file`, through `bytes`: the first two channels, a
    /// mono file's one on both sides. A failed read gives silence.
    fn read(&self, file: &mut File, at: usize, left: &mut [f32], right: &mut [f32], bytes: &mut Vec<u8>) {
        let (w, block) = (self.kind.bytes(), self.block());
        bytes.resize(left.len() * block, 0);
        let ok = file.seek(SeekFrom::Start(self.data + (at * block) as u64)).is_ok() && file.read_exact(bytes).is_ok();
        if !ok {
            left.fill(0.0);
            right.fill(0.0);
            return;
        }
        for (k, fr) in bytes.chunks_exact(block).enumerate() {
            let l = self.value(&fr[..w]);
            left[k] = l;
            right[k] = if self.channels > 1 { self.value(&fr[w..2 * w]) } else { l };
        }
    }
}

/// A WAV file's `fmt ` and `data` chunks.
fn wav(f: &mut File) -> Option<Layout> {
    let word = |d: &[u8], i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
    let long = |d: &[u8], i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    let mut fmt: Option<(Kind, u16, u32)> = None;
    let mut chunk = [0u8; 8];
    loop {
        f.read_exact(&mut chunk).ok()?;
        let len = u64::from(long(&chunk, 4));
        match &chunk[0..4] {
            b"fmt " => {
                if !(16..=1 << 16).contains(&len) {
                    return None;
                }
                let mut d = vec![0u8; len as usize];
                f.read_exact(&mut d).ok()?;
                f.seek(SeekFrom::Current((len & 1) as i64)).ok()?;
                let (channels, rate, align, bits) = (word(&d, 2), long(&d, 4), word(&d, 12), word(&d, 14));
                // WAVE_FORMAT_EXTENSIBLE names the format in its sub-format's first two bytes; bits left unused would
                // need shifting, which the decoder does.
                let (tag, valid) = match word(&d, 0) {
                    0xFFFE if d.len() >= 26 => (word(&d, 24), word(&d, 18)),
                    0xFFFE => return None,
                    t => (t, bits),
                };
                let kind = match (tag, bits) {
                    (1, 8) => Kind::U8,
                    (1, 16) => Kind::I16,
                    (1, 24) => Kind::I24,
                    (1, 32) => Kind::I32,
                    (3, 32) => Kind::F32,
                    (3, 64) => Kind::F64,
                    _ => return None,
                };
                if valid != bits || usize::from(align) != kind.bytes() * usize::from(channels) {
                    return None;
                }
                fmt = Some((kind, channels, rate));
            }
            b"data" => {
                let (kind, channels, rate) = fmt?;
                let data = f.stream_position().ok()?;
                let frames = len as usize / (kind.bytes() * usize::from(channels).max(1));
                return Some(Layout { rate, channels, frames, data, kind, big: false });
            }
            _ => {
                f.seek(SeekFrom::Current((len + (len & 1)) as i64)).ok()?;
            }
        }
    }
}

/// An AIFF / AIFF-C file's `COMM` and `SSND` chunks (big-endian).
fn aiff(f: &mut File, aifc: bool) -> Option<Layout> {
    let word = |d: &[u8], i: usize| u16::from_be_bytes([d[i], d[i + 1]]);
    let long = |d: &[u8], i: usize| u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    let mut comm: Option<(Kind, bool, u16, usize, u32)> = None;
    let mut chunk = [0u8; 8];
    loop {
        f.read_exact(&mut chunk).ok()?;
        let len = u64::from(long(&chunk, 4));
        match &chunk[0..4] {
            b"COMM" => {
                if !(18..=1 << 12).contains(&len) {
                    return None;
                }
                let mut d = vec![0u8; len as usize];
                f.read_exact(&mut d).ok()?;
                f.seek(SeekFrom::Current((len & 1) as i64)).ok()?;
                let (channels, frames, size) = (word(&d, 0), long(&d, 2) as usize, word(&d, 6));
                let rate = extended(&d[8..18]);
                if !(rate.is_finite() && rate >= 1.0) {
                    return None;
                }
                let comp = if aifc { d.get(18..22)? } else { b"NONE" };
                let (kind, big) = match (comp, size) {
                    (b"NONE" | b"none", 8) => (Kind::I8, true),
                    (b"NONE" | b"none", 16) => (Kind::I16, true),
                    (b"NONE" | b"none", 24) => (Kind::I24, true),
                    (b"NONE" | b"none", 32) => (Kind::I32, true),
                    (b"sowt", 16) => (Kind::I16, false),
                    _ => return None,
                };
                comm = Some((kind, big, channels, frames, rate as u32));
            }
            b"SSND" => {
                let (kind, big, channels, frames, rate) = comm?;
                let mut d = [0u8; 8];
                f.read_exact(&mut d).ok()?;
                let offset = u64::from(long(&d, 0));
                let data = f.stream_position().ok()? + offset;
                let block = (kind.bytes() * usize::from(channels).max(1)) as u64;
                // The frames the headers name, all of them in the chunk.
                let room = len.checked_sub(8 + offset)? / block;
                return (room == frames as u64).then_some(Layout { rate, channels, frames, data, kind, big });
            }
            _ => {
                f.seek(SeekFrom::Current((len + (len & 1)) as i64)).ok()?;
            }
        }
    }
}

/// The length in frames and the rate of a file that can play from disk ([`Layout::of`]), from its headers: the frames
/// and rate decoding it gives. Lemon Studio's engine keeps only these of a sample Lemon Sampler plays, not its decoded
/// frames (`os_engine::SampleLibrary::length_reader`).
pub fn length(path: &Path) -> Option<(usize, u32)> {
    Layout::of(path).map(|l| (l.frames, l.rate))
}

/// An 80-bit IEEE extended number (AIFF's sample rate).
fn extended(b: &[u8]) -> f64 {
    let exp = (i32::from(b[0] & 0x7f) << 8 | i32::from(b[1])) - 16383 - 63;
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    let v = mant as f64 * 2f64.powi(exp);
    if b[0] & 0x80 != 0 { -v } else { v }
}

// ---------------------------------------------------------------------------------------------------------------
// The file, the rings and the reader

fn pack(req: u32, x: usize) -> u64 {
    (u64::from(req) << 32) | (x as u64 & 0xffff_ffff)
}

fn unpack(v: u64) -> (u32, usize) {
    ((v >> 32) as u32, (v & 0xffff_ffff) as usize)
}

/// A voice's frames ahead of it, as `f32` bits.
struct Ring {
    left: Box<[AtomicU32]>,
    right: Box<[AtomicU32]>,
}

impl Ring {
    fn new() -> Ring {
        let side = || (0..RING).map(|_| AtomicU32::new(0)).collect();
        Ring { left: side(), right: side() }
    }

    /// Frame `k` from the ring's start.
    #[inline]
    fn at(&self, k: usize) -> (f32, f32) {
        let i = k & (RING - 1);
        (f32::from_bits(self.left[i].load(Ordering::Relaxed)), f32::from_bits(self.right[i].load(Ordering::Relaxed)))
    }
}

/// What a ring's filler knows: the request it fills (its number and first frame), how many frames it has put in, and
/// its buffers.
#[derive(Default)]
struct Fill {
    req: u32,
    start: usize,
    filled: usize,
    bytes: Vec<u8>,
    left: Vec<f32>,
    right: Vec<f32>,
}

/// One voice's ring. The voice (audio thread) sets `want` (a new request number and the first frame) and `used` (frames
/// from the first it no longer needs); the filler sets `have` (frames from the first that are in). Each value carries
/// the number of the request it belongs to, so a voice never takes frames of an earlier request.
#[derive(Default)]
struct Slot {
    busy: AtomicBool,
    want: AtomicU64,
    have: AtomicU64,
    used: AtomicU64,
    ring: OnceLock<Ring>,
    fill: Mutex<Fill>,
}

impl Slot {
    /// Reads the next frames of the slot's request into its ring: as many as fit up to a chunk, when at least `min` fit
    /// (or the rest of the file does). True when it read any.
    fn fill_some(&self, inner: &Inner, f: &mut Fill, min: usize) -> bool {
        let (req, start) = unpack(self.want.load(Ordering::Acquire));
        if req != f.req {
            (f.req, f.start, f.filled) = (req, start, 0);
            self.have.store(pack(req, 0), Ordering::Release);
        }
        let frames = inner.layout.frames;
        let at = f.start + f.filled;
        if at >= frames {
            return false;
        }
        let (ug, used) = unpack(self.used.load(Ordering::Acquire));
        let used = if ug == req { used } else { 0 };
        let k = (used + RING).saturating_sub(f.filled).min(CHUNK).min(frames - at);
        if k == 0 || k < min.min(frames - at) {
            return false;
        }
        let ring = self.ring.get_or_init(|| {
            inner.rings.fetch_add(1, Ordering::Relaxed);
            Ring::new()
        });
        f.left.resize(CHUNK, 0.0);
        f.right.resize(CHUNK, 0.0);
        if let Ok(mut file) = inner.file.lock() {
            inner.layout.read(&mut file, at, &mut f.left[..k], &mut f.right[..k], &mut f.bytes);
        }
        for i in 0..k {
            let j = (f.filled + i) & (RING - 1);
            ring.left[j].store(f.left[i].to_bits(), Ordering::Relaxed);
            ring.right[j].store(f.right[i].to_bits(), Ordering::Relaxed);
        }
        f.filled += k;
        inner.read.fetch_add(k, Ordering::Relaxed);
        self.have.store(pack(req, f.filled), Ordering::Release);
        true
    }

    /// Fills the ring until `need` frames of request `req` are in (an offline render, on its own thread). False when
    /// it gets no further.
    fn fill_to(&self, inner: &Inner, req: u32, need: usize) -> bool {
        let mut f = self.fill.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let (g, filled) = unpack(self.have.load(Ordering::Acquire));
            if g == req && filled >= need {
                return true;
            }
            if !self.fill_some(inner, &mut f, 1) {
                return false;
            }
        }
    }
}

struct Inner {
    layout: Layout,
    file: Mutex<File>,
    slots: Box<[Slot]>,
    closed: AtomicBool,
    /// The reader thread runs.
    alive: Arc<AtomicBool>,
    /// Rings made so far, and frames read into them.
    rings: AtomicUsize,
    read: AtomicUsize,
}

impl Drop for Inner {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Inner {
    /// The reader thread: each pass gives every ring in use a chunk; it sleeps when no ring has room, until a voice
    /// wakes it.
    fn run(&self) {
        while !self.closed.load(Ordering::Acquire) {
            let mut progress = false;
            for slot in self.slots.iter().filter(|s| s.busy.load(Ordering::Acquire)) {
                // An offline render may be filling the ring itself.
                if let Ok(mut f) = slot.fill.try_lock() {
                    // Whole chunks only: fewer, larger reads.
                    progress |= slot.fill_some(self, &mut f, CHUNK);
                }
            }
            if !progress {
                std::thread::park();
            }
        }
    }
}

/// A file kept on disk: open for as long as a played sample uses it, with its rings and reader thread. Dropping it
/// stops the reader, which then closes the file and frees the rings on its own thread.
pub struct Disk {
    inner: Arc<Inner>,
    reader: Option<Thread>,
}

impl Disk {
    pub fn open(path: &Path, layout: Layout) -> std::io::Result<Arc<Disk>> {
        let file = File::open(path)?;
        let inner = Arc::new(Inner {
            layout,
            file: Mutex::new(file),
            slots: (0..MAX_VOICES).map(|_| Slot::default()).collect(),
            closed: AtomicBool::new(false),
            alive: Arc::new(AtomicBool::new(true)),
            rings: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        });
        OPEN.fetch_add(1, Ordering::AcqRel);
        let r = inner.clone();
        let reader = std::thread::Builder::new()
            .name("lemon-sampler-disk".into())
            .spawn(move || {
                r.run();
                r.alive.store(false, Ordering::Release);
            })
            .map(|h| h.thread().clone());
        if reader.is_err() {
            inner.alive.store(false, Ordering::Release);
        }
        Ok(Arc::new(Disk { inner, reader: reader.ok() }))
    }

    pub fn layout(&self) -> &Layout {
        &self.inner.layout
    }

    /// Wakes the reader (the audio thread, after a block that played from the file).
    pub fn wake(&self) {
        if let Some(t) = &self.reader {
            t.unpark();
        }
    }

    /// Frames `start .. start + left.len()` read now (not on the audio thread); silence past the end.
    pub fn read_frames(&self, start: usize, left: &mut [f32], right: &mut [f32]) {
        let l = &self.inner.layout;
        let k = left.len().min(l.frames.saturating_sub(start));
        left[k..].fill(0.0);
        right[k..].fill(0.0);
        if k > 0 {
            let mut bytes = Vec::new();
            if let Ok(mut f) = self.inner.file.lock() {
                l.read(&mut f, start, &mut left[..k], &mut right[..k], &mut bytes);
            }
        }
    }

    /// Whether the reader thread still runs (it stops once the disk is dropped).
    pub fn reader_alive(&self) -> Arc<AtomicBool> {
        self.inner.alive.clone()
    }

    /// Rings made so far: each holds [`RING`] frames.
    pub fn rings(&self) -> usize {
        self.inner.rings.load(Ordering::Relaxed)
    }

    /// Rings a voice is using now.
    pub fn busy(&self) -> usize {
        self.inner.slots.iter().filter(|s| s.busy.load(Ordering::Acquire)).count()
    }

    /// Frames the reader and offline renders have read into rings so far.
    pub fn frames_read(&self) -> usize {
        self.inner.read.load(Ordering::Relaxed)
    }

    /// The reader has done all it can until the voices read on: every ring in use holds its request's frames up to
    /// the file's end or as far as its room allows (tests: a reader that keeps up).
    #[cfg(test)]
    pub(crate) fn settled(&self) -> bool {
        let frames = self.inner.layout.frames;
        self.inner.slots.iter().filter(|s| s.busy.load(Ordering::Acquire)).all(|s| {
            let (req, start) = unpack(s.want.load(Ordering::Acquire));
            let (g, filled) = unpack(s.have.load(Ordering::Acquire));
            let (ug, used) = unpack(s.used.load(Ordering::Acquire));
            let used = if ug == req { used } else { 0 };
            let rest = frames.saturating_sub(start + filled);
            g == req && (rest == 0 || (used + RING).saturating_sub(filled) < CHUNK.min(rest))
        })
    }

    /// A free ring for a voice.
    fn take(&self) -> Option<u16> {
        let free = |s: &Slot| s.busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok();
        self.inner.slots.iter().position(free).map(|i| i as u16)
    }
}

impl Drop for Disk {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::Release);
        self.wake();
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The played sample on disk

/// Frames held in memory: `left[i]`, `right[i]` are frame `start + i`.
pub struct Held {
    start: usize,
    left: Vec<f32>,
    right: Vec<f32>,
}

impl Held {
    fn end(&self) -> usize {
        self.start + self.left.len()
    }

    #[inline]
    fn at(&self, x: usize) -> (f32, f32) {
        (self.left[x - self.start], self.right[x - self.start])
    }
}

/// Where a voice reads the file: its ring (when it has one), the request the ring fills and the frames from the
/// request's first that the voice let go of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Feed {
    slot: Option<u16>,
    req: u32,
    start: usize,
    released: usize,
}

impl Feed {
    pub fn active(&self) -> bool {
        self.slot.is_some()
    }
}

/// Points voices start at, as a played sample of `frames` frames holds them: inside it, in order, each once.
pub fn points(points: &[usize], frames: usize) -> Vec<usize> {
    let mut p: Vec<usize> = points.iter().copied().filter(|&p| p < frames).collect();
    p.sort_unstable();
    p.dedup();
    p
}

/// Points a played sample learns to hold at most, besides the ones its settings name.
pub const LEARN: usize = 16;

/// A played sample kept on disk: the file, and the frames held around the points voices start at: the ones the
/// settings name (the sample's start, START OFFSET's frame, the loop start) and ones learned from notes that started
/// further in (trimmed audio clips, playback started inside a clip) where nothing was held.
pub struct Played {
    disk: Arc<Disk>,
    held: Vec<Arc<Held>>,
    named: Vec<usize>,
    learned: Vec<usize>,
    points: Vec<usize>,
    /// Points notes started at with nothing held there, frame + 1 (0: free), until a new played sample holds them.
    missed: [AtomicUsize; 8],
    /// A point was missed since the last [`Played::news`].
    news: AtomicBool,
}

impl Played {
    /// Holds the frames around the `named` points and the points an `old` played sample of the same disk learned or
    /// missed, reading them now; regions the old one holds already are shared.
    pub fn new(disk: Arc<Disk>, named: &[usize], old: Option<&Played>) -> Played {
        let n = disk.layout().frames;
        let named = self::points(named, n);
        let old = old.filter(|o| Arc::ptr_eq(&o.disk, &disk));
        let mut learned: Vec<usize> = Vec::new();
        for p in old.map_or_else(Vec::new, |o| o.learned.iter().copied().chain(o.missed()).collect()) {
            if p < n && !named.contains(&p) && !learned.contains(&p) && learned.len() < LEARN {
                learned.push(p);
            }
        }
        let points = self::points(&[named.as_slice(), learned.as_slice()].concat(), n);
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for &p in &points {
            let (a, b) = (p.saturating_sub(BEFORE), (p + HEAD).min(n));
            match spans.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => spans.push((a, b)),
            }
        }
        let held = spans
            .into_iter()
            .map(|(a, b)| {
                if let Some(h) = old.and_then(|o| o.held.iter().find(|h| h.start == a && h.end() == b)) {
                    return h.clone();
                }
                let (mut left, mut right) = (vec![0.0; b - a], vec![0.0; b - a]);
                disk.read_frames(a, &mut left, &mut right);
                Arc::new(Held { start: a, left, right })
            })
            .collect();
        Played { disk, held, named, learned, points, missed: Default::default(), news: AtomicBool::new(false) }
    }

    pub fn disk(&self) -> &Arc<Disk> {
        &self.disk
    }

    pub fn frames(&self) -> usize {
        self.disk.layout().frames
    }

    pub fn rate(&self) -> u32 {
        self.disk.layout().rate
    }

    /// The points it holds frames around.
    pub fn points(&self) -> &[usize] {
        &self.points
    }

    /// The points the settings name.
    pub fn named(&self) -> &[usize] {
        &self.named
    }

    /// Points notes started at with nothing held there since it was made.
    pub fn missed(&self) -> Vec<usize> {
        self.missed.iter().map(|m| m.load(Ordering::Acquire)).filter(|&m| m > 0).map(|m| m - 1).collect()
    }

    /// True once after a note started where nothing is held (the audio thread then asks for a new played sample).
    pub fn news(&self) -> bool {
        self.news.swap(false, Ordering::AcqRel)
    }

    /// A voice starts at `pos`. Where nothing is held its ring starts filling at once (it plays as soon as the frames
    /// are in) and, with `learn` (a note that starts further in), the point is noted, to be held by the next played
    /// sample. Never blocks or allocates (audio thread).
    pub fn start_at(&self, feed: &mut Feed, pos: f64, learn: bool) {
        let n = self.frames();
        let x = (pos.max(0.0) as usize).min(n);
        if x >= n || self.held.iter().any(|h| h.start <= x.saturating_sub(BEFORE) && (x + OVERLAP <= h.end() || h.end() == n)) {
            return;
        }
        self.aim(feed, x.saturating_sub(BEFORE));
        let noted = self.learned.len() + self.missed.iter().filter(|m| m.load(Ordering::Relaxed) > 0).count();
        if learn && noted < LEARN && !self.missed.iter().any(|m| m.load(Ordering::Relaxed) == x + 1) {
            if self.missed.iter().any(|m| m.compare_exchange(0, x + 1, Ordering::AcqRel, Ordering::Relaxed).is_ok()) {
                self.news.store(true, Ordering::Release);
            }
        }
    }

    /// Frames held in memory.
    pub fn held_frames(&self) -> usize {
        self.held.iter().map(|h| h.left.len()).sum()
    }

    /// Points `feed` at a ring filling from frame `start`, taking a ring when it has none. A ring that already fills
    /// from there is kept while it still holds that frame (a voice back in a loop has read past it: a new request).
    fn aim(&self, feed: &mut Feed, start: usize) {
        if feed.slot.is_some() && feed.start == start && feed.released == 0 {
            return;
        }
        let Some(i) = feed.slot.or_else(|| self.disk.take()) else { return };
        let slot = &self.disk.inner.slots[usize::from(i)];
        let req = unpack(slot.want.load(Ordering::Relaxed)).0.wrapping_add(1).max(1);
        slot.used.store(pack(req, 0), Ordering::Release);
        slot.want.store(pack(req, start), Ordering::Release);
        *feed = Feed { slot: Some(i), req, start, released: 0 };
        self.disk.wake();
    }

    /// One frame read at `pos` and `ratio` as [`resample::read`] reads the sample in memory: from the held frames or
    /// the voice's ring. `None` while its frames are not in (live playback; the reader is asked for them). With `wait`
    /// (an offline render) the frames are read now instead.
    pub fn read(&self, feed: &mut Feed, pos: f64, ratio: f64, quality: Quality, wait: bool) -> Option<(f32, f32)> {
        let n = self.frames();
        let (lo, hi) = resample::reach(n, pos, ratio);
        loop {
            // The voice's ring, once the frames are in.
            if let Some(i) = feed.slot {
                let slot = &self.disk.inner.slots[usize::from(i)];
                let (g, filled) = unpack(slot.have.load(Ordering::Acquire));
                if g == feed.req && lo >= feed.start + feed.released && hi < feed.start + filled {
                    if let Some(ring) = slot.ring.get() {
                        let rel = lo.saturating_sub(KEEP).saturating_sub(feed.start);
                        if rel > feed.released {
                            feed.released = rel;
                            slot.used.store(pack(feed.req, rel), Ordering::Release);
                        }
                        let s = feed.start;
                        return Some(resample::read_by(n, pos, ratio, quality, |x| ring.at(x - s)));
                    }
                }
            }
            // Held frames; the ring is started near their end, ready for when the voice leaves them.
            if let Some(h) = self.held.iter().find(|h| h.start <= lo && hi < h.end()) {
                if h.end() < n {
                    self.aim(feed, h.end().saturating_sub(OVERLAP).max(h.start));
                }
                return Some(resample::read_by(n, pos, ratio, quality, |x| h.at(x)));
            }
            // Not in: the ring is on its way here, or is sent here.
            let coming = feed.slot.is_some() && lo >= feed.start + feed.released && hi < feed.start + feed.released + RING;
            if !coming {
                self.aim(feed, lo.saturating_sub(BEFORE));
            }
            let i = feed.slot?;
            if !wait || !self.disk.inner.slots[usize::from(i)].fill_to(&self.disk.inner, feed.req, hi + 1 - feed.start) {
                return None;
            }
        }
    }

    /// The voice is over: its ring is free.
    pub fn release(&self, feed: &mut Feed) {
        if let Some(i) = feed.slot.take() {
            self.disk.inner.slots[usize::from(i)].busy.store(false, Ordering::Release);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// What the editor draws

/// What the editor draws of a sample kept on disk, read once in the background: every `step`-th frame (at most
/// [`Summary::POINTS`] of them), whether the sides differ, and the tones of colourful waveforms.
pub struct Summary {
    pub step: usize,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub stereo: bool,
    pub tones: Tones,
}

impl Summary {
    pub const POINTS: usize = 1 << 18;

    /// Reads the whole file a chunk at a time (never all of it in memory).
    pub fn read(path: &Path, layout: &Layout) -> Option<Summary> {
        let mut file = File::open(path).ok()?;
        let n = layout.frames;
        let step = n.div_ceil(Self::POINTS).max(1);
        // A whole number of tone blocks per chunk: the tones come out as those of the whole.
        const PART: usize = 64 * os_dsp::tone::BLOCK;
        let (mut l, mut r, mut bytes) = (vec![0.0f32; PART], vec![0.0f32; PART], Vec::new());
        let mut s = Summary { step, left: Vec::with_capacity(n / step + 1), right: Vec::with_capacity(n / step + 1), stereo: false, tones: Tones { rate: layout.rate, blocks: Vec::new() } };
        let mut at = 0;
        while at < n {
            let k = PART.min(n - at);
            layout.read(&mut file, at, &mut l[..k], &mut r[..k], &mut bytes);
            s.stereo |= l[..k] != r[..k];
            s.tones.blocks.extend(Tones::analyse_stereo(&l[..k], &r[..k], layout.rate).blocks);
            let mut j = (at.div_ceil(step) * step) - at;
            while j < k {
                s.left.push(l[j]);
                s.right.push(r[j]);
                j += step;
            }
            at += k;
        }
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dir() -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("lemon-sampler-stream-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A RIFF file of `tag` (1 PCM, 3 float, 0xFFFE extensible with `valid` bits) holding `data`.
    fn wav_file(tag: u16, channels: u16, bits: u16, valid: u16, rate: u32, data: &[u8]) -> Vec<u8> {
        let align = channels * bits / 8;
        let mut fmt = Vec::new();
        fmt.extend(tag.to_le_bytes());
        fmt.extend(channels.to_le_bytes());
        fmt.extend(rate.to_le_bytes());
        fmt.extend((rate * u32::from(align)).to_le_bytes());
        fmt.extend(align.to_le_bytes());
        fmt.extend(bits.to_le_bytes());
        if tag == 0xFFFE {
            fmt.extend(22u16.to_le_bytes());
            fmt.extend(valid.to_le_bytes());
            fmt.extend(0u32.to_le_bytes());
            fmt.extend(1u16.to_le_bytes());
            fmt.extend([0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
        }
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        // A chunk before `fmt ` and one of odd length between it and `data`, as files have.
        b.extend(b"JUNK");
        b.extend(4u32.to_le_bytes());
        b.extend([0; 4]);
        b.extend(b"fmt ");
        b.extend((fmt.len() as u32).to_le_bytes());
        b.extend(fmt);
        b.extend(b"xtra");
        b.extend(3u32.to_le_bytes());
        b.extend([1, 2, 3, 0]);
        b.extend(b"data");
        b.extend((data.len() as u32).to_le_bytes());
        b.extend(data);
        let len = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&len.to_le_bytes());
        b
    }

    /// An AIFF (or AIFF-C with `comp`) file of `size`-bit frames at 44100 Hz.
    fn aiff_file(comp: Option<&[u8; 4]>, channels: u16, size: u16, frames: u32, data: &[u8]) -> Vec<u8> {
        let mut comm = Vec::new();
        comm.extend(channels.to_be_bytes());
        comm.extend(frames.to_be_bytes());
        comm.extend(size.to_be_bytes());
        // 44100 as an 80-bit extended number.
        comm.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        if let Some(c) = comp {
            comm.extend(c);
            comm.extend([0, 0]);
        }
        let mut b = b"FORM\0\0\0\0".to_vec();
        b.extend(if comp.is_some() { b"AIFC" } else { b"AIFF" });
        b.extend(b"COMM");
        b.extend((comm.len() as u32).to_be_bytes());
        b.extend(comm);
        b.extend(b"SSND");
        b.extend((data.len() as u32 + 8).to_be_bytes());
        b.extend([0; 8]);
        b.extend(data);
        let len = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&len.to_be_bytes());
        b
    }

    /// Bytes of a varied signal: `frames` frames of `channels` values of `width` bytes, every byte pattern turning up.
    fn noise(frames: usize, channels: usize, width: usize) -> Vec<u8> {
        let mut x: u32 = 0x1234_5678;
        (0..frames * channels * width)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 24) as u8
            })
            .collect()
    }

    // The frames read from the file are the decoder's (os-engine), to the bit, in every format that streams.
    #[test]
    fn streamed_frames_are_the_decoded_ones() {
        let d = dir();
        let float32: Vec<u8> = (0..1000 * 2).flat_map(|i| ((i as f32 * 0.37).sin() * 1.3).to_le_bytes()).collect();
        let float64: Vec<u8> = (0..1000).flat_map(|i| ((i as f64 * 0.11).cos() * 0.9).to_le_bytes()).collect();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("u8 mono", wav_file(1, 1, 8, 8, 22050, &noise(1000, 1, 1))),
            ("i16 stereo", wav_file(1, 2, 16, 16, 44100, &noise(1000, 2, 2))),
            ("i16 mono", wav_file(1, 1, 16, 16, 44100, &noise(999, 1, 2))),
            ("i24 stereo", wav_file(1, 2, 24, 24, 48000, &noise(1000, 2, 3))),
            ("i32 stereo", wav_file(1, 2, 32, 32, 96000, &noise(1000, 2, 4))),
            ("f32 stereo", wav_file(3, 2, 32, 32, 48000, &float32)),
            ("f64 mono", wav_file(3, 1, 64, 64, 48000, &float64)),
            ("extensible i24 x4", wav_file(0xFFFE, 4, 24, 24, 48000, &noise(500, 4, 3))),
            ("aiff i16", aiff_file(None, 2, 16, 1000, &noise(1000, 2, 2))),
            ("aiff i24", aiff_file(None, 1, 24, 1000, &noise(1000, 1, 3))),
            ("aiff i32", aiff_file(None, 2, 32, 1000, &noise(1000, 2, 4))),
            ("aiff i8", aiff_file(None, 1, 8, 1000, &noise(1000, 1, 1))),
            ("aifc none", aiff_file(Some(b"NONE"), 2, 16, 1000, &noise(1000, 2, 2))),
            ("aifc sowt", aiff_file(Some(b"sowt"), 2, 16, 1000, &noise(1000, 2, 2))),
        ];
        for (what, bytes) in cases {
            let ext = if what.starts_with("ai") { "aif" } else { "wav" };
            let p = d.join(format!("x.{ext}"));
            std::fs::write(&p, &bytes).unwrap();
            let want = os_engine::sample::load(&p).unwrap_or_else(|e| panic!("{what}: {e}"));
            let l = Layout::of(&p).unwrap_or_else(|| panic!("{what}: streams"));
            assert_eq!((l.frames, l.rate), (want.frames(), want.rate), "{what}");
            // What Lemon Studio's engine keeps in place of the decode.
            assert_eq!(length(&p), Some((want.frames(), want.rate)), "{what}");
            let disk = Disk::open(&p, l).unwrap();
            let (mut a, mut b) = (vec![0.0; l.frames], vec![0.0; l.frames]);
            disk.read_frames(0, &mut a, &mut b);
            let bits = |x: &[f32]| x.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
            assert_eq!(bits(&a), bits(&want.left), "{what}: left");
            assert_eq!(bits(&b), bits(&want.right), "{what}: right");
        }
        std::fs::remove_dir_all(&d).ok();
    }

    // Files that do not stream: compressed, unused bits, frames missing.
    #[test]
    fn other_files_are_not_streamed() {
        let d = dir();
        let p = d.join("x.wav");
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("20 of 24 bits", wav_file(0xFFFE, 2, 24, 20, 48000, &noise(100, 2, 3))),
            ("12-bit", wav_file(1, 1, 12, 12, 48000, &noise(100, 1, 2))),
            ("a-law", wav_file(6, 1, 8, 8, 8000, &noise(100, 1, 1))),
            ("cut short", {
                let mut b = wav_file(1, 2, 16, 16, 44100, &noise(100, 2, 2));
                b.truncate(b.len() - 10);
                b
            }),
            ("aiff frames missing", aiff_file(None, 2, 16, 1000, &noise(900, 2, 2))),
            ("not audio", b"hello there, this is no sound file".to_vec()),
        ];
        for (what, bytes) in cases {
            std::fs::write(&p, &bytes).unwrap();
            assert_eq!(Layout::of(&p), None, "{what}");
            assert_eq!(length(&p), None, "{what}: decoded for its length");
        }
        std::fs::remove_dir_all(&d).ok();
    }

    // The summary keeps every step-th frame and the tones of the whole.
    #[test]
    fn the_summary_is_every_few_frames_and_the_whole_tones() {
        let d = dir();
        let p = d.join("x.wav");
        let frames = Summary::POINTS + 70_000;
        std::fs::write(&p, wav_file(1, 2, 16, 16, 44100, &noise(frames, 2, 2))).unwrap();
        let want = os_engine::sample::load(&p).unwrap();
        let s = Summary::read(&p, &Layout::of(&p).unwrap()).unwrap();
        assert_eq!(s.step, 2);
        assert_eq!(s.left.len(), frames.div_ceil(2));
        assert!(s.left.iter().enumerate().all(|(i, &v)| v == want.left[i * 2]));
        assert!(s.right.iter().enumerate().all(|(i, &v)| v == want.right[i * 2]));
        assert!(s.stereo);
        assert_eq!(s.tones, Tones::analyse_stereo(&want.left, &want.right, want.rate));
        std::fs::remove_dir_all(&d).ok();
    }
}
