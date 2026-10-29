// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Typed view of an FLP event stream: [`load`] maps events to an [`os_model::Project`],
//! [`apply`] writes model values back into the original stream in place, so everything the
//! model doesn't cover (unknown events, plugin states, …) is preserved and an unedited
//! project re-serializes byte-for-byte.
//!
// spec: flp/events/project-info.md, flp/events/channels.md, flp/events/patterns.md,
//       flp/events/mixer.md, flp/events/playlist.md

use std::collections::BTreeMap;

use os_model::clip_lfo::ClipLfo;
use os_model::{
    Arrangement, AutoMode, AutoPoint, Automation, Channel, ChannelKind, ChannelMisc, ChannelRack, ChannelSound, Clip, ClipSource, ControlPoint,
    DataFolder, Envelope, InitControl, MixerTrack,
    Note, Pattern, PlaylistTrack, PlaylistView, Project, ProjectInfo, Rgb, SampleFx, SlotPlugin, TimeMarker, WorkTime, CLIP_RECORD_LEN,
};

use crate::{Chunk, ChunkBody, Event, FlpFile, Header, Tag, Value};

// Event IDs (see the specs for meanings).
mod id {
    pub const VERSION: u8 = 199;
    pub const TEMPO: u8 = 156;
    pub const TITLE: u8 = 194;
    pub const COMMENTS: u8 = 195;
    pub const URL: u8 = 197;
    pub const GENRE: u8 = 206;
    pub const AUTHOR: u8 = 207;
    /// Events FL writes between the author and the comments (167, 202), and 237 (the project's start date and the
    /// time spent), which ends the project information (spec: flp/engine-events.md "Order in the file").
    pub const INFO_167: u8 = 167;
    pub const INFO_202: u8 = 202;
    pub const WORK_TIME: u8 = 237;
    /// Show info on opening, 1 = on (spec: flp/events/project-info.md).
    pub const SHOW_INFO: u8 = 10;
    /// The project events that stand before the project information and are not read (spec: flp/engine-events.md
    /// "Order in the file": 156, 67, 9, 11, 80, 17, 18, 35, 23, 44, 30, 10).
    pub const OTHER_PROJECT: [u8; 5] = [9, 35, 23, 44, 10];
    pub const CURRENT_PATTERN: u8 = 67;
    /// Current arrangement number (spec: flp/events/playlist.md).
    pub const CURRENT_ARRANGEMENT: u8 = 100;
    /// Playlist view: scroll, time zoom, track height (spec: flp/events/playlist.md "Playlist view").
    pub const PLAYLIST_VIEW: u8 = 242;

    pub const CH_START: u8 = 64;
    pub const CH_END: u8 = 51;
    pub const CH_TYPE: u8 = 21;
    pub const CH_ENABLED: u8 = 0;
    pub const CH_COLOR_FLAG: u8 = 41;
    pub const CH_LEVELS: u8 = 219;
    pub const CH_TARGET: u8 = 104;
    pub const CH_LAYER_CHILD: u8 = 94;
    pub const CH_ENVELOPE: u8 = 218;
    pub const PLUGIN_NAME: u8 = 201;
    pub const PLUGIN_REC: u8 = 212;
    pub const PLUGIN_STATE: u8 = 213;
    pub const PLUGIN_ICON: u8 = 155;
    pub const DISPLAY_NAME: u8 = 203;
    pub const COLOR: u8 = 128;
    pub const SAMPLE_PATH: u8 = 196;
    /// Channel rack state (spec: flp/events/channels.md): zipped row, lock, filter group.
    pub const CH_ZIPPED: u8 = 15;
    pub const CH_LOCKED: u8 = 32;
    pub const CH_GROUP: u8 = 145;
    /// Project-level rack settings (spec: flp/events/project-info.md).
    pub const SWING: u8 = 11;
    /// The bar: beats per bar (17) and steps per beat (18, not the denominator), how the Project page shows them (35:
    /// 1 = time signature, 0 = time division) and "play truncated notes" (30, present = on) (spec: flp/engine-events.md
    /// "Project").
    pub const TIME_NUM: u8 = 17;
    pub const TIME_DEN: u8 = 18;
    pub const TIME_MODE: u8 = 35;
    /// Song or pattern mode (9: 1 = song, 0 = pattern), one byte right after 67 (spec: flp/engine-events.md "Project").
    pub const SONG_MODE: u8 = 9;
    pub const PLAY_TRUNCATED: u8 = 30;
    /// Settings shown but not played yet: panning law (23), automatic crossfades (44), fast declick for cut groups (40,
    /// after event 100) (spec: flp/engine-events.md "Project").
    pub const PANNING_LAW: u8 = 23;
    pub const AUTO_CROSSFADES: u8 = 44;
    pub const FAST_DECLICK: u8 = 40;
    /// The project's main pitch, cents as a signed 16-bit value (spec: flp/engine-events.md "Project").
    pub const MAIN_PITCH: u8 = 80;
    /// Channel fine tune, cents (spec: audio/voice-levels.md).
    pub const CH_FINE: u8 = 142;
    /// Channel settings the engine reads (spec: flp/engine-events.md "A channel").
    pub const CH_SWING_SHARE: u8 = 97;
    /// Time shift, 0..=1024 (spec: audio/channel-misc.md "Settings").
    pub const CH_TIME_SHIFT: u8 = 89;
    pub const CH_ROOT: u8 = 135;
    pub const CH_SAMPLER_FLAGS: u8 = 143;
    pub const CH_SAMPLE_SETTINGS: u8 = 215;
    /// Echo delay (209: feed, pan, pitch, count, time; 138: mod Y low word, mod X high word) and tracking (228, twice:
    /// by volume, by key) (spec: audio/channel-misc.md "Settings").
    pub const CH_ECHO: u8 = 209;
    pub const CH_ECHO_MOD: u8 = 138;
    pub const CH_TRACKING: u8 = 228;
    pub const CH_ADJUST: u8 = 229;
    /// A sample's loop start and end in frames (spec: flp/events/channels.md "Loop points").
    pub const CH_LOOP_POINTS: u8 = 251;
    /// Cut groups (word 0 cuts, word 1 cut by) and the voice settings: maximum voices, slide time, mono / portamento
    /// (spec: audio/note-rules.md "Settings").
    pub const CH_CUT_GROUPS: u8 = 132;
    pub const CH_LAYER_FLAGS: u8 = 144;
    pub const CH_POLY: u8 = 221;
    /// A sample channel's precomputed effects and loop option (spec: audio/precomputed-effects.md "Settings").
    pub const SMP_PING_PONG: u8 = 20;
    pub const SMP_FEEDBACK: u8 = 69;
    pub const SMP_FLAGS: u8 = 70;
    pub const SMP_CUTOFF: u8 = 71;
    pub const SMP_BOOST: u8 = 74;
    pub const SMP_FADE_OUT: u8 = 75;
    pub const SMP_FADE_IN: u8 = 76;
    pub const SMP_RESONANCE: u8 = 83;
    pub const SMP_STEREO_DELAY: u8 = 85;
    pub const SMP_POGO: u8 = 86;
    pub const SMP_RING: u8 = 131;
    pub const SMP_REVERB: u8 = 139;
    pub const GROUP_NAME: u8 = 231;
    pub const CURRENT_GROUP: u8 = 146;
    /// The project's initial values, 12-byte records, in the prelude after 146 (spec: flp/engine-events.md "Event 216",
    /// flp/events/project-info.md, ui/settings-choices.md "Advanced: Initialize controls automatically").
    pub const INIT_CONTROLS: u8 = 216;

    pub const PAT_START: u8 = 65;
    pub const PAT_NOTES: u8 = 224;
    /// Channel control points (12-byte records) between 65 and 224.
    pub const PAT_CONTROLS: u8 = 223;
    pub const PAT_NAME: u8 = 193;
    pub const PAT_COLOR: u8 = 150;
    /// Pattern length in ticks (metadata) and per-channel loop lengths (after the notes), loop-mode-off flag and
    /// loop type (metadata) (spec: flp/events/patterns.md).
    pub const PAT_LENGTH: u8 = 164;
    pub const LOOP_CHANNEL: u8 = 160;
    pub const LOOP_STEPS: u8 = 161;
    pub const LOOP_OFF: u8 = 26;
    pub const LOOP_TYPE: u8 = 162;

    pub const ARR_START: u8 = 99;
    pub const ARR_NAME: u8 = 241;
    pub const ARR_CLIPS: u8 = 233;
    pub const PL_TRACK: u8 = 238;
    pub const PL_COLOR_FLAG: u8 = 43;
    pub const PL_NAME: u8 = 239;
    pub const ARR_UNKNOWN: u8 = 36;
    /// Time marker group (spec: flp/events/playlist.md "Time markers").
    pub const MARKER_POS: u8 = 148;
    pub const MARKER_NUM: u8 = 33;
    pub const MARKER_DEN: u8 = 34;
    pub const MARKER_NAME: u8 = 205;
    /// A key marker's scale (spec: flp/events/playlist.md "Time markers").
    pub const MARKER_SCALE: u8 = 101;
    pub const MARKER_IDS: [u8; 8] = [148, 33, 34, 46, 45, 168, 101, 205];
    /// Automation clip points (in the channel block) and links (before the first channel)
    /// (spec: flp/events/automation.md).
    pub const AUTO_DATA: u8 = 234;
    pub const AUTO_LINK: u8 = 227;
    /// A link's mapping formula, right after its event 227 (spec: flp/events/automation.md "Controller links").
    pub const LINK_FORMULA: u8 = 230;

    pub const MX_COLOR: u8 = 149;
    pub const MX_COLOR_FLAG: u8 = 42;
    /// The track's icon, after the colour flag and before the name (spec: flp/events/mixer.md "Track blocks").
    pub const MX_ICON: u8 = 95;
    pub const MX_NAME: u8 = 204;
    pub const MX_FLAGS: u8 = 236;
    pub const MX_SLOT: u8 = 98;
    pub const MX_ROUTES: u8 = 235;
    pub const MX_PARAMS: u8 = 225;
    /// After the routes: record source, monitor mode, audio input and audio output, u32s read as signed (spec:
    /// audio/track-io.md §1.6).
    pub const MX_REC_SOURCE: u8 = 165;
    pub const MX_MONITOR: u8 = 166;
    pub const MX_INPUT: u8 = 154;
    pub const MX_OUTPUT: u8 = 147;
    /// Number of stored mixer tracks, just before the first track (spec: flp/events/mixer.md).
    pub const MX_COUNT: u8 = 103;
}

const NOTE_LEN: usize = 24;
/// Clip record lengths: 88 bytes in files of FL 26, 80 bytes in files of FL 24.2 to 25, 60 bytes in files of FL 21
/// to 24.1 (20.99 betas included), 32 bytes up to FL 20 (spec: flp/layouts.md, "Playlist clip records").
const CLIP_LEN: usize = 88;
const CLIP_LEN_25: usize = 80;
const CLIP_LEN_21: usize = 60;
const CLIP_LEN_20: usize = 32;
const MIXER_REC_LEN: usize = 12;
const NOTE_FLAGS_DEFAULT: u16 = 0x4000;
const PATTERN_COLOR_DEFAULT: u32 = 0x002D_2C29;

/// Errors from [`load`] / [`apply`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProjectError {
    #[error("file has no FLdt event chunk")]
    NoEvents,
    #[error("file has no FLhd header")]
    NoHeader,
    #[error("{0}")]
    Structure(String),
    /// Files saved before FL 24.2 use a channel layout that isn't specified yet
    /// (specs REQUESTS.md #4).
    #[error("legacy project layout (saved by {0}) is not supported yet")]
    LegacyLayout(String),
    #[error("model does not match the file layout: {0}")]
    Mismatch(String),
}

// ---------------------------------------------------------------------------------------
// Text

/// Text encoding: 8-bit for files saved by major version ≤ 11, UTF-16LE from 12 on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextEnc {
    Ansi,
    Utf16,
}

fn decode_text(data: &[u8], enc: TextEnc) -> String {
    match enc {
        TextEnc::Ansi => {
            let body = data.strip_suffix(&[0]).unwrap_or(data);
            body.iter().map(|&b| b as char).collect()
        }
        TextEnc::Utf16 => {
            let units: Vec<u16> = data
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let units = units.strip_suffix(&[0]).unwrap_or(&units);
            String::from_utf16_lossy(units)
        }
    }
}

fn encode_text(s: &str, enc: TextEnc) -> Vec<u8> {
    match enc {
        TextEnc::Ansi => {
            let mut v: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
            v.push(0);
            v
        }
        TextEnc::Utf16 => {
            let mut v: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
            v.extend([0, 0]);
            v
        }
    }
}

/// Project comments store line breaks as a bare CR.
fn comments_from_file(s: String) -> String {
    s.replace('\r', "\n")
}
fn comments_to_file(s: &str) -> String {
    s.replace("\r\n", "\r").replace('\n', "\r")
}

/// Event 237: the start and the time spent, two little-endian doubles in days (spec: flp/events/project-info.md).
/// `None` unless it holds 16 bytes.
fn work_time_from(d: &[u8]) -> Option<WorkTime> {
    let d: &[u8; 16] = d.try_into().ok()?;
    let (a, b) = d.split_at(8);
    Some(WorkTime { started: f64::from_le_bytes(a.try_into().ok()?), spent: f64::from_le_bytes(b.try_into().ok()?) })
}

fn work_time_bytes(w: WorkTime) -> Vec<u8> {
    [w.started.to_le_bytes(), w.spent.to_le_bytes()].concat()
}

/// A project started now: the local time in days since 1899-12-30 00:00, nothing spent yet (spec:
/// flp/events/project-info.md, event 237 and "Writer notes").
pub fn work_time_now() -> WorkTime {
    let epoch = chrono::NaiveDate::from_ymd_opt(1899, 12, 30).and_then(|d| d.and_hms_opt(0, 0, 0)).expect("valid date");
    let ms = (chrono::Local::now().naive_local() - epoch).num_milliseconds();
    WorkTime { started: ms as f64 / 86_400_000.0, spent: 0.0 }
}

// ---------------------------------------------------------------------------------------
// Colors

/// `0x00BBGGRR`
fn rgb_from_bgr(v: u32) -> Rgb {
    Rgb::new(v as u8, (v >> 8) as u8, (v >> 16) as u8)
}
fn bgr_from_rgb(c: Rgb, alpha: u8) -> u32 {
    u32::from(c.r) | u32::from(c.g) << 8 | u32::from(c.b) << 16 | u32::from(alpha) << 24
}
/// Channel colors: FL raises each component to at least 0x14.
fn clamp_channel_color(c: Rgb) -> Rgb {
    Rgb::new(c.r.max(0x14), c.g.max(0x14), c.b.max(0x14))
}

// ---------------------------------------------------------------------------------------
// Layout: where each model field lives in the event stream

#[derive(Debug, Default)]
struct ChannelLoc {
    start: usize,
    end: usize,
    name: Option<usize>,
    plugin_rec: Option<usize>,
    /// Events 201 (plugin name) and 213 (plugin state) (spec: flp/events/plugins.md).
    plugin_name: Option<usize>,
    state: Option<usize>,
    color: Option<usize>,
    color_flag: Option<usize>,
    enabled: Option<usize>,
    levels: Option<usize>,
    target: Option<usize>,
    auto: Option<usize>,
    /// Event 196 (sample path) and event 20, which it follows (spec: flp/events/channels.md, row 40).
    sample: Option<usize>,
    ev20: Option<usize>,
    zipped: Option<usize>,
    locked: Option<usize>,
    group: Option<usize>,
    /// Last event 94 (layer child); the zip flag follows it or the target (104).
    last_child: Option<usize>,
}

#[derive(Debug, Default)]
struct PatternLoc {
    /// Index of the event 65 that starts the note block (where a new 223 goes).
    start: Option<usize>,
    controls: Option<usize>,
    notes: Option<usize>,
    /// Index of the event 65 that starts the metadata block.
    meta_start: Option<usize>,
    name: Option<usize>,
    color: Option<usize>,
    /// Loop length pairs (160, 161) after the notes.
    loops: Vec<usize>,
    /// Metadata: 157/158 (new loop/length events go after the last), 26 / 162, 164 and every marker group's events.
    meta_158: Option<usize>,
    loop_off: Vec<usize>,
    length: Option<usize>,
    markers: Vec<usize>,
}

#[derive(Debug, Default)]
struct PlTrackLoc {
    record: usize,
    color_flag: Option<usize>,
    name: Option<usize>,
}

#[derive(Debug, Default)]
struct ArrangementLoc {
    /// The arrangement's first event (99) and its name (241).
    start: usize,
    name: Option<usize>,
    clips: Option<usize>,
    /// Every event of the time-marker groups.
    markers: Vec<usize>,
    tracks: Vec<PlTrackLoc>,
}

#[derive(Debug, Default)]
struct MixerLoc {
    color: Option<usize>,
    color_flag: Option<usize>,
    icon: Option<usize>,
    name: Option<usize>,
    flags: Option<usize>,
    routes: Option<usize>,
    /// Events 165, 166, 154 and 147 (`id::MX_REC_SOURCE` ..).
    rec_source: Option<usize>,
    monitor: Option<usize>,
    input: Option<usize>,
    output: Option<usize>,
}

#[derive(Debug, Default)]
struct Layout {
    enc: Option<TextEnc>,
    version: Option<usize>,
    tempo: Option<usize>,
    title: Option<usize>,
    author: Option<usize>,
    genre: Option<usize>,
    comments: Option<usize>,
    url: Option<usize>,
    /// Events 167, 202 and 237 around the project information texts, and the last of the other project events before
    /// them (`id::OTHER_PROJECT`).
    info_167: Option<usize>,
    info_202: Option<usize>,
    work_time: Option<usize>,
    other_project: Option<usize>,
    /// Event 10, show info on opening.
    show_info: Option<usize>,
    current_pattern: Option<usize>,
    current_arrangement: Option<usize>,
    playlist_view: Option<usize>,
    channels: Vec<ChannelLoc>,
    patterns: BTreeMap<u16, PatternLoc>,
    /// Index just past the last pattern metadata block.
    meta_end: Option<usize>,
    arrangements: Vec<ArrangementLoc>,
    mixer: Vec<MixerLoc>,
    mixer_params: Option<usize>,
    /// Automation links (event 227 with a zero first field), in the prelude.
    links: Vec<usize>,
    /// Controller links (event 227 with the bit 0x8000 in its first field), in the prelude, each with the formula event
    /// (230) right after it.
    ctl_links: Vec<(usize, Option<usize>)>,
    swing: Option<usize>,
    /// Time signature and "play truncated notes" (spec: flp/engine-events.md "Project").
    time_num: Option<usize>,
    time_den: Option<usize>,
    truncated: Option<usize>,
    main_pitch: Option<usize>,
    /// The project settings among `id::OTHER_PROJECT` whose place FL's order fixes (9, 35, 23, 44; spec:
    /// flp/engine-events.md "Project", "Order in the file"): the id and where it first stands, to place 17, 18 and 30
    /// (35, 23 and 44 are also read: [`Layout::setting`]).
    unread_settings: Vec<(u8, usize)>,
    /// Fast declick for cut groups (event 40), after event 100.
    fast_declick: Option<usize>,
    groups: Vec<usize>,
    current_group: Option<usize>,
    /// Event 216, the initial values.
    init_controls: Option<usize>,
}

impl Layout {
    /// Where the project setting `eid` (9, 35, 23 or 44) first stands.
    fn setting(&self, eid: u8) -> Option<usize> {
        self.unread_settings.iter().find(|s| s.0 == eid).map(|s| s.1)
    }
}

/// Marker kind of a pattern's "Length" marker and its name (spec: flp/events/patterns.md, metadata block).
const LENGTH_MARKER_KIND: u32 = 6;
const LENGTH_MARKER_NAME: &str = "Length";
/// Loop type "Loop step channels" (event 162).
const LOOP_TYPE_STEP: u32 = 3;

fn int(e: &Event) -> u32 {
    e.int_value().unwrap_or(0)
}

fn events_of(file: &FlpFile) -> Result<&Vec<Event>, ProjectError> {
    file.chunks
        .iter()
        .find_map(|c| match &c.body {
            ChunkBody::Events(ev) => Some(ev),
            _ => None,
        })
        .ok_or(ProjectError::NoEvents)
}

fn events_of_mut(file: &mut FlpFile) -> Result<&mut Vec<Event>, ProjectError> {
    file.chunks
        .iter_mut()
        .find_map(|c| match &mut c.body {
            ChunkBody::Events(ev) => Some(ev),
            _ => None,
        })
        .ok_or(ProjectError::NoEvents)
}

/// The region the scanner is in. The file is: prelude (project info, note blocks) →
/// channel blocks (with pattern metadata between them) → arrangements → globals → mixer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    Prelude,
    Channels,
    Arrangements,
    Mixer,
}

fn scan(events: &[Event]) -> Result<Layout, ProjectError> {
    let mut l = Layout::default();
    let mut region = Region::Prelude;
    let mut i = 0;
    while i < events.len() {
        let e = &events[i];
        match (region, e.id) {
            (Region::Prelude | Region::Channels, id::CH_START) => {
                region = Region::Channels;
                // spec: flp/events/channels.md "Version scope": blocks end with 51 only in
                // files saved by FL 24.2+; a block reaching the next 64 without one is legacy.
                let end = (i + 1..events.len())
                    .take_while(|&k| events[k].id != id::CH_START && events[k].id != id::ARR_START)
                    .find(|&k| events[k].id == id::CH_END)
                    .ok_or_else(|| {
                        let version = l
                            .version
                            .and_then(|v| events[v].data())
                            .map(|d| decode_text(d, TextEnc::Ansi))
                            .unwrap_or_default();
                        ProjectError::LegacyLayout(version)
                    })?;
                l.channels.push(scan_channel(events, i, end));
                i = end + 1;
                continue;
            }
            (Region::Prelude | Region::Channels, id::PAT_START) => {
                let number = int(e) as u16;
                let loc = l.patterns.entry(number).or_default();
                let mut k = i + 1;
                let mut is_meta = false;
                // spec: flp/events/patterns.md "Control points": `65, 223, 224` (either may be absent).
                let has_controls = events.get(k).is_some_and(|n| n.id == id::PAT_CONTROLS);
                if has_controls {
                    loc.start = Some(i);
                    loc.controls = Some(k);
                    k += 1;
                }
                if events.get(k).is_some_and(|n| n.id == id::PAT_NOTES) {
                    loc.start = Some(i);
                    loc.notes = Some(k);
                    k += 1;
                    while events.get(k).is_some_and(|n| n.id == id::LOOP_CHANNEL || n.id == id::LOOP_STEPS) {
                        loc.loops.push(k);
                        k += 1;
                    }
                } else if has_controls {
                    // A block of control points only: nothing else belongs to it.
                } else {
                    // Metadata block: runs until the next block start or region change.
                    loc.meta_start = Some(i);
                    is_meta = true;
                    let mut in_marker = false;
                    while let Some(n) = events.get(k) {
                        in_marker |= n.id == id::MARKER_POS;
                        if in_marker && id::MARKER_IDS.contains(&n.id) {
                            loc.markers.push(k);
                        }
                        match n.id {
                            id::PAT_NAME => loc.name = Some(k),
                            id::PAT_COLOR => loc.color = Some(k),
                            157 | 158 => loc.meta_158 = Some(k),
                            id::LOOP_OFF | id::LOOP_TYPE => loc.loop_off.push(k),
                            id::PAT_LENGTH => loc.length = Some(k),
                            id::MARKER_NAME => in_marker = false,
                            id::PAT_START | id::CH_START | id::ARR_START => break,
                            _ => {}
                        }
                        k += 1;
                    }
                }
                if is_meta {
                    l.meta_end = Some(k);
                }
                i = k;
                continue;
            }
            (Region::Prelude, _) => match e.id {
                id::AUTO_LINK if e.data().is_some_and(|d| d.len() >= 20 && d[0] == 0 && d[1] == 0) => l.links.push(i),
                id::AUTO_LINK if e.data().and_then(os_model::ControllerLink::from_bytes).is_some() => l.ctl_links.push((i, None)),
                id::LINK_FORMULA => {
                    if let Some(c) = l.ctl_links.last_mut().filter(|c| c.0 + 1 == i) {
                        c.1 = Some(i);
                    }
                }
                id::VERSION => l.version = Some(i),
                id::TEMPO => l.tempo = Some(i),
                id::TITLE => l.title = Some(i),
                id::AUTHOR => l.author = Some(i),
                id::GENRE => l.genre = Some(i),
                id::COMMENTS => l.comments = Some(i),
                id::URL => l.url = Some(i),
                id::INFO_167 if l.info_167.is_none() => l.info_167 = Some(i),
                id::INFO_202 if l.info_202.is_none() => l.info_202 = Some(i),
                id::WORK_TIME if l.work_time.is_none() => l.work_time = Some(i),
                id::SHOW_INFO if l.show_info.is_none() => {
                    l.show_info = Some(i);
                    l.other_project = Some(i);
                }
                v if id::OTHER_PROJECT.contains(&v) => {
                    l.other_project = Some(i);
                    if v != 10 && !l.unread_settings.iter().any(|&(k, _)| k == v) {
                        l.unread_settings.push((v, i));
                    }
                }
                id::CURRENT_PATTERN => l.current_pattern = Some(i),
                id::SWING if l.swing.is_none() => l.swing = Some(i),
                id::TIME_NUM if l.time_num.is_none() => l.time_num = Some(i),
                id::TIME_DEN if l.time_den.is_none() => l.time_den = Some(i),
                id::PLAY_TRUNCATED if l.truncated.is_none() => l.truncated = Some(i),
                id::MAIN_PITCH if l.main_pitch.is_none() => l.main_pitch = Some(i),
                id::GROUP_NAME => l.groups.push(i),
                id::CURRENT_GROUP => l.current_group = Some(i),
                id::INIT_CONTROLS if l.init_controls.is_none() => l.init_controls = Some(i),
                _ => {}
            },
            (Region::Channels | Region::Arrangements, id::ARR_START) => {
                region = Region::Arrangements;
                l.arrangements.push(ArrangementLoc { start: i, ..ArrangementLoc::default() });
            }
            (Region::Arrangements, id::ARR_NAME) => {
                if let Some(a) = l.arrangements.last_mut().filter(|a| a.name.is_none() && a.clips.is_none()) {
                    a.name = Some(i);
                }
            }
            (Region::Arrangements, id::ARR_CLIPS) => {
                if let Some(a) = l.arrangements.last_mut().filter(|a| a.tracks.is_empty()) {
                    a.clips = Some(i);
                }
            }
            (Region::Arrangements, m) if id::MARKER_IDS.contains(&m) && l.arrangements.last().is_some_and(|a| a.tracks.is_empty()) => {
                if let Some(a) = l.arrangements.last_mut() {
                    a.markers.push(i);
                }
            }
            (Region::Arrangements, id::PL_TRACK) => {
                let arr = l.arrangements.last_mut().expect("in arrangement region");
                arr.tracks.push(PlTrackLoc { record: i, ..Default::default() });
            }
            (Region::Arrangements, id::PL_COLOR_FLAG) => {
                if let Some(t) = l.arrangements.last_mut().and_then(|a| a.tracks.last_mut()) {
                    t.color_flag = Some(i);
                }
            }
            (Region::Arrangements | Region::Mixer, id::CURRENT_ARRANGEMENT) => l.current_arrangement = Some(i),
            // Event 40 stands after event 100, among 29, 39, 31, 38, 103 (spec: flp/engine-events.md "Project").
            (Region::Arrangements, id::FAST_DECLICK) if l.current_arrangement.is_some() && l.fast_declick.is_none() => {
                l.fast_declick = Some(i)
            }
            (Region::Arrangements | Region::Mixer, id::PLAYLIST_VIEW) => l.playlist_view = Some(i),
            (Region::Arrangements, id::PL_NAME) => {
                if let Some(t) = l.arrangements.last_mut().and_then(|a| a.tracks.last_mut()) {
                    t.name = Some(i);
                }
            }
            // A mixer track header starts with 149 (if colored) or 42.
            (Region::Arrangements | Region::Mixer, id::MX_COLOR | id::MX_COLOR_FLAG) => {
                region = Region::Mixer;
                let start_new = match l.mixer.last() {
                    None => true,
                    Some(m) => m.flags.is_some(),
                };
                if start_new {
                    l.mixer.push(MixerLoc::default());
                }
                let m = l.mixer.last_mut().expect("just pushed");
                if e.id == id::MX_COLOR {
                    m.color = Some(i);
                } else {
                    m.color_flag = Some(i);
                }
            }
            // The icon belongs to the header, before the track's flags (spec: flp/events/mixer.md "Track blocks").
            (Region::Mixer, id::MX_ICON) => {
                if let Some(m) = l.mixer.last_mut().filter(|m| m.flags.is_none()) {
                    m.icon = Some(i);
                }
            }
            (Region::Mixer, id::MX_NAME) => {
                if let Some(m) = l.mixer.last_mut() {
                    m.name = Some(i);
                }
            }
            (Region::Mixer, id::MX_FLAGS) => {
                if let Some(m) = l.mixer.last_mut() {
                    m.flags = Some(i);
                }
            }
            (Region::Mixer, id::MX_ROUTES) => {
                if let Some(m) = l.mixer.last_mut() {
                    m.routes = Some(i);
                }
            }
            (Region::Mixer, id::MX_REC_SOURCE | id::MX_MONITOR | id::MX_INPUT | id::MX_OUTPUT) => {
                if let Some(m) = l.mixer.last_mut().filter(|m| m.routes.is_some()) {
                    let at = match e.id {
                        id::MX_REC_SOURCE => &mut m.rec_source,
                        id::MX_MONITOR => &mut m.monitor,
                        id::MX_INPUT => &mut m.input,
                        _ => &mut m.output,
                    };
                    *at = Some(i);
                }
            }
            (Region::Mixer, id::MX_PARAMS) => l.mixer_params = Some(i),
            _ => {}
        }
        i += 1;
    }
    Ok(l)
}

fn scan_channel(events: &[Event], start: usize, end: usize) -> ChannelLoc {
    let mut c = ChannelLoc { start, end, ..Default::default() };
    for k in start + 1..end {
        match events[k].id {
            id::DISPLAY_NAME => c.name = Some(k),
            id::PLUGIN_REC => c.plugin_rec = Some(k),
            id::PLUGIN_NAME => c.plugin_name = Some(k),
            id::PLUGIN_STATE => c.state = Some(k),
            id::COLOR => c.color = Some(k),
            id::CH_COLOR_FLAG => c.color_flag = Some(k),
            id::CH_ENABLED if c.enabled.is_none() => c.enabled = Some(k),
            id::CH_LEVELS => c.levels = Some(k),
            id::CH_TARGET => c.target = Some(k),
            id::AUTO_DATA => c.auto = Some(k),
            id::SAMPLE_PATH => c.sample = Some(k),
            20 => c.ev20 = Some(k),
            id::CH_ZIPPED => c.zipped = Some(k),
            id::CH_LOCKED => c.locked = Some(k),
            id::CH_GROUP => c.group = Some(k),
            id::CH_LAYER_CHILD => c.last_child = Some(k),
            _ => {}
        }
    }
    c
}

/// The major version of the program that saved the file (26 when the file does not say).
fn major_version(events: &[Event], version: Option<usize>) -> u32 {
    version
        .and_then(|i| events[i].data())
        .map(|d| decode_text(d, TextEnc::Ansi))
        .and_then(|s| s.split('.').next().and_then(|m| m.parse::<u32>().ok()))
        .unwrap_or(26)
}

/// True when the program that saved the file is older than FL 12.8 (a file that does not say is not).
fn before_12_8(events: &[Event], version: Option<usize>) -> bool {
    let text = version.and_then(|i| events[i].data()).map(|d| decode_text(d, TextEnc::Ansi)).unwrap_or_default();
    let mut parts = text.split('.').map(|p| p.trim_matches(char::from(0)).parse::<u32>().ok());
    match (parts.next().flatten(), parts.next().flatten()) {
        (Some(major), minor) => (major, minor.unwrap_or(0)) < (12, 8),
        _ => false,
    }
}

fn text_enc(events: &[Event], version: Option<usize>) -> TextEnc {
    if major_version(events, version) <= 11 { TextEnc::Ansi } else { TextEnc::Utf16 }
}

/// The record length a clip list fits: a whole number of records, each with `00 50` at byte 4, trying 88, 80, 60
/// and 32 bytes (spec: flp/layouts.md, "How to tell the generation").
fn list_clip_len(data: &[u8]) -> Option<usize> {
    [CLIP_LEN, CLIP_LEN_25, CLIP_LEN_21, CLIP_LEN_20]
        .into_iter()
        .find(|&n| !data.is_empty() && data.len() % n == 0 && data.chunks_exact(n).all(|r| r[4..6] == [0x00, 0x50]))
}

/// The clip record length of a file: the one its clip lists fit, else the one its version suggests (spec:
/// flp/layouts.md, "How to tell the generation").
fn clip_len(events: &[Event], version: Option<usize>) -> usize {
    events
        .iter()
        .filter(|e| e.id == id::ARR_CLIPS)
        .find_map(|e| e.data().and_then(list_clip_len))
        .unwrap_or_else(|| if major_version(events, version) < 26 { CLIP_LEN_25 } else { CLIP_LEN })
}

// ---------------------------------------------------------------------------------------
// Load

/// Reads the typed project from a parsed file.
pub fn load(file: &FlpFile) -> Result<Project, ProjectError> {
    let events = events_of(file)?;
    let header = file.header().ok_or(ProjectError::NoHeader)?;
    let mut l = scan(events)?;
    let enc = text_enc(events, l.version);
    l.enc = Some(enc);
    let clen = clip_len(events, l.version);
    // FL writes 194, 206, 207 and 195 empty when they are empty (spec: flp/events/project-info.md "Writer notes"): an
    // empty text reads as no value.
    let text = |i: Option<usize>| i.and_then(|i| events[i].data()).map(|d| decode_text(d, enc)).filter(|t| !t.is_empty());
    let init_controls = decode_init_controls(l.init_controls.and_then(|i| events[i].data()).unwrap_or(&[]));

    let mut p = Project {
        saved_by: l
            .version
            .and_then(|i| events[i].data())
            .map(|d| decode_text(d, TextEnc::Ansi))
            .unwrap_or_default(),
        ppq: header.ppq().unwrap_or(96),
        // The stored tempo. The initial values' tempo record is not read into it: the app sets the initial values on
        // opening in song mode, as FL does (spec: flp/engine-events.md "Event 216").
        tempo: l.tempo.map(|i| f64::from(int(&events[i])) / 1000.0),
        info: ProjectInfo {
            title: text(l.title),
            author: text(l.author),
            genre: text(l.genre),
            comments: text(l.comments).map(comments_from_file),
            url: text(l.url),
        },
        current_pattern: l.current_pattern.map(|i| int(&events[i]) as u16),
        current_arrangement: l.current_arrangement.map(|i| int(&events[i]) as u16),
        swing: l.swing.map(|i| int(&events[i]) as u8),
        groups: l.groups.iter().map(|&i| events[i].data().map(|d| decode_text(d, enc)).unwrap_or_default()).collect(),
        current_group: l.current_group.map(|i| int(&events[i])).filter(|&v| v != u32::MAX),
        bar: l.time_num.zip(l.time_den).map(|(n, d)| (int(&events[n]) as u16, int(&events[d]) as u16)).filter(|&(n, d)| n > 0 && d > 0),
        bar_as_signature: l.setting(id::TIME_MODE).map(|i| int(&events[i]) != 0),
        song_mode: l.setting(id::SONG_MODE).map(|i| int(&events[i]) != 0),
        play_truncated: l.truncated.is_some_and(|i| int(&events[i]) != 0),
        panning_law: l.setting(id::PANNING_LAW).map(|i| int(&events[i]) as u8),
        fast_declick: l.fast_declick.map(|i| int(&events[i]) != 0),
        auto_crossfades: l.setting(id::AUTO_CROSSFADES).map(|i| int(&events[i]) != 0),
        main_pitch: l.main_pitch.map(|i| int(&events[i]) as u16 as i16),
        main_volume: main_volume(l.mixer_params.and_then(|i| events[i].data()).unwrap_or(&[])),
        playlist_view: l.playlist_view.and_then(|i| events[i].data()).and_then(PlaylistView::from_bytes),
        work_time: l.work_time.and_then(|i| events[i].data()).and_then(work_time_from),
        show_info: l.show_info.map(|i| int(&events[i]) != 0),
        // Events 167 and 202 (spec: ui/data-folder.md §2); 202 is read as it is, empty or not.
        data_folder: (l.info_167.is_some() || l.info_202.is_some()).then(|| DataFolder {
            choice: l.info_167.map_or(0, |i| int(&events[i])),
            folder: l.info_202.and_then(|i| events[i].data()).map(|d| decode_text(d, enc)).unwrap_or_default(),
        }),
        init_controls,
        controller_links: l
            .ctl_links
            .iter()
            .filter_map(|&(i, f)| {
                let mut c = events[i].data().and_then(os_model::ControllerLink::from_bytes)?;
                c.formula = f.and_then(|f| events[f].data()).map(|d| decode_text(d, enc));
                Some(c)
            })
            .collect(),
        ..Default::default()
    };

    for (pos, loc) in l.channels.iter().enumerate() {
        let mut ch = load_channel(events, loc, enc);
        ch.rack.origin = Some(pos as u16);
        if let Some(d) = loc.auto.and_then(|i| events[i].data()) {
            if let Some(a) = decode_automation(ch.index, d) {
                p.automations.push(a);
            }
        }
        p.channels.push(ch);
    }
    for &i in &l.links {
        let d = events[i].data().unwrap_or(&[]);
        let ch = u16::from_le_bytes([d[2], d[3]]);
        let target = u32::from_le_bytes(d[8..12].try_into().unwrap());
        // A clip drives every target it has a link for, in file order (spec: flp/events/automation.md "Event 227").
        if let Some(a) = p.automations.iter_mut().find(|a| a.channel == ch) {
            a.targets.push(target);
        }
    }
    for (&number, loc) in &l.patterns {
        let length = loc.length.map(|i| int(&events[i])).filter(|&v| v != 0);
        let mut markers = decode_markers(events, &loc.markers, enc);
        markers.retain(|m| !(u32::from(m.kind) == LENGTH_MARKER_KIND && Some(m.position) == length));
        p.patterns.push(Pattern {
            id: os_model::Id::NONE,
            number,
            name: text(loc.name),
            color: loc.color.map(|i| int(&events[i])).filter(|v| v >> 24 == 0xFF).map(rgb_from_bgr),
            notes: loc.notes.and_then(|i| events[i].data()).map(decode_notes).unwrap_or_default(),
            controls: loc.controls.and_then(|i| events[i].data()).map(decode_controls).unwrap_or_default(),
            length,
            markers,
            loops: loc
                .loops
                .chunks_exact(2)
                .filter(|p| events[p[0]].id == id::LOOP_CHANNEL && events[p[1]].id == id::LOOP_STEPS)
                .map(|p| (int(&events[p[0]]) as u16, int(&events[p[1]])))
                .collect(),
            loop_off: loc
                .loop_off
                .iter()
                .any(|&i| events[i].id == id::LOOP_OFF && int(&events[i]) != 0)
                .then(|| loc.loop_off.iter().find(|&&i| events[i].id == id::LOOP_TYPE).map_or(LOOP_TYPE_STEP, |&i| int(&events[i]))),
        });
    }
    let mut a_index = 0;
    for (n, e) in events.iter().enumerate() {
        if e.id == id::ARR_START {
            let mut arr = Arrangement { number: int(e) as u16, ..Default::default() };
            // Name and clips follow the start event, before the first track record.
            for f in &events[n + 1..] {
                match f.id {
                    id::ARR_NAME => arr.name = f.data().map(|d| decode_text(d, enc)),
                    id::ARR_CLIPS => arr.clips = f.data().map(|d| decode_clips(d, clen)).unwrap_or_default(),
                    id::PL_TRACK | id::ARR_START => break,
                    _ => {}
                }
            }
            if let Some(loc) = l.arrangements.get(a_index) {
                arr.tracks = loc.tracks.iter().map(|t| load_pl_track(events, t, enc)).collect();
                arr.markers = decode_markers(events, &loc.markers, enc);
            }
            a_index += 1;
            p.arrangements.push(arr);
        }
    }
    p.mixer = load_mixer(events, &l, enc);
    Ok(p)
}

fn load_channel(events: &[Event], loc: &ChannelLoc, enc: TextEnc) -> Channel {
    let block = &events[loc.start..=loc.end];
    let find = |id: u8| block.iter().find(|e| e.id == id);
    let kind = match find(id::CH_TYPE).map(int) {
        Some(0) => ChannelKind::Sampler,
        Some(2) => ChannelKind::Plugin,
        Some(3) => ChannelKind::Layer,
        Some(4) => ChannelKind::AudioClip,
        Some(5) => ChannelKind::Automation,
        Some(v) => ChannelKind::Other(v as u8),
        None => ChannelKind::Other(255),
    };
    let levels = loc.levels.and_then(|i| events[i].data()).unwrap_or(&[]);
    let i32_at = |o: usize| levels.get(o..o + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()));
    let custom = loc.color_flag.is_some_and(|i| int(&events[i]) != 0);
    Channel {
        id: os_model::Id::NONE,
        index: int(&block[0]) as u16,
        kind,
        name: loc.name.and_then(|i| events[i].data()).map(|d| decode_text(d, enc)),
        plugin: find(id::PLUGIN_NAME)
            .and_then(Event::data)
            .map(|d| decode_text(d, enc))
            .filter(|s| !s.is_empty()),
        plugin_state: loc.state.and_then(|i| events[i].data()).map(<[u8]>::to_vec),
        color: loc.color.filter(|_| custom).map(|i| rgb_from_bgr(int(&events[i]))),
        enabled: loc.enabled.is_none_or(|i| int(&events[i]) != 0),
        pan: i32_at(0).map_or(0.0, |v| f64::from(v) / 6400.0 - 1.0),
        volume: i32_at(4).map_or(0.78125, |v| f64::from(v) / 12800.0),
        pitch_cents: i32_at(8).unwrap_or(0),
        mixer_track: loc.target.map_or(0, |i| int(&events[i]) as u16),
        sample_path: find(id::SAMPLE_PATH).and_then(Event::data).map(|d| decode_text(d, enc)),
        layer_children: block
            .iter()
            .filter(|e| e.id == id::CH_LAYER_CHILD)
            .map(|e| int(e) as u16)
            .collect(),
        envelopes: decode_envelopes(block),
        rack: ChannelRack {
            origin: None,
            locked: loc.locked.is_some_and(|i| int(&events[i]) != 0),
            zipped: loc.zipped.is_some_and(|i| int(&events[i]) != 0),
            group: loc.group.map_or(0, |i| int(&events[i])),
            stored_color: loc.color.map(|i| rgb_from_bgr(int(&events[i]))),
        },
        misc: load_misc(block),
        sound: load_sound(block, &i32_at),
    }
}

/// The Misc tab values the file stores outside the plugin state: level adjustments (event 229: pan offset,
/// volume multiplier, -, mod X offset, mod Y offset), swing share (97, 0..=128), time shift (89, 0..=1024), root note
/// (135), fine tune (142), the main pitch and add-to-key switches, the gate time and the key zone (215 bytes 11 and 63,
/// ints at 64, 68 and 72), the arpeggiator (215 ints at 40..56 and 92, byte 60), the echo delay (209, 138, 215 byte 10),
/// tracking (228 twice), cut groups (132) and the voice settings (221) (spec: flp/engine-events.md "A channel",
/// audio/voice-levels.md, audio/swing.md, audio/sampler.md, audio/channel-misc.md, audio/note-rules.md,
/// flp/events/channels.md). `write_misc` is the inverse. Full portamento is not read (its storage is OpenStudio-specs
/// REQUESTS.md #5).
fn load_misc(block: &[Event]) -> ChannelMisc {
    let find = |id: u8| block.iter().find(|e| e.id == id);
    let mut m = ChannelMisc::default();
    if let Some(d) = find(id::CH_ADJUST).and_then(Event::data).filter(|d| d.len() >= 20) {
        let v = |i: usize| i32::from_le_bytes(d[4 * i..4 * i + 4].try_into().unwrap()) as f32;
        m.levels = [v(0) / 6400.0, (v(1) - 12800.0) / 12800.0, v(3) / 256.0, v(4) / 256.0];
    }
    if let Some(e) = find(id::CH_SWING_SHARE) {
        m.swing = int(e) as f32 / 128.0;
    }
    if let Some(e) = find(id::CH_ROOT) {
        m.root = int(e).min(127) as u8;
    }
    if let Some(e) = find(id::CH_FINE) {
        m.fine = int(e) as i32;
    }
    if let Some(e) = find(id::CH_TIME_SHIFT) {
        m.shift = (int(e) as f32 / 1024.0).clamp(0.0, 1.0);
    }
    if let Some(d) = find(id::CH_SAMPLE_SETTINGS).and_then(Event::data) {
        let int_at = |o: usize| d.get(o..o + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap()));
        if let Some(&b) = d.get(10) {
            m.echo.ping_pong = b & ECHO_PING_PONG != 0;
            m.echo.fat = b & ECHO_FAT != 0;
        }
        if let Some(&b) = d.get(11) {
            m.main_pitch = b != 0;
        }
        let a = &mut m.arp;
        if let Some(v) = int_at(40) {
            a.direction = v.clamp(0, 255) as u8;
        }
        if let Some(v) = int_at(44) {
            a.range = v.max(0) as u32;
        }
        if let Some(v) = int_at(48) {
            a.chord = v;
        }
        if let Some(v) = int_at(52) {
            a.time = gate_knob(v);
        }
        if let Some(v) = int_at(56) {
            a.gate = (v as f32 / 48.0).clamp(0.0, 1.0);
        }
        if let Some(&b) = d.get(60) {
            a.slide = b != 0;
        }
        if let Some(v) = int_at(92) {
            a.repeat = v.max(0) as u32;
        }
        if let Some(&b) = d.get(63) {
            m.add_key = b != 0;
        }
        if let Some(v) = int_at(64) {
            m.gate = gate_knob(v);
        }
        for (k, v) in m.key_range.iter_mut().enumerate() {
            if let Some(z) = int_at(68 + 4 * k) {
                *v = z;
            }
        }
    }
    // Echo delay: event 209's five ints, event 138's mod moves.
    if let Some(d) = find(id::CH_ECHO).and_then(Event::data).filter(|d| d.len() >= 20) {
        let v = |i: usize| i32::from_le_bytes(d[4 * i..4 * i + 4].try_into().unwrap()) as f32;
        let e = &mut m.echo;
        e.feed = (v(0) / 25600.0).clamp(0.0, 1.0);
        e.pan = ((v(1) - 6400.0) / 12800.0).clamp(-1.0, 1.0);
        e.pitch = (v(2) / 1200.0).clamp(-1.0, 1.0);
        e.echoes = v(3).max(0.0) as u32;
        e.time = (v(4) / 384.0).clamp(0.0, 1.0);
    }
    if let Some(e) = find(id::CH_ECHO_MOD) {
        let v = int(e);
        m.echo.mod_y = echo_mod(v & 0xffff);
        m.echo.mod_x = echo_mod(v >> 16);
    }
    // Tracking: the first event 228 by volume, the second by key.
    for (t, d) in m.tracking.iter_mut().zip(block.iter().filter(|e| e.id == id::CH_TRACKING).filter_map(Event::data)) {
        if d.len() >= 16 {
            let v = |i: usize| i32::from_le_bytes(d[4 * i..4 * i + 4].try_into().unwrap());
            t.mid = v(0).clamp(0, 255) as u8;
            [t.pan, t.mod_x, t.mod_y] = [1, 2, 3].map(|i| (v(i) as f32 / 256.0).clamp(-1.0, 1.0));
        }
    }
    if let Some(e) = find(id::CH_CUT_GROUPS) {
        let v = int(e);
        m.cut = v & 0xffff;
        m.cut_by = v >> 16;
    }
    if let Some(d) = find(id::CH_POLY).and_then(Event::data).filter(|d| d.len() >= 9) {
        let u = |i: usize| u32::from_le_bytes(d[i..i + 4].try_into().unwrap());
        m.max_poly = u(0);
        m.slide = (u(4) as f32 / 1024.0).clamp(0.0, 1.0);
        m.mono = d[8] & 1 != 0;
        m.porta = d[8] & 2 != 0;
    }
    m.layer = load_layer(block);
    m
}

/// A layer's flags (event 144) and crossfade position (event 215, int at byte 76) (spec: audio/note-rules.md
/// "Settings").
fn load_layer(block: &[Event]) -> os_model::LayerSettings {
    let find = |id: u8| block.iter().find(|e| e.id == id);
    os_model::LayerSettings {
        flags: find(id::CH_LAYER_FLAGS).map_or(0, int),
        crossfade: find(id::CH_SAMPLE_SETTINGS)
            .and_then(Event::data)
            .and_then(|d| d.get(76..80))
            .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap())),
    }
}

/// The gate knob (0..=1, 1 = off) of a stored gate time, 450..=1447 with 1447 = off (spec: audio/channel-misc.md
/// "Settings"), and back.
fn gate_knob(v: i32) -> f32 {
    ((v - 450) as f32 / 997.0).clamp(0.0, 1.0)
}

fn gate_stored(knob: f32) -> i32 {
    450 + (knob.clamp(0.0, 1.0) * 997.0).round() as i32
}

/// Event 215 byte 10: the echo delay's ping-pong and fat mode (spec: audio/channel-misc.md "Settings").
const ECHO_PING_PONG: u8 = 2;
const ECHO_FAT: u8 = 4;

/// An echo mod move (0..=256, 128 = the value stays) as a bipolar knob, and back.
fn echo_mod(v: u32) -> f32 {
    ((v as f32 - 128.0) / 128.0).clamp(-1.0, 1.0)
}

fn echo_mod_stored(knob: f32) -> u32 {
    (128.0 + knob.clamp(-1.0, 1.0) * 128.0).round() as u32
}

/// Event 209's five ints: echo feed, pan, pitch (cents), number of echoes, time.
fn echo_ints(e: &os_model::Echo) -> [i32; 5] {
    [
        (e.feed * 25600.0).round() as i32,
        (6400.0 + e.pan * 12800.0).round() as i32,
        (e.pitch * 1200.0).round() as i32,
        e.echoes as i32,
        (e.time * 384.0).round() as i32,
    ]
}

/// Event 138: echo mod Y in the low word, mod X in the high word.
fn echo_mods(e: &os_model::Echo) -> u32 {
    echo_mod_stored(e.mod_y) | echo_mod_stored(e.mod_x) << 16
}

/// An event 228's four ints: the mid volume or key, the amounts for pan, mod X and mod Y (-256..=256).
fn tracking_ints(t: &os_model::Tracking) -> [i32; 4] {
    let amount = |v: f32| (v * 256.0).round() as i32;
    [i32::from(t.mid), amount(t.pan), amount(t.mod_x), amount(t.mod_y)]
}

/// The event 215 fields of the Misc tab (`load_misc`, `load_layer`): the echo flags (byte 10), the main pitch switch,
/// the arpeggiator (direction, range, chord, time, gate, slide, repeat), the add-to-key switch, the gate time, the key
/// zone and a layer's crossfade position.
fn misc_215_fields(m: &ChannelMisc) -> [(usize, Vec<u8>); 14] {
    let echo = if m.echo.ping_pong { ECHO_PING_PONG } else { 0 } | if m.echo.fat { ECHO_FAT } else { 0 };
    let a = &m.arp;
    let int = |v: i32| v.to_le_bytes().to_vec();
    [
        (10, vec![echo]),
        (11, vec![u8::from(m.main_pitch)]),
        (40, int(i32::from(a.direction))),
        (44, int(a.range as i32)),
        (48, int(a.chord)),
        (52, int(gate_stored(a.time))),
        (56, int((a.gate.clamp(0.0, 1.0) * 48.0).round() as i32)),
        (60, vec![u8::from(a.slide)]),
        (63, vec![u8::from(m.add_key)]),
        (64, int(gate_stored(m.gate))),
        (68, int(m.key_range[0])),
        (72, int(m.key_range[1])),
        (76, int(m.layer.crossfade)),
        (92, int(a.repeat as i32)),
    ]
}

/// A new event 215 for a channel that has none: FL's record of a new channel (`audio_clip_215`) holding the channel's
/// own values in every field the reader takes from it, so adding it changes nothing else.
fn new_sample_settings(ch: &Channel) -> Vec<u8> {
    let mut d = audio_clip_215(ch.sound.stretch);
    for (at, bytes) in sample_settings_fields(ch).iter().chain(&misc_215_fields(&ch.misc)) {
        d[*at..*at + bytes.len()].copy_from_slice(bytes);
    }
    d
}

/// Writes channel `ch`'s Misc tab values into its block (`block`, from its event 64 to its end event 51), `old` being
/// what the block reads as now (the inverse of `load_misc` and `load_layer`; spec: audio/voice-levels.md, audio/swing.md,
/// audio/channel-misc.md, audio/note-rules.md, flp/events/channels.md). Events the block holds are patched, only in
/// the values that changed; a missing event is returned, to go before the end event, when the value differs from what
/// reading the block without it gives. Every channel kind keeps them.
fn write_misc(block: &mut [Event], ch: &Channel, old: &ChannelMisc) -> Vec<Event> {
    let (m, n) = (&ch.misc, old);
    let mut add = Vec::new();
    if m == n {
        return add;
    }
    let find = |block: &[Event], id: u8| block.iter().position(|e| e.id == id);
    let ints = |v: &[i32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    // Level adjustments (229): pan offset, volume multiplier, -, mod X offset, mod Y offset.
    let adjust = |l: &[f32; 4]| {
        [(l[0] * 6400.0).round() as i32, (l[1] * 12800.0 + 12800.0).round() as i32, 0, (l[2] * 256.0).round() as i32, (l[3] * 256.0).round() as i32]
    };
    if m.levels != n.levels {
        let (v, was) = (adjust(&m.levels), adjust(&n.levels));
        match find(block, id::CH_ADJUST) {
            Some(i) => {
                for k in [0, 1, 3, 4].into_iter().filter(|&k| v[k] != was[k]) {
                    patch_bytes(&mut block[i], 4 * k, &v[k].to_le_bytes());
                }
            }
            None => add.push(Event::new(id::CH_ADJUST, Value::var(ints(&v)))),
        }
    }
    // Plain integer events: swing share, time shift, cut groups, layer flags, root note, fine tune.
    let cuts = |m: &ChannelMisc| m.cut & 0xffff | m.cut_by << 16;
    for (eid, v, was) in [
        (id::CH_SWING_SHARE, (m.swing * 128.0).round() as u32, (n.swing * 128.0).round() as u32),
        (id::CH_TIME_SHIFT, (m.shift * 1024.0).round() as u32, (n.shift * 1024.0).round() as u32),
        (id::CH_CUT_GROUPS, cuts(m), cuts(n)),
        (id::CH_LAYER_FLAGS, m.layer.flags, n.layer.flags),
        (id::CH_ROOT, u32::from(m.root), u32::from(n.root)),
        (id::CH_FINE, m.fine as u32, n.fine as u32),
    ] {
        if v == was {
            continue;
        }
        match find(block, eid) {
            Some(i) => set_int(&mut block[i], v),
            None => add.push(int_event(eid, v)),
        }
    }
    // Voice settings (221): maximum voices, slide time, mono (value 1) and portamento (value 2) in byte 8.
    let slide = |m: &ChannelMisc| (m.slide * 1024.0).round() as u32;
    let bits = |m: &ChannelMisc| u8::from(m.mono) | u8::from(m.porta) << 1;
    if (m.max_poly, slide(m), bits(m)) != (n.max_poly, slide(n), bits(n)) {
        match find(block, id::CH_POLY) {
            Some(i) => {
                if m.max_poly != n.max_poly {
                    patch_bytes(&mut block[i], 0, &m.max_poly.to_le_bytes());
                }
                if slide(m) != slide(n) {
                    patch_bytes(&mut block[i], 4, &slide(m).to_le_bytes());
                }
                // The byte's other bits are kept.
                if let Some(&b) = block[i].data().and_then(|d| d.get(8)) {
                    patch_bytes(&mut block[i], 8, &[b & !3 | bits(m)]);
                }
            }
            None => {
                let d: Vec<u8> = [m.max_poly.to_le_bytes(), slide(m).to_le_bytes()].concat().into_iter().chain([bits(m)]).collect();
                add.push(Event::new(id::CH_POLY, Value::var(d)));
            }
        }
    }
    let (fields, was) = (misc_215_fields(m), misc_215_fields(n));
    if fields != was {
        match find(block, id::CH_SAMPLE_SETTINGS) {
            // Only the fields that changed; a shorter record (older files) keeps what does not fit.
            Some(i) => {
                for ((at, bytes), _) in fields.iter().zip(&was).filter(|(f, w)| f != w) {
                    match (at, block[i].data().and_then(|d| d.get(10))) {
                        // The echo flags' byte keeps its other bits.
                        (10, Some(&b)) => patch_bytes(&mut block[i], 10, &[b & !(ECHO_PING_PONG | ECHO_FAT) | bytes[0]]),
                        _ => patch_bytes(&mut block[i], *at, bytes),
                    }
                }
            }
            None => add.push(Event::new(id::CH_SAMPLE_SETTINGS, Value::var(new_sample_settings(ch)))),
        }
    }
    // Echo delay: event 209's ints that changed, event 138 whole.
    let (v, was) = (echo_ints(&m.echo), echo_ints(&n.echo));
    if v != was {
        match find(block, id::CH_ECHO) {
            Some(i) => {
                for k in (0..5).filter(|&k| v[k] != was[k]) {
                    patch_bytes(&mut block[i], 4 * k, &v[k].to_le_bytes());
                }
            }
            None => add.push(Event::new(id::CH_ECHO, Value::var(ints(&v)))),
        }
    }
    if echo_mods(&m.echo) != echo_mods(&n.echo) {
        match find(block, id::CH_ECHO_MOD) {
            Some(i) => set_int(&mut block[i], echo_mods(&m.echo)),
            None => add.push(int_event(id::CH_ECHO_MOD, echo_mods(&m.echo))),
        }
    }
    // Tracking: the block's first and second event 228. A missing second one needs the first before it, so the first
    // is added too (with its own values) when both are missing.
    let held: Vec<usize> = block.iter().enumerate().filter(|(_, e)| e.id == id::CH_TRACKING).map(|(i, _)| i).take(2).collect();
    let (v, was) = (m.tracking.map(|t| tracking_ints(&t)), n.tracking.map(|t| tracking_ints(&t)));
    let last = (0..2).rev().find(|&k| v[k] != was[k]);
    for k in 0..last.map_or(0, |k| k + 1) {
        match held.get(k) {
            Some(&i) => {
                for j in (0..4).filter(|&j| v[k][j] != was[k][j]) {
                    patch_bytes(&mut block[i], 4 * j, &v[k][j].to_le_bytes());
                }
            }
            None => add.push(Event::new(id::CH_TRACKING, Value::var(ints(&v[k])))),
        }
    }
    add
}

/// Event 215 of a new audio clip channel as FL 26 writes it (declick mode 1, not stretched), its stretch ints
/// (bytes 96..112) replaced by the channel's (spec: flp/engine-events.md "A channel", audio/audio-clips.md).
const AUDIO_CLIP_215: &str = "8c0000000000000001000000ffffffff3c0000000000803f0000803f0000803f0000803f0000803f000000000300000000000000000400003000000000000100a7050000000000000001000000000000000000010000000000000000010000000000000000000000000000000000000002000000feffffffffffffff000000000000000000000000000000000000f03f0000000000000000ffffffff01010000000000000000e03f";

fn audio_clip_215(stretch: [u32; 4]) -> Vec<u8> {
    let mut d: Vec<u8> = (0..AUDIO_CLIP_215.len()).step_by(2).map(|i| u8::from_str_radix(&AUDIO_CLIP_215[i..i + 2], 16).unwrap_or(0)).collect();
    for (k, v) in stretch.iter().enumerate() {
        d[96 + 4 * k..100 + 4 * k].copy_from_slice(&v.to_le_bytes());
    }
    d
}

/// Filter (event 219 ints 3..5), sampler flags (143), declick mode and stretch settings (215).
fn load_sound(block: &[Event], levels: &dyn Fn(usize) -> Option<i32>) -> ChannelSound {
    let find = |id: u8| block.iter().find(|e| e.id == id);
    let mut s = ChannelSound::default();
    s.cutoff = levels(12).unwrap_or(s.cutoff);
    s.resonance = levels(16).unwrap_or(s.resonance);
    s.filter_type = levels(20).unwrap_or(s.filter_type);
    s.sampler_flags = find(id::CH_SAMPLER_FLAGS).map(int);
    if let Some(d) = find(id::CH_SAMPLE_SETTINGS).and_then(Event::data) {
        s.declick = d.get(83).copied().unwrap_or(0);
        for (k, v) in s.stretch.iter_mut().enumerate() {
            if let Some(b) = d.get(96 + 4 * k..100 + 4 * k) {
                *v = u32::from_le_bytes(b.try_into().unwrap());
            }
        }
        if let Some(b) = d.get(144..152) {
            s.start_offset_bits = u64::from_le_bytes(b.try_into().unwrap());
        }
    }
    s.loop_points = find(id::CH_LOOP_POINTS).and_then(Event::data).and_then(|d| d.get(4..20)).map(|d| {
        let at = |i: usize| i64::from_le_bytes(d[i..i + 8].try_into().unwrap());
        [at(0), at(8)]
    });
    s.fx = load_sample_fx(block);
    s
}

/// Event 251 as FL 26 writes it for a sample whose loop points stand at its ends (frames -1 and `i64::MAX`), the
/// frames replaced by `points` (spec: flp/events/channels.md "Loop points").
fn loop_points_251(points: [i64; 2]) -> Vec<u8> {
    let mut d = 2u32.to_le_bytes().to_vec();
    for v in points {
        d.extend_from_slice(&v.to_le_bytes());
    }
    d.extend_from_slice(&(-1f64).to_le_bytes());
    d.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x00, 0xff, 0xff, 0xff, 0x7f, 0xff, 0xff, 0xff, 0x7f, 0x00, 0x00]);
    d
}

/// A channel's start, length, loop options and precomputed effects (spec: audio/precomputed-effects.md "Settings");
/// what the file does not hold keeps the default. `apply` writes them for sampler channels (`write_sampler`).
fn load_sample_fx(block: &[Event]) -> SampleFx {
    let find = |id: u8| block.iter().find(|e| e.id == id).map(int);
    let mut f = SampleFx::default();
    let word = |id: u8, v: &mut u16| {
        if let Some(x) = find(id) {
            *v = x as u16;
        }
    };
    word(id::SMP_FLAGS, &mut f.flags);
    word(id::SMP_BOOST, &mut f.boost);
    word(id::SMP_FADE_OUT, &mut f.fade_out);
    word(id::SMP_FADE_IN, &mut f.fade_in);
    word(id::SMP_STEREO_DELAY, &mut f.stereo_delay);
    word(id::SMP_POGO, &mut f.pogo);
    word(id::SMP_FEEDBACK, &mut f.feedback);
    word(id::SMP_CUTOFF, &mut f.cutoff);
    word(id::SMP_RESONANCE, &mut f.resonance);
    if let Some(x) = find(id::SMP_RING) {
        (f.ring_mix, f.ring_freq) = (x as u16, (x >> 16) as u16);
    }
    if let Some(x) = find(id::SMP_REVERB) {
        (f.reverb, f.reverb_type) = (x as u16, (x >> 16) as u16);
    }
    if let Some(x) = find(id::SMP_PING_PONG) {
        f.ping_pong = x != 0;
    }
    if let Some(d) = block.iter().find(|e| e.id == id::CH_SAMPLE_SETTINGS).and_then(Event::data) {
        let byte = |o: usize, v: &mut bool| {
            if let Some(&b) = d.get(o) {
                *v = b != 0;
            }
        };
        byte(9, &mut f.remove_dc);
        byte(80, &mut f.normalize);
        byte(81, &mut f.polarity);
        byte(156, &mut f.loop_follows_reverse);
        let u32_at = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
        let u64_at = |o: usize| d.get(o..o + 8).map(|b| u64::from_le_bytes(b.try_into().unwrap()));
        f.crossfade = u32_at(84).unwrap_or(f.crossfade);
        f.trim = u32_at(88).unwrap_or(f.trim);
        f.start_bits = u64_at(128).unwrap_or(f.start_bits);
        f.length_bits = u64_at(136).unwrap_or(f.length_bits);
    }
    f
}

/// The five 68-byte envelope records of a channel (spec: audio/channel-envelopes.md); missing or short
/// records stay at the defaults.
fn decode_envelopes(block: &[Event]) -> [Envelope; 5] {
    let mut out = [Envelope::default(); 5];
    for (slot, e) in out.iter_mut().zip(block.iter().filter(|e| e.id == id::CH_ENVELOPE)) {
        let Some(d) = e.data().filter(|d| d.len() >= 68) else { continue };
        let v = |i: usize| i32::from_le_bytes(d[4 * i..4 * i + 4].try_into().unwrap());
        *slot = Envelope {
            enabled: v(1) != 0,
            delay: v(2),
            attack: v(3),
            hold: v(4),
            decay: v(5),
            sustain: v(6),
            release: v(7),
            amount: v(8),
            attack_tension: v(14),
            decay_tension: v(15),
            release_tension: v(16),
            flags: v(0),
            lfo_delay: v(9),
            lfo_attack: v(10),
            lfo_amount: v(11),
            lfo_speed: v(12),
            lfo_shape: v(13),
        };
    }
    out
}

fn decode_note(r: &[u8]) -> Note {
    let u16_at = |o: usize| u16::from_le_bytes([r[o], r[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
    Note {
        position: u32_at(0),
        channel: u16_at(6),
        length: u32_at(8),
        key: u16_at(12),
        fine_pitch: r[16],
        release: r[18],
        pan: r[20],
        velocity: r[21],
        mod_x: r[22],
        mod_y: r[23],
        flags: u16_at(4),
        group: r[19],
        unk14: u16_at(14),
        repeat: r[17],
    }
}

fn decode_notes(data: &[u8]) -> Vec<Note> {
    data.chunks_exact(NOTE_LEN).map(decode_note).collect()
}

const CONTROL_LEN: usize = 12;

fn decode_controls(data: &[u8]) -> Vec<ControlPoint> {
    data.chunks_exact(CONTROL_LEN)
        .map(|r| ControlPoint {
            position: u32::from_le_bytes(r[0..4].try_into().unwrap()),
            kind: u32::from_le_bytes(r[4..8].try_into().unwrap()),
            value: i32::from_le_bytes(r[8..12].try_into().unwrap()),
        })
        .collect()
}

fn encode_controls(points: &[ControlPoint]) -> Vec<u8> {
    // Ordered by position; equal positions keep their order (spec: patterns.md).
    let mut sorted = points.to_vec();
    sorted.sort_by_key(|p| p.position);
    let mut data = Vec::with_capacity(sorted.len() * CONTROL_LEN);
    for p in &sorted {
        data.extend_from_slice(&p.position.to_le_bytes());
        data.extend_from_slice(&p.kind.to_le_bytes());
        data.extend_from_slice(&p.value.to_le_bytes());
    }
    data
}

fn encode_note(n: &Note) -> [u8; NOTE_LEN] {
    let mut r = [0u8; NOTE_LEN];
    r[0..4].copy_from_slice(&n.position.to_le_bytes());
    r[4..6].copy_from_slice(&(n.flags | NOTE_FLAGS_DEFAULT).to_le_bytes());
    r[6..8].copy_from_slice(&n.channel.to_le_bytes());
    r[8..12].copy_from_slice(&n.length.to_le_bytes());
    r[12..14].copy_from_slice(&n.key.to_le_bytes());
    r[14..16].copy_from_slice(&n.unk14.to_le_bytes());
    r[16] = n.fine_pitch;
    r[17] = n.repeat;
    r[19] = n.group;
    r[18] = n.release;
    r[20] = n.pan;
    r[21] = n.velocity;
    r[22] = n.mod_x;
    r[23] = n.mod_y;
    r
}

/// Bit of record byte 19 for a muted clip (spec: flp/events/playlist.md).
const CLIP_MUTED: u8 = 0x20;

fn decode_clip(r: &[u8]) -> Clip {
    let u16_at = |o: usize| u16::from_le_bytes([r[o], r[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
    let f32_at = |o: usize| f32::from_le_bytes(r[o..o + 4].try_into().unwrap());
    let item = u16_at(6);
    let source = if item >= 0x5000 { ClipSource::Pattern(item - 0x5000) } else { ClipSource::Channel(item) };
    // Pattern clips: u32 ticks, `ffffffff` = none; others: f32, -1 = none (spec: "Clip offsets").
    let window = match source {
        ClipSource::Pattern(_) => (u32_at(24) != u32::MAX).then(|| (f64::from(u32_at(24)), f64::from(u32_at(28)))),
        ClipSource::Channel(_) => {
            let (a, b) = (f32_at(24), f32_at(28));
            (a != -1.0 && a.is_finite() && b.is_finite()).then(|| (f64::from(a), f64::from(b)))
        }
    };
    // A shorter record (80 bytes) leaves the rest of `raw` zero.
    let mut raw = [0u8; CLIP_RECORD_LEN];
    let n = r.len().min(CLIP_RECORD_LEN);
    raw[..n].copy_from_slice(&r[..n]);
    Clip {
        id: os_model::Id::NONE,
        position: u32_at(0),
        length: u32_at(8),
        track: 499u16.saturating_sub(u16_at(12)),
        source,
        muted: r[19] & CLIP_MUTED != 0,
        window,
        raw,
    }
}

fn decode_clips(data: &[u8], len: usize) -> Vec<Clip> {
    data.chunks_exact(len).map(decode_clip).collect()
}

/// The record of a clip: its stored bytes with the modeled fields written over them.
fn encode_clip(c: &Clip) -> [u8; CLIP_LEN] {
    let mut r = c.raw;
    r[0..4].copy_from_slice(&c.position.to_le_bytes());
    r[4..6].copy_from_slice(&0x5000u16.to_le_bytes());
    let item = match c.source {
        ClipSource::Pattern(n) => n + 0x5000,
        ClipSource::Channel(ch) => ch,
    };
    r[6..8].copy_from_slice(&item.to_le_bytes());
    r[8..12].copy_from_slice(&c.length.to_le_bytes());
    r[12..14].copy_from_slice(&499u16.saturating_sub(c.track).to_le_bytes());
    r[19] = if c.muted { r[19] | CLIP_MUTED } else { r[19] & !CLIP_MUTED };
    // Offsets are rewritten only when they changed (keeps odd "none" encodings as read).
    let same_kind = matches!((decode_clip(&c.raw).source, c.source), (ClipSource::Pattern(_), ClipSource::Pattern(_)) | (ClipSource::Channel(_), ClipSource::Channel(_)));
    let template_none = matches!(c.source, ClipSource::Channel(_)) && r[24..28] == [0xff; 4];
    if same_kind && !template_none && decode_clip(&c.raw).window == c.window {
        return r;
    }
    match (c.source, c.window) {
        (ClipSource::Pattern(_), None) => r[24..32].fill(0xff),
        (ClipSource::Pattern(_), Some((a, b))) => {
            r[24..28].copy_from_slice(&(a.max(0.0).round() as u32).to_le_bytes());
            r[28..32].copy_from_slice(&(b.max(0.0).round() as u32).to_le_bytes());
        }
        (ClipSource::Channel(_), None) => {
            r[24..28].copy_from_slice(&(-1.0f32).to_le_bytes());
            r[28..32].copy_from_slice(&(-1.0f32).to_le_bytes());
        }
        (ClipSource::Channel(_), Some((a, b))) => {
            r[24..28].copy_from_slice(&(a as f32).to_le_bytes());
            r[28..32].copy_from_slice(&(b as f32).to_le_bytes());
        }
    }
    r
}

/// Clip records of `len` bytes sorted by position. A clip without an id, or with one already used (a copy), gets
/// the next free id (spec: playlist.md, byte 32). 80-byte records hold 0 at bytes 76..79 (spec: flp/layouts.md).
fn encode_clips(clips: &[Clip], len: usize) -> Vec<u8> {
    let mut sorted = clips.to_vec();
    sorted.sort_by_key(|c| c.position);
    let mut next = sorted.iter().map(Clip::id).max().unwrap_or(0) + 1;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(sorted.len() * len);
    for c in &mut sorted {
        if c.id() == 0 || !seen.insert(c.id()) {
            c.set_id(next);
            seen.insert(next);
            next += 1;
        }
        let mut r = encode_clip(c);
        if len < CLIP_LEN {
            r[76..80].fill(0);
        }
        out.extend_from_slice(&r[..len]);
    }
    out
}

fn decode_markers(events: &[Event], idx: &[usize], enc: TextEnc) -> Vec<TimeMarker> {
    let mut out: Vec<TimeMarker> = Vec::new();
    for &i in idx {
        let e = &events[i];
        match e.id {
            id::MARKER_POS => {
                let v = int(e);
                out.push(TimeMarker { kind: (v >> 24) as u8, ..TimeMarker::new(v & 0x00FF_FFFF, "") });
            }
            id::MARKER_NUM => out.last_mut().into_iter().for_each(|m| m.numerator = int(e) as u8),
            id::MARKER_SCALE => out.last_mut().into_iter().for_each(|m| m.scale = int(e) as u16),
            id::MARKER_DEN => out.last_mut().into_iter().for_each(|m| m.denominator = int(e) as u8),
            id::MARKER_NAME => {
                let name = e.data().map(|d| decode_text(d, enc)).unwrap_or_default();
                out.last_mut().into_iter().for_each(|m| m.name = name.clone());
            }
            _ => {}
        }
    }
    out
}

fn marker_events(m: &TimeMarker, enc: TextEnc) -> Vec<Event> {
    vec![
        Event::new(id::MARKER_POS, Value::U32(m.position & 0x00FF_FFFF | u32::from(m.kind) << 24)),
        Event::new(id::MARKER_NUM, Value::U8(m.numerator)),
        Event::new(id::MARKER_DEN, Value::U8(m.denominator)),
        Event::new(46, Value::U8(0)),
        Event::new(45, Value::U8(0)),
        Event::new(168, Value::U32(0)),
        Event::new(id::MARKER_SCALE, Value::U16(m.scale)),
        Event::new(id::MARKER_NAME, Value::var(encode_text(&m.name, enc))),
    ]
}

const AUTO_HEADER_LEN: usize = 21;
const AUTO_POINT_LEN: usize = 24;
/// Behind the curve's points: where the count of the LFO envelope's points sits, and its points begin (spec:
/// flp/layouts.md "An automation clip's record").
const LFO_ENV_COUNT: usize = 44;
const LFO_ENV_POINTS: usize = 48;
/// Behind the envelope's points: the LFO's flags byte, 3 unused bytes, its switch, three ints, speed, tension, skew,
/// pulse width and phase offset.
const LFO_VALUES_LEN: usize = 40;

/// A new automation clip's event-234 header and tail (spec: flp/layouts.md "An automation clip's record"), with the
/// fields written from the points and the LFO left at 0: the curve's head (kind 4, version 3), the curve's flags byte 1
/// and its four ints at -1, the four ints 128, 128, 0, 128, the LFO envelope's head (kind 5, version 3), the LFO's
/// three ints at -1.
fn auto_template() -> (Vec<u8>, Vec<u8>) {
    let h = vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0];
    let mut t = vec![1, 0, 0, 0];
    t.extend([0xff; 16]);
    t.extend([0x80, 0, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0, 0, 5, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0]);
    t.extend([0; 8]);
    t.extend([0xff; 12]);
    t.extend([0; 20]);
    (h, t)
}

/// A new automation channel's data: a flat line at `value` over `beats` (two points, as FL creates one).
pub fn new_automation(channel: u16, target: u32, value: f64, beats: f64) -> Automation {
    flat_automation(channel, vec![target], os_model::target::bipolar(target), value, beats)
}

/// ADD > Automation clip's data (spec: ui/add-clip-channels.md §2): a flat line at `value` over `beats` and no target
/// (no event 227); FL stored header byte 8 (the LFO's mode) as 1 for it.
pub fn untargeted_automation(channel: u16, value: f64, beats: f64) -> Automation {
    flat_automation(channel, Vec::new(), true, value, beats)
}

/// A flat line; a bipolar target's LFO adds (FL stores the LFO's mode as 1 for one).
fn flat_automation(channel: u16, targets: Vec<u32>, bipolar: bool, value: f64, beats: f64) -> Automation {
    let (header, tail) = auto_template();
    let mut first = AutoPoint::new(0.0, value);
    first.flag = 0;
    let lfo = ClipLfo { mode: u8::from(bipolar), ..ClipLfo::default() };
    Automation { channel, targets, points: vec![first, AutoPoint::new(beats.max(0.0), value)], header, tail, lfo }
}

fn le_i32(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// `n` point records (24 bytes each), positions made absolute (stored: the distance from the previous point).
fn decode_points(d: &[u8]) -> Vec<AutoPoint> {
    let mut beat = 0.0;
    d.chunks_exact(AUTO_POINT_LEN)
        .map(|r| {
            beat += f64::from_le_bytes(r[0..8].try_into().unwrap());
            AutoPoint {
                beat,
                value: f64::from_le_bytes(r[8..16].try_into().unwrap()),
                tension: f32::from_le_bytes(r[16..20].try_into().unwrap()),
                mode: AutoMode::from_byte(r[20]),
                flag: r[23],
            }
        })
        .collect()
}

fn encode_points(d: &mut Vec<u8>, pts: &[AutoPoint]) {
    let mut prev = 0.0;
    for p in pts {
        d.extend_from_slice(&(p.beat - prev).max(0.0).to_le_bytes());
        prev = p.beat;
        d.extend_from_slice(&p.value.to_le_bytes());
        d.extend_from_slice(&p.tension.to_le_bytes());
        d.extend([p.mode.byte(), 0, 0, p.flag]);
    }
}

/// Where the LFO's values begin in `tail` (the bytes behind the curve's points), if the tail holds the whole layout.
fn lfo_values_at(tail: &[u8]) -> Option<usize> {
    let m = u32::from_le_bytes(tail.get(LFO_ENV_COUNT..LFO_ENV_POINTS)?.try_into().unwrap()) as usize;
    let o = LFO_ENV_POINTS.checked_add(m.checked_mul(AUTO_POINT_LEN)?)?;
    (tail.len() >= o.checked_add(LFO_VALUES_LEN)?).then_some(o)
}

/// The LFO read out of an event-234 `header` and `tail`, and both with what was read zeroed and the envelope's points
/// taken out (they are written from the model's `lfo`). A tail without the whole layout keeps the defaults.
fn split_lfo(header: &mut [u8], tail: &[u8]) -> (ClipLfo, Vec<u8>) {
    let mut lfo = ClipLfo { amount: le_i32(header, 4), mode: header[8], ..ClipLfo::default() };
    header[4..9].fill(0);
    let Some(o) = lfo_values_at(tail) else { return (lfo, tail.to_vec()) };
    lfo.envelope = decode_points(&tail[LFO_ENV_POINTS..o]);
    let v = &tail[o..o + LFO_VALUES_LEN];
    (lfo.flags, lfo.switch, lfo.speed) = (v[0], le_i32(v, 4), le_i32(v, 20));
    (lfo.tension, lfo.skew, lfo.pulse_width, lfo.phase) = (le_i32(v, 24), le_i32(v, 28), le_i32(v, 32), le_i32(v, 36));
    let mut rest = tail[..LFO_ENV_COUNT].to_vec();
    rest.extend([0; 4]);
    let mut v = v.to_vec();
    v[0] = 0;
    for f in [4, 20, 24, 28, 32, 36] {
        v[f..f + 4].fill(0);
    }
    rest.extend(v);
    rest.extend_from_slice(&tail[o + LFO_VALUES_LEN..]);
    (lfo, rest)
}

/// `tail` with the LFO `lfo` written into it: its envelope's count and points, its values. A tail without the layout
/// is written as it is while the LFO is at its defaults, else the template's tail is used.
fn join_lfo(tail: &[u8], lfo: &ClipLfo) -> Vec<u8> {
    let template;
    let (tail, o) = match lfo_values_at(tail) {
        Some(o) => (tail, o),
        None if *lfo == ClipLfo::default() => return tail.to_vec(),
        None => {
            template = auto_template().1;
            (&template[..], LFO_ENV_POINTS)
        }
    };
    let mut d = tail[..LFO_ENV_COUNT].to_vec();
    d.extend_from_slice(&(lfo.envelope.len() as u32).to_le_bytes());
    encode_points(&mut d, &lfo.envelope);
    let mut v = tail[o..o + LFO_VALUES_LEN].to_vec();
    v[0] = lfo.flags;
    for (f, x) in [(4, lfo.switch), (20, lfo.speed), (24, lfo.tension), (28, lfo.skew), (32, lfo.pulse_width), (36, lfo.phase)] {
        v[f..f + 4].copy_from_slice(&x.to_le_bytes());
    }
    d.extend(v);
    d.extend_from_slice(&tail[o + LFO_VALUES_LEN..]);
    d
}

fn decode_automation(channel: u16, d: &[u8]) -> Option<Automation> {
    if d.len() < AUTO_HEADER_LEN {
        return None;
    }
    let n = u32::from_le_bytes(d[17..21].try_into().unwrap()) as usize;
    let end = AUTO_HEADER_LEN.checked_add(n.checked_mul(AUTO_POINT_LEN)?)?;
    if d.len() < end {
        return None;
    }
    let points = decode_points(&d[AUTO_HEADER_LEN..end]);
    let mut header = d[..AUTO_HEADER_LEN].to_vec();
    header[17..21].fill(0); // the count is written from the points
    let (lfo, tail) = split_lfo(&mut header, &d[end..]);
    Some(Automation { channel, targets: Vec::new(), points, header, tail, lfo })
}

fn encode_automation(a: &Automation) -> Vec<u8> {
    let mut d = a.header.clone();
    d.resize(AUTO_HEADER_LEN, 0);
    d[4..8].copy_from_slice(&a.lfo.amount.to_le_bytes());
    d[8] = a.lfo.mode;
    d[17..21].copy_from_slice(&(a.points.len() as u32).to_le_bytes());
    encode_points(&mut d, &a.points);
    d.extend(join_lfo(&a.tail, &a.lfo));
    d
}

/// Event 227 linking automation channel `channel` to `target` (spec: flp/events/automation.md).
fn link_event(channel: u16, target: u32) -> Event {
    let mut d = vec![0u8; 20];
    d[2..4].copy_from_slice(&channel.to_le_bytes());
    d[8..12].copy_from_slice(&target.to_le_bytes());
    d[12..16].copy_from_slice(&8u32.to_le_bytes());
    d[16..20].copy_from_slice(&0x1D5u32.to_le_bytes());
    Event::new(id::AUTO_LINK, Value::var(d))
}

fn load_pl_track(events: &[Event], t: &PlTrackLoc, enc: TextEnc) -> PlaylistTrack {
    let r = events[t.record].data().unwrap_or(&[]);
    let custom = t.color_flag.is_some_and(|i| int(&events[i]) != 0);
    PlaylistTrack {
        number: r.get(0..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap())),
        name: t.name.and_then(|i| events[i].data()).map(|d| decode_text(d, enc)),
        color: r
            .get(4..8)
            .filter(|_| custom)
            .map(|b| rgb_from_bgr(u32::from_le_bytes(b.try_into().unwrap()))),
        enabled: r.get(12).is_none_or(|&b| b != 0),
        grouped: r.get(46).is_some_and(|&b| b != 0),
        collapsed: r.get(61).is_some_and(|&b| b == 0),
        locked: r.get(47).is_some_and(|&b| b != 0),
    }
}

fn mixer_param_records(data: &[u8]) -> impl Iterator<Item = (usize, u8, u8, u16, i32)> + '_ {
    data.chunks_exact(MIXER_REC_LEN).enumerate().map(|(n, r)| {
        (
            n * MIXER_REC_LEN,
            r[4],
            r[5],
            u16::from_le_bytes([r[6], r[7]]),
            i32::from_le_bytes(r[8..12].try_into().unwrap()),
        )
    })
}

const MX_KIND_PARAM: u8 = 0x1F;
const MX_KIND_ROUTE: u8 = 0x20;
const MX_VOLUME: u8 = 0xC0;
const MX_PAN: u8 = 0xC1;
const MX_STEREO: u8 = 0xC2;
const MX_EQ_GAIN: u8 = 0xD0;
const MX_EQ_FREQ: u8 = 0xD8;
const MX_EQ_WIDTH: u8 = 0xE0;

/// Per mixer track, the plugin-state event (213) of each slot 0-9 that holds a plugin (spec: flp/events/mixer.md
/// "Slot sub-block": a slot's plugin events come before its 98 marker). Track ranges as in `load_mixer`.
fn slot_state_events(events: &[Event], l: &Layout) -> Vec<[Option<usize>; 10]> {
    let starts: Vec<usize> = l.mixer.iter().map(|m| m.color.or(m.color_flag).or(m.name).or(m.flags).unwrap_or(0)).collect();
    (0..l.mixer.len())
        .map(|n| {
            let to = starts.get(n + 1).copied().or(l.mixer_params).unwrap_or(events.len());
            let mut out = [None; 10];
            let mut pending: Option<Option<usize>> = None;
            for (i, e) in events.iter().enumerate().take(to).skip(starts[n]) {
                match e.id {
                    id::PLUGIN_NAME => pending = Some(None),
                    id::PLUGIN_STATE => {
                        if let Some(p) = pending.as_mut() {
                            *p = Some(i);
                        }
                    }
                    id::MX_SLOT => {
                        if let (Some(o), Some(p)) = (out.get_mut(int(e) as usize), pending.take()) {
                            *o = p;
                        }
                    }
                    _ => {}
                }
            }
            out
        })
        .collect()
}

fn mixer_location(track: u16, slot: u16) -> u16 {
    0x7000 + track * 64 + slot
}

/// The track number a stored track's slot plugins (event 212) and parameter records (event 225) use: the last stored
/// track is the mixer's "current" track, numbered 501 there (spec: flp/events/mixer.md "The last track").
fn record_track(index: u16, len: usize) -> u16 {
    if index > 0 && usize::from(index) + 1 == len {
        MX_SPECIAL_TRACK
    } else {
        index
    }
}

/// Slot parameter IDs (kind 0x1F at the slot's location) and the mix level of a full mix (spec:
/// flp/events/plugins.md §5).
const MX_SLOT_ON: u8 = 0;
const MX_SLOT_MIX: u8 = 1;
const SLOT_MIX_FULL: f64 = 12800.0;

/// Sets parameter record (`MX_KIND_PARAM`, `pid`) at `loc`, or adds it after the records of that location (before the
/// first record of a later location) when the file has none.
fn set_param_record(data: &mut Vec<u8>, loc: u16, pid: u8, v: i32) {
    set_record(data, MX_KIND_PARAM, loc, pid, v);
}

/// Sets the record (`kind`, `pid`) at `loc`, or adds it before the first record of a later location.
fn set_record(data: &mut Vec<u8>, kind: u8, loc: u16, pid: u8, v: i32) {
    let found = mixer_param_records(data).find(|&(_, p, k, at, _)| k == kind && p == pid && at == loc).map(|r| r.0);
    if let Some(off) = found {
        data[off + 8..off + 12].copy_from_slice(&v.to_le_bytes());
        return;
    }
    let at = mixer_param_records(data)
        .find(|&(_, _, _, l, _)| l > loc)
        .map_or(data.len() - data.len() % MIXER_REC_LEN, |r| r.0);
    data.splice(at..at, param_rec(pid, kind, loc, v));
}

/// Event 216's records, whole ones only (spec: ui/settings-choices.md "Advanced: Initialize controls automatically").
fn decode_init_controls(data: &[u8]) -> Vec<InitControl> {
    data.chunks_exact(InitControl::LEN).map(|r| InitControl::from_bytes(r.try_into().unwrap())).collect()
}

/// The initial values as event 216 holds them; `old` (the event's bytes before) keeps any bytes past its last whole
/// record.
fn init_controls_bytes(list: &[InitControl], old: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = list.iter().flat_map(|c| c.to_bytes()).collect();
    out.extend_from_slice(&old[old.len() - old.len() % InitControl::LEN..]);
    out
}

/// The main volume: record id 0, kind 0 at place 0x4000 (spec: audio/mixer.md "Main volume").
fn main_volume(params: &[u8]) -> Option<u16> {
    mixer_param_records(params)
        .find(|&(_, p, k, loc, _)| p == 0 && k == 0 && loc == MX_SPECIAL_LOC)
        .map(|r| r.4.clamp(0, 16000) as u16)
}

/// Send records of files up to FL 24.1: kind 0x1F, id 64 + destination (destinations 0..=126) at the source's place
/// (spec: flp/layouts.md "Parameter ids"); later files use kind 0x20 with the destination as the id.
const MX_SEND_OLD: std::ops::RangeInclusive<u8> = 64..=190;

fn is_send(kind: u8, pid: u8) -> bool {
    kind == MX_KIND_ROUTE || (kind == MX_KIND_PARAM && MX_SEND_OLD.contains(&pid))
}

/// A send record's destination track.
fn send_dest(kind: u8, pid: u8) -> u16 {
    if kind == MX_KIND_ROUTE { u16::from(pid) } else { u16::from(pid - 64) }
}

/// Event 236 flags (spec: flp/layouts.md "A mixer track's record (event 236)"): reverse polarity, swap left / right,
/// effect slots enabled, track on. A 5-byte record (FL 10 and 11) holds no flags: all at their defaults.
const MX_FLAG_POLARITY: u8 = 0x01;
const MX_FLAG_SWAP: u8 = 0x02;
const MX_FLAG_SLOTS: u8 = 0x04;
const MX_FLAG_ON: u8 = 0x08;

fn track_flags(d: Option<&[u8]>) -> Option<u8> {
    d.filter(|d| d.len() >= 8).map(|d| d[4])
}

/// Flag 0x20 of a record written before FL 12.8: its one latency value is the output setting.
const MX_FLAG_LATE_OUT: u8 = 0x20;

/// A track record's output and input latency settings in milliseconds (spec: flp/layouts.md "A mixer track's record
/// (event 236)"): at bytes 0..3 and 8..11 of a 12-byte record, the output setting alone at 0..3 of an 8-byte one. A
/// record written before FL 12.8 (5 or 8 bytes) holds one value, with the other sign: the output setting when flag
/// 0x20 is set (in a 5-byte record: its fifth byte is not 0), else the input setting.
fn track_latency(d: &[u8], old: bool) -> (f32, f32) {
    let single = |at: usize| d.get(at..at + 4).map_or(0.0, |b| f32::from_le_bytes(b.try_into().expect("four bytes")));
    match d.len() {
        n if n >= 12 => (single(0), single(8)),
        n if n >= 8 && !old => (single(0), 0.0),
        n if n >= 5 => {
            let v = 0.0 - single(0);
            let out = if n >= 8 { d[4] & MX_FLAG_LATE_OUT != 0 } else { d[4] != 0 };
            if out { (v, 0.0) } else { (0.0, v) }
        }
        _ => (0.0, 0.0),
    }
}

/// Record `d` (8 bytes or more) with the latency settings `(out, in)`: in its own form where that holds them (the one
/// value of a record written before FL 12.8 when the other setting is 0, flag 0x20 left as it was when both are; the
/// output setting alone in a later 8-byte record), else grown to the 12-byte record, its flags kept (a Lemon Studio
/// choice: spec audio/delay-compensation.md "Lemon Studio").
fn with_latency(d: &[u8], old: bool, (out, inp): (f32, f32)) -> Vec<u8> {
    let mut r = d.to_vec();
    if r.len() < 12 {
        if !old && inp == 0.0 {
            r[..4].copy_from_slice(&out.to_le_bytes());
            return r;
        }
        if old && (out == 0.0 || inp == 0.0) {
            r[..4].copy_from_slice(&(0.0 - out - inp).to_le_bytes());
            if out != 0.0 || inp != 0.0 {
                r[4] = (r[4] & !MX_FLAG_LATE_OUT) | if inp == 0.0 { MX_FLAG_LATE_OUT } else { 0 };
            }
            return r;
        }
        r.resize(12, 0);
        r[4] &= !MX_FLAG_LATE_OUT;
    }
    r[..4].copy_from_slice(&out.to_le_bytes());
    r[8..12].copy_from_slice(&inp.to_le_bytes());
    r
}

fn with_flags(b: u8, m: &MixerTrack) -> u8 {
    let mut b = b & !(MX_FLAG_POLARITY | MX_FLAG_SWAP | MX_FLAG_SLOTS | MX_FLAG_ON);
    for (on, f) in [(m.polarity, MX_FLAG_POLARITY), (m.swap, MX_FLAG_SWAP), (m.slots_enabled, MX_FLAG_SLOTS), (m.enabled, MX_FLAG_ON)] {
        if on {
            b |= f;
        }
    }
    b
}

/// A mixer track's slot area (spec: flp/events/mixer.md "Slot sub-block"): the events between its flags (236) and its
/// routing (235), and each filled slot's plugin events (its 201 up to, not including, its 98 marker).
struct SlotArea {
    from: usize,
    to: usize,
    runs: [Option<(usize, usize)>; 10],
}

fn slot_areas(events: &[Event], l: &Layout) -> Vec<Option<SlotArea>> {
    l.mixer
        .iter()
        .map(|m| {
            let (f, t) = (m.flags?, m.routes?);
            if t <= f {
                return None;
            }
            let mut runs = [None; 10];
            let mut start = None;
            for (i, e) in events.iter().enumerate().take(t).skip(f + 1) {
                match e.id {
                    id::PLUGIN_NAME => start = Some(i),
                    id::MX_SLOT => {
                        if let (Some(r), Some(s)) = (runs.get_mut(int(e) as usize), start.take()) {
                            *r = Some((s, i));
                        }
                    }
                    _ => {}
                }
            }
            Some(SlotArea { from: f + 1, to: t, runs })
        })
        .collect()
}

/// A slot plugin's events as the file held them, while slot areas are rewritten.
struct SlotRun {
    track: usize,
    slot: usize,
    plugin: SlotPlugin,
    events: Vec<Event>,
    used: bool,
}

/// Every filled slot of the file with its plugin and events, by track position (spec: flp/events/mixer.md "Slot
/// sub-block"), and the number of stored tracks.
fn file_slot_runs(file: &FlpFile) -> Result<(Vec<SlotRun>, usize), ProjectError> {
    let events = events_of(file)?;
    let l = scan(events)?;
    let enc = text_enc(events, l.version);
    let mut out = Vec::new();
    for (track, area) in slot_areas(events, &l).iter().enumerate() {
        for (slot, run) in area.iter().flat_map(|a| a.runs.iter().enumerate()) {
            let Some((s, e)) = *run else { continue };
            let run = &events[s..e];
            let name = run[0].data().map(|d| decode_text(d, enc)).unwrap_or_default();
            let state = run.iter().find(|e| e.id == id::PLUGIN_STATE).and_then(Event::data).map(<[u8]>::to_vec).unwrap_or_default();
            out.push(SlotRun { track, slot, plugin: SlotPlugin { id: os_model::Id::NONE, name, state }, events: run.to_vec(), used: false });
        }
    }
    Ok((out, l.mixer.len()))
}

/// The file events of each wanted slot plugin, given as (slot, the file track its track came from, the plugin). Rounds
/// of matching, a closer match first for every slot: a plugin of that name in the same slot of the track it came from;
/// the same plugin (name and state) moved within that track; the same plugin moved from another track; a plugin of that
/// name moved within the track it came from. A track made in Lemon Studio (no file track) only finds the same plugin
/// moved from another track.
fn take_runs(pool: &mut [SlotRun], wanted: &[(usize, Option<usize>, &SlotPlugin)]) -> Vec<Option<Vec<Event>>> {
    type Rule = fn(&SlotRun, usize, Option<usize>, &SlotPlugin) -> bool;
    let rounds: [Rule; 4] = [
        |r, k, from, p| Some(r.track) == from && r.slot == k && r.plugin.name == p.name,
        |r, _, from, p| Some(r.track) == from && r.plugin == *p,
        |r, _, _, p| r.plugin == *p,
        |r, _, from, p| Some(r.track) == from && r.plugin.name == p.name,
    ];
    let mut out: Vec<Option<Vec<Event>>> = vec![None; wanted.len()];
    for rule in rounds {
        for (w, &(k, from, p)) in wanted.iter().enumerate() {
            if out[w].is_some() {
                continue;
            }
            if let Some(r) = pool.iter_mut().find(|r| !r.used && rule(r, k, from, p)) {
                r.used = true;
                out[w] = Some(r.events.clone());
            }
        }
    }
    out
}

/// A slot plugin's events (spec: flp/events/plugins.md §2): the file's own events when it had the plugin (window
/// record moved to the new place, state replaced), else new ones.
fn slot_plugin_events(p: &SlotPlugin, track: u16, slot: u8, run: Option<Vec<Event>>, enc: TextEnc) -> Vec<Event> {
    let Some(mut ev) = run else {
        let mut ev = vec![
            Event::new(id::PLUGIN_NAME, Value::var(encode_text(&p.name, enc))),
            Event::new(id::PLUGIN_REC, Value::var(plugin_record(Some((track, slot))))),
            Event::new(id::PLUGIN_ICON, Value::U32(0)),
            Event::new(id::COLOR, Value::U32(SLOT_COLOR_DEFAULT)),
            Event::new(id::CH_COLOR_FLAG, Value::U8(0)),
        ];
        // A plugin without a known state starts at its defaults (spec: some plugins store no 213).
        if !p.state.is_empty() {
            ev.push(Event::new(id::PLUGIN_STATE, Value::var(p.state.clone())));
        }
        return ev;
    };
    for e in ev.iter_mut().filter(|e| e.id == id::PLUGIN_REC) {
        let mut at = [0u8; 8];
        at[0..4].copy_from_slice(&u32::from(track).to_le_bytes());
        at[4..8].copy_from_slice(&u32::from(slot).to_le_bytes());
        patch_bytes(e, 0, &at);
    }
    match ev.iter().position(|e| e.id == id::PLUGIN_STATE) {
        Some(i) => set_data(&mut ev[i], p.state.clone()),
        None if !p.state.is_empty() => ev.push(Event::new(id::PLUGIN_STATE, Value::var(p.state.clone()))),
        None => {}
    }
    ev
}

/// Length of a plugin window record (event 212).
const PLUGIN_REC_LEN: usize = 52;
/// Colour FL stores for most slot plugins (spec: flp/events/plugins.md §1, corpus).
const SLOT_COLOR_DEFAULT: u32 = 0x0056_5148;

/// A new plugin window record (event 212, spec: flp/events/plugins.md §3): `+8 = 2`; channels flags 0x50
/// (generator, threaded); mixer slots carry their track and slot, flags 0x140.
fn plugin_record(slot: Option<(u16, u8)>) -> Vec<u8> {
    let mut r = vec![0u8; PLUGIN_REC_LEN];
    r[8] = 2;
    let flags: u32 = match slot {
        Some((t, s)) => {
            r[0..4].copy_from_slice(&u32::from(t).to_le_bytes());
            r[4..8].copy_from_slice(&u32::from(s).to_le_bytes());
            0x140
        }
        None => 0x50,
    };
    r[16..20].copy_from_slice(&flags.to_le_bytes());
    r
}

/// `t` with the track's audio input and output, record source and monitor mode from its block (a file without them
/// keeps the defaults; spec: audio/track-io.md §1.6).
fn io_of(events: &[Event], m: &MixerLoc, mut t: MixerTrack) -> MixerTrack {
    let at = |i: Option<usize>| i.map(|i| int(&events[i]));
    if let Some(v) = at(m.input) {
        t.audio_in = (v as i32).max(-1);
    }
    if let Some(v) = at(m.output) {
        t.audio_out = (v as i32).max(-1);
    }
    if let Some(v) = at(m.rec_source) {
        t.rec_source = v.min(255) as u8;
    }
    if let Some(v) = at(m.monitor) {
        t.monitor = v.min(255) as u8;
    }
    t
}

fn load_mixer(events: &[Event], l: &Layout, enc: TextEnc) -> Vec<MixerTrack> {
    let params = l.mixer_params.and_then(|i| events[i].data()).unwrap_or(&[]);
    let old = before_12_8(events, l.version);
    let mut out = Vec::new();
    // Track boundaries: header start of track n .. header start of track n+1.
    let starts: Vec<usize> = l
        .mixer
        .iter()
        .map(|m| m.color.or(m.color_flag).or(m.name).or(m.flags).unwrap_or(0))
        .collect();
    for (n, m) in l.mixer.iter().enumerate() {
        let index = n as u16;
        let from = starts[n];
        let to = starts.get(n + 1).copied().or(l.mixer_params).unwrap_or(events.len());
        let mut slots: [Option<SlotPlugin>; 10] = Default::default();
        let mut pending_plugin: Option<SlotPlugin> = None;
        let mut routes = Vec::new();
        for e in &events[from..to] {
            match e.id {
                id::PLUGIN_NAME => {
                    pending_plugin = e.data().map(|d| SlotPlugin { id: os_model::Id::NONE, name: decode_text(d, enc), state: Vec::new() });
                }
                // spec: flp/events/mixer.md (slot sub-block: 213 = plugin state)
                id::PLUGIN_STATE => {
                    if let (Some(p), Some(d)) = (pending_plugin.as_mut(), e.data()) {
                        p.state = d.to_vec();
                    }
                }
                id::MX_SLOT => {
                    if let Some(s) = slots.get_mut(int(e) as usize) {
                        *s = pending_plugin.take();
                    }
                }
                id::MX_ROUTES => {
                    routes = e
                        .data()
                        .unwrap_or(&[])
                        .iter()
                        .enumerate()
                        .filter(|&(_, &b)| b != 0)
                        .map(|(k, _)| k as u16)
                        .collect();
                }
                _ => {}
            }
        }
        let custom = m.color_flag.is_some_and(|i| int(&events[i]) != 0);
        // Slot LED and mix (spec: flp/events/plugins.md §5).
        let mut slot_on = [true; 10];
        let mut slot_mix = [1.0; 10];
        let rt = record_track(index, l.mixer.len());
        let base = mixer_location(rt, 0);
        // The stored switch and mix level; the initial values' records for them are set by the app on opening in song
        // mode (spec: flp/engine-events.md "Event 216").
        for (_, pid, kind, loc, v) in mixer_param_records(params) {
            if kind == MX_KIND_PARAM && (base..base + 10).contains(&loc) {
                let s = usize::from(loc - base);
                match pid {
                    MX_SLOT_ON => slot_on[s] = v != 0,
                    MX_SLOT_MIX => slot_mix[s] = f64::from(v) / SLOT_MIX_FULL,
                    _ => {}
                }
            }
        }
        let param = |pid: u8| {
            mixer_param_records(params)
                .find(|&(_, p, k, loc, _)| k == MX_KIND_PARAM && p == pid && loc == base)
                .map(|r| r.4)
        };
        // The equaliser: gain 0xD0 + band, frequency 0xD8 + band, width 0xE0 + band (spec: flp/events/mixer.md).
        let mut eq = os_model::EQ_DEFAULT;
        for (b, band) in eq.iter_mut().enumerate() {
            for (k, first) in [MX_EQ_GAIN, MX_EQ_FREQ, MX_EQ_WIDTH].into_iter().enumerate() {
                if let Some(v) = param(first + b as u8) {
                    band[k] = v;
                }
            }
        }
        let flags = track_flags(m.flags.and_then(|i| events[i].data()));
        let flag = |f: u8, default: bool| flags.map_or(default, |b| b & f != 0);
        let (latency_out, latency_in) = m.flags.and_then(|i| events[i].data()).map_or((0.0, 0.0), |d| track_latency(d, old));
        out.push(MixerTrack {
            index,
            name: m.name.and_then(|i| events[i].data()).map(|d| decode_text(d, enc)),
            color: m.color.filter(|_| custom).map(|i| rgb_from_bgr(int(&events[i]))),
            enabled: flag(MX_FLAG_ON, true),
            polarity: flag(MX_FLAG_POLARITY, false),
            swap: flag(MX_FLAG_SWAP, false),
            slots_enabled: flag(MX_FLAG_SLOTS, true),
            slots,
            routes,
            send_levels: mixer_param_records(params)
                .filter(|&(_, pid, k, loc, _)| is_send(k, pid) && loc == base)
                .map(|(_, pid, k, _, v)| (send_dest(k, pid), f64::from(v) / 16000.0))
                .collect(),
            volume: param(MX_VOLUME).map(|v| f64::from(v) / 16000.0),
            pan: param(MX_PAN).map(|v| f64::from(v) / 6400.0),
            stereo_separation: param(MX_STEREO).map(|v| f64::from(v) / 64.0),
            slot_on,
            slot_mix,
            eq,
            latency_out,
            latency_in,
            icon: m.icon.map(|i| int(&events[i]) as u16),
            origin: Some(index),
            ..io_of(events, m, MixerTrack::new(index))
        });
    }
    out
}

// ---------------------------------------------------------------------------------------
// Apply

fn set_int(e: &mut Event, v: u32) {
    e.value = match e.value {
        Value::U8(_) => Value::U8(v as u8),
        Value::U16(_) => Value::U16(v as u16),
        Value::U32(_) => Value::U32(v),
        Value::Var { .. } => return,
    };
}

/// Replaces a variable payload, keeping the original length prefix when it still fits.
fn set_data(e: &mut Event, data: Vec<u8>) {
    if e.data() == Some(data.as_slice()) {
        return;
    }
    e.value = Value::var(data);
}

fn patch_bytes(e: &mut Event, offset: usize, bytes: &[u8]) {
    if let Some(d) = e.data() {
        if d.len() >= offset + bytes.len() && &d[offset..offset + bytes.len()] != bytes {
            let mut d = d.to_vec();
            d[offset..offset + bytes.len()].copy_from_slice(bytes);
            e.value = Value::var(d);
        }
    }
}

/// Beats per bar or steps per beat as their one-byte event holds them (17, 18; spec: flp/engine-events.md).
fn signature_byte(v: u16) -> u8 {
    v.clamp(1, 255) as u8
}

/// An integer event of `id` holding `v`, sized by its id range (spec: flp/container.md).
fn int_event(id: u8, v: u32) -> Event {
    Event::new(
        id,
        match id {
            0..=63 => Value::U8(v as u8),
            64..=127 => Value::U16(v as u16),
            _ => Value::U32(v),
        },
    )
}

/// One 68-byte envelope record (the inverse of `decode_envelopes`).
fn encode_envelope(e: &Envelope) -> [u8; 68] {
    let ints = [
        e.flags,
        // FL writes -1 for an envelope that is on (spec: audio/envelopes-and-lfos.md).
        -i32::from(e.enabled),
        e.delay,
        e.attack,
        e.hold,
        e.decay,
        e.sustain,
        e.release,
        e.amount,
        e.lfo_delay,
        e.lfo_attack,
        e.lfo_amount,
        e.lfo_speed,
        e.lfo_shape,
        e.attack_tension,
        e.decay_tension,
        e.release_tension,
    ];
    let mut d = [0u8; 68];
    for (k, v) in ints.iter().enumerate() {
        d[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
    }
    d
}

/// The event 215 fields a sampler's sound settings hold (the inverse of `load_sound` and `load_sample_fx`): remove DC,
/// normalize, polarity, declick mode, crossfade, trim, start, length, the start offset and the loop following a
/// reversal. The Misc tab's fields (main pitch, add to key, key zone, …) are `misc_215_fields`.
fn sample_settings_fields(ch: &Channel) -> [(usize, Vec<u8>); 10] {
    let (s, f) = (&ch.sound, &ch.sound.fx);
    [
        (9, vec![u8::from(f.remove_dc)]),
        (80, vec![u8::from(f.normalize)]),
        (81, vec![u8::from(f.polarity)]),
        (83, vec![s.declick]),
        (84, f.crossfade.to_le_bytes().to_vec()),
        (88, f.trim.to_le_bytes().to_vec()),
        (128, f.start_bits.to_le_bytes().to_vec()),
        (136, f.length_bits.to_le_bytes().to_vec()),
        (144, s.start_offset_bits.to_le_bytes().to_vec()),
        (156, vec![u8::from(f.loop_follows_reverse)]),
    ]
}

/// The settings of a sampler channel that Lemon Sampler edits (spec: plugins/lemon-sampler.md §7): the five envelope
/// records (218), the filter (219 ints 3..5), the sampler flags (143), the loop points (251), the event 215 fields of
/// `sample_settings_fields`, and the precomputed effects' own events (audio/precomputed-effects.md "Settings"). Events
/// the block holds are patched; a value the block lacks is added before its end event when it differs from what reading
/// the block without it gives. Root note, fine tune and the rest of the Misc tab are `write_misc`'s.
fn write_sampler(
    events: &mut [Event],
    loc: &ChannelLoc,
    ch: &Channel,
    old: &Channel,
    inserts: &mut Vec<(usize, Event)>,
    removals: &mut Vec<usize>,
) {
    let find = |events: &[Event], id: u8| (loc.start + 1..loc.end).find(|&k| events[k].id == id);
    let mut add = Vec::new();
    if ch.envelopes != old.envelopes {
        let recs: Vec<usize> = (loc.start + 1..loc.end).filter(|&k| events[k].id == id::CH_ENVELOPE).collect();
        for (n, (e, o)) in ch.envelopes.iter().zip(&old.envelopes).enumerate() {
            let rec = encode_envelope(e);
            match recs.get(n) {
                // A short record reads as the defaults: it is replaced whole.
                Some(&k) if events[k].data().is_some_and(|d| d.len() < 68) => set_data(&mut events[k], rec.to_vec()),
                Some(&k) if e != o => patch_bytes(&mut events[k], 0, &rec),
                Some(_) => {}
                // Records are matched by position: every missing one is added, in order.
                None => add.push(Event::new(id::CH_ENVELOPE, Value::var(rec.to_vec()))),
            }
        }
    }
    let (s, o) = (&ch.sound, &old.sound);
    if let Some(i) = loc.levels {
        for (at, v, was) in [(12, s.cutoff, o.cutoff), (16, s.resonance, o.resonance), (20, s.filter_type, o.filter_type)] {
            if v == was {
                continue;
            }
            let mut d = events[i].data().unwrap_or(&[]).to_vec();
            if d.len() < at + 4 {
                d.resize(at + 4, 0);
            }
            d[at..at + 4].copy_from_slice(&v.to_le_bytes());
            set_data(&mut events[i], d);
        }
    }
    if let Some(v) = s.sampler_flags.filter(|&v| Some(v) != o.sampler_flags) {
        match find(events, id::CH_SAMPLER_FLAGS) {
            Some(i) => set_int(&mut events[i], v),
            None => add.push(int_event(id::CH_SAMPLER_FLAGS, v)),
        }
    }
    if s.loop_points != o.loop_points {
        // Without loop points the record holds the sample's ends; removing the sample (the Sampler window's file box
        // X) removes it, as in FL (spec: flp/events/channels.md "Loop points (event 251)").
        let points = s.loop_points.unwrap_or([-1, i64::MAX]);
        let removed = s.loop_points.is_none() && ch.sample_path.is_none() && old.sample_path.is_some();
        match find(events, id::CH_LOOP_POINTS) {
            Some(i) if removed => removals.push(i),
            Some(i) => patch_bytes(&mut events[i], 4, &[points[0].to_le_bytes(), points[1].to_le_bytes()].concat()),
            None if removed => {}
            None => add.push(Event::new(id::CH_LOOP_POINTS, Value::var(loop_points_251(points)))),
        }
    }
    let fields = sample_settings_fields(ch);
    let changed = fields != sample_settings_fields(old);
    match find(events, id::CH_SAMPLE_SETTINGS) {
        // A shorter record (older files) keeps what does not fit. The stretch ints are written above.
        Some(i) if changed => {
            for (at, bytes) in &fields {
                patch_bytes(&mut events[i], *at, bytes);
            }
        }
        Some(_) => {}
        // When the Misc tab's 215 fields changed too, `write_misc` adds the record (with these fields): one is added.
        None if (changed || s.stretch != o.stretch) && misc_215_fields(&ch.misc) == misc_215_fields(&old.misc) => {
            add.push(Event::new(id::CH_SAMPLE_SETTINGS, Value::var(new_sample_settings(ch))));
        }
        None => {}
    }
    let (f, g) = (&s.fx, &o.fx);
    for (eid, v, was) in [
        (id::SMP_PING_PONG, u32::from(f.ping_pong), u32::from(g.ping_pong)),
        (id::SMP_FEEDBACK, f.feedback.into(), g.feedback.into()),
        (id::SMP_FLAGS, f.flags.into(), g.flags.into()),
        (id::SMP_CUTOFF, f.cutoff.into(), g.cutoff.into()),
        (id::SMP_BOOST, f.boost.into(), g.boost.into()),
        (id::SMP_FADE_OUT, f.fade_out.into(), g.fade_out.into()),
        (id::SMP_FADE_IN, f.fade_in.into(), g.fade_in.into()),
        (id::SMP_RESONANCE, f.resonance.into(), g.resonance.into()),
        (id::SMP_STEREO_DELAY, f.stereo_delay.into(), g.stereo_delay.into()),
        (id::SMP_POGO, f.pogo.into(), g.pogo.into()),
        (id::SMP_RING, u32::from(f.ring_mix) | u32::from(f.ring_freq) << 16, u32::from(g.ring_mix) | u32::from(g.ring_freq) << 16),
        (id::SMP_REVERB, u32::from(f.reverb) | u32::from(f.reverb_type) << 16, u32::from(g.reverb) | u32::from(g.reverb_type) << 16),
    ] {
        if v == was {
            continue;
        }
        match find(events, eid) {
            Some(i) => set_int(&mut events[i], v),
            None => add.push(int_event(eid, v)),
        }
    }
    // Inserts at one index land in reverse order.
    inserts.extend(add.into_iter().rev().map(|e| (loc.end, e)));
}

/// Writes the model back into `file`, changing only the events that hold modeled values
/// that differ from what's stored. Returns an error when the model's shape (number of
/// channels, patterns with notes, mixer tracks, …) doesn't match the file: adding or
/// removing objects isn't supported yet.
pub fn apply(project: &Project, file: &mut FlpFile) -> Result<(), ProjectError> {
    restructure(project, file)?;
    // The file's effect slots as they are before inserts are added or removed: a renumbered track takes its own.
    let (file_slots, file_tracks) = file_slot_runs(file)?;
    grow_mixer(project, file)?;
    shrink_mixer(project, file)?;
    let current = load(file)?;
    let events = events_of_mut(file)?;
    let l = scan(events)?;
    let enc = text_enc(events, l.version);
    let clen = clip_len(events, l.version);
    let old_file = before_12_8(events, l.version);
    let mismatch = |what: &str, a: usize, b: usize| {
        ProjectError::Mismatch(format!("{what}: model has {a}, file has {b}"))
    };
    // Channels can be added (appended after the last block), not removed.
    if project.channels.len() < current.channels.len() {
        return Err(mismatch("channels", project.channels.len(), current.channels.len()));
    }
    // A project made by `create` has no mixer or playlist in its file yet: their settings are not written then
    // (see `unsaved_parts`).
    let skip_mixer = current.mixer.is_empty();
    let skip_arrangements = current.arrangements.is_empty();
    if !skip_mixer && project.mixer.len() != current.mixer.len() {
        return Err(mismatch("mixer tracks", project.mixer.len(), current.mixer.len()));
    }

    let mut inserts: Vec<(usize, Event)> = Vec::new();
    let mut removals: Vec<usize> = Vec::new();

    // Project info.
    if let (Some(i), Some(t)) = (l.tempo, project.tempo.filter(|&t| Some(t) != current.tempo)) {
        set_int(&mut events[i], (t * 1000.0).round() as u32);
    }
    // Title, genre, author, comments, URL (spec: flp/events/project-info.md). A changed text is patched in place. A
    // cleared one keeps its event, written empty, as FL 26 writes 194, 206, 207 and 195 when they are empty; a cleared
    // URL loses its event, as FL writes 197 only when there is a URL (Writer notes). An empty field the file lacks gets
    // no event, a filled one gets its event in FL's order; so do the start and time spent (237) and "show info on
    // opening" (10) when the model has them.
    let info = &project.info;
    let old_info = &current.info;
    let fields = [
        (id::TITLE, &info.title, &old_info.title),
        (id::GENRE, &info.genre, &old_info.genre),
        (id::AUTHOR, &info.author, &old_info.author),
        (id::COMMENTS, &info.comments, &old_info.comments),
        (id::URL, &info.url, &old_info.url),
    ];
    // spec: flp/engine-events.md "Order in the file": 194, 206, 207, 167, 202, 195, [197], 237. The five texts take
    // places 0, 1, 2, 5 and 6 of it.
    let order = [l.title, l.genre, l.author, l.info_167, l.info_202, l.comments, l.url, l.work_time];
    const PLACE: [usize; 5] = [0, 1, 2, 5, 6];
    // Without any of them in the file: behind the project events that come before them (156, 67, 9, 11, 80, 17, 18,
    // 35, 23, 44, 30, 10), as `create` does.
    let info_at = [l.tempo, l.current_pattern, l.swing, l.main_pitch, l.time_num, l.time_den, l.truncated, l.other_project]
        .into_iter()
        .flatten()
        .max()
        .or(l.version)
        .map_or(0, |i| i + 1);
    // Where place `p` of that order goes: after the nearest event before it that the file has, else before the nearest
    // one after it.
    let place = |p: usize| {
        order[..p].iter().rev().find_map(|i| i.map(|i| i + 1)).or_else(|| order[p + 1..].iter().find_map(|&i| i)).unwrap_or(info_at)
    };
    // The data folder's choice and folder (167, 202; spec: ui/data-folder.md §2), places 3 and 4: patched in place, or
    // added between the author and the comments when the model's differs from the file's (a missing event reads as
    // 0 / empty, so an unchanged model adds nothing).
    let mut folder_added = Vec::new();
    if project.data_folder != current.data_folder {
        let (df, old) = (project.data_folder.clone().unwrap_or_default(), current.data_folder.clone().unwrap_or_default());
        if df.choice != old.choice {
            match l.info_167 {
                Some(i) => set_int(&mut events[i], df.choice),
                None => folder_added.push((place(3), int_event(id::INFO_167, df.choice))),
            }
        }
        if df.folder != old.folder {
            match l.info_202 {
                Some(i) => set_data(&mut events[i], encode_text(&df.folder, enc)),
                None => folder_added.push((place(4), Event::new(id::INFO_202, Value::var(encode_text(&df.folder, enc))))),
            }
        }
    }
    let mut added = Vec::new();
    for (k, &(eid, value, old)) in fields.iter().enumerate() {
        if k == 3 {
            added.append(&mut folder_added);
        }
        if value == old {
            continue;
        }
        let p = PLACE[k];
        let text = value.as_deref().map(|v| if eid == id::COMMENTS { comments_to_file(v) } else { v.to_string() });
        match (order[p], text) {
            (Some(i), v) if eid == id::URL && v.as_deref().unwrap_or("").is_empty() => removals.push(i),
            (Some(i), v) => set_data(&mut events[i], encode_text(v.as_deref().unwrap_or(""), enc)),
            (None, Some(v)) if !v.is_empty() => added.push((place(p), Event::new(eid, Value::var(encode_text(&v, enc))))),
            _ => {}
        }
    }
    // Inserts at one place end up in reverse order of pushing: 237 goes behind the texts added at its place, which keep
    // the order above, and 10 (below) before them.
    if let Some(w) = project.work_time.filter(|&w| Some(w) != current.work_time) {
        match l.work_time {
            Some(i) => set_data(&mut events[i], work_time_bytes(w)),
            None => inserts.push((place(7), Event::new(id::WORK_TIME, Value::var(work_time_bytes(w))))),
        }
    }
    inserts.extend(added.into_iter().rev());
    // Show info on opening (event 10), where the file lacks it: behind the project events, before the texts.
    if project.show_info != current.show_info {
        let v = u32::from(project.show_info.unwrap_or(false));
        match l.show_info {
            Some(i) => set_int(&mut events[i], v),
            None => {
                let first_text = order.iter().flatten().min().copied();
                inserts.push((first_text.map_or(info_at, |f| f.min(info_at)), int_event(id::SHOW_INFO, v)));
            }
        }
    }
    if let (Some(i), Some(v)) = (l.current_pattern, project.current_pattern.filter(|&v| Some(v) != current.current_pattern)) {
        set_int(&mut events[i], v.into());
    }
    if let (Some(i), Some(v)) = (l.current_arrangement, project.current_arrangement.filter(|&v| Some(v) != current.current_arrangement)) {
        set_int(&mut events[i], v.into());
    }

    // Project settings the file lacks (11, 80, 17, 18, 30) go where FL 26 writes them (spec: flp/engine-events.md
    // "Project", "Order in the file": 156, 67, 9, 11, 80, 17, 18, 35, 23, 44, 30, 10): behind the last of the settings
    // before them that the file has. `settings` holds them in that order; see below.
    let mut settings: Vec<(usize, Event)> = Vec::new();
    let unread = |eid: u8| l.unread_settings.iter().find(|s| s.0 == eid).map(|s| s.1);
    // The swing and the main pitch also stay before a 17 or 18 the file has.
    let before_signature = |at: usize| [l.time_num, l.time_den].into_iter().flatten().fold(at, usize::min);
    // Song or pattern mode (event 9): right behind 156 and 67.
    if let Some(on) = project.song_mode.filter(|&on| Some(on) != current.song_mode) {
        match unread(id::SONG_MODE) {
            Some(i) => set_int(&mut events[i], u32::from(on)),
            None => {
                let at = [l.tempo, l.current_pattern].into_iter().flatten().max().map_or(0, |i| i + 1);
                settings.push((before_signature(at), Event::new(id::SONG_MODE, Value::U8(u8::from(on)))));
            }
        }
    }
    // Channel rack settings: main swing, filter groups, current filter (spec: flp/events/project-info.md).
    if let Some(v) = project.swing.filter(|&v| Some(v) != current.swing) {
        match l.swing {
            Some(i) => set_int(&mut events[i], v.into()),
            None if v != 0 => {
                // Behind the last of 156, 67 and 9 the file has.
                let at = [l.tempo, l.current_pattern, unread(9)].into_iter().flatten().max().map_or(0, |i| i + 1);
                settings.push((before_signature(at), Event::new(id::SWING, Value::U8(v))));
            }
            None => {}
        }
    }
    // Main pitch (event 80): FL writes it right after the swing.
    if let Some(v) = project.main_pitch.filter(|&v| Some(v) != current.main_pitch) {
        match l.main_pitch {
            Some(i) => set_int(&mut events[i], u32::from(v as u16)),
            None if v != 0 => {
                // Behind the last of 156, 67, 9 and 11 the file has.
                let at = [l.tempo, l.current_pattern, unread(9), l.swing].into_iter().flatten().max().map_or(0, |i| i + 1);
                settings.push((before_signature(at), Event::new(id::MAIN_PITCH, Value::U16(v as u16))));
            }
            None => {}
        }
    }
    // The bar (events 17, 18: beats per bar, steps per beat), its mode (35) and "play truncated notes" (30): patched
    // where the file has them, else added where FL 26 writes them (above). The model's `None` is (4, 4) and a time
    // signature.
    if project.bar != current.bar {
        let (n, d) = project.bar();
        let (n, d) = (u32::from(signature_byte(n)), u32::from(signature_byte(d)));
        match (l.time_num, l.time_den) {
            (Some(a), Some(b)) => {
                set_int(&mut events[a], n);
                set_int(&mut events[b], d);
            }
            (Some(a), None) => {
                set_int(&mut events[a], n);
                settings.push((a + 1, int_event(id::TIME_DEN, d)));
            }
            (None, Some(b)) => {
                set_int(&mut events[b], d);
                settings.push((b, int_event(id::TIME_NUM, n)));
            }
            (None, None) => {
                // Behind the last of 156, 67, 9, 11 and 80 the file has (so before 35, 23, 44, 30 and 10).
                let at = [l.tempo, l.current_pattern, unread(9), l.swing, l.main_pitch].into_iter().flatten().max().map_or(0, |i| i + 1);
                settings.push((at, int_event(id::TIME_NUM, n)));
                settings.push((at, int_event(id::TIME_DEN, d)));
            }
        }
    }
    if project.bar_as_signature != current.bar_as_signature {
        let v = u32::from(project.bar_is_signature());
        match unread(id::TIME_MODE) {
            Some(i) => set_int(&mut events[i], v),
            None => {
                // Behind the last of 156, 67, 9, 11, 80, 17 and 18 the file has (so before 23, 44, 30 and 10).
                let at = [l.tempo, l.current_pattern, unread(9), l.swing, l.main_pitch, l.time_num, l.time_den]
                    .into_iter()
                    .flatten()
                    .max()
                    .map_or(0, |i| i + 1);
                settings.push((at, int_event(id::TIME_MODE, v)));
            }
        }
    }
    // The panning law (event 23: 0 Circular, 2 Triangular; spec: flp/engine-events.md "Project").
    if let Some(v) = project.panning_law.filter(|&v| Some(v) != current.panning_law) {
        match unread(id::PANNING_LAW) {
            Some(i) => set_int(&mut events[i], v.into()),
            None => {
                // Behind the last of 156, 67, 9, 11, 80, 17, 18 and 35 the file has (so before 44, 30 and 10).
                let at = [l.tempo, l.current_pattern, unread(9), l.swing, l.main_pitch, l.time_num, l.time_den, unread(id::TIME_MODE)]
                    .into_iter()
                    .flatten()
                    .max()
                    .map_or(0, |i| i + 1);
                settings.push((at, int_event(id::PANNING_LAW, v.into())));
            }
        }
    }
    // Automatic crossfades (event 44: 1 on, 0 off; spec: flp/engine-events.md "The project settings, measured").
    if let Some(on) = project.auto_crossfades.filter(|&v| Some(v) != current.auto_crossfades) {
        match unread(id::AUTO_CROSSFADES) {
            Some(i) => set_int(&mut events[i], u32::from(on)),
            None => {
                // Behind the last of 156, 67, 9, 11, 80, 17, 18, 35 and 23 the file has (so before 30 and 10).
                let at = [l.tempo, l.current_pattern, unread(9), l.swing, l.main_pitch, l.time_num, l.time_den, unread(id::TIME_MODE), unread(id::PANNING_LAW)]
                    .into_iter()
                    .flatten()
                    .max()
                    .map_or(0, |i| i + 1);
                settings.push((at, int_event(id::AUTO_CROSSFADES, u32::from(on))));
            }
        }
    }
    // Event 30 is there with 1 when on; off removes it, as FL does (it never writes 0; spec: flp/engine-events.md
    // "Event 30").
    if project.play_truncated != current.play_truncated {
        match l.truncated {
            Some(i) if project.play_truncated => set_int(&mut events[i], 1),
            Some(i) => removals.push(i),
            None if !project.play_truncated => {}
            None => {
                // Behind the last of 156, 67, 9, 11, 80, 17, 18, 35, 23 and 44 the file has (so before 10).
                let at = [l.tempo, l.current_pattern, unread(9), l.swing, l.main_pitch, l.time_num, l.time_den, unread(35), unread(23), unread(44)]
                    .into_iter()
                    .flatten()
                    .max()
                    .map_or(0, |i| i + 1);
                settings.push((at, int_event(id::PLAY_TRUNCATED, 1)));
            }
        }
    }
    // Inserts at one place end up in reverse order of pushing: settings added at one place keep FL's order, ahead of
    // project information added there.
    inserts.extend(settings.into_iter().rev());
    let groups_at = l
        .groups
        .first()
        .copied()
        .or(l.current_group)
        .or(l.links.first().copied())
        .or(l.channels.first().map(|c| c.start))
        .unwrap_or(0);
    // The initial values (event 216; spec: ui/settings-choices.md "Advanced: Initialize controls automatically"): the
    // records written as the model has them, the ones Lemon Studio doesn't understand included, which the model keeps
    // as read. A file without the event gets it once there is a value, after 146 (or the group names) as FL's prelude
    // has it (flp/events/project-info.md); pushed first, so it stays behind a 146 added at the same place.
    if project.init_controls != current.init_controls {
        match l.init_controls {
            Some(i) => {
                let bytes = init_controls_bytes(&project.init_controls, events[i].data().unwrap_or(&[]));
                set_data(&mut events[i], bytes);
            }
            None => {
                let at = l.current_group.or(l.groups.last().copied()).map_or(groups_at, |i| i + 1);
                inserts.push((at, Event::new(id::INIT_CONTROLS, Value::var(init_controls_bytes(&project.init_controls, &[])))));
            }
        }
    }
    if project.groups != current.groups {
        removals.extend(l.groups.iter().copied());
        for name in project.groups.iter().rev() {
            inserts.push((groups_at, Event::new(id::GROUP_NAME, Value::var(encode_text(name, enc)))));
        }
    }
    if project.current_group != current.current_group {
        let v = project.current_group.unwrap_or(u32::MAX);
        match l.current_group {
            Some(i) => set_int(&mut events[i], v),
            None => {
                let at = l.groups.last().map_or(groups_at, |i| i + 1);
                inserts.push((at, Event::new(id::CURRENT_GROUP, Value::U32(v))));
            }
        }
    }

    // Channels.
    for (loc, (ch, old)) in l.channels.iter().zip(project.channels.iter().zip(&current.channels)) {
        // Stretch settings: event 215 ints 24..27 of the channel's block (spec: audio/audio-clips.md "Settings").
        if ch.sound.stretch != old.sound.stretch {
            let mut k = loc.start + 1;
            while k < events.len() && events[k].id != id::CH_START && events[k].id != id::ARR_START {
                if events[k].id == id::CH_SAMPLE_SETTINGS {
                    let bytes: Vec<u8> = ch.sound.stretch.iter().flat_map(|v| v.to_le_bytes()).collect();
                    patch_bytes(&mut events[k], 96, &bytes);
                    break;
                }
                k += 1;
            }
        }
        if ch.kind.is_sampler() {
            write_sampler(events, loc, ch, old, &mut inserts, &mut removals);
        }
        // The Misc tab's values, for every kind of channel.
        let add = write_misc(&mut events[loc.start..=loc.end], ch, &old.misc);
        inserts.extend(add.into_iter().rev().map(|e| (loc.end, e)));
        if ch.name != old.name {
            match (loc.name, &ch.name) {
                (Some(i), Some(n)) => set_data(&mut events[i], encode_text(n, enc)),
                (None, Some(n)) => {
                    // The name goes right after the 52-byte record (event 212).
                    let at = loc.plugin_rec.map_or(loc.start + 3, |i| i + 1);
                    inserts.push((at, Event::new(id::DISPLAY_NAME, Value::var(encode_text(n, enc)))));
                }
                (Some(_), None) => {
                    return Err(ProjectError::Mismatch("removing a channel name".into()));
                }
                (None, None) => {}
            }
        }
        if ch.color != old.color {
            if let (Some(ci), Some(fi)) = (loc.color, loc.color_flag) {
                match ch.color {
                    Some(c) => {
                        set_int(&mut events[ci], bgr_from_rgb(clamp_channel_color(c), 0));
                        set_int(&mut events[fi], 1);
                    }
                    None => set_int(&mut events[fi], 0),
                }
            }
        }
        if ch.enabled != old.enabled {
            if let Some(i) = loc.enabled {
                set_int(&mut events[i], u32::from(ch.enabled));
            }
        }
        if let Some(i) = loc.levels {
            if ch.pan != old.pan {
                let v = ((ch.pan + 1.0) * 6400.0).round() as i32;
                patch_bytes(&mut events[i], 0, &v.to_le_bytes());
            }
            if ch.volume != old.volume {
                let v = (ch.volume * 12800.0).round() as i32;
                patch_bytes(&mut events[i], 4, &v.to_le_bytes());
            }
            if ch.pitch_cents != old.pitch_cents {
                patch_bytes(&mut events[i], 8, &ch.pitch_cents.to_le_bytes());
            }
        }
        if ch.mixer_track != old.mixer_track {
            if let Some(i) = loc.target {
                set_int(&mut events[i], ch.mixer_track.into());
            }
        }
        // Rack state (spec: flp/events/channels.md, events 15 / 32 / 145).
        if ch.rack.locked != old.rack.locked {
            match loc.locked {
                Some(i) => set_int(&mut events[i], u32::from(ch.rack.locked)),
                None => inserts.push((loc.end, Event::new(id::CH_LOCKED, Value::U8(u8::from(ch.rack.locked))))),
            }
        }
        if ch.rack.zipped != old.rack.zipped {
            match (loc.zipped, ch.rack.zipped) {
                (Some(i), true) => set_int(&mut events[i], 1),
                (Some(i), false) => removals.push(i),
                (None, true) => {
                    let at = loc.last_child.or(loc.target).map_or(loc.end, |i| i + 1);
                    inserts.push((at, Event::new(id::CH_ZIPPED, Value::U8(1))));
                }
                (None, false) => {}
            }
        }
        if ch.rack.group != old.rack.group {
            match loc.group {
                Some(i) => set_int(&mut events[i], ch.rack.group),
                None => inserts.push((loc.end, Event::new(id::CH_GROUP, Value::U32(ch.rack.group)))),
            }
        }
        // A sampler's sample (Lemon Sampler loads one): event 196 replaced, or added right after event 20 where FL
        // writes it; a sample taken out takes its event with it (spec: flp/events/channels.md, block layout and "Loop
        // points (event 251)").
        if ch.sample_path != old.sample_path {
            match (&ch.sample_path, loc.sample, loc.ev20) {
                (Some(p), Some(i), _) => set_data(&mut events[i], encode_text(p, enc)),
                (Some(p), None, Some(i)) => inserts.push((i + 1, Event::new(id::SAMPLE_PATH, Value::var(encode_text(p, enc))))),
                (Some(p), None, None) => inserts.push((loc.end, Event::new(id::SAMPLE_PATH, Value::var(encode_text(p, enc))))),
                (None, Some(i), _) => removals.push(i),
                (None, None, _) => {}
            }
        }
        // A plugin channel's plugin (201) and state (213) (spec: flp/events/plugins.md §1-2). A state for a channel
        // that had none goes after the colour flag (41), where FL keeps it.
        if ch.kind == ChannelKind::Plugin && ch.plugin != old.plugin {
            if let (Some(i), Some(n)) = (loc.plugin_name, &ch.plugin) {
                set_data(&mut events[i], encode_text(n, enc));
            }
        }
        if ch.plugin_state != old.plugin_state {
            if let Some(st) = &ch.plugin_state {
                match loc.state {
                    Some(i) => set_data(&mut events[i], st.clone()),
                    None if ch.kind == ChannelKind::Plugin => {
                        let at = loc.color_flag.or(loc.color).or(loc.name).or(loc.plugin_rec).map_or(loc.start + 2, |i| i + 1);
                        inserts.push((at, Event::new(id::PLUGIN_STATE, Value::var(st.clone()))));
                    }
                    None => {}
                }
            }
        }
    }

    // New channels go after the last channel block (spec: flp/events/channels.md; automation.md for type 5).
    if project.channels.len() > current.channels.len() {
        let at = l.channels.last().map(|c| c.end + 1).ok_or_else(|| ProjectError::Structure("no channels".into()))?;
        let mut block = Vec::new();
        for ch in &project.channels[current.channels.len()..] {
            block.extend(channel_block_events(ch, project.automation(ch.index), enc)?);
        }
        inserts.extend(block.into_iter().rev().map(|e| (at, e)));
    }

    // Controller links (spec: flp/events/automation.md "Controller links"): the k-th link event takes the model's k-th
    // link and its formula (event 230 right after it), links beyond the file's go after its last one (before the first
    // channel block when it has none), events beyond the model's go. Unchanged, the events stay as they are.
    if project.controller_links != current.controller_links {
        let formula = |s: &str| Event::new(id::LINK_FORMULA, Value::var(encode_text(s, enc)));
        for (&(i, f), c) in l.ctl_links.iter().zip(&project.controller_links) {
            set_data(&mut events[i], c.bytes.to_vec());
            match (f, &c.formula) {
                (Some(f), Some(s)) => set_data(&mut events[f], encode_text(s, enc)),
                (Some(f), None) => removals.push(f),
                (None, Some(s)) => inserts.push((i + 1, formula(s))),
                (None, None) => {}
            }
        }
        let at = l.ctl_links.last().map_or_else(|| l.channels.first().map_or(0, |c| c.start), |&(i, f)| f.unwrap_or(i) + 1);
        for c in project.controller_links.iter().skip(l.ctl_links.len()).rev() {
            if let Some(s) = &c.formula {
                inserts.push((at, formula(s)));
            }
            inserts.push((at, Event::new(id::AUTO_LINK, Value::var(c.bytes.to_vec()))));
        }
        for &(i, f) in l.ctl_links.iter().skip(project.controller_links.len()) {
            removals.push(i);
            removals.extend(f);
        }
    }

    // Automation data of existing automation channels, and the links of every automation clip.
    for a in &project.automations {
        let old = current.automation(a.channel);
        if let Some(pos) = current.channels.iter().position(|c| c.index == a.channel) {
            if old != Some(a) {
                if let Some(i) = l.channels[pos].auto {
                    set_data(&mut events[i], encode_automation(a));
                }
            }
        }
        // One link per target (spec: flp/events/automation.md "Event 227"): the clip's k-th link takes its k-th
        // target, targets beyond its links get new links after its last one (before the first channel block when it
        // has none), links beyond its targets go. Controller links are not among `l.links` and stay as they are.
        let links: Vec<usize> =
            l.links.iter().copied().filter(|&i| events[i].data().is_some_and(|d| u16::from_le_bytes([d[2], d[3]]) == a.channel)).collect();
        for (&i, t) in links.iter().zip(&a.targets) {
            patch_bytes(&mut events[i], 8, &t.to_le_bytes());
        }
        let at = links.last().map_or_else(|| l.channels.first().map_or(0, |c| c.start), |&i| i + 1);
        inserts.extend(a.targets.iter().skip(links.len()).rev().map(|&t| (at, link_event(a.channel, t))));
        removals.extend(links.iter().skip(a.targets.len()));
    }

    // Patterns: notes, name, color. Patterns without blocks get new ones (spec: flp/events/patterns.md,
    // "Where blocks go"): note blocks in number order before the first channel, metadata blocks in number order
    // after the first channel's block.
    let mut new_note_blocks: Vec<(u16, Vec<Event>)> = Vec::new();
    let mut new_meta_blocks: Vec<(u16, Vec<Event>)> = Vec::new();
    for pat in &project.patterns {
        let old = current.patterns.iter().find(|p| p.number == pat.number);
        let loc = l.patterns.get(&pat.number);
        let has_block = loc.is_some_and(|l| l.start.is_some());
        let new_block = !has_block && (!pat.notes.is_empty() || !pat.controls.is_empty() || !pat.loops.is_empty());
        if new_block {
            new_note_blocks.push((pat.number, note_block_events(pat)));
        }
        if has_block && loc.is_some_and(|l| l.notes.is_none()) && !pat.notes.is_empty() {
            // A control-points-only block gains notes right after its points.
            if let Some(c) = loc.and_then(|l| l.controls) {
                inserts.push((c + 1, Event::new(id::PAT_NOTES, Value::var(encode_notes(&pat.notes)))));
            }
        }
        let notes_changed = !new_block && old.map(|o| &o.notes) != Some(&pat.notes);
        if notes_changed && loc.and_then(|l| l.notes).is_none() && !pat.notes.is_empty() && !has_block {
            return Err(ProjectError::Mismatch(format!("adding notes to pattern {} (no note block)", pat.number)));
        }
        // Without a note event there is nothing to rewrite here (no notes, or they were added with the block above);
        // the pattern's other parts still are.
        if let Some(i) = loc.and_then(|l| l.notes).filter(|_| notes_changed) {
            // Reuse the original bytes of notes that didn't change (keeps unknown fields).
            let original = events[i].data().unwrap_or(&[]).to_vec();
            let mut sorted = pat.notes.clone();
            sorted.sort_by_key(|n| n.position);
            let mut data = Vec::with_capacity(sorted.len() * NOTE_LEN);
            for n in &sorted {
                match original.chunks_exact(NOTE_LEN).find(|r| decode_note(r) == *n) {
                    Some(r) => data.extend_from_slice(r),
                    None => data.extend_from_slice(&encode_note(n)),
                }
            }
            set_data(&mut events[i], data);
        }
        if !new_block && old.map(|o| &o.controls) != Some(&pat.controls) {
            match (loc.and_then(|l| l.controls), pat.controls.is_empty()) {
                (Some(i), false) => set_data(&mut events[i], encode_controls(&pat.controls)),
                (Some(i), true) => removals.push(i),
                (None, true) => {}
                (None, false) => {
                    let Some(start) = loc.and_then(|l| l.start) else {
                        return Err(ProjectError::Mismatch(format!(
                            "adding control points to pattern {} (no note block)",
                            pat.number
                        )));
                    };
                    inserts.push((start + 1, Event::new(id::PAT_CONTROLS, Value::var(encode_controls(&pat.controls)))));
                }
            }
        }
        // Loop lengths follow the notes (spec: flp/events/patterns.md "Loop lengths").
        if !new_block && old.map(|o| &o.loops) != Some(&pat.loops) {
            removals.extend(loc.map(|l| l.loops.clone()).unwrap_or_default());
            if !pat.loops.is_empty() {
                let Some(at) = loc.and_then(|l| l.notes.or(l.controls).or(l.start)).map(|i| i + 1) else {
                    return Err(ProjectError::Mismatch(format!("adding loop lengths to pattern {} (no note block)", pat.number)));
                };
                inserts.extend(loop_events(&pat.loops).into_iter().rev().map(|e| (at, e)));
            }
        }
        let meta_start = loc.and_then(|l| l.meta_start);
        let wants_meta = pat.name.is_some()
            || pat.color.is_some()
            || pat.length.is_some()
            || pat.loop_off.is_some()
            || !pat.markers.is_empty();
        if meta_start.is_some()
            && (old.map(|o| o.length) != Some(pat.length)
                || old.map(|o| o.loop_off) != Some(pat.loop_off)
                || old.map(|o| &o.markers) != Some(&pat.markers))
        {
            // Loop-off flag, markers and length are rewritten together where they were.
            let m = loc.expect("has metadata");
            let old_ones: Vec<usize> = m.loop_off.iter().chain(&m.markers).chain(m.length.iter()).copied().collect();
            let at = old_ones.iter().min().copied().or(m.meta_158.map(|i| i + 1)).unwrap_or(m.meta_start.unwrap_or(0) + 1);
            removals.extend(old_ones);
            inserts.extend(meta_length_events(pat, enc).into_iter().rev().map(|e| (at, e)));
        }
        if meta_start.is_none() && wants_meta {
            // First name or colour of a pattern: its whole metadata block (defaults per the spec).
            new_meta_blocks.push((pat.number, meta_block_events(pat, enc)));
            continue;
        }
        if old.map(|o| &o.name) != Some(&pat.name) {
            match (loc.and_then(|l| l.name), &pat.name) {
                (Some(i), Some(n)) => set_data(&mut events[i], encode_text(n, enc)),
                (_, None) => {}
                (None, Some(n)) => {
                    // The name follows the pattern number.
                    let at = meta_start.map_or(0, |s| s + 1);
                    inserts.push((at, Event::new(id::PAT_NAME, Value::var(encode_text(n, enc)))));
                }
            }
        }
        if old.map(|o| &o.color) != Some(&pat.color) {
            match loc.and_then(|l| l.color) {
                Some(i) => set_int(
                    &mut events[i],
                    pat.color.map_or(PATTERN_COLOR_DEFAULT, |c| bgr_from_rgb(c, 0xFF)),
                ),
                None if pat.color.is_none() => {}
                None => {
                    let at = loc.and_then(|l| l.name).or(meta_start).map_or(0, |s| s + 1);
                    inserts.push((at, Event::new(id::PAT_COLOR, Value::U32(bgr_from_rgb(pat.color.unwrap(), 0xFF)))));
                }
            }
        }
    }
    // Insert new blocks. Inserts at one index apply highest number first, each block's events in reverse,
    // so the stream ends up ascending.
    let first_channel = l.channels.first().ok_or_else(|| ProjectError::Structure("no channels".into()))?;
    let after_first_channel = l.meta_end.unwrap_or(first_channel.end + 1);
    let above = |number: u16, pick: fn(&PatternLoc) -> Option<usize>| {
        l.patterns.iter().filter(|(n, _)| **n > number).filter_map(|(n, p)| pick(p).map(|i| (*n, i))).min().map(|(_, i)| i)
    };
    new_note_blocks.sort_by_key(|&(n, _)| std::cmp::Reverse(n));
    new_meta_blocks.sort_by_key(|&(n, _)| std::cmp::Reverse(n));
    for (n, block) in new_note_blocks {
        let at = above(n, |p| p.start).unwrap_or(first_channel.start);
        inserts.extend(block.into_iter().rev().map(|e| (at, e)));
    }
    for (n, block) in new_meta_blocks {
        let at = above(n, |p| p.meta_start).unwrap_or(after_first_channel);
        inserts.extend(block.into_iter().rev().map(|e| (at, e)));
    }

    // Arrangements, matched by number: removed ones lose their whole block, added ones are written after the last
    // (spec: flp/events/playlist.md "Arrangement block"); kept ones are patched below.
    let mut arr_end: Option<usize> = None;
    for (old_arr, loc) in current.arrangements.iter().zip(&l.arrangements) {
        let end = loc
            .tracks
            .iter()
            .flat_map(|t| [Some(t.record), t.color_flag, t.name])
            .flatten()
            .chain(loc.clips)
            .chain(loc.markers.iter().copied())
            .chain(loc.name)
            .max()
            .unwrap_or(loc.start)
            + 1;
        arr_end = Some(arr_end.map_or(end, |e| e.max(end)));
        if !project.arrangements.iter().any(|a| a.number == old_arr.number) {
            removals.extend(loc.start..end);
        }
    }
    if let (None, Some(n), Some(at)) = (l.current_arrangement, project.current_arrangement, arr_end) {
        // A file without event 100 (an older Lemon Studio save) gets it after its last arrangement.
        inserts.push((at, Event::new(id::CURRENT_ARRANGEMENT, Value::U16(n))));
    }
    // The playlist view (event 242) sits right before event 100, as FL writes it.
    if let Some(v) = project.playlist_view.filter(|&v| Some(v) != current.playlist_view) {
        match (l.playlist_view, l.current_arrangement.or(arr_end)) {
            (Some(i), _) => set_data(&mut events[i], v.raw.to_vec()),
            (None, Some(at)) => inserts.push((at, Event::new(id::PLAYLIST_VIEW, Value::var(v.raw.to_vec())))),
            (None, None) => {}
        }
    }
    if let Some(at) = arr_end.filter(|_| !skip_arrangements) {
        let added: Vec<Event> = project
            .arrangements
            .iter()
            .filter(|a| current.arrangements.iter().all(|o| o.number != a.number))
            .flat_map(|a| arrangement_events(a, enc, clen))
            .collect();
        inserts.extend(added.into_iter().rev().map(|e| (at, e)));
    }

    // Playlist clips, time markers and tracks.
    for (old_arr, loc) in current.arrangements.iter().zip(&l.arrangements) {
        let Some(arr) = project.arrangements.iter().find(|a| a.number == old_arr.number) else { continue };
        if arr.name != old_arr.name {
            if let Some(i) = loc.name {
                set_data(&mut events[i], encode_text(arr.name.as_deref().unwrap_or("Arrangement"), enc));
            }
        }
        if arr.clips != old_arr.clips {
            if let Some(i) = loc.clips {
                set_data(&mut events[i], encode_clips(&arr.clips, clen));
            }
        }
        if arr.markers != old_arr.markers {
            let Some(ci) = loc.clips else {
                return Err(ProjectError::Structure("arrangement without a clip list".into()));
            };
            removals.extend(loc.markers.iter().copied());
            let block: Vec<Event> = arr.markers.iter().flat_map(|m| marker_events(m, enc)).collect();
            inserts.extend(block.into_iter().rev().map(|e| (ci + 1, e)));
        }
        for ((t, old), tl) in arr.tracks.iter().zip(&old_arr.tracks).zip(&loc.tracks) {
            if t.enabled != old.enabled {
                patch_bytes(&mut events[tl.record], 12, &[u8::from(t.enabled)]);
            }
            // Groups (spec: flp/events/playlist.md, bytes 46 / 47 / 61).
            if t.grouped != old.grouped {
                patch_bytes(&mut events[tl.record], 46, &[u8::from(t.grouped)]);
            }
            if t.locked != old.locked {
                patch_bytes(&mut events[tl.record], 47, &[u8::from(t.locked)]);
            }
            if t.collapsed != old.collapsed {
                patch_bytes(&mut events[tl.record], 61, &[u8::from(!t.collapsed)]);
            }
            if t.color != old.color {
                if let Some(c) = t.color {
                    patch_bytes(&mut events[tl.record], 4, &bgr_from_rgb(c, 0).to_le_bytes());
                }
                if let Some(fi) = tl.color_flag {
                    set_int(&mut events[fi], u32::from(t.color.is_some()));
                }
            }
            if t.name != old.name {
                match (tl.name, &t.name) {
                    (Some(i), Some(n)) => set_data(&mut events[i], encode_text(n, enc)),
                    (None, Some(n)) => {
                        let at = tl.color_flag.map_or(tl.record + 1, |i| i + 1);
                        inserts.push((at, Event::new(id::PL_NAME, Value::var(encode_text(n, enc)))));
                    }
                    (Some(i), None) => removals.push(i),
                    (None, None) => {}
                }
            }
        }
    }

    // Mixer tracks.
    let mut params = l.mixer_params.and_then(|i| events[i].data()).map(<[u8]>::to_vec);
    let slot_states = if skip_mixer { Vec::new() } else { slot_state_events(events, &l) };
    // The file track each block now stands for: inserts are added or removed just before the last track
    // (`grow_mixer`, `shrink_mixer`), so a block keeps its position, the last one was the file's last, and an added one
    // has none.
    let n = project.mixer.len();
    let file_track = |t: usize| -> Option<u16> {
        let from = if t + 1 == n { file_tracks.checked_sub(1) } else { (t + 1 < file_tracks).then_some(t) };
        from.map(|f| f as u16)
    };
    // Tracks whose slots gained, lost or swapped plugins get their whole slot area rewritten (spec:
    // flp/events/plugins.md §2-3), and so do tracks holding plugins that came from another block (moved, deleted or
    // trimmed tracks: their slots follow them, spec: flp/events/mixer.md "Removing tracks"). A plugin that stays (or
    // moves) keeps its own events.
    let slot_names = |t: &MixerTrack| t.slots.iter().map(|s| s.as_ref().map(|s| s.name.clone())).collect::<Vec<_>>();
    let rewrite: Vec<bool> = project
        .mixer
        .iter()
        .zip(&current.mixer)
        .enumerate()
        .map(|(t, (m, old))| slot_names(m) != slot_names(old) || (m.slots.iter().any(Option::is_some) && m.origin != file_track(t)))
        .collect();
    if !skip_mixer && rewrite.iter().any(|&r| r) {
        let areas = slot_areas(events, &l);
        if let Some(t) = (0..n).find(|&t| rewrite[t] && areas.get(t).is_none_or(Option::is_none)) {
            return Err(ProjectError::Structure(format!("mixer track {t} has no slot area")));
        }
        // The file's plugins other than those of the blocks kept as they are.
        let kept: Vec<usize> = (0..n).filter(|&t| !rewrite[t]).filter_map(file_track).map(usize::from).collect();
        let mut pool: Vec<SlotRun> = file_slots.into_iter().filter(|r| !kept.contains(&r.track)).collect();
        let wanted: Vec<(usize, Option<usize>, &SlotPlugin)> = project
            .mixer
            .iter()
            .enumerate()
            .filter(|&(t, _)| rewrite[t])
            .flat_map(|(_, m)| m.slots.iter().enumerate().filter_map(move |(k, s)| Some((k, m.origin.map(usize::from), s.as_ref()?))))
            .collect();
        let mut runs = take_runs(&mut pool, &wanted).into_iter();
        for (t, m) in project.mixer.iter().enumerate().filter(|&(t, _)| rewrite[t]) {
            let area = areas[t].as_ref().expect("checked above");
            let mut block = Vec::new();
            for (k, slot) in m.slots.iter().enumerate() {
                if let Some(p) = slot {
                    let rt = record_track(t as u16, n);
                    block.extend(slot_plugin_events(p, rt, k as u8, runs.next().flatten(), enc));
                }
                block.push(Event::new(id::MX_SLOT, Value::U16(k as u16)));
            }
            removals.extend(area.from..area.to);
            inserts.extend(block.into_iter().rev().map(|e| (area.to, e)));
        }
    }
    for ((((m, old), ml), states), &rewritten) in project
        .mixer
        .iter()
        .zip(&current.mixer)
        .zip(&l.mixer)
        .zip(slot_states.iter().chain(std::iter::repeat(&[None; 10])))
        .zip(rewrite.iter())
    {
        // spec: flp/events/mixer.md "Slot sub-block": a plugin kept in its slot gets its new state (213).
        for ((new, prev), &at) in m.slots.iter().zip(&old.slots).zip(states).filter(|_| !rewritten) {
            if let (Some(new), Some(prev), Some(i)) = (new, prev, at) {
                if new.name == prev.name && new.state != prev.state {
                    set_data(&mut events[i], new.state.clone());
                }
            }
        }
        // Slot LED and mix level (spec: flp/events/plugins.md §5): kind 0x1F records ID 0 / 1 at the slot's location.
        if let Some(data) = params.as_mut() {
            for k in 0..10u16 {
                let loc = mixer_location(record_track(m.index, project.mixer.len()), k);
                let s = usize::from(k);
                if m.slot_on[s] != old.slot_on[s] {
                    set_param_record(data, loc, MX_SLOT_ON, i32::from(m.slot_on[s]));
                }
                if m.slot_mix[s] != old.slot_mix[s] {
                    set_param_record(data, loc, MX_SLOT_MIX, (m.slot_mix[s].clamp(0.0, 1.0) * SLOT_MIX_FULL).round() as i32);
                }
            }
        }
        let latency = (m.latency_out, m.latency_in);
        let relatency = latency != (old.latency_out, old.latency_in);
        if relatency || (m.enabled, m.polarity, m.swap, m.slots_enabled) != (old.enabled, old.polarity, old.swap, old.slots_enabled) {
            if let Some(i) = ml.flags {
                match track_flags(events[i].data()) {
                    Some(b) if !relatency => patch_bytes(&mut events[i], 4, &[with_flags(b, m)]),
                    Some(_) => {
                        let mut r = with_latency(events[i].data().unwrap_or(&[]), old_file, latency);
                        r[4] = with_flags(r[4], m);
                        set_data(&mut events[i], r);
                    }
                    // A 5-byte record (FL 10 / 11) has no flags: it becomes the 8-byte record of FL 12, its latency kept in
                    // the form the file's version reads (spec: flp/layouts.md "A mixer track's record (event 236)").
                    None => {
                        let d = events[i].data().unwrap_or(&[]);
                        let late = old_file && d.get(4).is_some_and(|&b| b != 0);
                        let base = if m.index == 0 { MX_FLAGS_MASTER } else { MX_FLAGS_INSERT };
                        let mut r = vec![0u8; 8];
                        r[4] = base | if late { MX_FLAG_LATE_OUT } else { 0 };
                        let mut r = with_latency(&r, old_file, latency);
                        r[4] = with_flags(r[4], m);
                        set_data(&mut events[i], r);
                    }
                }
            }
        }
        if m.name != old.name {
            match (ml.name, &m.name) {
                (Some(i), Some(n)) => set_data(&mut events[i], encode_text(n, enc)),
                (None, Some(n)) => {
                    let at = ml.flags.ok_or_else(|| ProjectError::Structure("mixer track without flags".into()))?;
                    inserts.push((at, Event::new(id::MX_NAME, Value::var(encode_text(n, enc)))));
                }
                // Back to the default name ("Insert N"): the name event goes.
                (Some(i), None) => removals.push(i),
                (None, None) => {}
            }
        }
        // The icon (event 95) stands right after the colour flag (spec: flp/events/mixer.md "Track blocks"). This comes
        // after the name: inserted at the same place as a new name (a header of 42 and 236 alone), it lands before it.
        if m.icon != old.icon {
            match (ml.icon, m.icon) {
                (Some(i), Some(v)) => set_int(&mut events[i], v.into()),
                (None, Some(v)) => {
                    let at = ml.color_flag.ok_or_else(|| ProjectError::Structure("mixer track without color flag".into()))?;
                    inserts.push((at + 1, Event::new(id::MX_ICON, Value::U16(v))));
                }
                (Some(i), None) => removals.push(i),
                (None, None) => {}
            }
        }
        if m.color != old.color {
            match (ml.color, m.color) {
                (Some(i), Some(c)) => set_int(&mut events[i], bgr_from_rgb(c, 0xFF)),
                (None, Some(c)) => {
                    let at = ml.color_flag.ok_or_else(|| ProjectError::Structure("mixer track without color flag".into()))?;
                    inserts.push((at, Event::new(id::MX_COLOR, Value::U32(bgr_from_rgb(c, 0xFF)))));
                }
                _ => {}
            }
            if let Some(fi) = ml.color_flag {
                set_int(&mut events[fi], u32::from(m.color.is_some()));
            }
        }
        // spec: flp/events/mixer.md "Routing (event 235)": one byte per destination from track 0, trailing zeros omitted.
        if m.routes != old.routes {
            let mut bytes = vec![0u8; m.routes.iter().map(|&r| usize::from(r) + 1).max().unwrap_or(0)];
            for &r in &m.routes {
                bytes[usize::from(r)] = 1;
            }
            match ml.routes {
                Some(i) => set_data(&mut events[i], bytes),
                None => return Err(ProjectError::Structure(format!("mixer track {} has no routing event", m.index))),
            }
        }
        // Audio input / output, record source, monitor mode (spec: audio/track-io.md §1.6). A file without them gets
        // the ones that changed after the routes, in the order FL writes them (each insert lands before the earlier
        // ones at the same place).
        let io = [
            (ml.output, id::MX_OUTPUT, m.audio_out as u32, old.audio_out as u32),
            (ml.input, id::MX_INPUT, m.audio_in as u32, old.audio_in as u32),
            (ml.monitor, id::MX_MONITOR, m.monitor.into(), old.monitor.into()),
            (ml.rec_source, id::MX_REC_SOURCE, m.rec_source.into(), old.rec_source.into()),
        ];
        for (at, eid, new, _) in io.into_iter().filter(|&(_, _, new, was)| new != was) {
            match (at, ml.routes) {
                (Some(i), _) => set_int(&mut events[i], new),
                (None, Some(r)) => inserts.push((r + 1, Event::new(eid, Value::U32(new)))),
                (None, None) => {}
            }
        }
        // spec: flp/events/mixer.md "Kind 0x20, route level": records right after the source track's own records.
        if m.send_levels != old.send_levels {
            if let Some(data) = params.as_mut() {
                let loc = mixer_location(record_track(m.index, project.mixer.len()), 0);
                let mut recs: Vec<[u8; MIXER_REC_LEN]> = data.chunks_exact(MIXER_REC_LEN).map(|r| r.try_into().unwrap()).collect();
                // Files with sends in the old form (kind 0x1F, id 64 + destination) keep that form.
                let old_form = recs.iter().any(|r| r[5] == MX_KIND_PARAM && MX_SEND_OLD.contains(&r[4]));
                recs.retain(|r| !(is_send(r[5], r[4]) && u16::from_le_bytes([r[6], r[7]]) == loc));
                let at = recs.iter().rposition(|r| u16::from_le_bytes([r[6], r[7]]) == loc).map_or_else(
                    || recs.iter().position(|r| u16::from_le_bytes([r[6], r[7]]) > loc).unwrap_or(recs.len()),
                    |p| p + 1,
                );
                let mut levels = m.send_levels.clone();
                levels.sort_by_key(|&(d, _)| d);
                for (k, &(dest, level)) in levels.iter().enumerate() {
                    let mut r = [0u8; MIXER_REC_LEN];
                    (r[4], r[5]) = if old_form { (64 + dest.min(126) as u8, MX_KIND_PARAM) } else { (dest.min(255) as u8, MX_KIND_ROUTE) };
                    r[6..8].copy_from_slice(&loc.to_le_bytes());
                    r[8..12].copy_from_slice(&((level * 16000.0).round() as i32).to_le_bytes());
                    recs.insert(at + k, r);
                }
                *data = recs.concat();
            }
        }
        if let Some(data) = params.as_mut() {
            for (pid, new, prev, scale) in [
                (MX_VOLUME, m.volume, old.volume, 16000.0),
                (MX_PAN, m.pan, old.pan, 6400.0),
                (MX_STEREO, m.stereo_separation, old.stereo_separation, 64.0),
            ] {
                if new == prev {
                    continue;
                }
                let Some(v) = new else { continue };
                let loc = mixer_location(record_track(m.index, project.mixer.len()), 0);
                let Some((off, ..)) = mixer_param_records(data)
                    .find(|&(_, p, k, at, _)| k == MX_KIND_PARAM && p == pid && at == loc)
                else {
                    return Err(ProjectError::Mismatch(format!("mixer track {} has no record for param {pid:#x}", m.index)));
                };
                let v = (v * scale).round() as i32;
                data[off + 8..off + 12].copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    if let (Some(v), Some(data)) = (project.main_volume.filter(|&v| Some(v) != current.main_volume), params.as_mut()) {
        set_record(data, 0, MX_SPECIAL_LOC, 0, i32::from(v));
    }
    if let (Some(i), Some(data)) = (l.mixer_params, params) {
        set_data(&mut events[i], data);
    }

    // Removals first; insert positions refer to the original stream, so shift them down.
    removals.sort_unstable();
    for (at, _) in inserts.iter_mut() {
        *at -= removals.iter().filter(|&&r| r < *at).count();
    }
    for &r in removals.iter().rev() {
        events.remove(r);
    }
    inserts.sort_by_key(|&(at, _)| std::cmp::Reverse(at));
    for (at, e) in inserts {
        events.insert(at, e);
    }
    // The header counts the channels (container.md).
    if project.channels.len() != current.channels.len() {
        for c in &mut file.chunks {
            if let ChunkBody::Header(h) = &mut c.body {
                if h.raw.len() >= 4 {
                    h.raw[2..4].copy_from_slice(&(project.channels.len() as u16).to_le_bytes());
                }
            }
        }
    }
    // And holds the timebase (Settings > Project ▸ Timebase; spec: flp/engine-events.md "The project settings,
    // measured"): the tick counts written above are the model's, already at this PPQ.
    if project.ppq != current.ppq {
        for c in &mut file.chunks {
            if let ChunkBody::Header(h) = &mut c.body {
                if h.raw.len() >= 6 {
                    h.raw[4..6].copy_from_slice(&project.ppq.max(1).to_le_bytes());
                }
            }
        }
    }
    Ok(())
}

/// Adds default insert tracks before the last ("current") track when the model has more tracks than the file (spec:
/// flp/events/mixer.md "Adding tracks"): their blocks go before the last track's block, their parameter records before
/// the current track's (track 501), and the count (event 103) follows. [`apply`] then patches the values as usual.
fn grow_mixer(project: &Project, file: &mut FlpFile) -> Result<(), ProjectError> {
    let events = events_of_mut(file)?;
    let l = scan(events)?;
    let enc = text_enc(events, l.version);
    let (old, new) = (l.mixer.len(), project.mixer.len());
    if old < 2 || new <= old {
        return Ok(());
    }
    let missing = |what: &str| ProjectError::Structure(format!("mixer without {what}"));
    let count = events.iter().position(|e| e.id == id::MX_COUNT).ok_or_else(|| missing("a track count"))?;
    let last = &l.mixer[old - 1];
    let at = last.color.or(last.color_flag).or(last.name).or(last.flags).ok_or_else(|| missing("a last track"))?;
    let params_at = l.mixer_params.ok_or_else(|| missing("parameter records"))?;
    if at <= count || params_at <= at {
        return Err(ProjectError::Structure("mixer events out of order".into()));
    }
    let first = (old - 1) as u16;
    let mut data = events[params_at].data().unwrap_or(&[]).to_vec();
    // Records are in location order and none sit between the last insert and track 501.
    let pos = mixer_param_records(&data)
        .find(|&(_, _, _, loc, _)| loc >= mixer_location(first, 0))
        .map_or(data.len() - data.len() % MIXER_REC_LEN, |r| r.0);
    let mut recs = Vec::new();
    let mut blocks = Vec::new();
    for t in first..(new - 1) as u16 {
        recs.extend(track_records(t, None));
        blocks.extend(track_block_events(&MixerTrack::new(t), false, enc));
    }
    data.splice(pos..pos, recs);
    set_data(&mut events[params_at], data);
    set_int(&mut events[count], new as u32);
    events.splice(at..at, blocks);
    Ok(())
}

/// Removes insert tracks when the model has fewer tracks than the file (spec: flp/events/mixer.md "Removing tracks"):
/// the inserts just before the last ("current") track lose their blocks and their parameter records, and the count
/// (event 103) follows. The model's tracks are already renumbered; [`apply`] then patches the kept ones as usual.
fn shrink_mixer(project: &Project, file: &mut FlpFile) -> Result<(), ProjectError> {
    let events = events_of_mut(file)?;
    let l = scan(events)?;
    let (old, new) = (l.mixer.len(), project.mixer.len());
    if new < 2 || new >= old {
        return Ok(());
    }
    let missing = |what: &str| ProjectError::Structure(format!("mixer without {what}"));
    let count = events.iter().position(|e| e.id == id::MX_COUNT).ok_or_else(|| missing("a track count"))?;
    let start = |m: &MixerLoc| m.color.into_iter().chain(m.color_flag).min();
    // Tracks new-1 .. old-2 go: from the first one's header to the last track's header.
    let from = start(&l.mixer[new - 1]).ok_or_else(|| missing("a track header"))?;
    let to = start(&l.mixer[old - 1]).ok_or_else(|| missing("a last track"))?;
    let params_at = l.mixer_params.ok_or_else(|| missing("parameter records"))?;
    if from <= count || to <= from || params_at <= to {
        return Err(ProjectError::Structure("mixer events out of order".into()));
    }
    let (lo, hi) = (mixer_location((new - 1) as u16, 0), mixer_location((old - 1) as u16, 0));
    let data = events[params_at].data().unwrap_or(&[]).to_vec();
    let kept: Vec<u8> = data
        .chunks(MIXER_REC_LEN)
        .filter(|r| r.len() < MIXER_REC_LEN || !(lo..hi).contains(&u16::from_le_bytes([r[6], r[7]])))
        .flatten()
        .copied()
        .collect();
    set_data(&mut events[params_at], kept);
    events.drain(from..to);
    set_int(&mut events[count], new as u32);
    Ok(())
}

/// Rebuilds the channel region when the model's channels are not the file's blocks in order (deleted, cloned, moved
/// or inserted channels): kept blocks are copied (a clone copies its source block), new ones are built, indices are
/// renumbered, and every reference to a channel follows it or is dropped with it (spec: flp/events/channels.md
/// "Deleting, cloning and moving channels"). [`apply`] then patches the values as usual.
fn restructure(project: &Project, file: &mut FlpFile) -> Result<(), ProjectError> {
    let events = events_of_mut(file)?;
    let l = scan(events)?;
    let enc = text_enc(events, l.version);
    let clen = clip_len(events, l.version);
    let n_old = l.channels.len();
    let origin: Vec<Option<usize>> =
        project.channels.iter().map(|c| c.rack.origin.map(usize::from).filter(|&o| o < n_old)).collect();
    let identity = origin.len() >= n_old
        && origin.iter().enumerate().all(|(i, o)| if i < n_old { *o == Some(i) } else { o.is_none() });
    if identity {
        return Ok(());
    }
    // Old index -> new index; the first channel with an origin is that block's own (later ones are clones).
    let mut new_of: Vec<Option<u16>> = vec![None; n_old];
    for (i, o) in origin.iter().enumerate() {
        if let Some(o) = *o {
            new_of[o].get_or_insert(i as u16);
        }
    }
    let map = |ch: u16| new_of.get(usize::from(ch)).copied().flatten();
    let (first, last) = match (l.channels.first(), l.channels.last()) {
        (Some(f), Some(t)) => (f.start, t.end + 1),
        _ => return Err(ProjectError::Structure("no channels".into())),
    };
    // What sits between blocks (pattern metadata after channel 0) stays after the block at the same position.
    let gaps: Vec<Vec<Event>> = (0..n_old)
        .map(|k| events[l.channels[k].end + 1..l.channels.get(k + 1).map_or(last, |c| c.start)].to_vec())
        .collect();
    let mut region = Vec::new();
    for (i, ch) in project.channels.iter().enumerate() {
        match origin[i] {
            Some(o) => {
                let mut b = events[l.channels[o].start..=l.channels[o].end].to_vec();
                set_int(&mut b[0], i as u32);
                b.retain_mut(|e| {
                    if e.id != id::CH_LAYER_CHILD {
                        return true;
                    }
                    let Some(n) = map(int(e) as u16) else { return false };
                    set_int(e, n.into());
                    true
                });
                region.extend(b);
            }
            None => region.extend(channel_block_events(ch, project.automation(ch.index), enc)?),
        }
        if let Some(g) = gaps.get(i) {
            region.extend(g.iter().cloned());
        }
    }
    for g in gaps.iter().skip(project.channels.len()) {
        region.extend(g.iter().cloned());
    }
    events.splice(first..last, region);

    // References outside the channel blocks (none of these events occur inside a block).
    let mut i = 0;
    while i < events.len() {
        match events[i].id {
            id::PAT_NOTES => {
                let d = events[i].data().unwrap_or(&[]).to_vec();
                let mut out = Vec::with_capacity(d.len());
                for r in d.chunks_exact(NOTE_LEN) {
                    if let Some(n) = map(u16::from_le_bytes([r[6], r[7]])) {
                        out.extend_from_slice(&r[..6]);
                        out.extend_from_slice(&n.to_le_bytes());
                        out.extend_from_slice(&r[8..]);
                    }
                }
                set_data(&mut events[i], out);
                // The loop pairs after the notes (kept in file order).
                let (s, mut e) = (i + 1, i + 1);
                while events.get(e).is_some_and(|x| x.id == id::LOOP_CHANNEL || x.id == id::LOOP_STEPS) {
                    e += 1;
                }
                let pairs: Vec<(u16, u32)> = events[s..e]
                    .chunks_exact(2)
                    .filter_map(|p| map(int(&p[0]) as u16).map(|c| (c, int(&p[1]))))
                    .collect();
                let n = pairs.len() * 2;
                events.splice(s..e, loop_events(&pairs));
                i = s + n;
                continue;
            }
            id::PAT_CONTROLS => {
                let d = events[i].data().unwrap_or(&[]).to_vec();
                let mut out = Vec::with_capacity(d.len());
                for r in d.chunks_exact(CONTROL_LEN) {
                    // Only channel parameters (0 volume, 1 pan, 4 pitch) hold a channel; other targets stay as read.
                    let ch = u16::from_le_bytes([r[6], r[7]]);
                    let n = if matches!(u16::from_le_bytes([r[4], r[5]]), 0 | 1 | 4) { map(ch) } else { Some(ch) };
                    if let Some(n) = n {
                        out.extend_from_slice(&r[..6]);
                        out.extend_from_slice(&n.to_le_bytes());
                        out.extend_from_slice(&r[8..]);
                    }
                }
                set_data(&mut events[i], out);
            }
            id::AUTO_LINK if events[i].data().is_some_and(|d| d.len() >= 20 && d[0] == 0 && d[1] == 0) => {
                let d = events[i].data().unwrap_or(&[]);
                let clip = map(u16::from_le_bytes([d[2], d[3]]));
                let target = u32::from_le_bytes(d[8..12].try_into().unwrap());
                // Channel targets are `channel << 16 | param` below 0x4000_0000 (spec: flp/events/automation.md).
                let new_target = if target >> 16 < 0x4000 {
                    map((target >> 16) as u16).map(|c| u32::from(c) << 16 | target & 0xFFFF)
                } else {
                    Some(target)
                };
                match (clip, new_target) {
                    (Some(c), Some(t)) => {
                        patch_bytes(&mut events[i], 2, &c.to_le_bytes());
                        patch_bytes(&mut events[i], 8, &t.to_le_bytes());
                    }
                    _ => {
                        events.remove(i);
                        continue;
                    }
                }
            }
            id::ARR_CLIPS => {
                let d = events[i].data().unwrap_or(&[]).to_vec();
                let mut out = Vec::with_capacity(d.len());
                for r in d.chunks_exact(clen) {
                    let item = u16::from_le_bytes([r[6], r[7]]);
                    if item >= 0x5000 {
                        out.extend_from_slice(r);
                    } else if let Some(n) = map(item) {
                        out.extend_from_slice(&r[..6]);
                        out.extend_from_slice(&n.to_le_bytes());
                        out.extend_from_slice(&r[8..]);
                    }
                }
                set_data(&mut events[i], out);
            }
            _ => {}
        }
        i += 1;
    }
    // The header counts the channels (container.md).
    for c in &mut file.chunks {
        if let ChunkBody::Header(h) = &mut c.body {
            if h.raw.len() >= 4 {
                h.raw[2..4].copy_from_slice(&(project.channels.len() as u16).to_le_bytes());
            }
        }
    }
    Ok(())
}

/// Notes as 24-byte records, sorted by position.
fn encode_notes(notes: &[Note]) -> Vec<u8> {
    let mut sorted = notes.to_vec();
    sorted.sort_by_key(|n| n.position);
    sorted.iter().flat_map(encode_note).collect()
}

/// A pattern's note block: `65 [223] [224]` (spec: flp/events/patterns.md).
fn note_block_events(pat: &Pattern) -> Vec<Event> {
    let mut block = vec![Event::new(id::PAT_START, Value::U16(pat.number))];
    if !pat.controls.is_empty() {
        block.push(Event::new(id::PAT_CONTROLS, Value::var(encode_controls(&pat.controls))));
    }
    // Loop lengths follow the notes; FL keeps none for a pattern without notes (spec: patterns.md "Loop lengths").
    if !pat.notes.is_empty() {
        block.push(Event::new(id::PAT_NOTES, Value::var(encode_notes(&pat.notes))));
    }
    block.extend(loop_events(&pat.loops));
    block
}

/// Loop length pairs `160 channel, 161 steps` (spec: flp/events/patterns.md "Loop lengths").
fn loop_events(loops: &[(u16, u32)]) -> Vec<Event> {
    loops
        .iter()
        .flat_map(|&(ch, steps)| [Event::new(id::LOOP_CHANNEL, Value::U32(ch.into())), Event::new(id::LOOP_STEPS, Value::U32(steps))])
        .collect()
}

/// The metadata events between 158 and 52: loop-off flag + loop type, the marker groups (the pattern's markers and
/// the "Length" marker, sorted by position as FL saves them), the length (spec: flp/events/patterns.md, metadata block).
fn meta_length_events(pat: &Pattern, enc: TextEnc) -> Vec<Event> {
    let mut out = Vec::new();
    if let Some(t) = pat.loop_off {
        out.push(Event::new(id::LOOP_OFF, Value::U8(1)));
        out.push(Event::new(id::LOOP_TYPE, Value::U32(t)));
    }
    let mut markers: Vec<TimeMarker> = pat.markers.clone();
    if let Some(len) = pat.length {
        markers.push(TimeMarker { kind: LENGTH_MARKER_KIND as u8, ..TimeMarker::new(len, LENGTH_MARKER_NAME) });
    }
    markers.sort_by_key(|m| m.position);
    for m in &markers {
        out.extend(marker_events(m, enc));
    }
    out.push(Event::new(id::PAT_LENGTH, Value::U32(pat.length.unwrap_or(0))));
    out
}

/// A pattern's metadata block with the spec's defaults for the fields Lemon Studio doesn't edit.
fn meta_block_events(pat: &Pattern, enc: TextEnc) -> Vec<Event> {
    let mut block = vec![Event::new(id::PAT_START, Value::U16(pat.number))];
    if let Some(n) = &pat.name {
        block.push(Event::new(id::PAT_NAME, Value::var(encode_text(n, enc))));
    }
    let color = pat.color.map_or(PATTERN_COLOR_DEFAULT, |c| bgr_from_rgb(c, 0xFF));
    block.push(Event::new(id::PAT_COLOR, Value::U32(color)));
    block.extend([
        Event::new(157, Value::U32(u32::MAX)),
        Event::new(158, Value::U32(u32::MAX)),
    ]);
    block.extend(meta_length_events(pat, enc));
    block.extend([
        Event::new(52, Value::U8(0)),
        Event::new(171, Value::U32(0)),
        Event::new(252, Value::var(vec![0, 0])),
        Event::new(253, Value::var(encode_text("0,0,0,0,0,0,0,0,0,0,0,0", enc))),
    ]);
    block
}

/// Version Lemon Studio writes as "saved by" (spec: flp/events/project-info.md; FL 26 opens it without a warning).
const COMPAT_VERSION: &str = "26.1.7.5653";
const COMPAT_BUILD: u32 = 5653;
/// A channel's color when none is set.
const CHANNEL_COLOR_DEFAULT: u32 = 0x0034_3330;

fn channel_type_code(kind: ChannelKind) -> u8 {
    match kind {
        ChannelKind::Sampler => 0,
        ChannelKind::Plugin => 2,
        ChannelKind::Layer => 3,
        ChannelKind::AudioClip => 4,
        ChannelKind::Automation => 5,
        ChannelKind::Other(v) => v,
    }
}

/// One channel block in the minimal form FL 26 loads (spec: flp/events/channels.md; every record not written here
/// takes FL's default).
fn channel_block_events(ch: &Channel, auto: Option<&Automation>, enc: TextEnc) -> Result<Vec<Event>, ProjectError> {
    let plugin = (ch.kind == ChannelKind::Plugin).then_some(ch.plugin.as_deref()).flatten().filter(|p| !p.is_empty());
    if matches!(ch.kind, ChannelKind::Other(_)) || (ch.kind == ChannelKind::Plugin && plugin.is_none()) {
        return Err(ProjectError::Mismatch(format!(
            "channel {}: creating unknown channels or plugin channels without a plugin is not supported",
            ch.index
        )));
    }
    if ch.kind == ChannelKind::Automation && auto.is_none() {
        return Err(ProjectError::Mismatch(format!("automation channel {} has no automation data", ch.index)));
    }
    let mut b = vec![
        Event::new(id::CH_START, Value::U16(ch.index)),
        Event::new(id::CH_TYPE, Value::U8(channel_type_code(ch.kind))),
        Event::new(id::PLUGIN_NAME, Value::var(encode_text(plugin.unwrap_or(""), enc))),
    ];
    // A plugin channel: its window record (spec: flp/events/plugins.md §3) and icon, then the state after the colour.
    if plugin.is_some() {
        b.push(Event::new(id::PLUGIN_REC, Value::var(plugin_record(None))));
    }
    if let Some(n) = &ch.name {
        b.push(Event::new(id::DISPLAY_NAME, Value::var(encode_text(n, enc))));
    }
    if plugin.is_some() {
        b.push(Event::new(id::PLUGIN_ICON, Value::U32(0)));
    }
    let color = ch.color.map_or(CHANNEL_COLOR_DEFAULT, |c| bgr_from_rgb(clamp_channel_color(c), 0));
    b.push(Event::new(id::COLOR, Value::U32(color)));
    b.push(Event::new(id::CH_COLOR_FLAG, Value::U8(u8::from(ch.color.is_some()))));
    if let Some(st) = ch.plugin_state.as_ref().filter(|_| plugin.is_some()) {
        b.push(Event::new(id::PLUGIN_STATE, Value::var(st.clone())));
    }
    b.push(Event::new(id::CH_ENABLED, Value::U8(u8::from(ch.enabled))));
    b.push(Event::new(id::CH_TARGET, Value::U16(ch.mixer_track)));
    for child in &ch.layer_children {
        b.push(Event::new(id::CH_LAYER_CHILD, Value::U16(*child)));
    }
    if ch.rack.zipped {
        b.push(Event::new(id::CH_ZIPPED, Value::U8(1)));
    }
    let mut levels = [0u8; 24];
    levels[0..4].copy_from_slice(&(((ch.pan + 1.0) * 6400.0).round() as i32).to_le_bytes());
    levels[4..8].copy_from_slice(&((ch.volume * 12800.0).round() as i32).to_le_bytes());
    levels[8..12].copy_from_slice(&ch.pitch_cents.to_le_bytes());
    levels[13] = 1; // the template's `00 01 00 00 …` tail
    b.push(Event::new(id::CH_LEVELS, Value::var(levels.to_vec())));
    if ch.kind == ChannelKind::AudioClip {
        // FL writes the sample settings right after the levels.
        b.push(Event::new(id::CH_SAMPLE_SETTINGS, Value::var(audio_clip_215(ch.sound.stretch))));
    }
    b.push(Event::new(id::CH_GROUP, Value::U32(ch.rack.group)));
    if let Some(a) = auto.filter(|_| ch.kind == ChannelKind::Automation) {
        b.push(Event::new(id::AUTO_DATA, Value::var(encode_automation(a))));
    }
    b.push(Event::new(id::CH_LOCKED, Value::U8(u8::from(ch.rack.locked))));
    if let Some(p) = &ch.sample_path {
        b.push(Event::new(id::SAMPLE_PATH, Value::var(encode_text(p, enc))));
    }
    b.push(Event::new(id::CH_END, Value::U8(0)));
    // The Misc tab's values that differ from what the block reads as.
    let old = load_misc(&b);
    let add = write_misc(&mut b, ch, &old);
    let end = b.len() - 1;
    b.splice(end..end, add);
    Ok(b)
}

/// Builds a new FLP file from the model: project info, tempo, patterns (notes, control points, names, colors)
/// and channels, in FL's order. FL fills in everything not written (mixer, playlist, per-channel settings), which
/// this version does not write yet: see [`unsaved_parts`]. Opens in FL 26 (checked 2026-10-09).
// spec: flp/container.md, flp/events/{project-info,channels,patterns}.md
pub fn create(project: &Project) -> Result<FlpFile, ProjectError> {
    let enc = TextEnc::Utf16;
    let mut ev = vec![
        Event::new(id::VERSION, Value::var(format!("{COMPAT_VERSION}\0").into_bytes())),
        Event::new(159, Value::U32(COMPAT_BUILD)),
        // Without it FL 26 reads only the first playlist clip (spec: flp/events/project-info.md, event 169).
        Event::new(169, Value::U32(15)),
        Event::new(id::TEMPO, Value::U32((project.tempo.unwrap_or(130.0) * 1000.0).round() as u32)),
    ];
    // The project settings in FL 26's order (spec: flp/engine-events.md "Project", "Order in the file": 156, 67, 9, 11,
    // 80, 17, 18, 35, 23, 44, 30, 10), those the model has.
    if let Some(p) = project.current_pattern {
        ev.push(Event::new(id::CURRENT_PATTERN, Value::U16(p)));
    }
    if let Some(on) = project.song_mode {
        ev.push(Event::new(id::SONG_MODE, Value::U8(u8::from(on))));
    }
    if let Some(v) = project.swing.filter(|&v| v != 0) {
        ev.push(Event::new(id::SWING, Value::U8(v)));
    }
    if let Some(v) = project.main_pitch.filter(|&v| v != 0) {
        ev.push(Event::new(id::MAIN_PITCH, Value::U16(v as u16)));
    }
    if let Some((n, d)) = project.bar {
        ev.push(Event::new(id::TIME_NUM, Value::U8(signature_byte(n))));
        ev.push(Event::new(id::TIME_DEN, Value::U8(signature_byte(d))));
    }
    if let Some(on) = project.bar_as_signature {
        ev.push(Event::new(id::TIME_MODE, Value::U8(u8::from(on))));
    }
    if let Some(v) = project.panning_law {
        ev.push(Event::new(id::PANNING_LAW, Value::U8(v)));
    }
    if let Some(on) = project.auto_crossfades {
        ev.push(Event::new(id::AUTO_CROSSFADES, Value::U8(u8::from(on))));
    }
    // Present only when on (spec: flp/engine-events.md "Event 30").
    if project.play_truncated {
        ev.push(Event::new(id::PLAY_TRUNCATED, Value::U8(1)));
    }
    if let Some(on) = project.show_info {
        ev.push(Event::new(id::SHOW_INFO, Value::U8(u8::from(on))));
    }
    // The project information as FL 26 writes it (spec: flp/events/project-info.md "Writer notes"): 194, 206, 207 and
    // 195 always, empty when empty; 197 only with a URL; then 237, the start (now for a project without one) and the
    // time spent.
    let info = &project.info;
    for (id, value) in [
        (id::TITLE, info.title.clone()),
        (id::GENRE, info.genre.clone()),
        (id::AUTHOR, info.author.clone()),
        (id::COMMENTS, info.comments.as_deref().map(comments_to_file)),
        (id::URL, info.url.clone()),
    ] {
        // The data folder (167, 202) between the author and the comments, when the model has one (spec:
        // ui/data-folder.md §2).
        if let Some(df) = project.data_folder.as_ref().filter(|_| id == id::COMMENTS) {
            ev.push(int_event(id::INFO_167, df.choice));
            ev.push(Event::new(id::INFO_202, Value::var(encode_text(&df.folder, enc))));
        }
        let v = value.unwrap_or_default();
        if id != id::URL || !v.is_empty() {
            ev.push(Event::new(id, Value::var(encode_text(&v, enc))));
        }
    }
    ev.push(Event::new(id::WORK_TIME, Value::var(work_time_bytes(project.work_time.unwrap_or_else(work_time_now)))));
    // Filter groups and the current filter (spec: flp/events/project-info.md), before the note blocks.
    for name in &project.groups {
        ev.push(Event::new(id::GROUP_NAME, Value::var(encode_text(name, enc))));
    }
    if !project.groups.is_empty() {
        ev.push(Event::new(id::CURRENT_GROUP, Value::U32(project.current_group.unwrap_or(u32::MAX))));
    }
    // The initial values: FL writes event 216 in every project, with length 0 when there are none (spec:
    // ui/settings-choices.md "Advanced: Initialize controls automatically"), after 146.
    ev.push(Event::new(id::INIT_CONTROLS, Value::var(init_controls_bytes(&project.init_controls, &[]))));
    let mut patterns: Vec<&Pattern> = project.patterns.iter().collect();
    patterns.sort_by_key(|p| p.number);
    for pat in &patterns {
        if !pat.notes.is_empty() || !pat.controls.is_empty() || !pat.loops.is_empty() {
            ev.extend(note_block_events(pat));
        }
    }
    for a in &project.automations {
        ev.extend(a.targets.iter().map(|&t| link_event(a.channel, t)));
    }
    for c in &project.controller_links {
        ev.push(Event::new(id::AUTO_LINK, Value::var(c.bytes.to_vec())));
        ev.extend(c.formula.as_deref().map(|s| Event::new(id::LINK_FORMULA, Value::var(encode_text(s, enc)))));
    }
    for (i, ch) in project.channels.iter().enumerate() {
        ev.extend(channel_block_events(ch, project.automation(ch.index), enc)?);
        if i == 0 {
            for pat in &patterns {
                if pat.name.is_some() || pat.color.is_some() || pat.length.is_some() || pat.loop_off.is_some() || !pat.markers.is_empty() {
                    ev.extend(meta_block_events(pat, enc));
                }
            }
        }
    }
    for arr in &project.arrangements {
        ev.extend(arrangement_events(arr, enc, CLIP_LEN));
    }
    // The playlist view and the arrangement shown, right after the last one's tracks as FL writes them
    // (spec: flp/events/playlist.md, events 242 and 100).
    if let Some(v) = project.playlist_view.filter(|_| !project.arrangements.is_empty()) {
        ev.push(Event::new(id::PLAYLIST_VIEW, Value::var(v.raw.to_vec())));
    }
    if let Some(n) = project.current_arrangement.filter(|_| !project.arrangements.is_empty()) {
        ev.push(Event::new(id::CURRENT_ARRANGEMENT, Value::U16(n)));
        // Fast declick for cut groups stands after event 100 (spec: flp/engine-events.md "Project").
        if let Some(on) = project.fast_declick {
            ev.push(Event::new(id::FAST_DECLICK, Value::U8(u8::from(on))));
        }
    }
    // The mixer follows the arrangements (spec: flp/events/mixer.md "Where the mixer sits").
    if !project.arrangements.is_empty() && !project.mixer.is_empty() {
        ev.extend(mixer_events(&project.mixer, project.main_volume, enc));
    }
    let mut header = Vec::with_capacity(6);
    header.extend(0u16.to_le_bytes()); // format
    header.extend((project.channels.len() as u16).to_le_bytes());
    header.extend(project.ppq.max(1).to_le_bytes());
    Ok(FlpFile {
        chunks: vec![
            Chunk { tag: Tag::HEADER, body: ChunkBody::Header(Header { raw: header }) },
            Chunk { tag: Tag::DATA, body: ChunkBody::Events(ev) },
        ],
        trailing: Vec::new(),
    })
}

/// Number of playlist tracks an arrangement stores (spec: flp/events/playlist.md).
pub const PLAYLIST_TRACKS: u32 = 500;

/// A track record (event 238) with the template's defaults (spec: flp/events/playlist.md "Event 238 layout").
fn track_record(t: &PlaylistTrack) -> Vec<u8> {
    let mut r = vec![0u8; 70];
    r[0..4].copy_from_slice(&t.number.to_le_bytes());
    r[4..8].copy_from_slice(&t.color.map_or(0x002D_2C29, |c| bgr_from_rgb(c, 0)).to_le_bytes());
    r[12] = u8::from(t.enabled);
    r[13..17].copy_from_slice(&1.0f32.to_le_bytes());
    r[17] = if t.number <= 33 { 0xf0 } else { 0xff };
    r[18..21].fill(0xff);
    r[30] = 5;
    r[38] = 1;
    r[46] = u8::from(t.grouped);
    r[47] = u8::from(t.locked);
    r[53..61].fill(0xff);
    r[61] = u8::from(!t.collapsed);
    r
}

/// An arrangement block: number, name, 36, clips, markers, then the 500 track records (spec: flp/events/playlist.md).
fn arrangement_events(arr: &Arrangement, enc: TextEnc, clen: usize) -> Vec<Event> {
    let mut ev = vec![
        Event::new(id::ARR_START, Value::U16(arr.number)),
        Event::new(id::ARR_NAME, Value::var(encode_text(arr.name.as_deref().unwrap_or("Arrangement"), enc))),
        Event::new(id::ARR_UNKNOWN, Value::U8(0)),
        Event::new(id::ARR_CLIPS, Value::var(encode_clips(&arr.clips, clen))),
    ];
    for m in &arr.markers {
        ev.extend(marker_events(m, enc));
    }
    for n in 1..=PLAYLIST_TRACKS {
        let t = arr.tracks.iter().find(|t| t.number == n).cloned().unwrap_or_else(|| PlaylistTrack::new(n));
        ev.push(Event::new(id::PL_TRACK, Value::var(track_record(&t))));
        ev.push(Event::new(id::PL_COLOR_FLAG, Value::U8(u8::from(t.color.is_some()))));
        if let Some(name) = &t.name {
            ev.push(Event::new(id::PL_NAME, Value::var(encode_text(name, enc))));
        }
    }
    ev
}

/// Track parameter records of a default track, in file order (spec: flp/events/mixer.md "Event 225"; the EQ-like IDs
/// keep the values every untouched track has).
const TRACK_PARAM_DEFAULTS: [(u8, i32); 12] = [
    (MX_VOLUME, 12800),
    (MX_PAN, 0),
    (MX_STEREO, 0),
    (0xD0, 0),
    (0xD1, 0),
    (0xD2, 0),
    (0xD8, 5777),
    (0xD9, 33145),
    (0xDA, 55825),
    (0xE0, 17500),
    (0xE1, 17500),
    (0xE2, 17500),
];
/// The record at location 0x4000 and the parameter block of "track 501" that every file has (spec: mixer.md).
const MX_SPECIAL_LOC: u16 = 0x4000;
const MX_SPECIAL_TRACK: u16 = 501;
/// Track flags (event 236) byte 4 of the master and the last track, and of an insert (spec: flp/events/mixer.md).
const MX_FLAGS_MASTER: u8 = 0x0C;
const MX_FLAGS_INSERT: u8 = 0x4C;

fn param_rec(pid: u8, kind: u8, loc: u16, v: i32) -> [u8; MIXER_REC_LEN] {
    let mut r = [0u8; MIXER_REC_LEN];
    r[4] = pid;
    r[5] = kind;
    r[6..8].copy_from_slice(&loc.to_le_bytes());
    r[8..12].copy_from_slice(&v.to_le_bytes());
    r
}

/// One track's parameter records in file order: each slot's LED and mix, the track parameters, the route levels
/// (spec: flp/events/plugins.md §5, mixer.md "Event 225").
fn track_records(t: u16, m: Option<&MixerTrack>) -> Vec<u8> {
    let d = MixerTrack::new(t);
    let m = m.unwrap_or(&d);
    let mut out = Vec::new();
    for k in 0..10u16 {
        let s = usize::from(k);
        out.extend(param_rec(MX_SLOT_ON, MX_KIND_PARAM, mixer_location(t, k), i32::from(m.slot_on[s])));
        out.extend(param_rec(MX_SLOT_MIX, MX_KIND_PARAM, mixer_location(t, k), (m.slot_mix[s].clamp(0.0, 1.0) * SLOT_MIX_FULL).round() as i32));
    }
    for (pid, v) in TRACK_PARAM_DEFAULTS {
        let v = match pid {
            MX_VOLUME => m.volume.map_or(v, |x| (x * 16000.0).round() as i32),
            MX_PAN => m.pan.map_or(v, |x| (x * 6400.0).round() as i32),
            MX_STEREO => m.stereo_separation.map_or(v, |x| (x * 64.0).round() as i32),
            _ => v,
        };
        out.extend(param_rec(pid, MX_KIND_PARAM, mixer_location(t, 0), v));
    }
    let mut levels = m.send_levels.clone();
    levels.sort_by_key(|&(d, _)| d);
    for (dest, level) in levels {
        out.extend(param_rec(dest.min(255) as u8, MX_KIND_ROUTE, mixer_location(t, 0), (level * 16000.0).round() as i32));
    }
    out
}

/// The routing bytes of event 235: one per destination from track 0, trailing zeros omitted, at least one byte.
fn route_bytes(routes: &[u16]) -> Vec<u8> {
    let mut bytes = vec![0u8; routes.iter().map(|&r| usize::from(r) + 1).max().unwrap_or(1)];
    for &r in routes {
        bytes[usize::from(r)] = 1;
    }
    bytes
}

/// One mixer track block (spec: flp/events/mixer.md "Track blocks"): colour, icon, name, flags, the ten slots, routing
/// and the trailing events with the values untouched tracks have.
fn track_block_events(m: &MixerTrack, last: bool, enc: TextEnc) -> Vec<Event> {
    let mut ev = Vec::new();
    if let Some(c) = m.color {
        ev.push(Event::new(id::MX_COLOR, Value::U32(bgr_from_rgb(c, 0xFF))));
    }
    ev.push(Event::new(id::MX_COLOR_FLAG, Value::U8(u8::from(m.color.is_some()))));
    if let Some(i) = m.icon {
        ev.push(Event::new(id::MX_ICON, Value::U16(i)));
    }
    if let Some(n) = m.name.as_ref().filter(|n| !n.is_empty()) {
        ev.push(Event::new(id::MX_NAME, Value::var(encode_text(n, enc))));
    }
    let mut flags = [0u8; 12];
    flags[..4].copy_from_slice(&m.latency_out.to_le_bytes());
    flags[8..].copy_from_slice(&m.latency_in.to_le_bytes());
    flags[4] = if m.index == 0 || last { MX_FLAGS_MASTER } else { MX_FLAGS_INSERT };
    let rt = if last && m.index > 0 { MX_SPECIAL_TRACK } else { m.index };
    flags[4] = with_flags(flags[4], m);
    ev.push(Event::new(id::MX_FLAGS, Value::var(flags.to_vec())));
    for (k, slot) in m.slots.iter().enumerate() {
        if let Some(p) = slot {
            ev.extend(slot_plugin_events(p, rt, k as u8, None, enc));
        }
        ev.push(Event::new(id::MX_SLOT, Value::U16(k as u16)));
    }
    ev.push(Event::new(id::MX_ROUTES, Value::var(route_bytes(&m.routes))));
    ev.push(Event::new(id::MX_REC_SOURCE, Value::U32(m.rec_source.into())));
    ev.push(Event::new(id::MX_MONITOR, Value::U32(m.monitor.into())));
    ev.push(Event::new(49, Value::U8(0)));
    ev.push(Event::new(id::MX_INPUT, Value::U32(m.audio_in as u32)));
    ev.push(Event::new(id::MX_OUTPUT, Value::U32(m.audio_out as u32)));
    ev
}

/// A whole mixer: the track count (event 103), the track blocks, and the parameter records (event 225) with the
/// special records every file has (spec: flp/events/mixer.md).
fn mixer_events(mixer: &[MixerTrack], main_volume: Option<u16>, enc: TextEnc) -> Vec<Event> {
    let mut ev = vec![Event::new(id::MX_COUNT, Value::U16(mixer.len() as u16))];
    for (n, m) in mixer.iter().enumerate() {
        ev.extend(track_block_events(m, n + 1 == mixer.len(), enc));
    }
    let mut recs = param_rec(0, 0, MX_SPECIAL_LOC, i32::from(main_volume.unwrap_or(Project::MAIN_VOLUME_DEFAULT))).to_vec();
    let (current, rest) = match mixer.split_last() {
        Some((c, rest)) if !rest.is_empty() => (Some(c), rest),
        _ => (None, mixer),
    };
    for m in rest {
        recs.extend(track_records(m.index, Some(m)));
    }
    // The last ("current") track's records sit at track 501 (spec: mixer.md "The last track").
    recs.extend(track_records(MX_SPECIAL_TRACK, current));
    ev.push(Event::new(id::MX_PARAMS, Value::var(recs)));
    ev
}

/// A fresh arrangement as FL's template has it: number 0, "Arrangement", 500 default tracks.
pub fn default_arrangement() -> Arrangement {
    Arrangement {
        number: 0,
        name: Some("Arrangement".into()),
        tracks: (1..=PLAYLIST_TRACKS).map(PlaylistTrack::new).collect(),
        ..Default::default()
    }
}

/// What `apply` cannot store in `file` yet: the mixer and playlist of a project made by [`create`] (their file
/// has none). Empty when nothing would be lost.
pub fn unsaved_parts(project: &Project, file: &FlpFile) -> Vec<&'static str> {
    let Ok(current) = load(file) else { return Vec::new() };
    let mut parts = Vec::new();
    if current.mixer.is_empty() && !mixer_is_default(&project.mixer) {
        parts.push("mixer settings");
    }
    // The main volume lives in the mixer's parameter records: a file without them cannot hold it.
    if current.main_volume.is_none() && project.main_volume.is_some_and(|v| v != Project::MAIN_VOLUME_DEFAULT) {
        parts.push("main volume");
    }
    if current.arrangements.is_empty() && project.arrangements.iter().any(|a| !a.clips.is_empty()) {
        parts.push("playlist clips");
    }
    parts
}

/// Tracks as a fresh project has them: unnamed, uncolored, unity volume (0.8), centered, routed to the master.
fn mixer_is_default(mixer: &[MixerTrack]) -> bool {
    mixer.iter().enumerate().all(|(n, t)| {
        // The last stored track is routed nowhere in FL's files.
        let hidden = n + 1 == mixer.len() && n > 0 && t.routes.is_empty();
        hidden ||
        t.name.is_none()
            && t.color.is_none()
            && t.enabled
            && t.slots.iter().all(Option::is_none)
            && t.slot_on.iter().all(|&on| on)
            && t.slot_mix.iter().all(|&m| m == 1.0)
            && !t.polarity
            && !t.swap
            && t.slots_enabled
            && t.send_levels.is_empty()
            && t.volume.is_none_or(|v| (v - 0.8).abs() < 1e-9)
            && t.pan.is_none_or(|v| v == 0.0)
            && t.stereo_separation.is_none_or(|v| v == 0.0)
            && t.routes == if t.index == 0 { Vec::<u16>::new() } else { vec![0] }
            && t.latency_out == 0.0
            && t.latency_in == 0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sampler(index: u16, name: &str, track: u16) -> Channel {
        Channel {
            id: os_model::Id::NONE,
            index,
            kind: ChannelKind::Sampler,
            name: Some(name.into()),
            plugin: None,
            plugin_state: None,
            color: None,
            enabled: true,
            volume: 10000.0 / 12800.0,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: track,
            sample_path: None,
            layer_children: vec![],
            envelopes: Default::default(), rack: Default::default(), misc: Default::default(), sound: Default::default(),
        }
    }

    fn new_project() -> Project {
        let mut b = sampler(1, "Bass", 2);
        b.color = Some(Rgb::new(200, 40, 90));
        b.pan = -0.5;
        b.volume = 0.5;
        b.pitch_cents = -50;
        b.sample_path = Some("C:/samples/bass.wav".into());
        Project {
            ppq: 96,
            tempo: Some(128.5),
            current_pattern: Some(2),
            info: ProjectInfo { title: Some("My song".into()), ..Default::default() },
            channels: vec![sampler(0, "Kick", 1), b],
            patterns: vec![
                Pattern { number: 2, name: Some("Verse".into()), color: Some(Rgb::new(10, 20, 30)), notes: vec![Note::step(0, 48), Note::step(1, 0)], controls: vec![ControlPoint::new(0, 1, ControlPoint::PAN, 3200)], ..Pattern::default() },
                Pattern { number: 1, notes: vec![Note::step(0, 0)], ..Pattern::default() },
            ],
            ..Project::default()
        }
    }

    /// A project made from the model alone reads back the same, and `apply` can edit it afterwards
    /// (spec: flp/container.md, flp/events/channels.md, patterns.md).
    #[test]
    fn created_project_reads_back_and_takes_edits() {
        let p = new_project();
        let file = create(&p).unwrap();
        let bytes = file.to_bytes().unwrap();
        let mut file = FlpFile::parse(&bytes).unwrap();
        assert_eq!(file.to_bytes().unwrap(), bytes);
        let back = load(&file).unwrap();
        assert_eq!(back.ppq, 96);
        assert_eq!(back.tempo, Some(128.5));
        assert_eq!(back.current_pattern, Some(2));
        assert_eq!(back.info.title.as_deref(), Some("My song"));
        assert_eq!(back.channels.len(), 2);
        assert_eq!(back.channels[0].name.as_deref(), Some("Kick"));
        let b = &back.channels[1];
        assert_eq!((b.color, b.pan, b.volume, b.pitch_cents, b.mixer_track), (Some(Rgb::new(200, 40, 90)), -0.5, 0.5, -50, 2));
        assert_eq!(b.sample_path.as_deref(), Some("C:/samples/bass.wav"));
        let numbers: Vec<u16> = back.patterns.iter().map(|p| p.number).collect();
        assert_eq!(numbers, vec![1, 2], "note blocks ascend");
        let verse = back.patterns.iter().find(|p| p.number == 2).unwrap();
        assert_eq!((verse.name.as_deref(), verse.color), (Some("Verse"), Some(Rgb::new(10, 20, 30))));
        assert_eq!(verse.notes.len(), 2);
        assert_eq!(verse.controls, p.patterns[0].controls);
        // The file has no mixer or playlist, and says so; edits to what it does store still apply.
        assert!(back.mixer.is_empty() && back.arrangements.is_empty());
        let mut edited = back.clone();
        edited.tempo = Some(100.0);
        edited.channels[0].volume = 0.25;
        // Samples loaded into a sampler: added where there was none, replaced where there was one.
        edited.channels[0].sample_path = Some("C:/samples/kick.wav".into());
        edited.channels[1].sample_path = Some("C:/samples/bass2.wav".into());
        edited.patterns[0].notes.push(Note::step(0, 96));
        edited.patterns.push(Pattern { number: 3, name: Some("Chorus".into()), notes: vec![Note::step(1, 24)], ..Pattern::default() });
        apply(&edited, &mut file).unwrap();
        let again = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(again.tempo, Some(100.0));
        assert_eq!(again.channels[0].volume, 0.25);
        assert_eq!(again.channels[0].sample_path.as_deref(), Some("C:/samples/kick.wav"));
        assert_eq!(again.channels[1].sample_path.as_deref(), Some("C:/samples/bass2.wav"));
        assert_eq!(again.patterns.iter().map(|p| p.number).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert_eq!(again.patterns[2].name.as_deref(), Some("Chorus"));
        assert_eq!(again.patterns[0].notes.len(), 2);
    }

    /// A sampler channel's settings that Lemon Sampler edits are saved (spec: plugins/lemon-sampler.md §7): a block
    /// made by `create` holds none of their events, so they are added; saving again patches them in place; another
    /// channel's events are left alone.
    #[test]
    fn sampler_settings_are_saved() {
        let mut p = new_project();
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        // The file's blocks are the model's channels, as after a save.
        for (i, c) in p.channels.iter_mut().enumerate() {
            c.rack.origin = Some(i as u16);
        }
        let ch = &mut p.channels[0];
        let vol = &mut ch.envelopes[Envelope::VOLUME];
        (vol.enabled, vol.attack, vol.sustain, vol.release_tension) = (true, 50, 100, -101);
        (vol.flags, vol.lfo_amount, vol.lfo_speed, vol.lfo_shape) = (3, -64, 40000, 2);
        ch.envelopes[Envelope::PITCH].amount = 64;
        (ch.sound.cutoff, ch.sound.resonance, ch.sound.filter_type, ch.sound.declick, ch.sound.sampler_flags) = (128, 64, 5, 3, Some(0));
        ch.sound.fx = SampleFx {
            flags: SampleFx::REVERSE | SampleFx::SWAP,
            boost: 128,
            stereo_delay: 1024,
            ring_mix: 64,
            ring_freq: 200,
            reverb: 30,
            reverb_type: 0,
            ping_pong: true,
            normalize: true,
            trim: 32768,
            start_bits: 0.25f64.to_bits(),
            length_bits: 0.5f64.to_bits(),
            loop_follows_reverse: true,
            ..SampleFx::default()
        };
        (ch.sound.loop_points, ch.sound.start_offset_bits) = (Some([694, 18000]), 0.125f64.to_bits());
        (ch.misc.root, ch.misc.fine, ch.misc.add_key, ch.misc.key_range) = (48, -35, true, [37, 60]);
        ch.envelopes[Envelope::PAN].flags = 0x20;
        apply(&p, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        let (c, want) = (&back.channels[0], &p.channels[0]);
        assert_eq!(c.envelopes, want.envelopes);
        assert_eq!(c.sound, want.sound);
        assert_eq!(c.misc, want.misc);
        assert!(c.misc.main_pitch, "an added event 215 keeps the main pitch switch on");
        let ev = events_of(&file).unwrap();
        // Event 215 is shared with the Misc tab's writer, which owns root and fine: each is added once.
        for eid in [id::CH_SAMPLE_SETTINGS, id::CH_ROOT, id::CH_FINE] {
            assert_eq!(ev.iter().filter(|e| e.id == eid).count(), 1, "event {eid}");
        }
        let rec = ev.iter().find(|e| e.id == id::CH_LOOP_POINTS).and_then(Event::data).unwrap();
        assert_eq!(rec.len(), 43, "FL's record size");
        let env = ev.iter().filter(|e| e.id == id::CH_ENVELOPE).nth(Envelope::VOLUME).and_then(Event::data).unwrap();
        assert_eq!(&env[4..8], &(-1i32).to_le_bytes(), "an envelope that is on is written as -1, as FL does");
        assert_eq!(back.channels[1], load(&create(&new_project()).unwrap()).unwrap().channels[1]);

        // Saved again: the events are there now and are patched, none added.
        let mut file = file;
        let count = |f: &FlpFile| events_of(f).unwrap().len();
        let before = count(&file);
        let ch = &mut p.channels[0];
        ch.envelopes[Envelope::VOLUME].attack = 32768;
        (ch.sound.cutoff, ch.sound.declick, ch.sound.sampler_flags) = (256, 0, Some(8));
        (ch.sound.fx.flags, ch.sound.fx.boost, ch.sound.fx.ping_pong) = (0, 0, false);
        ch.sound.fx.start_bits = 0f64.to_bits();
        (ch.sound.loop_points, ch.sound.start_offset_bits) = (None, 0);
        (ch.misc.root, ch.misc.fine, ch.misc.add_key, ch.misc.main_pitch, ch.misc.key_range) = (60, 0, false, false, [0, 256]);
        apply(&p, &mut file).unwrap();
        assert_eq!(count(&file), before);
        let back = load(&file).unwrap();
        assert_eq!(back.channels[0].envelopes, p.channels[0].envelopes);
        // Loop points taken away stay as a record holding the sample's ends.
        assert_eq!(back.channels[0].sound, ChannelSound { loop_points: Some([-1, i64::MAX]), ..p.channels[0].sound });
        assert_eq!(back.channels[0].misc, p.channels[0].misc);
    }

    /// A sample taken out (the Sampler window's file box X) takes its path (event 196) and loop points (event 251) out
    /// of the channel's block and turns Resample off; the other flags stay (spec: flp/events/channels.md "Loop points
    /// (event 251)", "Sampler flags (event 143)").
    #[test]
    fn cleared_sample_leaves_the_block() {
        let mut p = settled(new_project());
        p.channels[1].sound.loop_points = Some([694, 18000]);
        p.channels[1].sound.sampler_flags = Some(ChannelSound::RESAMPLE | ChannelSound::DEFAULT_FLAGS);
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        apply(&p, &mut file).unwrap();
        let has = |f: &FlpFile, eid: u8| events_of(f).unwrap().iter().any(|e| e.id == eid);
        assert!(has(&file, id::SAMPLE_PATH) && has(&file, id::CH_LOOP_POINTS));
        let mut p = load(&file).unwrap();
        assert_eq!(p.channels[1].sound.loop_points, Some([694, 18000]));
        let other = p.channels[0].clone();
        p.channels[1].clear_sample();
        assert_eq!(p.channels[1].sound.sampler_flags, Some(ChannelSound::DEFAULT_FLAGS));
        apply(&p, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        assert!(!has(&file, id::SAMPLE_PATH) && !has(&file, id::CH_LOOP_POINTS), "the only sample's events are gone");
        let back = load(&file).unwrap();
        assert_eq!(back.channels[1], p.channels[1]);
        assert_eq!(back.channels[0], other);
        // Writing the cleared channel again changes nothing; loading a new sample brings the path back.
        let mut again = file.clone();
        apply(&p, &mut again).unwrap();
        assert_eq!(again, file);
        p.channels[1].sample_path = Some("C:/samples/bass3.wav".into());
        apply(&p, &mut again).unwrap();
        assert_eq!(load(&again).unwrap().channels[1].sample_path.as_deref(), Some("C:/samples/bass3.wav"));
    }

    /// The events the engine reads beyond the editable settings (spec: flp/engine-events.md): time signature,
    /// "play truncated notes", the channel's swing share, root note, sampler flags, declick and stretch settings,
    /// add to key, key zone, start offset and loop points, level adjustments, filter, the envelope records' flags and LFO, cut groups and the voice settings.
    #[test]
    fn engine_settings_are_read() {
        let mut file = create(&new_project()).unwrap();
        {
            let ev = events_of_mut(&mut file).unwrap();
            let ch0 = ev.iter().position(|e| e.id == id::CH_START).unwrap();
            let mut s215 = vec![0u8; 168];
            s215[83] = 3;
            for (k, v) in [1536u32, 25, 10000, 5].iter().enumerate() {
                s215[96 + 4 * k..100 + 4 * k].copy_from_slice(&v.to_le_bytes());
            }
            // Add to key, key zone 37..60, start offset 1/8.
            s215[63] = 1;
            s215[68..76].copy_from_slice(&[37i32.to_le_bytes(), 60i32.to_le_bytes()].concat());
            s215[144..152].copy_from_slice(&0.125f64.to_le_bytes());
            // Event 251 as FL 26 saved it with loop start 694, end 695 (frames).
            let hex = "02000000b602000000000000b702000000000000000000000000f0bfffffffff00ffffff7fffffff7f0100";
            let r251: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
            let ints = |v: &[i32]| Value::var(v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>());
            let env = [1, -1, 100, 20000, 20000, 30000, 50, 20000, 0, 2000, 3000, -64, 20000, 2, 0, 0, -101];
            let mut add = vec![
                Event::new(id::CH_SWING_SHARE, Value::U16(64)),
                Event::new(id::CH_ROOT, Value::U32(48)),
                Event::new(id::CH_FINE, Value::U32(-35i32 as u32)),
                Event::new(id::CH_SAMPLER_FLAGS, Value::U32(10)),
                Event::new(id::CH_SAMPLE_SETTINGS, Value::var(s215)),
                Event::new(id::CH_LOOP_POINTS, Value::var(r251)),
                Event::new(id::CH_ADJUST, ints(&[-3200, 25600, 0, 128, -256])),
                // Cuts group 7, cut by group 9; 3 voices at most, slide time 820, mono and portamento.
                Event::new(id::CH_CUT_GROUPS, Value::U32(9 << 16 | 7)),
                Event::new(id::CH_POLY, Value::var([3u32.to_le_bytes(), 820u32.to_le_bytes()].concat().into_iter().chain([3u8]).collect::<Vec<u8>>())),
            ];
            add.extend((0..5).map(|_| Event::new(id::CH_ENVELOPE, ints(&env))));
            // Replace whatever envelope records the file has with five of these in the first channel.
            ev.retain(|e| e.id != id::CH_ENVELOPE);
            let at = ch0 + 1;
            for (k, e) in add.into_iter().enumerate() {
                ev.insert(at + k, e);
            }
            let levels = ev.iter_mut().skip(ch0).find(|e| e.id == id::CH_LEVELS);
            match levels {
                Some(e) => {
                    let mut d = e.data().unwrap().to_vec();
                    d[12..24].copy_from_slice(&[128i32, 64, 5].iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>());
                    *e = Event::new(id::CH_LEVELS, Value::var(d));
                }
                None => ev.insert(at, Event::new(id::CH_LEVELS, ints(&[6400, 10000, 0, 128, 64, 5]))),
            }
            ev.insert(0, Event::new(id::PLAY_TRUNCATED, Value::U8(1)));
            ev.insert(0, Event::new(id::TIME_DEN, Value::U8(8)));
            ev.insert(0, Event::new(id::TIME_NUM, Value::U8(6)));
        }
        let p = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!((p.bar, p.play_truncated), (Some((6, 8)), true));
        let c = &p.channels[0];
        assert_eq!((c.misc.swing, c.misc.root), (0.5, 48));
        // Fine tune (event 142) and the main pitch switch (215 byte 11, cleared here).
        assert_eq!((c.misc.fine, c.misc.main_pitch), (-35, false));
        assert_eq!((c.misc.add_key, c.misc.key_range), (true, [37, 60]));
        assert_eq!((c.sound.loop_points, c.sound.start_offset()), (Some([694, 695]), 0.125));
        assert_eq!(c.misc.levels, [-0.5, 1.0, 0.5, -1.0]);
        assert_eq!((c.misc.cut, c.misc.cut_by, c.misc.max_poly, c.misc.mono, c.misc.porta), (7, 9, 3, true, true));
        assert_eq!(c.misc.slide, 820.0 / 1024.0);
        assert_eq!((c.sound.cutoff, c.sound.resonance, c.sound.filter_type), (128, 64, 5));
        assert_eq!((c.sound.sampler_flags, c.sound.declick, c.sound.stretch), (Some(10), 3, [1536, 25, 10000, 5]));
        let e = c.envelopes[Envelope::VOLUME];
        assert_eq!((e.enabled, e.flags, e.lfo_delay, e.lfo_attack, e.lfo_amount, e.lfo_speed, e.lfo_shape), (true, 1, 2000, 3000, -64, 20000, 2));
        // Files without these events keep the defaults.
        let plain = load(&create(&new_project()).unwrap()).unwrap();
        assert_eq!((plain.bar, plain.play_truncated), (None, false));
        assert_eq!((plain.channels[0].misc.swing, plain.channels[0].misc.root, plain.channels[0].sound), (1.0, 60, ChannelSound::default()));
        assert_eq!((plain.channels[0].misc.fine, plain.channels[0].misc.main_pitch), (0, true));
        let m = plain.channels[0].misc;
        assert_eq!((m.add_key, m.key_range), (false, [0, 256]));
        assert_eq!((m.cut, m.cut_by, m.max_poly, m.mono, m.porta, m.slide), (0, 0, 0, false, false, 500.0 / 1024.0));
    }

    /// A sample channel's start, length, loop options and precomputed effects are read in the file's units (spec:
    /// audio/precomputed-effects.md "Settings"); writing the project back leaves their events as they were.
    #[test]
    fn precomputed_effect_settings_are_read_and_kept() {
        let mut file = create(&new_project()).unwrap();
        {
            let ev = events_of_mut(&mut file).unwrap();
            let ch0 = ev.iter().position(|e| e.id == id::CH_START).unwrap();
            ev.retain(|e| {
                ![
                    id::SMP_PING_PONG,
                    id::SMP_FEEDBACK,
                    id::SMP_FLAGS,
                    id::SMP_CUTOFF,
                    id::SMP_BOOST,
                    id::SMP_FADE_OUT,
                    id::SMP_FADE_IN,
                    id::SMP_RESONANCE,
                    id::SMP_STEREO_DELAY,
                    id::SMP_POGO,
                    id::SMP_RING,
                    id::SMP_REVERB,
                    id::CH_SAMPLE_SETTINGS,
                ]
                .contains(&e.id)
            });
            let mut s215 = vec![0u8; 168];
            (s215[9], s215[80], s215[81], s215[156]) = (1, 1, 1, 1);
            s215[84..88].copy_from_slice(&512u32.to_le_bytes());
            s215[88..92].copy_from_slice(&300u32.to_le_bytes());
            s215[128..136].copy_from_slice(&0.25f64.to_le_bytes());
            s215[136..144].copy_from_slice(&0.5f64.to_le_bytes());
            let add = [
                Event::new(id::SMP_PING_PONG, Value::U8(1)),
                Event::new(id::SMP_FEEDBACK, Value::U16(64)),
                Event::new(id::SMP_FLAGS, Value::U16(1 | 2 | 4 | 256)),
                Event::new(id::SMP_CUTOFF, Value::U16(512)),
                Event::new(id::SMP_BOOST, Value::U16(64)),
                Event::new(id::SMP_FADE_OUT, Value::U16(256)),
                Event::new(id::SMP_FADE_IN, Value::U16(1024)),
                Event::new(id::SMP_RESONANCE, Value::U16(1000)),
                Event::new(id::SMP_STEREO_DELAY, Value::U16(3072)),
                Event::new(id::SMP_POGO, Value::U16(128)),
                Event::new(id::SMP_RING, Value::U32(100 << 16 | 200)),
                Event::new(id::SMP_REVERB, Value::U32(200)),
                Event::new(id::CH_SAMPLE_SETTINGS, Value::var(s215)),
            ];
            for (k, e) in add.into_iter().enumerate() {
                ev.insert(ch0 + 1 + k, e);
            }
        }
        let bytes = file.to_bytes().unwrap();
        let p = load(&FlpFile::parse(&bytes).unwrap()).unwrap();
        let f = p.channels[0].sound.fx;
        assert_eq!(
            (f.flags, f.boost, f.fade_out, f.fade_in, f.stereo_delay, f.pogo, f.feedback, f.cutoff, f.resonance),
            (263, 64, 256, 1024, 3072, 128, 64, 512, 1000)
        );
        assert_eq!((f.ring_mix, f.ring_freq, f.reverb, f.reverb_type), (200, 100, 200, 0));
        assert_eq!((f.ping_pong, f.remove_dc, f.normalize, f.polarity, f.loop_follows_reverse), (true, true, true, true, true));
        assert_eq!((f.crossfade, f.trim, f.start(), f.length()), (512, 300, 0.25, 0.5));
        // Written back unchanged: the reader takes nothing the writer would rewrite.
        let mut again = FlpFile::parse(&bytes).unwrap();
        apply(&p, &mut again).unwrap();
        assert_eq!(again.to_bytes().unwrap(), bytes);
        // A short event 215 (older files) keeps the defaults past its end.
        let mut short = FlpFile::parse(&bytes).unwrap();
        {
            let ev = events_of_mut(&mut short).unwrap();
            let e = ev.iter_mut().find(|e| e.id == id::CH_SAMPLE_SETTINGS).unwrap();
            *e = Event::new(id::CH_SAMPLE_SETTINGS, Value::var(vec![0u8; 112]));
        }
        let f = load(&short).unwrap().channels[0].sound.fx;
        let d = SampleFx::default();
        assert_eq!((f.crossfade, f.trim, f.start_bits, f.length_bits, f.remove_dc), (d.crossfade, d.trim, d.start_bits, d.length_bits, false));
        assert_eq!(f.boost, 64);
    }

    /// A layer's flags (event 144) and crossfade position (event 215, int at byte 76) are read (spec:
    /// audio/note-rules.md "Settings"); without them a channel has flags 0.
    #[test]
    fn layer_settings_are_read() {
        let mut file = create(&new_project()).unwrap();
        {
            let ev = events_of_mut(&mut file).unwrap();
            let ch0 = ev.iter().position(|e| e.id == id::CH_START).unwrap();
            ev.retain(|e| e.id != id::CH_SAMPLE_SETTINGS);
            let mut s215 = vec![0u8; 168];
            s215[76..80].copy_from_slice(&160i32.to_le_bytes());
            ev.insert(ch0 + 1, Event::new(id::CH_LAYER_FLAGS, Value::U32(6)));
            ev.insert(ch0 + 1, Event::new(id::CH_SAMPLE_SETTINGS, Value::var(s215)));
        }
        let p = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        let l = p.channels[0].misc.layer;
        assert_eq!((l.flags, l.crossfade), (os_model::LayerSettings::CROSSFADE | os_model::LayerSettings::IN_TURN, 160));
        let plain = load(&create(&new_project()).unwrap()).unwrap();
        assert_eq!(plain.channels[0].misc.layer.flags, 0);
    }

    /// A Misc tab with every value away from its default (values the file's units hold exactly).
    fn edited_misc() -> ChannelMisc {
        ChannelMisc {
            levels: [-0.5, 1.0, 0.5, -1.0],
            max_poly: 3,
            slide: 820.0 / 1024.0,
            porta: true,
            mono: true,
            gate: 499.0 / 997.0,
            shift: 0.25,
            swing: 0.5,
            cut: 7,
            cut_by: 9,
            root: 48,
            main_pitch: false,
            add_key: true,
            fine: -35,
            layer: os_model::LayerSettings { flags: os_model::LayerSettings::CROSSFADE, crossfade: 160 },
            tracking: [
                os_model::Tracking { mid: 80, pan: 0.5, mod_x: -0.25, mod_y: 1.0 },
                os_model::Tracking { mid: 72, pan: -1.0, mod_x: 0.0, mod_y: 0.5 },
            ],
            arp: os_model::Arpeggio { direction: 4, time: 600.0 / 997.0, gate: 0.625, range: 0, repeat: 3, slide: true, chord: 7 },
            echo: os_model::Echo {
                feed: 0.375,
                time: 0.125,
                pan: 0.25,
                mod_x: -0.5,
                mod_y: 0.25,
                pitch: 0.25,
                echoes: 7,
                ping_pong: true,
                fat: true,
            },
            ..ChannelMisc::default()
        }
    }

    /// The Misc tab's values are written for every kind of channel (spec: audio/channel-misc.md "Settings",
    /// audio/voice-levels.md, audio/swing.md, audio/note-rules.md, flp/events/channels.md): events a block lacks are
    /// added, those it has are patched in place, and an untouched value adds nothing.
    #[test]
    fn misc_settings_are_written_for_every_channel_kind() {
        let mut p = new_project();
        for (index, kind) in [(2, ChannelKind::Plugin), (3, ChannelKind::Layer), (4, ChannelKind::AudioClip)] {
            let mut ch = sampler(index, "Other", 1);
            ch.kind = kind;
            if kind == ChannelKind::Plugin {
                (ch.plugin, ch.plugin_state) = (Some(crate::wrapper::WRAPPER.into()), Some(vec![10, 0, 0, 0]));
            }
            p.channels.push(ch);
        }
        let mut file = create(&p).unwrap();
        let fresh = load(&file).unwrap();
        // Untouched: nothing is added.
        let bytes = file.to_bytes().unwrap();
        apply(&fresh, &mut file).unwrap();
        assert_eq!(file.to_bytes().unwrap(), bytes);

        // Added where the block lacks them (only the audio clip has an event 215).
        let mut edited = fresh.clone();
        for ch in edited.channels.iter_mut().skip(1) {
            ch.misc = edited_misc();
        }
        apply(&edited, &mut file).unwrap();
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back.channels[0].misc, fresh.channels[0].misc, "an untouched channel");
        for (ch, want) in back.channels.iter().zip(&edited.channels).skip(1) {
            assert_eq!(ch.misc, want.misc, "{:?}", ch.kind);
            // An added event 215 changes nothing else the reader takes from it.
            assert_eq!(ch.sound, want.sound, "{:?}", ch.kind);
        }
        let count = |file: &FlpFile| events_of(file).unwrap().len();
        let added = count(&file);

        // Patched in place: no event is added, and the bits of event 221's byte 8 beyond mono and portamento stay.
        {
            let ev = events_of_mut(&mut file).unwrap();
            let poly = ev.iter_mut().filter(|e| e.id == id::CH_POLY).last().unwrap();
            let mut d = poly.data().unwrap().to_vec();
            d[8] |= 0x10;
            *poly = Event::new(id::CH_POLY, Value::var(d));
        }
        let mut again = load(&file).unwrap();
        for ch in again.channels.iter_mut().skip(1) {
            let m = &mut ch.misc;
            (m.levels, m.max_poly, m.slide, m.mono, m.porta) = ([0.25, -0.5, -0.25, 0.5], 0, 300.0 / 1024.0, false, true);
            (m.gate, m.shift, m.swing, m.cut, m.cut_by) = (1.0, 0.0, 1.0, 1, 0);
            (m.root, m.fine, m.main_pitch, m.add_key) = (72, 40, true, false);
            m.layer = os_model::LayerSettings { flags: os_model::LayerSettings::RANDOM, crossfade: 0 };
            m.tracking = [os_model::Tracking { mid: 100, ..m.tracking[0] }, os_model::Tracking { pan: 0.25, ..m.tracking[1] }];
            m.arp = os_model::Arpeggio { direction: 3, chord: -1, repeat: 1, ..m.arp };
            m.echo = os_model::Echo { feed: 0.5, mod_y: 0.0, fat: false, ..m.echo };
        }
        apply(&again, &mut file).unwrap();
        assert_eq!(count(&file), added);
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        for (ch, want) in back.channels.iter().zip(&again.channels).skip(1) {
            assert_eq!(ch.misc, want.misc, "{:?}", ch.kind);
        }
        let d = events_of(&file).unwrap().iter().filter(|e| e.id == id::CH_POLY).last().unwrap().data().unwrap().to_vec();
        assert_eq!(d[8], 0x10 | 2);
    }

    /// The arpeggiator, the echo delay and tracking are read in the units of audio/channel-misc.md "Settings": event 215
    /// (byte 10 echo flags, ints at 40..56 and 92, byte 60), 209 (five ints), 138 (mod Y low word, mod X high word) and
    /// the two 228 records (by volume, then by key). Written back, only the changed values move.
    #[test]
    fn arpeggiator_echo_and_tracking_are_read_and_patched_in_place() {
        let mut file = create(&new_project()).unwrap();
        let ints = |v: &[i32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        {
            let ev = events_of_mut(&mut file).unwrap();
            let ch0 = ev.iter().position(|e| e.id == id::CH_START).unwrap();
            let mut s215 = audio_clip_215([0; 4]);
            s215[10] = 0x01 | 2 | 4;
            for (at, v) in [(40, 3), (44, 0), (48, 7), (52, 1447), (56, 96), (92, 2)] {
                s215[at..at + 4].copy_from_slice(&i32::to_le_bytes(v));
            }
            s215[60] = 1;
            let add = [
                Event::new(id::CH_ECHO, Value::var(ints(&[12800, 3200, -300, 10, 49]))),
                Event::new(id::CH_ECHO_MOD, Value::U32(64 | 192 << 16)),
                Event::new(id::CH_TRACKING, Value::var(ints(&[80, 256, -128, 64]))),
                Event::new(id::CH_TRACKING, Value::var(ints(&[72, -256, 0, 128]))),
                Event::new(id::CH_SAMPLE_SETTINGS, Value::var(s215)),
            ];
            for (k, e) in add.into_iter().enumerate() {
                ev.insert(ch0 + 1 + k, e);
            }
        }
        let bytes = file.to_bytes().unwrap();
        let p = load(&FlpFile::parse(&bytes).unwrap()).unwrap();
        let m = p.channels[0].misc;
        let a = m.arp;
        assert_eq!((a.direction, a.range, a.chord, a.time, a.gate, a.slide, a.repeat), (3, 0, 7, 1.0, 1.0, true, 2));
        let e = m.echo;
        assert_eq!((e.feed, e.pan, e.pitch, e.echoes, e.time), (0.5, -0.25, -0.25, 10, 49.0 / 384.0));
        assert_eq!((e.mod_y, e.mod_x, e.ping_pong, e.fat), (-0.5, 0.5, true, true));
        assert_eq!(m.tracking[0], os_model::Tracking { mid: 80, pan: 1.0, mod_x: -0.5, mod_y: 0.25 });
        assert_eq!(m.tracking[1], os_model::Tracking { mid: 72, pan: -1.0, mod_x: 0.0, mod_y: 0.5 });
        // A channel without these events reads the spec's defaults.
        let d = load(&create(&new_project()).unwrap()).unwrap().channels[0].misc;
        assert_eq!((d.arp.direction, d.arp.range, d.arp.chord, d.arp.gate, d.arp.repeat, d.arp.slide), (0, 1, -1, 1.0, 1, false));
        assert_eq!((d.echo.feed, d.echo.pan, d.echo.echoes, d.echo.time * 384.0, d.echo.mod_x, d.echo.mod_y), (0.0, 0.0, 4, 144.0, 0.0, 0.0));
        assert_eq!((d.tracking[0].mid, d.tracking[1].mid), (100, 60));
        assert_eq!(gate_stored(d.arp.time), 1024);

        // Written back unchanged.
        let mut again = FlpFile::parse(&bytes).unwrap();
        apply(&p, &mut again).unwrap();
        assert_eq!(again.to_bytes().unwrap(), bytes);
        // One value each: echo time, fat mode, the key tracking's mod X, the chord. The arpeggio gate stored as 96
        // (beyond the knob's 48) and byte 10's other bit stay.
        let mut q = p.clone();
        let m = &mut q.channels[0].misc;
        (m.echo.time, m.echo.fat, m.tracking[1].mod_x) = (96.0 / 384.0, false, 0.5);
        m.arp.chord = 0;
        apply(&q, &mut again).unwrap();
        let ev = events_of(&again).unwrap();
        assert_eq!(ev.len(), events_of(&FlpFile::parse(&bytes).unwrap()).unwrap().len());
        let data = |id: u8, n: usize| ev.iter().filter(|e| e.id == id).nth(n).unwrap().data().unwrap().to_vec();
        assert_eq!(data(id::CH_ECHO, 0), ints(&[12800, 3200, -300, 10, 96]));
        assert_eq!(data(id::CH_TRACKING, 0), ints(&[80, 256, -128, 64]));
        assert_eq!(data(id::CH_TRACKING, 1), ints(&[72, -256, 128, 128]));
        let s = data(id::CH_SAMPLE_SETTINGS, 0);
        assert_eq!((s[10], &s[48..52], &s[56..60]), (0x01 | 2, &0i32.to_le_bytes()[..], &96i32.to_le_bytes()[..]));
        assert_eq!(load(&again).unwrap().channels[0].misc, q.channels[0].misc);
    }

    /// Tracking by key changed on a block without event 228: both records are added, the first with the read
    /// defaults (mid volume 100), so the second stays the key's.
    #[test]
    fn key_tracking_alone_adds_both_tracking_records() {
        let ints = |v: &[i32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
        let mut file = create(&new_project()).unwrap();
        let mut p = load(&file).unwrap();
        p.channels[1].misc.tracking[1].pan = 0.5;
        apply(&p, &mut file).unwrap();
        let t: Vec<Vec<u8>> = events_of(&file).unwrap().iter().filter(|e| e.id == id::CH_TRACKING).filter_map(Event::data).map(<[u8]>::to_vec).collect();
        assert_eq!(t, [ints(&[100, 0, 0, 0]), ints(&[60, 128, 0, 0])]);
        let back = load(&file).unwrap();
        assert_eq!((back.channels[0].misc, back.channels[1].misc), (ChannelMisc::default(), p.channels[1].misc));
        // Only tracking by volume: one record.
        let mut file = create(&new_project()).unwrap();
        let mut p = load(&file).unwrap();
        p.channels[1].misc.tracking[0].mid = 90;
        apply(&p, &mut file).unwrap();
        assert_eq!(events_of(&file).unwrap().iter().filter(|e| e.id == id::CH_TRACKING).count(), 1);
        assert_eq!(load(&file).unwrap().channels[1].misc, p.channels[1].misc);
    }

    /// A value changed alone adds or patches its own event only (spec: audio/swing.md: the swing share is event 97).
    #[test]
    fn a_misc_value_changed_alone_touches_its_own_event() {
        let mut file = create(&new_project()).unwrap();
        let before: Vec<u8> = events_of(&file).unwrap().iter().map(|e| e.id).collect();
        let mut p = load(&file).unwrap();
        p.channels[1].misc.swing = 0.25;
        apply(&p, &mut file).unwrap();
        let after: Vec<u8> = events_of(&file).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(after.len(), before.len() + 1);
        let ev = events_of(&file).unwrap();
        let swing: Vec<&Event> = ev.iter().filter(|e| e.id == id::CH_SWING_SHARE).collect();
        assert_eq!(swing.len(), 1);
        assert_eq!(int(swing[0]), 32);
        // It goes into channel 1's block, before its end event.
        let starts: Vec<usize> = ev.iter().enumerate().filter(|(_, e)| e.id == id::CH_START).map(|(i, _)| i).collect();
        let at = ev.iter().position(|e| e.id == id::CH_SWING_SHARE).unwrap();
        assert!(at > starts[1] && ev[at + 1].id == id::CH_END);
        let back = load(&file).unwrap();
        assert_eq!((back.channels[0].misc, back.channels[1].misc.swing), (ChannelMisc::default(), 0.25));
    }

    /// Track switches (event 236 flags), the main volume (record 0 at 0x4000) and the main pitch (event 80) are
    /// written by `create` and `apply` and read back (spec: flp/layouts.md, audio/mixer.md, audio/voice-levels.md).
    #[test]
    fn mixer_switches_main_volume_and_pitch_round_trip() {
        use os_model::MixerTrack;
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        p.mixer[1].polarity = true;
        p.mixer[2].swap = true;
        p.mixer[3].slots_enabled = false;
        p.main_volume = Some(6400);
        p.main_pitch = Some(-300);
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!((back.mixer.clone(), back.main_volume, back.main_pitch), (p.mixer.clone(), Some(6400), Some(-300)));
        let flags = |f: &FlpFile, t: usize| events_of(f).unwrap().iter().filter(|e| e.id == id::MX_FLAGS).nth(t).unwrap().data().unwrap()[4];
        assert_eq!([flags(&file, 1), flags(&file, 2), flags(&file, 3), flags(&file, 4)], [0x4D, 0x4E, 0x48, 0x4C]);
        let mut edited = back.clone();
        edited.mixer[1].polarity = false;
        edited.mixer[3].slots_enabled = true;
        edited.mixer[4].swap = true;
        edited.mixer[4].enabled = false;
        edited.main_volume = Some(16000);
        edited.main_pitch = Some(1200);
        apply(&edited, &mut file).unwrap();
        let again = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!((again.mixer, again.main_volume, again.main_pitch), (edited.mixer, Some(16000), Some(1200)));
        // A file without event 80 gets it where FL writes it once the main pitch is set: behind the tempo and the current
        // pattern (and a swing) the file has.
        let mut plain = create(&new_project()).unwrap();
        let mut q = load(&plain).unwrap();
        assert_eq!((q.main_pitch, q.main_volume), (None, None));
        q.main_pitch = Some(250);
        apply(&q, &mut plain).unwrap();
        let ev = events_of(&plain).unwrap();
        let at = ev.iter().position(|e| e.id == id::MAIN_PITCH).unwrap();
        assert_eq!([ev[at - 2].id, ev[at - 1].id], [id::TEMPO, id::CURRENT_PATTERN]);
        assert_eq!(load(&plain).unwrap().main_pitch, Some(250));
    }

    /// Song or pattern mode (event 9, one byte right after 67; spec: flp/engine-events.md "Project"): `create` writes it
    /// where the model has it, `load` reads it (`None` without it), `apply` patches it in place.
    #[test]
    fn song_mode_round_trip() {
        let reparse = |f: &FlpFile| FlpFile::parse(&f.to_bytes().unwrap()).unwrap();
        assert_eq!(load(&create(&new_project()).unwrap()).unwrap().song_mode, None);
        for on in [false, true] {
            let mut file = reparse(&create(&Project { song_mode: Some(on), ..new_project() }).unwrap());
            let ev = events_of(&file).unwrap();
            let at = ev.iter().position(|e| e.id == id::SONG_MODE).unwrap();
            assert_eq!((ev[at - 1].id, ev[at].value.clone()), (id::CURRENT_PATTERN, Value::U8(u8::from(on))));
            let mut q = load(&file).unwrap();
            assert_eq!(q.song_mode, Some(on));
            let before = file.to_bytes().unwrap();
            apply(&q, &mut file).unwrap();
            assert_eq!(file.to_bytes().unwrap(), before, "unchanged");
            q.song_mode = Some(!on);
            apply(&q, &mut file).unwrap();
            let file = reparse(&file);
            let ev = events_of(&file).unwrap();
            assert_eq!(ev.iter().filter(|e| e.id == id::SONG_MODE).count(), 1);
            assert_eq!(ev[at].id, id::SONG_MODE, "in place");
            assert_eq!(load(&file).unwrap().song_mode, Some(!on));
        }
    }

    /// The initial values (event 216; spec: ui/settings-choices.md "Advanced: Initialize controls automatically",
    /// flp/engine-events.md "Event 216"): `create` writes the event in every project, empty without values; the records
    /// read back, one Lemon Studio doesn't understand (a plugin parameter's) byte for byte; they are not read into the
    /// stored settings (event 156 stays the tempo, event 225 a slot's switch and mix level: the app sets the initial
    /// values on opening in song mode); `apply` rewrites the list in place and adds the event after 146 to a file
    /// without it.
    #[test]
    fn initial_values_round_trip() {
        use os_model::target;
        use os_model::MixerTrack;
        // A fresh project: event 216 of length 0.
        let file = create(&new_project()).unwrap();
        assert_eq!(event_data(&file, id::INIT_CONTROLS), Some(vec![]));
        assert!(load(&file).unwrap().init_controls.is_empty());

        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        p.mixer[2].slots[3] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Limiter".into(), state: vec![] });
        p.mixer[2].slot_mix[3] = 0.25;
        p.groups = vec!["Unsorted".into()];
        let plugin = InitControl { head: 0x0102_0304, target: target::channel_param(1, 7), value: -123_456 };
        p.init_controls = vec![
            InitControl { head: 0, target: target::channel(0, target::CHANNEL_VOLUME), value: 10000 },
            plugin,
            InitControl { head: 0, target: target::TEMPO, value: 99_500 },
            InitControl { head: 0, target: target::slot(2, 3, target::SLOT_ON), value: 0 },
            InitControl { head: 0, target: target::slot(2, 3, target::SLOT_MIX), value: 9600 },
        ];
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let ev = events_of(&file).unwrap();
        let at = ev.iter().position(|e| e.id == id::INIT_CONTROLS).unwrap();
        assert_eq!(ev[at - 1].id, id::CURRENT_GROUP, "after 146");
        let data = ev[at].data().unwrap();
        assert_eq!(data.len(), 5 * 12);
        assert_eq!(data[0..12], [0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0x27, 0, 0], "channel 0's volume at 10000");
        assert_eq!(data[12..24], plugin.to_bytes());
        let back = load(&file).unwrap();
        assert_eq!(back.init_controls, p.init_controls);
        // Event 156 holds 128.5 and stays the tempo; the slot keeps its stored switch and mix level.
        assert_eq!(int(&ev[ev.iter().position(|e| e.id == id::TEMPO).unwrap()]), 128_500);
        assert_eq!(back.tempo, Some(128.5));
        assert_eq!((back.mixer[2].slot_on[3], back.mixer[2].slot_mix[3]), (true, 0.25));

        // Unchanged, the file stays as it is; a changed list is written in place, unknown records kept.
        let before = file.to_bytes().unwrap();
        apply(&back, &mut file).unwrap();
        assert_eq!(file.to_bytes().unwrap(), before);
        let mut edited = back.clone();
        edited.init_controls.retain(|c| c.target != target::TEMPO);
        edited.init_controls[0].value = 6400;
        // The values set on opening are saved as the stored settings, as FL saves them.
        edited.tempo = Some(99.5);
        edited.mixer[2].slot_on[3] = false;
        apply(&edited, &mut file).unwrap();
        let again = load(&file).unwrap();
        assert_eq!(again.init_controls, edited.init_controls);
        assert_eq!(again.init_controls[1].to_bytes(), plugin.to_bytes());
        assert_eq!((again.tempo, again.mixer[2].slot_on[3]), (Some(99.5), false));
        edited.init_controls.clear();
        apply(&edited, &mut file).unwrap();
        assert_eq!(event_data(&file, id::INIT_CONTROLS), Some(vec![]), "an empty list keeps the event, length 0");

        // A file without event 216 gets it after 146 once there is a value.
        let mut plain = create(&p).unwrap();
        events_of_mut(&mut plain).unwrap().retain(|e| e.id != id::INIT_CONTROLS);
        let mut q = load(&plain).unwrap();
        assert!(q.init_controls.is_empty());
        apply(&q, &mut plain).unwrap();
        assert_eq!(event_data(&plain, id::INIT_CONTROLS), None, "nothing to write");
        q.init_controls = vec![InitControl { head: 0, target: target::mixer(1, target::MIXER_VOLUME), value: 12800 }];
        apply(&q, &mut plain).unwrap();
        let ev = events_of(&plain).unwrap();
        let at = ev.iter().position(|e| e.id == id::INIT_CONTROLS).unwrap();
        assert_eq!(ev[at - 1].id, id::CURRENT_GROUP);
        assert_eq!(load(&plain).unwrap().init_controls, q.init_controls);
    }

    /// The bar (events 17, 18: beats per bar, steps per beat; 18 is not the denominator), its mode (35) and "play
    /// truncated notes" (30, present only when on) are saved (spec: flp/engine-events.md "Project", "The project
    /// settings, measured", "Event 30"): `create` writes them where the model has them; `apply` patches them in place in
    /// a file that has them (an opened project keeps every other event as it was) and removes 30 for off. Both write
    /// the project settings in FL 26's order ("Order in the file": 156, 67, 9, 11, 80, 17, 18, 35, 23, 44, 30, 10, then
    /// the information): `apply` adds a missing 11, 80, 17, 18, 35 or 30 behind the last of the settings before it
    /// that the file has. 40 is read and left as it is; 23 and 44 as long as the model's are the file's
    /// (`panning_law_round_trips`, `automatic_crossfades_round_trip`).
    #[test]
    fn time_settings_and_truncated_notes_round_trip() {
        const ORDER: [u8; 12] =
            [id::TEMPO, id::CURRENT_PATTERN, 9, id::SWING, id::MAIN_PITCH, id::TIME_NUM, id::TIME_DEN, id::TIME_MODE, 23, 44, id::PLAY_TRUNCATED, 10];
        let prelude_ids = |f: &FlpFile, keep: &[u8]| -> Vec<u8> {
            events_of(f).unwrap().iter().take_while(|e| e.id != id::CH_START).map(|e| e.id).filter(|i| keep.contains(i)).collect()
        };
        let reparse = |f: &FlpFile| FlpFile::parse(&f.to_bytes().unwrap()).unwrap();
        let value_of = |f: &FlpFile, eid: u8| events_of(f).unwrap().iter().take_while(|e| e.id != id::CH_START).find(|e| e.id == eid).map(int);
        // What Lemon Studio writes, in FL's order: the settings the model has, then the title.
        let lemon = [
            id::TEMPO,
            id::CURRENT_PATTERN,
            id::SWING,
            id::MAIN_PITCH,
            id::TIME_NUM,
            id::TIME_DEN,
            id::TIME_MODE,
            id::PLAY_TRUNCATED,
            id::TITLE,
        ];
        // A new project in 3/8: 17 = 3, 18 = 16 / 8 = 2.
        let mut p = new_project();
        (p.bar, p.bar_as_signature, p.play_truncated, p.swing, p.main_pitch) = (Some((3, 2)), Some(true), true, Some(64), Some(100));
        let file = reparse(&create(&p).unwrap());
        let back = load(&file).unwrap();
        assert_eq!((back.bar, back.bar_as_signature, back.play_truncated), (Some((3, 2)), Some(true), true));
        assert_eq!((back.signature(), back.bar_ticks()), (Some((3, 8)), 144));
        assert_eq!(prelude_ids(&file, &lemon), lemon);
        assert_eq!((value_of(&file, id::TIME_NUM), value_of(&file, id::TIME_DEN), value_of(&file, id::PLAY_TRUNCATED)), (Some(3), Some(2), Some(1)));
        // Off, event 30 is not written at all; a time division of 3 steps per beat keeps its values.
        let mut off = back.clone();
        (off.bar, off.bar_as_signature, off.play_truncated) = (Some((5, 3)), Some(false), false);
        let file = reparse(&create(&off).unwrap());
        assert_eq!(value_of(&file, id::PLAY_TRUNCATED), None);
        let back = load(&file).unwrap();
        assert_eq!((back.bar, back.bar_as_signature, back.play_truncated), (Some((5, 3)), Some(false), false));
        assert_eq!((back.signature(), back.bar_ticks()), (None, 360), "five beats of three sixteenths");

        // A file laid out as FL 26 writes one: its settings behind the tempo in FL's order, those the model does not
        // read among them, without the ids in `missing`. Triangular panning (23 = 2), automatic crossfades off (44 = 0).
        let fl_file = |missing: &[u8]| -> FlpFile {
            let mut f = create(&Project { current_pattern: None, ..new_project() }).unwrap();
            let ev = events_of_mut(&mut f).unwrap();
            let t = ev.iter().position(|e| e.id == id::TEMPO).unwrap();
            let value = |eid: u8| match eid {
                id::TIME_NUM | id::TIME_DEN => 4,
                id::SWING => 64,
                23 => 2,
                44 => 0,
                _ => 1,
            };
            for (k, eid) in ORDER[1..].iter().copied().filter(|e| !missing.contains(e)).enumerate() {
                ev.insert(t + 1 + k, int_event(eid, value(eid)));
            }
            reparse(&f)
        };
        let fl = fl_file(&[]);
        assert_eq!(prelude_ids(&fl, &ORDER), ORDER);
        let before = events_of(&fl).unwrap().clone();
        let opened = load(&fl).unwrap();
        assert_eq!((opened.bar, opened.bar_as_signature, opened.play_truncated), (Some((4, 4)), Some(true), true));
        assert_eq!((opened.panning_law, opened.auto_crossfades, opened.fast_declick), (Some(2), Some(false), None));
        let mut file = fl.clone();
        apply(&opened, &mut file).unwrap();
        assert_eq!(file.to_bytes().unwrap(), fl.to_bytes().unwrap(), "unchanged: byte for byte");
        // 7/8 and play truncated notes off: 17 and 18 change in place, 30 goes, nothing else moves.
        let mut edited = opened.clone();
        (edited.bar, edited.play_truncated) = (Some((7, 2)), false);
        apply(&edited, &mut file).unwrap();
        let file = reparse(&file);
        let after = events_of(&file).unwrap();
        let kept: Vec<&Event> = before.iter().filter(|e| e.id != id::PLAY_TRUNCATED).collect();
        assert_eq!(after.len(), kept.len(), "patched in place, 30 removed");
        for (a, b) in after.iter().zip(kept) {
            match a.id {
                id::TIME_NUM => assert_eq!(int(a), 7),
                id::TIME_DEN => assert_eq!(int(a), 2),
                _ => assert_eq!(a, b),
            }
        }
        let again = load(&file).unwrap();
        assert_eq!((again.bar, again.signature(), again.play_truncated), (Some((7, 2)), Some((7, 8)), false));
        assert_eq!((again.panning_law, again.auto_crossfades), (Some(2), Some(false)), "23 and 44 as they were");
        // On again: 30 = 1 back in its place. A time division of 3 / 3: 35 = 0 in place, 18 = 3.
        let mut file = file.clone();
        let mut on = again.clone();
        (on.play_truncated, on.bar, on.bar_as_signature) = (true, Some((3, 3)), Some(false));
        apply(&on, &mut file).unwrap();
        let file = reparse(&file);
        assert_eq!(prelude_ids(&file, &ORDER), ORDER);
        assert_eq!((value_of(&file, id::PLAY_TRUNCATED), value_of(&file, id::TIME_MODE), value_of(&file, id::TIME_DEN)), (Some(1), Some(0), Some(3)));
        let back = load(&file).unwrap();
        assert_eq!((back.bar, back.bar_as_signature, back.play_truncated, back.bar_ticks()), (Some((3, 3)), Some(false), true, 216));

        // Missing from such a file, they are added where FL writes them: 9 behind 67 (or 156), 11 behind 9 (or 67), 80 behind 11, 17 and 18
        // behind 80, 35 behind 18 (before 23, 44), 30 behind 44 (before 10), or behind 35 when the file has no 23, 44.
        let without = |gone: &[u8]| -> Vec<u8> { ORDER.iter().copied().filter(|e| !gone.contains(e)).collect() };
        let cases: [&[u8]; 9] = [
            &[id::TIME_NUM, id::TIME_DEN, id::TIME_MODE, id::PLAY_TRUNCATED],
            &[id::TIME_NUM, id::TIME_DEN],
            &[id::TIME_MODE],
            &[id::PLAY_TRUNCATED],
            &[id::TIME_NUM, id::TIME_DEN, id::TIME_MODE, 23, 44, id::PLAY_TRUNCATED],
            &[id::SWING, id::MAIN_PITCH],
            &[id::SWING, id::MAIN_PITCH, id::TIME_NUM, id::TIME_DEN, id::PLAY_TRUNCATED],
            &[9, id::SWING],
            &[id::CURRENT_PATTERN, 9, id::SWING, id::MAIN_PITCH, id::TIME_NUM, id::TIME_DEN, id::TIME_MODE, id::PLAY_TRUNCATED],
        ];
        for missing in cases {
            let mut file = fl_file(missing);
            let mut q = load(&file).unwrap();
            assert_eq!(q.song_mode, (!missing.contains(&9)).then_some(true));
            (q.bar, q.bar_as_signature, q.play_truncated, q.swing, q.main_pitch) = (Some((6, 2)), Some(true), true, Some(64), Some(1));
            // Pattern mode: 9 patched to 0, or added behind 156 and 67.
            q.song_mode = Some(false);
            apply(&q, &mut file).unwrap();
            let file = reparse(&file);
            let gone: Vec<u8> = missing.iter().copied().filter(|e| [id::CURRENT_PATTERN, 23, 44].contains(e)).collect();
            assert_eq!(prelude_ids(&file, &ORDER), without(&gone), "missing {missing:?}");
            assert_eq!(value_of(&file, id::SONG_MODE), Some(0));
            let back = load(&file).unwrap();
            assert_eq!(
                (back.bar, back.bar_as_signature, back.play_truncated, back.swing, back.main_pitch, back.song_mode),
                (Some((6, 2)), Some(true), true, Some(64), Some(1), Some(false))
            );
        }
        // A swing or main pitch added to a file with 17 and 18 ahead of the current pattern (as Lemon Studio wrote
        // them for a while) stays before them.
        let mut file = fl_file(&[9, id::SWING, id::MAIN_PITCH, id::TIME_MODE, 23, 44, id::PLAY_TRUNCATED, 10]);
        {
            let ev = events_of_mut(&mut file).unwrap();
            let c = ev.iter().position(|e| e.id == id::CURRENT_PATTERN).unwrap();
            let e = ev.remove(c);
            ev.insert(c + 2, e);
        }
        let mut file = reparse(&file);
        let mut q = load(&file).unwrap();
        (q.swing, q.main_pitch) = (Some(32), Some(50));
        apply(&q, &mut file).unwrap();
        let file = reparse(&file);
        assert_eq!(prelude_ids(&file, &ORDER), [id::TEMPO, id::SWING, id::MAIN_PITCH, id::TIME_NUM, id::TIME_DEN, id::CURRENT_PATTERN]);
        assert_eq!((load(&file).unwrap().swing, load(&file).unwrap().main_pitch), (Some(32), Some(50)));

        // Lemon Studio files without them: settings added in one save keep FL's order (11, 80, 17, 18, 35, 30) behind
        // the current pattern, before the title, also when the title is added in the same save.
        let mut p = new_project();
        (p.swing, p.main_pitch) = (Some(64), Some(100));
        for start in [new_project(), p, Project { current_pattern: None, info: ProjectInfo::default(), ..new_project() }] {
            let mut file = create(&start).unwrap();
            let mut q = load(&file).unwrap();
            assert_eq!((q.bar, q.bar_as_signature, q.play_truncated), (None, None, false));
            (q.bar, q.bar_as_signature, q.play_truncated, q.swing, q.main_pitch) = (Some((5, 4)), Some(false), true, Some(32), Some(-100));
            q.info.title = Some("Song".into());
            apply(&q, &mut file).unwrap();
            let file = reparse(&file);
            let want: Vec<u8> = lemon.iter().copied().filter(|&e| e != id::CURRENT_PATTERN || start.current_pattern.is_some()).collect();
            assert_eq!(prelude_ids(&file, &lemon), want);
            let back = load(&file).unwrap();
            assert_eq!(
                (back.bar, back.bar_as_signature, back.play_truncated, back.swing, back.main_pitch),
                (Some((5, 4)), Some(false), true, Some(32), Some(-100))
            );
            assert_eq!(back.info.title.as_deref(), Some("Song"));
        }

        // Fast declick for cut groups (event 40) stands after event 100: read there, kept through a save.
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        (p.current_arrangement, p.fast_declick) = (Some(0), Some(true));
        let mut file = reparse(&create(&p).unwrap());
        let ev = events_of(&file).unwrap();
        let at = ev.iter().position(|e| e.id == id::CURRENT_ARRANGEMENT).unwrap();
        assert_eq!((ev[at + 1].id, int(&ev[at + 1])), (id::FAST_DECLICK, 1));
        let mut q = load(&file).unwrap();
        assert_eq!(q.fast_declick, Some(true));
        q.tempo = Some(100.0);
        apply(&q, &mut file).unwrap();
        assert_eq!(load(&reparse(&file)).unwrap().fast_declick, Some(true));
    }

    /// Settings > Project ▸ Advanced ▸ Panning law (event 23: 0 Circular, 2 Triangular; spec: flp/engine-events.md
    /// "Project", "Order in the file"): `create` writes the model's value in FL's order; `apply` patches it where the
    /// file has it, else adds it behind the last of 156, 67, 9, 11, 80, 17, 18 and 35 the file has (before 44, 30 and
    /// 10). Circular is written as 0, as FL's default template holds it; a model without one leaves the file as it is.
    #[test]
    fn panning_law_round_trips() {
        const ORDER: [u8; 11] =
            [id::TEMPO, id::CURRENT_PATTERN, id::SWING, id::MAIN_PITCH, id::TIME_NUM, id::TIME_DEN, id::TIME_MODE, id::PANNING_LAW, id::AUTO_CROSSFADES, id::PLAY_TRUNCATED, id::TITLE];
        let reparse = |f: &FlpFile| FlpFile::parse(&f.to_bytes().unwrap()).unwrap();
        let prelude = |f: &FlpFile| -> Vec<(u8, u32)> {
            events_of(f).unwrap().iter().take_while(|e| e.id != id::CH_START).filter(|e| ORDER.contains(&e.id)).map(|e| (e.id, int(e))).collect()
        };
        let ids = |f: &FlpFile| -> Vec<u8> { prelude(f).into_iter().map(|e| e.0).collect() };
        let law_of = |f: &FlpFile| prelude(f).into_iter().find(|e| e.0 == id::PANNING_LAW).map(|e| e.1);
        let mut p = new_project();
        (p.bar, p.bar_as_signature, p.play_truncated, p.auto_crossfades) = (Some((4, 4)), Some(true), true, Some(true));
        // Written by `create` behind 35, before 44 and 30.
        p.panning_law = Some(2);
        let file = reparse(&create(&p).unwrap());
        assert_eq!(law_of(&file), Some(2));
        let full = ids(&file);
        let at = |eid: u8| full.iter().position(|&e| e == eid).unwrap();
        assert!(at(id::TIME_MODE) + 1 == at(id::PANNING_LAW) && at(id::PANNING_LAW) + 1 == at(id::AUTO_CROSSFADES), "{full:?}");
        assert_eq!(load(&file).unwrap().panning_law, Some(2));
        // Patched in place: back to Circular is 0, nothing else moves.
        let mut q = load(&file).unwrap();
        q.panning_law = Some(0);
        let mut patched = file.clone();
        apply(&q, &mut patched).unwrap();
        let patched = reparse(&patched);
        let (a, b) = (events_of(&file).unwrap(), events_of(&patched).unwrap());
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(b.iter()).all(|(x, y)| if x.id == id::PANNING_LAW { int(y) == 0 } else { x == y }));
        assert_eq!(load(&patched).unwrap().panning_law, Some(0));
        // Unchanged: the file stays byte for byte.
        let mut same = patched.clone();
        apply(&load(&patched).unwrap(), &mut same).unwrap();
        assert_eq!(same.to_bytes().unwrap(), patched.to_bytes().unwrap());
        // Added to a file without it: behind 35, or behind 18 when there is no 35, or behind the current pattern;
        // always before 44 and 30.
        for (mode, bar) in [(Some(true), Some((4, 4))), (None, Some((3, 2))), (None, None)] {
            let mut start = p.clone();
            (start.panning_law, start.bar_as_signature, start.bar) = (None, mode, bar);
            let mut file = reparse(&create(&start).unwrap());
            assert_eq!(law_of(&file), None);
            let mut q = load(&file).unwrap();
            q.panning_law = Some(2);
            apply(&q, &mut file).unwrap();
            let file = reparse(&file);
            let want: Vec<u8> = full
                .iter()
                .copied()
                .filter(|&e| (e != id::TIME_MODE || mode.is_some()) && (![id::TIME_NUM, id::TIME_DEN].contains(&e) || bar.is_some()))
                .collect();
            assert_eq!(ids(&file), want, "{mode:?} {bar:?}");
            assert_eq!(load(&file).unwrap().panning_law, Some(2));
        }
        // A model without one writes nothing.
        let mut start = p.clone();
        start.panning_law = None;
        let mut file = reparse(&create(&start).unwrap());
        let mut q = load(&file).unwrap();
        q.tempo = Some(99.0);
        apply(&q, &mut file).unwrap();
        assert_eq!(law_of(&reparse(&file)), None);
    }

    /// Settings > Project ▸ Playlist ▸ Automatic crossfades (event 44: 1 on, 0 off; spec: flp/engine-events.md "The
    /// project settings, measured"): `create` writes it behind 23, before 30; `apply` patches it where the file has it
    /// (off is 0, not a removal), else adds it behind the last of 156, 67, 9, 11, 80, 17, 18, 35 and 23 the file has; a
    /// model without one leaves the file as it is.
    #[test]
    fn automatic_crossfades_round_trip() {
        const ORDER: [u8; 6] = [id::TIME_NUM, id::TIME_DEN, id::TIME_MODE, id::PANNING_LAW, id::AUTO_CROSSFADES, id::PLAY_TRUNCATED];
        let reparse = |f: &FlpFile| FlpFile::parse(&f.to_bytes().unwrap()).unwrap();
        let prelude = |f: &FlpFile| -> Vec<(u8, u32)> {
            events_of(f).unwrap().iter().take_while(|e| e.id != id::CH_START).filter(|e| ORDER.contains(&e.id)).map(|e| (e.id, int(e))).collect()
        };
        let mut p = new_project();
        (p.bar, p.bar_as_signature, p.panning_law, p.auto_crossfades, p.play_truncated) = (Some((4, 4)), Some(true), Some(0), Some(true), true);
        let file = reparse(&create(&p).unwrap());
        assert_eq!(prelude(&file), [(id::TIME_NUM, 4), (id::TIME_DEN, 4), (id::TIME_MODE, 1), (id::PANNING_LAW, 0), (id::AUTO_CROSSFADES, 1), (id::PLAY_TRUNCATED, 1)]);
        // Off: 0 in place, nothing else moves; unchanged, byte for byte.
        let mut q = load(&file).unwrap();
        assert_eq!(q.auto_crossfades, Some(true));
        q.auto_crossfades = Some(false);
        let mut off = file.clone();
        apply(&q, &mut off).unwrap();
        let off = reparse(&off);
        let (a, b) = (events_of(&file).unwrap(), events_of(&off).unwrap());
        assert!(a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| if x.id == id::AUTO_CROSSFADES { int(y) == 0 } else { x == y }));
        assert_eq!(load(&off).unwrap().auto_crossfades, Some(false));
        let mut same = off.clone();
        apply(&load(&off).unwrap(), &mut same).unwrap();
        assert_eq!(same.to_bytes().unwrap(), off.to_bytes().unwrap());
        // Added where it is missing: behind 23, or 35, or 18, always before 30.
        for (law, mode) in [(Some(0), Some(true)), (None, Some(true)), (None, None)] {
            let mut start = p.clone();
            (start.auto_crossfades, start.panning_law, start.bar_as_signature) = (None, law, mode);
            let mut file = reparse(&create(&start).unwrap());
            let mut q = load(&file).unwrap();
            assert_eq!(q.auto_crossfades, None);
            q.auto_crossfades = Some(true);
            apply(&q, &mut file).unwrap();
            let file = reparse(&file);
            let ids: Vec<u8> = prelude(&file).into_iter().map(|e| e.0).collect();
            let want: Vec<u8> = ORDER.iter().copied().filter(|&e| (e != id::PANNING_LAW || law.is_some()) && (e != id::TIME_MODE || mode.is_some())).collect();
            assert_eq!(ids, want, "{law:?} {mode:?}");
            assert_eq!(load(&file).unwrap().auto_crossfades, Some(true));
        }
        // A model without one writes nothing.
        let mut start = p.clone();
        start.auto_crossfades = None;
        let mut file = reparse(&create(&start).unwrap());
        let mut q = load(&file).unwrap();
        q.tempo = Some(99.0);
        apply(&q, &mut file).unwrap();
        assert!(!prelude(&reparse(&file)).iter().any(|e| e.0 == id::AUTO_CROSSFADES));
    }

    /// Settings > Project ▸ Timebase (PPQ): the header's PPQ and every tick count of the project change together;
    /// automation clip data (beats) does not (spec: flp/engine-events.md "The project settings, measured",
    /// ui/settings-choices.md "Project"). A ratio that is not whole rounds to the nearest tick.
    #[test]
    fn timebase_change_rescales_and_saves() {
        let mut p = new_project();
        p.patterns[0].notes = vec![Note { length: 50, ..Note::step(0, 48) }, Note { length: 1, ..Note::step(1, 7) }, Note::step(1, 0)];
        p.patterns[0].length = Some(384);
        p.patterns[0].markers = vec![TimeMarker::new(192, "Mid")];
        let mut arr = default_arrangement();
        let mut c = Clip::new(384, 288, 0, ClipSource::Pattern(2));
        c.window = Some((96.0, 384.0));
        arr.clips.push(c);
        arr.clips.push(Clip::new(0, 384, 1, ClipSource::Channel(2)));
        arr.markers.push(TimeMarker::new(768, "Verse"));
        p.arrangements.push(arr);
        let mut auto_ch = sampler(2, "Kick - Channel volume", 0);
        auto_ch.kind = ChannelKind::Automation;
        p.channels.push(auto_ch);
        p.automations.push(new_automation(2, os_model::target::channel(0, 0), 0.5, 4.0));
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let opened = load(&file).unwrap();

        let mut doubled = opened.clone();
        doubled.set_ppq(192);
        let n = &doubled.patterns.iter().find(|p| p.number == 2).unwrap();
        assert_eq!(n.notes.iter().map(|x| (x.position, x.length)).collect::<Vec<_>>(), [(0, 0), (14, 2), (96, 100)]);
        assert_eq!((n.length, n.markers[0].position, n.controls[0].position), (Some(768), 384, 0));
        let a = &doubled.arrangements[0];
        let pc = a.clips.iter().find(|c| c.source == ClipSource::Pattern(2)).unwrap();
        assert_eq!((pc.position, pc.length, pc.window), (768, 576, Some((192.0, 768.0))));
        let ac = a.clips.iter().find(|c| c.source == ClipSource::Channel(2)).unwrap();
        assert_eq!((ac.position, ac.length, ac.window), (0, 768, None));
        assert_eq!(a.markers[0].position, 1536);
        assert_eq!(doubled.automations, opened.automations, "automation data stays");

        apply(&doubled, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        assert_eq!(file.header().unwrap().ppq(), Some(192));
        let back = load(&file).unwrap();
        assert_eq!(back.ppq, 192);
        assert_eq!(back.patterns, doubled.patterns);
        assert_eq!(back.automations, opened.automations);
        let strip = |v: &[Clip]| {
            let mut v: Vec<_> = v.iter().map(|c| (c.position, c.length, c.track, c.source, c.window)).collect();
            v.sort_by_key(|c| (c.0, c.2));
            v
        };
        assert_eq!(strip(&back.arrangements[0].clips), strip(&doubled.arrangements[0].clips));
        assert_eq!(back.arrangements[0].markers, doubled.arrangements[0].markers);

        // 96 to 72: three quarters, to the nearest tick; a length that was not 0 stays at least 1.
        let mut q = opened.clone();
        q.set_ppq(72);
        let n = &q.patterns.iter().find(|p| p.number == 2).unwrap();
        assert_eq!(n.notes.iter().map(|x| (x.position, x.length)).collect::<Vec<_>>(), [(0, 0), (5, 1), (36, 38)]);
        // And back: whole ratios are exact.
        let mut r = doubled.clone();
        r.set_ppq(96);
        assert_eq!((r.patterns.clone(), r.arrangements[0].markers.clone()), (opened.patterns.clone(), opened.arrangements[0].markers.clone()));
    }

    /// A 5-byte track record (FL 10 / 11) holds no flags: the track reads as on, unswapped, slots enabled; an edit turns
    /// it into the 8-byte record with the latency value and its 0x20 byte kept (spec: flp/layouts.md).
    #[test]
    fn five_byte_track_records() {
        use os_model::MixerTrack;
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        let mut file = create(&p).unwrap();
        let old = [0x00, 0x00, 0xa0, 0x40, 0x01];
        {
            let ev = events_of_mut(&mut file).unwrap();
            let e = ev.iter_mut().filter(|e| e.id == id::MX_FLAGS).nth(2).unwrap();
            *e = Event::new(id::MX_FLAGS, Value::var(old.to_vec()));
        }
        let back = load(&file).unwrap();
        let t = &back.mixer[2];
        assert_eq!((t.enabled, t.polarity, t.swap, t.slots_enabled), (true, false, false, true));
        assert_eq!((t.latency_out, t.latency_in), (-5.0, 0.0));
        let mut edited = back.clone();
        edited.mixer[2].polarity = true;
        apply(&edited, &mut file).unwrap();
        let ev = events_of(&file).unwrap();
        let d = ev.iter().filter(|e| e.id == id::MX_FLAGS).nth(2).unwrap().data().unwrap();
        // The file says it is from a later FL: the latency is written in that version's form, the output setting as is.
        assert_eq!(d, &[0x00, 0x00, 0xa0, 0xc0, 0x4C | 0x01, 0, 0, 0]);
        assert_eq!(load(&file).unwrap().mixer, edited.mixer);
        // In a file from before FL 12.8 the record keeps its one value, flag 0x20 standing for the fifth byte.
        let mut r = vec![0u8; 8];
        r[4] = 0x4C | 0x20;
        assert_eq!(with_latency(&r, true, (-5.0, 0.0))[..5], [0x00, 0x00, 0xa0, 0x40, 0x4C | 0x20]);
        r[4] = 0x4C;
        assert_eq!(with_latency(&r, true, (0.0, 0.0))[4], 0x4C);
    }

    /// The two latency settings (spec: flp/layouts.md "A mixer track's record (event 236)"): written with new tracks,
    /// read back, and written again where an edit changes them.
    #[test]
    fn track_latency_settings_round_trip() {
        use os_model::MixerTrack;
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        (p.mixer[3].latency_out, p.mixer[3].latency_in) = (-3.49, 5.0);
        let mut file = create(&p).unwrap();
        let back = load(&file).unwrap();
        assert_eq!((back.mixer[3].latency_out, back.mixer[3].latency_in), (-3.49, 5.0));
        assert_eq!((back.mixer[2].latency_out, back.mixer[2].latency_in), (0.0, 0.0));
        let mut edited = back.clone();
        edited.mixer[3].latency_out = 0.0;
        (edited.mixer[4].latency_in, edited.mixer[4].polarity) = (-2.0, true);
        apply(&edited, &mut file).unwrap();
        assert_eq!(load(&file).unwrap().mixer, edited.mixer);
        let ev = events_of(&file).unwrap();
        let d = ev.iter().filter(|e| e.id == id::MX_FLAGS).nth(4).unwrap().data().unwrap();
        assert_eq!((&d[..4], d[4] & 0x01, &d[8..]), (&0f32.to_le_bytes()[..], 0x01, &(-2f32).to_le_bytes()[..]));
    }

    /// The record forms before FL 12.9: an 8-byte record of FL 12.8 or later holds the output setting; one written
    /// earlier, and a 5-byte record, hold one value with the other sign, the output setting when flag 0x20 (the
    /// 5-byte record's fifth byte) is set. Writing keeps the form where it holds the settings.
    #[test]
    fn older_track_latency_records() {
        let five = 5f32.to_le_bytes();
        let rec = |tail: &[u8]| [&five[..], tail].concat();
        assert_eq!(track_latency(&rec(&[0x2C, 0, 0, 0]), true), (-5.0, 0.0));
        assert_eq!(track_latency(&rec(&[0x0C, 0, 0, 0]), true), (0.0, -5.0));
        assert_eq!(track_latency(&rec(&[0x0C, 0, 0, 0]), false), (5.0, 0.0));
        assert_eq!(track_latency(&rec(&[1]), true), (-5.0, 0.0));
        assert_eq!(track_latency(&rec(&[0]), true), (0.0, -5.0));
        assert_eq!(track_latency(&rec(&[0x0C, 0, 0, 0, 0, 0, 0xA0, 0xC0]), true), (5.0, -5.0));
        // Written back.
        let old = rec(&[0x0C, 0, 0, 0]);
        assert_eq!(with_latency(&old, true, (-2.0, 0.0)), [&2f32.to_le_bytes()[..], &[0x2C, 0, 0, 0]].concat());
        assert_eq!(with_latency(&old, true, (0.0, -2.0)), [&2f32.to_le_bytes()[..], &[0x0C, 0, 0, 0]].concat());
        assert_eq!(with_latency(&old, false, (-2.0, 0.0)), [&(-2f32).to_le_bytes()[..], &[0x0C, 0, 0, 0]].concat());
        let both = with_latency(&rec(&[0x2C, 0, 0, 0]), true, (-2.0, 1.0));
        assert_eq!(both.len(), 12);
        assert_eq!(track_latency(&both, true), (-2.0, 1.0));
        assert_eq!(both[4], 0x0C);
    }

    /// Sends stored the old way (FL 24.1 and before: kind 0x1F, id 64 + destination) are read, and edits keep that
    /// form (spec: flp/layouts.md "Parameter ids").
    #[test]
    fn old_form_sends_are_read_and_kept() {
        use os_model::MixerTrack;
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        p.mixer[2].routes = vec![0, 5];
        p.mixer[2].send_levels = vec![(5, 0.5)];
        let mut file = create(&p).unwrap();
        {
            let ev = events_of_mut(&mut file).unwrap();
            let e = ev.iter_mut().rfind(|e| e.id == id::MX_PARAMS).unwrap();
            let mut d = e.data().unwrap().to_vec();
            for r in d.chunks_exact_mut(MIXER_REC_LEN) {
                if r[5] == MX_KIND_ROUTE {
                    (r[4], r[5]) = (64 + r[4], MX_KIND_PARAM);
                }
            }
            *e = Event::new(id::MX_PARAMS, Value::var(d));
        }
        let back = load(&file).unwrap();
        assert_eq!(back.mixer[2].send_levels, vec![(5, 0.5)]);
        let mut edited = back.clone();
        edited.mixer[2].send_levels = vec![(5, 0.25)];
        apply(&edited, &mut file).unwrap();
        assert_eq!(load(&file).unwrap().mixer[2].send_levels, vec![(5, 0.25)]);
        let ev = events_of(&file).unwrap();
        let d = ev.iter().rfind(|e| e.id == id::MX_PARAMS).unwrap().data().unwrap();
        assert!(mixer_param_records(d).all(|(_, _, k, _, _)| k != MX_KIND_ROUTE), "still in the old form");
    }

    /// A plugin channel made in Lemon Studio (spec: flp/events/plugins.md §2-3): its block holds the plugin name, a
    /// generator window record and the state after the colour flag; it reads back, and a changed state is written.
    #[test]
    fn created_plugin_channel_reads_back() {
        let mut p = new_project();
        let mut ch = sampler(2, "Lead", 3);
        ch.kind = ChannelKind::Plugin;
        ch.plugin = Some(crate::wrapper::WRAPPER.into());
        ch.plugin_state = Some(vec![10, 0, 0, 0, 9, 9]);
        p.channels.push(ch.clone());
        let mut file = create(&p).unwrap();
        let ids: Vec<u8> = {
            let ev = events_of(&file).unwrap();
            let start = ev.iter().rposition(|e| e.id == id::CH_START).unwrap();
            ev[start..start + 9].iter().map(|e| e.id).collect()
        };
        assert_eq!(ids, [64, 21, 201, 212, 203, 155, 128, 41, 213]);
        let rec = events_of(&file).unwrap().iter().filter(|e| e.id == id::PLUGIN_REC).last().unwrap().data().unwrap().to_vec();
        assert_eq!((rec.len(), rec[8], rec[16]), (52, 2, 0x50));
        let back = load(&file).unwrap();
        assert_eq!(back.channels[2].plugin, ch.plugin);
        assert_eq!(back.channels[2].plugin_state, ch.plugin_state);
        let mut edited = back.clone();
        edited.channels[2].plugin_state = Some(vec![1; 40]);
        apply(&edited, &mut file).unwrap();
        assert_eq!(load(&file).unwrap().channels[2].plugin_state, Some(vec![1; 40]));
        // Sampler channels keep their minimal block (no window record).
        assert!(events_of(&file).unwrap().iter().filter(|e| e.id == id::PLUGIN_REC).count() == 1);
    }

    /// A created project with a playlist stores its mixer (spec: flp/events/mixer.md, plugins.md): track settings,
    /// slot plugins, LEDs and mix levels read back, and later slot edits apply to the file.
    #[test]
    fn created_mixer_reads_back_and_takes_edits() {
        use os_model::MixerTrack;
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        let m = &mut p.mixer[2];
        m.name = Some("Bass bus".into());
        m.color = Some(Rgb::new(1, 2, 3));
        m.volume = Some(0.5);
        m.pan = Some(-0.25);
        m.routes = vec![0, 5];
        m.send_levels = vec![(5, 0.5)];
        m.slots[0] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Parametric EQ 2".into(), state: vec![1, 2, 3] });
        m.slots[3] = Some(SlotPlugin { id: os_model::Id::NONE, name: crate::wrapper::WRAPPER.into(), state: vec![10, 0, 0, 0] });
        m.slot_on[3] = false;
        m.slot_mix[3] = 0.5;
        p.mixer[0].enabled = false;
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back.mixer, p.mixer);
        assert!(unsaved_parts(&p, &file).is_empty());
        let ev = events_of(&file).unwrap();
        let count = ev.iter().find(|e| e.id == id::MX_COUNT).map(int);
        assert_eq!(count, Some(18));
        // Slot plugins carry their track and slot in the window record.
        let rec = ev.iter().filter(|e| e.id == id::PLUGIN_REC).nth(1).unwrap().data().unwrap();
        assert_eq!((rec[0], rec[4], rec[17]), (2, 3, 0x01));
        let mut edited = back.clone();
        edited.mixer[2].slots.swap(0, 9);
        edited.mixer[4].slots[1] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Limiter".into(), state: vec![] });
        edited.mixer[2].slot_mix[9] = 0.75;
        apply(&edited, &mut file).unwrap();
        assert_eq!(load(&file).unwrap().mixer, edited.mixer);
    }

    /// A track's audio input and output, record source and monitor mode are events 154, 147, 165 and 166 (spec:
    /// audio/track-io.md §1.6): written for a new project, read back, and changed in place; arming is not saved.
    #[test]
    fn track_inputs_and_outputs_are_saved() {
        use os_model::MixerTrack;
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        p.mixer[1].audio_in = 2;
        p.mixer[1].audio_out = 0;
        p.mixer[0].audio_out = -1;
        p.mixer[3].monitor = os_model::MONITOR_ON;
        p.mixer[3].rec_source = os_model::REC_EXTERNAL;
        p.mixer[3].armed = true;
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back.mixer, p.mixer);
        assert!(!back.mixer[3].armed);
        let at = |f: &FlpFile, eid: u8, t: usize| int(events_of(f).unwrap().iter().filter(|e| e.id == eid).nth(t).unwrap()) as i32;
        assert_eq!((at(&file, id::MX_INPUT, 1), at(&file, id::MX_OUTPUT, 1), at(&file, id::MX_OUTPUT, 0)), (2, 0, -1));
        assert_eq!((at(&file, id::MX_MONITOR, 3), at(&file, id::MX_REC_SOURCE, 3)), (3, 0));
        let mut edited = back.clone();
        edited.mixer[1].audio_in = -1;
        edited.mixer[5].audio_out = 1;
        edited.mixer[5].monitor = os_model::MONITOR_OFF;
        apply(&edited, &mut file).unwrap();
        assert_eq!(load(&file).unwrap().mixer, edited.mixer);
        assert_eq!((at(&file, id::MX_INPUT, 1), at(&file, id::MX_OUTPUT, 5), at(&file, id::MX_MONITOR, 5)), (-1, 1, 0));
    }

    /// The last stored track is the "current" track: its slot plugins and records use track 501. Added tracks go
    /// before it (spec: flp/events/mixer.md "The last track", "Adding tracks").
    #[test]
    fn added_mixer_tracks_go_before_the_current_track() {
        use os_model::MixerTrack;
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        p.mixer[17].slots[9] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Limiter".into(), state: vec![7; 4] });
        p.mixer[17].slot_mix[9] = 0.25;
        p.mixer[17].volume = Some(0.5);
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let ev = events_of(&file).unwrap();
        let rec = ev.iter().find(|e| e.id == id::PLUGIN_REC).unwrap().data().unwrap();
        assert_eq!(u32::from_le_bytes(rec[0..4].try_into().unwrap()), 501);
        let params = ev.iter().rfind(|e| e.id == id::MX_PARAMS).unwrap().data().unwrap();
        let tracks: std::collections::BTreeSet<u16> =
            mixer_param_records(params).filter(|r| r.3 >= 0x7000).map(|r| (r.3 - 0x7000) / 64).collect();
        assert_eq!(tracks, (0..17).chain([501]).collect());
        assert_eq!(load(&file).unwrap().mixer, p.mixer);
        // Three tracks added at the end: the current track moves to 20 with everything it had.
        let mut grown = p.clone();
        let mut current = grown.mixer.pop().unwrap();
        current.index = 20;
        grown.mixer.extend((17..20).map(MixerTrack::new));
        grown.mixer.push(current);
        grown.mixer[18].name = Some("New".into());
        grown.mixer[18].slots[0] = Some(SlotPlugin { id: os_model::Id::NONE, name: "Fruity Parametric EQ 2".into(), state: vec![1, 2] });
        grown.mixer[19].routes = vec![0, 18];
        apply(&grown, &mut file).unwrap();
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back.mixer, grown.mixer);
        let ev = events_of(&file).unwrap();
        assert_eq!(ev.iter().find(|e| e.id == id::MX_COUNT).map(int), Some(21));
        let params = ev.iter().rfind(|e| e.id == id::MX_PARAMS).unwrap().data().unwrap();
        let tracks: Vec<u16> = mixer_param_records(params).filter(|r| r.3 >= 0x7000).map(|r| (r.3 - 0x7000) / 64).collect();
        assert!(tracks.windows(2).all(|w| w[0] <= w[1]), "records stay in track order");
        assert_eq!(tracks.iter().filter(|&&t| t == 19).count(), 32);
        // Fewer tracks (spec: flp/events/mixer.md "Removing tracks"): inserts 18 and 19 go with their records, the
        // current track comes back as 18 with everything it had.
        let mut fewer = back.clone();
        let mut current = fewer.mixer.pop().unwrap();
        fewer.mixer.truncate(18);
        current.index = 18;
        fewer.mixer.push(current);
        fewer.mixer[17].name = Some("Kept".into());
        apply(&fewer, &mut file).unwrap();
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back.mixer, fewer.mixer);
        let ev = events_of(&file).unwrap();
        assert_eq!(ev.iter().find(|e| e.id == id::MX_COUNT).map(int), Some(19));
        let params = ev.iter().rfind(|e| e.id == id::MX_PARAMS).unwrap().data().unwrap();
        let tracks: std::collections::BTreeSet<u16> =
            mixer_param_records(params).filter(|r| r.3 >= 0x7000).map(|r| (r.3 - 0x7000) / 64).collect();
        assert_eq!(tracks, (0..18).chain([501]).collect());
    }

    /// A project of 18 tracks with effects whose window records (event 212) carry a marker byte naming the track that
    /// made them: tracks 3 and 4 hold the same plugin with different states, track 5 another, the current track one more.
    fn marked_mixer() -> (Project, FlpFile) {
        use os_model::MixerTrack;
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.mixer = (0..18).map(MixerTrack::new).collect();
        p.mixer[17].routes.clear();
        let fx = |name: &str, state: u8| Some(SlotPlugin { id: os_model::Id::NONE, name: name.into(), state: vec![state; 4] });
        p.mixer[3].slots[0] = fx("Fruity Limiter", 3);
        p.mixer[4].slots[0] = fx("Fruity Limiter", 4);
        p.mixer[5].slots[2] = fx("Fruity Delay 3", 5);
        p.mixer[17].slots[9] = fx("Fruity Limiter", 17);
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        for e in events_of_mut(&mut file).unwrap().iter_mut().filter(|e| e.id == id::PLUGIN_REC) {
            let track = e.data().unwrap()[0];
            patch_bytes(e, MARK, &[if track == 0xF5 { 17 } else { track }]);
        }
        let p = load(&file).unwrap();
        (p, file)
    }

    /// Byte of the slot window record that the tests mark (one Lemon Studio does not read).
    const MARK: usize = 40;

    /// The slot window records in file order: (track, slot, marker).
    fn slot_records(file: &FlpFile) -> Vec<(u32, u32, u8)> {
        let le = |d: &[u8], at: usize| u32::from_le_bytes(d[at..at + 4].try_into().unwrap());
        events_of(file).unwrap().iter().filter(|e| e.id == id::PLUGIN_REC).map(|e| e.data().unwrap()).map(|d| (le(d, 0), le(d, 4), d[MARK])).collect()
    }

    /// The model's tracks given new numbers in their new order.
    fn renumbered(mut mixer: Vec<os_model::MixerTrack>) -> Vec<os_model::MixerTrack> {
        for (i, m) in mixer.iter_mut().enumerate() {
            m.index = i as u16;
        }
        mixer
    }

    /// Moved tracks take their effect slots with them, events and all: two tracks holding the same plugin swap places,
    /// and the window records follow with their new track numbers (spec: flp/events/mixer.md "Removing tracks", "Slot
    /// sub-block").
    #[test]
    fn moved_mixer_tracks_take_their_effect_slots() {
        let (p, mut file) = marked_mixer();
        assert_eq!(slot_records(&file), vec![(3, 0, 3), (4, 0, 4), (5, 2, 5), (501, 9, 17)]);
        let mut moved = p.clone();
        moved.mixer.swap(3, 4);
        moved.mixer = renumbered(moved.mixer);
        moved.mixer[4].name = Some("Was 3".into());
        apply(&moved, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        assert_eq!(slot_records(&file), vec![(3, 0, 4), (4, 0, 3), (5, 2, 5), (501, 9, 17)]);
        let back = load(&file).unwrap();
        assert_eq!(back.mixer, moved.mixer);
        // Saved and loaded again, the tracks are the file's own: writing them again changes nothing.
        let mut again = file.clone();
        apply(&back, &mut again).unwrap();
        assert_eq!(again, file);
    }

    /// A deleted track's effects go with it and the tracks after it take theirs to their new numbers; the current track
    /// keeps its own as track 501 (spec: flp/events/mixer.md "Removing tracks", "The last track").
    #[test]
    fn deleted_mixer_track_takes_its_effect_slots() {
        let (p, mut file) = marked_mixer();
        let mut fewer = p.clone();
        fewer.mixer.remove(3);
        fewer.mixer = renumbered(fewer.mixer);
        apply(&fewer, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        assert_eq!(slot_records(&file), vec![(3, 0, 4), (4, 2, 5), (501, 9, 17)]);
        let back = load(&file).unwrap();
        assert_eq!(back.mixer, fewer.mixer);
        assert_eq!(events_of(&file).unwrap().iter().find(|e| e.id == id::MX_COUNT).map(int), Some(17));
    }

    /// Trimmed tracks (the inserts before the current track taken off the end) lose their effects; a track moved from
    /// there to an earlier place first keeps its own (spec: flp/events/mixer.md "Removing tracks").
    #[test]
    fn trimmed_mixer_tracks_take_their_effect_slots() {
        let (p, mut file) = marked_mixer();
        let mut trimmed = p.clone();
        let current = trimmed.mixer.pop().unwrap();
        let five = trimmed.mixer[5].clone();
        trimmed.mixer.truncate(3);
        trimmed.mixer.push(five);
        trimmed.mixer.push(current);
        trimmed.mixer = renumbered(trimmed.mixer);
        apply(&trimmed, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        assert_eq!(slot_records(&file), vec![(3, 2, 5), (501, 9, 17)]);
        let back = load(&file).unwrap();
        assert_eq!(back.mixer, trimmed.mixer);
        assert_eq!(events_of(&file).unwrap().iter().find(|e| e.id == id::MX_COUNT).map(int), Some(5));
    }

    /// A track's icon (event 95) is read, written after the colour flag, changed, taken off, and follows its track
    /// when tracks move (spec: flp/events/mixer.md "Track blocks").
    #[test]
    fn mixer_track_icon_round_trips() {
        let (mut p, mut file) = marked_mixer();
        assert!(p.mixer.iter().all(|m| m.icon.is_none()));
        p.mixer[2].icon = Some(42);
        p.mixer[3].icon = Some(7);
        p.mixer[3].name = Some("Drums".into());
        apply(&p, &mut file).unwrap();
        let file2 = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let back = load(&file2).unwrap();
        assert_eq!(back.mixer, p.mixer);
        let ev = events_of(&file2).unwrap();
        let after = |eid: u8| ev.iter().position(|e| e.id == id::MX_ICON).map(|i| ev[i - 1].id == eid);
        assert_eq!(after(id::MX_COLOR_FLAG), Some(true), "the icon follows the colour flag");
        let icon_then_name = ev.windows(2).any(|w| w[0].id == id::MX_ICON && int(&w[0]) == 7 && w[1].id == id::MX_NAME);
        assert!(icon_then_name, "an icon added with a name goes before it");
        // A track made with an icon reads it back too.
        assert_eq!(load(&create(&p).unwrap()).unwrap().mixer, p.mixer);
        // Changed, taken off, and moved with its track.
        let mut edited = back.clone();
        edited.mixer[2].icon = Some(43);
        edited.mixer[3].icon = None;
        edited.mixer.swap(2, 4);
        edited.mixer = renumbered(edited.mixer);
        let mut file = file2.clone();
        apply(&edited, &mut file).unwrap();
        let back = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back.mixer, edited.mixer);
        assert_eq!(back.mixer[4].icon, Some(43));
        assert_eq!(events_of(&file).unwrap().iter().filter(|e| e.id == id::MX_ICON).count(), 1);
    }

    #[test]
    fn unsaved_parts_name_what_a_created_file_lacks() {
        use os_model::MixerTrack;
        let mut p = new_project();
        let file = create(&p).unwrap();
        assert!(unsaved_parts(&p, &file).is_empty());
        let track = MixerTrack::new;
        p.mixer = (0..3).map(track).collect();
        assert!(unsaved_parts(&p, &file).is_empty(), "default mixer: nothing is lost");
        p.mixer[1].volume = Some(0.5);
        assert_eq!(unsaved_parts(&p, &file), vec!["mixer settings"]);
    }


    /// Mute (byte 19 bit 0x20), colour group, slide flag and unknown bytes survive a note edit
    /// (spec: flp/events/patterns.md, note flags).
    #[test]
    fn note_record_keeps_mute_and_flags() {
        let mut n = Note { flags: 0x4008, group: 0x04, unk14: 3, repeat: 4, ..Note::step(2, 96) };
        n.set_muted(true);
        let r = encode_note(&n);
        assert_eq!(r[19], 0x24);
        assert_eq!(decode_note(&r), n);
    }

    /// Byte 17 is the note repeat index (spec: flp/events/patterns.md).
    #[test]
    fn note_repeat_is_byte_17() {
        let n = Note { repeat: 14, ..Note::step(0, 0) };
        assert_eq!(encode_note(&n)[17], 14);
    }

    /// Every corpus clip list re-encodes to the same bytes, and every automation record too
    /// (spec: flp/events/playlist.md, automation.md).
    #[test]
    fn corpus_clips_and_automation_reencode_exactly() {
        let Some(list) = std::env::var_os("OPENSTUDIO_FLP_CORPUS") else { return };
        let text = std::fs::read_to_string(list).unwrap();
        let (mut lists, mut autos) = (0, 0);
        for path in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let file = FlpFile::parse(&std::fs::read(path).unwrap()).unwrap();
            for e in events_of(&file).unwrap() {
                match e.id {
                    id::ARR_CLIPS => {
                        let d = e.data().unwrap();
                        let n = list_clip_len(d).unwrap_or(CLIP_LEN);
                        assert_eq!(encode_clips(&decode_clips(d, n), n), d, "{path}");
                        lists += 1;
                    }
                    id::AUTO_DATA => {
                        let d = e.data().unwrap();
                        let a = decode_automation(0, d).unwrap_or_else(|| panic!("{path}: automation decodes"));
                        assert_eq!(encode_automation(&a), d, "{path}");
                        autos += 1;
                    }
                    _ => {}
                }
            }
        }
        eprintln!("{lists} clip lists, {autos} automation records re-encode exactly");
    }

    /// The LFO's fields and the range come back from a record as stored, and an edited LFO is written where it was read
    /// from, every other byte kept (spec: flp/layouts.md "An automation clip's record", audio/automation.md "The LFO").
    #[test]
    fn automation_lfo_fields_round_trip() {
        // A record as FL writes one: two curve points, the LFO on, adding, amount 38, two envelope points.
        let mut a = new_automation(2, os_model::target::channel(0, 0), 0.5, 4.0);
        a.lfo = ClipLfo {
            switch: -1,
            amount: 38,
            mode: 1,
            speed: 40898,
            tension: 32,
            skew: -128,
            pulse_width: 64,
            phase: 7,
            flags: 3,
            envelope: vec![os_model::clip_lfo::envelope_point(0.0, 0.0), os_model::clip_lfo::envelope_point(2.0, 1.0)],
        };
        let d = encode_automation(&a);
        assert_eq!(d.len(), 109 + 24 * 2 + 24 * 2, "109 + 24 n + 24 m");
        let o = 21 + 48;
        assert_eq!((le_i32(&d, 4), d[8]), (38, 1));
        assert_eq!(le_i32(&d, o + 44), 2, "the envelope's count");
        let v = o + 48 + 48;
        assert_eq!((d[v], le_i32(&d, v + 4), le_i32(&d, v + 8), le_i32(&d, v + 20)), (3, -1, -1, 40898));
        assert_eq!((le_i32(&d, v + 24), le_i32(&d, v + 28), le_i32(&d, v + 32), le_i32(&d, v + 36)), (32, -128, 64, 7));
        let back = decode_automation(2, &d).unwrap();
        assert_eq!(back.lfo, a.lfo);
        assert_eq!(back.points, a.points);
        assert_eq!(encode_automation(&back), d, "decoded and encoded again: the same bytes");
        // Bytes the model does not know stay, and an edit changes only its own field.
        let mut d2 = d.clone();
        d2[o + 1] = 0x55;
        d2.extend([9, 8, 7]);
        let mut back = decode_automation(2, &d2).unwrap();
        assert_eq!(encode_automation(&back), d2);
        back.lfo.speed = 26259;
        let e = encode_automation(&back);
        assert_eq!(le_i32(&e, v + 20), 26259);
        assert!(e.iter().zip(&d2).enumerate().all(|(i, (x, y))| x == y || (v + 20..v + 24).contains(&i)));
        // A record whose tail is cut short keeps it while the LFO is at its defaults; an LFO switched on gets the
        // new clip's layout.
        let mut short = Automation { header: vec![0; 21], tail: vec![1, 2, 3], ..new_automation(2, 0, 0.5, 4.0) };
        short.lfo = ClipLfo::default();
        assert_eq!(&encode_automation(&short)[21 + 48..], &[1, 2, 3]);
        short.lfo.switch = -1;
        let e = encode_automation(&short);
        assert_eq!(e.len(), 109 + 48 + 24);
        assert_eq!(decode_automation(2, &e).unwrap().lfo, short.lfo);
    }

    /// A new clip's record is FL's: amount 64, the LFO off with its flags 1, speed 45819, one envelope point at 1.
    #[test]
    fn a_new_clip_has_the_default_lfo() {
        let a = new_automation(2, os_model::target::channel(0, 0), 0.5, 4.0);
        let d = encode_automation(&a);
        assert_eq!(d.len(), 109 + 48 + 24);
        let t = &d[21 + 48..];
        assert_eq!(&t[36..48], &[5, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(f64::from_le_bytes(t[56..64].try_into().unwrap()), 1.0);
        assert_eq!(&t[72..80], &[1, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&t[92..96], &45819i32.to_le_bytes());
        assert_eq!((le_i32(&d, 4), d[8]), (64, 0));
        assert_eq!(decode_automation(2, &d).unwrap(), Automation { targets: Vec::new(), ..a });
        assert_eq!(untargeted_automation(2, 0.5, 4.0).lfo.mode, 1);
    }

    /// Files of FL 24.2 to 25 hold 80-byte clip records: read with their fields, written back at 80 bytes with 0
    /// at bytes 76..79; 88-byte lists are still told apart (spec: flp/layouts.md, "Playlist clip records").
    #[test]
    fn eighty_byte_clip_records_are_read_and_kept() {
        let a = Clip::new(384, 288, 2, ClipSource::Pattern(1));
        let b = Clip::new(768, 96, 0, ClipSource::Pattern(2));
        let d = encode_clips(&[a, b], CLIP_LEN_25);
        assert_eq!(d.len(), 2 * CLIP_LEN_25);
        assert_eq!(list_clip_len(&d), Some(CLIP_LEN_25));
        assert!(d[76..80].iter().all(|&x| x == 0));
        let back = decode_clips(&d, CLIP_LEN_25);
        assert_eq!((back[1].position, back[1].length, back[1].track), (768, 96, 0));
        assert_eq!(encode_clips(&back, CLIP_LEN_25), d);
        assert_eq!(list_clip_len(&encode_clips(&back, CLIP_LEN)), Some(CLIP_LEN));
    }

    /// Lists of older files, 60-byte (FL 21 to 24.1) and 32-byte (up to FL 20) records, are told apart and written
    /// back at their length; 32-byte records have no id (spec: flp/layouts.md, "How to tell the generation").
    #[test]
    fn older_clip_records_are_read_at_their_length() {
        let a = Clip::new(384, 288, 2, ClipSource::Pattern(1));
        let b = Clip::new(768, 96, 0, ClipSource::Pattern(2));
        for len in [CLIP_LEN_21, CLIP_LEN_20] {
            let d = encode_clips(&[a.clone(), b.clone()], len);
            assert_eq!(d.len(), 2 * len);
            assert_eq!(list_clip_len(&d), Some(len));
            let back = decode_clips(&d, len);
            assert_eq!((back[1].position, back[1].length, back[1].track), (768, 96, 0));
            assert_eq!(encode_clips(&back, len), d);
        }
    }

    /// A new project's playlist (clips with windows, mute, markers) and automation clips are written by `create`,
    /// read back, and later edits go through `apply` (spec: flp/events/playlist.md, automation.md).
    #[test]
    fn created_playlist_and_automation_round_trip() {
        let mut p = new_project();
        let mut arr = default_arrangement();
        let mut c = Clip::new(384, 288, 0, ClipSource::Pattern(1));
        c.window = Some((96.0, 384.0));
        c.muted = true;
        arr.clips.push(c);
        arr.markers.push(TimeMarker::new(768, "Verse"));
        p.arrangements.push(arr);
        let mut auto_ch = sampler(2, "Kick - Channel volume", 0);
        auto_ch.kind = ChannelKind::Automation;
        p.channels.push(auto_ch);
        let mut a = new_automation(2, os_model::target::channel(0, 0), 0.78125, 4.0);
        a.points.insert(1, AutoPoint { beat: 2.0, value: 0.25, tension: -0.5, mode: AutoMode::Stairs, flag: 0 });
        p.automations.push(a);
        p.arrangements[0].clips.push(Clip::new(0, 384, 1, ClipSource::Channel(2)));
        let file = create(&p).unwrap();
        let mut file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back.channels.len(), 3);
        assert_eq!(back.channels[2].kind, ChannelKind::Automation);
        assert_eq!(back.automations, p.automations);
        let arr = &back.arrangements[0];
        assert_eq!(arr.tracks.len(), 500);
        assert_eq!(arr.markers, p.arrangements[0].markers);
        assert!(arr.tracks.iter().all(|t| !t.grouped && !t.collapsed && !t.locked), "new tracks: no groups, expanded");
        let c = arr.clips.iter().find(|c| c.source == ClipSource::Pattern(1)).unwrap();
        assert_eq!((c.position, c.length, c.track, c.muted, c.window), (384, 288, 0, true, Some((96.0, 384.0))));
        let ac = arr.clips.iter().find(|c| c.source == ClipSource::Channel(2)).unwrap();
        assert_eq!((ac.track, ac.window), (1, None));
        assert_eq!(&ac.raw[24..28], &(-1.0f32).to_le_bytes());

        // Edits: move and unmute a clip, add a marker, change a point, add another automation channel.
        let mut edited = back.clone();
        let a0 = &mut edited.arrangements[0];
        a0.clips[0].position = 0;
        a0.clips[0].muted = false;
        // Audio clip fades and gain live in the record (spec: flp/events/playlist.md bytes 36-59).
        a0.clips[1].set_fade_in(Some((250.0, 1.5)));
        a0.clips[1].set_fade_out(Some((80.0, -2.0)));
        a0.clips[1].set_gain(0.5);
        a0.markers.push(TimeMarker::new(0, "Intro"));
        a0.tracks[3].enabled = false;
        a0.tracks[3].name = Some("Drums".into());
        // A collapsed group: track 3 leads, tracks 4 and 5 are its members (spec: flp/events/playlist.md).
        a0.tracks[3].collapsed = true;
        a0.tracks[4].grouped = true;
        a0.tracks[5].grouped = true;
        a0.tracks[5].locked = true;
        edited.automations[0].points[1].value = 0.9;
        let mut ch = sampler(3, "Tempo", 0);
        ch.kind = ChannelKind::Automation;
        edited.channels.push(ch);
        edited.automations.push(new_automation(3, os_model::target::TEMPO, 0.5, 8.0));
        apply(&edited, &mut file).unwrap();
        let again = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(file.header().unwrap().channel_count(), Some(4));
        assert_eq!(again.automations, edited.automations);
        assert_eq!(again.arrangements[0].markers, edited.arrangements[0].markers);
        assert_eq!(again.arrangements[0].tracks[3..6], edited.arrangements[0].tracks[3..6]);
        let mut want = edited.arrangements[0].clips.clone();
        want.sort_by_key(|c| c.position);
        let strip = |v: &[Clip]| v.iter().map(|c| (c.position, c.length, c.track, c.source, c.muted, c.window)).collect::<Vec<_>>();
        assert_eq!(strip(&again.arrangements[0].clips), strip(&want));
        let fx = |v: &[Clip]| v.iter().map(|c| (c.position, c.fade_in(), c.fade_out(), c.gain().to_bits())).collect::<Vec<_>>();
        assert_eq!(fx(&again.arrangements[0].clips), fx(&want), "fades and gain survive");
        assert!(want.iter().any(|c| c.fade_in() == Some((250.0, 1.5))));
        // Removing a track name works too.
        let mut e2 = again.clone();
        e2.arrangements[0].tracks[3].name = None;
        apply(&e2, &mut file).unwrap();
        let back3 = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back3.arrangements[0].tracks[3].name, None);
        if let Some(keep) = std::env::var_os("OS_KEEP_FLP") {
            let _ = std::fs::write(keep, file.to_bytes().unwrap());
        }
    }

    /// Automatic crossfades are two ordinary fades in the clip records, written without their bits (spec:
    /// flp/events/playlist.md bytes 36..56, audio/audio-clips.md "Automatic crossfades"): with event 44 on, the
    /// records' bytes 36..57 come back as written (the bits clear on a crossfade, set on a fade set by hand, other bits
    /// kept) through a new file, through the crossfades worked out again from what was read, and through edits.
    #[test]
    fn crossfade_fades_keep_their_bytes() {
        let mut p = new_project();
        (p.auto_crossfades, p.tempo) = (Some(true), Some(130.0));
        let mut tone = sampler(2, "Tone", 0);
        tone.kind = ChannelKind::AudioClip;
        tone.sample_path = Some("C:/samples/tone.wav".into());
        p.channels.push(tone);
        let src = ClipSource::Channel(2);
        let (mut a, mut b, mut c) = (Clip::new(0, 480, 0, src), Clip::new(240, 480, 0, src), Clip::new(2000, 480, 0, src));
        // A crossfade as FL writes one (240 ticks at 130 BPM, PPQ 96; tensions 2.289 / -2.289; no bits), and a fade set
        // by hand.
        a.set_fade_out_length(1153.846);
        a.set_fade_out_tension(-2.289);
        b.set_fade_in_length(1153.846);
        b.set_fade_in_tension(2.289);
        c.set_fade_in(Some((300.0, -1.5)));
        c.raw[56] |= 0x08;
        let mut arr = default_arrangement();
        arr.clips = vec![a, b, c];
        p.arrangements.push(arr);
        let bytes = |p: &Project| p.arrangements[0].clips.iter().map(|c| c.raw[36..57].to_vec()).collect::<Vec<_>>();
        let want = bytes(&p);
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let mut back = load(&file).unwrap();
        assert_eq!(back.auto_crossfades, Some(true));
        assert_eq!(bytes(&back), want, "through a new file");
        assert!(!os_model::crossfade::derive(&mut back, 130.0), "worked out again: nothing to change");
        assert_eq!(bytes(&back), want);
        back.arrangements[0].clips[2].position = 2400;
        apply(&back, &mut file).unwrap();
        let again = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(bytes(&again), want, "through edits");
        assert_eq!(again.arrangements[0].clips.iter().map(|c| c.raw[56]).collect::<Vec<_>>(), [0, 0, 9]);
    }

    /// The playlist view (event 242: scroll, time zoom, track height) sits before event 100, reads back and takes
    /// edits; a file without one gets it (spec: flp/events/playlist.md "Playlist view").
    #[test]
    fn playlist_view_round_trips() {
        let mut p = new_project();
        p.arrangements.push(default_arrangement());
        p.current_arrangement = Some(0);
        let mut v = os_model::PlaylistView::new(96);
        assert_eq!((v.row_px(), v.ticks_per_px(), v.left(), v.scroll_y()), (48, 6.4, 0.0, 0));
        v.set_height(22);
        v.set_scroll_y(64);
        p.playlist_view = Some(v);
        let file = create(&p).unwrap();
        let mut file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let ids: Vec<u8> = file.events().map(|e| e.id).collect();
        let at = ids.iter().position(|&i| i == id::PLAYLIST_VIEW).expect("event 242 written");
        assert_eq!(ids[at + 1], id::CURRENT_ARRANGEMENT, "242 comes right before 100");
        let back = load(&file).unwrap();
        assert_eq!(back.playlist_view, Some(v));
        assert_eq!(back.playlist_view.unwrap().row_px(), 60);

        let mut edited = back.clone();
        let mut v2 = v;
        v2.set_left(768.0);
        v2.set_ticks_per_px(0.285);
        v2.set_height(132);
        edited.playlist_view = Some(v2);
        apply(&edited, &mut file).unwrap();
        let again = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(again.playlist_view, Some(v2));
        assert_eq!(again.playlist_view.unwrap().row_px(), 280);
        assert_eq!(file.events().filter(|e| e.id == id::PLAYLIST_VIEW).count(), 1);

        // A file without event 242 gets one before event 100.
        let mut bare = p.clone();
        bare.playlist_view = None;
        let mut file = FlpFile::parse(&create(&bare).unwrap().to_bytes().unwrap()).unwrap();
        assert!(file.events().all(|e| e.id != id::PLAYLIST_VIEW));
        apply(&p, &mut file).unwrap();
        let ids: Vec<u8> = file.events().map(|e| e.id).collect();
        let at = ids.iter().position(|&i| i == id::PLAYLIST_VIEW).expect("event 242 added");
        assert_eq!(ids[at + 1], id::CURRENT_ARRANGEMENT);
        assert_eq!(load(&file).unwrap().playlist_view, Some(v));
    }

    /// Event 223 records: `position, channel << 16 | param, value` (spec: flp/events/patterns.md).
    #[test]
    fn control_points_round_trip_and_sort_by_position() {
        let pts = [
            ControlPoint::new(48, 1, ControlPoint::VOLUME, 5007),
            ControlPoint::new(24, 0, ControlPoint::PITCH, -134),
            ControlPoint::new(24, 0, ControlPoint::PAN, 6400),
        ];
        let data = encode_controls(&pts);
        assert_eq!(data.len(), 36);
        // Sorted by position; equal positions keep their order.
        assert_eq!(&data[0..12], &[24, 0, 0, 0, 4, 0, 0, 0, 0x7A, 0xFF, 0xFF, 0xFF]);
        let back = decode_controls(&data);
        assert_eq!(back[1], pts[2]);
        assert_eq!(back[2].channel(), 1);
        assert_eq!(back[2].param(), ControlPoint::VOLUME);
        assert_eq!(back[0].value, -134);
    }

    /// The project with every channel's origin set to its position (what `load` gives for a saved file).
    fn settled(mut p: Project) -> Project {
        for (i, c) in p.channels.iter_mut().enumerate() {
            c.rack.origin = Some(i as u16);
        }
        p
    }

    /// Swing, filter groups, the current filter, lock, zip, pattern length and loop lengths are written and read
    /// back; writing the same model again changes nothing; clearing them works (spec: flp/events/project-info.md,
    /// channels.md, patterns.md).
    #[test]
    fn rack_settings_round_trip() {
        let mut file = FlpFile::parse(&create(&new_project()).unwrap().to_bytes().unwrap()).unwrap();
        let mut m = load(&file).unwrap();
        m.swing = Some(64);
        m.groups = vec!["Drums".into(), os_model::UNSORTED_GROUP.into()];
        m.current_group = Some(0);
        m.channels[0].rack.zipped = true;
        m.channels[1].rack = ChannelRack { group: 1, locked: true, ..m.channels[1].rack };
        for pat in &mut m.patterns {
            if pat.number == 2 {
                pat.length = Some(72);
                pat.loops = vec![(0, 5), (1, Pattern::LOOP_FOLLOW)];
                pat.loop_off = Some(2);
            } else {
                pat.length = Some(192);
            }
        }
        apply(&m, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back, m);
        let mut again = file.clone();
        apply(&back, &mut again).unwrap();
        assert_eq!(again.to_bytes().unwrap(), file.to_bytes().unwrap(), "an unchanged model writes nothing");
        // The pattern length is a "Length" marker plus event 164 in the metadata block.
        let ev = events_of(&file).unwrap();
        assert!(ev.iter().any(|e| e.id == id::MARKER_POS && int(e) == 6 << 24 | 72));
        assert!(ev.iter().any(|e| e.id == id::PAT_LENGTH && int(e) == 72));

        let mut cleared = back.clone();
        cleared.swing = Some(0);
        cleared.current_group = None;
        cleared.channels[0].rack.zipped = false;
        cleared.channels[1].rack.locked = false;
        for pat in &mut cleared.patterns {
            (pat.length, pat.loops, pat.loop_off) = (None, Vec::new(), None);
        }
        let mut file2 = file.clone();
        apply(&cleared, &mut file2).unwrap();
        let back2 = load(&FlpFile::parse(&file2.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back2, cleared);
        assert!(!events_of(&file2).unwrap().iter().any(|e| e.id == id::CH_ZIPPED || e.id == id::LOOP_CHANNEL));
    }

    /// The payload of the first event `eid` in `file`.
    fn event_data(file: &FlpFile, eid: u8) -> Option<Vec<u8>> {
        events_of(file).unwrap().iter().find(|e| e.id == eid).and_then(Event::data).map(<[u8]>::to_vec)
    }

    fn utf16z(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).chain([0, 0]).collect()
    }

    /// Title, genre, author, comments and URL edited in a saved file: `create` writes the empty ones empty (the URL not
    /// at all), a changed text is patched, a missing one is added in FL's order, comments keep single-CR line breaks, the
    /// text is UTF-16LE; the same model again writes nothing; a cleared field keeps its event, written empty, and reads
    /// back as no value; an empty one the file lacks gets none (spec: flp/events/project-info.md, flp/container.md "Text
    /// payloads").
    #[test]
    fn project_info_round_trips() {
        let mut file = FlpFile::parse(&create(&new_project()).unwrap().to_bytes().unwrap()).unwrap();
        assert_eq!(event_data(&file, id::AUTHOR), Some(vec![0, 0]), "an empty field is written empty");
        assert_eq!(event_data(&file, id::URL), None, "an empty URL is not written");
        assert_eq!(load(&file).unwrap().info.author, None, "an empty field reads as no value");
        let mut m = load(&file).unwrap();
        m.info = ProjectInfo {
            title: Some("Ünïcode Ω title".into()),
            author: Some("Me".into()),
            genre: Some("Ambient".into()),
            comments: Some("First line\nSecond line\n\nLast".into()),
            url: Some("https://example.org/song".into()),
        };
        apply(&m, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back, m);
        assert_eq!(event_data(&file, id::TITLE), Some(utf16z("Ünïcode Ω title")));
        assert_eq!(event_data(&file, id::COMMENTS), Some(utf16z("First line\rSecond line\r\rLast")));
        let ids: Vec<u8> = events_of(&file).unwrap().iter().map(|e| e.id).collect();
        let at = ids.iter().position(|&i| i == id::TITLE).unwrap();
        assert_eq!(ids[at - 1], id::CURRENT_PATTERN);
        assert_eq!(ids[at..at + 6], [id::TITLE, id::GENRE, id::AUTHOR, id::COMMENTS, id::URL, id::WORK_TIME]);
        let mut again = file.clone();
        apply(&back, &mut again).unwrap();
        assert_eq!(again.to_bytes().unwrap(), file.to_bytes().unwrap(), "an unchanged model writes nothing");

        let mut cleared = back.clone();
        cleared.info.title = None;
        cleared.info.comments = Some(String::new());
        let mut file2 = file.clone();
        apply(&cleared, &mut file2).unwrap();
        assert_eq!((event_data(&file2, id::TITLE), event_data(&file2, id::COMMENTS)), (Some(vec![0, 0]), Some(vec![0, 0])));
        let back2 = load(&FlpFile::parse(&file2.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!((back2.info.title.as_deref(), back2.info.comments.as_deref()), (None, None));
        assert_eq!(back2.info.author.as_deref(), Some("Me"));

        // A file without any of them: an empty field adds nothing, a filled one goes after the current pattern.
        let mut bare = new_project();
        bare.info = ProjectInfo::default();
        let mut file3 = create(&bare).unwrap();
        events_of_mut(&mut file3).unwrap().retain(|e| ![id::TITLE, id::GENRE, id::AUTHOR, id::COMMENTS, id::WORK_TIME].contains(&e.id));
        let mut m3 = load(&file3).unwrap();
        m3.info.genre = Some(String::new());
        m3.info.url = Some("https://example.org".into());
        apply(&m3, &mut file3).unwrap();
        let ids: Vec<u8> = events_of(&file3).unwrap().iter().map(|e| e.id).collect();
        assert!(!ids.contains(&id::GENRE));
        assert_eq!(ids[ids.iter().position(|&i| i == id::URL).unwrap() - 1], id::CURRENT_PATTERN);
        assert_eq!(load(&file3).unwrap().info.url.as_deref(), Some("https://example.org"));
    }

    /// Files saved by FL 11 and older hold 8-bit text (spec: flp/container.md "Text payloads").
    #[test]
    fn project_info_is_8_bit_in_old_files() {
        let mut p = new_project();
        p.info = ProjectInfo::default();
        let mut file = create(&p).unwrap();
        for e in events_of_mut(&mut file).unwrap() {
            if e.id == id::VERSION {
                e.value = Value::var(b"11.1.1\0".to_vec());
            }
        }
        let mut m = load(&file).unwrap();
        m.info.title = Some("Café".into());
        m.info.comments = Some("a\nb".into());
        apply(&m, &mut file).unwrap();
        assert_eq!(event_data(&file, id::TITLE), Some(b"Caf\xe9\0".to_vec()));
        assert_eq!(event_data(&file, id::COMMENTS), Some(b"a\rb\0".to_vec()));
        assert_eq!(load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap().info, m.info);
    }

    /// FL 26 writes the project information as 194, 206, 207, 167, 202, 195, [197], 237, the four texts empty in a new
    /// project: an unchanged model keeps them byte for byte, an added comment or URL goes in that order (after 207,
    /// 167 and 202, before 237), and a cleared URL loses its event (spec: flp/engine-events.md "Order in the file",
    /// flp/events/project-info.md "Writer notes").
    #[test]
    fn project_info_keeps_fl_order() {
        const INFO: [u8; 8] = [id::TITLE, id::GENRE, id::AUTHOR, id::INFO_167, id::INFO_202, id::COMMENTS, id::URL, id::WORK_TIME];
        // A file holding `ids` of the project information, as FL writes them, behind the project events.
        let fl_file = |ids: &[u8]| {
            let mut p = new_project();
            p.info = ProjectInfo::default();
            let mut file = create(&p).unwrap();
            let ev = events_of_mut(&mut file).unwrap();
            ev.retain(|e| !INFO.contains(&e.id));
            let at = ev.iter().position(|e| e.id == id::CURRENT_PATTERN).unwrap() + 1;
            let info = ids.iter().map(|&eid| match eid {
                id::INFO_167 => Event::new(eid, Value::U32(2)),
                id::INFO_202 => Event::new(eid, Value::var(utf16z("C:\\Data\\"))),
                id::WORK_TIME => Event::new(eid, Value::var(vec![7; 16])),
                _ => Event::new(eid, Value::var(utf16z(""))),
            });
            ev.splice(at..at, info);
            FlpFile::parse(&file.to_bytes().unwrap()).unwrap()
        };
        let info_ids = |file: &FlpFile| -> Vec<u8> { events_of(file).unwrap().iter().map(|e| e.id).filter(|i| INFO.contains(i)).collect() };
        let saved = |m: &Project, file: &FlpFile| {
            let mut out = file.clone();
            apply(m, &mut out).unwrap();
            FlpFile::parse(&out.to_bytes().unwrap()).unwrap()
        };
        let new_fl = [id::TITLE, id::GENRE, id::AUTHOR, id::INFO_167, id::INFO_202, id::COMMENTS, id::WORK_TIME];

        // A new FL project: the empty texts load as no value and stay as they are.
        let file = fl_file(&new_fl);
        let m = load(&file).unwrap();
        assert_eq!((m.info.title.as_deref(), m.info.comments.as_deref(), m.info.url.as_deref()), (None, None, None));
        assert_eq!(saved(&m, &file).to_bytes().unwrap(), file.to_bytes().unwrap());

        // A URL goes behind the comments, before 237; the other texts stay empty.
        let mut m2 = m.clone();
        m2.info.url = Some("https://example.org".into());
        m2.info.title = Some("Song".into());
        let f2 = saved(&m2, &file);
        assert_eq!(info_ids(&f2), [id::TITLE, id::GENRE, id::AUTHOR, id::INFO_167, id::INFO_202, id::COMMENTS, id::URL, id::WORK_TIME]);
        assert_eq!((event_data(&f2, id::GENRE), event_data(&f2, id::COMMENTS)), (Some(vec![0, 0]), Some(vec![0, 0])));
        assert_eq!(load(&f2).unwrap().info, m2.info);

        // Cleared again: 197 goes, the others stay.
        let mut m3 = m2.clone();
        m3.info.url = None;
        let f3 = saved(&m3, &f2);
        assert_eq!(info_ids(&f3), new_fl);
        assert_eq!(load(&f3).unwrap().info, m3.info);

        // Without 195: added comments and a URL go behind 202, before 237.
        let file = fl_file(&[id::TITLE, id::GENRE, id::AUTHOR, id::INFO_167, id::INFO_202, id::WORK_TIME]);
        let mut m4 = load(&file).unwrap();
        m4.info.comments = Some("a\nb".into());
        m4.info.url = Some("u".into());
        let f4 = saved(&m4, &file);
        assert_eq!(info_ids(&f4), [id::TITLE, id::GENRE, id::AUTHOR, id::INFO_167, id::INFO_202, id::COMMENTS, id::URL, id::WORK_TIME]);
        assert_eq!(load(&f4).unwrap().info, m4.info);

        // With only 237: a title goes before it.
        let file = fl_file(&[id::WORK_TIME]);
        let mut m5 = load(&file).unwrap();
        m5.info.title = Some("T".into());
        assert_eq!(info_ids(&saved(&m5, &file)), [id::TITLE, id::WORK_TIME]);
    }

    /// The data folder (events 167, 202; spec: ui/data-folder.md §2): read as the file holds it, an unchanged model
    /// writes nothing, a changed one is patched in place, and one the file lacks goes between the author and the
    /// comments; `create` writes it there when the model has one, and nothing without.
    #[test]
    fn data_folder_round_trips() {
        let info_ids = |file: &FlpFile| -> Vec<u8> {
            let keep = [id::AUTHOR, id::INFO_167, id::INFO_202, id::COMMENTS, id::WORK_TIME];
            events_of(file).unwrap().iter().map(|e| e.id).filter(|i| keep.contains(i)).collect()
        };
        let reparse = |f: &FlpFile| FlpFile::parse(&f.to_bytes().unwrap()).unwrap();
        // Without one: `create` writes neither event; the model has none.
        let file = reparse(&create(&new_project()).unwrap());
        assert_eq!(info_ids(&file), [id::AUTHOR, id::COMMENTS, id::WORK_TIME]);
        let mut m = load(&file).unwrap();
        assert_eq!(m.data_folder, None);
        // Choosing "Don't use a data folder" explicitly adds nothing.
        m.data_folder = Some(DataFolder::default());
        let mut same = file.clone();
        apply(&m, &mut same).unwrap();
        assert_eq!(same.to_bytes().unwrap(), file.to_bytes().unwrap());
        // A specified folder goes between the author and the comments, 202 as UTF-16 text with its backslash.
        m.data_folder = Some(DataFolder { choice: DataFolder::SPECIFIED, folder: "C:\\Songs\\My song\\".into() });
        let mut f2 = file.clone();
        apply(&m, &mut f2).unwrap();
        let f2 = reparse(&f2);
        assert_eq!(info_ids(&f2), [id::AUTHOR, id::INFO_167, id::INFO_202, id::COMMENTS, id::WORK_TIME]);
        assert_eq!(event_data(&f2, id::INFO_202), Some(utf16z("C:\\Songs\\My song\\")));
        let back = load(&f2).unwrap();
        assert_eq!(back.data_folder, m.data_folder);
        let mut again = f2.clone();
        apply(&back, &mut again).unwrap();
        assert_eq!(again.to_bytes().unwrap(), f2.to_bytes().unwrap(), "an unchanged model writes nothing");
        // Patched in place: relative, its folder rewritten; then none, 202 empty (just the terminator).
        let mut m3 = back.clone();
        m3.data_folder = Some(DataFolder { choice: DataFolder::RELATIVE, folder: "D:\\Elsewhere\\".into() });
        let mut f3 = f2.clone();
        apply(&m3, &mut f3).unwrap();
        assert_eq!(events_of(&f3).unwrap().len(), events_of(&f2).unwrap().len());
        assert_eq!(load(&reparse(&f3)).unwrap().data_folder, m3.data_folder);
        m3.data_folder = Some(DataFolder::default());
        apply(&m3, &mut f3).unwrap();
        assert_eq!(event_data(&f3, id::INFO_202), Some(vec![0, 0]));
        assert_eq!(load(&reparse(&f3)).unwrap().data_folder, Some(DataFolder::default()));
        // `create` with one: in FL's order, and read back.
        let mut p = new_project();
        p.data_folder = Some(DataFolder { choice: DataFolder::RELATIVE, folder: "C:\\Work\\".into() });
        let f4 = reparse(&create(&p).unwrap());
        assert_eq!(info_ids(&f4), [id::AUTHOR, id::INFO_167, id::INFO_202, id::COMMENTS, id::WORK_TIME]);
        assert_eq!(load(&f4).unwrap().data_folder, p.data_folder);
    }

    /// Event 237 (start and time spent, two doubles in days) and event 10 (show info on opening): `create` writes the
    /// model's, or now and 0, and 10 behind 30; they read back; `apply` patches them in place, or adds them where FL
    /// writes them: 10 behind the project events, 237 behind the texts (spec: flp/events/project-info.md,
    /// flp/engine-events.md "Order in the file").
    #[test]
    fn work_time_and_show_info_round_trip() {
        let w = WorkTime { started: 42_139.675_694_444_4, spent: 6.122_916_666_7 };
        let mut p = new_project();
        p.play_truncated = true;
        p.work_time = Some(w);
        p.show_info = Some(true);
        let file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let m = load(&file).unwrap();
        assert_eq!((m.work_time, m.show_info), (Some(w), Some(true)));
        let mut bytes = w.started.to_le_bytes().to_vec();
        bytes.extend(w.spent.to_le_bytes());
        assert_eq!(event_data(&file, id::WORK_TIME), Some(bytes));
        let ids: Vec<u8> = events_of(&file).unwrap().iter().map(|e| e.id).collect();
        let at = ids.iter().position(|&i| i == id::SHOW_INFO).unwrap();
        assert_eq!((ids[at - 1], ids[at + 1]), (id::PLAY_TRUNCATED, id::TITLE));

        // Without a start in the model: now, nothing spent.
        p.work_time = None;
        let started = load(&create(&p).unwrap()).unwrap().work_time.expect("written");
        assert_eq!(started.spent, 0.0);
        assert!((started.started - work_time_now().started).abs() < 1.0 / 24.0, "{started:?}");

        // Patched in place.
        let mut m2 = m.clone();
        m2.work_time = Some(WorkTime { spent: 0.0, ..w });
        m2.show_info = Some(false);
        let mut f2 = file.clone();
        apply(&m2, &mut f2).unwrap();
        assert_eq!(events_of(&f2).unwrap().len(), events_of(&file).unwrap().len());
        let back = load(&FlpFile::parse(&f2.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!((back.work_time, back.show_info), (m2.work_time, Some(false)));

        // Added to a file without them.
        let mut f3 = file.clone();
        events_of_mut(&mut f3).unwrap().retain(|e| e.id != id::SHOW_INFO && e.id != id::WORK_TIME);
        let mut m3 = load(&f3).unwrap();
        assert_eq!((m3.work_time, m3.show_info), (None, None));
        m3.work_time = Some(w);
        m3.show_info = Some(true);
        m3.info.url = Some("https://example.org".into());
        apply(&m3, &mut f3).unwrap();
        let f3 = FlpFile::parse(&f3.to_bytes().unwrap()).unwrap();
        let ids: Vec<u8> = events_of(&f3).unwrap().iter().map(|e| e.id).collect();
        let at = ids.iter().position(|&i| i == id::SHOW_INFO).unwrap();
        assert_eq!(ids[at - 1..at + 7], [id::PLAY_TRUNCATED, id::SHOW_INFO, id::TITLE, id::GENRE, id::AUTHOR, id::COMMENTS, id::URL, id::WORK_TIME]);
        assert_eq!(load(&f3).unwrap(), m3);
    }

    /// Every marker action is the high byte of event 148, and a key marker's scale is event 101 (spec:
    /// flp/events/playlist.md "Marker kinds"): they survive a save, also when another marker changes and the group is
    /// written again; kinds the Action menu does not list are kept.
    #[test]
    fn arrangement_marker_kinds_and_scale_round_trip() {
        let mut p = new_project();
        let mut arr = default_arrangement();
        arr.clips.push(Clip::new(0, 1920, 0, ClipSource::Pattern(1)));
        let kinds = [0u8, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12];
        for (i, &k) in kinds.iter().enumerate() {
            arr.markers.push(TimeMarker { kind: k, ..TimeMarker::new(96 * i as u32, &format!("K{k}")) });
        }
        arr.markers[2].name = " ".into();
        let key = arr.markers.iter_mut().find(|m| m.kind == TimeMarker::KEY).unwrap();
        key.name = "C Major (Ionian)".into();
        key.scale = 0x52A;
        p.arrangements.push(arr);
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back.arrangements[0].markers, p.arrangements[0].markers);
        let ev = events_of(&file).unwrap();
        let scales: Vec<u32> = ev.iter().filter(|e| e.id == id::MARKER_SCALE).map(int).collect();
        assert_eq!(scales.iter().filter(|&&v| v == 0x52A).count(), 1);
        let pos: Vec<u32> = ev.iter().filter(|e| e.id == id::MARKER_POS).map(int).collect();
        assert!(pos.contains(&(u32::from(TimeMarker::PUNCH_OUT) << 24 | 96 * 9)), "{pos:x?}");

        // A changed action rewrites every group; the key marker keeps its scale.
        let mut edited = back.clone();
        edited.arrangements[0].markers[0].kind = TimeMarker::START;
        apply(&edited, &mut file).unwrap();
        let again = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(again.arrangements[0].markers, edited.arrangements[0].markers);
        assert_eq!(again.arrangements[0].markers.iter().find(|m| m.kind == TimeMarker::KEY).map(|m| m.scale), Some(0x52A));
    }

    /// A pattern's own time markers (piano-roll Alt+T) are marker groups in its metadata block, sorted by position
    /// together with the "Length" marker, before 164 (spec: flp/events/patterns.md, metadata block).
    #[test]
    fn pattern_markers_round_trip() {
        let mut file = FlpFile::parse(&create(&new_project()).unwrap().to_bytes().unwrap()).unwrap();
        let mut m = load(&file).unwrap();
        m.patterns[0].markers = vec![TimeMarker::new(0, "Pmark"), TimeMarker::new(192, "Mid"), TimeMarker::new(768, "Late")];
        apply(&m, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back, m);
        let ev = events_of(&file).unwrap();
        let meta = ev.iter().position(|e| e.id == id::PAT_START).unwrap();
        let block: Vec<(u8, u32)> = ev[meta..]
            .iter()
            .take_while(|e| e.id != id::PAT_LENGTH)
            .filter(|e| e.id == id::MARKER_POS)
            .map(|e| (e.id, int(e)))
            .collect();
        assert_eq!(block, vec![(148, 0), (148, 192), (148, 768)]);
        let mut again = file.clone();
        apply(&back, &mut again).unwrap();
        assert_eq!(again.to_bytes().unwrap(), file.to_bytes().unwrap(), "an unchanged model writes nothing");

        // With a length, the "Length" marker sits among them by position and doesn't come back as a marker.
        let mut with_len = back.clone();
        with_len.patterns[0].length = Some(384);
        let mut file2 = file.clone();
        apply(&with_len, &mut file2).unwrap();
        let back2 = load(&FlpFile::parse(&file2.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back2, with_len);
        let pos: Vec<u32> = events_of(&file2).unwrap().iter().filter(|e| e.id == id::MARKER_POS).map(int).collect();
        assert_eq!(pos, vec![0, 192, 6 << 24 | 384, 768]);

        // Removing them all leaves no marker group.
        let mut none = back2.clone();
        none.patterns[0].markers.clear();
        none.patterns[0].length = None;
        apply(&none, &mut file2).unwrap();
        let file2 = FlpFile::parse(&file2.to_bytes().unwrap()).unwrap();
        assert_eq!(load(&file2).unwrap(), none);
        assert!(!events_of(&file2).unwrap().iter().any(|e| e.id == id::MARKER_POS));

        // A pattern with nothing but a marker (no blocks in the file yet) gets its metadata block.
        let mut empty = none.clone();
        empty.patterns.push(Pattern { number: 7, markers: vec![TimeMarker::new(96, "Here")], ..Pattern::default() });
        let mut file3 = file2.clone();
        apply(&empty, &mut file3).unwrap();
        let back3 = load(&FlpFile::parse(&file3.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(back3.patterns.iter().find(|p| p.number == 7).map(|p| p.markers.clone()), Some(empty.patterns.last().unwrap().markers.clone()));

        // A new file built from the model keeps them too.
        let mut fresh = new_project();
        fresh.patterns[0].markers = vec![TimeMarker::new(48, "New")];
        let made = load(&FlpFile::parse(&create(&fresh).unwrap().to_bytes().unwrap()).unwrap()).unwrap();
        let n = fresh.patterns[0].number;
        assert_eq!(made.patterns.iter().find(|p| p.number == n).unwrap().markers, fresh.patterns[0].markers);
    }

    /// Automation clips on plugin parameters (a channel plugin's, a mixer slot's) are written as 227 links with the
    /// parameter target and read back; moving the channel renumbers the channel-plugin target and keeps its index
    /// (spec: plugins/plugin-parameters.md §3, §7).
    #[test]
    fn plugin_parameter_automation_round_trips() {
        use os_model::target::{channel_param, decode, slot_param, Target};
        let mut p = new_project();
        for (i, name) in [(2u16, "Kick - Band 1 level"), (3, "EQ - Insert 1 - Band 2 freq")] {
            p.channels.push(Channel { kind: ChannelKind::Automation, ..sampler(i, name, 0) });
        }
        p.automations.push(new_automation(2, channel_param(1, 35), 0.5, 4.0));
        p.automations.push(new_automation(3, slot_param(1, 0, 8), 0.25, 4.0));
        let mut arr = default_arrangement();
        arr.clips = vec![Clip::new(0, 384, 0, ClipSource::Channel(2)), Clip::new(0, 384, 1, ClipSource::Channel(3))];
        p.arrangements = vec![arr];
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back.automations, p.automations);
        let link = events_of(&file).unwrap().iter().filter(|e| e.id == id::AUTO_LINK).filter_map(|e| e.data()).map(|d| d.to_vec()).collect::<Vec<_>>();
        assert_eq!(&link[1][8..12], &0x7040_8008u32.to_le_bytes());
        // Swap the two sampler channels: the plugin target follows its channel.
        let mut m = back.clone();
        m.reorder_channels(&[1, 0, 2, 3]);
        apply(&m, &mut file).unwrap();
        let again = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(again.automations[0].target().map(decode), Some(Target::ChannelParam(0, 35)));
        assert_eq!(again.automations[1].target().map(decode), Some(Target::SlotParam(1, 0, 8)));
    }

    /// A clip with several targets has one link each: all are read in file order, an unchanged model writes the file
    /// back byte for byte, and changed targets patch, add or remove links; controller links stay as they are (spec:
    /// flp/events/automation.md "Event 227").
    #[test]
    fn several_targets_of_one_clip_round_trip() {
        use os_model::target::{channel, mixer, MIXER_VOLUME};
        let mut p = new_project();
        p.channels.push(Channel { kind: ChannelKind::Automation, ..sampler(2, "Sidechain", 0) });
        let mut a = new_automation(2, mixer(1, MIXER_VOLUME), 0.5, 4.0);
        a.targets.extend([mixer(2, MIXER_VOLUME), channel(0, 0)]);
        p.automations.push(a);
        let mut arr = default_arrangement();
        arr.clips = vec![Clip::new(0, 384, 0, ClipSource::Channel(2))];
        p.arrangements = vec![arr];
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        // A controller link (first field 0x8000 and up) before them.
        let events = events_of_mut(&mut file).unwrap();
        let first = events.iter().position(|e| e.id == id::AUTO_LINK).unwrap();
        let mut ctl = link_event(0, 0x1234_5678).data().unwrap().to_vec();
        ctl[0..2].copy_from_slice(&0x8001u16.to_le_bytes());
        events.insert(first, Event::new(id::AUTO_LINK, Value::var(ctl.clone())));
        let bytes = file.to_bytes().unwrap();
        let file = FlpFile::parse(&bytes).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back.automations[0].targets, p.automations[0].targets);
        let mut same = file.clone();
        apply(&back, &mut same).unwrap();
        assert_eq!(same.to_bytes().unwrap(), bytes, "unchanged: byte for byte");
        let links = |f: &FlpFile| {
            events_of(f).unwrap().iter().filter(|e| e.id == id::AUTO_LINK).filter_map(|e| e.data()).map(|d| d.to_vec()).collect::<Vec<_>>()
        };
        // Fewer targets (the third link goes), more (new links follow the clip's last one), none.
        let more = vec![mixer(1, MIXER_VOLUME), channel(1, 0), mixer(3, MIXER_VOLUME), mixer(4, MIXER_VOLUME)];
        for targets in [vec![mixer(1, MIXER_VOLUME), channel(1, 0)], more, vec![]] {
            let mut m = back.clone();
            m.automations[0].targets = targets.clone();
            let mut f = file.clone();
            apply(&m, &mut f).unwrap();
            let f = FlpFile::parse(&f.to_bytes().unwrap()).unwrap();
            assert_eq!(load(&f).unwrap().automations[0].targets, targets);
            let l = links(&f);
            assert_eq!(l.len(), targets.len() + 1);
            assert_eq!(l[0], ctl, "the controller link is left alone");
            assert!(l[1..].iter().all(|d| d[2..4] == 2u16.to_le_bytes()));
        }
        assert_eq!(back.controller_links, vec![os_model::ControllerLink::from_bytes(&ctl).unwrap()]);
    }

    /// Controller links are read in file order with their bytes as they are; a new file holds them; changed links patch
    /// their events, new ones follow the last, links gone are removed, and the automation links stay (spec:
    /// flp/events/automation.md "Controller links").
    #[test]
    fn controller_links_round_trip() {
        use os_model::target::{mixer, slot_param, MIXER_VOLUME};
        use os_model::ControllerLink;
        let mut p = new_project();
        p.channels.push(Channel { kind: ChannelKind::Automation, ..sampler(2, "Clip", 0) });
        p.automations.push(new_automation(2, mixer(3, MIXER_VOLUME), 0.5, 4.0));
        let mut odd = ControllerLink::new(0x7000 + 2 * 64 + 1, 2, slot_param(1, 0, 5));
        odd.bytes[4..8].copy_from_slice(&0x000D_2000u32.to_le_bytes());
        odd.formula = Some("1-Input*0.8-0.2".into());
        p.controller_links = vec![ControllerLink::new(0x7000 + 64, 1, mixer(2, MIXER_VOLUME)), odd];
        let file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let back = load(&file).unwrap();
        assert_eq!(back.controller_links, p.controller_links);
        assert_eq!(back.controller_links[0].output(), 1);
        assert_eq!(back.controller_links[0].source_slot(), Some((1, 0)));
        assert_eq!(back.controller_links[1].source_slot(), Some((2, 1)));
        assert_eq!(os_model::target::decode(back.controller_links[1].target()), os_model::target::Target::SlotParam(1, 0, 5));
        assert_eq!(back.automations[0].targets, vec![mixer(3, MIXER_VOLUME)]);
        let bytes = file.to_bytes().unwrap();
        let mut same = file.clone();
        apply(&back, &mut same).unwrap();
        assert_eq!(same.to_bytes().unwrap(), bytes, "unchanged: byte for byte");
        // The formula moves with its link, comes and goes with it.
        let mut inverted = ControllerLink::new(0x7000 + 64, 0, mixer(4, MIXER_VOLUME));
        inverted.formula = Some("1-Input".into());
        let (a, b) = (p.controller_links[0].clone(), p.controller_links[1].clone());
        let more = vec![b.clone(), inverted.clone(), a.clone(), inverted];
        for links in [vec![b.clone()], more, vec![a, b], vec![]] {
            let mut m = back.clone();
            m.controller_links = links.clone();
            let mut f = file.clone();
            apply(&m, &mut f).unwrap();
            let again = load(&FlpFile::parse(&f.to_bytes().unwrap()).unwrap()).unwrap();
            assert_eq!(again.controller_links, links);
            assert_eq!(again.automations[0].targets, vec![mixer(3, MIXER_VOLUME)]);
            let n = |id: u8| events_of(&f).unwrap().iter().filter(|e| e.id == id).count();
            assert_eq!(n(id::LINK_FORMULA), links.iter().filter(|c| c.formula.is_some()).count(), "no formula left over");
        }
    }

    /// Deleting, cloning and moving channels rewrites the blocks in model order and renumbers notes, control
    /// points, automation links and playlist clips (spec: flp/events/channels.md "Deleting, cloning and moving").
    #[test]
    fn deleted_cloned_and_moved_channels_are_written() {
        let mut p = new_project();
        p.channels.push(sampler(2, "Hat", 3));
        p.channels.push(Channel { kind: ChannelKind::Automation, ..sampler(3, "Hat - Channel volume", 0) });
        p.automations.push(new_automation(3, os_model::target::channel(2, 0), 0.5, 4.0));
        for pat in &mut p.patterns {
            pat.notes.push(Note::step(2, 24));
        }
        p.patterns[0].controls.push(ControlPoint::new(24, 2, ControlPoint::VOLUME, 100));
        let mut arr = default_arrangement();
        arr.clips = vec![Clip::new(0, 384, 0, ClipSource::Pattern(1)), Clip::new(0, 384, 1, ClipSource::Channel(3))];
        p.arrangements = vec![arr];
        let mut file = FlpFile::parse(&create(&p).unwrap().to_bytes().unwrap()).unwrap();
        let original = load(&file).unwrap();

        // Hat first, Kick, a clone of Kick, the automation clip; Bass deleted.
        let mut m = original.clone();
        m.reorder_channels(&[2, 0, 0, 3]);
        m.channels[2].name = Some("Kick #2".into());
        assert_eq!(m.channels[2].rack.origin, Some(0));
        apply(&m, &mut file).unwrap();
        let file = FlpFile::parse(&file.to_bytes().unwrap()).unwrap();
        assert_eq!(file.header().unwrap().channel_count(), Some(4));
        let back = load(&file).unwrap();
        assert_eq!(back, settled(m.clone()));
        let names: Vec<_> = back.channels.iter().map(|c| c.name.clone().unwrap_or_default()).collect();
        assert_eq!(names, ["Hat", "Kick", "Kick #2", "Hat - Channel volume"]);
        assert_eq!(back.automations[0].channel, 3);
        assert_eq!(back.automations[0].targets, vec![os_model::target::channel(0, 0)]);
        assert!(back.arrangements[0].clips.iter().any(|c| c.source == ClipSource::Channel(3)));
        let count = |p: &Project, ch: u16| p.patterns.iter().flat_map(|p| &p.notes).filter(|n| n.channel == ch).count();
        assert_eq!(count(&back, 2), 0, "a clone has no notes");
        assert_eq!(count(&back, 1), count(&original, 0), "the kick notes followed the kick");
        assert_eq!(back.patterns.iter().map(|p| p.notes.len()).sum::<usize>(), 4, "the bass note went");
        // Writing the restructured model again is a no-op.
        let mut again = file.clone();
        apply(&back, &mut again).unwrap();
        assert_eq!(again.to_bytes().unwrap(), file.to_bytes().unwrap());
    }
}
