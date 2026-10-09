//! Playback: an audio thread that owns the output stream and the engine.
//!
//! The UI sends a project snapshot on Play; `update` swaps in an edited snapshot, heard from the
//! next pattern loop. Pattern mode loops by rebuilding the engine at the pattern end (decoded
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
    /// Playback position in frames.
    pub position: AtomicU64,
    pub sample_rate: AtomicU32,
    /// Peak of the last block, f32 bits, per side.
    pub peak_l: AtomicU32,
    pub peak_r: AtomicU32,
    pub error: Mutex<Option<String>>,
}

enum Cmd {
    Play(Box<Project>, PlayMode),
    Update(Box<Project>),
    Preview(Box<Project>),
    Stop,
}

pub struct Transport {
    tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
}

impl Transport {
    pub fn new(factory: Option<PathBuf>) -> Transport {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        std::thread::Builder::new()
            .name("openstudio-audio".into())
            .spawn(move || audio_thread(rx, s, SampleLibrary::new(factory)))
            .expect("spawn audio thread");
        Transport { tx, shared }
    }

    pub fn play(&self, project: &Project, mode: PlayMode) {
        let _ = self.tx.send(Cmd::Play(Box::new(project.clone()), mode));
    }

    /// Replace the playing project (takes effect at the next pattern loop).
    pub fn update(&self, project: &Project) {
        let _ = self.tx.send(Cmd::Update(Box::new(project.clone())));
    }

    /// Play pattern 1 of `project` once (a one-note preview project); ignored while playing.
    pub fn preview(&self, project: Project) {
        let _ = self.tx.send(Cmd::Preview(Box::new(project)));
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

    pub fn take_error(&self) -> Option<String> {
        self.shared.error.lock().ok()?.take()
    }
}

fn audio_thread(rx: Receiver<Cmd>, shared: Arc<Shared>, mut library: SampleLibrary) {
    let mut session: Option<Session> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(15)) {
            Ok(Cmd::Update(project)) => {
                if let Some(s) = session.as_mut().filter(|s| s.looping) {
                    s.project = *project;
                }
            }
            Ok(Cmd::Preview(project)) => {
                if session.as_ref().is_none_or(|s| !s.looping) {
                    session = None;
                    match Session::start(&project, PlayMode::Pattern(1), false, &mut library, shared.clone()) {
                        Ok(s) => session = Some(s),
                        Err(e) => {
                            if let Ok(mut slot) = shared.error.lock() {
                                *slot = Some(e);
                            }
                        }
                    }
                }
            }
            Ok(Cmd::Play(project, mode)) => {
                session = None;
                match Session::start(&project, mode, true, &mut library, shared.clone()) {
                    Ok(s) => session = Some(s),
                    Err(e) => {
                        if let Ok(mut slot) = shared.error.lock() {
                            *slot = Some(e);
                        }
                    }
                }
            }
            Ok(Cmd::Stop) => session = None,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let mut finished = false;
        if let Some(s) = &mut session {
            if s.done.load(Ordering::Relaxed) {
                match s.mode {
                    PlayMode::Pattern(_) if !s.looping => finished = true,
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
        }
        if session.as_ref().is_none_or(|s| !s.looping) {
            shared.playing.store(false, Ordering::Relaxed);
        }
        if session.is_none() {
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
    /// False for a one-shot note preview.
    looping: bool,
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
        Ok(Session { _stream: stream, engine, done, project: project.clone(), mode, rate, looping })
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
                match engine.try_lock() {
                    Ok(mut e) if !e.finished() => {
                        e.process(&mut stereo);
                        if report {
                            shared.position.store(e.position(), Ordering::Relaxed);
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
