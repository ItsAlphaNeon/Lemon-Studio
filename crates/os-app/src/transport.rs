// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Playback: an audio thread that owns the output stream and the engines.
//!
//! One state machine, `State` (stopped / playing / paused), published in `Shared`:
//! - **Play** starts the pattern or the playlist from a start tick (a pattern loops inside the engine, the
//!   playlist plays out and stops by itself). A jump of the position while playing is a new Play there (a new
//!   engine). Before the first block, the hosted instruments are reset, the hosted effects too with "Reset plugins
//!   on transport"; with "Play truncated notes on transport" the notes under way at the start tick sound from there
//!   (`Starts`; spec: ui/settings-choices.md "Transport: Play truncated notes and Reset plugins").
//! - **Pause** holds the engine where it is (`Engine::pause`: the sampler channels fade out over a few milliseconds,
//!   the hosted instruments are let go of their notes, and they and the effects ring on); **Resume** fades back in
//!   from that exact place.
//! - **Stop** holds the engine as Pause does and rewinds to the start; the hosted plugins' tails ring out
//!   (`Ringing`), and every hosted plugin is reset once they are quiet (`Instrument::reset`: nothing frozen comes back
//!   later). It also ends live notes and previews.
//!
//! The UI sets the state at once (so a double click behaves), then sends the matching command; commands are
//! applied in order on the audio thread. `generation` counts play / stop requests: a song that finishes after a
//! newer request must not stop it.
//!
//! Everything plays on one stream (`Stream`), which lasts as long as the output device is open: its feeder thread
//! renders whole ticks of whichever engine it has (the playback, the live engine, a stopped playback ringing out)
//! into a lock-free queue, and the device callback only copies from it (spec: audio/implementing.md "Playing live"),
//! so a busy engine (an edit being swapped in, a heavy tick) never costs a silent block. The queue keeps one device
//! block and a margin queued (one block more, but 3 to 6 ms: `margin`); a block it could not fill counts as an
//! underrun (`Transport::underruns`). Play, Stop and live notes hand the feeder another engine (`Stream::swap`): what
//! the one before left queued fades out, and no stream, thread or device is opened for them. The hosted plugins are
//! only ever processed by that feeder, one engine at a time; the resets an engine asks for are made before its first
//! tick.
//!
//! The stream plays through the output device (`device::OutputDevice`, which also mixes the previews' stream: an
//! ASIO driver serves one stream per program). It opens with the audio settings (`audio_cfg`: device, rate, buffer
//! length) when a sound first needs it and stays open, silent when nothing plays (spec: audio/devices.md §2.3), until
//! Auto close; `reopen` moves the running playback onto the device opened with new settings at the position heard.
//!
//! `update` swaps in an edited snapshot at the current position (decoded samples stay cached in the library).
//! `preview` plays one note once on the previews' stream, whose engines host no plugins: previews of plugin channels
//! play as timed live notes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::device::{DeviceInfo, OutputDevice, Source};
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
/// Below this peak (-60 dBFS) a block is silence: a dropout there is not heard (`Transport::heard_dropouts`).
const SILENT: f32 = 0.001;
/// A device underrun this soon after a block that was not silent is heard.
const HEARD_WITHIN: Duration = Duration::from_millis(250);

/// Sound reached the device within `HEARD_WITHIN` (`Shared::sound_at`).
fn sound_lately(shared: &Shared) -> bool {
    let at = shared.sound_at.load(Ordering::Relaxed);
    at != 0 && now_ms().saturating_sub(at) <= HEARD_WITHIN.as_millis() as u64
}

/// Milliseconds since the first call, for `Shared::sound_at`.
fn now_ms() -> u64 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}
/// How long a stopped stream stays alive to finish its fade before it is dropped.
const CLOSE_AFTER: Duration = Duration::from_millis(40);
/// A stopped playback rings out its effects' tails (spec: audio/transport.md "Stop") until its output has stayed below
/// -96 dB (the engine's quiet, as an export's tail) for `RING_QUIET`, at most `RING_MAX` (an export's longest tail).
const RING_QUIET: Duration = Duration::from_secs(3);
const RING_MAX: Duration = Duration::from_secs(30);
const RING_QUIET_LEVEL: f32 = 1.584_893_1e-5;
/// What a stream keeps queued beyond the block its device is about to take (spec: audio/devices.md "Render-ahead"):
/// one block more, since some drivers ask for two blocks back to back, but at least `MARGIN_MIN` seconds (the feeder
/// has to wake and render a tick between two blocks) and at most `MARGIN_MAX`. Together with the block and part of a
/// tick it is what a live edit or note waits for.
const MARGIN_MIN: f64 = 0.003;
const MARGIN_MAX: f64 = 0.006;
/// A stream's queue: samples (interleaved stereo) and ticks it can hold, and the room the feeder leaves for one more
/// tick (a tick at 10 BPM and 24 PPQ at 192 kHz is 96 000 samples).
const QUEUE_SAMPLES: usize = 1 << 19;
const QUEUE_TICKS: usize = 1 << 12;
const TICK_ROOM: usize = 1 << 17;

/// State the UI reads every frame.
#[derive(Default)]
pub struct Shared {
    state: AtomicU8,
    /// Counts play and stop requests (see the module docs).
    generation: AtomicU64,
    /// The running playback is the playlist (else one pattern).
    song: AtomicBool,
    /// The song's loop points, start << 32 | end in ticks (0 = the whole song): the playlist's time selection.
    loop_range: AtomicU64,
    /// A quantized song jump for the playing song: unit << 32 | target in ticks (0 = none; `jump_on`).
    jump: AtomicU64,
    /// Paused by a Marker pause: its tick + 1 (0 = not); after Resume, the same for the app to take as the playback's
    /// start (spec: audio/transport.md "Time markers in SONG mode").
    marker_hold: AtomicU64,
    marker_resumed: AtomicU64,
    /// Playback position in frames of the output stream.
    position: AtomicU64,
    /// Playback position in ticks (f64 bits): what every playhead and the time display show. A pattern wraps.
    tick: AtomicU64,
    /// The tempo the playback plays at (f64 bits; 0 = none yet): tempo automation moves it.
    tempo: AtomicU64,
    sample_rate: AtomicU32,
    /// An export is running, its progress in 1/1000 of the song, and its outcome once done.
    exporting: AtomicBool,
    export_permille: AtomicU32,
    export_result: Mutex<Option<Result<(f64, Vec<PathBuf>), String>>>,
    /// Peak of the last block, f32 bits, per side.
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    error: Mutex<Option<String>>,
    /// Notes playing now, as (channel index, key); written by the audio callback.
    sounding: Mutex<Vec<(u16, u16)>>,
    /// The last samples that left the stream that plays, for the toolbar monitor and meter.
    tap: Mutex<Tap>,
    /// The playback's last block: how the position moves through it while the device plays it (`Transport::tick`).
    pace: Mutex<Option<Pace>>,
    /// Peaks per mixer track (L, R) since the mixer meters last read them (`TrackPeaks`).
    tracks: Mutex<TrackPeaks>,
    /// Device blocks the queue could not fill since the program started.
    underruns: AtomicU64,
    /// Those of them that cut into sound: the block before the gap, or the one rendered with it, was not silent
    /// (`SILENT`). The rest happened over silence and were not heard.
    heard: AtomicU64,
    /// Those of them where a hosted plugin missed its tick.
    plugin_late: AtomicU64,
    /// Those of them where the queue was empty (the feeder was late), and the device's own.
    feeder_late: AtomicU64,
    device_late: AtomicU64,
    /// Nanoseconds the feeder of the stream that plays spent rendering since the program started.
    render_ns: AtomicU64,
    /// When a stream last handed the device a block that was not silent (`now_ms`; 0: never).
    sound_at: AtomicU64,
    /// How far the stream that plays renders ahead of the device.
    ahead: AheadStats,
    /// Time spent rendering per second of audio (f32 bits), over the last quarter second of playback.
    load: AtomicU32,
    /// Frames the device asked for in its last block.
    block: AtomicU32,
    /// The output device the stream that plays opened.
    opened: Mutex<Option<DeviceInfo>>,
    /// The last playback's use of the workers: the next one, after a jump, starts from it (`Engine::set_load_gate`).
    gate: Mutex<Option<os_engine::LoadGate>>,
    /// An instrument's editor is open (`Transport::listen`).
    listening: AtomicBool,
    /// Each running stream's engine voices (its feeder writes them after every fill; a stream that closed drops its
    /// counter). Spec: audio/voices.md §5.
    engine_voices: Mutex<Vec<Weak<AtomicU32>>>,
    /// Disk recording into mixer tracks: where the playback's armed tracks hand their takes (`Transport::record_tracks`).
    record: Mutex<Option<Arc<dyn os_engine::RecordTap>>>,
}

/// The frames queued when the device asked for a block, since they were last taken (`AheadStats::take`): what the
/// engine rendered ahead of the device, so what an edit or a note played by hand waits for before the device has it.
pub struct Ahead {
    pub min: u64,
    pub max: u64,
    pub mean: f64,
    pub blocks: u64,
    /// The longest time between two blocks, µs: a driver that stalled (and then catches up) shows here.
    pub gap: u64,
}

/// `Ahead`, gathered by the device's callback.
struct AheadStats {
    min: AtomicU64,
    max: AtomicU64,
    sum: AtomicU64,
    blocks: AtomicU64,
    gap: AtomicU64,
}

impl Default for AheadStats {
    fn default() -> Self {
        AheadStats { min: AtomicU64::new(u64::MAX), max: AtomicU64::new(0), sum: AtomicU64::new(0), blocks: AtomicU64::new(0), gap: AtomicU64::new(0) }
    }
}

impl AheadStats {
    /// One block: the frames queued when the device asked, and the time since the block before (`None`: the first).
    fn record(&self, frames: u64, gap: Option<Duration>) {
        self.min.fetch_min(frames, Ordering::Relaxed);
        self.max.fetch_max(frames, Ordering::Relaxed);
        self.sum.fetch_add(frames, Ordering::Relaxed);
        self.blocks.fetch_add(1, Ordering::Relaxed);
        if let Some(g) = gap {
            self.gap.fetch_max(g.as_micros() as u64, Ordering::Relaxed);
        }
    }

    /// What was gathered since the last call (`None`: no block), and starts again.
    fn take(&self) -> Option<Ahead> {
        let blocks = self.blocks.swap(0, Ordering::Relaxed);
        let sum = self.sum.swap(0, Ordering::Relaxed);
        let (min, max) = (self.min.swap(u64::MAX, Ordering::Relaxed), self.max.swap(0, Ordering::Relaxed));
        let gap = self.gap.swap(0, Ordering::Relaxed);
        (blocks > 0).then(|| Ahead { min, max, mean: sum as f64 / blocks as f64, blocks, gap })
    }
}

/// Ring of the newest output frames (L, R), and when the device took the last block of them.
///
/// A device takes its blocks at once and plays them over their length: a sound server's are often a tenth of a second
/// or more. What is heard is not the end of the newest block but `heard` frames in, the time since it was taken at
/// the rate (spec: Linux shared mode, issue #160): the monitor shows and the meter reads up to there, so they move
/// as the sound does rather than in steps of a block.
struct Tap {
    l: Vec<f32>,
    r: Vec<f32>,
    /// Frames written since the start.
    total: u64,
    /// The last block: the device block it was for (`device::blocks`), its first frame, its length, when the device
    /// took it and the rate it plays at.
    block: u64,
    start: u64,
    frames: u64,
    at: Option<Instant>,
    rate: u32,
    /// The frame the meter has read to (`Transport::take_peaks`).
    metered: u64,
}

/// Frames the tap holds: the largest analyzer FFT behind the newest block (two of the longest buffer length).
const TAP_FRAMES: usize = crate::monitor::MAX_FFT + 2 * 8192;

impl Default for Tap {
    fn default() -> Self {
        Tap { l: vec![0.0; TAP_FRAMES], r: vec![0.0; TAP_FRAMES], total: 0, block: 0, start: 0, frames: 0, at: None, rate: 0, metered: 0 }
    }
}

impl Tap {
    /// One stream's block (interleaved, `w` channels: its first pair is taken) for device block `block`, taken `at`.
    /// The streams of one device block (playback and live notes) are summed, as the device hears them; a new device
    /// block comes after the last.
    fn push(&mut self, stereo: &[f32], w: usize, block: u64, at: Instant, rate: u32) {
        let n = (stereo.len() / w) as u64;
        let same = block != 0 && block == self.block && n == self.frames;
        if !same {
            (self.block, self.start, self.frames, self.at, self.rate) = (block, self.total, n, Some(at), rate);
            self.total += n;
        }
        for (i, f) in stereo.chunks_exact(w).enumerate() {
            let k = (self.start as usize + i) % TAP_FRAMES;
            if same {
                self.l[k] += f[0];
                self.r[k] += f[1];
            } else {
                (self.l[k], self.r[k]) = (f[0], f[1]);
            }
        }
    }

    /// The frames heard by `now`: into the last block as far as the time since the device took it (all of it once
    /// that is past).
    fn heard(&self, now: Instant) -> u64 {
        let Some(at) = self.at.filter(|_| self.rate > 0) else { return self.total };
        let into = (now.saturating_duration_since(at).as_secs_f64() * f64::from(self.rate)) as u64;
        self.start + into.min(self.frames)
    }
}

/// The mixer meters' feed. The playback renders ahead of the device a block at a time: its peaks are kept per tick
/// with the frame of the stream's output the tick starts at (counted from the stream's start: a looping pattern's
/// engine frames start again) and read once the device has played to there (`Pace::played`), so the meters move as the
/// sound is heard rather than a block at a time (issue #160). Other streams' peaks (live notes) are read at once.
#[derive(Default)]
struct TrackPeaks {
    now: Vec<[f32; 2]>,
    ticks: std::collections::VecDeque<(u64, Vec<[f32; 2]>)>,
    /// Emptied vectors to use again, so rendering does not allocate once it runs.
    spare: Vec<Vec<[f32; 2]>>,
}

/// Ticks of peaks `TrackPeaks` keeps waiting (some seconds of the fastest ticks); older ones are read at once.
const PEAK_TICKS: usize = 4096;

impl TrackPeaks {
    /// The peaks the playback's tick at output frame `frame` left in `e`.
    fn tick(&mut self, frame: u64, e: &mut Engine) {
        let mut v = self.spare.pop().unwrap_or_default();
        v.clear();
        e.take_track_peaks(&mut v);
        self.ticks.push_back((frame, v));
        while self.ticks.len() > PEAK_TICKS {
            self.release();
        }
    }

    /// Moves the oldest waiting tick's peaks to `now`.
    fn release(&mut self) {
        let Some((_, v)) = self.ticks.pop_front() else { return };
        merge(&mut self.now, &v);
        self.spare.push(v);
    }

    /// The peaks heard up to output frame `heard` (`None`: all of them, nothing is playing) since the last call.
    fn take(&mut self, heard: Option<u64>) -> Vec<[f32; 2]> {
        while self.ticks.front().is_some_and(|(f, _)| heard.is_none_or(|h| *f <= h)) {
            self.release();
        }
        let out = self.now.clone();
        self.now.iter_mut().for_each(|p| *p = [0.0; 2]);
        out
    }
}

/// `into` takes the larger of its peaks and those of `from`, growing to as many tracks.
fn merge(into: &mut Vec<[f32; 2]>, from: &[[f32; 2]]) {
    if into.len() < from.len() {
        into.resize(from.len(), [0.0; 2]);
    }
    for (o, p) in into.iter_mut().zip(from) {
        (o[0], o[1]) = (o[0].max(p[0]), o[1].max(p[1]));
    }
}

/// The playback's last block (the device's): its position at the start and at the end (engine frame, tick), the
/// stream's output frames played before and after it, its length, when the device took it and the rate it plays at.
#[derive(Clone, Copy, Debug)]
struct Pace {
    from: (u64, f64),
    to: (u64, f64),
    read: (u64, u64),
    frames: u64,
    at: Instant,
    rate: u32,
}

impl Pace {
    /// How far through the block the device is by `now`, 0..1.
    fn share(&self, now: Instant) -> f64 {
        let length = self.frames as f64 / f64::from(self.rate);
        if self.rate == 0 || length <= 0.0 { 1.0 } else { (now.saturating_duration_since(self.at).as_secs_f64() / length).min(1.0) }
    }

    /// The position heard by `now`, moving from the block's start to its end. A pattern that wrapped in the block is
    /// at the end at once (the wrap lies somewhere inside).
    fn at(&self, now: Instant) -> (u64, f64) {
        let s = self.share(now);
        let frame = self.from.0 + ((self.to.0.saturating_sub(self.from.0)) as f64 * s) as u64;
        let tick = if self.to.1 >= self.from.1 { self.from.1 + (self.to.1 - self.from.1) * s } else { self.to.1 };
        (frame, tick)
    }

    /// The stream's output frames played by `now`.
    fn played(&self, now: Instant) -> u64 {
        self.read.0 + (self.read.1.saturating_sub(self.read.0) as f64 * self.share(now)) as u64
    }
}

impl Shared {
    /// Back to the start: position, meters and lit keys.
    fn rewind(&self, tick: f64) {
        self.position.store(0, Ordering::Relaxed);
        self.tick.store(tick.to_bits(), Ordering::Relaxed);
        self.tempo.store(0, Ordering::Relaxed);
        self.peak_l.store(0, Ordering::Relaxed);
        self.peak_r.store(0, Ordering::Relaxed);
        if let Ok(mut p) = self.pace.lock() {
            *p = None;
        }
        if let Ok(mut v) = self.sounding.lock() {
            v.clear();
        }
    }
}

/// What another thread can see of the playback (`Transport::watch`).
#[derive(Clone)]
pub struct Watch(Arc<Shared>);

impl Watch {
    pub fn state(&self) -> State {
        match self.0.state.load(Ordering::Acquire) {
            PLAYING => State::Playing,
            PAUSED => State::Paused,
            _ => State::Stopped,
        }
    }

    /// Counts play and stop requests: a new value while playing is a new start (a jump of the position).
    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Acquire)
    }

    /// The position in ticks (`Transport::tick`).
    pub fn tick(&self) -> f64 {
        f64::from_bits(self.0.tick.load(Ordering::Relaxed))
    }

    /// The tempo playback plays at, `None` before its first tick (`Transport::tempo`).
    pub fn tempo(&self) -> Option<f64> {
        let t = f64::from_bits(self.0.tempo.load(Ordering::Relaxed));
        (self.state() != State::Stopped && t > 0.0).then_some(t)
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
    /// Levels changed, and nothing else (`Transport::update`).
    LiveEdits(Vec<os_engine::LiveEdit>),
    Preview(Box<Project>),
    /// Play a project's playlist once on the preview stream (clip preview).
    PreviewSong(Box<Project>),
    /// Like `Preview`, faded out after the given time if still sounding (browser sample preview).
    PreviewFor(Box<Project>, Option<Duration>),
    /// The hosted instruments changed (in `Transport::instruments`): rebuild the engines.
    Instruments,
    /// Live note (typing keyboard): channel (rack position), key, velocity; `NoteOff` releases.
    /// A live note: channel, key, velocity, MIDI channel (`Engine::note_on_midi`).
    NoteOn(usize, u16, u8, u8),
    NoteOff(usize, u16),
    /// A controller message for a channel (rack position): message, pitch bend range in cents (`Engine::midi`).
    Midi(usize, [u8; 3], f64),
    /// A tap on the Tempo tapper: a click, accented or not, at the metronome's volume (dB), like a live note.
    TapClick(bool, i8),
    /// An instrument's editor opened or the last one closed (`Transport::listen`).
    Listen(bool),
    /// Stop; true: the hosted effects' tails ring out (`Transport::stop`), false: they are silenced too.
    Stop(bool),
    /// Render with the hosted plugins and write the files (FILE > Export): project, mode, sample rate, the export.
    Export(Box<Project>, PlayMode, u32, Box<crate::export::Job>),
    /// The audio settings changed: move the playback to a new stream, close the live and preview streams.
    Reopen,
    /// Auto close (`Transport::close_device`): every stream closes; the playback keeps its place for `Open`.
    Close,
    /// The output may be used again (`Transport::open_device`).
    Open,
    /// Opens the ASIO driver's own settings window (`Transport::asio_panel`).
    AsioPanel,
    /// The FL Studio factory content folder changed (`Transport::set_factory`).
    Factory(Option<PathBuf>),
    /// Disk recording started or ended (`Transport::record_tracks`): the playback's engine takes it at once.
    Record,
    /// Tests: the open device fails as if it went away.
    #[cfg(test)]
    FailDevice,
}

/// Where an export is (`Transport::export_status`).
#[derive(Debug, Clone, PartialEq)]
pub enum Export {
    Idle,
    /// Rendering: 0..1 of the song.
    Running(f32),
    /// Finished: the length written in seconds and the files, or what went wrong.
    Done(Result<(f64, Vec<PathBuf>), String>),
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
    /// Bars of a recording count-in before the position moves (spec: ui/recording.md §3); 0: none.
    pub(crate) count_in: u32,
}

impl Starts {
    pub(crate) fn from_prefs() -> Starts {
        use crate::prefs::{check, PLAY_TRUNCATED_ON_TRANSPORT, RESET_PLUGINS_ON_TRANSPORT};
        Starts { truncated: check(&PLAY_TRUNCATED_ON_TRANSPORT), reset: check(&RESET_PLUGINS_ON_TRANSPORT), count_in: 0 }
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

/// Set by a render without a window (`crate::render`): the audio thread never opens an output device by itself.
static HEADLESS: AtomicBool = AtomicBool::new(false);

/// No output device opens unless playback asks for one: for renders without a window (`crate::render`). Call before
/// the first `Transport::new`.
pub fn headless() {
    HEADLESS.store(true, Ordering::Release);
}

pub struct Transport {
    tx: Sender<Cmd>,
    latest: Arc<Mutex<Option<Project>>>,
    /// The project `update` last sent, with its edits; `None` once a playback starts from a project of its own.
    sent: Mutex<Option<Project>>,
    instruments: Instruments,
    shared: Arc<Shared>,
    /// The audio thread: joined when the transport goes (see `Drop`).
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Transport {
    pub fn new(factory: Option<PathBuf>) -> Transport {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let instruments: Instruments = Arc::new(Mutex::new(Hosts::default()));
        let shared = Arc::new(Shared { sounding: Mutex::new(Vec::with_capacity(256)), ..Shared::default() });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("openstudio-audio".into())
            .spawn({
                let latest = latest.clone();
                let inst = instruments.clone();
                move || {
                    crate::device::init_thread();
                    audio_thread(rx, s, latest, inst, host_library(factory))
                }
            })
            .expect("spawn audio thread");
        Transport { tx, latest, sent: Mutex::new(None), instruments, shared, thread: Some(thread) }
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

    /// Peak per mixer track (L, R) heard since the last call (the mixer meters' feed); resets them.
    pub fn take_track_peaks(&self) -> Vec<[f32; 2]> {
        let heard = self.paced().map(|p| p.played(Instant::now()));
        self.shared.tracks.lock().map(|mut t| t.take(heard)).unwrap_or_default()
    }

    /// The sample rate streams open at (the audio settings' device and rate; what hosted plugins are activated at):
    /// the open device's, which the audio thread publishes (`device::rate`).
    pub fn device_rate() -> Option<u32> {
        crate::device::rate()
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

    /// Opens the ASIO driver's own settings window (Settings ▸ Audio ▸ Show ASIO panel), opening the driver first if
    /// it is closed: on the audio thread, which drives the driver. What went wrong comes back as an error.
    pub fn asio_panel(&self) {
        let _ = self.tx.send(Cmd::AsioPanel);
    }

    /// The FL Studio install folder that sample paths' placeholders point into is now `dir` (`crate::factory_folder`):
    /// the playing engines take their factory samples from there.
    pub fn set_factory(&self, dir: Option<PathBuf>) {
        let _ = self.tx.send(Cmd::Factory(dir));
    }

    /// Renders `project` offline at `rate`, with the hosted instruments and effects, and writes the files of `job`, on
    /// the audio thread: playback, previews and live notes stop at once and nothing else processes the plugins
    /// meanwhile. Follow it with `export_status`.
    pub fn export(&self, project: &Project, mode: PlayMode, rate: u32, job: crate::export::Job) {
        self.forget_sent();
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.state.store(STOPPED, Ordering::Release);
        self.shared.export_permille.store(0, Ordering::Relaxed);
        self.shared.exporting.store(true, Ordering::Release);
        let _ = self.tx.send(Cmd::Export(Box::new(project.clone()), mode, rate, Box::new(job)));
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
        self.play_counted(project, mode, start_tick, 0);
    }

    /// `play`, after a recording count-in of `bars` bars (spec: ui/recording.md §3): the position holds at
    /// `start_tick` while the metronome counts in.
    pub fn play_counted(&self, project: &Project, mode: PlayMode, start_tick: f64, bars: u32) {
        self.forget_sent();
        if let Ok(mut slot) = self.latest.lock() {
            *slot = None;
        }
        let generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.marker_hold.store(0, Ordering::Relaxed);
        self.shared.jump.store(0, Ordering::Relaxed);
        self.shared.song.store(matches!(mode, PlayMode::Song), Ordering::Relaxed);
        self.shared.rewind(start_tick);
        self.shared.state.store(PLAYING, Ordering::Release);
        let starts = Starts { count_in: bars, ..Starts::from_prefs() };
        let _ = self.tx.send(Cmd::Play(Box::new(project.clone()), mode, start_tick, generation, starts));
    }

    /// The song's loop points (the playlist's time selection; `None` = the whole song). Song playback follows them
    /// from the next tick it renders, also while playing.
    pub fn set_loop(&self, range: Option<(u32, u32)>) {
        let packed = range.filter(|&(a, b)| b > a).map_or(0, |(a, b)| u64::from(a) << 32 | u64::from(b));
        self.shared.loop_range.store(packed, Ordering::Relaxed);
    }

    /// The playing song jumps to tick `to` when it reaches the next multiple of `unit` ticks (the playlist's Quantize
    /// song jumps); a later call replaces one still waiting.
    pub fn jump_on(&self, unit: u32, to: u32) {
        self.shared.jump.store(u64::from(unit.max(1)) << 32 | u64::from(to), Ordering::Relaxed);
    }

    /// Replace the playing project, continuing from the current position.
    /// Disk recording into mixer tracks (spec: audio/track-io.md §2.5): the playback's armed tracks hand their takes to
    /// `tap` from now on (and any playback started later), or no longer (`None`).
    pub fn record_tracks(&self, tap: Option<Arc<dyn os_engine::RecordTap>>) {
        if let Ok(mut slot) = self.shared.record.lock() {
            *slot = tap;
        }
        let _ = self.tx.send(Cmd::Record);
    }

    /// Roughly how much later than the song it went with a device input reaches the engine, in frames at the device's
    /// rate, for a tick of `tick` frames: the device's two latencies (an ASIO driver's; 0 for others), the time the
    /// engine reads behind the newest input (two blocks, the longest margin and a tick), and the render-ahead (a block
    /// and the margin) (spec: audio/track-io.md §2.2, §2.5).
    pub fn input_delay(&self, tick: u64) -> u64 {
        let info = self.opened();
        let rate = info.as_ref().map_or(48000, |d| d.rate);
        let block = u64::from(self.block().max(64));
        let margin = (f64::from(rate) * MARGIN_MAX) as u64;
        let device = info.filter(|d| d.host == crate::device::ASIO).map_or(0, |d| u64::from(d.latency.unwrap_or(0)) + u64::from(d.input_latency.unwrap_or(0)));
        device + 2 * block + margin + tick + block + margin
    }

    /// What plays next may not be what `update` last sent: its next project goes whole.
    fn forget_sent(&self) {
        if let Ok(mut sent) = self.sent.lock() {
            *sent = None;
        }
    }

    pub fn update(&self, project: &Project) {
        // Levels alone reach the playing engines in place: a new engine at every move of a knob takes milliseconds of
        // building each time (up to some 70 in a large project). What was last sent says what changed.
        if let Ok(mut sent) = self.sent.lock() {
            if let Some(edits) = sent.as_mut().and_then(|s| os_engine::live_edits(s, project)) {
                let _ = self.tx.send(Cmd::LiveEdits(edits));
                return;
            }
            *sent = Some(project.clone());
        }
        // Edits can come faster than engines are built: keep only the newest.
        if let Ok(mut slot) = self.latest.lock() {
            *slot = Some(project.clone());
        }
        let _ = self.tx.send(Cmd::Update);
    }

    /// Play pattern 1 of `project` once (a one-note preview project), on the previews' stream.
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

    /// Start a live note: into the playback while playing, else into the live engine (which stays, so only the first
    /// note pays for building it). The live engine uses the project from the last `play` / `update`.
    pub fn note_on(&self, channel: usize, key: u16, velocity: u8) {
        self.note_on_midi(channel, key, velocity, 0);
    }

    /// `note_on` on MIDI channel `midi` (0..15) for a hosted instrument (MIDI channel through, spec:
    /// ui/channel-rack.md §18).
    pub fn note_on_midi(&self, channel: usize, key: u16, velocity: u8, midi: u8) {
        let _ = self.tx.send(Cmd::NoteOn(channel, key, velocity, midi));
    }

    pub fn note_off(&self, channel: usize, key: u16) {
        let _ = self.tx.send(Cmd::NoteOff(channel, key));
    }

    /// A controller message (not a note) for the channel at rack position `channel`, where a live note would go: a
    /// hosted instrument gets it as it is, another channel bends by pitch bend over ±`bend_cents` (spec:
    /// ui/midi-settings.md §4).
    pub fn midi(&self, channel: usize, bytes: [u8; 3], bend_cents: f64) {
        let _ = self.tx.send(Cmd::Midi(channel, bytes, bend_cents));
    }

    /// A tap on the Tempo tapper (spec: ui/tempo-tapper.md §8): the metronome's Tick, accented or not, where a live
    /// note would play (into the playback while playing, else into the live engine), at the metronome's volume.
    pub fn tap_click(&self, accent: bool) {
        let _ = self.tx.send(Cmd::TapClick(accent, crate::metronome::volume()));
    }

    /// An instrument's editor is open (or the last one closed). Its own keyboard plays notes the host never sees, which
    /// only sound if the plugin is processed: while nothing plays (stopped or paused) the live engine plays and keeps
    /// processing the hosted instruments. It uses the project from the last `play` / `update`.
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
            // Resumed from a Marker pause: the playback now counts as started there.
            let held = self.shared.marker_hold.swap(0, Ordering::AcqRel);
            if held != 0 {
                self.shared.marker_resumed.store(held, Ordering::Release);
            }
            let _ = self.tx.send(Cmd::Resume);
        }
    }

    /// The Marker pause playback was resumed from since the last call, once (spec: audio/transport.md "Time markers in
    /// SONG mode"): Stop then returns there.
    pub fn take_marker_resumed(&self) -> Option<u32> {
        let t = self.shared.marker_resumed.swap(0, Ordering::AcqRel);
        (t != 0).then(|| (t - 1).min(u64::from(u32::MAX)) as u32)
    }

    /// Stop everything: playback rewinds, hosted instruments are let go of their notes, live notes and previews end; the
    /// hosted plugins' tails (releases, reverb, delay) ring out, as in FL (spec: audio/transport.md "Stop"). Also works
    /// when already stopped: it silences notes still ringing from the typing keyboard or a preview, and tails still
    /// ringing.
    pub fn stop(&self) {
        self.stop_with(true);
    }

    /// `stop`, the effects' tails silenced too (TOOLS > Stop sound; a project opened or made new).
    pub fn silence(&self) {
        self.stop_with(false);
    }

    fn stop_with(&self, ring: bool) {
        self.forget_sent();
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.marker_hold.store(0, Ordering::Relaxed);
        self.shared.jump.store(0, Ordering::Relaxed);
        self.shared.marker_resumed.store(0, Ordering::Relaxed);
        self.shared.state.store(STOPPED, Ordering::Release);
        self.shared.rewind(0.0);
        let _ = self.tx.send(Cmd::Stop(ring));
    }

    /// A view of the playback's state, position and tempo for another thread (the MIDI master clock, `crate::midi_out`).
    pub fn watch(&self) -> Watch {
        Watch(self.shared.clone())
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

    /// Playback position in ticks of the running mode (a looping pattern wraps at its end): while playing, as far
    /// into the device's last block as it has played by now (`Pace`).
    pub fn tick(&self) -> f64 {
        self.paced().map_or_else(|| f64::from_bits(self.shared.tick.load(Ordering::Relaxed)), |p| p.at(Instant::now()).1)
    }

    pub fn seconds(&self) -> f64 {
        let rate = self.shared.sample_rate.load(Ordering::Relaxed).max(1);
        let frame = self.paced().map_or_else(|| self.shared.position.load(Ordering::Relaxed), |p| p.at(Instant::now()).0);
        frame as f64 / f64::from(rate)
    }

    /// The playback's last block, while it plays.
    fn paced(&self) -> Option<Pace> {
        if self.shared.state.load(Ordering::Acquire) != PLAYING {
            return None;
        }
        *self.shared.pace.lock().ok()?
    }

    /// Fills `out` with the output heard last (mono, oldest first, at most `monitor::MAX_FFT`) and returns the count
    /// of frames heard; an unchanged count means nothing was played since the last call (`Tap::heard`).
    pub fn tap(&self, out: &mut [f32]) -> u64 {
        let Ok(t) = self.shared.tap.lock() else { return 0 };
        let heard = t.heard(Instant::now());
        let n = out.len().min(crate::monitor::MAX_FFT).min(heard as usize);
        let skip = out.len() - n;
        out[..skip].fill(0.0);
        let first = heard as usize - n;
        for (i, o) in out[skip..].iter_mut().enumerate() {
            let k = (first + i) % TAP_FRAMES;
            *o = (t.l[k] + t.r[k]) * 0.5;
        }
        heard
    }

    /// The output stream's sample rate (0 until the first stream opened).
    pub fn sample_rate(&self) -> u32 {
        self.shared.sample_rate.load(Ordering::Relaxed)
    }

    /// Output peaks (L, R) of what was heard since the last call (the toolbar meter's feed).
    pub fn take_peaks(&self) -> (f32, f32) {
        let Ok(mut t) = self.shared.tap.lock() else { return (0.0, 0.0) };
        let heard = t.heard(Instant::now());
        let from = t.metered.max(heard.saturating_sub(TAP_FRAMES as u64));
        t.metered = heard;
        (from..heard).fold((0.0f32, 0.0f32), |(l, r), f| {
            let k = f as usize % TAP_FRAMES;
            (l.max(t.l[k].abs()), r.max(t.r[k].abs()))
        })
    }

    /// Peaks of the last playback block (L, R).
    pub fn peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.shared.peak_l.load(Ordering::Relaxed)),
            f32::from_bits(self.shared.peak_r.load(Ordering::Relaxed)),
        )
    }

    /// The tempo the running playback plays at, tempo automation included; `None` while stopped or before its first
    /// tick.
    pub fn tempo(&self) -> Option<f64> {
        let t = f64::from_bits(self.shared.tempo.load(Ordering::Relaxed));
        (self.playing() && t > 0.0).then_some(t)
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

    /// Device blocks that could not be filled in time since the program started.
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// The dropouts heard since the program started: those that cut into sound (`Shared::heard`). Each one turns the
    /// CPU panel's count of `underruns` red for a moment.
    pub fn heard_dropouts(&self) -> u64 {
        self.shared.heard.load(Ordering::Relaxed)
    }

    /// A hosted plugin has missed its tick since the program started: the count has its mark.
    pub fn plugin_late(&self) -> bool {
        self.shared.plugin_late.load(Ordering::Relaxed) > 0
    }

    /// The dropouts by kind since the program started (the playback checks read them: `crate::checks`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn drops(&self) -> Drops {
        let s = &self.shared;
        let n = |a: &AtomicU64| a.load(Ordering::Relaxed);
        Drops { feeder: n(&s.feeder_late), plugin: n(&s.plugin_late), device: n(&s.device_late), heard: n(&s.heard) }
    }

    /// Readers of the dropout counters and the render time, for a simulated device to sample with every block
    /// (`crate::device::sim`): feeder, plugin, device, heard, render ns.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn probes(&self) -> Vec<crate::device::sim::Probe> {
        let s = &self.shared;
        let p = |name: &str, f: fn(&Shared) -> &AtomicU64| {
            let s = s.clone();
            crate::device::sim::Probe::new(name, move || f(&s).load(Ordering::Relaxed))
        };
        vec![p("feeder", |s| &s.feeder_late), p("plugin", |s| &s.plugin_late), p("device", |s| &s.device_late), p("heard", |s| &s.heard), p("render", |s| &s.render_ns)]
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

    /// How far playback and live notes rendered ahead of the device since the last call (`None`: nothing played).
    #[cfg(test)]
    pub(crate) fn take_ahead(&self) -> Option<Ahead> {
        self.shared.ahead.take()
    }

    /// The output device the stream that plays last opened.
    pub fn opened(&self) -> Option<DeviceInfo> {
        self.shared.opened.lock().ok()?.clone()
    }

    /// The open device fails as if it was unplugged (it opens again, see `audio_thread`).
    #[cfg(test)]
    fn fail_device(&self) {
        let _ = self.tx.send(Cmd::FailDevice);
    }

    /// Shows as playing without starting anything: what the UI sees between Play and the first sound (tests have no
    /// device, so a real Play stops again).
    #[cfg(test)]
    pub(crate) fn pretend_playing(&self) {
        self.shared.state.store(PLAYING, Ordering::Release);
    }

    /// Shows as playing at `tick` of the pattern (`song` false) or the playlist, without starting anything.
    #[cfg(test)]
    pub(crate) fn pretend_at(&self, tick: f64, song: bool) {
        if let Ok(mut p) = self.shared.pace.lock() {
            *p = None;
        }
        self.shared.state.store(PLAYING, Ordering::Release);
        self.shared.song.store(song, Ordering::Relaxed);
        self.shared.tick.store(tick.to_bits(), Ordering::Relaxed);
    }

    /// Streams open now (each feeder holds its voice counter in `Shared::engine_voices`).
    #[cfg(test)]
    fn streams(&self) -> usize {
        self.shared.engine_voices.lock().map_or(0, |v| v.iter().filter(|w| w.strong_count() > 0).count())
    }
}

/// Dropouts by kind (`Transport::drops`): device blocks the queue could not fill (the feeder was late), ticks a hosted
/// plugin missed (busy elsewhere), the device's own underruns; and of all of them, those heard over sound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Drops {
    pub feeder: u64,
    pub plugin: u64,
    pub device: u64,
    pub heard: u64,
}

/// Whole ticks handed from a stream's feeder thread to its device callback: one writer, one reader, no locks. With
/// each tick the feeder records where it starts in the queue, its tick and the engine's frame there, so the callback
/// can publish the position of what is being heard.
struct Queue {
    /// Samples in a frame: 2 × the device's output pairs (`Engine::width`).
    w: usize,
    samples: Box<[AtomicU32]>,
    /// Samples written / read since the start.
    written: AtomicU64,
    read: AtomicU64,
    /// Per tick: first sample, length in samples, tick (f64 bits), engine frame, and 1 for a held tick (paused: the
    /// position does not move through it).
    ticks: Box<[[AtomicU64; 6]]>,
    ticks_written: AtomicU64,
    /// The tick record the reader is in.
    ticks_read: AtomicU64,
    /// The engine has played everything out (a live engine starts again with its next note).
    ended: AtomicBool,
    /// Largest number of frames the device asked for at once (0 until it asked).
    block: AtomicUsize,
    /// The newest engine the feeder has started on (`Slot::epoch`), the sample its first tick starts at, and whether
    /// it comes in from silence; the one the callback plays.
    epoch: AtomicU64,
    epoch_at: AtomicU64,
    fade_in: AtomicBool,
    playing: AtomicU64,
}

impl Queue {
    fn new(w: usize) -> Queue {
        Queue {
            w,
            samples: (0..QUEUE_SAMPLES).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            read: AtomicU64::new(0),
            ticks: (0..QUEUE_TICKS).map(|_| Default::default()).collect(),
            ticks_written: AtomicU64::new(0),
            ticks_read: AtomicU64::new(0),
            ended: AtomicBool::new(false),
            block: AtomicUsize::new(0),
            epoch: AtomicU64::new(0),
            epoch_at: AtomicU64::new(0),
            fade_in: AtomicBool::new(false),
            playing: AtomicU64::new(0),
        }
    }

    /// Feeder: engine `epoch` starts at sample `at` (its first ticks are pushed already).
    fn begin(&self, epoch: u64, at: u64, fade_in: bool) {
        self.fade_in.store(fade_in, Ordering::Relaxed);
        self.epoch_at.store(at, Ordering::Release);
        self.epoch.store(epoch, Ordering::Release);
    }

    /// Where engine `epoch` starts in the queue, once the feeder has started on it.
    fn start_of(&self, epoch: u64) -> Option<u64> {
        let e = self.epoch.load(Ordering::Acquire);
        let at = self.epoch_at.load(Ordering::Acquire);
        (e == epoch && self.epoch.load(Ordering::Acquire) == e).then_some(at)
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
    fn push(&self, s: &[f32], tick: f64, frame: u64, held: bool, tick_frames: u64) {
        let w = self.written.load(Ordering::Relaxed);
        for (i, &v) in s.iter().enumerate() {
            self.samples[(w as usize + i) % QUEUE_SAMPLES].store(v.to_bits(), Ordering::Relaxed);
        }
        let tw = self.ticks_written.load(Ordering::Relaxed);
        let rec = &self.ticks[tw as usize % QUEUE_TICKS];
        rec[0].store(w, Ordering::Relaxed);
        rec[1].store(s.len() as u64, Ordering::Relaxed);
        rec[2].store(tick.to_bits(), Ordering::Relaxed);
        rec[3].store(frame, Ordering::Relaxed);
        rec[4].store(u64::from(held), Ordering::Relaxed);
        rec[5].store(tick_frames, Ordering::Relaxed);
        self.ticks_written.store(tw + 1, Ordering::Release);
        self.written.store(w + s.len() as u64, Ordering::Release);
    }

    /// Callback: copies up to `out.len()` samples, none from `until` on, and returns how many.
    fn pop(&self, out: &mut [f32], until: Option<u64>) -> usize {
        let r = self.read.load(Ordering::Relaxed);
        let w = self.written.load(Ordering::Acquire).min(until.unwrap_or(u64::MAX));
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
        // The samples a whole tick lasts: a block delay compensation cut short still moves at the song's pace.
        let w = self.w as u64;
        let (start, len) = (rec[0].load(Ordering::Relaxed), w * rec[5].load(Ordering::Relaxed).max(1));
        // A held tick is heard at its start all through (the paused position stays put).
        let into = if rec[4].load(Ordering::Relaxed) != 0 { 0 } else { r.saturating_sub(start) };
        Some((rec[3].load(Ordering::Relaxed) + into / w, f64::from_bits(rec[2].load(Ordering::Relaxed)) + into as f64 / len as f64))
    }
}

/// Renders whichever engine is in a stream's slot into its queue, ahead of the device (on its own thread; tests call
/// `fill` directly).
struct Feeder {
    slot: Arc<Mutex<Slot>>,
    queue: Arc<Queue>,
    shared: Arc<Shared>,
    ctl: Arc<Ctl>,
    /// The hosted plugins, reset when an engine handed in asks for it (`Slot::reset`).
    hosts: Instruments,
    /// Publish the mixer meters and the load (the stream that plays, not the previews').
    meters: bool,
    rate: u32,
    /// Time spent rendering and frames rendered since the load was last published.
    busy: Duration,
    rendered: u64,
    /// The engine's voices after the last fill (registered in `Shared::engine_voices`).
    voices: Arc<AtomicU32>,
    /// The engine rendered (`Slot::epoch`), where in the queue it started, and whether it comes in from silence.
    epoch: u64,
    epoch_at: u64,
    fade_in: bool,
}

/// Frames a stream keeps queued beyond a device block of `block` frames (see `MARGIN_MIN`).
fn margin(block: u64, rate: u32) -> u64 {
    let frames = |seconds: f64| (f64::from(rate) * seconds) as u64;
    block.clamp(frames(MARGIN_MIN), frames(MARGIN_MAX))
}

impl Feeder {
    fn new(slot: Arc<Mutex<Slot>>, queue: Arc<Queue>, shared: Arc<Shared>, ctl: Arc<Ctl>, rate: u32, hosts: Instruments) -> Feeder {
        let voices = Arc::new(AtomicU32::new(0));
        if let Ok(mut v) = shared.engine_voices.lock() {
            v.retain(|w| w.strong_count() > 0);
            v.push(Arc::downgrade(&voices));
        }
        Feeder { slot, queue, shared, ctl, hosts, meters: true, rate, busy: Duration::ZERO, rendered: 0, voices, epoch: 0, epoch_at: 0, fade_in: false }
    }

    /// Renders whole ticks until the queue holds one device block (`block` frames before the device has asked) and
    /// the margin beyond it, or the engine has played out. It stops at the first tick that gets there, so the device
    /// finds the block, the margin and less than a tick more queued. An engine just handed in starts after the resets
    /// it asked for, and only its own ticks count: the callback goes on to them once it has faded out the ones before.
    fn fill(&mut self, block: usize) {
        let slot = self.slot.clone();
        let Ok(mut slot) = slot.lock() else { return };
        let t0 = Instant::now();
        let begun = slot.epoch != self.epoch;
        if begun {
            let bits = std::mem::take(&mut slot.reset);
            if bits != 0 {
                reset_some(&self.hosts, bits);
            }
            (self.epoch, self.epoch_at, self.fade_in) = (slot.epoch, self.queue.written.load(Ordering::Relaxed), slot.fade_in);
        }
        match slot.engine.as_mut() {
            Some(e) => self.render(e, block, t0),
            None => {
                self.queue.ended.store(true, Ordering::Release);
                self.voices.store(0, Ordering::Relaxed);
            }
        }
        if begun {
            self.queue.begin(self.epoch, self.epoch_at, self.fade_in);
        }
    }

    fn render(&mut self, e: &mut Engine, block: usize, t0: Instant) {
        let block = block.max(self.queue.block.load(Ordering::Relaxed)) as u64;
        let w = self.queue.w as u64;
        let want = w * (block + margin(block, self.rate));
        // While the callback still plays the engine before, what that left queued does not count.
        let from = if self.queue.playing.load(Ordering::Acquire) == self.epoch { 0 } else { self.epoch_at };
        let queued = |q: &Queue| q.written.load(Ordering::Acquire).saturating_sub(q.read.load(Ordering::Acquire).max(from));
        // Song playback loops over the loop points as they are now.
        if self.ctl.report.load(Ordering::Relaxed) {
            let r = self.shared.loop_range.load(Ordering::Relaxed);
            e.set_loop_range((r != 0).then(|| (r >> 32, r & 0xffff_ffff)));
            match self.shared.jump.swap(0, Ordering::Relaxed) {
                0 => {}
                j => e.jump_on(j >> 32, j & 0xffff_ffff),
            }
        }
        let mut frames = 0u64;
        let mut peak = 0.0f32;
        while queued(&self.queue) < want && self.queue.has_room() {
            match e.next_tick() {
                None => {
                    self.queue.ended.store(true, Ordering::Release);
                    break;
                }
                Some((s, tick, frame, held, tick_frames)) => {
                    frames += s.len() as u64 / w;
                    peak = s.iter().fold(peak, |m, x| m.max(x.abs()));
                    let at = self.queue.written.load(Ordering::Relaxed) / w;
                    self.queue.push(s, tick, frame, held, tick_frames);
                    // The playback's meters wait for the device to play this tick.
                    if self.meters && self.ctl.report.load(Ordering::Relaxed) {
                        if let Ok(mut t) = self.shared.tracks.lock() {
                            t.tick(at, e);
                        }
                    }
                    self.queue.ended.store(e.finished(), Ordering::Release);
                }
            }
            // The song came to a Marker pause: the transport is paused there, as by the Play button (spec:
            // audio/transport.md "Time markers in SONG mode").
            if let Some(t) = e.take_marker_pause() {
                if self.ctl.report.load(Ordering::Relaxed) && self.shared.state.compare_exchange(PLAYING, PAUSED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    self.ctl.held.store(true, Ordering::Relaxed);
                    self.shared.marker_hold.store(t + 1, Ordering::Release);
                    if let Ok(mut v) = self.shared.sounding.lock() {
                        v.clear();
                    }
                }
            }
        }
        self.voices.store(e.voice_count() as u32, Ordering::Relaxed);
        // A hosted plugin that missed a tick (busy elsewhere) is heard as a gap: a dropout like an empty block.
        let missed = e.take_missed();
        if missed > 0 {
            self.shared.underruns.fetch_add(missed, Ordering::Relaxed);
            self.shared.plugin_late.fetch_add(missed, Ordering::Relaxed);
            if peak > SILENT || sound_lately(&self.shared) {
                self.shared.heard.fetch_add(missed, Ordering::Relaxed);
            }
        }
        if frames > 0 && self.meters {
            let spent = t0.elapsed();
            self.shared.render_ns.fetch_add(spent.as_nanos() as u64, Ordering::Relaxed);
            self.busy += spent;
            self.rendered += frames;
            if self.rendered >= u64::from(self.rate / 4) {
                let load = self.busy.as_secs_f64() * f64::from(self.rate) / self.rendered as f64;
                self.shared.load.store((load as f32).to_bits(), Ordering::Relaxed);
                (self.busy, self.rendered) = (Duration::ZERO, 0);
            }
            if let Ok(mut t) = self.shared.tracks.lock() {
                e.take_track_peaks(&mut t.now);
            }
        }
        // So does the tempo shown.
        if frames > 0 && self.ctl.report.load(Ordering::Relaxed) {
            self.shared.tempo.store(e.tempo().to_bits(), Ordering::Relaxed);
        }
        // Lit keys run ahead of what is heard by the queue's length (a few milliseconds).
        if frames > 0 && self.ctl.report.load(Ordering::Relaxed) {
            if let Ok(mut v) = self.shared.sounding.lock() {
                v.clear();
                if !e.paused() {
                    v.extend(e.sounding().map(|(c, k)| (c as u16, k)));
                }
            }
        }
    }
}

/// The audio thread ends with the transport: its streams, feeder threads and engines are gone, and it has let go of
/// the hosted plugins, before the app destroys them. Left running at exit, it still held the plugins of a playing or
/// paused stream; those were then destroyed as the process ended, after their plugin files had shut down, and the
/// app crashed on close.
impl Drop for Transport {
    fn drop(&mut self) {
        // The audio thread ends when its command channel closes.
        drop(std::mem::replace(&mut self.tx, mpsc::channel().0));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
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
pub(crate) fn raise_priority() {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

/// macOS: the user-interactive class keeps the feeder on the performance cores.
#[cfg(target_os = "macos")]
pub(crate) fn raise_priority() {
    // SAFETY: changes the calling thread's own QoS class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// Linux: real-time priority, by itself or through rtkit (`crate::rt`); at normal priority when neither allows it. The
/// mixing threads (`crate::mix_threads`) take it too.
#[cfg(target_os = "linux")]
pub(crate) fn raise_priority() {
    if let Err(e) = crate::rt::promote(crate::rt::tid()) {
        eprintln!("Audio: a rendering thread runs at normal priority ({e})");
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
pub(crate) fn raise_priority() {}

/// The sample library for engines whose sampler channels Lemon Sampler instances play. They stretch for themselves
/// (spec: audio/time-stretch.md), and of a sample one of them plays the engine keeps only the length and rate Lemon
/// Sampler reads from the file's headers, for the files it can play from disk (spec: plugins/lemon-sampler.md "Keep on
/// disk (Lemon Studio)"); other files are decoded, a copy shared with the instance playing it. The note rules learn from
/// Lemon Sampler which files it loops, so a looping voice gets its note's end.
pub fn host_library(factory: Option<PathBuf>) -> SampleLibrary {
    let mut library = SampleLibrary::new(factory);
    library.resample_stretches = true;
    library.length_reader = Some(lemon_sampler::stream::length);
    library.loop_reader = Some(lemon_sampler::file_loops);
    library
}

/// An engine for `project` with the hosted plugins attached (`plugins`; a preview plays its channels itself), all from
/// one look at the list. The instruments come with the engine (`Engine::with_instruments`), so it keeps no decoded copy
/// of the samples they play from their files.
fn engine(project: &Project, library: &mut SampleLibrary, rate: u32, mode: PlayMode, inst: &Instruments, plugins: bool) -> Engine {
    let (list, fx) = inst.lock().ok().filter(|_| plugins).map(|l| (l.inst.clone(), l.fx.clone())).unwrap_or_default();
    if plugins && std::env::var_os("OS_DEBUG").is_some() {
        eprintln!("[transport] attach {} instrument(s), {} effect(s)", list.len(), fx.len());
    }
    // The engine hands its instruments one tick per call: Lemon Sampler's voices step on those calls.
    lemon_sampler::set_host_ticks(project.ppq);
    let mut e = Engine::with_instruments(project, library, rate, mode, &list);
    crate::project_plugins::attach_effects(&mut e, project, fx);
    e
}

/// Renders a whole song (or one pass of a pattern, or a span of either) with the hosted plugins and writes the files of
/// `job`; returns the written length in seconds and the files. Runs on the audio thread with no stream open.
// spec: audio/export.md
fn export(project: &Project, mode: PlayMode, rate: u32, job: &crate::export::Job, library: &mut SampleLibrary, inst: &Instruments, shared: &Shared) -> Result<(f64, Vec<PathBuf>), String> {
    let o = &job.options;
    let mut e = engine(project, library, rate, mode, inst, true);
    e.set_interpolation(o.points);
    e.set_effects(o.insert_fx, o.master_fx);
    match job.span {
        Some((a, b)) => e.set_span(a, b),
        // A whole song follows its start and skip markers (spec: audio/song-structure.md "Time markers").
        None if mode == PlayMode::Song => e.follow_markers(),
        None => {}
    }
    e.set_track_capture(o.split);
    let len = e.frames_left().max(1);
    // Cut needs nothing past the song's end.
    let cap = if o.tail == crate::export::Tail::Cut { len } else { len + u64::from(rate) * EXPORT_TAIL_MAX_SECS };
    let mut audio: Vec<f32> = Vec::with_capacity(2 * (len as usize + rate as usize));
    loop {
        let done = e.render_ticks(32, &mut audio);
        let frames = audio.len() as u64 / 2;
        shared.export_permille.store((frames * 1000 / len).min(999) as u32, Ordering::Relaxed);
        if done || frames >= cap {
            break;
        }
    }
    let tracks = e.take_track_audio();
    drop(e);
    // The file's length: the song, plus the tail when it rings out.
    let frames = if o.tail == crate::export::Tail::Ring { audio.len() as u64 / 2 } else { len };
    let files = crate::export::write(job, rate, audio, tracks, len)?;
    Ok((frames as f64 / f64::from(rate), files))
}

/// What a reset of the hosted plugins covers (`reset_some`, `Slot::reset`): the instruments (voices, tails, queued
/// notes), the effects (tails, delay lines).
const RESET_INST: u8 = 1;
const RESET_FX: u8 = 2;

/// Silences every hosted instrument and effect. Only called while no engine processes them.
fn reset_instruments(inst: &Instruments) {
    reset_hosts(inst, true);
}

/// Resets every hosted instrument, and every hosted effect when `effects`. Only called while no engine processes them.
///
/// A start of playback (Play, a jump while playing) calls it with "Reset plugins on transport" (spec:
/// ui/settings-choices.md "Transport: Play truncated notes and Reset plugins"): on, every hosted plugin is reset before
/// the first block at the new place; off, the effects are left alone, so a delay's tail rings on through the start. The
/// instruments are reset either way at a jump: the new engine does not know the notes the old one held at them (a
/// Lemon Studio choice; what FL does to an instrument's voices at a jump is not measured). A Play while a Stop's tails
/// ring leaves them alone too: the Stop let go of their notes, and their releases ring on into the new playback, as in
/// FL (spec: audio/transport.md "Tails after Pause and Stop").
fn reset_hosts(inst: &Instruments, effects: bool) {
    reset_some(inst, RESET_INST | if effects { RESET_FX } else { 0 });
}

/// Resets the hosted plugins `bits` names (`RESET_INST`, `RESET_FX`).
fn reset_some(inst: &Instruments, bits: u8) {
    let (list, fx): (Vec<SharedInstrument>, Vec<SharedEffect>) = inst
        .lock()
        .map(|l| {
            let list = l.inst.iter().filter(|_| bits & RESET_INST != 0).map(|(_, i)| i.clone()).collect();
            (list, l.fx.iter().filter(|_| bits & RESET_FX != 0).map(|(_, _, f)| f.clone()).collect())
        })
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

/// A playback: the request it answers, the project playing (edits replace it), its mode, and "Play truncated notes on
/// transport" as it started with (a move to new audio settings keeps it).
struct Play {
    generation: u64,
    project: Project,
    mode: PlayMode,
    truncated: bool,
}

/// What the stream's engine is.
enum Role {
    /// None, or one that has played out.
    Idle,
    /// The playback, playing or paused in the engine (`Ctl::held`).
    Song(Play),
    /// The stopped playback ringing out its effects' tails: its channels held as at Pause, its mixer running on.
    Ringing(Play, Ringing),
    /// Live notes while nothing plays (an engine with no schedule, `live_project`), and the instruments while an
    /// editor is open.
    Live,
}

/// The audio thread's state. The output device's stream (`main`) lasts while the device is open; whatever plays is its
/// engine, handed in or changed in place, so the hosted plugins are only ever processed by its feeder: one engine at a
/// time, the next one after the resets it asked for (`Stream::swap`).
struct Audio {
    shared: Arc<Shared>,
    latest: Arc<Mutex<Option<Project>>>,
    inst: Instruments,
    library: SampleLibrary,
    /// The stream that plays: playback, live notes, tails, the instruments while an editor is open.
    main: Option<Stream>,
    /// One-shot previews play on a stream of their own (their engines host no plugins), so they also sound while
    /// playing or paused; when a time-limited one (browser samples) is faded out.
    pre: Option<Stream>,
    preview_until: Option<Instant>,
    role: Role,
    /// A paused playback while the live engine has the instruments (live notes, an open editor; it lets go of the notes
    /// it held at them, `Engine::release_instruments`), or one paused when it moved to a new stream.
    parked: Option<(Engine, Play)>,
    live_project: Option<Project>,
    /// Resets owed to the hosted plugins: done with the next engine handed in (`Stream::swap`), or here once nothing
    /// processes them (a stopped playback's instruments once its channels have faded while its tails ring).
    pend: u8,
    /// The instruments may hold something since they were last reset: a start of playback resets them.
    dirty: bool,
    /// An editor is open: the live engine plays while nothing plays (`Transport::listen`). One that would not start is
    /// tried again on the next change of state, not every loop.
    listen: bool,
    listen_retry: bool,
    /// Auto close (`Transport::close_device`): no stream opens while `closed`; the playback waits in `held` (project,
    /// mode, generation, where); the streams fade out until `closing`, then they and the device go.
    closed: bool,
    closing: Option<Instant>,
    held: Option<(Project, PlayMode, u64, At)>,
    /// When a Stop's rewind is made again, once the callback can no longer publish a position from before it.
    rewind_at: Option<Instant>,
    /// The device failed while in use; the playback waits in `held` until it opens again.
    lost: Option<Lost>,
    /// An ASIO driver chosen opens at once (at the start, with new settings), not with the first note: it stays open,
    /// and loading it takes a while. When it does not open it is tried again (`ASIO_TRIES`): when, and the tries so far.
    open_asio: Option<(Instant, usize)>,
    /// Every stream above plays through it (dropped after them).
    device: OutputDevice,
}

fn audio_thread(rx: Receiver<Cmd>, shared: Arc<Shared>, latest: Arc<Mutex<Option<Project>>>, inst: Instruments, library: SampleLibrary) {
    let mut a = Audio {
        shared,
        latest,
        inst,
        library,
        main: None,
        pre: None,
        preview_until: None,
        role: Role::Idle,
        parked: None,
        live_project: None,
        pend: 0,
        dirty: true,
        listen: false,
        listen_retry: false,
        closed: false,
        closing: None,
        held: None,
        rewind_at: None,
        lost: None,
        open_asio: (!HEADLESS.load(Ordering::Acquire)).then(|| (Instant::now(), 0)),
        device: OutputDevice::default(),
    };
    // OS_DEBUG: how far playback renders ahead of the device, every 2 s.
    let debug = std::env::var_os("OS_DEBUG").is_some();
    let mut logged = Instant::now();
    loop {
        crate::device::pump();
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(cmd) => a.command(cmd),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        a.after();
        if debug && logged.elapsed() >= Duration::from_secs(2) {
            logged = Instant::now();
            a.log();
        }
    }
}

impl Audio {
    fn report(&self, e: String) {
        if let Ok(mut slot) = self.shared.error.lock() {
            *slot = Some(e);
        }
    }

    /// The stream that plays, opened (with the device) if it is not, or not on the device open now.
    fn stream(&mut self) -> Result<&Stream, String> {
        if self.main.as_ref().is_some_and(|s| s.opens != crate::device::opens() || self.device.failure().is_some()) {
            self.main = None;
            self.role = Role::Idle;
        }
        if self.main.is_none() {
            self.main = Some(Stream::open(&mut self.device, &self.shared, &self.inst, false)?);
        }
        Ok(self.main.as_ref().expect("opened"))
    }

    /// The previews' stream, opened if it is not.
    fn preview_stream(&mut self) -> Result<&Stream, String> {
        if self.pre.as_ref().is_some_and(|s| s.opens != crate::device::opens()) {
            self.pre = None;
        }
        if self.pre.is_none() {
            self.pre = Some(Stream::open(&mut self.device, &self.shared, &self.inst, true)?);
        }
        Ok(self.pre.as_ref().expect("opened"))
    }

    /// An engine for the stream `s` plays on: the playback of `project` from `at`, live notes, or a preview.
    fn build(&mut self, project: &Project, mode: PlayMode, kind: Kind, at: At, rate: u32, pairs: usize) -> Engine {
        let song = kind == Kind::Song;
        let mut e = engine(project, &mut self.library, rate, mode, &self.inst, kind != Kind::Preview);
        // The device's output pairs, and its inputs for the mixer tracks that hear them (a preview hears none; spec:
        // audio/track-io.md §2.2).
        let input = (kind != Kind::Preview).then(|| crate::device::input_reader((f64::from(rate) * MARGIN_MAX) as u64));
        e.set_device(pairs, input.map(|i| Box::new(i) as Box<dyn os_engine::DeviceInput>));
        // Playback loops: a pattern on its bar line, the song at its end (back to the start).
        e.set_looping(song);
        if kind == Kind::Live {
            e.clear_schedule();
            e.set_awake(self.listen);
        }
        // Playback clicks with the metronome switch; live notes and previews never do (spec: ui/metronome.md §4).
        if song {
            e.set_metronome(crate::metronome::switch());
        }
        // Seek before anything is rendered, so the first block is already the start position.
        if at.tick > 0.0 {
            e.set_truncated_on_transport(at.truncated);
            e.seek_tick(at.tick as u64);
            if song {
                let tempo = project.tick_tempo(project.tempo.unwrap_or(140.0));
                self.shared.position.store(os_engine::laws::tick_to_frame(at.tick, project.ppq.max(1), tempo, rate), Ordering::Relaxed);
            }
        }
        if song && at.count_in > 0 {
            e.set_count_in(at.count_in);
        }
        if song {
            e.set_run_on(crate::record::run_on_flag());
            e.set_nudge(crate::tapper::nudge_flag());
            e.set_record_tap(self.shared.record.lock().ok().and_then(|t| t.clone()));
            if let Some(gate) = self.shared.gate.lock().ok().and_then(|g| *g) {
                e.set_load_gate(gate);
            }
        }
        e
    }

    /// Hands `e` (or nothing) to the stream that plays, with the resets owed; the engine it played goes (freed here).
    fn hand(&mut self, e: Option<Engine>, fade_in: bool, reset: u8, report: bool) {
        let reset = reset | std::mem::take(&mut self.pend);
        if let Some(s) = &self.main {
            drop(s.swap(e, fade_in, reset, report));
        } else if reset != 0 {
            reset_some(&self.inst, reset);
        }
        if reset & RESET_INST != 0 {
            self.dirty = false;
        }
    }

    /// Remembers how the playback uses the workers for the one that follows.
    fn keep_gate(&self) {
        let gate = match (&self.role, &self.parked) {
            (Role::Song(_), _) => self.main.as_ref().and_then(|s| s.with(|e| e.load_gate())),
            (_, Some((e, _))) => Some(e.load_gate()),
            _ => None,
        };
        if let (Some(g), Ok(mut slot)) = (gate, self.shared.gate.lock()) {
            *slot = Some(g);
        }
    }

    /// The playback plays now (not paused in the engine).
    fn playing(&self) -> bool {
        matches!(self.role, Role::Song(_)) && self.main.as_ref().is_some_and(|s| !s.ctl.held.load(Ordering::Relaxed))
    }

    /// The live engine plays on the stream, built from `project` if another plays there (a paused playback is parked,
    /// letting go of what it held at the instruments; tails still ringing after Stop ring on there). False
    /// with no project to build it from or no device.
    fn go_live(&mut self, project: Option<&Project>) -> bool {
        if matches!(self.role, Role::Live) {
            return true;
        }
        let Some(project) = project else { return false };
        let (rate, pairs) = match self.stream() {
            Ok(s) => (s.rate, s.pairs),
            Err(e) => {
                self.report(e);
                return false;
            }
        };
        let e = self.build(project, PlayMode::Pattern(1), Kind::Live, At::default(), rate, pairs);
        if let Some((p, _)) = self.parked.as_mut() {
            p.release_instruments();
        }
        match std::mem::replace(&mut self.role, Role::Live) {
            Role::Song(p) => {
                let s = self.main.as_ref().expect("opened");
                s.with(|e| e.release_instruments());
                let old = s.swap(Some(e), false, std::mem::take(&mut self.pend), false);
                self.parked = old.map(|e| (e, p));
            }
            _ => self.hand(Some(e), false, 0, false),
        }
        self.dirty = true;
        true
    }

    /// The engine a live note goes to plays now: the playback while it plays, else the live engine.
    fn live_target(&mut self, tap: bool) -> bool {
        if self.playing() {
            return true;
        }
        // A click needs no channel: it also starts the live engine before any project was sent.
        let project = self.live_project.clone().or(tap.then(Project::default));
        self.go_live(project.as_ref())
    }

    /// The playback and a paused one parked (each rebuilt for an edited project, the instruments or the factory
    /// folder): a new engine takes over at the playing position (`Engine::continue_from`), built outside the lock.
    fn rebuild(&mut self) {
        let Audio { role, parked, main, library, inst, live_project, .. } = self;
        let Some(s) = main.as_ref() else { return };
        let spec = match role {
            Role::Song(p) | Role::Ringing(p, _) => Some((&p.project, p.mode, false)),
            Role::Live => live_project.as_ref().map(|p| (p, PlayMode::Pattern(1), true)),
            Role::Idle => None,
        };
        if let Some((project, mode, live)) = spec {
            let mut e = engine(project, library, s.rate, mode, inst, true);
            if live {
                e.clear_schedule();
            }
            let old = s.slot.lock().ok().and_then(|mut slot| {
                let old = slot.engine.as_mut()?;
                e.continue_from(old);
                Some(std::mem::replace(old, e))
            });
            drop(old); // freed after the feeder has the new engine
        }
        if let Some((old, p)) = parked.as_mut() {
            let mut e = engine(&p.project, library, s.rate, p.mode, inst, true);
            e.continue_from(old);
            *old = e;
        }
    }

    /// Starts playback `project` at `at` on the stream (paused: parked there until Resume).
    fn start(&mut self, project: Project, mode: PlayMode, generation: u64, at: At) -> Result<(), String> {
        let (rate, pairs) = self.stream().map(|s| (s.rate, s.pairs))?;
        let e = self.build(&project, mode, Kind::Song, at, rate, pairs);
        let p = Play { generation, project, mode, truncated: at.truncated };
        if at.paused {
            self.parked = Some((e, p));
        } else {
            self.main.as_ref().expect("opened").ctl.held.store(false, Ordering::Relaxed);
            // Started in the middle of the song, it comes in from silence.
            self.hand(Some(e), at.tick > 0.0, 0, true);
            self.role = Role::Song(p);
        }
        self.dirty = true;
        Ok(())
    }

    /// The playback, playing or paused, and where it is heard: what a new stream starts again from.
    fn take_playback(&mut self) -> Option<(Play, At)> {
        let tick = f64::from_bits(self.shared.tick.load(Ordering::Relaxed));
        let at = |p: &Play, paused: bool| At { tick, paused, truncated: p.truncated, count_in: 0 };
        let held = self.main.as_ref().is_some_and(|s| s.ctl.held.load(Ordering::Relaxed));
        let song = match std::mem::replace(&mut self.role, Role::Idle) {
            Role::Song(p) => Some((at(&p, held), p)),
            _ => None,
        };
        let parked = self.parked.take().map(|(_, p)| (at(&p, true), p));
        song.or(parked).map(|(a, p)| (p, a))
    }

    fn command(&mut self, cmd: Cmd) {
        match cmd {
            // The output is closed by Auto close: previews and live notes find no stream and open none.
            // So is a device that failed, until it opens again.
            Cmd::Preview(_) | Cmd::PreviewFor(..) | Cmd::PreviewSong(_) | Cmd::NoteOn(..) | Cmd::Midi(..) | Cmd::TapClick(..) if self.closed || self.lost.is_some() => {}
            Cmd::LiveEdits(edits) => {
                if let Some(s) = &self.main {
                    s.with(|e| edits.iter().for_each(|&x| e.apply_edit(x)));
                }
                if let Some((e, _)) = self.parked.as_mut() {
                    edits.iter().for_each(|&x| e.apply_edit(x));
                }
                let role = match &mut self.role {
                    Role::Song(p) | Role::Ringing(p, _) => Some(&mut p.project),
                    _ => None,
                };
                for p in [role, self.parked.as_mut().map(|p| &mut p.1.project), self.held.as_mut().map(|h| &mut h.0), self.live_project.as_mut()].into_iter().flatten() {
                    edits.iter().for_each(|x| x.apply_to(p));
                }
            }
            Cmd::Update => {
                if let Some(project) = self.latest.lock().ok().and_then(|mut l| l.take()) {
                    if let Role::Song(p) | Role::Ringing(p, _) = &mut self.role {
                        p.project = project.clone();
                    }
                    if let Some((_, p)) = self.parked.as_mut() {
                        p.project = project.clone();
                    }
                    if let Some(h) = self.held.as_mut() {
                        h.0 = project.clone();
                    }
                    self.live_project = Some(project);
                    self.rebuild();
                }
            }
            Cmd::Record => {
                let tap = self.shared.record.lock().ok().and_then(|t| t.clone());
                if let (Role::Song(_), Some(s)) = (&self.role, &self.main) {
                    s.with(|e| e.set_record_tap(tap.clone()));
                }
                if let Some((e, _)) = self.parked.as_mut() {
                    e.set_record_tap(tap);
                }
            }
            Cmd::Instruments => self.rebuild(),
            Cmd::Listen(on) => {
                self.listen = on;
                self.listen_retry = true;
                if let (Role::Live, Some(s)) = (&self.role, &self.main) {
                    s.with(|e| e.set_awake(on));
                }
            }
            cmd @ (Cmd::NoteOn(..) | Cmd::Midi(..) | Cmd::TapClick(..)) => {
                if self.live_target(matches!(cmd, Cmd::TapClick(..))) {
                    if let Some(s) = &self.main {
                        s.with(|e| match cmd {
                            Cmd::NoteOn(ch, key, vel, midi) => e.note_on_midi(ch, key, vel, midi),
                            Cmd::Midi(ch, bytes, bend) => e.midi(ch, bytes, bend),
                            Cmd::TapClick(accent, db) => e.tap_click(accent, db),
                            _ => {}
                        });
                    }
                }
            }
            Cmd::NoteOff(ch, key) => {
                if let Some(s) = &self.main {
                    s.with(|e| e.note_off(ch, key));
                }
                if let Some((e, _)) = self.parked.as_mut() {
                    e.note_off(ch, key);
                }
            }
            Cmd::PreviewFor(project, limit) => self.preview(&project, PlayMode::Pattern(1), limit),
            Cmd::PreviewSong(project) => self.preview(&project, PlayMode::Song, None),
            Cmd::Preview(project) => {
                // A plugin channel's preview plays as a timed live note on the engine that hosts the instrument.
                let note = project.patterns.first().and_then(|p| p.notes.first()).copied();
                let row = note.and_then(|n| project.channels.iter().position(|c| c.index == n.channel));
                let hosted = row.is_some_and(|r| self.inst.lock().is_ok_and(|l| l.inst.iter().any(|(i, _)| *i == r)));
                match (hosted, note, row) {
                    (true, Some(n), Some(r)) => {
                        let ready = self.playing() || {
                            let p = self.live_project.clone().unwrap_or_else(|| (*project).clone());
                            self.go_live(Some(&p))
                        };
                        if let (true, Some(s)) = (ready, &self.main) {
                            let tempo = project.tick_tempo(project.tempo.unwrap_or(140.0));
                            let frames = os_engine::laws::tick_to_frame(f64::from(n.length.max(1)), project.ppq.max(1), tempo, s.rate);
                            s.with(|e| e.note_for(r, n.key, n.velocity, frames));
                        }
                    }
                    _ => self.preview(&project, PlayMode::Pattern(1), None),
                }
            }
            Cmd::Play(project, mode, start, generation, starts) => self.play(*project, mode, start, generation, starts),
            Cmd::Reopen => self.reopen(),
            Cmd::Pause => {
                // The engine holds its channels where they are and lets the mixer's effects ring out.
                if let (Role::Song(_), Some(s)) = (&self.role, &self.main) {
                    s.ctl.held.store(true, Ordering::Relaxed);
                    s.with(|e| e.pause());
                }
                self.listen_retry = true;
            }
            Cmd::Resume => self.resume(),
            Cmd::Stop(ring) => self.stop(ring),
            Cmd::Factory(dir) => {
                self.library.set_factory_dir(dir);
                self.rebuild();
            }
            Cmd::AsioPanel if self.closed => self.report("the output is let go of (Auto close)".into()),
            Cmd::AsioPanel => {
                if let Err(e) = self.device.asio_panel() {
                    self.report(e);
                }
            }
            Cmd::Close => self.close(),
            Cmd::Open => self.open(),
            #[cfg(test)]
            Cmd::FailDevice => self.device.fail("test"),
            Cmd::Export(project, mode, rate, job) => self.export(&project, mode, rate, &job),
        }
    }

    /// One note (or a clip, a sample) once on the previews' stream, replacing the one sounding, faded out after
    /// `limit` if still sounding.
    fn preview(&mut self, project: &Project, mode: PlayMode, limit: Option<Duration>) {
        self.preview_until = limit.map(|d| Instant::now() + d);
        let (rate, pairs) = match self.preview_stream() {
            Ok(s) => (s.rate, s.pairs),
            Err(e) => return self.report(e),
        };
        let e = self.build(project, mode, Kind::Preview, At::default(), rate, pairs);
        if let Some(s) = &self.pre {
            drop(s.swap(Some(e), false, 0, false));
        }
    }

    fn play(&mut self, project: Project, mode: PlayMode, start: f64, generation: u64, starts: Starts) {
        // A newer request (stop, another play) already superseded this one.
        if generation != self.shared.generation.load(Ordering::Acquire) {
            return;
        }
        // A Play asked for while Auto close holds the output opens it (and drops the playback it held); so does one
        // while the device that failed is being tried again.
        self.closed = false;
        self.closing = None;
        self.held = None;
        self.lost = None;
        // The engine before fades out from what it has queued while the new one starts: the playback, the live
        // engine, tails ringing after Stop (with "Reset plugins on transport" off their hosted plugins ring on through
        // the start, `reset_hosts`). Typed notes go into the playing engine.
        self.keep_gate();
        self.parked = None;
        let ringing = matches!(std::mem::replace(&mut self.role, Role::Idle), Role::Ringing(..));
        self.listen_retry = true;
        self.live_project = Some(project.clone());
        let at = At { tick: start, paused: false, truncated: starts.truncated, count_in: starts.count_in };
        // The instruments are reset unless nothing has played them since they were, or they ring out what the Stop
        // before let go of (`reset_hosts`).
        let inst = self.dirty && !ringing;
        self.pend |= if inst { RESET_INST } else { 0 } | if starts.reset { RESET_INST | RESET_FX } else { 0 };
        if let Err(e) = self.start(project, mode, generation, at) {
            self.report(e);
            let _ = self.shared.state.compare_exchange(PLAYING, STOPPED, Ordering::AcqRel, Ordering::Acquire);
            self.shared.rewind(0.0);
        }
    }

    fn resume(&mut self) {
        // A playback parked while the live engine played: it takes the stream again, coming in from silence (the
        // live engine fades out; the playback delivers its own note offs).
        if let Some((mut e, p)) = self.parked.take() {
            e.resume();
            match self.stream() {
                Ok(s) => s.ctl.held.store(false, Ordering::Relaxed),
                Err(err) => {
                    self.report(err);
                    self.parked = Some((e, p));
                    return;
                }
            }
            self.hand(Some(e), true, 0, true);
            self.role = Role::Song(p);
            self.dirty = true;
            return;
        }
        if let (Role::Song(_), Some(s)) = (&self.role, &self.main) {
            s.with(|e| e.resume());
            s.ctl.held.store(false, Ordering::Relaxed);
        }
    }

    /// Stop: the playback's channels fade out. Its hosted plugins ring out their tails (an instrument's release and
    /// its own effects, the mixer's reverb and delay) as in FL: its channels are held as at Pause (`Engine::pause`:
    /// the instruments are let go of their notes and go on being processed) while its mixer runs on, until the tails
    /// are quiet (`Ringing`), and the plugins are reset then, so no frozen tail comes back with the next engine.
    /// Without `ring` (Stop sound), with no hosted plugin, or at a Stop while tails ring, they are reset at once.
    /// Previews and live notes end.
    fn stop(&mut self, ring: bool) {
        self.keep_gate();
        if let Some(p) = &self.pre {
            drop(p.swap(None, false, 0, false));
        }
        self.preview_until = None;
        self.held = None;
        // Paused with the live engine processing its plugins: its effects rang there.
        let parked = self.parked.take().is_some();
        let hosted = self.inst.lock().is_ok_and(|l| !l.fx.is_empty() || !l.inst.is_empty());
        match std::mem::replace(&mut self.role, Role::Idle) {
            Role::Song(p) if ring && hosted && !parked && self.main.is_some() => {
                let s = self.main.as_ref().expect("checked");
                s.ctl.report.store(false, Ordering::Relaxed);
                s.ctl.held.store(true, Ordering::Relaxed);
                s.with(|e| e.pause());
                self.role = Role::Ringing(p, Ringing::new(Instant::now()));
            }
            _ => self.hand(None, false, RESET_INST | RESET_FX, false),
        }
        self.rewind_at = Some(Instant::now() + CLOSE_AFTER);
        self.listen_retry = true;
    }

    fn reopen(&mut self) {
        self.listen_retry = true;
        self.open_asio = Some((Instant::now(), 0));
        self.preview_until = None;
        let ringing = matches!(self.role, Role::Ringing(..));
        let playback = self.take_playback();
        // The next stream opens the device with the new settings; a device that failed is tried with them at once.
        // Until one opens, the page says so rather than show the device let go of.
        self.main = None;
        self.pre = None;
        self.closing = None;
        self.device.close();
        if let Ok(mut o) = self.shared.opened.lock() {
            *o = None;
        }
        if let Some(l) = self.lost.as_mut() {
            (l.next, l.tries) = (Instant::now(), 0);
        }
        // Tails ringing after Stop go with their stream; their effects start again from silence. The playback goes on
        // from the position heard on a stream with the new settings; the new engine does not know the notes the old
        // one held at the instruments.
        if ringing || playback.is_some() {
            reset_instruments(&self.inst);
            (self.pend, self.dirty) = (0, false);
        }
        if let Some((p, at)) = playback {
            let generation = p.generation;
            if let Err(e) = self.start(p.project, p.mode, generation, at) {
                self.report(e);
                if generation == self.shared.generation.load(Ordering::Acquire) {
                    self.shared.state.store(STOPPED, Ordering::Release);
                    self.shared.rewind(0.0);
                }
            }
        }
    }

    fn close(&mut self) {
        self.closed = true;
        // The playback (paused by the app) is held where it was heard; every stream fades out and closes, then the
        // instruments are reset as after Stop (without the rewind: the transport is paused), the effects too if tails
        // rang.
        let ringing = matches!(self.role, Role::Ringing(..));
        let live = matches!(self.role, Role::Live);
        if let Some((p, mut at)) = self.take_playback() {
            at.paused = true;
            self.held = Some((p.project, p.mode, p.generation, at));
            self.pend |= RESET_INST;
        }
        self.role = Role::Idle;
        if ringing {
            self.pend |= RESET_INST | RESET_FX;
        }
        if live {
            self.pend |= RESET_INST;
        }
        for s in [&self.main, &self.pre].into_iter().flatten() {
            drop(s.swap(None, false, 0, false));
        }
        self.preview_until = None;
        self.closing = Some(Instant::now());
    }

    fn open(&mut self) {
        self.closed = false;
        self.closing = None;
        self.lost = None;
        self.listen_retry = true;
        self.open_asio = Some((Instant::now(), 0));
        // The held playback goes on at its place, unless a Stop or another Play came since; playing if the transport
        // was resumed meanwhile, else paused (`resume` follows).
        if let Some((project, mode, generation, mut at)) = self.held.take().filter(|h| h.2 == self.shared.generation.load(Ordering::Acquire)) {
            at.paused = self.shared.state.load(Ordering::Acquire) != PLAYING;
            if let Err(e) = self.start(project, mode, generation, at) {
                self.report(e);
                self.shared.state.store(STOPPED, Ordering::Release);
                self.shared.rewind(0.0);
            }
        }
        if self.pend != 0 {
            self.hand(None, false, 0, false);
        }
    }

    /// Renders and writes the files of `job` here, with the hosted plugins: every engine that could process them goes
    /// at once, and they start from silence (the streams stay, silent).
    fn export(&mut self, project: &Project, mode: PlayMode, rate: u32, job: &crate::export::Job) {
        for s in [&self.main, &self.pre].into_iter().flatten() {
            drop(s.quiet());
        }
        self.role = Role::Idle;
        self.parked = None;
        self.held = None;
        self.preview_until = None;
        reset_instruments(&self.inst);
        self.pend = 0;
        self.shared.rewind(0.0);
        let result = export(project, mode, rate, job, &mut self.library, &self.inst, &self.shared);
        reset_instruments(&self.inst);
        self.dirty = false;
        if let Ok(mut slot) = self.shared.export_result.lock() {
            *slot = Some(result);
        }
        self.shared.exporting.store(false, Ordering::Release);
        self.listen_retry = true;
    }

    /// What every loop does after its command: timed fades, tails, resets, the device's state, the live engine.
    fn after(&mut self) {
        if self.preview_until.is_some_and(|t| Instant::now() >= t) {
            self.preview_until = None;
            if let Some(s) = &self.pre {
                drop(s.swap(None, false, 0, false));
            }
        }
        // Auto close: the streams have faded out; they go, with the device, and the instruments are silenced.
        if self.closing.is_some_and(|t| t.elapsed() >= CLOSE_AFTER) {
            self.closing = None;
            self.main = None;
            self.pre = None;
            self.device.close();
            reset_some(&self.inst, std::mem::take(&mut self.pend));
        }
        // The block the callback was playing as Stop came may have published its position after the UI's rewind.
        if self.rewind_at.is_some_and(|t| Instant::now() >= t) {
            self.rewind_at = None;
            if !matches!(self.role, Role::Song(_)) && self.shared.state.load(Ordering::Acquire) == STOPPED {
                self.shared.rewind(0.0);
            }
        }
        self.ring();
        let xruns = self.device.take_xruns();
        self.shared.underruns.fetch_add(xruns, Ordering::Relaxed);
        self.shared.device_late.fetch_add(xruns, Ordering::Relaxed);
        if xruns > 0 && sound_lately(&self.shared) {
            self.shared.heard.fetch_add(xruns, Ordering::Relaxed);
        }
        self.lose();
        if !self.closed {
            self.find();
        }
        // An ASIO driver that did not open, and what plays instead.
        if let Some(note) = self.device.take_note() {
            self.report(note);
        }
        // The device frees the streams it let go of. It stays open while the app runs (spec: audio/devices.md §2.3,
        // silent when nothing plays), unless Auto close lets go of it: then also one that would stay open.
        self.device.collect();
        if self.closed && self.main.is_none() && self.pre.is_none() {
            self.device.close();
        }
        // The system's input device, with the output, while a track has an input chosen.
        if let Some(e) = self.device.sync_inputs() {
            self.report(e);
        }
        if let Some((_, tries)) = self.open_asio.filter(|&(at, _)| !self.closed && self.lost.is_none() && Instant::now() >= at) {
            self.open_asio = None;
            if crate::audio_cfg::AudioCfg::current().asio_driver().is_some() {
                self.device.wait = tries < ASIO_TRIES;
                let opened = self.device.open().map(|o| o.info.clone());
                self.device.wait = false;
                match opened {
                    Ok(info) => publish(&self.shared, &info),
                    Err(_) if tries < ASIO_TRIES => self.open_asio = Some((Instant::now() + ASIO_AGAIN, tries + 1)),
                    Err(e) => self.report(e),
                }
            }
        }
        // While an editor is open and nothing plays, the live engine processes the instruments. Tails ringing after
        // Stop give way to it.
        let still = match &self.role {
            Role::Idle | Role::Ringing(..) => true,
            Role::Song(_) => !self.playing(),
            Role::Live => false,
        };
        if self.listen && self.listen_retry && !self.closed && self.closing.is_none() && self.lost.is_none() && still {
            self.listen_retry = false;
            let p = self.live_project.clone();
            if p.is_some() && !self.go_live(p.as_ref()) {
                eprintln!("lemonstudio: no live engine for the open plugin editor");
            }
        }
        // A pattern loops inside the engine; a song played out (it never does while it loops) stops.
        let over = match (&self.role, &self.main) {
            (Role::Song(p), Some(s)) => s.played_out().then_some(p.generation),
            _ => None,
        };
        if let Some(generation) = over {
            self.hand(None, false, 0, false);
            self.role = Role::Idle;
            if generation == self.shared.generation.load(Ordering::Acquire) {
                self.shared.state.store(STOPPED, Ordering::Release);
                self.shared.rewind(0.0);
            }
            self.listen_retry = true;
        }
    }

    /// Tails ringing after Stop: the sampler voices the playback held go once its channels have faded out (they never
    /// sound again); the hosted plugins are reset when the tails are over, as the engine goes.
    fn ring(&mut self) {
        let Role::Ringing(_, r) = &mut self.role else { return };
        let Some(s) = &self.main else { return };
        let since = r.since;
        let rung = r.over(f32::from_bits(s.ctl.level.swap(0, Ordering::Relaxed)), Instant::now());
        if rung {
            self.role = Role::Idle;
            self.hand(None, false, RESET_INST | RESET_FX, false);
            self.dirty = false;
            if self.shared.state.load(Ordering::Acquire) == STOPPED {
                self.shared.rewind(0.0);
            }
            return;
        }
        if !r.forgot && since.elapsed() >= CLOSE_AFTER && s.with(|e| e.held().then(|| e.forget_voices()).is_some()).unwrap_or(false) {
            r.forgot = true;
            if self.shared.state.load(Ordering::Acquire) == STOPPED {
                self.shared.rewind(0.0);
            }
        }
    }

    /// A device that failed (it went away, a driver asked to be reset): the playback is held where it was heard and
    /// the streams go; it goes on when the device opens again (spec: audio/devices.md).
    fn lose(&mut self) {
        if self.lost.is_some() {
            return;
        }
        let Some((name, why)) = self.device.failure() else { return };
        if let Some((p, at)) = self.take_playback() {
            self.held = Some((p.project, p.mode, p.generation, at));
        }
        self.role = Role::Idle;
        self.main = None;
        self.pre = None;
        self.preview_until = None;
        self.closing = None;
        self.device.close();
        // As after Reopen: the new engines do not know the notes the old ones held at the instruments.
        reset_instruments(&self.inst);
        (self.pend, self.dirty) = (0, false);
        let now = Instant::now();
        self.lost = Some(Lost { since: now, next: now, tries: 0, name, why, told: false });
    }

    /// Tries to open a device that failed again (or the system's default one), and the held playback goes on there.
    fn find(&mut self) {
        let Some(l) = self.lost.as_mut() else { return };
        let now = Instant::now();
        let mut over = self.held.is_none() && !self.listen;
        // Not back soon: the transport shows it paused, so the playback does not jump back in much later.
        if now.duration_since(l.since) >= LOST_PAUSES && self.shared.state.compare_exchange(PLAYING, PAUSED, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            if let Ok(mut v) = self.shared.sounding.lock() {
                v.clear();
            }
        }
        if !over && now >= l.next {
            // The device chosen (an ASIO driver, a Windows device another program took) is tried again before the
            // system's default device (or shared mode) plays instead.
            self.device.wait = l.tries < ASIO_TRIES;
            let opened = self.device.open().map(|o| (o.info.name.clone(), o.info.instead.is_some()));
            self.device.wait = false;
            let l = self.lost.as_mut().expect("lost");
            match opened {
                Ok((name, instead)) => {
                    // Another device (the system's default, `audio_cfg::output`): say so (in place of an ASIO driver,
                    // its note says it).
                    let (old, why) = (l.name.clone(), l.why.clone());
                    if name != old && !instead {
                        self.report(format!("{old} stopped ({why}); playing on {name}"));
                    }
                    if let Some((project, mode, generation, mut at)) = self.held.take().filter(|h| h.2 == self.shared.generation.load(Ordering::Acquire)) {
                        at.paused = self.shared.state.load(Ordering::Acquire) != PLAYING;
                        if let Err(e) = self.start(project, mode, generation, at) {
                            self.report(e);
                            self.shared.state.store(STOPPED, Ordering::Release);
                            self.shared.rewind(0.0);
                        }
                    }
                    self.listen_retry = true;
                    over = true;
                }
                Err(e) => {
                    // Told once the device chosen gives way to another (`OutputDevice::wait`) and that fails too; at
                    // once when there is none to give way to.
                    let cfg = crate::audio_cfg::AudioCfg::current();
                    let waits = cfg.asio_driver().is_some() || cfg.exclusive() || cfg.device.is_some();
                    if !l.told && !(waits && l.tries < ASIO_TRIES) {
                        l.told = true;
                        let msg = format!("{} stopped ({}), and no output device opens: {e}", l.name, l.why);
                        l.tries += 1;
                        l.next = now + RETRY[l.tries.min(RETRY.len() - 1)];
                        self.report(msg);
                    } else {
                        l.tries += 1;
                        l.next = now + RETRY[l.tries.min(RETRY.len() - 1)];
                    }
                }
            }
        }
        if over {
            self.lost = None;
        }
    }

    /// OS_DEBUG: how far playback renders ahead of the device.
    fn log(&self) {
        let shared = &self.shared;
        if let Some(a) = shared.ahead.take() {
            let rate = f64::from(shared.sample_rate.load(Ordering::Relaxed).max(1));
            let latency = shared.opened.lock().ok().and_then(|o| o.as_ref().and_then(|i| i.latency)).map_or(String::new(), |l| format!("; the device adds {l}"));
            eprintln!(
                "[transport] ahead of the device: {}..{} frames, {:.0} on average ({:.1} ms), {} blocks of {} (at most {:.1} ms apart){latency}; {} dropouts",
                a.min,
                a.max,
                a.mean,
                a.mean * 1000.0 / rate,
                a.blocks,
                shared.block.load(Ordering::Relaxed),
                a.gap as f64 / 1000.0,
                shared.underruns.load(Ordering::Relaxed)
            );
        }
    }
}

/// The device opened: the UI shows it (`Transport::opened`, `sample_rate`).
fn publish(shared: &Shared, info: &DeviceInfo) {
    shared.sample_rate.store(info.rate, Ordering::Relaxed);
    if let Ok(mut o) = shared.opened.lock() {
        *o = Some(info.clone());
    }
}

/// A device that failed while in use (`audio_thread`): when, the next try at opening it again (or the system's default
/// device, `audio_cfg::output`) and how many failed, its name and what it said, and whether no device opening was told.
struct Lost {
    since: Instant,
    next: Instant,
    tries: usize,
    name: String,
    why: String,
    told: bool,
}

/// Waits between tries at opening a device that failed: at once, three times half a second apart (an ASIO driver
/// then gives way to the system's default device, `ASIO_TRIES`), then longer, then every 5 s (spec: audio/devices.md,
/// "Driver reset, lost device, fallback").
const RETRY: [Duration; 7] = [
    Duration::ZERO,
    Duration::from_millis(500),
    Duration::from_millis(500),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
];
/// An ASIO driver that does not open is tried again this many times, `ASIO_AGAIN` apart, before the system's default
/// device plays instead (as is a device chosen that failed while in use, at `RETRY`'s times).
const ASIO_TRIES: usize = 3;
const ASIO_AGAIN: Duration = Duration::from_millis(500);
/// A playback held this long for a device that failed shows as paused.
const LOST_PAUSES: Duration = Duration::from_secs(2);

/// Where a new stream starts: the tick to seek to before anything is rendered, whether it starts paused, and whether
/// the notes under way there sound ("Play truncated notes on transport", `Starts`).
#[derive(Clone, Copy, Default)]
struct At {
    tick: f64,
    paused: bool,
    truncated: bool,
    /// Bars of a recording count-in (`Starts::count_in`).
    count_in: u32,
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

/// Controls shared with a stream's feeder and callback.
#[derive(Default)]
struct Ctl {
    /// Paused in the engine (`Engine::pause`, a Marker pause): the stream runs on, ringing out the effects.
    held: AtomicBool,
    /// Publish position, meters and sounding notes (while the playback is the stream's engine and not stopped).
    report: AtomicBool,
    /// The largest output peak since the audio thread last took it (f32 bits): how a stopped playback's tails ring.
    level: AtomicU32,
    /// The engine handed in last (`Slot::epoch`): the callback fades out what it plays and goes on with it.
    want: AtomicU64,
    /// The engine the callback played to its end, + 1 (0: none): everything it rendered has been heard.
    done: AtomicU64,
}

/// A stopped playback ringing out its effects' tails: its channels held as at Pause, its mixer running on.
struct Ringing {
    since: Instant,
    /// Since when its output has been quiet.
    quiet: Option<Instant>,
    /// The sampler voices it held have gone (`Engine::forget_voices`).
    forgot: bool,
}

impl Ringing {
    fn new(since: Instant) -> Ringing {
        Ringing { since, quiet: None, forgot: false }
    }

    /// Its output's peak since the last call, at `now`: true once the tails are over (quiet for `RING_QUIET`, or
    /// rung for `RING_MAX`).
    fn over(&mut self, level: f32, now: Instant) -> bool {
        if level >= RING_QUIET_LEVEL {
            self.quiet = None;
        } else {
            self.quiet.get_or_insert(now);
        }
        self.quiet.is_some_and(|q| now - q >= RING_QUIET) || now - self.since >= RING_MAX
    }
}

/// Takes a stream off the device when it goes: the device's callback stops playing it at its next block.
struct Detach(Arc<AtomicBool>);

impl Drop for Detach {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// The engine a stream's feeder renders, and what the audio thread hands it with the next one.
struct Slot {
    engine: Option<Engine>,
    /// Counts the engines handed in (`Stream::swap`, `Ctl::want`).
    epoch: u64,
    /// The engine handed in last comes in from silence (a playback started in the middle, or resumed after the live
    /// engine played).
    fade_in: bool,
    /// Resets of the hosted plugins (`RESET_INST`, `RESET_FX`) the feeder makes before that engine's first tick.
    reset: u8,
}

/// A stream on the output device, open as long as the device is: its feeder thread renders whichever engine is in the
/// slot into the queue its callback plays from (spec: audio/implementing.md "Playing live"). Play, Stop, Pause, live
/// notes and editors hand it another engine or change the one it renders; none of them opens a stream, a thread or the
/// device (each new thread asked for real-time priority, which rtkit refuses after some 25 in 20 s), and the hosted
/// plugins are only ever processed by this one feeder.
struct Stream {
    _detach: Detach,
    /// Renders ahead of the stream; stopped after the stream leaves the device (fields drop in order).
    feed: FeedThread,
    slot: Arc<Mutex<Slot>>,
    ctl: Arc<Ctl>,
    rate: u32,
    pairs: usize,
    /// `device::opens` when it started: a device opened anew since does not play it.
    opens: u64,
}

impl Stream {
    /// Starts a stream on the output device (opened with the audio settings if it is closed), playing nothing yet.
    /// The previews' (`preview`) feeds no meters and no monitor tap.
    fn open(device: &mut OutputDevice, shared: &Arc<Shared>, inst: &Instruments, preview: bool) -> Result<Stream, String> {
        let info = device.open()?.info.clone();
        let rate = info.rate;
        let queue = Arc::new(Queue::new(2 * info.pairs()));
        // Nothing to play until an engine is handed in: the device's first blocks are silence, not dropouts.
        queue.ended.store(true, Ordering::Relaxed);
        let ctl = Arc::new(Ctl::default());
        let slot = Arc::new(Mutex::new(Slot { engine: None, epoch: 0, fade_in: false, reset: 0 }));
        let mut feeder = Feeder::new(slot.clone(), queue.clone(), shared.clone(), ctl.clone(), rate, inst.clone());
        feeder.meters = !preview;
        let feed = FeedThread::spawn(feeder);
        let mut r = Renderer::new(queue, ctl.clone(), shared.clone(), rate);
        r.tap = !preview;
        r.wake = feed.thread();
        r.gain = 0.0;
        let detach = Detach(r.detach.clone());
        device.add(Box::new(r))?;
        if !preview {
            publish(shared, &info);
        }
        Ok(Stream { _detach: detach, feed, slot, ctl, rate, pairs: info.pairs(), opens: crate::device::opens() })
    }

    /// Hands the feeder `engine` (`None`: nothing, the stream goes silent) and returns the one it rendered. The callback
    /// fades out what that one left queued, then plays the new one (from silence with `fade_in`); the feeder resets the
    /// hosted plugins `reset` names before its first tick. `report`: it is the playback, which publishes its position.
    fn swap(&self, engine: Option<Engine>, fade_in: bool, reset: u8, report: bool) -> Option<Engine> {
        let old = self.slot.lock().ok().map(|mut s| {
            s.epoch += 1;
            (s.fade_in, s.reset) = (fade_in, s.reset | reset);
            self.ctl.report.store(report, Ordering::Relaxed);
            self.ctl.want.store(s.epoch, Ordering::Release);
            std::mem::replace(&mut s.engine, engine)
        });
        self.feed.wake();
        old.flatten()
    }

    /// `swap` to nothing, at once: once it returns, the feeder processes no hosted plugin and resets none (an export).
    fn quiet(&self) -> Option<Engine> {
        let old = self.slot.lock().ok().and_then(|mut s| {
            s.epoch += 1;
            s.reset = 0;
            self.ctl.report.store(false, Ordering::Relaxed);
            self.ctl.want.store(s.epoch, Ordering::Release);
            s.engine.take()
        });
        self.feed.wake();
        old
    }

    /// `f` on the engine the feeder renders (none: `None`); the feeder renders again at once.
    fn with<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
        let r = self.slot.lock().ok().and_then(|mut s| s.engine.as_mut().map(f));
        self.feed.wake();
        r
    }

    /// The engine handed in last has played out, and the callback has played everything it rendered.
    fn played_out(&self) -> bool {
        let epoch = self.slot.lock().map_or(0, |s| s.epoch);
        self.ctl.done.load(Ordering::Acquire) == epoch + 1
    }
}

/// What one output stream does per block: copy the queued ticks, fade out what the engine before left queued when
/// another is handed in, publish the position. (Separate from the device so it can be tested without one.)
struct Renderer {
    queue: Arc<Queue>,
    /// The feeder thread, woken after each block; tests fill the queue in the callback instead (`inline`).
    wake: Option<std::thread::Thread>,
    inline: Option<Feeder>,
    ctl: Arc<Ctl>,
    shared: Arc<Shared>,
    /// The engine it plays (`Slot::epoch`).
    epoch: u64,
    /// Output gain, ramped toward 1 (playing) or 0 (another engine was handed in) by `step` per frame.
    gain: f32,
    step: f32,
    /// Feed the monitor tap with what this stream outputs.
    tap: bool,
    /// Its stream let go of it (`Detach`): the device no longer plays it.
    detach: Arc<AtomicBool>,
    /// When the device last asked for a block (monitored streams, for `Ahead::gap`).
    asked: Option<Instant>,
    /// A block in the queue's width, for a device of another (opened anew with other pairs while it plays).
    other: Vec<f32>,
    /// The peak of the last block it handed the device: a gap after a silent one is not heard.
    peak: f32,
}

impl Renderer {
    fn new(queue: Arc<Queue>, ctl: Arc<Ctl>, shared: Arc<Shared>, rate: u32) -> Renderer {
        let step = (1.0 / (f64::from(rate.max(1)) * FADE_SECONDS)) as f32;
        let other = Vec::with_capacity(queue.w * 2 * 8192);
        Renderer { queue, wake: None, inline: None, ctl, shared, epoch: 0, gain: 1.0, step, tap: false, detach: Arc::new(AtomicBool::new(false)), asked: None, other, peak: 0.0 }
    }

    /// Renders one block (see `render_block`) and copies it to the monitor tap, unless it played nothing.
    fn render(&mut self, stereo: &mut [f32]) {
        let played = self.render_block(stereo);
        if self.tap && played {
            if let Ok(mut t) = self.shared.tap.try_lock() {
                t.push(stereo, self.queue.w, crate::device::blocks(), Instant::now(), self.shared.sample_rate.load(Ordering::Relaxed));
            }
        }
    }

    /// Goes on to engine `want` once the feeder has a block of it queued (or it has nothing to play): what the engine
    /// before left queued is skipped.
    fn take_epoch(&mut self, want: u64, samples: usize) -> bool {
        let Some(at) = self.queue.start_of(want) else { return false };
        let r = self.queue.read.load(Ordering::Relaxed).max(at);
        let ready = self.queue.written.load(Ordering::Acquire).saturating_sub(r) >= samples as u64 || self.queue.ended.load(Ordering::Acquire);
        if !ready {
            return false;
        }
        self.queue.read.store(r, Ordering::Release);
        self.queue.playing.store(want, Ordering::Release);
        self.epoch = want;
        self.gain = if self.queue.fade_in.load(Ordering::Relaxed) { 0.0 } else { 1.0 };
        true
    }

    /// Plays `stereo.len() / w` frames (interleaved, the queue's `w` channels) from the queue; false when it played
    /// nothing. Once another engine is handed in, what is left of the one before fades out over `FADE_SECONDS` and
    /// the new one follows (from silence when it asked to): the time between is silence, not a dropout.
    fn render_block(&mut self, stereo: &mut [f32]) -> bool {
        let w = self.queue.w;
        // First: the feeder renders a block ahead of the device's size, which the next engine waits for.
        self.queue.block.fetch_max(stereo.len() / w, Ordering::Relaxed);
        if let Some(f) = &mut self.inline {
            f.fill(0);
        }
        let want = self.ctl.want.load(Ordering::Acquire);
        if want != self.epoch && self.gain == 0.0 && !self.take_epoch(want, stereo.len()) {
            stereo.fill(0.0);
            self.peak = 0.0;
            if let Some(t) = &self.wake {
                t.unpark();
            }
            return false;
        }
        let switching = want != self.epoch;
        let target = if switching { 0.0 } else { 1.0 };
        if self.tap {
            self.shared.block.store((stereo.len() / w) as u32, Ordering::Relaxed);
        }
        let idle = self.queue.queued() == 0 && self.queue.ended.load(Ordering::Acquire);
        if self.tap && !idle {
            let now = Instant::now();
            self.shared.ahead.record(self.queue.queued() / w as u64, self.asked.map(|t| now - t));
            self.asked = Some(now);
        }
        // Where the block starts (the playback's position moves from here to its end while the device plays it).
        let from = self.queue.heard();
        let read = self.queue.read.load(Ordering::Relaxed) / w as u64;
        // Fading out, it stops where the next engine's ticks start.
        let until = if switching { self.queue.start_of(want) } else { None };
        let got = self.queue.pop(stereo, until);
        let ended = self.queue.ended.load(Ordering::Acquire);
        let peak = stereo[..got].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        if got < stereo.len() {
            stereo[got..].fill(0.0);
            if switching {
                self.gain = 0.0;
            } else if !ended {
                self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                self.shared.feeder_late.fetch_add(1, Ordering::Relaxed);
                if peak > SILENT || self.peak > SILENT {
                    self.shared.heard.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.peak = peak;
        if peak > SILENT {
            self.shared.sound_at.store(now_ms(), Ordering::Relaxed);
        }
        if let Some(t) = &self.wake {
            t.unpark();
        }
        if ended && self.queue.queued() == 0 && !switching {
            self.ctl.done.store(self.epoch + 1, Ordering::Release);
        }
        if got == 0 {
            return false;
        }
        // Every stream frees the tick records it has played (`heard`); only playback publishes where it is, and not
        // from what an engine before left (the next publishes from its start).
        let heard = self.queue.heard();
        if self.ctl.report.load(Ordering::Relaxed) && !switching && !self.detach.load(Ordering::Acquire) {
            if let Some((frame, tick)) = heard {
                self.shared.position.store(frame, Ordering::Relaxed);
                self.shared.tick.store(tick.to_bits(), Ordering::Relaxed);
                if let Ok(mut p) = self.shared.pace.try_lock() {
                    let rate = self.shared.sample_rate.load(Ordering::Relaxed);
                    *p = Some(Pace { from: from.unwrap_or((frame, tick)), to: (frame, tick), read: (read, read + (got / w) as u64), frames: (got / w) as u64, at: Instant::now(), rate });
                }
            }
        }
        if self.gain != 1.0 || target != 1.0 {
            for f in stereo.chunks_exact_mut(w) {
                self.gain = if self.gain < target { (self.gain + self.step).min(target) } else { (self.gain - self.step).max(target) };
                for s in f {
                    *s *= self.gain;
                }
            }
        }
        true
    }
}

/// What the device plays of a stream: its block, and its own peaks (the first pair's) for the meters.
impl Source for Renderer {
    fn play(&mut self, out: &mut [f32], width: usize) {
        let w = self.queue.w;
        if width == w {
            self.render(out);
        } else {
            // The pairs both have; the device's others are silent.
            let mut other = std::mem::take(&mut self.other);
            other.resize(out.len() / width * w, 0.0);
            self.render(&mut other);
            for (o, f) in out.chunks_exact_mut(width).zip(other.chunks_exact(w)) {
                let n = width.min(w);
                o[..n].copy_from_slice(&f[..n]);
                o[n..].fill(0.0);
            }
            self.other = other;
        }
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for s in out.chunks_exact(width) {
            pl = pl.max(s[0].abs());
            pr = pr.max(s[1].abs());
        }
        // Meter peaks: the largest since the monitor last read them (non-negative f32 bits order like u32s).
        self.ctl.level.fetch_max(pl.max(pr).to_bits(), Ordering::Relaxed);
        if self.ctl.report.load(Ordering::Relaxed) {
            self.shared.peak_l.store(pl.to_bits(), Ordering::Relaxed);
            self.shared.peak_r.store(pr.to_bits(), Ordering::Relaxed);
        }
    }

    fn detached(&self) -> bool {
        self.detach.load(Ordering::Acquire)
    }
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
        renderer_with(Some(e), Instruments::default())
    }

    fn renderer_with(e: Option<Engine>, hosts: Instruments) -> (Renderer, Arc<Ctl>, Arc<Shared>) {
        let ctl = Arc::new(Ctl { report: AtomicBool::new(true), ..Ctl::default() });
        let shared = Arc::new(Shared::default());
        let queue = Arc::new(Queue::new(2));
        let slot = Arc::new(Mutex::new(Slot { engine: e, epoch: 0, fade_in: false, reset: 0 }));
        let mut r = Renderer::new(queue.clone(), ctl.clone(), shared.clone(), 48000);
        r.inline = Some(Feeder::new(slot, queue, shared.clone(), ctl.clone(), 48000, hosts));
        (r, ctl, shared)
    }

    /// `Stream::swap` on a test renderer's slot.
    fn hand(r: &Renderer, e: Option<Engine>, fade_in: bool, reset: u8) {
        let slot = r.inline.as_ref().unwrap().slot.clone();
        let mut s = slot.lock().unwrap();
        s.epoch += 1;
        (s.fade_in, s.reset, s.engine) = (fade_in, s.reset | reset, e);
        r.ctl.want.store(s.epoch, Ordering::Release);
    }

    /// The engine in a test renderer's slot.
    fn with_engine<R>(r: &Renderer, f: impl FnOnce(&mut Engine) -> R) -> R {
        f(r.inline.as_ref().unwrap().slot.lock().unwrap().engine.as_mut().unwrap())
    }

    /// Device tests: waits up to 3 s for `ready`, as a device opening for the first time may take a while. Wait on
    /// something only a running device does (the position, peaks, the tap): a stream's first block is rendered before
    /// its device opens, so voices and sounding notes show up early.
    fn wait_for(mut ready: impl FnMut() -> bool) {
        let start = Instant::now();
        while !ready() && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Tests that play on the simulated device (`device::sim`) take turns: there is one for the program.
    static SIM: Mutex<()> = Mutex::new(());

    /// The simulated device while it lives.
    struct Sim(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl Sim {
        fn new() -> Sim {
            let g = SIM.lock().unwrap_or_else(|e| e.into_inner());
            crate::device::sim::set(Some(crate::device::sim::Config { block: 256, rate: 48000 }));
            Sim(g)
        }
    }

    impl Drop for Sim {
        fn drop(&mut self) {
            crate::device::sim::set(None);
        }
    }

    fn sleep(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    /// Play, Stop, a live note, an editor opened, Pause and Resume with it open, a preview, Stop again: all on one
    /// stream that stays on the device, which opens once (on the simulated device: no sound card needed).
    #[test]
    fn the_transport_keeps_one_stream_on_one_device() {
        let _sim = Sim::new();
        let (_, project) = looping_engine();
        let t = Transport::new(None);
        t.update(&project);
        let opens = crate::device::opens();
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.2);
        assert!(t.playing() && t.seconds() > 0.2, "playing: {}", t.seconds());
        assert!(t.sounding().len() <= 8);
        t.stop();
        sleep(100);
        assert!(!t.playing() && t.tick() == 0.0 && t.seconds() == 0.0, "stopped and rewound");
        t.note_on(0, 60, 100);
        sleep(100);
        t.note_off(0, 60);
        t.listen(true);
        sleep(50);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.2);
        assert!(t.playing() && t.seconds() > 0.2, "playing again: {}", t.seconds());
        t.pause();
        sleep(100);
        let held = t.tick();
        sleep(200);
        assert!(t.paused() && t.tick() == held, "paused at {held}: {}", t.tick());
        t.resume();
        sleep(300);
        assert!(t.playing() && !t.paused() && t.tick() != held, "resumed from {held}");
        t.preview(project.clone());
        sleep(50);
        t.stop();
        t.listen(false);
        sleep(100);
        assert_eq!(t.take_error(), None);
        assert_eq!(crate::device::opens(), opens + 1, "the device opened once and stayed open");
        assert_eq!(t.streams(), 2, "the stream that plays and the previews'");
    }

    /// Stopped with nothing ringing, the device stays open and silent (spec: audio/devices.md §2.3), and the next
    /// Play starts on it at once.
    #[test]
    fn the_device_stays_open_when_nothing_plays() {
        let _sim = Sim::new();
        let (_, project) = looping_engine();
        let t = Transport::new(None);
        let opens = crate::device::opens();
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.1);
        t.stop();
        sleep(300);
        assert_eq!((t.streams(), t.engine_voices()), (1, 0), "open, nothing sounding");
        assert_eq!(t.peaks(), (0.0, 0.0));
        let t0 = Instant::now();
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.0);
        assert!(t0.elapsed() < Duration::from_millis(200), "plays at once: {:?}", t0.elapsed());
        t.stop();
        sleep(50);
        assert_eq!(crate::device::opens(), opens + 1);
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
        // The queue keeps the largest block, the margin and less than a tick, not more.
        let queued = r.queue.queued() / 2;
        assert!(queued < 2048 + margin(2048, 48000) + 300, "{queued} frames queued");
    }

    /// One more block, but 3 to 6 ms (spec: audio/devices.md "Render-ahead").
    #[test]
    fn the_margin_is_a_block_from_3_to_6_ms() {
        assert_eq!([64, 128, 256, 512, 2048].map(|b| margin(b, 44100)), [132, 132, 256, 264, 264]);
        assert_eq!([64, 128, 256, 512].map(|b| margin(b, 96000)), [288, 288, 288, 512]);
    }

    /// When the device asks for a block, the queue holds that block and the margin, and less than one tick more
    /// (ticks are whole): never less (no dropout while the feeder keeps up), never a few ticks more (live edits and
    /// notes wait for it).
    #[test]
    fn the_device_finds_a_block_and_the_margin_queued() {
        // 130 BPM at PPQ 96 and 48 kHz: ticks of 230 or 231 frames.
        let tick = 231;
        for n in [64usize, 128, 256, 1024] {
            let (e, _) = looping_engine();
            let (mut r, _, shared) = renderer(e);
            r.tap = true;
            let mut block = vec![0.0f32; 2 * n];
            for _ in 0..(48000 / n) {
                r.render(&mut block);
            }
            let a = shared.ahead.take().expect("blocks recorded");
            let least = n as u64 + margin(n as u64, 48000);
            assert!(a.min >= least && a.max < least + tick, "blocks of {n}: {}..{} frames, want {least}..{}", a.min, a.max, least + tick);
            assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
        }
    }

    /// Playback publishes the tempo it plays at, so the toolbar follows a tempo automation clip (spec: audio/timing.md
    /// "Tempo that changes").
    #[test]
    fn playback_publishes_the_automated_tempo() {
        use os_model::{AutoPoint, Automation, Channel, ChannelKind, Clip, ClipSource, target};
        let mut project = crate::app::test_project();
        project.tempo = Some(130.0);
        let index = project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
        project.channels.push(Channel { index, kind: ChannelKind::Automation, pan: -1.0, volume: 1.0, ..project.channels[0].clone() });
        // 0.25: (10000 + Round(0.25 * 512000)) / 1000 = 138 BPM.
        project.automations.push(Automation { channel: index, targets: vec![target::TEMPO], points: vec![AutoPoint::new(0.0, 0.25), AutoPoint::new(16.0, 0.25)], header: vec![0; 21], tail: vec![], lfo: Default::default() });
        project.arrangements = vec![os_model::Arrangement { clips: vec![Clip::new(0, 384 * 4, 0, ClipSource::Channel(index))], ..Default::default() }];
        let e = Engine::new(&project, &mut SampleLibrary::default(), 48000, PlayMode::Song);
        let (mut r, _, shared) = renderer(e);
        r.render(&mut vec![0.0f32; 2 * 4800]);
        assert_eq!(f64::from_bits(shared.tempo.load(Ordering::Relaxed)), 138.0);
        shared.rewind(0.0);
        assert_eq!(shared.tempo.load(Ordering::Relaxed), 0, "none once rewound");
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

    /// A block the queue could not fill counts as a dropout every time; as one heard only when sound was playing: the
    /// block before it, or what it did get, not silent.
    #[test]
    fn only_gaps_in_sound_are_heard() {
        let ctl = Arc::new(Ctl { report: AtomicBool::new(true), ..Ctl::default() });
        let shared = Arc::new(Shared::default());
        let queue = Arc::new(Queue::new(2));
        let mut r = Renderer::new(queue.clone(), ctl, shared.clone(), 48000);
        let mut block = vec![0.0f32; 2 * 256];
        // Over silence: counted, not heard.
        queue.push(&vec![0.0; 2 * 100], 0.0, 0, false, 100);
        r.render_block(&mut block);
        assert_eq!((shared.underruns.load(Ordering::Relaxed), shared.heard.load(Ordering::Relaxed)), (1, 0));
        // Cutting into sound: heard.
        queue.push(&vec![0.5; 2 * 100], 0.0, 0, false, 100);
        r.render_block(&mut block);
        assert_eq!((shared.underruns.load(Ordering::Relaxed), shared.heard.load(Ordering::Relaxed)), (2, 1));
        // Right after sound, even an empty block is heard; after a silent one, not.
        r.render_block(&mut block);
        assert_eq!(shared.heard.load(Ordering::Relaxed), 2);
        r.render_block(&mut block);
        assert_eq!((shared.underruns.load(Ordering::Relaxed), shared.heard.load(Ordering::Relaxed)), (4, 2));
    }

    /// Dropping the transport waits for the audio thread: once it is gone, nothing but the app holds a hosted effect,
    /// even one the thread was processing (here in an export) as the transport went. At exit the app then destroys
    /// every instance itself, while the plugin files are still loaded.
    #[test]
    fn dropping_the_transport_lets_go_of_hosted_plugins() {
        struct Slow(Arc<AtomicBool>);
        impl os_engine::HostedEffect for Slow {
            fn process_fx(&mut self, _rate: u32, _left: &mut [f32], _right: &mut [f32]) {
                if !self.0.swap(true, Ordering::AcqRel) {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        let (_, project) = looping_engine();
        let entered = Arc::new(AtomicBool::new(false));
        let fx: SharedEffect = Arc::new(Mutex::new(Slow(entered.clone())));
        let t = Transport::new(None);
        t.set_effects(vec![(0, 0, fx.clone())]);
        let path = std::env::temp_dir().join(format!("openstudio-drop-test-{}.wav", std::process::id()));
        t.export(&project, PlayMode::Pattern(1), 48000, crate::export::Job::wav(&path));
        let t0 = Instant::now();
        while !entered.load(Ordering::Acquire) {
            assert!(t0.elapsed() < Duration::from_secs(10), "the export processes the effect");
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(t);
        assert_eq!(Arc::strong_count(&fx), 1, "the audio thread let go of the effect");
        let _ = std::fs::remove_file(path);
    }

    /// An export renders one pass of the pattern (not a loop) on the audio thread, with progress, into a WAV file.
    #[test]
    fn export_writes_a_wav_file() {
        let (_, project) = looping_engine();
        let t = Transport::new(None);
        let path = std::env::temp_dir().join(format!("openstudio-export-test-{}.wav", std::process::id()));
        t.export(&project, PlayMode::Pattern(1), 48000, crate::export::Job::wav(&path));
        assert!(matches!(t.export_status(), Export::Running(_) | Export::Done(_)), "running at once");
        let t0 = Instant::now();
        let secs = loop {
            match t.export_status() {
                Export::Done(r) => break r.expect("export").0,
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

    /// Playback started in the middle of the song comes in from silence over the fade, then at full level.
    #[test]
    fn a_stream_that_starts_from_silence_fades_in() {
        let (e, _) = looping_engine();
        let (mut full, _, _) = renderer(e);
        let (e, _) = looping_engine();
        let (mut r, _, _) = renderer(e);
        r.gain = 0.0;
        let (mut a, mut b) = (vec![0.0f32; 512 * 2], vec![0.0f32; 512 * 2]);
        full.render(&mut a);
        r.render(&mut b);
        assert!(b[0].abs() <= a[0].abs() * r.step * 2.0 + 1e-6, "from silence: {} against {}", b[0], a[0]);
        full.render(&mut a);
        r.render(&mut b);
        assert_eq!(r.gain, 1.0, "over within two blocks");
        assert_eq!(a, b, "then as it plays");
    }

    /// Another engine handed to the stream (a Play, live notes after Stop): what the one before left queued fades out,
    /// then the new one plays from its first sample, as it would on a stream of its own; the time between is no
    /// dropout.
    #[test]
    fn a_new_engine_follows_the_fade_of_the_one_before() {
        let (e, _) = looping_engine();
        let (mut r, _, shared) = renderer(e);
        let (fresh, _) = looping_engine();
        let (mut alone, _, _) = renderer(fresh);
        let mut block = vec![0.0f32; 512 * 2];
        for _ in 0..20 {
            r.render(&mut block);
        }
        assert!(block.iter().any(|v| v.abs() > 0.0), "audible");
        let (next, _) = looping_engine();
        hand(&r, Some(next), false, 0);
        let mut got = Vec::new();
        for _ in 0..12 {
            r.render(&mut block);
            got.extend_from_slice(&block);
        }
        let mut want = Vec::new();
        for _ in 0..12 {
            alone.render(&mut block);
            want.extend_from_slice(&block);
        }
        // The fade (6 ms) is over within the first block of 512 frames; the new engine starts on a block's start.
        let n = 512 * 2;
        let at = (1..3).find(|&b| got[b * n..] == want[..got.len() - b * n]).expect("the new engine plays as it would alone");
        assert!(got[..n].iter().zip(&want[..n]).any(|(a, b)| a != b), "what was queued before is faded out first");
        assert!(got[n * (at - 1)..n * at].iter().rev().take(64).all(|v| v.abs() < 0.05), "faded out: {at}");
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0, "no dropout");
        // One that asks to come in from silence fades in.
        let (next, _) = looping_engine();
        hand(&r, Some(next), true, 0);
        for _ in 0..3 {
            r.render(&mut block);
        }
        assert_eq!(r.gain, 1.0, "faded in");
    }

    /// A stream that has just opened (silent, nothing played yet) starts its first engine at a device block longer
    /// than the margin: the feeder renders that block ahead, which the engine's start waits for.
    #[test]
    fn a_new_stream_starts_its_first_engine() {
        let (mut r, _, shared) = renderer_with(None, Instruments::default());
        r.gain = 0.0;
        let mut block = vec![0.0f32; 2048 * 2];
        r.render(&mut block);
        let (e, _) = looping_engine();
        hand(&r, Some(e), false, 0);
        for _ in 0..3 {
            r.render(&mut block);
        }
        assert!(block.iter().any(|v| v.abs() > 0.0), "playing");
        assert!(shared.position.load(Ordering::Relaxed) > 0, "the position moves");
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
    }

    /// Nothing handed in (stopped, the tails rung out): silence, no dropout, nothing for the monitor tap, no voices.
    #[test]
    fn a_stream_with_nothing_to_play_is_silent() {
        let (e, _) = looping_engine();
        let (mut r, _, shared) = renderer(e);
        r.tap = true;
        let mut block = vec![0.0f32; 256 * 2];
        for _ in 0..10 {
            r.render(&mut block);
        }
        hand(&r, None, false, 0);
        for _ in 0..4 {
            r.render(&mut block);
        }
        let tapped = shared.tap.lock().unwrap().total;
        for _ in 0..20 {
            r.render(&mut block);
            assert!(block.iter().all(|v| *v == 0.0), "silent");
        }
        assert_eq!(shared.tap.lock().unwrap().total, tapped, "the monitor shows what played last");
        assert_eq!(shared.underruns.load(Ordering::Relaxed), 0);
        assert_eq!(r.inline.as_ref().unwrap().voices.load(Ordering::Relaxed), 0);
        assert!(r.ctl.done.load(Ordering::Relaxed) > 0, "played out");
    }

    /// The resets an engine asks for are made before its first tick, by the feeder that processes the plugins: never
    /// while another thread does.
    #[test]
    fn the_feeder_resets_the_plugins_before_the_next_engine() {
        let (synth, fx) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let hosts: Instruments = Arc::new(Mutex::new(Hosts {
            inst: vec![(0, Arc::new(Mutex::new(Counted(synth.clone()))) as SharedInstrument)],
            fx: vec![(1, 0, Arc::new(Mutex::new(Counted(fx.clone()))) as SharedEffect)],
        }));
        let counts = || (synth.load(Ordering::Relaxed), fx.load(Ordering::Relaxed));
        let (e, _) = looping_engine();
        let (mut r, _, _) = renderer_with(Some(e), hosts);
        let mut block = vec![0.0f32; 256 * 2];
        r.render(&mut block);
        let (next, _) = looping_engine();
        hand(&r, Some(next), false, RESET_INST);
        assert_eq!(counts(), (0, 0), "not before the feeder takes it");
        r.render(&mut block);
        assert_eq!(counts(), (1, 0), "the instruments (a Play)");
        hand(&r, None, false, RESET_INST | RESET_FX);
        hand(&r, None, false, 0);
        r.render(&mut block);
        assert_eq!(counts(), (2, 1), "both (a Stop), once, also when another engine followed at once");
    }

    /// Pause in the engine (the stream runs on, ringing out the effects): once the channels are held, the published
    /// position stays exactly where they are held, block after block; resume goes on from there.
    #[test]
    fn a_paused_engine_holds_the_published_position() {
        let (e, _) = looping_engine();
        let (mut r, _, shared) = renderer(e);
        let at = |s: &Shared| (s.position.load(Ordering::Relaxed), f64::from_bits(s.tick.load(Ordering::Relaxed)));
        let mut block = vec![0.0f32; 512 * 2];
        for _ in 0..20 {
            r.render(&mut block);
        }
        with_engine(&r, |e| e.pause());
        // What was queued before the pause and the fade play out first (under 2000 frames).
        for _ in 0..8 {
            r.render(&mut block);
        }
        let held = at(&shared);
        for _ in 0..10 {
            r.render(&mut block);
            assert_eq!(at(&shared), held, "position held");
        }
        // Once the held ticks already queued have played, it goes on from there (no jump).
        with_engine(&r, |e| e.resume());
        for _ in 0..4 {
            r.render(&mut block);
        }
        let (frame, tick) = at(&shared);
        assert!(frame > held.0 && frame < held.0 + 4 * 512 && tick > held.1, "continues from {held:?}, got {frame}, {tick}");
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
        wait_for(|| t.seconds() > 0.0 && (!t.sounding().is_empty() || t.peaks().0 > 0.0));
        assert!(t.seconds() > 0.0 && (!t.sounding().is_empty() || t.peaks().0 > 0.0), "playing");
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
        wait_for(|| t.peaks().0 > 0.01);
        assert!(t.peaks().0 > 0.01, "pluck rings: {:?}", t.peaks());
        let track = project.channels[4].mixer_track;
        project.mixer.iter_mut().find(|m| m.index == track).unwrap().volume = Some(0.0);
        t.update(&project);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(t.peaks(), (0.0, 0.0), "fader at zero is silent within 100 ms");
    }

    /// Live notes: into the live engine while stopped, into the playback while playing; note off
    /// releases them (needs an audio device).
    #[test]
    #[ignore]
    fn live_notes_play_and_release() {
        let mut project = crate::app::test_project();
        project.patterns = vec![os_model::Pattern { number: 1, ..Default::default() }];
        let t = Transport::new(None);
        t.update(&project);
        t.note_on(4, 60, 100);
        // The live engine feeds the monitor tap once it plays.
        let mut b = vec![0.0f32; 64];
        wait_for(|| t.tap(&mut b) > 0);
        t.note_off(4, 60);
        assert_eq!(t.take_error(), None, "live engine started");
        assert!(t.tap(&mut b) > 0, "live engine plays");
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.0);
        t.note_on(4, 67, 100);
        std::thread::sleep(Duration::from_millis(100));
        assert!(t.sounding().contains(&(4, 67)), "held while playing: {:?}", t.sounding());
        assert!(t.peaks().0 > 0.01, "heard while playing");
        t.note_off(4, 67);
        std::thread::sleep(Duration::from_millis(100));
        assert!(t.sounding().is_empty() && t.peaks() == (0.0, 0.0), "released: {:?}", t.peaks());
    }

    /// Linux: while a project plays, the feeder and the device callback run at real-time priority (`crate::rt`; needs an
    /// audio device and rtkit or an rtprio limit): `hidden-tests cargo test -p os-app --bin lemonstudio
    /// transport::tests::audio_threads_run_realtime -- --ignored --exact --nocapture`.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn audio_threads_run_realtime() {
        crate::rt::enable_in_tests();
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() >= 0.5);
        std::thread::sleep(Duration::from_millis(300));
        let threads: Vec<(i32, String, bool)> = std::fs::read_dir("/proc/self/task")
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let tid: i32 = e.file_name().to_str()?.parse().ok()?;
                let name = std::fs::read_to_string(e.path().join("comm")).ok()?.trim().to_string();
                Some((tid, name, crate::rt::is_realtime(tid)))
            })
            .collect();
        t.stop();
        let rt: Vec<&str> = threads.iter().filter(|t| t.2).map(|t| t.1.as_str()).collect();
        let (promoted, demoted) = crate::rt::counts();
        println!("real-time threads now: {rt:?}; promoted {promoted}, demoted by the safety cap {demoted}");
        // A device that paces playback keeps both real-time; one that does not (the null device) lets them spin, and the
        // safety cap puts them back to normal priority instead of the kernel killing the program.
        assert!(promoted >= 2, "the feeder and the device callback were promoted: {threads:?}");
        assert!(rt.len() + demoted >= 2, "each is real-time still, or was demoted: {rt:?}");
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
        wait_for(|| t.seconds() >= 0.2);
        assert!(t.seconds() >= 0.2, "playing: {}", t.seconds());
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

    /// The tap sums the streams of one device block and appends the next; what is heard moves through the last block
    /// with the time since the device took it.
    #[test]
    fn the_tap_is_heard_through_the_block() {
        let mut t = Tap::default();
        let t0 = Instant::now();
        t.push(&[0.5; 2 * 4800], 2, 1, t0, 48000);
        assert_eq!((t.total, t.heard(t0)), (4800, 0), "taken, not heard yet");
        assert_eq!(t.heard(t0 + Duration::from_millis(50)), 2400, "half way through 100 ms");
        assert_eq!(t.heard(t0 + Duration::from_secs(1)), 4800, "not past its end");
        // Live notes in the same device block: summed, not after it.
        t.push(&[0.25; 2 * 4800], 2, 1, t0, 48000);
        assert_eq!((t.total, t.l[10], t.r[4799]), (4800, 0.75, 0.75));
        t.push(&[0.0; 2 * 4800], 2, 2, t0 + Duration::from_millis(100), 48000);
        assert_eq!((t.total, t.start, t.heard(t0 + Duration::from_millis(125))), (9600, 4800, 6000));
        // Without a rate (no stream opened yet), everything written is heard.
        let mut u = Tap::default();
        u.push(&[0.0; 8], 2, 0, t0, 0);
        assert_eq!(u.heard(t0), 4);
    }

    /// The playback's position moves through the device's block over its length; a pattern that wrapped is at the end.
    #[test]
    fn the_position_moves_through_the_block() {
        let t0 = Instant::now();
        let p = Pace { from: (1000, 10.0), to: (5800, 14.0), read: (0, 4800), frames: 4800, at: t0, rate: 48000 };
        assert_eq!(p.at(t0), (1000, 10.0));
        assert_eq!(p.at(t0 + Duration::from_millis(25)), (2200, 11.0));
        assert_eq!(p.played(t0 + Duration::from_millis(25)), 1200);
        assert_eq!(p.at(t0 + Duration::from_millis(500)), (5800, 14.0), "held at the end until the next block");
        let wrapped = Pace { to: (5800, 1.0), ..p };
        assert_eq!(wrapped.at(t0 + Duration::from_millis(10)).1, 1.0);
    }

    /// The playback's track peaks are read once the position gets to their tick; with nothing playing, all at once.
    #[test]
    fn track_peaks_wait_to_be_heard() {
        let mut t = TrackPeaks::default();
        t.ticks.push_back((0, vec![[0.5, 0.25]]));
        t.ticks.push_back((1000, vec![[0.75, 0.0], [0.1, 0.1]]));
        assert_eq!(t.take(Some(500)), vec![[0.5, 0.25]], "only the tick heard");
        assert_eq!(t.take(Some(900)), vec![[0.0, 0.0]], "nothing new heard");
        assert_eq!(t.take(Some(1000)), vec![[0.75, 0.0], [0.1, 0.1]]);
        t.ticks.push_back((2000, vec![[0.3, 0.3]]));
        merge(&mut t.now, &[[0.0, 0.9]]);
        assert_eq!(t.take(None), vec![[0.3, 0.9], [0.0, 0.0]], "stopped: everything, live notes' too");
        assert_eq!(t.spare.len(), 3, "their vectors are used again");
    }

    /// Linux shared mode on a sound server that takes long blocks (a tenth of a second and more at a 4096-frame
    /// buffer): the position and the meter move every UI frame, not once a block (issue #160). Needs a sound server.
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore]
    fn long_blocks_play_smoothly() {
        let _restore = DefaultAudio;
        crate::audio_cfg::AudioCfg { buffer: Some(4096), ..Default::default() }.apply();
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: (0..16).map(|i| os_model::Note { length: 12, key: 60, ..os_model::Note::step(ch, i * 24) }).collect(), ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.3);
        let o = t.opened().expect("opened");
        println!("{} at {} Hz, blocks of {}", o.host, o.rate, t.block());
        // Two seconds of 60 Hz UI frames.
        let (mut ticks, mut meters, mut master) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..120 {
            std::thread::sleep(Duration::from_millis(16));
            ticks.push(t.tick());
            meters.push(t.take_peaks().0);
            master.push(t.take_track_peaks().first().map_or(0.0, |p| p[0]));
        }
        t.stop();
        let steps: Vec<f64> = ticks.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > -100.0).collect();
        let still = steps.iter().filter(|d| **d == 0.0).count();
        let silent = meters.iter().filter(|m| **m == 0.0).count();
        let master_silent = master.iter().filter(|m| **m == 0.0).count();
        println!("frames the position stood still: {still} of {}; meter readings at 0: {silent} of {}; master track's: {master_silent}", steps.len(), meters.len());
        assert!(t.block() > 2048, "the server took long blocks: {}", t.block());
        assert!(still <= steps.len() / 10, "the position moves every frame: {ticks:?}");
        assert!(silent <= meters.len() / 10, "the meter reads every frame: {meters:?}");
        assert!(master_silent <= master.len() / 10, "the mixer's meters read every frame: {master:?}");
    }

    /// A device that fails while in use (unplugged, an ASIO driver asking to be reset) opens again: the playback goes on
    /// from where it was heard, saying nothing when it is the same device; paused stays paused (needs an audio device).
    #[test]
    #[ignore]
    fn a_failed_device_opens_again_and_playback_goes_on() {
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: (0..8).map(|i| os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, i * 48) }).collect(), ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        // A device opening for the first time may take a while.
        let start = Instant::now();
        while t.seconds() < 0.2 && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let before = t.seconds();
        assert!(before >= 0.2, "playing: {before}");
        t.fail_device();
        std::thread::sleep(Duration::from_millis(400));
        assert!(t.playing() && !t.paused(), "still playing");
        assert!(t.seconds() > before + 0.2, "goes on from {before}: {}", t.seconds());
        assert_eq!(t.take_error(), None, "the same device opened again: nothing to say");
        assert_eq!(t.streams(), 1, "the playback only");
        // Paused, it fails and comes back paused, at the same place.
        t.pause();
        std::thread::sleep(Duration::from_millis(150));
        let at = t.tick();
        t.fail_device();
        std::thread::sleep(Duration::from_millis(300));
        assert!(t.paused(), "paused stays paused");
        assert!((t.tick() - at).abs() < 1.0, "held at {at}: {}", t.tick());
        t.resume();
        std::thread::sleep(Duration::from_millis(300));
        assert!(t.tick() > at || t.tick() < at - 1.0, "resumed from {at}: {}", t.tick());
        t.stop();
    }

    /// An ASIO driver plays the playback at its own buffer length, and stays open once nothing plays (needs a driver:
    /// `LEMON_TEST_ASIO` names it, else FL Studio ASIO or the first one installed).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn an_asio_driver_plays() {
        use crate::audio_cfg::{AudioCfg, Set};
        let drivers = crate::device::asio_drivers();
        let pick = std::env::var("LEMON_TEST_ASIO").ok().or_else(|| drivers.iter().find(|d| d.as_str() == "FL Studio ASIO").or(drivers.first()).cloned());
        let Some(name) = pick else { return };
        let _restore = DefaultAudio;
        AudioCfg::default().with(Set::Asio(name.clone())).apply();
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: (0..8).map(|i| os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, i * 48) }).collect(), ..Default::default() }];
        let t = Transport::new(None);
        // It opens before anything plays.
        let start = Instant::now();
        while t.opened().is_none() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(t.opened().is_some_and(|o| o.host == crate::device::ASIO), "opened at once: {:?} {:?}", t.opened(), t.take_error());
        t.play(&project, PlayMode::Pattern(1), 0.0);
        let start = Instant::now();
        while t.seconds() < 0.3 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let error = t.take_error();
        let o = t.opened().expect("opened");
        assert_eq!(error, None);
        assert!(t.seconds() >= 0.3, "playing: {}", t.seconds());
        assert_eq!((o.host, o.name.as_str()), (crate::device::ASIO, name.as_str()));
        assert!(o.fixed.is_some_and(|b| b > 0) && o.rate > 0, "{o:?}");
        assert_eq!(t.block(), o.fixed.unwrap(), "blocks of the driver's buffer length");
        // The driver asks to be reset (a buffer length changed in its panel): it opens anew and the playback goes on.
        let (opens, before) = (crate::device::opens(), t.seconds());
        t.fail_device();
        let reset = Instant::now();
        while t.seconds() < before + 0.2 && reset.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(crate::device::opens() > opens, "opened anew");
        assert!(t.playing() && t.seconds() > before + 0.2, "goes on from {before}: {}", t.seconds());
        assert_eq!(t.take_error(), None, "the same driver: nothing to say");
        // Another buffer length it takes, chosen on the Audio page: it opens at that length, and then at its own again.
        let o = t.opened().expect("opened");
        eprintln!("{name}: {} Hz, {:?} frames; takes {:?} Hz and {:?} frames", o.rate, o.fixed, o.rates, o.sizes);
        assert!(o.rates.contains(&o.rate) || !crate::audio_cfg::RATES.contains(&o.rate), "its rate is among those it takes: {o:?}");
        for b in [o.sizes.iter().copied().find(|&b| Some(b) != o.fixed), None] {
            if o.sizes.is_empty() {
                break;
            }
            AudioCfg::current().with(Set::Buffer(b)).apply();
            t.reopen();
            let (want, start) = (b.or(o.fixed), Instant::now());
            while t.opened().and_then(|o| o.fixed) != want && start.elapsed() < Duration::from_secs(3) {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(t.opened().and_then(|o| o.fixed), want, "at {b:?}");
        }
        // Its panel opens on the audio thread (a window shows: only when asked for, and kept that many seconds).
        if let Ok(s) = std::env::var("LEMON_TEST_ASIO_PANEL") {
            t.asio_panel();
            std::thread::sleep(Duration::from_millis(1500));
            assert_eq!(t.take_error(), None, "the panel opened");
            eprintln!("{name}'s panel is open");
            std::thread::sleep(Duration::from_secs_f64(s.parse().unwrap_or(0.0)));
        }
        t.stop();
    }

    /// Puts the default audio settings back when dropped (a test that chose others ends, or fails).
    struct DefaultAudio;

    impl Drop for DefaultAudio {
        fn drop(&mut self) {
            crate::audio_cfg::AudioCfg::default().apply();
        }
    }

    /// An ASIO driver that does not open gives way to the system's default device, saying so (needs an audio device).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn a_missing_asio_driver_plays_on_the_default_device() {
        use crate::audio_cfg::{AudioCfg, Set};
        let _restore = DefaultAudio;
        AudioCfg::default().with(Set::Asio("No Such ASIO Driver".into())).apply();
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 96, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        let start = Instant::now();
        let mut error = None;
        while error.is_none() && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
            error = t.take_error();
        }
        let o = t.opened();
        t.stop();
        let error = error.expect("says so");
        assert!(error.starts_with("Couldn't open No Such ASIO Driver: not installed; using "), "{error}");
        let o = o.expect("opened");
        assert!(o.host != crate::device::ASIO, "a Windows device");
        assert_eq!(o.instead.as_ref(), Some(&error), "the status says so too");
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
        // Every position published until it has played on a little (the device may take a while to open).
        let t0 = Instant::now();
        let mut seen = vec![t.tick()];
        while seen.last().is_some_and(|&k| k <= start + 10.0) && t0.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(1));
            seen.push(t.tick());
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
        assert_eq!(t.opened().map(|o| o.fixed), Some(Some(2048)));
        // Paused: the new stream holds the same position, silent.
        t.pause();
        std::thread::sleep(Duration::from_millis(100));
        let held = t.tick();
        crate::audio_cfg::AudioCfg::default().apply();
        t.reopen();
        std::thread::sleep(Duration::from_millis(300));
        assert!(t.paused() && (t.tick() - held).abs() < 1.0, "held at {held}: {}", t.tick());
        assert_eq!(t.peaks(), (0.0, 0.0), "silent");
        assert_eq!(t.opened().map(|o| o.fixed), Some(None));
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
        wait_for(|| t.seconds() > 0.0);
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
        let mut b = vec![0.0f32; 2048];
        wait_for(|| t.tap(&mut b) > 0 && b.iter().any(|v| v.abs() > 0.01));
        let n1 = t.tap(&mut b);
        assert!(n1 > 0 && b.iter().any(|v| v.abs() > 0.01), "tap has audio");
        assert!(t.sample_rate() > 0);
        t.stop();
        std::thread::sleep(Duration::from_millis(150));
        let n2 = t.tap(&mut b);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(t.tap(&mut b), n2, "no more samples after stop");
    }

    /// A hosted instrument with a release (and no effect behind it): Stop lets the note go and the release ring out, as
    /// in FL (spec: audio/transport.md "Tails after Pause and Stop"); a Play while it rings carries it on; it is reset
    /// once quiet, and nothing comes back with the next Play (on the simulated device).
    #[test]
    fn stop_lets_an_instruments_release_ring() {
        let _sim = Sim::new();
        use crate::prefs::{set_check, RESET_PLUGINS_ON_TRANSPORT};
        use os_engine::Instrument;
        struct Releaser {
            level: f32,
            on: bool,
            calls: Arc<AtomicU32>,
            resets: Arc<AtomicU32>,
        }
        impl Instrument for Releaser {
            fn note_on(&mut self, _o: u32, _k: u8, _v: f32) {
                (self.level, self.on) = (0.3, true);
            }
            fn note_off(&mut self, _o: u32, _k: u8) {
                self.on = false;
            }
            fn process(&mut self, _r: u32, l: &mut [f32], r: &mut [f32]) {
                self.calls.fetch_add(1, Ordering::Relaxed);
                // Once let go of, it falls by half every 50 ms or so.
                if !self.on {
                    self.level *= 0.5f32.powf(l.len() as f32 / 2400.0);
                }
                l.fill(self.level);
                r.fill(self.level);
            }
            fn active(&self) -> bool {
                self.level > 1e-6
            }
            fn reset(&mut self) {
                (self.level, self.on) = (0.0, false);
                self.resets.fetch_add(1, Ordering::Relaxed);
            }
        }
        set_check(&RESET_PLUGINS_ON_TRANSPORT, false);
        let mut project = crate::app::test_project();
        let ch = project.channels[0].index;
        project.channels[0].kind = os_model::ChannelKind::Plugin;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 96 * 8, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let (calls, resets) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let inst = Arc::new(Mutex::new(Releaser { level: 0.0, on: false, calls: calls.clone(), resets: resets.clone() }));
        let t = Transport::new(None);
        t.set_instruments(vec![(0, inst.clone() as SharedInstrument)]);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.peaks().0 > 0.05);
        assert!(inst.lock().unwrap().on, "the note is held");
        let before = resets.load(Ordering::Relaxed);
        t.stop();
        std::thread::sleep(Duration::from_millis(40));
        let (c0, ringing) = (calls.load(Ordering::Relaxed), inst.lock().unwrap().level);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!inst.lock().unwrap().on, "Stop let the note go");
        assert!(calls.load(Ordering::Relaxed) > c0, "still processed after Stop");
        assert!(ringing > 0.0 && inst.lock().unwrap().level < ringing, "the release falls: {ringing}");
        assert_eq!(resets.load(Ordering::Relaxed), before, "not reset while it rings");
        assert_eq!((t.state(), t.tick()), (State::Stopped, 0.0), "rewound");
        // Play while it rings: not reset (the setting is off), the pattern's note starts it again.
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.0);
        assert_eq!(resets.load(Ordering::Relaxed), before, "Play while it rings");
        t.stop();
        let t0 = Instant::now();
        while resets.load(Ordering::Relaxed) == before && t0.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(resets.load(Ordering::Relaxed), before + 1, "reset once rung out");
        assert!(!inst.lock().unwrap().active(), "no tail left to come back");
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
        wait_for(|| t.seconds() > 0.0 && pl.voices() == 2 && t.engine_voices() > 0);
        assert!(t.seconds() > 0.0, "playing");
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
    /// plugins"); Stop sound, and the end of the tails a Stop let ring, reset both.
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

    /// The tails a Stop lets ring are over once the output has been quiet (below -96 dB) for `RING_QUIET`, a peak
    /// starting the wait again, or after `RING_MAX` whatever they do (spec: audio/transport.md "Stop").
    #[test]
    fn stopped_tails_ring_until_quiet() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut r = Ringing::new(t0);
        assert!(!r.over(0.2, at(10)), "a reverb's tail");
        assert!(!r.over(1e-6, at(20)), "quiet from here");
        assert!(!r.over(1e-6, at(20 + 2900)));
        assert!(!r.over(0.01, at(2950)), "an echo: the wait starts again");
        assert!(!r.over(0.0, at(3000)));
        assert!(!r.over(0.0, at(5900)));
        assert!(r.over(0.0, at(6000)), "quiet for {RING_QUIET:?}");
        let mut r = Ringing::new(t0);
        assert!(!r.over(0.5, at(29_990)));
        assert!(r.over(0.5, at(30_000)), "an endless feedback stops after {RING_MAX:?}");
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
            wait_for(|| t.seconds() > 0.0);
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
            assert_eq!(fx.load(Ordering::Relaxed), resets(3), "Stop lets the tail ring, setting {on}");
            // The effect is silent: it is reset once it has been quiet for `RING_QUIET`.
            let t0 = Instant::now();
            while fx.load(Ordering::Relaxed) == resets(3) && t0.elapsed() < RING_QUIET + Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(fx.load(Ordering::Relaxed), resets(3) + 1, "reset once quiet, setting {on}");
            assert!(t0.elapsed() + Duration::from_millis(250) >= RING_QUIET, "not before");
            // Stop sound, and a second Stop while tails ring, reset at once.
            t.play(&project, PlayMode::Pattern(1), 0.0);
            wait_for(|| t.seconds() > 0.0);
            t.silence();
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(fx.load(Ordering::Relaxed), resets(4) + 2, "Stop sound, setting {on}");
            t.play(&project, PlayMode::Pattern(1), 0.0);
            wait_for(|| t.seconds() > 0.0);
            t.stop();
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(fx.load(Ordering::Relaxed), resets(5) + 2, "ringing, setting {on}");
            t.stop();
            std::thread::sleep(Duration::from_millis(200));
            assert_eq!(fx.load(Ordering::Relaxed), resets(5) + 3, "a second Stop, setting {on}");
        }
    }

    /// A delay-like effect whose tail rings on after its input: Stop lets it ring (it is processed, its output heard
    /// on the monitor), Play with "Reset plugins on transport" off carries it on, and it is reset once quiet (needs an
    /// audio device: `cargo test -p os-app -- --ignored stop_lets`).
    #[test]
    #[ignore]
    fn stop_lets_effect_tails_ring() {
        use crate::prefs::{set_check, RESET_PLUGINS_ON_TRANSPORT};
        struct Tail {
            level: f32,
            calls: Arc<AtomicU32>,
            resets: Arc<AtomicU32>,
        }
        impl os_engine::HostedEffect for Tail {
            fn process_fx(&mut self, _rate: u32, l: &mut [f32], r: &mut [f32]) {
                self.calls.fetch_add(1, Ordering::Relaxed);
                let input = l.iter().chain(r.iter()).fold(0.0f32, |m, s| m.max(s.abs()));
                // Rings at 0.25 while fed, then falls by half every 50 ms or so.
                self.level = if input > 1e-3 { 0.25 } else { self.level * 0.5f32.powf(l.len() as f32 / 2400.0) };
                l.fill(self.level);
                r.fill(self.level);
            }
            fn reset_fx(&mut self) {
                self.level = 0.0;
                self.resets.fetch_add(1, Ordering::Relaxed);
            }
        }
        set_check(&RESET_PLUGINS_ON_TRANSPORT, false);
        let (_, project) = looping_engine();
        let (calls, resets) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let fx = Arc::new(Mutex::new(Tail { level: 0.0, calls: calls.clone(), resets: resets.clone() }));
        let t = Transport::new(None);
        t.set_effects(vec![(0, 0, fx.clone() as SharedEffect)]);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.3 && fx.lock().unwrap().level > 0.2);
        assert!(fx.lock().unwrap().level > 0.2, "fed");
        t.stop();
        std::thread::sleep(Duration::from_millis(40));
        let (c0, ringing) = (calls.load(Ordering::Relaxed), fx.lock().unwrap().level);
        std::thread::sleep(Duration::from_millis(100));
        assert!(calls.load(Ordering::Relaxed) > c0, "still processed after Stop");
        assert!(ringing > 0.0 && fx.lock().unwrap().level < ringing, "the tail falls: {ringing}");
        assert_eq!(resets.load(Ordering::Relaxed), 0, "not reset while it rings");
        assert_eq!((t.state(), t.tick()), (State::Stopped, 0.0), "rewound");
        // Play while it rings: the tail carries on (the setting is off), fed again by the pattern.
        t.play(&project, PlayMode::Pattern(1), 0.0);
        wait_for(|| t.seconds() > 0.0);
        assert_eq!(resets.load(Ordering::Relaxed), 0, "Play with the setting off");
        t.stop();
        let t0 = Instant::now();
        while resets.load(Ordering::Relaxed) == 0 && t0.elapsed() < Duration::from_secs(8) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(resets.load(Ordering::Relaxed), 1, "reset once rung out");
        std::thread::sleep(Duration::from_millis(100));
        let c1 = calls.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(calls.load(Ordering::Relaxed), c1, "no longer processed");
        assert_eq!(t.streams(), 1, "the stream stays, silent");
    }
}
