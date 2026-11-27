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
    if let Some(name) = s.session.rec_files.get(&track).and_then(|p| p.file_name()) {
        return name.to_string_lossy().into_owned();
    }
    file_name(
        &crate::backup::project_name(s.session.path.as_deref()),
        chrono::Local::now().naive_local(),
        &crate::mixer::name(&s.session, track),
    )
}

impl App {
    /// The disk-record button's right click: a Save dialog "Record <track> to", in the Recorded folder, holding the
    /// name the track's recording would get (spec: audio/disk-recording.md §3).
    pub fn choose_rec_file(&mut self, track: u16) {
        use crate::filedlg::{FileDlg, Mode};
        let title = format!("Record {} to", crate::mixer::name(&self.state.session, track));
        let dir = crate::track_rec::folder(&self.state.session);
        let name = armed_file(&self.state, track);
        let dlg = FileDlg::new(Mode::Save, &title, "record", crate::dialog::Then::RecordTo(track))
            .filter("Microsoft wave file", &["wav"])
            .start(Some(dir.as_path()))
            .name(&name);
        self.file_dialog(dlg);
    }

    /// The file chosen: the track's next recording goes there (".wav" added when missing), and the track is armed.
    pub fn rec_file_chosen(&mut self, track: u16, file: std::path::PathBuf) {
        let file = if file.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")) {
            file
        } else {
            file.with_extension("wav")
        };
        self.state.session.rec_files.insert(track, file);
        let armed = self.state.session.project.mixer.iter().any(|m| m.index == track && m.armed);
        if !armed {
            if let Some(msg) = self.arm_tracks(&[track], true) {
                self.state.message = Some(msg);
            }
        }
        self.invalidate();
    }

    /// Every tick: the session's `tick_in` with the song position and the playlist's clip brush.
    pub fn track_rec_tick(&mut self) {
        let (pos, lock) = (self.state.session.song_pos, crate::playlist::brush(&self.state));
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
        assert!(app
            .state
            .session
            .history
            .entries(&crate::settings_file::store())
            .iter()
            .any(|e| e.1 == crate::one_click::RECORDED));
        assert_eq!(app.arm_tracks(&[1], false), None);
        assert!(!app.state.session.project.mixer.iter().any(|m| m.armed));
        let _ = std::fs::remove_file(&path);
    }

    /// The disk-record button's right click: the file chosen in "Record <track> to" (".wav" added) is the track's next
    /// recording and shows in its hint, and the track is armed; a recording takes it once (spec:
    /// audio/disk-recording.md §3).
    #[test]
    fn right_click_on_arm_chooses_the_file() {
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        let dir = std::env::temp_dir().join(format!("lemon-recto-{}", std::process::id()));
        crate::dialog::fake::answer(Some(dir.join("vox take")));
        crate::mixer::right(&mut app, crate::mixer::MHit::Arm(3));
        assert_eq!(app.state.session.rec_files.get(&3), Some(&dir.join("vox take.wav")));
        assert_eq!(armed_file(&app.state, 3), "vox take.wav");
        assert!(crate::mixer::track(&app.state.session, 3).is_some_and(|m| m.armed));
        crate::dialog::fake::answer(None);
        crate::mixer::right(&mut app, crate::mixer::MHit::Arm(4));
        assert!(!crate::mixer::track(&app.state.session, 4).is_some_and(|m| m.armed), "Cancel leaves it");
    }

    /// Waits (up to 3 s) for `done`, ticking the recording.
    fn tick_until(app: &mut App, mut done: impl FnMut(&App) -> bool) {
        for _ in 0..600 {
            app.track_rec_tick();
            if done(app) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("timed out: {:?}", app.state.message);
    }

    /// Loop record: a pass becomes a file and a clip while the next one records; with Group loop recorded takes on, the
    /// later passes go on tracks grouped under the first one's (the tracks below move down) and named after the mixer
    /// track, the files "<stem>.wav", "<stem>_2.wav"...; with Blend off all the passes are muted (spec:
    /// audio/disk-recording.md §1).
    #[test]
    fn loop_passes_become_grouped_takes() {
        let st = crate::settings_file::store();
        let keep =
            (st.check(&crate::prefs::GROUP_LOOP_TAKES), Opt::Blend.on(&st), st.check(&crate::prefs::AUTO_AUDIO_CLIPS));
        let keep2 = (st.index(&crate::prefs::REC_DEPTH), st.check(&crate::prefs::AUTO_NAME_TRACKS));
        st.set_choice(&crate::prefs::REC_DEPTH, 2);
        st.set_check(&crate::prefs::AUTO_NAME_TRACKS, true);
        st.set_check(&crate::prefs::GROUP_LOOP_TAKES, true);
        st.set_check(&crate::prefs::AUTO_AUDIO_CLIPS, true);
        if Opt::Blend.on(&st) {
            Opt::Blend.toggle(&st);
        }
        let mut app = App::new(None);
        app.state.session.project = crate::locked::Locked::new(crate::projects::test_project());
        app.state.set_song_mode(true);
        app.arm_tracks(&[2], true);
        app.state.session.transport.pretend_at(0.0, true);
        app.state.session.rec.armed = true;
        app.track_rec_tick();
        let takes = app.state.session.track_rec.as_ref().expect("a take").takes.clone();
        let ch0 = app.state.session.project.channels.len();
        // A clip on track 2 that the grouped tracks push down.
        crate::edit::playlist::place_in(&mut app.state.session, os_model::ClipSource::Pattern(1), 1, 960, None);
        takes.push(2, 0, &vec![0.25; 9600]);
        takes.push(2, 48, &vec![0.25; 9600]);
        app.track_rec_tick();
        assert_eq!(app.state.session.project.channels.len(), ch0, "one pass: nothing yet");
        takes.push(2, 0, &vec![0.5; 4800]);
        takes.push(2, 48, &vec![0.5; 4800]);
        tick_until(&mut app, |a| a.state.session.project.channels.len() == ch0 + 1);
        takes.push(2, 0, &vec![0.75; 4800]);
        tick_until(&mut app, |a| a.state.session.project.channels.len() == ch0 + 2);
        app.state.session.transport.stop();
        tick_until(&mut app, |a| a.state.session.track_rec.is_none());
        let p = &app.state.session.project;
        assert_eq!(p.channels.len(), ch0 + 3, "{:?}", app.state.message);
        let arr = p.arrangement().unwrap();
        let takes: Vec<_> = arr.clips.iter().filter(|c| matches!(c.source, os_model::ClipSource::Channel(_))).collect();
        assert_eq!(
            takes.iter().map(|c| (c.track, c.position, c.muted)).collect::<Vec<_>>(),
            [(0, 0, true), (1, 0, true), (2, 0, true)]
        );
        assert!(!arr.tracks[0].grouped && arr.tracks[1].grouped && arr.tracks[2].grouped);
        assert!(arr.clips.iter().any(|c| c.source == os_model::ClipSource::Pattern(1) && c.track == 3), "moved down");
        let bus = crate::edit::mixer::name(&app.state.session, 2);
        assert_eq!(arr.tracks[1].name.as_deref(), Some(bus.as_str()), "named after the mixer track");
        let files: Vec<_> =
            p.channels[ch0..].iter().map(|c| std::path::PathBuf::from(c.sample_path.clone().unwrap())).collect();
        let stem = files[0].file_stem().unwrap().to_string_lossy().into_owned();
        assert!(stem.ends_with(&bus), "{stem}");
        let names: Vec<_> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, [format!("{stem}.wav"), format!("{stem}_2.wav"), format!("{stem}_3.wav")]);
        let lens: Vec<u64> = files.iter().map(|f| (std::fs::metadata(f).unwrap().len() - 44) / 8).collect();
        assert_eq!(lens, [9600, 4800, 2400], "each pass is its own part of the take");
        let entries = app.state.session.history.entries(&st);
        assert_eq!(entries.iter().filter(|e| e.1 == crate::one_click::RECORDED).count(), 3, "a step per pass");
        for f in files {
            let _ = std::fs::remove_file(f);
        }
        st.set_check(&crate::prefs::GROUP_LOOP_TAKES, keep.0);
        st.set_check(&crate::prefs::AUTO_AUDIO_CLIPS, keep.2);
        st.set_choice(&crate::prefs::REC_DEPTH, keep2.0);
        st.set_check(&crate::prefs::AUTO_NAME_TRACKS, keep2.1);
        if Opt::Blend.on(&st) != keep.1 {
            Opt::Blend.toggle(&st);
        }
    }
}
