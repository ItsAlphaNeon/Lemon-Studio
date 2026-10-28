// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The system's input device (a microphone, a line input) for the mixer tracks, when the output is not an ASIO driver
//! (spec: audio/track-io.md §2.2). Its stream opens with the output, only while a track hears its input or is armed
//! (`super::want_inputs`), so the microphone is not taken (and the system does not show it in use) the rest of the
//! time. It runs at the output's rate (resampled when the device will not), and its callback pushes into the same ring
//! an ASIO driver's inputs go into (`super::input_ring`), which the streams read at their own place.
//!
//! What the device is (its name and channels) is asked off the UI thread and kept a few seconds (`info`): asking the
//! sound server takes a while, and the mixer asks on every repaint.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize};

use super::{input_ring, INPUT_CHANNELS};
use crate::audio_cfg::{self, AudioCfg};

/// The system's default input device as the mixer offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub name: String,
    /// Its inputs (at most `INPUT_CHANNELS`).
    pub channels: usize,
}

/// How long what `info` found is kept before the device is asked again.
const KEEP: Duration = Duration::from_secs(3);

struct Cache {
    found: Option<Info>,
    /// When it was asked last (`None`: never), and whether an ask is under way.
    at: Option<Instant>,
    asking: bool,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache { found: None, at: None, asking: false });

/// The system's default input device for the audio settings in use (`None`: none, or not asked yet). It never waits:
/// a device asked about more than `KEEP` ago is asked again on a thread of its own.
pub fn info() -> Option<Info> {
    if cfg!(test) {
        return None;
    }
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if !c.asking && c.at.is_none_or(|t| t.elapsed() >= KEEP) {
        c.asking = true;
        let spawned = std::thread::Builder::new().name("input device".into()).spawn(|| {
            let found = ask(&AudioCfg::current());
            let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            (c.found, c.at, c.asking) = (found, Some(Instant::now()), false);
        });
        if spawned.is_err() {
            c.asking = false;
        }
    }
    c.found.clone()
}

/// Asks the host `cfg` plays on for its default input device.
fn ask(cfg: &AudioCfg) -> Option<Info> {
    let device = audio_cfg::host(cfg).default_input_device()?;
    let channels = usize::from(device.default_input_config().ok()?.channels()).min(INPUT_CHANNELS);
    (channels > 0).then(|| Info { name: audio_cfg::device_name(&device), channels })
}

/// The open input stream. Dropped, it stops and the ring has no inputs again.
pub struct SystemInput {
    stream: Option<cpal::Stream>,
    /// Its error callback said it does not play on (the device went away).
    failed: Arc<AtomicBool>,
    alsa: bool,
}

impl Drop for SystemInput {
    fn drop(&mut self) {
        drop(self.stream.take());
        input_ring().open(0, 0);
        if self.alsa {
            super::heal_sound_server();
        }
    }
}

impl SystemInput {
    /// The stream stopped for good (it is opened again with the output).
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

/// Opens the default input device of `cfg`'s host at `rate` (the output's), with blocks of about `block` frames.
pub fn open(cfg: &AudioCfg, rate: u32, block: Option<u32>) -> Result<SystemInput, String> {
    let host = audio_cfg::host(cfg);
    let device = host.default_input_device().ok_or("no audio input device")?;
    let own = device.default_input_config().map_err(|e| e.to_string())?;
    // The output's rate when the device takes it, else its own (resampled in the callback).
    let supported = if own.sample_rate() == rate {
        own
    } else {
        device
            .supported_input_configs()
            .ok()
            .and_then(|mut cs| cs.find(|c| c.channels() == own.channels() && c.sample_format() == own.sample_format() && c.min_sample_rate() <= rate && rate <= c.max_sample_rate()))
            .map(|c| c.with_sample_rate(rate))
            .unwrap_or(own)
    };
    let format = supported.sample_format();
    let range = supported.buffer_size().clone();
    let mut config: StreamConfig = supported.into();
    // Through the sound server a stream that asks for no length is buffered for seconds: it asks for the output's.
    let b = block.unwrap_or(512);
    if matches!(range, SupportedBufferSize::Range { min, max } if (min..=max).contains(&b)) {
        config.buffer_size = BufferSize::Fixed(b);
    }
    let channels = usize::from(config.channels).clamp(1, INPUT_CHANNELS);
    input_ring().open(channels, b as usize);
    let failed = Arc::new(AtomicBool::new(false));
    let mut stream = build_any(&device, format, &config, rate, failed.clone());
    if stream.is_err() && config.buffer_size != BufferSize::Default {
        config.buffer_size = BufferSize::Default;
        stream = build_any(&device, format, &config, rate, failed.clone());
    }
    let stream = stream.inspect_err(|_| input_ring().open(0, 0))?;
    stream.play().map_err(|e| {
        input_ring().open(0, 0);
        e.to_string()
    })?;
    Ok(SystemInput { stream: Some(stream), failed, alsa: host.id().name() == "ALSA" })
}

fn build_any(device: &cpal::Device, format: SampleFormat, config: &StreamConfig, rate: u32, failed: Arc<AtomicBool>) -> Result<cpal::Stream, String> {
    match format {
        SampleFormat::F32 => build::<f32>(device, config, rate, failed),
        SampleFormat::F64 => build::<f64>(device, config, rate, failed),
        SampleFormat::I16 => build::<i16>(device, config, rate, failed),
        SampleFormat::I24 => build::<cpal::I24>(device, config, rate, failed),
        SampleFormat::I32 => build::<i32>(device, config, rate, failed),
        SampleFormat::U16 => build::<u16>(device, config, rate, failed),
        other => Err(format!("unsupported input sample format {other:?}")),
    }
}

fn build<T>(device: &cpal::Device, config: &StreamConfig, rate: u32, failed: Arc<AtomicBool>) -> Result<cpal::Stream, String>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let device_ch = usize::from(config.channels).max(1);
    let ch = device_ch.min(INPUT_CHANNELS);
    let mut resample = Resampler::new(config.sample_rate, rate, ch);
    device
        .build_input_stream::<T, _, _>(
            config.clone(),
            move |data: &[T], _| {
                let frames = data.len() / device_ch;
                let at = |c: usize, f: usize| data[f * device_ch + c].to_sample::<f32>();
                match resample.as_mut() {
                    None => input_ring().push(frames, at),
                    Some(r) => {
                        let out = r.run(frames, at);
                        input_ring().push(out.len() / ch, |c, f| out[f * ch + c]);
                    }
                }
            },
            move |e| {
                use cpal::ErrorKind::*;
                match e.kind() {
                    Xrun | DeviceChanged | RealtimeDenied => {}
                    _ => {
                        eprintln!("audio input stream error: {e}");
                        failed.store(true, Ordering::Release);
                    }
                }
            },
            None,
        )
        .map_err(|e| e.to_string())
}

/// Linear resampling from the device's rate to the output's, block by block (`None` when they are the same).
struct Resampler {
    /// Input frames per output frame.
    step: f64,
    /// Where the next output frame falls, in input frames from the start of the next block (-1 ..= 0: between the
    /// previous block's last frame and this one's first).
    pos: f64,
    ch: usize,
    last: Vec<f32>,
    out: Vec<f32>,
}

impl Resampler {
    fn new(from: u32, to: u32, ch: usize) -> Option<Resampler> {
        (from != to && from > 0 && to > 0).then(|| Resampler {
            step: f64::from(from) / f64::from(to),
            pos: 0.0,
            ch,
            last: vec![0.0; ch],
            // Room for long blocks made now, not in the callback.
            out: Vec::with_capacity(ch * 32768),
        })
    }

    /// The output frames of a block of `frames` input frames, `at(channel, frame)` an input sample (interleaved).
    fn run(&mut self, frames: usize, at: impl Fn(usize, usize) -> f32) -> &[f32] {
        self.out.clear();
        if frames == 0 {
            return &self.out;
        }
        let sample = |s: &Self, c: usize, i: isize| if i < 0 { s.last[c] } else { at(c, i as usize) };
        while self.pos <= (frames - 1) as f64 {
            let i = self.pos.floor();
            let t = (self.pos - i) as f32;
            let i = i as isize;
            for c in 0..self.ch {
                let a = sample(self, c, i);
                let b = if i + 1 < frames as isize { sample(self, c, i + 1) } else { a };
                self.out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        for c in 0..self.ch {
            self.last[c] = at(c, frames - 1);
        }
        &self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp at 44.1 kHz comes out at 48 kHz as the same ramp, blocks joined without a seam.
    #[test]
    fn resampling_keeps_a_ramp_straight_across_blocks() {
        let mut r = Resampler::new(44100, 48000, 1).expect("rates differ");
        let mut got = Vec::new();
        let mut n = 0usize;
        for block in [441usize, 100, 1, 512, 300] {
            let start = n;
            got.extend_from_slice(r.run(block, |_, f| (start + f) as f32));
            n += block;
        }
        let step = 44100.0 / 48000.0;
        for (k, v) in got.iter().enumerate() {
            assert!((v - (k as f64 * step) as f32).abs() < 1e-3, "frame {k}: {v}");
        }
        let expected = (n as f64 / step) as usize;
        assert!(got.len().abs_diff(expected) <= 1, "{} frames for {n} in", got.len());
        assert!(Resampler::new(48000, 48000, 2).is_none());
    }
}
