// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Playback: an audio thread that owns the output streams and the engines.
//!
//! One state machine, `State` (stopped / playing / paused), published in `Shared`:
//! - **Play** starts the pattern or the playlist from a start tick (a pattern loops inside the engine, the
//!   playlist plays out and stops by itself). A jump of the position while playing is a new Play there (a new stream
//!   and engine). Before the first block, the hosted instruments are reset, the hosted effects too with "Reset plugins
//!   on transport"; with "Play truncated notes on transport" the notes under way at the start tick sound from there
//!   (`Starts`; spec: ui/settings-choices.md "Transport: Play truncated notes and Reset plugins").
//! - **Pause** fades the stream out over a few milliseconds and holds the engine where it is (position, ringing
//!   voices, hosted plugins untouched); **Resume** fades back in from that exact place.
//! - **Stop** fades out, drops the streams, silences every hosted instrument (`Instrument::reset`: voices and tails
//!   die, nothing resumes later) and rewinds to the start. It also ends live notes and previews.
//!
//! The UI sets the state at once (so a double click behaves), then sends the matching command; commands are
//! applied in order on the audio thread. `generation` counts play / stop requests: a song that finishes after a
//! newer request must not stop it.
//!
//! Every stream renders ahead of its device: a feeder thread renders whole ticks into a lock-free queue and the
//! device callback only copies from it (spec: audio/implementing.md "Playing live"), so a busy engine (an edit being
//! swapped in, a heavy tick) never costs a silent block. The queue holds one device block and `AHEAD_TICKS` ticks;
//! a block the queue could not fill counts as an underrun (`Transport::underruns`).
//!
//! Streams open with the audio settings (`audio_cfg`: device, rate, buffer length); `reopen` moves the running
//! playback onto a stream with new settings at the position heard.
//!
//! `update` swaps in an edited snapshot at the current position (decoded samples stay cached in the library).
//! `preview` plays one note once on its own stream. Hosted generator plugins (`set_instruments`) are attached to
//! the playback and live engines only: an instance must not be processed by two streams, so previews of plugin
//! channels play as timed live notes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig};
use os_engine::{Engine, PlayMode, SampleLibrary, SharedEffect, SharedInstrument};
use os_model::Project;

/// Transport state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Stopped,
    Playing,
    Paused,
}

const STOPPED: u8 = 0;
const PLAYING: u8 = 1;
const PAUSED: u8 = 2;

/// Seconds a stream takes to fade out (pause, stop) or in (resume): no clicks.
const FADE_SECONDS: f64 = 0.006;
/// How long a stopped stream stays alive to finish its fade before it is dropped.
const CLOSE_AFTER: Duration = Duration::from_millis(40);
/// Ticks a stream renders ahead of its device on top of one device block: what a live edit or note waits for.
const AHEAD_TICKS: u64 = 2;
/// A stream's queue: samples (interleaved stereo) and ticks it can hold, and the room the feeder leaves for one more
/// tick (a tick at 10 BPM and 24 PPQ at 192 kHz is 96 000 samples).
const QUEUE_SAMPLES: usize = 1 << 19;
const QUEUE_TICKS: usize = 1 << 12;
const TICK_ROOM: usize = 1 << 17;
/// Frames the queue is filled with before a stream starts (the device's block size is not known yet).
const PREFILL_FRAMES: usize = 1024;

/// State the UI reads every frame.
#[derive(Default)]
pub struct Shared {
    state: AtomicU8,
    /// Counts play and stop requests (see the module docs).
    generation: AtomicU64,
    /// The running playback is the playlist (else one pattern).
    song: AtomicBool,
    /// Playback position in frames of the output stream.
    position: AtomicU64,
    /// Playback position in ticks (f64 bits): what every playhead and the time display show. A pattern wraps.
    tick: AtomicU64,
    sample_rate: AtomicU32,
    /// An export is running, its progress in 1/1000 of the song, and its outcome once done.
    exporting: AtomicBool,
    export_permille: AtomicU32,
    export_result: Mutex<Option<Result<f64, String>>>,
    /// Peak of the last block, f32 bits, per side.
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    meter_l: AtomicU32,
    meter_r: AtomicU32,
    error: Mutex<Option<String>>,
    /// Notes playing now, as (channel index, key); written by the audio callback.
    sounding: Mutex<Vec<(u16, u16)>>,
    /// The last samples that left the playback / live stream (mono), for the toolbar monitor.
    tap: Mutex<Tap>,
    /// Peak per mixer track (L, R) since the mixer meters last read them.
    tracks: Mutex<Vec<[f32; 2]>>,
    /// Device blocks the queue could not fill since the program started.
    underruns: AtomicU64,
    /// Time spent rendering per second of audio (f32 bits), over the last quarter second of playback.
    load: AtomicU32,
    /// Frames the device asked for in its last block.
    block: AtomicU32,
    /// The output the playback or live stream opened: device name, rate, buffer length asked for.
    opened: Mutex<Option<(String, u32, Option<u32>)>>,
    /// An instrument's editor is open (`Transport::listen`).
    listening: AtomicBool,
    /// Each running stream's engine voices (its feeder writes them after every fill; a stream that closed drops its
    /// counter). Spec: audio/voices.md §5.
    engine_voices: Mutex<Vec<Weak<AtomicU32>>>,
}

/// Ring of the newest output samples.
struct Tap {
    buf: Vec<f32>,
    pos: usize,
    /// Samples written since the start; unchanged = no audio arrived.
    total: u64,
}

impl Default for Tap {
    fn default() -> Self {
        Tap { buf: vec![0.0; crate::monitor::MAX_FFT], pos: 0, total: 0 }
    }
}

impl Tap {
    fn push(&mut self, v: f32) {
        self.buf[self.pos] = v;
        self.pos = (self.pos + 1) % self.buf.len();
        self.total += 1;
    }
}

impl Shared {
    /// Back to the start: position, meters and lit keys.
    fn rewind(&self, tick: f64) {
        self.position.store(0, Ordering::Relaxed);
        self.tick.store(tick.to_bits(), Ordering::Relaxed);
        self.peak_l.store(0, Ordering::Relaxed);
        self.peak_r.store(0, Ordering::Relaxed);
        if let Ok(mut v) = self.sounding.lock() {
            v.clear();
        }
    }
}

enum Cmd {
    /// Project, mode, start tick (the piano roll's play-start marker in pattern mode), request generation, and the
    /// transport settings as they were asked for.
    Play(Box<Project>, PlayMode, f64, u64, Starts),
    Pause,
    Resume,
    /// The newest edited project is in `Transport::latest` (older ones are skipped).
    Update,
    Preview(Box<Project>),
    /// Play a project's playlist once on the preview stream (clip preview).
    PreviewSong(Box<Project>),
    /// Like `Preview`, faded out after the given time if still sounding (browser sample preview).
    PreviewFor(Box<Project>, Option<Duration>),
    /// The hosted instruments changed (in `Transport::instruments`): rebuild the engines.
    Instruments,
    /// Live note (typing keyboard): channel (rack position), key, velocity; `NoteOff` releases.
    NoteOn(usize, u16, u8),
    NoteOff(usize, u16),
    /// A tap on the Tempo tapper: a click, accented or not, like a live note.
    TapClick(bool),
    /// An instrument's editor opened or the last one closed (`Transport::listen`).
    Listen(bool),
    Stop,
    /// Render to a WAV file with the hosted plugins (FILE > Export): project, mode, sample rate, file.
    Export(Box<Project>, PlayMode, u32, PathBuf),
    /// The audio settings changed: move the playback to a new stream, close the live and preview streams.
    Reopen,
    /// Auto close (`Transport::close_device`): every stream closes; the playback keeps its place for `Open`.
    Close,
    /// The output may be used again (`Transport::open_device`).
    Open,
}

/// Where an export is (`Transport::export_status`).
#[derive(Debug, Clone, PartialEq)]
pub enum Export {
    Idle,
    /// Rendering: 0..1 of the song.
    Running(f32),
    /// Finished: the length written in seconds, or what went wrong.
    Done(Result<f64, String>),
}

/// The Audio > Mixer transport settings a start of playback goes by (Play, and a jump of the position while playing,
/// which the app makes a new Play), read from the program settings on the UI thread when the start is asked for, so a
/// change counts from the next start (spec: ui/settings-choices.md "Transport: Play truncated notes and Reset
/// plugins").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Starts {
    /// "Play truncated notes on transport": the notes under way at the start tick sound from there, afresh, for what
    /// is left of them (`Engine::set_truncated_on_transport`).
    pub(crate) truncated: bool,
    /// "Reset plugins on transport": the start resets the hosted effects too (their delay lines and tails), not only
    /// the instruments.
    pub(crate) reset: bool,
}

impl Starts {
    pub(crate) fn from_prefs() -> Starts {
        use crate::prefs::{check, PLAY_TRUNCATED_ON_TRANSPORT, RESET_PLUGINS_ON_TRANSPORT};
        Starts { truncated: check(&PLAY_TRUNCATED_ON_TRANSPORT), reset: check(&RESET_PLUGINS_ON_TRANSPORT) }
    }
}

/// An export renders at most this long past the song's end (instruments that never report silence).
const EXPORT_TAIL_MAX_SECS: u64 = 30;

/// Hosted instruments by rack row, and hosted mixer effects by (track, slot).
#[derive(Default)]
struct Hosts {
    inst: Vec<(usize, SharedInstrument)>,
    fx: Vec<(usize, usize, SharedEffect)>,
}

type Instruments = Arc<Mutex<Hosts>>;

pub struct Transport {
    tx: Sender<Cmd>,
    latest: Arc<Mutex<Option<Project>>>,
    instruments: Instruments,
    shared: Arc<Shared>,
}

impl Transport {
    pub fn new(factory: Option<PathBuf>) -> Transport {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let instruments: Instruments = Arc::new(Mutex::new(Hosts::default()));
        let shared = Arc::new(Shared { sounding: Mutex::new(Vec::with_capacity(256)), ..Shared::default() });
        let s = shared.clone();
        std::thread::Builder::new()
            .name("openstudio-audio".into())
            .spawn({
                let latest = latest.clone();
                let inst = instruments.clone();
                move || {
                    // The channels' Lemon Sampler instances stretch for themselves (spec: audio/time-stretch.md).
                    let mut library = SampleLibrary::new(factory);
                    library.resample_stretches = true;
                    audio_thread(rx, s, latest, inst, library)
                }
            })
            .expect("spawn audio thread");
        Transport { tx, latest, instruments, shared }
    }

    /// Hosted generator plugins by rack row; playing engines pick them up at once.
    pub fn set_instruments(&self, list: Vec<(usize, SharedInstrument)>) {
        if let Ok(mut slot) = self.instruments.lock() {
            slot.inst = list;
        }
        let _ = self.tx.send(Cmd::Instruments);
    }

    /// Hosted effect plugins in mixer slots (track, slot); playing engines pick them up at once.
    pub fn set_effects(&self, list: Vec<(usize, usize, SharedEffect)>) {
        if let Ok(mut slot) = self.instruments.lock() {
            slot.fx = list;
        }
        let _ = self.tx.send(Cmd::Instruments);
    }

    /// Peak per mixer track (L, R) since the last call (the mixer meters' feed); resets them.
    pub fn take_track_peaks(&self) -> Vec<[f32; 2]> {
        self.shared.tracks.lock().map(|mut v| {
            let out = v.clone();
            v.iter_mut().for_each(|p| *p = [0.0; 2]);
            out
        }).unwrap_or_default()
    }

    /// The sample rate streams open at (the audio settings' device and rate; what hosted plugins are activated at).
    pub fn device_rate() -> Option<u32> {
        crate::audio_cfg::rate()
    }

    /// The audio settings changed (`audio_cfg`): playing or paused playback goes on from the position heard on a
    /// stream with the new settings; live notes and previews open one when next needed.
    pub fn reopen(&self) {
        let _ = self.tx.send(Cmd::Reopen);
    }

    /// Lets go of the output device (Audio ▸ Auto close, `crate::auto_close`): playback, previews and live notes close
    /// their streams after their fade. A playback (pause it first) keeps its place; until `open_device` nothing opens
    /// a stream (previews and live notes go unheard), except a new `play`.
    pub fn close_device(&self) {
        let _ = self.tx.send(Cmd::Close);
    }

    /// The output device may be used again: a playback held by `close_device` goes on from its place on a new stream,
    /// playing if the transport plays by then (`resume`), else paused.
    pub fn open_device(&self) {
        let _ = self.tx.send(Cmd::Open);
    }

    /// Renders `project` offline to a 32-bit float WAV file at `rate`, with the hosted instruments and effects, on the
    /// audio thread: playback, previews and live notes stop at once and nothing else processes the plugins meanwhile.
    /// Follow it with `export_status`.
    pub fn export(&self, project: &Project, mode: PlayMode, rate: u32, path: PathBuf) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.state.store(STOPPED, Ordering::Release);
        self.shared.export_permille.store(0, Ordering::Relaxed);
        self.shared.exporting.store(true, Ordering::Release);
        let _ = self.tx.send(Cmd::Export(Box::new(project.clone()), mode, rate, path));
    }

    /// The running export's progress, or its outcome once (then `Idle` again).
    pub fn export_status(&self) -> Export {
        if let Some(r) = self.shared.export_result.lock().ok().and_then(|mut r| r.take()) {
            return Export::Done(r);
        }
        if self.shared.exporting.load(Ordering::Acquire) {
            return Export::Running(self.shared.export_permille.load(Ordering::Relaxed) as f32 / 1000.0);
        }
        Export::Idle
    }

    /// Start (or restart) playback from `start_tick`; replaces whatever is playing or paused. The transport settings
    /// (`Starts`) are read now.
    pub fn play(&self, project: &Project, mode: PlayMode, start_tick: f64) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = None;
        }
        let generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.song.store(matches!(mode, PlayMode::Song), Ordering::Relaxed);
        self.shared.rewind(start_tick);
        self.shared.state.store(PLAYING, Ordering::Release);
        let _ = self.tx.send(Cmd::Play(Box::new(project.clone()), mode, start_tick, generation, Starts::from_prefs()));
    }

    /// Replace the playing project, continuing from the current position.
    pub fn update(&self, project: &Project) {
        // Edits can come faster than engines are built: keep only the newest.
        if let Ok(mut slot) = self.latest.lock() {
            *slot = Some(project.clone());
        }
        let _ = self.tx.send(Cmd::Update);
    }

    /// Play pattern 1 of `project` once (a one-note preview project), on its own stream.
    pub fn preview(&self, project: Project) {
        let _ = self.tx.send(Cmd::Preview(Box::new(project)));
    }

    /// Play pattern 1 of `project` once on the preview stream and cut it off after `limit` (spec: ui/browser.md §7,
    /// sample preview). A new preview replaces the one sounding.
    pub fn preview_for(&self, project: Project, limit: Option<Duration>) {
        let _ = self.tx.send(Cmd::PreviewFor(Box::new(project), limit));
    }

    /// Play the playlist of `project` once on the preview stream (spec: ui/playlist.md §10, clip Preview).
    pub fn preview_song(&self, project: Project) {
        let _ = self.tx.send(Cmd::PreviewSong(Box::new(project)));
    }

    /// Start a live note: mixed into playback while playing, else on the live stream (kept open, so
    /// only the first note pays for opening the device). The live stream uses the project from the
    /// last `play` / `update`.
    pub fn note_on(&self, channel: usize, key: u16, velocity: u8) {
        let _ = self.tx.send(Cmd::NoteOn(channel, key, velocity));
    }

    pub fn note_off(&self, channel: usize, key: u16) {
        let _ = self.tx.send(Cmd::NoteOff(channel, key));
    }

    /// A tap on the Tempo tapper (spec: ui/tempo-tapper.md §8): the metronome's Tick, accented or not, where a live
    /// note would play (into playback while playing, else on the live stream).
    pub fn tap_click(&self, accent: bool) {
        let _ = self.tx.send(Cmd::TapClick(accent));
    }

    /// An instrument's editor is open (or the last one closed). Its own keyboard plays notes the host never sees, which
    /// only sound if the plugin is processed: while nothing plays (stopped or paused) the live stream stays open and
    /// keeps processing the hosted instruments. The live stream uses the project from the last `play` / `update`.
    pub fn listen(&self, on: bool) {
        if self.shared.listening.swap(on, Ordering::AcqRel) != on {
            let _ = self.tx.send(Cmd::Listen(on));
        }
    }

    /// Hold playback where it is (Playing -> Paused); `resume` continues from the same place.
    pub fn pause(&self) {
        if self.shared.state.compare_exchange(PLAYING, PAUSED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            if let Ok(mut v) = self.shared.sounding.lock() {
                v.clear();
            }
            let _ = self.tx.send(Cmd::Pause);
        }
    }

    pub fn resume(&self) {
        if self.shared.state.compare_exchange(PAUSED, PLAYING, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            let _ = self.tx.send(Cmd::Resume);
        }
    }

    /// Stop everything: playback rewinds, hosted instruments fall silent, live notes and previews end. Also works
    /// when already stopped (it silences notes still ringing from the typing keyboard or a preview).
    pub fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.state.store(STOPPED, Ordering::Release);
        self.shared.rewind(0.0);
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn state(&self) -> State {
        match self.shared.state.load(Ordering::Acquire) {
            PLAYING => State::Playing,
            PAUSED => State::Paused,
            _ => State::Stopped,
        }
    }

    /// Playing or paused: there is a running song position (stopped = rewound).
    pub fn playing(&self) -> bool {
        self.state() != State::Stopped
    }

    pub fn paused(&self) -> bool {
        self.state() == State::Paused
    }

    /// The running playback is the playlist (false: one pattern). Meaningful while `playing`.
    pub fn song(&self) -> bool {
        self.shared.song.load(Ordering::Relaxed)
    }

    /// Playback position in ticks of the running mode (a looping pattern wraps at its end).
    pub fn tick(&self) -> f64 {
        f64::from_bits(self.shared.tick.load(Ordering::Relaxed))
    }

    pub fn seconds(&self) -> f64 {
        let rate = self.shared.sample_rate.load(Ordering::Relaxed).max(1);
        self.shared.position.load(Ordering::Relaxed) as f64 / f64::from(rate)
    }

    /// Fills `out` with the newest output samples (oldest first, at most 8192) and returns the running sample
    /// count; an unchanged count means nothing was played since the last call.
    pub fn tap(&self, out: &mut [f32]) -> u64 {
        let Ok(t) = self.shared.tap.lock() else { return 0 };
        let n = out.len().min(t.buf.len());
        let skip = out.len() - n;
        out[..skip].fill(0.0);
        let len = t.buf.len();
        for (i, o) in out[skip..].iter_mut().enumerate() {
            *o = t.buf[(t.pos + len - n + i) % len];
        }
        t.total
    }

    /// The output stream's sample rate (0 until the first stream opened).
    pub fn sample_rate(&self) -> u32 {
        self.shared.sample_rate.load(Ordering::Relaxed)
    }

    /// Output peaks (L, R) since the last call, then resets them (the toolbar meter's feed).
    pub fn take_peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.shared.meter_l.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.shared.meter_r.swap(0, Ordering::Relaxed)),
        )
    }

    /// Peaks of the last playback block (L, R).
    pub fn peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.shared.peak_l.load(Ordering::Relaxed)),
            f32::from_bits(self.shared.peak_r.load(Ordering::Relaxed)),
        )
    }

    /// Notes playing now (pattern or song playback, not previews), as (channel index, key).
    pub fn sounding(&self) -> Vec<(u16, u16)> {
        self.shared.sounding.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn take_error(&self) -> Option<String> {
        self.shared.error.lock().ok()?.take()
    }

    /// The engine's own voices sounding on every stream (playback, previews, live notes), as of their last fill.
    /// Hosted instruments count theirs (spec: audio/voices.md §5).
    pub fn engine_voices(&self) -> usize {
        let Ok(v) = self.shared.engine_voices.lock() else { return 0 };
        v.iter().filter_map(Weak::upgrade).map(|n| n.load(Ordering::Relaxed) as usize).sum()
    }

    /// Device blocks that could not be filled in time (heard as a gap) since the program started.
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// Time the engine spends rendering per second of audio, 0..1 (more than 1: it cannot keep up), over the last
    /// quarter second of playback or live notes.
    pub fn load(&self) -> f32 {
        f32::from_bits(self.shared.load.load(Ordering::Relaxed))
    }

    /// Frames in the device's last block (0 before any stream ran).
    pub fn block(&self) -> u32 {
        self.shared.block.load(Ordering::Relaxed)
    }

    /// The output the playback or live stream last opened: device name, sample rate, buffer length asked for.
    pub fn opened(&self) -> Option<(String, u32, Option<u32>)> {
        self.shared.opened.lock().ok()?.clone()
    }

    /// Shows as playing without starting anything: what the UI sees between Play and the first sound (tests have no
    /// device, so a real Play stops again).
    #[cfg(test)]
    pub(crate) fn pretend_playing(&self) {
        self.shared.state.store(PLAYING, Ordering::Release);
    }

    /// Streams open now, fading ones included (each holds its voice counter in `Shared::engine_voices`).
    #[cfg(test)]
    fn streams(&self) -> usize {
        self.shared.engine_voices.lock().map_or(0, |v| v.iter().filter(|w| w.strong_count() > 0).count())
    }
}

/// Whole ticks handed from a stream's feeder thread to its device callback: one writer, one reader, no locks. With
/// each tick the feeder records where it starts in the queue, its tick and the engine's frame there, so the callback
/// can publish the position of what is being heard.
struct Queue {
    samples: Box<[AtomicU32]>,
    /// Samples written / read since the start.
    written: AtomicU64,
    read: AtomicU64,
    /// Per tick: first sample, length in samples, tick (f64 bits), engine frame.
    ticks: Box<[[AtomicU64; 4]]>,
    ticks_written: AtomicU64,
    /// The tick record the reader is in.
    ticks_read: AtomicU64,
    /// The engine has played everything out (a live engine starts again with its next note).
    ended: AtomicBool,
    /// Largest number of frames the device asked for at once (0 until it asked).
    block: AtomicUsize,
}

impl Queue {
    fn new() -> Queue {
        Queue {
            samples: (0..QUEUE_SAMPLES).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
            ticks: (0..QUEUE_TICKS).map(|_| Default::default()).collect(),
            ticks_written: AtomicU64::new(0),
            ticks_read: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            block: AtomicUsize::new(0),
        }
    }

    /// Samples waiting.
    fn queued(&self) -> u64 {
        self.written.load(Ordering::Acquire).saturating_sub(self.read.load(Ordering::Acquire))
    }

    /// Room for one more tick.
    fn has_room(&self) -> bool {
        self.queued() as usize + TICK_ROOM <= QUEUE_SAMPLES
            && self.ticks_written.load(Ordering::Relaxed) - self.ticks_read.load(Ordering::Acquire) < QUEUE_TICKS as u64 - 1
    }

    /// Feeder: appends one tick (call `has_room` first).
    fn push(&self, s: &[f32], tick: u64, frame: u64) {
        let w = self.written.load(Ordering::Relaxed);
        for (i, &v) in s.iter().enumerate() {
            self.samples[(w as usize + i) % QUEUE_SAMPLES].store(v.to_bits(), Ordering::Relaxed);
        }
        let tw = self.ticks_written.load(Ordering::Relaxed);
        let rec = &self.ticks[tw as usize % QUEUE_TICKS];
        rec[0].store(w, Ordering::Relaxed);
        rec[1].store(s.len() as u64, Ordering::Relaxed);
        rec[2].store((tick as f64).to_bits(), Ordering::Relaxed);
        rec[3].store(frame, Ordering::Relaxed);
        self.ticks_written.store(tw + 1, Ordering::Release);
        self.written.store(w + s.len() as u64, Ordering::Release);
    }

    /// Callback: copies up to `out.len()` samples and returns how many.
    fn pop(&self, out: &mut [f32]) -> usize {
        let r = self.read.load(Ordering::Relaxed);
        let w = self.written.load(Ordering::Acquire);
        let n = (w.saturating_sub(r) as usize).min(out.len());
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = f32::from_bits(self.samples[(r as usize + i) % QUEUE_SAMPLES].load(Ordering::Relaxed));
        }
        self.read.store(r + n as u64, Ordering::Release);
        n
    }

    /// Callback: the engine frame and the tick of the next sample to be read (the position being heard).
    fn heard(&self) -> Option<(u64, f64)> {
        let r = self.read.load(Ordering::Relaxed);
        let tw = self.ticks_written.load(Ordering::Acquire);
        let mut at = self.ticks_read.load(Ordering::Relaxed);
        while at + 1 < tw && self.ticks[(at + 1) as usize % QUEUE_TICKS][0].load(Ordering::Relaxed) <= r {
            at += 1;
        }
        self.ticks_read.store(at, Ordering::Release);
        if at >= tw {
            return None;
        }
        let rec = &self.ticks[at as usize % QUEUE_TICKS];
        let (start, len) = (rec[0].load(Ordering::Relaxed), rec[1].load(Ordering::Relaxed).max(1));
        let into = r.saturating_sub(start);
        Some((rec[3].load(Ordering::Relaxed) + into / 2, f64::from_bits(rec[2].load(Ordering::Relaxed)) + into as f64 / len as f64))
    }
}

/// Renders a stream's engine into its queue, ahead of the device (on its own thread; tests call `fill` directly).
struct Feeder {
    engine: Arc<Mutex<Engine>>,
    queue: Arc<Queue>,
    shared: Arc<Shared>,
    ctl: Arc<Ctl>,
    /// Publish the mixer meters and the load (playback and live streams, not previews).
    meters: bool,
    rate: u32,
    /// Time spent rendering and frames rendered since the load was last published.
    busy: Duration,
    rendered: u64,
    /// The engine's voices after the last fill (registered in `Shared::engine_voices`).
    voices: Arc<AtomicU32>,
}

impl Feeder {
    /// Renders whole ticks until the queue holds one device block (`block` frames before the device has asked) and
    /// `AHEAD_TICKS` ticks more, or the engine has played out.
    fn fill(&mut self, block: usize) {
        let block = 2 * block.max(self.queue.block.load(Ordering::Relaxed)) as u64;
        let Ok(mut e) = self.engine.lock() else { return };
        // A paused (or stopped) stream renders no more: its fade plays from what is queued, and the engine starts no
        // note that another stream (live notes, an open editor) would then hear ringing at the instruments.
        if self.ctl.paused.load(Ordering::Relaxed) && self.queue.queued() > 0 {
            return;
        }
        let t0 = Instant::now();
        let mut frames = 0u64;
        let mut tick_len = 0u64;
        while self.queue.queued() < block + AHEAD_TICKS * tick_len && self.queue.has_room() {
            match e.next_tick() {
                None => {
                    self.queue.ended.store(true, Ordering::Release);
                    break;
                }
                Some((s, tick, frame)) => {
                    tick_len = s.len() as u64;
                    frames += tick_len / 2;
                    self.queue.push(s, tick, frame);
                    self.queue.ended.store(e.finished(), Ordering::Release);
                }
            }
        }
        self.voices.store(e.voice_count() as u32, Ordering::Relaxed);
        // A hosted plugin that missed a tick (busy elsewhere) is heard as a gap: a dropout like an empty block.
        let missed = e.take_missed();
        if missed > 0 {
            self.shared.underruns.fetch_add(missed, Ordering::Relaxed);
        }
        if frames > 0 && self.meters {
            self.busy += t0.elapsed();
            self.rendered += frames;
            if self.rendered >= u64::from(self.rate / 4) {
                let load = self.busy.as_secs_f64() * f64::from(self.rate) / self.rendered as f64;
                self.shared.load.store((load as f32).to_bits(), Ordering::Relaxed);
                (self.busy, self.rendered) = (Duration::ZERO, 0);
            }
            if let Ok(mut v) = self.shared.tracks.lock() {
                e.take_track_peaks(&mut v);
            }
        }
        // Lit keys run ahead of what is heard by the queue's length (a few milliseconds).
        if frames > 0 && self.ctl.report.load(Ordering::Relaxed) {
            if let Ok(mut v) = self.shared.sounding.lock() {
                v.clear();
                if !self.ctl.paused.load(Ordering::Relaxed) {
                    v.extend(e.sounding().map(|(c, k)| (c as u16, k)));
                }
            }
        }
    }
}

/// A stream's feeder thread: woken by the device callback after each block; stopped and joined when dropped.
struct FeedThread {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FeedThread {
    fn spawn(mut feeder: Feeder) -> FeedThread {
        let stop = Arc::new(AtomicBool::new(false));
        let handle = std::thread::Builder::new()
            .name("lemonstudio-feeder".into())
            .spawn({
                let stop = stop.clone();
                move || {
                    raise_priority();
                    while !stop.load(Ordering::Acquire) {
                        feeder.fill(0);
                        std::thread::park_timeout(Duration::from_millis(5));
                    }
                }
            })
            .expect("spawn feeder thread");
        FeedThread { stop, handle: Some(handle) }
    }

    fn thread(&self) -> Option<std::thread::Thread> {
        self.handle.as_ref().map(|h| h.thread().clone())
    }

    /// Renders now (a live note arrived).
    fn wake(&self) {
        if let Some(h) = &self.handle {
            h.thread().unpark();
        }
    }
}

impl Drop for FeedThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            h.thread().unpark();
            let _ = h.join();
        }
    }
}

/// The feeder renders for the device: it runs above the UI.
#[cfg(windows)]
fn raise_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

/// macOS: the user-interactive class keeps the feeder on the performance cores.
#[cfg(target_os = "macos")]
fn raise_priority() {
    // SAFETY: changes the calling thread's own QoS class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn raise_priority() {}

fn attach(e: &mut Engine, inst: &Instruments) {
    if let Ok(list) = inst.lock() {
        if std::env::var_os("OS_DEBUG").is_some() {
            eprintln!("[transport] attach {} instrument(s), {} effect(s)", list.inst.len(), list.fx.len());
        }
        for (row, i) in list.inst.iter() {
            e.set_instrument(*row, i.clone());
        }
        for (track, slot, fx) in list.fx.iter() {
            e.set_effect(*track, *slot, fx.clone());
        }
    }
}

/// Renders a whole song (or one pass of a pattern) with the hosted plugins and writes it to `path`; returns its length
/// in seconds. Runs on the audio thread with no stream open.
// spec: audio/export.md
fn export(project: &Project, mode: PlayMode, rate: u32, path: &std::path::Path, library: &mut SampleLibrary, inst: &Instruments, shared: &Shared) -> Result<f64, String> {
    let mut e = Engine::new(project, library, rate, mode);
    attach(&mut e, inst);
    let len = e.length().max(1);
    let cap = len + u64::from(rate) * EXPORT_TAIL_MAX_SECS;
    let mut audio: Vec<f32> = Vec::with_capacity(2 * (len as usize + rate as usize));
    loop {
        let done = e.render_ticks(32, &mut audio);
        let frames = audio.len() as u64 / 2;
        shared.export_permille.store((frames * 1000 / len).min(999) as u32, Ordering::Relaxed);
        if done || frames >= cap {
            break;
        }
    }
    drop(e);
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut w = std::io::BufWriter::new(file);
    os_engine::wav::write_f32_stereo(&mut w, rate, &audio).and_then(|_| std::io::Write::flush(&mut w)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(audio.len() as f64 / 2.0 / f64::from(rate))
}

/// Silences every hosted instrument (voices, tails, queued notes) and effect (tails, delay lines). Only called while no
/// stream processes them.
fn reset_instruments(inst: &Instruments) {
    reset_hosts(inst, true);
}

/// Resets every hosted instrument, and every hosted effect when `effects`. Only called while no stream processes them.
///
/// A start of playback (Play, a jump while playing) calls it with "Reset plugins on transport" (spec:
/// ui/settings-choices.md "Transport: Play truncated notes and Reset plugins"): on, every hosted plugin is reset before
/// the first block at the new place; off, the effects are left alone, so a delay's tail rings on through the start. The
/// instruments are reset either way, as at Stop: the new engine does not know the notes the old one held at them (a
/// Lemon Studio choice; what FL does to an instrument's voices at a jump is not measured).
fn reset_hosts(inst: &Instruments, effects: bool) {
    let (list, fx): (Vec<SharedInstrument>, Vec<SharedEffect>) = inst
        .lock()
        .map(|l| (l.inst.iter().map(|(_, i)| i.clone()).collect(), l.fx.iter().filter(|_| effects).map(|(_, _, f)| f.clone()).collect()))
        .unwrap_or_default();
    for i in list {
        if let Ok(mut i) = i.lock() {
            i.reset();
        }
    }
    for f in fx {
        if let Ok(mut f) = f.lock() {
            f.reset_fx();
        }
    }
}

fn audio_thread(rx: Receiver<Cmd>, shared: Arc<Shared>, latest: Arc<Mutex<Option<Project>>>, inst: Instruments, mut library: SampleLibrary) {
    // The playback: the pattern or the playlist, running or paused.
    let mut session: Option<Session> = None;
    // One-shot note previews play on their own stream, so they also sound while playing or paused.
    let mut preview: Option<Session> = None;
    // When a time-limited preview (browser samples) is faded out.
    let mut preview_until: Option<Instant> = None;
    // Live notes while stopped or paused: an engine with no schedule on a stream that stays open.
    let mut live: Option<Session> = None;
    let mut live_project: Option<Project> = None;
    // Streams that were stopped and finish their fade before they are dropped.
    let mut closing: Vec<(Session, Instant)> = Vec::new();
    let mut reset_pending = false;
    // An instrument's editor is open: the live stream stays open while nothing plays (`Transport::listen`). A stream
    // that would not open is tried again on the next change of state, not every loop.
    let mut listen = false;
    let mut listen_retry = false;
    // Auto close (`Transport::close_device`): no stream opens while `closed`; the playback waits in `held` (project,
    // mode, generation, where).
    let mut closed = false;
    let mut held: Option<(Project, PlayMode, u64, At)> = None;
    let swap = |s: &mut Session, library: &mut SampleLibrary| {
        // Build outside the lock, then swap at the playing position.
        let mut e = Engine::new(&s.project, library, s.rate, s.mode);
        if s.kind == Kind::Live {
            e.clear_schedule();
        }
        attach(&mut e, &inst);
        let old = s.engine.lock().map(|mut slot| {
            e.continue_from(&mut slot);
            std::mem::replace(&mut *slot, e)
        });
        drop(old); // freed after the audio callback has the new engine
    };
    let report = |shared: &Shared, e: String| {
        if let Ok(mut slot) = shared.error.lock() {
            *slot = Some(e);
        }
    };
    // The live stream: live notes, previews of plugin channels, and the instruments while an editor is open.
    let open_live = |p: &Project, awake: bool, library: &mut SampleLibrary| {
        let s = Session::start(p, PlayMode::Pattern(1), Kind::Live, 0, library, shared.clone(), &inst, At::default())?;
        if let Ok(mut e) = s.engine.lock() {
            e.set_awake(awake);
        }
        Ok::<Session, String>(s)
    };
    // A paused playback lets go of the notes it holds at the instruments once the live stream processes them (live
    // notes, an open editor): there they would ring for as long as it is paused.
    let release_paused = |session: &Option<Session>| {
        if let Some(s) = session.as_ref().filter(|s| s.paused()) {
            if let Ok(mut e) = s.engine.lock() {
                e.release_instruments();
            }
        }
    };
    let retire = |s: Session, closing: &mut Vec<(Session, Instant)>| {
        s.ctl.report.store(false, Ordering::Relaxed);
        s.ctl.paused.store(true, Ordering::Relaxed);
        closing.push((s, Instant::now()));
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(10)) {
            // The output is closed by Auto close: previews and live notes find no stream and open none.
            Ok(Cmd::Preview(_) | Cmd::PreviewFor(..) | Cmd::PreviewSong(_) | Cmd::NoteOn(..) | Cmd::TapClick(_)) if closed => {}
            Ok(Cmd::Update) => {
                if let Some(project) = latest.lock().ok().and_then(|mut l| l.take()) {
                    for s in [session.as_mut(), live.as_mut()].into_iter().flatten() {
                        s.project = project.clone();
                        swap(s, &mut library);
                    }
                    if let Some(h) = held.as_mut() {
                        h.0 = project.clone();
                    }
                    live_project = Some(project);
                }
            }
            Ok(Cmd::Instruments) => {
                for s in [session.as_mut(), live.as_mut()].into_iter().flatten() {
                    swap(s, &mut library);
                }
            }
            Ok(Cmd::Listen(on)) => {
                listen = on;
                listen_retry = true;
                if let Some(s) = &live {
                    if let Ok(mut e) = s.engine.lock() {
                        e.set_awake(on);
                    }
                    s.feed.wake();
                }
            }
            Ok(cmd @ (Cmd::NoteOn(..) | Cmd::TapClick(_))) => {
                // A click needs no channel: it also opens the live stream before any project was sent.
                let empty = Project::default();
                let project = live_project.as_ref().or(matches!(cmd, Cmd::TapClick(_)).then_some(&empty));
                let target = match session.as_ref() {
                    Some(s) if !s.paused() => Some(s),
                    _ => {
                        if live.is_none() {
                            if let Some(p) = project {
                                match open_live(p, listen, &mut library) {
                                    Ok(s) => live = Some(s),
                                    Err(e) => report(&shared, e),
                                }
                            }
                        }
                        release_paused(&session);
                        live.as_ref()
                    }
                };
                if let Some(s) = target {
                    if let Ok(mut e) = s.engine.lock() {
                        match cmd {
                            Cmd::NoteOn(ch, key, vel) => e.note_on(ch, key, vel),
                            Cmd::TapClick(accent) => e.tap_click(accent),
                            _ => {}
                        }
                    }
                    s.feed.wake();
                }
            }
            Ok(Cmd::NoteOff(ch, key)) => {
                for s in [session.as_ref(), live.as_ref()].into_iter().flatten() {
                    if let Ok(mut e) = s.engine.lock() {
                        e.note_off(ch, key);
                    }
                    s.feed.wake();
                }
            }
            Ok(Cmd::PreviewFor(project, limit)) => {
                preview = None;
                preview_until = limit.map(|d| Instant::now() + d);
                match Session::start(&project, PlayMode::Pattern(1), Kind::Preview, 0, &mut library, shared.clone(), &inst, At::default()) {
                    Ok(s) => preview = Some(s),
                    Err(e) => report(&shared, e),
                }
            }
            Ok(Cmd::PreviewSong(project)) => {
                preview = None;
                preview_until = None;
                match Session::start(&project, PlayMode::Song, Kind::Preview, 0, &mut library, shared.clone(), &inst, At::default()) {
                    Ok(s) => preview = Some(s),
                    Err(e) => report(&shared, e),
                }
            }
            Ok(Cmd::Preview(project)) => {
                // A plugin channel's preview plays as a timed live note on the stream that hosts the instrument.
                let note = project.patterns.first().and_then(|p| p.notes.first()).copied();
                let row = note.and_then(|n| project.channels.iter().position(|c| c.index == n.channel));
                let hosted = row.is_some_and(|r| inst.lock().is_ok_and(|l| l.inst.iter().any(|(i, _)| *i == r)));
                if let (true, Some(n), Some(r)) = (hosted, note, row) {
                    let running = session.as_ref().is_some_and(|s| !s.paused());
                    if !running && live.is_none() {
                        if let Some(p) = live_project.as_ref().or(Some(&*project)) {
                            match open_live(p, listen, &mut library) {
                                Ok(s) => live = Some(s),
                                Err(e) => report(&shared, e),
                            }
                        }
                    }
                    let target = match session.as_ref() {
                        Some(s) if !s.paused() => Some(s),
                        _ => {
                            release_paused(&session);
                            live.as_ref()
                        }
                    };
                    if let Some(s) = target {
                        let tempo = project.tick_tempo(project.tempo.unwrap_or(140.0));
                        let frames = os_engine::laws::tick_to_frame(f64::from(n.length.max(1)), project.ppq.max(1), tempo, s.rate);
                        if let Ok(mut e) = s.engine.lock() {
                            e.note_for(r, n.key, n.velocity, frames);
                        }
                        s.feed.wake();
                    }
                    continue;
                }
                preview = None;
                preview_until = None;
                match Session::start(&project, PlayMode::Pattern(1), Kind::Preview, 0, &mut library, shared.clone(), &inst, At::default()) {
                    Ok(s) => preview = Some(s),
                    Err(e) => report(&shared, e),
                }
            }
            Ok(Cmd::Play(project, mode, start, generation, starts)) => {
                // A newer request (stop, another play) already superseded this one.
                if generation != shared.generation.load(Ordering::Acquire) {
                    continue;
                }
                // A Play asked for while Auto close holds the output opens it (and drops the playback it held).
                closed = false;
                held = None;
                // The old playback is replaced, not faded; of what it leaves ringing only the hosted effects' tails
                // go on, with "Reset plugins on transport" off (`reset_hosts`). A live stream left open would process
                // the same plugin instances a second time (each stream would only hear every other chunk of a ringing
                // sample): typed notes go into the playing engine instead.
                session = None;
                live = None;
                closing.clear();
                // A jump while playing arrives as a Stop and this Play: the Stop's pending reset gives way to this one.
                reset_hosts(&inst, starts.reset);
                reset_pending = false;
                listen_retry = true;
                live_project = Some((*project).clone());
                let at = At { tick: start, paused: false, truncated: starts.truncated };
                match Session::start(&project, mode, Kind::Song, generation, &mut library, shared.clone(), &inst, at) {
                    Ok(s) => session = Some(s),
                    Err(e) => {
                        report(&shared, e);
                        let _ = shared.state.compare_exchange(PLAYING, STOPPED, Ordering::AcqRel, Ordering::Acquire);
                        shared.rewind(0.0);
                    }
                }
            }
            Ok(Cmd::Reopen) => {
                live = None;
                listen_retry = true;
                preview = None;
                preview_until = None;
                closing.clear();
                if let Some(old) = session.take() {
                    let at = At { tick: f64::from_bits(shared.tick.load(Ordering::Relaxed)), paused: old.paused(), truncated: old.truncated };
                    let (project, mode, generation) = (old.project.clone(), old.mode, old.generation);
                    drop(old);
                    reset_instruments(&inst);
                    match Session::start(&project, mode, Kind::Song, generation, &mut library, shared.clone(), &inst, at) {
                        Ok(s) => session = Some(s),
                        Err(e) => {
                            report(&shared, e);
                            if generation == shared.generation.load(Ordering::Acquire) {
                                shared.state.store(STOPPED, Ordering::Release);
                                shared.rewind(0.0);
                            }
                        }
                    }
                }
            }
            Ok(Cmd::Pause) => {
                if let Some(s) = &session {
                    s.ctl.paused.store(true, Ordering::Relaxed);
                }
                if listen || live.is_some() {
                    release_paused(&session);
                }
                listen_retry = true;
            }
            Ok(Cmd::Resume) => {
                // As for Play: the live stream must not process the same plugin instances as the playback. Its held
                // notes are released (the playback delivers their note offs).
                if session.is_some() {
                    live = None;
                }
                if let Some(s) = &session {
                    s.ctl.paused.store(false, Ordering::Relaxed);
                }
            }
            Ok(Cmd::Stop) => {
                for s in [session.take(), live.take(), preview.take()].into_iter().flatten() {
                    retire(s, &mut closing);
                }
                held = None;
                reset_pending = true;
                listen_retry = true;
            }
            Ok(Cmd::Close) => {
                closed = true;
                // The playback (paused by the app) is held where it was heard; every stream fades out and closes, then
                // the instruments are reset as after Stop (without the rewind: the transport is paused).
                if let Some(s) = session.take() {
                    let at = At { tick: f64::from_bits(shared.tick.load(Ordering::Relaxed)), paused: true, truncated: s.truncated };
                    held = Some((s.project.clone(), s.mode, s.generation, at));
                    retire(s, &mut closing);
                    reset_pending = true;
                }
                for s in [live.take(), preview.take()].into_iter().flatten() {
                    retire(s, &mut closing);
                    reset_pending = true;
                }
                preview_until = None;
            }
            Ok(Cmd::Open) => {
                closed = false;
                listen_retry = true;
                // The held playback goes on at its place, unless a Stop or another Play came since; playing if the
                // transport was resumed meanwhile, else paused (`resume` follows).
                if let Some((project, mode, generation, mut at)) = held.take().filter(|h| h.2 == shared.generation.load(Ordering::Acquire)) {
                    closing.clear();
                    if reset_pending {
                        reset_pending = false;
                        reset_instruments(&inst);
                    }
                    at.paused = shared.state.load(Ordering::Acquire) != PLAYING;
                    match Session::start(&project, mode, Kind::Song, generation, &mut library, shared.clone(), &inst, at) {
                        Ok(s) => session = Some(s),
                        Err(e) => {
                            report(&shared, e);
                            shared.state.store(STOPPED, Ordering::Release);
                            shared.rewind(0.0);
                        }
                    }
                }
            }
            Ok(Cmd::Export(project, mode, rate, path)) => {
                // Every stream that could process the plugins goes at once (no fade), and they start from silence.
                session = None;
                live = None;
                preview = None;
                preview_until = None;
                held = None;
                closing.clear();
                reset_instruments(&inst);
                reset_pending = false;
                shared.rewind(0.0);
                let result = export(&project, mode, rate, &path, &mut library, &inst, &shared);
                reset_instruments(&inst);
                if let Ok(mut slot) = shared.export_result.lock() {
                    *slot = Some(result);
                }
                shared.exporting.store(false, Ordering::Release);
                listen_retry = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if preview.as_ref().is_some_and(|p| p.done.load(Ordering::Relaxed)) {
            preview = None;
        }
        if preview_until.is_some_and(|t| Instant::now() >= t) {
            preview_until = None;
            if let Some(s) = preview.take() {
                retire(s, &mut closing);
            }
        }
        // Stopped streams are dropped once their fade is over; then the instruments are silenced for good.
        closing.retain(|(_, since)| since.elapsed() < CLOSE_AFTER);
        if reset_pending && closing.is_empty() {
            reset_pending = false;
            reset_instruments(&inst);
            if session.is_none() && shared.state.load(Ordering::Acquire) == STOPPED {
                shared.rewind(0.0);
            }
        }
        // While an editor is open and nothing plays, the live stream processes the instruments (once the stopped
        // streams are gone and the instruments reset).
        if listen && listen_retry && !closed && live.is_none() && !reset_pending && closing.is_empty() && session.as_ref().is_none_or(|s| s.paused()) {
            listen_retry = false;
            if let Some(p) = live_project.as_ref() {
                release_paused(&session);
                match open_live(p, true, &mut library) {
                    Ok(s) => live = Some(s),
                    Err(e) => eprintln!("lemonstudio: live stream for the open plugin editor: {e}"),
                }
            }
        }
        // A pattern loops inside the engine; a song ends when it has played out and everything has rung out.
        if session.as_ref().is_some_and(|s| s.done.load(Ordering::Relaxed)) {
            if let Some(s) = session.take() {
                if s.generation == shared.generation.load(Ordering::Acquire) {
                    shared.state.store(STOPPED, Ordering::Release);
                    shared.rewind(0.0);
                }
            }
            listen_retry = true;
        }
    }
}

/// Where a new stream starts: the tick to seek to before anything is rendered, whether it starts paused, and whether
/// the notes under way there sound ("Play truncated notes on transport", `Starts`).
#[derive(Clone, Copy, Default)]
struct At {
    tick: f64,
    paused: bool,
    truncated: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Playback (pattern or song): reports position, peaks and sounding notes; patterns loop.
    Song,
    /// A one-shot note preview that leaves the song position and play state alone.
    Preview,
    /// Live notes only (no schedule); never finishes.
    Live,
}

/// Controls shared with a stream's callback.
struct Ctl {
    /// Fade out and hold the engine (pause, stop).
    paused: AtomicBool,
    /// Publish position, meters and sounding notes (the playback stream until it is stopped).
    report: AtomicBool,
}

struct Session {
    _stream: cpal::Stream,
    /// Renders ahead of the stream; stopped after the stream (fields drop in order).
    feed: FeedThread,
    kind: Kind,
    engine: Arc<Mutex<Engine>>,
    done: Arc<AtomicBool>,
    ctl: Arc<Ctl>,
    /// The play request this session answers.
    generation: u64,
    /// The project playing (live edits replace it).
    project: Project,
    mode: PlayMode,
    rate: u32,
    /// "Play truncated notes on transport" as this playback started with (a move to new audio settings keeps it).
    truncated: bool,
}

impl Session {
    fn paused(&self) -> bool {
        self.ctl.paused.load(Ordering::Relaxed)
    }

    /// Opens a stream with the audio settings and starts it at `at` (playback: the position is published at once).
    #[allow(clippy::too_many_arguments)]
    fn start(project: &Project, mode: PlayMode, kind: Kind, generation: u64, library: &mut SampleLibrary, shared: Arc<Shared>, inst: &Instruments, at: At) -> Result<Session, String> {
        let song = kind == Kind::Song;
        let out = crate::audio_cfg::output(&crate::audio_cfg::AudioCfg::current())?;
        let (device, format, mut config) = (out.device, out.format, out.config);
        let rate = config.sample_rate;
        let mut e = Engine::new(project, library, rate, mode);
        // Playback loops: a pattern on its bar line, the song at its end (back to the start).
        e.set_looping(song);
        if kind == Kind::Live {
            e.clear_schedule();
        }
        if kind != Kind::Preview {
            attach(&mut e, inst);
        }
        // Playback clicks with the metronome switch; live notes and previews never do (spec: ui/metronome.md §4).
        if song {
            e.set_metronome(crate::metronome::switch());
        }
        // Seek before anything is queued, so the first block is already the start position.
        if at.tick > 0.0 {
            e.set_truncated_on_transport(at.truncated);
            e.seek_tick(at.tick as u64);
            if song {
                let tempo = project.tick_tempo(project.tempo.unwrap_or(140.0));
                shared.position.store(os_engine::laws::tick_to_frame(at.tick, project.ppq.max(1), tempo, rate), Ordering::Relaxed);
            }
        }
        let engine = Arc::new(Mutex::new(e));
        let done = Arc::new(AtomicBool::new(false));
        let ctl = Arc::new(Ctl { paused: AtomicBool::new(at.paused), report: AtomicBool::new(song) });
        let fixed = match config.buffer_size {
            BufferSize::Fixed(b) => Some(b),
            BufferSize::Default => None,
        };
        if kind != Kind::Preview {
            shared.sample_rate.store(rate, Ordering::Relaxed);
            if let Ok(mut o) = shared.opened.lock() {
                *o = Some((out.name.clone(), rate, fixed));
            }
        }
        // The queue gets a first block before the stream starts; then the feeder keeps it filled.
        let queue = Arc::new(Queue::new());
        let mut feeder = Feeder::new(engine.clone(), queue.clone(), shared.clone(), ctl.clone(), rate);
        feeder.meters = kind != Kind::Preview;
        feeder.fill(PREFILL_FRAMES.max(fixed.unwrap_or(0) as usize));
        let feed = FeedThread::spawn(feeder);
        let renderer = || {
            let mut r = Renderer::new(queue.clone(), ctl.clone(), done.clone(), shared.clone(), rate);
            r.tap = kind != Kind::Preview;
            r.wake = feed.thread();
            r
        };
        let mut stream = build_any(&device, format, &config, renderer());
        if stream.is_err() && fixed.is_some() {
            // A buffer length the device refuses: its own.
            config.buffer_size = BufferSize::Default;
            stream = build_any(&device, format, &config, renderer());
        }
        let stream = stream?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Session { _stream: stream, feed, kind, engine, done, ctl, generation, project: project.clone(), mode, rate, truncated: at.truncated })
    }
}

/// An output stream in the device's sample format.
fn build_any(device: &cpal::Device, format: SampleFormat, config: &StreamConfig, renderer: Renderer) -> Result<cpal::Stream, String> {
    match format {
        SampleFormat::F32 => build::<f32>(device, config, renderer),
        SampleFormat::I16 => build::<i16>(device, config, renderer),
        SampleFormat::I32 => build::<i32>(device, config, renderer),
        SampleFormat::U16 => build::<u16>(device, config, renderer),
        other => Err(format!("unsupported output sample format {other:?}")),
    }
}

impl Feeder {
    fn new(engine: Arc<Mutex<Engine>>, queue: Arc<Queue>, shared: Arc<Shared>, ctl: Arc<Ctl>, rate: u32) -> Feeder {
        let voices = Arc::new(AtomicU32::new(0));
        if let Ok(mut v) = shared.engine_voices.lock() {
            v.retain(|w| w.strong_count() > 0);
            v.push(Arc::downgrade(&voices));
        }
        Feeder { engine, queue, shared, ctl, meters: true, rate, busy: Duration::ZERO, rendered: 0, voices }
    }
}

/// What one output stream does per block: copy the queued ticks, fade them for pause / stop, publish the position.
/// (Separate from the device so it can be tested without one.)
struct Renderer {
    queue: Arc<Queue>,
    /// The feeder thread, woken after each block; tests fill the queue in the callback instead (`inline`).
    wake: Option<std::thread::Thread>,
    inline: Option<Feeder>,
    ctl: Arc<Ctl>,
    done: Arc<AtomicBool>,
    shared: Arc<Shared>,
    /// Output gain, ramped toward 1 (playing) or 0 (paused) by `step` per frame.
    gain: f32,
    step: f32,
    /// Feed the monitor tap with what this stream outputs.
    tap: bool,
}

impl Renderer {
    fn new(queue: Arc<Queue>, ctl: Arc<Ctl>, done: Arc<AtomicBool>, shared: Arc<Shared>, rate: u32) -> Renderer {
        let step = (1.0 / (f64::from(rate.max(1)) * FADE_SECONDS)) as f32;
        // A stream that starts paused (moved to new audio settings while paused) starts silent.
        let gain = if ctl.paused.load(Ordering::Relaxed) { 0.0 } else { 1.0 };
        Renderer { queue, wake: None, inline: None, ctl, done, shared, gain, step, tap: false }
    }

    /// Renders one block (see `render_block`) and copies it to the monitor tap, unless the stream is held.
    fn render(&mut self, stereo: &mut [f32]) {
        let held = self.gain == 0.0 && self.ctl.paused.load(Ordering::Relaxed);
        self.render_block(stereo);
        if self.tap && !held {
            if let Ok(mut t) = self.shared.tap.try_lock() {
                for f in stereo.chunks_exact(2) {
                    t.push((f[0] + f[1]) * 0.5);
                }
            }
        }
    }

    /// Plays `stereo.len() / 2` frames (interleaved L/R) from the queue. A paused stream, once faded out, reads
    /// nothing, so its position and the queued ticks stay exactly where they were.
    fn render_block(&mut self, stereo: &mut [f32]) {
        let target = if self.ctl.paused.load(Ordering::Relaxed) { 0.0 } else { 1.0 };
        if self.gain == 0.0 && target == 0.0 {
            stereo.fill(0.0);
            return;
        }
        self.queue.block.fetch_max(stereo.len() / 2, Ordering::Relaxed);
        if self.tap {
            self.shared.block.store((stereo.len() / 2) as u32, Ordering::Relaxed);
        }
        if let Some(f) = &mut self.inline {
            f.fill(0);
        }
        let got = self.queue.pop(stereo);
        let ended = self.queue.ended.load(Ordering::Acquire);
        if got < stereo.len() {
            stereo[got..].fill(0.0);
            if !ended {
                self.shared.underruns.fetch_add(1, Ordering::Relaxed);
            }
        }
        if let Some(t) = &self.wake {
            t.unpark();
        }
        if ended && self.queue.queued() == 0 {
            self.done.store(true, Ordering::Relaxed);
            if got == 0 {
                return;
            }
        }
        // Every stream frees the tick records it has played (`heard`); only playback publishes where it is.
        let heard = self.queue.heard();
        if self.ctl.report.load(Ordering::Relaxed) {
            if let Some((frame, tick)) = heard {
                self.shared.position.store(frame, Ordering::Relaxed);
                self.shared.tick.store(tick.to_bits(), Ordering::Relaxed);
            }
        }
        if self.gain != 1.0 || target != 1.0 {
            for f in stereo.chunks_exact_mut(2) {
                self.gain = if self.gain < target { (self.gain + self.step).min(target) } else { (self.gain - self.step).max(target) };
                f[0] *= self.gain;
                f[1] *= self.gain;
            }
        }
    }
}

fn build<T>(device: &cpal::Device, config: &StreamConfig, mut renderer: Renderer) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let mut stereo: Vec<f32> = Vec::with_capacity(8192);
    device
        .build_output_stream::<T, _, _>(
            config.clone(),
            move |out: &mut [T], _| {
                let frames = out.len() / channels;
                stereo.resize(frames * 2, 0.0);
                renderer.render(&mut stereo);
                let (mut pl, mut pr) = (0.0f32, 0.0f32);
                for (frame, s) in out.chunks_mut(channels).zip(stereo.chunks_exact(2)) {
                    pl = pl.max(s[0].abs());
                    pr = pr.max(s[1].abs());
                    for (c, slot) in frame.iter_mut().enumerate() {
                        let v = if c < 2 { s[c] } else { 0.0 };
                        *slot = T::from_sample(v.clamp(-1.0, 1.0));
                    }
                }
                // Meter peaks: the largest since the monitor last read them (non-negative f32 bits order like u32s).
                if renderer.ctl.report.load(Ordering::Relaxed) {
                    renderer.shared.peak_l.store(pl.to_bits(), Ordering::Relaxed);
                    renderer.shared.peak_r.store(pr.to_bits(), Ordering::Relaxed);
                }
                // Toolbar meter: the largest peak since the monitor last read it (non-negative f32 bits order like
                // u32s), from every stream the monitor taps (playback and live notes).
                if renderer.tap {
                    renderer.shared.meter_l.fetch_max(pl.to_bits(), Ordering::Relaxed);
                    renderer.shared.meter_r.fetch_max(pr.to_bits(), Ordering::Relaxed);
                }
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looping_engine() -> (Engine, Project) {
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern {
            number: 1,
            notes: (0..8).map(|i| os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, i * 48) }).collect(),
            ..Default::default()
        }];
        project.tempo = Some(130.0);
        let mut library = SampleLibrary::default();
        let mut e = Engine::new(&project, &mut library, 48000, PlayMode::Pattern(1));
        e.set_looping(true);
        (e, project)
    }

    /// A stream's callback with its queue filled in the callback (no feeder thread).
    fn renderer(e: Engine) -> (Renderer, Arc<Ctl>, Arc<Shared>) {
        let ctl = Arc::new(Ctl { paused: AtomicBool::new(false), report: AtomicBool::new(true) });
        let shared = Arc::new(Shared::default());
        let queue = Arc::new(Queue::new());
        let mut r = Renderer::new(queue.clone(), ctl.clone(), Arc::new(AtomicBool::new(false)), shared.clone(), 48000);
        r.inline = Some(Feeder::new(Arc::new(Mutex::new(e)), queue, shared.clone(), ctl.clone(), 48000));
        (r, ctl, shared)
    }

    /// The queue hands out exactly what the engine renders, tick after tick, whatever the device's block size; the
    /// published position is that of the sample being heard.
    #[test]
    fn queued_ticks_play_the_engine_sample_for_sample() {
        let (mut want_engine, _) = looping_engine();
        let mut want = vec![0.0f32; 2 * 48000];
        want_engine.process(&mut want);
        let (e, _) = looping_engine();
        let (mut r, _, shared) = renderer(e);
        let mut got = Vec::new();
        let mut at = 0u64;
        for n in [480usize, 441, 1024, 7, 2048, 333].iter().cycle() {
            if got.len() >= want.len() {
                break;
            }
            let mut block = vec![0.0f32; 2 * n];
            r.render(&mut block);
            got.extend_from_slice(&block);
            at += *n as u64;
            assert_eq!(shared.position.load(Ordering::Relaxed), at, "position of the next sample heard");
        }
        assert_eq!(&got[..want.len()], &want[..], "same samples as the engine");
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
        // The queue keeps one block and a few ticks, not more.
        let queued = r.queue.queued() / 2;
        assert!(queued <= 2048 + 3 * 300, "{queued} frames queued");
    }

    /// Streams that publish no position (live notes, previews) still free the tick records they have played: a live
    /// stream used to stop taking ticks after `QUEUE_TICKS` of them (about 18 s), every block after that a dropout.
    #[test]
    fn streams_without_a_position_play_on_past_the_tick_records() {
        let (e, _) = looping_engine();
        let (mut r, ctl, shared) = renderer(e);
        ctl.report.store(false, Ordering::Relaxed);
        let mut block = vec![0.0f32; 2 * 512];
        let mut blocks = 0;
        while r.queue.ticks_written.load(Ordering::Relaxed) < 2 * QUEUE_TICKS as u64 {
            r.render(&mut block);
            blocks += 1;
            assert!(blocks < 100_000, "the queue stopped taking ticks at {}", r.queue.ticks_written.load(Ordering::Relaxed));
        }
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
    }

    /// A feeder that cannot keep up shows as underruns, never as a stall of the callback.
    #[test]
    fn an_empty_queue_counts_an_underrun() {
        let (e, _) = looping_engine();
        let (mut r, _, shared) = renderer(e);
        r.inline = None;
        let mut block = vec![1.0f32; 2 * 256];
        r.render(&mut block);
        assert!(block.iter().all(|v| *v == 0.0));
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 1);
    }

    /// An export renders one pass of the pattern (not a loop) on the audio thread, with progress, into a WAV file.
    #[test]
    fn export_writes_a_wav_file() {
        let (_, project) = looping_engine();
        let t = Transport::new(None);
        let path = std::env::temp_dir().join(format!("openstudio-export-test-{}.wav", std::process::id()));
        t.export(&project, PlayMode::Pattern(1), 48000, path.clone());
        assert!(matches!(t.export_status(), Export::Running(_) | Export::Done(_)), "running at once");
        let t0 = Instant::now();
        let secs = loop {
            match t.export_status() {
                Export::Done(r) => break r.expect("export"),
                _ if t0.elapsed() > Duration::from_secs(30) => panic!("export did not finish"),
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        assert_eq!(t.export_status(), Export::Idle, "the result is handed out once");
        assert_eq!(t.state(), State::Stopped);
        // One bar at 130 BPM is about 1.85 s; the sampler's ring-out adds a little, a loop would never end.
        assert!(secs > 1.8 && secs < 8.0, "length {secs}");
        let bytes = std::fs::read(&path).expect("file written");
        let _ = std::fs::remove_file(&path);
        assert_eq!(&bytes[..4], b"RIFF");
        let frames = (bytes.len() - 44) / 8;
        assert!((frames as f64 / 48000.0 - secs).abs() < 0.01, "file holds {frames} frames");
        let samples = bytes[44..].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]).abs());
        assert!(samples.fold(0.0f32, f32::max) > 0.01, "the notes are heard");
    }

    /// Pause fades out and then holds the engine exactly; resume continues from that frame (no restart, no jump).
    #[test]
    fn pause_holds_and_resume_continues() {
        let (e, _) = looping_engine();
        let (mut r, ctl, shared) = renderer(e);
        let mut block = vec![0.0f32; 512 * 2];
        for _ in 0..20 {
            r.render(&mut block);
        }
        let before = shared.position.load(Ordering::Relaxed);
        assert!(before > 0 && block.iter().any(|v| v.abs() > 0.0), "playing and audible");
        ctl.paused.store(true, Ordering::Relaxed);
        // The fade takes under 300 frames: the second block is silent and the position no longer moves.
        r.render(&mut block);
        r.render(&mut block);
        let held = shared.position.load(Ordering::Relaxed);
        assert!(held >= before && held <= before + 1024, "fade block only: {before} -> {held}");
        for _ in 0..10 {
            r.render(&mut block);
            assert!(block.iter().all(|v| *v == 0.0), "silent while paused");
        }
        assert_eq!(shared.position.load(Ordering::Relaxed), held, "position held");
        ctl.paused.store(false, Ordering::Relaxed);
        r.render(&mut block);
        let after = shared.position.load(Ordering::Relaxed);
        assert!(after > held && after <= held + 512, "continues from {held}, got {after}");
        assert!(shared.sounding.lock().unwrap().len() <= 8);
    }

    /// The tick published to the UI follows the engine (a looping pattern wraps with it).
    #[test]
    fn tick_wraps_with_the_loop() {
        let (e, _) = looping_engine();
        let (mut r, _, shared) = renderer(e);
        let mut block = vec![0.0f32; 4096 * 2];
        let mut last = -1.0f64;
        let mut wrapped = false;
        // One bar at 130 bpm is about 1.85 s = 88_800 frames at 48 kHz.
        for _ in 0..40 {
            r.render(&mut block);
            let t = f64::from_bits(shared.tick.load(Ordering::Relaxed));
            assert!((0.0..384.0 + 1.0).contains(&t), "tick {t} stays inside the one-bar pattern");
            wrapped |= t < last;
            last = t;
        }
        assert!(wrapped, "the tick wrapped at the loop point");
    }

    /// Needs an audio device: `cargo test -p os-app -- --ignored deleted_notes`.
    #[test]
    #[ignore]
    fn deleted_notes_stop_playing() {
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        let pat = os_model::Pattern {
            number: 1,
            notes: (0..8).map(|i| os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, i * 48) }).collect(),
            ..Default::default()
        };
        project.patterns = vec![pat];
        project.tempo = Some(130.0);
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(500));
        assert!(!t.sounding().is_empty() || t.peaks().0 > 0.0, "playing");
        project.patterns[0].notes.clear();
        t.update(&project);
        std::thread::sleep(Duration::from_millis(300));
        let mut loud = 0;
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            if !t.sounding().is_empty() {
                loud += 1;
            }
        }
        assert_eq!(loud, 0, "deleted notes still sounding");
    }

    /// A mixer fader edit silences a ringing note within a few blocks (needs an audio device).
    #[test]
    #[ignore]
    fn fader_edit_is_heard_at_once() {
        let mut project = crate::app::test_project();
        let ch = project.channels[4].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 384, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(250));
        assert!(t.peaks().0 > 0.01, "pluck rings: {:?}", t.peaks());
        let track = project.channels[4].mixer_track;
        project.mixer.iter_mut().find(|m| m.index == track).unwrap().volume = Some(0.0);
        t.update(&project);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(t.peaks(), (0.0, 0.0), "fader at zero is silent within 100 ms");
    }

    /// Live notes: on the live stream while stopped, mixed into playback while playing; note off
    /// releases them (needs an audio device).
    #[test]
    #[ignore]
    fn live_notes_play_and_release() {
        let mut project = crate::app::test_project();
        project.patterns = vec![os_model::Pattern { number: 1, ..Default::default() }];
        let t = Transport::new(None);
        t.update(&project);
        t.note_on(4, 60, 100);
        std::thread::sleep(Duration::from_millis(300));
        t.note_off(4, 60);
        assert_eq!(t.take_error(), None, "live stream opened");
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(300));
        t.note_on(4, 67, 100);
        std::thread::sleep(Duration::from_millis(100));
        assert!(t.sounding().contains(&(4, 67)), "held while playing: {:?}", t.sounding());
        assert!(t.peaks().0 > 0.01, "heard while playing");
        t.note_off(4, 67);
        std::thread::sleep(Duration::from_millis(100));
        assert!(t.sounding().is_empty() && t.peaks() == (0.0, 0.0), "released: {:?}", t.peaks());
    }

    /// Pause holds the position silently; resume continues from it; Stop rewinds (needs an audio device).
    #[test]
    #[ignore]
    fn pause_holds_position() {
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(400));
        t.pause();
        std::thread::sleep(Duration::from_millis(100));
        let at = t.seconds();
        let tick = t.tick();
        std::thread::sleep(Duration::from_millis(400));
        assert!(t.paused() && (t.seconds() - at).abs() < 1e-9 && (t.tick() - tick).abs() < 1e-9, "position held");
        assert_eq!(t.peaks(), (0.0, 0.0), "silent");
        t.resume();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!t.paused() && t.playing() && t.seconds() > at + 0.1, "resumed from {at}: {}", t.seconds());
        t.pause();
        t.stop();
        std::thread::sleep(Duration::from_millis(150));
        assert!(!t.playing() && t.seconds() == 0.0 && t.tick() == 0.0 && t.peaks() == (0.0, 0.0), "stopped = rewound");
    }

    /// Play from a position: what is heard starts there (nothing from the start is queued before the seek). New audio
    /// settings move the playback to a new stream at the position heard; paused stays paused (needs an audio device).
    #[test]
    #[ignore]
    fn playback_starts_at_its_position_and_moves_to_new_settings() {
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        let bar = u32::from(project.ppq) * 4;
        let notes = (0..16).map(|b| os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, b * bar) }).collect();
        project.patterns = vec![os_model::Pattern { number: 1, notes, ..Default::default() }];
        let start = f64::from(4 * bar);
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), start);
        let t0 = Instant::now();
        let mut seen = Vec::new();
        while t0.elapsed() < Duration::from_millis(300) {
            seen.push(t.tick());
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(seen.iter().all(|&k| k >= start), "heard before the start: {:?}", seen.iter().find(|&&k| k < start));
        assert!(seen.last().is_some_and(|&k| k > start + 10.0), "playing on");
        // A longer buffer: playback goes on from where it was, on a stream with that buffer.
        crate::audio_cfg::AudioCfg { buffer: Some(2048), ..Default::default() }.apply();
        let before = t.tick();
        t.reopen();
        std::thread::sleep(Duration::from_millis(300));
        assert!(t.playing() && !t.paused());
        assert!(t.tick() > before, "went on from {before}: {}", t.tick());
        assert_eq!(t.opened().map(|o| o.2), Some(Some(2048)));
        // Paused: the new stream holds the same position, silent.
        t.pause();
        std::thread::sleep(Duration::from_millis(100));
        let held = t.tick();
        crate::audio_cfg::AudioCfg::default().apply();
        t.reopen();
        std::thread::sleep(Duration::from_millis(300));
        assert!(t.paused() && (t.tick() - held).abs() < 1.0, "held at {held}: {}", t.tick());
        assert_eq!(t.peaks(), (0.0, 0.0), "silent");
        assert_eq!(t.opened().map(|o| o.2), Some(None));
        t.resume();
        std::thread::sleep(Duration::from_millis(200));
        assert!(t.tick() > held, "resumed");
        t.stop();
    }

    // spec: ui/settings-choices.md "Input / output: Auto close; CPU and Mixer: Smart disable, Align tick lengths,
    // Resampling quality (2026-10-16)"
    /// Auto close: closing the device holds a paused playback at its place with no stream open, and live notes open
    /// none; opening it again goes on from there on a new stream (needs an audio device; the pattern is silent).
    #[test]
    #[ignore]
    fn closing_the_device_holds_the_playback() {
        let mut project = crate::app::test_project();
        project.patterns = vec![os_model::Pattern { number: 1, ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!((t.take_error(), t.streams()), (None, 1), "playing");
        t.pause();
        t.close_device();
        std::thread::sleep(Duration::from_millis(200));
        let held = t.tick();
        assert!(t.paused() && held > 0.0, "paused at {held}");
        assert_eq!(t.streams(), 0, "no stream open");
        t.note_on(0, 60, 100);
        t.preview(project.clone());
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(t.streams(), 0, "a live note or a preview opens none");
        t.note_off(0, 60);
        assert!((t.tick() - held).abs() < 1e-9, "held");
        t.open_device();
        t.resume();
        std::thread::sleep(Duration::from_millis(300));
        assert!(t.playing() && !t.paused() && t.tick() > held, "went on from {held}: {}", t.tick());
        assert_eq!(t.streams(), 1);
        // Closed while stopped, a Play opens the output again.
        t.stop();
        t.close_device();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(t.streams(), 0);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(t.streams(), 1);
        t.stop();
    }

    /// The monitor tap carries what is played and goes quiet (count stops) when stopped (needs an audio device).
    #[test]
    #[ignore]
    fn tap_follows_playback() {
        let mut project = crate::app::test_project();
        let ch = project.channels[4].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 96, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(300));
        let mut b = vec![0.0f32; 2048];
        let n1 = t.tap(&mut b);
        assert!(n1 > 0 && b.iter().any(|v| v.abs() > 0.01), "tap has audio");
        assert!(t.sample_rate() > 0);
        t.stop();
        std::thread::sleep(Duration::from_millis(150));
        let n2 = t.tap(&mut b);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(t.tap(&mut b), n2, "no more samples after stop");
    }

    /// A hosted instrument that rings until reset: Stop must silence it, and nothing may resume on the next Play.
    #[test]
    #[ignore]
    fn stop_silences_hosted_instruments() {
        use os_engine::Instrument;
        struct Ringer {
            level: f32,
            resets: Arc<AtomicU32>,
        }
        impl Instrument for Ringer {
            fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {
                self.level = 0.3;
            }
            fn note_off(&mut self, _o: u32, _k: u8) {} // a long release: keeps ringing after the note
            fn process(&mut self, _r: u32, l: &mut [f32], r: &mut [f32]) {
                l.fill(self.level);
                r.fill(self.level);
            }
            fn active(&self) -> bool {
                self.level > 0.0
            }
            fn reset(&mut self) {
                self.level = 0.0;
                self.resets.fetch_add(1, Ordering::Relaxed);
            }
        }
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.channels[0].kind = os_model::ChannelKind::Plugin;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let resets = Arc::new(AtomicU32::new(0));
        let inst: SharedInstrument = Arc::new(Mutex::new(Ringer { level: 0.0, resets: resets.clone() }));
        let t = Transport::new(None);
        t.set_instruments(vec![(0, inst.clone())]);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(400));
        assert!(t.peaks().0 > 0.05, "the instrument is heard: {:?}", t.peaks());
        t.stop();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(t.peaks(), (0.0, 0.0), "silent after stop");
        assert!(resets.load(Ordering::Relaxed) >= 1, "instrument was reset");
        assert!(!inst.lock().unwrap().active(), "no tail left to resume");
    }

    /// The CPU panel's voices while a pattern plays and after Stop (spec: audio/voices.md §4-§5): the engine's own
    /// voices (a sampler channel without a hosted instrument) and a hosted Lemon Trio's are counted; Stop leaves none.
    #[test]
    #[ignore]
    fn stop_ends_every_voice() {
        let mut project = crate::app::test_project();
        let (synth, own) = (project.channels[0].index, project.channels[1].index);
        project.channels[0].kind = os_model::ChannelKind::Plugin;
        let note = |ch: u16, key: u16| os_model::Note { length: 384, key, ..os_model::Note::step(ch, 0) };
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![note(synth, 48), note(synth, 55), note(own, 60)], ..Default::default() }];
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        let osc = pl.index_of(crate::osc3::LEMON_TRIO).unwrap();
        pl.create_with(0, osc, None).unwrap();
        let t = Transport::new(None);
        t.set_instruments(vec![(0, pl.hosted[&0].plugin.clone() as SharedInstrument)]);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(pl.voices(), 2, "the synth's chord");
        assert!(t.engine_voices() >= 1, "the engine's own voice");
        t.stop();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!((t.engine_voices(), pl.voices()), (0, 0), "Stop ends every voice");
    }

    /// A hosted plugin (instrument or effect) that counts its resets.
    struct Counted(Arc<AtomicU32>);

    impl os_engine::HostedEffect for Counted {
        fn process_fx(&mut self, _rate: u32, _l: &mut [f32], _r: &mut [f32]) {}
        fn reset_fx(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl os_engine::Instrument for Counted {
        fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {}
        fn note_off(&mut self, _o: u32, _k: u8) {}
        fn process(&mut self, _r: u32, _l: &mut [f32], _rr: &mut [f32]) {}
        fn reset(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A start of playback (Play, a jump while playing) resets the hosted instruments, and the hosted effects only
    /// with "Reset plugins on transport" (spec: ui/settings-choices.md "Transport: Play truncated notes and Reset
    /// plugins"); Stop resets both.
    #[test]
    fn a_start_resets_hosted_effects_only_with_the_setting() {
        let (synth, fx) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let hosts: Instruments = Arc::new(Mutex::new(Hosts {
            inst: vec![(0, Arc::new(Mutex::new(Counted(synth.clone()))) as SharedInstrument)],
            fx: vec![(1, 0, Arc::new(Mutex::new(Counted(fx.clone()))) as SharedEffect)],
        }));
        let counts = || (synth.load(Ordering::Relaxed), fx.load(Ordering::Relaxed));
        reset_hosts(&hosts, false);
        assert_eq!(counts(), (1, 0), "off: the effect rings on");
        reset_hosts(&hosts, true);
        assert_eq!(counts(), (2, 1), "on: every hosted plugin");
        reset_instruments(&hosts);
        assert_eq!(counts(), (3, 2), "Stop");
    }

    /// The same through the audio thread (needs an audio device: `cargo test -p os-app -- --ignored a_jump_resets`):
    /// Play and a jump while playing (a Stop and a Play, as the playlist ruler and the song position slider make it,
    /// or a Play while playing, as the piano roll's marker does) reset a hosted effect with "Reset plugins on
    /// transport" on, not with it off; pause and resume never do; Stop does either way.
    #[test]
    #[ignore]
    fn a_jump_resets_hosted_effects_only_with_the_setting() {
        use crate::prefs::{set_check, RESET_PLUGINS_ON_TRANSPORT};
        let (_, project) = looping_engine();
        for on in [false, true] {
            set_check(&RESET_PLUGINS_ON_TRANSPORT, on);
            let fx = Arc::new(AtomicU32::new(0));
            let t = Transport::new(None);
            t.set_effects(vec![(0, 0, Arc::new(Mutex::new(Counted(fx.clone()))) as SharedEffect)]);
            let resets = |n: u32| if on { n } else { 0 };
            t.play(&project, PlayMode::Pattern(1), 0.0);
            std::thread::sleep(Duration::from_millis(300));
            assert!(t.playing() && t.take_error().is_none(), "playing");
            assert_eq!(fx.load(Ordering::Relaxed), resets(1), "Play, setting {on}");
            t.stop();
            t.play(&project, PlayMode::Pattern(1), 192.0);
            std::thread::sleep(Duration::from_millis(300));
            assert!(t.playing() && t.tick() >= 192.0, "jumped: {}", t.tick());
            assert_eq!(fx.load(Ordering::Relaxed), resets(2), "a jump, setting {on}");
            t.play(&project, PlayMode::Pattern(1), 96.0);
            std::thread::sleep(Duration::from_millis(300));
            assert_eq!(fx.load(Ordering::Relaxed), resets(3), "a Play while playing, setting {on}");
            t.pause();
            std::thread::sleep(Duration::from_millis(100));
            t.resume();
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(fx.load(Ordering::Relaxed), resets(3), "pause and resume, setting {on}");
            t.stop();
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(fx.load(Ordering::Relaxed), resets(3) + 1, "Stop, setting {on}");
        }
    }
}
