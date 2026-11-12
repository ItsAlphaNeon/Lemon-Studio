// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio


//! The session: the open project and what goes with it (its history, the command layer, where it is saved, what
//! playback still has to be told), apart from the interface's state. It lives inside `app::State` for now; the edits
//! take it alone, and it moves to a crate of its own later (specs `roadmap/control-plane.md`, part 2, steps 6 and 9).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)

use std::path::PathBuf;

pub struct Session {
    pub project: crate::locked::Locked,
    /// The project's undo history (spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    pub history: crate::history::History,
    /// The control plane: ids, its commands' requests for the app, events, proposals (`crate::control`).
    pub control: crate::control::Control,
    pub file: Option<os_flp::FlpFile>,
    pub path: Option<PathBuf>,
    /// What playback reads outside the project changed since the playing engine last got it; `App::tick` sends it to
    /// the transport (a write to the project says so by itself: `crate::locked`).
    pub dirty: bool,
    /// Samples to load into rows' Lemon Sampler instances on the next tick (browser send, drops on a rack row).
    pub sample_loads: Vec<(usize, String)>,
    /// An undo or redo changed the timebase (`history::Timebase`): playback stops on the next tick.
    pub retimed: bool,
    /// A setting inside a hosted plugin changed since the project was opened or saved (its knobs, its own window): the
    /// project counts as unsaved, as the plugin's state is saved with it.
    pub plugin_edited: bool,
    /// The loop type last chosen while loop mode was off, and the pattern it was chosen in (FL keeps it in memory, not
    /// in the file: `rack::loop_type`).
    pub loop_type: u8,
    pub loop_type_of: u16,
}

impl Session {
    pub fn new(project: os_model::Project) -> Self {
        Session {
            project: crate::locked::Locked::new(project),
            history: Default::default(),
            control: Default::default(),
            file: None,
            path: None,
            dirty: false,
            sample_loads: Vec::new(),
            retimed: false,
            plugin_edited: false,
            loop_type: 0,
            loop_type_of: 0,
        }
    }

    /// SONG (or PAT) on the toolbar: the project's song mode (event 9). As in FL it is no undo step and no unsaved
    /// change (`ui/undo-steps.md`): `App::unsaved` leaves it out, and an undo or redo of another step leaves it.
    pub fn song_mode(&self) -> bool {
        self.project.song_mode.unwrap_or(false)
    }

    pub fn set_song_mode(&mut self, on: bool) {
        self.project.write_if(|p| p.song_mode.replace(on) != Some(on));
    }

    /// The pattern shown: the project's (event 67), so a save opens on it again with its loops. As in FL, showing
    /// another is no undo step but an unsaved change (`ui/undo-steps.md`); an undo leaves it (control::command).
    pub fn pattern(&self) -> u16 {
        self.project.current_pattern.unwrap_or(1).max(1)
    }

    pub fn set_pattern(&mut self, n: u16) {
        self.project.write_if(|p| p.current_pattern.replace(n) != Some(n));
    }

    /// The tempo: the project's (BPM). Its edits are undo steps, as FL's "Knob tweak (tempo)" (`ui/undo-steps.md`).
    pub fn tempo(&self) -> f64 {
        self.project.tempo.unwrap_or(130.0)
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        self.project.write_if(|p| p.tempo.replace(bpm) != Some(bpm));
    }
}
