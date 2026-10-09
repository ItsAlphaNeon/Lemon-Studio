//! Playback: an audio thread that owns the output stream and the engine.
//!
//! The UI sends a project snapshot on Play; `update` swaps in an edited snapshot at the current
//! position (heard immediately). Pattern mode loops by rebuilding the engine at the pattern end (decoded
//! samples stay cached in the library). `preview` plays one note once while stopped.

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
    Play(Box<Project>, PlayMode),
    /// The newest edited project is in `Transport::latest` (older ones are skipped).
    Update,
    Preview(Box<Project>),
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

    pub fn play(&self, project: &Project, mode: PlayMode) {
        if let Ok(mut slot) = self.latest.lock() {
            *slot = None;
        }
        let _ = self.tx.send(Cmd::Play(Box::new(project.clone()), mode));
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
    let report = |shared: &Shared, e: String| {
        if let Ok(mut slot) = shared.error.lock() {
            *slot = Some(e);
        }
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(15)) {
            Ok(Cmd::Update) => {
                let project = latest.lock().ok().and_then(|mut l| l.take());
                if let (Some(s), Some(project)) = (session.as_mut(), project) {
                    s.project = project;
                    // Build outside the lock, then swap at the playing position.
                    let mut e = Engine::new(&s.project, &mut library, s.rate, s.mode);
                    if let Ok(mut slot) = s.engine.lock() {
                        e.continue_from(&mut slot);
                        *slot = e;
                    }
                }
            }
            Ok(Cmd::Preview(project)) => {
                preview = None;
                match Session::start(&project, PlayMode::Pattern(1), false, &mut library, shared.clone()) {
                    Ok(s) => preview = Some(s),
                    Err(e) => report(&shared, e),
                }
            }
            Ok(Cmd::Play(project, mode)) => {
                session = None;
                shared.paused.store(false, Ordering::Relaxed);
                match Session::start(&project, mode, true, &mut library, shared.clone()) {
                    Ok(s) => session = Some(s),
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
        let mut finished = false;
        if let Some(s) = &mut session {
            if s.done.load(Ordering::Relaxed) {
                match s.mode {
                    // Patterns loop: swap in a fresh engine at the start.
                    PlayMode::Pattern(_) => {
                        let e = Engine::new(&s.project, &mut library, s.rate, s.mode);
                        if let Ok(mut slot) = s.engine.lock() {
                            *slot = e;
                        }
                        s.done.store(false, Ordering::Relaxed);
                    }
                    PlayMode::Song => finished = true,
                }
            }
        }
        if finished {
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

struct Session {
    _stream: cpal::Stream,
    engine: Arc<Mutex<Engine>>,
    done: Arc<AtomicBool>,
    project: Project,
    mode: PlayMode,
    rate: u32,
}

impl Session {
    /// `looping == false`: a one-shot preview that leaves the song position and play state alone.
    fn start(project: &Project, mode: PlayMode, looping: bool, library: &mut SampleLibrary, shared: Arc<Shared>) -> Result<Session, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no audio output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let rate = config.sample_rate;
        let engine = Arc::new(Mutex::new(Engine::new(project, library, rate, mode)));
        let done = Arc::new(AtomicBool::new(false));
        if looping {
            shared.sample_rate.store(rate, Ordering::Relaxed);
            shared.position.store(0, Ordering::Relaxed);
        }
        let loop_at_end = matches!(mode, PlayMode::Pattern(_));
        let args = (&device, &config, engine.clone(), done.clone(), shared.clone(), loop_at_end, looping);
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
        Ok(Session { _stream: stream, engine, done, project: project.clone(), mode, rate })
    }
}

type BuildArgs<'a> = (&'a cpal::Device, &'a StreamConfig, Arc<Mutex<Engine>>, Arc<AtomicBool>, Arc<Shared>, bool, bool);

fn build<T>((device, config, engine, done, shared, loop_at_end, report): BuildArgs<'_>) -> Result<cpal::Stream, String>
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
                        // Patterns loop on the bar line; ringing voices are cut by the restart.
                        if loop_at_end && e.past_end() {
                            done.store(true, Ordering::Relaxed);
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
        t.play(&project, PlayMode::Pattern(1));
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

    /// Pause holds the position silently; resume continues from it (needs an audio device).
    #[test]
    #[ignore]
    fn pause_holds_position() {
        let mut project = crate::app::default_project();
        let ch = project.channels[0].index;
        project.patterns = vec![os_model::Pattern { number: 1, notes: vec![os_model::Note { length: 24, key: 60, ..os_model::Note::step(ch, 0) }], ..Default::default() }];
        let t = Transport::new(None);
        t.play(&project, PlayMode::Pattern(1));
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
