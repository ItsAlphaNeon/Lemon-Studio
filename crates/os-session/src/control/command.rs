// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The commands: every change the control plane makes to a project, described as an operation on things named by id
//! (`os_model::Id`), not as a click. A command first takes what it is about to change (its `Region`) and then changes
//! it; the region taken is the command that puts it back (`Command::Put`), so every command can be undone, and undoing
//! that gives the redo. Positions and lengths are ticks of the project's timebase (`ppq`).

#![allow(deprecated)] // writes the project directly (`crate::locked`; control-plane step 14 removes this)
use os_model::{ChannelKind, Clip, ClipSource, Id, MixerTrack, Note, Pattern, Project, Rgb, TrackLink};
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
        /// `"audio_clip"`, `"automation"` or `"layer"` for a channel of that kind (no plugin then), on the master track:
        /// an automation clip with `points` (else two at 0.5 over a bar), driving `target` (an automation target's id)
        /// when given; a layer playing `children` (channels' ids).
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        target: Option<u32>,
        #[serde(default)]
        points: Vec<PointIn>,
        #[serde(default)]
        children: Vec<u32>,
    },
    /// Audio file `path` as a clip at `position` on playlist track `track` (1-based; on an instrument track, the next
    /// free one below), on a new audio clip channel (through `mixer`, a mixer track's id, when given; else the track's
    /// audio link). The result names the channel and the clip.
    AddAudioClip {
        path: String,
        track: u16,
        position: u32,
        #[serde(default)]
        mixer: Option<u32>,
    },
    /// The channel a channel state file holds as the rack's last row, named `name` (else the file's channel name, else
    /// the file's name); its plugin starts from the state the file holds. The result names its id.
    LoadChannelState {
        path: String,
        #[serde(default)]
        name: Option<String>,
    },
    /// Automatic crossfades on or off: on, the audio clips that overlap on a track crossfade there; off, the crossfades
    /// they made go (spec: audio/audio-clips.md "Automatic crossfades").
    SetAutoCrossfades { on: bool },
    /// The channel goes, with its notes, its clips and its automation (as the rack's Delete does).
    DeleteChannel { channel: u32 },
    /// The channel moves to rack row `row` (0 = top).
    MoveChannel { channel: u32, row: usize },
    /// The channel's instrument replaced by generator `plugin` (none: Lemon Sampler), as the rack's Replace does: the
    /// channel keeps its row, notes, mixer track, levels and Misc settings, takes the plugin's name and a new instance
    /// from its defaults, and a new id (the result names it), so an undo brings the old instrument back by the old one.
    ReplaceInstrument {
        channel: u32,
        #[serde(default)]
        plugin: Option<String>,
    },
    /// A copy of the channel right below it (as the rack's Clone does): "<name> #2", its settings, its mixer track and a
    /// copy of its instrument, no notes. The result names its id.
    CloneChannel {
        channel: u32,
        /// Last in the rack instead (Make unique on an audio clip).
        #[serde(default)]
        at_end: bool,
    },
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
        /// Click removal (the sampler's declick mode, 0..=5).
        #[serde(default)]
        declick: Option<u8>,
        /// The channel's share of the main swing, 0..=1.
        #[serde(default)]
        swing: Option<f64>,
        /// Truncate swing notes.
        #[serde(default)]
        truncate_swing: Option<bool>,
        /// Cut group (0: none): the channel cuts the channels cut by it.
        #[serde(default)]
        cut: Option<u32>,
        /// Cut by group (0: none).
        #[serde(default)]
        cut_by: Option<u32>,
        /// Lock active / mute state: solos leave the channel as it is. An undo of another step leaves the lock as it is.
        #[serde(default)]
        locked: Option<bool>,
        /// A sampler's time stretch method (`os_model::stretch_mode`), its time, pitch and multiplier kept.
        #[serde(default)]
        stretch: Option<u32>,
        /// Zipped (folded to a thin row in the rack). An undo of another step leaves it as it is.
        #[serde(default)]
        zipped: Option<bool>,
        /// The instrument's CPU switches: Auto-bypass when silent, Allow threaded processing.
        #[serde(default)]
        auto_bypass: Option<bool>,
        #[serde(default)]
        threaded: Option<bool>,
        /// The pitch knob's range in semitones, 1..=48; a narrower one takes the knob with it.
        #[serde(default)]
        pitch_range: Option<u8>,
    },
    /// The channel rack's filter groups (spec: ui/channel-rack.md §7): `groups` their names in file order ("Unsorted"
    /// among them), `channels` each listed channel's group (an index into `groups`, `null` none), `shown` the filter
    /// shown (an index, `null` All).
    SetGroups {
        groups: Vec<String>,
        #[serde(default)]
        channels: Vec<GroupIn>,
        #[serde(default)]
        shown: Option<u32>,
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
    /// The tracks go and the mixer gets shorter (Trim unused tracks): the others close up in order, channels and sends
    /// follow them.
    RemoveTracks { tracks: Vec<u32> },
    /// The track moves to insert number `to` (1 = the first insert), the ones between moving over by one.
    MoveTrack { track: u32, to: u16 },
    /// The saved mixer track state in file `path` onto the track, its old effects going; `named`: with the state's
    /// name and colour.
    LoadTrackState {
        track: u32,
        path: String,
        #[serde(default)]
        named: bool,
    },
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
        /// The audio device input and output pair by their place in the menus, -1 none.
        #[serde(default)]
        audio_in: Option<i32>,
        #[serde(default)]
        audio_out: Option<i32>,
        /// What disk recording takes, 0 (External input only) ..= 5 (Post track).
        #[serde(default)]
        rec_source: Option<u8>,
        /// When the input is heard: `off`, `armed` or `on`.
        #[serde(default)]
        monitor: Option<String>,
        #[serde(default)]
        armed: Option<bool>,
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
    /// The effect in the track's slot `slot` goes, with the name and colour given to the slot.
    RemoveEffect { track: u32, slot: u8 },
    /// The effect in the track's slot `slot` and the one in slot `to` change places (Move up / down).
    MoveEffect { track: u32, slot: u8, to: u8 },
    /// The slot's light, mix level 0..=1, the name it shows (empty: the plugin's) and its colour (`#rrggbb`, empty:
    /// none).
    SetSlot {
        track: u32,
        slot: u8,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        mix: Option<f64>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
        /// The input port its sidechain input takes (0: none), for a plugin that has one.
        #[serde(default)]
        sidechain: Option<u32>,
        /// The slot plugin's CPU switches: Auto-bypass when silent, Allow threaded processing.
        #[serde(default)]
        auto_bypass: Option<bool>,
        #[serde(default)]
        threaded: Option<bool>,
    },

    // ----- Patterns
    /// A copy of pattern number `from` (an empty one when the project holds none with that number) as pattern number
    /// `to`, which no pattern has, named `name` (else as the original); nothing else moves. The result names its id.
    CopyPattern {
        from: u16,
        to: u16,
        #[serde(default)]
        name: Option<String>,
    },
    /// A new pattern after the last one. The result names its id.
    AddPattern {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
        /// At this number (1..=999, one no pattern has) instead of after the last one.
        #[serde(default)]
        number: Option<u16>,
    },
    /// The patterns numbered `from` and up move by `by` (1: a gap opens at `from`, as Insert one; -1: they close the
    /// gap at `from` - 1, which must hold no pattern), their clips with them.
    ShiftPatterns { from: u16, by: i8 },
    /// Pattern number `from` takes number `to` and the one there `from` (Move up / down), with their clips: by number,
    /// as a pattern number can have clips before the project holds a pattern for it.
    MovePattern { from: u16, to: u16 },
    /// A copy of the pattern right after it, named "<name> #k"; later patterns move up a number. The result names
    /// the copy's id.
    ClonePattern { pattern: u32 },
    /// Every note of the pattern moves by `semitones`; with `channels`, only those channels' notes.
    TransposePattern {
        pattern: u32,
        semitones: i32,
        #[serde(default)]
        channels: Vec<u32>,
    },
    /// The pattern keeps the first channel's notes (in rack order) and takes its name; each other channel with notes
    /// gets a pattern of its own right after it, and the original's clips get one clip per new pattern under them.
    SplitPattern { pattern: u32 },
    /// The time signature at the pattern's start (`numerator` / `denominator`), or none (`numerator` 0); its clips
    /// that kept their natural length follow.
    SetPatternSignature { pattern: u32, numerator: u8, denominator: u8 },
    /// The pattern's time markers (its time signatures over its length) become `markers`, put in position order; its
    /// clips that kept their natural length follow.
    SetPatternMarkers { pattern: u32, markers: Vec<MarkerIn> },
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
    /// The pattern's control events (a channel's volume, pan and pitch over the pattern, the event editor's) become
    /// `controls`, in this order; one named by its id keeps it.
    SetControls { pattern: u32, controls: Vec<ControlIn> },
    /// The pattern's step sequencer loops (spec: ui/channel-rack.md §15): `mode` 0 off, 1 Loop step channels, 2 Loop all
    /// channels, 3 Advanced looping; each of `loops` sets a channel's loop value as the file keeps it (a length in steps,
    /// a loop point's code, Bar), `null` for none.
    SetLoops {
        pattern: u32,
        #[serde(default)]
        mode: Option<u8>,
        #[serde(default)]
        loops: Vec<LoopIn>,
    },
    /// Step `step` (a sixteenth) of the channel's row in the pattern on or off, as a click in the step sequencer: on adds
    /// a step note (or brings back its notes when they are muted), off removes the notes inside the step, or mutes
    /// them with `mute` (the rack's "Mute removed steps").
    SetStep {
        pattern: u32,
        channel: u32,
        step: u32,
        on: bool,
        #[serde(default)]
        mute: bool,
    },

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
    /// Playlist track `track` (1-based): name, colour, enabled, grouped with the track above.
    SetPlaylistTrack {
        track: u16,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        color: Option<String>,
        #[serde(default)]
        enabled: Option<bool>,
        #[serde(default)]
        grouped: Option<bool>,
        /// Performance settings (spec: ui/performance-mode.md §4), as `os_model::PerfSettings` numbers them.
        #[serde(default)]
        motion: Option<u8>,
        #[serde(default)]
        press: Option<u8>,
        #[serde(default)]
        trigger_sync: Option<u8>,
        #[serde(default)]
        position_sync: Option<u8>,
        #[serde(default)]
        queue: Option<bool>,
        #[serde(default)]
        tolerant: Option<bool>,
        /// The source Draw and Paint place on the track: a pattern by number or a clip channel by id; `{}` none.
        #[serde(default)]
        clip_lock: Option<LockIn>,
        /// An audio track on a mixer track or an instrument track of a channel, by id; `{}` neither.
        #[serde(default)]
        link: Option<LinkIn>,
    },
    /// The points of the automation clip on channel `channel` (an id) become `points`, in beat order; a point named by
    /// its id keeps its values for what is not given. A clip keeps two points at least.
    SetPoints { channel: u32, points: Vec<PointIn> },
    /// A new arrangement after the others, a copy of arrangement `copy` when given, else empty; named `name`, else
    /// "Arrangement N" (a copy: "<name> #2"). The result names its id.
    AddArrangement {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        copy: Option<u32>,
    },
    /// The arrangement goes (a project keeps one); shown, the one before it is shown.
    DeleteArrangement { arrangement: u32 },
    /// The arrangement's name.
    SetArrangement { arrangement: u32, name: String },
    /// The shown arrangement's clips become `clips`, in this order (the order they are drawn in): a clip named by the id
    /// of one the playlist holds keeps it, and its values for what is not given; the others are new.
    SetClips { clips: Vec<ClipIn> },
    /// The shown arrangement's markers become `markers` (put in position order), a marker named by its id kept as
    /// `set_clips` keeps clips.
    SetMarkers { markers: Vec<MarkerIn> },
    /// A time marker in the playlist (spec: ui/playlist.md §7a). The result names its id.
    AddMarker { marker: MarkerIn },
    /// Each setting given is set on the marker.
    SetMarker {
        marker: u32,
        #[serde(default)]
        position: Option<u32>,
        #[serde(default)]
        name: Option<String>,
        /// Its action (`os_model::TimeMarker`'s kinds: 0 marker, 1 marker loop, 2 skip, 3 pause, 4 loop, 5 start, 8
        /// time signature, 9 punch in, 10 punch out, 12 key).
        #[serde(default)]
        kind: Option<u8>,
        /// A time signature marker's.
        #[serde(default)]
        numerator: Option<u8>,
        #[serde(default)]
        denominator: Option<u8>,
    },
    /// The marker goes.
    DeleteMarker { marker: u32 },
    /// Move content right (else left): the marker's section trades places with the next (previous) one, with the clips
    /// that start in them and the markers at their starts.
    MoveContent { marker: u32, right: bool },
    /// Playlist tracks `track` and `with` (1-based) change places, with their clips.
    SwapPlaylistTracks { track: u16, with: u16 },
    /// An empty playlist track at `at` (1-based); the ones from there move down (the last track's clips stay on it).
    InsertPlaylistTrack { at: u16 },
    /// Playlist track `track` goes with its clips; the ones below move up and an empty one comes in last.
    DeletePlaylistTrack { track: u16 },
    /// Playlist track `track` back to a new track's settings (its clips stay).
    ResetPlaylistTrack { track: u16 },

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
        /// A name typed for it (empty, or its plugin's name: none).
        #[serde(default)]
        name: Option<String>,
        /// `#rrggbb`, empty for none.
        #[serde(default)]
        color: Option<String>,
        /// An icon's name as the Change icon... list names it, empty for none.
        #[serde(default)]
        icon: Option<String>,
        #[serde(default)]
        auto_bypass: Option<bool>,
        #[serde(default)]
        threaded: Option<bool>,
    },
    /// Wrap in container on slot `slot` of the track (`all`: Wrap all in container, every effect of the track): the
    /// effects become members of a container in that slot, with their states, and what named their parameters
    /// (automation, recorded events, links) follows them. The result says what it did.
    WrapEffects {
        track: u32,
        slot: u8,
        #[serde(default)]
        all: bool,
    },
    /// Wrap in container on the channel's instrument: it becomes the first member of a container on the channel, which
    /// takes a new id (the result names it).
    WrapInstrument { channel: u32 },
    /// Member `member` of the container to place `to` (0 = first) of its list.
    MoveMember { container: u32, member: u32, to: usize },
    /// Member `member` goes from the container.
    RemoveMember { container: u32, member: u32 },
    /// Plugin `plugin` (its identity, else its name; an instrument with `instrument`, in a channel's container only)
    /// at the end of the container, from its defaults: an effect wet only, an instrument at the container's instrument
    /// levels. The result names its member id.
    AddMember {
        container: u32,
        plugin: String,
        #[serde(default)]
        instrument: bool,
    },
    /// Member `member` plays plugin `plugin` (as `add_member` names it) from its defaults, keeping its place, id and
    /// levels; its name, colour, icon and switches go back to a new plugin's.
    ReplaceMember {
        container: u32,
        member: u32,
        plugin: String,
        #[serde(default)]
        instrument: bool,
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
/// = centre), fine pitch 0..=240 (120 = none), release, mod X and Y 0..=128, repeat 0..=14 (0 = off), colour group
/// 0..=15. In `set_notes`, a note named by the id of a note the pattern holds keeps that note's values for what is not
/// given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteIn {
    pub channel: u32,
    pub position: u32,
    #[serde(default)]
    pub length: u32,
    #[serde(default = "middle_c")]
    pub key: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub velocity: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pan: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fine_pitch: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mod_x: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mod_y: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slide: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<u8>,
    /// Its own portamento switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub porta: Option<bool>,
    /// The stored record's other bits, kept for a copy: its flags, its byte 19 and bytes 14-15 (before the values above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<[u16; 3]>,
    /// `set_notes`: the note's id as the notes query names it, kept when the pattern held a note with it; a note
    /// without one, or with another, gets a new id. `add_notes` takes none: its notes are new.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
}

/// The container named by `container`'s place, its members, and member `member`'s index in them.
fn member_at(s: &crate::session::Session, container: u32, member: u32) -> Result<(u16, u8, Vec<crate::container::Member>, usize), Error> {
    let (t, k) = crate::edit::container::id_place(s, container).ok_or_else(|| format!("no container {container}"))?;
    let list = crate::edit::container::members(s, t, k).ok_or_else(|| format!("no container {container}"))?;
    let i = list.iter().position(|m| m.id == member).ok_or_else(|| format!("the container has no member {member}"))?;
    Ok((t, k, list, i))
}

/// The identity and name of the plugin `plugin` names (its identity, else its name): an effect, or an instrument.
fn plugin_named(s: &crate::session::Session, plugin: &str, instrument: bool) -> Result<(String, String), Error> {
    let list = if instrument { &s.plugins.available } else { &s.plugins.effects };
    let a = list.iter().find(|a| a.desc.uid() == plugin).or_else(|| list.iter().find(|a| a.desc.name == plugin)).ok_or_else(|| format!("no {} {plugin}", if instrument { "instrument" } else { "effect" }))?;
    Ok((a.desc.uid(), a.desc.name.clone()))
}

/// A time marker as commands give it: its position, name and action (`SetMarker`'s kinds), a time signature's
/// numerator and denominator, a key marker's root (0 = C) and scale (its place in `os_model::SCALES`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerIn {
    pub position: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub kind: Option<u8>,
    #[serde(default)]
    pub numerator: Option<u8>,
    #[serde(default)]
    pub denominator: Option<u8>,
    #[serde(default)]
    pub root: Option<u8>,
    #[serde(default)]
    pub scale: Option<i32>,
    /// `set_markers`: the marker's id, kept when the arrangement held a marker with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
}

impl MarkerIn {
    /// The marker as the arrangement keeps it.
    pub fn made(&self) -> Result<os_model::TimeMarker, Error> {
        use os_model::TimeMarker;
        ticks("position", self.position)?;
        let mut m = match (self.root, self.scale) {
            (Some(r), Some(k)) if r < 12 => TimeMarker::key(self.position, r, k),
            (Some(_), Some(_)) => return Err("a key marker's root is 0..=11".into()),
            _ => TimeMarker::new(self.position, self.name.as_deref().unwrap_or("Marker")),
        };
        if let Some(n) = &self.name {
            m.name = n.clone();
        }
        if let Some(k) = self.kind {
            m.kind = k;
        }
        if let Some(n) = self.numerator {
            m.numerator = n;
        }
        if let Some(d) = self.denominator {
            m.denominator = d;
        }
        Ok(m)
    }

    /// Marker `m` with everything it has: what `AddMarker` makes again.
    pub fn of(m: &os_model::TimeMarker) -> MarkerIn {
        let key = m.kind == os_model::TimeMarker::KEY;
        MarkerIn {
            position: m.position,
            name: Some(m.name.clone()),
            kind: Some(m.kind),
            numerator: Some(m.numerator),
            denominator: Some(m.denominator),
            root: key.then_some(m.root),
            scale: key.then_some(m.scale_index),
            id: (m.id.0 != 0).then_some(m.id.0),
        }
    }

    /// The marker, starting from the one of `held` its id names (its values not given kept), else made new.
    pub fn made_from(&self, held: &[os_model::TimeMarker]) -> Result<os_model::TimeMarker, Error> {
        let Some(was) = self.id.and_then(|i| held.iter().find(|m| m.id.0 == i)) else {
            let mut m = self.made()?;
            m.id = Id::NONE;
            return Ok(m);
        };
        ticks("position", self.position)?;
        let mut m = was.clone();
        m.position = self.position;
        if let Some(n) = &self.name {
            m.name = n.clone();
        }
        if let Some(k) = self.kind {
            m.kind = k;
        }
        if let Some(n) = self.numerator {
            m.numerator = n;
        }
        if let Some(d) = self.denominator {
            m.denominator = d;
        }
        if let (Some(r), Some(k)) = (self.root, self.scale) {
            if (r, k) != (was.root, was.scale_index) {
                let key = os_model::TimeMarker::key(m.position, r.min(11), k);
                (m.root, m.scale, m.scale_index, m.flag) = (key.root, key.scale, key.scale_index, key.flag);
            }
        }
        Ok(m)
    }
}

/// A playlist clip as commands give it: its place (`track` 1-based), its source (a pattern by its number, which a clip
/// may name without the project holding that pattern, or an audio or automation clip channel by its id), muted, its
/// window into the source (start and end, as stored), and the record as stored (`record`: kept for a copy). In
/// `set_clips`, a clip named by its id keeps its values for what is not given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClipIn {
    pub track: u16,
    pub position: u32,
    pub length: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    /// `[]` for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Vec<f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
}

impl ClipIn {
    /// Clip `c` with everything it has (its channel by id, from `p`).
    pub fn of(c: &Clip, p: &Project) -> ClipIn {
        let (pattern, channel) = match c.source {
            ClipSource::Pattern(n) => (Some(n), None),
            ClipSource::Channel(i) => (None, p.channels.iter().find(|x| x.index == i).map(|x| x.id.0)),
        };
        ClipIn {
            track: c.track + 1,
            position: c.position,
            length: c.length,
            pattern,
            channel,
            muted: Some(c.muted),
            window: Some(c.window.map_or_else(Vec::new, |(a, b)| vec![a, b])),
            record: Some(c.raw.to_vec()),
            id: (c.id.0 != 0).then_some(c.id.0),
        }
    }

    /// The clip, starting from the one of `held` its id names, else from a new clip's record.
    pub fn made(&self, p: &Project, held: &[Clip]) -> Result<Clip, Error> {
        ticks("position", self.position)?;
        ticks("length", self.length)?;
        if self.length == 0 {
            return Err("a clip's length can't be 0".into());
        }
        if self.track == 0 || self.track > crate::edit::playlist::TRACKS {
            return Err(format!("playlist tracks are 1..={}", crate::edit::playlist::TRACKS));
        }
        let was = self.id.and_then(|i| held.iter().find(|c| c.id.0 == i));
        let source = match (self.pattern, self.channel) {
            (Some(n), None) if (1..=crate::edit::patterns::MAX).contains(&n) => ClipSource::Pattern(n),
            (Some(n), None) => return Err(format!("patterns are 1..={}, not {n}", crate::edit::patterns::MAX)),
            (None, Some(c)) => ClipSource::Channel(p.channels[channel_row(p, c)?].index),
            (None, None) if was.is_some() => was.map(|c| c.source).unwrap_or(ClipSource::Pattern(1)),
            _ => return Err("a clip has a pattern or a channel".into()),
        };
        let mut c = was.copied().unwrap_or_else(|| Clip::new(self.position, self.length, self.track - 1, source));
        (c.position, c.length, c.track, c.source) = (self.position, self.length, self.track - 1, source);
        if was.is_none() {
            c.id = Id::NONE;
        }
        if let Some(m) = self.muted {
            c.muted = m;
        }
        if let Some(w) = &self.window {
            c.window = match w.as_slice() {
                [] => None,
                [a, b] => Some((*a, *b)),
                _ => return Err("a clip's window is [start, end], or [] for none".into()),
            };
        }
        if let Some(r) = &self.record {
            if r.len() != c.raw.len() {
                return Err(format!("a clip's record is {} bytes", c.raw.len()));
            }
            c.raw.copy_from_slice(r);
        }
        Ok(c)
    }
}

/// An automation clip's point as commands give it: its beat (from the clip's start), value 0..=1, tension -1..=1,
/// the shape of the segment it ends (`os_model::AutoMode`'s stored byte) and the stored flag; in `set_points`, a point
/// named by its id keeps its values for what is not given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointIn {
    pub beat: f64,
    pub value: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tension: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
}

impl PointIn {
    /// Point `p` with everything it has.
    pub fn of(p: &os_model::AutoPoint) -> PointIn {
        PointIn { beat: p.beat, value: p.value, tension: Some(p.tension), mode: Some(p.mode.byte()), flag: Some(p.flag), id: (p.id.0 != 0).then_some(p.id.0) }
    }
}

/// The automation clip on channel `id`.
fn auto_of(p: &Project, id: u32) -> Result<&os_model::Automation, Error> {
    let index = p.channels[channel_row(p, id)?].index;
    p.automation(index).ok_or_else(|| format!("channel {id} has no automation clip"))
}

/// Arrangement `id`.
fn arrangement_of(p: &Project, id: u32) -> Result<&os_model::Arrangement, Error> {
    p.arrangements.iter().find(|a| a.id.0 == id).ok_or_else(|| format!("no arrangement {id}"))
}

/// Marker `id`'s place in the shown arrangement's list.
fn marker_place(s: &crate::session::Session, id: u32) -> Result<usize, Error> {
    crate::edit::playlist::markers(s).iter().position(|m| m.id.0 == id).ok_or_else(|| format!("no marker {id}"))
}

/// Playlist track `track` (1-based) as the arrangement indexes it (0-based).
fn playlist_track(track: u16) -> Result<u16, Error> {
    if track == 0 || track > crate::edit::playlist::TRACKS {
        return Err(format!("playlist tracks are 1..={}", crate::edit::playlist::TRACKS));
    }
    Ok(track - 1)
}

/// A channel's filter group (`Command::SetGroups`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupIn {
    pub channel: u32,
    #[serde(default)]
    pub group: Option<u32>,
}

/// A track's clip lock (`Command::SetPlaylistTrack`): a pattern by number or a clip channel by id, neither for none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockIn {
    #[serde(default)]
    pub pattern: Option<u16>,
    #[serde(default)]
    pub channel: Option<u32>,
}

/// A playlist track's link (`Command::SetPlaylistTrack`): a mixer track or a channel by id, neither for none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkIn {
    #[serde(default)]
    pub mixer: Option<u32>,
    #[serde(default)]
    pub channel: Option<u32>,
}

/// A pattern's control event (`Command::SetControls`): at `position`, channel `channel` (an id) and `param` (0
/// volume, 1 pan, 4 pitch), or the stored `kind` for an event kept as read; `value` volume and pan 0..=12800 (pan
/// centre 6400), pitch in cents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlIn {
    pub position: u32,
    pub value: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub param: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
}

/// A channel's loop value (`Command::SetLoops`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoopIn {
    pub channel: u32,
    #[serde(default)]
    pub value: Option<u32>,
}

impl NoteIn {
    /// Note `n` of channel `channel` (an id) with every value it has: what `set_notes` puts back as it was.
    pub fn of(n: &Note, channel: u32) -> NoteIn {
        NoteIn {
            channel,
            position: n.position,
            length: n.length,
            key: n.key,
            velocity: Some(n.velocity),
            pan: Some(n.pan),
            fine_pitch: Some(n.fine_pitch),
            release: Some(n.release),
            mod_x: Some(n.mod_x),
            mod_y: Some(n.mod_y),
            repeat: Some(n.repeat),
            slide: Some(n.slide()),
            muted: Some(n.muted()),
            color: Some(n.group & 0x0f),
            porta: Some(n.porta()),
            raw: Some([n.flags, u16::from(n.group), n.unk14]),
            id: (n.id.0 != 0).then_some(n.id.0),
        }
    }
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
    /// Which channels have their mute state locked, by id (a client's `set_channel` locking one: the step's own undo).
    Locks(Vec<(u32, bool)>),
    /// An automation clip's points, by its channel's id.
    Points(u32, Vec<os_model::AutoPoint>),
    /// A channel as it was before its instrument was replaced, and the id it has now (`Command::ReplaceInstrument`).
    Swap(Box<os_model::Channel>, u32),
    /// Which channels are zipped, by id (as `Locks`).
    Zips(Vec<(u32, bool)>),
    /// The filter groups: their names, each channel's group by id, the filter shown.
    Groups(Box<(Vec<String>, Vec<(u32, Option<u32>)>, Option<u32>)>),
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
    /// `.0` put back, then the mixer tracks' arm switches by id as `.1` has them, and the mute lights of the tracks
    /// that were not there before (adding or deleting a track moves the others and makes or takes away the one before
    /// the hidden last track), as a whole project put back does (`Mutes::keep`). Its other side is the same with them
    /// as they are.
    WithOn(Box<Taken>, Vec<(u32, bool, bool)>),
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
            Command::AddAudioClip { .. } => "Add audio clip",
            Command::SetAutoCrossfades { .. } => "Automatic crossfades",
            Command::CloneChannel { .. } => "Clone channel",
            Command::ReplaceInstrument { .. } => crate::history::REPLACE_CHANNEL,
            Command::WrapEffects { all: false, .. } | Command::WrapInstrument { .. } => crate::container::WRAP,
            Command::WrapEffects { all: true, .. } => crate::container::WRAP_ALL,
            Command::DeleteChannel { .. } => "Delete channel",
            Command::MoveChannel { .. } => "Move channel",
            Command::SetChannel { .. } => "Channel settings",
            Command::LoadSample { .. } => crate::history::LOAD_SAMPLE,
            Command::LoadChannelState { .. } => "Load channel state",
            Command::AddTrack { .. } => "Insert mixer track",
            Command::DeleteTrack { .. } => "Delete mixer track",
            Command::SetTrack { .. } => "Mixer track settings",
            Command::ResetTrack { .. } => "Mixer track reset",
            Command::RemoveTracks { .. } => "Delete mixer track",
            Command::MoveEffect { .. } => "Move effect",
            Command::MoveTrack { .. } => "Move mixer track",
            Command::LoadTrackState { .. } => "Open mixer track state",
            Command::SetAutoBypass { .. } => "Auto-bypass for all plugins",
            Command::SetSend { .. } => "Mixer routing",
            Command::SetEq { .. } => "Mixer track equalizer",
            Command::AddEffect { .. } => "Add effect",
            Command::RemoveEffect { .. } => "Remove effect",
            Command::SetSlot { .. } => "Effect slot",
            Command::AddPattern { .. } => "Add pattern",
            Command::CopyPattern { .. } => "Make unique",
            Command::ShiftPatterns { by, .. } => if *by > 0 { "Insert pattern" } else { "Delete pattern" },
            Command::MovePattern { .. } => "Move pattern",
            Command::ClonePattern { .. } => "Clone pattern",
            Command::TransposePattern { .. } => "Transpose",
            Command::SplitPattern { .. } => "Split by instrument",
            Command::SetPatternSignature { .. } => "Pattern time signature",
            Command::SetPatternMarkers { .. } => "Pattern time markers",
            Command::DeletePattern { .. } => "Delete pattern",
            Command::SetPattern { .. } => "Pattern settings",
            Command::SetNotes { .. } | Command::AddNotes { .. } => "Edit notes",
            Command::SetStep { .. } => crate::history::STEP_SEQ_EDIT,
            Command::SetControls { .. } => "Edit events",
            Command::SetLoops { .. } => crate::history::LOOP_MODE,
            Command::SetGroups { .. } => "Filter groups",
            Command::AddClip { .. } => "Add clip",
            Command::DeleteClip { .. } => "Delete clip",
            Command::SetClip { .. } => "Edit clip",
            Command::SetPlaylistTrack { .. } => "Playlist track",
            Command::AddMarker { .. } | Command::SetMarker { .. } | Command::DeleteMarker { .. } => crate::edit::playlist::ARRANGER_EDIT,
            Command::SetClips { .. } | Command::SetMarkers { .. } => crate::edit::playlist::ARRANGER_EDIT,
            Command::AddArrangement { .. } => "Add arrangement",
            Command::SetPoints { .. } => crate::edit::playlist::ARRANGER_EDIT,
            Command::DeleteArrangement { .. } => "Delete arrangement",
            Command::SetArrangement { .. } => "Arrangement name",
            Command::MoveContent { .. } => crate::edit::playlist::ARRANGER_EDIT,
            Command::SwapPlaylistTracks { .. } | Command::InsertPlaylistTrack { .. } | Command::DeletePlaylistTrack { .. } | Command::ResetPlaylistTrack { .. } => crate::edit::playlist::ARRANGER_EDIT,
            Command::SetClipLfo { .. } => "Automation clip LFO",
            Command::SetMidiOut { .. } => "MIDI Out setting",
            Command::SetMember { .. } => crate::container::LEVEL,
            Command::MoveMember { .. } => crate::container::MOVE,
            Command::RemoveMember { .. } => crate::container::REMOVE,
            Command::AddMember { instrument: false, .. } => crate::container::ADD,
            Command::AddMember { instrument: true, .. } => crate::container::ADD_INSTRUMENT,
            Command::ReplaceMember { .. } => crate::container::REPLACE,
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
            audio_in: None,
            audio_out: None,
            rec_source: None,
            monitor: None,
            armed: None,
        }
    }

    /// A `SetProject` that sets nothing yet.
    /// `SetChannel` on channel `channel` setting nothing yet.
    pub fn set_channel(channel: u32) -> Command {
        Command::SetChannel {
            channel,
            name: None,
            color: None,
            volume: None,
            pan: None,
            pitch: None,
            enabled: None,
            track: None,
            declick: None,
            swing: None,
            truncate_swing: None,
            cut: None,
            cut_by: None,
            locked: None,
            stretch: None,
            zipped: None,
            auto_bypass: None,
            threaded: None,
            pitch_range: None,
        }
    }

    pub fn set_project() -> Command {
        Command::SetProject { beats: None, steps: None, signature: None, panning_law: None, play_truncated: None, pdc: None }
    }

    /// A command that adds a thing: its undo takes it away, so applying it always changes the project.
    pub fn adds(&self) -> bool {
        matches!(self, Command::AddChannel { .. } | Command::CloneChannel { .. } | Command::LoadChannelState { .. } | Command::AddTrack { .. } | Command::AddPattern { .. } | Command::CopyPattern { .. } | Command::AddClip { .. })
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
            Command::ShiftPatterns { .. } | Command::MovePattern { .. } | Command::ClonePattern { .. } | Command::SplitPattern { .. } | Command::SetPatternSignature { .. } | Command::SetPatternMarkers { .. } => {
                Taken::Project(Box::new(Side::of(s)))
            }
            Command::SetPattern { pattern, .. } | Command::SetNotes { pattern, .. } | Command::AddNotes { pattern, .. } | Command::SetStep { pattern, .. } | Command::SetLoops { pattern, .. } | Command::SetControls { pattern, .. } | Command::TransposePattern { pattern, .. } => {
                Taken::Pattern(Box::new(p.patterns[pattern_place(p, *pattern)?].clone()))
            }
            Command::AddClip { .. }
            | Command::DeleteClip { .. }
            | Command::SetClip { .. }
            | Command::SetPlaylistTrack { .. }
            | Command::SwapPlaylistTracks { .. }
            | Command::InsertPlaylistTrack { .. }
            | Command::DeletePlaylistTrack { .. }
            | Command::ResetPlaylistTrack { .. }
            | Command::SetClips { .. }
            | Command::SetMarkers { .. }
            | Command::AddMarker { .. }
            | Command::SetMarker { .. }
            | Command::DeleteMarker { .. }
            | Command::MoveContent { .. } => match p.arrangement() {
                Some(a) => Taken::Arrangement(Box::new(a.clone())),
                // A project without an arrangement gets one (`playlist::arr_mut`); the undo takes it away again.
                None => Taken::Project(Box::new(Side::of(s))),
            },
            // A slot moved takes the automation and remote control links on it along (`edit::mixer::follow_places`).
            Command::SetAutoBypass { .. } | Command::RemoveTracks { .. } | Command::MoveTrack { .. } | Command::MoveEffect { .. } => Taken::Project(Box::new(Side::of(s))),
            // A channel and a clip, and maybe a mixer track's routing.
            Command::AddAudioClip { .. } => Taken::Project(Box::new(Side::of(s))),
            // Every arrangement's clips.
            Command::SetAutoCrossfades { .. } => Taken::Project(Box::new(Side::of(s))),
            // The arrangements' list, and which one is shown.
            Command::AddArrangement { .. } | Command::DeleteArrangement { .. } => Taken::Project(Box::new(Side::of(s))),
            Command::SetArrangement { arrangement, .. } => Taken::Arrangement(Box::new(arrangement_of(p, *arrangement)?.clone())),
            Command::SetPoints { channel, .. } => Taken::Points(*channel, auto_of(p, *channel)?.points.clone()),
            // A wrap moves plugins and what names them across the project.
            Command::WrapEffects { .. } | Command::WrapInstrument { .. } => Taken::Project(Box::new(Side::of(s))),
            Command::LoadTrackState { track, .. } => Taken::Track(Box::new(p.mixer[track_place(p, *track)?].clone())),
            Command::SetClipLfo { channel, .. } => Taken::Lfo(*channel, Box::new(clip_lfo(p, *channel)?.clone())),
            Command::SetMidiOut { channel, .. } => Taken::Channel(Box::new(p.channels[channel_row(p, *channel)?].clone())),
            Command::SetMember { container, .. }
            | Command::MoveMember { container, .. }
            | Command::RemoveMember { container, .. }
            | Command::AddMember { container, .. }
            | Command::ReplaceMember { container, .. } => match crate::edit::container::id_place(s, *container).ok_or_else(|| format!("no container {container}"))? {
                (row, crate::container::CHANNEL) => Taken::Channel(Box::new(p.channels[usize::from(row)].clone())),
                (t, _) => Taken::Track(Box::new(p.mixer.iter().find(|m| m.index == t).ok_or("the container's track is gone")?.clone())),
            },
            Command::SetParam { owner, param, from, .. } => {
                Taken::Param(*owner, *param, from.ok_or("the parameter's value before is not known (the app fills it in)")?)
            }
            Command::MoveChannel { channel, .. } => Taken::Row(*channel, channel_row(p, *channel)?),
            // The id it has after is known once it is replaced (`apply`).
            Command::ReplaceInstrument { channel, .. } => Taken::Swap(Box::new(p.channels[channel_row(p, *channel)?].clone()), *channel),
            Command::SetGroups { .. } => groups(p),
            // What an add made is known once it is made (`apply`); this is not used.
            Command::AddChannel { .. } | Command::CloneChannel { .. } | Command::LoadChannelState { .. } | Command::AddTrack { .. } | Command::AddPattern { .. } | Command::CopyPattern { .. } => Taken::Gone(Thing::Channel(0)),
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
                Taken::Locks(_) => locks(p),
                Taken::Zips(_) => zips(p),
                Taken::Groups(_) => groups(p),
                Taken::TracksOn(_) => tracks_on(p),
                Taken::WithOn(t, _) => {
                    let inner = Command::Put(t.clone()).take(s)?;
                    Taken::WithOn(Box::new(inner), lights(p))
                }
                // The other way: the links back onto what they were on.
                Taken::Links(pairs) => Taken::Links(pairs.iter().map(|&(a, b)| (b, a)).collect()),
                // A channel with an automation clip comes back with it, as a deleted one does.
                Taken::Gone(Thing::Channel(id)) if channel_row(p, *id).is_ok_and(|r| p.automation(p.channels[r].index).is_some()) => {
                    Taken::Removed(Box::new(super::removal::Removed::of(s, Thing::Channel(*id))?))
                }
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
                Taken::Swap(c, now) => Taken::Swap(Box::new(p.channels[channel_row(p, *now)?].clone()), c.id.0),
                Taken::Points(ch, _) => Taken::Points(*ch, auto_of(p, *ch)?.points.clone()),
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
            Taken::WithOn(inner, _) => {
                let mut c = Command::Put(std::mem::replace(inner, Box::new(Taken::Mutes(Vec::new()))));
                c.rescale(from, to);
                if let Command::Put(t) = c {
                    *inner = t;
                }
            }
            Taken::Settings(_)
            | Taken::Lfo(..)
            | Taken::Channel(_)
            | Taken::Track(_)
            | Taken::Param(..)
            | Taken::Plugin(..)
            | Taken::Mutes(_)
            | Taken::Locks(_)
            | Taken::Zips(_)
            | Taken::Groups(_)
            | Taken::TracksOn(_)
            | Taken::Links(_)
            | Taken::Gone(_)
            | Taken::Deleted(..)
            | Taken::Row(..)
            | Taken::Points(..)
            | Taken::Swap(..) => {}
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
            Command::AddChannel { .. } | Command::CloneChannel { .. } | Command::LoadChannelState { .. } => Some(Thing::Channel as fn(u32) -> Thing),
            Command::AddTrack { .. } => Some(Thing::Track as fn(u32) -> Thing),
            Command::AddPattern { .. } | Command::CopyPattern { .. } => Some(Thing::Pattern as fn(u32) -> Thing),
            _ => None,
        };
        let replace = matches!(self, Command::ReplaceInstrument { .. });
        // A track added or deleted: the tracks' lights go back with it.
        let on = matches!(self, Command::AddTrack { .. } | Command::DeleteTrack { .. }).then(|| lights(&s.project));
        // The remote control links on rack rows follow a command that moves rows (an undo puts them back as
        // `Taken::Links`, not by following the rows again).
        let rows_before = matches!(self, Command::MoveChannel { .. } | Command::DeleteChannel { .. } | Command::CloneChannel { .. })
            .then(|| s.project.channels.iter().map(|c| c.id.0).collect::<Vec<u32>>());
        let result = self.change(s)?;
        if let Some(before) = rows_before {
            let now: std::collections::HashMap<u32, usize> = s.project.channels.iter().enumerate().map(|(r, c)| (c.id.0, r)).collect();
            let map: Vec<Option<usize>> = before.iter().map(|id| now.get(id).copied()).collect();
            if map.iter().enumerate().any(|(r, &m)| m != Some(r)) {
                crate::edit::rack::follow_links(s, &map);
            }
        }
        s.dirty = true;
        // An add's undo removes what it made, by the id its result names.
        let taken = match added {
            Some(thing) => {
                let id = result.as_object().and_then(|o| o.values().next()).and_then(serde_json::Value::as_u64).ok_or("an add names no id")?;
                Taken::Gone(thing(id as u32))
            }
            None => match taken {
                // A replaced channel's new id, from the result.
                Taken::Swap(c, _) if replace => Taken::Swap(c, result["channel"].as_u64().ok_or("a replace names no id")? as u32),
                t => t,
            },
        };
        let taken = match on {
            Some(on) => Taken::WithOn(Box::new(taken), on),
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

            Command::AddChannel { name, plugin, sample, track, kind: Some(kind), color, target, points, children } => {
                if plugin.is_some() || (sample.is_some() && kind != "audio_clip") {
                    return Err("a channel of a kind has no plugin, nor a sample unless an audio clip's".into());
                }
                let track = track.map(|t| track_number(&s.project, t)).transpose()?;
                let color = color.map(|c| parse_color(&c)).transpose()?.flatten();
                let index = s.project.channels.iter().map(|c| c.index + 1).max().unwrap_or(0);
                let (kind, volume, pan) = match kind.as_str() {
                    "audio_clip" => (ChannelKind::AudioClip, 0.78125, 0.0),
                    // An automation clip's range 0..1: its pan knob is the minimum (spec: audio/automation.md "Range").
                    "automation" => (ChannelKind::Automation, 1.0, -1.0),
                    "layer" => (ChannelKind::Layer, 1.0, 0.0),
                    k => return Err(format!("no channel kind {k}")),
                };
                let layer_children = children.iter().map(|&c| channel_row(&s.project, c).map(|r| s.project.channels[r].index)).collect::<Result<Vec<u16>, _>>()?;
                let auto = if kind == ChannelKind::Automation {
                    let mut pts = points.iter().map(|q| os_model::AutoPoint { tension: q.tension.unwrap_or(0.0), mode: q.mode.map_or(os_model::AutoMode::Single, os_model::AutoMode::from_byte), flag: q.flag.unwrap_or(0), ..os_model::AutoPoint::new(q.beat, q.value) }).collect::<Vec<_>>();
                    pts.sort_by(|a, b| a.beat.total_cmp(&b.beat));
                    let beats = pts.last().map_or(4.0, |p| p.beat);
                    let first = pts.first().map_or(0.5, |p| p.value);
                    let mut a = match target {
                        Some(t) => os_flp::project::new_automation(index, t, first, beats),
                        None => os_flp::project::untargeted_automation(index, 0.5, 4.0),
                    };
                    if pts.len() >= 2 {
                        a.points = pts;
                    }
                    Some(a)
                } else {
                    None
                };
                let ch = os_model::Channel {
                    id: Id::NONE,
                    index,
                    kind,
                    name: name.as_deref().and_then(text),
                    plugin: None,
                    plugin_state: None,
                    color,
                    enabled: true,
                    volume,
                    pan,
                    pitch_cents: 0,
                    mixer_track: track.unwrap_or(0),
                    sample_path: sample.clone().filter(|p| !p.is_empty()),
                    layer_children,
                    envelopes: Default::default(),
                    rack: Default::default(),
                    misc: Default::default(),
                    sound: if kind == ChannelKind::AudioClip { os_model::ChannelSound { sampler_flags: Some(os_model::ChannelSound::AUDIO_CLIP_FLAGS), ..Default::default() } } else { Default::default() },
                    lemon_icon: None,
                };
                s.project.get_mut().channels.push(ch);
                if let Some(a) = auto {
                    s.project.get_mut().automations.push(a);
                }
                s.touched.views.push(crate::session::View::RackAutosize);
                s.control.settle(s.project.get_mut());
                let id = s.project.channels.last().map_or(Id::NONE, |c| c.id);
                Ok(id_result("channel", id))
            }
            Command::SetAutoCrossfades { on } => {
                s.project.get_mut().auto_crossfades = Some(on);
                crate::edit::playlist::crossfades_switched(s);
                s.touched.clips = true;
                none
            }
            Command::AddAudioClip { path, track, position, mixer } => {
                ticks("position", position)?;
                if track == 0 || track > crate::edit::playlist::TRACKS {
                    return Err(format!("playlist tracks are 1..={}", crate::edit::playlist::TRACKS));
                }
                let mixer = mixer.map(|m| track_number(&s.project, m)).transpose()?;
                let i = crate::edit::playlist::add_audio_clip_on(s, &path, track - 1, position, mixer, None).ok_or_else(|| format!("{path} can't be read as audio"))?;
                s.control.settle(s.project.get_mut());
                let c = crate::edit::playlist::clips(s)[i];
                let ch = match c.source {
                    ClipSource::Channel(ix) => s.project.channels.iter().find(|x| x.index == ix).map_or(0, |x| x.id.0),
                    ClipSource::Pattern(_) => 0,
                };
                // The views select the new clip (`View::AudioClipAdded`).
                Ok(serde_json::json!({ "channel": ch, "clip": c.id.0 }))
            }
            Command::AddChannel { name, plugin, sample, track, .. } => {
                let track = track.map(|t| track_number(&s.project, t)).transpose()?;
                let sampler = plugin.as_deref().is_none_or(|p| p == crate::plugins::LEMON_SAMPLER);
                if !sampler && sample.is_some() {
                    return Err("a sample goes into a sampler channel (no plugin)".into());
                }
                let midi_out = plugin.as_deref() == Some(crate::lemon_midi_out::LEMON_MIDI_OUT);
                let row = match &plugin {
                    Some(p) if !sampler => crate::projects::add_plugin_channel(s.project.get_mut(), p),
                    _ => crate::projects::add_channel(s.project.get_mut()),
                };
                // A MIDI Out channel is hosted from its settings (`lemon_midi_out::sync_midi_outs`), on the master track; a
                // container plays its members (`sync_containers`), none yet.
                if midi_out {
                    crate::lemon_midi_out::make_midi_out(&mut s.project.get_mut().channels[row], &Default::default());
                }
                let container = plugin.as_deref() == Some(crate::container::CONTAINER);
                if container {
                    s.project.get_mut().channels[row].plugin_state = Some(crate::container::encode(&[]));
                }
                let ch = &mut s.project.get_mut().channels[row];
                if let Some(t) = track {
                    ch.mixer_track = t;
                }
                if let Some(path) = sample.filter(|p| !p.is_empty()) {
                    crate::edit::rack::auto_name(&s.settings, ch, std::path::Path::new(&path));
                    crate::edit::rack::read_tags(&s.settings, ch, std::path::Path::new(&path));
                    ch.sample_path = Some(path.clone());
                    s.sample_loads.push((row, path));
                }
                if let Some(n) = name {
                    s.project.get_mut().channels[row].name = text(&n);
                }
                s.touched.views.push(crate::session::View::RackAutosize);
                s.control.settle(s.project.get_mut());
                let id = s.project.channels[row].id;
                if !sampler && !midi_out && !container {
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
            Command::ReplaceInstrument { channel, plugin } => {
                let row = channel_row(&s.project, channel)?;
                let sampler = plugin.as_deref().is_none_or(|p| p == crate::plugins::LEMON_SAMPLER);
                crate::project_plugins::drop_row(s, row);
                let ch = &mut s.project.get_mut().channels[row];
                ch.plugin_state = None;
                // A new id at once, before the new instance is hosted and takes its channel's.
                ch.id = Id::NONE;
                let name = plugin.clone().unwrap_or_else(|| crate::plugins::LEMON_SAMPLER.to_string());
                let midi_out = name == crate::lemon_midi_out::LEMON_MIDI_OUT;
                if midi_out {
                    // Hosted from its settings, on the master track; the channel keeps its name (Set up for MIDI
                    // export).
                    crate::lemon_midi_out::make_midi_out(ch, &Default::default());
                } else if sampler {
                    ch.kind = os_model::ChannelKind::Sampler;
                    ch.plugin = None;
                    ch.clear_sample();
                } else {
                    ch.kind = os_model::ChannelKind::Plugin;
                    ch.plugin = Some(crate::osc3::file_name(&name).to_string());
                    // A Lemon Trio channel's sample is its custom shape's (spec: plugins/lemon-3x-osc.md §6): the new
                    // instance has none, so neither does the channel.
                    if crate::osc3::is_osc3(ch) {
                        ch.sample_path = None;
                    }
                }
                if !midi_out {
                    ch.name = Some(name.clone());
                }
                s.control.settle(s.project.get_mut());
                let id = s.project.channels[row].id;
                if !sampler && !midi_out {
                    s.control.host.push(super::HostReq::Instrument { channel: id, plugin: name });
                }
                Ok(id_result("channel", id))
            }
            Command::CloneChannel { channel, at_end } => {
                let row = channel_row(&s.project, channel)?;
                let new = crate::edit::rack::clone_channel_in(s, row, at_end);
                s.control.settle(s.project.get_mut());
                let id = s.project.channels[new].id;
                // Its instrument: a copy of the original's state, hosted on the next tick (the original keeps its row).
                s.plugins.stash_copy(row, id);
                Ok(id_result("channel", id))
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
            Command::SetChannel { channel, name, color, volume, pan, pitch, enabled, track, declick, swing, truncate_swing, cut, cut_by, locked, stretch, zipped, auto_bypass, threaded, pitch_range } => {
                let track = track.map(|t| track_number(&s.project, t)).transpose()?;
                if pitch_range.is_some_and(|r| !(1..=48).contains(&r)) {
                    return Err("pitch_range is 1..=48".into());
                }
                if declick.is_some_and(|d| d > crate::edit::rack::DECLICK_MAX) {
                    return Err(format!("declick is 0..={}", crate::edit::rack::DECLICK_MAX));
                }
                let swing = swing.map(|v| unit("swing", v, 0.0, 1.0)).transpose()?;
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
                    // The colour the file's event holds (what the playlist draws audio clips with) follows.
                    if ch.color.is_some() {
                        ch.rack.stored_color = ch.color;
                    }
                }
                if let Some(v) = volume {
                    ch.volume = v;
                }
                if let Some(v) = pan {
                    ch.pan = v;
                }
                if let Some(r) = pitch_range {
                    ch.misc.pitch_range = r;
                }
                if let Some(v) = pitch {
                    ch.pitch_cents = v;
                }
                if pitch_range.is_some() {
                    let lim = 100 * i32::from(ch.misc.pitch_range);
                    ch.pitch_cents = ch.pitch_cents.clamp(-lim, lim);
                }
                if let Some(v) = enabled {
                    ch.enabled = v;
                }
                if let Some(t) = track {
                    ch.mixer_track = t;
                }
                // Steps of 1/128, as the file keeps it.
                if let Some(v) = swing {
                    ch.misc.swing = ((v * 128.0).round() / 128.0) as f32;
                }
                if let Some(v) = truncate_swing {
                    ch.misc.truncate_swing = v;
                }
                if let Some(v) = cut {
                    ch.misc.cut = v;
                }
                if let Some(v) = cut_by {
                    ch.misc.cut_by = v;
                }
                if let Some(v) = locked {
                    ch.rack.locked = v;
                }
                if let Some(v) = zipped {
                    ch.rack.zipped = v;
                }
                if let Some(v) = auto_bypass {
                    ch.misc.cpu.smart_disable = v;
                }
                if let Some(v) = threaded {
                    ch.misc.cpu.threaded = v;
                }
                let sampler = ch.kind.is_sampler();
                if let Some(v) = stretch {
                    ch.sound.stretch[3] = v;
                }
                let sound = ch.sound.clone();
                if let Some(v) = declick {
                    s.project.get_mut().channels[row].sound.declick = v;
                    if sampler {
                        crate::edit::rack::sync_declick(s, row);
                    }
                }
                if stretch.is_some() && sampler {
                    s.plugins.set_sampler_stretch(row, &sound);
                }
                if sampler && (swing.is_some() || truncate_swing.is_some() || cut.is_some() || cut_by.is_some()) {
                    s.control.misc.push(row);
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
            Command::SetTrack { track, name, color, volume, pan, stereo, enabled, polarity, swap, effects_enabled, separator, dock, threaded, latency_in, latency_out, audio_in, audio_out, rec_source, monitor, armed } => {
                let monitor = match monitor.as_deref() {
                    None => None,
                    Some("off") => Some(os_model::MONITOR_OFF),
                    Some("armed") => Some(os_model::MONITOR_ARMED),
                    Some("on") => Some(os_model::MONITOR_ON),
                    Some(m) => return Err(format!("monitor must be off, armed or on, not {m}")),
                };
                if rec_source.is_some_and(|v| v > 5) {
                    return Err("rec_source must be 0..=5".into());
                }
                if audio_in.into_iter().chain(audio_out).any(|v| v < -1) {
                    return Err("audio_in and audio_out must be -1 or more".into());
                }
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
                if let Some(v) = audio_in {
                    m.audio_in = v;
                }
                if let Some(v) = audio_out {
                    m.audio_out = v;
                }
                if let Some(v) = rec_source {
                    m.rec_source = v;
                }
                if let Some(v) = monitor {
                    m.monitor = v;
                }
                if let Some(v) = armed {
                    m.armed = v;
                }
                none
            }
            Command::RemoveTracks { tracks } => {
                let gone = tracks.iter().map(|&t| track_number(&s.project, t)).collect::<Result<Vec<_>, _>>()?;
                if gone.contains(&0) {
                    return Err("the master track can't be deleted".into());
                }
                crate::edit::mixer::remove_tracks_in(s, &gone);
                none
            }
            Command::MoveTrack { track, to } => {
                let mut n = track_number(&s.project, track)?;
                let last = crate::edit::mixer::inserts(s);
                if n == 0 {
                    return Err("the master track doesn't move".into());
                }
                if !(1..=last).contains(&to) {
                    return Err(format!("a track moves to 1..={last}"));
                }
                // One place at a time, as Move left / right do.
                while n != to {
                    let o = if to > n { n + 1 } else { n - 1 };
                    let m = move |t: u16| if t == n { o } else if t == o { n } else { t };
                    crate::edit::mixer::renumber(s, &m, n.min(o), n.max(o) + 1, 0);
                    n = o;
                }
                none
            }
            Command::LoadChannelState { path, name } => {
                let path = std::path::PathBuf::from(path);
                let mut ch = crate::edit::rack::read_channel_state(&path).map_err(|e| format!("couldn't read the channel state {}: {e}", path.display()))?;
                ch.id = os_model::Id::NONE;
                if let Some(n) = name {
                    ch.name = text(&n);
                }
                if ch.name.as_deref().is_none_or(str::is_empty) {
                    ch.name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
                }
                let row = s.project.channels.len();
                crate::edit::rack::push_channel_in(s, ch);
                s.control.settle(s.project.get_mut());
                Ok(id_result("channel", s.project.channels[row].id))
            }
            Command::LoadTrackState { track, path, named } => {
                let n = track_number(&s.project, track)?;
                let path = std::path::PathBuf::from(path);
                if crate::mixer_state::load(&path).is_none() {
                    return Err(format!("couldn't read the mixer track state {}", path.display()));
                }
                crate::edit::mixer::open_state(s, n, &path, named);
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
                let container = plugin == crate::container::CONTAINER;
                let m = &mut s.project.get_mut().mixer[place];
                m.slots[k] = Some(crate::plugins::new_slot(&plugin));
                // A container plays its members from the next tick (`sync_containers`): nothing to host.
                if container {
                    m.slots[k] = Some(os_model::SlotPlugin { id: Id::NONE, name: plugin.clone(), state: crate::container::encode(&[]) });
                }
                m.slot_on[k] = true;
                m.slot_mix[k] = 1.0;
                m.slot_cpu[k] = os_model::PluginCpu::default();
                (m.slot_names[k], m.slot_colors[k], m.slot_icons[k]) = (None, None, None);
                s.control.settle(s.project.get_mut());
                let id = s.project.mixer[place].slots[k].as_ref().map_or(Id::NONE, |x| x.id);
                if !container {
                    s.control.host.push(super::HostReq::Effect { slot: id, plugin });
                }
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
                (m.slot_names[k], m.slot_colors[k], m.slot_icons[k]) = (None, None, None);
                // The instance goes now, its state kept for an undo that brings the slot back.
                let t = s.project.mixer[place].index;
                s.fx_requests.push(crate::edit::mixer::FxReq::Delete { track: t, slot });
                none
            }
            Command::MoveEffect { track, slot, to } => {
                if slot > 9 || to > 9 {
                    return Err(format!("slots are 0..=9, not {}", slot.max(to)));
                }
                let t = track_number(&s.project, track)?;
                crate::edit::mixer::swap_slots(s, t, slot, to);
                none
            }
            Command::SetSlot { track, slot, enabled, mix, name, color, sidechain, auto_bypass, threaded } => {
                let mix = mix.map(|v| unit("mix", v, 0.0, 1.0)).transpose()?;
                let color = color.map(|c| parse_color(&c)).transpose()?;
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
                if let Some(n) = name {
                    m.slot_names[usize::from(slot)] = text(&n);
                }
                if let Some(c) = color {
                    m.slot_colors[usize::from(slot)] = c;
                }
                if let Some(v) = auto_bypass {
                    m.slot_cpu[usize::from(slot)].smart_disable = v;
                }
                if let Some(v) = threaded {
                    m.slot_cpu[usize::from(slot)].threaded = v;
                }
                if let Some(port) = sidechain {
                    let t = m.index;
                    if !crate::project_plugins::set_slot_sidechain(s.project.get_mut(), &s.plugins, t, slot, port as usize) {
                        return Err("This effect has no sidechain input".into());
                    }
                }
                none
            }

            Command::ShiftPatterns { from, by } => {
                let max = crate::edit::patterns::MAX;
                if !(1..=max).contains(&from) || !(by == 1 || by == -1) || (by == -1 && from < 2) {
                    return Err(format!("patterns shift by 1 or -1 from 1..={max} (2.. for -1)"));
                }
                if by < 0 && s.project.pattern(from - 1).is_some_and(crate::edit::patterns::used) {
                    return Err(format!("pattern {} is in the way", from - 1));
                }
                let gap = from - 1;
                // An empty pattern in the gap goes.
                crate::edit::patterns::renumber_in(s, |n| match (n >= from, by > 0) {
                    (true, true) => Some((n + 1).min(max)),
                    (true, false) => Some(n - 1),
                    (false, false) if n == gap => None,
                    _ => Some(n),
                });
                none
            }
            Command::MovePattern { from: n, to } => {
                if !(1..=crate::edit::patterns::MAX).contains(&to) || !(1..=crate::edit::patterns::MAX).contains(&n) {
                    return Err(format!("patterns are 1..={}", crate::edit::patterns::MAX));
                }
                crate::edit::patterns::renumber_in(s, |m| Some(if m == n { to } else if m == to { n } else { m }));
                none
            }
            Command::ClonePattern { pattern } => {
                let n = s.project.patterns[pattern_place(&s.project, pattern)?].number;
                let max = crate::edit::patterns::MAX;
                if n >= max {
                    return Err("the last pattern can't be cloned".into());
                }
                let mut copy = s.project.patterns[pattern_place(&s.project, pattern)?].clone();
                copy.name = Some(crate::edit::patterns::clone_name(s, n));
                crate::edit::patterns::renumber_in(s, |m| Some(if m > n { (m + 1).min(max) } else { m }));
                copy.number = n + 1;
                copy.id = Id::NONE;
                *crate::edit::patterns::pat_mut(s, n + 1) = copy;
                s.control.settle(s.project.get_mut());
                let id = s.project.pattern(n + 1).map_or(Id::NONE, |p| p.id);
                Ok(id_result("pattern", id))
            }
            Command::TransposePattern { pattern, semitones, channels } => {
                let n = s.project.patterns[pattern_place(&s.project, pattern)?].number;
                let only = channels.iter().map(|&c| channel_row(&s.project, c).map(|r| s.project.channels[r].index)).collect::<Result<Vec<u16>, _>>()?;
                crate::edit::patterns::transpose(s, n, semitones, &only);
                none
            }
            Command::SplitPattern { pattern } => {
                let n = s.project.patterns[pattern_place(&s.project, pattern)?].number;
                crate::edit::patterns::split_by_channel(s, n);
                none
            }
            Command::SetPatternSignature { pattern, numerator, denominator } => {
                let n = s.project.patterns[pattern_place(&s.project, pattern)?].number;
                if numerator > 0 && (numerator > 16 || ![2, 4, 8, 16].contains(&denominator)) {
                    return Err(format!("not a time signature: {numerator}/{denominator}"));
                }
                crate::edit::patterns::set_signature(s, n, (numerator > 0).then_some((numerator, denominator)));
                none
            }
            Command::SetPatternMarkers { pattern, markers } => {
                let place = pattern_place(&s.project, pattern)?;
                let n = s.project.patterns[place].number;
                let old = s.project.pattern_length(n);
                let held = s.project.patterns[place].markers.clone();
                let mut list = markers.iter().map(|m| m.made_from(&held)).collect::<Result<Vec<_>, _>>()?;
                list.sort_by_key(|m| m.position);
                s.project.get_mut().patterns[place].markers = list;
                crate::edit::playlist::refit_pattern_clips(s, n, old);
                none
            }
            Command::CopyPattern { from, to, name } => {
                let max = crate::edit::patterns::MAX;
                if !(1..=max).contains(&from) || !(1..=max).contains(&to) {
                    return Err(format!("patterns are 1..={max}"));
                }
                if s.project.pattern(to).is_some() {
                    return Err(format!("pattern {to} is there already"));
                }
                let mut p = s.project.pattern(from).cloned().unwrap_or(Pattern { number: from, ..Pattern::default() });
                p.number = to;
                p.id = Id::NONE;
                p.notes.iter_mut().for_each(|n| n.id = Id::NONE);
                if let Some(n) = name {
                    p.name = text(&n);
                }
                let ps = &mut s.project.get_mut().patterns;
                let at = ps.partition_point(|q| q.number < to);
                ps.insert(at, p);
                s.control.settle(s.project.get_mut());
                let id = s.project.pattern(to).map_or(Id::NONE, |p| p.id);
                Ok(id_result("pattern", id))
            }
            Command::AddPattern { name, color, number } => {
                let color = color.map(|c| parse_color(&c)).transpose()?;
                let n = match number {
                    Some(n) if !(1..=crate::edit::patterns::MAX).contains(&n) => return Err(format!("patterns are 1..={}", crate::edit::patterns::MAX)),
                    Some(n) if s.project.pattern(n).is_some() => return Err(format!("pattern {n} is there already")),
                    Some(n) => n,
                    None => s.project.patterns.iter().map(|p| p.number).max().unwrap_or(0) + 1,
                };
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
                let place = pattern_place(&s.project, pattern)?;
                let notes = notes_of(&s.project, &notes, &s.project.patterns[place].notes)?;
                let only = channel.map(|c| channel_row(&s.project, c).map(|r| s.project.channels[r].index)).transpose()?;
                let held: std::collections::HashSet<u32> = s.project.patterns[place].notes.iter().map(|n| n.id.0).filter(|&i| i != 0).collect();
                let (notes, ids) = with_ids(s, notes, &held);
                let p = &mut s.project.get_mut().patterns[place];
                p.notes.retain(|n| only.is_some_and(|c| n.channel != c));
                p.notes.extend(notes);
                // One channel's notes merge with the others in position order; all of them keep the order given.
                if only.is_some() {
                    p.notes.sort_by_key(|n| (n.position, n.channel));
                }
                Ok(ids)
            }
            Command::SetGroups { groups, channels, shown } => {
                let n = groups.len() as u32;
                if shown.is_some_and(|g| g >= n) || channels.iter().any(|c| c.group.is_some_and(|g| g >= n)) {
                    return Err(format!("a group is 0..{n}"));
                }
                let rows = channels.iter().map(|c| channel_row(&s.project, c.channel).map(|r| (r, c.group))).collect::<Result<Vec<_>, _>>()?;
                let p = s.project.get_mut();
                p.groups = groups;
                for (r, g) in rows {
                    p.channels[r].rack.group = g;
                }
                p.current_group = shown;
                none
            }
            Command::SetControls { pattern, controls } => {
                let place = pattern_place(&s.project, pattern)?;
                let held: std::collections::HashSet<u32> = s.project.patterns[place].controls.iter().map(|c| c.id.0).filter(|&i| i != 0).collect();
                let mut seen = std::collections::HashSet::new();
                let mut list = Vec::with_capacity(controls.len());
                for c in &controls {
                    ticks("position", c.position)?;
                    let kind = match (c.kind, c.channel, c.param) {
                        (Some(k), None, None) => k,
                        (None, Some(ch), Some(param)) => u32::from(s.project.channels[channel_row(&s.project, ch)?].index) << 16 | u32::from(param),
                        _ => return Err("a control names its channel and param, or its kind".into()),
                    };
                    let id = c.id.filter(|i| held.contains(i) && seen.insert(*i)).map_or(Id::NONE, Id);
                    list.push(os_model::ControlPoint { position: c.position, kind, value: c.value, id });
                }
                s.project.get_mut().patterns[place].controls = list;
                s.control.settle(s.project.get_mut());
                none
            }
            Command::SetLoops { pattern, mode, loops } => {
                let place = pattern_place(&s.project, pattern)?;
                if mode.is_some_and(|m| m > 3) {
                    return Err("mode is 0..=3".into());
                }
                let loops = loops.iter().map(|l| channel_row(&s.project, l.channel).map(|r| (s.project.channels[r].index, l.value))).collect::<Result<Vec<_>, _>>()?;
                let p = &mut s.project.get_mut().patterns[place];
                if let Some(m) = mode {
                    p.loop_mode = (m > 0).then_some(u32::from(m));
                }
                for (ch, v) in loops {
                    p.loops.retain(|l| l.0 != ch);
                    if let Some(v) = v {
                        p.loops.push((ch, v));
                    }
                }
                p.loops.sort_by_key(|l| l.0);
                none
            }
            Command::SetStep { pattern, channel, step, on, mute } => {
                let place = pattern_place(&s.project, pattern)?;
                let ch = s.project.channels[channel_row(&s.project, channel)?].index;
                let n = s.project.patterns[place].number;
                crate::edit::rack::set_step_in(s, n, ch, step, on, mute);
                s.control.settle(s.project.get_mut());
                none
            }
            Command::AddNotes { pattern, notes } => {
                if notes.iter().any(|n| n.id.is_some()) {
                    return Err("add_notes makes new notes: give them no id (set_notes keeps a note's)".into());
                }
                let notes = notes_of(&s.project, &notes, &[])?;
                let place = pattern_place(&s.project, pattern)?;
                let (notes, ids) = with_ids(s, notes, &Default::default());
                let p = &mut s.project.get_mut().patterns[place];
                p.notes.extend(notes);
                p.notes.sort_by_key(|n| (n.position, n.channel));
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
            Command::SetPlaylistTrack { track, name, color, enabled, grouped, motion, press, trigger_sync, position_sync, queue, tolerant, clip_lock, link } => {
                let color = color.map(|c| parse_color(&c)).transpose()?;
                let link = match link {
                    Some(LinkIn { mixer: Some(m), channel: None }) => match track_number(&s.project, m)? {
                        0 => return Err("an audio track plays through an insert, not the master".into()),
                        n => Some(TrackLink::audio(n)),
                    },
                    Some(LinkIn { mixer: None, channel: Some(c) }) => {
                        let ch = &s.project.channels[channel_row(&s.project, c)?];
                        if !crate::track_links::instrument_kind(ch.kind) {
                            return Err("an instrument track plays a sampler, plugin or layer channel".into());
                        }
                        Some(TrackLink::instrument(ch.index))
                    }
                    Some(LinkIn { mixer: None, channel: None }) => Some(TrackLink::NONE),
                    Some(_) => return Err("a track links to a mixer track or a channel".into()),
                    None => None,
                };
                let lock = match clip_lock {
                    Some(LockIn { pattern: Some(n), channel: None }) => Some(Some(ClipSource::Pattern(n))),
                    Some(LockIn { pattern: None, channel: Some(c) }) => Some(Some(ClipSource::Channel(s.project.channels[channel_row(&s.project, c)?].index))),
                    Some(LockIn { pattern: None, channel: None }) => Some(None),
                    Some(_) => return Err("a lock is to a pattern or a channel".into()),
                    None => None,
                };
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
                if let Some(v) = grouped {
                    t.grouped = v && track > 1;
                }
                let p = &mut t.perf;
                for (v, f) in [(motion, &mut p.motion), (press, &mut p.press), (trigger_sync, &mut p.trigger_sync), (position_sync, &mut p.position_sync)] {
                    if let Some(v) = v {
                        *f = v;
                    }
                }
                if let Some(v) = queue {
                    p.queue = v;
                }
                if let Some(v) = tolerant {
                    p.tolerant = v;
                }
                if let Some(l) = lock {
                    t.clip_lock = l;
                }
                if let Some(l) = link {
                    t.link = l;
                }
                none
            }
            Command::SetPoints { channel, points } => {
                let index = s.project.channels[channel_row(&s.project, channel)?].index;
                let held = auto_of(&s.project, channel)?.points.clone();
                if points.len() < 2 {
                    return Err("an automation clip keeps two points at least".into());
                }
                let mut list = Vec::with_capacity(points.len());
                let mut seen = std::collections::HashSet::new();
                for q in &points {
                    if !(q.beat.is_finite() && q.beat >= 0.0) || !(0.0..=1.0).contains(&q.value) {
                        return Err("a point's beat is 0 and up, its value 0..=1".into());
                    }
                    let was = q.id.and_then(|i| held.iter().find(|p| p.id.0 == i)).filter(|p| seen.insert(p.id.0));
                    let mut p = was.copied().unwrap_or(os_model::AutoPoint { id: Id::NONE, ..os_model::AutoPoint::new(q.beat, q.value) });
                    (p.beat, p.value) = (q.beat, q.value);
                    if let Some(t) = q.tension {
                        p.tension = t.clamp(-1.0, 1.0);
                    }
                    if let Some(m) = q.mode {
                        p.mode = os_model::AutoMode::from_byte(m);
                    }
                    if let Some(f) = q.flag {
                        p.flag = f;
                    }
                    list.push(p);
                }
                list.sort_by(|a, b| a.beat.total_cmp(&b.beat));
                if let Some(a) = s.project.get_mut().automation_mut(index) {
                    a.points = list;
                }
                s.control.settle(s.project.get_mut());
                none
            }
            Command::AddArrangement { name, copy } => {
                let n = s.project.arrangements.iter().map(|a| a.number + 1).max().unwrap_or(0);
                let mut a = match copy {
                    Some(id) => arrangement_of(&s.project, id)?.clone(),
                    None => os_flp::project::default_arrangement(),
                };
                let name = match (name, copy) {
                    (Some(n), _) => n,
                    (None, Some(_)) => format!("{} #2", a.name.clone().unwrap_or_else(|| "Arrangement".into())),
                    (None, None) => format!("Arrangement {}", s.project.arrangements.len() + 1),
                };
                (a.number, a.name, a.id) = (n, Some(name), Id::NONE);
                // A copy's things are new.
                a.clips.iter_mut().for_each(|c| c.id = Id::NONE);
                a.markers.iter_mut().for_each(|m| m.id = Id::NONE);
                a.tracks.iter_mut().for_each(|t| t.id = Id::NONE);
                s.project.get_mut().arrangements.push(a);
                s.control.settle(s.project.get_mut());
                let id = s.project.arrangements.last().map_or(Id::NONE, |a| a.id);
                Ok(id_result("arrangement", id))
            }
            Command::DeleteArrangement { arrangement } => {
                if s.project.arrangements.len() < 2 {
                    return Err("The only arrangement cannot be deleted".into());
                }
                let i = s.project.arrangements.iter().position(|a| a.id.0 == arrangement).ok_or_else(|| format!("no arrangement {arrangement}"))?;
                let shown = s.project.arrangement_index() == i;
                s.project.get_mut().arrangements.remove(i);
                if shown {
                    let n = s.project.arrangements[i.saturating_sub(1)].number;
                    s.project.get_mut().current_arrangement = Some(n);
                }
                s.touched.clips = true;
                none
            }
            Command::SetArrangement { arrangement, name } => {
                let i = s.project.arrangements.iter().position(|a| a.id.0 == arrangement).ok_or_else(|| format!("no arrangement {arrangement}"))?;
                s.project.get_mut().arrangements[i].name = text(&name);
                none
            }
            Command::SetClips { clips } => {
                let held: Vec<Clip> = crate::edit::playlist::clips(s).to_vec();
                let list = clips.iter().map(|c| c.made(&s.project, &held)).collect::<Result<Vec<_>, _>>()?;
                // Each id once: a second clip naming one is new.
                let mut seen = std::collections::HashSet::new();
                let list: Vec<Clip> = list.into_iter().map(|mut c| {
                    if !c.id.is_none() && !seen.insert(c.id.0) {
                        c.id = Id::NONE;
                    }
                    c
                }).collect();
                crate::edit::playlist::arr_mut(s).clips = list;
                s.control.settle(s.project.get_mut());
                s.touched.clips = true;
                none
            }
            Command::SetMarkers { markers } => {
                let held: Vec<os_model::TimeMarker> = crate::edit::playlist::markers(s).to_vec();
                let mut list = markers.iter().map(|m| m.made_from(&held)).collect::<Result<Vec<_>, _>>()?;
                let mut seen = std::collections::HashSet::new();
                for m in &mut list {
                    if !m.id.is_none() && !seen.insert(m.id.0) {
                        m.id = Id::NONE;
                    }
                }
                list.sort_by_key(|m| m.position);
                crate::edit::playlist::arr_mut(s).markers = list;
                s.control.settle(s.project.get_mut());
                none
            }
            Command::AddMarker { marker } => {
                let m = marker.made()?;
                let i = crate::edit::playlist::add_marker(s, m);
                s.control.settle(s.project.get_mut());
                let id = crate::edit::playlist::markers(s)[i].id;
                Ok(id_result("marker", id))
            }
            Command::SetMarker { marker, position, name, kind, numerator, denominator } => {
                if let Some(p) = position {
                    ticks("position", p)?;
                }
                let i = marker_place(s, marker)?;
                let m = &mut crate::edit::playlist::arr_mut(s).markers[i];
                if let Some(p) = position {
                    m.position = p;
                }
                if let Some(n) = name {
                    m.name = n;
                }
                if let Some(k) = kind {
                    m.kind = k;
                }
                if let Some(n) = numerator {
                    m.numerator = n;
                }
                if let Some(d) = denominator {
                    m.denominator = d;
                }
                if position.is_some() {
                    crate::edit::playlist::sort_markers(s);
                }
                none
            }
            Command::DeleteMarker { marker } => {
                let i = marker_place(s, marker)?;
                crate::edit::playlist::arr_mut(s).markers.remove(i);
                none
            }
            Command::MoveContent { marker, right } => {
                let i = marker_place(s, marker)?;
                if !crate::edit::playlist::move_content(s, i, right) {
                    return Err("there is no section to trade places with".into());
                }
                s.touched.clips = true;
                none
            }
            Command::SwapPlaylistTracks { track, with } => {
                let (a, b) = (playlist_track(track)?, playlist_track(with)?);
                crate::edit::playlist::swap_tracks(s, a, b);
                s.touched.clips = true;
                none
            }
            Command::InsertPlaylistTrack { at } => {
                crate::edit::playlist::insert_track(s, playlist_track(at)?);
                s.touched.clips = true;
                none
            }
            Command::DeletePlaylistTrack { track } => {
                crate::edit::playlist::delete_track_in(s, playlist_track(track)?);
                s.touched.clips = true;
                none
            }
            Command::ResetPlaylistTrack { track } => {
                let t = playlist_track(track)?;
                let tr = &mut crate::edit::playlist::arr_mut(s).tracks[usize::from(t)];
                *tr = os_model::PlaylistTrack::new(tr.number);
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
            Command::SetMember { container, member, wet, dry, name, color, icon, auto_bypass, threaded } => {
                let (t, k) = crate::edit::container::id_place(s, container).ok_or_else(|| format!("no container {container}"))?;
                let wet = wet.map(|v| unit("wet", v, 0.0, 1.0)).transpose()?;
                let dry = dry.map(|v| unit("dry", v, 0.0, 1.0)).transpose()?;
                let color = color.map(|c| parse_color(&c)).transpose()?;
                let icon = match icon {
                    Some(n) if n.is_empty() => Some(None),
                    Some(n) => Some(Some(crate::icon::ICONS.iter().find(|(x, _)| *x == n).map(|(_, i)| *i).ok_or_else(|| format!("no icon {n}"))?)),
                    None => None,
                };
                if crate::edit::container::member(s, t, k, member).is_none() {
                    return Err(format!("the container has no member {member}"));
                }
                for (is_wet, v) in [(true, wet), (false, dry)] {
                    if let Some(v) = v {
                        crate::edit::container::put_level_in(s, t, k, member, is_wet, v as f32);
                    }
                }
                if name.is_some() || color.is_some() || icon.is_some() || auto_bypass.is_some() || threaded.is_some() {
                    let mut list = crate::edit::container::members(s, t, k).unwrap_or_default();
                    if let Some(m) = list.iter_mut().find(|m| m.id == member) {
                        if let Some(n) = name {
                            let n = n.trim();
                            m.label = (!n.is_empty() && n != m.name).then(|| n.to_string());
                        }
                        if let Some(c) = color {
                            m.color = c;
                        }
                        if let Some(i) = icon {
                            m.icon = i;
                        }
                        if let Some(v) = auto_bypass {
                            m.cpu.smart_disable = v;
                        }
                        if let Some(v) = threaded {
                            m.cpu.threaded = v;
                        }
                    }
                    crate::edit::container::set_members(s, t, k, &list);
                }
                none
            }
            Command::WrapEffects { track, slot, all } => {
                if slot > 9 {
                    return Err(format!("slots are 0..=9, not {slot}"));
                }
                let t = track_number(&s.project, track)?;
                let msg = crate::edit::container::wrap_slot_in(s, t, slot, all)?;
                s.control.settle(s.project.get_mut());
                Ok(serde_json::json!({ "message": msg }))
            }
            Command::WrapInstrument { channel } => {
                let row = channel_row(&s.project, channel)?;
                let msg = crate::edit::container::wrap_channel_in(s, row)?;
                let id = s.project.channels[row].id.0;
                Ok(serde_json::json!({ "channel": id, "message": msg }))
            }
            Command::MoveMember { container, member, to } => {
                let (t, k, mut list, i) = member_at(s, container, member)?;
                if to >= list.len() {
                    return Err(format!("place {to} is past the last ({})", list.len() - 1));
                }
                let m = list.remove(i);
                list.insert(to, m);
                crate::edit::container::set_members(s, t, k, &list);
                none
            }
            Command::RemoveMember { container, member } => {
                let (t, k, mut list, i) = member_at(s, container, member)?;
                list.remove(i);
                crate::edit::container::set_members(s, t, k, &list);
                none
            }
            Command::AddMember { container, plugin, instrument } => {
                let (t, k) = crate::edit::container::id_place(s, container).ok_or_else(|| format!("no container {container}"))?;
                if instrument && k != crate::container::CHANNEL {
                    return Err("an instrument goes in a channel's container".into());
                }
                let (uid, name) = plugin_named(s, &plugin, instrument)?;
                let mut list = crate::edit::container::members(s, t, k).unwrap_or_default();
                // The smallest free id: a removed member's goes to the next one added (its clips and links would follow it).
                let id = crate::container::free_id(&list).ok_or_else(|| format!("A container holds at most {} effects", os_model::target::MEMBER_IDS))?;
                let (wet, dry) = if instrument { crate::container::INSTRUMENT_LEVELS } else { (1.0, 0.0) };
                list.push(crate::container::Member { id, instrument, uid, name, wet, dry, state: Vec::new(), ..Default::default() });
                crate::edit::container::set_members(s, t, k, &list);
                Ok(serde_json::json!({ "member": id }))
            }
            Command::ReplaceMember { container, member, plugin, instrument } => {
                let (t, k, mut list, i) = member_at(s, container, member)?;
                let (uid, name) = plugin_named(s, &plugin, instrument)?;
                let m = &mut list[i];
                (m.instrument, m.uid, m.name, m.state) = (instrument, uid, name, Vec::new());
                // The new plugin's own: no name, colour or icon, the switches a new plugin starts with (as a slot's on
                // Replace).
                (m.cpu, m.label, m.color, m.icon) = (os_model::PluginCpu::default(), None, None, None);
                crate::edit::container::set_members(s, t, k, &list);
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
                let declick = ch.sound.declick != c.sound.declick && c.kind.is_sampler();
                let stretch = ch.sound.stretch != c.sound.stretch && c.kind.is_sampler();
                // Muting is a step of its own, a lock, a zip and a filter group none: the channel stays as it is in
                // those (`Mutes`).
                let (on, locked, zipped, group) = (ch.enabled, ch.rack.locked, ch.rack.zipped, ch.rack.group);
                *ch = *c;
                (ch.enabled, ch.rack.locked, ch.rack.zipped, ch.rack.group) = (on, locked, zipped, group);
                if declick {
                    crate::edit::rack::sync_declick(s, row);
                }
                if stretch {
                    let sound = s.project.channels[row].sound.clone();
                    s.plugins.set_sampler_stretch(row, &sound);
                }
            }
        }
        Taken::Track(m) => {
            if let Ok(place) = track_place(&s.project, m.id.0) {
                // Undo leaves its light and its arming as they are (arming is no part of the track, track-io.md §2.3).
                let (on, armed) = (s.project.mixer[place].enabled, s.project.mixer[place].armed);
                s.project.get_mut().mixer[place] = *m;
                (s.project.get_mut().mixer[place].enabled, s.project.get_mut().mixer[place].armed) = (on, armed);
            }
        }
        Taken::Pattern(p) => {
            if let Ok(place) = pattern_place(&s.project, p.id.0) {
                s.project.get_mut().patterns[place] = *p;
            }
            s.touched.notes = true;
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
        Taken::Points(ch, points) => {
            if let Ok(row) = channel_row(&s.project, ch) {
                let index = s.project.channels[row].index;
                if let Some(a) = s.project.get_mut().automation_mut(index) {
                    a.points = points;
                }
                s.touched.repaint = true;
            }
        }
        Taken::Swap(c, now) => {
            if let Ok(row) = channel_row(&s.project, now) {
                // The instrument there goes, kept by its channel's id; the one kept by `c`'s comes back on the tick
                // (`control::host`).
                crate::project_plugins::drop_row(s, row);
                let ch = &mut s.project.get_mut().channels[row];
                let (on, locked, zipped, group) = (ch.enabled, ch.rack.locked, ch.rack.zipped, ch.rack.group);
                *ch = *c;
                (ch.enabled, ch.rack.locked, ch.rack.zipped, ch.rack.group) = (on, locked, zipped, group);
                s.touched.repaint = true;
            }
        }
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
        Taken::Locks(on) => {
            for (id, v) in on {
                if let Some(c) = s.project.get_mut().channels.iter_mut().find(|c| c.id.0 == id) {
                    c.rack.locked = v;
                }
            }
        }
        Taken::Zips(on) => {
            for (id, v) in on {
                if let Some(c) = s.project.get_mut().channels.iter_mut().find(|c| c.id.0 == id) {
                    c.rack.zipped = v;
                }
            }
        }
        Taken::Groups(g) => {
            let (names, members, shown) = *g;
            let p = s.project.get_mut();
            p.groups = names;
            for (id, v) in members {
                if let Some(c) = p.channels.iter_mut().find(|c| c.id.0 == id) {
                    c.rack.group = v;
                }
            }
            p.current_group = shown;
        }
        Taken::WithOn(t, on) => {
            // A track that is there before and after keeps its light as it is now (`Mutes::keep`).
            let now: std::collections::HashMap<u32, bool> = s.project.mixer.iter().map(|m| (m.id.0, m.enabled)).collect();
            put(s, *t);
            for (id, on, armed) in on {
                if let Some(m) = s.project.get_mut().mixer.iter_mut().find(|m| m.id.0 == id) {
                    (m.enabled, m.armed) = (now.get(&id).copied().unwrap_or(on), armed);
                }
            }
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
pub(crate) fn put_channel_back(s: &mut crate::session::Session, row: usize, mut c: os_model::Channel) {
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

/// Which channels are zipped now (`Taken::Zips`).
fn zips(p: &Project) -> Taken {
    Taken::Zips(p.channels.iter().map(|c| (c.id.0, c.rack.zipped)).collect())
}

/// The filter groups now (`Taken::Groups`).
fn groups(p: &Project) -> Taken {
    Taken::Groups(Box::new((p.groups.clone(), p.channels.iter().map(|c| (c.id.0, c.rack.group)).collect(), p.current_group)))
}

/// Which channels' mute states are locked now (`Taken::Locks`).
fn locks(p: &Project) -> Taken {
    Taken::Locks(p.channels.iter().map(|c| (c.id.0, c.rack.locked)).collect())
}

/// Which mixer tracks are on now (`Taken::TracksOn`).
/// Every mixer track's mute light and arm switch, by id (`Taken::WithOn`).
fn lights(p: &Project) -> Vec<(u32, bool, bool)> {
    p.mixer.iter().map(|m| (m.id.0, m.enabled, m.armed)).collect()
}

fn tracks_on(p: &Project) -> Taken {
    Taken::TracksOn(p.mixer.iter().map(|m| (m.id.0, m.enabled)).collect())
}

impl Command {
    /// The undo of what a command switches on or off, beside its own (`Taken::Channel` and `Taken::Track` keep a
    /// channel or track on or off, and a channel's lock, as it is, since an undo of another step leaves muting alone):
    /// `set_channel` with `enabled` or `locked` and `set_track` with `enabled`, taken before they apply.
    pub fn on_off_undo(&self, s: &crate::session::Session) -> Vec<Command> {
        let put = |t: Taken| Command::Put(Box::new(t));
        match self {
            Command::SetChannel { enabled, locked, zipped, .. } => enabled
                .map(|_| put(mutes(&s.project)))
                .into_iter()
                .chain(locked.map(|_| put(locks(&s.project))))
                .chain(zipped.map(|_| put(zips(&s.project))))
                .collect(),
            Command::SetTrack { enabled: Some(_), .. } | Command::ResetTrack { .. } => vec![put(tracks_on(&s.project))],
            _ => Vec::new(),
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
/// `notes` as the pattern stores them; a note whose id names one of `held` starts from it (the values not given are
/// kept), any other from a new step.
fn notes_of(p: &Project, notes: &[NoteIn], held: &[Note]) -> Result<Vec<Note>, Error> {
    notes
        .iter()
        .map(|n| {
            let ch = p.channels[channel_row(p, n.channel)?].index;
            ticks("a note's position", n.position)?;
            ticks("a note's length", n.length)?;
            if n.key > 131 {
                return Err(format!("key {} is past the last (131)", n.key));
            }
            let check = |name: &str, v: Option<u8>, max: u8| match v {
                Some(v) if v > max => Err(format!("a note's {name} is 0..={max}, not {v}")),
                _ => Ok(v),
            };
            let was = n.id.filter(|&i| i != 0).and_then(|i| held.iter().find(|h| h.id.0 == i));
            let mut x = Note { id: Id(n.id.unwrap_or(0)), ..was.copied().unwrap_or_else(|| Note::step(ch, n.position)) };
            (x.channel, x.position, x.length, x.key) = (ch, n.position, n.length, n.key);
            let keep = |v: Option<u8>, now: u8, new: u8| v.unwrap_or(if was.is_some() { now } else { new });
            x.velocity = keep(n.velocity.map(|v| v.min(128)), x.velocity, 100);
            x.pan = keep(n.pan.map(|v| v.min(128)), x.pan, 64);
            x.fine_pitch = keep(check("fine pitch", n.fine_pitch, 240)?, x.fine_pitch, x.fine_pitch);
            x.release = keep(check("release", n.release, 128)?, x.release, x.release);
            x.mod_x = keep(check("mod X", n.mod_x, 128)?, x.mod_x, x.mod_x);
            x.mod_y = keep(check("mod Y", n.mod_y, 128)?, x.mod_y, x.mod_y);
            x.repeat = keep(check("repeat", n.repeat, 14)?, x.repeat, x.repeat);
            if let Some([flags, group, unk14]) = n.raw {
                (x.flags, x.group, x.unk14) = (flags, group as u8, unk14);
            }
            if let Some(c) = check("colour", n.color, 15)? {
                x.group = (x.group & !0x0f) | c;
            }
            if let Some(v) = n.slide {
                x.flags = if v { x.flags | Note::SLIDE } else { x.flags & !Note::SLIDE };
            }
            if let Some(v) = n.muted {
                x.set_muted(v);
            }
            if let Some(v) = n.porta {
                x.group = if v { x.group | Note::PORTA } else { x.group & !Note::PORTA };
            }
            Ok(x)
        })
        .collect()
}
