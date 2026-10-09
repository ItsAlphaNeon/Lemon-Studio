//! Playback: an audio thread that owns the output stream and the engine.
//!
//! The UI sends a project snapshot on Play; `update` swaps in an edited snapshot at the current
//! position (heard immediately; decoded samples stay cached in the library). Pattern mode loops
//! inside the engine (`Engine::set_looping`). `preview` plays one note once on its own stream.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use os_engine::{Engine, PlayMode, SampleLibrary};
use os_model::Project;

/// State the UI reads every frame.
#[derive(Default)]
pub struct Shared {
    pub playing: AtomicBool,
    /// Playback holds its position silently (Play pressed while playing).
    pub paused: AtomicBool,
    /// Playback position in frames.
    pub position: AtomicU64,
    pub sample_rate: AtomicU32,
    /// Peak of the last block, f32 bits, per side.
    pub peak_l: AtomicU32,
    pub peak_r: AtomicU32,
    pub error: Mutex<Option<String>>,
    /// Notes playing now, as (channel index, key); written by the audio callback.
    pub sounding: Mutex<Vec<(u16, u16)>>,
}

enum Cmd {
    /// Project, mode, start tick (the piano roll's play-start marker in pattern mode).
    Play(Box<Project>, PlayMode, f64),
    /// The newest edited project is in `Transport::latest` (older ones are skipped).
    Update,
    Preview(Box<Project>),
    /// Live note (typing keyboard): channel (rack position), key, velocity; `NoteOff` releases.
    NoteOn(usize, u16, u8),
    NoteOff(usize, u16),
    Stop,
}

pub struct Transport {
    tx: Sender<Cmd>,
    latest: Arc<Mutex<Option<Project>>>,
    pub shared: Arc<Shared>,
}

impl Transport {
    pub fn new(factory: Option<PathBuf>) -> Transport {
        let (tx, rx) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let shared = Arc::new(Shared { sounding: Mutex::new(Vec::with_capacity(256)), ..Shared::default() });
        let s = shared.clone();
        std::thread::Builder::new()
            .name("openstudio-audio".into())
            .spawn({
                let latest = latest.clone();
                move || audio_thread(rx, s, latest, SampleLibrary::new(factory))
            })
            .expect("spawn audio thread");
        Transport { tx, latest, shared }
    }

    pub fn play(&self, project: &Project, mode: PlayMode, start_tick: f64) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = None;
        }
        let _ = self.tx.send(Cmd::Play(Box::new(project.clone()), mode, start_tick));
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

    /// Start a live note: mixed into playback while playing, else on the live stream (kept open, so
    /// only the first note pays for opening the device). The live stream uses the project from the
    /// last `play` / `update`.
    pub fn note_on(&self, channel: usize, key: u16, velocity: u8) {
        let _ = self.tx.send(Cmd::NoteOn(channel, key, velocity));
    }

    pub fn note_off(&self, channel: usize, key: u16) {
        let _ = self.tx.send(Cmd::NoteOff(channel, key));
    }

    /// Hold playback at its position (silent); `resume` continues from there.
    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
        if let Ok(mut v) = self.shared.sounding.lock() {
            v.clear();
        }
    }

    pub fn resume(&self) {
        self.shared.paused.store(false, Ordering::Relaxed);
    }

    pub fn paused(&self) -> bool {
        self.playing() && self.shared.paused.load(Ordering::Relaxed)
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }

    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn seconds(&self) -> f64 {
        let rate = self.shared.sample_rate.load(Ordering::Relaxed).max(1);
        self.shared.position.load(Ordering::Relaxed) as f64 / f64::from(rate)
    }

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

fn audio_thread(rx: Receiver<Cmd>, shared: Arc<Shared>, latest: Arc<Mutex<Option<Project>>>, mut library: SampleLibrary) {
    let mut session: Option<Session> = None;
    // One-shot note previews play on their own stream, so they also sound while playing or paused.
    let mut preview: Option<Session> = None;
    // Live notes while stopped or paused: an engine with no schedule on a stream that stays open.
    let mut live: Option<Session> = None;
    let mut live_project: Option<Project> = None;
    let swap = |s: &mut Session, library: &mut SampleLibrary| {
        // Build outside the lock, then swap at the playing position.
        let mut e = Engine::new(&s.project, library, s.rate, s.mode);
        if s.kind == Kind::Live {
            e.clear_schedule();
        }
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
    loop {
        match rx.recv_timeout(Duration::from_millis(15)) {
            Ok(Cmd::Update) => {
                if let Some(project) = latest.lock().ok().and_then(|mut l| l.take()) {
                    for s in [session.as_mut(), live.as_mut()].into_iter().flatten() {
                        s.project = project.clone();
                        swap(s, &mut library);
                    }
                    live_project = Some(project);
                }
            }
            Ok(Cmd::NoteOn(ch, key, vel)) => {
                let paused = shared.paused.load(Ordering::Relaxed);
                let target = match session.as_ref() {
                    Some(s) if !paused => Some(s),
                    _ => {
                        if live.is_none() {
                            if let Some(p) = &live_project {
                                match Session::start(p, PlayMode::Pattern(1), Kind::Live, &mut library, shared.clone()) {
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
            Ok(Cmd::Preview(project)) => {
                preview = None;
                match Session::start(&project, PlayMode::Pattern(1), Kind::Preview, &mut library, shared.clone()) {
                    Ok(s) => preview = Some(s),
                    Err(e) => report(&shared, e),
                }
            }
            Ok(Cmd::Play(project, mode, start)) => {
                session = None;
                shared.paused.store(false, Ordering::Relaxed);
                live_project = Some((*project).clone());
                match Session::start(&project, mode, Kind::Song, &mut library, shared.clone()) {
                    Ok(s) => {
                        if start > 0.0 {
                            let tempo = project.tempo.unwrap_or(140.0);
                            let frame = os_engine::laws::tick_to_frame(start, project.ppq.max(1), tempo, s.rate);
                            if let Ok(mut e) = s.engine.lock() {
                                e.seek(frame);
                            }
                            shared.position.store(frame, Ordering::Relaxed);
                        }
                        session = Some(s);
                    }
                    Err(e) => report(&shared, e),
                }
            }
            Ok(Cmd::Stop) => {
                session = None;
                shared.paused.store(false, Ordering::Relaxed);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if preview.as_ref().is_some_and(|p| p.done.load(Ordering::Relaxed)) {
            preview = None;
        }
        // Patterns loop inside the engine (sample-accurate); a song ends when it has played out.
        if session.as_ref().is_some_and(|s| s.done.load(Ordering::Relaxed)) {
            session = None;
            shared.paused.store(false, Ordering::Relaxed);
        }
        if session.is_none() {
            shared.playing.store(false, Ordering::Relaxed);
            if let Ok(mut v) = shared.sounding.lock() {
                v.clear();
            }
            shared.peak_l.store(0, Ordering::Relaxed);
            shared.peak_r.store(0, Ordering::Relaxed);
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

struct Session {
    _stream: cpal::Stream,
    kind: Kind,
    engine: Arc<Mutex<Engine>>,
    done: Arc<AtomicBool>,
    /// The project playing (live edits replace it).
    project: Project,
    mode: PlayMode,
    rate: u32,
}

impl Session {
    fn start(project: &Project, mode: PlayMode, kind: Kind, library: &mut SampleLibrary, shared: Arc<Shared>) -> Result<Session, String> {
        let looping = kind == Kind::Song;
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let rate = config.sample_rate;
        let mut e = Engine::new(project, library, rate, mode);
        // Pattern playback loops on the bar line.
        e.set_looping(looping && matches!(mode, PlayMode::Pattern(_)));
        if kind == Kind::Live {
            e.clear_schedule();
        }
        let engine = Arc::new(Mutex::new(e));
        let done = Arc::new(AtomicBool::new(false));
        if looping {
            shared.sample_rate.store(rate, Ordering::Relaxed);
            shared.position.store(0, Ordering::Relaxed);
        }
        let args = (&device, &config, engine.clone(), done.clone(), shared.clone(), looping);
        let stream = match format {
            SampleFormat::F32 => build::<f32>(args),
            SampleFormat::I16 => build::<i16>(args),
            SampleFormat::I32 => build::<i32>(args),
            SampleFormat::U16 => build::<u16>(args),
            other => Err(format!("unsupported output sample format {other:?}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        if looping {
            shared.playing.store(true, Ordering::Relaxed);
        }
        Ok(Session { _stream: stream, kind, engine, done, project: project.clone(), mode, rate })
    }
}

type BuildArgs<'a> = (&'a cpal::Device, &'a StreamConfig, Arc<Mutex<Engine>>, Arc<AtomicBool>, Arc<Shared>, bool);

fn build<T>((device, config, engine, done, shared, report): BuildArgs<'_>) -> Result<cpal::Stream, String>
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
                // Paused: silence, position held.
                let paused = report && shared.paused.load(Ordering::Relaxed);
                match engine.try_lock() {
                    _ if paused => stereo.fill(0.0),
                    Ok(mut e) if !e.finished() => {
                        e.process(&mut stereo);
                        if report {
                            shared.position.store(e.position(), Ordering::Relaxed);
                            // Never block the audio thread; a missed update shows next block.
                            if let Ok(mut v) = shared.sounding.try_lock() {
                                v.clear();
                                v.extend(e.sounding().map(|(c, k)| (c as u16, k)));
                            }
                        }
                    }
                    Ok(_) => {
                        stereo.fill(0.0);
                        done.store(true, Ordering::Relaxed);
                    }
                    Err(_) => stereo.fill(0.0),
                }
                let (mut pl, mut pr) = (0.0f32, 0.0f32);
                for (frame, s) in out.chunks_mut(channels).zip(stereo.chunks_exact(2)) {
                    pl = pl.max(s[0].abs());
                    pr = pr.max(s[1].abs());
                    for (c, slot) in frame.iter_mut().enumerate() {
                        let v = if c < 2 { s[c] } else { 0.0 };
                        *slot = T::from_sample(v.clamp(-1.0, 1.0));
                    }
                }
                if report {
                    shared.peak_l.store(pl.to_bits(), Ordering::Relaxed);
                    shared.peak_r.store(pr.to_bits(), Ordering::Relaxed);
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

    /// Needs an audio device: `cargo test -p os-app -- --ignored deleted_notes`.
    #[test]
    #[ignore]
    fn deleted_notes_stop_playing() {
        let mut project = crate::app::default_project();
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
        let mut project = crate::app::default_project();
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
        let mut project = crate::app::default_project();
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

    /// Pause holds the position silently; resume continues from it (needs an audio device).
    #[test]
    #[ignore]
    fn pause_holds_position() {
        let mut project = crate::app::default_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1), 0.0);
        std::thread::sleep(Duration::from_millis(400));
        t.pause();
        std::thread::sleep(Duration::from_millis(100));
        let at = t.seconds();
        std::thread::sleep(Duration::from_millis(400));
        assert!(t.paused() && (t.seconds() - at).abs() < 1e-9, "position held");
        t.resume();
        std::thread::sleep(Duration::from_millis(300));
        assert!(!t.paused() && t.seconds() > at + 0.1, "resumed from {at}: {}", t.seconds());
    }
}
