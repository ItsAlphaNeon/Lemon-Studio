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
//! The device opens with the audio settings (`audio_cfg`) when first needed. A Windows (or ALSA, Core Audio) device
//! closes again once nothing plays (`close_idle`), as the streams did when each sound had its own: an open stream keeps
//! the computer from sleeping. A stream that failed (the device went away, a driver asked to be reset) reports it
//! (`failure`) and is opened anew the next time it is needed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig};
use crossbeam_queue::ArrayQueue;

use crate::audio_cfg::{self, AudioCfg};

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

/// The open stream and what it was opened with.
pub(crate) struct Open {
    /// Dropped first: the callback (and the sources in it) stop before the queues go.
    _stream: cpal::Stream,
    inbox: Arc<Queue>,
    outbox: Arc<Queue>,
    /// The device's name as the settings show it.
    pub(crate) name: String,
    pub(crate) rate: u32,
    /// The buffer length asked for (`None`: the device's own).
    pub(crate) fixed: Option<u32>,
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
}

impl OutputDevice {
    /// The open stream, opening it with the audio settings first if it is closed or has failed.
    pub(crate) fn open(&mut self) -> Result<&Open, String> {
        if self.open.as_ref().is_some_and(|o| o.failed.load(Ordering::Acquire)) {
            self.open = None;
        }
        if self.open.is_none() {
            self.open = Some(open(&AudioCfg::current(), &self.xruns)?);
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

    /// Nothing plays any more: lets go of the device.
    pub(crate) fn close_idle(&mut self) {
        self.close();
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
        Some((o.name.clone(), why))
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

/// Opens the stream for `cfg`: its device, rate and buffer length, else the device's own buffer length.
fn open(cfg: &AudioCfg, xruns: &Arc<AtomicU64>) -> Result<Open, String> {
    let out = audio_cfg::output(cfg)?;
    let (device, format, mut config) = (out.device, out.format, out.config);
    let inbox = Arc::new(Queue::new(MAX_SOURCES));
    let outbox = Arc::new(Queue::new(MAX_SOURCES));
    let errors = Errors { failed: Arc::new(AtomicBool::new(false)), fault: Arc::new(Mutex::new(None)), xruns: xruns.clone() };
    let mix = || Mix::new(inbox.clone(), outbox.clone());
    let fixed = match config.buffer_size {
        BufferSize::Fixed(b) => Some(b),
        BufferSize::Default => None,
    };
    let mut stream = build_any(&device, format, &config, mix(), &errors);
    if stream.is_err() && fixed.is_some() {
        // A buffer length the device refuses: its own.
        config.buffer_size = BufferSize::Default;
        stream = build_any(&device, format, &config, mix(), &errors);
    }
    let stream = stream?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(Open { _stream: stream, inbox, outbox, name: out.name, rate: config.sample_rate, fixed, failed: errors.failed, fault: errors.fault })
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
            Xrun => {
                self.xruns.fetch_add(1, Ordering::Relaxed);
            }
            // The stream plays on (moved to the new default device, or at a normal priority).
            DeviceChanged | RealtimeDenied => eprintln!("audio stream: {e}"),
            // Anything else: the stream has to be opened anew.
            _ => {
                eprintln!("audio stream error: {e}");
                if let Ok(mut f) = self.fault.lock() {
                    f.get_or_insert_with(|| e.to_string());
                }
                self.failed.store(true, Ordering::Release);
            }
        }
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
