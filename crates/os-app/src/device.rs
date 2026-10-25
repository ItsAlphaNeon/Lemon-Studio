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
//! The device opens with the audio settings (`audio_cfg`) when first needed: a Windows (or Linux, Core Audio) device
//! through cpal (on Linux the sound server's in shared mode, ALSA's in exclusive mode: `audio_cfg::host`), an ASIO
//! driver through `asio`, and a Windows device in exclusive mode, or in shared mode at a period
//! shorter than its default, through `wasapi`. A Windows device closes again once nothing plays (`close_idle`), as the
//! streams did when each sound had its own: an open stream keeps the computer from sleeping (and in exclusive mode
//! silences every other program). An ASIO driver stays open (another program could take it meanwhile) until Auto close
//! or new settings let go of it. A stream that failed (the device went away, a driver asked to be reset) reports it
//! (`failure`) and is opened anew the next time it is needed.
//!
//! What opened is published (`last`, `rate`), so the settings page and the hosted plugins never have to ask the device
//! themselves: asking an ASIO driver means loading it, and only the thread that drives the device may.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig};
use crossbeam_queue::ArrayQueue;

use crate::audio_cfg::{self, AudioCfg, BUFFERS};

#[cfg(windows)]
mod asio;
#[cfg(windows)]
mod wasapi;
#[cfg(target_os = "linux")]
mod sound_server;

/// Tests leave the sound server's cards alone, but for the one that checks they are given back.
static HEAL_IN_TESTS: AtomicBool = AtomicBool::new(false);

/// Linux: the sound server opens again the cards it gave up on while Lemon Studio (this run, or one that crashed) had
/// them through ALSA (`sound_server`); elsewhere nothing. It waits on the server: not for the audio callback.
pub(crate) fn heal_sound_server() {
    #[cfg(target_os = "linux")]
    if !cfg!(test) || HEAL_IN_TESTS.load(Ordering::Relaxed) {
        for card in sound_server::heal() {
            eprintln!("sound server: opened {card} again (it gave up while Lemon Studio had it through ALSA)");
        }
    }
}

/// `DeviceInfo::host` of an ASIO driver, and of a Windows device in exclusive mode.
pub const ASIO: &str = "ASIO";
pub const WASAPI_EXCLUSIVE: &str = "WASAPI exclusive";
/// `DeviceInfo::host` of a Linux device opened directly (exclusive mode), cpal's name for it.
const ALSA: &str = "ALSA";

/// The installed ASIO drivers' names (none off Windows). None is loaded to list them, so any thread may ask.
pub fn asio_drivers() -> Vec<String> {
    #[cfg(windows)]
    return asio::drivers();
    #[cfg(not(windows))]
    Vec::new()
}

/// What a Windows device offers for short buffers (`periods`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Periods {
    /// The device's own rate (the system mixer's for it).
    pub rate: u32,
    /// Shared mode: the system mixer's periods on it, frames at `rate` (IAudioClient3): the default, the step, the
    /// smallest and the largest. `None`: it does not say.
    pub shared: Option<(u32, u32, u32, u32)>,
    /// Shared mode: the buffer a stream gets when it asks for no length (0: unknown).
    pub shared_buffer: u32,
    /// Exclusive mode: its default and smallest period, in 100-nanosecond units (whatever the rate).
    pub exclusive: (i64, i64),
    /// The rates of `audio_cfg::RATES` it takes in exclusive mode (empty: unknown, or exclusive mode is refused).
    pub exclusive_rates: Vec<u32>,
}

impl Periods {
    /// Exclusive mode's default and smallest period in frames at `rate`.
    pub fn exclusive_frames(&self, rate: u32) -> (u32, u32) {
        let f = |hns: i64| (hns as f64 * f64::from(rate) / 1e7).round() as u32;
        (f(self.exclusive.0), f(self.exclusive.1))
    }

    /// The device's own period ("Device"): exclusive, its default at `rate`; shared, the system mixer's default for it.
    pub fn own(&self, exclusive: bool, rate: u32) -> u32 {
        if exclusive {
            self.exclusive_frames(rate).0
        } else {
            self.shared_period()
        }
    }

    /// Shared mode's period: the system mixer's default for the device.
    pub fn shared_period(&self) -> u32 {
        self.shared.map_or_else(|| self.exclusive_frames(self.rate).0, |s| s.0)
    }

    /// Shared mode's lengths shorter than its default period: its smallest, then each of `BUFFERS` that is a whole
    /// number of steps inside its range.
    pub fn small(&self) -> Vec<u32> {
        let Some((def, step, min, max)) = self.shared else { return Vec::new() };
        if min == 0 || min >= def {
            return Vec::new();
        }
        let mut v = vec![min];
        v.extend(BUFFERS.into_iter().filter(|&b| b > min && b < def && b <= max && b % step.max(1) == 0));
        v
    }

    /// The lengths the Buffer length menu offers after the device's own (spec: audio/wasapi-low-latency.md 4.3), each
    /// once. Shared: the short ones, then those of `BUFFERS` longer than the buffer a stream gets anyway (they add
    /// latency). Exclusive (at `rate`): those of `BUFFERS` from its smallest period up ("Smallest" comes before them).
    pub fn sizes(&self, exclusive: bool, rate: u32) -> Vec<u32> {
        if exclusive {
            let (def, min) = self.exclusive_frames(rate);
            return BUFFERS.into_iter().filter(|&b| b >= min && b != def && b != min).collect();
        }
        let small = self.small();
        let anyway = self.shared_buffer.max(self.shared_period());
        let longer = BUFFERS.into_iter().filter(|&b| b > anyway && !small.contains(&b));
        small.iter().copied().chain(longer).collect()
    }
}

/// What the Windows device named `device` (`None`: the default) offers for short buffers; `None` off Windows, for a
/// device that is not there, or when Windows does not say. It plays nothing: any thread may ask.
pub fn periods(device: Option<&str>) -> Option<Periods> {
    #[cfg(windows)]
    return wasapi::periods(device);
    #[cfg(not(windows))]
    {
        let _ = device;
        None
    }
}

/// What the device's callback plays: one stream's worth of sound (playback, a preview, live notes).
pub(crate) trait Source: Send {
    /// Writes the next `stereo.len() / 2` frames (interleaved L/R) into `stereo`, overwriting what is there.
    fn play(&mut self, stereo: &mut [f32]);
    /// Its owner let go of it: the callback no longer plays it and hands it back (`OutputDevice::collect`).
    fn detached(&self) -> bool;
}

/// Counts the device's blocks (0: none yet), so the sources of one block can tell they share it (`blocks`).
static BLOCKS: AtomicU64 = AtomicU64::new(0);

/// The device block being played now (sources asked within one block see the same count).
pub(crate) fn blocks() -> u64 {
    BLOCKS.load(Ordering::Relaxed)
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
        BLOCKS.fetch_add(1, Ordering::Relaxed);
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
    /// An ASIO driver's inputs open with its outputs (One-click audio recording), and its input latency, frames.
    pub inputs: u16,
    pub input_latency: Option<u32>,
    /// An ASIO driver's: the rates of `audio_cfg::RATES` and the lengths of `audio_cfg::BUFFERS` it takes (empty for
    /// a Windows device, which is not asked).
    pub rates: Vec<u32>,
    pub sizes: Vec<u32>,
    /// The driver's own output latency, frames: from the block it is handed to the sound leaving the device (an ASIO
    /// driver says it; a Windows device is not asked).
    pub latency: Option<u32>,
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

/// What plays the mix: a cpal stream, an ASIO driver or a WASAPI stream of Lemon Studio's own.
enum Stream {
    Cpal(#[allow(dead_code)] cpal::Stream),
    #[cfg(windows)]
    Asio(#[allow(dead_code)] asio::AsioOut),
    #[cfg(windows)]
    Wasapi(#[allow(dead_code)] wasapi::WasapiOut),
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

/// The one output stream (closed until first needed). Dropped (the audio thread ends as the app exits), it lets go of
/// the device as `close` does.
#[derive(Default)]
pub(crate) struct OutputDevice {
    open: Option<Open>,
    /// Blocks the device itself reports as lost (an ASIO driver's overload), since `take_xruns`.
    xruns: Arc<AtomicU64>,
    /// The device chosen not opening (an ASIO driver; a Windows device another program has, or that refuses exclusive
    /// mode for that reason) is an error, not the system's default device or shared mode (while it is tried again).
    pub(crate) wait: bool,
}

impl Drop for OutputDevice {
    fn drop(&mut self) {
        self.close();
    }
}

impl OutputDevice {
    /// The open stream, opening it with the audio settings first if it is closed or has failed.
    pub(crate) fn open(&mut self) -> Result<&Open, String> {
        if self.open.as_ref().is_some_and(|o| o.failed.load(Ordering::Acquire)) {
            self.close();
        }
        if self.open.is_none() {
            let cfg = AudioCfg::current();
            let o = open(&cfg, &self.xruns, self.wait)?;
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

    /// Lets go of the device; every source still in it goes with it. An ALSA device goes back to the sound server.
    pub(crate) fn close(&mut self) {
        let alsa = self.open.as_ref().is_some_and(|o| o.info.host == ALSA);
        self.open = None;
        if alsa {
            heal_sound_server();
        }
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
/// driver, which is tried again with the next open (spec: audio/devices.md, "Driver reset, lost device, fallback"). A
/// Windows device in exclusive mode that refuses it plays in shared mode, saying why (`open_exclusive`).
fn open(cfg: &AudioCfg, xruns: &Arc<AtomicU64>, wait: bool) -> Result<Open, String> {
    #[cfg(windows)]
    if let Some(name) = cfg.asio_driver() {
        let e = match open_asio(name, cfg, xruns) {
            Ok(o) => return Ok(o),
            Err(e) if wait => return Err(format!("Couldn't open {name}: {e}")),
            Err(e) => e,
        };
        let system = AudioCfg { asio: false, device: None, exclusive: false, ..cfg.clone() };
        let mut o = open_system(&system, xruns).map_err(|f| format!("Couldn't open {name}: {e} (nor the default device: {f})"))?;
        let instead = format!("Couldn't open {name}: {e}; using {}", o.info.name);
        say_instead(&mut o, instead);
        return Ok(o);
    }
    #[cfg(windows)]
    if cfg.exclusive() {
        return open_exclusive(cfg, xruns, wait);
    }
    #[cfg(windows)]
    let note = match open_small(cfg, xruns) {
        Ok(o) => return Ok(o),
        Err(note) => note,
    };
    #[cfg(not(windows))]
    let note: Option<String> = None;
    let mut o = open_shared(cfg, xruns, wait)?;
    if let Some(n) = note.filter(|_| o.note.is_none()) {
        say_instead(&mut o, n);
    }
    Ok(o)
}

/// What the status tells about how `o` opened, above the status lines and once as a message.
fn say_instead(o: &mut Open, instead: String) {
    (o.note, o.info.instead) = (Some(instead.clone()), Some(instead));
}

/// A Windows (ALSA, Core Audio) device in shared mode. A device chosen that is there but does not open (another
/// program has it to itself) gives way to the system's default device when that is another, saying so (unless `wait`:
/// then that is the error; spec: audio/wasapi-low-latency.md 4.5).
fn open_shared(cfg: &AudioCfg, xruns: &Arc<AtomicU64>, wait: bool) -> Result<Open, String> {
    let e = match open_system(cfg, xruns) {
        Ok(o) => return Ok(o),
        Err(e) => e,
    };
    let Some(name) = cfg.device.as_deref().filter(|_| !wait) else { return Err(e) };
    let mut o = open_system(&AudioCfg { device: None, ..cfg.clone() }, xruns).map_err(|_| e.clone())?;
    if o.info.name != name {
        let instead = format!("Couldn't open {name}: {e}; using {}", o.info.name);
        say_instead(&mut o, instead);
    }
    Ok(o)
}

/// A Windows device in exclusive mode (spec: audio/wasapi-low-latency.md 4.5). Refused, it plays in shared mode,
/// saying why: at once when Windows' sound settings do not allow it, else (another program has the device to itself)
/// unless `wait`. A device that is not there: the system's default, as in shared mode.
#[cfg(windows)]
fn open_exclusive(cfg: &AudioCfg, xruns: &Arc<AtomicU64>, wait: bool) -> Result<Open, String> {
    use wasapi::Refused;
    let (inbox, outbox, errors) = queues(xruns);
    let mix = Mix::new(inbox.clone(), outbox.clone());
    let shared = AudioCfg { exclusive: false, buffer: cfg.buffer.filter(|&b| b != audio_cfg::SMALLEST), ..cfg.clone() };
    let refused = match wasapi::open(cfg.device.as_deref(), wasapi::Mode::Exclusive, cfg.rate, cfg.buffer, mix, errors.clone()) {
        Ok(w) => return Ok(wasapi_open(w, WASAPI_EXCLUSIVE, inbox, outbox, errors)),
        Err(Refused::Missing) => return open_shared(&shared, xruns, wait),
        Err(r @ Refused::InUse) if wait => return Err(format!("Couldn't open {}: {}", cfg.device.as_deref().unwrap_or("the default device"), r.why())),
        Err(r) => r.why(),
    };
    let mut o = open_shared(&shared, xruns, false).map_err(|f| {
        format!("Couldn't open {}: {refused} (nor in shared mode: {f})", cfg.device.as_deref().unwrap_or("the default device"))
    })?;
    let name = cfg.device.clone().unwrap_or_else(|| o.info.name.clone());
    let instead = if o.info.name == name {
        format!("Couldn't open {name} in exclusive mode: {refused}; using shared mode")
    } else {
        format!("Couldn't open {name}: {refused}; using {}", o.info.name)
    };
    say_instead(&mut o, instead);
    Ok(o)
}

/// A Windows device in shared mode at a period shorter than its default (IAudioClient3), when the buffer length chosen
/// is one it offers. Otherwise cpal opens it: `Err` with what to tell (a short period needs the device's own rate).
#[cfg(windows)]
fn open_small(cfg: &AudioCfg, xruns: &Arc<AtomicU64>) -> Result<Open, Option<String>> {
    use wasapi::Refused;
    let Some(b) = cfg.buffer.filter(|&b| b != audio_cfg::SMALLEST) else { return Err(None) };
    let (inbox, outbox, errors) = queues(xruns);
    let mix = Mix::new(inbox.clone(), outbox.clone());
    match wasapi::open(cfg.device.as_deref(), wasapi::Mode::Small, cfg.rate, Some(b), mix, errors.clone()) {
        Ok(w) => Ok(wasapi_open(w, cpal::HostId::Wasapi.name(), inbox, outbox, errors)),
        Err(Refused::OwnRate(r)) => {
            let device = cfg.device.as_deref().unwrap_or("the default device");
            Err(Some(format!("Small buffers need {device}'s own rate ({r} Hz)")))
        }
        Err(_) => Err(None),
    }
}

/// The `Open` of a WASAPI stream: exclusive, its period is its blocks and its buffer (as an ASIO driver's); shared,
/// its buffer is the one the status shows.
#[cfg(windows)]
fn wasapi_open(w: wasapi::Opened, host: &'static str, inbox: Arc<Queue>, outbox: Arc<Queue>, errors: Errors) -> Open {
    let i = w.info;
    let fixed = Some(if host == WASAPI_EXCLUSIVE { i.period } else { i.buffer });
    let info = DeviceInfo { host, name: i.name, rate: i.rate, fixed, buffers: Some(i.range), channels: i.channels, inputs: 0, input_latency: None, rates: Vec::new(), sizes: Vec::new(), latency: i.latency, instead: None };
    Open { _stream: Stream::Wasapi(w.out), inbox, outbox, info, note: None, failed: errors.failed, fault: errors.fault }
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
    let info = DeviceInfo { host: ASIO, name: name.to_string(), rate: a.rate, fixed, buffers, channels: a.channels, inputs: a.inputs, input_latency: a.input_latency, rates: a.rates, sizes: a.sizes, latency: a.latency, instead: None };
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
    let info = DeviceInfo { host, name: out.name, rate: config.sample_rate, fixed, buffers, channels: config.channels, inputs: 0, input_latency: None, rates: Vec::new(), sizes: Vec::new(), latency: None, instead: None };
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

    /// The Buffer length menu's lengths per mode (spec: audio/wasapi-low-latency.md 4.3): only those that change
    /// something.
    #[test]
    fn buffer_lengths_per_mode() {
        // This PC's Realtek endpoint: 10 ms only in shared mode, 3 ms smallest in exclusive mode.
        let realtek = Periods { rate: 48000, shared: Some((480, 480, 480, 480)), shared_buffer: 1056, exclusive: (100_000, 30_000), exclusive_rates: vec![] };
        assert_eq!(realtek.small(), [] as [u32; 0]);
        assert_eq!(realtek.own(false, 48000), 480);
        assert_eq!(realtek.sizes(false, 48000), [2048, 4096, 8192], "only lengths over the buffer granted anyway");
        assert_eq!(realtek.exclusive_frames(48000), (480, 144));
        assert_eq!(realtek.exclusive_frames(44100), (441, 132));
        assert_eq!(realtek.sizes(true, 48000), [256, 512, 1024, 2048, 4096, 8192]);
        // Microsoft's HD Audio driver: 128 to 480 frames in steps of 32 at 48 kHz.
        let inbox = Periods { shared: Some((480, 32, 128, 480)), ..realtek.clone() };
        assert_eq!(inbox.small(), [128, 256]);
        assert_eq!(inbox.sizes(false, 48000), [128, 256, 2048, 4096, 8192]);
        // A device whose smallest is a length of the usual list: listed once, as "Smallest".
        let even = Periods { exclusive: (106_667, 26_667), ..realtek };
        assert_eq!(even.exclusive_frames(48000), (512, 128));
        assert_eq!(even.sizes(true, 48000), [256, 1024, 2048, 4096, 8192]);
    }

    /// Linux: shared mode plays through the sound server, exclusive mode on ALSA; once exclusive mode lets go, the
    /// sound server has the card again (`sound_server`). Needs a sound server and an audio card.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn linux_modes_and_the_card_given_back() {
        HEAL_IN_TESTS.store(true, Ordering::Relaxed);
        let xruns = Arc::new(AtomicU64::new(0));
        let o = open(&AudioCfg::default(), &xruns, false).expect("opens");
        assert_eq!(o.info.host, "PulseAudio", "shared mode: {:?}", o.info);
        drop(o);
        let mut d = OutputDevice::default();
        let cfg = AudioCfg::default().with(audio_cfg::Set::Mode(true));
        use cpal::traits::HostTrait;
        let card = |d: &cpal::Device| d.id().is_ok_and(|i| i.id().starts_with("hw:CARD="));
        let hw = audio_cfg::host(&cfg).output_devices().expect("ALSA's devices").find(card).map(|d| audio_cfg::device_name(&d)).expect("an audio card");
        AudioCfg { device: Some(hw), ..cfg }.apply();
        let (src, _, blocks) = level(0.0);
        // The sound server lets go of an idle card after some seconds (it is busy until then).
        let mut src = Some(src);
        let t0 = std::time::Instant::now();
        while let Err(e) = d.open() {
            assert!(t0.elapsed() < std::time::Duration::from_secs(15), "opens on ALSA: {e}");
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        d.add(src.take().expect("once")).expect("opens on ALSA");
        assert_eq!(d.open.as_ref().map(|o| o.info.host), Some(ALSA));
        // The sound server tries the card meanwhile (and fails: Lemon Studio has it).
        let busy = std::process::Command::new("pw-play").arg("--volume=0").arg("/usr/share/sounds/freedesktop/stereo/bell.oga").spawn();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        assert!(blocks.load(Ordering::Relaxed) > 0, "it plays");
        let _ = busy.map(|mut c| c.kill());
        d.close();
        AudioCfg::default().apply();
        std::thread::sleep(std::time::Duration::from_millis(1000));
        assert_eq!(sound_server::heal(), Vec::<String>::new(), "every card is back with the sound server");
    }

    /// What the Windows devices here offer for short buffers (`--nocapture` prints it). Needs an audio device.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn periods_of_the_windows_devices() {
        let (devices, _) = audio_cfg::devices(&AudioCfg::default());
        let p = periods(None).expect("the default device says");
        assert!(p.rate >= 8000 && p.exclusive.1 > 0 && p.exclusive.1 <= p.exclusive.0, "{p:?}");
        for d in devices {
            println!("{d}: {:?}", periods(Some(&d)));
        }
        assert_eq!(periods(Some("No Such Device")), None);
    }

    /// The first Windows device here that takes exclusive mode (Windows' sound settings allow it, `Periods`).
    #[cfg(windows)]
    fn exclusive_device() -> Option<String> {
        audio_cfg::devices(&AudioCfg::default()).0.into_iter().find(|d| periods(Some(d)).is_some_and(|p| !p.exclusive_rates.is_empty()))
    }

    /// Exclusive mode plays on a device that takes it, at its default period, its smallest or a length asked; while
    /// it has the device, a second exclusive open gives way to another device (or is an error while it waits). Needs
    /// a device that allows exclusive mode.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn exclusive_mode_plays_and_holds_the_device() {
        let Some(name) = exclusive_device() else { return eprintln!("no device here allows exclusive mode") };
        let p = periods(Some(&name)).expect("it says");
        let xruns = Arc::new(AtomicU64::new(0));
        let cfg = AudioCfg { device: Some(name.clone()), exclusive: true, ..AudioCfg::default() };
        for (buffer, frames) in [(None, p.exclusive_frames(p.rate).0), (Some(audio_cfg::SMALLEST), p.exclusive_frames(p.rate).1), (Some(1024), 1024)] {
            let o = open(&AudioCfg { buffer, ..cfg.clone() }, &xruns, false).expect("opens");
            assert_eq!((o.info.host, o.info.name.as_str(), o.info.instead.as_deref()), (WASAPI_EXCLUSIVE, name.as_str(), None));
            assert_eq!(o.info.fixed, Some(frames), "{buffer:?}");
            let (src, _, blocks) = level(0.0);
            assert!(o.inbox.push(src).is_ok());
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert!(blocks.load(Ordering::Relaxed) >= 3, "it plays");
            if buffer.is_none() {
                // Held: another exclusive open waits, or plays elsewhere in shared mode.
                let e = open(&cfg, &xruns, true).err().expect("in use");
                assert_eq!(e, format!("Couldn't open {name}: another program has it to itself"));
                match open(&cfg, &xruns, false) {
                    Ok(other) => {
                        assert!(other.info.host != WASAPI_EXCLUSIVE && other.info.name != name);
                        let want = format!("Couldn't open {name}: another program has it to itself; using {}", other.info.name);
                        assert_eq!(other.info.instead, Some(want));
                    }
                    Err(e) => assert!(e.contains("nor in shared mode"), "{e}"),
                }
            }
        }
    }

    /// The default device in exclusive mode: it plays so, or (Windows' sound settings do not allow it) plays in
    /// shared mode and says why. Needs an audio device.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn exclusive_mode_or_why_not() {
        let xruns = Arc::new(AtomicU64::new(0));
        let o = open(&AudioCfg { exclusive: true, ..AudioCfg::default() }, &xruns, false).expect("opens");
        if o.info.host == WASAPI_EXCLUSIVE {
            return;
        }
        let name = o.info.name.clone();
        let want = format!("Couldn't open {name} in exclusive mode: Windows' sound settings don't allow it for this device; using shared mode");
        assert_eq!(o.info.instead.as_deref(), Some(want.as_str()));
        assert_eq!(o.note.as_deref(), Some(want.as_str()), "told once");
    }
}
