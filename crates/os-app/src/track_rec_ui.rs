// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::track_rec` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use crate::app::{App, State};
use crate::session::{Session, View};
use crate::capture::{Recorder, Ring};
use crate::record::Opt;
#[allow(unused_imports)]
use crate::track_rec::*;

/// The file an armed track would record into now (the disk-record button's hint).
pub fn armed_file(s: &State, track: u16) -> String {
    file_name(&crate::backup::project_name(s.session.path.as_deref()), chrono::Local::now().naive_local(), &crate::mixer::name(&s.session, track))
}

impl App {
    /// Every tick: the session's `tick_in` with the song position and the playlist's last clip length.
    pub fn track_rec_tick(&mut self) {
        let (pos, lock) = (self.state.session.song_pos, self.state.playlist.lock_len);
        self.host(|ses| tick_in(ses, pos, lock));
    }

    /// Arm selected track(s) / Disarm selected track(s) (the track menu), and the disk-record button: arming turns
    /// Audio on in Record's filter, so the armed tracks record (spec: audio/track-io.md §2.5).
    pub fn arm_tracks(&mut self, tracks: &[u16], on: bool) -> Option<String> {
        // Arming is no undo step (the track is the same armed or not, §1.5).
        crate::control_ui::settle_ids(&mut self.state);
        let ses = &self.state.session;
        let cmds: Vec<_> = tracks
            .iter()
            .filter_map(|&t| crate::mixer::track(ses, t).filter(|m| m.armed != on))
            .map(|m| {
                let mut c = crate::control::command::Command::set_track(m.id.0);
                if let crate::control::command::Command::SetTrack { armed, .. } = &mut c {
                    *armed = Some(on);
                }
                c
            })
            .collect();
        let n = cmds.len();
        if n > 0 {
            let _ = crate::control_ui::run(&mut self.state, cmds);
        }
        self.state.session.dirty = true;
        self.invalidate();
        (on && n > 0 && !Opt::Audio.on(&self.state.session.settings)).then(|| {
            Opt::Audio.toggle(&self.state.session.settings);
            "Recording filter: Audio on (armed tracks record while Record is armed)".to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use os_engine::RecordTap;

    /// Arming turns Audio on; a take of an armed track from Record and Play to Stop becomes a clip at its first tick,
    /// its channel on that track, as "Recorded items" (spec: audio/track-io.md §2.5).
    #[test]
    fn an_armed_track_records_into_the_playlist() {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.set_song_mode(true);
        if Opt::Audio.on(&crate::settings_file::store()) {
            Opt::Audio.toggle(&crate::settings_file::store());
        }
        let msg = app.arm_tracks(&[1], true);
        assert!(msg.is_some_and(|m| m.contains("Audio on")) && Opt::Audio.on(&crate::settings_file::store()));
        assert!(app.arm_tracks(&[1], true).is_none(), "already armed: says nothing");
        app.state.session.transport.pretend_at(0.0, true);
        app.track_rec_tick();
        assert!(app.state.session.track_rec.is_none(), "Record is not armed");
        app.state.session.rec.armed = true;
        app.track_rec_tick();
        let takes = app.state.session.track_rec.as_ref().expect("a take").takes.clone();
        takes.push(2, 90, &[0.5; 96]);
        takes.push(1, 96, &vec![0.25; 9600]);
        takes.push(1, 97, &vec![0.25; 9600]);
        let channels = app.state.session.project.channels.len();
        app.state.session.transport.stop();
        for _ in 0..500 {
            app.track_rec_tick();
            if app.state.session.track_rec.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(app.state.session.track_rec.is_none(), "{:?}", app.state.message);
        assert_eq!(app.state.session.project.channels.len(), channels + 1, "{:?}", app.state.message);
        let ch = app.state.session.project.channels.last().unwrap();
        assert_eq!(ch.mixer_track, 1);
        let path = std::path::PathBuf::from(ch.sample_path.clone().unwrap());
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(path.parent().unwrap().ends_with("Recorded") && name.ends_with("_Bus 1.wav"), "{name}");
        let clip = app.state.session.project.arrangement().unwrap().clips.last().unwrap().clone();
        assert_eq!(clip.position, 96);
        assert!(app.state.session.history.entries(&crate::settings_file::store()).iter().any(|e| e.1 == crate::one_click::RECORDED));
        assert_eq!(app.arm_tracks(&[1], false), None);
        assert!(!app.state.session.project.mixer.iter().any(|m| m.armed));
        let _ = std::fs::remove_file(&path);
    }

}
