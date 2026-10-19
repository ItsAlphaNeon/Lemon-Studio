// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The output device: one stream for everything Lemon Studio plays, mixed here.
//!
//! An ASIO driver serves one stream per program, so playback, previews and live notes do not open streams of their
//! own: each is a `Source` that the device's callback mixes. Sources come in and go out through two lock-free queues,
//! so the callback never locks, allocates or frees memory; a source let go of (`Source::detached`) is handed back and
//! freed on the audio thread (`OutputDevice::collect`).
//!
//! The device opens with the audio settings (`audio_cfg`) when first needed: a Windows (or ALSA, Core Audio) device
//! through cpal, an ASIO driver through `asio`. A Windows device closes again once nothing plays (`close_idle`), as the
//! streams did when each sound had its own: an open stream keeps the computer from sleeping. An ASIO driver stays open
//! (another program could take it meanwhile) until Auto close or new settings let go of it. A stream that failed (the
//! device went away, a driver asked to be reset) reports it (`failure`) and is opened anew the next time it is needed.
//!
//! What opened is published (`last`, `rate`), so the settings page and the hosted plugins never have to ask the device
//! themselves: asking an ASIO driver means loading it, and only the thread that drives the device may.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig};
use crossbeam_queue::ArrayQueue;

use crate::audio_cfg::{self, AudioCfg};

#[cfg(windows)]
mod asio;

/// `DeviceInfo::host` of an ASIO driver.
pub const ASIO: &str = "ASIO";

/// The installed ASIO drivers' names (none off Windows). None is loaded to list them, so any thread may ask.
pub fn asio_drivers() -> Vec<String> {
    #[cfg(windows)]
    return asio::drivers();
    #[cfg(not(windows))]
    Vec::new()
}

/// What the device's callback plays: one stream's worth of sound (playback, a preview, live notes).
pub(crate) trait Source: Send {
    /// Writes the next `stereo.len() / 2` frames (interleaved L/R) into `stereo`, overwriting what is there.
    fn play(&mut self, stereo: &mut [f32]);
    /// Its owner let go of it: the callback no longer plays it and hands it back (`OutputDevice::collect`).
    fn detached(&self) -> bool;
}

/// Sources the device mixes at once (playback, live notes, a preview and the stopped ones still fading out).
const MAX_SOURCES: usize = 64;
/// Frames the mix holds without growing: twice the longest buffer length the settings offer.
const MAX_FRAMES: usize = 2 * 8192;

type Queue = ArrayQueue<Box<dyn Source>>;

/// The callback's side: the sources playing, the ones arriving and the ones handed back.
struct Mix {
    sources: Vec<Box<dyn Source>>,
    inbox: Arc<Queue>,
    outbox: Arc<Queue>,
    /// One source's block, and the sum of all of them (interleaved stereo).
    one: Vec<f32>,
    sum: Vec<f32>,
}

impl Mix {
    fn new(inbox: Arc<Queue>, outbox: Arc<Queue>) -> Mix {
        Mix { sources: Vec::with_capacity(MAX_SOURCES), inbox, outbox, one: Vec::with_capacity(2 * MAX_FRAMES), sum: Vec::with_capacity(2 * MAX_FRAMES) }
    }

    /// The next `frames` frames of every source, summed (interleaved stereo, not clipped).
    fn block(&mut self, frames: usize) -> &[f32] {
        while self.sources.len() < MAX_SOURCES {
            match self.inbox.pop() {
                Some(s) => self.sources.push(s),
                None => break,
            }
        }
        self.sum.clear();
        self.sum.resize(2 * frames, 0.0);
        self.one.resize(2 * frames, 0.0);
        let mut i = 0;
        while i < self.sources.len() {
            if self.sources[i].detached() {
                let s = self.sources.swap_remove(i);
                match self.outbox.push(s) {
                    // The source moved into `i` has not played yet.
                    Ok(()) => continue,
                    // No room to hand it back: it stays, silent, until there is.
                    Err(s) => {
                        let last = self.sources.len();
                        self.sources.push(s);
                        self.sources.swap(i, last);
                        i += 1;
                        continue;
                    }
                }
            }
            self.sources[i].play(&mut self.one);
            for (a, b) in self.sum.iter_mut().zip(&self.one) {
                *a += *b;
            }
            i += 1;
        }
        &self.sum
    }
}

/// What a device opened as (`Transport::opened`, `last`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The audio system it is on, cpal's name for it ("WASAPI", "ASIO", "ALSA", ...).
    pub host: &'static str,
    /// The device's name as the settings show it.
    pub name: String,
    pub rate: u32,
    /// The buffer length asked for (`None`: the device's own); an ASIO driver's own.
    pub fixed: Option<u32>,
    /// The buffer lengths the device says it takes, frames (`None`: it does not say).
    pub buffers: Option<(u32, u32)>,
    /// Output channels open (the master plays on the first two).
    pub channels: u16,
    /// An ASIO driver's: the rates of `audio_cfg::RATES` and the lengths of `audio_cfg::BUFFERS` it takes (empty for
    /// a Windows device, which is not asked).
    pub rates: Vec<u32>,
    pub sizes: Vec<u32>,
    /// It opened in place of the ASIO driver chosen: "Couldn't open <driver>: <why>; using <this device>".
    pub instead: Option<String>,
}

/// The device last opened and the settings it opened for (`rate`), and how many times one opened (`opens`).
static LAST: Mutex<Option<(AudioCfg, DeviceInfo)>> = Mutex::new(None);
static OPENS: AtomicU64 = AtomicU64::new(0);

/// The device that opened last (it may have closed since).
pub fn last() -> Option<DeviceInfo> {
    LAST.lock().ok()?.as_ref().map(|(_, i)| i.clone())
}

/// Counts the devices opened: a change means `last` is another (or the same opened again).
pub fn opens() -> u64 {
    OPENS.load(Ordering::Acquire)
}

/// The sample rate the device plays at with the audio settings in use: the rate it last opened at with them, else what
/// the settings' device says it would open at (`audio_cfg::rate`).
pub fn rate() -> Option<u32> {
    let cfg = AudioCfg::current();
    let opened = LAST.lock().ok().and_then(|l| l.as_ref().filter(|(c, _)| *c == cfg).map(|(_, i)| i.rate));
    opened.or_else(audio_cfg::rate)
}

/// Makes the calling thread one that can drive the device. Windows: a COM single-threaded apartment, which ASIO
/// drivers (COM objects) are made in and called from; cpal asks the same of the threads that open its streams.
pub(crate) fn init_thread() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32);
    }
}

/// Handles the window messages waiting for the calling thread. Windows: an ASIO driver may make windows on the thread
/// that loaded it (its control panel, a hidden one it keeps time with), which only work while that thread takes its
/// messages.
pub(crate) fn pump() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// What plays the mix: a cpal stream or an ASIO driver.
enum Stream {
    Cpal(#[allow(dead_code)] cpal::Stream),
    #[cfg(windows)]
    Asio(#[allow(dead_code)] asio::AsioOut),
}

/// The open stream and what it was opened with.
pub(crate) struct Open {
    /// Dropped first: the callback (and the sources in it) stop before the queues go.
    _stream: Stream,
    inbox: Arc<Queue>,
    outbox: Arc<Queue>,
    pub(crate) info: DeviceInfo,
    /// What to tell about how it opened (the ASIO driver that did not open, and what plays instead).
    note: Option<String>,
    /// The stream reported an error it does not recover from (the device went away), and what it said.
    failed: Arc<AtomicBool>,
    fault: Arc<Mutex<Option<String>>>,
}

/// The one output stream (closed until first needed).
#[derive(Default)]
pub(crate) struct OutputDevice {
    open: Option<Open>,
    /// Blocks the device itself reports as lost (an ASIO driver's overload), since `take_xruns`.
    xruns: Arc<AtomicU64>,
    /// An ASIO driver that does not open is an error, not the system's default device (while it is tried again).
    pub(crate) wait_for_asio: bool,
}

impl OutputDevice {
    /// The open stream, opening it with the audio settings first if it is closed or has failed.
    pub(crate) fn open(&mut self) -> Result<&Open, String> {
        if self.open.as_ref().is_some_and(|o| o.failed.load(Ordering::Acquire)) {
            self.open = None;
        }
        if self.open.is_none() {
            let cfg = AudioCfg::current();
            let o = open(&cfg, &self.xruns, self.wait_for_asio)?;
            if let Ok(mut l) = LAST.lock() {
                *l = Some((cfg, o.info.clone()));
            }
            OPENS.fetch_add(1, Ordering::AcqRel);
            self.open = Some(o);
        }
        Ok(self.open.as_ref().expect("opened"))
    }

    /// Tests: the open stream fails as if its device went away.
    #[cfg(test)]
    pub(crate) fn fail(&self, why: &str) {
        if let Some(o) = &self.open {
            if let Ok(mut f) = o.fault.lock() {
                *f = Some(why.into());
            }
            o.failed.store(true, Ordering::Release);
        }
    }

    /// Lets go of the device; every source still in it goes with it.
    pub(crate) fn close(&mut self) {
        self.open = None;
    }

    /// Nothing plays any more: lets go of a Windows device (an ASIO driver stays open).
    pub(crate) fn close_idle(&mut self) {
        if !self.is_asio() {
            self.close();
        }
    }

    /// An ASIO driver is open.
    pub(crate) fn is_asio(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.info.host == ASIO)
    }

    /// What to tell about how the device opened, once.
    pub(crate) fn take_note(&mut self) -> Option<String> {
        self.open.as_mut()?.note.take()
    }

    /// Opens the ASIO driver's own settings window (opening the driver first if it is closed).
    pub(crate) fn asio_panel(&mut self) -> Result<(), String> {
        self.open()?;
        if !self.is_asio() {
            return Err("the output is not an ASIO driver".into());
        }
        #[cfg(windows)]
        return asio::control_panel();
        #[cfg(not(windows))]
        Ok(())
    }

    /// Plays `source` from the next block on (opening the device first if it is closed).
    pub(crate) fn add(&mut self, source: Box<dyn Source>) -> Result<(), String> {
        let open = self.open()?;
        open.inbox.push(source).map_err(|_| "too many sounds at once on the audio device".to_string())
    }

    /// The open stream failed: the device's name and what the stream said (it opens anew with the next `open`).
    pub(crate) fn failure(&self) -> Option<(String, String)> {
        let o = self.open.as_ref().filter(|o| o.failed.load(Ordering::Acquire))?;
        let why = o.fault.lock().ok().and_then(|f| f.clone()).unwrap_or_else(|| "it stopped".into());
        Some((o.info.name.clone(), why))
    }

    /// The device's own dropouts since the last call.
    pub(crate) fn take_xruns(&self) -> u64 {
        self.xruns.swap(0, Ordering::Relaxed)
    }

    /// Frees the sources the callback handed back (it never frees memory itself).
    pub(crate) fn collect(&self) {
        if let Some(o) = &self.open {
            while o.outbox.pop().is_some() {}
        }
    }
}

/// Opens the stream for `cfg`. An ASIO driver that does not open (another program has it, its device is missing)
/// gives way to the system's default device, saying so (unless `wait`: then that is the error); the settings keep the
/// driver, which is tried again with the next open (spec: audio/devices.md, "Driver reset, lost device, fallback").
fn open(cfg: &AudioCfg, xruns: &Arc<AtomicU64>, wait: bool) -> Result<Open, String> {
    #[cfg(windows)]
    if let Some(name) = cfg.asio_driver() {
        let e = match open_asio(name, cfg, xruns) {
            Ok(o) => return Ok(o),
            Err(e) if wait => return Err(format!("Couldn't open {name}: {e}")),
            Err(e) => e,
        };
        let system = AudioCfg { asio: false, device: None, ..cfg.clone() };
        let mut o = open_system(&system, xruns).map_err(|f| format!("Couldn't open {name}: {e} (nor the default device: {f})"))?;
        let instead = format!("Couldn't open {name}: {e}; using {}", o.info.name);
        (o.note, o.info.instead) = (Some(instead.clone()), Some(instead));
        return Ok(o);
    }
    let _ = wait; // (no drivers to wait for off Windows)
    open_system(cfg, xruns)
}

/// The two queues a new stream's callback takes its sources from and hands them back on, and where its errors go.
fn queues(xruns: &Arc<AtomicU64>) -> (Arc<Queue>, Arc<Queue>, Errors) {
    let errors = Errors { failed: Arc::new(AtomicBool::new(false)), fault: Arc::new(Mutex::new(None)), xruns: xruns.clone() };
    (Arc::new(Queue::new(MAX_SOURCES)), Arc::new(Queue::new(MAX_SOURCES)), errors)
}

/// Opens ASIO driver `name`, at the rate and buffer length of `cfg` when it takes them.
#[cfg(windows)]
fn open_asio(name: &str, cfg: &AudioCfg, xruns: &Arc<AtomicU64>) -> Result<Open, String> {
    let (inbox, outbox, errors) = queues(xruns);
    let a = asio::open(name, cfg.rate, cfg.buffer, Mix::new(inbox.clone(), outbox.clone()), errors.clone())?;
    let (fixed, buffers) = (Some(a.frames), Some(a.buffers));
    let info = DeviceInfo { host: ASIO, name: name.to_string(), rate: a.rate, fixed, buffers, channels: a.channels, rates: a.rates, sizes: a.sizes, instead: None };
    Ok(Open { _stream: Stream::Asio(a.out), inbox, outbox, info, note: None, failed: errors.failed, fault: errors.fault })
}

/// Opens a Windows (ALSA, Core Audio) device's stream for `cfg`: its device, rate and buffer length, else the
/// device's own buffer length.
fn open_system(cfg: &AudioCfg, xruns: &Arc<AtomicU64>) -> Result<Open, String> {
    let out = audio_cfg::output(cfg)?;
    let (device, format, mut config) = (out.device, out.format, out.config);
    let (host, buffers) = (out.host, out.buffers);
    let (inbox, outbox, errors) = queues(xruns);
    let mix = || Mix::new(inbox.clone(), outbox.clone());
    let mut fixed = match config.buffer_size {
        BufferSize::Fixed(b) => Some(b),
        BufferSize::Default => None,
    };
    let mut stream = build_any(&device, format, &config, mix(), &errors);
    if stream.is_err() && fixed.is_some() {
        // A buffer length the device refuses: its own.
        config.buffer_size = BufferSize::Default;
        fixed = None;
        stream = build_any(&device, format, &config, mix(), &errors);
    }
    let stream = stream?;
    stream.play().map_err(|e| e.to_string())?;
    let info = DeviceInfo { host, name: out.name, rate: config.sample_rate, fixed, buffers, channels: config.channels, rates: Vec::new(), sizes: Vec::new(), instead: None };
    Ok(Open { _stream: Stream::Cpal(stream), inbox, outbox, info, note: None, failed: errors.failed, fault: errors.fault })
}

/// Where a stream's errors go (its error callback, which runs off the audio callback).
#[derive(Clone)]
struct Errors {
    failed: Arc<AtomicBool>,
    fault: Arc<Mutex<Option<String>>>,
    xruns: Arc<AtomicU64>,
}

impl Errors {
    fn report(&self, e: cpal::Error) {
        use cpal::ErrorKind::*;
        match e.kind() {
            Xrun => self.xrun(),
            // The stream plays on (moved to the new default device, or at a normal priority).
            DeviceChanged | RealtimeDenied => eprintln!("audio stream: {e}"),
            // Anything else: the stream has to be opened anew.
            _ => {
                eprintln!("audio stream error: {e}");
                self.fail(e.to_string());
            }
        }
    }

    /// The stream does not play on: it is opened anew (`OutputDevice::failure`); the first reason is kept.
    fn fail(&self, why: String) {
        if let Ok(mut f) = self.fault.lock() {
            f.get_or_insert(why);
        }
        self.failed.store(true, Ordering::Release);
    }

    /// The device lost a block.
    fn xrun(&self) {
        self.xruns.fetch_add(1, Ordering::Relaxed);
    }
}

/// An output stream in the device's sample format.
fn build_any(device: &cpal::Device, format: SampleFormat, config: &StreamConfig, mix: Mix, errors: &Errors) -> Result<cpal::Stream, String> {
    let errors = errors.clone();
    match format {
        SampleFormat::F32 => build::<f32>(device, config, mix, errors),
        SampleFormat::F64 => build::<f64>(device, config, mix, errors),
        SampleFormat::I16 => build::<i16>(device, config, mix, errors),
        SampleFormat::I24 => build::<cpal::I24>(device, config, mix, errors),
        SampleFormat::I32 => build::<i32>(device, config, mix, errors),
        SampleFormat::U16 => build::<u16>(device, config, mix, errors),
        other => Err(format!("unsupported output sample format {other:?}")),
    }
}

fn build<T>(device: &cpal::Device, config: &StreamConfig, mut mix: Mix, errors: Errors) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    device
        .build_output_stream::<T, _, _>(
            config.clone(),
            move |out: &mut [T], _| {
                let stereo = mix.block(out.len() / channels);
                // Left and right on the first two channels (a mono device gets the left), silence on the others.
                for (frame, s) in out.chunks_mut(channels).zip(stereo.chunks_exact(2)) {
                    for (c, slot) in frame.iter_mut().enumerate() {
                        let v = if c < 2 { s[c] } else { 0.0 };
                        *slot = T::from_sample(v.clamp(-1.0, 1.0));
                    }
                }
            },
            move |e| errors.report(e),
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A source that plays a constant and counts its blocks.
    struct Level {
        value: f32,
        gone: Arc<AtomicBool>,
        blocks: Arc<AtomicUsize>,
    }

    impl Source for Level {
        fn play(&mut self, stereo: &mut [f32]) {
            stereo.fill(self.value);
            self.blocks.fetch_add(1, Ordering::Relaxed);
        }
        fn detached(&self) -> bool {
            self.gone.load(Ordering::Relaxed)
        }
    }

    fn level(value: f32) -> (Box<dyn Source>, Arc<AtomicBool>, Arc<AtomicUsize>) {
        let (gone, blocks) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        (Box::new(Level { value, gone: gone.clone(), blocks: blocks.clone() }), gone, blocks)
    }

    fn mix(capacity: usize) -> (Mix, Arc<Queue>, Arc<Queue>) {
        let (inbox, outbox) = (Arc::new(Queue::new(capacity)), Arc::new(Queue::new(capacity)));
        (Mix::new(inbox.clone(), outbox.clone()), inbox, outbox)
    }

    /// No source: silence, as long as the device asks.
    #[test]
    fn an_empty_mix_is_silent() {
        let (mut m, _, _) = mix(4);
        assert_eq!(m.block(256), &[0.0; 512][..]);
        assert_eq!(m.block(3).len(), 6);
    }

    /// Every source plays every block, and the device hears their sum.
    #[test]
    fn sources_are_summed() {
        let (mut m, inbox, _) = mix(4);
        let (a, _, a_blocks) = level(0.25);
        let (b, _, b_blocks) = level(-0.5);
        assert!(inbox.push(a).is_ok() && inbox.push(b).is_ok());
        for _ in 0..3 {
            assert!(m.block(64).iter().all(|v| *v == -0.25));
        }
        assert_eq!((a_blocks.load(Ordering::Relaxed), b_blocks.load(Ordering::Relaxed)), (3, 3));
        // Not clipped here: the device's format conversion does that once.
        let (c, _, _) = level(1.0);
        let (d, _, _) = level(1.0);
        assert!(inbox.push(c).is_ok() && inbox.push(d).is_ok());
        assert!(m.block(8).iter().all(|v| *v == 1.75));
    }

    /// A source let go of stops at the next block and is handed back to be freed; the others play on.
    #[test]
    fn a_detached_source_is_handed_back() {
        let (mut m, inbox, outbox) = mix(4);
        let (a, a_gone, a_blocks) = level(0.25);
        let (b, _, b_blocks) = level(0.5);
        assert!(inbox.push(a).is_ok() && inbox.push(b).is_ok());
        m.block(32);
        a_gone.store(true, Ordering::Relaxed);
        assert!(m.block(32).iter().all(|v| *v == 0.5), "only the other source");
        assert_eq!(a_blocks.load(Ordering::Relaxed), 1, "not played once let go of");
        assert_eq!(b_blocks.load(Ordering::Relaxed), 2);
        assert_eq!(outbox.len(), 1, "handed back");
        assert_eq!(m.sources.len(), 1);
    }

    /// With no room to hand a source back, it waits in the mix, silent, and goes once there is room.
    #[test]
    fn a_full_outbox_keeps_the_source_silent() {
        let (mut m, inbox, outbox) = mix(1);
        let (a, a_gone, _) = level(0.25);
        let (b, b_gone, _) = level(0.5);
        let (c, _, _) = level(0.125);
        assert!(inbox.push(a).is_ok());
        m.block(16);
        assert!(inbox.push(b).is_ok());
        m.block(16);
        assert!(inbox.push(c).is_ok());
        m.block(16);
        a_gone.store(true, Ordering::Relaxed);
        b_gone.store(true, Ordering::Relaxed);
        assert!(m.block(16).iter().all(|v| *v == 0.125), "neither detached source plays");
        assert_eq!((outbox.len(), m.sources.len()), (1, 2), "one handed back, one waits");
        while outbox.pop().is_some() {}
        assert!(m.block(16).iter().all(|v| *v == 0.125));
        assert_eq!((outbox.len(), m.sources.len()), (1, 1));
    }

    /// A block longer than the reserved room still plays (the mix grows rather than cut it).
    #[test]
    fn a_long_block_plays_whole() {
        let (mut m, inbox, _) = mix(2);
        let (a, _, _) = level(0.5);
        assert!(inbox.push(a).is_ok());
        let got = m.block(MAX_FRAMES + 100);
        assert_eq!(got.len(), 2 * (MAX_FRAMES + 100));
        assert!(got.iter().all(|v| *v == 0.5));
    }
}
