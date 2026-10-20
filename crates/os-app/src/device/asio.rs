// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! ASIO output (Windows): the chosen driver plays the mix on its first two outputs, at the buffer length chosen on the
//! Audio page when it takes it, else the one set in its own panel (`control_panel`). Its first inputs open with the
//! outputs (full duplex): the same callback hands them to the capture while One-click audio recording records
//! (`crate::capture`; spec: ui/one-click-recording.md §2).
//!
//! The driver is loaded by name through asio-sys, which builds the ASIO SDK's host files (`asiosdk`). cpal's ASIO host
//! is not used: it finds a driver by loading every driver listed before it, and one whose hardware is missing may
//! show an error box for it. A driver is loaded, used and unloaded on the audio thread only; listing them (`drivers`)
//! reads the registry and loads none.
// spec: audio/devices.md

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use asio_sys as sys;

use super::{Errors, Mix};

static ASIO: OnceLock<sys::Asio> = OnceLock::new();

fn asio() -> &'static sys::Asio {
    ASIO.get_or_init(sys::Asio::new)
}

/// The installed drivers' names, in the registry's order (none is loaded: any thread may ask).
pub fn drivers() -> Vec<String> {
    asio().driver_names()
}

/// The open driver, playing the mix; dropping it stops the driver and unloads it.
pub(crate) struct AsioOut {
    driver: sys::Driver,
    callback: sys::BufferCallbackId,
    events: sys::DriverEventCallbackId,
}

impl Drop for AsioOut {
    fn drop(&mut self) {
        let _ = self.driver.stop();
        // The mix, and the sources in it, go with the callback (on this thread: the driver no longer calls it).
        self.driver.remove_callback(self.callback);
        self.driver.remove_event_callback(self.events);
        let _ = self.driver.dispose_buffers();
        // Then `driver` itself: the driver exits and is unloaded.
    }
}

/// What a driver opened as.
pub(crate) struct Opened {
    pub(crate) out: AsioOut,
    pub(crate) rate: u32,
    /// Its buffer length, and the lengths it takes.
    pub(crate) frames: u32,
    pub(crate) buffers: (u32, u32),
    pub(crate) channels: u16,
    /// Inputs open (0: none, or the driver refused them).
    pub(crate) inputs: u16,
    /// The rates of `audio_cfg::RATES` and the lengths of `audio_cfg::BUFFERS` it takes.
    pub(crate) rates: Vec<u32>,
    pub(crate) sizes: Vec<u32>,
    /// Its output and input latencies, frames (`ASIOGetLatencies`).
    pub(crate) latency: Option<u32>,
    pub(crate) input_latency: Option<u32>,
}

/// The inputs opened at most: the ones the menu lists.
const MAX_INPUTS: i32 = 8;

/// Loads driver `name` and plays `mix` on it: at `rate` when the driver takes it (else its own), at `buffer` frames
/// when it takes that (else its own, the length set in its panel). Overloads and requests to be reset (a buffer length
/// changed in its panel) go to `errors`. An error says why in a few words, for "Couldn't open <driver>: <why>".
pub(crate) fn open(name: &str, rate: Option<u32>, buffer: Option<u32>, mut mix: Mix, errors: Errors) -> Result<Opened, String> {
    if !drivers().iter().any(|d| d == name) {
        return Err("not installed".into());
    }
    let driver = asio().load_driver(name).map_err(|e| match e {
        sys::LoadDriverError::LoadDriverFailed => "in use by another program, or its device is missing".to_string(),
        sys::LoadDriverError::DriverAlreadyExists => "another ASIO driver is open".to_string(),
        sys::LoadDriverError::InitializationFailed(e) => format!("the driver did not start ({e})"),
    })?;
    let rates = crate::audio_cfg::RATES.into_iter().filter(|&r| driver.can_sample_rate(f64::from(r)).unwrap_or(false)).collect();
    let now = driver.sample_rate().unwrap_or(0.0);
    if let Some(r) = rate.map(f64::from).filter(|&r| (now - r).abs() >= 1.0) {
        if driver.can_sample_rate(r).unwrap_or(false) {
            let _ = driver.set_sample_rate(r);
        }
    }
    let mut now = driver.sample_rate().map_err(|e| e.to_string())?;
    if now < 1.0 {
        // Some drivers know no rate until one is set.
        let _ = driver.set_sample_rate(f64::from(rate.unwrap_or(48000)));
        now = driver.sample_rate().map_err(|e| e.to_string())?;
    }
    if now < 1.0 {
        return Err("the driver has no sample rate".into());
    }
    let counts = driver.channels().map_err(|e| e.to_string())?;
    let outs = counts.outs;
    if outs < 1 {
        return Err("the driver has no outputs".into());
    }
    let channels = outs.min(2) as usize;
    let ty = driver.output_data_type().map_err(|e| e.to_string())?;
    let (format, big) = Format::of(&ty).ok_or_else(|| format!("the driver's sample format ({ty:?}) is not one Lemon Studio writes"))?;
    let range = driver.buffersize_range().map_err(|e| e.to_string())?;
    let sizes = sizes(&range);
    // A length it refuses after all: its own.
    let ask = buffer.filter(|b| sizes.contains(b)).map(|b| b as i32);
    // Its inputs with its outputs, else its outputs alone (a driver that refuses its inputs, or has none, plays on).
    let ins = counts.ins.clamp(0, MAX_INPUTS) as usize;
    let in_format = driver.input_data_type().ok().and_then(|t| Format::of(&t));
    let prepare = |ins: usize, ask: Option<i32>| {
        if ins == 0 {
            return driver.prepare_output_stream(None, channels, ask);
        }
        let s = driver.prepare_input_stream(None, ins, ask)?;
        driver.prepare_output_stream(s.input, channels, ask)
    };
    let tries = [(ins, ask), (ins, None), (0, ask), (0, None)];
    let mut streams = Err(sys::AsioError::NoDrivers);
    for (i, a) in tries.into_iter().filter(|&(i, _)| i == 0 || in_format.is_some()) {
        streams = prepare(i, a);
        if streams.is_ok() {
            break;
        }
    }
    let streams = streams.map_err(|e| e.to_string())?;
    let input = streams.input.filter(|i| !i.buffer_infos.is_empty());
    let stream = streams.output.ok_or("the driver made no output buffers")?;
    let in_halves = Halves(input.as_ref().map_or(Vec::new(), |i| i.buffer_infos.iter().map(|b| b.buffers.map(|p| p as usize)).collect()));
    let inputs = in_halves.0.len() as u16;
    let (in_format, in_big) = in_format.unwrap_or((Format::F32, false));
    let frames = stream.buffer_size.max(1) as usize;
    // Each channel's two halves (the driver plays one while the callback fills the other).
    let halves = Halves(stream.buffer_infos.iter().map(|b| b.buffers.map(|p| p as usize)).collect());
    let callback = driver.add_callback(move |info| {
        let sum = mix.block(frames);
        let half = (info.buffer_index & 1) as usize;
        // One output: both sides (`Side::Both`).
        let mono = halves.0.len() == 1;
        for (ch, b) in halves.0.iter().enumerate() {
            let side = if mono { Side::Both } else { Side::One(ch) };
            // Safety: the driver's buffer holds `frames` samples of `format` until the buffers are disposed of, which
            // only happens after this callback is removed (`AsioOut::drop`).
            unsafe { write(b[half] as *mut u8, sum, side, frames, format, big) };
        }
        // The inputs of the same block, while One-click audio recording records.
        let cap = crate::capture::capture();
        if cap.on() && !in_halves.0.is_empty() {
            let last = in_halves.0.len() - 1;
            let n = in_format.bytes();
            cap.push(frames, |ch, f| {
                let p = in_halves.0[ch.min(last)][half] as *const u8;
                // Safety: the driver's input buffer holds `frames` samples of `in_format` while this callback runs.
                let b = unsafe { std::slice::from_raw_parts(p.add(f * n), n) };
                decode(b, in_format, in_big)
            });
        }
    });
    // Drivers may ask to be reset while they start; that is not a reason to open them anew.
    let started = Arc::new(AtomicBool::new(false));
    let events = driver.add_event_callback({
        let started = started.clone();
        move |e| {
            use sys::AsioMessageSelectors::*;
            let on = started.load(Ordering::Acquire);
            match e {
                // The messages this host takes besides the ones asio-sys answers for itself.
                sys::AsioDriverEvent::Message { selector: kAsioSelectorSupported, value } => value == kAsioOverload as i32,
                sys::AsioDriverEvent::Message { selector: kAsioResetRequest, .. } if on => {
                    errors.fail("the driver asked to be reset".into());
                    true
                }
                sys::AsioDriverEvent::Message { selector: kAsioResyncRequest, .. } if on => {
                    errors.fail("the driver lost its sync".into());
                    true
                }
                sys::AsioDriverEvent::Message { selector: kAsioOverload, .. } => {
                    errors.xrun();
                    true
                }
                sys::AsioDriverEvent::SampleRateChanged(r) if on && (r - now).abs() >= 1.0 => {
                    errors.fail(format!("the driver changed to {r} Hz"));
                    false
                }
                _ => false,
            }
        }
    });
    let out = AsioOut { driver, callback, events };
    out.driver.start().map_err(|e| e.to_string())?;
    started.store(true, Ordering::Release);
    let buffers = (range.min.max(1) as u32, range.max.max(1) as u32);
    let lat = out.driver.latencies().ok();
    let latency = lat.as_ref().map(|l| l.output).filter(|&l| l > 0).map(|l| l as u32);
    let input_latency = lat.map(|l| l.input).filter(|&l| l > 0 && inputs > 0).map(|l| l as u32);
    Ok(Opened { out, rate: now.round() as u32, frames: frames as u32, buffers, channels: channels as u16, inputs, rates, sizes, latency, input_latency })
}

/// The lengths of `audio_cfg::BUFFERS` a driver takes: within its range, and a power of two or a whole number of steps
/// past its smallest as it says (`ASIOGetBufferSize`'s granularity; asio-sys gives -1, powers of two, as `Only`). A
/// driver with one length (its smallest is its largest, or no granularity): none, its own being the one.
fn sizes(r: &sys::BufferSizeRange) -> Vec<u32> {
    let (min, max) = (r.min.max(1) as u32, r.max.max(1) as u32);
    let takes = |b: u32| match r.preferred {
        sys::BufferPreference::Only(_) => b.is_power_of_two(),
        sys::BufferPreference::Preferred(_) => false,
        sys::BufferPreference::Stepped { step, .. } => (b - min) % step.max(1) == 0,
    };
    if min >= max {
        return Vec::new();
    }
    crate::audio_cfg::BUFFERS.into_iter().filter(|&b| (min..=max).contains(&b) && takes(b)).collect()
}

/// Opens the open driver's own settings window (the driver's panel: its buffer length, its device). A buffer length
/// changed there comes back as a request to be reset, which opens the driver anew.
pub(crate) fn control_panel() -> Result<(), String> {
    unsafe extern "C" {
        // `ASIOError ASIOControlPanel(void)` in the SDK's common/asio.cpp, which asio-sys builds and links; it has C++
        // linkage, hence the MSVC name.
        #[link_name = "\u{1}?ASIOControlPanel@@YAJXZ"]
        fn asio_control_panel() -> i32;
    }
    if asio().loaded_driver().is_none() {
        return Err("no ASIO driver is open".into());
    }
    // Safety: a driver is loaded (on this thread, the one that drives it).
    match unsafe { asio_control_panel() } {
        0 => Ok(()),
        -1000 => Err("the driver has no panel".into()),
        e => Err(format!("the driver's panel did not open (error {e})")),
    }
}

/// The driver's buffer halves per channel, as addresses: only the callback writes to them.
struct Halves(Vec<[usize; 2]>);

/// How a driver takes its samples (`ASIOSampleType`): an integer of `bits` in a word of `bytes`, or a float; each
/// either way round (`big`: most significant byte first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Int { bytes: usize, bits: u32 },
    F32,
    F64,
}

impl Format {
    fn of(t: &sys::AsioSampleType) -> Option<(Format, bool)> {
        use sys::AsioSampleType::*;
        let int = |bytes, bits| Format::Int { bytes, bits };
        Some(match t {
            ASIOSTInt16LSB => (int(2, 16), false),
            ASIOSTInt24LSB => (int(3, 24), false),
            ASIOSTInt32LSB => (int(4, 32), false),
            ASIOSTInt32LSB16 => (int(4, 16), false),
            ASIOSTInt32LSB18 => (int(4, 18), false),
            ASIOSTInt32LSB20 => (int(4, 20), false),
            ASIOSTInt32LSB24 => (int(4, 24), false),
            ASIOSTFloat32LSB => (Format::F32, false),
            ASIOSTFloat64LSB => (Format::F64, false),
            ASIOSTInt16MSB => (int(2, 16), true),
            ASIOSTInt24MSB => (int(3, 24), true),
            ASIOSTInt32MSB => (int(4, 32), true),
            ASIOSTInt32MSB16 => (int(4, 16), true),
            ASIOSTInt32MSB18 => (int(4, 18), true),
            ASIOSTInt32MSB20 => (int(4, 20), true),
            ASIOSTInt32MSB24 => (int(4, 24), true),
            ASIOSTFloat32MSB => (Format::F32, true),
            ASIOSTFloat64MSB => (Format::F64, true),
            _ => return None,
        })
    }

    fn bytes(self) -> usize {
        match self {
            Format::Int { bytes, .. } => bytes,
            Format::F32 => 4,
            Format::F64 => 8,
        }
    }
}

/// One sample (clipped to ±1) as the driver takes it, into `out` (`format.bytes()` long).
fn encode(x: f32, format: Format, big: bool, out: &mut [u8]) {
    let x = x.clamp(-1.0, 1.0);
    let mut b = [0u8; 8];
    match format {
        Format::Int { bits, .. } => {
            let full = ((1i64 << (bits - 1)) - 1) as f64;
            b = ((f64::from(x) * full).round() as i64).to_le_bytes();
        }
        Format::F32 => b[..4].copy_from_slice(&x.to_le_bytes()),
        Format::F64 => b = f64::from(x).to_le_bytes(),
    }
    let b = &mut b[..format.bytes()];
    if big {
        b.reverse();
    }
    out.copy_from_slice(b);
}

/// One sample from the driver's `format` (`bytes` long) as a float.
fn decode(b: &[u8], format: Format, big: bool) -> f32 {
    let mut w = [0u8; 8];
    let n = format.bytes();
    w[..n].copy_from_slice(&b[..n]);
    if big {
        w[..n].reverse();
    }
    match format {
        Format::Int { bytes, bits } => {
            // Sign-extend the word, then scale by the bits used (low bits of the word).
            let shift = 64 - 8 * bytes as u32;
            let v = (i64::from_le_bytes(w) << shift) >> shift;
            let full = ((1i64 << (bits - 1)) - 1) as f64;
            (v as f64 / full) as f32
        }
        Format::F32 => f32::from_le_bytes([w[0], w[1], w[2], w[3]]),
        Format::F64 => f64::from_le_bytes(w) as f32,
    }
}

/// Which of the stereo mix an output gets: one side, or both summed and halved (a driver with one output).
#[derive(Clone, Copy)]
enum Side {
    One(usize),
    Both,
}

/// Writes `side` of the interleaved stereo `mix` into a driver buffer of `frames` samples.
///
/// Safety: `dst` holds `frames` samples of `format`, and nothing else uses it meanwhile.
unsafe fn write(dst: *mut u8, mix: &[f32], side: Side, frames: usize, format: Format, big: bool) {
    let n = format.bytes();
    let out = unsafe { std::slice::from_raw_parts_mut(dst, frames * n) };
    for (i, s) in out.chunks_exact_mut(n).enumerate() {
        let x = match side {
            Side::One(ch) => mix[2 * i + ch],
            Side::Both => 0.5 * (mix[2 * i] + mix[2 * i + 1]),
        };
        encode(x, format, big, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(x: f32, t: sys::AsioSampleType) -> Vec<u8> {
        let (f, big) = Format::of(&t).expect("a format we write");
        let mut out = vec![0; f.bytes()];
        encode(x, f, big, &mut out);
        out
    }

    /// Each sample format the drivers use, either way round, full scale and clipped.
    #[test]
    fn samples_are_written_as_the_driver_takes_them() {
        use sys::AsioSampleType::*;
        assert_eq!(bytes(1.0, ASIOSTInt16LSB), [0xff, 0x7f]);
        assert_eq!(bytes(-1.0, ASIOSTInt16LSB), [0x01, 0x80]);
        assert_eq!(bytes(4.0, ASIOSTInt16MSB), [0x7f, 0xff], "clipped");
        assert_eq!(bytes(0.5, ASIOSTInt24LSB), [0x00, 0x00, 0x40]);
        assert_eq!(bytes(-1.0, ASIOSTInt24MSB), [0x80, 0x00, 0x01]);
        assert_eq!(bytes(1.0, ASIOSTInt32LSB), i32::MAX.to_le_bytes());
        assert_eq!(bytes(-1.0, ASIOSTInt32LSB), (-i32::MAX).to_le_bytes());
        // 24 bits in a 32-bit word, in its low bits.
        assert_eq!(bytes(1.0, ASIOSTInt32LSB24), 0x7f_ffffi32.to_le_bytes());
        assert_eq!(bytes(-1.0, ASIOSTInt32LSB16), (-0x7fffi32).to_le_bytes());
        assert_eq!(bytes(0.25, ASIOSTFloat32LSB), 0.25f32.to_le_bytes());
        assert_eq!(bytes(0.25, ASIOSTFloat32MSB), 0.25f32.to_be_bytes());
        assert_eq!(bytes(-0.5, ASIOSTFloat64LSB), (-0.5f64).to_le_bytes());
        assert_eq!(Format::of(&ASIOSTDSDInt8LSB1), None, "DSD is not written");
    }

    /// Input samples read back as written, every format either way round.
    #[test]
    fn samples_are_read_as_the_driver_gives_them() {
        use sys::AsioSampleType::*;
        for t in [ASIOSTInt16LSB, ASIOSTInt24MSB, ASIOSTInt32LSB, ASIOSTInt32LSB24, ASIOSTInt32MSB16, ASIOSTFloat32MSB, ASIOSTFloat64LSB] {
            let (f, big) = Format::of(&t).expect("a format");
            for x in [0.5f32, -0.25, -1.0, 0.0] {
                let mut b = vec![0; f.bytes()];
                encode(x, f, big, &mut b);
                assert!((decode(&b, f, big) - x).abs() < 1e-4, "{t:?} {x}");
            }
        }
    }

    /// A buffer gets one side of the stereo mix; a driver's only output gets both, halved.
    #[test]
    fn a_buffer_gets_its_side() {
        let mix = [0.5, -0.5, 0.25, 0.75];
        let mut buf = [0u8; 8];
        unsafe { write(buf.as_mut_ptr(), &mix, Side::One(1), 2, Format::F32, false) };
        assert_eq!(buf[..4], (-0.5f32).to_le_bytes());
        assert_eq!(buf[4..], 0.75f32.to_le_bytes());
        unsafe { write(buf.as_mut_ptr(), &mix, Side::Both, 2, Format::F32, false) };
        assert_eq!(buf[..4], 0.0f32.to_le_bytes());
        assert_eq!(buf[4..], 0.5f32.to_le_bytes());
    }

    /// The buffer lengths offered for a driver: those of the usual list it takes.
    #[test]
    fn a_driver_offers_the_lengths_it_takes() {
        use sys::BufferPreference::*;
        let range = |min, max, preferred| sys::BufferSizeRange { min, max, preferred };
        assert_eq!(sizes(&range(64, 2048, Stepped { preferred: 512, step: 8 })), [64, 128, 256, 512, 1024, 2048]);
        assert_eq!(sizes(&range(96, 2048, Stepped { preferred: 512, step: 32 })), [128, 256, 512, 1024, 2048]);
        assert_eq!(sizes(&range(100, 1000, Only(256))), [128, 256, 512], "powers of two");
        assert_eq!(sizes(&range(512, 512, Preferred(512))), [] as [u32; 0], "its own only");
        assert_eq!(sizes(&range(256, 2048, Preferred(512))), [] as [u32; 0], "no granularity: its own only");
    }
}
