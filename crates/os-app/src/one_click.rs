// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! One-click audio recording (spec: ui/one-click-recording.md §2): the toolbar's microphone (and TOOLS > One-click
//! audio recording) picks an input, plays the song with Record armed and turns what came in into a new audio clip
//! channel and its clip in the playlist where playback started, or (Into the audio editor) into Tesla on the master
//! track (spec: plugins/tesla/README.md "Recording into Tesla from Lemon Studio").

use crate::app::{Action, App, State};
use crate::capture::{self, capture, Input, InputStream, Recorder};
use crate::playlist::{Cmd, Mark, PItem, Popup};

/// The undo step a take is placed with (as FL's History names it).
pub const RECORDED: &str = "Recorded items";

/// A recording under way, or its file being made.
pub struct AudioRec {
    /// A system input device's stream (`None`: the ASIO driver's inputs, in its output callback).
    stream: Option<InputStream>,
    recorder: Option<Recorder>,
    /// The system input's rate (`None`: the ASIO driver's, the output's).
    rate: Option<u32>,
    /// Where playback started: the clip goes there.
    start_tick: u32,
    start_secs: f64,
    /// Where the song's start falls in what came in: a running sum and count (`one_click_tick`).
    est: (f64, u32),
    /// Taking in has stopped; the writer is making the file.
    ending: bool,
    /// The take goes into Tesla on the master track instead of the playlist.
    editor: bool,
}

/// The menu (the button's, and TOOLS > One-click audio recording's submenu): the audio editor (which asks for the
/// input), then the inputs for the playlist.
pub fn items(_: &State) -> Vec<PItem> {
    let mut v = vec![PItem::new("Into the audio editor", Cmd::OneClickEditor)];
    v.extend(inputs(false));
    v
}

/// A stereo and a mono header per device with its inputs, each recording into the audio editor (`editor`) or the
/// playlist; the input picked last has a dot.
pub fn inputs(editor: bool) -> Vec<PItem> {
    let last = capture::last_input();
    let mut v = Vec::new();
    let list = devices();
    for (device, asio, channels) in &list {
        if *channels == 0 {
            v.push(header(device.clone()));
            v.push(PItem::new("(no audio inputs)", Cmd::Nothing).off());
            continue;
        }
        let inputs = Input::of(device, *asio, *channels);
        for stereo in [true, false] {
            let group: Vec<&Input> = inputs.iter().filter(|i| i.stereo == stereo).collect();
            if group.is_empty() {
                continue;
            }
            v.push(header(format!("{device} - {}", if stereo { "stereo" } else { "mono" })));
            for i in group {
                v.push(PItem::new(i.label(), Cmd::OneClick(i.clone(), editor)).mark(Mark::Radio(last.as_ref() == Some(i))));
            }
        }
    }
    if list.is_empty() {
        v.push(PItem::new("(no audio inputs)", Cmd::Nothing).off());
    }
    v
}

fn header(label: String) -> PItem {
    let mut h = PItem::new(label, Cmd::Nothing);
    h.head = true;
    h
}

/// `capture::devices`, listed again at most every few seconds (the menu is drawn on every repaint while open).
fn devices() -> Vec<(String, bool, u16)> {
    #[cfg(test)]
    {
        vec![("Test ASIO".to_string(), true, 2)]
    }
    #[cfg(not(test))]
    {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        type Listed = Option<(Instant, Vec<(String, bool, u16)>)>;
        static CACHE: Mutex<Listed> = Mutex::new(None);
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        match &*c {
            Some((at, list)) if at.elapsed() < Duration::from_secs(3) => list.clone(),
            _ => {
                let list = capture::devices();
                *c = Some((Instant::now(), list.clone()));
                list
            }
        }
    }
}

impl App {
    /// The microphone: stops a recording, else opens the menu under the button.
    pub(crate) fn one_click_button(&mut self) {
        if self.audio_rec.as_ref().is_some_and(|r| !r.ending) {
            return self.act(Action::Stop);
        }
        let (x, y) = self.mouse;
        let (x, y) = self.top_hit(x, y).map_or((x, y), |h| (h.rect.x, h.rect.bottom() + 2.0));
        self.state.playlist.popup = Some(Popup { x, y, items: items(&self.state), sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// Into the audio editor: the inputs, at the cursor (as FL prompts for one).
    pub(crate) fn one_click_editor_prompt(&mut self) {
        let (x, y) = self.mouse;
        self.state.playlist.popup = Some(Popup { x, y, items: inputs(true), sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// An input picked: records it from the song position, as Play from stopped with Record armed (spec §2); the take
    /// goes into the audio editor (`editor`) or the playlist.
    pub(crate) fn one_click_start(&mut self, input: Input, editor: bool) {
        if self.audio_rec.is_some() {
            self.state.message = Some("Already recording audio".into());
            return self.invalidate();
        }
        capture::set_last_input(&input);
        if self.transport.playing() {
            self.act(Action::Stop);
        }
        let stream = if input.asio {
            None
        } else {
            match capture::open_input(&input.device) {
                Ok(s) => Some(s),
                Err(e) => {
                    self.state.message = Some(format!("Audio input: {e}"));
                    return self.invalidate();
                }
            }
        };
        if let Err(e) = self.one_click_begin(input, stream, editor) {
            self.state.message = Some(format!("Audio recording: {e}"));
            return self.invalidate();
        }
        if !self.state.song_mode {
            self.act(Action::PatSong(true));
        }
        // Armed directly: the audio was the answer to "what to record".
        self.state.rec.armed = true;
        self.play_counted_if_armed();
        self.invalidate();
    }

    /// Starts taking in `input` into a new file (no playback: `one_click_start` plays).
    pub(crate) fn one_click_begin(&mut self, input: Input, stream: Option<InputStream>, editor: bool) -> Result<(), String> {
        let rate = stream.as_ref().map(|s| s.rate);
        let dir = crate::data_folder::folder(&self.state).or_else(crate::app::theme_dir).unwrap_or_else(std::env::temp_dir).join("Recorded");
        let name = capture::file_name(&crate::backup::project_name(self.state.path.as_deref()), chrono::Local::now().naive_local());
        capture().start(input.first, input.stereo);
        let recorder = match Recorder::start(dir.join(name), rate.unwrap_or(48000)) {
            Ok(r) => r,
            Err(e) => {
                capture().stop();
                return Err(e);
            }
        };
        let start_tick = self.state.playlist.pos;
        let p = &self.state.project;
        let tempo = p.tick_tempo(p.tempo.unwrap_or(140.0).max(1.0));
        let start_secs = os_engine::laws::tick_to_frame(f64::from(start_tick), p.ppq.max(1), tempo, 48000) as f64 / 48000.0;
        self.state.rec.audio = true;
        self.audio_rec = Some(AudioRec { stream, recorder: Some(recorder), rate, start_tick, start_secs, est: (0.0, 0), ending: false, editor });
        Ok(())
    }

    /// Every tick: follows where the song's start falls in the input; on stop, pause or disarm finishes the file;
    /// once it is made, places it (spec §2 The take).
    pub(crate) fn one_click_tick(&mut self) {
        let Some(r) = self.audio_rec.as_mut() else { return };
        if !r.ending {
            let t = &self.transport;
            let live = t.playing() && !t.paused() && self.state.rec.count.is_none();
            let device = t.opened();
            let rate = r.rate.or(device.as_ref().map(|d| d.rate)).unwrap_or(48000);
            if live && t.song() {
                r.est.0 += capture().frames() as f64 - (t.seconds() - r.start_secs) * f64::from(rate);
                r.est.1 += 1;
            }
            if t.playing() && !t.paused() && self.state.rec.armed {
                return;
            }
            // The input heard with the song's start came in after the output's and the input's latencies (ASIO).
            let latency = match (&r.rate, device.filter(|d| d.host == crate::device::ASIO)) {
                (None, Some(d)) => i64::from(d.latency.unwrap_or(0)) + i64::from(d.input_latency.unwrap_or(0)),
                _ => 0,
            };
            let head = if r.est.1 > 0 { (r.est.0 / f64::from(r.est.1)).round() as i64 + latency } else { capture().frames() as i64 };
            capture().stop();
            r.stream = None;
            r.rate = Some(rate);
            if let Some(w) = &r.recorder {
                w.set_rate(rate);
                w.set_head(head);
                w.finish();
            }
            r.ending = true;
            self.state.rec.audio = false;
            self.invalidate();
            return;
        }
        if !r.recorder.as_ref().is_none_or(Recorder::done) {
            return;
        }
        let Some(r) = self.audio_rec.take() else { return };
        let rate = r.rate.unwrap_or(48000).max(1);
        self.state.message = Some(match r.recorder.map_or(Err("no recorder".into()), Recorder::join) {
            Ok((path, frames)) if frames * 1000 >= u64::from(rate) => {
                let placed = if r.editor { self.take_to_editor(&path) } else { self.place_take(&path, frames, rate, r.start_tick) };
                match capture().dropped() {
                    0 => placed,
                    lost => format!("{placed} ({lost} frames were lost: the disk was too slow)"),
                }
            }
            Ok((path, _)) => {
                let _ = std::fs::remove_file(path);
                "Nothing was recorded".into()
            }
            Err(e) => format!("Audio recording: {e}"),
        });
        self.invalidate();
    }

    /// The take's audio clip channel and clip at `start`, on the first track free over it: one undo step.
    fn place_take(&mut self, path: &std::path::Path, frames: u64, rate: u32, start: u32) -> String {
        let s = &mut self.state;
        let p = &s.project;
        let tempo = p.tick_tempo(p.tempo.unwrap_or(140.0).max(1.0));
        let len = (frames as f64 / f64::from(rate) * tempo / 60.0 * f64::from(p.ppq.max(1))).ceil() as u32;
        let track = crate::playlist::free_track(s, 0, start, start + len.max(1));
        let text = path.to_string_lossy();
        match crate::playlist::add_audio_clip_as(s, &text, track, start, RECORDED) {
            Some(_) => format!("Recorded {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
            None => format!("{text} could not be read"),
        }
    }

    /// The take into Tesla on the master track: its first Tesla if that holds no sample, else a new one (spec:
    /// plugins/tesla/README.md "Recording into Tesla from Lemon Studio"). The file stays in `Recorded` either way.
    fn take_to_editor(&mut self, path: &std::path::Path) -> String {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let sample = match std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| tesla::wav::read(&b)) {
            Ok(s) => s,
            Err(e) => return format!("{name} could not be read: {e}"),
        };
        let text = path.to_string_lossy().into_owned();
        if self.tesla_for_recording(0, |s| {
            s.sample = sample;
            s.path = text;
        }) {
            format!("Recorded {name} into the audio editor")
        } else {
            let why = self.state.message.take().unwrap_or_default();
            format!("{why}: {name} is in the Recorded folder")
        }
    }

    /// Whether the empty-playlist check is waived: a recording plays an empty song all the same.
    pub(crate) fn one_click_starting(&self) -> bool {
        self.audio_rec.as_ref().is_some_and(|r| !r.ending)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::menus::{submenu, TOOLS_MENU};

    /// The tests that record share the one input capture.
    static RECORDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn app() -> App {
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        app
    }

    /// The menu: the audio editor, the device's stereo and mono inputs, the last pick dotted (spec §2).
    #[test]
    fn the_menu_lists_the_inputs() {
        let app = app();
        let labels = |v: &[PItem]| v.iter().map(|p| p.label.to_string()).collect::<Vec<_>>();
        let v = items(&app.state);
        assert_eq!(labels(&v), ["Into the audio editor", "Test ASIO - stereo", "In 1 - In 2", "Test ASIO - mono", "In 1", "In 2"]);
        assert!(v[0].enabled && v[0].cmd == Cmd::OneClickEditor && v[1].head && v[3].head);
        assert!(v.iter().all(|p| p.mark != Mark::Radio(true)), "nothing picked yet");
        capture::set_last_input(&Input { device: "Test ASIO".into(), asio: true, first: 1, stereo: false });
        let v = submenu(TOOLS_MENU, "One-click audio recording").expect("TOOLS submenu")(&app.state);
        assert_eq!(v[5].mark, Mark::Radio(true), "In 2 dotted");
    }

    /// A take from the song's start to stop: a WAV in the Recorded folder, a new audio clip channel and its clip at
    /// the start, as "Recorded items"; a take with nothing in it places nothing (spec §2 The take).
    #[test]
    fn a_take_becomes_a_clip() {
        let _one = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = app();
        let channels = app.state.project.channels.len();
        app.one_click_begin(Input { device: "Test ASIO".into(), asio: true, first: 0, stereo: true }, None, false).unwrap();
        assert!(app.state.rec.audio && capture().on());
        app.transport.pretend_at(0.0, true);
        app.state.rec.armed = true;
        app.one_click_tick();
        capture().push(4800, |ch, f| if ch == 0 { f as f32 / 4800.0 } else { 0.5 });
        app.transport.stop();
        for _ in 0..500 {
            app.one_click_tick();
            if app.audio_rec.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.audio_rec.is_none() && !app.state.rec.audio && !capture().on(), "{:?}", app.state.message);
        assert_eq!(app.state.project.channels.len(), channels + 1);
        let ch = app.state.project.channels.last().unwrap();
        let path = std::path::PathBuf::from(ch.sample_path.clone().unwrap());
        assert!(path.parent().unwrap().ends_with("Recorded"));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("untitled_") && name.ends_with("_REC.wav"), "{name}");
        let clip = app.state.project.arrangement().unwrap().clips.last().unwrap().clone();
        assert_eq!(clip.position, 0);
        assert!(app.state.history.entries().iter().any(|e| e.1 == RECORDED));
        let _ = std::fs::remove_file(&path);
    }

    /// Into the audio editor: asks for the input, then the take goes into Tesla on the master track instead of the
    /// playlist, and its file stays in the Recorded folder (spec §2 "Into the audio editor").
    #[test]
    fn a_take_goes_into_the_audio_editor() {
        use crate::mixer::FxReq;
        let _one = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = app();
        app.plugins.effects = crate::plugins::builtin_effects();
        crate::playlist::run(&mut app, Cmd::OneClickEditor);
        let Some(Popup { items, .. }) = app.state.playlist.popup.take() else { panic!("no input menu") };
        assert!(items.iter().any(|i| matches!(&i.cmd, Cmd::OneClick(_, true))));
        assert!(items.iter().all(|i| !matches!(&i.cmd, Cmd::OneClick(_, false))));
        let channels = app.state.project.channels.len();
        let clips = app.state.project.arrangement().map_or(0, |a| a.clips.len());
        app.one_click_begin(Input { device: "Test ASIO".into(), asio: true, first: 0, stereo: true }, None, true).unwrap();
        app.transport.pretend_at(0.0, true);
        app.state.rec.armed = true;
        app.one_click_tick();
        capture().push(4800, |ch, f| if ch == 0 { f as f32 / 4800.0 } else { 0.5 });
        app.transport.stop();
        for _ in 0..500 {
            app.one_click_tick();
            if app.audio_rec.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let msg = app.state.message.clone().unwrap_or_default();
        assert!(msg.starts_with("Recorded ") && msg.ends_with(" into the audio editor"), "{msg}");
        assert_eq!(app.state.project.channels.len(), channels, "no channel");
        assert_eq!(app.state.project.arrangement().map_or(0, |a| a.clips.len()), clips, "no clip");
        let Some(FxReq::Create { track: 0, state: Some(st), .. }) = app.state.mixer.pending.last().cloned() else {
            panic!("{:?}", app.state.mixer.pending)
        };
        let saved = tesla::state::from_plugin_state(&st).unwrap();
        assert!(saved.sample.frames() > 0);
        let path = std::path::PathBuf::from(&saved.path);
        assert!(path.exists() && path.parent().unwrap().ends_with("Recorded"), "{path:?}");
        let _ = std::fs::remove_file(&path);
    }
}
