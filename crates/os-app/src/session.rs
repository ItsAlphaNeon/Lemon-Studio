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
    /// What a command or an undo changed that the interface's views follow, taken by them after it (`crate::follow`).
    pub touched: Touched,
    /// The project's samples as decoded (`playlist::Samples`).
    pub samples: crate::playlist::Samples,
    /// Requests for the mixer's slot plugins, handled on the next tick (`mixer::FxReq`).
    pub fx_requests: Vec<crate::mixer::FxReq>,
    /// The remote control links the edit under way moved with what they drive (`remote::follow`), as (target before
    /// the edit, target now): its undo step puts them back (`control::edit`).
    pub link_moves: Vec<(crate::typein::Target, crate::typein::Target)>,
    /// The effects inside containers, by hosted key: mixer track, slot and member id (`App::sync_containers`). The
    /// plugin host's, kept here until it is the session's (control-plane step 7): commands find plugins by it.
    pub plugin_members: std::collections::BTreeMap<usize, (u16, u8, u32)>,
    /// The plugin host: the instances playing the channels and slots, the plugins found, presets, containers
    /// (`crate::plugins`; control-plane step 7).
    pub plugins: crate::plugins::Plugins,
    /// Rack rows played by a hosted plugin.
    pub hosted_rows: Vec<usize>,
    /// Requests for the plugin host, handled on the next tick: (row, `Plugins::available` index) to create, row to open.
    pub pending_plugin: Option<(usize, usize)>,
    pub pending_open: Option<usize>,
    /// The hosted plugins' parameter lists and the last tweaked parameters (spec: plugins/plugin-parameters.md).
    pub params: crate::params::ParamUi,
    /// MIDI controller input, and its held notes: (key, rack row they started on).
    pub(crate) midi: crate::midi_in::MidiIn,
    pub(crate) midi_out: crate::midi_out::MidiOut,
    /// The control plane's socket (`--control`), when it listens.
    pub(crate) rpc_server: Option<crate::control::server::Server>,
    /// An opened project is still loading its samplers (spec: ui/project-loading.md): progress shows, input waits.
    pub(crate) loading: Option<crate::loading::Loading>,
    /// The opened project's missing samples and plugins (`crate::missing`).
    pub(crate) missing: crate::missing::Missing,
    /// A zipped project being unzipped or written (`crate::zipped`): its panel shows, input waits.
    pub(crate) zipping: Option<crate::zipped::Zipping>,
    /// An export is rendering (FILE > Export): its first file's name and progress (0..1); the same panel shows, input
    /// waits.
    pub(crate) export: Option<(String, f32)>,
    /// The running export is PATTERNS > Quick render as audio clip / Render and replace's (spec: ui/pattern-render.md):
    /// what to do with its file.
    pub(crate) pattern_render: Option<crate::pattern_render::Pending>,
    /// The project, tempo and Lemon Studio's own saved state (`lemon_state::saved_form`) as last opened, saved or
    /// created: anything else has unsaved changes.
    pub(crate) saved: (os_model::Project, f64, Vec<u8>),
    /// The Autosave timer and the last backup (spec: ui/settings-choices.md "Backup files").
    pub(crate) backup: crate::backup::Backup,
    /// Performance mode's launched clips (spec: ui/performance-mode.md): what plays besides the arrangement.
    pub perform: crate::perform::Perform,
    /// Playback: the audio thread and what it plays (`crate::transport`). Last, so it goes after the plugins it plays.
    pub transport: crate::transport::Transport,
}

/// What the interface's views have to follow after a command or an undo (`Session::touched`). The session says what it
/// did; the interface knows what that means for its selections and windows.
#[derive(Debug, Default, PartialEq)]
pub struct Touched {
    /// The project was put back whole: the channels' ids by row before, the rows the views pointed at.
    pub rows: Option<Vec<u32>>,
    /// Clips moved in the playlist's list (the selection is kept by id).
    pub clips: bool,
    /// Mixer tracks may have gone.
    pub tracks: bool,
    /// A pattern's notes were put back.
    pub notes: bool,
    /// Undo step `.0`, which holds the rack's side of adding a channel, was undone or redone.
    pub rack_step: Option<u64>,
    /// What the views follow in this order, before the rest above.
    pub views: Vec<View>,
    /// The window has to be drawn again (`App::host`).
    pub repaint: bool,
}

/// A change the interface's views follow, in order (`Touched::views`).
#[derive(Debug, Clone, PartialEq)]
pub enum View {
    /// Mixer places moved: each track's new number by its old one, and each slot's new place, `None` for what went
    /// (what is not listed stays where it is).
    Places { tracks: Vec<Option<u16>>, slots: std::collections::HashMap<(u16, u8), Option<(u16, u8)>> },
    /// The mixer's selection follows tracks renumbered (`tracks` as in `Places`).
    MixerRenumbered(Vec<Option<u16>>),
    /// Mixer track `.0` becomes the current one, the only one selected.
    MixerSelect(u16),
    /// Patterns were renumbered: the clip and note selections go.
    PatternsRenumbered,
    /// The channels' rows moved: each row's new row, `None` for a channel that went (`rack::remap_state`).
    Rows(Vec<Option<usize>>),
    /// The channel rack's rows changed in number: its height follows (`rack::autosize`).
    RackAutosize,
    /// A container member's level knob was set: the open list shows it (`container::show_level`).
    MemberLevel { t: u16, k: u8, id: u32, wet: bool, v: f32 },
    /// The playlist's clips were put back: its selection goes.
    ClipsCleared,
    /// The timebase went from `.0` to `.1` ticks per quarter: the views' ticks and the clipboards follow (`app::retime`).
    Retimed(u16, u16),
    /// A message for the hint bar.
    Message(String),
}

impl View {
    /// `Places` for mixer places moved by `track` and `slot` (as `os_model::target::moved` takes them), over the
    /// project's tracks.
    pub fn places(p: &os_model::Project, track: &dyn Fn(u16) -> Option<u16>, slot: &dyn Fn(u16, u8) -> Option<(u16, u8)>) -> View {
        let n = p.mixer.len() as u16 + 1;
        let tracks = (0..n).map(track).collect();
        let slots = (0..n).flat_map(|t| (0..10u8).map(move |k| (t, k))).map(|(t, k)| ((t, k), slot(t, k))).filter(|&(at, to)| to != Some(at)).collect();
        View::Places { tracks, slots }
    }
}

impl Session {
    pub fn new(project: os_model::Project, transport: crate::transport::Transport) -> Self {
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
            touched: Touched::default(),
            samples: Default::default(),
            fx_requests: Vec::new(),
            link_moves: Vec::new(),
            plugin_members: Default::default(),
            plugins: crate::plugins::Plugins::default(),
            hosted_rows: Vec::new(),
            pending_plugin: None,
            pending_open: None,
            params: Default::default(),
            midi: crate::midi_in::MidiIn::none(),
            midi_out: crate::midi_out::MidiOut::none(),
            loading: None,
            missing: Default::default(),
            zipping: None,
            export: None,
            pattern_render: None,
            saved: (os_model::Project::default(), 0.0, Vec::new()),
            backup: Default::default(),
            rpc_server: None,
            perform: Default::default(),
            transport,
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
