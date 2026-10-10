//! Typed view of an FLP event stream: [`load`] maps events to an [`os_model::Project`],
//! [`apply`] writes model values back into the original stream in place, so everything the
//! model doesn't cover (unknown events, plugin states, …) is preserved and an unedited
//! project re-serializes byte-for-byte.
//!
// spec: flp/events/project-info.md, flp/events/channels.md, flp/events/patterns.md,
//       flp/events/mixer.md, flp/events/playlist.md

use std::collections::BTreeMap;

use os_model::{
    Arrangement, AutoMode, AutoPoint, Automation, Channel, ChannelKind, Clip, ClipSource, ControlPoint, Envelope, MixerTrack,
    Note, Pattern, PlaylistTrack, Project, ProjectInfo, Rgb, SlotPlugin, TimeMarker, CLIP_RECORD_LEN,
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
    pub const DISPLAY_NAME: u8 = 203;
    pub const COLOR: u8 = 128;
    pub const SAMPLE_PATH: u8 = 196;

    pub const PAT_START: u8 = 65;
    pub const PAT_NOTES: u8 = 224;
    /// Channel control points (12-byte records) between 65 and 224.
    pub const PAT_CONTROLS: u8 = 223;
    pub const PAT_NAME: u8 = 193;
    pub const PAT_COLOR: u8 = 150;

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
    /// (OpenStudio-specs REQUESTS.md #4).
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
    color: Option<usize>,
    color_flag: Option<usize>,
    enabled: Option<usize>,
    levels: Option<usize>,
    target: Option<usize>,
    auto: Option<usize>,
    /// Event 196 (sample path) and event 20, which it follows (spec: flp/events/channels.md, row 40).
    sample: Option<usize>,
    ev20: Option<usize>,
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
}

#[derive(Debug, Default)]
struct PlTrackLoc {
    record: usize,
    color_flag: Option<usize>,
    name: Option<usize>,
}

#[derive(Debug, Default)]
struct ArrangementLoc {
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
    channels: Vec<ChannelLoc>,
    patterns: BTreeMap<u16, PatternLoc>,
    /// Index just past the last pattern metadata block.
    meta_end: Option<usize>,
    arrangements: Vec<ArrangementLoc>,
    mixer: Vec<MixerLoc>,
    mixer_params: Option<usize>,
    /// Automation links (event 227 with a zero first field), in the prelude.
    links: Vec<usize>,
}

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
                } else if has_controls {
                    // A block of control points only: nothing else belongs to it.
                } else {
                    // Metadata block: runs until the next block start or region change.
                    loc.meta_start = Some(i);
                    is_meta = true;
                    while let Some(n) = events.get(k) {
                        match n.id {
                            id::PAT_NAME => loc.name = Some(k),
                            id::PAT_COLOR => loc.color = Some(k),
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
                _ => {}
            },
            (Region::Channels | Region::Arrangements, id::ARR_START) => {
                region = Region::Arrangements;
                l.arrangements.push(ArrangementLoc::default());
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
            id::COLOR => c.color = Some(k),
            id::CH_COLOR_FLAG => c.color_flag = Some(k),
            id::CH_ENABLED if c.enabled.is_none() => c.enabled = Some(k),
            id::CH_LEVELS => c.levels = Some(k),
            id::CH_TARGET => c.target = Some(k),
            id::AUTO_DATA => c.auto = Some(k),
            id::SAMPLE_PATH => c.sample = Some(k),
            20 => c.ev20 = Some(k),
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
        ..Default::default()
    };

    for loc in &l.channels {
        let ch = load_channel(events, loc, enc);
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
        // A clip may drive several targets; the model holds the first (the others stay in the file).
        if let Some(a) = p.automations.iter_mut().find(|a| a.channel == ch && a.target.is_none()) {
            a.target = Some(target);
        }
    }
    for (&number, loc) in &l.patterns {
        p.patterns.push(Pattern {
            number,
            name: text(loc.name),
            color: loc.color.map(|i| int(&events[i])).filter(|v| v >> 24 == 0xFF).map(rgb_from_bgr),
            notes: loc.notes.and_then(|i| events[i].data()).map(decode_notes).unwrap_or_default(),
            controls: loc.controls.and_then(|i| events[i].data()).map(decode_controls).unwrap_or_default(),
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
    }
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
    Automation { channel, target: Some(target), points: vec![first, AutoPoint::new(beats.max(0.0), value)], header, tail }
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
    Some(Automation { channel, target: None, points, header, tail: d[end..].to_vec() })
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

fn mixer_location(track: u16, slot: u16) -> u16 {
    0x7000 + track * 64 + slot
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
        let param = |pid: u8| {
            mixer_param_records(params)
                .find(|&(_, p, k, loc, _)| k == MX_KIND_PARAM && p == pid && loc == mixer_location(index, 0))
                .map(|r| r.4)
        };
        out.push(MixerTrack {
            index,
            name: m.name.and_then(|i| events[i].data()).map(|d| decode_text(d, enc)),
            color: m.color.filter(|_| custom).map(|i| rgb_from_bgr(int(&events[i]))),
            enabled: m
                .flags
                .and_then(|i| events[i].data())
                .and_then(|d| d.get(4))
                .is_none_or(|&b| b & 0x08 != 0),
            slots,
            routes,
            send_levels: mixer_param_records(params)
                .filter(|&(_, _, k, loc, _)| k == MX_KIND_ROUTE && loc == mixer_location(index, 0))
                .map(|(_, dest, _, _, v)| (u16::from(dest), f64::from(v) / 16000.0))
                .collect(),
            volume: param(MX_VOLUME).map(|v| f64::from(v) / 16000.0),
            pan: param(MX_PAN).map(|v| f64::from(v) / 6400.0),
            stereo_separation: param(MX_STEREO).map(|v| f64::from(v) / 64.0),
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
    if !skip_arrangements && project.arrangements.len() != current.arrangements.len() {
        return Err(mismatch("arrangements", project.arrangements.len(), current.arrangements.len()));
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

    // Channels.
    for (loc, (ch, old)) in l.channels.iter().zip(project.channels.iter().zip(&current.channels)) {
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
        let Some(target) = a.target else { continue };
        let link = l.links.iter().copied().find(|&i| events[i].data().is_some_and(|d| u16::from_le_bytes([d[2], d[3]]) == a.channel));
        match link {
            Some(i) => patch_bytes(&mut events[i], 8, &target.to_le_bytes()),
            None => {
                let at = l.channels.first().map_or(0, |c| c.start);
                inserts.push((at, link_event(a.channel, target)));
            }
        }
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
        let new_block = !has_block && (!pat.notes.is_empty() || !pat.controls.is_empty());
        if new_block {
            new_note_blocks.push((pat.number, note_block_events(pat)));
        }
        if has_block && loc.is_some_and(|l| l.notes.is_none()) && !pat.notes.is_empty() {
            // A control-points-only block gains notes right after its points.
            if let Some(c) = loc.and_then(|l| l.controls) {
                inserts.push((c + 1, Event::new(id::PAT_NOTES, Value::var(encode_notes(&pat.notes)))));
            }
        }
        if !new_block && old.map(|o| &o.notes) != Some(&pat.notes) {
            let Some(i) = loc.and_then(|l| l.notes) else {
                if pat.notes.is_empty() || has_block {
                    continue;
                }
                return Err(ProjectError::Mismatch(format!("adding notes to pattern {} (no note block)", pat.number)));
            };
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
        let meta_start = loc.and_then(|l| l.meta_start);
        if meta_start.is_none() && (pat.name.is_some() || pat.color.is_some()) {
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

    // Playlist clips, time markers and tracks.
    for ((arr, old_arr), loc) in project.arrangements.iter().zip(&current.arrangements).zip(&l.arrangements) {
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
    for ((m, old), ml) in project.mixer.iter().zip(&current.mixer).zip(&l.mixer) {
        if m.enabled != old.enabled {
            if let Some(i) = ml.flags {
                if let Some(&b) = events[i].data().and_then(|d| d.get(4)) {
                    let b = if m.enabled { b | 0x08 } else { b & !0x08 };
                    patch_bytes(&mut events[i], 4, &[b]);
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
                (Some(_), None) => {
                    return Err(ProjectError::Mismatch("removing a mixer track name".into()));
                }
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
                let loc = mixer_location(m.index, 0);
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
    if !pat.notes.is_empty() {
        block.push(Event::new(id::PAT_NOTES, Value::var(encode_notes(&pat.notes))));
    }
    block
}

/// A pattern's metadata block with the spec's defaults for the fields OpenStudio doesn't edit.
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
        Event::new(164, Value::U32(0)),
        Event::new(52, Value::U8(0)),
        Event::new(171, Value::U32(0)),
        Event::new(252, Value::var(vec![0, 0])),
        Event::new(253, Value::var(encode_text("0,0,0,0,0,0,0,0,0,0,0,0", enc))),
    ]);
    block
}

/// Version OpenStudio writes as "saved by" (spec: flp/events/project-info.md; FL 26 opens it without a warning).
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
    if matches!(ch.kind, ChannelKind::Plugin | ChannelKind::Other(_)) {
        return Err(ProjectError::Mismatch(format!(
            "channel {}: creating plugin or unknown channels is not supported yet",
            ch.index
        )));
    }
    if ch.kind == ChannelKind::Automation && auto.is_none() {
        return Err(ProjectError::Mismatch(format!("automation channel {} has no automation data", ch.index)));
    }
    let mut b = vec![
        Event::new(id::CH_START, Value::U16(ch.index)),
        Event::new(id::CH_TYPE, Value::U8(channel_type_code(ch.kind))),
        Event::new(id::PLUGIN_NAME, Value::var(encode_text("", enc))),
    ];
    if let Some(n) = &ch.name {
        b.push(Event::new(id::DISPLAY_NAME, Value::var(encode_text(n, enc))));
    }
    let color = ch.color.map_or(CHANNEL_COLOR_DEFAULT, |c| bgr_from_rgb(clamp_channel_color(c), 0));
    b.push(Event::new(id::COLOR, Value::U32(color)));
    b.push(Event::new(id::CH_COLOR_FLAG, Value::U8(u8::from(ch.color.is_some()))));
    b.push(Event::new(id::CH_ENABLED, Value::U8(u8::from(ch.enabled))));
    b.push(Event::new(id::CH_TARGET, Value::U16(ch.mixer_track)));
    for child in &ch.layer_children {
        b.push(Event::new(id::CH_LAYER_CHILD, Value::U16(*child)));
    }
    let mut levels = [0u8; 24];
    levels[0..4].copy_from_slice(&(((ch.pan + 1.0) * 6400.0).round() as i32).to_le_bytes());
    levels[4..8].copy_from_slice(&((ch.volume * 12800.0).round() as i32).to_le_bytes());
    levels[8..12].copy_from_slice(&ch.pitch_cents.to_le_bytes());
    levels[13] = 1; // the template's `00 01 00 00 …` tail
    b.push(Event::new(id::CH_LEVELS, Value::var(levels.to_vec())));
    if let Some(p) = &ch.sample_path {
        b.push(Event::new(id::SAMPLE_PATH, Value::var(encode_text(p, enc))));
    }
    if let Some(a) = auto.filter(|_| ch.kind == ChannelKind::Automation) {
        b.push(Event::new(id::AUTO_DATA, Value::var(encode_automation(a))));
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
    let mut patterns: Vec<&Pattern> = project.patterns.iter().collect();
    patterns.sort_by_key(|p| p.number);
    for pat in &patterns {
        if !pat.notes.is_empty() || !pat.controls.is_empty() {
            ev.extend(note_block_events(pat));
        }
    }
    for a in &project.automations {
        if let Some(t) = a.target {
            ev.push(link_event(a.channel, t));
        }
    }
    for (i, ch) in project.channels.iter().enumerate() {
        ev.extend(channel_block_events(ch, project.automation(ch.index), enc)?);
        if i == 0 {
            for pat in &patterns {
                if pat.name.is_some() || pat.color.is_some() {
                    ev.extend(meta_block_events(pat, enc));
                }
            }
        }
    }
    for arr in &project.arrangements {
        ev.extend(arrangement_events(arr, enc));
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
    r[29] = 5;
    r[37] = 1;
    r[51..59].fill(0xff);
    r[59] = 1;
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
        let t = arr.tracks.iter().find(|t| t.number == n).cloned().unwrap_or(PlaylistTrack { number: n, name: None, color: None, enabled: true });
        ev.push(Event::new(id::PL_TRACK, Value::var(track_record(&t))));
        ev.push(Event::new(id::PL_COLOR_FLAG, Value::U8(u8::from(t.color.is_some()))));
        if let Some(name) = &t.name {
            ev.push(Event::new(id::PL_NAME, Value::var(encode_text(name, enc))));
        }
    }
    ev
}

/// A fresh arrangement as FL's template has it: number 0, "Arrangement", 500 default tracks.
pub fn default_arrangement() -> Arrangement {
    Arrangement {
        number: 0,
        name: Some("Arrangement".into()),
        tracks: (1..=PLAYLIST_TRACKS).map(|n| PlaylistTrack { number: n, name: None, color: None, enabled: true }).collect(),
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
    if current.arrangements.is_empty() && project.arrangements.iter().any(|a| !a.clips.is_empty()) {
        parts.push("playlist clips");
    }
    parts
}

/// Tracks as a fresh project has them: unnamed, uncolored, unity volume (0.8), centered, routed to the master.
fn mixer_is_default(mixer: &[MixerTrack]) -> bool {
    mixer.iter().all(|t| {
        t.name.is_none()
            && t.color.is_none()
            && t.enabled
            && t.slots.iter().all(Option::is_none)
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
            color: None,
            enabled: true,
            volume: 10000.0 / 12800.0,
            pan: 0.0,
            pitch_cents: 0,
            mixer_track: track,
            sample_path: None,
            layer_children: vec![],
            envelopes: Default::default(),
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
                Pattern { number: 2, name: Some("Verse".into()), color: Some(Rgb::new(10, 20, 30)), notes: vec![Note::step(0, 48), Note::step(1, 0)], controls: vec![ControlPoint::new(0, 1, ControlPoint::PAN, 3200)] },
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

    #[test]
    fn unsaved_parts_name_what_a_created_file_lacks() {
        use os_model::MixerTrack;
        let mut p = new_project();
        let file = create(&p).unwrap();
        assert!(unsaved_parts(&p, &file).is_empty());
        let track = |i: u16| MixerTrack {
            index: i, name: None, color: None, enabled: true, slots: Default::default(),
            routes: if i == 0 { vec![] } else { vec![0] }, send_levels: vec![], volume: Some(0.8), pan: Some(0.0), stereo_separation: Some(0.0),
        };
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
        a0.markers.push(TimeMarker::new(0, "Intro"));
        a0.tracks[3].enabled = false;
        a0.tracks[3].name = Some("Drums".into());
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
        assert_eq!(again.arrangements[0].tracks[3], edited.arrangements[0].tracks[3]);
        let mut want = edited.arrangements[0].clips.clone();
        want.sort_by_key(|c| c.position);
        let strip = |v: &[Clip]| v.iter().map(|c| (c.position, c.length, c.track, c.source, c.muted, c.window)).collect::<Vec<_>>();
        assert_eq!(strip(&again.arrangements[0].clips), strip(&want));
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
}
