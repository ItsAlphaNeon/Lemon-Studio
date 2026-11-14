// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::one_click` (step 9: the session's side stays there).

#![allow(unused_imports)]
use crate::app::{Action, App, State};
use crate::session::{Session, View};
use crate::capture::{self, capture, Input, InputStream, Recorder};
use crate::playlist::{Cmd, Mark, PItem, Popup};
#[allow(unused_imports)]
use crate::one_click::*;

/// The menu (the button's, and TOOLS > One-click audio recording's submenu): the audio editor (which asks for the
/// input), then the inputs for the playlist.
pub fn items(_: &State) -> Vec<PItem> {
    let mut v = vec![PItem::new("Into the audio editor", Cmd::OneClickEditor)];
    v.extend(inputs(false));
    v
}

impl App {
    /// The microphone: stops a recording, else opens the menu under the button.
    pub(crate) fn one_click_button(&mut self) {
        if self.state.session.audio_rec.as_ref().is_some_and(|r| !r.ending) {
            return self.act(Action::Stop);
        }
        let (x, y) = self.mouse;
        let (x, y) = self.top_hit(x, y).map_or((x, y), |h| (h.rect.x, h.rect.bottom() + 2.0));
        self.state.playlist.popup = Some(Popup { x, y, items: items(&self.state), sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// Into the audio editor: the inputs, at the cursor.
    pub(crate) fn one_click_editor_prompt(&mut self) {
        let (x, y) = self.mouse;
        self.state.playlist.popup = Some(Popup { x, y, items: inputs(true), sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// An input picked: records it from the song position, as Play from stopped with Record armed (spec §2); the take
    /// goes into the audio editor (`editor`) or the playlist.
    pub(crate) fn one_click_start(&mut self, input: Input, editor: bool) {
        if self.state.session.audio_rec.is_some() {
            self.state.message = Some("Already recording audio".into());
            return self.invalidate();
        }
        capture::set_last_input(&self.state.session.settings, &input);
        if self.state.session.transport.playing() {
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
        if !self.state.song_mode() {
            self.act(Action::PatSong(true));
        }
        // Armed directly: the audio was the answer to "what to record".
        self.state.session.rec.armed = true;
        self.play_counted_if_armed();
        self.invalidate();
    }

    /// Starts taking in `input` into a new file (no playback: `one_click_start` plays).
    pub(crate) fn one_click_begin(&mut self, input: Input, stream: Option<InputStream>, editor: bool) -> Result<(), String> {
        let pos = self.state.session.song_pos;
        begin_in(&mut self.state.session, input, stream, editor, pos)
    }

    /// Every tick: the session's `tick_in`; a take for the audio editor goes there.
    pub(crate) fn one_click_tick(&mut self) {
        let lock = self.state.playlist.lock_len;
        let Some(path) = self.host(|ses| tick_in(ses, lock)) else { return };
        let placed = self.take_to_editor(&path);
        self.state.message = Some(with_lost(placed));
        self.invalidate();
    }

    /// The take into Tesla on the master track: its first Tesla if that holds no sample, else a new one (spec:
    /// plugins/tesla/README.md "Recording into Tesla from Lemon Studio"). The file stays in `Recorded` either way.
    pub(crate) fn take_to_editor(&mut self, path: &std::path::Path) -> String {
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
}

/// A stereo and a mono header per device with its inputs, each recording into the audio editor (`editor`) or the
/// playlist; the input picked last has a dot.
pub fn inputs(editor: bool) -> Vec<PItem> {
    let last = capture::last_input(&crate::settings_file::store());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::menus::{submenu, TOOLS_MENU};

    /// The tests that record share the one input capture.
    static RECORDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn app() -> App {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
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
        capture::set_last_input(&crate::settings_file::store(), &Input { device: "Test ASIO".into(), asio: true, first: 1, stereo: false });
        let v = submenu(TOOLS_MENU, "Quick record").expect("TOOLS submenu")(&app.state);
        assert_eq!(v[5].mark, Mark::Radio(true), "In 2 dotted");
    }

    /// Into the audio editor: asks for the input, then the take goes into Tesla on the master track instead of the
    /// playlist, and its file stays in the Recorded folder (spec §2 "Into the audio editor").
    #[test]
    fn a_take_goes_into_the_audio_editor() {
        use crate::mixer::FxReq;
        let _one = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = app();
        app.state.session.plugins.effects = crate::plugins::builtin_effects();
        crate::playlist::run(&mut app, Cmd::OneClickEditor);
        let Some(Popup { items, .. }) = app.state.playlist.popup.take() else { panic!("no input menu") };
        assert!(items.iter().any(|i| matches!(&i.cmd, Cmd::OneClick(_, true))));
        assert!(items.iter().all(|i| !matches!(&i.cmd, Cmd::OneClick(_, false))));
        let channels = app.state.session.project.channels.len();
        let clips = app.state.session.project.arrangement().map_or(0, |a| a.clips.len());
        app.one_click_begin(Input { device: "Test ASIO".into(), asio: true, first: 0, stereo: true }, None, true).unwrap();
        app.state.session.transport.pretend_at(0.0, true);
        app.state.session.rec.armed = true;
        app.one_click_tick();
        capture().push(4800, |ch, f| if ch == 0 { f as f32 / 4800.0 } else { 0.5 });
        app.state.session.transport.stop();
        for _ in 0..500 {
            app.one_click_tick();
            if app.state.session.audio_rec.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let msg = app.state.message.clone().unwrap_or_default();
        assert!(msg.starts_with("Recorded ") && msg.ends_with(" into the audio editor"), "{msg}");
        assert_eq!(app.state.session.project.channels.len(), channels, "no channel");
        assert_eq!(app.state.session.project.arrangement().map_or(0, |a| a.clips.len()), clips, "no clip");
        let Some(FxReq::Create { track: 0, state: Some(st), .. }) = app.state.session.fx_requests.last().cloned() else {
            panic!("{:?}", app.state.session.fx_requests)
        };
        let saved = tesla::state::from_plugin_state(&st).unwrap();
        assert!(saved.sample.frames() > 0);
        let path = std::path::PathBuf::from(&saved.path);
        assert!(path.exists() && path.parent().unwrap().ends_with("Recorded"), "{path:?}");
        let _ = std::fs::remove_file(&path);
    }

    /// A take from the song's start to stop: a WAV in the Recorded folder, a new audio clip channel and its clip at
    /// the start, as "Recorded items"; a take with nothing in it places nothing (spec §2 The take).
    #[test]
    fn a_take_becomes_a_clip() {
        let _one = RECORDING.lock().unwrap_or_else(|e| e.into_inner());
        let mut app = app();
        let channels = app.state.session.project.channels.len();
        app.one_click_begin(Input { device: "Test ASIO".into(), asio: true, first: 0, stereo: true }, None, false).unwrap();
        assert!(app.state.session.rec.audio && capture().on());
        app.state.session.transport.pretend_at(0.0, true);
        app.state.session.rec.armed = true;
        app.one_click_tick();
        capture().push(4800, |ch, f| if ch == 0 { f as f32 / 4800.0 } else { 0.5 });
        app.state.session.transport.stop();
        for _ in 0..500 {
            app.one_click_tick();
            if app.state.session.audio_rec.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.state.session.audio_rec.is_none() && !app.state.session.rec.audio && !capture().on(), "{:?}", app.state.message);
        assert_eq!(app.state.session.project.channels.len(), channels + 1);
        let ch = app.state.session.project.channels.last().unwrap();
        let path = std::path::PathBuf::from(ch.sample_path.clone().unwrap());
        assert!(path.parent().unwrap().ends_with("Recorded"));
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("untitled_") && name.ends_with("_REC.wav"), "{name}");
        let clip = app.state.session.project.arrangement().unwrap().clips.last().unwrap().clone();
        assert_eq!(clip.position, 0);
        assert!(app.state.session.history.entries(&crate::settings_file::store()).iter().any(|e| e.1 == RECORDED));
        let _ = std::fs::remove_file(&path);
    }
}
