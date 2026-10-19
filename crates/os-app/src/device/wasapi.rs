// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! WASAPI output of Lemon Studio's own (Windows), for the two modes cpal does not open (it plays plain shared mode,
//! `super::open_system`): exclusive mode, where Lemon Studio has the device to itself and the system's mixer is
//! bypassed, and the low-latency shared mode of IAudioClient3, where the system's mixer runs at a shorter period on
//! that device for every program.
//!
//! A stream plays the mix on a render thread of its own (in MMCSS's "Pro Audio" class), woken by Windows once per
//! period. The thread opens the device itself and says how that went before it plays; dropping the stream stops it.
//! Asking a device what it offers (`periods`) plays nothing, so any thread may ask.
// spec: audio/wasapi-low-latency.md

use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::{w, Interface};
use windows::Win32::Devices::Properties::DEVPKEY_Device_FriendlyName;
use windows::Win32::Foundation::{CloseHandle, HANDLE, PROPERTYKEY, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioClient3, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_DEVICE_IN_USE, AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED,
    AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
use windows::Win32::System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, SetEvent, WaitForMultipleObjects};

use super::{Errors, Mix, Periods};
use crate::audio_cfg::{RATES, SMALLEST};

/// How a stream shares its device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The device to Lemon Studio alone (event-driven exclusive mode).
    Exclusive,
    /// Shared, with the system's mixer at a period shorter than the device's default (IAudioClient3).
    Small,
}

/// Why a device did not open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// Windows' sound settings don't let programs take this device for themselves.
    NotAllowed,
    /// Another program has the device to itself.
    InUse,
    /// No such device (or it went away).
    Missing,
    /// `Mode::Small`: the device offers no such period.
    NotOffered,
    /// `Mode::Small`: the period needs the device's own rate (Hz).
    OwnRate(u32),
    Other(String),
}

impl Refused {
    /// Why, in a few words, for "Couldn't open <device>: <why>".
    pub(crate) fn why(&self) -> String {
        match self {
            Refused::NotAllowed => "Windows' sound settings don't allow it for this device".into(),
            Refused::InUse => "another program has it to itself".into(),
            Refused::Missing => "not found".into(),
            Refused::NotOffered => "it offers no such buffer length".into(),
            Refused::OwnRate(r) => format!("small buffers need its own rate ({r} Hz)"),
            Refused::Other(e) => e.clone(),
        }
    }

    fn of(e: &windows::core::Error) -> Refused {
        let code = e.code();
        if code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
            Refused::NotAllowed
        } else if code == AUDCLNT_E_DEVICE_IN_USE {
            Refused::InUse
        } else if code == AUDCLNT_E_DEVICE_INVALIDATED {
            Refused::Missing
        } else {
            Refused::Other(text(e))
        }
    }
}

/// A Windows error in a few words: its message, else its code.
fn text(e: &windows::core::Error) -> String {
    let m = e.message();
    let m = m.trim().trim_end_matches('.');
    if m.is_empty() {
        format!("error {:#010x}", e.code().0 as u32)
    } else {
        m.to_string()
    }
}

/// How the device takes its samples (little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    F32,
    I32,
    /// 24 bits in a 32-bit word, left-justified (its low byte zero).
    I24In32,
    /// 24 bits in 3 bytes.
    I24,
    I16,
}

impl Format {
    /// The formats tried in exclusive mode, in this order (the first the device takes).
    const ALL: [Format; 5] = [Format::F32, Format::I32, Format::I24In32, Format::I24, Format::I16];

    fn bytes(self) -> usize {
        match self {
            Format::F32 | Format::I32 | Format::I24In32 => 4,
            Format::I24 => 3,
            Format::I16 => 2,
        }
    }

    fn bits(self) -> u16 {
        match self {
            Format::F32 | Format::I32 => 32,
            Format::I24In32 | Format::I24 => 24,
            Format::I16 => 16,
        }
    }

    /// The format `w` describes, if Lemon Studio writes it.
    fn of(w: &WAVEFORMATEXTENSIBLE) -> Option<Format> {
        let f = w.Format;
        let (tag, bits) = (u32::from(f.wFormatTag), f.wBitsPerSample);
        let (float, valid) = if tag == WAVE_FORMAT_EXTENSIBLE {
            let sub = w.SubFormat;
            // Safety: every member of the union is a u16.
            let valid = unsafe { w.Samples.wValidBitsPerSample };
            if sub != KSDATAFORMAT_SUBTYPE_IEEE_FLOAT && sub != KSDATAFORMAT_SUBTYPE_PCM {
                return None;
            }
            (sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, if valid == 0 { bits } else { valid })
        } else {
            (tag == WAVE_FORMAT_IEEE_FLOAT, bits)
        };
        Some(match (float, bits, valid) {
            (true, 32, _) => Format::F32,
            (false, 32, 32) => Format::I32,
            (false, 32, 24) => Format::I24In32,
            (false, 24, _) => Format::I24,
            (false, 16, _) => Format::I16,
            _ => return None,
        })
    }
}

/// A WAVEFORMATEXTENSIBLE for `format` at `rate`, on `channels` channels placed as `mask` says.
fn wave(format: Format, rate: u32, channels: u16, mask: u32) -> WAVEFORMATEXTENSIBLE {
    let block = format.bytes() as u16 * channels;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: format.bytes() as u16 * 8,
            cbSize: 22,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: format.bits() },
        dwChannelMask: mask,
        SubFormat: if format == Format::F32 { KSDATAFORMAT_SUBTYPE_IEEE_FLOAT } else { KSDATAFORMAT_SUBTYPE_PCM },
    }
}

/// One sample (clipped to ±1) as the device takes it, into `out` (`format.bytes()` long).
fn encode(x: f32, format: Format, out: &mut [u8]) {
    let x = f64::from(x.clamp(-1.0, 1.0));
    let int = |bits: u32| (x * ((1i64 << (bits - 1)) - 1) as f64).round() as i32;
    match format {
        Format::F32 => out.copy_from_slice(&(x as f32).to_le_bytes()),
        Format::I32 => out.copy_from_slice(&int(32).to_le_bytes()),
        Format::I24In32 => out.copy_from_slice(&(int(24) << 8).to_le_bytes()),
        Format::I24 => out.copy_from_slice(&int(24).to_le_bytes()[..3]),
        Format::I16 => out.copy_from_slice(&(int(16) as i16).to_le_bytes()),
    }
}

/// Writes the interleaved stereo `mix` into a device buffer of `channels` channels: left and right on the first two (a
/// mono device gets the left), silence on the others.
fn write(out: &mut [u8], mix: &[f32], channels: usize, format: Format) {
    let n = format.bytes();
    for (frame, s) in out.chunks_exact_mut(n * channels).zip(mix.chunks_exact(2)) {
        for (c, slot) in frame.chunks_exact_mut(n).enumerate() {
            encode(if c < 2 { s[c] } else { 0.0 }, format, slot);
        }
    }
}

/// Frames at `rate` in a duration of `hns` (100-nanosecond units, as WASAPI counts), and back.
fn frames(hns: i64, rate: u32) -> u32 {
    (hns as f64 * f64::from(rate) / 1e7).round() as u32
}

fn hns(frames: u32, rate: u32) -> i64 {
    (f64::from(frames) * 1e7 / f64::from(rate.max(1))).round() as i64
}

/// COM on the calling thread for as long as this lives (multithreaded: WASAPI's objects need no window messages).
struct Com(bool);

impl Com {
    fn init() -> Com {
        // Safety: undone by `drop` on the same thread, only if it succeeded.
        Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// A Windows event, closed when dropped.
struct Event(HANDLE);

// Safety: an event handle may be waited on and set from any thread.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl Event {
    fn new(manual: bool) -> Result<Event, Refused> {
        unsafe { CreateEventW(None, manual, false, None) }.map(Event).map_err(|e| Refused::Other(text(&e)))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// The output device named `name` (`None`: the system's default, for music and games), and its name.
fn device(name: Option<&str>) -> Result<(IMMDevice, String), Refused> {
    unsafe {
        let all: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| Refused::Other(text(&e)))?;
        let Some(want) = name else {
            let d = all.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|_| Refused::Missing)?;
            let n = friendly(&d).unwrap_or_default();
            return Ok((d, n));
        };
        let list = all.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).map_err(|e| Refused::Other(text(&e)))?;
        for i in 0..list.GetCount().unwrap_or(0) {
            if let Ok(d) = list.Item(i) {
                if friendly(&d).as_deref() == Some(want) {
                    return Ok((d, want.to_string()));
                }
            }
        }
        Err(Refused::Missing)
    }
}

/// A device's name as cpal gives it (and the settings keep it): its friendly name.
fn friendly(d: &IMMDevice) -> Option<String> {
    unsafe {
        let store = d.OpenPropertyStore(STGM_READ).ok()?;
        let key = PROPERTYKEY { fmtid: DEVPKEY_Device_FriendlyName.fmtid, pid: DEVPKEY_Device_FriendlyName.pid };
        let mut v = store.GetValue(&key).ok()?;
        let s = PropVariantToStringAlloc(&v).ok();
        let _ = PropVariantClear(&mut v);
        let s = s?;
        let name = s.to_string().ok();
        CoTaskMemFree(Some(s.0 as *const _));
        name
    }
}

/// The format the system's mixer runs the device in (its rate is the device's own).
struct MixFormat {
    wave: WAVEFORMATEXTENSIBLE,
    rate: u32,
    channels: u16,
    mask: u32,
    format: Option<Format>,
}

fn mix_format(c: &IAudioClient) -> Result<MixFormat, Refused> {
    unsafe {
        let p = c.GetMixFormat().map_err(|e| Refused::of(&e))?;
        let f = *p;
        let mut wave = WAVEFORMATEXTENSIBLE::default();
        let extensible = u32::from(f.wFormatTag) == WAVE_FORMAT_EXTENSIBLE && f.cbSize >= 22;
        if extensible {
            wave = *(p as *const WAVEFORMATEXTENSIBLE);
        } else {
            wave.Format = WAVEFORMATEX { cbSize: 0, ..f };
        }
        CoTaskMemFree(Some(p as *const _));
        // Without a mask: the first channels as Windows places them (front left, front right, ...).
        let mask = if extensible { wave.dwChannelMask } else { (1u32 << f.nChannels.min(31)) - 1 };
        Ok(MixFormat { format: Format::of(&wave), rate: f.nSamplesPerSec, channels: f.nChannels, mask, wave })
    }
}

/// What the device named `name` (`None`: the default) offers for small buffers, asked on a thread of its own (so any
/// thread may call this). `None`: no such device, or Windows does not say.
pub(crate) fn periods(name: Option<&str>) -> Option<Periods> {
    let name = name.map(str::to_string);
    std::thread::spawn(move || {
        let _com = Com::init();
        ask(name.as_deref())
    })
    .join()
    .ok()
    .flatten()
}

fn ask(name: Option<&str>) -> Option<Periods> {
    unsafe {
        let (d, _) = device(name).ok()?;
        let c: IAudioClient = d.Activate(CLSCTX_ALL, None).ok()?;
        let m = mix_format(&c).ok()?;
        // The buffer a plain shared stream gets when it asks for no length (as cpal opens it).
        let plain = d.Activate::<IAudioClient>(CLSCTX_ALL, None).ok().and_then(|p| {
            p.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, 0, 0, &m.wave.Format, None).ok()?;
            p.GetBufferSize().ok()
        });
        let shared = d.Activate::<IAudioClient3>(CLSCTX_ALL, None).ok().and_then(|c3| {
            let (mut def, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
            c3.GetSharedModeEnginePeriod(&m.wave.Format, &mut def, &mut fundamental, &mut min, &mut max).ok()?;
            Some((def, fundamental, min, max))
        });
        let (mut def, mut min) = (0i64, 0i64);
        c.GetDevicePeriod(Some(&mut def), Some(&mut min)).ok()?;
        // The rates it takes in exclusive mode in some format (none while exclusive mode is refused).
        let takes = |r: u32| Format::ALL.iter().any(|&f| c.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, &wave(f, r, m.channels, m.mask).Format, None).0 == 0);
        let exclusive_rates = RATES.into_iter().filter(|&r| takes(r)).collect();
        Some(Periods { rate: m.rate, shared, shared_buffer: plain.unwrap_or(0), exclusive: (def, min), exclusive_rates })
    }
}

/// The open stream; dropping it stops the render thread (the mix, and the sources in it, go with it).
pub(crate) struct WasapiOut {
    stop: Arc<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for WasapiOut {
    fn drop(&mut self) {
        let _ = unsafe { SetEvent(self.stop.0) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// What a stream opened as.
pub(crate) struct Opened {
    pub(crate) out: WasapiOut,
    pub(crate) info: Info,
}

#[derive(Clone, Debug)]
pub(crate) struct Info {
    pub(crate) name: String,
    pub(crate) rate: u32,
    /// The frames it is woken for (exclusive: the whole buffer), and its buffer.
    pub(crate) period: u32,
    pub(crate) buffer: u32,
    pub(crate) channels: u16,
    /// The periods its mode takes, frames.
    pub(crate) range: (u32, u32),
    /// Its latency as Windows says (`GetStreamLatency`), frames.
    pub(crate) latency: Option<u32>,
}

/// Opens the device named `name` (`None`: the default) in `mode` and plays `mix` on it.
/// - Exclusive: at `rate` when the device takes it (else its own), in the first of `Format::ALL` it takes, for
///   `buffer` frames per period (`None`: its default period; `SMALLEST`: its smallest; shorter than that: its
///   smallest; a length the driver wants aligned: the one it names).
/// - Small: at the period `buffer` when the device offers it below its default (`Refused::NotOffered`), at its own
///   rate (`Refused::OwnRate`).
///
/// Lost blocks and a stream that stops go to `errors`.
pub(crate) fn open(name: Option<&str>, mode: Mode, rate: Option<u32>, buffer: Option<u32>, mix: Mix, errors: Errors) -> Result<Opened, Refused> {
    let stop = Arc::new(Event::new(true)?);
    let (tx, rx) = mpsc::channel();
    let name = name.map(str::to_string);
    let thread = std::thread::Builder::new()
        .name("lemonstudio wasapi".into())
        .spawn({
            let stop = stop.clone();
            move || render(name.as_deref(), mode, rate, buffer, mix, errors, &stop, tx)
        })
        .map_err(|e| Refused::Other(e.to_string()))?;
    match rx.recv() {
        Ok(Ok(info)) => Ok(Opened { out: WasapiOut { stop, thread: Some(thread) }, info }),
        Ok(Err(r)) => {
            let _ = thread.join();
            Err(r)
        }
        Err(_) => {
            let _ = thread.join();
            Err(Refused::Other("the output thread ended".into()))
        }
    }
}

/// The render thread: opens the stream, says how that went (`tx`), then fills a period each time Windows asks until
/// `stop` is set or the stream fails.
#[allow(clippy::too_many_arguments)]
fn render(name: Option<&str>, mode: Mode, rate: Option<u32>, buffer: Option<u32>, mut mix: Mix, errors: Errors, stop: &Event, tx: mpsc::Sender<Result<Info, Refused>>) {
    let _com = Com::init();
    let s = match start(name, mode, rate, buffer) {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(Err(e));
            return;
        }
    };
    if let Err(e) = unsafe { s.client.Start() } {
        let _ = tx.send(Err(Refused::of(&e)));
        return;
    }
    let mut task = 0;
    let mmcss = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) }.ok();
    let _ = tx.send(Ok(s.info.clone()));
    // A wake this long after the one before is a lost block (WASAPI reports none for output).
    let late = Duration::from_secs_f64(1.5 * f64::from(s.info.period) / f64::from(s.info.rate.max(1)));
    let mut last = Instant::now();
    loop {
        let woke = unsafe { WaitForMultipleObjects(&[s.event.0, stop.0], false, 2000) };
        if woke == WAIT_OBJECT_0 {
            let now = Instant::now();
            if s.exclusive && now.duration_since(last) > late {
                errors.xrun();
            }
            last = now;
            if let Err(e) = s.fill(&mut mix, &errors) {
                let why = if e.code() == AUDCLNT_E_DEVICE_INVALIDATED { "it went away, or another program took it for itself".into() } else { text(&e) };
                errors.fail(why);
                break;
            }
        } else if woke.0 == WAIT_OBJECT_0.0 + 1 {
            break;
        } else {
            errors.fail("it stopped asking for sound".into());
            break;
        }
    }
    let _ = unsafe { s.client.Stop() };
    if let Some(h) = mmcss {
        let _ = unsafe { AvRevertMmThreadCharacteristics(h) };
    }
}

/// A stream that is ready to start (its buffer filled with silence).
struct Started {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: Event,
    format: Format,
    channels: usize,
    exclusive: bool,
    info: Info,
}

impl Started {
    /// Fills what the device has room for: exclusive, a whole period; shared, the buffer's free part.
    fn fill(&self, mix: &mut Mix, errors: &Errors) -> windows::core::Result<()> {
        unsafe {
            let frames = if self.exclusive {
                self.info.buffer
            } else {
                let queued = self.client.GetCurrentPadding()?;
                // Played out before it was filled again.
                if queued == 0 {
                    errors.xrun();
                }
                self.info.buffer.saturating_sub(queued)
            };
            if frames == 0 {
                return Ok(());
            }
            let p = self.render.GetBuffer(frames)?;
            // Safety: Windows lends `frames` frames of the stream's format at `p` until `ReleaseBuffer`.
            let out = std::slice::from_raw_parts_mut(p, frames as usize * self.channels * self.format.bytes());
            write(out, mix.block(frames as usize), self.channels, self.format);
            self.render.ReleaseBuffer(frames, 0)
        }
    }
}

fn start(name: Option<&str>, mode: Mode, rate: Option<u32>, buffer: Option<u32>) -> Result<Started, Refused> {
    let (d, name) = device(name)?;
    let activate = || unsafe { d.Activate::<IAudioClient>(CLSCTX_ALL, None) }.map_err(|e| Refused::of(&e));
    let first = activate()?;
    let m = mix_format(&first)?;
    let (client, format, rate, range) = match mode {
        Mode::Exclusive => exclusive(first, &activate, &m, rate, buffer)?,
        Mode::Small => small(&d, &m, rate, buffer)?,
    };
    unsafe {
        let event = Event::new(false)?;
        client.SetEventHandle(event.0).map_err(|e| Refused::of(&e))?;
        let render: IAudioRenderClient = client.GetService().map_err(|e| Refused::of(&e))?;
        let size = client.GetBufferSize().map_err(|e| Refused::of(&e))?;
        // Silence until the first wake (exclusive mode asks for a full buffer before it starts).
        render.GetBuffer(size).map_err(|e| Refused::of(&e))?;
        render.ReleaseBuffer(size, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32).map_err(|e| Refused::of(&e))?;
        let latency = client.GetStreamLatency().ok().filter(|&l| l > 0).map(|l| frames(l, rate));
        let exclusive = mode == Mode::Exclusive;
        let period = if exclusive { size } else { buffer.unwrap_or(size) };
        let info = Info { name, rate, period, buffer: size, channels: m.channels, range, latency };
        Ok(Started { client, render, event, format, channels: usize::from(m.channels), exclusive, info })
    }
}

/// Exclusive mode on `client` (`activate` makes another for the same device): the rate, format and period as `open`
/// says, and the periods it takes.
fn exclusive(
    client: IAudioClient,
    activate: &dyn Fn() -> Result<IAudioClient, Refused>,
    m: &MixFormat,
    rate: Option<u32>,
    buffer: Option<u32>,
) -> Result<(IAudioClient, Format, u32, (u32, u32)), Refused> {
    let pick = |r: u32| -> Result<Option<Format>, Refused> {
        for f in Format::ALL {
            let hr = unsafe { client.IsFormatSupported(AUDCLNT_SHAREMODE_EXCLUSIVE, &wave(f, r, m.channels, m.mask).Format, None) };
            if hr.0 == 0 {
                return Ok(Some(f));
            } else if hr == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
                return Err(Refused::NotAllowed);
            } else if hr == AUDCLNT_E_DEVICE_IN_USE {
                return Err(Refused::InUse);
            }
        }
        Ok(None)
    };
    let want = rate.unwrap_or(m.rate);
    let (rate, format) = match pick(want)? {
        Some(f) => (want, f),
        // A rate the device does not run at itself: its own.
        None => match pick(m.rate)? {
            Some(f) => (m.rate, f),
            None => return Err(Refused::Other("it takes none of the sample formats Lemon Studio writes".into())),
        },
    };
    let w = wave(format, rate, m.channels, m.mask);
    let (mut def, mut min) = (0i64, 0i64);
    unsafe { client.GetDevicePeriod(Some(&mut def), Some(&mut min)) }.map_err(|e| Refused::of(&e))?;
    let period = match buffer {
        None => def,
        Some(SMALLEST) => min,
        Some(b) => hns(b, rate).max(min),
    };
    let init = |c: &IAudioClient, p: i64| unsafe { c.Initialize(AUDCLNT_SHAREMODE_EXCLUSIVE, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, p, p, &w.Format, None) };
    let client = match init(&client, period) {
        Ok(()) => client,
        // The driver wants another length: the one it names, on a new client (spec 1.3).
        Err(e) if e.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
            let aligned = unsafe { client.GetBufferSize() }.map_err(|e| Refused::of(&e))?;
            drop(client);
            let c = activate()?;
            init(&c, hns(aligned, rate)).map_err(|e| Refused::of(&e))?;
            c
        }
        Err(e) => return Err(Refused::of(&e)),
    };
    let largest = *crate::audio_cfg::BUFFERS.last().expect("buffer lengths");
    Ok((client, format, rate, (frames(min, rate), largest.max(frames(def, rate)))))
}

/// Low-latency shared mode: the period `buffer` when the device offers it, at its own rate.
fn small(d: &IMMDevice, m: &MixFormat, rate: Option<u32>, buffer: Option<u32>) -> Result<(IAudioClient, Format, u32, (u32, u32)), Refused> {
    unsafe {
        let c3: IAudioClient3 = d.Activate(CLSCTX_ALL, None).map_err(|e| Refused::of(&e))?;
        let (mut def, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
        c3.GetSharedModeEnginePeriod(&m.wave.Format, &mut def, &mut fundamental, &mut min, &mut max).map_err(|_| Refused::NotOffered)?;
        let offered = Periods { shared: Some((def, fundamental, min, max)), ..Default::default() }.small();
        let period = buffer.filter(|b| offered.contains(b)).ok_or(Refused::NotOffered)?;
        if rate.is_some_and(|r| r != m.rate) {
            return Err(Refused::OwnRate(m.rate));
        }
        let format = m.format.ok_or_else(|| Refused::Other("the system mixer's sample format is not one Lemon Studio writes".into()))?;
        c3.InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period, &m.wave.Format, None).map_err(|e| Refused::of(&e))?;
        let client: IAudioClient = c3.cast().map_err(|e| Refused::Other(text(&e)))?;
        Ok((client, format, m.rate, (min, def)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(x: f32, f: Format) -> Vec<u8> {
        let mut out = vec![0; f.bytes()];
        encode(x, f, &mut out);
        out
    }

    /// Each sample format, full scale and clipped; 24 bits in a 32-bit word sit in its high bits.
    #[test]
    fn samples_are_written_as_the_device_takes_them() {
        assert_eq!(bytes(0.25, Format::F32), 0.25f32.to_le_bytes());
        assert_eq!(bytes(2.0, Format::F32), 1.0f32.to_le_bytes(), "clipped");
        assert_eq!(bytes(1.0, Format::I32), i32::MAX.to_le_bytes());
        assert_eq!(bytes(-1.0, Format::I32), (-i32::MAX).to_le_bytes());
        assert_eq!(bytes(1.0, Format::I24In32), (0x7f_ffff_i32 << 8).to_le_bytes());
        assert_eq!(bytes(-1.0, Format::I24In32), (-0x7f_ffff_i32 << 8).to_le_bytes());
        assert_eq!(bytes(0.5, Format::I24), [0x00, 0x00, 0x40]);
        assert_eq!(bytes(-1.0, Format::I24), [0x01, 0x00, 0x80]);
        assert_eq!(bytes(1.0, Format::I16), [0xff, 0x7f]);
        assert_eq!(bytes(-4.0, Format::I16), [0x01, 0x80], "clipped");
    }

    /// Left and right on the first two channels, silence on the others; a mono device gets the left.
    #[test]
    fn channels_get_their_side() {
        let mix = [0.5, -0.5, 0.25, 0.75];
        let mut out = [0u8; 2 * 4 * 4];
        write(&mut out, &mix, 4, Format::F32);
        let at = |i: usize| f32::from_le_bytes(out[4 * i..4 * i + 4].try_into().unwrap());
        assert_eq!((0..8).map(at).collect::<Vec<_>>(), [0.5, -0.5, 0.0, 0.0, 0.25, 0.75, 0.0, 0.0]);
        let mut mono = [0u8; 2 * 2];
        write(&mut mono, &mix, 1, Format::I16);
        assert_eq!(mono, [0x00, 0x40, 0x00, 0x20]);
    }

    /// The formats Lemon Studio asks for read back as themselves; formats it does not write are not taken.
    #[test]
    fn a_format_reads_back() {
        for f in Format::ALL {
            let w = wave(f, 48000, 2, 3);
            assert_eq!(Format::of(&w), Some(f));
            let (block, avg) = (w.Format.nBlockAlign, w.Format.nAvgBytesPerSec);
            assert_eq!((usize::from(block), avg), (2 * f.bytes(), 48000 * 2 * f.bytes() as u32));
        }
        let mut odd = wave(Format::I16, 48000, 2, 3);
        odd.Format.wBitsPerSample = 8;
        assert_eq!(Format::of(&odd), None, "8-bit");
        let plain = WAVEFORMATEXTENSIBLE { Format: WAVEFORMATEX { wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16, wBitsPerSample: 32, ..Default::default() }, ..Default::default() };
        assert_eq!(Format::of(&plain), Some(Format::F32), "a plain float format");
    }

    /// Durations as WASAPI counts them, in frames and back.
    #[test]
    fn periods_in_frames() {
        assert_eq!(frames(30_000, 48000), 144, "3 ms");
        assert_eq!(frames(100_000, 44100), 441, "10 ms");
        assert_eq!(hns(144, 48000), 30_000);
        assert_eq!(frames(hns(132, 44100), 44100), 132);
    }

    /// What the status says when a device does not open.
    #[test]
    fn refusals_say_why() {
        assert_eq!(Refused::of(&AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED.into()), Refused::NotAllowed);
        assert_eq!(Refused::of(&AUDCLNT_E_DEVICE_IN_USE.into()), Refused::InUse);
        assert_eq!(Refused::NotAllowed.why(), "Windows' sound settings don't allow it for this device");
        assert_eq!(Refused::OwnRate(48000).why(), "small buffers need its own rate (48000 Hz)");
        assert!(matches!(Refused::of(&AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED.into()), Refused::Other(_)));
    }
}
