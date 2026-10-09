//! Typed view of an FLP event stream: [`load`] maps events to an [`os_model::Project`],
//! [`apply`] writes model values back into the original stream in place, so everything the
//! model doesn't cover (unknown events, plugin states, …) is preserved and an unedited
//! project re-serializes byte-for-byte.
//!
// spec: flp/events/project-info.md, flp/events/channels.md, flp/events/patterns.md,
//       flp/events/mixer.md, flp/events/playlist.md

use std::collections::BTreeMap;

use os_model::{
    Arrangement, Channel, ChannelKind, Clip, ClipSource, ControlPoint, Envelope, MixerTrack, Note, Pattern,
    PlaylistTrack, Project, ProjectInfo, Rgb, SlotPlugin,
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
    channels: Vec<ChannelLoc>,
    patterns: BTreeMap<u16, PatternLoc>,
    /// Index just past the last pattern metadata block.
    meta_end: Option<usize>,
    arrangements: Vec<ArrangementLoc>,
    mixer: Vec<MixerLoc>,
    mixer_params: Option<usize>,
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
            (Region::Arrangements, id::PL_TRACK) => {
                let arr = l.arrangements.last_mut().expect("in arrangement region");
                arr.tracks.push(PlTrackLoc { record: i, ..Default::default() });
            }
            (Region::Arrangements, id::PL_COLOR_FLAG) => {
                if let Some(t) = l.arrangements.last_mut().and_then(|a| a.tracks.last_mut()) {
                    t.color_flag = Some(i);
                }
            }
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
        ..Default::default()
    };

    for loc in &l.channels {
        p.channels.push(load_channel(events, loc, enc));
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

fn decode_clips(data: &[u8]) -> Vec<Clip> {
    data.chunks_exact(CLIP_LEN)
        .map(|r| {
            let u16_at = |o: usize| u16::from_le_bytes([r[o], r[o + 1]]);
            let u32_at = |o: usize| u32::from_le_bytes(r[o..o + 4].try_into().unwrap());
            let item = u16_at(6);
            Clip {
                position: u32_at(0),
                length: u32_at(8),
                track: 499u16.saturating_sub(u16_at(12)),
                source: if item >= 0x5000 {
                    ClipSource::Pattern(item - 0x5000)
                } else {
                    ClipSource::Channel(item)
                },
            }
        })
        .collect()
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
    if project.channels.len() != current.channels.len() {
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

    // Playlist tracks.
    for ((arr, old_arr), loc) in project.arrangements.iter().zip(&current.arrangements).zip(&l.arrangements) {
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
                    (Some(_), None) => {
                        return Err(ProjectError::Mismatch("removing a playlist track name".into()));
                    }
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
fn channel_block_events(ch: &Channel, enc: TextEnc) -> Result<Vec<Event>, ProjectError> {
    if matches!(ch.kind, ChannelKind::Plugin | ChannelKind::Automation | ChannelKind::Other(_)) {
        return Err(ProjectError::Mismatch(format!(
            "channel {}: creating plugin, automation or unknown channels is not supported yet",
            ch.index
        )));
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
    for (i, ch) in project.channels.iter().enumerate() {
        ev.extend(channel_block_events(ch, enc)?);
        if i == 0 {
            for pat in &patterns {
                if pat.name.is_some() || pat.color.is_some() {
                    ev.extend(meta_block_events(pat, enc));
                }
            }
        }
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
        edited.patterns[0].notes.push(Note::step(0, 96));
        edited.patterns.push(Pattern { number: 3, name: Some("Chorus".into()), notes: vec![Note::step(1, 24)], ..Pattern::default() });
        apply(&edited, &mut file).unwrap();
        let again = load(&FlpFile::parse(&file.to_bytes().unwrap()).unwrap()).unwrap();
        assert_eq!(again.tempo, Some(100.0));
        assert_eq!(again.channels[0].volume, 0.25);
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
