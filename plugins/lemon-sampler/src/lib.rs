// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Lemon Sampler: Lemon Studio's sampler instrument as a VST3 / CLAP plugin (nih-plug).
//!
// spec: plugins/lemon-sampler.md, plugins/plugin-formats.md
#![allow(semicolon_in_expressions_from_non_local_macros)]

pub mod dsp;
pub mod editor;
pub mod fx;
pub mod params;
pub mod stream;
pub mod view;

use std::num::NonZeroU32;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;
use os_engine::{envelope, Sample};

use params::{DiskKey, FxKey, Live, SamplerParams};

/// The processed sample for one (loaded sample version, precomputed settings, Keep on disk settings) set.
pub struct Processed {
    pub version: u64,
    pub key: FxKey,
    pub disk_key: DiskKey,
    /// New whenever what the voices play changes: the engine takes each processed sample once.
    pub serial: u64,
    pub sample: Option<Arc<Sample>>,
    /// **(Lemon Studio)** The played sample kept on disk (Keep on disk), in place of `sample`.
    pub disk: Option<Arc<stream::Played>>,
    /// The played sample's loop (start and end frame, both inclusive), whether or not voices use it.
    pub looped: Option<(usize, usize)>,
    /// Frames of the played sample the whole loaded file stands for (per-note starts are fractions of the file).
    pub file_frames: f64,
}

impl Processed {
    /// Length and rate of the played sample.
    pub fn shape(&self) -> Option<(usize, u32)> {
        match (&self.disk, &self.sample) {
            (Some(d), _) => Some((d.frames(), d.rate())),
            (None, Some(s)) => Some((s.frames(), s.rate)),
            _ => None,
        }
    }
}

/// The editor's summary of a sample kept on disk: for which load, whether it was asked for, and the summary once made.
#[derive(Default)]
struct SummarySlot {
    version: u64,
    asked: bool,
    summary: Option<Arc<stream::Summary>>,
}

/// State shared by the audio thread, the background task and the editor.
pub struct Shared {
    /// The loaded file decoded, while the played sample comes from memory (none while it plays from disk).
    pub raw: Mutex<Option<Arc<Sample>>>,
    /// Bumped on every load / clear.
    pub raw_version: AtomicU64,
    pub processed: Mutex<Option<Processed>>,
    processed_version: AtomicU64,
    serial: AtomicU64,
    loaded_path: Mutex<String>,
    /// Where the loaded file's frames lie, when it is a file that can play from disk.
    layout: Mutex<Option<stream::Layout>>,
    summary: Mutex<SummarySlot>,
    /// Source bit depth (low byte) and 0x100 when the source is floating point.
    pub bits: AtomicU32,
    /// What the played sample's steps need to know of the loaded file (its loop, 16-bit mono).
    source: Mutex<fx::Source>,
    /// The host's output rate (Hz), 44100 until the host tells.
    rate: AtomicU32,
    /// Host tempo (f64 bits), 130 BPM until the host tells.
    tempo: AtomicU64,
    /// Notes the editor plays itself (key, velocity, on) when the host does not play them.
    pub previews: Mutex<Vec<(u8, f32, bool)>>,
    /// Keys sounding (bit per key, 132 keys).
    pub sounding: [AtomicU64; 3],
    busy: AtomicBool,
    again: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Shared {
            raw: Mutex::new(None),
            raw_version: AtomicU64::new(0),
            processed: Mutex::new(None),
            processed_version: AtomicU64::new(0),
            serial: AtomicU64::new(0),
            loaded_path: Mutex::new(String::new()),
            layout: Mutex::new(None),
            summary: Mutex::new(SummarySlot::default()),
            bits: AtomicU32::new(16),
            source: Mutex::new(fx::Source::default()),
            rate: AtomicU32::new(44100),
            tempo: AtomicU64::new(130f64.to_bits()),
            previews: Mutex::new(Vec::new()),
            sounding: Default::default(),
            busy: AtomicBool::new(false),
            again: AtomicBool::new(false),
        }
    }
}

/// Bit depth of a sample file's data (WAV `fmt `, FLAC STREAMINFO), | 0x100 for floating point; 16 when unknown.
pub fn source_bits(path: &Path) -> u32 {
    source_info(path).0
}

/// A sample file's bit depth as [`source_bits`] gives it, and what the played sample needs from the file beyond its
/// frames: the first loop of a WAV file's `smpl` chunk (spec: audio/sampler.md "The sample file") and whether the
/// file is 16-bit mono PCM (audio/precomputed-effects.md "Stereo delay"). A WAV file's chunks are walked without
/// reading its audio. **(Lemon Studio)** Loops are read from WAV files only (not yet from a WavPack file's wrapper).
pub fn source_info(path: &Path) -> (u32, fx::Source) {
    let mut src = fx::Source::default();
    let Ok(mut f) = std::fs::File::open(path) else {
        return (16, src);
    };
    let mut head = Vec::new();
    if (&mut f).take(64).read_to_end(&mut head).is_err() {
        return (16, src);
    }
    if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WAVE" {
        let mut bits = 16;
        if f.seek(SeekFrom::Start(12)).is_err() {
            return (bits, src);
        }
        let word = |d: &[u8], i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
        let long = |d: &[u8], i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
        let mut chunk = [0u8; 8];
        while f.read_exact(&mut chunk).is_ok() {
            let len = u64::from(long(&chunk, 4));
            let id = [chunk[0], chunk[1], chunk[2], chunk[3]];
            if (&id == b"fmt " || &id == b"smpl") && len <= 1 << 16 {
                let mut d = vec![0u8; len as usize];
                if f.read_exact(&mut d).is_err() {
                    break;
                }
                if len & 1 == 1 && f.seek(SeekFrom::Current(1)).is_err() {
                    break;
                }
                if &id == b"fmt " && d.len() >= 16 {
                    // WAVE_FORMAT_EXTENSIBLE names the format in its sub-format's first two bytes.
                    let tag = if word(&d, 0) == 0xFFFE && d.len() >= 26 { word(&d, 24) } else { word(&d, 0) };
                    let (channels, depth) = (word(&d, 2), u32::from(word(&d, 14)));
                    bits = depth | if tag == 3 { 0x100 } else { 0 };
                    src.int16_mono = tag == 1 && channels == 1 && depth == 16;
                } else if &id == b"smpl" && d.len() >= 36 + 24 && long(&d, 28) >= 1 {
                    // 36 bytes of header, then 24 bytes per loop: id, type, start, end, fraction, play count.
                    src.file_loop = Some((long(&d, 36 + 8), long(&d, 36 + 12)));
                }
            } else if f.seek(SeekFrom::Current((len + (len & 1)) as i64)).is_err() {
                break;
            }
        }
        return (bits, src);
    }
    if head.len() > 22 && &head[0..4] == b"fLaC" {
        // STREAMINFO: bits per sample − 1 sits in 5 bits across bytes 20 and 21 of the file.
        return (u32::from(((head[20] & 1) << 4) | (head[21] >> 4)) + 1, src);
    }
    if head.len() > 4 && &head[0..4] == b"OggS" {
        return (32 | 0x100, src);
    }
    (16, src)
}

impl Shared {
    /// Loads a sample file and makes it the current one (UI / initialize thread). A decode the host already holds
    /// (it decodes a project's samples ahead, spec: ui/project-loading.md) is taken as it is. A file that plays from
    /// disk with the current settings (Keep on disk) is not decoded.
    pub fn load(&self, path: &str, params: &SamplerParams) -> Result<(), String> {
        let p = Path::new(path);
        let (bits, source) = source_info(p);
        let layout = stream::Layout::of(p);
        let on_disk = layout.as_ref().is_some_and(|l| self.streams(params, &source, l).is_some());
        let s = if on_disk { None } else { Some(os_engine::sample::load_shared(p).map_err(|e| e.to_string())?) };
        self.bits.store(bits, Ordering::Release);
        self.set_loaded(s, path, source, layout, params);
        Ok(())
    }

    /// Replaces the loaded sample (`path` is what the state remembers), with the file's loop and format in `source`
    /// and, for a file that can play from disk, its layout.
    fn set_loaded(&self, s: Option<Arc<Sample>>, path: &str, source: fx::Source, layout: Option<stream::Layout>, params: &SamplerParams) {
        {
            // All of it under the lock of `raw`, with the version: a decode made for an earlier load is never kept.
            let mut raw = self.raw.lock().unwrap();
            *raw = s;
            *self.source.lock().unwrap() = source;
            *self.layout.lock().unwrap() = layout;
            *self.loaded_path.lock().unwrap() = path.to_string();
            self.raw_version.fetch_add(1, Ordering::AcqRel);
        }
        if let Ok(mut p) = params.sample_path.write() {
            *p = path.to_string();
        }
        self.precompute(params);
    }

    pub fn clear(&self, params: &SamplerParams) {
        self.set_loaded(None, "", fx::Source::default(), None, params);
    }

    /// **(Lemon Studio)** The played sample plays from disk with these settings (Keep on disk on, no realtime stretch
    /// method, no step that changes the file's frames): its loop.
    fn streams(&self, params: &SamplerParams, source: &fx::Source, layout: &stream::Layout) -> Option<Option<(usize, usize)>> {
        let dk = params.disk_key();
        if !dk.keep || dk.realtime {
            return None;
        }
        fx::as_is(source, &params.fx_key(self.tempo(), self.rate()), layout.rate, layout.frames)
    }

    /// The host's output rate (files below 8000 Hz are converted to it).
    pub fn rate(&self) -> u32 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate.max(1), Ordering::Relaxed);
    }

    pub fn tempo(&self) -> f64 {
        f64::from_bits(self.tempo.load(Ordering::Relaxed))
    }

    pub fn set_tempo(&self, t: f64) {
        if t > 0.0 && (t - self.tempo()).abs() > 1e-9 {
            self.tempo.store(t.to_bits(), Ordering::Relaxed);
        }
    }

    /// Rebuilds the processed sample from the loaded one and the current settings. Calls that arrive while one runs
    /// only mark it to run again, so a knob drag never queues a backlog.
    pub fn precompute(&self, params: &SamplerParams) {
        if self.busy.swap(true, Ordering::AcqRel) {
            self.again.store(true, Ordering::Release);
            return;
        }
        loop {
            self.again.store(false, Ordering::Release);
            self.update(params);
            if !self.again.load(Ordering::Acquire) {
                break;
            }
        }
        self.busy.store(false, Ordering::Release);
        if self.again.swap(false, Ordering::AcqRel) {
            self.precompute(params);
        }
    }

    /// One pass of `precompute`. **(Lemon Studio)** Keep on disk: a played sample that is the file's frames as they
    /// are, from a file that can stream, plays from disk: the frames around the points voices start at are read, the
    /// decoded file is let go. Otherwise the file is decoded (when it is not already) and built as always (spec:
    /// plugins/lemon-sampler.md "Keep on disk (Lemon Studio)").
    fn update(&self, params: &SamplerParams) {
        let version = self.raw_version.load(Ordering::Acquire);
        let key = params.fx_key(self.tempo(), self.rate());
        let dk = params.disk_key();
        // What plays now, when it was made for this load.
        let now = self.processed.lock().unwrap().as_ref().filter(|p| p.version == version).map(|p| (p.key, p.disk_key, p.disk.clone(), p.looped));
        // Notes started where nothing of the file on disk is held: those points are held from now on.
        let missed = now.as_ref().and_then(|n| n.2.as_ref()).is_some_and(|d| !d.missed().is_empty());
        if now.as_ref().is_some_and(|n| n.0 == key && n.1 == dk) && !missed {
            return;
        }
        let (layout, source, path) = (*self.layout.lock().unwrap(), *self.source.lock().unwrap(), self.loaded_path());
        let stream = layout.and_then(|l| self.streams(params, &source, &l).map(|lp| (l, lp)));
        // Only the settings it was made for change: what plays stays.
        let restamp = || {
            if let Some(p) = self.processed.lock().unwrap().as_mut().filter(|p| p.version == version) {
                (p.key, p.disk_key) = (key, dk);
            }
        };
        if let Some((l, lp)) = stream {
            let start = (dk.start_offset() * l.frames as f64).floor() as usize;
            let mut points = vec![0, start];
            if let (true, Some((a, b))) = (dk.loop_on, lp) {
                if b > a {
                    points.push(a);
                }
            }
            let points = stream::points(&points, l.frames);
            let old = now.as_ref().and_then(|n| n.2.clone());
            if old.as_ref().is_some_and(|d| d.named() == points.as_slice() && !missed) && now.as_ref().is_some_and(|n| n.3 == lp) {
                restamp();
                return;
            }
            let disk = match &old {
                Some(d) => Some(d.disk().clone()),
                None => stream::Disk::open(Path::new(&path), l).map_err(|e| nih_log!("Lemon Sampler: {e}")).ok(),
            };
            if let Some(disk) = disk {
                let played = stream::Played::new(disk, &points, old.as_deref());
                params.frames.store(l.frames as u32, Ordering::Relaxed);
                self.store(Processed { version, key, disk_key: dk, serial: 0, sample: None, disk: Some(Arc::new(played)), looped: lp, file_frames: l.frames as f64 });
                // The file plays from disk: its decoded frames are let go.
                let mut raw = self.raw.lock().unwrap();
                if self.raw_version.load(Ordering::Acquire) == version {
                    *raw = None;
                }
                return;
            }
        }
        // From memory. Only Keep on disk's settings changed, and it played from memory already: nothing to build.
        if now.as_ref().is_some_and(|n| n.0 == key && n.2.is_none()) {
            restamp();
            return;
        }
        let raw = self.decoded(version, &path);
        let built = raw.map(|r| (fx::build(&r, &source, &key), r.frames()));
        params.frames.store(built.as_ref().map_or(0, |(b, _)| b.sample.frames() as u32), Ordering::Relaxed);
        let (sample, looped, file_frames) = match built {
            Some((b, raw_frames)) => (Some(Arc::new(b.sample)), b.looped, raw_frames as f64 * b.per_file_frame),
            None => (None, None, 0.0),
        };
        self.store(Processed { version, key, disk_key: dk, serial: 0, sample, disk: None, looped, file_frames });
    }

    /// Makes `p` the processed sample, with a serial of its own.
    fn store(&self, mut p: Processed) {
        p.serial = self.serial.fetch_add(1, Ordering::AcqRel) + 1;
        *self.processed.lock().unwrap() = Some(p);
        self.processed_version.fetch_add(1, Ordering::AcqRel);
    }

    /// The loaded file decoded: the copy held, else decoded now (it played from disk until now) and held for load
    /// `version` while that is still the current one.
    fn decoded(&self, version: u64, path: &str) -> Option<Arc<Sample>> {
        if let Some(r) = self.raw.lock().unwrap().clone() {
            return Some(r);
        }
        if path.is_empty() {
            return None;
        }
        let s = match os_engine::sample::load_shared(Path::new(path)) {
            Ok(s) => s,
            Err(e) => {
                nih_log!("Lemon Sampler: {e}");
                return None;
            }
        };
        let mut raw = self.raw.lock().unwrap();
        if self.raw_version.load(Ordering::Acquire) == version {
            *raw = Some(s.clone());
        }
        Some(s)
    }

    /// Whether a sample is loaded (in memory or on disk).
    pub fn has_sample(&self) -> bool {
        self.raw.lock().is_ok_and(|r| r.is_some()) || self.on_disk().is_some()
    }

    /// The loaded file's length in frames and its rate.
    pub fn length(&self) -> Option<(usize, u32)> {
        if let Some(r) = self.raw.lock().ok()?.as_ref() {
            return Some((r.frames(), r.rate));
        }
        self.on_disk().map(|d| (d.frames(), d.rate()))
    }

    /// The loaded file's length in seconds.
    pub fn secs(&self) -> Option<f64> {
        self.length().map(|(n, rate)| n as f64 / f64::from(rate.max(1)))
    }

    /// The played sample, when it plays from disk.
    pub fn on_disk(&self) -> Option<Arc<stream::Played>> {
        self.processed.lock().ok()?.as_ref()?.disk.clone()
    }

    /// The played sample whole in memory (Save as, Copy): one that plays from disk is the file's frames as they are,
    /// decoded for the occasion.
    pub fn whole(&self) -> Option<Arc<Sample>> {
        let disk = {
            let p = self.processed.lock().ok()?;
            let p = p.as_ref()?;
            if let Some(s) = &p.sample {
                return Some(s.clone());
            }
            p.disk.is_some()
        };
        if !disk {
            return None;
        }
        os_engine::sample::load_shared(Path::new(&self.loaded_path())).ok()
    }

    /// The editor's summary of the sample playing from disk: made on a thread of its own the first time it is asked
    /// for (`None` until it is ready; `version` then changes, so the editor paints again).
    pub fn summary(self: &Arc<Self>) -> Option<Arc<stream::Summary>> {
        let played = self.on_disk()?;
        let version = self.raw_version.load(Ordering::Acquire);
        let mut slot = self.summary.lock().ok()?;
        if slot.version == version && slot.asked {
            return slot.summary.clone();
        }
        *slot = SummarySlot { version, asked: true, summary: None };
        let (me, path, layout) = (self.clone(), self.loaded_path(), *played.disk().layout());
        drop(played);
        std::thread::spawn(move || {
            let made = stream::Summary::read(Path::new(&path), &layout).map(Arc::new);
            if let Ok(mut slot) = me.summary.lock() {
                if slot.version == version {
                    slot.summary = made;
                }
            }
            me.processed_version.fetch_add(1, Ordering::AcqRel);
        });
        None
    }

    /// Runs `precompute` on a thread of its own (editor edits while the host isn't processing).
    pub fn precompute_async(self: &Arc<Self>, params: &Arc<SamplerParams>) {
        if self.busy.load(Ordering::Acquire) {
            self.again.store(true, Ordering::Release);
            return;
        }
        let (s, p) = (self.clone(), params.clone());
        std::thread::spawn(move || s.precompute(&p));
    }

    /// True when the processed sample is out of date for the current settings.
    pub fn stale(&self, params: &SamplerParams) -> bool {
        let version = self.raw_version.load(Ordering::Acquire);
        let (key, dk) = (params.fx_key(self.tempo(), self.rate()), params.disk_key());
        !self.processed.lock().unwrap().as_ref().is_some_and(|p| p.version == version && p.key == key && p.disk_key == dk)
    }

    /// Changes whenever the shown sample changes (editor repaint check).
    pub fn version(&self) -> u64 {
        self.processed_version.load(Ordering::Acquire) ^ (self.raw_version.load(Ordering::Acquire) << 32)
    }

    pub fn loaded_path(&self) -> String {
        self.loaded_path.lock().unwrap().clone()
    }

    /// Sounding keys.
    pub fn sounding_keys(&self) -> Vec<i32> {
        let mut out = Vec::new();
        for (w, a) in self.sounding.iter().enumerate() {
            let bits = a.load(Ordering::Relaxed);
            for b in 0..64 {
                if bits & (1 << b) != 0 {
                    out.push((w * 64 + b) as i32);
                }
            }
        }
        out
    }
}

pub enum Task {
    Precompute,
}

pub struct LemonSampler {
    params: Arc<SamplerParams>,
    shared: Arc<Shared>,
    engine: dsp::Engine,
    /// (sample version, settings) last asked for; the serial of the processed sample playing.
    requested: Option<(u64, FxKey, DiskKey)>,
    current: Option<u64>,
    /// Pitch bend in cents (main pitch).
    bend: f64,
    /// The host renders offline (an export): voices wait for frames from disk.
    offline: bool,
}

impl Default for LemonSampler {
    fn default() -> Self {
        LemonSampler {
            params: Arc::new(SamplerParams::default()),
            shared: Arc::new(Shared::default()),
            engine: dsp::Engine::new(44100.0),
            requested: None,
            current: None,
            bend: 0.0,
            offline: false,
        }
    }
}

/// An envelope target's LFO (spec §5.2): times from the envelope time law, speed `0.1·2^(8v)` Hz or tempo-synced.
fn lfo(e: &params::EnvParams, tempo: f64) -> dsp::Lfo {
    let n = |v: f32| envelope::knob_seconds(params::time_field(v));
    let sp = f64::from(e.lfo_speed.live());
    let hz = if e.lfo_tempo.live() {
        16.0 * tempo / (60.0 * 2f64.powf(8.0 * (1.0 - sp)))
    } else {
        0.1 * 2f64.powf(8.0 * sp)
    };
    dsp::Lfo {
        shape: e.lfo_shape.live().clamp(0, 2) as u8,
        delay: n(e.lfo_delay.live()),
        attack: n(e.lfo_att.live()),
        amount: f64::from(e.lfo_amt.live()),
        hz,
        global: e.lfo_global.live(),
    }
}

/// The per-block settings of the voices from the parameters.
pub fn block(p: &SamplerParams, tempo: f64, bend: f64) -> dsp::Block {
    let mut targets = [dsp::Target::default(); 5];
    for (i, t) in targets.iter_mut().enumerate() {
        let e = p.env(i);
        // The Panning target has no envelope (spec §3).
        let mut env = if i == dsp::PAN { None } else { e.envelope(128.0) };
        if e.tempo.live() {
            // Tempo-based times: the time law at 120 BPM, scaled by the host tempo (Lemon Studio choice).
            env = env.map(|x| x.scaled(120.0 / tempo.max(1.0)));
        }
        *t = dsp::Target { env, amount: f64::from(e.amt.live()), lfo: lfo(e, tempo) };
    }
    // Resample and the realtime methods stretch in the voices, the other methods in the processed sample; PITCH plays
    // no part in Resample (spec §5.6).
    let mode = p.ts_mode.live();
    let (resample, realtime) = (mode == 0, params::stretch_realtime(mode));
    dsp::Block {
        root: p.root.live(),
        cents: f64::from(p.fine.live()) + if p.main_pitch.live() { bend } else { 0.0 },
        ts_cents: if realtime { params::ts_cents(p.ts_pitch.live()) } else { 0.0 },
        ts_ratio: if resample || realtime { params::mul_of(p.ts_mul.live()) } else { 1.0 },
        ts_time: if resample || realtime { p.ts_seconds(tempo) } else { None },
        realtime,
        targets,
        f_type: p.f_type.live().clamp(0, 7) as u8,
        f_cut: p.f_cut.live(),
        f_res: p.f_res.live(),
        start_offset: f64::from(p.start_offset.live()),
        // The played sample's loop (the file's, or the loop knobs' when they are off their defaults; ping-pong is
        // unfolded into the sample) plays while "Use loop points" is on (spec: audio/sampler.md "The sample file").
        loop_on: p.loop_use.live(),
        declick: p.declick.live(),
        zone: (p.zone_lo.live(), p.zone_hi.live()),
        add_key: p.add_key.live(),
    }
}

/// The voices play a processed sample from now on.
fn take(engine: &mut dsp::Engine, p: &Processed) {
    engine.sample = p.sample.clone();
    engine.set_disk(p.disk.clone());
    engine.looped = p.looped;
    engine.file_frames = p.file_frames;
}

impl LemonSampler {
    /// Takes a newer processed sample when the background task has one ready (never blocks).
    fn pick_up_sample(&mut self, ctx: &mut impl ProcessContext<Self>) {
        let p = &self.params;
        let want = (self.shared.raw_version.load(Ordering::Acquire), p.fx_key(self.shared.tempo(), self.shared.rate()), p.disk_key());
        if self.requested != Some(want) {
            self.requested = Some(want);
            ctx.execute_background(Task::Precompute);
        }
        // A note started where nothing of the file on disk is held: a played sample that holds it is made.
        if self.engine.disk().is_some_and(|d| d.news()) {
            ctx.execute_background(Task::Precompute);
        }
        if let Ok(p) = self.shared.processed.try_lock() {
            if let Some(p) = p.as_ref() {
                if self.current != Some(p.serial) {
                    self.current = Some(p.serial);
                    take(&mut self.engine, p);
                }
            }
        }
    }

    /// One note event (between the block's renders): `blk` follows a pitch bend.
    fn event(&mut self, ev: NoteEvent<()>, blk: &mut dsp::Block, tempo: f64) {
        match ev {
            NoteEvent::NoteOn { note, velocity, voice_id, .. } => self.engine.note_on_id(note, velocity, voice_id, blk),
            NoteEvent::NoteOff { note, .. } => self.engine.note_off(note, blk),
            NoteEvent::Choke { note, voice_id, .. } => self.engine.choke(voice_id, note, blk),
            // An audio clip's fades and gain, on its own voice.
            NoteEvent::PolyVolume { voice_id, note, gain, .. } => self.engine.volume(voice_id, note, gain),
            NoteEvent::PolyTuning { note, tuning, .. } => self.engine.tune(note, f64::from(tuning)),
            // The note's own pan, also while it sounds (spec: audio/voice-levels.md "Final levels").
            NoteEvent::PolyPan { voice_id, note, pan, .. } => self.engine.pan(voice_id, note, pan),
            NoteEvent::PolyModulation { voice_id, poly_modulation_id, normalized_offset, .. } => match poly_modulation_id {
                params::START_OFFSET_POLY => self.engine.start_later(voice_id, normalized_offset, blk),
                // The note's mod X / mod Y in knob units (the filter knobs' 0..1), added to the knobs for its voice.
                params::MOD_X_POLY => self.engine.modulate(voice_id, Some(normalized_offset), None),
                params::MOD_Y_POLY => self.engine.modulate(voice_id, None, Some(normalized_offset)),
                _ => {}
            },
            NoteEvent::MidiPitchBend { value, .. } => {
                // ±2 semitones (spec §5.1).
                self.bend = f64::from(value * 2.0 - 1.0) * 200.0;
                *blk = block(&self.params, tempo, self.bend);
            }
            NoteEvent::MidiCC { cc: 120 | 123, .. } => self.engine.all_off(),
            _ => {}
        }
    }

    fn publish_sounding(&self) {
        let mut bits = [0u64; 3];
        for k in self.engine.sounding() {
            let k = usize::from(k).min(191);
            bits[k / 64] |= 1 << (k % 64);
        }
        for (a, b) in self.shared.sounding.iter().zip(bits) {
            a.store(b, Ordering::Relaxed);
        }
    }
}

impl Plugin for LemonSampler {
    const NAME: &'static str = "Lemon Sampler";
    const VENDOR: &'static str = "Lemon Studio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = Task;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let (params, shared) = (self.params.clone(), self.shared.clone());
        Box::new(move |task| match task {
            Task::Precompute => shared.precompute(&params),
        })
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(Box::new(editor::SamplerEditor::new(self.params.clone(), self.shared.clone())))
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
        self.engine.set_rate(config.sample_rate);
        self.shared.set_rate(config.sample_rate.round() as u32);
        // An offline render (an export) gets the resampler's exact weights; live playback the tables.
        self.offline = config.process_mode == ProcessMode::Offline;
        self.engine.quality = if self.offline { os_dsp::resample::Quality::Exact } else { os_dsp::resample::Quality::Table };
        // A restored state names a sample: load it now (initialize may block).
        let path = self.params.sample_path.read().map(|p| p.clone()).unwrap_or_default();
        if !path.is_empty() && path != self.shared.loaded_path() {
            if let Err(e) = self.shared.load(&path, &self.params) {
                nih_log!("Lemon Sampler: {e}");
            }
        } else if path.is_empty() && !self.shared.loaded_path().is_empty() {
            // A restored state without a sample (the host undid a load into an empty channel): nothing plays.
            self.shared.clear(&self.params);
        } else {
            self.shared.precompute(&self.params);
        }
        if let Some(p) = self.shared.processed.lock().unwrap().as_ref() {
            self.current = Some(p.serial);
            self.requested = Some((p.version, p.key, p.disk_key));
            take(&mut self.engine, p);
        }
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.publish_sounding();
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        if let Some(t) = ctx.transport().tempo {
            self.shared.set_tempo(t);
        }
        self.pick_up_sample(ctx);
        // An offline render or an export waits for frames from disk (spec: plugins/lemon-sampler.md "Keep on disk
        // (Lemon Studio)").
        self.engine.wait = self.offline || stream::exporting();
        let tempo = self.shared.tempo();
        let mut blk = block(&self.params, tempo, self.bend);
        // Notes the editor plays itself.
        if let Ok(mut q) = self.shared.previews.try_lock() {
            for (key, vel, on) in q.drain(..) {
                if on {
                    self.engine.note_on(key, vel, &blk);
                } else {
                    self.engine.note_off(key, &blk);
                }
            }
        }
        let len = buffer.samples();
        let out = buffer.as_slice();
        for ch in out.iter_mut() {
            ch.fill(0.0);
        }
        let mut at = 0;
        while let Some(ev) = ctx.next_event() {
            let t = (ev.timing() as usize).min(len);
            if t > at {
                let (left, right) = out.split_at_mut(1);
                self.engine.render(&mut left[0][at..t], &mut right[0][at..t], &blk);
                at = t;
            }
            self.event(ev, &mut blk, tempo);
        }
        if at < len {
            let (left, right) = out.split_at_mut(1);
            self.engine.render(&mut left[0][at..len], &mut right[0][at..len], &blk);
        }
        // Voice ends go back to the host, at the end of the block (spec: audio/voices.md §4).
        let last = (len as u32).saturating_sub(1);
        for (note, voice_id) in self.engine.take_ended() {
            ctx.send_event(NoteEvent::VoiceTerminated { timing: last, voice_id, channel: 0, note });
        }
        self.publish_sounding();
        // A long sample may be quiet for a while: tell the host to keep processing until the voices end.
        if self.engine.has_voices() { ProcessStatus::KeepAlive } else { ProcessStatus::Normal }
    }
}

// CLAP id and VST3 class id keep the spelling of the plugin's earlier "Open" name: hosts find saved sessions'
// plugins by them (spec: plugins/plugin-formats.md, identity).
impl ClapPlugin for LemonSampler {
    const CLAP_ID: &'static str = "org.openstudio.open-sampler";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Lemon Studio's sampler instrument");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Sampler, ClapFeature::Stereo];
}

impl Vst3Plugin for LemonSampler {
    const VST3_CLASS_ID: [u8; 16] = *b"OpenStudioSamplr";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Sampler];
}

// Built as a plugin module: the C entry points hosts look for. Linked into Lemon Studio (`linked` feature): only the
// symbol-free table, so several Lemon plugins fit in one program (spec: plugins/plugin-formats.md).
#[cfg(not(feature = "linked"))]
nih_export_clap!(LemonSampler);
#[cfg(not(feature = "linked"))]
nih_export_vst3!(LemonSampler);
os_plugkit::linked_clap_entry!(LemonSampler);

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin at 48 kHz playing a sine of `hz` (half scale), with its parameters' defaults.
    fn plugin(hz: f64) -> LemonSampler {
        let mut p = LemonSampler::default();
        p.engine.set_rate(48000.0);
        let x: Vec<f32> = (0..48000).map(|i| (2.0 * std::f64::consts::PI * hz * i as f64 / 48000.0).sin() as f32 * 0.5).collect();
        p.engine.sample = Some(Arc::new(Sample { rate: 48000, left: x.clone(), right: x }));
        p
    }

    fn note_on(id: i32) -> NoteEvent<()> {
        NoteEvent::NoteOn { timing: 0, voice_id: Some(id), channel: 0, note: 60, velocity: 100.0 / 128.0 }
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Plays note `id` with `events` sent after its note on; the level of each side over the last half of 9600 frames.
    fn level(hz: f64, events: &[NoteEvent<()>]) -> (f32, f32) {
        let mut p = plugin(hz);
        let mut blk = block(&p.params, 130.0, 0.0);
        p.event(note_on(3), &mut blk, 130.0);
        for e in events {
            p.event(*e, &mut blk, 130.0);
        }
        let (mut l, mut r) = (vec![0.0; 9600], vec![0.0; 9600]);
        p.engine.render(&mut l, &mut r, &blk);
        (rms(&l[4800..]), rms(&r[4800..]))
    }

    // spec: audio/voice-levels.md "Final levels": the note's pan reaches its voice; with the channel's pan centred the
    // host's split times the voice's makes the note's own split.
    #[test]
    fn a_note_pan_reaches_its_voice() {
        let (l0, r0) = level(1000.0, &[]);
        assert!((l0 - 0.5 / 2f32.sqrt()).abs() < 1e-3 && l0 == r0, "{l0} {r0}");
        let pan = |voice_id, pan| NoteEvent::PolyPan { timing: 0, voice_id, channel: 0, note: 60, pan };
        let (l, r) = level(1000.0, &[pan(Some(3), -1.0)]);
        assert!((l - l0 * 2f32.sqrt()).abs() < 1e-4 && r < 1e-6, "{l} {r}");
        let (l, r) = level(1000.0, &[pan(Some(3), 0.5)]);
        let a = 1.5 * std::f32::consts::PI / 4.0;
        assert!((l * std::f32::consts::FRAC_1_SQRT_2 - l0 * a.cos()).abs() < 1e-4, "{l}");
        assert!((r * std::f32::consts::FRAC_1_SQRT_2 - r0 * a.sin()).abs() < 1e-4, "{r}");
        // Another note's pan leaves it alone; a pan by key reaches it.
        assert_eq!(level(1000.0, &[pan(Some(4), -1.0)]), (l0, r0));
        assert!(level(1000.0, &[pan(None, 1.0)]).0 < 1e-6);
    }

    // spec: audio/voice-levels.md "Final levels", audio/channel-filter.md: the note's mod X / mod Y add to the filter
    // knobs for its voice.
    #[test]
    fn a_note_filter_offset_reaches_its_voice() {
        let modulate = |voice_id, poly_modulation_id, normalized_offset| NoteEvent::PolyModulation {
            timing: 0,
            voice_id,
            poly_modulation_id,
            normalized_offset,
        };
        let open = level(1000.0, &[]).0;
        let dull = level(1000.0, &[modulate(3, params::MOD_X_POLY, -0.8)]).0;
        assert!(dull < open * 0.1, "{open} {dull}");
        assert_eq!(level(1000.0, &[modulate(4, params::MOD_X_POLY, -0.8)]).0, open);
        // Resonance on a closed-down fast low-pass changes the sound.
        let half = level(1000.0, &[modulate(3, params::MOD_X_POLY, -0.5)]).0;
        let res = level(1000.0, &[modulate(3, params::MOD_X_POLY, -0.5), modulate(3, params::MOD_Y_POLY, 0.9)]).0;
        assert!((res - half).abs() > 1e-3, "{half} {res}");
    }

    // A sounding voice follows a new pan and a new offset.
    #[test]
    fn a_sounding_voice_follows_new_note_levels() {
        let mut p = plugin(1000.0);
        let mut blk = block(&p.params, 130.0, 0.0);
        p.event(note_on(3), &mut blk, 130.0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        p.engine.render(&mut l, &mut r, &blk);
        assert!(rms(&r) > 0.3);
        p.event(NoteEvent::PolyPan { timing: 0, voice_id: Some(3), channel: 0, note: 60, pan: 1.0 }, &mut blk, 130.0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        p.engine.render(&mut l, &mut r, &blk);
        // The pan ramps over one control span.
        assert!(rms(&l[64..]) < 1e-6 && rms(&r[64..]) > 0.45, "{} {}", rms(&l[64..]), rms(&r[64..]));
        p.event(NoteEvent::PolyModulation { timing: 0, voice_id: 3, poly_modulation_id: params::MOD_X_POLY, normalized_offset: -0.9 }, &mut blk, 130.0);
        let (mut l, mut r) = (vec![0.0; 4800], vec![0.0; 4800]);
        p.engine.render(&mut l, &mut r, &blk);
        assert!(rms(&r[2400..]) < 0.05, "{}", rms(&r[2400..]));
    }

    /// A WAV file: `channels`, 16-bit, `frames` of silence, then an `smpl` chunk with one loop when given.
    fn wav(channels: u16, frames: u32, lp: Option<(u32, u32)>) -> Vec<u8> {
        let data = frames * u32::from(channels) * 2;
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF\0\0\0\0WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        for v in [1u16, channels] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        for v in [44100u32, 44100 * 2 * u32::from(channels)] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        for v in [2 * channels, 16] {
            w.extend_from_slice(&v.to_le_bytes());
        }
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        w.resize(w.len() + data as usize, 0);
        if let Some((a, b)) = lp {
            w.extend_from_slice(b"smpl");
            w.extend_from_slice(&60u32.to_le_bytes());
            let mut d = [0u32; 15];
            d[7] = 1;
            (d[11], d[12]) = (a, b);
            for v in d {
                w.extend_from_slice(&v.to_le_bytes());
            }
        }
        let len = (w.len() - 8) as u32;
        w[4..8].copy_from_slice(&len.to_le_bytes());
        w
    }

    // spec: audio/sampler.md "The sample file": the loop is the first loop of the `smpl` chunk, wherever it sits.
    #[test]
    fn the_file_loop_and_format_are_read() {
        let dir = std::env::temp_dir().join(format!("lemon-sampler-src-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.wav");
        std::fs::write(&p, wav(1, 100_001, Some((5, 90_000)))).unwrap();
        assert_eq!(source_info(&p), (16, fx::Source { int16_mono: true, file_loop: Some((5, 90_000)) }));
        std::fs::write(&p, wav(2, 1000, None)).unwrap();
        assert_eq!(source_info(&p), (16, fx::Source { int16_mono: false, file_loop: None }));
        assert_eq!(source_bits(&p), 16);
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Keep on disk (spec: plugins/lemon-sampler.md "Keep on disk (Lemon Studio)")

    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    /// A fresh directory for a test's files.
    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lemon-sampler-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A stereo 16-bit WAV of `frames` frames of a varied signal (tones and some noise, the sides different) at 44100
    /// Hz, with an `smpl` loop when given.
    fn music(frames: u32, lp: Option<(u32, u32)>) -> Vec<u8> {
        let mut w = wav(2, frames, lp);
        let mut x: u32 = 0x9e37_79b9;
        for i in 0..frames as usize {
            let t = i as f64 / 44100.0;
            for (c, hz) in [(0, 220.0), (1, 331.0)] {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let tau = 2.0 * std::f64::consts::PI * hz * t;
                let v = 0.4 * tau.sin() + 0.2 * (3.01 * tau).sin() + 0.1 * (f64::from(x >> 16) / 65536.0 - 0.5);
                let at = 44 + (i * 2 + c) * 2;
                w[at..at + 2].copy_from_slice(&((v * 32767.0) as i16).to_le_bytes());
            }
        }
        w
    }

    /// Sets a parameter as the editor does (normalised value).
    fn set(p: &impl Param, norm: f32) {
        params::live_set(params::param_addr(p.as_ptr()), norm, p.unmodulated_normalized_value());
    }

    /// Waits for the reader to fill every ring as far as it can, as a reader that keeps up has between two blocks.
    fn settle(d: &stream::Disk) {
        let t = Instant::now();
        while !d.settled() {
            assert!(t.elapsed() < Duration::from_secs(30), "the reader fills the rings");
            d.wake();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Waits for `f` to hold.
    fn eventually(what: &str, f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[derive(Clone, Copy)]
    enum Ev {
        /// Key, note id.
        On(u8, i32),
        Off(u8),
        /// A note that starts further in (a trimmed audio clip): note id, fraction of the file.
        Later(i32, f32),
    }

    const BLOCK: usize = 512;

    /// The sampler's sound side at 48 kHz: its parameters, the loaded sample and the voices.
    struct Rig {
        params: Arc<SamplerParams>,
        shared: Arc<Shared>,
        engine: dsp::Engine,
    }

    impl Rig {
        /// Loads `path` with Keep on disk `keep` and the parameters `setup` sets.
        fn new(path: &Path, keep: bool, setup: impl Fn(&SamplerParams)) -> Rig {
            let params = Arc::new(SamplerParams::default());
            set(&params.keep_disk, if keep { 1.0 } else { 0.0 });
            setup(&params);
            let shared = Arc::new(Shared::default());
            shared.set_rate(48000);
            shared.load(path.to_str().unwrap(), &params).unwrap();
            let mut engine = dsp::Engine::new(48000.0);
            take(&mut engine, shared.processed.lock().unwrap().as_ref().unwrap());
            Rig { params, shared, engine }
        }

        /// Settings changed: the processed sample is made again and played from now on.
        fn update(&mut self) {
            self.shared.precompute(&self.params);
            take(&mut self.engine, self.shared.processed.lock().unwrap().as_ref().unwrap());
        }

        /// Plays `events` (at the start of the block they name) for `blocks` blocks. Live (the voices don't wait), the
        /// reader has the time between blocks to fill the rings.
        fn play(&mut self, events: &[(usize, Ev)], blocks: usize) -> (Vec<f32>, Vec<f32>) {
            let blk = block(&self.params, 130.0, 0.0);
            let (mut l, mut r) = (vec![0.0; blocks * BLOCK], vec![0.0; blocks * BLOCK]);
            for b in 0..blocks {
                for &(_, e) in events.iter().filter(|(at, _)| *at == b) {
                    match e {
                        Ev::On(key, id) => self.engine.note_on_id(key, 0.8, Some(id), &blk),
                        Ev::Off(key) => self.engine.note_off(key, &blk),
                        Ev::Later(id, x) => self.engine.start_later(id, x, &blk),
                    }
                }
                let s = b * BLOCK..(b + 1) * BLOCK;
                self.engine.render(&mut l[s.clone()], &mut r[s], &blk);
                if let (Some(d), false) = (self.engine.disk(), self.engine.wait) {
                    settle(d.disk());
                }
            }
            (l, r)
        }
    }

    /// Both renders are the same to the bit, and not silence.
    fn same(what: &str, a: &(Vec<f32>, Vec<f32>), b: &(Vec<f32>, Vec<f32>)) {
        assert_eq!(a.0.len(), b.0.len());
        let differs = |i: usize| a.0[i].to_bits() != b.0[i].to_bits() || a.1[i].to_bits() != b.1[i].to_bits();
        if let Some(i) = (0..a.0.len()).find(|&i| differs(i)) {
            panic!("{what}: frame {i} differs: {:?} / {:?}", (a.0[i], a.1[i]), (b.0[i], b.1[i]));
        }
        assert!(rms(&a.0) > 0.05 && rms(&a.1) > 0.05, "{what}: plays");
    }

    // Played from disk with the reader keeping up, the voices give what they give from memory, to the bit: several
    // notes at several pitches, START OFFSET, the file's loop (voices wrapping round it again and again).
    #[test]
    fn a_sample_on_disk_plays_as_from_memory() {
        let dir = test_dir("live");
        let p = dir.join("a.wav");
        std::fs::write(&p, music(300_000, Some((150_000, 250_000)))).unwrap();
        let notes = [(0, Ev::On(60, 1)), (3, Ev::On(72, 2)), (10, Ev::On(84, 3)), (12, Ev::On(91, 4)), (40, Ev::Off(72)), (60, Ev::On(48, 5))];
        for (start, loop_on) in [(0.0, false), (0.3, true), (0.0, true)] {
            let setup = |p: &SamplerParams| {
                set(&p.start_offset, start);
                set(&p.loop_use, if loop_on { 1.0 } else { 0.0 });
            };
            let mut ram = Rig::new(&p, false, setup);
            let mut disk = Rig::new(&p, true, setup);
            assert!(ram.shared.on_disk().is_none());
            let played = disk.shared.on_disk().expect("plays from disk");
            assert!(disk.shared.raw.lock().unwrap().is_none(), "the decoded file is let go");
            let points = [0, (start * 300_000.0) as usize, if loop_on { 150_000 } else { 0 }];
            assert_eq!(played.points(), stream::points(&points, 300_000).as_slice());
            let what = format!("start {start}, loop {loop_on}");
            same(&what, &ram.play(&notes, 250), &disk.play(&notes, 250));
            // The voices read from their rings, beyond the frames held.
            assert!(played.disk().frames_read() > 300_000, "{what}: {}", played.disk().frames_read());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    // A step that needs the whole file (reverse) or a realtime stretch method keeps the played sample in memory, with
    // Keep on disk on or off: the same sound.
    #[test]
    fn a_reversed_or_stretched_sample_stays_in_memory() {
        let dir = test_dir("ram");
        let p = dir.join("a.wav");
        std::fs::write(&p, music(100_000, None)).unwrap();
        let notes = [(0, Ev::On(60, 1)), (5, Ev::On(67, 2))];
        let reverse = |p: &SamplerParams| set(&p.fx_reverse, 1.0);
        let stretch = |p: &SamplerParams| set(&p.ts_mode, p.ts_mode.preview_normalized(1));
        for (what, setup) in [("reverse", &reverse as &dyn Fn(&SamplerParams)), ("stretch", &stretch)] {
            let mut ram = Rig::new(&p, false, setup);
            let mut disk = Rig::new(&p, true, setup);
            assert!(disk.shared.on_disk().is_none(), "{what}");
            assert!(disk.engine.disk().is_none() && disk.engine.sample.is_some(), "{what}");
            same(what, &ram.play(&notes, 100), &disk.play(&notes, 100));
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    // An offline render (voices wait for their frames) is the render from memory, to the bit, with the reader given no
    // time at all and a note starting where nothing is held (a trimmed audio clip).
    #[test]
    fn an_offline_render_from_disk_is_the_one_from_memory() {
        let dir = test_dir("offline");
        let p = dir.join("a.wav");
        std::fs::write(&p, music(200_000, Some((20_000, 180_000)))).unwrap();
        let notes = [(0, Ev::On(60, 1)), (2, Ev::On(79, 2)), (5, Ev::On(64, 3)), (5, Ev::Later(3, 0.55)), (30, Ev::Off(79))];
        let setup = |p: &SamplerParams| set(&p.start_offset, 0.1);
        let mut ram = Rig::new(&p, false, setup);
        let mut disk = Rig::new(&p, true, setup);
        for r in [&mut ram, &mut disk] {
            r.engine.quality = os_dsp::resample::Quality::Exact;
            r.engine.wait = true;
        }
        assert!(disk.engine.disk().is_some());
        same("offline", &ram.play(&notes, 200), &disk.play(&notes, 200));
        std::fs::remove_dir_all(&dir).ok();
    }

    // A note that starts where nothing is held (a trimmed audio clip) has its frames read at once, and its start is
    // held from the next played sample on: played again, it is the render from memory to the bit.
    #[test]
    fn a_clip_start_is_held_once_played() {
        let dir = test_dir("learn");
        let p = dir.join("a.wav");
        std::fs::write(&p, music(300_000, None)).unwrap();
        let notes = [(0, Ev::On(60, 1)), (0, Ev::Later(1, 0.6)), (2, Ev::On(67, 2)), (2, Ev::Later(2, 0.25)), (3, Ev::On(62, 3))];
        let want = Rig::new(&p, false, |_| {}).play(&notes, 100);
        let mut r = Rig::new(&p, true, |_| {});
        r.play(&notes, 100);
        let played = r.shared.on_disk().unwrap();
        // The first note at frame 0 was held; the clip starts were not.
        assert_eq!(played.missed(), vec![180_000, 75_000]);
        assert!(played.news() && !played.news());
        r.update();
        let played = r.shared.on_disk().unwrap();
        assert_eq!(played.points(), &[0, 75_000, 180_000]);
        assert_eq!(played.named(), &[0]);
        assert!(played.missed().is_empty());
        r.engine = dsp::Engine::new(48000.0);
        take(&mut r.engine, r.shared.processed.lock().unwrap().as_ref().unwrap());
        same("held clip starts", &want, &r.play(&notes, 100));
        assert!(played.missed().is_empty());
        // Settings that change the named points keep the learned ones.
        set(&r.params.start_offset, 0.5);
        r.update();
        assert_eq!(r.shared.on_disk().unwrap().points(), &[0, 75_000, 150_000, 180_000]);
        std::fs::remove_dir_all(&dir).ok();
    }

    // A note at a START OFFSET whose frames are not held yet (the knob just moved) asks for them at once, without
    // learning the point: the new played sample holds it.
    #[test]
    fn a_moved_start_offset_is_read_at_once() {
        let dir = test_dir("offset");
        let p = dir.join("a.wav");
        std::fs::write(&p, music(200_000, None)).unwrap();
        let mut r = Rig::new(&p, true, |_| {});
        let played = r.shared.on_disk().unwrap();
        set(&r.params.start_offset, 0.5);
        let blk = block(&r.params, 130.0, 0.0);
        r.engine.note_on_id(60, 0.8, Some(1), &blk);
        assert_eq!(played.disk().busy(), 1, "the voice's ring fills before it plays");
        r.engine.note_on_id(60, 0.8, Some(2), &blk);
        assert_eq!(played.disk().busy(), 2);
        assert!(played.missed().is_empty() && !played.news());
        r.update();
        assert_eq!(r.shared.on_disk().unwrap().points(), &[0, 100_000]);
        std::fs::remove_dir_all(&dir).ok();
    }

    // A long file played from disk holds only the frames around its start points, and a ring per voice, in memory.
    #[test]
    fn a_long_sample_on_disk_holds_little_in_memory() {
        let dir = test_dir("memory");
        let p = dir.join("a.wav");
        let frames = 2_000_000;
        std::fs::write(&p, music(frames, Some((600_000, 1_900_000)))).unwrap();
        let mut r = Rig::new(&p, true, |p| set(&p.start_offset, 0.5));
        let played = r.shared.on_disk().expect("plays from disk");
        assert!(r.shared.raw.lock().unwrap().is_none());
        assert!(r.shared.processed.lock().unwrap().as_ref().unwrap().sample.is_none());
        assert_eq!(played.points(), &[0, 600_000, 1_000_000]);
        let held = played.held_frames();
        assert!(held <= 3 * (stream::HEAD + stream::BEFORE), "{held}");
        let notes = [(0, Ev::On(84, 1)), (0, Ev::On(86, 2)), (1, Ev::On(88, 3)), (1, Ev::On(90, 4))];
        r.play(&notes, 200);
        // Each voice read hundreds of thousands of frames through one ring.
        assert!(played.disk().frames_read() > 1_000_000, "{}", played.disk().frames_read());
        assert_eq!(played.disk().rings(), 4);
        let bytes = (held + played.disk().rings() * stream::RING) * 2 * std::mem::size_of::<f32>();
        assert!(bytes * 4 < frames as usize * 2 * std::mem::size_of::<f32>(), "{bytes}");
        std::fs::remove_dir_all(&dir).ok();
    }

    // A voice that ends lets go of its ring; the reader stops once its file is no longer played: Keep on disk turned
    // off, the plugin gone.
    #[test]
    fn the_reader_stops_with_its_voices_and_its_sample() {
        let dir = test_dir("reader");
        let p = dir.join("a.wav");
        std::fs::write(&p, music(100_000, None)).unwrap();
        let mut r = Rig::new(&p, true, |_| {});
        let disk = r.shared.on_disk().unwrap().disk().clone();
        r.play(&[(0, Ev::On(60, 1)), (0, Ev::On(84, 2))], 4);
        assert_eq!(disk.busy(), 2);
        // The high note reaches the end of the sample; then the other is let go of.
        r.play(&[], 60);
        assert_eq!(disk.busy(), 1);
        r.engine.all_off();
        r.play(&[], 4);
        assert!(!r.engine.has_voices());
        assert_eq!(disk.busy(), 0);
        let alive = disk.reader_alive();
        drop(disk);
        assert!(alive.load(Ordering::Acquire));
        // Keep on disk off: from memory, and the reader stops.
        set(&r.params.keep_disk, 0.0);
        r.update();
        assert!(r.shared.on_disk().is_none() && r.engine.sample.is_some());
        eventually("the reader stops when Keep on disk goes off", || !alive.load(Ordering::Acquire));
        // On again: from disk again, until the sampler goes.
        set(&r.params.keep_disk, 1.0);
        r.update();
        let alive = r.shared.on_disk().expect("plays from disk again").disk().reader_alive();
        assert!(r.shared.raw.lock().unwrap().is_none());
        r.play(&[(0, Ev::On(60, 1))], 2);
        drop(r);
        eventually("the reader stops with the sampler", || !alive.load(Ordering::Acquire));
        // The plugin itself.
        let mut plugin = LemonSampler::default();
        set(&plugin.params.keep_disk, 1.0);
        plugin.shared.load(p.to_str().unwrap(), &plugin.params).unwrap();
        take(&mut plugin.engine, plugin.shared.processed.lock().unwrap().as_ref().unwrap());
        let alive = plugin.engine.disk().expect("plays from disk").disk().reader_alive();
        drop(plugin);
        eventually("the reader stops with the plugin", || !alive.load(Ordering::Acquire));
        std::fs::remove_dir_all(&dir).ok();
    }
}
