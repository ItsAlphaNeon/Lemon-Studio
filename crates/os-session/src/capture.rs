// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 Samrat Dutta
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio input capture (One-click audio recording): the audio device's inputs into a WAV file.
//!
//! An ASIO driver's inputs come in its own callback, with its outputs (`device::asio`); any other device's inputs come
//! from a system input stream opened while recording (`InputStream`). Either way the callback pushes stereo frames into
//! one lock-free ring (`capture`), only while a recording is on, and a writer thread (`Recorder`) drains it to disk. At
//! the end the writer turns what it wrote into a WAV, leaving out the frames before the take's start (`Recorder::finish`).
// spec: ui/one-click-recording.md

use std::cell::UnsafeCell;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// One producer, one consumer ring of samples: the device's callback pushes, the writer thread pops; neither locks.
pub struct Ring {
    buf: Box<[UnsafeCell<f32>]>,
    /// Samples written and read so far (the slot is the count modulo the length).
    head: AtomicUsize,
    tail: AtomicUsize,
}

// Safety: a slot is written only by the producer before `head` moves past it, and read only by the consumer before
// `tail` moves past it; the counts are published with release / acquire.
unsafe impl Sync for Ring {}

impl Ring {
    pub fn new(len: usize) -> Ring {
        Ring {
            buf: (0..len.max(1)).map(|_| UnsafeCell::new(0.0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    /// Pushes `n` samples, the `k`-th being `at(k)`, as far as there is room; returns how many went in. Producer only.
    pub fn push_with(&self, n: usize, mut at: impl FnMut(usize) -> f32) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let free = self.buf.len() - (head - self.tail.load(Ordering::Acquire));
        let n = n.min(free);
        for k in 0..n {
            // Safety: the slot is free (the consumer has read past it) and only this producer writes.
            unsafe { *self.buf[(head + k) % self.buf.len()].get() = at(k) };
        }
        self.head.store(head + n, Ordering::Release);
        n
    }

    /// Appends every sample waiting to `out`. Consumer only.
    pub fn pop_into(&self, out: &mut Vec<f32>) {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        // Safety: slots between tail and head were written and published by the producer.
        out.extend((tail..head).map(|i| unsafe { *self.buf[i % self.buf.len()].get() }));
        self.tail.store(head, Ordering::Release);
    }

    /// Empties it. Consumer only.
    pub fn clear(&self) {
        self.tail.store(self.head.load(Ordering::Acquire), Ordering::Release);
    }
}

/// The capture the device's callbacks feed: whether a recording is on, which inputs it takes, and what came in.
pub struct Capture {
    ring: Ring,
    on: AtomicBool,
    /// The first input (0-based) and whether its next one makes a stereo pair (`set_source`).
    first: AtomicU32,
    stereo: AtomicBool,
    /// Stereo frames that came in since `start` (the ones the ring had no room for included), and those lost.
    frames: AtomicU64,
    dropped: AtomicU64,
}

/// About 20 s of stereo at 48 kHz: the writer drains it every few milliseconds.
const RING: usize = 1 << 21;

/// The one capture.
pub fn capture() -> &'static Capture {
    static C: OnceLock<Capture> = OnceLock::new();
    C.get_or_init(|| Capture {
        ring: Ring::new(RING),
        on: AtomicBool::new(false),
        first: AtomicU32::new(0),
        stereo: AtomicBool::new(true),
        frames: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
    })
}

impl Capture {
    /// Starts taking inputs `first` (and `first + 1` when `stereo`) from the next block.
    pub fn start(&self, first: u16, stereo: bool) {
        self.on.store(false, Ordering::Release);
        self.ring.clear();
        self.first.store(u32::from(first), Ordering::Relaxed);
        self.stereo.store(stereo, Ordering::Relaxed);
        self.frames.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.on.store(true, Ordering::Release);
    }

    pub fn stop(&self) {
        self.on.store(false, Ordering::Release);
    }

    pub fn on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// The inputs to take: the first, and whether it is a pair.
    pub fn source(&self) -> (usize, bool) {
        (self.first.load(Ordering::Relaxed) as usize, self.stereo.load(Ordering::Relaxed))
    }

    /// Stereo frames that came in since `start`.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Acquire)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// A callback's block of `frames` frames: `at(channel, frame)` is an input's sample. Pushes the chosen inputs as
    /// stereo (one input on both sides). The audio callback calls it; it neither locks nor allocates.
    pub fn push(&self, frames: usize, at: impl Fn(usize, usize) -> f32) {
        if !self.on() {
            return;
        }
        let (first, stereo) = self.source();
        let second = if stereo { first + 1 } else { first };
        let went = self.ring.push_with(2 * frames, |k| at(if k % 2 == 0 { first } else { second }, k / 2));
        self.dropped.fetch_add(((2 * frames - went) / 2) as u64, Ordering::Relaxed);
        self.frames.fetch_add(frames as u64, Ordering::AcqRel);
    }

    fn drain(&self, out: &mut Vec<f32>) {
        self.ring.pop_into(out);
    }
}

/// An input to record from: a device (the ASIO driver, or a system input device) and its first channel, alone or as a
/// pair with the next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub device: String,
    pub asio: bool,
    pub first: u16,
    pub stereo: bool,
}

impl Input {
    /// "In 1 - In 2", "In 3" (spec §2 The button and the menu).
    pub fn label(&self) -> String {
        let n = self.first + 1;
        if self.stereo {
            format!("In {n} - In {}", n + 1)
        } else {
            format!("In {n}")
        }
    }

    /// As kept in `settings.txt` (`rec_input`).
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            if self.asio { "asio" } else { "system" },
            self.first,
            if self.stereo { "stereo" } else { "mono" },
            self.device
        )
    }

    pub fn parse(key: &str) -> Option<Input> {
        let mut it = key.splitn(4, '|');
        let asio = it.next()? == "asio";
        let first = it.next()?.parse().ok()?;
        let stereo = it.next()? == "stereo";
        Some(Input { device: it.next()?.to_string(), asio, first, stereo })
    }

    /// The inputs of a device with `channels` inputs: its pairs, then each channel.
    pub fn of(device: &str, asio: bool, channels: u16) -> Vec<Input> {
        let d = |first, stereo| Input { device: device.to_string(), asio, first, stereo };
        let pairs = (0..channels / 2).map(|p| d(2 * p, true));
        pairs.chain((0..channels).map(|c| d(c, false))).collect()
    }
}

/// The input picked last (`rec_input`).
pub fn last_input(st: &crate::settings_file::SettingsStore) -> Option<Input> {
    st.get("rec_input").as_deref().and_then(Input::parse)
}

pub fn set_last_input(st: &crate::settings_file::SettingsStore, i: &Input) {
    st.write("rec_input", &i.key());
}

/// The devices to record from, with their input count: the ASIO driver when the output is one (its inputs once it has
/// opened; until then two are assumed and checked at the start), else the system's input devices.
pub fn devices() -> Vec<(String, bool, u16)> {
    let cfg = crate::audio_cfg::AudioCfg::current();
    if let Some(name) = cfg.asio_driver() {
        let opened =
            crate::device::last().filter(|d| d.host == crate::device::ASIO && d.name == name).map(|d| d.inputs);
        return vec![(name.to_string(), true, opened.unwrap_or(2))];
    }
    #[cfg(feature = "device")]
    return system_inputs();
    #[cfg(not(feature = "device"))]
    Vec::new()
}

/// The system's input devices and their channels (on Linux the sound server's, or ALSA's in exclusive mode, as the
/// output): the one Settings > Audio records from (else the system's default) first, asked for on its own so that it
/// is there even when the list leaves it out, then the others. A device that reports no format is listed with none.
#[cfg(feature = "device")]
fn system_inputs() -> Vec<(String, bool, u16)> {
    use cpal::traits::HostTrait;
    use crate::device::system_input::input_config;
    let cfg = crate::audio_cfg::AudioCfg::current();
    let channels =
        |d: &cpal::Device| input_config(d).inspect_err(|e| eprintln!("audio input {e}")).map_or(0, |c| c.channels());
    let mut v: Vec<(String, bool, u16)> = crate::audio_cfg::input_device(&cfg)
        .map(|d| (crate::audio_cfg::device_name(&d), false, channels(&d)))
        .into_iter()
        .collect();
    let host = crate::audio_cfg::host(&cfg);
    for d in host.input_devices().into_iter().flatten().filter(crate::audio_cfg::recordable) {
        let name = crate::audio_cfg::device_name(&d);
        if !v.iter().any(|(n, ..)| *n == name) {
            v.push((name, false, channels(&d)));
        }
    }
    v
}

/// A system input device's stream, recording while it is held.
pub struct InputStream {
    /// Taken when dropped: an ALSA device is let go of before the sound server is given it back.
    #[cfg(feature = "device")]
    stream: Option<cpal::Stream>,
    alsa: bool,
    pub rate: u32,
}

impl Drop for InputStream {
    fn drop(&mut self) {
        #[cfg(feature = "device")]
        drop(self.stream.take());
        if self.alsa {
            crate::device::heal_sound_server();
        }
    }
}

/// Audio input is not in this build: opening a device fails.
#[cfg(not(feature = "device"))]
pub fn open_input(name: &str) -> Result<InputStream, String> {
    Err(format!("{name}: audio input is not in this build"))
}

/// Opens system input device `name` in shared mode at its own rate, pushing into `capture`.
#[cfg(feature = "device")]
pub fn open_input(name: &str) -> Result<InputStream, String> {
    use cpal::traits::StreamTrait;
    use cpal::SampleFormat;
    let host = crate::audio_cfg::host(&crate::audio_cfg::AudioCfg::current());
    let device = crate::audio_cfg::find_input(&host, name).ok_or_else(|| format!("{name} is not there"))?;
    let supported = crate::device::system_input::input_config(&device)?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let stream = match format {
        SampleFormat::F32 => build::<f32>(&device, &config),
        SampleFormat::F64 => build::<f64>(&device, &config),
        SampleFormat::I16 => build::<i16>(&device, &config),
        SampleFormat::I24 => build::<cpal::I24>(&device, &config),
        SampleFormat::I32 => build::<i32>(&device, &config),
        SampleFormat::U16 => build::<u16>(&device, &config),
        other => Err(format!("unsupported input sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    let alsa = host.id().name() == "ALSA";
    Ok(InputStream { stream: Some(stream), alsa, rate: config.sample_rate })
}

#[cfg(feature = "device")]
fn build<T>(device: &cpal::Device, config: &cpal::StreamConfig) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    let channels = usize::from(config.channels).max(1);
    device
        .build_input_stream::<T, _, _>(
            config.clone(),
            move |data: &[T], _| {
                // An input past the device's channels is its last one.
                capture()
                    .push(data.len() / channels, |c, f| data[f * channels + c.min(channels - 1)].to_sample::<f32>());
            },
            |e| eprintln!("audio input stream: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

/// The take's file name: "<project>_<YYYY-MM-DD HH-MM-SS>_REC.wav" (spec §2 The take).
pub fn file_name(project: &str, now: chrono::NaiveDateTime) -> String {
    format!("{project}_{}_REC.wav", now.format("%Y-%m-%d %H-%M-%S"))
}

/// The writer: drains the capture into `<file>.part` until finished, then makes the WAV.
pub struct Recorder {
    stop: Arc<AtomicBool>,
    /// The first frame of the take (frames before it are left out), set before `finish`.
    head: Arc<AtomicI64>,
    /// The WAV's rate, set before `finish` when it was not known at the start (an ASIO driver's).
    rate: Arc<AtomicU32>,
    /// Frames written to the raw file and flushed, so a part of it can be read while the take goes on.
    written: Arc<AtomicU64>,
    part: PathBuf,
    /// The raw file is left on disk when the WAV is made (`keep_part`).
    keep: Arc<AtomicBool>,
    /// The WAV's sample format in bits: 16 or 24 (integer), else 32-bit float (`set_bits`).
    bits: Arc<AtomicU32>,
    /// The WAV made at the end (`set_file`).
    file: Arc<std::sync::Mutex<PathBuf>>,
    thread: Option<std::thread::JoinHandle<Result<(PathBuf, u64), String>>>,
}

impl Recorder {
    /// Starts writing what the capture takes in into `file` (a WAV at `rate` once finished).
    pub fn start(file: PathBuf, rate: u32) -> Result<Recorder, String> {
        Recorder::start_from(file, rate, |out| capture().drain(out))
    }

    /// Starts writing what `drain` hands over (stereo frames, appended to its argument) into `file`.
    pub fn start_from(
        file: PathBuf,
        rate: u32,
        mut drain: impl FnMut(&mut Vec<f32>) + Send + 'static,
    ) -> Result<Recorder, String> {
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let part = file.with_extension("wav.part");
        let raw = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let head = Arc::new(AtomicI64::new(0));
        let rate = Arc::new(AtomicU32::new(rate));
        let written = Arc::new(AtomicU64::new(0));
        let keep = Arc::new(AtomicBool::new(false));
        let bits = Arc::new(AtomicU32::new(32));
        let target = Arc::new(std::sync::Mutex::new(file));
        let (s, h, r, wr, p, k, b, f) = (
            stop.clone(),
            head.clone(),
            rate.clone(),
            written.clone(),
            part.clone(),
            keep.clone(),
            bits.clone(),
            target.clone(),
        );
        let thread = std::thread::Builder::new()
            .name("audio recorder".into())
            .spawn(move || {
                let mut w = BufWriter::new(raw);
                let mut buf = Vec::with_capacity(1 << 16);
                let mut samples = 0u64;
                loop {
                    let last = s.load(Ordering::Acquire);
                    buf.clear();
                    drain(&mut buf);
                    for x in &buf {
                        w.write_all(&x.to_le_bytes()).map_err(|e| e.to_string())?;
                    }
                    samples += buf.len() as u64;
                    if !buf.is_empty() {
                        // Readable by `segment_wav` while the take goes on (a loop record pass).
                        w.flush().map_err(|e| e.to_string())?;
                        wr.store(samples / 2, Ordering::Release);
                    }
                    if last {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let raw = w.into_inner().map_err(|e| e.to_string())?;
                drop(raw);
                let frames = samples / 2;
                let head = h.load(Ordering::Acquire);
                let file = f.lock().map_or_else(|e| e.into_inner().clone(), |f| f.clone());
                let n = segment_wav(&part, &file, r.load(Ordering::Acquire), b.load(Ordering::Acquire), head, frames)
                    .map_err(|e| e.to_string());
                if !k.load(Ordering::Acquire) {
                    let _ = std::fs::remove_file(&part);
                }
                Ok((file, n?))
            })
            .map_err(|e| e.to_string())?;
        Ok(Recorder { stop, head, rate, written, part: p, keep, bits, file: target, thread: Some(thread) })
    }

    /// Frames on disk so far (in the raw file, before the head is left out).
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// The raw file being written (`segment_wav` reads a part of it).
    pub fn part(&self) -> &Path {
        &self.part
    }

    /// The WAV's sample format: 16 or 24-bit integer, else 32-bit float (Disk recording's format, spec:
    /// audio/disk-recording.md §2). Set before `finish`.
    pub fn set_bits(&self, bits: u32) {
        self.bits.store(bits, Ordering::Release);
    }

    /// The WAV to make at the end instead of the one it started with (a loop record take's last pass). Set before
    /// `finish`.
    pub fn set_file(&self, file: PathBuf) {
        if let Ok(mut f) = self.file.lock() {
            *f = file;
        }
    }

    /// Leaves the raw file on disk once the WAV is made, for `segment_wav` (the caller removes it).
    pub fn keep_part(&self) {
        self.keep.store(true, Ordering::Release);
    }

    /// The take starts at frame `head` of what came in (negative: that much silence first).
    pub fn set_head(&self, head: i64) {
        self.head.store(head, Ordering::Release);
    }

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate, Ordering::Release);
    }

    /// Stops taking in; the WAV is made on the writer thread (`done`, then `join`).
    pub fn finish(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn done(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// The WAV and its frames.
    pub fn join(mut self) -> Result<(PathBuf, u64), String> {
        self.thread.take().ok_or("already joined")?.join().map_err(|_| "the recorder stopped".to_string())?
    }
}

/// Makes WAV `wav` (32-bit float stereo at `rate`) of the raw frames in `part` from frame `head` on; returns its frames.
#[cfg(test)]
fn to_wav(part: &Path, wav: &Path, rate: u32, head: i64, frames: u64) -> std::io::Result<u64> {
    segment_wav(part, wav, rate, 32, head, frames)
}

/// Makes WAV `wav` (stereo at `rate`, `bits` 16 or 24 for integer samples, else 32-bit float) of frames `start..end`
/// of the raw frames in `part` (a negative start: that much silence first); returns its frames. A loop record pass is a
/// part of its take (spec: audio/disk-recording.md §1); integer samples are rounded and clipped, not dithered (audio/disk-recording.md §2).
pub fn segment_wav(part: &Path, wav: &Path, rate: u32, bits: u32, start: i64, end: u64) -> std::io::Result<u64> {
    let (head, frames) = (start, end);
    let skip = head.max(0) as u64;
    let pad = (-head).max(0) as u64;
    let n = frames.saturating_sub(skip) + pad;
    let mut out = BufWriter::new(std::fs::File::create(wav)?);
    let mut r = std::fs::File::open(part)?;
    r.seek(SeekFrom::Start(skip.min(frames) * 8))?;
    let mut r = r.take(frames.saturating_sub(skip) * 8);
    let bytes = match bits {
        16 | 24 => bits as usize / 8,
        _ => {
            os_engine::wav::write_f32_stereo_header(&mut out, rate, n)?;
            out.write_all(&vec![0u8; (pad * 8) as usize])?;
            std::io::copy(&mut r, &mut out)?;
            out.flush()?;
            return Ok(n);
        }
    };
    int_header(&mut out, rate, bytes as u16 * 8, n)?;
    out.write_all(&vec![0u8; pad as usize * 2 * bytes])?;
    let mut buf = vec![0u8; 1 << 16];
    let mut have = 0;
    loop {
        let got = r.read(&mut buf[have..])?;
        have += got;
        let whole = have / 4 * 4;
        for x in buf[..whole].chunks_exact(4) {
            let v = f32::from_le_bytes([x[0], x[1], x[2], x[3]]).clamp(-1.0, 1.0);
            let full = if bytes == 2 { 32767.0 } else { 8_388_607.0 };
            let i = (v * full).round() as i32;
            out.write_all(&i.to_le_bytes()[..bytes])?;
        }
        buf.copy_within(whole..have, 0);
        have -= whole;
        if got == 0 {
            break;
        }
    }
    out.flush()?;
    Ok(n)
}

/// The header of an integer PCM stereo WAV of `frames` frames.
fn int_header(w: &mut impl Write, rate: u32, bits: u16, frames: u64) -> std::io::Result<()> {
    let align = 2 * bits / 8;
    let data = u32::try_from(frames * u64::from(align))
        .ok()
        .filter(|&n| n <= u32::MAX - 36)
        .ok_or_else(|| std::io::Error::other("too long for WAV"))?;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&2u16.to_le_bytes())?;
    w.write_all(&rate.to_le_bytes())?;
    w.write_all(&(rate * u32::from(align)).to_le_bytes())?;
    w.write_all(&align.to_le_bytes())?;
    w.write_all(&bits.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_order_and_drops_what_has_no_room() {
        let r = Ring::new(4);
        assert_eq!(r.push_with(3, |k| k as f32), 3);
        let mut out = Vec::new();
        r.pop_into(&mut out);
        assert_eq!(out, [0.0, 1.0, 2.0]);
        assert_eq!(r.push_with(6, |k| 10.0 + k as f32), 4, "four slots");
        out.clear();
        r.pop_into(&mut out);
        assert_eq!(out, [10.0, 11.0, 12.0, 13.0], "wrapped around");
    }

    #[test]
    fn inputs_their_labels_and_keys() {
        let v = Input::of("Driver", true, 3);
        let labels: Vec<String> = v.iter().map(Input::label).collect();
        assert_eq!(labels, ["In 1 - In 2", "In 1", "In 2", "In 3"]);
        assert_eq!(Input::parse(&v[0].key()), Some(v[0].clone()));
        assert_eq!(Input::parse(&v[3].key()), Some(v[3].clone()));
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 19).unwrap().and_hms_opt(21, 48, 43).unwrap();
        assert_eq!(file_name("untitled", t), "untitled_2026-10-19 21-48-43_REC.wav");
    }

    /// The WAV leaves out the frames before the head, or starts with silence for a negative one.
    #[test]
    fn a_wav_from_the_head_on() {
        let dir = std::env::temp_dir().join(format!("lemon_capture_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("t.wav.part");
        let raw: Vec<u8> = (0..8).flat_map(|i| (i as f32).to_le_bytes()).collect();
        std::fs::write(&part, raw).unwrap();
        let wav = dir.join("t.wav");
        assert_eq!(to_wav(&part, &wav, 48000, 1, 4).unwrap(), 3);
        let b = std::fs::read(&wav).unwrap();
        let data: Vec<f32> = b[44..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(data, [2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(to_wav(&part, &wav, 48000, -1, 4).unwrap(), 5);
        let b = std::fs::read(&wav).unwrap();
        assert_eq!(b.len(), 44 + 5 * 8);
        assert_eq!(&b[44..52], &[0u8; 8]);
        // 16 and 24-bit integer: rounded and clipped.
        let raw: Vec<u8> = [0.5f32, -1.0, 2.0, 0.25].iter().flat_map(|x| x.to_le_bytes()).collect();
        std::fs::write(&part, raw).unwrap();
        assert_eq!(segment_wav(&part, &wav, 44100, 16, 0, 2).unwrap(), 2);
        let b = std::fs::read(&wav).unwrap();
        assert_eq!((u16::from_le_bytes([b[20], b[21]]), u16::from_le_bytes([b[34], b[35]]), b.len()), (1, 16, 44 + 8));
        let v: Vec<i16> = b[44..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(v, [16384, -32767, 32767, 8192]);
        assert_eq!(segment_wav(&part, &wav, 44100, 24, 1, 2).unwrap(), 1);
        let b = std::fs::read(&wav).unwrap();
        assert_eq!((u16::from_le_bytes([b[34], b[35]]), &b[44..]), (24, &[0xff, 0xff, 0x7f, 0x00, 0x00, 0x20][..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real input device takes in (needs one: `cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn a_system_input_takes_in() {
        let (name, ..) = system_inputs().into_iter().next().expect("an input device");
        capture().start(0, true);
        let stream = open_input(&name).expect("opens");
        let rate = stream.rate;
        std::thread::sleep(std::time::Duration::from_millis(500));
        drop(stream);
        capture().stop();
        let frames = capture().frames();
        assert!(frames > 0, "{name}: nothing came in");
        eprintln!("{name}: {frames} frames in 0.5 s at {rate} Hz");
    }

    /// Stereo takes a pair; one input goes to both sides.
    #[test]
    fn a_block_goes_in_as_stereo() {
        let c = Capture {
            ring: Ring::new(64),
            on: AtomicBool::new(false),
            first: AtomicU32::new(0),
            stereo: AtomicBool::new(true),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        };
        c.push(2, |ch, f| (ch * 10 + f) as f32);
        assert_eq!(c.frames(), 0, "off");
        c.start(1, true);
        c.push(2, |ch, f| (ch * 10 + f) as f32);
        c.start(2, false);
        c.push(1, |ch, f| (ch * 10 + f) as f32);
        let mut out = Vec::new();
        c.drain(&mut out);
        assert_eq!(out, [20.0, 20.0], "start empties what came before");
        assert_eq!(c.frames(), 1);
    }
}
