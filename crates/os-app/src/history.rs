// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The project's undo history: one list of named steps for the whole project, oldest first, after "Last reset" (the
//! project as opened or made). An edit records a step holding what it is about to change (`record`); undo and redo
//! swap that with the project's, so a step always holds the other side of itself: the state before it while it is
//! done, the state after it while it is undone. Covered: a pattern's notes, control points and length (the piano roll
//! and the step sequencer), the playlist (`playlist::snapshot`), knob turns, sample loads (`load_sample`), the
//! project's initial values (`crate::init`), and with the playlist, PATTERNS > Split by channel's patterns and ADD's
//! clip channels (`playlist::snapshot_with`). Also mixer volume faders (`fader`), generators added to the rack
//! (`add_channel`, `insert_channel`), Replace (`replace_channel`), channel deletes (`delete_channels`), Settings >
//! Project's values (`project_settings`) and its timebase (`timebase`), Settings > Info's text (`project_info`) and the
//! plugin parameters Lemon Studio sets (`param`).
//!
//! The settings of General ▸ Undo history (`crate::prefs`): Maximum undo levels (past it the oldest step goes), Undo
//! knob tweaks, Alternate undo mode (what Ctrl+Z and Ctrl+Alt+Z do), Show undo history in the browser when undoing
//! changes and Show recent changes at the top (the browser's History folder, `views::browser`).
// spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)"

use os_model::{ControlPoint, Note};

use crate::app::State;
use crate::knobs::KnobId;
use crate::prefs;

/// The history's first entry: the project as opened or reset.
pub const LAST_RESET: &str = "Last reset";
/// A step-sequencer step click (FL's name).
pub const STEP_SEQ_EDIT: &str = "Step seq edit";
/// A sample loaded into a channel (FL's name; spec: ui/settings-choices.md "Undo history: the four checks").
pub const LOAD_SAMPLE: &str = "Load channel sample(s)";
/// "Create automation clip" (FL's name), with the control's initial value while "Initialize controls automatically" is
/// on (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
pub const CREATE_AUTOMATION: &str = "Create automation clip";
/// A chord written by step edit (spec: ui/step-edit.md).
pub const STEP_EDIT: &str = "Step edit";
/// A mixer track's volume fader moved (named like the knobs' steps, "Mixer track panning").
pub const MIXER_VOLUME: &str = "Mixer track volume";
/// A plugin parameter set by Lemon Studio (type-in, Set..., Paste value).
pub const PLUGIN_PARAMETER: &str = "Plugin parameter";
/// A generator added to the rack (`add_channel`).
pub const ADD_CHANNEL: &str = "Add channel";
/// Channels deleted from the rack (`delete_channels`).
pub const DELETE_CHANNEL: &str = "Delete channel";
/// A value of Settings > Project changed (`project_settings`, `timebase`).
pub const PROJECT_SETTINGS: &str = "Project settings";
/// A channel's generator replaced (`replace_channel`).
pub const REPLACE_CHANNEL: &str = "Replace channel";
/// Settings > Info's text changed (`project_info`).
pub const PROJECT_INFO: &str = "Project info";
/// A mixer track's audio input, output, record source or monitor mode changed (`track_io`).
pub const MIXER_TRACK_IO: &str = "Mixer track audio input / output";

/// What a step changes, as it is on the other side of the step.
#[derive(Debug, Clone, PartialEq)]
pub enum Snap {
    /// Pattern `number`'s notes, control points and length (the piano roll, the step sequencer).
    Pattern { number: u16, notes: Vec<Note>, controls: Vec<ControlPoint>, length: Option<u32> },
    /// The playlist (`playlist::capture`).
    Playlist(Box<crate::playlist::Snapshot>),
    /// A knob's value: the model's own for the channel knobs, else the knob's 0..1.
    Knob(KnobId, f64),
    /// Mixer track `.0`'s volume fader (0..1, the fader law; `None`: the track's default).
    Fader(u16, Option<f64>),
    /// Mixer track `.0`'s audio input, output, record source and monitor mode (`MixerTrack::audio_in` ...); its armed
    /// state is no part of it (spec: audio/track-io.md §2.3).
    TrackIo(u16, [i32; 4]),
    /// Parameter `.1` of hosted plugin instance `.0` (normalized), set by Lemon Studio itself (`crate::params`).
    Param(usize, usize, f64),
    /// What a sample load changes on channel `row`: its sample, its name ("Auto name channels"), its root note and its
    /// stretch ("Read sample root note" and "Read sample tempo information", `rack::read_tags`) and its sampler flags
    /// (Keep on disk, "Auto keep long audio on disk").
    Sample { row: usize, path: Option<String>, name: Option<String>, root: u8, stretch: [u32; 4], flags: Option<u32> },
    /// Channel `row` as a sample dropped below the rack's rows made it (`App::sample_to_rack`); `None` while it
    /// doesn't exist. Only the last row comes and goes.
    Channel { row: usize, ch: Option<Box<os_model::Channel>> },
    /// Control `.0`'s entry in the project's initial values; `None` while it isn't in the list (`crate::init`).
    Init(u32, Option<os_model::InitControl>),
    /// All the patterns and the current one (PATTERNS > Split by channel, spec: ui/split-by-channel.md §2).
    Patterns { patterns: Vec<os_model::Pattern>, current: u16 },
    /// What ADD > Audio clip / Automation clip changes in the rack (`RackSide`).
    Rack(Box<RackSide>),
    /// The rack's rows from a row on, with its groups, filter and selection (`Tail`).
    Tail(Box<Tail>),
    /// A generator added at the bottom of the rack (`crate::plugin_menu::add_generator`): the rack's side, and the
    /// generator (an index of the available plugins; `None`: Lemon Sampler, hosted from the channel's settings) that a
    /// redo hosts again, from its defaults.
    Generator(Box<RackSide>, Option<usize>),
    /// The values of Settings > Project's Time settings, Advanced and Playlist groups (`Settings`).
    Settings(Settings),
    /// The rack on one side of a channel delete (`Rows`).
    Rows(Box<Rows>),
    /// A generator inserted higher up the rack (`insert_channel`): the rack on one side (`Rows`, the inserted row its
    /// one row `gone`), and the generator a redo hosts again from its defaults (an index of the available plugins;
    /// `None`: Lemon Sampler, hosted from the channel's settings).
    Insert(Box<Rows>, Option<usize>),
    /// A channel on one side of Replace (`Replaced`).
    Replace(Box<Replaced>),
    /// The timebase on one side of its change, with the ticks it holds (`Timebase`).
    Timebase(Box<Timebase>),
    /// Settings > Info's text (`project_info`).
    Info(Box<os_model::ProjectInfo>),
    /// Several of these in one step, put back last first ("Create automation clip": the playlist and the initial
    /// value).
    Several(Vec<Snap>),
}

/// The rack's side of ADD > Audio clip / Automation clip (spec: ui/add-clip-channels.md §3): the added channel at `row`
/// (`None` while it doesn't exist; only the last row comes and goes), the rack's filter groups, filter and selection,
/// and the playlist's clip source.
#[derive(Debug, Clone, PartialEq)]
pub struct RackSide {
    pub row: usize,
    pub ch: Option<os_model::Channel>,
    pub groups: Vec<crate::rack::Group>,
    pub filter: crate::rack::Filter,
    pub unsorted_at: Option<usize>,
    pub sel: std::collections::BTreeSet<usize>,
    pub selected: usize,
    pub source: Option<os_model::ClipSource>,
    pub picker: crate::playlist::PickKind,
}

/// The rack's last rows, from row `from` on, with its filter groups, filter and selection: what FILE ▸ Import ▸ MIDI
/// file adds to the open project (spec: ui/midi-files.md §10.7). Rows only come and go at the end.
#[derive(Debug, Clone, PartialEq)]
pub struct Tail {
    pub from: usize,
    pub chans: Vec<os_model::Channel>,
    pub groups: Vec<crate::rack::Group>,
    pub filter: crate::rack::Filter,
    pub unsorted_at: Option<usize>,
    pub sel: std::collections::BTreeSet<usize>,
    pub selected: usize,
}

/// Channel `row` on one side of Replace ▸ (`crate::plugin_menu::replace_generator`). Put back, its instrument is hosted
/// as `host` says: (`Plugins::available` index, saved state; no state: from its defaults); `None`: none is, or Lemon
/// Sampler hosts from the channel's settings. `other` is how the other side's is hosted: a swap hands it over, as the
/// instance's state at the swap is out of the history's reach.
#[derive(Debug, Clone, PartialEq)]
pub struct Replaced {
    pub row: usize,
    pub ch: os_model::Channel,
    pub host: Option<(usize, Option<Vec<u8>>)>,
    pub other: Option<(usize, Option<Vec<u8>>)>,
}

/// The timebase on one side of its change (`timebase`), with the patterns and arrangements at that timebase, so an undo
/// brings back the ticks a lower timebase rounded. The history's other steps follow the timebase (`rescale`); these
/// keep their own.
#[derive(Debug, Clone, PartialEq)]
pub struct Timebase {
    pub ppq: u16,
    pub patterns: Vec<os_model::Pattern>,
    pub arrangements: Vec<os_model::Arrangement>,
}

impl Timebase {
    /// The project's timebase and ticks now.
    pub fn now(s: &State) -> Timebase {
        let p = &s.project;
        Timebase { ppq: p.ppq, patterns: p.patterns.clone(), arrangements: p.arrangements.clone() }
    }
}

/// What Settings > Project changes besides the timebase (`Timebase`): the bar and its
/// mode, the panning law, Play truncated notes in clips and Automatic crossfades (spec: ui/settings-choices.md
/// "Project"). An OS addition: FL's history was not checked for these.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub bar: Option<(u16, u16)>,
    pub bar_as_signature: Option<bool>,
    pub panning_law: Option<u8>,
    pub play_truncated: bool,
    pub auto_crossfades: Option<bool>,
}

/// The rack on one side of a channel delete (`crate::rack::delete_rows`): the channels and everything that refers to
/// them (notes, control points, automation clips, playlist clips, initial values), the rack's groups, filter and
/// selection. `full`: this side has the deleted rows, `gone` (rows of the full rack). `back`: the deleted rows'
/// instruments as the delete found them (row, `Plugins::available` index, saved state), hosted again by an undo.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows {
    pub full: bool,
    pub gone: Vec<usize>,
    pub back: Vec<(usize, usize, Vec<u8>)>,
    pub channels: Vec<os_model::Channel>,
    pub patterns: Vec<os_model::Pattern>,
    pub automations: Vec<os_model::Automation>,
    pub arrangements: Vec<os_model::Arrangement>,
    pub init_controls: Vec<os_model::InitControl>,
    pub groups: Vec<crate::rack::Group>,
    pub filter: crate::rack::Filter,
    pub unsorted_at: Option<usize>,
    pub sel: std::collections::BTreeSet<usize>,
    pub selected: usize,
}

impl Rows {
    /// The rack as it is now, `full` or not.
    pub fn now(s: &State, full: bool, gone: Vec<usize>, back: Vec<(usize, usize, Vec<u8>)>) -> Rows {
        let (p, r) = (&s.project, &s.rack);
        Rows {
            full,
            gone,
            back,
            channels: p.channels.clone(),
            patterns: p.patterns.clone(),
            automations: p.automations.clone(),
            arrangements: p.arrangements.clone(),
            init_controls: p.init_controls.clone(),
            groups: r.groups.clone(),
            filter: r.filter,
            unsorted_at: r.unsorted_at,
            sel: r.sel.clone(),
            selected: s.selected_channel,
        }
    }

    /// Puts this side back: the rack's rows now go to this side's (`State::rows_moved` takes the instruments along).
    fn put(self, s: &mut State) {
        let map: Vec<Option<usize>> = if self.full {
            (0..self.channels.len()).filter(|r| !self.gone.contains(r)).map(Some).collect()
        } else {
            let mut k = 0;
            (0..s.project.channels.len())
                .map(|r| {
                    if self.gone.contains(&r) {
                        return None;
                    }
                    k += 1;
                    Some(k - 1)
                })
                .collect()
        };
        let m = |r: usize| map.get(r).copied().flatten();
        let p = &mut s.project;
        (p.channels, p.patterns, p.automations) = (self.channels, self.patterns, self.automations);
        (p.arrangements, p.init_controls) = (self.arrangements, self.init_controls);
        (s.rack.groups, s.rack.filter, s.rack.unsorted_at) = (self.groups, self.filter, self.unsorted_at);
        (s.rack.sel, s.selected_channel) = (self.sel, self.selected);
        if m(s.plugin_channel).is_none() {
            s.hide(crate::app::Win::Plugin);
        }
        s.plugin_channel = m(s.plugin_channel).unwrap_or(s.selected_channel);
        s.hosted_rows = s.hosted_rows.iter().filter_map(|&r| m(r)).collect();
        s.sample_loads = std::mem::take(&mut s.sample_loads).into_iter().filter_map(|(r, p)| m(r).map(|r| (r, p))).collect();
        s.pending_open = s.pending_open.and_then(m);
        s.pending_plugin = s.pending_plugin.and_then(|(r, i)| m(r).map(|r| (r, i)));
        s.piano.clear_selection();
        s.playlist.sel.clear();
        s.playlist.hover_clip = None;
        let back = if self.full { self.back } else { Vec::new() };
        s.rows_moved.push((map, back));
        crate::rack::autosize(s);
    }
}

impl Settings {
    /// The project's values now.
    pub fn now(s: &State) -> Settings {
        let p = &s.project;
        Settings {
            bar: p.bar,
            bar_as_signature: p.bar_as_signature,
            panning_law: p.panning_law,
            play_truncated: p.play_truncated,
            auto_crossfades: p.auto_crossfades,
        }
    }
}

impl Tail {
    /// The rows from `from` on as the rack has them now.
    pub fn now(s: &State, from: usize) -> Tail {
        let r = &s.rack;
        Tail {
            from,
            chans: s.project.channels.get(from..).map(<[os_model::Channel]>::to_vec).unwrap_or_default(),
            groups: r.groups.clone(),
            filter: r.filter,
            unsorted_at: r.unsorted_at,
            sel: r.sel.clone(),
            selected: s.selected_channel,
        }
    }
}

impl RackSide {
    /// Row `row`'s side as the rack has it now.
    pub fn now(s: &State, row: usize) -> RackSide {
        let r = &s.rack;
        RackSide {
            row,
            ch: s.project.channels.get(row).cloned(),
            groups: r.groups.clone(),
            filter: r.filter,
            unsorted_at: r.unsorted_at,
            sel: r.sel.clone(),
            selected: s.selected_channel,
            source: s.playlist.source,
            picker: s.playlist.picker,
        }
    }
}

impl Snap {
    /// Pattern `number` as it is now.
    pub fn pattern(s: &State, number: u16) -> Snap {
        let p = s.project.pattern(number);
        Snap::Pattern {
            number,
            notes: p.map(|p| p.notes.clone()).unwrap_or_default(),
            controls: p.map(|p| p.controls.clone()).unwrap_or_default(),
            length: p.and_then(|p| p.length),
        }
    }

    /// Channel `row`'s side of a sample load as it is now (`Snap::Sample`).
    pub fn sample(s: &State, row: usize) -> Option<Snap> {
        let ch = s.project.channels.get(row)?;
        let (path, name) = (ch.sample_path.clone(), ch.name.clone());
        Some(Snap::Sample { row, path, name, root: ch.misc.root, stretch: ch.sound.stretch, flags: ch.sound.sampler_flags })
    }

    /// What this covers, as the project has it now.
    fn now(&self, s: &State) -> Snap {
        match self {
            Snap::Pattern { number, .. } => Snap::pattern(s, *number),
            Snap::Playlist(_) => Snap::Playlist(Box::new(crate::playlist::capture(s))),
            Snap::Knob(id, _) => Snap::Knob(*id, knob_value(s, *id)),
            // A track or an instance gone since is left as the step has it (`put` does nothing then).
            Snap::Fader(t, _) => crate::mixer::track(s, *t).map_or_else(|| self.clone(), |m| Snap::Fader(*t, m.volume)),
            Snap::TrackIo(t, _) => crate::mixer::track(s, *t).map_or_else(|| self.clone(), |m| Snap::TrackIo(*t, track_io_of(m))),
            Snap::Param(key, i, _) => crate::params::param_value(s, *key, *i).map_or_else(|| self.clone(), |v| Snap::Param(*key, *i, v)),
            // A channel gone since is left as the step has it (`put` does nothing then).
            Snap::Sample { row, .. } => Snap::sample(s, *row).unwrap_or_else(|| self.clone()),
            Snap::Channel { row, .. } => Snap::Channel { row: *row, ch: s.project.channels.get(*row).cloned().map(Box::new) },
            Snap::Init(t, _) => Snap::Init(*t, crate::init::entry(s, *t)),
            Snap::Patterns { .. } => Snap::Patterns { patterns: s.project.patterns.clone(), current: s.pattern },
            Snap::Rack(r) => Snap::Rack(Box::new(RackSide::now(s, r.row))),
            Snap::Tail(t) => Snap::Tail(Box::new(Tail::now(s, t.from))),
            Snap::Generator(r, g) => Snap::Generator(Box::new(RackSide::now(s, r.row)), *g),
            Snap::Settings(_) => Snap::Settings(Settings::now(s)),
            // The other side of the delete, with the instruments kept for the next undo.
            Snap::Rows(r) => Snap::Rows(Box::new(Rows::now(s, !r.full, r.gone.clone(), r.back.clone()))),
            Snap::Insert(r, g) => Snap::Insert(Box::new(Rows::now(s, !r.full, r.gone.clone(), Vec::new())), *g),
            // A channel gone since is left as the step has it (`put` does nothing then).
            Snap::Replace(r) => match s.project.channels.get(r.row) {
                Some(ch) => Snap::Replace(Box::new(Replaced { row: r.row, ch: ch.clone(), host: r.other.clone(), other: r.host.clone() })),
                None => self.clone(),
            },
            Snap::Timebase(_) => Snap::Timebase(Box::new(Timebase::now(s))),
            Snap::Info(_) => Snap::Info(Box::new(s.project.info.clone())),
            Snap::Several(v) => Snap::Several(v.iter().map(|x| x.now(s)).collect()),
        }
    }

    /// Whether this holds the playlist (`forget_playlist`).
    fn has_playlist(&self) -> bool {
        match self {
            Snap::Playlist(_) => true,
            Snap::Several(v) => v.iter().any(Snap::has_playlist),
            _ => false,
        }
    }

    /// `rescale` for this.
    fn rescale(&mut self, from: u16, to: u16) {
        match self {
            Snap::Pattern { notes, controls, length, .. } => {
                notes.iter_mut().for_each(|n| n.rescale(from, to));
                controls.iter_mut().for_each(|c| c.rescale(from, to));
                *length = length.map(|l| os_model::rescale_length(l, from, to));
            }
            Snap::Playlist(p) => {
                p.0.iter_mut().for_each(|c| c.rescale(from, to));
                p.1.iter_mut().for_each(|m| m.rescale(from, to));
            }
            Snap::Patterns { patterns, .. } => {
                for p in patterns {
                    p.notes.iter_mut().for_each(|n| n.rescale(from, to));
                    p.controls.iter_mut().for_each(|c| c.rescale(from, to));
                    p.markers.iter_mut().for_each(|m| m.rescale(from, to));
                    p.length = p.length.map(|l| os_model::rescale_length(l, from, to));
                }
            }
            Snap::Several(v) => v.iter_mut().for_each(|x| x.rescale(from, to)),
            // A channel or an initial value holds no ticks; a timebase step's are at its own timebase.
            Snap::Knob(..)
            | Snap::Fader(..)
            | Snap::TrackIo(..)
            | Snap::Param(..)
            | Snap::Sample { .. }
            | Snap::Channel { .. }
            | Snap::Init(..)
            | Snap::Rack(_)
            | Snap::Tail(_)
            | Snap::Generator(..)
            | Snap::Settings(_)
            | Snap::Replace(_)
            | Snap::Timebase(_)
            | Snap::Info(_) => {}
            Snap::Rows(r) | Snap::Insert(r, _) => {
                for p in &mut r.patterns {
                    p.notes.iter_mut().for_each(|n| n.rescale(from, to));
                    p.controls.iter_mut().for_each(|c| c.rescale(from, to));
                    p.markers.iter_mut().for_each(|m| m.rescale(from, to));
                    p.length = p.length.map(|l| os_model::rescale_length(l, from, to));
                }
                for a in &mut r.arrangements {
                    a.clips.iter_mut().for_each(|c| c.rescale(from, to));
                    a.markers.iter_mut().for_each(|m| m.rescale(from, to));
                }
            }
        }
    }

    /// Puts this into the project. A pattern's step also shows that pattern, as the piano roll's undo always did.
    fn put(self, s: &mut State) {
        match self {
            Snap::Pattern { number, notes, controls, length } => {
                s.pattern = number;
                let p = crate::patterns::pat_mut(s, number);
                (p.notes, p.controls, p.length) = (notes, controls, length);
                s.piano.clear_selection();
            }
            Snap::Playlist(p) => crate::playlist::restore(s, *p),
            Snap::Knob(id, v) => set_knob_value(s, id, v),
            Snap::Fader(t, v) => {
                if let Some(m) = crate::mixer::track_mut(s, t) {
                    m.volume = v;
                }
            }
            Snap::TrackIo(t, [i, o, r, m]) => {
                if let Some(x) = crate::mixer::track_mut(s, t) {
                    (x.audio_in, x.audio_out, x.rec_source, x.monitor) = (i, o, r as u8, m as u8);
                }
            }
            Snap::Param(key, i, v) => {
                crate::params::set_param_value(s, key, i, v);
            }
            Snap::Sample { row, path, name, root, stretch, flags } => {
                if let Some(ch) = s.project.channels.get_mut(row) {
                    (ch.sample_path, ch.name, ch.misc.root, ch.sound.stretch) = (path.clone(), name, root, stretch);
                    ch.sound.sampler_flags = flags;
                    // Its Lemon Sampler follows on the next tick: the sample (none: cleared), root note, stretch and
                    // Keep on disk.
                    s.sample_loads.push((row, path.unwrap_or_default()));
                }
            }
            // The channel goes or comes back only as the last row: rows below it would move.
            Snap::Channel { row, ch } => match ch {
                None if row + 1 == s.project.channels.len() => crate::rack::remove_last(s),
                Some(c) if row == s.project.channels.len() => crate::rack::push_channel(s, *c),
                _ => {}
            },
            Snap::Init(t, e) => crate::init::put(s, t, e),
            Snap::Patterns { patterns, current } => {
                s.project.patterns = patterns;
                s.pattern = current;
                s.piano.clear_selection();
            }
            Snap::Rack(r) => put_rack(s, *r),
            // The channel back: its generator is hosted again (Lemon Sampler from the channel's settings).
            Snap::Generator(r, g) => {
                let (row, back) = (r.row, r.ch.is_some() && r.row == s.project.channels.len());
                put_rack(s, *r);
                if let Some(index) = g.filter(|_| back) {
                    s.pending_plugin = Some((row, index));
                }
            }
            Snap::Rows(r) => r.put(s),
            // The inserted row back: its generator is hosted again (Lemon Sampler from the channel's settings).
            Snap::Insert(r, g) => {
                let (full, row) = (r.full, r.gone.first().copied());
                r.put(s);
                if let (Some(index), Some(row)) = (g.filter(|_| full), row) {
                    s.pending_plugin = Some((row, index));
                }
            }
            Snap::Replace(r) => put_replaced(s, *r),
            Snap::Timebase(t) => {
                let Timebase { ppq, patterns, arrangements } = *t;
                let from = s.project.ppq.max(1);
                crate::app::retime(s, from, ppq.max(1));
                (s.project.patterns, s.project.arrangements) = (patterns, arrangements);
                // Playback stops on the next tick, as for the change (`App::set_timebase`).
                s.retimed = true;
            }
            Snap::Info(v) => {
                s.project.info = *v;
                s.settings.info_edit = None;
            }
            Snap::Settings(v) => {
                let p = &mut s.project;
                let crossfades = p.auto_crossfades != v.auto_crossfades;
                (p.bar, p.bar_as_signature, p.panning_law) = (v.bar, v.bar_as_signature, v.panning_law);
                (p.play_truncated, p.auto_crossfades) = (v.play_truncated, v.auto_crossfades);
                if crossfades {
                    crate::playlist::crossfades_switched(s);
                }
            }
            // Rows past `from` go, then the step's own rows come back (the rack never goes empty).
            Snap::Tail(t) => {
                let Tail { from, chans, groups, filter, unsorted_at, sel, selected } = *t;
                while s.project.channels.len() > from.max(1) {
                    crate::rack::remove_last(s);
                }
                if s.project.channels.len() == from {
                    chans.into_iter().for_each(|c| crate::rack::push_channel(s, c));
                }
                (s.rack.groups, s.rack.filter, s.rack.unsorted_at) = (groups, filter, unsorted_at);
                (s.rack.sel, s.selected_channel) = (sel, selected);
            }
            Snap::Several(v) => v.into_iter().rev().for_each(|x| x.put(s)),
        }
        s.piano.dirty = true;
    }
}

/// Puts the rack's side `r` back (`Snap::Rack`, `Snap::Generator`).
fn put_rack(s: &mut State, r: RackSide) {
    let RackSide { row, ch, groups, filter, unsorted_at, sel, selected, source, picker } = r;
    match ch {
        None if row + 1 == s.project.channels.len() => crate::rack::remove_last(s),
        Some(c) if row == s.project.channels.len() => crate::rack::push_channel(s, c),
        _ => {}
    }
    (s.rack.groups, s.rack.filter, s.rack.unsorted_at) = (groups, filter, unsorted_at);
    (s.rack.sel, s.selected_channel) = (sel, selected);
    (s.playlist.source, s.playlist.picker) = (source, picker);
}

/// Puts Replace's side `r` back (`Snap::Replace`): the channel, and on the next tick its instrument (`State::rows_moved`
/// drops the row's instance and hosts a saved one; `State::pending_plugin` one from its defaults).
fn put_replaced(s: &mut State, r: Replaced) {
    let Replaced { row, ch, host, .. } = r;
    let Some(c) = s.project.channels.get_mut(row) else { return };
    *c = ch;
    s.hosted_rows.retain(|&x| x != row);
    s.sample_loads.retain(|&(x, _)| x != row);
    if s.pending_plugin.is_some_and(|(x, _)| x == row) {
        s.pending_plugin = None;
    }
    let map = (0..s.project.channels.len()).map(|x| (x != row).then_some(x)).collect();
    let back = match &host {
        Some((index, Some(blob))) => vec![(row, *index, blob.clone())],
        _ => Vec::new(),
    };
    s.rows_moved.push((map, back));
    if let Some((index, None)) = host {
        s.pending_plugin = Some((row, index));
    }
    s.selected_channel = row;
    s.rack.select_only(row);
}

/// The model's value of a knob: the channels' volume and pan as stored (exact), the others as the knob reads them.
fn knob_value(s: &State, id: KnobId) -> f64 {
    match id {
        KnobId::ChanVol(i) => s.project.channels.get(i).map_or(0.0, |c| c.volume),
        KnobId::ChanPan(i) => s.project.channels.get(i).map_or(0.0, |c| c.pan),
        _ => f64::from(s.knob(id)),
    }
}

fn set_knob_value(s: &mut State, id: KnobId, v: f64) {
    match id {
        KnobId::ChanVol(i) => s.project.channels.get_mut(i).into_iter().for_each(|c| c.volume = v),
        KnobId::ChanPan(i) => s.project.channels.get_mut(i).into_iter().for_each(|c| c.pan = v),
        _ => s.set_knob(id, v as f32),
    }
}

/// A knob step's name.
fn knob_name(id: KnobId) -> &'static str {
    match id {
        KnobId::ChanVol(_) => "Channel volume",
        KnobId::ChanPan(_) => "Channel panning",
        KnobId::MasterVol => "Main volume",
        KnobId::MasterPitch => "Main pitch",
        KnobId::MixPan(_) => "Mixer track panning",
        KnobId::MixSep(_) => "Mixer track stereo separation",
        KnobId::SlotMix(..) => "Effect mix level",
        KnobId::Send(..) => "Send level",
        KnobId::Swing => "Main swing",
        KnobId::Eq(..) => "Mixer track equalizer",
        KnobId::Strum(_) => "Piano roll strum",
        KnobId::Arp(_) => "Piano roll arpeggiate",
        KnobId::PianoDlg(_) => "Piano roll tool",
    }
}

#[derive(Debug, Clone)]
struct Step {
    name: &'static str,
    snap: Snap,
}

/// The project's undo history (`State::history`).
#[derive(Debug, Default)]
pub struct History {
    steps: Vec<Step>,
    /// Steps `..done` are done; the rest are undone (they can be redone).
    done: usize,
    /// The last move was an undo: with Alternate undo mode off, Ctrl+Z redoes it.
    undid: bool,
    /// The knob or fader the newest step's mouse-wheel turns go to (one step for a run of notches).
    wheel: Option<crate::knobs::Control>,
    /// The newest step was recorded since the last button release (`settle` looks at it then).
    fresh: bool,
}

impl History {
    /// How many steps there are ("Last reset" not counted).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// How many steps are done (undone ones not counted).
    pub fn done(&self) -> usize {
        self.done
    }

    /// The name of the newest done step.
    pub fn newest_done(&self) -> Option<&'static str> {
        self.done.checked_sub(1).map(|i| self.steps[i].name)
    }

    /// Forgets every step: the project was opened or made ("Last reset").
    pub fn reset(&mut self) {
        *self = History::default();
    }

    /// Whether entry `p` of the list (0 = "Last reset", `k` = step `k`) is undone: drawn grey.
    pub fn undone(&self, p: usize) -> bool {
        p > self.done
    }

    /// Whether entry `p` is where the project is now.
    pub fn current(&self, p: usize) -> bool {
        p == self.done
    }

    /// The entry where the project is now (0 = "Last reset").
    pub fn at(&self) -> usize {
        self.done
    }

    /// The list as the browser's History folder shows it: (entry, name), oldest first from "Last reset", or newest
    /// first with Show recent changes at the top.
    pub fn entries(&self) -> Vec<(usize, &'static str)> {
        let mut v: Vec<(usize, &'static str)> = std::iter::once(LAST_RESET).chain(self.steps.iter().map(|st| st.name)).enumerate().collect();
        if prefs::check(&prefs::RECENT_CHANGES_TOP) {
            v.reverse();
        }
        v
    }

    /// Drops the oldest steps past Maximum undo levels (undone steps last).
    fn trim(&mut self) {
        let max = usize::from(prefs::number(&prefs::MAX_UNDO_LEVELS));
        while self.steps.len() > max && self.done > 0 {
            self.steps.remove(0);
            self.done -= 1;
        }
        self.steps.truncate(max.max(self.done));
    }

    /// What Ctrl+Z does next: redo (Alternate undo mode off, after an undo) or undo.
    fn ctrl_z_redoes(&self) -> bool {
        !prefs::check(&prefs::ALTERNATE_UNDO) && self.undid && self.done < self.steps.len()
    }
}

/// Records step `name`: `snap` holds what the edit is about to change. Undone steps are dropped (a new action ends
/// their branch), and the oldest step past Maximum undo levels.
pub fn record(s: &mut State, name: &'static str, snap: Snap) {
    let h = &mut s.history;
    h.steps.truncate(h.done);
    h.steps.push(Step { name, snap });
    h.done = h.steps.len();
    h.undid = false;
    h.wheel = None;
    h.fresh = true;
    h.trim();
}

/// Names the newest step `name` while its press is held (recorded since the last release): a gesture that turned into
/// another edit, as a held click's clone (`crate::hold`). The step keeps what it holds.
pub fn rename(s: &mut State, name: &'static str) {
    let h = &mut s.history;
    if h.fresh && h.done > 0 && h.done == h.steps.len() {
        h.steps[h.done - 1].name = name;
    }
}

/// Drops the newest step: its edit changed nothing after all.
pub fn unrecord(s: &mut State) {
    let h = &mut s.history;
    if h.done > 0 && h.done == h.steps.len() {
        h.steps.pop();
        h.done -= 1;
    }
    (h.wheel, h.fresh) = (None, false);
}

/// A button was released: the step recorded since the last release goes when its edit changed nothing (a click on a
/// note that did not move it, a knob clicked without a turn).
pub fn settle(s: &mut State) {
    let h = &s.history;
    if h.fresh && h.done > 0 && h.done == h.steps.len() && h.steps[h.done - 1].snap.now(s) == h.steps[h.done - 1].snap {
        unrecord(s);
    }
    s.history.fresh = false;
}

/// Before a sample loads into channel `row` (the file dialog, a drop, the browser's send): one step, "Load channel
/// sample(s)", holding what the load changes. `row` past the last channel: the load makes that channel.
pub fn load_sample(s: &mut State, row: usize) {
    let snap = Snap::sample(s, row).unwrap_or(Snap::Channel { row: s.project.channels.len(), ch: None });
    record(s, LOAD_SAMPLE, snap);
}

/// Before generator `plugin` (`None`: Lemon Sampler) is added at the bottom of the rack (the plugin picker, F8, Add
/// one ▸, a generator dropped from the browser): one step, "Add channel"; undone, the channel and its instance go.
pub fn add_channel(s: &mut State, plugin: Option<usize>) {
    let side = RackSide::now(s, s.project.channels.len());
    record(s, ADD_CHANNEL, Snap::Generator(Box::new(side), plugin));
}

/// Before channel rows `gone` are deleted (the channel menu's Delete..., Delete on the rack, Purge unused audio
/// clips): one step, "Delete channel", holding the rack and `back`, the rows' instruments (row, `Plugins::available`
/// index, saved state); undone, the channels, their notes and clips and their instruments come back.
pub fn delete_channels(s: &mut State, mut gone: Vec<usize>, back: Vec<(usize, usize, Vec<u8>)>) {
    gone.sort_unstable();
    gone.dedup();
    let snap = Snap::Rows(Box::new(Rows::now(s, true, gone, back)));
    record(s, DELETE_CHANNEL, snap);
}

/// Before generator `plugin` (`None`: Lemon Sampler) is inserted as rack row `row`, above the last (a channel's Insert
/// ▸): one step, "Add channel", holding the rack as it is; undone, the channel and its instance go and the rows under
/// it move back up, redone, it comes back and its generator starts from its defaults.
pub fn insert_channel(s: &mut State, row: usize, plugin: Option<usize>) {
    let snap = Snap::Insert(Box::new(Rows::now(s, false, vec![row], Vec::new())), plugin);
    record(s, ADD_CHANNEL, snap);
}

/// Before Replace ▸ puts generator `plugin` (`None`: Lemon Sampler) on rack row `row`: one step, "Replace channel",
/// holding the channel and `old`, its instrument as Replace finds it (`Plugins::available` index, saved state).
/// Undone, the channel and its old instrument come back; redone, the new generator starts again from its defaults.
pub fn replace_channel(s: &mut State, row: usize, old: Option<(usize, Vec<u8>)>, plugin: Option<usize>) {
    let Some(ch) = s.project.channels.get(row).cloned() else { return };
    let (host, other) = (old.map(|(i, blob)| (i, Some(blob))), plugin.map(|i| (i, None)));
    record(s, REPLACE_CHANNEL, Snap::Replace(Box::new(Replaced { row, ch, host, other })));
}

/// Before the timebase changes (Settings > Project, a new project's Save as form): one step, "Project settings",
/// holding the timebase and the project's ticks, so an undo brings back the ticks exactly.
pub fn timebase(s: &mut State) {
    record(s, PROJECT_SETTINGS, Snap::Timebase(Box::new(Timebase::now(s))));
}

/// After Settings > Info's text changed from `before` (a box's accepted edit, the genre menu, a new project's Save as
/// form): one step, "Project info", unless it stays the same.
pub fn project_info(s: &mut State, before: os_model::ProjectInfo) {
    if s.project.info != before {
        record(s, PROJECT_INFO, Snap::Info(Box::new(before)));
    }
}

/// Before Settings > Project changes a value (`Settings`): one step, "Project settings". With Automatic crossfades,
/// which sets the clips' crossfades, the step holds the playlist too.
pub fn project_settings(s: &mut State, crossfades: bool) {
    let snap = Snap::Settings(Settings::now(s));
    if crossfades {
        crate::playlist::snapshot_with(s, PROJECT_SETTINGS, snap);
    } else {
        record(s, PROJECT_SETTINGS, snap);
    }
}

/// Before mixer track `track`'s volume fader moves (a drag, a typed value, the wheel): a step, "Mixer track volume",
/// unless Undo knob tweaks is off (a fader is a knob here; an OS choice). `wheel`: a mouse-wheel notch, which joins the
/// step of the notches just before it on the same fader.
pub fn fader(s: &mut State, track: u16, wheel: bool) {
    if !prefs::check(&prefs::UNDO_KNOB_TWEAKS) {
        return;
    }
    let me = Some(crate::knobs::Control::Fader(track));
    if wheel && s.history.wheel == me && s.history.done == s.history.steps.len() {
        return;
    }
    let Some(m) = crate::mixer::track(s, track) else { return };
    let snap = Snap::Fader(track, m.volume);
    record(s, MIXER_VOLUME, snap);
    if wheel {
        s.history.wheel = me;
    }
}

/// A mixer track's input, output, record source and monitor mode, as `Snap::TrackIo` holds them.
fn track_io_of(m: &os_model::MixerTrack) -> [i32; 4] {
    [m.audio_in, m.audio_out, i32::from(m.rec_source), i32::from(m.monitor)]
}

/// Before mixer track `track`'s audio input, output, record source or monitor mode changes: a step, "Mixer track audio
/// input / output".
pub fn track_io(s: &mut State, track: u16) {
    let Some(m) = crate::mixer::track(s, track) else { return };
    let snap = Snap::TrackIo(track, track_io_of(m));
    record(s, MIXER_TRACK_IO, snap);
}

/// After Lemon Studio set parameter `index` of plugin instance `key` from `before` to `to` (Type in value, the
/// parameter menu's Set... and Paste value): a step, "Plugin parameter", unless Undo knob tweaks is off or the value
/// stays. A plugin editor's own changes make none.
pub fn param(s: &mut State, key: usize, index: usize, before: Option<f64>, to: f64) {
    let Some(v) = before.filter(|&v| v != to && prefs::check(&prefs::UNDO_KNOB_TWEAKS)) else { return };
    record(s, PLUGIN_PARAMETER, Snap::Param(key, index, v));
    // A plugin may take the value on its next idle, so the button's release does not judge the step (`settle`).
    s.history.fresh = false;
}

/// Before knob `id` turns: a step unless Undo knob tweaks is off. `wheel`: a mouse-wheel notch, which joins the step
/// of the notches just before it on the same knob.
pub fn knob(s: &mut State, id: KnobId, wheel: bool) {
    // The piano roll tool dialogs' knobs only preview; their Accept is the step (spec: ui/piano-strum.md §1).
    if !prefs::check(&prefs::UNDO_KNOB_TWEAKS) || matches!(id, KnobId::Strum(_) | KnobId::Arp(_) | KnobId::PianoDlg(_)) {
        return;
    }
    let h = &s.history;
    if wheel && h.wheel == Some(crate::knobs::Control::Knob(id)) && h.done == h.steps.len() {
        return;
    }
    let snap = Snap::Knob(id, knob_value(s, id));
    record(s, knob_name(id), snap);
    if wheel {
        s.history.wheel = Some(crate::knobs::Control::Knob(id));
    }
}

fn swap(s: &mut State, i: usize) {
    let now = s.history.steps[i].snap.now(s);
    let old = std::mem::replace(&mut s.history.steps[i].snap, now);
    old.put(s);
}

/// Undoes the newest done step; its name, or `None` when nothing is done.
pub fn undo(s: &mut State) -> Option<&'static str> {
    let h = &mut s.history;
    if h.done == 0 {
        return None;
    }
    h.done -= 1;
    (h.undid, h.wheel, h.fresh) = (true, None, false);
    let i = h.done;
    swap(s, i);
    Some(s.history.steps[i].name)
}

/// Redoes the oldest undone step; its name, or `None` when nothing is undone.
pub fn redo(s: &mut State) -> Option<&'static str> {
    let h = &mut s.history;
    if h.done == h.steps.len() {
        return None;
    }
    let i = h.done;
    swap(s, i);
    let h = &mut s.history;
    h.done += 1;
    (h.undid, h.wheel, h.fresh) = (false, None, false);
    Some(h.steps[i].name)
}

/// "Step seq edit" as a menu entry writes it after Undo / Redo: "step seq edit".
fn lower(name: &str) -> String {
    let mut c = name.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

fn did(s: &mut State, redo_it: bool) -> String {
    let (verb, name) = if redo_it { ("Redo", redo(s)) } else { ("Undo", undo(s)) };
    match name {
        Some(n) => format!("{verb}: {}", lower(n)),
        None => format!("Nothing to {}", verb.to_lowercase()),
    }
}

/// Ctrl+Z (and EDIT's first entry): with Alternate undo mode on, one more step back; off, it toggles the last move
/// (undo, redo, undo). With Show undo history in the browser when undoing changes on, the browser shows the history.
/// Returns the hint bar's message.
pub fn ctrl_z(s: &mut State) -> Option<String> {
    let m = did(s, s.history.ctrl_z_redoes());
    shown_in_browser(s);
    Some(m)
}

/// Ctrl+Alt+Z (and EDIT's Redo): with Alternate undo mode on, redo; off, one more step back.
pub fn ctrl_alt_z(s: &mut State) -> Option<String> {
    let m = did(s, prefs::check(&prefs::ALTERNATE_UNDO));
    shown_in_browser(s);
    Some(m)
}

/// Ctrl+Shift+Z (Command+Shift+Z, macOS's usual redo): redo, in either mode. A Lemon Studio addition: FL redoes with
/// Ctrl+Alt+Z only.
pub fn ctrl_shift_z(s: &mut State) -> String {
    let m = did(s, true);
    shown_in_browser(s);
    m
}

fn shown_in_browser(s: &mut State) {
    if prefs::check(&prefs::UNDO_HISTORY_IN_BROWSER) {
        crate::browser::show_history(s);
    }
}

/// A click on entry `p` of the browser's History folder: undoes or redoes up to it.
pub fn go_to(s: &mut State, p: usize) -> Option<String> {
    let p = p.min(s.history.steps.len());
    while s.history.done > p {
        undo(s);
    }
    while s.history.done < p {
        redo(s);
    }
    let name = if p == 0 { LAST_RESET } else { s.history.steps[p - 1].name };
    Some(format!("History: {}", lower(name)))
}

/// The EDIT menu's entries (spec: ui/menus.md "EDIT"): the first says what Ctrl+Z does next ("Undo step seq edit",
/// "Redo step seq edit" with Alternate undo mode off after an undo), `redo` the second (Ctrl+Alt+Z's redo, Alternate
/// undo mode only). Without a step to act on, plain "Undo" / "Redo".
pub fn edit_label(s: &State, redo_entry: bool) -> String {
    let h = &s.history;
    let redo_it = redo_entry || h.ctrl_z_redoes();
    let (verb, step) = if redo_it { ("Redo", h.steps.get(h.done)) } else { ("Undo", h.done.checked_sub(1).and_then(|i| h.steps.get(i))) };
    match step {
        Some(st) => format!("{verb} {}", lower(st.name)),
        None => verb.into(),
    }
}

/// Whether EDIT's entry has a step to act on.
pub fn edit_enabled(s: &State, redo_entry: bool) -> bool {
    let h = &s.history;
    if redo_entry || h.ctrl_z_redoes() { h.done < h.steps.len() } else { h.done > 0 }
}

/// Whether EDIT shows its Redo entry: Alternate undo mode on and a history to act on (only Undo while it is empty).
pub fn redo_shown(s: &State) -> bool {
    prefs::check(&prefs::ALTERNATE_UNDO) && !s.history.steps.is_empty()
}

/// Maximum undo levels changed: the history keeps that many.
pub fn trim(s: &mut State) {
    s.history.trim();
}

/// Another arrangement is shown: the playlist's steps belong to the one left (spec: ui/playlist.md §10).
pub fn forget_playlist(s: &mut State) {
    let h = &mut s.history;
    let mut done = 0;
    let mut kept = Vec::with_capacity(h.steps.len());
    for (i, st) in std::mem::take(&mut h.steps).into_iter().enumerate() {
        if !st.snap.has_playlist() {
            done += usize::from(i < h.done);
            kept.push(st);
        }
    }
    (h.steps, h.done, h.wheel) = (kept, done, None);
}

/// The timebase changed from `from` to `to` ticks per quarter: the steps' notes, control points, lengths, clips and
/// markers follow, so no undo brings back ticks of the old timebase (a timebase step keeps its own, `Timebase`).
pub fn rescale(s: &mut State, from: u16, to: u16) {
    for st in &mut s.history.steps {
        st.snap.rescale(from, to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn note_count(s: &State) -> usize {
        s.project.pattern(s.pattern).map_or(0, |p| p.notes.len())
    }

    /// Two recorded edits: undo and redo swap them back and forth; a new action drops the undone branch; past
    /// Maximum undo levels the oldest step goes (spec: ui/settings-choices.md "Undo history: the four checks").
    #[test]
    fn steps_swap_branch_and_limit() {
        let mut app = App::new(None);
        let s = &mut app.state;
        let start = note_count(s);
        for k in 0..2 {
            let snap = Snap::pattern(s, s.pattern);
            record(s, STEP_SEQ_EDIT, snap);
            crate::rack::set_step(s, 0, 10 + k, true);
        }
        assert_eq!(note_count(s), start + 2);
        assert_eq!((undo(s), note_count(s)), (Some(STEP_SEQ_EDIT), start + 1));
        assert_eq!((undo(s), note_count(s)), (Some(STEP_SEQ_EDIT), start));
        assert_eq!(undo(s), None);
        assert_eq!((redo(s), note_count(s)), (Some(STEP_SEQ_EDIT), start + 1));
        assert!(s.history.undone(2) && s.history.current(1) && !s.history.undone(1));
        // A new action: the undone step is gone.
        let snap = Snap::pattern(s, s.pattern);
        record(s, "Piano roll add note", snap);
        crate::rack::set_step(s, 0, 14, true);
        assert_eq!(s.history.entries().iter().map(|e| e.1).collect::<Vec<_>>(), [LAST_RESET, STEP_SEQ_EDIT, "Piano roll add note"]);
        assert_eq!(redo(s), None);
        // Three levels kept: the oldest go.
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 3);
        for k in 0..3 {
            let snap = Snap::pattern(s, s.pattern);
            record(s, "Piano roll edit", snap);
            crate::rack::set_step(s, 0, 20 + k, true);
        }
        assert_eq!(s.history.len(), 3);
        assert!(s.history.entries().iter().skip(1).all(|e| e.1 == "Piano roll edit"), "{:?}", s.history.entries());
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 2);
        trim(s);
        assert_eq!((s.history.len(), s.history.done), (2, 2));
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, prefs::MAX_UNDO_LEVELS.default);
    }

    /// A step whose edit changed nothing is dropped when the button comes up (a step of an earlier press stays); a
    /// knob's wheel notches make one step.
    #[test]
    fn settle_and_wheel_runs() {
        let mut app = App::new(None);
        let s = &mut app.state;
        let snap = Snap::pattern(s, s.pattern);
        record(s, "Piano roll edit", snap);
        settle(s);
        assert_eq!(s.history.len(), 0, "nothing changed");
        let snap = Snap::pattern(s, s.pattern);
        record(s, "Piano roll edit", snap);
        crate::rack::set_step(s, 0, 9, !crate::rack::row_steps(s, 0, 10)[9]);
        settle(s);
        crate::rack::set_step(s, 0, 9, !crate::rack::row_steps(s, 0, 10)[9]);
        settle(s);
        assert_eq!(s.history.len(), 1, "an earlier press's step stays");
        s.history.reset();
        let id = KnobId::ChanVol(0);
        let v0 = s.project.channels[0].volume;
        for _ in 0..3 {
            knob(s, id, true);
            let v = s.knob(id) - 0.03;
            s.set_knob(id, v);
        }
        assert_eq!(s.history.len(), 1, "one step for the notches");
        assert_eq!(undo(s), Some("Channel volume"));
        assert_eq!(s.project.channels[0].volume, v0, "exactly back");
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Through the app, as the spec measured it: two step-sequencer clicks (row 0 step 1, row 1 step 2), Ctrl+Z,
    // Ctrl+Alt+Z, the first channel's volume knob and the EDIT menu (spec: ui/settings-choices.md "Undo history: the
    // four checks (2026-10-16)").

    use crate::app::Action;
    use crate::icons::Icon;
    use crate::views::browser::Item;
    use crate::views::menus::{bar_model, EDIT_MENU};
    use winit::keyboard::Key;

    /// A click on step `k` of rack row `row`: press and release.
    fn click_step(app: &mut App, row: usize, k: u32) {
        app.act(Action::Step(row, k));
        app.mouse_up();
    }

    /// Rack rows 0 and 1, their first steps.
    fn steps(app: &App) -> [Vec<bool>; 2] {
        [0, 1].map(|r| crate::rack::row_steps(&app.state, r, 4))
    }

    fn ctrl_z(app: &mut App) {
        app.key(&Key::Character("z".into()), true, false);
    }

    fn ctrl_alt_z(app: &mut App) {
        app.key(&Key::Character("z".into()), true, true);
    }

    fn ctrl_shift_z(app: &mut App) {
        app.shift = true;
        app.key(&Key::Character("Z".into()), true, false);
        app.shift = false;
    }

    /// A drag on knob `id`, `dy` pixels down.
    fn turn(app: &mut App, id: KnobId, dy: f32) {
        app.mouse_move(100.0, 300.0);
        app.act(Action::Knob(id));
        app.mouse_move(100.0, 300.0 + dy);
        app.mouse_up();
    }

    /// EDIT as it shows now: (entry, shortcut, can be chosen).
    fn edit_menu(app: &App) -> Vec<(String, &'static str, bool)> {
        bar_model(&app.state)[EDIT_MENU].rows.iter().map(|r| (r.label.clone(), r.shortcut, r.enabled)).collect()
    }

    fn entry(label: &str, shortcut: &'static str, enabled: bool) -> (String, &'static str, bool) {
        (label.into(), shortcut, enabled)
    }

    /// The browser's Current project ▸ History, opened: (name, mirrored arrow, drawn grey, where the project is).
    fn history_rows(app: &mut App) -> Vec<(String, bool, bool, bool)> {
        app.state.browser_tab = 1;
        app.state.browser_open.extend(["1:Current project".to_string(), "1:Current project/History".to_string()]);
        let h = &app.state.history;
        crate::views::browser::rows(&app.state)
            .into_iter()
            .filter_map(|r| match r.item {
                Item::History(e) => Some((r.label, r.icon == Icon::Redo, h.undone(e), r.bright)),
                _ => None,
            })
            .collect()
    }

    /// Alternate undo mode on (classic): each Ctrl+Z steps one further back, the third does nothing; Ctrl+Alt+Z
    /// redoes; EDIT shows "Undo step seq edit  Ctrl+Z" and "Redo step seq edit  Ctrl+Alt+Z" (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Alternate undo mode On).
    #[test]
    fn alternate_mode_on_steps_back_and_redoes() {
        let mut app = App::new(None);
        assert!(prefs::check(&prefs::ALTERNATE_UNDO), "on by default");
        let start = steps(&app);
        click_step(&mut app, 0, 0);
        let one = steps(&app);
        click_step(&mut app, 1, 1);
        let two = steps(&app);
        assert!(start != one && one != two);
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true), entry("Redo", "Ctrl+Alt+Z", false)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "the second click undone");
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true), entry("Redo step seq edit", "Ctrl+Alt+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), start, "then the first");
        assert_eq!(edit_menu(&app), [entry("Undo", "Ctrl+Z", false), entry("Redo step seq edit", "Ctrl+Alt+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!((steps(&app), app.state.message.as_deref()), (start.clone(), Some("Nothing to undo")), "a third does nothing");
        ctrl_alt_z(&mut app);
        assert_eq!(steps(&app), one, "Ctrl+Alt+Z redoes");
        // EDIT's Redo does what Ctrl+Alt+Z does.
        app.menu_item(EDIT_MENU, 1);
        assert_eq!(steps(&app), two);
        app.menu_item(EDIT_MENU, 0);
        assert_eq!(steps(&app), one);
    }

    /// Alternate undo mode off: Ctrl+Z three times undoes the second click, redoes it and undoes it again; Ctrl+Alt+Z
    /// then undoes the first. EDIT has one entry naming what Ctrl+Z does next, "Redo step seq edit  Ctrl+Z" after an
    /// undo (spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Alternate undo mode Off).
    #[test]
    fn alternate_mode_off_toggles_the_last_step() {
        let mut app = App::new(None);
        prefs::set_check(&prefs::ALTERNATE_UNDO, false);
        let start = steps(&app);
        click_step(&mut app, 0, 0);
        let one = steps(&app);
        click_step(&mut app, 1, 1);
        let two = steps(&app);
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "undid the second click");
        assert_eq!(edit_menu(&app), [entry("Redo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), two, "redid it");
        assert_eq!(edit_menu(&app), [entry("Undo step seq edit", "Ctrl+Z", true)]);
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "undid it again");
        ctrl_alt_z(&mut app);
        assert_eq!(steps(&app), start, "Ctrl+Alt+Z steps back further: the first click undone");
        ctrl_z(&mut app);
        assert_eq!(steps(&app), one, "Ctrl+Z toggles that back");
        // EDIT's one entry does what Ctrl+Z does.
        app.menu_item(EDIT_MENU, 0);
        assert_eq!(steps(&app), start);
        prefs::set_check(&prefs::ALTERNATE_UNDO, true);
    }

    /// Ctrl+Shift+Z (Command+Shift+Z on macOS) redoes in either mode, where Ctrl+Z alone would undo or toggle (a Lemon
    /// Studio addition; spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn ctrl_shift_z_redoes_in_either_mode() {
        for alternate in [true, false] {
            let mut app = App::new(None);
            prefs::set_check(&prefs::ALTERNATE_UNDO, alternate);
            click_step(&mut app, 0, 0);
            let one = steps(&app);
            click_step(&mut app, 1, 1);
            let two = steps(&app);
            ctrl_z(&mut app);
            assert_eq!(steps(&app), one, "alternate undo mode {alternate}: undone");
            ctrl_shift_z(&mut app);
            assert_eq!(steps(&app), two, "alternate undo mode {alternate}: redone");
            ctrl_shift_z(&mut app);
            let nothing = (two.clone(), Some("Nothing to redo"));
            assert_eq!((steps(&app), app.state.message.as_deref()), nothing, "alternate undo mode {alternate}");
        }
        prefs::set_check(&prefs::ALTERNATE_UNDO, true);
    }

    /// EDIT with an empty history: only Undo, greyed out (spec: ui/menus.md "EDIT"; ui/settings-choices.md "Undo
    /// history: the four checks (2026-10-16)").
    #[test]
    fn edit_menu_with_no_history() {
        let app = App::new(None);
        assert_eq!(edit_menu(&app), [entry("Undo", "Ctrl+Z", false)]);
    }

    /// Undo knob tweaks: on, a turn of a channel knob is a step and Ctrl+Z puts the knob back, the click before it
    /// staying; off, the turn makes no step and Ctrl+Z leaves the knob, undoing the click made before it (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Undo knob tweaks).
    #[test]
    fn knob_tweaks_on_and_off() {
        let id = KnobId::ChanVol(0);
        for on in [true, false] {
            let mut app = App::new(None);
            prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, on);
            let start = steps(&app);
            click_step(&mut app, 0, 0);
            let clicked = steps(&app);
            let v0 = app.state.project.channels[0].volume;
            turn(&mut app, id, 40.0);
            let v1 = app.state.project.channels[0].volume;
            assert_ne!(v1, v0, "the knob turned");
            assert_eq!(app.state.history.len(), if on { 2 } else { 1 });
            ctrl_z(&mut app);
            let vol = app.state.project.channels[0].volume;
            if on {
                assert_eq!((vol, steps(&app)), (v0, clicked), "the knob back, the click kept");
            } else {
                assert_eq!((vol, steps(&app)), (v1, start), "the knob kept, the click undone");
            }
        }
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, true);
    }

    /// The browser's History: "Last reset" then the steps, oldest first; undone steps grey with the mirrored arrow; a
    /// new action drops them; newest first with Show recent changes at the top; a click on an entry goes there (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)", Show undo history in the browser, Show
    /// recent changes at the top).
    #[test]
    fn history_folder_lists_greys_and_drops() {
        let mut app = App::new(None);
        let start = steps(&app);
        assert_eq!(history_rows(&mut app), [(LAST_RESET.into(), false, false, true)]);
        click_step(&mut app, 0, 0);
        click_step(&mut app, 1, 1);
        ctrl_z(&mut app);
        let seq = |undone: bool, now: bool| (STEP_SEQ_EDIT.to_string(), undone, undone, now);
        assert_eq!(history_rows(&mut app), [(LAST_RESET.into(), false, false, false), seq(false, true), seq(true, false)]);
        // Newest first: "Last reset" at the bottom.
        prefs::set_check(&prefs::RECENT_CHANGES_TOP, true);
        assert_eq!(history_rows(&mut app), [seq(true, false), seq(false, true), (LAST_RESET.into(), false, false, false)]);
        prefs::set_check(&prefs::RECENT_CHANGES_TOP, false);
        // A new action (a knob turn) drops the undone step: nothing to redo.
        turn(&mut app, KnobId::ChanVol(0), 40.0);
        let names: Vec<String> = history_rows(&mut app).into_iter().map(|r| r.0).collect();
        assert_eq!(names, [LAST_RESET, STEP_SEQ_EDIT, "Channel volume"]);
        assert!(history_rows(&mut app).iter().all(|r| !r.2), "none undone");
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.message.as_deref(), Some("Nothing to redo"));
        // A click on "Last reset" undoes everything; on the newest entry, redoes it all.
        let rows = crate::views::browser::rows(&app.state);
        let at = |e: usize| rows.iter().position(|r| r.item == Item::History(e)).expect("history row");
        let (first, last) = (at(0), at(2));
        app.act(Action::BrowserRow(first));
        app.mouse_up();
        assert_eq!(steps(&app), start);
        assert!(app.state.history.undone(1) && app.state.history.undone(2) && app.state.history.current(0));
        assert!(app.drag.is_none(), "a history entry is not dragged");
        app.act(Action::BrowserRow(last));
        app.mouse_up();
        assert!(app.state.history.current(2));
    }

    /// Show undo history in the browser when undoing changes: off, Ctrl+Z leaves the browser where it is; on, the
    /// browser shows Current project ▸ History, opened (spec: ui/settings-choices.md "Undo history: the four checks
    /// (2026-10-16)").
    #[test]
    fn ctrl_z_shows_the_history_in_the_browser() {
        let mut app = App::new(None);
        click_step(&mut app, 0, 0);
        click_step(&mut app, 1, 1);
        app.state.show_browser = false;
        app.state.browser_tab = 2;
        let open = app.state.browser_open.clone();
        ctrl_z(&mut app);
        assert_eq!((app.state.show_browser, app.state.browser_tab, &app.state.browser_open), (false, 2, &open), "off by default");
        prefs::set_check(&prefs::UNDO_HISTORY_IN_BROWSER, true);
        ctrl_z(&mut app);
        assert!(app.state.show_browser);
        assert_eq!(app.state.browser_tab, 1);
        assert!(app.state.browser_open.contains("1:Current project/History"));
        assert_eq!(app.state.browser_sel.as_deref(), Some("1:Current project/History/0"), "where the project now is");
        prefs::set_check(&prefs::UNDO_HISTORY_IN_BROWSER, false);
    }

    /// Maximum undo levels: past it the oldest steps go, so those edits stay; the General page shows the value (spec:
    /// ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn max_levels_drop_the_oldest() {
        let mut app = App::new(None);
        assert_eq!(prefs::number(&prefs::MAX_UNDO_LEVELS), 100, "100 by default");
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, 3);
        let shown = crate::views::settings::page(crate::views::settings::GENERAL, &app.state)
            .into_iter()
            .flat_map(|g| g.rows)
            .any(|r| matches!(r, crate::views::settings::Row::Value(v, "Maximum undo levels") if v == "3"));
        assert!(shown, "the General page shows the value");
        let start = crate::rack::row_steps(&app.state, 0, 5);
        for k in 0..5 {
            click_step(&mut app, 0, k);
        }
        assert_eq!(app.state.history.len(), 3);
        for _ in 0..4 {
            ctrl_z(&mut app);
        }
        let now = crate::rack::row_steps(&app.state, 0, 5);
        assert_eq!(now[..2], start[..2].iter().map(|b| !b).collect::<Vec<_>>()[..], "the two oldest clicks can no longer be undone");
        assert_eq!(now[2..], start[2..]);
        prefs::set_number(&prefs::MAX_UNDO_LEVELS, prefs::MAX_UNDO_LEVELS.default);
    }

    /// A drag on mixer track `t`'s volume fader, `dy` pixels down.
    fn fade(app: &mut App, t: u16, dy: f32) {
        app.mouse_move(100.0, 300.0);
        app.act(Action::Fader(t));
        app.mouse_move(100.0, 300.0 + dy);
        app.mouse_up();
    }

    fn names(s: &State) -> Vec<&'static str> {
        s.history.entries().into_iter().map(|e| e.1).collect()
    }

    /// A mixer fader drag is one step, "Mixer track volume", as EDIT names it; Ctrl+Z puts the fader back exactly and
    /// Ctrl+Alt+Z redoes it. A click without a move makes none, and with Undo knob tweaks off nor does a drag (OS).
    #[test]
    fn mixer_fader_drag_is_one_step() {
        let mut app = App::new(None);
        let vol = |app: &App| crate::mixer::track(&app.state, 1).map(|m| m.volume);
        let v0 = vol(&app);
        assert!(v0.is_some(), "insert 1 exists");
        fade(&mut app, 1, -30.0);
        let v1 = vol(&app);
        assert_ne!(v1, v0, "the fader moved");
        assert_eq!(names(&app.state), [LAST_RESET, MIXER_VOLUME]);
        assert_eq!(edit_menu(&app)[0], entry("Undo mixer track volume", "Ctrl+Z", true));
        ctrl_z(&mut app);
        assert_eq!(vol(&app), v0, "exactly back");
        ctrl_alt_z(&mut app);
        assert_eq!(vol(&app), v1, "redone");
        fade(&mut app, 1, 0.0);
        assert_eq!(app.state.history.len(), 1, "a click without a move");
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, false);
        fade(&mut app, 1, 20.0);
        assert_ne!(vol(&app), v1);
        assert_eq!(app.state.history.len(), 1, "Undo knob tweaks off");
        prefs::set_check(&prefs::UNDO_KNOB_TWEAKS, true);
    }

    /// A generator added at the bottom of the rack is one step, "Add channel": undone, the channel goes; redone, it
    /// comes back and its generator is asked for again (Lemon Sampler hosts from the channel's settings).
    #[test]
    fn an_added_generator_is_one_step() {
        let mut a = App::new(None);
        a.plugins = crate::plugins::Plugins::default();
        a.plugins.available = crate::plugins::builtin();
        let n = a.state.project.channels.len();
        let sampler = a.plugins.index_of(crate::plugins::LEMON_SAMPLER).unwrap();
        let trio = a.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        crate::plugin_menu::add_generator(&mut a, sampler, None);
        crate::plugin_menu::add_generator(&mut a, trio, None);
        assert_eq!(a.state.project.channels.len(), n + 2);
        assert_eq!(names(&a.state), [LAST_RESET, ADD_CHANNEL, ADD_CHANNEL]);
        a.state.pending_plugin = None;
        assert_eq!(undo(&mut a.state), Some(ADD_CHANNEL));
        assert_eq!((a.state.project.channels.len(), a.state.selected_channel), (n + 1, n), "the Trio channel went");
        assert_eq!(redo(&mut a.state), Some(ADD_CHANNEL));
        assert_eq!(a.state.project.channels.len(), n + 2);
        assert_eq!(a.state.pending_plugin, Some((n + 1, trio)), "its generator is hosted again");
        assert_eq!(a.state.project.channels[n + 1].plugin, Some(crate::osc3::file_name(crate::osc3::LEMON_TRIO).to_string()));
        undo(&mut a.state);
        undo(&mut a.state);
        assert_eq!(a.state.project.channels.len(), n, "both gone");
        assert_eq!(a.state.pending_plugin, None);
    }

    /// A channel delete is one step, "Delete channel": undone, the channel is back in its row with its notes and the
    /// rows below move back down, and the plugin host is told (rows moved, instruments to host again); redone, it goes
    /// again (OS).
    #[test]
    fn a_channel_delete_is_one_step() {
        let mut app = App::new(None);
        assert!(app.state.project.channels.len() >= 3);
        app.state.project.patterns[0].notes = vec![os_model::Note::step(1, 0), os_model::Note::step(2, 4)];
        let (chans, pats) = (app.state.project.channels.clone(), app.state.project.patterns.clone());
        crate::rack::delete_rows(&mut app, &[1]);
        let (gone_chans, gone_pats) = (app.state.project.channels.clone(), app.state.project.patterns.clone());
        assert_eq!(gone_chans.len(), chans.len() - 1);
        assert_eq!(names(&app.state), [LAST_RESET, DELETE_CHANNEL]);
        app.state.rows_moved.clear();
        ctrl_z(&mut app);
        assert_eq!((&app.state.project.channels, &app.state.project.patterns), (&chans, &pats), "all back");
        let (map, _) = app.state.rows_moved.last().expect("the host is told");
        assert_eq!(map[..2], [Some(0), Some(2)], "row 1 moved back down to 2");
        ctrl_alt_z(&mut app);
        assert_eq!((&app.state.project.channels, &app.state.project.patterns), (&gone_chans, &gone_pats), "deleted again");
        assert_eq!(app.state.rows_moved.last().unwrap().0[..3], [Some(0), None, Some(1)]);
        app.plugin_tick();
        assert!(app.state.rows_moved.is_empty(), "the host took the moves");
    }

    /// Settings > Project: the bar, its mode and the panning law are steps, "Project settings", put back by Ctrl+Z (OS).
    #[test]
    fn project_settings_are_steps() {
        let mut app = App::new(None);
        let start = Settings::now(&app.state);
        app.set_panning_law(2);
        app.set_bar(3, 4);
        app.set_time_mode(!app.state.project.bar_is_signature());
        assert_eq!(names(&app.state), [LAST_RESET, PROJECT_SETTINGS, PROJECT_SETTINGS, PROJECT_SETTINGS]);
        app.set_panning_law(2);
        assert_eq!(app.state.history.len(), 3, "the same value again makes no step");
        assert_eq!(edit_menu(&app)[0], entry("Undo project settings", "Ctrl+Z", true));
        ctrl_z(&mut app);
        assert_eq!(app.state.project.bar(), (3, 4), "the mode back");
        ctrl_z(&mut app);
        ctrl_z(&mut app);
        assert_eq!(Settings::now(&app.state), start);
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.project.panning_law, Some(2));
    }

    /// The timebase is a "Project settings" step: undone, the ticks a lower timebase rounded come back exactly; redone,
    /// the rounded ones; the same timebase again makes no step (OS).
    #[test]
    fn a_timebase_change_is_a_step() {
        let mut app = App::new(None);
        app.set_timebase(96);
        app.state.project.patterns[0].notes = vec![os_model::Note { length: 7, ..os_model::Note::step(0, 5) }];
        let before = app.state.project.clone();
        app.set_timebase(24);
        app.set_timebase(24);
        assert_eq!(names(&app.state), [LAST_RESET, PROJECT_SETTINGS]);
        let n = app.state.project.patterns[0].notes[0];
        assert_eq!((n.position, n.length), (1, 2), "rounded to the new ticks");
        let after = app.state.project.clone();
        ctrl_z(&mut app);
        assert_eq!(app.state.project, before, "every tick back");
        assert!(app.state.retimed, "playback stops");
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.project, after);
        app.plugin_tick();
        assert!(!app.state.retimed);
    }

    /// Settings > Info: an accepted box edit and a genre picked are each one step, "Project info"; an edit that changes
    /// nothing and one dropped with Escape make none (OS).
    #[test]
    fn project_info_edits_are_steps() {
        use crate::views::settings::InfoField;
        use winit::keyboard::{Key, NamedKey};
        let mut app = App::new(None);
        let start = app.state.project.info.clone();
        let type_in = |app: &mut App, field: InfoField, text: &str| {
            app.info_act(field);
            app.state.settings.info_edit.as_mut().unwrap().text.set(text);
        };
        type_in(&mut app, InfoField::Title, "  Song ");
        crate::project_info::accept_info(&mut app.state);
        type_in(&mut app, InfoField::Title, "Song");
        crate::project_info::accept_info(&mut app.state);
        type_in(&mut app, InfoField::Author, "Someone");
        app.state.settings.info_edit = None;
        app.info_act(InfoField::Author);
        app.state.settings.open = true;
        app.state.settings.tab = crate::views::settings::INFO;
        app.info_key(&Key::Named(NamedKey::Escape), false, false);
        app.set_genre(Some(crate::project_info::GENRES[0]));
        assert_eq!(names(&app.state), [LAST_RESET, PROJECT_INFO, PROJECT_INFO]);
        assert_eq!(app.state.project.info.title.as_deref(), Some("Song"));
        ctrl_z(&mut app);
        assert_eq!(app.state.project.info.genre, start.genre, "the genre back");
        ctrl_z(&mut app);
        assert_eq!(app.state.project.info, start);
        ctrl_alt_z(&mut app);
        assert_eq!(app.state.project.info.title.as_deref(), Some("Song"));
    }

    /// A generator inserted higher up the rack is one step, "Add channel": undone, it goes and the rows under it move
    /// back up with their notes; redone, it comes back in its row and its generator is asked for again (OS).
    #[test]
    fn an_inserted_generator_is_one_step() {
        let mut a = App::new(None);
        a.plugins = crate::plugins::Plugins::default();
        a.plugins.available = crate::plugins::builtin();
        let trio = a.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        assert!(a.state.project.channels.len() >= 3);
        a.state.project.patterns[0].notes = vec![os_model::Note::step(1, 0), os_model::Note::step(2, 4)];
        let (chans, pats) = (a.state.project.channels.clone(), a.state.project.patterns.clone());
        crate::plugin_menu::add_generator(&mut a, trio, Some(0));
        let (more, moved) = (a.state.project.channels.clone(), a.state.project.patterns.clone());
        assert_eq!((more.len(), a.state.selected_channel), (chans.len() + 1, 1));
        assert_ne!(moved, pats, "the notes follow their rows");
        assert_eq!(names(&a.state), [LAST_RESET, ADD_CHANNEL]);
        (a.state.pending_plugin, a.state.rows_moved) = (None, Vec::new());
        ctrl_z(&mut a);
        assert_eq!((&a.state.project.channels, &a.state.project.patterns), (&chans, &pats), "all as before");
        assert_eq!(a.state.rows_moved.last().unwrap().0[..3], [Some(0), None, Some(1)], "the instance goes");
        assert_eq!(a.state.pending_plugin, None);
        ctrl_alt_z(&mut a);
        assert_eq!((&a.state.project.channels, &a.state.project.patterns), (&more, &moved), "inserted again");
        assert_eq!(a.state.rows_moved.last().unwrap().0[..2], [Some(0), Some(2)]);
        assert_eq!(a.state.pending_plugin, Some((1, trio)), "its generator is hosted again");
        a.state.pending_plugin = None;
        a.plugin_tick();
        assert!(a.state.rows_moved.is_empty());
    }

    /// Replace is one step, "Replace channel": undone, the channel and its old instrument with the state Replace found
    /// come back; redone, the new generator starts from its defaults; undone again, the old state once more (OS).
    #[test]
    fn replace_is_one_step() {
        let mut a = App::new(None);
        a.plugins = crate::plugins::Plugins::default();
        a.plugins.available = crate::plugins::builtin();
        let trio = a.plugins.index_of(crate::osc3::LEMON_TRIO).unwrap();
        let old = a.state.project.channels[0].clone();
        let back: Vec<_> = a.plugin_copy(0).map(|(i, blob)| (0, i, blob)).into_iter().collect();
        crate::plugin_menu::replace_generator(&mut a, trio, 0);
        let new = a.state.project.channels[0].clone();
        assert_ne!(new, old);
        assert_eq!(names(&a.state), [LAST_RESET, REPLACE_CHANNEL]);
        (a.state.pending_plugin, a.state.rows_moved) = (None, Vec::new());
        ctrl_z(&mut a);
        assert_eq!(a.state.project.channels[0], old);
        let map: Vec<Option<usize>> = (0..a.state.project.channels.len()).map(|r| (r > 0).then_some(r)).collect();
        assert_eq!(a.state.rows_moved.last().unwrap(), &(map, back), "the new instance goes, the old one comes back");
        assert_eq!(a.state.pending_plugin, None);
        ctrl_alt_z(&mut a);
        assert_eq!(a.state.project.channels[0], new);
        assert_eq!((a.state.pending_plugin, a.state.rows_moved.last().unwrap().1.is_empty()), (Some((0, trio)), true), "from its defaults");
        // A saved state is handed over each way: undone again, the same one.
        let ch = a.state.project.channels[1].clone();
        record(&mut a.state, REPLACE_CHANNEL, Snap::Replace(Box::new(Replaced { row: 1, ch, host: Some((7, Some(vec![1, 2]))), other: None })));
        a.state.project.channels[1].name = Some("New".into());
        for _ in 0..2 {
            ctrl_z(&mut a);
            assert_eq!(a.state.rows_moved.last().unwrap().1, [(1, 7, vec![1, 2])]);
            ctrl_alt_z(&mut a);
            assert!(a.state.rows_moved.last().unwrap().1.is_empty());
        }
    }

    /// Put undone recordings in the recycle bin: stored and shown, on by default; Lemon Studio does not record yet
    /// (spec: ui/settings-choices.md "Undo history: the four checks (2026-10-16)").
    #[test]
    fn recycle_bin_check_is_kept() {
        assert!(prefs::check(&prefs::UNDONE_RECORDINGS_TO_BIN));
        assert!(!prefs::toggle(&prefs::UNDONE_RECORDINGS_TO_BIN));
        assert!(!prefs::check(&prefs::UNDONE_RECORDINGS_TO_BIN));
        prefs::set_check(&prefs::UNDONE_RECORDINGS_TO_BIN, true);
    }
}
