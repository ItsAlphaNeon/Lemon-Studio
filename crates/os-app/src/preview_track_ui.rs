// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::preview_track` (step 9: the session's side stays there).

#![allow(unused_imports)]
use crate::preview_track::*;

#[cfg(test)]
mod tests {
    use super::*;
    use os_model::Project;
    use crate::app::{preview_project, test_project};

    /// The preview of a short sine file (`wav`), routed to `track` of `open`.
    fn preview(open: &Project, track: u16) -> Project {
        let mut p = preview_project(&wav().to_string_lossy());
        route(&mut p, open, track);
        p
    }

    fn wav() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("os-preview-track-{}.wav", std::process::id()));
        if !path.exists() {
            let audio: Vec<f32> = (0..48_000).map(|i| ((i / 2) as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5).collect();
            os_engine::wav::write_f32_stereo(&mut std::fs::File::create(&path).unwrap(), 48_000, &audio).unwrap();
        }
        path
    }

    /// The first half second of `p`: the peaks of the left and right channels.
    fn peaks(p: &Project) -> (f32, f32) {
        let mut library = os_engine::SampleLibrary::default();
        let mut e = os_engine::Engine::new(p, &mut library, 48_000, os_engine::PlayMode::Pattern(1));
        let mut buf = vec![0.0f32; 48_000];
        e.process(&mut buf);
        let peak = |side: usize| buf.iter().skip(side).step_by(2).fold(0.0f32, |m, v| m.max(v.abs()));
        (peak(0), peak(1))
    }

    // spec: ui/settings-choices.md "Mixer: Browser preview track (2026-10-16)"
    /// An insert the open project has: the preview takes its mixer and plays into that track. "---", the master, the
    /// hidden last track and tracks past the inserts leave the preview as it was (straight into its own master).
    #[test]
    fn the_preview_enters_the_chosen_insert() {
        let mut open = test_project();
        open.mixer[5].volume = Some(0.3);
        open.mixer[5].name = Some("Drums bus".into());
        open.panning_law = Some(2);
        let before = preview_project("kick.wav");
        let mut p = before.clone();
        route(&mut p, &open, 5);
        assert_eq!(p.mixer, open.mixer, "the open project's mixer");
        assert_eq!(p.panning_law, Some(2));
        assert_eq!(p.channels.iter().map(|c| c.mixer_track).collect::<Vec<_>>(), vec![5]);
        assert_eq!((p.channels[0].sample_path.clone(), p.patterns.clone()), (before.channels[0].sample_path.clone(), before.patterns.clone()));
        let inserts = open.mixer.len() as u16 - 2;
        route(&mut p, &open, inserts);
        assert_eq!(p.channels[0].mixer_track, inserts, "the last insert");
        for track in [0, inserts + 1, inserts + 2, 125, u16::MAX] {
            let mut p = before.clone();
            route(&mut p, &open, track);
            assert_eq!(p, before, "track {track}: as before");
        }
        // A project with no mixer has no insert.
        let mut p = before.clone();
        route(&mut p, &Project::default(), 1);
        assert_eq!(p, before);
    }

    /// Heard: the insert's volume and pan apply, and its routing on to the master (a send to a silent track
    /// silences it); "---" and a missing track play as before.
    #[test]
    fn the_inserts_volume_pan_and_routing_apply() {
        let mut open = test_project();
        let plain = peaks(&preview(&open, 0));
        assert!(plain.0 > 0.1 && (plain.0 - plain.1).abs() < 1e-4, "---: centred and heard: {plain:?}");
        assert_eq!(peaks(&preview(&open, 40)), plain, "a track the project lacks: as ---");
        assert_eq!(peaks(&preview(&open, 3)), plain, "a default insert passes it as it is");
        open.mixer[3].pan = Some(-1.0);
        let left = peaks(&preview(&open, 3));
        assert!(left.0 > 0.1 && left.1 < 1e-3, "panned hard left: {left:?}");
        open.mixer[3].pan = None;
        open.mixer[3].volume = Some(0.4);
        let quiet = peaks(&preview(&open, 3));
        assert!(quiet.0 < plain.0 * 0.7 && quiet.0 > 0.0, "turned down: {quiet:?} against {plain:?}");
        open.mixer[3].volume = None;
        open.mixer[4].volume = Some(0.0);
        open.mixer[3].routes = vec![4];
        assert!(peaks(&preview(&open, 3)).0 < 1e-4, "routed on through a silent track");
        assert_eq!(peaks(&preview(&open, 0)), plain, "--- does not go through the project's tracks");
        let _ = std::fs::remove_file(wav());
    }
}
