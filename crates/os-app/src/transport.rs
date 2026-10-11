//! Playback: an audio thread that owns the output streams and the engines.
//!
//! One state machine, `State` (stopped / playing / paused), published in `Shared`:
//! - **Play** starts the pattern or the playlist from a start tick (a pattern loops inside the engine, the
//!   playlist plays out and stops by itself).
//! - **Pause** fades the stream out over a few milliseconds and holds the engine where it is (position, ringing
//!   voices, hosted plugins untouched); **Resume** fades back in from that exact place.
//! - **Stop** fades out, drops the streams, silences every hosted instrument (`Instrument::reset`: voices and tails
//!   die, nothing resumes later) and rewinds to the start. It also ends live notes and previews.
//!
//! The UI sets the state at once (so a double click behaves), then sends the matching command; commands are
//! applied in order on the audio thread. `generation` counts play / stop requests: a song that finishes after a
//! newer request must not stop it.
//!
//! `update` swaps in an edited snapshot at the current position (decoded samples stay cached in the library).
//! `preview` plays one note once on its own stream. Hosted generator plugins (`set_instruments`) are attached to
//! the playback and live engines only: an instance must not be processed by two streams, so previews of plugin
//! channels play as timed live notes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
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
    /// Project, mode, start tick (the piano roll's play-start marker in pattern mode), request generation.
    Play(Box<Project>, PlayMode, f64, u64),
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
    Stop,
}

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
                move || audio_thread(rx, s, latest, inst, SampleLibrary::new(factory))
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

    /// The output device's sample rate (what hosted plugins are activated at).
    pub fn device_rate() -> Option<u32> {
        let device = cpal::default_host().default_output_device()?;
        device.default_output_config().ok().map(|c| c.sample_rate())
    }

    /// Start (or restart) playback from `start_tick`; replaces whatever is playing or paused.
    pub fn play(&self, project: &Project, mode: PlayMode, start_tick: f64) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = None;
        }
        let generation = self.shared.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.song.store(matches!(mode, PlayMode::Song), Ordering::Relaxed);
        self.shared.rewind(start_tick);
        self.shared.state.store(PLAYING, Ordering::Release);
        let _ = self.tx.send(Cmd::Play(Box::new(project.clone()), mode, start_tick, generation));
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
}

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

/// Silences every hosted instrument (voices, tails, queued notes). Only called while no stream processes them.
fn reset_instruments(inst: &Instruments) {
    let (list, fx): (Vec<SharedInstrument>, Vec<SharedEffect>) = inst
        .lock()
        .map(|l| (l.inst.iter().map(|(_, i)| i.clone()).collect(), l.fx.iter().map(|(_, _, f)| f.clone()).collect()))
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
    let retire = |s: Session, closing: &mut Vec<(Session, Instant)>| {
        s.ctl.report.store(false, Ordering::Relaxed);
        s.ctl.paused.store(true, Ordering::Relaxed);
        closing.push((s, Instant::now()));
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(Cmd::Update) => {
                if let Some(project) = latest.lock().ok().and_then(|mut l| l.take()) {
                    for s in [session.as_mut(), live.as_mut()].into_iter().flatten() {
                        s.project = project.clone();
                        swap(s, &mut library);
                    }
                    live_project = Some(project);
                }
            }
            Ok(Cmd::Instruments) => {
                for s in [session.as_mut(), live.as_mut()].into_iter().flatten() {
                    swap(s, &mut library);
                }
            }
            Ok(Cmd::NoteOn(ch, key, vel)) => {
                let target = match session.as_ref() {
                    Some(s) if !s.paused() => Some(s),
                    _ => {
                        if live.is_none() {
                            if let Some(p) = &live_project {
                                match Session::start(p, PlayMode::Pattern(1), Kind::Live, 0, &mut library, shared.clone(), &inst) {
                                    Ok(s) => live = Some(s),
                                    Err(e) => report(&shared, e),
                                }
                            }
                        }
                        live.as_ref()
                    }
                };
                if let Some(mut e) = target.and_then(|s| s.engine.lock().ok()) {
                    e.note_on(ch, key, vel);
                }
            }
            Ok(Cmd::NoteOff(ch, key)) => {
                for s in [session.as_ref(), live.as_ref()].into_iter().flatten() {
                    if let Ok(mut e) = s.engine.lock() {
                        e.note_off(ch, key);
                    }
                }
            }
            Ok(Cmd::PreviewFor(project, limit)) => {
                preview = None;
                preview_until = limit.map(|d| Instant::now() + d);
                match Session::start(&project, PlayMode::Pattern(1), Kind::Preview, 0, &mut library, shared.clone(), &inst) {
                    Ok(s) => preview = Some(s),
                    Err(e) => report(&shared, e),
                }
            }
            Ok(Cmd::PreviewSong(project)) => {
                preview = None;
                preview_until = None;
                match Session::start(&project, PlayMode::Song, Kind::Preview, 0, &mut library, shared.clone(), &inst) {
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
                            match Session::start(p, PlayMode::Pattern(1), Kind::Live, 0, &mut library, shared.clone(), &inst) {
                                Ok(s) => live = Some(s),
                                Err(e) => report(&shared, e),
                            }
                        }
                    }
                    let target = match session.as_ref() {
                        Some(s) if !s.paused() => Some(s),
                        _ => live.as_ref(),
                    };
                    if let Some(s) = target {
                        let tempo = project.tempo.unwrap_or(140.0);
                        let frames = os_engine::laws::tick_to_frame(f64::from(n.length.max(1)), project.ppq.max(1), tempo, s.rate);
                        if let Ok(mut e) = s.engine.lock() {
                            e.note_for(r, n.key, n.velocity, frames);
                        }
                    }
                    continue;
                }
                preview = None;
                preview_until = None;
                match Session::start(&project, PlayMode::Pattern(1), Kind::Preview, 0, &mut library, shared.clone(), &inst) {
                    Ok(s) => preview = Some(s),
                    Err(e) => report(&shared, e),
                }
            }
            Ok(Cmd::Play(project, mode, start, generation)) => {
                // A newer request (stop, another play) already superseded this one.
                if generation != shared.generation.load(Ordering::Acquire) {
                    continue;
                }
                // The old playback is replaced, not faded; nothing may keep ringing into the new one. A live stream
                // left open would process the same plugin instances a second time (each stream would only hear
                // every other chunk of a ringing sample): typed notes go into the playing engine instead.
                session = None;
                live = None;
                closing.clear();
                reset_instruments(&inst);
                reset_pending = false;
                live_project = Some((*project).clone());
                match Session::start(&project, mode, Kind::Song, generation, &mut library, shared.clone(), &inst) {
                    Ok(s) => {
                        if start > 0.0 {
                            let tempo = project.tempo.unwrap_or(140.0);
                            let frame = os_engine::laws::tick_to_frame(start, project.ppq.max(1), tempo, s.rate);
                            if let Ok(mut e) = s.engine.lock() {
                                e.seek_tick(start as u64);
                            }
                            shared.position.store(frame, Ordering::Relaxed);
                        }
                        session = Some(s);
                    }
                    Err(e) => {
                        report(&shared, e);
                        let _ = shared.state.compare_exchange(PLAYING, STOPPED, Ordering::AcqRel, Ordering::Acquire);
                        shared.rewind(0.0);
                    }
                }
            }
            Ok(Cmd::Pause) => {
                if let Some(s) = &session {
                    s.ctl.paused.store(true, Ordering::Relaxed);
                }
            }
            Ok(Cmd::Resume) => {
                if let Some(s) = &session {
                    s.ctl.paused.store(false, Ordering::Relaxed);
                }
            }
            Ok(Cmd::Stop) => {
                for s in [session.take(), live.take(), preview.take()].into_iter().flatten() {
                    retire(s, &mut closing);
                }
                reset_pending = true;
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
        // A pattern loops inside the engine; a song ends when it has played out and everything has rung out.
        if session.as_ref().is_some_and(|s| s.done.load(Ordering::Relaxed)) {
            if let Some(s) = session.take() {
                if s.generation == shared.generation.load(Ordering::Acquire) {
                    shared.state.store(STOPPED, Ordering::Release);
                    shared.rewind(0.0);
                }
            }
        }
    }
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
}

impl Session {
    fn paused(&self) -> bool {
        self.ctl.paused.load(Ordering::Relaxed)
    }

    fn start(project: &Project, mode: PlayMode, kind: Kind, generation: u64, library: &mut SampleLibrary, shared: Arc<Shared>, inst: &Instruments) -> Result<Session, String> {
        let song = kind == Kind::Song;
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let rate = config.sample_rate;
        let mut e = Engine::new(project, library, rate, mode);
        // Pattern playback loops on the bar line.
        e.set_looping(song && matches!(mode, PlayMode::Pattern(_)));
        if kind == Kind::Live {
            e.clear_schedule();
        }
        if kind != Kind::Preview {
            attach(&mut e, inst);
        }
        let engine = Arc::new(Mutex::new(e));
        let done = Arc::new(AtomicBool::new(false));
        let ctl = Arc::new(Ctl { paused: AtomicBool::new(false), report: AtomicBool::new(song) });
        if kind != Kind::Preview {
            shared.sample_rate.store(rate, Ordering::Relaxed);
        }
        let mut renderer = Renderer::new(engine.clone(), ctl.clone(), done.clone(), shared, rate);
        renderer.tap = kind != Kind::Preview;
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &config, renderer),
            SampleFormat::I16 => build::<i16>(&device, &config, renderer),
            SampleFormat::I32 => build::<i32>(&device, &config, renderer),
            SampleFormat::U16 => build::<u16>(&device, &config, renderer),
            other => Err(format!("unsupported output sample format {other:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Session { _stream: stream, kind, engine, done, ctl, generation, project: project.clone(), mode, rate })
    }
}

/// What one output stream does per block: render the engine, fade it for pause / stop, publish the position.
/// (Separate from the device so it can be tested without one.)
struct Renderer {
    engine: Arc<Mutex<Engine>>,
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
    fn new(engine: Arc<Mutex<Engine>>, ctl: Arc<Ctl>, done: Arc<AtomicBool>, shared: Arc<Shared>, rate: u32) -> Renderer {
        let step = (1.0 / (f64::from(rate.max(1)) * FADE_SECONDS)) as f32;
        Renderer { engine, ctl, done, shared, gain: 1.0, step, tap: false }
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

    /// Renders `stereo.len() / 2` frames (interleaved L/R). A paused stream, once faded out, does not touch the
    /// engine at all, so its position and voices stay exactly where they were.
    fn render_block(&mut self, stereo: &mut [f32]) {
        let target = if self.ctl.paused.load(Ordering::Relaxed) { 0.0 } else { 1.0 };
        if self.gain == 0.0 && target == 0.0 {
            stereo.fill(0.0);
            return;
        }
        match self.engine.try_lock() {
            Ok(mut e) if !e.finished() => {
                e.process(stereo);
                if self.tap {
                    if let Ok(mut v) = self.shared.tracks.try_lock() {
                        e.take_track_peaks(&mut v);
                    }
                }
                if self.ctl.report.load(Ordering::Relaxed) {
                    self.shared.position.store(e.position(), Ordering::Relaxed);
                    self.shared.tick.store(e.tick_position().to_bits(), Ordering::Relaxed);
                    // Never block the audio thread; a missed update shows next block.
                    if let Ok(mut v) = self.shared.sounding.try_lock() {
                        v.clear();
                        if target > 0.0 {
                            v.extend(e.sounding().map(|(c, k)| (c as u16, k)));
                        }
                    }
                }
            }
            Ok(_) => {
                stereo.fill(0.0);
                self.done.store(true, Ordering::Relaxed);
                return;
            }
            Err(_) => {
                stereo.fill(0.0);
                return;
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

    fn renderer(e: Engine) -> (Renderer, Arc<Ctl>, Arc<Shared>) {
        let ctl = Arc::new(Ctl { paused: AtomicBool::new(false), report: AtomicBool::new(true) });
        let shared = Arc::new(Shared::default());
        let r = Renderer::new(Arc::new(Mutex::new(e)), ctl.clone(), Arc::new(AtomicBool::new(false)), shared.clone(), 48000);
        (r, ctl, shared)
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
}
