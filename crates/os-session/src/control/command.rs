// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The commands: every change the control plane makes to a project, described as an operation on things named by id
//! (`os_model::Id`), not as a click. A command first takes what it is about to change (its `Region`) and then changes
//! it; the region taken is the command that puts it back (`Command::Put`), so every command can be undone, and undoing
//! that gives the redo. Positions and lengths are ticks of the project's timebase (`ppq`).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{ChannelKind, Clip, ClipSource, Id, MixerTrack, Note, Pattern, Project, Rgb};
use serde::{Deserialize, Serialize};


/// A command (see the module). Read from JSON by its `op` (`{"op": "set_tempo", "bpm": 128}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    // ----- The project
    /// The song's tempo, 10..=522 BPM.
    SetTempo { bpm: f64 },
    /// Main swing, 0..=1.
    SetSwing { swing: f64 },
    /// The main volume as its knob has it, 0..=1 (0.8 = unity), and the main pitch in cents (-1200..=1200).
    SetMainVolume { volume: f64 },
    SetMainPitch { cents: i16 },
    /// The bar as a time signature: `numerator` / `denominator` (2, 4, 8 or 16).
    SetTimeSignature { numerator: u16, denominator: u16 },
    /// The Song info: each field given is set (empty text clears it).
    SetInfo {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        genre: Option<String>,
        #[serde(default)]
        comments: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
    /// Settings > Project's own settings: each one given is set. The bar as `beats` per bar (1..=32) and `steps` per
    /// beat (1..=16), a value the project already has kept as it is; `signature`: the bar shown as a time signature
    /// (else as a time division); `panning_law`: "circular" or "triangular"; `play_truncated`: notes a pattern clip
    /// cuts off still play.
    SetProject {
        #[serde(default)]
        beats: Option<u16>,
        #[serde(default)]
        steps: Option<u16>,
        #[serde(default)]
        signature: Option<bool>,
        #[serde(default)]
        panning_law: Option<String>,
        #[serde(default)]
        play_truncated: Option<bool>,
        /// Automatic plugin delay compensation.
        #[serde(default)]
        pdc: Option<bool>,
    },

    // ----- Channels
    /// A new channel at the bottom of the rack: a Lemon Sampler channel (with `sample` loaded, when given), or one
    /// played by generator `plugin` (a plugin's name as the plugin menu lists it). On mixer track `track` when given,
    /// else on the next free insert as the rack's Add button does. The result names its id.
    AddChannel {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        plugin: Option<String>,
        #[serde(default)]
        sample: Option<String>,
        #[serde(default)]
        track: Option<u32>,
    },
    /// The channel goes, with its notes, its clips and its automation (as the rack's Delete does).
    DeleteChannel { channel: u32 },
    /// The channel moves to rack row `row` (0 = top).
    MoveChannel { channel: u32, row: usize },
    /// Each setting given is set: name (empty = the default name), colour (`#rrggbb`, empty = the default), volume
    /// 0..=1 (0.78125 = the default), pan -1..=1, pitch in cents, enabled (the rack's mute light), mixer track (an id).
    SetChannel {
        channel: u32,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        volume: Option<f64>,
        #[serde(default)]
        pan: Option<f64>,
        #[serde(default)]
        pitch: Option<i32>,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        track: Option<u32>,
    },
    /// A sample file into a sampler channel (empty: none).
    LoadSample { channel: u32, path: String },

    // ----- The mixer
    /// A new insert track at number `at` (later ones move right), else after the last one in use. The result names
    /// its id.
    AddTrack {
        #[serde(default)]
        at: Option<u16>,
    },
    /// The track goes (as its menu's Delete does): later tracks move left, channels on it go to the master.
    DeleteTrack { track: u32 },
    /// Each setting given is set: name, colour, volume 0..=1 (0.8 = unity), pan -1..=1, stereo separation -1..=1,
    /// enabled (the mute light), polarity, swap left / right, effects enabled, separator (a group starts here), dock
    /// ("left", "middle" or "right"), threaded (allow threaded processing), input and track latency in ms
    /// (-1000..=1000).
    SetTrack {
        track: u32,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        volume: Option<f64>,
        #[serde(default)]
        pan: Option<f64>,
        #[serde(default)]
        stereo: Option<f64>,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        polarity: Option<bool>,
        #[serde(default)]
        swap: Option<bool>,
        #[serde(default)]
        effects_enabled: Option<bool>,
        #[serde(default)]
        separator: Option<bool>,
        #[serde(default)]
        dock: Option<String>,
        #[serde(default)]
        threaded: Option<bool>,
        #[serde(default)]
        latency_in: Option<f64>,
        #[serde(default)]
        latency_out: Option<f64>,
    },
    /// The track back to a new track's settings, routed to the master; its name, colour, levels... go, its effects stay.
    ResetTrack { track: u32 },
    /// Auto-bypass when silent, for every plugin: the mixer slots', the plugin channels' and the containers' members'.
    SetAutoBypass { on: bool },
    /// Track `from` sends to `to` at `level` 0..=1 (0.8 = unity, 0 = a sidechain only); no level: the route goes.
    SetSend {
        from: u32,
        to: u32,
        #[serde(default)]
        level: Option<f64>,
    },
    /// Band `band` (0 low shelf, 1 peaking, 2 high shelf) of the track's equaliser, as stored: gain -1800..=1800
    /// (0.01 dB), frequency and width 0..=65536.
    SetEq {
        track: u32,
        band: u8,
        #[serde(default)]
        gain: Option<i32>,
        #[serde(default)]
        frequency: Option<i32>,
        #[serde(default)]
        width: Option<i32>,
    },
    /// Effect `plugin` (its name as the plugin menu lists it) into the track's slot `slot` (0..=9), else its first
    /// empty one. The result names the slot's id.
    AddEffect {
        track: u32,
        #[serde(default)]
        slot: Option<u8>,
        plugin: String,
    },
    /// The effect in the track's slot `slot` goes.
    RemoveEffect { track: u32, slot: u8 },
    /// The slot's light and mix level 0..=1.
    SetSlot {
        track: u32,
        slot: u8,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        mix: Option<f64>,
    },

    // ----- Patterns
    /// A new pattern after the last one. The result names its id.
    AddPattern {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
    },
    /// The pattern goes, with its clips; later patterns move down a number.
    DeletePattern { pattern: u32 },
    /// Each setting given is set: name, colour, length in ticks (0 = Auto).
    SetPattern {
        pattern: u32,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        length: Option<u32>,
    },
    /// The pattern's notes for `channel` (or all its notes, without one) become `notes`.
    SetNotes {
        pattern: u32,
        #[serde(default)]
        channel: Option<u32>,
        notes: Vec<NoteIn>,
    },
    /// `notes` are added to the pattern.
    AddNotes { pattern: u32, notes: Vec<NoteIn> },

    // ----- The playlist (the arrangement shown)
    /// A clip of pattern `pattern` or of audio / automation channel `channel` on playlist track `track` (1-based) at
    /// `position`, `length` long (else its source's length). The result names its id.
    AddClip {
        track: u16,
        position: u32,
        #[serde(default)]
        length: Option<u32>,
        #[serde(default)]
        pattern: Option<u32>,
        #[serde(default)]
        channel: Option<u32>,
    },
    /// The clip goes.
    DeleteClip { clip: u32 },
    /// Each setting given is set: position, playlist track, length, muted.
    SetClip {
        clip: u32,
        #[serde(default)]
        position: Option<u32>,
        #[serde(default)]
        track: Option<u16>,
        #[serde(default)]
        length: Option<u32>,
        #[serde(default)]
        muted: Option<bool>,
    },
    /// Playlist track `track` (1-based): name, colour, enabled.
    SetPlaylistTrack {
        track: u16,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        enabled: Option<bool>,
    },

    // ----- Controls in channel and container windows
    /// The LFO of the automation clip on `channel`, as stored: on, add (else it multiplies), amount 0..=128, speed
    /// 0..=65536 (45819 = one cycle per beat), tension, skew and pulse width -128..=128.
    SetClipLfo {
        channel: u32,
        #[serde(default)]
        on: Option<bool>,
        #[serde(default)]
        add: Option<bool>,
        #[serde(default)]
        amount: Option<i32>,
        #[serde(default)]
        speed: Option<i32>,
        #[serde(default)]
        tension: Option<i32>,
        #[serde(default)]
        skew: Option<i32>,
        #[serde(default)]
        pulse_width: Option<i32>,
    },
    /// The settings of the MIDI Out on `channel`: MIDI channel 0..=15, port 0..=255, patch 0..=128 (0 = none), the
    /// two banks -1..=127 (-1 = none), send pan, volume and pitch, map note colour to MIDI channel.
    SetMidiOut {
        channel: u32,
        #[serde(default)]
        midi_channel: Option<u8>,
        #[serde(default)]
        port: Option<u8>,
        #[serde(default)]
        patch: Option<u8>,
        #[serde(default)]
        bank: Option<i32>,
        #[serde(default)]
        bank2: Option<i32>,
        #[serde(default)]
        send_levels: Option<bool>,
        #[serde(default)]
        map_color: Option<bool>,
    },
    /// The wet and dry levels 0..=1 of member `member` of the container held by `container` (the id of a mixer slot,
    /// or of a channel whose instrument is a container).
    SetMember {
        container: u32,
        member: u32,
        #[serde(default)]
        wet: Option<f64>,
        #[serde(default)]
        dry: Option<f64>,
    },

    // ----- Plugins
    /// Parameter `param` (the plugin's own parameter id) of the instance playing channel or slot `owner` to `value`
    /// 0..=1. `from` is the value before; the app fills it in (`App::command`), and the undo puts it back.
    SetParam {
        owner: u32,
        param: u32,
        value: f64,
        #[serde(default)]
        from: Option<f64>,
    },

    /// Puts back what a command took (its undo); not offered to clients.
    #[serde(skip)]
    Put(Box<Taken>),
}

/// A note as commands give it. Keys are MIDI numbers (60 = C5); velocity 0..=128 (100 by default), pan 0..=128 (64
/// = centre).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteIn {
    pub channel: u32,
    pub position: u32,
    #[serde(default)]
    pub length: u32,
    #[serde(default = "middle_c")]
    pub key: u16,
    #[serde(default)]
    pub velocity: Option<u8>,
    #[serde(default)]
    pub pan: Option<u8>,
    /// `set_notes`: the note's id as the notes query names it, kept when the pattern held a note with it; a note
    /// without one, or with another, gets a new id. `add_notes` takes none: its notes are new.
    #[serde(default)]
    pub id: Option<u32>,
}

fn middle_c() -> u16 {
    60
}

/// What a command is about to change, as it was (`Command::Put` puts it back).
#[derive(Debug, Clone, PartialEq)]
pub enum Taken {
    /// The project settings commands change.
    Settings(Box<Settings>),
    /// One channel (it stays on its row).
    Channel(Box<os_model::Channel>),
    /// One mixer track, by its place.
    Track(Box<MixerTrack>),
    /// One pattern.
    Pattern(Box<Pattern>),
    /// The LFO of an automation clip, by its channel's id.
    Lfo(u32, Box<os_model::clip_lfo::ClipLfo>),
    /// The arrangement shown.
    Arrangement(Box<os_model::Arrangement>),
    /// A plugin parameter: (owner, parameter, value).
    Param(u32, u32, f64),
    /// A plugin's whole state (a preset loaded into it): (owner, the state and what came with it).
    Plugin(u32, Box<PluginSide>),
    /// The whole project and the rack's own state of it (adding or deleting channels, tracks, patterns).
    Project(Box<Side>),
    /// Which channels are on, by id (a channel muted or soloed).
    Mutes(Vec<(u32, bool)>),
    /// Which mixer tracks are on, by id (a client's `set_track` switching one on or off: the step's own undo).
    TracksOn(Vec<(u32, bool)>),
    /// Remote control links re-pointed (a wrap moving plugins into a container, `crate::container`): each link on the
    /// first target of a pair goes onto the second. They are the program's settings, so this puts back only those links.
    Links(Vec<(crate::typein::Target, crate::typein::Target)>),
    /// What a command added, by id: putting it back removes it (the undo of an add). Its other side is `Back`.
    Gone(Thing),
    /// What `Gone` removed, put back as it was (the redo of an add): a channel on its row, a mixer track at its
    /// number, a pattern at its number. Its other side is `Gone`.
    Back(Box<Back>),
    /// Where a channel was, by id: putting it back moves the channel to that row again (the undo of a move).
    Row(u32, usize),
    /// What a delete took out, each piece where it was (the undo of a delete, `control::removal`). Its other side is
    /// `Deleted`.
    Removed(Box<super::removal::Removed>),
    /// A thing to delete again (the redo of a delete), with the ids the things the delete makes had. Its other side is
    /// `Removed`.
    Deleted(Thing, Vec<u32>),
}

/// Something a command added (`Taken::Gone`), by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thing {
    Channel(u32),
    Track(u32),
    Pattern(u32),
}

/// A thing as it was where it was (`Taken::Back`).
#[derive(Debug, Clone, PartialEq)]
pub enum Back {
    /// The channel and its row.
    Channel(usize, os_model::Channel),
    /// The track, at its number (`MixerTrack::index`).
    Track(MixerTrack),
    /// The pattern, at its number.
    Pattern(Pattern),
}

/// What a preset load changes of a plugin (`Taken::Plugin`): its state in its format's own bytes, the preset its
/// window's Presets box shows, and the name of its channel or slot ("Auto name channels", "Auto name effect slots";
/// `None`: the default name, for a slot no name typed or given).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginSide {
    pub state: Vec<u8>,
    pub preset: Option<std::path::PathBuf>,
    pub name: Option<String>,
    /// Not 0: the state and preset are the app's to fill in, as the plugin has them when this side's other one goes in
    /// (`Control::states`, `App::host_commands`). Only the app reads a plugin.
    pub fill: u64,
}

impl PluginSide {
    /// The name of plugin `owner`'s channel or slot as it is now; `None` when it has none of either.
    fn name_of(s: &crate::session::Session, owner: u32) -> Option<Option<String>> {
        let key = owner_key(s, owner)?;
        Some(match crate::plugins::fx_of(key) {
            // A container's effect has no name of its own (a preset renames nothing).
            None if key >= crate::plugins::MEMBER_KEY => None,
            None => s.project.channels[key].name.clone(),
            Some((t, k)) => crate::edit::mixer::slot_label(s, t, k),
        })
    }

    /// Puts the name back (`apply` sends the state and preset to the app).
    fn put_name(&self, s: &mut crate::session::Session, owner: u32) {
        let Some(key) = owner_key(s, owner).filter(|&k| k < crate::plugins::MEMBER_KEY) else { return };
        match crate::plugins::fx_of(key) {
            None => s.project.get_mut().channels[key].name = self.name.clone(),
            Some((t, k)) => crate::edit::mixer::set_slot_label(s, t, k, self.name.clone()),
        }
    }
}

/// The project settings commands change: the tempo (kept by the app beside the project), swing, main volume and
/// pitch, the bar and the Song info.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub tempo: f64,
    pub swing: Option<u8>,
    pub main_volume: Option<u16>,
    pub main_pitch: Option<i16>,
    pub bar: Option<(u16, u16)>,
    pub bar_as_signature: Option<bool>,
    pub info: os_model::ProjectInfo,
    /// Settings > Project's switches that are the project's own (its interface edits; no command sets them).
    pub panning_law: Option<u8>,
    pub play_truncated: bool,
    pub pdc_off: bool,
}

impl Settings {
    fn of(s: &crate::session::Session) -> Settings {
        let p = &s.project;
        Settings {
            tempo: s.tempo(),
            swing: p.swing,
            main_volume: p.main_volume,
            main_pitch: p.main_pitch,
            bar: p.bar,
            bar_as_signature: p.bar_as_signature,
            info: p.info.clone(),
            panning_law: p.panning_law,
            play_truncated: p.play_truncated,
            pdc_off: p.pdc_off,
        }
    }

    pub fn put(self, s: &mut crate::session::Session) {
        s.set_tempo(self.tempo);
        let p = s.project.get_mut();
        (p.swing, p.main_volume, p.main_pitch, p.bar, p.bar_as_signature, p.info) =
            (self.swing, self.main_volume, self.main_pitch, self.bar, self.bar_as_signature, self.info);
        (p.panning_law, p.play_truncated, p.pdc_off) = (self.panning_law, self.play_truncated, self.pdc_off);
    }
}

/// The whole project, the pattern shown, and the loop type the rack remembers while loop mode is off.
#[derive(Debug, Clone, PartialEq)]
pub struct Side {
    pub project: Project,
    pub tempo: f64,
    pub pattern: u16,
    pub loop_type: (u8, u16),
}

impl Side {
    fn of(s: &crate::session::Session) -> Side {
        Side {
            project: s.project.clone(),
            tempo: s.tempo(),
            pattern: s.pattern(),
            loop_type: (s.loop_type, s.loop_type_of),
        }
    }
}

/// What an edit in the interface changes (`control::edit`): the project settings, one channel (by row), one mixer
/// track (by number), the arrangement shown, the whole project with the rack's state of it, or which channels are on.
/// The smallest that holds the edit keeps the undo step small.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Settings,
    Channel(usize),
    Track(u16),
    Arrangement,
    Project,
    Mutes,
}

impl Taken {
    /// `region` as it is now (`None`: it names nothing).
    pub fn of(s: &crate::session::Session, region: Region) -> Option<Taken> {
        let p = &s.project;
        Some(match region {
            Region::Settings => Taken::Settings(Box::new(Settings::of(s))),
            Region::Channel(row) => Taken::Channel(Box::new(p.channels.get(row)?.clone())),
            Region::Track(n) => Taken::Track(Box::new(p.mixer.iter().find(|m| m.index == n)?.clone())),
            Region::Arrangement => Taken::Arrangement(Box::new(p.arrangement()?.clone())),
            Region::Project => Taken::Project(Box::new(Side::of(s))),
            Region::Mutes => mutes(p),
        })
    }
}

/// The steps of the project settings, the commands' and the interface's.
pub const TEMPO: &str = "Tempo";
pub const TIME_SIGNATURE: &str = "Time signature";
pub const SONG_INFO: &str = "Song info";
pub const PLUGIN_PARAMETER: &str = "Plugin parameter";
/// A preset loaded into a plugin from its window or dropped on it (`control::state_edit`).
pub const PLUGIN_PRESET: &str = "Plugin preset";

/// A command that can't be applied: an id that names nothing, a value out of range.
pub type Error = String;

/// What applying a command gives: the command that undoes it, and its result for the client (a new thing's id).
pub struct Done {
    pub undo: Command,
    pub result: serde_json::Value,
}

// ---------------------------------------------------------------------------------------------------------
// Finding things by id

/// The LFO of the automation clip on channel `id`.
fn clip_lfo(p: &Project, id: u32) -> Result<&os_model::clip_lfo::ClipLfo, Error> {
    let ch = p.channels[channel_row(p, id)?].index;
    p.automation(ch).map(|a| &a.lfo).ok_or_else(|| format!("channel {id} is not an automation clip"))
}

pub fn channel_row(p: &Project, id: u32) -> Result<usize, Error> {
    p.channels.iter().position(|c| c.id.0 == id).ok_or_else(|| format!("no channel {id}"))
}

/// A mixer track's number (0 = the master) by its id.
/// A mixer track by id: one the `project` query lists, so not the hidden last track (`mixer::inserts`).
pub fn track_number(p: &Project, id: u32) -> Result<u16, Error> {
    track_place(p, id).map(|i| p.mixer[i].index)
}

fn track_place(p: &Project, id: u32) -> Result<usize, Error> {
    let hidden = p.mixer.len().saturating_sub(1) as u16;
    p.mixer.iter().position(|m| m.id.0 == id && (m.index < hidden || p.mixer.len() < 2)).ok_or_else(|| format!("no mixer track {id}"))
}

pub fn pattern_place(p: &Project, id: u32) -> Result<usize, Error> {
    p.patterns.iter().position(|c| c.id.0 == id).ok_or_else(|| format!("no pattern {id}"))
}

fn clip_place(p: &Project, id: u32) -> Result<usize, Error> {
    p.arrangement().and_then(|a| a.clips.iter().position(|c| c.id.0 == id)).ok_or_else(|| format!("no clip {id} in the playlist"))
}

/// The channel or slot plugin with id `owner`: its `Plugins::hosted` key.
pub fn owner_key(s: &crate::session::Session, owner: u32) -> Option<usize> {
    let p = &s.project;
    let slot = |owner: u32| {
        p.mixer.iter().find_map(|m| {
            let k = m.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.id.0 == owner))?;
            Some((m.index, k as u8))
        })
    };
    if owner & MEMBER_OWNER != 0 {
        let (t, k) = crate::edit::container::id_place(s, owner & MEMBER_SLOT)?;
        let id = (owner & !MEMBER_OWNER) >> 24;
        return s.plugin_members.iter().find(|(_, m)| **m == (t, k, id)).map(|(&key, _)| key);
    }
    if let Some(r) = p.channels.iter().position(|c| c.id.0 == owner) {
        return Some(r);
    }
    slot(owner).map(|(t, k)| crate::plugins::fx_key(t, k))
}

/// The id of the channel or slot plugin hosted under `Plugins::hosted` key `key` (`owner_key`'s other way); for a
/// container's effect, its `member_owner` (0 while its container's slot has no id yet).
pub fn key_owner(s: &crate::session::Session, key: usize) -> Option<u32> {
    let p = &s.project;
    let slot_id = |t: u16, k: u8| p.mixer.iter().find(|m| m.index == t)?.slots.get(usize::from(k))?.as_ref().map(|s| s.id.0);
    if key >= crate::plugins::MEMBER_KEY {
        let &(t, k, id) = s.plugin_members.get(&key)?;
        return match crate::edit::container::place_id(s, t, k)?.0 {
            0 => Some(0),
            slot => member_owner(slot, id),
        };
    }
    match crate::plugins::fx_of(key) {
        None => p.channels.get(key).map(|c| c.id.0),
        Some((t, k)) => slot_id(t, k),
    }
}

/// The owner ids of the plugins inside containers (Lemon Studio's own, spec plugins/patcher/README.md §4.3, §4.4): this
/// bit, the member's id in its container (1 to 15) in bits 24 to 27 and its container's slot plugin id (or channel id)
/// in the low 24 bits, so that a member's undo steps follow it when its container moves and when the list is reordered.
pub const MEMBER_OWNER: u32 = 1 << 31;
const MEMBER_SLOT: u32 = (1 << 24) - 1;

/// The owner id of member `id` of the container whose slot plugin's (or channel's) id is `slot` (`None`: an id with no
/// room).
pub fn member_owner(slot: u32, id: u32) -> Option<u32> {
    (slot != 0 && slot <= MEMBER_SLOT && (1..=os_model::target::MEMBER_IDS).contains(&id)).then_some(MEMBER_OWNER | id << 24 | slot)
}

/// `#rrggbb` (empty: `None`, the default colour).
pub fn parse_color(text: &str) -> Result<Option<Rgb>, Error> {
    let t = text.trim().trim_start_matches('#');
    if t.is_empty() {
        return Ok(None);
    }
    let v = u32::from_str_radix(t, 16).ok().filter(|_| t.len() == 6).ok_or_else(|| format!("not a colour: {text}"))?;
    Ok(Some(Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8)))
}

pub fn color_text(c: Option<Rgb>) -> Option<String> {
    c.map(|c| format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b))
}

fn text(t: &str) -> Option<String> {
    (!t.is_empty()).then(|| t.to_string())
}

fn unit(name: &str, v: f64, lo: f64, hi: f64) -> Result<f64, Error> {
    if v.is_finite() && (lo..=hi).contains(&v) {
        Ok(v)
    } else {
        Err(format!("{name} must be {lo}..={hi}, not {v}"))
    }
}

/// The latest tick a position or length may reach (about 87 000 bars at 96 ticks a quarter): sums stay in range.
const MAX_TICK: u32 = 1 << 30;

fn ticks(name: &str, v: u32) -> Result<u32, Error> {
    if v <= MAX_TICK {
        Ok(v)
    } else {
        Err(format!("{name} {v} is past the last tick ({MAX_TICK})"))
    }
}

fn id_result(key: &str, id: Id) -> serde_json::Value {
    serde_json::json!({ key: id.0 })
}

/// `notes` with their ids: one of `held` (the pattern's before the command) given once is kept, any other note gets a new
/// one (`Control::fresh_note`); and the result naming them in the order given.
fn with_ids(s: &mut crate::session::Session, mut notes: Vec<os_model::Note>, held: &std::collections::HashSet<u32>) -> (Vec<os_model::Note>, serde_json::Value) {
    let mut kept = std::collections::HashSet::new();
    for n in &mut notes {
        if !(held.contains(&n.id.0) && kept.insert(n.id.0)) {
            n.id = s.control.fresh_note();
        }
    }
    let ids: Vec<u32> = notes.iter().map(|n| n.id.0).collect();
    (notes, serde_json::json!({ "notes": ids }))
}

// ---------------------------------------------------------------------------------------------------------
// Applying

impl Command {
    /// The undo history's name for this command's step.
    pub fn name(&self) -> &'static str {
        match self {
            Command::SetTempo { .. } => TEMPO,
            Command::SetSwing { .. } => "Main swing",
            Command::SetMainVolume { .. } => "Main volume",
            Command::SetMainPitch { .. } => "Main pitch",
            Command::SetTimeSignature { .. } => TIME_SIGNATURE,
            Command::SetInfo { .. } => SONG_INFO,
            Command::SetProject { .. } => crate::history::PROJECT_SETTINGS,
            Command::AddChannel { .. } => "Add channel",
            Command::DeleteChannel { .. } => "Delete channel",
            Command::MoveChannel { .. } => "Move channel",
            Command::SetChannel { .. } => "Channel settings",
            Command::LoadSample { .. } => crate::history::LOAD_SAMPLE,
            Command::AddTrack { .. } => "Insert mixer track",
            Command::DeleteTrack { .. } => "Delete mixer track",
            Command::SetTrack { .. } => "Mixer track settings",
            Command::ResetTrack { .. } => "Mixer track reset",
            Command::SetAutoBypass { .. } => "Auto-bypass for all plugins",
            Command::SetSend { .. } => "Mixer routing",
            Command::SetEq { .. } => "Mixer track equalizer",
            Command::AddEffect { .. } => "Add effect",
            Command::RemoveEffect { .. } => "Remove effect",
            Command::SetSlot { .. } => "Effect slot",
            Command::AddPattern { .. } => "Add pattern",
            Command::DeletePattern { .. } => "Delete pattern",
            Command::SetPattern { .. } => "Pattern settings",
            Command::SetNotes { .. } | Command::AddNotes { .. } => "Edit notes",
            Command::AddClip { .. } => "Add clip",
            Command::DeleteClip { .. } => "Delete clip",
            Command::SetClip { .. } => "Edit clip",
            Command::SetPlaylistTrack { .. } => "Playlist track",
            Command::SetClipLfo { .. } => "Automation clip LFO",
            Command::SetMidiOut { .. } => "MIDI Out setting",
            Command::SetMember { .. } => crate::container::LEVEL,
            Command::SetParam { .. } => PLUGIN_PARAMETER,
            Command::Put(_) => "Undo",
        }
    }

    /// The command that would put back what this one is about to change, as it is now: the undo of a change made in
    /// steps under one press (a knob's drag takes it at the press, `history::knob`).
    /// A `SetTrack` for track `track` that sets nothing yet.
    pub fn set_track(track: u32) -> Command {
        Command::SetTrack {
            track,
            name: None,
            color: None,
            volume: None,
            pan: None,
            stereo: None,
            enabled: None,
            polarity: None,
            swap: None,
            effects_enabled: None,
            separator: None,
            dock: None,
            threaded: None,
            latency_in: None,
            latency_out: None,
        }
    }

    /// A `SetProject` that sets nothing yet.
    pub fn set_project() -> Command {
        Command::SetProject { beats: None, steps: None, signature: None, panning_law: None, play_truncated: None, pdc: None }
    }

    pub fn undo_now(&self, s: &crate::session::Session) -> Result<Command, Error> {
        self.take(s).map(|t| Command::Put(Box::new(t)))
    }

    /// What this command is about to change, taken from the project as it is now.
    fn take(&self, s: &crate::session::Session) -> Result<Taken, Error> {
        let p = &s.project;
        Ok(match self {
            Command::SetTempo { .. }
            | Command::SetSwing { .. }
            | Command::SetMainVolume { .. }
            | Command::SetMainPitch { .. }
            | Command::SetTimeSignature { .. }
            | Command::SetInfo { .. }
            | Command::SetProject { .. } => Taken::Settings(Box::new(Settings::of(s))),
            Command::SetChannel { channel, .. } | Command::LoadSample { channel, .. } => Taken::Channel(Box::new(p.channels[channel_row(p, *channel)?].clone())),
            Command::SetTrack { track, .. } | Command::ResetTrack { track } | Command::SetEq { track, .. } | Command::SetSlot { track, .. } | Command::AddEffect { track, .. } | Command::RemoveEffect { track, .. } => {
                Taken::Track(Box::new(p.mixer[track_place(p, *track)?].clone()))
            }
            Command::SetSend { from, .. } => Taken::Track(Box::new(p.mixer[track_place(p, *from)?].clone())),
            Command::SetPattern { pattern, .. } | Command::SetNotes { pattern, .. } | Command::AddNotes { pattern, .. } => {
                Taken::Pattern(Box::new(p.patterns[pattern_place(p, *pattern)?].clone()))
            }
            Command::AddClip { .. } | Command::DeleteClip { .. } | Command::SetClip { .. } | Command::SetPlaylistTrack { .. } => match p.arrangement() {
                Some(a) => Taken::Arrangement(Box::new(a.clone())),
                // A project without an arrangement gets one (`playlist::arr_mut`); the undo takes it away again.
                None => Taken::Project(Box::new(Side::of(s))),
            },
            Command::SetAutoBypass { .. } => Taken::Project(Box::new(Side::of(s))),
            Command::SetClipLfo { channel, .. } => Taken::Lfo(*channel, Box::new(clip_lfo(p, *channel)?.clone())),
            Command::SetMidiOut { channel, .. } => Taken::Channel(Box::new(p.channels[channel_row(p, *channel)?].clone())),
            Command::SetMember { container, .. } => match crate::edit::container::id_place(s, *container).ok_or_else(|| format!("no container {container}"))? {
                (row, crate::container::CHANNEL) => Taken::Channel(Box::new(p.channels[usize::from(row)].clone())),
                (t, _) => Taken::Track(Box::new(p.mixer.iter().find(|m| m.index == t).ok_or("the container's track is gone")?.clone())),
            },
            Command::SetParam { owner, param, from, .. } => {
                Taken::Param(*owner, *param, from.ok_or("the parameter's value before is not known (the app fills it in)")?)
            }
            Command::MoveChannel { channel, .. } => Taken::Row(*channel, channel_row(p, *channel)?),
            // What an add made is known once it is made (`apply`); this is not used.
            Command::AddChannel { .. } | Command::AddTrack { .. } | Command::AddPattern { .. } => Taken::Gone(Thing::Channel(0)),
            Command::DeleteChannel { channel } => Taken::Removed(Box::new(super::removal::Removed::of(s, Thing::Channel(*channel))?)),
            Command::DeletePattern { pattern } => Taken::Removed(Box::new(super::removal::Removed::of(s, Thing::Pattern(*pattern))?)),
            Command::DeleteTrack { track } => Taken::Removed(Box::new(super::removal::Removed::of(s, Thing::Track(*track))?)),
            Command::Put(t) => match &**t {
                Taken::Settings(_) => Taken::Settings(Box::new(Settings::of(s))),
                Taken::Channel(c) => Taken::Channel(Box::new(p.channels[channel_row(p, c.id.0)?].clone())),
                Taken::Track(m) => Taken::Track(Box::new(p.mixer[track_place(p, m.id.0)?].clone())),
                Taken::Pattern(q) => Taken::Pattern(Box::new(p.patterns[pattern_place(p, q.id.0)?].clone())),
                Taken::Lfo(id, _) => Taken::Lfo(*id, Box::new(clip_lfo(p, *id)?.clone())),
                Taken::Arrangement(a) => {
                    Taken::Arrangement(Box::new(p.arrangements.iter().find(|x| x.id.0 == a.id.0).cloned().ok_or("the arrangement is gone")?))
                }
                // The value now is the one this puts back's other side: the value the parameter was set to.
                Taken::Param(..) => return Err("a parameter's undo is taken by `apply`".into()),
                Taken::Plugin(..) => return Err("a plugin state's undo is taken by `apply`".into()),
                Taken::Project(_) => Taken::Project(Box::new(Side::of(s))),
                Taken::Mutes(_) => mutes(p),
                Taken::TracksOn(_) => tracks_on(p),
                // The other way: the links back onto what they were on.
                Taken::Links(pairs) => Taken::Links(pairs.iter().map(|&(a, b)| (b, a)).collect()),
                Taken::Gone(thing) => Taken::Back(Box::new(match *thing {
                    Thing::Channel(id) => {
                        let row = channel_row(p, id)?;
                        if p.channels.len() < 2 {
                            return Err("a project keeps at least one channel".into());
                        }
                        Back::Channel(row, p.channels[row].clone())
                    }
                    Thing::Track(id) => {
                        let m = &p.mixer[track_place(p, id)?];
                        if m.index == 0 {
                            return Err("the master track can't be deleted".into());
                        }
                        Back::Track(m.clone())
                    }
                    Thing::Pattern(id) => Back::Pattern(p.patterns[pattern_place(p, id)?].clone()),
                })),
                Taken::Back(back) => Taken::Gone(match &**back {
                    Back::Channel(_, c) => Thing::Channel(c.id.0),
                    Back::Track(m) => Thing::Track(m.id.0),
                    Back::Pattern(q) => Thing::Pattern(q.id.0),
                }),
                Taken::Row(id, _) => Taken::Row(*id, channel_row(p, *id)?),
                Taken::Removed(r) => Taken::Deleted(r.thing(), r.made(p)),
                Taken::Deleted(thing, _) => Taken::Removed(Box::new(super::removal::Removed::of(s, *thing)?)),
            },
        })
    }

    /// The timebase changed from `from` to `to` ticks per quarter (`history::rescale`): what an undo command puts back
    /// follows, so no undo brings back ticks of the old timebase.
    pub fn rescale(&mut self, from: u16, to: u16) {
        let Command::Put(t) = self else { return };
        match &mut **t {
            Taken::Pattern(p) => {
                let mut one = Project { ppq: from, patterns: vec![std::mem::take(&mut **p)], ..Project::default() };
                one.set_ppq(to);
                **p = one.patterns.pop().unwrap_or_default();
            }
            Taken::Arrangement(a) => {
                a.clips.iter_mut().for_each(|c| c.rescale(from, to));
                a.markers.iter_mut().for_each(|m| m.rescale(from, to));
            }
            Taken::Project(side) => side.project.set_ppq(to),
            Taken::Back(b) => {
                if let Back::Pattern(p) = &mut **b {
                    rescale_pattern(p, from, to);
                }
            }
            Taken::Removed(r) => r.rescale(from, to),
            Taken::Settings(_)
            | Taken::Lfo(..)
            | Taken::Channel(_)
            | Taken::Track(_)
            | Taken::Param(..)
            | Taken::Plugin(..)
            | Taken::Mutes(_)
            | Taken::TracksOn(_)
            | Taken::Links(_)
            | Taken::Gone(_)
            | Taken::Deleted(..)
            | Taken::Row(..) => {}
        }
    }

    /// For an undo command (`Put`): the command putting back the same things as they are now (the undo history's
    /// check that a press changed something, `history::settle`). Others are as they are.
    pub fn now(&self, s: &crate::session::Session) -> Command {
        match self {
            Command::Put(t) if !matches!(**t, Taken::Param(..) | Taken::Plugin(..)) => self.take(s).map_or_else(|_| self.clone(), |t| Command::Put(Box::new(t))),
            _ => self.clone(),
        }
    }

    /// Applies the command to the project; see `Done`. Nothing is changed when it fails.
    pub fn apply(self, s: &mut crate::session::Session) -> Result<Done, Error> {
        let taken = match &self {
            // A parameter's undo: its other side is the value it moves from.
            Command::Put(t) => match &**t {
                Taken::Param(owner, param, v) => {
                    let now = s.control.param_value(*owner, *param).ok_or("the parameter's value is not known")?;
                    Taken::Param(*owner, *param, now).then_set(s, *v)
                }
                // A plugin's state: its other side is the state it replaces, which the app fills in as it puts this in.
                Taken::Plugin(owner, side) => {
                    let name = PluginSide::name_of(s, *owner).ok_or_else(|| format!("no plugin {owner}"))?;
                    let fill = s.control.put_state(*owner, (**side).clone());
                    side.put_name(s, *owner);
                    Taken::Plugin(*owner, Box::new(PluginSide { state: Vec::new(), preset: None, name, fill }))
                }
                _ => self.take(s)?,
            },
            _ => self.take(s)?,
        };
        let added = match &self {
            Command::AddChannel { .. } => Some(Thing::Channel as fn(u32) -> Thing),
            Command::AddTrack { .. } => Some(Thing::Track as fn(u32) -> Thing),
            Command::AddPattern { .. } => Some(Thing::Pattern as fn(u32) -> Thing),
            _ => None,
        };
        let result = self.change(s)?;
        s.dirty = true;
        // An add's undo removes what it made, by the id its result names.
        let taken = match added {
            Some(thing) => {
                let id = result.as_object().and_then(|o| o.values().next()).and_then(serde_json::Value::as_u64).ok_or("an add names no id")?;
                Taken::Gone(thing(id as u32))
            }
            None => taken,
        };
        Ok(Done { undo: Command::Put(Box::new(taken)), result })
    }

    /// `change` for a delete's redo (`control::removal`).
    pub(super) fn run_change(self, s: &mut crate::session::Session) -> Result<(), Error> {
        self.change(s).map(|_| ())
    }

    /// The change itself (`apply` took what it changes first).
    fn change(self, s: &mut crate::session::Session) -> Result<serde_json::Value, Error> {
        use serde_json::Value;
        let none = Ok(Value::Null);
        match self {
            Command::SetTempo { bpm } => {
                s.set_tempo(unit("bpm", bpm, 10.0, 522.0)?);
                none
            }
            Command::SetSwing { swing } => {
                s.project.get_mut().swing = Some((unit("swing", swing, 0.0, 1.0)? * 128.0).round() as u8);
                none
            }
            Command::SetMainVolume { volume } => {
                s.project.get_mut().main_volume = Some((unit("volume", volume, 0.0, 1.0)? * 16000.0).round() as u16);
                none
            }
            Command::SetMainPitch { cents } => {
                unit("cents", f64::from(cents), -1200.0, 1200.0)?;
                s.project.get_mut().main_pitch = Some(cents);
                none
            }
            Command::SetTimeSignature { numerator, denominator } => {
                if !(1..=16).contains(&numerator) || ![2, 4, 8, 16].contains(&denominator) {
                    return Err(format!("not a time signature: {numerator}/{denominator}"));
                }
                s.project.get_mut().bar = Some((numerator, 16 / denominator));
                s.project.get_mut().bar_as_signature = Some(true);
                none
            }
            Command::SetInfo { title, author, genre, comments, url } => {
                let i = &mut s.project.get_mut().info;
                for (field, v) in [(&mut i.title, title), (&mut i.author, author), (&mut i.genre, genre), (&mut i.comments, comments), (&mut i.url, url)] {
                    if let Some(v) = v {
                        *field = text(&v);
                    }
                }
                none
            }
            Command::SetProject { beats, steps, signature, panning_law, play_truncated, pdc } => {
                let (b0, s0) = s.project.bar();
                let check = |name: &str, v: Option<u16>, now: u16, max: u16| match v {
                    Some(v) if v != now && !(1..=max).contains(&v) => Err(format!("{name} must be 1..={max}, not {v}")),
                    _ => Ok(v.unwrap_or(now)),
                };
                let bar = (check("beats", beats, b0, 32)?, check("steps", steps, s0, 16)?);
                let law = match panning_law.as_deref() {
                    None => None,
                    Some("circular") => Some(0),
                    Some("triangular") => Some(2),
                    Some(l) => return Err(format!("panning_law must be circular or triangular, not {l}")),
                };
                let p = s.project.get_mut();
                if bar != (b0, s0) {
                    p.bar = Some(bar);
                }
                if let Some(on) = signature.filter(|&on| on != p.bar_is_signature()) {
                    p.bar_as_signature = Some(on);
                }
                if let Some(l) = law.filter(|&l| l != p.panning_law.unwrap_or(0)) {
                    p.panning_law = Some(l);
                }
                if let Some(on) = play_truncated {
                    p.play_truncated = on;
                }
                if let Some(on) = pdc {
                    p.pdc_off = !on;
                }
                none
            }

            Command::AddChannel { name, plugin, sample, track } => {
                let track = track.map(|t| track_number(&s.project, t)).transpose()?;
                let sampler = plugin.as_deref().is_none_or(|p| p == crate::plugins::LEMON_SAMPLER);
                if !sampler && sample.is_some() {
                    return Err("a sample goes into a sampler channel (no plugin)".into());
                }
                let row = match &plugin {
                    Some(p) if !sampler => crate::projects::add_plugin_channel(s.project.get_mut(), p),
                    _ => crate::projects::add_channel(s.project.get_mut()),
                };
                let ch = &mut s.project.get_mut().channels[row];
                if let Some(t) = track {
                    ch.mixer_track = t;
                }
                if let Some(path) = sample.filter(|p| !p.is_empty()) {
                    crate::edit::rack::auto_name(&s.settings, ch, std::path::Path::new(&path));
                    ch.sample_path = Some(path.clone());
                    s.sample_loads.push((row, path));
                }
                if let Some(n) = name {
                    s.project.get_mut().channels[row].name = text(&n);
                }
                s.touched.views.push(crate::session::View::RackAutosize);
                s.control.settle(s.project.get_mut());
                let id = s.project.channels[row].id;
                if !sampler {
                    s.control.host.push(super::HostReq::Instrument { channel: id, plugin: plugin.unwrap_or_default() });
                }
                Ok(id_result("channel", id))
            }
            Command::DeleteChannel { channel } => {
                let row = channel_row(&s.project, channel)?;
                let n = s.project.channels.len();
                if n < 2 {
                    return Err("a project keeps at least one channel".into());
                }
                let map: Vec<Option<usize>> = (0..n).map(|r| (r != row).then(|| if r < row { r } else { r - 1 })).collect();
                crate::edit::rack::remap_in(s, &map);
                none
            }
            Command::MoveChannel { channel, row: to } => {
                let row = channel_row(&s.project, channel)?;
                let n = s.project.channels.len();
                if to >= n {
                    return Err(format!("row {to} is past the last ({})", n - 1));
                }
                let mut order: Vec<usize> = (0..n).collect();
                let r = order.remove(row);
                order.insert(to, r);
                let mut map = vec![None; n];
                for (new, &old) in order.iter().enumerate() {
                    map[old] = Some(new);
                }
                crate::edit::rack::remap_in(s, &map);
                none
            }
            Command::SetChannel { channel, name, color, volume, pan, pitch, enabled, track } => {
                let track = track.map(|t| track_number(&s.project, t)).transpose()?;
                let color = color.map(|c| parse_color(&c)).transpose()?;
                let volume = volume.map(|v| unit("volume", v, 0.0, 1.0)).transpose()?;
                let pan = pan.map(|v| unit("pan", v, -1.0, 1.0)).transpose()?;
                let row = channel_row(&s.project, channel)?;
                let ch = &mut s.project.get_mut().channels[row];
                if let Some(n) = name {
                    ch.name = text(&n);
                }
                if let Some(c) = color {
                    ch.color = c.map(os_model::Rgb::for_channel);
                }
                if let Some(v) = volume {
                    ch.volume = v;
                }
                if let Some(v) = pan {
                    ch.pan = v;
                }
                if let Some(v) = pitch {
                    ch.pitch_cents = v;
                }
                if let Some(v) = enabled {
                    ch.enabled = v;
                }
                if let Some(t) = track {
                    ch.mixer_track = t;
                }
                none
            }
            Command::LoadSample { channel, path } => {
                let row = channel_row(&s.project, channel)?;
                let ch = &mut s.project.get_mut().channels[row];
                if !ch.kind.is_sampler() {
                    return Err(format!("channel {channel} is not a sampler channel"));
                }
                if path.is_empty() {
                    ch.sample_path = None;
                } else {
                    crate::edit::rack::auto_name(&s.settings, ch, std::path::Path::new(&path));
                    crate::edit::rack::read_tags(&s.settings, ch, std::path::Path::new(&path));
                    ch.sample_path = Some(path.clone());
                }
                s.sample_loads.push((row, path));
                none
            }

            Command::AddTrack { at } => {
                let last = crate::edit::mixer::inserts(s) + 1;
                if at.is_some_and(|a| a == 0 || a > last) {
                    return Err(format!("a new track goes at 1..={last}"));
                }
                let n = crate::edit::mixer::add_track_in(s, at);
                let place = s.project.mixer.iter().position(|m| m.index == n).unwrap_or(0);
                s.control.settle(s.project.get_mut());
                let id = s.project.mixer[place].id;
                Ok(id_result("track", id))
            }
            Command::DeleteTrack { track } => {
                let n = track_number(&s.project, track)?;
                if n == 0 {
                    return Err("the master track can't be deleted".into());
                }
                crate::edit::mixer::delete_track_in(s, n);
                none
            }
            Command::SetTrack { track, name, color, volume, pan, stereo, enabled, polarity, swap, effects_enabled, separator, dock, threaded, latency_in, latency_out } => {
                let dock = match dock.as_deref() {
                    None => None,
                    Some("left") => Some(os_model::DOCK_LEFT),
                    Some("middle") => Some(os_model::DOCK_MIDDLE),
                    Some("right") => Some(os_model::DOCK_RIGHT),
                    Some(d) => return Err(format!("dock must be left, middle or right, not {d}")),
                };
                let color = color.map(|c| parse_color(&c)).transpose()?;
                let volume = volume.map(|v| unit("volume", v, 0.0, 1.0)).transpose()?;
                let pan = pan.map(|v| unit("pan", v, -1.0, 1.0)).transpose()?;
                let stereo = stereo.map(|v| unit("stereo", v, -1.0, 1.0)).transpose()?;
                let max = crate::typein::LATENCY_MAX_MS;
                let latency_in = latency_in.map(|v| unit("latency_in", v, -max, max)).transpose()?;
                let latency_out = latency_out.map(|v| unit("latency_out", v, -max, max)).transpose()?;
                let place = track_place(&s.project, track)?;
                let m = &mut s.project.get_mut().mixer[place];
                if let Some(n) = name {
                    m.name = text(&n);
                }
                if let Some(c) = color {
                    m.color = c;
                }
                if volume.is_some() {
                    m.volume = volume;
                }
                if pan.is_some() {
                    m.pan = pan;
                }
                if stereo.is_some() {
                    m.stereo_separation = stereo;
                }
                if let Some(v) = enabled {
                    m.enabled = v;
                }
                if let Some(v) = polarity {
                    m.polarity = v;
                }
                if let Some(v) = swap {
                    m.swap = v;
                }
                if let Some(v) = effects_enabled {
                    m.slots_enabled = v;
                }
                if let Some(v) = separator {
                    m.separator = v;
                }
                if let Some(v) = dock {
                    m.dock = v;
                }
                if let Some(v) = threaded {
                    m.threaded = v;
                }
                if let Some(v) = latency_in {
                    m.latency_in = v as f32;
                }
                if let Some(v) = latency_out {
                    m.latency_out = v as f32;
                }
                none
            }
            Command::ResetTrack { track } => {
                let n = track_number(&s.project, track)?;
                crate::edit::mixer::reset_track(s, n);
                none
            }
            Command::SetAutoBypass { on } => {
                crate::edit::mixer::set_auto_bypass(s, on);
                none
            }
            Command::SetSend { from, to, level } => {
                let (a, b) = (track_number(&s.project, from)?, track_number(&s.project, to)?);
                if a == b {
                    return Err("a track can't send to itself".into());
                }
                let level = level.map(|v| unit("level", v, 0.0, 1.0)).transpose()?;
                if level.is_some() && crate::edit::mixer::makes_loop(s, a, b) {
                    return Err("that route would make a loop".into());
                }
                let place = track_place(&s.project, from)?;
                let m = &mut s.project.get_mut().mixer[place];
                m.send_levels.retain(|(d, _)| *d != b);
                match level {
                    Some(l) => {
                        if !m.routes.contains(&b) {
                            m.routes.push(b);
                            m.routes.sort_unstable();
                        }
                        // Unity is a route without a level, as the mixer writes it; levels by destination, as the
                        // send knobs keep them.
                        if l != 0.8 {
                            m.send_levels.push((b, l));
                            m.send_levels.sort_by_key(|&(d, _)| d);
                        }
                    }
                    None => m.routes.retain(|&r| r != b),
                }
                none
            }
            Command::SetEq { track, band, gain, frequency, width } => {
                if band > 2 {
                    return Err("bands are 0, 1 and 2".into());
                }
                for (name, v, lo, hi) in [("gain", gain, -1800, 1800), ("frequency", frequency, 0, 65536), ("width", width, 0, 65536)] {
                    if v.is_some_and(|v| !(lo..=hi).contains(&v)) {
                        return Err(format!("{name} must be {lo}..={hi}"));
                    }
                }
                let place = track_place(&s.project, track)?;
                let b = &mut s.project.get_mut().mixer[place].eq[usize::from(band)];
                for (k, v) in [gain, frequency, width].into_iter().enumerate() {
                    if let Some(v) = v {
                        b[k] = v;
                    }
                }
                none
            }
            Command::AddEffect { track, slot, plugin } => {
                let place = track_place(&s.project, track)?;
                let m = &s.project.mixer[place];
                let k = match slot {
                    Some(k) if k < 10 => usize::from(k),
                    Some(k) => return Err(format!("slots are 0..=9, not {k}")),
                    None => m.slots.iter().position(Option::is_none).ok_or("the track has no empty slot")?,
                };
                let m = &mut s.project.get_mut().mixer[place];
                m.slots[k] = Some(crate::plugins::new_slot(&plugin));
                m.slot_on[k] = true;
                m.slot_mix[k] = 1.0;
                m.slot_cpu[k] = os_model::PluginCpu::default();
                m.slot_names[k] = None;
                s.control.settle(s.project.get_mut());
                let id = s.project.mixer[place].slots[k].as_ref().map_or(Id::NONE, |x| x.id);
                s.control.host.push(super::HostReq::Effect { slot: id, plugin });
                Ok(id_result("slot", id))
            }
            Command::RemoveEffect { track, slot } => {
                let place = track_place(&s.project, track)?;
                let m = &mut s.project.get_mut().mixer[place];
                let k = usize::from(slot);
                if m.slots.get(k).is_none_or(Option::is_none) {
                    return Err(format!("slot {slot} is empty"));
                }
                m.slots[k] = None;
                m.slot_on[k] = true;
                m.slot_mix[k] = 1.0;
                m.slot_cpu[k] = os_model::PluginCpu::default();
                none
            }
            Command::SetSlot { track, slot, enabled, mix } => {
                let mix = mix.map(|v| unit("mix", v, 0.0, 1.0)).transpose()?;
                if slot > 9 {
                    return Err(format!("slots are 0..=9, not {slot}"));
                }
                let place = track_place(&s.project, track)?;
                let m = &mut s.project.get_mut().mixer[place];
                if let Some(v) = enabled {
                    m.slot_on[usize::from(slot)] = v;
                }
                if let Some(v) = mix {
                    m.slot_mix[usize::from(slot)] = v;
                }
                none
            }

            Command::AddPattern { name, color } => {
                let color = color.map(|c| parse_color(&c)).transpose()?;
                let n = s.project.patterns.iter().map(|p| p.number).max().unwrap_or(0) + 1;
                let p = crate::edit::patterns::pat_mut(s, n);
                p.name = name.as_deref().and_then(text);
                p.color = color.flatten();
                let place = s.project.patterns.iter().position(|p| p.number == n).unwrap_or(0);
                s.control.settle(s.project.get_mut());
                let id = s.project.patterns[place].id;
                Ok(id_result("pattern", id))
            }
            Command::DeletePattern { pattern } => {
                let n = s.project.patterns[pattern_place(&s.project, pattern)?].number;
                crate::edit::patterns::renumber_in(s, |m| if m == n { None } else if m > n { Some(m - 1) } else { Some(m) });
                none
            }
            Command::SetPattern { pattern, name, color, length } => {
                length.map(|l| ticks("length", l)).transpose()?;
                let color = color.map(|c| parse_color(&c)).transpose()?;
                let place = pattern_place(&s.project, pattern)?;
                let p = &mut s.project.get_mut().patterns[place];
                if let Some(n) = name {
                    p.name = text(&n);
                }
                if let Some(c) = color {
                    p.color = c;
                }
                if let Some(l) = length {
                    p.length = (l > 0).then_some(l);
                }
                none
            }
            Command::SetNotes { pattern, channel, notes } => {
                let notes = notes_of(&s.project, &notes)?;
                let only = channel.map(|c| channel_row(&s.project, c).map(|r| s.project.channels[r].index)).transpose()?;
                let place = pattern_place(&s.project, pattern)?;
                let held: std::collections::HashSet<u32> = s.project.patterns[place].notes.iter().map(|n| n.id.0).filter(|&i| i != 0).collect();
                let (notes, ids) = with_ids(s, notes, &held);
                let p = &mut s.project.get_mut().patterns[place];
                p.notes.retain(|n| only.is_some_and(|c| n.channel != c));
                p.notes.extend(notes);
                p.notes.sort_by_key(|n| n.position);
                Ok(ids)
            }
            Command::AddNotes { pattern, notes } => {
                if notes.iter().any(|n| n.id.is_some()) {
                    return Err("add_notes makes new notes: give them no id (set_notes keeps a note's)".into());
                }
                let notes = notes_of(&s.project, &notes)?;
                let place = pattern_place(&s.project, pattern)?;
                let (notes, ids) = with_ids(s, notes, &Default::default());
                let p = &mut s.project.get_mut().patterns[place];
                p.notes.extend(notes);
                p.notes.sort_by_key(|n| n.position);
                Ok(ids)
            }

            Command::AddClip { track, position, length, pattern, channel } => {
                ticks("position", position)?;
                length.map(|l| ticks("length", l)).transpose()?;
                let source = match (pattern, channel) {
                    (Some(p), None) => ClipSource::Pattern(s.project.patterns[pattern_place(&s.project, p)?].number),
                    (None, Some(c)) => {
                        let row = channel_row(&s.project, c)?;
                        let ch = &s.project.channels[row];
                        if !matches!(ch.kind, ChannelKind::AudioClip | ChannelKind::Automation) {
                            return Err(format!("channel {c} is not an audio or automation clip channel"));
                        }
                        ClipSource::Channel(ch.index)
                    }
                    _ => return Err("a clip has a pattern or a channel".into()),
                };
                if track == 0 || track > crate::edit::playlist::TRACKS {
                    return Err(format!("playlist tracks are 1..={}", crate::edit::playlist::TRACKS));
                }
                // Else the source's own length, as a clip placed in the playlist gets.
                let length = length.filter(|&l| l > 0).unwrap_or_else(|| crate::edit::playlist::source_len(s, source));
                let a = crate::edit::playlist::arr_mut(s);
                let at = a.clips.partition_point(|c| c.position <= position);
                a.clips.insert(at, Clip::new(position, length, track - 1, source));
                s.control.settle(s.project.get_mut());
                let id = crate::edit::playlist::clips(s)[at].id;
                s.touched.clips = true;
                Ok(id_result("clip", id))
            }
            Command::DeleteClip { clip } => {
                let i = clip_place(&s.project, clip)?;
                crate::edit::playlist::arr_mut(s).clips.remove(i);
                s.touched.clips = true;
                none
            }
            Command::SetClip { clip, position, track, length, muted } => {
                position.map(|v| ticks("position", v)).transpose()?;
                length.map(|v| ticks("length", v)).transpose()?;
                let i = clip_place(&s.project, clip)?;
                if track.is_some_and(|t| t == 0 || t > crate::edit::playlist::TRACKS) {
                    return Err(format!("playlist tracks are 1..={}", crate::edit::playlist::TRACKS));
                }
                if length == Some(0) {
                    return Err("a clip's length can't be 0".into());
                }
                let a = crate::edit::playlist::arr_mut(s);
                let mut c = a.clips.remove(i);
                c.position = position.unwrap_or(c.position);
                c.track = track.map_or(c.track, |t| t - 1);
                c.length = length.unwrap_or(c.length);
                c.muted = muted.unwrap_or(c.muted);
                let at = a.clips.partition_point(|x| x.position <= c.position);
                a.clips.insert(at, c);
                s.touched.clips = true;
                none
            }
            Command::SetPlaylistTrack { track, name, color, enabled } => {
                let color = color.map(|c| parse_color(&c)).transpose()?;
                if track == 0 || track > crate::edit::playlist::TRACKS {
                    return Err(format!("playlist tracks are 1..={}", crate::edit::playlist::TRACKS));
                }
                let t = &mut crate::edit::playlist::arr_mut(s).tracks[usize::from(track - 1)];
                if let Some(n) = name {
                    t.name = text(&n);
                }
                if let Some(c) = color {
                    t.color = c;
                }
                if let Some(v) = enabled {
                    t.enabled = v;
                }
                none
            }

            Command::SetClipLfo { channel, on, add, amount, speed, tension, skew, pulse_width } => {
                for (name, v, lo, hi) in [("amount", amount, 0, 128), ("speed", speed, 0, 65536), ("tension", tension, -128, 128), ("skew", skew, -128, 128), ("pulse_width", pulse_width, -128, 128)] {
                    if v.is_some_and(|v| !(lo..=hi).contains(&v)) {
                        return Err(format!("{name} must be {lo}..={hi}"));
                    }
                }
                clip_lfo(&s.project, channel)?;
                let ch = s.project.channels[channel_row(&s.project, channel)?].index;
                let Some(a) = s.project.get_mut().automation_mut(ch) else { return none };
                let l = &mut a.lfo;
                if let Some(on) = on {
                    l.switch = if on { -1 } else { 0 };
                }
                if let Some(add) = add {
                    l.mode = u8::from(add);
                }
                for (field, v) in [(&mut l.amount, amount), (&mut l.speed, speed), (&mut l.tension, tension), (&mut l.skew, skew), (&mut l.pulse_width, pulse_width)] {
                    if let Some(v) = v {
                        *field = v;
                    }
                }
                none
            }
            Command::SetMidiOut { channel, midi_channel, port, patch, bank, bank2, send_levels, map_color } => {
                let row = channel_row(&s.project, channel)?;
                let mut st = crate::lemon_midi_out::row_settings(&s.project, row).ok_or_else(|| format!("channel {channel} is not a MIDI Out"))?;
                if midi_channel.is_some_and(|c| c > 15) || patch.is_some_and(|p| p > 128) {
                    return Err("the MIDI channel is 0..=15 and the patch 0..=128".into());
                }
                if [bank, bank2].iter().flatten().any(|b| !(-1..=127).contains(b)) {
                    return Err("banks are -1..=127".into());
                }
                st.channel = midi_channel.unwrap_or(st.channel);
                st.port = port.unwrap_or(st.port);
                st.patch = patch.unwrap_or(st.patch);
                st.bank = [bank.unwrap_or(st.bank[0]), bank2.unwrap_or(st.bank[1])];
                st.send_levels = send_levels.unwrap_or(st.send_levels);
                st.map_color = map_color.unwrap_or(st.map_color);
                crate::lemon_midi_out::set_row_settings(s.project.get_mut(), row, &st);
                none
            }
            Command::SetMember { container, member, wet, dry } => {
                let (t, k) = crate::edit::container::id_place(s, container).ok_or_else(|| format!("no container {container}"))?;
                let wet = wet.map(|v| unit("wet", v, 0.0, 1.0)).transpose()?;
                let dry = dry.map(|v| unit("dry", v, 0.0, 1.0)).transpose()?;
                if crate::edit::container::member(s, t, k, member).is_none() {
                    return Err(format!("the container has no member {member}"));
                }
                for (is_wet, v) in [(true, wet), (false, dry)] {
                    if let Some(v) = v {
                        crate::edit::container::put_level_in(s, t, k, member, is_wet, v as f32);
                    }
                }
                none
            }
            Command::SetParam { owner, param, value, .. } => {
                unit("value", value, 0.0, 1.0)?;
                owner_key(s, owner).ok_or_else(|| format!("no channel or slot plugin {owner}"))?;
                s.control.set_param(owner, param, value);
                none
            }

            Command::Put(t) => {
                put(s, *t);
                none
            }
        }
    }
}

impl Taken {
    /// A parameter's undo: sets the value and keeps this (the value before) as the other side.
    fn then_set(self, s: &mut crate::session::Session, v: f64) -> Taken {
        if let Taken::Param(owner, param, _) = self {
            s.control.set_param(owner, param, v);
        }
        self
    }
}

/// Puts back what a command took.
fn put(s: &mut crate::session::Session, t: Taken) {
    match t {
        Taken::Settings(v) => v.put(s),
        Taken::Channel(c) => {
            if let Ok(row) = channel_row(&s.project, c.id.0) {
                let ch = &mut s.project.get_mut().channels[row];
                // A sampler's sample: its Lemon Sampler follows.
                if ch.sample_path != c.sample_path && c.kind.is_sampler() {
                    s.sample_loads.push((row, c.sample_path.clone().unwrap_or_default()));
                }
                // And its Misc values it plays.
                if ch.misc != c.misc && c.kind.is_sampler() {
                    s.control.misc.push(row);
                }
                // Muting is a step of its own, a lock, a zip and a filter group none: the channel stays as it is in
                // those (`Mutes`).
                let (on, locked, zipped, group) = (ch.enabled, ch.rack.locked, ch.rack.zipped, ch.rack.group);
                *ch = *c;
                (ch.enabled, ch.rack.locked, ch.rack.zipped, ch.rack.group) = (on, locked, zipped, group);
            }
        }
        Taken::Track(m) => {
            if let Ok(place) = track_place(&s.project, m.id.0) {
                let on = s.project.mixer[place].enabled;
                s.project.get_mut().mixer[place] = *m;
                s.project.get_mut().mixer[place].enabled = on;
            }
        }
        Taken::Pattern(p) => {
            if let Ok(place) = pattern_place(&s.project, p.id.0) {
                s.project.get_mut().patterns[place] = *p;
            }
        }
        Taken::Lfo(id, l) => {
            if let Some(a) = channel_row(&s.project, id).ok().map(|row| s.project.channels[row].index).and_then(|ch| s.project.get_mut().automation_mut(ch)) {
                a.lfo = *l;
            }
        }
        Taken::Arrangement(a) => {
            if let Some(cur) = s.project.get_mut().arrangements.iter_mut().find(|x| x.id.0 == a.id.0) {
                *cur = *a;
            }
            s.touched.clips = true;
        }
        // `apply` set it already.
        Taken::Param(..) | Taken::Plugin(..) => {}
        Taken::Project(side) => restore(s, *side),
        Taken::Links(pairs) => crate::remote::retarget(&s.settings, &pairs),
        Taken::Row(id, row) => {
            let _ = Command::MoveChannel { channel: id, row }.change(s);
        }
        Taken::Gone(thing) => {
            let _ = match thing {
                Thing::Channel(id) => Command::DeleteChannel { channel: id }.change(s),
                // The other way from `add_track_in`: the track goes, and the mixer is one shorter again (a delete
                // keeps its length), the hidden last track moving back with what it holds.
                Thing::Track(id) => track_number(&s.project, id).map(|n| {
                    crate::edit::mixer::delete_track_in(s, n);
                    let hidden = s.project.mixer.len() as u16 - 1;
                    crate::edit::mixer::shift_tracks(s, hidden, hidden + 1, -1);
                    s.project.get_mut().mixer.retain(|m| m.index != hidden);
                    serde_json::Value::Null
                }),
                Thing::Pattern(id) => {
                    if let Ok(place) = pattern_place(&s.project, id) {
                        s.project.get_mut().patterns.remove(place);
                        s.touched.views.push(crate::session::View::PatternsRenumbered);
                    }
                    Ok(serde_json::Value::Null)
                }
            };
        }
        Taken::Back(back) => put_back(s, *back),
        Taken::Removed(r) => r.put(s),
        Taken::Deleted(thing, made) => {
            let _ = super::removal::delete_again(s, thing, made);
        }
        Taken::Mutes(on) => {
            for (id, v) in on {
                if let Some(c) = s.project.get_mut().channels.iter_mut().find(|c| c.id.0 == id) {
                    c.enabled = v;
                }
            }
            s.dirty = true;
        }
        Taken::TracksOn(on) => {
            for (id, v) in on {
                if let Some(m) = s.project.get_mut().mixer.iter_mut().find(|m| m.id.0 == id) {
                    m.enabled = v;
                }
            }
            s.dirty = true;
        }
    }
}

/// A thing an add made, put back where it was with its id (`Taken::Back`).
fn put_back(s: &mut crate::session::Session, back: Back) {
    match back {
        Back::Channel(row, c) => put_channel_back(s, row, c),
        Back::Track(m) => {
            let n = crate::edit::mixer::add_track_in(s, Some(m.index));
            if let Some(t) = s.project.get_mut().mixer.iter_mut().find(|t| t.index == n) {
                *t = MixerTrack { index: n, ..m };
            }
            s.touched.tracks = true;
        }
        Back::Pattern(q) => {
            let pats = &mut s.project.get_mut().patterns;
            let at = pats.iter().position(|p| p.number > q.number).unwrap_or(pats.len());
            pats.insert(at, q);
            s.touched.views.push(crate::session::View::PatternsRenumbered);
        }
    }
}

/// Channel `c` put back on `row`, the rows from there one down with every index that names them.
pub(super) fn put_channel_back(s: &mut crate::session::Session, row: usize, mut c: os_model::Channel) {
    let old: Vec<u32> = s.project.channels.iter().map(|c| c.id.0).collect();
    let n = old.len();
    let row = row.min(n);
    // It comes in last, under an index no other channel has, so nothing naming the others is taken for it.
    c.index = n as u16;
    s.project.get_mut().channels.push(c);
    // The new last channel goes to its row, the ones from there one down.
    let map: Vec<Option<usize>> = (0..=n).map(|r| Some(if r == n { row } else if r < row { r } else { r + 1 })).collect();
    crate::edit::rack::remap_project(s.project.get_mut(), &map);
    crate::edit::rack::remap_session(s, &map);
    // The views follow by id, as after the project put back whole.
    s.touched.rows.get_or_insert(old);
    s.touched.views.push(crate::session::View::RackAutosize);
}

/// Pattern `p` at another timebase.
pub(super) fn rescale_pattern(p: &mut Pattern, from: u16, to: u16) {
    let mut one = Project { ppq: from, patterns: vec![std::mem::take(p)], ..Project::default() };
    one.set_ppq(to);
    *p = one.patterns.pop().unwrap_or_default();
}

/// Which channels are on now (`Taken::Mutes`).
fn mutes(p: &Project) -> Taken {
    Taken::Mutes(p.channels.iter().map(|c| (c.id.0, c.enabled)).collect())
}

/// Which mixer tracks are on now (`Taken::TracksOn`).
fn tracks_on(p: &Project) -> Taken {
    Taken::TracksOn(p.mixer.iter().map(|m| (m.id.0, m.enabled)).collect())
}

impl Command {
    /// The undo of what a command switches on or off, beside its own (`Taken::Channel` and `Taken::Track` keep a
    /// channel or track on or off as it is, since an undo of another step leaves muting alone): `set_channel` and
    /// `set_track` with `enabled`, taken before they apply.
    pub fn on_off_undo(&self, s: &crate::session::Session) -> Option<Command> {
        match self {
            Command::SetChannel { enabled: Some(_), .. } => Some(Command::Put(Box::new(mutes(&s.project)))),
            Command::SetTrack { enabled: Some(_), .. } | Command::ResetTrack { .. } => Some(Command::Put(Box::new(tracks_on(&s.project)))),
            _ => None,
        }
    }
}

/// The whole project back (an undo of adding or deleting channels, tracks or patterns): the rack's own state with it,
/// the selections kept where their things still are.
fn restore(s: &mut crate::session::Session, side: Side) {
    let old: Vec<u32> = s.project.channels.iter().map(|c| c.id.0).collect();
    let new_row: std::collections::HashMap<u32, usize> = side.project.channels.iter().enumerate().map(|(r, c)| (c.id.0, r)).collect();
    let map: Vec<Option<usize>> = old.iter().map(|id| new_row.get(id).copied()).collect();
    let Side { project, tempo, pattern, loop_type } = side;
    let shown = s.pattern();
    let numbers = |p: &Project| p.patterns.iter().map(|q| q.number).collect::<std::collections::BTreeSet<_>>();
    let patterns_before = numbers(&s.project);
    let mutes = Mutes::of(&s.project);
    s.project = crate::locked::Locked::new(project);
    mutes.keep(s.project.get_mut());
    s.set_tempo(tempo);
    s.set_pattern(pattern);
    crate::edit::rack::remap_session(s, &map);
    // The views follow the rows by id (`crate::follow`).
    s.touched.rows.get_or_insert(old);
    (s.loop_type, s.loop_type_of) = loop_type;
    // The pattern shown now stays shown, as in FL (an undo leaves the pattern shown); a step that made or removed
    // patterns (a clone, a delete) shows the pattern it showed. Each pattern's loops are its own, put back with it.
    if numbers(&s.project) == patterns_before {
        s.set_pattern(shown);
    }
    s.touched.tracks = true;
}

/// Muting and soloing are steps of their own (a channel's, `Taken::Mutes`) or none (a mixer track's, as in FL), and
/// locking a channel's mute state is none (`ui/undo-steps.md`): an undo or redo of another step leaves every channel
/// and mixer track that is still there muted, and every channel locked and zipped (no step either), or not as it is now,
/// and the song mode and performance mode as they are (none either). The states by id. The filter groups are put back
/// with the project: a step that made one (a MIDI file imported) takes it away.
struct Mutes {
    channels: std::collections::HashMap<u32, (bool, bool, bool)>,
    tracks: std::collections::HashMap<u32, bool>,
    /// The song mode and performance mode: no step either.
    song_mode: Option<bool>,
    performance: Option<bool>,
}

impl Mutes {
    fn of(p: &Project) -> Mutes {
        Mutes {
            channels: p.channels.iter().map(|c| (c.id.0, (c.enabled, c.rack.locked, c.rack.zipped))).collect(),
            tracks: p.mixer.iter().map(|m| (m.id.0, m.enabled)).collect(),
            song_mode: p.song_mode,
            performance: p.performance,
        }
    }

    fn keep(&self, p: &mut Project) {
        (p.song_mode, p.performance) = (self.song_mode, self.performance);
        for c in &mut p.channels {
            if let Some(&(on, locked, zipped)) = self.channels.get(&c.id.0) {
                (c.enabled, c.rack.locked, c.rack.zipped) = (on, locked, zipped);
            }
        }
        for m in &mut p.mixer {
            if let Some(&on) = self.tracks.get(&m.id.0) {
                m.enabled = on;
            }
        }
    }
}

/// Notes as the model stores them.
fn notes_of(p: &Project, notes: &[NoteIn]) -> Result<Vec<Note>, Error> {
    notes
        .iter()
        .map(|n| {
            let ch = p.channels[channel_row(p, n.channel)?].index;
            ticks("a note's position", n.position)?;
            ticks("a note's length", n.length)?;
            if n.key > 131 {
                return Err(format!("key {} is past the last (131)", n.key));
            }
            Ok(Note {
                length: n.length,
                key: n.key,
                velocity: n.velocity.unwrap_or(100).min(128),
                pan: n.pan.unwrap_or(64).min(128),
                id: Id(n.id.unwrap_or(0)),
                ..Note::step(ch, n.position)
            })
        })
        .collect()
}
