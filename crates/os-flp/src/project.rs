//! Typed view of an FLP event stream: [`load`] maps events to an [`os_model::Project`],
//! [`apply`] writes model values back into the original stream in place, so everything the
//! model doesn't cover (unknown events, plugin states, …) is preserved and an unedited
//! project re-serializes byte-for-byte.
//!
// spec: flp/events/project-info.md, flp/events/channels.md, flp/events/patterns.md,
//       flp/events/mixer.md, flp/events/playlist.md

use std::collections::BTreeMap;

use os_model::{
    Arrangement, AutoMode, AutoPoint, Automation, Channel, ChannelKind, ChannelMisc, ChannelRack, ChannelSound, Clip, ClipSource, ControlPoint,
    Envelope, MixerTrack,
    Note, Pattern, PlaylistTrack, PlaylistView, Project, ProjectInfo, Rgb, SlotPlugin, TimeMarker, CLIP_RECORD_LEN,
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
    /// Time signature and "play truncated notes" (spec: flp/engine-events.md "Project").
    pub const TIME_NUM: u8 = 17;
    pub const TIME_DEN: u8 = 18;
    pub const PLAY_TRUNCATED: u8 = 30;
    /// The project's main pitch, cents as a signed 16-bit value (spec: flp/engine-events.md "Project").
    pub const MAIN_PITCH: u8 = 80;
    /// Channel fine tune, cents (spec: audio/voice-levels.md).
    pub const CH_FINE: u8 = 142;
    /// Channel settings the engine reads (spec: flp/engine-events.md "A channel").
    pub const CH_SWING_SHARE: u8 = 97;
    pub const CH_ROOT: u8 = 135;
    pub const CH_SAMPLER_FLAGS: u8 = 143;
    pub const CH_SAMPLE_SETTINGS: u8 = 215;
    pub const CH_ADJUST: u8 = 229;
    /// Cut groups (word 0 cuts, word 1 cut by) and the voice settings: maximum voices, slide time, mono / portamento
    /// (spec: audio/note-rules.md "Settings").
    pub const CH_CUT_GROUPS: u8 = 132;
    pub const CH_LAYER_FLAGS: u8 = 144;
    pub const CH_POLY: u8 = 221;
    pub const GROUP_NAME: u8 = 231;
    pub const CURRENT_GROUP: u8 = 146;

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
    pub const MARKER_IDS: [u8; 8] = [148, 33, 34, 46, 45, 168, 101, 205];
    /// Automation clip points (in the channel block) and links (before the first channel)
    /// (spec: flp/events/automation.md).
    pub const AUTO_DATA: u8 = 234;
    pub const AUTO_LINK: u8 = 227;

    pub const MX_COLOR: u8 = 149;
    pub const MX_COLOR_FLAG: u8 = 42;
    pub const MX_NAME: u8 = 204;
    pub const MX_FLAGS: u8 = 236;
    pub const MX_SLOT: u8 = 98;
    pub const MX_ROUTES: u8 = 235;
    pub const MX_PARAMS: u8 = 225;
    /// Number of stored mixer tracks, just before the first track (spec: flp/events/mixer.md).
    pub const MX_COUNT: u8 = 103;
}

const NOTE_LEN: usize = 24;
const CLIP_LEN: usize = 88;
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
    name: Option<usize>,
    flags: Option<usize>,
    routes: Option<usize>,
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
    swing: Option<usize>,
    /// Time signature and "play truncated notes" (spec: flp/engine-events.md "Project").
    time_num: Option<usize>,
    time_den: Option<usize>,
    truncated: Option<usize>,
    main_pitch: Option<usize>,
    groups: Vec<usize>,
    current_group: Option<usize>,
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
                id::VERSION => l.version = Some(i),
                id::TEMPO => l.tempo = Some(i),
                id::TITLE => l.title = Some(i),
                id::AUTHOR => l.author = Some(i),
                id::GENRE => l.genre = Some(i),
                id::COMMENTS => l.comments = Some(i),
                id::URL => l.url = Some(i),
                id::CURRENT_PATTERN => l.current_pattern = Some(i),
                id::SWING if l.swing.is_none() => l.swing = Some(i),
                id::TIME_NUM if l.time_num.is_none() => l.time_num = Some(i),
                id::TIME_DEN if l.time_den.is_none() => l.time_den = Some(i),
                id::PLAY_TRUNCATED if l.truncated.is_none() => l.truncated = Some(i),
                id::MAIN_PITCH if l.main_pitch.is_none() => l.main_pitch = Some(i),
                id::GROUP_NAME => l.groups.push(i),
                id::CURRENT_GROUP => l.current_group = Some(i),
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

fn text_enc(events: &[Event], version: Option<usize>) -> TextEnc {
    let major = version
        .and_then(|i| events[i].data())
        .map(|d| decode_text(d, TextEnc::Ansi))
        .and_then(|s| s.split('.').next().and_then(|m| m.parse::<u32>().ok()))
        .unwrap_or(26);
    if major <= 11 { TextEnc::Ansi } else { TextEnc::Utf16 }
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
    let text = |i: Option<usize>| i.and_then(|i| events[i].data()).map(|d| decode_text(d, enc));

    let mut p = Project {
        saved_by: l
            .version
            .and_then(|i| events[i].data())
            .map(|d| decode_text(d, TextEnc::Ansi))
            .unwrap_or_default(),
        ppq: header.ppq().unwrap_or(96),
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
        time_signature: l.time_num.zip(l.time_den).map(|(n, d)| (int(&events[n]) as u16, int(&events[d]) as u16)).filter(|&(n, d)| n > 0 && d > 0),
        play_truncated: l.truncated.map(|i| int(&events[i]) != 0),
        main_pitch: l.main_pitch.map(|i| int(&events[i]) as u16 as i16),
        main_volume: main_volume(l.mixer_params.and_then(|i| events[i].data()).unwrap_or(&[])),
        playlist_view: l.playlist_view.and_then(|i| events[i].data()).and_then(PlaylistView::from_bytes),
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
                    id::ARR_CLIPS => arr.clips = f.data().map(decode_clips).unwrap_or_default(),
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
/// volume multiplier, -, mod X offset, mod Y offset), swing share (97, 0..=128), root note (135), fine tune (142) and
/// the main pitch switch (215 byte 11), cut groups (132) and the voice settings (221) (spec: flp/engine-events.md "A
/// channel", audio/voice-levels.md, audio/swing.md, audio/sampler.md, audio/note-rules.md).
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
    if let Some(&b) = find(id::CH_SAMPLE_SETTINGS).and_then(Event::data).and_then(|d| d.get(11)) {
        m.main_pitch = b != 0;
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
    }
    s
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
    let mut raw = [0u8; CLIP_RECORD_LEN];
    raw.copy_from_slice(&r[..CLIP_RECORD_LEN]);
    Clip {
        position: u32_at(0),
        length: u32_at(8),
        track: 499u16.saturating_sub(u16_at(12)),
        source,
        muted: r[19] & CLIP_MUTED != 0,
        window,
        raw,
    }
}

fn decode_clips(data: &[u8]) -> Vec<Clip> {
    data.chunks_exact(CLIP_LEN).map(decode_clip).collect()
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

/// Clip records sorted by position. A clip without an id, or with one already used (a copy), gets the next
/// free id (spec: playlist.md, byte 32).
fn encode_clips(clips: &[Clip]) -> Vec<u8> {
    let mut sorted = clips.to_vec();
    sorted.sort_by_key(|c| c.position);
    let mut next = sorted.iter().map(Clip::id).max().unwrap_or(0) + 1;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(sorted.len() * CLIP_LEN);
    for c in &mut sorted {
        if c.id() == 0 || !seen.insert(c.id()) {
            c.set_id(next);
            seen.insert(next);
            next += 1;
        }
        out.extend_from_slice(&encode_clip(c));
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
                out.push(TimeMarker { position: v & 0x00FF_FFFF, kind: (v >> 24) as u8, name: String::new(), numerator: 4, denominator: 4 });
            }
            id::MARKER_NUM => out.last_mut().into_iter().for_each(|m| m.numerator = int(e) as u8),
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
        Event::new(101, Value::U16(0)),
        Event::new(id::MARKER_NAME, Value::var(encode_text(&m.name, enc))),
    ]
}

const AUTO_HEADER_LEN: usize = 21;
const AUTO_POINT_LEN: usize = 24;

/// A new automation clip's event-234 header (count patched in on writing) and tail (spec: flp/events/automation.md).
fn auto_template(bipolar: bool) -> (Vec<u8>, Vec<u8>) {
    let h = vec![1, 0, 0, 0, 0x40, 0, 0, 0, u8::from(bipolar), 4, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0];
    let mut t = vec![1, 0, 0, 0];
    t.extend([0xff; 16]);
    t.extend([0x80, 0, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0, 0, 5, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0]);
    t.extend([0; 14]);
    t.extend([0xf0, 0x3f]);
    t.extend([0; 8]);
    t.extend([1, 0, 0, 0, 0, 0, 0, 0]);
    t.extend([0xff; 12]);
    t.extend([0xfb, 0xb2]);
    t.extend([0; 18]);
    (h, t)
}

/// A new automation channel's data: a flat line at `value` over `beats` (two points, as FL creates one).
pub fn new_automation(channel: u16, target: u32, value: f64, beats: f64) -> Automation {
    let (header, tail) = auto_template(os_model::target::bipolar(target));
    let mut first = AutoPoint::new(0.0, value);
    first.flag = 0;
    Automation { channel, targets: vec![target], points: vec![first, AutoPoint::new(beats.max(0.0), value)], header, tail }
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
    let mut beat = 0.0;
    let points = d[AUTO_HEADER_LEN..end]
        .chunks_exact(AUTO_POINT_LEN)
        .map(|r| {
            // Positions are stored as the distance from the previous point.
            beat += f64::from_le_bytes(r[0..8].try_into().unwrap());
            AutoPoint {
                beat,
                value: f64::from_le_bytes(r[8..16].try_into().unwrap()),
                tension: f32::from_le_bytes(r[16..20].try_into().unwrap()),
                mode: AutoMode::from_byte(r[20]),
                flag: r[23],
            }
        })
        .collect();
    let mut header = d[..AUTO_HEADER_LEN].to_vec();
    header[17..21].fill(0); // the count is written from the points
    Some(Automation { channel, targets: Vec::new(), points, header, tail: d[end..].to_vec() })
}

fn encode_automation(a: &Automation) -> Vec<u8> {
    let mut d = a.header.clone();
    d.resize(AUTO_HEADER_LEN, 0);
    d[17..21].copy_from_slice(&(a.points.len() as u32).to_le_bytes());
    let mut prev = 0.0;
    for p in &a.points {
        d.extend_from_slice(&(p.beat - prev).max(0.0).to_le_bytes());
        prev = p.beat;
        d.extend_from_slice(&p.value.to_le_bytes());
        d.extend_from_slice(&p.tension.to_le_bytes());
        d.extend([p.mode.byte(), 0, 0, p.flag]);
    }
    d.extend_from_slice(&a.tail);
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

/// The file events of the plugin the model has at (`track`, `slot`): the same plugin in the same slot, else the same
/// plugin (name and state) moved from another slot or track, else a plugin of that name moved within the track.
fn take_run(pool: &mut [SlotRun], track: usize, slot: usize, p: &SlotPlugin) -> Option<Vec<Event>> {
    let pick = [
        pool.iter().position(|r| !r.used && r.track == track && r.slot == slot && r.plugin.name == p.name),
        pool.iter().position(|r| !r.used && r.track == track && r.plugin == *p),
        pool.iter().position(|r| !r.used && r.plugin == *p),
        pool.iter().position(|r| !r.used && r.track == track && r.plugin.name == p.name),
    ]
    .into_iter()
    .flatten()
    .next()?;
    pool[pick].used = true;
    Some(pool[pick].events.clone())
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

fn load_mixer(events: &[Event], l: &Layout, enc: TextEnc) -> Vec<MixerTrack> {
    let params = l.mixer_params.and_then(|i| events[i].data()).unwrap_or(&[]);
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
                    pending_plugin = e.data().map(|d| SlotPlugin { name: decode_text(d, enc), state: Vec::new() });
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
            ..MixerTrack::new(index)
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

/// Writes the model back into `file`, changing only the events that hold modeled values
/// that differ from what's stored. Returns an error when the model's shape (number of
/// channels, patterns with notes, mixer tracks, …) doesn't match the file: adding or
/// removing objects isn't supported yet.
pub fn apply(project: &Project, file: &mut FlpFile) -> Result<(), ProjectError> {
    restructure(project, file)?;
    grow_mixer(project, file)?;
    let current = load(file)?;
    let events = events_of_mut(file)?;
    let l = scan(events)?;
    let enc = text_enc(events, l.version);
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
    let info = &project.info;
    let old_info = &current.info;
    for (loc, value, old) in [
        (l.title, &info.title, &old_info.title),
        (l.author, &info.author, &old_info.author),
        (l.genre, &info.genre, &old_info.genre),
        (l.comments, &info.comments, &old_info.comments),
        (l.url, &info.url, &old_info.url),
    ] {
        if value == old {
            continue;
        }
        let value = if loc == l.comments { value.as_deref().map(comments_to_file) } else { value.clone() };
        match (loc, value) {
            (Some(i), Some(v)) => set_data(&mut events[i], encode_text(&v, enc)),
            (Some(i), None) => set_data(&mut events[i], encode_text("", enc)),
            (None, Some(v)) if !v.is_empty() => {
                return Err(ProjectError::Mismatch("adding a missing project-info field".into()));
            }
            _ => {}
        }
    }
    if let (Some(i), Some(v)) = (l.current_pattern, project.current_pattern.filter(|&v| Some(v) != current.current_pattern)) {
        set_int(&mut events[i], v.into());
    }
    if let (Some(i), Some(v)) = (l.current_arrangement, project.current_arrangement.filter(|&v| Some(v) != current.current_arrangement)) {
        set_int(&mut events[i], v.into());
    }

    // Channel rack settings: main swing, filter groups, current filter (spec: flp/events/project-info.md).
    if let Some(v) = project.swing.filter(|&v| Some(v) != current.swing) {
        match l.swing {
            Some(i) => set_int(&mut events[i], v.into()),
            None if v != 0 => inserts.push((l.tempo.map_or(0, |i| i + 1), Event::new(id::SWING, Value::U8(v)))),
            None => {}
        }
    }
    // Main pitch (event 80): FL writes it right after the swing.
    if let Some(v) = project.main_pitch.filter(|&v| Some(v) != current.main_pitch) {
        match l.main_pitch {
            Some(i) => set_int(&mut events[i], u32::from(v as u16)),
            None if v != 0 => {
                let at = l.swing.or(l.tempo).map_or(0, |i| i + 1);
                inserts.push((at, Event::new(id::MAIN_PITCH, Value::U16(v as u16))));
            }
            None => {}
        }
    }
    let groups_at = l
        .groups
        .first()
        .copied()
        .or(l.current_group)
        .or(l.links.first().copied())
        .or(l.channels.first().map(|c| c.start))
        .unwrap_or(0);
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
        // A sampler's sample (Open Sampler loads one): event 196 replaced, or added right after event 20 where FL
        // writes it (spec: flp/events/channels.md). Clearing a sample is not written yet (the old path stays).
        if ch.sample_path != old.sample_path {
            if let Some(p) = &ch.sample_path {
                match (loc.sample, loc.ev20) {
                    (Some(i), _) => set_data(&mut events[i], encode_text(p, enc)),
                    (None, Some(i)) => inserts.push((i + 1, Event::new(id::SAMPLE_PATH, Value::var(encode_text(p, enc))))),
                    (None, None) => inserts.push((loc.end, Event::new(id::SAMPLE_PATH, Value::var(encode_text(p, enc))))),
                }
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
            .flat_map(|a| arrangement_events(a, enc))
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
                set_data(&mut events[i], encode_clips(&arr.clips));
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
    // Tracks whose slots gained, lost or swapped plugins get their whole slot area rewritten (spec:
    // flp/events/plugins.md §2-3); a plugin that stays (or moves) keeps its own events.
    let slot_names = |t: &MixerTrack| t.slots.iter().map(|s| s.as_ref().map(|s| s.name.clone())).collect::<Vec<_>>();
    let rewrite: Vec<bool> = project.mixer.iter().zip(&current.mixer).map(|(m, old)| slot_names(m) != slot_names(old)).collect();
    if !skip_mixer && rewrite.iter().any(|&r| r) {
        let areas = slot_areas(events, &l);
        let mut pool: Vec<SlotRun> = Vec::new();
        for (t, old) in current.mixer.iter().enumerate().filter(|&(t, _)| rewrite[t]) {
            let Some(area) = areas.get(t).and_then(Option::as_ref) else {
                return Err(ProjectError::Structure(format!("mixer track {t} has no slot area")));
            };
            for (k, (slot, run)) in old.slots.iter().zip(&area.runs).enumerate() {
                if let (Some(p), Some((s, e))) = (slot, run) {
                    pool.push(SlotRun { track: t, slot: k, plugin: p.clone(), events: events[*s..*e].to_vec(), used: false });
                }
            }
        }
        for (t, m) in project.mixer.iter().enumerate().filter(|&(t, _)| rewrite[t]) {
            let area = areas[t].as_ref().expect("checked above");
            let mut block = Vec::new();
            for (k, slot) in m.slots.iter().enumerate() {
                if let Some(p) = slot {
                    let rt = record_track(t as u16, project.mixer.len());
                    block.extend(slot_plugin_events(p, rt, k as u8, take_run(&mut pool, t, k, p), enc));
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
        if (m.enabled, m.polarity, m.swap, m.slots_enabled) != (old.enabled, old.polarity, old.swap, old.slots_enabled) {
            if let Some(i) = ml.flags {
                match track_flags(events[i].data()) {
                    Some(b) => patch_bytes(&mut events[i], 4, &[with_flags(b, m)]),
                    // A 5-byte record (FL 10 / 11) has no flags: it becomes the 8-byte record of FL 12, its latency kept
                    // (spec: flp/layouts.md "A mixer track's record (event 236)").
                    None => {
                        let d = events[i].data().unwrap_or(&[]);
                        let mut r = [0u8; 8];
                        let late = d.get(4).is_some_and(|&b| b != 0);
                        r[..d.len().min(4)].copy_from_slice(&d[..d.len().min(4)]);
                        let base = if m.index == 0 { MX_FLAGS_MASTER } else { MX_FLAGS_INSERT };
                        r[4] = with_flags(base | if late { 0x20 } else { 0 }, m);
                        set_data(&mut events[i], r.to_vec());
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

/// Rebuilds the channel region when the model's channels are not the file's blocks in order (deleted, cloned, moved
/// or inserted channels): kept blocks are copied (a clone copies its source block), new ones are built, indices are
/// renumbered, and every reference to a channel follows it or is dropped with it (spec: flp/events/channels.md
/// "Deleting, cloning and moving channels"). [`apply`] then patches the values as usual.
fn restructure(project: &Project, file: &mut FlpFile) -> Result<(), ProjectError> {
    let events = events_of_mut(file)?;
    let l = scan(events)?;
    let enc = text_enc(events, l.version);
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
                for r in d.chunks_exact(CLIP_LEN) {
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
        markers.push(TimeMarker {
            position: len,
            kind: LENGTH_MARKER_KIND as u8,
            name: LENGTH_MARKER_NAME.into(),
            numerator: 4,
            denominator: 4,
        });
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
    if let Some(v) = project.swing.filter(|&v| v != 0) {
        ev.push(Event::new(id::SWING, Value::U8(v)));
    }
    if let Some(v) = project.main_pitch.filter(|&v| v != 0) {
        ev.push(Event::new(id::MAIN_PITCH, Value::U16(v as u16)));
    }
    if let Some(p) = project.current_pattern {
        ev.push(Event::new(id::CURRENT_PATTERN, Value::U16(p)));
    }
    let info = &project.info;
    for (id, value) in [
        (id::TITLE, info.title.clone()),
        (id::GENRE, info.genre.clone()),
        (id::AUTHOR, info.author.clone()),
        (id::COMMENTS, info.comments.as_deref().map(comments_to_file)),
        (id::URL, info.url.clone()),
    ] {
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            ev.push(Event::new(id, Value::var(encode_text(&v, enc))));
        }
    }
    // Filter groups and the current filter (spec: flp/events/project-info.md), before the note blocks.
    for name in &project.groups {
        ev.push(Event::new(id::GROUP_NAME, Value::var(encode_text(name, enc))));
    }
    if !project.groups.is_empty() {
        ev.push(Event::new(id::CURRENT_GROUP, Value::U32(project.current_group.unwrap_or(u32::MAX))));
    }
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
        ev.extend(arrangement_events(arr, enc));
    }
    // The playlist view and the arrangement shown, right after the last one's tracks as FL writes them
    // (spec: flp/events/playlist.md, events 242 and 100).
    if let Some(v) = project.playlist_view.filter(|_| !project.arrangements.is_empty()) {
        ev.push(Event::new(id::PLAYLIST_VIEW, Value::var(v.raw.to_vec())));
    }
    if let Some(n) = project.current_arrangement.filter(|_| !project.arrangements.is_empty()) {
        ev.push(Event::new(id::CURRENT_ARRANGEMENT, Value::U16(n)));
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
fn arrangement_events(arr: &Arrangement, enc: TextEnc) -> Vec<Event> {
    let mut ev = vec![
        Event::new(id::ARR_START, Value::U16(arr.number)),
        Event::new(id::ARR_NAME, Value::var(encode_text(arr.name.as_deref().unwrap_or("Arrangement"), enc))),
        Event::new(id::ARR_UNKNOWN, Value::U8(0)),
        Event::new(id::ARR_CLIPS, Value::var(encode_clips(&arr.clips))),
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

/// One mixer track block (spec: flp/events/mixer.md "Track blocks"): colour, name, flags, the ten slots, routing and
/// the trailing events with the values untouched tracks have.
fn track_block_events(m: &MixerTrack, last: bool, enc: TextEnc) -> Vec<Event> {
    let mut ev = Vec::new();
    if let Some(c) = m.color {
        ev.push(Event::new(id::MX_COLOR, Value::U32(bgr_from_rgb(c, 0xFF))));
    }
    ev.push(Event::new(id::MX_COLOR_FLAG, Value::U8(u8::from(m.color.is_some()))));
    if let Some(n) = m.name.as_ref().filter(|n| !n.is_empty()) {
        ev.push(Event::new(id::MX_NAME, Value::var(encode_text(n, enc))));
    }
    let mut flags = [0u8; 12];
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
    ev.push(Event::new(165, Value::U32(3)));
    ev.push(Event::new(166, Value::U32(1)));
    ev.push(Event::new(49, Value::U8(0)));
    ev.push(Event::new(154, Value::U32(u32::MAX)));
    ev.push(Event::new(147, Value::U32(if m.index == 0 { 0 } else { u32::MAX })));
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sampler(index: u16, name: &str, track: u16) -> Channel {
        Channel {
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

    /// The events the engine reads beyond the editable settings (spec: flp/engine-events.md): time signature,
    /// "play truncated notes", the channel's swing share, root note, sampler flags, declick and stretch settings,
    /// level adjustments, filter, the envelope records' flags and LFO, cut groups and the voice settings.
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
            let ints = |v: &[i32]| Value::var(v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>());
            let env = [1, -1, 100, 20000, 20000, 30000, 50, 20000, 0, 2000, 3000, -64, 20000, 2, 0, 0, -101];
            let mut add = vec![
                Event::new(id::CH_SWING_SHARE, Value::U16(64)),
                Event::new(id::CH_ROOT, Value::U32(48)),
                Event::new(id::CH_FINE, Value::U32(-35i32 as u32)),
                Event::new(id::CH_SAMPLER_FLAGS, Value::U32(10)),
                Event::new(id::CH_SAMPLE_SETTINGS, Value::var(s215)),
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
        assert_eq!((p.time_signature, p.play_truncated), (Some((6, 8)), Some(true)));
        let c = &p.channels[0];
        assert_eq!((c.misc.swing, c.misc.root), (0.5, 48));
        // Fine tune (event 142) and the main pitch switch (215 byte 11, cleared here).
        assert_eq!((c.misc.fine, c.misc.main_pitch), (-35, false));
        assert_eq!(c.misc.levels, [-0.5, 1.0, 0.5, -1.0]);
        assert_eq!((c.misc.cut, c.misc.cut_by, c.misc.max_poly, c.misc.mono, c.misc.porta), (7, 9, 3, true, true));
        assert_eq!(c.misc.slide, 820.0 / 1024.0);
        assert_eq!((c.sound.cutoff, c.sound.resonance, c.sound.filter_type), (128, 64, 5));
        assert_eq!((c.sound.sampler_flags, c.sound.declick, c.sound.stretch), (Some(10), 3, [1536, 25, 10000, 5]));
        let e = c.envelopes[Envelope::VOLUME];
        assert_eq!((e.enabled, e.flags, e.lfo_delay, e.lfo_attack, e.lfo_amount, e.lfo_speed, e.lfo_shape), (true, 1, 2000, 3000, -64, 20000, 2));
        // Files without these events keep the defaults.
        let plain = load(&create(&new_project()).unwrap()).unwrap();
        assert_eq!((plain.time_signature, plain.play_truncated), (None, None));
        assert_eq!((plain.channels[0].misc.swing, plain.channels[0].misc.root, plain.channels[0].sound), (1.0, 60, ChannelSound::default()));
        assert_eq!((plain.channels[0].misc.fine, plain.channels[0].misc.main_pitch), (0, true));
        let m = plain.channels[0].misc;
        assert_eq!((m.cut, m.cut_by, m.max_poly, m.mono, m.porta, m.slide), (0, 0, 0, false, false, 500.0 / 1024.0));
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
        // A file without event 80 gets it after the swing (or the tempo) once the main pitch is set.
        let mut plain = create(&new_project()).unwrap();
        let mut q = load(&plain).unwrap();
        assert_eq!((q.main_pitch, q.main_volume), (None, None));
        q.main_pitch = Some(250);
        apply(&q, &mut plain).unwrap();
        let ev = events_of(&plain).unwrap();
        let at = ev.iter().position(|e| e.id == id::MAIN_PITCH).unwrap();
        assert_eq!(ev[at - 1].id, id::TEMPO);
        assert_eq!(load(&plain).unwrap().main_pitch, Some(250));
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
        let mut edited = back.clone();
        edited.mixer[2].polarity = true;
        apply(&edited, &mut file).unwrap();
        let ev = events_of(&file).unwrap();
        let d = ev.iter().filter(|e| e.id == id::MX_FLAGS).nth(2).unwrap().data().unwrap();
        assert_eq!(d, &[0x00, 0x00, 0xa0, 0x40, 0x4C | 0x20 | 0x01, 0, 0, 0]);
        assert_eq!(load(&file).unwrap().mixer, edited.mixer);
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
        m.slots[0] = Some(SlotPlugin { name: "Fruity Parametric EQ 2".into(), state: vec![1, 2, 3] });
        m.slots[3] = Some(SlotPlugin { name: crate::wrapper::WRAPPER.into(), state: vec![10, 0, 0, 0] });
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
        edited.mixer[4].slots[1] = Some(SlotPlugin { name: "Fruity Limiter".into(), state: vec![] });
        edited.mixer[2].slot_mix[9] = 0.75;
        apply(&edited, &mut file).unwrap();
        assert_eq!(load(&file).unwrap().mixer, edited.mixer);
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
        p.mixer[17].slots[9] = Some(SlotPlugin { name: "Fruity Limiter".into(), state: vec![7; 4] });
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
        grown.mixer[18].slots[0] = Some(SlotPlugin { name: "Fruity Parametric EQ 2".into(), state: vec![1, 2] });
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
        // Fewer tracks than the file is still refused.
        let mut fewer = back.clone();
        fewer.mixer.truncate(19);
        assert!(apply(&fewer, &mut file).is_err());
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
                        assert_eq!(encode_clips(&decode_clips(d)), d, "{path}");
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
